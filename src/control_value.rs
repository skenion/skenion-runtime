use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::GraphNode;

pub const FLOAT_KIND: &str = "object.core.float";
pub const INT_KIND: &str = "object.core.int";
pub const COLOR_KIND: &str = "object.core.color";
pub const BANG_KIND: &str = "object.core.bang";
pub const MESSAGE_KIND: &str = "object.core.message";
pub const COMMENT_KIND: &str = "object.core.comment";
pub const PANEL_KIND: &str = "object.core.panel";
pub const OPERATOR_ADD_KIND: &str = "object.core.operator.add";
pub const OPERATOR_SUB_KIND: &str = "object.core.operator.sub";
pub const OPERATOR_MUL_KIND: &str = "object.core.operator.mul";
pub const OPERATOR_DIV_KIND: &str = "object.core.operator.div";
pub const OPERATOR_POW_KIND: &str = "object.core.operator.pow";
pub const OPERATOR_MIN_KIND: &str = "object.core.operator.min";
pub const OPERATOR_MAX_KIND: &str = "object.core.operator.max";
pub const OPERATOR_SQRT_KIND: &str = "object.core.operator.sqrt";

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
    pub key: String,
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

    pub(crate) fn for_node_default(node: &GraphNode) -> Option<Self> {
        match node.kind.as_str() {
            FLOAT_KIND => Some(Self::Float {
                representation: read_representation_param(node, DEFAULT_FLOAT_REPRESENTATION),
                value: read_f64_param(node).unwrap_or(0.0),
            }),
            INT_KIND => {
                let representation = read_representation_param(node, DEFAULT_INT_REPRESENTATION);
                if is_unsigned_int_representation(&representation) {
                    Some(Self::Uint {
                        representation,
                        value: read_u64_param(node).unwrap_or(0),
                    })
                } else {
                    Some(Self::Int {
                        representation,
                        value: read_i64_param(node).unwrap_or(0),
                    })
                }
            }
            COLOR_KIND => Some(Self::Color {
                representation: read_representation_param(node, DEFAULT_COLOR_REPRESENTATION),
                color_space: read_color_space_param(node),
                value: read_rgba_param(node).unwrap_or([1.0, 1.0, 1.0, 1.0]),
            }),
            MESSAGE_KIND => Some(Self::string(read_string_param(node).unwrap_or_default())),
            COMMENT_KIND => Some(Self::string(
                node.params
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )),
            PANEL_KIND => Some(Self::string(
                node.params
                    .get("color")
                    .and_then(Value::as_str)
                    .unwrap_or("transparent"),
            )),
            OPERATOR_ADD_KIND | OPERATOR_SUB_KIND | OPERATOR_MUL_KIND | OPERATOR_DIV_KIND
            | OPERATOR_POW_KIND | OPERATOR_MIN_KIND | OPERATOR_MAX_KIND | OPERATOR_SQRT_KIND => {
                Some(Self::float(0.0))
            }
            _ => None,
        }
    }

    pub fn kind_label(&self) -> String {
        match self {
            Self::Float { representation, .. } => {
                format!(
                    "{}/{}",
                    value_type_id_for_float_representation(representation)
                        .unwrap_or("value.core.float32"),
                    representation
                )
            }
            Self::Int { representation, .. } => {
                format!(
                    "{}/{}",
                    value_type_id_for_int_representation(representation)
                        .unwrap_or("value.core.int32"),
                    representation
                )
            }
            Self::Uint { representation, .. } => {
                format!(
                    "{}/{}",
                    value_type_id_for_int_representation(representation)
                        .unwrap_or("value.core.uint32"),
                    representation
                )
            }
            Self::Bool { .. } => "value.core.bool".to_owned(),
            Self::String { .. } => "value.core.string".to_owned(),
            Self::Color { representation, .. } => format!("value.core.color/{representation}"),
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

pub(crate) fn value_type_id_for_float_representation(representation: &str) -> Option<&'static str> {
    match representation {
        "f64" => Some("value.core.float64"),
        "f32" => Some("value.core.float32"),
        "f16" => Some("value.core.float16"),
        "f8.e4m3" | "f8.e5m2" => Some("value.core.float8"),
        "ufloat64" => Some("value.core.ufloat64"),
        "ufloat32" => Some("value.core.ufloat32"),
        "ufloat16" => Some("value.core.ufloat16"),
        "ufloat8" => Some("value.core.ufloat8"),
        _ => None,
    }
}

pub(crate) fn value_type_id_for_int_representation(representation: &str) -> Option<&'static str> {
    match representation {
        "i64" => Some("value.core.int64"),
        "i32" => Some("value.core.int32"),
        "i16" => Some("value.core.int16"),
        "i8" => Some("value.core.int8"),
        "u64" => Some("value.core.uint64"),
        "u32" => Some("value.core.uint32"),
        "u16" => Some("value.core.uint16"),
        "u8" => Some("value.core.uint8"),
        _ => None,
    }
}

pub(crate) fn is_unsigned_int_representation(representation: &str) -> bool {
    matches!(representation, "u64" | "u32" | "u16" | "u8")
}

impl ControlMessage {
    pub fn bang() -> Self {
        Self {
            key: "bang".to_owned(),
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
            key: selector,
            atoms: vec![value],
        }
    }

    pub fn parse_text(text: &str) -> Self {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Self {
                key: "symbol".to_owned(),
                atoms: vec![ControlValue::string(String::new())],
            };
        }

        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let selector = parts.next().unwrap_or_default();
        let rest = parts.next().unwrap_or_default().trim();
        match selector {
            "bang" if rest.is_empty() => Self::bang(),
            "set" => Self {
                key: "set".to_owned(),
                atoms: parse_message_atoms(rest),
            },
            "float" => typed_or_generic_message(selector, rest, parse_float_atom),
            "int" => typed_or_generic_message(selector, rest, parse_int_atom),
            "uint" => typed_or_generic_message(selector, rest, parse_uint_atom),
            "bool" => typed_or_generic_message(selector, rest, parse_bool_atom),
            "symbol" => Self {
                key: "symbol".to_owned(),
                atoms: vec![ControlValue::string(rest.to_owned())],
            },
            "color" => typed_or_generic_message(selector, rest, parse_color_atom),
            "on" | "off" | "true" | "false" if rest.is_empty() => Self {
                key: selector.to_owned(),
                atoms: Vec::new(),
            },
            _ if rest.is_empty() => {
                if let Some(value) = parse_scalar_atom(trimmed) {
                    Self::from_value(value)
                } else {
                    Self {
                        key: "symbol".to_owned(),
                        atoms: vec![ControlValue::string(trimmed.to_owned())],
                    }
                }
            }
            _ => Self {
                key: selector.to_owned(),
                atoms: parse_message_atoms(rest),
            },
        }
    }

    pub fn first_atom(&self) -> Option<&ControlValue> {
        self.atoms.first()
    }

    pub fn to_text(&self) -> String {
        if self.atoms.is_empty() {
            return self.key.clone();
        }
        if self.atoms.len() == 1
            && let Some(payload) = typed_atom_payload_to_text(&self.key, &self.atoms[0])
        {
            return format!("{} {}", self.key, payload);
        }
        format!(
            "{} {}",
            self.key,
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
            key: selector.to_owned(),
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
    use serde_json::{Map, Value, json};

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
    fn serializes_control_messages_with_key_and_atoms() {
        assert_eq!(
            serde_json::to_value(ControlMessage::bang()).unwrap(),
            json!({ "key": "bang", "atoms": [] })
        );
        assert_eq!(
            serde_json::to_value(ControlMessage::from_value(ControlValue::float(0.5))).unwrap(),
            json!({
                "key": "float",
                "atoms": [{ "type": "float", "representation": "f32", "value": 0.5 }]
            })
        );
        assert_eq!(
            ControlMessage::parse_text("set on"),
            ControlMessage {
                key: "set".to_owned(),
                atoms: vec![ControlValue::bool(true)]
            }
        );
    }

    #[test]
    fn parses_and_formats_control_message_text() {
        assert_eq!(
            ControlMessage::parse_text("   "),
            ControlMessage {
                key: "symbol".to_owned(),
                atoms: vec![ControlValue::string(String::new())]
            }
        );
        assert_eq!(
            ControlMessage::parse_text("route 1 on label"),
            ControlMessage {
                key: "route".to_owned(),
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
                key: "set".to_owned(),
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
                key: "set".to_owned(),
                atoms: vec![ControlValue::color([1.0, 0.5, 0.25, 1.0])]
            }
        );
        assert_eq!(
            ControlMessage::parse_text("symbol hello world"),
            ControlMessage {
                key: "symbol".to_owned(),
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
                key: "float".to_owned(),
                atoms: vec![ControlValue::int(1), ControlValue::int(2)]
            }
        );
        assert_eq!(
            ControlMessage::parse_text("int nope"),
            ControlMessage {
                key: "int".to_owned(),
                atoms: vec![ControlValue::string("nope".to_owned())]
            }
        );
        assert_eq!(
            ControlMessage::parse_text("uint -1"),
            ControlMessage {
                key: "uint".to_owned(),
                atoms: vec![ControlValue::int(-1)]
            }
        );
        assert_eq!(
            ControlMessage::parse_text("bool maybe"),
            ControlMessage {
                key: "bool".to_owned(),
                atoms: vec![ControlValue::string("maybe".to_owned())]
            }
        );
        assert_eq!(
            ControlMessage::parse_text("color 1 2 3"),
            ControlMessage {
                key: "color".to_owned(),
                atoms: vec![
                    ControlValue::int(1),
                    ControlValue::int(2),
                    ControlValue::int(3)
                ]
            }
        );
        assert_eq!(
            (ControlMessage {
                key: "list".to_owned(),
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
            ControlValue::for_node_default(&node_with_representation(INT_KIND, json!(7), "u32")),
            Some(ControlValue::uint(7))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(COLOR_KIND, json!([0.1, 0.2, 0.3, 1.0]))),
            Some(ControlValue::color([0.1, 0.2, 0.3, 1.0]))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(MESSAGE_KIND, json!("perform"))),
            Some(ControlValue::string("perform".to_owned()))
        );
        let mut comment = node(COMMENT_KIND, json!(null));
        comment.params.insert("text".to_owned(), json!("note"));
        assert_eq!(
            ControlValue::for_node_default(&comment),
            Some(ControlValue::string("note".to_owned()))
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
            ControlValue::for_node_default(&node_with_representation(INT_KIND, json!(-1), "u32")),
            Some(ControlValue::uint(0))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(COLOR_KIND, json!([0.1, 0.2]))),
            Some(ControlValue::color([1.0, 1.0, 1.0, 1.0]))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(MESSAGE_KIND, json!(false))),
            Some(ControlValue::string(String::new()))
        );
        assert_eq!(
            ControlValue::for_node_default(&node(COMMENT_KIND, json!(null))),
            Some(ControlValue::string(String::new()))
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
    fn reports_kind_labels_for_issues() {
        assert_eq!(
            ControlValue::float(1.0).kind_label(),
            "value.core.float32/f32"
        );
        assert_eq!(ControlValue::int(1).kind_label(), "value.core.int32/i32");
        assert_eq!(ControlValue::uint(1).kind_label(), "value.core.uint32/u32");
        assert_eq!(ControlValue::bool(true).kind_label(), "value.core.bool");
        assert_eq!(
            ControlValue::string("x".to_owned()).kind_label(),
            "value.core.string"
        );
        assert_eq!(
            ControlValue::color([0.0, 0.0, 0.0, 1.0]).kind_label(),
            "value.core.color/rgba32f"
        );
        assert_eq!(ControlMessage::bang().key, "bang");
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
        node_with_params(kind, params)
    }

    fn node_with_representation(
        kind: &str,
        value: serde_json::Value,
        representation: &str,
    ) -> GraphNode {
        let mut params = Map::new();
        params.insert("value".to_owned(), value);
        params.insert("representation".to_owned(), json!(representation));
        node_with_params(kind, params)
    }

    fn node_with_params(kind: &str, params: Map<String, Value>) -> GraphNode {
        GraphNode {
            id: "value_1".to_owned(),
            kind: kind.to_owned(),
            kind_version: "0.1.0".to_owned(),
            params,
            ports: Vec::new(),
        }
    }
}
