use crate::schema::{ToolCall, ToolDefinition, ToolResult};
use crate::telemetry::trace::{
    TraceAttribute, TraceContext, TraceStatus, json_preview, text_preview,
};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

// Tools expose a model-facing definition and own the execution of their calls.
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema(&self) -> serde_json::Value;
    fn access_mode(&self, _call: &ToolCall) -> ToolAccessMode {
        ToolAccessMode::MutatesWorkspace
    }
    fn execute(&self, call: &ToolCall) -> ToolResult;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolAccessMode {
    ReadOnly,
    MutatesWorkspace,
}

#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: HashMap<&'static str, Arc<dyn Tool>>,
    middlewares: Vec<Arc<dyn ToolMiddleware>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<T>(&mut self, tool: T) -> Result<(), ToolRegistryError>
    where
        T: Tool + 'static,
    {
        let name = tool.name();
        if self.tools.contains_key(name) {
            return Err(ToolRegistryError::DuplicateTool { name });
        }

        self.tools.insert(name, Arc::new(tool));
        Ok(())
    }

    pub fn use_middleware<M>(&mut self, middleware: M)
    where
        M: ToolMiddleware + 'static,
    {
        self.middlewares.push(Arc::new(middleware));
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn names(&self) -> Vec<&'static str> {
        let mut names = self.tools.keys().copied().collect::<Vec<_>>();
        names.sort_unstable();
        names
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.names()
            .into_iter()
            .filter_map(|name| self.tools.get(name))
            .map(|tool| ToolDefinition::new(tool.name(), tool.description(), tool.input_schema()))
            .collect()
    }

    pub fn subset(&self, names: &[&'static str]) -> Result<Self, String> {
        let mut tools = HashMap::new();
        for name in names {
            let tool = self
                .tools
                .get(name)
                .ok_or_else(|| format!("tool profile references unknown tool: {name}"))?;
            tools.insert(*name, Arc::clone(tool));
        }

        Ok(Self {
            tools,
            middlewares: self.middlewares.clone(),
        })
    }

    pub fn execute(&self, call: &ToolCall) -> ToolResult {
        self.execute_with_trace(call, None)
    }

    pub fn execute_with_trace(
        &self,
        call: &ToolCall,
        trace_context: Option<&TraceContext>,
    ) -> ToolResult {
        let mut trace_span = trace_context.map(|context| {
            context.start_child(
                "Tool.Execute",
                vec![
                    TraceAttribute::new("tool.name", call.name.clone()),
                    TraceAttribute::new("tool.call_id", call.id.clone()),
                    TraceAttribute::new("tool.arguments.preview", json_preview(&call.arguments)),
                ],
            )
        });

        // Unknown tools are reported as observations instead of panicking the loop.
        let Some(tool) = self.tools.get(call.name.as_str()) else {
            if let Some(span) = &trace_span {
                span.add_attribute(TraceAttribute::new("tool.success", false));
                span.add_attribute(TraceAttribute::new("tool.unknown", true));
            }
            if let Some(span) = trace_span.take() {
                span.end_error(format!("tool '{}' is not registered", call.name));
            }
            return ToolResult::error(
                call.id.clone(),
                format!("tool '{}' is not registered", call.name),
            );
        };

        for middleware in &self.middlewares {
            if let Some(result) = middleware.before_execute(call) {
                if let Some(span) = &trace_span {
                    span.add_attributes([
                        TraceAttribute::new("tool.success", false),
                        TraceAttribute::new("tool.blocked", true),
                        TraceAttribute::new("tool.output.preview", text_preview(&result.output)),
                    ]);
                }
                if let Some(span) = trace_span.take() {
                    span.end_error("tool blocked by middleware");
                }
                // Policy rejections are not executed tool work, so they do not
                // receive timing context or after_execute callbacks.
                return result;
            }
        }

        let access_mode = tool.access_mode(call);
        if let Some(span) = &trace_span {
            span.add_attribute(TraceAttribute::new(
                "tool.access_mode",
                format!("{access_mode:?}"),
            ));
        }
        let start = Instant::now();
        let result = tool.execute(call);
        let context = ToolExecutionContext {
            elapsed: start.elapsed(),
            access_mode,
        };

        for middleware in &self.middlewares {
            middleware.after_execute(call, &result, &context);
        }

        if let Some(span) = &trace_span {
            span.add_attributes([
                TraceAttribute::new("tool.success", !result.is_error),
                TraceAttribute::new("tool.elapsed_ms", context.elapsed.as_millis() as u64),
                TraceAttribute::new("tool.output.preview", text_preview(&result.output)),
            ]);
        }
        if let Some(mut span) = trace_span {
            if result.is_error {
                span.end_with_status(TraceStatus::Error {
                    message: text_preview(&result.output),
                });
            } else {
                span.end_with_status(TraceStatus::Ok);
            }
        }

        result
    }

    pub fn is_read_only_call(&self, call: &ToolCall) -> bool {
        self.tools
            .get(call.name.as_str())
            .is_some_and(|tool| tool.access_mode(call) == ToolAccessMode::ReadOnly)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolExecutionContext {
    pub elapsed: Duration,
    pub access_mode: ToolAccessMode,
}

pub trait ToolMiddleware: Send + Sync {
    fn before_execute(&self, _call: &ToolCall) -> Option<ToolResult> {
        None
    }

    fn after_execute(
        &self,
        _call: &ToolCall,
        _result: &ToolResult,
        _context: &ToolExecutionContext,
    ) {
    }
}

impl<F> ToolMiddleware for F
where
    F: Fn(&ToolCall) -> Option<ToolResult> + Send + Sync,
{
    fn before_execute(&self, call: &ToolCall) -> Option<ToolResult> {
        self(call)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolRegistryError {
    DuplicateTool { name: &'static str },
}

impl fmt::Display for ToolRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateTool { name } => {
                write!(formatter, "tool '{name}' is already registered")
            }
        }
    }
}

impl std::error::Error for ToolRegistryError {}

#[cfg(test)]
mod tests {
    use super::{Tool, ToolExecutionContext, ToolMiddleware, ToolRegistry};
    use crate::schema::{ToolCall, ToolResult};
    use crate::tools::ReadFileTool;
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    #[derive(Debug, Default)]
    struct TestTool;

    impl Tool for TestTool {
        fn name(&self) -> &'static str {
            "test"
        }

        fn description(&self) -> &'static str {
            "Test-only tool."
        }

        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {}
            })
        }

        fn execute(&self, call: &ToolCall) -> ToolResult {
            ToolResult::ok(call.id.clone(), "ok")
        }
    }

    #[derive(Debug, Default)]
    struct ErrorTool;

    impl Tool for ErrorTool {
        fn name(&self) -> &'static str {
            "error"
        }

        fn description(&self) -> &'static str {
            "Test-only error tool."
        }

        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {}
            })
        }

        fn execute(&self, call: &ToolCall) -> ToolResult {
            ToolResult::error(call.id.clone(), "failed")
        }
    }

    #[derive(Clone)]
    struct AfterRecorder {
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl ToolMiddleware for AfterRecorder {
        fn after_execute(
            &self,
            call: &ToolCall,
            result: &ToolResult,
            context: &ToolExecutionContext,
        ) {
            self.calls.lock().unwrap().push(format!(
                "{}:{}:{:?}",
                call.id, result.is_error, context.access_mode
            ));
        }
    }

    #[test]
    fn registry_rejects_duplicate_tool_names() {
        let mut registry = ToolRegistry::new();

        registry.register(TestTool).unwrap();
        let error = registry.register(TestTool).unwrap_err();

        assert_eq!(error.to_string(), "tool 'test' is already registered");
    }

    #[test]
    fn registry_returns_definitions_in_stable_name_order() {
        let work_dir = tempdir().unwrap();

        let mut registry = ToolRegistry::new();
        registry
            .register(ReadFileTool::new(work_dir.path()).unwrap())
            .unwrap();
        registry.register(TestTool).unwrap();

        let names = registry
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["read_file", "test"]);
    }

    #[test]
    fn middleware_can_reject_before_tool_execution() {
        let mut registry = ToolRegistry::new();
        registry.register(TestTool).unwrap();
        registry.use_middleware(|call: &ToolCall| {
            Some(ToolResult::error(call.id.clone(), "blocked by middleware"))
        });

        let result = registry.execute(&ToolCall::new("call_1", "test", serde_json::json!({})));

        assert!(result.is_error);
        assert_eq!(result.output, "blocked by middleware");
    }

    #[test]
    fn middleware_is_not_called_for_unknown_tools() {
        let mut registry = ToolRegistry::new();
        registry.use_middleware(|call: &ToolCall| {
            Some(ToolResult::error(call.id.clone(), "blocked by middleware"))
        });

        let result = registry.execute(&ToolCall::new("call_1", "missing", serde_json::json!({})));

        assert!(result.is_error);
        assert_eq!(result.output, "tool 'missing' is not registered");
    }

    #[test]
    fn after_middleware_runs_after_successful_tool_execution() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut registry = ToolRegistry::new();
        registry.register(TestTool).unwrap();
        registry.use_middleware(AfterRecorder {
            calls: Arc::clone(&calls),
        });

        let result = registry.execute(&ToolCall::new("call_1", "test", serde_json::json!({})));

        assert!(!result.is_error);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            ["call_1:false:MutatesWorkspace"]
        );
    }

    #[test]
    fn after_middleware_runs_after_tool_error_result() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut registry = ToolRegistry::new();
        registry.register(ErrorTool).unwrap();
        registry.use_middleware(AfterRecorder {
            calls: Arc::clone(&calls),
        });

        let result = registry.execute(&ToolCall::new("call_1", "error", serde_json::json!({})));

        assert!(result.is_error);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            ["call_1:true:MutatesWorkspace"]
        );
    }

    #[test]
    fn after_middleware_does_not_run_when_before_middleware_rejects() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut registry = ToolRegistry::new();
        registry.register(TestTool).unwrap();
        registry.use_middleware(|call: &ToolCall| {
            Some(ToolResult::error(call.id.clone(), "blocked by middleware"))
        });
        registry.use_middleware(AfterRecorder {
            calls: Arc::clone(&calls),
        });

        let result = registry.execute(&ToolCall::new("call_1", "test", serde_json::json!({})));

        assert!(result.is_error);
        assert!(calls.lock().unwrap().is_empty());
    }
}
