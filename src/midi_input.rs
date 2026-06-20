use std::{
    sync::{Arc, Mutex, MutexGuard, OnceLock},
    thread,
    time::{Duration, Instant},
};

use midir::{Ignore, MidiInput, MidiInputPort};
use serde::{Deserialize, Serialize};

use crate::{
    ClockSourceStore, MidiClockAdapter, RuntimeClockDiagnostic, RuntimeClockDiagnosticSeverity,
    RuntimeClockSnapshot, RuntimeClockSourceId, TimestampedMidiMessage,
};

pub const RUNTIME_MIDI_INPUT_SCHEMA: &str = "skenion.runtime.midi-input";
pub const RUNTIME_MIDI_INPUT_SCHEMA_VERSION: &str = "0.1.0";
pub const RUNTIME_MIDI_CLOCK_INPUT_SCHEMA: &str = "skenion.runtime.clock-midi.input";
pub const RUNTIME_MIDI_CLOCK_INPUT_SCHEMA_VERSION: &str = "0.1.0";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMidiInputPort {
    pub index: usize,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMidiInputListReport {
    pub schema: String,
    pub schema_version: String,
    pub ports: Vec<RuntimeMidiInputPort>,
    pub diagnostics: Vec<RuntimeClockDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMidiClockInputRequest {
    pub source_id: String,
    pub port_index: usize,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMidiClockInputReport {
    pub schema: String,
    pub schema_version: String,
    pub source_id: RuntimeClockSourceId,
    pub requested_port_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<RuntimeMidiInputPort>,
    pub opened: bool,
    pub duration_ms: u64,
    pub store: ClockSourceStore,
    pub latest_snapshot: RuntimeClockSnapshot,
    pub diagnostics: Vec<RuntimeClockDiagnostic>,
}

pub fn list_midi_input_ports() -> RuntimeMidiInputListReport {
    let mut diagnostics = Vec::new();
    let ports = match create_midi_input("skenion-runtime-midi-list") {
        Ok(input) => {
            let midir_ports = input.ports();
            collect_midi_input_ports(&input, &midir_ports, &mut diagnostics)
        }
        Err(diagnostic) => {
            diagnostics.push(diagnostic);
            Vec::new()
        }
    };
    RuntimeMidiInputListReport {
        schema: RUNTIME_MIDI_INPUT_SCHEMA.to_owned(),
        schema_version: RUNTIME_MIDI_INPUT_SCHEMA_VERSION.to_owned(),
        ports,
        diagnostics,
    }
}

pub fn run_midi_clock_input(request: RuntimeMidiClockInputRequest) -> RuntimeMidiClockInputReport {
    let initial_adapter = MidiClockAdapter::new(request.source_id.clone(), None);
    let initial_snapshot = initial_adapter.current_snapshot();
    let source_id = initial_snapshot.source_id.clone();
    let mut initial_store = ClockSourceStore::new();
    initial_store.insert_or_update(initial_snapshot.clone());

    let mut diagnostics = Vec::new();
    let input = match create_midi_input("skenion-runtime-midi-clock") {
        Ok(input) => input,
        Err(diagnostic) => {
            diagnostics.push(diagnostic);
            diagnostics.push(invalid_port_diagnostic(
                request.port_index,
                0,
                "MIDI input host is unavailable",
            ));
            return midi_clock_input_report(
                request,
                None,
                false,
                initial_store,
                initial_snapshot,
                diagnostics,
            );
        }
    };

    let midir_ports = input.ports();
    let ports = collect_midi_input_ports(&input, &midir_ports, &mut diagnostics);
    let Some(midir_port) = midir_ports.get(request.port_index).cloned() else {
        diagnostics.push(invalid_port_diagnostic(
            request.port_index,
            ports.len(),
            "requested MIDI input port does not exist",
        ));
        return midi_clock_input_report(
            request,
            None,
            false,
            initial_store,
            initial_snapshot,
            diagnostics,
        );
    };
    let selected_port = ports
        .iter()
        .find(|port| port.index == request.port_index)
        .cloned()
        .unwrap_or_else(|| RuntimeMidiInputPort {
            index: request.port_index,
            name: format!("MIDI Input {}", request.port_index),
        });

    let adapter = Arc::new(Mutex::new(initial_adapter));
    let store = Arc::new(Mutex::new(initial_store));
    let callback_diagnostics = Arc::new(Mutex::new(Vec::new()));
    let adapter_for_callback = Arc::clone(&adapter);
    let store_for_callback = Arc::clone(&store);
    let diagnostics_for_callback = Arc::clone(&callback_diagnostics);

    let connection = input.connect(
        &midir_port,
        "skenion-runtime-midi-clock-input",
        move |_midir_timestamp, message, _| {
            // midir timestamps are backend-defined microseconds, so Runtime stamps
            // receipt with its own monotonic process clock instead.
            let received_host_time_ns = host_monotonic_timestamp_ns();
            let snapshot = {
                let mut adapter = lock_or_recover(&adapter_for_callback);
                adapter.apply_timestamped_message(TimestampedMidiMessage {
                    bytes: message.to_vec(),
                    received_host_time_ns,
                })
            };
            {
                let mut store = lock_or_recover(&store_for_callback);
                store.insert_or_update(snapshot.clone());
            }
            if !snapshot.diagnostics.is_empty() {
                let mut diagnostics = lock_or_recover(&diagnostics_for_callback);
                diagnostics.extend(snapshot.diagnostics);
            }
        },
        (),
    );

    let connection = match connection {
        Ok(connection) => connection,
        Err(error) => {
            diagnostics.push(RuntimeClockDiagnostic {
                severity: RuntimeClockDiagnosticSeverity::Error,
                code: "midi-input-open-failed".to_owned(),
                message: format!(
                    "failed to open MIDI input port {}: {error}",
                    request.port_index
                ),
            });
            return midi_clock_input_report(
                request,
                Some(selected_port),
                false,
                clone_store(&store),
                latest_snapshot(&store, &source_id, initial_snapshot),
                diagnostics,
            );
        }
    };

    thread::sleep(Duration::from_millis(request.duration_ms));
    drop(connection);

    diagnostics.extend(lock_or_recover(&callback_diagnostics).clone());
    let store = clone_store(&store);
    let latest_snapshot = store
        .get(&source_id)
        .cloned()
        .unwrap_or_else(|| MidiClockAdapter::new(source_id.as_str(), None).current_snapshot());

    midi_clock_input_report(
        request,
        Some(selected_port),
        true,
        store,
        latest_snapshot,
        diagnostics,
    )
}

pub fn format_midi_input_list_report_text(report: &RuntimeMidiInputListReport) -> String {
    let mut lines = vec![format!("midi inputs: {}", report.ports.len())];
    for port in &report.ports {
        lines.push(format!("port: {} {}", port.index, port.name));
    }
    push_diagnostics(&mut lines, &report.diagnostics);
    lines.join("\n") + "\n"
}

pub fn format_midi_clock_input_report_text(report: &RuntimeMidiClockInputReport) -> String {
    let mut lines = vec![
        format!("runtime midi clock input: {}", report.source_id),
        format!("requestedPortIndex: {}", report.requested_port_index),
        format!("opened: {}", report.opened),
        format!("durationMs: {}", report.duration_ms),
    ];
    if let Some(port) = &report.port {
        lines.push(format!("port: {} {}", port.index, port.name));
    }
    lines.push(format!(
        "songPositionSource: {:?}",
        report.latest_snapshot.song_position_source
    ));
    push_diagnostics(&mut lines, &report.diagnostics);
    lines.join("\n") + "\n"
}

fn create_midi_input(client_name: &str) -> Result<MidiInput, RuntimeClockDiagnostic> {
    let mut input = MidiInput::new(client_name).map_err(|error| RuntimeClockDiagnostic {
        severity: RuntimeClockDiagnosticSeverity::Error,
        code: "midi-input-unavailable".to_owned(),
        message: format!("failed to initialize MIDI input host: {error}"),
    })?;
    input.ignore(Ignore::None);
    Ok(input)
}

fn collect_midi_input_ports(
    input: &MidiInput,
    midir_ports: &[MidiInputPort],
    diagnostics: &mut Vec<RuntimeClockDiagnostic>,
) -> Vec<RuntimeMidiInputPort> {
    midir_ports
        .iter()
        .enumerate()
        .map(|(index, port)| RuntimeMidiInputPort {
            index,
            name: input.port_name(port).unwrap_or_else(|error| {
                diagnostics.push(RuntimeClockDiagnostic {
                    severity: RuntimeClockDiagnosticSeverity::Warning,
                    code: "midi-input-port-name-unavailable".to_owned(),
                    message: format!("failed to read MIDI input port {index} name: {error}"),
                });
                format!("MIDI Input {index}")
            }),
        })
        .collect()
}

fn invalid_port_diagnostic(
    index: usize,
    available_count: usize,
    reason: &str,
) -> RuntimeClockDiagnostic {
    RuntimeClockDiagnostic {
        severity: RuntimeClockDiagnosticSeverity::Error,
        code: "invalid-midi-input-port".to_owned(),
        message: format!(
            "{reason}; requested index {index}, available MIDI input ports {available_count}"
        ),
    }
}

fn midi_clock_input_report(
    request: RuntimeMidiClockInputRequest,
    port: Option<RuntimeMidiInputPort>,
    opened: bool,
    store: ClockSourceStore,
    latest_snapshot: RuntimeClockSnapshot,
    diagnostics: Vec<RuntimeClockDiagnostic>,
) -> RuntimeMidiClockInputReport {
    RuntimeMidiClockInputReport {
        schema: RUNTIME_MIDI_CLOCK_INPUT_SCHEMA.to_owned(),
        schema_version: RUNTIME_MIDI_CLOCK_INPUT_SCHEMA_VERSION.to_owned(),
        source_id: RuntimeClockSourceId::new(request.source_id),
        requested_port_index: request.port_index,
        port,
        opened,
        duration_ms: request.duration_ms,
        store,
        latest_snapshot,
        diagnostics,
    }
}

fn latest_snapshot(
    store: &Arc<Mutex<ClockSourceStore>>,
    source_id: &RuntimeClockSourceId,
    fallback: RuntimeClockSnapshot,
) -> RuntimeClockSnapshot {
    lock_or_recover(store)
        .get(source_id)
        .cloned()
        .unwrap_or(fallback)
}

fn clone_store(store: &Arc<Mutex<ClockSourceStore>>) -> ClockSourceStore {
    lock_or_recover(store).clone()
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn host_monotonic_timestamp_ns() -> u64 {
    static PROCESS_START: OnceLock<Instant> = OnceLock::new();
    PROCESS_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_nanos()
        .min(u128::from(u64::MAX)) as u64
}

fn push_diagnostics(lines: &mut Vec<String>, diagnostics: &[RuntimeClockDiagnostic]) {
    lines.push(format!("diagnostics: {}", diagnostics.len()));
    for diagnostic in diagnostics {
        lines.push(format!(
            "diagnostic: {:?} {} {}",
            diagnostic.severity, diagnostic.code, diagnostic.message
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MidiSongPositionSource;

    #[test]
    fn text_formatters_include_ports_diagnostics_and_snapshot_source() {
        let diagnostic = invalid_port_diagnostic(99, 0, "test invalid port");
        let list_report = RuntimeMidiInputListReport {
            schema: RUNTIME_MIDI_INPUT_SCHEMA.to_owned(),
            schema_version: RUNTIME_MIDI_INPUT_SCHEMA_VERSION.to_owned(),
            ports: vec![RuntimeMidiInputPort {
                index: 0,
                name: "Test MIDI".to_owned(),
            }],
            diagnostics: vec![diagnostic.clone()],
        };
        let list_text = format_midi_input_list_report_text(&list_report);
        assert!(list_text.contains("midi inputs: 1"));
        assert!(list_text.contains("port: 0 Test MIDI"));
        assert!(list_text.contains("invalid-midi-input-port"));

        let snapshot = MidiClockAdapter::new("midi-clock-input", None).current_snapshot();
        assert_eq!(
            snapshot.song_position_source,
            MidiSongPositionSource::Unknown
        );
        let mut store = ClockSourceStore::new();
        store.insert_or_update(snapshot.clone());
        let input_report = midi_clock_input_report(
            RuntimeMidiClockInputRequest {
                source_id: "midi-clock-input".to_owned(),
                port_index: 99,
                duration_ms: 0,
            },
            None,
            false,
            store,
            snapshot,
            vec![diagnostic],
        );
        let input_text = format_midi_clock_input_report_text(&input_report);
        assert!(input_text.contains("opened: false"));
        assert!(input_text.contains("songPositionSource: Unknown"));
        assert!(input_text.contains("invalid-midi-input-port"));
    }

    #[test]
    fn host_monotonic_timestamp_is_elapsed_nanoseconds() {
        let first = host_monotonic_timestamp_ns();
        thread::sleep(Duration::from_millis(1));
        assert!(host_monotonic_timestamp_ns() > first);
    }

    #[test]
    fn poisoned_mutex_is_recovered_for_reports() {
        let mutex = Arc::new(Mutex::new(ClockSourceStore::new()));
        let mutex_for_thread = Arc::clone(&mutex);
        let _ = thread::spawn(move || {
            let _guard = mutex_for_thread.lock().unwrap();
            panic!("poison test");
        })
        .join();
        let store = clone_store(&mutex);
        assert!(store.is_empty());
    }
}
