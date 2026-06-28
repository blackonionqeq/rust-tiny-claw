# Prompt Assembly, AGENTS.md, and Skills Design

## Goal

Move prompt construction out of the ReAct loop and into `context_engine` so the
engine no longer grows hard-coded prompt strings. The first implementation
should support workspace instructions from `AGENTS.md` and explicitly enabled
Codex-style skills, while keeping the behavior small enough for the current
course chapter.

## Non-Goals

- Do not add a full agent framework or plugin system.
- Do not automatically choose skills by embeddings, keyword search, or model
  routing.
- Do not implement context compaction, reminders, subagents, tracing, or
  token-budget enforcement in this change.
- Do not add a YAML parser unless skill metadata becomes complex enough to need
  one.

## Prompt Assembly

`AgentEngine` should ask `ContextManager` for the system prompt before each run.
The engine keeps responsibility for the ReAct loop, provider calls, tool
dispatch, and message history. `ContextManager` owns prompt sources, prompt
ordering, and rendering.

The rendered system prompt should use stable sections:

```text
# Base Instructions

<built-in tiny-claw instructions>

# Workspace Instructions

<AGENTS.md content, when present>

# Active Skills

## <skill id>

Source: <relative path to SKILL.md>

<skill body>
```

The order is fixed: base instructions first, workspace instructions second,
skills last. This makes test snapshots and model behavior easier to reason
about.

## AGENTS.md Loading

The first version should load only `<work_dir>/AGENTS.md`.

- Missing `AGENTS.md` is allowed and simply omits the workspace section.
- Present but unreadable `AGENTS.md` is an error, because the user likely
  expected those instructions to apply.
- The file content is treated as Markdown text and is not parsed.
- Parent-directory discovery is left for a later change, because recursive
  lookup introduces precedence, override, and workspace-boundary questions.

## Skills Layout

Skills should use a Codex-style directory layout:

```text
.tiny-claw/
  skills/
    rust/
      SKILL.md
    git/
      SKILL.md
```

The first version loads only explicitly enabled skills from:

```text
.tiny-claw/skills/<skill-id>/SKILL.md
```

Skill activation should initially come from `TINY_CLAW_SKILLS`, using a
comma-separated list such as:

```text
TINY_CLAW_SKILLS=rust,git
```

Skills are loaded and rendered in the order listed by the environment variable.
Unknown skill ids are errors. Empty entries are ignored after trimming
whitespace.

## Skill Frontmatter

`SKILL.md` may start with simple frontmatter:

```markdown
---
name: rust
description: Rust project conventions and Cargo workflows.
---

# Rust Skill
```

The initial parser should be deliberately small:

- Detect frontmatter only when the file starts with `---`.
- End frontmatter at the next line containing only `---`.
- Parse simple `key: value` pairs for `name` and `description`.
- Treat unsupported YAML features as plain text rather than blocking the skill.
- Use the directory name as the skill id even when frontmatter includes `name`.

This avoids adding `serde_yaml` or another YAML parser for two optional fields.
If future skills need nested metadata, arrays, or typed configuration, revisit a
maintained YAML or TOML dependency then.

## Recursive Skill Support Later

There are two separate recursive concerns.

Skill discovery recursion can later allow:

```text
.tiny-claw/skills/coding/rust/SKILL.md
.tiny-claw/skills/browser/agent-browser/SKILL.md
```

Those skills would use relative ids such as `coding/rust` and
`browser/agent-browser`.

Skill resource recursion should not automatically inject entire directories into
the prompt. A future version can allow `SKILL.md` to explicitly reference
Markdown files under `references/`, while `scripts/` and `assets/` remain
available as resources but are not prompt text by default.

## Error Handling

| Case | Behavior |
| --- | --- |
| `AGENTS.md` missing | Skip workspace section |
| `AGENTS.md` unreadable | Return context error |
| `TINY_CLAW_SKILLS` unset or empty | Skip skills section |
| Skill id not found | Return context error |
| `SKILL.md` unreadable | Return context error |
| Frontmatter malformed | Treat full file as Markdown body |
| Skill body empty | Allow it |

## Testing

Add focused unit tests for `context_engine`:

- Base prompt renders without workspace instructions or skills.
- `AGENTS.md` content appears in the workspace section.
- Explicit skills render in environment order.
- Missing explicit skills return a clear error.
- Frontmatter is stripped from the rendered skill body when valid.
- Malformed frontmatter falls back to rendering the full file body.

Integration with `AgentEngine` only needs a narrow test proving the engine uses
the `ContextManager` output as the first system message. Provider behavior,
tool execution, and Feishu integration should remain covered by their existing
tests.
