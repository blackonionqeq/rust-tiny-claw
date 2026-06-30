use crate::schema::Usage;
use crate::tools::ToolAccessMode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

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
