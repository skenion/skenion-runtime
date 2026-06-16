use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    ExecutionPlan, GraphDocument, PreviewDocument, RuntimeDiagnostic, RuntimeSessionSnapshot,
    RuntimeTelemetrySnapshot,
    render::render_scene_from_preview_document,
    telemetry::{
        PREVIEW_TELEMETRY_SCHEMA, PREVIEW_TELEMETRY_SCHEMA_VERSION, PreviewTelemetryHeartbeat,
        preview_telemetry_path, read_preview_telemetry, unix_ms_timestamp,
        write_preview_telemetry_heartbeat,
    },
};

pub(crate) trait PreviewHandle: Send {
    fn pid(&self) -> Option<u32>;
    fn try_wait(&mut self) -> Result<Option<i32>, String>;
    fn stop(&mut self) -> Result<Option<i32>, String>;
}

pub(crate) type PreviewSpawner =
    fn(&PreviewDocument, &Path) -> Result<Box<dyn PreviewHandle>, String>;

#[derive(Debug, Clone, PartialEq)]
pub struct PreviewContext {
    pub graph_id: String,
    pub graph_revision: String,
    pub session_revision: u64,
    pub graph: GraphDocument,
    pub plan: ExecutionPlan,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewState {
    Stopped,
    Starting,
    Running,
    Exited,
    Error,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePreviewStartRequest {
    #[serde(default)]
    pub restart: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePreviewStatusResponse {
    pub ok: bool,
    pub state: PreviewState,
    pub pid: Option<u32>,
    pub graph_id: Option<String>,
    pub graph_revision: Option<String>,
    pub session_revision: Option<u64>,
    pub preview_session_revision: Option<u64>,
    pub stale: bool,
    pub started_at: Option<String>,
    pub exited_at: Option<String>,
    pub exit_code: Option<i32>,
    pub message: Option<String>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Clone)]
struct PreviewStatus {
    state: PreviewState,
    pid: Option<u32>,
    graph_id: Option<String>,
    graph_revision: Option<String>,
    preview_session_revision: Option<u64>,
    started_at: Option<String>,
    exited_at: Option<String>,
    exit_code: Option<i32>,
    message: Option<String>,
    telemetry_path: Option<PathBuf>,
}

pub struct PreviewManager {
    status: PreviewStatus,
    handle: Option<Box<dyn PreviewHandle>>,
    dry_run: bool,
    spawner: PreviewSpawner,
}

impl Default for PreviewManager {
    fn default() -> Self {
        Self::from_env()
    }
}

impl PreviewManager {
    pub fn from_env() -> Self {
        Self::with_spawner(
            dry_run_enabled(std::env::var("SKENION_PREVIEW_DRY_RUN").ok().as_deref()),
            crate::visual::spawn_preview_document_handle,
        )
    }

    pub fn dry_run() -> Self {
        Self::with_spawner(true, crate::visual::spawn_preview_document_handle)
    }

    pub fn status(&mut self, snapshot: RuntimeSessionSnapshot) -> RuntimePreviewStatusResponse {
        self.poll();
        self.to_response(true, &snapshot, Vec::new())
    }

    pub fn telemetry(
        &mut self,
        snapshot: RuntimeSessionSnapshot,
        uptime_ms: u64,
    ) -> RuntimeTelemetrySnapshot {
        self.poll();
        let mut diagnostics = Vec::new();
        let heartbeat = match self.status.telemetry_path.as_deref() {
            Some(path) => match read_preview_telemetry(path) {
                Ok(heartbeat) => heartbeat,
                Err(error) => {
                    diagnostics.push(RuntimeDiagnostic::warning(format!(
                        "invalid preview telemetry heartbeat: {error}"
                    )));
                    None
                }
            },
            None => None,
        };

        RuntimeTelemetrySnapshot::from_parts(
            snapshot.clone(),
            self.to_response(true, &snapshot, Vec::new()),
            heartbeat,
            self.dry_run,
            uptime_ms,
            diagnostics,
        )
    }

    pub fn start(
        &mut self,
        context: Result<PreviewContext, Vec<RuntimeDiagnostic>>,
        snapshot: RuntimeSessionSnapshot,
        restart: bool,
    ) -> RuntimePreviewStatusResponse {
        self.poll();
        if self.is_active() && !restart {
            return self.to_response(true, &snapshot, Vec::new());
        }
        if restart {
            let _ = self.stop_current();
        }

        let context = match context {
            Ok(context) => context,
            Err(diagnostics) => return self.to_response(false, &snapshot, diagnostics),
        };

        self.status = PreviewStatus {
            state: PreviewState::Starting,
            pid: None,
            graph_id: Some(context.graph_id.clone()),
            graph_revision: Some(context.graph_revision.clone()),
            preview_session_revision: Some(context.session_revision),
            started_at: Some(now_string()),
            exited_at: None,
            exit_code: None,
            message: None,
            telemetry_path: Some(preview_telemetry_path(context.session_revision)),
        };

        let document = PreviewDocument::new(
            context.graph.clone(),
            context.plan.clone(),
            context.session_revision,
        );
        let handle = if self.dry_run {
            record_dry_run_heartbeat(&mut self.status, &document);
            Ok(Box::new(DryRunPreviewHandle) as Box<dyn PreviewHandle>)
        } else {
            let telemetry_path = self
                .status
                .telemetry_path
                .as_deref()
                .expect("preview telemetry path should be prepared before spawning");
            (self.spawner)(&document, telemetry_path)
        };

        match handle {
            Ok(handle) => {
                self.status.pid = handle.pid();
                self.status.state = PreviewState::Running;
                self.handle = Some(handle);
                self.to_response(true, &snapshot, Vec::new())
            }
            Err(error) => {
                self.status.state = PreviewState::Error;
                self.status.message = Some(error.clone());
                self.handle = None;
                self.to_response(false, &snapshot, vec![RuntimeDiagnostic::error(error)])
            }
        }
    }

    pub fn restart(
        &mut self,
        context: Result<PreviewContext, Vec<RuntimeDiagnostic>>,
        snapshot: RuntimeSessionSnapshot,
    ) -> RuntimePreviewStatusResponse {
        self.start(context, snapshot, true)
    }

    pub fn stop(&mut self, snapshot: RuntimeSessionSnapshot) -> RuntimePreviewStatusResponse {
        match self.stop_current() {
            Ok(()) => self.to_response(true, &snapshot, Vec::new()),
            Err(error) => self.to_response(false, &snapshot, vec![RuntimeDiagnostic::error(error)]),
        }
    }

    pub fn request_error(
        &self,
        snapshot: RuntimeSessionSnapshot,
        diagnostic: RuntimeDiagnostic,
    ) -> RuntimePreviewStatusResponse {
        self.to_response(false, &snapshot, vec![diagnostic])
    }

    fn is_active(&self) -> bool {
        matches!(
            self.status.state,
            PreviewState::Starting | PreviewState::Running
        )
    }

    fn poll(&mut self) {
        let Some(handle) = self.handle.as_mut() else {
            return;
        };
        match handle.try_wait() {
            Ok(Some(code)) => {
                self.status.state = PreviewState::Exited;
                self.status.pid = None;
                self.status.exited_at = Some(now_string());
                self.status.exit_code = Some(code);
                self.status.message = Some("preview process exited".to_owned());
                self.handle = None;
            }
            Ok(None) => {
                if self.status.state == PreviewState::Starting {
                    self.status.state = PreviewState::Running;
                }
            }
            Err(error) => {
                self.status.state = PreviewState::Error;
                self.status.pid = None;
                self.status.exited_at = Some(now_string());
                self.status.message = Some(error);
                self.handle = None;
            }
        }
    }

    fn stop_current(&mut self) -> Result<(), String> {
        self.poll();
        if let Some(mut handle) = self.handle.take() {
            let exit_code = match handle.stop() {
                Ok(exit_code) => exit_code,
                Err(error) => {
                    self.status.state = PreviewState::Error;
                    self.status.pid = None;
                    self.status.exited_at = Some(now_string());
                    self.status.message = Some(error.clone());
                    return Err(error);
                }
            };
            self.status.exit_code = exit_code;
            self.status.exited_at = Some(now_string());
        }
        self.status = PreviewStatus::stopped();
        Ok(())
    }

    fn to_response(
        &self,
        ok: bool,
        snapshot: &RuntimeSessionSnapshot,
        diagnostics: Vec<RuntimeDiagnostic>,
    ) -> RuntimePreviewStatusResponse {
        let session_revision = snapshot.loaded.then_some(snapshot.session_revision);
        let stale = self.status.state != PreviewState::Stopped
            && session_revision
                .zip(self.status.preview_session_revision)
                .is_some_and(|(session_revision, preview_revision)| {
                    session_revision != preview_revision
                });

        RuntimePreviewStatusResponse {
            ok,
            state: self.status.state.clone(),
            pid: self.status.pid,
            graph_id: self.status.graph_id.clone(),
            graph_revision: self.status.graph_revision.clone(),
            session_revision,
            preview_session_revision: self.status.preview_session_revision,
            stale,
            started_at: self.status.started_at.clone(),
            exited_at: self.status.exited_at.clone(),
            exit_code: self.status.exit_code,
            message: self.status.message.clone(),
            diagnostics,
        }
    }

    #[cfg(test)]
    fn with_test_spawner(dry_run: bool, spawner: PreviewSpawner) -> Self {
        Self::with_spawner(dry_run, spawner)
    }

    fn with_spawner(dry_run: bool, spawner: PreviewSpawner) -> Self {
        Self {
            status: PreviewStatus::stopped(),
            handle: None,
            dry_run,
            spawner,
        }
    }
}

impl PreviewStatus {
    fn stopped() -> Self {
        Self {
            state: PreviewState::Stopped,
            pid: None,
            graph_id: None,
            graph_revision: None,
            preview_session_revision: None,
            started_at: None,
            exited_at: None,
            exit_code: None,
            message: None,
            telemetry_path: None,
        }
    }
}

struct DryRunPreviewHandle;

impl PreviewHandle for DryRunPreviewHandle {
    fn pid(&self) -> Option<u32> {
        None
    }

    fn try_wait(&mut self) -> Result<Option<i32>, String> {
        Ok(None)
    }

    fn stop(&mut self) -> Result<Option<i32>, String> {
        Ok(Some(0))
    }
}

fn dry_run_enabled(value: Option<&str>) -> bool {
    value.is_some_and(|value| matches!(value, "1" | "true" | "TRUE" | "yes" | "YES"))
}

fn now_string() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("unix-ms:{millis}")
}

fn dry_run_heartbeat(document: &PreviewDocument) -> PreviewTelemetryHeartbeat {
    let scene = render_scene_from_preview_document(document);
    let (renderer, source_node_id, last_error) = match scene {
        Ok(scene) => (
            scene.renderer_label().to_owned(),
            scene.source_node_id(),
            None,
        ),
        Err(error) => ("clear-color".to_owned(), None, Some(error.to_string())),
    };

    PreviewTelemetryHeartbeat {
        schema: PREVIEW_TELEMETRY_SCHEMA.to_owned(),
        schema_version: PREVIEW_TELEMETRY_SCHEMA_VERSION.to_owned(),
        timestamp: unix_ms_timestamp(),
        pid: std::process::id(),
        graph_id: document.graph.id.clone(),
        graph_revision: document.graph.revision.clone(),
        session_revision: document.session_revision,
        renderer,
        backend: "dry-run".to_owned(),
        frames_rendered: 0,
        approx_fps: None,
        last_frame_ms: None,
        last_error,
        source_node_id,
    }
}

fn record_dry_run_heartbeat(status: &mut PreviewStatus, document: &PreviewDocument) {
    let telemetry_path = status
        .telemetry_path
        .as_deref()
        .expect("preview telemetry path should be prepared before dry-run start");
    let heartbeat = dry_run_heartbeat(document);
    if let Err(error) = write_preview_telemetry_heartbeat(telemetry_path, &heartbeat) {
        status.message = Some(format!(
            "failed to write dry-run preview telemetry: {error}"
        ));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        ExecutionGroup, ExecutionModel, GraphDocument, GraphNode, PREVIEW_TELEMETRY_SCHEMA,
        PREVIEW_TELEMETRY_SCHEMA_VERSION, PlanEdge, PlanNode, PreviewTelemetryHeartbeat,
        RuntimeSessionSnapshot, preview_manager::PreviewHandle,
        telemetry::write_preview_telemetry_heartbeat,
    };

    #[test]
    fn dry_run_detection_accepts_explicit_truthy_values() {
        assert!(dry_run_enabled(Some("1")));
        assert!(dry_run_enabled(Some("true")));
        assert!(dry_run_enabled(Some("TRUE")));
        assert!(dry_run_enabled(Some("yes")));
        assert!(dry_run_enabled(Some("YES")));
        assert!(!dry_run_enabled(Some("0")));
        assert!(!dry_run_enabled(Some("false")));
        assert!(!dry_run_enabled(None));
    }

    #[test]
    fn status_starts_stopped_without_loaded_session() {
        let mut manager = PreviewManager::dry_run();
        let response = manager.status(empty_snapshot());

        assert!(response.ok);
        assert_eq!(response.state, PreviewState::Stopped);
        assert_eq!(response.session_revision, None);
        assert!(!response.stale);
    }

    #[test]
    fn default_manager_reports_stopped_without_starting_process() {
        let mut manager = PreviewManager::default();
        let response = manager.status(empty_snapshot());

        assert!(response.ok);
        assert_eq!(response.state, PreviewState::Stopped);
    }

    #[test]
    fn dry_run_start_runs_without_pid_and_reports_preview_context() {
        let mut manager = PreviewManager::dry_run();
        let response = manager.start(Ok(context(1)), loaded_snapshot(1, "1"), false);

        assert!(response.ok);
        assert_eq!(response.state, PreviewState::Running);
        assert_eq!(response.pid, None);
        assert_eq!(response.graph_id.as_deref(), Some("minimal-value"));
        assert_eq!(response.graph_revision.as_deref(), Some("1"));
        assert_eq!(response.session_revision, Some(1));
        assert_eq!(response.preview_session_revision, Some(1));
        assert!(response.started_at.unwrap().starts_with("unix-ms:"));
        assert!(!response.stale);
    }

    #[test]
    fn dry_run_telemetry_reports_active_render_with_synthetic_heartbeat() {
        let mut manager = PreviewManager::dry_run();
        manager.start(Ok(context(1)), loaded_snapshot(1, "1"), false);

        let telemetry = manager.telemetry(loaded_snapshot(1, "1"), 120);

        assert!(telemetry.ok);
        assert_eq!(telemetry.preview.state, PreviewState::Running);
        assert!(telemetry.render.active);
        assert_eq!(telemetry.render.backend.as_deref(), Some("dry-run"));
        assert_eq!(telemetry.render.renderer.as_deref(), Some("clear-color"));
        assert_eq!(telemetry.process.uptime_ms, 120);
    }

    #[test]
    fn dry_run_heartbeat_write_failure_is_reported_on_status() {
        let blocker = std::env::temp_dir().join(format!(
            "skenion-dry-run-heartbeat-blocker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&blocker, b"blocker").expect("blocker should write");
        let mut status = PreviewStatus {
            state: PreviewState::Starting,
            pid: None,
            graph_id: Some("minimal-value".to_owned()),
            graph_revision: Some("1".to_owned()),
            preview_session_revision: Some(1),
            started_at: None,
            exited_at: None,
            exit_code: None,
            message: None,
            telemetry_path: Some(blocker.join("telemetry.json")),
        };
        let document = PreviewDocument::new(graph("1"), plan("1"), 1);

        record_dry_run_heartbeat(&mut status, &document);

        assert!(
            status
                .message
                .as_deref()
                .unwrap()
                .contains("failed to write dry-run preview telemetry")
        );
        std::fs::remove_file(blocker).expect("blocker should remove");
    }

    #[test]
    fn dry_run_heartbeat_reports_fullscreen_shader_renderer() {
        let document = PreviewDocument::new(shader_graph("7"), shader_plan("7"), 7);

        let heartbeat = dry_run_heartbeat(&document);

        assert_eq!(heartbeat.backend, "dry-run");
        assert_eq!(heartbeat.renderer, "fullscreen-shader");
        assert_eq!(heartbeat.source_node_id.as_deref(), Some("shader_1"));
        assert_eq!(heartbeat.graph_revision, "7");
    }

    #[test]
    fn dry_run_heartbeat_reports_scene_errors() {
        let mut graph = shader_graph("8");
        graph.nodes[0]
            .params
            .insert("language".to_owned(), json!("glsl"));
        let document = PreviewDocument::new(graph, shader_plan("8"), 8);

        let heartbeat = dry_run_heartbeat(&document);

        assert_eq!(heartbeat.renderer, "clear-color");
        assert_eq!(heartbeat.backend, "dry-run");
        assert!(
            heartbeat
                .last_error
                .as_deref()
                .unwrap()
                .contains("unsupported language glsl")
        );
    }

    #[test]
    fn start_without_context_returns_diagnostics() {
        let mut manager = PreviewManager::dry_run();
        let response = manager.start(
            Err(vec![RuntimeDiagnostic::error("no project loaded")]),
            empty_snapshot(),
            false,
        );

        assert!(!response.ok);
        assert_eq!(response.state, PreviewState::Stopped);
        assert!(
            response.diagnostics[0]
                .message
                .contains("no project loaded")
        );
    }

    #[test]
    fn second_start_without_restart_keeps_current_preview() {
        let mut manager = PreviewManager::dry_run();
        manager.start(Ok(context(1)), loaded_snapshot(1, "1"), false);

        let response = manager.start(Ok(context(2)), loaded_snapshot(2, "2"), false);

        assert!(response.ok);
        assert_eq!(response.state, PreviewState::Running);
        assert_eq!(response.graph_revision.as_deref(), Some("1"));
        assert_eq!(response.preview_session_revision, Some(1));
        assert!(response.stale);
    }

    #[test]
    fn telemetry_marks_preview_stale_after_session_revision_changes() {
        let mut manager = PreviewManager::dry_run();
        manager.start(Ok(context(1)), loaded_snapshot(1, "1"), false);

        let telemetry = manager.telemetry(loaded_snapshot(2, "2"), 0);

        assert_eq!(telemetry.session.session_revision, 2);
        assert_eq!(telemetry.preview.preview_session_revision, Some(1));
        assert!(telemetry.preview.stale);
    }

    #[test]
    fn restart_replaces_preview_context() {
        let mut manager = PreviewManager::dry_run();
        manager.start(Ok(context(1)), loaded_snapshot(1, "1"), false);

        let response = manager.restart(Ok(context(2)), loaded_snapshot(2, "2"));

        assert!(response.ok);
        assert_eq!(response.state, PreviewState::Running);
        assert_eq!(response.graph_revision.as_deref(), Some("2"));
        assert_eq!(response.preview_session_revision, Some(2));
        assert!(!response.stale);
    }

    #[test]
    fn stop_clears_preview_state() {
        let mut manager = PreviewManager::dry_run();
        manager.start(Ok(context(1)), loaded_snapshot(1, "1"), false);

        let response = manager.stop(loaded_snapshot(1, "1"));

        assert!(response.ok);
        assert_eq!(response.state, PreviewState::Stopped);
        assert_eq!(response.graph_id, None);
        assert_eq!(response.preview_session_revision, None);
        assert!(!response.stale);
    }

    #[test]
    fn fake_spawn_reports_pid_and_can_exit_on_status_poll() {
        let mut manager = PreviewManager::with_test_spawner(false, spawn_exiting_handle);
        let started = manager.start(Ok(context(1)), loaded_snapshot(1, "1"), false);
        assert_eq!(started.state, PreviewState::Running);
        assert_eq!(started.pid, Some(42));

        let exited = manager.status(loaded_snapshot(1, "1"));
        assert_eq!(exited.state, PreviewState::Exited);
        assert_eq!(exited.pid, None);
        assert_eq!(exited.exit_code, Some(0));
        assert_eq!(exited.message.as_deref(), Some("preview process exited"));
    }

    #[test]
    fn telemetry_reads_native_heartbeat_file() {
        let mut manager = PreviewManager::with_test_spawner(false, spawn_heartbeat_handle);
        manager.start(Ok(context(41)), loaded_snapshot(41, "1"), false);

        let telemetry = manager.telemetry(loaded_snapshot(41, "1"), 0);

        assert_eq!(telemetry.render.backend.as_deref(), Some("wgpu"));
        assert_eq!(telemetry.render.renderer.as_deref(), Some("clear-color"));
        assert_eq!(telemetry.render.frames_rendered, 3);
        assert_eq!(telemetry.render.source_node_id.as_deref(), Some("clear_1"));
    }

    #[test]
    fn telemetry_reports_invalid_heartbeat_as_warning() {
        let mut manager = PreviewManager::with_test_spawner(false, spawn_invalid_heartbeat_handle);
        manager.start(Ok(context(42)), loaded_snapshot(42, "1"), false);

        let telemetry = manager.telemetry(loaded_snapshot(42, "1"), 0);

        assert!(telemetry.ok);
        assert_eq!(
            telemetry.diagnostics[0].severity,
            crate::DiagnosticSeverity::Warning
        );
        assert!(
            telemetry.diagnostics[0]
                .message
                .contains("invalid preview telemetry heartbeat")
        );
        assert_eq!(telemetry.render.frames_rendered, 0);
    }

    #[test]
    fn starting_state_is_promoted_to_running_when_handle_is_alive() {
        let mut manager = PreviewManager::with_test_spawner(false, spawn_running_handle);
        manager.start(Ok(context(1)), loaded_snapshot(1, "1"), false);
        manager.status.state = PreviewState::Starting;

        let response = manager.status(loaded_snapshot(1, "1"));

        assert_eq!(response.state, PreviewState::Running);
    }

    #[test]
    fn fake_spawn_failure_reports_error_state() {
        let mut manager = PreviewManager::with_test_spawner(false, spawn_failure);
        let response = manager.start(Ok(context(1)), loaded_snapshot(1, "1"), false);

        assert!(!response.ok);
        assert_eq!(response.state, PreviewState::Error);
        assert_eq!(response.message.as_deref(), Some("spawn failed"));
        assert_eq!(response.diagnostics[0].message, "spawn failed");
    }

    #[test]
    fn handle_poll_error_reports_error_state() {
        let mut manager = PreviewManager::with_test_spawner(false, spawn_erroring_handle);
        manager.start(Ok(context(1)), loaded_snapshot(1, "1"), false);

        let response = manager.status(loaded_snapshot(1, "1"));

        assert_eq!(response.state, PreviewState::Error);
        assert_eq!(response.message.as_deref(), Some("poll failed"));
    }

    #[test]
    fn stop_failure_returns_diagnostic() {
        let mut manager = PreviewManager::with_test_spawner(false, spawn_unstoppable_handle);
        manager.start(Ok(context(1)), loaded_snapshot(1, "1"), false);

        let response = manager.stop(loaded_snapshot(1, "1"));

        assert!(!response.ok);
        assert_eq!(response.state, PreviewState::Error);
        assert_eq!(response.diagnostics[0].message, "stop failed");
    }

    #[test]
    fn fake_handle_stop_success_returns_stopped_state() {
        let mut manager = PreviewManager::with_test_spawner(false, spawn_running_handle);
        manager.start(Ok(context(1)), loaded_snapshot(1, "1"), false);

        let response = manager.stop(loaded_snapshot(1, "1"));

        assert!(response.ok);
        assert_eq!(response.state, PreviewState::Stopped);
    }

    #[test]
    fn start_request_defaults_restart_to_false() {
        let request: RuntimePreviewStartRequest =
            serde_json::from_value(json!({})).expect("request should deserialize");

        assert!(!request.restart);
    }

    fn spawn_exiting_handle(
        document: &PreviewDocument,
        telemetry_path: &Path,
    ) -> Result<Box<dyn PreviewHandle>, String> {
        assert_eq!(document.plan.graph_id, "minimal-value");
        assert_eq!(document.graph.id, "minimal-value");
        assert_eq!(document.session_revision, 1);
        assert!(
            telemetry_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("telemetry")
        );
        Ok(Box::new(FakePreviewHandle::new(Some(0), None, None)))
    }

    fn spawn_running_handle(
        _document: &PreviewDocument,
        _telemetry_path: &Path,
    ) -> Result<Box<dyn PreviewHandle>, String> {
        Ok(Box::new(FakePreviewHandle::new(None, None, None)))
    }

    fn spawn_erroring_handle(
        _document: &PreviewDocument,
        _telemetry_path: &Path,
    ) -> Result<Box<dyn PreviewHandle>, String> {
        Ok(Box::new(FakePreviewHandle::new(
            None,
            Some("poll failed"),
            None,
        )))
    }

    fn spawn_unstoppable_handle(
        _document: &PreviewDocument,
        _telemetry_path: &Path,
    ) -> Result<Box<dyn PreviewHandle>, String> {
        Ok(Box::new(FakePreviewHandle::new(
            None,
            None,
            Some("stop failed"),
        )))
    }

    fn spawn_failure(
        _document: &PreviewDocument,
        _telemetry_path: &Path,
    ) -> Result<Box<dyn PreviewHandle>, String> {
        Err("spawn failed".to_owned())
    }

    fn spawn_heartbeat_handle(
        document: &PreviewDocument,
        telemetry_path: &Path,
    ) -> Result<Box<dyn PreviewHandle>, String> {
        write_preview_telemetry_heartbeat(
            telemetry_path,
            &PreviewTelemetryHeartbeat {
                schema: PREVIEW_TELEMETRY_SCHEMA.to_owned(),
                schema_version: PREVIEW_TELEMETRY_SCHEMA_VERSION.to_owned(),
                timestamp: "unix-ms:1".to_owned(),
                pid: 42,
                graph_id: document.graph.id.clone(),
                graph_revision: document.graph.revision.clone(),
                session_revision: document.session_revision,
                renderer: "clear-color".to_owned(),
                backend: "wgpu".to_owned(),
                frames_rendered: 3,
                approx_fps: Some(60.0),
                last_frame_ms: Some(16.7),
                last_error: None,
                source_node_id: Some("clear_1".to_owned()),
            },
        )
        .expect("test heartbeat should write");
        Ok(Box::new(FakePreviewHandle::new(None, None, None)))
    }

    fn spawn_invalid_heartbeat_handle(
        _document: &PreviewDocument,
        telemetry_path: &Path,
    ) -> Result<Box<dyn PreviewHandle>, String> {
        if let Some(parent) = telemetry_path.parent() {
            std::fs::create_dir_all(parent).expect("invalid heartbeat parent should create");
        }
        std::fs::write(telemetry_path, b"{").expect("invalid heartbeat should write");
        Ok(Box::new(FakePreviewHandle::new(None, None, None)))
    }

    struct FakePreviewHandle {
        exit_code: Option<i32>,
        poll_error: Option<&'static str>,
        stop_error: Option<&'static str>,
    }

    impl FakePreviewHandle {
        fn new(
            exit_code: Option<i32>,
            poll_error: Option<&'static str>,
            stop_error: Option<&'static str>,
        ) -> Self {
            Self {
                exit_code,
                poll_error,
                stop_error,
            }
        }
    }

    impl PreviewHandle for FakePreviewHandle {
        fn pid(&self) -> Option<u32> {
            Some(42)
        }

        fn try_wait(&mut self) -> Result<Option<i32>, String> {
            match self.poll_error {
                Some(error) => Err(error.to_owned()),
                None => Ok(self.exit_code),
            }
        }

        fn stop(&mut self) -> Result<Option<i32>, String> {
            match self.stop_error {
                Some(error) => Err(error.to_owned()),
                None => Ok(Some(0)),
            }
        }
    }

    fn context(session_revision: u64) -> PreviewContext {
        PreviewContext {
            graph_id: "minimal-value".to_owned(),
            graph_revision: session_revision.to_string(),
            session_revision,
            graph: graph(&session_revision.to_string()),
            plan: plan(&session_revision.to_string()),
        }
    }

    fn graph(graph_revision: &str) -> GraphDocument {
        GraphDocument {
            schema: "skenion.graph".to_owned(),
            schema_version: "0.1.0".to_owned(),
            id: "minimal-value".to_owned(),
            revision: graph_revision.to_owned(),
            nodes: vec![GraphNode {
                id: "value_1".to_owned(),
                kind: "core.value-f32".to_owned(),
                kind_version: "0.1.0".to_owned(),
                params: serde_json::Map::new(),
                ports: Vec::new(),
            }],
            edges: Vec::new(),
        }
    }

    fn plan(graph_revision: &str) -> ExecutionPlan {
        ExecutionPlan {
            graph_id: "minimal-value".to_owned(),
            graph_revision: graph_revision.to_owned(),
            nodes: vec![PlanNode {
                node_id: "value_1".to_owned(),
                kind: "core.value-f32".to_owned(),
                kind_version: "0.1.0".to_owned(),
                execution_model: ExecutionModel::Value,
                order: 0,
            }],
            edges: vec![PlanEdge {
                from_node: "value_1".to_owned(),
                from_port: "out".to_owned(),
                to_node: "target_1".to_owned(),
                to_port: "in".to_owned(),
            }],
            groups: vec![ExecutionGroup {
                execution_model: ExecutionModel::Value,
                node_ids: vec!["value_1".to_owned()],
            }],
        }
    }

    fn shader_graph(graph_revision: &str) -> GraphDocument {
        let mut params = serde_json::Map::new();
        params.insert("language".to_owned(), json!("wgsl"));
        params.insert("source".to_owned(), json!(shader_source()));
        GraphDocument {
            schema: "skenion.graph".to_owned(),
            schema_version: "0.1.0".to_owned(),
            id: "fullscreen-shader".to_owned(),
            revision: graph_revision.to_owned(),
            nodes: vec![GraphNode {
                id: "shader_1".to_owned(),
                kind: "render.fullscreen-shader".to_owned(),
                kind_version: "0.1.0".to_owned(),
                params,
                ports: Vec::new(),
            }],
            edges: Vec::new(),
        }
    }

    fn shader_plan(graph_revision: &str) -> ExecutionPlan {
        ExecutionPlan {
            graph_id: "fullscreen-shader".to_owned(),
            graph_revision: graph_revision.to_owned(),
            nodes: vec![PlanNode {
                node_id: "shader_1".to_owned(),
                kind: "render.fullscreen-shader".to_owned(),
                kind_version: "0.1.0".to_owned(),
                execution_model: ExecutionModel::GpuPass,
                order: 0,
            }],
            edges: Vec::new(),
            groups: vec![ExecutionGroup {
                execution_model: ExecutionModel::GpuPass,
                node_ids: vec!["shader_1".to_owned()],
            }],
        }
    }

    fn shader_source() -> &'static str {
        r#"struct VertexOut {
  @builtin(position) position: vec4<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
  var positions = array<vec2<f32>, 3>(
    vec2<f32>(-1.0, -3.0),
    vec2<f32>(-1.0,  1.0),
    vec2<f32>( 3.0,  1.0)
  );

  var out: VertexOut;
  out.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
  return out;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
  return vec4<f32>(0.2, 0.3, 0.8, 1.0);
}"#
    }

    fn loaded_snapshot(session_revision: u64, graph_revision: &str) -> RuntimeSessionSnapshot {
        RuntimeSessionSnapshot {
            loaded: true,
            graph_id: Some("minimal-value".to_owned()),
            graph_revision: Some(graph_revision.to_owned()),
            session_revision,
            diagnostics: Vec::new(),
            plan: Some(plan(graph_revision)),
        }
    }

    fn empty_snapshot() -> RuntimeSessionSnapshot {
        RuntimeSessionSnapshot {
            loaded: false,
            graph_id: None,
            graph_revision: None,
            session_revision: 0,
            diagnostics: Vec::new(),
            plan: None,
        }
    }
}
