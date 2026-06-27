use crate::reporter::{Reporter, ReporterError};
use crate::schema::{ToolCall, ToolResult};

#[derive(Debug, Default)]
pub struct TerminalReporter;

impl Reporter for TerminalReporter {
    fn on_turn_start(&mut self, turn: usize) -> Result<(), ReporterError> {
        println!("[turn {turn}] reasoning");
        Ok(())
    }

    fn on_thinking_start(&mut self) -> Result<(), ReporterError> {
        println!("[thinking] tools disabled");
        Ok(())
    }

    fn on_thinking(&mut self, content: &str) -> Result<(), ReporterError> {
        println!("thinking: {content}");
        Ok(())
    }

    fn on_assistant_message(&mut self, content: &str) -> Result<(), ReporterError> {
        println!("assistant: {content}");
        Ok(())
    }

    fn on_tool_calls(&mut self, tool_calls: &[ToolCall]) -> Result<(), ReporterError> {
        println!("[engine] requested {} tool call(s)", tool_calls.len());
        for tool_call in tool_calls {
            println!("[action] {} {}", tool_call.name, tool_call.arguments);
        }
        Ok(())
    }

    fn on_tool_result(&mut self, result: &ToolResult) -> Result<(), ReporterError> {
        if result.is_error {
            println!("[observation:error] {}", result.output);
        } else {
            println!("[observation] {}", result.output);
        }
        Ok(())
    }

    fn on_parallel_tool_batch(&mut self) -> Result<(), ReporterError> {
        println!("[engine] executing tools in parallel");
        Ok(())
    }

    fn on_complete(&mut self) -> Result<(), ReporterError> {
        println!("[engine] task complete");
        Ok(())
    }
}
