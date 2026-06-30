use crate::provider::{Provider, ProviderError, StreamSink};
use crate::schema::{Message, ToolDefinition};
use crate::telemetry::metrics::{LlmCallRecord, Telemetry};
use std::time::Instant;

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

#[cfg(test)]
mod tests {
    use super::TelemetryProvider;
    use crate::provider::{Provider, ProviderError, StreamSink};
    use crate::schema::{Message, ToolDefinition, Usage};
    use crate::telemetry::Telemetry;

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
