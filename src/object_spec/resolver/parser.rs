use super::super::{ObjectSpecResolution, ParsedObjectSpec};
use super::atoms::contract_object_spec_atom_to_runtime;
use super::outcome::failure;

pub(in crate::object_spec) fn parse_object_spec_input_v01(
    input: &str,
) -> Result<ParsedObjectSpec, Box<ObjectSpecResolution>> {
    let parsed = skenion_contracts::parse_object_spec_v01(input);
    let creation_args = parsed
        .creation_args
        .iter()
        .map(contract_object_spec_atom_to_runtime)
        .collect::<Vec<_>>();
    if parsed.ok {
        return Ok(ParsedObjectSpec {
            input: parsed.input,
            display_text: parsed.display_text,
            class_symbol: parsed.class_name,
            creation_args,
        });
    }

    let issue = parsed.issues.first();
    let code = issue
        .map(|issue| runtime_object_spec_issue_code(&issue.code))
        .unwrap_or_else(|| "object-spec.invalid-syntax".to_owned());
    let message = issue
        .map(|issue| issue.message.clone())
        .unwrap_or_else(|| "object spec could not be parsed".to_owned());
    Err(Box::new(failure(
        &parsed.input,
        parsed.display_text,
        &parsed.class_name,
        creation_args,
        code,
        message,
    )))
}

pub(in crate::object_spec) fn runtime_object_spec_issue_code(code: &str) -> String {
    match code {
        "empty-object-spec" => "object-spec.empty".to_owned(),
        "invalid-syntax" => "object-spec.invalid-syntax".to_owned(),
        value if value.starts_with("object-spec.") => value.to_owned(),
        value => format!("object-spec.{value}"),
    }
}
