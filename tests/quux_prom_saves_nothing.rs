// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's boot PROM saves nothing and writes nothing to the disk (contract
//! Q8): from the boot until the microcode's location 6 runs it writes no
//! block of the disk, and main memory only in physical page 3 of 1024
//! words (words 6000-7777) --- its buffer, and the microcode's symbol
//! area, which it loads last over the buffer --- and word 2377, the command
//! list word of its disk transfers. On a GPT disk blocks 0 to 16 are the
//! protective MBR, the primary GPT header and its entry array, and the
//! machine never writes the table (contract Q8, decided 1). MIT's PROM
//! writes page 0 of memory to block 1 before it loads anything, "In order
//! to not clobber core" (`SAVE-A-PAGE`, `mit/sys/ucadr/promh.text`);
//! muir-sys's `promh.text` for QUUX drops that save.
//!
//! The PROM is `data/quux-promh.mcr`, PROM 2002, muir's built-in QUUX PROM:
//! it finds the microcode through the disk's GPT, the first microcode
//! partition with attribute bit 48 set, and not through MIT's `LABL` label,
//! and on a disk with no GPT it stops at `ERROR-NO-GPT`
//! ([`quux_s_prom_reads_a_gpt_not_mit_s_label`]). The microcode partition
//! holds a `.mcr` in QUUX's own format, as `dd` writes it, at the
//! partition's first block; the PROM reads it by 4-byte transfers, 4 blocks
//! a 1024-word page (contract G2 §4.4).
//!
//! The disk is made here from committed files and always runs:
//! `data/quux-disk.img`, the GPT disk sgdisk made, with
//! `support::idle_microcode_15` in its current `MCR1`. The PROM does not
//! care whose microcode it loads, only about its sections.
//! `tests/system_2002.rs` holds the same of the hand-over's disk, with
//! microcode 2002, where it is fetched.

use muir::band::{self, Label};
use muir::block_disk::Transfer;
use muir::engine::Engine;
use muir::micro::Micro;
use muir::pipeline::Pipeline;

mod support;

use support::prom_2002::{CCW, Halt, LIMIT, halt, machine_id, quux, same_bytes, saves_nothing};

/// **QUUX's PROM writes no block and only page 3 and word 2377 of
/// memory**, on a GPT disk made here with a microcode of QUUX's format in
/// its current `MCR1`. **Fails** a PROM that saves a page to the disk, or
/// stores outside its buffer.
#[test]
fn quux_s_prom_saves_nothing_on_a_disk_made_here() {
    let dir = support::scratch("quux-prom-saves-nothing");
    let mcr = support::idle_microcode_15();
    let (pack, _) = support::quux_gpt_disk(&dir, &mcr);
    saves_nothing(&pack, "img", &mcr, &dir);
}

/// `ERROR-NO-GPT`, where PROM 2002 halts when block 0's second sector is
/// not a GPT header (its `promh.sym`, held by `tests/quux_prom_halts.rs`).
const ERROR_NO_GPT: u16 = 0o36776;

/// **QUUX's PROM finds the microcode through a GPT, not MIT's label**: on a
/// pack with MIT's `LABL` label in block 0 and no GPT --- a T-300 label of
/// MIT's own layout, with `support::idle_microcode_15` in its `MCR1` ---
/// it reads blocks 0 to 3, a page, into its buffer at page 3 and stops
/// itself at `ERROR-NO-GPT`, having written nothing.
#[test]
fn quux_s_prom_reads_a_gpt_not_mit_s_label() {
    let dir = support::scratch("quux-prom-labl");
    let pack = dir.join("pack.img");
    let label = Label::initialize(&pack, &band::T300);
    label.write().unwrap();
    let mcr1 = label.partition("MCR1").expect("MCR1").clone();
    let mcr = support::idle_microcode_15();
    let mut bytes = std::fs::read(&pack).unwrap();
    let at = mcr1.start as usize * 1024;
    if bytes.len() < at + mcr.len() {
        bytes.resize(at + mcr.len(), 0);
    }
    bytes[at..at + mcr.len()].copy_from_slice(&mcr);
    std::fs::write(&pack, &bytes).unwrap();
    let id = machine_id(16);
    for name in ["micro", "rtl"] {
        let copy = dir.join(format!("{name}.img"));
        std::fs::copy(&pack, &copy).unwrap();
        let (at, mut m) = match name {
            "micro" => {
                let mut e = Micro::new(quux(&copy, id));
                (halt(&mut e, LIMIT), e.machine().clone())
            }
            _ => {
                let mut e = Pipeline::new(quux(&copy, id));
                (halt(&mut e, LIMIT), e.machine().clone())
            }
        };
        eprintln!("{name}: {at:?}");
        assert_eq!(at, Some(Halt { pc: ERROR_NO_GPT, stopped: true }), "{name}: at ERROR-NO-GPT");
        let log = m.block_disk.as_mut().unwrap().log.take().unwrap();
        let page: Vec<Transfer> =
            (0..4).map(|block| Transfer { write: false, block, page: 0o6000 }).collect();
        assert_eq!(log, page, "{name}");
        assert!(m.store_log.unwrap().iter().all(|&a| a == CCW), "{name}: only the command word");
        assert!(same_bytes(&copy, &pack), "{name}: the pack unchanged");
    }
}
