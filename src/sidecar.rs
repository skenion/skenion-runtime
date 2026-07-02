use serde::{Deserialize, Serialize};
use skenion_contracts::CONTRACTS_PACKAGE_VERSION;

use crate::{
    IssueSeverity, RUNTIME_API_VERSION, RUNTIME_SUPPORTED_CONTRACTS_RANGE,
    RuntimeConnectionProfile, RuntimeConnectionProfileMode, RuntimeEndpointMetadata,
    RuntimeEndpointProtocol, RuntimeIssue, RuntimeOwnershipMode, RuntimeProcessMetadata,
};

#[derive(Debug, Clone)]
pub struct RuntimeEndpointConfig {
    pub host: String,
    pub port: u16,
    pub url: String,
}

impl RuntimeEndpointConfig {
    pub fn new(host: String, port: u16) -> Self {
        let url = format!("http://{host}:{port}");
        Self { host, port, url }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSidecarStartupResponse {
    pub schema: &'static str,
    pub schema_version: &'static str,
    pub ok: bool,
    pub runtime: RuntimeSidecarRuntimeInfo,
    pub endpoint: RuntimeEndpointMetadata,
    pub profile: RuntimeConnectionProfile,
    pub default_session_id: String,
    pub default_session_url: String,
    pub health: RuntimeSidecarHealthInfo,
    pub token: RuntimeSidecarTokenInfo,
    pub shutdown: RuntimeSidecarShutdownInfo,
    pub issues: Vec<RuntimeIssue>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSidecarRuntimeInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub api_version: &'static str,
    pub contracts_built_against_version: &'static str,
    pub supported_contracts_range: &'static str,
}

impl RuntimeSidecarRuntimeInfo {
    fn current() -> Self {
        Self {
            name: "skenion-runtime",
            version: env!("CARGO_PKG_VERSION"),
            api_version: RUNTIME_API_VERSION,
            contracts_built_against_version: CONTRACTS_PACKAGE_VERSION,
            supported_contracts_range: RUNTIME_SUPPORTED_CONTRACTS_RANGE,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSidecarHealthInfo {
    pub ok: bool,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSidecarHealthResponse {
    pub schema: &'static str,
    pub schema_version: &'static str,
    pub ok: bool,
    pub readiness: &'static str,
    pub runtime: RuntimeSidecarRuntimeInfo,
    pub endpoint: RuntimeEndpointMetadata,
    pub profile: RuntimeConnectionProfile,
    pub issues: Vec<RuntimeIssue>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSidecarTokenInfo {
    pub required: bool,
    pub header: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSidecarShutdownInfo {
    pub supported: bool,
    pub method: &'static str,
    pub url: String,
    pub scope: &'static str,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSidecarShutdownRequest {
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub owner_window_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSidecarShutdownResponse {
    pub schema: &'static str,
    pub schema_version: &'static str,
    pub ok: bool,
    pub accepted: bool,
    pub action: &'static str,
    pub scope: &'static str,
    pub issues: Vec<RuntimeIssue>,
}

pub(crate) fn sidecar_startup_response(
    endpoint_config: &RuntimeEndpointConfig,
    default_session_id: &str,
    started_at_wall_clock: &str,
) -> RuntimeSidecarStartupResponse {
    let endpoint = runtime_endpoint_metadata(endpoint_config);
    RuntimeSidecarStartupResponse {
        schema: "skenion.runtime.sidecar.startup",
        schema_version: "0.1.0",
        ok: true,
        runtime: RuntimeSidecarRuntimeInfo::current(),
        endpoint: endpoint.clone(),
        profile: runtime_connection_profile(endpoint_config, started_at_wall_clock),
        default_session_id: default_session_id.to_owned(),
        default_session_url: format!("{}/v0/sessions/{default_session_id}", endpoint.url),
        health: RuntimeSidecarHealthInfo {
            ok: true,
            url: format!("{}/v0/sidecar/health", endpoint.url),
        },
        token: runtime_sidecar_token(),
        shutdown: RuntimeSidecarShutdownInfo {
            supported: true,
            method: "POST",
            url: format!("{}/v0/sidecar/shutdown", endpoint.url),
            scope: "owned-child-only",
        },
        issues: Vec::new(),
    }
}

pub(crate) fn sidecar_health_response(
    endpoint_config: &RuntimeEndpointConfig,
    started_at_wall_clock: &str,
) -> RuntimeSidecarHealthResponse {
    RuntimeSidecarHealthResponse {
        schema: "skenion.runtime.sidecar.health",
        schema_version: "0.1.0",
        ok: true,
        readiness: "ready",
        runtime: RuntimeSidecarRuntimeInfo::current(),
        endpoint: runtime_endpoint_metadata(endpoint_config),
        profile: runtime_connection_profile(endpoint_config, started_at_wall_clock),
        issues: Vec::new(),
    }
}

pub(crate) fn runtime_connection_profile(
    endpoint_config: &RuntimeEndpointConfig,
    started_at_wall_clock: &str,
) -> RuntimeConnectionProfile {
    RuntimeConnectionProfile {
        mode: RuntimeConnectionProfileMode::LocalManaged,
        ownership: RuntimeOwnershipMode::OwnedChild,
        display_name: Some("skenion runtime local sidecar".to_owned()),
        endpoint: runtime_endpoint_metadata(endpoint_config),
        process: Some(RuntimeProcessMetadata {
            owned_by_host: true,
            pid: Some(std::process::id()),
            executable_path: std::env::current_exe()
                .ok()
                .map(|path| path.display().to_string()),
            working_directory: std::env::current_dir()
                .ok()
                .map(|path| path.display().to_string()),
            started_at: Some(started_at_wall_clock.to_owned()),
            owner_window_id: None,
            platform: Some(std::env::consts::OS.to_owned()),
            arch: Some(std::env::consts::ARCH.to_owned()),
        }),
    }
}

fn runtime_endpoint_metadata(endpoint_config: &RuntimeEndpointConfig) -> RuntimeEndpointMetadata {
    RuntimeEndpointMetadata {
        url: endpoint_config.url.clone(),
        canonical_url: Some(endpoint_config.url.clone()),
        protocol: RuntimeEndpointProtocol::Http,
        host: Some(endpoint_config.host.clone()),
        port: Some(endpoint_config.port),
        tls: Some(false),
    }
}

fn runtime_sidecar_token() -> RuntimeSidecarTokenInfo {
    runtime_sidecar_token_from_value(std::env::var("SKENION_RUNTIME_TOKEN").ok())
}

fn runtime_sidecar_token_from_value(token: Option<String>) -> RuntimeSidecarTokenInfo {
    let token = match token {
        Some(token) if !token.is_empty() => Some(token),
        _ => None,
    };
    RuntimeSidecarTokenInfo {
        required: token.is_some(),
        header: "Authorization",
        token,
    }
}

pub(crate) fn sidecar_shutdown_response(body: &[u8]) -> RuntimeSidecarShutdownResponse {
    let issues = match sidecar_shutdown_request(body) {
        Ok(request) => shutdown_request_issues(request),
        Err(error) => vec![RuntimeIssue::error(format!(
            "invalid sidecar shutdown request: {error}"
        ))],
    };
    RuntimeSidecarShutdownResponse {
        schema: "skenion.runtime.sidecar.shutdown",
        schema_version: "0.1.0",
        ok: issues
            .iter()
            .all(|issue| issue.severity != IssueSeverity::Error),
        accepted: false,
        action: "host-owned-process-stop-required",
        scope: "owned-child-only",
        issues,
    }
}

fn sidecar_shutdown_request(
    body: &[u8],
) -> Result<RuntimeSidecarShutdownRequest, serde_json::Error> {
    if body.is_empty() {
        return Ok(RuntimeSidecarShutdownRequest {
            reason: None,
            owner_window_id: None,
        });
    }
    serde_json::from_slice(body)
}

fn shutdown_request_issues(request: RuntimeSidecarShutdownRequest) -> Vec<RuntimeIssue> {
    let mut issues = vec![RuntimeIssue::warning(
        "runtime shutdown is a structured primitive; the host must stop only its owned child process",
    )];
    if let Some(reason) = request.reason.filter(|reason| !reason.is_empty()) {
        issues.push(RuntimeIssue {
            severity: IssueSeverity::Info,
            message: format!("shutdown requested: {reason}"),
            code: Some("sidecar.shutdown.reason".to_owned()),
            details: request
                .owner_window_id
                .map(|owner_window_id| serde_json::json!({ "ownerWindowId": owner_window_id })),
        });
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_response_is_machine_readable() {
        let endpoint = RuntimeEndpointConfig::new("127.0.0.1".to_owned(), 3761);
        let response = sidecar_startup_response(&endpoint, "default", "unix-ms:1");

        assert!(response.ok);
        assert_eq!(response.endpoint.url, "http://127.0.0.1:3761");
        assert_eq!(response.default_session_id, "default");
        assert_eq!(
            response.profile.mode,
            RuntimeConnectionProfileMode::LocalManaged
        );
        assert_eq!(response.token.header, "Authorization");
        assert_eq!(response.shutdown.scope, "owned-child-only");
    }

    #[test]
    fn health_response_is_not_startup_handshake() {
        let endpoint = RuntimeEndpointConfig::new("127.0.0.1".to_owned(), 3761);
        let response = sidecar_health_response(&endpoint, "unix-ms:1");
        let value = serde_json::to_value(&response).expect("health response should serialize");

        assert!(response.ok);
        assert_eq!(response.schema, "skenion.runtime.sidecar.health");
        assert_eq!(response.readiness, "ready");
        assert_eq!(response.runtime.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(response.endpoint.url, "http://127.0.0.1:3761");
        assert!(value.get("token").is_none());
        assert!(value.get("shutdown").is_none());
        assert!(value.get("defaultSessionUrl").is_none());
    }

    #[test]
    fn token_info_reports_present_token_without_environment_mutation() {
        let present = runtime_sidecar_token_from_value(Some("test-token".to_owned()));
        let empty = runtime_sidecar_token_from_value(Some(String::new()));

        assert!(present.required);
        assert_eq!(present.token.as_deref(), Some("test-token"));
        assert!(!empty.required);
        assert!(empty.token.is_none());
    }

    #[test]
    fn shutdown_response_is_structured_without_killing_processes() {
        let empty = sidecar_shutdown_response(&[]);
        let reason = sidecar_shutdown_response(
            br#"{ "reason": "window-close", "ownerWindowId": "window-1" }"#,
        );
        let invalid = sidecar_shutdown_response(b"{");

        assert!(empty.ok);
        assert!(!empty.accepted);
        assert!(reason.ok);
        assert_eq!(reason.issues.len(), 2);
        assert!(!invalid.ok);
    }
}
