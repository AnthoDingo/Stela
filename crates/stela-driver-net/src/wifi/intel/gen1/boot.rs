// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright (C) 2003-2014, 2018-2026 Intel Corporation
// Copyright (C) 2013-2015 Intel Mobile Communications GmbH
// Copyright (C) 2016-2017 Intel Deutschland GmbH
//! The 8000 family's gen1 transport up to a firmware image's ALIVE, as
//! `iwl_trans_pcie_start_hw`, `iwl_trans_pcie_start_fw` and
//! `iwl_pcie_load_given_ucode_8000` drive it in pcie/gen1_2/{trans,rx,tx}.c
//! at Linux a804c5742bff224d4cb4d25dd383554d598be32b.
//!
//! The driver itself copies each section of the image into the device's SRAM
//! through the FH service channel, then the firmware boots and answers on the
//! single legacy RX queue. Interrupts stay masked: CSR_INT is polled. ALIVE
//! is boot metadata, not NVM, calibration or radio readiness.
use super::{Family, Profile};
use crate::{
    bus::dma::{
        mapping::{Direction, DmaDomain},
        memory::PageSource,
    },
    bytes::{le16, le32},
    firmware as resources,
    wifi::intel::{
        boot::{BootIo, Error},
        dma::{self, Chunk},
        firmware::{self, Firmware, ImageKind, Section},
        identity::Staged,
        receive::{self, Packets},
    },
};
use alloc::{sync::Arc, vec::Vec};
use core::sync::atomic::{Ordering, fence};

const HW_IF: u32 = 0x000;
const INT_COALESCING: u32 = 0x004;
const INT: u32 = 0x008;
const INT_MASK: u32 = 0x00c;
const FH_INT: u32 = 0x010;
const RESET: u32 = 0x020;
const GP: u32 = 0x024;
const HW_REV: u32 = 0x028;
const GIO: u32 = 0x03c;
const GP1_CLEAR: u32 = 0x05c;
const MAILBOX: u32 = 0x088;
const RF_ID: u32 = 0x09c;
const SHADOW: u32 = 0x0a8;
const CHICKEN: u32 = 0x100;
const HPET: u32 = 0x240;
const LINK_POWER: u32 = 0x250;
const PRPH_WADDR: u32 = 0x444;
const PRPH_RADDR: u32 = 0x448;
const PRPH_WDATA: u32 = 0x44c;
const PRPH_RDATA: u32 = 0x450;

const OWN: u32 = 0x00400000;
const WAKE_ME: u32 = 0x08000000;
const HAP_WAKE: u32 = 0x00080000;
const OS_ALIVE: u32 = 1 << 5;
const CLOCK_READY: u32 = 1;
const INIT_DONE: u32 = 4;
const ACCESS_REQUEST: u32 = 8;
const GOING_TO_SLEEP: u32 = 0x10;
const RF_KILL_SW: u32 = 0x08000000;
const SW_RESET: u32 = 0x80;
const MASTER_DISABLED: u32 = 0x100;
const STOP_MASTER: u32 = 0x200;
const RFKILL_HANDSHAKE: u32 = 2;
const CMD_BLOCKED: u32 = 4;
const INT_SW_ERR: u32 = 1 << 25;
const INT_FH_TX: u32 = 1 << 27;
const INT_HW_ERR: u32 = 1 << 29;
/// The bits `iwl_mvm_nic_config` owns in CSR_HW_IF_CONFIG_REG.
const NIC_CONFIG: u32 = 0xff0f;

// Legacy RX queue (rx.c `iwl_pcie_rx_hw_init`).
const RX_CONFIG: u32 = 0x1c00;
const RX_RBDCB_WPTR: u32 = 0x1c08;
const RX_FLUSH: u32 = 0x1c10;
const RX_STATUS_WPTR: u32 = 0x1bc0;
const RX_RBDCB_BASE: u32 = 0x1bc4;
const RX_WPTR: u32 = 0x1bc8;
const RX_RDPTR: u32 = 0x1bcc;
/// Enabled, RXF-empty ignored, interrupts to the host, 4 KiB buffers,
/// RB timeout 0x11 and 256 RBDs.
const RX_ENABLE: u32 = 0x80000000 | 0x4 | 0x1000 | (0x11 << 4) | (8 << 20);
const RX_QUEUE_SIZE: usize = 256;
const RX_BUFFER_BYTES: usize = 4096;
/// `iwl_pcie_rxq_restock` fills every slot but one, and the write pointer
/// moves in steps of eight.
const RX_BUFFERS: usize = RX_QUEUE_SIZE - 1;
const RX_PUBLISHED: u16 = (RX_BUFFERS & !7) as u16;
const HOST_INT_TIMEOUT: u8 = 0x40;

// TX queues (tx.c `iwl_pcie_tx_init`).
const KEEP_WARM: u32 = 0x197c;
const KEEP_WARM_BYTES: usize = 0x1000;
const QUEUES: usize = 31;
/// 256 TFDs of 128 bytes.
const RING_BYTES: usize = 256 * 128;
const SCD_TXFACT: u32 = 0xa02c10;
const SCD_GP_CTRL: u32 = 0xa02da8;
const SCD_AUTO_ACTIVE: u32 = 1 << 18;
const SCD_31_QUEUES: u32 = 1;

// Firmware load (trans.c `iwl_pcie_load_firmware_chunk_fh`).
const SERVICE_CONFIG: u32 = 0x1e20;
const SERVICE_STATUS: u32 = 0x1e28;
const SERVICE_SRAM: u32 = 0x19c8;
const SERVICE_DRAM_LOW: u32 = 0x1948;
const SERVICE_DRAM_HIGH: u32 = 0x194c;
const SERVICE_ENABLE: u32 = 0x80000000 | 0x00100000;
const SERVICE_VALID: u32 = (1 << 20) | (1 << 12) | 3;
const LOAD_STATUS: u32 = 0x1af0;
const WFPM_GP2: u32 = 0xa030b4;
const RELEASE_CPU_RESET: u32 = 0x300c;
const LMPM_CHICK: u32 = 0xa01ff8;
const EXTENDED_ADDRESS: u32 = 1;
const EXTENDED: core::ops::RangeInclusive<u32> = 0x40000..=0x57fff;
const CHUNK_BYTES: usize = 0x20000;
const CPU_SEPARATOR: u32 = 0xffff_cccc;
const PAGING_SEPARATOR: u32 = 0xaaaa_bbbb;
/// `wait_event_timeout(..., 5 * HZ)` for each chunk.
const CHUNK_TIMEOUT_US: u32 = 5_000_000;
/// The 36-bit DMA addresses the gen1 FH registers carry.
const DMA_LIMIT: u64 = 1 << 36;

/// What `iwl_alive_ntf_v3` (UCODE_ALIVE_NTFY_API_S_VER_3) says of a running
/// image. The scheduler's SRAM base is what `iwl_pcie_tx_start` programs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Alive {
    pub flags: u16,
    pub ucode_major: u32,
    pub ucode_minor: u32,
    /// 9 for the INIT image.
    pub image_type: u8,
    pub error_table: u32,
    pub log_table: u32,
    pub scheduler_base: u32,
    pub umac_major: u32,
    pub umac_minor: u32,
    pub umac_error_table: u32,
}

const ALIVE_BYTES: usize = 68;
const ALIVE_OK: u16 = 0xcafe;

impl Alive {
    /// `iwl_alive_fn`'s v3 case, which Linux picks by the payload's length.
    pub fn decode(payload: &[u8]) -> Result<Self, receive::Error> {
        if payload.len() != ALIVE_BYTES {
            return Err(receive::Error::AliveLength {
                minimum: ALIVE_BYTES,
                actual: payload.len(),
            });
        }
        let (status, flags) = (le16(payload, 0), le16(payload, 2));
        if status != ALIVE_OK {
            return Err(receive::Error::AliveRejected { status, flags });
        }
        Ok(Self {
            flags,
            ucode_major: le32(payload, 4),
            ucode_minor: le32(payload, 8),
            image_type: payload[13],
            error_table: le32(payload, 20),
            log_table: le32(payload, 24),
            scheduler_base: le32(payload, 40),
            umac_major: le32(payload, 52),
            umac_minor: le32(payload, 56),
            umac_error_table: le32(payload, 60),
        })
    }
}

/// One piece of a section, as `iwl_pcie_load_section` copies it.
struct Piece {
    destination: u32,
    chunk: Chunk,
}

/// `iwl_pcie_load_cpu_sections_8000`'s two lists: CPU1 up to the first
/// separator, CPU2 from there to the next one. Paged sections follow and
/// load later, by command.
fn cpu_sections<'a>(
    firmware: &Firmware<'a>,
    kind: ImageKind,
) -> Result<[Vec<Section<'a>>; 2], Error> {
    let all: Vec<Section<'a>> = firmware
        .sections()
        .iter()
        .filter(|section| section.image == kind)
        .copied()
        .collect();
    let end = |from: usize| {
        all[from..]
            .iter()
            .position(|s| s.data.is_empty() || matches!(s.offset, CPU_SEPARATOR | PAGING_SEPARATOR))
            .map_or(all.len(), |n| from + n)
    };
    let first = end(0);
    let second = if first < all.len() {
        all[first + 1..end(first + 1)].to_vec()
    } else {
        Vec::new()
    };
    if first == 0 {
        return Err(Error::Firmware(firmware::Error::Missing("image sections")));
    }
    Ok([all[..first].to_vec(), second])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Loaded,
    Alive,
    Poisoned,
    Stopped,
}

/// One image booted on an 8000-family function, keeping every allocation
/// the device was told about until a stop acknowledges its DMA.
pub struct Boot<'a, Q: BootIo> {
    io: Option<Q>,
    profile: Profile,
    firmware: Firmware<'a>,
    image: resources::Image<'a>,
    rbds: Chunk,
    status: Chunk,
    buffers: Vec<Chunk>,
    keep_warm: Chunk,
    rings: Vec<Chunk>,
    read: u16,
    state: State,
}

impl<'a, Q: BootIo> Boot<'a, Q> {
    /// Wake the function, set up its queues, copy `kind`'s sections into it
    /// and let it run.
    ///
    /// # Safety
    /// As for `RomBoot::start`: `io` alone reaches the claimed, stopped
    /// function with bus mastering on, `domain` isolates its requester with
    /// coherent DMA, and nothing else submits to or resets it.
    pub unsafe fn start(
        io: Q,
        profile: Profile,
        staged: Staged<'a>,
        kind: ImageKind,
        source: &dyn PageSource,
        domain: Arc<dyn DmaDomain>,
    ) -> Result<Self, Error> {
        if profile.family() != Family::F8000 {
            return Err(Error::Family);
        }
        let (firmware, image) = staged.into_parts();
        if !profile.observed().binds(profile.firmware_name(), &image) {
            return Err(Error::FirmwareBinding);
        }
        if firmware.phy_sku().is_none() {
            return Err(Error::PhyConfiguration);
        }
        let [cpu1, cpu2] = cpu_sections(&firmware, kind)?;
        let allocate = |bytes: &[u8], direction| -> Result<Chunk, Error> {
            let chunk = Chunk::allocate(source, domain.clone(), bytes, direction)?;
            let region = chunk.region();
            if region.address + region.bytes as u64 > DMA_LIMIT {
                return Err(Error::Dma(dma::Error::AllocationContract));
            }
            Ok(chunk)
        };
        let mut pieces = [Vec::new(), Vec::new()];
        for (sections, cpu) in [cpu1, cpu2].iter().zip(&mut pieces) {
            for section in sections {
                let mut list = Vec::new();
                for (n, data) in section.data.chunks(CHUNK_BYTES).enumerate() {
                    list.push(Piece {
                        destination: section
                            .offset
                            .checked_add((n * CHUNK_BYTES) as u32)
                            .ok_or(Error::Firmware(firmware::Error::SectionAddress))?,
                        chunk: allocate(data, Direction::ToDevice)?,
                    });
                }
                cpu.push(list);
            }
        }
        let mut buffers = Vec::with_capacity(RX_BUFFERS);
        let mut rbds = alloc::vec![0; RX_QUEUE_SIZE * 4];
        for slot in 0..RX_BUFFERS {
            let buffer = allocate(&[0; RX_BUFFER_BYTES], Direction::FromDevice)?;
            let page = (buffer.region().address >> 8) as u32;
            rbds[slot * 4..slot * 4 + 4].copy_from_slice(&page.to_le_bytes());
            buffers.push(buffer);
        }
        let rbds = allocate(&rbds, Direction::Bidirectional)?;
        let status = allocate(&[0; 16], Direction::Bidirectional)?;
        let keep_warm = allocate(&[0; KEEP_WARM_BYTES], Direction::Bidirectional)?;
        let mut rings = Vec::with_capacity(QUEUES);
        for _ in 0..QUEUES {
            rings.push(allocate(&[0; RING_BYTES], Direction::Bidirectional)?);
        }
        let mut boot = Self {
            io: Some(io),
            profile,
            firmware,
            image,
            rbds,
            status,
            buffers,
            keep_warm,
            rings,
            read: 0,
            state: State::Poisoned,
        };
        boot.start_hw()?;
        boot.start_fw(pieces)?;
        boot.state = State::Loaded;
        Ok(boot)
    }

    pub fn firmware(&self) -> &Firmware<'a> {
        &self.firmware
    }
    pub fn firmware_source(&self) -> resources::Image<'a> {
        self.image
    }

    fn io(&mut self) -> &mut Q {
        self.io.as_mut().expect("retained owner")
    }
    fn read(&mut self, register: u32) -> Result<u32, Error> {
        let value = self.io().read32(register)?;
        if value == u32::MAX || matches!(value & !0xf, 0xa5a5a5a0 | 0x5a5a5a50) {
            return Err(Error::FailedRead { register, value });
        }
        Ok(value)
    }
    fn write(&mut self, register: u32, value: u32) -> Result<(), Error> {
        self.io().write32(register, value)?;
        Ok(())
    }
    fn set(&mut self, register: u32, bits: u32) -> Result<(), Error> {
        let value = self.read(register)?;
        self.write(register, value | bits)
    }
    /// `iwl_poll_bits_mask`: 10 us steps up to `microseconds`.
    fn poll_register(
        &mut self,
        register: u32,
        mask: u32,
        value: u32,
        microseconds: u32,
    ) -> Result<(), Error> {
        for _ in 0..microseconds.div_ceil(10) {
            if self.read(register)? & mask == value {
                return Ok(());
            }
            self.io().delay_us(10)?;
        }
        Err(Error::Timeout { register })
    }
    /// `__iwl_trans_pcie_grab_nic_access`.
    fn grab(&mut self) -> Result<(), Error> {
        self.set(GP, ACCESS_REQUEST)?;
        self.io().delay_us(2)?;
        self.poll_register(GP, CLOCK_READY | GOING_TO_SLEEP, CLOCK_READY, 15000)
    }
    fn release(&mut self) -> Result<(), Error> {
        let gp = self.read(GP)?;
        self.write(GP, gp & !ACCESS_REQUEST)
    }
    /// `iwl_write_direct32`.
    fn write_direct(&mut self, register: u32, value: u32) -> Result<(), Error> {
        self.grab()?;
        self.write(register, value)?;
        self.release()
    }
    /// `iwl_read_direct32`.
    fn read_direct(&mut self, register: u32) -> Result<u32, Error> {
        self.grab()?;
        let value = self.read(register)?;
        self.release()?;
        Ok(value)
    }
    fn prph_read_held(&mut self, address: u32) -> Result<u32, Error> {
        self.write(PRPH_RADDR, (address & 0xfffff) | (3 << 24))?;
        self.read(PRPH_RDATA)
    }
    fn prph_write_held(&mut self, address: u32, value: u32) -> Result<(), Error> {
        self.write(PRPH_WADDR, (address & 0xfffff) | (3 << 24))?;
        self.write(PRPH_WDATA, value)
    }
    /// `iwl_read_prph`.
    fn prph_read(&mut self, address: u32) -> Result<u32, Error> {
        self.grab()?;
        let value = self.prph_read_held(address)?;
        self.release()?;
        Ok(value)
    }
    /// `iwl_write_prph`.
    fn prph_write(&mut self, address: u32, value: u32) -> Result<(), Error> {
        self.grab()?;
        self.prph_write_held(address, value)?;
        self.release()
    }
    /// `iwl_set_bits_mask_prph`, one NIC access for the read and the write.
    fn prph_update(&mut self, address: u32, clear: u32, set: u32) -> Result<(), Error> {
        self.grab()?;
        let value = self.prph_read_held(address)?;
        self.prph_write_held(address, (value & !clear) | set)?;
        self.release()
    }

    /// `iwl_pcie_set_hw_ready`.
    fn hw_ready(&mut self) -> Result<(), Error> {
        self.set(HW_IF, OWN)?;
        self.poll_register(HW_IF, OWN, OWN, 50)?;
        self.set(MAILBOX, OS_ALIVE)
    }
    /// `iwl_pcie_prepare_card_hw`, with no CSME session to defer to.
    fn prepare_card_hw(&mut self) -> Result<(), Error> {
        match self.hw_ready() {
            Err(Error::Timeout { .. }) => {}
            other => return other,
        }
        self.set(LINK_POWER, 1 << 31)?;
        self.io().delay_us(1000)?;
        for _ in 0..10 {
            self.set(HW_IF, WAKE_ME)?;
            for _ in 0..750 {
                match self.hw_ready() {
                    Err(Error::Timeout { .. }) => {}
                    other => return other,
                }
                self.io().delay_us(200)?;
            }
            self.io().delay_us(25000)?;
        }
        Err(Error::Timeout { register: HW_IF })
    }
    /// `iwl_pcie_apm_init` for the 8000 family, which has no APMG.
    fn apm_init(&mut self) -> Result<(), Error> {
        self.set(CHICKEN, 0x00800000)?;
        self.set(HPET, 0xffff0000)?;
        self.set(HW_IF, HAP_WAKE)?;
        self.set(GIO, 2)?;
        self.set(GP, INIT_DONE)?;
        self.poll_register(GP, CLOCK_READY, CLOCK_READY, 25000)
    }
    /// `iwl_pcie_check_hw_rf_kill`.
    fn check_rf_kill(&mut self) -> Result<(), Error> {
        if self.read(GP)? & RF_KILL_SW == 0 {
            return Err(Error::RfKilled);
        }
        Ok(())
    }

    /// `_iwl_trans_pcie_start_hw`, interrupts left masked.
    fn start_hw(&mut self) -> Result<(), Error> {
        let observed = self.profile.observed();
        if self.read(HW_REV)? != observed.hardware_revision
            || self.read(RF_ID)? != observed.radio_id
        {
            return Err(Error::IdentityChanged);
        }
        self.write(INT_MASK, 0)?;
        self.write(INT, u32::MAX)?;
        self.write(FH_INT, u32::MAX)?;
        self.prepare_card_hw()?;
        // `iwl_trans_pcie_sw_reset`, which retakes ownership.
        self.set(RESET, SW_RESET)?;
        self.io().delay_us(5000)?;
        self.prepare_card_hw()?;
        self.apm_init()?;
        self.check_rf_kill()
    }

    /// `iwl_trans_pcie_start_fw` and `iwl_pcie_load_given_ucode_8000`.
    fn start_fw(&mut self, pieces: [Vec<Vec<Piece>>; 2]) -> Result<(), Error> {
        self.prepare_card_hw()?;
        self.write(INT, u32::MAX)?;
        self.check_rf_kill()?;
        self.write(GP1_CLEAR, RFKILL_HANDSHAKE)?;
        self.write(GP1_CLEAR, CMD_BLOCKED)?;
        self.write(INT, u32::MAX)?;
        // `iwl_pcie_nic_init`.
        self.apm_init()?;
        self.nic_config()?;
        self.rx_init()?;
        self.tx_init()?;
        self.set(SHADOW, 0x800fffff)?;
        self.write(GP1_CLEAR, RFKILL_HANDSHAKE)?;
        self.write(GP1_CLEAR, RFKILL_HANDSHAKE)?;
        // Read before it is written, as Linux logs it.
        self.prph_read(WFPM_GP2)?;
        self.prph_write(WFPM_GP2, 0x01010101)?;
        self.prph_write(RELEASE_CPU_RESET, 1 << 24)?;
        for (cpu, sections) in pieces.into_iter().enumerate() {
            let shift = 16 * cpu as u32;
            let mut number = 1u32;
            for section in sections {
                for piece in section {
                    self.load_piece(piece)?;
                }
                let status = self.read_direct(LOAD_STATUS)?;
                self.write_direct(LOAD_STATUS, status | (number << shift))?;
                number = (number << 1) | 1;
            }
            self.write_direct(LOAD_STATUS, if cpu == 0 { 0xffff } else { u32::MAX })?;
        }
        self.check_rf_kill()
    }

    /// `iwl_mvm_nic_config`: the MAC step and the image's radio type, step
    /// and dash.
    fn nic_config(&mut self) -> Result<(), Error> {
        let phy = self.firmware.phy_sku().ok_or(Error::PhyConfiguration)?;
        let config = (self.profile.observed().hardware_revision & 0xf)
            | ((phy & 3) << 10)
            | (((phy >> 2) & 3) << 14)
            | (((phy >> 4) & 3) << 12);
        let value = self.read(HW_IF)?;
        self.write(HW_IF, (value & !NIC_CONFIG) | config)
    }

    /// `iwl_pcie_rx_init` for the legacy queue: the RBDs already name every
    /// buffer, so restocking is the write pointer alone.
    fn rx_init(&mut self) -> Result<(), Error> {
        self.rbds.expose();
        self.status.expose();
        for buffer in &mut self.buffers {
            buffer.expose();
        }
        let rbds = self.rbds.region().address;
        let status = self.status.region().address;
        self.grab()?;
        self.write(RX_CONFIG, 0)?;
        self.write(RX_RBDCB_WPTR, 0)?;
        self.write(RX_FLUSH, 0)?;
        self.write(RX_RDPTR, 0)?;
        self.write(RX_WPTR, 0)?;
        self.write(RX_RBDCB_BASE, (rbds >> 8) as u32)?;
        self.write(RX_STATUS_WPTR, (status >> 4) as u32)?;
        self.write(RX_CONFIG, RX_ENABLE)?;
        self.release()?;
        self.io().write8(INT_COALESCING, HOST_INT_TIMEOUT)?;
        fence(Ordering::Release);
        // `iwl_pcie_rxq_restock`, then `iwl_pcie_rx_init`, each move it.
        self.write(RX_WPTR, u32::from(RX_PUBLISHED))?;
        self.write(RX_WPTR, u32::from(RX_PUBLISHED))
    }

    /// `iwl_pcie_tx_init`: the keep-warm buffer and every queue's ring.
    fn tx_init(&mut self) -> Result<(), Error> {
        self.keep_warm.expose();
        for ring in &mut self.rings {
            ring.expose();
        }
        self.prph_write(SCD_TXFACT, 0)?;
        let keep_warm = self.keep_warm.region().address;
        self.write_direct(KEEP_WARM, (keep_warm >> 4) as u32)?;
        for queue in 0..QUEUES {
            let ring = self.rings[queue].region().address;
            let register = match queue {
                0..16 => 0x19d0 + 4 * queue as u32,
                16..20 => 0x1bf0 + 4 * (queue - 16) as u32,
                _ => 0x1b20 + 4 * (queue - 20) as u32,
            };
            self.write_direct(register, (ring >> 8) as u32)?;
        }
        self.prph_update(SCD_GP_CTRL, 0, SCD_AUTO_ACTIVE)?;
        self.prph_update(SCD_GP_CTRL, 0, SCD_31_QUEUES)
    }

    /// `iwl_pcie_load_section`'s step: one piece through the service
    /// channel, then its FH_TX completion. The piece is released once the
    /// device has read it.
    fn load_piece(&mut self, mut piece: Piece) -> Result<(), Error> {
        let extended = EXTENDED.contains(&piece.destination);
        if extended {
            self.prph_update(LMPM_CHICK, 0, EXTENDED_ADDRESS)?;
        }
        let region = piece.chunk.region();
        piece.chunk.expose();
        self.grab()?;
        self.write(SERVICE_CONFIG, 0)?;
        self.write(SERVICE_SRAM, piece.destination)?;
        self.write(SERVICE_DRAM_LOW, region.address as u32)?;
        self.write(
            SERVICE_DRAM_HIGH,
            (((region.address >> 32) as u32) << 28) | region.bytes as u32,
        )?;
        self.write(SERVICE_STATUS, SERVICE_VALID)?;
        self.write(SERVICE_CONFIG, SERVICE_ENABLE)?;
        self.release()?;
        let mut done = false;
        for _ in 0..CHUNK_TIMEOUT_US / 10 {
            let causes = self.read(INT)?;
            if causes & (INT_HW_ERR | INT_SW_ERR) != 0 {
                return Err(Error::DeviceError(causes));
            }
            if causes & INT_FH_TX != 0 {
                self.write(INT, INT_FH_TX)?;
                let fh = self.read(FH_INT)?;
                self.write(FH_INT, fh)?;
                done = true;
                break;
            }
            self.io().delay_us(10)?;
        }
        if !done {
            return Err(Error::Timeout {
                register: SERVICE_CONFIG,
            });
        }
        if extended {
            self.prph_update(LMPM_CHICK, EXTENDED_ADDRESS, 0)?;
        }
        // SAFETY: FH_TX reports this TFD's end: the service channel has read
        // the whole piece and holds no other descriptor of it.
        unsafe { piece.chunk.release()? };
        Ok(())
    }

    /// Look once for the image's ALIVE on the RX queue.
    pub fn poll(&mut self) -> Result<Option<Alive>, Error> {
        if self.state != State::Loaded {
            return Err(Error::State);
        }
        let result = self.poll_inner();
        match result {
            Ok(Some(_)) => self.state = State::Alive,
            Err(_) => self.state = State::Poisoned,
            Ok(None) => {}
        }
        result
    }
    fn poll_inner(&mut self) -> Result<Option<Alive>, Error> {
        let causes = self.read(INT)?;
        if causes != 0 {
            self.write(INT, causes)?;
            let fh = self.read(FH_INT)?;
            self.write(FH_INT, fh)?;
        }
        if causes & (INT_HW_ERR | INT_SW_ERR) != 0 {
            return Err(Error::DeviceError(causes));
        }
        self.check_rf_kill()?;
        // SAFETY: the status block is pinned, coherent and device-written;
        // this reads one aligned u16 without claiming it.
        let closed = unsafe { self.status.read16_coherent(0)? } & 0xfff;
        fence(Ordering::Acquire);
        let closed = closed % RX_QUEUE_SIZE as u16;
        while self.read != closed {
            let slot = self.read;
            if slot >= RX_PUBLISHED {
                return Err(Error::CompletionId(slot));
            }
            self.read = (slot + 1) % RX_QUEUE_SIZE as u16;
            // SAFETY: the closed RB count passed this slot: the device has
            // written and handed back its buffer.
            let bytes = unsafe { self.buffers[usize::from(slot)].snapshot_completed()? };
            // The legacy queue packs frames as the AX200 does.
            for packet in Packets::new(firmware::Family::Ax200, 0, &bytes)? {
                let packet = packet?;
                if packet.command == 1 && packet.group == 0 {
                    return Ok(Some(Alive::decode(packet.payload())?));
                }
            }
        }
        Ok(None)
    }
    /// Poll for ALIVE every millisecond, up to `milliseconds`
    /// (`MVM_UCODE_ALIVE_TIMEOUT` is two seconds).
    pub fn wait_alive(&mut self, milliseconds: u32) -> Result<Alive, Error> {
        for _ in 0..milliseconds {
            if let Some(alive) = self.poll()? {
                return Ok(alive);
            }
            self.io().delay_us(1000)?;
        }
        Err(Error::Timeout { register: INT })
    }

    /// Bus-master stop and reset, as `iwl_pcie_apm_stop` does, then the
    /// release of every allocation. A stop the device does not acknowledge
    /// frees nothing.
    pub fn stop(&mut self) -> Result<(), Error> {
        if self.state == State::Stopped {
            return Err(Error::State);
        }
        self.state = State::Poisoned;
        self.write(INT_MASK, 0)?;
        self.set(RESET, STOP_MASTER)?;
        self.poll_register(RESET, MASTER_DISABLED, MASTER_DISABLED, 100)?;
        self.set(RESET, SW_RESET)?;
        self.io().delay_us(5000)?;
        let gp = self.read(GP)?;
        self.write(GP, gp & !INIT_DONE)?;
        // SAFETY: master disable was acknowledged and the function reset: the
        // device no longer reads or writes any of these.
        unsafe {
            self.rbds.release()?;
            self.status.release()?;
            self.keep_warm.release()?;
            for chunk in self.buffers.iter_mut().chain(&mut self.rings) {
                chunk.release()?;
            }
        }
        self.state = State::Stopped;
        Ok(())
    }
}

impl<Q: BootIo> Drop for Boot<'_, Q> {
    fn drop(&mut self) {
        // A function that may still run keeps its owner: its exposed pages
        // stay pinned and the claim is never handed back.
        if self.state != State::Stopped
            && let Some(owner) = self.io.take()
        {
            core::mem::forget(owner);
        }
    }
}

/// Boot the INIT image, wait for its ALIVE and stop the function again.
///
/// # Safety
/// [`Boot::start`]'s.
pub unsafe fn init_alive<Q: BootIo>(
    io: Q,
    profile: Profile,
    staged: Staged<'_>,
    source: &dyn PageSource,
    domain: Arc<dyn DmaDomain>,
) -> Result<Alive, Error> {
    // SAFETY: forwarded from the caller.
    let mut boot = unsafe { Boot::start(io, profile, staged, ImageKind::Init, source, domain)? };
    let alive = boot.wait_alive(2000)?;
    boot.stop()?;
    Ok(alive)
}

#[cfg(test)]
mod tests;
