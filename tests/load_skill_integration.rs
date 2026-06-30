use rust_tiny_claw::context_engine::{ContextBudget, ContextManager};
use rust_tiny_claw::engine::{AgentEngine, RunOptions};
use rust_tiny_claw::memory::FileMemory;
use rust_tiny_claw::provider::{Provider, ProviderError};
use rust_tiny_claw::schema::{Message, ToolCall, ToolDefinition};
use rust_tiny_claw::telemetry::Telemetry;
use rust_tiny_claw::tools::{LoadSkillTool, ToolRegistry, WriteFileTool};
use serde_json::json;
use std::fs;
use tempfile::tempdir;

#[test]
fn mock_load_skill_keeps_body_out_of_initial_prompt_then_loads_on_demand()
-> Result<(), Box<dyn std::error::Error>> {
    let work_dir = tempdir()?;
    write_skill(
        work_dir.path(),
        "audit",
        "---\nname: Audit\ndescription: Load this skill before writing audit proof.\n---\n\n# Audit Skill\nWhen this skill is loaded, write AUDIT_SENTINEL to skill-proof.txt.\n",
    )?;

    let mut registry = ToolRegistry::new();
    registry.register(LoadSkillTool::new(
        work_dir.path(),
        vec!["audit".to_string()],
    )?)?;
    registry.register(WriteFileTool::new(work_dir.path())?)?;

    let mut engine = AgentEngine::new(
        LoadSkillProvider { calls: 0 },
        registry,
        ContextManager::new(work_dir.path(), vec!["audit".to_string()]),
        FileMemory::new(work_dir.path().join(".tiny-claw")),
        Telemetry::default(),
    );

    let transcript = engine.run_with_options(
        "Use the audit skill to create the proof file.",
        RunOptions {
            max_turns: 4,
            enable_thinking: false,
            plan_mode: false,
            stream: false,
            context_budget: ContextBudget::default(),
        },
    )?;

    assert!(
        transcript_has_tool_call(&transcript, "load_skill"),
        "expected transcript to call load_skill:\n{}",
        render_transcript(&transcript)
    );
    assert!(
        transcript_has_tool_call(&transcript, "write_file"),
        "expected transcript to call write_file after loading the skill:\n{}",
        render_transcript(&transcript)
    );
    assert_eq!(
        fs::read_to_string(work_dir.path().join("skill-proof.txt"))?,
        "AUDIT_SENTINEL\n"
    );

    Ok(())
}

struct LoadSkillProvider {
    calls: usize,
}

impl Provider for LoadSkillProvider {
    fn name(&self) -> &'static str {
        "load-skill-provider"
    }

    fn generate(
        &mut self,
        messages: &[Message],
        available_tools: Option<&[ToolDefinition]>,
    ) -> Result<Message, ProviderError> {
        self.calls += 1;
        let system_prompt = messages
            .iter()
            .find(|message| message.role == rust_tiny_claw::schema::Role::System)
            .map(|message| message.content.as_str())
            .unwrap_or_default();

        if self.calls == 1 {
            assert!(
                system_prompt.contains("Audit"),
                "initial prompt should include skill catalog metadata:\n{system_prompt}"
            );
            assert!(
                !system_prompt.contains("AUDIT_SENTINEL"),
                "initial prompt should not include full skill body:\n{system_prompt}"
            );
            assert_tool_available(available_tools, "load_skill");
            return Ok(Message::assistant_with_tools(
                "load audit skill",
                vec![ToolCall::new(
                    "call_load_audit",
                    "load_skill",
                    json!({ "skill_id": "audit" }),
                )],
            ));
        }

        if self.calls == 2 {
            assert!(
                messages
                    .iter()
                    .any(|message| message.content.contains("AUDIT_SENTINEL")),
                "second provider call should include loaded skill body:\n{}",
                render_transcript(messages)
            );
            assert_tool_available(available_tools, "write_file");
            return Ok(Message::assistant_with_tools(
                "write audit proof",
                vec![ToolCall::new(
                    "call_write_proof",
                    "write_file",
                    json!({
                        "path": "skill-proof.txt",
                        "content": "AUDIT_SENTINEL\n"
                    }),
                )],
            ));
        }

        Ok(Message::assistant("skill proof complete"))
    }
}

fn assert_tool_available(available_tools: Option<&[ToolDefinition]>, name: &str) {
    assert!(
        available_tools
            .unwrap_or_default()
            .iter()
            .any(|tool| tool.name == name),
        "expected tool {name} to be available"
    );
}

fn write_skill(
    work_dir: &std::path::Path,
    skill_id: &str,
    content: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let skill_dir = work_dir.join(".tiny-claw").join("skills").join(skill_id);
    fs::create_dir_all(&skill_dir)?;
    fs::write(skill_dir.join("SKILL.md"), content)?;
    Ok(())
}

fn transcript_has_tool_call(transcript: &[Message], tool_name: &str) -> bool {
    transcript.iter().any(|message| {
        message
            .tool_calls
            .iter()
            .any(|tool_call| tool_call.name == tool_name)
    })
}

fn render_transcript(transcript: &[Message]) -> String {
    transcript
        .iter()
        .map(|message| {
            let tool_calls = message
                .tool_calls
                .iter()
                .map(|tool_call| format!("{} {:?}", tool_call.name, tool_call.arguments))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{:?} tool_call_id={:?} tool_calls=[{}]\n{}",
                message.role, message.tool_call_id, tool_calls, message.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
}
