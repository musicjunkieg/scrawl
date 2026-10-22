//! Scrawl's bundle compiler for apps, through a C interface (`include/scrawl.h`), the same shape
//! as `scrawl-wasm`'s: add the recording's frames as RGBA pixels, optionally ask which keyframes
//! it picks and add those again at full size, then compile the capture (`scrawl.json`'s fields as
//! JSON, a [`scrawl_core::capture::Capture`]) into the bundle's files, all in memory.
//!
//! ScrawlKit builds this into `ScrawlCore.xcframework` and wraps it in Swift. Every function
//! takes and returns plain C types; strings are UTF-8 and NUL-terminated. What a function returns
//! is freed with its `_free` function; what a bundle's getters return is borrowed and lives as
//! long as the bundle. Errors come back as NULL (or `false`) with `*error` set to a message the
//! caller frees with `scrawl_string_free`. A panic inside is caught and reported the same way.

use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use scrawl_core::capture::{Capture, compile_with_full, keyframe_times};
use scrawl_core::model::RgbaFrame;

/// Collects a capture's frames, then builds its bundle.
#[derive(Default)]
pub struct ScrawlCompiler {
    frames: Vec<RgbaFrame>,
    full: Vec<RgbaFrame>,
}

/// A built bundle's files, in the order to save them (images, `scrawl.json`, `summary.md` last).
pub struct ScrawlBundle {
    names: Vec<CString>,
    files: Vec<Vec<u8>>,
    summary: CString,
}

/// A new, empty compiler. Free it with `scrawl_compiler_free`.
#[unsafe(no_mangle)]
pub extern "C" fn scrawl_compiler_new() -> *mut ScrawlCompiler {
    Box::into_raw(Box::default())
}

/// Frees a compiler and any frames it still holds. NULL is ignored.
///
/// # Safety
/// `compiler` is NULL or came from `scrawl_compiler_new` and isn't used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scrawl_compiler_free(compiler: *mut ScrawlCompiler) {
    if !compiler.is_null() {
        drop(unsafe { Box::from_raw(compiler) });
    }
}

/// Adds the screen as it looked `t_ms` into the capture: `len` bytes of `width`×`height` RGBA
/// (copied). Candidates can be small (the Mac app uses 480 px) when the keyframes are added again
/// with `scrawl_compiler_add_full_frame`. `false` when `compiler` or `rgba` is NULL; the pixel
/// count is checked when compiling.
///
/// # Safety
/// `compiler` is a live compiler and `rgba` points to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scrawl_compiler_add_frame(
    compiler: *mut ScrawlCompiler,
    t_ms: u64,
    width: u32,
    height: u32,
    rgba: *const u8,
    len: usize,
) -> bool {
    match (unsafe { compiler.as_mut() }, unsafe {
        frame(t_ms, width, height, rgba, len)
    }) {
        (Some(compiler), Some(frame)) => {
            compiler.frames.push(frame);
            true
        }
        _ => false,
    }
}

/// Adds a keyframe at full size, at one of `scrawl_compiler_keyframe_times`; the bundle's images
/// are made from these instead of the candidates.
///
/// # Safety
/// As `scrawl_compiler_add_frame`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scrawl_compiler_add_full_frame(
    compiler: *mut ScrawlCompiler,
    t_ms: u64,
    width: u32,
    height: u32,
    rgba: *const u8,
    len: usize,
) -> bool {
    match (unsafe { compiler.as_mut() }, unsafe {
        frame(t_ms, width, height, rgba, len)
    }) {
        (Some(compiler), Some(frame)) => {
            compiler.full.push(frame);
            true
        }
        _ => false,
    }
}

/// The times (ms) of the keyframes `scrawl_compiler_compile` will pick from the candidates added
/// so far, for `capture_json`. Writes their count to `*count`; free the array with
/// `scrawl_times_free`. NULL with `*error` set on bad JSON or frames (an empty list is a non-NULL
/// array with `*count` 0).
///
/// # Safety
/// `compiler` is a live compiler, `capture_json` a NUL-terminated string, `count` writable, and
/// `error` NULL or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scrawl_compiler_keyframe_times(
    compiler: *const ScrawlCompiler,
    capture_json: *const c_char,
    count: *mut usize,
    error: *mut *mut c_char,
) -> *mut u64 {
    let result = guard(|| {
        let compiler = unsafe { compiler.as_ref() }.ok_or("The compiler is NULL")?;
        let capture = unsafe { parse(capture_json) }?;
        keyframe_times(&capture, &compiler.frames).map_err(|e| format!("{e:#}"))
    });
    match result {
        Ok(times) => {
            if let Some(count) = unsafe { count.as_mut() } {
                *count = times.len();
            }
            Box::into_raw(times.into_boxed_slice()).cast()
        }
        Err(message) => {
            unsafe { report(error, message) };
            ptr::null_mut()
        }
    }
}

/// Frees an array from `scrawl_compiler_keyframe_times`. NULL is ignored.
///
/// # Safety
/// `times` is NULL or came from `scrawl_compiler_keyframe_times` with this `count`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scrawl_times_free(times: *mut u64, count: usize) {
    if !times.is_null() {
        drop(unsafe { Box::from_raw(ptr::slice_from_raw_parts_mut(times, count)) });
    }
}

/// Builds the bundle from the frames added so far and `capture_json`; the frames are used up.
/// Free the bundle with `scrawl_bundle_free`. NULL with `*error` set on bad JSON or frames.
///
/// # Safety
/// As `scrawl_compiler_keyframe_times`, with `compiler` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scrawl_compiler_compile(
    compiler: *mut ScrawlCompiler,
    capture_json: *const c_char,
    error: *mut *mut c_char,
) -> *mut ScrawlBundle {
    let result = guard(|| {
        let compiler = unsafe { compiler.as_mut() }.ok_or("The compiler is NULL")?;
        let capture = unsafe { parse(capture_json) }?;
        let frames = std::mem::take(&mut compiler.frames);
        let full = std::mem::take(&mut compiler.full);
        let built = compile_with_full(capture, frames, full).map_err(|e| format!("{e:#}"))?;
        let mut names = Vec::with_capacity(built.files.len());
        let mut files = Vec::with_capacity(built.files.len());
        for (name, bytes) in built.files {
            names.push(CString::new(name).map_err(|e| e.to_string())?);
            files.push(bytes);
        }
        let summary = CString::new(built.summary).map_err(|e| e.to_string())?;
        Ok(ScrawlBundle {
            names,
            files,
            summary,
        })
    });
    match result {
        Ok(bundle) => Box::into_raw(Box::new(bundle)),
        Err(message) => {
            unsafe { report(error, message) };
            ptr::null_mut()
        }
    }
}

/// Frees a bundle; the pointers its getters returned die with it. NULL is ignored.
///
/// # Safety
/// `bundle` is NULL or came from `scrawl_compiler_compile` and isn't used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scrawl_bundle_free(bundle: *mut ScrawlBundle) {
    if !bundle.is_null() {
        drop(unsafe { Box::from_raw(bundle) });
    }
}

/// How many files the bundle has (0 for NULL).
///
/// # Safety
/// `bundle` is NULL or a live bundle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scrawl_bundle_file_count(bundle: *const ScrawlBundle) -> usize {
    unsafe { bundle.as_ref() }.map_or(0, |b| b.files.len())
}

/// The path inside the bundle of file `index` (`frames/frame-01.png`), or NULL out of range.
///
/// # Safety
/// `bundle` is NULL or a live bundle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scrawl_bundle_file_name(
    bundle: *const ScrawlBundle,
    index: usize,
) -> *const c_char {
    unsafe { bundle.as_ref() }
        .and_then(|b| b.names.get(index))
        .map_or(ptr::null(), |name| name.as_ptr())
}

/// The bytes of file `index`, with their count in `*len`, or NULL out of range.
///
/// # Safety
/// `bundle` is NULL or a live bundle, and `len` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scrawl_bundle_file_data(
    bundle: *const ScrawlBundle,
    index: usize,
    len: *mut usize,
) -> *const u8 {
    let Some(bytes) = unsafe { bundle.as_ref() }.and_then(|b| b.files.get(index)) else {
        return ptr::null();
    };
    if let Some(len) = unsafe { len.as_mut() } {
        *len = bytes.len();
    }
    bytes.as_ptr()
}

/// `summary.md`'s text (also file `summary.md`), or NULL for a NULL bundle.
///
/// # Safety
/// `bundle` is NULL or a live bundle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scrawl_bundle_summary(bundle: *const ScrawlBundle) -> *const c_char {
    unsafe { bundle.as_ref() }.map_or(ptr::null(), |b| b.summary.as_ptr())
}

/// Frees an error message. NULL is ignored.
///
/// # Safety
/// `message` is NULL or came from one of these functions' `error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scrawl_string_free(message: *mut c_char) {
    if !message.is_null() {
        drop(unsafe { CString::from_raw(message) });
    }
}

/// A frame from C's pixels (copied), or `None` for NULL pixels.
unsafe fn frame(
    t_ms: u64,
    width: u32,
    height: u32,
    rgba: *const u8,
    len: usize,
) -> Option<RgbaFrame> {
    if rgba.is_null() {
        return None;
    }
    let rgba = unsafe { std::slice::from_raw_parts(rgba, len) }.to_vec();
    Some(RgbaFrame {
        t_ms,
        width,
        height,
        rgba,
        badge_top_right: false,
    })
}

unsafe fn parse(capture_json: *const c_char) -> Result<Capture, String> {
    if capture_json.is_null() {
        return Err("The capture JSON is NULL".into());
    }
    let json = unsafe { CStr::from_ptr(capture_json) }
        .to_str()
        .map_err(|_| "The capture JSON isn't UTF-8".to_owned())?;
    serde_json::from_str(json)
        .map_err(|e| format!("The capture isn't valid JSON for a Scrawl capture: {e}"))
}

/// Runs `body`, turning a panic into an error instead of unwinding into C.
fn guard<T>(body: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(result) => result,
        Err(panic) => Err(panic
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .map_or_else(
                || "The compiler panicked".to_owned(),
                |m| format!("The compiler panicked: {m}"),
            )),
    }
}

unsafe fn report(error: *mut *mut c_char, message: impl Into<String>) {
    if let Some(slot) = unsafe { error.as_mut() } {
        let message = message.into().replace('\0', " ");
        *slot = CString::new(message).map_or(ptr::null_mut(), CString::into_raw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPTURE: &CStr = cr#"{"id":"ffi-1","target":{"mode":"window","app":"Example","displayId":0,"frame":{"x":0,"y":0,"width":32,"height":20,"scale":1}},"durationMs":1000}"#;

    #[test]
    fn a_capture_compiles_through_the_c_interface() {
        unsafe {
            let compiler = scrawl_compiler_new();
            let rgba = vec![90_u8; 32 * 20 * 4];
            assert!(scrawl_compiler_add_frame(
                compiler,
                0,
                32,
                20,
                rgba.as_ptr(),
                rgba.len()
            ));
            let mut error = ptr::null_mut();
            let bundle = scrawl_compiler_compile(compiler, CAPTURE.as_ptr(), &mut error);
            assert!(!bundle.is_null() && error.is_null());
            let last = scrawl_bundle_file_count(bundle) - 1;
            let name = CStr::from_ptr(scrawl_bundle_file_name(bundle, last));
            assert_eq!(name.to_str(), Ok("summary.md"));
            let summary = CStr::from_ptr(scrawl_bundle_summary(bundle))
                .to_str()
                .unwrap();
            assert!(summary.contains("ffi-1.scrawl"), "{summary}");
            scrawl_bundle_free(bundle);
            scrawl_compiler_free(compiler);
        }
    }

    #[test]
    fn null_and_bad_input_are_errors_not_crashes() {
        unsafe {
            let mut error = ptr::null_mut();
            assert!(
                scrawl_compiler_compile(ptr::null_mut(), CAPTURE.as_ptr(), &mut error).is_null()
            );
            assert_eq!(CStr::from_ptr(error).to_str(), Ok("The compiler is NULL"));
            scrawl_string_free(error);
            let compiler = scrawl_compiler_new();
            let mut error = ptr::null_mut();
            let times = scrawl_compiler_keyframe_times(
                compiler,
                c"not json".as_ptr(),
                ptr::null_mut(),
                &mut error,
            );
            assert!(times.is_null());
            assert!(
                CStr::from_ptr(error)
                    .to_str()
                    .unwrap()
                    .contains("valid JSON")
            );
            scrawl_string_free(error);
            // A NULL error slot is allowed.
            assert!(scrawl_compiler_compile(compiler, ptr::null(), ptr::null_mut()).is_null());
            assert_eq!(scrawl_bundle_file_count(ptr::null()), 0);
            assert!(scrawl_bundle_summary(ptr::null()).is_null());
            scrawl_compiler_free(compiler);
        }
    }
}
