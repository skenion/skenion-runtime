use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use thiserror::Error;

use crate::{
    AudioDspPlanOptions, AudioRealtimeDspError, AudioRealtimeDspExecutor, AudioRealtimeDspOptions,
    ProjectRequestCurrent,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioBackendConfig {
    pub block_size: u32,
}

impl Default for AudioBackendConfig {
    fn default() -> Self {
        Self { block_size: 64 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioBackendInfo {
    pub device_name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: String,
}

pub struct RunningAudioBackend {
    stream: cpal::Stream,
    info: AudioBackendInfo,
}

impl RunningAudioBackend {
    pub fn info(&self) -> &AudioBackendInfo {
        &self.info
    }

    pub fn stream(&self) -> &cpal::Stream {
        &self.stream
    }

    pub fn keep_alive_for(self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

#[derive(Debug, Error)]
pub enum AudioBackendError {
    #[error("no default audio output device is available")]
    NoDefaultOutputDevice,
    #[error("audio backend error: {0}")]
    Backend(#[from] cpal::Error),
    #[error("default output sample format {0:?} is not supported by audio backend v0")]
    UnsupportedSampleFormat(cpal::SampleFormat),
    #[error("{0}")]
    Dsp(#[from] AudioRealtimeDspError),
}

pub fn start_default_audio_output_backend(
    request: &ProjectRequestCurrent,
    config: AudioBackendConfig,
) -> Result<RunningAudioBackend, AudioBackendError> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or(AudioBackendError::NoDefaultOutputDevice)?;
    let device_name = format!("{device}");
    let output_config = device.default_output_config()?;
    let sample_format = output_config.sample_format();
    if sample_format != cpal::SampleFormat::F32 {
        return Err(AudioBackendError::UnsupportedSampleFormat(sample_format));
    }

    let sample_rate = output_config.sample_rate();
    let channels = output_config.channels();
    let stream_config = output_config.config();
    let mut executor = AudioRealtimeDspExecutor::new(
        request,
        AudioRealtimeDspOptions {
            plan: AudioDspPlanOptions {
                block_size: config.block_size,
                sample_rate,
            },
            channels: usize::from(channels),
        },
    )?;
    let stream = device.build_output_stream(
        stream_config,
        move |data: &mut [f32], _| {
            executor.process_interleaved_output(data);
        },
        |error| eprintln!("skenion audio backend stream error: {error}"),
        None,
    )?;
    stream.play()?;

    Ok(RunningAudioBackend {
        stream,
        info: AudioBackendInfo {
            device_name,
            sample_rate,
            channels,
            sample_format: format!("{sample_format:?}"),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_audio_backend_config_uses_realtime_block_size() {
        assert_eq!(AudioBackendConfig::default().block_size, 64);
    }

    #[test]
    fn audio_backend_errors_describe_device_and_format_failures() {
        assert_eq!(
            AudioBackendError::NoDefaultOutputDevice.to_string(),
            "no default audio output device is available"
        );
        assert!(
            AudioBackendError::UnsupportedSampleFormat(cpal::SampleFormat::I16)
                .to_string()
                .contains("I16")
        );
    }
}
