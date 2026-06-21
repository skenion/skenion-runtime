use std::{
    collections::{BTreeMap, VecDeque},
    convert::Infallible,
    sync::{Arc, Mutex, RwLock},
};

use axum::{http::HeaderMap, response::sse::Event};
use serde::{Deserialize, Serialize};
use skenion_contracts::{
    RuntimeConnectionProfile, RuntimeConnectionProfileMode, RuntimeEventReplayGap,
    RuntimeEventReplayGapReason, RuntimeEventReplayMetadata, RuntimeEventReplayWindow,
    RuntimeSessionCapabilitySet, RuntimeSessionInfoResponse, RuntimeSessionLifecycleState,
};
use tokio::sync::broadcast;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::{
    PreviewManager, RuntimeDiagnostic, RuntimeHistoryEntry, RuntimeSession, RuntimeSessionSnapshot,
    runtime_time::created_at_now,
};

pub const DEFAULT_SESSION_ID: &str = "default";
const SESSION_EVENT_REPLAY_LIMIT: usize = 256;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSessionEvent {
    pub schema: &'static str,
    pub schema_version: &'static str,
    pub id: String,
    pub session_id: String,
    pub sequence: u64,
    pub session_revision: u64,
    pub kind: RuntimeSessionEventKind,
    pub snapshot: RuntimeSessionSnapshot,
    pub history: crate::RuntimeHistory,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mutation: Option<RuntimeHistoryEntry>,
    pub replay: RuntimeEventReplayMetadata,
    pub diagnostics: Vec<RuntimeDiagnostic>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeSessionEventKind {
    Snapshot,
    Load,
    Clear,
    Mutate,
    Undo,
    Redo,
}

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
    pub event_store: Arc<Mutex<VecDeque<RuntimeSessionEvent>>>,
    pub event_sequence: Arc<Mutex<u64>>,
    pub preview: Arc<Mutex<PreviewManager>>,
    replay_limit: usize,
}

impl RuntimeSessionRecord {
    fn new(session_id: &str, replay_limit: usize, dry_preview: bool) -> Self {
        let (events, _) = broadcast::channel(replay_limit);
        Self {
            id: session_id.to_owned(),
            session: Arc::new(RwLock::new(RuntimeSession::default())),
            events,
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
        let latest_sequence = store.back().map(|event| event.sequence).unwrap_or(1);
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
    let event = session_event_from_session(record, kind, session, false, None, diagnostics);
    store_session_event(record, event.clone());
    let _ = record.events.send(event);
}

pub fn session_event_from_session(
    record: &RuntimeSessionRecord,
    kind: RuntimeSessionEventKind,
    session: &RuntimeSession,
    replayed: bool,
    gap: Option<RuntimeEventReplayGap>,
    diagnostics: Vec<RuntimeDiagnostic>,
) -> RuntimeSessionEvent {
    let sequence = next_session_event_sequence(record);
    let snapshot = session.snapshot();
    let previous_cursor = sequence
        .checked_sub(1)
        .map(|previous| previous.to_string())
        .filter(|previous| previous != "0");
    let history = session.history();
    RuntimeSessionEvent {
        schema: "skenion.runtime.session.event",
        schema_version: "0.1.0",
        id: format!("{}_event_{sequence:06}", record.id),
        session_id: record.id.clone(),
        sequence,
        session_revision: snapshot.session_revision,
        kind,
        snapshot,
        mutation: history.entries.last().cloned(),
        history,
        replay: RuntimeEventReplayMetadata {
            cursor: sequence.to_string(),
            previous_cursor,
            replayed,
            gap,
            overflow: false,
        },
        diagnostics,
        created_at: created_at_now(),
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

pub fn replay_session_events(
    record: &RuntimeSessionRecord,
    after: Option<u64>,
    snapshot: RuntimeSessionEvent,
) -> Vec<RuntimeSessionEvent> {
    let Some(after) = after else {
        return vec![snapshot];
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
        replay.push(event);
    }
    if let Some(earliest) = earliest
        && after + 1 < earliest
    {
        let mut gap = snapshot;
        gap.replay.replayed = true;
        gap.replay.overflow = true;
        gap.replay.gap = Some(RuntimeEventReplayGap {
            expected_sequence: after + 1,
            actual_sequence: earliest,
            reason: RuntimeEventReplayGapReason::RetentionOverflow,
        });
        replay.insert(0, gap);
    }
    replay
}

pub fn event_cursor_from_headers(headers: &HeaderMap) -> Option<u64> {
    match headers.get("last-event-id") {
        Some(value) => value.to_str().ok().and_then(|cursor| cursor.parse().ok()),
        None => None,
    }
}

pub fn session_broadcast_event(
    result: Result<RuntimeSessionEvent, BroadcastStreamRecvError>,
    session_id: String,
) -> Result<Event, Infallible> {
    match result {
        Ok(event) => session_event(event),
        Err(_) => Ok(Event::default()
            .event("session-gap")
            .json_data(serde_json::json!({
                "schema": "skenion.runtime.session.stream-gap",
                "schemaVersion": "0.1.0",
                "sessionId": session_id,
                "reason": "receiver-lagged"
            }))
            .expect("runtime session gap event should serialize")),
    }
}

pub fn session_event(event: RuntimeSessionEvent) -> Result<Event, Infallible> {
    Ok(Event::default()
        .event("session")
        .id(event.replay.cursor.clone())
        .json_data(event)
        .expect("runtime session event should serialize"))
}

fn contract_session_snapshot(
    snapshot: &RuntimeSessionSnapshot,
) -> skenion_contracts::RuntimeSessionSnapshot {
    serde_json::from_value(
        serde_json::to_value(snapshot).expect("runtime session snapshot should serialize"),
    )
    .expect("runtime session snapshot should match contract shape")
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
        let snapshot = session_event_from_session(
            &record,
            RuntimeSessionEventKind::Snapshot,
            &session,
            false,
            None,
            Vec::new(),
        );
        let replay = replay_session_events(&record, Some(0), snapshot);
        let gap = replay[0]
            .replay
            .gap
            .as_ref()
            .expect("replay should include retention gap");

        assert_eq!(gap.expected_sequence, 1);
        assert_eq!(gap.actual_sequence, 2);
        assert_eq!(gap.reason, RuntimeEventReplayGapReason::RetentionOverflow);
        assert_eq!(replay[1].sequence, 2);
        assert_eq!(replay[2].sequence, 3);
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
