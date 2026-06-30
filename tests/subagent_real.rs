use rust_tiny_claw::app::build_engine;
use rust_tiny_claw::context_engine::ContextBudget;
use rust_tiny_claw::engine::RunOptions;
use rust_tiny_claw::memory::Session;
use rust_tiny_claw::schema::Message;
use std::env;
use std::fs;
use std::path::Path;

#[test]
#[ignore = "requires real provider credentials and network access"]
fn real_provider_delegates_to_subagent_and_joins_report() -> Result<(), Box<dyn std::error::Error>>
{
    let _ = dotenvy::dotenv();
    if !real_provider_is_configured() {
        eprintln!(
            "skipping real subagent smoke: set TINY_CLAW_PROVIDER to a non-mock provider and TINY_CLAW_API_KEY"
        );
        return Ok(());
    }

    let workspace = tempfile::tempdir()?;
    let work_dir = workspace.path();
    fs::create_dir_all(work_dir.join("src"))?;
    fs::write(
        work_dir.join("src/lib.rs"),
        r#"pub fn answer() -> u8 {
    42
}
"#,
    )?;

    let mut engine = build_engine(work_dir)?;
    let session = Session::new("real-subagent-delegation", work_dir);
    let transcript = engine.run_session(
        &session,
        r#"This is a rust-tiny-claw subagent smoke test.

Call delegate_agent exactly once with template_id "explorer". The delegated task is:

Inspect src/lib.rs with read-only tools. Report which public function it defines and include the file path as evidence.

After delegate_agent returns an agent_id, call join_agent with that exact agent_id. Do not answer from your own knowledge before join_agent returns. In your final response, summarize the joined subagent report and mention src/lib.rs and answer."#,
        RunOptions {
            max_turns: 8,
            enable_thinking: false,
            plan_mode: false,
            stream: false,
            context_budget: ContextBudget::default(),
        },
    )?;

    assert!(
        transcript_has_tool_call(&transcript, "delegate_agent"),
        "expected parent transcript to call delegate_agent:\n{}",
        render_transcript(&transcript)
    );
    assert!(
        transcript_has_tool_call(&transcript, "join_agent"),
        "expected parent transcript to call join_agent:\n{}",
        render_transcript(&transcript)
    );
    assert!(
        transcript_contains(&transcript, "src/lib.rs")
            && transcript_contains(&transcript, "answer"),
        "expected joined report or final answer to mention src/lib.rs and answer:\n{}",
        render_transcript(&transcript)
    );
    assert!(
        persisted_subagent_report_contains(work_dir, "src/lib.rs")
            && persisted_subagent_report_contains(work_dir, "answer"),
        "expected persisted subagent report under .tiny-claw/agents to mention src/lib.rs and answer"
    );

    Ok(())
}

fn real_provider_is_configured() -> bool {
    matches!(
        env::var("TINY_CLAW_PROVIDER").as_deref(),
        Ok("openai-compatible" | "claude-compatible")
    ) && env::var("TINY_CLAW_API_KEY")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn transcript_has_tool_call(transcript: &[Message], tool_name: &str) -> bool {
    transcript.iter().any(|message| {
        message
            .tool_calls
            .iter()
            .any(|tool_call| tool_call.name == tool_name)
    })
}

fn transcript_contains(transcript: &[Message], needle: &str) -> bool {
    transcript
        .iter()
        .any(|message| message.content.contains(needle))
}

fn persisted_subagent_report_contains(work_dir: &Path, needle: &str) -> bool {
    let agent_root = work_dir.join(".tiny-claw").join("agents");
    let Ok(entries) = fs::read_dir(agent_root) else {
        return false;
    };

    entries.filter_map(Result::ok).any(|entry| {
        fs::read_to_string(entry.path().join("report.md"))
            .map(|report| report.contains(needle))
            .unwrap_or(false)
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
