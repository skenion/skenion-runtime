use skenion_runtime::{
    RuntimeIoDeviceManager, RuntimeIoDirection, RuntimeIoIssueSeverity, RuntimeIoTransportKind,
};

#[test]
fn production_io_discovery_returns_structured_midi_response() {
    let response = RuntimeIoDeviceManager::new().list_devices();

    if response.ok {
        assert!(
            response
                .issues
                .iter()
                .all(|issue| issue.severity != RuntimeIoIssueSeverity::Error)
        );
        for (expected_index, device) in response.devices.iter().enumerate() {
            assert!(!device.id.is_empty());
            assert!(!device.name.is_empty());
            assert_eq!(device.transport_kind, RuntimeIoTransportKind::Midi);
            assert_eq!(device.directions, vec![RuntimeIoDirection::Input]);
            assert_eq!(device.backend, "midir");
            assert_eq!(device.index, Some(expected_index));
            assert!(!device.stable);
        }
    } else {
        assert!(response.devices.is_empty());
        assert!(
            response
                .issues
                .iter()
                .any(|issue| issue.severity == RuntimeIoIssueSeverity::Error)
        );
    }

    for issue in &response.issues {
        assert!(!issue.code.is_empty());
        assert!(!issue.message.is_empty());
    }
}
