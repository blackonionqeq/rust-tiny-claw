use crate::provider::sse::read_sse_data_lines;
use crate::provider::{Provider, ProviderError, StreamSink};
use crate::schema::{Message, Role, ToolCall, ToolDefinition, Usage};
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

    fn generate_stream(
        &mut self,
        messages: &[Message],
        available_tools: Option<&[ToolDefinition]>,
        sink: &mut dyn StreamSink,
    ) -> Result<Message, ProviderError> {
        let request = build_stream_request(&self.config.model, messages, available_tools)?;
        let response = self
            .client
            .post(self.endpoint())
            .bearer_auth(&self.config.api_key)
            .json(&request)
            .send()
            .map_err(|error| ProviderError::new(format!("provider request failed: {error}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().map_err(|error| {
                ProviderError::new(format!("failed to read response body: {error}"))
            })?;
            return Err(ProviderError::new(format!(
                "provider returned HTTP {status}: {body}"
            )));
        }

        let mut state = OpenAiStreamState::default();
        read_sse_data_lines(response, |data| {
            let chunk: ChatCompletionStreamChunk = serde_json::from_str(data).map_err(|error| {
                ProviderError::new(format!(
                    "failed to parse provider stream chunk: {error}; raw chunk: {data}"
                ))
            })?;

            state.apply(chunk, sink)
        })?;

        state.into_message()
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
        stream_options: None,
    })
}

fn build_stream_request(
    model: &str,
    messages: &[Message],
    available_tools: Option<&[ToolDefinition]>,
) -> Result<ChatCompletionRequest, ProviderError> {
    let mut request = build_request(model, messages, available_tools)?;
    request.stream = true;
    request.stream_options = Some(ChatCompletionStreamOptions {
        include_usage: true,
    });
    Ok(request)
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

    // Some strict OpenAI-compatible endpoints require assistant tool-call
    // messages to include `content: ""` instead of omitting the field.
    let content = if message.content.is_empty() {
        (message.role == Role::Assistant && !message.tool_calls.is_empty()).then(String::new)
    } else {
        Some(message.content.clone())
    };

    Ok(OpenAiMessage {
        role,
        content,
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

    let usage = response.usage.map(Into::into);
    let message = if tool_calls.is_empty() {
        Message::assistant(choice.message.content.unwrap_or_default())
    } else {
        Message::assistant_with_tools(choice.message.content.unwrap_or_default(), tool_calls)
    };

    Ok(with_optional_usage(message, usage))
}

fn with_optional_usage(message: Message, usage: Option<Usage>) -> Message {
    match usage {
        Some(usage) => message.with_usage(usage),
        None => message,
    }
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAiTool>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<ChatCompletionStreamOptions>,
}

#[derive(Debug, Serialize)]
struct ChatCompletionStreamOptions {
    include_usage: bool,
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
    usage: Option<OpenAiUsage>,
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

#[derive(Debug, Clone, Copy, Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}

impl From<OpenAiUsage> for Usage {
    fn from(usage: OpenAiUsage) -> Self {
        Self {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletionStreamChunk {
    choices: Vec<ChatCompletionStreamChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionStreamChoice {
    delta: ChatCompletionStreamDelta,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionStreamDelta {
    content: Option<String>,
    tool_calls: Option<Vec<ChatCompletionStreamToolCall>>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionStreamToolCall {
    index: usize,
    id: Option<String>,
    function: Option<ChatCompletionStreamToolCallFunction>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionStreamToolCallFunction {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Default)]
struct OpenAiStreamState {
    content: String,
    tool_calls: Vec<OpenAiStreamToolCallState>,
    usage: Option<Usage>,
}

#[derive(Debug, Default)]
struct OpenAiStreamToolCallState {
    id: String,
    name: String,
    arguments: String,
}

impl OpenAiStreamState {
    fn apply(
        &mut self,
        chunk: ChatCompletionStreamChunk,
        sink: &mut dyn StreamSink,
    ) -> Result<(), ProviderError> {
        // OpenAI-compatible endpoints may send a final usage-only chunk with
        // no choices when stream_options.include_usage is enabled.
        if let Some(usage) = chunk.usage {
            self.usage = Some(usage.into());
        }

        for choice in chunk.choices {
            if let Some(content) = choice.delta.content {
                sink.on_text(&content)?;
                self.content.push_str(&content);
            }

            for tool_call in choice.delta.tool_calls.unwrap_or_default() {
                // OpenAI-compatible streams send tool calls as indexed deltas.
                // The function arguments are partial JSON strings that must be
                // concatenated before they can become an internal ToolCall.
                while self.tool_calls.len() <= tool_call.index {
                    self.tool_calls.push(OpenAiStreamToolCallState::default());
                }

                let state = &mut self.tool_calls[tool_call.index];
                if let Some(id) = tool_call.id {
                    state.id = id;
                }

                if let Some(function) = tool_call.function {
                    if let Some(name) = function.name {
                        state.name = name;
                    }

                    if let Some(arguments) = function.arguments {
                        state.arguments.push_str(&arguments);
                    }
                }
            }
        }

        Ok(())
    }

    fn into_message(self) -> Result<Message, ProviderError> {
        let tool_calls = self
            .tool_calls
            .into_iter()
            .filter(|tool_call| !tool_call.id.is_empty() || !tool_call.name.is_empty())
            .map(|tool_call| {
                let arguments =
                    serde_json::from_str::<Value>(&tool_call.arguments).map_err(|error| {
                        ProviderError::new(format!(
                            "invalid tool call arguments for tool '{}': {error}; raw arguments: {}",
                            tool_call.name, tool_call.arguments
                        ))
                    })?;

                Ok(ToolCall::new(tool_call.id, tool_call.name, arguments))
            })
            .collect::<Result<Vec<_>, ProviderError>>()?;

        let message = if tool_calls.is_empty() {
            Message::assistant(self.content)
        } else {
            Message::assistant_with_tools(self.content, tool_calls)
        };

        Ok(with_optional_usage(message, self.usage))
    }
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
    fn request_keeps_empty_assistant_content_when_tool_calls_are_present() {
        let request = build_request(
            "deepseek-v4-flash",
            &[Message::assistant_with_tools(
                "",
                vec![ToolCall::new("call_1", "echo", json!({ "text": "hi" }))],
            )],
            Some(&[]),
        )
        .unwrap();

        assert_eq!(request.messages[0].role, "assistant");
        assert_eq!(request.messages[0].content.as_deref(), Some(""));
        assert!(request.messages[0].tool_calls.is_some());
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
            usage: None,
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
            usage: None,
        };

        let error = parse_response(response).unwrap_err().to_string();

        assert!(error.contains("invalid tool call arguments"));
        assert!(error.contains("raw arguments"));
    }

    #[test]
    fn stream_chunks_reconstruct_text_message() {
        let chunks = vec![
            ChatCompletionStreamChunk {
                choices: vec![ChatCompletionStreamChoice {
                    delta: ChatCompletionStreamDelta {
                        content: Some("hel".to_string()),
                        tool_calls: None,
                    },
                }],
                usage: None,
            },
            ChatCompletionStreamChunk {
                choices: vec![ChatCompletionStreamChoice {
                    delta: ChatCompletionStreamDelta {
                        content: Some("lo".to_string()),
                        tool_calls: None,
                    },
                }],
                usage: None,
            },
        ];
        let mut state = OpenAiStreamState::default();
        let mut sink = TestSink::default();

        for chunk in chunks {
            state.apply(chunk, &mut sink).unwrap();
        }

        let message = state.into_message().unwrap();
        assert_eq!(message.content, "hello");
        assert_eq!(sink.output, "hello");
    }

    #[test]
    fn stream_chunks_reconstruct_tool_call() {
        let chunks = vec![
            ChatCompletionStreamChunk {
                choices: vec![ChatCompletionStreamChoice {
                    delta: ChatCompletionStreamDelta {
                        content: None,
                        tool_calls: Some(vec![ChatCompletionStreamToolCall {
                            index: 0,
                            id: Some("call_1".to_string()),
                            function: Some(ChatCompletionStreamToolCallFunction {
                                name: Some("echo".to_string()),
                                arguments: Some(r#"{"text":"#.to_string()),
                            }),
                        }]),
                    },
                }],
                usage: None,
            },
            ChatCompletionStreamChunk {
                choices: vec![ChatCompletionStreamChoice {
                    delta: ChatCompletionStreamDelta {
                        content: None,
                        tool_calls: Some(vec![ChatCompletionStreamToolCall {
                            index: 0,
                            id: None,
                            function: Some(ChatCompletionStreamToolCallFunction {
                                name: None,
                                arguments: Some(r#""hi"}"#.to_string()),
                            }),
                        }]),
                    },
                }],
                usage: None,
            },
        ];
        let mut state = OpenAiStreamState::default();
        let mut sink = TestSink::default();

        for chunk in chunks {
            state.apply(chunk, &mut sink).unwrap();
        }

        let message = state.into_message().unwrap();
        assert_eq!(message.tool_calls.len(), 1);
        assert_eq!(message.tool_calls[0].id, "call_1");
        assert_eq!(message.tool_calls[0].arguments, json!({ "text": "hi" }));
    }

    #[test]
    fn response_maps_usage_to_internal_message() {
        let response = ChatCompletionResponse {
            choices: vec![ChatCompletionChoice {
                message: ChatCompletionMessage {
                    content: Some("hello".to_string()),
                    tool_calls: None,
                },
            }],
            usage: Some(OpenAiUsage {
                prompt_tokens: 3,
                completion_tokens: 5,
                total_tokens: 8,
            }),
        };

        let message = parse_response(response).unwrap();

        assert_eq!(
            message.usage,
            Some(Usage {
                prompt_tokens: 3,
                completion_tokens: 5,
                total_tokens: 8
            })
        );
    }

    #[test]
    fn stream_usage_only_chunk_is_attached_to_final_message() {
        let mut state = OpenAiStreamState::default();
        let mut sink = TestSink::default();

        state
            .apply(
                ChatCompletionStreamChunk {
                    choices: Vec::new(),
                    usage: Some(OpenAiUsage {
                        prompt_tokens: 2,
                        completion_tokens: 4,
                        total_tokens: 6,
                    }),
                },
                &mut sink,
            )
            .unwrap();

        let message = state.into_message().unwrap();
        assert_eq!(
            message.usage,
            Some(Usage {
                prompt_tokens: 2,
                completion_tokens: 4,
                total_tokens: 6
            })
        );
    }

    #[derive(Default)]
    struct TestSink {
        output: String,
    }

    impl StreamSink for TestSink {
        fn on_text(&mut self, text: &str) -> Result<(), ProviderError> {
            self.output.push_str(text);
            Ok(())
        }
    }
}
