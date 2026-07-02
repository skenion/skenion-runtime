use super::*;

impl RuntimeSession {
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
                vec![RuntimeIssue::structured_error(
                    "collaboration.target.no-project",
                    "no project loaded in runtime session",
                    serde_json::json!({ "target": target }),
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
            return self.patch_response(
                false,
                false,
                true,
                vec![RuntimeIssue::structured_error(
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
                Err(issues) => {
                    return self.patch_response(false, false, false, issues);
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
}

pub(super) fn apply_collaboration_changes_to_project_current(
    mut project: ProjectDocumentCurrent,
    view_revision: u64,
    target: &GraphTargetRef,
    changes: &[RuntimeCollaborationChange],
) -> Result<(ProjectDocumentCurrent, u64), Vec<RuntimeIssue>> {
    if matches!(
        &target.path,
        PatchPath::PackagePatchDefinition { .. } | PatchPath::EmbeddedPatchInstance { .. }
    ) {
        return Err(vec![RuntimeIssue::structured_error(
            "collaboration.target.unsupported",
            "collaboration target cannot be mutated in the active Runtime session",
            serde_json::json!({ "target": target }),
        )]);
    }
    let mut graph = graph_for_path_current(&project, &target.path).ok_or_else(|| {
        vec![RuntimeIssue::structured_error(
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
                    return Err(vec![RuntimeIssue::structured_error(
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
                        return Err(vec![unsupported_patch_view_change_issue(target)]);
                    }
                }
            }
            RuntimeCollaborationChange::NodeMove {
                node_id, from, to, ..
            } => {
                if !target_supports_view_state(&target.path) {
                    return Err(vec![unsupported_patch_view_change_issue(target)]);
                }
                if !graph.nodes.iter().any(|node| node.id == *node_id) {
                    return Err(vec![RuntimeIssue::structured_error(
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
                    return Err(vec![RuntimeIssue::structured_error(
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
                    return Err(vec![RuntimeIssue::structured_error(
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
                    return Err(vec![RuntimeIssue::structured_error(
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
                    return Err(vec![RuntimeIssue::structured_error(
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
        project.graph = graph;
        project.revision = project.graph.revision.clone();
        project.view_state = runtime_owned_view_state(reconcile_view_state_with_graph_current(
            &project.graph,
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
