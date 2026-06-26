# rust-tiny-claw

Rust learning project for building an Agent Harness lesson by lesson.

Current chapter state: the harness can run a two-stage ReAct loop, call local
workspace tools, and execute same-turn tool batches in parallel. It supports the
built-in mock provider plus OpenAI/Claude-compatible HTTP providers.

## Run

Use WSL Ubuntu for the full build/run path:

```powershell
wsl -d Ubuntu -- bash -lc "cd /mnt/d/codes/rust-projects/rust-tiny-claw && cargo fmt --check && cargo check"
wsl -d Ubuntu -- bash -lc "cd /mnt/d/codes/rust-projects/rust-tiny-claw && cargo run"
```

For a quick type check from Windows PowerShell, `cargo check` is usually enough.

The default provider is `mock`, so the project can run without an API key:

```bash
cargo run
```

The mock smoke run creates an indented file, edits it with `edit_file`, verifies
the result, then requests multiple independent `read_file` calls in one turn to
exercise parallel tool dispatch.

## Tool Dispatch

The harness supports lesson 8 parallel tool calling for read-only batches. When
a provider returns multiple read-only tool calls in the same assistant message,
the engine forks those calls onto scoped Rust threads, waits for all of them to
finish, then appends the observations in the original tool-call order. If any
call in the batch may mutate the workspace, the engine keeps the batch
sequential.

This implementation intentionally follows the course scope: it trusts the
model's same-turn independence assumption for read-only exploration and does
not yet add path-based file locks, async file APIs, or a global concurrency
limit. Those are production hardening topics for later lessons.

## Tool Set

Registered workspace tools:

- `read_file`: reads a workspace-relative file with optional line ranges.
- `write_file`: creates or fully overwrites a workspace-relative file, creating
  parent directories as needed.
- `bash`: runs a bash command from the workspace, combines stdout/stderr, returns
  non-zero exits as observations for model self-correction, applies a 30-second
  timeout, and truncates long output.
- `edit_file`: replaces one existing text block in a workspace file. It tries
  exact matching first, then conservative formatting fallbacks for newline,
  surrounding whitespace, and indentation differences. Ambiguous matches fail
  and ask the model to provide more context.

`read_file`, `write_file`, and `edit_file` reject absolute paths and paths that
escape the workspace. `bash` follows the course's local YOLO execution model,
but still binds execution to the workspace and enforces resource limits.

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
