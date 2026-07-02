use super::CoreNodeDescriptor;

pub(super) static ADD: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.operator.add",
    "operator.add",
    "Add",
    &["+", "add", "object.core.operator.add"],
    crate::object_spec::resolve_core_control_operator,
    "Core",
);

pub(super) static SUBTRACT: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.operator.sub",
    "operator.sub",
    "Subtract",
    &["-", "sub", "object.core.operator.sub"],
    crate::object_spec::resolve_core_control_operator,
    "Core",
);

pub(super) static MULTIPLY: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.operator.mul",
    "operator.mul",
    "Multiply",
    &["*", "mul", "object.core.operator.mul"],
    crate::object_spec::resolve_core_control_operator,
    "Core",
);

pub(super) static DIVIDE: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.operator.div",
    "operator.div",
    "Divide",
    &["/", "div", "object.core.operator.div"],
    crate::object_spec::resolve_core_control_operator,
    "Core",
);

pub(super) static POWER: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.operator.pow",
    "operator.pow",
    "Power",
    &["pow", "object.core.operator.pow"],
    crate::object_spec::resolve_core_control_operator,
    "Core",
);

pub(super) static MINIMUM: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.operator.min",
    "operator.min",
    "Minimum",
    &["min", "object.core.operator.min"],
    crate::object_spec::resolve_core_control_operator,
    "Core",
);

pub(super) static MAXIMUM: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.operator.max",
    "operator.max",
    "Maximum",
    &["max", "object.core.operator.max"],
    crate::object_spec::resolve_core_control_operator,
    "Core",
);

pub(super) static SQUARE_ROOT: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.operator.sqrt",
    "operator.sqrt",
    "Square Root",
    &["sqrt", "object.core.operator.sqrt"],
    crate::object_spec::resolve_core_control_operator,
    "Core",
);

pub(super) static FLOAT: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.float",
    "float",
    "Float",
    &["float", "f", "number", "object.core.float"],
    crate::object_spec::resolve_core_control_value,
    "Core",
);

pub(super) static INTEGER: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.int",
    "int",
    "Integer",
    &["int", "integer", "i", "object.core.int"],
    crate::object_spec::resolve_core_control_value,
    "Core",
);

pub(super) static BANG: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.bang",
    "bang",
    "Bang",
    &["bang", "b", "object.core.bang"],
    crate::object_spec::resolve_core_control_value,
    "Core",
);

pub(super) static MESSAGE: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.message",
    "message",
    "Message",
    &["message", "msg", "object.core.message"],
    crate::object_spec::resolve_core_control_value,
    "Core",
);

pub(super) static COMMENT: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.comment",
    "comment",
    "Comment",
    &["comment", "object.core.comment"],
    crate::object_spec::resolve_core_control_value,
    "Core",
);
