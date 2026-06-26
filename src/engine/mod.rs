use crate::context_engine::ContextManager;
use crate::memory::FileMemory;
use crate::provider::{Provider, ProviderError, StdoutStreamSink};
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
            format!("tools registered: {}", self.registry.len()),
            format!("context manager: {}", self.context.name()),
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
        // The message list is the agent's short-term memory for this lesson.
        // Later chapters can move prompt loading and compaction into ContextManager.
        let mut messages = vec![
            Message::system(
                "You are rust-tiny-claw, a small coding assistant running inside one workspace.",
            ),
            Message::user(user_prompt),
        ];

        for turn in 1..=options.max_turns {
            println!("[turn {turn}] reasoning");

            let available_tools = self.registry.definitions();
            if options.enable_thinking {
                println!("[thinking] tools disabled");

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
                    println!("thinking: {}", thinking.content);
                }

                if !thinking.content.is_empty() {
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
                println!("assistant: {}", response.content);
            }

            // Keep the provider's response before appending tool observations so the
            // context preserves the exact ReAct order: assistant action, then result.
            let tool_calls = response.tool_calls.clone();
            messages.push(response);

            if tool_calls.is_empty() {
                println!("[engine] task complete");
                return Ok(messages);
            }

            println!("[engine] requested {} tool call(s)", tool_calls.len());

            let results = self.execute_tool_batch(&tool_calls);
            for result in results {
                if result.is_error {
                    println!("[observation:error] {}", result.output);
                } else {
                    println!("[observation] {}", result.output);
                }

                messages.push(Message::observation(result.tool_call_id, result.output));
            }
        }

        Err(EngineError::TurnLimitExceeded {
            max_turns: options.max_turns,
        })
    }

    fn execute_tool_batch(&self, tool_calls: &[ToolCall]) -> Vec<ToolResult> {
        if tool_calls.len() <= 1 {
            return tool_calls
                .iter()
                .map(|tool_call| self.execute_one_tool(tool_call))
                .collect();
        }

        println!("[engine] executing tools in parallel");

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
}

fn execute_one_tool(registry: &ToolRegistry, tool_call: &ToolCall) -> ToolResult {
    println!("[action] {} {}", tool_call.name, tool_call.arguments);

    // Act and observe: the engine only dispatches the call. Tool-specific
    // argument parsing and execution stay inside the tool layer.
    registry.execute(tool_call)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOptions {
    pub max_turns: usize,
    pub enable_thinking: bool,
    pub stream: bool,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            max_turns: 16,
            enable_thinking: false,
            stream: true,
        }
    }
}

#[derive(Debug)]
pub enum EngineError {
    Provider(ProviderError),
    TurnLimitExceeded { max_turns: usize },
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Provider(error) => write!(formatter, "provider failed: {error}"),
            Self::TurnLimitExceeded { max_turns } => {
                write!(formatter, "agent loop exceeded {max_turns} turn(s)")
            }
        }
    }
}

impl std::error::Error for EngineError {}

impl From<ProviderError> for EngineError {
    fn from(error: ProviderError) -> Self {
        Self::Provider(error)
    }
}
