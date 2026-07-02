use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use super::graph_command::handle_graph_command;
use super::node_catalog::{NodeCatalogHelloRequest, handle_node_catalog_request};
use super::node_input::handle_node_input;
use super::presence::handle_selection_update;
use super::protocol::*;
use super::state::sync_required_issue;
use super::wire::{
    RuntimeRealtimeConnectionIdentity, RuntimeRealtimeEnvelope, RuntimeRealtimeIssue,
};
use super::{
    current_snapshot, empty_correlation_frame, runtime_issue, session_attached,
    session_sync_required,
};
use crate::RuntimeSessionRecord;

#[derive(Debug, Default)]
pub(in crate::realtime) struct RealtimeDispatch {
    pub(in crate::realtime) direct_frames: Vec<RuntimeRealtimeEnvelope>,
    pub(in crate::realtime) broadcast_events: Vec<RuntimeRealtimeEnvelope>,
}

impl RealtimeDispatch {
    pub(in crate::realtime) fn direct(frame: RuntimeRealtimeEnvelope) -> Self {
        Self {
            direct_frames: vec![frame],
            broadcast_events: Vec::new(),
        }
    }

    pub(in crate::realtime) fn command(
        ack: RuntimeRealtimeEnvelope,
        sender_events: Vec<RuntimeRealtimeEnvelope>,
        broadcast_events: Vec<RuntimeRealtimeEnvelope>,
    ) -> Self {
        let mut direct_frames = vec![ack];
        direct_frames.extend(sender_events);
        Self {
            direct_frames,
            broadcast_events,
        }
    }
}

pub(super) struct RuntimeRealtimeSessionEngine {
    record: RuntimeSessionRecord,
    identity: Option<RuntimeRealtimeConnectionIdentity>,
    high_water_sequence: u64,
}

impl RuntimeRealtimeSessionEngine {
    pub(super) fn new(record: RuntimeSessionRecord) -> Self {
        Self {
            record,
            identity: None,
            high_water_sequence: 0,
        }
    }

    pub(super) fn subscribe(&self) -> broadcast::Receiver<RuntimeRealtimeEnvelope> {
        self.record.realtime.subscribe()
    }

    pub(super) fn handle_text_frame(&mut self, text: &str) -> RealtimeDispatch {
        let value = match serde_json::from_str::<Value>(text) {
            Ok(value) => value,
            Err(error) => {
                return RealtimeDispatch::direct(runtime_issue(
                    &self.record.id,
                    None,
                    None,
                    "realtime.frame.invalid-json",
                    format!("invalid realtime JSON frame: {error}"),
                    None,
                ));
            }
        };
        let legacy_hello_envelope_fields = legacy_hello_envelope_fields(&value);
        let frame = match serde_json::from_value::<RuntimeRealtimeEnvelope>(value) {
            Ok(frame) => frame,
            Err(error) => {
                return RealtimeDispatch::direct(runtime_issue(
                    &self.record.id,
                    None,
                    None,
                    "realtime.frame.invalid-json",
                    format!("invalid realtime JSON frame: {error}"),
                    None,
                ));
            }
        };
        self.handle_client_frame(frame, legacy_hello_envelope_fields)
    }

    fn handle_client_frame(
        &mut self,
        frame: RuntimeRealtimeEnvelope,
        legacy_hello_envelope_fields: Vec<&'static str>,
    ) -> RealtimeDispatch {
        if frame.session_id != self.record.id {
            let actual_session_id = frame.session_id.clone();
            return RealtimeDispatch::direct(runtime_issue(
                &self.record.id,
                self.identity.as_ref(),
                Some(&frame),
                "realtime.session.mismatch",
                "frame sessionId does not match the WebSocket session",
                Some(json!({
                    "expectedSessionId": self.record.id,
                    "actualSessionId": actual_session_id,
                })),
            ));
        }

        match frame.message_type.as_str() {
            FRAME_SESSION_HELLO => self.handle_session_hello(frame, legacy_hello_envelope_fields),
            FRAME_SELECTION_UPDATE => self.handle_attached_frame(frame, handle_selection_update),
            FRAME_GRAPH_COMMAND => self.handle_attached_frame(frame, handle_graph_command),
            FRAME_NODE_INPUT => self.handle_attached_frame(frame, handle_node_input),
            FRAME_NODE_CATALOG_REQUEST => {
                self.handle_attached_frame(frame, handle_node_catalog_request)
            }
            _ => {
                let unsupported_type = frame.message_type.clone();
                RealtimeDispatch::direct(runtime_issue(
                    &self.record.id,
                    self.identity.as_ref(),
                    Some(&frame),
                    "realtime.frame.unsupported-type",
                    "unsupported Runtime realtime frame type",
                    Some(json!({ "type": unsupported_type })),
                ))
            }
        }
    }

    fn handle_session_hello(
        &mut self,
        frame: RuntimeRealtimeEnvelope,
        legacy_envelope_fields: Vec<&'static str>,
    ) -> RealtimeDispatch {
        let legacy_payload_fields = legacy_hello_payload_fields(&frame.payload);
        if !legacy_envelope_fields.is_empty() || !legacy_payload_fields.is_empty() {
            return RealtimeDispatch::direct(runtime_issue(
                &self.record.id,
                None,
                Some(&frame),
                "realtime.session.hello-legacy-identity",
                "session.hello identity hints are no longer supported",
                Some(json!({
                    "envelopeFields": legacy_envelope_fields,
                    "payloadFields": legacy_payload_fields,
                    "supportedPayloadFields": ["lastCursor", "resumeToken", "nodeCatalog"],
                })),
            ));
        }

        let snapshot = current_snapshot(&self.record);
        let hello = decode_hello_payload(&frame);
        let resumed_identity = match hello.resume_token.as_deref() {
            Some(resume_token) => match self.record.realtime.consume_resume_token(resume_token) {
                Ok(identity) => Some(identity),
                Err(issue) => {
                    let issued_identity = self.record.realtime.issue_connection_identity(None);
                    self.high_water_sequence = self.record.realtime.current_sequence();
                    let sync = session_sync_required(
                        &self.record,
                        &issued_identity,
                        &frame,
                        &snapshot,
                        issue,
                        hello.node_catalog.as_ref(),
                    );
                    self.identity = Some(issued_identity);
                    return RealtimeDispatch::direct(sync);
                }
            },
            None => None,
        };
        let issued_identity = self
            .record
            .realtime
            .issue_connection_identity(resumed_identity);

        let mut direct_frames = Vec::new();
        match hello.last_cursor.as_deref() {
            Some(last_cursor) => match self.record.realtime.replay_after(last_cursor) {
                Ok(replay) => {
                    self.high_water_sequence = replay.high_water_sequence;
                    direct_frames.push(session_attached(
                        &self.record,
                        &issued_identity,
                        &frame,
                        &snapshot,
                        hello.node_catalog.as_ref(),
                    ));
                    direct_frames.extend(replay.events);
                }
                Err(issue) => {
                    self.high_water_sequence = self.record.realtime.current_sequence();
                    direct_frames.push(session_sync_required(
                        &self.record,
                        &issued_identity,
                        &frame,
                        &snapshot,
                        issue,
                        hello.node_catalog.as_ref(),
                    ));
                }
            },
            None => {
                self.high_water_sequence = self.record.realtime.current_sequence();
                direct_frames.push(session_attached(
                    &self.record,
                    &issued_identity,
                    &frame,
                    &snapshot,
                    hello.node_catalog.as_ref(),
                ));
            }
        }
        self.identity = Some(issued_identity);
        RealtimeDispatch {
            direct_frames,
            broadcast_events: Vec::new(),
        }
    }

    fn handle_attached_frame(
        &mut self,
        frame: RuntimeRealtimeEnvelope,
        handler: impl FnOnce(
            &RuntimeSessionRecord,
            &RuntimeRealtimeConnectionIdentity,
            RuntimeRealtimeEnvelope,
        ) -> Result<RealtimeDispatch, RuntimeRealtimeIssue>,
    ) -> RealtimeDispatch {
        let Some(identity) = self.identity.clone() else {
            return RealtimeDispatch::direct(self.not_attached_issue(&frame));
        };
        match handler(&self.record, &identity, frame) {
            Ok(dispatch) => self.filter_direct_replayed_events(dispatch),
            Err(issue) => RealtimeDispatch::direct(self.issue_for_identity(&identity, issue)),
        }
    }

    fn filter_direct_replayed_events(&mut self, dispatch: RealtimeDispatch) -> RealtimeDispatch {
        let direct_frames = dispatch
            .direct_frames
            .into_iter()
            .filter(|frame| {
                if !frame
                    .payload
                    .get("replayed")
                    .and_then(|replayed| replayed.as_bool())
                    .unwrap_or(false)
                {
                    return true;
                }
                let Some(sequence) = frame.sequence else {
                    return true;
                };
                if sequence <= self.high_water_sequence {
                    return false;
                }
                self.high_water_sequence = sequence;
                true
            })
            .collect();
        RealtimeDispatch {
            direct_frames,
            broadcast_events: dispatch.broadcast_events,
        }
    }

    pub(super) fn handle_broadcast_event(
        &mut self,
        event: RuntimeRealtimeEnvelope,
    ) -> Option<RuntimeRealtimeEnvelope> {
        let sequence = event.sequence.unwrap_or_default();
        if sequence <= self.high_water_sequence {
            return None;
        }
        self.high_water_sequence = sequence;
        Some(event)
    }

    pub(super) fn handle_lagged_receiver(&self) -> Option<RuntimeRealtimeEnvelope> {
        let identity = self.identity.as_ref()?;
        let snapshot = current_snapshot(&self.record);
        let issue = sync_required_issue(
            "realtime.cursor.stream-lagged",
            "WebSocket receiver lagged beyond the Runtime realtime event window",
            Some(json!({ "currentCursor": self.record.realtime.current_cursor() })),
        );
        Some(session_sync_required(
            &self.record,
            identity,
            &empty_correlation_frame(&self.record.id),
            &snapshot,
            issue,
            None,
        ))
    }

    pub(super) fn binary_unsupported_issue(&self) -> RuntimeRealtimeEnvelope {
        runtime_issue(
            &self.record.id,
            self.identity.as_ref(),
            None,
            "realtime.frame.binary-unsupported",
            "Runtime realtime frames must be JSON text",
            None,
        )
    }

    fn not_attached_issue(&self, frame: &RuntimeRealtimeEnvelope) -> RuntimeRealtimeEnvelope {
        runtime_issue(
            &self.record.id,
            None,
            Some(frame),
            "realtime.session.not-attached",
            "send session.hello before client actions",
            None,
        )
    }

    fn issue_for_identity(
        &self,
        identity: &RuntimeRealtimeConnectionIdentity,
        issue: RuntimeRealtimeIssue,
    ) -> RuntimeRealtimeEnvelope {
        runtime_issue(
            &self.record.id,
            Some(identity),
            None,
            &issue.code,
            issue.message,
            issue.details,
        )
    }

    pub(super) fn publish(&self, event: RuntimeRealtimeEnvelope) {
        self.record.realtime.publish(event);
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HelloPayload {
    pub(super) last_cursor: Option<String>,
    pub(super) resume_token: Option<String>,
    #[serde(default)]
    pub(super) node_catalog: Option<NodeCatalogHelloRequest>,
}

pub(super) fn decode_hello_payload(frame: &RuntimeRealtimeEnvelope) -> HelloPayload {
    let mut hello =
        serde_json::from_value::<HelloPayload>(frame.payload.clone()).unwrap_or_default();
    if hello.last_cursor.is_none() {
        hello.last_cursor = frame.cursor.clone();
    }
    hello
}

fn legacy_hello_envelope_fields(value: &Value) -> Vec<&'static str> {
    if value.get("type").and_then(Value::as_str) != Some(FRAME_SESSION_HELLO) {
        return Vec::new();
    }
    legacy_hello_identity_fields(value)
}

fn legacy_hello_payload_fields(payload: &Value) -> Vec<&'static str> {
    legacy_hello_identity_fields(payload)
}

fn legacy_hello_identity_fields(value: &Value) -> Vec<&'static str> {
    const LEGACY_FIELDS: [&str; 3] = ["clientId", "windowId", "hints"];
    let Some(object) = value.as_object() else {
        return Vec::new();
    };
    LEGACY_FIELDS
        .into_iter()
        .filter(|field| object.contains_key(*field))
        .collect()
}
