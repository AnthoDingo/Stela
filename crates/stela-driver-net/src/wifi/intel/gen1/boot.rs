// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright (C) 2003-2014, 2018-2026 Intel Corporation
// Copyright (C) 2013-2015 Intel Mobile Communications GmbH
// Copyright (C) 2016-2017 Intel Deutschland GmbH
//! The 8000 and 9000 families' gen1 transport, as `iwl_trans_pcie_start_hw`,
//! `iwl_trans_pcie_start_fw`, `iwl_pcie_load_given_ucode_8000`,
//! `iwl_pcie_tx_start` and `iwl_pcie_enqueue_hcmd` drive it in
//! pcie/gen1_2/{trans,rx,tx}.c at Linux
//! a804c5742bff224d4cb4d25dd383554d598be32b.
//!
//! The driver itself copies each section of the image into the device's SRAM
//! through the FH service channel, then the firmware boots and answers on RX
//! queue 0: the legacy queue on the 8000 family, the RFH's first queue on
//! the 9000 family. Once the scheduler runs, host commands go out one at a
//! time on TX queue 0. Interrupts stay masked: CSR_INT is polled. ALIVE is
//! boot metadata, not NVM, calibration or radio readiness.
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
        command::{Command, Id},
        dma::{self, Chunk},
        firmware::{self, Firmware, ImageKind, Section},
        identity::Staged,
        receive::{self, Packets},
    },
};
use alloc::{collections::VecDeque, sync::Arc, vec::Vec};
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
const RX_BUFFER_BYTES: usize = 4096;
const HOST_INT_TIMEOUT: u8 = 0x40;

// RFH queue 0 (rx.c `iwl_pcie_rx_mq_hw_init`), peripheral addresses.
const RFH_DMA_CFG: u32 = 0xa09820;
const RFH_RXQ_ACTIVE: u32 = 0xa0980c;
const RFH_GEN_CFG: u32 = 0xa09800;
const RFH_FREE_BASE: u32 = 0xa08000;
const RFH_USED_BASE: u32 = 0xa08100;
const RFH_STATUS_BASE: u32 = 0xa08200;
const RFH_FREE_WIDX: u32 = 0xa08080;
const RFH_FREE_RIDX: u32 = 0xa080c0;
const RFH_USED_WIDX: u32 = 0xa08180;
/// The CSR that publishes queue 0's free-BD write index.
const RFH_FREE_WIDX_TRIGGER: u32 = 0x1c80;
/// Enabled, 4 KiB buffers, minimum RB 4-8, oversize frames dropped, 512 BDs.
const RFH_ENABLE: u32 = 0x80000000 | (4 << 16) | (3 << 24) | (1 << 26) | (9 << 20);
/// Both DMA snoops, default queue 0 and, on an integrated MAC, 64-byte
/// chunks.
const RFH_GENERAL: u32 = 3;
/// Queue 0's free and used rings.
const RFH_QUEUE_0: u32 = 1 | (1 << 16);
/// `UREG_CHICK` and `UREG_CHICK_MSI_ENABLE`: `iwl_pcie_conf_msix_hw` with
/// MSI-X off and multi-queue RX.
const UREG_CHICK: u32 = 0xa05c00;
const UREG_CHICK_MSI: u32 = 1 << 24;
/// The AX200's free and used descriptor layouts are the 9000's.
const RFH_LAYOUT: firmware::Family = firmware::Family::Ax200;

/// RX queue 0's shape: its entries, and the buffers restocked into it.
/// `iwl_pcie_rx{s,mq}_restock` fills every slot but one, and the write
/// pointer moves in steps of eight.
const fn rx_entries(family: Family) -> u16 {
    match family {
        Family::F8000 => 256,
        Family::F9000 => 512,
    }
}
const fn rx_buffers(family: Family) -> usize {
    rx_entries(family) as usize - 1
}
const fn rx_published(family: Family) -> u16 {
    (rx_entries(family) - 1) & !7
}

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

// The scheduler and queue 0 (tx.c `iwl_pcie_tx_start`,
// `iwl_trans_pcie_txq_enable`).
const SCD_SRAM_BASE: u32 = 0xa02c00;
const SCD_DRAM_BASE: u32 = 0xa02c08;
const SCD_QUEUE_0_READ: u32 = 0xa02c68;
const SCD_QUEUE_0_STATUS: u32 = 0xa02d0c;
const SCD_AGGREGATION: u32 = 0xa02e48;
const SCD_ENABLE: u32 = 0xa02e54;
/// Inactive, its activation enabled.
const SCD_QUEUE_INACTIVE: u32 = 1 << 19;
/// Active on FIFO 7 (`IWL_MVM_TX_FIFO_CMD`), with the window-size limit
/// and every status bit.
const SCD_QUEUE_COMMANDS: u32 = (1 << 3) | 7 | (1 << 4) | 0x017f_0000;
/// `SCD_CONTEXT_MEM_LOWER_BOUND`, and the words up to
/// `SCD_TRANS_TBL_OFFSET_QUEUE(31)` that the start clears.
const SCD_CONTEXT: u32 = 0x600;
const SCD_CONTEXT_WORDS: usize = 135;
/// Window size and frame limit, both `IWL_FRAME_LIMIT`.
const SCD_QUEUE_0_WINDOW: u32 = 64 | (64 << 16);
/// Every FIFO active.
const SCD_FIFOS: u32 = 0xff;
const HBUS_TARG_MEM_WADDR: u32 = 0x410;
const HBUS_TARG_MEM_WDAT: u32 = 0x418;
const HBUS_TARG_WRPTR: u32 = 0x460;
/// The eight TX DMA channels' configuration, with DMA and credits enabled.
const TX_CHANNEL: u32 = 0x1d00;
const TX_CHANNEL_ENABLE: u32 = 0x8000_0008;
const TX_CHICKEN: u32 = 0x1e98;
const TX_CHICKEN_AUTO_RETRY: u32 = 2;
/// 31 queues' byte-count tables, 320 two-byte entries each.
const BYTE_COUNT_BYTES: usize = QUEUES * 320 * 2;
/// A gen1 TFD: three reserved bytes, the TB count, then 20 TBs of a 32-bit
/// address and a 16-bit word of the address's high nibble and the length.
const TFD_BYTES: usize = 128;
const TFD_SLOTS: u16 = 256;
/// `IWL_FIRST_TB_SIZE`.
const FIRST_TB: usize = 20;
/// ponytail: one command buffer, so one command in flight; every INIT and
/// RT command up to the PHY DB's fits. A buffer per TFD slot when commands
/// go asynchronous.
const COMMAND_BYTES: usize = 4096;
/// `HOST_COMPLETE_TIMEOUT`.
const COMMAND_TIMEOUT_US: u32 = 2_000_000;

// Firmware paging (fw/paging.c): the CSS in a 4 KiB block, then the paged
// image in blocks of eight 4 KiB pages.
const PAGE_BYTES: usize = 4096;
const PAGING_BLOCK_BYTES: usize = 8 * PAGE_BYTES;
/// `NUM_OF_FW_PAGING_BLOCKS`: the CSS's and 32 data blocks.
const PAGING_BLOCKS: usize = 33;
/// `PAGING_CMD_IS_SECURED | PAGING_CMD_IS_ENABLED`, and `BLOCK_2_EXP_SIZE`.
const PAGING_SECURED_ENABLED: u32 = 0x300;
const PAGING_BLOCK_SHIFT: u32 = 15;

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

/// One RX ring entry naming `buffer` (numbered from zero) at `address`:
/// the legacy RBD's page, or `iwl_pcie_restock_bd`'s page with its virtual
/// RB ID, here the buffer's number from one.
fn rbd(family: Family, address: u64, buffer: u16) -> Result<Vec<u8>, Error> {
    Ok(match family {
        Family::F8000 => ((address >> 8) as u32).to_le_bytes().to_vec(),
        Family::F9000 => match receive::free_descriptor(RFH_LAYOUT, address, buffer + 1)? {
            receive::FreeDescriptor::Ax200(bytes) => bytes.to_vec(),
            receive::FreeDescriptor::Ax210(_) => unreachable!("the AX200 layout"),
        },
    })
}
const fn rbd_stride(family: Family) -> usize {
    match family {
        Family::F8000 => 4,
        Family::F9000 => 8,
    }
}

/// `iwl_pcie_gen1_tfd_set_tb` for each region, in order.
fn tfd(regions: &[(u64, usize)]) -> [u8; TFD_BYTES] {
    let mut bytes = [0; TFD_BYTES];
    bytes[3] = regions.len() as u8;
    for (n, &(address, length)) in regions.iter().enumerate() {
        let at = 4 + 6 * n;
        bytes[at..at + 4].copy_from_slice(&(address as u32).to_le_bytes());
        let high = ((address >> 32) & 0xf) as u16 | ((length as u16) << 4);
        bytes[at + 4..at + 6].copy_from_slice(&high.to_le_bytes());
    }
    bytes
}

/// A packet the firmware sent on RX queue 0, copied out of its RB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    pub command: u8,
    pub group: u8,
    pub sequence: u16,
    pub payload: Vec<u8>,
}
impl Notification {
    /// `SEQ_RX_FRAME`: sent by the firmware itself, not a command's response.
    pub fn unsolicited(&self) -> bool {
        self.sequence & 0x8000 != 0
    }
}

/// `iwl_alloc_fw_paging_mem` and `iwl_fill_paging_mem`: the image's paged
/// part is the two sections after the paging separator, its CSS then its
/// pages, sized by the PAGING TLV. Each block is one allocation; the result
/// also gives the pages in the last block.
fn paging_blocks(
    firmware: &Firmware<'_>,
    kind: ImageKind,
    allocate: impl Fn(&[u8]) -> Result<Chunk, Error>,
) -> Result<(Vec<Chunk>, u32), Error> {
    let invalid = || Error::Firmware(firmware::Error::Missing("paging sections"));
    let Some(bytes) = firmware.paging_bytes() else {
        return Ok((Vec::new(), 0));
    };
    let sections: Vec<_> = firmware
        .sections()
        .iter()
        .filter(|section| section.image == kind)
        .collect();
    let separator = sections
        .iter()
        .position(|s| s.offset == PAGING_SEPARATOR)
        .ok_or_else(invalid)?;
    let (Some(css), Some(pages)) = (sections.get(separator + 1), sections.get(separator + 2))
    else {
        return Err(invalid());
    };
    let bytes = bytes as usize;
    let count = bytes.div_ceil(PAGING_BLOCK_BYTES);
    let last = bytes / PAGE_BYTES - 8 * (count - 1);
    if count == 0
        || count >= PAGING_BLOCKS
        || css.data.len() > PAGE_BYTES
        || pages.data.len() != (count - 1) * PAGING_BLOCK_BYTES + last * PAGE_BYTES
    {
        return Err(invalid());
    }
    let mut block = alloc::vec![0; PAGE_BYTES];
    block[..css.data.len()].copy_from_slice(css.data);
    let mut blocks = alloc::vec![allocate(&block)?];
    for data in pages.data.chunks(PAGING_BLOCK_BYTES) {
        let mut block = alloc::vec![0; PAGING_BLOCK_BYTES];
        block[..data.len()].copy_from_slice(data);
        blocks.push(allocate(&block)?);
    }
    Ok((blocks, last as u32))
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
    /// The scheduler runs: host commands may go out.
    Running,
    Poisoned,
    Stopped,
}

/// One image booted on a gen1 function, keeping every allocation the
/// device was told about until a stop acknowledges its DMA.
pub struct Boot<'a, Q: BootIo> {
    io: Option<Q>,
    profile: Profile,
    firmware: Firmware<'a>,
    image: resources::Image<'a>,
    /// The legacy RBDs, or the RFH's free BDs.
    rbds: Chunk,
    /// The RFH's used BDs; the legacy queue has none.
    used: Option<Chunk>,
    status: Chunk,
    buffers: Vec<Chunk>,
    keep_warm: Chunk,
    rings: Vec<Chunk>,
    /// A runtime image's paging blocks, and the pages in the last one.
    paging: Vec<Chunk>,
    paging_last: u32,
    byte_counts: Chunk,
    command: Chunk,
    /// The next RB the device closes.
    read: u16,
    /// The next RX ring slot to restock, and the write index the device
    /// was last told.
    rx_write: u16,
    rx_published: u16,
    /// Each RX ring slot's buffer, and whether the device holds each buffer.
    slot_buffer: Vec<u16>,
    device_owned: Vec<bool>,
    /// Queue 0's next TFD.
    command_write: u16,
    received: VecDeque<Notification>,
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
        let family = profile.family();
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
        let entries = usize::from(rx_entries(family));
        let mut buffers = Vec::with_capacity(rx_buffers(family));
        let stride = rbd_stride(family);
        let mut rbds = alloc::vec![0; entries * stride];
        for (slot, entry) in rbds
            .chunks_exact_mut(stride)
            .take(rx_buffers(family))
            .enumerate()
        {
            let buffer = allocate(&[0; RX_BUFFER_BYTES], Direction::FromDevice)?;
            entry.copy_from_slice(&rbd(family, buffer.region().address, slot as u16)?);
            buffers.push(buffer);
        }
        let rbds = allocate(&rbds, Direction::Bidirectional)?;
        let used = match family {
            Family::F8000 => None,
            Family::F9000 => Some(allocate(
                &alloc::vec![0; entries * receive::used_stride(RFH_LAYOUT)],
                Direction::Bidirectional,
            )?),
        };
        let status = allocate(&[0; 16], Direction::Bidirectional)?;
        let keep_warm = allocate(&[0; KEEP_WARM_BYTES], Direction::Bidirectional)?;
        let mut rings = Vec::with_capacity(QUEUES);
        for _ in 0..QUEUES {
            rings.push(allocate(&[0; RING_BYTES], Direction::Bidirectional)?);
        }
        let (paging, paging_last) = match kind {
            ImageKind::Runtime => paging_blocks(&firmware, kind, |bytes| {
                allocate(bytes, Direction::Bidirectional)
            })?,
            _ => (Vec::new(), 0),
        };
        let byte_counts = allocate(&[0; BYTE_COUNT_BYTES], Direction::Bidirectional)?;
        let command = allocate(&[0; COMMAND_BYTES], Direction::ToDevice)?;
        let mut slot_buffer: Vec<u16> = (0..rx_buffers(family) as u16).collect();
        slot_buffer.push(0);
        let mut boot = Self {
            io: Some(io),
            profile,
            firmware,
            image,
            rbds,
            used,
            status,
            buffers,
            keep_warm,
            rings,
            paging,
            paging_last,
            byte_counts,
            command,
            read: 0,
            rx_write: rx_buffers(family) as u16,
            rx_published: rx_published(family),
            slot_buffer,
            device_owned: alloc::vec![true; rx_buffers(family)],
            command_write: 0,
            received: VecDeque::new(),
            state: State::Poisoned,
        };
        boot.start_hw()?;
        boot.start_fw(pieces)?;
        boot.state = State::Loaded;
        Ok(boot)
    }

    /// `iwl_send_paging_cmd`'s payload, which hands the paging blocks to
    /// the device; `None` for an image without paging.
    pub fn paging_command(&mut self) -> Option<Vec<u8>> {
        if self.paging.is_empty() {
            return None;
        }
        let count = self.paging.len() as u32 - 1;
        let mut payload = Vec::with_capacity(12 + 4 * PAGING_BLOCKS);
        for word in [
            PAGING_SECURED_ENABLED | self.paging_last,
            PAGING_BLOCK_SHIFT,
            count,
        ] {
            payload.extend(word.to_le_bytes());
        }
        for block in &mut self.paging {
            block.expose();
            payload.extend(((block.region().address >> 12) as u32).to_le_bytes());
        }
        payload.resize(12 + 4 * PAGING_BLOCKS, 0);
        Some(payload)
    }

    /// The function's owner back, once a stop has freed every allocation.
    pub fn into_owner(mut self) -> Result<Q, Error> {
        if self.state != State::Stopped {
            return Err(Error::State);
        }
        Ok(self.io.take().expect("retained owner"))
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
    pub(super) fn read(&mut self, register: u32) -> Result<u32, Error> {
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
    pub(super) fn prph_read(&mut self, address: u32) -> Result<u32, Error> {
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
    /// `iwl_trans_write_mem`: SRAM words through HBUS_TARG_MEM, under one
    /// NIC access.
    fn write_memory(&mut self, address: u32, words: &[u32]) -> Result<(), Error> {
        self.grab()?;
        self.write(HBUS_TARG_MEM_WADDR, address)?;
        for &word in words {
            self.write(HBUS_TARG_MEM_WDAT, word)?;
        }
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
        if self.profile.family() == Family::F9000 {
            self.prph_write(UREG_CHICK, UREG_CHICK_MSI)?;
        }
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

    /// `iwl_pcie_rx_init`: the RBDs already name every buffer, so restocking
    /// is the write pointer alone, which moves twice as Linux moves it, once
    /// in `iwl_pcie_rx{s,mq}_restock` and once in `iwl_pcie_rx_init`.
    fn rx_init(&mut self) -> Result<(), Error> {
        self.rbds.expose();
        self.status.expose();
        if let Some(used) = &mut self.used {
            used.expose();
        }
        for buffer in &mut self.buffers {
            buffer.expose();
        }
        match self.profile.family() {
            Family::F8000 => self.legacy_rx_init()?,
            Family::F9000 => self.rfh_init()?,
        }
        self.io().write8(INT_COALESCING, HOST_INT_TIMEOUT)?;
        fence(Ordering::Release);
        let (trigger, published) = (self.rx_trigger(), u32::from(self.rx_published));
        self.write(trigger, published)?;
        self.write(trigger, published)
    }

    /// The CSR that publishes RX queue 0's write index.
    fn rx_trigger(&self) -> u32 {
        match self.profile.family() {
            Family::F8000 => RX_WPTR,
            Family::F9000 => RFH_FREE_WIDX_TRIGGER,
        }
    }

    /// `iwl_pcie_rx_mq_hw_init` for queue 0.
    fn rfh_init(&mut self) -> Result<(), Error> {
        let used = self.used.as_ref().expect("an RFH queue").region().address;
        let rings = [
            (RFH_FREE_BASE, self.rbds.region().address),
            (RFH_USED_BASE, used),
            (RFH_STATUS_BASE, self.status.region().address),
        ];
        self.grab()?;
        self.prph_write_held(RFH_DMA_CFG, 0)?;
        self.prph_write_held(RFH_RXQ_ACTIVE, 0)?;
        for (register, address) in rings {
            self.prph_write_held(register, address as u32)?;
            self.prph_write_held(register + 4, (address >> 32) as u32)?;
        }
        self.prph_write_held(RFH_FREE_WIDX, 0)?;
        self.prph_write_held(RFH_FREE_RIDX, 0)?;
        self.prph_write_held(RFH_USED_WIDX, 0)?;
        self.prph_write_held(RFH_DMA_CFG, RFH_ENABLE)?;
        self.prph_write_held(RFH_GEN_CFG, RFH_GENERAL)?;
        self.prph_write_held(RFH_RXQ_ACTIVE, RFH_QUEUE_0)?;
        self.release()
    }

    /// `iwl_pcie_rx_hw_init`.
    fn legacy_rx_init(&mut self) -> Result<(), Error> {
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
        self.release()
    }

    /// `iwl_pcie_tx_init`: the keep-warm buffer and every queue's ring.
    fn tx_init(&mut self) -> Result<(), Error> {
        self.keep_warm.expose();
        self.byte_counts.expose();
        self.command.expose();
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
    /// What came before ALIVE is dropped; what follows it stays queued.
    fn poll_inner(&mut self) -> Result<Option<Alive>, Error> {
        self.receive()?;
        while let Some(packet) = self.received.pop_front() {
            if packet.command == 1 && packet.group == 0 {
                return Ok(Some(Alive::decode(&packet.payload)?));
            }
        }
        Ok(None)
    }

    /// Take every RB the device closed: its packets join `received`, and
    /// the buffer goes straight back on the ring, as `iwl_pcie_rx_handle`
    /// restocks, with one write-index update for the pass.
    fn receive(&mut self) -> Result<(), Error> {
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
        let family = self.profile.family();
        let closed = closed % rx_entries(family);
        while self.read != closed {
            let slot = self.read;
            self.read = (slot + 1) % rx_entries(family);
            // The legacy queue closes its RBDs in order; the RFH names the
            // closed buffer in the used ring, by the ID it was given.
            let buffer = match &self.used {
                None => self.slot_buffer[usize::from(slot)],
                Some(used) => {
                    let stride = receive::used_stride(RFH_LAYOUT);
                    // SAFETY: the used ring is pinned and coherent, and the
                    // closed count says the device wrote this entry.
                    let raw = unsafe { used.read32_coherent(usize::from(slot) * stride)? };
                    receive::used_descriptor(RFH_LAYOUT, &raw.to_le_bytes())?.id - 1
                }
            };
            match self.device_owned.get_mut(usize::from(buffer)) {
                Some(owned @ true) => *owned = false,
                _ => return Err(Error::CompletionId(buffer)),
            }
            // SAFETY: the device closed this buffer it held: it has written
            // and handed it back.
            let bytes = unsafe { self.buffers[usize::from(buffer)].snapshot_completed()? };
            // The legacy queue packs frames as the AX200 does.
            for packet in Packets::new(firmware::Family::Ax200, 0, &bytes)? {
                let packet = packet?;
                self.received.push_back(Notification {
                    command: packet.command,
                    group: packet.group,
                    sequence: packet.sequence,
                    payload: packet.payload().to_vec(),
                });
            }
            self.restock(buffer)?;
        }
        let write = self.rx_write & !7;
        if write != self.rx_published {
            fence(Ordering::Release);
            self.rx_published = write;
            let trigger = self.rx_trigger();
            self.write(trigger, u32::from(write))?;
        }
        Ok(())
    }

    /// Give `buffer` back to the device at the next ring slot. That slot
    /// trails every RB the device closed: the device takes slots in order,
    /// so the entry there named a buffer it has already handed back.
    fn restock(&mut self, buffer: u16) -> Result<(), Error> {
        let family = self.profile.family();
        let (slot, stride) = (usize::from(self.rx_write), rbd_stride(family));
        let chunk = &self.buffers[usize::from(buffer)];
        let entry = rbd(family, chunk.region().address, buffer)?;
        // SAFETY: the device handed both the buffer and the slot back (see
        // above), and only ever reads the ring; the buffer returns to it
        // whole, after this CPU's copy.
        unsafe {
            chunk.ready_for_device()?;
            self.rbds.write_owned(slot * stride, &entry)?;
        }
        self.slot_buffer[slot] = buffer;
        self.device_owned[usize::from(buffer)] = true;
        self.rx_write = (self.rx_write + 1) % rx_entries(family);
        Ok(())
    }

    /// `iwl_pcie_tx_start` with queue 0 enabled for commands, as
    /// `iwl_trans_ac_txq_enable` does it, after the image's ALIVE.
    pub fn start_scheduler(&mut self, alive: &Alive) -> Result<(), Error> {
        if self.state != State::Alive {
            return Err(Error::State);
        }
        self.state = State::Poisoned;
        let base = self.prph_read(SCD_SRAM_BASE)?;
        // `iwl_trans_pcie_fw_alive` warns on a scheduler that moved.
        if base != alive.scheduler_base {
            return Err(Error::Rejected {
                register: SCD_SRAM_BASE,
            });
        }
        self.write_memory(base + SCD_CONTEXT, &[0; SCD_CONTEXT_WORDS])?;
        let byte_counts = self.byte_counts.region().address;
        self.prph_write(SCD_DRAM_BASE, (byte_counts >> 10) as u32)?;
        self.prph_write(SCD_ENABLE, 0)?;
        self.prph_write(SCD_QUEUE_0_STATUS, SCD_QUEUE_INACTIVE)?;
        self.prph_update(SCD_AGGREGATION, 1, 0)?;
        self.write_direct(HBUS_TARG_WRPTR, 0)?;
        self.prph_write(SCD_QUEUE_0_READ, 0)?;
        self.write_memory(base + SCD_CONTEXT, &[0])?;
        self.write_memory(base + SCD_CONTEXT + 4, &[SCD_QUEUE_0_WINDOW])?;
        self.prph_write(SCD_QUEUE_0_STATUS, SCD_QUEUE_COMMANDS)?;
        self.prph_write(SCD_ENABLE, 1)?;
        self.prph_write(SCD_TXFACT, SCD_FIFOS)?;
        for channel in 0..8 {
            self.write_direct(TX_CHANNEL + 0x20 * channel, TX_CHANNEL_ENABLE)?;
        }
        let chicken = self.read_direct(TX_CHICKEN)?;
        self.write_direct(TX_CHICKEN, chicken | TX_CHICKEN_AUTO_RETRY)?;
        self.state = State::Running;
        Ok(())
    }

    /// Send one host command on queue 0 and wait for its response, as a
    /// synchronous `iwl_pcie_enqueue_hcmd` wanting the response does: the
    /// response's payload. Notifications that arrive meanwhile stay queued.
    pub fn send(&mut self, id: Id, payload: &[u8]) -> Result<Vec<u8>, Error> {
        if self.state != State::Running {
            return Err(Error::State);
        }
        self.state = State::Poisoned;
        let response = self.send_inner(id, payload)?;
        self.state = State::Running;
        Ok(response)
    }
    fn send_inner(&mut self, id: Id, payload: &[u8]) -> Result<Vec<u8>, Error> {
        let index = self.command_write;
        let command = Command::encode(id, 0, index, payload)?;
        let bytes = command.bytes();
        if bytes.len() > COMMAND_BYTES {
            return Err(Error::Command(super::super::command::Error::PayloadSize));
        }
        // The first TB holds the header, as Linux's `first_tb_bufs` do.
        let address = self.command.region().address;
        let first = bytes.len().min(FIRST_TB);
        let mut regions = alloc::vec![(address, first)];
        if bytes.len() > first {
            regions.push((address + first as u64, bytes.len() - first));
        }
        let slot = usize::from(index) * TFD_BYTES;
        // SAFETY: one command is in flight at a time, and the last one's
        // response ended the device's reads of the buffer and of its TFD;
        // the device never writes either.
        unsafe {
            self.command.write_owned(0, bytes)?;
            self.rings[0].write_owned(slot, &tfd(&regions))?;
        }
        fence(Ordering::Release);
        self.command_write = (index + 1) % TFD_SLOTS;
        self.write(HBUS_TARG_WRPTR, u32::from(self.command_write))?;
        let sequence = command.sequence();
        for _ in 0..COMMAND_TIMEOUT_US / 10 {
            self.receive()?;
            if let Some(at) = self.received.iter().position(|p| !p.unsolicited()) {
                let response = self.received.remove(at).expect("found");
                if response.sequence != sequence || response.command != id.opcode {
                    return Err(Error::CommandSequence(response.sequence));
                }
                return Ok(response.payload);
            }
            self.io().delay_us(10)?;
        }
        Err(Error::Timeout {
            register: HBUS_TARG_WRPTR,
        })
    }

    /// The next notification the firmware sends, waiting up to
    /// `microseconds` for one.
    pub fn next_notification(&mut self, microseconds: u32) -> Result<Notification, Error> {
        if !matches!(self.state, State::Alive | State::Running) {
            return Err(Error::State);
        }
        let state = core::mem::replace(&mut self.state, State::Poisoned);
        for _ in 0..microseconds.div_ceil(10) {
            if let Some(packet) = self.received.pop_front() {
                self.state = state;
                return Ok(packet);
            }
            self.receive()?;
            self.io().delay_us(10)?;
        }
        Err(Error::Timeout { register: INT })
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
            self.byte_counts.release()?;
            for block in &mut self.paging {
                block.release()?;
            }
            self.command.release()?;
            if let Some(used) = &mut self.used {
                used.release()?;
            }
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

#[cfg(test)]
pub(in crate::wifi::intel) mod tests;
