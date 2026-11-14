// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright (C) 2005-2014, 2018-2026 Intel Corporation
// Copyright (C) 2013-2015 Intel Mobile Communications GmbH
// Copyright (C) 2015-2017 Intel Deutschland GmbH
//! Intel Wireless 8265/8275 and Wireless-AC 9560 identity and firmware
//! selection, from pcie/drv.c, cfg/{8000,9000,rf-jf}.c,
//! `iwl_drv_get_fwname_pre` and `iwl_pci_gen1_2_probe` at Linux
//! a804c5742bff224d4cb4d25dd383554d598be32b.
//!
//! These families boot through the gen1 transport: separate INIT and RT
//! images whose sections the driver copies into SRAM itself, and a TX
//! scheduler the driver programs. The native driver has none of that yet,
//! so admission stops once the function is identified and its image staged.
use super::{
    firmware::Error as FirmwareError,
    identity::{self, IdentityError, Observed, Platform, StageError, Staged},
};
use crate::firmware as resources;

pub mod boot;

const INTEL: u16 = 0x8086;
const WIRELESS_8265: u16 = 0x24fd;
const WIRELESS_9560: u16 = 0x9df0;

/// The 0x24FD subdevices `iwl_hw_card_ids` lists. It takes no other.
const SUBDEVICES_8265: [u16; 29] = [
    0x0010, 0x0110, 0x1110, 0x1130, 0x0130, 0x1010, 0x10d0, 0x0050, 0x0150, 0x9010, 0x8110, 0x8050,
    0x8010, 0x0810, 0x9110, 0x8130, 0x0910, 0x0930, 0x0950, 0x0850, 0x1014, 0x3e02, 0x3e01, 0x1012,
    0x0012, 0x0014, 0x9074, 0x1431, 0x1432,
];
/// `IWL_CFG_MAC_TYPE_PU`, the 9560's MAC in the PCH.
const MAC_PU: u32 = 0x31;
/// `IWL_CFG_RF_TYPE_JF2` and `IWL_CFG_RF_TYPE_JF1`.
const RF_JF2: u32 = 0x105;
const RF_JF1: u32 = 0x108;

/// cfg/8000.c's `IWL8265_FW_PRE` at `IWL8265_UCODE_API_MAX`.
const IMAGE_8265: &str = "iwlwifi-8265-36.ucode";
/// `iwl_drv_get_fwname_pre` for a PU MAC and a JF radio, both of which it
/// names at step B, at `IWL9000_UCODE_API_MAX`.
const IMAGE_9560: &str = "iwlwifi-9000-pu-b0-jf-b0-46.ucode";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// `IWL_DEVICE_FAMILY_8000`.
    F8000,
    /// `IWL_DEVICE_FAMILY_9000`.
    F9000,
}

impl Family {
    /// The image API `iwl_get_ucode_api_versions` settles on: 8265's own
    /// maximum, and for the 9560 the 9000 MAC's, since the JF radio's range
    /// (77) starts above it.
    pub const fn firmware_api(self) -> u32 {
        match self {
            Self::F8000 => 36,
            Self::F9000 => 46,
        }
    }
}

/// `iwl_hw_card_ids` for the admitted gen1 devices, with what their
/// `iwl_mac_cfg` says of the platform: the 8265 is a discrete card, and
/// `iwl9560_mac_cfg` an integrated MAC with a 650 us crystal.
pub const fn device(id: u16) -> Option<(Family, Platform)> {
    Some(match id {
        WIRELESS_8265 => (Family::F8000, Platform::discrete(0)),
        WIRELESS_9560 => (Family::F9000, Platform::integrated(650, 0)),
        _ => return None,
    })
}

/// Every image [`Profile::select`] can name for the device `id`: what
/// `stela setup` writes under the device binding
/// (`stela_loader::host_firmware::recipes::require_wifi` mirrors this).
pub const fn device_images(id: u16) -> &'static [&'static str] {
    match id {
        WIRELESS_8265 => &[IMAGE_8265],
        WIRELESS_9560 => &[IMAGE_9560],
        _ => &[],
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    observed: Observed,
    family: Family,
    platform: Platform,
    firmware: &'static str,
}

impl Profile {
    /// `iwl_pci_gen1_2_probe`'s match. The 8265's image is fixed by its
    /// configuration; the 9560's is built from its MAC and radio, which must
    /// be the PU MAC and a JF radio. A blank OTP reads a zero RF type, which
    /// only `map_crf_id`'s 9000-family reading names, and that is not ported.
    pub fn select(observed: Observed) -> Result<Self, IdentityError> {
        if observed.vendor != INTEL {
            return Err(IdentityError::UnsupportedPci);
        }
        let (family, platform) = device(observed.device).ok_or(IdentityError::UnsupportedPci)?;
        if observed.hardware_revision == u32::MAX || observed.radio_id == u32::MAX {
            return Err(IdentityError::FailedRead);
        }
        let firmware = match family {
            Family::F8000 => {
                if !SUBDEVICES_8265.contains(&observed.subsystem_device) {
                    return Err(IdentityError::UnsupportedPci);
                }
                IMAGE_8265
            }
            Family::F9000 => {
                let mac = (observed.hardware_revision & 0xfff0) >> 4;
                if mac != MAC_PU {
                    return Err(IdentityError::UnsupportedMac(mac));
                }
                if !matches!((observed.radio_id & 0x00ff_f000) >> 12, RF_JF1 | RF_JF2) {
                    return Err(IdentityError::UnsupportedRadio(observed.radio_id));
                }
                IMAGE_9560
            }
        };
        Ok(Self {
            observed,
            family,
            platform,
            firmware,
        })
    }

    pub fn observed(&self) -> Observed {
        self.observed
    }
    pub fn family(&self) -> Family {
        self.family
    }
    pub fn platform(&self) -> Platform {
        self.platform
    }
    pub fn firmware_name(&self) -> &'static str {
        self.firmware
    }

    /// The image under the adapter's bindings ([`Observed::load`]), refused
    /// unless it has the family's API.
    pub fn stage<'a>(
        &self,
        provider: &'a dyn resources::Provider,
    ) -> Result<Staged<'a>, StageError> {
        let expected = self.family.firmware_api();
        identity::stage_image(&self.observed, self.firmware, provider, |fw| {
            match fw.api_version() {
                actual if actual == expected => Ok(()),
                actual => Err(FirmwareError::FirmwareApi { expected, actual }),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firmware::{Catalog, Provenance, Resource};
    use crate::wifi::intel::{firmware::ImageKind, power::Silicon};
    use alloc::{vec, vec::Vec};
    use sha2::{Digest, Sha256};

    fn observed(device: u16, subsystem_device: u16, mac: u32, radio: u32) -> Observed {
        Observed {
            vendor: INTEL,
            device,
            subsystem_vendor: INTEL,
            subsystem_device,
            pci_revision: 0x10,
            link_speed: 1,
            hardware_revision: mac << 4 | 2,
            radio_id: radio << 12 | 0x100,
            silicon: Silicon::default(),
        }
    }
    fn latitude_7490() -> Observed {
        // The 8265 reads the 8000 family's revision and no RF ID; neither
        // takes part in its selection.
        observed(WIRELESS_8265, 0x0010, 0x20, 0)
    }
    fn latitude_7400() -> Observed {
        observed(WIRELESS_9560, 0x0034, MAC_PU, RF_JF2)
    }

    #[test]
    fn each_device_names_its_one_image_and_platform() {
        let p = Profile::select(latitude_7490()).unwrap();
        assert_eq!(
            (p.family(), p.firmware_name(), p.platform().integrated),
            (Family::F8000, "iwlwifi-8265-36.ucode", false)
        );
        let p = Profile::select(latitude_7400()).unwrap();
        assert_eq!(
            (p.family(), p.firmware_name(), p.platform().integrated),
            (Family::F9000, "iwlwifi-9000-pu-b0-jf-b0-46.ucode", true)
        );
        assert_eq!(p.platform().xtal_latency, 650);
        // JF1, the 9461 and 9462, runs the same image.
        let jf1 = Profile::select(observed(WIRELESS_9560, 0x0034, MAC_PU, RF_JF1)).unwrap();
        assert_eq!(jf1.firmware_name(), IMAGE_9560);
        for subdevice in SUBDEVICES_8265 {
            let o = Observed {
                subsystem_device: subdevice,
                ..latitude_7490()
            };
            assert_eq!(Profile::select(o).unwrap().firmware_name(), IMAGE_8265);
        }
    }

    #[test]
    fn what_linux_would_not_match_is_refused_by_what_differs() {
        for (o, error) in [
            (
                Observed {
                    subsystem_device: 0x0020,
                    ..latitude_7490()
                },
                IdentityError::UnsupportedPci,
            ),
            (
                Observed {
                    vendor: 0x1234,
                    ..latitude_7400()
                },
                IdentityError::UnsupportedPci,
            ),
            (
                Observed {
                    device: 0x2526,
                    ..latitude_7400()
                },
                IdentityError::UnsupportedPci,
            ),
            (
                Observed {
                    hardware_revision: u32::MAX,
                    ..latitude_7400()
                },
                IdentityError::FailedRead,
            ),
            (
                Observed {
                    radio_id: u32::MAX,
                    ..latitude_7490()
                },
                IdentityError::FailedRead,
            ),
            // A 9260's TH MAC.
            (
                observed(WIRELESS_9560, 0x0034, 0x32, RF_JF2),
                IdentityError::UnsupportedMac(0x32),
            ),
            // An HR radio, and a blank OTP.
            (
                observed(WIRELESS_9560, 0x0034, MAC_PU, 0x10a),
                IdentityError::UnsupportedRadio(0x10a100),
            ),
            (
                observed(WIRELESS_9560, 0x0034, MAC_PU, 0),
                IdentityError::UnsupportedRadio(0x100),
            ),
        ] {
            assert_eq!(Profile::select(o), Err(error), "{o:?}");
        }
    }

    /// No device is both a gen1 and a gen2 one, and every listed device
    /// names exactly the files of [`device_images`].
    #[test]
    fn gen1_devices_are_their_own_and_their_images_are_listed() {
        let mut listed = 0;
        for id in 0..=u16::MAX {
            if device(id).is_none() {
                assert!(device_images(id).is_empty());
                continue;
            }
            listed += 1;
            assert!(identity::device(id).is_none(), "{id:04x}");
            let o = match id {
                WIRELESS_8265 => latitude_7490(),
                _ => Observed {
                    device: id,
                    ..latitude_7400()
                },
            };
            let name = Profile::select(o).unwrap().firmware_name();
            assert_eq!(device_images(id), &[name]);
        }
        assert_eq!(listed, 2);
    }

    fn image(api: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; 88];
        bytes[4..8].copy_from_slice(&0x0a4c_5749u32.to_le_bytes());
        bytes[72..76].copy_from_slice(&api.to_le_bytes());
        bytes.extend(29u32.to_le_bytes());
        bytes.extend(8u32.to_le_bytes());
        bytes.extend(0u32.to_le_bytes());
        bytes.extend((1u32 << 20).to_le_bytes());
        bytes
    }
    /// A catalog holding `bytes` as `name` under the device binding, as
    /// `stela setup` writes it for an adapter it knows by PCI IDs.
    fn device_catalog(device: u16, name: &'static str, bytes: Vec<u8>) -> Catalog {
        let hardware = alloc::format!("pci:8086:{device:04x}");
        let target = resources::Target {
            driver: "wifi-intel",
            hardware: &hardware,
            revision: identity::DEVICE_REVISION,
            board: None,
        };
        Catalog::from_handoff(vec![Resource {
            name,
            target,
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

    #[test]
    fn staging_takes_the_device_binding_and_refuses_another_api() {
        for (o, api) in [(latitude_7490(), 36), (latitude_7400(), 46)] {
            let profile = Profile::select(o).unwrap();
            let name = profile.firmware_name();
            let catalog = device_catalog(o.device, name, image(api));
            let staged = profile.stage(&catalog).unwrap();
            assert_eq!(staged.firmware().api_version(), api);
            assert!(o.binds(name, &staged.source()));
            let catalog = device_catalog(o.device, name, image(api - 1));
            assert!(matches!(
                profile.stage(&catalog),
                Err(StageError::Firmware {
                    error: FirmwareError::FirmwareApi { expected, actual },
                    ..
                }) if expected == api && actual == api - 1
            ));
            let empty = Catalog::from_handoff(Vec::new()).unwrap();
            assert!(matches!(
                profile.stage(&empty),
                Err(StageError::Resource { error, .. })
                    if error.kind == resources::ErrorKind::Missing
            ));
        }
    }

    /// The pinned linux-firmware images parse, carry their family's API, and
    /// hold both the INIT and the RT image the gen1 boot loads in turn.
    #[test]
    #[ignore = "oracle: STELA_IWLWIFI_IMAGES"]
    fn real_8265_and_9560_images_hold_init_and_runtime_sections() {
        extern crate std;
        let root = std::env::var("STELA_IWLWIFI_IMAGES").expect("actual pinned image directory");
        for (name, family) in [(IMAGE_8265, Family::F8000), (IMAGE_9560, Family::F9000)] {
            let bytes = std::fs::read(std::path::Path::new(&root).join(name)).unwrap();
            let firmware = super::super::firmware::Firmware::parse(&bytes).unwrap();
            assert_eq!(firmware.api_version(), family.firmware_api(), "{name}");
            for kind in [ImageKind::Init, ImageKind::Runtime] {
                assert!(
                    firmware.sections().iter().any(|s| s.image == kind),
                    "{name}: no {kind:?} section"
                );
            }
        }
    }
}
