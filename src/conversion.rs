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
        "number.float" => numeric_to_float(value, representation.unwrap_or("f32")),
        "number.int" => numeric_to_int(value, representation.unwrap_or("i32")),
        "number.uint" => numeric_to_uint(value, representation.unwrap_or("u32")),
        "boolean" => match value {
            ControlValue::Bool { value } => Some(ControlValue::bool(*value)),
            _ => None,
        },
        "color" => color_to_color(value, representation.unwrap_or("rgba32f"), "linear"),
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
        "rgba16f" => value.map(|component| component.clamp(0.0, 1.0) as f32 as f64),
        _ => value.map(|component| component.clamp(0.0, 1.0)),
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
}
