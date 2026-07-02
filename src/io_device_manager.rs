use std::sync::Arc;

use serde::{Deserialize, Serialize};

#[cfg(not(test))]
use crate::midi_input::{collect_midi_input_ports, create_midi_input};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeIoIssueSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeIoIssue {
    pub severity: RuntimeIoIssueSeverity,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeIoTransportKind {
    Midi,
    Hid,
    Serial,
    Inline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeIoDirection {
    Input,
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeIoDeviceDescriptor {
    pub id: String,
    pub name: String,
    pub transport_kind: RuntimeIoTransportKind,
    pub directions: Vec<RuntimeIoDirection>,
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    pub stable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeIoDeviceListResponse {
    pub ok: bool,
    pub devices: Vec<RuntimeIoDeviceDescriptor>,
    pub issues: Vec<RuntimeIoIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeIoInlineFrame {
    pub at_ns: u64,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RuntimeIoBindingConfig {
    #[serde(rename = "midi")]
    Midi { device_id: String },
    #[serde(rename = "hid")]
    Hid { device_id: String },
    #[serde(rename = "serial")]
    Serial {
        device_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        baud_rate: Option<u32>,
    },
    #[serde(rename = "inline")]
    Inline { frames: Vec<RuntimeIoInlineFrame> },
}

pub struct RuntimeIoDeviceManager {
    devices: Arc<dyn RuntimeIoDeviceRegistry>,
}

impl RuntimeIoDeviceManager {
    #[cfg(not(test))]
    pub fn new() -> Self {
        Self {
            devices: Arc::new(MidirRuntimeIoDeviceRegistry),
        }
    }

    #[cfg(test)]
    pub fn new() -> Self {
        Self {
            devices: Arc::new(StaticRuntimeIoDeviceRegistry),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_device_registry(devices: Arc<dyn RuntimeIoDeviceRegistry>) -> Self {
        Self { devices }
    }

    pub fn list_devices(&self) -> RuntimeIoDeviceListResponse {
        self.devices.list_devices()
    }
}

impl Default for RuntimeIoDeviceManager {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) trait RuntimeIoDeviceRegistry: Send + Sync {
    fn list_devices(&self) -> RuntimeIoDeviceListResponse;
}

#[cfg(not(test))]
struct MidirRuntimeIoDeviceRegistry;

#[cfg(not(test))]
impl RuntimeIoDeviceRegistry for MidirRuntimeIoDeviceRegistry {
    fn list_devices(&self) -> RuntimeIoDeviceListResponse {
        match create_midi_input("skenion-runtime-io-discovery") {
            Ok(input) => {
                let midir_ports = input.ports();
                let mut issues = Vec::new();
                let ports = collect_midi_input_ports(&input, &midir_ports, &mut issues);
                RuntimeIoDeviceListResponse {
                    ok: !issues
                        .iter()
                        .any(|issue| issue.severity == RuntimeIoIssueSeverity::Error),
                    devices: ports
                        .into_iter()
                        .map(|port| RuntimeIoDeviceDescriptor {
                            id: format!("midir:input:{}", port.index),
                            name: port.name,
                            transport_kind: RuntimeIoTransportKind::Midi,
                            directions: vec![RuntimeIoDirection::Input],
                            backend: "midir".to_owned(),
                            index: Some(port.index),
                            stable: false,
                        })
                        .collect(),
                    issues,
                }
            }
            Err(issue) => RuntimeIoDeviceListResponse {
                ok: false,
                devices: Vec::new(),
                issues: vec![issue],
            },
        }
    }
}

#[cfg(test)]
struct StaticRuntimeIoDeviceRegistry;

#[cfg(test)]
impl RuntimeIoDeviceRegistry for StaticRuntimeIoDeviceRegistry {
    fn list_devices(&self) -> RuntimeIoDeviceListResponse {
        RuntimeIoDeviceListResponse {
            ok: true,
            devices: Vec::new(),
            issues: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeDeviceRegistry {
        devices: Vec<RuntimeIoDeviceDescriptor>,
    }

    impl RuntimeIoDeviceRegistry for FakeDeviceRegistry {
        fn list_devices(&self) -> RuntimeIoDeviceListResponse {
            RuntimeIoDeviceListResponse {
                ok: true,
                devices: self.devices.clone(),
                issues: Vec::new(),
            }
        }
    }

    #[test]
    fn lists_runtime_io_devices_without_opening_them() {
        let manager = RuntimeIoDeviceManager::with_device_registry(Arc::new(FakeDeviceRegistry {
            devices: vec![RuntimeIoDeviceDescriptor {
                id: "midir:input:0".to_owned(),
                name: "Fake MIDI".to_owned(),
                transport_kind: RuntimeIoTransportKind::Midi,
                directions: vec![RuntimeIoDirection::Input],
                backend: "midir".to_owned(),
                index: Some(0),
                stable: false,
            }],
        }));

        let response = manager.list_devices();

        assert!(response.ok);
        assert_eq!(response.devices.len(), 1);
        assert_eq!(response.devices[0].id, "midir:input:0");
        assert_eq!(
            response.devices[0].directions,
            vec![RuntimeIoDirection::Input]
        );
    }

    #[test]
    fn default_manager_reports_empty_test_registry() {
        let response = RuntimeIoDeviceManager::default().list_devices();

        assert!(response.ok);
        assert!(response.devices.is_empty());
        assert!(response.issues.is_empty());
    }

    #[test]
    fn io_binding_config_carries_transport_options_but_no_decoder_semantics() {
        let binding = RuntimeIoBindingConfig::Serial {
            device_id: "serial:/dev/tty.usbmodem101".to_owned(),
            baud_rate: Some(115_200),
        };

        assert!(matches!(binding, RuntimeIoBindingConfig::Serial { .. }));
    }
}
