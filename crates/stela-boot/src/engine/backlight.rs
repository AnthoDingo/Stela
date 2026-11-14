//! The panel backlight of a Skylake or Kaby Lake laptop, stepped by its
//! brightness keys. The guest is a Mac mini, whose displays have no
//! brightness of its own to set, so the host keeps this: the firmware
//! notifies the panel's ACPI output (`_BCM`) with 0x86 or 0x87, as for
//! Linux's `acpi_video`, and each notification moves the PWM duty cycle in
//! BLC_PWM_PCH_CTL2 by a step (`stela_boot_core::backlight`).
use core::sync::atomic::{AtomicUsize, Ordering};
use stela_boot_core::backlight;

/// The engine's uncached mapping of BLC_PWM_PCH_CTL1, or 0.
static CTL1: AtomicUsize = AtomicUsize::new(0);

/// Keep the registers the loader observed and mapped, if any.
pub(super) fn install(registers: Option<usize>) {
    if let Some(registers) = registers {
        CTL1.store(registers, Ordering::Release);
    }
}

pub(super) fn available() -> bool {
    CTL1.load(Ordering::Acquire) != 0
}

/// ACPI's brightness notifications for a display output: 0x86 brighter,
/// 0x87 dimmer (ACPI 6.5, B.7). Others are not the backlight's.
pub(super) fn notified(value: u32) {
    match value {
        0x86 => step(true),
        0x87 => step(false),
        _ => {}
    }
}

fn step(up: bool) {
    let ctl1 = CTL1.load(Ordering::Acquire);
    if ctl1 == 0 {
        return;
    }
    let ctl2 = (ctl1 + backlight::CTL2_FROM_CTL1) as *mut u32;
    // SAFETY: `ctl1` is the uncached mapping of the graphics function's
    // BLC_PWM_PCH_CTL1 the loader observed running (`pci_io::intel_backlight`)
    // and mapped with its page; CTL2 is the next register on that page. No
    // driver owns the function, and only the deferred ACPI work, on one
    // processor, steps it.
    unsafe {
        let value = ctl2.read_volatile();
        ctl2.write_volatile(backlight::step(value, up));
    }
}
