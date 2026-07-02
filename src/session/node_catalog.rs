use serde_json::{Value, json};
use skenion_contracts::NodeCatalogSnapshotV01;

use crate::{PackageRegistryListResponseV01, ProjectDocumentCurrent, object_spec::ObjectRegistry};

#[derive(Debug, Clone)]
pub(super) struct RuntimeNodeCatalogCache {
    visible_key: Value,
    snapshot: NodeCatalogSnapshotV01,
}

impl RuntimeNodeCatalogCache {
    pub(super) fn for_project(project: Option<&ProjectDocumentCurrent>) -> Self {
        Self::for_project_with_packages(project, None)
    }

    pub(super) fn for_project_with_packages(
        project: Option<&ProjectDocumentCurrent>,
        packages: Option<&PackageRegistryListResponseV01>,
    ) -> Self {
        Self {
            visible_key: node_catalog_visible_key(project, packages),
            snapshot: ObjectRegistry::for_project_with_packages(project, packages)
                .catalog_projection(),
        }
    }

    pub(super) fn refresh(
        &mut self,
        project: Option<&ProjectDocumentCurrent>,
        packages: Option<&PackageRegistryListResponseV01>,
    ) {
        let visible_key = node_catalog_visible_key(project, packages);
        if self.visible_key == visible_key {
            return;
        }
        self.visible_key = visible_key;
        self.snapshot =
            ObjectRegistry::for_project_with_packages(project, packages).catalog_projection();
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

fn node_catalog_visible_key(
    project: Option<&ProjectDocumentCurrent>,
    packages: Option<&PackageRegistryListResponseV01>,
) -> Value {
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

    let package_objects = packages
        .map(|packages| {
            packages
                .packages
                .iter()
                .flat_map(|package| {
                    package.provides.objects.iter().map(|object| {
                        json!({
                            "packageId": &package.package_id,
                            "packageVersion": &package.version,
                            "objectId": &object.object_id,
                            "primaryObjectSpec": &object.primary_object_spec,
                            "aliases": &object.aliases,
                            "definitionPath": &object.definition_path,
                        })
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    json!({
        "providers": package_objects,
        "projectPatches": project_patches,
    })
}
