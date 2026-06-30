use crate::telemetry::json_exporter::JsonTraceExporter;
use crate::telemetry::otlp_exporter::OtlpTraceExporter;
use crate::telemetry::trace::TraceSpanRecord;
use std::env;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub trait TraceExporter: Send + Sync {
    fn export(&self, batch: &[TraceSpanRecord]) -> Result<(), TraceExportError>;

    fn shutdown(&self) -> Result<(), TraceExportError> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceMode {
    Off,
    Json,
    Otlp,
    Both,
    Debug,
}

impl TraceMode {
    pub fn from_env_value(value: &str) -> Result<Self, TraceExportError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "off" => Ok(Self::Off),
            "json" => Ok(Self::Json),
            "otlp" => Ok(Self::Otlp),
            "both" => Ok(Self::Both),
            "debug" => Ok(Self::Debug),
            other => Err(TraceExportError::new(format!(
                "invalid TINY_CLAW_TRACE value: {other}"
            ))),
        }
    }

    pub fn is_debug(self) -> bool {
        self == Self::Debug
    }
}

#[derive(Debug, Clone)]
pub struct TraceExporterConfig {
    pub mode: TraceMode,
    pub trace_dir: PathBuf,
    pub otlp_endpoint: Option<String>,
}

impl TraceExporterConfig {
    pub fn from_env(memory_root: &Path) -> Result<Self, TraceExportError> {
        let mode = env::var("TINY_CLAW_TRACE")
            .ok()
            .map(|value| TraceMode::from_env_value(&value))
            .transpose()?
            .unwrap_or(TraceMode::Off);
        let trace_dir = memory_root.join("traces");
        let otlp_endpoint = env::var("TINY_CLAW_OTLP_ENDPOINT").ok();

        Ok(Self {
            mode,
            trace_dir,
            otlp_endpoint,
        })
    }

    pub fn build_exporter(&self) -> Result<Option<Arc<dyn TraceExporter>>, TraceExportError> {
        match self.mode {
            TraceMode::Off => Ok(None),
            TraceMode::Json | TraceMode::Debug => {
                Ok(Some(Arc::new(JsonTraceExporter::new(&self.trace_dir))))
            }
            TraceMode::Otlp => Ok(Some(Arc::new(OtlpTraceExporter::new(
                self.otlp_endpoint.clone(),
            )?))),
            TraceMode::Both => Ok(Some(Arc::new(FanOutTraceExporter::new(vec![
                Arc::new(JsonTraceExporter::new(&self.trace_dir)),
                Arc::new(OtlpTraceExporter::new(self.otlp_endpoint.clone())?),
            ])))),
        }
    }
}

pub struct FanOutTraceExporter {
    exporters: Vec<Arc<dyn TraceExporter>>,
}

impl FanOutTraceExporter {
    pub fn new(exporters: Vec<Arc<dyn TraceExporter>>) -> Self {
        Self { exporters }
    }
}

impl TraceExporter for FanOutTraceExporter {
    fn export(&self, batch: &[TraceSpanRecord]) -> Result<(), TraceExportError> {
        let mut first_error = None;
        for exporter in &self.exporters {
            if let Err(error) = exporter.export(batch) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn shutdown(&self) -> Result<(), TraceExportError> {
        let mut first_error = None;
        for exporter in &self.exporters {
            if let Err(error) = exporter.shutdown() {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceExportError {
    message: String,
}

impl TraceExportError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for TraceExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TraceExportError {}

impl From<std::io::Error> for TraceExportError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<serde_json::Error> for TraceExportError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error.to_string())
    }
}
