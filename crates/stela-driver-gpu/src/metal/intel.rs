//! Intel graphics under Stela's Metal device: i915 in the kernel and ANV as
//! the Vulkan driver, with brw generating shader code.
//!
//! As for NVIDIA ([`super::nvidia`]), hardware access, memory management,
//! command submission and code generation stay with i915, ANV and brw; this
//! module only admits a device and supplies the facts Vulkan does not report,
//! and [`open`] serves it. Only graphics that have been run with Stela are
//! admitted, one device ID at a time.
use super::{
    abi::DeviceLocation,
    capabilities::Platform,
    vulkan::{DriverId, PhysicalDeviceType, backend::ImageLayouts, physical::Identity},
};
use alloc::string::String;

pub mod open;

pub const VENDOR_ID: u32 = 0x8086;

/// The graphics run with Stela, by PCI device ID, with the name Metal
/// reports as `MTLArchitecture.name`: the Latitude 7490's UHD 620.
pub const DEVICES: [(u32, &str); 1] = [(0x5917, "Kaby Lake GT2")];

/// ISL's linear surfaces: a row pitch and base address that are multiples of
/// the element size for render targets and of 64 bytes where the display or
/// the samplers read them (`isl_calc_row_pitch_alignment`,
/// `isl_calc_base_alignment`). 64 bytes holds for every format.
const LINEAR_ALIGNMENT: u64 = 64;
/// ANV's sparse block, the 64 KiB of Vulkan's standard shapes.
const SPARSE_PAGE_BYTES: u64 = 64 << 10;

/// Why a Vulkan device is not admitted as an Intel Metal device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    NotIntel {
        vendor_id: u32,
    },
    /// Only ANV is integrated with Stela's i915.
    NotAnv(DriverId),
    /// i915 drives the integrated graphics; discrete parts are Xe's.
    NotIntegrated(PhysicalDeviceType),
    /// Graphics not yet run with Stela.
    NotRun {
        device_id: u32,
    },
}

/// Admits the Vulkan device `identity`, with the facts its platform adds.
pub fn admit(identity: &Identity) -> Result<Platform, Refusal> {
    if identity.vendor_id != VENDOR_ID {
        return Err(Refusal::NotIntel {
            vendor_id: identity.vendor_id,
        });
    }
    if identity.driver_id != DriverId::IntelOpenSourceMesa {
        return Err(Refusal::NotAnv(identity.driver_id));
    }
    if identity.device_type != PhysicalDeviceType::IntegratedGpu {
        return Err(Refusal::NotIntegrated(identity.device_type));
    }
    let (_, architecture) = DEVICES
        .iter()
        .find(|(device_id, _)| *device_id == identity.device_id)
        .ok_or(Refusal::NotRun {
            device_id: identity.device_id,
        })?;
    Ok(Platform {
        architecture: String::from(*architecture),
        location: DeviceLocation::BuiltIn,
        location_number: 0,
        max_transfer_rate: 0,
        headless: false,
        removable: false,
        linear_row_alignment: LINEAR_ALIGNMENT,
        linear_offset_alignment: LINEAR_ALIGNMENT,
        sparse_page_bytes: SPARSE_PAGE_BYTES,
        // ANV has no VK_KHR_unified_image_layouts (the Intel port's decision 2).
        image_layouts: ImageLayouts::General,
    })
}
