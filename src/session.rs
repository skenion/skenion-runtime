use std::{
    collections::{BTreeMap, HashSet},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    CanvasNodeView, ControlState, DataFlow, DataType, DummyExecutionReport, Edge, EdgeSpecV02,
    ExecutionPlan, GraphDocument, GraphDocumentV02, GraphFragmentOutsideEndpointPolicyV02,
    GraphNode, GraphNodeV02, GraphPatch, GraphTargetRef, IdConflictPolicy, IdRemapResult,
    NodeDefinition, NodeDefinitionV02, NodeRegistry, PasteGraphFragmentRequest,
    PasteGraphFragmentResponse, PastePlacement, PatchPath, Port, PortActivation, PortDirection,
    PortDirectionV02, PortRateV02, PortRef, PortSpecV02, PreviewContext,
    PreviewControlStateSnapshot, ProjectDocumentV02, ProjectRequest, ProjectRequestV02,
    RuntimeCollaborationChange, RuntimeControlEventRequest, RuntimeControlEventResponse,
    RuntimeControlReadRequest, RuntimeControlReadResponse, RuntimeControlReadTarget,
    RuntimeControlStateResponse, RuntimeDiagnostic, RuntimeOperationDiagnostic,
    RuntimeOperationEnvelope, StringOrStrings, ViewState, build_execution_plan,
    build_execution_plan_request_v02, create_default_view_state_for_graph, read_graph_param,
    read_graph_port, run_dummy_execution, server::registry_from_nodes,
};
const UNRESOLVED_OBJECT_NODE_KIND: &str = "core.unresolved-object";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSessionSnapshot {
    pub session_revision: u64,
    pub view_revision: u64,
    pub control_revision: u64,
    pub project: Option<ProjectDocumentV02>,
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
    pub graph_patch: Option<GraphPatch>,
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
    project: Option<ProjectDocumentV02>,
    nodes_v02: Vec<NodeDefinitionV02>,
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
}

impl Default for RuntimeSession {
    fn default() -> Self {
        Self {
            project: None,
            nodes_v02: Vec::new(),
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
        before: Box<ProjectDocumentV02>,
        after: Box<ProjectDocumentV02>,
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
            project: self.project.clone(),
            diagnostics: self.diagnostics.clone(),
            plan: self.plan.clone(),
        }
    }

    pub fn preview_context(&self) -> Result<PreviewContext, Vec<RuntimeDiagnostic>> {
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

    pub fn load_project_v02(&mut self, request: ProjectRequestV02) -> RuntimeSessionResponse {
        let document = project_document_from_request_v02(&request);
        if let Err(report) = skenion_contracts::validate_project_document_v02(&document) {
            let diagnostics = report
                .errors()
                .iter()
                .map(|error| {
                    RuntimeDiagnostic::structured_error(
                        "project.invalid-v0.2",
                        error.message.clone(),
                        serde_json::json!({ "projectId": document.id }),
                    )
                })
                .collect();
            return self.response(false, diagnostics, None);
        }

        let (plan, mut diagnostics) = match build_execution_plan_request_v02(&request) {
            Ok(result) => result,
            Err(diagnostics) => return self.response(false, diagnostics, None),
        };
        diagnostics.extend(unresolved_object_diagnostics_v02(&document.graph));

        let graph = graph_document_v02_to_v01(&document.graph);
        let legacy_nodes = request
            .nodes
            .iter()
            .map(node_definition_v02_to_v01)
            .collect::<Vec<_>>();
        let registry = match registry_from_nodes(legacy_nodes) {
            Ok(registry) => registry,
            Err(diagnostics) => return self.response(false, diagnostics, None),
        };
        let control_state = ControlState::from_graph(&graph);
        let view_state = reconcile_view_state_with_graph(&graph, Some(document.view_state.clone()));
        self.project = Some(document);
        self.nodes_v02 = request.nodes;
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

        self.response(true, diagnostics, None)
    }

    pub fn import_legacy_project_v01(&mut self, request: ProjectRequest) -> RuntimeSessionResponse {
        let ProjectRequest {
            graph,
            nodes,
            view_state,
        } = request;
        let request = ProjectRequestV02 {
            document: None,
            graph: graph_document_v01_to_v02(graph),
            nodes: nodes.iter().map(node_definition_v01_to_v02).collect(),
            patch_library: Vec::new(),
            view_state,
        };
        let mut response = self.load_project_v02(request);
        if response.ok {
            let diagnostic = RuntimeDiagnostic::structured_warning(
                "project.legacy-v0.1-import",
                "legacy v0.1 project imported into active v0.2 Runtime session",
                serde_json::json!({ "activeSchemaVersion": "0.2.0" }),
            );
            self.diagnostics.push(diagnostic.clone());
            response.snapshot = self.snapshot();
            response.diagnostics.push(diagnostic);
        }
        response
    }

    pub fn load_project(&mut self, _request: ProjectRequest) -> RuntimeSessionResponse {
        self.response(
            false,
            vec![RuntimeDiagnostic::structured_error(
                "project.active-v0.1-unsupported",
                "v0.1 ProjectRequest is a legacy import input only; use load_project_v02 for active sessions or import_legacy_project_v01 for migration",
                serde_json::json!({ "activeSchemaVersion": "0.2.0" }),
            )],
            None,
        )
    }

    pub fn validate_current(&mut self) -> RuntimeSessionResponse {
        let diagnostics = match self.current_project_request_v02() {
            Some(request) => match crate::validate_project_request_v02(&request) {
                Ok((mut diagnostics, _)) => {
                    diagnostics.extend(unresolved_object_diagnostics_v02(&request.graph));
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
        let request = match self.current_project_request_v02() {
            Some(request) => request,
            None => {
                let diagnostics = vec![RuntimeDiagnostic::error(
                    "no project loaded in runtime session",
                )];
                self.diagnostics = diagnostics.clone();
                return self.response(false, diagnostics, None);
            }
        };

        match build_execution_plan_request_v02(&request) {
            Ok((plan, mut diagnostics)) => {
                diagnostics.extend(unresolved_object_diagnostics_v02(&request.graph));
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

    pub fn apply_patch(&mut self, patch: GraphPatch) -> RuntimePatchResponse {
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
        if let Err(report) = skenion_contracts::validate_runtime_operation_envelope(&envelope) {
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

    pub fn apply_collaboration_change_set_v02(
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
        let target_revision = match target_graph_revision_v02(&project, &target) {
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

        let (next_project, next_view_revision) = match apply_collaboration_changes_to_project_v02(
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
        self.nodes_v02 = Vec::new();
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

    pub fn graph(&self) -> Option<GraphDocument> {
        self.graph.clone()
    }

    pub fn project_document_v02(&self) -> Option<ProjectDocumentV02> {
        self.project.clone()
    }

    pub fn target_revision_v02(&self, target: &GraphTargetRef) -> Option<String> {
        self.project
            .as_ref()
            .and_then(|project| target_graph_revision_v02(project, target).ok())
    }

    pub fn view_state(&self) -> Option<ViewState> {
        self.view_state.clone()
    }

    fn current_project_request_v02(&self) -> Option<ProjectRequestV02> {
        let project = self.project.as_ref()?;
        Some(ProjectRequestV02 {
            document: self.project.clone(),
            graph: project.graph.clone(),
            nodes: self.nodes_v02.clone(),
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
                    "project.active-v0.1-graph-patch-unsupported",
                    "active Runtime sessions use v0.2 ProjectDocument graph targets; v0.1 graphPatch is supported only through explicit legacy import/migration",
                    serde_json::json!({ "activeSchemaVersion": "0.2.0" }),
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

        let previous_view_state = runtime_owned_view_state(reconcile_view_state_with_graph(
            &graph,
            self.view_state.clone(),
        ));
        let mut next_view_state =
            reconcile_view_state_with_graph(&next_graph, Some(previous_view_state.clone()));
        let mut inverse_view_patch = None;
        if let Some(view_patch) = &mutation.view_patch {
            match apply_view_patch_to_view_state(&next_graph, next_view_state, view_patch) {
                Ok((patched_view_state, inverse_patch)) => {
                    next_view_state = patched_view_state;
                    inverse_view_patch = Some(inverse_patch);
                }
                Err(diagnostics) => {
                    return self.patch_response(false, false, false, diagnostics);
                }
            }
        }
        next_view_state = runtime_owned_view_state(next_view_state);
        let view_changed = previous_view_state != next_view_state;

        if !view_changed {
            return self.patch_response(true, false, false, Vec::new());
        }

        let diagnostics = unresolved_object_diagnostics(&next_graph);
        let plan =
            build_execution_plan(&next_graph, &registry).expect("validated project should plan");
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
            if view_changed {
                self.view_revision + 1
            } else {
                self.view_revision
            },
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

        let target_revision = match target_graph_revision_v02(&project, &request.target) {
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
            match paste_graph_fragment_into_project_v02(
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
        before: ProjectDocumentV02,
        after: ProjectDocumentV02,
        next_view_revision: u64,
        mutation: RuntimeMutationRequest,
        subject_event_id: Option<String>,
    ) -> RuntimePatchResponse {
        let before_view_revision = self.view_revision;
        let request = ProjectRequestV02 {
            document: Some(after.clone()),
            graph: after.graph.clone(),
            nodes: self.nodes_v02.clone(),
            patch_library: after.patch_library.clone(),
            view_state: Some(after.view_state.clone()),
        };
        let (plan, mut diagnostics) = match build_execution_plan_request_v02(&request) {
            Ok(result) => result,
            Err(diagnostics) => return self.patch_response(false, false, false, diagnostics),
        };
        diagnostics.extend(unresolved_object_diagnostics_v02(&after.graph));
        let graph = graph_document_v02_to_v01(&after.graph);
        let registry = match registry_from_nodes(
            self.nodes_v02
                .iter()
                .map(node_definition_v02_to_v01)
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
                .unwrap_or_else(|| reconcile_view_state_with_graph(&graph, None)),
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
        mut project: ProjectDocumentV02,
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
        let request = ProjectRequestV02 {
            document: Some(project.clone()),
            graph: project.graph.clone(),
            nodes: self.nodes_v02.clone(),
            patch_library: project.patch_library.clone(),
            view_state: Some(project.view_state.clone()),
        };
        let (plan, mut diagnostics) = match build_execution_plan_request_v02(&request) {
            Ok(result) => result,
            Err(diagnostics) => {
                return self.patch_response(false, false, false, diagnostics);
            }
        };
        diagnostics.extend(unresolved_object_diagnostics_v02(&project.graph));
        let graph = graph_document_v02_to_v01(&project.graph);
        let registry = match registry_from_nodes(
            self.nodes_v02
                .iter()
                .map(node_definition_v02_to_v01)
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
                .unwrap_or_else(|| reconcile_view_state_with_graph(&graph, None)),
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
    current: &ProjectDocumentV02,
    before: &ProjectDocumentV02,
    after: &ProjectDocumentV02,
    direction: HistoryDirection,
) -> ProjectDocumentV02 {
    let (expected_current, exact_target) = match direction {
        HistoryDirection::Undo => (after, before),
        HistoryDirection::Redo => (before, after),
    };
    if current == expected_current {
        return exact_target.clone();
    }

    let mut project = current.clone();
    apply_graph_history_delta_v02(&mut project.graph, &before.graph, &after.graph, direction);
    project.view_state = view_state_history_delta_v02(
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
        if apply_graph_history_delta_v02(
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

fn apply_graph_history_delta_v02(
    current: &mut GraphDocumentV02,
    before: &GraphDocumentV02,
    after: &GraphDocumentV02,
    direction: HistoryDirection,
) -> bool {
    match direction {
        HistoryDirection::Undo => undo_graph_history_delta_v02(current, before, after),
        HistoryDirection::Redo => redo_graph_history_delta_v02(current, before, after),
    }
}

fn undo_graph_history_delta_v02(
    current: &mut GraphDocumentV02,
    before: &GraphDocumentV02,
    after: &GraphDocumentV02,
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

fn redo_graph_history_delta_v02(
    current: &mut GraphDocumentV02,
    before: &GraphDocumentV02,
    after: &GraphDocumentV02,
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

fn view_state_history_delta_v02(
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

fn project_document_from_request_v02(request: &ProjectRequestV02) -> ProjectDocumentV02 {
    if let Some(document) = &request.document {
        return document.clone();
    }
    let graph = request.graph.clone();
    let view_state = request.view_state.clone().unwrap_or_else(|| {
        reconcile_view_state_with_graph(&graph_document_v02_to_v01(&graph), None)
    });
    ProjectDocumentV02 {
        schema: "skenion.project".to_owned(),
        schema_version: "0.2.0".to_owned(),
        id: graph.id.clone(),
        revision: graph.revision.clone(),
        metadata: None,
        graph,
        view_state,
        patch_library: request.patch_library.clone(),
        tutorial: None,
        help: None,
    }
}

fn graph_document_v02_to_v01(graph: &GraphDocumentV02) -> GraphDocument {
    GraphDocument {
        schema: "skenion.graph".to_owned(),
        schema_version: "0.1.0".to_owned(),
        id: graph.id.clone(),
        revision: graph.revision.clone(),
        nodes: graph
            .nodes
            .iter()
            .map(|node| graph_node_v02_to_v01(node, &node.id))
            .collect(),
        edges: graph.edges.iter().map(edge_v02_to_v01).collect(),
    }
}

fn graph_document_v01_to_v02(graph: GraphDocument) -> GraphDocumentV02 {
    GraphDocumentV02 {
        schema: "skenion.graph".to_owned(),
        schema_version: "0.2.0".to_owned(),
        id: graph.id,
        revision: graph.revision,
        nodes: graph.nodes.iter().map(graph_node_v01_to_v02).collect(),
        edges: graph
            .edges
            .iter()
            .enumerate()
            .map(|(index, edge)| edge_v01_to_v02(index, edge))
            .collect(),
        cable_styles: None,
    }
}

fn graph_node_v01_to_v02(node: &GraphNode) -> GraphNodeV02 {
    GraphNodeV02 {
        id: node.id.clone(),
        kind: node.kind.clone(),
        kind_version: node.kind_version.clone(),
        params: node.params.clone(),
        ports: node.ports.iter().map(port_v01_to_v02).collect(),
        port_groups: None,
    }
}

fn edge_v01_to_v02(index: usize, edge: &Edge) -> EdgeSpecV02 {
    EdgeSpecV02 {
        id: format!(
            "legacy_edge_{}_{}_{}_{}",
            sanitize_id_fragment(&edge.from.node),
            sanitize_id_fragment(&edge.from.port),
            sanitize_id_fragment(&edge.to.node),
            index + 1
        ),
        source: crate::EdgeEndpointV02 {
            node_id: edge.from.node.clone(),
            port_id: edge.from.port.clone(),
        },
        target: crate::EdgeEndpointV02 {
            node_id: edge.to.node.clone(),
            port_id: edge.to.port.clone(),
        },
        resolved_type: None,
        order: None,
        enabled: None,
        adapter: None,
        feedback: None,
        style_override: None,
        label: None,
        description: None,
    }
}

fn node_definition_v01_to_v02(definition: &NodeDefinition) -> NodeDefinitionV02 {
    NodeDefinitionV02 {
        schema: "skenion.node.definition".to_owned(),
        schema_version: "0.2.0".to_owned(),
        id: definition.id.clone(),
        version: definition.version.clone(),
        display_name: definition.display_name.clone(),
        category: definition.category.clone(),
        script_api_version: definition.script_api_version.clone(),
        bundle_hash: definition.bundle_hash.clone(),
        surface: definition
            .surface
            .as_ref()
            .map(|surface| skenion_contracts::NodeSurfaceV02 {
                palette: surface.palette.clone(),
            }),
        ports: definition.ports.iter().map(port_v01_to_v02).collect(),
        port_groups: None,
        execution: skenion_contracts::NodeExecutionV02 {
            model: execution_model_v01_to_v02(&definition.execution.model),
            clock: definition.execution.clock.clone(),
        },
        state: skenion_contracts::NodeStateV02 {
            persistent: definition.state.persistent,
        },
        permissions: definition.permissions.clone(),
        capabilities: definition.capabilities.clone(),
    }
}

fn node_definition_v02_to_v01(definition: &NodeDefinitionV02) -> NodeDefinition {
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
        ports: definition.ports.iter().map(port_v02_to_v01).collect(),
        execution: skenion_contracts::NodeExecutionV01 {
            model: execution_model_v02_to_v01(&definition.execution.model),
            clock: definition.execution.clock.clone(),
        },
        state: skenion_contracts::NodeStateV01 {
            persistent: definition.state.persistent,
        },
        permissions: definition.permissions.clone(),
        capabilities: definition.capabilities.clone(),
    }
}

fn port_v01_to_v02(port: &Port) -> PortSpecV02 {
    PortSpecV02 {
        id: port.id.clone(),
        direction: match port.direction {
            PortDirection::Input => PortDirectionV02::Input,
            PortDirection::Output => PortDirectionV02::Output,
        },
        port_type: port.data_type.data_kind.clone(),
        label: port.label.clone(),
        rate: Some(match port.data_type.flow {
            DataFlow::Event => PortRateV02::Event,
            DataFlow::Signal => PortRateV02::Audio,
            DataFlow::Resource => PortRateV02::Resource,
            DataFlow::Stream => PortRateV02::Io,
            DataFlow::Value => PortRateV02::Control,
        }),
        accepts: None,
        min_connections: port.required.filter(|required| *required).map(|_| 1),
        max_connections: None,
        merge_policy: None,
        fan_out_policy: None,
        trigger_mode: port.activation.as_ref().map(|activation| match activation {
            PortActivation::Trigger => skenion_contracts::TriggerModeV02::Trigger,
            PortActivation::Latched => skenion_contracts::TriggerModeV02::Latched,
        }),
        default_value: port.default_value.clone(),
        latch: None,
        required: port.required,
        style_key: None,
        group: None,
        description: None,
    }
}

fn execution_model_v01_to_v02(
    model: &crate::ExecutionModel,
) -> skenion_contracts::ExecutionModelV02 {
    match model {
        crate::ExecutionModel::Event => skenion_contracts::ExecutionModelV02::Event,
        crate::ExecutionModel::Value => skenion_contracts::ExecutionModelV02::Value,
        crate::ExecutionModel::Frame => skenion_contracts::ExecutionModelV02::Frame,
        crate::ExecutionModel::AudioBlock => skenion_contracts::ExecutionModelV02::AudioBlock,
        crate::ExecutionModel::VideoFrame => skenion_contracts::ExecutionModelV02::VideoFrame,
        crate::ExecutionModel::GpuPass => skenion_contracts::ExecutionModelV02::GpuPass,
        crate::ExecutionModel::AsyncResource => skenion_contracts::ExecutionModelV02::AsyncResource,
        crate::ExecutionModel::ScriptControl => skenion_contracts::ExecutionModelV02::ScriptControl,
        crate::ExecutionModel::NativePlugin => skenion_contracts::ExecutionModelV02::NativePlugin,
    }
}

fn execution_model_v02_to_v01(
    model: &skenion_contracts::ExecutionModelV02,
) -> crate::ExecutionModel {
    match model {
        skenion_contracts::ExecutionModelV02::Event => crate::ExecutionModel::Event,
        skenion_contracts::ExecutionModelV02::Value => crate::ExecutionModel::Value,
        skenion_contracts::ExecutionModelV02::Frame => crate::ExecutionModel::Frame,
        skenion_contracts::ExecutionModelV02::AudioBlock => crate::ExecutionModel::AudioBlock,
        skenion_contracts::ExecutionModelV02::VideoFrame => crate::ExecutionModel::VideoFrame,
        skenion_contracts::ExecutionModelV02::GpuPass => crate::ExecutionModel::GpuPass,
        skenion_contracts::ExecutionModelV02::AsyncResource => crate::ExecutionModel::AsyncResource,
        skenion_contracts::ExecutionModelV02::ScriptControl => crate::ExecutionModel::ScriptControl,
        skenion_contracts::ExecutionModelV02::NativePlugin => crate::ExecutionModel::NativePlugin,
    }
}

fn sanitize_id_fragment(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn reconcile_view_state_with_graph(
    graph: &GraphDocument,
    view_state: Option<ViewState>,
) -> ViewState {
    let mut reconciled = create_default_view_state_for_graph(graph);
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

fn target_graph_revision_v02(
    project: &ProjectDocumentV02,
    target: &GraphTargetRef,
) -> Result<String, Box<RuntimeOperationDiagnostic>> {
    Ok(target_graph_v02(project, target)?.revision.clone())
}

fn target_graph_v02<'a>(
    project: &'a ProjectDocumentV02,
    target: &GraphTargetRef,
) -> Result<&'a GraphDocumentV02, Box<RuntimeOperationDiagnostic>> {
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
    Result<(ProjectDocumentV02, u64, IdRemapResult, String), PasteProjectError>;
type PasteProjectError = (Vec<RuntimeOperationDiagnostic>, IdRemapResult);

fn paste_graph_fragment_into_project_v02(
    mut project: ProjectDocumentV02,
    view_revision: u64,
    request: &PasteGraphFragmentRequest,
) -> PasteProjectResult {
    let target_path = request.target.path.clone();
    let (next_graph, id_remap) = {
        let graph = match graph_for_path_v02(&project, &target_path) {
            Some(graph) => graph,
            None => {
                return Err((
                    vec![operation_error(
                        "paste.target.missing-graph",
                        "paste target graph is not available in the active v0.2 project",
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
        paste_graph_fragment_into_graph_v02(graph, request)?
    };
    let revision_after = next_graph.revision.clone();
    let mut next_view_revision = view_revision;
    match &target_path {
        PatchPath::Root | PatchPath::HelpWorkingCopy { .. } => {
            let graph_v01 = graph_document_v02_to_v01(&next_graph);
            let view_patch =
                lower_fragment_view_patch(view_revision, request, &id_remap.node_id_map);
            let view_state = if let Some(view_patch) = view_patch {
                match apply_view_patch_to_view_state(
                    &graph_v01,
                    reconcile_view_state_with_graph(&graph_v01, Some(project.view_state.clone())),
                    &view_patch,
                ) {
                    Ok((view_state, _)) => {
                        next_view_revision += 1;
                        view_state
                    }
                    Err(diagnostics) => {
                        return Err((
                            diagnostics
                                .iter()
                                .map(|diagnostic| {
                                    runtime_diagnostic_to_operation_diagnostic(
                                        diagnostic,
                                        &request.target,
                                    )
                                })
                                .collect(),
                            id_remap,
                        ));
                    }
                }
            } else {
                reconcile_view_state_with_graph(&graph_v01, Some(project.view_state.clone()))
            };
            project.graph = next_graph;
            project.revision = project.graph.revision.clone();
            project.view_state = view_state;
        }
        PatchPath::ProjectPatchDefinition { patch_id } => {
            let Some(patch) = project
                .patch_library
                .iter_mut()
                .find(|patch| patch.id == *patch_id)
            else {
                return Err((
                    vec![operation_error(
                        "paste.target.missing-project-patch-definition",
                        format!(
                            "project patch definition {patch_id} is not loaded in this runtime session"
                        ),
                        Some(request.target.clone()),
                        None,
                        None,
                        None,
                        None,
                    )],
                    id_remap,
                ));
            };
            patch.graph = next_graph;
            patch.revision = patch.graph.revision.clone();
        }
        PatchPath::PackagePatchDefinition { .. } | PatchPath::EmbeddedPatchInstance { .. } => {
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
                id_remap,
            ));
        }
    }

    Ok((project, next_view_revision, id_remap, revision_after))
}

fn apply_collaboration_changes_to_project_v02(
    mut project: ProjectDocumentV02,
    view_revision: u64,
    target: &GraphTargetRef,
    changes: &[RuntimeCollaborationChange],
) -> Result<(ProjectDocumentV02, u64), Vec<RuntimeDiagnostic>> {
    let mut graph = graph_for_path_v02(&project, &target.path).ok_or_else(|| {
        vec![RuntimeDiagnostic::structured_error(
            "collaboration.target.missing-graph",
            "collaboration target graph is not available in the active v0.2 project",
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
    match &target.path {
        PatchPath::Root | PatchPath::HelpWorkingCopy { .. } => {
            let graph_v01 = graph_document_v02_to_v01(&graph);
            project.graph = graph;
            project.revision = project.graph.revision.clone();
            project.view_state = runtime_owned_view_state(reconcile_view_state_with_graph(
                &graph_v01,
                Some(view_state),
            ));
            if view_changed {
                next_view_revision += 1;
            }
        }
        PatchPath::ProjectPatchDefinition { patch_id } => {
            let Some(patch) = project
                .patch_library
                .iter_mut()
                .find(|patch| patch.id == *patch_id)
            else {
                return Err(vec![RuntimeDiagnostic::structured_error(
                    "collaboration.target.missing-project-patch-definition",
                    format!(
                        "project patch definition {patch_id} is not loaded in this runtime session"
                    ),
                    serde_json::json!({ "patchId": patch_id, "target": target }),
                )]);
            };
            patch.graph = graph;
            patch.revision = patch.graph.revision.clone();
        }
        PatchPath::PackagePatchDefinition { .. } | PatchPath::EmbeddedPatchInstance { .. } => {
            return Err(vec![RuntimeDiagnostic::structured_error(
                "collaboration.target.unsupported",
                "collaboration target cannot be mutated in the active Runtime session",
                serde_json::json!({ "target": target }),
            )]);
        }
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

fn graph_for_path_v02(project: &ProjectDocumentV02, path: &PatchPath) -> Option<GraphDocumentV02> {
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

fn paste_graph_fragment_into_graph_v02(
    mut graph: GraphDocumentV02,
    request: &PasteGraphFragmentRequest,
) -> Result<(GraphDocumentV02, IdRemapResult), (Vec<RuntimeOperationDiagnostic>, IdRemapResult)> {
    let outside_policy = request
        .options
        .as_ref()
        .and_then(|options| options.outside_endpoint_policy)
        .unwrap_or(GraphFragmentOutsideEndpointPolicyV02::Reject);
    let id_conflict_policy = request
        .options
        .as_ref()
        .and_then(|options| options.id_conflict_policy)
        .unwrap_or(IdConflictPolicy::Remap);

    let fragment_analysis =
        skenion_contracts::analyze_graph_fragment_v02(&request.fragment, outside_policy);
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
            .push(remap_edge_v02(edge, &node_id_map, pasted_edge_id));
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

fn remap_edge_v02(
    edge: &EdgeSpecV02,
    node_id_map: &BTreeMap<String, String>,
    edge_id: String,
) -> EdgeSpecV02 {
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

pub(crate) fn graph_node_v02_to_v01(node: &GraphNodeV02, pasted_id: &str) -> GraphNode {
    GraphNode {
        id: pasted_id.to_owned(),
        kind: node.kind.clone(),
        kind_version: node.kind_version.clone(),
        params: node.params.clone(),
        ports: node.ports.iter().map(port_v02_to_v01).collect(),
    }
}

fn port_v02_to_v01(port: &PortSpecV02) -> Port {
    Port {
        id: port.id.clone(),
        direction: match port.direction {
            PortDirectionV02::Input => PortDirection::Input,
            PortDirectionV02::Output => PortDirection::Output,
        },
        label: port.label.clone(),
        data_type: data_type_from_port_spec(port),
        required: port.required,
        default_value: port.default_value.clone(),
        activation: port.trigger_mode.as_ref().map(|trigger| match trigger {
            skenion_contracts::TriggerModeV02::Trigger => PortActivation::Trigger,
            skenion_contracts::TriggerModeV02::Latched => PortActivation::Latched,
            skenion_contracts::TriggerModeV02::Passive => PortActivation::Latched,
        }),
    }
}

fn data_type_from_port_spec(port: &PortSpecV02) -> DataType {
    let data_kind = normalize_port_type(&port.port_type);
    let format = match data_kind.as_str() {
        "number.float" => Some(StringOrStrings::One("f32".to_owned())),
        "gpu.texture2d" => Some(StringOrStrings::One("rgba8unorm".to_owned())),
        _ => None,
    };
    let color_space = (data_kind == "gpu.texture2d").then(|| "srgb".to_owned());
    DataType {
        flow: match port.rate {
            Some(PortRateV02::Event) => DataFlow::Event,
            Some(PortRateV02::Audio) => DataFlow::Signal,
            Some(PortRateV02::Resource) | Some(PortRateV02::Io) => DataFlow::Resource,
            Some(PortRateV02::Control | PortRateV02::Render | PortRateV02::Gpu) | None => {
                if port.port_type == "message.any" {
                    DataFlow::Event
                } else if data_kind == "gpu.texture2d" {
                    DataFlow::Resource
                } else {
                    DataFlow::Value
                }
            }
        },
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

fn normalize_port_type(port_type: &str) -> String {
    match port_type {
        "value.number" => "number.float".to_owned(),
        other => other
            .strip_prefix("value.")
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| other.to_owned()),
    }
}

fn remap_edge(edge: &EdgeSpecV02, node_id_map: &BTreeMap<String, String>) -> Edge {
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

pub(crate) fn edge_v02_to_v01(edge: &EdgeSpecV02) -> Edge {
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

fn unresolved_object_diagnostics_v02(graph: &GraphDocumentV02) -> Vec<RuntimeDiagnostic> {
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

fn created_at_now() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("unix-ms:{millis}")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{Value, json};

    use crate::{
        ControlMessage, ControlValue, Edge, EdgeEndpointV02, EdgeSpecV02, GraphDocument,
        GraphDocumentV02, GraphPatch, NodeRegistry, PasteGraphFragmentRequest, PortRef,
        PortSpecV02, ProjectRequest, ProjectRequestV02, RuntimeCollaborationChange,
        RuntimeControlEmission, RuntimeControlEventRequest, RuntimeControlReadRequest,
        RuntimeControlReadTarget, RuntimeDiagnostic, RuntimeOperationDiagnostic,
        RuntimeOperationEnvelope, ViewState,
    };

    use super::{
        HistoryEntry, RuntimeHistoryEntryKind, RuntimeMutationRequest, RuntimePatchResponse,
        RuntimeSession, RuntimeViewPatch, RuntimeViewPatchOperation, lower_fragment_view_patch,
        port_v02_to_v01, remap_edge, runtime_diagnostic_to_operation_diagnostic,
    };

    #[test]
    fn invalid_registry_load_returns_diagnostics_without_revision_change() {
        let mut session = RuntimeSession::default();
        let mut request = sample_project_v02();
        request.nodes[0].schema_version = "9.9.9".to_owned();

        let response = session.load_project_v02(request);

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
    fn plan_current_reports_invalid_stored_project() {
        let mut session = RuntimeSession {
            graph: Some(sample_project().graph),
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
            graph: Some(sample_project().graph),
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
            graph: Some(sample_project().graph),
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
        let loaded = session.import_legacy_project_v01(sample_project());
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
            graph: Some(sample_project().graph),
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
        assert!(session.import_legacy_project_v01(sample_project()).ok);

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
        assert_eq!(bang.emitted.len(), 2);
        assert_eq!(bang.emitted[0].node_id, "value_1");
        assert_eq!(bang.emitted[0].port_id, "value");
        assert_eq!(
            emitted_value(&bang.emitted[0]),
            Some(ControlValue::float(32.0))
        );
        assert_eq!(bang.emitted[1].node_id, "target_1");
        assert_eq!(
            emitted_value(&bang.emitted[1]),
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
                .import_legacy_project_v01(object_routing_project())
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
                .get("number.float:speed"),
            Some(&ControlMessage::from_value(ControlValue::float(1.5)))
        );
    }

    #[test]
    fn control_read_addresses_params_ports_and_state() {
        let mut session = RuntimeSession::default();
        let mut project = sample_project();
        project.graph.nodes[0]
            .params
            .insert("value".to_owned(), json!(0.0));
        assert!(session.import_legacy_project_v01(project).ok);
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

        assert!(session.import_legacy_project_v01(sample_project()).ok);
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

        let mut project_with_debug = sample_project_json();
        project_with_debug["graph"]["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "id": "debug_1",
                "kind": "debug.sink",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": []
            }));
        project_with_debug["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "schema": "skenion.node.definition",
                "schemaVersion": "0.1.0",
                "id": "debug.sink",
                "version": "0.1.0",
                "displayName": "Debug Sink",
                "category": "Debug",
                "ports": [],
                "execution": { "model": "value" },
                "state": { "persistent": false },
                "permissions": [],
                "capabilities": []
            }));
        assert!(
            session
                .import_legacy_project_v01(
                    serde_json::from_value(project_with_debug).expect("debug project should parse")
                )
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
        assert!(session.import_legacy_project_v01(sample_project()).ok);
        assert!(
            session
                .apply_control_event(set_control_request("value_1", "in", f32_value(32.0)))
                .ok
        );
        let before = session.snapshot();

        let response =
            session.apply_control_event(control_request("value_1", "in", ControlValue::bool(true)));

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
        assert!(session.import_legacy_project_v01(sample_project()).ok);
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
        assert!(session.import_legacy_project_v01(sample_project()).ok);
        assert!(
            session
                .apply_control_event(set_control_request("value_1", "in", f32_value(32.0)))
                .ok
        );

        let response = session.apply_patch(set_value_patch("1", 0.75));

        assert_active_v01_graph_patch_rejected(&response);
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

        session.import_legacy_project_v01(sample_project());
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
        let loaded = session.import_legacy_project_v01(sample_project());

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
        let loaded = session.import_legacy_project_v01(sample_project());
        assert!(loaded.ok);

        let response = session.apply_patch(set_value_patch("1", 0.75));

        assert_active_v01_graph_patch_rejected(&response);
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

        let response = session.import_legacy_project_v01(unresolved_project());

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
        let loaded = session.import_legacy_project_v01(sample_project_with_unresolved_definition());
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

        assert_active_v01_graph_patch_rejected(&response);
        assert!(response.snapshot.loaded());
        assert_eq!(patch_graph(&response).revision, "1");
    }

    #[test]
    fn patch_with_wrong_base_revision_conflicts_without_mutating_session() {
        let mut session = RuntimeSession::default();
        session.import_legacy_project_v01(sample_project());

        let response = session.apply_patch(set_value_patch("0", 0.75));
        let snapshot = session.snapshot();

        assert_active_v01_graph_patch_rejected(&response);
        assert!(latest_history_entry(&response).is_none());
        assert!((response.history.entries).is_empty());
        assert_eq!(patch_graph(&response).revision, "1");
        assert_eq!(snapshot.graph_revision(), Some("1"));
        assert_eq!(snapshot.session_revision, 1);
    }

    #[test]
    fn invalid_patch_operations_do_not_mutate_session() {
        let mut session = RuntimeSession::default();
        session.import_legacy_project_v01(sample_project());

        let duplicate = session.apply_patch(duplicate_edge_patch());
        let missing = session.apply_patch(missing_node_patch());
        let snapshot = session.snapshot();

        assert_active_v01_graph_patch_rejected(&duplicate);
        assert!(latest_history_entry(&duplicate).is_none());
        assert!((duplicate.history.entries).is_empty());
        assert_active_v01_graph_patch_rejected(&missing);
        assert_eq!(snapshot.graph_revision(), Some("1"));
        assert_eq!(snapshot.session_revision, 1);
    }

    #[test]
    fn incompatible_patch_result_does_not_mutate_session() {
        let mut session = RuntimeSession::default();
        session.import_legacy_project_v01(sample_project());

        let response = session.apply_patch(incompatible_edge_patch());
        let snapshot = session.snapshot();

        assert_active_v01_graph_patch_rejected(&response);
        assert_eq!(snapshot.graph_revision(), Some("1"));
        assert_eq!(snapshot.session_revision, 1);
    }

    #[test]
    fn registry_invalid_patch_result_does_not_mutate_session() {
        let mut session = RuntimeSession::default();
        session.import_legacy_project_v01(sample_project());

        let response = session.apply_patch(missing_definition_node_patch());
        let snapshot = session.snapshot();

        assert_active_v01_graph_patch_rejected(&response);
        assert_eq!(snapshot.graph_revision(), Some("1"));
        assert_eq!(snapshot.session_revision, 1);
    }

    #[test]
    fn remove_node_patch_removes_incident_edges() {
        let mut session = RuntimeSession::default();
        session.import_legacy_project_v01(sample_project());

        let response = session.apply_patch(graph_patch(json!({
          "schema": "skenion.graph.patch",
          "schemaVersion": "0.1.0",
          "id": "remove-node",
          "baseRevision": "1",
          "ops": [
            { "op": "removeNode", "nodeId": "value_1" }
          ]
        })));

        assert_active_v01_graph_patch_rejected(&response);
        let graph = patch_graph(&response);
        assert_eq!(graph.revision, "1");
        assert!(graph.nodes.iter().any(|node| node.id == "value_1"));
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn patch_non_numeric_revision_gets_suffix() {
        let mut project = sample_project();
        project.graph.revision = "rev_0001".to_owned();
        let mut session = RuntimeSession::default();
        session.import_legacy_project_v01(project);

        let response = session.apply_patch(set_value_patch("rev_0001", 0.75));

        assert_active_v01_graph_patch_rejected(&response);
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
        session.import_legacy_project_v01(sample_project());
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
        session.import_legacy_project_v01(sample_project());
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
        let loaded = session.import_legacy_project_v01(sample_project());
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
        assert!(session.import_legacy_project_v01(sample_project()).ok);

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
        let loaded = session.import_legacy_project_v01(sample_project());
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
    fn view_patch_helper_reports_missing_view_and_from_mismatch() {
        let mut session = RuntimeSession::default();
        let loaded = session.import_legacy_project_v01(sample_project());
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
        let loaded = session.import_legacy_project_v01(sample_project());
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

        assert_active_v01_graph_patch_rejected(&response);
        assert_eq!(response.snapshot.graph_revision(), Some("1"));
        assert_eq!(response.snapshot.view_revision, 1);
        assert!(latest_history_entry(&response).is_none());
    }

    #[test]
    fn new_patch_after_undo_clears_redo_stack() {
        let mut session = RuntimeSession::default();
        session.import_legacy_project_v01(sample_project());
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
    fn active_v01_remove_node_patch_is_rejected_without_history() {
        let mut session = RuntimeSession::default();
        session.import_legacy_project_v01(sample_project());
        let removed = session.apply_patch(graph_patch(json!({
          "schema": "skenion.graph.patch",
          "schemaVersion": "0.1.0",
          "id": "remove-node",
          "baseRevision": "1",
          "ops": [
            { "op": "removeNode", "nodeId": "value_1" }
          ]
        })));

        assert_active_v01_graph_patch_rejected(&removed);
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
    fn active_v01_connection_and_delete_patches_are_rejected_without_history() {
        let mut project = sample_project();
        project.graph.edges.clear();
        let mut session = RuntimeSession::default();
        assert!(session.import_legacy_project_v01(project).ok);
        let connected = session.apply_patch(duplicate_edge_patch());
        assert_active_v01_graph_patch_rejected(&connected);
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
        assert_active_v01_graph_patch_rejected(&deleted);
        assert!(patch_graph(&deleted).edges.is_empty());
        assert_eq!(deleted.history.undo_depth, 0);
    }

    #[test]
    fn multiple_undo_operations_keep_advancing_revision() {
        let mut session = RuntimeSession::default();
        session.import_legacy_project_v01(sample_project());
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
        invalid_inverse.import_legacy_project_v01(sample_project());
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
        invalid_redo.import_legacy_project_v01(sample_project());
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
            Some("project.active-v0.1-graph-patch-unsupported")
        );

        let mut no_actor_history = RuntimeSession::default();
        no_actor_history.import_legacy_project_v01(sample_project());
        let no_actor_undo = no_actor_history.undo_for_actor("participant-a");
        let no_actor_redo = no_actor_history.redo_for_actor("participant-a");
        assert!(!no_actor_undo.ok);
        assert!(no_actor_undo.diagnostics[0].message.contains("actor"));
        assert!(!no_actor_redo.ok);
        assert!(no_actor_redo.diagnostics[0].message.contains("actor"));

        let mut invalid_actor_inverse = RuntimeSession::default();
        invalid_actor_inverse.import_legacy_project_v01(sample_project());
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
        invalid_actor_redo.import_legacy_project_v01(sample_project());
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
        session.import_legacy_project_v01(sample_project());

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
        session.import_legacy_project_v01(sample_project());

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
    fn paste_graph_fragment_remaps_past_existing_generated_ids() {
        let mut session = RuntimeSession::default();
        session.import_legacy_project_v01(sample_project());
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
        session.import_legacy_project_v01(sample_project());
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
        session.import_legacy_project_v01(sample_project());
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
        session.import_legacy_project_v01(sample_project());
        let mut operation = paste_operation("1");
        operation.request.options = Some(skenion_contracts::PasteGraphFragmentOptions {
            outside_endpoint_policy: None,
            id_conflict_policy: Some(skenion_contracts::IdConflictPolicy::Reject),
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
    fn paste_graph_fragment_reports_apply_mutation_failures_as_operation_diagnostics() {
        let mut session = RuntimeSession::default();
        session.import_legacy_project_v01(sample_project());
        let mut operation = paste_operation("1");
        operation.request.fragment.nodes[1].kind = "missing.kind".to_owned();

        let response = session.apply_runtime_operation(operation);

        assert!(!response.ok);
        assert!(!response.applied);
        assert_eq!(response.history_entry_id, None);
        assert_eq!(response.revision_after, None);
        assert_eq!(response.diagnostics[0].code, "paste.lowering.failed");
        assert!(
            response.diagnostics[0]
                .message
                .contains("missing node definition")
        );
    }

    #[test]
    fn paste_operation_validation_reports_fragment_analysis_errors() {
        let mut session = RuntimeSession::default();
        session.load_project_v02(sample_project_v02());
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
            Some(skenion_contracts::GraphFragmentViewV02 { nodes: None });
        let patch = lower_fragment_view_patch(1, &operation.request, &BTreeMap::new());
        assert!(patch.is_none());

        let edge = EdgeSpecV02 {
            id: "edge".to_owned(),
            source: EdgeEndpointV02 {
                node_id: "outside_source".to_owned(),
                port_id: "out".to_owned(),
            },
            target: EdgeEndpointV02 {
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
        });
        assert_eq!(warning.severity, crate::DiagnosticSeverity::Warning);
        assert_eq!(warning.code.as_deref(), Some("paste.warning"));
        assert_eq!(info.severity, crate::DiagnosticSeverity::Info);
        assert_eq!(info.code.as_deref(), Some("paste.info"));
    }

    #[test]
    fn v02_active_cutover_private_helpers_cover_defensive_paths() {
        let root_target = paste_operation("1").request.target;
        let change: RuntimeCollaborationChange = serde_json::from_value(json!({
          "op": "node.add",
          "changeId": "change-add-duplicate-value",
          "node": {
            "id": "value_1",
            "kind": "core.float",
            "kindVersion": "0.2.0",
            "params": {},
            "ports": value_f32_ports_v02_json()
          }
        }))
        .expect("collaboration change should parse");

        let mut unloaded = RuntimeSession::default();
        let no_project = unloaded.apply_collaboration_change_set_v02(
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

        let active_v01 = unloaded.load_project(sample_project());
        assert_eq!(
            active_v01.diagnostics[0].code.as_deref(),
            Some("project.active-v0.1-unsupported")
        );

        let mut invalid_request = sample_project_v02();
        let mut invalid_document = super::project_document_from_request_v02(&invalid_request);
        invalid_document.schema_version = "0.1.0".to_owned();
        invalid_request.document = Some(invalid_document);
        let invalid_document_response = unloaded.load_project_v02(invalid_request);
        assert_eq!(
            invalid_document_response.diagnostics[0].code.as_deref(),
            Some("project.invalid-v0.2")
        );

        let mut session = RuntimeSession::default();
        assert!(session.load_project_v02(sample_project_v02()).ok);
        assert!(session.project_document_v02().is_some());
        assert_eq!(
            session.target_revision_v02(&root_target).as_deref(),
            Some("1")
        );

        let mut stale_target = root_target.clone();
        stale_target.base_revision = "0".to_owned();
        let stale = session.apply_collaboration_change_set_v02(
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
        let missing_target = session.apply_collaboration_change_set_v02(
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

        let duplicate = session.apply_collaboration_change_set_v02(
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
        let paste_error = super::paste_graph_fragment_into_project_v02(
            super::project_document_from_request_v02(&sample_project_v02()),
            1,
            &PasteGraphFragmentRequest {
                target: unsupported_target,
                fragment: paste_operation("1").request.fragment,
                placement: None,
                options: None,
            },
        )
        .expect_err("package patch target should not be mutable");
        assert_eq!(paste_error.0[0].code, "paste.target.missing-graph");

        let unresolved = super::unresolved_object_diagnostics(&GraphDocument {
            schema: "skenion.graph".to_owned(),
            schema_version: "0.1.0".to_owned(),
            id: "unresolved-defaults".to_owned(),
            revision: "1".to_owned(),
            nodes: vec![
                serde_json::from_value(json!({
                  "id": "missing_object",
                  "kind": "core.unresolved-object",
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
    fn paste_graph_fragment_applies_position_and_anchor_placement() {
        let mut positioned = RuntimeSession::default();
        positioned.import_legacy_project_v01(sample_project());
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
        anchored.import_legacy_project_v01(sample_project());
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
        session.import_legacy_project_v01(sample_project());

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
        session.import_legacy_project_v01(sample_project());
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
        session.import_legacy_project_v01(sample_project());
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
        session.import_legacy_project_v01(sample_project());
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
        session.import_legacy_project_v01(sample_project());
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
        session.import_legacy_project_v01(sample_project());
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
        session.import_legacy_project_v01(sample_project());
        let mut operation = paste_operation("1");
        operation.request.fragment.edges[0].target.node_id = "outside".to_owned();
        operation.request.options = Some(skenion_contracts::PasteGraphFragmentOptions {
            outside_endpoint_policy: Some(
                skenion_contracts::GraphFragmentOutsideEndpointPolicyV02::Omit,
            ),
            id_conflict_policy: Some(skenion_contracts::IdConflictPolicy::Remap),
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
        session.import_legacy_project_v01(sample_project());
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
    fn paste_graph_fragment_converts_v02_port_rates_for_lowered_v01_nodes() {
        let cases = [
            (
                json!({ "id": "event", "direction": "input", "type": "message.any", "rate": "event", "triggerMode": "trigger" }),
                crate::DataFlow::Event,
                "message.any",
                Some(crate::PortActivation::Trigger),
            ),
            (
                json!({ "id": "audio", "direction": "output", "type": "signal.audio", "rate": "audio" }),
                crate::DataFlow::Signal,
                "signal.audio",
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
                json!({ "id": "render", "direction": "input", "type": "value.number", "rate": "render", "triggerMode": "passive" }),
                crate::DataFlow::Value,
                "number.float",
                Some(crate::PortActivation::Latched),
            ),
            (
                json!({ "id": "gpu", "direction": "input", "type": "value.color", "rate": "gpu", "triggerMode": "latched" }),
                crate::DataFlow::Value,
                "color",
                Some(crate::PortActivation::Latched),
            ),
            (
                json!({ "id": "texture", "direction": "output", "type": "gpu.texture2d", "rate": "gpu" }),
                crate::DataFlow::Resource,
                "gpu.texture2d",
                None,
            ),
            (
                json!({ "id": "default", "direction": "input", "type": "message.any" }),
                crate::DataFlow::Event,
                "message.any",
                None,
            ),
        ];

        for (value, expected_flow, expected_kind, expected_activation) in cases {
            let port: PortSpecV02 = serde_json::from_value(value).expect("port should parse");
            let lowered = port_v02_to_v01(&port);
            assert_eq!(lowered.data_type.flow, expected_flow);
            assert_eq!(lowered.data_type.data_kind, expected_kind);
            assert_eq!(lowered.activation, expected_activation);
        }
    }

    fn graph_patch(value: Value) -> GraphPatch {
        serde_json::from_value(value).expect("patch should parse")
    }

    fn patch_graph(response: &RuntimePatchResponse) -> &GraphDocumentV02 {
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

    fn assert_active_v01_graph_patch_rejected(response: &RuntimePatchResponse) {
        assert!(!response.ok);
        assert!(!response.applied);
        assert!(!response.conflict);
        assert_eq!(
            response.diagnostics[0].code.as_deref(),
            Some("project.active-v0.1-graph-patch-unsupported")
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
          "schemaVersion": "0.2.0",
          "nodes": [
            {
              "id": "value_1",
              "kind": "core.float",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": value_f32_ports_v02_json()
            },
            {
              "id": "pasted_target",
              "kind": "core.float",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": value_f32_ports_v02_json()
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

    fn sample_project() -> ProjectRequest {
        serde_json::from_value(sample_project_json()).expect("sample project should parse")
    }

    fn sample_project_v02() -> ProjectRequestV02 {
        serde_json::from_value(json!({
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.2.0",
            "id": "minimal-value",
            "revision": "1",
            "nodes": [
              {
                "id": "value_1",
                "kind": "core.float",
                "kindVersion": "0.2.0",
                "params": {},
                "ports": value_f32_ports_v02_json()
              },
              {
                "id": "target_1",
                "kind": "core.float",
                "kindVersion": "0.2.0",
                "params": {},
                "ports": value_f32_ports_v02_json()
              }
            ],
            "edges": [
              {
                "id": "edge_value_target",
                "source": { "nodeId": "value_1", "portId": "value" },
                "target": { "nodeId": "target_1", "portId": "cold" },
                "resolvedType": "number.float"
              }
            ]
          },
          "nodes": [
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.2.0",
              "id": "core.float",
              "version": "0.2.0",
              "displayName": "Float Value",
              "category": "Values",
              "ports": value_f32_ports_v02_json(),
              "execution": { "model": "value" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": []
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
        .expect("v0.2 sample project should parse")
    }

    fn sample_project_with_unresolved_definition() -> ProjectRequest {
        let mut value = sample_project_json();
        value["nodes"]
            .as_array_mut()
            .expect("nodes should be an array")
            .push(unresolved_definition_json());
        serde_json::from_value(value).expect("sample project should parse")
    }

    fn unresolved_project() -> ProjectRequest {
        let mut value = sample_project_json();
        value["graph"]["nodes"]
            .as_array_mut()
            .expect("graph nodes should be an array")
            .push(unresolved_node_json("unresolved_1", "user.manipulator"));
        value["nodes"]
            .as_array_mut()
            .expect("nodes should be an array")
            .push(unresolved_definition_json());
        serde_json::from_value(value).expect("unresolved project should parse")
    }

    fn object_routing_project() -> ProjectRequest {
        let mut value = sample_project_json();
        value["graph"]["nodes"][0]["params"] = json!({ "sendName": "speed" });
        serde_json::from_value(value).expect("object routing project should parse")
    }

    fn sample_project_json() -> Value {
        json!({
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": "minimal-value",
            "revision": "1",
            "nodes": [
              {
                "id": "value_1",
                "kind": "core.float",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": value_f32_ports_json()
              },
              {
                "id": "target_1",
                "kind": "core.float",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": value_f32_ports_json()
              }
            ],
            "edges": [
              { "from": { "node": "value_1", "port": "value" }, "to": { "node": "target_1", "port": "in" } }
            ]
          },
          "nodes": [
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "core.float",
              "version": "0.1.0",
              "displayName": "Float Value",
              "category": "Values",
              "ports": value_f32_ports_json(),
              "execution": { "model": "value" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": []
            }
          ]
        })
    }

    fn value_f32_ports_json() -> Value {
        json!([
          {
            "id": "in",
            "direction": "input",
            "label": "In",
            "type": { "flow": "event", "dataKind": "message.any" },
            "required": false,
            "activation": "trigger"
          },
          {
            "id": "cold",
            "direction": "input",
            "label": "Cold",
            "type": { "flow": "value", "dataKind": "number.float" },
            "required": false,
            "activation": "latched"
          },
          {
            "id": "value",
            "direction": "output",
            "label": "Value",
            "type": { "flow": "value", "dataKind": "number.float" }
          }
        ])
    }

    fn value_f32_ports_v02_json() -> Value {
        json!([
          {
            "id": "in",
            "direction": "input",
            "label": "In",
            "type": "message.any",
            "rate": "event",
            "required": false,
            "triggerMode": "trigger"
          },
          {
            "id": "cold",
            "direction": "input",
            "label": "Cold",
            "type": "number.float",
            "rate": "control",
            "required": false,
            "triggerMode": "latched"
          },
          {
            "id": "value",
            "direction": "output",
            "label": "Value",
            "type": "number.float",
            "rate": "control"
          }
        ])
    }

    fn unresolved_node_json(id: &str, object_text: &str) -> Value {
        json!({
          "id": id,
          "kind": "core.unresolved-object",
          "kindVersion": "0.1.0",
          "params": {
            "objectText": object_text,
            "diagnosticMessage": format!("{object_text} is not available in the local runtime registry."),
            "requestedKind": object_text
          },
          "ports": []
        })
    }

    fn unresolved_definition_json() -> Value {
        json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": "core.unresolved-object",
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
