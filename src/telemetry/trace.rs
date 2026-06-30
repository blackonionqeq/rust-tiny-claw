use crate::telemetry::exporter::{TraceExportError, TraceExporter};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_QUEUE_CAPACITY: usize = 1024;
const DEFAULT_BATCH_SIZE: usize = 64;
const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct TraceId(pub u128);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct SpanId(pub u64);

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TraceSpanRecord {
    pub trace_id: TraceId,
    pub span_id: SpanId,
    pub parent_span_id: Option<SpanId>,
    pub name: String,
    pub start_time_unix_nano: u128,
    pub end_time_unix_nano: u128,
    pub attributes: Vec<TraceAttribute>,
    pub events: Vec<TraceEvent>,
    pub status: TraceStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TraceAttribute {
    pub key: String,
    pub value: TraceAttributeValue,
}

impl TraceAttribute {
    pub fn new(key: impl Into<String>, value: impl Into<TraceAttributeValue>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "value")]
pub enum TraceAttributeValue {
    String(String),
    Bool(bool),
    I64(i64),
    F64(f64),
}

impl From<String> for TraceAttributeValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for TraceAttributeValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl From<bool> for TraceAttributeValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for TraceAttributeValue {
    fn from(value: i64) -> Self {
        Self::I64(value)
    }
}

impl From<usize> for TraceAttributeValue {
    fn from(value: usize) -> Self {
        Self::I64(value as i64)
    }
}

impl From<u64> for TraceAttributeValue {
    fn from(value: u64) -> Self {
        Self::I64(value as i64)
    }
}

impl From<f64> for TraceAttributeValue {
    fn from(value: f64) -> Self {
        Self::F64(value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TraceEvent {
    pub name: String,
    pub time_unix_nano: u128,
    pub attributes: Vec<TraceAttribute>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum TraceStatus {
    Ok,
    Error { message: String },
}

#[derive(Clone)]
pub struct TraceRecorder {
    inner: Arc<TraceRecorderInner>,
}

struct TraceRecorderInner {
    enabled: bool,
    debug_flush: bool,
    ids: AtomicU64,
    dropped_spans: AtomicU64,
    active: Mutex<HashMap<SpanId, StartedSpan>>,
    sender: Option<SyncSender<TraceSpanRecord>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

struct StartedSpan {
    trace_id: TraceId,
    span_id: SpanId,
    parent_span_id: Option<SpanId>,
    name: String,
    start_time_unix_nano: u128,
    attributes: Vec<TraceAttribute>,
    events: Vec<TraceEvent>,
}

#[derive(Clone)]
pub struct TraceContext {
    recorder: TraceRecorder,
    trace_id: TraceId,
    current_span_id: SpanId,
}

pub struct SpanGuard {
    recorder: TraceRecorder,
    span_id: SpanId,
    ended: bool,
}

impl TraceRecorder {
    pub fn disabled() -> Self {
        Self {
            inner: Arc::new(TraceRecorderInner {
                enabled: false,
                debug_flush: false,
                ids: AtomicU64::new(seed_id()),
                dropped_spans: AtomicU64::new(0),
                active: Mutex::new(HashMap::new()),
                sender: None,
                worker: Mutex::new(None),
            }),
        }
    }

    pub fn new(exporter: Arc<dyn TraceExporter>, debug_flush: bool) -> Self {
        Self::with_options(
            exporter,
            debug_flush,
            DEFAULT_QUEUE_CAPACITY,
            DEFAULT_BATCH_SIZE,
            DEFAULT_FLUSH_INTERVAL,
        )
    }

    pub fn with_options(
        exporter: Arc<dyn TraceExporter>,
        debug_flush: bool,
        queue_capacity: usize,
        batch_size: usize,
        flush_interval: Duration,
    ) -> Self {
        let (sender, receiver) = sync_channel(queue_capacity);
        let worker = thread::spawn(move || {
            export_worker(receiver, exporter, batch_size, flush_interval);
        });

        Self {
            inner: Arc::new(TraceRecorderInner {
                enabled: true,
                debug_flush,
                ids: AtomicU64::new(seed_id()),
                dropped_spans: AtomicU64::new(0),
                active: Mutex::new(HashMap::new()),
                sender: Some(sender),
                worker: Mutex::new(Some(worker)),
            }),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.enabled
    }

    pub fn start_root(
        &self,
        name: impl Into<String>,
        attributes: Vec<TraceAttribute>,
    ) -> (TraceContext, SpanGuard) {
        let trace_id = TraceId(now_unix_nano() ^ self.next_span_id().0 as u128);
        let span_id = self.start_span(trace_id, None, name.into(), attributes);
        (
            TraceContext {
                recorder: self.clone(),
                trace_id,
                current_span_id: span_id,
            },
            SpanGuard {
                recorder: self.clone(),
                span_id,
                ended: false,
            },
        )
    }

    fn start_span(
        &self,
        trace_id: TraceId,
        parent_span_id: Option<SpanId>,
        name: String,
        attributes: Vec<TraceAttribute>,
    ) -> SpanId {
        let span_id = self.next_span_id();
        if self.inner.enabled {
            self.inner.active.lock().unwrap().insert(
                span_id,
                StartedSpan {
                    trace_id,
                    span_id,
                    parent_span_id,
                    name,
                    start_time_unix_nano: now_unix_nano(),
                    attributes,
                    events: Vec::new(),
                },
            );
        }
        span_id
    }

    fn next_span_id(&self) -> SpanId {
        SpanId(self.inner.ids.fetch_add(1, Ordering::Relaxed))
    }

    fn finish_span(&self, span_id: SpanId, status: TraceStatus) {
        if !self.inner.enabled {
            return;
        }

        let Some(started) = self.inner.active.lock().unwrap().remove(&span_id) else {
            return;
        };

        let record = TraceSpanRecord {
            trace_id: started.trace_id,
            span_id: started.span_id,
            parent_span_id: started.parent_span_id,
            name: started.name,
            start_time_unix_nano: started.start_time_unix_nano,
            end_time_unix_nano: now_unix_nano(),
            attributes: started.attributes,
            events: started.events,
            status,
        };

        if let Some(sender) = &self.inner.sender {
            match sender.try_send(record) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    self.inner.dropped_spans.fetch_add(1, Ordering::Relaxed);
                }
                Err(TrySendError::Disconnected(_)) => {}
            }
        }
    }

    fn add_attribute(&self, span_id: SpanId, attribute: TraceAttribute) {
        if !self.inner.enabled {
            return;
        }

        if let Some(span) = self.inner.active.lock().unwrap().get_mut(&span_id) {
            span.attributes.push(attribute);
        }
    }

    pub fn dropped_spans(&self) -> u64 {
        self.inner.dropped_spans.load(Ordering::Relaxed)
    }

    pub fn shutdown(&self) -> Result<(), TraceExportError> {
        if !self.inner.enabled {
            return Ok(());
        }

        if self.inner.debug_flush {
            std::thread::sleep(Duration::from_millis(20));
        }

        Ok(())
    }
}

impl TraceContext {
    pub fn start_child(
        &self,
        name: impl Into<String>,
        attributes: Vec<TraceAttribute>,
    ) -> SpanGuard {
        let span_id = self.recorder.start_span(
            self.trace_id,
            Some(self.current_span_id),
            name.into(),
            attributes,
        );
        SpanGuard {
            recorder: self.recorder.clone(),
            span_id,
            ended: false,
        }
    }

    pub fn child_context(&self, span_id: SpanId) -> Self {
        Self {
            recorder: self.recorder.clone(),
            trace_id: self.trace_id,
            current_span_id: span_id,
        }
    }

    pub fn trace_id(&self) -> TraceId {
        self.trace_id
    }

    pub fn current_span_id(&self) -> SpanId {
        self.current_span_id
    }
}

impl SpanGuard {
    pub fn context(&self, parent: &TraceContext) -> TraceContext {
        parent.child_context(self.span_id)
    }

    pub fn span_id(&self) -> SpanId {
        self.span_id
    }

    pub fn add_attribute(&self, attribute: TraceAttribute) {
        self.recorder.add_attribute(self.span_id, attribute);
    }

    pub fn add_attributes(&self, attributes: impl IntoIterator<Item = TraceAttribute>) {
        for attribute in attributes {
            self.add_attribute(attribute);
        }
    }

    pub fn end_ok(mut self) {
        self.end_with_status(TraceStatus::Ok);
    }

    pub fn end_error(mut self, message: impl Into<String>) {
        self.end_with_status(TraceStatus::Error {
            message: compact_preview(&message.into(), 256),
        });
    }

    pub fn end_with_status(&mut self, status: TraceStatus) {
        if self.ended {
            return;
        }
        self.recorder.finish_span(self.span_id, status);
        self.ended = true;
    }
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        self.end_with_status(TraceStatus::Ok);
    }
}

pub fn attr(key: impl Into<String>, value: impl Into<TraceAttributeValue>) -> TraceAttribute {
    TraceAttribute::new(key, value)
}

pub fn json_preview(value: &Value) -> String {
    compact_preview(&value.to_string(), 512)
}

pub fn text_preview(value: &str) -> String {
    compact_preview(value, 512)
}

fn compact_preview(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_string();
    }
    let preview = value.chars().take(max_chars).collect::<String>();
    format!("{preview}...[truncated chars: {}]", char_count - max_chars)
}

fn export_worker(
    receiver: Receiver<TraceSpanRecord>,
    exporter: Arc<dyn TraceExporter>,
    batch_size: usize,
    flush_interval: Duration,
) {
    let mut batch = Vec::with_capacity(batch_size);
    while let Ok(record) = receiver.recv_timeout(flush_interval) {
        batch.push(record);
        while batch.len() < batch_size {
            match receiver.try_recv() {
                Ok(record) => batch.push(record),
                Err(_) => break,
            }
        }
        let _ = exporter.export(&batch);
        batch.clear();
    }

    while let Ok(record) = receiver.try_recv() {
        batch.push(record);
        if batch.len() >= batch_size {
            let _ = exporter.export(&batch);
            batch.clear();
        }
    }
    if !batch.is_empty() {
        let _ = exporter.export(&batch);
    }
    let _ = exporter.shutdown();
}

fn now_unix_nano() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn seed_id() -> u64 {
    (now_unix_nano() as u64).max(1)
}

impl fmt::Debug for TraceRecorder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TraceRecorder")
            .field("enabled", &self.inner.enabled)
            .field("dropped_spans", &self.dropped_spans())
            .finish()
    }
}

impl Drop for TraceRecorderInner {
    fn drop(&mut self) {
        let _ = self.sender.take();
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TraceAttribute, TraceRecorder, TraceSpanRecord};
    use crate::telemetry::{TraceExportError, TraceExporter};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Default)]
    struct CollectingExporter {
        records: Mutex<Vec<TraceSpanRecord>>,
    }

    impl TraceExporter for CollectingExporter {
        fn export(&self, batch: &[TraceSpanRecord]) -> Result<(), TraceExportError> {
            self.records.lock().unwrap().extend_from_slice(batch);
            Ok(())
        }
    }

    #[test]
    fn root_and_child_spans_preserve_parent_ids() {
        let exporter = Arc::new(CollectingExporter::default());
        let recorder =
            TraceRecorder::with_options(exporter.clone(), true, 8, 8, Duration::from_millis(1));

        let (context, root) =
            recorder.start_root("Agent.Run", vec![TraceAttribute::new("session.id", "s1")]);
        let child = context.start_child("Agent.Turn", Vec::new());
        child.end_ok();
        root.end_ok();
        drop(context);
        drop(recorder);

        let records = exporter.records.lock().unwrap();
        assert_eq!(records.len(), 2);
        let root = records
            .iter()
            .find(|span| span.name == "Agent.Run")
            .unwrap();
        let child = records
            .iter()
            .find(|span| span.name == "Agent.Turn")
            .unwrap();
        assert_eq!(root.parent_span_id, None);
        assert_eq!(child.parent_span_id, Some(root.span_id));
        assert_eq!(child.trace_id, root.trace_id);
    }
}
