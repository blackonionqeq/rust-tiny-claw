use crate::context_engine::ContextManager;
use crate::memory::FileMemory;
use crate::provider::{Provider, ProviderError};
use crate::schema::Message;
use crate::telemetry::Telemetry;
use crate::tools::ToolRegistry;
use std::fmt;

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

    pub fn boot_plan(&self) -> Vec<String> {
        vec![
            format!("provider: {}", self.provider.name()),
            format!("tools registered: {}", self.registry.len()),
            format!("context manager: {}", self.context.name()),
            format!("memory root: {}", self.memory.root().display()),
            format!("telemetry: {}", self.telemetry.name()),
            "lesson 02: ReAct main loop available".to_string(),
        ]
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
            // Reason: ask the model/provider what to do next with the full timeline.
            let response = self.provider.generate(&messages, &available_tools)?;

            if !response.content.is_empty() {
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

            for tool_call in tool_calls {
                println!("[action] {} {}", tool_call.name, tool_call.arguments);

                // Act and observe: the engine only dispatches the call. Tool-specific
                // argument parsing and execution stay inside the tool layer.
                let result = self.registry.execute(&tool_call);
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOptions {
    pub max_turns: usize,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self { max_turns: 16 }
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
