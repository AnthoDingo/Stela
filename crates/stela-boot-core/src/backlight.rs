//! The panel backlight that Skylake and Kaby Lake graphics drive through
//! their south display engine, as i915's `lpt_setup_backlight`,
//! `lpt_get_backlight` and `lpt_set_backlight` do
//! (drivers/gpu/drm/i915/display/intel_backlight.c at Linux
//! a804c5742bff224d4cb4d25dd383554d598be32b). BLC_PWM_PCH_CTL1 enables the
//! PWM; BLC_PWM_PCH_CTL2 holds its period in the high half and, in the low
//! half, the duty cycle that sets the brightness.
//!
//! Later PCHs (Cannon Point on) split period and duty into two registers,
//! i915's `cnp_*_backlight`: they are not this layout.

/// BLC_PWM_PCH_CTL1's offset in the graphics function's MMIO BAR (GTTMMADR).
pub const CTL1: u64 = 0xc8250;
/// BLC_PWM_PCH_CTL2, next to it.
pub const CTL2_FROM_CTL1: usize = 4;
/// `BLM_PCH_PWM_ENABLE`.
pub const ENABLE: u32 = 1 << 31;
/// The brightness keys move the duty by a sixteenth of the period.
pub const STEPS: u32 = 16;

/// Skylake (0x19xx) and Kaby Lake (0x59xx) integrated graphics, whose PCH is
/// Sunrise Point or Kaby Point: the layout above.
pub fn graphics(vendor: u16, device: u16) -> bool {
    vendor == 0x8086 && matches!(device >> 8, 0x19 | 0x59)
}

/// The PWM period: `pwm_level_max`.
pub fn period(ctl2: u32) -> u32 {
    ctl2 >> 16
}
/// The duty cycle: `lpt_get_backlight`.
pub fn duty(ctl2: u32) -> u32 {
    ctl2 & 0xffff
}

/// What firmware left running and Stela may step: the PWM on, its period
/// set, and the duty within it.
pub fn usable(ctl1: u32, ctl2: u32) -> bool {
    ctl1 & ENABLE != 0 && period(ctl2) != 0 && duty(ctl2) <= period(ctl2)
}

/// CTL2 one step brighter or dimmer, its period kept. One step is the
/// dimmest it goes, so a key never turns the panel off.
pub fn step(ctl2: u32, up: bool) -> u32 {
    let period = period(ctl2);
    let step = (period / STEPS).max(1);
    let duty = duty(ctl2);
    let next = if up {
        duty.saturating_add(step).min(period)
    } else {
        duty.saturating_sub(step).max(step)
    };
    (ctl2 & !0xffff) | next
}

/// CTL2 to write back after the GPU's reset: what firmware left (`before`),
/// when it had a period and the reset left none (`after`). i915 starts the
/// panel at the duty it reads (`lpt_setup_backlight`), as Linux inherits it
/// from firmware without resetting the GPU, so a cleared CTL2 lights the
/// panel at 0.
pub fn inherited(before: u32, after: u32) -> Option<u32> {
    (period(before) != 0 && duty(before) <= period(before) && period(after) == 0)
        .then_some(before)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Latitude 7490: a 7500-cycle period, its duty at about half.
    const LATITUDE_7490: u32 = (7500 << 16) | 3764;

    #[test]
    fn only_skylake_and_kaby_lake_graphics_have_this_layout() {
        for device in [0x5917, 0x5916, 0x591b, 0x1916, 0x191b] {
            assert!(graphics(0x8086, device), "{device:04x}");
        }
        // Whiskey Lake (Cannon Point PCH), Ice Lake, Tiger Lake, another vendor.
        for (vendor, device) in [
            (0x8086, 0x3ea0),
            (0x8086, 0x8a52),
            (0x8086, 0x9a49),
            (0x10de, 0x5917),
        ] {
            assert!(!graphics(vendor, device), "{vendor:04x}:{device:04x}");
        }
    }

    #[test]
    fn a_running_pwm_with_a_period_is_usable_and_nothing_else() {
        assert!(usable(ENABLE, LATITUDE_7490));
        assert!(!usable(0, LATITUDE_7490));
        assert!(!usable(ENABLE, 3764));
        assert!(!usable(ENABLE, (100 << 16) | 101));
    }

    #[test]
    fn a_step_moves_a_sixteenth_keeps_the_period_and_never_turns_the_panel_off() {
        let up = step(LATITUDE_7490, true);
        assert_eq!((period(up), duty(up)), (7500, 3764 + 468));
        let down = step(LATITUDE_7490, false);
        assert_eq!((period(down), duty(down)), (7500, 3764 - 468));
        // Brightest stays brightest.
        let full = (7500 << 16) | 7400;
        assert_eq!(duty(step(full, true)), 7500);
        assert_eq!(duty(step(step(full, true), true)), 7500);
        // Dimmest is one step, not off, even from below it.
        let dim = (7500 << 16) | 100;
        assert_eq!(duty(step(dim, false)), 468);
        let mut level = LATITUDE_7490;
        for _ in 0..40 {
            level = step(level, false);
        }
        assert_eq!(duty(level), 468);
        // A tiny period still moves.
        assert_eq!(duty(step((8 << 16) | 4, true)), 5);
    }

    #[test]
    fn firmwares_level_returns_only_where_the_reset_cleared_the_period() {
        assert_eq!(inherited(LATITUDE_7490, 0), Some(LATITUDE_7490));
        assert_eq!(inherited(LATITUDE_7490, 3764), Some(LATITUDE_7490));
        // The reset kept it, or firmware left nothing sound to return.
        assert_eq!(inherited(LATITUDE_7490, LATITUDE_7490), None);
        assert_eq!(inherited(0, 0), None);
        assert_eq!(inherited((100 << 16) | 101, 0), None);
    }
}
