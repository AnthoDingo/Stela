//! Stopping and resetting an Intel graphics version 9 GPU's engines through
//! its own registers before Stela resets the function, as the Intel port's
//! decision requires (quiesce, then FLR). Only MMIO: nothing here needs bus
//! mastering, which stays off.
//!
//! The sequence follows i915 (Linux v7.2.12): hold forcewake for every domain
//! (`intel_uncore.c`, `fw_domain_init` for graphics versions 9 and 10), stop
//! each engine's command streamer (`gt/intel_engine_cs.c`,
//! `__intel_engine_stop_cs` and `intel_engine_stop_cs`), ask each engine to
//! get ready for reset (`gt/intel_reset.c`, `gen8_engine_reset_prepare`),
//! reset the whole GT twice through `GEN6_GDRST` and let it settle
//! (`gen6_hw_domain_reset`), then cancel the requests
//! (`gen8_engine_reset_cancel`) and release forcewake. Where i915 resets
//! anyway on a second attempt, with the warning that on Kaby Lake this may
//! hang the system if a batch is executing (`gen8_reset_engines`), this
//! refuses instead. Masked registers take the bits to change in their high
//! half (`REG_MASKED_FIELD_ENABLE`).

/// The GT's forcewake domains on graphics versions 9 and 10: request and
/// acknowledge registers (`gt/intel_gt_regs.h`).
pub const FORCEWAKE: [Domain; 3] = [
    Domain {
        name: "render",
        request: 0xa278,
        ack: 0x0d84,
    },
    Domain {
        name: "gt",
        request: 0xa188,
        ack: 0x13_0044,
    },
    Domain {
        name: "media",
        request: 0xa270,
        ack: 0x0d88,
    },
];
/// `FORCEWAKE_KERNEL`, the bit i915 requests and waits to see acknowledged.
pub const FORCEWAKE_KERNEL: u32 = 1;
/// `FORCEWAKE_ACK_TIMEOUT_MS`.
pub const FORCEWAKE_MICROS: u64 = 50_000;

/// The engines of graphics version 9 (`G75_FEATURES`' engine mask), by MMIO
/// base (`i915_reg.h`); the Latitude 7490's i915 lists the same four.
pub const ENGINES: [Engine; 4] = [
    Engine {
        name: "rcs0",
        base: 0x0_2000,
    },
    Engine {
        name: "bcs0",
        base: 0x2_2000,
    },
    Engine {
        name: "vcs0",
        base: 0x1_2000,
    },
    Engine {
        name: "vecs0",
        base: 0x1_a000,
    },
];
/// Per-engine registers, as offsets from the engine's base
/// (`gt/intel_engine_regs.h`).
pub const RING_TAIL: usize = 0x30;
pub const RING_HEAD: usize = 0x34;
pub const RING_CTL: usize = 0x3c;
pub const RING_MI_MODE: usize = 0x9c;
pub const RING_RESET_CTL: usize = 0xd0;
/// `RING_VALID` in `RING_CTL`: the ring is enabled.
pub const RING_VALID: u32 = 1;
/// `STOP_RING` and `MODE_IDLE` in `RING_MI_MODE`.
pub const STOP_RING: u32 = 1 << 8;
pub const MODE_IDLE: u32 = 1 << 9;
/// `HEAD_ADDR` and `TAIL_ADDR`.
pub const HEAD_ADDR: u32 = 0x001f_fffc;
pub const TAIL_ADDR: u32 = 0x001f_fff8;
/// `RESET_CTL_REQUEST_RESET`, `RESET_CTL_READY_TO_RESET` and
/// `RESET_CTL_CAT_ERROR` in `RING_RESET_CTL`.
pub const REQUEST_RESET: u32 = 1 << 0;
pub const READY_TO_RESET: u32 = 1 << 1;
pub const CAT_ERROR: u32 = 1 << 2;
/// `GEN6_GDRST` and its `GEN6_GRDOM_FULL` domain.
pub const GDRST: usize = 0x941c;
pub const GRDOM_FULL: u32 = 1;

/// `intel_engine_stop_cs`: 1 ms of fast polling, then
/// `CONFIG_DRM_I915_STOP_TIMEOUT` (100 ms).
pub const STOP_MICROS: u64 = 1_000 + 100_000;
/// `gen8_engine_reset_prepare`'s wait for readiness.
pub const READY_MICROS: u64 = 700;
/// `gen6_hw_domain_reset`'s wait for each reset to complete, the number of
/// resets below graphics version 12.70, and the settling delay after them.
pub const RESET_MICROS: u64 = 2_000;
pub const RESETS: u32 = 2;
pub const SETTLE_MICROS: u64 = 50;
/// Polling interval of every wait above.
pub const POLL_MICROS: u64 = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Domain {
    pub name: &'static str,
    pub request: usize,
    pub ack: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Engine {
    pub name: &'static str,
    pub base: usize,
}

/// Why the engines were not quiesced, with what the registers held.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error<E> {
    /// A register access or the timer failed.
    Access(E),
    /// A forcewake domain did not acknowledge within its bound.
    Forcewake { domain: &'static str, ack: u32 },
    /// An engine's command streamer did not go idle, and its ring still had
    /// commands (`HEAD` short of `TAIL`).
    Busy {
        engine: &'static str,
        mode: u32,
        head: u32,
        tail: u32,
    },
    /// An engine reports a catastrophic error, which i915 resets around
    /// without the readiness handshake.
    Catastrophic {
        engine: &'static str,
        reset_ctl: u32,
    },
    /// An engine did not become ready for reset within i915's bound.
    NotReady {
        engine: &'static str,
        reset_ctl: u32,
    },
    /// The full GT reset did not complete within i915's bound.
    Reset { gdrst: u32 },
}

/// What the engines held before the quiesce, for the boot log.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Quiesced {
    /// Engines, by index in [`ENGINES`], whose ring firmware left enabled.
    pub rings_enabled: u8,
    /// Engines that were executing when asked to stop (not idle at once).
    pub were_busy: u8,
}

/// The value a masked register write sets `bits` with.
pub const fn masked_enable(bits: u32) -> u32 {
    bits << 16 | bits
}

/// The value a masked register write clears `bits` with.
pub const fn masked_disable(bits: u32) -> u32 {
    bits << 16
}

/// Quiesce the GT through `read` and `write` (BAR0 register accesses) and
/// `delay` (a busy wait of the given microseconds). On failure the reset
/// requests are cancelled and the forcewake domains held so far released, as
/// best they can be, and the GPU must be refused.
pub fn quiesce<E>(
    mut read: impl FnMut(usize) -> Result<u32, E>,
    mut write: impl FnMut(usize, u32) -> Result<(), E>,
    mut delay: impl FnMut(u64) -> Result<(), E>,
) -> Result<Quiesced, Error<E>> {
    let mut held = 0;
    let result = run(&mut read, &mut write, &mut delay, &mut held);
    // Cancelling a request never made changes nothing, so every engine's is.
    let mut release = || -> Result<(), E> {
        for engine in ENGINES {
            write(engine.base + RING_RESET_CTL, masked_disable(REQUEST_RESET))?;
        }
        for domain in &FORCEWAKE[..held] {
            write(domain.request, masked_disable(FORCEWAKE_KERNEL))?;
        }
        Ok(())
    };
    let released = release();
    let quiesced = result?;
    released.map_err(Error::Access)?;
    Ok(quiesced)
}

/// Read `offset` until `done` holds of it, for at most `micros`: the value
/// that satisfied it, or the last one read.
fn poll<E>(
    read: &mut impl FnMut(usize) -> Result<u32, E>,
    delay: &mut impl FnMut(u64) -> Result<(), E>,
    offset: usize,
    done: impl Fn(u32) -> bool,
    micros: u64,
) -> Result<Result<u32, u32>, Error<E>> {
    let mut waited = 0;
    loop {
        let value = read(offset).map_err(Error::Access)?;
        if done(value) {
            return Ok(Ok(value));
        }
        if waited >= micros {
            return Ok(Err(value));
        }
        delay(POLL_MICROS).map_err(Error::Access)?;
        waited += POLL_MICROS;
    }
}

fn run<E>(
    read: &mut impl FnMut(usize) -> Result<u32, E>,
    write: &mut impl FnMut(usize, u32) -> Result<(), E>,
    delay: &mut impl FnMut(u64) -> Result<(), E>,
    held: &mut usize,
) -> Result<Quiesced, Error<E>> {
    for domain in FORCEWAKE {
        write(domain.request, masked_enable(FORCEWAKE_KERNEL)).map_err(Error::Access)?;
        *held += 1;
        let acked = |ack| ack & FORCEWAKE_KERNEL != 0;
        if let Err(ack) = poll(read, delay, domain.ack, acked, FORCEWAKE_MICROS)? {
            return Err(Error::Forcewake {
                domain: domain.name,
                ack,
            });
        }
    }
    let mut quiesced = Quiesced::default();
    let idle = |mode| mode & MODE_IDLE != 0;
    for (index, engine) in ENGINES.iter().enumerate() {
        let bit = 1 << index;
        if read(engine.base + RING_CTL).map_err(Error::Access)? & RING_VALID != 0 {
            quiesced.rings_enabled |= bit;
        }
        let mode = engine.base + RING_MI_MODE;
        write(mode, masked_enable(STOP_RING)).map_err(Error::Access)?;
        if poll(read, delay, mode, idle, 0)?.is_ok() {
            continue;
        }
        quiesced.were_busy |= bit;
        if let Err(mode) = poll(read, delay, mode, idle, STOP_MICROS)? {
            // As i915: an idle flag that never sets over an empty ring.
            let head = read(engine.base + RING_HEAD).map_err(Error::Access)? & HEAD_ADDR;
            let tail = read(engine.base + RING_TAIL).map_err(Error::Access)? & TAIL_ADDR;
            if head != tail {
                return Err(Error::Busy {
                    engine: engine.name,
                    mode,
                    head,
                    tail,
                });
            }
        }
    }
    for engine in ENGINES {
        let control = engine.base + RING_RESET_CTL;
        let reset_ctl = read(control).map_err(Error::Access)?;
        if reset_ctl & CAT_ERROR != 0 {
            return Err(Error::Catastrophic {
                engine: engine.name,
                reset_ctl,
            });
        }
        if reset_ctl & READY_TO_RESET != 0 {
            continue;
        }
        write(control, masked_enable(REQUEST_RESET)).map_err(Error::Access)?;
        let ready = |ctl| ctl & READY_TO_RESET != 0;
        if let Err(reset_ctl) = poll(read, delay, control, ready, READY_MICROS)? {
            return Err(Error::NotReady {
                engine: engine.name,
                reset_ctl,
            });
        }
    }
    for _ in 0..RESETS {
        write(GDRST, GRDOM_FULL).map_err(Error::Access)?;
        let complete = |gdrst| gdrst & GRDOM_FULL == 0;
        if let Err(gdrst) = poll(read, delay, GDRST, complete, RESET_MICROS)? {
            return Err(Error::Reset { gdrst });
        }
    }
    delay(SETTLE_MICROS).map_err(Error::Access)?;
    Ok(quiesced)
}

#[cfg(test)]
mod tests;
