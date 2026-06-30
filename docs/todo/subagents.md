# Subagent Runtime Notes

## Current Shape

Subagents are runtime-managed agent instances, not normal workspace tools. The
main agent sees `delegate_agent`, `agent_status`, `join_agent`, and
`cancel_agent` as callable actions, but the engine dispatches those commands to
`AgentSupervisor` instead of `ToolRegistry`.

The first built-in template is `explorer`. It starts an independent engine and
session, shares the parent workspace, and receives only the `read_file`, `grep`,
and `load_skill` tools. The parent receives lifecycle observations and the final
report, while subagent events, transcript, and report are persisted under:

```text
.tiny-claw/agents/<agent_id>/
```

This keeps the parent transcript focused on decisions while preserving an audit
trail for the delegated investigation.

## Known Weaknesses

- Each subagent currently uses one `std::thread`. That is simple and adequate
  for low concurrency, but it needs a global concurrency limit or task pool
  before high-volume delegation.
- The supervisor's agent table is in memory. Reports and history survive on
  disk, but `status` and `join` cannot recover a running or completed agent
  after process restart.
- Cancellation is cooperative and only checked around the subagent run. It does
  not interrupt an in-flight provider request or slow tool call.
- Built-in templates, tool profiles, and skill sets are hardcoded. That keeps
  the first version safe, but later custom templates need loading, validation,
  and privilege narrowing rules.
- Reports are Markdown by prompt contract only. There is no structured schema
  validation or section-level check before the parent consumes a report.
- CLI wiring exposes the supervisor, but Feishu mode does not yet attach it.
  Gateway use needs explicit policy decisions around concurrency, permissions,
  and audit visibility.
- There is no audit UI or replay command yet. The raw files are inspectable, but
  reviewing a complex subagent run still requires reading JSONL manually.

## Near-Term Follow-Ups

- Add a runtime-level maximum for active subagents per parent run and per
  workspace.
- Persist enough agent metadata to list previous runs and inspect terminal
  states after restart, even if live joins remain in-memory only.
- Add a small report validator for the existing Markdown sections:
  `Summary`, `Evidence`, and `Uncertainty`.
- Make template definitions data-driven once there are at least two real
  templates, while keeping templates unable to grant themselves broader tools.
- Add an audit command or report view that summarizes events, tool calls, and
  final output for a given `agent_id`.
- Decide whether Feishu should expose subagents. If it does, add gateway-level
  concurrency limits and permission policy before enabling the runtime commands.

## Longer-Term Coordination Models

### Blackboard And Shared Task List

A blackboard design gives multiple agents a shared coordination surface:
facts, open questions, assigned tasks, partial findings, and final decisions.
Agents do not need to message each other directly for every update. Instead,
they publish evidence and claim or complete tasks on the shared board.

This fits broad investigations where the main problem is keeping distributed
work visible and avoiding duplicate effort. It also gives humans a natural audit
surface: the board shows what was known, who added it, and which tasks remain.

### Peer-To-Peer Negotiation

Peer-to-peer negotiation lets agents communicate directly when their scopes
overlap. One agent can ask another for evidence, challenge an assumption,
request a narrower result, or negotiate ownership of a subtask.

This is useful when agents have distinct roles, such as planner, implementer,
reviewer, and tester. It is more expressive than a shared task list, but it also
requires message routing, conversation limits, and clear rules for when the
parent agent must arbitrate.

### Debate And Consensus Voting

Debate asks multiple agents to analyze the same question independently, then
compare arguments. A judge, vote, or consensus rule decides which conclusion is
accepted, or records unresolved disagreement.

This is best suited for high-risk choices, architecture alternatives, test
strategy, or ambiguous bug diagnosis. It is not a default execution model
because it multiplies cost and latency. It should be reserved for places where
independent reasoning and disagreement are more valuable than speed.

## Non-Goals For Now

- Do not add recursive subagent delegation. Current subagents should remain
  unable to spawn sub-subagents.
- Do not replace the runtime supervisor with ordinary tools.
- Do not introduce a heavy multi-agent framework dependency.
- Do not add blackboard, peer negotiation, or debate orchestration until the
  simpler lifecycle, persistence, and audit gaps are addressed.
