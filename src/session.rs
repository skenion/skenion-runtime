use serde::{Deserialize, Serialize};

use crate::{
    DummyExecutionReport, ExecutionPlan, GraphDocument, GraphPatch, NodeRegistry, ProjectRequest,
    RuntimeDiagnostic, apply_graph_patch, build_execution_plan, run_dummy_execution,
    server::{registry_from_nodes, validate_graph_with_registry},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSessionSnapshot {
    pub loaded: bool,
    pub graph_id: Option<String>,
    pub graph_revision: Option<String>,
    pub session_revision: u64,
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
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRunRequest {
    pub frames: Option<usize>,
}

#[derive(Debug, Default)]
pub struct RuntimeSession {
    graph: Option<GraphDocument>,
    registry: Option<NodeRegistry>,
    plan: Option<ExecutionPlan>,
    diagnostics: Vec<RuntimeDiagnostic>,
    revision: u64,
}

impl RuntimeSession {
    pub fn snapshot(&self) -> RuntimeSessionSnapshot {
        RuntimeSessionSnapshot {
            loaded: self.graph.is_some(),
            graph_id: self.graph.as_ref().map(|graph| graph.id.clone()),
            graph_revision: self.graph.as_ref().map(|graph| graph.revision.clone()),
            session_revision: self.revision,
            diagnostics: self.diagnostics.clone(),
            plan: self.plan.clone(),
        }
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
        self.graph = Some(graph);
        self.registry = Some(registry);
        self.plan = Some(plan);
        self.diagnostics = Vec::new();
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
                vec![RuntimeDiagnostic::error(format!(
                    "patch baseRevision {} does not match session graph revision {}",
                    patch.base_revision, graph.revision
                ))],
            );
        }

        let next_revision = next_graph_revision(&graph.revision);
        let patched_graph = match apply_graph_patch(&graph, &patch, Some(&next_revision)) {
            Ok(patched_graph) => patched_graph,
            Err(error) => {
                return self.patch_response(
                    false,
                    false,
                    false,
                    Some(graph.clone()),
                    vec![RuntimeDiagnostic::error(error.to_string())],
                );
            }
        };

        if let Err(diagnostics) = validate_graph_with_registry(&patched_graph, &registry) {
            return self.patch_response(false, false, false, Some(graph.clone()), diagnostics);
        }

        let plan =
            build_execution_plan(&patched_graph, &registry).expect("validated project should plan");
        self.graph = Some(patched_graph.clone());
        self.registry = Some(registry);
        self.plan = Some(plan);
        self.diagnostics = Vec::new();
        self.revision += 1;

        self.patch_response(true, true, false, Some(patched_graph), Vec::new())
    }

    pub fn reject_patch(
        &self,
        conflict: bool,
        diagnostics: Vec<RuntimeDiagnostic>,
    ) -> RuntimePatchResponse {
        self.patch_response(false, false, conflict, self.graph.clone(), diagnostics)
    }

    pub fn clear(&mut self) -> RuntimeSessionResponse {
        self.graph = None;
        self.registry = None;
        self.plan = None;
        self.diagnostics = Vec::new();
        self.revision += 1;
        self.response(true, Vec::new(), None)
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
        diagnostics: Vec<RuntimeDiagnostic>,
    ) -> RuntimePatchResponse {
        RuntimePatchResponse {
            ok,
            applied,
            conflict,
            graph,
            session: self.response(ok, diagnostics.clone(), None),
            diagnostics,
        }
    }

    fn loaded_project(&self) -> Option<(&GraphDocument, &NodeRegistry)> {
        Some((self.graph.as_ref()?, self.registry.as_ref()?))
    }
}

fn next_graph_revision(current: &str) -> String {
    current
        .parse::<u64>()
        .map(|revision| (revision + 1).to_string())
        .unwrap_or_else(|_| format!("{current}+1"))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::{GraphPatch, NodeRegistry, ProjectRequest, RuntimeDiagnostic};

    use super::RuntimeSession;

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
        assert_eq!(response.graph.as_ref().unwrap().revision, "2");
        assert_eq!(response.session.graph_revision.as_deref(), Some("2"));
        assert_eq!(response.session.session_revision, 2);
        assert_eq!(response.session.plan.as_ref().unwrap().graph_revision, "2");
        assert_eq!(
            response.graph.as_ref().unwrap().nodes[0].params["value"],
            Value::from(0.75)
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
                "ports": [
                  { "id": "value", "direction": "output", "type": { "flow": "value", "dataKind": "f32" } }
                ]
              },
              {
                "id": "target_1",
                "kind": "core.target",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": [
                  { "id": "value", "direction": "input", "type": { "flow": "value", "dataKind": "f32" }, "activation": "latched" },
                  { "id": "bang", "direction": "input", "type": { "flow": "event", "dataKind": "bang" }, "activation": "trigger" }
                ]
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
              "ports": [
                { "id": "value", "direction": "output", "type": { "flow": "value", "dataKind": "f32" } }
              ],
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
              "ports": [
                { "id": "value", "direction": "input", "type": { "flow": "value", "dataKind": "f32" }, "activation": "latched" },
                { "id": "bang", "direction": "input", "type": { "flow": "event", "dataKind": "bang" }, "activation": "trigger" }
              ],
              "execution": { "model": "value" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": []
            }
          ]
        })
    }
}
