use super::AgentEngine;
use crate::context_engine::ContextManager;
use crate::memory::{FileMemory, Session};
use crate::provider::{Provider, ProviderError};
use crate::schema::{Message, Role, ToolCall, ToolDefinition, ToolResult};
use crate::telemetry::Telemetry;
use crate::tools::{Tool, ToolAccessMode, ToolRegistry};
use serde_json::json;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct NoopProvider;

impl Provider for NoopProvider {
    fn name(&self) -> &'static str {
        "noop"
    }

    fn generate(
        &mut self,
        _messages: &[Message],
        _available_tools: Option<&[ToolDefinition]>,
    ) -> Result<Message, ProviderError> {
        Ok(Message::assistant("done"))
    }
}

struct CapturingProvider {
    seen_messages: Arc<Mutex<Option<Vec<Message>>>>,
}

impl CapturingProvider {
    fn new(seen_messages: Arc<Mutex<Option<Vec<Message>>>>) -> Self {
        Self { seen_messages }
    }
}

impl Provider for CapturingProvider {
    fn name(&self) -> &'static str {
        "capturing"
    }

    fn generate(
        &mut self,
        messages: &[Message],
        _available_tools: Option<&[ToolDefinition]>,
    ) -> Result<Message, ProviderError> {
        *self.seen_messages.lock().unwrap() = Some(messages.to_vec());
        Ok(Message::assistant("done"))
    }
}

struct RecordingProvider {
    calls: Arc<Mutex<Vec<Vec<Message>>>>,
}

impl RecordingProvider {
    fn new(calls: Arc<Mutex<Vec<Vec<Message>>>>) -> Self {
        Self { calls }
    }
}

impl Provider for RecordingProvider {
    fn name(&self) -> &'static str {
        "recording"
    }

    fn generate(
        &mut self,
        messages: &[Message],
        _available_tools: Option<&[ToolDefinition]>,
    ) -> Result<Message, ProviderError> {
        self.calls.lock().unwrap().push(messages.to_vec());
        Ok(Message::assistant("done"))
    }
}

struct DelayTool;

impl Tool for DelayTool {
    fn name(&self) -> &'static str {
        "delay"
    }

    fn description(&self) -> &'static str {
        "Test-only delay tool."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "label": { "type": "string" },
                "sleep_ms": { "type": "integer" }
            },
            "required": ["label", "sleep_ms"]
        })
    }

    fn access_mode(&self, call: &ToolCall) -> ToolAccessMode {
        match call
            .arguments
            .get("read_only")
            .and_then(|value| value.as_bool())
        {
            Some(true) => ToolAccessMode::ReadOnly,
            _ => ToolAccessMode::MutatesWorkspace,
        }
    }

    fn execute(&self, call: &ToolCall) -> ToolResult {
        let label = call
            .arguments
            .get("label")
            .and_then(|value| value.as_str())
            .unwrap_or("missing");
        let sleep_ms = call
            .arguments
            .get("sleep_ms")
            .and_then(|value| value.as_u64())
            .unwrap_or_default();

        std::thread::sleep(Duration::from_millis(sleep_ms));
        ToolResult::ok(call.id.clone(), label)
    }
}

struct RecordingTool {
    starts: Mutex<Vec<String>>,
}

impl RecordingTool {
    fn new() -> Self {
        Self {
            starts: Mutex::new(Vec::new()),
        }
    }
}

impl Tool for RecordingTool {
    fn name(&self) -> &'static str {
        "record"
    }

    fn description(&self) -> &'static str {
        "Test-only recording tool."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "label": { "type": "string" },
                "read_only": { "type": "boolean" }
            },
            "required": ["label"]
        })
    }

    fn access_mode(&self, call: &ToolCall) -> ToolAccessMode {
        match call
            .arguments
            .get("read_only")
            .and_then(|value| value.as_bool())
        {
            Some(true) => ToolAccessMode::ReadOnly,
            _ => ToolAccessMode::MutatesWorkspace,
        }
    }

    fn execute(&self, call: &ToolCall) -> ToolResult {
        let label = call
            .arguments
            .get("label")
            .and_then(|value| value.as_str())
            .unwrap_or("missing")
            .to_string();

        let mut starts = self.starts.lock().unwrap();
        let previous = starts.join(",");
        starts.push(label);

        ToolResult::ok(call.id.clone(), previous)
    }
}

#[test]
fn parallel_tool_batch_preserves_call_order() {
    let work_dir = unique_temp_dir();
    fs::create_dir_all(&work_dir).unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(DelayTool).unwrap();

    let engine = AgentEngine::new(
        NoopProvider,
        registry,
        ContextManager::default(),
        FileMemory::new(&work_dir),
        Telemetry::default(),
    );
    let results = engine.execute_tool_batch(&[
        ToolCall::new(
            "call_1",
            "delay",
            json!({ "label": "slow", "sleep_ms": 40, "read_only": true }),
        ),
        ToolCall::new(
            "call_2",
            "delay",
            json!({ "label": "fast", "sleep_ms": 0, "read_only": true }),
        ),
    ]);

    fs::remove_dir_all(&work_dir).unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].tool_call_id, "call_1");
    assert_eq!(results[0].output, "slow");
    assert_eq!(results[1].tool_call_id, "call_2");
    assert_eq!(results[1].output, "fast");
}

#[test]
fn mutating_tool_batch_runs_sequentially() {
    let work_dir = unique_temp_dir();
    fs::create_dir_all(&work_dir).unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(RecordingTool::new()).unwrap();

    let engine = AgentEngine::new(
        NoopProvider,
        registry,
        ContextManager::default(),
        FileMemory::new(&work_dir),
        Telemetry::default(),
    );
    let results = engine.execute_tool_batch(&[
        ToolCall::new("call_1", "record", json!({ "label": "first" })),
        ToolCall::new("call_2", "record", json!({ "label": "second" })),
    ]);

    fs::remove_dir_all(&work_dir).unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].output, "");
    assert_eq!(results[1].output, "first");
}

#[test]
fn run_uses_context_manager_system_prompt() {
    let work_dir = unique_temp_dir();
    fs::create_dir_all(&work_dir).unwrap();
    fs::write(
        work_dir.join("AGENTS.md"),
        "Follow workspace instructions.\n",
    )
    .unwrap();
    let skill_dir = work_dir.join(".tiny-claw").join("skills").join("rust");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "# Rust Skill\nPrefer cargo.\n").unwrap();

    let seen_messages = Arc::new(Mutex::new(None));
    let mut engine = AgentEngine::new(
        CapturingProvider::new(Arc::clone(&seen_messages)),
        ToolRegistry::new(),
        ContextManager::new(&work_dir, vec!["rust".to_string()]),
        FileMemory::new(&work_dir),
        Telemetry::default(),
    );

    engine
        .run_with_options(
            "hello",
            super::RunOptions {
                max_turns: 1,
                enable_thinking: false,
                stream: false,
                working_memory_messages: 12,
            },
        )
        .unwrap();

    fs::remove_dir_all(&work_dir).unwrap();

    let messages = seen_messages.lock().unwrap().clone().unwrap();
    assert_eq!(messages[0].role, Role::System);
    assert!(
        messages[0]
            .content
            .contains("Follow workspace instructions.")
    );
    assert!(messages[0].content.contains("# Available Skills"));
    assert!(messages[0].content.contains("id: rust"));
    assert!(!messages[0].content.contains("Prefer cargo."));
}

#[test]
fn run_session_sends_bounded_working_memory_to_provider() {
    let work_dir = unique_temp_dir();
    fs::create_dir_all(&work_dir).unwrap();

    let seen_messages = Arc::new(Mutex::new(None));
    let mut engine = AgentEngine::new(
        CapturingProvider::new(Arc::clone(&seen_messages)),
        ToolRegistry::new(),
        ContextManager::new(&work_dir, Vec::new()),
        FileMemory::new(&work_dir),
        Telemetry::default(),
    );
    let session = Session::new("chat_1", &work_dir);
    session.append_many([
        Message::user("old user message"),
        Message::assistant("recent assistant message"),
    ]);

    engine
        .run_session_with_reporter(
            &session,
            "current user message",
            super::RunOptions {
                max_turns: 1,
                enable_thinking: false,
                stream: false,
                working_memory_messages: 2,
            },
            &mut crate::reporter::terminal::TerminalReporter,
        )
        .unwrap();

    fs::remove_dir_all(&work_dir).unwrap();

    let messages = seen_messages.lock().unwrap().clone().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].role, Role::System);
    assert_eq!(messages[1].content, "recent assistant message");
    assert_eq!(messages[2].content, "current user message");
}

#[test]
fn run_session_keeps_provider_context_isolated_by_session() {
    let work_dir = unique_temp_dir();
    fs::create_dir_all(&work_dir).unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut engine = AgentEngine::new(
        RecordingProvider::new(Arc::clone(&calls)),
        ToolRegistry::new(),
        ContextManager::new(&work_dir, Vec::new()),
        FileMemory::new(&work_dir),
        Telemetry::default(),
    );
    let front = Session::new("front", &work_dir);
    let back = Session::new("back", &work_dir);
    let options = super::RunOptions {
        max_turns: 1,
        enable_thinking: false,
        stream: false,
        working_memory_messages: 12,
    };

    engine
        .run_session_with_reporter(
            &front,
            "front first request",
            options,
            &mut crate::reporter::terminal::TerminalReporter,
        )
        .unwrap();
    engine
        .run_session_with_reporter(
            &back,
            "back only request",
            options,
            &mut crate::reporter::terminal::TerminalReporter,
        )
        .unwrap();
    engine
        .run_session_with_reporter(
            &front,
            "front second request",
            options,
            &mut crate::reporter::terminal::TerminalReporter,
        )
        .unwrap();

    fs::remove_dir_all(&work_dir).unwrap();

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert_context_contains(&calls[0], "front first request");
    assert_context_excludes(&calls[0], "back only request");
    assert_context_contains(&calls[1], "back only request");
    assert_context_excludes(&calls[1], "front first request");
    assert_context_contains(&calls[2], "front first request");
    assert_context_contains(&calls[2], "front second request");
    assert_context_excludes(&calls[2], "back only request");
}

fn assert_context_contains(messages: &[Message], content: &str) {
    assert!(
        messages.iter().any(|message| message.content == content),
        "expected provider context to contain {content:?}, got {messages:?}"
    );
}

fn assert_context_excludes(messages: &[Message], content: &str) {
    assert!(
        messages.iter().all(|message| message.content != content),
        "expected provider context to exclude {content:?}, got {messages:?}"
    );
}

fn unique_temp_dir() -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("rust-tiny-claw-engine-test-{suffix}"))
}
