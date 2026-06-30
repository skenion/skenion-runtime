use super::{CoreNodeConstructor, CoreNodeImplementation};

pub(crate) fn first_party_core_nodes() -> &'static [&'static dyn CoreNodeImplementation] {
    FIRST_PARTY_CORE_NODES
}

static FIRST_PARTY_CORE_NODES: &[&dyn CoreNodeImplementation] = &[
    &Add,
    &Subtract,
    &Multiply,
    &Divide,
    &Power,
    &Minimum,
    &Maximum,
    &SquareRoot,
    &Float,
    &Integer,
    &UnsignedInteger,
    &Bang,
    &Message,
    &Comment,
    &AudioSignal,
    &AudioOscillator,
    &AudioMultiply,
    &AudioInput,
    &AudioOutput,
    &Subpatch,
    &Inlet,
    &Outlet,
];

macro_rules! core_node {
    ($type_name:ident, $kind:literal, $display:literal, [$($alias:literal),* $(,)?], $constructor:expr) => {
        struct $type_name;

        impl CoreNodeImplementation for $type_name {
            fn kind(&self) -> &'static str {
                $kind
            }

            fn display_name(&self) -> &'static str {
                $display
            }

            fn aliases(&self) -> &'static [&'static str] {
                &[$($alias),*]
            }

            fn constructor(&self) -> CoreNodeConstructor {
                $constructor
            }
        }
    };
}

core_node!(
    Add,
    "object.core.operator.add",
    "Add",
    ["+", "add", "object.core.operator.add"],
    CoreNodeConstructor::ControlOperator
);
core_node!(
    Subtract,
    "object.core.operator.sub",
    "Subtract",
    ["-", "sub", "object.core.operator.sub"],
    CoreNodeConstructor::ControlOperator
);
core_node!(
    Multiply,
    "object.core.operator.mul",
    "Multiply",
    ["*", "mul", "object.core.operator.mul"],
    CoreNodeConstructor::ControlOperator
);
core_node!(
    Divide,
    "object.core.operator.div",
    "Divide",
    ["/", "div", "object.core.operator.div"],
    CoreNodeConstructor::ControlOperator
);
core_node!(
    Power,
    "object.core.operator.pow",
    "Power",
    ["pow", "object.core.operator.pow"],
    CoreNodeConstructor::ControlOperator
);
core_node!(
    Minimum,
    "object.core.operator.min",
    "Minimum",
    ["min", "object.core.operator.min"],
    CoreNodeConstructor::ControlOperator
);
core_node!(
    Maximum,
    "object.core.operator.max",
    "Maximum",
    ["max", "object.core.operator.max"],
    CoreNodeConstructor::ControlOperator
);
core_node!(
    SquareRoot,
    "object.core.operator.sqrt",
    "Square Root",
    ["sqrt", "object.core.operator.sqrt"],
    CoreNodeConstructor::ControlOperator
);
core_node!(
    Float,
    "object.core.float",
    "Float",
    ["f", "float", "number", "object.core.float"],
    CoreNodeConstructor::ControlValue
);
core_node!(
    Integer,
    "object.core.int",
    "Integer",
    ["i", "int", "object.core.int"],
    CoreNodeConstructor::ControlValue
);
core_node!(
    UnsignedInteger,
    "object.core.uint",
    "Unsigned Integer",
    ["u", "uint", "object.core.uint"],
    CoreNodeConstructor::ControlValue
);
core_node!(
    Bang,
    "object.core.bang",
    "Bang",
    ["b", "bang", "object.core.bang"],
    CoreNodeConstructor::ControlValue
);
core_node!(
    Message,
    "object.core.message",
    "Message",
    ["msg", "message", "object.core.message"],
    CoreNodeConstructor::ControlValue
);
core_node!(
    Comment,
    "object.core.comment",
    "Comment",
    ["comment", "object.core.comment"],
    CoreNodeConstructor::ControlValue
);
core_node!(
    AudioSignal,
    "object.core.audio.sig",
    "Signal",
    ["sig~", "object.core.audio.sig"],
    CoreNodeConstructor::Audio
);
core_node!(
    AudioOscillator,
    "object.core.audio.osc",
    "Oscillator",
    ["osc~", "object.core.audio.osc"],
    CoreNodeConstructor::Audio
);
core_node!(
    AudioMultiply,
    "object.core.audio.operator.mul",
    "Audio Multiply",
    ["*~", "object.core.audio.operator.mul"],
    CoreNodeConstructor::Audio
);
core_node!(
    AudioInput,
    "object.core.audio.input",
    "Audio Input",
    ["adc~", "object.core.audio.input"],
    CoreNodeConstructor::Audio
);
core_node!(
    AudioOutput,
    "object.core.audio.output",
    "Audio Output",
    ["dac~", "object.core.audio.output"],
    CoreNodeConstructor::Audio
);
core_node!(
    Subpatch,
    "object.core.subpatch",
    "Subpatch",
    ["p", "object.core.subpatch"],
    CoreNodeConstructor::Subpatch
);
core_node!(
    Inlet,
    "object.core.inlet",
    "Inlet",
    ["inlet", "object.core.inlet"],
    CoreNodeConstructor::BoundaryPort
);
core_node!(
    Outlet,
    "object.core.outlet",
    "Outlet",
    ["outlet", "object.core.outlet"],
    CoreNodeConstructor::BoundaryPort
);
