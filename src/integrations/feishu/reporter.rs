use crate::integrations::feishu::client::FeishuClient;
use crate::reporter::{Reporter, ReporterError};
use crate::schema::{ToolCall, ToolResult};

#[derive(Debug, Clone)]
pub struct FeishuReporter {
    client: FeishuClient,
    chat_id: String,
}

impl FeishuReporter {
    pub fn new(client: FeishuClient, chat_id: impl Into<String>) -> Self {
        Self {
            client,
            chat_id: chat_id.into(),
        }
    }

    fn send(&self, text: impl AsRef<str>) -> Result<(), ReporterError> {
        self.client
            .send_text_to_chat(&self.chat_id, text.as_ref())
            .map_err(|error| ReporterError::new(error.to_string()))
    }
}

impl Reporter for FeishuReporter {
    fn on_turn_start(&mut self, turn: usize) -> Result<(), ReporterError> {
        self.send(format!("[turn {turn}] reasoning"))
    }

    fn on_thinking_start(&mut self) -> Result<(), ReporterError> {
        self.send("[thinking] tools disabled")
    }

    fn on_thinking(&mut self, content: &str) -> Result<(), ReporterError> {
        self.send(format!("thinking: {content}"))
    }

    fn on_assistant_message(&mut self, content: &str) -> Result<(), ReporterError> {
        if content.trim().is_empty() {
            return Ok(());
        }
        self.send(content)
    }

    fn on_tool_calls(&mut self, tool_calls: &[ToolCall]) -> Result<(), ReporterError> {
        self.send(format!(
            "[engine] requested {} tool call(s)",
            tool_calls.len()
        ))
    }

    fn on_tool_result(&mut self, result: &ToolResult) -> Result<(), ReporterError> {
        if result.is_error {
            self.send(format!("[observation:error] {}", result.output))
        } else {
            self.send(format!("[observation] {}", result.output))
        }
    }

    fn on_parallel_tool_batch(&mut self) -> Result<(), ReporterError> {
        self.send("[engine] executing tools in parallel")
    }

    fn on_complete(&mut self) -> Result<(), ReporterError> {
        self.send("[engine] task complete")
    }
}
