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
    http::{HeaderMap, HeaderValue, Method, StatusCode, header::CONTENT_TYPE},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use skenion_contracts::{
    CONTRACTS_COMPATIBILITY_LINE, CONTRACTS_COMPATIBILITY_RANGE, CONTRACTS_PACKAGE_VERSION,
    PackageRegistryListResponseV01, RuntimeCollaborationAck, RuntimeCollaborationCausalMetadata,
    RuntimeCollaborationChange, RuntimeCollaborationConflict, RuntimeCollaborationNack,
    RuntimeCollaborationNackReason, RuntimeCollaborationOperationBatch,
    RuntimeCollaborationOperationBatchResult, RuntimeCollaborationOperationDiagnostic,
    RuntimeCollaborationOperationEnvelope, RuntimeCollaborationOperationPayload,
    RuntimeCollaborationOperationResult, RuntimeCollaborationOperationStatus,
    RuntimeCollaborationPresenceEnvelope, RuntimeCollaborationRebase,
    RuntimeCollaborationRebaseStrategy, RuntimeCollaborationSelectionEnvelope,
    RuntimeCollaborationServerClock, RuntimeCollaborationUndoRedoAction,
    RuntimeSessionInfoResponse, validate_runtime_collaboration_operation_batch,
    validate_runtime_collaboration_operation_batch_result,
    validate_runtime_collaboration_operation_envelope,
    validate_runtime_collaboration_operation_result,
    validate_runtime_collaboration_presence_envelope,
    validate_runtime_collaboration_selection_envelope,
};
use tokio_stream::{
    Stream, StreamExt,
    wrappers::{BroadcastStream, IntervalStream, errors::BroadcastStreamRecvError},
};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::{
    CURRENT_SCHEMA_VERSION, DummyExecutionReport, ExecutionPlan, GeneratedShaderResponse,
    NodeDefinition, NodeDefinitionCurrent, NodeRegistry, PreviewDocument, ProjectDocumentCurrent,
    ProjectRequestCurrent, RunProjectRequestCurrent, RuntimeControlEventRequest,
    RuntimeControlEventResponse, RuntimeControlReadRequest, RuntimeControlReadResponse,
    RuntimeControlStateResponse, RuntimeExtensionListResponse, RuntimeExtensionManager,
    RuntimeExtensionRegistrySnapshot, RuntimeIoDeviceListResponse, RuntimeIoDeviceManager,
    RuntimeLogSnapshotResponse, RuntimeLogStore, RuntimeMutationRequest, RuntimeOperationEnvelope,
    RuntimePackageManager, RuntimePackageRegistrySnapshot, RuntimePatchResponse,
    RuntimePreviewStartRequest, RuntimeTelemetrySnapshot, SessionRunRequest, ShaderDiagnostic,
    ShaderDiagnosticPhase, ShaderDiagnosticSource, build_execution_plan_request_current,
    build_execution_plan_run_request_current, collaboration_broadcast_event_after_high_water,
    collaboration_event, generated_shader_response_from_preview_document,
    project_document_payload_schema_diagnostics, project_document_validation_diagnostics_current,
    run_dummy_execution,
    runtime_time::created_at_now,
    schema_version_diagnostic,
    session_registry::{
        RuntimeSessionEventKind, RuntimeSessionRecord, RuntimeSessionRegistry, SessionEventsQuery,
        capture_session_replay, event_cursor_from_headers, publish_session_event,
        session_broadcast_event_after_high_water, session_event, session_snapshot_event,
    },
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

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
enum CollaborationOperationResponse {
    Batch(Box<RuntimeCollaborationOperationBatchResult>),
    Single(Box<RuntimeCollaborationOperationResult>),
    Paste(Box<crate::PasteGraphFragmentResponse>),
}

#[derive(Debug)]
struct LoweredCollaborationChangeSet {
    target: crate::GraphTargetRef,
    changes: Vec<RuntimeCollaborationChange>,
    actor_id: Option<String>,
    client_id: Option<String>,
    description: Option<String>,
    transformed_payload: RuntimeCollaborationOperationPayload,
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
            "/v0/sessions/{session_id}/operations",
            post(apply_session_collaboration_operations_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/collaboration/presence",
            post(update_session_collaboration_presence_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/collaboration/selection",
            post(update_session_collaboration_selection_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/collaboration/events/stream",
            get(session_collaboration_events_stream_by_id),
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
            "session.project",
            "session.project.v0.1",
            "session.events.stream",
            "session.validate",
            "session.plan",
            "session.run",
            "session.mutate",
            "session.operation",
            "session.collaboration.operations",
            "session.collaboration.operationBatch",
            "session.collaboration.events.stream",
            "session.collaboration.presence",
            "session.collaboration.selection",
            "session.collaboration.idempotency",
            "session.collaboration.rebase.crdtMerge",
            "session.history",
            "session.undo",
            "session.redo",
            "session.clear",
            "session.addressing",
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
    let after = query.after.or_else(|| event_cursor_from_headers(&headers));
    let receiver = record.events.subscribe();
    let snapshot = {
        let session = record
            .session
            .read()
            .expect("runtime session lock should not be poisoned");
        session_snapshot_event(&record, &session)
    };
    let replay = capture_session_replay(&record, after, snapshot);
    let high_water_sequence = replay.high_water_sequence;
    let replay = tokio_stream::iter(replay.events.into_iter().map(session_event));
    let live_record = record.clone();
    let live = BroadcastStream::new(receiver).filter_map(move |result| {
        session_broadcast_event_after_high_water(result, live_record.clone(), high_water_sequence)
    });
    Sse::new(replay.chain(live)).keep_alive(KeepAlive::default())
}

async fn session_collaboration_events_stream_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionEventsQuery>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    session_collaboration_events_stream_for(
        state.sessions.get_or_create(&session_id),
        query,
        headers,
    )
}

fn session_collaboration_events_stream_for(
    record: RuntimeSessionRecord,
    query: SessionEventsQuery,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let after = query.after.or_else(|| event_cursor_from_headers(&headers));
    let replay = record.collaboration.capture_replay(after);
    let high_water_sequence = replay.high_water_sequence;
    let replay = tokio_stream::iter(replay.events.into_iter().map(collaboration_event));
    let live_record = record.clone();
    let live =
        BroadcastStream::new(record.collaboration.events.subscribe()).filter_map(move |result| {
            collaboration_broadcast_event_after_high_water(
                result,
                &live_record.collaboration,
                &live_record.id,
                high_water_sequence,
            )
        });
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
    let request = match decode_project_payload(value) {
        Ok(ProjectPayload::Current(request)) => request,
        Err(diagnostics) => {
            let response = session.response(false, diagnostics, None);
            return session_json(state, response);
        }
    };
    let response = session.load_project_current_with_package_registry_revision(
        *request,
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
    let _coordination_guard = record.collaboration.operation_guard();
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
    let _coordination_guard = record.collaboration.operation_guard();
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

async fn apply_session_collaboration_operations_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
    Json(value): Json<serde_json::Value>,
) -> Json<CollaborationOperationResponse> {
    apply_session_collaboration_operations_for(
        &state,
        state.sessions.get_or_create(&session_id),
        value,
    )
}

fn apply_session_collaboration_operations_for(
    _state: &RuntimeServerState,
    record: RuntimeSessionRecord,
    value: serde_json::Value,
) -> Json<CollaborationOperationResponse> {
    let response = match value.get("schema").and_then(serde_json::Value::as_str) {
        Some("skenion.runtime.operation") => {
            match serde_json::from_value::<RuntimeOperationEnvelope>(value) {
                Ok(operation) => {
                    let _coordination_guard = record.collaboration.operation_guard();
                    let mut session = record
                        .session
                        .write()
                        .expect("runtime session lock should not be poisoned");
                    let response = session.apply_runtime_operation(operation);
                    if response.ok && response.applied {
                        publish_session_event(
                            &record,
                            RuntimeSessionEventKind::Mutate,
                            &session,
                            Vec::new(),
                        );
                    }
                    CollaborationOperationResponse::Paste(Box::new(response))
                }
                Err(error) => CollaborationOperationResponse::Paste(Box::new(
                    invalid_paste_operation_response(
                        &record,
                        format!("invalid runtime operation: {error}"),
                    ),
                )),
            }
        }
        Some("skenion.runtime.collaboration.operation-batch") => {
            match serde_json::from_value::<RuntimeCollaborationOperationBatch>(value) {
                Ok(batch) => CollaborationOperationResponse::Batch(Box::new(
                    apply_collaboration_operation_batch(&record, batch),
                )),
                Err(error) => CollaborationOperationResponse::Batch(Box::new(
                    invalid_collaboration_batch_result(
                        &record.id,
                        format!("invalid collaboration batch: {error}"),
                    ),
                )),
            }
        }
        _ => match serde_json::from_value::<RuntimeCollaborationOperationEnvelope>(value) {
            Ok(operation) => CollaborationOperationResponse::Single(Box::new(
                apply_collaboration_operation(&record, operation),
            )),
            Err(error) => CollaborationOperationResponse::Single(Box::new(
                invalid_collaboration_operation_result(
                    &record,
                    "invalid-operation",
                    "unknown",
                    "invalid-operation",
                    RuntimeCollaborationCausalMetadata {
                        base_revision: current_session_graph_revision(&record),
                        base_sequence: 0,
                        vector: BTreeMap::from([("runtime".to_owned(), 0)]),
                        observed_operation_ids: None,
                    },
                    format!("invalid collaboration operation: {error}"),
                ),
            )),
        },
    };
    Json(response)
}

async fn update_session_collaboration_presence_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
    Json(presence): Json<RuntimeCollaborationPresenceEnvelope>,
) -> Response {
    update_session_collaboration_presence_for(state.sessions.get_or_create(&session_id), presence)
}

fn update_session_collaboration_presence_for(
    record: RuntimeSessionRecord,
    mut presence: RuntimeCollaborationPresenceEnvelope,
) -> Response {
    let _coordination_guard = record.collaboration.operation_guard();
    presence.session_id = record.id.clone();
    if let Err(report) = validate_runtime_collaboration_presence_envelope(&presence) {
        return invalid_collaboration_metadata_response(
            "collaboration.invalid-presence",
            format!("invalid collaboration presence: {report}"),
        );
    }
    let sequence = record.collaboration.reserve_sequence();
    record
        .collaboration
        .publish_presence(sequence, presence.clone());
    Json(presence).into_response()
}

async fn update_session_collaboration_selection_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
    Json(selection): Json<RuntimeCollaborationSelectionEnvelope>,
) -> Response {
    update_session_collaboration_selection_for(state.sessions.get_or_create(&session_id), selection)
}

fn update_session_collaboration_selection_for(
    record: RuntimeSessionRecord,
    mut selection: RuntimeCollaborationSelectionEnvelope,
) -> Response {
    let _coordination_guard = record.collaboration.operation_guard();
    selection.session_id = record.id.clone();
    if let Err(report) = validate_runtime_collaboration_selection_envelope(&selection) {
        return invalid_collaboration_metadata_response(
            "collaboration.invalid-selection",
            format!("invalid collaboration selection: {report}"),
        );
    }
    let sequence = record.collaboration.reserve_sequence();
    record
        .collaboration
        .publish_selection(sequence, selection.clone());
    Json(selection).into_response()
}

fn invalid_collaboration_metadata_response(code: &str, message: String) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "ok": false,
            "diagnostics": [{
                "severity": "error",
                "code": code,
                "message": message
            }]
        })),
    )
        .into_response()
}

fn apply_collaboration_operation_batch(
    record: &RuntimeSessionRecord,
    batch: RuntimeCollaborationOperationBatch,
) -> RuntimeCollaborationOperationBatchResult {
    if let Err(report) = validate_runtime_collaboration_operation_batch(&batch) {
        return invalid_collaboration_batch_result(
            &record.id,
            format!("invalid collaboration batch: {report}"),
        );
    }
    if batch.session_id != record.id {
        return invalid_collaboration_batch_result(
            &record.id,
            format!(
                "collaboration batch sessionId {} does not match Runtime session {}",
                batch.session_id, record.id
            ),
        );
    }

    let results = batch
        .operations
        .into_iter()
        .map(|operation| apply_collaboration_operation(record, operation))
        .collect();
    let result = RuntimeCollaborationOperationBatchResult {
        schema: "skenion.runtime.collaboration.operation-batch-result".to_owned(),
        schema_version: "0.1.0".to_owned(),
        session_id: record.id.clone(),
        results,
        diagnostics: Vec::new(),
        created_at: created_at_now(),
    };
    validate_runtime_collaboration_operation_batch_result(&result)
        .expect("runtime collaboration batch result should validate");
    result
}

fn apply_collaboration_operation(
    record: &RuntimeSessionRecord,
    operation: RuntimeCollaborationOperationEnvelope,
) -> RuntimeCollaborationOperationResult {
    let _operation_guard = record.collaboration.operation_guard();
    if let Err(report) = validate_runtime_collaboration_operation_envelope(&operation) {
        return publish_collaboration_operation_result(
            record,
            invalid_collaboration_operation_result(
                record,
                &operation.operation_id,
                &operation.participant_id,
                &operation.idempotency_key,
                operation.causal.clone(),
                format!("invalid collaboration operation: {report}"),
            ),
        );
    }
    if operation.session_id != record.id {
        return publish_collaboration_operation_result(
            record,
            invalid_collaboration_operation_result(
                record,
                &operation.operation_id,
                &operation.participant_id,
                &operation.idempotency_key,
                operation.causal.clone(),
                format!(
                    "collaboration operation sessionId {} does not match Runtime session {}",
                    operation.session_id, record.id
                ),
            ),
        );
    }
    if record
        .collaboration
        .has_idempotency_key(&operation.idempotency_key)
    {
        return publish_collaboration_operation_result(
            record,
            duplicate_collaboration_operation_result(record, &operation),
        );
    }

    let payload_target = collaboration_payload_target(&operation.payload);
    let current_revision = current_session_target_revision(record, payload_target);
    let payload_requires_rebase = collaboration_payload_base_revision(&operation.payload)
        .is_some_and(|base_revision| base_revision != current_revision);
    let requires_rebase =
        operation.causal.base_revision != current_revision || payload_requires_rebase;
    let transformed_payload =
        transform_collaboration_payload_to_revision(&operation.payload, &current_revision);
    let rebase = requires_rebase.then(|| {
        collaboration_rebase(
            record,
            &operation,
            current_revision.clone(),
            RuntimeCollaborationRebaseStrategy::CrdtMerge,
            Some(transformed_payload.clone()),
            Vec::new(),
        )
    });

    match &transformed_payload {
        RuntimeCollaborationOperationPayload::PasteGraphFragment {
            request,
            description,
            ..
        } => {
            let runtime_operation = RuntimeOperationEnvelope {
                schema: "skenion.runtime.operation".to_owned(),
                schema_version: "0.1.0".to_owned(),
                id: operation.operation_id.clone(),
                kind: "pasteGraphFragment".to_owned(),
                request: (**request).clone(),
                attribution: Some(crate::RuntimeOperationAttribution {
                    actor_id: Some(operation.participant_id.clone()),
                    client_id: operation.correlation_id.clone(),
                    label: description.clone(),
                }),
                correlation_id: operation.correlation_id.clone(),
                created_at: Some(operation.submitted_at.clone()),
            };
            apply_collaboration_paste_operation(record, operation, runtime_operation, rebase)
        }
        RuntimeCollaborationOperationPayload::ChangeSet {
            target,
            changes,
            description,
            ..
        } => {
            let lowered = LoweredCollaborationChangeSet {
                target: target.clone(),
                changes: changes.clone(),
                actor_id: Some(operation.participant_id.clone()),
                client_id: operation.correlation_id.clone(),
                description: description.clone(),
                transformed_payload: transformed_payload.clone(),
            };
            apply_collaboration_change_set_operation(record, operation, lowered, rebase)
        }
        RuntimeCollaborationOperationPayload::UndoRedo { action, .. } => {
            apply_collaboration_undo_redo_operation(record, operation, action.clone(), rebase)
        }
    }
}

fn collaboration_payload_base_revision(
    payload: &RuntimeCollaborationOperationPayload,
) -> Option<&str> {
    match payload {
        RuntimeCollaborationOperationPayload::ChangeSet { target, .. } => {
            Some(target.base_revision.as_str())
        }
        RuntimeCollaborationOperationPayload::PasteGraphFragment { request, .. } => {
            Some(request.target.base_revision.as_str())
        }
        RuntimeCollaborationOperationPayload::UndoRedo { .. } => None,
    }
}

fn collaboration_payload_target(
    payload: &RuntimeCollaborationOperationPayload,
) -> Option<&crate::GraphTargetRef> {
    match payload {
        RuntimeCollaborationOperationPayload::ChangeSet { target, .. } => Some(target),
        RuntimeCollaborationOperationPayload::PasteGraphFragment { request, .. } => {
            Some(&request.target)
        }
        RuntimeCollaborationOperationPayload::UndoRedo { .. } => None,
    }
}

fn transform_collaboration_payload_to_revision(
    payload: &RuntimeCollaborationOperationPayload,
    revision: &str,
) -> RuntimeCollaborationOperationPayload {
    let mut payload = payload.clone();
    match &mut payload {
        RuntimeCollaborationOperationPayload::ChangeSet { target, .. } => {
            target.base_revision = revision.to_owned();
        }
        RuntimeCollaborationOperationPayload::PasteGraphFragment { request, .. } => {
            request.target.base_revision = revision.to_owned();
        }
        RuntimeCollaborationOperationPayload::UndoRedo { .. } => {}
    }
    payload
}

fn apply_collaboration_paste_operation(
    record: &RuntimeSessionRecord,
    operation: RuntimeCollaborationOperationEnvelope,
    runtime_operation: RuntimeOperationEnvelope,
    rebase: Option<RuntimeCollaborationRebase>,
) -> RuntimeCollaborationOperationResult {
    let sequence = record.collaboration.reserve_sequence();
    let mut session = record
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    let response = session.apply_runtime_operation(runtime_operation);
    if response.ok && response.applied {
        publish_session_event(
            record,
            RuntimeSessionEventKind::Mutate,
            &session,
            Vec::new(),
        );
    }
    let revision = response
        .revision_after
        .clone()
        .unwrap_or_else(|| response.revision_before.clone());
    let diagnostics: Vec<_> = response
        .diagnostics
        .iter()
        .map(|diagnostic| {
            collaboration_diagnostic_from_operation_diagnostic(
                diagnostic,
                &operation.operation_id,
                &operation.participant_id,
                &operation.idempotency_key,
            )
        })
        .collect();
    let status = if response.ok && response.applied {
        if rebase.is_some() {
            RuntimeCollaborationOperationStatus::Rebased
        } else {
            RuntimeCollaborationOperationStatus::Accepted
        }
    } else {
        RuntimeCollaborationOperationStatus::Rejected
    };
    let nack = if status == RuntimeCollaborationOperationStatus::Rejected {
        Some(RuntimeCollaborationNack {
            reason: RuntimeCollaborationNackReason::InvalidOperation,
            retryable: Some(response.conflict),
            diagnostics: Some(diagnostics.clone()),
        })
    } else {
        None
    };
    let ack = if status != RuntimeCollaborationOperationStatus::Rejected {
        Some(collaboration_ack(&operation, sequence, revision))
    } else {
        None
    };

    publish_collaboration_operation_result(
        record,
        RuntimeCollaborationOperationResult {
            schema: "skenion.runtime.collaboration.operation-result".to_owned(),
            schema_version: "0.1.0".to_owned(),
            session_id: record.id.clone(),
            operation_id: operation.operation_id,
            participant_id: operation.participant_id,
            idempotency_key: operation.idempotency_key,
            status,
            causal: operation.causal,
            ack,
            nack,
            rebase,
            diagnostics,
            created_at: created_at_now(),
        },
    )
}

fn apply_collaboration_change_set_operation(
    record: &RuntimeSessionRecord,
    operation: RuntimeCollaborationOperationEnvelope,
    lowered: LoweredCollaborationChangeSet,
    rebase: Option<RuntimeCollaborationRebase>,
) -> RuntimeCollaborationOperationResult {
    let sequence = record.collaboration.reserve_sequence();
    let mut session = record
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    let target = lowered.target.clone();
    let response = session.apply_collaboration_change_set_current(
        target.clone(),
        lowered.changes,
        lowered.actor_id,
        lowered.client_id,
        lowered.description,
    );
    if response.ok && response.applied {
        publish_session_event(
            record,
            RuntimeSessionEventKind::Mutate,
            &session,
            Vec::new(),
        );
    }
    let target_revision = session
        .target_revision_current(&target)
        .or_else(|| response.snapshot.graph_revision().map(ToOwned::to_owned))
        .unwrap_or_else(|| "0".to_owned());
    collaboration_result_from_patch_response(
        record,
        operation,
        response,
        sequence,
        rebase,
        Some(target_revision),
        Some(lowered.transformed_payload),
    )
}

fn apply_collaboration_undo_redo_operation(
    record: &RuntimeSessionRecord,
    operation: RuntimeCollaborationOperationEnvelope,
    action: RuntimeCollaborationUndoRedoAction,
    rebase: Option<RuntimeCollaborationRebase>,
) -> RuntimeCollaborationOperationResult {
    let sequence = record.collaboration.reserve_sequence();
    let mut session = record
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    let response = match action {
        RuntimeCollaborationUndoRedoAction::Undo => {
            session.undo_for_actor(&operation.participant_id)
        }
        RuntimeCollaborationUndoRedoAction::Redo => {
            session.redo_for_actor(&operation.participant_id)
        }
    };
    if response.ok && response.applied {
        publish_session_event(
            record,
            RuntimeSessionEventKind::Mutate,
            &session,
            Vec::new(),
        );
    }
    collaboration_result_from_patch_response(
        record, operation, response, sequence, rebase, None, None,
    )
}

fn collaboration_result_from_patch_response(
    record: &RuntimeSessionRecord,
    operation: RuntimeCollaborationOperationEnvelope,
    response: RuntimePatchResponse,
    sequence: u64,
    rebase: Option<RuntimeCollaborationRebase>,
    revision_override: Option<String>,
    transformed_payload: Option<RuntimeCollaborationOperationPayload>,
) -> RuntimeCollaborationOperationResult {
    let revision = revision_override.unwrap_or_else(|| {
        response
            .snapshot
            .graph_revision()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "0".to_owned())
    });
    let diagnostics: Vec<_> = response
        .diagnostics
        .iter()
        .map(|diagnostic| {
            collaboration_diagnostic_from_runtime_diagnostic(
                diagnostic,
                &operation.operation_id,
                &operation.participant_id,
                &operation.idempotency_key,
            )
        })
        .collect();
    let accepted = response.ok;
    let status = if accepted && rebase.is_some() {
        RuntimeCollaborationOperationStatus::Rebased
    } else if accepted {
        RuntimeCollaborationOperationStatus::Accepted
    } else {
        RuntimeCollaborationOperationStatus::Rejected
    };
    let nack = (!accepted).then(|| RuntimeCollaborationNack {
        reason: RuntimeCollaborationNackReason::InvalidOperation,
        retryable: Some(response.conflict),
        diagnostics: Some(diagnostics.clone()),
    });
    let ack = accepted.then(|| collaboration_ack(&operation, sequence, revision));
    let rebase = rebase.map(|mut rebase| {
        if rebase.transformed_payload.is_none() {
            rebase.transformed_payload = transformed_payload;
        }
        rebase
    });

    publish_collaboration_operation_result(
        record,
        RuntimeCollaborationOperationResult {
            schema: "skenion.runtime.collaboration.operation-result".to_owned(),
            schema_version: "0.1.0".to_owned(),
            session_id: record.id.clone(),
            operation_id: operation.operation_id,
            participant_id: operation.participant_id,
            idempotency_key: operation.idempotency_key,
            status,
            causal: operation.causal,
            ack,
            nack,
            rebase,
            diagnostics,
            created_at: created_at_now(),
        },
    )
}

fn publish_collaboration_operation_result(
    record: &RuntimeSessionRecord,
    result: RuntimeCollaborationOperationResult,
) -> RuntimeCollaborationOperationResult {
    validate_runtime_collaboration_operation_result(&result)
        .expect("runtime collaboration operation result should validate");
    record.collaboration.remember_result(result.clone());
    let sequence = match result.ack.as_ref() {
        Some(ack) => ack.sequence,
        None => record.collaboration.reserve_sequence(),
    };
    record
        .collaboration
        .publish_operation_result(&record.id, sequence, result.clone());
    result
}

fn duplicate_collaboration_operation_result(
    record: &RuntimeSessionRecord,
    operation: &RuntimeCollaborationOperationEnvelope,
) -> RuntimeCollaborationOperationResult {
    RuntimeCollaborationOperationResult {
        schema: "skenion.runtime.collaboration.operation-result".to_owned(),
        schema_version: "0.1.0".to_owned(),
        session_id: record.id.clone(),
        operation_id: operation.operation_id.clone(),
        participant_id: operation.participant_id.clone(),
        idempotency_key: operation.idempotency_key.clone(),
        status: RuntimeCollaborationOperationStatus::Duplicate,
        causal: operation.causal.clone(),
        ack: None,
        nack: Some(RuntimeCollaborationNack {
            reason: RuntimeCollaborationNackReason::DuplicateIdempotencyKey,
            retryable: Some(false),
            diagnostics: Some(vec![collaboration_diagnostic(
                "warning",
                "collaboration.duplicate-idempotency-key",
                "collaboration operation idempotency key was already processed",
                &operation.operation_id,
                &operation.participant_id,
                &operation.idempotency_key,
            )]),
        }),
        rebase: None,
        diagnostics: Vec::new(),
        created_at: created_at_now(),
    }
}

fn collaboration_rebase(
    record: &RuntimeSessionRecord,
    operation: &RuntimeCollaborationOperationEnvelope,
    current_revision: String,
    strategy: RuntimeCollaborationRebaseStrategy,
    transformed_payload: Option<RuntimeCollaborationOperationPayload>,
    conflicts: Vec<RuntimeCollaborationConflict>,
) -> RuntimeCollaborationRebase {
    let sequence = record.collaboration.current_sequence();
    RuntimeCollaborationRebase {
        from: operation.causal.clone(),
        to: RuntimeCollaborationCausalMetadata {
            base_revision: current_revision,
            base_sequence: sequence,
            vector: BTreeMap::from([("runtime".to_owned(), sequence)]),
            observed_operation_ids: Some(vec![operation.operation_id.clone()]),
        },
        strategy,
        transformed_payload,
        conflicts,
    }
}

fn invalid_collaboration_operation_result(
    record: &RuntimeSessionRecord,
    operation_id: &str,
    participant_id: &str,
    idempotency_key: &str,
    causal: RuntimeCollaborationCausalMetadata,
    message: String,
) -> RuntimeCollaborationOperationResult {
    RuntimeCollaborationOperationResult {
        schema: "skenion.runtime.collaboration.operation-result".to_owned(),
        schema_version: "0.1.0".to_owned(),
        session_id: record.id.clone(),
        operation_id: operation_id.to_owned(),
        participant_id: participant_id.to_owned(),
        idempotency_key: idempotency_key.to_owned(),
        status: RuntimeCollaborationOperationStatus::Rejected,
        causal,
        ack: None,
        nack: Some(RuntimeCollaborationNack {
            reason: RuntimeCollaborationNackReason::InvalidOperation,
            retryable: Some(false),
            diagnostics: Some(vec![collaboration_diagnostic(
                "error",
                "collaboration.invalid-operation",
                &message,
                operation_id,
                participant_id,
                idempotency_key,
            )]),
        }),
        rebase: None,
        diagnostics: Vec::new(),
        created_at: created_at_now(),
    }
}

fn invalid_collaboration_batch_result(
    session_id: &str,
    message: String,
) -> RuntimeCollaborationOperationBatchResult {
    let result = RuntimeCollaborationOperationResult {
        schema: "skenion.runtime.collaboration.operation-result".to_owned(),
        schema_version: "0.1.0".to_owned(),
        session_id: session_id.to_owned(),
        operation_id: "invalid-batch".to_owned(),
        participant_id: "runtime".to_owned(),
        idempotency_key: "invalid-batch".to_owned(),
        status: RuntimeCollaborationOperationStatus::Rejected,
        causal: RuntimeCollaborationCausalMetadata {
            base_revision: "0".to_owned(),
            base_sequence: 0,
            vector: BTreeMap::from([("runtime".to_owned(), 0)]),
            observed_operation_ids: None,
        },
        ack: None,
        nack: Some(RuntimeCollaborationNack {
            reason: RuntimeCollaborationNackReason::InvalidOperation,
            retryable: Some(false),
            diagnostics: Some(vec![RuntimeCollaborationOperationDiagnostic {
                severity: "error".to_owned(),
                code: "collaboration.invalid-batch".to_owned(),
                message: message.clone(),
                path: None,
                participant_id: Some("runtime".to_owned()),
                operation_id: Some("invalid-batch".to_owned()),
                idempotency_key: Some("invalid-batch".to_owned()),
                expected_revision: None,
                actual_revision: None,
                expected_sequence: None,
                actual_sequence: None,
            }]),
        }),
        rebase: None,
        diagnostics: Vec::new(),
        created_at: created_at_now(),
    };
    let batch_result = RuntimeCollaborationOperationBatchResult {
        schema: "skenion.runtime.collaboration.operation-batch-result".to_owned(),
        schema_version: "0.1.0".to_owned(),
        session_id: session_id.to_owned(),
        results: vec![result],
        diagnostics: vec![RuntimeCollaborationOperationDiagnostic {
            severity: "error".to_owned(),
            code: "collaboration.invalid-batch".to_owned(),
            message,
            path: None,
            participant_id: Some("runtime".to_owned()),
            operation_id: None,
            idempotency_key: None,
            expected_revision: None,
            actual_revision: None,
            expected_sequence: None,
            actual_sequence: None,
        }],
        created_at: created_at_now(),
    };
    validate_runtime_collaboration_operation_batch_result(&batch_result)
        .expect("invalid collaboration batch result should validate");
    batch_result
}

fn invalid_paste_operation_response(
    record: &RuntimeSessionRecord,
    message: String,
) -> crate::PasteGraphFragmentResponse {
    let revision_before = current_session_graph_revision(record);
    crate::PasteGraphFragmentResponse {
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
            message,
            path: None,
            target: None,
            expected_revision: None,
            actual_revision: None,
            duplicates: None,
            nodes: None,
            edges: None,
        }],
    }
}

fn collaboration_ack(
    operation: &RuntimeCollaborationOperationEnvelope,
    sequence: u64,
    revision: String,
) -> RuntimeCollaborationAck {
    let mut vector = operation.causal.vector.clone();
    vector.insert("runtime".to_owned(), sequence);
    vector.insert(operation.participant_id.clone(), sequence);
    RuntimeCollaborationAck {
        sequence,
        revision: revision.clone(),
        server_clock: RuntimeCollaborationServerClock {
            revision,
            sequence,
            vector,
        },
        applied_at: created_at_now(),
    }
}

fn collaboration_diagnostic_from_operation_diagnostic(
    diagnostic: &crate::RuntimeOperationDiagnostic,
    operation_id: &str,
    participant_id: &str,
    idempotency_key: &str,
) -> RuntimeCollaborationOperationDiagnostic {
    RuntimeCollaborationOperationDiagnostic {
        severity: diagnostic.severity.clone(),
        code: diagnostic.code.clone(),
        message: diagnostic.message.clone(),
        path: diagnostic.path.clone(),
        participant_id: Some(participant_id.to_owned()),
        operation_id: Some(operation_id.to_owned()),
        idempotency_key: Some(idempotency_key.to_owned()),
        expected_revision: diagnostic.expected_revision.clone(),
        actual_revision: diagnostic.actual_revision.clone(),
        expected_sequence: None,
        actual_sequence: None,
    }
}

fn collaboration_diagnostic_from_runtime_diagnostic(
    diagnostic: &RuntimeDiagnostic,
    operation_id: &str,
    participant_id: &str,
    idempotency_key: &str,
) -> RuntimeCollaborationOperationDiagnostic {
    RuntimeCollaborationOperationDiagnostic {
        severity: match &diagnostic.severity {
            DiagnosticSeverity::Error => "error",
            DiagnosticSeverity::Warning => "warning",
            DiagnosticSeverity::Info => "info",
        }
        .to_owned(),
        code: diagnostic
            .code
            .clone()
            .unwrap_or_else(|| "runtime.patch".to_owned()),
        message: diagnostic.message.clone(),
        path: None,
        participant_id: Some(participant_id.to_owned()),
        operation_id: Some(operation_id.to_owned()),
        idempotency_key: Some(idempotency_key.to_owned()),
        expected_revision: None,
        actual_revision: None,
        expected_sequence: None,
        actual_sequence: None,
    }
}

fn collaboration_diagnostic(
    severity: &str,
    code: &str,
    message: &str,
    operation_id: &str,
    participant_id: &str,
    idempotency_key: &str,
) -> RuntimeCollaborationOperationDiagnostic {
    RuntimeCollaborationOperationDiagnostic {
        severity: severity.to_owned(),
        code: code.to_owned(),
        message: message.to_owned(),
        path: None,
        participant_id: Some(participant_id.to_owned()),
        operation_id: Some(operation_id.to_owned()),
        idempotency_key: Some(idempotency_key.to_owned()),
        expected_revision: None,
        actual_revision: None,
        expected_sequence: None,
        actual_sequence: None,
    }
}

fn current_session_graph_revision(record: &RuntimeSessionRecord) -> String {
    record
        .session
        .read()
        .expect("runtime session lock should not be poisoned")
        .snapshot()
        .graph_revision()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "0".to_owned())
}

fn current_session_target_revision(
    record: &RuntimeSessionRecord,
    target: Option<&crate::GraphTargetRef>,
) -> String {
    let session = record
        .session
        .read()
        .expect("runtime session lock should not be poisoned");
    match target {
        Some(target) => session
            .target_revision_current(target)
            .or_else(|| target.target_revision.clone())
            .unwrap_or_else(|| target.base_revision.clone()),
        None => session
            .snapshot()
            .graph_revision()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "0".to_owned()),
    }
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
    let _coordination_guard = record.collaboration.operation_guard();
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
    let _coordination_guard = record.collaboration.operation_guard();
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
    let nodes = take_node_definitions_current(&mut value)?;
    let _ = take_frames_current(&mut value)?;
    let document = decode_project_document_current(value)?;
    Ok(ProjectRequestCurrent::from_project_document(
        document, nodes,
    ))
}

fn decode_run_project_document_request_current(
    mut value: serde_json::Value,
) -> Result<RunProjectRequestCurrent, Vec<RuntimeDiagnostic>> {
    let nodes = take_node_definitions_current(&mut value)?;
    let frames = take_frames_current(&mut value)?;
    let document = decode_project_document_current(value)?;
    Ok(RunProjectRequestCurrent::from_project_document(
        document, nodes, frames,
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

fn take_node_definitions_current(
    value: &mut serde_json::Value,
) -> Result<Vec<NodeDefinitionCurrent>, Vec<RuntimeDiagnostic>> {
    let nodes = value
        .as_object_mut()
        .and_then(|object| object.remove("nodes"))
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
    serde_json::from_value(nodes).map_err(invalid_project_payload)
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
    use skenion_contracts::{
        RuntimeCollaborationEventEnvelope, RuntimeCollaborationEventKind,
        RuntimeCollaborationOperationBatchResult, RuntimeCollaborationOperationResult,
        validate_runtime_collaboration_event_envelope,
        validate_runtime_collaboration_operation_batch_result,
        validate_runtime_collaboration_operation_result, validate_runtime_session_event,
    };
    use tower::ServiceExt;

    use crate::{
        CanvasNodeView, RuntimeIoDeviceDescriptor, RuntimeIoDeviceListResponse, RuntimeLogEvent,
        RuntimeLogSource, RuntimeSession, RuntimeSessionEvent, RuntimeViewPatch,
        RuntimeViewPatchOperation, io_device_manager::RuntimeIoDeviceRegistry,
        session_registry::DEFAULT_SESSION_ID,
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
        for removed in ["session.import.legacy.v0.1", "session.defaultAlias"] {
            assert!(
                !capabilities
                    .iter()
                    .any(|capability| capability.as_str() == Some(removed)),
                "removed compatibility capability {removed} should not be advertised"
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
    async fn session_addressed_route_family_covers_canonical_surface() {
        let app = runtime_router_with_dry_preview();
        let registry = RuntimeSessionRegistry::dry_preview();
        assert_eq!(registry.default_record().id, DEFAULT_SESSION_ID);

        post_json_with(
            app.clone(),
            "/v0/sessions/gamma/load",
            sample_project_document_current(),
        )
        .await;
        let default_info = get_json_with(app.clone(), "/v0/sessions/default/info").await;
        let gamma_info = get_json_with(app.clone(), "/v0/sessions/gamma/info").await;
        assert_eq!(default_info["sessionId"], DEFAULT_SESSION_ID);
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
            sample_shader_project_current(),
        )
        .await;
        assert_eq!(
            get_json_with(app.clone(), "/v0/sessions/shader/render/generated-shader").await["ok"],
            true
        );

        post_json_with(
            app.clone(),
            "/v0/sessions/delta/load",
            sample_project_document_current(),
        )
        .await;
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
                "/v0/sessions/gamma/operation",
                paste_operation("1")
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
            sample_project_document_current(),
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
        let app = runtime_router_with_fake_io_devices(Vec::new());

        let empty = get_json_with(app.clone(), "/v0/runtime/logs").await;
        assert_eq!(empty["schema"], "skenion.runtime.logs");
        assert_eq!(empty["events"], json!([]));
        assert_eq!(empty["retention"]["replayLimit"], 200);
        assert_eq!(
            empty["retention"]["replayLevels"],
            json!(["warning", "error"])
        );

        let undo = post_empty_with(app.clone(), "/v0/sessions/default/undo").await;
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
        let _ = post_empty_with(app.clone(), "/v0/sessions/default/undo").await;

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
                    .uri("/v0/sessions/default/events/stream")
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

    fn session_event_from_sse_text(text: &str) -> RuntimeSessionEvent {
        let data = text
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .collect::<Vec<_>>()
            .join("");
        serde_json::from_str(&data).expect("session SSE event data should parse")
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
        assert!(text.contains("\"cursor\":\"1\""));
        let event = session_event_from_sse_text(text);
        validate_runtime_session_event(&event).expect("live event should validate");
    }

    #[tokio::test]
    async fn session_event_stream_replays_after_query_and_last_event_id() {
        let state = runtime_state_with_dry_preview();
        let app = runtime_router_with_state(state.clone());
        post_json_with(
            app.clone(),
            "/v0/sessions/alpha/load",
            sample_project_document_current(),
        )
        .await;
        post_json_with(
            app.clone(),
            "/v0/sessions/alpha/load",
            sample_shader_project_current(),
        )
        .await;

        let record = state.sessions.get_or_create("alpha");
        assert_eq!(
            crate::session_registry::current_session_event_sequence(&record),
            2
        );

        let after_query = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v0/sessions/alpha/events/stream?after=1")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        let mut after_query_stream = after_query.into_body().into_data_stream();
        let after_query_chunk =
            tokio::time::timeout(Duration::from_secs(1), after_query_stream.next())
                .await
                .expect("session event stream should emit")
                .expect("session event stream should have a chunk")
                .expect("session event stream chunk should be ok");
        let after_query_text =
            std::str::from_utf8(&after_query_chunk).expect("session event stream should be utf8");
        let after_query_event = session_event_from_sse_text(after_query_text);

        assert_eq!(after_query_event.sequence, 2);
        assert!(after_query_event.replay.replayed);
        validate_runtime_session_event(&after_query_event)
            .expect("after query replay event should validate");
        assert_eq!(
            crate::session_registry::current_session_event_sequence(&record),
            2
        );

        let last_event_id = app
            .oneshot(
                Request::builder()
                    .uri("/v0/sessions/alpha/events/stream")
                    .header("last-event-id", "1")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        let mut last_event_id_stream = last_event_id.into_body().into_data_stream();
        let last_event_id_chunk =
            tokio::time::timeout(Duration::from_secs(1), last_event_id_stream.next())
                .await
                .expect("session event stream should emit")
                .expect("session event stream should have a chunk")
                .expect("session event stream chunk should be ok");
        let last_event_id_text =
            std::str::from_utf8(&last_event_id_chunk).expect("session event stream should be utf8");
        let last_event_id_event = session_event_from_sse_text(last_event_id_text);

        assert_eq!(last_event_id_event.sequence, 2);
        assert!(last_event_id_event.replay.replayed);
        validate_runtime_session_event(&last_event_id_event)
            .expect("Last-Event-ID replay event should validate");
        assert_eq!(
            crate::session_registry::current_session_event_sequence(&record),
            2
        );
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
            details: None,
        };
        let session = RuntimeSession::default();
        let record = RuntimeSessionRegistry::dry_preview().default_record();
        let session_event = session_snapshot_event(&record, &session);

        assert!(runtime_log_broadcast_event(Ok(log_event)).is_ok());
        assert!(runtime_log_broadcast_event(Err(BroadcastStreamRecvError::Lagged(1))).is_ok());
        assert!(
            session_broadcast_event_after_high_water(Ok(session_event), record.clone(), 0)
                .is_some()
        );
        assert!(
            session_broadcast_event_after_high_water(
                Err(BroadcastStreamRecvError::Lagged(1)),
                record,
                0
            )
            .is_some()
        );
    }

    #[tokio::test]
    async fn per_session_events_carry_session_id_sequence_and_replay_metadata() {
        let state = runtime_state_with_dry_preview();
        let app = runtime_router_with_state(state.clone());

        post_json_with(
            app.clone(),
            "/v0/sessions/alpha/load",
            sample_project_document_current(),
        )
        .await;
        post_json_with(
            app,
            "/v0/sessions/beta/load",
            sample_shader_project_current(),
        )
        .await;

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

        let replay = capture_session_replay(&alpha, Some(0), alpha_events[0].clone()).events;
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
                .any(|diagnostic| diagnostic["message"]
                    .as_str()
                    .unwrap()
                    .contains("ambiguous-algebraic-loop"))
        );

        let plan = post_json("/v0/plan", sample_ambiguous_loop_project_current()).await;
        assert_eq!(plan["ok"], false);
        assert!(
            plan["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("ambiguous-algebraic-loop")
        );

        let run = post_json("/v0/run", sample_ambiguous_loop_project_current()).await;
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
    async fn session_endpoint_returns_empty_state() {
        let response = get_json("/v0/sessions/default").await;

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

        let empty = get_json_with(app.clone(), "/v0/sessions/default").await;
        assert_eq!(empty["ok"], true);
        assert_eq!(empty["snapshot"]["project"], Value::Null);

        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;
        let project = get_json_with(app, "/v0/sessions/default").await;

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
            "topics": ["core.subpatch"]
        });
        let response = post_json_with(app.clone(), "/v0/sessions/default/load", project).await;

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
            "core.subpatch"
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

        let snapshot = get_json_with(app, "/v0/sessions/default").await;
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
    async fn session_load_rejects_missing_graph_schema_version() {
        let app = runtime_router();
        let response = post_json_with(
            app,
            "/v0/sessions/default/load",
            json!({ "graph": 42, "nodes": [] }),
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

        for path in ["/v0/sessions/default", "/v0/sessions/alpha"] {
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
            sample_project_document_current(),
        )
        .await;
        let explicit = get_json_with(app.clone(), "/v0/sessions/default").await;
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
        skenion_contracts::validate_runtime_session_info_response(&info)
            .expect("session info should validate against contracts");
    }

    #[tokio::test]
    async fn explicit_sessions_keep_graph_control_and_history_state_separate() {
        let app = runtime_router();

        post_json_with(
            app.clone(),
            "/v0/sessions/alpha/load",
            sample_project_document_current(),
        )
        .await;
        post_json_with(
            app.clone(),
            "/v0/sessions/beta/load",
            sample_shader_project_current(),
        )
        .await;
        post_json_with(
            app.clone(),
            "/v0/sessions/alpha/operation",
            paste_operation("1"),
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
        let loaded = post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;
        let mut invalid = sample_project_document_current();
        invalid["nodes"] = json!([]);

        let response = post_json_with(app.clone(), "/v0/sessions/default/load", invalid).await;

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

        let snapshot = get_json_with(app, "/v0/sessions/default").await;
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
    async fn session_validate_plan_and_run_use_loaded_project_document_patch_library() {
        let app = runtime_router();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_subpatch_project_document_current(),
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

    #[tokio::test]
    async fn session_operation_endpoint_applies_and_rejects_conflicts() {
        let app = runtime_router();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;

        let patched = post_json_with(
            app.clone(),
            "/v0/sessions/default/operation",
            paste_operation("1"),
        )
        .await;
        assert_eq!(patched["ok"], true);
        assert_eq!(patched["applied"], true);
        assert_eq!(patched["conflict"], false);
        assert_eq!(patched["revisionBefore"], "1");
        assert_eq!(patched["revisionAfter"], "2");

        let snapshot = get_json_with(app.clone(), "/v0/sessions/default").await;
        let history = get_json_with(app.clone(), "/v0/sessions/default/history").await;
        assert_eq!(snapshot["snapshot"]["project"]["graph"]["revision"], "2");
        assert_eq!(history["entries"][0]["kind"], "apply");
        assert_eq!(history["undoDepth"], 1);
        assert_eq!(history["redoDepth"], 0);
        assert_eq!(snapshot["snapshot"]["sessionRevision"], 2);
        assert_eq!(snapshot["snapshot"]["plan"]["graphRevision"], "2");

        let conflict =
            post_json_with(app, "/v0/sessions/default/operation", paste_operation("1")).await;
        assert_eq!(conflict["ok"], false);
        assert_eq!(conflict["applied"], false);
        assert_eq!(conflict["conflict"], true);
        assert!(
            conflict["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("does not match target graph revision")
        );
    }

    #[tokio::test]
    async fn actor_attribution_is_optional_and_client_metadata_is_preserved() {
        let app = runtime_router();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;

        let mut omitted_operation = paste_operation("1");
        omitted_operation
            .as_object_mut()
            .expect("operation should be an object")
            .remove("attribution");
        let omitted = post_json_with(
            app.clone(),
            "/v0/sessions/default/operation",
            omitted_operation,
        )
        .await;
        let mut attributed_operation = paste_operation("2");
        attributed_operation["attribution"]["clientId"] = json!("studio-window-a");
        let attributed = post_json_with(
            app.clone(),
            "/v0/sessions/default/operation",
            attributed_operation,
        )
        .await;
        let history = get_json_with(app, "/v0/sessions/default/history").await;

        assert_eq!(omitted["ok"], true);
        assert_eq!(attributed["ok"], true);
        assert_eq!(history["entries"][0]["clientId"], Value::Null);
        assert_eq!(history["entries"][1]["clientId"], "studio-window-a");
        assert_eq!(
            history["entries"][1]["mutation"]["clientId"],
            "studio-window-a"
        );
    }

    #[tokio::test]
    async fn session_operation_endpoint_pastes_graph_fragment() {
        let app = runtime_router();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;

        let response = post_json_with(
            app.clone(),
            "/v0/sessions/default/operation",
            paste_operation("1"),
        )
        .await;

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

        let snapshot = get_json_with(app, "/v0/sessions/default").await;
        assert_eq!(snapshot["snapshot"]["project"]["graph"]["revision"], "2");
        assert!(
            snapshot["snapshot"]["project"]["graph"]["edges"]
                .as_array()
                .unwrap()
                .iter()
                .any(|edge| {
                    edge["source"]["nodeId"] == "value_1_2"
                        && edge["source"]["portId"] == "value"
                        && edge["target"]["nodeId"] == "pasted_target"
                        && edge["target"]["portId"] == "cold"
                })
        );
    }

    #[tokio::test]
    async fn session_operation_endpoint_pastes_into_project_patch_definition_target() {
        let app = runtime_router();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_subpatch_project_document_current(),
        )
        .await;

        let response = post_json_with(
            app.clone(),
            "/v0/sessions/default/operation",
            patch_definition_paste_operation("1"),
        )
        .await;

        assert_eq!(response["ok"], true);
        assert_eq!(response["applied"], true);
        assert_eq!(response["revisionBefore"], "1");
        assert_eq!(response["revisionAfter"], "2");
        assert_eq!(
            response["target"]["path"]["kind"],
            "project-patch-definition"
        );

        let snapshot = get_json_with(app, "/v0/sessions/default").await;
        assert_eq!(snapshot["snapshot"]["project"]["graph"]["revision"], "1");
        assert_eq!(
            snapshot["snapshot"]["project"]["patchLibrary"][0]["revision"],
            "2"
        );
        assert_eq!(
            snapshot["snapshot"]["project"]["patchLibrary"][0]["graph"]["revision"],
            "2"
        );
        assert!(
            snapshot["snapshot"]["project"]["patchLibrary"][0]["graph"]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["id"] == "patch_debug")
        );
    }

    #[tokio::test]
    async fn session_operation_endpoint_rejects_invalid_envelope_json() {
        let response = post_json(
            "/v0/sessions/default/operation",
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
    async fn session_operations_endpoint_accepts_runtime_paste_operation_alias() {
        let app = runtime_router();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;

        let response = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            paste_operation("1"),
        )
        .await;

        assert_eq!(
            response["schema"],
            "skenion.runtime.paste-graph-fragment.response"
        );
        assert_eq!(response["ok"], true);
        assert_eq!(response["applied"], true);
        assert_eq!(response["revisionBefore"], "1");
        assert_eq!(response["revisionAfter"], "2");
    }

    #[tokio::test]
    async fn collaboration_operations_apply_paste_and_publish_result_events() {
        let state = runtime_state_with_dry_preview();
        let app = runtime_router_with_state(state.clone());
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;

        let response = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_paste_operation("1", "op-collab-paste", "idem-collab-paste"),
        )
        .await;
        let result: RuntimeCollaborationOperationResult =
            serde_json::from_value(response).expect("collaboration result should parse");
        validate_runtime_collaboration_operation_result(&result)
            .expect("collaboration result should validate");

        assert_eq!(result.status, RuntimeCollaborationOperationStatus::Accepted);
        assert_eq!(
            result.ack.as_ref().map(|ack| ack.revision.as_str()),
            Some("2")
        );
        assert_eq!(result.operation_id, "op-collab-paste");

        let snapshot = get_json_with(app, "/v0/sessions/default").await;
        assert_eq!(snapshot["snapshot"]["project"]["graph"]["revision"], "2");

        let replay = state
            .sessions
            .default_record()
            .collaboration
            .capture_replay(Some(0))
            .events;
        assert_eq!(replay.len(), 1);
        let event: RuntimeCollaborationEventEnvelope = replay[0].clone();
        validate_runtime_collaboration_event_envelope(&event)
            .expect("collaboration event should validate");
        assert_eq!(event.kind, RuntimeCollaborationEventKind::OperationResult);
    }

    #[tokio::test]
    async fn collaboration_operations_report_duplicate_idempotency_without_reapplying() {
        let app = runtime_router();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;
        post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_paste_operation("1", "op-collab-paste", "idem-collab-paste"),
        )
        .await;

        let duplicate = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_paste_operation("2", "op-collab-paste-retry", "idem-collab-paste"),
        )
        .await;
        let result: RuntimeCollaborationOperationResult =
            serde_json::from_value(duplicate).expect("duplicate result should parse");
        validate_runtime_collaboration_operation_result(&result)
            .expect("duplicate result should validate");

        assert_eq!(
            result.status,
            RuntimeCollaborationOperationStatus::Duplicate
        );
        assert_eq!(
            result.nack.as_ref().map(|nack| nack.reason.clone()),
            Some(RuntimeCollaborationNackReason::DuplicateIdempotencyKey)
        );
        let snapshot = get_json_with(app, "/v0/sessions/default").await;
        assert_eq!(snapshot["snapshot"]["project"]["graph"]["revision"], "2");
    }

    #[tokio::test]
    async fn collaboration_operations_serialize_concurrent_idempotency() {
        let app = runtime_router();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;

        let first = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_paste_operation("1", "op-concurrent-a", "idem-concurrent"),
        );
        let second = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_paste_operation("1", "op-concurrent-b", "idem-concurrent"),
        );
        let (first, second) = tokio::join!(first, second);
        let statuses = [first, second]
            .into_iter()
            .map(|value| {
                let result: RuntimeCollaborationOperationResult =
                    serde_json::from_value(value).expect("collaboration result should parse");
                validate_runtime_collaboration_operation_result(&result)
                    .expect("collaboration result should validate");
                result.status
            })
            .collect::<Vec<_>>();

        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == RuntimeCollaborationOperationStatus::Accepted)
                .count(),
            1
        );
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == RuntimeCollaborationOperationStatus::Duplicate)
                .count(),
            1
        );
        let snapshot = get_json_with(app, "/v0/sessions/default").await;
        assert_eq!(snapshot["snapshot"]["project"]["graph"]["revision"], "2");
    }

    #[tokio::test]
    async fn collaboration_operations_transform_stale_paste_to_current_revision() {
        let app = runtime_router();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;
        post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_paste_operation("1", "op-fresh", "idem-fresh"),
        )
        .await;

        let stale = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_paste_operation("1", "op-stale", "idem-stale"),
        )
        .await;
        let result: RuntimeCollaborationOperationResult =
            serde_json::from_value(stale).expect("stale result should parse");
        validate_runtime_collaboration_operation_result(&result)
            .expect("stale result should validate");

        assert_eq!(result.status, RuntimeCollaborationOperationStatus::Rebased);
        assert_eq!(
            result.rebase.as_ref().map(|rebase| rebase.strategy),
            Some(RuntimeCollaborationRebaseStrategy::CrdtMerge)
        );
        let transformed_payload = result
            .rebase
            .as_ref()
            .and_then(|rebase| rebase.transformed_payload.as_ref())
            .expect("stale paste should expose transformed payload");
        let transformed_payload_json = serde_json::to_value(transformed_payload)
            .expect("transformed payload should serialize");
        assert_eq!(transformed_payload_json["kind"], "pasteGraphFragment");
        assert_eq!(
            transformed_payload_json["request"]["target"]["baseRevision"],
            "2"
        );
        assert_eq!(
            result.ack.as_ref().map(|ack| ack.revision.as_str()),
            Some("3")
        );
        let snapshot = get_json_with(app, "/v0/sessions/default").await;
        assert_eq!(snapshot["snapshot"]["project"]["graph"]["revision"], "3");
    }

    #[tokio::test]
    async fn collaboration_operation_batches_report_mixed_accept_and_rebase_results() {
        let app = runtime_router();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;

        let response = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            json!({
              "schema": "skenion.runtime.collaboration.operation-batch",
              "schemaVersion": "0.1.0",
              "sessionId": "default",
              "operations": [
                collaboration_paste_operation("1", "op-batch-accepted", "idem-batch-accepted"),
                collaboration_paste_operation("1", "op-batch-rebased", "idem-batch-rebased")
              ],
              "submittedAt": "2026-06-22T00:00:00.000Z"
            }),
        )
        .await;
        let result: RuntimeCollaborationOperationBatchResult =
            serde_json::from_value(response).expect("batch result should parse");
        validate_runtime_collaboration_operation_batch_result(&result)
            .expect("batch result should validate");

        assert_eq!(result.results.len(), 2);
        assert_eq!(
            result.results[0].status,
            RuntimeCollaborationOperationStatus::Accepted
        );
        assert_eq!(
            result.results[1].status,
            RuntimeCollaborationOperationStatus::Rebased
        );
        assert_eq!(
            result.results[1]
                .ack
                .as_ref()
                .map(|ack| ack.revision.as_str()),
            Some("3")
        );
    }

    #[tokio::test]
    async fn collaboration_change_sets_apply_node_view_and_edge_operations() {
        let app = runtime_router();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;

        let accepted = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_change_set_operation(
                "1",
                "op-change-add",
                "idem-change-add",
                "participant-a",
                vec![
                    json!({
                          "op": "node.add",
                          "changeId": "change-add-gain",
                          "node": {
                            "id": "gain",
                            "kind": "core.float",
                            "kindVersion": "0.1.0",
                            "params": {},
                            "ports": value_f32_ports_current_json()
                          },
                      "view": { "x": 360.0, "y": 140.0 }
                    }),
                    json!({
                      "op": "node.move",
                      "changeId": "change-move-value",
                      "nodeId": "value_1",
                      "from": { "x": 96.0, "y": 96.0 },
                      "to": { "x": 160.0, "y": 140.0 }
                    }),
                    json!({
                      "op": "edge.connect",
                      "changeId": "change-connect-value-gain",
                      "edge": {
                        "id": "edge-value-gain",
                        "source": { "nodeId": "value_1", "portId": "value" },
                        "target": { "nodeId": "gain", "portId": "cold" }
                      }
                    }),
                    json!({
                      "op": "node.delete",
                      "changeId": "change-delete-target",
                      "nodeId": "target_1"
                    }),
                ],
            ),
        )
        .await;
        let result: RuntimeCollaborationOperationResult =
            serde_json::from_value(accepted).expect("change-set result should parse");
        validate_runtime_collaboration_operation_result(&result)
            .expect("change-set result should validate");
        assert_eq!(result.status, RuntimeCollaborationOperationStatus::Accepted);
        assert_eq!(
            result.ack.as_ref().map(|ack| ack.revision.as_str()),
            Some("2")
        );

        let snapshot = get_json_with(app.clone(), "/v0/sessions/default").await;
        assert_eq!(snapshot["snapshot"]["project"]["graph"]["revision"], "2");
        assert!(
            snapshot["snapshot"]["project"]["graph"]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["id"] == "gain")
        );
        assert!(
            snapshot["snapshot"]["project"]["graph"]["edges"]
                .as_array()
                .unwrap()
                .iter()
                .any(
                    |edge| edge["target"]["nodeId"] == "gain" && edge["target"]["portId"] == "cold"
                )
        );
        assert_eq!(
            snapshot["snapshot"]["project"]["viewState"]["canvas"]["nodes"]["value_1"]["x"],
            160.0
        );

        let stale_move = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_change_set_operation(
                "1",
                "op-change-stale-move",
                "idem-change-stale-move",
                "participant-a",
                vec![json!({
                  "op": "node.move",
                  "changeId": "change-stale-move-value",
                  "nodeId": "value_1",
                  "from": { "x": 160.0, "y": 140.0 },
                  "to": { "x": 220.0, "y": 180.0 }
                })],
            ),
        )
        .await;
        let result: RuntimeCollaborationOperationResult =
            serde_json::from_value(stale_move).expect("stale move result should parse");
        assert_eq!(result.status, RuntimeCollaborationOperationStatus::Rebased);
        assert_eq!(
            result
                .rebase
                .as_ref()
                .and_then(|rebase| rebase.transformed_payload.as_ref())
                .and_then(collaboration_payload_base_revision),
            Some("2")
        );

        let disconnected = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_change_set_operation(
                "2",
                "op-change-disconnect",
                "idem-change-disconnect",
                "participant-a",
                vec![json!({
                  "op": "edge.disconnect",
                  "changeId": "change-disconnect-value-gain",
                  "edgeId": "edge-value-gain"
                })],
            ),
        )
        .await;
        let result: RuntimeCollaborationOperationResult =
            serde_json::from_value(disconnected).expect("disconnect result should parse");
        validate_runtime_collaboration_operation_result(&result)
            .expect("disconnect result should validate");
        assert_eq!(result.status, RuntimeCollaborationOperationStatus::Accepted);

        let snapshot = get_json_with(app, "/v0/sessions/default").await;
        assert_eq!(snapshot["snapshot"]["project"]["graph"]["revision"], "3");
        assert!(
            snapshot["snapshot"]["project"]["graph"]["edges"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn collaboration_change_sets_resolve_patch_definition_target_revision() {
        let app = runtime_router();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_subpatch_project_document_current(),
        )
        .await;
        let root_bump = post_json_with(
            app.clone(),
            "/v0/sessions/default/operation",
            root_render_paste_operation("1"),
        )
        .await;
        assert_eq!(root_bump["ok"], true);

        let accepted = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_patch_change_set_operation(
                "1",
                "op-patch-change",
                "idem-patch-change",
                "participant-a",
                vec![json!({
                  "op": "node.add",
                  "changeId": "change-add-patch-debug",
                  "node": render_clear_color_node_current_json("patch_debug_collab")
                })],
            ),
        )
        .await;
        let result: RuntimeCollaborationOperationResult =
            serde_json::from_value(accepted).expect("patch change-set result should parse");
        validate_runtime_collaboration_operation_result(&result)
            .expect("patch change-set result should validate");
        assert_eq!(result.status, RuntimeCollaborationOperationStatus::Accepted);
        assert!(result.rebase.is_none());
        assert_eq!(
            result.ack.as_ref().map(|ack| ack.revision.as_str()),
            Some("2")
        );

        let stale = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_patch_change_set_operation(
                "1",
                "op-patch-stale-change",
                "idem-patch-stale-change",
                "participant-a",
                vec![json!({
                  "op": "node.add",
                  "changeId": "change-add-patch-debug-rebased",
                  "node": render_clear_color_node_current_json("patch_debug_rebased")
                })],
            ),
        )
        .await;
        let result: RuntimeCollaborationOperationResult =
            serde_json::from_value(stale).expect("stale patch change-set result should parse");
        validate_runtime_collaboration_operation_result(&result)
            .expect("stale patch change-set result should validate");
        assert_eq!(result.status, RuntimeCollaborationOperationStatus::Rebased);
        assert_eq!(
            result
                .rebase
                .as_ref()
                .and_then(|rebase| rebase.transformed_payload.as_ref())
                .and_then(collaboration_payload_base_revision),
            Some("2")
        );
        assert_eq!(
            result.ack.as_ref().map(|ack| ack.revision.as_str()),
            Some("3")
        );

        let snapshot = get_json_with(app, "/v0/sessions/default").await;
        assert_eq!(snapshot["snapshot"]["project"]["graph"]["revision"], "2");
        assert_eq!(
            snapshot["snapshot"]["project"]["patchLibrary"][0]["graph"]["revision"],
            "3"
        );
        let patch_nodes = snapshot["snapshot"]["project"]["patchLibrary"][0]["graph"]["nodes"]
            .as_array()
            .unwrap();
        assert!(
            patch_nodes
                .iter()
                .any(|node| node["id"] == "patch_debug_collab")
        );
        assert!(
            patch_nodes
                .iter()
                .any(|node| node["id"] == "patch_debug_rebased")
        );
    }

    #[tokio::test]
    async fn collaboration_undo_redo_uses_participant_scoped_history() {
        let app = runtime_router();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;
        post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_paste_operation_for("1", "op-a-paste", "idem-a-paste", "participant-a"),
        )
        .await;
        post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_paste_operation_for("2", "op-b-paste", "idem-b-paste", "participant-b"),
        )
        .await;

        let undone = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_undo_redo_operation(
                "3",
                "op-a-undo",
                "idem-a-undo",
                "participant-a",
                "undo",
            ),
        )
        .await;
        let result: RuntimeCollaborationOperationResult =
            serde_json::from_value(undone).expect("undo result should parse");
        validate_runtime_collaboration_operation_result(&result)
            .expect("undo result should validate");
        assert_eq!(result.status, RuntimeCollaborationOperationStatus::Accepted);
        assert_eq!(
            result.ack.as_ref().map(|ack| ack.revision.as_str()),
            Some("4")
        );

        let snapshot = get_json_with(app, "/v0/sessions/default").await;
        let nodes = snapshot["snapshot"]["project"]["graph"]["nodes"]
            .as_array()
            .unwrap();
        assert!(!nodes.iter().any(|node| node["id"] == "pasted_target"));
        assert!(nodes.iter().any(|node| node["id"] == "pasted_target_2"));

        let app = runtime_router();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;
        post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_paste_operation_for(
                "1",
                "op-redo-paste",
                "idem-redo-paste",
                "participant-a",
            ),
        )
        .await;
        post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_undo_redo_operation(
                "2",
                "op-redo-undo",
                "idem-redo-undo",
                "participant-a",
                "undo",
            ),
        )
        .await;
        let redone = post_json_with(
            app,
            "/v0/sessions/default/operations",
            collaboration_undo_redo_operation(
                "3",
                "op-redo-redo",
                "idem-redo-redo",
                "participant-a",
                "redo",
            ),
        )
        .await;
        let result: RuntimeCollaborationOperationResult =
            serde_json::from_value(redone).expect("redo result should parse");
        assert_eq!(result.status, RuntimeCollaborationOperationStatus::Accepted);
        assert_eq!(
            result.ack.as_ref().map(|ack| ack.revision.as_str()),
            Some("4")
        );
    }

    #[tokio::test]
    async fn collaboration_presence_selection_and_stream_routes_publish_events() {
        let state = runtime_state_with_dry_preview();
        let app = runtime_router_with_state(state.clone());

        let presence = post_json_with(
            app.clone(),
            "/v0/sessions/beta/collaboration/presence",
            collaboration_presence("wrong-session", "participant-a"),
        )
        .await;
        assert_eq!(presence["sessionId"], "beta");
        assert_eq!(presence["participantId"], "participant-a");

        let live_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v0/sessions/default/collaboration/events/stream?after=0")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        assert_eq!(live_response.status(), StatusCode::OK);
        let live_presence = post_json_with(
            app.clone(),
            "/v0/sessions/default/collaboration/presence",
            collaboration_presence("wrong-session", "participant-live"),
        )
        .await;
        assert_eq!(live_presence["sessionId"], "default");
        let mut live_stream = live_response.into_body().into_data_stream();
        let live_chunk = tokio::time::timeout(Duration::from_secs(1), live_stream.next())
            .await
            .expect("default collaboration stream should emit")
            .expect("default collaboration stream should have a chunk")
            .expect("default collaboration stream chunk should be ok");
        let live_text =
            std::str::from_utf8(&live_chunk).expect("collaboration stream should be utf8");
        assert!(live_text.contains("\"participantId\":\"participant-live\""));

        let selection = post_json_with(
            app.clone(),
            "/v0/sessions/beta/collaboration/selection",
            collaboration_selection("wrong-session", "participant-a"),
        )
        .await;
        assert_eq!(selection["sessionId"], "beta");

        let default_selection = post_json_with(
            app.clone(),
            "/v0/sessions/default/collaboration/selection",
            collaboration_selection("wrong-session", "participant-live"),
        )
        .await;
        assert_eq!(default_selection["sessionId"], "default");

        let beta_replay = state
            .sessions
            .get_or_create("beta")
            .collaboration
            .capture_replay(Some(0));
        assert_eq!(beta_replay.events.len(), 2);
        assert_eq!(
            beta_replay.events[0].kind,
            RuntimeCollaborationEventKind::Presence
        );
        assert_eq!(
            beta_replay.events[1].kind,
            RuntimeCollaborationEventKind::Selection
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v0/sessions/beta/collaboration/events/stream?after=0")
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
            .expect("collaboration event stream should emit")
            .expect("collaboration event stream should have a chunk")
            .expect("collaboration event stream chunk should be ok");
        let text = std::str::from_utf8(&chunk).expect("collaboration stream should be utf8");
        assert!(text.contains("event: collaboration"));
        assert!(text.contains("\"kind\":\"presence\""));

        let header_response = app
            .oneshot(
                Request::builder()
                    .uri("/v0/sessions/beta/collaboration/events/stream")
                    .header("last-event-id", "0")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        assert_eq!(header_response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn collaboration_presence_and_selection_reject_invalid_metadata() {
        let app = runtime_router();
        let mut invalid_presence = collaboration_presence("default", "participant-invalid");
        invalid_presence["expiresAt"] = invalid_presence["updatedAt"].clone();
        let (presence_status, presence_body) = post_json_status_with(
            app.clone(),
            "/v0/sessions/default/collaboration/presence",
            invalid_presence,
        )
        .await;
        assert_eq!(presence_status, StatusCode::BAD_REQUEST);
        assert_eq!(presence_body["ok"], false);
        assert_eq!(
            presence_body["diagnostics"][0]["code"],
            "collaboration.invalid-presence"
        );

        let mut invalid_selection = collaboration_selection("default", "participant-invalid");
        invalid_selection["expiresAt"] = invalid_selection["updatedAt"].clone();
        let (selection_status, selection_body) = post_json_status_with(
            app,
            "/v0/sessions/default/collaboration/selection",
            invalid_selection,
        )
        .await;
        assert_eq!(selection_status, StatusCode::BAD_REQUEST);
        assert_eq!(selection_body["ok"], false);
        assert_eq!(
            selection_body["diagnostics"][0]["code"],
            "collaboration.invalid-selection"
        );
    }

    #[tokio::test]
    async fn collaboration_operations_report_structured_invalid_inputs() {
        let app = runtime_router();

        let invalid_runtime_operation = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            json!({
              "schema": "skenion.runtime.operation",
              "schemaVersion": "0.1.0",
              "kind": "pasteGraphFragment"
            }),
        )
        .await;
        assert_eq!(invalid_runtime_operation["ok"], false);
        assert_eq!(
            invalid_runtime_operation["diagnostics"][0]["code"],
            "paste.operation.invalid-json"
        );

        let invalid_batch = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            json!({
              "schema": "skenion.runtime.collaboration.operation-batch",
              "schemaVersion": "0.1.0",
              "sessionId": "default"
            }),
        )
        .await;
        let batch: RuntimeCollaborationOperationBatchResult =
            serde_json::from_value(invalid_batch).expect("invalid batch result should parse");
        validate_runtime_collaboration_operation_batch_result(&batch)
            .expect("invalid batch result should validate");
        assert_eq!(
            batch.results[0].status,
            RuntimeCollaborationOperationStatus::Rejected
        );

        let duplicate_batch = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            json!({
              "schema": "skenion.runtime.collaboration.operation-batch",
              "schemaVersion": "0.1.0",
              "sessionId": "default",
              "operations": [
                collaboration_undo_redo_operation("0", "op-dup-a", "same-idem", "participant-a", "undo"),
                collaboration_undo_redo_operation("0", "op-dup-b", "same-idem", "participant-a", "undo")
              ],
              "submittedAt": "2026-06-22T00:00:00.000Z"
            }),
        )
        .await;
        let batch: RuntimeCollaborationOperationBatchResult =
            serde_json::from_value(duplicate_batch).expect("duplicate batch result should parse");
        assert_eq!(batch.results[0].operation_id, "invalid-batch");

        let invalid_single = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            json!({ "schema": "not-a-runtime-collaboration-operation" }),
        )
        .await;
        let result: RuntimeCollaborationOperationResult =
            serde_json::from_value(invalid_single).expect("invalid single result should parse");
        validate_runtime_collaboration_operation_result(&result)
            .expect("invalid single result should validate");
        assert_eq!(result.status, RuntimeCollaborationOperationStatus::Rejected);

        let session_mismatch_batch = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            json!({
              "schema": "skenion.runtime.collaboration.operation-batch",
              "schemaVersion": "0.1.0",
              "sessionId": "other",
              "operations": [],
              "submittedAt": "2026-06-22T00:00:00.000Z"
            }),
        )
        .await;
        let batch: RuntimeCollaborationOperationBatchResult =
            serde_json::from_value(session_mismatch_batch)
                .expect("session mismatch batch result should parse");
        assert_eq!(batch.results[0].operation_id, "invalid-batch");

        let mut invalid_operation =
            collaboration_paste_operation("1", "op-invalid", "idem-invalid");
        invalid_operation["schemaVersion"] = json!("bogus");
        let invalid_operation = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            invalid_operation,
        )
        .await;
        let result: RuntimeCollaborationOperationResult = serde_json::from_value(invalid_operation)
            .expect("invalid operation result should parse");
        assert_eq!(result.status, RuntimeCollaborationOperationStatus::Rejected);

        let mut mismatch = collaboration_paste_operation("1", "op-mismatch", "idem-mismatch");
        mismatch["sessionId"] = json!("other");
        let mismatch =
            post_json_with(app.clone(), "/v0/sessions/default/operations", mismatch).await;
        let result: RuntimeCollaborationOperationResult =
            serde_json::from_value(mismatch).expect("mismatch result should parse");
        assert_eq!(result.status, RuntimeCollaborationOperationStatus::Rejected);

        post_json_with(
            app.clone(),
            "/v0/sessions/gamma/load",
            sample_project_document_current(),
        )
        .await;
        let mut by_id_operation =
            collaboration_paste_operation_for("1", "op-by-id", "idem-by-id", "participant-a");
        by_id_operation["sessionId"] = json!("gamma");
        let by_id = post_json_with(app, "/v0/sessions/gamma/operations", by_id_operation).await;
        let result: RuntimeCollaborationOperationResult =
            serde_json::from_value(by_id).expect("by-id result should parse");
        assert_eq!(result.status, RuntimeCollaborationOperationStatus::Accepted);
    }

    #[tokio::test]
    async fn collaboration_change_set_and_undo_failures_are_nacked() {
        let app = runtime_router();

        let no_project = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_change_set_operation(
                "0",
                "op-no-project-change",
                "idem-no-project-change",
                "participant-a",
                vec![json!({
                  "op": "node.add",
                  "changeId": "change-add-without-project",
                  "node": {
                    "id": "gain",
                    "kind": "core.float",
                    "kindVersion": "0.1.0",
                    "params": {},
                    "ports": value_f32_ports_current_json()
                  }
                })],
            ),
        )
        .await;
        let result: RuntimeCollaborationOperationResult =
            serde_json::from_value(no_project).expect("no-project result should parse");
        validate_runtime_collaboration_operation_result(&result)
            .expect("no-project result should validate");
        assert_eq!(result.status, RuntimeCollaborationOperationStatus::Rejected);
        assert_eq!(
            result.nack.as_ref().map(|nack| nack.reason.clone()),
            Some(RuntimeCollaborationNackReason::InvalidOperation)
        );

        let no_project_paste = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_paste_operation("0", "op-no-project-paste", "idem-no-project-paste"),
        )
        .await;
        let result: RuntimeCollaborationOperationResult =
            serde_json::from_value(no_project_paste).expect("no-project paste result should parse");
        validate_runtime_collaboration_operation_result(&result)
            .expect("no-project paste result should validate");
        assert_eq!(result.status, RuntimeCollaborationOperationStatus::Rejected);
        assert!(result.ack.is_none());
        assert!(
            result
                .nack
                .as_ref()
                .and_then(|nack| nack.diagnostics.as_ref())
                .unwrap()[0]
                .code
                .contains("paste.target.no-project")
        );

        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;
        let unknown_edge = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_change_set_operation(
                "1",
                "op-unknown-edge",
                "idem-unknown-edge",
                "participant-a",
                vec![json!({
                  "op": "edge.disconnect",
                  "changeId": "change-disconnect-missing",
                  "edgeId": "missing-edge-id"
                })],
            ),
        )
        .await;
        let result: RuntimeCollaborationOperationResult =
            serde_json::from_value(unknown_edge).expect("unknown-edge result should parse");
        assert_eq!(result.status, RuntimeCollaborationOperationStatus::Rejected);
        assert!(
            result
                .nack
                .as_ref()
                .and_then(|nack| nack.diagnostics.as_ref())
                .unwrap()[0]
                .message
                .contains("cannot resolve edge id")
        );

        let empty_undo = post_json_with(
            app.clone(),
            "/v0/sessions/default/operations",
            collaboration_undo_redo_operation(
                "1",
                "op-empty-undo",
                "idem-empty-undo",
                "participant-a",
                "undo",
            ),
        )
        .await;
        let result: RuntimeCollaborationOperationResult =
            serde_json::from_value(empty_undo).expect("empty undo result should parse");
        assert_eq!(result.status, RuntimeCollaborationOperationStatus::Rejected);

        let empty_redo = post_json_with(
            app,
            "/v0/sessions/default/operations",
            collaboration_undo_redo_operation(
                "1",
                "op-empty-redo",
                "idem-empty-redo",
                "participant-a",
                "redo",
            ),
        )
        .await;
        let result: RuntimeCollaborationOperationResult =
            serde_json::from_value(empty_redo).expect("empty redo result should parse");
        assert_eq!(result.status, RuntimeCollaborationOperationStatus::Rejected);
    }

    #[test]
    fn collaboration_private_helpers_cover_direct_branch_shapes() {
        let state = RuntimeServerState::default();
        let record = state.sessions.default_record();
        let operation: RuntimeCollaborationOperationEnvelope =
            serde_json::from_value(collaboration_undo_redo_operation(
                "1",
                "op-helper",
                "idem-helper",
                "participant-a",
                "undo",
            ))
            .expect("operation should parse");

        let response = {
            let mut session = record
                .session
                .write()
                .expect("runtime session lock should not be poisoned");
            assert!(
                session
                    .load_project_current(sample_project_request_current())
                    .ok
            );
            session.apply_mutation(RuntimeMutationRequest {
                graph_patch: None,
                view_patch: Some(RuntimeViewPatch {
                    base_view_revision: 1,
                    ops: vec![RuntimeViewPatchOperation::MoveNodeView {
                        node_id: "value_1".to_owned(),
                        from: None,
                        to: CanvasNodeView {
                            x: 120.0,
                            y: 120.0,
                            width: None,
                            height: None,
                            collapsed: None,
                        },
                    }],
                }),
                actor_id: Some("participant-a".to_owned()),
                client_id: Some("studio-test".to_owned()),
                description: Some("view-only helper mutation".to_owned()),
            })
        };
        let rebase = collaboration_rebase(
            &record,
            &operation,
            "2".to_owned(),
            RuntimeCollaborationRebaseStrategy::CrdtMerge,
            None,
            Vec::new(),
        );
        let result = collaboration_result_from_patch_response(
            &record,
            operation.clone(),
            response,
            record.collaboration.reserve_sequence(),
            Some(rebase),
            None,
            Some(operation.payload.clone()),
        );
        assert_eq!(result.status, RuntimeCollaborationOperationStatus::Rebased);
        assert!(result.rebase.unwrap().transformed_payload.is_some());

        let warning = collaboration_diagnostic_from_runtime_diagnostic(
            &RuntimeDiagnostic {
                severity: DiagnosticSeverity::Warning,
                message: "warning".to_owned(),
                code: None,
                details: None,
            },
            "op-helper",
            "participant-a",
            "idem-helper",
        );
        assert_eq!(warning.severity, "warning");
        assert_eq!(warning.code, "runtime.patch");

        let info = collaboration_diagnostic_from_runtime_diagnostic(
            &RuntimeDiagnostic {
                severity: DiagnosticSeverity::Info,
                message: "info".to_owned(),
                code: Some("custom.info".to_owned()),
                details: None,
            },
            "op-helper",
            "participant-a",
            "idem-helper",
        );
        assert_eq!(info.severity, "info");
        assert_eq!(info.code, "custom.info");
    }

    #[tokio::test]
    async fn session_mutate_endpoint_reports_errors_without_loaded_session() {
        let response = post_json(
            "/v0/sessions/default/mutate",
            graph_mutation(set_value_patch("1")),
        )
        .await;

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
    async fn session_mutate_endpoint_rejects_graph_patch_mutations() {
        let app = runtime_router();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;

        let response = post_json_with(
            app,
            "/v0/sessions/default/mutate",
            graph_mutation(set_value_patch("1")),
        )
        .await;

        assert_eq!(response["ok"], false);
        assert_eq!(response["applied"], false);
        assert_eq!(response["conflict"], false);
        assert_eq!(response["snapshot"]["project"]["graph"]["revision"], "1");
        assert_eq!(response["history"]["entries"].as_array().unwrap().len(), 0);
        assert_eq!(
            response["diagnostics"][0]["code"],
            "project.graph-patch-unsupported"
        );
    }

    #[tokio::test]
    async fn session_load_and_addressed_mutate_cover_decode_and_event_branches() {
        let state = runtime_state_with_dry_preview();
        let app = runtime_router_with_state(state.clone());

        let bad_load = post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            json!({ "graph": { "schemaVersion": "0.1.0" } }),
        )
        .await;
        assert_eq!(bad_load["ok"], false);
        assert!(
            bad_load["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("invalid project request")
        );

        let bad_mutation = post_json_with(
            app.clone(),
            "/v0/sessions/gamma/mutate",
            json!({ "viewPatch": { "baseViewRevision": "wrong", "ops": [] } }),
        )
        .await;
        assert_eq!(bad_mutation["ok"], false);
        assert!(
            bad_mutation["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("invalid runtime mutation")
        );

        post_json_with(
            app.clone(),
            "/v0/sessions/gamma/load",
            sample_project_document_current(),
        )
        .await;
        let moved = post_json_with(
            app.clone(),
            "/v0/sessions/gamma/mutate",
            json!({
              "viewPatch": {
                "baseViewRevision": 1,
                "ops": [
                  {
                    "op": "moveNodeView",
                    "nodeId": "value_1",
                    "to": { "x": 128.0, "y": 64.0 }
                  }
                ]
              },
              "clientId": "studio-test",
              "description": "move through addressed route"
            }),
        )
        .await;
        assert_eq!(moved["ok"], true);
        assert_eq!(moved["applied"], true);
        assert_eq!(
            moved["snapshot"]["project"]["viewState"]["canvas"]["nodes"]["value_1"]["x"],
            128.0
        );

        let gamma = state.sessions.get_or_create("gamma");
        let events = gamma
            .event_store
            .lock()
            .expect("event store should not be poisoned")
            .clone();
        assert!(
            events
                .iter()
                .any(|event| event.kind == RuntimeSessionEventKind::Mutate)
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
    async fn session_history_endpoint_returns_empty_and_event_history() {
        let app = runtime_router();

        let empty = get_json_with(app.clone(), "/v0/sessions/default/history").await;
        assert_eq!(empty["schema"], "skenion.runtime.history");
        assert_eq!(empty["entries"].as_array().unwrap().len(), 0);
        assert_eq!(empty["canUndo"], false);
        assert_eq!(empty["canRedo"], false);

        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;
        post_json_with(
            app.clone(),
            "/v0/sessions/default/operation",
            paste_operation("1"),
        )
        .await;
        let history = get_json_with(app, "/v0/sessions/default/history").await;

        assert_eq!(history["entries"].as_array().unwrap().len(), 1);
        assert_eq!(history["entries"][0]["kind"], "apply");
        assert_eq!(history["undoDepth"], 1);
        assert_eq!(history["redoDepth"], 0);
    }

    #[tokio::test]
    async fn session_undo_and_redo_endpoints_update_graph_and_history() {
        let app = runtime_router();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;
        post_json_with(
            app.clone(),
            "/v0/sessions/default/operation",
            paste_operation("1"),
        )
        .await;

        let undo = post_empty_with(app.clone(), "/v0/sessions/default/undo").await;
        assert_eq!(undo["ok"], true);
        assert_eq!(undo["applied"], true);
        assert_eq!(undo["history"]["entries"][1]["kind"], "undo");
        assert_eq!(undo["snapshot"]["project"]["graph"]["revision"], "3");
        assert_eq!(undo["history"]["entries"].as_array().unwrap().len(), 2);
        assert_eq!(undo["history"]["undoDepth"], 0);
        assert_eq!(undo["history"]["redoDepth"], 1);

        let redo = post_empty_with(app, "/v0/sessions/default/redo").await;
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

        let undo = post_empty_with(app.clone(), "/v0/sessions/default/undo").await;
        let redo = post_empty_with(app, "/v0/sessions/default/redo").await;

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
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;

        let set = post_json_with(
            app.clone(),
            "/v0/sessions/default/control/event",
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
            "/v0/sessions/default/control/event",
            json!({ "nodeId": "value_1", "portId": "in", "message": { "selector": "bang", "atoms": [] } }),
        )
        .await;
        assert_eq!(bang["ok"], true);
        assert_eq!(
            bang["emitted"],
            json!([
                { "nodeId": "value_1", "portId": "value", "message": { "selector": "float", "atoms": [{ "type": "float", "representation": "f32", "value": 32.0 }] } }
            ])
        );

        let input = post_json_with(
            app.clone(),
            "/v0/sessions/default/control/event",
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
                { "nodeId": "value_1", "portId": "value", "message": { "selector": "float", "atoms": [{ "type": "float", "representation": "f32", "value": 12.0 }] } }
            ])
        );

        let state = get_json_with(app.clone(), "/v0/sessions/default/control/state").await;
        assert_eq!(state["ok"], true);
        assert_eq!(
            state["values"]["value_1"],
            json!({ "type": "float", "representation": "f32", "value": 12.0 })
        );

        let state_read = post_json_with(
            app.clone(),
            "/v0/sessions/default/control/read",
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
            "/v0/sessions/default/control/read",
            json!({ "nodeId": "value_1", "target": "port", "id": "value" }),
        )
        .await;
        assert_eq!(port_read["ok"], true);
        assert_eq!(port_read["value"]["value"]["id"], json!("value"));

        let wrong_type = post_json_with(
            app,
            "/v0/sessions/default/control/event",
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
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;
        post_empty_with(app.clone(), "/v0/sessions/default/preview/start").await;
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
            "/v0/sessions/default/control/event",
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
            "/v0/sessions/default/control/event",
            json!({
                "nodeId": "value_1",
                "portId": "in",
                "message": { "selector": "set", "atoms": [{ "type": "float", "representation": "f32", "value": 1.0 }] }
            }),
        )
        .await;
        let state = get_json_with(app, "/v0/sessions/default/control/state").await;
        let read = post_json_with(
            runtime_router(),
            "/v0/sessions/default/control/read",
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
            sample_project_document_current(),
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
            sample_project_document_current(),
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
    async fn preview_start_request_restart_replaces_existing_preview() {
        let app = runtime_router_with_dry_preview();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;
        post_empty_with(app.clone(), "/v0/sessions/default/preview/start").await;
        post_json_with(
            app.clone(),
            "/v0/sessions/default/operation",
            paste_operation("1"),
        )
        .await;

        let stale = get_json_with(app.clone(), "/v0/sessions/default/preview").await;
        assert_eq!(stale["state"], "running");
        assert_eq!(stale["graphRevision"], "1");
        assert_eq!(stale["sessionRevision"], 2);
        assert_eq!(stale["previewSessionRevision"], 1);
        assert_eq!(stale["stale"], true);

        let restarted = post_json_with(
            app,
            "/v0/sessions/default/preview/start",
            json!({ "restart": true }),
        )
        .await;
        assert_eq!(restarted["ok"], true);
        assert_eq!(restarted["graphRevision"], "2");
        assert_eq!(restarted["previewSessionRevision"], 2);
        assert_eq!(restarted["stale"], false);
    }

    #[tokio::test]
    async fn preview_start_rejects_invalid_request_json() {
        let app = runtime_router_with_dry_preview();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
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
            sample_project_document_current(),
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
            sample_project_document_current(),
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
            sample_project_document_current(),
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
    async fn telemetry_endpoint_marks_preview_stale_after_patch() {
        let app = runtime_router_with_dry_preview();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_project_document_current(),
        )
        .await;
        post_empty_with(app.clone(), "/v0/sessions/default/preview/start").await;
        post_json_with(
            app.clone(),
            "/v0/sessions/default/operation",
            paste_operation("1"),
        )
        .await;

        let response = get_json_with(app, "/v0/sessions/default/telemetry").await;

        assert_eq!(response["session"]["sessionRevision"], 2);
        assert_eq!(response["preview"]["state"], "running");
        assert_eq!(response["preview"]["previewSessionRevision"], 1);
        assert_eq!(response["preview"]["stale"], true);
    }

    #[tokio::test]
    async fn generated_shader_endpoint_returns_source_and_source_map() {
        let app = runtime_router_with_dry_preview();
        post_json_with(
            app.clone(),
            "/v0/sessions/default/load",
            sample_shader_project_current(),
        )
        .await;

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

        let app = runtime_router_with_dry_preview();
        let mut project = sample_shader_project_current();
        project["graph"]["nodes"][0]["params"]["source"] = json!(
            "// @skenion.uniform bad vec3\n@fragment\nfn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }"
        );
        let loaded = post_json_with(app.clone(), "/v0/sessions/default/load", project).await;
        assert_eq!(loaded["ok"], true);

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

    async fn post_json_status_with(app: Router, path: &str, payload: Value) -> (StatusCode, Value) {
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
        write_server_package_manifest(
            package_dir,
            &format!(
                r#"{{
                  "schema": "skenion.package.manifest",
                  "schemaVersion": "0.1.0",
                  "id": "{package_id}",
                  "version": "0.46.0",
                  "category": "patch",
                  "source": "workspace",
                  "root": "package",
                  "trust": "trusted",
                  "contracts": {{
                    "line": "0.46",
                    "range": ">=0.46.0 <0.47.0"
                  }},
                  "provides": {{
                    "patches": [
                      {{
                        "id": "{package_id}.main",
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

    fn sample_project_document_current() -> Value {
        json!({
          "schema": "skenion.project",
          "schemaVersion": "0.1.0",
          "id": "minimal-value-project",
          "revision": "1",
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
                "ports": value_f32_ports_current_json()
              },
              {
                "id": "target_1",
                "kind": "core.float",
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
                "resolvedType": "number.float"
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
          "patchLibrary": [],
          "nodes": [
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "core.float",
              "version": "0.1.0",
              "displayName": "Float Value",
              "category": "Values",
              "ports": value_f32_ports_current_json(),
              "execution": { "model": "value" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": []
            }
          ]
        })
    }

    fn sample_project_request_current() -> ProjectRequestCurrent {
        let mut value = sample_project_document_current();
        let nodes = value
            .as_object_mut()
            .and_then(|object| object.remove("nodes"))
            .expect("sample project document should carry node definitions");
        let nodes = serde_json::from_value::<Vec<NodeDefinitionCurrent>>(nodes)
            .expect("nodes should parse");
        let document =
            serde_json::from_value::<ProjectDocumentCurrent>(value).expect("document should parse");
        ProjectRequestCurrent::from_project_document(document, nodes)
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

    fn sample_shader_project_current() -> Value {
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
                    "type": "number.float",
                    "rate": "control",
                    "required": false,
                    "defaultValue": 0.5,
                    "triggerMode": "latched"
                  },
                  {
                    "id": "out",
                    "direction": "output",
                    "label": "Out",
                    "type": "gpu.texture2d",
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
              "id": "render.fullscreen-shader",
              "version": "0.1.0",
              "displayName": "Fullscreen Shader",
              "category": "Render",
              "ports": [
                {
                  "id": "speed",
                  "direction": "input",
                  "label": "Speed",
                  "type": "number.float",
                  "rate": "control",
                  "required": false,
                  "defaultValue": 0.5,
                  "triggerMode": "latched"
                },
                {
                  "id": "out",
                  "direction": "output",
                  "label": "Out",
                  "type": "gpu.texture2d",
                  "rate": "resource"
                }
              ],
              "execution": { "model": "gpu_pass", "clock": "frame" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": []
            }
          ]
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
                "kind": "render.clear-color",
                "kindVersion": "0.1.0",
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
                "kindVersion": "0.1.0",
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
              "schemaVersion": "0.1.0",
              "id": "render.clear-color",
              "version": "0.1.0",
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
              "capabilities": ["render.frame.v0.1"]
            },
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "render.output",
              "version": "0.1.0",
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
              "capabilities": ["render.output.v0.1"]
            }
          ]
        })
    }

    fn sample_subpatch_project_document_current() -> Value {
        json!({
          "schema": "skenion.project",
          "schemaVersion": "0.1.0",
          "id": "subpatch-project",
          "revision": "1",
          "graph": {
            "schema": "skenion.graph",
            "schemaVersion": "0.1.0",
            "id": "subpatch-project-root",
            "revision": "1",
            "nodes": [
              {
                "id": "clear_color",
                "kind": "render.clear-color",
                "kindVersion": "0.1.0",
                "params": { "color": [0.12, 0.2, 0.34, 1] },
                "ports": [
                  { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
                ]
              },
              {
                "id": "fx",
                "kind": "core.subpatch",
                "kindVersion": "0.1.0",
                "params": { "patchRef": "identity" },
                "ports": [
                  { "id": "in", "direction": "input", "type": "render.frame", "rate": "render", "required": true },
                  { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
                ]
              },
              {
                "id": "output",
                "kind": "render.output",
                "kindVersion": "0.1.0",
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
                "schemaVersion": "0.1.0",
                "id": "identity-frame-patch",
                "revision": "1",
                "nodes": [
                  {
                    "id": "patch_in",
                    "kind": "core.inlet",
                    "kindVersion": "0.1.0",
                    "params": { "portId": "in", "label": "Input" },
                    "ports": [
                      { "id": "out", "direction": "output", "type": "render.frame", "rate": "render", "description": "Frame entering the patch" }
                    ]
                  },
                  {
                    "id": "pass",
                    "kind": "test.pass",
                    "kindVersion": "0.1.0",
                    "params": {},
                    "ports": [
                      { "id": "in", "direction": "input", "type": "render.frame", "rate": "render", "required": true },
                      { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
                    ]
                  },
                  {
                    "id": "patch_out",
                    "kind": "core.outlet",
                    "kindVersion": "0.1.0",
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
              "schemaVersion": "0.1.0",
              "id": "render.clear-color",
              "version": "0.1.0",
              "displayName": "Clear Color",
              "category": "Render",
              "ports": [
                { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
              ],
              "execution": { "model": "gpu_pass", "clock": "frame" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": ["render.frame.v0.1"]
            },
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "render.output",
              "version": "0.1.0",
              "displayName": "Render Output",
              "category": "Render",
              "ports": [
                { "id": "in", "direction": "input", "type": "render.frame", "rate": "render", "required": true }
              ],
              "execution": { "model": "gpu_pass", "clock": "frame" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": ["render.output.v0.1"]
            },
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "test.pass",
              "version": "0.1.0",
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
                "kind": "core.value-transform",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": [
                  { "id": "in", "direction": "input", "type": "value.number", "rate": "control" },
                  { "id": "out", "direction": "output", "type": "value.number", "rate": "control" }
                ]
              },
              {
                "id": "b",
                "kind": "core.value-transform",
                "kindVersion": "0.1.0",
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
              "schemaVersion": "0.1.0",
              "id": "core.value-transform",
              "version": "0.1.0",
              "displayName": "Value Transform",
              "category": "Core",
              "ports": [
                { "id": "in", "direction": "input", "type": "value.number", "rate": "control" },
                { "id": "out", "direction": "output", "type": "value.number", "rate": "control" }
              ],
              "execution": { "model": "value" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": ["value.number.v0.1"]
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
                  "schemaVersion": "0.1.0",
                  "nodes": [
                    {
                      "id": "value_1",
                      "kind": "core.float",
                      "kindVersion": "0.1.0",
                      "params": {},
                      "ports": value_f32_ports_current_json()
                    },
                    {
                      "id": "pasted_target",
                      "kind": "core.float",
                      "kindVersion": "0.1.0",
                      "params": {},
                      "ports": value_f32_ports_current_json()
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

    fn root_render_paste_operation(base_revision: &str) -> Value {
        paste_render_node_operation(base_revision, json!({ "kind": "root" }), "root_debug")
    }

    fn patch_definition_paste_operation(base_revision: &str) -> Value {
        paste_render_node_operation(
            base_revision,
            json!({ "kind": "project-patch-definition", "patchId": "identity" }),
            "patch_debug",
        )
    }

    fn paste_render_node_operation(base_revision: &str, path: Value, node_id: &str) -> Value {
        json!({
          "schema": "skenion.runtime.operation",
          "schemaVersion": "0.1.0",
          "id": format!("op-paste-{node_id}"),
          "kind": "pasteGraphFragment",
          "request": {
            "target": {
              "path": path,
              "baseRevision": base_revision
            },
            "fragment": {
              "schema": "skenion.graph.fragment",
              "schemaVersion": "0.1.0",
              "nodes": [
                render_clear_color_node_current_json(node_id)
              ],
              "edges": []
            },
            "options": {
              "idConflictPolicy": "remap"
            }
          },
          "attribution": {
            "clientId": "studio-test",
            "label": "Paste render test node"
          }
        })
    }

    fn collaboration_paste_operation(
        base_revision: &str,
        operation_id: &str,
        idempotency_key: &str,
    ) -> Value {
        collaboration_paste_operation_for(
            base_revision,
            operation_id,
            idempotency_key,
            "participant-a",
        )
    }

    fn collaboration_paste_operation_for(
        base_revision: &str,
        operation_id: &str,
        idempotency_key: &str,
        participant_id: &str,
    ) -> Value {
        json!({
          "schema": "skenion.runtime.collaboration.operation",
          "schemaVersion": "0.1.0",
          "operationId": operation_id,
          "sessionId": "default",
          "participantId": participant_id,
          "idempotencyKey": idempotency_key,
          "causal": {
            "baseRevision": base_revision,
            "baseSequence": 0,
            "vector": { participant_id: 0 }
          },
          "payload": {
            "kind": "pasteGraphFragment",
            "request": {
              "target": {
                "path": { "kind": "root" },
                "baseRevision": base_revision
              },
                  "fragment": {
                    "schema": "skenion.graph.fragment",
                    "schemaVersion": "0.1.0",
                    "nodes": [
                      {
                        "id": "value_1",
                        "kind": "core.float",
                        "kindVersion": "0.1.0",
                        "params": {},
                        "ports": value_f32_ports_current_json()
                      },
                      {
                        "id": "pasted_target",
                        "kind": "core.float",
                        "kindVersion": "0.1.0",
                        "params": {},
                        "ports": value_f32_ports_current_json()
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
            "description": "Collaborative paste test fragment"
          },
          "correlationId": "studio-test",
          "submittedAt": "2026-06-22T00:00:00.000Z"
        })
    }

    fn collaboration_change_set_operation(
        base_revision: &str,
        operation_id: &str,
        idempotency_key: &str,
        participant_id: &str,
        changes: Vec<Value>,
    ) -> Value {
        collaboration_change_set_operation_with_path(
            base_revision,
            operation_id,
            idempotency_key,
            participant_id,
            json!({ "kind": "root" }),
            changes,
        )
    }

    fn collaboration_patch_change_set_operation(
        base_revision: &str,
        operation_id: &str,
        idempotency_key: &str,
        participant_id: &str,
        changes: Vec<Value>,
    ) -> Value {
        collaboration_change_set_operation_with_path(
            base_revision,
            operation_id,
            idempotency_key,
            participant_id,
            json!({ "kind": "project-patch-definition", "patchId": "identity" }),
            changes,
        )
    }

    fn collaboration_change_set_operation_with_path(
        base_revision: &str,
        operation_id: &str,
        idempotency_key: &str,
        participant_id: &str,
        path: Value,
        changes: Vec<Value>,
    ) -> Value {
        json!({
          "schema": "skenion.runtime.collaboration.operation",
          "schemaVersion": "0.1.0",
          "operationId": operation_id,
          "sessionId": "default",
          "participantId": participant_id,
          "idempotencyKey": idempotency_key,
          "causal": {
            "baseRevision": base_revision,
            "baseSequence": 0,
            "vector": { participant_id: 0 }
          },
          "payload": {
            "kind": "changeSet",
            "target": {
              "path": path,
              "baseRevision": base_revision
            },
            "changes": changes,
            "description": "Collaborative change-set test"
          },
          "correlationId": "studio-test",
          "submittedAt": "2026-06-22T00:00:00.000Z"
        })
    }

    fn render_clear_color_node_current_json(node_id: &str) -> Value {
        json!({
          "id": node_id,
          "kind": "render.clear-color",
          "kindVersion": "0.1.0",
          "params": { "color": [0.02, 0.04, 0.08, 1.0] },
          "ports": [
            { "id": "out", "direction": "output", "type": "render.frame", "rate": "render" }
          ]
        })
    }

    fn collaboration_undo_redo_operation(
        base_revision: &str,
        operation_id: &str,
        idempotency_key: &str,
        participant_id: &str,
        action: &str,
    ) -> Value {
        json!({
          "schema": "skenion.runtime.collaboration.operation",
          "schemaVersion": "0.1.0",
          "operationId": operation_id,
          "sessionId": "default",
          "participantId": participant_id,
          "idempotencyKey": idempotency_key,
          "causal": {
            "baseRevision": base_revision,
            "baseSequence": 0,
            "vector": { participant_id: 0 }
          },
          "payload": {
            "kind": "undoRedo",
            "action": action,
            "scope": {
              "kind": "participant",
              "participantId": participant_id
            }
          },
          "correlationId": "studio-test",
          "submittedAt": "2026-06-22T00:00:00.000Z"
        })
    }

    fn collaboration_presence(session_id: &str, participant_id: &str) -> Value {
        json!({
          "schema": "skenion.runtime.collaboration.presence",
          "schemaVersion": "0.1.0",
          "sessionId": session_id,
          "participantId": participant_id,
          "presence": {
            "state": "active",
            "displayName": "Participant A"
          },
          "updatedAt": "2026-06-22T00:00:00.000Z",
          "expiresAt": "2026-06-22T00:05:00.000Z"
        })
    }

    fn collaboration_selection(session_id: &str, participant_id: &str) -> Value {
        json!({
          "schema": "skenion.runtime.collaboration.selection",
          "schemaVersion": "0.1.0",
          "sessionId": session_id,
          "participantId": participant_id,
          "target": {
            "path": { "kind": "root" },
            "baseRevision": "1"
          },
          "selection": {
            "ranges": [
              { "kind": "nodes", "nodeIds": ["value_1"] }
            ],
            "activeRangeIndex": 0
          },
          "cursor": {
            "kind": "canvas",
            "x": 12.0,
            "y": 34.0
          },
          "updatedAt": "2026-06-22T00:00:01.000Z",
          "expiresAt": "2026-06-22T00:05:01.000Z"
        })
    }

    fn value_f32_ports_current_json() -> Value {
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
