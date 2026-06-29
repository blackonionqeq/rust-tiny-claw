#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryCode {
    EditTextNotFound,
    EditTextAmbiguous,
    FileNotFound,
    PermissionDenied,
    MissingArgument,
    UnknownTool,
    UnknownToolError,
}

impl RecoveryCode {
    fn as_str(self) -> &'static str {
        match self {
            Self::EditTextNotFound => "EDIT_TEXT_NOT_FOUND",
            Self::EditTextAmbiguous => "EDIT_TEXT_AMBIGUOUS",
            Self::FileNotFound => "FILE_NOT_FOUND",
            Self::PermissionDenied => "PERMISSION_DENIED",
            Self::MissingArgument => "MISSING_ARGUMENT",
            Self::UnknownTool => "UNKNOWN_TOOL",
            Self::UnknownToolError => "UNKNOWN_TOOL_ERROR",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryAdvice {
    pub code: RecoveryCode,
    pub recoverable: bool,
    pub guidance: &'static str,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryManager;

impl RecoveryManager {
    pub fn new() -> Self {
        Self
    }

    pub fn analyze(&self, tool_name: &str, raw_error: &str) -> RecoveryAdvice {
        let lower_error = raw_error.to_ascii_lowercase();

        if lower_error.contains("missing string argument") {
            return RecoveryAdvice {
                code: RecoveryCode::MissingArgument,
                recoverable: true,
                guidance: "Fix the tool arguments so they match the tool schema before retrying.",
            };
        }

        if lower_error.contains("is not registered") {
            return RecoveryAdvice {
                code: RecoveryCode::UnknownTool,
                recoverable: true,
                guidance: "Choose one of the registered tools instead of retrying this tool name.",
            };
        }

        match tool_name {
            "edit_file" => analyze_edit_file_error(&lower_error),
            "read_file" | "write_file" => analyze_file_error(&lower_error),
            _ => None,
        }
        .unwrap_or(RecoveryAdvice {
            code: RecoveryCode::UnknownToolError,
            recoverable: true,
            guidance: "Read the raw error carefully, avoid repeating the same call unchanged, and choose the next tool call from the current context.",
        })
    }

    // Recovery keeps the raw error as the source of truth and adds only one
    // short next-step hint. Repeated-error detection belongs to the later
    // system-reminder layer, not this single-observation formatter.
    pub fn render_tool_error(&self, tool_name: &str, raw_error: &str) -> String {
        let advice = self.analyze(tool_name, raw_error);

        format!(
            "Tool call failed.\n\n\
             tool: {tool_name}\n\
             error_code: {}\n\
             recoverable: {}\n\n\
             Raw error:\n\
             ```text\n\
             {raw_error}\n\
             ```\n\n\
             Recovery guidance:\n\
             - {}",
            advice.code.as_str(),
            advice.recoverable,
            advice.guidance
        )
    }
}

fn analyze_edit_file_error(lower_error: &str) -> Option<RecoveryAdvice> {
    if lower_error.contains("old_text was not found")
        || lower_error.contains("old_text")
            && (lower_error.contains("not found") || lower_error.contains("not match"))
    {
        return Some(RecoveryAdvice {
            code: RecoveryCode::EditTextNotFound,
            recoverable: true,
            guidance: "Read the target file again, then retry with old_text copied from the latest file contents.",
        });
    }

    if lower_error.contains("matched")
        && (lower_error.contains("locations") || lower_error.contains("similar locations"))
    {
        return Some(RecoveryAdvice {
            code: RecoveryCode::EditTextAmbiguous,
            recoverable: true,
            guidance: "Retry with more surrounding old_text context so the edit matches exactly one location.",
        });
    }

    analyze_file_error(lower_error)
}

fn analyze_file_error(lower_error: &str) -> Option<RecoveryAdvice> {
    if lower_error.contains("no such file or directory")
        || lower_error.contains("failed to resolve path")
        || lower_error.contains("must name an existing file")
    {
        return Some(RecoveryAdvice {
            code: RecoveryCode::FileNotFound,
            recoverable: true,
            guidance: "Inspect the workspace paths before retrying; use read-only exploration such as listing files or grep to find the correct path.",
        });
    }

    if lower_error.contains("permission denied") {
        return Some(RecoveryAdvice {
            code: RecoveryCode::PermissionDenied,
            recoverable: false,
            guidance: "Do not try to bypass workspace permissions. Pick another valid workspace path or report the permission issue.",
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{RecoveryCode, RecoveryManager};

    #[test]
    fn edit_file_not_found_keeps_raw_error_and_suggests_reading_file() {
        let manager = RecoveryManager::new();
        let output = manager.render_tool_error("edit_file", "old_text was not found in the file");

        assert!(output.contains("error_code: EDIT_TEXT_NOT_FOUND"));
        assert!(output.contains("old_text was not found in the file"));
        assert!(output.contains("Read the target file again"));
    }

    #[test]
    fn edit_file_ambiguous_match_suggests_more_context() {
        let manager = RecoveryManager::new();
        let advice = manager.analyze(
            "edit_file",
            "old_text matched 2 similar locations after ignoring indentation",
        );

        assert_eq!(advice.code, RecoveryCode::EditTextAmbiguous);
        assert!(
            advice
                .guidance
                .contains("more surrounding old_text context")
        );
    }

    #[test]
    fn missing_read_path_maps_to_file_not_found() {
        let manager = RecoveryManager::new();
        let advice = manager.analyze(
            "read_file",
            "failed to resolve path 'src/missing.rs': No such file or directory",
        );

        assert_eq!(advice.code, RecoveryCode::FileNotFound);
        assert!(advice.guidance.contains("Inspect the workspace paths"));
    }

    #[test]
    fn unknown_error_stays_generic_but_preserves_raw_error() {
        let manager = RecoveryManager::new();
        let output = manager.render_tool_error("custom", "multi\nline\nfailure");

        assert!(output.contains("error_code: UNKNOWN_TOOL_ERROR"));
        assert!(output.contains("multi\nline\nfailure"));
        assert!(output.contains("avoid repeating the same call unchanged"));
    }
}
