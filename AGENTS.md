# AGENTS.md

## Project Goal

This repository is a Rust rewrite of the `go-tiny-claw` Agent Harness course
project. Build it lesson by lesson. Keep each change aligned with the current
course chapter instead of implementing later features early.

The desired architecture is a small, explicit Harness runtime:

- `engine`: ReAct main loop and orchestration.
- `provider`: model provider abstraction and concrete adapters.
- `context_engine`: prompt composition, context tracking, compaction, reminders.
- `tools`: tool traits, registry, dispatch, and middleware.
- `memory`: file-backed session state, plans, todos, and working memory.
- `integrations/feishu`: Feishu event and approval integration.
- `telemetry`: token cost, elapsed time, and tracing.

## Development Rules

- Prefer Rust/Cargo conventions over copying the Go layout literally.
- Keep the framework minimal. Do not add a heavy agent framework dependency.
- Use clear traits at module boundaries, especially for providers and tools.
- Add real behavior incrementally as the course reaches that topic.
- Keep placeholders small and compileable.
- Avoid implementing Feishu, compaction, subagents, tracing, or benchmarks before
  their matching lessons unless explicitly requested.
- Keep generated course material out of Git. The `1274/` directory and
  `geektime-1274-offline.tar.gz` are intentionally ignored.

## Running And Verification

Use WSL Ubuntu for commands that need to build or run the Rust binary.

Windows-side `cargo check` may work, but `cargo run` currently fails because
`link.exe` resolves to Git for Windows instead of the MSVC linker, and the
Windows SDK link libraries are not available in this shell.

From PowerShell, run:

```powershell
wsl -d Ubuntu -- bash -lc "cd <repo-wsl-path> && cargo fmt --check"
wsl -d Ubuntu -- bash -lc "cd <repo-wsl-path> && cargo check"
wsl -d Ubuntu -- bash -lc "cd <repo-wsl-path> && cargo run"
```

If only type-checking from the Windows shell, this is acceptable:

```powershell
cargo check
```

Before finishing code changes, run at least:

```powershell
wsl -d Ubuntu -- bash -lc "cd <repo-wsl-path> && cargo fmt --check && cargo check"
```

For behavior changes to the startup path, also run:

```powershell
wsl -d Ubuntu -- bash -lc "cd <repo-wsl-path> && cargo run"
```
