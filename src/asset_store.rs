use std::{
    collections::BTreeMap,
    fs,
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{Arc, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::body::Bytes;
use serde::Serialize;

use crate::RuntimeIssue;

pub(crate) type SharedRuntimeAssetStore = Arc<RwLock<RuntimeAssetStore>>;

#[derive(Debug, Clone, Default)]
pub struct RuntimeAssetStore {
    assets: BTreeMap<String, RuntimeAsset>,
}

impl RuntimeAssetStore {
    pub(crate) fn shared() -> SharedRuntimeAssetStore {
        Arc::new(RwLock::new(Self::default()))
    }

    pub(crate) fn list(&self) -> Vec<RuntimeAsset> {
        self.assets.values().cloned().collect()
    }

    pub(crate) fn get(&self, asset_id: &str) -> Option<RuntimeAsset> {
        self.assets.get(asset_id).cloned()
    }

    fn insert(&mut self, asset: RuntimeAsset) {
        self.assets.insert(asset.id.clone(), asset);
    }
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
    pub issues: Vec<RuntimeIssue>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAssetListResponse {
    pub ok: bool,
    pub assets: Vec<RuntimeAsset>,
    pub issues: Vec<RuntimeIssue>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAssetGetResponse {
    pub ok: bool,
    pub asset: Option<RuntimeAsset>,
    pub issues: Vec<RuntimeIssue>,
}

pub(crate) fn store_asset(
    store: &SharedRuntimeAssetStore,
    name: String,
    mime_type: String,
    bytes: Bytes,
) -> RuntimeAssetImportResponse {
    let id = asset_id(&name, &mime_type, &bytes);
    store_asset_with_id(store, id, name, mime_type, bytes, runtime_asset_directory())
}

pub(crate) fn store_asset_with_id(
    store: &SharedRuntimeAssetStore,
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
            issues: vec![RuntimeIssue::error(format!(
                "failed to create runtime asset directory: {error}"
            ))],
        };
    }
    let path = directory.join(&id);
    if let Err(error) = fs::write(&path, &bytes) {
        return RuntimeAssetImportResponse {
            ok: false,
            asset: None,
            issues: vec![RuntimeIssue::error(format!(
                "failed to store runtime asset: {error}"
            ))],
        };
    }
    let asset = RuntimeAsset {
        id,
        name,
        mime_type,
        kind,
        size_bytes: bytes.len().try_into().unwrap_or(u64::MAX),
        runtime_uri,
    };
    store
        .write()
        .expect("runtime asset store lock should not be poisoned")
        .insert(asset.clone());
    RuntimeAssetImportResponse {
        ok: true,
        asset: Some(asset),
        issues: Vec::new(),
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

pub(crate) fn asset_kind(mime_type: &str) -> String {
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
