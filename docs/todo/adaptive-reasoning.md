# Adaptive Reasoning Notes

## Context

`rust-tiny-claw` currently uses a fixed `RunOptions.enable_thinking` boolean. That is useful for smoke tests, but it is not the shape most production agent systems eventually want. Reasoning has real latency and token cost, while many tool calls and retrieval-style tasks do not benefit from an extra thinking phase.

The next step should be a policy-driven mode:

```rust
pub enum ReasoningMode {
    Off,
    On,
    Auto,
}
```

`Auto` should become the default once the policy is reliable enough.

## What Current Providers Expose

OpenAI reasoning models expose a `reasoning.effort` control. The official docs describe it as guidance for how much the model should think, with lower effort favoring speed and token economy and higher effort improving quality for harder tasks. The same docs also note that models reason adaptively within the selected effort level: simpler tasks use fewer reasoning tokens, while harder tasks use more.

Anthropic exposes two related mechanisms:

- Manual extended thinking with `thinking: { "type": "enabled", "budget_tokens": N }`.
- Adaptive thinking with `thinking: { "type": "adaptive" }`, optionally guided by an effort parameter.

Anthropic's docs describe adaptive thinking as letting Claude decide whether and how much to use extended thinking based on request complexity. They also note that adaptive thinking is especially relevant for agentic workflows because it can reason between tool calls.

## Harness-Level Policy

Even when providers expose adaptive thinking, the harness should still own an outer policy. Provider-native adaptive thinking controls model-side compute, but the harness has additional context:

- User intent and latency expectations.
- Tool availability.
- Tool failure history.
- Whether the task is read-only or mutating.
- Whether the loop is recovering from malformed tool calls.
- Context size and number of files involved.
- Local budget limits and maximum turn count.

The harness should decide whether to request no thinking, provider-native adaptive thinking, or an explicit pre-action thinking phase.

## Suggested First Policy

Start with deterministic heuristics instead of another model call.

Use `Off` when:

- The prompt is short and asks for a single direct tool action.
- The task is classification, formatting, echoing, or quick lookup.
- The user asks for speed or direct execution.

Use `On` when:

- The task mentions design, plan, analyze, debug, implement, refactor, compare, migrate, or review.
- The operation is potentially destructive or spans multiple files.
- A previous tool call failed.
- Provider tool-call JSON was malformed and needs a correction turn.
- The task has already consumed multiple turns without progress.

Use provider-native adaptive thinking when:

- The provider supports it cleanly.
- The task is agentic or multi-step, but the harness does not need a separate visible thinking phase.
- Tool calls may happen repeatedly and the model benefits from interleaved reasoning.

## Implementation Sketch

Replace:

```rust
pub struct RunOptions {
    pub max_turns: usize,
    pub enable_thinking: bool,
    pub stream: bool,
}
```

with:

```rust
pub struct RunOptions {
    pub max_turns: usize,
    pub reasoning: ReasoningMode,
    pub stream: bool,
}
```

Then resolve the effective behavior per turn:

```rust
let reasoning = match options.reasoning {
    ReasoningMode::Off => EffectiveReasoning::NoThinkingPhase,
    ReasoningMode::On => EffectiveReasoning::ThinkingPhase,
    ReasoningMode::Auto => policy.decide(&messages, &available_tools, turn_state),
};
```

Provider-native controls should be modeled separately from the engine's explicit thinking phase. A future provider request config could carry fields like:

```rust
pub enum ProviderReasoning {
    Default,
    Disabled,
    EffortLow,
    EffortMedium,
    EffortHigh,
    Adaptive,
}
```

That keeps the engine policy independent from vendor-specific request shapes.

## Open Questions

- Should `Auto` default to a harness thinking phase, provider-native adaptive reasoning, or no thinking when provider support is unknown?
- Should thinking output ever be displayed, or only logged as trace data?
- Should malformed tool calls trigger an automatic correction turn before returning `ProviderError`?
- Should failed tools force reasoning on for the next turn only, or for the rest of the run?
- How should cost and latency budgets be represented in `RunOptions`?

## References

- OpenAI reasoning models docs: https://platform.openai.com/docs/guides/reasoning
- Anthropic extended thinking docs: https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking
- Anthropic adaptive thinking docs: https://platform.claude.com/docs/en/build-with-claude/adaptive-thinking
