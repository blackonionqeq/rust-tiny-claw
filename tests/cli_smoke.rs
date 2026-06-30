use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use tempfile::tempdir;

#[test]
fn cli_mock_smoke_uses_real_startup_path_and_fallback_prompt()
-> Result<(), Box<dyn std::error::Error>> {
    let work_dir = tempdir()?;
    seed_workspace(work_dir.path())?;

    let mut child = Command::new(tiny_claw_bin())
        .current_dir(work_dir.path())
        .arg("--workspace")
        .arg(work_dir.path())
        .env("TINY_CLAW_PROVIDER", "mock")
        .env("TINY_CLAW_STREAM", "0")
        .env_remove("TINY_CLAW_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    child.stdin.as_mut().unwrap().write_all(b"\n")?;
    let output = child.wait_with_output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "expected CLI smoke to exit successfully\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("rust-tiny-claw engine boot sequence"),
        "expected boot sequence output, got:\n{stdout}"
    );
    assert!(
        stdout.contains("starting two-stage ReAct loop"),
        "expected ReAct loop output, got:\n{stdout}"
    );

    let memory_root = work_dir.path().join(".tiny-claw");
    assert!(
        memory_root.is_dir(),
        "expected CLI run to create memory root at {}",
        memory_root.display()
    );

    let edited = fs::read_to_string(memory_root.join("smoke/edit-target.rs"))?;
    assert!(
        edited.contains("Forbidden!"),
        "expected fallback smoke prompt to edit target file, got:\n{edited}"
    );
    assert!(
        !edited.contains("No auth, everyone can access."),
        "expected fallback smoke prompt to replace original auth block, got:\n{edited}"
    );

    Ok(())
}

fn tiny_claw_bin() -> &'static str {
    option_env!("CARGO_BIN_EXE_tiny-claw")
        .expect("Cargo should provide CARGO_BIN_EXE_tiny-claw for integration tests")
}

fn seed_workspace(work_dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(work_dir.join("src/bin"))?;
    fs::write(
        work_dir.join("Cargo.toml"),
        "[package]\nname = \"cli-smoke-fixture\"\nversion = \"0.0.0\"\n",
    )?;
    fs::write(
        work_dir.join("README.md"),
        "# CLI Smoke Fixture\n\nA small workspace fixture.\n",
    )?;
    fs::write(
        work_dir.join("src/bin/tiny-claw.rs"),
        "fn main() {\n    // TODO: fixture\n}\n",
    )?;
    Ok(())
}
