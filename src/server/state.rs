use std::{sync::Arc, time::Instant};

use crate::{
    RuntimeExtensionManager, RuntimeExtensionRegistrySnapshot, RuntimeIoDeviceManager,
    RuntimeLogStore, RuntimePackageManager, RuntimePackageRegistrySnapshot,
    asset_store::{RuntimeAssetStore, SharedRuntimeAssetStore},
    runtime_time::created_at_now,
    session_registry::RuntimeSessionRegistry,
    sidecar::{
        RuntimeEndpointConfig, RuntimeSidecarHealthResponse, RuntimeSidecarStartupResponse,
        sidecar_health_response, sidecar_startup_response,
    },
};

use super::{DEFAULT_HOST, DEFAULT_PORT};

#[derive(Clone)]
pub struct RuntimeServerState {
    pub sessions: RuntimeSessionRegistry,
    pub assets: SharedRuntimeAssetStore,
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
        Self {
            sessions: RuntimeSessionRegistry::default(),
            assets: RuntimeAssetStore::shared(),
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
