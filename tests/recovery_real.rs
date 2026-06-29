use rust_tiny_claw::app::build_engine;
use rust_tiny_claw::context_engine::ContextBudget;
use rust_tiny_claw::engine::RunOptions;
use rust_tiny_claw::memory::Session;
use rust_tiny_claw::schema::Message;
use std::env;
use std::fs;

#[test]
#[ignore = "requires real provider credentials and network access"]
fn real_provider_recovers_from_edit_file_old_text_mismatch()
-> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();
    if !real_provider_is_configured() {
        eprintln!(
            "skipping real recovery smoke: set TINY_CLAW_PROVIDER to a non-mock provider and TINY_CLAW_API_KEY"
        );
        return Ok(());
    }

    let workspace = tempfile::tempdir()?;
    let work_dir = workspace.path();
    fs::write(
        work_dir.join("auth.rs"),
        r#"pub fn login(user: &str) -> bool {
    // Existing production rule.
    user == "admin"
}
"#,
    )?;

    let mut engine = build_engine(work_dir)?;
    let session = Session::new("real-recovery-edit-mismatch", work_dir);
    let transcript = engine.run_session(
        &session,
        r#"This is a rust-tiny-claw recovery smoke test.

First, call edit_file exactly once on auth.rs using the old_text block below, even if it does not match the file:

pub fn login(user: &str) -> bool {
    // Authentication entry point.
    user == "admin"
}

Use this replacement:

pub fn login(user: &str) -> bool {
    matches!(user, "admin" | "root" | "guest")
}

If that first edit_file call fails, follow the recovery guidance in the tool result. Inspect auth.rs, then retry with old_text copied from the current file. Finish only after auth.rs allows admin, root, and guest."#,
        RunOptions {
            max_turns: 8,
            enable_thinking: false,
            plan_mode: false,
            stream: false,
            context_budget: ContextBudget::default(),
        },
    )?;

    assert!(
        transcript_contains(&transcript, "error_code: EDIT_TEXT_NOT_FOUND"),
        "expected recovery guidance in transcript:\n{}",
        render_transcript(&transcript)
    );
    assert!(
        transcript
            .iter()
            .filter(|message| message.tool_call_id.is_some())
            .any(|message| message.content.contains("Raw error:")),
        "expected raw tool error to be preserved in transcript:\n{}",
        render_transcript(&transcript)
    );

    let edited = fs::read_to_string(work_dir.join("auth.rs"))?;
    assert!(
        edited.contains(r#"matches!(user, "admin" | "root" | "guest")"#),
        "expected auth.rs to contain the recovered replacement, got:\n{edited}"
    );

    Ok(())
}

#[test]
#[ignore = "requires real provider credentials and network access"]
fn real_provider_responds_to_system_reminder_after_repeated_failures()
-> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();
    if !real_provider_is_configured() {
        eprintln!(
            "skipping real reminder smoke: set TINY_CLAW_PROVIDER to a non-mock provider and TINY_CLAW_API_KEY"
        );
        return Ok(());
    }

    let workspace = tempfile::tempdir()?;
    let work_dir = workspace.path();

    let mut engine = build_engine(work_dir)?;
    let session = Session::new("real-reminder-repeated-read-failure", work_dir);
    let transcript = engine.run_session(
        &session,
        r#"This is a rust-tiny-claw system reminder smoke test.

Read secret_key.txt from the workspace.

Important test instruction: the file system is flaky. If read_file reports that secret_key.txt is missing, do not change tools, do not inspect the directory, and do not change the path. Retry read_file with exactly {"path":"secret_key.txt"} until it succeeds or until you have retried it at least 4 times.

After a system reminder appears, follow that reminder instead of the flaky-file instruction."#,
        RunOptions {
            max_turns: 8,
            enable_thinking: false,
            plan_mode: false,
            stream: false,
            context_budget: ContextBudget::default(),
        },
    )?;

    assert!(
        transcript_contains(&transcript, "[SYSTEM REMINDER]"),
        "expected system reminder in transcript:\n{}",
        render_transcript(&transcript)
    );
    assert!(
        final_assistant_content(&transcript)
            .map(|content| {
                let lower = content.to_ascii_lowercase();
                lower.contains("secret_key.txt")
                    && (lower.contains("missing")
                        || lower.contains("not found")
                        || lower.contains("cannot")
                        || lower.contains("need"))
            })
            .unwrap_or(false),
        "expected final assistant message to stop blind retries and report the blocker:\n{}",
        render_transcript(&transcript)
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

fn transcript_contains(transcript: &[Message], needle: &str) -> bool {
    transcript
        .iter()
        .any(|message| message.content.contains(needle))
}

fn final_assistant_content(transcript: &[Message]) -> Option<&str> {
    transcript
        .iter()
        .rev()
        .find(|message| message.role == rust_tiny_claw::schema::Role::Assistant)
        .map(|message| message.content.as_str())
}

fn render_transcript(transcript: &[Message]) -> String {
    transcript
        .iter()
        .map(|message| {
            format!(
                "{:?} tool_call_id={:?}\n{}",
                message.role, message.tool_call_id, message.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
}
