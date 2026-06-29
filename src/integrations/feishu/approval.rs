use crate::integrations::feishu::client::{ClientError, FeishuClient};
use crate::schema::ToolCall;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

pub const DEFAULT_REJECTION_REASON: &str =
    "Human approval rejected this dangerous tool call. Use a safer and auditable approach instead.";

const DEFAULT_APPROVAL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const COMPLETED_APPROVAL_TTL: Duration = Duration::from_secs(30 * 60);
const MAX_ARGUMENT_PREVIEW_CHARS: usize = 1200;

#[derive(Debug)]
pub struct ApprovalManager {
    state: Mutex<ApprovalState>,
    next_id: AtomicU64,
    timeout: Duration,
    completed_ttl: Duration,
}

impl ApprovalManager {
    pub fn new() -> Self {
        Self::with_timeout(DEFAULT_APPROVAL_TIMEOUT)
    }

    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            state: Mutex::new(ApprovalState::default()),
            next_id: AtomicU64::new(1),
            timeout,
            completed_ttl: COMPLETED_APPROVAL_TTL,
        }
    }

    pub fn wait_for_tool_approval(
        &self,
        client: &FeishuClient,
        chat_id: &str,
        call: &ToolCall,
        reason: &str,
    ) -> Result<ApprovalResolution, ApprovalError> {
        let approval_id = self.next_approval_id();
        let (sender, receiver) = channel();

        {
            let mut state = self.lock_state()?;
            state.prune_completed(self.completed_ttl);
            state.pending.insert(approval_id.clone(), sender);
        }

        let card = approval_card(&approval_id, call, reason);
        if let Err(error) = client.send_interactive_card_to_chat(chat_id, &card) {
            let mut state = self.lock_state()?;
            state.pending.remove(&approval_id);
            return Err(ApprovalError::SendCard(error));
        }

        match receiver.recv_timeout(self.timeout) {
            Ok(resolution) => Ok(resolution),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let mut state = self.lock_state()?;
                state.pending.remove(&approval_id);
                state.completed.insert(
                    approval_id,
                    CompletedApproval {
                        handled_at: Instant::now(),
                    },
                );
                Ok(ApprovalResolution::rejected(DEFAULT_REJECTION_REASON))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err(ApprovalError::WaitChannelClosed)
            }
        }
    }

    pub fn resolve(
        &self,
        approval_id: &str,
        decision: ApprovalDecision,
        rejection_reason: Option<String>,
    ) -> Result<ResolveOutcome, ApprovalError> {
        let resolution = match decision {
            ApprovalDecision::Approve => ApprovalResolution::approved(),
            ApprovalDecision::Reject => {
                let reason = normalize_rejection_reason(rejection_reason);
                ApprovalResolution::rejected(reason)
            }
        };

        let sender = {
            let mut state = self.lock_state()?;
            state.prune_completed(self.completed_ttl);

            if let Some(sender) = state.pending.remove(approval_id) {
                state.completed.insert(
                    approval_id.to_string(),
                    CompletedApproval {
                        handled_at: Instant::now(),
                    },
                );
                Some(sender)
            } else if state.completed.contains_key(approval_id) {
                return Ok(ResolveOutcome::AlreadyHandled);
            } else {
                return Ok(ResolveOutcome::Unknown);
            }
        };

        if let Some(sender) = sender {
            let _ = sender.send(resolution);
        }

        Ok(ResolveOutcome::Resolved)
    }

    fn next_approval_id(&self) -> String {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        format!("approval_{id}")
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, ApprovalState>, ApprovalError> {
        self.state.lock().map_err(|_| ApprovalError::StatePoisoned)
    }
}

impl Default for ApprovalManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Default)]
struct ApprovalState {
    pending: HashMap<String, Sender<ApprovalResolution>>,
    completed: HashMap<String, CompletedApproval>,
}

impl ApprovalState {
    fn prune_completed(&mut self, ttl: Duration) {
        let now = Instant::now();
        self.completed
            .retain(|_, completed| now.duration_since(completed.handled_at) < ttl);
    }
}

#[derive(Debug)]
struct CompletedApproval {
    handled_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalResolution {
    pub allowed: bool,
    pub reason: String,
}

impl ApprovalResolution {
    pub fn approved() -> Self {
        Self {
            allowed: true,
            reason: "Human approval allowed this tool call.".to_string(),
        }
    }

    pub fn rejected(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveOutcome {
    Resolved,
    AlreadyHandled,
    Unknown,
}

#[derive(Debug)]
pub enum ApprovalError {
    SendCard(ClientError),
    StatePoisoned,
    WaitChannelClosed,
}

impl fmt::Display for ApprovalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SendCard(error) => write!(formatter, "failed to send approval card: {error}"),
            Self::StatePoisoned => write!(formatter, "approval manager state is poisoned"),
            Self::WaitChannelClosed => write!(formatter, "approval wait channel closed"),
        }
    }
}

impl std::error::Error for ApprovalError {}

fn normalize_rejection_reason(reason: Option<String>) -> String {
    reason
        .map(|reason| reason.trim().to_string())
        .filter(|reason| !reason.is_empty())
        .unwrap_or_else(|| DEFAULT_REJECTION_REASON.to_string())
}

fn approval_card(approval_id: &str, call: &ToolCall, reason: &str) -> Value {
    let content = format!(
        "**Tool:** {}\n**Reason:** {}\n**Approval ID:** {}\n\n**Arguments**\n{}",
        sanitize_lark_md(&call.name),
        sanitize_lark_md(reason),
        sanitize_lark_md(approval_id),
        sanitize_lark_md(&argument_preview(&call.arguments))
    );

    json!({
        "config": {
            "wide_screen_mode": true,
            "update_multi": true
        },
        "header": {
            "template": "orange",
            "title": {
                "tag": "plain_text",
                "content": "Dangerous tool call approval"
            }
        },
        "elements": [
            {
                "tag": "div",
                "text": {
                    "tag": "lark_md",
                    "content": content
                }
            },
            {
                "tag": "input",
                "name": "reject_reason",
                "placeholder": {
                    "tag": "plain_text",
                    "content": "Optional rejection reason or safer alternative"
                }
            },
            {
                "tag": "action",
                "actions": [
                    {
                        "tag": "button",
                        "text": {
                            "tag": "plain_text",
                            "content": "Allow"
                        },
                        "type": "primary",
                        "value": {
                            "action": "approve_tool_call",
                            "approval_id": approval_id
                        }
                    },
                    {
                        "tag": "button",
                        "text": {
                            "tag": "plain_text",
                            "content": "Reject"
                        },
                        "type": "danger",
                        "value": {
                            "action": "reject_tool_call",
                            "approval_id": approval_id
                        }
                    }
                ]
            }
        ]
    })
}

fn argument_preview(arguments: &Value) -> String {
    let rendered =
        serde_json::to_string_pretty(arguments).unwrap_or_else(|_| arguments.to_string());
    truncate_chars(&rendered, MAX_ARGUMENT_PREVIEW_CHARS)
}

fn sanitize_lark_md(value: &str) -> String {
    value
        .replace('`', "'")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}\n...[truncated]...")
    } else {
        truncated
    }
}

pub fn callback_response(outcome: ResolveOutcome) -> Value {
    let content = match outcome {
        ResolveOutcome::Resolved => "Approval recorded.",
        ResolveOutcome::AlreadyHandled => "This approval has already been handled.",
        ResolveOutcome::Unknown => "Approval request was not found or has expired.",
    };

    json!({
        "toast": {
            "type": "info",
            "content": content
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ApprovalDecision, ApprovalManager, DEFAULT_REJECTION_REASON, ResolveOutcome, approval_card,
        normalize_rejection_reason,
    };
    use crate::integrations::feishu::event::{FeishuCallback, parse_callback};
    use crate::schema::ToolCall;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn empty_rejection_reason_uses_default() {
        assert_eq!(
            normalize_rejection_reason(Some("  ".to_string())),
            DEFAULT_REJECTION_REASON
        );
    }

    #[test]
    fn non_empty_rejection_reason_is_trimmed() {
        assert_eq!(
            normalize_rejection_reason(Some(" use dry-run first ".to_string())),
            "use dry-run first"
        );
    }

    #[test]
    fn approval_card_avoids_markdown_code_html_tags() {
        let call = ToolCall::new(
            "call_1",
            "bash",
            json!({
                "command": "printf `<code>` && rm -rf ./tmp"
            }),
        );

        let card = approval_card(
            "approval_1",
            &call,
            "recursive deletion requires human approval",
        );
        let content = card["elements"][0]["text"]["content"].as_str().unwrap();

        assert!(!content.contains('`'));
        assert!(!content.contains("```"));
        assert!(content.contains("&lt;code&gt;"));
        assert!(content.contains("\"command\""));
    }

    #[test]
    fn first_resolution_wins_for_duplicate_callbacks() {
        let manager = ApprovalManager::with_timeout(Duration::from_secs(5));
        let (sender, receiver) = channel_for_test();
        {
            let mut state = manager.state.lock().unwrap();
            state.pending.insert("approval_1".to_string(), sender);
        }

        assert_eq!(
            manager
                .resolve("approval_1", ApprovalDecision::Approve, None)
                .unwrap(),
            ResolveOutcome::Resolved
        );
        assert_eq!(
            manager
                .resolve(
                    "approval_1",
                    ApprovalDecision::Reject,
                    Some("changed mind".to_string())
                )
                .unwrap(),
            ResolveOutcome::AlreadyHandled
        );

        let resolution = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(resolution.allowed);
    }

    #[test]
    fn unknown_resolution_is_reported() {
        let manager = ApprovalManager::with_timeout(Duration::from_secs(5));

        assert_eq!(
            manager
                .resolve("missing", ApprovalDecision::Approve, None)
                .unwrap(),
            ResolveOutcome::Unknown
        );
    }

    #[test]
    fn protocol_like_reject_then_conflicting_mobile_click_keeps_first_result() {
        let manager = ApprovalManager::with_timeout(Duration::from_secs(5));
        let (sender, receiver) = channel_for_test();
        {
            let mut state = manager.state.lock().unwrap();
            state.pending.insert("approval_1".to_string(), sender);
        }

        let desktop_reject = json!({
            "token": "verify",
            "open_id": "ou_desktop",
            "action": {
                "value": {
                    "action": "reject_tool_call",
                    "approval_id": "approval_1"
                },
                "form_value": {
                    "reject_reason": "Use ls and a dry-run before deleting files."
                }
            }
        });
        let action = match parse_callback(&desktop_reject, Some("verify")).unwrap() {
            FeishuCallback::CardAction(action) => action,
            other => panic!("unexpected callback: {other:?}"),
        };

        assert_eq!(
            manager
                .resolve(&action.approval_id, action.decision, action.reject_reason)
                .unwrap(),
            ResolveOutcome::Resolved
        );

        let mobile_allow = json!({
            "token": "verify",
            "open_id": "ou_mobile",
            "action": {
                "value": {
                    "action": "approve_tool_call",
                    "approval_id": "approval_1"
                }
            }
        });
        let action = match parse_callback(&mobile_allow, Some("verify")).unwrap() {
            FeishuCallback::CardAction(action) => action,
            other => panic!("unexpected callback: {other:?}"),
        };

        assert_eq!(
            manager
                .resolve(&action.approval_id, action.decision, action.reject_reason)
                .unwrap(),
            ResolveOutcome::AlreadyHandled
        );

        let resolution = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(!resolution.allowed);
        assert_eq!(
            resolution.reason,
            "Use ls and a dry-run before deleting files."
        );
    }

    fn channel_for_test() -> (
        std::sync::mpsc::Sender<super::ApprovalResolution>,
        std::sync::mpsc::Receiver<super::ApprovalResolution>,
    ) {
        std::sync::mpsc::channel()
    }
}
