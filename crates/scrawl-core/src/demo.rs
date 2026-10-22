//! A deterministic demo capture (synthetic OpenCode frames, a click, a scroll, ink and timed
//! speech), built into a bundle: what `scrawl demo` shows, and the input of the golden test
//! that pins `scrawl.json` and `summary.md`.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::build::{BuildOptions, BuiltBundle, SourceExtras};
use crate::model::*;
use crate::render::render_synthetic_demo_frame;

/// Builds the demo bundle in `bundle_dir`.
pub fn demo_bundle(
    bundle_dir: &Path,
    options: &BuildOptions,
) -> Result<(PathBuf, ScrawlBundle, String)> {
    demo_capture(|target, frames, strokes, events, transcript, note| {
        crate::build::build_bundle_from_parts(
            bundle_dir,
            target,
            9_800,
            frames,
            strokes,
            events,
            transcript,
            None,
            note,
            SourceExtras::default(),
            options,
        )
    })
}

/// The demo bundle built in memory (`build::build_bundle`), as if it lived in `bundle_dir`.
pub fn demo_built(bundle_dir: &Path, options: &BuildOptions) -> Result<BuiltBundle> {
    demo_capture(|target, frames, strokes, events, transcript, note| {
        crate::build::build_bundle(
            bundle_dir,
            "demo".into(),
            target,
            9_800,
            frames,
            strokes,
            events,
            transcript,
            None,
            None,
            note,
            SourceExtras::default(),
            options,
        )
    })
}

/// The demo capture's parts, handed to `build`: target, frames, ink, events, transcript, note.
fn demo_capture<T>(
    build: impl FnOnce(
        TargetInfo,
        Vec<RgbaFrame>,
        Vec<Stroke>,
        Vec<InteractionEvent>,
        TranscriptData,
        Option<String>,
    ) -> Result<T>,
) -> Result<T> {
    let (fw, fh) = (1280_u32, 800_u32);
    let target = TargetInfo {
        mode: CaptureMode::Window,
        app: Some("OpenCode".into()),
        bundle_id: Some("ai.opencode.desktop".into()),
        window_title: Some("session/tool-running — OpenCode".into()),
        window_id: Some(4210),
        display_id: 1,
        frame: TargetFrame {
            x: 100.0,
            y: 60.0,
            width: f64::from(fw),
            height: f64::from(fh),
            scale: 2.0,
        },
    };

    let frames = vec![
        render_synthetic_demo_frame(fw, fh, 400, 0),
        render_synthetic_demo_frame(fw, fh, 1_500, 1),
        render_synthetic_demo_frame(fw, fh, 3_600, 1),
        render_synthetic_demo_frame(fw, fh, 6_400, 2),
        render_synthetic_demo_frame(fw, fh, 9_200, 2),
    ];

    // 1. Circle around the shifted header band (cx=0.52, cy=0.14, r_x=0.14, r_y=0.06)
    let mut circle_pts = Vec::new();
    for i in 0..=36 {
        let angle = (i as f32 / 36.0) * std::f32::consts::TAU;
        let x = ((0.52 + 0.13 * angle.cos()) * SCALE as f32).round() as i32;
        let y = ((0.15 + 0.055 * angle.sin()) * SCALE as f32).round() as i32;
        circle_pts.extend_from_slice(&[x, y, i * 18]);
    }
    let circle_stroke = Stroke {
        t0: 2_900,
        t1: 2_900 + 36 * 18,
        fw,
        fh,
        on: Some("Band header".into()),
        pin: None,
        laser: false,
        pts: circle_pts,
    };

    // 2. Arrow pointing from sidebar item to the tool card (shaft + arrowhead)
    let mut shaft_pts = Vec::new();
    for i in 0..=12 {
        let t = i as f32 / 12.0;
        let x = ((0.22 + (0.44 - 0.22) * t) * SCALE as f32).round() as i32;
        let y = ((0.48 + (0.36 - 0.48) * t) * SCALE as f32).round() as i32;
        shaft_pts.extend_from_slice(&[x, y, i * 16]);
    }
    let arrow_shaft = Stroke {
        t0: 5_800,
        t1: 5_800 + 12 * 16,
        fw,
        fh,
        on: Some("Used 12 tools".into()),
        pin: None,
        laser: false,
        pts: shaft_pts,
    };
    let arrow_head = Stroke {
        t0: 6_100,
        t1: 6_160,
        fw,
        fh,
        on: Some("Used 12 tools".into()),
        pin: None,
        laser: false,
        pts: vec![
            (0.41 * SCALE as f32) as i32,
            (0.35 * SCALE as f32) as i32,
            0,
            (0.44 * SCALE as f32) as i32,
            (0.36 * SCALE as f32) as i32,
            30,
            (0.42 * SCALE as f32) as i32,
            (0.40 * SCALE as f32) as i32,
            60,
        ],
    };

    // 3. Numbered pin on the composer send button
    let pin_stroke = Stroke {
        t0: 8_900,
        t1: 8_900,
        fw,
        fh,
        on: Some("Send button".into()),
        pin: Some(1),
        laser: false,
        pts: vec![
            (0.88 * SCALE as f32) as i32,
            (0.88 * SCALE as f32) as i32,
            0,
        ],
    };

    let strokes = vec![circle_stroke, arrow_shaft, arrow_head, pin_stroke];
    let events = vec![
        InteractionEvent::Click {
            t: 1_250,
            x: 0.46,
            y: 0.36,
            button: Some("left".into()),
            label: Some("Used 12 tools".into()),
            role: Some("button".into()),
        },
        InteractionEvent::Scroll {
            t: 4_800,
            x: 0.55,
            y: 0.55,
            dx: 0.0,
            dy: -240.0,
            label: Some("Transcript".into()),
        },
    ];

    let transcript = TranscriptData {
        source: "speech-analyzer".into(),
        model: Some("SpeechTranscriber en_US".into()),
        text: "When I click Used 12 tools this band header jumps down by four pixels, and the row spacing under the card feels too tight.".into(),
        segments: vec![
            TranscriptSegment {
                t0: 600,
                t1: 4_100,
                text: "When I click Used 12 tools this band header jumps down by four pixels,".into(),
            },
            TranscriptSegment {
                t0: 5_400,
                t1: 9_400,
                text: "and the row spacing under the card feels too tight.".into(),
            },
        ],
        words: vec![
            WordTiming { t0: 600, t1: 850, text: "When".into() },
            WordTiming { t0: 850, t1: 980, text: "I".into() },
            WordTiming { t0: 980, t1: 1_320, text: "click".into() },
            WordTiming { t0: 1_320, t1: 1_650, text: "Used".into() },
            WordTiming { t0: 1_650, t1: 1_900, text: "12".into() },
            WordTiming { t0: 1_900, t1: 2_300, text: "tools".into() },
            WordTiming { t0: 2_450, t1: 2_700, text: "this".into() },
            WordTiming { t0: 2_700, t1: 3_050, text: "band".into() },
            WordTiming { t0: 3_050, t1: 3_400, text: "header".into() },
            WordTiming { t0: 3_400, t1: 3_700, text: "jumps".into() },
            WordTiming { t0: 3_700, t1: 3_900, text: "down".into() },
            WordTiming { t0: 3_900, t1: 4_100, text: "by four pixels,".into() },
            WordTiming { t0: 5_400, t1: 5_800, text: "and the".into() },
            WordTiming { t0: 5_800, t1: 6_300, text: "row spacing".into() },
            WordTiming { t0: 6_300, t1: 7_200, text: "under the card".into() },
            WordTiming { t0: 7_800, t1: 9_400, text: "feels too tight.".into() },
        ],
        estimated: false,
        missing: None,
    };

    build(
        target,
        frames,
        strokes,
        events,
        transcript,
        Some("Header inset shifts on expand".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The demo bundle's `scrawl.json` and `summary.md` against `tests/golden/<case>/`, so a
    /// refactor of the pipeline can't change what agents read without a test saying so. The
    /// creation time and the bundle's folder are blanked and floats rounded to 3 places, so the
    /// files are the same on every platform. `SCRAWL_UPDATE_GOLDEN=1 cargo test -p scrawl-core
    /// golden` rewrites them; review the diff before committing it.
    /// `build::build_bundle` hands back exactly the files `build_bundle_from_parts` writes.
    #[test]
    fn the_in_memory_build_holds_the_files_written_to_disk() {
        let root = std::env::temp_dir().join(format!("scrawl-in-memory-{}", std::process::id()));
        let dir = root.join("demo.scrawl");
        let options = BuildOptions::default();
        demo_bundle(&dir, &options).unwrap();
        let built = demo_built(&dir, &options).unwrap();
        let names: Vec<&str> = built.files.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names.first(), Some(&"contact-sheet.png"));
        assert_eq!(names[names.len() - 2..], ["scrawl.json", "summary.md"]);
        let mut on_disk = 0;
        for entry in std::fs::read_dir(dir.join("frames")).unwrap() {
            on_disk += usize::from(entry.unwrap().file_type().unwrap().is_file());
        }
        assert_eq!(built.files.len(), on_disk + 3);
        for (name, bytes) in &built.files {
            assert!(
                &std::fs::read(dir.join(name)).unwrap() == bytes,
                "{name} differs"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn demo_bundle_matches_the_golden_files() {
        let mut differs = Vec::new();
        for (case, agent_context) in [("demo", false), ("demo-agent-context", true)] {
            let root =
                std::env::temp_dir().join(format!("scrawl-golden-{}-{case}", std::process::id()));
            let dir = root.join("demo.scrawl");
            let options = BuildOptions {
                agent_context,
                ..BuildOptions::default()
            };
            demo_bundle(&dir, &options).unwrap();
            let json: serde_json::Value =
                serde_json::from_slice(&std::fs::read(dir.join("scrawl.json")).unwrap()).unwrap();
            let json = serde_json::to_string_pretty(&golden_json(json)).unwrap() + "\n";
            let summary = std::fs::read_to_string(dir.join("summary.md")).unwrap();
            for (file, actual) in [("scrawl.json", json), ("summary.md", summary)] {
                differs.extend(check_golden(case, file, &golden_paths(&actual, &dir)));
            }
            std::fs::remove_dir_all(root).unwrap();
        }
        assert!(differs.is_empty(), "{}", differs.join("\n"));
    }

    fn golden_json(value: serde_json::Value) -> serde_json::Value {
        use serde_json::Value;
        match value {
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(k, v)| match k.as_str() {
                        "createdAtMs" => (k, Value::from(0)),
                        _ => (k, golden_json(v)),
                    })
                    .collect(),
            ),
            Value::Array(items) => Value::Array(items.into_iter().map(golden_json).collect()),
            Value::Number(n) if n.is_f64() => {
                let rounded = (n.as_f64().unwrap() * 1000.0).round() / 1000.0;
                // `+ 0.0` turns -0.0 into 0.0.
                serde_json::Number::from_f64(rounded + 0.0).map_or(Value::Null, Value::Number)
            }
            other => other,
        }
    }

    /// The bundle's folder as `<bundle>`, with `/` between the parts of paths inside it on every
    /// platform (Windows writes `\`, which `scrawl.json` escapes as `\\`).
    fn golden_paths(text: &str, dir: &Path) -> String {
        let mut out = text.to_owned();
        let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        for form in [canonical.display().to_string(), dir.display().to_string()] {
            out = out.replace(&form.replace('\\', "\\\\"), "<bundle>");
            out = out.replace(&form, "<bundle>");
        }
        out.lines()
            .map(|line| {
                if line.contains("<bundle>") {
                    line.replace("\\\\", "/").replace('\\', "/")
                } else {
                    line.to_owned()
                }
            })
            .map(|line| line + "\n")
            .collect()
    }

    /// `None` when `actual` matches the golden file, else a message with all of `actual`
    /// between markers, so a run that can't write the files still shows what they should hold.
    fn check_golden(case: &str, file: &str, actual: &str) -> Option<String> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/golden")
            .join(case)
            .join(file);
        if std::env::var_os("SCRAWL_UPDATE_GOLDEN").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, actual).unwrap();
            return None;
        }
        let expected = std::fs::read_to_string(&path).unwrap_or_default();
        (expected != actual).then(|| {
            format!(
                "{case}/{file} differs from {} (SCRAWL_UPDATE_GOLDEN=1 rewrites it)\n\
                 ----- actual {case}/{file} -----\n{actual}----- end {case}/{file} -----",
                path.display()
            )
        })
    }
}
