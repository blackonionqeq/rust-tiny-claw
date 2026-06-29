use crate::integrations::feishu::approval::ApprovalDecision;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMessage {
    pub chat_id: String,
    pub message_id: String,
    pub sender_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedMessage {
    pub chat_id: String,
    pub message_id: String,
    pub sender_id: String,
    pub message_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeishuCallback {
    Challenge { challenge: String },
    Message(IncomingMessage),
    UnsupportedMessage(UnsupportedMessage),
    CardAction(CardAction),
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardAction {
    pub approval_id: String,
    pub decision: ApprovalDecision,
    pub operator_id: Option<String>,
    pub reject_reason: Option<String>,
}

pub fn parse_callback(
    body: &Value,
    verify_token: Option<&str>,
) -> Result<FeishuCallback, EventError> {
    if body.get("type").and_then(Value::as_str) == Some("url_verification") {
        verify_callback_token(body, verify_token)?;
        let challenge = body
            .get("challenge")
            .and_then(Value::as_str)
            .ok_or_else(|| EventError::new("missing url verification challenge"))?;
        return Ok(FeishuCallback::Challenge {
            challenge: challenge.to_string(),
        });
    }

    verify_callback_token(body, verify_token)?;

    if let Some(card_action) = parse_card_action(body)? {
        return Ok(FeishuCallback::CardAction(card_action));
    }

    let header_event_type = body
        .pointer("/header/event_type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if header_event_type != "im.message.receive_v1" {
        return Ok(FeishuCallback::Ignored);
    }

    parse_message(body)
}

fn parse_card_action(body: &Value) -> Result<Option<CardAction>, EventError> {
    let Some(action) = body
        .get("action")
        .or_else(|| body.pointer("/event/action"))
        .or_else(|| body.pointer("/schema/action"))
    else {
        return Ok(None);
    };

    let value = action
        .get("value")
        .ok_or_else(|| EventError::new("missing Feishu card action value"))?;
    let action_name = value
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| EventError::new("missing Feishu card action name"))?;

    let decision = match action_name {
        "approve_tool_call" => ApprovalDecision::Approve,
        "reject_tool_call" => ApprovalDecision::Reject,
        _ => return Ok(None),
    };

    let approval_id = value
        .get("approval_id")
        .and_then(Value::as_str)
        .ok_or_else(|| EventError::new("missing Feishu approval_id"))?
        .to_string();
    let operator_id = body
        .get("open_id")
        .or_else(|| body.pointer("/operator/open_id"))
        .or_else(|| body.pointer("/event/operator/open_id"))
        .or_else(|| body.pointer("/event/operator/operator_id/open_id"))
        .or_else(|| body.pointer("/event/operator/operator_id/user_id"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let reject_reason = action
        .get("form_value")
        .or_else(|| action.get("formValue"))
        .and_then(|form_value| form_value.get("reject_reason"))
        .and_then(Value::as_str)
        .map(ToString::to_string);

    Ok(Some(CardAction {
        approval_id,
        decision,
        operator_id,
        reject_reason,
    }))
}

fn verify_callback_token(body: &Value, verify_token: Option<&str>) -> Result<(), EventError> {
    let Some(expected) = verify_token else {
        return Ok(());
    };

    let actual = body
        .get("token")
        .or_else(|| body.pointer("/header/token"))
        .and_then(Value::as_str);

    if actual == Some(expected) {
        Ok(())
    } else {
        Err(EventError::new("Feishu callback verify token mismatch"))
    }
}

fn parse_message(body: &Value) -> Result<FeishuCallback, EventError> {
    let event = body
        .get("event")
        .ok_or_else(|| EventError::new("missing Feishu event object"))?;
    let message = event
        .get("message")
        .ok_or_else(|| EventError::new("missing Feishu message object"))?;

    let message_id = string_at(message, "/message_id")?;
    let chat_id = string_at(message, "/chat_id")?;
    let sender_id = event
        .pointer("/sender/sender_id/open_id")
        .or_else(|| event.pointer("/sender/sender_id/user_id"))
        .or_else(|| event.pointer("/sender/sender_id/union_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| EventError::new("missing Feishu sender id"))?
        .to_string();
    let message_type = string_at(message, "/message_type")?;

    if message_type != "text" {
        return Ok(FeishuCallback::UnsupportedMessage(UnsupportedMessage {
            chat_id,
            message_id,
            sender_id,
            message_type,
        }));
    }

    let content = string_at(message, "/content")?;
    let content: TextContent = serde_json::from_str(&content)
        .map_err(|error| EventError::new(format!("invalid Feishu text content: {error}")))?;

    Ok(FeishuCallback::Message(IncomingMessage {
        chat_id,
        message_id,
        sender_id,
        text: content.text,
    }))
}

fn string_at(value: &Value, pointer: &str) -> Result<String, EventError> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| EventError::new(format!("missing string field: {pointer}")))
}

#[derive(Debug, Deserialize)]
struct TextContent {
    text: String,
}

#[derive(Debug)]
pub struct EventError {
    message: String,
}

impl EventError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for EventError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for EventError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_url_verification_challenge() {
        let body = json!({
            "type": "url_verification",
            "token": "verify",
            "challenge": "abc"
        });

        let callback = parse_callback(&body, Some("verify")).unwrap();

        assert_eq!(
            callback,
            FeishuCallback::Challenge {
                challenge: "abc".to_string()
            }
        );
    }

    #[test]
    fn normalizes_message_event() {
        let body = json!({
            "header": {
                "event_type": "im.message.receive_v1",
                "token": "verify"
            },
            "event": {
                "sender": {
                    "sender_id": { "open_id": "ou_1" }
                },
                "message": {
                    "message_id": "om_1",
                    "chat_id": "oc_1",
                    "message_type": "text",
                    "content": "{\"text\":\"run tests\"}"
                }
            }
        });

        let incoming = match parse_callback(&body, Some("verify")).unwrap() {
            FeishuCallback::Message(incoming) => incoming,
            other => panic!("unexpected callback: {other:?}"),
        };

        assert_eq!(incoming.chat_id, "oc_1");
        assert_eq!(incoming.message_id, "om_1");
        assert_eq!(incoming.sender_id, "ou_1");
        assert_eq!(incoming.text, "run tests");
    }

    #[test]
    fn normalizes_unsupported_message_event() {
        let body = json!({
            "header": {
                "event_type": "im.message.receive_v1",
                "token": "verify"
            },
            "event": {
                "sender": {
                    "sender_id": { "open_id": "ou_1" }
                },
                "message": {
                    "message_id": "om_1",
                    "chat_id": "oc_1",
                    "message_type": "image",
                    "content": "{}"
                }
            }
        });

        let unsupported = match parse_callback(&body, Some("verify")).unwrap() {
            FeishuCallback::UnsupportedMessage(unsupported) => unsupported,
            other => panic!("unexpected callback: {other:?}"),
        };

        assert_eq!(unsupported.chat_id, "oc_1");
        assert_eq!(unsupported.message_id, "om_1");
        assert_eq!(unsupported.sender_id, "ou_1");
        assert_eq!(unsupported.message_type, "image");
    }

    #[test]
    fn parses_top_level_card_action() {
        let body = json!({
            "token": "verify",
            "open_id": "ou_1",
            "action": {
                "value": {
                    "action": "reject_tool_call",
                    "approval_id": "approval_1"
                },
                "form_value": {
                    "reject_reason": "use dry-run first"
                }
            }
        });

        let action = match parse_callback(&body, Some("verify")).unwrap() {
            FeishuCallback::CardAction(action) => action,
            other => panic!("unexpected callback: {other:?}"),
        };

        assert_eq!(action.approval_id, "approval_1");
        assert_eq!(action.decision, ApprovalDecision::Reject);
        assert_eq!(action.operator_id.as_deref(), Some("ou_1"));
        assert_eq!(action.reject_reason.as_deref(), Some("use dry-run first"));
    }

    #[test]
    fn parses_event_wrapped_card_action() {
        let body = json!({
            "header": {
                "event_type": "card.action.trigger",
                "token": "verify"
            },
            "event": {
                "operator": {
                    "operator_id": { "open_id": "ou_1" }
                },
                "action": {
                    "value": {
                        "action": "approve_tool_call",
                        "approval_id": "approval_1"
                    }
                }
            }
        });

        let action = match parse_callback(&body, Some("verify")).unwrap() {
            FeishuCallback::CardAction(action) => action,
            other => panic!("unexpected callback: {other:?}"),
        };

        assert_eq!(action.approval_id, "approval_1");
        assert_eq!(action.decision, ApprovalDecision::Approve);
        assert_eq!(action.operator_id.as_deref(), Some("ou_1"));
        assert_eq!(action.reject_reason, None);
    }
}
