use super::*;

type PasteProjectResult =
    Result<(ProjectDocumentCurrent, u64, IdRemapResult, String), PasteProjectError>;
type PasteProjectError = (Vec<RuntimeOperationIssue>, IdRemapResult);

impl RuntimeSession {
    pub fn apply_runtime_operation(
        &mut self,
        envelope: RuntimeOperationEnvelope,
    ) -> PasteGraphFragmentResponse {
        let target = envelope.request.target.clone();
        if let Err(report) = crate::validate_runtime_operation_envelope(&envelope) {
            return self.reject_paste_response(
                target,
                false,
                operation_issues_from_validation_report(report.to_string()),
                empty_id_remap(),
            );
        }

        self.paste_graph_fragment(envelope)
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
            Err(issue) => {
                return self.reject_paste_response(target, false, vec![*issue], empty_id_remap());
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
                Err((issues, id_remap)) => {
                    return self.reject_paste_response(target, false, issues, id_remap);
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
        let issues = response
            .issues
            .iter()
            .map(|issue| runtime_issue_to_operation_issue(issue, &target))
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
            issues,
        }
    }

    fn reject_paste_response(
        &self,
        target: GraphTargetRef,
        conflict: bool,
        issues: Vec<RuntimeOperationIssue>,
        id_remap: IdRemapResult,
    ) -> PasteGraphFragmentResponse {
        let revision_before = self
            .project
            .as_ref()
            .map(|project| project.graph.revision.clone())
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
            issues,
        }
    }
}

pub(super) fn paste_graph_fragment_into_project_current(
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
        let view_patch = lower_fragment_view_patch(view_revision, request, &id_remap.node_id_map);
        let view_state = if let Some(view_patch) = view_patch {
            let (view_state, _) = apply_view_patch_to_view_state_current(
                &next_graph,
                reconcile_view_state_with_graph_current(
                    &next_graph,
                    Some(project.view_state.clone()),
                ),
                &view_patch,
            )
            .expect("lowered fragment view patch should reference pasted graph nodes");
            next_view_revision += 1;
            view_state
        } else {
            reconcile_view_state_with_graph_current(&next_graph, Some(project.view_state.clone()))
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

pub(super) fn paste_graph_fragment_into_graph_current(
    mut graph: GraphDocumentCurrent,
    request: &PasteGraphFragmentRequest,
) -> Result<(GraphDocumentCurrent, IdRemapResult), (Vec<RuntimeOperationIssue>, IdRemapResult)> {
    if let Some(interface_policy) = request
        .options
        .as_ref()
        .and_then(|options| options.interface_incident_edge_policy)
    {
        return Err((
            vec![RuntimeOperationIssue {
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

    let payload_identity_issues =
        payload_identity_fragment_issues_current(request, &graph.revision);
    if !payload_identity_issues.is_empty() {
        return Err((payload_identity_issues, empty_id_remap()));
    }

    let fragment_analysis =
        skenion_contracts::analyze_graph_fragment_v01(&request.fragment, outside_policy);
    if !fragment_analysis.ok {
        let issues = fragment_analysis
            .issues
            .iter()
            .filter(|issue| issue.severity == "error")
            .map(|issue| RuntimeOperationIssue {
                severity: issue.severity.clone(),
                code: format!("paste.fragment.{}", issue.code),
                message: issue.message.clone(),
                path: None,
                target: Some(request.target.clone()),
                expected_revision: None,
                actual_revision: Some(graph.revision.clone()),
                duplicates: None,
                nodes: issue.nodes.clone(),
                edges: issue.edges.clone(),
                interface_policy: None,
                interface_detail: None,
            })
            .collect();
        return Err((
            issues,
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

fn payload_identity_fragment_issues_current(
    request: &PasteGraphFragmentRequest,
    graph_revision: &str,
) -> Vec<RuntimeOperationIssue> {
    request
        .fragment
        .nodes
        .iter()
        .filter(|node| {
            crate::current_node_identity::graph_node_object_id(node)
                .is_some_and(is_payload_identity_node_kind_current)
        })
        .map(|node| RuntimeOperationIssue {
            severity: "error".to_owned(),
            code: "paste.fragment.payload-node-kind".to_owned(),
            message: format!(
                "node {} uses payload identity {} as an executable implementation",
                node.id,
                crate::current_node_identity::graph_node_object_id(node).unwrap_or("<missing>")
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

pub(super) fn remap_edge_current(
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

pub(super) fn next_available_edge_id(base: &str, used_edge_ids: &HashSet<String>) -> String {
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

pub(super) fn lower_fragment_view_patch(
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

fn operation_issues_from_validation_report(message: String) -> Vec<RuntimeOperationIssue> {
    vec![RuntimeOperationIssue {
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

pub(super) fn runtime_issue_to_operation_issue(
    issue: &RuntimeIssue,
    target: &GraphTargetRef,
) -> RuntimeOperationIssue {
    RuntimeOperationIssue {
        severity: match issue.severity {
            crate::IssueSeverity::Error => "error",
            crate::IssueSeverity::Warning => "warning",
            crate::IssueSeverity::Info => "info",
        }
        .to_owned(),
        code: issue
            .code
            .clone()
            .unwrap_or_else(|| "paste.lowering.failed".to_owned()),
        message: issue.message.clone(),
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
