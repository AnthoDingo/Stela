//! Host GPU discovery. An entry is a PCI inventory item, not an initialized
//! accelerator or a Metal device. The native vendor ports live under this
//! module.
#![no_std]
extern crate alloc;

use stela_driver_base::bus;

use crate::bus::pci::{Address, ConfigSpace, Function};

#[cfg(feature = "metal")]
pub mod metal;

#[cfg(feature = "nouveau-port")]
pub mod nouveau;

#[cfg(feature = "i915-port")]
pub mod i915;

pub mod cold;
pub mod intel_quiesce;
pub mod opregion;
pub mod nouveau_chip;

#[cfg(feature = "linux-kernel")]
pub mod drm;

/// The native ports' hosted C test programs, which `build.rs` compiles.
#[cfg(all(test, feature = "linux-kernel"))]
mod c_tests {
    extern crate std;

    // Clippy's build compiles no C, so it lists no programs.
    #[cfg_attr(clippy, allow(dead_code))]
    fn run(program: &str) {
        let output = std::process::Command::new(program)
            .output()
            .unwrap_or_else(|error| panic!("cannot run {program}: {error}"));
        assert!(
            output.status.success(),
            "{program} failed ({}):\n{}{}",
            output.status,
            std::string::String::from_utf8_lossy(&output.stdout),
            std::string::String::from_utf8_lossy(&output.stderr)
        );
    }

    include!(concat!(env!("OUT_DIR"), "/c_tests.rs"));
}

/// PCI base class for VGA, 3D and other display controllers.
const DISPLAY_CLASS: u8 = 0x03;
const NVIDIA_VENDOR: u16 = 0x10de;
const AMD_VENDOR: u16 = 0x1002;
const INTEL_VENDOR: u16 = 0x8086;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    Other(u16),
}

impl Vendor {
    pub fn from_pci(id: u16) -> Self {
        match id {
            NVIDIA_VENDOR => Self::Nvidia,
            AMD_VENDOR => Self::Amd,
            INTEL_VENDOR => Self::Intel,
            other => Self::Other(other),
        }
    }
}

/// An unbound host display controller. Vendor identity does not establish
/// architecture support, discrete/integrated placement, or Metal features.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub address: Address,
    pub vendor: Vendor,
    pub device: u16,
    pub class: [u8; 3],
}

impl Adapter {
    /// Inventory only: does not size BARs, enable bus mastering, reset the
    /// device, change the firmware's scanout or allocate GPU memory.
    pub fn discover<C: ConfigSpace>(function: &Function<C>) -> Option<Self> {
        (function.class[0] == DISPLAY_CLASS).then_some(Self {
            address: function.address,
            vendor: Vendor::from_pci(function.vendor),
            device: function.device,
            class: function.class,
        })
    }

    /// Every discovered adapter remains unbound until a real port exists.
    pub fn unavailable_reason(&self) -> &'static str {
        match self.vendor {
            Vendor::Nvidia => "NVIDIA requires nouveau/GSP binding and an NVK-enabled image",
            Vendor::Amd => "AMDGPU is not ported to Stela",
            Vendor::Intel => "Intel i915/xe are not ported to Stela",
            Vendor::Other(_) => "no Stela GPU backend for this PCI vendor",
        }
    }
}

/// A bridge i915 looks up, which the Intel image shows to Linux, identity
/// only, with the host bridge's configuration. Inventory only, as [`Adapter`].
///
/// - The ISA (LPC/eSPI) bridge names the PCH, which i915 identifies it by
///   (`display/intel_pch.c`).
/// - The host bridge at 00:00.0 holds MCHBAR, which i915 reads from its
///   configuration (`i915_gmch.c`, `i915_gmch_bridge_setup`): read once here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuBridge {
    pub address: Address,
    pub device: u16,
    pub subsystem_vendor: u16,
    pub subsystem_device: u16,
    pub class: [u8; 3],
    pub revision: u8,
    /// The host bridge's first 256 bytes of configuration, read as words.
    pub config: Option<[u8; 256]>,
}

impl GpuBridge {
    /// `PCI_CLASS_BRIDGE_ISA`.
    const ISA: [u8; 2] = [0x06, 0x01];
    /// `PCI_CLASS_BRIDGE_HOST`.
    const HOST: [u8; 2] = [0x06, 0x00];

    /// Reads an Intel ISA bridge's revision and subsystem words, and the
    /// Intel host bridge at 00:00.0's first 256 bytes of configuration.
    pub fn discover<C: ConfigSpace>(
        function: &mut Function<C>,
    ) -> Result<Option<Self>, crate::bus::pci::ConfigError> {
        let host = function.class[..2] == Self::HOST
            && function.address.segment == 0
            && function.address.bus == 0
            && function.address.device == 0
            && function.address.function == 0;
        if function.vendor != INTEL_VENDOR || (function.class[..2] != Self::ISA && !host) {
            return Ok(None);
        }
        let subsystem = function.config().read32(0x2c)?;
        let config = if host {
            let mut bytes = [0u8; 256];
            for (offset, word) in bytes.chunks_exact_mut(4).enumerate() {
                let value = function.config().read32(offset as u16 * 4)?;
                word.copy_from_slice(&value.to_le_bytes());
            }
            Some(bytes)
        } else {
            None
        };
        Ok(Some(Self {
            address: function.address,
            device: function.device,
            subsystem_vendor: subsystem as u16,
            subsystem_device: (subsystem >> 16) as u16,
            class: function.class,
            revision: function.config().read8(0x08)?,
            config,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ReadOnlyConfig {
        vendor: u16,
        class: [u8; 3],
    }

    impl ConfigSpace for ReadOnlyConfig {
        fn read32(&self, offset: u16) -> Result<u32, crate::bus::pci::ConfigError> {
            Ok(match offset {
                0 => 0x1234_0000 | u32::from(self.vendor),
                8 => u32::from_be_bytes([self.class[0], self.class[1], self.class[2], 0x21]),
                0x2c => 0x081c_1028,
                _ => panic!("GPU inventory must not read device registers"),
            })
        }

        fn write32(&mut self, _: u16, _: u32) -> Result<(), crate::bus::pci::ConfigError> {
            panic!("GPU inventory must not change the controller");
        }

        fn write16(&mut self, _: u16, _: u16) -> Result<(), crate::bus::pci::ConfigError> {
            panic!("GPU inventory must not change the controller");
        }

        fn write8(&mut self, _: u16, _: u8) -> Result<(), crate::bus::pci::ConfigError> {
            panic!("GPU inventory must not change the controller");
        }

        fn read8(&self, offset: u16) -> Result<u8, crate::bus::pci::ConfigError> {
            Ok((self.read32(offset & !3)? >> (8 * (offset & 3))) as u8)
        }

        fn read16(&self, offset: u16) -> Result<u16, crate::bus::pci::ConfigError> {
            Ok((self.read32(offset & !3)? >> (8 * (offset & 2))) as u16)
        }
    }

    fn function(vendor: u16, class: [u8; 3]) -> Function<ReadOnlyConfig> {
        Function::new(
            Address {
                segment: 0,
                bus: 1,
                device: 0,
                function: 0,
            },
            ReadOnlyConfig { vendor, class },
        )
        .unwrap()
    }

    #[test]
    fn display_discovery_does_not_touch_hardware_or_claim_support() {
        for vendor in [NVIDIA_VENDOR, AMD_VENDOR, INTEL_VENDOR, 0x1234] {
            for subclass in [0, 2, 0x80] {
                let function = function(vendor, [DISPLAY_CLASS, subclass, 0]);
                let adapter = Adapter::discover(&function).unwrap();
                assert_eq!(adapter.address, function.address);
                assert_eq!(adapter.device, function.device);
                assert_eq!(adapter.class, function.class);
                assert!(!adapter.unavailable_reason().is_empty());
            }
            assert!(Adapter::discover(&function(vendor, [4, 3, 0])).is_none());
        }
    }

    #[test]
    fn an_intel_isa_bridge_is_identified_and_others_are_not() {
        let bridge = GpuBridge::discover(&mut function(INTEL_VENDOR, [0x06, 0x01, 0x00]));
        assert_eq!(
            bridge,
            Ok(Some(GpuBridge {
                address: Address {
                    segment: 0,
                    bus: 1,
                    device: 0,
                    function: 0,
                },
                device: 0x1234,
                subsystem_vendor: 0x1028,
                subsystem_device: 0x081c,
                class: [0x06, 0x01, 0x00],
                revision: 0x21,
                config: None,
            }))
        );
        assert_eq!(
            GpuBridge::discover(&mut function(0x1022, [0x06, 0x01, 0x00])),
            Ok(None)
        );
        assert_eq!(
            GpuBridge::discover(&mut function(INTEL_VENDOR, [0x06, 0x04, 0x00])),
            Ok(None)
        );
    }

    #[test]
    fn the_host_bridge_at_the_root_is_read_whole_and_one_elsewhere_is_not() {
        struct Words;

        impl ConfigSpace for Words {
            fn read32(&self, offset: u16) -> Result<u32, crate::bus::pci::ConfigError> {
                Ok(match offset {
                    0 => 0x5904_8086,
                    8 => u32::from_be_bytes([6, 0, 0, 0x08]),
                    0x2c => 0x081c_1028,
                    0x48 => 0xfed1_0001,
                    other => u32::from(other),
                })
            }

            fn write32(&mut self, _: u16, _: u32) -> Result<(), crate::bus::pci::ConfigError> {
                panic!("GPU inventory must not change the controller");
            }

            fn write16(&mut self, _: u16, _: u16) -> Result<(), crate::bus::pci::ConfigError> {
                panic!("GPU inventory must not change the controller");
            }

            fn write8(&mut self, _: u16, _: u8) -> Result<(), crate::bus::pci::ConfigError> {
                panic!("GPU inventory must not change the controller");
            }

            fn read8(&self, offset: u16) -> Result<u8, crate::bus::pci::ConfigError> {
                Ok((self.read32(offset & !3)? >> (8 * (offset & 3))) as u8)
            }

            fn read16(&self, offset: u16) -> Result<u16, crate::bus::pci::ConfigError> {
                Ok((self.read32(offset & !3)? >> (8 * (offset & 2))) as u16)
            }
        }

        let at = |bus| {
            Function::new(
                Address {
                    segment: 0,
                    bus,
                    device: 0,
                    function: 0,
                },
                Words,
            )
            .unwrap()
        };
        let bridge = GpuBridge::discover(&mut at(0)).unwrap().unwrap();
        let config = bridge.config.unwrap();
        assert_eq!(bridge.device, 0x5904);
        assert_eq!(config[0x48..0x4c], 0xfed1_0001u32.to_le_bytes());
        assert_eq!(config[0x80..0x84], 0x80u32.to_le_bytes());
        assert_eq!(GpuBridge::discover(&mut at(1)), Ok(None));
    }
}
