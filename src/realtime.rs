use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use skenion_contracts::{
    InterfaceIncidentEdgePolicyV01, NodeCatalogSnapshotV01, PackageChecksumV01,
};
use tokio::sync::broadcast;

use crate::{
    CanvasNodeView, ControlMessage, ControlValue, GraphTargetRef, PatchPath,
    RuntimeCollaborationChange, RuntimeControlEventRequest, RuntimeControlEventResponse,
    RuntimeDiagnostic, RuntimeMutationRequest, RuntimePatchResponse, RuntimeSessionRecord,
    RuntimeSessionSnapshot, RuntimeViewPatch,
    object_text::{
        ObjectRegistry, ObjectTextPortActivation, ObjectTextPortDirection, ObjectTextPortRate,
        ObjectTextResolution, materialize_object_text_node_v01,
        materialize_unresolved_object_text_node_v01, object_text_node_definition_v01,
        unresolved_object_text_node_definition_v01,
    },
    runtime_time::created_at_now,
};
#[cfg(test)]
use crate::{EndpointBindingValueFormat, ValueOccurrenceHeader};

pub const RUNTIME_REALTIME_SCHEMA: &str = "skenion.runtime.realtime";
pub const RUNTIME_REALTIME_SCHEMA_VERSION: &str = "0.1.0";
pub const RUNTIME_REALTIME_REPLAY_LIMIT: usize = 256;
const RUNTIME_REALTIME_PRESENCE_LIMIT_MULTIPLIER: usize = 2;
const RUNTIME_REALTIME_RESUME_TOKEN_TTL: Duration = Duration::from_secs(5 * 60);
const RUNTIME_REALTIME_RESUME_TOKEN_BYTES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RuntimeRealtimeIdempotencyScope {
    client_id: String,
    window_id: String,
    message_type: String,
    idempotency_key: String,
}

#[derive(Debug, Clone)]
struct RuntimeRealtimeIdempotencyEntry {
    event_cursor: String,
    event_sequence: u64,
    ack_payload: Value,
    emitted_result: Option<RuntimeRealtimeEnvelope>,
    inserted_at: SystemTime,
}

#[derive(Debug, Clone)]
struct RuntimeRealtimeCachedCommandResult {
    event_cursor: String,
    ack_payload: Value,
    emitted_result: Option<RuntimeRealtimeEnvelope>,
}

#[derive(Debug, Clone)]
struct RuntimeRealtimePresenceEntry {
    presence: Value,
    expires_at: SystemTime,
    updated_sequence: u64,
}

#[derive(Debug, Clone)]
struct RuntimeRealtimeResumeIdentity {
    client_id: String,
    window_id: String,
    expires_at: SystemTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRealtimeEnvelope {
    pub schema: String,
    pub schema_version: String,
    #[serde(rename = "type")]
    pub message_type: String,
    pub message_id: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRealtimeDiagnostic {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeRealtimeConnectionIdentity {
    connection_id: String,
    client_id: String,
    window_id: String,
    resume_token: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeRealtimeSessionRevisions {
    session_revision: u64,
    view_revision: u64,
    control_revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    graph_revision: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RuntimeRealtimeReplay {
    pub events: Vec<RuntimeRealtimeEnvelope>,
    pub high_water_sequence: u64,
}

#[derive(Debug)]
pub struct RuntimeRealtimeState {
    events: broadcast::Sender<RuntimeRealtimeEnvelope>,
    event_store: Mutex<VecDeque<RuntimeRealtimeEnvelope>>,
    event_sequence: Mutex<u64>,
    connection_sequence: Mutex<u64>,
    replay_limit: usize,
    incarnation_id: String,
    idempotency_results:
        Mutex<BTreeMap<RuntimeRealtimeIdempotencyScope, RuntimeRealtimeIdempotencyEntry>>,
    presence: Mutex<BTreeMap<String, RuntimeRealtimePresenceEntry>>,
    resume_tokens: Mutex<BTreeMap<String, RuntimeRealtimeResumeIdentity>>,
}

impl RuntimeRealtimeState {
    pub fn new(session_id: &str, replay_limit: usize) -> Arc<Self> {
        let (events, _) = broadcast::channel(replay_limit);
        Arc::new(Self {
            events,
            event_store: Mutex::new(VecDeque::new()),
            event_sequence: Mutex::new(1),
            connection_sequence: Mutex::new(1),
            replay_limit,
            incarnation_id: format!("{}-{}", session_id, created_at_now().replace(':', "-")),
            idempotency_results: Mutex::new(BTreeMap::new()),
            presence: Mutex::new(BTreeMap::new()),
            resume_tokens: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeRealtimeEnvelope> {
        self.events.subscribe()
    }

    pub fn current_sequence(&self) -> u64 {
        self.event_sequence
            .lock()
            .expect("runtime realtime sequence lock should not be poisoned")
            .saturating_sub(1)
    }

    pub fn current_cursor(&self) -> String {
        self.cursor_for(self.current_sequence())
    }

    fn issue_connection_identity(
        &self,
        resumed: Option<RuntimeRealtimeResumeIdentity>,
    ) -> RuntimeRealtimeConnectionIdentity {
        let mut sequence = self
            .connection_sequence
            .lock()
            .expect("runtime realtime connection sequence lock should not be poisoned");
        let current = *sequence;
        *sequence += 1;
        let resume_token = generate_resume_token();
        let (client_id, window_id) = resumed
            .map(|resumed| (resumed.client_id, resumed.window_id))
            .unwrap_or_else(|| {
                (
                    format!("rtclient-{current:06}"),
                    format!("rtwindow-{current:06}"),
                )
            });
        let identity = RuntimeRealtimeConnectionIdentity {
            connection_id: format!("rtconn-{current:06}"),
            client_id,
            window_id,
            resume_token,
        };
        self.remember_resume_identity(&identity);
        identity
    }

    fn remember_resume_identity(&self, identity: &RuntimeRealtimeConnectionIdentity) {
        let now = SystemTime::now();
        let mut resume_tokens = self
            .resume_tokens
            .lock()
            .expect("runtime realtime resume token lock should not be poisoned");
        resume_tokens.retain(|_, identity| identity.expires_at > now);
        resume_tokens.insert(
            identity.resume_token.clone(),
            RuntimeRealtimeResumeIdentity {
                client_id: identity.client_id.clone(),
                window_id: identity.window_id.clone(),
                expires_at: now + RUNTIME_REALTIME_RESUME_TOKEN_TTL,
            },
        );
        trim_btree_map_by_key(&mut resume_tokens, self.replay_limit.max(1) * 2);
    }

    fn consume_resume_token(
        &self,
        resume_token: &str,
    ) -> Result<RuntimeRealtimeResumeIdentity, RuntimeRealtimeDiagnostic> {
        let now = SystemTime::now();
        let mut resume_tokens = self
            .resume_tokens
            .lock()
            .expect("runtime realtime resume token lock should not be poisoned");
        resume_tokens.retain(|_, identity| identity.expires_at > now);
        resume_tokens.remove(resume_token).ok_or_else(|| {
            sync_required_diagnostic(
                "realtime.resume-token.invalid",
                "resumeToken is unknown or expired; reconnect without it for a fresh identity",
                Some(json!({
                    "resumeToken": resume_token,
                    "ttlMs": RUNTIME_REALTIME_RESUME_TOKEN_TTL.as_millis() as u64,
                })),
            )
        })
    }

    fn next_event_sequence(&self) -> u64 {
        let mut sequence = self
            .event_sequence
            .lock()
            .expect("runtime realtime event sequence lock should not be poisoned");
        let current = *sequence;
        *sequence += 1;
        current
    }

    fn cursor_for(&self, sequence: u64) -> String {
        format!("{}:{sequence}", self.incarnation_id)
    }

    fn parse_cursor(&self, cursor: &str) -> Result<u64, RuntimeRealtimeDiagnostic> {
        let Some((incarnation_id, sequence)) = cursor.rsplit_once(':') else {
            return Err(sync_required_diagnostic(
                "realtime.cursor.invalid",
                "lastCursor must be a Runtime realtime cursor issued by this session",
                Some(json!({ "lastCursor": cursor })),
            ));
        };
        if incarnation_id != self.incarnation_id {
            return Err(sync_required_diagnostic(
                "realtime.cursor.incarnation-mismatch",
                "lastCursor belongs to a different session incarnation",
                Some(json!({
                    "expectedIncarnation": self.incarnation_id,
                    "actualIncarnation": incarnation_id,
                })),
            ));
        }
        sequence.parse::<u64>().map_err(|_| {
            sync_required_diagnostic(
                "realtime.cursor.invalid",
                "lastCursor sequence is not a number",
                Some(json!({ "lastCursor": cursor })),
            )
        })
    }

    pub fn replay_after(
        &self,
        last_cursor: &str,
    ) -> Result<RuntimeRealtimeReplay, RuntimeRealtimeDiagnostic> {
        let after = self.parse_cursor(last_cursor)?;
        let current = self.current_sequence();
        if after > current {
            return Err(sync_required_diagnostic(
                "realtime.cursor.unknown",
                "lastCursor is ahead of the current Runtime realtime cursor",
                Some(json!({ "lastCursor": last_cursor, "currentCursor": self.current_cursor() })),
            ));
        }

        let store = self
            .event_store
            .lock()
            .expect("runtime realtime event store lock should not be poisoned");
        let high_water_sequence = store
            .back()
            .and_then(|event| event.sequence)
            .unwrap_or(current);
        let earliest = store.front().and_then(|event| event.sequence);
        if let Some(earliest) = earliest
            && after + 1 < earliest
        {
            return Err(sync_required_diagnostic(
                "realtime.cursor.expired",
                "lastCursor is outside the retained Runtime realtime event window",
                Some(json!({
                    "lastCursor": last_cursor,
                    "earliestRetainedCursor": self.cursor_for(earliest),
                    "currentCursor": self.current_cursor(),
                })),
            ));
        }

        let events = store
            .iter()
            .filter(|event| event.sequence.unwrap_or_default() > after)
            .cloned()
            .map(mark_replayed)
            .collect();
        Ok(RuntimeRealtimeReplay {
            events,
            high_water_sequence,
        })
    }

    fn cached_command_result(
        &self,
        identity: &RuntimeRealtimeConnectionIdentity,
        message_type: &str,
        idempotency_key: &str,
    ) -> Option<RuntimeRealtimeCachedCommandResult> {
        self.prune_idempotency_results(SystemTime::now());
        self.idempotency_results
            .lock()
            .expect("runtime realtime idempotency lock should not be poisoned")
            .get(&RuntimeRealtimeIdempotencyScope {
                client_id: identity.client_id.clone(),
                window_id: identity.window_id.clone(),
                message_type: message_type.to_owned(),
                idempotency_key: idempotency_key.to_owned(),
            })
            .map(|entry| RuntimeRealtimeCachedCommandResult {
                event_cursor: entry.event_cursor.clone(),
                ack_payload: entry.ack_payload.clone(),
                emitted_result: entry.emitted_result.clone(),
            })
    }

    fn remember_ack(&self, ack: RememberAckInput<'_>) {
        let mut idempotency_results = self
            .idempotency_results
            .lock()
            .expect("runtime realtime idempotency lock should not be poisoned");
        idempotency_results.insert(
            RuntimeRealtimeIdempotencyScope {
                client_id: ack.identity.client_id.clone(),
                window_id: ack.identity.window_id.clone(),
                message_type: ack.message_type.to_owned(),
                idempotency_key: ack.idempotency_key.to_owned(),
            },
            RuntimeRealtimeIdempotencyEntry {
                event_cursor: ack.event_cursor.to_owned(),
                event_sequence: ack.event_sequence,
                ack_payload: ack.ack_payload,
                emitted_result: ack.emitted_result,
                inserted_at: SystemTime::now(),
            },
        );
        Self::prune_idempotency_results_locked(
            &mut idempotency_results,
            self.earliest_retained_sequence(),
            self.replay_limit.max(1),
            SystemTime::now(),
        );
    }

    fn remember_presence(
        &self,
        identity: &RuntimeRealtimeConnectionIdentity,
        presence: Value,
        expires_at: SystemTime,
        updated_sequence: u64,
    ) {
        let retention_limit = self.replay_limit.max(1) * RUNTIME_REALTIME_PRESENCE_LIMIT_MULTIPLIER;
        let mut presence_entries = self
            .presence
            .lock()
            .expect("runtime realtime presence lock should not be poisoned");
        Self::prune_presence_locked(&mut presence_entries, SystemTime::now(), retention_limit);
        presence_entries.insert(
            format!("{}:{}", identity.client_id, identity.window_id),
            RuntimeRealtimePresenceEntry {
                presence,
                expires_at,
                updated_sequence,
            },
        );
        Self::prune_presence_locked(&mut presence_entries, SystemTime::now(), retention_limit);
    }

    fn publish(&self, event: RuntimeRealtimeEnvelope) {
        let mut store = self
            .event_store
            .lock()
            .expect("runtime realtime event store lock should not be poisoned");
        store.push_back(event.clone());
        while store.len() > self.replay_limit {
            store.pop_front();
        }
        let earliest_retained_sequence = store.front().and_then(|event| event.sequence);
        drop(store);
        self.prune_after_event_store_update(earliest_retained_sequence);
        let _ = self.events.send(event);
    }

    fn earliest_retained_sequence(&self) -> Option<u64> {
        self.event_store
            .lock()
            .expect("runtime realtime event store lock should not be poisoned")
            .front()
            .and_then(|event| event.sequence)
    }

    fn prune_after_event_store_update(&self, earliest_retained_sequence: Option<u64>) {
        let now = SystemTime::now();
        let mut idempotency_results = self
            .idempotency_results
            .lock()
            .expect("runtime realtime idempotency lock should not be poisoned");
        Self::prune_idempotency_results_locked(
            &mut idempotency_results,
            earliest_retained_sequence,
            self.replay_limit.max(1),
            now,
        );
        drop(idempotency_results);

        let retention_limit = self.replay_limit.max(1) * RUNTIME_REALTIME_PRESENCE_LIMIT_MULTIPLIER;
        let mut presence = self
            .presence
            .lock()
            .expect("runtime realtime presence lock should not be poisoned");
        Self::prune_presence_locked(&mut presence, now, retention_limit);
    }

    fn prune_idempotency_results(&self, now: SystemTime) {
        let mut idempotency_results = self
            .idempotency_results
            .lock()
            .expect("runtime realtime idempotency lock should not be poisoned");
        Self::prune_idempotency_results_locked(
            &mut idempotency_results,
            self.earliest_retained_sequence(),
            self.replay_limit.max(1),
            now,
        );
    }

    fn prune_idempotency_results_locked(
        idempotency_results: &mut BTreeMap<
            RuntimeRealtimeIdempotencyScope,
            RuntimeRealtimeIdempotencyEntry,
        >,
        earliest_retained_sequence: Option<u64>,
        retention_limit: usize,
        now: SystemTime,
    ) {
        if let Some(earliest_retained_sequence) = earliest_retained_sequence {
            idempotency_results
                .retain(|_, entry| entry.event_sequence >= earliest_retained_sequence);
        }
        idempotency_results.retain(|_, entry| {
            now.duration_since(entry.inserted_at)
                .map(|age| age <= RUNTIME_REALTIME_RESUME_TOKEN_TTL)
                .unwrap_or(true)
        });
        if idempotency_results.len() <= retention_limit {
            return;
        }

        let mut oldest = idempotency_results
            .iter()
            .map(|(scope, entry)| (entry.event_sequence, entry.inserted_at, scope.clone()))
            .collect::<Vec<_>>();
        oldest
            .sort_by_key(|(sequence, inserted_at, scope)| (*sequence, *inserted_at, scope.clone()));
        for (_, _, scope) in oldest
            .into_iter()
            .take(idempotency_results.len().saturating_sub(retention_limit))
        {
            idempotency_results.remove(&scope);
        }
    }

    fn prune_presence_locked(
        presence: &mut BTreeMap<String, RuntimeRealtimePresenceEntry>,
        now: SystemTime,
        retention_limit: usize,
    ) {
        presence.retain(|_, entry| {
            let _ = &entry.presence;
            entry.expires_at > now
        });
        if presence.len() <= retention_limit {
            return;
        }

        let mut oldest = presence
            .iter()
            .map(|(key, entry)| (entry.updated_sequence, key.clone()))
            .collect::<Vec<_>>();
        oldest.sort();
        for (_, key) in oldest
            .into_iter()
            .take(presence.len().saturating_sub(retention_limit))
        {
            presence.remove(&key);
        }
    }
}

struct RememberAckInput<'a> {
    identity: &'a RuntimeRealtimeConnectionIdentity,
    message_type: &'a str,
    idempotency_key: &'a str,
    event_cursor: &'a str,
    event_sequence: u64,
    ack_payload: Value,
    emitted_result: Option<RuntimeRealtimeEnvelope>,
}

#[derive(Clone, Copy)]
struct RealtimeEventPosition<'a> {
    sequence: u64,
    cursor: &'a str,
}

pub async fn handle_runtime_realtime_socket(record: RuntimeSessionRecord, socket: WebSocket) {
    let mut receiver = record.realtime.subscribe();
    let (mut sender, mut socket_receiver) = socket.split();
    let mut identity: Option<RuntimeRealtimeConnectionIdentity> = None;
    let mut high_water_sequence = 0;

    loop {
        tokio::select! {
            Some(message) = socket_receiver.next() => {
                let message = match message {
                    Ok(message) => message,
                    Err(_) => break,
                };
                match message {
                    Message::Text(text) => {
                        let parsed = serde_json::from_str::<RuntimeRealtimeEnvelope>(&text);
                        let frame = match parsed {
                            Ok(frame) => frame,
                            Err(error) => {
                                let diagnostic = runtime_error(&record.id, None, None, "realtime.frame.invalid-json", format!("invalid realtime JSON frame: {error}"), None);
                                if send_frame(&mut sender, &diagnostic).await.is_err() {
                                    break;
                                }
                                continue;
                            }
                        };

                        if frame.session_id != record.id {
                            let diagnostic = runtime_error(&record.id, None, Some(&frame), "realtime.session.mismatch", "frame sessionId does not match the WebSocket session", Some(json!({"expectedSessionId": record.id, "actualSessionId": frame.session_id})));
                            if send_frame(&mut sender, &diagnostic).await.is_err() {
                                break;
                            }
                            continue;
                        }

                        match frame.message_type.as_str() {
                            "session.hello" => {
                                let snapshot = current_snapshot(&record);
                                let hello = decode_hello_payload(&frame);
                                let resumed_identity = match hello.resume_token.as_deref() {
                                    Some(resume_token) => {
                                        match record.realtime.consume_resume_token(resume_token) {
                                            Ok(identity) => Some(identity),
                                            Err(diagnostic) => {
                                                let issued_identity =
                                                    record.realtime.issue_connection_identity(None);
                                                high_water_sequence =
                                                    record.realtime.current_sequence();
                                                let sync = session_sync_required(
                                                    &record,
                                                    &issued_identity,
                                                    &frame,
                                                    &snapshot,
                                                    diagnostic,
                                                    hello.node_catalog.as_ref(),
                                                );
                                                if send_frame(&mut sender, &sync).await.is_err() {
                                                    break;
                                                }
                                                identity = Some(issued_identity);
                                                continue;
                                            }
                                        }
                                    }
                                    None => None,
                                };
                                let issued_identity =
                                    record.realtime.issue_connection_identity(resumed_identity);
                                match hello.last_cursor.as_deref() {
                                    Some(last_cursor) => {
                                        match record.realtime.replay_after(last_cursor) {
                                            Ok(replay) => {
                                                high_water_sequence = replay.high_water_sequence;
                                                let attached = session_attached(&record, &issued_identity, &frame, &snapshot, hello.node_catalog.as_ref());
                                                if send_frame(&mut sender, &attached).await.is_err() {
                                                    break;
                                                }
                                                for event in replay.events {
                                                    if send_frame(&mut sender, &event).await.is_err() {
                                                        break;
                                                    }
                                                }
                                            }
                                            Err(diagnostic) => {
                                                high_water_sequence = record.realtime.current_sequence();
                                                let sync = session_sync_required(&record, &issued_identity, &frame, &snapshot, diagnostic, hello.node_catalog.as_ref());
                                                if send_frame(&mut sender, &sync).await.is_err() {
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                    None => {
                                        high_water_sequence = record.realtime.current_sequence();
                                        let attached = session_attached(&record, &issued_identity, &frame, &snapshot, hello.node_catalog.as_ref());
                                        if send_frame(&mut sender, &attached).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                                identity = Some(issued_identity);
                            }
                            "presence.update" => {
                                let Some(identity) = identity.as_ref() else {
                                    let diagnostic = runtime_error(&record.id, None, Some(&frame), "realtime.session.not-attached", "send session.hello before client actions", None);
                                    if send_frame(&mut sender, &diagnostic).await.is_err() {
                                        break;
                                    }
                                    continue;
                                };
                                match handle_presence_update(&record, identity, frame) {
                                    Ok((ack, event)) => {
                                        if send_frame(&mut sender, &ack).await.is_err() {
                                            break;
                                        }
                                        if let Some(event) = event {
                                            record.realtime.publish(event);
                                        }
                                    }
                                    Err(diagnostic) => {
                                        let diagnostic = runtime_error(&record.id, Some(identity), None, &diagnostic.code, diagnostic.message, diagnostic.details);
                                        if send_frame(&mut sender, &diagnostic).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                            }
                            "control.command" => {
                                let Some(identity) = identity.as_ref() else {
                                    let diagnostic = runtime_error(&record.id, None, Some(&frame), "realtime.session.not-attached", "send session.hello before client actions", None);
                                    if send_frame(&mut sender, &diagnostic).await.is_err() {
                                        break;
                                    }
                                    continue;
                                };
                                match handle_control_command(&record, identity, frame) {
                                    Ok((ack, event, local_event)) => {
                                        if send_frame(&mut sender, &ack).await.is_err() {
                                            break;
                                        }
                                        if let Some(local_event) = local_event {
                                            if send_frame(&mut sender, &local_event).await.is_err()
                                            {
                                                break;
                                            }
                                        }
                                        if let Some(event) = event {
                                            record.realtime.publish(event);
                                        }
                                    }
                                    Err(diagnostic) => {
                                        let diagnostic = runtime_error(&record.id, Some(identity), None, &diagnostic.code, diagnostic.message, diagnostic.details);
                                        if send_frame(&mut sender, &diagnostic).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                            }
                            "graph.command" => {
                                let Some(identity) = identity.as_ref() else {
                                    let diagnostic = runtime_error(&record.id, None, Some(&frame), "realtime.session.not-attached", "send session.hello before client actions", None);
                                    if send_frame(&mut sender, &diagnostic).await.is_err() {
                                        break;
                                    }
                                    continue;
                                };
                                match handle_graph_command(&record, identity, frame) {
                                    Ok((ack, events, local_event)) => {
                                        if send_frame(&mut sender, &ack).await.is_err() {
                                            break;
                                        }
                                        if let Some(local_event) = local_event {
                                            if send_frame(&mut sender, &local_event).await.is_err()
                                            {
                                                break;
                                            }
                                        }
                                        for event in events {
                                            record.realtime.publish(event);
                                        }
                                    }
                                    Err(diagnostic) => {
                                        let diagnostic = runtime_error(&record.id, Some(identity), None, &diagnostic.code, diagnostic.message, diagnostic.details);
                                        if send_frame(&mut sender, &diagnostic).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                            }
                            "nodeCatalog.request" => {
                                let Some(identity) = identity.as_ref() else {
                                    let diagnostic = runtime_error(&record.id, None, Some(&frame), "realtime.session.not-attached", "send session.hello before client actions", None);
                                    if send_frame(&mut sender, &diagnostic).await.is_err() {
                                        break;
                                    }
                                    continue;
                                };
                                match handle_node_catalog_request(&record, identity, frame) {
                                    Ok(response) => {
                                        if send_frame(&mut sender, &response).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(diagnostic) => {
                                        let diagnostic = runtime_error(&record.id, Some(identity), None, &diagnostic.code, diagnostic.message, diagnostic.details);
                                        if send_frame(&mut sender, &diagnostic).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                            }
                            _ => {
                                let diagnostic = runtime_error(&record.id, identity.as_ref(), Some(&frame), "realtime.frame.unsupported-type", "unsupported Runtime realtime frame type", Some(json!({"type": frame.message_type})));
                                if send_frame(&mut sender, &diagnostic).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Message::Close(_) => break,
                    Message::Ping(payload) => {
                        if sender.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Message::Pong(_) => {}
                    Message::Binary(_) => {
                        let diagnostic = runtime_error(&record.id, identity.as_ref(), None, "realtime.frame.binary-unsupported", "Runtime realtime frames must be JSON text", None);
                        if send_frame(&mut sender, &diagnostic).await.is_err() {
                            break;
                        }
                    }
                }
            }
            event = receiver.recv() => {
                match event {
                    Ok(event) if event.sequence.unwrap_or_default() <= high_water_sequence => {}
                    Ok(event) => {
                        if send_frame(&mut sender, &event).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let Some(identity) = identity.as_ref() else {
                            continue;
                        };
                        let snapshot = current_snapshot(&record);
                        let diagnostic = sync_required_diagnostic(
                            "realtime.cursor.stream-lagged",
                            "WebSocket receiver lagged beyond the Runtime realtime event window",
                            Some(json!({ "currentCursor": record.realtime.current_cursor() })),
                        );
                        let sync = session_sync_required(&record, identity, &empty_correlation_frame(&record.id), &snapshot, diagnostic, None);
                        if send_frame(&mut sender, &sync).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HelloPayload {
    last_cursor: Option<String>,
    resume_token: Option<String>,
    #[serde(default)]
    node_catalog: Option<NodeCatalogHelloRequest>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeCatalogHelloRequest {
    #[serde(default)]
    mode: NodeCatalogHelloMode,
    #[serde(default)]
    known_revision: Option<Value>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum NodeCatalogHelloMode {
    #[default]
    None,
    IfChanged,
    Always,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeCatalogRequestPayload {
    #[serde(default)]
    known_revision: Option<Value>,
}

fn decode_hello_payload(frame: &RuntimeRealtimeEnvelope) -> HelloPayload {
    let mut hello =
        serde_json::from_value::<HelloPayload>(frame.payload.clone()).unwrap_or_default();
    if hello.last_cursor.is_none() {
        hello.last_cursor = frame.cursor.clone();
    }
    hello
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PresenceUpdatePayload {
    #[serde(default)]
    presence: Value,
    #[serde(default)]
    ttl_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphCommandPayload {
    kind: String,
    #[serde(default)]
    base_session_revision: Option<u64>,
    #[serde(default)]
    base_graph_revision: Option<String>,
    #[serde(default)]
    base_view_revision: Option<u64>,
    #[serde(default)]
    target: Option<GraphTargetRef>,
    #[serde(default)]
    view_patch: Option<RuntimeViewPatch>,
    #[serde(default)]
    changes: Option<Vec<RuntimeCollaborationChange>>,
    #[serde(default)]
    object_text: Option<String>,
    #[serde(default)]
    node_id: Option<String>,
    #[serde(default)]
    requested_node_id: Option<String>,
    #[serde(default)]
    view: Option<CanvasNodeView>,
    #[serde(default)]
    params: Option<Map<String, Value>>,
    #[serde(default)]
    port_id: Option<String>,
    #[serde(default)]
    message: Option<ControlMessage>,
    #[serde(default)]
    unresolved_policy: Option<ObjectUnresolvedPolicy>,
    #[serde(default)]
    interface_incident_edge_policy: Option<InterfaceIncidentEdgePolicyV01>,
    #[serde(default)]
    surface_path: Option<Value>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ObjectUnresolvedPolicy {
    Reject,
    MaterializeDiagnostic,
}

#[derive(Debug)]
struct GraphCommandOutcome {
    response: RuntimePatchResponse,
    node_result: Option<Value>,
    control_emission: Option<GraphControlEmission>,
    catalog_snapshot: Option<NodeCatalogSnapshotV01>,
}

#[derive(Debug)]
struct GraphControlEmission {
    request: RuntimeControlEventRequest,
    response: RuntimeControlEventResponse,
    changed_values: BTreeMap<String, ControlValue>,
}

fn handle_presence_update(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: RuntimeRealtimeEnvelope,
) -> Result<(RuntimeRealtimeEnvelope, Option<RuntimeRealtimeEnvelope>), RuntimeRealtimeDiagnostic> {
    let idempotency_key = frame.idempotency_key.clone().ok_or_else(|| {
        sync_required_diagnostic(
            "realtime.command.idempotency-key-required",
            "presence.update requires idempotencyKey",
            None,
        )
    })?;
    if let Some(cached) =
        record
            .realtime
            .cached_command_result(identity, &frame.message_type, &idempotency_key)
    {
        return Ok((
            command_ack_from_cached(record, identity, &frame, cached),
            None,
        ));
    }

    let payload = serde_json::from_value::<PresenceUpdatePayload>(frame.payload.clone()).map_err(
        |error| {
            sync_required_diagnostic(
                "realtime.presence.invalid-payload",
                format!("invalid presence.update payload: {error}"),
                None,
            )
        },
    )?;
    let ttl_ms = payload.ttl_ms.unwrap_or(30_000).clamp(1_000, 300_000);
    let now = SystemTime::now();
    let updated_at = unix_ms_timestamp(now);
    let expires_at_time = now + Duration::from_millis(ttl_ms);
    let expires_at = unix_ms_timestamp(expires_at_time);
    let sequence = record.realtime.next_event_sequence();
    let cursor = record.realtime.cursor_for(sequence);
    let presence_payload = json!({
        "presence": payload.presence,
        "ttlMs": ttl_ms,
        "updatedAt": updated_at,
        "expiresAt": expires_at,
        "ephemeral": true,
        "replayed": false,
    });
    let presence = json!({
        "sessionId": record.id,
        "connectionId": identity.connection_id,
        "clientId": identity.client_id,
        "windowId": identity.window_id,
        "presence": presence_payload,
    });
    record
        .realtime
        .remember_presence(identity, presence.clone(), expires_at_time, sequence);

    let event = RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: "presence.updated".to_owned(),
        message_id: format!("{}_presence_{sequence:06}", record.id),
        session_id: record.id.clone(),
        connection_id: Some(identity.connection_id.clone()),
        client_id: Some(identity.client_id.clone()),
        window_id: Some(identity.window_id.clone()),
        command_id: frame
            .command_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        correlation_id: frame
            .correlation_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        idempotency_key: Some(idempotency_key.clone()),
        sequence: Some(sequence),
        cursor: Some(cursor.clone()),
        created_at: Some(created_at_now()),
        payload: presence,
    };
    let ack = command_ack(record, identity, &frame, &cursor, false);
    record.realtime.remember_ack(RememberAckInput {
        identity,
        message_type: &frame.message_type,
        idempotency_key: &idempotency_key,
        event_cursor: &cursor,
        event_sequence: sequence,
        ack_payload: ack.payload.clone(),
        emitted_result: None,
    });
    Ok((ack, Some(event)))
}

fn handle_control_command(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: RuntimeRealtimeEnvelope,
) -> Result<
    (
        RuntimeRealtimeEnvelope,
        Option<RuntimeRealtimeEnvelope>,
        Option<RuntimeRealtimeEnvelope>,
    ),
    RuntimeRealtimeDiagnostic,
> {
    let idempotency_key = frame.idempotency_key.clone().ok_or_else(|| {
        sync_required_diagnostic(
            "realtime.command.idempotency-key-required",
            "control.command requires idempotencyKey",
            None,
        )
    })?;
    if let Some(cached) =
        record
            .realtime
            .cached_command_result(identity, &frame.message_type, &idempotency_key)
    {
        let emitted_result = cached.emitted_result.clone();
        return Ok((
            control_ack_from_cached(record, identity, &frame, cached),
            None,
            emitted_result,
        ));
    }

    let request = serde_json::from_value::<RuntimeControlEventRequest>(frame.payload.clone())
        .map_err(|error| {
            sync_required_diagnostic(
                "realtime.control.invalid-payload",
                format!("invalid control.command payload: {error}"),
                None,
            )
        })?;
    let sequence = record.realtime.next_event_sequence();
    let cursor = record.realtime.cursor_for(sequence);
    let (mut response, changed_values, request_for_event) =
        apply_control_command(record, request.clone());
    let accepted = response.ok;
    let ack = control_ack(
        record, identity, &frame, &response, sequence, &cursor, false,
    );
    let event = if accepted {
        control_emitted_event(
            record,
            identity,
            &frame,
            &request_for_event,
            &mut response,
            changed_values,
            RealtimeEventPosition {
                sequence,
                cursor: &cursor,
            },
        )
    } else {
        None
    };
    record.realtime.remember_ack(RememberAckInput {
        identity,
        message_type: &frame.message_type,
        idempotency_key: &idempotency_key,
        event_cursor: &cursor,
        event_sequence: sequence,
        ack_payload: ack.payload.clone(),
        emitted_result: event.clone(),
    });

    Ok((ack, event, None))
}

fn handle_node_catalog_request(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: RuntimeRealtimeEnvelope,
) -> Result<RuntimeRealtimeEnvelope, RuntimeRealtimeDiagnostic> {
    let request = serde_json::from_value::<NodeCatalogRequestPayload>(frame.payload.clone())
        .map_err(|error| {
            sync_required_diagnostic(
                "realtime.node-catalog.invalid-payload",
                format!("invalid nodeCatalog.request payload: {error}"),
                None,
            )
        })?;
    let snapshot = node_catalog_snapshot_for_record(record);
    let (message_type, payload) =
        if catalog_revision_matches(request.known_revision.as_ref(), &snapshot.catalog_revision) {
            (
                "nodeCatalog.unchanged",
                node_catalog_unchanged_response_payload(snapshot),
            )
        } else {
            (
                "nodeCatalog.snapshot",
                node_catalog_snapshot_response_payload(snapshot),
            )
        };
    Ok(node_catalog_response(
        record,
        identity,
        &frame,
        message_type,
        payload,
    ))
}

fn handle_graph_command(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: RuntimeRealtimeEnvelope,
) -> Result<
    (
        RuntimeRealtimeEnvelope,
        Vec<RuntimeRealtimeEnvelope>,
        Option<RuntimeRealtimeEnvelope>,
    ),
    RuntimeRealtimeDiagnostic,
> {
    let idempotency_key = frame.idempotency_key.clone().ok_or_else(|| {
        sync_required_diagnostic(
            "realtime.command.idempotency-key-required",
            "graph.command requires idempotencyKey",
            None,
        )
    })?;
    if let Some(cached) =
        record
            .realtime
            .cached_command_result(identity, &frame.message_type, &idempotency_key)
    {
        let applied_result = cached.emitted_result.clone();
        return Ok((
            graph_ack_from_cached(record, identity, &frame, cached),
            Vec::new(),
            applied_result,
        ));
    }

    let payload =
        serde_json::from_value::<GraphCommandPayload>(frame.payload.clone()).map_err(|error| {
            sync_required_diagnostic(
                "realtime.graph.invalid-payload",
                format!("invalid graph.command payload: {error}"),
                None,
            )
        })?;
    let sequence = record.realtime.next_event_sequence();
    let cursor = record.realtime.cursor_for(sequence);
    let outcome = apply_graph_command(record, identity, &frame, &payload);
    let GraphCommandOutcome {
        response,
        node_result,
        control_emission,
        catalog_snapshot,
    } = outcome;
    let position = RealtimeEventPosition {
        sequence,
        cursor: &cursor,
    };
    let ack = graph_ack(
        record,
        identity,
        &frame,
        &payload,
        &response,
        node_result.as_ref(),
        position,
        false,
    );
    let event = if response.applied {
        Some(graph_applied_event(
            record,
            identity,
            &frame,
            &payload,
            &response,
            node_result.as_ref(),
            sequence,
            cursor.clone(),
        ))
    } else if let Some(mut control_emission) = control_emission {
        if control_emission.response.ok {
            control_emitted_event(
                record,
                identity,
                &frame,
                &control_emission.request,
                &mut control_emission.response,
                control_emission.changed_values,
                position,
            )
        } else {
            None
        }
    } else {
        None
    };
    let mut events = event.into_iter().collect::<Vec<_>>();
    if let Some(catalog_snapshot) = catalog_snapshot {
        let catalog_sequence = record.realtime.next_event_sequence();
        let catalog_cursor = record.realtime.cursor_for(catalog_sequence);
        events.push(node_catalog_changed_event(
            record,
            identity,
            &frame,
            catalog_snapshot,
            catalog_sequence,
            catalog_cursor,
        ));
    }
    record.realtime.remember_ack(RememberAckInput {
        identity,
        message_type: &frame.message_type,
        idempotency_key: &idempotency_key,
        event_cursor: &cursor,
        event_sequence: sequence,
        ack_payload: ack.payload.clone(),
        emitted_result: events.first().cloned(),
    });

    Ok((ack, events, None))
}

fn apply_graph_command(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    payload: &GraphCommandPayload,
) -> GraphCommandOutcome {
    let mut session = record
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    let before = session.snapshot();
    let before_catalog_revision = node_catalog_snapshot_for_session(&session).catalog_revision;

    if let Some(base_session_revision) = payload.base_session_revision
        && base_session_revision != before.session_revision
    {
        return GraphCommandOutcome::from_response(graph_command_rejected_response(
            &session,
            true,
            RuntimeDiagnostic::structured_error(
                "graph.command.session-revision-conflict",
                format!(
                    "baseSessionRevision {base_session_revision} does not match session revision {}",
                    before.session_revision
                ),
                json!({
                    "expectedRevision": base_session_revision,
                    "actualRevision": before.session_revision,
                    "commandKind": payload.kind,
                }),
            ),
        ));
    }

    if payload.kind == "node.input" {
        let Some(node_id) = payload.node_id.clone() else {
            return GraphCommandOutcome::with_node_result(
                graph_command_rejected_response(
                    &session,
                    false,
                    RuntimeDiagnostic::structured_error(
                        "graph.command.node-id-required",
                        "graph.command kind node.input requires payload.nodeId",
                        json!({ "commandKind": payload.kind }),
                    ),
                ),
                node_command_result(payload, None, None, Vec::new(), None),
            );
        };
        let Some(port_id) = payload.port_id.clone() else {
            return GraphCommandOutcome::with_node_result(
                graph_command_rejected_response(
                    &session,
                    false,
                    RuntimeDiagnostic::structured_error(
                        "graph.command.port-id-required",
                        "graph.command kind node.input requires payload.portId",
                        json!({ "commandKind": payload.kind, "nodeId": node_id }),
                    ),
                ),
                node_command_result(payload, None, Some(&node_id), Vec::new(), None),
            );
        };
        let Some(message) = payload.message.clone() else {
            return GraphCommandOutcome::with_node_result(
                graph_command_rejected_response(
                    &session,
                    false,
                    RuntimeDiagnostic::structured_error(
                        "graph.command.message-required",
                        "graph.command kind node.input requires payload.message",
                        json!({
                            "commandKind": payload.kind,
                            "nodeId": node_id,
                            "portId": port_id,
                        }),
                    ),
                ),
                node_command_result(payload, None, Some(&node_id), Vec::new(), None),
            );
        };
        drop(session);
        let request = RuntimeControlEventRequest {
            node_id,
            port_id,
            message,
        };
        let (response, changed_values, applied_request) = apply_control_command(record, request);
        let (snapshot, history) = {
            let session = record
                .session
                .read()
                .expect("runtime session lock should not be poisoned");
            (session.snapshot(), session.history())
        };
        let input = node_input_result(&applied_request, &response);
        let patch_response = RuntimePatchResponse {
            ok: response.ok,
            applied: false,
            conflict: false,
            snapshot,
            history,
            diagnostics: response.diagnostics.clone(),
        };
        let control_emission = response.ok.then_some(GraphControlEmission {
            request: applied_request.clone(),
            response,
            changed_values,
        });
        return GraphCommandOutcome::with_node_result_and_control_emission(
            patch_response,
            node_command_result(
                payload,
                None,
                Some(&applied_request.node_id),
                Vec::new(),
                Some(input),
            ),
            control_emission,
        );
    }

    let response = match payload.kind.as_str() {
        "view.patch" => {
            let Some(view_patch) = payload.view_patch.clone() else {
                return GraphCommandOutcome::from_response(graph_command_rejected_response(
                    &session,
                    false,
                    RuntimeDiagnostic::structured_error(
                        "graph.command.view-patch-required",
                        "graph.command kind view.patch requires payload.viewPatch",
                        json!({ "commandKind": payload.kind }),
                    ),
                ));
            };
            if let Some(base_view_revision) = payload.base_view_revision
                && base_view_revision != view_patch.base_view_revision
            {
                return GraphCommandOutcome::from_response(graph_command_rejected_response(
                    &session,
                    true,
                    RuntimeDiagnostic::structured_error(
                        "graph.command.view-revision-conflict",
                        format!(
                            "baseViewRevision {base_view_revision} does not match viewPatch.baseViewRevision {}",
                            view_patch.base_view_revision
                        ),
                        json!({
                            "expectedRevision": base_view_revision,
                            "actualRevision": view_patch.base_view_revision,
                            "commandKind": payload.kind,
                        }),
                    ),
                ));
            }
            if let Some(base_graph_revision) = payload.base_graph_revision.as_deref() {
                let actual_graph_revision = before.graph_revision().map(ToOwned::to_owned);
                if actual_graph_revision.as_deref() != Some(base_graph_revision) {
                    return GraphCommandOutcome::from_response(graph_command_rejected_response(
                        &session,
                        true,
                        RuntimeDiagnostic::structured_error(
                            "graph.command.graph-revision-conflict",
                            format!(
                                "baseGraphRevision {base_graph_revision} does not match graph revision {}",
                                actual_graph_revision.as_deref().unwrap_or("none")
                            ),
                            json!({
                                "expectedRevision": base_graph_revision,
                                "actualRevision": actual_graph_revision,
                                "commandKind": payload.kind,
                            }),
                        ),
                    ));
                }
            }
            if let Some(target) = payload.target.as_ref() {
                if !matches!(target.path, PatchPath::Root) {
                    return GraphCommandOutcome::from_response(graph_command_rejected_response(
                        &session,
                        false,
                        RuntimeDiagnostic::structured_error(
                            "graph.command.view-target-unsupported",
                            "view.patch realtime commands currently support only the loaded root graph view",
                            json!({ "target": target, "commandKind": payload.kind }),
                        ),
                    ));
                }
                let actual_target_revision = session.target_revision_current(target);
                if actual_target_revision.as_deref() != Some(target.base_revision.as_str()) {
                    return GraphCommandOutcome::from_response(graph_command_rejected_response(
                        &session,
                        true,
                        RuntimeDiagnostic::structured_error(
                            "graph.command.target-revision-conflict",
                            format!(
                                "target baseRevision {} does not match target graph revision {}",
                                target.base_revision,
                                actual_target_revision.as_deref().unwrap_or("none")
                            ),
                            json!({
                                "expectedRevision": target.base_revision,
                                "actualRevision": actual_target_revision,
                                "target": target,
                                "commandKind": payload.kind,
                            }),
                        ),
                    ));
                }
            }

            session.apply_mutation(
                RuntimeMutationRequest::view_patch(view_patch)
                    .with_client_id(identity.client_id.clone())
                    .with_description(
                        payload.description.clone().unwrap_or_else(|| {
                            format!("Realtime graph command {}", frame.message_id)
                        }),
                    ),
            )
        }
        "collaboration.changeSet" => {
            let Some(target) = payload.target.clone() else {
                return GraphCommandOutcome::from_response(graph_command_rejected_response(
                    &session,
                    false,
                    RuntimeDiagnostic::structured_error(
                        "graph.command.target-required",
                        "graph.command kind collaboration.changeSet requires payload.target",
                        json!({ "commandKind": payload.kind }),
                    ),
                ));
            };
            let changes = payload.changes.clone().unwrap_or_default();
            if changes.is_empty() {
                return GraphCommandOutcome::from_response(graph_command_rejected_response(
                    &session,
                    false,
                    RuntimeDiagnostic::structured_error(
                        "graph.command.changes-required",
                        "graph.command kind collaboration.changeSet requires at least one change",
                        json!({ "target": target, "commandKind": payload.kind }),
                    ),
                ));
            }
            if let Some(base_graph_revision) = payload.base_graph_revision.as_deref()
                && base_graph_revision != target.base_revision
            {
                return GraphCommandOutcome::from_response(graph_command_rejected_response(
                    &session,
                    true,
                    RuntimeDiagnostic::structured_error(
                        "graph.command.target-revision-conflict",
                        format!(
                            "baseGraphRevision {base_graph_revision} does not match target.baseRevision {}",
                            target.base_revision
                        ),
                        json!({
                            "expectedRevision": base_graph_revision,
                            "actualRevision": target.base_revision,
                            "target": target,
                            "commandKind": payload.kind,
                        }),
                    ),
                ));
            }
            session.apply_collaboration_change_set_current(
                target,
                changes,
                None,
                Some(identity.client_id.clone()),
                payload
                    .description
                    .clone()
                    .or_else(|| Some(format!("Realtime graph command {}", frame.message_id))),
            )
        }
        "node.resolve" => {
            return apply_object_resolve_graph_command(&session, payload);
        }
        "node.create" => {
            return apply_object_create_graph_command(&mut session, identity, frame, payload)
                .with_catalog_change(before_catalog_revision, &session);
        }
        "node.replace" => {
            return apply_object_replace_graph_command(&mut session, identity, frame, payload)
                .with_catalog_change(before_catalog_revision, &session);
        }
        "node.delete" => {
            return apply_node_delete_graph_command(&mut session, identity, frame, payload)
                .with_catalog_change(before_catalog_revision, &session);
        }
        "node.update" => {
            return apply_node_update_graph_command(&mut session, identity, frame, payload)
                .with_catalog_change(before_catalog_revision, &session);
        }
        _ => graph_command_rejected_response(
            &session,
            false,
            RuntimeDiagnostic::structured_error(
                "graph.command.kind-unsupported",
                format!(
                    "unsupported graph.command kind {}; supported kinds are view.patch, collaboration.changeSet, node.resolve, node.create, node.replace, node.delete, node.update, and node.input",
                    payload.kind
                ),
                json!({
                    "kind": payload.kind,
                    "supportedKinds": ["view.patch", "collaboration.changeSet", "node.resolve", "node.create", "node.replace", "node.delete", "node.update", "node.input"],
                }),
            ),
        ),
    };
    GraphCommandOutcome::from_response(response)
        .with_catalog_change(before_catalog_revision, &session)
}

impl GraphCommandOutcome {
    fn from_response(response: RuntimePatchResponse) -> Self {
        Self {
            response,
            node_result: None,
            control_emission: None,
            catalog_snapshot: None,
        }
    }

    fn with_node_result(response: RuntimePatchResponse, node_result: Value) -> Self {
        Self {
            response,
            node_result: Some(node_result),
            control_emission: None,
            catalog_snapshot: None,
        }
    }

    fn with_node_result_and_control_emission(
        response: RuntimePatchResponse,
        node_result: Value,
        control_emission: Option<GraphControlEmission>,
    ) -> Self {
        Self {
            response,
            node_result: Some(node_result),
            control_emission,
            catalog_snapshot: None,
        }
    }

    fn with_catalog_change(
        mut self,
        before_catalog_revision: PackageChecksumV01,
        session: &crate::RuntimeSession,
    ) -> Self {
        if self.response.applied {
            let snapshot = node_catalog_snapshot_for_session(session);
            if snapshot.catalog_revision != before_catalog_revision {
                self.catalog_snapshot = Some(snapshot);
            }
        }
        self
    }
}

fn resolve_object_command_text(
    session: &crate::RuntimeSession,
    object_text: &str,
) -> ObjectTextResolution {
    let project = session.project_document_current();
    ObjectRegistry::for_project(project.as_ref()).resolve(object_text)
}

fn apply_object_resolve_graph_command(
    session: &crate::RuntimeSession,
    payload: &GraphCommandPayload,
) -> GraphCommandOutcome {
    let Some(object_text) = payload.object_text.as_deref() else {
        return GraphCommandOutcome::from_response(graph_command_rejected_response(
            session,
            false,
            RuntimeDiagnostic::structured_error(
                "graph.command.object-text-required",
                "graph.command kind node.resolve requires payload.objectText",
                json!({ "commandKind": payload.kind }),
            ),
        ));
    };
    let resolution = resolve_object_command_text(session, object_text);
    let node_result = node_command_result(payload, Some(&resolution), None, Vec::new(), None);
    if let Err(response) = validate_object_command_target(session, payload, true) {
        return GraphCommandOutcome::with_node_result(response, node_result);
    }

    GraphCommandOutcome::with_node_result(
        RuntimePatchResponse {
            ok: true,
            applied: false,
            conflict: false,
            snapshot: session.snapshot(),
            history: session.history(),
            diagnostics: object_text_runtime_diagnostics(&resolution),
        },
        node_result,
    )
}

fn apply_object_create_graph_command(
    session: &mut crate::RuntimeSession,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    payload: &GraphCommandPayload,
) -> GraphCommandOutcome {
    let Some(object_text) = payload.object_text.as_deref() else {
        return GraphCommandOutcome::from_response(graph_command_rejected_response(
            session,
            false,
            RuntimeDiagnostic::structured_error(
                "graph.command.object-text-required",
                "graph.command kind node.create requires payload.objectText",
                json!({ "commandKind": payload.kind }),
            ),
        ));
    };
    let resolution = resolve_object_command_text(session, object_text);
    let node_result = node_command_result(
        payload,
        Some(&resolution),
        payload.requested_node_id.as_deref(),
        Vec::new(),
        None,
    );
    let target = match validate_object_command_target(session, payload, false) {
        Ok(target) => target,
        Err(response) => return GraphCommandOutcome::with_node_result(response, node_result),
    };
    let node_id = payload
        .requested_node_id
        .clone()
        .unwrap_or_else(|| generated_node_id_for_create(session, &target, &resolution));
    let Some((node, definition)) =
        materialize_object_command_node(session, payload, &resolution, &node_id)
    else {
        let node_result =
            node_command_result(payload, Some(&resolution), Some(&node_id), Vec::new(), None);
        return GraphCommandOutcome::with_node_result(
            graph_command_rejected_response(
                session,
                false,
                RuntimeDiagnostic::structured_error(
                    "node.command.unresolved",
                    "object text could not be resolved for node.create",
                    json!({
                        "commandKind": payload.kind,
                        "target": target,
                        "objectText": object_text,
                        "unresolvedPolicy": object_unresolved_policy(payload),
                        "resolution": object_resolution_json(&resolution),
                    }),
                ),
            ),
            node_result,
        );
    };

    let response = session.apply_object_node_create_current(
        target,
        node,
        payload.view.clone(),
        Some(definition),
        None,
        Some(identity.client_id.clone()),
        payload
            .description
            .clone()
            .or_else(|| Some(format!("Realtime graph command {}", frame.message_id))),
    );
    let node_result =
        node_command_result(payload, Some(&resolution), Some(&node_id), Vec::new(), None);
    GraphCommandOutcome::with_node_result(response, node_result)
}

fn apply_object_replace_graph_command(
    session: &mut crate::RuntimeSession,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    payload: &GraphCommandPayload,
) -> GraphCommandOutcome {
    let Some(object_text) = payload.object_text.as_deref() else {
        return GraphCommandOutcome::from_response(graph_command_rejected_response(
            session,
            false,
            RuntimeDiagnostic::structured_error(
                "graph.command.object-text-required",
                "graph.command kind node.replace requires payload.objectText",
                json!({ "commandKind": payload.kind }),
            ),
        ));
    };
    let resolution = resolve_object_command_text(session, object_text);
    let node_id = payload.node_id.clone();
    let node_result = node_command_result(
        payload,
        Some(&resolution),
        node_id.as_deref(),
        Vec::new(),
        None,
    );
    let target = match validate_object_command_target(session, payload, false) {
        Ok(target) => target,
        Err(response) => return GraphCommandOutcome::with_node_result(response, node_result),
    };
    let Some(node_id) = node_id else {
        return GraphCommandOutcome::with_node_result(
            graph_command_rejected_response(
                session,
                false,
                RuntimeDiagnostic::structured_error(
                    "graph.command.node-id-required",
                    "graph.command kind node.replace requires payload.nodeId",
                    json!({ "commandKind": payload.kind, "target": target }),
                ),
            ),
            node_result,
        );
    };
    let Some((node, definition)) =
        materialize_object_command_node(session, payload, &resolution, &node_id)
    else {
        return GraphCommandOutcome::with_node_result(
            graph_command_rejected_response(
                session,
                false,
                RuntimeDiagnostic::structured_error(
                    "node.command.unresolved",
                    "object text could not be resolved for node.replace",
                    json!({
                        "commandKind": payload.kind,
                        "target": target,
                        "nodeId": node_id,
                        "objectText": object_text,
                        "unresolvedPolicy": object_unresolved_policy(payload),
                        "resolution": object_resolution_json(&resolution),
                    }),
                ),
            ),
            node_result,
        );
    };

    let (response, dropped_edge_ids) = session.apply_object_node_replace_current(
        target,
        node,
        payload.view.clone(),
        Some(definition),
        payload.interface_incident_edge_policy,
        None,
        Some(identity.client_id.clone()),
        payload
            .description
            .clone()
            .or_else(|| Some(format!("Realtime graph command {}", frame.message_id))),
    );
    let node_result = node_command_result(
        payload,
        Some(&resolution),
        Some(&node_id),
        dropped_edge_ids,
        None,
    );
    GraphCommandOutcome::with_node_result(response, node_result)
}

fn apply_node_delete_graph_command(
    session: &mut crate::RuntimeSession,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    payload: &GraphCommandPayload,
) -> GraphCommandOutcome {
    let node_id = payload.node_id.clone();
    let node_result = node_command_result(payload, None, node_id.as_deref(), Vec::new(), None);
    let target = match validate_object_command_target(session, payload, false) {
        Ok(target) => target,
        Err(response) => return GraphCommandOutcome::with_node_result(response, node_result),
    };
    let Some(node_id) = node_id else {
        return GraphCommandOutcome::with_node_result(
            graph_command_rejected_response(
                session,
                false,
                RuntimeDiagnostic::structured_error(
                    "graph.command.node-id-required",
                    "graph.command kind node.delete requires payload.nodeId",
                    json!({ "commandKind": payload.kind, "target": target }),
                ),
            ),
            node_result,
        );
    };

    let (response, dropped_edge_ids) = session.apply_node_delete_current(
        target,
        node_id.clone(),
        None,
        Some(identity.client_id.clone()),
        payload
            .description
            .clone()
            .or_else(|| Some(format!("Realtime graph command {}", frame.message_id))),
    );
    let node_result = node_command_result(payload, None, Some(&node_id), dropped_edge_ids, None);
    GraphCommandOutcome::with_node_result(response, node_result)
}

fn apply_node_update_graph_command(
    session: &mut crate::RuntimeSession,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    payload: &GraphCommandPayload,
) -> GraphCommandOutcome {
    let node_id = payload.node_id.clone();
    let node_result = node_command_result(payload, None, node_id.as_deref(), Vec::new(), None);
    let target = match validate_object_command_target(session, payload, false) {
        Ok(target) => target,
        Err(response) => return GraphCommandOutcome::with_node_result(response, node_result),
    };
    let Some(node_id) = node_id else {
        return GraphCommandOutcome::with_node_result(
            graph_command_rejected_response(
                session,
                false,
                RuntimeDiagnostic::structured_error(
                    "graph.command.node-id-required",
                    "graph.command kind node.update requires payload.nodeId",
                    json!({ "commandKind": payload.kind, "target": target }),
                ),
            ),
            node_result,
        );
    };
    let params = payload.params.clone().unwrap_or_default();
    if params.is_empty() {
        return GraphCommandOutcome::with_node_result(
            graph_command_rejected_response(
                session,
                false,
                RuntimeDiagnostic::structured_error(
                    "graph.command.params-required",
                    "graph.command kind node.update requires non-empty payload.params",
                    json!({
                        "commandKind": payload.kind,
                        "target": target,
                        "nodeId": node_id,
                    }),
                ),
            ),
            node_result,
        );
    }

    let response = session.apply_node_update_current(
        target,
        node_id.clone(),
        params,
        None,
        Some(identity.client_id.clone()),
        payload
            .description
            .clone()
            .or_else(|| Some(format!("Realtime graph command {}", frame.message_id))),
    );
    let node_result = node_command_result(payload, None, Some(&node_id), Vec::new(), None);
    GraphCommandOutcome::with_node_result(response, node_result)
}

fn validate_object_command_target(
    session: &crate::RuntimeSession,
    payload: &GraphCommandPayload,
    require_existing: bool,
) -> Result<GraphTargetRef, RuntimePatchResponse> {
    let Some(target) = payload.target.clone() else {
        return Err(graph_command_rejected_response(
            session,
            false,
            RuntimeDiagnostic::structured_error(
                "graph.command.target-required",
                format!(
                    "graph.command kind {} requires payload.target",
                    payload.kind
                ),
                json!({ "commandKind": payload.kind }),
            ),
        ));
    };
    if let Some(base_graph_revision) = payload.base_graph_revision.as_deref()
        && base_graph_revision != target.base_revision
    {
        return Err(graph_command_rejected_response(
            session,
            true,
            RuntimeDiagnostic::structured_error(
                "graph.command.target-revision-conflict",
                format!(
                    "baseGraphRevision {base_graph_revision} does not match target.baseRevision {}",
                    target.base_revision
                ),
                json!({
                    "expectedRevision": base_graph_revision,
                    "actualRevision": target.base_revision,
                    "target": target,
                    "commandKind": payload.kind,
                }),
            ),
        ));
    }
    match session.target_revision_current(&target) {
        Some(actual_revision) if actual_revision != target.base_revision => {
            Err(graph_command_rejected_response(
                session,
                true,
                RuntimeDiagnostic::structured_error(
                    "graph.command.target-revision-conflict",
                    format!(
                        "target baseRevision {} does not match target graph revision {}",
                        target.base_revision, actual_revision
                    ),
                    json!({
                        "expectedRevision": target.base_revision,
                        "actualRevision": actual_revision,
                        "target": target,
                        "commandKind": payload.kind,
                    }),
                ),
            ))
        }
        Some(_) => Ok(target),
        None if require_existing => Err(graph_command_rejected_response(
            session,
            false,
            RuntimeDiagnostic::structured_error(
                "node.target.missing-graph",
                "node target graph is not available in the active current 0.1 project",
                json!({ "target": target, "commandKind": payload.kind }),
            ),
        )),
        None => Ok(target),
    }
}

fn generated_node_id_for_create(
    session: &crate::RuntimeSession,
    target: &GraphTargetRef,
    resolution: &ObjectTextResolution,
) -> String {
    let base = node_id_slug(&resolution.display_text)
        .or_else(|| node_id_slug(&resolution.class_symbol))
        .unwrap_or_else(|| "node".to_owned());
    let used = session
        .project_document_current()
        .and_then(|project| graph_for_node_command_target(&project, target).cloned())
        .map(|graph| {
            graph
                .nodes
                .into_iter()
                .map(|node| node.id)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    next_generated_node_id(&base, &used)
}

fn graph_for_node_command_target<'a>(
    project: &'a crate::ProjectDocumentCurrent,
    target: &GraphTargetRef,
) -> Option<&'a crate::GraphDocumentCurrent> {
    match &target.path {
        PatchPath::Root => Some(&project.graph),
        PatchPath::HelpWorkingCopy {
            working_copy_id, ..
        } if working_copy_id == &project.graph.id => Some(&project.graph),
        PatchPath::ProjectPatchDefinition { patch_id } => project
            .patch_library
            .iter()
            .find(|patch| patch.id == *patch_id)
            .map(|patch| &patch.graph),
        PatchPath::HelpWorkingCopy { .. }
        | PatchPath::PackagePatchDefinition { .. }
        | PatchPath::EmbeddedPatchInstance { .. } => None,
    }
}

fn node_id_slug(input: &str) -> Option<String> {
    let mut slug = String::new();
    let mut previous_separator = false;
    for character in input.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            previous_separator = false;
        } else if !previous_separator && !slug.is_empty() {
            slug.push('_');
            previous_separator = true;
        }
    }
    while slug.ends_with('_') {
        slug.pop();
    }
    if slug.is_empty() {
        return None;
    }
    if slug
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_digit())
    {
        slug.insert_str(0, "node_");
    }
    Some(slug)
}

fn next_generated_node_id(base: &str, used: &[String]) -> String {
    if !used.iter().any(|node_id| node_id == base) {
        return base.to_owned();
    }
    for index in 2.. {
        let candidate = format!("{base}_{index}");
        if !used.iter().any(|node_id| node_id == &candidate) {
            return candidate;
        }
    }
    unreachable!("unbounded generated node id loop must return")
}

fn materialize_object_command_node(
    _session: &crate::RuntimeSession,
    payload: &GraphCommandPayload,
    resolution: &ObjectTextResolution,
    node_id: &str,
) -> Option<(crate::GraphNodeCurrent, crate::NodeDefinitionCurrent)> {
    if resolution.ok() {
        let mut node = materialize_object_text_node_v01(resolution, node_id).ok()?;
        merge_payload_params(&mut node.params, payload.params.as_ref());
        let definition = object_text_node_definition_v01(resolution)?;
        return Some((node, definition));
    }
    if object_unresolved_policy(payload) == ObjectUnresolvedPolicy::MaterializeDiagnostic {
        let mut node = materialize_unresolved_object_text_node_v01(resolution, node_id);
        merge_payload_params(&mut node.params, payload.params.as_ref());
        return Some((node, unresolved_object_text_node_definition_v01()));
    }
    None
}

fn merge_payload_params(params: &mut Map<String, Value>, overrides: Option<&Map<String, Value>>) {
    let Some(overrides) = overrides else {
        return;
    };
    for (key, value) in overrides {
        params.insert(key.clone(), value.clone());
    }
}

fn object_unresolved_policy(payload: &GraphCommandPayload) -> ObjectUnresolvedPolicy {
    payload
        .unresolved_policy
        .unwrap_or(ObjectUnresolvedPolicy::MaterializeDiagnostic)
}

fn object_text_runtime_diagnostics(resolution: &ObjectTextResolution) -> Vec<RuntimeDiagnostic> {
    resolution
        .diagnostics
        .iter()
        .map(|diagnostic| {
            RuntimeDiagnostic::structured_error(
                diagnostic.code.clone(),
                diagnostic.message.clone(),
                json!({
                    "surface": "object-text",
                    "objectText": resolution.input,
                    "displayText": resolution.display_text,
                    "classSymbol": resolution.class_symbol,
                    "candidateCount": resolution.candidates.len(),
                    "candidates": resolution.candidates.iter().map(object_text_candidate_json).collect::<Vec<_>>(),
                }),
            )
        })
        .collect()
}

fn node_command_result(
    payload: &GraphCommandPayload,
    resolution: Option<&ObjectTextResolution>,
    node_id: Option<&str>,
    dropped_edge_ids: Vec<String>,
    input: Option<Value>,
) -> Value {
    json!({
        "kind": payload.kind,
        "nodeId": node_id,
        "requestedNodeId": payload.requested_node_id,
        "target": payload.target,
        "objectText": payload.object_text,
        "unresolvedPolicy": object_unresolved_policy(payload),
        "interfaceIncidentEdgePolicy": payload.interface_incident_edge_policy,
        "droppedEdgeIds": dropped_edge_ids,
        "resolution": resolution.map(object_resolution_json),
        "input": input,
    })
}

fn node_input_result(
    request: &RuntimeControlEventRequest,
    response: &RuntimeControlEventResponse,
) -> Value {
    json!({
        "nodeId": request.node_id,
        "portId": request.port_id,
        "message": request.message,
        "accepted": response.ok,
        "changed": response.changed,
        "controlRevision": response.control_revision,
        "emitted": response.emitted,
    })
}

fn object_resolution_json(resolution: &ObjectTextResolution) -> Value {
    json!({
        "input": resolution.input,
        "displayText": resolution.display_text,
        "classSymbol": resolution.class_symbol,
        "resolved": resolution.ok(),
        "resolvedKind": resolution.resolved_kind,
        "resolvedKindVersion": resolution.resolved_kind_version,
        "candidateCount": resolution.candidates.len(),
        "candidates": resolution.candidates.iter().map(object_text_candidate_json).collect::<Vec<_>>(),
        "params": resolution.params,
        "ports": resolution.instance_ports.iter().map(object_text_port_json).collect::<Vec<_>>(),
        "diagnostics": resolution.diagnostics.iter().map(|diagnostic| {
            json!({
                "code": diagnostic.code,
                "message": diagnostic.message,
            })
        }).collect::<Vec<_>>(),
    })
}

fn object_text_candidate_json(candidate: &crate::object_text::ObjectTextCandidateSummary) -> Value {
    json!({
        "id": candidate.id,
        "source": candidate.source,
        "kind": candidate.kind,
        "displayName": candidate.display_name,
    })
}

fn object_text_port_json(port: &crate::object_text::ObjectTextPort) -> Value {
    json!({
        "id": port.id,
        "direction": match &port.direction {
            ObjectTextPortDirection::Input => "input",
            ObjectTextPortDirection::Output => "output",
        },
        "type": port.port_type,
        "rate": match &port.rate {
            ObjectTextPortRate::Event => "event",
            ObjectTextPortRate::Control => "control",
            ObjectTextPortRate::Audio => "audio",
            ObjectTextPortRate::Render => "render",
            ObjectTextPortRate::Gpu => "gpu",
            ObjectTextPortRate::Resource => "resource",
            ObjectTextPortRate::Io => "io",
        },
        "activation": port.activation.as_ref().map(|activation| match activation {
            ObjectTextPortActivation::Trigger => "trigger",
            ObjectTextPortActivation::Latched => "latched",
            ObjectTextPortActivation::Passive => "passive",
        }),
    })
}

fn graph_command_rejected_response(
    session: &crate::RuntimeSession,
    conflict: bool,
    diagnostic: RuntimeDiagnostic,
) -> RuntimePatchResponse {
    RuntimePatchResponse {
        ok: false,
        applied: false,
        conflict,
        snapshot: session.snapshot(),
        history: session.history(),
        diagnostics: vec![diagnostic],
    }
}

fn apply_control_command(
    record: &RuntimeSessionRecord,
    request: RuntimeControlEventRequest,
) -> (
    RuntimeControlEventResponse,
    BTreeMap<String, ControlValue>,
    RuntimeControlEventRequest,
) {
    let (mut response, control_snapshot, changed_values) = {
        let mut session = record
            .session
            .write()
            .expect("runtime session lock should not be poisoned");
        let before = session.control_state_response().values;
        let response = session.apply_control_event(request.clone());
        let after = session.control_state_response().values;
        let changed_values = changed_control_values(&before, &after);
        let control_snapshot = if response.ok && response.changed {
            session.preview_control_state_snapshot()
        } else {
            None
        };
        (response, control_snapshot, changed_values)
    };

    if let Some(control_snapshot) = control_snapshot {
        let mut preview = record
            .preview
            .lock()
            .expect("runtime preview lock should not be poisoned");
        if let Err(error) = preview.update_control_state(control_snapshot) {
            response
                .diagnostics
                .push(RuntimeDiagnostic::warning(format!(
                    "failed to update running preview control state: {error}"
                )));
        }
    }

    (response, changed_values, request)
}

fn changed_control_values(
    before: &BTreeMap<String, ControlValue>,
    after: &BTreeMap<String, ControlValue>,
) -> BTreeMap<String, ControlValue> {
    after
        .iter()
        .filter(|(node_id, value)| before.get(*node_id) != Some(*value))
        .map(|(node_id, value)| (node_id.clone(), value.clone()))
        .collect()
}

fn control_emitted_event(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    request: &RuntimeControlEventRequest,
    response: &mut RuntimeControlEventResponse,
    changed_values: BTreeMap<String, ControlValue>,
    position: RealtimeEventPosition<'_>,
) -> Option<RuntimeRealtimeEnvelope> {
    if response.emitted.is_empty() && changed_values.is_empty() {
        return None;
    }

    Some(RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: "control.emitted".to_owned(),
        message_id: format!("{}_control_{:06}", record.id, position.sequence),
        session_id: record.id.clone(),
        connection_id: Some(identity.connection_id.clone()),
        client_id: Some(identity.client_id.clone()),
        window_id: Some(identity.window_id.clone()),
        command_id: frame
            .command_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        correlation_id: frame
            .correlation_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        idempotency_key: frame.idempotency_key.clone(),
        sequence: Some(position.sequence),
        cursor: Some(position.cursor.to_owned()),
        created_at: Some(created_at_now()),
        payload: json!({
            "commandId": frame.command_id.clone().unwrap_or_else(|| frame.message_id.clone()),
            "correlationId": frame.correlation_id.clone().unwrap_or_else(|| frame.message_id.clone()),
            "idempotencyKey": frame.idempotency_key,
            "controlSequence": position.sequence,
            "controlRevision": response.control_revision,
            "changed": response.changed,
            "request": request,
            "emitted": response.emitted,
            "values": changed_values,
            "diagnostics": response.diagnostics,
            "replayed": false,
        }),
    })
}

fn command_ack(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    event_cursor: &str,
    cached: bool,
) -> RuntimeRealtimeEnvelope {
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: "command.ack".to_owned(),
        message_id: format!("{}_ack_{}", record.id, frame.message_id),
        session_id: record.id.clone(),
        connection_id: Some(identity.connection_id.clone()),
        client_id: Some(identity.client_id.clone()),
        window_id: Some(identity.window_id.clone()),
        command_id: frame
            .command_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        correlation_id: frame
            .correlation_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        idempotency_key: frame.idempotency_key.clone(),
        sequence: None,
        cursor: Some(record.realtime.current_cursor()),
        created_at: Some(created_at_now()),
        payload: json!({
            "accepted": true,
            "cached": cached,
            "eventCursor": event_cursor,
        }),
    }
}

fn command_ack_from_cached(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    cached: RuntimeRealtimeCachedCommandResult,
) -> RuntimeRealtimeEnvelope {
    command_ack_with_payload(
        record,
        identity,
        frame,
        mark_ack_payload_cached(cached.ack_payload),
    )
}

fn control_ack(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    response: &RuntimeControlEventResponse,
    sequence: u64,
    event_cursor: &str,
    cached: bool,
) -> RuntimeRealtimeEnvelope {
    control_ack_with_payload(
        record,
        identity,
        frame,
        json!({
            "status": if response.ok { "accepted" } else { "rejected" },
            "accepted": response.ok,
            "cached": cached,
            "changed": response.changed,
            "controlSequence": sequence,
            "controlRevision": response.control_revision,
            "commandId": frame.command_id.clone().unwrap_or_else(|| frame.message_id.clone()),
            "correlationId": frame.correlation_id.clone().unwrap_or_else(|| frame.message_id.clone()),
            "idempotencyKey": frame.idempotency_key,
            "eventCursor": event_cursor,
            "diagnostics": response.diagnostics,
        }),
    )
}

fn graph_ack(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    command: &GraphCommandPayload,
    response: &RuntimePatchResponse,
    node_result: Option<&Value>,
    position: RealtimeEventPosition<'_>,
    cached: bool,
) -> RuntimeRealtimeEnvelope {
    graph_ack_with_payload(
        record,
        identity,
        frame,
        json!({
            "status": if response.ok { "accepted" } else if response.conflict { "conflict" } else { "rejected" },
            "accepted": response.ok,
            "applied": response.applied,
            "conflict": response.conflict,
            "cached": cached,
            "graphSequence": position.sequence,
            "commandId": frame.command_id.clone().unwrap_or_else(|| frame.message_id.clone()),
            "correlationId": frame.correlation_id.clone().unwrap_or_else(|| frame.message_id.clone()),
            "idempotencyKey": frame.idempotency_key,
            "eventCursor": position.cursor,
            "kind": command.kind,
            "target": command.target,
            "surfacePath": command.surface_path,
            "baseSessionRevision": command.base_session_revision,
            "baseGraphRevision": command.base_graph_revision,
            "baseViewRevision": command.base_view_revision.or_else(|| command.view_patch.as_ref().map(|patch| patch.base_view_revision)),
            "node": node_result,
            "sessionRevision": response.snapshot.session_revision,
            "graphRevision": response.snapshot.graph_revision(),
            "viewRevision": response.snapshot.view_revision,
            "historySummary": {
                "latestEntryId": response.history.entries.last().map(|entry| entry.id.clone()),
                "canUndo": response.history.can_undo,
                "canRedo": response.history.can_redo,
                "undoDepth": response.history.undo_depth,
                "redoDepth": response.history.redo_depth,
            },
            "diagnostics": response.diagnostics,
        }),
    )
}

fn graph_ack_from_cached(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    cached: RuntimeRealtimeCachedCommandResult,
) -> RuntimeRealtimeEnvelope {
    let mut payload = mark_ack_payload_cached(cached.ack_payload);
    if let Some(object) = payload.as_object_mut() {
        object.insert("eventCursor".to_owned(), Value::String(cached.event_cursor));
    }
    graph_ack_with_payload(record, identity, frame, payload)
}

fn graph_applied_event(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    command: &GraphCommandPayload,
    response: &RuntimePatchResponse,
    node_result: Option<&Value>,
    sequence: u64,
    cursor: String,
) -> RuntimeRealtimeEnvelope {
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: "graph.applied".to_owned(),
        message_id: format!("{}_graph_{sequence:06}", record.id),
        session_id: record.id.clone(),
        connection_id: Some(identity.connection_id.clone()),
        client_id: Some(identity.client_id.clone()),
        window_id: Some(identity.window_id.clone()),
        command_id: frame
            .command_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        correlation_id: frame
            .correlation_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        idempotency_key: frame.idempotency_key.clone(),
        sequence: Some(sequence),
        cursor: Some(cursor),
        created_at: Some(created_at_now()),
        payload: json!({
            "commandId": frame.command_id.clone().unwrap_or_else(|| frame.message_id.clone()),
            "correlationId": frame.correlation_id.clone().unwrap_or_else(|| frame.message_id.clone()),
            "idempotencyKey": frame.idempotency_key,
            "graphSequence": sequence,
            "kind": command.kind,
            "target": command.target,
            "surfacePath": command.surface_path,
            "baseSessionRevision": command.base_session_revision,
            "baseGraphRevision": command.base_graph_revision,
            "baseViewRevision": command.base_view_revision.or_else(|| command.view_patch.as_ref().map(|patch| patch.base_view_revision)),
            "node": node_result,
            "sessionRevision": response.snapshot.session_revision,
            "graphRevision": response.snapshot.graph_revision(),
            "viewRevision": response.snapshot.view_revision,
            "historyEntryId": response.history.entries.last().map(|entry| entry.id.clone()),
            "diagnostics": response.diagnostics,
            "replayed": false,
        }),
    }
}

fn node_catalog_changed_event(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    snapshot: NodeCatalogSnapshotV01,
    sequence: u64,
    cursor: String,
) -> RuntimeRealtimeEnvelope {
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: "nodeCatalog.changed".to_owned(),
        message_id: format!("{}_node_catalog_changed_{sequence:06}", record.id),
        session_id: record.id.clone(),
        connection_id: Some(identity.connection_id.clone()),
        client_id: Some(identity.client_id.clone()),
        window_id: Some(identity.window_id.clone()),
        command_id: frame
            .command_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        correlation_id: frame
            .correlation_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        idempotency_key: frame.idempotency_key.clone(),
        sequence: Some(sequence),
        cursor: Some(cursor),
        created_at: Some(created_at_now()),
        payload: json!({
            "catalogRevision": snapshot.catalog_revision.clone(),
            "snapshot": snapshot,
            "replayed": false,
        }),
    }
}

fn control_ack_from_cached(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    cached: RuntimeRealtimeCachedCommandResult,
) -> RuntimeRealtimeEnvelope {
    let mut payload = mark_ack_payload_cached(cached.ack_payload);
    if let Some(object) = payload.as_object_mut() {
        object.insert("eventCursor".to_owned(), Value::String(cached.event_cursor));
    }
    control_ack_with_payload(record, identity, frame, payload)
}

fn command_ack_with_payload(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    payload: Value,
) -> RuntimeRealtimeEnvelope {
    ack_with_payload(record, identity, frame, "command.ack", payload)
}

fn control_ack_with_payload(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    payload: Value,
) -> RuntimeRealtimeEnvelope {
    ack_with_payload(record, identity, frame, "control.ack", payload)
}

fn graph_ack_with_payload(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    payload: Value,
) -> RuntimeRealtimeEnvelope {
    ack_with_payload(record, identity, frame, "graph.ack", payload)
}

fn node_catalog_response(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    message_type: &str,
    payload: Value,
) -> RuntimeRealtimeEnvelope {
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: message_type.to_owned(),
        message_id: format!("{}_node_catalog_{}", record.id, frame.message_id),
        session_id: record.id.clone(),
        connection_id: Some(identity.connection_id.clone()),
        client_id: Some(identity.client_id.clone()),
        window_id: Some(identity.window_id.clone()),
        command_id: frame
            .command_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        correlation_id: frame
            .correlation_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        idempotency_key: frame.idempotency_key.clone(),
        sequence: None,
        cursor: Some(record.realtime.current_cursor()),
        created_at: Some(created_at_now()),
        payload,
    }
}

fn ack_with_payload(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    message_type: &str,
    payload: Value,
) -> RuntimeRealtimeEnvelope {
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: message_type.to_owned(),
        message_id: format!("{}_ack_{}", record.id, frame.message_id),
        session_id: record.id.clone(),
        connection_id: Some(identity.connection_id.clone()),
        client_id: Some(identity.client_id.clone()),
        window_id: Some(identity.window_id.clone()),
        command_id: frame
            .command_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        correlation_id: frame
            .correlation_id
            .clone()
            .or_else(|| Some(frame.message_id.clone())),
        idempotency_key: frame.idempotency_key.clone(),
        sequence: None,
        cursor: Some(record.realtime.current_cursor()),
        created_at: Some(created_at_now()),
        payload,
    }
}

fn mark_ack_payload_cached(mut payload: Value) -> Value {
    if let Some(object) = payload.as_object_mut() {
        object.insert("cached".to_owned(), Value::Bool(true));
    }
    payload
}

fn hello_node_catalog_payload(
    record: &RuntimeSessionRecord,
    request: Option<&NodeCatalogHelloRequest>,
) -> Value {
    let snapshot = node_catalog_snapshot_for_record(record);
    match request.map(|request| request.mode).unwrap_or_default() {
        NodeCatalogHelloMode::None => node_catalog_status_payload("notRequested", snapshot, false),
        NodeCatalogHelloMode::IfChanged
            if catalog_revision_matches(
                request.and_then(|request| request.known_revision.as_ref()),
                &snapshot.catalog_revision,
            ) =>
        {
            node_catalog_status_payload("unchanged", snapshot, false)
        }
        NodeCatalogHelloMode::IfChanged | NodeCatalogHelloMode::Always => {
            node_catalog_status_payload("included", snapshot, true)
        }
    }
}

fn node_catalog_status_payload(
    status: &str,
    snapshot: NodeCatalogSnapshotV01,
    include_snapshot: bool,
) -> Value {
    let mut payload = json!({
        "status": status,
        "catalogRevision": snapshot.catalog_revision.clone(),
    });
    if include_snapshot && let Some(object) = payload.as_object_mut() {
        object.insert(
            "snapshot".to_owned(),
            serde_json::to_value(snapshot).expect("node catalog snapshot should serialize"),
        );
    }
    payload
}

fn catalog_revision_matches(
    known_revision: Option<&Value>,
    catalog_revision: &PackageChecksumV01,
) -> bool {
    let Some(known_revision) = known_revision else {
        return false;
    };
    if known_revision.as_str() == Some(catalog_revision.value.as_str()) {
        return true;
    }
    serde_json::to_value(catalog_revision).expect("node catalog revision should serialize")
        == *known_revision
}

fn node_catalog_snapshot_response_payload(snapshot: NodeCatalogSnapshotV01) -> Value {
    node_catalog_status_payload("included", snapshot, true)
}

fn node_catalog_unchanged_response_payload(snapshot: NodeCatalogSnapshotV01) -> Value {
    node_catalog_status_payload("unchanged", snapshot, false)
}

fn session_attached(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    snapshot: &RuntimeSessionSnapshot,
    node_catalog: Option<&NodeCatalogHelloRequest>,
) -> RuntimeRealtimeEnvelope {
    let node_catalog = hello_node_catalog_payload(record, node_catalog);
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: "session.attached".to_owned(),
        message_id: format!("{}_attached_{}", record.id, identity.connection_id),
        session_id: record.id.clone(),
        connection_id: Some(identity.connection_id.clone()),
        client_id: Some(identity.client_id.clone()),
        window_id: Some(identity.window_id.clone()),
        command_id: None,
        correlation_id: Some(frame.message_id.clone()),
        idempotency_key: None,
        sequence: None,
        cursor: Some(record.realtime.current_cursor()),
        created_at: Some(created_at_now()),
        payload: json!({
            "connectionId": identity.connection_id,
            "clientId": identity.client_id,
            "windowId": identity.window_id,
            "resumeToken": identity.resume_token,
            "currentRevisions": current_revisions(snapshot),
            "snapshot": snapshot,
            "globalCursor": record.realtime.current_cursor(),
            "nodeCatalog": node_catalog,
        }),
    }
}

fn session_sync_required(
    record: &RuntimeSessionRecord,
    identity: &RuntimeRealtimeConnectionIdentity,
    frame: &RuntimeRealtimeEnvelope,
    snapshot: &RuntimeSessionSnapshot,
    diagnostic: RuntimeRealtimeDiagnostic,
    node_catalog: Option<&NodeCatalogHelloRequest>,
) -> RuntimeRealtimeEnvelope {
    let node_catalog = hello_node_catalog_payload(record, node_catalog);
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: "session.syncRequired".to_owned(),
        message_id: format!("{}_sync_required_{}", record.id, identity.connection_id),
        session_id: record.id.clone(),
        connection_id: Some(identity.connection_id.clone()),
        client_id: Some(identity.client_id.clone()),
        window_id: Some(identity.window_id.clone()),
        command_id: None,
        correlation_id: Some(frame.message_id.clone()),
        idempotency_key: None,
        sequence: None,
        cursor: Some(record.realtime.current_cursor()),
        created_at: Some(created_at_now()),
        payload: json!({
            "connectionId": identity.connection_id,
            "clientId": identity.client_id,
            "windowId": identity.window_id,
            "resumeToken": identity.resume_token,
            "currentRevisions": current_revisions(snapshot),
            "snapshot": snapshot,
            "globalCursor": record.realtime.current_cursor(),
            "nodeCatalog": node_catalog,
            "diagnostic": diagnostic,
        }),
    }
}

fn runtime_error(
    session_id: &str,
    identity: Option<&RuntimeRealtimeConnectionIdentity>,
    frame: Option<&RuntimeRealtimeEnvelope>,
    code: &str,
    message: impl Into<String>,
    details: Option<Value>,
) -> RuntimeRealtimeEnvelope {
    let diagnostic = RuntimeRealtimeDiagnostic {
        code: code.to_owned(),
        message: message.into(),
        details,
    };
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: "runtime.error".to_owned(),
        message_id: format!(
            "{}_error_{}",
            session_id,
            created_at_now().replace(':', "_")
        ),
        session_id: session_id.to_owned(),
        connection_id: identity.map(|identity| identity.connection_id.clone()),
        client_id: identity.map(|identity| identity.client_id.clone()),
        window_id: identity.map(|identity| identity.window_id.clone()),
        command_id: frame.and_then(|frame| frame.command_id.clone()),
        correlation_id: frame.map(|frame| frame.message_id.clone()),
        idempotency_key: frame.and_then(|frame| frame.idempotency_key.clone()),
        sequence: None,
        cursor: None,
        created_at: Some(created_at_now()),
        payload: json!({ "diagnostic": diagnostic }),
    }
}

fn empty_correlation_frame(session_id: &str) -> RuntimeRealtimeEnvelope {
    RuntimeRealtimeEnvelope {
        schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
        schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
        message_type: "runtime.internal".to_owned(),
        message_id: "runtime-internal".to_owned(),
        session_id: session_id.to_owned(),
        connection_id: None,
        client_id: None,
        window_id: None,
        command_id: None,
        correlation_id: None,
        idempotency_key: None,
        sequence: None,
        cursor: None,
        created_at: None,
        payload: Value::Null,
    }
}

fn generate_resume_token() -> String {
    let mut bytes = [0_u8; RUNTIME_REALTIME_RESUME_TOKEN_BYTES];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .expect("operating system random source should be available for realtime resume tokens");

    let mut token = String::with_capacity("rtresume-".len() + bytes.len() * 2);
    token.push_str("rtresume-");
    for byte in bytes {
        token.push_str(&format!("{byte:02x}"));
    }
    token
}

fn current_snapshot(record: &RuntimeSessionRecord) -> RuntimeSessionSnapshot {
    record
        .session
        .read()
        .expect("runtime session lock should not be poisoned")
        .snapshot()
}

pub(crate) fn node_catalog_snapshot_for_record(
    record: &RuntimeSessionRecord,
) -> NodeCatalogSnapshotV01 {
    let session = record
        .session
        .read()
        .expect("runtime session lock should not be poisoned");
    node_catalog_snapshot_for_session(&session)
}

fn node_catalog_snapshot_for_session(session: &crate::RuntimeSession) -> NodeCatalogSnapshotV01 {
    session.node_catalog_snapshot()
}

fn current_revisions(snapshot: &RuntimeSessionSnapshot) -> RuntimeRealtimeSessionRevisions {
    RuntimeRealtimeSessionRevisions {
        session_revision: snapshot.session_revision,
        view_revision: snapshot.view_revision,
        control_revision: snapshot.control_revision,
        graph_revision: snapshot.graph_revision().map(ToOwned::to_owned),
    }
}

#[cfg(test)]
pub(crate) fn validate_value_occurrence_header_for_session_binding<'a>(
    header: &ValueOccurrenceHeader,
    binding_formats: &'a [EndpointBindingValueFormat],
) -> Result<&'a EndpointBindingValueFormat, RuntimeDiagnostic> {
    if let Err(report) = skenion_contracts::validate_value_occurrence_header_v01(header) {
        return Err(RuntimeDiagnostic::structured_error(
            "runtime.value-binding.invalid-header",
            "invalid value occurrence header",
            json!({
                "bindingId": header.binding_id,
                "errors": report
                    .errors()
                    .iter()
                    .map(|error| error.message.clone())
                    .collect::<Vec<_>>(),
            }),
        ));
    }

    let Some(binding_format) = binding_formats
        .iter()
        .find(|binding_format| binding_format.binding_id == header.binding_id)
    else {
        return Err(RuntimeDiagnostic::structured_error(
            "runtime.value-binding.unknown-binding",
            "value occurrence header references an unknown binding",
            json!({
                "bindingId": header.binding_id,
            }),
        ));
    };

    if binding_format.binding_epoch != header.binding_epoch {
        return Err(RuntimeDiagnostic::structured_error(
            "runtime.value-binding.stale-epoch",
            "value occurrence header binding epoch does not match the current binding",
            json!({
                "bindingId": header.binding_id,
                "expectedBindingEpoch": binding_format.binding_epoch,
                "receivedBindingEpoch": header.binding_epoch,
            }),
        ));
    }

    if binding_format.format_revision != header.format_revision {
        return Err(RuntimeDiagnostic::structured_error(
            "runtime.value-binding.stale-format-revision",
            "value occurrence header format revision does not match the current binding",
            json!({
                "bindingId": header.binding_id,
                "expectedFormatRevision": binding_format.format_revision,
                "receivedFormatRevision": header.format_revision,
            }),
        ));
    }

    Ok(binding_format)
}

fn mark_replayed(mut event: RuntimeRealtimeEnvelope) -> RuntimeRealtimeEnvelope {
    if let Some(payload) = event.payload.as_object_mut() {
        payload.insert("replayed".to_owned(), Value::Bool(true));
        if let Some(presence) = payload
            .get_mut("presence")
            .and_then(|presence| presence.as_object_mut())
        {
            presence.insert("replayed".to_owned(), Value::Bool(true));
        }
    }
    event
}

fn sync_required_diagnostic(
    code: &str,
    message: impl Into<String>,
    details: Option<Value>,
) -> RuntimeRealtimeDiagnostic {
    RuntimeRealtimeDiagnostic {
        code: code.to_owned(),
        message: message.into(),
        details,
    }
}

async fn send_frame(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    frame: &RuntimeRealtimeEnvelope,
) -> Result<(), axum::Error> {
    sender
        .send(Message::Text(
            serde_json::to_string(frame)
                .expect("runtime realtime frame should serialize")
                .into(),
        ))
        .await
}

fn unix_ms_timestamp(time: SystemTime) -> String {
    let millis = time
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("unix-ms:{millis}")
}

fn trim_btree_map_by_key<T>(map: &mut BTreeMap<String, T>, retention_limit: usize) {
    if map.len() <= retention_limit {
        return;
    }
    let remove_count = map.len().saturating_sub(retention_limit);
    let keys = map.keys().take(remove_count).cloned().collect::<Vec<_>>();
    for key in keys {
        map.remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_binding_format() -> EndpointBindingValueFormat {
        EndpointBindingValueFormat {
            binding_id: "edge_value_target".to_owned(),
            binding_epoch: 2,
            format_revision: 7,
            format_digest: None,
            value_format: crate::ValueFormat {
                value_type_id: "value.core.float32".to_owned(),
                format: Some("f32".to_owned()),
                shape: None,
                dynamic_shape: None,
                layout: None,
                strides: None,
                byte_length: None,
                sample_rate: None,
                channels: None,
                channel_layout: None,
                color_space: None,
                color_range: None,
                transfer: None,
                primaries: None,
                alpha_policy: None,
                resource_kind: None,
            },
            source: Some(crate::ValueEndpointRef {
                node_id: "value_1".to_owned(),
                port_id: "value".to_owned(),
            }),
            target: Some(crate::ValueEndpointRef {
                node_id: "target_1".to_owned(),
                port_id: "cold".to_owned(),
            }),
            delivery: None,
        }
    }

    fn test_occurrence_header() -> ValueOccurrenceHeader {
        ValueOccurrenceHeader {
            binding_id: "edge_value_target".to_owned(),
            binding_epoch: 2,
            format_revision: 7,
            sequence: 1,
            clock: None,
            timestamp: None,
            payload_kind: crate::ValuePayloadKind::Json,
            byte_length: None,
            byte_offset: None,
            actual_shape: None,
            flags: None,
            dropped_before: None,
            duration: None,
        }
    }

    fn realtime_event(session_id: &str, sequence: u64, cursor: &str) -> RuntimeRealtimeEnvelope {
        RuntimeRealtimeEnvelope {
            schema: RUNTIME_REALTIME_SCHEMA.to_owned(),
            schema_version: RUNTIME_REALTIME_SCHEMA_VERSION.to_owned(),
            message_type: "presence.updated".to_owned(),
            message_id: format!("{session_id}_presence_{sequence:06}"),
            session_id: session_id.to_owned(),
            connection_id: None,
            client_id: None,
            window_id: None,
            command_id: None,
            correlation_id: None,
            idempotency_key: None,
            sequence: Some(sequence),
            cursor: Some(cursor.to_owned()),
            created_at: Some(created_at_now()),
            payload: json!({ "replayed": false }),
        }
    }

    #[test]
    fn value_occurrence_header_guard_accepts_current_binding() {
        let binding = test_binding_format();
        let header = test_occurrence_header();
        let binding_formats = [binding.clone()];

        let accepted =
            validate_value_occurrence_header_for_session_binding(&header, &binding_formats)
                .expect("current binding should be accepted");

        assert_eq!(accepted, &binding);
    }

    #[test]
    fn value_occurrence_header_guard_rejects_invalid_header() {
        let mut header = test_occurrence_header();
        header.binding_id.clear();

        let diagnostic =
            validate_value_occurrence_header_for_session_binding(&header, &[test_binding_format()])
                .expect_err("invalid header should be rejected");

        assert_eq!(
            diagnostic.code.as_deref(),
            Some("runtime.value-binding.invalid-header")
        );
    }

    #[test]
    fn value_occurrence_header_guard_rejects_unknown_binding() {
        let mut header = test_occurrence_header();
        header.binding_id = "missing_edge".to_owned();

        let diagnostic =
            validate_value_occurrence_header_for_session_binding(&header, &[test_binding_format()])
                .expect_err("unknown binding should be rejected");

        assert_eq!(
            diagnostic.code.as_deref(),
            Some("runtime.value-binding.unknown-binding")
        );
    }

    #[test]
    fn value_occurrence_header_guard_rejects_stale_binding_metadata() {
        let binding = test_binding_format();
        let mut stale_epoch = test_occurrence_header();
        stale_epoch.binding_epoch = 1;
        let epoch_diagnostic = validate_value_occurrence_header_for_session_binding(
            &stale_epoch,
            std::slice::from_ref(&binding),
        )
        .expect_err("stale epoch should be rejected");
        assert_eq!(
            epoch_diagnostic.code.as_deref(),
            Some("runtime.value-binding.stale-epoch")
        );

        let mut stale_format = test_occurrence_header();
        stale_format.format_revision = 6;
        let format_diagnostic =
            validate_value_occurrence_header_for_session_binding(&stale_format, &[binding])
                .expect_err("stale format revision should be rejected");
        assert_eq!(
            format_diagnostic.code.as_deref(),
            Some("runtime.value-binding.stale-format-revision")
        );
    }

    #[test]
    fn idempotency_results_follow_retained_event_window() {
        let state = RuntimeRealtimeState::new("default", 2);
        let identity = state.issue_connection_identity(None);

        for sequence in 1..=3 {
            let cursor = state.cursor_for(sequence);
            let idempotency_key = format!("key-{sequence}");
            state.remember_ack(RememberAckInput {
                identity: &identity,
                message_type: "presence.update",
                idempotency_key: &idempotency_key,
                event_cursor: &cursor,
                event_sequence: sequence,
                ack_payload: json!({ "eventCursor": cursor }),
                emitted_result: None,
            });
            state.publish(realtime_event("default", sequence, &cursor));
        }

        let idempotency_results = state
            .idempotency_results
            .lock()
            .expect("runtime realtime idempotency lock should not be poisoned");
        assert_eq!(idempotency_results.len(), 2);
        assert!(
            !idempotency_results.contains_key(&RuntimeRealtimeIdempotencyScope {
                client_id: identity.client_id.clone(),
                window_id: identity.window_id.clone(),
                message_type: "presence.update".to_owned(),
                idempotency_key: "key-1".to_owned(),
            })
        );
        assert!(
            idempotency_results.contains_key(&RuntimeRealtimeIdempotencyScope {
                client_id: identity.client_id.clone(),
                window_id: identity.window_id.clone(),
                message_type: "presence.update".to_owned(),
                idempotency_key: "key-2".to_owned(),
            })
        );
        assert!(
            idempotency_results.contains_key(&RuntimeRealtimeIdempotencyScope {
                client_id: identity.client_id.clone(),
                window_id: identity.window_id.clone(),
                message_type: "presence.update".to_owned(),
                idempotency_key: "key-3".to_owned(),
            })
        );
    }

    #[test]
    fn presence_entries_are_ttl_pruned_and_count_bounded() {
        let state = RuntimeRealtimeState::new("default", 1);
        let now = SystemTime::now();
        let expired_at = now
            .checked_sub(Duration::from_secs(1))
            .expect("test time should support subtraction");
        let future = now + Duration::from_secs(60);
        let expired = state.issue_connection_identity(None);
        let active_a = state.issue_connection_identity(None);
        let active_b = state.issue_connection_identity(None);
        let active_c = state.issue_connection_identity(None);

        state.remember_presence(&expired, json!({ "client": "expired" }), expired_at, 1);
        state.remember_presence(&active_a, json!({ "client": "a" }), future, 2);
        state.remember_presence(&active_b, json!({ "client": "b" }), future, 3);
        state.remember_presence(&active_c, json!({ "client": "c" }), future, 4);

        let presence = state
            .presence
            .lock()
            .expect("runtime realtime presence lock should not be poisoned");
        assert_eq!(presence.len(), 2);
        assert!(!presence.contains_key(&format!("{}:{}", expired.client_id, expired.window_id)));
        assert!(!presence.contains_key(&format!("{}:{}", active_a.client_id, active_a.window_id)));
        assert!(presence.contains_key(&format!("{}:{}", active_b.client_id, active_b.window_id)));
        assert!(presence.contains_key(&format!("{}:{}", active_c.client_id, active_c.window_id)));
    }
}
