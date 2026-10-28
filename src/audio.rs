use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};

pub use scrawl_capture::audio::{
    AudioCapture, RecordedAudio, Recorder, Sink, decode_wav_16k, decode_wav_pcm16, encode_wav_16k,
    pcm16, recorded_audio_from_wav, speech_islands,
};
// Opening a device into a `Sink` is `scrawl-capture`'s (`native::mic`).
use scrawl_capture::native::mic::{device_name, open_device};

fn log(line: &str) {
    crate::capture::scrawl_log(line);
}

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
        Some(log),
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
            match open_device(&device, sink.clone(), Some(log)) {
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
    let stream = open_device(&device, sink, Some(log))?;
    crate::capture::scrawl_log(format!(
        "Microphone: {} (default)",
        device_name(&device).unwrap_or_default()
    ));
    Ok(stream)
}
