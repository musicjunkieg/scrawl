//! The microphone, through `cpal`: opening an input device into a `Sink` (16 kHz mono through
//! `Downmix`), finding a device by id, and a `Recorder` on the default input.

use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
pub use cpal;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::audio::{Downmix, Recorder, Sink};

/// Records the default input device (`cpal::default_host()`), opened on the recorder's own
/// thread. `log` hears the device's format and a failure partway.
pub fn start_default(log: Option<fn(&str)>) -> Result<Recorder> {
    Recorder::start(
        "scrawl-audio",
        move |sink| {
            let device = cpal::default_host()
                .default_input_device()
                .context("No microphone is available")?;
            open_device(&device, sink, log)
        },
        log,
    )
}

/// The input device with `id` (`cpal::DeviceId`'s string), if it is connected.
pub fn device(id: &str) -> Option<cpal::Device> {
    cpal::default_host()
        .input_devices()
        .ok()?
        .find(|device| device.id().is_ok_and(|d| d.id() == id))
}

/// Opens `device` at its default input format and starts it feeding `sink`.
pub fn open_device(
    device: &cpal::Device,
    sink: Sink,
    log: Option<fn(&str)>,
) -> Result<cpal::Stream> {
    let supported = device.default_input_config()?;
    let config = supported.config();
    if let Some(log) = log {
        log(&format!(
            "Microphone: {} Hz, {} ch, {:?}",
            config.sample_rate,
            config.channels,
            supported.sample_format()
        ));
    }
    ensure!(
        config.channels > 0 && config.sample_rate > 0,
        "Invalid microphone format"
    );
    macro_rules! stream {
        ($ty:ty) => {
            input_stream::<$ty>(&device, config, sink.clone())?
        };
    }
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => stream!(f32),
        cpal::SampleFormat::F64 => stream!(f64),
        cpal::SampleFormat::I8 => stream!(i8),
        cpal::SampleFormat::I16 => stream!(i16),
        cpal::SampleFormat::I24 => stream!(cpal::I24),
        cpal::SampleFormat::I32 => stream!(i32),
        cpal::SampleFormat::I64 => stream!(i64),
        cpal::SampleFormat::U8 => stream!(u8),
        cpal::SampleFormat::U16 => stream!(u16),
        cpal::SampleFormat::U24 => stream!(cpal::U24),
        cpal::SampleFormat::U32 => stream!(u32),
        cpal::SampleFormat::U64 => stream!(u64),
        format => bail!("Unsupported microphone format: {format}"),
    };
    stream.play().context("Could not start microphone")?;
    Ok(stream)
}

/// The device's name ("MacBook Pro Microphone").
pub fn device_name(device: &cpal::Device) -> Option<String> {
    Some(device.description().ok()?.name().to_owned())
}

fn input_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    sink: Sink,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = usize::from(config.channels);
    let mut downmix = Downmix::new(config.sample_rate);
    let mut frame_f32 = Vec::with_capacity(channels);
    let samples = Arc::clone(&sink.samples);
    Ok(device.build_input_stream::<T, _, _>(
        config,
        move |data, _| {
            let Ok(mut output) = samples.lock() else {
                return;
            };
            for frame in data.chunks_exact(channels) {
                frame_f32.clear();
                frame_f32.extend(frame.iter().map(|sample| sample.to_sample::<f32>()));
                downmix.push(&frame_f32, &mut output);
            }
        },
        move |err| {
            // A buffer under- or overrun drops a few samples; the recording goes on.
            if err.kind() == cpal::ErrorKind::Xrun {
                return;
            }
            sink.fail(err.to_string());
        },
        None,
    )?)
}
