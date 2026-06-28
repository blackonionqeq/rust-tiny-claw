# Progressive Skill Loading Design

## Goal

Reduce initial prompt size by rendering only compact skill metadata in the
system prompt, while still allowing the model to load relevant skill
instructions when needed.

This extends the existing explicit skill model. `TINY_CLAW_SKILLS` continues to
define which skills are enabled for a run. Progressive loading changes whether
enabled skill bodies are injected immediately.

## Non-Goals

- Do not add embedding search, ranking, or automatic semantic skill routing.
- Do not recursively inject skill resources into the prompt.
- Do not implement user slash commands in the first progressive loading change.
- Do not add a general plugin system.

## Skill Manifest

The context engine should parse a compact manifest from each enabled
`SKILL.md`. The first manifest fields are:

```yaml
name: rust
description: Rust project conventions and Cargo workflows.
disable-model-invocation: false
```

`name` and `description` are optional display metadata. The skill directory name
remains the authoritative skill id.

`disable-model-invocation` is an invocation policy. It controls whether the
model may discover and load the skill by itself.

The first implementation should support strict boolean values only:

- `true`
- `false`

If `disable-model-invocation` is present with any other value, return a context
error. This field has permission semantics, so silently treating malformed
values as ordinary Markdown would be risky.

## Prompt Rendering

The system prompt should render a compact catalog instead of full skill bodies:

```text
# Available Skills

The following enabled skills can be loaded when relevant. To use one, call
load_skill with its id.

- id: rust
  name: Rust
  description: Rust project conventions and Cargo workflows.
  source: .tiny-claw/skills/rust/SKILL.md
```

Only model-invokable skills appear in this list.

An enabled skill with `disable-model-invocation: true` must not appear in the
catalog. The model should not learn that skill id from the initial prompt.

## Model-Initiated Loading

Add a read-only `load_skill` tool.

Input:

```json
{ "skill_id": "rust" }
```

Behavior:

- Validate the skill id with the same path-safety rules used by the context
  engine.
- Load only skills enabled by `TINY_CLAW_SKILLS`.
- Reject missing or disabled skills with a clear tool error.
- Reject skills whose manifest has `disable-model-invocation: true`.
- Return the skill body with valid frontmatter removed, plus the source path.

The engine should track skills loaded during the run and make repeated
`load_skill` calls idempotent. Re-loading an already loaded skill may return the
same body or a short "already loaded" result, but it must not duplicate the
skill instructions in long-term conversation state.

## User-Invoked Loading Later

`disable-model-invocation: true` does not mean a skill is unusable. It means the
model may not discover or request it on its own.

A later slash-command feature can allow explicit user loading, for example:

```text
/skill load secret-deploy
```

User-invoked loading should be treated as a user-authorized context event. It
may load skills that are enabled but hidden from model invocation. The event
should record that the skill was loaded by user command rather than by a model
tool call.

Slash-command loading should still enforce path safety and workspace skill
boundaries.

## Skill Resources Later

Skill body loading should not recursively load referenced files.

If a skill needs additional Markdown references later, add a separate
`load_skill_resource` tool:

```json
{ "skill_id": "rust", "path": "references/cargo.md" }
```

That tool should allow only relative paths inside the selected skill directory
and should reject scripts, assets, parent traversal, and absolute paths unless a
future design explicitly grants access.

## Error Handling

| Case | Behavior |
| --- | --- |
| Enabled skill missing | Context error |
| Frontmatter missing | Use id, empty metadata, model-invokable |
| `name` missing | Use skill id for display |
| `description` missing | Render an empty or omitted description |
| `disable-model-invocation` missing | Treat as `false` |
| `disable-model-invocation: true` | Hide from catalog and model tool loading |
| Invalid `disable-model-invocation` value | Context error |
| Model loads hidden skill | Tool error |
| Model loads unenabled skill | Tool error |

## Testing

Add focused tests around `context_engine` and the tool boundary:

- Enabled skills render as catalog entries without full bodies.
- Manifest metadata appears in the catalog.
- Skills with `disable-model-invocation: true` are omitted from the catalog.
- Invalid `disable-model-invocation` values return context errors.
- `load_skill` returns the body for enabled model-invokable skills.
- `load_skill` rejects hidden, missing, and unenabled skills.
- Repeated `load_skill` calls do not duplicate loaded instructions in context.
