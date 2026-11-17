//! i915 in a Linux environment: the initcalls that register it, DRM and the
//! subsystems beneath them, which `build/port.rs` captures.
use core::{
    ffi::{c_int, c_void},
    ptr,
};
use stela_linux::owner::Errno;

unsafe extern "C" {
    static stela_linux_i915_image: c_void;
    fn stela_i915_display_params();
    fn stela_linux_intel_stolen(start: u64, size: u64) -> c_int;
    fn stela_linux_start(image: *const c_void, registry: *mut *mut c_void) -> c_int;
}

/// Bring Linux up as far as a device can bind, stopping at the first stage
/// that fails. Each environment does this once.
///
/// # Safety
/// On the canonical Linux owner's original managed task after Initial and
/// SRCU startup have completed. Initcalls may sleep.
pub unsafe fn start() -> Result<(), Errno> {
    // SAFETY: the caller's contract: the only Linux task, before i915 copies
    // its display parameters.
    unsafe { stela_i915_display_params() };
    let mut registry = ptr::null_mut();
    // SAFETY: the caller's contract; the image is the build's, and the
    // registry's state lasts as long as what it started.
    match unsafe { stela_linux_start(&raw const stela_linux_i915_image, &raw mut registry) } {
        0 => Ok(()),
        error => Err(error),
    }
}

/// Report the GPU's stolen memory, `size` bytes at `start`, as x86's early
/// quirks do, and lend it to i915, which requests it.
///
/// # Safety
/// Before [`start`], on the canonical Linux owner's task. The range is the
/// GPU's stolen memory, which firmware reserved and nothing else uses.
pub unsafe fn report_stolen(start: u64, size: u64) -> Result<(), Errno> {
    // SAFETY: the caller's contract.
    match unsafe { stela_linux_intel_stolen(start, size) } {
        0 => Ok(()),
        error => Err(error),
    }
}
