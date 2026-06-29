use std::collections::{BTreeMap, HashSet};

use crate::{
    DataFlow, DataType, Edge, EdgeSpecCurrent, GraphDocument, GraphDocumentCurrent, GraphNode,
    GraphNodeCurrent, NodeDefinition, NodeDefinitionCurrent, Port, PortActivation, PortDirection,
    PortDirectionCurrent, PortRateCurrent, PortRef, PortSpecCurrent, ProjectDocumentCurrent,
    StringOrStrings, object_text::ObjectRegistry,
};

pub(super) fn normalized_node_definitions_current(
    document: &ProjectDocumentCurrent,
    explicit_nodes: Vec<NodeDefinitionCurrent>,
) -> Vec<NodeDefinitionCurrent> {
    let mut nodes = explicit_nodes;
    let mut seen = nodes
        .iter()
        .map(|definition| (definition.id.clone(), definition.version.clone()))
        .collect::<HashSet<_>>();

    for definition in ObjectRegistry::for_project(Some(document)).node_definition_projection() {
        let key = (definition.id.clone(), definition.version.clone());
        if seen.insert(key) {
            nodes.push(definition);
        }
    }

    nodes
}

pub(super) fn lower_graph_for_execution(graph: &GraphDocumentCurrent) -> GraphDocument {
    GraphDocument {
        schema: "skenion.graph".to_owned(),
        schema_version: "0.1.0".to_owned(),
        id: graph.id.clone(),
        revision: graph.revision.clone(),
        nodes: graph
            .nodes
            .iter()
            .map(|node| lower_graph_node_for_execution(node, &node.id))
            .collect(),
        edges: graph.edges.iter().map(lower_edge_for_execution).collect(),
    }
}

pub(super) fn lower_node_definition_for_execution(
    definition: &NodeDefinitionCurrent,
) -> NodeDefinition {
    NodeDefinition {
        schema: "skenion.node.definition".to_owned(),
        schema_version: "0.1.0".to_owned(),
        id: definition.id.clone(),
        version: definition.version.clone(),
        display_name: definition.display_name.clone(),
        category: definition.category.clone(),
        script_api_version: definition.script_api_version.clone(),
        bundle_hash: definition.bundle_hash.clone(),
        surface: definition
            .surface
            .as_ref()
            .map(|surface| skenion_contracts::NodeSurfaceV01 {
                palette: surface.palette.clone(),
            }),
        ports: definition
            .ports
            .iter()
            .map(lower_port_for_execution)
            .collect(),
        execution: skenion_contracts::NodeExecutionV01 {
            model: lower_execution_model_for_execution(&definition.execution.model),
            clock: definition.execution.clock.clone(),
        },
        state: skenion_contracts::NodeStateV01 {
            persistent: definition.state.persistent,
        },
        permissions: definition.permissions.clone(),
        capabilities: definition.capabilities.clone(),
    }
}

pub(super) fn lower_execution_model_for_execution(
    model: &skenion_contracts::ExecutionModelV01,
) -> crate::ExecutionModel {
    match model {
        skenion_contracts::ExecutionModelV01::Event => crate::ExecutionModel::Event,
        skenion_contracts::ExecutionModelV01::Control => crate::ExecutionModel::Control,
        skenion_contracts::ExecutionModelV01::Frame => crate::ExecutionModel::Frame,
        skenion_contracts::ExecutionModelV01::AudioBlock => crate::ExecutionModel::AudioBlock,
        skenion_contracts::ExecutionModelV01::VideoFrame => crate::ExecutionModel::VideoFrame,
        skenion_contracts::ExecutionModelV01::GpuPass => crate::ExecutionModel::GpuPass,
        skenion_contracts::ExecutionModelV01::AsyncResource => crate::ExecutionModel::AsyncResource,
        skenion_contracts::ExecutionModelV01::ScriptControl => crate::ExecutionModel::ScriptControl,
        skenion_contracts::ExecutionModelV01::NativePlugin => crate::ExecutionModel::NativePlugin,
    }
}

pub(crate) fn lower_graph_node_for_execution(
    node: &GraphNodeCurrent,
    pasted_id: &str,
) -> GraphNode {
    GraphNode {
        id: pasted_id.to_owned(),
        kind: node.kind.clone(),
        kind_version: node.kind_version.clone(),
        params: node.params.clone(),
        ports: node.ports.iter().map(lower_port_for_execution).collect(),
    }
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
