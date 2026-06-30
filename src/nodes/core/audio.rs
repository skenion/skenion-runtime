use super::CoreNodeDescriptor;

pub(super) static SIGNAL: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.audio.sig",
    "audio.sig",
    "Signal",
    &["sig~", "object.core.audio.sig"],
    crate::object_spec::resolve_core_audio,
    "Core Audio",
);

pub(super) static OSCILLATOR: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.audio.osc",
    "audio.osc",
    "Oscillator",
    &["osc~", "object.core.audio.osc"],
    crate::object_spec::resolve_core_audio,
    "Core Audio",
);

pub(super) static MULTIPLY: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.audio.operator.mul",
    "audio.operator.mul",
    "Audio Multiply",
    &["*~", "object.core.audio.operator.mul"],
    crate::object_spec::resolve_core_audio,
    "Core Audio",
);

pub(super) static INPUT: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.audio.input",
    "audio.input",
    "Audio Input",
    &["adc~", "object.core.audio.input"],
    crate::object_spec::resolve_core_audio,
    "Core Audio",
);

pub(super) static OUTPUT: CoreNodeDescriptor = CoreNodeDescriptor::new(
    "object.core.audio.output",
    "audio.output",
    "Audio Output",
    &["dac~", "object.core.audio.output"],
    crate::object_spec::resolve_core_audio,
    "Core Audio",
);
