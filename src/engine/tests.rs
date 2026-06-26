use super::AgentEngine;
use crate::context_engine::ContextManager;
use crate::memory::FileMemory;
use crate::provider::{Provider, ProviderError};
use crate::schema::{Message, ToolCall, ToolDefinition, ToolResult};
use crate::telemetry::Telemetry;
use crate::tools::{Tool, ToolAccessMode, ToolRegistry};
use serde_json::json;
use std::fs;
use std::sync::Mutex;
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

fn unique_temp_dir() -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("rust-tiny-claw-engine-test-{suffix}"))
}
