use super::*;
use crate::nodes::CoreNodeImplementation;
use crate::object_spec::{ObjectRegistryCandidate, ObjectSpecResolution, ParsedObjectSpec};

static TEST_ALIASES: &[&str] = &["test", "object.test.alias"];

#[test]
fn descriptor_exposes_core_node_metadata() {
    let descriptor = CoreNodeDescriptor::new(
        "object.test.node",
        "test.node",
        "Test Node",
        TEST_ALIASES,
        crate::object_spec::resolve_core_audio,
        "Core Audio",
    );

    assert_eq!(descriptor.kind(), "object.test.node");
    assert_eq!(descriptor.object_id(), "test.node");
    assert_eq!(descriptor.display_name(), "Test Node");
    assert_eq!(descriptor.aliases(), TEST_ALIASES);
    assert_eq!(descriptor.catalog_category(), "Core Audio");
}

#[test]
fn descriptor_maps_non_audio_nodes_to_core_catalog_category() {
    let descriptor = CoreNodeDescriptor::new(
        "object.test.node",
        "test.node",
        "Test Node",
        &[],
        crate::object_spec::resolve_core_control_value,
        "Core",
    );
    assert_eq!(descriptor.catalog_category(), "Core");
}

struct DefaultCategoryNode {
    kind: &'static str,
}

impl CoreNodeImplementation for DefaultCategoryNode {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn object_id(&self) -> &'static str {
        "test.node"
    }

    fn display_name(&self) -> &'static str {
        "Test Node"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &[]
    }

    fn resolve(
        &self,
        _parsed: ParsedObjectSpec,
        _candidate: &ObjectRegistryCandidate,
    ) -> ObjectSpecResolution {
        unreachable!("default category tests do not resolve object specs")
    }
}

#[test]
fn default_core_node_category_follows_audio_kind_namespace() {
    let audio = DefaultCategoryNode {
        kind: "object.core.audio.test",
    };
    let control = DefaultCategoryNode {
        kind: "object.core.test",
    };

    assert_eq!(audio.catalog_category(), "Core Audio");
    assert_eq!(control.catalog_category(), "Core");
}
