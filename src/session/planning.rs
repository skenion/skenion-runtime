use crate::{GraphDocumentCurrent, ObjectResolutionStatusCurrent, RuntimeIssue};

pub(super) fn unresolved_object_issues_current(graph: &GraphDocumentCurrent) -> Vec<RuntimeIssue> {
    graph
        .nodes
        .iter()
        .filter(|node| {
            node.object_resolution.as_ref().is_some_and(|resolution| {
                resolution.status != ObjectResolutionStatusCurrent::Resolved
            })
        })
        .map(|node| {
            let object_spec = node.object_spec.as_deref().unwrap_or(node.id.as_str());
            let issue_message = node
                .object_resolution
                .as_ref()
                .and_then(|resolution| resolution.issues.first())
                .map(|issue| issue.message.as_str())
                .unwrap_or("object spec could not be resolved");
            RuntimeIssue::error(format!("unresolved object {object_spec}: {issue_message}"))
        })
        .collect()
}
