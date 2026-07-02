use super::super::*;
use super::types::{
    ApplyObjectNodeCreateCurrentRequest, ApplyObjectNodeReplaceCurrentRequest,
    ObjectNodeCreateCurrentEdit, ObjectNodeReplaceCurrentEdit,
};
use super::validation::{
    invalid_incident_edge_ids_current, node_target_revision_conflict_response,
};

impl RuntimeSession {
    pub(crate) fn apply_object_node_create_current(
        &mut self,
        request: ApplyObjectNodeCreateCurrentRequest,
    ) -> RuntimePatchResponse {
        let ApplyObjectNodeCreateCurrentRequest {
            target,
            node,
            view,
            definition,
            mutation,
        } = request;
        let previous_nodes_current = self.nodes_current.clone();
        self.ensure_object_node_definition_current(definition);
        let response = self.apply_object_node_create_current_inner(ObjectNodeCreateCurrentEdit {
            target,
            node,
            view,
            mutation,
        });
        if !response.applied {
            self.nodes_current = previous_nodes_current;
        }
        response
    }

    pub(crate) fn apply_object_node_replace_current(
        &mut self,
        request: ApplyObjectNodeReplaceCurrentRequest,
    ) -> (RuntimePatchResponse, Vec<String>) {
        let ApplyObjectNodeReplaceCurrentRequest {
            target,
            node,
            view,
            definition,
            interface_incident_edge_policy,
            mutation,
        } = request;
        let previous_nodes_current = self.nodes_current.clone();
        self.ensure_object_node_definition_current(definition);
        let (response, dropped_edge_ids) =
            self.apply_object_node_replace_current_inner(ObjectNodeReplaceCurrentEdit {
                target,
                node,
                view,
                interface_incident_edge_policy,
                mutation,
            });
        if !response.applied {
            self.nodes_current = previous_nodes_current;
        }
        (response, dropped_edge_ids)
    }

    fn ensure_object_node_definition_current(&mut self, definition: Option<NodeDefinitionCurrent>) {
        let Some(definition) = definition else {
            return;
        };
        if self
            .nodes_current
            .iter()
            .any(|existing| existing.id == definition.id)
        {
            return;
        }
        if self.nodes_current.iter().any(|existing| {
            node_definition_shape_key_current(existing)
                == node_definition_shape_key_current(&definition)
        }) {
            return;
        }
        self.nodes_current.push(definition);
    }

    fn apply_object_node_create_current_inner(
        &mut self,
        edit: ObjectNodeCreateCurrentEdit,
    ) -> RuntimePatchResponse {
        let ObjectNodeCreateCurrentEdit {
            target,
            node,
            view,
            mutation,
        } = edit;
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
        if graph.nodes.iter().any(|existing| existing.id == node.id) {
            return self.patch_response(
                false,
                false,
                false,
                vec![RuntimeIssue::structured_error(
                    "node.create.node-id-conflict",
                    format!("node id {} already exists in target graph", node.id),
                    json!({ "nodeId": node.id, "target": target }),
                )],
            );
        }

        let mut next_project = project.clone();
        let mut view_state = project.view_state.clone();
        let mut view_changed = false;
        if let Some(view) = view {
            if target_supports_view_state(&target.path) {
                view_state.canvas.nodes.insert(node.id.clone(), view);
                view_changed = true;
            } else {
                return self.patch_response(
                    false,
                    false,
                    false,
                    vec![unsupported_patch_view_change_issue(&target)],
                );
            }
        }
        graph.nodes.push(node);
        graph.revision = next_graph_revision(&graph.revision);
        let next_view_revision = apply_graph_to_project_current(
            &mut next_project,
            graph,
            view_state,
            view_changed,
            &target.path,
            self.view_revision,
        );

        self.apply_project_document_update(
            project,
            next_project,
            next_view_revision,
            mutation,
            None,
        )
    }

    fn apply_object_node_replace_current_inner(
        &mut self,
        edit: ObjectNodeReplaceCurrentEdit,
    ) -> (RuntimePatchResponse, Vec<String>) {
        let ObjectNodeReplaceCurrentEdit {
            target,
            node,
            view,
            interface_incident_edge_policy,
            mutation,
        } = edit;
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
        let Some(node_index) = graph
            .nodes
            .iter()
            .position(|existing| existing.id == node.id)
        else {
            return (
                self.patch_response(
                    false,
                    false,
                    false,
                    vec![RuntimeIssue::structured_error(
                        "node.replace.node-missing",
                        format!("node {} does not exist in target graph", node.id),
                        json!({ "nodeId": node.id, "target": target }),
                    )],
                ),
                Vec::new(),
            );
        };

        let previous_node = graph.nodes[node_index].clone();
        graph.nodes[node_index] = node.clone();
        let invalid_incident_edge_ids = invalid_incident_edge_ids_current(&graph, &node.id);
        let policy = interface_incident_edge_policy.unwrap_or(InterfaceIncidentEdgePolicyV01::Drop);
        let mut issues = Vec::new();
        if !invalid_incident_edge_ids.is_empty() {
            match policy {
                InterfaceIncidentEdgePolicyV01::Drop => {
                    let invalid = invalid_incident_edge_ids
                        .iter()
                        .map(String::as_str)
                        .collect::<HashSet<_>>();
                    graph
                        .edges
                        .retain(|edge| !invalid.contains(edge.id.as_str()));
                    issues.push(RuntimeIssue::structured_warning(
                        "node.replace.incident-edges-dropped",
                        format!(
                            "node.replace dropped {} incident edge(s) that no longer match node {}'s interface",
                            invalid_incident_edge_ids.len(),
                            node.id
                        ),
                        json!({
                            "target": target,
                            "nodeId": node.id,
                            "droppedEdgeIds": invalid_incident_edge_ids,
                            "interfaceIncidentEdgePolicy": policy,
                        }),
                    ));
                }
                InterfaceIncidentEdgePolicyV01::Reject => {
                    return (
                        self.patch_response(
                            false,
                            false,
                            false,
                            vec![RuntimeIssue::structured_error(
                                "node.replace.invalid-incident-edge",
                                format!(
                                    "node.replace would leave {} invalid incident edge(s) on node {}",
                                    invalid_incident_edge_ids.len(),
                                    node.id
                                ),
                                json!({
                                    "target": target,
                                    "nodeId": node.id,
                                    "edgeIds": invalid_incident_edge_ids,
                                    "interfaceIncidentEdgePolicy": policy,
                                }),
                            )],
                        ),
                        Vec::new(),
                    );
                }
                InterfaceIncidentEdgePolicyV01::PreserveIssue => {
                    return (
                        self.patch_response(
                            false,
                            false,
                            false,
                            vec![RuntimeIssue::structured_error(
                                "node.replace.preserve-issue-unsupported",
                                "node.replace cannot preserve invalid incident edges in the current Runtime graph substrate",
                                json!({
                                    "target": target,
                                    "nodeId": node.id,
                                    "edgeIds": invalid_incident_edge_ids,
                                    "interfaceIncidentEdgePolicy": policy,
                                }),
                            )],
                        ),
                        Vec::new(),
                    );
                }
            }
        }

        let mut next_project = project.clone();
        let mut view_state = project.view_state.clone();
        let mut view_changed = false;
        if let Some(view) = view {
            if target_supports_view_state(&target.path) {
                let previous = view_state
                    .canvas
                    .nodes
                    .insert(node.id.clone(), view.clone());
                view_changed = previous.as_ref() != Some(&view);
            } else {
                return (
                    self.patch_response(
                        false,
                        false,
                        false,
                        vec![unsupported_patch_view_change_issue(&target)],
                    ),
                    Vec::new(),
                );
            }
        }

        let graph_changed =
            previous_node != graph.nodes[node_index] || !invalid_incident_edge_ids.is_empty();
        if !graph_changed && !view_changed {
            return (self.patch_response(true, false, false, issues), Vec::new());
        }
        graph.revision = next_graph_revision(&graph.revision);
        let next_view_revision = apply_graph_to_project_current(
            &mut next_project,
            graph,
            view_state,
            view_changed,
            &target.path,
            self.view_revision,
        );

        let mut response = self.apply_project_document_update(
            project,
            next_project,
            next_view_revision,
            mutation,
            None,
        );
        if response.applied {
            response.issues.extend(issues);
        }
        (response, invalid_incident_edge_ids)
    }
}

fn node_definition_shape_key_current(definition: &NodeDefinitionCurrent) -> String {
    serde_json::to_string(&serde_json::json!({
        "id": definition.id,
        "ports": definition.ports,
        "execution": definition.execution,
    }))
    .expect("node definition shape key should serialize")
}
