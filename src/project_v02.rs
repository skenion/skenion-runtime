use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;

use crate::{
    CycleValidationV02, EdgeSpecV02, ExecutionGroup, ExecutionModel, ExecutionModelV02,
    FanOutPolicyV02, GraphDocumentV02, GraphValidationResultV02, MergePolicyV02, NodeDefinitionV02,
    PlanEdge, PlanEdgeMetadata, PlanNode, RuntimeDiagnostic,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRequestV02 {
    pub graph: GraphDocumentV02,
    pub nodes: Vec<NodeDefinitionV02>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunProjectRequestV02 {
    pub graph: GraphDocumentV02,
    pub nodes: Vec<NodeDefinitionV02>,
    pub frames: Option<usize>,
}

type V02Validation =
    Result<(Vec<RuntimeDiagnostic>, GraphValidationResultV02), Vec<RuntimeDiagnostic>>;

pub fn validate_project_v02(
    graph: &GraphDocumentV02,
    nodes: &[NodeDefinitionV02],
) -> V02Validation {
    let mut diagnostics = Vec::new();
    let mut registry: HashMap<(&str, &str), &NodeDefinitionV02> = HashMap::new();

    for definition in nodes {
        if let Err(report) = skenion_contracts::validate_node_definition_v02(definition) {
            diagnostics.extend(
                report
                    .errors()
                    .iter()
                    .map(|error| RuntimeDiagnostic::error(error.message.clone())),
            );
        }
        registry.insert(
            (definition.id.as_str(), definition.version.as_str()),
            definition,
        );
    }

    let graph_analysis = skenion_contracts::analyze_graph_document_v02(graph);
    diagnostics.extend(graph_analysis.diagnostics.iter().map(|diagnostic| {
        let message = format!("{}: {}", diagnostic.code, diagnostic.message);
        if diagnostic.severity == "warning" {
            RuntimeDiagnostic::warning(message)
        } else {
            RuntimeDiagnostic::error(message)
        }
    }));

    for node in &graph.nodes {
        match registry.get(&(node.kind.as_str(), node.kind_version.as_str())) {
            Some(definition) => validate_node_snapshot_v02(node, definition, &mut diagnostics),
            None => diagnostics.push(RuntimeDiagnostic::error(format!(
                "missing node definition: {}@{}",
                node.kind, node.kind_version
            ))),
        }
    }

    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == crate::DiagnosticSeverity::Error)
    {
        Err(diagnostics)
    } else {
        Ok((diagnostics, graph_analysis))
    }
}

pub fn build_execution_plan_v02(
    graph: &GraphDocumentV02,
    nodes: &[NodeDefinitionV02],
) -> Result<(crate::ExecutionPlan, Vec<RuntimeDiagnostic>), Vec<RuntimeDiagnostic>> {
    let (diagnostics, analysis) = validate_project_v02(graph, nodes)?;
    let registry = nodes
        .iter()
        .map(|definition| {
            (
                (definition.id.as_str(), definition.version.as_str()),
                definition,
            )
        })
        .collect::<HashMap<_, _>>();
    let mut groups_by_model: BTreeMap<String, ExecutionGroup> = BTreeMap::new();
    let mut plan_nodes = Vec::new();

    for (order, node) in graph.nodes.iter().enumerate() {
        let definition = registry
            .get(&(node.kind.as_str(), node.kind_version.as_str()))
            .expect("v0.2 validation should resolve definitions");
        let execution_model = map_execution_model_v02(&definition.execution.model);
        plan_nodes.push(PlanNode {
            node_id: node.id.clone(),
            kind: node.kind.clone(),
            kind_version: node.kind_version.clone(),
            execution_model: execution_model.clone(),
            order,
        });
        groups_by_model
            .entry(format!("{execution_model:?}"))
            .or_insert_with(|| ExecutionGroup {
                execution_model: execution_model.clone(),
                node_ids: Vec::new(),
            })
            .node_ids
            .push(node.id.clone());
    }

    Ok((
        crate::ExecutionPlan {
            graph_id: graph.id.clone(),
            graph_revision: graph.revision.clone(),
            nodes: plan_nodes,
            edges: graph
                .edges
                .iter()
                .map(|edge| plan_edge_v02(graph, edge, &analysis))
                .collect(),
            groups: groups_by_model.into_values().collect(),
        },
        diagnostics,
    ))
}

fn validate_node_snapshot_v02(
    node: &crate::GraphNodeV02,
    definition: &NodeDefinitionV02,
    diagnostics: &mut Vec<RuntimeDiagnostic>,
) {
    let definition_ports = definition
        .ports
        .iter()
        .map(|port| (port.id.as_str(), port))
        .collect::<HashMap<_, _>>();
    let snapshot_ports = node
        .ports
        .iter()
        .map(|port| (port.id.as_str(), port))
        .collect::<HashMap<_, _>>();

    for definition_port in &definition.ports {
        if !snapshot_ports.contains_key(definition_port.id.as_str()) {
            diagnostics.push(RuntimeDiagnostic::error(format!(
                "port snapshot missing manifest port: {}.{}",
                node.id, definition_port.id
            )));
        }
    }

    for snapshot_port in &node.ports {
        let Some(definition_port) = definition_ports.get(snapshot_port.id.as_str()) else {
            diagnostics.push(RuntimeDiagnostic::error(format!(
                "port snapshot references missing manifest port: {}.{}",
                node.id, snapshot_port.id
            )));
            continue;
        };

        if snapshot_port.direction != definition_port.direction {
            diagnostics.push(RuntimeDiagnostic::error(format!(
                "port snapshot mismatch: {}.{} direction differs from definition",
                node.id, snapshot_port.id
            )));
        }
        if snapshot_port.port_type != definition_port.port_type {
            diagnostics.push(RuntimeDiagnostic::error(format!(
                "port snapshot mismatch: {}.{} type {} != definition type {}",
                node.id, snapshot_port.id, snapshot_port.port_type, definition_port.port_type
            )));
        }
    }
}

fn plan_edge_v02(
    graph: &GraphDocumentV02,
    edge: &EdgeSpecV02,
    analysis: &GraphValidationResultV02,
) -> PlanEdge {
    let source = find_port(graph, &edge.source.node_id, &edge.source.port_id)
        .expect("v0.2 validation should resolve source port");
    let target = find_port(graph, &edge.target.node_id, &edge.target.port_id)
        .expect("v0.2 validation should resolve target port");

    PlanEdge {
        from_node: edge.source.node_id.clone(),
        from_port: edge.source.port_id.clone(),
        to_node: edge.target.node_id.clone(),
        to_port: edge.target.port_id.clone(),
        metadata: Some(PlanEdgeMetadata {
            resolved_type: Some(
                edge.resolved_type
                    .clone()
                    .unwrap_or_else(|| source.port_type.clone()),
            ),
            merge_policy: Some(merge_policy_label(target.merge_policy.as_ref())),
            fan_out_policy: Some(fan_out_policy_label(source.fan_out_policy.as_ref())),
            order: edge.order,
            feedback: edge.feedback.clone(),
            cycle_classification: cycle_classification_for_edge(edge, analysis),
        }),
    }
}

fn find_port<'a>(
    graph: &'a GraphDocumentV02,
    node_id: &str,
    port_id: &str,
) -> Option<&'a crate::PortSpecV02> {
    graph
        .nodes
        .iter()
        .find(|node| node.id == node_id)?
        .ports
        .iter()
        .find(|port| port.id == port_id)
}

fn cycle_classification_for_edge(
    edge: &EdgeSpecV02,
    analysis: &GraphValidationResultV02,
) -> Option<String> {
    analysis
        .cycles
        .iter()
        .find(|cycle| cycle.edges.iter().any(|edge_id| edge_id == &edge.id))
        .map(|cycle| cycle_validation_label(&cycle.classification).to_owned())
}

fn cycle_validation_label(classification: &CycleValidationV02) -> &'static str {
    match classification {
        CycleValidationV02::NoCycle => "no-cycle",
        CycleValidationV02::ValidFeedback => "valid-feedback",
        CycleValidationV02::RiskyFeedback => "risky-feedback",
        CycleValidationV02::AmbiguousAlgebraicLoop => "ambiguous-algebraic-loop",
        CycleValidationV02::InvalidCycle => "invalid-cycle",
    }
}

fn merge_policy_label(policy: Option<&MergePolicyV02>) -> String {
    match policy {
        Some(MergePolicyV02::OrderedEvents) => "ordered-events",
        Some(MergePolicyV02::Mix) => "mix",
        Some(MergePolicyV02::Array) => "array",
        Some(MergePolicyV02::Latest) => "latest",
        Some(MergePolicyV02::First) => "first",
        Some(MergePolicyV02::Custom) => "custom",
        Some(MergePolicyV02::Forbid) | None => "forbid",
    }
    .to_owned()
}

fn fan_out_policy_label(policy: Option<&FanOutPolicyV02>) -> String {
    match policy {
        Some(FanOutPolicyV02::Forbid) => "forbid",
        Some(FanOutPolicyV02::Copy) => "copy",
        Some(FanOutPolicyV02::Share) => "share",
        Some(FanOutPolicyV02::Allow) | None => "allow",
    }
    .to_owned()
}

fn map_execution_model_v02(model: &ExecutionModelV02) -> ExecutionModel {
    match model {
        ExecutionModelV02::Event => ExecutionModel::Event,
        ExecutionModelV02::Value => ExecutionModel::Value,
        ExecutionModelV02::Frame => ExecutionModel::Frame,
        ExecutionModelV02::AudioBlock => ExecutionModel::AudioBlock,
        ExecutionModelV02::VideoFrame => ExecutionModel::VideoFrame,
        ExecutionModelV02::GpuPass => ExecutionModel::GpuPass,
        ExecutionModelV02::AsyncResource => ExecutionModel::AsyncResource,
        ExecutionModelV02::ScriptControl => ExecutionModel::ScriptControl,
        ExecutionModelV02::NativePlugin => ExecutionModel::NativePlugin,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::{DiagnosticSeverity, FeedbackBoundaryV02, PortDirectionV02, PortSpecV02};

    fn graph(value: Value) -> GraphDocumentV02 {
        serde_json::from_value(value).expect("graph should parse")
    }

    fn definition(value: Value) -> NodeDefinitionV02 {
        serde_json::from_value(value).expect("definition should parse")
    }

    fn clear_definition() -> NodeDefinitionV02 {
        definition(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.2.0",
          "id": "render.clear-color",
          "version": "0.2.0",
          "displayName": "Clear Color",
          "category": "Render",
          "ports": [
            { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
          ],
          "execution": { "model": "gpu_pass", "clock": "frame" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }))
    }

    fn output_definition() -> NodeDefinitionV02 {
        definition(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.2.0",
          "id": "render.output",
          "version": "0.2.0",
          "displayName": "Render Output",
          "category": "Render",
          "ports": [
            { "id": "in", "direction": "input", "type": "render.frame", "rate": "render", "required": true }
          ],
          "execution": { "model": "gpu_pass", "clock": "frame" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }))
    }

    fn render_graph() -> GraphDocumentV02 {
        graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.2.0",
          "id": "render",
          "revision": "1",
          "nodes": [
            {
              "id": "clear",
              "kind": "render.clear-color",
              "kindVersion": "0.2.0",
              "params": {},
              "ports": [
                { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
              ]
            },
            {
              "id": "output",
              "kind": "render.output",
              "kindVersion": "0.2.0",
              "params": {},
              "ports": [
                { "id": "in", "direction": "input", "type": "render.frame", "rate": "render", "required": true }
              ]
            }
          ],
          "edges": [
            {
              "id": "edge_clear_output",
              "source": { "nodeId": "clear", "portId": "out" },
              "target": { "nodeId": "output", "portId": "in" },
              "resolvedType": "render.frame"
            }
          ]
        }))
    }

    #[test]
    fn validates_and_builds_v02_plan_metadata() {
        let graph = render_graph();
        let nodes = vec![clear_definition(), output_definition()];
        let (warnings, analysis) =
            validate_project_v02(&graph, &nodes).expect("project should validate");
        assert!(warnings.is_empty());
        assert!(analysis.cycles.is_empty());

        let (plan, diagnostics) =
            build_execution_plan_v02(&graph, &nodes).expect("plan should build");
        assert!(diagnostics.is_empty());
        assert_eq!(plan.graph_id, "render");
        assert_eq!(plan.nodes.len(), 2);
        assert_eq!(plan.groups.len(), 1);
        let metadata = plan.edges[0]
            .metadata
            .as_ref()
            .expect("metadata should exist");
        assert_eq!(metadata.resolved_type.as_deref(), Some("render.frame"));
        assert_eq!(metadata.merge_policy.as_deref(), Some("forbid"));
        assert_eq!(metadata.fan_out_policy.as_deref(), Some("allow"));
        assert_eq!(metadata.cycle_classification, None);
    }

    #[test]
    fn records_merge_order_feedback_and_risky_warnings() {
        let mut graph = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.2.0",
          "id": "feedback",
          "revision": "1",
          "nodes": [
            {
              "id": "node",
              "kind": "render.feedback-composite",
              "kindVersion": "0.2.0",
              "params": {},
              "ports": [
                { "id": "previous", "direction": "input", "type": "render.frame", "rate": "render" },
                { "id": "out", "direction": "output", "type": "render.frame", "rate": "render", "fanOutPolicy": "copy" }
              ]
            }
          ],
          "edges": [
            {
              "id": "edge_feedback",
              "source": { "nodeId": "node", "portId": "out" },
              "target": { "nodeId": "node", "portId": "previous" },
              "order": 3,
              "feedback": { "enabled": true, "boundary": "render-frame", "intentional": true }
            }
          ]
        }));
        let definition = definition(json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.2.0",
          "id": "render.feedback-composite",
          "version": "0.2.0",
          "displayName": "Feedback",
          "category": "Render",
          "ports": [
            { "id": "previous", "direction": "input", "type": "render.frame", "rate": "render" },
            { "id": "out", "direction": "output", "type": "render.frame", "rate": "render", "fanOutPolicy": "copy" }
          ],
          "execution": { "model": "gpu_pass", "clock": "frame" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": []
        }));

        let (plan, diagnostics) =
            build_execution_plan_v02(&graph, std::slice::from_ref(&definition))
                .expect("feedback should plan");
        assert!(diagnostics.is_empty());
        let metadata = plan.edges[0].metadata.as_ref().unwrap();
        assert_eq!(metadata.order, Some(3));
        assert_eq!(metadata.fan_out_policy.as_deref(), Some("copy"));
        assert_eq!(
            metadata.cycle_classification.as_deref(),
            Some("valid-feedback")
        );
        assert_eq!(
            metadata.feedback.as_ref().unwrap().boundary,
            FeedbackBoundaryV02::RenderFrame
        );

        graph.edges[0].feedback.as_mut().unwrap().boundary = FeedbackBoundaryV02::SameTurn;
        let (_plan, diagnostics) =
            build_execution_plan_v02(&graph, &[definition]).expect("risky feedback should plan");
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Warning);
        assert!(diagnostics[0].message.contains("risky-feedback"));
    }

    #[test]
    fn rejects_invalid_graph_definitions_and_snapshots() {
        let graph = render_graph();
        let missing = validate_project_v02(&graph, &[]).expect_err("missing definitions fail");
        assert!(missing[0].message.contains("missing node definition"));

        let mut invalid_definition = clear_definition();
        invalid_definition.permissions.push("network".to_owned());
        let invalid_definition_result =
            validate_project_v02(&graph, &[invalid_definition, output_definition()])
                .expect_err("invalid definition should fail");
        assert!(
            invalid_definition_result
                .iter()
                .any(|diagnostic| diagnostic.message.contains("unsupported permission"))
        );

        let mut mismatch = render_graph();
        mismatch.nodes[0].ports.clear();
        mismatch.nodes[1].ports[0].direction = PortDirectionV02::Output;
        mismatch.nodes[1].ports[0].port_type = "value.number".to_owned();
        mismatch.nodes[1].ports.push(PortSpecV02 {
            id: "extra".to_owned(),
            direction: PortDirectionV02::Input,
            port_type: "render.frame".to_owned(),
            label: None,
            rate: None,
            accepts: None,
            min_connections: None,
            max_connections: None,
            merge_policy: None,
            fan_out_policy: None,
            trigger_mode: None,
            default_value: None,
            latch: None,
            required: None,
            style_key: None,
            group: None,
            description: None,
        });
        let mismatch_result =
            validate_project_v02(&mismatch, &[clear_definition(), output_definition()])
                .expect_err("snapshot mismatch should fail");
        let messages = mismatch_result
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(messages.contains("missing manifest port"));
        assert!(messages.contains("direction differs from definition"));
        assert!(messages.contains("type value.number"));
        assert!(messages.contains("missing source port"));
        assert!(messages.contains("missing manifest port: output.extra"));
    }

    #[test]
    fn labels_all_v02_policy_and_execution_variants() {
        for (policy, expected) in [
            (Some(MergePolicyV02::Forbid), "forbid"),
            (Some(MergePolicyV02::OrderedEvents), "ordered-events"),
            (Some(MergePolicyV02::Mix), "mix"),
            (Some(MergePolicyV02::Array), "array"),
            (Some(MergePolicyV02::Latest), "latest"),
            (Some(MergePolicyV02::First), "first"),
            (Some(MergePolicyV02::Custom), "custom"),
            (None, "forbid"),
        ] {
            assert_eq!(merge_policy_label(policy.as_ref()), expected);
        }

        for (policy, expected) in [
            (Some(FanOutPolicyV02::Allow), "allow"),
            (Some(FanOutPolicyV02::Forbid), "forbid"),
            (Some(FanOutPolicyV02::Copy), "copy"),
            (Some(FanOutPolicyV02::Share), "share"),
            (None, "allow"),
        ] {
            assert_eq!(fan_out_policy_label(policy.as_ref()), expected);
        }

        for (classification, expected) in [
            (CycleValidationV02::NoCycle, "no-cycle"),
            (CycleValidationV02::ValidFeedback, "valid-feedback"),
            (CycleValidationV02::RiskyFeedback, "risky-feedback"),
            (
                CycleValidationV02::AmbiguousAlgebraicLoop,
                "ambiguous-algebraic-loop",
            ),
            (CycleValidationV02::InvalidCycle, "invalid-cycle"),
        ] {
            assert_eq!(cycle_validation_label(&classification), expected);
        }

        for (model, expected) in [
            (ExecutionModelV02::Event, ExecutionModel::Event),
            (ExecutionModelV02::Value, ExecutionModel::Value),
            (ExecutionModelV02::Frame, ExecutionModel::Frame),
            (ExecutionModelV02::AudioBlock, ExecutionModel::AudioBlock),
            (ExecutionModelV02::VideoFrame, ExecutionModel::VideoFrame),
            (ExecutionModelV02::GpuPass, ExecutionModel::GpuPass),
            (
                ExecutionModelV02::AsyncResource,
                ExecutionModel::AsyncResource,
            ),
            (
                ExecutionModelV02::ScriptControl,
                ExecutionModel::ScriptControl,
            ),
            (
                ExecutionModelV02::NativePlugin,
                ExecutionModel::NativePlugin,
            ),
        ] {
            assert_eq!(map_execution_model_v02(&model), expected);
        }
    }
}
