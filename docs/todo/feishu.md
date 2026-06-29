# Feishu TODO

## Current Scope

The Feishu gateway is still an optional entrypoint around the local harness. The
current runtime may start one agent run per incoming text message. It now keeps a
small in-process dedup cache keyed by Feishu message id, but that cache is lost
when the process restarts.

## Queue

- Add a workspace-level task queue before allowing unbounded concurrent agent
  runs from Feishu chats.
- Preserve Feishu callback latency by acknowledging events quickly, then
  enqueueing the agent work.
- Record task status transitions so later Feishu replies can say queued,
  running, succeeded, failed, or cancelled.
- Keep the first queue local and in memory unless a deployment needs restart
  recovery.

## Concurrency

- Add a configurable maximum number of active Feishu agent runs.
- Use workspace-level serialization first, because the current tools can mutate
  files inside one shared workspace.
- Consider per-chat fairness once multiple chats can submit work at the same
  time.
- Keep engine-level read-only tool parallelism separate from Feishu request
  concurrency.

## Permissions

- Later: move tool approval rules into a runtime-editable configuration file so
  deployments can change command behavior without rebuilding the binary.
- Keep the current `allow` / `ask` / `deny` policy shape, but load the rule
  source from configuration once the built-in defaults become limiting.
- Consider safe reload semantics so an in-flight approval keeps the policy that
  created it while new tool calls see the updated rules.

## Cancellation

- Add a Feishu command shape for cancelling queued or running work, such as
  plain-text `/cancel`.
- Teach the runtime to cancel queued tasks immediately.
- For running tasks, add cooperative cancellation points around provider calls
  and tool dispatch before attempting hard process termination.
- Report cancellation back to the originating chat.

## Deduplication

- Persist processed message ids if duplicate execution across process restarts
  becomes a real deployment issue.
- Keep a TTL on dedup entries so storage cannot grow without bound.
- Include unsupported message replies in dedup so Feishu retries do not spam the
  chat.
