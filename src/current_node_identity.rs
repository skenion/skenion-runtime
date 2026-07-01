use crate::{GraphNodeCurrent, ObjectImplementationRefCurrent, ObjectProviderRefCurrent};

pub(crate) fn graph_node_executable_kind(node: &GraphNodeCurrent) -> Option<String> {
    node.implementation
        .as_ref()
        .map(implementation_executable_kind)
}

pub(crate) fn graph_node_object_id(node: &GraphNodeCurrent) -> Option<&str> {
    node.implementation
        .as_ref()
        .map(|implementation| implementation.object_id.as_str())
}

pub(crate) fn implementation_executable_kind(
    implementation: &ObjectImplementationRefCurrent,
) -> String {
    match &implementation.provider {
        ObjectProviderRefCurrent::Core => {
            core_object_id_to_executable_kind(&implementation.object_id)
        }
        ObjectProviderRefCurrent::ProjectPatch { patch_id, .. } => {
            project_patch_object_kind(patch_id)
        }
        ObjectProviderRefCurrent::Package { .. } => implementation.object_id.clone(),
    }
}

pub(crate) fn core_object_id_to_executable_kind(object_id: &str) -> String {
    if object_id.starts_with("object.core.") {
        object_id.to_owned()
    } else {
        format!("object.core.{object_id}")
    }
}

fn project_patch_object_kind(patch_id: &str) -> String {
    format!(
        "object.project.patch.{}",
        patch_id
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '.') {
                    character
                } else {
                    '-'
                }
            })
            .collect::<String>()
    )
}
