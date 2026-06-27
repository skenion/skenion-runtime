use serde_json::{Map, Value, json};

const CURRENT_KIND_VERSION: &str = "0.1.0";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ObjectTextResolution {
    pub(crate) input: String,
    pub(crate) display_text: String,
    pub(crate) class_symbol: String,
    pub(crate) creation_args: Vec<ObjectTextAtom>,
    pub(crate) resolved_kind: Option<String>,
    pub(crate) resolved_kind_version: Option<String>,
    pub(crate) params: Map<String, Value>,
    pub(crate) instance_ports: Vec<ObjectTextPort>,
    pub(crate) diagnostics: Vec<ObjectTextDiagnostic>,
}

impl ObjectTextResolution {
    pub(crate) fn ok(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ObjectTextAtom {
    Float(f64),
    Int(i64),
    Bool(bool),
    Symbol(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObjectTextDiagnostic {
    pub(crate) code: String,
    pub(crate) message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObjectTextPort {
    pub(crate) id: String,
    pub(crate) direction: ObjectTextPortDirection,
    pub(crate) port_type: String,
    pub(crate) rate: ObjectTextPortRate,
    pub(crate) activation: Option<ObjectTextPortActivation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObjectTextPortDirection {
    Input,
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObjectTextPortRate {
    Event,
    Control,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObjectTextPortActivation {
    Trigger,
    Latched,
}

pub(crate) fn resolve_object_text_v01(input: &str) -> ObjectTextResolution {
    let display_text = match normalize_input(input) {
        Ok(display_text) => display_text,
        Err((display_text, message)) => {
            return failure(
                input,
                display_text,
                "<invalid>",
                Vec::new(),
                "object-text.invalid-syntax",
                message,
            );
        }
    };
    let tokens = tokenize(&display_text);
    let Some((class_symbol, arg_tokens)) = tokens.split_first() else {
        return failure(
            input,
            "<empty>".to_owned(),
            "<empty>",
            Vec::new(),
            "object-text.empty",
            "object text must contain a class symbol",
        );
    };
    let creation_args = arg_tokens
        .iter()
        .map(|token| parse_atom(token))
        .collect::<Vec<_>>();

    if is_payload_identity_kind(class_symbol) {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-text.payload-identity",
            format!("{class_symbol} is a payload identity, not an executable object"),
        );
    }

    if let Some(message) = unsupported_first_party_audio_message(class_symbol) {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-text.unsupported-first-party",
            message,
        );
    }

    if let Some(kind) = control_operator_kind(class_symbol) {
        return resolve_control_operator(input, display_text, class_symbol, creation_args, kind);
    }

    if let Some(kind) = control_value_kind(class_symbol) {
        return resolve_control_value(input, display_text, class_symbol, creation_args, kind);
    }

    if let Some(kind) = audio_object_kind(class_symbol) {
        return resolve_audio_object(input, display_text, class_symbol, creation_args, kind);
    }

    if matches!(class_symbol.as_str(), "p" | "object.core.subpatch") {
        return resolve_named_ref_object(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object.core.subpatch",
            "patchRef",
            "subpatch object text requires exactly one patch reference",
        );
    }

    if matches!(class_symbol.as_str(), "inlet" | "object.core.inlet") {
        return resolve_optional_named_ref_object(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object.core.inlet",
            "portId",
        );
    }

    if matches!(class_symbol.as_str(), "outlet" | "object.core.outlet") {
        return resolve_optional_named_ref_object(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object.core.outlet",
            "portId",
        );
    }

    failure(
        input,
        display_text,
        class_symbol,
        creation_args,
        "object-text.unresolved",
        format!("{class_symbol} is not available in the local Runtime object resolver"),
    )
}

pub(crate) fn is_payload_identity_kind(kind: &str) -> bool {
    matches!(
        kind,
        "value"
            | "data"
            | "payload"
            | "bool"
            | "string"
            | "object.core.bool"
            | "object.core.string"
            | "value.core.message"
            | "value.core.bang"
            | "value.core.string"
            | "value.core.tensor"
    ) || kind.starts_with("value.")
        || kind.starts_with("data.")
        || kind.starts_with("payload.")
        || kind.starts_with("control.")
}

fn resolve_control_operator(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectTextAtom>,
    kind: &'static str,
) -> ObjectTextResolution {
    if kind == "object.core.operator.sqrt" {
        if !creation_args.is_empty() {
            return failure(
                input,
                display_text,
                class_symbol,
                creation_args,
                "object-text.invalid-arg-count",
                "sqrt accepts no creation arguments",
            );
        }
        return success(
            input,
            display_text,
            class_symbol,
            creation_args,
            kind,
            Map::new(),
            control_sqrt_ports(),
        );
    }

    if creation_args.len() > 1 {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-text.invalid-arg-count",
            format!("{class_symbol} accepts at most one creation argument"),
        );
    }

    let right = match creation_args.first() {
        Some(arg) => match numeric_value(arg) {
            Some(value) => value,
            None => {
                return failure(
                    input,
                    display_text,
                    class_symbol,
                    creation_args,
                    "object-text.invalid-arg-type",
                    format!("{class_symbol} creation argument must be numeric"),
                );
            }
        },
        None => 0.0,
    };
    let mut params = Map::new();
    insert_number(&mut params, "right", right);
    success(
        input,
        display_text,
        class_symbol,
        creation_args,
        kind,
        params,
        control_operator_ports(),
    )
}

fn resolve_control_value(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectTextAtom>,
    kind: &'static str,
) -> ObjectTextResolution {
    match kind {
        "object.core.bang" => {
            if !creation_args.is_empty() {
                return failure(
                    input,
                    display_text,
                    class_symbol,
                    creation_args,
                    "object-text.invalid-arg-count",
                    format!("{class_symbol} accepts no creation arguments"),
                );
            }
            success(
                input,
                display_text,
                class_symbol,
                creation_args,
                kind,
                Map::new(),
                bang_ports(),
            )
        }
        "object.core.message" | "object.core.comment" => {
            let text = creation_args
                .iter()
                .map(atom_display_text)
                .collect::<Vec<_>>()
                .join(" ");
            let mut params = Map::new();
            params.insert("text".to_owned(), Value::String(text));
            let ports = if kind == "object.core.message" {
                message_ports()
            } else {
                comment_ports()
            };
            success(
                input,
                display_text,
                class_symbol,
                creation_args,
                kind,
                params,
                ports,
            )
        }
        "object.core.float" => resolve_number_value(
            input,
            display_text,
            class_symbol,
            creation_args,
            NumberValueSpec {
                kind,
                port_type: "value.core.float32",
                coerce: numeric_value,
                to_json: |value| json!(value),
            },
        ),
        "object.core.int" => resolve_number_value(
            input,
            display_text,
            class_symbol,
            creation_args,
            NumberValueSpec {
                kind,
                port_type: "value.core.int32",
                coerce: integer_value,
                to_json: |value| json!(value),
            },
        ),
        "object.core.uint" => resolve_number_value(
            input,
            display_text,
            class_symbol,
            creation_args,
            NumberValueSpec {
                kind,
                port_type: "value.core.uint32",
                coerce: unsigned_value,
                to_json: |value| json!(value),
            },
        ),
        _ => unreachable!("control value resolver received unknown kind"),
    }
}

struct NumberValueSpec<T> {
    kind: &'static str,
    port_type: &'static str,
    coerce: fn(&ObjectTextAtom) -> Option<T>,
    to_json: fn(T) -> Value,
}

fn resolve_number_value<T>(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectTextAtom>,
    spec: NumberValueSpec<T>,
) -> ObjectTextResolution {
    if creation_args.len() > 1 {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-text.invalid-arg-count",
            format!("{class_symbol} accepts at most one creation argument"),
        );
    }

    let value = match creation_args.first() {
        Some(arg) => match (spec.coerce)(arg) {
            Some(value) => (spec.to_json)(value),
            None => {
                return failure(
                    input,
                    display_text,
                    class_symbol,
                    creation_args,
                    "object-text.invalid-arg-type",
                    format!("{class_symbol} creation argument has the wrong numeric type"),
                );
            }
        },
        None => json!(0),
    };
    let mut params = Map::new();
    params.insert("value".to_owned(), value);
    success(
        input,
        display_text,
        class_symbol,
        creation_args,
        spec.kind,
        params,
        stored_value_ports(spec.port_type),
    )
}

fn resolve_audio_object(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectTextAtom>,
    kind: &'static str,
) -> ObjectTextResolution {
    match kind {
        "object.core.audio.sig" => resolve_audio_number_param(
            input,
            display_text,
            class_symbol,
            creation_args,
            AudioNumberParamSpec {
                kind,
                param_key: "value",
                default_value: 0.0,
                ports: audio_sig_ports(),
            },
        ),
        "object.core.audio.osc" => resolve_audio_number_param(
            input,
            display_text,
            class_symbol,
            creation_args,
            AudioNumberParamSpec {
                kind,
                param_key: "frequency",
                default_value: 440.0,
                ports: audio_osc_ports(),
            },
        ),
        "object.core.audio.operator.mul" => {
            if !creation_args.is_empty() {
                return failure(
                    input,
                    display_text,
                    class_symbol,
                    creation_args,
                    "object-text.invalid-arg-count",
                    "*~ accepts no creation arguments in the current Runtime audio substrate",
                );
            }
            success(
                input,
                display_text,
                class_symbol,
                creation_args,
                kind,
                Map::new(),
                audio_binary_ports(),
            )
        }
        "object.core.audio.input" | "object.core.audio.output" => {
            if !creation_args.is_empty() {
                return failure(
                    input,
                    display_text,
                    class_symbol,
                    creation_args,
                    "object-text.invalid-arg-count",
                    format!("{class_symbol} accepts no creation arguments"),
                );
            }
            let ports = if kind == "object.core.audio.input" {
                audio_input_ports()
            } else {
                audio_output_ports()
            };
            success(
                input,
                display_text,
                class_symbol,
                creation_args,
                kind,
                Map::new(),
                ports,
            )
        }
        _ => unreachable!("audio object resolver received unknown kind"),
    }
}

struct AudioNumberParamSpec {
    kind: &'static str,
    param_key: &'static str,
    default_value: f64,
    ports: Vec<ObjectTextPort>,
}

fn resolve_audio_number_param(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectTextAtom>,
    spec: AudioNumberParamSpec,
) -> ObjectTextResolution {
    if creation_args.len() > 1 {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-text.invalid-arg-count",
            format!("{class_symbol} accepts at most one creation argument"),
        );
    }
    let value = match creation_args.first() {
        Some(arg) => match numeric_value(arg) {
            Some(value) => value,
            None => {
                return failure(
                    input,
                    display_text,
                    class_symbol,
                    creation_args,
                    "object-text.invalid-arg-type",
                    format!("{class_symbol} creation argument must be numeric"),
                );
            }
        },
        None => spec.default_value,
    };
    let mut params = Map::new();
    insert_number(&mut params, spec.param_key, value);
    success(
        input,
        display_text,
        class_symbol,
        creation_args,
        spec.kind,
        params,
        spec.ports,
    )
}

fn resolve_named_ref_object(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectTextAtom>,
    kind: &'static str,
    param_key: &'static str,
    count_message: &'static str,
) -> ObjectTextResolution {
    if creation_args.len() != 1 {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-text.invalid-arg-count",
            count_message,
        );
    }
    let Some(reference) = symbol_value(&creation_args[0]) else {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-text.invalid-arg-type",
            format!("{class_symbol} reference argument must be a symbol"),
        );
    };
    let mut params = Map::new();
    params.insert(param_key.to_owned(), Value::String(reference));
    success(
        input,
        display_text,
        class_symbol,
        creation_args,
        kind,
        params,
        Vec::new(),
    )
}

fn resolve_optional_named_ref_object(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectTextAtom>,
    kind: &'static str,
    param_key: &'static str,
) -> ObjectTextResolution {
    if creation_args.len() > 1 {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-text.invalid-arg-count",
            format!("{class_symbol} accepts at most one creation argument"),
        );
    }
    let mut params = Map::new();
    if let Some(arg) = creation_args.first() {
        let Some(reference) = symbol_value(arg) else {
            return failure(
                input,
                display_text,
                class_symbol,
                creation_args,
                "object-text.invalid-arg-type",
                format!("{class_symbol} reference argument must be a symbol"),
            );
        };
        params.insert(param_key.to_owned(), Value::String(reference));
    }
    success(
        input,
        display_text,
        class_symbol,
        creation_args,
        kind,
        params,
        Vec::new(),
    )
}

fn success(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectTextAtom>,
    resolved_kind: &str,
    params: Map<String, Value>,
    instance_ports: Vec<ObjectTextPort>,
) -> ObjectTextResolution {
    ObjectTextResolution {
        input: input.to_owned(),
        display_text,
        class_symbol: class_symbol.to_owned(),
        creation_args,
        resolved_kind: Some(resolved_kind.to_owned()),
        resolved_kind_version: Some(CURRENT_KIND_VERSION.to_owned()),
        params,
        instance_ports,
        diagnostics: Vec::new(),
    }
}

fn failure(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectTextAtom>,
    code: &str,
    message: impl Into<String>,
) -> ObjectTextResolution {
    ObjectTextResolution {
        input: input.to_owned(),
        display_text,
        class_symbol: class_symbol.to_owned(),
        creation_args,
        resolved_kind: None,
        resolved_kind_version: None,
        params: Map::new(),
        instance_ports: Vec::new(),
        diagnostics: vec![ObjectTextDiagnostic {
            code: code.to_owned(),
            message: message.into(),
        }],
    }
}

fn normalize_input(input: &str) -> Result<String, (String, String)> {
    let trimmed = input.trim();
    if trimmed.starts_with('[') || trimmed.ends_with(']') {
        if !(trimmed.starts_with('[') && trimmed.ends_with(']')) {
            return Err((
                trimmed.to_owned(),
                "object text brackets must be balanced".to_owned(),
            ));
        }
        return Ok(trimmed[1..trimmed.len() - 1].trim().to_owned());
    }
    Ok(trimmed.to_owned())
}

fn tokenize(display_text: &str) -> Vec<String> {
    display_text.split_whitespace().map(str::to_owned).collect()
}

fn parse_atom(token: &str) -> ObjectTextAtom {
    if token == "true" {
        return ObjectTextAtom::Bool(true);
    }
    if token == "false" {
        return ObjectTextAtom::Bool(false);
    }
    if is_integer_token(token)
        && let Ok(value) = token.parse::<i64>()
    {
        return ObjectTextAtom::Int(value);
    }
    if is_float_token(token)
        && let Ok(value) = token.parse::<f64>()
        && value.is_finite()
    {
        return ObjectTextAtom::Float(value);
    }
    ObjectTextAtom::Symbol(token.to_owned())
}

fn is_integer_token(token: &str) -> bool {
    let digits = token.strip_prefix(['+', '-']).unwrap_or(token);
    !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit())
}

fn is_float_token(token: &str) -> bool {
    token.contains('.') || token.contains('e') || token.contains('E')
}

fn numeric_value(atom: &ObjectTextAtom) -> Option<f64> {
    match atom {
        ObjectTextAtom::Float(value) => Some(*value),
        ObjectTextAtom::Int(value) => Some(*value as f64),
        ObjectTextAtom::Bool(_) | ObjectTextAtom::Symbol(_) => None,
    }
}

fn integer_value(atom: &ObjectTextAtom) -> Option<i64> {
    match atom {
        ObjectTextAtom::Int(value) => Some(*value),
        ObjectTextAtom::Float(_) | ObjectTextAtom::Bool(_) | ObjectTextAtom::Symbol(_) => None,
    }
}

fn unsigned_value(atom: &ObjectTextAtom) -> Option<u64> {
    match atom {
        ObjectTextAtom::Int(value) if *value >= 0 => Some(*value as u64),
        ObjectTextAtom::Float(_) | ObjectTextAtom::Bool(_) | ObjectTextAtom::Symbol(_) => None,
        ObjectTextAtom::Int(_) => None,
    }
}

fn symbol_value(atom: &ObjectTextAtom) -> Option<String> {
    match atom {
        ObjectTextAtom::Symbol(value) if !value.is_empty() => Some(value.clone()),
        ObjectTextAtom::Float(_) | ObjectTextAtom::Int(_) | ObjectTextAtom::Bool(_) => None,
        ObjectTextAtom::Symbol(_) => None,
    }
}

fn atom_display_text(atom: &ObjectTextAtom) -> String {
    match atom {
        ObjectTextAtom::Float(value) => value.to_string(),
        ObjectTextAtom::Int(value) => value.to_string(),
        ObjectTextAtom::Bool(value) => value.to_string(),
        ObjectTextAtom::Symbol(value) => value.clone(),
    }
}

fn insert_number(params: &mut Map<String, Value>, key: &str, value: f64) {
    params.insert(key.to_owned(), json!(value));
}

fn control_operator_kind(class_symbol: &str) -> Option<&'static str> {
    match class_symbol {
        "+" | "add" | "object.core.operator.add" => Some("object.core.operator.add"),
        "-" | "sub" | "object.core.operator.sub" => Some("object.core.operator.sub"),
        "*" | "mul" | "object.core.operator.mul" => Some("object.core.operator.mul"),
        "/" | "div" | "object.core.operator.div" => Some("object.core.operator.div"),
        "pow" | "object.core.operator.pow" => Some("object.core.operator.pow"),
        "min" | "object.core.operator.min" => Some("object.core.operator.min"),
        "max" | "object.core.operator.max" => Some("object.core.operator.max"),
        "sqrt" | "object.core.operator.sqrt" => Some("object.core.operator.sqrt"),
        _ => None,
    }
}

fn control_value_kind(class_symbol: &str) -> Option<&'static str> {
    match class_symbol {
        "f" | "float" | "number" | "object.core.float" => Some("object.core.float"),
        "i" | "int" | "object.core.int" => Some("object.core.int"),
        "u" | "uint" | "object.core.uint" => Some("object.core.uint"),
        "b" | "bang" | "object.core.bang" => Some("object.core.bang"),
        "msg" | "message" | "object.core.message" => Some("object.core.message"),
        "comment" | "object.core.comment" => Some("object.core.comment"),
        _ => None,
    }
}

fn audio_object_kind(class_symbol: &str) -> Option<&'static str> {
    match class_symbol {
        "sig~" | "object.core.audio.sig" => Some("object.core.audio.sig"),
        "osc~" | "object.core.audio.osc" => Some("object.core.audio.osc"),
        "*~" | "object.core.audio.operator.mul" => Some("object.core.audio.operator.mul"),
        "adc~" | "object.core.audio.input" => Some("object.core.audio.input"),
        "dac~" | "object.core.audio.output" => Some("object.core.audio.output"),
        _ => None,
    }
}

fn unsupported_first_party_audio_message(class_symbol: &str) -> Option<&'static str> {
    match class_symbol {
        "+~"
        | "-~"
        | "/~"
        | "object.core.audio.operator.add"
        | "object.core.audio.operator.sub"
        | "object.core.audio.operator.div" => {
            Some("audio add/sub/div aliases are not executable in the current Runtime substrate")
        }
        "sqrt~" | "object.core.audio.operator.sqrt" => {
            Some("audio sqrt is not executable in the current Runtime substrate")
        }
        "phasor~" | "object.core.audio.phasor" => {
            Some("audio phasor is not executable in the current Runtime substrate")
        }
        _ => None,
    }
}

fn input_port(
    id: &str,
    port_type: &str,
    rate: ObjectTextPortRate,
    activation: ObjectTextPortActivation,
) -> ObjectTextPort {
    ObjectTextPort {
        id: id.to_owned(),
        direction: ObjectTextPortDirection::Input,
        port_type: port_type.to_owned(),
        rate,
        activation: Some(activation),
    }
}

fn output_port(id: &str, port_type: &str, rate: ObjectTextPortRate) -> ObjectTextPort {
    ObjectTextPort {
        id: id.to_owned(),
        direction: ObjectTextPortDirection::Output,
        port_type: port_type.to_owned(),
        rate,
        activation: None,
    }
}

fn stored_value_ports(port_type: &str) -> Vec<ObjectTextPort> {
    vec![
        input_port(
            "in",
            "value.core.message",
            ObjectTextPortRate::Control,
            ObjectTextPortActivation::Trigger,
        ),
        input_port(
            "cold",
            port_type,
            ObjectTextPortRate::Control,
            ObjectTextPortActivation::Latched,
        ),
        output_port("value", port_type, ObjectTextPortRate::Control),
    ]
}

fn control_operator_ports() -> Vec<ObjectTextPort> {
    vec![
        input_port(
            "in",
            "value.core.float32",
            ObjectTextPortRate::Control,
            ObjectTextPortActivation::Trigger,
        ),
        input_port(
            "right",
            "value.core.float32",
            ObjectTextPortRate::Control,
            ObjectTextPortActivation::Latched,
        ),
        output_port("out", "value.core.float32", ObjectTextPortRate::Control),
    ]
}

fn control_sqrt_ports() -> Vec<ObjectTextPort> {
    vec![
        input_port(
            "in",
            "value.core.float32",
            ObjectTextPortRate::Control,
            ObjectTextPortActivation::Trigger,
        ),
        output_port("out", "value.core.float32", ObjectTextPortRate::Control),
    ]
}

fn bang_ports() -> Vec<ObjectTextPort> {
    vec![
        input_port(
            "in",
            "value.core.message",
            ObjectTextPortRate::Control,
            ObjectTextPortActivation::Trigger,
        ),
        output_port("out", "value.core.bang", ObjectTextPortRate::Event),
    ]
}

fn message_ports() -> Vec<ObjectTextPort> {
    vec![
        input_port(
            "in",
            "value.core.message",
            ObjectTextPortRate::Control,
            ObjectTextPortActivation::Trigger,
        ),
        output_port("out", "value.core.message", ObjectTextPortRate::Control),
    ]
}

fn comment_ports() -> Vec<ObjectTextPort> {
    vec![input_port(
        "in",
        "value.core.message",
        ObjectTextPortRate::Control,
        ObjectTextPortActivation::Trigger,
    )]
}

fn audio_sig_ports() -> Vec<ObjectTextPort> {
    vec![
        input_port(
            "value",
            "value.core.float32",
            ObjectTextPortRate::Control,
            ObjectTextPortActivation::Latched,
        ),
        output_port("out", "value.core.float32", ObjectTextPortRate::Audio),
    ]
}

fn audio_osc_ports() -> Vec<ObjectTextPort> {
    vec![
        input_port(
            "frequency",
            "value.core.float32",
            ObjectTextPortRate::Control,
            ObjectTextPortActivation::Latched,
        ),
        output_port("out", "value.core.float32", ObjectTextPortRate::Audio),
    ]
}

fn audio_binary_ports() -> Vec<ObjectTextPort> {
    vec![
        input_port(
            "left",
            "value.core.float32",
            ObjectTextPortRate::Audio,
            ObjectTextPortActivation::Latched,
        ),
        input_port(
            "right",
            "value.core.float32",
            ObjectTextPortRate::Audio,
            ObjectTextPortActivation::Latched,
        ),
        output_port("out", "value.core.float32", ObjectTextPortRate::Audio),
    ]
}

fn audio_input_ports() -> Vec<ObjectTextPort> {
    vec![
        output_port("left", "value.core.float32", ObjectTextPortRate::Audio),
        output_port("right", "value.core.float32", ObjectTextPortRate::Audio),
    ]
}

fn audio_output_ports() -> Vec<ObjectTextPort> {
    vec![
        input_port(
            "left",
            "value.core.float32",
            ObjectTextPortRate::Audio,
            ObjectTextPortActivation::Latched,
        ),
        input_port(
            "right",
            "value.core.float32",
            ObjectTextPortRate::Audio,
            ObjectTextPortActivation::Latched,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_kind(resolution: &ObjectTextResolution, kind: &str) {
        assert!(resolution.ok(), "{resolution:?}");
        assert_eq!(resolution.resolved_kind.as_deref(), Some(kind));
        assert_eq!(resolution.resolved_kind_version.as_deref(), Some("0.1.0"));
    }

    fn assert_diagnostic(resolution: &ObjectTextResolution, code: &str) {
        assert_eq!(resolution.resolved_kind, None);
        assert_eq!(resolution.diagnostics[0].code, code);
    }

    #[test]
    fn resolves_runtime_control_aliases_and_validates_args() {
        let add = resolve_object_text_v01("[+ 1e3]");
        assert!(add.ok());
        assert_eq!(add.display_text, "+ 1e3");
        assert_eq!(add.class_symbol, "+");
        assert_eq!(
            add.resolved_kind.as_deref(),
            Some("object.core.operator.add")
        );
        assert_eq!(add.resolved_kind_version.as_deref(), Some("0.1.0"));
        assert_eq!(add.params["right"], json!(1000.0));
        assert_eq!(add.instance_ports[0].id, "in");

        let sqrt = resolve_object_text_v01("sqrt 2");
        assert_eq!(sqrt.diagnostics[0].code, "object-text.invalid-arg-count");

        let invalid = resolve_object_text_v01("+ true");
        assert_eq!(invalid.diagnostics[0].code, "object-text.invalid-arg-type");

        for (input, kind, param, value) in [
            ("- -2", "object.core.operator.sub", "right", json!(-2.0)),
            ("/ 4", "object.core.operator.div", "right", json!(4.0)),
            ("* 3", "object.core.operator.mul", "right", json!(3.0)),
            ("pow 2", "object.core.operator.pow", "right", json!(2.0)),
            ("max 8", "object.core.operator.max", "right", json!(8.0)),
            ("min 1", "object.core.operator.min", "right", json!(1.0)),
        ] {
            let resolution = resolve_object_text_v01(input);
            assert_kind(&resolution, kind);
            assert_eq!(resolution.params[param], value);
            assert_eq!(resolution.instance_ports.len(), 3);
        }

        let sqrt = resolve_object_text_v01("sqrt");
        assert_kind(&sqrt, "object.core.operator.sqrt");
        assert_eq!(sqrt.instance_ports.len(), 2);

        let default_add = resolve_object_text_v01("object.core.operator.add");
        assert_kind(&default_add, "object.core.operator.add");
        assert_eq!(default_add.params["right"], json!(0.0));

        assert_diagnostic(
            &resolve_object_text_v01("sqrt 1"),
            "object-text.invalid-arg-count",
        );
        assert_diagnostic(
            &resolve_object_text_v01("+ 1 2"),
            "object-text.invalid-arg-count",
        );
        assert_diagnostic(
            &resolve_object_text_v01("object.core.operator.mul false"),
            "object-text.invalid-arg-type",
        );
    }

    #[test]
    fn resolves_runtime_value_audio_and_subpatch_aliases() {
        let float = resolve_object_text_v01("f 0.25");
        assert!(float.ok());
        assert_eq!(float.resolved_kind.as_deref(), Some("object.core.float"));
        assert_eq!(float.params["value"], json!(0.25));

        let osc = resolve_object_text_v01("osc~ 220");
        assert!(osc.ok());
        assert_eq!(osc.resolved_kind.as_deref(), Some("object.core.audio.osc"));
        assert_eq!(osc.params["frequency"], json!(220.0));

        let mul = resolve_object_text_v01("*~");
        assert!(mul.ok());
        assert_eq!(
            mul.resolved_kind.as_deref(),
            Some("object.core.audio.operator.mul")
        );
        assert_eq!(mul.instance_ports.len(), 3);

        let scalar_mul = resolve_object_text_v01("*~ 0.5");
        assert_eq!(
            scalar_mul.diagnostics[0].code,
            "object-text.invalid-arg-count"
        );

        let unsupported = resolve_object_text_v01("+~");
        assert_eq!(
            unsupported.diagnostics[0].code,
            "object-text.unsupported-first-party"
        );

        for input in [
            "-~",
            "/~",
            "sqrt~",
            "phasor~",
            "object.core.audio.operator.add",
            "object.core.audio.operator.sqrt",
            "object.core.audio.phasor",
        ] {
            assert_diagnostic(
                &resolve_object_text_v01(input),
                "object-text.unsupported-first-party",
            );
        }

        let sig = resolve_object_text_v01("sig~");
        assert_kind(&sig, "object.core.audio.sig");
        assert_eq!(sig.params["value"], json!(0.0));

        let invalid_sig = resolve_object_text_v01("sig~ false");
        assert_diagnostic(&invalid_sig, "object-text.invalid-arg-type");
        assert_diagnostic(
            &resolve_object_text_v01("sig~ 1 2"),
            "object-text.invalid-arg-count",
        );

        let osc = resolve_object_text_v01("object.core.audio.osc 220");
        assert_kind(&osc, "object.core.audio.osc");
        assert_eq!(osc.params["frequency"], json!(220.0));
        assert_diagnostic(
            &resolve_object_text_v01("osc~ nope"),
            "object-text.invalid-arg-type",
        );

        let audio_input = resolve_object_text_v01("adc~");
        assert_kind(&audio_input, "object.core.audio.input");
        assert_eq!(audio_input.instance_ports[0].id, "left");

        let audio_output = resolve_object_text_v01("dac~");
        assert_kind(&audio_output, "object.core.audio.output");
        assert_eq!(audio_output.instance_ports[0].id, "left");

        let invalid_audio_output = resolve_object_text_v01("dac~ 1");
        assert_diagnostic(&invalid_audio_output, "object-text.invalid-arg-count");

        let subpatch = resolve_object_text_v01("p voice");
        assert!(subpatch.ok());
        assert_eq!(
            subpatch.resolved_kind.as_deref(),
            Some("object.core.subpatch")
        );
        assert_eq!(subpatch.params["patchRef"], json!("voice"));
    }

    #[test]
    fn resolves_runtime_value_boxes_and_boundary_aliases() {
        for (input, kind, value) in [
            ("float", "object.core.float", json!(0)),
            ("int -7", "object.core.int", json!(-7)),
            ("uint 9", "object.core.uint", json!(9)),
        ] {
            let resolution = resolve_object_text_v01(input);
            assert_kind(&resolution, kind);
            assert_eq!(resolution.params["value"], value);
            assert_eq!(resolution.instance_ports.len(), 3);
        }

        assert_diagnostic(
            &resolve_object_text_v01("int 1.5"),
            "object-text.invalid-arg-type",
        );
        assert_diagnostic(
            &resolve_object_text_v01("uint -1"),
            "object-text.invalid-arg-type",
        );
        assert_diagnostic(
            &resolve_object_text_v01("float 1 2"),
            "object-text.invalid-arg-count",
        );

        let bang = resolve_object_text_v01("bang");
        assert_kind(&bang, "object.core.bang");
        assert!(bang.params.is_empty());
        assert_eq!(bang.instance_ports[1].port_type, "value.core.bang");
        assert_diagnostic(
            &resolve_object_text_v01("object.core.bang 1"),
            "object-text.invalid-arg-count",
        );

        let float_alias = resolve_object_text_v01("f 1.5");
        assert_kind(&float_alias, "object.core.float");
        assert_eq!(float_alias.params["value"], json!(1.5));
        assert_diagnostic(
            &resolve_object_text_v01("float true"),
            "object-text.invalid-arg-type",
        );

        let message = resolve_object_text_v01("message set gain");
        assert_kind(&message, "object.core.message");
        assert_eq!(message.params["text"], json!("set gain"));
        let empty_message = resolve_object_text_v01("msg");
        assert_kind(&empty_message, "object.core.message");
        assert_eq!(empty_message.params["text"], json!(""));

        let comment = resolve_object_text_v01("comment hello world");
        assert_kind(&comment, "object.core.comment");
        assert_eq!(comment.params["text"], json!("hello world"));
        assert_eq!(comment.instance_ports.len(), 1);
        let empty_comment = resolve_object_text_v01("object.core.comment");
        assert_kind(&empty_comment, "object.core.comment");
        assert_eq!(empty_comment.params["text"], json!(""));

        let inlet = resolve_object_text_v01("inlet left");
        assert_kind(&inlet, "object.core.inlet");
        assert_eq!(inlet.params["portId"], json!("left"));

        let anonymous_outlet = resolve_object_text_v01("outlet");
        assert_kind(&anonymous_outlet, "object.core.outlet");
        assert!(anonymous_outlet.params.is_empty());
        let named_outlet = resolve_object_text_v01("object.core.outlet right");
        assert_kind(&named_outlet, "object.core.outlet");
        assert_eq!(named_outlet.params["portId"], json!("right"));

        assert_diagnostic(
            &resolve_object_text_v01("p"),
            "object-text.invalid-arg-count",
        );
        assert_diagnostic(
            &resolve_object_text_v01("p true"),
            "object-text.invalid-arg-type",
        );
        assert_diagnostic(
            &resolve_object_text_v01("inlet left right"),
            "object-text.invalid-arg-count",
        );
        assert_diagnostic(
            &resolve_object_text_v01("outlet 1"),
            "object-text.invalid-arg-type",
        );
    }

    #[test]
    fn rejects_payload_identities_as_object_text() {
        for input in [
            "value",
            "data",
            "payload",
            "value.core.float32",
            "bool",
            "string",
            "object.core.bool",
            "object.core.string",
            "value.core.bang",
            "value.core.message",
            "value.core.string",
            "value.core.tensor",
            "data.vendor.payload",
            "payload.vendor.frame",
            "control.float",
        ] {
            let resolution = resolve_object_text_v01(input);
            assert_eq!(resolution.resolved_kind, None);
            assert_eq!(
                resolution.diagnostics[0].code,
                "object-text.payload-identity"
            );
        }
    }

    #[test]
    fn reports_unresolved_and_syntax_diagnostics_without_runtime_mapping() {
        let unresolved = resolve_object_text_v01("user.manipulator 1");
        assert_eq!(unresolved.diagnostics[0].code, "object-text.unresolved");

        let invalid = resolve_object_text_v01("[+ 1");
        assert_eq!(invalid.diagnostics[0].code, "object-text.invalid-syntax");

        let empty = resolve_object_text_v01("   ");
        assert_eq!(empty.diagnostics[0].code, "object-text.empty");
    }
}
