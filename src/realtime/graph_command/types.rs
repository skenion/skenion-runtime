use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use skenion_contracts::InterfaceIncidentEdgePolicyV01;

use super::super::protocol::GraphCommandKind;
use super::super::wire::{RuntimeRealtimeConnectionIdentity, RuntimeRealtimeEnvelope};
use crate::{
    CanvasNodeView, GraphTargetRef, PasteGraphFragmentRequest, PasteGraphFragmentResponse,
    RuntimeCollaborationChange, RuntimePatchResponse, RuntimeSessionRecord, RuntimeViewPatch,
};

#[derive(Clone, Copy)]
pub(in crate::realtime) struct RealtimeEventPosition<'a> {
    pub(in crate::realtime) sequence: u64,
    pub(in crate::realtime) cursor: &'a str,
}

pub(in crate::realtime) struct GraphEventContext<'a> {
    pub(super) record: &'a RuntimeSessionRecord,
    pub(super) identity: &'a RuntimeRealtimeConnectionIdentity,
    pub(super) frame: &'a RuntimeRealtimeEnvelope,
    pub(super) command: &'a GraphCommandPayload,
    pub(super) response: &'a RuntimePatchResponse,
    pub(super) node_result: Option<&'a Value>,
    pub(super) operation_result: Option<&'a PasteGraphFragmentResponse>,
    pub(super) position: RealtimeEventPosition<'a>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::realtime) struct GraphCommandPayload {
    pub(in crate::realtime) kind: String,
    #[serde(default)]
    pub(in crate::realtime) base_session_revision: Option<u64>,
    #[serde(default)]
    pub(in crate::realtime) base_graph_revision: Option<String>,
    #[serde(default)]
    pub(in crate::realtime) base_view_revision: Option<u64>,
    #[serde(default)]
    pub(in crate::realtime) target: Option<GraphTargetRef>,
    #[serde(default)]
    pub(in crate::realtime) view_patch: Option<RuntimeViewPatch>,
    #[serde(default)]
    pub(in crate::realtime) changes: Option<Vec<RuntimeCollaborationChange>>,
    #[serde(default)]
    #[serde(rename = "objectSpec")]
    pub(in crate::realtime) object_spec: Option<String>,
    #[serde(default)]
    pub(in crate::realtime) node_id: Option<String>,
    #[serde(default)]
    pub(in crate::realtime) requested_node_id: Option<String>,
    #[serde(default)]
    pub(in crate::realtime) view: Option<CanvasNodeView>,
    #[serde(default)]
    pub(in crate::realtime) params: Option<Map<String, Value>>,
    #[serde(default)]
    pub(in crate::realtime) request: Option<PasteGraphFragmentRequest>,
    #[serde(default)]
    pub(in crate::realtime) scope: Option<HistoryCommandScope>,
    #[serde(default)]
    pub(in crate::realtime) unresolved_policy: Option<ObjectUnresolvedPolicy>,
    #[serde(default)]
    pub(in crate::realtime) interface_incident_edge_policy: Option<InterfaceIncidentEdgePolicyV01>,
    #[serde(default)]
    pub(in crate::realtime) surface_path: Option<Value>,
    #[serde(default)]
    pub(in crate::realtime) description: Option<String>,
}

impl GraphCommandPayload {
    pub(super) fn command_kind(&self) -> Option<GraphCommandKind> {
        GraphCommandKind::parse(&self.kind)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(in crate::realtime) enum HistoryCommandScope {
    Client,
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::realtime) enum ObjectUnresolvedPolicy {
    Reject,
    MaterializeIssue,
}
