use crate::schema::{ToolCall, ToolResult};
use crate::tools::{Tool, ToolAccessMode};
use serde_json::json;

#[derive(Debug, Default)]
pub struct RequestUserHelpTool;

impl RequestUserHelpTool {
    pub fn new() -> Self {
        Self
    }

    fn required_string(call: &ToolCall, name: &str) -> Result<String, String> {
        let Some(value) = call.arguments.get(name).and_then(|value| value.as_str()) else {
            return Err(format!("missing string argument: {name}"));
        };

        let value = value.trim();
        if value.is_empty() {
            return Err(format!("argument '{name}' must not be empty"));
        }

        Ok(value.to_string())
    }
}

impl Tool for RequestUserHelpTool {
    fn name(&self) -> &'static str {
        "request_user_help"
    }

    fn description(&self) -> &'static str {
        "Ask the user for specific help when the current prompt, context, and available tools are insufficient. Use this only after summarizing what was tried and what is needed; after calling it, stop blind retries and surface the question to the user."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "reason": {
                    "type": "string",
                    "description": "Why the task cannot continue with the current prompt, context, and tools."
                },
                "tried": {
                    "type": "string",
                    "description": "What has already been attempted or checked."
                },
                "needed": {
                    "type": "string",
                    "description": "The information, confirmation, or user action needed to proceed."
                },
                "question": {
                    "type": "string",
                    "description": "The concrete question to ask the user."
                }
            },
            "required": ["reason", "tried", "needed", "question"]
        })
    }

    fn access_mode(&self, _call: &ToolCall) -> ToolAccessMode {
        ToolAccessMode::ReadOnly
    }

    fn execute(&self, call: &ToolCall) -> ToolResult {
        let reason = match Self::required_string(call, "reason") {
            Ok(value) => value,
            Err(error) => return ToolResult::error(call.id.clone(), error),
        };
        let tried = match Self::required_string(call, "tried") {
            Ok(value) => value,
            Err(error) => return ToolResult::error(call.id.clone(), error),
        };
        let needed = match Self::required_string(call, "needed") {
            Ok(value) => value,
            Err(error) => return ToolResult::error(call.id.clone(), error),
        };
        let question = match Self::required_string(call, "question") {
            Ok(value) => value,
            Err(error) => return ToolResult::error(call.id.clone(), error),
        };

        ToolResult::ok(
            call.id.clone(),
            format!(
                "USER_HELP_REQUESTED\nreason: {reason}\ntried: {tried}\nneeded: {needed}\nquestion: {question}"
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::RequestUserHelpTool;
    use crate::schema::ToolCall;
    use crate::tools::{Tool, ToolAccessMode};
    use serde_json::json;

    #[test]
    fn request_user_help_returns_structured_observation() {
        let tool = RequestUserHelpTool::new();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "request_user_help",
            json!({
                "reason": "The target file path is ambiguous.",
                "tried": "Searched the workspace for matching files.",
                "needed": "The user must identify the intended file.",
                "question": "Which file should I edit?"
            }),
        ));

        assert!(!result.is_error);
        assert!(result.output.starts_with("USER_HELP_REQUESTED"));
        assert!(
            result
                .output
                .contains("question: Which file should I edit?")
        );
    }

    #[test]
    fn request_user_help_rejects_empty_required_fields() {
        let tool = RequestUserHelpTool::new();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "request_user_help",
            json!({
                "reason": "blocked",
                "tried": "",
                "needed": "input",
                "question": "What should I use?"
            }),
        ));

        assert!(result.is_error);
        assert_eq!(result.output, "argument 'tried' must not be empty");
    }

    #[test]
    fn request_user_help_is_read_only() {
        let tool = RequestUserHelpTool::new();

        assert_eq!(
            tool.access_mode(&ToolCall::new("call_1", "request_user_help", json!({}))),
            ToolAccessMode::ReadOnly
        );
    }
}
