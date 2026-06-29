use crate::schema::{Message, ToolCall, ToolResult};
use serde_json::{Map, Value};

const SAME_FINGERPRINT_THRESHOLD: usize = 3;
const SAME_ERROR_CODE_THRESHOLD: usize = 3;
const CONSECUTIVE_ERROR_CALL_THRESHOLD: usize = 4;
const CONSECUTIVE_ERROR_TURN_THRESHOLD: usize = 3;
const REMINDER_COOLDOWN_TURNS: usize = 2;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReminderManager {
    // This is streak-based state, not a full sliding window. It only tracks
    // consecutive failures until a successful tool result proves forward progress.
    consecutive_error_calls: usize,
    consecutive_error_turns: usize,
    same_fingerprint: Streak,
    same_error_code: Streak,
    cooldown_turns: usize,
}

impl ReminderManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe_tool_batch(
        &mut self,
        tool_calls: &[ToolCall],
        results: &[ToolResult],
    ) -> Option<Message> {
        if tool_calls.is_empty() || results.is_empty() {
            return None;
        }

        if results.iter().any(|result| !result.is_error) {
            self.reset_after_progress();
            return None;
        }

        if self.cooldown_turns > 0 {
            self.cooldown_turns -= 1;
        }

        self.consecutive_error_turns += 1;
        self.consecutive_error_calls += results.len();

        let mut strongest_reason = None;
        for (tool_call, result) in tool_calls.iter().zip(results) {
            let error_code = extract_error_code(&result.output);
            // Fingerprints include normalized arguments so trivial path spelling
            // changes like "./a.txt" vs "a.txt " still count as the same retry.
            let fingerprint = normalized_fingerprint(tool_call, error_code);

            let same_fingerprint_count = self.same_fingerprint.observe(fingerprint);
            let same_error_code_count = self.same_error_code.observe(error_code.to_string());

            if same_fingerprint_count >= SAME_FINGERPRINT_THRESHOLD {
                strongest_reason = Some(ReminderReason::RepeatedSimilarCall {
                    count: same_fingerprint_count,
                    tool_name: tool_call.name.clone(),
                    error_code: error_code.to_string(),
                });
            } else if same_error_code_count >= SAME_ERROR_CODE_THRESHOLD {
                strongest_reason = Some(ReminderReason::RepeatedErrorCode {
                    count: same_error_code_count,
                    error_code: error_code.to_string(),
                });
            }
        }

        if strongest_reason.is_none()
            && self.consecutive_error_turns >= CONSECUTIVE_ERROR_TURN_THRESHOLD
        {
            strongest_reason = Some(ReminderReason::ConsecutiveErrorTurns {
                count: self.consecutive_error_turns,
            });
        }

        if strongest_reason.is_none()
            && self.consecutive_error_calls >= CONSECUTIVE_ERROR_CALL_THRESHOLD
        {
            strongest_reason = Some(ReminderReason::ConsecutiveErrorCalls {
                count: self.consecutive_error_calls,
            });
        }

        if self.cooldown_turns == 0 {
            strongest_reason.map(|reason| {
                self.cooldown_turns = REMINDER_COOLDOWN_TURNS;
                Message::user(render_system_reminder(reason))
            })
        } else {
            None
        }
    }

    fn reset_after_progress(&mut self) {
        self.consecutive_error_calls = 0;
        self.consecutive_error_turns = 0;
        self.same_fingerprint = Streak::default();
        self.same_error_code = Streak::default();
        self.cooldown_turns = 0;
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Streak {
    value: Option<String>,
    count: usize,
}

impl Streak {
    fn observe(&mut self, value: String) -> usize {
        if self.value.as_deref() == Some(value.as_str()) {
            self.count += 1;
        } else {
            self.value = Some(value);
            self.count = 1;
        }
        self.count
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReminderReason {
    RepeatedSimilarCall {
        count: usize,
        tool_name: String,
        error_code: String,
    },
    RepeatedErrorCode {
        count: usize,
        error_code: String,
    },
    ConsecutiveErrorCalls {
        count: usize,
    },
    ConsecutiveErrorTurns {
        count: usize,
    },
}

fn render_system_reminder(reason: ReminderReason) -> String {
    let summary = match reason {
        ReminderReason::RepeatedSimilarCall {
            count,
            tool_name,
            error_code,
        } => format!(
            "You have made {count} similar failing `{tool_name}` tool call(s) with error_code `{error_code}`."
        ),
        ReminderReason::RepeatedErrorCode { count, error_code } => {
            format!("You have hit error_code `{error_code}` {count} time(s) in a row.")
        }
        ReminderReason::ConsecutiveErrorCalls { count } => {
            format!("Your last {count} tool call(s) all failed.")
        }
        ReminderReason::ConsecutiveErrorTurns { count } => {
            format!("Your last {count} tool-use turn(s) all failed.")
        }
    };

    format!(
        "[SYSTEM REMINDER]\n\
         {summary}\n\n\
         Pause before making another tool call. Summarize what you tried, identify the shared failure pattern, and change strategy. \
         Use read-only exploration when it can produce new evidence. If you cannot solve this with the available tools, stop retrying and tell the user exactly what help or information you need. \
         Do not repeat a similar tool call unless you have new evidence that it will succeed."
    )
}

fn extract_error_code(output: &str) -> &str {
    // RecoveryManager renders tool errors with an `error_code:` line. Unknown or
    // legacy errors fall back to a generic bucket so broad failure streaks still work.
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("error_code: "))
        .map(str::trim)
        .filter(|code| !code.is_empty())
        .unwrap_or("UNKNOWN_TOOL_ERROR")
}

fn normalized_fingerprint(tool_call: &ToolCall, error_code: &str) -> String {
    format!(
        "{}:{}:{}",
        tool_call.name,
        error_code,
        normalize_json(&tool_call.arguments)
    )
}

fn normalize_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => normalize_string(value),
        Value::Array(values) => {
            let values = values.iter().map(normalize_json).collect::<Vec<_>>();
            format!("[{}]", values.join(","))
        }
        Value::Object(object) => normalize_object(object),
    }
}

fn normalize_object(object: &Map<String, Value>) -> String {
    let mut entries = object
        .iter()
        .map(|(key, value)| {
            let normalized_value = if is_path_key(key) {
                value
                    .as_str()
                    .map(normalize_path_string)
                    .unwrap_or_else(|| normalize_json(value))
            } else {
                normalize_json(value)
            };
            format!("{}:{}", key.trim(), normalized_value)
        })
        .collect::<Vec<_>>();
    entries.sort();
    format!("{{{}}}", entries.join(","))
}

fn is_path_key(key: &str) -> bool {
    matches!(
        key,
        "path" | "file" | "file_path" | "filepath" | "old_path" | "new_path"
    )
}

fn normalize_string(value: &str) -> String {
    value
        .trim()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_path_string(value: &str) -> String {
    let mut parts = Vec::new();
    let normalized = value.trim().replace('\\', "/");
    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::ReminderManager;
    use crate::schema::{ToolCall, ToolResult};
    use serde_json::json;

    #[test]
    fn repeated_normalized_failure_triggers_reminder() {
        let mut manager = ReminderManager::new();

        assert!(
            manager
                .observe_tool_batch(&[call("1", "a.txt")], &[error("1")])
                .is_none()
        );
        assert!(
            manager
                .observe_tool_batch(&[call("2", "./a.txt ")], &[error("2")])
                .is_none()
        );
        let reminder = manager
            .observe_tool_batch(&[call("3", "nested/../a.txt")], &[error("3")])
            .expect("third normalized failure should trigger");

        assert!(reminder.content.contains("[SYSTEM REMINDER]"));
        assert!(reminder.content.contains("similar failing `read_file`"));
        assert!(reminder.content.contains("FILE_NOT_FOUND"));
    }

    #[test]
    fn consecutive_failures_trigger_even_when_errors_differ() {
        let mut manager = ReminderManager::new();

        assert!(
            manager
                .observe_tool_batch(
                    &[tool("1", "read_file")],
                    &[raw_error("1", "FILE_NOT_FOUND")]
                )
                .is_none()
        );
        assert!(
            manager
                .observe_tool_batch(
                    &[tool("2", "edit_file")],
                    &[raw_error("2", "EDIT_TEXT_NOT_FOUND")]
                )
                .is_none()
        );
        let reminder = manager
            .observe_tool_batch(
                &[tool("3", "grep")],
                &[raw_error("3", "UNKNOWN_TOOL_ERROR")],
            )
            .expect("third all-error turn should trigger");

        assert!(reminder.content.contains("tool-use turn(s) all failed"));
    }

    #[test]
    fn success_resets_failure_state() {
        let mut manager = ReminderManager::new();

        assert!(
            manager
                .observe_tool_batch(&[call("1", "a.txt")], &[error("1")])
                .is_none()
        );
        assert!(
            manager
                .observe_tool_batch(&[call("2", "a.txt")], &[ToolResult::ok("2", "ok")])
                .is_none()
        );
        assert!(
            manager
                .observe_tool_batch(&[call("3", "a.txt")], &[error("3")])
                .is_none()
        );
        assert!(
            manager
                .observe_tool_batch(&[call("4", "a.txt")], &[error("4")])
                .is_none()
        );
    }

    fn call(id: &str, path: &str) -> ToolCall {
        ToolCall::new(id, "read_file", json!({ "path": path }))
    }

    fn tool(id: &str, name: &str) -> ToolCall {
        ToolCall::new(id, name, json!({ "value": id }))
    }

    fn error(id: &str) -> ToolResult {
        raw_error(id, "FILE_NOT_FOUND")
    }

    fn raw_error(id: &str, code: &str) -> ToolResult {
        ToolResult::error(
            id,
            format!("Tool call failed.\nerror_code: {code}\nRaw error: no"),
        )
    }
}
