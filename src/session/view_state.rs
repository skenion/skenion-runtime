#[cfg(test)]
use crate::GraphDocument;
use crate::{
    CanvasNodeView, CanvasViewState, GraphDocumentCurrent, GraphTargetRef, PatchPath, RuntimeIssue,
    RuntimeViewPatch, RuntimeViewPatchOperation, ViewState,
};

pub(super) fn reconcile_view_state_with_graph_current(
    graph: &GraphDocumentCurrent,
    view_state: Option<ViewState>,
) -> ViewState {
    let mut reconciled = default_view_state_for_graph_current(graph);
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

    reconciled
}

fn default_view_state_for_graph_current(graph: &GraphDocumentCurrent) -> ViewState {
    ViewState {
        schema: "skenion.view-state".to_owned(),
        schema_version: "0.1.0".to_owned(),
        canvas: CanvasViewState {
            nodes: graph
                .nodes
                .iter()
                .enumerate()
                .map(|(index, node)| (node.id.clone(), default_canvas_node_view_for_index(index)))
                .collect(),
        },
    }
}

fn default_canvas_node_view_for_index(index: usize) -> CanvasNodeView {
    CanvasNodeView {
        x: 160.0 * (index as f64),
        y: 0.0,
        width: None,
        height: None,
        collapsed: None,
    }
}

pub(super) fn runtime_owned_view_state(view_state: ViewState) -> ViewState {
    view_state
}

pub(super) fn target_supports_view_state(path: &PatchPath) -> bool {
    matches!(path, PatchPath::Root | PatchPath::HelpWorkingCopy { .. })
}

pub(super) fn unsupported_patch_view_change_issue(target: &GraphTargetRef) -> RuntimeIssue {
    RuntimeIssue::structured_error(
        "collaboration.patch-view-unsupported",
        "project patch definition targets do not currently carry editable view state in Runtime",
        serde_json::json!({ "target": target }),
    )
}

#[cfg(test)]
pub(super) fn apply_view_patch_to_view_state(
    graph: &GraphDocument,
    view_state: ViewState,
    patch: &RuntimeViewPatch,
) -> Result<(ViewState, RuntimeViewPatch), Vec<RuntimeIssue>> {
    apply_view_patch_to_view_state_with_node_lookup(
        |node_id| graph.nodes.iter().any(|node| node.id == node_id),
        view_state,
        patch,
    )
}

pub(super) fn apply_view_patch_to_view_state_current(
    graph: &GraphDocumentCurrent,
    view_state: ViewState,
    patch: &RuntimeViewPatch,
) -> Result<(ViewState, RuntimeViewPatch), Vec<RuntimeIssue>> {
    apply_view_patch_to_view_state_with_node_lookup(
        |node_id| graph.nodes.iter().any(|node| node.id == node_id),
        view_state,
        patch,
    )
}

fn apply_view_patch_to_view_state_with_node_lookup(
    has_node_id: impl Fn(&str) -> bool,
    mut view_state: ViewState,
    patch: &RuntimeViewPatch,
) -> Result<(ViewState, RuntimeViewPatch), Vec<RuntimeIssue>> {
    let mut inverse_ops = Vec::new();
    for op in &patch.ops {
        match op {
            RuntimeViewPatchOperation::SetNodeView { node_id, view } => {
                if !has_node_id(node_id) {
                    return Err(vec![RuntimeIssue::error(format!(
                        "view patch node {node_id} does not exist"
                    ))]);
                }
                let Some(previous) = view_state.canvas.nodes.get(node_id).cloned() else {
                    return Err(vec![RuntimeIssue::error(format!(
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
                if !has_node_id(node_id) {
                    return Err(vec![RuntimeIssue::error(format!(
                        "view patch node {node_id} does not exist"
                    ))]);
                }
                let Some(previous) = view_state.canvas.nodes.get(node_id).cloned() else {
                    return Err(vec![RuntimeIssue::error(format!(
                        "view patch node {node_id} has no view state"
                    ))]);
                };
                if let Some(from) = from
                    && from != &previous
                {
                    return Err(vec![RuntimeIssue::error(format!(
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
