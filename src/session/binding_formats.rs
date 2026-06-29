use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    EdgeSpecCurrent, EndpointBindingValueFormat, GraphDocumentCurrent, PortSpecCurrent,
    ProjectDocumentCurrent, ValueEndpointRef, ValueFormat,
};

pub(super) fn derive_runtime_binding_formats(
    project: Option<&ProjectDocumentCurrent>,
) -> Vec<EndpointBindingValueFormat> {
    let Some(project) = project else {
        return Vec::new();
    };

    let graph = &project.graph;
    let format_revision = runtime_binding_format_revision(graph.revision.as_str());
    let mut binding_formats = Vec::new();

    for edge in &graph.edges {
        let Some(value_format) = value_format_for_edge(graph, edge) else {
            continue;
        };
        let binding_format = EndpointBindingValueFormat {
            binding_id: edge.id.clone(),
            binding_epoch: 1,
            format_revision,
            format_digest: Some(sha256_hex_for_json(&value_format)),
            value_format,
            source: Some(ValueEndpointRef {
                node_id: edge.source.node_id.clone(),
                port_id: edge.source.port_id.clone(),
            }),
            target: Some(ValueEndpointRef {
                node_id: edge.target.node_id.clone(),
                port_id: edge.target.port_id.clone(),
            }),
            delivery: None,
        };
        if skenion_contracts::validate_endpoint_binding_value_format_v01(&binding_format).is_ok() {
            binding_formats.push(binding_format);
        }
    }

    binding_formats.sort_by(|left, right| left.binding_id.cmp(&right.binding_id));
    binding_formats
}

fn value_format_for_edge(
    graph: &GraphDocumentCurrent,
    edge: &EdgeSpecCurrent,
) -> Option<ValueFormat> {
    let port_type = edge.resolved_type.as_deref().or_else(|| {
        find_graph_port(graph, &edge.source.node_id, &edge.source.port_id)
            .map(|port| port.port_type.as_str())
    })?;
    value_format_for_port_type(port_type)
}

fn find_graph_port<'a>(
    graph: &'a GraphDocumentCurrent,
    node_id: &str,
    port_id: &str,
) -> Option<&'a PortSpecCurrent> {
    graph
        .nodes
        .iter()
        .find(|node| node.id == node_id)?
        .ports
        .iter()
        .find(|port| port.id == port_id)
}

pub(super) fn value_format_for_port_type(port_type: &str) -> Option<ValueFormat> {
    let value_type_id = runtime_value_type_id_for_port_type(port_type)?;
    let value_format = ValueFormat {
        format: runtime_value_format_label(value_type_id.as_str()).map(str::to_owned),
        value_type_id,
        shape: None,
        dynamic_shape: None,
        layout: None,
        strides: None,
        byte_length: None,
        sample_rate: None,
        channels: None,
        channel_layout: None,
        color_space: None,
        color_range: None,
        transfer: None,
        primaries: None,
        alpha_policy: None,
        resource_kind: None,
    };

    if skenion_contracts::validate_value_format_v01(&value_format).is_ok() {
        Some(value_format)
    } else {
        None
    }
}

fn runtime_value_type_id_for_port_type(port_type: &str) -> Option<String> {
    match port_type {
        "value.core.message" => Some("value.core.message".to_owned()),
        "value.core.float32" => Some("value.core.float32".to_owned()),
        "value.core.int32" => Some("value.core.int32".to_owned()),
        "value.core.uint32" => Some("value.core.uint32".to_owned()),
        "value.core.bool" => Some("value.core.bool".to_owned()),
        "value.core.string" => Some("value.core.string".to_owned()),
        "value.core.color" => Some("value.core.color".to_owned()),
        "value.core.bang" => Some("value.core.bang".to_owned()),
        value_type if value_type.starts_with("value.") => Some(value_type.to_owned()),
        _ => None,
    }
}

pub(super) fn runtime_value_format_label(value_type_id: &str) -> Option<&'static str> {
    match value_type_id {
        "value.core.float16" => Some("f16"),
        "value.core.float32" => Some("f32"),
        "value.core.float64" => Some("f64"),
        "value.core.ufloat8" => Some("ufloat8"),
        "value.core.ufloat16" => Some("ufloat16"),
        "value.core.ufloat32" => Some("ufloat32"),
        "value.core.ufloat64" => Some("ufloat64"),
        "value.core.int8" => Some("i8"),
        "value.core.int16" => Some("i16"),
        "value.core.int32" => Some("i32"),
        "value.core.int64" => Some("i64"),
        "value.core.uint8" => Some("u8"),
        "value.core.uint16" => Some("u16"),
        "value.core.uint32" => Some("u32"),
        "value.core.uint64" => Some("u64"),
        "value.core.color" => Some("rgba32f"),
        _ => None,
    }
}

pub(super) fn runtime_binding_format_revision(graph_revision: &str) -> u64 {
    graph_revision
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(1)
}

fn sha256_hex_for_json<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).expect("runtime binding value format should serialize");
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}
