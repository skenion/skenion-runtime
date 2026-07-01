use midir::{Ignore, MidiInput, MidiInputPort};
use serde::{Deserialize, Serialize};

use crate::io_device_manager::{RuntimeIoIssue, RuntimeIoIssueSeverity};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeMidiInputPort {
    pub index: usize,
    pub name: String,
}

pub(crate) fn create_midi_input(client_name: &str) -> Result<MidiInput, RuntimeIoIssue> {
    let mut input = MidiInput::new(client_name).map_err(|error| RuntimeIoIssue {
        severity: RuntimeIoIssueSeverity::Error,
        code: "io-device-enumeration-failed".to_owned(),
        message: format!("failed to initialize MIDI input host: {error}"),
    })?;
    input.ignore(Ignore::None);
    Ok(input)
}

pub(crate) fn collect_midi_input_ports(
    input: &MidiInput,
    midir_ports: &[MidiInputPort],
    issues: &mut Vec<RuntimeIoIssue>,
) -> Vec<RuntimeMidiInputPort> {
    midir_ports
        .iter()
        .enumerate()
        .map(|(index, port)| RuntimeMidiInputPort {
            index,
            name: input.port_name(port).unwrap_or_else(|error| {
                issues.push(RuntimeIoIssue {
                    severity: RuntimeIoIssueSeverity::Warning,
                    code: "io-device-name-unavailable".to_owned(),
                    message: format!("failed to read MIDI input port {index} name: {error}"),
                });
                format!("MIDI Input {index}")
            }),
        })
        .collect()
}
