use crate::ControlValue;

pub fn convert_control_value_to_stored(
    value: &ControlValue,
    stored: &ControlValue,
) -> Option<ControlValue> {
    match stored {
        ControlValue::Float { representation, .. } => numeric_to_float(value, representation),
        ControlValue::Int { representation, .. } => numeric_to_int(value, representation),
        ControlValue::Uint { representation, .. } => numeric_to_uint(value, representation),
        ControlValue::Color {
            representation,
            color_space,
            ..
        } => color_to_color(value, representation, color_space),
        _ => None,
    }
}

pub fn convert_control_value_to_data_kind(
    value: &ControlValue,
    data_kind: &str,
    representation: Option<&str>,
) -> Option<ControlValue> {
    match data_kind {
        "value.core.float32" => numeric_to_float(value, representation.unwrap_or("f32")),
        "value.core.int32" => numeric_to_int(value, representation.unwrap_or("i32")),
        "value.core.uint32" => numeric_to_uint(value, representation.unwrap_or("u32")),
        "value.core.bool" => match value {
            ControlValue::Bool { value } => Some(ControlValue::bool(*value)),
            _ => None,
        },
        "value.core.color" => color_to_color(value, representation.unwrap_or("rgba32f"), "linear"),
        _ => None,
    }
}

fn numeric_to_float(value: &ControlValue, representation: &str) -> Option<ControlValue> {
    let numeric = numeric_as_f64(value)?;
    Some(ControlValue::Float {
        representation: representation.to_owned(),
        value: quantize_float(sanitize_f64(numeric), representation),
    })
}

fn numeric_to_int(value: &ControlValue, representation: &str) -> Option<ControlValue> {
    let numeric = numeric_as_f64(value)?;
    let (min, max) = int_range(representation)?;
    Some(ControlValue::Int {
        representation: representation.to_owned(),
        value: sanitize_f64(numeric).trunc().clamp(min as f64, max as f64) as i64,
    })
}

fn numeric_to_uint(value: &ControlValue, representation: &str) -> Option<ControlValue> {
    let numeric = numeric_as_f64(value)?;
    let max = uint_max(representation)?;
    Some(ControlValue::Uint {
        representation: representation.to_owned(),
        value: sanitize_f64(numeric).trunc().clamp(0.0, max as f64) as u64,
    })
}

fn color_to_color(
    value: &ControlValue,
    representation: &str,
    color_space: &str,
) -> Option<ControlValue> {
    let ControlValue::Color { value, .. } = value else {
        return None;
    };
    Some(ControlValue::Color {
        representation: representation.to_owned(),
        color_space: color_space.to_owned(),
        value: convert_color(*value, representation),
    })
}

fn numeric_as_f64(value: &ControlValue) -> Option<f64> {
    match value {
        ControlValue::Float { value, .. } => Some(*value),
        ControlValue::Int { value, .. } => Some(*value as f64),
        ControlValue::Uint { value, .. } => Some(*value as f64),
        ControlValue::Bool { value } => Some(if *value { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn sanitize_f64(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

fn quantize_float(value: f64, representation: &str) -> f64 {
    match representation {
        "f64" => value,
        "f32" | "f16" => value as f32 as f64,
        "f8.e4m3" | "f8.e5m2" => (value * 16.0).round() / 16.0,
        "ufloat16" => value.max(0.0) as f32 as f64,
        "ufloat8" => (value.max(0.0) * 16.0).round() / 16.0,
        _ => value,
    }
}

fn int_range(representation: &str) -> Option<(i64, i64)> {
    match representation {
        "i64" => Some((i64::MIN, i64::MAX)),
        "i32" => Some((i32::MIN as i64, i32::MAX as i64)),
        "i16" => Some((i16::MIN as i64, i16::MAX as i64)),
        "i8" => Some((i8::MIN as i64, i8::MAX as i64)),
        _ => None,
    }
}

fn uint_max(representation: &str) -> Option<u64> {
    match representation {
        "u64" => Some(u64::MAX),
        "u32" => Some(u32::MAX as u64),
        "u16" => Some(u16::MAX as u64),
        "u8" => Some(u8::MAX as u64),
        _ => None,
    }
}

fn convert_color(value: [f64; 4], representation: &str) -> [f64; 4] {
    match representation {
        "rgba8unorm" => value.map(quantize_unorm8),
        "rgb8unorm" => [
            quantize_unorm8(value[0]),
            quantize_unorm8(value[1]),
            quantize_unorm8(value[2]),
            1.0,
        ],
        "rgba16f" => value.map(|component| sanitize_f64(component).clamp(0.0, 1.0) as f32 as f64),
        _ => value.map(|component| sanitize_f64(component).clamp(0.0, 1.0)),
    }
}

fn quantize_unorm8(value: f64) -> f64 {
    (sanitize_f64(value).clamp(0.0, 1.0) * 255.0).round() / 255.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_numeric_values_with_saturating_policy() {
        let i8_target = ControlValue::Int {
            representation: "i8".to_owned(),
            value: 0,
        };
        let u8_target = ControlValue::Uint {
            representation: "u8".to_owned(),
            value: 0,
        };

        assert_eq!(
            convert_control_value_to_stored(&ControlValue::int(300), &i8_target),
            Some(ControlValue::Int {
                representation: "i8".to_owned(),
                value: 127,
            })
        );
        assert_eq!(
            convert_control_value_to_stored(&ControlValue::int(-200), &i8_target),
            Some(ControlValue::Int {
                representation: "i8".to_owned(),
                value: -128,
            })
        );
        assert_eq!(
            convert_control_value_to_stored(&ControlValue::int(-1), &u8_target),
            Some(ControlValue::Uint {
                representation: "u8".to_owned(),
                value: 0,
            })
        );
        assert_eq!(
            convert_control_value_to_stored(&ControlValue::float(12.9), &u8_target),
            Some(ControlValue::Uint {
                representation: "u8".to_owned(),
                value: 12,
            })
        );
        assert_eq!(
            convert_control_value_to_stored(&ControlValue::bool(true), &ControlValue::float(0.0)),
            Some(ControlValue::float(1.0))
        );
        assert_eq!(
            convert_control_value_to_stored(&ControlValue::bool(false), &i8_target),
            Some(ControlValue::Int {
                representation: "i8".to_owned(),
                value: 0,
            })
        );
        assert_eq!(
            convert_control_value_to_stored(&ControlValue::bool(true), &u8_target),
            Some(ControlValue::Uint {
                representation: "u8".to_owned(),
                value: 1,
            })
        );
    }

    #[test]
    fn sanitizes_float_and_quantizes_color() {
        let f32_target = ControlValue::float(0.0);
        assert_eq!(
            convert_control_value_to_stored(
                &ControlValue::Float {
                    representation: "f32".to_owned(),
                    value: f64::NAN,
                },
                &f32_target,
            ),
            Some(ControlValue::float(0.0))
        );

        let color_target = ControlValue::Color {
            representation: "rgba8unorm".to_owned(),
            color_space: "linear".to_owned(),
            value: [0.0, 0.0, 0.0, 1.0],
        };
        assert_eq!(
            convert_control_value_to_stored(
                &ControlValue::color([-1.0, 0.5, 2.0, 1.0]),
                &color_target
            ),
            Some(ControlValue::Color {
                representation: "rgba8unorm".to_owned(),
                color_space: "linear".to_owned(),
                value: [0.0, 128.0 / 255.0, 1.0, 1.0],
            })
        );
    }

    #[test]
    fn converts_to_requested_data_kind_and_representations() {
        assert_eq!(
            convert_control_value_to_data_kind(
                &ControlValue::uint(7),
                "value.core.float32",
                Some("f64")
            ),
            Some(ControlValue::Float {
                representation: "f64".to_owned(),
                value: 7.0,
            })
        );
        assert_eq!(
            convert_control_value_to_data_kind(
                &ControlValue::bool(true),
                "value.core.float32",
                None
            ),
            Some(ControlValue::float(1.0))
        );
        assert_eq!(
            convert_control_value_to_data_kind(
                &ControlValue::float(1.28),
                "value.core.float32",
                Some("f8.e4m3")
            ),
            Some(ControlValue::Float {
                representation: "f8.e4m3".to_owned(),
                value: 1.25,
            })
        );
        assert_eq!(
            convert_control_value_to_data_kind(
                &ControlValue::float(-1.0),
                "value.core.float32",
                Some("ufloat16")
            ),
            Some(ControlValue::Float {
                representation: "ufloat16".to_owned(),
                value: 0.0,
            })
        );
        assert_eq!(
            convert_control_value_to_data_kind(
                &ControlValue::float(1.28),
                "value.core.float32",
                Some("ufloat8")
            ),
            Some(ControlValue::Float {
                representation: "ufloat8".to_owned(),
                value: 1.25,
            })
        );
        assert_eq!(
            convert_control_value_to_data_kind(
                &ControlValue::float(f64::INFINITY),
                "value.core.float32",
                Some("f16")
            ),
            Some(ControlValue::Float {
                representation: "f16".to_owned(),
                value: 0.0,
            })
        );
        assert_eq!(
            convert_control_value_to_data_kind(
                &ControlValue::float(1.5),
                "value.core.float32",
                Some("vendor.float")
            ),
            Some(ControlValue::Float {
                representation: "vendor.float".to_owned(),
                value: 1.5,
            })
        );
        assert_eq!(
            convert_control_value_to_data_kind(
                &ControlValue::uint(300),
                "value.core.int32",
                Some("i16")
            ),
            Some(ControlValue::Int {
                representation: "i16".to_owned(),
                value: 300,
            })
        );
        assert_eq!(
            convert_control_value_to_data_kind(
                &ControlValue::int(-1),
                "value.core.uint32",
                Some("u16")
            ),
            Some(ControlValue::Uint {
                representation: "u16".to_owned(),
                value: 0,
            })
        );
        assert_eq!(
            convert_control_value_to_data_kind(
                &ControlValue::bool(false),
                "value.core.int32",
                None
            ),
            Some(ControlValue::int(0))
        );
        assert_eq!(
            convert_control_value_to_data_kind(
                &ControlValue::bool(true),
                "value.core.uint32",
                None
            ),
            Some(ControlValue::uint(1))
        );
        assert_eq!(
            convert_control_value_to_data_kind(&ControlValue::bool(true), "value.core.bool", None),
            Some(ControlValue::bool(true))
        );
        assert_eq!(
            convert_control_value_to_data_kind(
                &ControlValue::color([0.25, 0.5, 0.75, 0.0]),
                "value.core.color",
                Some("rgb8unorm")
            ),
            Some(ControlValue::Color {
                representation: "rgb8unorm".to_owned(),
                color_space: "linear".to_owned(),
                value: [64.0 / 255.0, 128.0 / 255.0, 191.0 / 255.0, 1.0],
            })
        );
    }

    #[test]
    fn rejects_incompatible_or_unknown_conversions() {
        assert_eq!(
            convert_control_value_to_stored(&ControlValue::float(1.0), &ControlValue::bool(false)),
            None
        );
        assert_eq!(
            convert_control_value_to_stored(
                &ControlValue::string("x"),
                &ControlValue::Float {
                    representation: "bad.float".to_owned(),
                    value: 0.0,
                },
            ),
            None
        );
        assert_eq!(
            convert_control_value_to_data_kind(&ControlValue::float(1.0), "unknown.kind", None),
            None
        );
        assert_eq!(
            convert_control_value_to_data_kind(&ControlValue::float(1.0), "value.core.bool", None),
            None
        );
        assert_eq!(
            convert_control_value_to_data_kind(&ControlValue::float(1.0), "value.core.color", None),
            None
        );
        assert_eq!(
            convert_control_value_to_data_kind(
                &ControlValue::float(1.0),
                "value.core.int32",
                Some("bad.int")
            ),
            None
        );
        assert_eq!(
            convert_control_value_to_data_kind(
                &ControlValue::float(1.0),
                "value.core.uint32",
                Some("bad.uint")
            ),
            None
        );
    }

    #[test]
    fn clamps_wide_integer_representations_and_half_color() {
        assert_eq!(
            convert_control_value_to_data_kind(
                &ControlValue::float(i64::MAX as f64),
                "value.core.int32",
                Some("i64")
            ),
            Some(ControlValue::Int {
                representation: "i64".to_owned(),
                value: i64::MAX,
            })
        );
        assert_eq!(
            convert_control_value_to_data_kind(
                &ControlValue::float(u64::MAX as f64),
                "value.core.uint32",
                Some("u64")
            ),
            Some(ControlValue::Uint {
                representation: "u64".to_owned(),
                value: u64::MAX,
            })
        );
        assert_eq!(
            convert_control_value_to_data_kind(
                &ControlValue::color([-0.5, 0.25, 2.0, f64::NAN]),
                "value.core.color",
                Some("rgba16f")
            ),
            Some(ControlValue::Color {
                representation: "rgba16f".to_owned(),
                color_space: "linear".to_owned(),
                value: [0.0, 0.25, 1.0, 0.0],
            })
        );
    }
}
