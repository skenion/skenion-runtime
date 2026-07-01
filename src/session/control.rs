use crate::{
    PreviewControlStateSnapshot, RuntimeControlEventRequest, RuntimeControlEventResponse,
    RuntimeControlReadRequest, RuntimeControlReadResponse, RuntimeControlReadTarget,
    RuntimeControlStateResponse, RuntimeIssue, read_graph_param, read_graph_port,
};

use super::RuntimeSession;

impl RuntimeSession {
    pub fn apply_control_event(
        &mut self,
        request: RuntimeControlEventRequest,
    ) -> RuntimeControlEventResponse {
        let Some(graph) = self.execution_graph() else {
            return RuntimeControlEventResponse {
                ok: false,
                changed: false,
                control_revision: Some(self.control_revision),
                emitted: Vec::new(),
                issues: vec![RuntimeIssue::error("no project loaded in runtime session")],
            };
        };

        let before = self.control_state.clone();
        let response = self.control_state.apply_event(request, &graph);
        if response.ok {
            let changed = self.control_state != before;
            if changed {
                self.control_revision += 1;
            }
            self.issues = Vec::new();
            return response.with_runtime_metadata(changed, self.control_revision);
        } else {
            self.issues = response.issues.clone();
        }
        response.with_runtime_metadata(false, self.control_revision)
    }

    pub fn control_state_response(&self) -> RuntimeControlStateResponse {
        let loaded = self.project.is_some();
        RuntimeControlStateResponse {
            ok: loaded,
            control_revision: self.control_revision,
            values: self.control_state.values.clone(),
            channels: self.control_state.channels.clone(),
            issues: if loaded {
                Vec::new()
            } else {
                vec![RuntimeIssue::error("no project loaded in runtime session")]
            },
        }
    }

    pub fn control_revision(&self) -> u64 {
        self.control_revision
    }

    pub fn preview_control_state_snapshot(&self) -> Option<PreviewControlStateSnapshot> {
        self.project.as_ref()?;
        Some(PreviewControlStateSnapshot::new(
            self.revision,
            self.control_revision,
            &self.control_state,
        ))
    }

    pub fn read_control(&self, request: RuntimeControlReadRequest) -> RuntimeControlReadResponse {
        let Some(graph) = self.execution_graph() else {
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
}
