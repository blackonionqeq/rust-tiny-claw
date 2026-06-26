use rust_tiny_claw::context_engine::ContextManager;
use rust_tiny_claw::engine::{AgentEngine, RunOptions};
use rust_tiny_claw::memory::FileMemory;
use rust_tiny_claw::provider::{
    ClaudeCompatibleProvider, MockProvider, OpenAiCompatibleProvider, Provider,
};
use rust_tiny_claw::telemetry::Telemetry;
use rust_tiny_claw::tools::{BashTool, ReadFileTool, ToolRegistry, WriteFileTool};
use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();

    println!("rust-tiny-claw engine boot sequence");

    let provider = build_provider()?;

    let work_dir = env::current_dir()?;

    let mut registry = ToolRegistry::new();
    registry.register(ReadFileTool::new(&work_dir)?)?;
    registry.register(WriteFileTool::new(&work_dir)?)?;
    registry.register(BashTool::new(&work_dir)?)?;

    // TODO(ch10-ch15): load AGENTS.md, manage sessions, compact context, inject reminders.
    let context = ContextManager::default();

    // TODO(ch11/ch14): persist session state, plans, todos, and working memory on disk.
    let memory = FileMemory::new(".tiny-claw");

    // TODO(ch18-ch20): track token cost, elapsed time, and traces.
    let telemetry = Telemetry::default();

    let engine = AgentEngine::new(provider, registry, context, memory, telemetry);
    let mut engine = engine;

    let options = RunOptions {
        max_turns: 6,
        enable_thinking: false,
        stream: stream_enabled()?,
    };

    for line in engine.boot_plan(options) {
        println!("- {line}");
    }

    println!("starting two-stage ReAct loop");
    engine.run_with_options(
        "Smoke-test the lesson 6 minimal tool set: read Cargo.toml, write a small file under .tiny-claw/smoke, then use bash to print that file and confirm the result.",
        options,
    )?;

    Ok(())
}

fn stream_enabled() -> Result<bool, Box<dyn std::error::Error>> {
    match env::var("TINY_CLAW_STREAM") {
        Ok(value) => parse_bool_env("TINY_CLAW_STREAM", &value),
        Err(_) => Ok(true),
    }
}

fn parse_bool_env(name: &str, value: &str) -> Result<bool, Box<dyn std::error::Error>> {
    match value {
        "1" | "true" | "TRUE" | "True" | "yes" | "YES" | "Yes" => Ok(true),
        "0" | "false" | "FALSE" | "False" | "no" | "NO" | "No" => Ok(false),
        _ => Err(format!("invalid {name} value: {value}").into()),
    }
}

fn build_provider() -> Result<Box<dyn Provider>, Box<dyn std::error::Error>> {
    match env::var("TINY_CLAW_PROVIDER")
        .unwrap_or_else(|_| "mock".to_string())
        .as_str()
    {
        "mock" => Ok(Box::new(MockProvider::default())),
        "claude-compatible" => Ok(Box::new(ClaudeCompatibleProvider::from_env()?)),
        "openai-compatible" => Ok(Box::new(OpenAiCompatibleProvider::from_env()?)),
        other => Err(format!("unsupported TINY_CLAW_PROVIDER: {other}").into()),
    }
}
