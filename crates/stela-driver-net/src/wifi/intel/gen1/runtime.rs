// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright (C) 2012-2014, 2018-2026 Intel Corporation
// Copyright (C) 2013-2015 Intel Mobile Communications GmbH
// Copyright (C) 2016-2017 Intel Deutschland GmbH
//! The runtime image's start, as `iwl_mvm_load_rt_fw` and `iwl_mvm_up`
//! (mvm/fw.c) drive it with fw/{paging,phy-db,init}.c, mvm/sf.c and
//! mvm/coex.c at Linux a804c5742bff224d4cb4d25dd383554d598be32b. It goes
//! up to `DQA_ENABLE_CMD`; the queues and stations that follow are not
//! ported.
//!
//! Not sent: the firmware's own debug configuration (`LDBG_CONFIG_CMD`,
//! which Linux sends only with the ini debug set), and the BIOS's LARI
//! regulatory configuration, which needs its UEFI and ACPI tables.
use super::{
    Profile,
    boot::{Alive, Boot},
    init::{self, Init},
};
use crate::{
    bus::dma::{mapping::DmaDomain, memory::PageSource},
    wifi::intel::{
        boot::{BootIo, Error},
        command::Id,
        firmware::ImageKind,
        identity::Staged,
    },
};
use alloc::{sync::Arc, vec::Vec};

const fn id(group: u8, opcode: u8) -> Id {
    Id {
        group,
        opcode,
        version: 0,
    }
}
const PAGING: Id = id(0, 0x4f);
const SHARED_MEMORY: Id = id(2, 0x00);
const SHARED_MEMORY_LEGACY: Id = id(0, 0x8c);
const SMART_FIFO: Id = id(0, 0xd1);
const PHY_DB: Id = id(0, 0x6c);
const BT_CONFIG: Id = id(0, 0x9b);
const SOC_CONFIGURATION: Id = id(2, 0x01);
const DQA_ENABLE: Id = id(5, 0x00);

/// `IWL_UCODE_TLV_CAPA_*`.
const CAPA_DQA: u32 = 12;
const CAPA_SOC_LATENCY: u32 = 37;
const CAPA_BT_MPLUT: u32 = 67;
const CAPA_EXTENDED_SHARED_MEMORY: u32 = 80;

/// `IWL_PHY_DB_CFG`, `_CALIB_NCH`, `_CALIB_CHG_PAPD` and `_CALIB_CHG_TXP`.
const PHY_DB_CONFIGURATION: u16 = 1;
const PHY_DB_CHANNELS: u16 = 2;
const PHY_DB_PAPD: u16 = 4;
const PHY_DB_TX_POWER: u16 = 5;

/// `BT_COEX_NW`, and `BT_COEX_MPLUT_ENABLED`, `_SYNC2SCO_ENABLED` and
/// `_HIGH_BAND_RET`.
const BT_COEX_NW: u32 = 1;
const BT_MPLUT: u32 = 1;
const BT_SYNC2SCO: u32 = 1 << 2;
const BT_HIGH_BAND_RETENTION: u32 = 1 << 4;
/// `SOC_CONFIG_CMD_FLAGS_DISCRETE`.
const SOC_DISCRETE: u32 = 1;
/// The command queue `iwl_mvm_send_dqa_cmd` names, `IWL_MVM_DQA_CMD_QUEUE`.
const COMMAND_QUEUE: u32 = 0;
/// `MVM_UCODE_ALIVE_TIMEOUT`.
const ALIVE_TIMEOUT_MS: u32 = 2000;

/// What the runtime image gave on its way up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Runtime {
    pub alive: Alive,
    /// `iwl_shared_mem_cfg`, raw: the FIFO sizes.
    pub shared_memory: Vec<u8>,
}

/// Boot the runtime image and bring it up to DQA, from what INIT gave. The
/// function stays running; a failure stops it.
///
/// # Safety
/// [`Boot::start`]'s.
pub unsafe fn start<'a, Q: BootIo>(
    io: Q,
    profile: Profile,
    staged: Staged<'a>,
    init: &Init,
    source: &dyn PageSource,
    domain: Arc<dyn DmaDomain>,
) -> Result<(Boot<'a, Q>, Runtime), Error> {
    let integrated = profile.platform().integrated;
    let latency = profile.platform().xtal_latency;
    // SAFETY: forwarded from the caller.
    let mut boot = unsafe { Boot::start(io, profile, staged, ImageKind::Runtime, source, domain)? };
    let result = boot
        .wait_alive(ALIVE_TIMEOUT_MS)
        .and_then(|alive| configure(&mut boot, alive, init, integrated, latency));
    match result {
        Ok(runtime) => Ok((boot, runtime)),
        Err(error) => {
            boot.stop()?;
            Err(error)
        }
    }
}

/// Everything `iwl_mvm_up` sends after the runtime image's ALIVE, up to
/// `DQA_ENABLE_CMD`.
pub fn configure<Q: BootIo>(
    boot: &mut Boot<'_, Q>,
    alive: Alive,
    init: &Init,
    integrated: bool,
    xtal_latency: u32,
) -> Result<Runtime, Error> {
    let firmware = boot.firmware();
    let capability = |bit| firmware.supports_capability(bit);
    let (dqa, soc, mplut, extended) = (
        capability(CAPA_DQA),
        capability(CAPA_SOC_LATENCY),
        capability(CAPA_BT_MPLUT),
        capability(CAPA_EXTENDED_SHARED_MEMORY),
    );
    let (configuration, calibration) = init::phy_configuration(firmware, init::UCODE_REGULAR)?;
    boot.start_scheduler(&alive)?;
    if let Some(paging) = boot.paging_command() {
        boot.send(PAGING, &paging)?;
    }
    let shared_memory = boot.send(
        if extended {
            SHARED_MEMORY
        } else {
            SHARED_MEMORY_LEGACY
        },
        &[],
    )?;
    boot.send(SMART_FIFO, &smart_fifo())?;
    let antennas = (configuration & init::TX_CHAIN) >> 16;
    boot.send(init::TX_ANTENNAS, &antennas.to_le_bytes())?;
    for section in phy_db_order(&init.phy_db)? {
        boot.send(PHY_DB, &section)?;
    }
    let mut payload = configuration.to_le_bytes().to_vec();
    payload.extend(calibration);
    boot.send(init::PHY_CONFIGURATION, &payload)?;
    let modules = BT_SYNC2SCO | BT_HIGH_BAND_RETENTION | if mplut { BT_MPLUT } else { 0 };
    let mut payload = BT_COEX_NW.to_le_bytes().to_vec();
    payload.extend(modules.to_le_bytes());
    boot.send(BT_CONFIG, &payload)?;
    if soc {
        // ponytail: no LTR delay or low-latency crystal, which neither the
        // 8265 nor the 9560 configures.
        let mut payload = (if integrated { 0 } else { SOC_DISCRETE })
            .to_le_bytes()
            .to_vec();
        payload.extend(xtal_latency.to_le_bytes());
        boot.send(SOC_CONFIGURATION, &payload)?;
    }
    if dqa {
        boot.send(DQA_ENABLE, &COMMAND_QUEUE.to_le_bytes())?;
    }
    Ok(Runtime {
        alive,
        shared_memory,
    })
}

/// `iwl_mvm_sf_config` in `SF_INIT_OFF`, with no station:
/// `iwl_sf_cfg_cmd`'s state, the scan and MIMO2 watermarks, one-second
/// long-delay timeouts and the default full-on aging and idle timers.
fn smart_fifo() -> Vec<u8> {
    const SF_INIT_OFF: u32 = 3;
    let mut words = alloc::vec![SF_INIT_OFF, 4096, 8192];
    words.extend([1_000_000; 10]);
    for _ in 0..5 {
        words.extend([400, 160]);
    }
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// `iwl_send_phy_db_data`: the configuration and the channel section, then
/// each PAPD and TX-power channel group by its number, which is the data's
/// first word. A later section replaces an earlier one, as
/// `iwl_phy_db_set_section` stores them. Each payload is
/// `iwl_phy_db_cmd`: type, length, data.
fn phy_db_order(sections: &[(u16, Vec<u8>)]) -> Result<Vec<Vec<u8>>, Error> {
    let latest = |kind: u16, group: Option<u16>| {
        sections.iter().rev().find(|(k, data)| {
            *k == kind && group.is_none_or(|g| data.get(..2) == Some(&g.to_le_bytes()[..]))
        })
    };
    let missing = Error::InitializationResponse {
        command: 0x6b,
        group: 0,
    };
    let mut order = Vec::new();
    for kind in [PHY_DB_CONFIGURATION, PHY_DB_CHANNELS] {
        order.push(latest(kind, None).ok_or(missing.clone())?);
    }
    for kind in [PHY_DB_PAPD, PHY_DB_TX_POWER] {
        let mut groups: Vec<u16> = sections
            .iter()
            .filter(|(k, data)| *k == kind && data.len() >= 2)
            .map(|(_, data)| u16::from_le_bytes([data[0], data[1]]))
            .collect();
        groups.sort_unstable();
        groups.dedup();
        for group in groups {
            order.extend(latest(kind, Some(group)));
        }
    }
    Ok(order
        .into_iter()
        .map(|(kind, data)| {
            let mut payload = kind.to_le_bytes().to_vec();
            payload.extend((data.len() as u16).to_le_bytes());
            payload.extend(data);
            payload
        })
        .collect())
}

#[cfg(test)]
mod tests;
