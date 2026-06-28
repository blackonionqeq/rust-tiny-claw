# rust-tiny-claw

Rust learning project for building an Agent Harness lesson by lesson.

Current chapter state: the harness can run a two-stage ReAct loop, call local
workspace tools, and execute same-turn tool batches in parallel. It supports the
built-in mock provider plus OpenAI/Claude-compatible HTTP providers.

## Run

Use WSL Ubuntu from the repository root for the full build/check path:

```powershell
wsl -d Ubuntu -- bash -lc "cargo fmt --check && cargo check"
```

For a quick type check from Windows PowerShell, `cargo check` is usually enough.

Runtime configuration is loaded from the environment and `.env`. If
`TINY_CLAW_PROVIDER` is unset, the code defaults to `mock`; if `.env` sets a
real provider, `cargo run --bin tiny-claw` will use that provider.

To force the deterministic mock smoke run without an API key:

```powershell
wsl -d Ubuntu -- bash -lc "TINY_CLAW_PROVIDER=mock cargo run --bin tiny-claw"
```

To run a real prompt with the provider configured in `.env`, pass the prompt
after `--`:

```powershell
wsl -d Ubuntu -- bash -lc "cargo run --bin tiny-claw -- 'Read AGENTS.md and summarize the project rules.'"
```

By default, the agent workspace is the current working directory. When running
from this source repository but testing another project, pass the target
workspace explicitly:

```powershell
wsl -d Ubuntu -- bash -lc "cargo run --bin tiny-claw -- --workspace /mnt/d/codes/other-project 'Read AGENTS.md and summarize the project rules.'"
```

The shorter `-C` form is also supported:

```powershell
wsl -d Ubuntu -- bash -lc "cargo run --bin tiny-claw -- -C /mnt/d/codes/other-project 'List the active project rules.'"
```

You can also pipe stdin:

```powershell
wsl -d Ubuntu -- bash -lc "printf 'List the registered tools.\n' | cargo run --bin tiny-claw"
```

When no CLI prompt or stdin is provided, the binary falls back to the
deterministic mock smoke prompt. That smoke run creates an indented file, edits
it with `edit_file`, verifies the result, then requests multiple independent
`read_file` and `grep` calls in one turn to exercise parallel read-only tool
dispatch.

To test explicitly enabled skills, create Codex-style skill files such as
`.tiny-claw/skills/rust/SKILL.md` and set `TINY_CLAW_SKILLS`:

```powershell
wsl -d Ubuntu -- bash -lc "TINY_CLAW_SKILLS=rust cargo run --bin tiny-claw -- -C /mnt/d/codes/other-project 'Use the active skill and inspect this repository.'"
```

Enabled skills are advertised to the model as compact metadata first. The model
can call `load_skill` to load the full `SKILL.md` body when relevant. Add
`disable-model-invocation: true` to a skill's frontmatter to keep it out of the
model-visible catalog and prevent model-initiated loading.

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
- `grep`: searches workspace files with ripgrep-compatible regular expressions,
  optional path narrowing, case sensitivity, context lines, and bounded output.
- `load_skill`: loads the full body for an enabled model-invokable skill from
  `.tiny-claw/skills/<skill-id>/SKILL.md`.

`read_file`, `write_file`, `edit_file`, `grep`, and `load_skill` reject absolute
paths or ids that escape the workspace. `grep` prefers `rg` in `PATH`, falls
back to system `grep` when `rg` is missing, and reports a clear observation if
neither command is available. The fallback may not follow ripgrep's ignore
rules. `bash` follows the course's local YOLO execution model, but still binds
execution to the workspace and enforces resource limits.

## Feishu Integration

Feishu mode is compiled only when the `feishu` feature is enabled. Start from
`.env.feishu.example`, keep Feishu credentials in `.env.feishu`, and run the
callback server explicitly:

```powershell
wsl -d Ubuntu -- bash -lc "cargo run --features feishu --bin tiny-claw-feishu"
```

The first callback endpoint is `POST /feishu/events`. It supports Feishu URL
verification, text message receive events, tenant access token caching, and
plain text replies to the originating chat. It also applies in-process message
deduplication and replies with an unsupported-message notice for non-text
messages. Encrypted callbacks, cards, approvals, persistent deduplication, and
task scheduling are still later integration work.

For Linux server deployment, nginx reverse proxy setup, and release binary
usage, see `docs/usage/feishu.md`.

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
