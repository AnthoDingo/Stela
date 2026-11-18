use super::*;
use crate::metal::{
    abi::DeviceLocation,
    device::fixture::{MockBackend, MockCompiler},
    vulkan::{DriverId, PhysicalDeviceType},
};
use core::num::NonZeroU64;

const PCI: PciBusInfo = PciBusInfo {
    domain: 0,
    bus: 0,
    device: 2,
    function: 0,
};

/// The mock backend as the Latitude 7490's graphics under ANV.
fn latitude_7490() -> MockBackend {
    let mut backend = MockBackend::new();
    let identity = &mut backend.physical.as_mut().unwrap().identity;
    identity.vendor_id = super::super::VENDOR_ID;
    identity.device_id = 0x5917;
    identity.driver_id = DriverId::IntelOpenSourceMesa;
    identity.device_type = PhysicalDeviceType::IntegratedGpu;
    identity.pci_bus = Some(PCI);
    backend
}

fn serve_on(backend: MockBackend) -> Result<Service<MockBackend, MockCompiler>, OpenError> {
    let epoch = DeviceEpoch::from_registered(NonZeroU64::MIN);
    serve(backend, MockCompiler::default(), &Gpu { pci: PCI }, epoch)
}

#[test]
fn the_latitude_7490s_graphics_are_served_built_in() {
    let mut service = serve_on(latitude_7490()).unwrap();
    let capabilities = service.device().capabilities();
    assert_eq!(capabilities.architecture, "Kaby Lake GT2");
    assert_eq!(capabilities.location, DeviceLocation::BuiltIn);
    assert!(!capabilities.removable);
}

#[test]
fn other_vendors_drivers_types_and_devices_are_refused() {
    let refused = |change: fn(&mut crate::metal::vulkan::physical::Identity)| {
        let mut backend = latitude_7490();
        change(&mut backend.physical.as_mut().unwrap().identity);
        serve_on(backend).err()
    };
    assert_eq!(
        refused(|identity| identity.vendor_id = 0x10de),
        Some(OpenError::Refused(Refusal::NotIntel { vendor_id: 0x10de }))
    );
    assert_eq!(
        refused(|identity| identity.driver_id = DriverId::MesaNvk),
        Some(OpenError::Refused(Refusal::NotAnv(DriverId::MesaNvk)))
    );
    assert_eq!(
        refused(|identity| identity.device_type = PhysicalDeviceType::DiscreteGpu),
        Some(OpenError::Refused(Refusal::NotIntegrated(
            PhysicalDeviceType::DiscreteGpu
        )))
    );
    assert_eq!(
        refused(|identity| identity.device_id = 0x3ea0),
        Some(OpenError::Refused(Refusal::NotRun { device_id: 0x3ea0 }))
    );
}
