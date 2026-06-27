use rust_tiny_claw::app::{build_engine, stream_enabled};
use rust_tiny_claw::engine::RunOptions;
use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();

    println!("rust-tiny-claw engine boot sequence");

    let work_dir = env::current_dir()?;
    let mut engine = build_engine(&work_dir)?;

    let options = RunOptions {
        max_turns: 12,
        enable_thinking: false,
        stream: stream_enabled()?,
    };

    for line in engine.boot_plan(options) {
        println!("- {line}");
    }

    println!("starting two-stage ReAct loop");
    engine.run_with_options(
        "Smoke-test the lesson 8 harness. Create .tiny-claw/smoke/edit-target.rs with an indented TODO auth block. Read it once. Then call edit_file exactly once to replace that block with a Forbidden return; in old_text, omit the original indentation so the fuzzy indentation fallback is exercised. Read the file once more to confirm the replacement. Do not repeat the edit flow after it succeeds. Finally, read Cargo.toml, README.md, and src/bin/tiny-claw.rs and call grep for TODO in one independent batch so the engine can execute multiple read-only tool calls in parallel.",
        options,
    )?;

    Ok(())
}
