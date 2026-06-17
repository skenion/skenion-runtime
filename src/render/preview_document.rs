use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use serde::{Deserialize, Serialize};

use crate::{ControlState, ExecutionPlan, GraphDocument};

pub const PREVIEW_DOCUMENT_SCHEMA: &str = "skenion.preview.document";
pub const PREVIEW_DOCUMENT_SCHEMA_VERSION: &str = "0.1.0";

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewDocument {
    pub schema: String,
    pub schema_version: String,
    pub graph: GraphDocument,
    pub plan: ExecutionPlan,
    pub control_state: ControlState,
    pub session_revision: u64,
}

impl PreviewDocument {
    pub fn new(graph: GraphDocument, plan: ExecutionPlan, session_revision: u64) -> Self {
        let control_state = ControlState::from_graph(&graph);
        Self::with_control_state(graph, plan, control_state, session_revision)
    }

    pub fn with_control_state(
        graph: GraphDocument,
        plan: ExecutionPlan,
        control_state: ControlState,
        session_revision: u64,
    ) -> Self {
        Self {
            schema: PREVIEW_DOCUMENT_SCHEMA.to_owned(),
            schema_version: PREVIEW_DOCUMENT_SCHEMA_VERSION.to_owned(),
            graph,
            plan,
            control_state,
            session_revision,
        }
    }
}

pub fn write_preview_document(document: &PreviewDocument) -> Result<PathBuf, String> {
    let directory = preview_temp_dir();
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = directory.join(format!(
        "preview-document-{}-{}.json",
        std::process::id(),
        document.session_revision
    ));
    let bytes = serde_json::to_vec_pretty(document).map_err(|error| error.to_string())?;
    fs::write(&path, bytes).map_err(|error| error.to_string())?;
    Ok(path)
}

pub(crate) fn preview_temp_dir() -> PathBuf {
    std::env::temp_dir().join("skenion-runtime-preview")
}

pub(crate) fn cleanup_stale_preview_temp_files(max_age: Duration) -> Result<usize, String> {
    cleanup_stale_preview_temp_files_in(&preview_temp_dir(), max_age, SystemTime::now())
        .map_err(|error| error.to_string())
}

pub(crate) fn remove_preview_temp_file(path: &Path) -> Result<(), String> {
    if !is_preview_temp_file(path) || !path.exists() {
        return Ok(());
    }
    fs::remove_file(path).map_err(|error| error.to_string())
}

fn cleanup_stale_preview_temp_files_in(
    directory: &Path,
    max_age: Duration,
    now: SystemTime,
) -> io::Result<usize> {
    if !directory.exists() {
        return Ok(0);
    }

    let mut removed = 0;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if !is_preview_temp_file(&path) {
            continue;
        }
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if now
            .duration_since(modified)
            .map(|age| age >= max_age)
            .unwrap_or(false)
        {
            fs::remove_file(path)?;
            removed += 1;
        }
    }

    Ok(removed)
}

fn is_preview_temp_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    (name.starts_with("preview-document-") && name.ends_with(".json"))
        || (name.starts_with("preview-") && name.contains("-telemetry.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ExecutionModel, ExecutionPlan, GraphDocument, GraphNode, PlanNode,
        render::RENDER_CLEAR_COLOR_KIND,
    };

    #[test]
    fn preview_document_sets_stable_identity_fields() {
        let document = PreviewDocument::new(graph(), plan(), 12);

        assert_eq!(document.schema, PREVIEW_DOCUMENT_SCHEMA);
        assert_eq!(document.schema_version, PREVIEW_DOCUMENT_SCHEMA_VERSION);
        assert_eq!(document.graph.id, "render-graph");
        assert_eq!(document.plan.graph_id, "render-graph");
        assert!(document.control_state.values.is_empty());
        assert_eq!(document.session_revision, 12);
    }

    #[test]
    fn preview_document_round_trips_json() {
        let document = PreviewDocument::new(graph(), plan(), 3);
        let bytes = serde_json::to_vec(&document).expect("document should serialize");
        let decoded: PreviewDocument =
            serde_json::from_slice(&bytes).expect("document should deserialize");

        assert_eq!(decoded, document);
    }

    #[test]
    fn write_preview_document_writes_json_file() {
        let document = PreviewDocument::new(graph(), plan(), 4);
        let path = write_preview_document(&document).expect("document should be written");
        let bytes = std::fs::read(&path).expect("written document should be readable");
        let decoded: PreviewDocument =
            serde_json::from_slice(&bytes).expect("written document should be JSON");

        assert_eq!(decoded, document);
        std::fs::remove_file(path).expect("test document should be removable");
    }

    #[test]
    fn stale_preview_cleanup_removes_only_preview_temp_files() {
        let directory = std::env::temp_dir().join(format!(
            "skenion-preview-cleanup-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).expect("test directory should create");
        let document_path = directory.join("preview-document-1-2.json");
        let telemetry_path = directory.join("preview-1-2-3-telemetry.json");
        let temp_telemetry_path = directory.join("preview-1-2-3-telemetry.json.tmp");
        let unrelated_path = directory.join("keep.json");
        std::fs::write(&document_path, b"{}").expect("document should write");
        std::fs::write(&telemetry_path, b"{}").expect("telemetry should write");
        std::fs::write(&temp_telemetry_path, b"{}").expect("temp telemetry should write");
        std::fs::write(&unrelated_path, b"{}").expect("unrelated file should write");

        let removed = cleanup_stale_preview_temp_files_in(
            &directory,
            Duration::ZERO,
            SystemTime::now() + Duration::from_secs(1),
        )
        .expect("cleanup should succeed");

        assert_eq!(removed, 3);
        assert!(!document_path.exists());
        assert!(!telemetry_path.exists());
        assert!(!temp_telemetry_path.exists());
        assert!(unrelated_path.exists());
        std::fs::remove_dir_all(directory).expect("test directory should remove");
    }

    fn graph() -> GraphDocument {
        GraphDocument {
            schema: "skenion.graph".to_owned(),
            schema_version: "0.1.0".to_owned(),
            id: "render-graph".to_owned(),
            revision: "1".to_owned(),
            nodes: vec![GraphNode {
                id: "clear_1".to_owned(),
                kind: RENDER_CLEAR_COLOR_KIND.to_owned(),
                kind_version: "0.1.0".to_owned(),
                params: serde_json::Map::new(),
                ports: Vec::new(),
            }],
            edges: Vec::new(),
        }
    }

    fn plan() -> ExecutionPlan {
        ExecutionPlan {
            graph_id: "render-graph".to_owned(),
            graph_revision: "1".to_owned(),
            nodes: vec![PlanNode {
                node_id: "clear_1".to_owned(),
                kind: RENDER_CLEAR_COLOR_KIND.to_owned(),
                kind_version: "0.1.0".to_owned(),
                execution_model: ExecutionModel::GpuPass,
                order: 0,
            }],
            edges: Vec::new(),
            groups: Vec::new(),
        }
    }
}
