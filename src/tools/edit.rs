use crate::schema::{ToolCall, ToolResult};
use crate::tools::Tool;
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct EditFileTool {
    work_dir: PathBuf,
}

impl EditFileTool {
    pub fn new(work_dir: impl Into<PathBuf>) -> Result<Self, std::io::Error> {
        let work_dir = work_dir.into().canonicalize()?;
        Ok(Self { work_dir })
    }

    fn resolve_path_for_edit(&self, path: &str) -> Result<PathBuf, String> {
        let requested = Path::new(path);
        if requested.is_absolute() {
            return Err("path must be relative to the workspace".to_string());
        }

        let full_path = self.work_dir.join(requested);
        let resolved = full_path
            .canonicalize()
            .map_err(|error| format!("failed to resolve path '{path}': {error}"))?;

        if !resolved.starts_with(&self.work_dir) {
            return Err(format!("path '{path}' is outside the workspace"));
        }

        if !resolved.is_file() {
            return Err(format!("path '{path}' must name an existing file"));
        }

        Ok(resolved)
    }
}

impl Tool for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn description(&self) -> &'static str {
        "Replace one existing text block in a workspace file. Provide enough old_text context to match exactly one location."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Workspace-relative path to an existing file, such as src/main.rs."
                },
                "old_text": {
                    "type": "string",
                    "description": "Existing text to replace. Include enough surrounding lines to make the match unique."
                },
                "new_text": {
                    "type": "string",
                    "description": "Replacement text."
                }
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    fn execute(&self, call: &ToolCall) -> ToolResult {
        let Some(path) = call.arguments.get("path").and_then(|value| value.as_str()) else {
            return ToolResult::error(call.id.clone(), "missing string argument: path");
        };
        let Some(old_text) = call
            .arguments
            .get("old_text")
            .and_then(|value| value.as_str())
        else {
            return ToolResult::error(call.id.clone(), "missing string argument: old_text");
        };
        let Some(new_text) = call
            .arguments
            .get("new_text")
            .and_then(|value| value.as_str())
        else {
            return ToolResult::error(call.id.clone(), "missing string argument: new_text");
        };

        let resolved = match self.resolve_path_for_edit(path) {
            Ok(path) => path,
            Err(error) => return ToolResult::error(call.id.clone(), error),
        };

        let original = match std::fs::read_to_string(&resolved) {
            Ok(content) => content,
            Err(error) => {
                return ToolResult::error(
                    call.id.clone(),
                    format!("failed to read file '{}': {error}", resolved.display()),
                );
            }
        };

        let edited = match fuzzy_replace(&original, old_text, new_text) {
            Ok(content) => content,
            Err(error) => return ToolResult::error(call.id.clone(), error),
        };

        match std::fs::write(&resolved, edited) {
            Ok(()) => ToolResult::ok(call.id.clone(), format!("edited file: {path}")),
            Err(error) => ToolResult::error(
                call.id.clone(),
                format!("failed to write file '{}': {error}", resolved.display()),
            ),
        }
    }
}

fn fuzzy_replace(original: &str, old_text: &str, new_text: &str) -> Result<String, String> {
    // Keep the fallback chain conservative: every level must find exactly one
    // match, otherwise the model needs to provide more context.
    match count_matches(original, old_text) {
        1 => return Ok(original.replacen(old_text, new_text, 1)),
        count if count > 1 => {
            return Err(format!(
                "old_text matched {count} locations; provide more surrounding context"
            ));
        }
        _ => {}
    }

    // Normalize for matching, then restore the file's original newline style
    // before writing back.
    let newline_style = NewlineStyle::detect(original);
    let normalized_content = normalize_newlines(original);
    let normalized_old = normalize_newlines(old_text);
    let normalized_new = normalize_newlines(new_text);

    match count_matches(&normalized_content, &normalized_old) {
        1 => {
            let replaced = normalized_content.replacen(&normalized_old, &normalized_new, 1);
            return Ok(newline_style.apply(&replaced));
        }
        count if count > 1 => {
            return Err(format!(
                "old_text matched {count} locations after normalizing newlines; provide more surrounding context"
            ));
        }
        _ => {}
    }

    let trimmed_old = normalized_old.trim();
    if !trimmed_old.is_empty() {
        match count_matches(&normalized_content, trimmed_old) {
            1 => {
                let replaced = normalized_content.replacen(trimmed_old, &normalized_new, 1);
                return Ok(newline_style.apply(&replaced));
            }
            count if count > 1 => {
                return Err(format!(
                    "old_text matched {count} locations after trimming surrounding whitespace; provide more surrounding context"
                ));
            }
            _ => {}
        }
    }

    line_by_line_replace(&normalized_content, &normalized_old, &normalized_new)
        .map(|content| newline_style.apply(&content))
}

fn line_by_line_replace(content: &str, old_text: &str, new_text: &str) -> Result<String, String> {
    let content_lines = content.split('\n').collect::<Vec<_>>();
    let old_lines = old_text
        .trim()
        .split('\n')
        .map(str::trim)
        .collect::<Vec<_>>();

    if old_lines.is_empty() || old_lines.iter().all(|line| line.is_empty()) {
        return Err("old_text must contain non-whitespace text".to_string());
    }

    if content_lines.len() < old_lines.len() {
        return Err("old_text was not found in the file".to_string());
    }

    let mut match_count = 0;
    let mut match_start = 0;

    for start in 0..=content_lines.len() - old_lines.len() {
        let matched = old_lines
            .iter()
            .enumerate()
            .all(|(offset, old_line)| content_lines[start + offset].trim() == *old_line);

        if matched {
            match_count += 1;
            match_start = start;
        }
    }

    match match_count {
        0 => Err("old_text was not found in the file".to_string()),
        count if count > 1 => Err(format!(
            "old_text matched {count} similar locations after ignoring indentation; provide more surrounding context"
        )),
        _ => {
            let match_end = match_start + old_lines.len();
            // L4 ignores indentation to locate the block, but replacement
            // still inherits the matched block's base indent.
            let base_indent = base_indentation(&content_lines[match_start..match_end]);
            let replacement = reindent_replacement(new_text, base_indent);

            let mut lines =
                Vec::with_capacity(content_lines.len() - old_lines.len() + replacement.len());
            lines.extend(
                content_lines[..match_start]
                    .iter()
                    .map(|line| (*line).to_string()),
            );
            lines.extend(replacement);
            lines.extend(
                content_lines[match_end..]
                    .iter()
                    .map(|line| (*line).to_string()),
            );

            Ok(lines.join("\n"))
        }
    }
}

fn base_indentation<'a>(lines: &'a [&str]) -> &'a str {
    lines
        .iter()
        .find(|line| !line.trim().is_empty())
        .map(|line| indentation_prefix(line))
        .unwrap_or("")
}

fn reindent_replacement<'a>(new_text: &'a str, base_indent: &'a str) -> Vec<String> {
    let lines = new_text.split('\n').collect::<Vec<_>>();
    let min_indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| indentation_prefix(line).len())
        .min()
        .unwrap_or(0);

    lines
        .into_iter()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("{base_indent}{}", &line[min_indent..])
            }
        })
        .collect()
}

fn indentation_prefix(line: &str) -> &str {
    let end = line
        .char_indices()
        .find_map(|(index, character)| (!character.is_whitespace()).then_some(index))
        .unwrap_or(line.len());
    &line[..end]
}

fn count_matches(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }

    haystack.matches(needle).count()
}

fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NewlineStyle {
    Lf,
    Crlf,
}

impl NewlineStyle {
    fn detect(text: &str) -> Self {
        let crlf_count = text.matches("\r\n").count();
        let lf_count = text.matches('\n').count();

        if crlf_count > 0 && crlf_count == lf_count {
            Self::Crlf
        } else {
            Self::Lf
        }
    }

    fn apply(self, text: &str) -> String {
        match self {
            Self::Lf => text.to_string(),
            Self::Crlf => text.replace('\n', "\r\n"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EditFileTool, fuzzy_replace};
    use crate::schema::ToolCall;
    use crate::tools::Tool;
    use serde_json::json;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn fuzzy_replace_uses_exact_match_first() {
        let result = fuzzy_replace("one\ntwo\nthree\n", "two", "2").unwrap();
        assert_eq!(result, "one\n2\nthree\n");
    }

    #[test]
    fn fuzzy_replace_rejects_repeated_exact_match() {
        let error = fuzzy_replace("same\nsame\n", "same", "new").unwrap_err();
        assert!(error.contains("matched 2 locations"));
    }

    #[test]
    fn fuzzy_replace_normalizes_newlines_without_changing_file_style() {
        let result = fuzzy_replace("one\r\ntwo\r\n", "one\ntwo\n", "uno\ndos\n").unwrap();
        assert_eq!(result, "uno\r\ndos\r\n");
    }

    #[test]
    fn fuzzy_replace_ignores_surrounding_whitespace() {
        let result =
            fuzzy_replace("before\nalpha\nbeta\nafter\n", " \nalpha\nbeta\n ", "x\ny").unwrap();
        assert_eq!(result, "before\nx\ny\nafter\n");
    }

    #[test]
    fn fuzzy_replace_ignores_indentation_and_preserves_base_indent() {
        let result = fuzzy_replace(
            "fn main() {\n        if user == nil {\n            return err\n        }\n}\n",
            "if user == nil {\n    return err\n}",
            "if user.is_none() {\n    return Err(error);\n}",
        )
        .unwrap();

        assert_eq!(
            result,
            "fn main() {\n        if user.is_none() {\n            return Err(error);\n        }\n}\n"
        );
    }

    #[test]
    fn fuzzy_replace_rejects_repeated_indentation_insensitive_match() {
        let error = fuzzy_replace(
            "    if ready {\n        run();\n    }\n    if ready {\n        run();\n    }\n",
            "if ready {\nrun();\n}",
            "run_once();",
        )
        .unwrap_err();

        assert!(error.contains("similar locations"));
    }

    #[test]
    fn edit_file_edits_workspace_file() {
        let work_dir = tempdir().unwrap();
        fs::create_dir_all(work_dir.path().join("src")).unwrap();
        fs::write(
            work_dir.path().join("src/main.rs"),
            "fn main() {\n    println!(\"old\");\n}\n",
        )
        .unwrap();

        let tool = EditFileTool::new(work_dir.path()).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "edit_file",
            json!({
                "path": "src/main.rs",
                "old_text": "println!(\"old\");",
                "new_text": "println!(\"new\");"
            }),
        ));
        let edited = fs::read_to_string(work_dir.path().join("src/main.rs")).unwrap();

        assert!(!result.is_error);
        assert_eq!(edited, "fn main() {\n    println!(\"new\");\n}\n");
    }

    #[test]
    fn edit_file_rejects_parent_directory_escape() {
        let work_dir = tempdir().unwrap();

        let tool = EditFileTool::new(work_dir.path()).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "edit_file",
            json!({
                "path": "../outside.txt",
                "old_text": "old",
                "new_text": "new"
            }),
        ));

        assert!(result.is_error);
        assert!(
            result.output.contains("outside the workspace")
                || result.output.contains("failed to resolve")
        );
    }

    #[test]
    fn edit_file_rejects_missing_arguments() {
        let work_dir = tempdir().unwrap();

        let tool = EditFileTool::new(work_dir.path()).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "edit_file",
            json!({ "path": "file.txt", "old_text": "old" }),
        ));

        assert!(result.is_error);
        assert!(result.output.contains("new_text"));
    }
}
