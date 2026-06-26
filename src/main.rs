use rust_tiny_claw::context_engine::ContextManager;
use rust_tiny_claw::engine::{AgentEngine, RunOptions};
use rust_tiny_claw::memory::FileMemory;
use rust_tiny_claw::provider::MockProvider;
use rust_tiny_claw::telemetry::Telemetry;
use rust_tiny_claw::tools::{EchoTool, ToolRegistry};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("rust-tiny-claw engine boot sequence");

    // TODO(ch05): replace MockProvider with a real Claude/OpenAI-compatible provider.
    let provider = MockProvider::default();

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

    println!("starting lesson 03 mock two-stage ReAct loop");
    engine.run_with_options(
        "Check whether the minimal agent loop can call a tool.",
        RunOptions {
            max_turns: 4,
            enable_thinking: true,
        },
    )?;

    Ok(())
}
