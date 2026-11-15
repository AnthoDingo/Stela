// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright (C) 2012-2014, 2018-2026 Intel Corporation
//! The INIT run against the boot tests' model playing the firmware: a
//! scripted one for each chip, and Linux's own packets in the oracle cases,
//! where every command must match Linux's byte for byte.
extern crate std;
use super::*;
use crate::wifi::intel::{
    dma::tests::{Domain, Source},
    gen1::boot::tests::{
        Event, Line, Model, alive_packet, catalog, image, latitude_7400, latitude_7490, normalize,
        packet, parse, staged,
    },
    identity::Observed,
};
use alloc::{string::String, vec, vec::Vec};

/// The Latitude 7490's OTP address words, f4:d1:08:42:39:25.
const WFMP_7490: [(u32, u32); 2] = [(0x03080, 0xf4d1_0842), (0x03084, 0x3925)];

/// Boot `observed`'s image on `model`, run INIT and stop.
fn run_on(model: &Model, domain: &Arc<Domain>, observed: Observed, image: Vec<u8>) -> Init {
    let catalog = catalog(observed, image);
    let (profile, staged) = staged(observed, &catalog);
    let source = Source::default();
    // SAFETY: the model is the function's only user and the test domain
    // isolates it.
    let (_, init, _) =
        unsafe { run(model.clone(), profile, staged, &source, domain.clone()) }.unwrap();
    init
}

/// An NVM_ACCESS_CMD response for command `index`.
fn nvm_response(index: u16, section: u16, offset: u16, status: u16, data: &[u8]) -> Vec<u8> {
    let mut payload = offset.to_le_bytes().to_vec();
    payload.extend((data.len() as u16).to_le_bytes());
    payload.extend(section.to_le_bytes());
    payload.extend(status.to_le_bytes());
    payload.extend(data);
    packet(0x88, 1, index, &payload)
}

/// A firmware that has NVM sections `present` (each under 2 KiB but the
/// third section, which takes two chunks), then calibrates into two PHY DB
/// sections. Returns what INIT kept and every command it sent.
fn scripted(observed: Observed, present: &[(u16, Vec<u8>)]) -> (Init, Vec<Vec<u8>>) {
    let domain = Arc::new(Domain::default());
    let model = Model::new(domain.clone(), alive_packet(), observed);
    {
        let mut d = model.0.lock().unwrap();
        // A notification Linux logs and drops, right after ALIVE.
        d.after_alive.push(packet(0xb1, 0, 0xc001, &[0; 20]));
        let mut index = 0;
        for section in 0..NVM_SECTIONS {
            match present.iter().find(|(s, _)| *s == section) {
                Some((_, data)) => {
                    for (n, chunk) in data.chunks(usize::from(NVM_CHUNK)).enumerate() {
                        let offset = (n * usize::from(NVM_CHUNK)) as u16;
                        d.responses
                            .push_back(vec![nvm_response(index, section, offset, 0, chunk)]);
                        index += 1;
                    }
                }
                None => {
                    d.responses
                        .push_back(vec![nvm_response(index, section, 0, 0x100, &[])]);
                    index += 1;
                }
            }
        }
        d.responses
            .push_back(vec![packet(0x98, 1, index, &[0xfe, 0xca, 0, 0])]);
        let phy_db = |kind: u16, data: &[u8], sequence: u16| {
            let mut payload = kind.to_le_bytes().to_vec();
            payload.extend((data.len() as u16).to_le_bytes());
            payload.extend(data);
            packet(PHY_DB, 0, sequence, &payload)
        };
        d.responses.push_back(vec![
            packet(0x6a, 1, index + 1, &[1, 0, 0, 0]),
            phy_db(1, &[0x11; 48], 0x8001),
            phy_db(5, &[0x55; 20], 0x8002),
            packet(INIT_COMPLETE, 0, 0x8003, &[0; 4]),
        ]);
        d.registers.insert(MAC_OTP, 0x50eb71eb);
        d.registers.insert(MAC_OTP + 4, 0x6ab8);
        d.prph.extend(WFMP_7490);
    }
    let api = Profile::select(observed).unwrap().family().firmware_api();
    let init = run_on(&model, &domain, observed, image(api).0);
    // The stop acknowledged master disable, so nothing stays mapped.
    assert!(domain.records.lock().unwrap().live.is_empty());
    let commands = model.0.lock().unwrap().commands.clone();
    (init, commands)
}

pub(in crate::wifi::intel) fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn the_9560_reads_its_nvm_and_csr_address_then_keeps_its_phy_db() {
    let long: Vec<u8> = (0..2148).map(|n| n as u8).collect();
    let (init, commands) = scripted(latitude_7400(), &[(1, vec![0x35; 64]), (3, long.clone())]);
    assert_eq!(init.nvm, [(1, vec![0x35; 64]), (3, long)]);
    assert_eq!(init.hardware_address, [0x50, 0xeb, 0x71, 0xeb, 0x6a, 0xb8]);
    assert_eq!(init.phy_db, [(1, vec![0x11; 48]), (5, vec![0x55; 20])]);
    // Thirteen sections, the third in two chunks, then the antennas and the
    // PHY configuration, as Linux sent them on the 7400.
    assert_eq!(commands.len(), 16);
    assert_eq!(commands[0], hex("88010000080000000000000000000008"));
    assert_eq!(commands[4], hex("88010400080000000000030000080008"));
    assert_eq!(commands[14], hex("98010e000400000003000000"));
    assert_eq!(
        commands[15],
        hex("6a010f000c00000018003300db518e1f01000000")
    );
}

/// With a hardware section and no override, as on the 7490, the 8265 reads
/// its OTP; an override wins over it.
#[test]
fn the_8265_takes_its_address_from_its_otp_unless_the_nvm_overrides_it() {
    let hardware = (HARDWARE, vec![0x10; 548]);
    let (init, commands) = scripted(latitude_7490(), core::slice::from_ref(&hardware));
    assert_eq!(init.hardware_address, [0xf4, 0xd1, 0x08, 0x42, 0x39, 0x25]);
    assert_eq!(commands.len(), 15);
    let mut mac_override = vec![0, 0, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
    mac_override.resize(24, 0);
    let (init, _) = scripted(latitude_7490(), &[hardware, (MAC_OVERRIDE, mac_override)]);
    assert_eq!(init.hardware_address, [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
}

#[test]
fn a_reserved_override_without_a_hardware_section_has_no_address() {
    let domain = Arc::new(Domain::default());
    let observed = latitude_7490();
    let model = Model::new(domain.clone(), alive_packet(), observed);
    {
        let mut d = model.0.lock().unwrap();
        for section in 0..NVM_SECTIONS {
            let data: &[u8] = &[0, 0, 0x02, 0xcc, 0xaa, 0xff, 0xee, 0x00];
            let response = if section == MAC_OVERRIDE {
                nvm_response(section, section, 0, 0, data)
            } else {
                nvm_response(section, section, 0, 0x100, &[])
            };
            d.responses.push_back(vec![response]);
        }
    }
    let catalog = catalog(observed, image(36).0);
    let (profile, staged) = staged(observed, &catalog);
    // SAFETY: as above.
    let result = unsafe {
        run(
            model.clone(),
            profile,
            staged,
            &Source::default(),
            domain.clone(),
        )
    };
    assert_eq!(
        result.err(),
        Some(Error::Nvm(crate::wifi::intel::nvm::Error::HardwareAddress))
    );
    // Stopped all the same.
    assert!(domain.records.lock().unwrap().live.is_empty());
}

/// Linux's INIT run, from the probe to INIT_COMPLETE, in the trace that
/// `trace_variable` names, against this one on a model of `observed` that
/// answers each read as the card did and each command with Linux's packets.
fn linux_init_matches(observed: Observed, trace_variable: &str, address: [u8; 6]) {
    let trace = std::fs::read_to_string(std::env::var(trace_variable).unwrap()).unwrap();
    let root = std::env::var("STELA_IWLWIFI_IMAGES").unwrap();
    let name = Profile::select(observed).unwrap().firmware_name();
    let image = std::fs::read(std::path::Path::new(&root).join(name)).unwrap();
    let field = |line: &str, name: &str| {
        line.split_whitespace()
            .find_map(|f| f.strip_prefix(name))
            .map(String::from)
    };
    let mut accesses = Vec::new();
    let (mut alive, mut after_alive) = (None, Vec::new());
    let (mut commands, mut responses): (Vec<Vec<u8>>, Vec<Vec<Vec<u8>>>) = (vec![], vec![]);
    let mut phy_db = 0;
    for line in trace.lines() {
        match line.split_whitespace().nth(2) {
            Some("iwlwifi:iwlwifi_dev_hcmd") => {
                commands.push(hex(&field(line, "hcmd=").unwrap()));
                responses.push(Vec::new());
            }
            Some("iwlwifi:iwlwifi_dev_rx") => {
                let bytes = hex(&field(line, "rxbuf=").unwrap());
                let command = field(line, "cmd=").unwrap();
                if alive.is_none() {
                    assert_eq!(command, "0x1");
                    alive = Some(bytes);
                    continue;
                }
                phy_db += usize::from(command == "0x6b");
                match responses.last_mut() {
                    Some(packets) => packets.push(bytes),
                    None => after_alive.push(bytes),
                }
                if command == "0x4" {
                    break;
                }
            }
            _ => {
                if let Some(Line::Access(event, read)) = parse(line) {
                    accesses.push((event, read));
                }
            }
        }
    }
    let domain = Arc::new(Domain::default());
    let model = Model::new(domain.clone(), alive.expect("an ALIVE"), observed);
    {
        let mut d = model.0.lock().unwrap();
        d.after_alive = after_alive;
        d.responses = responses.into_iter().collect();
        // Read outside the trace (see `normalize`).
        d.prph.extend(WFMP_7490);
        // From the reset on, every read answers what the card answered.
        let start = accesses
            .iter()
            .position(|(e, _)| matches!(e, Event::Read(0x20) | Event::Write(0x20, _)))
            .unwrap();
        for (event, value) in &accesses[start..] {
            match (event, value) {
                (Event::Read(r), Some(v)) if *r != 0x24 && *r != 0x8 => {
                    d.replay.entry(*r).or_default().push_back(*v)
                }
                (Event::PrphRead(a), Some(v)) => d.replay_prph.entry(*a).or_default().push_back(*v),
                _ => {}
            }
        }
    }
    let init = run_on(&model, &domain, observed, image);
    let d = model.0.lock().unwrap();
    assert_eq!(d.commands.len(), commands.len(), "commands: ours, Linux");
    for (n, (ours, linux)) in d.commands.iter().zip(&commands).enumerate() {
        assert_eq!(ours, linux, "command {n}");
    }
    assert_eq!(init.hardware_address, address);
    assert_eq!(init.phy_db.len(), phy_db);
    // Every access up to INIT_COMPLETE; ours go on to the stop.
    let ours = normalize(&d.events);
    let linux = normalize(&accesses.iter().map(|(e, _)| *e).collect::<Vec<_>>());
    for (n, (a, b)) in ours.iter().zip(&linux).enumerate() {
        assert_eq!(a, b, "access {n}: ours {a:x?}, Linux {b:x?}");
    }
    assert!(ours.len() > linux.len(), "{} {}", ours.len(), linux.len());
}

#[test]
#[ignore = "oracle: STELA_IWLWIFI_9560_TRACE and STELA_IWLWIFI_IMAGES"]
fn linux_9560_init_run_matches() {
    let address = [0x50, 0xeb, 0x71, 0xeb, 0x6a, 0xb8];
    linux_init_matches(latitude_7400(), "STELA_IWLWIFI_9560_TRACE", address);
}

#[test]
#[ignore = "oracle: STELA_IWLWIFI_GEN1_TRACE and STELA_IWLWIFI_IMAGES"]
fn linux_8265_init_run_matches() {
    let address = [0xf4, 0xd1, 0x08, 0x42, 0x39, 0x25];
    linux_init_matches(latitude_7490(), "STELA_IWLWIFI_GEN1_TRACE", address);
}
