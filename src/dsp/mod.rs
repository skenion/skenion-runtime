mod domains;
mod offline;
mod planning;
mod ports;
mod realtime;
mod types;
mod values;

pub use offline::run_offline_audio_dsp_current;
pub(crate) use planning::build_audio_dsp_plan;
pub use planning::build_audio_dsp_plan_current;
pub(super) use planning::build_audio_dsp_plan_with_graph_current;
pub use realtime::AudioRealtimeDspExecutor;
pub use types::{
    AudioDspBlockReport, AudioDspBuffer, AudioDspControlInput, AudioDspPlan, AudioDspPlanEdge,
    AudioDspPlanError, AudioDspPlanNode, AudioDspPlanOptions, AudioDspRenderedBuffer,
    AudioDspSignalInput, AudioDspSignalOutput, AudioDspSnapshot, AudioEndpointPlanNode,
    AudioOfflineDspError, AudioOfflineDspOptions, AudioOfflineDspReport, AudioRealtimeDspError,
    AudioRealtimeDspOptions,
};

const AUDIO_SIGNAL_KIND: &str = "value.core.float32";
const AUDIO_INPUT_KIND: &str = "object.core.audio.input";
const AUDIO_OUTPUT_KIND: &str = "object.core.audio.output";
const AUDIO_CLOCK_BRIDGE_KIND: &str = "object.core.audio.clock-bridge";
const AUDIO_RESAMPLE_KIND: &str = "object.core.audio.resample";
const DEFAULT_BLOCK_SIZE: u32 = 64;
const DEFAULT_SAMPLE_RATE: u32 = 48_000;
const DEFAULT_SAMPLE_FORMAT: &str = "f32";

#[cfg(test)]
mod tests;
