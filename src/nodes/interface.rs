#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoreNodeConstructor {
    ControlOperator,
    ControlValue,
    Audio,
    Subpatch,
    BoundaryPort,
}

pub(crate) trait CoreNodeImplementation: Sync {
    fn kind(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn aliases(&self) -> &'static [&'static str];
    fn constructor(&self) -> CoreNodeConstructor;

    fn catalog_category(&self) -> &'static str {
        match self.constructor() {
            CoreNodeConstructor::Audio => "Core Audio",
            CoreNodeConstructor::ControlOperator
            | CoreNodeConstructor::ControlValue
            | CoreNodeConstructor::Subpatch
            | CoreNodeConstructor::BoundaryPort => "Core",
        }
    }
}
