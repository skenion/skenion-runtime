use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
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
use futures_util::StreamExt;
use serde_json::{Value, json};
use skenion_contracts::CONTRACTS_PACKAGE_VERSION;
use tower::ServiceExt;

use crate::{
    IssueSeverity, RUNTIME_API_VERSION, RUNTIME_SUPPORTED_CONTRACTS_RANGE, RuntimeEndpointConfig,
    RuntimeExtensionManager, RuntimeExtensionRegistrySnapshot, RuntimeIoDeviceDescriptor,
    RuntimeIoDeviceListResponse, RuntimeIoDeviceManager, RuntimeLogStore, RuntimePackageManager,
    RuntimePackageRegistrySnapshot, RuntimeSessionRegistry,
    asset_store::{RuntimeAssetStore, asset_kind, store_asset_with_id},
    io_device_manager::RuntimeIoDeviceRegistry,
    runtime_time::created_at_now,
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
            issues: Vec::new(),
        }
    }
}

fn current_core_node_json(id: &str, object_id: &str, params: Value, ports: Value) -> Value {
    json!({
      "id": id,
      "implementation": {
        "provider": { "kind": "core" },
        "objectId": object_id
      },
      "objectSpec": object_id,
      "objectResolution": {
        "status": "resolved",
        "candidates": [],
        "issues": []
      },
      "params": params,
      "ports": ports
    })
}

fn normalize_current_fixture_value(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                normalize_current_fixture_value(item);
            }
        }
        Value::Object(object) => {
            if object.contains_key("kind")
                && object.contains_key("kindVersion")
                && object.contains_key("ports")
            {
                let id = object
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("node")
                    .to_owned();
                let kind = object
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let params = object.remove("params").unwrap_or_else(|| json!({}));
                let ports = object.remove("ports").unwrap_or_else(|| json!([]));
                if let Some(object_id) = kind.strip_prefix("object.core.") {
                    *value = current_core_node_json(&id, object_id, params, ports);
                }
                return;
            }
            for child in object.values_mut() {
                normalize_current_fixture_value(child);
            }
        }
        _ => {}
    }
}

fn current_fixture_value(mut value: Value) -> Value {
    normalize_current_fixture_value(&mut value);
    value
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
        response["supportedContractsRange"],
        RUNTIME_SUPPORTED_CONTRACTS_RANGE
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
        response["supportedContractsRange"],
        RUNTIME_SUPPORTED_CONTRACTS_RANGE
    );
    let capabilities = response["capabilities"].as_array().unwrap();
    for expected in [
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
        "runtime.sidecar.local",
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
        "project.validate",
        "project.validate.v0.1",
        "project.plan",
        "project.plan.v0.1",
        "dummy.run",
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
        "session.validate",
        "session.plan",
        "session.run",
        "runtime.profile.localManaged",
        "runtime.profile.localShared",
        "runtime.profile.remote",
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
            "selection.update",
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
            "node.input",
        ),
    ] {
        let (status, body) =
            request_json_status_with(app.clone(), method.clone(), path, json!({})).await;
        assert_eq!(status, StatusCode::GONE, "{path}");
        assert_eq!(body["ok"], false, "{path}");
        assert_eq!(body["schema"], "skenion.runtime.http-live-channel-disabled");
        assert_eq!(
            body["issues"][0]["code"],
            "runtime.http-live-channel-disabled"
        );
        assert_eq!(
            body["issues"][0]["details"]["websocketEndpoint"],
            "/v0/sessions/default"
        );
        assert_eq!(
            body["issues"][0]["details"]["replacement"]["type"], replacement_type,
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
        startup["runtime"]["supportedContractsRange"],
        RUNTIME_SUPPORTED_CONTRACTS_RANGE
    );
    assert_eq!(startup["endpoint"]["protocol"], "http");
    assert_eq!(startup["profile"]["mode"], "local-managed");
    assert_eq!(startup["profile"]["ownership"], "owned-child");
    assert_eq!(
        startup["profile"]["displayName"],
        "skenion runtime local sidecar"
    );
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
        health["runtime"]["supportedContractsRange"],
        RUNTIME_SUPPORTED_CONTRACTS_RANGE
    );
    assert_eq!(health["endpoint"]["protocol"], "http");
    assert_eq!(health["profile"]["mode"], "local-managed");
    assert_eq!(
        health["profile"]["displayName"],
        "skenion runtime local sidecar"
    );
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
    assert_eq!(response["issues"], json!([]));
}

#[tokio::test]
async fn successful_extension_startup_keeps_runtime_logs_empty() {
    let package_dir = server_temp_extension_dir("success-package");
    write_server_valid_extension_manifest(&package_dir);
    let app = runtime_router_with_extension_package_dirs(vec![package_dir]);

    let extensions = get_json_with(app.clone(), "/v0/extensions").await;
    assert_eq!(extensions["ok"], true);
    assert_eq!(extensions["issues"], json!([]));
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
    assert_eq!(
        first_packages["packages"][0]["contracts"]["version"],
        CONTRACTS_PACKAGE_VERSION
    );
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
async fn runtime_packages_redact_absolute_package_paths_without_log_projection() {
    let package_dir = server_temp_package_dir("redacted-extension-only");
    write_server_valid_extension_manifest(&package_dir);
    let (app, _) = runtime_router_with_package_dirs(vec![package_dir.clone()]);

    let packages = get_json_with(app.clone(), "/v0/packages").await;
    assert_eq!(packages["ok"], false);
    assert_eq!(packages["issues"][0]["code"], "package.root.extension-only");
    assert!(
        !packages
            .to_string()
            .contains(&package_dir.display().to_string())
    );

    let logs = get_json_with(app, "/v0/runtime/logs").await;
    assert_eq!(logs["events"], json!([]));
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
async fn startup_extension_scan_reports_package_issues_without_log_projection() {
    let missing_manifest_dir = server_temp_extension_dir("startup-missing-manifest");
    let malformed_manifest_dir = server_temp_extension_dir("startup-malformed-manifest");
    write_server_extension_manifest(&malformed_manifest_dir, "{ not-json");
    let app = runtime_router_with_extension_package_dirs(vec![
        missing_manifest_dir.clone(),
        malformed_manifest_dir.clone(),
    ]);

    let startup_logs = get_json_with(app.clone(), "/v0/runtime/logs").await;
    assert_eq!(startup_logs["events"], json!([]));

    let first_extensions = get_json_with(app.clone(), "/v0/extensions").await;
    let second_extensions = get_json_with(app.clone(), "/v0/extensions").await;
    assert_eq!(first_extensions, second_extensions);
    assert_eq!(first_extensions["ok"], false);
    assert_eq!(
        first_extensions["issues"][0]["code"],
        "extension.manifest.missing"
    );
    assert_eq!(
        first_extensions["extensions"][0]["issues"][0]["code"],
        "extension.manifest.parse-failed"
    );

    let logs_after_polling = get_json_with(app, "/v0/runtime/logs").await;
    assert_eq!(logs_after_polling["events"], json!([]));
}

#[tokio::test]
async fn runtime_extension_issues_preserve_package_context_without_log_backlog() {
    let missing_manifest_dir = server_temp_extension_dir("stream-missing-manifest");
    let app = runtime_router_with_extension_package_dirs(vec![missing_manifest_dir.clone()]);

    let extensions = get_json_with(app.clone(), "/v0/extensions").await;
    assert_eq!(
        extensions["issues"][0]["code"],
        "extension.manifest.missing"
    );
    assert_eq!(
        extensions["issues"][0]["details"]["packagePath"],
        missing_manifest_dir.display().to_string()
    );

    let logs = get_json_with(app, "/v0/runtime/logs").await;
    assert_eq!(logs["events"], json!([]));
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

    state.logs.record_event(
        IssueSeverity::Error,
        Some("runtime.test-no-undo".to_owned()),
        "no patch event available to undo".to_owned(),
        Some(json!({ "source": "test" })),
    );

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
    state.logs.record_event(
        IssueSeverity::Error,
        Some("runtime.test-no-undo".to_owned()),
        "no patch event available to undo".to_owned(),
        Some(json!({ "source": "test" })),
    );

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
        missing["issues"][0]["message"]
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
        missing_file["issues"][0]["message"]
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
        &state.assets,
        "asset_create_error".to_owned(),
        "clip.mov".to_owned(),
        "video/quicktime".to_owned(),
        Bytes::from_static(b"asset"),
        base.clone(),
    );
    assert!(!create_error.ok);
    assert!(
        create_error.issues[0]
            .message
            .contains("failed to create runtime asset directory")
    );

    std::fs::remove_file(&base).expect("blocker should remove");
    std::fs::create_dir_all(&base).expect("base directory should create");
    std::fs::create_dir(base.join("asset_write_error")).expect("asset blocker should create");

    let write_error = store_asset_with_id(
        &state.assets,
        "asset_write_error".to_owned(),
        "clip.mov".to_owned(),
        "video/quicktime".to_owned(),
        Bytes::from_static(b"asset"),
        base.clone(),
    );
    assert!(!write_error.ok);
    assert!(
        write_error.issues[0]
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
async fn project_operation_routes_are_not_runtime_http_surface() {
    let app = runtime_router();

    for path in ["/v0/validate", "/v0/plan", "/v0/run"] {
        let status = post_status_with(app.clone(), path, json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
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
        response["issues"][0]["code"],
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
    assert_eq!(response["issues"].as_array().unwrap().len(), 0);
    assert_eq!(response["snapshot"]["plan"], Value::Null);
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
async fn session_load_drops_obsolete_object_implementation_version_fields() {
    let app = runtime_router();
    let mut project = sample_subpatch_project_document_current();
    project["graph"]["nodes"][0]["implementation"]["version"] = json!("0.1.0");
    project["graph"]["nodes"][1]["implementation"]["version"] = json!("0.1.0");
    project["patchLibrary"][0]["graph"]["nodes"][1]["implementation"]["version"] = json!("0.1.0");

    let response = post_json_with(
        app.clone(),
        "/v0/sessions/default/load",
        session_load_request(project),
    )
    .await;

    assert_eq!(response["ok"], true);
    let obsolete_paths = response["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|issue| issue["code"] == "project.load.obsolete-field-dropped")
        .map(|issue| issue["details"]["path"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        obsolete_paths,
        vec![
            "$.graph.nodes[0].implementation.version",
            "$.graph.nodes[1].implementation.version",
            "$.patchLibrary[0].graph.nodes[1].implementation.version",
        ]
    );
    assert_eq!(
        response["snapshot"]["project"]["graph"]["nodes"][0]["implementation"]["version"],
        Value::Null
    );
    assert_eq!(
        response["snapshot"]["project"]["graph"]["nodes"][1]["implementation"]["version"],
        Value::Null
    );
    assert_eq!(
        response["snapshot"]["project"]["patchLibrary"][0]["graph"]["nodes"][1]["implementation"]["version"],
        Value::Null
    );

    let snapshot = get_json_with(app.clone(), "/v0/sessions/default/snapshot").await;
    assert_eq!(
        snapshot["snapshot"]["project"]["graph"]["nodes"][0]["implementation"]["version"],
        Value::Null
    );
    assert_eq!(
        snapshot["snapshot"]["project"]["patchLibrary"][0]["graph"]["nodes"][1]["implementation"]["version"],
        Value::Null
    );

    let logs = get_json_with(app, "/v0/runtime/logs").await;
    assert_eq!(logs["events"].as_array().unwrap().len(), 1);
    assert_eq!(logs["events"][0]["code"], "project.load.repaired");
    assert_eq!(logs["events"][0]["level"], "warning");
    assert_eq!(
        logs["events"][0]["details"]["obsoleteImplementationVersionPaths"],
        json!(obsolete_paths)
    );
}

#[tokio::test]
async fn session_load_drops_unknown_project_fields_before_canonical_snapshot() {
    let app = runtime_router();
    let mut request = session_load_request(sample_project_document_current());
    request["ignoredLoadField"] = json!("drop");
    request["project"]["ignoredProjectField"] = json!(true);
    request["project"]["graph"]["ignoredGraphField"] = json!(true);
    request["project"]["graph"]["nodes"][0]["ignoredNodeField"] = json!(true);
    request["project"]["graph"]["nodes"][0]["ports"][0]["ignoredPortField"] = json!(true);
    request["project"]["graph"]["edges"][0]["ignoredEdgeField"] = json!(true);
    request["project"]["graph"]["edges"][0]["source"]["ignoredEndpointField"] = json!(true);
    request["project"]["viewState"]["canvas"]["ignoredCanvasField"] = json!(true);
    request["project"]["viewState"]["canvas"]["nodes"]["value_1"]["ignoredViewField"] = json!(true);
    if request["project"]["graph"]["nodes"][0]
        .get("implementation")
        .is_some()
    {
        request["project"]["graph"]["nodes"][0]["implementation"]["ignoredImplementationField"] =
            json!(true);
        request["project"]["graph"]["nodes"][0]["implementation"]["provider"]["ignoredProviderField"] =
            json!(true);
    }

    let response = post_json_with(app.clone(), "/v0/sessions/default/load", request).await;

    assert_eq!(response["ok"], true, "{response}");
    let project = &response["snapshot"]["project"];
    assert!(project.get("ignoredProjectField").is_none());
    assert!(project["graph"].get("ignoredGraphField").is_none());
    assert!(
        project["graph"]["nodes"][0]
            .get("ignoredNodeField")
            .is_none()
    );
    assert!(
        project["graph"]["nodes"][0]["ports"][0]
            .get("ignoredPortField")
            .is_none()
    );
    assert!(
        project["graph"]["edges"][0]
            .get("ignoredEdgeField")
            .is_none()
    );
    assert!(
        project["graph"]["edges"][0]["source"]
            .get("ignoredEndpointField")
            .is_none()
    );
    assert!(
        project["viewState"]["canvas"]
            .get("ignoredCanvasField")
            .is_none()
    );
    assert!(
        project["viewState"]["canvas"]["nodes"]["value_1"]
            .get("ignoredViewField")
            .is_none()
    );

    let logs = get_json_with(app, "/v0/runtime/logs").await;
    assert_eq!(logs["events"], json!([]));
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
    assert_eq!(second["issues"][0]["code"], "runtime.session-load.conflict");
    assert_eq!(
        second["issues"][0]["details"]["current"]["documentId"],
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
        rejected["issues"][0]["code"],
        "runtime.session-load.conflict"
    );
    assert_eq!(
        rejected["issues"][0]["details"]["mismatches"][0]["field"],
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
        response["issues"][0]["code"],
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
        response["issues"][0]["code"],
        "project.document.top-level-nodes-rejected"
    );

    let snapshot = get_json_with(app, "/v0/sessions/default/snapshot").await;
    assert_eq!(snapshot["ok"], true);
    assert_eq!(
        snapshot["snapshot"]["project"]["graph"]["id"],
        "minimal-value"
    );
    assert_eq!(snapshot["snapshot"]["issues"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn invalid_session_load_returns_issues_and_keeps_runtime_healthy() {
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
        response["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == "node.port-snapshot.type-mismatch")
    );

    let health = get_json_with(app.clone(), "/health").await;
    assert_eq!(health["ok"], true);

    let snapshot = get_json_with(app.clone(), "/v0/sessions/default/snapshot").await;
    assert_eq!(snapshot["ok"], true);
    assert_eq!(snapshot["snapshot"]["project"], Value::Null);
    assert_eq!(snapshot["snapshot"]["sessionRevision"], 0);

    let logs = get_json_with(app, "/v0/runtime/logs").await;
    assert_eq!(logs["events"], json!([]));
}

#[tokio::test]
async fn repairable_session_load_drops_invalid_edges_and_records_warning_log() {
    let app = runtime_router();
    let mut project = sample_project_document_current();
    project["graph"]["edges"][0]["target"]["portId"] = json!("value");

    let response = post_json_with(
        app.clone(),
        "/v0/sessions/default/load",
        session_load_request(project),
    )
    .await;

    assert_eq!(response["ok"], true);
    assert_eq!(response["snapshot"]["project"]["graph"]["revision"], "2");
    assert_eq!(response["snapshot"]["project"]["graph"]["edges"], json!([]));
    assert!(response["issues"].as_array().unwrap().iter().any(|issue| {
        issue["code"] == "project.load.edge-dropped" && issue["severity"] == "warning"
    }));

    let logs = get_json_with(app, "/v0/runtime/logs").await;
    assert_eq!(logs["events"].as_array().unwrap().len(), 1);
    assert_eq!(logs["events"][0]["code"], "project.load.repaired");
    assert_eq!(logs["events"][0]["level"], "warning");
    assert_eq!(
        logs["events"][0]["details"]["droppedEdgeIds"],
        json!(["edge_value_target"])
    );
}

#[tokio::test]
async fn session_project_operation_routes_are_not_runtime_http_surface() {
    let app = runtime_router();

    for path in [
        "/v0/sessions/default/validate",
        "/v0/sessions/default/plan",
        "/v0/sessions/default/run",
    ] {
        let status = post_status_with(app.clone(), path, json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
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
        response["issues"][0]["message"]
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

    let response = post_raw_with(app, "/v0/sessions/default/preview/start", b"{".to_vec()).await;

    assert_eq!(response["ok"], false);
    assert_eq!(response["state"], "stopped");
    assert!(
        response["issues"][0]["message"]
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
    assert_eq!(response["render"]["issues"], json!([]));
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
    assert_eq!(response["render"]["issues"], json!([]));
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
    assert_eq!(response["render"]["issues"], json!([]));
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
    assert_eq!(response["issues"], json!([]));
}

#[tokio::test]
async fn generated_shader_endpoint_reports_session_or_shader_issues() {
    let empty = get_json_with(
        runtime_router_with_dry_preview(),
        "/v0/sessions/default/render/generated-shader",
    )
    .await;
    assert_eq!(empty["ok"], false);
    assert_eq!(empty["issues"][0]["phase"], json!("source-sync"));

    let mut project = sample_shader_project_current();
    project["graph"]["nodes"][0]["params"]["source"] = json!(
        "// @skenion.uniform bad vec3\n@fragment\nfn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }"
    );
    let app = runtime_router_with_loaded_shader_dry_preview(project);

    let response = get_json_with(app, "/v0/sessions/default/render/generated-shader").await;
    assert_eq!(response["ok"], false);
    assert_eq!(response["issues"][0]["phase"], json!("interface-analysis"));
    assert_eq!(
        response["issues"][0]["code"],
        json!("unsupported-uniform-type")
    );
    assert_eq!(response["issues"][0]["line"], json!(1));
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
            response.issues
        );
    }
    runtime_router_with_state(state)
}

fn runtime_state_with_dry_preview() -> RuntimeServerState {
    let logs = std::sync::Arc::new(RuntimeLogStore::default());
    RuntimeServerState {
        sessions: RuntimeSessionRegistry::dry_preview(),
        assets: RuntimeAssetStore::shared(),
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
        assets: RuntimeAssetStore::shared(),
        io_devices: std::sync::Arc::new(RuntimeIoDeviceManager::with_device_registry(Arc::new(
            ServerFakeIoDeviceRegistry { devices },
        ))),
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
    let extension_scan = RuntimeExtensionManager::with_package_dirs(package_dirs).scan_registry();
    runtime_router_with_state(RuntimeServerState {
        sessions: RuntimeSessionRegistry::dry_preview(),
        assets: RuntimeAssetStore::shared(),
        io_devices: Arc::new(RuntimeIoDeviceManager::new()),
        extensions: Arc::new(extension_scan.into_snapshot()),
        packages: Arc::new(RuntimePackageRegistrySnapshot::default()),
        logs,
        endpoint: RuntimeEndpointConfig::new(DEFAULT_HOST.to_owned(), DEFAULT_PORT),
        started_at_wall_clock: created_at_now(),
        started_at: std::time::Instant::now(),
    })
}

fn runtime_router_with_package_dirs(package_dirs: Vec<PathBuf>) -> (Router, RuntimeServerState) {
    let logs = Arc::new(RuntimeLogStore::default());
    let package_scan = RuntimePackageManager::with_package_dirs(package_dirs).scan_registry();
    let state = RuntimeServerState {
        sessions: RuntimeSessionRegistry::dry_preview(),
        assets: RuntimeAssetStore::shared(),
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
    let contracts_version = CONTRACTS_PACKAGE_VERSION;
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
                "version": "{contracts_version}"
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
    current_fixture_value(json!({
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
    }))
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
    current_fixture_value(json!({
      "schema": "skenion.project",
      "schemaVersion": "0.1.0",
      "id": "shader-issues-project",
      "documentId": "10000000-0000-0000-0000-000000000003",
      "revision": "1",
      "graph": {
        "schema": "skenion.graph",
        "schemaVersion": "0.1.0",
        "id": "shader-issues",
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
    }))
}

fn sample_subpatch_project_document_current() -> Value {
    current_fixture_value(json!({
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
    }))
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
