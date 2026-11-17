use super::*;

const ASLS: u64 = 0x7a92_7018;

fn region(major: u8, minor: u8, mboxes: u32, rvda: u64, rvds: u32) -> [u8; SIZE] {
    let mut region = [0; SIZE];
    region[..SIGNATURE.len()].copy_from_slice(SIGNATURE);
    region[MINOR] = minor;
    region[MAJOR] = major;
    region[MBOXES..MBOXES + 4].copy_from_slice(&mboxes.to_le_bytes());
    region[RVDA..RVDA + 8].copy_from_slice(&rvda.to_le_bytes());
    region[RVDS..RVDS + 4].copy_from_slice(&rvds.to_le_bytes());
    region
}

#[test]
fn a_latitude_7490s_opregion_keeps_its_vbt_in_mailbox_4_before_the_asle_extension() {
    // Its OpRegion as read over SSH: version 2.0, mailboxes 0x1f, no raw VBT,
    // and a 6144-byte VBT at 0x400.
    assert_eq!(
        locate(ASLS, &region(2, 0, 0x1f, 0, 0)),
        Some(Places {
            raw: None,
            mailbox: (0x400, 6144),
            version: (2, 0),
        })
    );
}

#[test]
fn mailbox_4_runs_to_the_opregions_end_without_the_asle_extension() {
    assert_eq!(locate(ASLS, &region(2, 0, 0x7, 0, 0)).unwrap().mailbox, (0x400, 0x1c00));
}

#[test]
fn a_raw_vbt_is_a_physical_address_in_version_2_0_and_an_offset_from_2_1() {
    let raw = |major, minor| locate(ASLS, &region(major, minor, 0x1f, 0x2000, 0x1800)).unwrap().raw;
    assert_eq!(raw(2, 0), Some((0x2000, 0x1800)));
    assert_eq!(raw(2, 1), Some((ASLS + 0x2000, 0x1800)));
    assert_eq!(raw(3, 0), Some((ASLS + 0x2000, 0x1800)));
}

#[test]
fn a_raw_vbt_needs_the_asle_mailbox_an_address_and_a_size() {
    assert_eq!(locate(ASLS, &region(2, 1, 0x1b, 0x2000, 0x1800)).unwrap().raw, None);
    assert_eq!(locate(ASLS, &region(2, 1, 0x1f, 0, 0x1800)).unwrap().raw, None);
    assert_eq!(locate(ASLS, &region(2, 1, 0x1f, 0x2000, 0)).unwrap().raw, None);
    assert_eq!(locate(ASLS, &region(1, 0, 0x1f, 0x2000, 0x1800)).unwrap().raw, None);
}

#[test]
fn memory_without_the_opregion_signature_is_refused() {
    let mut other = region(2, 0, 0x1f, 0, 0);
    other[0] = b'X';
    assert_eq!(locate(ASLS, &other), None);
}
