use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{
    CanvasNodeView, ControlState, DummyExecutionReport, ExecutionPlan, GraphDocument, GraphPatch,
    GraphPatchEvent, GraphPatchEventKind, NodeDefinition, NodeRegistry, PreviewContext,
    PreviewControlStateSnapshot, ProjectRequest, RuntimeControlEventRequest,
    RuntimeControlEventResponse, RuntimeControlReadRequest, RuntimeControlReadResponse,
    RuntimeControlReadTarget, RuntimeControlStateResponse, RuntimeDiagnostic, ViewState,
    apply_graph_patch, build_execution_plan, create_default_view_state_for_graph,
    invert_graph_patch, read_graph_param, read_graph_port, run_dummy_execution,
    server::{registry_from_nodes, validate_graph_with_registry},
};

const UNRESOLVED_OBJECT_NODE_KIND: &str = "core.unresolved-object";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSessionSnapshot {
    pub session_revision: u64,
    pub view_revision: u64,
    pub control_revision: u64,
    pub project: Option<RuntimeProjectSnapshot>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
    pub plan: Option<ExecutionPlan>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeProjectSnapshot {
    pub graph: GraphDocument,
    pub view_state: ViewState,
    pub nodes: Vec<NodeDefinition>,
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
    pub graph_patch: Option<GraphPatch>,
    #[serde(default)]
    pub view_patch: Option<RuntimeViewPatch>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
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
        mutation: RuntimeMutationRequest,
        inverse_mutation: RuntimeMutationRequest,
    },
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
        let project = match (
            self.graph.clone(),
            self.view_state.clone(),
            self.registry.clone(),
        ) {
            (Some(graph), Some(view_state), Some(registry)) => Some(RuntimeProjectSnapshot {
                graph,
                view_state: runtime_owned_view_state(view_state),
                nodes: registry.definitions().cloned().collect(),
            }),
            _ => None,
        };
        RuntimeSessionSnapshot {
            session_revision: self.revision,
            view_revision: self.view_revision,
            control_revision: self.control_revision,
            project,
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

    pub fn load_project(&mut self, request: ProjectRequest) -> RuntimeSessionResponse {
        let ProjectRequest {
            graph,
            nodes,
            view_state,
        } = request;
        let registry = match registry_from_nodes(nodes) {
            Ok(registry) => registry,
            Err(diagnostics) => return self.response(false, diagnostics, None),
        };

        if let Err(diagnostics) = validate_graph_with_registry(&graph, &registry) {
            return self.response(false, diagnostics, None);
        }

        let diagnostics = unresolved_object_diagnostics(&graph);
        let plan = build_execution_plan(&graph, &registry).expect("validated project should plan");
        let control_state = ControlState::from_graph(&graph);
        let view_state = reconcile_view_state_with_graph(&graph, view_state);
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

    pub fn validate_current(&mut self) -> RuntimeSessionResponse {
        let diagnostics = match self.loaded_project() {
            Some((graph, registry)) => match validate_graph_with_registry(graph, registry) {
                Ok(()) => unresolved_object_diagnostics(graph),
                Err(diagnostics) => diagnostics,
            },
            None => vec![RuntimeDiagnostic::error(
                "no project loaded in runtime session",
            )],
        };
        let ok = diagnostics.is_empty();
        self.diagnostics = diagnostics.clone();
        self.response(ok, diagnostics, None)
    }

    pub fn plan_current(&mut self) -> RuntimeSessionResponse {
        let (graph, registry) = match self.loaded_project() {
            Some(project) => project,
            None => {
                let diagnostics = vec![RuntimeDiagnostic::error(
                    "no project loaded in runtime session",
                )];
                self.diagnostics = diagnostics.clone();
                return self.response(false, diagnostics, None);
            }
        };

        if let Err(diagnostics) = validate_graph_with_registry(graph, registry) {
            self.diagnostics = diagnostics.clone();
            self.plan = None;
            return self.response(false, diagnostics, None);
        }

        let diagnostics = unresolved_object_diagnostics(graph);
        let plan = build_execution_plan(graph, registry).expect("validated project should plan");
        self.plan = Some(plan);
        self.diagnostics = diagnostics.clone();
        self.response(true, diagnostics, None)
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
            client_id: None,
            description: None,
        })
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

    pub fn reject_patch(
        &self,
        conflict: bool,
        diagnostics: Vec<RuntimeDiagnostic>,
    ) -> RuntimePatchResponse {
        self.patch_response(false, false, conflict, diagnostics)
    }

    pub fn clear(&mut self) -> RuntimeSessionResponse {
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

    pub fn view_state(&self) -> Option<ViewState> {
        self.view_state.clone()
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

        let mut next_graph = graph.clone();
        let mut inverse_graph_patch = None;
        let mut graph_event = None;
        let mut graph_changed = false;
        if let Some(graph_patch) = mutation.graph_patch.as_mut() {
            if graph_patch.base_revision != graph.revision {
                return self.patch_response(
                    false,
                    false,
                    true,
                    vec![RuntimeDiagnostic::error(format!(
                        "patch baseRevision {} does not match session graph revision {}",
                        graph_patch.base_revision, graph.revision
                    ))],
                );
            }
            let next_revision = next_graph_revision(&graph.revision);
            let mut inverse_patch = match invert_graph_patch(&graph, graph_patch) {
                Ok(inverse_patch) => inverse_patch,
                Err(error) => {
                    return self.patch_response(
                        false,
                        false,
                        false,
                        vec![RuntimeDiagnostic::error(error.to_string())],
                    );
                }
            };
            next_graph = apply_graph_patch(&graph, graph_patch, Some(&next_revision))
                .expect("patch inversion preflight should make graph patch application infallible");
            if let Err(diagnostics) = validate_graph_with_registry(&next_graph, &registry) {
                return self.patch_response(false, false, false, diagnostics);
            }
            inverse_patch.base_revision = next_revision.clone();
            graph_event = Some(self.create_patch_event(
                match kind {
                    RuntimeHistoryEntryKind::Apply => GraphPatchEventKind::Apply,
                    RuntimeHistoryEntryKind::Undo => GraphPatchEventKind::Undo,
                    RuntimeHistoryEntryKind::Redo => GraphPatchEventKind::Redo,
                },
                graph_patch.clone(),
                inverse_patch.clone(),
                graph.revision.clone(),
                next_revision,
                subject_event_id.clone(),
            ));
            inverse_graph_patch = Some(inverse_patch);
            graph_changed = true;
        }

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

        if !graph_changed && !view_changed {
            return self.patch_response(true, false, false, Vec::new());
        }

        let diagnostics = unresolved_object_diagnostics(&next_graph);
        let plan =
            build_execution_plan(&next_graph, &registry).expect("validated project should plan");
        let control_state = ControlState::from_graph(&next_graph);
        let mut inverse_mutation = RuntimeMutationRequest {
            graph_patch: inverse_graph_patch,
            view_patch: inverse_view_patch,
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
            graph_event.clone(),
        );
        let history_stack_entry = HistoryEntry::Mutation {
            event_id: history_entry.id.clone(),
            mutation,
            inverse_mutation,
        };

        self.graph = Some(next_graph.clone());
        self.registry = Some(registry);
        self.plan = Some(plan);
        self.view_state = Some(next_view_state);
        if view_changed {
            self.view_revision += 1;
        }
        self.control_state = control_state;
        if graph_changed {
            self.control_revision = 0;
        }
        self.diagnostics = diagnostics.clone();
        self.revision += 1;
        self.history_entries.push(history_entry);
        if matches!(kind, RuntimeHistoryEntryKind::Apply) {
            self.undo_stack.push(history_stack_entry);
            self.redo_stack.clear();
        }

        self.patch_response(true, true, false, diagnostics)
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
        }
    }

    fn create_patch_event(
        &mut self,
        kind: GraphPatchEventKind,
        patch: GraphPatch,
        inverse_patch: GraphPatch,
        revision_before: String,
        revision_after: String,
        subject_event_id: Option<String>,
    ) -> GraphPatchEvent {
        let sequence = self.next_event_sequence;
        self.next_event_sequence += 1;
        GraphPatchEvent {
            schema: "skenion.graph.patch.event".to_owned(),
            schema_version: "0.1.0".to_owned(),
            id: format!("event_{sequence:06}"),
            sequence,
            kind,
            client_id: patch.client_id.clone(),
            description: patch.description.clone(),
            patch,
            inverse_patch,
            revision_before,
            revision_after,
            subject_event_id,
            created_at: created_at_now(),
        }
    }

    fn create_runtime_history_entry(
        &mut self,
        kind: RuntimeHistoryEntryKind,
        mutation: RuntimeMutationRequest,
        inverse_mutation: RuntimeMutationRequest,
        subject_event_id: Option<String>,
        graph_event: Option<GraphPatchEvent>,
    ) -> RuntimeHistoryEntry {
        if let Some(event) = graph_event {
            let GraphPatchEvent {
                id,
                sequence,
                client_id,
                description,
                created_at,
                ..
            } = event;
            return RuntimeHistoryEntry {
                id,
                sequence,
                kind,
                client_id: mutation.client_id.clone().or(client_id),
                description: mutation.description.clone().or(description),
                mutation,
                inverse_mutation,
                subject_event_id,
                created_at,
            };
        }

        let sequence = self.next_event_sequence;
        self.next_event_sequence += 1;
        RuntimeHistoryEntry {
            id: format!("runtime_event_{sequence:06}"),
            sequence,
            kind,
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

fn next_graph_revision(current: &str) -> String {
    current
        .parse::<u64>()
        .map(|revision| (revision + 1).to_string())
        .unwrap_or_else(|_| format!("{current}+1"))
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

fn created_at_now() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("unix-ms:{millis}")
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::{
        ControlMessage, ControlValue, Edge, GraphDocument, GraphPatch, NodeRegistry, PortRef,
        ProjectRequest, RuntimeControlEmission, RuntimeControlEventRequest,
        RuntimeControlReadRequest, RuntimeControlReadTarget, RuntimeDiagnostic, ViewState,
    };

    use super::{
        HistoryEntry, RuntimeHistoryEntryKind, RuntimeMutationRequest, RuntimePatchResponse,
        RuntimeSession, RuntimeViewPatch, RuntimeViewPatchOperation,
    };

    #[test]
    fn invalid_registry_load_returns_diagnostics_without_revision_change() {
        let mut session = RuntimeSession::default();
        let mut request = sample_project();
        request.nodes[0].schema_version = "9.9.9".to_owned();

        let response = session.load_project(request);

        assert!(!response.ok);
        assert!(!response.snapshot.loaded());
        assert_eq!(response.snapshot.session_revision, 0);
        assert!(
            response.diagnostics[0]
                .message
                .contains("invalid node definition")
        );
    }

    #[test]
    fn validate_and_plan_fail_without_loaded_project() {
        let mut session = RuntimeSession::default();

        let validation = session.validate_current();
        let plan = session.plan_current();

        assert!(!validation.ok);
        assert!(!plan.ok);
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
                .contains("missing node definition")
        );
    }

    #[test]
    fn validate_current_reports_invalid_stored_project() {
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
                .contains("missing node definition")
        );
    }

    #[test]
    fn run_current_rebuilds_missing_plan() {
        let mut session = RuntimeSession::default();
        let loaded = session.load_project(sample_project());
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
                .contains("missing node definition")
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
        assert!(session.load_project(sample_project()).ok);

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
        assert!(session.load_project(object_routing_project()).ok);

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
        assert!(session.load_project(project).ok);
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

        assert!(session.load_project(sample_project()).ok);
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
                .load_project(
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
        assert!(session.load_project(sample_project()).ok);
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
        assert!(session.load_project(sample_project()).ok);
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
        assert!(session.load_project(sample_project()).ok);
        assert!(
            session
                .apply_control_event(set_control_request("value_1", "in", f32_value(32.0)))
                .ok
        );

        let response = session.apply_patch(set_value_patch("1", 0.75));

        assert!(response.ok);
        assert_eq!(
            session.control_state_response().values.get("value_1"),
            Some(&ControlValue::float(0.75))
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

        session.load_project(sample_project());
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
        let loaded = session.load_project(sample_project());

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
        let loaded = session.load_project(sample_project());
        assert!(loaded.ok);

        let response = session.apply_patch(set_value_patch("1", 0.75));

        assert!(response.ok);
        assert!(response.applied);
        assert!(!response.conflict);
        let entry = latest_history_entry(&response).unwrap();
        assert_eq!(entry.kind, RuntimeHistoryEntryKind::Apply);
        assert_eq!(entry.sequence, 1);
        assert_eq!(
            entry.mutation.graph_patch.as_ref().unwrap().base_revision,
            "1"
        );
        assert_eq!(
            entry
                .inverse_mutation
                .graph_patch
                .as_ref()
                .unwrap()
                .base_revision,
            "2"
        );
        assert_eq!((response.history.entries).len(), 1);
        assert_eq!(response.history.undo_depth, 1);
        assert_eq!(response.history.redo_depth, 0);
        assert_eq!(patch_graph(&response).revision, "2");
        assert_eq!(response.snapshot.graph_revision(), Some("2"));
        assert_eq!(response.snapshot.session_revision, 2);
        assert_eq!(response.snapshot.plan.as_ref().unwrap().graph_revision, "2");
        assert_eq!(
            patch_graph(&response).nodes[0].params["value"],
            Value::from(0.75)
        );
        assert_eq!(
            session.control_state.value_for_node("value_1"),
            Some(&ControlValue::float(0.75))
        );
    }

    #[test]
    fn unresolved_object_loads_session_with_error_diagnostic() {
        let mut session = RuntimeSession::default();

        let response = session.load_project(unresolved_project());

        assert!(response.ok);
        assert!(response.snapshot.loaded());
        assert_eq!(response.diagnostics.len(), 1);
        assert!(
            response.diagnostics[0]
                .message
                .contains("unresolved object user.manipulator")
        );
        assert_eq!(session.snapshot().diagnostics, response.diagnostics);

        let plan = session.plan_current();
        assert!(plan.ok);
        assert_eq!(plan.diagnostics.len(), 1);
    }

    #[test]
    fn replace_node_with_unresolved_object_applies_with_error_diagnostic() {
        let mut session = RuntimeSession::default();
        let loaded = session.load_project(sample_project_with_unresolved_definition());
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

        assert!(response.ok);
        assert!(response.applied);
        assert!(response.snapshot.loaded());
        assert_eq!(patch_graph(&response).revision, "2");
        assert!(patch_graph(&response).edges.is_empty());
        assert_eq!(response.diagnostics.len(), 1);
        assert!(
            response.diagnostics[0]
                .message
                .contains("unresolved object user.manipulator")
        );
        assert_eq!(session.snapshot().diagnostics, response.diagnostics);
    }

    #[test]
    fn patch_with_wrong_base_revision_conflicts_without_mutating_session() {
        let mut session = RuntimeSession::default();
        session.load_project(sample_project());

        let response = session.apply_patch(set_value_patch("0", 0.75));
        let snapshot = session.snapshot();

        assert!(!response.ok);
        assert!(!response.applied);
        assert!(response.conflict);
        assert!(latest_history_entry(&response).is_none());
        assert!((response.history.entries).is_empty());
        assert_eq!(patch_graph(&response).revision, "1");
        assert_eq!(snapshot.graph_revision(), Some("1"));
        assert_eq!(snapshot.session_revision, 1);
        assert!(
            response.diagnostics[0]
                .message
                .contains("does not match session graph revision")
        );
    }

    #[test]
    fn invalid_patch_operations_do_not_mutate_session() {
        let mut session = RuntimeSession::default();
        session.load_project(sample_project());

        let duplicate = session.apply_patch(duplicate_edge_patch());
        let missing = session.apply_patch(missing_node_patch());
        let snapshot = session.snapshot();

        assert!(!duplicate.ok);
        assert!(!duplicate.applied);
        assert!(!duplicate.conflict);
        assert!(latest_history_entry(&duplicate).is_none());
        assert!((duplicate.history.entries).is_empty());
        assert!(duplicate.diagnostics[0].message.contains("already exists"));
        assert!(!missing.ok);
        assert!(missing.diagnostics[0].message.contains("does not exist"));
        assert_eq!(snapshot.graph_revision(), Some("1"));
        assert_eq!(snapshot.session_revision, 1);
    }

    #[test]
    fn incompatible_patch_result_does_not_mutate_session() {
        let mut session = RuntimeSession::default();
        session.load_project(sample_project());

        let response = session.apply_patch(incompatible_edge_patch());
        let snapshot = session.snapshot();

        assert!(!response.ok);
        assert!(!response.applied);
        assert!(!response.conflict);
        let diagnostics = response
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        assert!(
            diagnostics.contains("incompatible edge")
                || diagnostics.contains("output")
                || diagnostics.contains("not an input port"),
            "{diagnostics}"
        );
        assert_eq!(snapshot.graph_revision(), Some("1"));
        assert_eq!(snapshot.session_revision, 1);
    }

    #[test]
    fn registry_invalid_patch_result_does_not_mutate_session() {
        let mut session = RuntimeSession::default();
        session.load_project(sample_project());

        let response = session.apply_patch(missing_definition_node_patch());
        let snapshot = session.snapshot();

        assert!(!response.ok);
        assert!(!response.applied);
        assert!(!response.conflict);
        assert!(
            response.diagnostics[0]
                .message
                .contains("missing node definition")
        );
        assert_eq!(snapshot.graph_revision(), Some("1"));
        assert_eq!(snapshot.session_revision, 1);
    }

    #[test]
    fn remove_node_patch_removes_incident_edges() {
        let mut session = RuntimeSession::default();
        session.load_project(sample_project());

        let response = session.apply_patch(graph_patch(json!({
          "schema": "skenion.graph.patch",
          "schemaVersion": "0.1.0",
          "id": "remove-node",
          "baseRevision": "1",
          "ops": [
            { "op": "removeNode", "nodeId": "value_1" }
          ]
        })));

        assert!(response.ok);
        let graph = patch_graph(&response);
        assert_eq!(graph.revision, "2");
        assert!(graph.nodes.iter().all(|node| node.id != "value_1"));
        assert!(graph.edges.is_empty());
    }

    #[test]
    fn patch_non_numeric_revision_gets_suffix() {
        let mut project = sample_project();
        project.graph.revision = "rev_0001".to_owned();
        let mut session = RuntimeSession::default();
        session.load_project(project);

        let response = session.apply_patch(set_value_patch("rev_0001", 0.75));

        assert!(response.ok);
        assert_eq!(patch_graph(&response).revision, "rev_0001+1");
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
        session.load_project(sample_project());
        let applied = session.apply_patch(set_value_patch("1", 0.75));
        let apply_event_id = latest_history_entry(&applied).unwrap().id.clone();

        let undone = session.undo();

        assert!(undone.ok);
        assert!(undone.applied);
        assert_eq!(patch_graph(&undone).revision, "3");
        assert!(!patch_graph(&undone).nodes[0].params.contains_key("value"));
        assert_eq!(undone.snapshot.session_revision, 3);
        let undo_entry = latest_history_entry(&undone).unwrap();
        assert_eq!(undo_entry.kind, RuntimeHistoryEntryKind::Undo);
        assert_eq!(
            undo_entry.subject_event_id.as_deref(),
            Some(apply_event_id.as_str())
        );
        assert_eq!(
            undo_entry
                .mutation
                .graph_patch
                .as_ref()
                .unwrap()
                .base_revision,
            "2"
        );
        assert_eq!(
            undo_entry
                .inverse_mutation
                .graph_patch
                .as_ref()
                .unwrap()
                .base_revision,
            "3"
        );
        assert_eq!((undone.history.entries).len(), 2);
        assert_eq!(undone.history.undo_depth, 0);
        assert_eq!(undone.history.redo_depth, 1);
    }

    #[test]
    fn redo_after_undo_reapplies_graph_and_records_history_entry() {
        let mut session = RuntimeSession::default();
        session.load_project(sample_project());
        session.apply_patch(set_value_patch("1", 0.75));
        session.undo();

        let redone = session.redo();

        assert!(redone.ok);
        assert!(redone.applied);
        assert_eq!(patch_graph(&redone).revision, "4");
        assert_eq!(
            patch_graph(&redone).nodes[0].params["value"],
            Value::from(0.75)
        );
        assert_eq!(redone.snapshot.session_revision, 4);
        let redo_entry = latest_history_entry(&redone).unwrap();
        assert_eq!(redo_entry.kind, RuntimeHistoryEntryKind::Redo);
        assert_eq!(
            redo_entry
                .mutation
                .graph_patch
                .as_ref()
                .unwrap()
                .base_revision,
            "3"
        );
        assert_eq!(
            redo_entry
                .inverse_mutation
                .graph_patch
                .as_ref()
                .unwrap()
                .base_revision,
            "4"
        );
        assert_eq!((redone.history.entries).len(), 3);
        assert_eq!(redone.history.undo_depth, 1);
        assert_eq!(redone.history.redo_depth, 0);
    }

    #[test]
    fn view_state_patch_undo_redo_moves_once_from_start_to_end() {
        let mut session = RuntimeSession::default();
        let loaded = session.load_project(sample_project());
        assert!(loaded.ok);
        let start = loaded
            .snapshot
            .view_state()
            .cloned()
            .expect("loaded view state");
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
        assert!(session.load_project(sample_project()).ok);

        let empty = session.apply_mutation(RuntimeMutationRequest {
            graph_patch: None,
            view_patch: None,
            client_id: None,
            description: None,
        });
        let conflict = session.apply_mutation(RuntimeMutationRequest {
            graph_patch: None,
            view_patch: Some(RuntimeViewPatch {
                base_view_revision: 99,
                ops: Vec::new(),
            }),
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
        let loaded = session.load_project(sample_project());
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
        let loaded = session.load_project(sample_project());
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
        let loaded = session.load_project(sample_project());
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
            client_id: None,
            description: Some("set graph without moving view".to_owned()),
        });

        assert!(response.ok);
        assert!(response.applied);
        assert_eq!(response.snapshot.graph_revision(), Some("2"));
        assert_eq!(response.snapshot.view_revision, 1);
        assert_eq!(
            latest_history_entry(&response)
                .unwrap()
                .inverse_mutation
                .view_patch
                .as_ref()
                .unwrap()
                .base_view_revision,
            1
        );
    }

    #[test]
    fn new_patch_after_undo_clears_redo_stack() {
        let mut session = RuntimeSession::default();
        session.load_project(sample_project());
        session.apply_patch(set_value_patch("1", 0.75));
        let undone = session.undo();
        assert_eq!(undone.history.redo_depth, 1);

        let applied = session.apply_patch(set_value_patch("3", 0.25));

        assert!(applied.ok);
        assert_eq!(patch_graph(&applied).revision, "4");
        assert_eq!((applied.history.entries).len(), 3);
        assert_eq!(applied.history.undo_depth, 1);
        assert_eq!(applied.history.redo_depth, 0);
        assert!(!applied.history.can_redo);
    }

    #[test]
    fn undo_remove_node_restores_node_and_incident_edges() {
        let mut session = RuntimeSession::default();
        session.load_project(sample_project());
        let removed = session.apply_patch(graph_patch(json!({
          "schema": "skenion.graph.patch",
          "schemaVersion": "0.1.0",
          "id": "remove-node",
          "baseRevision": "1",
          "ops": [
            { "op": "removeNode", "nodeId": "value_1" }
          ]
        })));
        assert!(patch_graph(&removed).edges.is_empty());

        let undone = session.undo();

        assert!(undone.ok);
        assert_eq!(patch_graph(&undone).revision, "3");
        assert!(
            patch_graph(&undone)
                .nodes
                .iter()
                .any(|node| node.id == "value_1")
        );
        assert_eq!(patch_graph(&undone).edges.len(), 1);
    }

    #[test]
    fn global_undo_after_remote_node_delete_restores_delete_before_connection_undo() {
        let mut project = sample_project();
        project.graph.edges.clear();
        let mut session = RuntimeSession::default();
        assert!(session.load_project(project).ok);
        let connected = session.apply_patch(duplicate_edge_patch());
        assert!(connected.ok);
        assert_eq!(patch_graph(&connected).edges.len(), 1);
        let deleted = session.apply_patch(graph_patch(json!({
          "schema": "skenion.graph.patch",
          "schemaVersion": "0.1.0",
          "id": "delete-target",
          "baseRevision": "2",
          "ops": [
            { "op": "removeNode", "nodeId": "target_1" }
          ]
        })));
        assert!(deleted.ok);
        assert!(patch_graph(&deleted).edges.is_empty());

        let undo_delete = session.undo();

        assert!(undo_delete.ok);
        assert!(
            patch_graph(&undo_delete)
                .nodes
                .iter()
                .any(|node| node.id == "target_1")
        );
        assert_eq!(patch_graph(&undo_delete).edges.len(), 1);

        let undo_connect = session.undo();

        assert!(undo_connect.ok);
        assert!(
            patch_graph(&undo_connect)
                .nodes
                .iter()
                .any(|node| node.id == "target_1")
        );
        assert!(patch_graph(&undo_connect).edges.is_empty());
    }

    #[test]
    fn multiple_undo_operations_keep_advancing_revision() {
        let mut session = RuntimeSession::default();
        session.load_project(sample_project());
        session.apply_patch(set_value_patch("1", 0.75));
        session.apply_patch(set_value_patch("2", 0.25));

        let first_undo = session.undo();
        let second_undo = session.undo();

        assert!(first_undo.ok);
        assert_eq!(patch_graph(&first_undo).revision, "4");
        assert_eq!(
            patch_graph(&first_undo).nodes[0].params["value"],
            Value::from(0.75)
        );
        assert!(second_undo.ok);
        assert_eq!(patch_graph(&second_undo).revision, "5");
        assert!(
            !patch_graph(&second_undo).nodes[0]
                .params
                .contains_key("value")
        );
        assert_eq!((second_undo.history.entries).len(), 4);
        assert_eq!(second_undo.history.redo_depth, 2);
    }

    #[test]
    fn failed_history_operations_do_not_mutate_stacks_or_session() {
        let mut no_loaded = RuntimeSession::default();
        no_loaded.undo_stack.push(HistoryEntry::Mutation {
            event_id: "event_bad".to_owned(),
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
        invalid_inverse.load_project(sample_project());
        invalid_inverse.undo_stack.push(HistoryEntry::Mutation {
            event_id: "event_bad_inverse".to_owned(),
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
        invalid_redo.load_project(sample_project());
        invalid_redo.redo_stack.push(HistoryEntry::Mutation {
            event_id: "event_bad_redo".to_owned(),
            mutation: graph_mutation(missing_definition_node_patch()),
            inverse_mutation: graph_mutation(set_value_patch("1", 0.5)),
        });
        let invalid_redo_response = invalid_redo.redo();
        assert!(!invalid_redo_response.ok);
        assert_eq!(invalid_redo_response.history.redo_depth, 1);
        assert!(
            invalid_redo_response.diagnostics[0]
                .message
                .contains("missing node definition")
        );
    }

    #[test]
    fn reject_patch_uses_current_session_snapshot() {
        let mut session = RuntimeSession::default();
        session.load_project(sample_project());

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

    fn graph_patch(value: Value) -> GraphPatch {
        serde_json::from_value(value).expect("patch should parse")
    }

    fn patch_graph(response: &RuntimePatchResponse) -> &GraphDocument {
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

    fn latest_history_entry(
        response: &RuntimePatchResponse,
    ) -> Option<&super::RuntimeHistoryEntry> {
        response.history.entries.last()
    }

    fn graph_mutation(patch: GraphPatch) -> RuntimeMutationRequest {
        RuntimeMutationRequest {
            graph_patch: Some(patch),
            view_patch: None,
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
