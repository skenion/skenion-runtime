use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{
    ControlState, DummyExecutionReport, ExecutionPlan, GraphDocument, GraphPatch, GraphPatchEvent,
    GraphPatchEventKind, GraphPatchHistory, NodeRegistry, PreviewContext,
    PreviewControlStateSnapshot, ProjectRequest, RuntimeControlEventRequest,
    RuntimeControlEventResponse, RuntimeControlReadRequest, RuntimeControlReadResponse,
    RuntimeControlReadTarget, RuntimeControlStateResponse, RuntimeDiagnostic, apply_graph_patch,
    build_execution_plan, invert_graph_patch, read_graph_param, read_graph_port,
    run_dummy_execution,
    server::{registry_from_nodes, validate_graph_with_registry},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSessionSnapshot {
    pub loaded: bool,
    pub graph_id: Option<String>,
    pub graph_revision: Option<String>,
    pub session_revision: u64,
    pub control_revision: u64,
    pub diagnostics: Vec<RuntimeDiagnostic>,
    pub plan: Option<ExecutionPlan>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSessionResponse {
    pub ok: bool,
    pub loaded: bool,
    pub graph_id: Option<String>,
    pub graph_revision: Option<String>,
    pub session_revision: u64,
    pub control_revision: u64,
    pub diagnostics: Vec<RuntimeDiagnostic>,
    pub plan: Option<ExecutionPlan>,
    pub report: Option<DummyExecutionReport>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePatchResponse {
    pub ok: bool,
    pub applied: bool,
    pub conflict: bool,
    pub graph: Option<GraphDocument>,
    pub session: RuntimeSessionResponse,
    pub event: Option<GraphPatchEvent>,
    pub history: GraphPatchHistory,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRunRequest {
    pub frames: Option<usize>,
}

#[derive(Debug)]
pub struct RuntimeSession {
    graph: Option<GraphDocument>,
    registry: Option<NodeRegistry>,
    plan: Option<ExecutionPlan>,
    control_state: ControlState,
    diagnostics: Vec<RuntimeDiagnostic>,
    revision: u64,
    control_revision: u64,
    event_log: Vec<GraphPatchEvent>,
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
            control_state: ControlState::default(),
            diagnostics: Vec::new(),
            revision: 0,
            control_revision: 0,
            event_log: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            next_event_sequence: 1,
        }
    }
}

#[derive(Debug, Clone)]
struct HistoryEntry {
    event_id: String,
    patch: GraphPatch,
    inverse_patch: GraphPatch,
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
            loaded: self.graph.is_some(),
            graph_id: self.graph.as_ref().map(|graph| graph.id.clone()),
            graph_revision: self.graph.as_ref().map(|graph| graph.revision.clone()),
            session_revision: self.revision,
            control_revision: self.control_revision,
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
        let ProjectRequest { graph, nodes } = request;
        let registry = match registry_from_nodes(nodes) {
            Ok(registry) => registry,
            Err(diagnostics) => return self.response(false, diagnostics, None),
        };

        if let Err(diagnostics) = validate_graph_with_registry(&graph, &registry) {
            return self.response(false, diagnostics, None);
        }

        let plan = build_execution_plan(&graph, &registry).expect("validated project should plan");
        let control_state = ControlState::from_graph(&graph);
        self.graph = Some(graph);
        self.registry = Some(registry);
        self.plan = Some(plan);
        self.control_state = control_state;
        self.control_revision = 0;
        self.diagnostics = Vec::new();
        self.clear_history();
        self.revision += 1;

        self.response(true, Vec::new(), None)
    }

    pub fn validate_current(&mut self) -> RuntimeSessionResponse {
        let diagnostics = match self.loaded_project() {
            Some((graph, registry)) => validate_graph_with_registry(graph, registry)
                .err()
                .unwrap_or_default(),
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

        let plan = build_execution_plan(graph, registry).expect("validated project should plan");
        self.plan = Some(plan);
        self.diagnostics = Vec::new();
        self.response(true, Vec::new(), None)
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

    pub fn apply_patch(&mut self, patch: GraphPatch) -> RuntimePatchResponse {
        let (graph, registry) = match (self.graph.clone(), self.registry.clone()) {
            (Some(graph), Some(registry)) => (graph, registry),
            _ => {
                return self.patch_response(
                    false,
                    false,
                    false,
                    None,
                    None,
                    vec![RuntimeDiagnostic::error(
                        "no project loaded in runtime session",
                    )],
                );
            }
        };

        if patch.base_revision != graph.revision {
            return self.patch_response(
                false,
                false,
                true,
                Some(graph.clone()),
                None,
                vec![RuntimeDiagnostic::error(format!(
                    "patch baseRevision {} does not match session graph revision {}",
                    patch.base_revision, graph.revision
                ))],
            );
        }

        let next_revision = next_graph_revision(&graph.revision);
        let mut inverse_patch = match invert_graph_patch(&graph, &patch) {
            Ok(inverse_patch) => inverse_patch,
            Err(error) => {
                return self.patch_response(
                    false,
                    false,
                    false,
                    Some(graph.clone()),
                    None,
                    vec![RuntimeDiagnostic::error(error.to_string())],
                );
            }
        };
        let patched_graph = apply_graph_patch(&graph, &patch, Some(&next_revision))
            .expect("patch inversion preflight should make graph patch application infallible");

        if let Err(diagnostics) = validate_graph_with_registry(&patched_graph, &registry) {
            return self.patch_response(
                false,
                false,
                false,
                Some(graph.clone()),
                None,
                diagnostics,
            );
        }

        let plan =
            build_execution_plan(&patched_graph, &registry).expect("validated project should plan");
        let control_state = ControlState::from_graph(&patched_graph);
        inverse_patch.base_revision = next_revision.clone();
        let event = self.create_patch_event(
            GraphPatchEventKind::Apply,
            patch.clone(),
            inverse_patch.clone(),
            graph.revision.clone(),
            next_revision,
            None,
        );
        let history_entry = HistoryEntry {
            event_id: event.id.clone(),
            patch,
            inverse_patch,
        };
        self.graph = Some(patched_graph.clone());
        self.registry = Some(registry);
        self.plan = Some(plan);
        self.control_state = control_state;
        self.control_revision = 0;
        self.diagnostics = Vec::new();
        self.revision += 1;
        self.event_log.push(event.clone());
        self.undo_stack.push(history_entry);
        self.redo_stack.clear();

        self.patch_response(
            true,
            true,
            false,
            Some(patched_graph),
            Some(event),
            Vec::new(),
        )
    }

    pub fn history(&self) -> GraphPatchHistory {
        GraphPatchHistory {
            schema: "skenion.graph.patch.history".to_owned(),
            schema_version: "0.1.0".to_owned(),
            events: self.event_log.clone(),
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
                self.graph.clone(),
                None,
                vec![RuntimeDiagnostic::error("no patch event available to undo")],
            );
        };
        let outcome = self.apply_history_entry(entry.clone(), HistoryDirection::Undo);
        if outcome.applied {
            let response = outcome.response;
            self.redo_stack.push(entry);
            self.patch_response(
                true,
                true,
                false,
                response.graph,
                response.event,
                response.diagnostics,
            )
        } else {
            let response = outcome.response;
            self.undo_stack.push(entry);
            self.patch_response(
                false,
                false,
                response.conflict,
                response.graph,
                None,
                response.diagnostics,
            )
        }
    }

    pub fn redo(&mut self) -> RuntimePatchResponse {
        let Some(entry) = self.redo_stack.pop() else {
            return self.patch_response(
                false,
                false,
                false,
                self.graph.clone(),
                None,
                vec![RuntimeDiagnostic::error("no patch event available to redo")],
            );
        };
        let outcome = self.apply_history_entry(entry.clone(), HistoryDirection::Redo);
        if outcome.applied {
            let response = outcome.response;
            self.undo_stack.push(entry);
            self.patch_response(
                true,
                true,
                false,
                response.graph,
                response.event,
                response.diagnostics,
            )
        } else {
            let response = outcome.response;
            self.redo_stack.push(entry);
            self.patch_response(
                false,
                false,
                response.conflict,
                response.graph,
                None,
                response.diagnostics,
            )
        }
    }

    pub fn reject_patch(
        &self,
        conflict: bool,
        diagnostics: Vec<RuntimeDiagnostic>,
    ) -> RuntimePatchResponse {
        self.patch_response(
            false,
            false,
            conflict,
            self.graph.clone(),
            None,
            diagnostics,
        )
    }

    pub fn clear(&mut self) -> RuntimeSessionResponse {
        self.graph = None;
        self.registry = None;
        self.plan = None;
        self.control_state = ControlState::default();
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
            loaded: snapshot.loaded,
            graph_id: snapshot.graph_id,
            graph_revision: snapshot.graph_revision,
            session_revision: snapshot.session_revision,
            control_revision: snapshot.control_revision,
            diagnostics,
            plan: snapshot.plan,
            report,
        }
    }

    fn patch_response(
        &self,
        ok: bool,
        applied: bool,
        conflict: bool,
        graph: Option<GraphDocument>,
        event: Option<GraphPatchEvent>,
        diagnostics: Vec<RuntimeDiagnostic>,
    ) -> RuntimePatchResponse {
        RuntimePatchResponse {
            ok,
            applied,
            conflict,
            graph,
            session: self.response(ok, diagnostics.clone(), None),
            event,
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
        let (graph, registry) = match (self.graph.clone(), self.registry.clone()) {
            (Some(graph), Some(registry)) => (graph, registry),
            _ => {
                return HistoryApplyOutcome::rejected(self.patch_response(
                    false,
                    false,
                    false,
                    None,
                    None,
                    vec![RuntimeDiagnostic::error(
                        "no project loaded in runtime session",
                    )],
                ));
            }
        };

        let revision_before = graph.revision.clone();
        let next_revision = next_graph_revision(&revision_before);
        let mut patch = match direction {
            HistoryDirection::Undo => entry.inverse_patch.clone(),
            HistoryDirection::Redo => entry.patch.clone(),
        };
        patch.id = match direction {
            HistoryDirection::Undo => format!("undo_{}", entry.event_id),
            HistoryDirection::Redo => format!("redo_{}", entry.event_id),
        };
        patch.base_revision = revision_before.clone();
        let patched_graph = match apply_graph_patch(&graph, &patch, Some(&next_revision)) {
            Ok(patched_graph) => patched_graph,
            Err(error) => {
                return HistoryApplyOutcome::rejected(self.patch_response(
                    false,
                    false,
                    false,
                    Some(graph.clone()),
                    None,
                    vec![RuntimeDiagnostic::error(error.to_string())],
                ));
            }
        };

        if let Err(diagnostics) = validate_graph_with_registry(&patched_graph, &registry) {
            return HistoryApplyOutcome::rejected(self.patch_response(
                false,
                false,
                false,
                Some(graph.clone()),
                None,
                diagnostics,
            ));
        }

        let plan =
            build_execution_plan(&patched_graph, &registry).expect("validated project should plan");
        let control_state = ControlState::from_graph(&patched_graph);
        let mut inverse_patch = match direction {
            HistoryDirection::Undo => entry.patch.clone(),
            HistoryDirection::Redo => entry.inverse_patch.clone(),
        };
        inverse_patch.id = match direction {
            HistoryDirection::Undo => format!("redo_{}", entry.event_id),
            HistoryDirection::Redo => format!("undo_{}", entry.event_id),
        };
        inverse_patch.base_revision = next_revision.clone();
        let event = self.create_patch_event(
            direction.event_kind(),
            patch,
            inverse_patch,
            revision_before,
            next_revision,
            Some(entry.event_id),
        );

        self.graph = Some(patched_graph.clone());
        self.registry = Some(registry);
        self.plan = Some(plan);
        self.control_state = control_state;
        self.control_revision = 0;
        self.diagnostics = Vec::new();
        self.revision += 1;
        self.event_log.push(event.clone());

        HistoryApplyOutcome::applied(self.patch_response(
            true,
            true,
            false,
            Some(patched_graph),
            Some(event),
            Vec::new(),
        ))
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

    fn clear_history(&mut self) {
        self.event_log.clear();
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

impl HistoryDirection {
    fn event_kind(self) -> GraphPatchEventKind {
        match self {
            Self::Undo => GraphPatchEventKind::Undo,
            Self::Redo => GraphPatchEventKind::Redo,
        }
    }
}

fn next_graph_revision(current: &str) -> String {
    current
        .parse::<u64>()
        .map(|revision| (revision + 1).to_string())
        .unwrap_or_else(|_| format!("{current}+1"))
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
        ControlMessage, ControlValue, GraphPatch, GraphPatchEventKind, NodeRegistry,
        ProjectRequest, RuntimeControlEmission, RuntimeControlEventRequest,
        RuntimeControlReadRequest, RuntimeControlReadTarget, RuntimeDiagnostic,
    };

    use super::{HistoryEntry, RuntimeSession};

    #[test]
    fn invalid_registry_load_returns_diagnostics_without_revision_change() {
        let mut session = RuntimeSession::default();
        let mut request = sample_project();
        request.nodes[0].schema_version = "9.9.9".to_owned();

        let response = session.load_project(request);

        assert!(!response.ok);
        assert!(!response.loaded);
        assert_eq!(response.session_revision, 0);
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
        assert!(response.plan.is_none());
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
        assert!(response.plan.is_some());
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
            session.apply_control_event(control_request("value_1", "set", f32_value(32.0)));

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

        let set = session.apply_control_event(control_request("value_1", "set", f32_value(32.0)));
        assert!(set.ok);
        assert!(set.changed);
        assert!(set.emitted.is_empty());
        assert_eq!(session.snapshot().session_revision, 1);
        assert_eq!(session.snapshot().control_revision, 1);
        assert_eq!(session.control_revision(), 1);
        assert_eq!(set.control_revision, Some(1));
        assert_eq!(
            session.control_state_response().values.get("value_1"),
            Some(&ControlValue::F32(32.0))
        );

        let bang = session.apply_control_event(bang_control_request("value_1", "bang"));
        assert!(bang.ok);
        assert_eq!(bang.emitted.len(), 1);
        assert_eq!(bang.emitted[0].node_id, "value_1");
        assert_eq!(bang.emitted[0].port_id, "value");
        assert_eq!(
            emitted_value(&bang.emitted[0]),
            Some(ControlValue::F32(32.0))
        );
        assert!(!bang.changed);
        assert_eq!(session.snapshot().session_revision, 1);
        assert_eq!(session.snapshot().control_revision, 1);
        assert_eq!(bang.control_revision, Some(1));

        let input = session.apply_control_event(control_request("value_1", "in", f32_value(12.0)));
        assert!(input.ok);
        assert!(input.changed);
        assert_eq!(
            emitted_value(&input.emitted[0]),
            Some(ControlValue::F32(12.0))
        );
        assert_eq!(
            session.control_state_response().values.get("value_1"),
            Some(&ControlValue::F32(12.0))
        );
        assert_eq!(session.snapshot().session_revision, 1);
        assert_eq!(session.snapshot().control_revision, 2);
        assert_eq!(session.control_revision(), 2);
        assert_eq!(input.control_revision, Some(2));
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
            Some(ControlValue::F32(1.5))
        );
        assert_eq!(
            session
                .control_state_response()
                .channels
                .get("number.f32:speed"),
            Some(&ControlMessage::from_value(ControlValue::F32(1.5)))
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
                .apply_control_event(control_request("value_1", "set", f32_value(32.0)))
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
            json!({ "type": "f32", "value": 32.0 })
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

        let missing_control_state = session.read_control(control_read(
            "target_1",
            RuntimeControlReadTarget::State,
            "value",
        ));
        assert!(!missing_control_state.ok);
        assert!(
            missing_control_state.diagnostics[0]
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
                .apply_control_event(control_request("value_1", "set", f32_value(32.0)))
                .ok
        );
        let before = session.snapshot();

        let response =
            session.apply_control_event(control_request("value_1", "in", ControlValue::Bool(true)));

        assert!(!response.ok);
        assert!(response.emitted.is_empty());
        assert_eq!(session.snapshot().session_revision, before.session_revision);
        assert_eq!(
            session.control_state_response().values.get("value_1"),
            Some(&ControlValue::F32(32.0))
        );
    }

    #[test]
    fn graph_patch_rebuilds_control_state_from_graph_params() {
        let mut session = RuntimeSession::default();
        assert!(session.load_project(sample_project()).ok);
        assert!(
            session
                .apply_control_event(control_request("value_1", "set", f32_value(32.0)))
                .ok
        );

        let response = session.apply_patch(set_value_patch("1", 0.75));

        assert!(response.ok);
        assert_eq!(
            session.control_state_response().values.get("value_1"),
            Some(&ControlValue::F32(0.75))
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
            Some(&ControlValue::F32(0.0))
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
    fn patch_without_loaded_session_returns_error() {
        let mut session = RuntimeSession::default();

        let response = session.apply_patch(set_value_patch("1", 0.75));

        assert!(!response.ok);
        assert!(!response.applied);
        assert!(!response.conflict);
        assert!(response.graph.is_none());
        assert!(!response.session.loaded);
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
        assert_eq!(
            response.event.as_ref().unwrap().kind,
            GraphPatchEventKind::Apply
        );
        assert_eq!(response.event.as_ref().unwrap().sequence, 1);
        assert_eq!(response.event.as_ref().unwrap().revision_before, "1");
        assert_eq!(response.event.as_ref().unwrap().revision_after, "2");
        assert_eq!(
            response.event.as_ref().unwrap().inverse_patch.base_revision,
            "2"
        );
        assert_eq!(response.history.events.len(), 1);
        assert_eq!(response.history.undo_depth, 1);
        assert_eq!(response.history.redo_depth, 0);
        assert_eq!(response.graph.as_ref().unwrap().revision, "2");
        assert_eq!(response.session.graph_revision.as_deref(), Some("2"));
        assert_eq!(response.session.session_revision, 2);
        assert_eq!(response.session.plan.as_ref().unwrap().graph_revision, "2");
        assert_eq!(
            response.graph.as_ref().unwrap().nodes[0].params["value"],
            Value::from(0.75)
        );
        assert_eq!(
            session.control_state.value_for_node("value_1"),
            Some(&ControlValue::F32(0.75))
        );
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
        assert!(response.event.is_none());
        assert!(response.history.events.is_empty());
        assert_eq!(response.graph.as_ref().unwrap().revision, "1");
        assert_eq!(snapshot.graph_revision.as_deref(), Some("1"));
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
        assert!(duplicate.event.is_none());
        assert!(duplicate.history.events.is_empty());
        assert!(duplicate.diagnostics[0].message.contains("already exists"));
        assert!(!missing.ok);
        assert!(missing.diagnostics[0].message.contains("does not exist"));
        assert_eq!(snapshot.graph_revision.as_deref(), Some("1"));
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
        assert!(
            response.diagnostics[0]
                .message
                .contains("incompatible edge")
        );
        assert_eq!(snapshot.graph_revision.as_deref(), Some("1"));
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
        assert_eq!(snapshot.graph_revision.as_deref(), Some("1"));
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
        let graph = response.graph.as_ref().unwrap();
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
        assert_eq!(response.graph.as_ref().unwrap().revision, "rev_0001+1");
    }

    #[test]
    fn history_starts_empty_and_undo_redo_empty_stack_returns_errors() {
        let mut session = RuntimeSession::default();

        let history = session.history();
        let undo = session.undo();
        let redo = session.redo();

        assert_eq!(history.schema, "skenion.graph.patch.history");
        assert!(!history.can_undo);
        assert!(!history.can_redo);
        assert!(!undo.ok);
        assert!(!undo.applied);
        assert!(undo.event.is_none());
        assert!(undo.diagnostics[0].message.contains("available to undo"));
        assert!(!redo.ok);
        assert!(!redo.applied);
        assert!(redo.event.is_none());
        assert!(redo.diagnostics[0].message.contains("available to redo"));
    }

    #[test]
    fn undo_after_patch_restores_graph_and_creates_event() {
        let mut session = RuntimeSession::default();
        session.load_project(sample_project());
        let applied = session.apply_patch(set_value_patch("1", 0.75));
        let apply_event_id = applied.event.as_ref().unwrap().id.clone();

        let undone = session.undo();

        assert!(undone.ok);
        assert!(undone.applied);
        assert_eq!(undone.graph.as_ref().unwrap().revision, "3");
        assert!(
            !undone.graph.as_ref().unwrap().nodes[0]
                .params
                .contains_key("value")
        );
        assert_eq!(undone.session.session_revision, 3);
        assert_eq!(
            undone.event.as_ref().unwrap().kind,
            GraphPatchEventKind::Undo
        );
        assert_eq!(
            undone.event.as_ref().unwrap().subject_event_id.as_deref(),
            Some(apply_event_id.as_str())
        );
        assert_eq!(undone.event.as_ref().unwrap().revision_before, "2");
        assert_eq!(undone.event.as_ref().unwrap().revision_after, "3");
        assert_eq!(undone.history.events.len(), 2);
        assert_eq!(undone.history.undo_depth, 0);
        assert_eq!(undone.history.redo_depth, 1);
    }

    #[test]
    fn redo_after_undo_reapplies_graph_and_creates_event() {
        let mut session = RuntimeSession::default();
        session.load_project(sample_project());
        session.apply_patch(set_value_patch("1", 0.75));
        session.undo();

        let redone = session.redo();

        assert!(redone.ok);
        assert!(redone.applied);
        assert_eq!(redone.graph.as_ref().unwrap().revision, "4");
        assert_eq!(
            redone.graph.as_ref().unwrap().nodes[0].params["value"],
            Value::from(0.75)
        );
        assert_eq!(redone.session.session_revision, 4);
        assert_eq!(
            redone.event.as_ref().unwrap().kind,
            GraphPatchEventKind::Redo
        );
        assert_eq!(redone.event.as_ref().unwrap().revision_before, "3");
        assert_eq!(redone.event.as_ref().unwrap().revision_after, "4");
        assert_eq!(redone.history.events.len(), 3);
        assert_eq!(redone.history.undo_depth, 1);
        assert_eq!(redone.history.redo_depth, 0);
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
        assert_eq!(applied.graph.as_ref().unwrap().revision, "4");
        assert_eq!(applied.history.events.len(), 3);
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
        assert!(removed.graph.as_ref().unwrap().edges.is_empty());

        let undone = session.undo();

        assert!(undone.ok);
        assert_eq!(undone.graph.as_ref().unwrap().revision, "3");
        assert!(
            undone
                .graph
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .any(|node| node.id == "value_1")
        );
        assert_eq!(undone.graph.as_ref().unwrap().edges.len(), 1);
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
        assert_eq!(first_undo.graph.as_ref().unwrap().revision, "4");
        assert_eq!(
            first_undo.graph.as_ref().unwrap().nodes[0].params["value"],
            Value::from(0.75)
        );
        assert!(second_undo.ok);
        assert_eq!(second_undo.graph.as_ref().unwrap().revision, "5");
        assert!(
            !second_undo.graph.as_ref().unwrap().nodes[0]
                .params
                .contains_key("value")
        );
        assert_eq!(second_undo.history.events.len(), 4);
        assert_eq!(second_undo.history.redo_depth, 2);
    }

    #[test]
    fn failed_history_operations_do_not_mutate_stacks_or_session() {
        let mut no_loaded = RuntimeSession::default();
        no_loaded.undo_stack.push(HistoryEntry {
            event_id: "event_bad".to_owned(),
            patch: set_value_patch("1", 0.75),
            inverse_patch: set_value_patch("1", 0.5),
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
        invalid_inverse.undo_stack.push(HistoryEntry {
            event_id: "event_bad_inverse".to_owned(),
            patch: set_value_patch("1", 0.75),
            inverse_patch: missing_node_patch(),
        });
        let invalid_inverse_response = invalid_inverse.undo();
        assert!(!invalid_inverse_response.ok);
        assert_eq!(invalid_inverse_response.history.undo_depth, 1);
        assert_eq!(
            invalid_inverse_response.session.graph_revision.as_deref(),
            Some("1")
        );

        let mut invalid_redo = RuntimeSession::default();
        invalid_redo.load_project(sample_project());
        invalid_redo.redo_stack.push(HistoryEntry {
            event_id: "event_bad_redo".to_owned(),
            patch: missing_definition_node_patch(),
            inverse_patch: set_value_patch("1", 0.5),
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
        assert!(response.event.is_none());
        assert_eq!(response.history.events.len(), 0);
        assert_eq!(response.graph.as_ref().unwrap().revision, "1");
        assert_eq!(response.session.graph_revision.as_deref(), Some("1"));
        assert!(response.diagnostics[0].message.contains("unsupported op"));
    }

    fn graph_patch(value: Value) -> GraphPatch {
        serde_json::from_value(value).expect("patch should parse")
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
        ControlValue::F32(value)
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
                "to": { "node": "target_1", "port": "value" }
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
                "to": { "node": "target_1", "port": "bang" }
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
                "kind": "core.value-f32",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": value_f32_ports_json()
              },
              {
                "id": "target_1",
                "kind": "core.target",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": target_ports_json()
              }
            ],
            "edges": [
              { "from": { "node": "value_1", "port": "value" }, "to": { "node": "target_1", "port": "value" } }
            ]
          },
          "nodes": [
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "core.value-f32",
              "version": "0.1.0",
              "displayName": "Float Value",
              "category": "Values",
              "ports": value_f32_ports_json(),
              "execution": { "model": "value" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": []
            },
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "core.target",
              "version": "0.1.0",
              "displayName": "Target",
              "category": "Values",
              "ports": target_ports_json(),
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
            "type": { "flow": "value", "dataKind": "number.f32" },
            "required": false,
            "activation": "trigger"
          },
          {
            "id": "set",
            "direction": "input",
            "label": "Set",
            "type": { "flow": "value", "dataKind": "number.f32" },
            "required": false,
            "activation": "latched"
          },
          {
            "id": "bang",
            "direction": "input",
            "label": "Bang",
            "type": { "flow": "event", "dataKind": "event.bang" },
            "required": false,
            "activation": "trigger"
          },
          {
            "id": "value",
            "direction": "output",
            "label": "Value",
            "type": { "flow": "value", "dataKind": "number.f32" }
          }
        ])
    }

    fn target_ports_json() -> Value {
        json!([
          {
            "id": "value",
            "direction": "input",
            "label": "Value",
            "type": { "flow": "value", "dataKind": "number.f32" },
            "activation": "latched"
          },
          {
            "id": "bang",
            "direction": "input",
            "label": "Bang",
            "type": { "flow": "event", "dataKind": "event.bang" },
            "activation": "trigger"
          }
        ])
    }
}
