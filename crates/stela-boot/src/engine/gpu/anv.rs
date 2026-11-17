//! ANV in an Intel image, before a Metal device opens over it (the Intel
//! port's Phase 4): once the display is taken, a Vulkan instance through it
//! and the GPUs it enumerates, logged.
use super::stack::{self, Stack};
use crate::output;
use core::ffi::{c_char, c_void};
use stela_drivers::gpu::metal::native::{self, vk};

/// What Mesa's code runs on, as NVK's service does (`gate::STACK_BYTES`).
const STACK_BYTES: usize = 16 << 20;

/// ANV's entry point, as Vulkan types it.
unsafe extern "system" fn entry(
    instance: vk::Instance,
    name: *const c_char,
) -> vk::PFN_vkVoidFunction {
    use vk::Handle;
    // SAFETY: the loader's call, and both ends are one function pointer.
    unsafe {
        core::mem::transmute(stela_mesa::vk_icdGetInstanceProcAddr(
            instance.as_raw() as *mut c_void,
            name,
        ))
    }
}

/// Runs the image's C constructors, then logs what ANV enumerates.
pub(super) fn probe() {
    let Some(window) = super::installed().map(|installed| installed.window) else {
        return;
    };
    let Some(stack) = Stack::map(window, STACK_BYTES) else {
        output::arguments(format_args!("GPU ANV: no memory for its stack\n"));
        return;
    };
    let mut body = || {
        // SAFETY: the one call, before any other of the C's.
        unsafe { stela_mesa::stela_constructors() };
        // SAFETY: ANV's loader entry, linked into the image.
        match unsafe { native::enumerate(entry) } {
            Ok(devices) if devices.is_empty() => {
                output::arguments(format_args!("GPU ANV: no physical device\n"));
            }
            Ok(devices) => {
                for device in devices {
                    output::arguments(format_args!("GPU ANV physical device {device}\n"));
                }
            }
            Err(error) => output::arguments(format_args!("GPU ANV: no instance: {error:?}\n")),
        }
    };
    // SAFETY: the stack is this call's alone, and outlives it.
    unsafe { stack::on_stack(stack.top(), &mut body) };
}
