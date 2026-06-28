# TODO

## Provider

- Add adaptive reasoning mode to replace the fixed `enable_thinking` boolean.
- Add provider retry and backoff policy.
- Consider vendor-specific profiles only after at least two real providers diverge in behavior or configuration.

## Tool Calling

- Later: support a self-correction retry when tool call JSON is invalid.
- Later: add a global concurrency limit for parallel tool batches before adding
  high-volume network tools.
- Later: consider path-based file locks only when the engine needs finer-grained
  scheduling than the current batch policy, such as concurrent writes to
  different files or cross-turn resource protection for background tasks.
- Later: add explicit background task tools (`start_bg_task`, `read_bg_task`,
  `stop_bg_task`, `list_bg_tasks`) for long-running local commands; see
  `docs/todo/background-tasks.md`.
- Expand tests for multi-turn tool call histories and provider-specific edge cases.
- Later: evaluate head/tail previews and shared tool-output offloading on top of
  the existing ranged `read_file` support.

## Runtime

- Add a minimal integration smoke test gated by a real API key.
- Add provider/runtime CLI flags when environment-only configuration becomes
  limiting. Prompt input and workspace override are already available through
  positional CLI text, stdin, `--workspace`, and `-C`.

## Context And Memory

- Later: revisit working memory budgeting when token usage tracking lands.
  The current session window is message-count based. Keep full session history
  intact, but move provider request assembly toward a context-engine budget that
  can combine max messages, estimated input tokens or characters, tool
  call/observation pairing, and later real provider token telemetry.

## Integrations

- Track Feishu gateway follow-ups in `docs/todo/feishu.md`.
