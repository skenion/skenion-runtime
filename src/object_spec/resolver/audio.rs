use serde_json::Map;

use super::super::ports::{
    audio_binary_ports, audio_input_ports, audio_osc_ports, audio_output_ports, audio_sig_ports,
};
use super::super::{ObjectRegistryCandidate, ObjectSpecAtom, ObjectSpecPort, ObjectSpecResolution};
use super::atoms::{insert_number, numeric_value};
use super::outcome::{failure, success};

pub(super) fn resolve_audio_object(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidate: &ObjectRegistryCandidate,
) -> ObjectSpecResolution {
    let kind = candidate.executable_kind.as_str();
    match kind {
        "object.core.audio.sig" => resolve_audio_number_param(
            input,
            display_text,
            class_symbol,
            creation_args,
            candidate,
            AudioNumberParamSpec {
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
            candidate,
            AudioNumberParamSpec {
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
                    "object-spec.invalid-arg-count",
                    "*~ accepts no creation arguments in the current Runtime audio substrate",
                );
            }
            success(
                input,
                display_text,
                class_symbol,
                creation_args,
                candidate,
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
                    "object-spec.invalid-arg-count",
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
                candidate,
                Map::new(),
                ports,
            )
        }
        _ => unreachable!("audio object resolver received unknown kind"),
    }
}

struct AudioNumberParamSpec {
    param_key: &'static str,
    default_value: f64,
    ports: Vec<ObjectSpecPort>,
}

fn resolve_audio_number_param(
    input: &str,
    display_text: String,
    class_symbol: &str,
    creation_args: Vec<ObjectSpecAtom>,
    candidate: &ObjectRegistryCandidate,
    spec: AudioNumberParamSpec,
) -> ObjectSpecResolution {
    if creation_args.len() > 1 {
        return failure(
            input,
            display_text,
            class_symbol,
            creation_args,
            "object-spec.invalid-arg-count",
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
                    "object-spec.invalid-arg-type",
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
        candidate,
        params,
        spec.ports,
    )
}

pub(in crate::object_spec) fn unsupported_first_party_audio_message(
    class_symbol: &str,
) -> Option<&'static str> {
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
