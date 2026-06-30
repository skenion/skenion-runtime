use super::CoreNodeDescriptor;

pub(super) static SUBPATCH: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.subpatch",
    "subpatch",
    "Subpatch",
    &["p", "object.core.subpatch"],
    crate::object_spec::resolve_core_subpatch,
    "Core",
);

pub(super) static INLET: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.inlet",
    "inlet",
    "Inlet",
    &["inlet", "object.core.inlet"],
    crate::object_spec::resolve_core_boundary_port,
    "Core",
);

pub(super) static OUTLET: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.outlet",
    "outlet",
    "Outlet",
    &["outlet", "object.core.outlet"],
    crate::object_spec::resolve_core_boundary_port,
    "Core",
);
