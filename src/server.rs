use std::sync::{Arc, RwLock};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderValue, Method, header::CONTENT_TYPE},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::{
    DummyExecutionReport, ExecutionPlan, GraphDocument, GraphPatch, NodeDefinition, NodeRegistry,
    RuntimeSession, SessionRunRequest, build_execution_plan, run_dummy_execution, validate_project,
};

pub const RUNTIME_API_VERSION: &str = "0.1.0";
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 3761;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRequest {
    pub graph: GraphDocument,
    pub nodes: Vec<NodeDefinition>,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeDiagnostic {
    pub severity: DiagnosticSeverity,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Error,
}

#[derive(Clone)]
pub struct RuntimeServerState {
    pub session: Arc<RwLock<RuntimeSession>>,
}

impl Default for RuntimeServerState {
    fn default() -> Self {
        Self {
            session: Arc::new(RwLock::new(RuntimeSession::default())),
        }
    }
}

pub fn runtime_router() -> Router {
    runtime_router_with_state(RuntimeServerState::default())
}

pub fn runtime_router_with_state(state: RuntimeServerState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v0/runtime/info", get(runtime_info))
        .route("/v0/validate", post(validate_project_endpoint))
        .route("/v0/plan", post(plan_project_endpoint))
        .route("/v0/run", post(run_project_endpoint))
        .route("/v0/session", get(session_snapshot).delete(clear_session))
        .route("/v0/session/load", post(load_session))
        .route("/v0/session/validate", post(validate_session))
        .route("/v0/session/plan", post(plan_session))
        .route("/v0/session/run", post(run_session))
        .route("/v0/session/patch", post(patch_session))
        .with_state(state)
        .layer(cors_layer())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        service: "skenion-runtime",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn runtime_info() -> Json<RuntimeInfoResponse> {
    Json(RuntimeInfoResponse {
        name: "skenion-runtime",
        version: env!("CARGO_PKG_VERSION"),
        api_version: RUNTIME_API_VERSION,
        capabilities: vec![
            "project.validate",
            "project.plan",
            "dummy.run",
            "session.load",
            "session.validate",
            "session.plan",
            "session.run",
            "session.patch",
            "session.clear",
        ],
    })
}

async fn validate_project_endpoint(
    Json(request): Json<ProjectRequest>,
) -> Json<RuntimeApiResponse> {
    let diagnostics = validate_project_request(&request.graph, request.nodes);
    Json(match diagnostics {
        Ok(()) => RuntimeApiResponse::ok(),
        Err(diagnostics) => RuntimeApiResponse::diagnostics(diagnostics),
    })
}

async fn plan_project_endpoint(Json(request): Json<ProjectRequest>) -> Json<RuntimeApiResponse> {
    let registry = match registry_from_nodes(request.nodes) {
        Ok(registry) => registry,
        Err(diagnostics) => return Json(RuntimeApiResponse::diagnostics(diagnostics)),
    };

    if let Err(diagnostics) = validate_graph_with_registry(&request.graph, &registry) {
        return Json(RuntimeApiResponse::diagnostics(diagnostics));
    }

    let plan =
        build_execution_plan(&request.graph, &registry).expect("validated project should plan");
    Json(RuntimeApiResponse {
        ok: true,
        diagnostics: Vec::new(),
        plan: Some(plan),
        report: None,
    })
}

async fn run_project_endpoint(Json(request): Json<RunProjectRequest>) -> Json<RuntimeApiResponse> {
    let registry = match registry_from_nodes(request.nodes) {
        Ok(registry) => registry,
        Err(diagnostics) => return Json(RuntimeApiResponse::diagnostics(diagnostics)),
    };

    if let Err(diagnostics) = validate_graph_with_registry(&request.graph, &registry) {
        return Json(RuntimeApiResponse::diagnostics(diagnostics));
    }

    let plan =
        build_execution_plan(&request.graph, &registry).expect("validated project should plan");
    let report = run_dummy_execution(&plan, request.frames.unwrap_or(1));
    Json(RuntimeApiResponse {
        ok: true,
        diagnostics: Vec::new(),
        plan: Some(plan),
        report: Some(report),
    })
}

async fn session_snapshot(
    State(state): State<RuntimeServerState>,
) -> Json<crate::RuntimeSessionResponse> {
    let session = state
        .session
        .read()
        .expect("runtime session lock should not be poisoned");
    Json(session.response(true, session.snapshot().diagnostics, None))
}

async fn load_session(
    State(state): State<RuntimeServerState>,
    Json(request): Json<ProjectRequest>,
) -> Json<crate::RuntimeSessionResponse> {
    let mut session = state
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    Json(session.load_project(request))
}

async fn validate_session(
    State(state): State<RuntimeServerState>,
) -> Json<crate::RuntimeSessionResponse> {
    let mut session = state
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    Json(session.validate_current())
}

async fn plan_session(
    State(state): State<RuntimeServerState>,
) -> Json<crate::RuntimeSessionResponse> {
    let mut session = state
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    Json(session.plan_current())
}

async fn run_session(
    State(state): State<RuntimeServerState>,
    Json(request): Json<SessionRunRequest>,
) -> Json<crate::RuntimeSessionResponse> {
    let mut session = state
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    Json(session.run_current(request.frames.unwrap_or(1)))
}

async fn patch_session(
    State(state): State<RuntimeServerState>,
    Json(value): Json<serde_json::Value>,
) -> Json<crate::RuntimePatchResponse> {
    let mut session = state
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    let patch = match serde_json::from_value::<GraphPatch>(value) {
        Ok(patch) => patch,
        Err(error) => {
            return Json(session.reject_patch(
                false,
                vec![RuntimeDiagnostic::error(format!(
                    "invalid graph patch: {error}"
                ))],
            ));
        }
    };

    Json(session.apply_patch(patch))
}

async fn clear_session(
    State(state): State<RuntimeServerState>,
) -> Json<crate::RuntimeSessionResponse> {
    let mut session = state
        .session
        .write()
        .expect("runtime session lock should not be poisoned");
    Json(session.clear())
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

fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _| {
            matches!(
                origin.to_str(),
                Ok("http://127.0.0.1:5173" | "http://localhost:5173")
            )
        }))
        .allow_methods([Method::DELETE, Method::GET, Method::POST])
        .allow_headers([CONTENT_TYPE])
}

#[cfg(test)]
mod tests {
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

    use super::*;

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
        assert_eq!(response["capabilities"][0], "project.validate");
        assert_eq!(response["capabilities"][1], "project.plan");
        assert_eq!(response["capabilities"][2], "dummy.run");
        assert_eq!(response["capabilities"][3], "session.load");
        assert!(
            response["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|capability| capability == "session.patch")
        );
    }

    #[tokio::test]
    async fn cors_allows_local_studio_origin() {
        let response = runtime_router()
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/v0/runtime/info")
                    .header(ORIGIN, "http://127.0.0.1:5173")
                    .header(ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
            "http://127.0.0.1:5173"
        );
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
    async fn session_endpoint_returns_empty_state() {
        let response = get_json("/v0/session").await;

        assert_eq!(response["ok"], true);
        assert_eq!(response["loaded"], false);
        assert_eq!(response["graphId"], Value::Null);
        assert_eq!(response["graphRevision"], Value::Null);
        assert_eq!(response["sessionRevision"], 0);
        assert_eq!(response["diagnostics"].as_array().unwrap().len(), 0);
        assert_eq!(response["plan"], Value::Null);
        assert_eq!(response["report"], Value::Null);
    }

    #[tokio::test]
    async fn session_load_stores_valid_project() {
        let app = runtime_router();
        let response = post_json_with(app.clone(), "/v0/session/load", sample_project()).await;

        assert_eq!(response["ok"], true);
        assert_eq!(response["loaded"], true);
        assert_eq!(response["graphId"], "minimal-value");
        assert_eq!(response["graphRevision"], "1");
        assert_eq!(response["sessionRevision"], 1);
        assert_eq!(response["plan"]["graphId"], "minimal-value");

        let snapshot = get_json_with(app, "/v0/session").await;
        assert_eq!(snapshot["loaded"], true);
        assert_eq!(snapshot["plan"]["nodes"][0]["nodeId"], "value_1");
    }

    #[tokio::test]
    async fn invalid_session_load_does_not_replace_existing_session() {
        let app = runtime_router();
        let loaded = post_json_with(app.clone(), "/v0/session/load", sample_project()).await;
        let mut invalid = sample_project();
        invalid["nodes"] = json!([]);

        let response = post_json_with(app.clone(), "/v0/session/load", invalid).await;

        assert_eq!(loaded["sessionRevision"], 1);
        assert_eq!(response["ok"], false);
        assert_eq!(response["loaded"], true);
        assert_eq!(response["graphId"], "minimal-value");
        assert_eq!(response["sessionRevision"], 1);
        assert!(
            response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("missing node definition")
        );

        let snapshot = get_json_with(app, "/v0/session").await;
        assert_eq!(snapshot["ok"], true);
        assert_eq!(snapshot["loaded"], true);
        assert_eq!(snapshot["graphId"], "minimal-value");
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
        assert_eq!(plan["plan"]["graphId"], "minimal-value");

        let run = post_json_with(app, "/v0/session/run", json!({ "frames": 2 })).await;
        assert_eq!(run["ok"], true);
        assert_eq!(run["report"]["frameCount"], 2);
        assert_eq!(
            run["report"]["frames"][0]["executedNodes"][0]["status"],
            "simulated"
        );
    }

    #[tokio::test]
    async fn session_patch_endpoint_applies_and_rejects_conflicts() {
        let app = runtime_router();
        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;

        let patched = post_json_with(app.clone(), "/v0/session/patch", set_value_patch("1")).await;
        assert_eq!(patched["ok"], true);
        assert_eq!(patched["applied"], true);
        assert_eq!(patched["conflict"], false);
        assert_eq!(patched["graph"]["revision"], "2");
        assert_eq!(patched["session"]["graphRevision"], "2");
        assert_eq!(patched["session"]["sessionRevision"], 2);
        assert_eq!(patched["session"]["plan"]["graphRevision"], "2");

        let conflict = post_json_with(app, "/v0/session/patch", set_value_patch("1")).await;
        assert_eq!(conflict["ok"], false);
        assert_eq!(conflict["applied"], false);
        assert_eq!(conflict["conflict"], true);
        assert_eq!(conflict["graph"]["revision"], "2");
        assert!(
            conflict["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("does not match session graph revision")
        );
    }

    #[tokio::test]
    async fn session_patch_endpoint_reports_errors_without_loaded_session() {
        let response = post_json("/v0/session/patch", set_value_patch("1")).await;

        assert_eq!(response["ok"], false);
        assert_eq!(response["applied"], false);
        assert_eq!(response["conflict"], false);
        assert_eq!(response["graph"], Value::Null);
        assert!(
            response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("no project loaded")
        );
    }

    #[tokio::test]
    async fn session_patch_endpoint_reports_unsupported_operations() {
        let app = runtime_router();
        post_json_with(app.clone(), "/v0/session/load", sample_project()).await;

        let response = post_json_with(
            app,
            "/v0/session/patch",
            json!({
              "schema": "skenion.graph.patch",
              "schemaVersion": "0.1.0",
              "id": "unsupported",
              "baseRevision": "1",
              "ops": [
                { "op": "moveNode", "nodeId": "value_1" }
              ]
            }),
        )
        .await;

        assert_eq!(response["ok"], false);
        assert_eq!(response["applied"], false);
        assert_eq!(response["conflict"], false);
        assert_eq!(response["graph"]["revision"], "1");
        assert!(
            response["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains("invalid graph patch")
        );
    }

    #[tokio::test]
    async fn session_run_fails_without_loaded_project() {
        let response = post_json("/v0/session/run", json!({ "frames": 2 })).await;

        assert_eq!(response["ok"], false);
        assert_eq!(response["loaded"], false);
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
        assert_eq!(response["loaded"], false);
        assert_eq!(response["graphId"], Value::Null);
        assert_eq!(response["sessionRevision"], 2);
        assert_eq!(response["plan"], Value::Null);
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
                "kind": "core.value-f32",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": [
                  { "id": "value", "direction": "output", "type": { "flow": "value", "dataKind": "f32" } }
                ]
              },
              {
                "id": "target_1",
                "kind": "core.target",
                "kindVersion": "0.1.0",
                "params": {},
                "ports": [
                  { "id": "value", "direction": "input", "type": { "flow": "value", "dataKind": "f32" }, "activation": "latched" }
                ]
              }
            ],
            "edges": [
              { "from": { "node": "value_1", "port": "value" }, "to": { "node": "target_1", "port": "value" } }
            ]
          },
          "nodes": [
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "core.value-f32",
              "version": "0.1.0",
              "displayName": "Float Value",
              "category": "Values",
              "ports": [
                { "id": "value", "direction": "output", "type": { "flow": "value", "dataKind": "f32" } }
              ],
              "execution": { "model": "value" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": []
            },
            {
              "schema": "skenion.node.definition",
              "schemaVersion": "0.1.0",
              "id": "core.target",
              "version": "0.1.0",
              "displayName": "Target",
              "category": "Values",
              "ports": [
                { "id": "value", "direction": "input", "type": { "flow": "value", "dataKind": "f32" }, "activation": "latched" }
              ],
              "execution": { "model": "value" },
              "state": { "persistent": false },
              "permissions": [],
              "capabilities": []
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
}
