# Integration Test TODO

## Current Coverage

The default test suite covers most pure runtime behavior with mock providers:
tool dispatch, context compaction, recovery guidance, reminders, skill catalog
loading, subagent supervision, telemetry wrappers, and JSON trace export.

Ignored real-provider smoke tests currently cover:

- Non-streaming and streaming provider usage telemetry.
- Real-provider JSON trace output for a simple prompt.
- Subagent delegation and joined reports.
- Edit recovery after an `edit_file` old-text mismatch.
- System reminders after repeated tool failures.

## Gaps

- Add a CLI-level mock smoke test that runs `cargo run --bin tiny-claw` and
  verifies the real startup path: environment loading, default mock provider,
  registered tools, session memory root, and the fallback smoke prompt.
- Add a real-provider main-agent tool loop smoke test where the model uses
  `read_file`, `write_file`, `edit_file`, and `grep` to complete a small
  workspace task. The current real-provider tests mostly cover telemetry,
  recovery, tracing, and subagents rather than the ordinary successful tool
  workflow.
- Add a real-provider streaming tool-call smoke test. Existing streaming
  coverage checks text and usage, but not a streamed tool call followed by
  engine dispatch and a final answer.
- Add a real-provider `load_skill` smoke test. The test should create an
  enabled workspace skill, ensure only catalog metadata is present initially,
  then ask the model to call `load_skill` and follow the loaded instructions.
- Add Feishu feature verification to the normal release checklist, at minimum
  `cargo check --features feishu --bin tiny-claw-feishu`. Later, add a fixture
  integration test for event handling plus approval middleware behavior.
- Add an OTLP integration test with a local mock collector so the exporter path
  is checked beyond endpoint normalization.

## Notes

- Keep real-provider tests ignored by default and gated by
  `TINY_CLAW_PROVIDER` plus `TINY_CLAW_API_KEY`.
- Prefer small prompts with deterministic assertions over broad behavioral
  expectations. Assert concrete tool calls, output files, trace spans, or
  transcript observations whenever possible.
- Keep each smoke focused on one runtime contract so provider variance is easy
  to diagnose.
