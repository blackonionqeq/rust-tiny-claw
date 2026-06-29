use crate::context_engine::{ContextBudget, ContextCompactor, ContextError, ContextManager};
use crate::memory::{FileMemory, Session};
use crate::provider::{Provider, ProviderError, StdoutStreamSink};
use crate::reporter::terminal::TerminalReporter;
use crate::reporter::{Reporter, ReporterError};
use crate::schema::{Message, ToolCall, ToolResult};
use crate::telemetry::Telemetry;
use crate::tools::ToolRegistry;
use std::fmt;

#[cfg(test)]
mod tests;

pub struct AgentEngine<P> {
    provider: P,
    registry: ToolRegistry,
    context: ContextManager,
    memory: FileMemory,
    telemetry: Telemetry,
}

impl<P> AgentEngine<P>
where
    P: Provider,
{
    pub fn new(
        provider: P,
        registry: ToolRegistry,
        context: ContextManager,
        memory: FileMemory,
        telemetry: Telemetry,
    ) -> Self {
        Self {
            provider,
            registry,
            context,
            memory,
            telemetry,
        }
    }

    pub fn boot_plan(&self, options: RunOptions) -> Vec<String> {
        vec![
            format!("provider: {}", self.provider.name()),
            format!("streaming: {}", options.stream),
            format!("thinking phase: {}", options.enable_thinking),
            format!("plan mode: {}", options.plan_mode),
            format!("tools registered: {}", self.registry.len()),
            format!("context manager: {}", self.context.name()),
            format!("workspace: {}", self.context.work_dir().display()),
            format!("memory root: {}", self.memory.root().display()),
            format!("telemetry: {}", self.telemetry.name()),
            "two-stage ReAct loop available".to_string(),
        ]
    }

    pub fn boot_plan_default(&self) -> Vec<String> {
        self.boot_plan(RunOptions::default())
    }

    pub fn run(&mut self, user_prompt: impl Into<String>) -> Result<Vec<Message>, EngineError> {
        self.run_with_options(user_prompt, RunOptions::default())
    }

    pub fn run_with_options(
        &mut self,
        user_prompt: impl Into<String>,
        options: RunOptions,
    ) -> Result<Vec<Message>, EngineError> {
        let mut reporter = TerminalReporter;
        self.run_with_reporter(user_prompt, options, &mut reporter)
    }

    pub fn run_with_reporter(
        &mut self,
        user_prompt: impl Into<String>,
        options: RunOptions,
        reporter: &mut dyn Reporter,
    ) -> Result<Vec<Message>, EngineError> {
        let session = Session::new("one-shot", self.context.work_dir().clone());
        self.run_session_with_reporter(&session, user_prompt, options, reporter)
    }

    pub fn run_session(
        &mut self,
        session: &Session,
        user_prompt: impl Into<String>,
        options: RunOptions,
    ) -> Result<Vec<Message>, EngineError> {
        let mut reporter = TerminalReporter;
        self.run_session_with_reporter(session, user_prompt, options, &mut reporter)
    }

    pub fn run_session_with_reporter(
        &mut self,
        session: &Session,
        user_prompt: impl Into<String>,
        options: RunOptions,
        reporter: &mut dyn Reporter,
    ) -> Result<Vec<Message>, EngineError> {
        // Hold the per-session run lock for the whole ReAct loop so concurrent
        // requests from the same chat cannot reorder history or tool results.
        let _run_guard = session.lock_run();
        session.append(Message::user(user_prompt));

        for turn in 1..=options.max_turns {
            reporter.on_turn_start(turn)?;

            // The session owns the full transcript. Provider calls receive a
            // temporary compacted copy so long-lived sessions keep their original
            // history while model requests stay within the configured budget.
            let mut messages = vec![Message::system(
                self.context.build_system_prompt(options.plan_mode)?,
            )];
            messages.extend(session.history());
            let mut messages = ContextCompactor::new(options.context_budget).compact(&messages);

            let available_tools = self.registry.definitions();
            if options.enable_thinking {
                reporter.on_thinking_start()?;

                // Phase 1: hide the tool schema so the provider cannot emit tool calls.
                // This is distinct from passing an empty tool list to an enabled tool mode.
                let thinking = if options.stream {
                    let mut sink = StdoutStreamSink;
                    let thinking = self.provider.generate_stream(&messages, None, &mut sink)?;
                    if !thinking.content.is_empty() {
                        println!();
                    }
                    thinking
                } else {
                    self.provider.generate(&messages, None)?
                };

                if !thinking.content.is_empty() && !options.stream {
                    reporter.on_thinking(&thinking.content)?;
                }

                if !thinking.content.is_empty() {
                    session.append(thinking.clone());
                    messages.push(thinking);
                }
            }

            // Phase 2: restore tool access and let the provider act on the accumulated context.
            let response = if options.stream {
                let mut sink = StdoutStreamSink;
                let response =
                    self.provider
                        .generate_stream(&messages, Some(&available_tools), &mut sink)?;
                if !response.content.is_empty() {
                    println!();
                }
                response
            } else {
                self.provider.generate(&messages, Some(&available_tools))?
            };

            if !response.content.is_empty() && !options.stream {
                reporter.on_assistant_message(&response.content)?;
            }

            // Keep the provider's response before appending tool observations so the
            // context preserves the exact ReAct order: assistant action, then result.
            let tool_calls = response.tool_calls.clone();
            session.append(response.clone());
            messages.push(response);

            if tool_calls.is_empty() {
                reporter.on_complete()?;
                return Ok(self.session_transcript(session, options.plan_mode)?);
            }

            reporter.on_tool_calls(&tool_calls)?;

            let results = self.execute_tool_batch_with_reporter(&tool_calls, reporter)?;
            for result in results {
                reporter.on_tool_result(&result)?;

                session.append(Message::observation(result.tool_call_id, result.output));
            }
        }

        Err(EngineError::TurnLimitExceeded {
            max_turns: options.max_turns,
        })
    }

    #[cfg(test)]
    fn execute_tool_batch(&self, tool_calls: &[ToolCall]) -> Vec<ToolResult> {
        self.execute_tool_batch_internal(tool_calls)
    }

    fn execute_tool_batch_with_reporter(
        &self,
        tool_calls: &[ToolCall],
        reporter: &mut dyn Reporter,
    ) -> Result<Vec<ToolResult>, EngineError> {
        if tool_calls.len() <= 1 || !self.can_execute_in_parallel(tool_calls) {
            return Ok(tool_calls
                .iter()
                .map(|tool_call| self.execute_one_tool(tool_call))
                .collect());
        }

        reporter.on_parallel_tool_batch()?;
        Ok(self.execute_tool_batch_internal(tool_calls))
    }

    fn execute_tool_batch_internal(&self, tool_calls: &[ToolCall]) -> Vec<ToolResult> {
        if tool_calls.len() <= 1 || !self.can_execute_in_parallel(tool_calls) {
            return tool_calls
                .iter()
                .map(|tool_call| self.execute_one_tool(tool_call))
                .collect();
        }

        // Scoped threads let the batch borrow the immutable registry instead of
        // forcing the whole engine/provider stack into 'static shared ownership.
        let registry = &self.registry;
        std::thread::scope(|scope| {
            let handles = tool_calls
                .iter()
                .map(|tool_call| {
                    (
                        tool_call.id.clone(),
                        scope.spawn(move || execute_one_tool(registry, tool_call)),
                    )
                })
                .collect::<Vec<_>>();

            // Join in the original tool-call order so the model receives
            // observations aligned with the action list it produced.
            handles
                .into_iter()
                .map(|(tool_call_id, handle)| match handle.join() {
                    Ok(result) => result,
                    Err(_) => ToolResult::error(
                        tool_call_id,
                        "tool execution panicked before returning a result",
                    ),
                })
                .collect()
        })
    }

    fn execute_one_tool(&self, tool_call: &ToolCall) -> ToolResult {
        execute_one_tool(&self.registry, tool_call)
    }

    fn can_execute_in_parallel(&self, tool_calls: &[ToolCall]) -> bool {
        // Keep lesson 8 concurrency focused on exploration: any mutating or
        // unknown tool keeps the whole batch sequential.
        tool_calls
            .iter()
            .all(|tool_call| self.registry.is_read_only_call(tool_call))
    }

    fn session_transcript(
        &self,
        session: &Session,
        plan_mode: bool,
    ) -> Result<Vec<Message>, EngineError> {
        let mut transcript = vec![Message::system(
            self.context.build_system_prompt(plan_mode)?,
        )];
        transcript.extend(session.history());
        Ok(transcript)
    }
}

fn execute_one_tool(registry: &ToolRegistry, tool_call: &ToolCall) -> ToolResult {
    // Act and observe: the engine only dispatches the call. Tool-specific
    // argument parsing and execution stay inside the tool layer.
    registry.execute(tool_call)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOptions {
    pub max_turns: usize,
    pub enable_thinking: bool,
    pub plan_mode: bool,
    pub stream: bool,
    pub context_budget: ContextBudget,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            max_turns: 16,
            enable_thinking: false,
            plan_mode: false,
            stream: true,
            context_budget: ContextBudget::default(),
        }
    }
}

#[derive(Debug)]
pub enum EngineError {
    Context(ContextError),
    Provider(ProviderError),
    Reporter(ReporterError),
    TurnLimitExceeded { max_turns: usize },
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Context(error) => write!(formatter, "context failed: {error}"),
            Self::Provider(error) => write!(formatter, "provider failed: {error}"),
            Self::Reporter(error) => write!(formatter, "reporter failed: {error}"),
            Self::TurnLimitExceeded { max_turns } => {
                write!(formatter, "agent loop exceeded {max_turns} turn(s)")
            }
        }
    }
}

impl std::error::Error for EngineError {}

impl From<ContextError> for EngineError {
    fn from(error: ContextError) -> Self {
        Self::Context(error)
    }
}

impl From<ProviderError> for EngineError {
    fn from(error: ProviderError) -> Self {
        Self::Provider(error)
    }
}

impl From<ReporterError> for EngineError {
    fn from(error: ReporterError) -> Self {
        Self::Reporter(error)
    }
}
