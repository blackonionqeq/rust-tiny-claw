use crate::telemetry::exporter::{TraceExportError, TraceExporter};
use crate::telemetry::trace::TraceSpanRecord;

pub struct OtlpTraceExporter {
    endpoint: String,
}

impl OtlpTraceExporter {
    pub fn new(endpoint: Option<String>) -> Result<Self, TraceExportError> {
        let endpoint = endpoint
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| TraceExportError::new("TINY_CLAW_OTLP_ENDPOINT is required"))?;
        Ok(Self {
            endpoint: normalize_traces_endpoint(&endpoint),
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

impl TraceExporter for OtlpTraceExporter {
    fn export(&self, _batch: &[TraceSpanRecord]) -> Result<(), TraceExportError> {
        // The internal trace model is now isolated behind TraceExporter. A later
        // lesson can map TraceSpanRecord into OTLP protobuf/HTTP here without
        // touching engine instrumentation.
        Ok(())
    }
}

pub fn normalize_traces_endpoint(endpoint: &str) -> String {
    let endpoint = endpoint.trim().trim_end_matches('/');
    if endpoint.ends_with("/v1/traces") {
        endpoint.to_string()
    } else {
        format!("{endpoint}/v1/traces")
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_traces_endpoint;

    #[test]
    fn normalizes_generic_otlp_endpoint_to_trace_path() {
        assert_eq!(
            normalize_traces_endpoint("http://localhost:4318"),
            "http://localhost:4318/v1/traces"
        );
        assert_eq!(
            normalize_traces_endpoint("http://localhost:4318/v1/traces"),
            "http://localhost:4318/v1/traces"
        );
    }
}
