use rust_tiny_claw::eval::{BenchmarkRunner, Tier};
use std::env;
use std::process::ExitCode;

// Automated benchmark entry point.
//
//   tiny-claw-bench             # deterministic tier (default): no network, no cost
//   tiny-claw-bench --live      # live tier: real provider, costs money
//   tiny-claw-bench --json      # emit the report as JSON instead of a table
//   tiny-claw-bench --keep      # keep per-case workspace dirs for inspection
//
// The deterministic tier is the CI regression gate: it replays scripted model
// turns so a pass-rate drop must come from the engine, tools, compaction, or
// recovery code. The live tier measures real agent capability and requires
// TINY_CLAW_PROVIDER and TINY_CLAW_API_KEY.
fn main() -> ExitCode {
    let _ = dotenvy::dotenv();
    let args: Vec<String> = env::args().skip(1).collect();
    let live = args.iter().any(|arg| arg == "--live");
    let json = args.iter().any(|arg| arg == "--json");
    let keep = args.iter().any(|arg| arg == "--keep");

    for arg in &args {
        if !matches!(arg.as_str(), "--live" | "--json" | "--keep") {
            eprintln!("unknown argument: {arg}");
            eprintln!("usage: tiny-claw-bench [--live] [--json] [--keep]");
            return ExitCode::FAILURE;
        }
    }

    let tier = if live {
        Tier::Live
    } else {
        Tier::Deterministic
    };
    let mut runner = BenchmarkRunner::new(tier, model_label(tier));
    if keep {
        runner = runner.keep_workspaces();
    }

    let cases = if live {
        rust_tiny_claw::eval::default_live_suite()
    } else {
        rust_tiny_claw::eval::default_deterministic_suite()
    };

    let report = runner.run(cases);

    if json {
        match report.to_json() {
            Ok(rendered) => println!("{rendered}"),
            Err(error) => {
                eprintln!("failed to render report: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        report.print_table();
    }

    if report.passed_count() == report.total() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn model_label(tier: Tier) -> String {
    match tier {
        Tier::Deterministic => "scripted".to_string(),
        Tier::Live => env::var("TINY_CLAW_MODEL")
            .or_else(|_| env::var("TINY_CLAW_PROVIDER"))
            .unwrap_or_else(|_| "live".to_string()),
    }
}
