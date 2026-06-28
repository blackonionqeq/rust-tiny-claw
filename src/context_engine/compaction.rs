use crate::schema::{Message, Role};

const FAR_OBSERVATION_MARKER: &str = "[tool output compacted";
const RECENT_OBSERVATION_MARKER: &str = "[tool output truncated";
const ASSISTANT_FOLD_MARKER: &str = "[earlier assistant content folded]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudget {
    pub max_chars: usize,
    // Recent messages are a protection zone, not a hard message-count window.
    // Older history can still be sent after low-value content is compacted.
    pub retain_recent_messages: usize,
    pub max_recent_observation_chars: usize,
    pub far_observation_mask_chars: usize,
    pub far_assistant_fold_chars: usize,
    pub head_chars: usize,
    pub tail_chars: usize,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            max_chars: 32_000,
            retain_recent_messages: 12,
            max_recent_observation_chars: 8_000,
            far_observation_mask_chars: 1_000,
            far_assistant_fold_chars: 1_000,
            head_chars: 1_000,
            tail_chars: 1_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextCompactor {
    budget: ContextBudget,
}

impl ContextCompactor {
    pub fn new(budget: ContextBudget) -> Self {
        Self { budget }
    }

    // Build a temporary provider context. The returned messages may contain
    // masked or truncated content and must not be written back to Session.
    pub fn compact(&self, messages: &[Message]) -> Vec<Message> {
        if self.estimate_chars(messages) <= self.budget.max_chars {
            return messages.to_vec();
        }

        let protect_start = messages
            .len()
            .saturating_sub(self.budget.retain_recent_messages);

        messages
            .iter()
            .enumerate()
            .map(|(index, message)| self.compact_message(index >= protect_start, message))
            .collect()
    }

    fn compact_message(&self, is_recent: bool, message: &Message) -> Message {
        if message.role == Role::System {
            return message.clone();
        }

        let mut compacted = message.clone();
        let content_chars = compacted.content.chars().count();

        if compacted.tool_call_id.is_some() {
            if is_recent {
                if content_chars > self.budget.max_recent_observation_chars {
                    compacted.content = truncate_head_tail(
                        &compacted.content,
                        self.budget.head_chars,
                        self.budget.tail_chars,
                        RECENT_OBSERVATION_MARKER,
                    );
                }
            } else if content_chars > self.budget.far_observation_mask_chars {
                compacted.content =
                    format!("{FAR_OBSERVATION_MARKER}; original chars: {content_chars}]");
            }
        } else if !is_recent
            && compacted.role == Role::Assistant
            && content_chars > self.budget.far_assistant_fold_chars
        {
            compacted.content = ASSISTANT_FOLD_MARKER.to_string();
        }

        compacted
    }

    fn estimate_chars(&self, messages: &[Message]) -> usize {
        messages
            .iter()
            .map(|message| {
                let tool_call_chars = message
                    .tool_calls
                    .iter()
                    .map(|tool_call| {
                        tool_call.id.chars().count()
                            + tool_call.name.chars().count()
                            + tool_call.arguments.to_string().chars().count()
                    })
                    .sum::<usize>();

                message.content.chars().count() + tool_call_chars
            })
            .sum()
    }
}

fn truncate_head_tail(content: &str, head_chars: usize, tail_chars: usize, marker: &str) -> String {
    // Slice by Unicode scalar values rather than bytes so CJK text cannot panic
    // on an invalid UTF-8 boundary.
    let chars = content.chars().collect::<Vec<_>>();
    let len = chars.len();
    let keep = head_chars.saturating_add(tail_chars);

    if len <= keep {
        return content.to_string();
    }

    let head = chars.iter().take(head_chars).collect::<String>();
    let tail = chars
        .iter()
        .skip(len.saturating_sub(tail_chars))
        .collect::<String>();
    let omitted = len - keep;

    format!("{head}\n\n{marker}; omitted chars: {omitted}]\n\n{tail}")
}

#[cfg(test)]
mod tests {
    use super::{ContextBudget, ContextCompactor};
    use crate::schema::{Message, Role, ToolCall};
    use serde_json::json;

    #[test]
    fn compactor_returns_messages_unchanged_under_budget() {
        let messages = vec![Message::system("system"), Message::user("small")];
        let compactor = ContextCompactor::new(ContextBudget {
            max_chars: 1_000,
            ..ContextBudget::default()
        });

        assert_eq!(compactor.compact(&messages), messages);
    }

    #[test]
    fn compactor_masks_far_observation_but_keeps_tool_call_id() {
        let messages = vec![
            Message::system("system"),
            Message::assistant_with_tools(
                "",
                vec![ToolCall::new(
                    "call_1",
                    "read_file",
                    json!({ "path": "large.log" }),
                )],
            ),
            Message::observation("call_1", "A".repeat(200)),
            Message::user("current request"),
            Message::assistant("current answer"),
        ];
        let compactor = small_compactor(2);

        let compacted = compactor.compact(&messages);

        assert_eq!(compacted[0], messages[0]);
        assert_eq!(compacted[1].tool_calls, messages[1].tool_calls);
        assert_eq!(compacted[2].tool_call_id.as_deref(), Some("call_1"));
        assert!(compacted[2].content.contains("tool output compacted"));
        assert!(compacted[2].content.contains("original chars: 200"));
        assert!(!compacted[2].content.contains(&"A".repeat(100)));
    }

    #[test]
    fn compactor_truncates_recent_large_observation_with_head_and_tail() {
        let content = format!("{}{}{}", "H".repeat(20), "M".repeat(80), "T".repeat(20));
        let messages = vec![
            Message::system("system"),
            Message::assistant_with_tools(
                "",
                vec![ToolCall::new(
                    "call_1",
                    "read_file",
                    json!({ "path": "log" }),
                )],
            ),
            Message::observation("call_1", content),
        ];
        let compactor = small_compactor(3);

        let compacted = compactor.compact(&messages);

        assert!(compacted[2].content.starts_with(&"H".repeat(8)));
        assert!(compacted[2].content.contains("tool output truncated"));
        assert!(compacted[2].content.ends_with(&"T".repeat(8)));
        assert!(!compacted[2].content.contains(&"M".repeat(40)));
    }

    #[test]
    fn compactor_folds_far_verbose_assistant_content_without_dropping_tool_calls() {
        let tool_calls = vec![ToolCall::new(
            "call_1",
            "bash",
            json!({ "command": "date" }),
        )];
        let messages = vec![
            Message::system("system"),
            Message::assistant_with_tools("thinking ".repeat(30), tool_calls.clone()),
            Message::user("recent"),
            Message::assistant("answer"),
        ];
        let compactor = small_compactor(2);

        let compacted = compactor.compact(&messages);

        assert_eq!(compacted[1].role, Role::Assistant);
        assert_eq!(compacted[1].tool_calls, tool_calls);
        assert_eq!(compacted[1].content, "[earlier assistant content folded]");
    }

    #[test]
    fn compactor_handles_multibyte_text_safely_for_chinese_and_japanese() {
        for content in ["你好世界".repeat(50), "こんにちは世界".repeat(50)] {
            let messages = vec![
                Message::system("system"),
                Message::assistant_with_tools(
                    "",
                    vec![ToolCall::new(
                        "call_1",
                        "read_file",
                        json!({ "path": "i18n" }),
                    )],
                ),
                Message::observation("call_1", content),
            ];
            let compactor = small_compactor(3);

            let compacted = compactor.compact(&messages);

            assert_eq!(compacted[2].tool_call_id.as_deref(), Some("call_1"));
            assert!(compacted[2].content.contains("tool output truncated"));
        }
    }

    fn small_compactor(retain_recent_messages: usize) -> ContextCompactor {
        ContextCompactor::new(ContextBudget {
            max_chars: 40,
            retain_recent_messages,
            max_recent_observation_chars: 30,
            far_observation_mask_chars: 20,
            far_assistant_fold_chars: 20,
            head_chars: 8,
            tail_chars: 8,
        })
    }
}
