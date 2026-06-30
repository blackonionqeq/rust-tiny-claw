use crate::provider::{Provider, ProviderError, StreamSink};
use crate::schema::{Message, ToolCall, ToolDefinition, ToolResult, Usage};
use crate::tools::{ToolAccessMode, ToolExecutionContext, ToolMiddleware};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

#[derive(Debug, Clone, Default)]
pub struct Telemetry {
    inner: Arc<TelemetryInner>,
}

#[derive(Debug, Default)]
struct TelemetryInner {
    prompt_tokens: AtomicU64,
    completion_tokens: AtomicU64,
    total_tokens: AtomicU64,
    llm_call_count: AtomicU64,
    llm_failed_call_count: AtomicU64,
    llm_elapsed_ms: AtomicUsize,
    tool_call_count: AtomicU64,
    tool_failed_call_count: AtomicU64,
    tool_elapsed_ms: AtomicUsize,
}

impl Telemetry {
    pub fn name(&self) -> &'static str {
        "telemetry"
    }

    pub fn record_llm_call(&self, record: LlmCallRecord) {
        self.inner.llm_call_count.fetch_add(1, Ordering::Relaxed);
        if !record.success {
            self.inner
                .llm_failed_call_count
                .fetch_add(1, Ordering::Relaxed);
        }
        self.inner
            .llm_elapsed_ms
            .fetch_add(record.elapsed_ms as usize, Ordering::Relaxed);

        if let Some(usage) = record.usage {
            self.inner
                .prompt_tokens
                .fetch_add(usage.prompt_tokens, Ordering::Relaxed);
            self.inner
                .completion_tokens
                .fetch_add(usage.completion_tokens, Ordering::Relaxed);
            self.inner
                .total_tokens
                .fetch_add(usage.total_tokens, Ordering::Relaxed);
        }
    }

    pub fn record_tool_call(&self, record: ToolCallRecord) {
        self.inner.tool_call_count.fetch_add(1, Ordering::Relaxed);
        if !record.success {
            self.inner
                .tool_failed_call_count
                .fetch_add(1, Ordering::Relaxed);
        }
        self.inner
            .tool_elapsed_ms
            .fetch_add(record.elapsed_ms as usize, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> TelemetrySnapshot {
        TelemetrySnapshot {
            llm: LlmTotals {
                prompt_tokens: self.inner.prompt_tokens.load(Ordering::Relaxed),
                completion_tokens: self.inner.completion_tokens.load(Ordering::Relaxed),
                total_tokens: self.inner.total_tokens.load(Ordering::Relaxed),
                call_count: self.inner.llm_call_count.load(Ordering::Relaxed),
                failed_call_count: self.inner.llm_failed_call_count.load(Ordering::Relaxed),
                elapsed_ms: self.inner.llm_elapsed_ms.load(Ordering::Relaxed) as u128,
            },
            tools: ToolTotals {
                call_count: self.inner.tool_call_count.load(Ordering::Relaxed),
                failed_call_count: self.inner.tool_failed_call_count.load(Ordering::Relaxed),
                elapsed_ms: self.inner.tool_elapsed_ms.load(Ordering::Relaxed) as u128,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmCallRecord {
    pub provider: &'static str,
    pub model: Option<String>,
    pub stream: bool,
    pub elapsed_ms: u128,
    pub usage: Option<Usage>,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LlmTotals {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub call_count: u64,
    pub failed_call_count: u64,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolTotals {
    pub call_count: u64,
    pub failed_call_count: u64,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TelemetrySnapshot {
    pub llm: LlmTotals,
    pub tools: ToolTotals,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallRecord {
    pub tool_name: String,
    pub tool_call_id: String,
    pub access_mode: ToolAccessMode,
    pub elapsed_ms: u128,
    pub success: bool,
}

pub struct TelemetryProvider<P> {
    inner: P,
    telemetry: Telemetry,
}

impl<P> TelemetryProvider<P> {
    pub fn new(inner: P, telemetry: Telemetry) -> Self {
        Self { inner, telemetry }
    }
}

impl<P> Provider for TelemetryProvider<P>
where
    P: Provider,
{
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn generate(
        &mut self,
        messages: &[Message],
        available_tools: Option<&[ToolDefinition]>,
    ) -> Result<Message, ProviderError> {
        let provider = self.inner.name();
        let start = Instant::now();
        let result = self.inner.generate(messages, available_tools);
        let elapsed_ms = start.elapsed().as_millis();

        // Keep the wrapper transparent: record metadata after the call, then
        // return the provider result unchanged.
        self.telemetry.record_llm_call(match &result {
            Ok(message) => LlmCallRecord {
                provider,
                model: None,
                stream: false,
                elapsed_ms,
                usage: message.usage,
                success: true,
                error: None,
            },
            Err(error) => LlmCallRecord {
                provider,
                model: None,
                stream: false,
                elapsed_ms,
                usage: None,
                success: false,
                error: Some(error.to_string()),
            },
        });

        result
    }

    fn generate_stream(
        &mut self,
        messages: &[Message],
        available_tools: Option<&[ToolDefinition]>,
        sink: &mut dyn StreamSink,
    ) -> Result<Message, ProviderError> {
        let provider = self.inner.name();
        let start = Instant::now();
        let result = self.inner.generate_stream(messages, available_tools, sink);
        let elapsed_ms = start.elapsed().as_millis();

        // Stream telemetry follows the same transparent boundary as generate.
        self.telemetry.record_llm_call(match &result {
            Ok(message) => LlmCallRecord {
                provider,
                model: None,
                stream: true,
                elapsed_ms,
                usage: message.usage,
                success: true,
                error: None,
            },
            Err(error) => LlmCallRecord {
                provider,
                model: None,
                stream: true,
                elapsed_ms,
                usage: None,
                success: false,
                error: Some(error.to_string()),
            },
        });

        result
    }
}

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
    use super::{Telemetry, TelemetryProvider, TelemetryToolMiddleware};
    use crate::provider::{Provider, ProviderError, StreamSink};
    use crate::schema::{Message, ToolCall, ToolDefinition, ToolResult, Usage};
    use crate::tools::{ToolAccessMode, ToolExecutionContext, ToolMiddleware};
    use std::time::Duration;

    struct UsageProvider;

    impl Provider for UsageProvider {
        fn name(&self) -> &'static str {
            "usage-provider"
        }

        fn generate(
            &mut self,
            _messages: &[Message],
            _available_tools: Option<&[ToolDefinition]>,
        ) -> Result<Message, ProviderError> {
            Ok(Message::assistant("ok").with_usage(Usage {
                prompt_tokens: 2,
                completion_tokens: 3,
                total_tokens: 5,
            }))
        }
    }

    struct FailingProvider;

    impl Provider for FailingProvider {
        fn name(&self) -> &'static str {
            "failing-provider"
        }

        fn generate(
            &mut self,
            _messages: &[Message],
            _available_tools: Option<&[ToolDefinition]>,
        ) -> Result<Message, ProviderError> {
            Err(ProviderError::new("boom"))
        }
    }

    struct StreamProvider;

    impl Provider for StreamProvider {
        fn name(&self) -> &'static str {
            "stream-provider"
        }

        fn generate(
            &mut self,
            _messages: &[Message],
            _available_tools: Option<&[ToolDefinition]>,
        ) -> Result<Message, ProviderError> {
            Ok(Message::assistant("unused"))
        }

        fn generate_stream(
            &mut self,
            _messages: &[Message],
            _available_tools: Option<&[ToolDefinition]>,
            sink: &mut dyn StreamSink,
        ) -> Result<Message, ProviderError> {
            sink.on_text("ok")?;
            Ok(Message::assistant("ok"))
        }
    }

    #[test]
    fn message_constructors_default_usage_to_none() {
        assert_eq!(Message::system("system").usage, None);
        assert_eq!(Message::user("user").usage, None);
        assert_eq!(Message::assistant("assistant").usage, None);
        assert_eq!(Message::observation("call_1", "observation").usage, None);
    }

    #[test]
    fn provider_wrapper_records_success_and_usage() {
        let telemetry = Telemetry::default();
        let mut provider = TelemetryProvider::new(UsageProvider, telemetry.clone());

        provider.generate(&[], None).unwrap();

        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.llm.call_count, 1);
        assert_eq!(snapshot.llm.failed_call_count, 0);
        assert_eq!(snapshot.llm.prompt_tokens, 2);
        assert_eq!(snapshot.llm.completion_tokens, 3);
        assert_eq!(snapshot.llm.total_tokens, 5);
    }

    #[test]
    fn provider_wrapper_records_failed_call_without_usage() {
        let telemetry = Telemetry::default();
        let mut provider = TelemetryProvider::new(FailingProvider, telemetry.clone());

        let error = provider.generate(&[], None).unwrap_err();

        assert_eq!(error.to_string(), "boom");
        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.llm.call_count, 1);
        assert_eq!(snapshot.llm.failed_call_count, 1);
        assert_eq!(snapshot.llm.total_tokens, 0);
    }

    #[test]
    fn provider_wrapper_records_stream_call() {
        let telemetry = Telemetry::default();
        let mut provider = TelemetryProvider::new(StreamProvider, telemetry.clone());
        let mut sink = TestSink::default();

        provider.generate_stream(&[], None, &mut sink).unwrap();

        assert_eq!(sink.output, "ok");
        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.llm.call_count, 1);
        assert_eq!(snapshot.llm.failed_call_count, 0);
    }

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

    #[derive(Default)]
    struct TestSink {
        output: String,
    }

    impl StreamSink for TestSink {
        fn on_text(&mut self, text: &str) -> Result<(), ProviderError> {
            self.output.push_str(text);
            Ok(())
        }
    }
}
