use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

use crate::{ControlMessage, ControlState, ControlValue, telemetry::unix_ms_timestamp};

pub const PREVIEW_CONTROL_STATE_SCHEMA: &str = "skenion.preview.control-state";
pub const PREVIEW_CONTROL_STATE_SCHEMA_VERSION: &str = "0.1.0";
static PREVIEW_CONTROL_STATE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewControlStateSnapshot {
    pub schema: String,
    pub schema_version: String,
    pub session_revision: u64,
    pub control_revision: u64,
    pub values: std::collections::BTreeMap<String, ControlValue>,
    pub channels: std::collections::BTreeMap<String, ControlMessage>,
    #[serde(default)]
    pub operator_right: std::collections::BTreeMap<String, ControlValue>,
    pub written_at: String,
}

impl PreviewControlStateSnapshot {
    pub fn new(session_revision: u64, control_revision: u64, control_state: &ControlState) -> Self {
        Self {
            schema: PREVIEW_CONTROL_STATE_SCHEMA.to_owned(),
            schema_version: PREVIEW_CONTROL_STATE_SCHEMA_VERSION.to_owned(),
            session_revision,
            control_revision,
            values: control_state.values.clone(),
            channels: control_state.channels.clone(),
            operator_right: control_state.operator_right.clone(),
            written_at: unix_ms_timestamp(),
        }
    }

    pub fn control_state(&self) -> ControlState {
        ControlState {
            values: self.values.clone(),
            channels: self.channels.clone(),
            operator_right: self.operator_right.clone(),
        }
    }
}

pub fn preview_control_state_path(session_revision: u64) -> PathBuf {
    let directory = std::env::temp_dir().join("skenion-runtime-preview");
    let nonce = PREVIEW_CONTROL_STATE_COUNTER.fetch_add(1, Ordering::Relaxed);
    directory.join(format!(
        "preview-{}-{}-{}-control-state.json",
        std::process::id(),
        nonce,
        session_revision
    ))
}

pub fn read_preview_control_state_snapshot(
    path: &Path,
) -> Result<Option<PreviewControlStateSnapshot>, String> {
    let Some(bytes) = read_preview_control_state_bytes(path).map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };

    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| error.to_string())
}

pub fn write_preview_control_state_snapshot(
    path: &Path,
    snapshot: &PreviewControlStateSnapshot,
) -> Result<(), String> {
    write_preview_control_state_snapshot_io(path, snapshot).map_err(|error| error.to_string())
}

fn read_preview_control_state_bytes(path: &Path) -> io::Result<Option<Vec<u8>>> {
    if !path.exists() {
        return Ok(None);
    }

    fs::read(path).map(Some)
}

fn write_preview_control_state_snapshot_io(
    path: &Path,
    snapshot: &PreviewControlStateSnapshot,
) -> io::Result<()> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(fs::create_dir_all)
        .transpose()?;
    let temp_path = path.with_extension("json.tmp");
    let bytes =
        serde_json::to_vec_pretty(snapshot).expect("preview control snapshot should serialize");
    fs::write(&temp_path, bytes)?;
    fs::rename(&temp_path, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_control_state_snapshot_round_trips() {
        let mut control_state = ControlState::default();
        control_state
            .values
            .insert("slider_1".to_owned(), ControlValue::float(0.75));
        control_state.channels.insert(
            "value.core.float32:speed".to_owned(),
            ControlMessage::from_value(ControlValue::float(0.75)),
        );
        control_state
            .operator_right
            .insert("mul_1".to_owned(), ControlValue::float(0.5));
        let snapshot = PreviewControlStateSnapshot::new(12, 5, &control_state);
        let bytes = serde_json::to_vec(&snapshot).expect("snapshot should serialize");
        let decoded: PreviewControlStateSnapshot =
            serde_json::from_slice(&bytes).expect("snapshot should deserialize");

        assert_eq!(decoded.schema, PREVIEW_CONTROL_STATE_SCHEMA);
        assert_eq!(decoded.schema_version, PREVIEW_CONTROL_STATE_SCHEMA_VERSION);
        assert_eq!(decoded.session_revision, 12);
        assert_eq!(decoded.control_revision, 5);
        assert_eq!(decoded.control_state(), control_state);
    }

    #[test]
    fn writes_and_reads_preview_control_state_atomically() {
        let path = std::env::temp_dir().join(format!(
            "skenion-preview-control-state-test-{}.json",
            std::process::id()
        ));
        let mut control_state = ControlState::default();
        control_state.channels.insert(
            "value.core.bool:enabled".to_owned(),
            ControlMessage::from_value(ControlValue::bool(true)),
        );
        control_state
            .operator_right
            .insert("mul_1".to_owned(), ControlValue::float(0.5));
        let snapshot = PreviewControlStateSnapshot::new(3, 2, &control_state);

        write_preview_control_state_snapshot(&path, &snapshot).expect("snapshot should write");
        let decoded = read_preview_control_state_snapshot(&path)
            .expect("snapshot should read")
            .expect("snapshot should exist");

        assert_eq!(decoded, snapshot);
        std::fs::remove_file(path).expect("snapshot should be removable");
    }

    #[test]
    fn read_missing_preview_control_state_returns_none() {
        let path = unique_temp_path("missing-control-state.json");

        let snapshot =
            read_preview_control_state_snapshot(&path).expect("missing snapshot should be ok");

        assert_eq!(snapshot, None);
    }

    #[test]
    fn read_invalid_preview_control_state_reports_error() {
        let path = unique_temp_path("invalid-control-state.json");
        fs::write(&path, b"{").expect("invalid snapshot should write");

        let error =
            read_preview_control_state_snapshot(&path).expect_err("invalid snapshot should fail");

        assert!(error.contains("EOF") || error.contains("expected"));
        fs::remove_file(path).expect("invalid snapshot should remove");
    }

    #[test]
    fn read_preview_control_state_reports_read_errors() {
        let directory = unique_temp_path("read-error");
        fs::create_dir_all(&directory).expect("directory should create");

        let error = read_preview_control_state_snapshot(&directory)
            .expect_err("directory read should fail");

        assert!(!error.is_empty());
        fs::remove_dir_all(directory).expect("directory should remove");
    }

    #[test]
    fn write_preview_control_state_reports_parent_create_error() {
        let blocker = unique_temp_path("control-state-parent-blocker");
        fs::write(&blocker, b"blocker").expect("blocker should write");
        let snapshot = PreviewControlStateSnapshot::new(1, 1, &ControlState::default());

        let error = write_preview_control_state_snapshot(&blocker.join("control.json"), &snapshot)
            .expect_err("blocked parent should fail");

        assert!(!error.is_empty());
        fs::remove_file(blocker).expect("blocker should remove");
    }

    fn unique_temp_path(name: &str) -> PathBuf {
        let nonce = PREVIEW_CONTROL_STATE_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "skenion-preview-control-state-test-{}-{nonce}-{name}",
            std::process::id()
        ))
    }
}
