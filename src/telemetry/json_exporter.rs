use crate::telemetry::exporter::{TraceExportError, TraceExporter};
use crate::telemetry::trace::{SpanId, TraceSpanRecord};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct JsonTraceExporter {
    trace_dir: PathBuf,
}

impl JsonTraceExporter {
    pub fn new(trace_dir: impl AsRef<Path>) -> Self {
        Self {
            trace_dir: trace_dir.as_ref().to_path_buf(),
        }
    }
}

impl TraceExporter for JsonTraceExporter {
    fn export(&self, batch: &[TraceSpanRecord]) -> Result<(), TraceExportError> {
        if batch.is_empty() {
            return Ok(());
        }

        fs::create_dir_all(&self.trace_dir)?;
        let tree = build_trace_tree(batch);
        let file_name = format!(
            "trace-{}-{}.json",
            batch[0].trace_id.0,
            batch
                .iter()
                .map(|span| span.start_time_unix_nano)
                .min()
                .unwrap_or(0)
        );
        let path = self.trace_dir.join(file_name);
        fs::write(path, serde_json::to_vec_pretty(&tree)?)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TraceTreeNode {
    pub span: TraceSpanRecord,
    pub children: Vec<TraceTreeNode>,
}

pub fn build_trace_tree(records: &[TraceSpanRecord]) -> Vec<TraceTreeNode> {
    let mut children_by_parent: HashMap<Option<SpanId>, Vec<TraceSpanRecord>> = HashMap::new();
    for record in records {
        children_by_parent
            .entry(record.parent_span_id)
            .or_default()
            .push(record.clone());
    }

    for children in children_by_parent.values_mut() {
        children.sort_by_key(|span| span.start_time_unix_nano);
    }

    build_nodes(None, &mut children_by_parent)
}

fn build_nodes(
    parent: Option<SpanId>,
    children_by_parent: &mut HashMap<Option<SpanId>, Vec<TraceSpanRecord>>,
) -> Vec<TraceTreeNode> {
    children_by_parent
        .remove(&parent)
        .unwrap_or_default()
        .into_iter()
        .map(|span| {
            let children = build_nodes(Some(span.span_id), children_by_parent);
            TraceTreeNode { span, children }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::build_trace_tree;
    use crate::telemetry::{SpanId, TraceId, TraceSpanRecord, TraceStatus};

    #[test]
    fn json_tree_groups_children_under_parent() {
        let root = span("root", 1, None, 10);
        let child = span("child", 2, Some(SpanId(1)), 11);

        let tree = build_trace_tree(&[child, root]);

        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].span.name, "root");
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].span.name, "child");
    }

    fn span(
        name: &str,
        span_id: u64,
        parent_span_id: Option<SpanId>,
        start_time_unix_nano: u128,
    ) -> TraceSpanRecord {
        TraceSpanRecord {
            trace_id: TraceId(1),
            span_id: SpanId(span_id),
            parent_span_id,
            name: name.to_string(),
            start_time_unix_nano,
            end_time_unix_nano: start_time_unix_nano + 1,
            attributes: Vec::new(),
            events: Vec::new(),
            status: TraceStatus::Ok,
        }
    }
}
