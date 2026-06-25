# rust-tiny-claw

Rust learning project for building an Agent Harness lesson by lesson.

The current code is the chapter 1 skeleton: it wires together the major
runtime pieces without implementing the full agent behavior yet.

## Module Map

- `engine`: ReAct main loop and orchestration.
- `provider`: model provider abstraction and concrete adapters.
- `context_engine`: prompt composition, context tracking, compaction, reminders.
- `tools`: tool traits, registry, dispatch, and middleware.
- `memory`: file-backed session state, plans, todos, and working memory.
- `integrations/feishu`: Feishu event and approval integration.
- `telemetry`: token cost, elapsed time, and tracing.

## First Check

```bash
cargo run
```

Expected output includes the provider, registered tools, context manager,
memory root, telemetry, and the next implementation step.
