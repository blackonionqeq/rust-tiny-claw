use rust_tiny_claw::provider::{
    ClaudeCompatibleProvider, OpenAiCompatibleProvider, Provider, ProviderError, StreamSink,
};
use rust_tiny_claw::schema::Message;
use rust_tiny_claw::telemetry::{Telemetry, TelemetryProvider};
use std::env;

#[test]
#[ignore = "requires real provider credentials and network access"]
fn real_provider_reports_non_stream_usage() -> Result<(), Box<dyn std::error::Error>> {
    let Some(provider) = real_provider()? else {
        eprintln!(
            "skipping real telemetry smoke: set TINY_CLAW_PROVIDER to a non-mock provider and TINY_CLAW_API_KEY"
        );
        return Ok(());
    };
    let telemetry = Telemetry::default();
    let mut provider = TelemetryProvider::new(provider, telemetry.clone());

    let message = provider.generate(&telemetry_prompt(), None)?;

    assert_expected_response(&message.content);
    assert!(
        message.usage.is_some(),
        "expected real provider to report non-stream usage for response: {:?}",
        message.content
    );
    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot.llm.call_count, 1);
    assert_eq!(snapshot.llm.failed_call_count, 0);
    assert!(snapshot.llm.total_tokens > 0);

    Ok(())
}

#[test]
#[ignore = "requires real provider credentials and network access"]
fn real_provider_reports_stream_usage() -> Result<(), Box<dyn std::error::Error>> {
    let Some(provider) = real_provider()? else {
        eprintln!(
            "skipping real telemetry stream smoke: set TINY_CLAW_PROVIDER to a non-mock provider and TINY_CLAW_API_KEY"
        );
        return Ok(());
    };
    let telemetry = Telemetry::default();
    let mut provider = TelemetryProvider::new(provider, telemetry.clone());
    let mut sink = CapturingSink::default();

    let message = provider.generate_stream(&telemetry_prompt(), None, &mut sink)?;

    assert_expected_response(&message.content);
    assert_expected_response(&sink.output);
    assert!(
        message.usage.is_some(),
        "expected real provider to report stream usage for response: {:?}",
        message.content
    );
    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot.llm.call_count, 1);
    assert_eq!(snapshot.llm.failed_call_count, 0);
    assert!(snapshot.llm.total_tokens > 0);

    Ok(())
}

fn real_provider() -> Result<Option<Box<dyn Provider + Send>>, Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();
    if !real_provider_is_configured() {
        return Ok(None);
    }

    match env::var("TINY_CLAW_PROVIDER")?.as_str() {
        "openai-compatible" => Ok(Some(Box::new(OpenAiCompatibleProvider::from_env()?))),
        "claude-compatible" => Ok(Some(Box::new(ClaudeCompatibleProvider::from_env()?))),
        _ => Ok(None),
    }
}

fn real_provider_is_configured() -> bool {
    matches!(
        env::var("TINY_CLAW_PROVIDER").as_deref(),
        Ok("openai-compatible" | "claude-compatible")
    ) && env::var("TINY_CLAW_API_KEY")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn telemetry_prompt() -> Vec<Message> {
    vec![
        Message::system("Reply tersely and do not use markdown."),
        Message::user("Reply with exactly this token: telemetry-ok"),
    ]
}

fn assert_expected_response(content: &str) {
    assert!(
        content.trim().to_ascii_lowercase().contains("telemetry-ok"),
        "expected response to contain telemetry-ok, got: {content:?}"
    );
}

#[derive(Default)]
struct CapturingSink {
    output: String,
}

impl StreamSink for CapturingSink {
    fn on_text(&mut self, text: &str) -> Result<(), ProviderError> {
        self.output.push_str(text);
        Ok(())
    }
}
