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
pub const PANEL_KIND: &str = "core.panel";
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlMessage {
    pub selector: String,
    #[serde(default)]
    pub atoms: Vec<ControlValue>,
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
            PANEL_KIND => Some(Self::String(
                node.params
                    .get("color")
                    .and_then(Value::as_str)
                    .unwrap_or("transparent")
                    .to_owned(),
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

impl ControlMessage {
    pub fn bang() -> Self {
        Self {
            selector: "bang".to_owned(),
            atoms: Vec::new(),
        }
    }

    pub fn from_value(value: ControlValue) -> Self {
        let selector = match value {
            ControlValue::F32(_) => "float",
            ControlValue::I32(_) => "int",
            ControlValue::Bool(_) => "bool",
            ControlValue::String(_) => "symbol",
            ControlValue::Rgba(_) => "rgba",
        }
        .to_owned();
        Self {
            selector,
            atoms: vec![value],
        }
    }

    pub fn parse_text(text: &str) -> Self {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Self {
                selector: "symbol".to_owned(),
                atoms: vec![ControlValue::String(String::new())],
            };
        }

        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let selector = parts.next().unwrap_or_default();
        let rest = parts.next().unwrap_or_default().trim();
        match selector {
            "bang" if rest.is_empty() => Self::bang(),
            "set" => Self {
                selector: "set".to_owned(),
                atoms: parse_message_atoms(rest),
            },
            "on" | "off" | "true" | "false" if rest.is_empty() => Self {
                selector: selector.to_owned(),
                atoms: Vec::new(),
            },
            _ if rest.is_empty() => {
                if let Some(value) = parse_scalar_atom(trimmed) {
                    Self::from_value(value)
                } else {
                    Self {
                        selector: "symbol".to_owned(),
                        atoms: vec![ControlValue::String(trimmed.to_owned())],
                    }
                }
            }
            _ => Self {
                selector: selector.to_owned(),
                atoms: parse_message_atoms(rest),
            },
        }
    }

    pub fn first_atom(&self) -> Option<&ControlValue> {
        self.atoms.first()
    }

    pub fn to_text(&self) -> String {
        if self.atoms.is_empty() {
            return self.selector.clone();
        }
        format!(
            "{} {}",
            self.selector,
            self.atoms
                .iter()
                .map(atom_to_text)
                .collect::<Vec<_>>()
                .join(" ")
        )
    }
}

fn parse_message_atoms(text: &str) -> Vec<ControlValue> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split_whitespace()
        .map(|token| {
            parse_scalar_atom(token).unwrap_or_else(|| ControlValue::String(token.to_owned()))
        })
        .collect()
}

fn parse_scalar_atom(token: &str) -> Option<ControlValue> {
    match token.to_ascii_lowercase().as_str() {
        "true" | "on" => Some(ControlValue::Bool(true)),
        "false" | "off" => Some(ControlValue::Bool(false)),
        _ => token
            .parse::<i64>()
            .map(ControlValue::I32)
            .or_else(|_| token.parse::<f64>().map(ControlValue::F32))
            .ok(),
    }
}

fn atom_to_text(atom: &ControlValue) -> String {
    match atom {
        ControlValue::F32(value) => value.to_string(),
        ControlValue::I32(value) => value.to_string(),
        ControlValue::Bool(value) => {
            if *value {
                "on".to_owned()
            } else {
                "off".to_owned()
            }
        }
        ControlValue::String(value) => value.clone(),
        ControlValue::Rgba(value) => {
            format!("rgba {} {} {} {}", value[0], value[1], value[2], value[3])
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
    }

    #[test]
    fn serializes_control_messages_with_selector_and_atoms() {
        assert_eq!(
            serde_json::to_value(ControlMessage::bang()).unwrap(),
            json!({ "selector": "bang", "atoms": [] })
        );
        assert_eq!(
            serde_json::to_value(ControlMessage::from_value(ControlValue::F32(0.5))).unwrap(),
            json!({ "selector": "float", "atoms": [{ "type": "f32", "value": 0.5 }] })
        );
        assert_eq!(
            ControlMessage::parse_text("set on"),
            ControlMessage {
                selector: "set".to_owned(),
                atoms: vec![ControlValue::Bool(true)]
            }
        );
    }

    #[test]
    fn parses_and_formats_control_message_text() {
        assert_eq!(
            ControlMessage::parse_text("   "),
            ControlMessage {
                selector: "symbol".to_owned(),
                atoms: vec![ControlValue::String(String::new())]
            }
        );
        assert_eq!(
            ControlMessage::parse_text("route 1 on label"),
            ControlMessage {
                selector: "route".to_owned(),
                atoms: vec![
                    ControlValue::I32(1),
                    ControlValue::Bool(true),
                    ControlValue::String("label".to_owned())
                ]
            }
        );
        assert_eq!(
            ControlMessage::parse_text("set"),
            ControlMessage {
                selector: "set".to_owned(),
                atoms: Vec::new()
            }
        );
        assert_eq!(ControlMessage::bang().to_text(), "bang");
        assert_eq!(
            ControlMessage::from_value(ControlValue::String("ready".to_owned())).to_text(),
            "symbol ready"
        );
        assert_eq!(
            ControlMessage::from_value(ControlValue::Rgba([1.0, 0.5, 0.25, 1.0])).to_text(),
            "rgba rgba 1 0.5 0.25 1"
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
            ControlValue::for_node_default(&node("core.comment", json!(null))),
            None
        );
        let mut panel = node(PANEL_KIND, json!(null));
        panel.params.insert("color".to_owned(), json!("#00ff00"));
        assert_eq!(
            ControlValue::for_node_default(&panel),
            Some(ControlValue::String("#00ff00".to_owned()))
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
        assert_eq!(
            ControlValue::for_node_default(&node("core.comment", json!(null))),
            None
        );
        assert_eq!(
            ControlValue::for_node_default(&node(PANEL_KIND, json!(null))),
            Some(ControlValue::String("transparent".to_owned()))
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
        assert_eq!(ControlMessage::bang().selector, "bang");
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
