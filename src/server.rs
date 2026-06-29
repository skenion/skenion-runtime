use std::{
    collections::BTreeMap,
    convert::Infallible,
    fs,
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{Arc, RwLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        DefaultBodyLimit, Multipart, Path, State,
        ws::{WebSocketUpgrade, rejection::WebSocketUpgradeRejection},
    },
    http::{
        HeaderValue, Method, StatusCode,
        header::{CONTENT_TYPE, UPGRADE},
    },
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use skenion_contracts::{
    CONTRACTS_COMPATIBILITY_LINE, CONTRACTS_COMPATIBILITY_RANGE, CONTRACTS_PACKAGE_VERSION,
};
use tokio_stream::{
    Stream, StreamExt,
    wrappers::{BroadcastStream, IntervalStream, errors::BroadcastStreamRecvError},
};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::{
    CURRENT_SCHEMA_VERSION, DummyExecutionReport, ExecutionPlan, GeneratedShaderResponse,
    NodeDefinition, NodeRegistry, PackageRegistryListResponseV01, PreviewDocument,
    ProjectDocumentCurrent, ProjectRequestCurrent, RunProjectRequestCurrent,
    RuntimeControlReadRequest, RuntimeControlReadResponse, RuntimeControlStateResponse,
    RuntimeExtensionListResponse, RuntimeExtensionManager, RuntimeExtensionRegistrySnapshot,
    RuntimeIoDeviceListResponse, RuntimeIoDeviceManager, RuntimeLogSnapshotResponse,
    RuntimeLogStore, RuntimePackageManager, RuntimePackageRegistrySnapshot,
    RuntimePreviewStartRequest, RuntimeSessionEventKind, RuntimeSessionInfoResponse,
    RuntimeSessionLoadModeCurrent, RuntimeSessionLoadRequestCurrent, RuntimeTelemetrySnapshot,
    SessionRunRequest, ShaderDiagnostic, ShaderDiagnosticPhase, ShaderDiagnosticSource,
    build_execution_plan_request_current, build_execution_plan_run_request_current,
    generated_shader_response_from_preview_document, project_document_payload_schema_diagnostics,
    project_document_validation_diagnostics_current,
    realtime::{handle_runtime_realtime_socket, node_catalog_snapshot_for_record},
    run_dummy_execution,
    runtime_time::created_at_now,
    schema_version_diagnostic,
    session_registry::{RuntimeSessionRecord, RuntimeSessionRegistry, publish_session_event},
    sidecar::{
        RuntimeEndpointConfig, RuntimeSidecarHealthResponse, RuntimeSidecarShutdownResponse,
        RuntimeSidecarStartupResponse, runtime_connection_profile, sidecar_health_response,
        sidecar_shutdown_response, sidecar_startup_response,
    },
    validate_project_request_current,
};

pub const RUNTIME_API_VERSION: &str = "0.1.0";
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 3761;
const MAX_ASSET_UPLOAD_BYTES: usize = 512 * 1024 * 1024;
const RUNTIME_SESSION_LOAD_REQUEST_SCHEMA: &str = "skenion.runtime.session-load-request";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub ok: bool,
    pub service: &'static str,
    pub version: &'static str,
    pub api_version: &'static str,
    pub contracts_built_against_version: &'static str,
    pub supported_contracts_line: &'static str,
    pub supported_contracts_range: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfoResponse {
    pub name: &'static str,
    pub version: &'static str,
    pub api_version: &'static str,
    pub contracts_built_against_version: &'static str,
    pub supported_contracts_line: &'static str,
    pub supported_contracts_range: &'static str,
    pub capabilities: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeApiResponse {
    pub ok: bool,
    pub diagnostics: Vec<RuntimeDiagnostic>,
    pub plan: Option<ExecutionPlan>,
    pub report: Option<DummyExecutionReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeDiagnostic {
    pub severity: DiagnosticSeverity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
}

#[derive(Clone)]
pub struct RuntimeServerState {
    pub sessions: RuntimeSessionRegistry,
    pub assets: Arc<RwLock<RuntimeAssetStore>>,
    pub io_devices: Arc<RuntimeIoDeviceManager>,
    pub extensions: Arc<RuntimeExtensionRegistrySnapshot>,
    pub packages: Arc<RuntimePackageRegistrySnapshot>,
    pub logs: Arc<RuntimeLogStore>,
    pub endpoint: RuntimeEndpointConfig,
    pub started_at_wall_clock: String,
    pub started_at: Instant,
}

impl Default for RuntimeServerState {
    fn default() -> Self {
        Self::with_endpoint(DEFAULT_HOST.to_owned(), DEFAULT_PORT)
    }
}

impl RuntimeServerState {
    pub fn with_endpoint(host: String, port: u16) -> Self {
        let logs = Arc::new(RuntimeLogStore::default());
        let extension_scan = RuntimeExtensionManager::from_env().scan_registry();
        let package_scan = RuntimePackageManager::from_env().scan_registry();
        logs.record_runtime_diagnostics(extension_scan.log_diagnostics());
        logs.record_runtime_diagnostics(package_scan.log_diagnostics());
        Self {
            sessions: RuntimeSessionRegistry::default(),
            assets: Arc::new(RwLock::new(RuntimeAssetStore::default())),
            io_devices: Arc::new(RuntimeIoDeviceManager::new()),
            extensions: Arc::new(extension_scan.into_snapshot()),
            packages: Arc::new(package_scan.into_snapshot()),
            logs,
            endpoint: RuntimeEndpointConfig::new(host, port),
            started_at_wall_clock: created_at_now(),
            started_at: Instant::now(),
        }
    }

    pub fn sidecar_startup_response(&self) -> RuntimeSidecarStartupResponse {
        sidecar_startup_response(
            &self.endpoint,
            self.sessions.default_session_id(),
            &self.started_at_wall_clock,
        )
    }

    pub fn sidecar_health_response(&self) -> RuntimeSidecarHealthResponse {
        sidecar_health_response(&self.endpoint, &self.started_at_wall_clock)
    }
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeAssetStore {
    assets: BTreeMap<String, RuntimeAsset>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAsset {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub kind: String,
    pub size_bytes: u64,
    pub runtime_uri: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAssetImportResponse {
    pub ok: bool,
    pub asset: Option<RuntimeAsset>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAssetListResponse {
    pub ok: bool,
    pub assets: Vec<RuntimeAsset>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAssetGetResponse {
    pub ok: bool,
    pub asset: Option<RuntimeAsset>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

pub fn runtime_router() -> Router {
    runtime_router_with_state(RuntimeServerState::default())
}

pub fn runtime_router_with_state(state: RuntimeServerState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v0/runtime/info", get(runtime_info))
        .route("/v0/sidecar/startup", get(sidecar_startup))
        .route("/v0/sidecar/health", get(sidecar_health))
        .route("/v0/sidecar/shutdown", post(sidecar_shutdown))
        .route("/v0/extensions", get(runtime_extensions))
        .route("/v0/packages", get(runtime_packages))
        .route("/v0/runtime/logs", get(runtime_logs))
        .route("/v0/runtime/logs/stream", get(runtime_logs_stream))
        .route("/v0/io/devices", get(io_devices))
        .route("/v0/validate", post(validate_project_endpoint))
        .route("/v0/plan", post(plan_project_endpoint))
        .route("/v0/run", post(run_project_endpoint))
        .route(
            "/v0/sessions/{session_id}",
            get(realtime_session_by_id).delete(clear_session_by_id),
        )
        .route("/v0/sessions/{session_id}/info", get(session_info_by_id))
        .route(
            "/v0/sessions/{session_id}/snapshot",
            get(session_snapshot_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/node-catalog",
            get(session_node_catalog_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/events/stream",
            get(disabled_session_events_stream_by_id),
        )
        .route("/v0/sessions/{session_id}/load", post(load_session_by_id))
        .route(
            "/v0/sessions/{session_id}/validate",
            post(validate_session_by_id),
        )
        .route("/v0/sessions/{session_id}/plan", post(plan_session_by_id))
        .route("/v0/sessions/{session_id}/run", post(run_session_by_id))
        .route(
            "/v0/sessions/{session_id}/mutate",
            post(disabled_session_mutate_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/operation",
            post(disabled_session_operation_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/operations",
            post(disabled_session_operations_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/collaboration/presence",
            post(disabled_session_collaboration_presence_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/collaboration/selection",
            post(disabled_session_collaboration_selection_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/collaboration/events/stream",
            get(disabled_session_collaboration_events_stream_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/history",
            get(session_history_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/undo",
            post(disabled_session_undo_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/redo",
            post(disabled_session_redo_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/control/event",
            post(disabled_session_control_event_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/control/state",
            get(control_state_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/control/read",
            post(control_read_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/preview",
            get(preview_status_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/preview/start",
            post(start_preview_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/preview/stop",
            post(stop_preview_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/preview/restart",
            post(restart_preview_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/render/generated-shader",
            get(generated_shader_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/telemetry",
            get(session_telemetry_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/telemetry/stream",
            get(session_telemetry_stream_by_id),
        )
        .route(
            "/v0/assets/import",
            post(import_asset).layer(DefaultBodyLimit::max(MAX_ASSET_UPLOAD_BYTES)),
        )
        .route("/v0/assets", get(list_assets))
        .route("/v0/assets/{asset_id}", get(get_asset))
        .with_state(state)
        .layer(cors_layer())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        service: "skenion-runtime",
        version: env!("CARGO_PKG_VERSION"),
        api_version: RUNTIME_API_VERSION,
        contracts_built_against_version: CONTRACTS_PACKAGE_VERSION,
        supported_contracts_line: CONTRACTS_COMPATIBILITY_LINE,
        supported_contracts_range: CONTRACTS_COMPATIBILITY_RANGE,
    })
}

async fn runtime_info() -> Json<RuntimeInfoResponse> {
    Json(RuntimeInfoResponse {
        name: "skenion-runtime",
        version: env!("CARGO_PKG_VERSION"),
        api_version: RUNTIME_API_VERSION,
        contracts_built_against_version: CONTRACTS_PACKAGE_VERSION,
        supported_contracts_line: CONTRACTS_COMPATIBILITY_LINE,
        supported_contracts_range: CONTRACTS_COMPATIBILITY_RANGE,
        capabilities: vec![
            "project.validate",
            "project.validate.v0.1",
            "project.plan",
            "project.plan.v0.1",
            "dummy.run",
            "session.load",
            "session.load.v0.1",
            "session.realtime.websocket",
            "session.realtime.v0",
            "session.project",
            "session.project.v0.1",
            "session.nodeCatalog",
            "session.nodeCatalog.v0.1",
            "session.nodeCatalog.realtime.v0.1",
            "session.node.resolve",
            "session.node.create",
            "session.node.replace",
            "session.node.delete",
            "session.node.update",
            "session.node.input",
            "session.graph.changeSet.realtime.v0.1",
            "session.graph.pasteFragment.realtime.v0.1",
            "session.history.realtime.v0.1",
            "session.collaboration.selection.realtime.v0.1",
            "session.control.nodeInput.realtime.v0.1",
            "session.validate",
            "session.plan",
            "session.run",
            "session.history",
            "session.clear",
            "session.addressing",
            "session.info",
            "session.control.state",
            "session.control.read",
            "session.control.channels",
            "session.control.messages",
            "session.preview.controlState",
            "session.preview.status",
            "session.preview.start",
            "session.preview.stop",
            "session.preview.restart",
            "session.render.generatedShader",
            "assets.import",
            "assets.list",
            "assets.get",
            "session.telemetry",
            "session.telemetry.stream",
            "runtime.logs",
            "runtime.logs.stream",
            "runtime.extensions",
            "runtime.packages",
            "runtime.profile.localManaged",
            "runtime.profile.localShared",
            "runtime.profile.remote",
            "runtime.sidecar.startup",
            "runtime.sidecar.health",
            "runtime.sidecar.shutdown",
            "io.devices",
        ],
    })
}

async fn disabled_session_events_stream_by_id(Path(session_id): Path<String>) -> Response {
    http_live_channel_disabled_response(
        &session_id,
        serde_json::json!({
            "type": "session.hello",
            "details": "Use the WebSocket session and resume with payload.lastCursor for replay."
        }),
    )
}

async fn disabled_session_mutate_by_id(Path(session_id): Path<String>) -> Response {
    http_live_channel_disabled_response(
        &session_id,
        serde_json::json!({
            "type": "graph.command",
            "kinds": ["view.patch", "graph.changeSet"]
        }),
    )
}

async fn disabled_session_operation_by_id(Path(session_id): Path<String>) -> Response {
    http_live_channel_disabled_response(
        &session_id,
        serde_json::json!({
            "type": "graph.command",
            "kind": "graph.pasteFragment"
        }),
    )
}

async fn disabled_session_operations_by_id(Path(session_id): Path<String>) -> Response {
    http_live_channel_disabled_response(
        &session_id,
        serde_json::json!({
            "type": "graph.command",
            "kinds": ["graph.changeSet", "graph.pasteFragment", "history.undo", "history.redo"]
        }),
    )
}

async fn disabled_session_collaboration_presence_by_id(Path(session_id): Path<String>) -> Response {
    http_live_channel_disabled_response(
        &session_id,
        serde_json::json!({ "type": "presence.update" }),
    )
}

async fn disabled_session_collaboration_selection_by_id(
    Path(session_id): Path<String>,
) -> Response {
    http_live_channel_disabled_response(
        &session_id,
        serde_json::json!({ "type": "selection.update" }),
    )
}

async fn disabled_session_collaboration_events_stream_by_id(
    Path(session_id): Path<String>,
) -> Response {
    http_live_channel_disabled_response(
        &session_id,
        serde_json::json!({
            "type": "session.hello",
            "details": "Use WebSocket realtime events instead of collaboration SSE."
        }),
    )
}

async fn disabled_session_undo_by_id(Path(session_id): Path<String>) -> Response {
    http_live_channel_disabled_response(
        &session_id,
        serde_json::json!({
            "type": "graph.command",
            "kind": "history.undo"
        }),
    )
}

async fn disabled_session_redo_by_id(Path(session_id): Path<String>) -> Response {
    http_live_channel_disabled_response(
        &session_id,
        serde_json::json!({
            "type": "graph.command",
            "kind": "history.redo"
        }),
    )
}

async fn disabled_session_control_event_by_id(Path(session_id): Path<String>) -> Response {
    http_live_channel_disabled_response(
        &session_id,
        serde_json::json!({
            "type": "graph.command",
            "kind": "node.input"
        }),
    )
}

fn http_live_channel_disabled_response(
    session_id: &str,
    replacement: serde_json::Value,
) -> Response {
    (
        StatusCode::GONE,
        Json(serde_json::json!({
            "ok": false,
            "schema": "skenion.runtime.http-live-channel-disabled",
            "schemaVersion": "0.1.0",
            "sessionId": session_id,
            "diagnostics": [{
                "severity": "error",
                "code": "runtime.http-live-channel-disabled",
                "message": "HTTP live mutation and event channels are disabled; use the session WebSocket instead.",
                "details": {
                    "websocketEndpoint": format!("/v0/sessions/{session_id}"),
                    "replacement": replacement
                }
            }]
        })),
    )
        .into_response()
}

async fn sidecar_startup(
    State(state): State<RuntimeServerState>,
) -> Json<RuntimeSidecarStartupResponse> {
    Json(state.sidecar_startup_response())
}

async fn sidecar_health(
    State(state): State<RuntimeServerState>,
) -> Json<RuntimeSidecarHealthResponse> {
    Json(state.sidecar_health_response())
}

async fn sidecar_shutdown(
    State(state): State<RuntimeServerState>,
    body: Bytes,
) -> Json<RuntimeSidecarShutdownResponse> {
    let response = sidecar_shutdown_response(&body);
    state.logs.record_runtime_diagnostics(&response.diagnostics);
    Json(response)
}

async fn runtime_extensions(
    State(state): State<RuntimeServerState>,
) -> Json<RuntimeExtensionListResponse> {
    Json(state.extensions.response())
}

async fn runtime_packages(
    State(state): State<RuntimeServerState>,
) -> Json<PackageRegistryListResponseV01> {
    Json(state.packages.response())
}

async fn runtime_logs(State(state): State<RuntimeServerState>) -> Json<RuntimeLogSnapshotResponse> {
    Json(state.logs.snapshot())
}

async fn runtime_logs_stream(
    State(state): State<RuntimeServerState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let receiver = state.logs.subscribe();
    let replay = tokio_stream::iter(
        state
            .logs
            .snapshot()
            .events
            .into_iter()
            .map(runtime_log_event),
    );
    let live = BroadcastStream::new(receiver).map(runtime_log_broadcast_event);
    Sse::new(replay.chain(live)).keep_alive(KeepAlive::default())
}

fn runtime_log_broadcast_event(
    result: Result<crate::RuntimeLogEvent, BroadcastStreamRecvError>,
) -> Result<Event, Infallible> {
    match result {
        Ok(event) => runtime_log_event(event),
        Err(_) => Ok(Event::default()
            .event("log-gap")
            .data("runtime log stream receiver lagged")),
    }
}

fn runtime_log_event(event: crate::RuntimeLogEvent) -> Result<Event, Infallible> {
    Ok(Event::default()
        .event("log")
        .json_data(event)
        .expect("runtime log event should serialize"))
}

fn runtime_api_json(
    state: &RuntimeServerState,
    response: RuntimeApiResponse,
) -> Json<RuntimeApiResponse> {
    state.logs.record_runtime_diagnostics(&response.diagnostics);
    Json(response)
}

fn session_json(
    state: &RuntimeServerState,
    response: crate::RuntimeSessionResponse,
) -> Json<crate::RuntimeSessionResponse> {
    state.logs.record_runtime_diagnostics(&response.diagnostics);
    Json(response)
}

fn control_read_json(
    state: &RuntimeServerState,
    response: RuntimeControlReadResponse,
) -> Json<RuntimeControlReadResponse> {
    state.logs.record_runtime_diagnostics(&response.diagnostics);
    Json(response)
}

fn preview_status_json(
    state: &RuntimeServerState,
    response: crate::RuntimePreviewStatusResponse,
) -> Json<crate::RuntimePreviewStatusResponse> {
    state.logs.record_runtime_diagnostics(&response.diagnostics);
    Json(response)
}

fn asset_import_json(
    state: &RuntimeServerState,
    response: RuntimeAssetImportResponse,
) -> Json<RuntimeAssetImportResponse> {
    state.logs.record_runtime_diagnostics(&response.diagnostics);
    Json(response)
}

fn asset_get_json(
    state: &RuntimeServerState,
    response: RuntimeAssetGetResponse,
) -> Json<RuntimeAssetGetResponse> {
    state.logs.record_runtime_diagnostics(&response.diagnostics);
    Json(response)
}

fn generated_shader_json(
    state: &RuntimeServerState,
    response: GeneratedShaderResponse,
) -> Json<GeneratedShaderResponse> {
    state.logs.record_shader_diagnostics(&response.diagnostics);
    Json(response)
}

async fn io_devices(State(state): State<RuntimeServerState>) -> Json<RuntimeIoDeviceListResponse> {
    let response = state.io_devices.list_devices();
    state.logs.record_io_diagnostics(&response.diagnostics);
    Json(response)
}

async fn validate_project_endpoint(
    State(state): State<RuntimeServerState>,
    Json(value): Json<serde_json::Value>,
) -> Json<RuntimeApiResponse> {
    let response = match decode_project_payload(value) {
        Ok(ProjectPayload::Current(request)) => match validate_project_request_current(&request) {
            Ok((diagnostics, _)) => RuntimeApiResponse {
                ok: true,
                diagnostics,
                plan: None,
                report: None,
            },
            Err(diagnostics) => RuntimeApiResponse::diagnostics(diagnostics),
        },
        Err(diagnostics) => RuntimeApiResponse::diagnostics(diagnostics),
    };
    runtime_api_json(&state, response)
}

async fn plan_project_endpoint(
    State(state): State<RuntimeServerState>,
    Json(value): Json<serde_json::Value>,
) -> Json<RuntimeApiResponse> {
    match decode_project_payload(value) {
        Ok(ProjectPayload::Current(request)) => {
            match build_execution_plan_request_current(&request) {
                Ok((plan, diagnostics)) => runtime_api_json(
                    &state,
                    RuntimeApiResponse {
                        ok: true,
                        diagnostics,
                        plan: Some(plan),
                        report: None,
                    },
                ),
                Err(diagnostics) => {
                    runtime_api_json(&state, RuntimeApiResponse::diagnostics(diagnostics))
                }
            }
        }
        Err(diagnostics) => runtime_api_json(&state, RuntimeApiResponse::diagnostics(diagnostics)),
    }
}

async fn run_project_endpoint(
    State(state): State<RuntimeServerState>,
    Json(value): Json<serde_json::Value>,
) -> Json<RuntimeApiResponse> {
    match decode_run_project_payload(value) {
        Ok(RunProjectPayload::Current(request)) => {
            match build_execution_plan_run_request_current(&request) {
                Ok((plan, diagnostics)) => {
                    let report = run_dummy_execution(&plan, request.frames.unwrap_or(1));
                    runtime_api_json(
                        &state,
                        RuntimeApiResponse {
                            ok: true,
                            diagnostics,
                            plan: Some(plan),
                            report: Some(report),
                        },
                    )
                }
                Err(diagnostics) => {
                    runtime_api_json(&state, RuntimeApiResponse::diagnostics(diagnostics))
                }
            }
        }
        Err(diagnostics) => runtime_api_json(&state, RuntimeApiResponse::diagnostics(diagnostics)),
    }
}

async fn realtime_session_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    match ws {
        Ok(ws) => {
            let record = state.sessions.get_or_create(&session_id);
            ws.on_upgrade(move |socket| handle_runtime_realtime_socket(record, socket))
                .into_response()
        }
        Err(_) => (
            StatusCode::UPGRADE_REQUIRED,
            [(UPGRADE, HeaderValue::from_static("websocket"))],
            Json(serde_json::json!({
                "schema": "skenion.runtime.realtime.upgradeRequired",
                "schemaVersion": "0.1.0",
                "ok": false,
                "sessionId": session_id,
                "diagnostic": {
                    "code": "realtime.websocket-upgrade-required",
                    "message": "GET /v0/sessions/{sessionId} is the Runtime realtime WebSocket endpoint; send a WebSocket Upgrade request.",
                    "details": {
                        "endpoint": "/v0/sessions/{sessionId}",
                        "upgrade": "websocket"
                    }
                }
            })),
        )
            .into_response(),
    }
}

async fn session_info_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
) -> Json<RuntimeSessionInfoResponse> {
    Json(session_info_for(
        &state,
        state.sessions.get_or_create(&session_id),
    ))
}

fn session_info_for(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
) -> RuntimeSessionInfoResponse {
    let profile = runtime_connection_profile(&state.endpoint, &state.started_at_wall_clock);
    record.info_response(profile)
}

async fn session_snapshot_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
) -> Json<crate::RuntimeSessionResponse> {
    session_snapshot_for(&state, state.sessions.get_or_create(&session_id))
}

fn session_snapshot_for(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
) -> Json<crate::RuntimeSessionResponse> {
    let session = record
        .session
        .read()
        .expect("runtime session lock should not be poisoned");
    session_json(state, session.response(true, Vec::new(), None))
}

async fn session_node_catalog_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
) -> Response {
    let Some(record) = state.sessions.get_existing(&session_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "schema": "skenion.runtime.sessionNotFound",
                "schemaVersion": "0.1.0",
                "ok": false,
                "sessionId": session_id,
                "diagnostic": {
                    "code": "runtime.session-not-found",
                    "message": "No Runtime session exists for the requested sessionId."
                }
            })),
        )
            .into_response();
    };
    Json(node_catalog_snapshot_for_record(&record)).into_response()
}

async fn load_session_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
    Json(value): Json<serde_json::Value>,
) -> Json<crate::RuntimeSessionResponse> {
    load_session_for(&state, state.sessions.get_or_create(&session_id), value)
}

fn load_session_for(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
    value: serde_json::Value,
) -> Json<crate::RuntimeSessionResponse> {
    let _coordination_guard = record.collaboration.operation_guard();
    let mut session = record
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    let request = match decode_runtime_session_load_request_payload(value) {
        Ok(RuntimeSessionLoadPayload::Current(request)) => request,
        Err(diagnostics) => {
            let response = session.response(false, diagnostics, None);
            return session_json(state, response);
        }
    };
    if let Err(diagnostics) = validate_session_load_precondition(&session, &request) {
        let response = session.response(false, diagnostics, None);
        return session_json(state, response);
    }
    let project_request =
        ProjectRequestCurrent::from_project_document(request.project.clone(), Vec::new());
    let response = session.load_project_current_with_package_registry_revision(
        project_request,
        Some(state.packages.revision()),
    );
    if response.ok && response.snapshot.loaded() {
        publish_session_event(
            &record,
            RuntimeSessionEventKind::Load,
            &session,
            response.diagnostics.clone(),
        );
    }
    session_json(state, response)
}

async fn validate_session_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
) -> Json<crate::RuntimeSessionResponse> {
    validate_session_for(&state, state.sessions.get_or_create(&session_id))
}

fn validate_session_for(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
) -> Json<crate::RuntimeSessionResponse> {
    let mut session = record
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    let response = session.validate_current();
    session_json(state, response)
}

async fn plan_session_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
) -> Json<crate::RuntimeSessionResponse> {
    plan_session_for(&state, state.sessions.get_or_create(&session_id))
}

fn plan_session_for(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
) -> Json<crate::RuntimeSessionResponse> {
    let mut session = record
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    let response = session.plan_current();
    session_json(state, response)
}

async fn run_session_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
    Json(request): Json<SessionRunRequest>,
) -> Json<crate::RuntimeSessionResponse> {
    run_session_for(&state, state.sessions.get_or_create(&session_id), request)
}

fn run_session_for(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
    request: SessionRunRequest,
) -> Json<crate::RuntimeSessionResponse> {
    let mut session = record
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    let response = session.run_current(request.frames.unwrap_or(1));
    session_json(state, response)
}

async fn session_history_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
) -> Json<crate::RuntimeHistory> {
    session_history_for(state.sessions.get_or_create(&session_id))
}

fn session_history_for(record: RuntimeSessionRecord) -> Json<crate::RuntimeHistory> {
    let session = record
        .session
        .read()
        .expect("runtime session lock should not be poisoned");
    Json(session.history())
}

async fn control_state_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
) -> Json<RuntimeControlStateResponse> {
    control_state_for(state.sessions.get_or_create(&session_id))
}

fn control_state_for(record: RuntimeSessionRecord) -> Json<RuntimeControlStateResponse> {
    let session = record
        .session
        .read()
        .expect("runtime session lock should not be poisoned");
    Json(session.control_state_response())
}

async fn control_read_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
    Json(request): Json<RuntimeControlReadRequest>,
) -> Json<RuntimeControlReadResponse> {
    control_read_for(&state, state.sessions.get_or_create(&session_id), request)
}

fn control_read_for(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
    request: RuntimeControlReadRequest,
) -> Json<RuntimeControlReadResponse> {
    let session = record
        .session
        .read()
        .expect("runtime session lock should not be poisoned");
    let response = session.read_control(request);
    control_read_json(state, response)
}

async fn clear_session_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
) -> Json<crate::RuntimeSessionResponse> {
    clear_session_for(&state, state.sessions.get_or_create(&session_id))
}

fn clear_session_for(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
) -> Json<crate::RuntimeSessionResponse> {
    let _coordination_guard = record.collaboration.operation_guard();
    let snapshot = {
        let session = record
            .session
            .read()
            .expect("runtime session lock should not be poisoned");
        session.snapshot()
    };
    let _ = record
        .preview
        .lock()
        .expect("runtime preview lock should not be poisoned")
        .stop(snapshot);

    let mut session = record
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    let response = session.clear();
    if response.ok {
        publish_session_event(
            &record,
            RuntimeSessionEventKind::Clear,
            &session,
            response.diagnostics.clone(),
        );
    }
    session_json(state, response)
}

async fn preview_status_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
) -> Json<crate::RuntimePreviewStatusResponse> {
    preview_status_for(&state, state.sessions.get_or_create(&session_id))
}

fn preview_status_for(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
) -> Json<crate::RuntimePreviewStatusResponse> {
    let snapshot = {
        let session = record
            .session
            .read()
            .expect("runtime session lock should not be poisoned");
        session.snapshot()
    };
    let mut preview = record
        .preview
        .lock()
        .expect("runtime preview lock should not be poisoned");
    let response = preview.status(snapshot);
    preview_status_json(state, response)
}

async fn start_preview_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
    body: Bytes,
) -> Json<crate::RuntimePreviewStatusResponse> {
    start_preview_for(&state, state.sessions.get_or_create(&session_id), body)
}

fn start_preview_for(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
    body: Bytes,
) -> Json<crate::RuntimePreviewStatusResponse> {
    let snapshot = {
        let session = record
            .session
            .read()
            .expect("runtime session lock should not be poisoned");
        session.snapshot()
    };
    let request = match preview_start_request(&body) {
        Ok(request) => request,
        Err(diagnostic) => {
            let preview = record
                .preview
                .lock()
                .expect("runtime preview lock should not be poisoned");
            let response = preview.request_error(snapshot, diagnostic);
            return preview_status_json(state, response);
        }
    };
    let context = {
        let session = record
            .session
            .read()
            .expect("runtime session lock should not be poisoned");
        session.preview_context()
    };
    let mut preview = record
        .preview
        .lock()
        .expect("runtime preview lock should not be poisoned");
    let response = preview.start(context, snapshot, request.restart);
    preview_status_json(state, response)
}

async fn restart_preview_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
) -> Json<crate::RuntimePreviewStatusResponse> {
    restart_preview_for(&state, state.sessions.get_or_create(&session_id))
}

fn restart_preview_for(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
) -> Json<crate::RuntimePreviewStatusResponse> {
    let (snapshot, context) = {
        let session = record
            .session
            .read()
            .expect("runtime session lock should not be poisoned");
        (session.snapshot(), session.preview_context())
    };
    let mut preview = record
        .preview
        .lock()
        .expect("runtime preview lock should not be poisoned");
    let response = preview.restart(context, snapshot);
    preview_status_json(state, response)
}

async fn stop_preview_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
) -> Json<crate::RuntimePreviewStatusResponse> {
    stop_preview_for(&state, state.sessions.get_or_create(&session_id))
}

fn stop_preview_for(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
) -> Json<crate::RuntimePreviewStatusResponse> {
    let snapshot = {
        let session = record
            .session
            .read()
            .expect("runtime session lock should not be poisoned");
        session.snapshot()
    };
    let mut preview = record
        .preview
        .lock()
        .expect("runtime preview lock should not be poisoned");
    let response = preview.stop(snapshot);
    preview_status_json(state, response)
}

async fn generated_shader_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
) -> Json<GeneratedShaderResponse> {
    generated_shader_for(&state, state.sessions.get_or_create(&session_id))
}

fn generated_shader_for(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
) -> Json<GeneratedShaderResponse> {
    let context = {
        let session = record
            .session
            .read()
            .expect("runtime session lock should not be poisoned");
        session.preview_context()
    };

    let response = match context {
        Ok(context) => {
            let document = PreviewDocument::with_control_state(
                context.graph,
                context.plan,
                context.control_state,
                context.session_revision,
            );
            generated_shader_response_from_preview_document(&document)
        }
        Err(diagnostics) => GeneratedShaderResponse {
            ok: false,
            node_id: None,
            language: None,
            source: None,
            source_map: None,
            diagnostics: diagnostics
                .into_iter()
                .map(|diagnostic| {
                    ShaderDiagnostic::error(
                        ShaderDiagnosticPhase::SourceSync,
                        "generated-shader-unavailable",
                        diagnostic.message,
                        ShaderDiagnosticSource::Runtime,
                    )
                })
                .collect(),
        },
    };

    generated_shader_json(state, response)
}

async fn import_asset(
    State(state): State<RuntimeServerState>,
    mut multipart: Multipart,
) -> Json<RuntimeAssetImportResponse> {
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() != Some("file") {
            continue;
        }
        let name = field
            .file_name()
            .map(str::to_owned)
            .unwrap_or_else(|| "asset.bin".to_owned());
        let mime_type = field
            .content_type()
            .map(str::to_owned)
            .unwrap_or_else(|| "application/octet-stream".to_owned());
        let bytes = match field.bytes().await {
            Ok(bytes) => bytes,
            Err(error) => {
                let response = RuntimeAssetImportResponse {
                    ok: false,
                    asset: None,
                    diagnostics: vec![RuntimeDiagnostic::error(format!(
                        "failed to read uploaded asset bytes: {error}"
                    ))],
                };
                return asset_import_json(&state, response);
            }
        };

        let response = store_asset(&state, name, mime_type, bytes);
        return asset_import_json(&state, response);
    }

    let response = RuntimeAssetImportResponse {
        ok: false,
        asset: None,
        diagnostics: vec![RuntimeDiagnostic::error(
            "asset import request did not include a file field",
        )],
    };
    asset_import_json(&state, response)
}

async fn list_assets(State(state): State<RuntimeServerState>) -> Json<RuntimeAssetListResponse> {
    let assets = state
        .assets
        .read()
        .expect("runtime asset store lock should not be poisoned")
        .assets
        .values()
        .cloned()
        .collect();
    Json(RuntimeAssetListResponse {
        ok: true,
        assets,
        diagnostics: Vec::new(),
    })
}

async fn get_asset(
    State(state): State<RuntimeServerState>,
    Path(asset_id): Path<String>,
) -> Json<RuntimeAssetGetResponse> {
    let asset = state
        .assets
        .read()
        .expect("runtime asset store lock should not be poisoned")
        .assets
        .get(&asset_id)
        .cloned();
    let ok = asset.is_some();
    let response = RuntimeAssetGetResponse {
        ok,
        asset,
        diagnostics: if ok {
            Vec::new()
        } else {
            vec![RuntimeDiagnostic::error(format!(
                "asset {asset_id} does not exist"
            ))]
        },
    };
    asset_get_json(&state, response)
}

async fn session_telemetry_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
) -> Json<RuntimeTelemetrySnapshot> {
    Json(telemetry_snapshot(
        &state,
        state.sessions.get_or_create(&session_id),
    ))
}

async fn session_telemetry_stream_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    session_telemetry_stream_for(state, session_id)
}

fn session_telemetry_stream_for(
    state: RuntimeServerState,
    session_id: String,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream =
        IntervalStream::new(tokio::time::interval(Duration::from_millis(1000))).map(move |_| {
            let record = state.sessions.get_or_create(&session_id);
            let event = Event::default()
                .event("telemetry")
                .json_data(telemetry_snapshot(&state, record))
                .expect("telemetry snapshot should serialize");
            Ok(event)
        });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn telemetry_snapshot(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
) -> RuntimeTelemetrySnapshot {
    let snapshot = {
        let session = record
            .session
            .read()
            .expect("runtime session lock should not be poisoned");
        session.snapshot()
    };
    let mut preview = record
        .preview
        .lock()
        .expect("runtime preview lock should not be poisoned");
    preview.telemetry(
        snapshot,
        state
            .started_at
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
    )
}

fn store_asset(
    state: &RuntimeServerState,
    name: String,
    mime_type: String,
    bytes: Bytes,
) -> RuntimeAssetImportResponse {
    let id = asset_id(&name, &mime_type, &bytes);
    store_asset_with_id(state, id, name, mime_type, bytes, runtime_asset_directory())
}

fn store_asset_with_id(
    state: &RuntimeServerState,
    id: String,
    name: String,
    mime_type: String,
    bytes: Bytes,
    directory: PathBuf,
) -> RuntimeAssetImportResponse {
    let kind = asset_kind(&mime_type);
    let runtime_uri = format!("skenion-runtime://assets/{id}");
    if let Err(error) = fs::create_dir_all(&directory) {
        return RuntimeAssetImportResponse {
            ok: false,
            asset: None,
            diagnostics: vec![RuntimeDiagnostic::error(format!(
                "failed to create runtime asset directory: {error}"
            ))],
        };
    }
    let path = directory.join(&id);
    if let Err(error) = fs::write(&path, &bytes) {
        return RuntimeAssetImportResponse {
            ok: false,
            asset: None,
            diagnostics: vec![RuntimeDiagnostic::error(format!(
                "failed to store runtime asset: {error}"
            ))],
        };
    }
    let asset = RuntimeAsset {
        id: id.clone(),
        name,
        mime_type,
        kind,
        size_bytes: bytes.len().try_into().unwrap_or(u64::MAX),
        runtime_uri,
    };
    state
        .assets
        .write()
        .expect("runtime asset store lock should not be poisoned")
        .assets
        .insert(id, asset.clone());
    RuntimeAssetImportResponse {
        ok: true,
        asset: Some(asset),
        diagnostics: Vec::new(),
    }
}

fn runtime_asset_directory() -> PathBuf {
    std::env::temp_dir().join("skenion-runtime-assets")
}

fn asset_id(name: &str, mime_type: &str, bytes: &Bytes) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hasher);
    mime_type.hash(&mut hasher);
    bytes.hash(&mut hasher);
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    format!("asset_{:016x}", hasher.finish())
}

fn asset_kind(mime_type: &str) -> String {
    if mime_type.starts_with("video/") {
        "video".to_owned()
    } else if mime_type.starts_with("image/") {
        "image".to_owned()
    } else if mime_type.starts_with("audio/") {
        "audio".to_owned()
    } else {
        "binary".to_owned()
    }
}

impl RuntimeApiResponse {
    fn diagnostics(diagnostics: Vec<RuntimeDiagnostic>) -> Self {
        Self {
            ok: false,
            diagnostics,
            plan: None,
            report: None,
        }
    }
}

impl RuntimeDiagnostic {
    pub(crate) fn error(message: impl Into<String>) -> Self {
        Self {
            severity: DiagnosticSeverity::Error,
            message: message.into(),
            code: None,
            details: None,
        }
    }

    pub(crate) fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: DiagnosticSeverity::Warning,
            message: message.into(),
            code: None,
            details: None,
        }
    }

    pub(crate) fn structured_error(
        code: impl Into<String>,
        message: impl Into<String>,
        details: serde_json::Value,
    ) -> Self {
        Self {
            severity: DiagnosticSeverity::Error,
            message: message.into(),
            code: Some(code.into()),
            details: Some(details),
        }
    }

    pub(crate) fn structured_warning(
        code: impl Into<String>,
        message: impl Into<String>,
        details: serde_json::Value,
    ) -> Self {
        Self {
            severity: DiagnosticSeverity::Warning,
            message: message.into(),
            code: Some(code.into()),
            details: Some(details),
        }
    }
}

enum ProjectPayload {
    Current(Box<ProjectRequestCurrent>),
}

enum RunProjectPayload {
    Current(Box<RunProjectRequestCurrent>),
}

enum RuntimeSessionLoadPayload {
    Current(Box<RuntimeSessionLoadRequestCurrent>),
}

fn decode_runtime_session_load_request_payload(
    value: serde_json::Value,
) -> Result<RuntimeSessionLoadPayload, Vec<RuntimeDiagnostic>> {
    if is_project_document(&value) {
        return Err(vec![RuntimeDiagnostic::structured_error(
            "runtime.session-load.raw-project-rejected",
            "Runtime session load requires a skenion.runtime.session-load-request envelope; raw ProjectDocument bodies are no longer accepted.",
            json!({
                "schema": "skenion.runtime.session-load-request",
                "schemaVersion": CURRENT_SCHEMA_VERSION,
                "replacement": {
                    "schema": RUNTIME_SESSION_LOAD_REQUEST_SCHEMA,
                    "fields": ["project", "mode", "precondition"]
                }
            }),
        )]);
    }

    match runtime_session_load_request_schema_version(&value).as_deref() {
        Some(CURRENT_SCHEMA_VERSION) => decode_runtime_session_load_request_current(value)
            .map(Box::new)
            .map(RuntimeSessionLoadPayload::Current),
        received => Err(vec![runtime_session_load_schema_version_diagnostic(
            &value, received,
        )]),
    }
}

fn decode_runtime_session_load_request_current(
    value: serde_json::Value,
) -> Result<RuntimeSessionLoadRequestCurrent, Vec<RuntimeDiagnostic>> {
    if let Some(project) = value.get("project") {
        reject_top_level_nodes_current(project)?;
        let schema_diagnostics = project_document_payload_schema_diagnostics(project);
        if !schema_diagnostics.is_empty() {
            return Err(schema_diagnostics);
        }
    }
    let request = serde_json::from_value::<RuntimeSessionLoadRequestCurrent>(value)
        .map_err(invalid_runtime_session_load_payload)?;
    if let Err(report) = skenion_contracts::validate_runtime_session_load_request_v01(&request) {
        return Err(runtime_session_load_validation_diagnostics_current(
            &request, &report,
        ));
    }
    Ok(request)
}

fn validate_session_load_precondition(
    session: &crate::RuntimeSession,
    request: &RuntimeSessionLoadRequestCurrent,
) -> Result<(), Vec<RuntimeDiagnostic>> {
    let snapshot = session.snapshot();
    match &request.mode {
        RuntimeSessionLoadModeCurrent::ForceReplace => Ok(()),
        RuntimeSessionLoadModeCurrent::LoadIfEmpty if !snapshot.loaded() => Ok(()),
        RuntimeSessionLoadModeCurrent::LoadIfEmpty => Err(vec![session_load_conflict_diagnostic(
            request,
            &snapshot,
            "loadIfEmpty requires an empty Runtime session",
            Vec::new(),
        )]),
        RuntimeSessionLoadModeCurrent::ReplaceIfMatch => {
            let Some(current_project) = snapshot.project.as_ref() else {
                return Err(vec![session_load_conflict_diagnostic(
                    request,
                    &snapshot,
                    "replaceIfMatch requires an existing Runtime session project",
                    Vec::new(),
                )]);
            };
            let Some(precondition) = request.precondition.as_ref() else {
                return Err(vec![RuntimeDiagnostic::structured_error(
                    "runtime.session-load.precondition-required",
                    "replaceIfMatch requires a precondition",
                    session_load_request_details(request, &snapshot, Vec::new()),
                )]);
            };

            let mut mismatches = Vec::new();
            if let Some(expected) = &precondition.document_id
                && expected != &current_project.document_id
            {
                mismatches.push(json!({
                    "field": "documentId",
                    "expected": expected,
                    "actual": current_project.document_id,
                }));
            }
            if let Some(expected) = &precondition.session_revision {
                let actual = snapshot.session_revision.to_string();
                if expected != &actual {
                    mismatches.push(json!({
                        "field": "sessionRevision",
                        "expected": expected,
                        "actual": actual,
                    }));
                }
            }
            if let Some(expected) = &precondition.graph_revision {
                let actual = current_project.graph.revision.as_str();
                if expected != actual {
                    mismatches.push(json!({
                        "field": "graphRevision",
                        "expected": expected,
                        "actual": actual,
                    }));
                }
            }

            if mismatches.is_empty() {
                Ok(())
            } else {
                Err(vec![session_load_conflict_diagnostic(
                    request,
                    &snapshot,
                    "replaceIfMatch precondition does not match the current Runtime session",
                    mismatches,
                )])
            }
        }
    }
}

fn runtime_session_load_request_schema_version(value: &serde_json::Value) -> Option<String> {
    if value.get("schema").and_then(|schema| schema.as_str())
        != Some(RUNTIME_SESSION_LOAD_REQUEST_SCHEMA)
    {
        return None;
    }

    value
        .get("schemaVersion")
        .and_then(|version| version.as_str())
        .map(str::to_owned)
}

fn runtime_session_load_schema_version_diagnostic(
    value: &serde_json::Value,
    received_schema_version: Option<&str>,
) -> RuntimeDiagnostic {
    let received_schema = value.get("schema").and_then(|schema| schema.as_str());
    if received_schema != Some(RUNTIME_SESSION_LOAD_REQUEST_SCHEMA) {
        return RuntimeDiagnostic::structured_error(
            "runtime.session-load.invalid-schema",
            "Runtime session load requires a skenion.runtime.session-load-request envelope",
            json!({
                "expectedSchema": RUNTIME_SESSION_LOAD_REQUEST_SCHEMA,
                "receivedSchema": received_schema,
                "expectedSchemaVersion": CURRENT_SCHEMA_VERSION,
                "receivedSchemaVersion": received_schema_version,
            }),
        );
    }

    match received_schema_version {
        Some(version) => RuntimeDiagnostic::structured_error(
            "runtime.session-load.unsupported-schema-version",
            format!("unsupported Runtime session load schemaVersion: {version}"),
            json!({
                "expectedSchema": RUNTIME_SESSION_LOAD_REQUEST_SCHEMA,
                "expectedSchemaVersion": CURRENT_SCHEMA_VERSION,
                "receivedSchemaVersion": version,
            }),
        ),
        None => RuntimeDiagnostic::structured_error(
            "runtime.session-load.missing-schema-version",
            "missing schemaVersion in Runtime session load request",
            json!({
                "expectedSchema": RUNTIME_SESSION_LOAD_REQUEST_SCHEMA,
                "expectedSchemaVersion": CURRENT_SCHEMA_VERSION,
                "receivedSchemaVersion": serde_json::Value::Null,
            }),
        ),
    }
}

fn runtime_session_load_validation_diagnostics_current(
    request: &RuntimeSessionLoadRequestCurrent,
    report: &skenion_contracts::ValidationReportV01,
) -> Vec<RuntimeDiagnostic> {
    report
        .errors()
        .iter()
        .map(|error| {
            RuntimeDiagnostic::structured_error(
                "runtime.session-load.invalid-0.1",
                error.message.clone(),
                json!({
                    "schema": RUNTIME_SESSION_LOAD_REQUEST_SCHEMA,
                    "schemaVersion": request.schema_version,
                    "mode": runtime_session_load_mode_label(&request.mode),
                    "projectId": request.project.id,
                    "documentId": request.project.document_id,
                }),
            )
        })
        .collect()
}

fn invalid_runtime_session_load_payload(error: serde_json::Error) -> Vec<RuntimeDiagnostic> {
    vec![RuntimeDiagnostic::structured_error(
        "runtime.session-load.invalid-payload",
        format!("invalid Runtime session load request: {error}"),
        json!({
            "schema": RUNTIME_SESSION_LOAD_REQUEST_SCHEMA,
            "schemaVersion": CURRENT_SCHEMA_VERSION,
        }),
    )]
}

fn session_load_conflict_diagnostic(
    request: &RuntimeSessionLoadRequestCurrent,
    snapshot: &crate::RuntimeSessionSnapshot,
    message: &'static str,
    mismatches: Vec<serde_json::Value>,
) -> RuntimeDiagnostic {
    RuntimeDiagnostic::structured_error(
        "runtime.session-load.conflict",
        message,
        session_load_request_details(request, snapshot, mismatches),
    )
}

fn session_load_request_details(
    request: &RuntimeSessionLoadRequestCurrent,
    snapshot: &crate::RuntimeSessionSnapshot,
    mismatches: Vec<serde_json::Value>,
) -> serde_json::Value {
    let current = snapshot.project.as_ref().map(|project| {
        json!({
            "documentId": project.document_id,
            "projectId": project.id,
            "projectRevision": project.revision,
            "graphId": project.graph.id,
            "graphRevision": project.graph.revision,
            "sessionRevision": snapshot.session_revision.to_string(),
        })
    });

    json!({
        "requested": {
            "mode": runtime_session_load_mode_label(&request.mode),
            "documentId": request.project.document_id,
            "projectId": request.project.id,
            "projectRevision": request.project.revision,
            "graphId": request.project.graph.id,
            "graphRevision": request.project.graph.revision,
            "precondition": request.precondition,
        },
        "current": current,
        "mismatches": mismatches,
    })
}

fn runtime_session_load_mode_label(mode: &RuntimeSessionLoadModeCurrent) -> &'static str {
    match mode {
        RuntimeSessionLoadModeCurrent::LoadIfEmpty => "loadIfEmpty",
        RuntimeSessionLoadModeCurrent::ReplaceIfMatch => "replaceIfMatch",
        RuntimeSessionLoadModeCurrent::ForceReplace => "forceReplace",
    }
}

fn decode_project_payload(
    value: serde_json::Value,
) -> Result<ProjectPayload, Vec<RuntimeDiagnostic>> {
    match project_schema_version(&value).as_deref() {
        Some(CURRENT_SCHEMA_VERSION) => decode_project_payload_current(value)
            .map(Box::new)
            .map(ProjectPayload::Current),
        received => Err(vec![
            schema_version_diagnostic(project_schema_surface(&value), received)
                .expect("current schema version should have decoded as current 0.1"),
        ]),
    }
}

fn decode_run_project_payload(
    value: serde_json::Value,
) -> Result<RunProjectPayload, Vec<RuntimeDiagnostic>> {
    match project_schema_version(&value).as_deref() {
        Some(CURRENT_SCHEMA_VERSION) => decode_run_project_payload_current(value)
            .map(Box::new)
            .map(RunProjectPayload::Current),
        received => Err(vec![
            schema_version_diagnostic(project_schema_surface(&value), received)
                .expect("current schema version should have decoded as current 0.1"),
        ]),
    }
}

fn decode_project_payload_current(
    value: serde_json::Value,
) -> Result<ProjectRequestCurrent, Vec<RuntimeDiagnostic>> {
    if is_project_document_current(&value) {
        return decode_project_document_request_current(value);
    }

    serde_json::from_value(value).map_err(invalid_project_payload)
}

fn decode_run_project_payload_current(
    value: serde_json::Value,
) -> Result<RunProjectRequestCurrent, Vec<RuntimeDiagnostic>> {
    if is_project_document_current(&value) {
        return decode_run_project_document_request_current(value);
    }

    serde_json::from_value(value).map_err(invalid_project_payload)
}

fn decode_project_document_request_current(
    mut value: serde_json::Value,
) -> Result<ProjectRequestCurrent, Vec<RuntimeDiagnostic>> {
    reject_top_level_nodes_current(&value)?;
    let _ = take_frames_current(&mut value)?;
    let document = decode_project_document_current(value)?;
    Ok(ProjectRequestCurrent::from_project_document(
        document,
        Vec::new(),
    ))
}

fn decode_run_project_document_request_current(
    mut value: serde_json::Value,
) -> Result<RunProjectRequestCurrent, Vec<RuntimeDiagnostic>> {
    reject_top_level_nodes_current(&value)?;
    let frames = take_frames_current(&mut value)?;
    let document = decode_project_document_current(value)?;
    Ok(RunProjectRequestCurrent::from_project_document(
        document,
        Vec::new(),
        frames,
    ))
}

fn decode_project_document_current(
    value: serde_json::Value,
) -> Result<ProjectDocumentCurrent, Vec<RuntimeDiagnostic>> {
    let schema_diagnostics = project_document_payload_schema_diagnostics(&value);
    if !schema_diagnostics.is_empty() {
        return Err(schema_diagnostics);
    }
    let document =
        serde_json::from_value::<ProjectDocumentCurrent>(value).map_err(invalid_project_payload)?;
    if let Err(report) = skenion_contracts::validate_project_document_v01(&document) {
        return Err(project_document_validation_diagnostics_current(
            &document, &report,
        ));
    }
    Ok(document)
}

fn reject_top_level_nodes_current(value: &serde_json::Value) -> Result<(), Vec<RuntimeDiagnostic>> {
    if value.get("nodes").is_none() {
        return Ok(());
    }

    Err(vec![RuntimeDiagnostic::structured_error(
        "project.document.top-level-nodes-rejected",
        "ProjectDocument payloads must not include top-level nodes; node definitions must come from Runtime registry/catalog sources or an explicit legacy ProjectRequest wrapper",
        serde_json::json!({
            "surface": "project",
            "field": "nodes",
            "schema": "skenion.project",
        }),
    )])
}

fn take_frames_current(
    value: &mut serde_json::Value,
) -> Result<Option<usize>, Vec<RuntimeDiagnostic>> {
    let frames = value
        .as_object_mut()
        .and_then(|object| object.remove("frames"))
        .unwrap_or(serde_json::Value::Null);
    serde_json::from_value(frames).map_err(invalid_project_payload)
}

fn project_schema_version(value: &serde_json::Value) -> Option<String> {
    if is_project_document(value) {
        return value
            .get("schemaVersion")
            .and_then(|version| version.as_str())
            .map(str::to_owned);
    }

    value
        .get("graph")
        .and_then(|graph| graph.get("schemaVersion"))
        .and_then(|version| version.as_str())
        .map(str::to_owned)
}

fn is_project_document_current(value: &serde_json::Value) -> bool {
    is_project_document(value)
        && value
            .get("schemaVersion")
            .and_then(|version| version.as_str())
            == Some(CURRENT_SCHEMA_VERSION)
}

fn is_project_document(value: &serde_json::Value) -> bool {
    value.get("schema").and_then(|schema| schema.as_str()) == Some("skenion.project")
}

fn project_schema_surface(value: &serde_json::Value) -> &'static str {
    if is_project_document(value) {
        "project"
    } else {
        "graph"
    }
}

fn invalid_project_payload(error: serde_json::Error) -> Vec<RuntimeDiagnostic> {
    vec![RuntimeDiagnostic::error(format!(
        "invalid project request: {error}"
    ))]
}

pub(crate) fn registry_from_nodes(
    nodes: Vec<NodeDefinition>,
) -> Result<NodeRegistry, Vec<RuntimeDiagnostic>> {
    let mut registry = NodeRegistry::new();
    let mut diagnostics = Vec::new();

    for definition in nodes {
        if let Err(error) = registry.insert(definition) {
            diagnostics.push(RuntimeDiagnostic::error(error.to_string()));
        }
    }

    if diagnostics.is_empty() {
        Ok(registry)
    } else {
        Err(diagnostics)
    }
}

fn preview_start_request(body: &[u8]) -> Result<RuntimePreviewStartRequest, RuntimeDiagnostic> {
    if body.is_empty() {
        return Ok(RuntimePreviewStartRequest { restart: false });
    }
    serde_json::from_slice(body).map_err(|error| {
        RuntimeDiagnostic::error(format!("invalid preview start request: {error}"))
    })
}

fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _| {
            matches!(
                origin.to_str(),
                Ok("http://127.0.0.1:5173"
                    | "http://localhost:5173"
                    | "http://127.0.0.1:5174"
                    | "http://localhost:5174"
                    | "http://127.0.0.1:5175"
                    | "http://localhost:5175")
            )
        }))
        .allow_methods([Method::DELETE, Method::GET, Method::POST])
        .allow_headers([CONTENT_TYPE])
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        sync::Arc,
    };

    use axum::{
        body::{Body, to_bytes},
        http::{
            Method, Request, StatusCode,
            header::{
                ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_REQUEST_METHOD, CONTENT_TYPE, ORIGIN,
            },
        },
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use crate::{
        RuntimeIoDeviceDescriptor, RuntimeIoDeviceListResponse,
        io_device_manager::RuntimeIoDeviceRegistry, session_registry::DEFAULT_SESSION_ID,
    };

    use super::*;

    struct ServerFakeIoDeviceRegistry {
        devices: Vec<RuntimeIoDeviceDescriptor>,
    }

    impl RuntimeIoDeviceRegistry for ServerFakeIoDeviceRegistry {
        fn list_devices(&self) -> RuntimeIoDeviceListResponse {
            RuntimeIoDeviceListResponse {
                ok: true,
                devices: self.devices.clone(),
                diagnostics: Vec::new(),
            }
        }
    }

    #[tokio::test]
    async fn health_response() {
        let response = get_json("/health").await;

        assert_eq!(response["ok"], true);
        assert_eq!(response["service"], "skenion-runtime");
        assert_eq!(response["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(response["apiVersion"], RUNTIME_API_VERSION);
        assert_eq!(
            response["contractsBuiltAgainstVersion"],
            CONTRACTS_PACKAGE_VERSION
        );
        assert_eq!(
            response["supportedContractsLine"],
            CONTRACTS_COMPATIBILITY_LINE
        );
        assert_eq!(
            response["supportedContractsRange"],
            CONTRACTS_COMPATIBILITY_RANGE
        );
    }

    #[tokio::test]
    async fn runtime_info_response() {
        let response = get_json("/v0/runtime/info").await;

        assert_eq!(response["name"], "skenion-runtime");
        assert_eq!(response["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(response["apiVersion"], RUNTIME_API_VERSION);
        assert_eq!(
            response["contractsBuiltAgainstVersion"],
            CONTRACTS_PACKAGE_VERSION
        );
        assert_eq!(
            response["supportedContractsLine"],
            CONTRACTS_COMPATIBILITY_LINE
        );
        assert_eq!(
            response["supportedContractsRange"],
            CONTRACTS_COMPATIBILITY_RANGE
        );
        let capabilities = response["capabilities"].as_array().unwrap();
        for expected in [
            "project.validate",
            "project.validate.v0.1",
            "project.plan",
            "project.plan.v0.1",
            "dummy.run",
            "session.load",
            "session.load.v0.1",
            "session.nodeCatalog.realtime.v0.1",
            "session.graph.changeSet.realtime.v0.1",
            "session.graph.pasteFragment.realtime.v0.1",
            "session.history.realtime.v0.1",
            "session.collaboration.selection.realtime.v0.1",
            "session.control.nodeInput.realtime.v0.1",
            "session.history",
            "session.control.state",
            "session.control.channels",
            "session.control.messages",
            "assets.import",
            "assets.list",
            "assets.get",
            "session.preview.start",
            "session.render.generatedShader",
            "session.telemetry",
            "session.telemetry.stream",
            "runtime.logs",
            "runtime.logs.stream",
            "runtime.extensions",
            "session.addressing",
            "session.info",
            "runtime.profile.localManaged",
            "runtime.sidecar.startup",
            "runtime.sidecar.health",
            "runtime.sidecar.shutdown",
            "io.devices",
        ] {
            assert!(
                capabilities
                    .iter()
                    .any(|capability| capability.as_str() == Some(expected)),
                "missing capability {expected}"
            );
        }
        for removed in [
            "session.import.legacy.v0.1",
            "session.defaultAlias",
            "session.mutate",
            "session.operation",
            "session.pasteGraphFragment",
            "session.collaboration.operations",
            "session.collaboration.events.stream",
            "session.collaboration.presence",
            "session.collaboration.selection",
            "session.events.stream",
            "session.events.replay",
            "session.undo",
            "session.redo",
            "session.control.event",
        ] {
            assert!(
                !capabilities
                    .iter()
                    .any(|capability| capability.as_str() == Some(removed)),
                "removed compatibility capability {removed} should not be advertised"
            );
        }
    }

    #[tokio::test]
    async fn legacy_http_live_routes_return_gone_with_ws_replacements() {
        let app = runtime_router_with_dry_preview();
        for (method, path, replacement_type) in [
            (
                Method::GET,
                "/v0/sessions/default/events/stream",
                "session.hello",
            ),
            (Method::POST, "/v0/sessions/default/mutate", "graph.command"),
            (
                Method::POST,
                "/v0/sessions/default/operation",
                "graph.command",
            ),
            (
                Method::POST,
                "/v0/sessions/default/operations",
                "graph.command",
            ),
            (
                Method::POST,
                "/v0/sessions/default/collaboration/presence",
                "presence.update",
            ),
            (
                Method::POST,
                "/v0/sessions/default/collaboration/selection",
                "selection.update",
            ),
            (
                Method::GET,
                "/v0/sessions/default/collaboration/events/stream",
                "session.hello",
            ),
            (Method::POST, "/v0/sessions/default/undo", "graph.command"),
            (Method::POST, "/v0/sessions/default/redo", "graph.command"),
            (
                Method::POST,
                "/v0/sessions/default/control/event",
                "graph.command",
            ),
        ] {
            let (status, body) =
                request_json_status_with(app.clone(), method.clone(), path, json!({})).await;
            assert_eq!(status, StatusCode::GONE, "{path}");
            assert_eq!(body["ok"], false, "{path}");
            assert_eq!(body["schema"], "skenion.runtime.http-live-channel-disabled");
            assert_eq!(
                body["diagnostics"][0]["code"],
                "runtime.http-live-channel-disabled"
            );
            assert_eq!(
                body["diagnostics"][0]["details"]["websocketEndpoint"],
                "/v0/sessions/default"
            );
            assert_eq!(
                body["diagnostics"][0]["details"]["replacement"]["type"], replacement_type,
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn sidecar_startup_health_and_shutdown_are_machine_readable() {
        let app = runtime_router();

        let startup = get_json_with(app.clone(), "/v0/sidecar/startup").await;
        let health = get_json_with(app.clone(), "/v0/sidecar/health").await;
        let empty_shutdown = post_empty_with(app.clone(), "/v0/sidecar/shutdown").await;
        let shutdown = post_json_with(
            app.clone(),
            "/v0/sidecar/shutdown",
            json!({ "reason": "window-close", "ownerWindowId": "window-1" }),
        )
        .await;
        let invalid_shutdown = post_raw_with(app, "/v0/sidecar/shutdown", b"{".to_vec()).await;
        let startup_from_state = runtime_state_with_dry_preview().sidecar_startup_response();

        assert_eq!(startup["schema"], "skenion.runtime.sidecar.startup");
        assert_eq!(startup["ok"], true);
        assert_eq!(startup["runtime"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(startup["runtime"]["apiVersion"], RUNTIME_API_VERSION);
        assert_eq!(
            startup["runtime"]["contractsBuiltAgainstVersion"],
            CONTRACTS_PACKAGE_VERSION
        );
        assert_eq!(
            startup["runtime"]["supportedContractsLine"],
            CONTRACTS_COMPATIBILITY_LINE
        );
        assert_eq!(
            startup["runtime"]["supportedContractsRange"],
            CONTRACTS_COMPATIBILITY_RANGE
        );
        assert_eq!(startup["endpoint"]["protocol"], "http");
        assert_eq!(startup["profile"]["mode"], "local-managed");
        assert_eq!(startup["profile"]["ownership"], "owned-child");
        assert_eq!(startup["defaultSessionId"], DEFAULT_SESSION_ID);
        assert_eq!(startup["token"]["required"], false);
        assert_eq!(startup["token"]["header"], "Authorization");
        assert_eq!(startup["shutdown"]["scope"], "owned-child-only");
        assert!(startup["defaultSessionUrl"].is_string());
        assert_eq!(health["schema"], "skenion.runtime.sidecar.health");
        assert_eq!(health["ok"], true);
        assert_eq!(health["readiness"], "ready");
        assert_eq!(health["runtime"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(health["runtime"]["apiVersion"], RUNTIME_API_VERSION);
        assert_eq!(
            health["runtime"]["contractsBuiltAgainstVersion"],
            CONTRACTS_PACKAGE_VERSION
        );
        assert_eq!(
            health["runtime"]["supportedContractsLine"],
            CONTRACTS_COMPATIBILITY_LINE
        );
        assert_eq!(
            health["runtime"]["supportedContractsRange"],
            CONTRACTS_COMPATIBILITY_RANGE
        );
        assert_eq!(health["endpoint"]["protocol"], "http");
        assert_eq!(health["profile"]["mode"], "local-managed");
        assert!(health.get("token").is_none());
        assert!(health.get("shutdown").is_none());
        assert!(health.get("defaultSessionUrl").is_none());
        assert_eq!(empty_shutdown["ok"], true);
        assert_eq!(shutdown["schema"], "skenion.runtime.sidecar.shutdown");
        assert_eq!(shutdown["ok"], true);
        assert_eq!(shutdown["accepted"], false);
        assert_eq!(shutdown["action"], "host-owned-process-stop-required");
        assert_eq!(shutdown["scope"], "owned-child-only");
        assert_eq!(invalid_shutdown["ok"], false);
        assert!(startup_from_state.ok);
        assert!(
            runtime_state_with_dry_preview()
                .sidecar_health_response()
                .ok
        );
    }

    #[tokio::test]
    async fn runtime_extensions_response_defaults_to_empty_package_list() {
        let response = get_json("/v0/extensions").await;

        assert_eq!(response["ok"], true);
        assert_eq!(response["extensions"], json!([]));
        assert_eq!(response["diagnostics"], json!([]));
    }

    #[tokio::test]
    async fn successful_extension_startup_keeps_runtime_logs_empty() {
        let package_dir = server_temp_extension_dir("success-package");
        write_server_valid_extension_manifest(&package_dir);
        let app = runtime_router_with_extension_package_dirs(vec![package_dir]);

        let extensions = get_json_with(app.clone(), "/v0/extensions").await;
        assert_eq!(extensions["ok"], true);
        assert_eq!(extensions["diagnostics"], json!([]));
        assert_eq!(extensions["extensions"][0]["status"], "loaded");

        let logs = get_json_with(app, "/v0/runtime/logs").await;
        assert_eq!(logs["events"], json!([]));
    }

    #[tokio::test]
    async fn runtime_packages_endpoint_returns_startup_snapshot_without_rescan() {
        let package_dir = server_temp_package_dir("endpoint-startup-snapshot");
        write_server_valid_package_manifest(&package_dir, "example/server-package");
        let (app, state) = runtime_router_with_package_dirs(vec![package_dir.clone()]);

        let first_packages = get_json_with(app.clone(), "/v0/packages").await;
        serde_json::from_value::<PackageRegistryListResponseV01>(first_packages.clone())
            .expect("package registry endpoint should match Contracts DTO");
        assert_eq!(first_packages["ok"], true);
        assert_eq!(
            first_packages["packages"][0]["packageId"],
            "example/server-package"
        );
        assert_eq!(first_packages["packages"][0]["version"], "0.49.0");
        assert_eq!(first_packages["packages"][0]["contracts"]["line"], "0.49");
        assert_eq!(
            first_packages["packages"][0]["provides"]["patches"][0]["id"],
            "example.server-package.main"
        );
        assert_eq!(
            first_packages["packages"][0]["manifestPath"],
            crate::RUNTIME_PACKAGE_MANIFEST_FILE
        );
        assert_eq!(state.packages.revision(), 1);
        assert_eq!(state.packages.event_id(), "package-registry-event-000001");

        write_server_package_manifest(&package_dir, "{ not-json");
        let second_packages = get_json_with(app.clone(), "/v0/packages").await;
        assert_eq!(second_packages, first_packages);

        let logs_after_polling = get_json_with(app, "/v0/runtime/logs").await;
        assert_eq!(logs_after_polling["events"], json!([]));
    }

    #[tokio::test]
    async fn runtime_packages_and_logs_redact_absolute_package_paths() {
        let package_dir = server_temp_package_dir("redacted-extension-only");
        write_server_valid_extension_manifest(&package_dir);
        let (app, _) = runtime_router_with_package_dirs(vec![package_dir.clone()]);

        let packages = get_json_with(app.clone(), "/v0/packages").await;
        assert_eq!(packages["ok"], false);
        assert_eq!(
            packages["diagnostics"][0]["code"],
            "package.root.extension-only"
        );
        assert!(
            !packages
                .to_string()
                .contains(&package_dir.display().to_string())
        );

        let logs = get_json_with(app, "/v0/runtime/logs").await;
        assert_eq!(logs["events"][0]["code"], "package.root.extension-only");
        assert!(
            !logs
                .to_string()
                .contains(&package_dir.display().to_string())
        );
    }

    #[tokio::test]
    async fn session_load_pins_package_registry_snapshot_revision() {
        let package_dir = server_temp_package_dir("session-pin");
        write_server_valid_package_manifest(&package_dir, "example/session-pin");
        let (app, state) = runtime_router_with_package_dirs(vec![package_dir]);

        let loaded = post_json_with(
            app,
            "/v0/sessions/default/load",
            session_load_request(sample_project_document_current()),
        )
        .await;
        assert_eq!(loaded["ok"], true);

        let record = state.sessions.default_record();
        let session = record
            .session
            .read()
            .expect("runtime session lock should not be poisoned");
        assert_eq!(
            session.snapshot().package_registry_revision,
            Some(state.packages.revision())
        );
    }

    #[tokio::test]
    async fn startup_extension_scan_logs_package_diagnostics_once() {
        let missing_manifest_dir = server_temp_extension_dir("startup-missing-manifest");
        let malformed_manifest_dir = server_temp_extension_dir("startup-malformed-manifest");
        write_server_extension_manifest(&malformed_manifest_dir, "{ not-json");
        let app = runtime_router_with_extension_package_dirs(vec![
            missing_manifest_dir.clone(),
            malformed_manifest_dir.clone(),
        ]);

        let startup_logs = get_json_with(app.clone(), "/v0/runtime/logs").await;
        let startup_events = startup_logs["events"].as_array().unwrap();
        let malformed_manifest_path =
            malformed_manifest_dir.join(crate::RUNTIME_EXTENSION_MANIFEST_FILE);
        let expected_malformed_manifest_path =
            std::fs::canonicalize(&malformed_manifest_path).unwrap_or(malformed_manifest_path);
        assert_eq!(startup_events.len(), 2);
        assert!(startup_events.iter().any(|event| {
            event["code"] == "extension.manifest.missing"
                && event["details"]["packagePath"] == missing_manifest_dir.display().to_string()
                && event["details"]["action"] == "scan"
                && event["details"]["registryEvent"] == "extension-package-load"
        }));
        assert!(startup_events.iter().any(|event| {
            event["code"] == "extension.manifest.parse-failed"
                && event["details"]["packagePath"] == malformed_manifest_dir.display().to_string()
                && event["details"]["manifestPath"]
                    == expected_malformed_manifest_path.display().to_string()
        }));

        let first_extensions = get_json_with(app.clone(), "/v0/extensions").await;
        let second_extensions = get_json_with(app.clone(), "/v0/extensions").await;
        assert_eq!(first_extensions, second_extensions);
        assert_eq!(first_extensions["ok"], false);
        assert_eq!(
            first_extensions["diagnostics"][0]["code"],
            "extension.manifest.missing"
        );
        assert_eq!(
            first_extensions["extensions"][0]["diagnostics"][0]["code"],
            "extension.manifest.parse-failed"
        );

        let logs_after_polling = get_json_with(app, "/v0/runtime/logs").await;
        assert_eq!(logs_after_polling["events"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn runtime_log_stream_preserves_package_diagnostic_context() {
        let missing_manifest_dir = server_temp_extension_dir("stream-missing-manifest");
        let app = runtime_router_with_extension_package_dirs(vec![missing_manifest_dir.clone()]);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v0/runtime/logs/stream")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        let mut stream = response.into_body().into_data_stream();
        let chunk = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("runtime log stream should emit")
            .expect("runtime log stream should have a chunk")
            .expect("runtime log stream chunk should be ok");
        let text = std::str::from_utf8(&chunk).expect("runtime log stream should be utf8");

        assert!(text.contains("event: log"));
        assert!(text.contains("\"code\":\"extension.manifest.missing\""));
        assert!(text.contains("\"details\""));
        assert!(text.contains("\"registryEvent\":\"extension-package-load\""));
        assert!(text.contains(&missing_manifest_dir.display().to_string()));
    }

    #[tokio::test]
    async fn runtime_log_snapshot_replays_warning_error_backlog() {
        let state = runtime_state_with_dry_preview();
        let app = runtime_router_with_state(state.clone());

        let empty = get_json_with(app.clone(), "/v0/runtime/logs").await;
        assert_eq!(empty["schema"], "skenion.runtime.logs");
        assert_eq!(empty["events"], json!([]));
        assert_eq!(empty["retention"]["replayLimit"], 200);
        assert_eq!(
            empty["retention"]["replayLevels"],
            json!(["warning", "error"])
        );

        state
            .logs
            .record_runtime_diagnostics(&[RuntimeDiagnostic::structured_error(
                "runtime.test-no-undo",
                "no patch event available to undo",
                json!({ "source": "test" }),
            )]);

        let snapshot = get_json_with(app, "/v0/runtime/logs").await;
        let events = snapshot["events"].as_array().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["source"], "runtime");
        assert_eq!(events[0]["level"], "error");
        assert!(
            events[0]["message"]
                .as_str()
                .unwrap()
                .contains("available to undo")
        );
    }

    #[tokio::test]
    async fn runtime_log_stream_replays_backlog_as_sse() {
        let state = runtime_state_with_dry_preview();
        let app = runtime_router_with_state(state.clone());
        state
            .logs
            .record_runtime_diagnostics(&[RuntimeDiagnostic::structured_error(
                "runtime.test-no-undo",
                "no patch event available to undo",
                json!({ "source": "test" }),
            )]);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v0/runtime/logs/stream")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("text/event-stream")
        );
        let mut stream = response.into_body().into_data_stream();
        let chunk = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("runtime log stream should emit")
            .expect("runtime log stream should have a chunk")
            .expect("runtime log stream chunk should be ok");
        let text = std::str::from_utf8(&chunk).expect("runtime log stream should be utf8");
        assert!(text.contains("event: log"));
        assert!(text.contains("available to undo"));
    }

    #[tokio::test]
    async fn io_device_api_reports_empty_state() {
        let app = runtime_router_with_fake_io_devices(Vec::new());

        let devices = get_json_with(app.clone(), "/v0/io/devices").await;
        assert_eq!(devices["ok"], true);
        assert_eq!(devices["devices"], json!([]));
    }

    #[tokio::test]
    async fn asset_import_list_and_get_endpoints() {
        let app = runtime_router();
        let boundary = "skenion-test-boundary";
        let body = format!(
            "--{boundary}\r\ncontent-disposition: form-data; name=\"file\"; filename=\"clip.mov\"\r\ncontent-type: video/quicktime\r\n\r\nasset-bytes\r\n--{boundary}--\r\n"
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v0/assets/import")
                    .header(
                        CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        assert_eq!(response.status(), StatusCode::OK);
        let imported = body_json(response.into_body()).await;
        assert_eq!(imported["ok"], true);
        assert_eq!(imported["asset"]["name"], "clip.mov");
        assert_eq!(imported["asset"]["mimeType"], "video/quicktime");
        assert_eq!(imported["asset"]["kind"], "video");
        let asset_id = imported["asset"]["id"].as_str().unwrap();
        assert!(
            imported["asset"]["runtimeUri"]
                .as_str()
                .unwrap()
                .contains(asset_id)
        );

        let mut large_body = format!(
            "--{boundary}\r\ncontent-disposition: form-data; name=\"file\"; filename=\"large.mp4\"\r\ncontent-type: video/mp4\r\n\r\n"
        )
        .into_bytes();
        large_body.extend(vec![b'x'; 3 * 1024 * 1024]);
        large_body.extend(format!("\r\n--{boundary}--\r\n").into_bytes());
        let large = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v0/assets/import")
                    .header(
                        CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(large_body))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        assert_eq!(large.status(), StatusCode::OK);
        let large = body_json(large.into_body()).await;
        assert_eq!(large["ok"], true);
        assert_eq!(large["asset"]["name"], "large.mp4");

        let unnamed_body = format!(
            "--{boundary}\r\ncontent-disposition: form-data; name=\"file\"\r\n\r\nasset-bytes\r\n--{boundary}--\r\n"
        );
        let unnamed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v0/assets/import")
                    .header(
                        CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(unnamed_body))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        let unnamed = body_json(unnamed.into_body()).await;
        assert_eq!(unnamed["ok"], true);
        assert_eq!(unnamed["asset"]["name"], "asset.bin");
        assert_eq!(unnamed["asset"]["mimeType"], "application/octet-stream");
        assert_eq!(unnamed["asset"]["kind"], "binary");

        let listed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v0/assets")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        let listed = body_json(listed.into_body()).await;
        assert_eq!(listed["ok"], true);
        assert_eq!(listed["assets"].as_array().unwrap().len(), 3);

        let fetched = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/v0/assets/{asset_id}"))
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        let fetched = body_json(fetched.into_body()).await;
        assert_eq!(fetched["ok"], true);
        assert_eq!(fetched["asset"]["id"], asset_id);

        let missing = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v0/assets/missing")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        let missing = body_json(missing.into_body()).await;
        assert_eq!(missing["ok"], false);
        assert!(
            missing["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("missing")
        );

        let ignored_field = format!(
            "--{boundary}\r\ncontent-disposition: form-data; name=\"metadata\"\r\n\r\nignored\r\n--{boundary}--\r\n"
        );
        let missing_file = runtime_router()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v0/assets/import")
                    .header(
                        CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(ignored_field))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        let missing_file = body_json(missing_file.into_body()).await;
        assert_eq!(missing_file["ok"], false);
        assert!(
            missing_file["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("did not include a file field")
        );

        let malformed_file = format!(
            "--{boundary}\r\ncontent-disposition: form-data; name=\"file\"; filename=\"broken.bin\"\r\ncontent-type: application/octet-stream\r\n\r\nunterminated"
        );
        let malformed = runtime_router()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v0/assets/import")
                    .header(
                        CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(malformed_file))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        let malformed = body_json(malformed.into_body()).await;
        assert_eq!(malformed["ok"], false);
    }

    #[test]
    fn asset_store_helpers_report_filesystem_errors_and_kind_labels() {
        let state = RuntimeServerState::default();
        let base = std::env::temp_dir().join(format!(
            "skenion-asset-store-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&base, b"not a directory").expect("blocker should write");

        let create_error = store_asset_with_id(
            &state,
            "asset_create_error".to_owned(),
            "clip.mov".to_owned(),
            "video/quicktime".to_owned(),
            Bytes::from_static(b"asset"),
            base.clone(),
        );
        assert!(!create_error.ok);
        assert!(
            create_error.diagnostics[0]
                .message
                .contains("failed to create runtime asset directory")
        );

        std::fs::remove_file(&base).expect("blocker should remove");
        std::fs::create_dir_all(&base).expect("base directory should create");
        std::fs::create_dir(base.join("asset_write_error")).expect("asset blocker should create");

        let write_error = store_asset_with_id(
            &state,
            "asset_write_error".to_owned(),
            "clip.mov".to_owned(),
            "video/quicktime".to_owned(),
            Bytes::from_static(b"asset"),
            base.clone(),
        );
        assert!(!write_error.ok);
        assert!(
            write_error.diagnostics[0]
                .message
                .contains("failed to store runtime asset")
        );

        assert_eq!(asset_kind("video/mp4"), "video");
        assert_eq!(asset_kind("image/png"), "image");
        assert_eq!(asset_kind("audio/wav"), "audio");
        assert_eq!(asset_kind("application/octet-stream"), "binary");

        std::fs::remove_dir_all(base).expect("base directory should remove");
    }

    #[tokio::test]
    async fn cors_allows_local_studio_origin() {
        for origin in [
            "http://127.0.0.1:5173",
            "http://localhost:5173",
            "http://127.0.0.1:5174",
            "http://localhost:5174",
            "http://127.0.0.1:5175",
            "http://localhost:5175",
        ] {
            let response = runtime_router()
                .oneshot(
                    Request::builder()
                        .method(Method::OPTIONS)
                        .uri("/v0/runtime/info")
                        .header(ORIGIN, origin)
                        .header(ACCESS_CONTROL_REQUEST_METHOD, "GET")
                        .body(Body::empty())
                        .expect("request should build"),
                )
                .await
                .expect("router should respond");

            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers().get(ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
                origin
            );
        }
    }

    #[tokio::test]
    async fn cors_rejects_unknown_origin() {
        let response = runtime_router()
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/v0/runtime/info")
                    .header(ORIGIN, "http://example.test")
                    .header(ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response
                .headers()
                .get(ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
    }

    #[tokio::test]
    async fn project_endpoints_reject_missing_graph_schema_version() {
        for path in ["/v0/validate", "/v0/plan", "/v0/run"] {
            let response = post_json(path, json!({ "graph": 42, "nodes": [] })).await;

            assert_eq!(response["ok"], false);
            assert_eq!(
                response["diagnostics"][0]["code"],
                "project.missing-schema-version"
            );
            assert_eq!(response["plan"], Value::Null);
            assert_eq!(response["report"], Value::Null);
        }
    }

    #[tokio::test]
    async fn current_project_endpoints_validate_plan_and_run_with_edge_metadata() {
        let validation = post_json("/v0/validate", sample_project_current()).await;
        assert_eq!(validation["ok"], true);
        assert_eq!(validation["diagnostics"].as_array().unwrap().len(), 0);
        assert_eq!(validation["plan"], Value::Null);

        let plan = post_json("/v0/plan", sample_project_current()).await;
        assert_eq!(plan["ok"], true);
        assert_eq!(plan["plan"]["graphId"], "render-output-current");
        assert_eq!(plan["plan"]["graphRevision"], "1");
        assert_eq!(
            plan["plan"]["edges"][0]["metadata"]["resolvedType"],
            "value.core.tensor"
        );
        assert_eq!(
            plan["plan"]["edges"][0]["metadata"]["mergePolicy"],
            "forbid"
        );
        assert_eq!(
            plan["plan"]["edges"][0]["metadata"]["fanOutPolicy"],
            "allow"
        );
        assert_eq!(
            plan["plan"]["edges"][0]["metadata"]["cycleClassification"],
            Value::Null
        );
        assert_eq!(plan["report"], Value::Null);

        let mut run_request = sample_project_current();
        run_request["frames"] = json!(3);
        let run = post_json("/v0/run", run_request).await;
        assert_eq!(run["ok"], true);
        assert_eq!(run["report"]["frameCount"], 3);
        assert_eq!(
            run["report"]["frames"][0]["executedNodes"][0]["status"],
            "simulated"
        );
    }

    #[tokio::test]
    async fn current_project_document_payload_expands_patch_library_before_plan_and_run() {
        let validation =
            post_json("/v0/validate", sample_subpatch_project_document_current()).await;
        assert_eq!(validation["ok"], true);
        assert_eq!(validation["diagnostics"].as_array().unwrap().len(), 0);

        let plan = post_json("/v0/plan", sample_subpatch_project_document_current()).await;
        assert_eq!(plan["ok"], true);
        assert_eq!(plan["plan"]["graphId"], "subpatch-project-root");
        assert!(
            plan["plan"]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["nodeId"] == "fx::pass")
        );
        assert!(
            plan["plan"]["edges"]
                .as_array()
                .unwrap()
                .iter()
                .any(|edge| {
                    edge["fromNode"] == "clear_color"
                        && edge["toNode"] == "fx::pass"
                        && edge["toPort"] == "in"
                })
        );

        let mut run_request = sample_subpatch_project_document_current();
        run_request["frames"] = json!(2);
        let run = post_json("/v0/run", run_request).await;
        assert_eq!(run["ok"], true);
        assert_eq!(run["report"]["frameCount"], 2);
    }

    #[tokio::test]
    async fn current_project_document_payload_reports_decode_and_contract_errors() {
        let malformed_project = json!({
          "schema": "skenion.project",
          "schemaVersion": "0.1.0"
        });
        let malformed_response = post_json("/v0/validate", malformed_project).await;
        assert_eq!(malformed_response["ok"], false);
        assert_eq!(
            malformed_response["diagnostics"][0]["code"],
            "project.missing-schema-version"
        );
        assert_eq!(
            malformed_response["diagnostics"][0]["details"]["surface"],
            "graph"
        );

        let mut duplicate_patch = sample_subpatch_project_document_current();
        let patch = duplicate_patch["patchLibrary"][0].clone();
        duplicate_patch["patchLibrary"]
            .as_array_mut()
            .unwrap()
            .push(patch);

        let response = post_json("/v0/plan", duplicate_patch).await;
        assert_eq!(response["ok"], false);
        assert_eq!(
            response["diagnostics"][0]["code"],
            json!("project.invalid-0.1")
        );
        assert_eq!(
            response["diagnostics"][0]["details"]["projectId"],
            json!("subpatch-project")
        );
    }

    #[tokio::test]
    async fn current_project_endpoints_reject_ambiguous_algebraic_loop() {
        let response = post_json("/v0/validate", sample_ambiguous_loop_project_current()).await;

        assert_eq!(response["ok"], false);
        assert!(
            response["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| diagnostic["code"] == "graph.ambiguous-algebraic-loop")
        );

        let plan = post_json("/v0/plan", sample_ambiguous_loop_project_current()).await;
        assert_eq!(plan["ok"], false);
        assert!(
            plan["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| diagnostic["code"] == "graph.ambiguous-algebraic-loop")
        );

        let run = post_json("/v0/run", sample_ambiguous_loop_project_current()).await;
        assert_eq!(run["ok"], false);
        assert!(
            run["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| diagnostic["code"] == "graph.ambiguous-algebraic-loop")
        );
    }

    #[tokio::test]
    async fn project_endpoints_reject_missing_and_unsupported_schema_versions() {
        let mut missing = sample_project_current();
        missing["graph"]
            .as_object_mut()
            .unwrap()
            .remove("schemaVersion");
        let missing_response = post_json("/v0/validate", missing).await;
        assert_eq!(missing_response["ok"], false);
        assert_eq!(
            missing_response["diagnostics"][0]["code"],
            "project.missing-schema-version"
        );
        assert_eq!(
            missing_response["diagnostics"][0]["details"]["surface"],
            "graph"
        );
        assert_eq!(
            missing_response["diagnostics"][0]["details"]["expectedSchemaVersion"],
            "0.1.0"
        );
        assert_eq!(
            missing_response["diagnostics"][0]["details"]["receivedSchemaVersion"],
            Value::Null
        );

        let mut unsupported = sample_project_current();
        unsupported["graph"]["schemaVersion"] = json!("9.9.9");
        let unsupported_response = post_json("/v0/plan", unsupported).await;
        assert_eq!(unsupported_response["ok"], false);
        assert_eq!(
            unsupported_response["diagnostics"][0]["code"],
            "project.unsupported-schema-version"
        );
        assert_eq!(
            unsupported_response["diagnostics"][0]["details"]["surface"],
            "graph"
        );
        assert_eq!(
            unsupported_response["diagnostics"][0]["details"]["expectedSchemaVersion"],
            "0.1.0"
        );
        assert_eq!(
            unsupported_response["diagnostics"][0]["details"]["receivedSchemaVersion"],
            "9.9.9"
        );

        let mut missing_run = sample_project_current();
        missing_run["graph"]
            .as_object_mut()
            .unwrap()
            .remove("schemaVersion");
        let missing_run_response = post_json("/v0/run", missing_run).await;
        assert_eq!(missing_run_response["ok"], false);
        assert_eq!(
            missing_run_response["diagnostics"][0]["code"],
            "project.missing-schema-version"
        );

        let mut unsupported_run = sample_project_current();
        unsupported_run["graph"]["schemaVersion"] = json!("9.9.9");
        let unsupported_run_response = post_json("/v0/run", unsupported_run).await;
        assert_eq!(unsupported_run_response["ok"], false);
        assert_eq!(
            unsupported_run_response["diagnostics"][0]["code"],
            "project.unsupported-schema-version"
        );

        let mut unsupported_project = sample_project_document_current();
        unsupported_project["schemaVersion"] = json!("9.9.9");
        let unsupported_project_response = post_json("/v0/validate", unsupported_project).await;
        assert_eq!(unsupported_project_response["ok"], false);
        assert_eq!(
            unsupported_project_response["diagnostics"][0]["code"],
            "project.unsupported-schema-version"
        );
        assert_eq!(
            unsupported_project_response["diagnostics"][0]["details"]["surface"],
            "project"
        );
        assert_eq!(
            unsupported_project_response["diagnostics"][0]["details"]["expectedSchemaVersion"],
            "0.1.0"
        );
        assert_eq!(
            unsupported_project_response["diagnostics"][0]["details"]["receivedSchemaVersion"],
            "9.9.9"
        );

        let mut unsupported_project_graph = sample_project_document_current();
        unsupported_project_graph["graph"]["schemaVersion"] = json!("9.9.9");
        let unsupported_project_graph_response =
            post_json("/v0/validate", unsupported_project_graph).await;
        assert_eq!(unsupported_project_graph_response["ok"], false);
        assert_eq!(
            unsupported_project_graph_response["diagnostics"][0]["code"],
            "project.unsupported-schema-version"
        );
        assert_eq!(
            unsupported_project_graph_response["diagnostics"][0]["details"]["surface"],
            "graph"
        );
        assert_eq!(
            unsupported_project_graph_response["diagnostics"][0]["details"]["expectedSchemaVersion"],
            "0.1.0"
        );
        assert_eq!(
            unsupported_project_graph_response["diagnostics"][0]["details"]["receivedSchemaVersion"],
            "9.9.9"
        );
    }

    #[tokio::test]
    async fn project_endpoints_reject_malformed_payloads() {
        let mut request = sample_project_current();
        request["nodes"] = json!({});

        let response = post_json("/v0/validate", request).await;

        assert_eq!(response["ok"], false);
        assert!(
            response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("invalid project request")
        );
    }

    #[tokio::test]
    async fn project_document_ingest_rejects_top_level_nodes() {
        let mut project = sample_project_document_current();
        project["nodes"] = json!([value_f32_node_definition_current_json()]);

        for path in ["/v0/validate", "/v0/plan", "/v0/run"] {
            let response = post_json(path, project.clone()).await;
            assert_eq!(response["ok"], false, "{path}");
            assert_eq!(
                response["diagnostics"][0]["code"], "project.document.top-level-nodes-rejected",
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn session_load_rejects_raw_project_body() {
        let app = runtime_router();
        let raw_project = sample_project_document_current();

        let response = post_json_with(app.clone(), "/v0/sessions/default/load", raw_project).await;

        assert_eq!(response["ok"], false);
        assert_eq!(response["snapshot"]["project"], Value::Null);
        assert_eq!(
            response["diagnostics"][0]["code"],
            "runtime.session-load.raw-project-rejected"
        );
        let snapshot = get_json_with(app, "/v0/sessions/default/snapshot").await;
        assert_eq!(snapshot["snapshot"]["sessionRevision"], 0);
        assert_eq!(snapshot["snapshot"]["project"], Value::Null);
    }

    #[tokio::test]
    async fn session_endpoint_returns_empty_state() {
        let response = get_json("/v0/sessions/default/snapshot").await;

        assert_eq!(response["ok"], true);
        assert_eq!(response["snapshot"]["project"], Value::Null);
        assert_eq!(response["snapshot"]["sessionRevision"], 0);
        assert_eq!(response["snapshot"]["viewRevision"], 0);
        assert_eq!(response["snapshot"]["controlRevision"], 0);
        assert_eq!(response["diagnostics"].as_array().unwrap().len(), 0);
        assert_eq!(response["snapshot"]["plan"], Value::Null);
        assert_eq!(response["report"], Value::Null);
    }

    #[tokio::test]
    async fn session_snapshot_returns_loaded_project() {
        let app = runtime_router();

        let empty = get_json_with(app.clone(), "/v0/sessions/default/snapshot").await;
        assert_eq!(empty["ok"], true);
        assert_eq!(empty["snapshot"]["project"], Value::Null);

        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            session_load_request(sample_project_document_current()),
        )
        .await;
        let project = get_json_with(app, "/v0/sessions/default/snapshot").await;

        assert_eq!(project["ok"], true);
        assert_eq!(
            project["snapshot"]["project"]["id"],
            "minimal-value-project"
        );
        assert_eq!(
            project["snapshot"]["project"]["graph"]["id"],
            "minimal-value"
        );
        assert!(project["snapshot"]["project"]["nodes"].is_null());
    }

    #[tokio::test]
    async fn session_load_stores_valid_project() {
        let app = runtime_router();
        let mut project = sample_subpatch_project_document_current();
        project["metadata"] = json!({
            "title": "Loaded Subpatch Project",
            "source": "session-load-test"
        });
        project["tutorial"] = json!({
            "steps": [{ "id": "intro", "title": "Intro" }]
        });
        project["help"] = json!({
            "topics": ["object.core.subpatch"]
        });
        let response = post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            session_load_request(project),
        )
        .await;

        assert_eq!(response["ok"], true);
        assert_eq!(response["snapshot"]["project"]["id"], "subpatch-project");
        assert_eq!(
            response["snapshot"]["project"]["metadata"]["title"],
            "Loaded Subpatch Project"
        );
        assert_eq!(
            response["snapshot"]["project"]["metadata"]["source"],
            "session-load-test"
        );
        assert_eq!(
            response["snapshot"]["project"]["tutorial"]["steps"][0]["id"],
            "intro"
        );
        assert_eq!(
            response["snapshot"]["project"]["help"]["topics"][0],
            "object.core.subpatch"
        );
        assert_eq!(
            response["snapshot"]["project"]["patchLibrary"][0]["id"],
            "identity"
        );
        assert_eq!(
            response["snapshot"]["project"]["graph"]["id"],
            "subpatch-project-root"
        );
        assert_eq!(response["snapshot"]["project"]["graph"]["revision"], "1");
        assert_eq!(response["snapshot"]["sessionRevision"], 1);
        assert_eq!(
            response["snapshot"]["plan"]["graphId"],
            "subpatch-project-root"
        );

        let snapshot = get_json_with(app, "/v0/sessions/default/snapshot").await;
        assert_eq!(
            snapshot["snapshot"]["project"]["graph"]["id"],
            "subpatch-project-root"
        );
        assert!(
            snapshot["snapshot"]["plan"]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["nodeId"] == "fx::pass")
        );
    }

    #[tokio::test]
    async fn session_load_if_empty_rejects_loaded_session() {
        let app = runtime_router();
        let first = post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            session_load_request(sample_project_document_current()),
        )
        .await;
        let second = post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            session_load_request(sample_subpatch_project_document_current()),
        )
        .await;

        assert_eq!(first["ok"], true);
        assert_eq!(second["ok"], false);
        assert_eq!(
            second["diagnostics"][0]["code"],
            "runtime.session-load.conflict"
        );
        assert_eq!(
            second["diagnostics"][0]["details"]["current"]["documentId"],
            "10000000-0000-0000-0000-000000000001"
        );
        assert_eq!(second["snapshot"]["sessionRevision"], 1);
        assert_eq!(
            second["snapshot"]["project"]["graph"]["id"],
            "minimal-value"
        );

        let snapshot = get_json_with(app, "/v0/sessions/default/snapshot").await;
        assert_eq!(snapshot["snapshot"]["sessionRevision"], 1);
        assert_eq!(
            snapshot["snapshot"]["project"]["graph"]["id"],
            "minimal-value"
        );
    }

    #[tokio::test]
    async fn session_load_replace_if_match_enforces_preconditions() {
        let app = runtime_router();
        let loaded = post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            session_load_request(sample_project_document_current()),
        )
        .await;
        assert_eq!(loaded["ok"], true);

        let mut replacement = sample_subpatch_project_document_current();
        replacement["documentId"] = loaded["snapshot"]["project"]["documentId"].clone();
        let rejected = post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            session_load_request_with_mode(
                replacement.clone(),
                "replaceIfMatch",
                Some(json!({
                    "documentId": loaded["snapshot"]["project"]["documentId"],
                    "sessionRevision": "999",
                    "graphRevision": loaded["snapshot"]["project"]["graph"]["revision"],
                })),
            ),
        )
        .await;

        assert_eq!(rejected["ok"], false);
        assert_eq!(
            rejected["diagnostics"][0]["code"],
            "runtime.session-load.conflict"
        );
        assert_eq!(
            rejected["diagnostics"][0]["details"]["mismatches"][0]["field"],
            "sessionRevision"
        );
        assert_eq!(rejected["snapshot"]["sessionRevision"], 1);
        assert_eq!(
            rejected["snapshot"]["project"]["graph"]["id"],
            "minimal-value"
        );

        let accepted = post_json_with(
            app,
            "/v0/sessions/default/load",
            session_load_request_with_mode(
                replacement,
                "replaceIfMatch",
                Some(json!({
                    "documentId": loaded["snapshot"]["project"]["documentId"],
                    "sessionRevision": loaded["snapshot"]["sessionRevision"].to_string(),
                    "graphRevision": loaded["snapshot"]["project"]["graph"]["revision"],
                })),
            ),
        )
        .await;

        assert_eq!(accepted["ok"], true);
        assert_eq!(accepted["snapshot"]["sessionRevision"], 2);
        assert_eq!(
            accepted["snapshot"]["project"]["graph"]["id"],
            "subpatch-project-root"
        );
    }

    #[tokio::test]
    async fn session_load_force_replace_overwrites_loaded_session() {
        let app = runtime_router();
        let loaded = post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            session_load_request(sample_project_document_current()),
        )
        .await;
        assert_eq!(loaded["ok"], true);

        let replaced = post_json_with(
            app,
            "/v0/sessions/default/load",
            session_load_request_with_mode(
                sample_subpatch_project_document_current(),
                "forceReplace",
                None,
            ),
        )
        .await;

        assert_eq!(replaced["ok"], true);
        assert_eq!(replaced["snapshot"]["sessionRevision"], 2);
        assert_eq!(
            replaced["snapshot"]["project"]["documentId"],
            "10000000-0000-0000-0000-000000000002"
        );
        assert_eq!(
            replaced["snapshot"]["project"]["graph"]["id"],
            "subpatch-project-root"
        );
    }

    #[tokio::test]
    async fn session_load_rejects_missing_graph_schema_version() {
        let app = runtime_router();
        let response = post_json_with(
            app,
            "/v0/sessions/default/load",
            session_load_request(json!({ "graph": 42 })),
        )
        .await;

        assert_eq!(response["ok"], false);
        assert_eq!(response["snapshot"]["project"], Value::Null);
        assert_eq!(
            response["diagnostics"][0]["code"],
            "project.missing-schema-version"
        );
    }

    #[tokio::test]
    async fn legacy_import_routes_are_not_runtime_api_surface() {
        let app = runtime_router();
        let default_status = post_status_with(
            app.clone(),
            "/v0/sessions/default/import/legacy-v0.1",
            sample_project(),
        )
        .await;
        let named_status = post_status_with(
            app.clone(),
            "/v0/sessions/alpha/import/legacy-v0.1",
            sample_project(),
        )
        .await;

        assert_eq!(default_status, StatusCode::NOT_FOUND);
        assert_eq!(named_status, StatusCode::NOT_FOUND);

        for path in [
            "/v0/sessions/default/snapshot",
            "/v0/sessions/alpha/snapshot",
        ] {
            let snapshot = get_json_with(app.clone(), path).await;
            assert_eq!(snapshot["snapshot"]["project"], Value::Null);
            assert_eq!(snapshot["snapshot"]["sessionRevision"], 0);
        }
    }

    #[tokio::test]
    async fn default_session_uses_explicit_session_route_only() {
        let app = runtime_router();

        assert_eq!(
            status_with(app.clone(), "/v0/session").await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_with(app.clone(), "/v0/session/info").await,
            StatusCode::NOT_FOUND
        );

        let loaded = post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            session_load_request(sample_project_document_current()),
        )
        .await;
        let explicit = get_json_with(app.clone(), "/v0/sessions/default/snapshot").await;
        let info = get_json_with(app, "/v0/sessions/default/info").await;

        assert_eq!(loaded["ok"], true);
        assert_eq!(explicit["ok"], true);
        assert_eq!(loaded["snapshot"], explicit["snapshot"]);
        assert_eq!(info["sessionId"], DEFAULT_SESSION_ID);
        assert_eq!(
            info["snapshot"]["project"]["graph"]["id"],
            loaded["snapshot"]["project"]["graph"]["id"]
        );
        let info = serde_json::from_value::<RuntimeSessionInfoResponse>(info)
            .expect("session info should match contract shape");
        crate::validate_runtime_session_info_response(&info)
            .expect("session info should validate against runtime transport");
    }

    #[tokio::test]
    async fn invalid_session_load_does_not_replace_existing_session() {
        let app = runtime_router();
        let loaded = post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            session_load_request(sample_project_document_current()),
        )
        .await;
        let mut invalid = sample_project_document_current();
        invalid["nodes"] = json!([]);

        let response = post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            session_load_request(invalid),
        )
        .await;

        assert_eq!(loaded["snapshot"]["sessionRevision"], 1);
        assert_eq!(response["ok"], false);
        assert_eq!(
            response["snapshot"]["project"]["graph"]["id"],
            "minimal-value"
        );
        assert_eq!(response["snapshot"]["sessionRevision"], 1);
        assert_eq!(
            response["diagnostics"][0]["code"],
            "project.document.top-level-nodes-rejected"
        );

        let snapshot = get_json_with(app, "/v0/sessions/default/snapshot").await;
        assert_eq!(snapshot["ok"], true);
        assert_eq!(
            snapshot["snapshot"]["project"]["graph"]["id"],
            "minimal-value"
        );
        assert_eq!(
            snapshot["snapshot"]["diagnostics"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn invalid_session_load_returns_diagnostics_and_keeps_runtime_healthy() {
        let app = runtime_router();
        let mut invalid = sample_project_document_current();
        invalid["graph"]["nodes"][1]["ports"][1]["type"] = json!("value.core.bool");

        let response = post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            session_load_request(invalid),
        )
        .await;

        assert_eq!(response["ok"], false);
        assert_eq!(response["snapshot"]["project"], Value::Null);
        assert_eq!(response["snapshot"]["plan"], Value::Null);
        assert!(
            response["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| diagnostic["message"]
                    .as_str()
                    .unwrap()
                    .contains(
                        "edge edge_value_target cannot connect value_1:value value.core.float32 to target_1:cold value.core.bool",
                    ))
        );

        let health = get_json_with(app.clone(), "/health").await;
        assert_eq!(health["ok"], true);

        let snapshot = get_json_with(app.clone(), "/v0/sessions/default/snapshot").await;
        assert_eq!(snapshot["ok"], true);
        assert_eq!(snapshot["snapshot"]["project"], Value::Null);
        assert_eq!(snapshot["snapshot"]["sessionRevision"], 0);

        let logs = get_json_with(app, "/v0/runtime/logs").await;
        assert!(logs["events"].as_array().unwrap().iter().any(|event| {
            event["message"].as_str().unwrap().contains(
                "edge edge_value_target cannot connect value_1:value value.core.float32 to target_1:cold value.core.bool",
            )
        }));
    }

    #[tokio::test]
    async fn session_validate_plan_and_run_use_loaded_project_document_patch_library() {
        let app = runtime_router();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            session_load_request(sample_subpatch_project_document_current()),
        )
        .await;

        let validation = post_empty_with(app.clone(), "/v0/sessions/default/validate").await;
        assert_eq!(validation["ok"], true);
        assert_eq!(validation["diagnostics"].as_array().unwrap().len(), 0);

        let plan = post_empty_with(app.clone(), "/v0/sessions/default/plan").await;
        assert_eq!(plan["ok"], true);
        assert_eq!(plan["snapshot"]["project"]["id"], "subpatch-project");
        assert_eq!(
            plan["snapshot"]["project"]["patchLibrary"][0]["id"],
            "identity"
        );
        assert_eq!(plan["snapshot"]["plan"]["graphId"], "subpatch-project-root");
        assert!(
            plan["snapshot"]["plan"]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["nodeId"] == "fx::pass")
        );

        let run = post_json_with(app, "/v0/sessions/default/run", json!({ "frames": 2 })).await;
        assert_eq!(run["ok"], true);
        assert_eq!(run["report"]["frameCount"], 2);
        assert_eq!(
            run["report"]["frames"][0]["executedNodes"][0]["status"],
            "simulated"
        );
    }

    #[test]
    fn registry_from_nodes_reports_duplicate_definitions() {
        let nodes: Vec<NodeDefinition> = serde_json::from_value(sample_project()["nodes"].clone())
            .expect("sample project nodes should parse");
        let duplicate_nodes = vec![nodes[0].clone(), nodes[0].clone()];

        let diagnostics =
            registry_from_nodes(duplicate_nodes).expect_err("duplicate definitions should fail");

        assert!(diagnostics[0].message.contains("duplicate node definition"));
    }

    #[tokio::test]
    async fn session_run_fails_without_loaded_project() {
        let response = post_json("/v0/sessions/default/run", json!({ "frames": 2 })).await;

        assert_eq!(response["ok"], false);
        assert_eq!(response["snapshot"]["project"], Value::Null);
        assert!(
            response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("no project loaded in runtime session")
        );
    }

    #[tokio::test]
    async fn session_clear_removes_loaded_project() {
        let app = runtime_router();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            session_load_request(sample_project_document_current()),
        )
        .await;

        let response = delete_json_with(app, "/v0/sessions/default").await;

        assert_eq!(response["ok"], true);
        assert_eq!(response["snapshot"]["project"], Value::Null);
        assert_eq!(response["snapshot"]["sessionRevision"], 2);
        assert_eq!(response["snapshot"]["plan"], Value::Null);
    }

    #[tokio::test]
    async fn preview_status_reports_stopped_without_loaded_session() {
        let response = get_json_with(
            runtime_router_with_dry_preview(),
            "/v0/sessions/default/preview",
        )
        .await;

        assert_eq!(response["ok"], true);
        assert_eq!(response["state"], "stopped");
        assert_eq!(response["sessionRevision"], Value::Null);
        assert_eq!(response["previewSessionRevision"], Value::Null);
        assert_eq!(response["stale"], false);
    }

    #[tokio::test]
    async fn preview_start_requires_loaded_session() {
        let response = post_json_with(
            runtime_router_with_dry_preview(),
            "/v0/sessions/default/preview/start",
            json!({}),
        )
        .await;

        assert_eq!(response["ok"], false);
        assert_eq!(response["state"], "stopped");
        assert!(
            response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("no project loaded")
        );
    }

    #[tokio::test]
    async fn preview_start_stop_and_restart_use_session_plan() {
        let app = runtime_router_with_dry_preview();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            session_load_request(sample_project_document_current()),
        )
        .await;

        let started = post_empty_with(app.clone(), "/v0/sessions/default/preview/start").await;
        assert_eq!(started["ok"], true);
        assert_eq!(started["state"], "running");
        assert_eq!(started["graphId"], "minimal-value");
        assert_eq!(started["graphRevision"], "1");
        assert_eq!(started["sessionRevision"], 1);
        assert_eq!(started["previewSessionRevision"], 1);
        assert_eq!(started["stale"], false);

        let stopped = post_empty_with(app.clone(), "/v0/sessions/default/preview/stop").await;
        assert_eq!(stopped["ok"], true);
        assert_eq!(stopped["state"], "stopped");
        assert_eq!(stopped["graphId"], Value::Null);

        let restarted = post_empty_with(app, "/v0/sessions/default/preview/restart").await;
        assert_eq!(restarted["ok"], true);
        assert_eq!(restarted["state"], "running");
        assert_eq!(restarted["previewSessionRevision"], 1);
    }

    #[tokio::test]
    async fn preview_start_rejects_invalid_request_json() {
        let app = runtime_router_with_dry_preview();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            session_load_request(sample_project_document_current()),
        )
        .await;

        let response =
            post_raw_with(app, "/v0/sessions/default/preview/start", b"{".to_vec()).await;

        assert_eq!(response["ok"], false);
        assert_eq!(response["state"], "stopped");
        assert!(
            response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("invalid preview start request")
        );
    }

    #[tokio::test]
    async fn session_clear_stops_preview() {
        let app = runtime_router_with_dry_preview();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            session_load_request(sample_project_document_current()),
        )
        .await;
        post_empty_with(app.clone(), "/v0/sessions/default/preview/start").await;

        let cleared = delete_json_with(app.clone(), "/v0/sessions/default").await;
        assert_eq!(cleared["ok"], true);

        let preview = get_json_with(app, "/v0/sessions/default/preview").await;
        assert_eq!(preview["state"], "stopped");
        assert_eq!(preview["sessionRevision"], Value::Null);
        assert_eq!(preview["stale"], false);
    }

    #[tokio::test]
    async fn telemetry_endpoint_reports_empty_session() {
        let response = get_json_with(
            runtime_router_with_dry_preview(),
            "/v0/sessions/default/telemetry",
        )
        .await;

        assert_eq!(response["schema"], "skenion.runtime.telemetry");
        assert_eq!(response["schemaVersion"], "0.1.0");
        assert_eq!(response["ok"], true);
        assert_eq!(response["session"]["project"], Value::Null);
        assert_eq!(response["preview"]["state"], "stopped");
        assert_eq!(response["render"]["active"], false);
        assert_eq!(response["render"]["diagnostics"], json!([]));
        assert_eq!(response["render"]["generatedSourceAvailable"], false);
        assert_eq!(
            response["process"]["runtimeVersion"],
            env!("CARGO_PKG_VERSION")
        );
    }

    #[tokio::test]
    async fn telemetry_endpoint_reports_loaded_session_without_preview() {
        let app = runtime_router_with_dry_preview();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            session_load_request(sample_project_document_current()),
        )
        .await;

        let response = get_json_with(app, "/v0/sessions/default/telemetry").await;

        assert_eq!(response["session"]["loaded"], true);
        assert_eq!(response["session"]["graphId"], "minimal-value");
        assert_eq!(response["session"]["graphRevision"], "1");
        assert_eq!(response["session"]["sessionRevision"], 1);
        assert_eq!(response["preview"]["state"], "stopped");
        assert_eq!(response["render"]["active"], false);
        assert_eq!(response["render"]["diagnostics"], json!([]));
        assert_eq!(response["render"]["generatedSourceAvailable"], false);
    }

    #[tokio::test]
    async fn telemetry_endpoint_reports_dry_run_preview() {
        let app = runtime_router_with_dry_preview();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            session_load_request(sample_project_document_current()),
        )
        .await;
        post_empty_with(app.clone(), "/v0/sessions/default/preview/start").await;

        let response = get_json_with(app, "/v0/sessions/default/telemetry").await;

        assert_eq!(response["preview"]["state"], "running");
        assert_eq!(response["preview"]["stale"], false);
        assert_eq!(response["render"]["active"], true);
        assert_eq!(response["render"]["backend"], "dry-run");
        assert_eq!(response["render"]["renderer"], "clear-color");
        assert_eq!(response["render"]["framesRendered"], 0);
        assert_eq!(response["render"]["diagnostics"], json!([]));
        assert_eq!(response["render"]["generatedSourceAvailable"], false);
    }

    #[tokio::test]
    async fn generated_shader_endpoint_returns_source_and_source_map() {
        let app = runtime_router_with_loaded_shader_dry_preview(sample_shader_project_current());

        let response = get_json_with(app, "/v0/sessions/default/render/generated-shader").await;
        assert_eq!(response["ok"], true);
        assert_eq!(response["nodeId"], "shader_1");
        assert_eq!(response["language"], "wgsl");
        assert!(
            response["source"]
                .as_str()
                .unwrap()
                .contains("struct SkenionFrame")
        );
        assert!(response["source"].as_str().unwrap().contains("speed: f32"));
        assert!(response["source"].as_str().unwrap().contains("fn fs_main"));
        assert!(
            response["sourceMap"]["userSourceStartLine"]
                .as_u64()
                .unwrap()
                > 1
        );
        assert_eq!(response["diagnostics"], json!([]));
    }

    #[tokio::test]
    async fn generated_shader_endpoint_reports_session_or_shader_diagnostics() {
        let empty = get_json_with(
            runtime_router_with_dry_preview(),
            "/v0/sessions/default/render/generated-shader",
        )
        .await;
        assert_eq!(empty["ok"], false);
        assert_eq!(empty["diagnostics"][0]["phase"], json!("source-sync"));

        let mut project = sample_shader_project_current();
        project["graph"]["nodes"][0]["params"]["source"] = json!(
            "// @skenion.uniform bad vec3\n@fragment\nfn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }"
        );
        let app = runtime_router_with_loaded_shader_dry_preview(project);

        let response = get_json_with(app, "/v0/sessions/default/render/generated-shader").await;
        assert_eq!(response["ok"], false);
        assert_eq!(
            response["diagnostics"][0]["phase"],
            json!("interface-analysis")
        );
        assert_eq!(
            response["diagnostics"][0]["code"],
            json!("unsupported-uniform-type")
        );
        assert_eq!(response["diagnostics"][0]["line"], json!(1));
    }

    #[tokio::test]
    async fn telemetry_stream_endpoint_returns_sse_response() {
        let response = runtime_router_with_dry_preview()
            .oneshot(
                Request::builder()
                    .uri("/v0/sessions/default/telemetry/stream")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("text/event-stream")
        );
        let mut stream = response.into_body().into_data_stream();
        let chunk = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("telemetry stream should emit")
            .expect("telemetry stream should have a chunk")
            .expect("telemetry stream chunk should be ok");
        let text = std::str::from_utf8(&chunk).expect("telemetry stream should be utf8");
        assert!(text.contains("event: telemetry"));
        assert!(text.contains("skenion.runtime.telemetry"));
    }

    async fn get_json(path: &str) -> Value {
        get_json_with(runtime_router(), path).await
    }

    async fn get_json_with(app: Router, path: &str) -> Value {
        let response = app
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        assert_eq!(response.status(), StatusCode::OK);
        body_json(response.into_body()).await
    }

    async fn status_with(app: Router, path: &str) -> StatusCode {
        app.oneshot(
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond")
        .status()
    }

    async fn post_json(path: &str, payload: Value) -> Value {
        post_json_with(runtime_router(), path, payload).await
    }

    fn session_load_request(project: Value) -> Value {
        session_load_request_with_mode(project, "loadIfEmpty", None)
    }

    fn session_load_request_with_mode(
        project: Value,
        mode: &str,
        precondition: Option<Value>,
    ) -> Value {
        let mut request = json!({
            "schema": "skenion.runtime.session-load-request",
            "schemaVersion": "0.1.0",
            "project": project,
            "mode": mode
        });
        if let Some(precondition) = precondition {
            request["precondition"] = precondition;
        }
        request
    }

    async fn post_json_with(app: Router, path: &str, payload: Value) -> Value {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(payload.to_string()))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        assert_eq!(response.status(), StatusCode::OK);
        body_json(response.into_body()).await
    }

    async fn request_json_status_with(
        app: Router,
        method: Method,
        path: &str,
        payload: Value,
    ) -> (StatusCode, Value) {
        let body = if method == Method::GET {
            Body::empty()
        } else {
            Body::from(payload.to_string())
        };
        let response = app
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(CONTENT_TYPE, "application/json")
                    .body(body)
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        let status = response.status();
        (status, body_json(response.into_body()).await)
    }

    async fn post_status_with(app: Router, path: &str, payload: Value) -> StatusCode {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(payload.to_string()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond")
        .status()
    }

    async fn post_raw_with(app: Router, path: &str, payload: Vec<u8>) -> Value {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(payload))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        assert_eq!(response.status(), StatusCode::OK);
        body_json(response.into_body()).await
    }

    async fn post_empty_with(app: Router, path: &str) -> Value {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        assert_eq!(response.status(), StatusCode::OK);
        body_json(response.into_body()).await
    }

    async fn delete_json_with(app: Router, path: &str) -> Value {
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(path)
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        assert_eq!(response.status(), StatusCode::OK);
        body_json(response.into_body()).await
    }

    async fn body_json(body: Body) -> Value {
        let bytes = to_bytes(body, usize::MAX)
            .await
            .expect("body should collect");
        serde_json::from_slice(&bytes).expect("body should be json")
    }

    fn runtime_router_with_dry_preview() -> Router {
        runtime_router_with_state(runtime_state_with_dry_preview())
    }

    fn runtime_router_with_loaded_shader_dry_preview(project: Value) -> Router {
        let state = runtime_state_with_dry_preview();
        let request = serde_json::from_value::<ProjectRequestCurrent>(project)
            .expect("shader test project request should parse");
        let record = state.sessions.default_record();
        {
            let mut session = record
                .session
                .write()
                .expect("runtime session lock should not be poisoned");
            let response = session.load_project_current_with_package_registry_revision(
                request,
                Some(state.packages.revision()),
            );
            assert!(
                response.ok,
                "shader test project should load: {:?}",
                response.diagnostics
            );
        }
        runtime_router_with_state(state)
    }

    fn runtime_state_with_dry_preview() -> RuntimeServerState {
        let logs = std::sync::Arc::new(RuntimeLogStore::default());
        RuntimeServerState {
            sessions: RuntimeSessionRegistry::dry_preview(),
            assets: std::sync::Arc::new(std::sync::RwLock::new(RuntimeAssetStore::default())),
            io_devices: std::sync::Arc::new(RuntimeIoDeviceManager::new()),
            extensions: std::sync::Arc::new(RuntimeExtensionRegistrySnapshot::default()),
            packages: std::sync::Arc::new(RuntimePackageRegistrySnapshot::default()),
            logs,
            endpoint: RuntimeEndpointConfig::new(DEFAULT_HOST.to_owned(), DEFAULT_PORT),
            started_at_wall_clock: created_at_now(),
            started_at: std::time::Instant::now(),
        }
    }

    fn runtime_router_with_fake_io_devices(devices: Vec<RuntimeIoDeviceDescriptor>) -> Router {
        let logs = std::sync::Arc::new(RuntimeLogStore::default());
        runtime_router_with_state(RuntimeServerState {
            sessions: RuntimeSessionRegistry::dry_preview(),
            assets: std::sync::Arc::new(std::sync::RwLock::new(RuntimeAssetStore::default())),
            io_devices: std::sync::Arc::new(RuntimeIoDeviceManager::with_device_registry(
                Arc::new(ServerFakeIoDeviceRegistry { devices }),
            )),
            extensions: std::sync::Arc::new(RuntimeExtensionRegistrySnapshot::default()),
            packages: std::sync::Arc::new(RuntimePackageRegistrySnapshot::default()),
            logs,
            endpoint: RuntimeEndpointConfig::new(DEFAULT_HOST.to_owned(), DEFAULT_PORT),
            started_at_wall_clock: created_at_now(),
            started_at: std::time::Instant::now(),
        })
    }

    fn runtime_router_with_extension_package_dirs(package_dirs: Vec<PathBuf>) -> Router {
        let logs = Arc::new(RuntimeLogStore::default());
        let extension_scan =
            RuntimeExtensionManager::with_package_dirs(package_dirs).scan_registry();
        logs.record_runtime_diagnostics(extension_scan.log_diagnostics());
        runtime_router_with_state(RuntimeServerState {
            sessions: RuntimeSessionRegistry::dry_preview(),
            assets: Arc::new(std::sync::RwLock::new(RuntimeAssetStore::default())),
            io_devices: Arc::new(RuntimeIoDeviceManager::new()),
            extensions: Arc::new(extension_scan.into_snapshot()),
            packages: Arc::new(RuntimePackageRegistrySnapshot::default()),
            logs,
            endpoint: RuntimeEndpointConfig::new(DEFAULT_HOST.to_owned(), DEFAULT_PORT),
            started_at_wall_clock: created_at_now(),
            started_at: std::time::Instant::now(),
        })
    }

    fn runtime_router_with_package_dirs(
        package_dirs: Vec<PathBuf>,
    ) -> (Router, RuntimeServerState) {
        let logs = Arc::new(RuntimeLogStore::default());
        let package_scan = RuntimePackageManager::with_package_dirs(package_dirs).scan_registry();
        logs.record_runtime_diagnostics(package_scan.log_diagnostics());
        let state = RuntimeServerState {
            sessions: RuntimeSessionRegistry::dry_preview(),
            assets: Arc::new(std::sync::RwLock::new(RuntimeAssetStore::default())),
            io_devices: Arc::new(RuntimeIoDeviceManager::new()),
            extensions: Arc::new(RuntimeExtensionRegistrySnapshot::default()),
            packages: Arc::new(package_scan.into_snapshot()),
            logs,
            endpoint: RuntimeEndpointConfig::new(DEFAULT_HOST.to_owned(), DEFAULT_PORT),
            started_at_wall_clock: created_at_now(),
            started_at: std::time::Instant::now(),
        };
        (runtime_router_with_state(state.clone()), state)
    }

    fn server_temp_extension_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "skenion-runtime-server-extension-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("extension temp dir should create");
        dir
    }

    fn server_temp_package_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "skenion-runtime-server-package-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("package temp dir should create");
        dir
    }

    fn write_server_extension_manifest(package_dir: &Path, body: &str) {
        std::fs::write(
            package_dir.join(crate::RUNTIME_EXTENSION_MANIFEST_FILE),
            body,
        )
        .expect("extension manifest should write");
    }

    fn write_server_valid_extension_manifest(package_dir: &Path) {
        write_server_extension_manifest(
            package_dir,
            r#"{
              "schema": "skenion.extension.manifest",
              "schemaVersion": "0.1.0",
              "id": "example/server-success",
              "version": "0.1.0",
              "runtimeAbiVersion": "0.1.0",
              "kind": "node-pack",
              "provides": {},
              "permissions": []
            }"#,
        );
    }

    fn write_server_package_manifest(package_dir: &Path, body: &str) {
        std::fs::write(package_dir.join(crate::RUNTIME_PACKAGE_MANIFEST_FILE), body)
            .expect("package manifest should write");
    }

    fn write_server_valid_package_manifest(package_dir: &Path, package_id: &str) {
        let provided_id = package_id.replace('/', ".");
        write_server_package_manifest(
            package_dir,
            &format!(
                r#"{{
                  "schema": "skenion.package.manifest",
                  "schemaVersion": "0.1.0",
                  "id": "{package_id}",
                  "version": "0.49.0",
                  "category": "patch",
                  "contracts": {{
                    "line": "0.49",
                    "range": ">=0.49.0 <0.50.0"
                  }},
                  "provides": {{
                    "patches": [
                      {{
                        "id": "{provided_id}.main",
                        "path": "patches/main.skenion.json"
                      }}
                    ]
                  }},
                  "paths": {{
                    "patches": ["patches/main.skenion.json"]
                  }},
                  "checksums": [
                    {{
                      "id": "manifest",
                      "path": "skenion.package.json",
                      "checksum": {{
                        "algorithm": "sha256",
                        "value": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                      }}
                    }}
                  ],
                  "evidence": [
                    {{
                      "id": "manifest-checksum",
                      "kind": "checksum",
                      "path": "evidence/manifest.sha256",
                      "checksum": {{
                        "algorithm": "sha256",
                        "value": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                      }}
                    }}
                  ]
                }}"#
            ),
        );
    }

    fn sample_project() -> Value {
        json!({
              "graph": {
                "schema": "skenion.graph",
                "schemaVersion": "0.1.0",
                "id": "minimal-value",
                "revision": "1",
                "nodes": [
                  {
                    "id": "value_1",
                    "kind": "object.core.float",
                    "kindVersion": "0.1.0",
                    "params": {},
                    "ports": value_f32_ports_json()
                  },
                  {
                    "id": "target_1",
                    "kind": "object.core.float",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": value_f32_ports_json()
              }
            ],
            "edges": [
              { "from": { "node": "value_1", "port": "value" }, "to": { "node": "target_1", "port": "in" } }
            ]
          },
          "nodes": [
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "object.core.float",
              "version": "0.1.0",
              "displayName": "Float",
              "category": "Typed Controls",
              "ports": value_f32_ports_json(),
              "execution": { "model": "control" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": ["value.core.float32.v0.1"]
            }
          ]
        })
    }

    fn sample_project_document_current() -> Value {
        json!({
          "schema": "skenion.project",
          "schemaVersion": "0.1.0",
          "id": "minimal-value-project",
          "documentId": "10000000-0000-0000-0000-000000000001",
          "revision": "1",
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": "minimal-value",
            "revision": "1",
            "nodes": [
              {
                "id": "value_1",
                "kind": "object.core.float",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": value_f32_ports_current_json()
              },
              {
                "id": "target_1",
                "kind": "object.core.float",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": value_f32_ports_current_json()
              }
            ],
            "edges": [
              {
                "id": "edge_value_target",
                "source": { "nodeId": "value_1", "portId": "value" },
                "target": { "nodeId": "target_1", "portId": "cold" },
                "resolvedType": "value.core.float32"
              }
            ]
          },
          "viewState": {
            "schema": "skenion.view-state",
            "schemaVersion": "0.1.0",
            "canvas": {
              "nodes": {
                "value_1": { "x": 96.0, "y": 96.0 },
                "target_1": { "x": 260.0, "y": 96.0 }
              }
            }
          },
          "patchLibrary": []
        })
    }

    fn value_f32_node_definition_current_json() -> Value {
        json!({
          "schema": "skenion.node.definition",
          "schemaVersion": "0.1.0",
          "id": "object.core.float",
          "version": "0.1.0",
          "displayName": "Float",
          "category": "Typed Controls",
          "ports": value_f32_ports_current_json(),
          "execution": { "model": "control" },
          "state": { "persistent": false },
          "permissions": [],
          "capabilities": ["value.core.float32.v0.1"]
        })
    }

    fn value_f32_ports_json() -> Value {
        json!([
          {
            "id": "in",
            "direction": "input",
            "label": "In",
            "type": { "flow": "control", "dataKind": "value.core.message" },
            "required": false,
            "activation": "trigger"
          },
          {
            "id": "cold",
            "direction": "input",
            "label": "Cold",
            "type": { "flow": "control", "dataKind": "value.core.float32" },
            "required": false,
            "activation": "latched"
          },
          {
            "id": "value",
            "direction": "output",
            "label": "Value",
            "type": { "flow": "control", "dataKind": "value.core.float32" }
          }
        ])
    }

    fn sample_shader_project_current() -> Value {
        json!({
          "schema": "skenion.project",
          "schemaVersion": "0.1.0",
          "id": "shader-diagnostics-project",
          "documentId": "10000000-0000-0000-0000-000000000003",
          "revision": "1",
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": "shader-diagnostics",
            "revision": "1",
            "nodes": [
              {
                "id": "shader_1",
                "kind": "object.core.render.fullscreen-shader",
                "kindVersion": "0.1.0",
                "params": {
                  "language": "wgsl",
                  "source": "// @skenion.uniform speed value.core.float32 default=0.5\n@fragment\nfn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(skenion.speed, 0.0, 1.0, 1.0); }"
                },
                "ports": [
                {
                  "id": "speed",
                  "direction": "input",
                  "label": "Speed",
                  "type": "value.core.float32",
                  "rate": "control",
                  "required": false,
                  "defaultValue": 0.5,
                  "triggerMode": "latched"
                },
                {
                  "id": "out",
                  "direction": "output",
                  "label": "Out",
                  "type": "value.core.tensor",
                  "rate": "resource"
                }
                ]
              }
            ],
            "edges": []
          },
          "nodes": [
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "object.core.render.fullscreen-shader",
              "version": "0.1.0",
              "displayName": "Fullscreen Shader",
              "category": "Render",
              "ports": [
                {
                  "id": "speed",
                  "direction": "input",
                  "label": "Speed",
                  "type": "value.core.float32",
                  "rate": "control",
                  "required": false,
                  "defaultValue": 0.5,
                  "triggerMode": "latched"
                },
                {
                  "id": "out",
                  "direction": "output",
                  "label": "Out",
                  "type": "value.core.tensor",
                  "rate": "resource"
                }
              ],
              "execution": { "model": "gpu_pass", "clock": "frame" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": []
            }
          ],
          "viewState": {
            "schema": "skenion.view-state",
            "schemaVersion": "0.1.0",
            "canvas": {
              "nodes": {
                "shader_1": { "x": 96.0, "y": 96.0 }
              }
            }
          },
          "patchLibrary": []
        })
    }

    fn sample_project_current() -> Value {
        json!({
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": "render-output-current",
            "revision": "1",
            "nodes": [
              {
                "id": "clear_color",
                "kind": "object.core.render.clear-color",
                "kindVersion": "0.1.0",
                "params": { "color": [0.12, 0.2, 0.34, 1] },
                "ports": [
                  {
                    "id": "out",
                    "direction": "output",
                    "type": "value.core.tensor",
                    "rate": "render"
                  }
                ]
              },
              {
                "id": "output",
                "kind": "object.core.render.output",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": [
                  {
                    "id": "in",
                    "direction": "input",
                    "type": "value.core.tensor",
                    "rate": "render",
                    "required": true
                  }
                ]
              }
            ],
            "edges": [
              {
                "id": "edge_clear_output",
                "source": { "nodeId": "clear_color", "portId": "out" },
                "target": { "nodeId": "output", "portId": "in" },
                "resolvedType": "value.core.tensor"
              }
            ]
          },
          "nodes": [
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "object.core.render.clear-color",
              "version": "0.1.0",
              "displayName": "Clear Color",
              "category": "Render",
              "ports": [
                {
                  "id": "out",
                  "direction": "output",
                  "type": "value.core.tensor",
                  "rate": "render"
                }
              ],
              "execution": { "model": "gpu_pass", "clock": "frame" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": ["value.core.tensor.v0.1"]
            },
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "object.core.render.output",
              "version": "0.1.0",
              "displayName": "Render Output",
              "category": "Render",
              "ports": [
                {
                  "id": "in",
                  "direction": "input",
                  "type": "value.core.tensor",
                  "rate": "render",
                  "required": true
                }
              ],
              "execution": { "model": "gpu_pass", "clock": "frame" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": ["object.core.render.output.v0.1"]
            }
          ]
        })
    }

    fn sample_subpatch_project_document_current() -> Value {
        json!({
          "schema": "skenion.project",
          "schemaVersion": "0.1.0",
          "id": "subpatch-project",
          "documentId": "10000000-0000-0000-0000-000000000002",
          "revision": "1",
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": "subpatch-project-root",
            "revision": "1",
            "nodes": [
              {
                "id": "clear_color",
                "kind": "object.core.float",
                "kindVersion": "0.1.0",
                "params": { "value": 0.25 },
                "ports": value_f32_ports_current_json()
              },
              {
                "id": "fx",
                "kind": "object.core.subpatch",
                "kindVersion": "0.1.0",
                "params": { "patchRef": "identity" },
                "ports": [
                  { "id": "in", "direction": "input", "type": "value.core.float32", "rate": "control", "required": true },
                  { "id": "out", "direction": "output", "type": "value.core.float32", "rate": "control" }
                ]
              },
              {
                "id": "output",
                "kind": "object.core.float",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": value_f32_ports_current_json()
              }
            ],
            "edges": [
              {
                "id": "edge_clear_fx",
                "source": { "nodeId": "clear_color", "portId": "value" },
                "target": { "nodeId": "fx", "portId": "in" },
                "resolvedType": "value.core.float32"
              },
              {
                "id": "edge_fx_output",
                "source": { "nodeId": "fx", "portId": "out" },
                "target": { "nodeId": "output", "portId": "cold" },
                "resolvedType": "value.core.float32"
              }
            ]
          },
          "viewState": {
            "schema": "skenion.view-state",
            "schemaVersion": "0.1.0",
            "canvas": { "nodes": {} }
          },
          "patchLibrary": [
            {
              "id": "identity",
              "revision": "1",
              "metadata": { "title": "Identity Frame" },
              "graph": {
                "schema": "skenion.graph",
                "schemaVersion": "0.1.0",
                "id": "identity-frame-patch",
                "revision": "1",
                "nodes": [
                  {
                    "id": "patch_in",
                    "kind": "object.core.inlet",
                    "kindVersion": "0.1.0",
                    "params": { "portId": "in", "label": "Input" },
                    "ports": [
                      { "id": "out", "direction": "output", "type": "value.core.float32", "rate": "control", "description": "Value entering the patch" }
                    ]
                  },
                  {
                    "id": "pass",
                    "kind": "object.core.float",
                    "kindVersion": "0.1.0",
                    "params": {},
                    "ports": value_f32_ports_current_json()
                  },
                  {
                    "id": "patch_out",
                    "kind": "object.core.outlet",
                    "kindVersion": "0.1.0",
                    "params": { "portId": "out", "label": "Output" },
                    "ports": [
                      { "id": "in", "direction": "input", "type": "value.core.float32", "rate": "control", "required": true, "description": "Value leaving the patch" }
                    ]
                  }
                ],
                "edges": [
                  {
                    "id": "edge_in_pass",
                    "source": { "nodeId": "patch_in", "portId": "out" },
                    "target": { "nodeId": "pass", "portId": "in" },
                    "resolvedType": "value.core.float32"
                  },
                  {
                    "id": "edge_pass_out",
                    "source": { "nodeId": "pass", "portId": "value" },
                    "target": { "nodeId": "patch_out", "portId": "in" },
                    "resolvedType": "value.core.float32"
                  }
                ]
              }
            }
          ]
        })
    }

    fn sample_ambiguous_loop_project_current() -> Value {
        json!({
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": "ambiguous-algebraic-loop-current",
            "revision": "1",
            "nodes": [
              {
                "id": "a",
                "kind": "object.core.float-transform",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": [
                  { "id": "in", "direction": "input", "type": "value.core.float32", "rate": "control" },
                  { "id": "out", "direction": "output", "type": "value.core.float32", "rate": "control" }
                ]
              },
              {
                "id": "b",
                "kind": "object.core.float-transform",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": [
                  { "id": "in", "direction": "input", "type": "value.core.float32", "rate": "control" },
                  { "id": "out", "direction": "output", "type": "value.core.float32", "rate": "control" }
                ]
              }
            ],
            "edges": [
              {
                "id": "edge_a_b",
                "source": { "nodeId": "a", "portId": "out" },
                "target": { "nodeId": "b", "portId": "in" }
              },
              {
                "id": "edge_b_a",
                "source": { "nodeId": "b", "portId": "out" },
                "target": { "nodeId": "a", "portId": "in" }
              }
            ]
          },
          "nodes": [
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "object.core.float-transform",
              "version": "0.1.0",
              "displayName": "Value Transform",
              "category": "Core",
              "ports": [
                { "id": "in", "direction": "input", "type": "value.core.float32", "rate": "control" },
                { "id": "out", "direction": "output", "type": "value.core.float32", "rate": "control" }
              ],
              "execution": { "model": "control" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": ["value.core.float32.v0.1"]
            }
          ]
        })
    }

    fn value_f32_ports_current_json() -> Value {
        json!([
          {
            "id": "in",
            "direction": "input",
            "label": "In",
            "type": "value.core.message",
            "rate": "control",
            "required": false,
            "triggerMode": "trigger",
            "accepts": [
              "value.core.float32",
              "value.core.int32",
              "value.core.uint32",
              "value.core.bool",
              "value.core.bang"
            ],
            "messageKeys": {
              "accepted": ["bang", "set", "float", "int", "uint", "bool"],
              "silent": ["set"],
              "trigger": ["bang", "float", "int", "uint", "bool"],
              "store": ["set", "float", "int", "uint", "bool"],
              "emit": ["bang", "float", "int", "uint", "bool"]
            }
          },
          {
            "id": "cold",
            "direction": "input",
            "label": "Cold",
            "type": "value.core.float32",
            "rate": "control",
            "required": false,
            "triggerMode": "passive"
          },
          {
            "id": "value",
            "direction": "output",
            "label": "Value",
            "type": "value.core.float32",
            "rate": "control"
          }
        ])
    }
}
