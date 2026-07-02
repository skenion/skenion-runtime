use super::super::*;
use super::validation::node_target_revision_conflict_response;

impl RuntimeSession {
    pub(crate) fn apply_node_delete_current(
        &mut self,
        target: GraphTargetRef,
        node_id: String,
        actor_id: Option<String>,
        client_id: Option<String>,
        description: Option<String>,
    ) -> (RuntimePatchResponse, Vec<String>) {
        self.apply_node_delete_current_inner(target, node_id, actor_id, client_id, description)
    }

    pub(crate) fn apply_node_update_current(
        &mut self,
        target: GraphTargetRef,
        node_id: String,
        params: Map<String, Value>,
        actor_id: Option<String>,
        client_id: Option<String>,
        description: Option<String>,
    ) -> RuntimePatchResponse {
        self.apply_node_update_current_inner(
            target,
            node_id,
            params,
            actor_id,
            client_id,
            description,
        )
    }

    fn apply_node_delete_current_inner(
        &mut self,
        target: GraphTargetRef,
        node_id: String,
        actor_id: Option<String>,
        client_id: Option<String>,
        description: Option<String>,
    ) -> (RuntimePatchResponse, Vec<String>) {
        let Some(project) = self.project.as_ref().cloned() else {
            return (
                self.patch_response(
                    false,
                    false,
                    false,
                    vec![RuntimeIssue::structured_error(
                        "node.target.no-project",
                        "no project loaded in runtime session",
                        json!({ "target": target }),
                    )],
                ),
                Vec::new(),
            );
        };

        let target_revision = match target_graph_revision_current(&project, &target) {
            Ok(revision) => revision,
            Err(issue) => {
                return (
                    self.patch_response(
                        false,
                        false,
                        false,
                        vec![operation_issue_to_runtime_issue(*issue)],
                    ),
                    Vec::new(),
                );
            }
        };
        if target.base_revision != target_revision {
            return (
                node_target_revision_conflict_response(self, &target, &target_revision),
                Vec::new(),
            );
        }

        let mut graph = match graph_for_path_current(&project, &target.path) {
            Some(graph) => graph,
            None => {
                return (
                    self.patch_response(
                        false,
                        false,
                        false,
                        vec![RuntimeIssue::structured_error(
                            "node.target.missing-graph",
                            "node target graph is not available in the active current 0.1 project",
                            json!({ "target": target }),
                        )],
                    ),
                    Vec::new(),
                );
            }
        };
        let Some(node_index) = graph.nodes.iter().position(|node| node.id == node_id) else {
            return (
                self.patch_response(
                    false,
                    false,
                    false,
                    vec![RuntimeIssue::structured_error(
                        "node.delete.node-missing",
                        format!("node {node_id} does not exist in target graph"),
                        json!({ "nodeId": node_id, "target": target }),
                    )],
                ),
                Vec::new(),
            );
        };

        graph.nodes.remove(node_index);
        let dropped_edge_ids = graph
            .edges
            .iter()
            .filter(|edge| edge.source.node_id == node_id || edge.target.node_id == node_id)
            .map(|edge| edge.id.clone())
            .collect::<Vec<_>>();
        let dropped = dropped_edge_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        graph
            .edges
            .retain(|edge| !dropped.contains(edge.id.as_str()));
        graph.revision = next_graph_revision(&graph.revision);

        let mut next_project = project.clone();
        let mut view_state = project.view_state.clone();
        let view_changed = target_supports_view_state(&target.path)
            && view_state.canvas.nodes.remove(&node_id).is_some();
        let next_view_revision = apply_graph_to_project_current(
            &mut next_project,
            graph,
            view_state,
            view_changed,
            &target.path,
            self.view_revision,
        );

        let response = self.apply_project_document_update(
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
        );
        (response, dropped_edge_ids)
    }

    fn apply_node_update_current_inner(
        &mut self,
        target: GraphTargetRef,
        node_id: String,
        params: Map<String, Value>,
        actor_id: Option<String>,
        client_id: Option<String>,
        description: Option<String>,
    ) -> RuntimePatchResponse {
        if params.is_empty() {
            return self.patch_response(
                false,
                false,
                false,
                vec![RuntimeIssue::structured_error(
                    "node.update.params-required",
                    "node.update requires at least one payload.params entry",
                    json!({ "nodeId": node_id, "target": target }),
                )],
            );
        }
        let Some(project) = self.project.as_ref().cloned() else {
            return self.patch_response(
                false,
                false,
                false,
                vec![RuntimeIssue::structured_error(
                    "node.target.no-project",
                    "no project loaded in runtime session",
                    json!({ "target": target }),
                )],
            );
        };

        let target_revision = match target_graph_revision_current(&project, &target) {
            Ok(revision) => revision,
            Err(issue) => {
                return self.patch_response(
                    false,
                    false,
                    false,
                    vec![operation_issue_to_runtime_issue(*issue)],
                );
            }
        };
        if target.base_revision != target_revision {
            return node_target_revision_conflict_response(self, &target, &target_revision);
        }

        let mut graph = match graph_for_path_current(&project, &target.path) {
            Some(graph) => graph,
            None => {
                return self.patch_response(
                    false,
                    false,
                    false,
                    vec![RuntimeIssue::structured_error(
                        "node.target.missing-graph",
                        "node target graph is not available in the active current 0.1 project",
                        json!({ "target": target }),
                    )],
                );
            }
        };
        let Some(node) = graph.nodes.iter_mut().find(|node| node.id == node_id) else {
            return self.patch_response(
                false,
                false,
                false,
                vec![RuntimeIssue::structured_error(
                    "node.update.node-missing",
                    format!("node {node_id} does not exist in target graph"),
                    json!({ "nodeId": node_id, "target": target }),
                )],
            );
        };

        for (key, value) in params {
            node.params.insert(key, value);
        }
        graph.revision = next_graph_revision(&graph.revision);

        let mut next_project = project.clone();
        let next_view_revision = apply_graph_to_project_current(
            &mut next_project,
            graph,
            project.view_state.clone(),
            false,
            &target.path,
            self.view_revision,
        );

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
}
