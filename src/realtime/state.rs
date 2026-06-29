use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::SystemTime,
};

use rand::TryRngCore;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::runtime_time::created_at_now;

use super::{
    protocol::{
        RUNTIME_REALTIME_PRESENCE_LIMIT_MULTIPLIER, RUNTIME_REALTIME_RESUME_TOKEN_BYTES,
        RUNTIME_REALTIME_RESUME_TOKEN_TTL,
    },
    wire::{
        RuntimeRealtimeConnectionIdentity, RuntimeRealtimeDiagnostic, RuntimeRealtimeEnvelope,
        RuntimeRealtimeReplay,
    },
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct RuntimeRealtimeIdempotencyScope {
    pub(super) client_id: String,
    pub(super) window_id: String,
    pub(super) message_type: String,
    pub(super) idempotency_key: String,
}

#[derive(Debug, Clone)]
pub(super) struct RuntimeRealtimeIdempotencyEntry {
    event_cursor: String,
    event_sequence: u64,
    ack_payload: Value,
    emitted_results: Vec<RuntimeRealtimeEnvelope>,
    inserted_at: SystemTime,
}

#[derive(Debug, Clone)]
pub(super) struct RuntimeRealtimeCachedCommandResult {
    pub(super) event_cursor: String,
    pub(super) ack_payload: Value,
    pub(super) emitted_results: Vec<RuntimeRealtimeEnvelope>,
}

#[derive(Debug, Clone)]
pub(super) struct RuntimeRealtimePresenceEntry {
    presence: Value,
    expires_at: SystemTime,
    updated_sequence: u64,
}

#[derive(Debug, Clone)]
pub(super) struct RuntimeRealtimeResumeIdentity {
    pub(super) client_id: String,
    pub(super) window_id: String,
    pub(super) expires_at: SystemTime,
}

#[derive(Debug)]
pub struct RuntimeRealtimeState {
    events: broadcast::Sender<RuntimeRealtimeEnvelope>,
    event_store: Mutex<VecDeque<RuntimeRealtimeEnvelope>>,
    event_sequence: Mutex<u64>,
    connection_sequence: Mutex<u64>,
    replay_limit: usize,
    incarnation_id: String,
    pub(super) idempotency_results:
        Mutex<BTreeMap<RuntimeRealtimeIdempotencyScope, RuntimeRealtimeIdempotencyEntry>>,
    pub(super) presence: Mutex<BTreeMap<String, RuntimeRealtimePresenceEntry>>,
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

    pub(super) fn issue_connection_identity(
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

    pub(super) fn consume_resume_token(
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

    pub(super) fn next_event_sequence(&self) -> u64 {
        let mut sequence = self
            .event_sequence
            .lock()
            .expect("runtime realtime event sequence lock should not be poisoned");
        let current = *sequence;
        *sequence += 1;
        current
    }

    pub(super) fn cursor_for(&self, sequence: u64) -> String {
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

    pub(super) fn cached_command_result(
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
                emitted_results: entry.emitted_results.clone(),
            })
    }

    pub(super) fn remember_ack(&self, ack: RememberAckInput<'_>) {
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
                emitted_results: ack.emitted_results,
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

    pub(super) fn remember_presence(
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

    pub(super) fn publish(&self, event: RuntimeRealtimeEnvelope) {
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

pub(super) struct RememberAckInput<'a> {
    pub(super) identity: &'a RuntimeRealtimeConnectionIdentity,
    pub(super) message_type: &'a str,
    pub(super) idempotency_key: &'a str,
    pub(super) event_cursor: &'a str,
    pub(super) event_sequence: u64,
    pub(super) ack_payload: Value,
    pub(super) emitted_results: Vec<RuntimeRealtimeEnvelope>,
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

pub(super) fn sync_required_diagnostic(
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
