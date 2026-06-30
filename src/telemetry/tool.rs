use crate::schema::{ToolCall, ToolResult};
use crate::telemetry::metrics::{Telemetry, ToolCallRecord};
use crate::tools::{ToolExecutionContext, ToolMiddleware};

#[derive(Clone)]
pub struct TelemetryToolMiddleware {
    telemetry: Telemetry,
}

impl TelemetryToolMiddleware {
    pub fn new(telemetry: Telemetry) -> Self {
        Self { telemetry }
    }
}

impl ToolMiddleware for TelemetryToolMiddleware {
    fn after_execute(&self, call: &ToolCall, result: &ToolResult, context: &ToolExecutionContext) {
        self.telemetry.record_tool_call(ToolCallRecord {
            tool_name: call.name.clone(),
            tool_call_id: call.id.clone(),
            access_mode: context.access_mode,
            elapsed_ms: context.elapsed.as_millis(),
            success: !result.is_error,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::TelemetryToolMiddleware;
    use crate::schema::{ToolCall, ToolResult};
    use crate::telemetry::Telemetry;
    use crate::tools::{ToolAccessMode, ToolExecutionContext, ToolMiddleware};
    use std::time::Duration;

    #[test]
    fn tool_middleware_records_tool_call() {
        let telemetry = Telemetry::default();
        let middleware = TelemetryToolMiddleware::new(telemetry.clone());
        let call = ToolCall::new("call_1", "read_file", serde_json::json!({}));
        let result = ToolResult::ok("call_1", "ok");
        let context = ToolExecutionContext {
            elapsed: Duration::from_millis(12),
            access_mode: ToolAccessMode::ReadOnly,
        };

        middleware.after_execute(&call, &result, &context);

        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.tools.call_count, 1);
        assert_eq!(snapshot.tools.failed_call_count, 0);
        assert_eq!(snapshot.tools.elapsed_ms, 12);
    }
}
