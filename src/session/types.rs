use serde::{Deserialize, Serialize};

use crate::{
    CanvasNodeView, EndpointBindingValueFormat, ExecutionPlan, GraphPatch, ProjectDocumentCurrent,
    RuntimeIssue, ViewState,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSessionSnapshot {
    pub session_revision: u64,
    pub view_revision: u64,
    pub control_revision: u64,
    #[serde(skip)]
    pub package_registry_revision: Option<u64>,
    pub project: Option<ProjectDocumentCurrent>,
    pub binding_formats: Vec<EndpointBindingValueFormat>,
    pub issues: Vec<RuntimeIssue>,
    pub plan: Option<ExecutionPlan>,
}

impl RuntimeSessionSnapshot {
    pub fn loaded(&self) -> bool {
        self.project.is_some()
    }

    pub fn graph_id(&self) -> Option<&str> {
        self.project
            .as_ref()
            .map(|project| project.graph.id.as_str())
    }

    pub fn graph_revision(&self) -> Option<&str> {
        self.project
            .as_ref()
            .map(|project| project.graph.revision.as_str())
    }

    pub fn view_state(&self) -> Option<&ViewState> {
        self.project.as_ref().map(|project| &project.view_state)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSessionResponse {
    pub ok: bool,
    pub snapshot: RuntimeSessionSnapshot,
    pub issues: Vec<RuntimeIssue>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePatchResponse {
    pub ok: bool,
    pub applied: bool,
    pub conflict: bool,
    pub snapshot: RuntimeSessionSnapshot,
    pub history: RuntimeHistory,
    pub issues: Vec<RuntimeIssue>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMutationRequest {
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) graph_patch: Option<GraphPatch>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_patch: Option<RuntimeViewPatch>,
    #[serde(skip)]
    pub actor_id: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl RuntimeMutationRequest {
    pub fn view_patch(view_patch: RuntimeViewPatch) -> Self {
        Self {
            graph_patch: None,
            view_patch: Some(view_patch),
            actor_id: None,
            client_id: None,
            description: None,
        }
    }

    pub fn with_client_id(mut self, client_id: impl Into<String>) -> Self {
        self.client_id = Some(client_id.into());
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeViewPatch {
    pub base_view_revision: u64,
    pub ops: Vec<RuntimeViewPatchOperation>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "op")]
pub enum RuntimeViewPatchOperation {
    #[serde(rename = "setNodeView")]
    SetNodeView {
        #[serde(rename = "nodeId")]
        node_id: String,
        view: CanvasNodeView,
    },
    #[serde(rename = "moveNodeView")]
    MoveNodeView {
        #[serde(rename = "nodeId")]
        node_id: String,
        #[serde(default)]
        from: Option<CanvasNodeView>,
        to: CanvasNodeView,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeHistory {
    pub schema: &'static str,
    pub schema_version: &'static str,
    pub entries: Vec<RuntimeHistoryEntry>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub undo_depth: u64,
    pub redo_depth: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeHistoryEntry {
    pub id: String,
    pub sequence: u64,
    pub kind: RuntimeHistoryEntryKind,
    pub mutation: RuntimeMutationRequest,
    pub inverse_mutation: RuntimeMutationRequest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_event_id: Option<String>,
    #[serde(skip)]
    pub actor_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeHistoryEntryKind {
    Apply,
    Undo,
    Redo,
}
