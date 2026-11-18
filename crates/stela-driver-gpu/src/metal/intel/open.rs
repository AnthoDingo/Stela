//! Opens the Metal service for Intel graphics: admits its Vulkan device
//! ([`super::admit`]) and serves a [`Device`] over the backend and compiler,
//! as `nvidia::open` does for NVIDIA. [`open`] composes the same native
//! stack: ANV through `NativeBackend` and the AIR compiler service for the
//! device ANV reports.
use super::Refusal;
#[cfg(feature = "metal-vulkan")]
use crate::metal::{
    compiler::{self, AirCompiler},
    native::{Loader, MemoryExporter, NativeBackend, vk},
    nvidia::open::stack,
};
use crate::metal::{
    device::Device,
    error::Error,
    service::{Service, identity::DeviceEpoch},
    shader::Compiler,
    vulkan::{backend::Backend, physical::PciBusInfo},
};
#[cfg(feature = "metal-vulkan")]
use alloc::boxed::Box;

/// The graphics the platform hands the Metal device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Gpu {
    pub pci: PciBusInfo,
}

/// Why the Metal service did not open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenError {
    /// The device is not Intel graphics this route drives.
    Refused(Refusal),
    /// The device lacks a requirement or could not be created.
    Device(Error),
}

impl From<Error> for OpenError {
    fn from(error: Error) -> Self {
        Self::Device(error)
    }
}

/// Serves the Metal device over `backend`, which drives `gpu`, and
/// `compiler`, in the gate's `epoch` for it.
pub fn serve<B: Backend, C: Compiler>(
    backend: B,
    compiler: C,
    gpu: &Gpu,
    epoch: DeviceEpoch,
) -> Result<Service<B, C>, OpenError> {
    Ok(Service::new(device(backend, compiler, gpu)?, epoch))
}

/// The Metal device over `backend`, once it is admitted as `gpu`.
fn device<B: Backend, C: Compiler>(
    backend: B,
    compiler: C,
    gpu: &Gpu,
) -> Result<Device<B, C>, OpenError> {
    let identity = &backend.physical_device().identity;
    if identity.pci_bus != Some(gpu.pci) {
        return Err(Error::Invalid("the backend drives another GPU").into());
    }
    let platform = super::admit(identity).map_err(OpenError::Refused)?;
    Ok(Device::new(backend, compiler, &platform)?)
}

/// The Metal service over ANV and the AIR compiler.
#[cfg(feature = "metal-vulkan")]
pub type NativeService = Service<NativeBackend, AirCompiler>;

/// Opens ANV's device for `gpu` and serves it with the AIR compiler service
/// `compiler` answers for it.
///
/// # Safety
///
/// `loader` is ANV's entry point and `compiler` answers the AIR compiler
/// service with ANV's NIR options for the device it is given, both outliving
/// the service.
#[cfg(feature = "metal-vulkan")]
pub unsafe fn open(
    loader: Loader,
    exporter: Box<dyn MemoryExporter>,
    compiler: impl FnOnce(vk::PhysicalDevice) -> compiler::Service,
    gpu: &Gpu,
    epoch: DeviceEpoch,
) -> Result<NativeService, OpenError> {
    // SAFETY: the caller's guarantees.
    let (backend, compiler) = unsafe { stack(loader, gpu.pci, exporter, compiler) }?;
    serve(backend, compiler, gpu, epoch)
}

/// Opens ANV's device for `gpu` as [`open`] does, for a service whose device
/// was lost ([`Service::with_reopen`]).
///
/// # Safety
///
/// As for [`open`].
#[cfg(feature = "metal-vulkan")]
pub unsafe fn reopen(
    loader: Loader,
    exporter: Box<dyn MemoryExporter>,
    compiler: impl FnOnce(vk::PhysicalDevice) -> compiler::Service,
    gpu: &Gpu,
) -> Result<Device<NativeBackend, AirCompiler>, Error> {
    // SAFETY: the caller's guarantees.
    let (backend, compiler) = unsafe { stack(loader, gpu.pci, exporter, compiler) }?;
    device(backend, compiler, gpu).map_err(|error| match error {
        OpenError::Device(error) => error,
        OpenError::Refused(_) => Error::Invalid("the GPU was refused as it reopened"),
    })
}

#[cfg(test)]
mod tests;
