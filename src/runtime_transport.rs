use std::{collections::BTreeMap, error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use skenion_contracts::{
    CanvasNodeViewV01, EdgeSpecV01, EndpointBindingValueFormatV01, GraphNodeV01, GraphTargetRef,
    InterfaceIncidentEdgePolicyV01, InterfaceIssueDetailV01, PasteGraphFragmentRequest,
    ProjectDocumentV01, validate_paste_graph_fragment_request, validate_project_document_v01,
};

use crate::{RuntimeIssue, project_current::is_payload_identity_node_kind_current};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeValidationError {
    pub message: String,
}

impl RuntimeValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeValidationReport {
    errors: Vec<RuntimeValidationError>,
}

impl RuntimeValidationReport {
    fn new(errors: Vec<RuntimeValidationError>) -> Self {
        Self { errors }
    }

    pub fn errors(&self) -> &[RuntimeValidationError] {
        &self.errors
    }
}

impl fmt::Display for RuntimeValidationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            self.errors
                .iter()
                .map(|error| error.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        )
    }
}

impl Error for RuntimeValidationReport {}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeOperationAttribution {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeOperationEnvelope {
    pub schema: String,
    pub schema_version: String,
    pub id: String,
    pub kind: String,
    pub request: PasteGraphFragmentRequest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attribution: Option<RuntimeOperationAttribution>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct IdRemapResult {
    pub node_id_map: BTreeMap<String, String>,
    pub edge_id_map: BTreeMap<String, String>,
    pub omitted_edge_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeOperationIssue {
    pub severity: String,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<GraphTargetRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duplicates: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edges: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface_policy: Option<InterfaceIncidentEdgePolicyV01>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface_detail: Option<InterfaceIssueDetailV01>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct PasteGraphFragmentResponse {
    pub schema: String,
    pub schema_version: String,
    pub ok: bool,
    pub applied: bool,
    pub conflict: bool,
    pub target: GraphTargetRef,
    pub revision_before: String,
    pub revision_after: Option<String>,
    pub history_entry_id: Option<String>,
    pub id_remap: IdRemapResult,
    pub issues: Vec<RuntimeOperationIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationCausalMetadata {
    pub base_revision: String,
    pub base_sequence: u64,
    pub vector: BTreeMap<String, u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_operation_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeCollaborationAuthSubjectKind {
    Anonymous,
    User,
    Service,
    Deferred,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationAuthSubject {
    pub kind: RuntimeCollaborationAuthSubjectKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationParticipant {
    pub participant_id: String,
    pub session_id: String,
    pub joined_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_subject: Option<RuntimeCollaborationAuthSubject>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationCanvasPosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "op", rename_all_fields = "camelCase")]
pub enum RuntimeCollaborationChange {
    #[serde(rename = "node.add")]
    NodeAdd {
        change_id: String,
        node: Box<GraphNodeV01>,
        #[serde(skip_serializing_if = "Option::is_none")]
        view: Option<RuntimeCollaborationCanvasPosition>,
    },
    #[serde(rename = "node.move")]
    NodeMove {
        change_id: String,
        node_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        from: Option<RuntimeCollaborationCanvasPosition>,
        to: RuntimeCollaborationCanvasPosition,
    },
    #[serde(rename = "node.delete")]
    NodeDelete {
        change_id: String,
        node_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tombstone_id: Option<String>,
    },
    #[serde(rename = "edge.connect")]
    EdgeConnect {
        change_id: String,
        edge: Box<EdgeSpecV01>,
    },
    #[serde(rename = "edge.disconnect")]
    EdgeDisconnect { change_id: String, edge_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeCollaborationUndoRedoAction {
    Undo,
    Redo,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeCollaborationUndoScopeKind {
    Participant,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationUndoScope {
    pub kind: RuntimeCollaborationUndoScopeKind,
    pub participant_id: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RuntimeCollaborationOperationPayload {
    ChangeSet {
        target: GraphTargetRef,
        changes: Vec<RuntimeCollaborationChange>,
        #[serde(skip_serializing_if = "Option::is_none")]
        undo_group_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    PasteGraphFragment {
        request: Box<PasteGraphFragmentRequest>,
        #[serde(skip_serializing_if = "Option::is_none")]
        undo_group_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    UndoRedo {
        action: RuntimeCollaborationUndoRedoAction,
        scope: RuntimeCollaborationUndoScope,
        #[serde(skip_serializing_if = "Option::is_none")]
        subject_operation_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        undo_group_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_operations: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationOperationEnvelope {
    pub schema: String,
    pub schema_version: String,
    pub operation_id: String,
    pub session_id: String,
    pub participant_id: String,
    pub idempotency_key: String,
    pub causal: RuntimeCollaborationCausalMetadata,
    pub payload: RuntimeCollaborationOperationPayload,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_subject: Option<RuntimeCollaborationAuthSubject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    pub submitted_at: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationOperationBatch {
    pub schema: String,
    pub schema_version: String,
    pub session_id: String,
    pub operations: Vec<RuntimeCollaborationOperationEnvelope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationOperationIssue {
    pub severity: String,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub participant_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_sequence: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_sequence: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationServerClock {
    pub revision: String,
    pub sequence: u64,
    pub vector: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationAck {
    pub sequence: u64,
    pub revision: String,
    pub server_clock: RuntimeCollaborationServerClock,
    pub applied_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeCollaborationNackReason {
    BaseRevisionMismatch,
    CausalityGap,
    DuplicateIdempotencyKey,
    InvalidOperation,
    ParticipantExpired,
    UnsupportedOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationNack {
    pub reason: RuntimeCollaborationNackReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issues: Option<Vec<RuntimeCollaborationOperationIssue>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationConflict {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeCollaborationRebaseStrategy {
    OtTransform,
    CrdtMerge,
    ServerReject,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationRebase {
    pub from: RuntimeCollaborationCausalMetadata,
    pub to: RuntimeCollaborationCausalMetadata,
    pub strategy: RuntimeCollaborationRebaseStrategy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transformed_payload: Option<RuntimeCollaborationOperationPayload>,
    pub conflicts: Vec<RuntimeCollaborationConflict>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeCollaborationOperationStatus {
    Accepted,
    Duplicate,
    Rejected,
    Rebased,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationOperationResult {
    pub schema: String,
    pub schema_version: String,
    pub session_id: String,
    pub operation_id: String,
    pub participant_id: String,
    pub idempotency_key: String,
    pub status: RuntimeCollaborationOperationStatus,
    pub causal: RuntimeCollaborationCausalMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ack: Option<RuntimeCollaborationAck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nack: Option<RuntimeCollaborationNack>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rebase: Option<RuntimeCollaborationRebase>,
    pub issues: Vec<RuntimeCollaborationOperationIssue>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationOperationBatchResult {
    pub schema: String,
    pub schema_version: String,
    pub session_id: String,
    pub results: Vec<RuntimeCollaborationOperationResult>,
    pub issues: Vec<RuntimeCollaborationOperationIssue>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeCollaborationPresenceState {
    Joined,
    Active,
    Idle,
    Away,
    Left,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationPresence {
    pub state: RuntimeCollaborationPresenceState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_window_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationPresenceEnvelope {
    pub schema: String,
    pub schema_version: String,
    pub session_id: String,
    pub participant_id: String,
    pub presence: RuntimeCollaborationPresence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_subject: Option<RuntimeCollaborationAuthSubject>,
    pub updated_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationPortEndpoint {
    pub node_id: String,
    pub port_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationTextPosition {
    pub node_id: String,
    pub field: String,
    pub offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum RuntimeCollaborationSelectionRange {
    Nodes {
        node_ids: Vec<String>,
    },
    Edges {
        edge_ids: Vec<String>,
    },
    Ports {
        endpoints: Vec<RuntimeCollaborationPortEndpoint>,
    },
    Text {
        anchor: RuntimeCollaborationTextPosition,
        focus: RuntimeCollaborationTextPosition,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationSelection {
    pub ranges: Vec<RuntimeCollaborationSelectionRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_range_index: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum RuntimeCollaborationCursor {
    Canvas {
        x: f64,
        y: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        client_window_id: Option<String>,
    },
    Node {
        node_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        port_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        client_window_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationSelectionEnvelope {
    pub schema: String,
    pub schema_version: String,
    pub session_id: String,
    pub participant_id: String,
    pub target: GraphTargetRef,
    pub selection: RuntimeCollaborationSelection,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<RuntimeCollaborationCursor>,
    pub updated_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RuntimeCollaborationEventPayload {
    OperationResult {
        result: Box<RuntimeCollaborationOperationResult>,
    },
    Presence {
        presence: Box<RuntimeCollaborationPresenceEnvelope>,
    },
    Selection {
        selection: Box<RuntimeCollaborationSelectionEnvelope>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeCollaborationEventKind {
    OperationResult,
    Presence,
    Selection,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCollaborationEventEnvelope {
    pub schema: String,
    pub schema_version: String,
    pub event_id: String,
    pub session_id: String,
    pub sequence: u64,
    pub causal: RuntimeCollaborationCausalMetadata,
    pub kind: RuntimeCollaborationEventKind,
    pub payload: RuntimeCollaborationEventPayload,
    pub replay: RuntimeEventReplayMetadata,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeSessionEventKind {
    Snapshot,
    Load,
    Clear,
    Mutate,
    Undo,
    Redo,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeSessionLifecycleState {
    Initializing,
    Ready,
    Closing,
    Closed,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeConnectionProfileMode {
    LocalManaged,
    LocalShared,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeOwnershipMode {
    OwnedChild,
    External,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeEndpointProtocol {
    Http,
    Https,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEndpointMetadata {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_url: Option<String>,
    pub protocol: RuntimeEndpointProtocol,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeProcessMetadata {
    pub owned_by_host: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_window_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeConnectionProfile {
    pub mode: RuntimeConnectionProfileMode,
    pub ownership: RuntimeOwnershipMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub endpoint: RuntimeEndpointMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process: Option<RuntimeProcessMetadata>,
}

pub type RuntimeTransportProjectSnapshot = ProjectDocumentV01;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTransportSessionSnapshot {
    pub session_revision: u64,
    pub view_revision: u64,
    pub control_revision: u64,
    pub project: Option<RuntimeTransportProjectSnapshot>,
    pub binding_formats: Vec<EndpointBindingValueFormatV01>,
    pub issues: Vec<RuntimeIssue>,
    pub plan: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTransportMutationRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<RuntimeOperationEnvelope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_patch: Option<RuntimeTransportViewPatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RuntimeTransportViewPatch {
    pub base_view_revision: u64,
    pub ops: Vec<RuntimeTransportViewPatchOperation>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, tag = "op")]
pub enum RuntimeTransportViewPatchOperation {
    #[serde(rename = "setNodeView")]
    SetNodeView {
        #[serde(rename = "nodeId")]
        node_id: String,
        view: CanvasNodeViewV01,
    },
    #[serde(rename = "moveNodeView")]
    MoveNodeView {
        #[serde(rename = "nodeId")]
        node_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        from: Option<CanvasNodeViewV01>,
        to: CanvasNodeViewV01,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeTransportHistoryEntryKind {
    Apply,
    Undo,
    Redo,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTransportHistoryEntry {
    pub id: String,
    pub sequence: u64,
    pub kind: RuntimeTransportHistoryEntryKind,
    pub mutation: RuntimeTransportMutationRequest,
    pub inverse_mutation: RuntimeTransportMutationRequest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_event_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTransportHistory {
    pub schema: String,
    pub schema_version: String,
    pub entries: Vec<RuntimeTransportHistoryEntry>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub undo_depth: u64,
    pub redo_depth: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEventReplayWindow {
    pub cursor_kind: String,
    pub current_cursor: String,
    pub earliest_sequence: u64,
    pub latest_sequence: u64,
    pub replay_limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overflow: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSessionCapabilitySet {
    pub session_addressing: bool,
    pub event_replay: bool,
    pub multi_window: bool,
    pub auth_policy: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSessionInfoResponse {
    pub schema: String,
    pub schema_version: String,
    pub ok: bool,
    pub session_id: String,
    pub lifecycle: RuntimeSessionLifecycleState,
    pub snapshot: RuntimeTransportSessionSnapshot,
    pub profile: RuntimeConnectionProfile,
    pub capabilities: RuntimeSessionCapabilitySet,
    pub event_replay: RuntimeEventReplayWindow,
    pub issues: Vec<RuntimeIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeEventReplayGapReason {
    RetentionOverflow,
    StreamReset,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEventReplayGap {
    pub expected_sequence: u64,
    pub actual_sequence: u64,
    pub reason: RuntimeEventReplayGapReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEventReplayMetadata {
    pub cursor: String,
    pub previous_cursor: Option<String>,
    pub replayed: bool,
    pub gap: Option<RuntimeEventReplayGap>,
    pub overflow: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSessionEvent {
    pub schema: String,
    pub schema_version: String,
    pub id: String,
    pub session_id: String,
    pub sequence: u64,
    pub session_revision: u64,
    pub kind: RuntimeSessionEventKind,
    pub snapshot: RuntimeTransportSessionSnapshot,
    pub history: RuntimeTransportHistory,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mutation: Option<RuntimeTransportHistoryEntry>,
    pub replay: RuntimeEventReplayMetadata,
    pub issues: Vec<RuntimeIssue>,
    pub created_at: String,
}

pub fn validate_runtime_operation_envelope(
    envelope: &RuntimeOperationEnvelope,
) -> Result<(), RuntimeValidationReport> {
    let mut errors = Vec::new();
    if envelope.schema != "skenion.runtime.operation" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schema skenion.runtime.operation, found {}",
            envelope.schema
        )));
    }
    if envelope.schema_version != "0.1.0" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schemaVersion 0.1.0, found {}",
            envelope.schema_version
        )));
    }
    if envelope.id.is_empty() {
        errors.push(RuntimeValidationError::new(
            "runtime operation id must not be empty",
        ));
    }
    if envelope.kind != "pasteGraphFragment" {
        errors.push(RuntimeValidationError::new(format!(
            "unsupported runtime operation kind: {}",
            envelope.kind
        )));
    }
    if !paste_request_contains_payload_identity(&envelope.request)
        && let Err(report) = validate_paste_graph_fragment_request(&envelope.request)
    {
        errors.extend(runtime_errors_from_contract_report(report.to_string()));
    }

    finish_validation(errors)
}

pub fn validate_paste_graph_fragment_response(
    response: &PasteGraphFragmentResponse,
) -> Result<(), RuntimeValidationReport> {
    let mut errors = Vec::new();
    if response.schema != "skenion.runtime.paste-graph-fragment.response" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schema skenion.runtime.paste-graph-fragment.response, found {}",
            response.schema
        )));
    }
    if response.schema_version != "0.1.0" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schemaVersion 0.1.0, found {}",
            response.schema_version
        )));
    }
    if response.applied && !response.ok {
        errors.push(RuntimeValidationError::new(
            "paste response cannot be applied when ok is false",
        ));
    }
    if response.applied && response.revision_after.is_none() {
        errors.push(RuntimeValidationError::new(
            "applied paste response must include revisionAfter",
        ));
    }
    for issue in &response.issues {
        if matches!(
            issue.code.as_str(),
            "interface-drift" | "invalid-incident-edge"
        ) && issue.interface_detail.is_none()
        {
            errors.push(RuntimeValidationError::new(format!(
                "runtime operation issue {} requires interfaceDetail",
                issue.code
            )));
        }
        if let Some(detail) = &issue.interface_detail
            && detail.recovery_actions.is_empty()
        {
            errors.push(RuntimeValidationError::new(format!(
                "runtime operation issue {} interfaceDetail requires recoveryActions",
                issue.code
            )));
        }
    }

    finish_validation(errors)
}

pub fn validate_runtime_collaboration_operation_envelope(
    envelope: &RuntimeCollaborationOperationEnvelope,
) -> Result<(), RuntimeValidationReport> {
    let mut errors = Vec::new();
    if envelope.schema != "skenion.runtime.collaboration.operation" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schema skenion.runtime.collaboration.operation, found {}",
            envelope.schema
        )));
    }
    if envelope.schema_version != "0.1.0" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schemaVersion 0.1.0, found {}",
            envelope.schema_version
        )));
    }
    errors.extend(validate_runtime_collaboration_operation_envelope_semantics(
        envelope,
    ));

    finish_validation(errors)
}

pub fn validate_runtime_collaboration_operation_batch(
    batch: &RuntimeCollaborationOperationBatch,
) -> Result<(), RuntimeValidationReport> {
    let mut errors = Vec::new();
    if batch.schema != "skenion.runtime.collaboration.operation-batch" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schema skenion.runtime.collaboration.operation-batch, found {}",
            batch.schema
        )));
    }
    if batch.schema_version != "0.1.0" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schemaVersion 0.1.0, found {}",
            batch.schema_version
        )));
    }
    errors.extend(duplicate_errors(
        batch
            .operations
            .iter()
            .map(|operation| operation.idempotency_key.as_str())
            .collect(),
        "collaboration idempotency key",
    ));
    for operation in &batch.operations {
        if operation.session_id != batch.session_id {
            errors.push(RuntimeValidationError::new(
                "collaboration batch operation sessionId must match batch sessionId",
            ));
        }
        errors.extend(validate_runtime_collaboration_operation_envelope_semantics(
            operation,
        ));
    }

    finish_validation(errors)
}

pub fn validate_runtime_collaboration_operation_result(
    result: &RuntimeCollaborationOperationResult,
) -> Result<(), RuntimeValidationReport> {
    let mut errors = Vec::new();
    if result.schema != "skenion.runtime.collaboration.operation-result" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schema skenion.runtime.collaboration.operation-result, found {}",
            result.schema
        )));
    }
    if result.schema_version != "0.1.0" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schemaVersion 0.1.0, found {}",
            result.schema_version
        )));
    }

    errors.extend(validate_runtime_collaboration_causality(
        &result.causal,
        "operation result causal",
    ));

    let has_ack = result.ack.is_some();
    let has_nack = result.nack.is_some();
    let has_rebase = result.rebase.is_some();

    let status_requires_ack = matches!(
        result.status,
        RuntimeCollaborationOperationStatus::Accepted
            | RuntimeCollaborationOperationStatus::Rebased
    );
    if status_requires_ack && !has_ack {
        errors.push(RuntimeValidationError::new(
            "accepted or rebased collaboration result must include ack",
        ));
    }
    if result.status == RuntimeCollaborationOperationStatus::Accepted && has_nack {
        errors.push(RuntimeValidationError::new(
            "accepted collaboration result must not include nack or rebase",
        ));
    }
    if result.status == RuntimeCollaborationOperationStatus::Accepted && has_rebase {
        errors.push(RuntimeValidationError::new(
            "accepted collaboration result must not include nack or rebase",
        ));
    }

    let status_requires_nack = matches!(
        result.status,
        RuntimeCollaborationOperationStatus::Duplicate
            | RuntimeCollaborationOperationStatus::Rejected
    );
    if status_requires_nack && !has_nack {
        errors.push(RuntimeValidationError::new(
            "duplicate or rejected collaboration result must include nack",
        ));
    }
    let has_duplicate_idempotency_nack = match result.nack.as_ref() {
        Some(nack) => nack.reason == RuntimeCollaborationNackReason::DuplicateIdempotencyKey,
        None => false,
    };
    if result.status == RuntimeCollaborationOperationStatus::Duplicate
        && !has_duplicate_idempotency_nack
    {
        errors.push(RuntimeValidationError::new(
            "duplicate collaboration result nack reason must be duplicate-idempotency-key",
        ));
    }
    if result.status == RuntimeCollaborationOperationStatus::Rebased && !has_rebase {
        errors.push(RuntimeValidationError::new(
            "rebased collaboration result must include rebase metadata",
        ));
    }
    if let Some(rebase) = &result.rebase {
        errors.extend(validate_runtime_collaboration_causality(
            &rebase.from,
            "rebase from causal",
        ));
        errors.extend(validate_runtime_collaboration_causality(
            &rebase.to,
            "rebase to causal",
        ));
    }

    finish_validation(errors)
}

pub fn validate_runtime_collaboration_operation_batch_result(
    result: &RuntimeCollaborationOperationBatchResult,
) -> Result<(), RuntimeValidationReport> {
    let mut errors = Vec::new();
    if result.schema != "skenion.runtime.collaboration.operation-batch-result" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schema skenion.runtime.collaboration.operation-batch-result, found {}",
            result.schema
        )));
    }
    if result.schema_version != "0.1.0" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schemaVersion 0.1.0, found {}",
            result.schema_version
        )));
    }
    if result.results.is_empty() {
        errors.push(RuntimeValidationError::new(
            "collaboration batch result must include at least one operation result",
        ));
    }
    errors.extend(duplicate_errors(
        result
            .results
            .iter()
            .map(|operation_result| operation_result.idempotency_key.as_str())
            .collect(),
        "collaboration batch result idempotency key",
    ));
    for operation_result in &result.results {
        if operation_result.session_id != result.session_id {
            errors.push(RuntimeValidationError::new(
                "collaboration batch result operation sessionId must match batch result sessionId",
            ));
        }
        if let Err(report) = validate_runtime_collaboration_operation_result(operation_result) {
            errors.extend(report.errors().iter().cloned());
        }
    }

    finish_validation(errors)
}

pub fn validate_runtime_collaboration_presence_envelope(
    presence: &RuntimeCollaborationPresenceEnvelope,
) -> Result<(), RuntimeValidationReport> {
    let mut errors = Vec::new();
    if presence.schema != "skenion.runtime.collaboration.presence" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schema skenion.runtime.collaboration.presence, found {}",
            presence.schema
        )));
    }
    if presence.schema_version != "0.1.0" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schemaVersion 0.1.0, found {}",
            presence.schema_version
        )));
    }
    errors.extend(validate_runtime_collaboration_auth_separation(
        &presence.participant_id,
        presence.auth_subject.as_ref(),
        "presence",
    ));
    errors.extend(validate_runtime_collaboration_expiry(
        &presence.updated_at,
        &presence.expires_at,
        "presence",
    ));

    finish_validation(errors)
}

pub fn validate_runtime_collaboration_selection_envelope(
    selection: &RuntimeCollaborationSelectionEnvelope,
) -> Result<(), RuntimeValidationReport> {
    let mut errors = Vec::new();
    if selection.schema != "skenion.runtime.collaboration.selection" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schema skenion.runtime.collaboration.selection, found {}",
            selection.schema
        )));
    }
    if selection.schema_version != "0.1.0" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schemaVersion 0.1.0, found {}",
            selection.schema_version
        )));
    }
    errors.extend(validate_runtime_collaboration_expiry(
        &selection.updated_at,
        &selection.expires_at,
        "selection",
    ));

    finish_validation(errors)
}

pub fn validate_runtime_collaboration_event_envelope(
    event: &RuntimeCollaborationEventEnvelope,
) -> Result<(), RuntimeValidationReport> {
    let mut errors = Vec::new();
    if event.schema != "skenion.runtime.collaboration.event" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schema skenion.runtime.collaboration.event, found {}",
            event.schema
        )));
    }
    if event.schema_version != "0.1.0" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schemaVersion 0.1.0, found {}",
            event.schema_version
        )));
    }
    errors.extend(validate_runtime_collaboration_causality(
        &event.causal,
        "collaboration event causal",
    ));
    if event.kind != runtime_collaboration_event_payload_kind(&event.payload) {
        errors.push(RuntimeValidationError::new(
            "collaboration event kind must match payload kind",
        ));
    }
    match &event.replay.gap {
        Some(gap) if gap.expected_sequence >= gap.actual_sequence => {
            errors.push(RuntimeValidationError::new(
                "collaboration event replay gap expectedSequence must be less than actualSequence",
            ));
        }
        _ => {}
    }

    finish_validation(errors)
}

pub fn validate_runtime_session_info_response(
    response: &RuntimeSessionInfoResponse,
) -> Result<(), RuntimeValidationReport> {
    let mut errors = Vec::new();
    if response.schema != "skenion.runtime.session.info" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schema skenion.runtime.session.info, found {}",
            response.schema
        )));
    }
    if response.schema_version != "0.1.0" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schemaVersion 0.1.0, found {}",
            response.schema_version
        )));
    }
    if response.session_id.is_empty() {
        errors.push(RuntimeValidationError::new("sessionId must not be empty"));
    }
    errors.extend(runtime_session_snapshot_errors(&response.snapshot));
    errors.extend(runtime_issue_errors("session info", &response.issues));
    errors.extend(runtime_profile_errors(&response.profile));
    if response.capabilities.auth_policy != "deferred" {
        errors.push(RuntimeValidationError::new(
            "runtime session authPolicy must be deferred",
        ));
    }
    if response.event_replay.cursor_kind != "sequence" {
        errors.push(RuntimeValidationError::new(
            "runtime eventReplay cursorKind must be sequence",
        ));
    }
    if response.event_replay.current_cursor.is_empty() {
        errors.push(RuntimeValidationError::new(
            "runtime eventReplay currentCursor must not be empty",
        ));
    }
    if response.event_replay.earliest_sequence == 0 {
        errors.push(RuntimeValidationError::new(
            "runtime eventReplay earliestSequence must be at least 1",
        ));
    }
    if matches!(
        (&response.profile.mode, &response.profile.ownership),
        (
            RuntimeConnectionProfileMode::LocalManaged,
            RuntimeOwnershipMode::OwnedChild
        ) | (
            RuntimeConnectionProfileMode::LocalShared,
            RuntimeOwnershipMode::External
        ) | (
            RuntimeConnectionProfileMode::Remote,
            RuntimeOwnershipMode::Remote
        )
    ) {
    } else {
        errors.push(RuntimeValidationError::new(
            "runtime profile ownership must match local-managed, local-shared, or remote mode",
        ));
    }

    finish_validation(errors)
}

pub fn validate_runtime_session_event(
    event: &RuntimeSessionEvent,
) -> Result<(), RuntimeValidationReport> {
    let mut errors = Vec::new();
    if event.schema != "skenion.runtime.session.event" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schema skenion.runtime.session.event, found {}",
            event.schema
        )));
    }
    if event.schema_version != "0.1.0" {
        errors.push(RuntimeValidationError::new(format!(
            "expected schemaVersion 0.1.0, found {}",
            event.schema_version
        )));
    }
    if event.session_id.is_empty() {
        errors.push(RuntimeValidationError::new("sessionId must not be empty"));
    }
    if event.id.is_empty() {
        errors.push(RuntimeValidationError::new("event id must not be empty"));
    }
    if event.sequence == 0 {
        errors.push(RuntimeValidationError::new("sequence must be at least 1"));
    }
    if event.created_at.is_empty() {
        errors.push(RuntimeValidationError::new("createdAt must not be empty"));
    }
    errors.extend(runtime_session_snapshot_errors(&event.snapshot));
    errors.extend(runtime_issue_errors("event", &event.issues));
    errors.extend(runtime_history_errors(&event.history));
    if let Some(mutation) = &event.mutation {
        errors.extend(runtime_history_entry_errors(mutation, "mutation"));
    }
    if event.replay.cursor.is_empty() {
        errors.push(RuntimeValidationError::new(
            "replay cursor must not be empty",
        ));
    }
    if event
        .replay
        .previous_cursor
        .as_ref()
        .is_some_and(String::is_empty)
    {
        errors.push(RuntimeValidationError::new(
            "replay previousCursor must not be empty",
        ));
    }
    if let Some(gap) = &event.replay.gap {
        if gap.expected_sequence == 0 || gap.actual_sequence == 0 {
            errors.push(RuntimeValidationError::new(
                "replay gap sequences must be at least 1",
            ));
        }
        if gap.expected_sequence >= gap.actual_sequence {
            errors.push(RuntimeValidationError::new(
                "replay gap expectedSequence must be less than actualSequence",
            ));
        }
    }
    if event.session_revision != event.snapshot.session_revision {
        errors.push(RuntimeValidationError::new(
            "event sessionRevision must match snapshot.sessionRevision",
        ));
    }

    finish_validation(errors)
}

fn validate_runtime_collaboration_causality(
    causal: &RuntimeCollaborationCausalMetadata,
    label: &str,
) -> Vec<RuntimeValidationError> {
    let max_vector = causal.vector.values().copied().max().unwrap_or(0);
    if causal.base_sequence < max_vector {
        vec![RuntimeValidationError::new(format!(
            "{label} baseSequence must be greater than or equal to the causal vector maximum"
        ))]
    } else {
        Vec::new()
    }
}

fn validate_runtime_collaboration_auth_separation(
    participant_id: &str,
    auth_subject: Option<&RuntimeCollaborationAuthSubject>,
    label: &str,
) -> Vec<RuntimeValidationError> {
    let Some(subject) = auth_subject else {
        return Vec::new();
    };
    let Some(subject_id) = subject.subject_id.as_deref() else {
        return Vec::new();
    };

    if subject_id == participant_id {
        vec![RuntimeValidationError::new(format!(
            "{label} participantId must not mirror auth subject id"
        ))]
    } else {
        Vec::new()
    }
}

fn validate_runtime_collaboration_expiry(
    updated_at: &str,
    expires_at: &str,
    label: &str,
) -> Vec<RuntimeValidationError> {
    if expires_at <= updated_at {
        vec![RuntimeValidationError::new(format!(
            "{label} expiresAt must be later than updatedAt"
        ))]
    } else {
        Vec::new()
    }
}

fn validate_runtime_collaboration_payload(
    payload: &RuntimeCollaborationOperationPayload,
    participant_id: &str,
) -> Vec<RuntimeValidationError> {
    match payload {
        RuntimeCollaborationOperationPayload::ChangeSet { changes, .. } => duplicate_errors(
            changes
                .iter()
                .map(runtime_collaboration_change_id)
                .collect(),
            "collaboration change id",
        ),
        RuntimeCollaborationOperationPayload::PasteGraphFragment { request, .. } => {
            if paste_request_contains_payload_identity(request) {
                return Vec::new();
            }
            match validate_paste_graph_fragment_request(request) {
                Ok(_) => Vec::new(),
                Err(report) => runtime_errors_from_contract_report(report.to_string()),
            }
        }
        RuntimeCollaborationOperationPayload::UndoRedo { scope, .. } => {
            if scope.participant_id != participant_id {
                vec![RuntimeValidationError::new(
                    "undoRedo scope participantId must match operation participantId",
                )]
            } else {
                Vec::new()
            }
        }
    }
}

fn validate_runtime_collaboration_operation_envelope_semantics(
    envelope: &RuntimeCollaborationOperationEnvelope,
) -> Vec<RuntimeValidationError> {
    let mut errors = Vec::new();
    errors.extend(validate_runtime_collaboration_causality(
        &envelope.causal,
        "operation causal",
    ));
    errors.extend(validate_runtime_collaboration_auth_separation(
        &envelope.participant_id,
        envelope.auth_subject.as_ref(),
        "operation",
    ));
    errors.extend(validate_runtime_collaboration_payload(
        &envelope.payload,
        &envelope.participant_id,
    ));

    if !envelope
        .causal
        .vector
        .contains_key(&envelope.participant_id)
    {
        errors.push(RuntimeValidationError::new(
            "operation causal vector must include participantId",
        ));
    }

    errors
}

fn runtime_collaboration_change_id(change: &RuntimeCollaborationChange) -> &str {
    match change {
        RuntimeCollaborationChange::NodeAdd { change_id, .. } => change_id,
        RuntimeCollaborationChange::NodeMove { change_id, .. } => change_id,
        RuntimeCollaborationChange::NodeDelete { change_id, .. } => change_id,
        RuntimeCollaborationChange::EdgeConnect { change_id, .. } => change_id,
        RuntimeCollaborationChange::EdgeDisconnect { change_id, .. } => change_id,
    }
}

fn paste_request_contains_payload_identity(request: &PasteGraphFragmentRequest) -> bool {
    request.fragment.nodes.iter().any(|node| {
        crate::current_node_identity::graph_node_object_id(node)
            .is_some_and(is_payload_identity_node_kind_current)
    })
}

fn runtime_collaboration_event_payload_kind(
    payload: &RuntimeCollaborationEventPayload,
) -> RuntimeCollaborationEventKind {
    match payload {
        RuntimeCollaborationEventPayload::OperationResult { .. } => {
            RuntimeCollaborationEventKind::OperationResult
        }
        RuntimeCollaborationEventPayload::Presence { .. } => {
            RuntimeCollaborationEventKind::Presence
        }
        RuntimeCollaborationEventPayload::Selection { .. } => {
            RuntimeCollaborationEventKind::Selection
        }
    }
}

fn runtime_profile_errors(profile: &RuntimeConnectionProfile) -> Vec<RuntimeValidationError> {
    let mut errors = Vec::new();
    if profile.endpoint.url.is_empty() {
        errors.push(RuntimeValidationError::new(
            "endpoint url must not be empty",
        ));
    }
    if profile
        .endpoint
        .canonical_url
        .as_ref()
        .is_some_and(String::is_empty)
    {
        errors.push(RuntimeValidationError::new(
            "endpoint canonicalUrl must not be empty",
        ));
    }
    if profile.endpoint.host.as_ref().is_some_and(String::is_empty) {
        errors.push(RuntimeValidationError::new(
            "endpoint host must not be empty",
        ));
    }
    if let Some(process) = &profile.process {
        if process.pid == Some(0) {
            errors.push(RuntimeValidationError::new(
                "process pid must be at least 1",
            ));
        }
        if process
            .executable_path
            .as_ref()
            .is_some_and(String::is_empty)
        {
            errors.push(RuntimeValidationError::new(
                "process executablePath must not be empty",
            ));
        }
        if process
            .working_directory
            .as_ref()
            .is_some_and(String::is_empty)
        {
            errors.push(RuntimeValidationError::new(
                "process workingDirectory must not be empty",
            ));
        }
        if process
            .owner_window_id
            .as_ref()
            .is_some_and(String::is_empty)
        {
            errors.push(RuntimeValidationError::new(
                "process ownerWindowId must not be empty",
            ));
        }
        if process.platform.as_ref().is_some_and(String::is_empty) {
            errors.push(RuntimeValidationError::new(
                "process platform must not be empty",
            ));
        }
        if process.arch.as_ref().is_some_and(String::is_empty) {
            errors.push(RuntimeValidationError::new(
                "process arch must not be empty",
            ));
        }
    }
    errors
}

fn runtime_session_snapshot_errors(
    snapshot: &RuntimeTransportSessionSnapshot,
) -> Vec<RuntimeValidationError> {
    let mut errors = Vec::new();
    errors.extend(runtime_issue_errors("snapshot", &snapshot.issues));
    if snapshot.plan.as_ref().is_some_and(|plan| !plan.is_object()) {
        errors.push(RuntimeValidationError::new(
            "snapshot plan must be an object or null",
        ));
    }
    if let Some(project) = &snapshot.project
        && let Err(report) = validate_project_document_v01(project)
    {
        errors.extend(report.errors().iter().map(|error| {
            RuntimeValidationError::new(format!("snapshot project {}", error.message))
        }));
    }
    errors
}

fn runtime_issue_errors(label: &str, issues: &[RuntimeIssue]) -> Vec<RuntimeValidationError> {
    let mut errors = Vec::new();
    if issues.iter().any(|issue| issue.message.is_empty()) {
        errors.push(RuntimeValidationError::new(format!(
            "{label} issues must include non-empty message"
        )));
    }
    errors
}

fn runtime_history_errors(history: &RuntimeTransportHistory) -> Vec<RuntimeValidationError> {
    let mut errors = Vec::new();
    if history.schema != "skenion.runtime.history" {
        errors.push(RuntimeValidationError::new(format!(
            "expected history schema skenion.runtime.history, found {}",
            history.schema
        )));
    }
    if history.schema_version != "0.1.0" {
        errors.push(RuntimeValidationError::new(format!(
            "expected history schemaVersion 0.1.0, found {}",
            history.schema_version
        )));
    }
    for entry in &history.entries {
        errors.extend(runtime_history_entry_errors(entry, "history entry"));
    }
    errors
}

fn runtime_history_entry_errors(
    entry: &RuntimeTransportHistoryEntry,
    label: &str,
) -> Vec<RuntimeValidationError> {
    let mut errors = Vec::new();
    if entry.id.is_empty() {
        errors.push(RuntimeValidationError::new(format!(
            "{label} id must not be empty"
        )));
    }
    if entry.sequence == 0 {
        errors.push(RuntimeValidationError::new(format!(
            "{label} sequence must be at least 1"
        )));
    }
    if entry.created_at.is_empty() {
        errors.push(RuntimeValidationError::new(format!(
            "{label} createdAt must not be empty"
        )));
    }
    if entry
        .subject_event_id
        .as_ref()
        .is_some_and(String::is_empty)
    {
        errors.push(RuntimeValidationError::new(format!(
            "{label} subjectEventId must not be empty"
        )));
    }
    if entry.client_id.as_ref().is_some_and(String::is_empty) {
        errors.push(RuntimeValidationError::new(format!(
            "{label} clientId must not be empty"
        )));
    }
    errors.extend(runtime_mutation_request_errors(
        &entry.mutation,
        &format!("{label} mutation"),
    ));
    errors.extend(runtime_mutation_request_errors(
        &entry.inverse_mutation,
        &format!("{label} inverseMutation"),
    ));
    errors
}

fn runtime_mutation_request_errors(
    mutation: &RuntimeTransportMutationRequest,
    label: &str,
) -> Vec<RuntimeValidationError> {
    let mut errors = Vec::new();
    if let Some(operation) = &mutation.operation
        && let Err(report) = validate_runtime_operation_envelope(operation)
    {
        errors.extend(report.errors().iter().map(|error| {
            RuntimeValidationError::new(format!("{label} operation {}", error.message))
        }));
    }
    if let Some(view_patch) = &mutation.view_patch {
        for operation in &view_patch.ops {
            match operation {
                RuntimeTransportViewPatchOperation::SetNodeView { node_id, .. }
                | RuntimeTransportViewPatchOperation::MoveNodeView { node_id, .. } => {
                    if node_id.is_empty() {
                        errors.push(RuntimeValidationError::new(format!(
                            "{label} viewPatch operation nodeId must not be empty"
                        )));
                    }
                }
            }
        }
    }
    if mutation.client_id.as_ref().is_some_and(String::is_empty) {
        errors.push(RuntimeValidationError::new(format!(
            "{label} clientId must not be empty"
        )));
    }
    errors
}

fn duplicate_errors(values: Vec<&str>, label: &str) -> Vec<RuntimeValidationError> {
    let mut seen = std::collections::HashSet::new();
    let mut errors = Vec::new();

    for value in values {
        if !seen.insert(value) {
            errors.push(RuntimeValidationError::new(format!(
                "duplicate {label}: {value}"
            )));
        }
    }

    errors
}

fn runtime_errors_from_contract_report(text: String) -> Vec<RuntimeValidationError> {
    text.split("; ")
        .map(|message| RuntimeValidationError::new(message.to_owned()))
        .collect()
}

fn finish_validation(errors: Vec<RuntimeValidationError>) -> Result<(), RuntimeValidationReport> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(RuntimeValidationReport::new(errors))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IssueSeverity, RuntimeIssue};
    use serde_json::json;
    use skenion_contracts::{
        GraphFragmentOutsideEndpointPolicyV01, GraphFragmentV01, GraphFragmentViewV01,
        IdConflictPolicy, InterfaceIncidentEdgePolicyV01, InterfaceIssueDetailV01,
        InterfaceRecoveryActionIdV01, PasteGraphFragmentOptions, PatchPath, PortDirectionV01,
    };

    #[test]
    fn runtime_operation_and_paste_response_validation_reports_semantic_errors() {
        let envelope = RuntimeOperationEnvelope {
            schema: "wrong.schema".to_owned(),
            schema_version: "9.9.9".to_owned(),
            id: String::new(),
            kind: "unsupported".to_owned(),
            request: paste_request(),
            attribution: Some(RuntimeOperationAttribution {
                actor_id: Some("actor_1".to_owned()),
                client_id: Some("client_1".to_owned()),
                label: Some("paste".to_owned()),
            }),
            correlation_id: Some("correlation_1".to_owned()),
            created_at: Some("2026-06-27T00:00:00.000Z".to_owned()),
        };

        let report = validate_runtime_operation_envelope(&envelope).unwrap_err();

        assert_messages_include(
            &report,
            &[
                "expected schema skenion.runtime.operation",
                "expected schemaVersion 0.1.0",
                "runtime operation id must not be empty",
                "unsupported runtime operation kind",
            ],
        );

        let response = PasteGraphFragmentResponse {
            schema: "wrong.response".to_owned(),
            schema_version: "9.9.9".to_owned(),
            ok: false,
            applied: true,
            conflict: false,
            target: root_target(),
            revision_before: "1".to_owned(),
            revision_after: None,
            history_entry_id: None,
            id_remap: IdRemapResult {
                node_id_map: BTreeMap::new(),
                edge_id_map: BTreeMap::new(),
                omitted_edge_ids: Vec::new(),
            },
            issues: vec![
                RuntimeOperationIssue {
                    severity: "error".to_owned(),
                    code: "interface-drift".to_owned(),
                    message: "drift".to_owned(),
                    path: None,
                    target: None,
                    expected_revision: None,
                    actual_revision: None,
                    duplicates: None,
                    nodes: None,
                    edges: None,
                    interface_policy: Some(InterfaceIncidentEdgePolicyV01::Drop),
                    interface_detail: None,
                },
                RuntimeOperationIssue {
                    severity: "error".to_owned(),
                    code: "invalid-incident-edge".to_owned(),
                    message: "edge".to_owned(),
                    path: None,
                    target: None,
                    expected_revision: None,
                    actual_revision: None,
                    duplicates: None,
                    nodes: None,
                    edges: None,
                    interface_policy: Some(InterfaceIncidentEdgePolicyV01::PreserveIssue),
                    interface_detail: Some(interface_detail(Vec::new())),
                },
            ],
        };

        let report = validate_paste_graph_fragment_response(&response).unwrap_err();

        assert_messages_include(
            &report,
            &[
                "expected schema skenion.runtime.paste-graph-fragment.response",
                "expected schemaVersion 0.1.0",
                "paste response cannot be applied when ok is false",
                "applied paste response must include revisionAfter",
                "issue interface-drift requires interfaceDetail",
                "interfaceDetail requires recoveryActions",
            ],
        );
    }

    #[test]
    fn collaboration_operation_and_batch_validation_reports_semantic_errors() {
        let operation = RuntimeCollaborationOperationEnvelope {
            schema: "wrong.operation".to_owned(),
            schema_version: "9.9.9".to_owned(),
            operation_id: "operation_1".to_owned(),
            session_id: "other-session".to_owned(),
            participant_id: "participant_1".to_owned(),
            idempotency_key: "idem_1".to_owned(),
            causal: causal_without_participant(),
            payload: RuntimeCollaborationOperationPayload::ChangeSet {
                target: root_target(),
                changes: vec![node_move("dup"), node_move("dup")],
                undo_group_id: Some("undo_1".to_owned()),
                description: Some("move".to_owned()),
            },
            auth_subject: Some(RuntimeCollaborationAuthSubject {
                kind: RuntimeCollaborationAuthSubjectKind::User,
                subject_id: Some("participant_1".to_owned()),
                issuer: Some("test".to_owned()),
                display_name: Some("Participant".to_owned()),
            }),
            correlation_id: Some("correlation_1".to_owned()),
            submitted_at: "2026-06-27T00:00:00.000Z".to_owned(),
        };

        let report = validate_runtime_collaboration_operation_envelope(&operation).unwrap_err();

        assert_messages_include(
            &report,
            &[
                "expected schema skenion.runtime.collaboration.operation",
                "expected schemaVersion 0.1.0",
                "operation causal baseSequence must be greater than or equal to the causal vector maximum",
                "operation participantId must not mirror auth subject id",
                "duplicate collaboration change id: dup",
                "operation causal vector must include participantId",
            ],
        );

        let batch = RuntimeCollaborationOperationBatch {
            schema: "wrong.batch".to_owned(),
            schema_version: "9.9.9".to_owned(),
            session_id: "session_1".to_owned(),
            operations: vec![operation.clone(), operation],
            submitted_at: Some("2026-06-27T00:00:00.000Z".to_owned()),
        };

        let report = validate_runtime_collaboration_operation_batch(&batch).unwrap_err();

        assert_messages_include(
            &report,
            &[
                "expected schema skenion.runtime.collaboration.operation-batch",
                "expected schemaVersion 0.1.0",
                "duplicate collaboration idempotency key: idem_1",
                "collaboration batch operation sessionId must match batch sessionId",
            ],
        );
    }

    #[test]
    fn collaboration_results_validate_ack_nack_rebase_and_batch_shape() {
        let mut accepted = operation_result(RuntimeCollaborationOperationStatus::Accepted);
        accepted.ack = None;
        accepted.nack = Some(nack(RuntimeCollaborationNackReason::InvalidOperation));
        accepted.rebase = Some(rebase());
        accepted.causal = causal_without_participant();

        let report = validate_runtime_collaboration_operation_result(&accepted).unwrap_err();

        assert_messages_include(
            &report,
            &[
                "operation result causal baseSequence must be greater than or equal to the causal vector maximum",
                "accepted or rebased collaboration result must include ack",
                "accepted collaboration result must not include nack or rebase",
            ],
        );

        let mut duplicate = operation_result(RuntimeCollaborationOperationStatus::Duplicate);
        duplicate.nack = Some(nack(RuntimeCollaborationNackReason::InvalidOperation));

        let report = validate_runtime_collaboration_operation_result(&duplicate).unwrap_err();

        assert_messages_include(
            &report,
            &["duplicate collaboration result nack reason must be duplicate-idempotency-key"],
        );

        let mut rebased = operation_result(RuntimeCollaborationOperationStatus::Rebased);
        rebased.ack = Some(ack());
        rebased.rebase = None;

        let report = validate_runtime_collaboration_operation_result(&rebased).unwrap_err();

        assert_messages_include(
            &report,
            &["rebased collaboration result must include rebase"],
        );

        let mut mismatched = operation_result(RuntimeCollaborationOperationStatus::Rejected);
        mismatched.session_id = "other-session".to_owned();
        mismatched.idempotency_key = "idem_batch".to_owned();
        mismatched.nack = None;

        let batch = RuntimeCollaborationOperationBatchResult {
            schema: "wrong.batch-result".to_owned(),
            schema_version: "9.9.9".to_owned(),
            session_id: "session_1".to_owned(),
            results: vec![mismatched.clone(), mismatched],
            issues: Vec::new(),
            created_at: "2026-06-27T00:00:01.000Z".to_owned(),
        };

        let report = validate_runtime_collaboration_operation_batch_result(&batch).unwrap_err();

        assert_messages_include(
            &report,
            &[
                "expected schema skenion.runtime.collaboration.operation-batch-result",
                "expected schemaVersion 0.1.0",
                "duplicate collaboration batch result idempotency key: idem_batch",
                "collaboration batch result operation sessionId must match batch result sessionId",
                "duplicate or rejected collaboration result must include nack",
            ],
        );
    }

    #[test]
    fn presence_selection_event_session_and_history_validation_report_transport_errors() {
        let presence = RuntimeCollaborationPresenceEnvelope {
            schema: "wrong.presence".to_owned(),
            schema_version: "9.9.9".to_owned(),
            session_id: "session_1".to_owned(),
            participant_id: "participant_1".to_owned(),
            presence: RuntimeCollaborationPresence {
                state: RuntimeCollaborationPresenceState::Active,
                display_name: Some("Participant".to_owned()),
                color: Some("#00f".to_owned()),
                status_text: Some("editing".to_owned()),
                capabilities: Some(vec!["edit".to_owned()]),
                connection_id: Some("connection_1".to_owned()),
                client_window_id: Some("window_1".to_owned()),
            },
            auth_subject: Some(RuntimeCollaborationAuthSubject {
                kind: RuntimeCollaborationAuthSubjectKind::User,
                subject_id: Some("participant_1".to_owned()),
                issuer: Some("test".to_owned()),
                display_name: Some("Participant".to_owned()),
            }),
            updated_at: "2026-06-27T00:05:00.000Z".to_owned(),
            expires_at: "2026-06-27T00:04:00.000Z".to_owned(),
        };

        let report = validate_runtime_collaboration_presence_envelope(&presence).unwrap_err();

        assert_messages_include(
            &report,
            &[
                "expected schema skenion.runtime.collaboration.presence",
                "expected schemaVersion 0.1.0",
                "presence participantId must not mirror auth subject id",
                "presence expiresAt must be later than updatedAt",
            ],
        );

        let selection = RuntimeCollaborationSelectionEnvelope {
            schema: "wrong.selection".to_owned(),
            schema_version: "9.9.9".to_owned(),
            session_id: "session_1".to_owned(),
            participant_id: "participant_1".to_owned(),
            target: root_target(),
            selection: RuntimeCollaborationSelection {
                ranges: vec![RuntimeCollaborationSelectionRange::Ports {
                    endpoints: vec![RuntimeCollaborationPortEndpoint {
                        node_id: "node_1".to_owned(),
                        port_id: "out".to_owned(),
                    }],
                }],
                active_range_index: Some(0),
            },
            cursor: Some(RuntimeCollaborationCursor::Node {
                node_id: "node_1".to_owned(),
                port_id: Some("out".to_owned()),
                client_window_id: Some("window_1".to_owned()),
            }),
            updated_at: "2026-06-27T00:05:00.000Z".to_owned(),
            expires_at: "2026-06-27T00:04:00.000Z".to_owned(),
        };

        let report = validate_runtime_collaboration_selection_envelope(&selection).unwrap_err();

        assert_messages_include(
            &report,
            &[
                "expected schema skenion.runtime.collaboration.selection",
                "expected schemaVersion 0.1.0",
                "selection expiresAt must be later than updatedAt",
            ],
        );

        let event = RuntimeCollaborationEventEnvelope {
            schema: "wrong.event".to_owned(),
            schema_version: "9.9.9".to_owned(),
            event_id: "event_1".to_owned(),
            session_id: "session_1".to_owned(),
            sequence: 1,
            causal: causal_without_participant(),
            kind: RuntimeCollaborationEventKind::Presence,
            payload: RuntimeCollaborationEventPayload::Selection {
                selection: Box::new(selection),
            },
            replay: replay_with_bad_gap(),
            created_at: "2026-06-27T00:00:02.000Z".to_owned(),
        };

        let report = validate_runtime_collaboration_event_envelope(&event).unwrap_err();

        assert_messages_include(
            &report,
            &[
                "expected schema skenion.runtime.collaboration.event",
                "expected schemaVersion 0.1.0",
                "collaboration event causal baseSequence must be greater than or equal to the causal vector maximum",
                "collaboration event kind must match payload kind",
                "collaboration event replay gap expectedSequence must be less than actualSequence",
            ],
        );

        let session_info = RuntimeSessionInfoResponse {
            schema: "wrong.session.info".to_owned(),
            schema_version: "9.9.9".to_owned(),
            ok: true,
            session_id: String::new(),
            lifecycle: RuntimeSessionLifecycleState::Ready,
            snapshot: bad_snapshot(),
            profile: bad_profile(),
            capabilities: RuntimeSessionCapabilitySet {
                session_addressing: true,
                event_replay: true,
                multi_window: true,
                auth_policy: "required".to_owned(),
            },
            event_replay: RuntimeEventReplayWindow {
                cursor_kind: "timestamp".to_owned(),
                current_cursor: String::new(),
                earliest_sequence: 0,
                latest_sequence: 0,
                replay_limit: Some(128),
                overflow: Some(false),
            },
            issues: vec![empty_runtime_issue()],
        };

        let report = validate_runtime_session_info_response(&session_info).unwrap_err();

        assert_messages_include(
            &report,
            &[
                "expected schema skenion.runtime.session.info",
                "expected schemaVersion 0.1.0",
                "sessionId must not be empty",
                "snapshot issues must include non-empty message",
                "snapshot plan must be an object or null",
                "session info issues must include non-empty message",
                "endpoint url must not be empty",
                "runtime session authPolicy must be deferred",
                "runtime eventReplay cursorKind must be sequence",
                "runtime eventReplay currentCursor must not be empty",
                "runtime eventReplay earliestSequence must be at least 1",
                "runtime profile ownership must match",
            ],
        );

        let session_event = RuntimeSessionEvent {
            schema: "wrong.session.event".to_owned(),
            schema_version: "9.9.9".to_owned(),
            id: String::new(),
            session_id: String::new(),
            sequence: 0,
            session_revision: 99,
            kind: RuntimeSessionEventKind::Mutate,
            snapshot: bad_snapshot(),
            history: bad_history(),
            mutation: Some(bad_history_entry()),
            replay: replay_with_bad_gap(),
            issues: vec![empty_runtime_issue()],
            created_at: String::new(),
        };

        let report = validate_runtime_session_event(&session_event).unwrap_err();

        assert_messages_include(
            &report,
            &[
                "expected schema skenion.runtime.session.event",
                "expected schemaVersion 0.1.0",
                "sessionId must not be empty",
                "event id must not be empty",
                "sequence must be at least 1",
                "createdAt must not be empty",
                "event issues must include non-empty message",
                "expected history schema skenion.runtime.history",
                "history entry id must not be empty",
                "mutation viewPatch operation nodeId must not be empty",
                "replay cursor must not be empty",
                "replay previousCursor must not be empty",
                "replay gap expectedSequence must be less than actualSequence",
                "event sessionRevision must match snapshot.sessionRevision",
            ],
        );
    }

    fn assert_messages_include(report: &RuntimeValidationReport, expected: &[&str]) {
        let text = report.to_string();
        for expected in expected {
            assert!(
                text.contains(expected),
                "expected report to include {expected:?}, got {text:?}"
            );
        }
    }

    fn root_target() -> GraphTargetRef {
        GraphTargetRef {
            path: PatchPath::Root,
            base_revision: "1".to_owned(),
            target_revision: None,
        }
    }

    fn paste_request() -> PasteGraphFragmentRequest {
        PasteGraphFragmentRequest {
            target: root_target(),
            fragment: GraphFragmentV01 {
                schema: "skenion.graph.fragment".to_owned(),
                schema_version: "0.1.0".to_owned(),
                id: Some("fragment_1".to_owned()),
                nodes: Vec::new(),
                edges: Vec::new(),
                view: Some(GraphFragmentViewV01 {
                    nodes: Some(BTreeMap::new()),
                }),
                omitted_edges: Some(Vec::new()),
                metadata: None,
            },
            placement: None,
            options: Some(PasteGraphFragmentOptions {
                outside_endpoint_policy: Some(GraphFragmentOutsideEndpointPolicyV01::Reject),
                id_conflict_policy: Some(IdConflictPolicy::Reject),
                interface_incident_edge_policy: Some(InterfaceIncidentEdgePolicyV01::Reject),
                preserve_relative_positions: Some(true),
            }),
        }
    }

    fn interface_detail(
        recovery_actions: Vec<InterfaceRecoveryActionIdV01>,
    ) -> InterfaceIssueDetailV01 {
        InterfaceIssueDetailV01 {
            edge_id: "edge_1".to_owned(),
            source_node_id: "source_1".to_owned(),
            source_port_id: "out".to_owned(),
            target_node_id: "target_1".to_owned(),
            target_port_id: "in".to_owned(),
            missing_endpoint: None,
            expected_direction: Some(PortDirectionV01::Input),
            actual_direction: Some(PortDirectionV01::Output),
            expected_type: Some("value.core.float32".to_owned()),
            actual_type: Some("value.core.string".to_owned()),
            cardinality: None,
            recovery_actions,
        }
    }

    fn causal_without_participant() -> RuntimeCollaborationCausalMetadata {
        RuntimeCollaborationCausalMetadata {
            base_revision: "1".to_owned(),
            base_sequence: 1,
            vector: BTreeMap::from([("other_participant".to_owned(), 2)]),
            observed_operation_ids: Some(vec!["operation_0".to_owned()]),
        }
    }

    fn valid_causal() -> RuntimeCollaborationCausalMetadata {
        RuntimeCollaborationCausalMetadata {
            base_revision: "1".to_owned(),
            base_sequence: 2,
            vector: BTreeMap::from([("participant_1".to_owned(), 2)]),
            observed_operation_ids: None,
        }
    }

    fn node_move(change_id: &str) -> RuntimeCollaborationChange {
        RuntimeCollaborationChange::NodeMove {
            change_id: change_id.to_owned(),
            node_id: "node_1".to_owned(),
            from: Some(RuntimeCollaborationCanvasPosition { x: 0.0, y: 0.0 }),
            to: RuntimeCollaborationCanvasPosition { x: 1.0, y: 1.0 },
        }
    }

    fn operation_result(
        status: RuntimeCollaborationOperationStatus,
    ) -> RuntimeCollaborationOperationResult {
        RuntimeCollaborationOperationResult {
            schema: "skenion.runtime.collaboration.operation-result".to_owned(),
            schema_version: "0.1.0".to_owned(),
            session_id: "session_1".to_owned(),
            operation_id: "operation_1".to_owned(),
            participant_id: "participant_1".to_owned(),
            idempotency_key: "idem_1".to_owned(),
            status,
            causal: valid_causal(),
            ack: Some(ack()),
            nack: None,
            rebase: None,
            issues: Vec::new(),
            created_at: "2026-06-27T00:00:00.000Z".to_owned(),
        }
    }

    fn ack() -> RuntimeCollaborationAck {
        RuntimeCollaborationAck {
            sequence: 2,
            revision: "2".to_owned(),
            server_clock: RuntimeCollaborationServerClock {
                revision: "2".to_owned(),
                sequence: 2,
                vector: BTreeMap::from([("participant_1".to_owned(), 2)]),
            },
            applied_at: "2026-06-27T00:00:00.000Z".to_owned(),
        }
    }

    fn nack(reason: RuntimeCollaborationNackReason) -> RuntimeCollaborationNack {
        RuntimeCollaborationNack {
            reason,
            retryable: Some(false),
            issues: Some(vec![RuntimeCollaborationOperationIssue {
                severity: "error".to_owned(),
                code: "invalid-operation".to_owned(),
                message: "invalid".to_owned(),
                path: Some("/payload".to_owned()),
                participant_id: Some("participant_1".to_owned()),
                operation_id: Some("operation_1".to_owned()),
                idempotency_key: Some("idem_1".to_owned()),
                expected_revision: Some("2".to_owned()),
                actual_revision: Some("1".to_owned()),
                expected_sequence: Some(2),
                actual_sequence: Some(1),
            }]),
        }
    }

    fn rebase() -> RuntimeCollaborationRebase {
        RuntimeCollaborationRebase {
            from: valid_causal(),
            to: valid_causal(),
            strategy: RuntimeCollaborationRebaseStrategy::OtTransform,
            transformed_payload: Some(RuntimeCollaborationOperationPayload::UndoRedo {
                action: RuntimeCollaborationUndoRedoAction::Undo,
                scope: RuntimeCollaborationUndoScope {
                    kind: RuntimeCollaborationUndoScopeKind::Participant,
                    participant_id: "participant_1".to_owned(),
                },
                subject_operation_id: Some("operation_0".to_owned()),
                undo_group_id: None,
                max_operations: Some(1),
            }),
            conflicts: vec![RuntimeCollaborationConflict {
                code: "conflict".to_owned(),
                message: "conflict".to_owned(),
                change_ids: Some(vec!["change_1".to_owned()]),
                node_ids: Some(vec!["node_1".to_owned()]),
                edge_ids: Some(vec!["edge_1".to_owned()]),
            }],
        }
    }

    fn replay_with_bad_gap() -> RuntimeEventReplayMetadata {
        RuntimeEventReplayMetadata {
            cursor: String::new(),
            previous_cursor: Some(String::new()),
            replayed: true,
            gap: Some(RuntimeEventReplayGap {
                expected_sequence: 2,
                actual_sequence: 1,
                reason: RuntimeEventReplayGapReason::RetentionOverflow,
            }),
            overflow: true,
        }
    }

    fn bad_snapshot() -> RuntimeTransportSessionSnapshot {
        RuntimeTransportSessionSnapshot {
            session_revision: 1,
            view_revision: 1,
            control_revision: 1,
            project: None,
            binding_formats: Vec::new(),
            issues: vec![empty_runtime_issue()],
            plan: Some(json!(false)),
        }
    }

    fn empty_runtime_issue() -> RuntimeIssue {
        RuntimeIssue {
            severity: IssueSeverity::Error,
            message: String::new(),
            code: Some("runtime.empty".to_owned()),
            details: Some(json!({ "field": "message" })),
        }
    }

    fn bad_profile() -> RuntimeConnectionProfile {
        RuntimeConnectionProfile {
            mode: RuntimeConnectionProfileMode::LocalManaged,
            ownership: RuntimeOwnershipMode::Remote,
            display_name: Some("broken".to_owned()),
            endpoint: RuntimeEndpointMetadata {
                url: String::new(),
                canonical_url: Some(String::new()),
                protocol: RuntimeEndpointProtocol::Http,
                host: Some(String::new()),
                port: Some(3761),
                tls: Some(false),
            },
            process: Some(RuntimeProcessMetadata {
                owned_by_host: true,
                pid: Some(0),
                executable_path: Some(String::new()),
                working_directory: Some(String::new()),
                started_at: Some("2026-06-27T00:00:00.000Z".to_owned()),
                owner_window_id: Some(String::new()),
                platform: Some(String::new()),
                arch: Some(String::new()),
            }),
        }
    }

    fn bad_history() -> RuntimeTransportHistory {
        RuntimeTransportHistory {
            schema: "wrong.history".to_owned(),
            schema_version: "9.9.9".to_owned(),
            entries: vec![bad_history_entry()],
            can_undo: true,
            can_redo: true,
            undo_depth: 1,
            redo_depth: 1,
        }
    }

    fn bad_history_entry() -> RuntimeTransportHistoryEntry {
        RuntimeTransportHistoryEntry {
            id: String::new(),
            sequence: 0,
            kind: RuntimeTransportHistoryEntryKind::Apply,
            mutation: bad_mutation(),
            inverse_mutation: bad_mutation(),
            subject_event_id: Some(String::new()),
            client_id: Some(String::new()),
            description: Some("bad".to_owned()),
            created_at: String::new(),
        }
    }

    fn bad_mutation() -> RuntimeTransportMutationRequest {
        RuntimeTransportMutationRequest {
            operation: None,
            view_patch: Some(RuntimeTransportViewPatch {
                base_view_revision: 1,
                ops: vec![
                    RuntimeTransportViewPatchOperation::SetNodeView {
                        node_id: String::new(),
                        view: CanvasNodeViewV01 {
                            x: 0.0,
                            y: 0.0,
                            width: Some(10.0),
                            height: Some(10.0),
                            collapsed: Some(false),
                        },
                    },
                    RuntimeTransportViewPatchOperation::MoveNodeView {
                        node_id: String::new(),
                        from: None,
                        to: CanvasNodeViewV01 {
                            x: 1.0,
                            y: 1.0,
                            width: None,
                            height: None,
                            collapsed: None,
                        },
                    },
                ],
            }),
            client_id: Some(String::new()),
            description: Some("bad mutation".to_owned()),
        }
    }
}
