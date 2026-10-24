// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **PROM 2002's halts**, each reached on both engines: QUUX's built-in
//! PROM, `data/quux-promh.mcr`, halts --- `HALT-CONS`, `ERR` up --- at the
//! halt its `promh.text` names for each fault, and writes nothing to the
//! disk on the way. Each halt after the PROM has set error stop stops the
//! machine; `ERROR-NOT-REVISION-15`, which comes before, leaves the clock
//! running, the PC held at the halt.
//!
//! Every disk is `data/quux-disk.img`, the GPT disk sgdisk made, with
//! `support::idle_microcode_15` in its current `MCR1`, the one fault
//! planted in it: in the GPT, which the PROM reads at `READ-LABEL` to
//! `FOUND-PARTITION`, or in the microcode's `.mcr`, which it reads at
//! QUUX's revision-15 reader, its halts named in the comment that opens
//! it. `ERROR-NOT-REVISION-15` is the machine's: MACHINE-ID's hardware
//! revision below 15, tested at `GO` by `<7:4>` all set below 16.
//!
//! The addresses are those PROM 2002's `promh.sym` gives, which
//! [`the_halts_are_promh_sym_s`] holds them to where the hand-over is
//! fetched; nothing else here needs it.

use std::path::{Path, PathBuf};

use muir::engine::Engine;
use muir::micro::Micro;
use muir::pipeline::Pipeline;

mod support;

use support::prom_2002::{Halt, halt, machine_id, quux};

/// More than the PROM takes to reach any halt or the microcode's 6, which
/// is under 300,000 microcycles on either engine (measured).
const LIMIT: u64 = 2_000_000;

/// PROM 2002's halts, by `promh.sym`.
const ERROR_NO_GPT: u16 = 0o36776;
const ERROR_NO_CURRENT_MICR: u16 = 0o37000;
const ERROR_ODD_MICR_START: u16 = 0o37002;
const ERROR_NOT_REVISION_15: u16 = 0o37004;
const ERROR_MCR_FORMAT: u16 = 0o37006;
const ERROR_MICROCODE_NOT_REVISION_15: u16 = 0o37010;
const ERROR_SECTION_ORDER: u16 = 0o37012;
const ERROR_BAD_WIDTH: u16 = 0o37014;
const ERROR_NONZERO_PADDING: u16 = 0o37016;
const ERROR_BAD_SECTION_TYPE: u16 = 0o36022;
const ERROR_BAD_ADDRESS: u16 = 0o36024;
const ERROR_MICROCODE_TOO_BIG: u16 = 0o36036;
const ERROR_BUFFER_NOT_LOADED: u16 = 0o36042;

/// Each halt by its name, as `promh.sym` spells it.
const HALTS: &[(&str, u16)] = &[
    ("ERROR-NO-GPT", ERROR_NO_GPT),
    ("ERROR-NO-CURRENT-MICR", ERROR_NO_CURRENT_MICR),
    ("ERROR-ODD-MICR-START", ERROR_ODD_MICR_START),
    ("ERROR-NOT-REVISION-15", ERROR_NOT_REVISION_15),
    ("ERROR-MCR-FORMAT", ERROR_MCR_FORMAT),
    ("ERROR-MICROCODE-NOT-REVISION-15", ERROR_MICROCODE_NOT_REVISION_15),
    ("ERROR-SECTION-ORDER", ERROR_SECTION_ORDER),
    ("ERROR-BAD-WIDTH", ERROR_BAD_WIDTH),
    ("ERROR-NONZERO-PADDING", ERROR_NONZERO_PADDING),
    ("ERROR-BAD-SECTION-TYPE", ERROR_BAD_SECTION_TYPE),
    ("ERROR-BAD-ADDRESS", ERROR_BAD_ADDRESS),
    ("ERROR-MICROCODE-TOO-BIG", ERROR_MICROCODE_TOO_BIG),
    ("ERROR-BUFFER-NOT-LOADED", ERROR_BUFFER_NOT_LOADED),
];

/// Runs `pack` on both engines on MACHINE-ID `id`, each on a copy, and
/// says where each stopped, holding each to have written no block.
fn stops(pack: &Path, id: u32, limit: u64, what: &str) -> [Option<Halt>; 2] {
    ["micro", "rtl"].map(|engine| {
        let copy = pack.with_extension(format!("{engine}.img"));
        std::fs::copy(pack, &copy).unwrap();
        let (pc, m) = if engine == "micro" {
            let mut e = Micro::new(quux(&copy, id));
            (halt(&mut e, limit), e.machine().clone())
        } else {
            let mut e = Pipeline::new(quux(&copy, id));
            (halt(&mut e, limit), e.machine().clone())
        };
        let log = m.block_disk.as_ref().unwrap().log.as_ref().unwrap();
        eprintln!(
            "{what} on {engine}: {}, {} blocks read",
            pc.map_or("ran on".to_string(), |h| format!(
                "{} at {:o}",
                if h.stopped { "stopped" } else { "ERR up, running" },
                h.pc
            )),
            log.len()
        );
        assert!(!log.iter().any(|t| t.write), "{what} on {engine}: no block written");
        assert!(
            std::fs::read(&copy).unwrap() == std::fs::read(pack).unwrap(),
            "{what} on {engine}: the disk unchanged"
        );
        pc
    })
}

/// The name of the halt at `pc`, for a failure message.
fn named(h: Option<Halt>) -> String {
    match h {
        None => "no halt".to_string(),
        Some(Halt { pc, .. }) => HALTS
            .iter()
            .find(|h| h.1 == pc)
            .map_or_else(|| format!("{pc:o}"), |h| format!("{} at {pc:o}", h.0)),
    }
}

/// The idle microcode's `.mcr` as 32-bit words, with where its sections'
/// headers are checked, so that a fault lands where it is meant to.
fn words() -> Vec<u32> {
    let b = support::idle_microcode_15();
    let w: Vec<u32> = b.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect();
    assert_eq!(w[..2], [muir::mcr::FORMAT_WORD, 4], "the format word and four sections");
    assert_eq!(w[2..11], [6, 1, 32, 32, 0, 0, 0, 0, 15], "section 6, saying 15");
    assert_eq!(w[11..19], [1, 8, 64, 64, 0, 0, 0, 0], "the control store's header");
    assert_eq!(w[35..43], [3, 1024, 40, 64, 0o6000, 0, 0, 0], "the symbol area's header");
    assert_eq!(w[2091..2099], [4, 1, 40, 64, 0o40, 0, 0, 0], "A memory's header");
    w
}

/// A disk in `dir` named `name` with the idle microcode, `edit` applied to
/// its words, in `MCR1`.
fn disk_with(dir: &Path, name: &str, edit: impl FnOnce(&mut Vec<u32>)) -> PathBuf {
    let mut w = words();
    edit(&mut w);
    let mcr: Vec<u8> = w.iter().flat_map(|x| x.to_le_bytes()).collect();
    let (pack, _) = support::quux_gpt_disk(dir, &mcr);
    let named = dir.join(format!("{name}.img"));
    std::fs::rename(pack, &named).unwrap();
    named
}

/// The GPT's entries in a disk image's bytes: the offset of each entry
/// that is in use, by the header at LBA 1 (UEFI: the entry array's LBA at
/// 72, the count at 80, the entry size at 84; an entry's type GUID at 0).
fn gpt_entries(bytes: &[u8]) -> Vec<usize> {
    let le =
        |at: usize, n: usize| bytes[at..at + n].iter().rev().fold(0u64, |v, &x| v << 8 | x as u64);
    assert_eq!(&bytes[512..520], b"EFI PART");
    let (array, count, size) = (le(512 + 72, 8) as usize, le(512 + 80, 4), le(512 + 84, 4));
    (0..count as usize)
        .map(|k| array * 512 + k * size as usize)
        .filter(|&at| bytes[at..at + 16].iter().any(|&b| b != 0))
        .collect()
}

/// The entry named `MCR1` in `bytes`: its name is UTF-16LE at 56.
fn mcr1_entry(bytes: &[u8]) -> usize {
    let name: Vec<u8> = "MCR1".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    *gpt_entries(bytes)
        .iter()
        .find(|&&at| bytes[at + 56..at + 64] == name[..])
        .expect("MCR1 in the GPT")
}

/// A disk in `dir` named `name` with the idle microcode in `MCR1` and
/// `edit` applied to the image's bytes.
fn gpt_with(dir: &Path, name: &str, edit: impl FnOnce(&mut Vec<u8>)) -> PathBuf {
    let pack = disk_with(dir, name, |_| {});
    let mut bytes = std::fs::read(&pack).unwrap();
    edit(&mut bytes);
    std::fs::write(&pack, &bytes).unwrap();
    pack
}

/// **Each fault stops PROM 2002 at its own halt**, on both engines, with
/// no block of the disk written; the disk without a fault, the control,
/// runs on into the microcode on both. **Fails** a PROM, or an engine,
/// that takes a fault it should refuse or stops at another halt.
#[test]
fn each_fault_stops_prom_2002_at_its_halt() {
    let dir = support::scratch("quux-prom-halts");
    let id = machine_id(16);
    let control = disk_with(&dir, "control", |_| {});
    assert_eq!(stops(&control, id, 400_000, "the control"), [None, None], "the control runs on");

    type Edit = fn(&mut Vec<u32>);
    let mcr: &[(&str, u16, Edit)] = &[
        ("a format word not QUUX's", ERROR_MCR_FORMAT, |w| w[0] = 0x5155_0002),
        ("section 6 saying 14", ERROR_MICROCODE_NOT_REVISION_15, |w| w[10] = 14),
        ("the control store's type 5", ERROR_BAD_SECTION_TYPE, |w| w[11] = 5),
        ("the symbol area's type 1, a second control store", ERROR_SECTION_ORDER, |w| w[35] = 1),
        ("the control store 48 bits wide", ERROR_BAD_WIDTH, |w| w[13] = 48),
        ("the control store's header word 5 set", ERROR_NONZERO_PADDING, |w| w[16] = 1),
        ("the control store from 40000", ERROR_BAD_ADDRESS, |w| w[15] = 0o40000),
        ("the control store from 36000", ERROR_MICROCODE_TOO_BIG, |w| w[15] = 0o36000),
        ("the symbol area from 4000", ERROR_BUFFER_NOT_LOADED, |w| w[39] = 0o4000),
    ];
    let mut failed = Vec::new();
    for (k, &(what, want, edit)) in mcr.iter().enumerate() {
        let pack = disk_with(&dir, &format!("mcr-{k}"), edit);
        let got = stops(&pack, id, LIMIT, what);
        if got != [Some(Halt { pc: want, stopped: true }); 2] {
            failed.push(format!("{what}: {} and {}", named(got[0]), named(got[1])));
        }
        std::fs::remove_file(pack).unwrap();
    }

    type Bytes = fn(&mut Vec<u8>);
    let gpt: &[(&str, u16, Bytes)] = &[
        ("block 0 zero, no GPT", ERROR_NO_GPT, |b| b[..1024].fill(0)),
        ("the header's signature broken", ERROR_NO_GPT, |b| b[512] = b'e'),
        ("entries of 384 bytes", ERROR_NO_GPT, |b| b[512 + 85] = 1),
        ("no entry with attribute bit 48", ERROR_NO_CURRENT_MICR, |b| {
            for at in gpt_entries(b) {
                b[at + 48 + 6] &= !1;
            }
        }),
        ("MCR1 from an odd sector", ERROR_ODD_MICR_START, |b| {
            let at = mcr1_entry(b);
            b[at + 32] |= 1;
        }),
    ];
    for (k, &(what, want, edit)) in gpt.iter().enumerate() {
        let pack = gpt_with(&dir, &format!("gpt-{k}"), edit);
        let got = stops(&pack, id, LIMIT, what);
        if got != [Some(Halt { pc: want, stopped: true }); 2] {
            failed.push(format!("{what}: {} and {}", named(got[0]), named(got[1])));
        }
        std::fs::remove_file(pack).unwrap();
    }
    assert!(failed.is_empty(), "stopped elsewhere:\n{}", failed.join("\n"));
}

/// **PROM 2002 halts at `ERROR-NOT-REVISION-15` on a machine of revision
/// 14 or below**, before it reads the disk, and runs on at 15: MACHINE-ID's
/// `<7:4>` all set or a bit of `<15:8>` (`GO`). The check comes before the
/// PROM sets error stop, so `ERR` goes up and the clock runs on, the PC
/// held at the halt. **Fails** a PROM that runs on an earlier revision, or
/// an engine that gives it another MACHINE-ID.
#[test]
fn prom_2002_halts_on_a_revision_below_15() {
    let dir = support::scratch("quux-prom-revision");
    let pack = disk_with(&dir, "pack", |_| {});
    for r in [0, 13, 14] {
        let got = stops(&pack, machine_id(r), LIMIT, &format!("revision {r}"));
        let at = Halt { pc: ERROR_NOT_REVISION_15, stopped: false };
        assert_eq!(got, [Some(at); 2], "revision {r}: {}", named(got[0]));
    }
    for r in [15, 16] {
        let got = stops(&pack, machine_id(r), 400_000, &format!("revision {r}"));
        assert_eq!(got, [None, None], "revision {r}: {} and {}", named(got[0]), named(got[1]));
    }
}

/// **The halts are where PROM 2002's `promh.sym` puts them**: the
/// hand-over's sources' `sys/ubin/promh.sym`, which the fetch script
/// copies beside the disks and whose `promh.mcr` is `data/quux-promh.mcr`
/// byte for byte (`tests/system_2002.rs`). Skipped without the hand-over.
#[test]
fn the_halts_are_promh_sym_s() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("ref/band-2002-2999694");
    let sym = dir.join("promh.sym");
    if !sym.exists() {
        eprintln!(
            "skipped: ref/band-2002-2999694/promh.sym is not present; tools/fetch-handover-2002.sh fetches it"
        );
        return;
    }
    let mcr = std::fs::read(dir.join("promh.mcr")).unwrap();
    assert!(
        mcr == std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("data/quux-promh.mcr"))
            .unwrap(),
        "the symbols are the built-in PROM's"
    );
    let text = std::fs::read_to_string(&sym).unwrap();
    for &(name, at) in HALTS {
        let line = text
            .lines()
            .find(|l| l.split_whitespace().next() == Some(name))
            .unwrap_or_else(|| panic!("{name} is not in promh.sym"));
        let words: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(words[1], "I-MEM", "{line}");
        assert_eq!(u16::from_str_radix(words[2], 8).unwrap(), at, "{line}");
    }
}
