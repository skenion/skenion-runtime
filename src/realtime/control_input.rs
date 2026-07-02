use std::collections::BTreeMap;

use crate::{
    ControlValue, RuntimeControlEventRequest, RuntimeControlEventResponse, RuntimeIssue,
    RuntimeSessionRecord,
};

pub(super) fn apply_control_input(
    record: &RuntimeSessionRecord,
    request: RuntimeControlEventRequest,
) -> (
    RuntimeControlEventResponse,
    BTreeMap<String, ControlValue>,
    RuntimeControlEventRequest,
) {
    let (mut response, control_snapshot, changed_values) = {
        let mut session = record
            .session
            .write()
            .expect("runtime session lock should not be poisoned");
        let before = session.control_state_response().values;
        let response = session.apply_control_event(request.clone());
        let after = session.control_state_response().values;
        let changed_values = changed_control_values(&before, &after);
        let control_snapshot = if response.ok && response.changed {
            session.preview_control_state_snapshot()
        } else {
            None
        };
        (response, control_snapshot, changed_values)
    };

    if let Some(control_snapshot) = control_snapshot {
        let mut preview = record
            .preview
            .lock()
            .expect("runtime preview lock should not be poisoned");
        if let Err(error) = preview.update_control_state(control_snapshot) {
            response.issues.push(RuntimeIssue::warning(format!(
                "failed to update running preview control state: {error}"
            )));
        }
    }

    (response, changed_values, request)
}

fn changed_control_values(
    before: &BTreeMap<String, ControlValue>,
    after: &BTreeMap<String, ControlValue>,
) -> BTreeMap<String, ControlValue> {
    after
        .iter()
        .filter(|(node_id, value)| before.get(*node_id) != Some(*value))
        .map(|(node_id, value)| (node_id.clone(), value.clone()))
        .collect()
}
