use super::*;

#[derive(Debug, Clone, Copy)]
pub(super) enum HistoryDirection {
    Undo,
    Redo,
}

#[derive(Debug, Clone)]
pub(super) enum HistoryEntry {
    Mutation {
        event_id: String,
        actor_id: Option<String>,
        mutation: RuntimeMutationRequest,
        inverse_mutation: RuntimeMutationRequest,
    },
    ProjectDocument {
        event_id: String,
        actor_id: Option<String>,
        before: Box<ProjectDocumentCurrent>,
        after: Box<ProjectDocumentCurrent>,
        before_view_revision: u64,
        after_view_revision: u64,
        mutation: RuntimeMutationRequest,
        inverse_mutation: RuntimeMutationRequest,
    },
}

impl HistoryEntry {
    pub(super) fn actor_id(&self) -> Option<&str> {
        match self {
            Self::Mutation { actor_id, .. } => actor_id.as_deref(),
            Self::ProjectDocument { actor_id, .. } => actor_id.as_deref(),
        }
    }
}

pub(super) struct HistoryApplyOutcome {
    pub(super) applied: bool,
    pub(super) response: RuntimePatchResponse,
}

impl HistoryApplyOutcome {
    pub(super) fn applied(response: RuntimePatchResponse) -> Self {
        Self {
            applied: true,
            response,
        }
    }

    pub(super) fn rejected(response: RuntimePatchResponse) -> Self {
        Self {
            applied: false,
            response,
        }
    }
}

impl RuntimeSession {
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
                vec![RuntimeIssue::error("no patch event available to undo")],
            );
        };
        let outcome = self.apply_history_entry(entry.clone(), HistoryDirection::Undo);
        if outcome.applied {
            let response = outcome.response;
            self.redo_stack.push(entry);
            self.patch_response(true, true, false, response.issues)
        } else {
            let response = outcome.response;
            self.undo_stack.push(entry);
            self.patch_response(false, false, response.conflict, response.issues)
        }
    }

    pub fn undo_for_actor(&mut self, actor_id: &str) -> RuntimePatchResponse {
        let Some(index) = self
            .undo_stack
            .iter()
            .rposition(|entry| entry.actor_id() == Some(actor_id))
        else {
            return self.patch_response(
                false,
                false,
                false,
                vec![RuntimeIssue::error(format!(
                    "no patch event available to undo for actor {actor_id}"
                ))],
            );
        };
        let entry = self.undo_stack.remove(index);
        let outcome = self.apply_history_entry(entry.clone(), HistoryDirection::Undo);
        if outcome.applied {
            let response = outcome.response;
            self.redo_stack.push(entry);
            self.patch_response(true, true, false, response.issues)
        } else {
            let response = outcome.response;
            self.undo_stack.insert(index, entry);
            self.patch_response(false, false, response.conflict, response.issues)
        }
    }

    pub fn redo(&mut self) -> RuntimePatchResponse {
        let Some(entry) = self.redo_stack.pop() else {
            return self.patch_response(
                false,
                false,
                false,
                vec![RuntimeIssue::error("no patch event available to redo")],
            );
        };
        let outcome = self.apply_history_entry(entry.clone(), HistoryDirection::Redo);
        if outcome.applied {
            let response = outcome.response;
            self.undo_stack.push(entry);
            self.patch_response(true, true, false, response.issues)
        } else {
            let response = outcome.response;
            self.redo_stack.push(entry);
            self.patch_response(false, false, response.conflict, response.issues)
        }
    }

    pub fn redo_for_actor(&mut self, actor_id: &str) -> RuntimePatchResponse {
        let Some(index) = self
            .redo_stack
            .iter()
            .rposition(|entry| entry.actor_id() == Some(actor_id))
        else {
            return self.patch_response(
                false,
                false,
                false,
                vec![RuntimeIssue::error(format!(
                    "no patch event available to redo for actor {actor_id}"
                ))],
            );
        };
        let entry = self.redo_stack.remove(index);
        let outcome = self.apply_history_entry(entry.clone(), HistoryDirection::Redo);
        if outcome.applied {
            let response = outcome.response;
            self.undo_stack.push(entry);
            self.patch_response(true, true, false, response.issues)
        } else {
            let response = outcome.response;
            self.redo_stack.insert(index, entry);
            self.patch_response(false, false, response.conflict, response.issues)
        }
    }
}

pub(super) fn project_document_history_delta(
    current: &ProjectDocumentCurrent,
    before: &ProjectDocumentCurrent,
    after: &ProjectDocumentCurrent,
    direction: HistoryDirection,
) -> ProjectDocumentCurrent {
    let (expected_current, exact_target) = match direction {
        HistoryDirection::Undo => (after, before),
        HistoryDirection::Redo => (before, after),
    };
    if current == expected_current {
        return exact_target.clone();
    }

    let mut project = current.clone();
    apply_graph_history_delta_current(&mut project.graph, &before.graph, &after.graph, direction);
    project.view_state = view_state_history_delta_current(
        &project.view_state,
        &before.view_state,
        &after.view_state,
        direction,
    );

    for patch in &mut project.patch_library {
        let Some(before_patch) = before
            .patch_library
            .iter()
            .find(|entry| entry.id == patch.id)
        else {
            continue;
        };
        let Some(after_patch) = after
            .patch_library
            .iter()
            .find(|entry| entry.id == patch.id)
        else {
            continue;
        };
        if apply_graph_history_delta_current(
            &mut patch.graph,
            &before_patch.graph,
            &after_patch.graph,
            direction,
        ) {
            patch.graph.revision = next_graph_revision(&patch.graph.revision);
            patch.revision = patch.graph.revision.clone();
        }
    }

    project
}

pub(super) fn apply_graph_history_delta_current(
    current: &mut GraphDocumentCurrent,
    before: &GraphDocumentCurrent,
    after: &GraphDocumentCurrent,
    direction: HistoryDirection,
) -> bool {
    match direction {
        HistoryDirection::Undo => undo_graph_history_delta_current(current, before, after),
        HistoryDirection::Redo => redo_graph_history_delta_current(current, before, after),
    }
}

pub(super) fn undo_graph_history_delta_current(
    current: &mut GraphDocumentCurrent,
    before: &GraphDocumentCurrent,
    after: &GraphDocumentCurrent,
) -> bool {
    let before_node_ids = before
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    let added_node_ids = after
        .nodes
        .iter()
        .filter(|node| !before_node_ids.contains(node.id.as_str()))
        .map(|node| node.id.clone())
        .collect::<HashSet<_>>();
    let before_edge_ids = before
        .edges
        .iter()
        .map(|edge| edge.id.as_str())
        .collect::<HashSet<_>>();
    let added_edge_ids = after
        .edges
        .iter()
        .filter(|edge| !before_edge_ids.contains(edge.id.as_str()))
        .map(|edge| edge.id.clone())
        .collect::<HashSet<_>>();

    let before_nodes = before
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    let after_nodes = after
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<BTreeMap<_, _>>();

    let original_nodes_len = current.nodes.len();
    current
        .nodes
        .retain(|node| !added_node_ids.contains(node.id.as_str()));
    let mut changed = current.nodes.len() != original_nodes_len;

    for node in &mut current.nodes {
        let Some(before_node) = before_nodes.get(node.id.as_str()) else {
            continue;
        };
        let Some(after_node) = after_nodes.get(node.id.as_str()) else {
            continue;
        };
        if node == *after_node {
            *node = (*before_node).clone();
            changed = true;
        }
    }

    let original_edges_len = current.edges.len();
    current.edges.retain(|edge| {
        !added_edge_ids.contains(edge.id.as_str())
            && !added_node_ids.contains(edge.source.node_id.as_str())
            && !added_node_ids.contains(edge.target.node_id.as_str())
    });
    changed |= current.edges.len() != original_edges_len;

    changed
}

pub(super) fn redo_graph_history_delta_current(
    current: &mut GraphDocumentCurrent,
    before: &GraphDocumentCurrent,
    after: &GraphDocumentCurrent,
) -> bool {
    let before_node_ids = before
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    let current_node_ids = current
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .collect::<HashSet<_>>();
    let before_nodes = before
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    let after_nodes = after
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    let mut changed = false;

    for node in &mut current.nodes {
        let Some(before_node) = before_nodes.get(node.id.as_str()) else {
            continue;
        };
        let Some(after_node) = after_nodes.get(node.id.as_str()) else {
            continue;
        };
        if node == *before_node {
            *node = (*after_node).clone();
            changed = true;
        }
    }
    for node in &after.nodes {
        if !before_node_ids.contains(node.id.as_str()) && !current_node_ids.contains(&node.id) {
            current.nodes.push(node.clone());
            changed = true;
        }
    }

    let before_edge_ids = before
        .edges
        .iter()
        .map(|edge| edge.id.as_str())
        .collect::<HashSet<_>>();
    let current_edge_ids = current
        .edges
        .iter()
        .map(|edge| edge.id.clone())
        .collect::<HashSet<_>>();
    let current_node_ids = current
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    for edge in &after.edges {
        if before_edge_ids.contains(edge.id.as_str()) || current_edge_ids.contains(&edge.id) {
            continue;
        }
        if current_node_ids.contains(edge.source.node_id.as_str())
            && current_node_ids.contains(edge.target.node_id.as_str())
        {
            current.edges.push(edge.clone());
            changed = true;
        }
    }

    changed
}

pub(super) fn view_state_history_delta_current(
    current: &ViewState,
    before: &ViewState,
    after: &ViewState,
    direction: HistoryDirection,
) -> ViewState {
    let mut next = current.clone();
    match direction {
        HistoryDirection::Undo => {
            for node_id in after.canvas.nodes.keys() {
                if !before.canvas.nodes.contains_key(node_id) {
                    next.canvas.nodes.remove(node_id);
                }
            }
            for (node_id, before_view) in &before.canvas.nodes {
                let Some(after_view) = after.canvas.nodes.get(node_id) else {
                    continue;
                };
                if next.canvas.nodes.get(node_id) == Some(after_view) {
                    next.canvas
                        .nodes
                        .insert(node_id.clone(), before_view.clone());
                }
            }
        }
        HistoryDirection::Redo => {
            for (node_id, after_view) in &after.canvas.nodes {
                if !before.canvas.nodes.contains_key(node_id) {
                    next.canvas
                        .nodes
                        .entry(node_id.clone())
                        .or_insert_with(|| after_view.clone());
                }
            }
            for (node_id, before_view) in &before.canvas.nodes {
                let Some(after_view) = after.canvas.nodes.get(node_id) else {
                    continue;
                };
                if next.canvas.nodes.get(node_id) == Some(before_view) {
                    next.canvas
                        .nodes
                        .insert(node_id.clone(), after_view.clone());
                }
            }
        }
    }
    next
}
