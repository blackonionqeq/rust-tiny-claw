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
- Track subagent runtime follow-ups in `docs/todo/subagents.md`.

## Context And Memory

- Later: evolve automatic Plan Mode from the current local heuristic into a
  richer decider. Keep explicit `--plan` as an override, but consider a
  model-backed preflight classifier for language-agnostic task complexity,
  structured reasons, and confidence thresholds before injecting the Plan Mode
  prompt.
- Later: add persistent session storage behind the current in-memory `Session`
  and `SessionManager` APIs. Prefer a small `SessionStore` abstraction with a
  file-backed JSONL implementation so `append_many` can durably append messages
  and `get_or_create` can reload prior history by session id.
- Later: revisit context budgeting when token usage tracking lands. The current
  compactor is character-count based. Keep full session history intact, but
  evolve provider request assembly toward real token telemetry and adaptive
  watermarks.
- Later: consider a more advanced context budgeter once sessions become long
  enough to justify it:
  - Use provider `prompt_tokens` usage to calibrate local estimates and reserve
    output tokens before each request.
  - Build context from newest to oldest under a soft budget instead of always
    compacting the full history copy.
  - Cache compacted forms for unchanged old messages so repeated turns do not
    rescan or reallocate the same large observations.
  - Add memory paging/search for old tool outputs so masked details can be
    pulled back into context when the model explicitly needs them.

## Integrations

- Track Feishu gateway follow-ups in `docs/todo/feishu.md`.
