use super::*;

pub(crate) struct ApplyObjectNodeCreateCurrentRequest {
    pub(crate) target: GraphTargetRef,
    pub(crate) node: GraphNodeCurrent,
    pub(crate) view: Option<CanvasNodeView>,
    pub(crate) definition: Option<NodeDefinitionCurrent>,
    pub(crate) mutation: RuntimeMutationRequest,
}

pub(crate) struct ApplyObjectNodeReplaceCurrentRequest {
    pub(crate) target: GraphTargetRef,
    pub(crate) node: GraphNodeCurrent,
    pub(crate) view: Option<CanvasNodeView>,
    pub(crate) definition: Option<NodeDefinitionCurrent>,
    pub(crate) interface_incident_edge_policy: Option<InterfaceIncidentEdgePolicyV01>,
    pub(crate) mutation: RuntimeMutationRequest,
}

struct ObjectNodeCreateCurrentEdit {
    target: GraphTargetRef,
    node: GraphNodeCurrent,
    view: Option<CanvasNodeView>,
    mutation: RuntimeMutationRequest,
}

struct ObjectNodeReplaceCurrentEdit {
    target: GraphTargetRef,
    node: GraphNodeCurrent,
    view: Option<CanvasNodeView>,
    interface_incident_edge_policy: Option<InterfaceIncidentEdgePolicyV01>,
    mutation: RuntimeMutationRequest,
}

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

    fn ensure_object_node_definition_current(&mut self, definition: Option<NodeDefinitionCurrent>) {
        let Some(definition) = definition else {
            return;
        };
        if self
            .nodes_current
            .iter()
            .any(|existing| existing.id == definition.id && existing.version == definition.version)
        {
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
                vec![RuntimeDiagnostic::structured_error(
                    "node.target.no-project",
                    "no project loaded in runtime session",
                    json!({ "target": target }),
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
            return node_target_revision_conflict_response(self, &target, &target_revision);
        }

        let mut graph = match graph_for_path_current(&project, &target.path) {
            Some(graph) => graph,
            None => {
                return self.patch_response(
                    false,
                    false,
                    false,
                    vec![RuntimeDiagnostic::structured_error(
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
                vec![RuntimeDiagnostic::structured_error(
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
                    vec![unsupported_patch_view_change_diagnostic(&target)],
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
                    vec![RuntimeDiagnostic::structured_error(
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
            Err(diagnostic) => {
                return (
                    self.patch_response(
                        false,
                        false,
                        false,
                        vec![operation_diagnostic_to_runtime_diagnostic(*diagnostic)],
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
                        vec![RuntimeDiagnostic::structured_error(
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
                    vec![RuntimeDiagnostic::structured_error(
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
        let mut diagnostics = Vec::new();
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
                    diagnostics.push(RuntimeDiagnostic::structured_warning(
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
                            vec![RuntimeDiagnostic::structured_error(
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
                InterfaceIncidentEdgePolicyV01::PreserveDiagnostic => {
                    return (
                        self.patch_response(
                            false,
                            false,
                            false,
                            vec![RuntimeDiagnostic::structured_error(
                                "node.replace.preserve-diagnostic-unsupported",
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
                        vec![unsupported_patch_view_change_diagnostic(&target)],
                    ),
                    Vec::new(),
                );
            }
        }

        let graph_changed =
            previous_node != graph.nodes[node_index] || !invalid_incident_edge_ids.is_empty();
        if !graph_changed && !view_changed {
            return (
                self.patch_response(true, false, false, diagnostics),
                Vec::new(),
            );
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
            response.diagnostics.extend(diagnostics);
        }
        (response, invalid_incident_edge_ids)
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
                    vec![RuntimeDiagnostic::structured_error(
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
            Err(diagnostic) => {
                return (
                    self.patch_response(
                        false,
                        false,
                        false,
                        vec![operation_diagnostic_to_runtime_diagnostic(*diagnostic)],
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
                        vec![RuntimeDiagnostic::structured_error(
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
                    vec![RuntimeDiagnostic::structured_error(
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
                vec![RuntimeDiagnostic::structured_error(
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
                vec![RuntimeDiagnostic::structured_error(
                    "node.target.no-project",
                    "no project loaded in runtime session",
                    json!({ "target": target }),
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
            return node_target_revision_conflict_response(self, &target, &target_revision);
        }

        let mut graph = match graph_for_path_current(&project, &target.path) {
            Some(graph) => graph,
            None => {
                return self.patch_response(
                    false,
                    false,
                    false,
                    vec![RuntimeDiagnostic::structured_error(
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
                vec![RuntimeDiagnostic::structured_error(
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

fn node_target_revision_conflict_response(
    session: &RuntimeSession,
    target: &GraphTargetRef,
    actual_revision: &str,
) -> RuntimePatchResponse {
    session.patch_response(
        false,
        false,
        true,
        vec![RuntimeDiagnostic::structured_error(
            "node.command.target-revision-conflict",
            format!(
                "target baseRevision {} does not match target graph revision {}",
                target.base_revision, actual_revision
            ),
            json!({
                "expectedRevision": target.base_revision,
                "actualRevision": actual_revision,
                "target": target,
            }),
        )],
    )
}

fn invalid_incident_edge_ids_current(graph: &GraphDocumentCurrent, node_id: &str) -> Vec<String> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.source.node_id == node_id || edge.target.node_id == node_id)
        .filter(|edge| !edge_is_valid_current(graph, edge))
        .map(|edge| edge.id.clone())
        .collect()
}

fn edge_is_valid_current(graph: &GraphDocumentCurrent, edge: &EdgeSpecCurrent) -> bool {
    let Some(source_node) = graph
        .nodes
        .iter()
        .find(|node| node.id == edge.source.node_id)
    else {
        return false;
    };
    let Some(target_node) = graph
        .nodes
        .iter()
        .find(|node| node.id == edge.target.node_id)
    else {
        return false;
    };
    let Some(source_port) = source_node
        .ports
        .iter()
        .find(|port| port.id == edge.source.port_id)
    else {
        return false;
    };
    let Some(target_port) = target_node
        .ports
        .iter()
        .find(|port| port.id == edge.target.port_id)
    else {
        return false;
    };

    source_port.direction == PortDirectionCurrent::Output
        && target_port.direction == PortDirectionCurrent::Input
        && port_types_compatible_current(source_port, target_port)
}

fn port_types_compatible_current(source: &PortSpecCurrent, target: &PortSpecCurrent) -> bool {
    source.port_type == target.port_type
        || target
            .accepts
            .as_ref()
            .is_some_and(|accepted| accepted.iter().any(|kind| kind == &source.port_type))
}
