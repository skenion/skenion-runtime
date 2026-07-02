use serde_json::{Map, Value, json};

use super::super::ObjectSpecAtom;

pub(in crate::object_spec) fn contract_object_spec_atom_to_runtime(
    atom: &skenion_contracts::ObjectSpecAtomV01,
) -> ObjectSpecAtom {
    match atom {
        skenion_contracts::ObjectSpecAtomV01::Float { value, .. } => ObjectSpecAtom::Float(*value),
        skenion_contracts::ObjectSpecAtomV01::Int { value, .. } => ObjectSpecAtom::Int(*value),
        skenion_contracts::ObjectSpecAtomV01::Uint { value, .. } => {
            if *value <= i64::MAX as u64 {
                ObjectSpecAtom::Int(*value as i64)
            } else {
                ObjectSpecAtom::Symbol(value.to_string())
            }
        }
        skenion_contracts::ObjectSpecAtomV01::Bool { value } => ObjectSpecAtom::Bool(*value),
        skenion_contracts::ObjectSpecAtomV01::Identifier { value }
        | skenion_contracts::ObjectSpecAtomV01::String { value } => {
            ObjectSpecAtom::Symbol(value.clone())
        }
    }
}

pub(super) fn numeric_value(atom: &ObjectSpecAtom) -> Option<f64> {
    match atom {
        ObjectSpecAtom::Float(value) => Some(*value),
        ObjectSpecAtom::Int(value) => Some(*value as f64),
        ObjectSpecAtom::Bool(_) | ObjectSpecAtom::Symbol(_) => None,
    }
}

pub(super) fn integer_value(atom: &ObjectSpecAtom) -> Option<i64> {
    match atom {
        ObjectSpecAtom::Int(value) => Some(*value),
        ObjectSpecAtom::Float(_) | ObjectSpecAtom::Bool(_) | ObjectSpecAtom::Symbol(_) => None,
    }
}

pub(super) fn symbol_value(atom: &ObjectSpecAtom) -> Option<String> {
    match atom {
        ObjectSpecAtom::Symbol(value) if !value.is_empty() => Some(value.clone()),
        ObjectSpecAtom::Float(_) | ObjectSpecAtom::Int(_) | ObjectSpecAtom::Bool(_) => None,
        ObjectSpecAtom::Symbol(_) => None,
    }
}

pub(super) fn atom_display_text(atom: &ObjectSpecAtom) -> String {
    match atom {
        ObjectSpecAtom::Float(value) => value.to_string(),
        ObjectSpecAtom::Int(value) => value.to_string(),
        ObjectSpecAtom::Bool(value) => value.to_string(),
        ObjectSpecAtom::Symbol(value) => value.clone(),
    }
}

pub(super) fn insert_number(params: &mut Map<String, Value>, key: &str, value: f64) {
    params.insert(key.to_owned(), json!(value));
}
