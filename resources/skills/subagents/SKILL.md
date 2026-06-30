---
name: Subagents
description: Delegation rules and runtime commands for isolated subagent exploration.
---

# Subagents

Use subagents for bounded investigation that would otherwise spend many turns in
the main context. Keep final decisions, edits, approvals, and small known reads
in the main agent.

## Templates

- `explorer`: read-only repository exploration. It can use `read_file`, `grep`,
  and `load_skill`. It cannot edit files or run shell commands.

## Commands

- `delegate_agent`: start a subagent. Required arguments are `template_id` and
  `task`. Optional `role_prompt_append` may narrow the investigation but cannot
  grant tools or skills.
- `agent_status`: check whether an agent is `running`, `completed`, `failed`, or
  `cancelled`.
- `join_agent`: wait for completion and return the final report.
- `cancel_agent`: request cooperative cancellation.

## Task Shape

Write delegated tasks as specific investigations with the expected evidence. A
good task names the area to inspect, the question to answer, and the kind of
paths, symbols, or docs that should appear in the report.

Example:

```text
template_id: explorer
task: Inspect how tool access modes are represented. Report the relevant files,
traits, and tests. Do not propose code changes.
```

Subagent reports use Markdown:

```markdown
## Summary

## Evidence

## Uncertainty
```
