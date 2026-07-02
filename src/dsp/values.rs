use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::AudioDspPlanNode;

pub(super) fn control_input_f32_from_params(
    node: &AudioDspPlanNode,
    port_id: &str,
    node_params_by_id: &BTreeMap<String, Map<String, Value>>,
) -> Option<f32> {
    for input in &node.control_inputs {
        if input.port_id != port_id {
            continue;
        }
        let node_id = input.source_node_id.as_deref()?;
        let params = node_params_by_id.get(node_id)?;
        return Some(param_f32(params, "value", 0.0));
    }
    None
}

pub(super) fn param_f32(params: &Map<String, Value>, key: &str, default: f32) -> f32 {
    params
        .get(key)
        .and_then(Value::as_f64)
        .map(|value| value as f32)
        .unwrap_or(default)
}
