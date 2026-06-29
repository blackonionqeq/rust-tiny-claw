# Feishu Tool Approval Middleware

## Purpose

The Feishu gateway needs a registry-level safety middleware that can intercept dangerous tool calls before execution and ask a human reviewer to approve them. Approval should use a Feishu message card with direct allow/reject buttons, and rejection should optionally carry a human-written reason back to the agent.

The feature is only for the Feishu gateway path. The local CLI keeps the existing YOLO behavior unless the project later adds local permission prompts or sandboxing.

## Scope

Implement a minimal but extensible approval path:

- Add middleware support to `ToolRegistry`.
- Add a separate policy layer that classifies tool calls as `allow`, `ask`, or `deny`.
- Use built-in rule definitions for the first version, without scattering hard-coded checks through the registry or Feishu server.
- Send an interactive Feishu card when a tool call requires approval.
- Let the reviewer click `Allow` or `Reject`.
- Include one optional text input on the card for a rejection reason or safer alternative.
- Return the reviewer text to the agent when present; otherwise return a default rejection message.

Out of scope for the first implementation:

- Hot-reloading permission files.
- A public permission configuration schema.
- Multi-step approval workflows.
- Feishu card templates managed in the Feishu console.
- Full multi-session engine factory work.
- Sandboxing.

## Architecture

### Registry Middleware

`ToolRegistry` remains the only execution gateway for tools. It should own a middleware list and run each middleware before calling `Tool::execute`.

The registry does not contain safety rules. It only provides an extension point:

- Unknown tools still return the existing error result.
- Registered tools are looked up before middleware runs, so middleware only evaluates valid calls.
- Middleware can allow the call or return a rejected `ToolResult`.
- Middleware order is insertion order.

This keeps tool implementations such as `BashTool` free of approval logic.

### Policy Layer

Add a small policy module under the tools boundary, for example `src/tools/permission.rs`.

Core concepts:

- `PermissionDecision::Allow`
- `PermissionDecision::Ask { reason }`
- `PermissionDecision::Deny { reason }`
- `ToolPolicy` trait with `decide(&self, call: &ToolCall) -> PermissionDecision`
- `RuleBasedToolPolicy` backed by a list of rules

Rules should describe:

- Target tool name.
- Argument field to inspect, such as `command` for `bash`.
- Match pattern.
- Decision and human-readable reason.

The first rule set should focus on `bash` commands that are dangerous in shared server contexts, such as recursive force deletion, privilege escalation, destructive database commands, and cluster deletion commands. These rules live in one policy module so a later `.tiny-claw/permissions.toml` or YAML loader can replace the source without changing registry or Feishu code.

### Feishu Approval Manager

Add an approval manager inside `src/integrations/feishu/`.

Responsibilities:

- Create a unique `approval_id` for each pending tool call.
- Store a waiting channel or equivalent one-shot responder in a thread-safe map.
- Send an approval card to the chat tied to the current Feishu run.
- Block the current agent run until the card callback resolves the approval.
- Clean up the pending entry after resolution.
- Return a timeout rejection if no one responds within a bounded duration.

The manager should be scoped through `FeishuServerState`, not a process-wide mutable global, so tests can isolate state and later multi-session work has a clear ownership path.

### Feishu Card

Send the approval request using `im/v1/messages` with `msg_type: "interactive"`.

The card should include:

- Title: dangerous tool call approval.
- Tool name.
- Matched reason.
- A compact argument preview with truncation.
- Approval ID for traceability.
- One optional input field named `reject_reason`.
- Two buttons:
  - `Allow`, with value `{ action: "approve_tool_call", approval_id: "..." }`
  - `Reject`, with value `{ action: "reject_tool_call", approval_id: "..." }`

The single input field covers both rejection reason and suggested safer alternative. Keeping it optional reduces friction for reviewers.

### Card Callback

Extend Feishu callback parsing to recognize card action callbacks in addition to text message callbacks.

The parsed callback should include:

- `approval_id`
- action: approve or reject
- operator ID when available
- `reject_reason` from `action.form_value.reject_reason` when present

Server behavior:

- `approve_tool_call`: resolve the pending approval as allowed.
- `reject_tool_call`: resolve as denied.
- For rejection, trim the input value. If it is non-empty, return it to the agent. If it is empty, use the default message:

```text
Human approval rejected this dangerous tool call. Use a safer and auditable approach instead.
```

The callback response should include a toast confirming the action. If practical, return an updated card that marks the approval as approved or rejected, but the first implementation can keep card updates small as long as the reviewer gets clear feedback.

Approval callbacks must be one-shot. Desktop and mobile clients may not observe card updates at the same time, so the server cannot rely on the card UI to prevent duplicate or conflicting clicks. The first callback that successfully resolves an `approval_id` is the only result delivered to the waiting agent run. Later callbacks for the same `approval_id` must not change the stored result, re-send on the waiting channel, or execute any tool-side effect. They should return an idempotent toast such as `This approval has already been handled`.

### Engine Assembly

The Feishu server currently calls `build_engine(&work_dir)` for every incoming message. Introduce a Feishu-specific builder or builder option that accepts the approval manager and chat ID, then attaches the approval middleware.

The middleware flow:

1. Policy returns `Allow`: continue.
2. Policy returns `Deny`: return an error `ToolResult` immediately.
3. Policy returns `Ask`: send a card and wait.
4. Human allows: continue to `Tool::execute`.
5. Human rejects or approval times out: return an error `ToolResult` containing the human reason or default rejection message.

This keeps the CLI builder unchanged.

## Error Handling

- If sending the approval card fails, reject the tool call with an error explaining that approval could not be requested.
- If a card callback references an unknown approval ID, acknowledge the callback with a warning toast and do not start an agent run.
- If the approval wait times out, remove the pending entry and return a rejection result to the agent.
- If two clients or reviewers click the same card, only the first result wins. Later callbacks receive an already handled response and must not mutate the approval result.

## Testing

Unit tests should cover:

- Registry middleware runs before tool execution.
- Middleware rejection prevents tool execution.
- Policy rules classify representative `bash` commands as `allow`, `ask`, or `deny`.
- Feishu callback parsing extracts approve/reject action values and `reject_reason`.
- Approval manager resolves allow and reject paths.
- Empty rejection reason falls back to the default message.
- Duplicate or conflicting callbacks for the same approval ID are idempotent: the first result wins and later callbacks do not overwrite it.

Integration or smoke tests should avoid real Feishu network calls. Use a fake client boundary or direct parser/manager tests for the first version.

## Open Decisions

The initial implementation should choose a conservative timeout. A value around ten minutes is enough for a human approval card without leaving pending calls around indefinitely.

The initial rule list should be intentionally small and visible. Expansion belongs in the policy module, and external configuration can be introduced later without changing the middleware contract.
