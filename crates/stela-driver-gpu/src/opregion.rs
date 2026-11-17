//! Where an Intel GPU's Video BIOS Table lies in its ACPI OpRegion, found as
//! i915 finds it (Linux v7.2.12, `display/intel_opregion.c`,
//! `intel_opregion_setup`): firmware names the OpRegion's physical address in
//! the GPU's ASLS register, and the VBT is either raw data the ASLE mailbox
//! points at (RVDA and RVDS, OpRegion 2.0 on) or mailbox #4, inside the
//! OpRegion. i915 reads the OpRegion through ACPI, which Stela does not build,
//! so the loader reads it while firmware still maps it and hands the VBT on.
//!
//! Only the location is decided here; whether the bytes are a valid VBT is
//! i915's `intel_bios_is_valid_vbt`, as for any VBT it reads.

/// `OPREGION_SIZE`: the bytes from ASLS that make up the OpRegion.
pub const SIZE: usize = 8 * 1024;
/// `OPREGION_SIGNATURE`, at the OpRegion's start.
const SIGNATURE: &[u8; 16] = b"IntelGraphicsMem";
/// `struct opregion_header`: `over.minor`, `over.major` and `mboxes`.
const MINOR: usize = 0x16;
const MAJOR: usize = 0x17;
const MBOXES: usize = 0x58;
/// `MBOX_ASLE` (mailbox #3) and `MBOX_ASLE_EXT` (mailbox #5).
const MBOX_ASLE: u32 = 1 << 2;
const MBOX_ASLE_EXT: u32 = 1 << 4;
/// `OPREGION_ASLE_OFFSET`, and `struct opregion_asle`'s `rvda` and `rvds`.
const ASLE: usize = 0x300;
const RVDA: usize = ASLE + 0xba;
const RVDS: usize = ASLE + 0xc2;
/// `OPREGION_VBT_OFFSET` and `OPREGION_ASLE_EXT_OFFSET`.
const MAILBOX_VBT: usize = 0x400;
const ASLE_EXT: usize = 0x1c00;

/// Where an OpRegion's VBT may be, in the order i915 tries them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Places {
    /// The raw VBT the ASLE mailbox points at: its physical address and
    /// bytes. i915 takes it when it is a valid VBT.
    pub raw: Option<(u64, usize)>,
    /// Mailbox #4: the VBT's offset and bytes within the OpRegion, which
    /// i915 takes otherwise.
    pub mailbox: (usize, usize),
    /// The OpRegion's version, major and minor.
    pub version: (u8, u8),
}

/// Where the OpRegion read from `asls` keeps its VBT, or `None` when it is
/// not an OpRegion (its signature differs, as i915 refuses it).
pub fn locate(asls: u64, region: &[u8; SIZE]) -> Option<Places> {
    if &region[..SIGNATURE.len()] != SIGNATURE {
        return None;
    }
    let (minor, major) = (region[MINOR], region[MAJOR]);
    let mboxes = u32::from_le_bytes(word(region, MBOXES));
    let raw = (major >= 2 && mboxes & MBOX_ASLE != 0)
        .then(|| {
            let rvda = u64::from_le_bytes(word(region, RVDA));
            let rvds = u32::from_le_bytes(word(region, RVDS));
            // 2.0 names the VBT's physical address; 2.1 on, its offset from
            // the OpRegion.
            let address = if major > 2 || minor >= 1 {
                asls.checked_add(rvda)
            } else {
                Some(rvda)
            };
            (rvda != 0 && rvds != 0)
                .then_some(address)
                .flatten()
                .map(|address| (address, rvds as usize))
        })
        .flatten();
    // The ASLE extension mailbox, when present, ends mailbox #4.
    let end = if mboxes & MBOX_ASLE_EXT != 0 {
        ASLE_EXT
    } else {
        SIZE
    };
    Some(Places {
        raw,
        mailbox: (MAILBOX_VBT, end - MAILBOX_VBT),
        version: (major, minor),
    })
}

fn word<const N: usize>(region: &[u8; SIZE], offset: usize) -> [u8; N] {
    let mut bytes = [0; N];
    bytes.copy_from_slice(&region[offset..offset + N]);
    bytes
}

#[cfg(test)]
mod tests;
