//! Movies on Windows. The recorder is `scrawl_capture::native::wgc`: it captures with
//! Windows.Graphics.Capture, crops and scales each frame on the GPU (D3D11 video processor, which
//! also converts to NV12) and encodes H.264 into `recording.mp4` through a Media Foundation sink
//! writer (the GPU's encoder when it has one). Small BGRA copies made the same way are the
//! keyframe candidates.
//!
//! Decoding (`frames`, `probe`, `export_audio`) goes through the Media Foundation source reader.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer, IMFAttributes, IMFSample, IMFSourceReader, MF_E_INVALIDSTREAMNUMBER,
    MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
    MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_MINIMUM_DISPLAY_APERTURE,
    MF_MT_SUBTYPE, MF_PD_DURATION, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_AUDIO_STREAM,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_SOURCE_READER_MEDIASOURCE,
    MF_SOURCE_READERF_ENDOFSTREAM, MFAudioFormat_Float, MFCreateAttributes, MFCreateMediaType,
    MFCreateSourceReaderFromURL, MFMediaType_Audio, MFMediaType_Video, MFVideoArea,
    MFVideoFormat_RGB32,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::core::{GUID, HSTRING, Interface};

use crate::capture::{Sampler, scrawl_log};
use crate::helper::{FrameTimes, Probe};
use crate::model::RgbaFrame;
// The recorder is `scrawl-capture`'s (`native::wgc`); decoding below shares its Media
// Foundation setup.
#[cfg(test)]
pub use scrawl_capture::native::wgc::write_test_movie;
use scrawl_capture::native::wgc::{media_startup, unpack};

/// The recording thread: Windows.Graphics.Capture and Media Foundation
/// (`scrawl_capture::native::wgc`), with Scrawl's log, the `sck_ready_ms` metric and the
/// synthetic demo frame when a frame can't be taken.
pub fn run_sampler(sampler: Sampler) -> Option<PathBuf> {
    scrawl_capture::native::wgc::run_sampler(
        sampler,
        scrawl_capture::native::SamplerHooks {
            log: Some(|line| scrawl_log(line)),
            ready: Some(|started_at| crate::ui::start_metric("sck_ready_ms", started_at)),
            placeholder: |t_ms, phase| {
                crate::render::render_synthetic_demo_frame(1280, 800, t_ms, phase)
            },
        },
    )
}

// MARK: Reading movies

fn reader(movie: &Path, video_processing: bool) -> Result<IMFSourceReader> {
    media_startup();
    let mut attributes: Option<IMFAttributes> = None;
    unsafe { MFCreateAttributes(&mut attributes, 1) }?;
    let attributes = attributes.context("attributes")?;
    if video_processing {
        unsafe { attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1) }?;
    }
    unsafe { MFCreateSourceReaderFromURL(&HSTRING::from(movie.as_os_str()), &attributes) }
        .with_context(|| format!("Could not open {}", movie.display()))
}

fn duration_100ns(reader: &IMFSourceReader) -> u64 {
    unsafe {
        reader.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
    }
    .ok()
    .and_then(|value: PROPVARIANT| u64::try_from(&value).ok())
    .unwrap_or(0)
}

pub fn probe(movie: &Path) -> Result<Probe> {
    let reader = reader(movie, false)?;
    let duration_ms = duration_100ns(&reader) / 10_000;
    let (mut width, mut height, mut fps, mut audio_tracks) = (0, 0, 0.0, 0);
    for stream in 0.. {
        let media = match unsafe { reader.GetNativeMediaType(stream, 0) } {
            Ok(media) => media,
            Err(error) if error.code() == MF_E_INVALIDSTREAMNUMBER => break,
            Err(_) => break,
        };
        let major = unsafe { media.GetGUID(&MF_MT_MAJOR_TYPE) }.unwrap_or_default();
        if major == MFMediaType_Audio {
            audio_tracks += 1;
        } else if major == MFMediaType_Video && width == 0 {
            if let Ok(size) = unsafe { media.GetUINT64(&MF_MT_FRAME_SIZE) } {
                (width, height) = unpack(size);
            }
            if let Ok(rate) = unsafe { media.GetUINT64(&MF_MT_FRAME_RATE) } {
                let (n, d) = unpack(rate);
                fps = f64::from(n) / f64::from(d.max(1));
            }
        }
    }
    Ok(Probe {
        duration_ms,
        width,
        height,
        audio_tracks,
        fps,
    })
}

/// A decoded video frame as RGBA, and its time in ms.
struct Decoded {
    t_ms: u64,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

struct VideoReader {
    reader: IMFSourceReader,
    width: u32,
    height: u32,
    /// The visible area inside each (possibly padded) decoded frame.
    aperture: (u32, u32, u32, u32),
    duration_ms: u64,
}

impl VideoReader {
    fn open(movie: &Path) -> Result<Self> {
        let reader = reader(movie, true)?;
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        unsafe {
            reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
            reader.SetStreamSelection(stream, true)?;
        }
        let wanted = unsafe { MFCreateMediaType() }?;
        unsafe {
            wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            wanted.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            reader.SetCurrentMediaType(stream, None, &wanted)?;
        }
        let current = unsafe { reader.GetCurrentMediaType(stream) }?;
        let (width, height) = unpack(unsafe { current.GetUINT64(&MF_MT_FRAME_SIZE) }?);
        let mut area = MFVideoArea::default();
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(
                (&raw mut area).cast::<u8>(),
                std::mem::size_of::<MFVideoArea>(),
            )
        };
        let aperture = if unsafe { current.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, bytes, None) }
            .is_ok()
            && area.Area.cx > 0
        {
            (
                area.OffsetX.value.max(0) as u32,
                area.OffsetY.value.max(0) as u32,
                area.Area.cx as u32,
                area.Area.cy as u32,
            )
        } else {
            (0, 0, width, height)
        };
        Ok(Self {
            duration_ms: duration_100ns(&reader) / 10_000,
            reader,
            width,
            height,
            aperture,
        })
    }

    fn seek(&self, ms: u64) -> Result<()> {
        let position = PROPVARIANT::from(ms as i64 * 10_000);
        unsafe { self.reader.SetCurrentPosition(&GUID::zeroed(), &position) }?;
        Ok(())
    }

    /// The next decoded frame, or `None` at the end.
    fn next(&self) -> Result<Option<(u64, IMFSample)>> {
        let mut flags = 0_u32;
        let mut time = 0_i64;
        let mut sample = None;
        unsafe {
            self.reader.ReadSample(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                0,
                None,
                Some(&mut flags),
                Some(&mut time),
                Some(&mut sample),
            )
        }?;
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            return Ok(None);
        }
        match sample {
            Some(sample) => Ok(Some(((time.max(0) / 10_000) as u64, sample))),
            None => self.next(),
        }
    }

    fn convert(&self, t_ms: u64, sample: &IMFSample, max_edge: u32) -> Result<Decoded> {
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }?;
        let (ax, ay, aw, ah) = self.aperture;
        let mut rgba = vec![0_u8; aw as usize * ah as usize * 4];
        let mut copy = |base: *const u8, pitch: isize| {
            for y in 0..ah as usize {
                let row =
                    unsafe { base.offset((y as isize + ay as isize) * pitch + ax as isize * 4) };
                let src = unsafe { std::slice::from_raw_parts(row, aw as usize * 4) };
                let dst = &mut rgba[y * aw as usize * 4..][..aw as usize * 4];
                for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                    d.copy_from_slice(&[s[2], s[1], s[0], 255]);
                }
            }
        };
        if let Ok(two_d) = buffer.cast::<IMF2DBuffer>() {
            let mut scan0 = std::ptr::null_mut();
            let mut pitch = 0_i32;
            unsafe { two_d.Lock2D(&mut scan0, &mut pitch) }?;
            copy(scan0, pitch as isize);
            unsafe { two_d.Unlock2D() }?;
        } else {
            let mut data = std::ptr::null_mut();
            let mut len = 0_u32;
            unsafe { buffer.Lock(&mut data, None, Some(&mut len)) }?;
            ensure!(
                len as usize >= self.width as usize * self.height as usize * 4,
                "Short video frame"
            );
            copy(data, self.width as isize * 4);
            unsafe { buffer.Unlock() }?;
        }
        Ok(shrink_to_edge(
            Decoded {
                t_ms,
                width: aw,
                height: ah,
                rgba,
            },
            max_edge,
        ))
    }

    /// The frame on screen at each time (the last one at or before it, give or take 40 ms),
    /// for times in increasing order. Returns `(requested, frame)`.
    fn at(&self, times: &[u64], max_edge: u32) -> Result<Vec<(u64, Decoded)>> {
        let mut out = Vec::with_capacity(times.len());
        on_screen_at(
            times,
            self.duration_ms,
            |t| self.seek(t),
            || self.next(),
            |want, t, sample| {
                out.push((want, self.convert(t, sample, max_edge)?));
                Ok(())
            },
        )?;
        Ok(out)
    }
}

/// Walks a movie's samples (`next`, in order; `seek` jumps to the key frame before a time) and
/// hands `emit` the one on screen at each requested time (in increasing order): the last at or
/// before it, give or take 40 ms. A time past the end gets the last sample, and is still
/// answered as the time asked for, so callers can match it.
fn on_screen_at<S>(
    times: &[u64],
    duration_ms: u64,
    mut seek: impl FnMut(u64) -> Result<()>,
    mut next: impl FnMut() -> Result<Option<(u64, S)>>,
    mut emit: impl FnMut(u64, u64, &S) -> Result<()>,
) -> Result<()> {
    let mut previous: Option<(u64, S)> = None;
    let mut pending: Option<(u64, S)> = None;
    let mut position: Option<u64> = None;
    for &want in times {
        let target = want.min(duration_ms.saturating_sub(1));
        // Far ahead (or behind): seek, landing on the key frame before it.
        if position.is_none_or(|p| target + 40 < p || target > p + 2_000) {
            seek(target.saturating_sub(40))?;
            previous = None;
            pending = None;
        }
        loop {
            let sample = match pending.take() {
                Some(sample) => Some(sample),
                None => next()?,
            };
            let Some((t, sample)) = sample else {
                break;
            };
            position = Some(t);
            if t > target + 40 {
                if previous.is_none() {
                    previous = Some((t, sample));
                } else {
                    pending = Some((t, sample));
                }
                break;
            }
            previous = Some((t, sample));
        }
        if let Some((t, sample)) = previous.as_ref() {
            emit(want, *t, sample)?;
        }
    }
    Ok(())
}

/// Box-filters a frame down so its long edge is at most `max_edge` (0 keeps it).
fn shrink_to_edge(frame: Decoded, max_edge: u32) -> Decoded {
    let long = frame.width.max(frame.height);
    if max_edge == 0 || long <= max_edge {
        return frame;
    }
    let scale = f64::from(max_edge) / f64::from(long);
    let w = ((f64::from(frame.width) * scale).round() as u32).max(1);
    let h = ((f64::from(frame.height) * scale).round() as u32).max(1);
    let (sw, sh) = (frame.width as usize, frame.height as usize);
    let mut out = vec![0_u8; w as usize * h as usize * 4];
    for y in 0..h as usize {
        let y0 = y * sh / h as usize;
        let y1 = ((y + 1) * sh / h as usize).max(y0 + 1);
        for x in 0..w as usize {
            let x0 = x * sw / w as usize;
            let x1 = ((x + 1) * sw / w as usize).max(x0 + 1);
            let mut acc = [0_u32; 3];
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            for sy in y0..y1 {
                let row = &frame.rgba[(sy * sw + x0) * 4..(sy * sw + x1) * 4];
                for px in row.chunks_exact(4) {
                    acc[0] += u32::from(px[0]);
                    acc[1] += u32::from(px[1]);
                    acc[2] += u32::from(px[2]);
                }
            }
            let o = &mut out[(y * w as usize + x) * 4..][..4];
            o.copy_from_slice(&[
                (acc[0] / n) as u8,
                (acc[1] / n) as u8,
                (acc[2] / n) as u8,
                255,
            ]);
        }
    }
    Decoded {
        t_ms: frame.t_ms,
        width: w,
        height: h,
        rgba: out,
    }
}

pub fn frames(movie: &Path, times: FrameTimes, max_edge: u32) -> Result<Vec<RgbaFrame>> {
    let video = VideoReader::open(movie)?;
    let (requested, sampled): (Vec<u64>, bool) = match times {
        FrameTimes::At(times) => {
            if times.is_empty() {
                return Ok(Vec::new());
            }
            (times.to_vec(), false)
        }
        FrameTimes::Every(ms) => (
            (0..video.duration_ms.max(1))
                .step_by(ms.max(1) as usize)
                .collect(),
            true,
        ),
    };
    let mut order: Vec<usize> = (0..requested.len()).collect();
    order.sort_by_key(|&i| requested[i]);
    let sorted: Vec<u64> = order.iter().map(|&i| requested[i]).collect();
    let decoded = video.at(&sorted, max_edge)?;
    ensure!(
        !decoded.is_empty(),
        "No frames decoded from {}",
        movie.display()
    );
    let mut out: Vec<RgbaFrame> = Vec::with_capacity(decoded.len());
    for (want, frame) in decoded {
        let t = if sampled { frame.t_ms } else { want };
        if sampled && out.last().is_some_and(|f| f.t_ms == t) {
            continue;
        }
        out.push(RgbaFrame::new(t, frame.width, frame.height, frame.rgba));
    }
    if !sampled {
        // Back in the order asked for.
        let mut by_time: Vec<Option<RgbaFrame>> = out.into_iter().map(Some).collect();
        let mut reordered = Vec::with_capacity(by_time.len());
        for &t in &requested {
            if let Some(slot) = by_time
                .iter_mut()
                .find(|f| f.as_ref().is_some_and(|f| f.t_ms == t))
            {
                reordered.push(slot.take().unwrap_or_else(|| unreachable!()));
            }
        }
        return Ok(reordered);
    }
    Ok(out)
}

pub fn shrink(_movie: &Path, _out: &Path, _max_edge: u32) -> Result<crate::retention::Shrunk> {
    bail!("Only cabled-iPhone recordings (macOS) are re-encoded")
}

/// The movie's first audio track as 16 kHz mono WAV, starting at the movie's start.
pub fn export_audio(movie: &Path, wav: &Path) -> Result<()> {
    let reader = reader(movie, false)?;
    let stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
    unsafe {
        reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
        reader.SetStreamSelection(stream, true)?;
    }
    let wanted = unsafe { MFCreateMediaType() }?;
    unsafe {
        wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        wanted.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)?;
        reader.SetCurrentMediaType(stream, None, &wanted)?;
    }
    let current = unsafe { reader.GetCurrentMediaType(stream) }?;
    let rate = unsafe { current.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND) }?.max(1);
    let channels = unsafe { current.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS) }?.max(1) as usize;
    ensure!(
        unsafe { current.GetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE) }.unwrap_or(32) == 32,
        "Unexpected audio format"
    );
    let mut mono: Vec<f32> = Vec::new();
    loop {
        let mut flags = 0_u32;
        let mut time = 0_i64;
        let mut sample = None;
        unsafe {
            reader.ReadSample(
                stream,
                0,
                None,
                Some(&mut flags),
                Some(&mut time),
                Some(&mut sample),
            )
        }?;
        if let Some(sample) = sample {
            if mono.is_empty() && time > 0 {
                // Silence before the first sound, so audio time is movie time.
                mono.resize((time as u64 * u64::from(rate) / 10_000_000) as usize, 0.0);
            }
            let buffer = unsafe { sample.ConvertToContiguousBuffer() }?;
            let mut data = std::ptr::null_mut();
            let mut len = 0_u32;
            unsafe { buffer.Lock(&mut data, None, Some(&mut len)) }?;
            let floats =
                unsafe { std::slice::from_raw_parts(data.cast::<f32>(), len as usize / 4) };
            mono.extend(
                floats
                    .chunks_exact(channels)
                    .map(|frame| frame.iter().sum::<f32>() / channels as f32),
            );
            unsafe { buffer.Unlock() }?;
        }
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            break;
        }
    }
    // Area-average down (or hold up) to 16 kHz.
    let out_len = (mono.len() as u64 * 16_000 / u64::from(rate)) as usize;
    let ratio = f64::from(rate) / 16_000.0;
    let resampled: Vec<f32> = (0..out_len)
        .map(|i| {
            let start = (i as f64 * ratio) as usize;
            let end = (((i + 1) as f64 * ratio) as usize).clamp(start + 1, mono.len());
            let slice = &mono[start.min(mono.len() - 1)..end];
            slice.iter().sum::<f32>() / slice.len().max(1) as f32
        })
        .collect();
    std::fs::write(wav, crate::audio::encode_wav_16k(&resampled)?)
        .with_context(|| format!("Could not write {}", wav.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Picks from a movie with a frame every 100 ms up to `last`, seeking to the frame at or
    /// before a time as a key frame every 1 s would.
    fn pick(times: &[u64], duration_ms: u64, last: u64) -> Vec<(u64, u64)> {
        let cursor = std::cell::Cell::new(0_u64);
        let mut out = Vec::new();
        on_screen_at(
            times,
            duration_ms,
            |t| {
                cursor.set(t / 1_000 * 1_000);
                Ok(())
            },
            || {
                let t = cursor.get();
                cursor.set(t + 100);
                Ok((t <= last).then_some((t, t)))
            },
            |want, t, &sample| {
                assert_eq!(t, sample);
                out.push((want, t));
                Ok(())
            },
        )
        .unwrap();
        out
    }

    #[test]
    fn a_time_past_the_end_gets_the_last_frame_under_its_own_time() {
        // A 3 s movie whose last frame is at 2.9 s; the last keyframe is asked for at 3.2 s.
        assert_eq!(
            pick(&[0, 1_250, 3_200], 3_000, 2_900),
            [(0, 0), (1_250, 1_200), (3_200, 2_900)]
        );
    }

    #[test]
    fn frames_are_the_last_at_or_before_each_time_give_or_take_40_ms() {
        assert_eq!(
            pick(&[150, 160, 1_030, 2_510], 3_000, 2_900),
            [(150, 100), (160, 200), (1_030, 1_000), (2_510, 2_500)]
        );
        // Far ahead: a seek, then the walk from the key frame.
        assert_eq!(
            pick(&[150, 2_510], 3_000, 2_900),
            [(150, 100), (2_510, 2_500)]
        );
    }
}
