# TODO

## Provider

- Add adaptive reasoning mode to replace the fixed `enable_thinking` boolean.
- Add provider retry and backoff policy.
- Consider vendor-specific profiles only after at least two real providers diverge in behavior or configuration.

## Tool Calling

- Later: support a self-correction retry when tool call JSON is invalid.
- Expand tests for multi-turn tool call histories and provider-specific edge cases.

## Runtime

- Add a minimal integration smoke test gated by a real API key.
- Add CLI flags when environment-only configuration becomes limiting.
