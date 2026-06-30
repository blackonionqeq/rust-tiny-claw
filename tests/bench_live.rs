// Live capability suite. Real provider, real cost, non-deterministic. Ignored
// by default; run explicitly with real credentials:
//
//   TINY_CLAW_PROVIDER=openai-compatible TINY_CLAW_API_KEY=... \
//     cargo test --test bench_live -- --ignored --nocapture
//
// This mirrors the env-gated pattern used by the other `*_real` integration
// tests.
use rust_tiny_claw::eval::{BenchmarkRunner, Tier};
use std::env;

#[test]
#[ignore = "requires real provider credentials, network access, and spends money"]
fn live_suite_runs_against_real_provider() {
    let _ = dotenvy::dotenv();
    if !live_provider_configured() {
        eprintln!(
            "skipping live benchmark: set TINY_CLAW_PROVIDER=openai-compatible|claude-compatible and TINY_CLAW_API_KEY"
        );
        return;
    }

    let runner = BenchmarkRunner::new(
        Tier::Live,
        env::var("TINY_CLAW_PROVIDER").unwrap_or_default(),
    );
    let report = runner.run(rust_tiny_claw::eval::default_live_suite());

    // We cannot assert a fixed pass rate against a probabilistic model. The
    // value of this test is producing the report. We only assert the runner
    // completed every case without an infrastructure error.
    for result in &report.results {
        let infrastructure_error = result.error.as_ref().is_some_and(|error| {
            error.starts_with("workspace ") || error.starts_with("engine build")
        });
        assert!(
            !infrastructure_error,
            "infrastructure error for {}: {result:?}",
            result.id
        );
    }
    println!("{}", report.to_json().unwrap_or_else(|error| error));
}

fn live_provider_configured() -> bool {
    matches!(
        env::var("TINY_CLAW_PROVIDER").ok().as_deref(),
        Some("openai-compatible" | "claude-compatible")
    ) && env::var("TINY_CLAW_API_KEY")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}
