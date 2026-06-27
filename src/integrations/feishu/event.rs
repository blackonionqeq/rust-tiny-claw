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
pub enum FeishuCallback {
    Challenge { challenge: String },
    Message(IncomingMessage),
    Ignored,
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

    let header_event_type = body
        .pointer("/header/event_type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if header_event_type != "im.message.receive_v1" {
        return Ok(FeishuCallback::Ignored);
    }

    Ok(FeishuCallback::Message(parse_message(body)?))
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

fn parse_message(body: &Value) -> Result<IncomingMessage, EventError> {
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
        return Err(EventError::new(format!(
            "unsupported Feishu message type: {message_type}"
        )));
    }

    let content = string_at(message, "/content")?;
    let content: TextContent = serde_json::from_str(&content)
        .map_err(|error| EventError::new(format!("invalid Feishu text content: {error}")))?;

    Ok(IncomingMessage {
        chat_id,
        message_id,
        sender_id,
        text: content.text,
    })
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
}
