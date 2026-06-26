use rust_tiny_claw::context_engine::ContextManager;
use rust_tiny_claw::engine::{AgentEngine, RunOptions};
use rust_tiny_claw::memory::FileMemory;
use rust_tiny_claw::provider::{
    ClaudeCompatibleProvider, MockProvider, OpenAiCompatibleProvider, Provider,
};
use rust_tiny_claw::telemetry::Telemetry;
use rust_tiny_claw::tools::{EchoTool, ToolRegistry};
use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();

    println!("rust-tiny-claw engine boot sequence");

    let provider = build_provider()?;

    // TODO(ch06-ch08): register real read/write/edit/bash tools behind middleware.
    let mut registry = ToolRegistry::new();
    registry.register(EchoTool::default());

    // TODO(ch10-ch15): load AGENTS.md, manage sessions, compact context, inject reminders.
    let context = ContextManager::default();

    // TODO(ch11/ch14): persist session state, plans, todos, and working memory on disk.
    let memory = FileMemory::new(".tiny-claw");

    // TODO(ch18-ch20): track token cost, elapsed time, and traces.
    let telemetry = Telemetry::default();

    let engine = AgentEngine::new(provider, registry, context, memory, telemetry);
    let mut engine = engine;

    for line in engine.boot_plan() {
        println!("- {line}");
    }

    println!("starting two-stage ReAct loop");
    engine.run_with_options(
        "Call the echo tool exactly once with the text 'workspace tools are wired', then report the observation and finish.",
        RunOptions {
            max_turns: 4,
            enable_thinking: false,
        },
    )?;

    Ok(())
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
