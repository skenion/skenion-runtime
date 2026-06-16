use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{ExecutionPlan, GraphDocument};

pub const PREVIEW_DOCUMENT_SCHEMA: &str = "skenion.preview.document";
pub const PREVIEW_DOCUMENT_SCHEMA_VERSION: &str = "0.1.0";

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewDocument {
    pub schema: String,
    pub schema_version: String,
    pub graph: GraphDocument,
    pub plan: ExecutionPlan,
    pub session_revision: u64,
}

impl PreviewDocument {
    pub fn new(graph: GraphDocument, plan: ExecutionPlan, session_revision: u64) -> Self {
        Self {
            schema: PREVIEW_DOCUMENT_SCHEMA.to_owned(),
            schema_version: PREVIEW_DOCUMENT_SCHEMA_VERSION.to_owned(),
            graph,
            plan,
            session_revision,
        }
    }
}

pub fn write_preview_document(document: &PreviewDocument) -> Result<PathBuf, String> {
    let directory = std::env::temp_dir().join("skenion-runtime-preview");
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
