mod claude_compatible;
mod openai_compatible;
mod sse;

use crate::schema::{Message, ToolCall, ToolDefinition};
use serde_json::json;

pub use claude_compatible::ClaudeCompatibleProvider;
pub use openai_compatible::OpenAiCompatibleProvider;

// Provider is the narrow boundary between the harness and an LLM backend.
// The trait stays synchronous while concrete adapters own their network details.
pub trait Provider {
    fn name(&self) -> &'static str;

    // None means tool mode is disabled for this request; Some means the provider
    // may expose the supplied tool definitions to the model.
    fn generate(
        &mut self,
        messages: &[Message],
        available_tools: Option<&[ToolDefinition]>,
    ) -> Result<Message, ProviderError>;

    fn generate_stream(
        &mut self,
        messages: &[Message],
        available_tools: Option<&[ToolDefinition]>,
        sink: &mut dyn StreamSink,
    ) -> Result<Message, ProviderError> {
        let message = self.generate(messages, available_tools)?;
        if !message.content.is_empty() {
            sink.on_text(&message.content)?;
        }
        Ok(message)
    }
}

pub trait ProviderFactory: Send + Sync {
    fn create(&self) -> Result<Box<dyn Provider + Send>, ProviderError>;
}

impl<T> Provider for Box<T>
where
    T: Provider + ?Sized,
{
    fn name(&self) -> &'static str {
        (**self).name()
    }

    fn generate(
        &mut self,
        messages: &[Message],
        available_tools: Option<&[ToolDefinition]>,
    ) -> Result<Message, ProviderError> {
        (**self).generate(messages, available_tools)
    }

    fn generate_stream(
        &mut self,
        messages: &[Message],
        available_tools: Option<&[ToolDefinition]>,
        sink: &mut dyn StreamSink,
    ) -> Result<Message, ProviderError> {
        (**self).generate_stream(messages, available_tools, sink)
    }
}

pub trait StreamSink {
    fn on_text(&mut self, text: &str) -> Result<(), ProviderError>;
}

pub struct StdoutStreamSink;

impl StreamSink for StdoutStreamSink {
    fn on_text(&mut self, text: &str) -> Result<(), ProviderError> {
        print!("{text}");
        Ok(())
    }
}

#[derive(Debug)]
pub struct ProviderError {
    message: String,
}

impl ProviderError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for ProviderError {}

#[derive(Debug, Default)]
pub struct MockProvider {
    // The mock uses turn state to demonstrate a two-step ReAct exchange.
    turn: usize,
}

impl Provider for MockProvider {
    fn name(&self) -> &'static str {
        "mock-provider"
    }

    fn generate(
        &mut self,
        messages: &[Message],
        available_tools: Option<&[ToolDefinition]>,
    ) -> Result<Message, ProviderError> {
        let Some(available_tools) = available_tools else {
            return Ok(Message::assistant(
                "I should first plan the edit-file smoke test without using tools. The task needs workspace file access, so I will use the local tools once they are available.",
            ));
        };

        self.turn += 1;

        if self.turn == 1 {
            if !available_tools.iter().any(|tool| tool.name == "write_file") {
                return Err(ProviderError::new(
                    "mock provider expected a write_file tool",
                ));
            }

            return Ok(Message::assistant_with_tools(
                "I will create an indented Rust file for the edit_file smoke test.",
                vec![ToolCall::new(
                    "call_001",
                    "write_file",
                    json!({
                        "path": ".tiny-claw/smoke/edit-target.rs",
                        "content": "fn main() {\n    // TODO: add auth\n    if true {\n        println!(\"No auth, everyone can access.\");\n    }\n}\n"
                    }),
                )],
            ));
        }

        if self.turn == 2 {
            if !available_tools.iter().any(|tool| tool.name == "read_file") {
                return Err(ProviderError::new(
                    "mock provider expected a read_file tool",
                ));
            }

            return Ok(Message::assistant_with_tools(
                "I will read the target before editing it.",
                vec![ToolCall::new(
                    "call_002",
                    "read_file",
                    json!({ "path": ".tiny-claw/smoke/edit-target.rs", "start_line": 1, "line_count": 80 }),
                )],
            ));
        }

        if self.turn == 3 {
            if !available_tools.iter().any(|tool| tool.name == "edit_file") {
                return Err(ProviderError::new(
                    "mock provider expected an edit_file tool",
                ));
            }

            return Ok(Message::assistant_with_tools(
                "I will use edit_file with old_text that omits indentation so the fallback matcher is exercised.",
                vec![ToolCall::new(
                    "call_003",
                    "edit_file",
                    json!({
                        "path": ".tiny-claw/smoke/edit-target.rs",
                        "old_text": "// TODO: add auth\nif true {\nprintln!(\"No auth, everyone can access.\");\n}",
                        "new_text": "// TODO: add auth\nif user.is_none() {\n    println!(\"Forbidden!\");\n    return;\n}"
                    }),
                )],
            ));
        }

        if self.turn == 4 {
            if !available_tools.iter().any(|tool| tool.name == "read_file") {
                return Err(ProviderError::new(
                    "mock provider expected a read_file tool",
                ));
            }

            return Ok(Message::assistant_with_tools(
                "I will read the edited file to verify the replacement.",
                vec![ToolCall::new(
                    "call_004",
                    "read_file",
                    json!({ "path": ".tiny-claw/smoke/edit-target.rs", "start_line": 1, "line_count": 80 }),
                )],
            ));
        }

        if self.turn == 5 {
            if !available_tools.iter().any(|tool| tool.name == "read_file") {
                return Err(ProviderError::new(
                    "mock provider expected a read_file tool",
                ));
            }
            if !available_tools.iter().any(|tool| tool.name == "grep") {
                return Err(ProviderError::new("mock provider expected a grep tool"));
            }

            return Ok(Message::assistant_with_tools(
                "I will read three independent project files and grep for TODO in one batch to exercise parallel read-only tool dispatch.",
                vec![
                    ToolCall::new(
                        "call_005_a",
                        "read_file",
                        json!({ "path": "Cargo.toml", "start_line": 1, "line_count": 80 }),
                    ),
                    ToolCall::new(
                        "call_005_b",
                        "read_file",
                        json!({ "path": "README.md", "start_line": 1, "line_count": 80 }),
                    ),
                    ToolCall::new(
                        "call_005_c",
                        "read_file",
                        json!({ "path": "src/bin/tiny-claw.rs", "start_line": 1, "line_count": 80 }),
                    ),
                    ToolCall::new(
                        "call_005_d",
                        "grep",
                        json!({ "query": "TODO", "path": "src", "max_matches": 20 }),
                    ),
                ],
            ));
        }

        let last_observation = messages
            .iter()
            .rev()
            .find(|message| message.tool_call_id.is_some())
            .map(|message| message.content.as_str())
            .unwrap_or("no observation");

        Ok(Message::assistant(format!(
            "Observed: {last_observation}. Task complete."
        )))
    }
}
