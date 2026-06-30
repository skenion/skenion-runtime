#[cfg(test)]
use serde_json::{Map, Value, json};
use skenion_contracts::MessageKeyPolicyV01;

#[cfg(test)]
use super::ObjectSpecCandidateSummary;
use super::ports::message_key_policy;
use super::{
    CURRENT_KIND_VERSION, ObjectSpecDiagnostic, ObjectSpecPort, ObjectSpecPortActivation,
    ObjectSpecPortDirection, ObjectSpecPortRate, ObjectSpecResolution,
};
use crate::{
    GraphNodeCurrent, NodeDefinitionCurrent, PortDirectionCurrent, PortRateCurrent,
    PortSpecCurrent, current_node_identity::implementation_executable_kind,
};

pub(crate) fn materialize_object_spec_node_v01(
    resolution: &ObjectSpecResolution,
    node_id: impl Into<String>,
) -> Result<GraphNodeCurrent, ObjectSpecDiagnostic> {
    let Some(implementation) = resolution.implementation.clone() else {
        return Err(primary_resolution_diagnostic(resolution));
    };

    Ok(GraphNodeCurrent {
        id: node_id.into(),
        implementation: Some(implementation),
        object_spec: Some(resolution.display_text.clone()),
        object_resolution: Some(resolution.object_resolution.clone()),
        binding_ref: None,
        params: resolution.params.clone(),
        ports: resolution
            .instance_ports
            .iter()
            .map(object_spec_port_to_current)
            .collect(),
        port_groups: None,
    })
}

pub(crate) fn object_spec_node_definition_v01(
    resolution: &ObjectSpecResolution,
) -> Option<NodeDefinitionCurrent> {
    let implementation = resolution.implementation.as_ref()?;
    let version = implementation
        .version
        .clone()
        .unwrap_or_else(|| CURRENT_KIND_VERSION.to_owned());
    let ports = resolution
        .instance_ports
        .iter()
        .map(object_spec_port_to_current)
        .collect::<Vec<_>>();
    let has_audio_port = ports
        .iter()
        .any(|port| port.rate == Some(PortRateCurrent::Audio));

    Some(NodeDefinitionCurrent {
        schema: "skenion.node.definition".to_owned(),
        schema_version: CURRENT_KIND_VERSION.to_owned(),
        id: implementation_executable_kind(implementation),
        version,
        display_name: object_spec_definition_display_name(&implementation.object_id),
        category: object_spec_definition_category(&implementation.object_id).to_owned(),
        script_api_version: None,
        bundle_hash: None,
        surface: None,
        ports,
        port_groups: None,
        execution: skenion_contracts::NodeExecutionV01 {
            model: if has_audio_port {
                skenion_contracts::ExecutionModelV01::AudioBlock
            } else {
                skenion_contracts::ExecutionModelV01::Control
            },
            clock: None,
        },
        state: skenion_contracts::NodeStateV01 { persistent: false },
        permissions: Vec::new(),
        capabilities: Vec::new(),
    })
}

#[cfg(test)]
pub(crate) fn materialize_unresolved_object_spec_node_v01(
    resolution: &ObjectSpecResolution,
    node_id: impl Into<String>,
) -> GraphNodeCurrent {
    let diagnostic = primary_resolution_diagnostic(resolution);
    let mut params = Map::new();
    params.insert(
        "objectSpec".to_owned(),
        Value::String(resolution.display_text.clone()),
    );
    params.insert(
        "requestedKind".to_owned(),
        Value::String(resolution.class_symbol.clone()),
    );
    params.insert("diagnosticCode".to_owned(), Value::String(diagnostic.code));
    params.insert(
        "diagnosticMessage".to_owned(),
        Value::String(diagnostic.message),
    );
    params.insert(
        "candidateCount".to_owned(),
        json!(resolution.candidates.len()),
    );
    if !resolution.candidates.is_empty() {
        params.insert(
            "candidates".to_owned(),
            Value::Array(
                resolution
                    .candidates
                    .iter()
                    .map(object_spec_candidate_json)
                    .collect(),
            ),
        );
    }

    GraphNodeCurrent {
        id: node_id.into(),
        implementation: None,
        object_spec: Some(resolution.display_text.clone()),
        object_resolution: Some(resolution.object_resolution.clone()),
        binding_ref: None,
        params,
        ports: Vec::new(),
        port_groups: None,
    }
}

#[cfg(test)]
fn object_spec_candidate_json(candidate: &ObjectSpecCandidateSummary) -> Value {
    json!({
        "id": candidate.id,
        "source": candidate.source,
        "implementation": candidate.implementation,
        "objectSpec": candidate.object_spec,
        "displayName": candidate.display_name,
    })
}

pub(crate) fn unresolved_object_spec_node_definition_v01() -> NodeDefinitionCurrent {
    NodeDefinitionCurrent {
        schema: "skenion.node.definition".to_owned(),
        schema_version: CURRENT_KIND_VERSION.to_owned(),
        id: "object.core.unresolved".to_owned(),
        version: CURRENT_KIND_VERSION.to_owned(),
        display_name: "Unresolved Object".to_owned(),
        category: "Diagnostics".to_owned(),
        script_api_version: None,
        bundle_hash: None,
        surface: None,
        ports: Vec::new(),
        port_groups: None,
        execution: skenion_contracts::NodeExecutionV01 {
            model: skenion_contracts::ExecutionModelV01::Event,
            clock: None,
        },
        state: skenion_contracts::NodeStateV01 { persistent: false },
        permissions: Vec::new(),
        capabilities: vec!["diagnostic.unresolved-object.v0.1".to_owned()],
    }
}

fn primary_resolution_diagnostic(resolution: &ObjectSpecResolution) -> ObjectSpecDiagnostic {
    resolution
        .diagnostics
        .first()
        .cloned()
        .unwrap_or_else(|| ObjectSpecDiagnostic {
            code: "object-spec.unresolved".to_owned(),
            message: format!(
                "{} is not available in the local Runtime object resolver",
                resolution.class_symbol
            ),
        })
}

pub(super) fn object_spec_port_to_current(port: &ObjectSpecPort) -> PortSpecCurrent {
    PortSpecCurrent {
        id: port.id.clone(),
        direction: match &port.direction {
            ObjectSpecPortDirection::Input => PortDirectionCurrent::Input,
            ObjectSpecPortDirection::Output => PortDirectionCurrent::Output,
        },
        port_type: port.port_type.clone(),
        label: port.label.clone(),
        rate: Some(match &port.rate {
            ObjectSpecPortRate::Event => PortRateCurrent::Event,
            ObjectSpecPortRate::Control => PortRateCurrent::Control,
            ObjectSpecPortRate::Audio => PortRateCurrent::Audio,
            ObjectSpecPortRate::Render => PortRateCurrent::Render,
            ObjectSpecPortRate::Gpu => PortRateCurrent::Gpu,
            ObjectSpecPortRate::Resource => PortRateCurrent::Resource,
            ObjectSpecPortRate::Io => PortRateCurrent::Io,
        }),
        accepts: port.accepts.clone().or_else(|| message_input_accepts(port)),
        min_connections: None,
        max_connections: None,
        merge_policy: None,
        fan_out_policy: None,
        trigger_mode: port.activation.as_ref().map(|activation| match activation {
            ObjectSpecPortActivation::Trigger => skenion_contracts::TriggerModeV01::Trigger,
            ObjectSpecPortActivation::Latched => skenion_contracts::TriggerModeV01::Latched,
            ObjectSpecPortActivation::Passive => skenion_contracts::TriggerModeV01::Passive,
        }),
        message_keys: port
            .message_keys
            .clone()
            .or_else(|| default_message_input_key_policy(port)),
        default_value: None,
        latch: None,
        required: matches!(&port.direction, ObjectSpecPortDirection::Input).then_some(false),
        style_key: None,
        group: None,
        description: None,
    }
}

fn message_input_accepts(port: &ObjectSpecPort) -> Option<Vec<String>> {
    if matches!(&port.direction, ObjectSpecPortDirection::Input)
        && port.port_type == "value.core.message"
    {
        return Some(
            [
                "value.core.float32",
                "value.core.int32",
                "value.core.uint32",
                "value.core.bool",
                "value.core.bang",
                "value.core.message",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        );
    }
    None
}

fn default_message_input_key_policy(port: &ObjectSpecPort) -> Option<MessageKeyPolicyV01> {
    if matches!(&port.direction, ObjectSpecPortDirection::Input)
        && port.port_type == "value.core.message"
    {
        return Some(message_key_policy(
            &["bang", "set", "float", "int", "uint", "bool", "message"],
            &["set"],
            &["bang", "float", "int", "uint", "bool", "message"],
            &["set", "float", "int", "uint", "bool", "message"],
            &["bang", "float", "int", "uint", "bool", "message"],
        ));
    }
    None
}

fn object_spec_definition_display_name(kind: &str) -> String {
    kind.rsplit('.')
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(kind)
        .replace('-', " ")
}

fn object_spec_definition_category(kind: &str) -> &'static str {
    if kind.starts_with("object.core.audio.") || kind.starts_with("audio.") {
        "Runtime Audio"
    } else {
        "Runtime Objects"
    }
}
