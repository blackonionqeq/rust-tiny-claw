use crate::provider::{Provider, ProviderError};
use crate::schema::{Message, ToolDefinition};
use std::collections::VecDeque;

// A deterministic provider used by the benchmark's deterministic tier. It
// replays a precomputed sequence of assistant turns so the engine drives the
// real tools against an isolated workspace without any network access or API
// cost. This is what makes the harness-mechanics regression suite runnable in
// CI: the model side is fixed, so a change in pass rate must come from the
// engine, tools, compaction, or recovery code.
//
// Each `generate` call with tool access enabled pops the next scripted turn.
// A turn with no tool calls ends the ReAct loop (the engine treats an empty
// tool-call list as completion), so scripted cases should finish with a plain
// assistant message. When the script is exhausted the provider returns a
// tool-free message so the run terminates instead of spinning to `max_turns`.
pub struct ScriptedProvider {
    turns: VecDeque<Message>,
}

impl ScriptedProvider {
    pub fn new(turns: Vec<Message>) -> Self {
        Self {
            turns: VecDeque::from(turns),
        }
    }
}

impl Provider for ScriptedProvider {
    fn name(&self) -> &'static str {
        "scripted-provider"
    }

    fn generate(
        &mut self,
        _messages: &[Message],
        available_tools: Option<&[ToolDefinition]>,
    ) -> Result<Message, ProviderError> {
        // The thinking phase calls generate with tools disabled. Benchmarks run
        // with thinking off, so this branch is only defensive: return an empty
        // assistant message and let the subsequent action call consume a turn.
        if available_tools.is_none() {
            return Ok(Message::assistant(String::new()));
        }

        Ok(self
            .turns
            .pop_front()
            .unwrap_or_else(|| Message::assistant("scripted provider exhausted; ending run")))
    }
}
