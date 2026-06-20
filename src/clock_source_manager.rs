use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::{
    ClockSourceStore, MidiClockAdapter, RuntimeClockDiagnostic, RuntimeClockDiagnosticSeverity,
    RuntimeClockSnapshot, RuntimeClockSourceId, RuntimeClockSourceKind, TimestampedMidiMessage,
    contract::ClockState,
    midi_input::{
        collect_midi_input_ports, create_midi_input, host_monotonic_timestamp_ns,
        invalid_port_diagnostic,
    },
};

const MIDI_EVENT_QUEUE_CAPACITY: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeClockSourceStatus {
    Running,
    Stopped,
    Error,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockSourceSnapshot {
    pub source_id: RuntimeClockSourceId,
    pub source_kind: RuntimeClockSourceKind,
    pub status: RuntimeClockSourceStatus,
    pub latest_snapshot: Option<ClockState>,
    pub diagnostics: Vec<RuntimeClockDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockSourceListResponse {
    pub ok: bool,
    pub sources: Vec<ClockSourceSnapshot>,
    pub diagnostics: Vec<RuntimeClockDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockSourceSnapshotResponse {
    pub ok: bool,
    pub source: Option<ClockSourceSnapshot>,
    pub diagnostics: Vec<RuntimeClockDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiInputDescriptor {
    pub index: usize,
    pub name: String,
    pub backend: String,
    pub id: Option<String>,
    pub stable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiInputListResponse {
    pub ok: bool,
    pub inputs: Vec<MidiInputDescriptor>,
    pub diagnostics: Vec<RuntimeClockDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiClockSourceStartRequest {
    pub source_id: String,
    pub input_port_index: usize,
    #[serde(default)]
    pub time_signature: Option<crate::contract::ClockTimeSignature>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiClockSourceStartResponse {
    pub ok: bool,
    pub source: Option<ClockSourceSnapshot>,
    pub diagnostics: Vec<RuntimeClockDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiClockSourceStopRequest {
    pub source_id: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiClockSourceStopResponse {
    pub ok: bool,
    pub source: Option<ClockSourceSnapshot>,
    pub diagnostics: Vec<RuntimeClockDiagnostic>,
}

pub struct ClockSourceManager {
    store: Arc<RwLock<ClockSourceStore>>,
    tasks: Mutex<BTreeMap<RuntimeClockSourceId, ClockSourceTaskHandle>>,
    midi_inputs: Arc<dyn MidiInputRegistry>,
}

impl ClockSourceManager {
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(ClockSourceStore::new())),
            tasks: Mutex::new(BTreeMap::new()),
            midi_inputs: Arc::new(MidirInputRegistry),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_midi_input_registry(midi_inputs: Arc<dyn MidiInputRegistry>) -> Self {
        Self {
            store: Arc::new(RwLock::new(ClockSourceStore::new())),
            tasks: Mutex::new(BTreeMap::new()),
            midi_inputs,
        }
    }

    pub fn list_sources(&self) -> ClockSourceListResponse {
        let running = self.running_source_ids();
        let store = read_or_recover(&self.store);
        ClockSourceListResponse {
            ok: true,
            sources: store
                .list()
                .into_iter()
                .map(|snapshot| source_snapshot(snapshot, &running))
                .collect(),
            diagnostics: Vec::new(),
        }
    }

    pub fn get_source(&self, source_id: impl Into<String>) -> ClockSourceSnapshotResponse {
        let source_id = RuntimeClockSourceId::new(source_id);
        let running = self.running_source_ids();
        let store = read_or_recover(&self.store);
        let source = store
            .get(&source_id)
            .map(|snapshot| source_snapshot(snapshot, &running));
        match source {
            Some(source) => ClockSourceSnapshotResponse {
                ok: true,
                source: Some(source),
                diagnostics: Vec::new(),
            },
            None => ClockSourceSnapshotResponse {
                ok: false,
                source: None,
                diagnostics: vec![clock_source_not_found_diagnostic(&source_id)],
            },
        }
    }

    pub fn list_midi_inputs(&self) -> MidiInputListResponse {
        self.midi_inputs.list_inputs()
    }

    pub fn start_midi_clock(
        &self,
        request: MidiClockSourceStartRequest,
    ) -> MidiClockSourceStartResponse {
        let source_id = RuntimeClockSourceId::new(request.source_id.trim().to_owned());
        if source_id.as_str().is_empty() {
            return MidiClockSourceStartResponse {
                ok: false,
                source: None,
                diagnostics: vec![invalid_clock_source_id_diagnostic()],
            };
        }

        let mut tasks = lock_or_recover(&self.tasks);
        if tasks.contains_key(&source_id) {
            return MidiClockSourceStartResponse {
                ok: false,
                source: self.snapshot_for_source(&source_id, RuntimeClockSourceStatus::Running),
                diagnostics: vec![RuntimeClockDiagnostic {
                    severity: RuntimeClockDiagnosticSeverity::Error,
                    code: "clock-source-already-running".to_owned(),
                    message: format!("clock source {source_id} is already running"),
                }],
            };
        }

        let (event_tx, event_rx) = mpsc::sync_channel(MIDI_EVENT_QUEUE_CAPACITY);
        let dropped_event_count = Arc::new(AtomicU64::new(0));
        let connection = match self.midi_inputs.open_midi_clock_input(
            request.input_port_index,
            event_tx,
            Arc::clone(&dropped_event_count),
        ) {
            Ok(connection) => connection,
            Err(diagnostic) => {
                return MidiClockSourceStartResponse {
                    ok: false,
                    source: None,
                    diagnostics: vec![diagnostic],
                };
            }
        };

        let adapter = MidiClockAdapter::new(source_id.as_str(), request.time_signature);
        let initial_snapshot = adapter.current_snapshot();
        {
            let mut store = write_or_recover(&self.store);
            store.insert_or_update(initial_snapshot.clone());
        }

        let (stop_tx, stop_rx) = mpsc::channel();
        let store = Arc::clone(&self.store);
        let join = thread::spawn(move || {
            midi_clock_source_worker(
                connection,
                adapter,
                event_rx,
                stop_rx,
                store,
                dropped_event_count,
            )
        });
        tasks.insert(
            source_id.clone(),
            ClockSourceTaskHandle {
                stop_tx,
                join: Some(join),
            },
        );
        drop(tasks);

        MidiClockSourceStartResponse {
            ok: true,
            source: Some(source_snapshot_with_status(
                &initial_snapshot,
                RuntimeClockSourceStatus::Running,
            )),
            diagnostics: Vec::new(),
        }
    }

    pub fn stop_midi_clock(
        &self,
        request: MidiClockSourceStopRequest,
    ) -> MidiClockSourceStopResponse {
        let source_id = RuntimeClockSourceId::new(request.source_id.trim().to_owned());
        if source_id.as_str().is_empty() {
            return MidiClockSourceStopResponse {
                ok: false,
                source: None,
                diagnostics: vec![invalid_clock_source_id_diagnostic()],
            };
        }

        let task = {
            let mut tasks = lock_or_recover(&self.tasks);
            tasks.remove(&source_id)
        };
        let Some(mut task) = task else {
            return MidiClockSourceStopResponse {
                ok: false,
                source: self.snapshot_for_source(&source_id, RuntimeClockSourceStatus::Stopped),
                diagnostics: vec![clock_source_not_found_diagnostic(&source_id)],
            };
        };

        let mut diagnostics = Vec::new();
        if task.stop_tx.send(()).is_err() {
            diagnostics.push(RuntimeClockDiagnostic {
                severity: RuntimeClockDiagnosticSeverity::Warning,
                code: "clock-source-stop-failed".to_owned(),
                message: format!("clock source {source_id} stop signal was not received"),
            });
        }
        if let Some(join) = task.join.take()
            && join.join().is_err()
        {
            diagnostics.push(RuntimeClockDiagnostic {
                severity: RuntimeClockDiagnosticSeverity::Error,
                code: "clock-source-stop-failed".to_owned(),
                message: format!("clock source {source_id} worker panicked during stop"),
            });
        }

        MidiClockSourceStopResponse {
            ok: !diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == RuntimeClockDiagnosticSeverity::Error),
            source: self.snapshot_for_source(&source_id, RuntimeClockSourceStatus::Stopped),
            diagnostics,
        }
    }

    fn running_source_ids(&self) -> BTreeMap<RuntimeClockSourceId, RuntimeClockSourceStatus> {
        lock_or_recover(&self.tasks)
            .keys()
            .cloned()
            .map(|source_id| (source_id, RuntimeClockSourceStatus::Running))
            .collect()
    }

    fn snapshot_for_source(
        &self,
        source_id: &RuntimeClockSourceId,
        status: RuntimeClockSourceStatus,
    ) -> Option<ClockSourceSnapshot> {
        let store = read_or_recover(&self.store);
        store
            .get(source_id)
            .map(|snapshot| source_snapshot_with_status(snapshot, status))
    }
}

impl Default for ClockSourceManager {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) trait MidiClockInputConnection: Send {}

pub(crate) trait MidiInputRegistry: Send + Sync {
    fn list_inputs(&self) -> MidiInputListResponse;

    fn open_midi_clock_input(
        &self,
        input_port_index: usize,
        event_sender: SyncSender<TimestampedMidiMessage>,
        dropped_event_count: Arc<AtomicU64>,
    ) -> Result<Box<dyn MidiClockInputConnection>, RuntimeClockDiagnostic>;
}

struct MidirInputRegistry;

struct MidirClockInputConnection {
    _connection: midir::MidiInputConnection<()>,
}

impl MidiClockInputConnection for MidirClockInputConnection {}

impl MidiInputRegistry for MidirInputRegistry {
    fn list_inputs(&self) -> MidiInputListResponse {
        let report = crate::list_midi_input_ports();
        let diagnostics = report
            .diagnostics
            .into_iter()
            .map(map_midi_input_diagnostic)
            .collect::<Vec<_>>();
        let ok = !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == RuntimeClockDiagnosticSeverity::Error);
        MidiInputListResponse {
            ok,
            inputs: report
                .ports
                .into_iter()
                .map(MidiInputDescriptor::from)
                .collect(),
            diagnostics,
        }
    }

    fn open_midi_clock_input(
        &self,
        input_port_index: usize,
        event_sender: SyncSender<TimestampedMidiMessage>,
        dropped_event_count: Arc<AtomicU64>,
    ) -> Result<Box<dyn MidiClockInputConnection>, RuntimeClockDiagnostic> {
        let input = create_midi_input("skenion-runtime-clock-source-api")
            .map_err(map_midi_input_diagnostic)?;
        let midir_ports = input.ports();
        let mut diagnostics = Vec::new();
        let ports = collect_midi_input_ports(&input, &midir_ports, &mut diagnostics);
        let Some(midir_port) = midir_ports.get(input_port_index) else {
            return Err(invalid_port_diagnostic(
                input_port_index,
                ports.len(),
                "requested MIDI input port does not exist",
            ));
        };
        let connection = input
            .connect(
                midir_port,
                "skenion-runtime-clock-source-input",
                move |_midir_timestamp, message, _| {
                    // midir timestamps are backend-defined microseconds, so Runtime stamps
                    // receipt with its own monotonic process clock instead.
                    let timestamped = TimestampedMidiMessage {
                        bytes: message.to_vec(),
                        received_host_time_ns: host_monotonic_timestamp_ns(),
                    };
                    match event_sender.try_send(timestamped) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {
                            dropped_event_count.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(TrySendError::Disconnected(_)) => {}
                    }
                },
                (),
            )
            .map_err(|error| RuntimeClockDiagnostic {
                severity: RuntimeClockDiagnosticSeverity::Error,
                code: "midi-input-open-failed".to_owned(),
                message: format!("failed to open MIDI input port {input_port_index}: {error}"),
            })?;
        Ok(Box::new(MidirClockInputConnection {
            _connection: connection,
        }))
    }
}

struct ClockSourceTaskHandle {
    stop_tx: mpsc::Sender<()>,
    join: Option<JoinHandle<()>>,
}

fn midi_clock_source_worker(
    _connection: Box<dyn MidiClockInputConnection>,
    mut adapter: MidiClockAdapter,
    event_rx: Receiver<TimestampedMidiMessage>,
    stop_rx: Receiver<()>,
    store: Arc<RwLock<ClockSourceStore>>,
    dropped_event_count: Arc<AtomicU64>,
) {
    loop {
        match stop_rx.try_recv() {
            Ok(()) | Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {}
        }

        match event_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(timestamped) => {
                let mut snapshot = adapter.apply_timestamped_message(timestamped);
                let dropped = dropped_event_count.swap(0, Ordering::Relaxed);
                if dropped > 0 {
                    snapshot.diagnostics.push(RuntimeClockDiagnostic {
                        severity: RuntimeClockDiagnosticSeverity::Warning,
                        code: "clock-source-event-queue-full".to_owned(),
                        message: format!("dropped {dropped} MIDI clock event(s) because the runtime event queue was full"),
                    });
                }
                let mut store = write_or_recover(&store);
                store.insert_or_update(snapshot);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn source_snapshot(
    snapshot: &RuntimeClockSnapshot,
    running: &BTreeMap<RuntimeClockSourceId, RuntimeClockSourceStatus>,
) -> ClockSourceSnapshot {
    let status = running
        .get(&snapshot.source_id)
        .copied()
        .unwrap_or(RuntimeClockSourceStatus::Stopped);
    source_snapshot_with_status(snapshot, status)
}

fn source_snapshot_with_status(
    snapshot: &RuntimeClockSnapshot,
    status: RuntimeClockSourceStatus,
) -> ClockSourceSnapshot {
    ClockSourceSnapshot {
        source_id: snapshot.source_id.clone(),
        source_kind: snapshot.source_kind.clone(),
        status,
        latest_snapshot: Some(snapshot.clock_state.clone()),
        diagnostics: snapshot.diagnostics.clone(),
    }
}

fn invalid_clock_source_id_diagnostic() -> RuntimeClockDiagnostic {
    RuntimeClockDiagnostic {
        severity: RuntimeClockDiagnosticSeverity::Error,
        code: "invalid-clock-source-id".to_owned(),
        message: "clock source id must be a non-empty string".to_owned(),
    }
}

fn clock_source_not_found_diagnostic(source_id: &RuntimeClockSourceId) -> RuntimeClockDiagnostic {
    RuntimeClockDiagnostic {
        severity: RuntimeClockDiagnosticSeverity::Error,
        code: "clock-source-not-found".to_owned(),
        message: format!("clock source {source_id} was not found"),
    }
}

fn map_midi_input_diagnostic(mut diagnostic: RuntimeClockDiagnostic) -> RuntimeClockDiagnostic {
    if diagnostic.code == "midi-input-unavailable" {
        diagnostic.code = "midi-input-enumeration-failed".to_owned();
    }
    diagnostic
}

impl From<crate::RuntimeMidiInputPort> for MidiInputDescriptor {
    fn from(port: crate::RuntimeMidiInputPort) -> Self {
        Self {
            index: port.index,
            name: port.name,
            backend: "midir".to_owned(),
            id: None,
            stable: false,
        }
    }
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn read_or_recover<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|error| error.into_inner())
}

fn write_or_recover<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeConnection;

    impl MidiClockInputConnection for FakeConnection {}

    struct FakeRegistry {
        inputs: Vec<MidiInputDescriptor>,
    }

    impl MidiInputRegistry for FakeRegistry {
        fn list_inputs(&self) -> MidiInputListResponse {
            MidiInputListResponse {
                ok: true,
                inputs: self.inputs.clone(),
                diagnostics: Vec::new(),
            }
        }

        fn open_midi_clock_input(
            &self,
            input_port_index: usize,
            _event_sender: SyncSender<TimestampedMidiMessage>,
            _dropped_event_count: Arc<AtomicU64>,
        ) -> Result<Box<dyn MidiClockInputConnection>, RuntimeClockDiagnostic> {
            if self
                .inputs
                .iter()
                .any(|input| input.index == input_port_index)
            {
                Ok(Box::new(FakeConnection))
            } else {
                Err(invalid_port_diagnostic(
                    input_port_index,
                    self.inputs.len(),
                    "requested MIDI input port does not exist",
                ))
            }
        }
    }

    #[test]
    fn rejects_duplicate_running_source_id() {
        let manager = ClockSourceManager::with_midi_input_registry(Arc::new(FakeRegistry {
            inputs: vec![MidiInputDescriptor {
                index: 0,
                name: "Fake MIDI".to_owned(),
                backend: "midir".to_owned(),
                id: None,
                stable: false,
            }],
        }));

        let first = manager.start_midi_clock(MidiClockSourceStartRequest {
            source_id: "midi-clock-1".to_owned(),
            input_port_index: 0,
            time_signature: None,
        });
        assert!(first.ok);

        let duplicate = manager.start_midi_clock(MidiClockSourceStartRequest {
            source_id: "midi-clock-1".to_owned(),
            input_port_index: 0,
            time_signature: None,
        });
        assert!(!duplicate.ok);
        assert_eq!(
            duplicate.diagnostics[0].code,
            "clock-source-already-running"
        );

        let stop = manager.stop_midi_clock(MidiClockSourceStopRequest {
            source_id: "midi-clock-1".to_owned(),
        });
        assert!(stop.ok);

        let restarted = manager.start_midi_clock(MidiClockSourceStartRequest {
            source_id: "midi-clock-1".to_owned(),
            input_port_index: 0,
            time_signature: None,
        });
        assert!(restarted.ok);
        let _ = manager.stop_midi_clock(MidiClockSourceStopRequest {
            source_id: "midi-clock-1".to_owned(),
        });
    }
}
