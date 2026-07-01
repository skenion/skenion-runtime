use std::{collections::BTreeMap, fmt, path::Path};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::contract::{
    ClockAuthority, ClockCapability, ClockField, ClockState, ClockTimeSignature, MidiClockIssue,
    MidiClockIssueSeverity, MidiClockMessage, MidiClockMessageKind, MidiClockSnapshot,
    apply_midi_clock_message, midi_clock_snapshot_to_clock_state, parse_midi_clock_message,
};

pub const RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA: &str = "skenion.clock.midi.fixture";
pub const RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA_VERSION: &str = "0.1.0";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct RuntimeMidiClockSourceId(pub String);

impl RuntimeMidiClockSourceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RuntimeMidiClockSourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeMidiClockSourceKind {
    MidiClock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MidiSongPositionSource {
    Spp,
    StartReset,
    TickAccumulated,
    Unknown,
}

impl MidiSongPositionSource {
    fn label(self) -> &'static str {
        match self {
            Self::Spp => "runtime.midi-clock.spp",
            Self::StartReset => "runtime.midi-clock.start-reset",
            Self::TickAccumulated => "runtime.midi-clock.tick-accumulated",
            Self::Unknown => "runtime.midi-clock.unknown",
        }
    }

    fn song_position_authority(self) -> ClockAuthority {
        match self {
            Self::Spp => ClockAuthority::Authoritative,
            Self::StartReset | Self::TickAccumulated => ClockAuthority::Derived,
            Self::Unknown => ClockAuthority::Unavailable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeClockIssueSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeClockIssue {
    pub severity: RuntimeClockIssueSeverity,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMidiClockStateSnapshot {
    pub source_id: RuntimeMidiClockSourceId,
    pub source_kind: RuntimeMidiClockSourceKind,
    pub clock_state: ClockState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received_host_time_ns: Option<u64>,
    pub song_position_source: MidiSongPositionSource,
    pub issues: Vec<RuntimeClockIssue>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMidiClockTimeline {
    snapshots: BTreeMap<RuntimeMidiClockSourceId, RuntimeMidiClockStateSnapshot>,
}

impl RuntimeMidiClockTimeline {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, snapshot: RuntimeMidiClockStateSnapshot) {
        self.snapshots.insert(snapshot.source_id.clone(), snapshot);
    }

    pub fn get(
        &self,
        source_id: &RuntimeMidiClockSourceId,
    ) -> Option<&RuntimeMidiClockStateSnapshot> {
        self.snapshots.get(source_id)
    }

    pub fn list(&self) -> Vec<&RuntimeMidiClockStateSnapshot> {
        self.snapshots.values().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }

    pub fn len(&self) -> usize {
        self.snapshots.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimestampedMidiMessage {
    pub bytes: Vec<u8>,
    pub received_host_time_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMidiClockFixture {
    pub schema: String,
    pub schema_version: String,
    pub source_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_signature: Option<ClockTimeSignature>,
    pub events: Vec<RuntimeMidiClockFixtureEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMidiClockFixtureEvent {
    pub at_ns: u64,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMidiClockFixtureReport {
    pub schema: String,
    pub schema_version: String,
    pub source_id: RuntimeMidiClockSourceId,
    pub event_count: usize,
    pub timeline: RuntimeMidiClockTimeline,
    pub latest_snapshot: RuntimeMidiClockStateSnapshot,
    pub issues: Vec<RuntimeClockIssue>,
}

#[derive(Debug, Error)]
pub enum MidiClockFixtureError {
    #[error("failed to read MIDI Clock fixture {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse MIDI Clock fixture {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid MIDI Clock fixture {path}: {message}")]
    Invalid { path: String, message: String },
}

pub struct MidiClockAdapter {
    source_id: RuntimeMidiClockSourceId,
    snapshot: MidiClockSnapshot,
    song_position_source: MidiSongPositionSource,
    last_received_host_time_ns: Option<u64>,
}

impl MidiClockAdapter {
    pub fn new(source_id: impl Into<String>, time_signature: Option<ClockTimeSignature>) -> Self {
        let source_id = RuntimeMidiClockSourceId::new(source_id);
        let mut snapshot = MidiClockSnapshot::new(source_id.as_str());
        snapshot.time_signature = time_signature;
        Self {
            source_id,
            snapshot,
            song_position_source: MidiSongPositionSource::Unknown,
            last_received_host_time_ns: None,
        }
    }

    pub fn current_snapshot(&self) -> RuntimeMidiClockStateSnapshot {
        self.runtime_snapshot(Vec::new(), self.last_received_host_time_ns)
    }

    pub fn apply_timestamped_message(
        &mut self,
        timestamped: TimestampedMidiMessage,
    ) -> RuntimeMidiClockStateSnapshot {
        let received_host_time_ns = timestamped.received_host_time_ns;
        self.last_received_host_time_ns = Some(received_host_time_ns);

        let Some(mut message) = parse_runtime_midi_clock_message(&timestamped.bytes) else {
            let issue = invalid_or_unsupported_message_issue(&timestamped.bytes);
            return self.runtime_snapshot(vec![issue], Some(received_host_time_ns));
        };
        message.received_host_time_ns = Some(received_host_time_ns);

        let message_kind = message.kind;
        let result = apply_midi_clock_message(&self.snapshot, &message);
        self.snapshot = result.snapshot;
        let issues: Vec<RuntimeClockIssue> = result
            .issues
            .into_iter()
            .map(runtime_clock_issue_from_midi)
            .collect();

        if !issues
            .iter()
            .any(|issue| issue.severity == RuntimeClockIssueSeverity::Error)
        {
            self.update_song_position_source(message_kind);
        }

        self.runtime_snapshot(issues, Some(received_host_time_ns))
    }

    fn update_song_position_source(&mut self, message_kind: MidiClockMessageKind) {
        self.song_position_source = match message_kind {
            MidiClockMessageKind::Start => MidiSongPositionSource::StartReset,
            MidiClockMessageKind::SongPositionPointer => MidiSongPositionSource::Spp,
            MidiClockMessageKind::Tick => match self.song_position_source {
                MidiSongPositionSource::Unknown => MidiSongPositionSource::Unknown,
                MidiSongPositionSource::Spp
                | MidiSongPositionSource::StartReset
                | MidiSongPositionSource::TickAccumulated => {
                    MidiSongPositionSource::TickAccumulated
                }
            },
            MidiClockMessageKind::Continue | MidiClockMessageKind::Stop => {
                self.song_position_source
            }
        };
    }

    fn runtime_snapshot(
        &self,
        issues: Vec<RuntimeClockIssue>,
        received_host_time_ns: Option<u64>,
    ) -> RuntimeMidiClockStateSnapshot {
        RuntimeMidiClockStateSnapshot {
            source_id: self.source_id.clone(),
            source_kind: RuntimeMidiClockSourceKind::MidiClock,
            clock_state: self.corrected_clock_state(),
            received_host_time_ns,
            song_position_source: self.song_position_source,
            issues,
        }
    }

    fn corrected_clock_state(&self) -> ClockState {
        let mut state = midi_clock_snapshot_to_clock_state(&self.snapshot);
        let source = self.song_position_source.label();
        let authority = self.song_position_source.song_position_authority();
        let song_position_value = match authority {
            ClockAuthority::Unavailable => None,
            _ => Some(self.snapshot.song_position_sixteenth),
        };

        if authority == ClockAuthority::Unavailable {
            state.song_position_sixteenth =
                Some(clock_field(song_position_value, authority, source));
            state
                .capabilities
                .retain(|capability| capability != &ClockCapability::SongPosition);
            set_bar_beat_unavailable(&mut state, source);
        } else {
            state.song_position_sixteenth =
                Some(clock_field(song_position_value, authority.clone(), source));
            set_bar_beat_authority(&mut state, ClockAuthority::Derived, source);
        }
        state
    }
}

pub fn run_midi_clock_fixture_file(
    path: impl AsRef<Path>,
) -> Result<RuntimeMidiClockFixtureReport, MidiClockFixtureError> {
    let path = path.as_ref();
    let path_label = path.display().to_string();
    let bytes = std::fs::read(path).map_err(|source| MidiClockFixtureError::Read {
        path: path_label.clone(),
        source,
    })?;
    let fixture: RuntimeMidiClockFixture =
        serde_json::from_slice(&bytes).map_err(|source| MidiClockFixtureError::Parse {
            path: path_label.clone(),
            source,
        })?;
    run_midi_clock_fixture(fixture, path_label)
}

pub fn run_midi_clock_fixture(
    fixture: RuntimeMidiClockFixture,
    path_label: impl Into<String>,
) -> Result<RuntimeMidiClockFixtureReport, MidiClockFixtureError> {
    let path = path_label.into();
    validate_midi_clock_fixture(&fixture, &path)?;

    let mut adapter = MidiClockAdapter::new(fixture.source_id.clone(), fixture.time_signature);
    let mut timeline = RuntimeMidiClockTimeline::new();
    let mut issues = Vec::new();
    let mut latest_snapshot = adapter.current_snapshot();

    for event in &fixture.events {
        latest_snapshot = adapter.apply_timestamped_message(TimestampedMidiMessage {
            bytes: event.bytes.clone(),
            received_host_time_ns: event.at_ns,
        });
        issues.extend(latest_snapshot.issues.clone());
        timeline.record(latest_snapshot.clone());
    }

    if timeline.is_empty() {
        timeline.record(latest_snapshot.clone());
    }

    Ok(RuntimeMidiClockFixtureReport {
        schema: "skenion.runtime.clock-midi.report".to_owned(),
        schema_version: "0.1.0".to_owned(),
        source_id: RuntimeMidiClockSourceId::new(fixture.source_id),
        event_count: fixture.events.len(),
        timeline,
        latest_snapshot,
        issues,
    })
}

pub fn format_midi_clock_fixture_report_text(report: &RuntimeMidiClockFixtureReport) -> String {
    let snapshot = &report.latest_snapshot;
    let state = &snapshot.clock_state;
    let mut lines = vec![
        format!("runtime midi clock: {}", snapshot.source_id),
        format!("events: {}", report.event_count),
        format!("sourceKind: {:?}", snapshot.source_kind),
        format!("songPositionSource: {:?}", snapshot.song_position_source),
    ];
    push_field(&mut lines, "running", state.running.as_ref());
    push_field(
        &mut lines,
        "songPositionSixteenth",
        state.song_position_sixteenth.as_ref(),
    );
    push_field(&mut lines, "bar", state.bar.as_ref());
    push_field(&mut lines, "beat", state.beat.as_ref());
    push_field(&mut lines, "tempoBpm", state.tempo_bpm.as_ref());
    lines.push(format!("issues: {}", report.issues.len()));
    for issue in &report.issues {
        lines.push(format!(
            "issue: {:?} {} {}",
            issue.severity, issue.code, issue.message
        ));
    }
    lines.join("\n") + "\n"
}

fn validate_midi_clock_fixture(
    fixture: &RuntimeMidiClockFixture,
    path: &str,
) -> Result<(), MidiClockFixtureError> {
    if fixture.schema != RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA {
        return Err(MidiClockFixtureError::Invalid {
            path: path.to_owned(),
            message: format!(
                "expected schema {RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA}, got {}",
                fixture.schema
            ),
        });
    }
    if fixture.schema_version != RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA_VERSION {
        return Err(MidiClockFixtureError::Invalid {
            path: path.to_owned(),
            message: format!(
                "expected schemaVersion {RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA_VERSION}, got {}",
                fixture.schema_version
            ),
        });
    }
    if fixture.source_id.trim().is_empty() {
        return Err(MidiClockFixtureError::Invalid {
            path: path.to_owned(),
            message: "sourceId must be a non-empty string".to_owned(),
        });
    }
    for (index, event) in fixture.events.iter().enumerate() {
        if event.bytes.is_empty() {
            return Err(MidiClockFixtureError::Invalid {
                path: path.to_owned(),
                message: format!("events[{index}].bytes must be non-empty"),
            });
        }
    }
    Ok(())
}

fn parse_runtime_midi_clock_message(bytes: &[u8]) -> Option<MidiClockMessage> {
    parse_midi_clock_message(bytes).or_else(|| {
        (bytes.first() == Some(&0xf2)).then_some(MidiClockMessage {
            kind: MidiClockMessageKind::SongPositionPointer,
            song_position_sixteenth: None,
            received_host_time_ns: None,
        })
    })
}

fn invalid_or_unsupported_message_issue(_bytes: &[u8]) -> RuntimeClockIssue {
    RuntimeClockIssue {
        severity: RuntimeClockIssueSeverity::Error,
        code: "unsupported-midi-clock-message".to_owned(),
        message: "MIDI Clock adapter supports tick/start/continue/stop/SPP messages only"
            .to_owned(),
    }
}

fn clock_field<T>(value: Option<T>, authority: ClockAuthority, source: &str) -> ClockField<T> {
    ClockField {
        value,
        authority,
        source: source.to_owned(),
        confidence: None,
    }
}

fn unavailable_field<T>(source: &str) -> ClockField<T> {
    clock_field(None, ClockAuthority::Unavailable, source)
}

fn set_bar_beat_unavailable(state: &mut ClockState, source: &str) {
    state.bar = Some(unavailable_field(source));
    state.beat = Some(unavailable_field(source));
    state.division = Some(unavailable_field(source));
    state.tick_in_division = Some(unavailable_field(source));
    state
        .capabilities
        .retain(|capability| capability != &ClockCapability::BarBeat);
}

fn set_bar_beat_authority(state: &mut ClockState, authority: ClockAuthority, source: &str) {
    set_field_authority(&mut state.bar, authority.clone(), source);
    set_field_authority(&mut state.beat, authority.clone(), source);
    set_field_authority(&mut state.division, authority.clone(), source);
    set_field_authority(&mut state.tick_in_division, authority, source);
}

fn set_field_authority<T>(
    field: &mut Option<ClockField<T>>,
    authority: ClockAuthority,
    source: &str,
) {
    if let Some(field) = field.as_mut()
        && field.value.is_some()
    {
        field.authority = authority;
        field.source = source.to_owned();
    }
}

fn push_field<T: fmt::Display>(
    lines: &mut Vec<String>,
    label: &str,
    field: Option<&ClockField<T>>,
) {
    let Some(field) = field else {
        lines.push(format!("{label}: <missing>"));
        return;
    };
    let value = field
        .value
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| "unavailable".to_owned());
    lines.push(format!(
        "{label}: {value} authority={:?} source={}",
        field.authority, field.source
    ));
}

fn runtime_clock_issue_from_midi(issue: MidiClockIssue) -> RuntimeClockIssue {
    RuntimeClockIssue {
        severity: match issue.severity {
            MidiClockIssueSeverity::Warning => RuntimeClockIssueSeverity::Warning,
            MidiClockIssueSeverity::Error => RuntimeClockIssueSeverity::Error,
        },
        code: issue.code,
        message: issue.message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulated_start_ticks_and_stop_update_timeline_snapshot() {
        let fixture = RuntimeMidiClockFixture {
            schema: RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA.to_owned(),
            schema_version: RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA_VERSION.to_owned(),
            source_id: "midi-clock-start-stop".to_owned(),
            time_signature: Some(ClockTimeSignature {
                numerator: 4,
                denominator: 4,
            }),
            events: vec![
                event(0, &[0xfa]),
                event(1_000_000, &[0xf8]),
                event(2_000_000, &[0xfc]),
            ],
        };

        let report = run_midi_clock_fixture(fixture, "inline").unwrap();
        let snapshot = &report.latest_snapshot;

        assert_eq!(report.event_count, 3);
        assert_eq!(report.timeline.len(), 1);
        assert!(snapshot.clock_state.running.as_ref().unwrap().value == Some(false));
        assert_eq!(
            snapshot.song_position_source,
            MidiSongPositionSource::TickAccumulated
        );
        assert_eq!(
            snapshot
                .clock_state
                .song_position_sixteenth
                .as_ref()
                .unwrap()
                .authority,
            ClockAuthority::Derived
        );
        assert!(report.issues.is_empty());
    }

    #[test]
    fn spp_position_is_authoritative_and_meter_derives_bar_beat() {
        let mut adapter = MidiClockAdapter::new(
            "midi-clock-spp",
            Some(ClockTimeSignature {
                numerator: 4,
                denominator: 4,
            }),
        );

        let snapshot = adapter.apply_timestamped_message(TimestampedMidiMessage {
            bytes: vec![0xf2, 16, 0],
            received_host_time_ns: 10,
        });

        assert_eq!(snapshot.song_position_source, MidiSongPositionSource::Spp);
        assert_eq!(
            snapshot
                .clock_state
                .song_position_sixteenth
                .as_ref()
                .unwrap()
                .authority,
            ClockAuthority::Authoritative
        );
        assert_eq!(snapshot.clock_state.bar.as_ref().unwrap().value, Some(2));
        assert_eq!(
            snapshot.clock_state.bar.as_ref().unwrap().authority,
            ClockAuthority::Derived
        );
        assert_eq!(snapshot.clock_state.beat.as_ref().unwrap().value, Some(1));
    }

    #[test]
    fn continue_without_position_anchor_leaves_song_position_unavailable() {
        let mut adapter = MidiClockAdapter::new("midi-clock-continue", None);

        adapter.apply_timestamped_message(TimestampedMidiMessage {
            bytes: vec![0xfb],
            received_host_time_ns: 10,
        });
        let snapshot = adapter.apply_timestamped_message(TimestampedMidiMessage {
            bytes: vec![0xf8],
            received_host_time_ns: 20,
        });

        let field = snapshot
            .clock_state
            .song_position_sixteenth
            .as_ref()
            .unwrap();
        assert_eq!(
            snapshot.song_position_source,
            MidiSongPositionSource::Unknown
        );
        assert_eq!(field.value, None);
        assert_eq!(field.authority, ClockAuthority::Unavailable);
        assert!(
            !snapshot
                .clock_state
                .capabilities
                .contains(&ClockCapability::SongPosition)
        );
    }

    #[test]
    fn meterless_spp_leaves_bar_and_beat_unavailable() {
        let mut adapter = MidiClockAdapter::new("midi-clock-meterless", None);

        let snapshot = adapter.apply_timestamped_message(TimestampedMidiMessage {
            bytes: vec![0xf2, 16, 0],
            received_host_time_ns: 10,
        });

        assert_eq!(
            snapshot.clock_state.bar.as_ref().unwrap().authority,
            ClockAuthority::Unavailable
        );
        assert_eq!(snapshot.clock_state.bar.as_ref().unwrap().value, None);
        assert_eq!(
            snapshot.clock_state.beat.as_ref().unwrap().authority,
            ClockAuthority::Unavailable
        );
    }

    #[test]
    fn invalid_spp_bytes_produce_issue_without_state_change() {
        let mut adapter = MidiClockAdapter::new("midi-clock-invalid", None);

        let snapshot = adapter.apply_timestamped_message(TimestampedMidiMessage {
            bytes: vec![0xf2, 0x80, 0],
            received_host_time_ns: 10,
        });

        assert_eq!(snapshot.issues.len(), 1);
        assert_eq!(
            snapshot.issues[0].code,
            "invalid-midi-song-position-pointer"
        );
        assert_eq!(
            snapshot.song_position_source,
            MidiSongPositionSource::Unknown
        );
    }

    #[test]
    fn start_reset_position_is_derived_until_ticks_accumulate() {
        let mut adapter = MidiClockAdapter::new("midi-clock-start", None);

        let snapshot = adapter.apply_timestamped_message(TimestampedMidiMessage {
            bytes: vec![0xfa],
            received_host_time_ns: 5,
        });

        assert_eq!(
            snapshot.song_position_source,
            MidiSongPositionSource::StartReset
        );
        let field = snapshot
            .clock_state
            .song_position_sixteenth
            .as_ref()
            .unwrap();
        assert_eq!(field.value, Some(0));
        assert_eq!(field.authority, ClockAuthority::Derived);
        assert_eq!(field.source, "runtime.midi-clock.start-reset");
    }

    #[test]
    fn midi_clock_timeline_gets_and_lists_snapshots() {
        let adapter = MidiClockAdapter::new("midi-clock-timeline", None);
        let snapshot = adapter.current_snapshot();
        let source_id = snapshot.source_id.clone();
        let mut timeline = RuntimeMidiClockTimeline::new();

        assert!(timeline.is_empty());
        timeline.record(snapshot);

        assert_eq!(source_id.as_str(), "midi-clock-timeline");
        assert_eq!(source_id.to_string(), "midi-clock-timeline");
        assert_eq!(timeline.len(), 1);
        assert_eq!(timeline.get(&source_id).unwrap().source_id, source_id);
        assert_eq!(timeline.list().len(), 1);
    }

    #[test]
    fn fixture_file_runs_and_text_report_formats_fields() {
        let path = temp_fixture_path("valid-empty");
        std::fs::write(
            &path,
            r#"{
              "schema": "skenion.clock.midi.fixture",
              "schemaVersion": "0.1.0",
              "sourceId": "midi-clock-empty",
              "events": []
            }"#,
        )
        .unwrap();

        let report = run_midi_clock_fixture_file(&path).unwrap();
        let text = format_midi_clock_fixture_report_text(&report);
        std::fs::remove_file(path).unwrap();

        assert_eq!(report.event_count, 0);
        assert_eq!(report.timeline.len(), 1);
        assert!(text.contains("runtime midi clock: midi-clock-empty"));
        assert!(text.contains("songPositionSource: Unknown"));
        assert!(text.contains("tempoBpm: unavailable authority=Unavailable"));

        let inline_empty = run_midi_clock_fixture(
            RuntimeMidiClockFixture {
                schema: RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA.to_owned(),
                schema_version: RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA_VERSION.to_owned(),
                source_id: "midi-clock-inline-empty".to_owned(),
                time_signature: None,
                events: vec![],
            },
            "inline-empty",
        )
        .unwrap();
        assert_eq!(inline_empty.timeline.len(), 1);

        let string_label_report = run_midi_clock_fixture(
            RuntimeMidiClockFixture {
                schema: RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA.to_owned(),
                schema_version: RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA_VERSION.to_owned(),
                source_id: "midi-clock-string-label".to_owned(),
                time_signature: None,
                events: vec![event(0, &[0xfa])],
            },
            "inline-string-label".to_owned(),
        )
        .unwrap();
        assert_eq!(string_label_report.event_count, 1);
    }

    #[test]
    fn fixture_file_reports_read_parse_and_validation_errors() {
        let missing = temp_fixture_path("missing");
        let read_error = run_midi_clock_fixture_file(&missing).unwrap_err();
        assert!(read_error.to_string().contains("failed to read"));

        let parse_path = temp_fixture_path("parse");
        std::fs::write(&parse_path, "{").unwrap();
        let parse_error = run_midi_clock_fixture_file(&parse_path).unwrap_err();
        std::fs::remove_file(parse_path).unwrap();
        assert!(parse_error.to_string().contains("failed to parse"));

        let invalid_cases = [
            (
                RuntimeMidiClockFixture {
                    schema: "wrong".to_owned(),
                    schema_version: RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA_VERSION.to_owned(),
                    source_id: "source".to_owned(),
                    time_signature: None,
                    events: vec![],
                },
                "expected schema",
            ),
            (
                RuntimeMidiClockFixture {
                    schema: RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA.to_owned(),
                    schema_version: "9.9.9".to_owned(),
                    source_id: "source".to_owned(),
                    time_signature: None,
                    events: vec![],
                },
                "expected schemaVersion",
            ),
            (
                RuntimeMidiClockFixture {
                    schema: RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA.to_owned(),
                    schema_version: RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA_VERSION.to_owned(),
                    source_id: " ".to_owned(),
                    time_signature: None,
                    events: vec![],
                },
                "sourceId must be",
            ),
            (
                RuntimeMidiClockFixture {
                    schema: RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA.to_owned(),
                    schema_version: RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA_VERSION.to_owned(),
                    source_id: "source".to_owned(),
                    time_signature: None,
                    events: vec![event(0, &[])],
                },
                "bytes must be",
            ),
        ];

        for (fixture, expected) in invalid_cases {
            let error = run_midi_clock_fixture(fixture, "inline").unwrap_err();
            assert!(error.to_string().contains(expected));
        }
    }

    #[test]
    fn unsupported_messages_and_tick_overflow_report_issues() {
        let mut adapter = MidiClockAdapter::new("midi-clock-issues", None);

        let unsupported = adapter.apply_timestamped_message(TimestampedMidiMessage {
            bytes: vec![0x90, 60, 127],
            received_host_time_ns: 1,
        });
        assert_eq!(unsupported.issues.len(), 1);
        assert_eq!(unsupported.issues[0].code, "unsupported-midi-clock-message");

        let invalid_spp = adapter.apply_timestamped_message(TimestampedMidiMessage {
            bytes: vec![0xf2],
            received_host_time_ns: 2,
        });
        assert_eq!(
            invalid_spp.issues[0].code,
            "invalid-midi-song-position-pointer"
        );

        adapter.snapshot.tick_index = u64::MAX;
        adapter.song_position_source = MidiSongPositionSource::StartReset;
        let overflow = adapter.apply_timestamped_message(TimestampedMidiMessage {
            bytes: vec![0xf8],
            received_host_time_ns: 3,
        });
        assert!(
            overflow
                .issues
                .iter()
                .any(|issue| issue.code == "midi-clock-tick-overflow"
                    && issue.severity == RuntimeClockIssueSeverity::Warning)
        );
        assert_eq!(
            overflow.song_position_source,
            MidiSongPositionSource::TickAccumulated
        );
    }

    #[test]
    fn invalid_fixture_text_report_formats_issues_and_error_severity() {
        let report = run_midi_clock_fixture(
            RuntimeMidiClockFixture {
                schema: RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA.to_owned(),
                schema_version: RUNTIME_MIDI_CLOCK_FIXTURE_SCHEMA_VERSION.to_owned(),
                source_id: "midi-clock-invalid-spp".to_owned(),
                time_signature: None,
                events: vec![event(0, &[0xf2])],
            },
            "inline",
        )
        .unwrap();
        let text = format_midi_clock_fixture_report_text(&report);

        assert!(text.contains("issues: 1"));
        assert!(text.contains("issue:"));
        assert!(text.contains("invalid-midi-song-position-pointer"));

        let issue = runtime_clock_issue_from_midi(MidiClockIssue {
            severity: MidiClockIssueSeverity::Error,
            code: "runtime-midi-clock-error".to_owned(),
            message: "runtime MIDI Clock error".to_owned(),
        });
        assert_eq!(issue.severity, RuntimeClockIssueSeverity::Error);
    }

    #[test]
    fn private_format_helpers_cover_missing_and_none_fields() {
        let mut lines = Vec::new();
        push_field::<bool>(&mut lines, "missingBool", None);
        push_field::<f64>(&mut lines, "missingFloat", None);
        push_field::<u64>(&mut lines, "missing", None);
        assert_eq!(lines[0], "missingBool: <missing>");
        assert_eq!(lines[1], "missingFloat: <missing>");
        assert_eq!(lines[2], "missing: <missing>");

        let bool_field = clock_field::<bool>(None, ClockAuthority::Unavailable, "bool-source");
        push_field(&mut lines, "unavailableBool", Some(&bool_field));
        assert_eq!(
            lines[3],
            "unavailableBool: unavailable authority=Unavailable source=bool-source"
        );

        let mut field = Some(clock_field(
            Some(1_u64),
            ClockAuthority::Authoritative,
            "before",
        ));
        set_field_authority(&mut field, ClockAuthority::Derived, "after");
        let field = field.unwrap();
        assert_eq!(field.authority, ClockAuthority::Derived);
        assert_eq!(field.source, "after");

        let mut unavailable: Option<ClockField<u64>> = None;
        set_field_authority(&mut unavailable, ClockAuthority::Derived, "after");
        assert!(unavailable.is_none());
    }

    fn event(at_ns: u64, bytes: &[u8]) -> RuntimeMidiClockFixtureEvent {
        RuntimeMidiClockFixtureEvent {
            at_ns,
            bytes: bytes.to_vec(),
        }
    }

    fn temp_fixture_path(label: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("skenion-runtime-midi-clock-{label}-{nonce}.json"))
    }
}
