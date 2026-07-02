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
    response::{IntoResponse, Response},
    routing::{get, post},
};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::{
    GeneratedShaderResponse, IssueSeverity, PackageRegistryListResponseV01, PreviewDocument,
    ProjectRequestCurrent, RuntimeControlReadRequest, RuntimeControlReadResponse,
    RuntimeControlStateResponse, RuntimeExtensionListResponse, RuntimeIoDeviceListResponse,
    RuntimeIssue, RuntimePreviewStartRequest, RuntimeSessionEventKind, RuntimeSessionInfoResponse,
    RuntimeSessionResponse, ShaderIssue, ShaderIssuePhase, ShaderIssueSource,
    asset_store::{
        RuntimeAssetGetResponse, RuntimeAssetImportResponse, RuntimeAssetListResponse, store_asset,
    },
    generated_shader_response_from_preview_document, http_live_disabled,
    realtime::{handle_runtime_realtime_socket, node_catalog_snapshot_for_record},
    request_payload::{
        RuntimeSessionLoadPayload, decode_runtime_session_load_request_payload,
        validate_session_load_precondition,
    },
    runtime_info::{HealthResponse, RuntimeInfoResponse, health_response, runtime_info_response},
    session_registry::{RuntimeSessionRecord, publish_session_event},
    sidecar::{
        RuntimeSidecarHealthResponse, RuntimeSidecarShutdownResponse,
        RuntimeSidecarStartupResponse, runtime_connection_profile, sidecar_shutdown_response,
    },
};

mod logs;
mod state;
mod telemetry;

use logs::{runtime_logs, runtime_logs_stream};
pub use state::RuntimeServerState;
use telemetry::{session_telemetry_by_id, session_telemetry_stream_by_id};

pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 3761;
const MAX_ASSET_UPLOAD_BYTES: usize = 512 * 1024 * 1024;

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
            get(http_live_disabled::session_events_stream),
        )
        .route("/v0/sessions/{session_id}/load", post(load_session_by_id))
        .route(
            "/v0/sessions/{session_id}/mutate",
            post(http_live_disabled::mutate),
        )
        .route(
            "/v0/sessions/{session_id}/operation",
            post(http_live_disabled::operation),
        )
        .route(
            "/v0/sessions/{session_id}/operations",
            post(http_live_disabled::operations),
        )
        .route(
            "/v0/sessions/{session_id}/collaboration/presence",
            post(http_live_disabled::collaboration_presence),
        )
        .route(
            "/v0/sessions/{session_id}/collaboration/selection",
            post(http_live_disabled::collaboration_selection),
        )
        .route(
            "/v0/sessions/{session_id}/collaboration/events/stream",
            get(http_live_disabled::collaboration_events_stream),
        )
        .route(
            "/v0/sessions/{session_id}/history",
            get(session_history_by_id),
        )
        .route(
            "/v0/sessions/{session_id}/undo",
            post(http_live_disabled::undo),
        )
        .route(
            "/v0/sessions/{session_id}/redo",
            post(http_live_disabled::redo),
        )
        .route(
            "/v0/sessions/{session_id}/control/event",
            post(http_live_disabled::control_event),
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
    Json(health_response())
}

async fn runtime_info() -> Json<RuntimeInfoResponse> {
    Json(runtime_info_response())
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
    State(_state): State<RuntimeServerState>,
    body: Bytes,
) -> Json<RuntimeSidecarShutdownResponse> {
    let response = sidecar_shutdown_response(&body);
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

fn session_json(
    _state: &RuntimeServerState,
    response: crate::RuntimeSessionResponse,
) -> Json<crate::RuntimeSessionResponse> {
    Json(response)
}

fn control_read_json(
    _state: &RuntimeServerState,
    response: RuntimeControlReadResponse,
) -> Json<RuntimeControlReadResponse> {
    Json(response)
}

fn preview_status_json(
    _state: &RuntimeServerState,
    response: crate::RuntimePreviewStatusResponse,
) -> Json<crate::RuntimePreviewStatusResponse> {
    Json(response)
}

fn asset_import_json(
    _state: &RuntimeServerState,
    response: RuntimeAssetImportResponse,
) -> Json<RuntimeAssetImportResponse> {
    Json(response)
}

fn asset_get_json(
    _state: &RuntimeServerState,
    response: RuntimeAssetGetResponse,
) -> Json<RuntimeAssetGetResponse> {
    Json(response)
}

fn generated_shader_json(
    _state: &RuntimeServerState,
    response: GeneratedShaderResponse,
) -> Json<GeneratedShaderResponse> {
    Json(response)
}

async fn io_devices(State(state): State<RuntimeServerState>) -> Json<RuntimeIoDeviceListResponse> {
    let response = state.io_devices.list_devices();
    Json(response)
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
                "issue": {
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
    session_json(state, session.response(true, Vec::new()))
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
                "issue": {
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
    let (request, load_repair_issues) = match decode_runtime_session_load_request_payload(value) {
        Ok(RuntimeSessionLoadPayload::Current {
            request,
            repair_issues,
        }) => (request, repair_issues),
        Err(issues) => {
            let response = session.response(false, issues);
            return session_json(state, response);
        }
    };
    if let Err(issues) = validate_session_load_precondition(&session, &request) {
        let response = session.response(false, issues);
        return session_json(state, response);
    }
    let project_request =
        ProjectRequestCurrent::from_project_document(request.project.clone(), Vec::new());
    let mut response = session.load_project_current_with_package_registry(
        project_request,
        Some(state.packages.revision()),
        Some(state.packages.response()),
    );
    if response.ok && !load_repair_issues.is_empty() {
        response.issues = session.append_loaded_project_issues(load_repair_issues);
        response.snapshot.issues = response.issues.clone();
    }
    record_session_load_repair_log(state, &record, &response);
    if response.ok && response.snapshot.loaded() {
        publish_session_event(
            &record,
            RuntimeSessionEventKind::Load,
            &session,
            response.issues.clone(),
        );
    }
    session_json(state, response)
}

fn record_session_load_repair_log(
    state: &RuntimeServerState,
    record: &RuntimeSessionRecord,
    response: &RuntimeSessionResponse,
) {
    if !response.ok {
        return;
    }
    let dropped_edge_ids = response
        .issues
        .iter()
        .filter(|issue| issue.code.as_deref() == Some("project.load.edge-dropped"))
        .filter_map(|issue| {
            issue
                .details
                .as_ref()
                .and_then(|details| details.get("edgeId"))
                .and_then(|edge_id| edge_id.as_str())
                .map(ToOwned::to_owned)
        })
        .collect::<Vec<_>>();
    let obsolete_implementation_version_paths = response
        .issues
        .iter()
        .filter(|issue| issue.code.as_deref() == Some("project.load.obsolete-field-dropped"))
        .filter_map(|issue| {
            issue
                .details
                .as_ref()
                .and_then(|details| details.get("path"))
                .and_then(|path| path.as_str())
                .map(ToOwned::to_owned)
        })
        .collect::<Vec<_>>();
    if dropped_edge_ids.is_empty() && obsolete_implementation_version_paths.is_empty() {
        return;
    }

    let message = match (
        dropped_edge_ids.len(),
        obsolete_implementation_version_paths.len(),
    ) {
        (edge_count, 0) => format!(
            "Runtime loaded session {} after dropping {edge_count} incompatible edge(s).",
            record.id
        ),
        (0, field_count) => format!(
            "Runtime loaded session {} after dropping {field_count} obsolete object implementation version field(s).",
            record.id
        ),
        (edge_count, field_count) => format!(
            "Runtime loaded session {} after dropping {edge_count} incompatible edge(s) and {field_count} obsolete object implementation version field(s).",
            record.id
        ),
    };
    state.logs.record_event(
        IssueSeverity::Warning,
        Some("project.load.repaired".to_owned()),
        message,
        Some(serde_json::json!({
            "sessionId": record.id,
            "droppedEdgeIds": dropped_edge_ids,
            "obsoleteImplementationVersionPaths": obsolete_implementation_version_paths,
            "graphId": response.snapshot.graph_id(),
            "graphRevision": response.snapshot.graph_revision(),
            "sessionRevision": response.snapshot.session_revision,
        })),
    );
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
            response.issues.clone(),
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
        Err(issue) => {
            let preview = record
                .preview
                .lock()
                .expect("runtime preview lock should not be poisoned");
            let response = preview.request_error(snapshot, issue);
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
        Err(issues) => GeneratedShaderResponse {
            ok: false,
            node_id: None,
            language: None,
            source: None,
            source_map: None,
            issues: issues
                .into_iter()
                .map(|issue| {
                    ShaderIssue::error(
                        ShaderIssuePhase::SourceSync,
                        "generated-shader-unavailable",
                        issue.message,
                        ShaderIssueSource::Runtime,
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
                    issues: vec![RuntimeIssue::error(format!(
                        "failed to read uploaded asset bytes: {error}"
                    ))],
                };
                return asset_import_json(&state, response);
            }
        };

        let response = store_asset(&state.assets, name, mime_type, bytes);
        return asset_import_json(&state, response);
    }

    let response = RuntimeAssetImportResponse {
        ok: false,
        asset: None,
        issues: vec![RuntimeIssue::error(
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
        .list();
    Json(RuntimeAssetListResponse {
        ok: true,
        assets,
        issues: Vec::new(),
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
        .get(&asset_id);
    let ok = asset.is_some();
    let response = RuntimeAssetGetResponse {
        ok,
        asset,
        issues: if ok {
            Vec::new()
        } else {
            vec![RuntimeIssue::error(format!(
                "asset {asset_id} does not exist"
            ))]
        },
    };
    asset_get_json(&state, response)
}

fn preview_start_request(body: &[u8]) -> Result<RuntimePreviewStartRequest, RuntimeIssue> {
    if body.is_empty() {
        return Ok(RuntimePreviewStartRequest { restart: false });
    }
    serde_json::from_slice(body)
        .map_err(|error| RuntimeIssue::error(format!("invalid preview start request: {error}")))
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
mod tests;
