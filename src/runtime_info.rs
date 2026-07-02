use serde::{Deserialize, Serialize};
use skenion_contracts::CONTRACTS_PACKAGE_VERSION;

pub const RUNTIME_API_VERSION: &str = "0.1.0";
pub const RUNTIME_SUPPORTED_CONTRACTS_RANGE: &str =
    env!("SKENION_RUNTIME_SUPPORTED_CONTRACTS_RANGE");

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub ok: bool,
    pub service: &'static str,
    pub version: &'static str,
    pub api_version: &'static str,
    pub contracts_built_against_version: &'static str,
    pub supported_contracts_range: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfoResponse {
    pub name: &'static str,
    pub version: &'static str,
    pub api_version: &'static str,
    pub contracts_built_against_version: &'static str,
    pub supported_contracts_range: &'static str,
    pub capabilities: Vec<&'static str>,
}

pub(crate) fn health_response() -> HealthResponse {
    HealthResponse {
        ok: true,
        service: "skenion-runtime",
        version: env!("CARGO_PKG_VERSION"),
        api_version: RUNTIME_API_VERSION,
        contracts_built_against_version: CONTRACTS_PACKAGE_VERSION,
        supported_contracts_range: RUNTIME_SUPPORTED_CONTRACTS_RANGE,
    }
}

pub(crate) fn runtime_info_response() -> RuntimeInfoResponse {
    RuntimeInfoResponse {
        name: "skenion-runtime",
        version: env!("CARGO_PKG_VERSION"),
        api_version: RUNTIME_API_VERSION,
        contracts_built_against_version: CONTRACTS_PACKAGE_VERSION,
        supported_contracts_range: RUNTIME_SUPPORTED_CONTRACTS_RANGE,
        capabilities: runtime_capabilities(),
    }
}

fn runtime_capabilities() -> Vec<&'static str> {
    vec![
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
        "runtime.sidecar.local",
        "runtime.sidecar.startup",
        "runtime.sidecar.health",
        "runtime.sidecar.shutdown",
        "io.devices",
    ]
}
