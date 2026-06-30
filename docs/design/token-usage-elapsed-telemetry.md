# Token Usage And Elapsed-Time Telemetry

## Purpose

The harness needs low-overhead telemetry for auditability, traceability, cost governance, and latency diagnosis. Operators should be able to answer two basic questions for every model and tool call:

- How many tokens did each model call consume?
- How much wall-clock time did model calls and tool calls take?

The design records this at the provider boundary, where every model request already passes through a narrow interface. Provider adapters parse vendor-specific usage fields. A telemetry wrapper records elapsed time and aggregates usage without pushing observability logic into the ReAct loop.

## Scope

First implementation:

- Add a model-agnostic `Usage` struct to the shared schema.
- Attach `usage: Option<Usage>` to assistant messages returned by providers.
- Parse usage from OpenAI-compatible and Claude-compatible non-stream responses.
- Parse usage from OpenAI-compatible and Claude-compatible stream responses.
- Add a telemetry provider wrapper that records one event per provider call.
- Extend tool middleware with an `after_execute` hook that receives elapsed time.
- Record one telemetry event per executed tool call.
- Keep aggregation in process memory and expose a snapshot for tests and future reporters.

Out of scope for the first implementation:

- Cost calculation using model price tables.
- Persistent metrics files.
- Remote metrics export.
- Token-budget enforcement or adaptive compaction based on usage.
- Full tracing span trees.

## Architecture

### Schema Usage

Add a provider-neutral usage type in `src/schema/mod.rs`:

```rust
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}
```

Extend `Message` with:

```rust
pub usage: Option<Usage>
```

Only provider-produced assistant messages should normally carry usage. System, user, and tool observation messages keep `None`.

This keeps vendor API fields out of the engine and lets session history preserve the exact model response metadata for later reporting.

### Provider Adapter Responsibility

Each concrete provider owns vendor-specific parsing:

- OpenAI-compatible non-stream responses map top-level `usage.prompt_tokens`, `usage.completion_tokens`, and `usage.total_tokens`.
- Claude-compatible non-stream responses map `usage.input_tokens` to `prompt_tokens` and `usage.output_tokens` to `completion_tokens`; `total_tokens` is their sum when no explicit total exists.
- OpenAI-compatible streams should request usage with `stream_options: { "include_usage": true }` and capture the final usage-bearing chunk.
- Claude-compatible streams should aggregate usage from stream events that expose input/output token counts.

Provider adapters should only parse and attach usage. They should not log, calculate cost, mutate global state, or know about session totals.

### Telemetry Provider Wrapper

Add a wrapper in `src/telemetry/` that implements the same `Provider` trait:

```rust
pub struct TelemetryProvider<P> {
    inner: P,
    telemetry: Telemetry,
}
```

Both `generate` and `generate_stream` follow the same flow:

1. Capture `Instant::now()`.
2. Call the inner provider.
3. Calculate elapsed time.
4. Record success or failure, including `message.usage` when present.
5. Return the original result unchanged.

This wrapper should be installed in `app::build_provider()` after selecting the real provider, so CLI, Feishu, and subagent provider factories all receive the same behavior. The engine remains unaware of telemetry.

### Telemetry Data Model

`Telemetry` should provide low-level records and totals:

```rust
pub struct LlmCallRecord {
    pub provider: &'static str,
    pub model: Option<String>,
    pub stream: bool,
    pub elapsed_ms: u128,
    pub usage: Option<Usage>,
    pub success: bool,
    pub error: Option<String>,
}

pub struct LlmTotals {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub call_count: u64,
    pub failed_call_count: u64,
    pub elapsed_ms: u128,
}

pub struct ToolTotals {
    pub call_count: u64,
    pub failed_call_count: u64,
    pub elapsed_ms: u128,
}

pub struct TelemetrySnapshot {
    pub llm: LlmTotals,
    pub tools: ToolTotals,
}

pub struct ToolCallRecord {
    pub tool_name: String,
    pub tool_call_id: String,
    pub access_mode: ToolAccessMode,
    pub elapsed_ms: u128,
    pub success: bool,
}
```

The first version should expose:

- `record_llm_call(record)`
- `record_tool_call(record)`
- `snapshot() -> TelemetrySnapshot`
- `name() -> &'static str`

Detailed per-call history can be optional and bounded. Tests need enough visibility to assert that records are emitted, but the runtime should not retain unbounded logs by default.

### Tool Execution Timing

Tool elapsed time should be measured at the registry boundary. `ToolRegistry::execute()` is the only common path for local tool execution, including sequential calls and the individual calls inside parallel batches.

Extend the middleware trait without breaking existing approval middleware:

```rust
pub struct ToolExecutionContext {
    pub elapsed: Duration,
    pub access_mode: ToolAccessMode,
}

pub trait ToolMiddleware: Send + Sync {
    fn before_execute(&self, call: &ToolCall) -> Option<ToolResult> {
        None
    }

    fn after_execute(
        &self,
        call: &ToolCall,
        result: &ToolResult,
        context: &ToolExecutionContext,
    ) {
    }
}
```

`ToolRegistry::execute()` should own the timing flow:

1. Look up the registered tool.
2. Run `before_execute` middleware in insertion order.
3. If a middleware rejects the call, return the rejection without recording tool execution elapsed time.
4. Capture `Instant::now()`.
5. Execute the tool.
6. Calculate elapsed time.
7. Build `ToolExecutionContext` with elapsed duration and access mode.
8. Run `after_execute` middleware in insertion order with the call, result, and execution context.
9. Return the original tool result.

Telemetry can then be attached as a normal middleware:

```rust
pub struct TelemetryToolMiddleware {
    telemetry: Telemetry,
}
```

The middleware records:

- tool name
- tool call ID
- elapsed milliseconds
- success flag based on `ToolResult::is_error`
- access mode from `ToolExecutionContext`

Parallel batches do not need a special aggregate timer for the first version. Each tool call is timed independently in the thread that executes it. The overall turn elapsed time can be derived later from higher-level tracing if needed.

## Stream Handling

Stream mode must not estimate usage from text length. It should use provider-reported usage metadata.

OpenAI-compatible stream support:

- Add `stream_options` to the request only when `stream = true`.
- Extend `ChatCompletionStreamChunk` with optional `usage`.
- Allow chunks with empty `choices`, because usage-only chunks may arrive at the end.
- Store usage in `OpenAiStreamState`.
- Move the stored usage into the final `Message`.

Claude-compatible stream support:

- Extend stream event structs for usage-bearing events.
- Store the latest or accumulated input/output token counts in `ClaudeStreamState`.
- Move the final usage into the final `Message`.

If a provider endpoint does not return stream usage, the message should still complete normally with `usage = None`; telemetry should record elapsed time and mark usage as unavailable.

## Performance Design

Telemetry must stay off the critical path as much as practical.

The first implementation should be synchronous and in-process. It does only a few cheap operations per provider call:

- one `Instant::now()`
- one elapsed calculation
- copying a small `Usage` struct
- one aggregate update after the provider call completes

That overhead is negligible compared with a network model call or SSE stream. It also keeps the implementation simple and deterministic.

Tool timing has the same shape: one timestamp before `Tool::execute`, one elapsed calculation after it returns, and one telemetry update. This is negligible for shell commands and file operations, and it preserves the existing tool implementations.

For shared aggregation, prefer one of these low-contention shapes:

- Atomic counters for totals: `AtomicU64` for token and call counts, plus a small helper for elapsed milliseconds.
- A `Mutex<VecDeque<...>>` only for an optional bounded recent-event buffer containing compact LLM and tool records.

Do not lock around the provider call or stream read. The wrapper should collect local variables on the stack, then update telemetry once at the end. That avoids serializing concurrent Feishu sessions or subagent calls.

Likewise, do not hold telemetry locks while a tool runs. Tool execution may call the filesystem or spawn processes. Telemetry should update only after the result is available.

### Async Export

Do not introduce async export in the first version. The project currently uses blocking providers, and adding a runtime or channel just for counters would add complexity without improving the hot path.

If later metrics must be written to disk or sent to a remote backend, add a background worker:

- provider wrapper sends compact records over a bounded channel with `try_send`
- if the channel is full, drop detailed records but still update in-memory counters
- worker batches writes or remote exports
- shutdown flush is best-effort

The provider call must never wait for metrics IO.

### Subprocess Export

Do not use a subprocess for first-party telemetry. A child process adds spawn cost, lifecycle failure modes, log routing problems, and cross-platform complexity. It is only justified if metrics are delegated to an existing external collector. Even then, the harness should communicate through a non-blocking buffered boundary and keep local counters independent of collector health.

## Error Handling

- If a provider call fails, telemetry records elapsed time, provider name, stream flag, `success = false`, and a short error string.
- Failed calls should not invent token usage.
- If a provider response omits usage, telemetry records elapsed time with `usage = None`.
- If parsing usage fails because the usage field has an unexpected shape, treat that as a provider response parse error only when the rest of the response cannot be trusted. Optional missing usage should not fail the run.
- Stream parsing should continue to reject malformed content or tool-call chunks as it does today.
- If tool execution returns an error `ToolResult`, telemetry still records elapsed time with `success = false`.
- If a `before_execute` middleware blocks a call, that is a policy decision rather than executed tool work. It should not be counted as tool execution time.

## Testing

Unit tests should cover:

- `Message` constructors set `usage = None` by default.
- OpenAI-compatible non-stream response maps usage into the final message.
- OpenAI-compatible stream state captures usage-only final chunks.
- Claude-compatible non-stream response maps input/output tokens.
- Claude-compatible stream state captures usage events.
- Telemetry wrapper records success, elapsed time, stream flag, and usage.
- Telemetry wrapper records failed provider calls without usage.
- `ToolRegistry::execute()` calls `after_execute` after a successful tool invocation.
- `ToolRegistry::execute()` calls `after_execute` after a tool returns an error result.
- `ToolRegistry::execute()` does not call `after_execute` when `before_execute` rejects the call.
- Parallel read-only tool batches emit one tool timing event per call.
- Concurrent recording updates totals without panics or lost counts.

The implementation should keep network-free tests by constructing response structs and stream states directly.

## Future Work

Cost calculation can be added later as a pure function over `Usage`, model name, and a small price table. It should remain separate from provider parsing so model pricing changes do not affect adapter correctness.

Full turn-level tracing can later connect model calls, tool calls, and subagent calls into a span tree. This design deliberately records the raw timing events first so tracing can build on reliable measurements instead of changing provider or tool adapters again.
