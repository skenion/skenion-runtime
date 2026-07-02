use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::{IssueSeverity, unix_ms_timestamp};

pub const RUNTIME_LOG_SCHEMA: &str = "skenion.runtime.logs";
pub const RUNTIME_LOG_SCHEMA_VERSION: &str = "0.1.0";
pub const DEFAULT_RUNTIME_LOG_BACKLOG_LIMIT: usize = 200;
const RUNTIME_LOG_CHANNEL_CAPACITY: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeLogSource {
    Runtime,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLogEvent {
    pub id: u64,
    pub timestamp: String,
    pub source: RuntimeLogSource,
    pub level: IssueSeverity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLogRetention {
    pub replay_limit: usize,
    pub replay_levels: Vec<IssueSeverity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLogSnapshotResponse {
    pub schema: String,
    pub schema_version: String,
    pub ok: bool,
    pub events: Vec<RuntimeLogEvent>,
    pub retention: RuntimeLogRetention,
}

pub struct RuntimeLogStore {
    inner: Arc<Mutex<RuntimeLogStoreInner>>,
    sender: broadcast::Sender<RuntimeLogEvent>,
}

#[derive(Debug)]
struct RuntimeLogStoreInner {
    next_id: u64,
    backlog_limit: usize,
    warning_error_backlog: VecDeque<RuntimeLogEvent>,
}

impl RuntimeLogStore {
    pub fn new(backlog_limit: usize) -> Self {
        let (sender, _) = broadcast::channel(RUNTIME_LOG_CHANNEL_CAPACITY);
        Self {
            inner: Arc::new(Mutex::new(RuntimeLogStoreInner {
                next_id: 1,
                backlog_limit,
                warning_error_backlog: VecDeque::with_capacity(backlog_limit),
            })),
            sender,
        }
    }

    pub fn snapshot(&self) -> RuntimeLogSnapshotResponse {
        let inner = self
            .inner
            .lock()
            .expect("runtime log store lock should not be poisoned");
        RuntimeLogSnapshotResponse {
            schema: RUNTIME_LOG_SCHEMA.to_owned(),
            schema_version: RUNTIME_LOG_SCHEMA_VERSION.to_owned(),
            ok: true,
            events: inner.warning_error_backlog.iter().cloned().collect(),
            retention: RuntimeLogRetention {
                replay_limit: inner.backlog_limit,
                replay_levels: vec![IssueSeverity::Warning, IssueSeverity::Error],
            },
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeLogEvent> {
        self.sender.subscribe()
    }

    pub fn record_event(
        &self,
        level: IssueSeverity,
        code: Option<String>,
        message: String,
        details: Option<serde_json::Value>,
    ) {
        let event = {
            let mut inner = self
                .inner
                .lock()
                .expect("runtime log store lock should not be poisoned");
            let event = RuntimeLogEvent {
                id: inner.next_id,
                timestamp: unix_ms_timestamp(),
                source: RuntimeLogSource::Runtime,
                level: level.clone(),
                code,
                message,
                details,
            };
            inner.next_id = inner.next_id.saturating_add(1);
            if matches!(level, IssueSeverity::Warning | IssueSeverity::Error) {
                if inner.warning_error_backlog.len() == inner.backlog_limit {
                    inner.warning_error_backlog.pop_front();
                }
                inner.warning_error_backlog.push_back(event.clone());
            }
            event
        };

        let _ = self.sender.send(event);
    }
}

impl Default for RuntimeLogStore {
    fn default() -> Self {
        Self::new(DEFAULT_RUNTIME_LOG_BACKLOG_LIMIT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn runtime_log_store_replays_only_warning_and_error_backlog() {
        let store = RuntimeLogStore::new(2);
        store.record_event(IssueSeverity::Info, None, "connected".to_owned(), None);
        store.record_event(
            IssueSeverity::Warning,
            None,
            "first warning".to_owned(),
            None,
        );
        store.record_event(IssueSeverity::Error, None, "first error".to_owned(), None);
        store.record_event(
            IssueSeverity::Warning,
            None,
            "second warning".to_owned(),
            None,
        );

        let snapshot = store.snapshot();

        assert_eq!(snapshot.events.len(), 2);
        assert_eq!(snapshot.events[0].message, "first error");
        assert_eq!(snapshot.events[1].message, "second warning");
        assert_eq!(
            snapshot.retention.replay_levels,
            vec![IssueSeverity::Warning, IssueSeverity::Error]
        );
    }

    #[test]
    fn runtime_log_store_streams_explicit_events() {
        let store = RuntimeLogStore::new(8);
        let mut receiver = store.subscribe();

        store.record_event(
            IssueSeverity::Warning,
            Some("runtime.warning".to_owned()),
            "first warning".to_owned(),
            None,
        );
        store.record_event(
            IssueSeverity::Info,
            Some("runtime.info".to_owned()),
            "info note".to_owned(),
            None,
        );

        let snapshot = store.snapshot();

        assert_eq!(snapshot.events.len(), 1);
        assert_eq!(snapshot.events[0].level, IssueSeverity::Warning);
        assert_eq!(snapshot.events[0].code.as_deref(), Some("runtime.warning"));
        assert_eq!(receiver.try_recv().unwrap().message, "first warning");
    }

    #[test]
    fn runtime_log_store_preserves_structured_event_context() {
        let store = RuntimeLogStore::new(8);

        store.record_event(
            IssueSeverity::Warning,
            Some("extension.manifest.missing".to_owned()),
            "extension package is missing a manifest".to_owned(),
            Some(json!({
                "packagePath": "/tmp/skenion-extension",
                "action": "scan",
            })),
        );

        let snapshot = store.snapshot();

        assert_eq!(snapshot.events.len(), 1);
        assert_eq!(
            snapshot.events[0].code.as_deref(),
            Some("extension.manifest.missing")
        );
        assert_eq!(
            snapshot.events[0].details.as_ref().unwrap()["packagePath"],
            "/tmp/skenion-extension"
        );
        assert_eq!(
            snapshot.events[0].details.as_ref().unwrap()["action"],
            "scan"
        );
    }
}
