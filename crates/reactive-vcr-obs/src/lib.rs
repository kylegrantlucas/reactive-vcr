//! OBS Studio plugin: a video filter that applies reactive-vcr's ntsc-rs and bender-rs effects
//! to any source, driven live by an OBS audio source the user picks.
//!
//! The plugin links nothing at build time; libobs symbols resolve when OBS loads it.

mod ffi;
mod filter;

use ffi::*;
use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicPtr, Ordering};

static MODULE: AtomicPtr<obs_module_t> = AtomicPtr::new(std::ptr::null_mut());

#[unsafe(no_mangle)]
pub extern "C" fn obs_module_set_pointer(module: *mut obs_module_t) {
    MODULE.store(module, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn obs_current_module() -> *mut obs_module_t {
    MODULE.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn obs_module_ver() -> u32 {
    LIBOBS_API_VER
}

#[unsafe(no_mangle)]
pub extern "C" fn obs_module_name() -> *const c_char {
    c"reactive-vcr".as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn obs_module_description() -> *const c_char {
    c"Audio-reactive ntsc-rs VHS and bender-rs filter".as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn obs_module_load() -> bool {
    let info = filter::source_info();
    // SAFETY: libobs copies the struct during the call, and `info` is a complete
    // OBS 30.0.0 `obs_source_info`.
    unsafe { obs_register_source_s(&info, size_of::<obs_source_info>()) };
    true
}

/// Log through OBS, prefixed with the plugin name.
pub(crate) fn log(level: std::ffi::c_int, message: &str) {
    let message = CString::new(message.replace('\0', "")).expect("NULs removed");
    // SAFETY: both strings are NUL-terminated and the format consumes one `char *`.
    unsafe { blog(level, c"[reactive-vcr] %s".as_ptr(), message.as_ptr()) };
}

/// Run an OBS callback body; a panic is logged and `fallback` runs instead, since unwinding
/// into C is undefined behavior.
pub(crate) fn guard<R>(
    callback: &CStr,
    fallback: impl FnOnce() -> R,
    body: impl FnOnce() -> R,
) -> R {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => value,
        Err(panic) => {
            let detail = panic
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown panic");
            log(
                LOG_WARNING,
                &format!("{} panicked: {detail}", callback.to_string_lossy()),
            );
            fallback()
        }
    }
}
