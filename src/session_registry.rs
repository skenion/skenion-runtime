use std::{
    collections::{BTreeMap, VecDeque},
    convert::Infallible,
    sync::{Arc, Mutex, RwLock},
};

use axum::{http::HeaderMap, response::sse::Event};
use serde::Deserialize;
use serde_json::Value;
use skenion_contracts::{
    RuntimeConnectionProfile, RuntimeConnectionProfileMode, RuntimeEventReplayGap,
    RuntimeEventReplayGapReason, RuntimeEventReplayMetadata, RuntimeEventReplayWindow,
    RuntimeHistory as ContractRuntimeHistory, RuntimeHistoryEntry as ContractRuntimeHistoryEntry,
    RuntimeSessionCapabilitySet, RuntimeSessionInfoResponse, RuntimeSessionLifecycleState,
    validate_runtime_session_event,
};
pub use skenion_contracts::{RuntimeSessionEvent, RuntimeSessionEventKind};
use tokio::sync::broadcast;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::{
    COLLABORATION_EVENT_REPLAY_LIMIT, PreviewManager, RuntimeCollaborationLog, RuntimeDiagnostic,
    RuntimeSession, RuntimeSessionSnapshot, runtime_time::created_at_now,
};

pub const DEFAULT_SESSION_ID: &str = "default";
const SESSION_EVENT_REPLAY_LIMIT: usize = 256;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEventsQuery {
    pub after: Option<u64>,
}

#[derive(Clone)]
pub struct RuntimeSessionRegistry {
    sessions: Arc<RwLock<BTreeMap<String, RuntimeSessionRecord>>>,
    default_session_id: String,
    replay_limit: usize,
    dry_preview: bool,
}

impl Default for RuntimeSessionRegistry {
    fn default() -> Self {
        Self::new(false)
    }
}

impl RuntimeSessionRegistry {
    pub fn new(dry_preview: bool) -> Self {
        let registry = Self {
            sessions: Arc::new(RwLock::new(BTreeMap::new())),
            default_session_id: DEFAULT_SESSION_ID.to_owned(),
            replay_limit: SESSION_EVENT_REPLAY_LIMIT,
            dry_preview,
        };
        registry.get_or_create(DEFAULT_SESSION_ID);
        registry
    }

    pub fn dry_preview() -> Self {
        Self::new(true)
    }

    pub fn default_session_id(&self) -> &str {
        &self.default_session_id
    }

    pub fn default_record(&self) -> RuntimeSessionRecord {
        self.get_or_create(self.default_session_id())
    }

    pub fn get_or_create(&self, session_id: &str) -> RuntimeSessionRecord {
        let session_id = if session_id.is_empty() {
            self.default_session_id()
        } else {
            session_id
        };
        if let Some(record) = self
            .sessions
            .read()
            .expect("runtime session registry lock should not be poisoned")
            .get(session_id)
            .cloned()
        {
            return record;
        }

        let mut sessions = self
            .sessions
            .write()
            .expect("runtime session registry lock should not be poisoned");
        sessions
            .entry(session_id.to_owned())
            .or_insert_with(|| {
                RuntimeSessionRecord::new(session_id, self.replay_limit, self.dry_preview)
            })
            .clone()
    }
}

#[derive(Clone)]
pub struct RuntimeSessionRecord {
    pub id: String,
    pub session: Arc<RwLock<RuntimeSession>>,
    pub events: broadcast::Sender<RuntimeSessionEvent>,
    pub collaboration: Arc<RuntimeCollaborationLog>,
    pub event_store: Arc<Mutex<VecDeque<RuntimeSessionEvent>>>,
    pub event_sequence: Arc<Mutex<u64>>,
    pub preview: Arc<Mutex<PreviewManager>>,
    replay_limit: usize,
}

#[derive(Debug, Clone)]
pub struct RuntimeSessionReplay {
    pub events: Vec<RuntimeSessionEvent>,
    pub high_water_sequence: u64,
}

impl RuntimeSessionRecord {
    fn new(session_id: &str, replay_limit: usize, dry_preview: bool) -> Self {
        let (events, _) = broadcast::channel(replay_limit);
        Self {
            id: session_id.to_owned(),
            session: Arc::new(RwLock::new(RuntimeSession::default())),
            events,
            collaboration: RuntimeCollaborationLog::new(COLLABORATION_EVENT_REPLAY_LIMIT),
            event_store: Arc::new(Mutex::new(VecDeque::new())),
            event_sequence: Arc::new(Mutex::new(1)),
            preview: Arc::new(Mutex::new(if dry_preview {
                PreviewManager::dry_run()
            } else {
                PreviewManager::from_env()
            })),
            replay_limit,
        }
    }

    pub fn event_replay_window(&self) -> RuntimeEventReplayWindow {
        let store = self
            .event_store
            .lock()
            .expect("runtime session event store lock should not be poisoned");
        let earliest_sequence = store.front().map(|event| event.sequence).unwrap_or(1);
        let latest_sequence = store
            .back()
            .map(|event| event.sequence)
            .unwrap_or_else(|| current_session_event_sequence(self));
        RuntimeEventReplayWindow {
            cursor_kind: "sequence".to_owned(),
            current_cursor: latest_sequence.to_string(),
            earliest_sequence,
            latest_sequence,
            replay_limit: Some(self.replay_limit as u64),
            overflow: Some(store.len() >= self.replay_limit),
        }
    }

    pub fn info_response(&self, profile: RuntimeConnectionProfile) -> RuntimeSessionInfoResponse {
        let snapshot = {
            let session = self
                .session
                .read()
                .expect("runtime session lock should not be poisoned");
            session.snapshot()
        };
        let mut diagnostics = Vec::new();
        for diagnostic in &snapshot.diagnostics {
            diagnostics.push(
                serde_json::to_value(diagnostic)
                    .expect("runtime diagnostic should serialize to JSON"),
            );
        }
        RuntimeSessionInfoResponse {
            schema: "skenion.runtime.session.info".to_owned(),
            schema_version: "0.1.0".to_owned(),
            ok: true,
            session_id: self.id.clone(),
            lifecycle: RuntimeSessionLifecycleState::Ready,
            snapshot: contract_session_snapshot(&snapshot),
            profile,
            capabilities: runtime_session_capabilities(),
            event_replay: self.event_replay_window(),
            diagnostics,
        }
    }
}

pub fn publish_session_event(
    record: &RuntimeSessionRecord,
    kind: RuntimeSessionEventKind,
    session: &RuntimeSession,
    diagnostics: Vec<RuntimeDiagnostic>,
) {
    let sequence = next_session_event_sequence(record);
    let event = session_event_from_session(
        record,
        kind,
        session,
        event_replay_fields(session_event_id(&record.id, sequence), sequence),
        diagnostics,
    );
    store_session_event(record, event.clone());
    let _ = record.events.send(event);
}

fn session_event_from_session(
    record: &RuntimeSessionRecord,
    kind: RuntimeSessionEventKind,
    session: &RuntimeSession,
    replay: SessionEventReplayFields,
    diagnostics: Vec<RuntimeDiagnostic>,
) -> RuntimeSessionEvent {
    let snapshot = session.snapshot();
    let history = session.history();
    let event = RuntimeSessionEvent {
        schema: "skenion.runtime.session.event".to_owned(),
        schema_version: "0.1.0".to_owned(),
        id: replay.id,
        session_id: record.id.clone(),
        sequence: replay.sequence,
        session_revision: snapshot.session_revision,
        kind,
        snapshot: contract_session_snapshot(&snapshot),
        mutation: history.entries.last().map(contract_history_entry),
        history: contract_runtime_history(&history),
        replay: RuntimeEventReplayMetadata {
            cursor: replay.cursor,
            previous_cursor: replay.previous_cursor,
            replayed: replay.replayed,
            gap: replay.gap,
            overflow: replay.overflow,
        },
        diagnostics: contract_diagnostics(&diagnostics),
        created_at: created_at_now(),
    };
    validate_runtime_session_event(&event).expect("runtime session event should validate");
    event
}

pub fn session_snapshot_event(
    record: &RuntimeSessionRecord,
    session: &RuntimeSession,
) -> RuntimeSessionEvent {
    let latest_sequence = current_session_event_sequence(record);
    let sequence = latest_sequence.max(1);
    let previous_cursor = if latest_sequence == 0 {
        None
    } else {
        previous_cursor_for(latest_sequence)
    };
    session_event_from_session(
        record,
        RuntimeSessionEventKind::Snapshot,
        session,
        SessionEventReplayFields {
            id: format!("{}_snapshot_{latest_sequence:06}", record.id),
            sequence,
            cursor: latest_sequence.to_string(),
            previous_cursor,
            replayed: false,
            gap: None,
            overflow: false,
        },
        Vec::new(),
    )
}

struct SessionEventReplayFields {
    id: String,
    sequence: u64,
    cursor: String,
    previous_cursor: Option<String>,
    replayed: bool,
    gap: Option<RuntimeEventReplayGap>,
    overflow: bool,
}

fn event_replay_fields(id: String, sequence: u64) -> SessionEventReplayFields {
    SessionEventReplayFields {
        id,
        sequence,
        cursor: sequence.to_string(),
        previous_cursor: previous_cursor_for(sequence),
        replayed: false,
        gap: None,
        overflow: false,
    }
}

fn next_session_event_sequence(record: &RuntimeSessionRecord) -> u64 {
    let mut sequence = record
        .event_sequence
        .lock()
        .expect("runtime session event sequence lock should not be poisoned");
    let current = *sequence;
    *sequence += 1;
    current
}

pub fn current_session_event_sequence(record: &RuntimeSessionRecord) -> u64 {
    let next_sequence = record
        .event_sequence
        .lock()
        .expect("runtime session event sequence lock should not be poisoned");
    next_sequence.saturating_sub(1)
}

fn previous_cursor_for(sequence: u64) -> Option<String> {
    sequence
        .checked_sub(1)
        .filter(|previous| *previous > 0)
        .map(|previous| previous.to_string())
}

fn session_event_id(session_id: &str, sequence: u64) -> String {
    format!("{session_id}_event_{sequence:06}")
}

fn store_session_event(record: &RuntimeSessionRecord, event: RuntimeSessionEvent) {
    let mut store = record
        .event_store
        .lock()
        .expect("runtime session event store lock should not be poisoned");
    store.push_back(event);
    while store.len() > record.replay_limit {
        store.pop_front();
    }
}

pub fn capture_session_replay(
    record: &RuntimeSessionRecord,
    after: Option<u64>,
    snapshot: RuntimeSessionEvent,
) -> RuntimeSessionReplay {
    let high_water_sequence = latest_stored_session_event_sequence(record)
        .unwrap_or_else(|| current_session_event_sequence(record));
    let Some(after) = after else {
        return RuntimeSessionReplay {
            events: vec![snapshot],
            high_water_sequence,
        };
    };
    let store = record
        .event_store
        .lock()
        .expect("runtime session event store lock should not be poisoned");
    let earliest = store.front().map(|event| event.sequence);
    let mut replay = Vec::new();
    for event in store.iter().filter(|event| event.sequence > after) {
        let mut event = event.clone();
        event.replay.replayed = true;
        validate_runtime_session_event(&event).expect("runtime replay event should validate");
        replay.push(event);
    }
    if let Some(earliest) = earliest
        && after + 1 < earliest
    {
        let gap = replay_gap_event(
            snapshot,
            &record.id,
            after,
            earliest,
            RuntimeEventReplayGapReason::RetentionOverflow,
        );
        replay.insert(0, gap);
    }
    RuntimeSessionReplay {
        events: replay,
        high_water_sequence,
    }
}

fn replay_gap_event(
    mut snapshot: RuntimeSessionEvent,
    session_id: &str,
    after: u64,
    actual_sequence: u64,
    reason: RuntimeEventReplayGapReason,
) -> RuntimeSessionEvent {
    let expected_sequence = after + 1;
    snapshot.id = format!("{session_id}_gap_{expected_sequence:06}_{actual_sequence:06}");
    snapshot.sequence = expected_sequence;
    snapshot.replay = RuntimeEventReplayMetadata {
        cursor: expected_sequence.to_string(),
        previous_cursor: if after == 0 {
            None
        } else {
            Some(after.to_string())
        },
        replayed: true,
        gap: Some(RuntimeEventReplayGap {
            expected_sequence,
            actual_sequence,
            reason,
        }),
        overflow: true,
    };
    validate_runtime_session_event(&snapshot).expect("runtime replay gap event should validate");
    snapshot
}

pub fn event_cursor_from_headers(headers: &HeaderMap) -> Option<u64> {
    match headers.get("last-event-id") {
        Some(value) => value.to_str().ok().and_then(|cursor| cursor.parse().ok()),
        None => None,
    }
}

pub fn session_broadcast_event_after_high_water(
    result: Result<RuntimeSessionEvent, BroadcastStreamRecvError>,
    record: RuntimeSessionRecord,
    high_water_sequence: u64,
) -> Option<Result<Event, Infallible>> {
    match result {
        Ok(event) if event.sequence <= high_water_sequence => None,
        Ok(event) => Some(session_event(event)),
        Err(BroadcastStreamRecvError::Lagged(skipped)) => {
            Some(session_event(session_lag_gap_event(&record, skipped)))
        }
    }
}

pub fn session_event(event: RuntimeSessionEvent) -> Result<Event, Infallible> {
    Ok(Event::default()
        .event("session")
        .id(event.replay.cursor.clone())
        .json_data(event)
        .expect("runtime session event should serialize"))
}

pub fn session_lag_gap_event(record: &RuntimeSessionRecord, skipped: u64) -> RuntimeSessionEvent {
    let actual_sequence = oldest_retained_session_event_sequence(record)
        .unwrap_or_else(|| current_session_event_sequence(record))
        .max(2);
    let skipped = skipped.max(1);
    let expected_sequence = actual_sequence.saturating_sub(skipped).max(1);
    let session = record
        .session
        .read()
        .expect("runtime session lock should not be poisoned");
    session_event_from_session(
        record,
        RuntimeSessionEventKind::Snapshot,
        &session,
        SessionEventReplayFields {
            id: format!(
                "{}_stream_gap_{expected_sequence:06}_{actual_sequence:06}",
                record.id
            ),
            sequence: expected_sequence,
            cursor: expected_sequence.to_string(),
            previous_cursor: previous_cursor_for(expected_sequence),
            replayed: true,
            gap: Some(RuntimeEventReplayGap {
                expected_sequence,
                actual_sequence,
                reason: RuntimeEventReplayGapReason::StreamReset,
            }),
            overflow: true,
        },
        Vec::new(),
    )
}

fn oldest_retained_session_event_sequence(record: &RuntimeSessionRecord) -> Option<u64> {
    record
        .event_store
        .lock()
        .expect("runtime session event store lock should not be poisoned")
        .front()
        .map(|event| event.sequence)
}

fn latest_stored_session_event_sequence(record: &RuntimeSessionRecord) -> Option<u64> {
    record
        .event_store
        .lock()
        .expect("runtime session event store lock should not be poisoned")
        .back()
        .map(|event| event.sequence)
}

fn contract_session_snapshot(
    snapshot: &RuntimeSessionSnapshot,
) -> skenion_contracts::RuntimeSessionSnapshot {
    serde_json::from_value(
        serde_json::to_value(snapshot).expect("runtime session snapshot should serialize"),
    )
    .expect("runtime session snapshot should match contract shape")
}

fn contract_runtime_history(history: &crate::RuntimeHistory) -> ContractRuntimeHistory {
    serde_json::from_value(serde_json::to_value(history).expect("runtime history should serialize"))
        .expect("runtime history should match contract shape")
}

fn contract_history_entry(entry: &crate::RuntimeHistoryEntry) -> ContractRuntimeHistoryEntry {
    serde_json::from_value(
        serde_json::to_value(entry).expect("runtime history entry should serialize"),
    )
    .expect("runtime history entry should match contract shape")
}

fn contract_diagnostics(diagnostics: &[RuntimeDiagnostic]) -> Vec<Value> {
    let mut values = Vec::new();
    for diagnostic in diagnostics {
        values.push(
            serde_json::to_value(diagnostic).expect("runtime diagnostic should serialize to JSON"),
        );
    }
    values
}

fn runtime_session_capabilities() -> RuntimeSessionCapabilitySet {
    RuntimeSessionCapabilitySet {
        session_addressing: true,
        default_session_alias: true,
        event_replay: true,
        multi_window: true,
        profiles: vec![
            RuntimeConnectionProfileMode::LocalManaged,
            RuntimeConnectionProfileMode::LocalShared,
            RuntimeConnectionProfileMode::Remote,
        ],
        auth_policy: "deferred".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};
    use skenion_contracts::RuntimeEventReplayGapReason;
    use tokio_stream::{StreamExt, wrappers::BroadcastStream};

    use super::*;
    use crate::sidecar::RuntimeEndpointConfig;

    #[test]
    fn registry_creates_default_alias_and_named_records() {
        let registry = RuntimeSessionRegistry::dry_preview();

        assert_eq!(registry.default_record().id, DEFAULT_SESSION_ID);
        assert_eq!(registry.get_or_create("").id, DEFAULT_SESSION_ID);
        assert_eq!(registry.get_or_create("alpha").id, "alpha");
    }

    #[test]
    fn per_session_events_are_monotonic_and_replay_gaps_are_deterministic() {
        let record = RuntimeSessionRecord::new("gap-test", 2, true);
        let session = RuntimeSession::default();

        for _ in 0..3 {
            publish_session_event(
                &record,
                RuntimeSessionEventKind::Snapshot,
                &session,
                Vec::new(),
            );
        }
        let snapshot = session_snapshot_event(&record, &session);
        let replay = capture_session_replay(&record, Some(0), snapshot).events;
        let gap = replay[0]
            .replay
            .gap
            .as_ref()
            .expect("replay should include retention gap");

        assert_eq!(
            replay
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(gap.expected_sequence, 1);
        assert_eq!(gap.actual_sequence, 2);
        assert_eq!(gap.reason, RuntimeEventReplayGapReason::RetentionOverflow);
        for event in replay {
            validate_runtime_session_event(&event).expect("replayed event should validate");
        }
    }

    #[test]
    fn retention_gap_after_nonzero_cursor_preserves_previous_cursor() {
        let record = RuntimeSessionRecord::new("gap-after-test", 2, true);
        let session = RuntimeSession::default();

        for _ in 0..4 {
            publish_session_event(
                &record,
                RuntimeSessionEventKind::Snapshot,
                &session,
                Vec::new(),
            );
        }
        let replay =
            capture_session_replay(&record, Some(1), session_snapshot_event(&record, &session))
                .events;
        let gap = replay[0]
            .replay
            .gap
            .as_ref()
            .expect("replay should include retention gap");

        assert_eq!(
            replay
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
        assert_eq!(replay[0].replay.previous_cursor.as_deref(), Some("1"));
        assert_eq!(gap.expected_sequence, 2);
        assert_eq!(gap.actual_sequence, 3);
    }

    #[test]
    fn stream_snapshot_does_not_consume_canonical_sequence() {
        let record = RuntimeSessionRecord::new("snapshot-test", 2, true);
        let session = RuntimeSession::default();

        let snapshot = session_snapshot_event(&record, &session);

        assert_eq!(current_session_event_sequence(&record), 0);
        assert_eq!(snapshot.sequence, 1);
        assert_eq!(snapshot.replay.cursor, "0");
        validate_runtime_session_event(&snapshot).expect("snapshot event should validate");

        publish_session_event(
            &record,
            RuntimeSessionEventKind::Snapshot,
            &session,
            vec![RuntimeDiagnostic::warning("covered diagnostic")],
        );
        let stored = record
            .event_store
            .lock()
            .expect("event store should not be poisoned")
            .front()
            .cloned()
            .expect("published event should be stored");
        assert_eq!(stored.sequence, 1);
        assert_eq!(stored.replay.cursor, "1");
        assert_eq!(stored.diagnostics[0]["message"], "covered diagnostic");
        assert_eq!(current_session_event_sequence(&record), 1);
    }

    #[test]
    fn live_lag_gap_is_contract_shaped() {
        let record = RuntimeSessionRecord::new("lag-test", 2, true);
        let session = RuntimeSession::default();
        for _ in 0..3 {
            publish_session_event(
                &record,
                RuntimeSessionEventKind::Snapshot,
                &session,
                Vec::new(),
            );
        }

        let gap = session_lag_gap_event(&record, 99);

        assert_eq!(gap.session_id, "lag-test");
        assert_eq!(
            gap.replay.gap.as_ref().unwrap().reason,
            RuntimeEventReplayGapReason::StreamReset
        );
        assert_eq!(gap.replay.gap.as_ref().unwrap().expected_sequence, 1);
        assert_eq!(gap.replay.gap.as_ref().unwrap().actual_sequence, 2);
        validate_runtime_session_event(&gap).expect("live lag gap event should validate");
    }

    #[tokio::test]
    async fn broadcast_lag_gap_and_retained_live_events_are_monotonic() {
        let record = RuntimeSessionRecord::new("lag-order-test", 2, true);
        let session = RuntimeSession::default();
        let receiver = record.events.subscribe();
        for _ in 0..5 {
            publish_session_event(
                &record,
                RuntimeSessionEventKind::Snapshot,
                &session,
                Vec::new(),
            );
        }

        let mut stream = BroadcastStream::new(receiver);
        let BroadcastStreamRecvError::Lagged(skipped) = stream
            .next()
            .await
            .expect("lagged stream should yield an item")
            .expect_err("first stream item should be a lag error");
        let gap = session_lag_gap_event(&record, skipped);
        let first_retained = stream
            .next()
            .await
            .expect("first retained event should arrive")
            .expect("first retained event should be ok");
        let second_retained = stream
            .next()
            .await
            .expect("second retained event should arrive")
            .expect("second retained event should be ok");
        let sequences = vec![
            gap.sequence,
            first_retained.sequence,
            second_retained.sequence,
        ];

        assert_eq!(
            gap.replay.gap.as_ref().unwrap().actual_sequence,
            first_retained.sequence
        );
        assert_eq!(sequences, vec![1, 4, 5]);
        assert!(sequences.windows(2).all(|pair| pair[0] <= pair[1]));
        validate_runtime_session_event(&gap).expect("lag gap event should validate");
        validate_runtime_session_event(&first_retained)
            .expect("first retained live event should validate");
        validate_runtime_session_event(&second_retained)
            .expect("second retained live event should validate");
    }

    #[test]
    fn live_lag_gap_at_stream_start_uses_valid_conservative_cursor() {
        let record = RuntimeSessionRecord::new("lag-at-start-test", 2, true);
        let session = RuntimeSession::default();
        publish_session_event(
            &record,
            RuntimeSessionEventKind::Snapshot,
            &session,
            Vec::new(),
        );

        let gap = session_lag_gap_event(&record, 0);

        assert_eq!(gap.sequence, 1);
        assert_eq!(gap.replay.gap.as_ref().unwrap().expected_sequence, 1);
        assert_eq!(gap.replay.gap.as_ref().unwrap().actual_sequence, 2);
        validate_runtime_session_event(&gap).expect("start gap event should validate");
    }

    #[test]
    fn high_water_filter_drops_replayed_live_duplicates() {
        let record = RuntimeSessionRecord::new("high-water-test", 4, true);
        let session = RuntimeSession::default();
        publish_session_event(
            &record,
            RuntimeSessionEventKind::Snapshot,
            &session,
            Vec::new(),
        );
        let replay =
            capture_session_replay(&record, Some(0), session_snapshot_event(&record, &session));
        let first_event = record
            .event_store
            .lock()
            .expect("event store should not be poisoned")[0]
            .clone();

        assert_eq!(replay.high_water_sequence, 1);
        assert!(
            session_broadcast_event_after_high_water(
                Ok(first_event),
                record.clone(),
                replay.high_water_sequence
            )
            .is_none()
        );

        publish_session_event(
            &record,
            RuntimeSessionEventKind::Snapshot,
            &session,
            Vec::new(),
        );
        let second_event = record
            .event_store
            .lock()
            .expect("event store should not be poisoned")[1]
            .clone();
        assert!(
            session_broadcast_event_after_high_water(
                Ok(second_event),
                record,
                replay.high_water_sequence
            )
            .is_some()
        );
    }

    #[test]
    fn event_cursor_reads_last_event_id_header() {
        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", HeaderValue::from_static("41"));

        assert_eq!(event_cursor_from_headers(&headers), Some(41));
        assert_eq!(event_cursor_from_headers(&HeaderMap::new()), None);
    }

    #[test]
    fn session_info_matches_contract_shape() {
        let endpoint = RuntimeEndpointConfig::new("127.0.0.1".to_owned(), 3761);
        let profile = crate::sidecar::runtime_connection_profile(&endpoint, "unix-ms:1");
        let record = RuntimeSessionRegistry::dry_preview().default_record();

        let response = record.info_response(profile);

        assert_eq!(response.session_id, DEFAULT_SESSION_ID);
        skenion_contracts::validate_runtime_session_info_response(&response)
            .expect("session info should validate");
    }

    #[test]
    fn session_info_preserves_snapshot_diagnostics() {
        let endpoint = RuntimeEndpointConfig::new("127.0.0.1".to_owned(), 3761);
        let profile = crate::sidecar::runtime_connection_profile(&endpoint, "unix-ms:1");
        let record = RuntimeSessionRegistry::dry_preview().default_record();
        record
            .session
            .write()
            .expect("runtime session lock should not be poisoned")
            .validate_current();

        let response = record.info_response(profile);

        assert_eq!(response.diagnostics.len(), 1);
        assert_eq!(
            response.diagnostics[0]["message"],
            "no project loaded in runtime session"
        );
    }
}
