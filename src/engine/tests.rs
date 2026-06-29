use super::AgentEngine;
use crate::context_engine::{ContextBudget, ContextManager};
use crate::memory::{FileMemory, Session};
use crate::provider::{Provider, ProviderError};
use crate::schema::{Message, Role, ToolCall, ToolDefinition, ToolResult};
use crate::telemetry::Telemetry;
use crate::tools::{Tool, ToolAccessMode, ToolRegistry};
use serde_json::json;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::tempdir;

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

struct FailingThenCapturingProvider {
    calls: Arc<Mutex<Vec<Vec<Message>>>>,
    call_count: usize,
}

impl FailingThenCapturingProvider {
    fn new(calls: Arc<Mutex<Vec<Vec<Message>>>>) -> Self {
        Self {
            calls,
            call_count: 0,
        }
    }
}

impl Provider for FailingThenCapturingProvider {
    fn name(&self) -> &'static str {
        "failing-then-capturing"
    }

    fn generate(
        &mut self,
        messages: &[Message],
        _available_tools: Option<&[ToolDefinition]>,
    ) -> Result<Message, ProviderError> {
        self.calls.lock().unwrap().push(messages.to_vec());
        self.call_count += 1;
        if self.call_count <= 3 {
            return Ok(Message::assistant_with_tools(
                "",
                vec![ToolCall::new(
                    format!("call_{}", self.call_count),
                    "edit_file",
                    json!({ "path": format!("src/file_{}.rs", self.call_count) }),
                )],
            ));
        }

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

struct ErrorTool;

impl Tool for ErrorTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn description(&self) -> &'static str {
        "Test-only failing edit tool."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    fn execute(&self, call: &ToolCall) -> ToolResult {
        ToolResult::error(call.id.clone(), "old_text was not found in the file")
    }
}

#[test]
fn parallel_tool_batch_preserves_call_order() {
    let work_dir = tempdir().unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(DelayTool).unwrap();

    let engine = AgentEngine::new(
        NoopProvider,
        registry,
        ContextManager::default(),
        FileMemory::new(work_dir.path()),
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

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].tool_call_id, "call_1");
    assert_eq!(results[0].output, "slow");
    assert_eq!(results[1].tool_call_id, "call_2");
    assert_eq!(results[1].output, "fast");
}

#[test]
fn mutating_tool_batch_runs_sequentially() {
    let work_dir = tempdir().unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(RecordingTool::new()).unwrap();

    let engine = AgentEngine::new(
        NoopProvider,
        registry,
        ContextManager::default(),
        FileMemory::new(work_dir.path()),
        Telemetry::default(),
    );
    let results = engine.execute_tool_batch(&[
        ToolCall::new("call_1", "record", json!({ "label": "first" })),
        ToolCall::new("call_2", "record", json!({ "label": "second" })),
    ]);

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].output, "");
    assert_eq!(results[1].output, "first");
}

#[test]
fn tool_errors_are_enhanced_with_recovery_guidance() {
    let work_dir = tempdir().unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(ErrorTool).unwrap();

    let engine = AgentEngine::new(
        NoopProvider,
        registry,
        ContextManager::default(),
        FileMemory::new(work_dir.path()),
        Telemetry::default(),
    );
    let results = engine.execute_tool_batch(&[ToolCall::new(
        "call_1",
        "edit_file",
        json!({ "path": "src/main.rs" }),
    )]);

    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    assert_eq!(results[0].tool_call_id, "call_1");
    assert!(results[0].output.contains("Tool call failed."));
    assert!(
        results[0]
            .output
            .contains("error_code: EDIT_TEXT_NOT_FOUND")
    );
    assert!(
        results[0]
            .output
            .contains("old_text was not found in the file")
    );
    assert!(results[0].output.contains("Read the target file again"));
}

#[test]
fn run_injects_system_reminder_after_repeated_tool_failures() {
    let work_dir = tempdir().unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(ErrorTool).unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut engine = AgentEngine::new(
        FailingThenCapturingProvider::new(Arc::clone(&calls)),
        registry,
        ContextManager::new(work_dir.path(), Vec::new()),
        FileMemory::new(work_dir.path()),
        Telemetry::default(),
    );

    let transcript = engine
        .run_with_options(
            "try a few edits",
            super::RunOptions {
                max_turns: 4,
                enable_thinking: false,
                plan_mode: false,
                stream: false,
                context_budget: ContextBudget::default(),
            },
        )
        .unwrap();

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    let fourth_call_messages = &calls[3];
    assert!(
        fourth_call_messages
            .iter()
            .any(|message| message.content.contains("[SYSTEM REMINDER]")
                && message.content.contains("EDIT_TEXT_NOT_FOUND"))
    );
    assert!(
        transcript
            .iter()
            .any(|message| message.content.contains("[SYSTEM REMINDER]"))
    );
}

#[test]
fn run_uses_context_manager_system_prompt() {
    let work_dir = tempdir().unwrap();
    fs::write(
        work_dir.path().join("AGENTS.md"),
        "Follow workspace instructions.\n",
    )
    .unwrap();
    let skill_dir = work_dir
        .path()
        .join(".tiny-claw")
        .join("skills")
        .join("rust");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "# Rust Skill\nPrefer cargo.\n").unwrap();

    let seen_messages = Arc::new(Mutex::new(None));
    let mut engine = AgentEngine::new(
        CapturingProvider::new(Arc::clone(&seen_messages)),
        ToolRegistry::new(),
        ContextManager::new(work_dir.path(), vec!["rust".to_string()]),
        FileMemory::new(work_dir.path()),
        Telemetry::default(),
    );

    engine
        .run_with_options(
            "hello",
            super::RunOptions {
                max_turns: 1,
                enable_thinking: false,
                plan_mode: false,
                stream: false,
                context_budget: ContextBudget::default(),
            },
        )
        .unwrap();

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
fn run_includes_plan_mode_in_system_prompt_when_enabled() {
    let work_dir = tempdir().unwrap();

    let seen_messages = Arc::new(Mutex::new(None));
    let mut engine = AgentEngine::new(
        CapturingProvider::new(Arc::clone(&seen_messages)),
        ToolRegistry::new(),
        ContextManager::new(work_dir.path(), Vec::new()),
        FileMemory::new(work_dir.path()),
        Telemetry::default(),
    );

    engine
        .run_with_options(
            "continue the task",
            super::RunOptions {
                max_turns: 1,
                enable_thinking: false,
                plan_mode: true,
                stream: false,
                context_budget: ContextBudget::default(),
            },
        )
        .unwrap();

    let messages = seen_messages.lock().unwrap().clone().unwrap();
    assert_eq!(messages[0].role, Role::System);
    assert!(messages[0].content.contains("# Plan Mode"));
    assert!(messages[0].content.contains("PLAN.md"));
    assert!(messages[0].content.contains("TODO.md"));
}

#[test]
fn run_session_sends_full_history_to_provider_when_under_budget() {
    let work_dir = tempdir().unwrap();

    let seen_messages = Arc::new(Mutex::new(None));
    let mut engine = AgentEngine::new(
        CapturingProvider::new(Arc::clone(&seen_messages)),
        ToolRegistry::new(),
        ContextManager::new(work_dir.path(), Vec::new()),
        FileMemory::new(work_dir.path()),
        Telemetry::default(),
    );
    let session = Session::new("chat_1", work_dir.path());
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
                plan_mode: false,
                stream: false,
                context_budget: ContextBudget::default(),
            },
            &mut crate::reporter::terminal::TerminalReporter,
        )
        .unwrap();

    let messages = seen_messages.lock().unwrap().clone().unwrap();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].role, Role::System);
    assert_eq!(messages[1].content, "old user message");
    assert_eq!(messages[2].content, "recent assistant message");
    assert_eq!(messages[3].content, "current user message");
}

#[test]
fn run_session_compacts_provider_context_without_mutating_session_history() {
    let work_dir = tempdir().unwrap();

    let seen_messages = Arc::new(Mutex::new(None));
    let mut engine = AgentEngine::new(
        CapturingProvider::new(Arc::clone(&seen_messages)),
        ToolRegistry::new(),
        ContextManager::new(work_dir.path(), Vec::new()),
        FileMemory::new(work_dir.path()),
        Telemetry::default(),
    );
    let session = Session::new("chat_1", work_dir.path());
    let long_output = "0123456789".repeat(30);
    session.append_many([
        Message::user("please read the log"),
        Message::assistant_with_tools(
            "",
            vec![ToolCall::new(
                "call_1",
                "read_file",
                json!({ "path": "large.log" }),
            )],
        ),
        Message::observation("call_1", long_output.clone()),
        Message::user("next task"),
    ]);

    engine
        .run_session_with_reporter(
            &session,
            "current user message",
            super::RunOptions {
                max_turns: 1,
                enable_thinking: false,
                plan_mode: false,
                stream: false,
                context_budget: ContextBudget {
                    max_chars: 80,
                    retain_recent_messages: 2,
                    max_recent_observation_chars: 40,
                    far_observation_mask_chars: 20,
                    far_assistant_fold_chars: 20,
                    head_chars: 8,
                    tail_chars: 8,
                },
            },
            &mut crate::reporter::terminal::TerminalReporter,
        )
        .unwrap();

    let messages = seen_messages.lock().unwrap().clone().unwrap();
    let compacted_observation = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("call_1"))
        .expect("provider context should include the tool observation");
    assert!(
        compacted_observation
            .content
            .contains("tool output compacted")
    );
    assert_eq!(
        compacted_observation.tool_call_id.as_deref(),
        Some("call_1")
    );
    assert!(messages.iter().any(|message| {
        message
            .tool_calls
            .iter()
            .any(|tool_call| tool_call.id == "call_1")
    }));

    let history = session.history();
    assert!(
        history
            .iter()
            .any(|message| message.tool_call_id.as_deref() == Some("call_1")
                && message.content == long_output)
    );
}

#[test]
fn run_session_keeps_provider_context_isolated_by_session() {
    let work_dir = tempdir().unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut engine = AgentEngine::new(
        RecordingProvider::new(Arc::clone(&calls)),
        ToolRegistry::new(),
        ContextManager::new(work_dir.path(), Vec::new()),
        FileMemory::new(work_dir.path()),
        Telemetry::default(),
    );
    let front = Session::new("front", work_dir.path());
    let back = Session::new("back", work_dir.path());
    let options = super::RunOptions {
        max_turns: 1,
        enable_thinking: false,
        plan_mode: false,
        stream: false,
        context_budget: ContextBudget::default(),
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
