//! ANV in an Intel image, before a Metal device opens over it (the Intel
//! port's Phase 4): once the display is taken, a Vulkan instance through it,
//! the GPUs it enumerates and what a Metal device over each would derive
//! (`native::assess`), logged for the checkpoint before Phase 5.
use super::stack::{self, Stack};
use crate::output;
use alloc::string::String;
use core::ffi::{c_char, c_void};
use stela_drivers::gpu::metal::{
    abi::DeviceLocation,
    capabilities::Platform,
    native::{self, vk},
};

/// What Mesa's code runs on, as NVK's service does (`gate::STACK_BYTES`).
const STACK_BYTES: usize = 16 << 20;

/// The Latitude 7490's graphics, built in, for the checkpoint's derivation.
/// Provisional until `metal/intel.rs` (Phase 5): the linear alignments are
/// ISL's 64-byte row pitch and a page, and the sparse page is ANV's 64 KiB
/// block (`anv_private.h`).
fn platform() -> Platform {
    Platform {
        architecture: String::from("Kaby Lake GT2"),
        location: DeviceLocation::BuiltIn,
        location_number: 0,
        max_transfer_rate: 0,
        headless: false,
        removable: false,
        linear_row_alignment: 64,
        linear_offset_alignment: 4096,
        sparse_page_bytes: 64 << 10,
    }
}

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

/// Logs what ANV enumerates and what a Metal device over it would derive.
/// ANV, NIR and brw have no static constructors,
/// and without the Metal compiler no LLVM is linked to bring any, so
/// `stela_constructors`, which refuses an image without one, is not run.
pub(super) fn probe() {
    let Some(window) = super::installed().map(|installed| installed.window) else {
        return;
    };
    let Some(stack) = Stack::map(window, STACK_BYTES) else {
        output::arguments(format_args!("GPU ANV: no memory for its stack\n"));
        return;
    };
    let mut body = || {
        // SAFETY: ANV's loader entry, linked into the image.
        match unsafe { native::assess(entry, &platform()) } {
            Ok(lines) if lines.is_empty() => {
                output::arguments(format_args!("GPU ANV: no physical device\n"));
            }
            Ok(lines) => {
                for line in lines {
                    output::arguments(format_args!("GPU ANV {line}\n"));
                }
            }
            Err(error) => output::arguments(format_args!("GPU ANV: no instance: {error:?}\n")),
        }
    };
    // SAFETY: the stack is this call's alone, and outlives it.
    unsafe { stack::on_stack(stack.top(), &mut body) };
}
