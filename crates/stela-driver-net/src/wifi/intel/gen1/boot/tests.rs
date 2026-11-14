// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright (C) 2003-2014, 2018-2026 Intel Corporation
//! An 8265 model: CSRs, the peripheral space, the FH service channel copying
//! into a modeled SRAM, and a firmware that answers the end of the CPU2 load
//! with ALIVE on the legacy RX queue. The oracle case drives it with Linux's
//! own reads and compares the accesses with Linux's.
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
enum Event {
    Read(u32),
    Write(u32, u32),
    Write8(u32, u32),
    PrphRead(u32),
    PrphWrite(u32, u32),
}

struct Device {
    registers: BTreeMap<u32, u32>,
    prph: BTreeMap<u32, u32>,
    /// Linux's read values, per register and per peripheral address.
    replay: BTreeMap<u32, VecDeque<u32>>,
    replay_prph: BTreeMap<u32, VecDeque<u32>>,
    prph_read: u32,
    prph_write: u32,
    events: Vec<Event>,
    sram: Vec<(u32, Vec<u8>)>,
    domain: Arc<Domain>,
    alive: Vec<u8>,
}

#[derive(Clone)]
struct Model(Arc<Mutex<Device>>);

impl Model {
    fn new(domain: Arc<Domain>, alive: Vec<u8>) -> Self {
        Self(Arc::new(Mutex::new(Device {
            registers: BTreeMap::from([
                (HW_IF, 0x18080000),
                (GP, RF_KILL_SW),
                (HW_REV, 0x230),
                (RF_ID, 0xd55555d5),
            ]),
            prph: BTreeMap::new(),
            replay: BTreeMap::new(),
            replay_prph: BTreeMap::new(),
            prph_read: 0,
            prph_write: 0,
            events: Vec::new(),
            sram: Vec::new(),
            domain,
            alive,
        })))
    }
}

fn region(domain: &Domain, address: u64, bytes: usize) -> (u64, *mut u8) {
    let r = domain.records.lock().unwrap();
    let id = (address - 0x100000) / 0x10000;
    let pointer = r.live[&id]
        .address((address - (0x100000 + id * 0x10000)) as usize, bytes)
        .unwrap();
    (id, pointer.as_ptr())
}
fn device_read(domain: &Domain, address: u64, bytes: usize) -> Vec<u8> {
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
    /// The firmware's boot: ALIVE in the first RB, one closed RB.
    fn boot(&mut self) {
        let rbds = u64::from(self.registers[&RX_RBDCB_BASE]) << 8;
        let first = device_read(&self.domain, rbds, 4);
        let page = u64::from(u32::from_le_bytes(first.try_into().unwrap())) << 8;
        let mut buffer = vec![0; RX_BUFFER_BYTES];
        buffer[..self.alive.len()].copy_from_slice(&self.alive);
        let end = self.alive.len().next_multiple_of(64);
        buffer[end..end + 4].copy_from_slice(&0x55550000u32.to_le_bytes());
        device_write(&self.domain, page, &buffer, true);
        let status = u64::from(self.registers[&RX_STATUS_WPTR]) << 4;
        device_write(&self.domain, status, &1u16.to_le_bytes(), false);
        *self.registers.entry(INT).or_default() |= 0x80000000 | INT_ALIVE;
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

fn observed() -> Observed {
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
/// A catalog holding `bytes` as the 8265's image under its device binding.
fn catalog(bytes: Vec<u8>) -> Catalog {
    let name = "iwlwifi-8265-36.ucode";
    Catalog::from_handoff(vec![Resource {
        name,
        target: Target {
            driver: "wifi-intel",
            hardware: "pci:8086:24fd",
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

/// An API 36 image whose INIT sections mirror the real one's shapes: a
/// section past one service-channel piece, one in the extended range, both
/// separators and a paged section that must not load.
fn image() -> (Vec<u8>, Vec<(u32, Vec<u8>)>) {
    let mut bytes = vec![0u8; 88];
    bytes[4..8].copy_from_slice(&0x0a4c_5749u32.to_le_bytes());
    bytes[72..76].copy_from_slice(&36u32.to_le_bytes());
    let mut tlv = |kind: u32, data: &[u8]| {
        bytes.extend(kind.to_le_bytes());
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(data);
        bytes.resize(bytes.len().next_multiple_of(4), 0);
    };
    tlv(29, &[0, 0, 0, 0, 0, 0, 0x10, 0]);
    tlv(23, &0x18u32.to_le_bytes());
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

fn alive_packet() -> Vec<u8> {
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

fn staged<'a>(catalog: &'a Catalog) -> (Profile, Staged<'a>) {
    let profile = Profile::select(observed()).unwrap();
    let staged = profile.stage(catalog).unwrap();
    (profile, staged)
}

#[test]
fn the_init_image_loads_piece_by_piece_answers_alive_and_stops() {
    let (bytes, expected) = image();
    let catalog = catalog(bytes);
    let (profile, staged) = staged(&catalog);
    let domain = Arc::new(Domain::default());
    let model = Model::new(domain.clone(), alive_packet());
    let source = Source::default();
    // SAFETY: the model is the function's only user and the test domain
    // isolates it.
    let alive =
        unsafe { init_alive(model.clone(), profile, staged, &source, domain.clone()) }.unwrap();
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
    let d = model.0.lock().unwrap();
    // Every INIT piece reached its SRAM address, in order, and nothing else.
    assert_eq!(d.sram, expected);
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
    let status: Vec<_> = d
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Write(LOAD_STATUS, v) => Some(*v),
            _ => None,
        })
        .collect();
    assert_eq!(status, [1, 3, 0xffff, 0x1ffff, u32::MAX]);
    assert!(d.events.contains(&Event::Write(RX_WPTR, 0xf8)));
    assert!(d.events.contains(&Event::Write(RX_CONFIG, 0x80801114)));
    assert!(d.events.contains(&Event::Write(HW_IF, 0x18489000)));
    // The stop acknowledged master disable, so nothing stays mapped.
    assert!(domain.records.lock().unwrap().live.is_empty());
}

#[test]
fn an_image_without_init_sections_or_of_another_family_is_refused() {
    let mut bytes = vec![0u8; 88];
    bytes[4..8].copy_from_slice(&0x0a4c_5749u32.to_le_bytes());
    bytes[72..76].copy_from_slice(&36u32.to_le_bytes());
    bytes.extend(29u32.to_le_bytes());
    bytes.extend(8u32.to_le_bytes());
    bytes.extend([0, 0, 0, 0, 0, 0, 0x10, 0]);
    bytes.extend(23u32.to_le_bytes());
    bytes.extend(4u32.to_le_bytes());
    bytes.extend(0x18u32.to_le_bytes());
    let catalog = catalog(bytes);
    let (profile, staged) = staged(&catalog);
    let domain = Arc::new(Domain::default());
    let model = Model::new(domain.clone(), alive_packet());
    // SAFETY: as above.
    let error = unsafe { init_alive(model.clone(), profile, staged, &Source::default(), domain) };
    assert_eq!(
        error,
        Err(Error::Firmware(firmware::Error::Missing("image sections")))
    );
    // Refused before the device was touched.
    assert!(model.0.lock().unwrap().events.is_empty());
}

/// Accesses as the oracle compares them: interrupt and NIC-access traffic
/// dropped, polls folded, DMA addresses kept only as their presence.
fn normalize(events: &[Event]) -> Vec<Event> {
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
            Event::PrphWrite(a, v) => Event::PrphWrite(a & 0xfffff, v),
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
enum Line {
    Access(Event, Option<u32>),
    Alive(Vec<u8>),
}
fn parse(line: &str) -> Option<Line> {
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

/// Linux's own boot of the INIT image on a Latitude 7490, from the probe to
/// ALIVE, against this transport on a model that answers each read as the
/// card did: the same accesses, in the same order.
#[test]
#[ignore = "oracle: STELA_IWLWIFI_GEN1_TRACE and STELA_IWLWIFI_IMAGES"]
fn linux_8265_init_boot_accesses_match() {
    let trace =
        std::fs::read_to_string(std::env::var("STELA_IWLWIFI_GEN1_TRACE").unwrap()).unwrap();
    let root = std::env::var("STELA_IWLWIFI_IMAGES").unwrap();
    let image = std::fs::read(std::path::Path::new(&root).join("iwlwifi-8265-36.ucode")).unwrap();
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
    let catalog = catalog(image);
    let (profile, staged) = staged(&catalog);
    let domain = Arc::new(Domain::default());
    let model = Model::new(domain.clone(), alive);
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
    assert_eq!((ours.ucode_major, ours.image_type), (36, 9));
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
