# Agent Tracing And OTLP Export

## Purpose

Token and elapsed-time telemetry answers how much the harness spent and how long broad classes of work took. Tracing should answer a different question: how one agent run moved through the ReAct loop, where the run diverged, which prompt context was sent, which LLM call produced which tool calls, and which parallel tool execution became the bottleneck.

The trace model should be useful locally as a JSON decision tree and externally as OpenTelemetry trace data that can be viewed in Jaeger, Zipkin, Tempo, or another backend through an OpenTelemetry Collector.

## Scope

First tracing implementation:

- Add an internal span model aligned with OpenTelemetry trace concepts.
- Add explicit `TraceContext` propagation through the engine and tool registry.
- Record root run spans, turn spans, context compaction spans, LLM spans, and tool spans.
- Preserve existing `Telemetry` counters for cheap aggregate totals.
- Export local JSON traces for local audit, incident review, and deterministic debugging.
- Export OTLP trace batches when an endpoint is configured.
- Keep trace export asynchronous, bounded, and best-effort so observability cannot block the agent loop.

Out of scope for the first tracing implementation:

- Token-level stream events.
- Full prompt and full tool output capture by default.
- Cost calculation or budget enforcement.
- Jaeger-specific direct protocol integration.
- Runtime-wide async conversion. The current blocking engine can use a background worker without changing provider and tool traits to async.

## Architecture

### Module Layout

Keep tracing inside the existing observability area:

```text
src/telemetry/
  mod.rs              # existing aggregate telemetry
  trace.rs            # span model, TraceContext, TraceRecorder
  exporter.rs         # TraceExporter trait and exporter configuration
  json_exporter.rs    # local trace file exporter
  otlp_exporter.rs    # OTLP exporter
```

`Telemetry` remains the home for aggregate counters. `TraceRecorder` owns detailed span records and export delivery. This keeps "metrics totals" and "decision path replay" separate while still allowing LLM/tool instrumentation to update both from the same boundary.

### Internal Span Model

The internal span type should be close to OpenTelemetry so the exporter is a mapping layer instead of a redesign:

```rust
pub struct TraceSpanRecord {
    pub trace_id: TraceId,
    pub span_id: SpanId,
    pub parent_span_id: Option<SpanId>,
    pub name: String,
    pub start_time_unix_nano: u128,
    pub end_time_unix_nano: u128,
    pub attributes: Vec<TraceAttribute>,
    pub events: Vec<TraceEvent>,
    pub status: TraceStatus,
}
```

Attributes should support the OTLP scalar value shapes the harness needs:

- string
- bool
- i64
- f64

JSON values can be accepted at the API boundary, but the recorder should normalize them into scalar attributes or compact string previews before storage. This prevents exporter-specific behavior from leaking into engine code.

### Trace Context

Rust should not copy Go's implicit `context.Context` pattern. Use explicit context values so parent-child relationships are obvious at call sites and testable without thread-local state:

```rust
pub struct TraceContext {
    recorder: TraceRecorder,
    trace_id: TraceId,
    current_span_id: SpanId,
}

pub struct SpanGuard {
    recorder: TraceRecorder,
    span_id: SpanId,
}
```

`TraceContext::start_child(name)` creates a child span under `current_span_id` and returns a guard. The guard records end time on `end()` or `Drop`. For scopes that need to pass the child as a parent to nested work, the guard exposes `context() -> TraceContext`.

The engine creates one root context per `run_session_with_reporter()` call:

```text
Agent.Run
  Agent.Turn
    Context.Compaction
    LLM.Thinking
    LLM.Action
    Tool.Execute
```

Parallel tool calls receive cloned child contexts for the same turn span. Each tool span then shares the turn parent but records independent start and end times, which gives OTLP backends enough information to render the overlap as a timeline.

### Instrumentation Boundaries

Trace at stable module boundaries rather than inside every implementation detail.

Engine instrumentation:

- `Agent.Run`: session ID, workspace, options, provider name.
- `Agent.Turn`: turn index, plan mode, stream mode, thinking enabled.
- `Context.Compaction`: input message count, output message count, configured budget.
- `LLM.Thinking`: provider name, stream flag, success flag, usage totals when available.
- `LLM.Action`: provider name, stream flag, available tool count, emitted tool call count, usage totals when available.

Tool registry instrumentation:

- `Tool.Execute`: tool name, tool call ID, access mode, success flag, argument preview, output preview.
- Policy rejections should be recorded as `Tool.Policy` or a `Tool.Execute` span with `tool.blocked = true`, depending on the final middleware shape. The important rule is that blocked calls must be distinguishable from tools that actually ran and failed.

Subagent instrumentation can be added later at the supervisor boundary:

- `Subagent.Run`
- `Subagent.Command`
- `Subagent.Provider`

### Sensitive And Large Data

Tracing must help debug failures without silently creating a prompt and output archive.

Default capture:

- message counts
- token usage
- model/provider names
- tool names and call IDs
- compact argument preview
- compact output preview
- content hashes for large prompt/tool data

Do not store full system prompts, full user prompts, full tool outputs, or environment variables by default. A future debug mode may allow explicit full-content capture, but it should be opt-in and clearly named.

## Export Design

### Exporter Trait

Use a small exporter trait behind the recorder:

```rust
pub trait TraceExporter: Send + Sync {
    fn export(&self, batch: &[TraceSpanRecord]) -> Result<(), TraceExportError>;
    fn shutdown(&self) -> Result<(), TraceExportError> {
        Ok(())
    }
}
```

The recorder should not know whether a batch becomes JSON, OTLP HTTP, or both. Multiple exporters can be composed by a fan-out exporter when needed.

### JSON Export

The JSON exporter writes trace records under:

```text
.tiny-claw/traces/
```

Use a tree-shaped JSON view for humans and tests, ordered by start time under each parent. This format is for local replay, audit review, and deterministic debugging. It is not the canonical wire model.

### OTLP Export

OTLP export should target the OpenTelemetry Collector instead of Jaeger directly:

```text
rust-tiny-claw -> OTLP HTTP /v1/traces -> OTel Collector -> Jaeger / Tempo / Zipkin
```

The official Rust OpenTelemetry stack supports an OTLP exporter path where application spans are collected by the SDK, serialized, and sent to a collector. The HTTP span exporter uses the `/v1/traces` signal path when a generic OTLP endpoint is configured. Newer OpenTelemetry Rust SDK versions require keeping the tracer provider and calling `shutdown()` explicitly on exit.

There are two viable implementation paths:

- Direct SDK path: create real OpenTelemetry spans through `opentelemetry` and `opentelemetry-otlp`, then let the SDK batch processor export them.
- Internal-model path: keep `TraceSpanRecord` as the source of truth and map records to OTLP payloads in `OtlpTraceExporter`.

Use the internal-model path first. It keeps runtime instrumentation explicit, avoids threading SDK span handles through engine logic, and preserves local JSON export. The exporter can still use OpenTelemetry crates for OTLP data structures and transport if that is lighter than hand-writing protocol payloads.

## Performance Design

### Synchronous Collection

Span collection on the hot path should do only cheap work:

- allocate IDs
- read current time
- store a small set of normalized attributes
- enqueue the completed span with `try_send`

It must not serialize JSON, perform network IO, compress payloads, or wait for an exporter while the model or tool path is running.

### Asynchronous Export

Export runs in a background worker:

```text
SpanGuard end
  -> TraceRecorder::finish_span(record)
  -> bounded_queue.try_send(record)
  -> return to engine/tool path

background worker
  -> collect up to N spans or wait up to T milliseconds
  -> export batch to JSON and/or OTLP
  -> retry once or drop according to policy
```

Recommended defaults:

- bounded queue capacity: 1024 spans
- batch size: 64 spans
- flush interval: 2 seconds
- shutdown flush timeout: 1 second

If the queue is full, drop detailed span records and increment a dropped-span counter. Never block the agent run waiting for trace export capacity. Existing aggregate `Telemetry` counters should still be updated independently, so high-level totals survive trace backpressure.

### Export Modes

Use environment-controlled modes:

```text
TINY_CLAW_TRACE=off
TINY_CLAW_TRACE=json
TINY_CLAW_TRACE=otlp
TINY_CLAW_TRACE=both
TINY_CLAW_TRACE=debug
```

`debug` may force a synchronous flush at the end of each run to make tests and local debugging deterministic. Normal `json`, `otlp`, and `both` modes should remain asynchronous and best-effort.

OTLP endpoint configuration:

```text
TINY_CLAW_OTLP_ENDPOINT=http://localhost:4318
```

The exporter should append `/v1/traces` when the configured endpoint is the generic collector base URL. If a future setting accepts a traces-specific endpoint, document that it must include the full path.

## Error Handling

- Provider errors mark the LLM span status as error and record a short error message.
- Tool `ToolResult::is_error` marks the tool span status as error.
- Export failures are recorded as exporter diagnostics, not agent errors.
- Queue overflow increments a dropped-span count and continues.
- Shutdown flush is best-effort with a timeout.
- JSON write failures and OTLP failures must never change the engine result.

## Testing

Unit tests should cover:

- root span creation with stable trace ID and no parent.
- child span creation with the expected parent span ID.
- guard drop records end time and duration.
- parallel child spans can be recorded under one parent without panics.
- engine run creates root, turn, LLM, and tool span names in expected order.
- provider failure marks the LLM span as error.
- tool error marks the tool span as error.
- queue overflow drops spans without blocking.
- JSON exporter builds a parent-child tree from flat records.
- OTLP exporter maps trace ID, span ID, parent ID, timestamps, attributes, and status.

Network-free tests should use fake exporters that collect batches in memory. OTLP wire-level tests should construct payloads locally and avoid requiring a running collector.

## Relationship To Existing Telemetry

Existing `Telemetry` remains responsible for cheap process-local totals:

- token totals
- call counts
- failed call counts
- elapsed-time totals

Tracing adds detailed per-run structure:

- operation nesting
- parent-child relationships
- timing overlap for parallel tools
- selected metadata for replay, audit, and debugging

The two systems can be updated from the same instrumentation boundaries, but they should not share storage. Aggregate metrics must stay available even when trace export is disabled or overloaded.

## Future Work

- Add trace IDs to terminal and Feishu reporter messages so operators can correlate chat output with trace files.
- Add sampling once long-running Feishu deployments need lower trace volume.
- Add semantic conventions for LLM-specific attributes if the OpenTelemetry community stabilizes them.
- Add a simple local trace viewer after the JSON format is stable.
- Add subagent and approval spans when those runtime paths require deeper observability.
