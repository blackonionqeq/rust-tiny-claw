use crate::integrations::feishu::token::{TenantTokenCache, TokenError};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, warn};

const SEND_MESSAGE_URL: &str = "https://open.feishu.cn/open-apis/im/v1/messages";

#[derive(Debug, Clone)]
pub struct FeishuClient {
    client: Client,
    token_cache: Arc<TenantTokenCache>,
}

impl FeishuClient {
    pub fn new(token_cache: Arc<TenantTokenCache>) -> Self {
        Self {
            client: Client::new(),
            token_cache,
        }
    }

    pub fn send_text_to_chat(&self, chat_id: &str, text: &str) -> Result<(), ClientError> {
        let text_len = text.chars().count();
        let token = self.token_cache.tenant_access_token()?;
        let content = serde_json::to_string(&TextContent { text }).map_err(|error| {
            ClientError::new(format!("failed to encode Feishu message content: {error}"))
        })?;

        let response = self
            .client
            .post(SEND_MESSAGE_URL)
            .query(&[("receive_id_type", "chat_id")])
            .bearer_auth(token)
            .json(&SendMessageRequest {
                receive_id: chat_id,
                msg_type: "text",
                content,
            })
            .send()
            .map_err(|error| {
                ClientError::new(format!("Feishu send message request failed: {error}"))
            })?;

        let status = response.status();
        let body = response.text().map_err(|error| {
            ClientError::new(format!("failed to read Feishu send response: {error}"))
        })?;

        if !status.is_success() {
            warn!(
                %chat_id,
                text_len,
                %status,
                "Feishu send message HTTP request failed"
            );
            return Err(ClientError::new(format!(
                "Feishu send message endpoint returned HTTP {status}: {body}"
            )));
        }

        let response: SendMessageResponse = serde_json::from_str(&body).map_err(|error| {
            ClientError::new(format!(
                "invalid Feishu send response: {error}; raw response: {body}"
            ))
        })?;

        if response.code != 0 {
            warn!(
                %chat_id,
                text_len,
                code = response.code,
                msg = %response.msg,
                "Feishu send message API returned an error"
            );
            return Err(ClientError::new(format!(
                "Feishu send message returned code {}: {}",
                response.code, response.msg
            )));
        }

        debug!(%chat_id, text_len, "Feishu text message sent");
        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct SendMessageRequest<'a> {
    receive_id: &'a str,
    msg_type: &'a str,
    content: String,
}

#[derive(Debug, Serialize)]
struct TextContent<'a> {
    text: &'a str,
}

#[derive(Debug, Deserialize)]
struct SendMessageResponse {
    code: i64,
    msg: String,
}

#[derive(Debug)]
pub struct ClientError {
    message: String,
}

impl ClientError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl From<TokenError> for ClientError {
    fn from(error: TokenError) -> Self {
        Self::new(error.to_string())
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for ClientError {}
