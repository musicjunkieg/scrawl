# Bundle processing

## Goal
Reduce the time from recorded frames to a complete review bundle without changing decoded pixels, frame selection, or metadata.

## Benchmark
Build separately, then run `CARGO_TARGET_DIR=<isolated-target> nice -n 10 bun scripts/bench-processing.ts`. `SCRAWL_BIN` optionally selects a saved control binary; otherwise the script uses `$CARGO_TARGET_DIR/debug/scrawl` or `target/debug/scrawl`.
One warmup and seven measured subprocess runs of `scrawl demo --out <temporary-directory>`.
Primary metric: `bundle_processing_ms` (full demo wall-clock median). Secondary: MAD, min/max, pipeline stage medians, total PNG bytes.
The debug build is intentional and fixed throughout this loop. Demo includes synthetic frame rendering and process startup; `SCRAWL_BENCH=1` emits separate selection, annotation, contact-sheet, artifact-write and pipeline timings. No recording, transcription service, or clipboard access is involved.

Build: `CARGO_TARGET_DIR=<isolated-target> taskpolicy -c utility cargo build -j 3 --offline`.
Check: `CARGO_TARGET_DIR=<isolated-target> taskpolicy -c utility cargo test -j 3 --offline -- --test-threads=2`.
The October 11, 2026 run used branch `perf/processing`, base `3810ac8`, an isolated worktree and APFS-cloned target directory. Build/test never touched the main checkout's target or UI files.

## Scope
`crates/scrawl-core/src/render.rs`, `src/capture.rs`, `crates/scrawl-core/src/keyframes.rs`, `src/lib.rs`. UI and dependencies excluded.

## Hypotheses
1. Faster PNG settings reduce serial encoding cost.
2. Bounded parallel PNG encoding reduces the artifact critical path.
3. Thread-local CoreText font reuse reduces wrapping overhead.
4. Rasterizer interior fast paths and bounded annotation parallelism reduce CPU work.
5. Audio transcription can overlap frame preparation (requires a representative audio benchmark).

## Results
Baseline: first pass **3295.176 ms** (MAD 70.771, min 3175.561, max 5723.548). Repeated before optimization due to startup/system-load outliers: **3187.609 ms** (MAD 157.009, min 2806.437, max 3418.159). Pipeline median 2656.766 ms: selection 2.434, annotation 26.006, sheet 702.285, artifacts 1885.682. PNG total 494,314 bytes. Subsequent decisions require substantially exceeding this noise or confirmation with stage timings.

### H1 — fast PNG with Sub filter: keep
1234.413 ms (MAD 14.362, min 1218.897, max 1282.734), pipeline 895.761 ms, artifacts 308.836 ms. All 27 tests pass, including alpha round-trip. All five demo PNGs decode to identical RGBA pixels. Output grows from 494,314 to 1,079,182 bytes (2.18×); lossless encoding saves ~1.6 seconds of artifact time for ~571 KiB extra per demo bundle.

### H2 — bounded parallel PNG encoding: keep
1084.326 ms (MAD 25.575, min 1052.845, max 1144.351), pipeline 731.577 ms, artifacts 121.645 ms. Two scoped frame workers plus caller encoding the sheet; no frame-buffer clones. All 27 tests pass. All five decoded images, normalized manifests and summaries match baseline.

### H3 — CoreText font cache: discard
Initial cached run: 1013.887 ms (MAD 4.448), but the uncached control rerun rose to 1995.218 ms (MAD 153.230). Alternating cached/uncached runs were overwhelmed by host contention: uncached 6915.369 ms (MAD 4112.601), cached 6186.433 ms (MAD 2347.591). Even unrelated artifact encoding varied by >10×. No attributable, durable win; removed the bounded thread-local cache. All 27 tests passed with the cache. Capacity checks remained within the CPU gate, but utility-class work was heavily contended.

### H4a — opaque rounded-rectangle spans: keep
Fill only guaranteed fully covered opaque interior spans, retaining the existing SDF/blend path at edges and for transparency. Added byte-for-byte reference-rasterizer tests for fractional bounds, clipping, tiny rectangles, clamped radii, and alpha. 28 tests pass; all demo pixels and normalized metadata match baseline.

Utility-QoS measurements remained noisy: candidate 3642.167 ms (MAD 86.894), control 2092.217 (MAD 58.209), candidate repeat 1069.397 (MAD 141.745). Alternating controls: before 2390.047 (MAD 1208.685), after 1783.087 (MAD 679.694), 6/7 pairs improved. To avoid scheduling dominating the comparison, switched **benchmark only** to `nice -n 10 bun scripts/bench-processing.ts` (all builds/tests remain utility QoS, -j 3). Fresh same-scheduler comparison: **1039.142 → 743.164 ms**, MAD 15.429 → 3.802; contact sheet **550.291 → 412.557 ms**; pipeline **693.175 → 553.357 ms**. This also accelerates demo-only synthetic input generation, so use pipeline timing when estimating real post-recording gains.

### H4b — opaque blend shortcut: discard
743.164 → 741.051 ms (MAD 4.079, min 710.657, max 795.242). Pipeline 556.064 ms versus 553.357 before. No win beyond noise after rectangle spans already bypass blending. All 28 tests pass; removed the shortcut.

### H4c — skip hypot away from rounded corners: discard
743.164 → 734.990 ms (MAD 7.146, min 719.343, max 764.254), pipeline 545.984 ms. An ~8 ms shift is too close to the noise floor; removed the branch. All 28 tests pass.

### Stop / next hypotheses
The remaining micro-optimizations reached the noise floor. Annotation is ~26 ms; selection including luma is ~2.4 ms. More parallelism here has a small ceiling relative to its complexity. H5 (audio overlap) is deferred: demo uses a ready transcript and cannot measure transcription, and its selection/luma work offers only ~2 ms overlap. A representative recorded-audio fixture and separate transcription benchmark are prerequisites to a justified pipeline redesign. `blit_scaled` and `solid` remain unchanged; preserving premultiplied-alpha interpolation and rounding would require a separate focused experiment.

## Final before / after
Fresh control and final runs both used `nice -n 10`, one warmup and seven measurements. Control is the saved pre-optimization binary with identical timing instrumentation. These are **debug-build demo measurements**, not release-build or transcription measurements.

| Metric (ms unless stated) | Before | After |
| --- | ---: | ---: |
| Full demo median | 3161.009 | 756.974 |
| Full demo MAD | 154.304 | 4.726 |
| Full demo min | 2734.972 | 743.791 |
| Full demo max | 3472.059 | 769.348 |
| Pipeline median, excluding demo synthesis/startup | 2679.299 | 568.483 |
| Pipeline MAD | 140.593 | 7.164 |
| Selection median | 2.022 | 2.269 |
| Annotation median | 27.288 | 25.974 |
| Contact-sheet median | 616.126 | 424.044 |
| Artifacts median | 1928.300 | 116.154 |
| Total PNG bytes | 494314 | 1079182 |

**4.18× faster full demo (76.1% less time); 4.71× faster measured pipeline (78.8% less time).** Median stage values need not sum to the median total.

Final verification: 29 tests pass, including new fractional/clipped rasterizer equivalence and artifact failure/empty-frame cases. Pillow decoded all five final PNGs to RGBA and compared dimensions and every byte against the original baseline: identical. Manifests match after removing generated `id`/`createdAtMs`; summaries match after normalizing the bundle path. PNG compression is lossless; the explicit tradeoff is 2.18× encoded bytes. No dependencies changed.
