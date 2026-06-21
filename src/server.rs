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
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{HeaderMap, HeaderValue, Method, header::CONTENT_TYPE},
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use skenion_contracts::RuntimeSessionInfoResponse;
use tokio_stream::{
    Stream, StreamExt,
    wrappers::{BroadcastStream, IntervalStream, errors::BroadcastStreamRecvError},
};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::{
    DummyExecutionReport, ExecutionPlan, GeneratedShaderResponse, GraphDocument, NodeDefinition,
    NodeDefinitionV02, NodeRegistry, PreviewDocument, ProjectDocumentV02, ProjectRequestV02,
    RunProjectRequestV02, RuntimeControlEventRequest, RuntimeControlEventResponse,
    RuntimeControlReadRequest, RuntimeControlReadResponse, RuntimeControlStateResponse,
    RuntimeExtensionListResponse, RuntimeExtensionManager, RuntimeIoDeviceListResponse,
    RuntimeIoDeviceManager, RuntimeLogSnapshotResponse, RuntimeLogStore, RuntimeMutationRequest,
    RuntimeOperationEnvelope, RuntimePreviewStartRequest, RuntimeTelemetrySnapshot,
    SessionRunRequest, ShaderDiagnostic, ShaderDiagnosticPhase, ShaderDiagnosticSource, ViewState,
    build_execution_plan, build_execution_plan_request_v02, build_execution_plan_run_request_v02,
    generated_shader_response_from_preview_document, run_dummy_execution,
    runtime_time::created_at_now,
    session_registry::{
        DEFAULT_SESSION_ID, RuntimeSessionEventKind, RuntimeSessionRecord, RuntimeSessionRegistry,
        SessionEventsQuery, event_cursor_from_headers, publish_session_event,
        replay_session_events, session_broadcast_event, session_event, session_event_from_session,
    },
    sidecar::{
        RuntimeEndpointConfig, RuntimeSidecarShutdownResponse, RuntimeSidecarStartupResponse,
        runtime_connection_profile, sidecar_shutdown_response, sidecar_startup_response,
    },
    validate_project, validate_project_request_v02,
};

pub const RUNTIME_API_VERSION: &str = "0.1.0";
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 3761;
const MAX_ASSET_UPLOAD_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRequest {
    pub graph: GraphDocument,
    pub nodes: Vec<NodeDefinition>,
    #[serde(default)]
    pub view_state: Option<ViewState>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunProjectRequest {
    pub graph: GraphDocument,
    pub nodes: Vec<NodeDefinition>,
    pub frames: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub ok: bool,
    pub service: &'static str,
    pub version: &'static str,
    pub api_version: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfoResponse {
    pub name: &'static str,
    pub version: &'static str,
    pub api_version: &'static str,
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
    pub extensions: Arc<RuntimeExtensionManager>,
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
        Self {
            sessions: RuntimeSessionRegistry::default(),
            assets: Arc::new(RwLock::new(RuntimeAssetStore::default())),
            io_devices: Arc::new(RuntimeIoDeviceManager::new()),
            extensions: Arc::new(RuntimeExtensionManager::from_env()),
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
        .route("/v0/runtime/logs", get(runtime_logs))
        .route("/v0/runtime/logs/stream", get(runtime_logs_stream))
        .route("/v0/io/devices", get(io_devices))
        .route("/v0/validate", post(validate_project_endpoint))
        .route("/v0/plan", post(plan_project_endpoint))
        .route("/v0/run", post(run_project_endpoint))
        .route(
            "/v0/sessions/{session_id}",
            get(session_snapshot_by_id).delete(clear_session_by_id),
        )
        .route("/v0/sessions/{session_id}/info", get(session_info_by_id))
        .route(
            "/v0/sessions/{session_id}/events/stream",
            get(session_events_stream_by_id),
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
            post(mutate_session_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/operation",
            post(apply_session_operation_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/history",
            get(session_history_by_id),
        )
        .route("/v0/sessions/{session_id}/undo", post(undo_session_by_id))
        .route("/v0/sessions/{session_id}/redo", post(redo_session_by_id))
        .route(
            "/v0/sessions/{session_id}/control/event",
            post(control_event_by_id),
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
        .route("/v0/session", get(session_snapshot).delete(clear_session))
        .route("/v0/session/info", get(session_info))
        .route("/v0/session/events/stream", get(session_events_stream))
        .route("/v0/session/load", post(load_session))
        .route("/v0/session/validate", post(validate_session))
        .route("/v0/session/plan", post(plan_session))
        .route("/v0/session/run", post(run_session))
        .route("/v0/session/mutate", post(mutate_session))
        .route("/v0/session/operation", post(apply_session_operation))
        .route("/v0/session/history", get(session_history))
        .route("/v0/session/undo", post(undo_session))
        .route("/v0/session/redo", post(redo_session))
        .route("/v0/session/control/event", post(control_event))
        .route("/v0/session/control/state", get(control_state))
        .route("/v0/session/control/read", post(control_read))
        .route("/v0/session/preview", get(preview_status))
        .route("/v0/session/preview/start", post(start_preview))
        .route("/v0/session/preview/stop", post(stop_preview))
        .route("/v0/session/preview/restart", post(restart_preview))
        .route("/v0/session/render/generated-shader", get(generated_shader))
        .route(
            "/v0/assets/import",
            post(import_asset).layer(DefaultBodyLimit::max(MAX_ASSET_UPLOAD_BYTES)),
        )
        .route("/v0/assets", get(list_assets))
        .route("/v0/assets/{asset_id}", get(get_asset))
        .route("/v0/session/telemetry", get(session_telemetry))
        .route(
            "/v0/session/telemetry/stream",
            get(session_telemetry_stream),
        )
        .with_state(state)
        .layer(cors_layer())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        service: "skenion-runtime",
        version: env!("CARGO_PKG_VERSION"),
        api_version: RUNTIME_API_VERSION,
    })
}

async fn runtime_info() -> Json<RuntimeInfoResponse> {
    Json(RuntimeInfoResponse {
        name: "skenion-runtime",
        version: env!("CARGO_PKG_VERSION"),
        api_version: RUNTIME_API_VERSION,
        capabilities: vec![
            "project.validate",
            "project.validate.v0.2",
            "project.plan",
            "project.plan.v0.2",
            "dummy.run",
            "session.load",
            "session.project",
            "session.events.stream",
            "session.validate",
            "session.plan",
            "session.run",
            "session.mutate",
            "session.operation",
            "session.history",
            "session.undo",
            "session.redo",
            "session.clear",
            "session.addressing",
            "session.defaultAlias",
            "session.info",
            "session.events.replay",
            "session.control.event",
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

async fn sidecar_startup(
    State(state): State<RuntimeServerState>,
) -> Json<RuntimeSidecarStartupResponse> {
    Json(state.sidecar_startup_response())
}

async fn sidecar_health(
    State(state): State<RuntimeServerState>,
) -> Json<RuntimeSidecarStartupResponse> {
    Json(state.sidecar_startup_response())
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
    Json(state.extensions.list_extensions())
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

async fn session_events_stream(
    State(state): State<RuntimeServerState>,
    Query(query): Query<SessionEventsQuery>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    session_events_stream_for(state.sessions.default_record(), query, headers)
}

async fn session_events_stream_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionEventsQuery>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    session_events_stream_for(state.sessions.get_or_create(&session_id), query, headers)
}

fn session_events_stream_for(
    record: RuntimeSessionRecord,
    query: SessionEventsQuery,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let after = query
        .after
        .or_else(|| event_cursor_from_headers(&headers))
        .map(|sequence| sequence.max(1));
    let snapshot = {
        let session = record
            .session
            .read()
            .expect("runtime session lock should not be poisoned");
        session_event_from_session(
            &record,
            RuntimeSessionEventKind::Snapshot,
            &session,
            false,
            None,
            session.snapshot().diagnostics,
        )
    };
    let replay_events = replay_session_events(&record, after, snapshot);
    let replay = tokio_stream::iter(replay_events.into_iter().map(session_event));
    let session_id = record.id.clone();
    let live = BroadcastStream::new(record.events.subscribe())
        .map(move |result| session_broadcast_event(result, session_id.clone()));
    Sse::new(replay.chain(live)).keep_alive(KeepAlive::default())
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

fn patch_json(
    state: &RuntimeServerState,
    response: crate::RuntimePatchResponse,
) -> Json<crate::RuntimePatchResponse> {
    state.logs.record_runtime_diagnostics(&response.diagnostics);
    Json(response)
}

fn paste_operation_json(
    response: crate::PasteGraphFragmentResponse,
) -> Json<crate::PasteGraphFragmentResponse> {
    Json(response)
}

fn control_event_json(
    state: &RuntimeServerState,
    response: RuntimeControlEventResponse,
) -> Json<RuntimeControlEventResponse> {
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
        Ok(ProjectPayload::V01(request)) => {
            match validate_project_request(&request.graph, request.nodes) {
                Ok(()) => RuntimeApiResponse::ok(),
                Err(diagnostics) => RuntimeApiResponse::diagnostics(diagnostics),
            }
        }
        Ok(ProjectPayload::V02(request)) => match validate_project_request_v02(&request) {
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
        Ok(ProjectPayload::V01(request)) => {
            let registry = match registry_from_nodes(request.nodes) {
                Ok(registry) => registry,
                Err(diagnostics) => {
                    return runtime_api_json(&state, RuntimeApiResponse::diagnostics(diagnostics));
                }
            };

            if let Err(diagnostics) = validate_graph_with_registry(&request.graph, &registry) {
                return runtime_api_json(&state, RuntimeApiResponse::diagnostics(diagnostics));
            }

            let plan = build_execution_plan(&request.graph, &registry)
                .expect("validated project should plan");
            runtime_api_json(
                &state,
                RuntimeApiResponse {
                    ok: true,
                    diagnostics: Vec::new(),
                    plan: Some(plan),
                    report: None,
                },
            )
        }
        Ok(ProjectPayload::V02(request)) => match build_execution_plan_request_v02(&request) {
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
        },
        Err(diagnostics) => runtime_api_json(&state, RuntimeApiResponse::diagnostics(diagnostics)),
    }
}

async fn run_project_endpoint(
    State(state): State<RuntimeServerState>,
    Json(value): Json<serde_json::Value>,
) -> Json<RuntimeApiResponse> {
    match decode_run_project_payload(value) {
        Ok(RunProjectPayload::V01(request)) => {
            let registry = match registry_from_nodes(request.nodes) {
                Ok(registry) => registry,
                Err(diagnostics) => {
                    return runtime_api_json(&state, RuntimeApiResponse::diagnostics(diagnostics));
                }
            };

            if let Err(diagnostics) = validate_graph_with_registry(&request.graph, &registry) {
                return runtime_api_json(&state, RuntimeApiResponse::diagnostics(diagnostics));
            }

            let plan = build_execution_plan(&request.graph, &registry)
                .expect("validated project should plan");
            let report = run_dummy_execution(&plan, request.frames.unwrap_or(1));
            runtime_api_json(
                &state,
                RuntimeApiResponse {
                    ok: true,
                    diagnostics: Vec::new(),
                    plan: Some(plan),
                    report: Some(report),
                },
            )
        }
        Ok(RunProjectPayload::V02(request)) => {
            match build_execution_plan_run_request_v02(&request) {
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

async fn session_snapshot(
    State(state): State<RuntimeServerState>,
) -> Json<crate::RuntimeSessionResponse> {
    session_snapshot_for(state.sessions.default_record())
}

async fn session_snapshot_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
) -> Json<crate::RuntimeSessionResponse> {
    session_snapshot_for(state.sessions.get_or_create(&session_id))
}

fn session_snapshot_for(record: RuntimeSessionRecord) -> Json<crate::RuntimeSessionResponse> {
    let session = record
        .session
        .read()
        .expect("runtime session lock should not be poisoned");
    Json(session.response(true, session.snapshot().diagnostics, None))
}

async fn session_info(State(state): State<RuntimeServerState>) -> Json<RuntimeSessionInfoResponse> {
    Json(session_info_for(&state, state.sessions.default_record()))
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

async fn load_session(
    State(state): State<RuntimeServerState>,
    Json(request): Json<ProjectRequest>,
) -> Json<crate::RuntimeSessionResponse> {
    load_session_for(&state, state.sessions.default_record(), request)
}

async fn load_session_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
    Json(request): Json<ProjectRequest>,
) -> Json<crate::RuntimeSessionResponse> {
    load_session_for(&state, state.sessions.get_or_create(&session_id), request)
}

fn load_session_for(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
    request: ProjectRequest,
) -> Json<crate::RuntimeSessionResponse> {
    let mut session = record
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    let response = session.load_project(request);
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

async fn validate_session(
    State(state): State<RuntimeServerState>,
) -> Json<crate::RuntimeSessionResponse> {
    validate_session_for(&state, state.sessions.default_record())
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

async fn plan_session(
    State(state): State<RuntimeServerState>,
) -> Json<crate::RuntimeSessionResponse> {
    plan_session_for(&state, state.sessions.default_record())
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

async fn run_session(
    State(state): State<RuntimeServerState>,
    Json(request): Json<SessionRunRequest>,
) -> Json<crate::RuntimeSessionResponse> {
    run_session_for(&state, state.sessions.default_record(), request)
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

async fn mutate_session(
    State(state): State<RuntimeServerState>,
    Json(value): Json<serde_json::Value>,
) -> Json<crate::RuntimePatchResponse> {
    mutate_session_for(&state, state.sessions.default_record(), value)
}

async fn mutate_session_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
    Json(value): Json<serde_json::Value>,
) -> Json<crate::RuntimePatchResponse> {
    mutate_session_for(&state, state.sessions.get_or_create(&session_id), value)
}

fn mutate_session_for(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
    value: serde_json::Value,
) -> Json<crate::RuntimePatchResponse> {
    let mut session = record
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    let mutation = match serde_json::from_value::<RuntimeMutationRequest>(value) {
        Ok(mutation) => mutation,
        Err(error) => {
            let response = session.reject_patch(
                false,
                vec![RuntimeDiagnostic::error(format!(
                    "invalid runtime mutation: {error}"
                ))],
            );
            return patch_json(state, response);
        }
    };

    let response = session.apply_mutation(mutation);
    if response.ok && response.applied {
        publish_session_event(
            &record,
            RuntimeSessionEventKind::Mutate,
            &session,
            response.diagnostics.clone(),
        );
    }
    patch_json(state, response)
}

async fn apply_session_operation(
    State(state): State<RuntimeServerState>,
    Json(value): Json<serde_json::Value>,
) -> Json<crate::PasteGraphFragmentResponse> {
    apply_session_operation_for(&state, state.sessions.default_record(), value)
}

async fn apply_session_operation_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
    Json(value): Json<serde_json::Value>,
) -> Json<crate::PasteGraphFragmentResponse> {
    apply_session_operation_for(&state, state.sessions.get_or_create(&session_id), value)
}

fn apply_session_operation_for(
    _state: &RuntimeServerState,
    record: RuntimeSessionRecord,
    value: serde_json::Value,
) -> Json<crate::PasteGraphFragmentResponse> {
    let mut session = record
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    let operation = match serde_json::from_value::<RuntimeOperationEnvelope>(value) {
        Ok(operation) => operation,
        Err(error) => {
            let revision_before = session
                .snapshot()
                .graph_revision()
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| "0".to_owned());
            return paste_operation_json(crate::PasteGraphFragmentResponse {
                schema: "skenion.runtime.paste-graph-fragment.response".to_owned(),
                schema_version: "0.1.0".to_owned(),
                ok: false,
                applied: false,
                conflict: false,
                target: crate::GraphTargetRef {
                    path: crate::PatchPath::Root,
                    base_revision: revision_before.clone(),
                    target_revision: None,
                },
                revision_before,
                revision_after: None,
                history_entry_id: None,
                id_remap: crate::IdRemapResult {
                    node_id_map: BTreeMap::new(),
                    edge_id_map: BTreeMap::new(),
                    omitted_edge_ids: Vec::new(),
                },
                diagnostics: vec![crate::RuntimeOperationDiagnostic {
                    severity: "error".to_owned(),
                    code: "paste.operation.invalid-json".to_owned(),
                    message: format!("invalid runtime operation: {error}"),
                    path: None,
                    target: None,
                    expected_revision: None,
                    actual_revision: None,
                    duplicates: None,
                    nodes: None,
                    edges: None,
                }],
            });
        }
    };

    let response = session.apply_runtime_operation(operation);
    if response.ok && response.applied {
        publish_session_event(
            &record,
            RuntimeSessionEventKind::Mutate,
            &session,
            Vec::new(),
        );
    }
    paste_operation_json(response)
}

async fn session_history(State(state): State<RuntimeServerState>) -> Json<crate::RuntimeHistory> {
    session_history_for(state.sessions.default_record())
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

async fn undo_session(
    State(state): State<RuntimeServerState>,
) -> Json<crate::RuntimePatchResponse> {
    undo_session_for(&state, state.sessions.default_record())
}

async fn undo_session_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
) -> Json<crate::RuntimePatchResponse> {
    undo_session_for(&state, state.sessions.get_or_create(&session_id))
}

fn undo_session_for(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
) -> Json<crate::RuntimePatchResponse> {
    let mut session = record
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    let response = session.undo();
    if response.ok && response.applied {
        publish_session_event(
            &record,
            RuntimeSessionEventKind::Undo,
            &session,
            response.diagnostics.clone(),
        );
    }
    patch_json(state, response)
}

async fn redo_session(
    State(state): State<RuntimeServerState>,
) -> Json<crate::RuntimePatchResponse> {
    redo_session_for(&state, state.sessions.default_record())
}

async fn redo_session_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
) -> Json<crate::RuntimePatchResponse> {
    redo_session_for(&state, state.sessions.get_or_create(&session_id))
}

fn redo_session_for(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
) -> Json<crate::RuntimePatchResponse> {
    let mut session = record
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    let response = session.redo();
    if response.ok && response.applied {
        publish_session_event(
            &record,
            RuntimeSessionEventKind::Redo,
            &session,
            response.diagnostics.clone(),
        );
    }
    patch_json(state, response)
}

async fn control_event(
    State(state): State<RuntimeServerState>,
    Json(request): Json<RuntimeControlEventRequest>,
) -> Json<RuntimeControlEventResponse> {
    control_event_for(&state, state.sessions.default_record(), request)
}

async fn control_event_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
    Json(request): Json<RuntimeControlEventRequest>,
) -> Json<RuntimeControlEventResponse> {
    control_event_for(&state, state.sessions.get_or_create(&session_id), request)
}

fn control_event_for(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
    request: RuntimeControlEventRequest,
) -> Json<RuntimeControlEventResponse> {
    let (mut response, control_snapshot) = {
        let mut session = record
            .session
            .write()
            .expect("runtime session lock should not be poisoned");
        let response = session.apply_control_event(request);
        let control_snapshot = if response.ok && response.changed {
            session.preview_control_state_snapshot()
        } else {
            None
        };
        (response, control_snapshot)
    };

    if let Some(control_snapshot) = control_snapshot {
        let mut preview = record
            .preview
            .lock()
            .expect("runtime preview lock should not be poisoned");
        if let Err(error) = preview.update_control_state(control_snapshot) {
            add_preview_control_update_warning(&mut response, error);
        }
    }

    control_event_json(state, response)
}

fn add_preview_control_update_warning(response: &mut RuntimeControlEventResponse, error: String) {
    response
        .diagnostics
        .push(RuntimeDiagnostic::warning(format!(
            "failed to update running preview control state: {error}"
        )));
}

async fn control_state(
    State(state): State<RuntimeServerState>,
) -> Json<RuntimeControlStateResponse> {
    control_state_for(state.sessions.default_record())
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

async fn control_read(
    State(state): State<RuntimeServerState>,
    Json(request): Json<RuntimeControlReadRequest>,
) -> Json<RuntimeControlReadResponse> {
    control_read_for(&state, state.sessions.default_record(), request)
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

async fn clear_session(
    State(state): State<RuntimeServerState>,
) -> Json<crate::RuntimeSessionResponse> {
    clear_session_for(&state, state.sessions.default_record())
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

async fn preview_status(
    State(state): State<RuntimeServerState>,
) -> Json<crate::RuntimePreviewStatusResponse> {
    preview_status_for(&state, state.sessions.default_record())
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

async fn start_preview(
    State(state): State<RuntimeServerState>,
    body: Bytes,
) -> Json<crate::RuntimePreviewStatusResponse> {
    start_preview_for(&state, state.sessions.default_record(), body)
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

async fn restart_preview(
    State(state): State<RuntimeServerState>,
) -> Json<crate::RuntimePreviewStatusResponse> {
    restart_preview_for(&state, state.sessions.default_record())
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

async fn stop_preview(
    State(state): State<RuntimeServerState>,
) -> Json<crate::RuntimePreviewStatusResponse> {
    stop_preview_for(&state, state.sessions.default_record())
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

async fn generated_shader(
    State(state): State<RuntimeServerState>,
) -> Json<GeneratedShaderResponse> {
    generated_shader_for(&state, state.sessions.default_record())
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

async fn session_telemetry(
    State(state): State<RuntimeServerState>,
) -> Json<RuntimeTelemetrySnapshot> {
    Json(telemetry_snapshot(&state, state.sessions.default_record()))
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

async fn session_telemetry_stream(
    State(state): State<RuntimeServerState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    session_telemetry_stream_for(state, DEFAULT_SESSION_ID.to_owned())
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
    fn ok() -> Self {
        Self {
            ok: true,
            diagnostics: Vec::new(),
            plan: None,
            report: None,
        }
    }

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
}

fn validate_project_request(
    graph: &GraphDocument,
    nodes: Vec<NodeDefinition>,
) -> Result<(), Vec<RuntimeDiagnostic>> {
    let registry = registry_from_nodes(nodes)?;
    validate_graph_with_registry(graph, &registry)
}

enum ProjectPayload {
    V01(ProjectRequest),
    V02(ProjectRequestV02),
}

enum RunProjectPayload {
    V01(RunProjectRequest),
    V02(RunProjectRequestV02),
}

fn decode_project_payload(
    value: serde_json::Value,
) -> Result<ProjectPayload, Vec<RuntimeDiagnostic>> {
    match project_schema_version(&value).as_deref() {
        Some("0.2.0") => decode_project_payload_v02(value).map(ProjectPayload::V02),
        Some("0.1.0") => serde_json::from_value(value)
            .map(ProjectPayload::V01)
            .map_err(invalid_project_payload),
        Some(version) => Err(vec![RuntimeDiagnostic::error(format!(
            "unsupported graph schemaVersion: {version}"
        ))]),
        None => Err(vec![RuntimeDiagnostic::error(
            "missing graph.schemaVersion in project request",
        )]),
    }
}

fn decode_run_project_payload(
    value: serde_json::Value,
) -> Result<RunProjectPayload, Vec<RuntimeDiagnostic>> {
    match project_schema_version(&value).as_deref() {
        Some("0.2.0") => decode_run_project_payload_v02(value).map(RunProjectPayload::V02),
        Some("0.1.0") => serde_json::from_value(value)
            .map(RunProjectPayload::V01)
            .map_err(invalid_project_payload),
        Some(version) => Err(vec![RuntimeDiagnostic::error(format!(
            "unsupported graph schemaVersion: {version}"
        ))]),
        None => Err(vec![RuntimeDiagnostic::error(
            "missing graph.schemaVersion in project request",
        )]),
    }
}

fn decode_project_payload_v02(
    value: serde_json::Value,
) -> Result<ProjectRequestV02, Vec<RuntimeDiagnostic>> {
    if is_project_document_v02(&value) {
        return decode_project_document_request_v02(value);
    }

    serde_json::from_value(value).map_err(invalid_project_payload)
}

fn decode_run_project_payload_v02(
    value: serde_json::Value,
) -> Result<RunProjectRequestV02, Vec<RuntimeDiagnostic>> {
    if is_project_document_v02(&value) {
        return decode_run_project_document_request_v02(value);
    }

    serde_json::from_value(value).map_err(invalid_project_payload)
}

fn decode_project_document_request_v02(
    mut value: serde_json::Value,
) -> Result<ProjectRequestV02, Vec<RuntimeDiagnostic>> {
    let nodes = take_node_definitions_v02(&mut value)?;
    let _ = take_frames_v02(&mut value)?;
    let document = decode_project_document_v02(value)?;
    Ok(ProjectRequestV02::from_project_document(document, nodes))
}

fn decode_run_project_document_request_v02(
    mut value: serde_json::Value,
) -> Result<RunProjectRequestV02, Vec<RuntimeDiagnostic>> {
    let nodes = take_node_definitions_v02(&mut value)?;
    let frames = take_frames_v02(&mut value)?;
    let document = decode_project_document_v02(value)?;
    Ok(RunProjectRequestV02::from_project_document(
        document, nodes, frames,
    ))
}

fn decode_project_document_v02(
    value: serde_json::Value,
) -> Result<ProjectDocumentV02, Vec<RuntimeDiagnostic>> {
    let document =
        serde_json::from_value::<ProjectDocumentV02>(value).map_err(invalid_project_payload)?;
    if let Err(report) = skenion_contracts::validate_project_document_v02(&document) {
        return Err(report
            .errors()
            .iter()
            .map(|error| {
                RuntimeDiagnostic::structured_error(
                    "project.invalid-v0.2",
                    error.message.clone(),
                    serde_json::json!({ "projectId": document.id }),
                )
            })
            .collect());
    }
    Ok(document)
}

fn take_node_definitions_v02(
    value: &mut serde_json::Value,
) -> Result<Vec<NodeDefinitionV02>, Vec<RuntimeDiagnostic>> {
    let nodes = value
        .as_object_mut()
        .and_then(|object| object.remove("nodes"))
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
    serde_json::from_value(nodes).map_err(invalid_project_payload)
}

fn take_frames_v02(value: &mut serde_json::Value) -> Result<Option<usize>, Vec<RuntimeDiagnostic>> {
    let frames = value
        .as_object_mut()
        .and_then(|object| object.remove("frames"))
        .unwrap_or(serde_json::Value::Null);
    serde_json::from_value(frames).map_err(invalid_project_payload)
}

fn project_schema_version(value: &serde_json::Value) -> Option<String> {
    value
        .get("graph")
        .and_then(|graph| graph.get("schemaVersion"))
        .and_then(|version| version.as_str())
        .or_else(|| {
            (value.get("schema").and_then(|schema| schema.as_str()) == Some("skenion.project"))
                .then(|| {
                    value
                        .get("schemaVersion")
                        .and_then(|version| version.as_str())
                })
                .flatten()
        })
        .map(str::to_owned)
}

fn is_project_document_v02(value: &serde_json::Value) -> bool {
    value.get("schema").and_then(|schema| schema.as_str()) == Some("skenion.project")
        && value
            .get("schemaVersion")
            .and_then(|version| version.as_str())
            == Some("0.2.0")
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

pub(crate) fn validate_graph_with_registry(
    graph: &GraphDocument,
    registry: &NodeRegistry,
) -> Result<(), Vec<RuntimeDiagnostic>> {
    validate_project(graph, registry).map_err(|report| {
        report
            .errors()
            .iter()
            .map(|error| RuntimeDiagnostic::error(error.message.clone()))
            .collect()
    })
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
    use std::sync::Arc;

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
    use skenion_contracts::RuntimeEventReplayMetadata;
    use tower::ServiceExt;

    use crate::{
        RuntimeIoDeviceDescriptor, RuntimeIoDeviceListResponse, RuntimeLogEvent, RuntimeLogSource,
        RuntimeSession, RuntimeSessionEvent, io_device_manager::RuntimeIoDeviceRegistry,
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
    }

    #[tokio::test]
    async fn runtime_info_response() {
        let response = get_json("/v0/runtime/info").await;

        assert_eq!(response["name"], "skenion-runtime");
        assert_eq!(response["apiVersion"], RUNTIME_API_VERSION);
        let capabilities = response["capabilities"].as_array().unwrap();
        for expected in [
            "project.validate",
            "project.validate.v0.2",
            "project.plan",
            "project.plan.v0.2",
            "dummy.run",
            "session.load",
            "session.mutate",
            "session.operation",
            "session.history",
            "session.undo",
            "session.redo",
            "session.control.event",
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
            "session.events.replay",
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
        assert_eq!(startup["endpoint"]["protocol"], "http");
        assert_eq!(startup["profile"]["mode"], "local-managed");
        assert_eq!(startup["profile"]["ownership"], "owned-child");
        assert_eq!(startup["defaultSessionId"], DEFAULT_SESSION_ID);
        assert_eq!(startup["token"]["required"], false);
        assert_eq!(startup["token"]["header"], "Authorization");
        assert_eq!(startup["shutdown"]["scope"], "owned-child-only");
        assert_eq!(health["health"]["ok"], true);
        assert_eq!(empty_shutdown["ok"], true);
        assert_eq!(shutdown["schema"], "skenion.runtime.sidecar.shutdown");
        assert_eq!(shutdown["ok"], true);
        assert_eq!(shutdown["accepted"], false);
        assert_eq!(shutdown["action"], "host-owned-process-stop-required");
        assert_eq!(shutdown["scope"], "owned-child-only");
        assert_eq!(invalid_shutdown["ok"], false);
        assert!(startup_from_state.ok);
    }

    #[tokio::test]
    async fn session_addressed_route_family_covers_canonical_surface() {
        let app = runtime_router_with_dry_preview();
        let registry = RuntimeSessionRegistry::dry_preview();
        assert_eq!(registry.get_or_create("").id, DEFAULT_SESSION_ID);

        post_json_with(app.clone(), "/v0/sessions/gamma/load", sample_project()).await;
        let alias_info = get_json_with(app.clone(), "/v0/session/info").await;
        let gamma_info = get_json_with(app.clone(), "/v0/sessions/gamma/info").await;
        assert_eq!(alias_info["sessionId"], DEFAULT_SESSION_ID);
        assert_eq!(gamma_info["sessionId"], "gamma");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v0/sessions/gamma/events/stream")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body().into_data_stream();
        let chunk = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("session event stream should emit")
            .expect("session event stream should have a chunk")
            .expect("session event stream chunk should be ok");
        let text = std::str::from_utf8(&chunk).expect("session event stream should be utf8");
        assert!(text.contains("\"sessionId\":\"gamma\""));

        assert_eq!(
            post_empty_with(app.clone(), "/v0/sessions/gamma/validate").await["ok"],
            true
        );
        assert_eq!(
            post_empty_with(app.clone(), "/v0/sessions/gamma/plan").await["ok"],
            true
        );
        assert_eq!(
            post_json_with(
                app.clone(),
                "/v0/sessions/gamma/run",
                json!({ "frames": 1 })
            )
            .await["ok"],
            true
        );

        let read = post_json_with(
            app.clone(),
            "/v0/sessions/gamma/control/read",
            json!({ "nodeId": "value_1", "target": "port", "id": "value" }),
        )
        .await;
        assert_eq!(read["ok"], true);

        assert_eq!(
            get_json_with(app.clone(), "/v0/sessions/gamma/preview").await["ok"],
            true
        );
        assert_eq!(
            post_empty_with(app.clone(), "/v0/sessions/gamma/preview/start").await["ok"],
            true
        );
        assert_eq!(
            post_empty_with(app.clone(), "/v0/sessions/gamma/preview/restart").await["ok"],
            true
        );
        assert_eq!(
            post_empty_with(app.clone(), "/v0/sessions/gamma/preview/stop").await["ok"],
            true
        );

        assert_eq!(
            get_json_with(app.clone(), "/v0/sessions/gamma/telemetry").await["ok"],
            true
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v0/sessions/gamma/telemetry/stream")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        let mut stream = response.into_body().into_data_stream();
        let chunk = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("telemetry stream should emit")
            .expect("telemetry stream should have a chunk")
            .expect("telemetry stream chunk should be ok");
        let text = std::str::from_utf8(&chunk).expect("telemetry stream should be utf8");
        assert!(text.contains("event: telemetry"));

        post_json_with(
            app.clone(),
            "/v0/sessions/shader/load",
            sample_shader_project(),
        )
        .await;
        assert_eq!(
            get_json_with(app.clone(), "/v0/sessions/shader/render/generated-shader").await["ok"],
            true
        );

        post_json_with(app.clone(), "/v0/sessions/delta/load", sample_project()).await;
        assert_eq!(
            post_json_with(
                app.clone(),
                "/v0/sessions/delta/operation",
                paste_operation("1")
            )
            .await["ok"],
            true
        );

        assert_eq!(
            post_json_with(
                app.clone(),
                "/v0/sessions/gamma/mutate",
                graph_mutation(set_value_patch("1"))
            )
            .await["ok"],
            true
        );
        assert_eq!(
            post_empty_with(app.clone(), "/v0/sessions/gamma/undo").await["ok"],
            true
        );
        assert_eq!(
            post_empty_with(app.clone(), "/v0/sessions/gamma/redo").await["ok"],
            true
        );
        assert_eq!(
            delete_json_with(app, "/v0/sessions/gamma").await["ok"],
            true
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
    async fn runtime_log_snapshot_replays_warning_error_backlog() {
        let app = runtime_router_with_fake_io_devices(Vec::new());

        let empty = get_json_with(app.clone(), "/v0/runtime/logs").await;
        assert_eq!(empty["schema"], "skenion.runtime.logs");
        assert_eq!(empty["events"], json!([]));
        assert_eq!(empty["retention"]["replayLimit"], 200);
        assert_eq!(
            empty["retention"]["replayLevels"],
            json!(["warning", "error"])
        );

        let undo = post_empty_with(app.clone(), "/v0/session/undo").await;
        assert_eq!(undo["ok"], false);

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
        let app = runtime_router_with_fake_io_devices(Vec::new());
        let _ = post_empty_with(app.clone(), "/v0/session/undo").await;

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
    async fn session_event_stream_replays_current_snapshot_as_sse() {
        let app = runtime_router_with_fake_io_devices(Vec::new());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v0/session/events/stream")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body().into_data_stream();
        let chunk = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("session event stream should emit")
            .expect("session event stream should have a chunk")
            .expect("session event stream chunk should be ok");
        let text = std::str::from_utf8(&chunk).expect("session event stream should be utf8");
        assert!(text.contains("event: session"));
        assert!(text.contains("skenion.runtime.session.event"));
        assert!(text.contains("\"kind\":\"snapshot\""));
    }

    #[tokio::test]
    async fn session_event_stream_emits_live_events_after_cursor() {
        let state = runtime_state_with_dry_preview();
        let app = runtime_router_with_state(state.clone());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v0/sessions/alpha/events/stream?after=0")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(response.status(), StatusCode::OK);
        let record = state.sessions.get_or_create("alpha");
        {
            let session = record
                .session
                .read()
                .expect("runtime session lock should not be poisoned");
            publish_session_event(
                &record,
                RuntimeSessionEventKind::Snapshot,
                &session,
                Vec::new(),
            );
        }

        let mut stream = response.into_body().into_data_stream();
        let chunk = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("session event stream should emit")
            .expect("session event stream should have a chunk")
            .expect("session event stream chunk should be ok");
        let text = std::str::from_utf8(&chunk).expect("session event stream should be utf8");
        assert!(text.contains("event: session"));
        assert!(text.contains("\"sessionId\":\"alpha\""));
        assert!(text.contains("\"cursor\":\"2\""));
    }

    #[test]
    fn stream_broadcast_helpers_format_live_and_gap_events() {
        let log_event = RuntimeLogEvent {
            id: 1,
            timestamp: "1970-01-01T00:00:00.000Z".to_owned(),
            source: RuntimeLogSource::Runtime,
            level: DiagnosticSeverity::Warning,
            code: Some("test-log".to_owned()),
            message: "test log".to_owned(),
        };
        let session = RuntimeSession::default();
        let session_event = RuntimeSessionEvent {
            schema: "skenion.runtime.session.event",
            schema_version: "0.1.0",
            id: "session_event_000001".to_owned(),
            session_id: DEFAULT_SESSION_ID.to_owned(),
            sequence: 1,
            session_revision: 0,
            kind: RuntimeSessionEventKind::Snapshot,
            snapshot: session.snapshot(),
            history: session.history(),
            mutation: None,
            replay: RuntimeEventReplayMetadata {
                cursor: "1".to_owned(),
                previous_cursor: None,
                replayed: false,
                gap: None,
                overflow: false,
            },
            diagnostics: Vec::new(),
            created_at: "1970-01-01T00:00:00.000Z".to_owned(),
        };

        assert!(runtime_log_broadcast_event(Ok(log_event)).is_ok());
        assert!(runtime_log_broadcast_event(Err(BroadcastStreamRecvError::Lagged(1))).is_ok());
        assert!(session_broadcast_event(Ok(session_event), DEFAULT_SESSION_ID.to_owned()).is_ok());
        assert!(
            session_broadcast_event(
                Err(BroadcastStreamRecvError::Lagged(1)),
                DEFAULT_SESSION_ID.to_owned()
            )
            .is_ok()
        );
    }

    #[tokio::test]
    async fn per_session_events_carry_session_id_sequence_and_replay_metadata() {
        let state = runtime_state_with_dry_preview();
        let app = runtime_router_with_state(state.clone());

        post_json_with(app.clone(), "/v0/sessions/alpha/load", sample_project()).await;
        post_json_with(app, "/v0/sessions/beta/load", sample_shader_project()).await;

        let alpha = state.sessions.get_or_create("alpha");
        let beta = state.sessions.get_or_create("beta");
        let alpha_events = alpha
            .event_store
            .lock()
            .expect("event store should not be poisoned")
            .clone();
        let beta_events = beta
            .event_store
            .lock()
            .expect("event store should not be poisoned")
            .clone();

        assert_eq!(alpha_events.len(), 1);
        assert_eq!(alpha_events[0].session_id, "alpha");
        assert_eq!(alpha_events[0].sequence, 1);
        assert_eq!(alpha_events[0].replay.cursor, "1");
        assert_eq!(beta_events[0].session_id, "beta");
        assert_eq!(beta_events[0].sequence, 1);

        let replay = replay_session_events(&alpha, Some(0), alpha_events[0].clone());
        assert_eq!(replay.len(), 1);
        assert!(replay[0].replay.replayed);
        assert_eq!(replay[0].session_id, "alpha");
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

    #[test]
    fn preview_control_update_warning_is_attached_to_control_response() {
        let mut response = RuntimeControlEventResponse {
            ok: true,
            changed: true,
            control_revision: Some(1),
            emitted: Vec::new(),
            diagnostics: Vec::new(),
        };

        add_preview_control_update_warning(&mut response, "snapshot write failed".to_owned());

        assert_eq!(response.diagnostics.len(), 1);
        assert_eq!(
            response.diagnostics[0].severity,
            crate::DiagnosticSeverity::Warning
        );
        assert!(
            response.diagnostics[0]
                .message
                .contains("snapshot write failed")
        );
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
    async fn valid_project_validation_response() {
        let response = post_json("/v0/validate", sample_project()).await;

        assert_eq!(response["ok"], true);
        assert_eq!(response["diagnostics"].as_array().unwrap().len(), 0);
        assert_eq!(response["plan"], Value::Null);
        assert_eq!(response["report"], Value::Null);
    }

    #[tokio::test]
    async fn invalid_project_validation_response() {
        let mut request = sample_project();
        request["nodes"] = json!([]);
        let response = post_json("/v0/validate", request).await;

        assert_eq!(response["ok"], false);
        assert!(
            response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("missing node definition")
        );
    }

    #[tokio::test]
    async fn validation_response_reports_registry_errors() {
        let mut request = sample_project();
        let duplicate = request["nodes"][0].clone();
        request["nodes"].as_array_mut().unwrap().push(duplicate);

        let response = post_json("/v0/validate", request).await;

        assert_eq!(response["ok"], false);
        assert!(
            response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("duplicate node definition")
        );
    }

    #[tokio::test]
    async fn plan_endpoint_returns_execution_plan() {
        let response = post_json("/v0/plan", sample_project()).await;

        assert_eq!(response["ok"], true);
        assert_eq!(response["plan"]["graphId"], "minimal-value");
        assert_eq!(response["plan"]["nodes"][0]["nodeId"], "value_1");
        assert_eq!(response["report"], Value::Null);
    }

    #[tokio::test]
    async fn plan_endpoint_reports_registry_errors() {
        let mut request = sample_project();
        request["nodes"][0]["schemaVersion"] = json!("9.9.9");

        let response = post_json("/v0/plan", request).await;

        assert_eq!(response["ok"], false);
        assert!(
            response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("invalid node definition")
        );
    }

    #[tokio::test]
    async fn plan_endpoint_reports_graph_errors() {
        let mut request = sample_project();
        request["nodes"] = json!([]);

        let response = post_json("/v0/plan", request).await;

        assert_eq!(response["ok"], false);
        assert!(
            response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("missing node definition")
        );
    }

    #[tokio::test]
    async fn run_endpoint_returns_dummy_execution_report() {
        let mut request = sample_project();
        request["frames"] = json!(2);
        let response = post_json("/v0/run", request).await;

        assert_eq!(response["ok"], true);
        assert_eq!(response["report"]["frameCount"], 2);
        assert_eq!(
            response["report"]["frames"][0]["executedNodes"][0]["status"],
            "simulated"
        );
    }

    #[tokio::test]
    async fn run_endpoint_defaults_to_one_frame() {
        let response = post_json("/v0/run", sample_project()).await;

        assert_eq!(response["ok"], true);
        assert_eq!(response["report"]["frameCount"], 1);
        assert_eq!(response["report"]["frames"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn run_endpoint_reports_registry_errors() {
        let mut request = sample_project();
        request["nodes"][0]["schemaVersion"] = json!("9.9.9");

        let response = post_json("/v0/run", request).await;

        assert_eq!(response["ok"], false);
        assert!(
            response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("invalid node definition")
        );
    }

    #[tokio::test]
    async fn run_endpoint_reports_graph_errors() {
        let mut request = sample_project();
        request["nodes"] = json!([]);

        let response = post_json("/v0/run", request).await;

        assert_eq!(response["ok"], false);
        assert!(
            response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("missing node definition")
        );
    }

    #[tokio::test]
    async fn v02_project_endpoints_validate_plan_and_run_with_edge_metadata() {
        let validation = post_json("/v0/validate", sample_project_v02()).await;
        assert_eq!(validation["ok"], true);
        assert_eq!(validation["diagnostics"].as_array().unwrap().len(), 0);
        assert_eq!(validation["plan"], Value::Null);

        let plan = post_json("/v0/plan", sample_project_v02()).await;
        assert_eq!(plan["ok"], true);
        assert_eq!(plan["plan"]["graphId"], "render-output-v02");
        assert_eq!(plan["plan"]["graphRevision"], "1");
        assert_eq!(
            plan["plan"]["edges"][0]["metadata"]["resolvedType"],
            "render.frame"
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

        let mut run_request = sample_project_v02();
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
    async fn v02_project_document_payload_expands_patch_library_before_plan_and_run() {
        let validation = post_json("/v0/validate", sample_subpatch_project_document_v02()).await;
        assert_eq!(validation["ok"], true);
        assert_eq!(validation["diagnostics"].as_array().unwrap().len(), 0);

        let plan = post_json("/v0/plan", sample_subpatch_project_document_v02()).await;
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

        let mut run_request = sample_subpatch_project_document_v02();
        run_request["frames"] = json!(2);
        let run = post_json("/v0/run", run_request).await;
        assert_eq!(run["ok"], true);
        assert_eq!(run["report"]["frameCount"], 2);
    }

    #[tokio::test]
    async fn v02_project_document_payload_reports_decode_and_contract_errors() {
        let malformed_project = json!({
          "schema": "skenion.project",
          "schemaVersion": "0.2.0"
        });
        let malformed_response = post_json("/v0/validate", malformed_project).await;
        assert_eq!(malformed_response["ok"], false);
        assert!(
            malformed_response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("invalid project request")
        );

        let mut duplicate_patch = sample_subpatch_project_document_v02();
        let patch = duplicate_patch["patchLibrary"][0].clone();
        duplicate_patch["patchLibrary"]
            .as_array_mut()
            .unwrap()
            .push(patch);

        let response = post_json("/v0/plan", duplicate_patch).await;
        assert_eq!(response["ok"], false);
        assert_eq!(
            response["diagnostics"][0]["code"],
            json!("project.invalid-v0.2")
        );
        assert_eq!(
            response["diagnostics"][0]["details"]["projectId"],
            json!("subpatch-project")
        );
    }

    #[tokio::test]
    async fn v02_project_endpoints_reject_ambiguous_algebraic_loop() {
        let response = post_json("/v0/validate", sample_ambiguous_loop_project_v02()).await;

        assert_eq!(response["ok"], false);
        assert!(
            response["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| diagnostic["message"]
                    .as_str()
                    .unwrap()
                    .contains("ambiguous-algebraic-loop"))
        );

        let plan = post_json("/v0/plan", sample_ambiguous_loop_project_v02()).await;
        assert_eq!(plan["ok"], false);
        assert!(
            plan["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("ambiguous-algebraic-loop")
        );

        let run = post_json("/v0/run", sample_ambiguous_loop_project_v02()).await;
        assert_eq!(run["ok"], false);
        assert!(
            run["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("ambiguous-algebraic-loop")
        );
    }

    #[tokio::test]
    async fn project_endpoints_reject_missing_and_unsupported_schema_versions() {
        let mut missing = sample_project_v02();
        missing["graph"]
            .as_object_mut()
            .unwrap()
            .remove("schemaVersion");
        let missing_response = post_json("/v0/validate", missing).await;
        assert_eq!(missing_response["ok"], false);
        assert!(
            missing_response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("missing graph.schemaVersion")
        );

        let mut unsupported = sample_project_v02();
        unsupported["graph"]["schemaVersion"] = json!("9.9.9");
        let unsupported_response = post_json("/v0/plan", unsupported).await;
        assert_eq!(unsupported_response["ok"], false);
        assert!(
            unsupported_response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("unsupported graph schemaVersion: 9.9.9")
        );

        let mut missing_run = sample_project_v02();
        missing_run["graph"]
            .as_object_mut()
            .unwrap()
            .remove("schemaVersion");
        let missing_run_response = post_json("/v0/run", missing_run).await;
        assert_eq!(missing_run_response["ok"], false);
        assert!(
            missing_run_response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("missing graph.schemaVersion")
        );

        let mut unsupported_run = sample_project_v02();
        unsupported_run["graph"]["schemaVersion"] = json!("9.9.9");
        let unsupported_run_response = post_json("/v0/run", unsupported_run).await;
        assert_eq!(unsupported_run_response["ok"], false);
        assert!(
            unsupported_run_response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("unsupported graph schemaVersion: 9.9.9")
        );
    }

    #[tokio::test]
    async fn project_endpoints_reject_malformed_payloads() {
        let mut request = sample_project_v02();
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
    async fn session_endpoint_returns_empty_state() {
        let response = get_json("/v0/session").await;

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

        let empty = get_json_with(app.clone(), "/v0/session").await;
        assert_eq!(empty["ok"], true);
        assert_eq!(empty["snapshot"]["project"], Value::Null);

        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;
        let project = get_json_with(app, "/v0/session").await;

        assert_eq!(project["ok"], true);
        assert_eq!(
            project["snapshot"]["project"]["graph"]["id"],
            "minimal-value"
        );
        assert_eq!(
            project["snapshot"]["project"]["nodes"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn session_load_stores_valid_project() {
        let app = runtime_router();
        let response = post_json_with(app.clone(), "/v0/session/load", sample_project()).await;

        assert_eq!(response["ok"], true);
        assert_eq!(
            response["snapshot"]["project"]["graph"]["id"],
            "minimal-value"
        );
        assert_eq!(response["snapshot"]["project"]["graph"]["revision"], "1");
        assert_eq!(response["snapshot"]["sessionRevision"], 1);
        assert_eq!(response["snapshot"]["plan"]["graphId"], "minimal-value");

        let snapshot = get_json_with(app, "/v0/session").await;
        assert_eq!(
            snapshot["snapshot"]["project"]["graph"]["id"],
            "minimal-value"
        );
        assert_eq!(
            snapshot["snapshot"]["plan"]["nodes"][0]["nodeId"],
            "value_1"
        );
    }

    #[tokio::test]
    async fn default_alias_and_explicit_default_session_share_behavior() {
        let app = runtime_router();

        let alias = post_json_with(app.clone(), "/v0/session/load", sample_project()).await;
        let explicit = get_json_with(app.clone(), "/v0/sessions/default").await;
        let info = get_json_with(app, "/v0/sessions/default/info").await;

        assert_eq!(alias["ok"], true);
        assert_eq!(explicit["ok"], true);
        assert_eq!(alias["snapshot"], explicit["snapshot"]);
        assert_eq!(info["sessionId"], DEFAULT_SESSION_ID);
        assert_eq!(
            info["snapshot"]["project"]["graph"]["id"],
            alias["snapshot"]["project"]["graph"]["id"]
        );
        let info = serde_json::from_value::<RuntimeSessionInfoResponse>(info)
            .expect("session info should match contract shape");
        skenion_contracts::validate_runtime_session_info_response(&info)
            .expect("session info should validate against contracts");
    }

    #[tokio::test]
    async fn explicit_sessions_keep_graph_control_and_history_state_separate() {
        let app = runtime_router();

        post_json_with(app.clone(), "/v0/sessions/alpha/load", sample_project()).await;
        post_json_with(
            app.clone(),
            "/v0/sessions/beta/load",
            sample_shader_project(),
        )
        .await;
        post_json_with(
            app.clone(),
            "/v0/sessions/alpha/mutate",
            graph_mutation(set_value_patch("1")),
        )
        .await;
        let alpha_control = post_json_with(
            app.clone(),
            "/v0/sessions/alpha/control/event",
            json!({
                "nodeId": "value_1",
                "portId": "in",
                "message": { "selector": "set", "atoms": [{ "type": "float", "representation": "f32", "value": 7.0 }] }
            }),
        )
        .await;

        let alpha = get_json_with(app.clone(), "/v0/sessions/alpha").await;
        let beta = get_json_with(app.clone(), "/v0/sessions/beta").await;
        let alpha_history = get_json_with(app.clone(), "/v0/sessions/alpha/history").await;
        let beta_history = get_json_with(app.clone(), "/v0/sessions/beta/history").await;
        let beta_control = get_json_with(app, "/v0/sessions/beta/control/state").await;

        assert_eq!(alpha_control["ok"], true);
        assert_eq!(alpha["snapshot"]["project"]["graph"]["id"], "minimal-value");
        assert_eq!(alpha["snapshot"]["project"]["graph"]["revision"], "2");
        assert_eq!(
            beta["snapshot"]["project"]["graph"]["id"],
            "shader-diagnostics"
        );
        assert_eq!(beta["snapshot"]["project"]["graph"]["revision"], "1");
        assert_eq!(alpha_history["entries"].as_array().unwrap().len(), 1);
        assert_eq!(beta_history["entries"], json!([]));
        assert_eq!(beta_control["controlRevision"], 0);
    }

    #[tokio::test]
    async fn invalid_session_load_does_not_replace_existing_session() {
        let app = runtime_router();
        let loaded = post_json_with(app.clone(), "/v0/session/load", sample_project()).await;
        let mut invalid = sample_project();
        invalid["nodes"] = json!([]);

        let response = post_json_with(app.clone(), "/v0/session/load", invalid).await;

        assert_eq!(loaded["snapshot"]["sessionRevision"], 1);
        assert_eq!(response["ok"], false);
        assert_eq!(
            response["snapshot"]["project"]["graph"]["id"],
            "minimal-value"
        );
        assert_eq!(response["snapshot"]["sessionRevision"], 1);
        assert!(
            response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("missing node definition")
        );

        let snapshot = get_json_with(app, "/v0/session").await;
        assert_eq!(snapshot["ok"], true);
        assert_eq!(
            snapshot["snapshot"]["project"]["graph"]["id"],
            "minimal-value"
        );
        assert_eq!(snapshot["diagnostics"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn session_validate_plan_and_run_use_loaded_project() {
        let app = runtime_router();
        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;

        let validation = post_empty_with(app.clone(), "/v0/session/validate").await;
        assert_eq!(validation["ok"], true);
        assert_eq!(validation["diagnostics"].as_array().unwrap().len(), 0);

        let plan = post_empty_with(app.clone(), "/v0/session/plan").await;
        assert_eq!(plan["ok"], true);
        assert_eq!(plan["snapshot"]["plan"]["graphId"], "minimal-value");

        let run = post_json_with(app, "/v0/session/run", json!({ "frames": 2 })).await;
        assert_eq!(run["ok"], true);
        assert_eq!(run["report"]["frameCount"], 2);
        assert_eq!(
            run["report"]["frames"][0]["executedNodes"][0]["status"],
            "simulated"
        );
    }

    #[tokio::test]
    async fn session_mutate_endpoint_applies_and_rejects_conflicts() {
        let app = runtime_router();
        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;

        let patched = post_json_with(
            app.clone(),
            "/v0/session/mutate",
            graph_mutation(set_value_patch("1")),
        )
        .await;
        assert_eq!(patched["ok"], true);
        assert_eq!(patched["applied"], true);
        assert_eq!(patched["conflict"], false);
        assert_eq!(patched["snapshot"]["project"]["graph"]["revision"], "2");
        assert_eq!(patched["history"]["entries"][0]["kind"], "apply");
        assert_eq!(
            patched["history"]["entries"][0]["mutation"]["graphPatch"]["baseRevision"],
            "1"
        );
        assert_eq!(
            patched["history"]["entries"][0]["inverseMutation"]["graphPatch"]["baseRevision"],
            "2"
        );
        assert_eq!(patched["history"]["undoDepth"], 1);
        assert_eq!(patched["history"]["redoDepth"], 0);
        assert_eq!(patched["snapshot"]["sessionRevision"], 2);
        assert_eq!(patched["snapshot"]["plan"]["graphRevision"], "2");

        let conflict = post_json_with(
            app,
            "/v0/session/mutate",
            graph_mutation(set_value_patch("1")),
        )
        .await;
        assert_eq!(conflict["ok"], false);
        assert_eq!(conflict["applied"], false);
        assert_eq!(conflict["conflict"], true);
        assert_eq!(conflict["snapshot"]["project"]["graph"]["revision"], "2");
        assert_eq!(conflict["history"]["entries"].as_array().unwrap().len(), 1);
        assert!(
            conflict["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("does not match session graph revision")
        );
    }

    #[tokio::test]
    async fn actor_attribution_is_optional_and_client_metadata_is_preserved() {
        let app = runtime_router();
        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;

        let omitted = post_json_with(
            app.clone(),
            "/v0/session/mutate",
            graph_mutation(set_value_patch("1")),
        )
        .await;
        let mut attributed_mutation = graph_mutation(set_value_patch("2"));
        attributed_mutation["clientId"] = json!("studio-window-a");
        let attributed = post_json_with(app, "/v0/session/mutate", attributed_mutation).await;

        assert_eq!(omitted["ok"], true);
        assert_eq!(omitted["history"]["entries"][0]["clientId"], Value::Null);
        assert_eq!(attributed["ok"], true);
        assert_eq!(
            attributed["history"]["entries"][1]["clientId"],
            "studio-window-a"
        );
        assert_eq!(
            attributed["history"]["entries"][1]["mutation"]["clientId"],
            "studio-window-a"
        );
    }

    #[tokio::test]
    async fn session_operation_endpoint_pastes_graph_fragment() {
        let app = runtime_router();
        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;

        let response =
            post_json_with(app.clone(), "/v0/session/operation", paste_operation("1")).await;

        assert_eq!(response["ok"], true);
        assert_eq!(response["applied"], true);
        assert_eq!(response["revisionBefore"], "1");
        assert_eq!(response["revisionAfter"], "2");
        assert_eq!(
            response["idRemap"]["nodeIdMap"]["value_1"],
            json!("value_1_2")
        );
        assert_eq!(
            response["idRemap"]["edgeIdMap"]["edge_value_to_pasted"],
            json!("edge_value_to_pasted")
        );

        let snapshot = get_json_with(app, "/v0/session").await;
        assert_eq!(snapshot["snapshot"]["project"]["graph"]["revision"], "2");
        assert!(
            snapshot["snapshot"]["project"]["graph"]["edges"]
                .as_array()
                .unwrap()
                .iter()
                .any(|edge| {
                    edge["from"]["node"] == "value_1_2"
                        && edge["from"]["port"] == "value"
                        && edge["to"]["node"] == "pasted_target"
                        && edge["to"]["port"] == "cold"
                })
        );
    }

    #[tokio::test]
    async fn session_operation_endpoint_rejects_invalid_envelope_json() {
        let response = post_json(
            "/v0/session/operation",
            json!({
              "schema": "skenion.runtime.operation",
              "schemaVersion": "0.1.0",
              "kind": "pasteGraphFragment"
            }),
        )
        .await;

        assert_eq!(response["ok"], false);
        assert_eq!(response["applied"], false);
        assert_eq!(response["target"]["path"]["kind"], "root");
        assert_eq!(
            response["diagnostics"][0]["code"],
            "paste.operation.invalid-json"
        );
    }

    #[tokio::test]
    async fn session_mutate_endpoint_reports_errors_without_loaded_session() {
        let response = post_json("/v0/session/mutate", graph_mutation(set_value_patch("1"))).await;

        assert_eq!(response["ok"], false);
        assert_eq!(response["applied"], false);
        assert_eq!(response["conflict"], false);
        assert_eq!(response["snapshot"]["project"], Value::Null);
        assert_eq!(response["history"]["entries"].as_array().unwrap().len(), 0);
        assert!(
            response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("no project loaded")
        );
    }

    #[tokio::test]
    async fn session_mutate_endpoint_reports_unsupported_operations() {
        let app = runtime_router();
        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;

        let response = post_json_with(
            app,
            "/v0/session/mutate",
            graph_mutation(json!({
              "schema": "skenion.graph.patch",
              "schemaVersion": "0.1.0",
              "id": "unsupported",
              "baseRevision": "1",
              "ops": [
                { "op": "moveNode", "nodeId": "value_1" }
              ]
            })),
        )
        .await;

        assert_eq!(response["ok"], false);
        assert_eq!(response["applied"], false);
        assert_eq!(response["conflict"], false);
        assert_eq!(response["snapshot"]["project"]["graph"]["revision"], "1");
        assert_eq!(response["history"]["entries"].as_array().unwrap().len(), 0);
        assert!(
            response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("invalid runtime mutation")
        );
    }

    #[tokio::test]
    async fn session_history_endpoint_returns_empty_and_event_history() {
        let app = runtime_router();

        let empty = get_json_with(app.clone(), "/v0/session/history").await;
        assert_eq!(empty["schema"], "skenion.runtime.history");
        assert_eq!(empty["entries"].as_array().unwrap().len(), 0);
        assert_eq!(empty["canUndo"], false);
        assert_eq!(empty["canRedo"], false);

        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;
        post_json_with(
            app.clone(),
            "/v0/session/mutate",
            graph_mutation(set_value_patch("1")),
        )
        .await;
        let history = get_json_with(app, "/v0/session/history").await;

        assert_eq!(history["entries"].as_array().unwrap().len(), 1);
        assert_eq!(history["entries"][0]["kind"], "apply");
        assert_eq!(history["undoDepth"], 1);
        assert_eq!(history["redoDepth"], 0);
    }

    #[tokio::test]
    async fn session_undo_and_redo_endpoints_update_graph_and_history() {
        let app = runtime_router();
        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;
        post_json_with(
            app.clone(),
            "/v0/session/mutate",
            graph_mutation(set_value_patch("1")),
        )
        .await;

        let undo = post_empty_with(app.clone(), "/v0/session/undo").await;
        assert_eq!(undo["ok"], true);
        assert_eq!(undo["applied"], true);
        assert_eq!(undo["history"]["entries"][1]["kind"], "undo");
        assert_eq!(undo["snapshot"]["project"]["graph"]["revision"], "3");
        assert_eq!(undo["history"]["entries"].as_array().unwrap().len(), 2);
        assert_eq!(undo["history"]["undoDepth"], 0);
        assert_eq!(undo["history"]["redoDepth"], 1);

        let redo = post_empty_with(app, "/v0/session/redo").await;
        assert_eq!(redo["ok"], true);
        assert_eq!(redo["applied"], true);
        assert_eq!(redo["history"]["entries"][2]["kind"], "redo");
        assert_eq!(redo["snapshot"]["project"]["graph"]["revision"], "4");
        assert_eq!(redo["history"]["entries"].as_array().unwrap().len(), 3);
        assert_eq!(redo["history"]["undoDepth"], 1);
        assert_eq!(redo["history"]["redoDepth"], 0);
    }

    #[tokio::test]
    async fn session_undo_and_redo_endpoints_report_empty_history() {
        let app = runtime_router();

        let undo = post_empty_with(app.clone(), "/v0/session/undo").await;
        let redo = post_empty_with(app, "/v0/session/redo").await;

        assert_eq!(undo["ok"], false);
        assert_eq!(undo["applied"], false);
        assert!(
            undo["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("available to undo")
        );
        assert_eq!(redo["ok"], false);
        assert_eq!(redo["applied"], false);
        assert!(
            redo["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("available to redo")
        );
    }

    #[tokio::test]
    async fn session_control_event_and_state_endpoints_follow_value_semantics() {
        let app = runtime_router();
        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;

        let set = post_json_with(
            app.clone(),
            "/v0/session/control/event",
            json!({
                "nodeId": "value_1",
                "portId": "in",
                "message": { "selector": "set", "atoms": [{ "type": "float", "representation": "f32", "value": 32.0 }] }
            }),
        )
        .await;
        assert_eq!(set["ok"], true);
        assert_eq!(set["emitted"], json!([]));

        let bang = post_json_with(
            app.clone(),
            "/v0/session/control/event",
            json!({ "nodeId": "value_1", "portId": "in", "message": { "selector": "bang", "atoms": [] } }),
        )
        .await;
        assert_eq!(bang["ok"], true);
        assert_eq!(
            bang["emitted"],
            json!([
                { "nodeId": "value_1", "portId": "value", "message": { "selector": "float", "atoms": [{ "type": "float", "representation": "f32", "value": 32.0 }] } },
                { "nodeId": "target_1", "portId": "value", "message": { "selector": "float", "atoms": [{ "type": "float", "representation": "f32", "value": 32.0 }] } }
            ])
        );

        let input = post_json_with(
            app.clone(),
            "/v0/session/control/event",
            json!({
                "nodeId": "value_1",
                "portId": "in",
                "message": { "selector": "float", "atoms": [{ "type": "float", "representation": "f32", "value": 12.0 }] }
            }),
        )
        .await;
        assert_eq!(input["ok"], true);
        assert_eq!(
            input["emitted"],
            json!([
                { "nodeId": "value_1", "portId": "value", "message": { "selector": "float", "atoms": [{ "type": "float", "representation": "f32", "value": 12.0 }] } },
                { "nodeId": "target_1", "portId": "value", "message": { "selector": "float", "atoms": [{ "type": "float", "representation": "f32", "value": 12.0 }] } }
            ])
        );

        let state = get_json_with(app.clone(), "/v0/session/control/state").await;
        assert_eq!(state["ok"], true);
        assert_eq!(
            state["values"]["value_1"],
            json!({ "type": "float", "representation": "f32", "value": 12.0 })
        );

        let state_read = post_json_with(
            app.clone(),
            "/v0/session/control/read",
            json!({ "nodeId": "value_1", "target": "state", "id": "value" }),
        )
        .await;
        assert_eq!(state_read["ok"], true);
        assert_eq!(
            state_read["value"],
            json!({ "type": "float", "representation": "f32", "value": 12.0 })
        );

        let port_read = post_json_with(
            app.clone(),
            "/v0/session/control/read",
            json!({ "nodeId": "value_1", "target": "port", "id": "value" }),
        )
        .await;
        assert_eq!(port_read["ok"], true);
        assert_eq!(port_read["value"]["value"]["id"], json!("value"));

        let wrong_type = post_json_with(
            app,
            "/v0/session/control/event",
            json!({
                "nodeId": "value_1",
                "portId": "in",
                "message": { "selector": "bool", "atoms": [{ "type": "bool", "value": true }] }
            }),
        )
        .await;
        assert_eq!(wrong_type["ok"], false);
        assert_eq!(wrong_type["emitted"], json!([]));
        assert!(
            wrong_type["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("expects number.float")
        );
    }

    #[tokio::test]
    async fn session_control_event_reports_running_preview_snapshot_update_warnings() {
        let state = runtime_state_with_dry_preview();
        let app = runtime_router_with_state(state.clone());
        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;
        post_empty_with(app.clone(), "/v0/session/preview/start").await;
        let blocker = std::env::temp_dir().join(format!(
            "skenion-preview-control-update-blocker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&blocker, b"blocker").expect("blocker should write");
        state
            .sessions
            .default_record()
            .preview
            .lock()
            .expect("preview lock should not be poisoned")
            .set_control_state_path_for_test(blocker.join("control-state.json"));

        let response = post_json_with(
            app,
            "/v0/session/control/event",
            json!({
                "nodeId": "value_1",
                "portId": "in",
                "message": { "selector": "set", "atoms": [{ "type": "float", "representation": "f32", "value": 2.0 }] }
            }),
        )
        .await;

        assert_eq!(response["ok"], true);
        assert_eq!(response["changed"], true);
        assert!(
            response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("failed to update running preview control state")
        );
        std::fs::remove_file(blocker).expect("blocker should remove");
    }

    #[tokio::test]
    async fn session_control_endpoints_report_missing_session() {
        let app = runtime_router();

        let event = post_json_with(
            app.clone(),
            "/v0/session/control/event",
            json!({
                "nodeId": "value_1",
                "portId": "in",
                "message": { "selector": "set", "atoms": [{ "type": "float", "representation": "f32", "value": 1.0 }] }
            }),
        )
        .await;
        let state = get_json_with(app, "/v0/session/control/state").await;
        let read = post_json_with(
            runtime_router(),
            "/v0/session/control/read",
            json!({ "nodeId": "value_1", "target": "state", "id": "value" }),
        )
        .await;

        assert_eq!(event["ok"], false);
        assert_eq!(event["emitted"], json!([]));
        assert!(
            event["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("no project loaded")
        );
        assert_eq!(state["ok"], false);
        assert_eq!(state["values"], json!({}));
        assert_eq!(read["ok"], false);
        assert!(
            read["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("no project loaded")
        );
    }

    #[tokio::test]
    async fn session_run_fails_without_loaded_project() {
        let response = post_json("/v0/session/run", json!({ "frames": 2 })).await;

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
        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;

        let response = delete_json_with(app, "/v0/session").await;

        assert_eq!(response["ok"], true);
        assert_eq!(response["snapshot"]["project"], Value::Null);
        assert_eq!(response["snapshot"]["sessionRevision"], 2);
        assert_eq!(response["snapshot"]["plan"], Value::Null);
    }

    #[tokio::test]
    async fn preview_status_reports_stopped_without_loaded_session() {
        let response =
            get_json_with(runtime_router_with_dry_preview(), "/v0/session/preview").await;

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
            "/v0/session/preview/start",
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
        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;

        let started = post_empty_with(app.clone(), "/v0/session/preview/start").await;
        assert_eq!(started["ok"], true);
        assert_eq!(started["state"], "running");
        assert_eq!(started["graphId"], "minimal-value");
        assert_eq!(started["graphRevision"], "1");
        assert_eq!(started["sessionRevision"], 1);
        assert_eq!(started["previewSessionRevision"], 1);
        assert_eq!(started["stale"], false);

        let stopped = post_empty_with(app.clone(), "/v0/session/preview/stop").await;
        assert_eq!(stopped["ok"], true);
        assert_eq!(stopped["state"], "stopped");
        assert_eq!(stopped["graphId"], Value::Null);

        let restarted = post_empty_with(app, "/v0/session/preview/restart").await;
        assert_eq!(restarted["ok"], true);
        assert_eq!(restarted["state"], "running");
        assert_eq!(restarted["previewSessionRevision"], 1);
    }

    #[tokio::test]
    async fn preview_start_request_restart_replaces_existing_preview() {
        let app = runtime_router_with_dry_preview();
        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;
        post_empty_with(app.clone(), "/v0/session/preview/start").await;
        post_json_with(
            app.clone(),
            "/v0/session/mutate",
            graph_mutation(set_value_patch("1")),
        )
        .await;

        let stale = get_json_with(app.clone(), "/v0/session/preview").await;
        assert_eq!(stale["state"], "running");
        assert_eq!(stale["graphRevision"], "1");
        assert_eq!(stale["sessionRevision"], 2);
        assert_eq!(stale["previewSessionRevision"], 1);
        assert_eq!(stale["stale"], true);

        let restarted =
            post_json_with(app, "/v0/session/preview/start", json!({ "restart": true })).await;
        assert_eq!(restarted["ok"], true);
        assert_eq!(restarted["graphRevision"], "2");
        assert_eq!(restarted["previewSessionRevision"], 2);
        assert_eq!(restarted["stale"], false);
    }

    #[tokio::test]
    async fn preview_start_rejects_invalid_request_json() {
        let app = runtime_router_with_dry_preview();
        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;

        let response = post_raw_with(app, "/v0/session/preview/start", b"{".to_vec()).await;

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
        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;
        post_empty_with(app.clone(), "/v0/session/preview/start").await;

        let cleared = delete_json_with(app.clone(), "/v0/session").await;
        assert_eq!(cleared["ok"], true);

        let preview = get_json_with(app, "/v0/session/preview").await;
        assert_eq!(preview["state"], "stopped");
        assert_eq!(preview["sessionRevision"], Value::Null);
        assert_eq!(preview["stale"], false);
    }

    #[tokio::test]
    async fn telemetry_endpoint_reports_empty_session() {
        let response =
            get_json_with(runtime_router_with_dry_preview(), "/v0/session/telemetry").await;

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
        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;

        let response = get_json_with(app, "/v0/session/telemetry").await;

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
        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;
        post_empty_with(app.clone(), "/v0/session/preview/start").await;

        let response = get_json_with(app, "/v0/session/telemetry").await;

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
    async fn telemetry_endpoint_marks_preview_stale_after_patch() {
        let app = runtime_router_with_dry_preview();
        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;
        post_empty_with(app.clone(), "/v0/session/preview/start").await;
        post_json_with(
            app.clone(),
            "/v0/session/mutate",
            graph_mutation(set_value_patch("1")),
        )
        .await;

        let response = get_json_with(app, "/v0/session/telemetry").await;

        assert_eq!(response["session"]["sessionRevision"], 2);
        assert_eq!(response["preview"]["state"], "running");
        assert_eq!(response["preview"]["previewSessionRevision"], 1);
        assert_eq!(response["preview"]["stale"], true);
    }

    #[tokio::test]
    async fn generated_shader_endpoint_returns_source_and_source_map() {
        let app = runtime_router_with_dry_preview();
        post_json_with(app.clone(), "/v0/session/load", sample_shader_project()).await;

        let response = get_json_with(app, "/v0/session/render/generated-shader").await;
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
            "/v0/session/render/generated-shader",
        )
        .await;
        assert_eq!(empty["ok"], false);
        assert_eq!(empty["diagnostics"][0]["phase"], json!("source-sync"));

        let app = runtime_router_with_dry_preview();
        let mut project = sample_shader_project();
        project["graph"]["nodes"][0]["params"]["source"] = json!(
            "// @skenion.uniform bad vec3\n@fragment\nfn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }"
        );
        let loaded = post_json_with(app.clone(), "/v0/session/load", project).await;
        assert_eq!(loaded["ok"], true);

        let response = get_json_with(app, "/v0/session/render/generated-shader").await;
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
                    .uri("/v0/session/telemetry/stream")
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

    async fn post_json(path: &str, payload: Value) -> Value {
        post_json_with(runtime_router(), path, payload).await
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

    fn runtime_state_with_dry_preview() -> RuntimeServerState {
        let logs = std::sync::Arc::new(RuntimeLogStore::default());
        RuntimeServerState {
            sessions: RuntimeSessionRegistry::dry_preview(),
            assets: std::sync::Arc::new(std::sync::RwLock::new(RuntimeAssetStore::default())),
            io_devices: std::sync::Arc::new(RuntimeIoDeviceManager::new()),
            extensions: std::sync::Arc::new(RuntimeExtensionManager::default()),
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
            extensions: std::sync::Arc::new(RuntimeExtensionManager::default()),
            logs,
            endpoint: RuntimeEndpointConfig::new(DEFAULT_HOST.to_owned(), DEFAULT_PORT),
            started_at_wall_clock: created_at_now(),
            started_at: std::time::Instant::now(),
        })
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
                "kind": "core.float",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": value_f32_ports_json()
              },
              {
                "id": "target_1",
                "kind": "core.float",
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
              "id": "core.float",
              "version": "0.1.0",
              "displayName": "Float Value",
              "category": "Values",
              "ports": value_f32_ports_json(),
              "execution": { "model": "value" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": []
            }
          ]
        })
    }

    fn value_f32_ports_json() -> Value {
        json!([
          {
            "id": "in",
            "direction": "input",
            "label": "In",
            "type": { "flow": "event", "dataKind": "message.any" },
            "required": false,
            "activation": "trigger"
          },
          {
            "id": "cold",
            "direction": "input",
            "label": "Cold",
            "type": { "flow": "value", "dataKind": "number.float" },
            "required": false,
            "activation": "latched"
          },
          {
            "id": "value",
            "direction": "output",
            "label": "Value",
            "type": { "flow": "value", "dataKind": "number.float" }
          }
        ])
    }

    fn sample_shader_project() -> Value {
        json!({
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": "shader-diagnostics",
            "revision": "1",
            "nodes": [
              {
                "id": "shader_1",
                "kind": "render.fullscreen-shader",
                "kindVersion": "0.1.0",
                "params": {
                  "language": "wgsl",
                  "source": "// @skenion.uniform speed number.float default=0.5\n@fragment\nfn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(skenion.speed, 0.0, 1.0, 1.0); }"
                },
                "ports": [
                  {
                    "id": "speed",
                    "direction": "input",
                    "label": "Speed",
                    "type": { "flow": "value", "dataKind": "number.float", "format": "f32" },
                    "required": false,
                    "default": 0.5,
                    "activation": "latched"
                  },
                  {
                    "id": "out",
                    "direction": "output",
                    "label": "Out",
                    "type": {
                      "flow": "resource",
                      "dataKind": "gpu.texture2d",
                      "format": "rgba8unorm",
                      "colorSpace": "srgb"
                    }
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
              "id": "render.fullscreen-shader",
              "version": "0.1.0",
              "displayName": "Fullscreen Shader",
              "category": "Render",
              "ports": [
                {
                  "id": "out",
                  "direction": "output",
                  "label": "Out",
                  "type": {
                    "flow": "resource",
                    "dataKind": "gpu.texture2d",
                    "format": "rgba8unorm",
                    "colorSpace": "srgb"
                  }
                }
              ],
              "execution": { "model": "gpu_pass" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": []
            }
          ]
        })
    }

    fn sample_project_v02() -> Value {
        json!({
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.2.0",
            "id": "render-output-v02",
            "revision": "1",
            "nodes": [
              {
                "id": "clear_color",
                "kind": "render.clear-color",
                "kindVersion": "0.2.0",
                "params": { "color": [0.12, 0.2, 0.34, 1] },
                "ports": [
                  {
                    "id": "out",
                    "direction": "output",
                    "type": "render.frame",
                    "rate": "render"
                  }
                ]
              },
              {
                "id": "output",
                "kind": "render.output",
                "kindVersion": "0.2.0",
                "params": {},
                "ports": [
                  {
                    "id": "in",
                    "direction": "input",
                    "type": "render.frame",
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
                "resolvedType": "render.frame"
              }
            ]
          },
          "nodes": [
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.2.0",
              "id": "render.clear-color",
              "version": "0.2.0",
              "displayName": "Clear Color",
              "category": "Render",
              "ports": [
                {
                  "id": "out",
                  "direction": "output",
                  "type": "render.frame",
                  "rate": "render"
                }
              ],
              "execution": { "model": "gpu_pass", "clock": "frame" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": ["render.frame.v0.2"]
            },
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.2.0",
              "id": "render.output",
              "version": "0.2.0",
              "displayName": "Render Output",
              "category": "Render",
              "ports": [
                {
                  "id": "in",
                  "direction": "input",
                  "type": "render.frame",
                  "rate": "render",
                  "required": true
                }
              ],
              "execution": { "model": "gpu_pass", "clock": "frame" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": ["render.output.v0.2"]
            }
          ]
        })
    }

    fn sample_subpatch_project_document_v02() -> Value {
        json!({
          "schema": "skenion.project",
          "schemaVersion": "0.2.0",
          "id": "subpatch-project",
          "revision": "1",
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.2.0",
            "id": "subpatch-project-root",
            "revision": "1",
            "nodes": [
              {
                "id": "clear_color",
                "kind": "render.clear-color",
                "kindVersion": "0.2.0",
                "params": { "color": [0.12, 0.2, 0.34, 1] },
                "ports": [
                  { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
                ]
              },
              {
                "id": "fx",
                "kind": "core.subpatch",
                "kindVersion": "0.2.0",
                "params": { "patchRef": "identity" },
                "ports": [
                  { "id": "in", "direction": "input", "type": "render.frame", "rate": "render", "required": true },
                  { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
                ]
              },
              {
                "id": "output",
                "kind": "render.output",
                "kindVersion": "0.2.0",
                "params": {},
                "ports": [
                  { "id": "in", "direction": "input", "type": "render.frame", "rate": "render", "required": true }
                ]
              }
            ],
            "edges": [
              {
                "id": "edge_clear_fx",
                "source": { "nodeId": "clear_color", "portId": "out" },
                "target": { "nodeId": "fx", "portId": "in" },
                "resolvedType": "render.frame"
              },
              {
                "id": "edge_fx_output",
                "source": { "nodeId": "fx", "portId": "out" },
                "target": { "nodeId": "output", "portId": "in" },
                "resolvedType": "render.frame"
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
                "schemaVersion": "0.2.0",
                "id": "identity-frame-patch",
                "revision": "1",
                "nodes": [
                  {
                    "id": "patch_in",
                    "kind": "core.inlet",
                    "kindVersion": "0.2.0",
                    "params": { "portId": "in", "label": "Input" },
                    "ports": [
                      { "id": "out", "direction": "output", "type": "render.frame", "rate": "render", "description": "Frame entering the patch" }
                    ]
                  },
                  {
                    "id": "pass",
                    "kind": "test.pass",
                    "kindVersion": "0.2.0",
                    "params": {},
                    "ports": [
                      { "id": "in", "direction": "input", "type": "render.frame", "rate": "render", "required": true },
                      { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
                    ]
                  },
                  {
                    "id": "patch_out",
                    "kind": "core.outlet",
                    "kindVersion": "0.2.0",
                    "params": { "portId": "out", "label": "Output" },
                    "ports": [
                      { "id": "in", "direction": "input", "type": "render.frame", "rate": "render", "required": true, "description": "Frame leaving the patch" }
                    ]
                  }
                ],
                "edges": [
                  {
                    "id": "edge_in_pass",
                    "source": { "nodeId": "patch_in", "portId": "out" },
                    "target": { "nodeId": "pass", "portId": "in" },
                    "resolvedType": "render.frame"
                  },
                  {
                    "id": "edge_pass_out",
                    "source": { "nodeId": "pass", "portId": "out" },
                    "target": { "nodeId": "patch_out", "portId": "in" },
                    "resolvedType": "render.frame"
                  }
                ]
              }
            }
          ],
          "nodes": [
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.2.0",
              "id": "render.clear-color",
              "version": "0.2.0",
              "displayName": "Clear Color",
              "category": "Render",
              "ports": [
                { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
              ],
              "execution": { "model": "gpu_pass", "clock": "frame" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": ["render.frame.v0.2"]
            },
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.2.0",
              "id": "render.output",
              "version": "0.2.0",
              "displayName": "Render Output",
              "category": "Render",
              "ports": [
                { "id": "in", "direction": "input", "type": "render.frame", "rate": "render", "required": true }
              ],
              "execution": { "model": "gpu_pass", "clock": "frame" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": ["render.output.v0.2"]
            },
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.2.0",
              "id": "test.pass",
              "version": "0.2.0",
              "displayName": "Pass",
              "category": "Test",
              "ports": [
                { "id": "in", "direction": "input", "type": "render.frame", "rate": "render", "required": true },
                { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
              ],
              "execution": { "model": "gpu_pass", "clock": "frame" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": []
            }
          ]
        })
    }

    fn sample_ambiguous_loop_project_v02() -> Value {
        json!({
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.2.0",
            "id": "ambiguous-algebraic-loop-v02",
            "revision": "1",
            "nodes": [
              {
                "id": "a",
                "kind": "core.value-transform",
                "kindVersion": "0.2.0",
                "params": {},
                "ports": [
                  { "id": "in", "direction": "input", "type": "value.number", "rate": "control" },
                  { "id": "out", "direction": "output", "type": "value.number", "rate": "control" }
                ]
              },
              {
                "id": "b",
                "kind": "core.value-transform",
                "kindVersion": "0.2.0",
                "params": {},
                "ports": [
                  { "id": "in", "direction": "input", "type": "value.number", "rate": "control" },
                  { "id": "out", "direction": "output", "type": "value.number", "rate": "control" }
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
              "schemaVersion": "0.2.0",
              "id": "core.value-transform",
              "version": "0.2.0",
              "displayName": "Value Transform",
              "category": "Core",
              "ports": [
                { "id": "in", "direction": "input", "type": "value.number", "rate": "control" },
                { "id": "out", "direction": "output", "type": "value.number", "rate": "control" }
              ],
              "execution": { "model": "value" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": ["value.number.v0.2"]
            }
          ]
        })
    }

    fn set_value_patch(base_revision: &str) -> Value {
        json!({
          "schema": "skenion.graph.patch",
          "schemaVersion": "0.1.0",
          "id": "set-value",
          "baseRevision": base_revision,
          "ops": [
            { "op": "setNodeParam", "nodeId": "value_1", "key": "value", "value": 0.75 }
          ]
        })
    }

    fn graph_mutation(graph_patch: Value) -> Value {
        json!({ "graphPatch": graph_patch })
    }

    fn paste_operation(base_revision: &str) -> Value {
        json!({
          "schema": "skenion.runtime.operation",
          "schemaVersion": "0.1.0",
          "id": "op-paste",
          "kind": "pasteGraphFragment",
          "request": {
            "target": {
              "path": { "kind": "root" },
              "baseRevision": base_revision
            },
            "fragment": {
              "schema": "skenion.graph.fragment",
              "schemaVersion": "0.2.0",
              "nodes": [
                {
                  "id": "value_1",
                  "kind": "core.float",
                  "kindVersion": "0.1.0",
                  "params": {},
                  "ports": value_f32_ports_v02_json()
                },
                {
                  "id": "pasted_target",
                  "kind": "core.float",
                  "kindVersion": "0.1.0",
                  "params": {},
                  "ports": value_f32_ports_v02_json()
                }
              ],
              "edges": [
                {
                  "id": "edge_value_to_pasted",
                  "source": { "nodeId": "value_1", "portId": "value" },
                  "target": { "nodeId": "pasted_target", "portId": "cold" }
                }
              ]
            },
            "options": {
              "idConflictPolicy": "remap"
            }
          },
          "attribution": {
            "clientId": "studio-test",
            "label": "Paste test fragment"
          }
        })
    }

    fn value_f32_ports_v02_json() -> Value {
        json!([
          {
            "id": "in",
            "direction": "input",
            "label": "In",
            "type": "message.any",
            "rate": "event",
            "required": false,
            "triggerMode": "trigger"
          },
          {
            "id": "cold",
            "direction": "input",
            "label": "Cold",
            "type": "number.float",
            "rate": "control",
            "required": false,
            "triggerMode": "latched"
          },
          {
            "id": "value",
            "direction": "output",
            "label": "Value",
            "type": "number.float",
            "rate": "control"
          }
        ])
    }
}
