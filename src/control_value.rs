use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::GraphNode;

pub const VALUE_F32_KIND: &str = "core.value-f32";
pub const VALUE_I32_KIND: &str = "core.value-i32";
pub const VALUE_BOOL_KIND: &str = "core.value-bool";
pub const COLOR_RGBA_KIND: &str = "core.color-rgba";
pub const STRING_KIND: &str = "core.string";
pub const TOGGLE_KIND: &str = "core.toggle";
pub const MESSAGE_KIND: &str = "core.message";
pub const SEND_F32_KIND: &str = "core.send-f32";
pub const SEND_I32_KIND: &str = "core.send-i32";
pub const SEND_BOOL_KIND: &str = "core.send-bool";
pub const SEND_RGBA_KIND: &str = "core.send-rgba";
pub const RECEIVE_F32_KIND: &str = "core.receive-f32";
pub const RECEIVE_I32_KIND: &str = "core.receive-i32";
pub const RECEIVE_BOOL_KIND: &str = "core.receive-bool";
pub const RECEIVE_RGBA_KIND: &str = "core.receive-rgba";
pub const UI_BUTTON_KIND: &str = "ui.button";
pub const UI_SLIDER_F32_KIND: &str = "ui.slider-f32";
pub const UI_TOGGLE_KIND: &str = "ui.toggle";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum ControlValue {
    F32(f64),
    I32(i64),
    Bool(bool),
    String(String),
    Rgba([f64; 4]),
    Bang,
}

impl ControlValue {
    pub fn for_node_default(node: &GraphNode) -> Option<Self> {
        match node.kind.as_str() {
            VALUE_F32_KIND => Some(Self::F32(read_f64_param(node).unwrap_or(0.0))),
            VALUE_I32_KIND => Some(Self::I32(read_i64_param(node).unwrap_or(0))),
            VALUE_BOOL_KIND => Some(Self::Bool(read_bool_param(node).unwrap_or(false))),
            COLOR_RGBA_KIND => Some(Self::Rgba(
                read_rgba_param(node).unwrap_or([1.0, 1.0, 1.0, 1.0]),
            )),
            STRING_KIND | MESSAGE_KIND => Some(Self::String(
                read_string_param(node).unwrap_or_default().to_owned(),
            )),
            TOGGLE_KIND => Some(Self::Bool(read_bool_param(node).unwrap_or(false))),
            UI_SLIDER_F32_KIND => Some(Self::F32(read_f64_param(node).unwrap_or(0.0))),
            UI_TOGGLE_KIND => Some(Self::Bool(read_bool_param(node).unwrap_or(false))),
            _ => None,
        }
    }

    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::F32(_) => "f32",
            Self::I32(_) => "i32",
            Self::Bool(_) => "bool",
            Self::String(_) => "string",
            Self::Rgba(_) => "rgba",
            Self::Bang => "bang",
        }
    }

    pub fn matches_stored_type(&self, stored: &Self) -> bool {
        matches!(
            (self, stored),
            (Self::F32(_), Self::F32(_))
                | (Self::I32(_), Self::I32(_))
                | (Self::Bool(_), Self::Bool(_))
                | (Self::String(_), Self::String(_))
                | (Self::Rgba(_), Self::Rgba(_))
        )
    }

    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Self::F32(value) => Some(*value as f32),
            _ => None,
        }
    }

    pub fn as_rgba_f32(&self) -> Option<[f32; 4]> {
        match self {
            Self::Rgba(value) => Some(value.map(|component| component.clamp(0.0, 1.0) as f32)),
            _ => None,
        }
    }
}

fn read_f64_param(node: &GraphNode) -> Option<f64> {
    node.params.get("value").and_then(Value::as_f64)
}

fn read_i64_param(node: &GraphNode) -> Option<i64> {
    node.params.get("value").and_then(Value::as_i64)
}

fn read_bool_param(node: &GraphNode) -> Option<bool> {
    node.params.get("value").and_then(Value::as_bool)
}

fn read_string_param(node: &GraphNode) -> Option<&str> {
    node.params.get("value").and_then(Value::as_str)
}

fn read_rgba_param(node: &GraphNode) -> Option<[f64; 4]> {
    let values = node.params.get("value")?.as_array()?;
    let [r, g, b, a] = values.as_slice() else {
        return None;
    };
    Some([r.as_f64()?, g.as_f64()?, b.as_f64()?, a.as_f64()?])
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, json};

    use super::*;

    #[test]
    fn serializes_control_values_with_canonical_type_labels() {
        assert_eq!(
            serde_json::to_value(ControlValue::F32(32.0)).unwrap(),
            json!({ "type": "f32", "value": 32.0 })
        );
        assert_eq!(
            serde_json::to_value(ControlValue::I32(32)).unwrap(),
            json!({ "type": "i32", "value": 32 })
        );
        assert_eq!(
            serde_json::to_value(ControlValue::Bool(true)).unwrap(),
            json!({ "type": "bool", "value": true })
        );
        assert_eq!(
            serde_json::to_value(ControlValue::String("ready".to_owned())).unwrap(),
            json!({ "type": "string", "value": "ready" })
        );
        assert_eq!(
            serde_json::to_value(ControlValue::Rgba([0.1, 0.2, 0.3, 1.0])).unwrap(),
            json!({ "type": "rgba", "value": [0.1, 0.2, 0.3, 1.0] })
        );
        assert_eq!(
            serde_json::to_value(ControlValue::Bang).unwrap(),
            json!({ "type": "bang" })
        );
    }

    #[test]
    fn derives_default_value_from_graph_node_params() {
        assert_eq!(
            ControlValue::for_node_default(&node(VALUE_F32_KIND, json!(1.5))),
            Some(ControlValue::F32(1.5))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(VALUE_I32_KIND, json!(7))),
            Some(ControlValue::I32(7))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(VALUE_BOOL_KIND, json!(true))),
            Some(ControlValue::Bool(true))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(COLOR_RGBA_KIND, json!([0.1, 0.2, 0.3, 1.0]))),
            Some(ControlValue::Rgba([0.1, 0.2, 0.3, 1.0]))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(STRING_KIND, json!("ready"))),
            Some(ControlValue::String("ready".to_owned()))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(TOGGLE_KIND, json!(true))),
            Some(ControlValue::Bool(true))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(MESSAGE_KIND, json!("perform"))),
            Some(ControlValue::String("perform".to_owned()))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(UI_SLIDER_F32_KIND, json!(0.75))),
            Some(ControlValue::F32(0.75))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(UI_TOGGLE_KIND, json!(true))),
            Some(ControlValue::Bool(true))
        );
    }

    #[test]
    fn invalid_or_missing_params_use_type_defaults() {
        assert_eq!(
            ControlValue::for_node_default(&node(VALUE_F32_KIND, json!("bad"))),
            Some(ControlValue::F32(0.0))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(VALUE_I32_KIND, json!(1.25))),
            Some(ControlValue::I32(0))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(VALUE_BOOL_KIND, json!("bad"))),
            Some(ControlValue::Bool(false))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(COLOR_RGBA_KIND, json!([0.1, 0.2]))),
            Some(ControlValue::Rgba([1.0, 1.0, 1.0, 1.0]))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(STRING_KIND, json!(false))),
            Some(ControlValue::String(String::new()))
        );
    }

    #[test]
    fn matches_only_stored_value_types() {
        assert!(ControlValue::F32(1.0).matches_stored_type(&ControlValue::F32(0.0)));
        assert!(ControlValue::I32(1).matches_stored_type(&ControlValue::I32(0)));
        assert!(ControlValue::Bool(true).matches_stored_type(&ControlValue::Bool(false)));
        assert!(
            ControlValue::String("a".to_owned())
                .matches_stored_type(&ControlValue::String("b".to_owned()))
        );
        assert!(
            ControlValue::Rgba([1.0, 0.0, 0.0, 1.0])
                .matches_stored_type(&ControlValue::Rgba([0.0, 0.0, 0.0, 1.0]))
        );
        assert!(!ControlValue::F32(1.0).matches_stored_type(&ControlValue::I32(1)));
        assert!(!ControlValue::Bang.matches_stored_type(&ControlValue::F32(1.0)));
    }

    #[test]
    fn reports_kind_labels_for_diagnostics() {
        assert_eq!(ControlValue::F32(1.0).kind_label(), "f32");
        assert_eq!(ControlValue::I32(1).kind_label(), "i32");
        assert_eq!(ControlValue::Bool(true).kind_label(), "bool");
        assert_eq!(ControlValue::String("x".to_owned()).kind_label(), "string");
        assert_eq!(
            ControlValue::Rgba([0.0, 0.0, 0.0, 1.0]).kind_label(),
            "rgba"
        );
        assert_eq!(ControlValue::Bang.kind_label(), "bang");
    }

    #[test]
    fn converts_shader_values() {
        assert_eq!(ControlValue::F32(1.25).as_f32(), Some(1.25));
        assert_eq!(
            ControlValue::Rgba([-1.0, 0.25, 2.0, 1.0]).as_rgba_f32(),
            Some([0.0, 0.25, 1.0, 1.0])
        );
        assert_eq!(ControlValue::Bool(false).as_f32(), None);
        assert_eq!(ControlValue::F32(0.0).as_rgba_f32(), None);
    }

    fn node(kind: &str, value: serde_json::Value) -> GraphNode {
        let mut params = Map::new();
        params.insert("value".to_owned(), value);
        GraphNode {
            id: "value_1".to_owned(),
            kind: kind.to_owned(),
            kind_version: "0.1.0".to_owned(),
            params,
            ports: Vec::new(),
        }
    }
}
