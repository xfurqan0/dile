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

/// Every input device on the default host, named so that a person can tell them apart.
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
        if is_discard_device(&id) {
            continue;
        }
        devices.push(DeviceInfo {
            name: tidy_name(&device.to_string(), &id),
            is_default: Some(&id) == default_id.as_ref(),
            id,
        });
    }
    disambiguate(&mut devices);
    Ok(devices)
}

/// ALSA's `null`, which is a bin rather than a microphone.
///
/// It describes itself as *"Discard all samples (playback) or generate zero samples
/// (capture)"*, it is enumerated as an input because it technically is one, and choosing it
/// gives a product that records perfect silence forever and says "nothing heard" every time.
/// Windows has no equivalent to offer, so no Windows user has ever had to not pick it.
///
/// Matched on the whole id rather than a suffix so that a sound card someone named `null`
/// keeps its place, and so that a change in how cpal spells an id makes this stop filtering
/// rather than start filtering something real.
fn is_discard_device(id: &str) -> bool {
    id == "alsa:null"
}

/// What to call a device in a list somebody has to choose from.
///
/// On Windows the system's own name is already the answer — *"Mikrofon (PRO X)"* — and this
/// returns it untouched. ALSA's is a card name and a description joined by a comma, and the
/// description is routinely empty, which arrives as `"sof-hda-dsp, "`: a trailing separator
/// with nothing after it. Trimming that is not renaming anything; it is dropping punctuation
/// that was only ever there to join two fields, one of which does not exist.
///
/// A name that trims away to nothing falls back to the id, because a blank row in a device
/// list is worse than a technical one.
fn tidy_name(raw: &str, id: &str) -> String {
    let trimmed = raw.trim().trim_end_matches([',', ';', '-']).trim();
    if trimmed.is_empty() {
        short_id(id)
    } else {
        trimmed.to_owned()
    }
}

/// Give devices that share a name the part of their id that differs.
///
/// One piece of hardware appears in the ALSA list many times over — `sysdefault:`, `hw:` and
/// `plughw:` for each of its subdevices — and every one of them carries the same card name.
/// Seven identical rows is not a choice, it is a guess, and the one a user is most likely to
/// land on (`hw:`) is also the one that refuses a sample rate it does not run at natively.
///
/// Only names that actually collide are touched, which is what keeps this from doing anything
/// at all on a platform whose names are already distinct.
fn disambiguate(devices: &mut [DeviceInfo]) {
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for device in devices.iter() {
        *seen.entry(device.name.as_str()).or_default() += 1;
    }
    let collisions: Vec<String> = seen
        .into_iter()
        .filter(|&(_, count)| count > 1)
        .map(|(name, _)| name.to_owned())
        .collect();

    for device in devices.iter_mut() {
        if collisions.contains(&device.name) {
            device.name = format!("{} ({})", device.name, short_id(&device.id));
        }
    }
}

/// An id with its host prefix taken off, for use inside a name.
///
/// `alsa:hw:CARD=0,DEV=6` reads as `hw:CARD=0,DEV=6`. The host is the same for every row in
/// the list, so printing it seven times says nothing.
fn short_id(id: &str) -> String {
    id.split_once(':')
        .map_or_else(|| id.to_owned(), |(_, rest)| rest.to_owned())
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
    use super::{DeviceInfo, disambiguate, is_discard_device, short_id, tidy_name};

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
        for device in &devices {
            assert!(!device.name.trim().is_empty(), "{device:?} has no name");
        }
        let mut names: Vec<&str> = devices.iter().map(|device| device.name.as_str()).collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before, "two rows a person cannot tell apart");
    }

    /// The measured shape on Fedora 44: a card name, a comma, and an empty description.
    #[test]
    fn an_alsa_name_loses_the_separator_its_second_field_never_filled() {
        assert_eq!(
            tidy_name("sof-hda-dsp, ", "alsa:hw:CARD=0,DEV=0"),
            "sof-hda-dsp"
        );
        assert_eq!(
            tidy_name("  HDA Intel PCH,  ", "alsa:hw:CARD=0"),
            "HDA Intel PCH"
        );
    }

    /// A description that says something keeps every character of it, comma included.
    #[test]
    fn a_name_with_a_real_second_field_is_left_alone() {
        assert_eq!(
            tidy_name("sof-hda-dsp, HDA Analog", "alsa:hw:CARD=0,DEV=0"),
            "sof-hda-dsp, HDA Analog"
        );
        assert_eq!(
            tidy_name("Mikrofon (PRO X)", "wasapi:{0.0.1.0}"),
            "Mikrofon (PRO X)"
        );
    }

    /// A blank row is worse than a technical one.
    #[test]
    fn a_name_that_trims_to_nothing_falls_back_to_the_id() {
        assert_eq!(
            tidy_name(", ", "alsa:plughw:CARD=0,DEV=7"),
            "plughw:CARD=0,DEV=7"
        );
        assert_eq!(tidy_name("   ", "alsa:pipewire"), "pipewire");
    }

    /// Seven rows reading `sof-hda-dsp` is a guess, not a choice.
    #[test]
    fn devices_that_share_a_name_are_told_apart_by_their_id() {
        let mut devices = vec![
            device("sof-hda-dsp", "alsa:hw:CARD=0,DEV=0"),
            device("sof-hda-dsp", "alsa:plughw:CARD=0,DEV=0"),
            device("PipeWire Sound Server", "alsa:pipewire"),
        ];
        disambiguate(&mut devices);
        assert_eq!(devices[0].name, "sof-hda-dsp (hw:CARD=0,DEV=0)");
        assert_eq!(devices[1].name, "sof-hda-dsp (plughw:CARD=0,DEV=0)");
        // Untouched: nothing else is called this, so there is nothing to tell it apart from.
        assert_eq!(devices[2].name, "PipeWire Sound Server");
    }

    /// Which is also the whole of what happens on a platform whose names already differ.
    #[test]
    fn distinct_names_are_left_exactly_as_the_system_spells_them() {
        let mut devices = vec![
            device("Mikrofon (PRO X)", "wasapi:{0.0.1.0}"),
            device("Microphone Array", "wasapi:{0.0.1.1}"),
        ];
        let before = devices.clone();
        disambiguate(&mut devices);
        assert_eq!(devices, before);
    }

    /// The bin is not a microphone, and a card someone named `null` still is one.
    #[test]
    fn only_alsas_own_discard_device_is_left_out_of_the_list() {
        assert!(is_discard_device("alsa:null"));
        assert!(!is_discard_device("alsa:nullcard"));
        assert!(!is_discard_device("alsa:hw:CARD=null"));
        assert!(!is_discard_device("wasapi:null"));
    }

    #[test]
    fn a_short_id_drops_the_host_every_row_shares() {
        assert_eq!(short_id("alsa:hw:CARD=0,DEV=6"), "hw:CARD=0,DEV=6");
        assert_eq!(short_id("nocolon"), "nocolon");
    }

    fn device(name: &str, id: &str) -> DeviceInfo {
        DeviceInfo {
            name: name.to_owned(),
            id: id.to_owned(),
            is_default: false,
        }
    }
}
