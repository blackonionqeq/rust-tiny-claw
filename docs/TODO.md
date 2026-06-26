# TODO

## Provider

- Add adaptive reasoning mode to replace the fixed `enable_thinking` boolean.
- Add provider retry and backoff policy.
- Consider vendor-specific profiles only after at least two real providers diverge in behavior or configuration.

## Tool Calling

- Later: support a self-correction retry when tool call JSON is invalid.
- Later: add explicit background task tools (`start_bg_task`, `read_bg_task`,
  `stop_bg_task`, `list_bg_tasks`) for long-running local commands; see
  `docs/background-task-notes.md`.
- Later: consider a workspace search tool backed by ripgrep, returning matching
  file paths, line numbers, and small context windows so models can locate code
  before calling `read_file` and `edit_file`.
- Expand tests for multi-turn tool call histories and provider-specific edge cases.
- Later: evaluate head/tail previews and shared tool-output offloading on top of
  the existing ranged `read_file` support.

## Runtime

- Add a minimal integration smoke test gated by a real API key.
- Add CLI flags when environment-only configuration becomes limiting.
