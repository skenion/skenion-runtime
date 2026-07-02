use super::{CoreNodeImplementation, CoreNodeResolveFn};
use crate::object_spec::{ObjectRegistryCandidate, ObjectSpecResolution, ParsedObjectSpec};

mod audio;
mod control;
mod patching;

pub(crate) fn first_party_core_nodes() -> &'static [&'static dyn CoreNodeImplementation] {
    FIRST_PARTY_CORE_NODES
}

static FIRST_PARTY_CORE_NODES: &[&dyn CoreNodeImplementation] = &[
    &control::ADD,
    &control::SUBTRACT,
    &control::MULTIPLY,
    &control::DIVIDE,
    &control::POWER,
    &control::MINIMUM,
    &control::MAXIMUM,
    &control::SQUARE_ROOT,
    &control::FLOAT,
    &control::INTEGER,
    &control::BANG,
    &control::MESSAGE,
    &control::COMMENT,
    &audio::SIGNAL,
    &audio::OSCILLATOR,
    &audio::MULTIPLY,
    &audio::INPUT,
    &audio::OUTPUT,
    &patching::SUBPATCH,
    &patching::INLET,
    &patching::OUTLET,
];

pub(super) struct CoreNodeDescriptor {
    kind: &'static str,
    object_id: &'static str,
    display_name: &'static str,
    aliases: &'static [&'static str],
    resolve: CoreNodeResolveFn,
    catalog_category: &'static str,
}

impl CoreNodeDescriptor {
    pub(super) const fn new(
        kind: &'static str,
        object_id: &'static str,
        display_name: &'static str,
        aliases: &'static [&'static str],
        resolve: CoreNodeResolveFn,
        catalog_category: &'static str,
    ) -> Self {
        Self {
            kind,
            object_id,
            display_name,
            aliases,
            resolve,
            catalog_category,
        }
    }
}

impl CoreNodeImplementation for CoreNodeDescriptor {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn object_id(&self) -> &'static str {
        self.object_id
    }

    fn display_name(&self) -> &'static str {
        self.display_name
    }

    fn aliases(&self) -> &'static [&'static str] {
        self.aliases
    }

    fn resolve(
        &self,
        parsed: ParsedObjectSpec,
        candidate: &ObjectRegistryCandidate,
    ) -> ObjectSpecResolution {
        (self.resolve)(parsed, candidate)
    }

    fn catalog_category(&self) -> &'static str {
        self.catalog_category
    }
}

#[cfg(test)]
mod tests;
