use crate::schema::{Message, ToolCall, ToolDefinition};
use serde_json::json;

// Provider is the narrow boundary between the harness and an LLM backend.
// This lesson keeps it synchronous; real network adapters can come later.
pub trait Provider {
    fn name(&self) -> &'static str;

    fn generate(
        &mut self,
        messages: &[Message],
        available_tools: &[ToolDefinition],
    ) -> Result<Message, ProviderError>;
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
        available_tools: &[ToolDefinition],
    ) -> Result<Message, ProviderError> {
        self.turn += 1;

        if self.turn == 1 {
            let echo_available = available_tools.iter().any(|tool| tool.name == "echo");
            if !echo_available {
                return Err(ProviderError::new("mock provider expected an echo tool"));
            }

            return Ok(Message::assistant_with_tools(
                "I will ask the echo tool for a simple observation.",
                vec![ToolCall::new(
                    "call_001",
                    "echo",
                    json!({ "text": "workspace tools are wired" }),
                )],
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
