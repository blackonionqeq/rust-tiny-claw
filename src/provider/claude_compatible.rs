use crate::provider::sse::read_sse_data_lines;
use crate::provider::{Provider, ProviderError, StreamSink};
use crate::schema::{Message, Role, ToolCall, ToolDefinition};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::time::Duration;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";
const DEFAULT_TIMEOUT_SECONDS: u64 = 60;
const DEFAULT_MAX_TOKENS: u64 = 4096;
const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug)]
pub struct ClaudeCompatibleProvider {
    client: Client,
    config: ClaudeCompatibleConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCompatibleConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub timeout_seconds: u64,
    pub max_tokens: u64,
    pub anthropic_version: String,
}

impl ClaudeCompatibleConfig {
    pub fn from_env() -> Result<Self, ProviderError> {
        let api_key = required_env("TINY_CLAW_API_KEY")?;
        let base_url =
            env::var("TINY_CLAW_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
        let model = env::var("TINY_CLAW_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        let timeout_seconds =
            parse_optional_u64_env("TINY_CLAW_TIMEOUT_SECONDS", DEFAULT_TIMEOUT_SECONDS)?;
        let max_tokens = parse_optional_u64_env("TINY_CLAW_MAX_TOKENS", DEFAULT_MAX_TOKENS)?;
        let anthropic_version = env::var("TINY_CLAW_ANTHROPIC_VERSION")
            .unwrap_or_else(|_| DEFAULT_ANTHROPIC_VERSION.to_string());

        Ok(Self {
            api_key,
            base_url,
            model,
            timeout_seconds,
            max_tokens,
            anthropic_version,
        })
    }
}

impl ClaudeCompatibleProvider {
    pub fn from_env() -> Result<Self, ProviderError> {
        Self::new(ClaudeCompatibleConfig::from_env()?)
    }

    pub fn new(config: ClaudeCompatibleConfig) -> Result<Self, ProviderError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|error| ProviderError::new(format!("failed to build HTTP client: {error}")))?;

        Ok(Self { client, config })
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.config.base_url.trim_end_matches('/'))
    }
}

impl Provider for ClaudeCompatibleProvider {
    fn name(&self) -> &'static str {
        "claude-compatible"
    }

    fn generate(
        &mut self,
        messages: &[Message],
        available_tools: Option<&[ToolDefinition]>,
    ) -> Result<Message, ProviderError> {
        let request = build_request(
            &self.config.model,
            self.config.max_tokens,
            messages,
            available_tools,
        )?;
        let response = self
            .client
            .post(self.endpoint())
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", &self.config.anthropic_version)
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

        let response: ClaudeMessageResponse = serde_json::from_str(&body).map_err(|error| {
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
        let request = build_stream_request(
            &self.config.model,
            self.config.max_tokens,
            messages,
            available_tools,
        )?;
        let response = self
            .client
            .post(self.endpoint())
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", &self.config.anthropic_version)
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

        let mut state = ClaudeStreamState::default();
        read_sse_data_lines(response, |data| {
            let event: ClaudeStreamEvent = serde_json::from_str(data).map_err(|error| {
                ProviderError::new(format!(
                    "failed to parse provider stream chunk: {error}; raw chunk: {data}"
                ))
            })?;

            state.apply(event, sink)
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

fn parse_optional_u64_env(name: &str, default: u64) -> Result<u64, ProviderError> {
    match env::var(name) {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|error| ProviderError::new(format!("invalid {name} value: {error}"))),
        Err(_) => Ok(default),
    }
}

fn build_request(
    model: &str,
    max_tokens: u64,
    messages: &[Message],
    available_tools: Option<&[ToolDefinition]>,
) -> Result<ClaudeMessageRequest, ProviderError> {
    let mut system = Vec::new();
    let mut claude_messages = Vec::new();
    let mut pending_tool_results = Vec::new();

    for message in messages {
        if let Some(tool_result) = to_claude_tool_result_block(message) {
            pending_tool_results.push(tool_result);
            continue;
        }

        flush_tool_results(&mut claude_messages, &mut pending_tool_results);

        match message.role {
            Role::System => {
                if !message.content.is_empty() {
                    system.push(ClaudeTextBlock {
                        block_type: "text",
                        text: message.content.clone(),
                    });
                }
            }
            Role::User => claude_messages.push(to_claude_user_message(message)),
            Role::Assistant => {
                let assistant = to_claude_assistant_message(message);
                if !assistant.content.is_empty() {
                    claude_messages.push(assistant);
                }
            }
        }
    }
    flush_tool_results(&mut claude_messages, &mut pending_tool_results);

    let tools = available_tools
        .and_then(|tools| (!tools.is_empty()).then(|| tools.iter().map(to_claude_tool).collect()));

    Ok(ClaudeMessageRequest {
        model: model.to_string(),
        max_tokens,
        system: (!system.is_empty()).then_some(system),
        messages: claude_messages,
        tools,
        stream: false,
    })
}

fn flush_tool_results(
    claude_messages: &mut Vec<ClaudeMessageParam>,
    pending_tool_results: &mut Vec<ClaudeContentBlockParam>,
) {
    if pending_tool_results.is_empty() {
        return;
    }

    claude_messages.push(ClaudeMessageParam {
        role: "user",
        content: std::mem::take(pending_tool_results),
    });
}

fn to_claude_tool_result_block(message: &Message) -> Option<ClaudeContentBlockParam> {
    let tool_call_id = message.tool_call_id.as_ref()?;

    Some(ClaudeContentBlockParam::ToolResult {
        block_type: "tool_result",
        tool_use_id: tool_call_id.clone(),
        content: message.content.clone(),
        is_error: None,
    })
}

fn build_stream_request(
    model: &str,
    max_tokens: u64,
    messages: &[Message],
    available_tools: Option<&[ToolDefinition]>,
) -> Result<ClaudeMessageRequest, ProviderError> {
    let mut request = build_request(model, max_tokens, messages, available_tools)?;
    request.stream = true;
    Ok(request)
}

fn to_claude_user_message(message: &Message) -> ClaudeMessageParam {
    ClaudeMessageParam {
        role: "user",
        content: vec![ClaudeContentBlockParam::Text {
            block_type: "text",
            text: message.content.clone(),
        }],
    }
}

fn to_claude_assistant_message(message: &Message) -> ClaudeMessageParam {
    let mut content = Vec::new();

    if !message.content.is_empty() {
        content.push(ClaudeContentBlockParam::Text {
            block_type: "text",
            text: message.content.clone(),
        });
    }

    for tool_call in &message.tool_calls {
        content.push(ClaudeContentBlockParam::ToolUse {
            block_type: "tool_use",
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            input: tool_call.arguments.clone(),
        });
    }

    ClaudeMessageParam {
        role: "assistant",
        content,
    }
}

fn to_claude_tool(tool: &ToolDefinition) -> ClaudeTool {
    ClaudeTool {
        name: tool.name.clone(),
        description: tool.description.clone(),
        input_schema: tool.input_schema.clone(),
    }
}

fn parse_response(response: ClaudeMessageResponse) -> Result<Message, ProviderError> {
    let mut content = String::new();
    let mut tool_calls = Vec::new();

    for block in response.content {
        match block {
            ClaudeContentBlock::Text { text, .. } => {
                content.push_str(&text);
            }
            ClaudeContentBlock::ToolUse {
                id, name, input, ..
            } => {
                if !input.is_object() {
                    return Err(ProviderError::new(format!(
                        "invalid tool call arguments for tool '{name}': expected JSON object; raw arguments: {input}"
                    )));
                }

                tool_calls.push(ToolCall::new(id, name, input));
            }
            // DeepSeek's Claude-compatible endpoint can return `thinking`
            // blocks. They are not part of the engine-facing message, so keep
            // parsing text/tool_use blocks and ignore the rest.
            ClaudeContentBlock::Other => {}
        }
    }

    if tool_calls.is_empty() {
        Ok(Message::assistant(content))
    } else {
        Ok(Message::assistant_with_tools(content, tool_calls))
    }
}

#[derive(Debug, Serialize)]
struct ClaudeMessageRequest {
    model: String,
    max_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Vec<ClaudeTextBlock>>,
    messages: Vec<ClaudeMessageParam>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ClaudeTool>>,
    stream: bool,
}

#[derive(Debug, Serialize)]
struct ClaudeTextBlock {
    #[serde(rename = "type")]
    block_type: &'static str,
    text: String,
}

#[derive(Debug, Serialize)]
struct ClaudeMessageParam {
    role: &'static str,
    content: Vec<ClaudeContentBlockParam>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ClaudeContentBlockParam {
    Text {
        #[serde(rename = "type")]
        block_type: &'static str,
        text: String,
    },
    ToolUse {
        #[serde(rename = "type")]
        block_type: &'static str,
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        #[serde(rename = "type")]
        block_type: &'static str,
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

#[derive(Debug, Serialize)]
struct ClaudeTool {
    name: String,
    description: String,
    input_schema: Value,
}

#[derive(Debug, Deserialize)]
struct ClaudeMessageResponse {
    content: Vec<ClaudeContentBlock>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClaudeContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClaudeStreamEvent {
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: ClaudeStreamContentBlockStart,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta {
        index: usize,
        delta: ClaudeStreamDelta,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClaudeStreamContentBlockStart {
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClaudeStreamDelta {
    #[serde(rename = "text_delta")]
    Text { text: String },
    #[serde(rename = "input_json_delta")]
    InputJson { partial_json: String },
    #[serde(other)]
    Other,
}

#[derive(Debug, Default)]
struct ClaudeStreamState {
    content: String,
    tool_calls: Vec<ClaudeStreamToolCallState>,
}

#[derive(Debug, Default)]
struct ClaudeStreamToolCallState {
    index: usize,
    id: String,
    name: String,
    arguments: String,
}

impl ClaudeStreamState {
    fn apply(
        &mut self,
        event: ClaudeStreamEvent,
        sink: &mut dyn StreamSink,
    ) -> Result<(), ProviderError> {
        match event {
            ClaudeStreamEvent::ContentBlockStart {
                index,
                content_block: ClaudeStreamContentBlockStart::ToolUse { id, name },
            } => {
                self.tool_calls.push(ClaudeStreamToolCallState {
                    index,
                    id,
                    name,
                    arguments: String::new(),
                });
            }
            ClaudeStreamEvent::ContentBlockDelta {
                delta: ClaudeStreamDelta::Text { text },
                ..
            } => {
                sink.on_text(&text)?;
                self.content.push_str(&text);
            }
            ClaudeStreamEvent::ContentBlockDelta {
                index,
                delta: ClaudeStreamDelta::InputJson { partial_json },
            } => {
                // Claude streams tool input as JSON fragments on the content
                // block index that started the tool_use.
                if let Some(tool_call) = self
                    .tool_calls
                    .iter_mut()
                    .find(|tool_call| tool_call.index == index)
                {
                    tool_call.arguments.push_str(&partial_json);
                }
            }
            ClaudeStreamEvent::ContentBlockStart { .. }
            | ClaudeStreamEvent::ContentBlockDelta { .. }
            | ClaudeStreamEvent::Other => {}
        }

        Ok(())
    }

    fn into_message(self) -> Result<Message, ProviderError> {
        let tool_calls = self
            .tool_calls
            .into_iter()
            .map(|tool_call| {
                let arguments =
                    serde_json::from_str::<Value>(&tool_call.arguments).map_err(|error| {
                        ProviderError::new(format!(
                            "invalid tool call arguments for tool '{}': {error}; raw arguments: {}",
                            tool_call.name, tool_call.arguments
                        ))
                    })?;

                if !arguments.is_object() {
                    return Err(ProviderError::new(format!(
                        "invalid tool call arguments for tool '{}': expected JSON object; raw arguments: {arguments}",
                        tool_call.name
                    )));
                }

                Ok(ToolCall::new(tool_call.id, tool_call.name, arguments))
            })
            .collect::<Result<Vec<_>, ProviderError>>()?;

        if tool_calls.is_empty() {
            Ok(Message::assistant(self.content))
        } else {
            Ok(Message::assistant_with_tools(self.content, tool_calls))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::ToolDefinition;
    use serde_json::json;

    #[test]
    fn request_moves_system_messages_to_system_field() {
        let request = build_request(
            "claude-test",
            1024,
            &[Message::system("system prompt"), Message::user("hello")],
            None,
        )
        .unwrap();

        assert_eq!(request.system.unwrap()[0].text, "system prompt");
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].role, "user");
    }

    #[test]
    fn request_maps_tool_observation_to_tool_result_block() {
        let request = build_request(
            "claude-test",
            1024,
            &[Message::observation("toolu_1", "24C")],
            Some(&[]),
        )
        .unwrap();

        match &request.messages[0].content[0] {
            ClaudeContentBlockParam::ToolResult {
                tool_use_id,
                content,
                ..
            } => {
                assert_eq!(tool_use_id, "toolu_1");
                assert_eq!(content, "24C");
            }
            _ => panic!("expected tool_result block"),
        }
    }

    #[test]
    fn request_groups_adjacent_tool_observations_into_one_user_message() {
        let request = build_request(
            "claude-test",
            1024,
            &[
                Message::assistant_with_tools(
                    "reading files",
                    vec![
                        ToolCall::new("toolu_1", "read_file", json!({ "path": "a.txt" })),
                        ToolCall::new("toolu_2", "read_file", json!({ "path": "b.txt" })),
                    ],
                ),
                Message::observation("toolu_1", "A"),
                Message::observation("toolu_2", "B"),
            ],
            Some(&[]),
        )
        .unwrap();

        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.messages[1].role, "user");
        assert_eq!(request.messages[1].content.len(), 2);

        match &request.messages[1].content[0] {
            ClaudeContentBlockParam::ToolResult {
                tool_use_id,
                content,
                ..
            } => {
                assert_eq!(tool_use_id, "toolu_1");
                assert_eq!(content, "A");
            }
            _ => panic!("expected first tool_result block"),
        }
        match &request.messages[1].content[1] {
            ClaudeContentBlockParam::ToolResult {
                tool_use_id,
                content,
                ..
            } => {
                assert_eq!(tool_use_id, "toolu_2");
                assert_eq!(content, "B");
            }
            _ => panic!("expected second tool_result block"),
        }
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
            build_request("claude-test", 1024, &[Message::user("hello")], Some(&tools)).unwrap();

        let tools = request.tools.unwrap();
        assert_eq!(tools[0].name, "echo");
        assert_eq!(tools[0].input_schema["type"], "object");
    }

    #[test]
    fn response_maps_tool_use_blocks_to_internal_message() {
        let response = ClaudeMessageResponse {
            content: vec![
                ClaudeContentBlock::Text {
                    text: "calling echo".to_string(),
                },
                ClaudeContentBlock::ToolUse {
                    id: "toolu_1".to_string(),
                    name: "echo".to_string(),
                    input: json!({ "text": "hi" }),
                },
            ],
        };

        let message = parse_response(response).unwrap();

        assert_eq!(message.content, "calling echo");
        assert_eq!(message.tool_calls.len(), 1);
        assert_eq!(message.tool_calls[0].arguments, json!({ "text": "hi" }));
    }

    #[test]
    fn response_rejects_non_object_tool_arguments() {
        let response = ClaudeMessageResponse {
            content: vec![ClaudeContentBlock::ToolUse {
                id: "toolu_1".to_string(),
                name: "echo".to_string(),
                input: json!("not an object"),
            }],
        };

        let error = parse_response(response).unwrap_err().to_string();

        assert!(error.contains("invalid tool call arguments"));
        assert!(error.contains("raw arguments"));
    }

    #[test]
    fn response_ignores_non_action_content_blocks() {
        let response: ClaudeMessageResponse = serde_json::from_value(json!({
            "content": [
                {
                    "type": "thinking",
                    "thinking": "planning",
                    "signature": "sig"
                },
                {
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "echo",
                    "input": { "text": "hi" }
                }
            ]
        }))
        .unwrap();

        let message = parse_response(response).unwrap();

        assert_eq!(message.content, "");
        assert_eq!(message.tool_calls.len(), 1);
        assert_eq!(message.tool_calls[0].name, "echo");
    }

    #[test]
    fn stream_events_reconstruct_text_message() {
        let events = vec![
            ClaudeStreamEvent::ContentBlockDelta {
                index: 0,
                delta: ClaudeStreamDelta::Text {
                    text: "hel".to_string(),
                },
            },
            ClaudeStreamEvent::ContentBlockDelta {
                index: 0,
                delta: ClaudeStreamDelta::Text {
                    text: "lo".to_string(),
                },
            },
        ];
        let mut state = ClaudeStreamState::default();
        let mut sink = TestSink::default();

        for event in events {
            state.apply(event, &mut sink).unwrap();
        }

        let message = state.into_message().unwrap();
        assert_eq!(message.content, "hello");
        assert_eq!(sink.output, "hello");
    }

    #[test]
    fn stream_events_reconstruct_tool_call() {
        let events = vec![
            ClaudeStreamEvent::ContentBlockStart {
                index: 1,
                content_block: ClaudeStreamContentBlockStart::ToolUse {
                    id: "toolu_1".to_string(),
                    name: "echo".to_string(),
                },
            },
            ClaudeStreamEvent::ContentBlockDelta {
                index: 1,
                delta: ClaudeStreamDelta::InputJson {
                    partial_json: r#"{"text":"#.to_string(),
                },
            },
            ClaudeStreamEvent::ContentBlockDelta {
                index: 1,
                delta: ClaudeStreamDelta::InputJson {
                    partial_json: r#""hi"}"#.to_string(),
                },
            },
        ];
        let mut state = ClaudeStreamState::default();
        let mut sink = TestSink::default();

        for event in events {
            state.apply(event, &mut sink).unwrap();
        }

        let message = state.into_message().unwrap();
        assert_eq!(message.tool_calls.len(), 1);
        assert_eq!(message.tool_calls[0].id, "toolu_1");
        assert_eq!(message.tool_calls[0].arguments, json!({ "text": "hi" }));
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
