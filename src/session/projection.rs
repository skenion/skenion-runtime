use std::collections::{BTreeMap, HashSet};

use crate::{
    DataFlow, DataType, Edge, EdgeSpecCurrent, GraphDocument, GraphDocumentCurrent, GraphNode,
    GraphNodeCurrent, NodeDefinitionCurrent, PackageRegistryListResponseV01, Port, PortActivation,
    PortDirection, PortDirectionCurrent, PortRateCurrent, PortRef, PortSpecCurrent,
    ProjectDocumentCurrent, StringOrStrings, current_node_identity::graph_node_executable_kind,
    object_spec::ObjectRegistry,
};

pub(super) fn normalized_node_definitions_current(
    document: &ProjectDocumentCurrent,
    explicit_nodes: Vec<NodeDefinitionCurrent>,
    packages: Option<&PackageRegistryListResponseV01>,
) -> Vec<NodeDefinitionCurrent> {
    let mut nodes = explicit_nodes;
    let explicit_ids = nodes
        .iter()
        .map(|definition| definition.id.clone())
        .collect::<HashSet<_>>();
    let mut seen = nodes
        .iter()
        .map(node_definition_shape_key_current)
        .collect::<HashSet<_>>();

    for definition in ObjectRegistry::for_project_with_packages(Some(document), packages)
        .node_definition_projection()
    {
        if explicit_ids.contains(&definition.id) {
            continue;
        }
        let key = node_definition_shape_key_current(&definition);
        if seen.insert(key) {
            nodes.push(definition);
        }
    }

    nodes
}

fn node_definition_shape_key_current(definition: &NodeDefinitionCurrent) -> String {
    serde_json::to_string(&serde_json::json!({
        "id": definition.id,
        "ports": definition.ports,
        "execution": definition.execution,
    }))
    .expect("node definition shape key should serialize")
}

pub(super) fn lower_graph_for_execution(graph: &GraphDocumentCurrent) -> GraphDocument {
    let nodes = graph
        .nodes
        .iter()
        .filter_map(|node| lower_graph_node_for_execution(node, &node.id))
        .collect::<Vec<_>>();
    let executable_node_ids = nodes
        .iter()
        .map(|node| node.id.clone())
        .collect::<HashSet<_>>();
    GraphDocument {
        schema: "skenion.graph".to_owned(),
        schema_version: "0.1.0".to_owned(),
        id: graph.id.clone(),
        revision: graph.revision.clone(),
        nodes,
        edges: graph
            .edges
            .iter()
            .filter(|edge| {
                executable_node_ids.contains(&edge.source.node_id)
                    && executable_node_ids.contains(&edge.target.node_id)
            })
            .map(lower_edge_for_execution)
            .collect(),
    }
}

pub(crate) fn lower_graph_node_for_execution(
    node: &GraphNodeCurrent,
    pasted_id: &str,
) -> Option<GraphNode> {
    Some(GraphNode {
        id: pasted_id.to_owned(),
        kind: graph_node_executable_kind(node)?,
        kind_version: "0.1.0".to_owned(),
        params: node.params.clone(),
        ports: node.ports.iter().map(lower_port_for_execution).collect(),
    })
}

pub(super) fn lower_port_for_execution(port: &PortSpecCurrent) -> Port {
    Port {
        id: port.id.clone(),
        direction: match port.direction {
            PortDirectionCurrent::Input => PortDirection::Input,
            PortDirectionCurrent::Output => PortDirection::Output,
        },
        label: port.label.clone(),
        data_type: data_type_from_port_spec(port),
        required: port.required,
        default_value: port.default_value.clone(),
        activation: port.trigger_mode.as_ref().map(|trigger| match trigger {
            skenion_contracts::TriggerModeV01::Trigger => PortActivation::Trigger,
            skenion_contracts::TriggerModeV01::Latched => PortActivation::Latched,
            skenion_contracts::TriggerModeV01::Passive => PortActivation::Latched,
        }),
    }
}

fn data_type_from_port_spec(port: &PortSpecCurrent) -> DataType {
    let (canonical_flow, data_kind) = current_port_type_parts(&port.port_type);
    let format = match data_kind.as_str() {
        "value.core.float32" => Some(StringOrStrings::One("f32".to_owned())),
        "value.core.tensor" => Some(StringOrStrings::One("rgba8unorm".to_owned())),
        _ => None,
    };
    let color_space = (data_kind == "value.core.tensor").then(|| "srgb".to_owned());
    DataType {
        flow: canonical_flow.unwrap_or_else(|| match port.rate {
            Some(PortRateCurrent::Event) => DataFlow::Event,
            Some(PortRateCurrent::Audio) => DataFlow::Signal,
            Some(PortRateCurrent::Resource) | Some(PortRateCurrent::Io) => DataFlow::Resource,
            Some(PortRateCurrent::Control | PortRateCurrent::Render | PortRateCurrent::Gpu)
            | None => {
                if data_kind == "value.core.tensor" {
                    DataFlow::Resource
                } else {
                    DataFlow::Control
                }
            }
        }),
        data_kind,
        unit: None,
        range: None,
        shape: None,
        channels: None,
        sample_rate: None,
        format,
        color_space,
        frame_rate: None,
        alpha_policy: None,
        values: None,
    }
}

fn current_port_type_parts(port_type: &str) -> (Option<DataFlow>, String) {
    match port_type {
        value_type if value_type.starts_with("value.") => (None, value_type.to_owned()),
        other => (None, other.to_owned()),
    }
}

pub(super) fn remap_edge(edge: &EdgeSpecCurrent, node_id_map: &BTreeMap<String, String>) -> Edge {
    Edge {
        from: PortRef {
            node: node_id_map
                .get(&edge.source.node_id)
                .cloned()
                .unwrap_or_else(|| edge.source.node_id.clone()),
            port: edge.source.port_id.clone(),
        },
        to: PortRef {
            node: node_id_map
                .get(&edge.target.node_id)
                .cloned()
                .unwrap_or_else(|| edge.target.node_id.clone()),
            port: edge.target.port_id.clone(),
        },
    }
}

pub(crate) fn lower_edge_for_execution(edge: &EdgeSpecCurrent) -> Edge {
    remap_edge(edge, &BTreeMap::new())
}
