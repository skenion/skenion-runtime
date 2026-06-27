use std::collections::{BTreeMap, HashMap, VecDeque};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    Edge, ExecutionModel, FeedbackPolicyCurrent, GraphDocument, NodeRegistry,
    ProjectValidationReport, validate_project,
};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPlan {
    pub(crate) graph_id: String,
    pub(crate) graph_revision: String,
    pub(crate) nodes: Vec<PlanNode>,
    pub(crate) edges: Vec<PlanEdge>,
    pub(crate) groups: Vec<ExecutionGroup>,
}

impl ExecutionPlan {
    pub fn graph_id(&self) -> &str {
        &self.graph_id
    }

    pub fn graph_revision(&self) -> &str {
        &self.graph_revision
    }

    pub fn node_ids(&self) -> impl Iterator<Item = &str> {
        self.nodes.iter().map(|node| node.node_id.as_str())
    }

    pub fn contains_node(&self, node_id: &str) -> bool {
        self.node_ids().any(|candidate| candidate == node_id)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlanNode {
    pub(crate) node_id: String,
    pub(crate) kind: String,
    pub(crate) kind_version: String,
    pub(crate) execution_model: ExecutionModel,
    pub(crate) order: usize,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlanEdge {
    pub(crate) from_node: String,
    pub(crate) from_port: String,
    pub(crate) to_node: String,
    pub(crate) to_port: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) metadata: Option<PlanEdgeMetadata>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlanEdgeMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resolved_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) merge_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) fan_out_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) order: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) feedback: Option<FeedbackPolicyCurrent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cycle_classification: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExecutionGroup {
    pub(crate) execution_model: ExecutionModel,
    pub(crate) node_ids: Vec<String>,
}

#[derive(Debug, Error)]
pub(crate) enum PlanError {
    #[error("{0}")]
    InvalidProject(#[from] ProjectValidationReport),
    #[error("cycle detected: {nodes}")]
    Cycle { nodes: String },
}

pub fn build_execution_plan(
    graph: &GraphDocument,
    registry: &NodeRegistry,
) -> Result<ExecutionPlan, PlanError> {
    validate_project(graph, registry)?;
    let ordered_node_ids = topological_order(graph)?;
    let order_by_node: HashMap<&str, usize> = ordered_node_ids
        .iter()
        .enumerate()
        .map(|(order, node_id)| (node_id.as_str(), order))
        .collect();

    let mut nodes = Vec::new();
    let mut groups_by_model: BTreeMap<String, ExecutionGroup> = BTreeMap::new();
    for node_id in &ordered_node_ids {
        let node = graph
            .nodes
            .iter()
            .find(|candidate| candidate.id == *node_id)
            .expect("topological order should only contain graph nodes");
        let definition = registry
            .get(&node.kind, &node.kind_version)
            .expect("project validation should resolve node definition");
        let order = order_by_node
            .get(node.id.as_str())
            .copied()
            .expect("node should have order");
        let execution_model = definition.execution.model.clone();

        nodes.push(PlanNode {
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

    Ok(ExecutionPlan {
        graph_id: graph.id.clone(),
        graph_revision: graph.revision.clone(),
        nodes,
        edges: graph.edges.iter().map(plan_edge).collect(),
        groups: groups_by_model.into_values().collect(),
    })
}

pub fn format_plan_text(plan: &ExecutionPlan) -> String {
    let mut output = format!(
        "valid project: {} revision {}\n\nnodes:\n",
        plan.graph_id, plan.graph_revision
    );

    for node in &plan.nodes {
        output.push_str(&format!(
            "  {} {} {}@{} model={}\n",
            node.order,
            node.node_id,
            node.kind,
            node.kind_version,
            execution_model_label(&node.execution_model)
        ));
    }

    output.push_str("\ngroups:\n");
    for group in &plan.groups {
        output.push_str(&format!(
            "  {}:\n",
            execution_model_label(&group.execution_model)
        ));
        for node_id in &group.node_ids {
            output.push_str(&format!("    {node_id}\n"));
        }
    }

    output
}

fn topological_order(graph: &GraphDocument) -> Result<Vec<String>, PlanError> {
    let mut indegree: HashMap<&str, usize> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), 0usize))
        .collect();
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();

    for edge in &graph.edges {
        if indegree.contains_key(edge.from.node.as_str())
            && indegree.contains_key(edge.to.node.as_str())
        {
            adjacency
                .entry(edge.from.node.as_str())
                .or_default()
                .push(edge.to.node.as_str());
            *indegree
                .get_mut(edge.to.node.as_str())
                .expect("target node exists") += 1;
        }
    }

    let mut queue = graph
        .nodes
        .iter()
        .filter(|node| indegree.get(node.id.as_str()).copied() == Some(0))
        .map(|node| node.id.as_str())
        .collect::<VecDeque<_>>();
    let mut ordered = Vec::new();

    while let Some(node_id) = queue.pop_front() {
        ordered.push(node_id.to_owned());
        for next in adjacency.get(node_id).into_iter().flatten().copied() {
            let next_indegree = indegree.get_mut(next).expect("adjacent node exists");
            *next_indegree -= 1;
            if *next_indegree == 0 {
                queue.push_back(next);
            }
        }
    }

    if ordered.len() == graph.nodes.len() {
        return Ok(ordered);
    }

    let cyclic_nodes = indegree
        .iter()
        .filter_map(|(node_id, count)| (*count > 0).then_some(*node_id))
        .collect::<Vec<_>>()
        .join(", ");
    Err(PlanError::Cycle {
        nodes: cyclic_nodes,
    })
}

fn plan_edge(edge: &Edge) -> PlanEdge {
    PlanEdge {
        from_node: edge.from.node.clone(),
        from_port: edge.from.port.clone(),
        to_node: edge.to.node.clone(),
        to_port: edge.to.port.clone(),
        metadata: None,
    }
}

fn execution_model_label(model: &ExecutionModel) -> &'static str {
    match model {
        ExecutionModel::Event => "event",
        ExecutionModel::Control => "control",
        ExecutionModel::Frame => "frame",
        ExecutionModel::AudioBlock => "audio_block",
        ExecutionModel::VideoFrame => "video_frame",
        ExecutionModel::GpuPass => "gpu_pass",
        ExecutionModel::AsyncResource => "async_resource",
        ExecutionModel::ScriptControl => "script_control",
        ExecutionModel::NativePlugin => "native_plugin",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{GraphDocument, NodeDefinition};

    fn registry() -> NodeRegistry {
        let mut registry = NodeRegistry::new();
        for definition in [
            json!({
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "object.core.float",
              "version": "0.1.0",
              "displayName": "Float",
              "category": "Core",
              "ports": [
                { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
              ],
              "execution": { "model": "control" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": []
            }),
            json!({
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "object.core.pass-float",
              "version": "0.1.0",
              "displayName": "Pass Float",
              "category": "Core",
              "ports": [
                { "id": "in", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.float32" }, "activation": "latched" },
                { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
              ],
              "execution": { "model": "control" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": []
            }),
        ] {
            registry
                .insert(serde_json::from_value::<NodeDefinition>(definition).unwrap())
                .unwrap();
        }
        registry
    }

    fn graph(value: serde_json::Value) -> GraphDocument {
        serde_json::from_value(value).unwrap()
    }

    fn graph_node(id: &str) -> serde_json::Value {
        json!({
          "id": id,
          "kind": "object.core.float",
          "kindVersion": "0.1.0",
          "params": {},
          "ports": [
            { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
          ]
        })
    }

    #[test]
    fn builds_plan_for_dag() {
        let graph = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "dag",
          "revision": "1",
          "nodes": [
            {
              "id": "value",
              "kind": "object.core.float",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
              ]
            },
            {
              "id": "pass",
              "kind": "object.core.pass-float",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "in", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.float32" }, "activation": "latched" },
                { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
              ]
            }
          ],
          "edges": [
            { "from": { "node": "value", "port": "out" }, "to": { "node": "pass", "port": "in" } }
          ]
        }));

        let plan = build_execution_plan(&graph, &registry()).unwrap();
        assert_eq!(plan.nodes[0].node_id, "value");
        assert_eq!(plan.nodes[1].node_id, "pass");
        assert_eq!(plan.groups[0].node_ids, vec!["value", "pass"]);
        assert_eq!(plan.edges[0].from_node, "value");
        assert!(format_plan_text(&plan).contains("model=control"));
    }

    #[test]
    fn rejects_invalid_project_before_planning() {
        let graph = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "missing-definition",
          "revision": "1",
          "nodes": [graph_node("value")],
          "edges": []
        }));

        let error = build_execution_plan(&graph, &NodeRegistry::new()).unwrap_err();

        assert!(matches!(error, PlanError::InvalidProject(_)));
    }

    #[test]
    fn rejects_cycle() {
        let graph = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "cycle",
          "revision": "1",
          "nodes": [
            {
              "id": "a",
              "kind": "object.core.pass-float",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "in", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.float32" }, "activation": "latched" },
                { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
              ]
            },
            {
              "id": "b",
              "kind": "object.core.pass-float",
              "kindVersion": "0.1.0",
              "params": {},
              "ports": [
                { "id": "in", "direction": "input", "type": { "flow": "control", "dataKind": "value.core.float32" }, "activation": "latched" },
                { "id": "out", "direction": "output", "type": { "flow": "control", "dataKind": "value.core.float32" } }
              ]
            }
          ],
          "edges": [
            { "from": { "node": "a", "port": "out" }, "to": { "node": "b", "port": "in" } },
            { "from": { "node": "b", "port": "out" }, "to": { "node": "a", "port": "in" } }
          ]
        }));

        let error = build_execution_plan(&graph, &registry()).unwrap_err();
        assert!(error.to_string().contains("cycle detected"));
    }

    #[test]
    fn topological_order_ignores_edges_with_missing_nodes() {
        let graph = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "dangling",
          "revision": "1",
          "nodes": [graph_node("value")],
          "edges": [
            { "from": { "node": "value", "port": "out" }, "to": { "node": "missing", "port": "in" } },
            { "from": { "node": "missing", "port": "out" }, "to": { "node": "value", "port": "out" } }
          ]
        }));

        assert_eq!(topological_order(&graph).unwrap(), vec!["value"]);
    }

    #[test]
    fn topological_order_reports_cycles() {
        let graph = graph(json!({
          "schema": "skenion.graph",
          "schemaVersion": "0.1.0",
          "id": "cycle",
          "revision": "1",
          "nodes": [graph_node("a"), graph_node("b")],
          "edges": [
            { "from": { "node": "a", "port": "out" }, "to": { "node": "b", "port": "out" } },
            { "from": { "node": "b", "port": "out" }, "to": { "node": "a", "port": "out" } }
          ]
        }));

        let error = topological_order(&graph).unwrap_err();

        assert!(matches!(error, PlanError::Cycle { .. }));
        assert!(error.to_string().contains("cycle detected"));
    }

    #[test]
    fn formats_all_execution_model_labels() {
        let models = [
            ExecutionModel::Event,
            ExecutionModel::Control,
            ExecutionModel::Frame,
            ExecutionModel::AudioBlock,
            ExecutionModel::VideoFrame,
            ExecutionModel::GpuPass,
            ExecutionModel::AsyncResource,
            ExecutionModel::ScriptControl,
            ExecutionModel::NativePlugin,
        ];
        let nodes = models
            .iter()
            .enumerate()
            .map(|(order, execution_model)| PlanNode {
                node_id: format!("node-{order}"),
                kind: "object.core.node".to_owned(),
                kind_version: "0.1.0".to_owned(),
                execution_model: execution_model.clone(),
                order,
            })
            .collect::<Vec<_>>();
        let groups = models
            .iter()
            .enumerate()
            .map(|(order, execution_model)| ExecutionGroup {
                execution_model: execution_model.clone(),
                node_ids: vec![format!("node-{order}")],
            })
            .collect();
        let plan = ExecutionPlan {
            graph_id: "models".to_owned(),
            graph_revision: "1".to_owned(),
            nodes,
            edges: Vec::new(),
            groups,
        };

        let text = format_plan_text(&plan);

        for label in [
            "event",
            "control",
            "frame",
            "audio_block",
            "video_frame",
            "gpu_pass",
            "async_resource",
            "script_control",
            "native_plugin",
        ] {
            assert!(text.contains(label));
        }
    }
}
