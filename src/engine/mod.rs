use crate::agent_runtime::AgentSupervisor;
use crate::context_engine::{
    ContextBudget, ContextCompactor, ContextError, ContextManager, RecoveryManager, ReminderManager,
};
use crate::memory::{FileMemory, Session};
use crate::provider::{Provider, ProviderError, StdoutStreamSink};
use crate::reporter::terminal::TerminalReporter;
use crate::reporter::{Reporter, ReporterError};
use crate::schema::{Message, ToolCall, ToolResult};
use crate::telemetry::{
    Telemetry, TraceAttribute, TraceContext, TraceExporterConfig, TraceRecorder, TraceStatus,
};
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
    supervisor: Option<AgentSupervisor>,
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
            supervisor: None,
        }
    }

    pub fn with_supervisor(mut self, supervisor: AgentSupervisor) -> Self {
        self.supervisor = Some(supervisor);
        self
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
        let trace_recorder = build_trace_recorder(self.memory.root());
        let (trace_context, mut run_span) = trace_recorder.start_root(
            "Agent.Run",
            vec![
                TraceAttribute::new("session.id", session.id().to_string()),
                TraceAttribute::new("workspace", self.context.work_dir().display().to_string()),
                TraceAttribute::new("provider.name", self.provider.name()),
                TraceAttribute::new("run.max_turns", options.max_turns),
                TraceAttribute::new("run.stream", options.stream),
                TraceAttribute::new("run.thinking_enabled", options.enable_thinking),
                TraceAttribute::new("run.plan_mode", options.plan_mode),
            ],
        );
        let trace_context = trace_recorder.is_enabled().then_some(trace_context);
        session.append(Message::user(user_prompt));
        let mut reminders = ReminderManager::new();

        for turn in 1..=options.max_turns {
            let mut turn_span = trace_context.as_ref().map(|context| {
                context.start_child(
                    "Agent.Turn",
                    vec![
                        TraceAttribute::new("turn.index", turn),
                        TraceAttribute::new("run.stream", options.stream),
                        TraceAttribute::new("run.thinking_enabled", options.enable_thinking),
                        TraceAttribute::new("run.plan_mode", options.plan_mode),
                    ],
                )
            });
            let turn_context = turn_span
                .as_ref()
                .and_then(|span| trace_context.as_ref().map(|context| span.context(context)));
            reporter.on_turn_start(turn)?;

            // The session owns the full transcript. Provider calls receive a
            // temporary compacted copy so long-lived sessions keep their original
            // history while model requests stay within the configured budget.
            let mut messages = vec![Message::system(
                self.context.build_system_prompt(options.plan_mode)?,
            )];
            messages.extend(session.history());
            let mut compaction_span = turn_context.as_ref().map(|context| {
                context.start_child(
                    "Context.Compaction",
                    vec![
                        TraceAttribute::new("context.input_message_count", messages.len()),
                        TraceAttribute::new(
                            "context.budget.max_chars",
                            options.context_budget.max_chars,
                        ),
                    ],
                )
            });
            let mut messages = ContextCompactor::new(options.context_budget).compact(&messages);
            if let Some(span) = &compaction_span {
                span.add_attribute(TraceAttribute::new(
                    "context.output_message_count",
                    messages.len(),
                ));
            }
            if let Some(mut span) = compaction_span.take() {
                span.end_with_status(TraceStatus::Ok);
            }

            let available_tools = self.action_definitions();
            if options.enable_thinking {
                reporter.on_thinking_start()?;

                // Phase 1: hide the tool schema so the provider cannot emit tool calls.
                // This is distinct from passing an empty tool list to an enabled tool mode.
                let thinking = self.generate_with_trace(
                    "LLM.Thinking",
                    &messages,
                    None,
                    options.stream,
                    turn_context.as_ref(),
                )?;

                if !thinking.content.is_empty() && !options.stream {
                    reporter.on_thinking(&thinking.content)?;
                }

                if !thinking.content.is_empty() {
                    session.append(thinking.clone());
                    messages.push(thinking);
                }
            }

            // Phase 2: restore tool access and let the provider act on the accumulated context.
            let response = self.generate_with_trace(
                "LLM.Action",
                &messages,
                Some(&available_tools),
                options.stream,
                turn_context.as_ref(),
            )?;

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

            let results = self.execute_action_batch_with_reporter(
                &tool_calls,
                reporter,
                turn_context.as_ref(),
            )?;
            let reminder = reminders.observe_tool_batch(&tool_calls, &results);
            for result in results {
                reporter.on_tool_result(&result)?;

                session.append(Message::observation(result.tool_call_id, result.output));
            }
            if let Some(reminder) = reminder {
                session.append(reminder);
            }
            if let Some(mut span) = turn_span.take() {
                span.end_with_status(TraceStatus::Ok);
            }
        }

        run_span.end_with_status(TraceStatus::Error {
            message: format!("agent loop exceeded {} turn(s)", options.max_turns),
        });
        let _ = trace_recorder.shutdown();
        Err(EngineError::TurnLimitExceeded {
            max_turns: options.max_turns,
        })
    }

    fn generate_with_trace(
        &mut self,
        span_name: &str,
        messages: &[Message],
        available_tools: Option<&[crate::schema::ToolDefinition]>,
        stream: bool,
        trace_context: Option<&TraceContext>,
    ) -> Result<Message, ProviderError> {
        let mut span = trace_context.map(|context| {
            context.start_child(
                span_name,
                vec![
                    TraceAttribute::new("provider.name", self.provider.name()),
                    TraceAttribute::new("llm.stream", stream),
                    TraceAttribute::new("llm.input_message_count", messages.len()),
                    TraceAttribute::new(
                        "llm.available_tool_count",
                        available_tools.map_or(0, |tools| tools.len()),
                    ),
                ],
            )
        });

        let result = if stream {
            let mut sink = StdoutStreamSink;
            let result = self
                .provider
                .generate_stream(messages, available_tools, &mut sink);
            if result
                .as_ref()
                .map(|message| !message.content.is_empty())
                .unwrap_or(false)
            {
                println!();
            }
            result
        } else {
            self.provider.generate(messages, available_tools)
        };

        if let Some(trace_span) = &span {
            match &result {
                Ok(message) => {
                    trace_span.add_attributes([
                        TraceAttribute::new("llm.success", true),
                        TraceAttribute::new(
                            "llm.emitted_tool_call_count",
                            message.tool_calls.len(),
                        ),
                    ]);
                    if let Some(usage) = message.usage {
                        trace_span.add_attributes([
                            TraceAttribute::new("llm.usage.prompt_tokens", usage.prompt_tokens),
                            TraceAttribute::new(
                                "llm.usage.completion_tokens",
                                usage.completion_tokens,
                            ),
                            TraceAttribute::new("llm.usage.total_tokens", usage.total_tokens),
                        ]);
                    }
                }
                Err(error) => {
                    trace_span.add_attributes([
                        TraceAttribute::new("llm.success", false),
                        TraceAttribute::new("llm.error", error.to_string()),
                    ]);
                }
            }
        }
        if let Some(mut trace_span) = span.take() {
            match &result {
                Ok(_) => trace_span.end_with_status(TraceStatus::Ok),
                Err(error) => trace_span.end_with_status(TraceStatus::Error {
                    message: error.to_string(),
                }),
            }
        }

        result
    }

    #[cfg(test)]
    fn execute_tool_batch(&self, tool_calls: &[ToolCall]) -> Vec<ToolResult> {
        self.execute_action_batch_internal(tool_calls)
    }

    fn execute_action_batch_with_reporter(
        &self,
        tool_calls: &[ToolCall],
        reporter: &mut dyn Reporter,
        trace_context: Option<&TraceContext>,
    ) -> Result<Vec<ToolResult>, EngineError> {
        if tool_calls.len() <= 1 || !self.can_execute_in_parallel(tool_calls) {
            return Ok(tool_calls
                .iter()
                .map(|tool_call| self.execute_one_action(tool_call, trace_context))
                .collect());
        }

        reporter.on_parallel_tool_batch()?;
        Ok(self.execute_action_batch_internal_with_trace(tool_calls, trace_context))
    }

    #[cfg(test)]
    fn execute_action_batch_internal(&self, tool_calls: &[ToolCall]) -> Vec<ToolResult> {
        self.execute_action_batch_internal_with_trace(tool_calls, None)
    }

    fn execute_action_batch_internal_with_trace(
        &self,
        tool_calls: &[ToolCall],
        trace_context: Option<&TraceContext>,
    ) -> Vec<ToolResult> {
        if tool_calls.len() <= 1 || !self.can_execute_in_parallel(tool_calls) {
            return tool_calls
                .iter()
                .map(|tool_call| self.execute_one_action(tool_call, trace_context))
                .collect();
        }

        // Scoped threads let the batch borrow the immutable registry instead of
        // forcing the whole engine/provider stack into 'static shared ownership.
        let registry = &self.registry;
        std::thread::scope(|scope| {
            let handles = tool_calls
                .iter()
                .map(|tool_call| {
                    let trace_context = trace_context.cloned();
                    (
                        tool_call.id.clone(),
                        scope.spawn(move || {
                            execute_one_tool(registry, tool_call, trace_context.as_ref())
                        }),
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

    fn execute_one_action(
        &self,
        tool_call: &ToolCall,
        trace_context: Option<&TraceContext>,
    ) -> ToolResult {
        if AgentSupervisor::is_runtime_command(&tool_call.name) {
            let Some(supervisor) = &self.supervisor else {
                return ToolResult::error(
                    tool_call.id.clone(),
                    format!("runtime command '{}' is not available", tool_call.name),
                );
            };
            return supervisor.execute_command(tool_call);
        }

        execute_one_tool(&self.registry, tool_call, trace_context)
    }

    fn can_execute_in_parallel(&self, tool_calls: &[ToolCall]) -> bool {
        // Keep lesson 8 concurrency focused on exploration: any mutating or
        // unknown tool keeps the whole batch sequential.
        tool_calls.iter().all(|tool_call| {
            !AgentSupervisor::is_runtime_command(&tool_call.name)
                && self.registry.is_read_only_call(tool_call)
        })
    }

    fn action_definitions(&self) -> Vec<crate::schema::ToolDefinition> {
        let mut definitions = self.registry.definitions();
        if let Some(supervisor) = &self.supervisor {
            definitions.extend(supervisor.runtime_definitions());
        }
        definitions
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

fn execute_one_tool(
    registry: &ToolRegistry,
    tool_call: &ToolCall,
    trace_context: Option<&TraceContext>,
) -> ToolResult {
    // Act and observe: the engine only dispatches the call. Tool-specific
    // argument parsing and execution stay inside the tool layer.
    let mut result = registry.execute_with_trace(tool_call, trace_context);
    if result.is_error {
        // Only true tool errors get recovery wrapping here. Command failures
        // such as `bash` non-zero exits remain normal observations so the model
        // can self-correct from the command output without changing error flow.
        result.output = RecoveryManager::new().render_tool_error(&tool_call.name, &result.output);
    }
    result
}

fn build_trace_recorder(memory_root: &std::path::Path) -> TraceRecorder {
    let Ok(config) = TraceExporterConfig::from_env(memory_root) else {
        return TraceRecorder::disabled();
    };
    let debug_flush = config.mode.is_debug();
    match config.build_exporter() {
        Ok(Some(exporter)) => TraceRecorder::new(exporter, debug_flush),
        Ok(None) | Err(_) => TraceRecorder::disabled(),
    }
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
