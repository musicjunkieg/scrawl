//! The narration: one recording in progress, the engine behind the app's microphone recorder,
//! and the 16 kHz mono WAV every recording is kept as.

use std::io::Cursor;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use anyhow::{Context, Result, anyhow, bail, ensure};

/// Every recording is 16 kHz mono.
pub const SAMPLE_RATE: u32 = 16_000;

/// A finished recording of the narration: 16 kHz mono WAV.
#[derive(Clone, Debug)]
pub struct RecordedAudio {
    pub sample_count: usize,
    pub duration_ms: u64,
    pub wav_bytes: Vec<u8>,
    /// The engine that transcribed while recording, if one did.
    pub live: crate::transcribe::Slot,
}

/// One recording of the narration in progress. `Recorder` records whatever input its `open`
/// function starts; another source only has to hand back 16 kHz mono WAV when stopped.
pub trait AudioCapture: Send {
    fn stop(self: Box<Self>) -> Result<RecordedAudio>;
}

/// Where an input's callback puts what it hears: 16 kHz mono samples, and the error that
/// ended the input early, if one did.
#[derive(Clone, Default)]
pub struct Sink {
    pub samples: Arc<Mutex<Vec<f32>>>,
    pub error: Arc<Mutex<Option<String>>>,
}

impl Sink {
    /// Records why the input stopped; what it heard before stays.
    pub fn fail(&self, message: String) {
        if let Ok(mut error) = self.error.lock() {
            *error = Some(message);
        }
    }
}

/// Turns an input's frames (any channel count and rate) into 16 kHz mono by area averaging: a
/// continuous-phase downmix and decimator, including non-integer ratios such as 44.1 kHz to
/// 16 kHz.
#[derive(Clone, Debug)]
pub struct Downmix {
    rate: u32,
    phase: u32,
    sum: f64,
}

impl Downmix {
    /// For an input at `rate` Hz (more than 0).
    pub fn new(rate: u32) -> Self {
        Self {
            rate,
            phase: 0,
            sum: 0.0,
        }
    }

    /// Adds one frame (a sample per channel) and appends any 16 kHz samples it completes.
    pub fn push(&mut self, frame: &[f32], output: &mut Vec<f32>) {
        let mono = frame.iter().map(|&sample| f64::from(sample)).sum::<f64>() / frame.len() as f64;
        let mono = if mono.is_finite() { mono } else { 0.0 };
        let mut remaining = SAMPLE_RATE;
        while remaining > 0 {
            let weight = remaining.min(self.rate - self.phase);
            self.sum += mono * f64::from(weight);
            self.phase += weight;
            remaining -= weight;
            if self.phase == self.rate {
                output.push((self.sum / f64::from(self.rate)) as f32);
                self.phase = 0;
                self.sum = 0.0;
            }
        }
    }
}

/// A recording from an input that runs on its own thread (the app's microphone, through
/// `cpal`). The input is opened, kept and closed on that thread, so it never has to be `Send`.
pub struct Recorder {
    worker: Option<JoinHandle<Result<()>>>,
    stop_sender: Option<Sender<()>>,
    sink: Sink,
    live: crate::transcribe::Slot,
    log: Option<fn(&str)>,
}

impl Recorder {
    /// Runs `open` on a new thread, named `thread_name`. It starts the input feeding `Sink` and
    /// returns what keeps the input running, which the thread drops when the recording stops.
    /// `log` hears about an input that failed partway.
    pub fn start<G: 'static>(
        thread_name: &str,
        open: impl FnOnce(Sink) -> Result<G> + Send + 'static,
        log: Option<fn(&str)>,
    ) -> Result<Self> {
        let sink = Sink::default();
        let (stop_sender, stop_receiver) = std::sync::mpsc::channel::<()>();
        let worker_sink = sink.clone();
        let worker = std::thread::Builder::new()
            .name(thread_name.into())
            .spawn(move || {
                let input = open(worker_sink)?;
                // The input never crosses threads. Disconnect also stops abandoned recorders.
                let _ = stop_receiver.recv();
                drop(input);
                Ok(())
            })
            .context("Could not spawn the audio worker")?;
        Ok(Self {
            worker: Some(worker),
            stop_sender: Some(stop_sender),
            sink,
            live: Default::default(),
            log,
        })
    }

    /// The samples heard so far, for an engine that transcribes while recording.
    pub fn samples(&self) -> Arc<Mutex<Vec<f32>>> {
        Arc::clone(&self.sink.samples)
    }

    /// The engine transcribing while recording, handed on with the recording.
    pub fn set_live(&mut self, live: crate::transcribe::Slot) {
        self.live = live;
    }

    fn join_worker(&mut self) -> Result<()> {
        self.stop_sender.take();
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| anyhow!("Microphone worker panicked"))??;
        }
        Ok(())
    }

    pub fn stop(mut self) -> Result<RecordedAudio> {
        // Joining drops the input and stops its callback before taking the buffer,
        // including a stop requested while the device was still being initialized.
        self.join_worker()?;
        let error = self
            .sink
            .error
            .lock()
            .map_err(|_| anyhow!("Microphone error lock poisoned"))?
            .take();
        let samples_16k = std::mem::take(
            &mut *self
                .sink
                .samples
                .lock()
                .map_err(|_| anyhow!("Microphone buffer lock poisoned"))?,
        );
        // An overload, or a device unplugged partway, keeps what was heard before it.
        if let Some(error) = error {
            if samples_16k.is_empty() {
                bail!("Microphone capture failed: {error}");
            }
            if let Some(log) = self.log {
                log(&format!("Microphone: {error}; keeping what it heard"));
            }
        }
        let sample_count = samples_16k.len();
        let duration_ms = sample_count as u64 * 1_000 / u64::from(SAMPLE_RATE);
        let wav_bytes = encode_wav_16k(&samples_16k)?;
        Ok(RecordedAudio {
            sample_count,
            duration_ms,
            wav_bytes,
            live: std::mem::take(&mut self.live),
        })
    }
}

impl AudioCapture for Recorder {
    fn stop(self: Box<Self>) -> Result<RecordedAudio> {
        Recorder::stop(*self)
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.join_worker();
    }
}

/// Reads a 16 kHz mono WAV (as written by `scrawl-helper audio`) back into a recording.
pub fn recorded_audio_from_wav(wav_bytes: Vec<u8>) -> Result<RecordedAudio> {
    let reader = hound::WavReader::new(Cursor::new(&wav_bytes))?;
    let spec = reader.spec();
    ensure!(
        spec.sample_rate == SAMPLE_RATE && spec.channels == 1 && spec.bits_per_sample == 16,
        "Expected 16 kHz mono 16-bit audio"
    );
    let sample_count = reader.duration() as usize;
    Ok(RecordedAudio {
        sample_count,
        duration_ms: sample_count as u64 * 1_000 / u64::from(SAMPLE_RATE),
        wav_bytes,
        live: Default::default(),
    })
}

/// Decodes a 16 kHz mono 16-bit WAV (this module's own format) into samples.
pub fn decode_wav_16k(wav_bytes: &[u8]) -> Result<Vec<f32>> {
    let reader = hound::WavReader::new(Cursor::new(wav_bytes))?;
    let spec = reader.spec();
    ensure!(
        spec.sample_rate == SAMPLE_RATE
            && spec.channels == 1
            && spec.bits_per_sample == 16
            && spec.sample_format == hound::SampleFormat::Int,
        "Expected 16 kHz mono 16-bit audio"
    );
    reader
        .into_samples::<i16>()
        .map(|sample| Ok(f32::from(sample?) / 32_768.0))
        .collect()
}

const ISLAND_WINDOW_MS: u64 = 50;
const ISLAND_MERGE_SILENCE_MS: u64 = 350;

/// Energy-based speech spans in milliseconds, quantized to 50 ms. These are not recognizer
/// timestamps. The speech threshold follows the recording's own noise floor (10th percentile of
/// window energy) so a quiet microphone and a noisy room both find their speech.
pub fn speech_islands(samples_16k: &[f32]) -> Vec<(u64, u64)> {
    let window = (u64::from(SAMPLE_RATE) * ISLAND_WINDOW_MS / 1_000) as usize;
    let rms: Vec<f64> = samples_16k
        .chunks(window)
        .map(|chunk| {
            (chunk
                .iter()
                .map(|&x| {
                    if x.is_finite() {
                        f64::from(x).powi(2)
                    } else {
                        0.0
                    }
                })
                .sum::<f64>()
                / chunk.len() as f64)
                .sqrt()
        })
        .collect();
    if rms.is_empty() {
        return Vec::new();
    }
    let mut sorted = rms.clone();
    sorted.sort_by(f64::total_cmp);
    let floor = sorted[sorted.len() / 10];
    let threshold = (floor * 3.0).clamp(0.005, 0.02);
    let mut islands: Vec<(u64, u64)> = Vec::new();
    for (index, &level) in rms.iter().enumerate() {
        if level <= threshold {
            continue;
        }
        let start = index as u64 * ISLAND_WINDOW_MS;
        let end = start + ISLAND_WINDOW_MS;
        match islands.last_mut() {
            Some(last) if start.saturating_sub(last.1) < ISLAND_MERGE_SILENCE_MS => last.1 = end,
            _ => islands.push((start, end)),
        }
    }
    let duration = samples_16k.len() as u64 * 1_000 / u64::from(SAMPLE_RATE);
    for island in &mut islands {
        island.1 = island.1.min(duration);
    }
    islands.retain(|&(a, b)| b > a);
    islands
}

/// One sample as `audio.wav` stores it.
pub fn pcm16(sample: f32) -> i16 {
    let sample = if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    (sample * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16
}

pub fn encode_wav_16k(samples_16k: &[f32]) -> Result<Vec<u8>> {
    let mut bytes = Cursor::new(Vec::with_capacity(44 + samples_16k.len() * 2));
    {
        let mut writer = hound::WavWriter::new(
            &mut bytes,
            hound::WavSpec {
                channels: 1,
                sample_rate: SAMPLE_RATE,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )?;
        let mut i16_writer = writer.get_i16_writer(samples_16k.len() as u32);
        for &sample in samples_16k {
            i16_writer.write_sample(pcm16(sample));
        }
        i16_writer.flush()?;
        writer.finalize()?;
    }
    Ok(bytes.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn recorder(
        worker: Option<JoinHandle<Result<()>>>,
        stop_sender: Option<Sender<()>>,
        sink: Sink,
    ) -> Recorder {
        Recorder {
            worker,
            stop_sender,
            sink,
            live: Default::default(),
            log: None,
        }
    }

    #[test]
    fn stop_joins_worker_before_reading_its_last_samples() {
        let sink = Sink::default();
        let output = Arc::clone(&sink.samples);
        let (stop_sender, stop_receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            // Model a final callback flushed while the worker drops its stream.
            assert!(matches!(
                stop_receiver.recv_timeout(Duration::from_secs(2)),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
            ));
            output.lock().unwrap().extend([0.1, 0.2, 0.3]);
            Ok(())
        });
        let audio = recorder(Some(worker), Some(stop_sender), sink)
            .stop()
            .unwrap();
        assert_eq!(audio.sample_count, 3);
        assert_eq!(
            hound::WavReader::new(Cursor::new(audio.wav_bytes))
                .unwrap()
                .duration(),
            3
        );
    }

    #[test]
    fn a_device_lost_partway_keeps_what_it_heard() {
        let lost = |samples: Vec<f32>| {
            let sink = Sink::default();
            *sink.samples.lock().unwrap() = samples;
            sink.fail("Device disconnected".into());
            recorder(None, None, sink)
        };
        assert_eq!(lost(vec![0.1; 160]).stop().unwrap().sample_count, 160);
        assert!(lost(Vec::new()).stop().is_err());
    }

    #[test]
    fn stop_propagates_background_initialization_failure() {
        let recorder = Recorder::start(
            "test-audio",
            |_| -> Result<()> { bail!("device unavailable") },
            None,
        )
        .unwrap();
        assert!(
            recorder
                .stop()
                .unwrap_err()
                .to_string()
                .contains("device unavailable")
        );
    }

    #[test]
    fn a_started_recorder_keeps_what_its_input_heard() {
        let mut recorder = Recorder::start(
            "test-audio",
            |sink| {
                sink.samples.lock().unwrap().extend([0.25; 1_600]);
                Ok(())
            },
            None,
        )
        .unwrap();
        recorder.set_live(Default::default());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while recorder.samples().lock().unwrap().is_empty() && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        let audio = recorder.stop().unwrap();
        assert_eq!((audio.sample_count, audio.duration_ms), (1_600, 100));
    }

    #[test]
    fn downmix_averages_channels_and_resamples_to_16k() {
        // 48 kHz stereo: every three frames make one sample, averaged over both channels.
        let mut downmix = Downmix::new(48_000);
        let mut output = Vec::new();
        for _ in 0..300 {
            downmix.push(&[0.5, 0.1], &mut output);
        }
        assert_eq!(output.len(), 100);
        assert!(output.iter().all(|&s| (s - 0.3).abs() < 1e-6));
        // 44.1 kHz keeps a continuous phase across the non-integer ratio.
        let mut downmix = Downmix::new(44_100);
        let mut output = Vec::new();
        for _ in 0..44_100 {
            downmix.push(&[1.0], &mut output);
        }
        assert_eq!(output.len(), 16_000);
        // A broken sample is silence, not NaN.
        let mut downmix = Downmix::new(16_000);
        let mut output = Vec::new();
        downmix.push(&[f32::NAN], &mut output);
        assert_eq!(output, vec![0.0]);
    }

    #[test]
    fn speech_islands_merge_only_short_silences() {
        let mut samples = vec![0.0; 32_000];
        samples[1_600..4_800].fill(0.2); // 100..300
        samples[8_800..12_800].fill(0.2); // 550..800: merge 250 ms gap
        samples[18_400..20_800].fill(0.2); // 1150..1300: keep 350 ms gap
        assert_eq!(speech_islands(&samples), vec![(100, 800), (1_150, 1_300)]);
        assert!(speech_islands(&[0.001; 800]).is_empty());
        assert!(speech_islands(&[]).is_empty());
    }

    #[test]
    fn speech_islands_adapt_to_a_noisy_floor() {
        let mut samples: Vec<f32> = (0..32_000)
            .map(|i| if i % 2 == 0 { 0.006 } else { -0.006 })
            .collect();
        samples[8_000..16_000]
            .iter_mut()
            .for_each(|s| *s = if *s > 0.0 { 0.15 } else { -0.15 });
        assert_eq!(speech_islands(&samples), vec![(500, 1_000)]);
    }

    #[test]
    fn decoded_wav_round_trips() {
        let wav = encode_wav_16k(&[0.0, 0.5, -0.5]).unwrap();
        assert_eq!(decode_wav_16k(&wav).unwrap(), vec![0.0, 0.5, -0.5]);
        assert!(decode_wav_16k(b"nope").is_err());
    }

    #[test]
    fn wav_is_mono_16k_pcm_and_clips_safely() {
        let bytes = encode_wav_16k(&[-2.0, -0.5, 0.0, 0.5, 2.0, f32::NAN]).unwrap();
        let mut reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(reader.spec().sample_format, hound::SampleFormat::Int);
        assert_eq!(
            reader
                .samples::<i16>()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap(),
            vec![-32_768, -16_384, 0, 16_384, 32_767, 0]
        );
    }
}
