use rust_tiny_claw::context_engine::{ContextBudget, ContextManager};
use rust_tiny_claw::engine::{AgentEngine, RunOptions};
use rust_tiny_claw::memory::FileMemory;
use rust_tiny_claw::provider::{
    ClaudeCompatibleProvider, OpenAiCompatibleProvider, Provider, ProviderError,
};
use rust_tiny_claw::schema::{Message, ToolCall, ToolDefinition, ToolResult};
use rust_tiny_claw::telemetry::Telemetry;
use rust_tiny_claw::tools::{Tool, ToolAccessMode, ToolRegistry};
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use tempfile::tempdir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct TraceProvider {
    calls: usize,
}

impl TraceProvider {
    fn new() -> Self {
        Self { calls: 0 }
    }
}

impl Provider for TraceProvider {
    fn name(&self) -> &'static str {
        "trace-provider"
    }

    fn generate(
        &mut self,
        _messages: &[Message],
        available_tools: Option<&[ToolDefinition]>,
    ) -> Result<Message, ProviderError> {
        self.calls += 1;
        if self.calls == 1 {
            assert!(available_tools.is_some());
            return Ok(Message::assistant_with_tools(
                "read fixture",
                vec![ToolCall::new(
                    "call_trace_1",
                    "trace_read",
                    json!({ "path": "fixture.txt" }),
                )],
            ));
        }

        Ok(Message::assistant("trace complete"))
    }
}

struct TraceReadTool;

impl Tool for TraceReadTool {
    fn name(&self) -> &'static str {
        "trace_read"
    }

    fn description(&self) -> &'static str {
        "Test-only read tool for trace integration."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            },
            "required": ["path"]
        })
    }

    fn access_mode(&self, _call: &ToolCall) -> ToolAccessMode {
        ToolAccessMode::ReadOnly
    }

    fn execute(&self, call: &ToolCall) -> ToolResult {
        ToolResult::ok(call.id.clone(), "fixture contents")
    }
}

#[test]
fn mock_run_writes_json_trace_for_engine_and_tool_spans() -> Result<(), Box<dyn std::error::Error>>
{
    let _guard = ENV_LOCK.lock().unwrap();
    let previous_trace = env::var("TINY_CLAW_TRACE").ok();
    let previous_otlp = env::var("TINY_CLAW_OTLP_ENDPOINT").ok();
    set_env("TINY_CLAW_TRACE", "debug");
    remove_env("TINY_CLAW_OTLP_ENDPOINT");

    let result = run_mock_trace_test();

    restore_env("TINY_CLAW_TRACE", previous_trace);
    restore_env("TINY_CLAW_OTLP_ENDPOINT", previous_otlp);
    result
}

fn run_mock_trace_test() -> Result<(), Box<dyn std::error::Error>> {
    let work_dir = tempdir()?;
    fs::write(work_dir.path().join("fixture.txt"), "fixture contents")?;

    let mut registry = ToolRegistry::new();
    registry.register(TraceReadTool)?;
    let memory_root = work_dir.path().join(".tiny-claw");
    let mut engine = AgentEngine::new(
        TraceProvider::new(),
        registry,
        ContextManager::new(work_dir.path(), Vec::new()),
        FileMemory::new(&memory_root),
        Telemetry::default(),
    );

    engine.run_with_options(
        "trace this run",
        RunOptions {
            max_turns: 3,
            enable_thinking: false,
            plan_mode: false,
            stream: false,
            context_budget: ContextBudget::default(),
        },
    )?;

    let spans = read_trace_spans(&memory_root.join("traces"))?;
    assert_span_exists(&spans, "Agent.Run");
    assert_span_exists(&spans, "Agent.Turn");
    assert_span_exists(&spans, "Context.Compaction");
    assert_span_exists(&spans, "LLM.Action");
    assert_span_exists(&spans, "Tool.Execute");

    let tool_span = spans
        .iter()
        .find(|span| span["name"] == "Tool.Execute")
        .expect("tool span should exist");
    assert_attribute(tool_span, "tool.name", "trace_read");
    assert_attribute(tool_span, "tool.call_id", "call_trace_1");
    assert_attribute(tool_span, "tool.success", true);
    assert!(tool_span["parent_span_id"].is_number());

    Ok(())
}

#[test]
#[ignore = "requires real provider credentials and network access"]
fn real_provider_run_writes_json_trace() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = ENV_LOCK.lock().unwrap();
    let _ = dotenvy::dotenv();
    let Some(provider) = real_provider()? else {
        eprintln!(
            "skipping real trace smoke: set TINY_CLAW_PROVIDER to a non-mock provider and TINY_CLAW_API_KEY"
        );
        return Ok(());
    };
    let previous_trace = env::var("TINY_CLAW_TRACE").ok();
    let previous_otlp = env::var("TINY_CLAW_OTLP_ENDPOINT").ok();
    set_env("TINY_CLAW_TRACE", "debug");
    remove_env("TINY_CLAW_OTLP_ENDPOINT");

    let result = run_real_trace_test(provider);

    restore_env("TINY_CLAW_TRACE", previous_trace);
    restore_env("TINY_CLAW_OTLP_ENDPOINT", previous_otlp);
    result
}

fn run_real_trace_test(
    provider: Box<dyn Provider + Send>,
) -> Result<(), Box<dyn std::error::Error>> {
    let work_dir = tempdir()?;
    let memory_root = work_dir.path().join(".tiny-claw");
    let mut engine = AgentEngine::new(
        provider,
        ToolRegistry::new(),
        ContextManager::new(work_dir.path(), Vec::new()),
        FileMemory::new(&memory_root),
        Telemetry::default(),
    );

    let transcript = engine.run_with_options(
        "Reply with exactly this token: trace-ok",
        RunOptions {
            max_turns: 1,
            enable_thinking: false,
            plan_mode: false,
            stream: false,
            context_budget: ContextBudget::default(),
        },
    )?;
    assert!(
        transcript
            .iter()
            .any(|message| message.content.to_ascii_lowercase().contains("trace-ok")),
        "expected real provider transcript to contain trace-ok, got {transcript:?}"
    );

    let spans = read_trace_spans(&memory_root.join("traces"))?;
    assert_span_exists(&spans, "Agent.Run");
    assert_span_exists(&spans, "Agent.Turn");
    assert_span_exists(&spans, "LLM.Action");
    Ok(())
}

fn real_provider() -> Result<Option<Box<dyn Provider + Send>>, Box<dyn std::error::Error>> {
    if !real_provider_is_configured() {
        return Ok(None);
    }

    match env::var("TINY_CLAW_PROVIDER")?.as_str() {
        "openai-compatible" => Ok(Some(Box::new(OpenAiCompatibleProvider::from_env()?))),
        "claude-compatible" => Ok(Some(Box::new(ClaudeCompatibleProvider::from_env()?))),
        _ => Ok(None),
    }
}

fn real_provider_is_configured() -> bool {
    matches!(
        env::var("TINY_CLAW_PROVIDER").as_deref(),
        Ok("openai-compatible" | "claude-compatible")
    ) && env::var("TINY_CLAW_API_KEY")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn read_trace_spans(trace_dir: &Path) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let mut spans = Vec::new();
    for entry in fs::read_dir(trace_dir)? {
        let entry = entry?;
        if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let value: Value = serde_json::from_slice(&fs::read(entry.path())?)?;
        collect_spans(&value, &mut spans);
    }
    Ok(spans)
}

fn collect_spans(value: &Value, spans: &mut Vec<Value>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_spans(value, spans);
            }
        }
        Value::Object(object) => {
            if let Some(span) = object.get("span") {
                spans.push(span.clone());
            }
            if let Some(children) = object.get("children") {
                collect_spans(children, spans);
            }
        }
        _ => {}
    }
}

fn assert_span_exists(spans: &[Value], name: &str) {
    assert!(
        spans.iter().any(|span| span["name"] == name),
        "expected trace span {name:?}, got {spans:#?}"
    );
}

fn assert_attribute(span: &Value, key: &str, expected: impl Into<Value>) {
    let expected = expected.into();
    let attribute = span["attributes"]
        .as_array()
        .and_then(|attributes| {
            attributes
                .iter()
                .find(|attribute| attribute["key"].as_str() == Some(key))
        })
        .unwrap_or_else(|| panic!("expected span attribute {key:?}, got {span:#?}"));
    assert_eq!(attribute["value"]["value"], expected);
}

fn set_env(key: &str, value: &str) {
    unsafe {
        env::set_var(key, value);
    }
}

fn remove_env(key: &str) {
    unsafe {
        env::remove_var(key);
    }
}

fn restore_env(key: &str, value: Option<String>) {
    match value {
        Some(value) => set_env(key, &value),
        None => remove_env(key),
    }
}
