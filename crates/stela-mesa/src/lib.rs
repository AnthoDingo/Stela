//! Mesa's Vulkan driver, linked into `stela.efi` for its Metal device: NVK,
//! NIR and NAK (feature `nvk`), or ANV, NIR and brw (feature `anv`).
//!
//! The C is the archive `cargo xtask gpu nvk` or `anv` builds; NVK's Rust is
//! NAK and NIL as dependencies of this crate. Nothing but the driver's loader
//! entry and the start of the C is called from here: Mesa reaches its host
//! through `compat/libc`, and the Metal compiler is `metal::compiler`'s.
#![no_std]

#[cfg(feature = "nvk")]
extern crate nak;
#[cfg(feature = "nvk")]
extern crate nil;
#[cfg(all(feature = "nvk", feature = "anv"))]
compile_error!("an image carries one Vulkan driver: `nvk` or `anv`");

use core::ffi::{c_char, c_void};

/// The SHA-256 of what turns NIR into machine code (`build.rs`), which
/// keys the Metal device's pipeline store.
pub const COMPILER_SHA256: &str = env!("STELA_MESA_COMPILER_SHA256");

unsafe extern "system" {
    /// The Vulkan loader-ICD entry of NVK.
    pub fn vk_icdGetInstanceProcAddr(instance: *mut c_void, name: *const c_char) -> *const c_void;
}

unsafe extern "C" {
    /// Runs the image's C and C++ static constructors, before anything else
    /// of theirs.
    pub fn stela_constructors();
}
