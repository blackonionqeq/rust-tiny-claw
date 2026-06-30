use rust_tiny_claw::app::{build_engine, stream_enabled};
use rust_tiny_claw::context_engine::ContextBudget;
use rust_tiny_claw::engine::RunOptions;
use rust_tiny_claw::memory::SessionManager;
use rust_tiny_claw::plan_mode::PlanModeSetting;
use std::env;
use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;

const SMOKE_PROMPT: &str = "Smoke-test the harness. Create .tiny-claw/smoke/edit-target.rs with an indented TODO auth block. Read it once. Then call edit_file exactly once to replace that block with a Forbidden return; in old_text, omit the original indentation so the fuzzy indentation fallback is exercised. Read the file once more to confirm the replacement. Do not repeat the edit flow after it succeeds. Finally, read Cargo.toml, README.md, and src/bin/tiny-claw.rs and call grep for TODO in one independent batch so the engine can execute multiple read-only tool calls in parallel.";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();

    println!("rust-tiny-claw engine boot sequence");

    let cli_input = cli_input_from_process()?;
    let work_dir = cli_input.work_dir.canonicalize()?;
    let mut engine = build_engine(&work_dir)?;
    let sessions = SessionManager::new();
    let session = sessions.get_or_create(format!("cli:{}", work_dir.display()), work_dir.clone());
    let plan_mode = cli_input.plan_mode.resolve(&cli_input.prompt, &work_dir);

    let options = RunOptions {
        max_turns: 12,
        enable_thinking: false,
        plan_mode,
        stream: stream_enabled()?,
        context_budget: ContextBudget::default(),
    };

    for line in engine.boot_plan(options) {
        println!("- {line}");
    }

    println!("starting two-stage ReAct loop");
    engine.run_session(&session, cli_input.prompt, options)?;

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CliInput {
    work_dir: PathBuf,
    plan_mode: PlanModeSetting,
    prompt: String,
}

fn cli_input_from_process() -> Result<CliInput, Box<dyn std::error::Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut stdin = io::stdin();
    let stdin_prompt = if stdin.is_terminal() {
        None
    } else {
        let mut input = String::new();
        stdin.read_to_string(&mut input)?;
        Some(input)
    };
    let default_work_dir = env::current_dir()?;

    parse_cli_input(args, stdin_prompt, default_work_dir)
        .map_err(|error| -> Box<dyn std::error::Error> { error.into() })
}

fn parse_cli_input(
    args: Vec<String>,
    stdin: Option<String>,
    default_work_dir: PathBuf,
) -> Result<CliInput, String> {
    let mut work_dir = default_work_dir;
    let mut plan_mode = PlanModeSetting::Auto;
    let mut prompt_parts = Vec::new();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--plan" => {
                plan_mode = PlanModeSetting::On;
            }
            "--plan-mode" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("--plan-mode requires on or auto".to_string());
                };
                plan_mode = parse_plan_mode(value)?;
            }
            "--workspace" | "-C" => {
                index += 1;
                let Some(path) = args.get(index) else {
                    return Err(format!("{} requires a path", args[index - 1]));
                };
                work_dir = PathBuf::from(path);
            }
            arg => prompt_parts.push(arg.to_string()),
        }

        index += 1;
    }

    let prompt = if !prompt_parts.is_empty() {
        prompt_parts.join(" ")
    } else if let Some(input) = stdin {
        let trimmed = input.trim();
        if !trimmed.is_empty() {
            trimmed.to_string()
        } else {
            SMOKE_PROMPT.to_string()
        }
    } else {
        SMOKE_PROMPT.to_string()
    };

    Ok(CliInput {
        work_dir,
        plan_mode,
        prompt,
    })
}

fn parse_plan_mode(value: &str) -> Result<PlanModeSetting, String> {
    match value {
        "on" | "ON" | "On" => Ok(PlanModeSetting::On),
        "auto" | "AUTO" | "Auto" => Ok(PlanModeSetting::Auto),
        _ => Err(format!(
            "invalid --plan-mode value: {value}; expected on or auto"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_args_become_prompt() {
        let input = parse_cli_input(
            vec!["inspect".to_string(), "skills".to_string()],
            None,
            PathBuf::from("/default"),
        )
        .unwrap();

        assert_eq!(input.work_dir, PathBuf::from("/default"));
        assert_eq!(input.plan_mode, PlanModeSetting::Auto);
        assert_eq!(input.prompt, "inspect skills");
    }

    #[test]
    fn piped_stdin_becomes_prompt_without_args() {
        let input = parse_cli_input(
            Vec::new(),
            Some("use rust skill\n".to_string()),
            PathBuf::from("/default"),
        )
        .unwrap();

        assert_eq!(input.prompt, "use rust skill");
        assert_eq!(input.plan_mode, PlanModeSetting::Auto);
    }

    #[test]
    fn no_input_falls_back_to_smoke_prompt() {
        let input = parse_cli_input(Vec::new(), None, PathBuf::from("/default")).unwrap();

        assert_eq!(input.prompt, SMOKE_PROMPT);
        assert_eq!(input.plan_mode, PlanModeSetting::Auto);
    }

    #[test]
    fn workspace_flag_sets_work_dir_without_becoming_prompt() {
        let input = parse_cli_input(
            vec![
                "--workspace".to_string(),
                "/tmp/project".to_string(),
                "inspect".to_string(),
            ],
            None,
            PathBuf::from("/default"),
        )
        .unwrap();

        assert_eq!(input.work_dir, PathBuf::from("/tmp/project"));
        assert_eq!(input.plan_mode, PlanModeSetting::Auto);
        assert_eq!(input.prompt, "inspect");
    }

    #[test]
    fn short_workspace_flag_sets_work_dir() {
        let input = parse_cli_input(
            vec![
                "-C".to_string(),
                "/tmp/project".to_string(),
                "inspect".to_string(),
            ],
            None,
            PathBuf::from("/default"),
        )
        .unwrap();

        assert_eq!(input.work_dir, PathBuf::from("/tmp/project"));
        assert_eq!(input.plan_mode, PlanModeSetting::Auto);
        assert_eq!(input.prompt, "inspect");
    }

    #[test]
    fn workspace_flag_requires_path() {
        let error = parse_cli_input(
            vec!["--workspace".to_string()],
            None,
            PathBuf::from("/default"),
        )
        .unwrap_err();

        assert_eq!(error, "--workspace requires a path");
    }

    #[test]
    fn plan_flag_enables_plan_mode_without_becoming_prompt() {
        let input = parse_cli_input(
            vec![
                "--plan".to_string(),
                "build".to_string(),
                "feature".to_string(),
            ],
            None,
            PathBuf::from("/default"),
        )
        .unwrap();

        assert_eq!(input.work_dir, PathBuf::from("/default"));
        assert_eq!(input.plan_mode, PlanModeSetting::On);
        assert_eq!(input.prompt, "build feature");
    }

    #[test]
    fn plan_mode_on_flag_enables_plan_mode_without_becoming_prompt() {
        let input = parse_cli_input(
            vec![
                "--plan-mode".to_string(),
                "on".to_string(),
                "-C".to_string(),
                "/tmp/project".to_string(),
                "continue".to_string(),
            ],
            None,
            PathBuf::from("/default"),
        )
        .unwrap();

        assert_eq!(input.work_dir, PathBuf::from("/tmp/project"));
        assert_eq!(input.plan_mode, PlanModeSetting::On);
        assert_eq!(input.prompt, "continue");
    }

    #[test]
    fn plan_mode_auto_flag_keeps_auto_without_becoming_prompt() {
        let input = parse_cli_input(
            vec![
                "--plan-mode".to_string(),
                "auto".to_string(),
                "inspect".to_string(),
            ],
            None,
            PathBuf::from("/default"),
        )
        .unwrap();

        assert_eq!(input.plan_mode, PlanModeSetting::Auto);
        assert_eq!(input.prompt, "inspect");
    }

    #[test]
    fn plan_mode_flag_requires_value() {
        let error = parse_cli_input(
            vec!["--plan-mode".to_string()],
            None,
            PathBuf::from("/default"),
        )
        .unwrap_err();

        assert_eq!(error, "--plan-mode requires on or auto");
    }
}
