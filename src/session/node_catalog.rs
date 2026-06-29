use serde_json::{Value, json};
use skenion_contracts::NodeCatalogSnapshotV01;

use crate::{ProjectDocumentCurrent, object_text::ObjectRegistry};

#[derive(Debug, Clone)]
pub(super) struct RuntimeNodeCatalogCache {
    visible_key: Value,
    snapshot: NodeCatalogSnapshotV01,
}

impl RuntimeNodeCatalogCache {
    pub(super) fn for_project(project: Option<&ProjectDocumentCurrent>) -> Self {
        Self {
            visible_key: node_catalog_visible_key(project),
            snapshot: ObjectRegistry::for_project(project).catalog_projection(),
        }
    }

    pub(super) fn refresh(&mut self, project: Option<&ProjectDocumentCurrent>) {
        let visible_key = node_catalog_visible_key(project);
        if self.visible_key == visible_key {
            return;
        }
        self.visible_key = visible_key;
        self.snapshot = ObjectRegistry::for_project(project).catalog_projection();
    }

    pub(super) fn snapshot(&self) -> NodeCatalogSnapshotV01 {
        self.snapshot.clone()
    }
}

impl Default for RuntimeNodeCatalogCache {
    fn default() -> Self {
        Self::for_project(None)
    }
}

fn node_catalog_visible_key(project: Option<&ProjectDocumentCurrent>) -> Value {
    let mut project_patches = project
        .map(|project| {
            project
                .patch_library
                .iter()
                .map(|patch| {
                    let metadata = patch.metadata.as_ref();
                    json!({
                        "id": &patch.id,
                        "title": metadata.and_then(|metadata| metadata.title.as_deref()),
                        "description": metadata.and_then(|metadata| metadata.description.as_deref()),
                        "interfaceDigest": skenion_contracts::compute_patch_interface_digest_v01(patch),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    project_patches.sort_by(|left, right| {
        left.get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(right.get("id").and_then(Value::as_str).unwrap_or_default())
    });

    json!({
        "providers": [],
        "projectPatches": project_patches,
    })
}
