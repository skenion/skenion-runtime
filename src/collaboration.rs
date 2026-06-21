use std::{
    collections::{BTreeMap, VecDeque},
    convert::Infallible,
    sync::{Arc, Mutex, MutexGuard},
};

use axum::response::sse::Event;
use skenion_contracts::{
    RuntimeCollaborationEventEnvelope, RuntimeCollaborationEventKind,
    RuntimeCollaborationEventPayload, RuntimeCollaborationOperationResult,
    RuntimeCollaborationPresenceEnvelope, RuntimeCollaborationSelectionEnvelope,
    RuntimeEventReplayGap, RuntimeEventReplayGapReason, RuntimeEventReplayMetadata,
    validate_runtime_collaboration_event_envelope,
};
use tokio::sync::broadcast;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::{Edge, runtime_time::created_at_now};

pub const COLLABORATION_EVENT_REPLAY_LIMIT: usize = 256;

#[derive(Debug, Clone)]
pub struct RuntimeCollaborationReplay {
    pub events: Vec<RuntimeCollaborationEventEnvelope>,
    pub high_water_sequence: u64,
}

#[derive(Debug)]
pub struct RuntimeCollaborationLog {
    pub events: broadcast::Sender<RuntimeCollaborationEventEnvelope>,
    event_store: Mutex<VecDeque<RuntimeCollaborationEventEnvelope>>,
    event_sequence: Mutex<u64>,
    operation_lock: Mutex<()>,
    replay_limit: usize,
    idempotency_results: Mutex<BTreeMap<String, RuntimeCollaborationOperationResult>>,
    presence: Mutex<BTreeMap<String, RuntimeCollaborationPresenceEnvelope>>,
    selection: Mutex<BTreeMap<String, RuntimeCollaborationSelectionEnvelope>>,
    edge_ids: Mutex<BTreeMap<String, Edge>>,
}

impl RuntimeCollaborationLog {
    pub fn new(replay_limit: usize) -> Arc<Self> {
        let (events, _) = broadcast::channel(replay_limit);
        Arc::new(Self {
            events,
            event_store: Mutex::new(VecDeque::new()),
            event_sequence: Mutex::new(1),
            operation_lock: Mutex::new(()),
            replay_limit,
            idempotency_results: Mutex::new(BTreeMap::new()),
            presence: Mutex::new(BTreeMap::new()),
            selection: Mutex::new(BTreeMap::new()),
            edge_ids: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn reserve_sequence(&self) -> u64 {
        let mut sequence = self
            .event_sequence
            .lock()
            .expect("runtime collaboration event sequence lock should not be poisoned");
        let current = *sequence;
        *sequence += 1;
        current
    }

    pub fn current_sequence(&self) -> u64 {
        let next_sequence = self
            .event_sequence
            .lock()
            .expect("runtime collaboration event sequence lock should not be poisoned");
        next_sequence.saturating_sub(1)
    }

    pub fn operation_guard(&self) -> MutexGuard<'_, ()> {
        self.operation_lock
            .lock()
            .expect("runtime collaboration operation lock should not be poisoned")
    }

    pub fn remember_result(&self, result: RuntimeCollaborationOperationResult) {
        self.idempotency_results
            .lock()
            .expect("runtime collaboration idempotency lock should not be poisoned")
            .insert(result.idempotency_key.clone(), result);
    }

    pub fn has_idempotency_key(&self, idempotency_key: &str) -> bool {
        self.idempotency_results
            .lock()
            .expect("runtime collaboration idempotency lock should not be poisoned")
            .contains_key(idempotency_key)
    }

    pub fn remember_edge_id(&self, edge_id: String, edge: Edge) {
        self.edge_ids
            .lock()
            .expect("runtime collaboration edge id lock should not be poisoned")
            .insert(edge_id, edge);
    }

    pub fn forget_edge_id(&self, edge_id: &str) {
        self.edge_ids
            .lock()
            .expect("runtime collaboration edge id lock should not be poisoned")
            .remove(edge_id);
    }

    pub fn forget_incident_edge_ids(&self, node_id: &str) {
        self.edge_ids
            .lock()
            .expect("runtime collaboration edge id lock should not be poisoned")
            .retain(|_, edge| edge.from.node != node_id && edge.to.node != node_id);
    }

    pub fn edge_by_id(&self, edge_id: &str) -> Option<Edge> {
        self.edge_ids
            .lock()
            .expect("runtime collaboration edge id lock should not be poisoned")
            .get(edge_id)
            .cloned()
    }

    pub fn publish_operation_result(
        &self,
        session_id: &str,
        sequence: u64,
        result: RuntimeCollaborationOperationResult,
    ) -> RuntimeCollaborationEventEnvelope {
        let event = self.event(
            session_id,
            sequence,
            RuntimeCollaborationEventKind::OperationResult,
            RuntimeCollaborationEventPayload::OperationResult {
                result: Box::new(result),
            },
        );
        self.publish(event.clone());
        event
    }

    pub fn publish_presence(
        &self,
        sequence: u64,
        presence: RuntimeCollaborationPresenceEnvelope,
    ) -> RuntimeCollaborationEventEnvelope {
        let session_id = presence.session_id.clone();
        self.presence
            .lock()
            .expect("runtime collaboration presence lock should not be poisoned")
            .insert(presence.participant_id.clone(), presence.clone());
        let event = self.event(
            &session_id,
            sequence,
            RuntimeCollaborationEventKind::Presence,
            RuntimeCollaborationEventPayload::Presence {
                presence: Box::new(presence),
            },
        );
        self.publish(event.clone());
        event
    }

    pub fn publish_selection(
        &self,
        sequence: u64,
        selection: RuntimeCollaborationSelectionEnvelope,
    ) -> RuntimeCollaborationEventEnvelope {
        let session_id = selection.session_id.clone();
        self.selection
            .lock()
            .expect("runtime collaboration selection lock should not be poisoned")
            .insert(selection.participant_id.clone(), selection.clone());
        let event = self.event(
            &session_id,
            sequence,
            RuntimeCollaborationEventKind::Selection,
            RuntimeCollaborationEventPayload::Selection {
                selection: Box::new(selection),
            },
        );
        self.publish(event.clone());
        event
    }

    pub fn capture_replay(&self, after: Option<u64>) -> RuntimeCollaborationReplay {
        let high_water_sequence = self
            .latest_stored_sequence()
            .unwrap_or_else(|| self.current_sequence());
        let store = self
            .event_store
            .lock()
            .expect("runtime collaboration event store lock should not be poisoned");
        let Some(after) = after else {
            return RuntimeCollaborationReplay {
                events: store.iter().cloned().collect(),
                high_water_sequence,
            };
        };

        let earliest = store.front().map(|event| event.sequence);
        let mut replay = Vec::new();
        for event in store.iter().filter(|event| event.sequence > after) {
            let mut event = event.clone();
            event.replay.replayed = true;
            validate_runtime_collaboration_event_envelope(&event)
                .expect("runtime collaboration replay event should validate");
            replay.push(event);
        }
        if let Some(earliest) = earliest
            && after + 1 < earliest
            && let Some(snapshot) = store.front()
        {
            replay.insert(
                0,
                collaboration_gap_event(
                    snapshot.clone(),
                    after,
                    earliest,
                    RuntimeEventReplayGapReason::RetentionOverflow,
                ),
            );
        }

        RuntimeCollaborationReplay {
            events: replay,
            high_water_sequence,
        }
    }

    pub fn stream_lag_gap_event(
        &self,
        session_id: &str,
        skipped: u64,
    ) -> Option<RuntimeCollaborationEventEnvelope> {
        let store = self
            .event_store
            .lock()
            .expect("runtime collaboration event store lock should not be poisoned");
        let snapshot = store.front().cloned()?;
        let actual_sequence = snapshot.sequence.max(2);
        let skipped = skipped.max(1);
        let expected_sequence = actual_sequence.saturating_sub(skipped).max(1);
        Some(collaboration_gap_event(
            snapshot,
            expected_sequence.saturating_sub(1),
            actual_sequence,
            RuntimeEventReplayGapReason::StreamReset,
        ))
        .map(|mut event| {
            event.event_id = format!(
                "{session_id}_collaboration_stream_gap_{expected_sequence:06}_{actual_sequence:06}"
            );
            event.session_id = session_id.to_owned();
            validate_runtime_collaboration_event_envelope(&event)
                .expect("runtime collaboration stream lag gap event should validate");
            event
        })
    }

    fn event(
        &self,
        session_id: &str,
        sequence: u64,
        kind: RuntimeCollaborationEventKind,
        payload: RuntimeCollaborationEventPayload,
    ) -> RuntimeCollaborationEventEnvelope {
        RuntimeCollaborationEventEnvelope {
            schema: "skenion.runtime.collaboration.event".to_owned(),
            schema_version: "0.1.0".to_owned(),
            event_id: format!("{session_id}_collaboration_{sequence:06}"),
            session_id: session_id.to_owned(),
            sequence,
            causal: collaboration_event_causal(sequence),
            kind,
            payload,
            replay: RuntimeEventReplayMetadata {
                cursor: sequence.to_string(),
                previous_cursor: sequence
                    .checked_sub(1)
                    .filter(|previous| *previous > 0)
                    .map(|previous| previous.to_string()),
                replayed: false,
                gap: None,
                overflow: false,
            },
            created_at: created_at_now(),
        }
    }

    fn publish(&self, event: RuntimeCollaborationEventEnvelope) {
        validate_runtime_collaboration_event_envelope(&event)
            .expect("runtime collaboration event should validate");
        let mut store = self
            .event_store
            .lock()
            .expect("runtime collaboration event store lock should not be poisoned");
        store.push_back(event.clone());
        while store.len() > self.replay_limit {
            store.pop_front();
        }
        let _ = self.events.send(event);
    }

    fn latest_stored_sequence(&self) -> Option<u64> {
        self.event_store
            .lock()
            .expect("runtime collaboration event store lock should not be poisoned")
            .back()
            .map(|event| event.sequence)
    }
}

pub fn collaboration_event(event: RuntimeCollaborationEventEnvelope) -> Result<Event, Infallible> {
    Ok(Event::default()
        .event("collaboration")
        .id(event.replay.cursor.clone())
        .json_data(event)
        .expect("runtime collaboration event should serialize"))
}

pub fn collaboration_broadcast_event_after_high_water(
    result: Result<RuntimeCollaborationEventEnvelope, BroadcastStreamRecvError>,
    log: &RuntimeCollaborationLog,
    session_id: &str,
    high_water_sequence: u64,
) -> Option<Result<Event, Infallible>> {
    match result {
        Ok(event) if event.sequence <= high_water_sequence => None,
        Ok(event) => Some(collaboration_event(event)),
        Err(BroadcastStreamRecvError::Lagged(skipped)) => log
            .stream_lag_gap_event(session_id, skipped)
            .map(collaboration_event),
    }
}

fn collaboration_gap_event(
    mut event: RuntimeCollaborationEventEnvelope,
    after: u64,
    actual_sequence: u64,
    reason: RuntimeEventReplayGapReason,
) -> RuntimeCollaborationEventEnvelope {
    let expected_sequence = after + 1;
    event.event_id = format!(
        "{}_collaboration_gap_{expected_sequence:06}_{actual_sequence:06}",
        event.session_id
    );
    event.sequence = expected_sequence;
    event.replay = RuntimeEventReplayMetadata {
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
    validate_runtime_collaboration_event_envelope(&event)
        .expect("runtime collaboration replay gap event should validate");
    event
}

fn collaboration_event_causal(
    sequence: u64,
) -> skenion_contracts::RuntimeCollaborationCausalMetadata {
    skenion_contracts::RuntimeCollaborationCausalMetadata {
        base_revision: sequence.to_string(),
        base_sequence: sequence,
        vector: BTreeMap::from([("runtime".to_owned(), sequence)]),
        observed_operation_ids: None,
    }
}

#[cfg(test)]
mod tests {
    use skenion_contracts::{
        GraphTargetRef, PatchPath, RuntimeCollaborationCursor, RuntimeCollaborationPresence,
        RuntimeCollaborationPresenceEnvelope, RuntimeCollaborationPresenceState,
        RuntimeCollaborationSelection, RuntimeCollaborationSelectionEnvelope,
        RuntimeCollaborationSelectionRange, RuntimeEventReplayGapReason,
        validate_runtime_collaboration_event_envelope,
    };

    use super::*;
    use crate::{Edge, PortRef};

    fn edge(from_node: &str, to_node: &str) -> Edge {
        Edge {
            from: PortRef {
                node: from_node.to_owned(),
                port: "out".to_owned(),
            },
            to: PortRef {
                node: to_node.to_owned(),
                port: "in".to_owned(),
            },
        }
    }

    fn presence(participant_id: &str) -> RuntimeCollaborationPresenceEnvelope {
        RuntimeCollaborationPresenceEnvelope {
            schema: "skenion.runtime.collaboration.presence".to_owned(),
            schema_version: "0.1.0".to_owned(),
            session_id: "default".to_owned(),
            participant_id: participant_id.to_owned(),
            presence: RuntimeCollaborationPresence {
                state: RuntimeCollaborationPresenceState::Active,
                display_name: Some("A".to_owned()),
                color: None,
                status_text: None,
                capabilities: None,
                connection_id: None,
                client_window_id: None,
            },
            auth_subject: None,
            updated_at: "2026-06-22T00:00:00.000Z".to_owned(),
            expires_at: "2026-06-22T00:05:00.000Z".to_owned(),
        }
    }

    fn selection(participant_id: &str) -> RuntimeCollaborationSelectionEnvelope {
        RuntimeCollaborationSelectionEnvelope {
            schema: "skenion.runtime.collaboration.selection".to_owned(),
            schema_version: "0.1.0".to_owned(),
            session_id: "default".to_owned(),
            participant_id: participant_id.to_owned(),
            target: GraphTargetRef {
                path: PatchPath::Root,
                base_revision: "1".to_owned(),
                target_revision: None,
            },
            selection: RuntimeCollaborationSelection {
                ranges: vec![RuntimeCollaborationSelectionRange::Nodes {
                    node_ids: vec!["value_1".to_owned()],
                }],
                active_range_index: Some(0),
            },
            cursor: Some(RuntimeCollaborationCursor::Canvas {
                x: 1.0,
                y: 2.0,
                client_window_id: None,
            }),
            updated_at: "2026-06-22T00:00:01.000Z".to_owned(),
            expires_at: "2026-06-22T00:05:01.000Z".to_owned(),
        }
    }

    #[test]
    fn edge_id_map_forgets_direct_and_incident_edges() {
        let log = RuntimeCollaborationLog::new(4);
        log.remember_edge_id("edge-a".to_owned(), edge("source", "target"));
        log.remember_edge_id("edge-b".to_owned(), edge("other", "target"));

        assert!(log.edge_by_id("edge-a").is_some());
        log.forget_edge_id("edge-a");
        assert!(log.edge_by_id("edge-a").is_none());
        assert!(log.edge_by_id("edge-b").is_some());

        log.forget_incident_edge_ids("target");
        assert!(log.edge_by_id("edge-b").is_none());
    }

    #[test]
    fn presence_selection_replay_gap_and_sse_helpers_are_contract_shaped() {
        let log = RuntimeCollaborationLog::new(2);
        let first = log.publish_presence(1, presence("participant-a"));
        let second = log.publish_selection(2, selection("participant-a"));
        let third = log.publish_presence(3, presence("participant-b"));

        validate_runtime_collaboration_event_envelope(&first).expect("presence event validates");
        validate_runtime_collaboration_event_envelope(&second).expect("selection event validates");
        validate_runtime_collaboration_event_envelope(&third).expect("presence event validates");

        let snapshot = log.capture_replay(None);
        assert_eq!(snapshot.high_water_sequence, 3);
        assert_eq!(snapshot.events.len(), 2);
        assert_eq!(snapshot.events[0].sequence, 2);

        let replay = log.capture_replay(Some(0));
        assert_eq!(replay.events.len(), 3);
        let gap = replay.events[0]
            .replay
            .gap
            .as_ref()
            .expect("retention gap should be inserted");
        assert_eq!(gap.reason, RuntimeEventReplayGapReason::RetentionOverflow);
        assert_eq!(gap.expected_sequence, 1);
        assert_eq!(gap.actual_sequence, 2);
        assert!(replay.events[1].replay.replayed);

        assert!(collaboration_event(third.clone()).is_ok());
        assert!(
            collaboration_broadcast_event_after_high_water(Ok(second.clone()), &log, "default", 2)
                .is_none()
        );
        assert!(
            collaboration_broadcast_event_after_high_water(Ok(third), &log, "default", 2).is_some()
        );
        assert!(
            collaboration_broadcast_event_after_high_water(
                Err(BroadcastStreamRecvError::Lagged(1)),
                &log,
                "default",
                2
            )
            .is_some()
        );
        let gap_event = log
            .stream_lag_gap_event("default", 1)
            .expect("lag should produce a replay gap");
        assert_eq!(
            gap_event.replay.gap.as_ref().map(|gap| gap.reason.clone()),
            Some(RuntimeEventReplayGapReason::StreamReset)
        );
    }

    #[test]
    fn replay_gap_after_nonzero_cursor_preserves_previous_cursor() {
        let log = RuntimeCollaborationLog::new(1);
        log.publish_presence(1, presence("participant-a"));
        log.publish_presence(2, presence("participant-b"));
        log.publish_selection(3, selection("participant-b"));

        let replay = log.capture_replay(Some(1));
        let gap = &replay.events[0];
        assert_eq!(gap.sequence, 2);
        assert_eq!(gap.replay.previous_cursor.as_deref(), Some("1"));
        assert!(gap.replay.overflow);
    }
}
