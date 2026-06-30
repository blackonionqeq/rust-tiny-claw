// CI regression gate. The deterministic tier replays scripted model turns, so
// the real engine, tools, fuzzy edit, and dispatch paths all execute against
// isolated workspaces with no network and no API cost. A failure here means a
// change broke harness mechanics, not that the model got unlucky.
use rust_tiny_claw::eval::{BenchmarkRunner, Tier};

#[test]
fn deterministic_suite_passes() {
    let runner = BenchmarkRunner::new(Tier::Deterministic, "scripted");
    let report = runner.run(rust_tiny_claw::eval::default_deterministic_suite());

    assert_eq!(
        report.passed_count(),
        report.total(),
        "deterministic benchmark suite regressed: {report:#?}"
    );
    // Both scripted cases are two-turn edit flows (action + completion).
    for result in &report.results {
        assert_eq!(
            result.turns_used, 2,
            "expected two turns for {}, got {}",
            result.id, result.turns_used
        );
        assert_eq!(
            result.tool_failures, 0,
            "expected no tool failures for {}, got {}",
            result.id, result.tool_failures
        );
    }
}
