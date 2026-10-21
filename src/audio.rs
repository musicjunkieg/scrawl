use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub use scrawl_capture::audio::{
    AudioCapture, Downmix, RecordedAudio, Recorder, Sink, decode_wav_16k, encode_wav_16k, pcm16,
    recorded_audio_from_wav, speech_islands,
};

/// Starts recording the chosen microphone (`mic`), else the default, with live transcription
/// when it is on (`live_transcribe`).
pub fn start_recorder() -> Result<Recorder> {
    let started = std::time::Instant::now();
    let mut recorder = Recorder::start(
        "scrawl-audio",
        move |sink| {
            let stream = open_stream(sink).inspect_err(|error| {
                crate::capture::scrawl_log(format!("Microphone startup failed: {error:#}"));
            })?;
            crate::ui::start_metric("audio_ready_ms", started);
            Ok(stream)
        },
        Some(|line| crate::capture::scrawl_log(line)),
    )?;
    let live = crate::live_transcribe::wanted()
        .then(|| crate::live_transcribe::Live::start(recorder.samples()))
        .and_then(|live| {
            live.inspect_err(|error| {
                crate::capture::scrawl_log(format!("Live transcription: {error:#}"));
            })
            .ok()
        });
    recorder.set_live(crate::transcribe::live_slot(live));
    Ok(recorder)
}

/// The chosen microphone (`mic`), else the macOS default. A chosen device that is missing or
/// won't open never stops the recording.
fn open_stream(sink: Sink) -> Result<cpal::Stream> {
    let choice = crate::mic::choice();
    if let Some(choice) = &choice {
        if let Some(device) = crate::mic::device(&choice.id) {
            let input = crate::mic::Input {
                id: choice.id.clone(),
                name: device_name(&device).unwrap_or_else(|| choice.name.clone()),
            };
            crate::mic::note_pick(&crate::mic::Pick::Chosen(&input));
            match open_device(&device, sink.clone()) {
                Ok(stream) => {
                    crate::capture::scrawl_log(format!("Microphone: {}", input.name));
                    return Ok(stream);
                }
                Err(e) => crate::capture::scrawl_log(format!(
                    "Microphone: could not open {} ({e:#}); using the default",
                    input.name
                )),
            }
        } else {
            let default = cpal::default_host()
                .default_input_device()
                .and_then(|device| {
                    Some(crate::mic::Input {
                        id: device.id().ok()?.id().to_owned(),
                        name: device_name(&device)?,
                    })
                });
            let inputs = crate::mic::Inputs {
                default: default.as_ref().map(|d| d.id.clone()),
                devices: default.into_iter().collect(),
            };
            crate::mic::note_pick(&crate::mic::Pick::new(Some(choice), &inputs));
        }
    }
    let device = cpal::default_host()
        .default_input_device()
        .context("No microphone is available")?;
    let stream = open_device(&device, sink)?;
    crate::capture::scrawl_log(format!(
        "Microphone: {} (default)",
        device_name(&device).unwrap_or_default()
    ));
    Ok(stream)
}

fn open_device(device: &cpal::Device, sink: Sink) -> Result<cpal::Stream> {
    let supported = device.default_input_config()?;
    let config = supported.config();
    crate::capture::scrawl_log(format!(
        "Microphone: {} Hz, {} ch, {:?}",
        config.sample_rate,
        config.channels,
        supported.sample_format()
    ));
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

fn device_name(device: &cpal::Device) -> Option<String> {
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
