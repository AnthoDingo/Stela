// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright (C) 2012-2014, 2018-2026 Intel Corporation
//! The runtime image's start against the boot tests' model: the PHY DB's
//! order, paging, a scripted bring-up, and Linux's own, where every command
//! but the two left out must match Linux's byte for byte.
extern crate std;
use super::*;
use crate::wifi::intel::{
    dma::tests::{Domain, Source},
    gen1::{
        boot::tests::{
            Event, Line, Model, alive_packet, catalog, device_read, image, latitude_7400,
            latitude_7490, normalize, packet, parse, staged,
        },
        init::tests::hex,
    },
    identity::Observed,
};
use alloc::{string::String, vec, vec::Vec};

#[test]
fn phy_db_goes_back_configuration_first_then_each_group_in_order() {
    let group = |kind: u16, group: u16, fill: u8| {
        let mut data = group.to_le_bytes().to_vec();
        data.extend([fill; 6]);
        (kind, data)
    };
    // As the INIT image reports them: TX power from the last group down,
    // PAPD last, and one group reported twice.
    let sections = vec![
        (1, vec![0x11; 4]),
        (2, vec![0x22; 2]),
        group(5, 2, 0xa0),
        group(5, 1, 0xa1),
        group(5, 0, 0xa2),
        group(5, 1, 0xb1),
        group(4, 0, 0xc0),
        (3, vec![0x33]),
    ];
    let sent: Vec<_> = phy_db_order(&sections)
        .unwrap()
        .iter()
        .map(|p| (u16::from_le_bytes([p[0], p[1]]), p[4..].to_vec()))
        .collect();
    assert_eq!(
        sent,
        [
            (1, vec![0x11; 4]),
            (2, vec![0x22; 2]),
            group(4, 0, 0xc0),
            group(5, 0, 0xa2),
            group(5, 1, 0xb1),
            group(5, 2, 0xa0),
        ]
    );
    // Each payload says its own length.
    assert_eq!(phy_db_order(&sections).unwrap()[0][..4], [1, 0, 4, 0]);
    // Without the configuration section there is nothing to send.
    assert!(phy_db_order(&sections[1..]).is_err());
}

#[test]
fn the_smart_fifo_command_is_linuxs_unassociated_one() {
    let payload = smart_fifo();
    assert_eq!(payload.len(), 92);
    assert_eq!(
        payload[..20],
        hex("03000000001000000020000040420f0040420f00")[..]
    );
    assert_eq!(payload[52..60], hex("90010000a0000000")[..]);
}

/// A runtime image with a paged part: a CSS of 100 bytes and ten pages,
/// so one full block and a last one of two pages.
fn paged_image() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (mut bytes, _) = image(46);
    let mut tlv = |kind: u32, data: &[u8]| {
        bytes.extend(kind.to_le_bytes());
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(data);
        bytes.resize(bytes.len().next_multiple_of(4), 0);
    };
    let css: Vec<u8> = (0..100).map(|n| n as u8).collect();
    let pages: Vec<u8> = (0..10 * 4096).map(|n| (n / 4096) as u8 + 1).collect();
    tlv(32, &(10u32 * 4096).to_le_bytes());
    for (offset, data) in [
        (0xffff_cccc, &[0u8; 4][..]),
        (0xaaaa_bbbb, &[0; 4][..]),
        (0x0, &css[..]),
        (0x0, &pages[..]),
    ] {
        let mut payload = u32::to_le_bytes(offset).to_vec();
        payload.extend(data);
        tlv(19, &payload);
    }
    (bytes, css, pages)
}

/// A firmware answering each command in turn, by its opcode.
fn answering(model: &Model, opcodes: &[u8]) {
    let mut d = model.0.lock().unwrap();
    for (index, &opcode) in opcodes.iter().enumerate() {
        d.responses
            .push_back(vec![packet(opcode, 1, index as u16, &[0; 4])]);
    }
}

fn phy_db() -> Init {
    Init {
        nvm: Vec::new(),
        hardware_address: [0; 6],
        phy_db: vec![(1, vec![0x11; 48]), (2, vec![0x22; 4]), (5, vec![0, 0, 9])],
    }
}

#[test]
fn the_runtime_image_pages_then_configures_and_stops() {
    let observed = latitude_7400();
    let (bytes, css, pages) = paged_image();
    let catalog = catalog(observed, bytes);
    let (profile, staged) = staged(observed, &catalog);
    let domain = Arc::new(Domain::default());
    let model = Model::new(domain.clone(), alive_packet(), observed);
    answering(
        &model,
        &[0x4f, 0x8c, 0xd1, 0x98, 0x6c, 0x6c, 0x6c, 0x6a, 0x9b],
    );
    // SAFETY: the model is the function's only user and the test domain
    // isolates it.
    let (mut boot, runtime) = unsafe {
        start(
            model.clone(),
            profile,
            staged,
            &phy_db(),
            &Source::default(),
            domain.clone(),
        )
    }
    .unwrap();
    assert_eq!(runtime.alive.scheduler_base, 0x817080);
    let commands = model.0.lock().unwrap().commands.clone();
    let ids: Vec<_> = commands.iter().map(|c| (c[1], c[0])).collect();
    // Paging, legacy shared memory (no capability 80), smart FIFO, the
    // antennas, three PHY DB sections, the PHY configuration and BT; no SOC
    // or DQA without their capabilities.
    assert_eq!(
        ids,
        [
            (1, 0x4f),
            (1, 0x8c),
            (1, 0xd1),
            (1, 0x98),
            (1, 0x6c),
            (1, 0x6c),
            (1, 0x6c),
            (1, 0x6a),
            (1, 0x9b)
        ]
    );
    // The runtime image's calibration triggers, and BT without MPLUT.
    assert_eq!(commands[7][8..], hex("18003300db15060f8b95020f")[..]);
    assert_eq!(commands[8][8..], hex("0100000014000000")[..]);
    // Two data blocks, two pages in the last; the CSS and the pages sit in
    // their blocks, zero-filled.
    let paging = &commands[0][8..];
    assert_eq!(paging.len(), 12 + 4 * 33);
    assert_eq!(paging[..12], hex("020300000f00000002000000")[..]);
    let block = |n: usize| {
        u64::from(u32::from_le_bytes(
            paging[12 + 4 * n..16 + 4 * n].try_into().unwrap(),
        )) << 12
    };
    assert!(paging[24..].iter().all(|&b| b == 0));
    let first = device_read(&domain, block(0), 4096);
    assert_eq!(
        (&first[..100], first[100..].iter().all(|&b| b == 0)),
        (&css[..], true)
    );
    assert_eq!(device_read(&domain, block(1), 32768), pages[..32768]);
    let last = device_read(&domain, block(2), 32768);
    assert_eq!(last[..8192], pages[32768..]);
    assert!(last[8192..].iter().all(|&b| b == 0));
    boot.stop().unwrap();
    // The paging blocks went with the rest.
    assert!(domain.records.lock().unwrap().live.is_empty());
}

#[test]
fn a_runtime_failure_stops_the_function() {
    let observed = latitude_7490();
    let catalog = catalog(observed, image(36).0);
    let (profile, staged) = staged(observed, &catalog);
    let domain = Arc::new(Domain::default());
    let model = Model::new(domain.clone(), alive_packet(), observed);
    answering(&model, &[0x8c, 0xd1, 0x98]);
    let init = Init {
        phy_db: Vec::new(),
        ..phy_db()
    };
    // SAFETY: as above.
    let result = unsafe {
        start(
            model.clone(),
            profile,
            staged,
            &init,
            &Source::default(),
            domain.clone(),
        )
    };
    assert!(matches!(
        result.err(),
        Some(Error::InitializationResponse { command: 0x6b, .. })
    ));
    assert!(domain.records.lock().unwrap().live.is_empty());
}

/// `LDBG_CONFIG_CMD` and LARI, which Stela does not send.
fn left_out(command: &[u8]) -> bool {
    matches!((command[1], command[0]), (1, 0xf6) | (0x0c, 0x01))
}

/// Linux's runtime start, from the reset before the runtime image's load
/// to DQA_ENABLE's response, in the trace `trace_variable` names, against
/// this one on a model of `observed` that answers each read as the card did
/// and each command with Linux's response, renumbered for the commands left
/// out.
fn linux_runtime_matches(observed: Observed, trace_variable: &str) {
    let trace = std::fs::read_to_string(std::env::var(trace_variable).unwrap()).unwrap();
    let root = std::env::var("STELA_IWLWIFI_IMAGES").unwrap();
    let name = Profile::select(observed).unwrap().firmware_name();
    let image = std::fs::read(std::path::Path::new(&root).join(name)).unwrap();
    let field = |line: &str, name: &str| {
        line.split_whitespace()
            .find_map(|f| f.strip_prefix(name))
            .map(String::from)
    };
    // INIT's PHY DB, the runtime image's ALIVE, then each command with the
    // packets that followed it.
    let mut accesses = Vec::new();
    let (mut alives, mut alive, mut phy_db) = (0, None, Vec::new());
    let mut after_alive = Vec::new();
    let mut commands: Vec<(Vec<u8>, Vec<Vec<u8>>)> = Vec::new();
    for line in trace.lines() {
        match line.split_whitespace().nth(2) {
            Some("iwlwifi:iwlwifi_dev_rx") => {
                let bytes = hex(&field(line, "rxbuf=").unwrap());
                if alive.is_none() {
                    match field(line, "cmd=").unwrap().as_str() {
                        "0x1" => {
                            alives += 1;
                            if alives == 3 {
                                alive = Some(bytes);
                            }
                        }
                        "0x6b" => {
                            let length = usize::from(u16::from_le_bytes([bytes[10], bytes[11]]));
                            let kind = u16::from_le_bytes([bytes[8], bytes[9]]);
                            phy_db.push((kind, bytes[12..12 + length].to_vec()));
                        }
                        _ => {}
                    }
                    continue;
                }
                match commands.last_mut() {
                    Some((_, packets)) => packets.push(bytes),
                    None => after_alive.push(bytes),
                }
            }
            Some("iwlwifi:iwlwifi_dev_hcmd") if alive.is_some() => {
                if commands.last().is_some_and(|(c, _)| (c[1], c[0]) == (5, 0)) {
                    break;
                }
                commands.push((hex(&field(line, "hcmd=").unwrap()), Vec::new()));
            }
            _ => {
                if let Some(Line::Access(event, read)) = parse(line) {
                    accesses.push((event, read, alive.is_some()));
                }
            }
        }
    }
    // Each response goes with its own command (Linux sends the smart FIFO
    // without waiting); left-out commands lose theirs, and the rest are
    // renumbered as Stela sends them.
    let response = |seq: &[u8]| {
        commands
            .iter()
            .flat_map(|(_, packets)| packets)
            .find(|p| p[7] & 0x80 == 0 && p[6..8] == *seq)
            .cloned()
            .unwrap()
    };
    let mut ours = Vec::new();
    let mut responses = Vec::new();
    for (command, packets) in &commands {
        let notifications = packets.iter().filter(|p| p[7] & 0x80 != 0).cloned();
        if left_out(command) {
            match responses.last_mut() {
                Some(last) => Vec::extend(last, notifications),
                None => after_alive.extend(notifications),
            }
            continue;
        }
        let index = (ours.len() as u16).to_le_bytes();
        let mut reply = response(&command[2..4]);
        reply[6..8].copy_from_slice(&index);
        let mut command = command.clone();
        command[2..4].copy_from_slice(&index);
        ours.push(command);
        responses.push([vec![reply], notifications.collect()].concat());
    }
    after_alive.retain(|p| p[7] & 0x80 != 0);
    let domain = Arc::new(Domain::default());
    let model = Model::new(domain.clone(), alive.expect("a runtime ALIVE"), observed);
    // From the software reset of the runtime image's `start_hw`: the last
    // reset before its `apm_init`, the second-to-last before ALIVE.
    let alive_at = accesses.iter().position(|(_, _, after)| *after).unwrap();
    let apm: Vec<_> = (0..alive_at)
        .filter(|&n| matches!(accesses[n].0, Event::Write(0x100, _)))
        .collect();
    let reset = accesses[..apm[apm.len() - 2]]
        .iter()
        .rposition(|(e, _, _)| matches!(e, Event::Write(0x20, _)))
        .unwrap();
    let from = reset - usize::from(accesses[reset - 1].0 == Event::Read(0x20));
    let accesses = &accesses[from..];
    {
        let mut d = model.0.lock().unwrap();
        d.after_alive = after_alive;
        d.responses = responses.into_iter().collect();
        for &(event, value, _) in accesses {
            match (event, value) {
                (Event::Read(r), Some(v)) if r != 0x24 && r != 0x8 => {
                    d.replay.entry(r).or_default().push_back(v)
                }
                (Event::PrphRead(a), Some(v)) => d.replay_prph.entry(a).or_default().push_back(v),
                _ => {}
            }
        }
    }
    let catalog = catalog(observed, image);
    let (profile, staged) = staged(observed, &catalog);
    let init = Init {
        phy_db,
        ..phy_db_free()
    };
    // SAFETY: as above.
    let (mut boot, runtime) = unsafe {
        start(
            model.clone(),
            profile,
            staged,
            &init,
            &Source::default(),
            domain.clone(),
        )
    }
    .unwrap();
    assert_eq!(runtime.alive.image_type, 1);
    let d = model.0.lock().unwrap();
    assert_eq!(d.commands.len(), ours.len(), "commands: ours, Linux");
    for (n, (a, b)) in d.commands.iter().zip(&ours).enumerate() {
        // The paging blocks' addresses are DMA addresses, present only.
        let mask = |c: &[u8]| {
            let mut c = c.to_vec();
            if (c[1], c[0]) == (1, 0x4f) {
                for word in c[20..].chunks_mut(4) {
                    let present = word.iter().any(|&b| b != 0);
                    word.copy_from_slice(&u32::from(present).to_le_bytes());
                }
            }
            c
        };
        assert_eq!(mask(a), mask(b), "command {n}");
    }
    // Every access; Linux's doorbells count the commands left out.
    let doorbell = |e: &Event| !matches!(e, Event::Write(0x460, _));
    let ours: Vec<_> = normalize(&d.events).into_iter().filter(doorbell).collect();
    let linux: Vec<_> = normalize(&accesses.iter().map(|(e, _, _)| *e).collect::<Vec<_>>())
        .into_iter()
        .filter(doorbell)
        .collect();
    for (n, (a, b)) in ours.iter().zip(&linux).enumerate() {
        assert_eq!(a, b, "access {n}: ours {a:x?}, Linux {b:x?}");
    }
    assert_eq!(ours.len(), linux.len());
    drop(d);
    boot.stop().unwrap();
}

fn phy_db_free() -> Init {
    Init {
        nvm: Vec::new(),
        hardware_address: [0; 6],
        phy_db: Vec::new(),
    }
}

#[test]
#[ignore = "oracle: STELA_IWLWIFI_9560_TRACE and STELA_IWLWIFI_IMAGES"]
fn linux_9560_runtime_start_matches() {
    linux_runtime_matches(latitude_7400(), "STELA_IWLWIFI_9560_TRACE");
}

#[test]
#[ignore = "oracle: STELA_IWLWIFI_GEN1_TRACE and STELA_IWLWIFI_IMAGES"]
fn linux_8265_runtime_start_matches() {
    linux_runtime_matches(latitude_7490(), "STELA_IWLWIFI_GEN1_TRACE");
}
