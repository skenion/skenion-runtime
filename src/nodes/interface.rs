use crate::object_spec::{ObjectSpecResolution, ParsedObjectSpec};

pub(crate) type CoreNodeResolveFn =
    fn(ParsedObjectSpec, &crate::object_spec::ObjectRegistryCandidate) -> ObjectSpecResolution;

pub(crate) trait CoreNodeImplementation: Sync {
    fn kind(&self) -> &'static str;
    fn object_id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn aliases(&self) -> &'static [&'static str];
    fn resolve(
        &self,
        parsed: ParsedObjectSpec,
        candidate: &crate::object_spec::ObjectRegistryCandidate,
    ) -> ObjectSpecResolution;

    fn catalog_category(&self) -> &'static str {
        if self.kind().starts_with("object.core.audio.") {
            "Core Audio"
        } else {
            "Core"
        }
    }
}
