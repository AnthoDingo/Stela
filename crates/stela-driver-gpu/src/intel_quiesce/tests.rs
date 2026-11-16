use super::*;
use alloc::{collections::BTreeMap, vec::Vec};
use core::cell::RefCell;

/// Registers that answer a write as the hardware does, as far as the quiesce
/// looks: forcewake acknowledges, a stopped ring goes idle, a requested
/// engine gets ready, a GT reset completes; each unless told otherwise.
struct Gt {
    registers: BTreeMap<usize, u32>,
    writes: Vec<(usize, u32)>,
    waited: u64,
    /// The forcewake domain, by request register, that never acknowledges.
    deaf: Option<usize>,
    /// An engine base whose ring never goes idle.
    stuck: Option<usize>,
    /// An engine base that never gets ready for reset.
    unready: Option<usize>,
    resets_complete: bool,
}

impl Gt {
    fn new() -> Self {
        Self {
            registers: BTreeMap::new(),
            writes: Vec::new(),
            waited: 0,
            deaf: None,
            stuck: None,
            unready: None,
            resets_complete: true,
        }
    }

    fn read(&self, offset: usize) -> u32 {
        self.registers.get(&offset).copied().unwrap_or(0)
    }

    fn write(&mut self, offset: usize, value: u32) {
        self.writes.push((offset, value));
        let set = |gt: &mut Self, offset, bits: u32, on: bool| {
            let old = gt.read(offset);
            gt.registers
                .insert(offset, if on { old | bits } else { old & !bits });
        };
        if let Some(domain) = FORCEWAKE.iter().find(|domain| domain.request == offset) {
            let on = value & FORCEWAKE_KERNEL != 0 && self.deaf != Some(offset);
            set(self, domain.ack, FORCEWAKE_KERNEL, on);
        } else if offset == GDRST {
            set(self, GDRST, GRDOM_FULL, !self.resets_complete);
        } else if let Some(engine) = ENGINES.iter().find(|engine| {
            offset == engine.base + RING_MI_MODE || offset == engine.base + RING_RESET_CTL
        }) {
            if offset == engine.base + RING_MI_MODE {
                let stopped = value == masked_enable(STOP_RING);
                set(
                    self,
                    offset,
                    MODE_IDLE,
                    stopped && self.stuck != Some(engine.base),
                );
            } else if value == masked_enable(REQUEST_RESET) {
                set(
                    self,
                    offset,
                    READY_TO_RESET,
                    self.unready != Some(engine.base),
                );
            } else if value == masked_disable(REQUEST_RESET) {
                set(self, offset, READY_TO_RESET, false);
            }
        }
    }
}

fn run(gt: &RefCell<Gt>) -> Result<Quiesced, Error<()>> {
    quiesce(
        |offset| Ok(gt.borrow().read(offset)),
        |offset, value| {
            gt.borrow_mut().write(offset, value);
            Ok(())
        },
        |micros| {
            gt.borrow_mut().waited += micros;
            Ok(())
        },
    )
}

fn released(gt: &Gt, domains: usize) -> bool {
    let tail = &gt.writes[gt.writes.len() - ENGINES.len() - domains..];
    ENGINES
        .iter()
        .map(|engine| (engine.base + RING_RESET_CTL, masked_disable(REQUEST_RESET)))
        .chain(
            FORCEWAKE[..domains]
                .iter()
                .map(|domain| (domain.request, masked_disable(FORCEWAKE_KERNEL))),
        )
        .eq(tail.iter().copied())
}

#[test]
fn an_idle_gt_is_held_awake_stopped_reset_twice_and_released() {
    let gt = RefCell::new(Gt::new());
    gt.borrow_mut()
        .registers
        .insert(ENGINES[0].base + RING_CTL, RING_VALID);
    assert_eq!(
        run(&gt),
        Ok(Quiesced {
            rings_enabled: 1,
            were_busy: 0
        })
    );
    let gt = gt.into_inner();
    let mut expected = FORCEWAKE
        .iter()
        .map(|domain| (domain.request, masked_enable(FORCEWAKE_KERNEL)))
        .chain(
            ENGINES
                .iter()
                .map(|engine| (engine.base + RING_MI_MODE, masked_enable(STOP_RING))),
        )
        .chain(
            ENGINES
                .iter()
                .map(|engine| (engine.base + RING_RESET_CTL, masked_enable(REQUEST_RESET))),
        )
        .chain([(GDRST, GRDOM_FULL), (GDRST, GRDOM_FULL)])
        .collect::<Vec<_>>();
    expected.extend_from_slice(&gt.writes[expected.len()..]);
    assert_eq!(gt.writes, expected);
    assert!(released(&gt, FORCEWAKE.len()));
    assert_eq!(gt.waited, SETTLE_MICROS);
}

#[test]
fn a_ring_that_does_not_go_idle_with_commands_left_is_refused_before_any_reset() {
    let gt = RefCell::new(Gt::new());
    {
        let mut gt = gt.borrow_mut();
        gt.stuck = Some(ENGINES[2].base);
        gt.registers.insert(ENGINES[2].base + RING_HEAD, 0x40);
        gt.registers.insert(ENGINES[2].base + RING_TAIL, 0x80);
    }
    assert_eq!(
        run(&gt),
        Err(Error::Busy {
            engine: "vcs0",
            mode: 0,
            head: 0x40,
            tail: 0x80
        })
    );
    let gt = gt.into_inner();
    assert!(!gt.writes.iter().any(|&(offset, _)| offset == GDRST));
    assert!(gt.waited >= STOP_MICROS);
    assert!(released(&gt, FORCEWAKE.len()));
}

#[test]
fn an_idle_flag_that_never_sets_over_an_empty_ring_counts_as_stopped() {
    let gt = RefCell::new(Gt::new());
    {
        let mut gt = gt.borrow_mut();
        gt.stuck = Some(ENGINES[1].base);
        // HEAD's low bits are not part of its address.
        gt.registers.insert(ENGINES[1].base + RING_HEAD, 0x81);
        gt.registers.insert(ENGINES[1].base + RING_TAIL, 0x80);
    }
    assert_eq!(
        run(&gt),
        Ok(Quiesced {
            rings_enabled: 0,
            were_busy: 0b10
        })
    );
}

#[test]
fn an_engine_that_does_not_get_ready_is_refused_instead_of_reset_anyway() {
    let gt = RefCell::new(Gt::new());
    gt.borrow_mut().unready = Some(ENGINES[3].base);
    assert_eq!(
        run(&gt),
        Err(Error::NotReady {
            engine: "vecs0",
            reset_ctl: 0
        })
    );
    let gt = gt.into_inner();
    assert!(!gt.writes.iter().any(|&(offset, _)| offset == GDRST));
    assert!(released(&gt, FORCEWAKE.len()));
}

#[test]
fn a_catastrophic_error_is_refused() {
    let gt = RefCell::new(Gt::new());
    gt.borrow_mut()
        .registers
        .insert(ENGINES[0].base + RING_RESET_CTL, CAT_ERROR);
    assert_eq!(
        run(&gt),
        Err(Error::Catastrophic {
            engine: "rcs0",
            reset_ctl: CAT_ERROR
        })
    );
}

#[test]
fn a_domain_that_does_not_wake_is_refused_and_only_held_domains_are_released() {
    let gt = RefCell::new(Gt::new());
    gt.borrow_mut().deaf = Some(FORCEWAKE[1].request);
    assert_eq!(
        run(&gt),
        Err(Error::Forcewake {
            domain: "gt",
            ack: 0
        })
    );
    let gt = gt.into_inner();
    assert!(gt.waited >= FORCEWAKE_MICROS);
    assert!(released(&gt, 2));
}

#[test]
fn a_gt_reset_that_does_not_complete_is_refused() {
    let gt = RefCell::new(Gt::new());
    gt.borrow_mut().resets_complete = false;
    assert_eq!(run(&gt), Err(Error::Reset { gdrst: GRDOM_FULL }));
    assert!(released(&gt.into_inner(), FORCEWAKE.len()));
}

#[test]
fn masked_writes_carry_the_bits_to_change_in_their_high_half() {
    assert_eq!(masked_enable(STOP_RING), 0x0100_0100);
    assert_eq!(masked_disable(REQUEST_RESET), 0x0001_0000);
}
