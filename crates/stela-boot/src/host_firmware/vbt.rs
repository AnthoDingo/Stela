// SPDX-License-Identifier: GPL-2.0-or-later
//! The Intel GPU's Video BIOS Table, read from its ACPI OpRegion while boot
//! services still map it, as SMBIOS is ([`super::smbios`]): i915 reads it
//! through ACPI, which Stela does not build, and without it does not know the
//! laptop's panel is on eDP, its power sequence or its backlight. Where the
//! VBT lies is i915's rule (`stela_drivers::gpu::opregion`). The GPU's start
//! hands it to Linux as the GPU's ROM image, which i915 searches for a VBT
//! when it has none from the OpRegion.
use crate::output;
use alloc::{vec, vec::Vec};
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use stela_boot_core::host_resources::smbios::{Span, grant};
use stela_drivers::gpu::opregion;
use uefi::{
    boot::{self, MemoryType},
    mem::memory_map::MemoryMap,
};

/// `struct vbt_header`'s signature.
const SIGNATURE: &[u8; 4] = b"$VBT";
/// The most a raw VBT may be: i915 maps what RVDS says, Stela copies it.
const MAX_RAW: usize = 64 * 1024;

/// The VBT [`capture`] kept for the boot, published once.
static VBT: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
static BYTES: AtomicUsize = AtomicUsize::new(0);

/// Copy `output.len()` bytes at physical `address`, when the firmware's
/// memory map describes them as data or ACPI memory, as SMBIOS's tables are
/// read. Only while boot services run.
fn read(address: u64, output: &mut [u8]) -> Result<(), &'static str> {
    let map = boot::memory_map(MemoryType::LOADER_DATA).map_err(|_| "no memory map")?;
    let cr4: u64;
    // SAFETY: read-only x64 control-register observation during UEFI.
    unsafe {
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
    }
    grant(
        map.entries().map(|entry| Span {
            start: entry.phys_start,
            pages: entry.page_count,
            kind: entry.ty,
            attributes: entry.att.bits(),
        }),
        address,
        output.len(),
        if cr4 & (1 << 12) != 0 { 56 } else { 47 },
    )
    .map_err(|_| "not described as readable memory")?;
    // SAFETY: the extent was just proved readable data or ACPI memory in the
    // firmware's current map, boot services are live and UEFI identity-maps
    // described memory; nothing of Stela's own paging is installed yet.
    unsafe {
        core::ptr::copy_nonoverlapping(address as *const u8, output.as_mut_ptr(), output.len());
    }
    Ok(())
}

/// Read the VBT of the OpRegion at `asls` and keep it for the GPU's start,
/// saying where it came from or why there is none.
pub(crate) fn capture(asls: u64) {
    let mut region = vec![0u8; opregion::SIZE];
    if let Err(why) = read(asls, &mut region) {
        output::arguments(format_args!("DISPLAY OpRegion at {asls:#x}: {why}\n"));
        return;
    }
    let Some(places) = opregion::locate(asls, region.as_slice().try_into().expect("OpRegion size"))
    else {
        output::arguments(format_args!("DISPLAY OpRegion at {asls:#x}: no OpRegion signature\n"));
        return;
    };
    // The raw VBT first, when it is one, then mailbox #4, as i915 tries them.
    let raw = places.raw.and_then(|(address, bytes)| {
        let mut vbt = vec![0u8; bytes];
        (bytes <= MAX_RAW && read(address, &mut vbt).is_ok() && vbt.starts_with(SIGNATURE))
            .then_some(vbt)
    });
    let (vbt, from): (Vec<u8>, &str) = match raw {
        Some(vbt) => (vbt, "raw data"),
        None => {
            let (offset, bytes) = places.mailbox;
            (region[offset..offset + bytes].to_vec(), "mailbox #4")
        }
    };
    let (major, minor) = places.version;
    if !vbt.starts_with(SIGNATURE) {
        output::arguments(format_args!(
            "DISPLAY OpRegion {major}.{minor} at {asls:#x}: no VBT\n"
        ));
        return;
    }
    output::arguments(format_args!(
        "DISPLAY VBT: {} bytes in OpRegion {major}.{minor} at {asls:#x}, {from}\n",
        vbt.len()
    ));
    let kept = vbt.leak();
    BYTES.store(kept.len(), Ordering::Release);
    VBT.store(kept.as_mut_ptr(), Ordering::Release);
}

/// The VBT [`capture`] kept, if any.
pub(crate) fn captured() -> Option<&'static [u8]> {
    let pointer = VBT.load(Ordering::Acquire);
    if pointer.is_null() {
        return None;
    }
    // SAFETY: a leaked allocation published whole by `capture`, never freed or
    // written again.
    Some(unsafe { core::slice::from_raw_parts(pointer, BYTES.load(Ordering::Acquire)) })
}
