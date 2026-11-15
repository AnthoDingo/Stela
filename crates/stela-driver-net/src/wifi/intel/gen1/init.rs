// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright (C) 2012-2014, 2018-2026 Intel Corporation
// Copyright (C) 2013-2015 Intel Mobile Communications GmbH
// Copyright (C) 2016-2017 Intel Deutschland GmbH
//! The INIT image's run after ALIVE, as `iwl_run_init_mvm_ucode` drives it
//! in mvm/fw.c with `iwl_nvm_init` (mvm/nvm.c), `iwl_set_hw_address`
//! (iwl-nvm-parse.c) and the PHY DB of fw/phy-db.c, at Linux
//! a804c5742bff224d4cb4d25dd383554d598be32b: the scheduler, the NVM
//! sections, the hardware address, the antennas and PHY configuration, then
//! the calibration results the firmware reports up to INIT_COMPLETE.
//!
//! What is kept is raw: the NVM is not parsed into channels or SKU, and the
//! PHY DB sections are what the RT image is to be sent back.
use super::{
    Family, Profile,
    boot::{Alive, Boot},
};
use crate::{
    bus::dma::{mapping::DmaDomain, memory::PageSource},
    bytes::le16,
    wifi::intel::{
        boot::{BootIo, Error},
        command::Id,
        firmware::{Firmware, ImageKind},
        identity::Staged,
        nvm::HardwareAddress,
    },
};
use alloc::{sync::Arc, vec::Vec};

const NVM_ACCESS: Id = Id {
    group: 0,
    opcode: 0x88,
    version: 0,
};
pub(super) const TX_ANTENNAS: Id = Id {
    group: 0,
    opcode: 0x98,
    version: 0,
};
pub(super) const PHY_CONFIGURATION: Id = Id {
    group: 0,
    opcode: 0x6a,
    version: 0,
};
/// `CALIB_RES_NOTIF_PHY_DB` and `INIT_COMPLETE_NOTIF`.
const PHY_DB: u8 = 0x6b;
const INIT_COMPLETE: u8 = 0x04;
/// `NVM_MAX_NUM_SECTIONS` and `IWL_NVM_DEFAULT_CHUNK_SIZE`.
const NVM_SECTIONS: u16 = 13;
const NVM_CHUNK: u16 = 2048;
/// `READ_NVM_CHUNK_NOT_VALID_ADDRESS`.
const NVM_NOT_VALID_ADDRESS: u16 = 1;
/// `NVM_SECTION_TYPE_MAC_OVERRIDE`; the address starts at its second word
/// (`MAC_ADDRESS_OVERRIDE_EXT_NVM`).
const MAC_OVERRIDE: u16 = 11;
const RESERVED_MAC: [u8; 6] = [0x02, 0xcc, 0xaa, 0xff, 0xee, 0x00];
/// `NVM_SECTION_TYPE_HW_8000`, without which Linux finds no OTP address.
const HARDWARE: u16 = 10;
/// The 9000 family's `mac_addr_from_csr`: the OTP words, then the strap's.
const MAC_OTP: u32 = 0x380;
const MAC_STRAP: u32 = 0x388;
/// `WFMP_MAC_ADDR_0`, the 8000 family's OTP address.
const WFMP_MAC: u32 = 0xa03080;
/// `IWL_UCODE_TLV_DEF_CALIB`, and `IWL_UCODE_REGULAR` and `IWL_UCODE_INIT`
/// among its image types.
const DEFAULT_CALIBRATION: u32 = 22;
pub(super) const UCODE_REGULAR: u32 = 0;
const UCODE_INIT: u32 = 1;
/// `FW_PHY_CFG_TX_CHAIN`.
pub(super) const TX_CHAIN: u32 = 0xf << 16;
/// `MVM_UCODE_CALIB_TIMEOUT`, for each notification.
const CALIBRATION_TIMEOUT_US: u32 = 2_000_000;
/// `MVM_UCODE_ALIVE_TIMEOUT`.
const ALIVE_TIMEOUT_MS: u32 = 2000;

/// What the INIT image gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Init {
    /// Every NVM section the firmware had, by type.
    pub nvm: Vec<(u16, Vec<u8>)>,
    pub hardware_address: [u8; 6],
    /// Each `CALIB_RES_NOTIF_PHY_DB`'s type and data, in order.
    pub phy_db: Vec<(u16, Vec<u8>)>,
}

/// Boot the INIT image, run it to INIT_COMPLETE and stop the function again,
/// handing its owner back.
///
/// # Safety
/// [`Boot::start`]'s.
pub unsafe fn run<Q: BootIo>(
    io: Q,
    profile: Profile,
    staged: Staged<'_>,
    source: &dyn PageSource,
    domain: Arc<dyn DmaDomain>,
) -> Result<(Alive, Init, Q), Error> {
    let family = profile.family();
    // SAFETY: forwarded from the caller.
    let mut boot = unsafe { Boot::start(io, profile, staged, ImageKind::Init, source, domain)? };
    let result = boot
        .wait_alive(ALIVE_TIMEOUT_MS)
        .and_then(|alive| Ok((alive, calibrate(&mut boot, family, &alive)?)));
    boot.stop()?;
    let (alive, init) = result?;
    Ok((alive, init, boot.into_owner()?))
}

/// Everything after the INIT image's ALIVE, up to INIT_COMPLETE.
pub fn calibrate<Q: BootIo>(
    boot: &mut Boot<'_, Q>,
    family: Family,
    alive: &Alive,
) -> Result<Init, Error> {
    boot.start_scheduler(alive)?;
    let mut nvm = Vec::new();
    for section in 0..NVM_SECTIONS {
        if let Some(data) = read_section(boot, section)? {
            nvm.push((section, data));
        }
    }
    let hardware_address = hardware_address(boot, family, &nvm)?;
    let (configuration, calibration) = phy_configuration(boot.firmware(), UCODE_INIT)?;
    // ponytail: the firmware's antennas alone; Linux also masks them with
    // the NVM's, which needs its SKU parsed.
    let antennas = (configuration & TX_CHAIN) >> 16;
    boot.send(TX_ANTENNAS, &antennas.to_le_bytes())?;
    let mut payload = configuration.to_le_bytes().to_vec();
    payload.extend(calibration);
    boot.send(PHY_CONFIGURATION, &payload)?;
    let mut phy_db = Vec::new();
    loop {
        let notification = boot.next_notification(CALIBRATION_TIMEOUT_US)?;
        if notification.group != 0 {
            continue;
        }
        match notification.command {
            // `iwl_calib_res_notif_phy_db`: type, length and the data.
            PHY_DB => {
                let p = &notification.payload;
                let length = if p.len() >= 4 {
                    usize::from(le16(p, 2))
                } else {
                    usize::MAX
                };
                let data = p.get(4..4 + length).ok_or(Error::InitializationResponse {
                    command: PHY_DB,
                    group: 0,
                })?;
                phy_db.push((le16(p, 0), data.to_vec()));
            }
            INIT_COMPLETE => break,
            _ => {}
        }
    }
    Ok(Init {
        nvm,
        hardware_address,
        phy_db,
    })
}

/// `iwl_nvm_read_section`: 2 KiB chunks until a short one. A section the
/// firmware refuses is absent, as `iwl_nvm_init` skips it.
fn read_section<Q: BootIo>(boot: &mut Boot<'_, Q>, section: u16) -> Result<Option<Vec<u8>>, Error> {
    let malformed = Error::InitializationResponse {
        command: NVM_ACCESS.opcode,
        group: 1,
    };
    let mut data = Vec::new();
    loop {
        let offset = u16::try_from(data.len()).map_err(|_| malformed.clone())?;
        // `iwl_nvm_access_cmd`: read, target 0, then type, offset, length.
        let mut request = [0; 8];
        request[2..4].copy_from_slice(&section.to_le_bytes());
        request[4..6].copy_from_slice(&offset.to_le_bytes());
        request[6..8].copy_from_slice(&NVM_CHUNK.to_le_bytes());
        let response = boot.send(NVM_ACCESS, &request)?;
        if response.len() < 8 {
            return Err(malformed);
        }
        // `iwl_nvm_access_resp`: offset, length, type, status, data.
        let (read_offset, length, status) =
            (le16(&response, 0), le16(&response, 2), le16(&response, 6));
        if status != 0 {
            return Ok((offset != 0 && status == NVM_NOT_VALID_ADDRESS).then_some(data));
        }
        let chunk = response
            .get(8..8 + usize::from(length))
            .filter(|_| read_offset == offset)
            .ok_or(malformed.clone())?;
        data.extend_from_slice(chunk);
        if length != NVM_CHUNK {
            return Ok(Some(data));
        }
    }
}

/// `iwl_set_hw_address`: the 9000 family reads its CSRs, strap first; the
/// 8000 family takes the NVM's override, unless reserved or invalid, then
/// its OTP.
fn hardware_address<Q: BootIo>(
    boot: &mut Boot<'_, Q>,
    family: Family,
    nvm: &[(u16, Vec<u8>)],
) -> Result<[u8; 6], Error> {
    let address = match family {
        Family::F9000 => {
            let strap = [boot.read(MAC_STRAP)?, boot.read(MAC_STRAP + 4)?];
            let otp = [boot.read(MAC_OTP)?, boot.read(MAC_OTP + 4)?];
            HardwareAddress::select(strap, Some(otp))?.bytes
        }
        Family::F8000 => {
            let mut address = nvm
                .iter()
                .find(|(section, _)| *section == MAC_OVERRIDE)
                .and_then(|(_, data)| data.get(2..8))
                .map(|bytes| <[u8; 6]>::try_from(bytes).expect("six bytes"))
                .filter(|a| HardwareAddress::valid(*a) && *a != RESERVED_MAC);
            if address.is_none() && nvm.iter().any(|(section, _)| *section == HARDWARE) {
                let words = [boot.prph_read(WFMP_MAC)?, boot.prph_read(WFMP_MAC + 4)?];
                address = Some(HardwareAddress::decode(words[0], words[1]));
            }
            address.ok_or(Error::Nvm(crate::wifi::intel::nvm::Error::HardwareAddress))?
        }
    };
    if !HardwareAddress::valid(address) {
        return Err(Error::Nvm(crate::wifi::intel::nvm::Error::HardwareAddress));
    }
    Ok(address)
}

/// `iwl_mvm_get_phy_config` and the `default_calib` of the image type
/// `ucode`: the PHY SKU, and the flow and event triggers in
/// `iwl_calib_ctrl`'s order, which is the TLV's.
pub(super) fn phy_configuration(
    firmware: &Firmware<'_>,
    ucode: u32,
) -> Result<(u32, [u8; 8]), Error> {
    let configuration = firmware.phy_sku().ok_or(Error::PhyConfiguration)?;
    let calibration = firmware
        .tlvs()
        .iter()
        .filter(|tlv| tlv.kind == DEFAULT_CALIBRATION && tlv.data.len() == 12)
        .find(|tlv| tlv.data[..4] == ucode.to_le_bytes())
        .ok_or(Error::PhyConfiguration)?;
    Ok((
        configuration,
        calibration.data[4..12].try_into().expect("eight bytes"),
    ))
}

#[cfg(test)]
pub(in crate::wifi::intel) mod tests;
