use std::{
    collections::{BTreeMap, HashSet},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    CanvasNodeView, CanvasViewState, ControlState, DataFlow, DataType, DummyExecutionReport, Edge,
    EdgeSpecCurrent, EndpointBindingValueFormat, ExecutionPlan, GraphDocument,
    GraphDocumentCurrent, GraphFragmentOutsideEndpointPolicyCurrent, GraphNode, GraphNodeCurrent,
    GraphPatch, GraphTargetRef, IdConflictPolicy, IdRemapResult, NodeDefinition,
    NodeDefinitionCurrent, NodeRegistry, PasteGraphFragmentRequest, PasteGraphFragmentResponse,
    PastePlacement, PatchPath, PlanError, Port, PortActivation, PortDirection,
    PortDirectionCurrent, PortRateCurrent, PortRef, PortSpecCurrent, PreviewContext,
    PreviewControlStateSnapshot, ProjectDocumentCurrent, ProjectRequestCurrent,
    RuntimeCollaborationChange, RuntimeControlEventRequest, RuntimeControlEventResponse,
    RuntimeControlReadRequest, RuntimeControlReadResponse, RuntimeControlReadTarget,
    RuntimeControlStateResponse, RuntimeDiagnostic, RuntimeOperationDiagnostic,
    RuntimeOperationEnvelope, StringOrStrings, ValueEndpointRef, ValueFormat, ViewState,
    build_execution_plan, build_execution_plan_request_current,
    project_current::is_payload_identity_node_kind_current,
    project_document_validation_diagnostics_current, read_graph_param, read_graph_port,
    run_dummy_execution, server::registry_from_nodes, validate_project_request_current,
};
const UNRESOLVED_OBJECT_NODE_KIND: &str = "object.core.unresolved";

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
    pub diagnostics: Vec<RuntimeDiagnostic>,
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

pub(crate) fn derive_runtime_binding_formats(
    project: Option<&ProjectDocumentCurrent>,
) -> Vec<EndpointBindingValueFormat> {
    let Some(project) = project else {
        return Vec::new();
    };

    let graph = &project.graph;
    let format_revision = runtime_binding_format_revision(graph.revision.as_str());
    let mut binding_formats = Vec::new();

    for edge in &graph.edges {
        let Some(value_format) = value_format_for_edge(graph, edge) else {
            continue;
        };
        let binding_format = EndpointBindingValueFormat {
            binding_id: edge.id.clone(),
            binding_epoch: 1,
            format_revision,
            format_digest: Some(sha256_hex_for_json(&value_format)),
            value_format,
            source: Some(ValueEndpointRef {
                node_id: edge.source.node_id.clone(),
                port_id: edge.source.port_id.clone(),
            }),
            target: Some(ValueEndpointRef {
                node_id: edge.target.node_id.clone(),
                port_id: edge.target.port_id.clone(),
            }),
            delivery: None,
        };
        if skenion_contracts::validate_endpoint_binding_value_format_v01(&binding_format).is_ok() {
            binding_formats.push(binding_format);
        }
    }

    binding_formats.sort_by(|left, right| left.binding_id.cmp(&right.binding_id));
    binding_formats
}

fn value_format_for_edge(
    graph: &GraphDocumentCurrent,
    edge: &EdgeSpecCurrent,
) -> Option<ValueFormat> {
    let port_type = edge.resolved_type.as_deref().or_else(|| {
        find_graph_port(graph, &edge.source.node_id, &edge.source.port_id)
            .map(|port| port.port_type.as_str())
    })?;
    value_format_for_port_type(port_type)
}

fn find_graph_port<'a>(
    graph: &'a GraphDocumentCurrent,
    node_id: &str,
    port_id: &str,
) -> Option<&'a PortSpecCurrent> {
    graph
        .nodes
        .iter()
        .find(|node| node.id == node_id)?
        .ports
        .iter()
        .find(|port| port.id == port_id)
}

fn value_format_for_port_type(port_type: &str) -> Option<ValueFormat> {
    let value_type_id = runtime_value_type_id_for_port_type(port_type)?;
    let value_format = ValueFormat {
        format: runtime_value_format_label(value_type_id.as_str()).map(str::to_owned),
        value_type_id,
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
    };

    if skenion_contracts::validate_value_format_v01(&value_format).is_ok() {
        Some(value_format)
    } else {
        None
    }
}

fn runtime_value_type_id_for_port_type(port_type: &str) -> Option<String> {
    match port_type {
        "value.core.message" => Some("value.core.message".to_owned()),
        "value.core.float32" => Some("value.core.float32".to_owned()),
        "value.core.int32" => Some("value.core.int32".to_owned()),
        "value.core.uint32" => Some("value.core.uint32".to_owned()),
        "value.core.bool" => Some("value.core.bool".to_owned()),
        "value.core.string" => Some("value.core.string".to_owned()),
        "value.core.color" => Some("value.core.color".to_owned()),
        "value.core.bang" => Some("value.core.bang".to_owned()),
        value_type if value_type.starts_with("value.") => Some(value_type.to_owned()),
        _ => None,
    }
}

fn runtime_value_format_label(value_type_id: &str) -> Option<&'static str> {
    match value_type_id {
        "value.core.float16" => Some("f16"),
        "value.core.float32" => Some("f32"),
        "value.core.float64" => Some("f64"),
        "value.core.ufloat8" => Some("ufloat8"),
        "value.core.ufloat16" => Some("ufloat16"),
        "value.core.ufloat32" => Some("ufloat32"),
        "value.core.ufloat64" => Some("ufloat64"),
        "value.core.int8" => Some("i8"),
        "value.core.int16" => Some("i16"),
        "value.core.int32" => Some("i32"),
        "value.core.int64" => Some("i64"),
        "value.core.uint8" => Some("u8"),
        "value.core.uint16" => Some("u16"),
        "value.core.uint32" => Some("u32"),
        "value.core.uint64" => Some("u64"),
        "value.core.color" => Some("rgba32f"),
        _ => None,
    }
}

fn runtime_binding_format_revision(graph_revision: &str) -> u64 {
    graph_revision
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(1)
}

fn sha256_hex_for_json<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).expect("runtime binding value format should serialize");
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSessionResponse {
    pub ok: bool,
    pub snapshot: RuntimeSessionSnapshot,
    pub diagnostics: Vec<RuntimeDiagnostic>,
    pub report: Option<DummyExecutionReport>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePatchResponse {
    pub ok: bool,
    pub applied: bool,
    pub conflict: bool,
    pub snapshot: RuntimeSessionSnapshot,
    pub history: RuntimeHistory,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRunRequest {
    pub frames: Option<usize>,
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

#[derive(Debug)]
pub struct RuntimeSession {
    project: Option<ProjectDocumentCurrent>,
    nodes_current: Vec<NodeDefinitionCurrent>,
    graph: Option<GraphDocument>,
    registry: Option<NodeRegistry>,
    plan: Option<ExecutionPlan>,
    view_state: Option<ViewState>,
    control_state: ControlState,
    diagnostics: Vec<RuntimeDiagnostic>,
    revision: u64,
    view_revision: u64,
    control_revision: u64,
    history_entries: Vec<RuntimeHistoryEntry>,
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    next_event_sequence: u64,
    package_registry_revision: Option<u64>,
}

impl Default for RuntimeSession {
    fn default() -> Self {
        Self {
            project: None,
            nodes_current: Vec::new(),
            graph: None,
            registry: None,
            plan: None,
            view_state: None,
            control_state: ControlState::default(),
            diagnostics: Vec::new(),
            revision: 0,
            view_revision: 0,
            control_revision: 0,
            history_entries: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            next_event_sequence: 1,
            package_registry_revision: None,
        }
    }
}

#[derive(Debug, Clone)]
enum HistoryEntry {
    Mutation {
        event_id: String,
        actor_id: Option<String>,
        mutation: RuntimeMutationRequest,
        inverse_mutation: RuntimeMutationRequest,
    },
    ProjectDocument {
        event_id: String,
        actor_id: Option<String>,
        before: Box<ProjectDocumentCurrent>,
        after: Box<ProjectDocumentCurrent>,
        before_view_revision: u64,
        after_view_revision: u64,
        mutation: RuntimeMutationRequest,
        inverse_mutation: RuntimeMutationRequest,
    },
}

impl HistoryEntry {
    fn actor_id(&self) -> Option<&str> {
        match self {
            Self::Mutation { actor_id, .. } => actor_id.as_deref(),
            Self::ProjectDocument { actor_id, .. } => actor_id.as_deref(),
        }
    }
}

struct HistoryApplyOutcome {
    applied: bool,
    response: RuntimePatchResponse,
}

impl HistoryApplyOutcome {
    fn applied(response: RuntimePatchResponse) -> Self {
        Self {
            applied: true,
            response,
        }
    }

    fn rejected(response: RuntimePatchResponse) -> Self {
        Self {
            applied: false,
            response,
        }
    }
}

impl RuntimeSession {
    pub fn snapshot(&self) -> RuntimeSessionSnapshot {
        RuntimeSessionSnapshot {
            session_revision: self.revision,
            view_revision: self.view_revision,
            control_revision: self.control_revision,
            package_registry_revision: self.package_registry_revision,
            project: self.project.clone(),
            binding_formats: derive_runtime_binding_formats(self.project.as_ref()),
            diagnostics: self.diagnostics.clone(),
            plan: self.plan.clone(),
        }
    }

    pub(crate) fn preview_context(&self) -> Result<PreviewContext, Vec<RuntimeDiagnostic>> {
        let Some(graph) = &self.graph else {
            return Err(vec![RuntimeDiagnostic::error(
                "no project loaded in runtime session",
            )]);
        };
        let Some(plan) = &self.plan else {
            return Err(vec![RuntimeDiagnostic::error(
                "no execution plan available in runtime session",
            )]);
        };

        Ok(PreviewContext {
            graph_id: graph.id.clone(),
            graph_revision: graph.revision.clone(),
            session_revision: self.revision,
            control_revision: self.control_revision,
            graph: graph.clone(),
            plan: plan.clone(),
            control_state: self.control_state.clone(),
        })
    }

    pub fn load_project_current(
        &mut self,
        request: ProjectRequestCurrent,
    ) -> RuntimeSessionResponse {
        self.load_project_current_with_package_registry_revision(request, None)
    }

    pub fn load_project_current_with_package_registry_revision(
        &mut self,
        request: ProjectRequestCurrent,
        package_registry_revision: Option<u64>,
    ) -> RuntimeSessionResponse {
        let document = project_document_from_request_current(&request);
        if let Err(report) = skenion_contracts::validate_project_document_v01(&document) {
            let mut diagnostics =
                project_document_validation_diagnostics_current(&document, &report);
            if let Err(runtime_diagnostics) = validate_project_request_current(&request) {
                diagnostics.extend(runtime_diagnostics);
            }
            return self.response(false, diagnostics, None);
        }

        let (plan, mut diagnostics) = match build_execution_plan_request_current(&request) {
            Ok(result) => result,
            Err(diagnostics) => return self.response(false, diagnostics, None),
        };
        diagnostics.extend(unresolved_object_diagnostics_current(&document.graph));

        let graph = lower_graph_for_execution(&document.graph);
        let lowered_nodes = request
            .nodes
            .iter()
            .map(lower_node_definition_for_execution)
            .collect::<Vec<_>>();
        let registry = match registry_from_nodes(lowered_nodes) {
            Ok(registry) => registry,
            Err(diagnostics) => return self.response(false, diagnostics, None),
        };
        let control_state = ControlState::from_graph(&graph);
        let view_state = reconcile_view_state_with_graph_current(
            &document.graph,
            Some(document.view_state.clone()),
        );
        self.project = Some(document);
        self.nodes_current = request.nodes;
        self.graph = Some(graph);
        self.registry = Some(registry);
        self.plan = Some(plan);
        self.view_state = Some(view_state);
        self.control_state = control_state;
        self.view_revision = 1;
        self.control_revision = 0;
        self.diagnostics = diagnostics.clone();
        self.clear_history();
        self.revision += 1;
        self.package_registry_revision = package_registry_revision;

        self.response(true, diagnostics, None)
    }

    pub fn validate_current(&mut self) -> RuntimeSessionResponse {
        let diagnostics = match self.current_project_request_current() {
            Some(request) => match crate::validate_project_request_current(&request) {
                Ok((mut diagnostics, _)) => {
                    diagnostics.extend(unresolved_object_diagnostics_current(&request.graph));
                    diagnostics
                }
                Err(diagnostics) => diagnostics,
            },
            None => vec![RuntimeDiagnostic::error(
                "no project loaded in runtime session",
            )],
        };
        let ok = diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity != crate::DiagnosticSeverity::Error);
        self.diagnostics = diagnostics.clone();
        self.response(ok, diagnostics, None)
    }

    pub fn plan_current(&mut self) -> RuntimeSessionResponse {
        let request = match self.current_project_request_current() {
            Some(request) => request,
            None => {
                let diagnostics = vec![RuntimeDiagnostic::error(
                    "no project loaded in runtime session",
                )];
                self.diagnostics = diagnostics.clone();
                return self.response(false, diagnostics, None);
            }
        };

        match build_execution_plan_request_current(&request) {
            Ok((plan, mut diagnostics)) => {
                diagnostics.extend(unresolved_object_diagnostics_current(&request.graph));
                self.plan = Some(plan);
                self.diagnostics = diagnostics.clone();
                self.response(true, diagnostics, None)
            }
            Err(diagnostics) => {
                self.diagnostics = diagnostics.clone();
                self.plan = None;
                self.response(false, diagnostics, None)
            }
        }
    }

    pub fn run_current(&mut self, frames: usize) -> RuntimeSessionResponse {
        if self.loaded_project().is_none() {
            let diagnostics = vec![RuntimeDiagnostic::error(
                "no project loaded in runtime session",
            )];
            self.diagnostics = diagnostics.clone();
            return self.response(false, diagnostics, None);
        }

        if self.plan.is_none() {
            let response = self.plan_current();
            if !response.ok {
                return response;
            }
        }

        let report = self
            .plan
            .as_ref()
            .map(|plan| run_dummy_execution(plan, frames));
        self.response(true, self.diagnostics.clone(), report)
    }

    pub fn apply_mutation(&mut self, mutation: RuntimeMutationRequest) -> RuntimePatchResponse {
        self.apply_mutation_with_history(mutation, RuntimeHistoryEntryKind::Apply, None)
    }

    #[cfg(test)]
    pub(crate) fn apply_patch(&mut self, patch: GraphPatch) -> RuntimePatchResponse {
        self.apply_mutation(RuntimeMutationRequest {
            graph_patch: Some(patch),
            view_patch: None,
            actor_id: None,
            client_id: None,
            description: None,
        })
    }

    pub fn apply_runtime_operation(
        &mut self,
        envelope: RuntimeOperationEnvelope,
    ) -> PasteGraphFragmentResponse {
        let target = envelope.request.target.clone();
        if let Err(report) = crate::validate_runtime_operation_envelope(&envelope) {
            return self.reject_paste_response(
                target,
                false,
                operation_diagnostics_from_validation_report(report.to_string()),
                IdRemapResult {
                    node_id_map: BTreeMap::new(),
                    edge_id_map: BTreeMap::new(),
                    omitted_edge_ids: Vec::new(),
                },
            );
        }

        self.paste_graph_fragment(envelope)
    }

    pub fn apply_collaboration_change_set_current(
        &mut self,
        target: GraphTargetRef,
        changes: Vec<RuntimeCollaborationChange>,
        actor_id: Option<String>,
        client_id: Option<String>,
        description: Option<String>,
    ) -> RuntimePatchResponse {
        let Some(project) = self.project.as_ref().cloned() else {
            return self.patch_response(
                false,
                false,
                false,
                vec![RuntimeDiagnostic::structured_error(
                    "collaboration.target.no-project",
                    "no project loaded in runtime session",
                    serde_json::json!({ "target": target }),
                )],
            );
        };
        let target_revision = match target_graph_revision_current(&project, &target) {
            Ok(revision) => revision,
            Err(diagnostic) => {
                return self.patch_response(
                    false,
                    false,
                    false,
                    vec![operation_diagnostic_to_runtime_diagnostic(*diagnostic)],
                );
            }
        };
        if target.base_revision != target_revision {
            return self.patch_response(
                false,
                false,
                true,
                vec![RuntimeDiagnostic::structured_error(
                    "collaboration.revision-conflict",
                    format!(
                        "target baseRevision {} does not match target graph revision {}",
                        target.base_revision, target_revision
                    ),
                    serde_json::json!({
                        "expectedRevision": target.base_revision,
                        "actualRevision": target_revision,
                        "target": target,
                    }),
                )],
            );
        }

        let (next_project, next_view_revision) =
            match apply_collaboration_changes_to_project_current(
                project.clone(),
                self.view_revision,
                &target,
                &changes,
            ) {
                Ok(result) => result,
                Err(diagnostics) => {
                    return self.patch_response(false, false, false, diagnostics);
                }
            };
        self.apply_project_document_update(
            project,
            next_project,
            next_view_revision,
            RuntimeMutationRequest {
                graph_patch: None,
                view_patch: None,
                actor_id,
                client_id,
                description,
            },
            None,
        )
    }

    pub fn history(&self) -> RuntimeHistory {
        RuntimeHistory {
            schema: "skenion.runtime.history",
            schema_version: "0.1.0",
            entries: self.history_entries.clone(),
            can_undo: !self.undo_stack.is_empty(),
            can_redo: !self.redo_stack.is_empty(),
            undo_depth: self.undo_stack.len() as u64,
            redo_depth: self.redo_stack.len() as u64,
        }
    }

    pub fn undo(&mut self) -> RuntimePatchResponse {
        let Some(entry) = self.undo_stack.pop() else {
            return self.patch_response(
                false,
                false,
                false,
                vec![RuntimeDiagnostic::error("no patch event available to undo")],
            );
        };
        let outcome = self.apply_history_entry(entry.clone(), HistoryDirection::Undo);
        if outcome.applied {
            let response = outcome.response;
            self.redo_stack.push(entry);
            self.patch_response(true, true, false, response.diagnostics)
        } else {
            let response = outcome.response;
            self.undo_stack.push(entry);
            self.patch_response(false, false, response.conflict, response.diagnostics)
        }
    }

    pub fn undo_for_actor(&mut self, actor_id: &str) -> RuntimePatchResponse {
        let Some(index) = self
            .undo_stack
            .iter()
            .rposition(|entry| entry.actor_id() == Some(actor_id))
        else {
            return self.patch_response(
                false,
                false,
                false,
                vec![RuntimeDiagnostic::error(format!(
                    "no patch event available to undo for actor {actor_id}"
                ))],
            );
        };
        let entry = self.undo_stack.remove(index);
        let outcome = self.apply_history_entry(entry.clone(), HistoryDirection::Undo);
        if outcome.applied {
            let response = outcome.response;
            self.redo_stack.push(entry);
            self.patch_response(true, true, false, response.diagnostics)
        } else {
            let response = outcome.response;
            self.undo_stack.insert(index, entry);
            self.patch_response(false, false, response.conflict, response.diagnostics)
        }
    }

    pub fn redo(&mut self) -> RuntimePatchResponse {
        let Some(entry) = self.redo_stack.pop() else {
            return self.patch_response(
                false,
                false,
                false,
                vec![RuntimeDiagnostic::error("no patch event available to redo")],
            );
        };
        let outcome = self.apply_history_entry(entry.clone(), HistoryDirection::Redo);
        if outcome.applied {
            let response = outcome.response;
            self.undo_stack.push(entry);
            self.patch_response(true, true, false, response.diagnostics)
        } else {
            let response = outcome.response;
            self.redo_stack.push(entry);
            self.patch_response(false, false, response.conflict, response.diagnostics)
        }
    }

    pub fn redo_for_actor(&mut self, actor_id: &str) -> RuntimePatchResponse {
        let Some(index) = self
            .redo_stack
            .iter()
            .rposition(|entry| entry.actor_id() == Some(actor_id))
        else {
            return self.patch_response(
                false,
                false,
                false,
                vec![RuntimeDiagnostic::error(format!(
                    "no patch event available to redo for actor {actor_id}"
                ))],
            );
        };
        let entry = self.redo_stack.remove(index);
        let outcome = self.apply_history_entry(entry.clone(), HistoryDirection::Redo);
        if outcome.applied {
            let response = outcome.response;
            self.undo_stack.push(entry);
            self.patch_response(true, true, false, response.diagnostics)
        } else {
            let response = outcome.response;
            self.redo_stack.insert(index, entry);
            self.patch_response(false, false, response.conflict, response.diagnostics)
        }
    }

    pub fn reject_patch(
        &self,
        conflict: bool,
        diagnostics: Vec<RuntimeDiagnostic>,
    ) -> RuntimePatchResponse {
        self.patch_response(false, false, conflict, diagnostics)
    }

    pub fn clear(&mut self) -> RuntimeSessionResponse {
        self.project = None;
        self.nodes_current = Vec::new();
        self.graph = None;
        self.registry = None;
        self.plan = None;
        self.view_state = None;
        self.control_state = ControlState::default();
        self.view_revision = 0;
        self.control_revision = 0;
        self.diagnostics = Vec::new();
        self.clear_history();
        self.revision += 1;
        self.package_registry_revision = None;
        self.response(true, Vec::new(), None)
    }

    pub fn apply_control_event(
        &mut self,
        request: RuntimeControlEventRequest,
    ) -> RuntimeControlEventResponse {
        let Some(graph) = self.graph.as_ref() else {
            return RuntimeControlEventResponse {
                ok: false,
                changed: false,
                control_revision: Some(self.control_revision),
                emitted: Vec::new(),
                diagnostics: vec![RuntimeDiagnostic::error(
                    "no project loaded in runtime session",
                )],
            };
        };

        let before = self.control_state.clone();
        let response = self.control_state.apply_event(request, graph);
        if response.ok {
            let changed = self.control_state != before;
            if changed {
                self.control_revision += 1;
            }
            self.diagnostics = Vec::new();
            return response.with_runtime_metadata(changed, self.control_revision);
        } else {
            self.diagnostics = response.diagnostics.clone();
        }
        response.with_runtime_metadata(false, self.control_revision)
    }

    pub fn control_state_response(&self) -> RuntimeControlStateResponse {
        RuntimeControlStateResponse {
            ok: self.graph.is_some(),
            control_revision: self.control_revision,
            values: self.control_state.values.clone(),
            channels: self.control_state.channels.clone(),
            diagnostics: if self.graph.is_some() {
                Vec::new()
            } else {
                vec![RuntimeDiagnostic::error(
                    "no project loaded in runtime session",
                )]
            },
        }
    }

    pub fn control_revision(&self) -> u64 {
        self.control_revision
    }

    pub fn preview_control_state_snapshot(&self) -> Option<PreviewControlStateSnapshot> {
        self.graph.as_ref()?;
        Some(PreviewControlStateSnapshot::new(
            self.revision,
            self.control_revision,
            &self.control_state,
        ))
    }

    pub fn read_control(&self, request: RuntimeControlReadRequest) -> RuntimeControlReadResponse {
        let Some(graph) = self.graph.as_ref() else {
            return RuntimeControlReadResponse::error(
                request,
                "no project loaded in runtime session",
            );
        };
        let Some(node) = graph.nodes.iter().find(|node| node.id == request.node_id) else {
            let node_id = request.node_id.clone();
            return RuntimeControlReadResponse::error(
                request,
                format!("control read node {node_id} does not exist"),
            );
        };

        match request.target.clone() {
            RuntimeControlReadTarget::Param => {
                let Some(value) = read_graph_param(node, &request.id) else {
                    let node_id = node.id.clone();
                    let id = request.id.clone();
                    return RuntimeControlReadResponse::error(
                        request,
                        format!("node {node_id} param {id} does not exist"),
                    );
                };
                RuntimeControlReadResponse::ok(request, value)
            }
            RuntimeControlReadTarget::Port => {
                let Some(value) = read_graph_port(node, &request.id) else {
                    let node_id = node.id.clone();
                    let id = request.id.clone();
                    return RuntimeControlReadResponse::error(
                        request,
                        format!("node {node_id} port {id} does not exist"),
                    );
                };
                RuntimeControlReadResponse::ok(request, value)
            }
            RuntimeControlReadTarget::State => {
                if request.id != "value" {
                    let node_id = node.id.clone();
                    let id = request.id.clone();
                    return RuntimeControlReadResponse::error(
                        request,
                        format!("node {node_id} state {id} does not exist"),
                    );
                }
                let Some(value) = self.control_state.values.get(&node.id) else {
                    let node_id = node.id.clone();
                    return RuntimeControlReadResponse::error(
                        request,
                        format!("node {node_id} has no runtime control state"),
                    );
                };
                let value = serde_json::to_value(value)
                    .expect("runtime control values should serialize to JSON");
                RuntimeControlReadResponse::ok(request, value)
            }
        }
    }

    pub fn response(
        &self,
        ok: bool,
        diagnostics: Vec<RuntimeDiagnostic>,
        report: Option<DummyExecutionReport>,
    ) -> RuntimeSessionResponse {
        let snapshot = self.snapshot();
        RuntimeSessionResponse {
            ok,
            snapshot,
            diagnostics,
            report,
        }
    }

    #[cfg(test)]
    pub(crate) fn graph(&self) -> Option<GraphDocument> {
        self.graph.clone()
    }

    pub fn project_document_current(&self) -> Option<ProjectDocumentCurrent> {
        self.project.clone()
    }

    pub fn target_revision_current(&self, target: &GraphTargetRef) -> Option<String> {
        self.project
            .as_ref()
            .and_then(|project| target_graph_revision_current(project, target).ok())
    }

    pub fn view_state(&self) -> Option<ViewState> {
        self.view_state.clone()
    }

    fn current_project_request_current(&self) -> Option<ProjectRequestCurrent> {
        let project = self.project.as_ref()?;
        Some(ProjectRequestCurrent {
            document: self.project.clone(),
            graph: project.graph.clone(),
            nodes: self.nodes_current.clone(),
            patch_library: project.patch_library.clone(),
            view_state: self.view_state.clone(),
        })
    }

    fn apply_mutation_with_history(
        &mut self,
        mut mutation: RuntimeMutationRequest,
        kind: RuntimeHistoryEntryKind,
        subject_event_id: Option<String>,
    ) -> RuntimePatchResponse {
        let (graph, registry) = match (self.graph.clone(), self.registry.clone()) {
            (Some(graph), Some(registry)) => (graph, registry),
            _ => {
                return self.patch_response(
                    false,
                    false,
                    false,
                    vec![RuntimeDiagnostic::error(
                        "no project loaded in runtime session",
                    )],
                );
            }
        };

        if mutation.graph_patch.is_none() && mutation.view_patch.is_none() {
            return self.patch_response(
                false,
                false,
                false,
                vec![RuntimeDiagnostic::error(
                    "runtime mutation did not include graphPatch or viewPatch",
                )],
            );
        }

        if mutation.graph_patch.is_some() {
            return self.patch_response(
                false,
                false,
                false,
                vec![RuntimeDiagnostic::structured_error(
                    "project.graph-patch-unsupported",
                    "active Runtime sessions use current 0.1 ProjectDocument graph targets; graphPatch mutations are unsupported",
                    serde_json::json!({ "activeSchemaVersion": "0.1.0" }),
                )],
            );
        }

        let next_graph = graph.clone();

        if let Some(view_patch) = &mutation.view_patch
            && view_patch.base_view_revision != self.view_revision
        {
            return self.patch_response(
                false,
                false,
                true,
                vec![RuntimeDiagnostic::error(format!(
                    "view patch baseViewRevision {} does not match session view revision {}",
                    view_patch.base_view_revision, self.view_revision
                ))],
            );
        }

        let previous_view_state = runtime_owned_view_state(
            reconcile_view_state_with_execution_graph(&graph, self.view_state.clone()),
        );
        let mut next_view_state = reconcile_view_state_with_execution_graph(
            &next_graph,
            Some(previous_view_state.clone()),
        );
        let view_patch = mutation
            .view_patch
            .as_ref()
            .expect("view patch should exist after no-op and active v0.1 graph patch rejection");
        let (patched_view_state, inverse_patch) =
            match apply_view_patch_to_view_state(&next_graph, next_view_state, view_patch) {
                Ok(result) => result,
                Err(diagnostics) => {
                    return self.patch_response(false, false, false, diagnostics);
                }
            };
        next_view_state = patched_view_state;
        let inverse_view_patch = Some(inverse_patch);
        next_view_state = runtime_owned_view_state(next_view_state);
        let view_changed = previous_view_state != next_view_state;

        if !view_changed {
            return self.patch_response(true, false, false, Vec::new());
        }

        let plan =
            match build_session_execution_plan(&next_graph, &registry, "session-mutation-plan") {
                Ok(plan) => plan,
                Err(diagnostics) => {
                    self.plan = None;
                    self.diagnostics = diagnostics.clone();
                    return self.patch_response(false, false, false, diagnostics);
                }
            };
        let diagnostics = unresolved_object_diagnostics(&next_graph);
        let control_state = ControlState::from_graph(&next_graph);
        let mut inverse_mutation = RuntimeMutationRequest {
            graph_patch: None,
            view_patch: inverse_view_patch,
            actor_id: mutation.actor_id.clone(),
            client_id: mutation.client_id.clone(),
            description: mutation
                .description
                .as_ref()
                .map(|description| format!("Inverse of {description}")),
        };
        normalize_mutation_base_revisions(
            &mut mutation,
            graph.revision.clone(),
            self.view_revision,
        );
        normalize_mutation_base_revisions(
            &mut inverse_mutation,
            next_graph.revision.clone(),
            self.view_revision + 1,
        );
        let history_entry = self.create_runtime_history_entry(
            kind,
            mutation.clone(),
            inverse_mutation.clone(),
            subject_event_id,
        );
        let history_stack_entry = HistoryEntry::Mutation {
            event_id: history_entry.id.clone(),
            actor_id: history_entry.actor_id.clone(),
            mutation,
            inverse_mutation,
        };

        self.graph = Some(next_graph.clone());
        self.registry = Some(registry);
        self.plan = Some(plan);
        self.view_state = Some(next_view_state);
        if let (Some(project), Some(view_state)) = (self.project.as_mut(), self.view_state.as_ref())
        {
            project.view_state = view_state.clone();
        }
        if view_changed {
            self.view_revision += 1;
        }
        self.control_state = control_state;
        self.diagnostics = diagnostics.clone();
        self.revision += 1;
        self.history_entries.push(history_entry);
        if matches!(kind, RuntimeHistoryEntryKind::Apply) {
            self.undo_stack.push(history_stack_entry);
            self.redo_stack.clear();
        }

        self.patch_response(true, true, false, diagnostics)
    }

    fn paste_graph_fragment(
        &mut self,
        envelope: RuntimeOperationEnvelope,
    ) -> PasteGraphFragmentResponse {
        let request = envelope.request.clone();
        let target = request.target.clone();
        let Some(project) = self.project.as_ref().cloned() else {
            return self.reject_paste_response(
                target,
                false,
                vec![operation_error(
                    "paste.target.no-project",
                    "no project loaded in runtime session",
                    Some(request.target),
                    None,
                    None,
                    None,
                    None,
                )],
                empty_id_remap(),
            );
        };

        let target_revision = match target_graph_revision_current(&project, &request.target) {
            Ok(revision) => revision,
            Err(diagnostic) => {
                return self.reject_paste_response(
                    target,
                    false,
                    vec![*diagnostic],
                    empty_id_remap(),
                );
            }
        };

        if request.target.base_revision != target_revision {
            return self.reject_paste_response(
                target,
                true,
                vec![operation_error(
                    "paste.revision-conflict",
                    format!(
                        "target baseRevision {} does not match target graph revision {}",
                        request.target.base_revision, target_revision
                    ),
                    Some(request.target.clone()),
                    Some(request.target.base_revision.clone()),
                    Some(target_revision.clone()),
                    None,
                    None,
                )],
                empty_id_remap(),
            );
        }

        let revision_before = target_revision;
        let (next_project, next_view_revision, id_remap, revision_after) =
            match paste_graph_fragment_into_project_current(
                project.clone(),
                self.view_revision,
                &request,
            ) {
                Ok(result) => result,
                Err((diagnostics, id_remap)) => {
                    return self.reject_paste_response(target, false, diagnostics, id_remap);
                }
            };
        let mutation = RuntimeMutationRequest {
            graph_patch: None,
            view_patch: None,
            actor_id: envelope
                .attribution
                .as_ref()
                .and_then(|attribution| attribution.actor_id.clone()),
            client_id: envelope
                .attribution
                .as_ref()
                .and_then(|attribution| attribution.client_id.clone()),
            description: envelope
                .attribution
                .as_ref()
                .and_then(|attribution| attribution.label.clone())
                .or_else(|| Some(format!("Paste graph fragment {}", envelope.id))),
        };

        let response = self.apply_project_document_update(
            project,
            next_project,
            next_view_revision,
            mutation,
            None,
        );
        let history_entry_id = if response.applied {
            response
                .history
                .entries
                .last()
                .map(|entry| entry.id.clone())
        } else {
            None
        };
        let diagnostics = response
            .diagnostics
            .iter()
            .map(|diagnostic| runtime_diagnostic_to_operation_diagnostic(diagnostic, &target))
            .collect();

        PasteGraphFragmentResponse {
            schema: "skenion.runtime.paste-graph-fragment.response".to_owned(),
            schema_version: "0.1.0".to_owned(),
            ok: response.ok,
            applied: response.applied,
            conflict: response.conflict,
            target,
            revision_before,
            revision_after: response.applied.then_some(revision_after),
            history_entry_id,
            id_remap,
            diagnostics,
        }
    }

    fn apply_project_document_update(
        &mut self,
        before: ProjectDocumentCurrent,
        after: ProjectDocumentCurrent,
        next_view_revision: u64,
        mutation: RuntimeMutationRequest,
        subject_event_id: Option<String>,
    ) -> RuntimePatchResponse {
        let before_view_revision = self.view_revision;
        let request = ProjectRequestCurrent {
            document: Some(after.clone()),
            graph: after.graph.clone(),
            nodes: self.nodes_current.clone(),
            patch_library: after.patch_library.clone(),
            view_state: Some(after.view_state.clone()),
        };
        let (plan, mut diagnostics) = match build_execution_plan_request_current(&request) {
            Ok(result) => result,
            Err(diagnostics) => return self.patch_response(false, false, false, diagnostics),
        };
        diagnostics.extend(unresolved_object_diagnostics_current(&after.graph));
        let graph = lower_graph_for_execution(&after.graph);
        let registry = match registry_from_nodes(
            self.nodes_current
                .iter()
                .map(lower_node_definition_for_execution)
                .collect(),
        ) {
            Ok(registry) => registry,
            Err(diagnostics) => return self.patch_response(false, false, false, diagnostics),
        };
        let inverse_mutation = RuntimeMutationRequest {
            graph_patch: None,
            view_patch: None,
            actor_id: mutation.actor_id.clone(),
            client_id: mutation.client_id.clone(),
            description: mutation
                .description
                .as_ref()
                .map(|description| format!("Inverse of {description}")),
        };
        let history_entry = self.create_runtime_history_entry(
            RuntimeHistoryEntryKind::Apply,
            mutation.clone(),
            inverse_mutation.clone(),
            subject_event_id,
        );
        let history_stack_entry = HistoryEntry::ProjectDocument {
            event_id: history_entry.id.clone(),
            actor_id: history_entry.actor_id.clone(),
            before: Box::new(before),
            after: Box::new(after.clone()),
            before_view_revision,
            after_view_revision: next_view_revision,
            mutation,
            inverse_mutation,
        };

        self.project = Some(after);
        self.graph = Some(graph.clone());
        self.registry = Some(registry);
        self.plan = Some(plan);
        self.view_state = Some(
            self.project
                .as_ref()
                .map(|project| project.view_state.clone())
                .unwrap_or_else(|| reconcile_view_state_with_execution_graph(&graph, None)),
        );
        self.view_revision = next_view_revision;
        self.control_state = ControlState::from_graph(&graph);
        self.control_revision = 0;
        self.diagnostics = diagnostics.clone();
        self.revision += 1;
        self.history_entries.push(history_entry);
        self.undo_stack.push(history_stack_entry);
        self.redo_stack.clear();

        self.patch_response(true, true, false, diagnostics)
    }

    fn reject_paste_response(
        &self,
        target: GraphTargetRef,
        conflict: bool,
        diagnostics: Vec<RuntimeOperationDiagnostic>,
        id_remap: IdRemapResult,
    ) -> PasteGraphFragmentResponse {
        let revision_before = self
            .graph
            .as_ref()
            .map(|graph| graph.revision.clone())
            .unwrap_or_else(|| target.base_revision.clone());
        PasteGraphFragmentResponse {
            schema: "skenion.runtime.paste-graph-fragment.response".to_owned(),
            schema_version: "0.1.0".to_owned(),
            ok: false,
            applied: false,
            conflict,
            target,
            revision_before,
            revision_after: None,
            history_entry_id: None,
            id_remap,
            diagnostics,
        }
    }

    fn patch_response(
        &self,
        ok: bool,
        applied: bool,
        conflict: bool,
        diagnostics: Vec<RuntimeDiagnostic>,
    ) -> RuntimePatchResponse {
        let snapshot = self.snapshot();
        RuntimePatchResponse {
            ok,
            applied,
            conflict,
            snapshot,
            history: self.history(),
            diagnostics,
        }
    }

    fn loaded_project(&self) -> Option<(&GraphDocument, &NodeRegistry)> {
        Some((self.graph.as_ref()?, self.registry.as_ref()?))
    }

    fn apply_history_entry(
        &mut self,
        entry: HistoryEntry,
        direction: HistoryDirection,
    ) -> HistoryApplyOutcome {
        match entry {
            HistoryEntry::Mutation {
                event_id,
                mutation,
                inverse_mutation,
                ..
            } => {
                let mut mutation_to_apply = match direction {
                    HistoryDirection::Undo => inverse_mutation,
                    HistoryDirection::Redo => mutation,
                };
                self.rebase_mutation_to_current_revisions(&mut mutation_to_apply);
                let response = self.apply_mutation_with_history(
                    mutation_to_apply,
                    match direction {
                        HistoryDirection::Undo => RuntimeHistoryEntryKind::Undo,
                        HistoryDirection::Redo => RuntimeHistoryEntryKind::Redo,
                    },
                    Some(event_id),
                );
                if response.applied {
                    HistoryApplyOutcome::applied(response)
                } else {
                    HistoryApplyOutcome::rejected(response)
                }
            }
            HistoryEntry::ProjectDocument {
                event_id,
                before,
                after,
                before_view_revision,
                after_view_revision,
                mutation,
                inverse_mutation,
                ..
            } => {
                let (target_project, view_revision, mutation_to_record, inverse_to_record) =
                    match direction {
                        HistoryDirection::Undo => (
                            (*before).clone(),
                            before_view_revision,
                            inverse_mutation,
                            mutation,
                        ),
                        HistoryDirection::Redo => (
                            (*after).clone(),
                            after_view_revision,
                            mutation,
                            inverse_mutation,
                        ),
                    };
                let project = self
                    .project
                    .as_ref()
                    .map(|current| {
                        project_document_history_delta(current, &before, &after, direction)
                    })
                    .unwrap_or(target_project);
                let response = self.restore_project_document_state(
                    project,
                    view_revision,
                    match direction {
                        HistoryDirection::Undo => RuntimeHistoryEntryKind::Undo,
                        HistoryDirection::Redo => RuntimeHistoryEntryKind::Redo,
                    },
                    mutation_to_record,
                    inverse_to_record,
                    Some(event_id),
                );
                if response.applied {
                    HistoryApplyOutcome::applied(response)
                } else {
                    HistoryApplyOutcome::rejected(response)
                }
            }
        }
    }

    fn restore_project_document_state(
        &mut self,
        mut project: ProjectDocumentCurrent,
        view_revision: u64,
        mutation: RuntimeHistoryEntryKind,
        mutation_to_record: RuntimeMutationRequest,
        inverse_to_record: RuntimeMutationRequest,
        subject_event_id: Option<String>,
    ) -> RuntimePatchResponse {
        if let Some(current) = self.project.as_ref() {
            project.graph.revision = next_graph_revision(&current.graph.revision);
            project.revision = project.graph.revision.clone();
        }
        let request = ProjectRequestCurrent {
            document: Some(project.clone()),
            graph: project.graph.clone(),
            nodes: self.nodes_current.clone(),
            patch_library: project.patch_library.clone(),
            view_state: Some(project.view_state.clone()),
        };
        let (plan, mut diagnostics) = match build_execution_plan_request_current(&request) {
            Ok(result) => result,
            Err(diagnostics) => {
                return self.patch_response(false, false, false, diagnostics);
            }
        };
        diagnostics.extend(unresolved_object_diagnostics_current(&project.graph));
        let graph = lower_graph_for_execution(&project.graph);
        let registry = match registry_from_nodes(
            self.nodes_current
                .iter()
                .map(lower_node_definition_for_execution)
                .collect(),
        ) {
            Ok(registry) => registry,
            Err(diagnostics) => {
                return self.patch_response(false, false, false, diagnostics);
            }
        };
        let history_entry = self.create_runtime_history_entry(
            mutation,
            mutation_to_record,
            inverse_to_record,
            subject_event_id,
        );

        self.project = Some(project);
        self.graph = Some(graph.clone());
        self.registry = Some(registry);
        self.plan = Some(plan);
        self.view_state = Some(
            self.project
                .as_ref()
                .map(|project| project.view_state.clone())
                .unwrap_or_else(|| reconcile_view_state_with_execution_graph(&graph, None)),
        );
        self.view_revision = view_revision;
        self.control_state = ControlState::from_graph(&graph);
        self.control_revision = 0;
        self.diagnostics = diagnostics.clone();
        self.revision += 1;
        self.history_entries.push(history_entry);

        self.patch_response(true, true, false, diagnostics)
    }

    fn create_runtime_history_entry(
        &mut self,
        kind: RuntimeHistoryEntryKind,
        mutation: RuntimeMutationRequest,
        inverse_mutation: RuntimeMutationRequest,
        subject_event_id: Option<String>,
    ) -> RuntimeHistoryEntry {
        let sequence = self.next_event_sequence;
        self.next_event_sequence += 1;
        RuntimeHistoryEntry {
            id: format!("runtime_event_{sequence:06}"),
            sequence,
            kind,
            actor_id: mutation.actor_id.clone(),
            client_id: mutation.client_id.clone(),
            description: mutation.description.clone(),
            mutation,
            inverse_mutation,
            subject_event_id,
            created_at: created_at_now(),
        }
    }

    fn rebase_mutation_to_current_revisions(&self, mutation: &mut RuntimeMutationRequest) {
        if let (Some(graph), Some(graph_patch)) =
            (self.graph.as_ref(), mutation.graph_patch.as_mut())
        {
            graph_patch.base_revision = graph.revision.clone();
        }
        if let Some(view_patch) = mutation.view_patch.as_mut() {
            view_patch.base_view_revision = self.view_revision;
        }
    }

    fn clear_history(&mut self) {
        self.history_entries.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.next_event_sequence = 1;
    }
}

#[derive(Debug, Clone, Copy)]
enum HistoryDirection {
    Undo,
    Redo,
}

fn project_document_history_delta(
    current: &ProjectDocumentCurrent,
    before: &ProjectDocumentCurrent,
    after: &ProjectDocumentCurrent,
    direction: HistoryDirection,
) -> ProjectDocumentCurrent {
    let (expected_current, exact_target) = match direction {
        HistoryDirection::Undo => (after, before),
        HistoryDirection::Redo => (before, after),
    };
    if current == expected_current {
        return exact_target.clone();
    }

    let mut project = current.clone();
    apply_graph_history_delta_current(&mut project.graph, &before.graph, &after.graph, direction);
    project.view_state = view_state_history_delta_current(
        &project.view_state,
        &before.view_state,
        &after.view_state,
        direction,
    );

    for patch in &mut project.patch_library {
        let Some(before_patch) = before
            .patch_library
            .iter()
            .find(|entry| entry.id == patch.id)
        else {
            continue;
        };
        let Some(after_patch) = after
            .patch_library
            .iter()
            .find(|entry| entry.id == patch.id)
        else {
            continue;
        };
        if apply_graph_history_delta_current(
            &mut patch.graph,
            &before_patch.graph,
            &after_patch.graph,
            direction,
        ) {
            patch.graph.revision = next_graph_revision(&patch.graph.revision);
            patch.revision = patch.graph.revision.clone();
        }
    }

    project
}

fn apply_graph_history_delta_current(
    current: &mut GraphDocumentCurrent,
    before: &GraphDocumentCurrent,
    after: &GraphDocumentCurrent,
    direction: HistoryDirection,
) -> bool {
    match direction {
        HistoryDirection::Undo => undo_graph_history_delta_current(current, before, after),
        HistoryDirection::Redo => redo_graph_history_delta_current(current, before, after),
    }
}

fn undo_graph_history_delta_current(
    current: &mut GraphDocumentCurrent,
    before: &GraphDocumentCurrent,
    after: &GraphDocumentCurrent,
) -> bool {
    let before_node_ids = before
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    let added_node_ids = after
        .nodes
        .iter()
        .filter(|node| !before_node_ids.contains(node.id.as_str()))
        .map(|node| node.id.clone())
        .collect::<HashSet<_>>();
    let before_edge_ids = before
        .edges
        .iter()
        .map(|edge| edge.id.as_str())
        .collect::<HashSet<_>>();
    let added_edge_ids = after
        .edges
        .iter()
        .filter(|edge| !before_edge_ids.contains(edge.id.as_str()))
        .map(|edge| edge.id.clone())
        .collect::<HashSet<_>>();

    let before_nodes = before
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    let after_nodes = after
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<BTreeMap<_, _>>();

    let original_nodes_len = current.nodes.len();
    current
        .nodes
        .retain(|node| !added_node_ids.contains(node.id.as_str()));
    let mut changed = current.nodes.len() != original_nodes_len;

    for node in &mut current.nodes {
        let Some(before_node) = before_nodes.get(node.id.as_str()) else {
            continue;
        };
        let Some(after_node) = after_nodes.get(node.id.as_str()) else {
            continue;
        };
        if node == *after_node {
            *node = (*before_node).clone();
            changed = true;
        }
    }

    let original_edges_len = current.edges.len();
    current.edges.retain(|edge| {
        !added_edge_ids.contains(edge.id.as_str())
            && !added_node_ids.contains(edge.source.node_id.as_str())
            && !added_node_ids.contains(edge.target.node_id.as_str())
    });
    changed |= current.edges.len() != original_edges_len;

    changed
}

fn redo_graph_history_delta_current(
    current: &mut GraphDocumentCurrent,
    before: &GraphDocumentCurrent,
    after: &GraphDocumentCurrent,
) -> bool {
    let before_node_ids = before
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    let current_node_ids = current
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .collect::<HashSet<_>>();
    let before_nodes = before
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    let after_nodes = after
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    let mut changed = false;

    for node in &mut current.nodes {
        let Some(before_node) = before_nodes.get(node.id.as_str()) else {
            continue;
        };
        let Some(after_node) = after_nodes.get(node.id.as_str()) else {
            continue;
        };
        if node == *before_node {
            *node = (*after_node).clone();
            changed = true;
        }
    }
    for node in &after.nodes {
        if !before_node_ids.contains(node.id.as_str()) && !current_node_ids.contains(&node.id) {
            current.nodes.push(node.clone());
            changed = true;
        }
    }

    let before_edge_ids = before
        .edges
        .iter()
        .map(|edge| edge.id.as_str())
        .collect::<HashSet<_>>();
    let current_edge_ids = current
        .edges
        .iter()
        .map(|edge| edge.id.clone())
        .collect::<HashSet<_>>();
    let current_node_ids = current
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    for edge in &after.edges {
        if before_edge_ids.contains(edge.id.as_str()) || current_edge_ids.contains(&edge.id) {
            continue;
        }
        if current_node_ids.contains(edge.source.node_id.as_str())
            && current_node_ids.contains(edge.target.node_id.as_str())
        {
            current.edges.push(edge.clone());
            changed = true;
        }
    }

    changed
}

fn view_state_history_delta_current(
    current: &ViewState,
    before: &ViewState,
    after: &ViewState,
    direction: HistoryDirection,
) -> ViewState {
    let mut next = current.clone();
    match direction {
        HistoryDirection::Undo => {
            for node_id in after.canvas.nodes.keys() {
                if !before.canvas.nodes.contains_key(node_id) {
                    next.canvas.nodes.remove(node_id);
                }
            }
            for (node_id, before_view) in &before.canvas.nodes {
                let Some(after_view) = after.canvas.nodes.get(node_id) else {
                    continue;
                };
                if next.canvas.nodes.get(node_id) == Some(after_view) {
                    next.canvas
                        .nodes
                        .insert(node_id.clone(), before_view.clone());
                }
            }
        }
        HistoryDirection::Redo => {
            for (node_id, after_view) in &after.canvas.nodes {
                if !before.canvas.nodes.contains_key(node_id) {
                    next.canvas
                        .nodes
                        .entry(node_id.clone())
                        .or_insert_with(|| after_view.clone());
                }
            }
            for (node_id, before_view) in &before.canvas.nodes {
                let Some(after_view) = after.canvas.nodes.get(node_id) else {
                    continue;
                };
                if next.canvas.nodes.get(node_id) == Some(before_view) {
                    next.canvas
                        .nodes
                        .insert(node_id.clone(), after_view.clone());
                }
            }
        }
    }
    next.canvas.viewport = None;
    next
}

fn next_graph_revision(current: &str) -> String {
    current
        .parse::<u64>()
        .map(|revision| (revision + 1).to_string())
        .unwrap_or_else(|_| format!("{current}+1"))
}

fn project_document_from_request_current(
    request: &ProjectRequestCurrent,
) -> ProjectDocumentCurrent {
    if let Some(document) = &request.document {
        return document.clone();
    }
    let graph = request.graph.clone();
    let view_state = request
        .view_state
        .clone()
        .unwrap_or_else(|| reconcile_view_state_with_graph_current(&graph, None));
    serde_json::from_value(json!({
        "schema": "skenion.project",
        "schemaVersion": "0.1.0",
        "id": graph.id.clone(),
        "revision": graph.revision.clone(),
        "graph": graph,
        "viewState": view_state,
        "patchLibrary": request.patch_library.clone(),
    }))
    .expect("synthesized current project document should match contract shape")
}

fn lower_graph_for_execution(graph: &GraphDocumentCurrent) -> GraphDocument {
    GraphDocument {
        schema: "skenion.graph".to_owned(),
        schema_version: "0.1.0".to_owned(),
        id: graph.id.clone(),
        revision: graph.revision.clone(),
        nodes: graph
            .nodes
            .iter()
            .map(|node| lower_graph_node_for_execution(node, &node.id))
            .collect(),
        edges: graph.edges.iter().map(lower_edge_for_execution).collect(),
    }
}

fn lower_node_definition_for_execution(definition: &NodeDefinitionCurrent) -> NodeDefinition {
    NodeDefinition {
        schema: "skenion.node.definition".to_owned(),
        schema_version: "0.1.0".to_owned(),
        id: definition.id.clone(),
        version: definition.version.clone(),
        display_name: definition.display_name.clone(),
        category: definition.category.clone(),
        script_api_version: definition.script_api_version.clone(),
        bundle_hash: definition.bundle_hash.clone(),
        surface: definition
            .surface
            .as_ref()
            .map(|surface| skenion_contracts::NodeSurfaceV01 {
                palette: surface.palette.clone(),
            }),
        ports: definition
            .ports
            .iter()
            .map(lower_port_for_execution)
            .collect(),
        execution: skenion_contracts::NodeExecutionV01 {
            model: lower_execution_model_for_execution(&definition.execution.model),
            clock: definition.execution.clock.clone(),
        },
        state: skenion_contracts::NodeStateV01 {
            persistent: definition.state.persistent,
        },
        permissions: definition.permissions.clone(),
        capabilities: definition.capabilities.clone(),
    }
}

fn lower_execution_model_for_execution(
    model: &skenion_contracts::ExecutionModelV01,
) -> crate::ExecutionModel {
    match model {
        skenion_contracts::ExecutionModelV01::Event => crate::ExecutionModel::Event,
        skenion_contracts::ExecutionModelV01::Control => crate::ExecutionModel::Control,
        skenion_contracts::ExecutionModelV01::Frame => crate::ExecutionModel::Frame,
        skenion_contracts::ExecutionModelV01::AudioBlock => crate::ExecutionModel::AudioBlock,
        skenion_contracts::ExecutionModelV01::VideoFrame => crate::ExecutionModel::VideoFrame,
        skenion_contracts::ExecutionModelV01::GpuPass => crate::ExecutionModel::GpuPass,
        skenion_contracts::ExecutionModelV01::AsyncResource => crate::ExecutionModel::AsyncResource,
        skenion_contracts::ExecutionModelV01::ScriptControl => crate::ExecutionModel::ScriptControl,
        skenion_contracts::ExecutionModelV01::NativePlugin => crate::ExecutionModel::NativePlugin,
    }
}

fn reconcile_view_state_with_graph_current(
    graph: &GraphDocumentCurrent,
    view_state: Option<ViewState>,
) -> ViewState {
    let mut reconciled = default_view_state_for_graph_current(graph);
    let Some(view_state) = view_state else {
        return reconciled;
    };

    for node in &graph.nodes {
        if let Some(node_view) = view_state.canvas.nodes.get(&node.id) {
            reconciled
                .canvas
                .nodes
                .insert(node.id.clone(), node_view.clone());
        }
    }
    if view_state.canvas.viewport.is_some() {
        reconciled.canvas.viewport = view_state.canvas.viewport;
    }

    reconciled
}

fn reconcile_view_state_with_execution_graph(
    graph: &GraphDocument,
    view_state: Option<ViewState>,
) -> ViewState {
    let mut reconciled = default_view_state_for_execution_graph(graph);
    let Some(view_state) = view_state else {
        return reconciled;
    };

    for node in &graph.nodes {
        if let Some(node_view) = view_state.canvas.nodes.get(&node.id) {
            reconciled
                .canvas
                .nodes
                .insert(node.id.clone(), node_view.clone());
        }
    }
    if view_state.canvas.viewport.is_some() {
        reconciled.canvas.viewport = view_state.canvas.viewport;
    }

    reconciled
}

fn default_view_state_for_graph_current(graph: &GraphDocumentCurrent) -> ViewState {
    ViewState {
        schema: "skenion.view-state".to_owned(),
        schema_version: "0.1.0".to_owned(),
        canvas: CanvasViewState {
            nodes: graph
                .nodes
                .iter()
                .enumerate()
                .map(|(index, node)| {
                    (
                        node.id.clone(),
                        CanvasNodeView {
                            x: 160.0 * (index as f64),
                            y: 0.0,
                            width: None,
                            height: None,
                            collapsed: None,
                        },
                    )
                })
                .collect(),
            viewport: None,
        },
    }
}

fn default_view_state_for_execution_graph(graph: &GraphDocument) -> ViewState {
    ViewState {
        schema: "skenion.view-state".to_owned(),
        schema_version: "0.1.0".to_owned(),
        canvas: CanvasViewState {
            nodes: graph
                .nodes
                .iter()
                .enumerate()
                .map(|(index, node)| {
                    (
                        node.id.clone(),
                        CanvasNodeView {
                            x: 160.0 * (index as f64),
                            y: 0.0,
                            width: None,
                            height: None,
                            collapsed: None,
                        },
                    )
                })
                .collect(),
            viewport: None,
        },
    }
}

fn runtime_owned_view_state(mut view_state: ViewState) -> ViewState {
    view_state.canvas.viewport = None;
    view_state
}

fn normalize_mutation_base_revisions(
    mutation: &mut RuntimeMutationRequest,
    graph_revision: String,
    view_revision: u64,
) {
    if let Some(graph_patch) = mutation.graph_patch.as_mut() {
        graph_patch.base_revision = graph_revision;
    }
    if let Some(view_patch) = mutation.view_patch.as_mut() {
        view_patch.base_view_revision = view_revision;
    }
}

fn target_graph_revision_current(
    project: &ProjectDocumentCurrent,
    target: &GraphTargetRef,
) -> Result<String, Box<RuntimeOperationDiagnostic>> {
    Ok(target_graph_current(project, target)?.revision.clone())
}

fn target_graph_current<'a>(
    project: &'a ProjectDocumentCurrent,
    target: &GraphTargetRef,
) -> Result<&'a GraphDocumentCurrent, Box<RuntimeOperationDiagnostic>> {
    match &target.path {
        PatchPath::Root => Ok(&project.graph),
        PatchPath::HelpWorkingCopy {
            working_copy_id, ..
        } if working_copy_id == &project.graph.id => Ok(&project.graph),
        PatchPath::HelpWorkingCopy {
            working_copy_id, ..
        } => Err(Box::new(operation_error(
            "paste.target.missing-help-working-copy",
            format!("help working copy {working_copy_id} is not loaded in this runtime session"),
            Some(target.clone()),
            None,
            Some(project.graph.revision.clone()),
            None,
            None,
        ))),
        PatchPath::ProjectPatchDefinition { patch_id } => project
            .patch_library
            .iter()
            .find(|patch| patch.id == *patch_id)
            .map(|patch| &patch.graph)
            .ok_or_else(|| {
                Box::new(operation_error(
                    "paste.target.missing-project-patch-definition",
                    format!(
                        "project patch definition {patch_id} is not loaded in this runtime session"
                    ),
                    Some(target.clone()),
                    None,
                    Some(project.graph.revision.clone()),
                    None,
                    None,
                ))
            }),
        PatchPath::PackagePatchDefinition {
            package_id,
            patch_id,
            ..
        } => Err(Box::new(operation_error(
            "paste.target.immutable-help-source",
            format!(
                "package/help source patch {package_id}/{patch_id} is immutable; paste into a project patch or help working copy instead"
            ),
            Some(target.clone()),
            None,
            Some(project.graph.revision.clone()),
            None,
            None,
        ))),
        PatchPath::EmbeddedPatchInstance { node_id, .. } => Err(Box::new(operation_error(
            "paste.target.unsupported-embedded-patch-instance",
            format!(
                "embedded patch instance owned by node {node_id} cannot be mutated by the current runtime session substrate"
            ),
            Some(target.clone()),
            None,
            Some(project.graph.revision.clone()),
            None,
            None,
        ))),
    }
}

type PasteProjectResult =
    Result<(ProjectDocumentCurrent, u64, IdRemapResult, String), PasteProjectError>;
type PasteProjectError = (Vec<RuntimeOperationDiagnostic>, IdRemapResult);

fn paste_graph_fragment_into_project_current(
    mut project: ProjectDocumentCurrent,
    view_revision: u64,
    request: &PasteGraphFragmentRequest,
) -> PasteProjectResult {
    let target_path = request.target.path.clone();
    if matches!(
        target_path,
        PatchPath::PackagePatchDefinition { .. } | PatchPath::EmbeddedPatchInstance { .. }
    ) {
        return Err((
            vec![operation_error(
                "paste.target.unsupported",
                "paste target cannot be mutated in the active Runtime session",
                Some(request.target.clone()),
                None,
                None,
                None,
                None,
            )],
            empty_id_remap(),
        ));
    }
    let (next_graph, id_remap) = {
        let graph = match graph_for_path_current(&project, &target_path) {
            Some(graph) => graph,
            None => {
                return Err((
                    vec![operation_error(
                        "paste.target.missing-graph",
                        "paste target graph is not available in the active current 0.1 project",
                        Some(request.target.clone()),
                        None,
                        None,
                        None,
                        None,
                    )],
                    empty_id_remap(),
                ));
            }
        };
        paste_graph_fragment_into_graph_current(graph, request)?
    };
    let revision_after = next_graph.revision.clone();
    let mut next_view_revision = view_revision;
    if matches!(
        &target_path,
        PatchPath::Root | PatchPath::HelpWorkingCopy { .. }
    ) {
        let execution_graph = lower_graph_for_execution(&next_graph);
        let view_patch = lower_fragment_view_patch(view_revision, request, &id_remap.node_id_map);
        let view_state = if let Some(view_patch) = view_patch {
            let (view_state, _) = apply_view_patch_to_view_state(
                &execution_graph,
                reconcile_view_state_with_execution_graph(
                    &execution_graph,
                    Some(project.view_state.clone()),
                ),
                &view_patch,
            )
            .expect("lowered fragment view patch should reference pasted graph nodes");
            next_view_revision += 1;
            view_state
        } else {
            reconcile_view_state_with_execution_graph(
                &execution_graph,
                Some(project.view_state.clone()),
            )
        };
        project.graph = next_graph;
        project.revision = project.graph.revision.clone();
        project.view_state = view_state;
    } else if let PatchPath::ProjectPatchDefinition { patch_id } = &target_path {
        let patch = project
            .patch_library
            .iter_mut()
            .find(|patch| patch.id == *patch_id)
            .expect("project patch definition lookup was already proven");
        patch.graph = next_graph;
        patch.revision = patch.graph.revision.clone();
    }

    Ok((project, next_view_revision, id_remap, revision_after))
}

fn apply_collaboration_changes_to_project_current(
    mut project: ProjectDocumentCurrent,
    view_revision: u64,
    target: &GraphTargetRef,
    changes: &[RuntimeCollaborationChange],
) -> Result<(ProjectDocumentCurrent, u64), Vec<RuntimeDiagnostic>> {
    if matches!(
        &target.path,
        PatchPath::PackagePatchDefinition { .. } | PatchPath::EmbeddedPatchInstance { .. }
    ) {
        return Err(vec![RuntimeDiagnostic::structured_error(
            "collaboration.target.unsupported",
            "collaboration target cannot be mutated in the active Runtime session",
            serde_json::json!({ "target": target }),
        )]);
    }
    let mut graph = graph_for_path_current(&project, &target.path).ok_or_else(|| {
        vec![RuntimeDiagnostic::structured_error(
            "collaboration.target.missing-graph",
            "collaboration target graph is not available in the active current 0.1 project",
            serde_json::json!({ "target": target }),
        )]
    })?;
    let mut graph_changed = false;
    let mut view_changed = false;
    let mut view_state = project.view_state.clone();

    for change in changes {
        match change {
            RuntimeCollaborationChange::NodeAdd { node, view, .. } => {
                if graph.nodes.iter().any(|existing| existing.id == node.id) {
                    return Err(vec![RuntimeDiagnostic::structured_error(
                        "collaboration.node-id-conflict",
                        format!("node id {} already exists in target graph", node.id),
                        serde_json::json!({ "nodeId": node.id, "target": target }),
                    )]);
                }
                graph.nodes.push((**node).clone());
                graph_changed = true;
                if let Some(view) = view {
                    if target_supports_view_state(&target.path) {
                        view_state.canvas.nodes.insert(
                            node.id.clone(),
                            CanvasNodeView {
                                x: view.x,
                                y: view.y,
                                width: None,
                                height: None,
                                collapsed: None,
                            },
                        );
                        view_changed = true;
                    } else {
                        return Err(vec![unsupported_patch_view_change_diagnostic(target)]);
                    }
                }
            }
            RuntimeCollaborationChange::NodeMove {
                node_id, from, to, ..
            } => {
                if !target_supports_view_state(&target.path) {
                    return Err(vec![unsupported_patch_view_change_diagnostic(target)]);
                }
                if !graph.nodes.iter().any(|node| node.id == *node_id) {
                    return Err(vec![RuntimeDiagnostic::structured_error(
                        "collaboration.node-missing",
                        format!("node {node_id} does not exist in target graph"),
                        serde_json::json!({ "nodeId": node_id, "target": target }),
                    )]);
                }
                let previous =
                    view_state
                        .canvas
                        .nodes
                        .get(node_id)
                        .cloned()
                        .unwrap_or(CanvasNodeView {
                            x: 0.0,
                            y: 0.0,
                            width: None,
                            height: None,
                            collapsed: None,
                        });
                if let Some(from) = from
                    && (previous.x != from.x || previous.y != from.y)
                {
                    return Err(vec![RuntimeDiagnostic::structured_error(
                        "collaboration.view-conflict",
                        format!("node {node_id} view does not match collaboration from position"),
                        serde_json::json!({
                            "nodeId": node_id,
                            "expected": { "x": from.x, "y": from.y },
                            "actual": { "x": previous.x, "y": previous.y },
                        }),
                    )]);
                }
                view_state.canvas.nodes.insert(
                    node_id.clone(),
                    CanvasNodeView {
                        x: to.x,
                        y: to.y,
                        width: previous.width,
                        height: previous.height,
                        collapsed: previous.collapsed,
                    },
                );
                view_changed = true;
            }
            RuntimeCollaborationChange::NodeDelete { node_id, .. } => {
                let original_len = graph.nodes.len();
                graph.nodes.retain(|node| node.id != *node_id);
                if graph.nodes.len() == original_len {
                    return Err(vec![RuntimeDiagnostic::structured_error(
                        "collaboration.node-missing",
                        format!("node {node_id} does not exist in target graph"),
                        serde_json::json!({ "nodeId": node_id, "target": target }),
                    )]);
                }
                graph.edges.retain(|edge| {
                    edge.source.node_id != *node_id && edge.target.node_id != *node_id
                });
                if target_supports_view_state(&target.path) {
                    view_state.canvas.nodes.remove(node_id);
                    view_changed = true;
                }
                graph_changed = true;
            }
            RuntimeCollaborationChange::EdgeConnect { edge, .. } => {
                if graph.edges.iter().any(|existing| existing.id == edge.id) {
                    return Err(vec![RuntimeDiagnostic::structured_error(
                        "collaboration.edge-id-conflict",
                        format!("edge id {} already exists in target graph", edge.id),
                        serde_json::json!({ "edgeId": edge.id, "target": target }),
                    )]);
                }
                graph.edges.push((**edge).clone());
                graph_changed = true;
            }
            RuntimeCollaborationChange::EdgeDisconnect { edge_id, .. } => {
                let original_len = graph.edges.len();
                graph.edges.retain(|edge| edge.id != *edge_id);
                if graph.edges.len() == original_len {
                    return Err(vec![RuntimeDiagnostic::structured_error(
                        "collaboration.edge-missing",
                        format!("collaboration edge.disconnect cannot resolve edge id {edge_id}"),
                        serde_json::json!({ "edgeId": edge_id, "target": target }),
                    )]);
                }
                graph_changed = true;
            }
        }
    }

    if graph_changed {
        graph.revision = next_graph_revision(&graph.revision);
    }
    let mut next_view_revision = view_revision;
    if matches!(
        &target.path,
        PatchPath::Root | PatchPath::HelpWorkingCopy { .. }
    ) {
        let execution_graph = lower_graph_for_execution(&graph);
        project.graph = graph;
        project.revision = project.graph.revision.clone();
        project.view_state = runtime_owned_view_state(reconcile_view_state_with_execution_graph(
            &execution_graph,
            Some(view_state),
        ));
        if view_changed {
            next_view_revision += 1;
        }
    } else if let PatchPath::ProjectPatchDefinition { patch_id } = &target.path {
        let patch = project
            .patch_library
            .iter_mut()
            .find(|patch| patch.id == *patch_id)
            .expect("project patch definition lookup was already proven");
        patch.graph = graph;
        patch.revision = patch.graph.revision.clone();
    }

    Ok((project, next_view_revision))
}

fn target_supports_view_state(path: &PatchPath) -> bool {
    matches!(path, PatchPath::Root | PatchPath::HelpWorkingCopy { .. })
}

fn unsupported_patch_view_change_diagnostic(target: &GraphTargetRef) -> RuntimeDiagnostic {
    RuntimeDiagnostic::structured_error(
        "collaboration.patch-view-unsupported",
        "project patch definition targets do not currently carry editable view state in Runtime",
        serde_json::json!({ "target": target }),
    )
}

fn graph_for_path_current(
    project: &ProjectDocumentCurrent,
    path: &PatchPath,
) -> Option<GraphDocumentCurrent> {
    match path {
        PatchPath::Root => Some(project.graph.clone()),
        PatchPath::HelpWorkingCopy {
            working_copy_id, ..
        } if working_copy_id == &project.graph.id => Some(project.graph.clone()),
        PatchPath::ProjectPatchDefinition { patch_id } => project
            .patch_library
            .iter()
            .find(|patch| patch.id == *patch_id)
            .map(|patch| patch.graph.clone()),
        PatchPath::HelpWorkingCopy { .. }
        | PatchPath::PackagePatchDefinition { .. }
        | PatchPath::EmbeddedPatchInstance { .. } => None,
    }
}

fn paste_graph_fragment_into_graph_current(
    mut graph: GraphDocumentCurrent,
    request: &PasteGraphFragmentRequest,
) -> Result<(GraphDocumentCurrent, IdRemapResult), (Vec<RuntimeOperationDiagnostic>, IdRemapResult)>
{
    if let Some(interface_policy) = request
        .options
        .as_ref()
        .and_then(|options| options.interface_incident_edge_policy)
    {
        return Err((
            vec![RuntimeOperationDiagnostic {
                severity: "error".to_owned(),
                code: "paste.options.unsupported-interface-incident-edge-policy".to_owned(),
                message:
                    "interfaceIncidentEdgePolicy is not supported by the current Runtime paste substrate"
                        .to_owned(),
                path: Some("request.options.interfaceIncidentEdgePolicy".to_owned()),
                target: Some(request.target.clone()),
                expected_revision: None,
                actual_revision: Some(graph.revision.clone()),
                duplicates: None,
                nodes: None,
                edges: None,
                interface_policy: Some(interface_policy),
                interface_detail: None,
            }],
            empty_id_remap(),
        ));
    }

    let outside_policy = request
        .options
        .as_ref()
        .and_then(|options| options.outside_endpoint_policy)
        .unwrap_or(GraphFragmentOutsideEndpointPolicyCurrent::Reject);
    let id_conflict_policy = request
        .options
        .as_ref()
        .and_then(|options| options.id_conflict_policy)
        .unwrap_or(IdConflictPolicy::Remap);

    let payload_identity_diagnostics =
        payload_identity_fragment_diagnostics_current(request, &graph.revision);
    if !payload_identity_diagnostics.is_empty() {
        return Err((payload_identity_diagnostics, empty_id_remap()));
    }

    let fragment_analysis =
        skenion_contracts::analyze_graph_fragment_v01(&request.fragment, outside_policy);
    if !fragment_analysis.ok {
        let diagnostics = fragment_analysis
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == "error")
            .map(|diagnostic| RuntimeOperationDiagnostic {
                severity: diagnostic.severity.clone(),
                code: format!("paste.fragment.{}", diagnostic.code),
                message: diagnostic.message.clone(),
                path: None,
                target: Some(request.target.clone()),
                expected_revision: None,
                actual_revision: Some(graph.revision.clone()),
                duplicates: None,
                nodes: diagnostic.nodes.clone(),
                edges: diagnostic.edges.clone(),
                interface_policy: None,
                interface_detail: None,
            })
            .collect();
        return Err((
            diagnostics,
            IdRemapResult {
                omitted_edge_ids: fragment_analysis.omitted_edge_ids,
                ..empty_id_remap()
            },
        ));
    }

    let mut used_node_ids: HashSet<String> =
        graph.nodes.iter().map(|node| node.id.clone()).collect();
    let mut duplicate_nodes = Vec::new();
    let mut node_id_map = BTreeMap::new();
    for node in &request.fragment.nodes {
        let pasted_id = if used_node_ids.contains(&node.id) {
            duplicate_nodes.push(node.id.clone());
            match id_conflict_policy {
                IdConflictPolicy::Reject => node.id.clone(),
                IdConflictPolicy::Remap => next_available_node_id(&node.id, &used_node_ids),
            }
        } else {
            node.id.clone()
        };
        used_node_ids.insert(pasted_id.clone());
        node_id_map.insert(node.id.clone(), pasted_id);
    }

    if id_conflict_policy == IdConflictPolicy::Reject && !duplicate_nodes.is_empty() {
        return Err((
            vec![operation_error(
                "paste.id-conflict",
                "pasted fragment contains node ids that already exist in the target graph",
                Some(request.target.clone()),
                None,
                Some(graph.revision.clone()),
                Some(duplicate_nodes),
                None,
            )],
            IdRemapResult {
                node_id_map,
                omitted_edge_ids: fragment_analysis.omitted_edge_ids,
                ..empty_id_remap()
            },
        ));
    }

    let omitted_edge_ids: HashSet<String> =
        fragment_analysis.omitted_edge_ids.iter().cloned().collect();
    let mut used_edge_ids: HashSet<String> =
        graph.edges.iter().map(|edge| edge.id.clone()).collect();
    let mut edge_id_map = BTreeMap::new();
    for node in &request.fragment.nodes {
        let pasted_id = node_id_map
            .get(&node.id)
            .expect("node remap should include every fragment node");
        let mut node = node.clone();
        node.id = pasted_id.clone();
        graph.nodes.push(node);
    }

    for edge in &request.fragment.edges {
        if omitted_edge_ids.contains(&edge.id) {
            continue;
        }
        let pasted_edge_id = if used_edge_ids.contains(&edge.id) {
            match id_conflict_policy {
                IdConflictPolicy::Reject => {
                    return Err((
                        vec![operation_error(
                            "paste.edge-id-conflict",
                            "pasted fragment contains edge ids that already exist in the target graph",
                            Some(request.target.clone()),
                            None,
                            Some(graph.revision.clone()),
                            None,
                            Some(vec![edge.id.clone()]),
                        )],
                        IdRemapResult {
                            node_id_map,
                            edge_id_map,
                            omitted_edge_ids: fragment_analysis.omitted_edge_ids,
                        },
                    ));
                }
                IdConflictPolicy::Remap => next_available_edge_id(&edge.id, &used_edge_ids),
            }
        } else {
            edge.id.clone()
        };
        used_edge_ids.insert(pasted_edge_id.clone());
        edge_id_map.insert(edge.id.clone(), pasted_edge_id.clone());
        graph
            .edges
            .push(remap_edge_current(edge, &node_id_map, pasted_edge_id));
    }

    graph.revision = next_graph_revision(&graph.revision);

    Ok((
        graph,
        IdRemapResult {
            node_id_map,
            edge_id_map,
            omitted_edge_ids: fragment_analysis.omitted_edge_ids,
        },
    ))
}

fn payload_identity_fragment_diagnostics_current(
    request: &PasteGraphFragmentRequest,
    graph_revision: &str,
) -> Vec<RuntimeOperationDiagnostic> {
    request
        .fragment
        .nodes
        .iter()
        .filter(|node| is_payload_identity_node_kind_current(&node.kind))
        .map(|node| RuntimeOperationDiagnostic {
            severity: "error".to_owned(),
            code: "paste.fragment.payload-node-kind".to_owned(),
            message: format!(
                "node {} uses payload identity {} as an executable kind",
                node.id, node.kind
            ),
            path: None,
            target: Some(request.target.clone()),
            expected_revision: None,
            actual_revision: Some(graph_revision.to_owned()),
            duplicates: None,
            nodes: Some(vec![node.id.clone()]),
            edges: None,
            interface_policy: None,
            interface_detail: None,
        })
        .collect()
}

fn remap_edge_current(
    edge: &EdgeSpecCurrent,
    node_id_map: &BTreeMap<String, String>,
    edge_id: String,
) -> EdgeSpecCurrent {
    let mut edge = edge.clone();
    edge.id = edge_id;
    edge.source.node_id = node_id_map
        .get(&edge.source.node_id)
        .cloned()
        .unwrap_or_else(|| edge.source.node_id.clone());
    edge.target.node_id = node_id_map
        .get(&edge.target.node_id)
        .cloned()
        .unwrap_or_else(|| edge.target.node_id.clone());
    edge
}

fn next_available_edge_id(base: &str, used_edge_ids: &HashSet<String>) -> String {
    let mut index = 2;
    loop {
        let candidate = format!("{base}_{index}");
        if !used_edge_ids.contains(&candidate) {
            return candidate;
        }
        index += 1;
    }
}

fn empty_id_remap() -> IdRemapResult {
    IdRemapResult {
        node_id_map: BTreeMap::new(),
        edge_id_map: BTreeMap::new(),
        omitted_edge_ids: Vec::new(),
    }
}

fn next_available_node_id(base: &str, used_node_ids: &HashSet<String>) -> String {
    let mut index = 2;
    loop {
        let candidate = format!("{base}_{index}");
        if !used_node_ids.contains(&candidate) {
            return candidate;
        }
        index += 1;
    }
}

pub(crate) fn lower_graph_node_for_execution(
    node: &GraphNodeCurrent,
    pasted_id: &str,
) -> GraphNode {
    GraphNode {
        id: pasted_id.to_owned(),
        kind: node.kind.clone(),
        kind_version: node.kind_version.clone(),
        params: node.params.clone(),
        ports: node.ports.iter().map(lower_port_for_execution).collect(),
    }
}

fn lower_port_for_execution(port: &PortSpecCurrent) -> Port {
    Port {
        id: port.id.clone(),
        direction: match port.direction {
            PortDirectionCurrent::Input => PortDirection::Input,
            PortDirectionCurrent::Output => PortDirection::Output,
        },
        label: port.label.clone(),
        data_type: data_type_from_port_spec(port),
        required: port.required,
        default_value: port.default_value.clone(),
        activation: port.trigger_mode.as_ref().map(|trigger| match trigger {
            skenion_contracts::TriggerModeV01::Trigger => PortActivation::Trigger,
            skenion_contracts::TriggerModeV01::Latched => PortActivation::Latched,
            skenion_contracts::TriggerModeV01::Passive => PortActivation::Latched,
        }),
    }
}

fn data_type_from_port_spec(port: &PortSpecCurrent) -> DataType {
    let (canonical_flow, data_kind) = current_port_type_parts(&port.port_type);
    let format = match data_kind.as_str() {
        "value.core.float32" => Some(StringOrStrings::One("f32".to_owned())),
        "value.core.tensor" => Some(StringOrStrings::One("rgba8unorm".to_owned())),
        _ => None,
    };
    let color_space = (data_kind == "value.core.tensor").then(|| "srgb".to_owned());
    DataType {
        flow: canonical_flow.unwrap_or_else(|| match port.rate {
            Some(PortRateCurrent::Event) => DataFlow::Event,
            Some(PortRateCurrent::Audio) => DataFlow::Signal,
            Some(PortRateCurrent::Resource) | Some(PortRateCurrent::Io) => DataFlow::Resource,
            Some(PortRateCurrent::Control | PortRateCurrent::Render | PortRateCurrent::Gpu)
            | None => {
                if data_kind == "value.core.tensor" {
                    DataFlow::Resource
                } else {
                    DataFlow::Control
                }
            }
        }),
        data_kind,
        unit: None,
        range: None,
        shape: None,
        channels: None,
        sample_rate: None,
        format,
        color_space,
        frame_rate: None,
        alpha_policy: None,
        values: None,
    }
}

fn current_port_type_parts(port_type: &str) -> (Option<DataFlow>, String) {
    match port_type {
        value_type if value_type.starts_with("value.") => (None, value_type.to_owned()),
        other => (None, other.to_owned()),
    }
}

fn remap_edge(edge: &EdgeSpecCurrent, node_id_map: &BTreeMap<String, String>) -> Edge {
    Edge {
        from: PortRef {
            node: node_id_map
                .get(&edge.source.node_id)
                .cloned()
                .unwrap_or_else(|| edge.source.node_id.clone()),
            port: edge.source.port_id.clone(),
        },
        to: PortRef {
            node: node_id_map
                .get(&edge.target.node_id)
                .cloned()
                .unwrap_or_else(|| edge.target.node_id.clone()),
            port: edge.target.port_id.clone(),
        },
    }
}

pub(crate) fn lower_edge_for_execution(edge: &EdgeSpecCurrent) -> Edge {
    remap_edge(edge, &BTreeMap::new())
}

fn lower_fragment_view_patch(
    view_revision: u64,
    request: &PasteGraphFragmentRequest,
    node_id_map: &BTreeMap<String, String>,
) -> Option<RuntimeViewPatch> {
    let fragment_views = request.fragment.view.as_ref()?.nodes.as_ref()?;
    let mut ops = Vec::new();
    let placement_delta = placement_delta(request, fragment_views);
    for (source_node_id, pasted_node_id) in node_id_map {
        let Some(view) = fragment_views.get(source_node_id) else {
            continue;
        };
        let mut pasted_view = view.clone();
        if let Some((dx, dy)) = placement_delta {
            pasted_view.x += dx;
            pasted_view.y += dy;
        }
        ops.push(RuntimeViewPatchOperation::SetNodeView {
            node_id: pasted_node_id.clone(),
            view: pasted_view,
        });
    }
    (!ops.is_empty()).then_some(RuntimeViewPatch {
        base_view_revision: view_revision,
        ops,
    })
}

fn placement_delta(
    request: &PasteGraphFragmentRequest,
    fragment_views: &BTreeMap<String, CanvasNodeView>,
) -> Option<(f64, f64)> {
    match request.placement.as_ref()? {
        PastePlacement::Position { x, y } => {
            let min_x = fragment_views
                .values()
                .map(|view| view.x)
                .reduce(f64::min)
                .unwrap_or(0.0);
            let min_y = fragment_views
                .values()
                .map(|view| view.y)
                .reduce(f64::min)
                .unwrap_or(0.0);
            Some((x - min_x, y - min_y))
        }
        PastePlacement::Anchor {
            offset_x, offset_y, ..
        } => Some((offset_x.unwrap_or_default(), offset_y.unwrap_or_default())),
    }
}

fn operation_error(
    code: impl Into<String>,
    message: impl Into<String>,
    target: Option<GraphTargetRef>,
    expected_revision: Option<String>,
    actual_revision: Option<String>,
    duplicates: Option<Vec<String>>,
    edges: Option<Vec<String>>,
) -> RuntimeOperationDiagnostic {
    RuntimeOperationDiagnostic {
        severity: "error".to_owned(),
        code: code.into(),
        message: message.into(),
        path: None,
        target,
        expected_revision,
        actual_revision,
        duplicates,
        nodes: None,
        edges,
        interface_policy: None,
        interface_detail: None,
    }
}

fn operation_diagnostics_from_validation_report(
    message: String,
) -> Vec<RuntimeOperationDiagnostic> {
    vec![RuntimeOperationDiagnostic {
        severity: "error".to_owned(),
        code: "paste.operation.invalid-envelope".to_owned(),
        message,
        path: None,
        target: None,
        expected_revision: None,
        actual_revision: None,
        duplicates: None,
        nodes: None,
        edges: None,
        interface_policy: None,
        interface_detail: None,
    }]
}

fn runtime_diagnostic_to_operation_diagnostic(
    diagnostic: &RuntimeDiagnostic,
    target: &GraphTargetRef,
) -> RuntimeOperationDiagnostic {
    RuntimeOperationDiagnostic {
        severity: match diagnostic.severity {
            crate::DiagnosticSeverity::Error => "error",
            crate::DiagnosticSeverity::Warning => "warning",
            crate::DiagnosticSeverity::Info => "info",
        }
        .to_owned(),
        code: diagnostic
            .code
            .clone()
            .unwrap_or_else(|| "paste.lowering.failed".to_owned()),
        message: diagnostic.message.clone(),
        path: None,
        target: Some(target.clone()),
        expected_revision: None,
        actual_revision: None,
        duplicates: None,
        nodes: None,
        edges: None,
        interface_policy: None,
        interface_detail: None,
    }
}

fn operation_diagnostic_to_runtime_diagnostic(
    diagnostic: RuntimeOperationDiagnostic,
) -> RuntimeDiagnostic {
    let details = serde_json::json!({
        "path": diagnostic.path,
        "target": diagnostic.target,
        "expectedRevision": diagnostic.expected_revision,
        "actualRevision": diagnostic.actual_revision,
        "duplicates": diagnostic.duplicates,
        "nodes": diagnostic.nodes,
        "edges": diagnostic.edges,
    });
    match diagnostic.severity.as_str() {
        "warning" => {
            RuntimeDiagnostic::structured_warning(diagnostic.code, diagnostic.message, details)
        }
        "info" => RuntimeDiagnostic {
            severity: crate::DiagnosticSeverity::Info,
            message: diagnostic.message,
            code: Some(diagnostic.code),
            details: Some(details),
        },
        _ => RuntimeDiagnostic::structured_error(diagnostic.code, diagnostic.message, details),
    }
}

fn apply_view_patch_to_view_state(
    graph: &GraphDocument,
    mut view_state: ViewState,
    patch: &RuntimeViewPatch,
) -> Result<(ViewState, RuntimeViewPatch), Vec<RuntimeDiagnostic>> {
    let mut inverse_ops = Vec::new();
    for op in &patch.ops {
        match op {
            RuntimeViewPatchOperation::SetNodeView { node_id, view } => {
                if !graph.nodes.iter().any(|node| node.id == *node_id) {
                    return Err(vec![RuntimeDiagnostic::error(format!(
                        "view patch node {node_id} does not exist"
                    ))]);
                }
                let Some(previous) = view_state.canvas.nodes.get(node_id).cloned() else {
                    return Err(vec![RuntimeDiagnostic::error(format!(
                        "view patch node {node_id} has no view state"
                    ))]);
                };
                view_state
                    .canvas
                    .nodes
                    .insert(node_id.clone(), view.clone());
                inverse_ops.insert(
                    0,
                    RuntimeViewPatchOperation::SetNodeView {
                        node_id: node_id.clone(),
                        view: previous,
                    },
                );
            }
            RuntimeViewPatchOperation::MoveNodeView { node_id, from, to } => {
                if !graph.nodes.iter().any(|node| node.id == *node_id) {
                    return Err(vec![RuntimeDiagnostic::error(format!(
                        "view patch node {node_id} does not exist"
                    ))]);
                }
                let Some(previous) = view_state.canvas.nodes.get(node_id).cloned() else {
                    return Err(vec![RuntimeDiagnostic::error(format!(
                        "view patch node {node_id} has no view state"
                    ))]);
                };
                if let Some(from) = from
                    && from != &previous
                {
                    return Err(vec![RuntimeDiagnostic::error(format!(
                        "view patch node {node_id} from view does not match current view"
                    ))]);
                }
                view_state.canvas.nodes.insert(node_id.clone(), to.clone());
                inverse_ops.insert(
                    0,
                    RuntimeViewPatchOperation::MoveNodeView {
                        node_id: node_id.clone(),
                        from: Some(to.clone()),
                        to: previous,
                    },
                );
            }
        }
    }

    Ok((
        runtime_owned_view_state(view_state),
        RuntimeViewPatch {
            base_view_revision: patch.base_view_revision,
            ops: inverse_ops,
        },
    ))
}

fn unresolved_object_diagnostics(graph: &GraphDocument) -> Vec<RuntimeDiagnostic> {
    graph
        .nodes
        .iter()
        .filter(|node| node.kind == UNRESOLVED_OBJECT_NODE_KIND)
        .map(|node| {
            let object_text = node
                .params
                .get("objectText")
                .and_then(|value| value.as_str())
                .unwrap_or(node.id.as_str());
            let diagnostic_message = node
                .params
                .get("diagnosticMessage")
                .and_then(|value| value.as_str())
                .unwrap_or("object text could not be resolved");
            RuntimeDiagnostic::error(format!(
                "unresolved object {object_text}: {diagnostic_message}"
            ))
        })
        .collect()
}

fn unresolved_object_diagnostics_current(graph: &GraphDocumentCurrent) -> Vec<RuntimeDiagnostic> {
    graph
        .nodes
        .iter()
        .filter(|node| node.kind == UNRESOLVED_OBJECT_NODE_KIND)
        .map(|node| {
            let object_text = node
                .params
                .get("objectText")
                .and_then(|value| value.as_str())
                .unwrap_or(node.id.as_str());
            let diagnostic_message = node
                .params
                .get("diagnosticMessage")
                .and_then(|value| value.as_str())
                .unwrap_or("object text could not be resolved");
            RuntimeDiagnostic::error(format!(
                "unresolved object {object_text}: {diagnostic_message}"
            ))
        })
        .collect()
}

fn build_session_execution_plan(
    graph: &GraphDocument,
    registry: &NodeRegistry,
    surface: &'static str,
) -> Result<ExecutionPlan, Vec<RuntimeDiagnostic>> {
    build_execution_plan(graph, registry).map_err(|error| {
        let mut diagnostics = plan_error_diagnostics(error, surface, graph);
        diagnostics.extend(unresolved_object_diagnostics(graph));
        diagnostics
    })
}

fn plan_error_diagnostics(
    error: PlanError,
    surface: &'static str,
    graph: &GraphDocument,
) -> Vec<RuntimeDiagnostic> {
    let details = || {
        json!({
            "surface": surface,
            "graphId": graph.id,
            "graphRevision": graph.revision,
        })
    };
    match error {
        PlanError::InvalidProject(report) => report
            .errors()
            .iter()
            .map(|error| {
                RuntimeDiagnostic::structured_error(
                    "session.plan.invalid-project",
                    error.message.clone(),
                    details(),
                )
            })
            .collect(),
        PlanError::Cycle { nodes } => vec![RuntimeDiagnostic::structured_error(
            "session.plan.cycle",
            format!("cycle detected: {nodes}"),
            json!({
                "surface": surface,
                "graphId": graph.id,
                "graphRevision": graph.revision,
                "nodes": nodes,
            }),
        )],
    }
}

fn created_at_now() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("unix-ms:{millis}")
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};

    use serde_json::{Value, json};

    use crate::{
        ControlMessage, ControlValue, Edge, EdgeEndpointCurrent, EdgeSpecCurrent, GraphDocument,
        GraphDocumentCurrent, GraphPatch, NodeRegistry, PasteGraphFragmentRequest, PortRef,
        PortSpecCurrent, ProjectRequestCurrent, RuntimeCollaborationChange, RuntimeControlEmission,
        RuntimeControlEventRequest, RuntimeControlReadRequest, RuntimeControlReadTarget,
        RuntimeDiagnostic, RuntimeOperationDiagnostic, RuntimeOperationEnvelope, ViewState,
    };

    use super::{
        HistoryEntry, RuntimeHistoryEntryKind, RuntimeMutationRequest, RuntimePatchResponse,
        RuntimeSession, RuntimeViewPatch, RuntimeViewPatchOperation, lower_fragment_view_patch,
        lower_port_for_execution, remap_edge, runtime_diagnostic_to_operation_diagnostic,
    };

    #[test]
    fn invalid_registry_load_returns_diagnostics_without_revision_change() {
        let mut session = RuntimeSession::default();
        let mut request = sample_project_current();
        request.nodes[0].schema_version = "9.9.9".to_owned();

        let response = session.load_project_current(request);

        assert!(!response.ok);
        assert!(!response.snapshot.loaded());
        assert_eq!(response.snapshot.session_revision, 0);
        assert!(!response.diagnostics.is_empty());
    }

    #[test]
    fn validate_and_plan_fail_without_loaded_project() {
        let mut session = RuntimeSession::default();

        let validation = session.validate_current();
        let plan = session.plan_current();
        let preview_control = session.preview_control_state_snapshot();

        assert!(!validation.ok);
        assert!(!plan.ok);
        assert!(preview_control.is_none());
        assert!(
            plan.diagnostics[0]
                .message
                .contains("no project loaded in runtime session")
        );
    }

    #[test]
    fn session_snapshot_derives_endpoint_binding_value_formats() {
        let mut session = RuntimeSession::default();

        let response = session.load_project_current(binding_project_current());

        assert!(response.ok);
        assert_eq!(response.snapshot.binding_formats.len(), 1);
        let binding = &response.snapshot.binding_formats[0];
        assert_eq!(binding.binding_id, "edge_value_target");
        assert_eq!(binding.binding_epoch, 1);
        assert_eq!(binding.format_revision, 1);
        assert_eq!(binding.format_digest.as_ref().map(String::len), Some(64));
        assert_eq!(binding.value_format.value_type_id, "value.core.float32");
        assert_eq!(binding.value_format.format.as_deref(), Some("f32"));
        assert_eq!(binding.source.as_ref().unwrap().node_id, "value_1");
        assert_eq!(binding.source.as_ref().unwrap().port_id, "value");
        assert_eq!(binding.target.as_ref().unwrap().node_id, "target_1");
        assert_eq!(binding.target.as_ref().unwrap().port_id, "cold");

        let snapshot_json =
            serde_json::to_value(&response.snapshot).expect("snapshot should serialize");
        assert!(snapshot_json.get("bindingFormats").is_some());
    }

    #[test]
    fn plan_current_reports_invalid_stored_project() {
        let mut session = RuntimeSession {
            graph: Some(sample_internal_graph()),
            registry: Some(NodeRegistry::new()),
            plan: None,
            diagnostics: Vec::new(),
            revision: 1,
            ..RuntimeSession::default()
        };

        let response = session.plan_current();

        assert!(!response.ok);
        assert!(response.snapshot.plan.is_none());
        assert!(
            response.diagnostics[0]
                .message
                .contains("no project loaded")
        );
    }

    #[test]
    fn validate_current_reports_invalid_stored_project() {
        let mut session = RuntimeSession {
            graph: Some(sample_internal_graph()),
            registry: None,
            plan: None,
            diagnostics: Vec::new(),
            revision: 1,
            ..RuntimeSession::default()
        };

        let response = session.validate_current();

        assert!(!response.ok);
        assert!(
            response.diagnostics[0]
                .message
                .contains("no project loaded")
        );
    }

    #[test]
    fn validate_current_reports_registry_validation_errors() {
        let mut session = RuntimeSession {
            graph: Some(sample_internal_graph()),
            registry: Some(NodeRegistry::new()),
            plan: None,
            diagnostics: Vec::new(),
            revision: 1,
            ..RuntimeSession::default()
        };

        let response = session.validate_current();

        assert!(!response.ok);
        assert!(
            response.diagnostics[0]
                .message
                .contains("no project loaded")
        );
    }

    #[test]
    fn run_current_rebuilds_missing_plan() {
        let mut session = RuntimeSession::default();
        let loaded = load_sample_project(&mut session);
        assert!(loaded.ok);
        session.plan = None;

        let response = session.run_current(2);

        assert!(response.ok);
        assert!(response.snapshot.plan.is_some());
        assert_eq!(response.report.unwrap().frame_count, 2);
    }

    #[test]
    fn run_current_returns_plan_failure_when_rebuild_fails() {
        let mut session = RuntimeSession {
            graph: Some(sample_internal_graph()),
            registry: Some(NodeRegistry::new()),
            plan: None,
            diagnostics: Vec::new(),
            revision: 1,
            ..RuntimeSession::default()
        };

        let response = session.run_current(2);

        assert!(!response.ok);
        assert!(response.report.is_none());
        assert!(
            response.diagnostics[0]
                .message
                .contains("no project loaded")
        );
    }

    #[test]
    fn control_event_fails_without_loaded_project() {
        let mut session = RuntimeSession::default();

        let response =
            session.apply_control_event(set_control_request("value_1", "in", f32_value(32.0)));

        assert!(!response.ok);
        assert!(response.emitted.is_empty());
        assert_eq!(session.snapshot().session_revision, 0);
        assert!(
            response.diagnostics[0]
                .message
                .contains("no project loaded")
        );
    }

    #[test]
    fn control_set_bang_and_in_follow_typed_value_semantics() {
        let mut session = RuntimeSession::default();
        assert!(load_sample_project(&mut session).ok);

        let set =
            session.apply_control_event(set_control_request("value_1", "in", f32_value(32.0)));
        assert!(set.ok);
        assert!(set.changed);
        assert!(set.emitted.is_empty());
        assert_eq!(session.snapshot().session_revision, 1);
        assert_eq!(session.snapshot().control_revision, 1);
        assert_eq!(session.control_revision(), 1);
        assert_eq!(set.control_revision, Some(1));
        assert_eq!(
            session.control_state_response().values.get("value_1"),
            Some(&ControlValue::float(32.0))
        );

        let same_set =
            session.apply_control_event(set_control_request("value_1", "in", f32_value(32.0)));
        assert!(same_set.ok);
        assert!(!same_set.changed);
        assert_eq!(same_set.control_revision, Some(1));

        let bang = session.apply_control_event(bang_control_request("value_1", "in"));
        assert!(bang.ok);
        assert_eq!(bang.emitted.len(), 1);
        assert_eq!(bang.emitted[0].node_id, "value_1");
        assert_eq!(bang.emitted[0].port_id, "value");
        assert_eq!(
            emitted_value(&bang.emitted[0]),
            Some(ControlValue::float(32.0))
        );
        assert!(bang.changed);
        assert_eq!(session.snapshot().session_revision, 1);
        assert_eq!(session.snapshot().control_revision, 2);
        assert_eq!(bang.control_revision, Some(2));

        let input = session.apply_control_event(control_request("value_1", "in", f32_value(12.0)));
        assert!(input.ok);
        assert!(input.changed);
        assert_eq!(
            emitted_value(&input.emitted[0]),
            Some(ControlValue::float(12.0))
        );
        assert_eq!(
            session.control_state_response().values.get("value_1"),
            Some(&ControlValue::float(12.0))
        );
        assert_eq!(
            session.control_state_response().values.get("target_1"),
            Some(&ControlValue::float(12.0))
        );
        assert_eq!(session.snapshot().session_revision, 1);
        assert_eq!(session.snapshot().control_revision, 3);
        assert_eq!(session.control_revision(), 3);
        assert_eq!(input.control_revision, Some(3));
    }

    #[test]
    fn control_object_send_name_updates_typed_channel_state() {
        let mut session = RuntimeSession::default();
        assert!(
            session
                .load_project_current(object_routing_project_current())
                .ok
        );

        let response =
            session.apply_control_event(control_request("value_1", "in", f32_value(1.5)));

        assert!(response.ok);
        assert_eq!(response.emitted[0].node_id, "value_1");
        assert_eq!(response.emitted[0].port_id, "value");
        assert_eq!(
            emitted_value(&response.emitted[0]),
            Some(ControlValue::float(1.5))
        );
        assert_eq!(
            session
                .control_state_response()
                .channels
                .get("value.core.float32:speed"),
            Some(&ControlMessage::from_value(ControlValue::float(1.5)))
        );
    }

    #[test]
    fn control_read_addresses_params_ports_and_state() {
        let mut session = RuntimeSession::default();
        let mut project = sample_project_current();
        project.graph.nodes[0]
            .params
            .insert("value".to_owned(), json!(0.0));
        assert!(session.load_project_current(project).ok);
        assert!(
            session
                .apply_control_event(set_control_request("value_1", "in", f32_value(32.0)))
                .ok
        );

        let param = session.read_control(control_read(
            "value_1",
            RuntimeControlReadTarget::Param,
            "value",
        ));
        assert!(param.ok);
        assert_eq!(
            param.value.unwrap(),
            json!({ "type": "json", "value": 0.0 })
        );

        let port = session.read_control(control_read(
            "value_1",
            RuntimeControlReadTarget::Port,
            "value",
        ));
        assert!(port.ok);
        assert_eq!(port.value.unwrap()["value"]["id"], json!("value"));

        let state = session.read_control(control_read(
            "value_1",
            RuntimeControlReadTarget::State,
            "value",
        ));
        assert!(state.ok);
        assert_eq!(
            state.value.unwrap(),
            json!({ "type": "float", "representation": "f32", "value": 32.0 })
        );
    }

    #[test]
    fn invalid_control_read_reports_diagnostics() {
        let mut session = RuntimeSession::default();
        let missing_session = session.read_control(control_read(
            "value_1",
            RuntimeControlReadTarget::State,
            "value",
        ));
        assert!(!missing_session.ok);
        assert!(
            missing_session.diagnostics[0]
                .message
                .contains("no project loaded")
        );

        assert!(load_sample_project(&mut session).ok);
        let missing_port = session.read_control(control_read(
            "value_1",
            RuntimeControlReadTarget::Port,
            "missing",
        ));
        assert!(!missing_port.ok);
        assert!(
            missing_port.diagnostics[0]
                .message
                .contains("port missing does not exist")
        );

        let missing_node = session.read_control(control_read(
            "missing",
            RuntimeControlReadTarget::State,
            "value",
        ));
        assert!(!missing_node.ok);
        assert!(
            missing_node.diagnostics[0]
                .message
                .contains("node missing does not exist")
        );

        let missing_param = session.read_control(control_read(
            "value_1",
            RuntimeControlReadTarget::Param,
            "missing",
        ));
        assert!(!missing_param.ok);
        assert!(
            missing_param.diagnostics[0]
                .message
                .contains("param missing does not exist")
        );

        let missing_state_id = session.read_control(control_read(
            "value_1",
            RuntimeControlReadTarget::State,
            "other",
        ));
        assert!(!missing_state_id.ok);
        assert!(
            missing_state_id.diagnostics[0]
                .message
                .contains("state other does not exist")
        );

        assert!(
            session
                .load_project_current(debug_sink_project_current())
                .ok
        );
        let missing_runtime_state = session.read_control(control_read(
            "debug_1",
            RuntimeControlReadTarget::State,
            "value",
        ));
        assert!(!missing_runtime_state.ok);
        assert!(
            missing_runtime_state.diagnostics[0]
                .message
                .contains("has no runtime control state")
        );
    }

    #[test]
    fn invalid_control_event_does_not_mutate_state_or_revision() {
        let mut session = RuntimeSession::default();
        assert!(load_sample_project(&mut session).ok);
        assert!(
            session
                .apply_control_event(set_control_request("value_1", "in", f32_value(32.0)))
                .ok
        );
        let before = session.snapshot();

        let response = session.apply_control_event(control_request(
            "value_1",
            "in",
            ControlValue::color([0.0, 0.0, 0.0, 1.0]),
        ));

        assert!(!response.ok);
        assert!(response.emitted.is_empty());
        assert_eq!(session.snapshot().session_revision, before.session_revision);
        assert_eq!(
            session.control_state_response().values.get("value_1"),
            Some(&ControlValue::float(32.0))
        );
    }

    #[test]
    fn failed_control_propagation_does_not_mutate_state_or_revision() {
        let mut session = RuntimeSession::default();
        assert!(load_sample_project(&mut session).ok);
        session.graph.as_mut().unwrap().edges = vec![Edge {
            from: PortRef {
                node: "value_1".to_owned(),
                port: "value".to_owned(),
            },
            to: PortRef {
                node: "target_1".to_owned(),
                port: "missing".to_owned(),
            },
        }];
        let before = session.snapshot();

        let response =
            session.apply_control_event(control_request("value_1", "in", f32_value(9.0)));

        assert!(!response.ok);
        assert!(response.diagnostics[0].message.contains("port missing"));
        assert_eq!(response.control_revision, Some(before.control_revision));
        assert_eq!(session.snapshot().control_revision, before.control_revision);
        assert_eq!(
            session.control_state_response().values.get("value_1"),
            Some(&ControlValue::float(0.0))
        );
        assert_eq!(
            session.control_state_response().values.get("target_1"),
            Some(&ControlValue::float(0.0))
        );
    }

    #[test]
    fn graph_patch_rebuilds_control_state_from_graph_params() {
        let mut session = RuntimeSession::default();
        assert!(load_sample_project(&mut session).ok);
        assert!(
            session
                .apply_control_event(set_control_request("value_1", "in", f32_value(32.0)))
                .ok
        );

        let response = session.apply_patch(set_value_patch("1", 0.75));

        assert_graph_patch_rejected(&response);
        assert_eq!(
            session.control_state_response().values.get("value_1"),
            Some(&ControlValue::float(32.0))
        );
    }

    #[test]
    fn preview_context_requires_loaded_project_and_plan() {
        let mut session = RuntimeSession::default();

        let missing_project = session.preview_context();
        assert!(
            missing_project
                .unwrap_err()
                .first()
                .unwrap()
                .message
                .contains("no project loaded")
        );

        load_sample_project(&mut session);
        let context = session.preview_context().expect("context should exist");
        assert_eq!(context.graph_id, "minimal-value");
        assert_eq!(context.graph_revision, "1");
        assert_eq!(context.session_revision, 1);
        assert_eq!(context.plan.graph_id, "minimal-value");
        assert_eq!(
            context.control_state.value_for_node("value_1"),
            Some(&ControlValue::float(0.0))
        );

        session.plan = None;
        let missing_plan = session.preview_context();
        assert!(
            missing_plan
                .unwrap_err()
                .first()
                .unwrap()
                .message
                .contains("no execution plan available")
        );
    }

    #[test]
    fn graph_and_view_state_accessors_return_loaded_project_copies() {
        let mut session = RuntimeSession::default();
        let loaded = load_sample_project(&mut session);

        assert!(loaded.ok);
        assert_eq!(session.graph().unwrap().id, "minimal-value");
        assert!(
            session
                .view_state()
                .unwrap()
                .canvas
                .nodes
                .contains_key("value_1")
        );
    }

    #[test]
    fn patch_without_loaded_session_returns_error() {
        let mut session = RuntimeSession::default();

        let response = session.apply_patch(set_value_patch("1", 0.75));

        assert!(!response.ok);
        assert!(!response.applied);
        assert!(!response.conflict);
        assert!(response.snapshot.project.is_none());
        assert!(!response.snapshot.loaded());
        assert!(
            response.diagnostics[0]
                .message
                .contains("no project loaded in runtime session")
        );
    }

    #[test]
    fn patch_with_matching_revision_applies_and_rebuilds_plan() {
        let mut session = RuntimeSession::default();
        let loaded = load_sample_project(&mut session);
        assert!(loaded.ok);

        let response = session.apply_patch(set_value_patch("1", 0.75));

        assert_graph_patch_rejected(&response);
        assert_eq!(response.history.entries.len(), 0);
        assert_eq!(response.history.undo_depth, 0);
        assert_eq!(response.history.redo_depth, 0);
        assert_eq!(patch_graph(&response).revision, "1");
        assert_eq!(response.snapshot.graph_revision(), Some("1"));
        assert_eq!(response.snapshot.session_revision, 1);
        assert_eq!(
            session.control_state.value_for_node("value_1"),
            Some(&ControlValue::float(0.0))
        );
    }

    #[test]
    fn unresolved_object_loads_session_with_error_diagnostic() {
        let mut session = RuntimeSession::default();

        let response = session.load_project_current(unresolved_project_current());

        assert!(response.ok);
        assert!(response.snapshot.loaded());
        assert!(response.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("unresolved object user.manipulator")
        }));
        assert_eq!(session.snapshot().diagnostics, response.diagnostics);

        let plan = session.plan_current();
        assert!(plan.ok);
        assert!(plan.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("unresolved object user.manipulator")
        }));
    }

    #[test]
    fn replace_node_with_unresolved_object_applies_with_error_diagnostic() {
        let mut session = RuntimeSession::default();
        let loaded =
            session.load_project_current(sample_project_current_with_unresolved_definition());
        assert!(loaded.ok);

        let response = session.apply_patch(graph_patch(json!({
          "schema": "skenion.graph.patch",
          "schemaVersion": "0.1.0",
          "id": "replace-target-unresolved",
          "baseRevision": "1",
          "ops": [
            {
              "op": "replaceNode",
              "nodeId": "target_1",
              "node": unresolved_node_json("target_1", "user.manipulator"),
              "edgePolicy": "removeInvalidEdges"
            }
          ]
        })));

        assert_graph_patch_rejected(&response);
        assert!(response.snapshot.loaded());
        assert_eq!(patch_graph(&response).revision, "1");
    }

    #[test]
    fn patch_with_wrong_base_revision_conflicts_without_mutating_session() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);

        let response = session.apply_patch(set_value_patch("0", 0.75));
        let snapshot = session.snapshot();

        assert_graph_patch_rejected(&response);
        assert!(latest_history_entry(&response).is_none());
        assert!((response.history.entries).is_empty());
        assert_eq!(patch_graph(&response).revision, "1");
        assert_eq!(snapshot.graph_revision(), Some("1"));
        assert_eq!(snapshot.session_revision, 1);
    }

    #[test]
    fn invalid_patch_operations_do_not_mutate_session() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);

        let duplicate = session.apply_patch(duplicate_edge_patch());
        let missing = session.apply_patch(missing_node_patch());
        let snapshot = session.snapshot();

        assert_graph_patch_rejected(&duplicate);
        assert!(latest_history_entry(&duplicate).is_none());
        assert!((duplicate.history.entries).is_empty());
        assert_graph_patch_rejected(&missing);
        assert_eq!(snapshot.graph_revision(), Some("1"));
        assert_eq!(snapshot.session_revision, 1);
    }

    #[test]
    fn incompatible_patch_result_does_not_mutate_session() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);

        let response = session.apply_patch(incompatible_edge_patch());
        let snapshot = session.snapshot();

        assert_graph_patch_rejected(&response);
        assert_eq!(snapshot.graph_revision(), Some("1"));
        assert_eq!(snapshot.session_revision, 1);
    }

    #[test]
    fn payload_identity_session_load_preserves_existing_project() {
        let mut session = RuntimeSession::default();
        let loaded = load_sample_project(&mut session);
        assert!(loaded.ok);

        let mut invalid = sample_project_current();
        invalid.graph.nodes[0].kind = "object.core.bool".to_owned();
        let response = session.load_project_current(invalid);
        let snapshot = session.snapshot();

        assert!(!response.ok);
        assert!(response.snapshot.loaded());
        assert_eq!(response.snapshot.graph_revision(), Some("1"));
        assert_eq!(
            response.snapshot.session_revision,
            loaded.snapshot.session_revision
        );
        assert!(
            response
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.as_deref() == Some("graph.payload-node-kind"))
        );
        assert_eq!(snapshot.graph_revision(), Some("1"));
        assert_eq!(snapshot.session_revision, loaded.snapshot.session_revision);
        assert!(
            snapshot
                .project
                .as_ref()
                .unwrap()
                .graph
                .nodes
                .iter()
                .all(|node| node.kind != "object.core.bool")
        );
    }

    #[test]
    fn registry_invalid_patch_result_does_not_mutate_session() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);

        let response = session.apply_patch(missing_definition_node_patch());
        let snapshot = session.snapshot();

        assert_graph_patch_rejected(&response);
        assert_eq!(snapshot.graph_revision(), Some("1"));
        assert_eq!(snapshot.session_revision, 1);
    }

    #[test]
    fn remove_node_patch_removes_incident_edges() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);

        let response = session.apply_patch(graph_patch(json!({
          "schema": "skenion.graph.patch",
          "schemaVersion": "0.1.0",
          "id": "remove-node",
          "baseRevision": "1",
          "ops": [
            { "op": "removeNode", "nodeId": "value_1" }
          ]
        })));

        assert_graph_patch_rejected(&response);
        let graph = patch_graph(&response);
        assert_eq!(graph.revision, "1");
        assert!(graph.nodes.iter().any(|node| node.id == "value_1"));
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn patch_non_numeric_revision_gets_suffix() {
        let mut project = sample_project_current();
        project.graph.revision = "rev_0001".to_owned();
        let mut session = RuntimeSession::default();
        session.load_project_current(project);

        let response = session.apply_patch(set_value_patch("rev_0001", 0.75));

        assert_graph_patch_rejected(&response);
        assert_eq!(patch_graph(&response).revision, "rev_0001");
    }

    #[test]
    fn history_starts_empty_and_undo_redo_empty_stack_returns_errors() {
        let mut session = RuntimeSession::default();

        let history = session.history();
        let undo = session.undo();
        let redo = session.redo();

        assert_eq!(history.schema, "skenion.runtime.history");
        assert!(!history.can_undo);
        assert!(!history.can_redo);
        assert!(!undo.ok);
        assert!(!undo.applied);
        assert!(latest_history_entry(&undo).is_none());
        assert!(undo.diagnostics[0].message.contains("available to undo"));
        assert!(!redo.ok);
        assert!(!redo.applied);
        assert!(latest_history_entry(&redo).is_none());
        assert!(redo.diagnostics[0].message.contains("available to redo"));
    }

    #[test]
    fn undo_after_patch_restores_graph_and_records_history_entry() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);
        let applied = session.apply_runtime_operation(paste_operation("1"));
        assert!(applied.ok);
        let apply_event_id = applied.history_entry_id.clone().unwrap();

        let undone = session.undo();

        assert!(undone.ok);
        assert!(undone.applied);
        assert_eq!(patch_graph(&undone).revision, "3");
        assert!(
            !patch_graph(&undone)
                .nodes
                .iter()
                .any(|node| node.id == "pasted_target")
        );
        assert_eq!(undone.snapshot.session_revision, 3);
        let undo_entry = latest_history_entry(&undone).unwrap();
        assert_eq!(undo_entry.kind, RuntimeHistoryEntryKind::Undo);
        assert_eq!(
            undo_entry.subject_event_id.as_deref(),
            Some(apply_event_id.as_str())
        );
        assert!(undo_entry.mutation.graph_patch.is_none());
        assert!(undo_entry.inverse_mutation.graph_patch.is_none());
        assert_eq!((undone.history.entries).len(), 2);
        assert_eq!(undone.history.undo_depth, 0);
        assert_eq!(undone.history.redo_depth, 1);
    }

    #[test]
    fn redo_after_undo_reapplies_graph_and_records_history_entry() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);
        session.apply_runtime_operation(paste_operation("1"));
        session.undo();

        let redone = session.redo();

        assert!(redone.ok);
        assert!(redone.applied);
        assert_eq!(patch_graph(&redone).revision, "4");
        assert!(
            patch_graph(&redone)
                .nodes
                .iter()
                .any(|node| node.id == "pasted_target")
        );
        assert_eq!(redone.snapshot.session_revision, 4);
        let redo_entry = latest_history_entry(&redone).unwrap();
        assert_eq!(redo_entry.kind, RuntimeHistoryEntryKind::Redo);
        assert!(redo_entry.mutation.graph_patch.is_none());
        assert!(redo_entry.inverse_mutation.graph_patch.is_none());
        assert_eq!((redone.history.entries).len(), 3);
        assert_eq!(redone.history.undo_depth, 1);
        assert_eq!(redone.history.redo_depth, 0);
    }

    #[test]
    fn view_state_patch_undo_redo_moves_once_from_start_to_end() {
        let mut session = RuntimeSession::default();
        let loaded = load_sample_project(&mut session);
        assert!(loaded.ok);
        let mut start = loaded
            .snapshot
            .view_state()
            .cloned()
            .expect("loaded view state");
        start.canvas.viewport = None;
        let mut moved = start.clone();
        moved.canvas.nodes.get_mut("value_1").unwrap().x += 240.0;
        moved.canvas.nodes.get_mut("value_1").unwrap().y += 120.0;

        let applied = session.apply_mutation(RuntimeMutationRequest {
            graph_patch: None,
            view_patch: Some(RuntimeViewPatch {
                base_view_revision: 1,
                ops: vec![RuntimeViewPatchOperation::MoveNodeView {
                    node_id: "value_1".to_owned(),
                    from: Some(start.canvas.nodes["value_1"].clone()),
                    to: moved.canvas.nodes["value_1"].clone(),
                }],
            }),
            actor_id: None,
            client_id: Some("studio-a".to_owned()),
            description: Some("drag value_1".to_owned()),
        });

        assert!(applied.ok);
        assert!(applied.applied);
        let apply_entry = latest_history_entry(&applied).unwrap();
        assert_eq!(apply_entry.kind, RuntimeHistoryEntryKind::Apply);
        assert!(apply_entry.mutation.graph_patch.is_none());
        assert!(apply_entry.mutation.view_patch.is_some());
        assert_eq!(applied.history.undo_depth, 1);
        assert_eq!(applied.snapshot.view_revision, 2);
        assert_eq!(patch_view_state(&applied), &moved);

        let undone = session.undo();

        assert!(undone.ok);
        assert!(undone.applied);
        let undo_entry = latest_history_entry(&undone).unwrap();
        assert_eq!(undo_entry.kind, RuntimeHistoryEntryKind::Undo);
        assert!(undo_entry.mutation.graph_patch.is_none());
        assert!(undo_entry.mutation.view_patch.is_some());
        assert_eq!(undone.history.undo_depth, 0);
        assert_eq!(undone.history.redo_depth, 1);
        assert_eq!(undone.snapshot.view_revision, 3);
        assert_eq!(patch_view_state(&undone), &start);

        let redone = session.redo();

        assert!(redone.ok);
        assert!(redone.applied);
        let redo_entry = latest_history_entry(&redone).unwrap();
        assert_eq!(redo_entry.kind, RuntimeHistoryEntryKind::Redo);
        assert!(redo_entry.mutation.graph_patch.is_none());
        assert!(redo_entry.mutation.view_patch.is_some());
        assert_eq!(redone.history.undo_depth, 1);
        assert_eq!(redone.history.redo_depth, 0);
        assert_eq!(redone.snapshot.view_revision, 4);
        assert_eq!(patch_view_state(&redone), &moved);
    }

    #[test]
    fn empty_and_conflicting_view_mutations_are_rejected_without_history() {
        let mut session = RuntimeSession::default();
        assert!(load_sample_project(&mut session).ok);

        let empty = session.apply_mutation(RuntimeMutationRequest {
            graph_patch: None,
            view_patch: None,
            actor_id: None,
            client_id: None,
            description: None,
        });
        let conflict = session.apply_mutation(RuntimeMutationRequest {
            graph_patch: None,
            view_patch: Some(RuntimeViewPatch {
                base_view_revision: 99,
                ops: Vec::new(),
            }),
            actor_id: None,
            client_id: None,
            description: None,
        });

        assert!(!empty.ok);
        assert!(!empty.applied);
        assert!(empty.diagnostics[0].message.contains("did not include"));
        assert!(!conflict.ok);
        assert!(conflict.conflict);
        assert!(conflict.diagnostics[0].message.contains("baseViewRevision"));
        assert_eq!(conflict.history.entries.len(), 0);
    }

    #[test]
    fn view_patch_set_node_view_success_errors_and_noop_paths() {
        let mut session = RuntimeSession::default();
        let loaded = load_sample_project(&mut session);
        assert!(loaded.ok);
        let start = loaded
            .snapshot
            .view_state()
            .cloned()
            .expect("loaded view state");
        let value_view = start.canvas.nodes["value_1"].clone();
        let mut moved_view = value_view.clone();
        moved_view.x += 80.0;

        let set = session.apply_mutation(RuntimeMutationRequest {
            graph_patch: None,
            view_patch: Some(RuntimeViewPatch {
                base_view_revision: 1,
                ops: vec![RuntimeViewPatchOperation::SetNodeView {
                    node_id: "value_1".to_owned(),
                    view: moved_view.clone(),
                }],
            }),
            actor_id: None,
            client_id: None,
            description: Some("set node view".to_owned()),
        });
        assert!(set.ok);
        assert!(set.applied);
        assert_eq!(patch_view_state(&set).canvas.nodes["value_1"], moved_view);

        let noop = session.apply_mutation(RuntimeMutationRequest {
            graph_patch: None,
            view_patch: Some(RuntimeViewPatch {
                base_view_revision: 2,
                ops: vec![RuntimeViewPatchOperation::SetNodeView {
                    node_id: "value_1".to_owned(),
                    view: moved_view.clone(),
                }],
            }),
            actor_id: None,
            client_id: None,
            description: None,
        });
        assert!(noop.ok);
        assert!(!noop.applied);

        let missing_node = session.apply_mutation(RuntimeMutationRequest {
            graph_patch: None,
            view_patch: Some(RuntimeViewPatch {
                base_view_revision: 2,
                ops: vec![RuntimeViewPatchOperation::SetNodeView {
                    node_id: "missing".to_owned(),
                    view: value_view.clone(),
                }],
            }),
            actor_id: None,
            client_id: None,
            description: None,
        });
        assert!(!missing_node.ok);
        assert!(
            missing_node.diagnostics[0]
                .message
                .contains("does not exist")
        );
    }

    #[test]
    fn view_mutation_on_invalid_stored_graph_returns_diagnostics_without_panic() {
        let mut session = RuntimeSession::default();
        let loaded = load_sample_project(&mut session);
        assert!(loaded.ok);
        assert!(loaded.snapshot.plan.is_some());
        let start = loaded
            .snapshot
            .view_state()
            .expect("loaded view state")
            .canvas
            .nodes["value_1"]
            .clone();
        let mut moved = start.clone();
        moved.x += 24.0;

        let mut invalid_graph = session.graph().expect("loaded graph");
        let target_node = invalid_graph
            .nodes
            .iter_mut()
            .find(|node| node.id == "target_1")
            .expect("sample graph should include target node");
        let cold_port = target_node
            .ports
            .iter_mut()
            .find(|port| port.id == "cold")
            .expect("sample target should include cold inlet");
        cold_port.data_type.data_kind = "value.core.bool".to_owned();
        session.graph = Some(invalid_graph);

        let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            session.apply_mutation(RuntimeMutationRequest {
                graph_patch: None,
                view_patch: Some(RuntimeViewPatch {
                    base_view_revision: loaded.snapshot.view_revision,
                    ops: vec![RuntimeViewPatchOperation::MoveNodeView {
                        node_id: "value_1".to_owned(),
                        from: Some(start),
                        to: moved,
                    }],
                }),
                actor_id: None,
                client_id: Some("studio-a".to_owned()),
                description: Some("drag invalid stored graph".to_owned()),
            })
        }))
        .expect("invalid stored graph should return diagnostics instead of panicking");

        assert!(!response.ok);
        assert!(!response.applied);
        assert!(!response.conflict);
        assert_eq!(
            response.snapshot.session_revision,
            loaded.snapshot.session_revision
        );
        assert_eq!(
            response.snapshot.view_revision,
            loaded.snapshot.view_revision
        );
        assert!(response.snapshot.plan.is_none());
        assert!(
            response.diagnostics.iter().any(|diagnostic| {
                diagnostic.code.as_deref() == Some("session.plan.invalid-project")
                    && diagnostic.message.contains(
                        "incompatible edge value_1:value value.core.float32 -> target_1:cold value.core.bool",
                    )
            })
        );
        assert!(
            response.snapshot.diagnostics.iter().any(|diagnostic| {
                diagnostic.message.contains(
                    "incompatible edge value_1:value value.core.float32 -> target_1:cold value.core.bool",
                )
            })
        );
        assert_eq!(response.history.entries.len(), 0);
    }

    #[test]
    fn view_patch_helper_reports_missing_view_and_from_mismatch() {
        let mut session = RuntimeSession::default();
        let loaded = load_sample_project(&mut session);
        assert!(loaded.ok);
        let graph = session.graph().expect("loaded graph");
        let mut view_state = loaded
            .snapshot
            .view_state()
            .cloned()
            .expect("loaded view state");
        let value_view = view_state.canvas.nodes["value_1"].clone();
        let mut moved_view = value_view.clone();
        moved_view.y += 80.0;

        view_state.canvas.nodes.remove("value_1");
        let missing_set_view = super::apply_view_patch_to_view_state(
            &graph,
            view_state.clone(),
            &RuntimeViewPatch {
                base_view_revision: 1,
                ops: vec![RuntimeViewPatchOperation::SetNodeView {
                    node_id: "value_1".to_owned(),
                    view: moved_view.clone(),
                }],
            },
        );
        let missing_move_view = super::apply_view_patch_to_view_state(
            &graph,
            view_state,
            &RuntimeViewPatch {
                base_view_revision: 1,
                ops: vec![RuntimeViewPatchOperation::MoveNodeView {
                    node_id: "value_1".to_owned(),
                    from: None,
                    to: moved_view.clone(),
                }],
            },
        );
        let missing_move_node = super::apply_view_patch_to_view_state(
            &graph,
            loaded.snapshot.view_state().cloned().unwrap(),
            &RuntimeViewPatch {
                base_view_revision: 1,
                ops: vec![RuntimeViewPatchOperation::MoveNodeView {
                    node_id: "missing".to_owned(),
                    from: None,
                    to: moved_view.clone(),
                }],
            },
        );

        let mut mismatched_from = value_view.clone();
        mismatched_from.x += 1.0;
        let from_mismatch = super::apply_view_patch_to_view_state(
            &graph,
            loaded.snapshot.view_state().cloned().unwrap(),
            &RuntimeViewPatch {
                base_view_revision: 1,
                ops: vec![RuntimeViewPatchOperation::MoveNodeView {
                    node_id: "value_1".to_owned(),
                    from: Some(mismatched_from),
                    to: moved_view,
                }],
            },
        );

        assert!(
            missing_set_view
                .unwrap_err()
                .first()
                .unwrap()
                .message
                .contains("has no view state")
        );
        assert!(
            missing_move_view
                .unwrap_err()
                .first()
                .unwrap()
                .message
                .contains("has no view state")
        );
        assert!(
            missing_move_node
                .unwrap_err()
                .first()
                .unwrap()
                .message
                .contains("does not exist")
        );
        assert!(
            from_mismatch
                .unwrap_err()
                .first()
                .unwrap()
                .message
                .contains("from view does not match")
        );
    }

    #[test]
    fn combined_graph_and_noop_view_mutation_keeps_view_revision_stable() {
        let mut session = RuntimeSession::default();
        let loaded = load_sample_project(&mut session);
        assert!(loaded.ok);
        let value_view = loaded.snapshot.view_state().unwrap().canvas.nodes["value_1"].clone();

        let response = session.apply_mutation(RuntimeMutationRequest {
            graph_patch: Some(set_value_patch("1", 0.5)),
            view_patch: Some(RuntimeViewPatch {
                base_view_revision: 1,
                ops: vec![RuntimeViewPatchOperation::SetNodeView {
                    node_id: "value_1".to_owned(),
                    view: value_view,
                }],
            }),
            actor_id: None,
            client_id: None,
            description: Some("set graph without moving view".to_owned()),
        });

        assert_graph_patch_rejected(&response);
        assert_eq!(response.snapshot.graph_revision(), Some("1"));
        assert_eq!(response.snapshot.view_revision, 1);
        assert!(latest_history_entry(&response).is_none());
    }

    #[test]
    fn new_patch_after_undo_clears_redo_stack() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);
        session.apply_runtime_operation(paste_operation("1"));
        let undone = session.undo();
        assert_eq!(undone.history.redo_depth, 1);

        let applied = session.apply_runtime_operation(paste_operation("3"));

        assert!(applied.ok);
        assert_eq!(applied.revision_after.as_deref(), Some("4"));
        let history = session.history();
        assert_eq!(history.entries.len(), 3);
        assert_eq!(history.undo_depth, 1);
        assert_eq!(history.redo_depth, 0);
        assert!(!history.can_redo);
    }

    #[test]
    fn graph_patch_remove_node_patch_is_rejected_without_history() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);
        let removed = session.apply_patch(graph_patch(json!({
          "schema": "skenion.graph.patch",
          "schemaVersion": "0.1.0",
          "id": "remove-node",
          "baseRevision": "1",
          "ops": [
            { "op": "removeNode", "nodeId": "value_1" }
          ]
        })));

        assert_graph_patch_rejected(&removed);
        assert!(
            patch_graph(&removed)
                .nodes
                .iter()
                .any(|node| node.id == "value_1")
        );
        assert_eq!(patch_graph(&removed).edges.len(), 1);
        assert_eq!(removed.history.undo_depth, 0);
    }

    #[test]
    fn graph_patch_connection_and_delete_patches_are_rejected_without_history() {
        let mut project = sample_project_current();
        project.graph.edges.clear();
        let mut session = RuntimeSession::default();
        assert!(session.load_project_current(project).ok);
        let connected = session.apply_patch(duplicate_edge_patch());
        assert_graph_patch_rejected(&connected);
        assert!(patch_graph(&connected).edges.is_empty());
        let deleted = session.apply_patch(graph_patch(json!({
          "schema": "skenion.graph.patch",
          "schemaVersion": "0.1.0",
          "id": "delete-target",
          "baseRevision": "2",
          "ops": [
            { "op": "removeNode", "nodeId": "target_1" }
          ]
        })));
        assert_graph_patch_rejected(&deleted);
        assert!(patch_graph(&deleted).edges.is_empty());
        assert_eq!(deleted.history.undo_depth, 0);
    }

    #[test]
    fn multiple_undo_operations_keep_advancing_revision() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);
        session.apply_runtime_operation(paste_operation("1"));
        session.apply_runtime_operation(paste_operation("2"));

        let first_undo = session.undo();
        let second_undo = session.undo();

        assert!(first_undo.ok);
        assert_eq!(patch_graph(&first_undo).revision, "4");
        assert!(
            !patch_graph(&first_undo)
                .nodes
                .iter()
                .any(|node| node.id == "pasted_target_2")
        );
        assert!(second_undo.ok);
        assert_eq!(patch_graph(&second_undo).revision, "5");
        assert!(
            !patch_graph(&second_undo)
                .nodes
                .iter()
                .any(|node| node.id == "pasted_target")
        );
        assert_eq!((second_undo.history.entries).len(), 4);
        assert_eq!(second_undo.history.redo_depth, 2);
    }

    #[test]
    fn failed_history_operations_do_not_mutate_stacks_or_session() {
        let mut no_loaded = RuntimeSession::default();
        no_loaded.undo_stack.push(HistoryEntry::Mutation {
            event_id: "event_bad".to_owned(),
            actor_id: None,
            mutation: graph_mutation(set_value_patch("1", 0.75)),
            inverse_mutation: graph_mutation(set_value_patch("1", 0.5)),
        });
        let no_loaded_response = no_loaded.undo();
        assert!(!no_loaded_response.ok);
        assert_eq!(no_loaded_response.history.undo_depth, 1);
        assert!(
            no_loaded_response.diagnostics[0]
                .message
                .contains("no project loaded")
        );

        let mut invalid_inverse = RuntimeSession::default();
        load_sample_project(&mut invalid_inverse);
        invalid_inverse.undo_stack.push(HistoryEntry::Mutation {
            event_id: "event_bad_inverse".to_owned(),
            actor_id: None,
            mutation: graph_mutation(set_value_patch("1", 0.75)),
            inverse_mutation: graph_mutation(missing_node_patch()),
        });
        let invalid_inverse_response = invalid_inverse.undo();
        assert!(!invalid_inverse_response.ok);
        assert_eq!(invalid_inverse_response.history.undo_depth, 1);
        assert_eq!(
            invalid_inverse_response.snapshot.graph_revision(),
            Some("1")
        );

        let mut invalid_redo = RuntimeSession::default();
        load_sample_project(&mut invalid_redo);
        invalid_redo.redo_stack.push(HistoryEntry::Mutation {
            event_id: "event_bad_redo".to_owned(),
            actor_id: None,
            mutation: graph_mutation(missing_definition_node_patch()),
            inverse_mutation: graph_mutation(set_value_patch("1", 0.5)),
        });
        let invalid_redo_response = invalid_redo.redo();
        assert!(!invalid_redo_response.ok);
        assert_eq!(invalid_redo_response.history.redo_depth, 1);
        assert_eq!(
            invalid_redo_response.diagnostics[0].code.as_deref(),
            Some("project.graph-patch-unsupported")
        );

        let mut no_actor_history = RuntimeSession::default();
        load_sample_project(&mut no_actor_history);
        let no_actor_undo = no_actor_history.undo_for_actor("participant-a");
        let no_actor_redo = no_actor_history.redo_for_actor("participant-a");
        assert!(!no_actor_undo.ok);
        assert!(no_actor_undo.diagnostics[0].message.contains("actor"));
        assert!(!no_actor_redo.ok);
        assert!(no_actor_redo.diagnostics[0].message.contains("actor"));

        let mut invalid_actor_inverse = RuntimeSession::default();
        load_sample_project(&mut invalid_actor_inverse);
        invalid_actor_inverse
            .undo_stack
            .push(HistoryEntry::Mutation {
                event_id: "event_bad_actor_inverse".to_owned(),
                actor_id: Some("participant-a".to_owned()),
                mutation: graph_mutation(set_value_patch("1", 0.75)),
                inverse_mutation: graph_mutation(missing_node_patch()),
            });
        let invalid_actor_inverse_response = invalid_actor_inverse.undo_for_actor("participant-a");
        assert!(!invalid_actor_inverse_response.ok);
        assert_eq!(invalid_actor_inverse_response.history.undo_depth, 1);

        let mut invalid_actor_redo = RuntimeSession::default();
        load_sample_project(&mut invalid_actor_redo);
        invalid_actor_redo.redo_stack.push(HistoryEntry::Mutation {
            event_id: "event_bad_actor_redo".to_owned(),
            actor_id: Some("participant-a".to_owned()),
            mutation: graph_mutation(missing_definition_node_patch()),
            inverse_mutation: graph_mutation(set_value_patch("1", 0.5)),
        });
        let invalid_actor_redo_response = invalid_actor_redo.redo_for_actor("participant-a");
        assert!(!invalid_actor_redo_response.ok);
        assert_eq!(invalid_actor_redo_response.history.redo_depth, 1);
    }

    #[test]
    fn reject_patch_uses_current_session_snapshot() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);

        let response = session.reject_patch(
            false,
            vec![RuntimeDiagnostic::error(
                "invalid graph patch: unsupported op",
            )],
        );

        assert!(!response.ok);
        assert!(!response.applied);
        assert!(latest_history_entry(&response).is_none());
        assert_eq!((response.history.entries).len(), 0);
        assert_eq!(patch_graph(&response).revision, "1");
        assert_eq!(response.snapshot.graph_revision(), Some("1"));
        assert!(response.diagnostics[0].message.contains("unsupported op"));
    }

    #[test]
    fn paste_graph_fragment_lowers_to_root_graph_mutation_with_id_remap() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);

        let response = session.apply_runtime_operation(paste_operation("1"));

        assert!(response.ok);
        assert!(response.applied);
        assert!(!response.conflict);
        assert_eq!(response.revision_before, "1");
        assert_eq!(response.revision_after.as_deref(), Some("2"));
        assert_eq!(
            response
                .id_remap
                .node_id_map
                .get("value_1")
                .map(String::as_str),
            Some("value_1_2")
        );
        assert_eq!(
            response
                .id_remap
                .edge_id_map
                .get("edge_value_to_pasted")
                .map(String::as_str),
            Some("edge_value_to_pasted")
        );
        let graph = session.graph().expect("graph should remain loaded");
        assert!(graph.nodes.iter().any(|node| node.id == "value_1_2"));
        assert!(graph.nodes.iter().any(|node| node.id == "pasted_target"));
        assert!(graph.edges.iter().any(|edge| {
            edge.from.node == "value_1_2"
                && edge.from.port == "value"
                && edge.to.node == "pasted_target"
                && edge.to.port == "cold"
        }));
        assert_eq!(
            response.history_entry_id.as_deref(),
            Some("runtime_event_000001")
        );
    }

    #[test]
    fn payload_identity_paste_is_rejected_without_mutating_session() {
        let mut session = RuntimeSession::default();
        let loaded = load_sample_project(&mut session);
        assert!(loaded.ok);
        let mut operation = paste_operation("1");
        operation.request.fragment.nodes[0].kind = "object.core.string".to_owned();

        let response = session.apply_runtime_operation(operation);
        let snapshot = session.snapshot();

        assert!(!response.ok);
        assert!(!response.applied);
        assert_eq!(
            response.diagnostics[0].code,
            "paste.fragment.payload-node-kind"
        );
        assert_eq!(response.revision_after, None);
        assert_eq!(snapshot.session_revision, loaded.snapshot.session_revision);
        assert_eq!(snapshot.graph_revision(), Some("1"));
        assert!(session.history().entries.is_empty());
        let graph = session.graph().expect("graph should remain loaded");
        assert!(!graph.nodes.iter().any(|node| node.id == "pasted_target"));
    }

    #[test]
    fn paste_graph_fragment_remaps_past_existing_generated_ids() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);
        let first = session.apply_runtime_operation(paste_operation("1"));
        assert!(first.ok);

        let second = session.apply_runtime_operation(paste_operation("2"));

        assert!(second.ok);
        assert_eq!(
            second
                .id_remap
                .node_id_map
                .get("value_1")
                .map(String::as_str),
            Some("value_1_3")
        );
    }

    #[test]
    fn paste_graph_fragment_uses_default_descriptions_without_attribution() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);
        let mut operation = paste_operation("1");
        operation.attribution = None;

        let response = session.apply_runtime_operation(operation);

        assert!(response.ok);
        let mut history = session.history();
        let entry = history.entries.pop().expect("history should exist");
        assert_eq!(
            entry.description.as_deref(),
            Some("Paste graph fragment op-paste")
        );
        assert!(entry.mutation.graph_patch.is_none());
    }

    #[test]
    fn paste_graph_fragment_reports_no_loaded_project() {
        let mut session = RuntimeSession::default();

        let response = session.apply_runtime_operation(paste_operation("1"));

        assert!(!response.ok);
        assert!(!response.applied);
        assert_eq!(response.revision_before, "1");
        assert_eq!(response.diagnostics[0].code, "paste.target.no-project");
    }

    #[test]
    fn paste_graph_fragment_rejects_invalid_operation_envelope() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);
        let mut operation = paste_operation("1");
        operation.kind = "loadProject".to_owned();

        let response = session.apply_runtime_operation(operation);

        assert!(!response.ok);
        assert!(!response.applied);
        assert_eq!(
            response.diagnostics[0].code,
            "paste.operation.invalid-envelope"
        );
        assert!(
            response.diagnostics[0]
                .message
                .contains("unsupported runtime operation kind")
        );
    }

    #[test]
    fn paste_graph_fragment_rejects_id_conflicts_when_requested() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);
        let mut operation = paste_operation("1");
        operation.request.options = Some(skenion_contracts::PasteGraphFragmentOptions {
            outside_endpoint_policy: None,
            id_conflict_policy: Some(skenion_contracts::IdConflictPolicy::Reject),
            interface_incident_edge_policy: None,
            preserve_relative_positions: None,
        });

        let response = session.apply_runtime_operation(operation);

        assert!(!response.ok);
        assert!(!response.applied);
        assert_eq!(response.diagnostics[0].code, "paste.id-conflict");
        assert_eq!(
            response.diagnostics[0].duplicates.as_deref(),
            Some(&["value_1".to_owned()][..])
        );
        assert_eq!(
            response
                .id_remap
                .node_id_map
                .get("value_1")
                .map(String::as_str),
            Some("value_1")
        );
    }

    #[test]
    fn paste_graph_fragment_rejects_unsupported_interface_incident_edge_policy() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);
        let mut operation = paste_operation("1");
        operation.request.options = Some(skenion_contracts::PasteGraphFragmentOptions {
            outside_endpoint_policy: None,
            id_conflict_policy: Some(skenion_contracts::IdConflictPolicy::Remap),
            interface_incident_edge_policy: Some(
                skenion_contracts::InterfaceIncidentEdgePolicyV01::Reject,
            ),
            preserve_relative_positions: None,
        });

        let response = session.apply_runtime_operation(operation);

        assert!(!response.ok);
        assert!(!response.applied);
        assert_eq!(response.revision_after, None);
        assert!(response.id_remap.node_id_map.is_empty());
        assert_eq!(
            response.diagnostics[0].code,
            "paste.options.unsupported-interface-incident-edge-policy"
        );
        assert_eq!(
            response.diagnostics[0].path.as_deref(),
            Some("request.options.interfaceIncidentEdgePolicy")
        );
        assert_eq!(
            response.diagnostics[0].interface_policy,
            Some(skenion_contracts::InterfaceIncidentEdgePolicyV01::Reject)
        );
        assert_eq!(session.snapshot().graph_revision(), Some("1"));
    }

    #[test]
    fn paste_graph_fragment_reports_apply_mutation_failures_as_operation_diagnostics() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);
        let mut operation = paste_operation("1");
        operation.request.fragment.nodes[1].kind = "missing.kind".to_owned();

        let response = session.apply_runtime_operation(operation);

        assert!(!response.ok);
        assert!(!response.applied);
        assert_eq!(response.history_entry_id, None);
        assert_eq!(response.revision_after, None);
        assert_eq!(response.diagnostics[0].code, "node-definition.missing");
        assert!(
            response.diagnostics[0]
                .message
                .contains("missing node definition")
        );
    }

    #[test]
    fn paste_operation_validation_reports_fragment_analysis_errors() {
        let mut session = RuntimeSession::default();
        session.load_project_current(sample_project_current());
        let mut operation = paste_operation("1");
        operation.request.fragment.nodes[1].ports[1].id = "renamed".to_owned();

        let response = session.apply_runtime_operation(operation);

        assert!(!response.ok);
        assert!(!response.applied);
        assert_eq!(
            response.diagnostics[0].code,
            "paste.operation.invalid-envelope"
        );
        assert!(
            response.diagnostics[0]
                .message
                .contains("missing-target-port")
        );
        assert!(response.id_remap.node_id_map.is_empty());
    }

    #[test]
    fn paste_lowering_skips_fragment_view_entries_missing_from_view_map() {
        let mut operation = paste_operation("1");
        operation
            .request
            .fragment
            .view
            .as_mut()
            .expect("fragment should include view")
            .nodes
            .as_mut()
            .expect("fragment view should include nodes")
            .remove("pasted_target");
        let node_id_map = [("pasted_target".to_owned(), "pasted_target".to_owned())]
            .into_iter()
            .collect();

        let patch = lower_fragment_view_patch(1, &operation.request, &node_id_map);

        assert!(patch.is_none());
    }

    #[test]
    fn paste_lowering_handles_absent_fragment_view_and_unmapped_edge_endpoints() {
        let mut operation = paste_operation("1");
        operation.request.fragment.view = None;

        let patch = lower_fragment_view_patch(1, &operation.request, &BTreeMap::new());

        assert!(patch.is_none());

        operation.request.fragment.view =
            Some(skenion_contracts::GraphFragmentViewV01 { nodes: None });
        let patch = lower_fragment_view_patch(1, &operation.request, &BTreeMap::new());
        assert!(patch.is_none());

        let edge = EdgeSpecCurrent {
            id: "edge".to_owned(),
            source: EdgeEndpointCurrent {
                node_id: "outside_source".to_owned(),
                port_id: "out".to_owned(),
            },
            target: EdgeEndpointCurrent {
                node_id: "outside_target".to_owned(),
                port_id: "in".to_owned(),
            },
            resolved_type: None,
            order: None,
            enabled: None,
            adapter: None,
            feedback: None,
            style_override: None,
            label: None,
            description: None,
        };

        let remapped = remap_edge(&edge, &BTreeMap::new());

        assert_eq!(remapped.from.node, "outside_source");
        assert_eq!(remapped.to.node, "outside_target");
    }

    #[test]
    fn runtime_diagnostic_conversion_preserves_severity_and_code_defaults() {
        let target = skenion_contracts::GraphTargetRef {
            path: skenion_contracts::PatchPath::Root,
            base_revision: "1".to_owned(),
            target_revision: None,
        };
        let error = RuntimeDiagnostic::error("plain error");
        let warning = RuntimeDiagnostic {
            severity: crate::DiagnosticSeverity::Warning,
            message: "coded warning".to_owned(),
            code: Some("runtime.warning".to_owned()),
            details: None,
        };
        let info = RuntimeDiagnostic {
            severity: crate::DiagnosticSeverity::Info,
            message: "info".to_owned(),
            code: Some("runtime.info".to_owned()),
            details: None,
        };

        let converted_error = runtime_diagnostic_to_operation_diagnostic(&error, &target);
        let converted_warning = runtime_diagnostic_to_operation_diagnostic(&warning, &target);
        let converted_info = runtime_diagnostic_to_operation_diagnostic(&info, &target);

        assert_eq!(converted_error.severity, "error");
        assert_eq!(converted_error.code, "paste.lowering.failed");
        assert_eq!(converted_warning.severity, "warning");
        assert_eq!(converted_warning.code, "runtime.warning");
        assert_eq!(converted_info.severity, "info");
        assert_eq!(converted_info.code, "runtime.info");

        let warning =
            super::operation_diagnostic_to_runtime_diagnostic(RuntimeOperationDiagnostic {
                severity: "warning".to_owned(),
                code: "paste.warning".to_owned(),
                message: "warning".to_owned(),
                path: None,
                target: Some(target.clone()),
                expected_revision: Some("1".to_owned()),
                actual_revision: Some("2".to_owned()),
                duplicates: None,
                nodes: None,
                edges: None,
                interface_policy: None,
                interface_detail: None,
            });
        let info = super::operation_diagnostic_to_runtime_diagnostic(RuntimeOperationDiagnostic {
            severity: "info".to_owned(),
            code: "paste.info".to_owned(),
            message: "info".to_owned(),
            path: None,
            target: Some(target),
            expected_revision: None,
            actual_revision: None,
            duplicates: None,
            nodes: None,
            edges: None,
            interface_policy: None,
            interface_detail: None,
        });
        assert_eq!(warning.severity, crate::DiagnosticSeverity::Warning);
        assert_eq!(warning.code.as_deref(), Some("paste.warning"));
        assert_eq!(info.severity, crate::DiagnosticSeverity::Info);
        assert_eq!(info.code.as_deref(), Some("paste.info"));
    }

    #[test]
    fn rejected_collaboration_edge_connect_preserves_session_graph() {
        let mut session = RuntimeSession::default();
        let loaded = load_sample_project(&mut session);
        assert!(loaded.ok);
        let target = paste_operation("1").request.target;

        let response = session.apply_collaboration_change_set_current(
            target,
            vec![collaboration_change(json!({
              "op": "edge.connect",
              "changeId": "connect-output-to-output",
              "edge": {
                "id": "edge_invalid_direction",
                "source": { "nodeId": "value_1", "portId": "value" },
                "target": { "nodeId": "target_1", "portId": "value" }
              }
            }))],
            None,
            None,
            None,
        );
        let snapshot = session.snapshot();

        assert!(!response.ok);
        assert!(!response.applied);
        assert!(
            response
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.as_deref() == Some("graph.edge-target-direction"))
        );
        assert_eq!(snapshot.session_revision, loaded.snapshot.session_revision);
        assert_eq!(snapshot.graph_revision(), Some("1"));
        assert!(session.history().entries.is_empty());
        let graph = session.graph().expect("graph should remain loaded");
        assert!(graph.edges.iter().all(|edge| {
            !(edge.from.node == "value_1"
                && edge.from.port == "value"
                && edge.to.node == "target_1"
                && edge.to.port == "value")
        }));
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn rejected_payload_identity_collaboration_node_add_preserves_session_graph() {
        let mut session = RuntimeSession::default();
        let loaded = load_sample_project(&mut session);
        assert!(loaded.ok);
        let target = paste_operation("1").request.target;

        let response = session.apply_collaboration_change_set_current(
            target,
            vec![collaboration_change(json!({
              "op": "node.add",
              "changeId": "add-payload-identity",
              "node": {
                "id": "payload_identity",
                "kind": "string",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": []
              }
            }))],
            None,
            None,
            None,
        );
        let snapshot = session.snapshot();

        assert!(!response.ok);
        assert!(!response.applied);
        assert!(
            response
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.as_deref() == Some("graph.payload-node-kind"))
        );
        assert_eq!(snapshot.session_revision, loaded.snapshot.session_revision);
        assert_eq!(snapshot.graph_revision(), Some("1"));
        assert!(session.history().entries.is_empty());
        assert!(
            session
                .graph()
                .unwrap()
                .nodes
                .iter()
                .all(|node| node.id != "payload_identity")
        );
    }

    #[test]
    fn current_active_cutover_private_helpers_cover_defensive_paths() {
        let root_target = paste_operation("1").request.target;
        let change: RuntimeCollaborationChange = serde_json::from_value(json!({
          "op": "node.add",
          "changeId": "change-add-duplicate-value",
          "node": {
            "id": "value_1",
            "kind": "object.core.float",
            "kindVersion": "0.1.0",
            "params": {},
            "ports": value_f32_ports_current_json()
          }
        }))
        .expect("collaboration change should parse");

        let mut unloaded = RuntimeSession::default();
        let no_project = unloaded.apply_collaboration_change_set_current(
            root_target.clone(),
            vec![change.clone()],
            None,
            None,
            None,
        );
        assert_eq!(
            no_project.diagnostics[0].code.as_deref(),
            Some("collaboration.target.no-project")
        );

        let mut invalid_request = sample_project_current();
        let mut invalid_document = super::project_document_from_request_current(&invalid_request);
        invalid_document.schema_version = "9.9.9".to_owned();
        invalid_request.document = Some(invalid_document);
        let invalid_document_response = unloaded.load_project_current(invalid_request);
        assert_eq!(
            invalid_document_response.diagnostics[0].code.as_deref(),
            Some("project.unsupported-schema-version")
        );
        assert_eq!(
            invalid_document_response.diagnostics[0]
                .details
                .as_ref()
                .unwrap()["surface"],
            "project"
        );
        assert_eq!(
            invalid_document_response.diagnostics[0]
                .details
                .as_ref()
                .unwrap()["expectedSchemaVersion"],
            "0.1.0"
        );
        assert_eq!(
            invalid_document_response.diagnostics[0]
                .details
                .as_ref()
                .unwrap()["receivedSchemaVersion"],
            "9.9.9"
        );

        let mut session = RuntimeSession::default();
        assert!(session.load_project_current(sample_project_current()).ok);
        assert!(session.project_document_current().is_some());
        assert_eq!(
            session.target_revision_current(&root_target).as_deref(),
            Some("1")
        );

        let mut stale_target = root_target.clone();
        stale_target.base_revision = "0".to_owned();
        let stale = session.apply_collaboration_change_set_current(
            stale_target,
            vec![change.clone()],
            None,
            None,
            None,
        );
        assert!(stale.conflict);
        assert_eq!(
            stale.diagnostics[0].code.as_deref(),
            Some("collaboration.revision-conflict")
        );

        let missing_help_target: skenion_contracts::GraphTargetRef =
            serde_json::from_value(json!({
              "path": { "kind": "help-working-copy", "workingCopyId": "missing-help" },
              "baseRevision": "1"
            }))
            .expect("target should parse");
        let missing_target = session.apply_collaboration_change_set_current(
            missing_help_target,
            vec![change.clone()],
            None,
            None,
            None,
        );
        assert_eq!(
            missing_target.diagnostics[0].code.as_deref(),
            Some("paste.target.missing-help-working-copy")
        );

        let duplicate = session.apply_collaboration_change_set_current(
            root_target.clone(),
            vec![change],
            None,
            None,
            None,
        );
        assert_eq!(
            duplicate.diagnostics[0].code.as_deref(),
            Some("collaboration.node-id-conflict")
        );

        let unsupported_target: skenion_contracts::GraphTargetRef = serde_json::from_value(json!({
          "path": {
            "kind": "package-patch-definition",
            "packageId": "pkg",
            "patchId": "help"
          },
          "baseRevision": "1"
        }))
        .expect("target should parse");
        let paste_error = super::paste_graph_fragment_into_project_current(
            super::project_document_from_request_current(&sample_project_current()),
            1,
            &PasteGraphFragmentRequest {
                target: unsupported_target,
                fragment: paste_operation("1").request.fragment,
                placement: None,
                options: None,
            },
        )
        .expect_err("package patch target should not be mutable");
        assert_eq!(paste_error.0[0].code, "paste.target.unsupported");

        let unresolved = super::unresolved_object_diagnostics(&GraphDocument {
            schema: "skenion.graph".to_owned(),
            schema_version: "0.1.0".to_owned(),
            id: "unresolved-defaults".to_owned(),
            revision: "1".to_owned(),
            nodes: vec![
                serde_json::from_value(json!({
                  "id": "missing_object",
                  "kind": "object.core.unresolved",
                  "kindVersion": "0.1.0",
                  "params": {},
                  "ports": []
                }))
                .expect("node should parse"),
            ],
            edges: Vec::new(),
        });
        assert!(
            unresolved[0]
                .message
                .contains("object text could not be resolved")
        );
    }

    #[test]
    fn active_current_failure_paths_cover_registry_restore_and_history_rejection() {
        let mut duplicate_request = sample_project_current();
        duplicate_request
            .nodes
            .push(duplicate_request.nodes[0].clone());
        let mut duplicate_session = RuntimeSession::default();
        let duplicate_load = duplicate_session.load_project_current(duplicate_request);
        assert!(!duplicate_load.ok);
        assert!(
            duplicate_load.diagnostics[0]
                .message
                .contains("duplicate node definition")
        );

        let mut invalid_request = sample_project_current();
        let mut invalid_document = super::project_document_from_request_current(&invalid_request);
        invalid_document.graph.nodes[0].kind = "missing.kind".to_owned();
        invalid_request.document = Some(invalid_document.clone());
        let mut invalid_session = RuntimeSession {
            project: Some(invalid_document),
            nodes_current: invalid_request.nodes,
            revision: 1,
            view_revision: 1,
            ..RuntimeSession::default()
        };

        let validation = invalid_session.validate_current();
        let plan = invalid_session.plan_current();
        assert!(!validation.ok);
        assert!(!plan.ok);
        assert!(plan.snapshot.plan.is_none());
        assert!(
            plan.diagnostics[0]
                .message
                .contains("missing node definition")
        );

        let mut update_session = RuntimeSession::default();
        assert!(
            update_session
                .load_project_current(sample_project_current())
                .ok
        );
        let before = update_session
            .project_document_current()
            .expect("project should load");
        let mut after = before.clone();
        after.graph.revision = "2".to_owned();
        after.revision = "2".to_owned();
        update_session
            .nodes_current
            .push(update_session.nodes_current[0].clone());
        let update = update_session.apply_project_document_update(
            before,
            after,
            1,
            described_runtime_mutation("apply described project document"),
            None,
        );
        assert!(!update.ok);
        assert!(
            update.diagnostics[0]
                .message
                .contains("duplicate node definition")
        );

        let mut restore_plan_session = RuntimeSession::default();
        assert!(
            restore_plan_session
                .load_project_current(sample_project_current())
                .ok
        );
        let mut invalid_restore = restore_plan_session
            .project_document_current()
            .expect("project should load");
        invalid_restore.graph.nodes[0].kind = "missing.kind".to_owned();
        let restored = restore_plan_session.restore_project_document_state(
            invalid_restore,
            1,
            RuntimeHistoryEntryKind::Undo,
            described_runtime_mutation("restore invalid project"),
            empty_runtime_mutation(),
            None,
        );
        assert!(!restored.ok);
        assert!(
            restored.diagnostics[0]
                .message
                .contains("missing node definition")
        );

        let mut restore_registry_session = RuntimeSession::default();
        assert!(
            restore_registry_session
                .load_project_current(sample_project_current())
                .ok
        );
        let restore_project = restore_registry_session
            .project_document_current()
            .expect("project should load");
        restore_registry_session
            .nodes_current
            .push(restore_registry_session.nodes_current[0].clone());
        let restored = restore_registry_session.restore_project_document_state(
            restore_project,
            1,
            RuntimeHistoryEntryKind::Redo,
            described_runtime_mutation("restore registry project"),
            empty_runtime_mutation(),
            None,
        );
        assert!(!restored.ok);
        assert!(
            restored.diagnostics[0]
                .message
                .contains("duplicate node definition")
        );

        let mut history_session = RuntimeSession::default();
        assert!(
            history_session
                .load_project_current(sample_project_current())
                .ok
        );
        let before = history_session
            .project_document_current()
            .expect("project should load");
        let mut after = before.clone();
        after.graph.nodes[0].kind = "missing.kind".to_owned();
        let entry = HistoryEntry::ProjectDocument {
            event_id: "event".to_owned(),
            actor_id: None,
            before: Box::new(before),
            after: Box::new(after),
            before_view_revision: 1,
            after_view_revision: 2,
            mutation: empty_runtime_mutation(),
            inverse_mutation: empty_runtime_mutation(),
        };

        let outcome = history_session.apply_history_entry(entry, super::HistoryDirection::Redo);
        assert!(!outcome.applied);

        let mut view_session = RuntimeSession::default();
        let loaded = view_session.load_project_current(sample_project_current());
        assert!(loaded.ok);
        let start = loaded
            .snapshot
            .view_state()
            .expect("current 0.1 load should include view state")
            .canvas
            .nodes["value_1"]
            .clone();
        let mut moved = start.clone();
        moved.x += 12.0;
        let view_patch = view_session.apply_mutation(RuntimeMutationRequest {
            graph_patch: None,
            view_patch: Some(RuntimeViewPatch {
                base_view_revision: 1,
                ops: vec![RuntimeViewPatchOperation::MoveNodeView {
                    node_id: "value_1".to_owned(),
                    from: Some(start),
                    to: moved,
                }],
            }),
            actor_id: None,
            client_id: None,
            description: Some("current 0.1 active view move".to_owned()),
        });
        assert!(view_patch.ok);
        assert!(view_patch.applied);

        let mut mutation = graph_mutation(set_value_patch("old", 1.0));
        mutation.view_patch = Some(RuntimeViewPatch {
            base_view_revision: 1,
            ops: Vec::new(),
        });
        super::normalize_mutation_base_revisions(&mut mutation, "graph-new".to_owned(), 9);
        assert_eq!(
            mutation
                .graph_patch
                .as_ref()
                .map(|patch| patch.base_revision.as_str()),
            Some("graph-new")
        );
        assert_eq!(
            mutation
                .view_patch
                .as_ref()
                .map(|patch| patch.base_view_revision),
            Some(9)
        );
    }

    #[test]
    fn history_delta_helpers_merge_non_top_project_patch_and_view_edits() {
        let mut before = super::project_document_from_request_current(&sample_project_current());
        before.patch_library = vec![
            patch_definition_current("identity"),
            patch_definition_current("before-only"),
        ];
        before.view_state.canvas.nodes.insert(
            "before_only_view".to_owned(),
            crate::CanvasNodeView {
                x: 11.0,
                y: 12.0,
                width: None,
                height: None,
                collapsed: None,
            },
        );

        let mut after = before.clone();
        after.graph.nodes.push(graph_node_current("root_added"));
        after.graph.revision = "2".to_owned();
        after.revision = "2".to_owned();
        after.view_state.canvas.nodes.insert(
            "root_added".to_owned(),
            crate::CanvasNodeView {
                x: 400.0,
                y: 96.0,
                width: None,
                height: None,
                collapsed: None,
            },
        );
        after.view_state.canvas.nodes.remove("before_only_view");
        after
            .patch_library
            .retain(|patch| patch.id != "before-only");
        after.patch_library[0]
            .graph
            .nodes
            .push(graph_node_current("patch_added"));
        after.patch_library[0].graph.revision = "2".to_owned();
        after.patch_library[0].revision = "2".to_owned();

        let mut current = after.clone();
        current
            .graph
            .nodes
            .push(graph_node_current("other_actor_root"));
        current
            .patch_library
            .push(patch_definition_current("current-only"));
        current
            .patch_library
            .push(patch_definition_current("before-only"));
        current.patch_library[0]
            .graph
            .nodes
            .push(graph_node_current("other_actor_patch"));
        current.view_state.canvas.nodes.insert(
            "other_actor_root".to_owned(),
            crate::CanvasNodeView {
                x: 800.0,
                y: 96.0,
                width: None,
                height: None,
                collapsed: None,
            },
        );

        let undone = super::project_document_history_delta(
            &current,
            &before,
            &after,
            super::HistoryDirection::Undo,
        );
        assert!(
            !undone
                .graph
                .nodes
                .iter()
                .any(|node| node.id == "root_added")
        );
        assert!(
            undone
                .graph
                .nodes
                .iter()
                .any(|node| node.id == "other_actor_root")
        );
        assert!(
            !undone.patch_library[0]
                .graph
                .nodes
                .iter()
                .any(|node| node.id == "patch_added")
        );
        assert!(
            undone.patch_library[0]
                .graph
                .nodes
                .iter()
                .any(|node| node.id == "other_actor_patch")
        );
        assert!(
            undone
                .view_state
                .canvas
                .nodes
                .contains_key("other_actor_root")
        );

        let redone = super::project_document_history_delta(
            &undone,
            &before,
            &after,
            super::HistoryDirection::Redo,
        );
        assert!(
            redone
                .graph
                .nodes
                .iter()
                .any(|node| node.id == "root_added")
        );
        assert!(
            redone
                .graph
                .nodes
                .iter()
                .any(|node| node.id == "other_actor_root")
        );
        assert!(
            redone.patch_library[0]
                .graph
                .nodes
                .iter()
                .any(|node| node.id == "patch_added")
        );

        let mut before_graph = before.graph.clone();
        before_graph.nodes.push(graph_node_current("before_only"));
        let mut after_graph = before_graph.clone();
        after_graph.nodes.retain(|node| node.id != "before_only");
        let mut current_graph = after_graph.clone();
        current_graph.nodes.push(graph_node_current("before_only"));
        current_graph
            .nodes
            .push(graph_node_current("not_in_before"));
        assert!(super::undo_graph_history_delta_current(
            &mut current_graph.clone(),
            &before_graph,
            &after_graph
        ));
        assert!(super::redo_graph_history_delta_current(
            &mut current_graph,
            &before_graph,
            &after_graph
        ));

        let _ = super::view_state_history_delta_current(
            &before.view_state,
            &before.view_state,
            &after.view_state,
            super::HistoryDirection::Undo,
        );
        let _ = super::view_state_history_delta_current(
            &before.view_state,
            &before.view_state,
            &after.view_state,
            super::HistoryDirection::Redo,
        );
    }

    #[test]
    fn active_lowering_helpers_cover_surface_ports_models_and_id_sanitizing() {
        let definition: crate::NodeDefinitionCurrent = serde_json::from_value(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": "core.matrix",
          "version": "0.1.0",
          "displayName": "Matrix",
          "category": "Test",
          "surface": { "palette": "cyan" },
          "ports": [
            { "id": "signal", "direction": "input", "type": "value.core.float32", "rate": "audio" },
            { "id": "resource", "direction": "input", "type": "resource.buffer", "rate": "resource" },
            { "id": "stream", "direction": "output", "type": "io.midi", "rate": "io" }
          ],
          "execution": { "model": "audio_block" },
          "state": { "persistent": true },
          "permissions": [],
          "capabilities": []
        }))
        .expect("current 0.1 definition should parse");

        let lowered = super::lower_node_definition_for_execution(&definition);
        assert_eq!(
            lowered
                .surface
                .as_ref()
                .and_then(|surface| surface.palette.as_deref()),
            Some("cyan")
        );
        assert_eq!(lowered.ports[0].data_type.flow, crate::DataFlow::Signal);
        assert_eq!(lowered.ports[1].data_type.flow, crate::DataFlow::Resource);
        assert_eq!(lowered.ports[2].data_type.flow, crate::DataFlow::Resource);

        let cases = [
            (
                crate::ExecutionModel::Event,
                skenion_contracts::ExecutionModelV01::Event,
            ),
            (
                crate::ExecutionModel::Control,
                skenion_contracts::ExecutionModelV01::Control,
            ),
            (
                crate::ExecutionModel::Frame,
                skenion_contracts::ExecutionModelV01::Frame,
            ),
            (
                crate::ExecutionModel::AudioBlock,
                skenion_contracts::ExecutionModelV01::AudioBlock,
            ),
            (
                crate::ExecutionModel::VideoFrame,
                skenion_contracts::ExecutionModelV01::VideoFrame,
            ),
            (
                crate::ExecutionModel::GpuPass,
                skenion_contracts::ExecutionModelV01::GpuPass,
            ),
            (
                crate::ExecutionModel::AsyncResource,
                skenion_contracts::ExecutionModelV01::AsyncResource,
            ),
            (
                crate::ExecutionModel::ScriptControl,
                skenion_contracts::ExecutionModelV01::ScriptControl,
            ),
            (
                crate::ExecutionModel::NativePlugin,
                skenion_contracts::ExecutionModelV01::NativePlugin,
            ),
        ];
        for (internal, active) in cases {
            assert_eq!(
                super::lower_execution_model_for_execution(&active),
                internal
            );
        }
    }

    #[test]
    fn paste_private_helpers_cover_fragment_and_edge_conflict_errors() {
        let graph = sample_project_current().graph;
        let mut invalid_fragment = paste_operation("1").request;
        invalid_fragment.fragment.edges[0].target.node_id = "outside".to_owned();
        let invalid =
            super::paste_graph_fragment_into_graph_current(graph.clone(), &invalid_fragment)
                .expect_err("outside endpoint should fail analysis");
        assert_eq!(
            invalid.0[0].code,
            "paste.fragment.fragment-edge-outside-selection"
        );

        let mut edge_conflict = paste_operation("1").request;
        edge_conflict.options = Some(skenion_contracts::PasteGraphFragmentOptions {
            outside_endpoint_policy: None,
            id_conflict_policy: Some(skenion_contracts::IdConflictPolicy::Reject),
            interface_incident_edge_policy: None,
            preserve_relative_positions: None,
        });
        edge_conflict.fragment.nodes[0].id = "new_value".to_owned();
        edge_conflict.fragment.nodes[1].id = "new_target".to_owned();
        edge_conflict.fragment.edges[0].id = "edge_value_target".to_owned();
        edge_conflict.fragment.edges[0].source.node_id = "new_value".to_owned();
        edge_conflict.fragment.edges[0].target.node_id = "new_target".to_owned();
        let edge_conflict = super::paste_graph_fragment_into_graph_current(graph, &edge_conflict)
            .expect_err("duplicate edge id should fail");
        assert_eq!(edge_conflict.0[0].code, "paste.edge-id-conflict");
        assert_eq!(edge_conflict.1.edge_id_map.get("edge_value_target"), None);

        let mut used_edges = HashSet::new();
        used_edges.insert("edge_2".to_owned());
        assert_eq!(super::next_available_edge_id("edge", &used_edges), "edge_3");

        let mut unsupported = paste_operation("1").request;
        unsupported.target.path = skenion_contracts::PatchPath::EmbeddedPatchInstance {
            owner_path: vec!["root".to_owned()],
            node_id: "subpatch".to_owned(),
        };
        let unsupported = super::paste_graph_fragment_into_project_current(
            super::project_document_from_request_current(&sample_project_current()),
            1,
            &unsupported,
        )
        .expect_err("embedded patch target should fail");
        assert_eq!(unsupported.0[0].code, "paste.target.unsupported");

        let mut missing_graph = paste_operation("1").request;
        missing_graph.target.path = skenion_contracts::PatchPath::ProjectPatchDefinition {
            patch_id: "missing".to_owned(),
        };
        let missing_graph = super::paste_graph_fragment_into_project_current(
            super::project_document_from_request_current(&sample_project_current()),
            1,
            &missing_graph,
        )
        .expect_err("missing project patch should fail");
        assert_eq!(missing_graph.0[0].code, "paste.target.missing-graph");

        let mut project_with_patch =
            super::project_document_from_request_current(&sample_project_current());
        project_with_patch
            .patch_library
            .push(patch_definition_current("identity"));
        let mut patch_paste = paste_operation("1").request;
        patch_paste.target.path = skenion_contracts::PatchPath::ProjectPatchDefinition {
            patch_id: "identity".to_owned(),
        };
        let (patched_project, _, _, revision_after) =
            super::paste_graph_fragment_into_project_current(project_with_patch, 1, &patch_paste)
                .expect("project patch paste should apply");
        assert_eq!(revision_after, "2");
        assert_eq!(patched_project.patch_library[0].revision, "2");

        let remapped = super::remap_edge_current(
            &EdgeSpecCurrent {
                id: "edge".to_owned(),
                source: EdgeEndpointCurrent {
                    node_id: "outside_source".to_owned(),
                    port_id: "out".to_owned(),
                },
                target: EdgeEndpointCurrent {
                    node_id: "outside_target".to_owned(),
                    port_id: "in".to_owned(),
                },
                resolved_type: None,
                order: None,
                enabled: None,
                adapter: None,
                feedback: None,
                style_override: None,
                label: None,
                description: None,
            },
            &BTreeMap::new(),
            "edge_2".to_owned(),
        );
        assert_eq!(remapped.source.node_id, "outside_source");
        assert_eq!(remapped.target.node_id, "outside_target");
        assert_eq!(super::next_graph_revision("2"), "3");
        assert_eq!(super::next_graph_revision("rev"), "rev+1");
    }

    #[test]
    fn collaboration_private_helpers_cover_patch_target_error_matrix() {
        let mut project = super::project_document_from_request_current(&sample_project_current());
        project
            .patch_library
            .push(patch_definition_current("identity"));
        let root_target = skenion_contracts::GraphTargetRef {
            path: skenion_contracts::PatchPath::Root,
            base_revision: "1".to_owned(),
            target_revision: None,
        };
        let patch_target = skenion_contracts::GraphTargetRef {
            path: skenion_contracts::PatchPath::ProjectPatchDefinition {
                patch_id: "identity".to_owned(),
            },
            base_revision: "1".to_owned(),
            target_revision: None,
        };

        let patch_view_error = super::apply_collaboration_changes_to_project_current(
            project.clone(),
            1,
            &patch_target,
            &[collaboration_change(json!({
              "op": "node.add",
              "changeId": "add-with-view",
              "node": value_node_current_json("patch_added"),
              "view": { "x": 1.0, "y": 2.0 }
            }))],
        )
        .expect_err("patch definition views are not active Runtime state");
        assert_eq!(
            patch_view_error[0].code.as_deref(),
            Some("collaboration.patch-view-unsupported")
        );

        let patch_add = super::apply_collaboration_changes_to_project_current(
            project.clone(),
            1,
            &patch_target,
            &[collaboration_change(json!({
              "op": "node.add",
              "changeId": "add-patch-node",
              "node": value_node_current_json("patch_added_without_view")
            }))],
        )
        .expect("patch definition node add without view should apply");
        assert_eq!(patch_add.0.patch_library[0].revision, "2");

        let patch_move_error = super::apply_collaboration_changes_to_project_current(
            project.clone(),
            1,
            &patch_target,
            &[collaboration_change(json!({
              "op": "node.move",
              "changeId": "move-patch-node",
              "nodeId": "patch_value",
              "to": { "x": 1.0, "y": 2.0 }
            }))],
        )
        .expect_err("patch definition move view should fail");
        assert_eq!(
            patch_move_error[0].code.as_deref(),
            Some("collaboration.patch-view-unsupported")
        );

        let missing_move = super::apply_collaboration_changes_to_project_current(
            project.clone(),
            1,
            &root_target,
            &[collaboration_change(json!({
              "op": "node.move",
              "changeId": "move-missing",
              "nodeId": "missing",
              "to": { "x": 1.0, "y": 2.0 }
            }))],
        )
        .expect_err("moving a missing node should fail");
        assert_eq!(
            missing_move[0].code.as_deref(),
            Some("collaboration.node-missing")
        );

        let view_conflict = super::apply_collaboration_changes_to_project_current(
            project.clone(),
            1,
            &root_target,
            &[collaboration_change(json!({
              "op": "node.move",
              "changeId": "move-conflict",
              "nodeId": "value_1",
              "from": { "x": -1.0, "y": -1.0 },
              "to": { "x": 1.0, "y": 2.0 }
            }))],
        )
        .expect_err("move from mismatch should fail");
        assert_eq!(
            view_conflict[0].code.as_deref(),
            Some("collaboration.view-conflict")
        );

        let missing_delete = super::apply_collaboration_changes_to_project_current(
            project.clone(),
            1,
            &root_target,
            &[collaboration_change(json!({
              "op": "node.delete",
              "changeId": "delete-missing",
              "nodeId": "missing"
            }))],
        )
        .expect_err("deleting a missing node should fail");
        assert_eq!(
            missing_delete[0].code.as_deref(),
            Some("collaboration.node-missing")
        );

        let duplicate_edge = super::apply_collaboration_changes_to_project_current(
            project.clone(),
            1,
            &root_target,
            &[collaboration_change(json!({
              "op": "edge.connect",
              "changeId": "connect-duplicate",
              "edge": {
                "id": "edge_value_target",
                "source": { "nodeId": "value_1", "portId": "value" },
                "target": { "nodeId": "target_1", "portId": "cold" }
              }
            }))],
        )
        .expect_err("duplicate edge id should fail");
        assert_eq!(
            duplicate_edge[0].code.as_deref(),
            Some("collaboration.edge-id-conflict")
        );

        let missing_graph_target = skenion_contracts::GraphTargetRef {
            path: skenion_contracts::PatchPath::HelpWorkingCopy {
                working_copy_id: "missing-help".to_owned(),
                source_package_id: None,
                source_patch_id: None,
            },
            base_revision: "1".to_owned(),
            target_revision: None,
        };
        let missing_graph = super::apply_collaboration_changes_to_project_current(
            project.clone(),
            1,
            &missing_graph_target,
            &[],
        )
        .expect_err("missing help graph should fail");
        assert_eq!(
            missing_graph[0].code.as_deref(),
            Some("collaboration.target.missing-graph")
        );

        let unsupported_target = skenion_contracts::GraphTargetRef {
            path: skenion_contracts::PatchPath::PackagePatchDefinition {
                package_id: "pkg".to_owned(),
                patch_id: "help".to_owned(),
                version: None,
            },
            base_revision: "1".to_owned(),
            target_revision: None,
        };
        let unsupported = super::apply_collaboration_changes_to_project_current(
            project,
            1,
            &unsupported_target,
            &[],
        )
        .expect_err("package patch target should fail");
        assert_eq!(
            unsupported[0].code.as_deref(),
            Some("collaboration.target.unsupported")
        );

        let mut unresolved_graph = sample_project_current().graph;
        unresolved_graph.nodes.push(
            serde_json::from_value(json!({
              "id": "unresolved_current",
              "kind": "object.core.unresolved",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": []
            }))
            .expect("unresolved current 0.1 node should parse"),
        );
        let unresolved = super::unresolved_object_diagnostics_current(&unresolved_graph);
        assert!(
            unresolved[0]
                .message
                .contains("object text could not be resolved")
        );
    }

    #[test]
    fn paste_graph_fragment_applies_position_and_anchor_placement() {
        let mut positioned = RuntimeSession::default();
        load_sample_project(&mut positioned);
        let mut position_operation = paste_operation("1");
        position_operation.request.placement =
            Some(skenion_contracts::PastePlacement::Position { x: 300.0, y: 400.0 });

        let position_response = positioned.apply_runtime_operation(position_operation);

        assert!(position_response.ok);
        let position_view = positioned.view_state().expect("view state should exist");
        let pasted_value_view = position_view
            .canvas
            .nodes
            .get("value_1_2")
            .expect("pasted value view should exist");
        let pasted_target_view = position_view
            .canvas
            .nodes
            .get("pasted_target")
            .expect("pasted target view should exist");
        assert_eq!((pasted_value_view.x, pasted_value_view.y), (300.0, 400.0));
        assert_eq!((pasted_target_view.x, pasted_target_view.y), (470.0, 400.0));

        let mut anchored = RuntimeSession::default();
        load_sample_project(&mut anchored);
        let mut anchor_operation = paste_operation("1");
        anchor_operation.request.placement = Some(skenion_contracts::PastePlacement::Anchor {
            node_id: "value_1".to_owned(),
            offset_x: Some(16.0),
            offset_y: Some(32.0),
        });

        let anchor_response = anchored.apply_runtime_operation(anchor_operation);

        assert!(anchor_response.ok);
        let anchor_view = anchored.view_state().expect("view state should exist");
        let pasted_value_view = anchor_view
            .canvas
            .nodes
            .get("value_1_2")
            .expect("pasted value view should exist");
        assert_eq!((pasted_value_view.x, pasted_value_view.y), (26.0, 52.0));
    }

    #[test]
    fn paste_graph_fragment_reports_base_revision_conflict() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);

        let response = session.apply_runtime_operation(paste_operation("0"));

        assert!(!response.ok);
        assert!(!response.applied);
        assert!(response.conflict);
        assert_eq!(response.revision_before, "1");
        assert_eq!(response.revision_after, None);
        assert_eq!(response.diagnostics[0].code, "paste.revision-conflict");
        assert_eq!(session.graph().unwrap().revision, "1");
    }

    #[test]
    fn paste_graph_fragment_rejects_missing_help_working_copy_target() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);
        let mut operation = paste_operation("1");
        operation.request.target.path = skenion_contracts::PatchPath::HelpWorkingCopy {
            working_copy_id: "missing-help-copy".to_owned(),
            source_package_id: Some("skenion.core".to_owned()),
            source_patch_id: Some("float-help".to_owned()),
        };

        let response = session.apply_runtime_operation(operation);

        assert!(!response.ok);
        assert!(!response.applied);
        assert_eq!(
            response.diagnostics[0].code,
            "paste.target.missing-help-working-copy"
        );
    }

    #[test]
    fn paste_graph_fragment_allows_loaded_help_working_copy_target() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);
        let mut operation = paste_operation("1");
        operation.request.target.path = skenion_contracts::PatchPath::HelpWorkingCopy {
            working_copy_id: "minimal-value".to_owned(),
            source_package_id: Some("skenion.core".to_owned()),
            source_patch_id: Some("float-help".to_owned()),
        };

        let response = session.apply_runtime_operation(operation);

        assert!(response.ok);
        assert!(response.applied);
        assert_eq!(response.revision_after.as_deref(), Some("2"));
        assert_eq!(
            response
                .id_remap
                .node_id_map
                .get("value_1")
                .map(String::as_str),
            Some("value_1_2")
        );
    }

    #[test]
    fn paste_graph_fragment_rejects_project_patch_definition_target() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);
        let mut operation = paste_operation("1");
        operation.request.target.path = skenion_contracts::PatchPath::ProjectPatchDefinition {
            patch_id: "identity".to_owned(),
        };

        let response = session.apply_runtime_operation(operation);

        assert!(!response.ok);
        assert!(!response.applied);
        assert_eq!(
            response.diagnostics[0].code,
            "paste.target.missing-project-patch-definition"
        );
        assert!(
            response.diagnostics[0]
                .message
                .contains("project patch definition identity is not loaded")
        );
    }

    #[test]
    fn paste_graph_fragment_rejects_embedded_patch_instance_target() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);
        let mut operation = paste_operation("1");
        operation.request.target.path = skenion_contracts::PatchPath::EmbeddedPatchInstance {
            owner_path: vec!["root".to_owned()],
            node_id: "subpatch_1".to_owned(),
        };

        let response = session.apply_runtime_operation(operation);

        assert!(!response.ok);
        assert!(!response.applied);
        assert_eq!(
            response.diagnostics[0].code,
            "paste.target.unsupported-embedded-patch-instance"
        );
    }

    #[test]
    fn paste_graph_fragment_rejects_outside_endpoint_by_default() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);
        let mut operation = paste_operation("1");
        operation.request.fragment.edges[0].target.node_id = "outside".to_owned();

        let response = session.apply_runtime_operation(operation);

        assert!(!response.ok);
        assert!(!response.applied);
        assert_eq!(
            response.diagnostics[0].code,
            "paste.operation.invalid-envelope"
        );
        assert!(
            response.diagnostics[0]
                .message
                .contains("fragment-edge-outside-selection")
        );
    }

    #[test]
    fn paste_graph_fragment_omits_outside_endpoint_when_requested() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);
        let mut operation = paste_operation("1");
        operation.request.fragment.edges[0].target.node_id = "outside".to_owned();
        operation.request.options = Some(skenion_contracts::PasteGraphFragmentOptions {
            outside_endpoint_policy: Some(
                skenion_contracts::GraphFragmentOutsideEndpointPolicyV01::Omit,
            ),
            id_conflict_policy: Some(skenion_contracts::IdConflictPolicy::Remap),
            interface_incident_edge_policy: None,
            preserve_relative_positions: Some(true),
        });

        let response = session.apply_runtime_operation(operation);

        assert!(response.ok);
        assert!(response.applied);
        assert_eq!(
            response.id_remap.omitted_edge_ids,
            vec!["edge_value_to_pasted"]
        );
        let graph = session.graph().unwrap();
        assert!(!graph.edges.iter().any(|edge| edge.to.node == "outside"));
    }

    #[test]
    fn paste_graph_fragment_rejects_immutable_help_source_target() {
        let mut session = RuntimeSession::default();
        load_sample_project(&mut session);
        let mut operation = paste_operation("1");
        operation.request.target.path = skenion_contracts::PatchPath::PackagePatchDefinition {
            package_id: "skenion.core".to_owned(),
            patch_id: "float-help".to_owned(),
            version: Some("0.37.0".to_owned()),
        };

        let response = session.apply_runtime_operation(operation);

        assert!(!response.ok);
        assert!(!response.applied);
        assert_eq!(
            response.diagnostics[0].code,
            "paste.target.immutable-help-source"
        );
    }

    #[test]
    fn paste_graph_fragment_converts_current_port_rates_for_lowered_execution_nodes() {
        let cases = [
            (
                json!({ "id": "event", "direction": "input", "type": "value.core.bang", "rate": "event", "triggerMode": "trigger" }),
                crate::DataFlow::Event,
                "value.core.bang",
                Some(crate::PortActivation::Trigger),
            ),
            (
                json!({ "id": "message", "direction": "input", "type": "value.core.message", "rate": "control", "triggerMode": "trigger" }),
                crate::DataFlow::Control,
                "value.core.message",
                Some(crate::PortActivation::Trigger),
            ),
            (
                json!({ "id": "audio", "direction": "output", "type": "value.core.float32", "rate": "audio" }),
                crate::DataFlow::Signal,
                "value.core.float32",
                None,
            ),
            (
                json!({ "id": "resource", "direction": "input", "type": "resource.buffer", "rate": "resource" }),
                crate::DataFlow::Resource,
                "resource.buffer",
                None,
            ),
            (
                json!({ "id": "io", "direction": "output", "type": "io.midi", "rate": "io" }),
                crate::DataFlow::Resource,
                "io.midi",
                None,
            ),
            (
                json!({ "id": "render", "direction": "input", "type": "value.core.float32", "rate": "render", "triggerMode": "passive" }),
                crate::DataFlow::Control,
                "value.core.float32",
                Some(crate::PortActivation::Latched),
            ),
            (
                json!({ "id": "gpu", "direction": "input", "type": "value.core.color", "rate": "gpu", "triggerMode": "latched" }),
                crate::DataFlow::Control,
                "value.core.color",
                Some(crate::PortActivation::Latched),
            ),
            (
                json!({ "id": "texture", "direction": "output", "type": "value.core.tensor", "rate": "gpu" }),
                crate::DataFlow::Resource,
                "value.core.tensor",
                None,
            ),
            (
                json!({ "id": "default", "direction": "input", "type": "value.core.message" }),
                crate::DataFlow::Control,
                "value.core.message",
                None,
            ),
        ];

        for (value, expected_flow, expected_kind, expected_activation) in cases {
            let port: PortSpecCurrent = serde_json::from_value(value).expect("port should parse");
            let lowered = lower_port_for_execution(&port);
            assert_eq!(lowered.data_type.flow, expected_flow);
            assert_eq!(lowered.data_type.data_kind, expected_kind);
            assert_eq!(lowered.activation, expected_activation);
        }
    }

    fn graph_patch(value: Value) -> GraphPatch {
        serde_json::from_value(value).expect("patch should parse")
    }

    fn patch_graph(response: &RuntimePatchResponse) -> &GraphDocumentCurrent {
        &response
            .snapshot
            .project
            .as_ref()
            .expect("patch response should include project")
            .graph
    }

    fn patch_view_state(response: &RuntimePatchResponse) -> &ViewState {
        &response
            .snapshot
            .project
            .as_ref()
            .expect("patch response should include project")
            .view_state
    }

    fn assert_graph_patch_rejected(response: &RuntimePatchResponse) {
        assert!(!response.ok);
        assert!(!response.applied);
        assert!(!response.conflict);
        assert_eq!(
            response.diagnostics[0].code.as_deref(),
            Some("project.graph-patch-unsupported")
        );
    }

    fn latest_history_entry(
        response: &RuntimePatchResponse,
    ) -> Option<&super::RuntimeHistoryEntry> {
        response.history.entries.last()
    }

    fn graph_mutation(patch: GraphPatch) -> RuntimeMutationRequest {
        RuntimeMutationRequest {
            graph_patch: Some(patch),
            view_patch: None,
            actor_id: None,
            client_id: None,
            description: None,
        }
    }

    fn empty_runtime_mutation() -> RuntimeMutationRequest {
        RuntimeMutationRequest {
            graph_patch: None,
            view_patch: None,
            actor_id: None,
            client_id: None,
            description: None,
        }
    }

    fn described_runtime_mutation(description: &str) -> RuntimeMutationRequest {
        RuntimeMutationRequest {
            graph_patch: None,
            view_patch: None,
            actor_id: None,
            client_id: None,
            description: Some(description.to_owned()),
        }
    }

    fn set_value_patch(base_revision: &str, value: f64) -> GraphPatch {
        graph_patch(json!({
          "schema": "skenion.graph.patch",
          "schemaVersion": "0.1.0",
          "id": "set-value",
          "baseRevision": base_revision,
          "ops": [
            { "op": "setNodeParam", "nodeId": "value_1", "key": "value", "value": value }
          ]
        }))
    }

    fn paste_operation(base_revision: &str) -> RuntimeOperationEnvelope {
        serde_json::from_value(json!({
          "schema": "skenion.runtime.operation",
          "schemaVersion": "0.1.0",
          "id": "op-paste",
          "kind": "pasteGraphFragment",
          "request": {
            "target": {
              "path": { "kind": "root" },
              "baseRevision": base_revision
            },
            "fragment": paste_fragment_json(),
            "options": {
              "idConflictPolicy": "remap"
            }
          },
          "attribution": {
            "clientId": "studio-test",
            "label": "Paste test fragment"
          }
        }))
        .expect("paste operation should parse")
    }

    fn paste_fragment_json() -> Value {
        json!({
          "schema": "skenion.graph.fragment",
          "schemaVersion": "0.1.0",
          "nodes": [
            {
              "id": "value_1",
              "kind": "object.core.float",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": value_f32_ports_current_json()
            },
            {
              "id": "pasted_target",
              "kind": "object.core.float",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": value_f32_ports_current_json()
            }
          ],
          "edges": [
            {
              "id": "edge_value_to_pasted",
              "source": { "nodeId": "value_1", "portId": "value" },
              "target": { "nodeId": "pasted_target", "portId": "cold" }
            }
          ],
          "view": {
            "nodes": {
              "value_1": { "x": 10.0, "y": 20.0 },
              "pasted_target": { "x": 180.0, "y": 20.0 }
            }
          }
        })
    }

    fn graph_node_current(id: &str) -> crate::GraphNodeCurrent {
        serde_json::from_value(value_node_current_json(id))
            .expect("current 0.1 graph node should parse")
    }

    fn value_node_current_json(id: &str) -> Value {
        json!({
          "id": id,
          "kind": "object.core.float",
          "kindVersion": "0.1.0",
          "params": {},
          "ports": value_f32_ports_current_json()
        })
    }

    fn patch_definition_current(id: &str) -> skenion_contracts::PatchDefinitionV01 {
        serde_json::from_value(json!({
          "id": id,
          "revision": "1",
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": format!("{id}-graph"),
            "revision": "1",
            "nodes": [value_node_current_json("patch_value")],
            "edges": []
          }
        }))
        .expect("patch definition should parse")
    }

    fn collaboration_change(value: Value) -> RuntimeCollaborationChange {
        serde_json::from_value(value).expect("collaboration change should parse")
    }

    fn f32_value(value: f64) -> ControlValue {
        ControlValue::float(value)
    }

    fn control_request(
        node_id: &str,
        port_id: &str,
        value: ControlValue,
    ) -> RuntimeControlEventRequest {
        RuntimeControlEventRequest {
            node_id: node_id.to_owned(),
            port_id: port_id.to_owned(),
            message: ControlMessage::from_value(value),
        }
    }

    fn set_control_request(
        node_id: &str,
        port_id: &str,
        value: ControlValue,
    ) -> RuntimeControlEventRequest {
        RuntimeControlEventRequest {
            node_id: node_id.to_owned(),
            port_id: port_id.to_owned(),
            message: ControlMessage {
                selector: "set".to_owned(),
                atoms: vec![value],
            },
        }
    }

    fn bang_control_request(node_id: &str, port_id: &str) -> RuntimeControlEventRequest {
        RuntimeControlEventRequest {
            node_id: node_id.to_owned(),
            port_id: port_id.to_owned(),
            message: ControlMessage::bang(),
        }
    }

    fn emitted_value(emission: &RuntimeControlEmission) -> Option<ControlValue> {
        emission.message.first_atom().cloned()
    }

    fn control_read(
        node_id: &str,
        target: RuntimeControlReadTarget,
        id: &str,
    ) -> RuntimeControlReadRequest {
        RuntimeControlReadRequest {
            node_id: node_id.to_owned(),
            target,
            id: id.to_owned(),
        }
    }

    fn duplicate_edge_patch() -> GraphPatch {
        graph_patch(json!({
          "schema": "skenion.graph.patch",
          "schemaVersion": "0.1.0",
          "id": "duplicate-edge",
          "baseRevision": "1",
          "ops": [
            {
              "op": "addEdge",
              "edge": {
                "from": { "node": "value_1", "port": "value" },
                "to": { "node": "target_1", "port": "in" }
              }
            }
          ]
        }))
    }

    fn missing_node_patch() -> GraphPatch {
        graph_patch(json!({
          "schema": "skenion.graph.patch",
          "schemaVersion": "0.1.0",
          "id": "missing-node",
          "baseRevision": "1",
          "ops": [
            { "op": "setNodeParam", "nodeId": "missing", "key": "value", "value": 1 }
          ]
        }))
    }

    fn incompatible_edge_patch() -> GraphPatch {
        graph_patch(json!({
          "schema": "skenion.graph.patch",
          "schemaVersion": "0.1.0",
          "id": "incompatible-edge",
          "baseRevision": "1",
          "ops": [
            {
              "op": "addEdge",
              "edge": {
                "from": { "node": "value_1", "port": "value" },
                "to": { "node": "target_1", "port": "value" }
              }
            }
          ]
        }))
    }

    fn missing_definition_node_patch() -> GraphPatch {
        graph_patch(json!({
          "schema": "skenion.graph.patch",
          "schemaVersion": "0.1.0",
          "id": "missing-definition-node",
          "baseRevision": "1",
          "ops": [
            {
              "op": "addNode",
              "node": {
                "id": "missing_kind_1",
                "kind": "missing.kind",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": []
              }
            }
          ]
        }))
    }

    fn load_sample_project(session: &mut RuntimeSession) -> super::RuntimeSessionResponse {
        session.load_project_current(sample_project_current())
    }

    fn sample_internal_graph() -> GraphDocument {
        super::lower_graph_for_execution(&sample_project_current().graph)
    }

    fn sample_project_current() -> ProjectRequestCurrent {
        serde_json::from_value(json!({
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": "minimal-value",
            "revision": "1",
            "nodes": [
              {
                "id": "value_1",
                "kind": "object.core.float",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": value_f32_ports_current_json()
              },
              {
                "id": "target_1",
                "kind": "object.core.float",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": value_f32_ports_current_json()
              }
            ],
            "edges": [
              {
                "id": "edge_value_target",
                "source": { "nodeId": "value_1", "portId": "value" },
                "target": { "nodeId": "target_1", "portId": "cold" },
                "resolvedType": "value.core.float32"
              }
            ]
          },
          "nodes": [
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "object.core.float",
              "version": "0.1.0",
              "displayName": "Float",
              "category": "Typed Controls",
              "ports": value_f32_ports_current_json(),
              "execution": { "model": "control" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": ["value.core.float32.v0.1"]
            }
          ],
          "viewState": {
            "schema": "skenion.view-state",
            "schemaVersion": "0.1.0",
            "canvas": {
              "nodes": {
                "value_1": { "x": 96.0, "y": 96.0 },
                "target_1": { "x": 260.0, "y": 96.0 }
              }
            }
          }
        }))
        .expect("current 0.1 sample project should parse")
    }

    fn binding_project_current() -> ProjectRequestCurrent {
        serde_json::from_value(json!({
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": "minimal-binding",
            "revision": "1",
            "nodes": [
              {
                "id": "value_1",
                "kind": "object.core.float",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": value_binding_ports_current_json()
              },
              {
                "id": "target_1",
                "kind": "object.core.float",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": value_binding_ports_current_json()
              }
            ],
            "edges": [
              {
                "id": "edge_value_target",
                "source": { "nodeId": "value_1", "portId": "value" },
                "target": { "nodeId": "target_1", "portId": "cold" },
                "resolvedType": "value.core.float32"
              }
            ]
          },
          "nodes": [
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "object.core.float",
              "version": "0.1.0",
              "displayName": "Float",
              "category": "Typed Controls",
              "ports": value_binding_ports_current_json(),
              "execution": { "model": "control" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": ["value.core.float32.v0.1"]
            }
          ],
          "viewState": {
            "schema": "skenion.view-state",
            "schemaVersion": "0.1.0",
            "canvas": {
              "nodes": {
                "value_1": { "x": 96.0, "y": 96.0 },
                "target_1": { "x": 260.0, "y": 96.0 }
              }
            }
          }
        }))
        .expect("binding sample project should parse")
    }

    fn sample_project_current_with_unresolved_definition() -> ProjectRequestCurrent {
        let mut request = sample_project_current();
        request.nodes.push(
            serde_json::from_value(unresolved_definition_current_json())
                .expect("unresolved current 0.1 definition should parse"),
        );
        request
    }

    fn unresolved_project_current() -> ProjectRequestCurrent {
        let mut request = sample_project_current();
        request.graph.nodes.push(
            serde_json::from_value(unresolved_node_json("unresolved_1", "user.manipulator"))
                .expect("unresolved current 0.1 node should parse"),
        );
        request.nodes.push(
            serde_json::from_value(unresolved_definition_current_json())
                .expect("unresolved current 0.1 definition should parse"),
        );
        request
    }

    fn object_routing_project_current() -> ProjectRequestCurrent {
        let mut request = sample_project_current();
        request.graph.nodes[0]
            .params
            .insert("sendName".to_owned(), json!("speed"));
        request
    }

    fn debug_sink_project_current() -> ProjectRequestCurrent {
        let mut request = sample_project_current();
        request.graph.nodes.push(
            serde_json::from_value(json!({
                "id": "debug_1",
                "kind": "debug.sink",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": []
            }))
            .expect("debug current 0.1 node should parse"),
        );
        request.nodes.push(
            serde_json::from_value(json!({
                "schema": "skenion.node.definition",
                "schemaVersion": "0.1.0",
                "id": "debug.sink",
                "version": "0.1.0",
                "displayName": "Debug Sink",
                "category": "Debug",
                "ports": [],
                "execution": { "model": "control" },
                "state": { "persistent": false },
                "permissions": [],
                "capabilities": []
            }))
            .expect("debug current 0.1 definition should parse"),
        );
        request
    }

    fn value_f32_ports_current_json() -> Value {
        json!([
          {
            "id": "in",
            "direction": "input",
            "label": "In",
            "type": "value.core.message",
            "rate": "control",
            "required": false,
            "triggerMode": "trigger",
            "accepts": [
              "value.core.float32",
              "value.core.int32",
              "value.core.uint32",
              "value.core.bool",
              "value.core.bang"
            ],
            "messageKeys": {
              "accepted": ["bang", "set", "float", "int", "uint", "bool"],
              "silent": ["set"],
              "trigger": ["bang", "float", "int", "uint", "bool"],
              "store": ["set", "float", "int", "uint", "bool"],
              "emit": ["bang", "float", "int", "uint", "bool"]
            }
          },
          {
            "id": "cold",
            "direction": "input",
            "label": "Cold",
            "type": "value.core.float32",
            "rate": "control",
            "required": false,
            "triggerMode": "passive"
          },
          {
            "id": "value",
            "direction": "output",
            "label": "Value",
            "type": "value.core.float32",
            "rate": "control"
          }
        ])
    }

    fn value_binding_ports_current_json() -> Value {
        json!([
          {
            "id": "cold",
            "direction": "input",
            "label": "Cold",
            "type": "value.core.float32",
            "rate": "control",
            "required": false,
            "triggerMode": "passive"
          },
          {
            "id": "value",
            "direction": "output",
            "label": "Value",
            "type": "value.core.float32",
            "rate": "control"
          }
        ])
    }

    fn unresolved_node_json(id: &str, object_text: &str) -> Value {
        json!({
          "id": id,
          "kind": "object.core.unresolved",
          "kindVersion": "0.1.0",
          "params": {
            "objectText": object_text,
            "diagnosticMessage": format!("{object_text} is not available in the local runtime registry."),
            "requestedKind": object_text
          },
          "ports": []
        })
    }

    fn unresolved_definition_current_json() -> Value {
        json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": "object.core.unresolved",
          "version": "0.1.0",
          "displayName": "Unresolved Object",
          "category": "Diagnostics",
          "ports": [],
          "execution": { "model": "event" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": ["diagnostic.unresolved-object.v0.1"]
        })
    }
}
