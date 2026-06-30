use rust_tiny_claw::app::build_engine;
use rust_tiny_claw::context_engine::ContextBudget;
use rust_tiny_claw::engine::RunOptions;
use rust_tiny_claw::memory::Session;
use rust_tiny_claw::schema::{Message, Role};
use std::env;
use std::fs;
use std::sync::Mutex;
use tempfile::tempdir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
#[ignore = "requires real provider credentials and network access"]
fn real_provider_completes_main_agent_tool_loop() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();
    if !real_provider_is_configured() {
        eprintln!(
            "skipping real tool-loop smoke: set TINY_CLAW_PROVIDER to a non-mock provider and TINY_CLAW_API_KEY"
        );
        return Ok(());
    }

    let workspace = tempdir()?;
    let work_dir = workspace.path();
    fs::create_dir_all(work_dir.join("src"))?;
    fs::write(
        work_dir.join("notes.txt"),
        "alpha\nTODO: replace me\nomega\n",
    )?;
    fs::write(
        work_dir.join("src/lib.rs"),
        "pub fn marker() {}\n// TODO: fixture\n",
    )?;

    let mut engine = build_engine(work_dir)?;
    let session = Session::new("real-main-agent-tool-loop", work_dir);
    let transcript = engine.run_session(
        &session,
        r#"This is a rust-tiny-claw real-provider tool-loop smoke test.

Use the available tools to do this exact workspace task:
1. Read notes.txt.
2. Write generated.txt with exactly: tool-loop-ok
3. Edit notes.txt, replacing TODO: replace me with done: tool-loop-ok.
4. Grep for TODO under src.

Use read_file, write_file, edit_file, and grep at least once. Finish only after the files are updated."#,
        RunOptions {
            max_turns: 10,
            enable_thinking: false,
            plan_mode: false,
            stream: false,
            context_budget: ContextBudget::default(),
        },
    )?;

    for tool_name in ["read_file", "write_file", "edit_file", "grep"] {
        assert!(
            transcript_has_tool_call(&transcript, tool_name),
            "expected transcript to call {tool_name}:\n{}",
            render_transcript(&transcript)
        );
    }
    assert_eq!(
        fs::read_to_string(work_dir.join("generated.txt"))?,
        "tool-loop-ok"
    );
    assert!(
        fs::read_to_string(work_dir.join("notes.txt"))?.contains("done: tool-loop-ok"),
        "expected notes.txt to contain the edited marker"
    );

    Ok(())
}

#[test]
#[ignore = "requires real provider credentials and network access"]
fn real_provider_streams_tool_call_through_engine_dispatch()
-> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();
    if !real_provider_is_configured() {
        eprintln!(
            "skipping real streaming tool-call smoke: set TINY_CLAW_PROVIDER to a non-mock provider and TINY_CLAW_API_KEY"
        );
        return Ok(());
    }

    let workspace = tempdir()?;
    let work_dir = workspace.path();
    fs::write(work_dir.join("stream.txt"), "stream-tool-ok\n")?;

    let mut engine = build_engine(work_dir)?;
    let session = Session::new("real-streaming-tool-call", work_dir);
    let transcript = engine.run_session(
        &session,
        r#"This is a rust-tiny-claw streaming tool-call smoke test.

Call read_file exactly once for stream.txt. After the tool result returns, reply with the token stream-tool-ok."#,
        RunOptions {
            max_turns: 4,
            enable_thinking: false,
            plan_mode: false,
            stream: true,
            context_budget: ContextBudget::default(),
        },
    )?;

    assert!(
        transcript_has_tool_call(&transcript, "read_file"),
        "expected streamed provider response to call read_file:\n{}",
        render_transcript(&transcript)
    );
    assert!(
        final_assistant_content(&transcript)
            .map(|content| content.to_ascii_lowercase().contains("stream-tool-ok"))
            .unwrap_or(false),
        "expected final assistant response to contain stream-tool-ok:\n{}",
        render_transcript(&transcript)
    );

    Ok(())
}

#[test]
#[ignore = "requires real provider credentials and network access"]
fn real_provider_loads_enabled_workspace_skill() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = ENV_LOCK.lock().unwrap();
    let _ = dotenvy::dotenv();
    if !real_provider_is_configured() {
        eprintln!(
            "skipping real load_skill smoke: set TINY_CLAW_PROVIDER to a non-mock provider and TINY_CLAW_API_KEY"
        );
        return Ok(());
    }

    let workspace = tempdir()?;
    let work_dir = workspace.path();
    write_skill(
        work_dir,
        "audit",
        "---\nname: Audit\ndescription: Load this skill before writing audit proof.\n---\n\n# Audit Skill\nAfter this skill is loaded, write skill-proof.txt with exactly: real-skill-ok\n",
    )?;

    let previous_skills = env::var("TINY_CLAW_SKILLS").ok();
    set_env("TINY_CLAW_SKILLS", "audit");
    let result = run_real_load_skill_test(work_dir);
    restore_env("TINY_CLAW_SKILLS", previous_skills);
    result
}

fn run_real_load_skill_test(work_dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = build_engine(work_dir)?;
    let session = Session::new("real-load-skill", work_dir);
    let transcript = engine.run_session(
        &session,
        r#"This is a rust-tiny-claw load_skill smoke test.

The Available Skills catalog includes an Audit skill. Call load_skill with skill_id "audit" before writing any proof file. Then follow the loaded skill instructions exactly."#,
        RunOptions {
            max_turns: 6,
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
    assert_eq!(
        fs::read_to_string(work_dir.join("skill-proof.txt"))?,
        "real-skill-ok"
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

fn final_assistant_content(transcript: &[Message]) -> Option<&str> {
    transcript
        .iter()
        .rev()
        .find(|message| message.role == Role::Assistant)
        .map(|message| message.content.as_str())
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

fn set_env(key: &str, value: &str) {
    unsafe {
        env::set_var(key, value);
    }
}

fn remove_env(key: &str) {
    unsafe {
        env::remove_var(key);
    }
}

fn restore_env(key: &str, value: Option<String>) {
    match value {
        Some(value) => set_env(key, &value),
        None => remove_env(key),
    }
}
