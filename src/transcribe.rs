//! Transcription: Apple's SpeechAnalyzer through `scrawl-helper` (measured word and segment
//! timings), or Hex when `"transcriber": "hex"` is set and Hex is connected (`crate::hex`; word
//! times estimated). Any Hex trouble falls back to Apple for that recording.

use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};

use anyhow::Result;

use crate::audio::RecordedAudio;
use crate::model::TranscriptData;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Engine {
    #[default]
    Apple,
    Hex,
}

impl Engine {
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "apple" => Some(Self::Apple),
            "hex" => Some(Self::Hex),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Apple => "apple",
            Self::Hex => "hex",
        }
    }

    /// The configured engine: `"transcriber"` in the config file, Apple when missing or unknown.
    pub fn configured() -> Self {
        crate::handoff::transcriber()
            .as_deref()
            .and_then(Self::parse)
            .unwrap_or_default()
    }

    /// What recordings use now: a command-line `--transcriber`, else the configured engine.
    pub fn preferred() -> Self {
        match OVERRIDE.load(Ordering::Relaxed) {
            1 => Self::Apple,
            2 => Self::Hex,
            _ => Self::configured(),
        }
    }
}

/// The on-device engine's name in menus: Apple's SpeechAnalyzer on macOS; on Windows, Parakeet
/// through `transcribe.cpp` (`crate::win::speech`). Both are `Engine::Apple` in the config.
#[cfg(target_os = "macos")]
pub const LOCAL_ENGINE_TITLE: &str = "Apple Speech";
#[cfg(not(target_os = "macos"))]
pub const LOCAL_ENGINE_TITLE: &str = "Parakeet (on this PC)";

static OVERRIDE: AtomicU8 = AtomicU8::new(0);
static LOGGED_NOT_CONNECTED: std::sync::Once = std::sync::Once::new();

/// What the menu's Transcription submenu shows. A Hex preference stays saved while Hex is away,
/// but the tick follows what recordings would really use.
#[derive(Debug, PartialEq, Eq)]
pub struct MenuState {
    pub apple_checked: bool,
    pub hex_title: &'static str,
    pub hex_enabled: bool,
    pub hex_checked: bool,
    /// Ready Hex models as `(id, title, chosen)`, empty when Hex is away.
    pub models: Vec<(String, String, bool)>,
}

pub fn menu_state(
    preferred: Engine,
    status: Option<&crate::hex::Status>,
    configured_model: Option<&str>,
) -> MenuState {
    let Some(status) = status else {
        return MenuState {
            apple_checked: true,
            hex_title: "Hex (not running)",
            hex_enabled: false,
            hex_checked: false,
            models: Vec::new(),
        };
    };
    let chosen = crate::hex::pick_model(&status.models, configured_model);
    let models: Vec<_> = status
        .usable_models()
        .map(|model| {
            let title = if model.name.is_empty() {
                &model.id
            } else {
                &model.name
            };
            (
                model.id.clone(),
                title.clone(),
                chosen.as_deref() == Some(model.id.as_str()),
            )
        })
        .collect();
    let ready = chosen.is_some();
    MenuState {
        apple_checked: !(ready && preferred == Engine::Hex),
        hex_title: if ready { "Hex" } else { "Hex (no model ready)" },
        hex_enabled: ready,
        hex_checked: ready && preferred == Engine::Hex,
        models,
    }
}

/// Pins the engine for this process (`scrawl reprocess --transcriber`).
pub fn set_override(engine: Engine) {
    OVERRIDE.store(
        match engine {
            Engine::Apple => 1,
            Engine::Hex => 2,
        },
        Ordering::Relaxed,
    );
}

/// The audio a transcriber hears: 16 kHz mono WAV, both on disk and in memory, so an engine
/// can take whichever it needs.
#[derive(Clone, Copy, Debug)]
pub struct Speech<'a> {
    pub wav_path: &'a Path,
    pub wav_bytes: &'a [u8],
    pub duration_ms: u64,
}

/// One engine that turns recorded speech into a transcript. Engines are tried in order
/// (`transcribe_with`); one that returns `Ok(None)` (not connected, no model ready) or an error
/// hands the recording to the next.
pub trait Transcriber {
    /// Names the engine in logs.
    fn name(&self) -> &str;
    fn transcribe(&mut self, speech: Speech<'_>) -> Result<Option<TranscriptData>>;
}

/// Missing microphones and a failed transcriber never prevent saving a bundle.
pub fn transcribe(wav: Option<&Path>, audio: Option<&RecordedAudio>) -> TranscriptData {
    let (Some(wav), Some(audio)) = (wav, audio.filter(|audio| audio.sample_count > 0)) else {
        return TranscriptData {
            source: "none".into(),
            ..Default::default()
        };
    };
    let speech = Speech {
        wav_path: wav,
        wav_bytes: &audio.wav_bytes,
        duration_ms: audio.duration_ms,
    };
    let mut hex = Hex;
    let mut live = Live(audio.live.take());
    let mut local = Local;
    let mut engines: Vec<&mut dyn Transcriber> = Vec::with_capacity(3);
    if Engine::preferred() == Engine::Hex {
        engines.push(&mut hex);
    }
    engines.push(&mut live);
    engines.push(&mut local);
    transcribe_with(&mut engines, speech)
}

/// Asks each engine in turn and keeps the first transcript. If none answers, the bundle says why.
pub fn transcribe_with(engines: &mut [&mut dyn Transcriber], speech: Speech<'_>) -> TranscriptData {
    let mut failure = None;
    for index in 0..engines.len() {
        match engines[index].transcribe(speech) {
            Ok(Some(transcript)) => return transcript,
            Ok(None) => {}
            Err(error) => {
                let name = engines[index].name().to_owned();
                let next = engines.get(index + 1).map_or("nothing", |next| next.name());
                crate::capture::scrawl_log(format!(
                    "{name} transcription failed, using {next}: {error:#}"
                ));
                failure = Some(format!("{name} failed ({error:#})."));
            }
        }
    }
    TranscriptData {
        source: "offline".into(),
        missing: Some(crate::model::MissingTranscript {
            reason: failure.unwrap_or_else(|| "No transcriber was available.".into()),
            retry: false,
        }),
        ..Default::default()
    }
}

/// Hex (`crate::hex`), when it is connected and has a model ready.
pub struct Hex;

impl Transcriber for Hex {
    fn name(&self) -> &str {
        "Hex"
    }

    fn transcribe(&mut self, speech: Speech<'_>) -> Result<Option<TranscriptData>> {
        let not_connected = |reason: String| {
            LOGGED_NOT_CONNECTED.call_once(|| {
                crate::capture::scrawl_log(format!(
                    "Hex preferred but unavailable ({reason}); using {LOCAL_ENGINE_TITLE}"
                ));
            });
            Ok(None)
        };
        let client = match crate::hex::Client::connect() {
            Ok(client) => client,
            Err(error) => return not_connected(format!("{error:#}")),
        };
        let models = client.models()?;
        let configured = crate::handoff::hex_model();
        let Some(model) = crate::hex::pick_model(&models, configured.as_deref()) else {
            return not_connected("no ready model".into());
        };
        crate::hex::transcribe_audio(&client, &model, speech.wav_bytes).map(Some)
    }
}

/// The transcript made while recording (`crate::live_transcribe`), when one ran.
pub struct Live(pub Option<crate::live_transcribe::Live>);

impl Transcriber for Live {
    fn name(&self) -> &str {
        "Live"
    }

    fn transcribe(&mut self, speech: Speech<'_>) -> Result<Option<TranscriptData>> {
        let Some(live) = self.0.take() else {
            return Ok(None);
        };
        let done = live.finish(speech.wav_bytes)?;
        crate::capture::scrawl_log(format!(
            "Live transcription: {} words, {} of {} ms streamed, tail {} ms",
            done.transcript.words.len(),
            done.streamed_samples as u64 * 1_000 / 16_000,
            speech.duration_ms,
            done.tail_ms
        ));
        Ok(Some(done.transcript))
    }
}

/// The on-device engine (`LOCAL_ENGINE_TITLE`). It always answers: a failure becomes a
/// transcript that says why it is missing.
pub struct Local;

impl Transcriber for Local {
    fn name(&self) -> &str {
        LOCAL_ENGINE_TITLE
    }

    fn transcribe(&mut self, speech: Speech<'_>) -> Result<Option<TranscriptData>> {
        let transcript = crate::helper::transcribe(speech.wav_path).unwrap_or_else(|error| {
            crate::capture::scrawl_log(format!("{LOCAL_ENGINE_TITLE} failed: {error:#}"));
            TranscriptData {
                source: "offline".into(),
                missing: Some(crate::model::MissingTranscript {
                    reason: format!("Apple Speech failed ({error:#})."),
                    retry: false,
                }),
                ..Default::default()
            }
        });
        if let Some(missing) = &transcript.missing {
            crate::capture::scrawl_log(format!("No transcript: {}", missing.reason));
            if missing.retry {
                crate::speech_model::ensure();
            }
        }
        Ok(Some(transcript))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(source: &str) -> TranscriptData {
        TranscriptData {
            source: source.into(),
            ..Default::default()
        }
    }

    #[test]
    fn engine_names_parse_and_unknown_is_not_an_engine() {
        assert_eq!(Engine::parse("hex"), Some(Engine::Hex));
        assert_eq!(Engine::parse(" Apple "), Some(Engine::Apple));
        assert_eq!(Engine::parse("whisper"), None);
        assert_eq!(Engine::default(), Engine::Apple);
    }

    /// A scripted engine: answers with `answer` and counts how often it was asked.
    struct Fake {
        answer: fn() -> Result<Option<TranscriptData>>,
        asked: usize,
    }

    impl Fake {
        fn new(answer: fn() -> Result<Option<TranscriptData>>) -> Self {
            Self { answer, asked: 0 }
        }
    }

    impl Transcriber for Fake {
        fn name(&self) -> &str {
            "Fake"
        }

        fn transcribe(&mut self, _: Speech<'_>) -> Result<Option<TranscriptData>> {
            self.asked += 1;
            (self.answer)()
        }
    }

    fn speech() -> Speech<'static> {
        Speech {
            wav_path: Path::new("audio.wav"),
            wav_bytes: &[],
            duration_ms: 1_000,
        }
    }

    #[test]
    fn the_first_engine_that_answers_wins() {
        let mut hex = Fake::new(|| Ok(Some(named("hex:parakeet_v3"))));
        let mut local = Fake::new(|| Ok(Some(named("speech-analyzer"))));
        let result = transcribe_with(&mut [&mut hex, &mut local], speech());
        assert_eq!(result.source, "hex:parakeet_v3");
        assert_eq!((hex.asked, local.asked), (1, 0));
    }

    #[test]
    fn unavailable_or_failing_engines_fall_through_to_the_next() {
        let mut away = Fake::new(|| Ok(None));
        let mut failing = Fake::new(|| Err(anyhow::anyhow!("Hex took too long")));
        let mut local = Fake::new(|| Ok(Some(named("speech-analyzer"))));
        let result = transcribe_with(&mut [&mut away, &mut failing, &mut local], speech());
        assert_eq!(result.source, "speech-analyzer");
        assert_eq!((away.asked, failing.asked, local.asked), (1, 1, 1));
    }

    #[test]
    fn no_answer_is_a_missing_transcript_naming_the_last_failure() {
        let mut failing = Fake::new(|| Err(anyhow::anyhow!("busy")));
        let mut away = Fake::new(|| Ok(None));
        let result = transcribe_with(&mut [&mut failing, &mut away], speech());
        let missing = result.missing.expect("missing");
        assert_eq!(missing.reason, "Fake failed (busy).");
        assert!(!missing.retry);
        let none = transcribe_with(&mut [], speech());
        assert_eq!(
            none.missing.unwrap().reason,
            "No transcriber was available."
        );
    }

    #[test]
    fn an_empty_live_slot_passes_without_finishing_anything() {
        assert!(Live(None).transcribe(speech()).unwrap().is_none());
    }

    #[test]
    fn menu_follows_what_recordings_would_use() {
        use crate::hex::{Model, Status};
        let status = |models: Vec<Model>| Status {
            version: "2.2.0".into(),
            models,
        };
        let ready = |id: &str, name: &str| Model {
            id: id.into(),
            name: name.into(),
            installed: true,
            verified: true,
        };

        let away = menu_state(Engine::Hex, None, None);
        assert!(away.apple_checked && !away.hex_checked && !away.hex_enabled);
        assert_eq!(away.hex_title, "Hex (not running)");

        let up = status(vec![
            ready("parakeet_v3", "Parakeet v3"),
            ready("qwen3_asr06_b", ""),
        ]);
        let hex = menu_state(Engine::Hex, Some(&up), Some("qwen3_asr06_b"));
        assert!(hex.hex_checked && !hex.apple_checked && hex.hex_enabled);
        assert_eq!(
            hex.models,
            vec![
                ("parakeet_v3".into(), "Parakeet v3".into(), false),
                ("qwen3_asr06_b".into(), "qwen3_asr06_b".into(), true),
            ]
        );
        let apple = menu_state(Engine::Apple, Some(&up), None);
        assert!(apple.apple_checked && !apple.hex_checked && apple.hex_enabled);

        let empty = status(vec![Model {
            id: "parakeet_v3".into(),
            name: String::new(),
            installed: false,
            verified: false,
        }]);
        let none = menu_state(Engine::Hex, Some(&empty), None);
        assert!(none.apple_checked && !none.hex_enabled && none.models.is_empty());
        assert_eq!(none.hex_title, "Hex (no model ready)");
    }

    #[test]
    fn no_audio_is_none_without_asking_any_engine() {
        assert_eq!(transcribe(None, None).source, "none");
    }
}
