# Integration Test TODO

## Current Coverage

The default test suite covers most pure runtime behavior with mock providers:
tool dispatch, context compaction, recovery guidance, reminders, skill catalog
loading, subagent supervision, telemetry wrappers, and JSON trace export.

Default integration tests currently cover:

- CLI startup through the real `tiny-claw` binary with the mock provider,
  registered tools, session memory root, and fallback smoke prompt.
- `load_skill` progressive loading with a mock provider, proving that the
  initial prompt contains catalog metadata but not the full skill body.

Ignored real-provider smoke tests currently cover:

- Non-streaming and streaming provider usage telemetry.
- Real-provider JSON trace output for a simple prompt.
- Subagent delegation and joined reports.
- Edit recovery after an `edit_file` old-text mismatch.
- System reminders after repeated tool failures.
- Main-agent successful tool loop with `read_file`, `write_file`, `edit_file`,
  and `grep`.
- Streaming tool-call reconstruction followed by engine dispatch and final
  answer.
- Real-provider `load_skill` tool-call compatibility with an enabled workspace
  skill.

Feature-gated integration tests currently cover:

- Feishu router construction, approval card-action fixture parsing, approval
  resolution response shaping, and `tiny-claw-feishu` feature compilation.

## Gaps

- Later, deepen Feishu fixture integration around message event handling and
  approval middleware once the Feishu client boundary is easier to fake without
  real network calls.
- Implement real OTLP export before adding mock-collector integration coverage.
  The current `OtlpTraceExporter` only validates and normalizes the endpoint;
  its `export` method is still a placeholder.

## Suggested Order

1. Keep the CLI-level mock smoke test in the default suite. It is deterministic,
   network-free, and covers the real startup path that direct `AgentEngine`
   tests bypass.
2. Keep real-provider tool-loop, streaming tool-call, and `load_skill` tests
   ignored by default. Run them manually when changing provider adapters, tool
   schema handling, streaming reconstruction, or prompt skill loading.
3. Keep Feishu checks feature-gated. Use `cargo check --features feishu --bin
   tiny-claw-feishu` plus fixture tests until the client boundary supports
   fuller network-free event handling tests.
4. Add OTLP mock-collector coverage only after `OtlpTraceExporter` maps
   `TraceSpanRecord` batches to real OTLP HTTP/protobuf payloads.

## Test Design Notes

- Default integration tests should use mock providers or local fixtures only.
  They should be stable enough for normal CI and should assert concrete runtime
  artifacts such as files, trace spans, transcript tool calls, and process exit
  status.
- Real-provider tests should remain narrow smoke tests. They should validate
  provider compatibility with the runtime contract, not broad model behavior.
- CLI smoke tests should prefer the built test binary path when available, such
  as Cargo's `CARGO_BIN_EXE_tiny-claw`, instead of shelling out to nested
  `cargo run` from inside the test.
- Workspace smoke tests should seed every file named by the prompt in the temp
  workspace so failures point to runtime behavior rather than missing fixtures.
- Assertions should favor tool-call presence, output files, persisted reports,
  telemetry snapshots, or trace records over exact final assistant wording.

## Notes

- Keep real-provider tests ignored by default and gated by
  `TINY_CLAW_PROVIDER` plus `TINY_CLAW_API_KEY`.
- Prefer small prompts with deterministic assertions over broad behavioral
  expectations. Assert concrete tool calls, output files, trace spans, or
  transcript observations whenever possible.
- Keep each smoke focused on one runtime contract so provider variance is easy
  to diagnose.
