# Subagent Runtime and Templates Design

## Goal

Add subagent support as a runtime-level capability instead of a normal tool.
The first version should let the main agent delegate isolated exploration work
to a reusable subagent template, while keeping the design open for later
non-blocking agents, agent-to-agent messaging, and custom subagent
configuration.

The immediate target is context isolation: the main agent keeps its transcript
focused on planning and final decisions, while an explorer subagent can spend
many turns reading files, grepping, and summarizing evidence in its own isolated
history.

## Non-Goals

- Do not implement subagents as `ToolRegistry` tools.
- Do not add a heavy multi-agent framework dependency.
- Do not implement peer-to-peer negotiation, shared task boards, or voting
  teams in the first version.
- Do not add a hard `max_turns` field to the initial subagent template model.
  Runtime cancellation, timeouts, cost budgets, or token budgets can be designed
  later as separate control-plane limits.
- Do not let the model freely grant mutating tools or arbitrary skills to a
  subagent.

## Current Constraints

`AgentEngine` currently owns a synchronous ReAct loop with a provider, tool
registry, context manager, file memory, and telemetry. The tool layer is also
synchronous: a `Tool::execute` call returns one `ToolResult`.

That shape is useful for workspace actions, but it is the wrong boundary for a
subagent. A subagent has lifecycle, state, output, and future communication
needs. If it is implemented as a tool, the tool layer must construct or hold an
engine, the main loop blocks until the child finishes, and later operations such
as status, cancellation, events, or messaging have no natural place to live.

## Architecture

Introduce an `agent_runtime` layer above individual engine runs:

```text
src/agent_runtime/
  mod.rs
  supervisor.rs
  spec.rs
  templates.rs
  profiles.rs
  handle.rs
  events.rs
```

The boundaries should be:

- `engine`: run one agent loop from a resolved `AgentSpec`.
- `agent_runtime`: spawn, track, join, cancel, and report agent instances.
- `tools`: expose workspace capabilities only; tools do not know about agents.
- `context_engine`: compose role-specific system prompts and enabled skill
  catalogs.
- `memory`: store independent transcripts and reports for each agent.
- `reporter`: display agent-scoped events with agent id and role.

The runtime should own an `AgentSupervisor`:

```rust
struct AgentSupervisor {
    provider_factory: Arc<dyn ProviderFactory>,
    templates: SubagentTemplateRegistry,
    tool_profiles: ToolProfileRegistry,
    skill_sets: SkillSetRegistry,
    memory: FileMemory,
    telemetry: Telemetry,
}
```

Each spawned subagent returns an `AgentHandle`:

```rust
struct AgentHandle {
    id: AgentId,
}
```

The handle is the stable reference for later status, join, cancel, event, or
message operations.

## Provider Creation

Concurrent agents should not share one mutable provider instance. Add a provider
factory abstraction before or alongside the first runtime implementation:

```rust
trait ProviderFactory: Send + Sync {
    fn create(&self) -> Result<Box<dyn Provider + Send>, ProviderError>;
}
```

Each agent run receives its own provider instance. This keeps concurrency,
provider-local state, cancellation, and later rate limiting easier to reason
about.

## Runtime Commands

The first runtime should support a small control surface:

```text
delegate_agent(template, task, overrides?) -> agent_id
agent_status(agent_id) -> running | completed | failed | cancelled
join_agent(agent_id) -> final report or failure
cancel_agent(agent_id) -> cancellation result
```

These commands may be exposed to the model using structured tool-call schemas,
because current provider APIs already support tool/function calling. However,
they should not be registered in `ToolRegistry` and should not implement
`Tool::execute`. The engine should classify assistant actions into runtime
commands and normal tool calls, then dispatch each to the correct layer.

This gives the model a familiar structured action interface while keeping the
implementation boundary clean.

Internally, keep action definitions typed:

```rust
enum ActionDefinition {
    Tool(ToolDefinition),
    RuntimeCommand(RuntimeCommandDefinition),
}

enum AssistantAction {
    ToolCall(ToolCall),
    RuntimeCommand(RuntimeCommandCall),
}
```

Provider adapters may map both variants to vendor tool/function schemas when
making requests, but the engine should preserve the distinction after parsing
the response. This prevents runtime commands from becoming accidental workspace
tools and leaves room for later non-tool provider protocols.

## Prompt Exposure and Skill Loading

The main system prompt should not contain the full subagent operating manual.
It should only expose a compact delegation hint and point the model to a
project-provided system skill for detailed guidance.

Initial system prompt section:

```markdown
# Subagent Delegation

You may delegate bounded investigation work to subagents when broad exploration
would pollute the main context. Use subagents for multi-file or uncertain
exploration that can be summarized as evidence. Do not use subagents for small
known reads, final decisions, or workspace mutations.

When you need detailed subagent templates, delegation rules, waiting behavior,
or examples, load the `subagents` skill.
```

The detailed guidance should live in a project skill, for example:

```text
.tiny-claw/skills/subagents/SKILL.md
```

That skill should describe:

- When to delegate and when to stay in the main agent.
- Available subagent templates and their capability boundaries.
- How to write a useful delegated task.
- How `delegate_agent`, `join_agent`, `agent_status`, and `cancel_agent` work.
- How blocking and non-blocking delegation differ.
- Which overrides are available, if any.
- Example delegation requests and expected reports.

This follows the existing progressive skill-loading design: the initial prompt
keeps only the catalog and a short routing hint, while detailed operational
instructions are loaded only when relevant. It also lets template documentation
evolve without growing the base system prompt.

Subagents may use `load_skill`, but only within the resolved `AgentSpec.skills`
allowlist. This keeps progressive loading available inside a subagent while
preserving the template's skill boundary.

## Waiting and Wakeup

Subagents should be spawned as runtime tasks, but the main agent still needs an
explicit synchronization point when its next decision depends on the subagent
result.

Prefer this split:

```text
delegate_agent(...) -> agent_id
join_agent(agent_id) -> wait for final report
```

This keeps delegation non-blocking by default while still supporting blocking
behavior when it is intentional. If the main agent strongly depends on the
subagent result, it can call `join_agent` immediately after `delegate_agent`.
If it wants parallel exploration, it can delegate several agents first and join
them later.

A later convenience field can be added:

```text
delegate_agent(template, task, wait_for_completion=true)
```

That field should desugar internally to `delegate_agent` followed by
`join_agent`; it should not change the underlying lifecycle model.

`join_agent` is the first wakeup mechanism: if the subagent is still running,
the parent run waits at that synchronization point until the child completes,
fails, or is cancelled. The supervisor should wake the blocked join when it
receives the child's completion event.

Future event-driven wakeup can be added with parent subscriptions:

```text
delegate_agent(template, task, notify_parent_on_completion=true)
```

In that mode, the supervisor records an `AgentCompleted` event, appends a compact
message to the parent session, and schedules or offers a parent continuation.
This should be designed carefully because autonomous parent resumption affects
ordering, user visibility, approvals, and cost control. It is not required for
the first version.

## Subagent Templates

A subagent template is a reusable, human-authored role and capability package.
The main agent should choose a template and provide a task, instead of
reconstructing tools, skills, prompts, and output rules every time.

Example template:

```toml
id = "explorer"
name = "Explorer Subagent"
description = "Read-only repository exploration with evidence-backed summary."

role_prompt = """
You are an explorer subagent. Investigate the requested topic using the
available read-only tools. Return a concise report with concrete evidence,
including file paths and relevant symbols when possible.
"""

tool_profile = "read_only"
skill_set = "rust_explorer"
output_contract = "exploration_report"

[overrides]
allow_role_prompt_append = true
allow_extra_skills = false
allow_tool_profile = false
allow_output_contract = false
allow_context_budget = false
```

Templates should be loaded from existing project storage instead of a new top
level directory. A later implementation can use:

```text
.tiny-claw/subagents/templates/explorer.toml
.tiny-claw/subagents/tool-profiles.toml
.tiny-claw/subagents/skill-sets.toml
```

Built-in templates can also be compiled into the binary, with workspace
templates extending or overriding them if that becomes useful.

## Tool Profiles

Templates should reference tool profiles rather than listing individual tools
inline. A profile is a named tool set plus optional policy.

Examples:

```toml
[tool_profiles.read_only]
include = ["read_file", "grep", "load_skill"]

[tool_profiles.tester]
include = ["read_file", "grep", "bash", "load_skill"]
policy = "test_commands_only"
```

The first `explorer` template should use `read_only` and should not include
`bash`. Shell access is difficult to make truly read-only. If shell access is
added later, it should go through an explicit command policy rather than
exposing the full foreground `bash` tool under a misleading read-only label.

## Skill Sets

Skills should use the same indirection as tools:

```toml
[skill_sets.rust_explorer]
skills = ["rust", "repo-search"]
```

Templates reference a skill set. A future override can request extra skills,
but the template decides whether extra skills are allowed and which skill ids
are permitted.

This keeps common subagent roles reusable while avoiding model-driven privilege
expansion.

## AgentSpec Resolution

Templates are not executed directly. Runtime delegation resolves a template and
request into a final `AgentSpec`:

```rust
struct DelegateAgentRequest {
    template_id: String,
    task: String,
    overrides: AgentOverrides,
}

struct AgentOverrides {
    role_prompt_append: Option<String>,
    extra_skills: Vec<String>,
    tool_profile: Option<String>,
    output_contract: Option<String>,
    context_budget: Option<ContextBudget>,
}

struct AgentSpec {
    template_id: Option<String>,
    role: AgentRole,
    task: String,
    system_prompt: String,
    tool_profile: ToolProfileId,
    skills: Vec<String>,
    context_budget: ContextBudget,
    output_contract: OutputContract,
    parent_session_id: Option<String>,
}
```

Resolution flow:

```text
DelegateAgentRequest
  -> SubagentTemplateRegistry.resolve(template_id)
  -> apply allowed overrides
  -> resolve tool profile and skill set
  -> validate policy
  -> AgentSpec
  -> AgentSupervisor.spawn(spec)
```

The model-facing request expresses intent. The runtime owns the final authority
to grant tools, skills, prompts, budgets, and output contracts.

## Template Sources and Precedence

The first implementation should include built-in templates and profiles for
stable system behavior. Workspace templates may be added later, but they should
not silently override built-in template ids in the first version.

Recommended precedence:

```text
built-in templates: always available
workspace templates: may add new ids
workspace overrides of built-in ids: disabled until explicitly designed
```

This prevents a workspace file from weakening the default security boundary of
a built-in template such as `explorer`.

## Overrides

The design should reserve a model-customization path even if the first
implementation keeps it mostly closed.

First version behavior:

- Support `template_id` and `task`.
- Include the `overrides` field in internal structs if convenient, but reject or
  ignore all capability-expanding overrides.
- Optionally allow only `role_prompt_append`, because it does not grant new
  tools or skills.

Future behavior:

- Allow `extra_skills` only when the template allows them.
- Allow `tool_profile` only for explicit template-approved alternatives.
- Allow `output_contract` only for known contracts.
- Allow `context_budget` only within runtime-approved bounds.

The rule is: templates define the capability ceiling; delegation requests supply
task-specific parameters and safe customizations.

## Initial Templates

Start with one template:

```text
explorer
```

Purpose:

- Explore code, docs, or logs in an isolated context.
- Use read-only tools.
- Exclude `bash` in the first version.
- Return a concise evidence-backed report.
- Avoid editing files, running mutating commands, or making final project
  changes.

Later templates can include:

```text
reviewer
```

Read-only code review. Returns findings with severity, file, line, and reason.

```text
tester
```

Runs verification commands under a restricted shell policy and summarizes
results. This should wait until background task and command policy boundaries
are clearer.

## Memory and Observability

Each subagent should have an independent transcript and event stream, for
example:

```text
.tiny-claw/agents/<agent_id>/history.jsonl
.tiny-claw/agents/<agent_id>/events.jsonl
.tiny-claw/agents/<agent_id>/report.md
```

The main agent receives only lifecycle observations and the final report unless
it explicitly asks for more detail. This preserves the main context while still
leaving an auditable record on disk.

Runtime observations should be represented distinctly from tool observations in
local memory:

```rust
enum ObservationKind {
    ToolResult,
    RuntimeEvent,
}
```

Provider adapters may still render runtime events through the vendor-compatible
message shape required by each API, but the local transcript should preserve the
semantic difference between a workspace tool result and an agent runtime event.

Reporter output should include agent identity:

```text
[agent_001 explorer] tool read_file src/engine/mod.rs
[agent_001 explorer] completed
```

## Error Handling

| Case | Behavior |
| --- | --- |
| Unknown template | Return a runtime command error to the main agent |
| Template references unknown tool profile | Fail spawn before creating an agent |
| Template references unknown skill set | Fail spawn before creating an agent |
| Override not allowed by template | Reject the delegation request |
| Subagent fails during provider call | Mark agent failed and expose error through status/join |
| Subagent is cancelled | Mark cancelled and keep partial transcript/events |
| Subagent completes without final content | Return an empty-report error |

## Completion Reports

The first output contract should be Markdown rather than strict JSON. Use a
stable section shape:

```markdown
## Summary

## Evidence

## Uncertainty
```

This is easier for the model to produce reliably while still giving the main
agent a predictable report structure. Structured JSON reports can be added later
for templates that need machine-readable outputs.

## Execution and Cancellation

Keep the first runtime implementation aligned with the current synchronous
engine. Use `std::thread` for background subagent runs instead of converting the
whole engine to async early.

Cancellation should initially be cooperative. `cancel_agent` marks the agent as
cancel requested; the subagent run checks that flag between turns and stops
before the next provider call or tool batch. A provider call or foreground tool
already in progress may not stop immediately. Hard interruption can be designed
later if the project adds cancellable provider and tool APIs.

## Incremental Plan

1. Add `ProviderFactory` and update app construction to build engines from a
   factory-compatible provider configuration.
2. Introduce `AgentSpec` as a resolved runtime input for a single agent run.
3. Add template, tool profile, and skill set registries with one built-in
   `explorer` template.
4. Add `AgentSupervisor` with in-memory handles and background execution.
5. Add runtime command classification for `delegate_agent`, `agent_status`,
   `join_agent`, and `cancel_agent`.
6. Persist subagent transcripts, events, and final reports under `.tiny-claw`.

Each step should remain compileable and testable on its own.

## Testing

Add focused tests for:

- Template resolution produces the expected `AgentSpec`.
- Disallowed overrides are rejected.
- Unknown templates, profiles, and skill sets return clear errors.
- `delegate_agent` creates a running handle without registering a normal tool.
- `join_agent` returns only the final report to the main session.
- The explorer template receives only read-only tools.
- Failed or cancelled agents keep inspectable event records.

Integration tests can use `MockProvider` to simulate a subagent that calls
`grep` or `read_file`, then returns a final report.
