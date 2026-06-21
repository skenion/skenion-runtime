use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    PreviewState, RuntimeDiagnostic, RuntimePreviewStatusResponse, RuntimeSessionSnapshot,
};

pub const TELEMETRY_SCHEMA: &str = "skenion.runtime.telemetry";
pub const TELEMETRY_SCHEMA_VERSION: &str = "0.1.0";
pub const PREVIEW_TELEMETRY_SCHEMA: &str = "skenion.preview.telemetry";
pub const PREVIEW_TELEMETRY_SCHEMA_VERSION: &str = "0.1.0";
static PREVIEW_TELEMETRY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTelemetrySnapshot {
    pub schema: String,
    pub schema_version: String,
    pub ok: bool,
    pub timestamp: String,
    pub session: RuntimeTelemetrySession,
    pub preview: RuntimeTelemetryPreview,
    pub render: RuntimeTelemetryRender,
    pub process: RuntimeTelemetryProcess,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTelemetrySession {
    pub loaded: bool,
    pub graph_id: Option<String>,
    pub graph_revision: Option<String>,
    pub session_revision: u64,
    pub control_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTelemetryPreview {
    pub state: PreviewState,
    pub pid: Option<u32>,
    pub stale: bool,
    pub graph_id: Option<String>,
    pub graph_revision: Option<String>,
    pub session_revision: Option<u64>,
    pub preview_session_revision: Option<u64>,
    pub control_revision: Option<u64>,
    pub preview_control_revision: Option<u64>,
    pub control_live: bool,
    pub last_control_update_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTelemetryRender {
    pub active: bool,
    pub backend: Option<String>,
    pub renderer: Option<String>,
    pub frames_rendered: u64,
    pub approx_fps: Option<f64>,
    pub last_frame_ms: Option<f64>,
    pub last_error: Option<String>,
    pub source_node_id: Option<String>,
    pub diagnostics: Vec<ShaderDiagnostic>,
    pub generated_source_available: bool,
    pub control_revision: Option<u64>,
    pub preview_control_revision: Option<u64>,
    pub control_live: bool,
    pub last_control_update_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTelemetryProcess {
    pub runtime_version: String,
    pub uptime_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewTelemetryHeartbeat {
    pub schema: String,
    pub schema_version: String,
    pub timestamp: String,
    pub pid: u32,
    pub graph_id: String,
    pub graph_revision: String,
    pub session_revision: u64,
    pub renderer: String,
    pub backend: String,
    pub frames_rendered: u64,
    pub approx_fps: Option<f64>,
    pub last_frame_ms: Option<f64>,
    pub last_error: Option<String>,
    pub source_node_id: Option<String>,
    pub diagnostics: Vec<ShaderDiagnostic>,
    pub generated_source_available: bool,
    pub control_revision: Option<u64>,
    pub preview_control_revision: Option<u64>,
    pub control_live: bool,
    pub last_control_update_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShaderDiagnosticSeverity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShaderDiagnosticPhase {
    InterfaceAnalysis,
    SourceSync,
    WgslGeneration,
    WgslCompile,
    RenderPipeline,
    RenderFrame,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShaderDiagnosticSource {
    User,
    Generated,
    Runtime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShaderDiagnostic {
    pub severity: ShaderDiagnosticSeverity,
    pub phase: ShaderDiagnosticPhase,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_column: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uniform_id: Option<String>,
    pub source: ShaderDiagnosticSource,
}

impl ShaderDiagnostic {
    pub fn new(
        severity: ShaderDiagnosticSeverity,
        phase: ShaderDiagnosticPhase,
        code: impl Into<String>,
        message: impl Into<String>,
        source: ShaderDiagnosticSource,
    ) -> Self {
        Self {
            severity,
            phase,
            code: code.into(),
            message: message.into(),
            line: None,
            column: None,
            end_line: None,
            end_column: None,
            uniform_id: None,
            source,
        }
    }

    pub fn error(
        phase: ShaderDiagnosticPhase,
        code: impl Into<String>,
        message: impl Into<String>,
        source: ShaderDiagnosticSource,
    ) -> Self {
        Self::new(
            ShaderDiagnosticSeverity::Error,
            phase,
            code,
            message,
            source,
        )
    }

    pub fn with_line_column(mut self, line: Option<usize>, column: Option<usize>) -> Self {
        self.line = line;
        self.column = column;
        self
    }

    pub fn with_uniform_id(mut self, uniform_id: Option<String>) -> Self {
        self.uniform_id = uniform_id;
        self
    }
}

#[derive(Debug)]
pub struct PreviewTelemetryWriter {
    path: PathBuf,
    graph_id: String,
    graph_revision: String,
    session_revision: u64,
    renderer: String,
    backend: String,
    source_node_id: Option<String>,
    frames_rendered: u64,
    first_frame_at: Option<Instant>,
    last_write_at: Option<Instant>,
    last_frame_ms: Option<f64>,
    last_error: Option<String>,
    diagnostics: Vec<ShaderDiagnostic>,
    generated_source_available: bool,
    control_revision: Option<u64>,
    preview_control_revision: Option<u64>,
    control_live: bool,
    last_control_update_at: Option<String>,
}

impl RuntimeTelemetrySnapshot {
    pub(crate) fn from_parts(
        session: RuntimeSessionSnapshot,
        preview: RuntimePreviewStatusResponse,
        heartbeat: Option<PreviewTelemetryHeartbeat>,
        dry_run: bool,
        uptime_ms: u64,
        diagnostics: Vec<RuntimeDiagnostic>,
    ) -> Self {
        let render = render_from_preview(&preview, heartbeat, dry_run, &diagnostics);

        Self {
            schema: TELEMETRY_SCHEMA.to_owned(),
            schema_version: TELEMETRY_SCHEMA_VERSION.to_owned(),
            ok: true,
            timestamp: unix_ms_timestamp(),
            session: RuntimeTelemetrySession {
                loaded: session.loaded(),
                graph_id: session.graph_id().map(ToOwned::to_owned),
                graph_revision: session.graph_revision().map(ToOwned::to_owned),
                session_revision: session.session_revision,
                control_revision: session.control_revision,
            },
            preview: RuntimeTelemetryPreview {
                state: preview.state,
                pid: preview.pid,
                stale: preview.stale,
                graph_id: preview.graph_id,
                graph_revision: preview.graph_revision,
                session_revision: preview.session_revision,
                preview_session_revision: preview.preview_session_revision,
                control_revision: preview.control_revision,
                preview_control_revision: preview.preview_control_revision,
                control_live: preview.control_live,
                last_control_update_at: preview.last_control_update_at,
            },
            render,
            process: RuntimeTelemetryProcess {
                runtime_version: env!("CARGO_PKG_VERSION").to_owned(),
                uptime_ms,
            },
            diagnostics,
        }
    }
}

impl PreviewTelemetryWriter {
    pub fn new(
        path: PathBuf,
        graph_id: String,
        graph_revision: String,
        session_revision: u64,
        renderer: impl Into<String>,
        backend: impl Into<String>,
        source_node_id: Option<String>,
    ) -> Self {
        let renderer = renderer.into();
        let generated_source_available = renderer == "fullscreen-shader";
        Self {
            path,
            graph_id,
            graph_revision,
            session_revision,
            renderer,
            backend: backend.into(),
            source_node_id,
            frames_rendered: 0,
            first_frame_at: None,
            last_write_at: None,
            last_frame_ms: None,
            last_error: None,
            diagnostics: Vec::new(),
            generated_source_available,
            control_revision: None,
            preview_control_revision: None,
            control_live: false,
            last_control_update_at: None,
        }
    }

    pub fn record_frame(&mut self, last_frame_ms: f64) {
        let now = Instant::now();
        self.frames_rendered += 1;
        self.first_frame_at.get_or_insert(now);
        self.last_frame_ms = Some(last_frame_ms.max(0.0));
        self.write_if_due(now, false);
    }

    pub fn record_error(&mut self, error: impl Into<String>) {
        let message = error.into();
        self.last_error = Some(message.clone());
        self.diagnostics.push(ShaderDiagnostic::error(
            ShaderDiagnosticPhase::RenderFrame,
            "render-error",
            message,
            ShaderDiagnosticSource::Runtime,
        ));
        self.write_if_due(Instant::now(), true);
    }

    pub fn record_shader_diagnostic(&mut self, diagnostic: ShaderDiagnostic) {
        if diagnostic.severity == ShaderDiagnosticSeverity::Error {
            self.last_error = Some(diagnostic.message.clone());
        }
        self.diagnostics.push(diagnostic);
        self.write_if_due(Instant::now(), true);
    }

    pub fn record_control_revision(
        &mut self,
        control_revision: u64,
        last_control_update_at: String,
    ) {
        self.control_revision = Some(control_revision);
        self.preview_control_revision = Some(control_revision);
        self.control_live = true;
        self.last_control_update_at = Some(last_control_update_at);
        self.write_if_due(Instant::now(), true);
    }

    fn write_if_due(&mut self, now: Instant, force: bool) {
        let due = force
            || self
                .last_write_at
                .map(|last_write| now.duration_since(last_write) >= Duration::from_millis(250))
                .unwrap_or(true);
        if !due {
            return;
        }

        if let Err(error) = write_preview_telemetry_heartbeat(&self.path, &self.heartbeat(now)) {
            self.last_error = Some(error);
        }
        self.last_write_at = Some(now);
    }

    fn heartbeat(&self, now: Instant) -> PreviewTelemetryHeartbeat {
        let elapsed = self
            .first_frame_at
            .map(|first_frame| now.duration_since(first_frame).as_secs_f64())
            .unwrap_or_default();
        let approx_fps = if self.frames_rendered > 1 && elapsed > 0.0 {
            Some(round_1(self.frames_rendered as f64 / elapsed))
        } else {
            None
        };

        PreviewTelemetryHeartbeat {
            schema: PREVIEW_TELEMETRY_SCHEMA.to_owned(),
            schema_version: PREVIEW_TELEMETRY_SCHEMA_VERSION.to_owned(),
            timestamp: unix_ms_timestamp(),
            pid: std::process::id(),
            graph_id: self.graph_id.clone(),
            graph_revision: self.graph_revision.clone(),
            session_revision: self.session_revision,
            renderer: self.renderer.clone(),
            backend: self.backend.clone(),
            frames_rendered: self.frames_rendered,
            approx_fps,
            last_frame_ms: self.last_frame_ms.map(round_1),
            last_error: self.last_error.clone(),
            source_node_id: self.source_node_id.clone(),
            diagnostics: self.diagnostics.clone(),
            generated_source_available: self.generated_source_available,
            control_revision: self.control_revision,
            preview_control_revision: self.preview_control_revision,
            control_live: self.control_live,
            last_control_update_at: self.last_control_update_at.clone(),
        }
    }
}

pub fn preview_telemetry_path(session_revision: u64) -> PathBuf {
    let directory = std::env::temp_dir().join("skenion-runtime-preview");
    let nonce = PREVIEW_TELEMETRY_COUNTER.fetch_add(1, Ordering::Relaxed);
    directory.join(format!(
        "preview-{}-{}-{}-telemetry.json",
        std::process::id(),
        nonce,
        session_revision
    ))
}

pub fn read_preview_telemetry(path: &Path) -> Result<Option<PreviewTelemetryHeartbeat>, String> {
    let Some(bytes) = read_preview_telemetry_bytes(path).map_err(|error| error.to_string())? else {
        return Ok(None);
    };

    match serde_json::from_slice(&bytes) {
        Ok(heartbeat) => Ok(Some(heartbeat)),
        Err(error) => Err(error.to_string()),
    }
}

pub fn write_preview_telemetry_heartbeat(
    path: &Path,
    heartbeat: &PreviewTelemetryHeartbeat,
) -> Result<(), String> {
    write_preview_telemetry_heartbeat_io(path, heartbeat).map_err(|error| error.to_string())
}

fn read_preview_telemetry_bytes(path: &Path) -> io::Result<Option<Vec<u8>>> {
    if !path.exists() {
        return Ok(None);
    }

    fs::read(path).map(Some)
}

fn write_preview_telemetry_heartbeat_io(
    path: &Path,
    heartbeat: &PreviewTelemetryHeartbeat,
) -> io::Result<()> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(fs::create_dir_all)
        .transpose()?;
    let temp_path = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(heartbeat)
        .expect("preview telemetry heartbeat should always serialize");
    fs::write(&temp_path, bytes)?;
    fs::rename(&temp_path, path)
}

pub fn unix_ms_timestamp() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("unix-ms:{millis}")
}

fn render_from_preview(
    preview: &RuntimePreviewStatusResponse,
    heartbeat: Option<PreviewTelemetryHeartbeat>,
    dry_run: bool,
    diagnostics: &[RuntimeDiagnostic],
) -> RuntimeTelemetryRender {
    let active = matches!(
        preview.state,
        PreviewState::Starting | PreviewState::Running
    );
    if dry_run && active {
        let renderer = heartbeat
            .as_ref()
            .map(|heartbeat| heartbeat.renderer.clone())
            .unwrap_or_else(|| "none".to_owned());
        let source_node_id = heartbeat
            .as_ref()
            .and_then(|heartbeat| heartbeat.source_node_id.clone());
        let last_error = heartbeat
            .as_ref()
            .and_then(|heartbeat| heartbeat.last_error.clone())
            .or_else(|| preview.message.clone());
        return RuntimeTelemetryRender {
            active,
            backend: Some("dry-run".to_owned()),
            renderer: Some(renderer),
            frames_rendered: 0,
            approx_fps: None,
            last_frame_ms: None,
            last_error,
            source_node_id,
            diagnostics: heartbeat
                .as_ref()
                .map(|heartbeat| heartbeat.diagnostics.clone())
                .unwrap_or_default(),
            generated_source_available: heartbeat
                .as_ref()
                .is_some_and(|heartbeat| heartbeat.generated_source_available),
            control_revision: heartbeat
                .as_ref()
                .and_then(|heartbeat| heartbeat.control_revision)
                .or(preview.control_revision),
            preview_control_revision: heartbeat
                .as_ref()
                .and_then(|heartbeat| heartbeat.preview_control_revision)
                .or(preview.preview_control_revision),
            control_live: heartbeat
                .as_ref()
                .map(|heartbeat| heartbeat.control_live)
                .unwrap_or(preview.control_live),
            last_control_update_at: heartbeat
                .as_ref()
                .and_then(|heartbeat| heartbeat.last_control_update_at.clone())
                .or_else(|| preview.last_control_update_at.clone()),
        };
    }

    if let Some(heartbeat) = heartbeat {
        let control_revision = heartbeat.control_revision.or(preview.control_revision);
        let preview_control_revision = heartbeat
            .preview_control_revision
            .or(preview.preview_control_revision);
        let control_live = heartbeat.control_live || preview.control_live;
        let last_control_update_at = heartbeat
            .last_control_update_at
            .or_else(|| preview.last_control_update_at.clone());
        return RuntimeTelemetryRender {
            active,
            backend: Some(heartbeat.backend),
            renderer: Some(heartbeat.renderer),
            frames_rendered: heartbeat.frames_rendered,
            approx_fps: heartbeat.approx_fps,
            last_frame_ms: heartbeat.last_frame_ms,
            last_error: heartbeat.last_error.or_else(|| preview.message.clone()),
            source_node_id: heartbeat.source_node_id,
            diagnostics: heartbeat.diagnostics,
            generated_source_available: heartbeat.generated_source_available,
            control_revision,
            preview_control_revision,
            control_live,
            last_control_update_at,
        };
    }

    RuntimeTelemetryRender {
        active,
        backend: None,
        renderer: None,
        frames_rendered: 0,
        approx_fps: None,
        last_frame_ms: None,
        last_error: diagnostics
            .first()
            .map(|diagnostic| diagnostic.message.clone())
            .or_else(|| preview.message.clone()),
        source_node_id: None,
        diagnostics: Vec::new(),
        generated_source_available: false,
        control_revision: preview.control_revision,
        preview_control_revision: preview.preview_control_revision,
        control_live: preview.control_live,
        last_control_update_at: preview.last_control_update_at.clone(),
    }
}

fn round_1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        DiagnosticSeverity, GraphDocument, GraphNode, RuntimeDiagnostic, RuntimeProjectSnapshot,
        create_default_view_state_for_graph,
    };

    #[test]
    fn writes_and_reads_preview_heartbeat_atomically() {
        let path = std::env::temp_dir().join(format!(
            "skenion-preview-telemetry-test-{}.json",
            std::process::id()
        ));
        let heartbeat = heartbeat();

        write_preview_telemetry_heartbeat(&path, &heartbeat).expect("heartbeat should write");
        let decoded = read_preview_telemetry(&path)
            .expect("heartbeat should read")
            .expect("heartbeat should exist");

        assert_eq!(decoded, heartbeat);
        std::fs::remove_file(path).expect("heartbeat should be removable");
    }

    #[test]
    fn preview_telemetry_path_uses_runtime_preview_directory() {
        let path = preview_telemetry_path(77);
        let text = path.to_string_lossy();

        assert!(text.contains("skenion-runtime-preview"));
        assert!(text.contains("77-telemetry.json"));
    }

    #[test]
    fn write_preview_telemetry_creates_nested_parent_directory() {
        let directory = std::env::temp_dir().join(format!(
            "skenion-preview-telemetry-nested-{}",
            std::process::id()
        ));
        let path = directory.join("child").join("telemetry.json");

        write_preview_telemetry_heartbeat(&path, &heartbeat()).expect("heartbeat should write");

        assert!(path.exists());
        std::fs::remove_dir_all(directory).expect("nested heartbeat dir should be removable");
    }

    #[test]
    fn read_preview_telemetry_tolerates_missing_file() {
        let path = std::env::temp_dir().join("skenion-preview-telemetry-missing.json");

        assert_eq!(
            read_preview_telemetry(&path).expect("read should work"),
            None
        );
    }

    #[test]
    fn read_preview_telemetry_reports_invalid_json() {
        let path = std::env::temp_dir().join(format!(
            "skenion-preview-telemetry-invalid-{}.json",
            std::process::id()
        ));
        std::fs::write(&path, b"{").expect("invalid heartbeat should write");

        let error = read_preview_telemetry(&path).expect_err("invalid heartbeat should fail");

        assert!(error.contains("EOF"));
        std::fs::remove_file(path).expect("heartbeat should be removable");
    }

    #[test]
    fn read_preview_telemetry_reports_read_errors() {
        let directory = std::env::temp_dir().join(format!(
            "skenion-preview-telemetry-read-error-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).expect("directory should be created");

        let error = read_preview_telemetry(&directory).expect_err("directory read should fail");

        assert!(!error.is_empty());
        std::fs::remove_dir_all(directory).expect("directory should be removable");
    }

    #[test]
    fn runtime_snapshot_uses_dry_run_render_state() {
        let snapshot = RuntimeTelemetrySnapshot::from_parts(
            session_snapshot(true),
            preview_status(PreviewState::Running),
            None,
            true,
            42,
            Vec::new(),
        );

        assert_eq!(snapshot.schema, TELEMETRY_SCHEMA);
        assert!(snapshot.session.loaded);
        assert_eq!(snapshot.preview.state, PreviewState::Running);
        assert!(snapshot.render.active);
        assert_eq!(snapshot.render.backend.as_deref(), Some("dry-run"));
        assert_eq!(snapshot.process.uptime_ms, 42);
    }

    #[test]
    fn runtime_snapshot_uses_native_heartbeat() {
        let snapshot = RuntimeTelemetrySnapshot::from_parts(
            session_snapshot(true),
            preview_status(PreviewState::Running),
            Some(heartbeat()),
            false,
            12,
            Vec::new(),
        );

        assert_eq!(snapshot.render.backend.as_deref(), Some("wgpu"));
        assert_eq!(snapshot.render.renderer.as_deref(), Some("clear-color"));
        assert_eq!(snapshot.render.frames_rendered, 10);
        assert_eq!(snapshot.render.source_node_id.as_deref(), Some("clear_1"));
    }

    #[test]
    fn runtime_snapshot_falls_back_to_preview_control_metadata_for_native_heartbeat() {
        let mut heartbeat = heartbeat();
        heartbeat.control_live = false;
        heartbeat.last_control_update_at = None;

        let snapshot = RuntimeTelemetrySnapshot::from_parts(
            session_snapshot(true),
            preview_status(PreviewState::Running),
            Some(heartbeat),
            false,
            12,
            Vec::new(),
        );

        assert!(snapshot.render.control_live);
        assert_eq!(
            snapshot.render.last_control_update_at.as_deref(),
            Some("unix-ms:2")
        );
    }

    #[test]
    fn runtime_snapshot_keeps_invalid_heartbeat_diagnostic_nonfatal() {
        let diagnostic = RuntimeDiagnostic {
            severity: DiagnosticSeverity::Warning,
            message: "invalid preview telemetry heartbeat: expected value".to_owned(),
            code: None,
            details: None,
        };
        let snapshot = RuntimeTelemetrySnapshot::from_parts(
            session_snapshot(false),
            preview_status(PreviewState::Stopped),
            None,
            false,
            0,
            vec![diagnostic],
        );

        assert!(snapshot.ok);
        assert_eq!(
            snapshot.diagnostics[0].severity,
            DiagnosticSeverity::Warning
        );
        assert_eq!(
            snapshot.render.last_error.as_deref(),
            Some("invalid preview telemetry heartbeat: expected value")
        );
    }

    #[test]
    fn preview_telemetry_writer_writes_first_frame() {
        let path = std::env::temp_dir().join(format!(
            "skenion-preview-telemetry-writer-{}.json",
            std::process::id()
        ));
        let mut writer = PreviewTelemetryWriter::new(
            path.clone(),
            "graph".to_owned(),
            "1".to_owned(),
            2,
            "clear-color",
            "wgpu",
            Some("clear_1".to_owned()),
        );
        writer.record_frame(16.67);
        let decoded = read_preview_telemetry(&path)
            .expect("heartbeat should read")
            .expect("heartbeat should exist");

        assert_eq!(decoded.frames_rendered, 1);
        assert_eq!(decoded.last_frame_ms, Some(16.7));
        assert_eq!(decoded.source_node_id.as_deref(), Some("clear_1"));
        std::fs::remove_file(path).expect("heartbeat should be removable");
    }

    #[test]
    fn preview_telemetry_writer_records_control_revision() {
        let path = std::env::temp_dir().join(format!(
            "skenion-preview-telemetry-control-revision-{}.json",
            std::process::id()
        ));
        let mut writer = test_writer(path.clone());

        writer.record_control_revision(2, "unix-ms:2".to_owned());
        let decoded = read_preview_telemetry(&path)
            .expect("heartbeat should read")
            .expect("heartbeat should exist");

        assert_eq!(decoded.control_revision, Some(2));
        assert_eq!(decoded.preview_control_revision, Some(2));
        assert!(decoded.control_live);
        assert_eq!(decoded.last_control_update_at.as_deref(), Some("unix-ms:2"));
        std::fs::remove_file(path).expect("heartbeat should be removable");
    }

    #[test]
    fn preview_telemetry_writer_rate_limits_non_forced_writes() {
        let path = std::env::temp_dir().join(format!(
            "skenion-preview-telemetry-rate-limit-{}.json",
            std::process::id()
        ));
        let mut writer = test_writer(path.clone());

        writer.record_frame(16.0);
        writer.record_frame(17.0);
        let decoded = read_preview_telemetry(&path)
            .expect("heartbeat should read")
            .expect("heartbeat should exist");

        assert_eq!(decoded.frames_rendered, 1);
        assert_eq!(decoded.last_frame_ms, Some(16.0));
        std::fs::remove_file(path).expect("heartbeat should be removable");
    }

    #[test]
    fn preview_telemetry_writer_forces_error_heartbeat() {
        let path = std::env::temp_dir().join(format!(
            "skenion-preview-telemetry-error-{}.json",
            std::process::id()
        ));
        let mut writer = test_writer(path.clone());

        writer.record_error("surface lost");
        let decoded = read_preview_telemetry(&path)
            .expect("heartbeat should read")
            .expect("heartbeat should exist");

        assert_eq!(decoded.frames_rendered, 0);
        assert_eq!(decoded.last_error.as_deref(), Some("surface lost"));
        assert_eq!(
            decoded.diagnostics[0].phase,
            ShaderDiagnosticPhase::RenderFrame
        );
        std::fs::remove_file(path).expect("heartbeat should be removable");
    }

    #[test]
    fn preview_telemetry_writer_records_shader_diagnostics() {
        let path = std::env::temp_dir().join(format!(
            "skenion-preview-telemetry-shader-diagnostic-{}.json",
            std::process::id()
        ));
        let mut writer = test_writer(path.clone());

        writer.record_shader_diagnostic(ShaderDiagnostic {
            severity: ShaderDiagnosticSeverity::Warning,
            phase: ShaderDiagnosticPhase::InterfaceAnalysis,
            code: "unused-uniform".to_owned(),
            message: "uniform tint is not referenced".to_owned(),
            line: Some(2),
            column: Some(5),
            end_line: None,
            end_column: None,
            uniform_id: Some("tint".to_owned()),
            source: ShaderDiagnosticSource::User,
        });
        let warning = read_preview_telemetry(&path)
            .expect("heartbeat should read")
            .expect("heartbeat should exist");
        assert_eq!(warning.last_error, None);
        assert_eq!(
            warning.diagnostics[0].severity,
            ShaderDiagnosticSeverity::Warning
        );

        writer.record_shader_diagnostic(ShaderDiagnostic::error(
            ShaderDiagnosticPhase::WgslCompile,
            "wgsl-validation",
            "invalid shader module",
            ShaderDiagnosticSource::Generated,
        ));
        let error = read_preview_telemetry(&path)
            .expect("heartbeat should read")
            .expect("heartbeat should exist");
        assert_eq!(error.last_error.as_deref(), Some("invalid shader module"));
        assert_eq!(error.diagnostics.len(), 2);
        assert_eq!(
            error.diagnostics[1].phase,
            ShaderDiagnosticPhase::WgslCompile
        );
        std::fs::remove_file(path).expect("heartbeat should be removable");
    }

    #[test]
    fn preview_telemetry_writer_keeps_write_errors_as_last_error() {
        let blocker = std::env::temp_dir().join(format!(
            "skenion-preview-telemetry-blocker-{}",
            std::process::id()
        ));
        std::fs::write(&blocker, b"not a directory").expect("blocker should write");
        let path = blocker.join("telemetry.json");
        let mut writer = test_writer(path);

        writer.record_frame(16.0);

        assert!(writer.last_error.is_some());
        std::fs::remove_file(blocker).expect("blocker should be removable");
    }

    #[test]
    fn preview_telemetry_writer_calculates_approximate_fps() {
        let path = std::env::temp_dir().join(format!(
            "skenion-preview-telemetry-fps-{}.json",
            std::process::id()
        ));
        let mut writer = test_writer(path);
        let now = Instant::now();
        writer.frames_rendered = 2;
        writer.first_frame_at = Some(now - Duration::from_secs(1));

        let heartbeat = writer.heartbeat(now);

        assert_eq!(heartbeat.approx_fps, Some(2.0));
    }

    fn heartbeat() -> PreviewTelemetryHeartbeat {
        PreviewTelemetryHeartbeat {
            schema: PREVIEW_TELEMETRY_SCHEMA.to_owned(),
            schema_version: PREVIEW_TELEMETRY_SCHEMA_VERSION.to_owned(),
            timestamp: "unix-ms:1".to_owned(),
            pid: 123,
            graph_id: "clear-color-render".to_owned(),
            graph_revision: "2".to_owned(),
            session_revision: 5,
            renderer: "clear-color".to_owned(),
            backend: "wgpu".to_owned(),
            frames_rendered: 10,
            approx_fps: Some(59.8),
            last_frame_ms: Some(16.7),
            last_error: None,
            source_node_id: Some("clear_1".to_owned()),
            diagnostics: vec![ShaderDiagnostic::error(
                ShaderDiagnosticPhase::RenderFrame,
                "test-diagnostic",
                "test diagnostic",
                ShaderDiagnosticSource::Runtime,
            )],
            generated_source_available: false,
            control_revision: Some(7),
            preview_control_revision: Some(7),
            control_live: true,
            last_control_update_at: Some("unix-ms:2".to_owned()),
        }
    }

    fn test_writer(path: PathBuf) -> PreviewTelemetryWriter {
        PreviewTelemetryWriter::new(
            path,
            "graph".to_owned(),
            "1".to_owned(),
            2,
            "clear-color",
            "wgpu",
            Some("clear_1".to_owned()),
        )
    }

    fn session_snapshot(loaded: bool) -> RuntimeSessionSnapshot {
        let graph = loaded.then(|| GraphDocument {
            schema: "skenion.graph".to_owned(),
            schema_version: "0.1.0".to_owned(),
            id: "clear-color-render".to_owned(),
            revision: "2".to_owned(),
            nodes: vec![GraphNode {
                id: "clear_1".to_owned(),
                kind: "render.clear-color".to_owned(),
                kind_version: "0.1.0".to_owned(),
                params: serde_json::Map::new(),
                ports: Vec::new(),
            }],
            edges: Vec::new(),
        });
        let project = graph.map(|graph| RuntimeProjectSnapshot {
            view_state: create_default_view_state_for_graph(&graph),
            graph,
            nodes: Vec::new(),
        });
        RuntimeSessionSnapshot {
            session_revision: if loaded { 5 } else { 0 },
            view_revision: if loaded { 1 } else { 0 },
            control_revision: if loaded { 7 } else { 0 },
            project,
            diagnostics: Vec::new(),
            plan: None,
        }
    }

    fn preview_status(state: PreviewState) -> RuntimePreviewStatusResponse {
        let active = state != PreviewState::Stopped;
        RuntimePreviewStatusResponse {
            ok: true,
            state,
            pid: active.then_some(123),
            graph_id: active.then(|| "clear-color-render".to_owned()),
            graph_revision: active.then(|| "2".to_owned()),
            session_revision: active.then_some(5),
            preview_session_revision: active.then_some(5),
            control_revision: active.then_some(7),
            preview_control_revision: active.then_some(7),
            control_live: active,
            last_control_update_at: active.then(|| "unix-ms:2".to_owned()),
            stale: false,
            started_at: active.then(|| "unix-ms:1".to_owned()),
            exited_at: None,
            exit_code: None,
            message: None,
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn preview_heartbeat_json_shape_is_camel_case() {
        let value = serde_json::to_value(heartbeat()).expect("heartbeat should serialize");

        assert_eq!(value["schema"], json!(PREVIEW_TELEMETRY_SCHEMA));
        assert_eq!(
            value["schemaVersion"],
            json!(PREVIEW_TELEMETRY_SCHEMA_VERSION)
        );
        assert_eq!(value["framesRendered"], json!(10));
        assert_eq!(value["sourceNodeId"], json!("clear_1"));
        assert_eq!(value["diagnostics"][0]["phase"], json!("render-frame"));
        assert_eq!(value["generatedSourceAvailable"], json!(false));
    }
}
