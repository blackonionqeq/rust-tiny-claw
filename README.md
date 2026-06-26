# rust-tiny-claw

Rust learning project for building an Agent Harness lesson by lesson.

The current code is an early runnable harness: it wires together the major
runtime pieces, runs a small two-stage ReAct loop, exposes one placeholder
`echo` tool, and can use either the mock provider or real OpenAI/Claude-compatible
HTTP providers.

## Module Map

- `engine`: ReAct main loop and orchestration.
- `provider`: model provider abstraction and concrete adapters.
- `context_engine`: prompt composition, context tracking, compaction, reminders.
- `tools`: tool traits, registry, dispatch, and middleware.
- `memory`: file-backed session state, plans, todos, and working memory.
- `integrations/feishu`: Feishu event and approval integration.
- `telemetry`: token cost, elapsed time, and tracing.

## First Check

Use WSL Ubuntu for the full build/run path on this machine:

```powershell
wsl -d Ubuntu -- bash -lc "cd /mnt/d/codes/rust-projects/rust-tiny-claw && cargo fmt --check && cargo check"
wsl -d Ubuntu -- bash -lc "cd /mnt/d/codes/rust-projects/rust-tiny-claw && cargo run"
```

For a quick type check from Windows PowerShell, `cargo check` is usually enough.

The default provider is `mock`, so the project can run without an API key:

```bash
cargo run
```

Expected output includes the provider, streaming mode, thinking phase setting,
registered tools, context manager, memory root, telemetry, and a short ReAct
exchange through the `echo` tool.

## Provider Configuration

Runtime configuration is read from environment variables and `.env` via
`dotenvy`. Start from `.env.example` when using a real provider.

Supported values:

- `TINY_CLAW_PROVIDER=mock` uses the built-in deterministic provider.
- `TINY_CLAW_PROVIDER=openai-compatible` sends chat completion requests to
  `{TINY_CLAW_BASE_URL}/chat/completions`.
- `TINY_CLAW_PROVIDER=claude-compatible` sends Messages API requests to
  `{TINY_CLAW_BASE_URL}/v1/messages`.

Common variables:

- `TINY_CLAW_API_KEY`: required for real providers.
- `TINY_CLAW_BASE_URL`: provider base URL.
- `TINY_CLAW_MODEL`: model name.
- `TINY_CLAW_TIMEOUT_SECONDS`: HTTP timeout, default `60`.
- `TINY_CLAW_STREAM`: `true` by default; set `false` to print complete messages
  after each provider call.

Claude-compatible providers also use:

- `TINY_CLAW_MAX_TOKENS`: maximum response tokens, default `4096`.
- `TINY_CLAW_ANTHROPIC_VERSION`: Anthropic API version header, default
  `2023-06-01`.
