use crate::provider::{Provider, ProviderError};
use crate::schema::{Message, Role, ToolCall, ToolDefinition};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::time::Duration;

const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
const DEFAULT_MODEL: &str = "deepseek-v4-flash";
const DEFAULT_TIMEOUT_SECONDS: u64 = 60;

#[derive(Debug)]
pub struct OpenAiCompatibleProvider {
    client: Client,
    config: OpenAiCompatibleConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiCompatibleConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub timeout_seconds: u64,
}

impl OpenAiCompatibleConfig {
    pub fn from_env() -> Result<Self, ProviderError> {
        let api_key = required_env("TINY_CLAW_API_KEY")?;
        let base_url =
            env::var("TINY_CLAW_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
        let model = env::var("TINY_CLAW_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        let timeout_seconds = match env::var("TINY_CLAW_TIMEOUT_SECONDS") {
            Ok(value) => value.parse::<u64>().map_err(|error| {
                ProviderError::new(format!("invalid TINY_CLAW_TIMEOUT_SECONDS value: {error}"))
            })?,
            Err(_) => DEFAULT_TIMEOUT_SECONDS,
        };

        Ok(Self {
            api_key,
            base_url,
            model,
            timeout_seconds,
        })
    }
}

impl OpenAiCompatibleProvider {
    pub fn from_env() -> Result<Self, ProviderError> {
        Self::new(OpenAiCompatibleConfig::from_env()?)
    }

    pub fn new(config: OpenAiCompatibleConfig) -> Result<Self, ProviderError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|error| ProviderError::new(format!("failed to build HTTP client: {error}")))?;

        Ok(Self { client, config })
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        )
    }
}

impl Provider for OpenAiCompatibleProvider {
    fn name(&self) -> &'static str {
        "openai-compatible"
    }

    fn generate(
        &mut self,
        messages: &[Message],
        available_tools: Option<&[ToolDefinition]>,
    ) -> Result<Message, ProviderError> {
        let request = build_request(&self.config.model, messages, available_tools)?;
        let response = self
            .client
            .post(self.endpoint())
            .bearer_auth(&self.config.api_key)
            .json(&request)
            .send()
            .map_err(|error| ProviderError::new(format!("provider request failed: {error}")))?;

        let status = response.status();
        let body = response.text().map_err(|error| {
            ProviderError::new(format!("failed to read response body: {error}"))
        })?;

        if !status.is_success() {
            return Err(ProviderError::new(format!(
                "provider returned HTTP {status}: {body}"
            )));
        }

        let response: ChatCompletionResponse = serde_json::from_str(&body).map_err(|error| {
            ProviderError::new(format!(
                "failed to parse provider response: {error}; raw response: {body}"
            ))
        })?;

        parse_response(response)
    }
}

fn required_env(name: &str) -> Result<String, ProviderError> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(ProviderError::new(format!(
            "missing required environment variable: {name}"
        ))),
    }
}

fn build_request(
    model: &str,
    messages: &[Message],
    available_tools: Option<&[ToolDefinition]>,
) -> Result<ChatCompletionRequest, ProviderError> {
    let messages = messages
        .iter()
        .map(to_openai_message)
        .collect::<Result<Vec<_>, _>>()?;

    let tools = available_tools
        .and_then(|tools| (!tools.is_empty()).then(|| tools.iter().map(to_openai_tool).collect()));

    Ok(ChatCompletionRequest {
        model: model.to_string(),
        messages,
        tools,
        stream: false,
    })
}

fn to_openai_message(message: &Message) -> Result<OpenAiMessage, ProviderError> {
    if let Some(tool_call_id) = &message.tool_call_id {
        return Ok(OpenAiMessage {
            role: "tool",
            content: Some(message.content.clone()),
            tool_call_id: Some(tool_call_id.clone()),
            tool_calls: None,
        });
    }

    let role = match message.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
    };

    let tool_calls = (!message.tool_calls.is_empty()).then(|| {
        message
            .tool_calls
            .iter()
            .map(|tool_call| OpenAiToolCall {
                id: tool_call.id.clone(),
                call_type: "function",
                function: OpenAiToolCallFunction {
                    name: tool_call.name.clone(),
                    arguments: tool_call.arguments.to_string(),
                },
            })
            .collect()
    });

    Ok(OpenAiMessage {
        role,
        content: (!message.content.is_empty()).then(|| message.content.clone()),
        tool_call_id: None,
        tool_calls,
    })
}

fn to_openai_tool(tool: &ToolDefinition) -> OpenAiTool {
    OpenAiTool {
        tool_type: "function",
        function: OpenAiToolFunction {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.input_schema.clone(),
        },
    }
}

fn parse_response(response: ChatCompletionResponse) -> Result<Message, ProviderError> {
    let choice = response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| ProviderError::new("provider returned no choices"))?;

    let tool_calls = choice
        .message
        .tool_calls
        .unwrap_or_default()
        .into_iter()
        .map(|tool_call| {
            let arguments =
                serde_json::from_str::<Value>(&tool_call.function.arguments).map_err(|error| {
                    ProviderError::new(format!(
                        "invalid tool call arguments for tool '{}': {error}; raw arguments: {}",
                        tool_call.function.name, tool_call.function.arguments
                    ))
                })?;

            Ok(ToolCall::new(
                tool_call.id,
                tool_call.function.name,
                arguments,
            ))
        })
        .collect::<Result<Vec<_>, ProviderError>>()?;

    if tool_calls.is_empty() {
        Ok(Message::assistant(
            choice.message.content.unwrap_or_default(),
        ))
    } else {
        Ok(Message::assistant_with_tools(
            choice.message.content.unwrap_or_default(),
            tool_calls,
        ))
    }
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAiTool>>,
    stream: bool,
}

#[derive(Debug, Serialize)]
struct OpenAiMessage {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Debug, Serialize)]
struct OpenAiTool {
    #[serde(rename = "type")]
    tool_type: &'static str,
    function: OpenAiToolFunction,
}

#[derive(Debug, Serialize)]
struct OpenAiToolFunction {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Serialize)]
struct OpenAiToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: &'static str,
    function: OpenAiToolCallFunction,
}

#[derive(Debug, Serialize)]
struct OpenAiToolCallFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatCompletionChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChoice {
    message: ChatCompletionMessage,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionMessage {
    content: Option<String>,
    tool_calls: Option<Vec<ChatCompletionToolCall>>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionToolCall {
    id: String,
    function: ChatCompletionToolCallFunction,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionToolCallFunction {
    name: String,
    arguments: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::ToolDefinition;
    use serde_json::json;

    #[test]
    fn request_omits_tools_when_tool_mode_is_disabled() {
        let request = build_request("deepseek-v4-flash", &[Message::user("hello")], None).unwrap();

        assert!(request.tools.is_none());
    }

    #[test]
    fn request_maps_tool_observation_to_tool_message() {
        let request = build_request(
            "deepseek-v4-flash",
            &[Message::observation("call_1", "24C")],
            Some(&[]),
        )
        .unwrap();

        assert_eq!(request.messages[0].role, "tool");
        assert_eq!(request.messages[0].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(request.messages[0].content.as_deref(), Some("24C"));
    }

    #[test]
    fn request_maps_tool_definitions() {
        let tools = vec![ToolDefinition::new(
            "echo",
            "Echo input.",
            json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                },
                "required": ["text"]
            }),
        )];

        let request =
            build_request("deepseek-v4-flash", &[Message::user("hello")], Some(&tools)).unwrap();

        let tools = request.tools.unwrap();
        assert_eq!(tools[0].tool_type, "function");
        assert_eq!(tools[0].function.name, "echo");
    }

    #[test]
    fn response_maps_tool_calls_to_internal_message() {
        let response = ChatCompletionResponse {
            choices: vec![ChatCompletionChoice {
                message: ChatCompletionMessage {
                    content: Some("calling echo".to_string()),
                    tool_calls: Some(vec![ChatCompletionToolCall {
                        id: "call_1".to_string(),
                        function: ChatCompletionToolCallFunction {
                            name: "echo".to_string(),
                            arguments: r#"{"text":"hi"}"#.to_string(),
                        },
                    }]),
                },
            }],
        };

        let message = parse_response(response).unwrap();

        assert_eq!(message.content, "calling echo");
        assert_eq!(message.tool_calls.len(), 1);
        assert_eq!(message.tool_calls[0].arguments, json!({ "text": "hi" }));
    }

    #[test]
    fn response_rejects_invalid_tool_call_arguments() {
        let response = ChatCompletionResponse {
            choices: vec![ChatCompletionChoice {
                message: ChatCompletionMessage {
                    content: None,
                    tool_calls: Some(vec![ChatCompletionToolCall {
                        id: "call_1".to_string(),
                        function: ChatCompletionToolCallFunction {
                            name: "echo".to_string(),
                            arguments: "not json".to_string(),
                        },
                    }]),
                },
            }],
        };

        let error = parse_response(response).unwrap_err().to_string();

        assert!(error.contains("invalid tool call arguments"));
        assert!(error.contains("raw arguments"));
    }
}
