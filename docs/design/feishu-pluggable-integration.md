# Feishu Pluggable Integration Design

## Background

`rust-tiny-claw` should be able to receive commands from Feishu and report agent
progress back to the originating chat. Feishu provides official SDKs for some
languages, but not for Rust. A non-official Rust SDK exists, and a Go/Node
sidecar could use an official SDK, but either choice would add more surface area
than this project currently needs.

The project should treat Feishu as an optional interaction entrypoint, not as a
core engine dependency. The first implementation should own only the smallest
OpenAPI subset required for the integration.

## Goals

- Keep CLI mode as the default runtime.
- Make Feishu integration opt-in at compile time.
- Avoid linking Feishu HTTP server and crypto dependencies into the default
  binary.
- Keep the agent engine independent from Feishu event JSON, OpenAPI payloads,
  and message formatting.
- Preserve a narrow path for later ChatOps features: cards, approvals, event
  deduplication, workspace locking, task queues, and session memory.
- Implement a minimal Feishu OpenAPI client instead of a complete SDK.

## Non-Goals

- Do not implement a complete Feishu SDK.
- Do not add a Go/Node sidecar for the first Rust version.
- Do not enable Feishu by environment variable in the default CLI binary.
- Do not implement approval cards, persistent chat sessions, encrypted event
  bodies, or global task scheduling in the first integration pass.
- Do not move the engine to async only for Feishu. Keep async boundaries inside
  the integration entrypoint unless the engine later needs a broader runtime
  redesign.

## Recommended Architecture

The engine should report lifecycle events through a small `Reporter` trait. The
engine knows only that it can announce thinking, tool calls, tool results, and
assistant messages. It should not know whether those events are printed to a
terminal, sent to Feishu, written to a trace sink, or forwarded to another UI.

Expected component split:

```text
src/
  engine/
    reporter.rs        # Reporter trait
  reporter/
    terminal.rs        # default CLI reporter
  integrations/
    feishu/            # compiled only with feature = "feishu"
      config.rs        # env/config parsing
      token.rs         # tenant access token cache
      event.rs         # Feishu event parsing into internal input
      client.rs        # minimal OpenAPI calls
      reporter.rs      # FeishuReporter implementation
      server.rs        # HTTP callback endpoint
  bin/
    tiny-claw.rs
    tiny-claw-feishu.rs
```

The first Feishu implementation should directly call the Feishu OpenAPI over
HTTP instead of wrapping a full SDK abstraction. It only needs the API calls and
event types required for a basic chat command loop:

- URL verification challenge.
- Message receive event.
- Send text message back to a chat.
- Tenant access token retrieval and caching.

## Compile-Time Plug-in Strategy

Use a Cargo feature and a separate binary:

```toml
[features]
default = []
feishu = ["dep:tokio", "dep:axum", "dep:hmac", "dep:sha2", "dep:base64"]

[[bin]]
name = "tiny-claw"
path = "src/bin/tiny-claw.rs"

[[bin]]
name = "tiny-claw-feishu"
path = "src/bin/tiny-claw-feishu.rs"
required-features = ["feishu"]
```

The default command remains CLI-focused:

```bash
cargo run --bin tiny-claw
```

Feishu mode is explicit:

```bash
cargo run --features feishu --bin tiny-claw-feishu
```

This keeps default builds small and prevents optional Feishu dependencies from
being compiled or linked unless the user asks for them.

## Environment Configuration

Use separate environment files for common runtime configuration and optional
Feishu integration configuration:

```text
.env.example          # committed common runtime template
.env.feishu.example   # committed Feishu template
.env                  # local common runtime values, ignored by git
.env.feishu           # local Feishu values, ignored by git
```

The default CLI binary should load only `.env`. The Feishu binary should load
`.env` first, then `.env.feishu`. This keeps provider/runtime configuration
shared while keeping optional Feishu credentials out of the default CLI path.

Common variables belong in `.env.example`:

```env
TINY_CLAW_PROVIDER=mock
TINY_CLAW_STREAM=true
TINY_CLAW_API_KEY=
TINY_CLAW_BASE_URL=
TINY_CLAW_MODEL=
```

Feishu-specific variables belong in `.env.feishu.example`:

```env
FEISHU_APP_ID=
FEISHU_APP_SECRET=
FEISHU_VERIFY_TOKEN=
FEISHU_ENCRYPT_KEY=
FEISHU_CALLBACK_HOST=0.0.0.0
FEISHU_CALLBACK_PORT=48080
```

This follows the same plug-in boundary as the compile-time feature: optional
integrations bring their own config file and are only loaded by their own
entrypoint. Future integrations can follow the same pattern, such as
`.env.slack.example` or `.env.dingtalk.example`.

## Data Flow

```text
Feishu callback
  -> verify/challenge handling
  -> parse raw event JSON
  -> normalize to IncomingMessage
  -> create FeishuReporter for the chat
  -> run AgentEngine with the message text
  -> Reporter events call Feishu send-message OpenAPI
```

The normalized input should be small and platform-neutral:

```rust
struct IncomingMessage {
    chat_id: String,
    message_id: String,
    sender_id: String,
    text: String,
}
```

The engine should accept a reporter from the caller. This lets CLI mode pass a
terminal reporter and Feishu mode pass a chat-scoped reporter without branching
inside the engine.

## Integration Tradeoffs

### Direct OpenAPI Calls

This is the recommended first implementation.

Pros:

- Small dependency surface.
- Easy to audit and teach.
- Matches the current minimal integration scope.
- Keeps Feishu-specific details inside `integrations/feishu`.

Cons:

- The project must own token caching, event parsing, and request payloads.
- More platform details are visible than when using a full SDK.

### Non-Official Rust SDK

Pros:

- May provide typed clients and event helpers.
- Could speed up broader API coverage later.

Cons:

- Not the official Feishu SDK.
- Adds a dependency whose abstractions may not match this project's integration
  boundary.
- May hide protocol details that are useful to understand while building the
  first small integration.

This remains a possible later swap if the integration grows beyond a few
OpenAPI endpoints.

### Go or Node Sidecar

Pros:

- Can use an official Feishu SDK.
- Keeps Feishu protocol details out of Rust.

Cons:

- Introduces a multi-process deployment model too early.
- Requires another local API contract between the sidecar and Rust engine.
- Makes the learning project harder to run and reason about.

This should be reserved for production needs, not the first in-repo
implementation.

## Error Handling

- Callback endpoints should return quickly and not block on long agent runs.
- Event parsing failures should be logged and acknowledged with a non-panicking
  HTTP response when possible.
- Message send failures should be surfaced through logs first; retry policy can
  be added later.
- Token fetch failures should fail the Feishu mode startup or the first send
  operation with a clear error.
- The first pass can support verify token and URL challenge. Encrypted callback
  bodies can be added after the plain event path works.

## Concurrency And Safety

The first Feishu version may spawn one task per incoming message. Before
enabling this in a shared workspace, add one of these guards:

- A workspace mutex that serializes mutating agent runs.
- A task queue keyed by workspace path.
- A scheduler that allows read-only tasks in parallel and serializes tasks that
  may write files or run high-risk commands.

This is intentionally separate from the initial Feishu adapter so the adapter
does not become responsible for engine-level execution policy.

## Testing Plan

- Unit-test event challenge parsing.
- Unit-test message event normalization into `IncomingMessage`.
- Unit-test token cache expiration behavior with mocked time or short TTLs.
- Unit-test `FeishuReporter` payload construction without making network calls.
- Add an integration smoke test for the Feishu callback handler with a local
  HTTP request once the server module exists.
- Keep default `cargo check` free of Feishu dependencies; also verify
  `cargo check --features feishu` before finishing Feishu implementation work.

## Open Extension Points

- Rich Feishu cards for tool calls and approval prompts.
- Event deduplication by event ID or message ID.
- Per-chat or per-thread session memory.
- Workspace-level task queue.
- Encrypted callback body support.
- A future SDK-backed client if direct OpenAPI calls become too broad.
