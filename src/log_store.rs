use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::{
    DiagnosticSeverity, RuntimeClockDiagnostic, RuntimeClockDiagnosticSeverity, RuntimeDiagnostic,
    RuntimeIoDiagnostic, RuntimeIoDiagnosticSeverity, ShaderDiagnostic, ShaderDiagnosticSeverity,
    unix_ms_timestamp,
};

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
    pub level: DiagnosticSeverity,
    pub code: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLogRetention {
    pub replay_limit: usize,
    pub replay_levels: Vec<DiagnosticSeverity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLogSnapshotResponse {
    pub schema: String,
    pub schema_version: String,
    pub ok: bool,
    pub events: Vec<RuntimeLogEvent>,
    pub retention: RuntimeLogRetention,
    pub diagnostics: Vec<RuntimeDiagnostic>,
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
                replay_levels: vec![DiagnosticSeverity::Warning, DiagnosticSeverity::Error],
            },
            diagnostics: Vec::new(),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeLogEvent> {
        self.sender.subscribe()
    }

    pub fn record_runtime_diagnostics(&self, diagnostics: &[RuntimeDiagnostic]) {
        for diagnostic in diagnostics {
            self.push(
                diagnostic.severity.clone(),
                None,
                diagnostic.message.clone(),
            );
        }
    }

    pub fn record_shader_diagnostics(&self, diagnostics: &[ShaderDiagnostic]) {
        for diagnostic in diagnostics {
            self.push(
                shader_diagnostic_severity(&diagnostic.severity),
                Some(diagnostic.code.clone()),
                diagnostic.message.clone(),
            );
        }
    }

    pub fn record_clock_diagnostics(&self, diagnostics: &[RuntimeClockDiagnostic]) {
        for diagnostic in diagnostics {
            self.push(
                clock_diagnostic_severity(&diagnostic.severity),
                Some(diagnostic.code.clone()),
                diagnostic.message.clone(),
            );
        }
    }

    pub fn record_io_diagnostics(&self, diagnostics: &[RuntimeIoDiagnostic]) {
        for diagnostic in diagnostics {
            self.push(
                io_diagnostic_severity(&diagnostic.severity),
                Some(diagnostic.code.clone()),
                diagnostic.message.clone(),
            );
        }
    }

    fn push(&self, level: DiagnosticSeverity, code: Option<String>, message: String) {
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
            };
            inner.next_id = inner.next_id.saturating_add(1);
            if matches!(
                level,
                DiagnosticSeverity::Warning | DiagnosticSeverity::Error
            ) {
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

fn clock_diagnostic_severity(severity: &RuntimeClockDiagnosticSeverity) -> DiagnosticSeverity {
    match severity {
        RuntimeClockDiagnosticSeverity::Error => DiagnosticSeverity::Error,
        RuntimeClockDiagnosticSeverity::Warning => DiagnosticSeverity::Warning,
    }
}

fn io_diagnostic_severity(severity: &RuntimeIoDiagnosticSeverity) -> DiagnosticSeverity {
    match severity {
        RuntimeIoDiagnosticSeverity::Error => DiagnosticSeverity::Error,
        RuntimeIoDiagnosticSeverity::Warning => DiagnosticSeverity::Warning,
    }
}

fn shader_diagnostic_severity(severity: &ShaderDiagnosticSeverity) -> DiagnosticSeverity {
    match severity {
        ShaderDiagnosticSeverity::Error => DiagnosticSeverity::Error,
        ShaderDiagnosticSeverity::Warning => DiagnosticSeverity::Warning,
        ShaderDiagnosticSeverity::Info => DiagnosticSeverity::Info,
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        RuntimeClockDiagnostic, RuntimeClockDiagnosticSeverity, RuntimeIoDiagnostic,
        RuntimeIoDiagnosticSeverity,
    };

    use super::*;

    #[test]
    fn runtime_log_store_replays_only_warning_and_error_backlog() {
        let store = RuntimeLogStore::new(2);
        store.record_runtime_diagnostics(&[RuntimeDiagnostic {
            severity: DiagnosticSeverity::Info,
            message: "connected".to_owned(),
            code: None,
            details: None,
        }]);
        store.record_runtime_diagnostics(&[RuntimeDiagnostic::warning("first warning")]);
        store.record_runtime_diagnostics(&[RuntimeDiagnostic::error("first error")]);
        store.record_runtime_diagnostics(&[RuntimeDiagnostic::warning("second warning")]);

        let snapshot = store.snapshot();

        assert_eq!(snapshot.events.len(), 2);
        assert_eq!(snapshot.events[0].message, "first error");
        assert_eq!(snapshot.events[1].message, "second warning");
        assert_eq!(
            snapshot.retention.replay_levels,
            vec![DiagnosticSeverity::Warning, DiagnosticSeverity::Error]
        );
    }

    #[test]
    fn runtime_log_store_records_clock_io_and_shader_diagnostics() {
        let store = RuntimeLogStore::new(8);
        let mut receiver = store.subscribe();

        store.record_clock_diagnostics(&[
            RuntimeClockDiagnostic {
                severity: RuntimeClockDiagnosticSeverity::Warning,
                code: "clock-drift".to_owned(),
                message: "clock drifted".to_owned(),
            },
            RuntimeClockDiagnostic {
                severity: RuntimeClockDiagnosticSeverity::Error,
                code: "clock-lost".to_owned(),
                message: "clock lost".to_owned(),
            },
        ]);
        store.record_io_diagnostics(&[
            RuntimeIoDiagnostic {
                severity: RuntimeIoDiagnosticSeverity::Warning,
                code: "io-name".to_owned(),
                message: "device name unavailable".to_owned(),
            },
            RuntimeIoDiagnostic {
                severity: RuntimeIoDiagnosticSeverity::Error,
                code: "io-host".to_owned(),
                message: "device host unavailable".to_owned(),
            },
        ]);
        store.record_shader_diagnostics(&[
            ShaderDiagnostic {
                severity: ShaderDiagnosticSeverity::Warning,
                phase: crate::ShaderDiagnosticPhase::InterfaceAnalysis,
                code: "shader-warning".to_owned(),
                message: "shader warning".to_owned(),
                line: None,
                column: None,
                end_line: None,
                end_column: None,
                uniform_id: None,
                source: crate::ShaderDiagnosticSource::User,
            },
            ShaderDiagnostic {
                severity: ShaderDiagnosticSeverity::Info,
                phase: crate::ShaderDiagnosticPhase::InterfaceAnalysis,
                code: "shader-info".to_owned(),
                message: "shader note".to_owned(),
                line: None,
                column: None,
                end_line: None,
                end_column: None,
                uniform_id: None,
                source: crate::ShaderDiagnosticSource::User,
            },
        ]);

        let snapshot = store.snapshot();

        assert_eq!(snapshot.events.len(), 5);
        assert_eq!(snapshot.events[0].level, DiagnosticSeverity::Warning);
        assert_eq!(snapshot.events[1].level, DiagnosticSeverity::Error);
        assert_eq!(snapshot.events[2].code.as_deref(), Some("io-name"));
        assert_eq!(snapshot.events[3].code.as_deref(), Some("io-host"));
        assert_eq!(snapshot.events[4].code.as_deref(), Some("shader-warning"));
        assert_eq!(receiver.try_recv().unwrap().message, "clock drifted");
    }
}
