//! The only part of this crate that knows cpal exists.
//!
//! Two entry points, and no more: [`list`] for the settings window and [`DeviceSource`] for
//! the stream itself. Everything downstream works on [`Source`], so a test never reaches a
//! driver and CI — `windows-latest`, with no audio device at all — never fails for the want
//! of a microphone.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};

use crate::error::Error;
use crate::source::{RawSink, Source, SourceSpec, StreamGuard};

/// An input device, as the settings window will list it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Human-readable name, as the operating system spells it.
    pub name: String,
    /// The stable handle to store in settings.
    ///
    /// cpal's own identifier, which survives a reboot and a reconnection where the platform
    /// allows it. The name does not: two identical headsets have one name and two ids.
    pub id: String,
    /// Whether this is the host's default input.
    pub is_default: bool,
}

/// Every input device on the default host.
pub(crate) fn list() -> Result<Vec<DeviceInfo>, Error> {
    let host = cpal::default_host();
    let default_id = host
        .default_input_device()
        .and_then(|device| device.id().ok())
        .map(|id| id.to_string());

    let mut devices = Vec::new();
    for device in host.input_devices().map_err(Error::Devices)? {
        let Ok(id) = device.id() else {
            // A device that vanished between the enumeration and this call. Skipping it is
            // the honest answer; it is not there any more.
            continue;
        };
        let id = id.to_string();
        devices.push(DeviceInfo {
            name: device.to_string(),
            is_default: Some(&id) == default_id.as_ref(),
            id,
        });
    }
    Ok(devices)
}

/// A cpal input stream, wrapped as a [`Source`].
pub(crate) struct DeviceSource {
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
}

impl DeviceSource {
    /// Open the named device, or the host default when `wanted` is `None`.
    ///
    /// `wanted` is matched against the stable id first and the display name second, so a
    /// settings file written before the id was stored still finds the right microphone.
    pub(crate) fn open(wanted: Option<&str>) -> Result<Self, Error> {
        let host = cpal::default_host();
        let device = match wanted {
            None => host.default_input_device().ok_or(Error::NoDevice)?,
            Some(wanted) => find(&host, wanted)?,
        };

        // The device's native config, not a requested one: asking WASAPI for a rate it does
        // not run at gets a resampler inside the driver, or a refusal. Dile resamples in
        // this crate, where the quality is ours to measure.
        let config = device
            .default_input_config()
            .map_err(Error::UnsupportedConfig)?;

        Ok(DeviceSource { device, config })
    }

    /// The device's name, for a log line or the panel.
    pub(crate) fn name(&self) -> String {
        self.device.to_string()
    }

    /// The device's native sample format, for a log line.
    pub(crate) fn sample_format(&self) -> SampleFormat {
        self.config.sample_format()
    }
}

fn find(host: &cpal::Host, wanted: &str) -> Result<cpal::Device, Error> {
    for device in host.input_devices().map_err(Error::Devices)? {
        let matches_id = device
            .id()
            .map(|id| id.to_string() == wanted)
            .unwrap_or(false);
        if matches_id || device.to_string() == wanted {
            return Ok(device);
        }
    }
    Err(Error::DeviceNotFound(wanted.to_string()))
}

impl Source for DeviceSource {
    fn spec(&self) -> SourceSpec {
        SourceSpec::new(self.config.sample_rate(), self.config.channels())
    }

    fn start(self: Box<Self>, sink: RawSink) -> Result<Box<dyn StreamGuard>, Error> {
        let config = self.config.config();
        let format = self.config.sample_format();

        let stream = match format {
            SampleFormat::F32 => build_f32(&self.device, config, sink),
            SampleFormat::I8 => build::<i8>(&self.device, config, sink),
            SampleFormat::I16 => build::<i16>(&self.device, config, sink),
            SampleFormat::I32 => build::<i32>(&self.device, config, sink),
            SampleFormat::U8 => build::<u8>(&self.device, config, sink),
            SampleFormat::U16 => build::<u16>(&self.device, config, sink),
            SampleFormat::U32 => build::<u32>(&self.device, config, sink),
            SampleFormat::F64 => build::<f64>(&self.device, config, sink),
            other => return Err(Error::UnsupportedSampleFormat(other)),
        }?;

        stream.play().map_err(Error::Stream)?;
        Ok(Box::new(DeviceGuard { _stream: stream }))
    }
}

/// The float path, which is what WASAPI hands over on a normal Windows machine.
///
/// Separate from the generic one because it needs no conversion at all: the callback is a
/// single `push` of the buffer it was given.
fn build_f32(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut sink: RawSink,
) -> Result<cpal::Stream, Error> {
    device
        .build_input_stream(config, move |data: &[f32], _| sink.push(data), report, None)
        .map_err(Error::Stream)
}

/// Every other integer or double format, converted a stack block at a time.
///
/// The scratch array is on the stack and its size is fixed, so the callback still allocates
/// nothing — the one property that matters more here than anywhere else in the crate.
fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut sink: RawSink,
) -> Result<cpal::Stream, Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    const BLOCK: usize = 1024;
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                let mut scratch = [0.0f32; BLOCK];
                for block in data.chunks(BLOCK) {
                    for (destination, &source) in scratch.iter_mut().zip(block) {
                        *destination = f32::from_sample(source);
                    }
                    sink.push(&scratch[..block.len()]);
                }
            },
            report,
            None,
        )
        .map_err(Error::Stream)
}

/// What the host says when the stream goes wrong.
///
/// Logged rather than propagated: the error callback runs on the audio thread, there is
/// nowhere to return a `Result` to, and a stream that was automatically rerouted is not a
/// failure at all. WP5 turns a `DeviceNotAvailable` here into a visible panel state; until
/// there is a panel, a log line is the honest amount of handling.
fn report(error: cpal::Error) {
    match error.kind() {
        cpal::ErrorKind::DeviceChanged => {
            log::info!("the audio route changed and the input stream followed it: {error}");
        }
        cpal::ErrorKind::Xrun => log::warn!("the input stream glitched: {error}"),
        _ => log::error!("input stream error: {error}"),
    }
}

struct DeviceGuard {
    _stream: cpal::Stream,
}

impl StreamGuard for DeviceGuard {}

#[cfg(test)]
mod tests {
    /// Listing devices touches the host, which CI does not have. Ignored rather than
    /// deleted: it is the check the maintainer runs on a machine with a microphone.
    #[test]
    #[ignore = "needs audio hardware"]
    fn the_host_lists_at_least_one_input() {
        let devices = super::list().expect("the device list is readable");
        assert!(!devices.is_empty(), "this machine has a microphone");
        assert_eq!(
            devices.iter().filter(|device| device.is_default).count(),
            1,
            "exactly one default input"
        );
    }
}
