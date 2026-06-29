use serde_json::json;

use crate::{
    ExecutionPlan, GraphDocument, GraphDocumentCurrent, NodeRegistry, PlanError, RuntimeDiagnostic,
    build_execution_plan,
};

const UNRESOLVED_OBJECT_NODE_KIND: &str = "object.core.unresolved";

pub(super) fn unresolved_object_diagnostics(graph: &GraphDocument) -> Vec<RuntimeDiagnostic> {
    graph
        .nodes
        .iter()
        .filter(|node| node.kind == UNRESOLVED_OBJECT_NODE_KIND)
        .map(|node| {
            let object_text = node
                .params
                .get("objectText")
                .and_then(|value| value.as_str())
                .unwrap_or(node.id.as_str());
            let diagnostic_message = node
                .params
                .get("diagnosticMessage")
                .and_then(|value| value.as_str())
                .unwrap_or("object text could not be resolved");
            RuntimeDiagnostic::error(format!(
                "unresolved object {object_text}: {diagnostic_message}"
            ))
        })
        .collect()
}

pub(super) fn unresolved_object_diagnostics_current(
    graph: &GraphDocumentCurrent,
) -> Vec<RuntimeDiagnostic> {
    graph
        .nodes
        .iter()
        .filter(|node| node.kind == UNRESOLVED_OBJECT_NODE_KIND)
        .map(|node| {
            let object_text = node
                .params
                .get("objectText")
                .and_then(|value| value.as_str())
                .unwrap_or(node.id.as_str());
            let diagnostic_message = node
                .params
                .get("diagnosticMessage")
                .and_then(|value| value.as_str())
                .unwrap_or("object text could not be resolved");
            RuntimeDiagnostic::error(format!(
                "unresolved object {object_text}: {diagnostic_message}"
            ))
        })
        .collect()
}

pub(super) fn build_session_execution_plan(
    graph: &GraphDocument,
    registry: &NodeRegistry,
    surface: &'static str,
) -> Result<ExecutionPlan, Vec<RuntimeDiagnostic>> {
    build_execution_plan(graph, registry).map_err(|error| {
        let mut diagnostics = plan_error_diagnostics(error, surface, graph);
        diagnostics.extend(unresolved_object_diagnostics(graph));
        diagnostics
    })
}

fn plan_error_diagnostics(
    error: PlanError,
    surface: &'static str,
    graph: &GraphDocument,
) -> Vec<RuntimeDiagnostic> {
    let details = || {
        json!({
            "surface": surface,
            "graphId": graph.id,
            "graphRevision": graph.revision,
        })
    };
    match error {
        PlanError::InvalidProject(report) => report
            .errors()
            .iter()
            .map(|error| {
                RuntimeDiagnostic::structured_error(
                    "session.plan.invalid-project",
                    error.message.clone(),
                    details(),
                )
            })
            .collect(),
        PlanError::Cycle { nodes } => vec![RuntimeDiagnostic::structured_error(
            "session.plan.cycle",
            format!("cycle detected: {nodes}"),
            json!({
                "surface": surface,
                "graphId": graph.id,
                "graphRevision": graph.revision,
                "nodes": nodes,
            }),
        )],
    }
}
