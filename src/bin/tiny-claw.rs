use rust_tiny_claw::app::{build_engine, stream_enabled};
use rust_tiny_claw::engine::RunOptions;
use std::env;
use std::io::{self, IsTerminal, Read};

const SMOKE_PROMPT: &str = "Smoke-test the lesson 8 harness. Create .tiny-claw/smoke/edit-target.rs with an indented TODO auth block. Read it once. Then call edit_file exactly once to replace that block with a Forbidden return; in old_text, omit the original indentation so the fuzzy indentation fallback is exercised. Read the file once more to confirm the replacement. Do not repeat the edit flow after it succeeds. Finally, read Cargo.toml, README.md, and src/bin/tiny-claw.rs and call grep for TODO in one independent batch so the engine can execute multiple read-only tool calls in parallel.";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();

    println!("rust-tiny-claw engine boot sequence");

    let work_dir = env::current_dir()?;
    let mut engine = build_engine(&work_dir)?;

    let options = RunOptions {
        max_turns: 12,
        enable_thinking: false,
        stream: stream_enabled()?,
    };

    for line in engine.boot_plan(options) {
        println!("- {line}");
    }

    println!("starting two-stage ReAct loop");
    engine.run_with_options(prompt_from_cli()?, options)?;

    Ok(())
}

fn prompt_from_cli() -> Result<String, Box<dyn std::error::Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut stdin = io::stdin();

    if stdin.is_terminal() {
        return Ok(prompt_from_inputs(args, None));
    }

    let mut input = String::new();
    stdin.read_to_string(&mut input)?;
    Ok(prompt_from_inputs(args, Some(input)))
}

fn prompt_from_inputs(args: Vec<String>, stdin: Option<String>) -> String {
    if !args.is_empty() {
        return args.join(" ");
    }

    if let Some(input) = stdin {
        let trimmed = input.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    SMOKE_PROMPT.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_args_become_prompt() {
        let prompt = prompt_from_inputs(vec!["inspect".to_string(), "skills".to_string()], None);

        assert_eq!(prompt, "inspect skills");
    }

    #[test]
    fn piped_stdin_becomes_prompt_without_args() {
        let prompt = prompt_from_inputs(Vec::new(), Some("use rust skill\n".to_string()));

        assert_eq!(prompt, "use rust skill");
    }

    #[test]
    fn no_input_falls_back_to_smoke_prompt() {
        let prompt = prompt_from_inputs(Vec::new(), None);

        assert_eq!(prompt, SMOKE_PROMPT);
    }
}
