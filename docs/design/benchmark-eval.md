# Benchmark And Evaluation Harness

## Purpose

When a change touches the fuzzy `edit_file` matcher, the compaction threshold,
or an error-recovery prompt, "it still feels fine in the REPL" is not a
measurement. The benchmark harness turns that into a number: seed an isolated
workspace, let the engine act, then grade the result with an objective
assertion instead of trusting the model's self-report. This is the
test-driven-evaluation idea from SWE-bench (fail-to-pass), scaled down to what
this project needs.

It reuses the telemetry and tracing already built into the engine rather than
introducing a parallel measurement system. A `TelemetrySnapshot` after each run
supplies token counts and tool-failure counts; the run wall-clock supplies
latency; the reporter supplies turns used.

## Scope

First implementation:

- A new `eval` library module: `TestCase`, `Setup`, `Validate`, `TestResult`,
  `SuiteReport`, `BenchmarkRunner`, and a `ScriptedProvider`.
- Two tiers sharing one runner:
  - `Tier::Deterministic` replays a scripted provider so the suite exercises
    real engine/tool mechanics with no network and no API cost.
  - `Tier::Live` drives a real provider so the suite measures actual agent
    capability.
- Cross-platform structured setup/validate (`WriteFile`, `FileContains`) plus
  shell-based setup/validate for richer live testbeds.
- A `tiny-claw-bench` binary entry point.
- A CI-runnable deterministic integration test and an ignored live test.

Out of scope for the first implementation:

- Cost in currency. The report carries token counts (already recorded by
  telemetry). A `cost(usage, model)` pure function plus a price table is the
  natural follow-up, kept separate so stale prices never affect adapter
  correctness.
- Trajectory-level or LLM-as-judge evaluation. The first version is
  outcome-based (pass/fail) only.
- Parallel case execution. Cases run sequentially so each gets a clean,
  non-interleaving workspace and a quiet stdout.
- Persistent on-disk baseline diffing beyond JSON output.

## Architecture

### Two Tiers, One Runner

`BenchmarkRunner` takes a `Tier` and runs each `TestCase` the same way:

1. Create an isolated workspace under the system temp dir
   (`tiny-claw-bench/<slug>-<pid>-<seq>`), removed after the case unless
   `keep_workspaces` is set.
2. Apply `Setup` (write a file or run a shell script).
3. Assemble an engine via `app::build_runtime` with a fresh `Telemetry`, an
   empty skills list, and a provider chosen by the tier.
4. Run the ReAct loop with `stream: false` and `enable_thinking: false` through
   a silent `BenchReporter` that counts turns.
5. Snapshot telemetry, run `Validate`, and assemble a `TestResult`.

The only tier difference is the provider. The deterministic tier wraps a
`ScriptedProvider` in `TelemetryProvider`; the live tier wraps a real provider
built from `TINY_CLAW_PROVIDER`.

### Why The Deterministic Tier Is The CI Gate

The live tier is probabilistic and costs money, so it cannot guard every PR.
The deterministic tier fixes the model side with `ScriptedProvider`: each
`generate` call with tools enabled pops the next scripted assistant message,
and the engine drives the **real** `edit_file`, `read_file`, and dispatch paths
against the isolated workspace. A scripted case that flips from pass to fail
after a change can only mean the engine, tools, compaction, or recovery code
regressed. That is exactly the signal a `Compactor` threshold change or a
fuzzy-edit regex tweak needs.

`ScriptedProvider` implements the existing `Provider` trait, so the engine is
unaware it is being driven by a script. A scripted case ends when a turn
carries no tool calls (the engine treats an empty tool-call list as
completion), so each case finishes with a plain assistant message.

### Data Model

```rust
pub enum Tier { Deterministic, Live }

pub enum Setup {
    WriteFile { path: String, content: String },
    Shell(String),
}

pub enum Validate {
    FileContains { path: String, needle: String },
    Shell(String),
}

pub struct TestCase {
    pub id: String,
    pub name: String,
    pub setup: Option<Setup>,
    pub prompt: String,
    pub validate: Validate,
    pub max_turns: usize,
    pub script: Option<Vec<Message>>,
}

pub struct TestResult {
    pub id: String,
    pub name: String,
    pub passed: bool,
    pub turns_used: usize,      // smoothness: how many ReAct steps the solve took
    pub tool_failures: u64,     // smoothness: tool errors/retries along the way
    pub llm_calls: u64,
    pub total_tokens: u64,
    pub elapsed_ms: u128,
    pub error: Option<String>,
}
```

`turns_used` and `tool_failures` are the two "smoothness" metrics: a clean
one-shot solve and a task that limped through twenty turns with tool errors
both pass by the outcome metric, but only these two distinguish them.
`turns_used` comes from `BenchReporter::on_turn_start`; `tool_failures` comes
straight from `TelemetrySnapshot::tools.failed_call_count`.

### Run Options

Benchmark cases run with `stream: false` because the engine writes streamed
text to a hardcoded `StdoutStreamSink`, which would drown benchmark output. The
reporter is a no-op `BenchReporter` that only records the turn count, so the
suite stays quiet and `Passed` is decided only by `Validate`.

### Setup And Validate Portability

The deterministic suite uses `Setup::WriteFile` and `Validate::FileContains`
exclusively, so it runs anywhere Rust does, with no shell dependency. The live
suite uses `Setup::Shell` / `Validate::Shell` for richer testbeds (for example
`grep`-based assertions) and requires `sh` on PATH, which holds in WSL and on
CI Linux runners.

## Testing

Unit tests (network-free) cover:

- `FileContains` validate passes when the needle is present and fails with a
  helpful message when it is absent.
- `WriteFile` setup creates parent directories.
- `SuiteReport` aggregates pass count, pass rate, tokens, and tool failures,
  and handles the empty suite.
- `SuiteReport` serializes to JSON.

Integration tests:

- `tests/bench_deterministic.rs` runs the default deterministic suite and
  asserts every case passes, each in two turns with no tool failures. This is
  the CI regression gate.
- `tests/bench_live.rs` is `#[ignore]` and env-gated, mirroring the other
  `*_real` tests. It produces a report against a real provider but asserts no
  fixed pass rate, since the live tier is probabilistic.

## Running

```powershell
# Deterministic tier (CI default): no network, no cost
wsl -d Ubuntu -- bash -lc "cd <repo-wsl-path> && cargo run --bin tiny-claw-bench"

# JSON report for baseline diffing
wsl -d Ubuntu -- bash -lc "cd <repo-wsl-path> && cargo run --bin tiny-claw-bench -- --json"

# Live tier: real provider, costs money
wsl -d Ubuntu -- bash -lc "cd <repo-wsl-path> && \
  TINY_CLAW_PROVIDER=openai-compatible TINY_CLAW_API_KEY=... \
  cargo run --bin tiny-claw-bench -- --live"
```

`--keep` retains the per-case workspace directories under the system temp dir
for post-mortem inspection of a failed agent run.

## Future Work

- A `cost(usage, model)` pure function backed by a small price table, plumbed
  into `TestResult` as `cost_usd: Option<f64>` (`None` for unknown models).
- A checked-in JSON baseline and a CI comparison so PRs surface pass-rate or
  token regressions automatically.
- Trajectory-level metrics (tool-call sequence accuracy) and an optional
  LLM-as-judge path for open-ended cases without objective assertions.
- Optional parallel case execution once per-case stdout isolation is in place.
