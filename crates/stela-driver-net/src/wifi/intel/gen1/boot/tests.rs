// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright (C) 2003-2014, 2018-2026 Intel Corporation
//! An 8265 and 9560 model: CSRs, the peripheral space, the FH service
//! channel copying into a modeled SRAM, and a firmware that answers the end
//! of the CPU2 load with ALIVE on RX queue 0, legacy or RFH, then each
//! command on queue 0 with scripted packets. The oracle cases drive it with
//! Linux's own reads and packets and compare the accesses with Linux's.
extern crate std;
use super::*;
use crate::{
    firmware::{Catalog, Provenance, Resource, Target},
    wifi::intel::{
        boot::BootIo,
        dma::tests::{Domain, Source},
        gen1::Profile,
        identity::{DEVICE_REVISION, Observed},
        power::{AccessError, Csr, Silicon},
    },
};
use alloc::{
    collections::{BTreeMap, VecDeque},
    string::String,
    vec,
};
use sha2::{Digest, Sha256};
use std::sync::Mutex;

/// CSR_INT_BIT_ALIVE.
const INT_ALIVE: u32 = 1;

/// One access as Linux's trace records it: peripheral accesses as such,
/// never as the CSR pair that carries them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::wifi::intel) enum Event {
    Read(u32),
    Write(u32, u32),
    Write8(u32, u32),
    PrphRead(u32),
    PrphWrite(u32, u32),
}

pub(in crate::wifi::intel) struct Device {
    pub(in crate::wifi::intel) registers: BTreeMap<u32, u32>,
    pub(in crate::wifi::intel) prph: BTreeMap<u32, u32>,
    /// Linux's read values, per register and per peripheral address.
    pub(in crate::wifi::intel) replay: BTreeMap<u32, VecDeque<u32>>,
    pub(in crate::wifi::intel) replay_prph: BTreeMap<u32, VecDeque<u32>>,
    prph_read: u32,
    prph_write: u32,
    pub(in crate::wifi::intel) events: Vec<Event>,
    sram: Vec<(u32, Vec<u8>)>,
    domain: Arc<Domain>,
    alive: Vec<u8>,
    /// Packets sent right after ALIVE, then after each command in turn.
    pub(in crate::wifi::intel) after_alive: Vec<Vec<u8>>,
    pub(in crate::wifi::intel) responses: VecDeque<Vec<Vec<u8>>>,
    /// Every command read off queue 0, whole.
    pub(in crate::wifi::intel) commands: Vec<Vec<u8>>,
    /// RBs closed, and queue 0's next TFD.
    closed: u16,
    command_read: u16,
}

#[derive(Clone)]
pub(in crate::wifi::intel) struct Model(pub(in crate::wifi::intel) Arc<Mutex<Device>>);

impl Model {
    pub(in crate::wifi::intel) fn new(
        domain: Arc<Domain>,
        alive: Vec<u8>,
        observed: Observed,
    ) -> Self {
        Self(Arc::new(Mutex::new(Device {
            registers: BTreeMap::from([
                (HW_IF, 0x18080000),
                (GP, RF_KILL_SW),
                (HW_REV, observed.hardware_revision),
                (RF_ID, observed.radio_id),
            ]),
            // The scheduler the synthetic ALIVE names.
            prph: BTreeMap::from([(SCD_SRAM_BASE & 0xfffff, 0x817080)]),
            replay: BTreeMap::new(),
            replay_prph: BTreeMap::new(),
            prph_read: 0,
            prph_write: 0,
            events: Vec::new(),
            sram: Vec::new(),
            domain,
            alive,
            after_alive: Vec::new(),
            responses: VecDeque::new(),
            commands: Vec::new(),
            closed: 0,
            command_read: 0,
        })))
    }
}

/// A packet as the firmware writes it into an RB: its length and queue 0,
/// then the command header and the payload.
pub(in crate::wifi::intel) fn packet(
    command: u8,
    group: u8,
    sequence: u16,
    payload: &[u8],
) -> Vec<u8> {
    let mut bytes = ((payload.len() + 4) as u32).to_le_bytes().to_vec();
    bytes.extend([command, group]);
    bytes.extend(sequence.to_le_bytes());
    bytes.extend(payload);
    bytes
}

fn region(domain: &Domain, address: u64, bytes: usize) -> (u64, *mut u8) {
    let r = domain.records.lock().unwrap();
    let id = (address - 0x100000) / 0x10000;
    let pointer = r.live[&id]
        .address((address - (0x100000 + id * 0x10000)) as usize, bytes)
        .unwrap();
    (id, pointer.as_ptr())
}
pub(in crate::wifi::intel) fn device_read(domain: &Domain, address: u64, bytes: usize) -> Vec<u8> {
    let (_, pointer) = region(domain, address, bytes);
    let mut copy = vec![0; bytes];
    // SAFETY: a live model mapping of at least `bytes`, no concurrent writer.
    unsafe { core::ptr::copy_nonoverlapping(pointer, copy.as_mut_ptr(), bytes) };
    copy
}
fn device_write(domain: &Domain, address: u64, bytes: &[u8], complete: bool) {
    let (id, pointer) = region(domain, address, bytes.len());
    // SAFETY: a live model mapping the driver gave the device to write.
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len()) };
    if complete {
        domain.records.lock().unwrap().completed.insert(id);
    }
}

impl Device {
    fn register(&mut self, register: u32) -> u32 {
        let stored = self.registers.get(&register).copied().unwrap_or(0);
        let value = self
            .replay
            .get_mut(&register)
            .and_then(VecDeque::pop_front)
            .unwrap_or(stored);
        if register == GP {
            value | RF_KILL_SW
        } else {
            value
        }
    }
    /// The service channel's TFD: copy the piece, then end it with FH_TX.
    fn service(&mut self) {
        let low = u64::from(self.registers[&SERVICE_DRAM_LOW]);
        let high = self.registers[&SERVICE_DRAM_HIGH];
        let address = low | (u64::from(high >> 28) << 32);
        let bytes = (high & 0x0fffffff) as usize;
        let data = device_read(&self.domain, address, bytes);
        self.sram.push((self.registers[&SERVICE_SRAM], data));
        *self.registers.entry(INT).or_default() |= INT_FH_TX;
        *self.registers.entry(FH_INT).or_default() |= 3;
    }
    fn prph64(&self, address: u32) -> u64 {
        let word = |a| u64::from(self.prph.get(&(a & 0xfffff)).copied().unwrap_or(0));
        word(address) | word(address + 4) << 32
    }
    /// One packet in the next RB, which then closes. The legacy queue's
    /// RBD names the RB's page; the RFH's free BD names it with an ID that
    /// the used ring hands back.
    fn deliver(&mut self, packet: &[u8]) {
        let (page, status) = if let Some(&rbds) = self.registers.get(&RX_RBDCB_BASE) {
            let slot = u64::from(self.closed % 256);
            let entry = device_read(&self.domain, (u64::from(rbds) << 8) + slot * 4, 4);
            let page = u64::from(u32::from_le_bytes(entry.try_into().unwrap())) << 8;
            (page, u64::from(self.registers[&RX_STATUS_WPTR]) << 4)
        } else {
            let slot = u64::from(self.closed % 512);
            let entry = device_read(&self.domain, self.prph64(RFH_FREE_BASE) + slot * 8, 8);
            let entry = u64::from_le_bytes(entry.try_into().unwrap());
            let id = (entry & 0xfff) as u32;
            let used = self.prph64(RFH_USED_BASE) + slot * 4;
            device_write(&self.domain, used, &id.to_le_bytes(), false);
            (entry & !0xfff, self.prph64(RFH_STATUS_BASE))
        };
        let mut buffer = vec![0; RX_BUFFER_BYTES];
        buffer[..packet.len()].copy_from_slice(packet);
        let end = packet.len().next_multiple_of(64);
        buffer[end..end + 4].copy_from_slice(&0x55550000u32.to_le_bytes());
        device_write(&self.domain, page, &buffer, true);
        self.closed += 1;
        let closed = self.closed & 0xfff;
        device_write(&self.domain, status, &closed.to_le_bytes(), false);
        *self.registers.entry(INT).or_default() |= 0x80000000;
    }
    /// The firmware's boot: ALIVE, then what follows it.
    fn boot(&mut self) {
        let alive = core::mem::take(&mut self.alive);
        self.deliver(&alive);
        *self.registers.entry(INT).or_default() |= INT_ALIVE;
        for packet in core::mem::take(&mut self.after_alive) {
            self.deliver(&packet);
        }
    }
    /// Queue 0's doorbell: read each new TFD's TBs as one command, and
    /// answer it with the next scripted packets.
    fn doorbell(&mut self, write: u16) {
        let ring = u64::from(self.registers[&0x19d0]) << 8;
        while self.command_read != write {
            let at = ring + u64::from(self.command_read) * TFD_BYTES as u64;
            let tfd = device_read(&self.domain, at, TFD_BYTES);
            let mut command = Vec::new();
            for n in 0..usize::from(tfd[3]) {
                let low = u32::from_le_bytes(tfd[4 + 6 * n..8 + 6 * n].try_into().unwrap());
                let high = u16::from_le_bytes(tfd[8 + 6 * n..10 + 6 * n].try_into().unwrap());
                let address = u64::from(low) | (u64::from(high & 0xf) << 32);
                command.extend(device_read(&self.domain, address, usize::from(high >> 4)));
            }
            self.commands.push(command);
            for packet in self.responses.pop_front().unwrap_or_default() {
                self.deliver(&packet);
            }
            self.command_read = (self.command_read + 1) % TFD_SLOTS;
        }
    }
}

impl Csr for Model {
    fn read32(&mut self, register: u32) -> Result<u32, AccessError> {
        let mut d = self.0.lock().unwrap();
        if register == PRPH_RDATA {
            let address = d.prph_read & 0xfffff;
            d.events.push(Event::PrphRead(address));
            let stored = d.prph.get(&address).copied().unwrap_or(0);
            return Ok(d
                .replay_prph
                .get_mut(&address)
                .and_then(VecDeque::pop_front)
                .unwrap_or(stored));
        }
        d.events.push(Event::Read(register));
        Ok(d.register(register))
    }
    fn write32(&mut self, register: u32, value: u32) -> Result<(), AccessError> {
        let mut d = self.0.lock().unwrap();
        match register {
            PRPH_RADDR => d.prph_read = value,
            PRPH_WADDR => d.prph_write = value,
            PRPH_WDATA => {
                let address = d.prph_write & 0xfffff;
                d.events.push(Event::PrphWrite(address, value));
                d.prph.insert(address, value);
            }
            _ => {
                d.events.push(Event::Write(register, value));
                match register {
                    INT | FH_INT => *d.registers.entry(register).or_default() &= !value,
                    GP => {
                        let clock = if value & INIT_DONE != 0 {
                            CLOCK_READY
                        } else {
                            0
                        };
                        d.registers.insert(GP, value & !CLOCK_READY | clock);
                    }
                    RESET => {
                        let disabled = if value & STOP_MASTER != 0 {
                            MASTER_DISABLED
                        } else {
                            0
                        };
                        d.registers.insert(RESET, value | disabled);
                        if value & SW_RESET != 0 {
                            *d.registers.get_mut(&HW_IF).unwrap() &= !OWN;
                            d.registers.insert(GP, RF_KILL_SW);
                        }
                    }
                    _ => {
                        d.registers.insert(register, value);
                        if register == SERVICE_CONFIG && value == SERVICE_ENABLE {
                            d.service();
                        }
                        if register == LOAD_STATUS && value == u32::MAX {
                            d.boot();
                        }
                        if register == HBUS_TARG_WRPTR {
                            d.doorbell(value as u16);
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn delay_us(&mut self, _: u32) -> Result<(), AccessError> {
        Ok(())
    }
}
// SAFETY: a synchronous model with no other user of its state.
unsafe impl BootIo for Model {
    fn write8(&mut self, register: u32, value: u8) -> Result<(), AccessError> {
        let mut d = self.0.lock().unwrap();
        d.events.push(Event::Write8(register, u32::from(value)));
        Ok(())
    }
    fn reset_function(&mut self) -> Result<bool, crate::block::BlockError> {
        Ok(true)
    }
}

/// The Latitude 7490's 8265.
pub(in crate::wifi::intel) fn latitude_7490() -> Observed {
    Observed {
        vendor: 0x8086,
        device: 0x24fd,
        subsystem_vendor: 0x8086,
        subsystem_device: 0x0050,
        pci_revision: 0x88,
        link_speed: 1,
        hardware_revision: 0x230,
        radio_id: 0xd55555d5,
        silicon: Silicon::default(),
    }
}
/// The Latitude 7400 2-in-1's 9560, as Linux read it.
pub(in crate::wifi::intel) fn latitude_7400() -> Observed {
    Observed {
        device: 0x9df0,
        subsystem_device: 0x4030,
        pci_revision: 0x30,
        hardware_revision: 0x312,
        radio_id: 0x105110,
        ..latitude_7490()
    }
}
/// A catalog holding `bytes` as `observed`'s image under its device binding.
pub(in crate::wifi::intel) fn catalog(observed: Observed, bytes: Vec<u8>) -> Catalog {
    let name = Profile::select(observed).unwrap().firmware_name();
    let hardware = alloc::format!("pci:8086:{:04x}", observed.device);
    Catalog::from_handoff(vec![Resource {
        name,
        target: Target {
            driver: "wifi-intel",
            hardware: &hardware,
            revision: DEVICE_REVISION,
            board: None,
        },
        provenance: Provenance {
            source: "test",
            revision: "immutable",
            member: name,
            license: "BSD",
        },
        expected_length: bytes.len(),
        expected_sha256: Sha256::digest(&bytes).into(),
        bytes,
    }])
    .unwrap()
}

/// An image of API `api` whose INIT sections mirror the real one's shapes: a
/// section past one service-channel piece, one in the extended range, both
/// separators and a paged section that must not load.
pub(in crate::wifi::intel) fn image(api: u32) -> (Vec<u8>, Vec<(u32, Vec<u8>)>) {
    let mut bytes = vec![0u8; 88];
    bytes[4..8].copy_from_slice(&0x0a4c_5749u32.to_le_bytes());
    bytes[72..76].copy_from_slice(&api.to_le_bytes());
    let mut tlv = |kind: u32, data: &[u8]| {
        bytes.extend(kind.to_le_bytes());
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(data);
        bytes.resize(bytes.len().next_multiple_of(4), 0);
    };
    tlv(29, &[0, 0, 0, 0, 0, 0, 0x10, 0]);
    // Both chains, as the real images' PHY SKU has them.
    tlv(23, &0x330018u32.to_le_bytes());
    // The INIT image's calibration triggers.
    let mut calibration = 1u32.to_le_bytes().to_vec();
    calibration.extend(0x1f8e51dbu32.to_le_bytes());
    calibration.extend(1u32.to_le_bytes());
    tlv(22, &calibration);
    // The runtime image's.
    let mut calibration = 0u32.to_le_bytes().to_vec();
    calibration.extend(0x0f0615dbu32.to_le_bytes());
    calibration.extend(0x0f02958bu32.to_le_bytes());
    tlv(22, &calibration);
    let fill = |seed: u8, n: usize| {
        (0..n)
            .map(|i| seed.wrapping_add(i as u8))
            .collect::<Vec<_>>()
    };
    let sections: [(u32, Vec<u8>); 6] = [
        (0x404000, fill(1, 0x2c8)),
        (0x0, fill(2, 0x50000)),
        (CPU_SEPARATOR, vec![0; 4]),
        (0x405000, fill(3, 0x2b8)),
        (PAGING_SEPARATOR, vec![0; 4]),
        (0x1000000, fill(4, 64)),
    ];
    for (offset, data) in &sections {
        let mut payload = offset.to_le_bytes().to_vec();
        payload.extend(data);
        tlv(20, &payload);
    }
    // A runtime section the INIT boot leaves alone.
    let mut runtime = 0x404000u32.to_le_bytes().to_vec();
    runtime.extend(fill(9, 16));
    tlv(19, &runtime);
    let mut expected = Vec::new();
    for (offset, data) in [&sections[0], &sections[1], &sections[3]] {
        for (n, piece) in data.chunks(CHUNK_BYTES).enumerate() {
            expected.push((offset + (n * CHUNK_BYTES) as u32, piece.to_vec()));
        }
    }
    (bytes, expected)
}

pub(in crate::wifi::intel) fn alive_packet() -> Vec<u8> {
    let mut packet = 0xb8000048u32.to_le_bytes().to_vec();
    packet.extend([1, 0, 0, 0x80]);
    let mut payload = vec![0u8; ALIVE_BYTES];
    payload[..2].copy_from_slice(&ALIVE_OK.to_le_bytes());
    for (at, word) in [
        (4, 36u32),
        (8, 0xc8e8e144),
        (20, 0x813a6c),
        (40, 0x817080),
        (60, 0x88105c),
    ] {
        payload[at..at + 4].copy_from_slice(&word.to_le_bytes());
    }
    payload[13] = 9;
    packet.extend(payload);
    packet
}

pub(in crate::wifi::intel) fn staged(
    observed: Observed,
    catalog: &Catalog,
) -> (Profile, Staged<'_>) {
    let profile = Profile::select(observed).unwrap();
    let staged = profile.stage(catalog).unwrap();
    (profile, staged)
}

/// Boot `observed`'s synthetic INIT image to ALIVE and stop; the model's
/// state is left for the caller to check.
fn synthetic_boot(observed: Observed, api: u32) -> (Model, Arc<Domain>) {
    let (bytes, expected) = image(api);
    let catalog = catalog(observed, bytes);
    let (profile, staged) = staged(observed, &catalog);
    let domain = Arc::new(Domain::default());
    let model = Model::new(domain.clone(), alive_packet(), observed);
    let source = Source::default();
    // SAFETY: the model is the function's only user and the test domain
    // isolates it.
    let mut boot = unsafe {
        Boot::start(
            model.clone(),
            profile,
            staged,
            ImageKind::Init,
            &source,
            domain.clone(),
        )
    }
    .unwrap();
    let alive = boot.wait_alive(10).unwrap();
    boot.stop().unwrap();
    assert_eq!(
        alive,
        Alive {
            flags: 0,
            ucode_major: 36,
            ucode_minor: 0xc8e8e144,
            image_type: 9,
            error_table: 0x813a6c,
            log_table: 0,
            scheduler_base: 0x817080,
            umac_major: 0,
            umac_minor: 0,
            umac_error_table: 0x88105c,
        }
    );
    {
        let d = model.0.lock().unwrap();
        // Every INIT piece reached its SRAM address, in order, and nothing
        // else.
        assert_eq!(d.sram, expected);
        let status: Vec<_> = d
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Write(LOAD_STATUS, v) => Some(*v),
                _ => None,
            })
            .collect();
        assert_eq!(status, [1, 3, 0xffff, 0x1ffff, u32::MAX]);
    }
    // The stop acknowledged master disable, so nothing stays mapped.
    assert!(domain.records.lock().unwrap().live.is_empty());
    (model, domain)
}

#[test]
fn the_8265_init_image_loads_piece_by_piece_answers_alive_and_stops() {
    let (model, _) = synthetic_boot(latitude_7490(), 36);
    let d = model.0.lock().unwrap();
    // The extended-range piece alone was wrapped in LMPM_CHICK.
    let chick: Vec<_> = d
        .events
        .iter()
        .filter_map(|e| match e {
            Event::PrphWrite(0x01ff8, v) => Some(*v),
            _ => None,
        })
        .collect();
    assert_eq!(chick, [1, 0]);
    assert!(d.events.contains(&Event::Write(RX_WPTR, 0xf8)));
    assert!(d.events.contains(&Event::Write(RX_CONFIG, 0x80801114)));
    assert!(d.events.contains(&Event::Write(HW_IF, 0x18489000)));
    assert!(
        !d.events
            .contains(&Event::PrphWrite(UREG_CHICK & 0xfffff, UREG_CHICK_MSI))
    );
}

/// The 9560 boots on the RFH's queue 0: 511 free BDs carrying their IDs,
/// the used ring naming the ALIVE's RB, and MSI chosen in UREG_CHICK.
#[test]
fn the_9560_init_image_answers_alive_on_the_rfh() {
    let (model, _) = synthetic_boot(latitude_7400(), 46);
    let d = model.0.lock().unwrap();
    assert!(
        d.events
            .contains(&Event::PrphWrite(UREG_CHICK & 0xfffff, UREG_CHICK_MSI))
    );
    assert!(
        d.events
            .contains(&Event::PrphWrite(RFH_DMA_CFG & 0xfffff, 0x87940000))
    );
    assert!(
        d.events
            .contains(&Event::PrphWrite(RFH_GEN_CFG & 0xfffff, 3))
    );
    assert!(
        d.events
            .contains(&Event::PrphWrite(RFH_RXQ_ACTIVE & 0xfffff, 0x10001))
    );
    let triggers = d
        .events
        .iter()
        .filter(|e| **e == Event::Write(RFH_FREE_WIDX_TRIGGER, 0x1f8))
        .count();
    assert_eq!(triggers, 2);
    // No legacy queue was set up.
    assert!(
        !d.events
            .iter()
            .any(|e| matches!(e, Event::Write(RX_CONFIG, _)))
    );
}

#[test]
fn an_image_without_init_sections_is_refused() {
    let mut bytes = vec![0u8; 88];
    bytes[4..8].copy_from_slice(&0x0a4c_5749u32.to_le_bytes());
    bytes[72..76].copy_from_slice(&36u32.to_le_bytes());
    bytes.extend(29u32.to_le_bytes());
    bytes.extend(8u32.to_le_bytes());
    bytes.extend([0, 0, 0, 0, 0, 0, 0x10, 0]);
    bytes.extend(23u32.to_le_bytes());
    bytes.extend(4u32.to_le_bytes());
    bytes.extend(0x18u32.to_le_bytes());
    let catalog = catalog(latitude_7490(), bytes);
    let (profile, staged) = staged(latitude_7490(), &catalog);
    let domain = Arc::new(Domain::default());
    let model = Model::new(domain.clone(), alive_packet(), latitude_7490());
    // SAFETY: as above.
    let error = unsafe {
        Boot::start(
            model.clone(),
            profile,
            staged,
            ImageKind::Init,
            &Source::default(),
            domain,
        )
    };
    assert_eq!(
        error.err(),
        Some(Error::Firmware(firmware::Error::Missing("image sections")))
    );
    // Refused before the device was touched.
    assert!(model.0.lock().unwrap().events.is_empty());
}

/// Accesses as the oracle compares them: interrupt and NIC-access traffic
/// dropped, polls folded, DMA addresses kept only as their presence. RX
/// restocks are dropped too: Linux restocks from its own buffer pool, in
/// its own batches. So are Linux's ICT table and its logged-only read of the
/// OTP version, and the 8000 family's OTP address, which Linux reads through
/// `iwl_trans_read_prph`, outside the trace.
pub(in crate::wifi::intel) fn normalize(events: &[Event]) -> Vec<Event> {
    const DROPPED: [u32; 5] = [INT, INT_MASK, FH_INT, GP, 0x5];
    const ADDRESS: u32 = 0xadd0_0000;
    let start = events
        .iter()
        .position(|e| matches!(e, Event::Read(RESET) | Event::Write(RESET, _)))
        .expect("a reset");
    let mut out: Vec<Event> = Vec::new();
    for event in &events[start..] {
        let event = match *event {
            Event::Read(r) | Event::Write(r, _) | Event::Write8(r, _) if DROPPED.contains(&r) => {
                continue;
            }
            Event::Write(RX_WPTR | RFH_FREE_WIDX_TRIGGER, v) if v != 0xf8 && v != 0x1f8 => continue,
            Event::Write(0xa0, _) | Event::PrphRead(0x0333c | 0x03080 | 0x03084) => continue,
            Event::Write(r, v) => Event::Write(
                r,
                match r {
                    RX_RBDCB_BASE | RX_STATUS_WPTR | KEEP_WARM | SERVICE_DRAM_LOW => ADDRESS,
                    0x19d0..=0x1a0c | 0x1bf0..=0x1bfc | 0x1b20..=0x1b48 => ADDRESS,
                    SERVICE_DRAM_HIGH => v & 0x0fffffff,
                    _ => v,
                },
            ),
            Event::PrphRead(a) => Event::PrphRead(a & 0xfffff),
            Event::PrphWrite(a, v) => Event::PrphWrite(
                a & 0xfffff,
                match a & 0xfffff {
                    0x08000 | 0x08004 | 0x08100 | 0x08104 | 0x08200 | 0x08204 => ADDRESS,
                    0x02c08 => ADDRESS,
                    _ => v,
                },
            ),
            other => other,
        };
        if matches!(event, Event::Read(_)) && out.last() == Some(&event) {
            continue;
        }
        out.push(event);
    }
    out
}

/// One `.hex` line's access, and the ALIVE packet that ends the boot.
pub(in crate::wifi::intel) enum Line {
    Access(Event, Option<u32>),
    Alive(Vec<u8>),
}
pub(in crate::wifi::intel) fn parse(line: &str) -> Option<Line> {
    let field = |name: &str| {
        line.split_whitespace()
            .find_map(|f| f.strip_prefix(name))
            .map(String::from)
    };
    let number = |name: &str| u32::from_str_radix(field(name)?.trim_start_matches("0x"), 16).ok();
    let kind = line.split_whitespace().nth(2)?;
    let (offset, value) = (number("offs="), number("val="));
    Some(match kind {
        "iwlwifi_io:iwlwifi_dev_ioread32" => Line::Access(Event::Read(offset?), value),
        "iwlwifi_io:iwlwifi_dev_iowrite32" => Line::Access(Event::Write(offset?, value?), None),
        "iwlwifi_io:iwlwifi_dev_iowrite8" => Line::Access(Event::Write8(offset?, value?), None),
        "iwlwifi_io:iwlwifi_dev_ioread_prph32" => {
            Line::Access(Event::PrphRead(offset? & 0xfffff), value)
        }
        "iwlwifi_io:iwlwifi_dev_iowrite_prph32" => {
            Line::Access(Event::PrphWrite(offset? & 0xfffff, value?), None)
        }
        "iwlwifi:iwlwifi_dev_rx" if number("cmd=") == Some(1) => {
            let hex = field("rxbuf=")?;
            Line::Alive(
                (0..hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                    .collect(),
            )
        }
        _ => return None,
    })
}

/// Linux's own boot of the INIT image, from the probe to ALIVE, in the
/// trace `trace_variable` names, against this transport on a model of
/// `observed` that answers each read as the card did: the same accesses, in
/// the same order.
fn linux_init_boot_accesses_match(observed: Observed, trace_variable: &str) {
    let trace = std::fs::read_to_string(std::env::var(trace_variable).unwrap()).unwrap();
    let root = std::env::var("STELA_IWLWIFI_IMAGES").unwrap();
    let profile = Profile::select(observed).unwrap();
    let path = std::path::Path::new(&root).join(profile.firmware_name());
    let image = std::fs::read(path).unwrap();
    let mut linux = Vec::new();
    let mut alive = None;
    for line in trace.lines() {
        match parse(line) {
            Some(Line::Access(event, read)) => linux.push((event, read)),
            Some(Line::Alive(packet)) => {
                alive = Some(packet);
                break;
            }
            None => {}
        }
    }
    let alive = alive.expect("an ALIVE in the trace");
    let catalog = catalog(observed, image);
    let (profile, staged) = staged(observed, &catalog);
    let domain = Arc::new(Domain::default());
    let model = Model::new(domain.clone(), alive, observed);
    {
        // From the reset on, every read answers what the card answered.
        let start = linux
            .iter()
            .position(|(e, _)| matches!(e, Event::Read(RESET) | Event::Write(RESET, _)))
            .unwrap();
        let mut d = model.0.lock().unwrap();
        for (event, value) in &linux[start..] {
            match (event, value) {
                (Event::Read(r), Some(v)) if *r != GP && *r != INT => {
                    d.replay.entry(*r).or_default().push_back(*v)
                }
                (Event::PrphRead(a), Some(v)) => d.replay_prph.entry(*a).or_default().push_back(*v),
                _ => {}
            }
        }
    }
    let source = Source::default();
    // SAFETY: as above.
    let mut boot = unsafe {
        Boot::start(
            model.clone(),
            profile,
            staged,
            ImageKind::Init,
            &source,
            domain.clone(),
        )
    }
    .unwrap();
    let ours = boot.wait_alive(10).unwrap();
    let api = Profile::select(observed).unwrap().family().firmware_api();
    assert_eq!((ours.ucode_major, ours.image_type), (api, 9));
    let ours_events = normalize(&model.0.lock().unwrap().events);
    let linux_events = normalize(&linux.iter().map(|(e, _)| *e).collect::<Vec<_>>());
    // The reset, the queues, every piece and both load-status ends.
    assert!(linux_events.len() > 150, "{}", linux_events.len());
    for (n, (a, b)) in ours_events.iter().zip(&linux_events).enumerate() {
        assert_eq!(a, b, "access {n}: ours {a:x?}, Linux {b:x?}");
    }
    assert_eq!(
        ours_events.len(),
        linux_events.len(),
        "accesses: ours, Linux"
    );
    boot.stop().unwrap();
}

#[test]
#[ignore = "oracle: STELA_IWLWIFI_GEN1_TRACE and STELA_IWLWIFI_IMAGES"]
fn linux_8265_init_boot_accesses_match() {
    linux_init_boot_accesses_match(latitude_7490(), "STELA_IWLWIFI_GEN1_TRACE");
}

/// The same on the Latitude 7400 2-in-1's 9560.
#[test]
#[ignore = "oracle: STELA_IWLWIFI_9560_TRACE and STELA_IWLWIFI_IMAGES"]
fn linux_9560_init_boot_accesses_match() {
    linux_init_boot_accesses_match(latitude_7400(), "STELA_IWLWIFI_9560_TRACE");
}
