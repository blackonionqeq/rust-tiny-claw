use crate::schema::{ToolCall, ToolResult};
use std::fmt;

pub mod terminal;

pub trait Reporter {
    fn on_turn_start(&mut self, turn: usize) -> Result<(), ReporterError>;
    fn on_thinking_start(&mut self) -> Result<(), ReporterError>;
    fn on_thinking(&mut self, content: &str) -> Result<(), ReporterError>;
    fn on_assistant_message(&mut self, content: &str) -> Result<(), ReporterError>;
    fn on_tool_calls(&mut self, tool_calls: &[ToolCall]) -> Result<(), ReporterError>;
    fn on_tool_result(&mut self, result: &ToolResult) -> Result<(), ReporterError>;
    fn on_parallel_tool_batch(&mut self) -> Result<(), ReporterError>;
    fn on_complete(&mut self) -> Result<(), ReporterError>;
}

#[derive(Debug)]
pub struct ReporterError {
    message: String,
}

impl ReporterError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ReporterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for ReporterError {}
