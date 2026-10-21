//! The Windows screen recorder: Windows.Graphics.Capture captures, the D3D11 video processor
//! crops and scales each frame on the GPU (also converting to NV12), and a Media Foundation
//! sink writer encodes H.264 into the movie (the GPU's encoder when it has one). Small BGRA
//! copies made the same way are the candidate frames. `run_sampler` is what
//! `screen::Session::start` runs on Windows.

use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
    D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_COLOR_SPACE,
    D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
    D3D11_VIDEO_USAGE_PLAYBACK_NORMAL, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
    ID3D11Multithread, ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoDevice,
    ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView,
    ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFMediaType, IMFSample, IMFSinkWriter, MF_MT_ALL_SAMPLES_INDEPENDENT,
    MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
    MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_VERSION, MFCreateAttributes, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFMediaType_Video,
    MFSTARTUP_FULL, MFStartup, MFVideoFormat_H264, MFVideoFormat_NV12,
    MFVideoInterlace_Progressive,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::core::{GUID, HSTRING, Interface};

use crate::model::{CaptureMode, RgbaFrame, TargetInfo};
use crate::native::SamplerHooks;
use crate::screen::{CANDIDATE_EDGE, CANDIDATE_EDGE_NO_VIDEO, FRAME_SAMPLE_INTERVAL, Sampler};

/// Frames are in pixels already, so a recording is never upscaled.
pub const MAX_CAPTURE_SCALE: f64 = 1.0;

fn capture_size(target: &TargetInfo, long_edge: f64) -> (u32, u32) {
    crate::screen::capture_size(target, long_edge, MAX_CAPTURE_SCALE)
}

/// Recordings keep at most this many frames a second (screens only send frames on change).
const MAX_FPS: u32 = 30;
const FRAME_100NS: i64 = 10_000_000 / MAX_FPS as i64;

/// Starts WinRT and Media Foundation once, and joins this thread to the multithreaded
/// apartment.
pub fn media_startup() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_FULL);
    });
    // Each thread that uses COM joins the multithreaded apartment.
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
}

/// Two `u32`s as Media Foundation packs them into a `u64` attribute.
pub fn pack(hi: u32, lo: u32) -> u64 {
    (u64::from(hi) << 32) | u64::from(lo)
}

/// A packed Media Foundation `u64` attribute's two halves.
pub fn unpack(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

/// QueryPerformanceCounter in 100 ns units, the clock `SystemRelativeTime` uses.
fn qpc_100ns() -> i64 {
    let (mut now, mut freq) = (0_i64, 1_i64);
    unsafe {
        let _ = QueryPerformanceCounter(&mut now);
        let _ = QueryPerformanceFrequency(&mut freq);
    }
    (now as i128 * 10_000_000 / freq.max(1) as i128) as i64
}

// MARK: Recording

struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    winrt: IDirect3DDevice,
}

impl Gpu {
    fn new() -> Result<Self> {
        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .context("D3D11CreateDevice")?;
        let device: ID3D11Device = device.context("no D3D11 device")?;
        let context: ID3D11DeviceContext = context.context("no D3D11 context")?;
        if let Ok(multithread) = device.cast::<ID3D11Multithread>() {
            let _ = unsafe { multithread.SetMultithreadProtected(true) };
        }
        let dxgi: IDXGIDevice = device.cast()?;
        let winrt: IDirect3DDevice =
            unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }?.cast()?;
        Ok(Self {
            video_device: device.cast()?,
            video_context: context.cast()?,
            device,
            context,
            winrt,
        })
    }

    fn texture(
        &self,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
        staging: bool,
    ) -> Result<ID3D11Texture2D> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: if staging {
                D3D11_USAGE_STAGING
            } else {
                D3D11_USAGE_DEFAULT
            },
            BindFlags: if staging {
                0
            } else {
                D3D11_BIND_RENDER_TARGET.0 as u32
            },
            CPUAccessFlags: if staging {
                D3D11_CPU_ACCESS_READ.0 as u32
            } else {
                0
            },
            MiscFlags: 0,
        };
        let mut texture = None;
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut texture)) }?;
        texture.context("CreateTexture2D")
    }
}

/// Crops, scales and converts the latest frame into one output size and format.
struct Scaler {
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    output: ID3D11Texture2D,
    output_view: ID3D11VideoProcessorOutputView,
    staging: ID3D11Texture2D,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
}

impl Scaler {
    fn new(
        gpu: &Gpu,
        input: (u32, u32),
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
    ) -> Result<Self> {
        let rate = DXGI_RATIONAL {
            Numerator: MAX_FPS,
            Denominator: 1,
        };
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: input.0,
            InputHeight: input.1,
            OutputFrameRate: rate,
            OutputWidth: width,
            OutputHeight: height,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        let enumerator = unsafe { gpu.video_device.CreateVideoProcessorEnumerator(&content) }?;
        let processor = unsafe { gpu.video_device.CreateVideoProcessor(&enumerator, 0) }?;
        let output = gpu.texture(width, height, format, false)?;
        let staging = gpu.texture(width, height, format, true)?;
        let desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0::default(),
        };
        let mut output_view = None;
        unsafe {
            gpu.video_device.CreateVideoProcessorOutputView(
                &output,
                &enumerator,
                &desc,
                Some(&mut output_view),
            )
        }?;
        let vc = &gpu.video_context;
        unsafe {
            vc.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            // Input: full-range RGB. Output: BT.709 limited-range YCbCr for video, full-range
            // RGB for candidates.
            let input_space = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 };
            vc.VideoProcessorSetStreamColorSpace(&processor, 0, &input_space);
            let output_space = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                _bitfield: if format == DXGI_FORMAT_NV12 {
                    4 | (1 << 4)
                } else {
                    0
                },
            };
            vc.VideoProcessorSetOutputColorSpace(&processor, &output_space);
        }
        Ok(Self {
            enumerator,
            processor,
            output,
            output_view: output_view.context("output view")?,
            staging,
            width,
            height,
            format,
        })
    }

    /// Draws `source` (the `crop` of `input`) into the output, and copies it to the CPU.
    fn run(&self, gpu: &Gpu, input: &ID3D11Texture2D, crop: RECT) -> Result<Vec<u8>> {
        let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0::default(),
        };
        let mut input_view: Option<ID3D11VideoProcessorInputView> = None;
        unsafe {
            gpu.video_device.CreateVideoProcessorInputView(
                input,
                &self.enumerator,
                &desc,
                Some(&mut input_view),
            )
        }?;
        let out_rect = RECT {
            left: 0,
            top: 0,
            right: self.width as i32,
            bottom: self.height as i32,
        };
        let vc = &gpu.video_context;
        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: ManuallyDrop::new(input_view),
            ..Default::default()
        };
        unsafe {
            vc.VideoProcessorSetStreamSourceRect(&self.processor, 0, true, Some(&crop));
            vc.VideoProcessorSetStreamDestRect(&self.processor, 0, true, Some(&out_rect));
            vc.VideoProcessorSetOutputTargetRect(&self.processor, true, Some(&out_rect));
        }
        let streams = [stream];
        let result =
            unsafe { vc.VideoProcessorBlt(&self.processor, &self.output_view, 0, &streams) };
        let [stream] = streams;
        drop(ManuallyDrop::into_inner(stream.pInputSurface));
        result?;
        unsafe { gpu.context.CopyResource(&self.staging, &self.output) };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            gpu.context
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }?;
        let pitch = mapped.RowPitch as usize;
        let base = mapped.pData.cast::<u8>();
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out;
        if self.format == DXGI_FORMAT_NV12 {
            // Y rows, then interleaved UV rows (half height), each `pitch` apart.
            out = Vec::with_capacity(w * h * 3 / 2);
            for row in 0..h + h / 2 {
                let line = unsafe { std::slice::from_raw_parts(base.add(row * pitch), w) };
                out.extend_from_slice(line);
            }
        } else {
            out = Vec::with_capacity(w * h * 4);
            for row in 0..h {
                let line = unsafe { std::slice::from_raw_parts(base.add(row * pitch), w * 4) };
                for px in line.chunks_exact(4) {
                    out.extend_from_slice(&[px[2], px[1], px[0], 255]);
                }
            }
        }
        unsafe { gpu.context.Unmap(&self.staging, 0) };
        Ok(out)
    }
}

struct Encoder {
    writer: IMFSinkWriter,
    stream: u32,
    first: Option<i64>,
    last: Option<i64>,
}

impl Encoder {
    fn new(path: &Path, width: u32, height: u32) -> Result<Self> {
        let mut attributes: Option<IMFAttributes> = None;
        unsafe { MFCreateAttributes(&mut attributes, 2) }?;
        let attributes = attributes.context("attributes")?;
        unsafe { attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1) }?;
        let writer = unsafe {
            MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, &attributes)
        }?;
        let bitrate = (f64::from(width) * f64::from(height) * f64::from(MAX_FPS) * 0.08) as u32;
        let output = video_type(&MFVideoFormat_H264, width, height)?;
        unsafe { output.SetUINT32(&MF_MT_AVG_BITRATE, bitrate.clamp(2_000_000, 16_000_000)) }?;
        let stream = unsafe { writer.AddStream(&output) }?;
        let input = video_type(&MFVideoFormat_NV12, width, height)?;
        unsafe {
            input.SetUINT32(&MF_MT_DEFAULT_STRIDE, width)?;
            input.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
            writer.SetInputMediaType(stream, &input, None)?;
            writer.BeginWriting()?;
        }
        Ok(Self {
            writer,
            stream,
            first: None,
            last: None,
        })
    }

    /// Writes one NV12 frame captured at `time` (100 ns, QPC clock).
    fn write(&mut self, nv12: &[u8], time: i64) -> Result<()> {
        let first = *self.first.get_or_insert(time);
        let mut t = time - first;
        if let Some(last) = self.last
            && t <= last
        {
            t = last + 1;
        }
        self.last = Some(t);
        let buffer = unsafe { MFCreateMemoryBuffer(nv12.len() as u32) }?;
        unsafe {
            let mut data = std::ptr::null_mut();
            buffer.Lock(&mut data, None, None)?;
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), data, nv12.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(nv12.len() as u32)?;
        }
        let sample: IMFSample = unsafe { MFCreateSample() }?;
        unsafe {
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(t)?;
            sample.SetSampleDuration(FRAME_100NS)?;
            self.writer.WriteSample(self.stream, &sample)?;
        }
        Ok(())
    }

    fn finish(self) -> Result<()> {
        unsafe { self.writer.Finalize() }?;
        Ok(())
    }
}

fn video_type(subtype: &GUID, width: u32, height: u32) -> Result<IMFMediaType> {
    let media = unsafe { MFCreateMediaType() }?;
    unsafe {
        media.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        media.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        media.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        media.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
        media.SetUINT64(&MF_MT_FRAME_RATE, pack(MAX_FPS, 1))?;
        media.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
    }
    Ok(media)
}

/// The capture item and the pixels of it to record.
struct Source {
    item: GraphicsCaptureItem,
    /// `[x, y, w, h]` of the target inside the item's frames (the whole frame for a window).
    crop: Option<[i32; 4]>,
}

fn capture_source(target: &TargetInfo) -> Result<Source> {
    let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    if target.mode == CaptureMode::Window
        && let Some(id) = target.window_id
    {
        let hwnd = crate::native::win::hwnd(id as i32 as isize);
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd) }?;
        return Ok(Source { item, crop: None });
    }
    let f = target.frame;
    let monitor = crate::native::win::monitor_at(f.x + f.width / 2.0, f.y + f.height / 2.0)
        .context("No display under the target")?;
    let handle = windows::Win32::Graphics::Gdi::HMONITOR(monitor.handle as *mut _);
    let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(handle) }?;
    let [mx, my, mw, mh] = monitor.rect;
    let crop = [
        (f.x - mx).round().max(0.0) as i32,
        (f.y - my).round().max(0.0) as i32,
        f.width.round().min(mw) as i32,
        f.height.round().min(mh) as i32,
    ];
    Ok(Source {
        item,
        crop: Some(crop),
    })
}

struct Recorder {
    gpu: Gpu,
    pool: Direct3D11CaptureFramePool,
    _session: GraphicsCaptureSession,
    source: Source,
    pool_size: SizeInt32,
    /// The newest frame, copied out of the frame pool (which recycles its textures).
    latest: Option<(ID3D11Texture2D, (u32, u32), i64)>,
    encoded: Option<i64>,
    video: Option<(Scaler, Encoder)>,
    candidates: Scaler,
    out: (u32, u32),
}

impl Recorder {
    fn new(target: &TargetInfo, video_path: Option<&Path>, long_edge: f64) -> Result<Self> {
        let gpu = Gpu::new()?;
        let source = capture_source(target)?;
        let pool_size = source.item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &gpu.winrt,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            pool_size,
        )?;
        let session = pool.CreateCaptureSession(&source.item)?;
        let _ = session.SetIsCursorCaptureEnabled(true);
        // Windows 11 can drop the yellow capture border.
        let _ = session.SetIsBorderRequired(false);
        let input = (
            pool_size.Width.max(1) as u32,
            pool_size.Height.max(1) as u32,
        );
        // H.264 encoders and decoders work in 16 px macroblocks and pad rows to match; a size
        // off that grid comes back sheared.
        let (w, h) = capture_size(target, long_edge);
        let out = ((w & !15).max(16), (h & !15).max(16));
        let video = match video_path {
            Some(path) => {
                let scaler = Scaler::new(&gpu, input, out.0, out.1, DXGI_FORMAT_NV12)?;
                Some((scaler, Encoder::new(path, out.0, out.1)?))
            }
            None => None,
        };
        let cand = capture_size(
            target,
            if video.is_some() {
                CANDIDATE_EDGE
            } else {
                CANDIDATE_EDGE_NO_VIDEO
            },
        );
        let candidates = Scaler::new(&gpu, input, cand.0, cand.1, DXGI_FORMAT_B8G8R8A8_UNORM)?;
        session.StartCapture()?;
        Ok(Self {
            gpu,
            pool,
            _session: session,
            source,
            pool_size,
            latest: None,
            encoded: None,
            video,
            candidates,
            out,
        })
    }

    fn crop(&self, size: (u32, u32)) -> RECT {
        let [x, y, w, h] = self
            .source
            .crop
            .unwrap_or([0, 0, size.0 as i32, size.1 as i32]);
        RECT {
            left: x.clamp(0, size.0 as i32),
            top: y.clamp(0, size.1 as i32),
            right: (x + w).clamp(1, size.0 as i32),
            bottom: (y + h).clamp(1, size.1 as i32),
        }
    }

    /// Takes every waiting frame, keeping the newest. Returns its capture time if one arrived.
    fn pump(&mut self) -> Result<Option<i64>> {
        let mut newest: Option<Direct3D11CaptureFrame> = None;
        while let Ok(frame) = self.pool.TryGetNextFrame() {
            newest = Some(frame);
        }
        let Some(frame) = newest else {
            return Ok(None);
        };
        let content = frame.ContentSize()?;
        let time = frame.SystemRelativeTime()?.Duration;
        let surface = frame.Surface()?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
        let texture: ID3D11Texture2D = unsafe { access.GetInterface() }?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut desc) };
        let size = (
            (content.Width.max(1) as u32).min(desc.Width),
            (content.Height.max(1) as u32).min(desc.Height),
        );
        let reuse = self
            .latest
            .as_ref()
            .filter(|(_, s, _)| *s == (desc.Width, desc.Height))
            .map(|(t, _, _)| t.clone());
        let copy = match reuse {
            Some(copy) => copy,
            None => self
                .gpu
                .texture(desc.Width, desc.Height, DXGI_FORMAT_B8G8R8A8_UNORM, false)?,
        };
        unsafe { self.gpu.context.CopyResource(&copy, &texture) };
        // Only the frame's content area is the window (a resized window leaves padding).
        self.latest = Some((copy, size, time));
        drop(frame);
        if content.Width != self.pool_size.Width || content.Height != self.pool_size.Height {
            self.pool_size = content;
            self.pool.Recreate(
                &self.gpu.winrt,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                content,
            )?;
        }
        Ok(Some(time))
    }

    /// Encodes the newest frame if it is new and the frame rate allows.
    fn encode(&mut self, force_at: Option<i64>) -> Result<()> {
        let Some((texture, size, time)) = self.latest.clone() else {
            return Ok(());
        };
        let time = force_at.unwrap_or(time);
        if force_at.is_none()
            && self
                .encoded
                .is_some_and(|last| time <= last || time - last < FRAME_100NS)
        {
            return Ok(());
        }
        let crop = self.crop(size);
        let Some((scaler, encoder)) = self.video.as_mut() else {
            return Ok(());
        };
        let nv12 = scaler.run(&self.gpu, &texture, crop)?;
        encoder.write(&nv12, time)?;
        self.encoded = Some(time);
        Ok(())
    }

    fn candidate(&self, t_ms: u64) -> Result<Option<RgbaFrame>> {
        let Some((texture, size, _)) = self.latest.as_ref() else {
            return Ok(None);
        };
        let rgba = self.candidates.run(&self.gpu, texture, self.crop(*size))?;
        Ok(Some(RgbaFrame::new(
            t_ms,
            self.candidates.width,
            self.candidates.height,
            rgba,
        )))
    }
}

/// Records `sampler`'s target until it stops: the movie (when `video_output_path` is set) and
/// candidate frames, at the buffer's interval or at once when `urgent`. Returns the movie.
pub fn run_sampler(sampler: Sampler, hooks: SamplerHooks) -> Option<PathBuf> {
    media_startup();
    let Sampler {
        target,
        video_output_path,
        started_at,
        stopping,
        urgent,
        frames,
        video_offset_ms,
        ..
    } = sampler;
    let long_edge = if target.mode == CaptureMode::Display {
        2560.0
    } else {
        1280.0
    };
    let mut recorder = match Recorder::new(&target, video_output_path.as_deref(), long_edge) {
        Ok(recorder) => Some(recorder),
        Err(error) => {
            hooks.log(&format!("Screen capture failed to start: {error:#}"));
            None
        }
    };
    if let Some(ref r) = recorder {
        hooks.log(&format!(
            "Recording {}x{} ({:?})",
            r.out.0, r.out.1, target.mode
        ));
    }
    if let Some(ready) = hooks.ready {
        ready(started_at);
    }
    let mut last_sample = Instant::now()
        .checked_sub(FRAME_SAMPLE_INTERVAL)
        .unwrap_or_else(Instant::now);
    let mut phase = 0_u32;
    let mut failed = false;
    while !stopping.load(Ordering::Acquire) {
        if let Some(r) = recorder.as_mut()
            && !failed
        {
            let step = r.pump().and_then(|arrived| {
                if let Some(time) = arrived
                    && r.encoded.is_none()
                {
                    // The movie starts at its first frame; place that on the capture clock.
                    let lag_ms = (qpc_100ns() - time).max(0) / 10_000;
                    let offset = (started_at.elapsed().as_millis() as i64 - lag_ms).max(0);
                    video_offset_ms.store(offset as u64, Ordering::Release);
                }
                r.encode(None)
            });
            if let Err(error) = step {
                hooks.log(&format!("Recording frame failed: {error:#}"));
                failed = true;
            }
        }
        let is_urgent = urgent.swap(false, Ordering::AcqRel);
        let interval = frames
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .interval();
        if is_urgent || last_sample.elapsed() >= interval {
            last_sample = Instant::now();
            let t_ms = started_at.elapsed().as_millis() as u64;
            if is_urgent {
                phase = phase.saturating_add(1);
            }
            let frame = recorder
                .as_ref()
                .and_then(|r| r.candidate(t_ms).ok().flatten())
                .unwrap_or_else(|| (hooks.placeholder)(t_ms, phase));
            frames
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(frame, is_urgent);
        }
        std::thread::park_timeout(Duration::from_millis(8));
    }
    let recorder = recorder?;
    let mut recorder = recorder;
    // Hold the last frame until the end, so the movie lasts as long as the capture.
    let _ = recorder.pump();
    let _ = recorder.encode(Some(qpc_100ns()));
    let Recorder { video, .. } = recorder;
    let (_, encoder) = video?;
    encoder.first?;
    match encoder.finish() {
        Ok(()) => video_output_path,
        Err(error) => {
            hooks.log(&format!("Finishing the recording failed: {error:#}"));
            None
        }
    }
}
