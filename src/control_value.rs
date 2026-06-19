use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::GraphNode;

pub const FLOAT_KIND: &str = "core.float";
pub const INT_KIND: &str = "core.int";
pub const UINT_KIND: &str = "core.uint";
pub const BOOL_KIND: &str = "core.bool";
pub const COLOR_KIND: &str = "core.color";
pub const STRING_KIND: &str = "core.string";
pub const BANG_KIND: &str = "core.bang";
pub const MESSAGE_KIND: &str = "core.message";
pub const PANEL_KIND: &str = "core.panel";

pub const DEFAULT_FLOAT_REPRESENTATION: &str = "f32";
pub const DEFAULT_INT_REPRESENTATION: &str = "i32";
pub const DEFAULT_UINT_REPRESENTATION: &str = "u32";
pub const DEFAULT_COLOR_REPRESENTATION: &str = "rgba32f";
pub const DEFAULT_COLOR_SPACE: &str = "linear";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ControlValue {
    #[serde(rename = "float")]
    Float { representation: String, value: f64 },
    #[serde(rename = "int")]
    Int { representation: String, value: i64 },
    #[serde(rename = "uint")]
    Uint { representation: String, value: u64 },
    #[serde(rename = "bool")]
    Bool { value: bool },
    #[serde(rename = "string")]
    String { value: String },
    #[serde(rename = "color")]
    Color {
        representation: String,
        #[serde(rename = "colorSpace", default = "default_color_space")]
        color_space: String,
        value: [f64; 4],
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlMessage {
    pub selector: String,
    #[serde(default)]
    pub atoms: Vec<ControlValue>,
}

impl ControlValue {
    pub fn float(value: f64) -> Self {
        Self::Float {
            representation: DEFAULT_FLOAT_REPRESENTATION.to_owned(),
            value,
        }
    }

    pub fn int(value: i64) -> Self {
        Self::Int {
            representation: DEFAULT_INT_REPRESENTATION.to_owned(),
            value,
        }
    }

    pub fn uint(value: u64) -> Self {
        Self::Uint {
            representation: DEFAULT_UINT_REPRESENTATION.to_owned(),
            value,
        }
    }

    pub fn bool(value: bool) -> Self {
        Self::Bool { value }
    }

    pub fn string(value: impl Into<String>) -> Self {
        Self::String {
            value: value.into(),
        }
    }

    pub fn color(value: [f64; 4]) -> Self {
        Self::Color {
            representation: DEFAULT_COLOR_REPRESENTATION.to_owned(),
            color_space: DEFAULT_COLOR_SPACE.to_owned(),
            value,
        }
    }

    pub fn for_node_default(node: &GraphNode) -> Option<Self> {
        match node.kind.as_str() {
            FLOAT_KIND => Some(Self::Float {
                representation: read_representation_param(node, DEFAULT_FLOAT_REPRESENTATION),
                value: read_f64_param(node).unwrap_or(0.0),
            }),
            INT_KIND => Some(Self::Int {
                representation: read_representation_param(node, DEFAULT_INT_REPRESENTATION),
                value: read_i64_param(node).unwrap_or(0),
            }),
            UINT_KIND => Some(Self::Uint {
                representation: read_representation_param(node, DEFAULT_UINT_REPRESENTATION),
                value: read_u64_param(node).unwrap_or(0),
            }),
            BOOL_KIND => Some(Self::bool(read_bool_param(node).unwrap_or(false))),
            COLOR_KIND => Some(Self::Color {
                representation: read_representation_param(node, DEFAULT_COLOR_REPRESENTATION),
                color_space: read_color_space_param(node),
                value: read_rgba_param(node).unwrap_or([1.0, 1.0, 1.0, 1.0]),
            }),
            STRING_KIND | MESSAGE_KIND => {
                Some(Self::string(read_string_param(node).unwrap_or_default()))
            }
            PANEL_KIND => Some(Self::string(
                node.params
                    .get("color")
                    .and_then(Value::as_str)
                    .unwrap_or("transparent"),
            )),
            _ => None,
        }
    }

    pub fn kind_label(&self) -> String {
        match self {
            Self::Float { representation, .. } => format!("number.float/{representation}"),
            Self::Int { representation, .. } => format!("number.int/{representation}"),
            Self::Uint { representation, .. } => format!("number.uint/{representation}"),
            Self::Bool { .. } => "boolean".to_owned(),
            Self::String { .. } => "string".to_owned(),
            Self::Color { representation, .. } => format!("color/{representation}"),
        }
    }

    pub fn matches_stored_type(&self, stored: &Self) -> bool {
        matches!(
            (self, stored),
            (Self::Float { .. }, Self::Float { .. })
                | (Self::Int { .. }, Self::Int { .. })
                | (Self::Uint { .. }, Self::Uint { .. })
                | (Self::Bool { .. }, Self::Bool { .. })
                | (Self::String { .. }, Self::String { .. })
                | (Self::Color { .. }, Self::Color { .. })
        )
    }

    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Self::Float { value, .. } => Some(sanitize_f64(*value) as f32),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Self::Int { value, .. } => {
                Some((*value).clamp(i32::MIN as i64, i32::MAX as i64) as i32)
            }
            _ => None,
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        match self {
            Self::Uint { value, .. } => Some((*value).min(u32::MAX as u64) as u32),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool { value } => Some(*value),
            _ => None,
        }
    }

    pub fn as_rgba_f32(&self) -> Option<[f32; 4]> {
        match self {
            Self::Color { value, .. } => {
                Some(value.map(|component| component.clamp(0.0, 1.0) as f32))
            }
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
            ControlValue::Float { .. } => "float",
            ControlValue::Int { .. } => "int",
            ControlValue::Uint { .. } => "uint",
            ControlValue::Bool { .. } => "bool",
            ControlValue::String { .. } => "symbol",
            ControlValue::Color { .. } => "color",
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
                atoms: vec![ControlValue::string(String::new())],
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
            "float" => typed_or_generic_message(selector, rest, parse_float_atom),
            "int" => typed_or_generic_message(selector, rest, parse_int_atom),
            "uint" => typed_or_generic_message(selector, rest, parse_uint_atom),
            "bool" => typed_or_generic_message(selector, rest, parse_bool_atom),
            "symbol" => Self {
                selector: "symbol".to_owned(),
                atoms: vec![ControlValue::string(rest.to_owned())],
            },
            "color" => typed_or_generic_message(selector, rest, parse_color_atom),
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
                        atoms: vec![ControlValue::string(trimmed.to_owned())],
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
        if self.atoms.len() == 1
            && let Some(payload) = typed_atom_payload_to_text(&self.selector, &self.atoms[0])
        {
            return format!("{} {}", self.selector, payload);
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
    let tokens = text.split_whitespace().collect::<Vec<_>>();
    let mut atoms = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        let token = tokens[index];
        if token == "color"
            && index + 4 < tokens.len()
            && let Some(color) = parse_color_atom(&tokens[index + 1..index + 5].join(" "))
        {
            atoms.push(color);
            index += 5;
            continue;
        }
        atoms.push(
            parse_scalar_atom(token).unwrap_or_else(|| ControlValue::string(token.to_owned())),
        );
        index += 1;
    }
    atoms
}

fn parse_scalar_atom(token: &str) -> Option<ControlValue> {
    match token.to_ascii_lowercase().as_str() {
        "true" | "on" => Some(ControlValue::bool(true)),
        "false" | "off" => Some(ControlValue::bool(false)),
        _ => token
            .parse::<i64>()
            .map(ControlValue::int)
            .or_else(|_| token.parse::<f64>().map(ControlValue::float))
            .ok(),
    }
}

fn typed_or_generic_message(
    selector: &str,
    rest: &str,
    parser: fn(&str) -> Option<ControlValue>,
) -> ControlMessage {
    parser(rest)
        .map(ControlMessage::from_value)
        .unwrap_or_else(|| ControlMessage {
            selector: selector.to_owned(),
            atoms: parse_message_atoms(rest),
        })
}

fn parse_float_atom(text: &str) -> Option<ControlValue> {
    let mut tokens = text.split_whitespace();
    let token = tokens.next()?;
    tokens.next().is_none().then_some(())?;
    token.parse::<f64>().ok().map(ControlValue::float)
}

fn parse_int_atom(text: &str) -> Option<ControlValue> {
    let mut tokens = text.split_whitespace();
    let token = tokens.next()?;
    tokens.next().is_none().then_some(())?;
    token.parse::<i64>().ok().map(ControlValue::int)
}

fn parse_uint_atom(text: &str) -> Option<ControlValue> {
    let mut tokens = text.split_whitespace();
    let token = tokens.next()?;
    tokens.next().is_none().then_some(())?;
    token.parse::<u64>().ok().map(ControlValue::uint)
}

fn parse_bool_atom(text: &str) -> Option<ControlValue> {
    let mut tokens = text.split_whitespace();
    let token = tokens.next()?.to_ascii_lowercase();
    tokens.next().is_none().then_some(())?;
    match token.as_str() {
        "1" | "on" | "true" => Some(ControlValue::bool(true)),
        "0" | "off" | "false" => Some(ControlValue::bool(false)),
        _ => None,
    }
}

fn parse_color_atom(text: &str) -> Option<ControlValue> {
    let tokens = text.split_whitespace().collect::<Vec<_>>();
    let [r, g, b, a] = tokens.as_slice() else {
        return None;
    };
    Some(ControlValue::color([
        r.parse::<f64>().ok()?,
        g.parse::<f64>().ok()?,
        b.parse::<f64>().ok()?,
        a.parse::<f64>().ok()?,
    ]))
}

fn typed_atom_payload_to_text(selector: &str, atom: &ControlValue) -> Option<String> {
    match (selector, atom) {
        ("float", ControlValue::Float { value, .. }) => Some(value.to_string()),
        ("int", ControlValue::Int { value, .. }) => Some(value.to_string()),
        ("uint", ControlValue::Uint { value, .. }) => Some(value.to_string()),
        ("bool", ControlValue::Bool { value }) => {
            Some(if *value { "on" } else { "off" }.to_owned())
        }
        ("symbol", ControlValue::String { value }) => Some(value.clone()),
        ("color", ControlValue::Color { value, .. }) => Some(format!(
            "{} {} {} {}",
            value[0], value[1], value[2], value[3]
        )),
        _ => None,
    }
}

fn atom_to_text(atom: &ControlValue) -> String {
    match atom {
        ControlValue::Float { value, .. } => value.to_string(),
        ControlValue::Int { value, .. } => value.to_string(),
        ControlValue::Uint { value, .. } => value.to_string(),
        ControlValue::Bool { value } => {
            if *value {
                "on".to_owned()
            } else {
                "off".to_owned()
            }
        }
        ControlValue::String { value } => value.clone(),
        ControlValue::Color { value, .. } => {
            format!("color {} {} {} {}", value[0], value[1], value[2], value[3])
        }
    }
}

fn read_f64_param(node: &GraphNode) -> Option<f64> {
    node.params.get("value").and_then(Value::as_f64)
}

fn read_i64_param(node: &GraphNode) -> Option<i64> {
    node.params.get("value").and_then(Value::as_i64)
}

fn read_u64_param(node: &GraphNode) -> Option<u64> {
    node.params.get("value").and_then(Value::as_u64)
}

fn read_bool_param(node: &GraphNode) -> Option<bool> {
    node.params.get("value").and_then(Value::as_bool)
}

fn read_string_param(node: &GraphNode) -> Option<&str> {
    node.params.get("value").and_then(Value::as_str)
}

fn read_representation_param(node: &GraphNode, default: &str) -> String {
    node.params
        .get("representation")
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_owned()
}

fn read_color_space_param(node: &GraphNode) -> String {
    node.params
        .get("colorSpace")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_COLOR_SPACE)
        .to_owned()
}

fn read_rgba_param(node: &GraphNode) -> Option<[f64; 4]> {
    let values = node.params.get("value")?.as_array()?;
    let [r, g, b, a] = values.as_slice() else {
        return None;
    };
    Some([r.as_f64()?, g.as_f64()?, b.as_f64()?, a.as_f64()?])
}

fn default_color_space() -> String {
    DEFAULT_COLOR_SPACE.to_owned()
}

fn sanitize_f64(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, json};

    use super::*;

    #[test]
    fn serializes_control_values_with_canonical_type_labels() {
        assert_eq!(
            serde_json::to_value(ControlValue::float(32.0)).unwrap(),
            json!({ "type": "float", "representation": "f32", "value": 32.0 })
        );
        assert_eq!(
            serde_json::to_value(ControlValue::int(32)).unwrap(),
            json!({ "type": "int", "representation": "i32", "value": 32 })
        );
        assert_eq!(
            serde_json::to_value(ControlValue::uint(32)).unwrap(),
            json!({ "type": "uint", "representation": "u32", "value": 32 })
        );
        assert_eq!(
            serde_json::to_value(ControlValue::bool(true)).unwrap(),
            json!({ "type": "bool", "value": true })
        );
        assert_eq!(
            serde_json::to_value(ControlValue::string("ready".to_owned())).unwrap(),
            json!({ "type": "string", "value": "ready" })
        );
        assert_eq!(
            serde_json::to_value(ControlValue::color([0.1, 0.2, 0.3, 1.0])).unwrap(),
            json!({
                "type": "color",
                "representation": "rgba32f",
                "colorSpace": "linear",
                "value": [0.1, 0.2, 0.3, 1.0]
            })
        );
    }

    #[test]
    fn deserializes_color_with_default_color_space() {
        assert_eq!(
            serde_json::from_value::<ControlValue>(json!({
                "type": "color",
                "representation": "rgba32f",
                "value": [0.1, 0.2, 0.3, 1.0]
            }))
            .unwrap(),
            ControlValue::Color {
                representation: "rgba32f".to_owned(),
                color_space: "linear".to_owned(),
                value: [0.1, 0.2, 0.3, 1.0],
            }
        );
    }

    #[test]
    fn serializes_control_messages_with_selector_and_atoms() {
        assert_eq!(
            serde_json::to_value(ControlMessage::bang()).unwrap(),
            json!({ "selector": "bang", "atoms": [] })
        );
        assert_eq!(
            serde_json::to_value(ControlMessage::from_value(ControlValue::float(0.5))).unwrap(),
            json!({
                "selector": "float",
                "atoms": [{ "type": "float", "representation": "f32", "value": 0.5 }]
            })
        );
        assert_eq!(
            ControlMessage::parse_text("set on"),
            ControlMessage {
                selector: "set".to_owned(),
                atoms: vec![ControlValue::bool(true)]
            }
        );
    }

    #[test]
    fn parses_and_formats_control_message_text() {
        assert_eq!(
            ControlMessage::parse_text("   "),
            ControlMessage {
                selector: "symbol".to_owned(),
                atoms: vec![ControlValue::string(String::new())]
            }
        );
        assert_eq!(
            ControlMessage::parse_text("route 1 on label"),
            ControlMessage {
                selector: "route".to_owned(),
                atoms: vec![
                    ControlValue::int(1),
                    ControlValue::bool(true),
                    ControlValue::string("label".to_owned())
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
            ControlMessage::from_value(ControlValue::string("ready".to_owned())).to_text(),
            "symbol ready"
        );
        assert_eq!(
            ControlMessage::from_value(ControlValue::uint(9)).to_text(),
            "uint 9"
        );
        assert_eq!(
            ControlMessage::from_value(ControlValue::int(-7)).to_text(),
            "int -7"
        );
        assert_eq!(
            ControlMessage::from_value(ControlValue::bool(true)).to_text(),
            "bool on"
        );
        assert_eq!(
            ControlMessage::from_value(ControlValue::bool(false)).to_text(),
            "bool off"
        );
        assert_eq!(
            ControlMessage::from_value(ControlValue::color([1.0, 0.5, 0.25, 1.0])).to_text(),
            "color 1 0.5 0.25 1"
        );
        assert_eq!(
            ControlMessage::parse_text("color 1 0.5 0.25 1"),
            ControlMessage::from_value(ControlValue::color([1.0, 0.5, 0.25, 1.0]))
        );
        assert_eq!(
            ControlMessage::parse_text("set color 1 0.5 0.25 1"),
            ControlMessage {
                selector: "set".to_owned(),
                atoms: vec![ControlValue::color([1.0, 0.5, 0.25, 1.0])]
            }
        );
        assert_eq!(
            ControlMessage::parse_text("symbol hello world"),
            ControlMessage {
                selector: "symbol".to_owned(),
                atoms: vec![ControlValue::string("hello world".to_owned())]
            }
        );
        assert_eq!(
            ControlMessage::parse_text("float 1.25"),
            ControlMessage::from_value(ControlValue::float(1.25))
        );
        assert_eq!(
            ControlMessage::parse_text("int -12"),
            ControlMessage::from_value(ControlValue::int(-12))
        );
        assert_eq!(
            ControlMessage::parse_text("uint 12"),
            ControlMessage::from_value(ControlValue::uint(12))
        );
        assert_eq!(
            ControlMessage::parse_text("bool off"),
            ControlMessage::from_value(ControlValue::bool(false))
        );
        assert_eq!(
            ControlMessage::parse_text("float 1 2"),
            ControlMessage {
                selector: "float".to_owned(),
                atoms: vec![ControlValue::int(1), ControlValue::int(2)]
            }
        );
        assert_eq!(
            ControlMessage::parse_text("int nope"),
            ControlMessage {
                selector: "int".to_owned(),
                atoms: vec![ControlValue::string("nope".to_owned())]
            }
        );
        assert_eq!(
            ControlMessage::parse_text("uint -1"),
            ControlMessage {
                selector: "uint".to_owned(),
                atoms: vec![ControlValue::int(-1)]
            }
        );
        assert_eq!(
            ControlMessage::parse_text("bool maybe"),
            ControlMessage {
                selector: "bool".to_owned(),
                atoms: vec![ControlValue::string("maybe".to_owned())]
            }
        );
        assert_eq!(
            ControlMessage::parse_text("color 1 2 3"),
            ControlMessage {
                selector: "color".to_owned(),
                atoms: vec![
                    ControlValue::int(1),
                    ControlValue::int(2),
                    ControlValue::int(3)
                ]
            }
        );
        assert_eq!(
            (ControlMessage {
                selector: "list".to_owned(),
                atoms: vec![
                    ControlValue::float(1.5),
                    ControlValue::uint(2),
                    ControlValue::bool(true),
                    ControlValue::bool(false),
                    ControlValue::string("label".to_owned()),
                    ControlValue::color([0.1, 0.2, 0.3, 1.0])
                ]
            })
            .to_text(),
            "list 1.5 2 on off label color 0.1 0.2 0.3 1"
        );
    }

    #[test]
    fn derives_default_value_from_graph_node_params() {
        assert_eq!(
            ControlValue::for_node_default(&node(FLOAT_KIND, json!(1.5))),
            Some(ControlValue::float(1.5))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(INT_KIND, json!(7))),
            Some(ControlValue::int(7))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(UINT_KIND, json!(7))),
            Some(ControlValue::uint(7))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(BOOL_KIND, json!(true))),
            Some(ControlValue::bool(true))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(COLOR_KIND, json!([0.1, 0.2, 0.3, 1.0]))),
            Some(ControlValue::color([0.1, 0.2, 0.3, 1.0]))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(STRING_KIND, json!("ready"))),
            Some(ControlValue::string("ready".to_owned()))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(BOOL_KIND, json!(true))),
            Some(ControlValue::bool(true))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(MESSAGE_KIND, json!("perform"))),
            Some(ControlValue::string("perform".to_owned()))
        );
        assert_eq!(
            ControlValue::for_node_default(&node("core.comment", json!(null))),
            None
        );
        let mut panel = node(PANEL_KIND, json!(null));
        panel.params.insert("color".to_owned(), json!("#00ff00"));
        assert_eq!(
            ControlValue::for_node_default(&panel),
            Some(ControlValue::string("#00ff00".to_owned()))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(FLOAT_KIND, json!(0.75))),
            Some(ControlValue::float(0.75))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(BOOL_KIND, json!(true))),
            Some(ControlValue::bool(true))
        );
    }

    #[test]
    fn invalid_or_missing_params_use_type_defaults() {
        assert_eq!(
            ControlValue::for_node_default(&node(FLOAT_KIND, json!("bad"))),
            Some(ControlValue::float(0.0))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(INT_KIND, json!(1.25))),
            Some(ControlValue::int(0))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(UINT_KIND, json!(-1))),
            Some(ControlValue::uint(0))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(BOOL_KIND, json!("bad"))),
            Some(ControlValue::bool(false))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(COLOR_KIND, json!([0.1, 0.2]))),
            Some(ControlValue::color([1.0, 1.0, 1.0, 1.0]))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(STRING_KIND, json!(false))),
            Some(ControlValue::string(String::new()))
        );
        assert_eq!(
            ControlValue::for_node_default(&node("core.comment", json!(null))),
            None
        );
        assert_eq!(
            ControlValue::for_node_default(&node(PANEL_KIND, json!(null))),
            Some(ControlValue::string("transparent".to_owned()))
        );
    }

    #[test]
    fn matches_only_stored_value_types() {
        assert!(ControlValue::float(1.0).matches_stored_type(&ControlValue::float(0.0)));
        assert!(ControlValue::int(1).matches_stored_type(&ControlValue::int(0)));
        assert!(ControlValue::bool(true).matches_stored_type(&ControlValue::bool(false)));
        assert!(
            ControlValue::string("a".to_owned())
                .matches_stored_type(&ControlValue::string("b".to_owned()))
        );
        assert!(
            ControlValue::color([1.0, 0.0, 0.0, 1.0])
                .matches_stored_type(&ControlValue::color([0.0, 0.0, 0.0, 1.0]))
        );
        assert!(!ControlValue::float(1.0).matches_stored_type(&ControlValue::int(1)));
    }

    #[test]
    fn reports_kind_labels_for_diagnostics() {
        assert_eq!(ControlValue::float(1.0).kind_label(), "number.float/f32");
        assert_eq!(ControlValue::int(1).kind_label(), "number.int/i32");
        assert_eq!(ControlValue::uint(1).kind_label(), "number.uint/u32");
        assert_eq!(ControlValue::bool(true).kind_label(), "boolean");
        assert_eq!(ControlValue::string("x".to_owned()).kind_label(), "string");
        assert_eq!(
            ControlValue::color([0.0, 0.0, 0.0, 1.0]).kind_label(),
            "color/rgba32f"
        );
        assert_eq!(ControlMessage::bang().selector, "bang");
    }

    #[test]
    fn converts_shader_values() {
        assert_eq!(ControlValue::float(1.25).as_f32(), Some(1.25));
        assert_eq!(ControlValue::float(f64::NAN).as_f32(), Some(0.0));
        assert_eq!(ControlValue::int(i64::MAX).as_i32(), Some(i32::MAX));
        assert_eq!(ControlValue::uint(u64::MAX).as_u32(), Some(u32::MAX));
        assert_eq!(ControlValue::bool(true).as_bool(), Some(true));
        assert_eq!(
            ControlValue::color([-1.0, 0.25, 2.0, 1.0]).as_rgba_f32(),
            Some([0.0, 0.25, 1.0, 1.0])
        );
        assert_eq!(ControlValue::bool(false).as_f32(), None);
        assert_eq!(ControlValue::bool(false).as_i32(), None);
        assert_eq!(ControlValue::bool(false).as_u32(), None);
        assert_eq!(ControlValue::float(0.0).as_rgba_f32(), None);
        assert_eq!(ControlValue::float(0.0).as_bool(), None);
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
