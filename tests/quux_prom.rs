// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's boot PROM in its own addresses (contract Q2): 1K words at
//! control store 36000-37777, read only and never overlaid. Reset starts
//! the PC there; the microcode lives in 0-35777, which is RAM from the
//! start, and there is no PROM-disable bit. The CADR keeps MIT's overlay.

use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{ALU, ALWAYS, JUMP, N, SETO, filler, m_dest, target};
use muir::machine::{Geometry, Machine, Word};
use muir::micro::Micro;
use muir::rtl::Rtl;

mod support;

/// Where QUUX's PROM starts.
const BASE: u16 = 0o36000;

/// A QUUX with `prom` at 36000, the RAM word `ram` at 6 and at 36005.
fn quux(prom: &[Insn], ram: Insn) -> Machine {
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
    let mut words = vec![filler(); 1024];
    words[..prom.len()].copy_from_slice(prom);
    m.load_prom(&words);
    m.imem[6] = ram;
    m.imem[BASE as usize + 5] = ram;
    m
}

/// The PROM program: M 1 set, then a jump to 36005 (the PROM's own word
/// there sets M 2), then a jump to 6 in the RAM (whose word sets M 3).
fn program() -> Vec<Insn> {
    let mut p = vec![filler(); 8];
    p[0] = Insn::new(ALU | SETO | m_dest(1));
    p[1] = Insn::new(JUMP | target(BASE as u64 + 5) | ALWAYS | N);
    p[5] = Insn::new(ALU | SETO | m_dest(2));
    p[6] = Insn::new(JUMP | target(6) | ALWAYS | N);
    p
}

/// Both engines booted and run `steps` microcycles; M 1 to M 3 and the PCs
/// each executed.
fn run(m: Machine, steps: usize) -> [([Word; 3], Vec<u16>); 2] {
    let mut e = Micro::new(m.clone());
    e.boot();
    let mut pe = Vec::new();
    for _ in 0..steps {
        pe.push(e.pc());
        e.step().unwrap();
    }
    let mut r = Rtl::new(m);
    r.boot();
    let mut pr = Vec::new();
    for _ in 0..steps {
        pr.push(r.pc());
        r.step().unwrap();
    }
    let mm = |x: &Machine| [x.mmem[1], x.mmem[2], x.mmem[3]];
    [(mm(e.machine()), pe), (mm(r.machine()), pr)]
}

/// **QUUX starts at 36000, runs its PROM there, and reaches the RAM below
/// with no PROM-disable**: the first microcycle after the boot is 36000's;
/// at 36005 it runs the PROM's word, not the RAM's word at the same
/// address; and a jump to 6 runs the RAM's word at 6, though nothing
/// turned the PROM off.
#[test]
fn quux_boots_at_36000_and_the_ram_below_is_live() {
    let ram = Insn::new(ALU | SETO | m_dest(3));
    for (k, (m, pcs)) in run(quux(&program(), ram), 40).into_iter().enumerate() {
        let name = ["micro", "rtl"][k];
        let first = pcs.iter().position(|&p| p != 0).map(|i| pcs[i]);
        assert_eq!(first, Some(BASE), "{name}: the PC after the boot, {pcs:?}");
        let ones = Geometry::QUUX.word_mask();
        assert_eq!(m, [ones; 3], "{name}: the PROM's 0 and 5, then the RAM's 6");
    }
}

/// **The PROM is not written by `WRITE-I-MEM`**, nor by anything else: a
/// word stored at 36005 through the control store's write path stays the
/// RAM's, which QUUX never fetches there.
#[test]
fn quux_s_prom_is_read_only() {
    let mut m = quux(&program(), Insn::new(ALU | SETO | m_dest(3)));
    m.write_imem(BASE + 5, Insn::new(0));
    assert_eq!(m.fetch(BASE + 5), program()[5], "the PROM's word, still");
    assert_eq!(m.fetch(6), Insn::new(ALU | SETO | m_dest(3)), "the RAM below");
}

/// **The PROM-disable bit does nothing on QUUX**: set, the PROM still
/// answers at 36000 and the RAM at 0.
#[test]
fn quux_has_no_prom_disable() {
    let mut m = quux(&program(), Insn::new(0));
    m.imem[0] = Insn::new(ALU | SETO | m_dest(7));
    for disable in [false, true] {
        m.mode.prom_disable = disable;
        assert_eq!(m.fetch(BASE), program()[0], "36000, disable {disable}");
        assert_eq!(m.fetch(0), m.imem[0], "0, disable {disable}");
    }
}

/// **The CADR keeps MIT's overlay**: its PROM at 0 until `PROMDISABLE`,
/// the RAM there after, and 36000 always RAM.
#[test]
fn the_cadr_keeps_the_overlay() {
    let mut m = Machine::new();
    m.load_prom(&program());
    m.imem[0] = Insn::new(ALU | SETO | m_dest(7));
    m.imem[BASE as usize] = Insn::new(ALU | SETO | m_dest(6));
    assert_eq!(m.fetch(0), program()[0]);
    assert_eq!(m.fetch(BASE), m.imem[BASE as usize]);
    m.mode.prom_disable = true;
    assert_eq!(m.fetch(0), m.imem[0]);
}

/// Where 36000's `JUMP GO` goes: `GO`, at 36043, as PROM 2001's symbol
/// table `promh.sym` says, `GO I-MEM 36043`
/// ([`prom_2001_is_the_release_s`]).
const GO: u64 = 0o36043;

/// PROM 2001's last word: its `promh.locs` says `(I-MEM 36662)`, so the
/// code is at 36000-36661, and 36661 is its last halt,
/// `ERROR-A-MEM-SECTION-32-BITS`, where it stops on a microcode partition
/// without the 40-bit A-memory section (contract G2 §2.8).
const LAST_2001: usize = 0o36661;

/// PROM 2001's halts after its disk routines, as its `promh.tbl` and
/// `promh.sym` put them: no GPT header (or its entry array's LBA past 32
/// bits), no current microcode partition, one whose first LBA is odd, and
/// one without the 40-bit A-memory section.
const HALTS_2001: [(u64, &str); 4] = [
    (0o36653, "ERROR-NO-GPT"),
    (0o36655, "ERROR-NO-CURRENT-MICR"),
    (0o36657, "ERROR-ODD-MICR-START"),
    (0o36661, "ERROR-A-MEM-SECTION-32-BITS"),
];

/// **A QUUX PROM file, PROM 2001, is read from 36000, in partition order**
/// (contracts Q2 and Q8): the assembler writes the control store section
/// from 0, so the reader takes 36000-37777 of it and refuses a file with
/// anything assembled below 36000, or past 37777; and the file is QUUX's
/// `.mcr`, every word's two halves swapped from MIT's order, so a file in
/// MIT's order is refused saying so. 36000 is `JUMP GO`, and the code ends
/// at [`LAST_2001`]. Its sections hold MIT's values, in revision 13's
/// shapes: A memory as the 40-bit section (contract G2 appendix A1.12)
/// with MIT's words, and the dispatch memory of 4,096 entries whose first
/// 2,048 are MIT's; the main-memory section is four blocks, as MIT's; the
/// control store section from 0, the program QUUX's.
#[test]
fn prom_2001_is_read_from_36000_in_partition_order() {
    use muir::mcr::{parse, parse_partition_order, swap_halves};
    use muir::prom::parse_quux_mcr;
    const G: muir::machine::Geometry = muir::machine::Geometry::QUUX;
    let file = include_bytes!("../data/quux-promh.mcr");
    let words = parse_quux_mcr(file, G).unwrap();
    assert_eq!(words.len(), 1024);
    assert_eq!(words[0].raw() >> 43 & 3, 1, "a jump");
    assert_eq!(words[0].jump().target as u64, GO);
    let base = 0o36000;
    assert_ne!(words[LAST_2001 - base].raw(), 0, "the last word");
    assert!(words[LAST_2001 - base + 1..].iter().all(|w| w.raw() == 0), "nothing past it");
    let err = parse_quux_mcr(&swap_halves(file).unwrap(), G).unwrap_err();
    assert!(err.contains("MIT's order"), "{err}");
    // MIT's own PROM, which is in MIT's order and assembled at 0.
    let mit_file = include_bytes!("../mit/sys/ubin/promh.mcr");
    let err = parse_quux_mcr(mit_file, G).unwrap_err();
    assert!(err.contains("MIT's order"), "{err}");
    let err = parse_quux_mcr(&swap_halves(mit_file).unwrap(), G).unwrap_err();
    assert!(err.contains("QUUX's PROM is assembled at 36000"), "MIT's, at 0: {err}");
    assert_eq!(words, muir::prom::quux_boot_prom(), "the built-in PROM");
    let quux = parse_partition_order(file).unwrap();
    let mits = parse(mit_file).unwrap();
    assert!(quux.amem_wide, "A memory as the 40-bit section");
    assert_eq!(quux.amem, mits.amem, "A memory's values MIT's");
    assert_eq!(quux.dmem.len(), 4096, "revision 13's 4,096 dispatch entries");
    assert_eq!(quux.dmem[..2048], mits.dmem[..], "the first 2,048 MIT's");
    assert_eq!(quux.main_memory.map(|m| m.1), Some(4), "four blocks");
    assert_eq!(quux.imem_start, mits.imem_start, "the control store section from 0");
    assert_ne!(quux.imem, mits.imem, "the program is QUUX's");
}

/// **PROM 2001 is QUUX's release's, byte for byte**, where the release
/// (`release-2001`, fetched by `tools/fetch-system-for-quux.sh` into
/// `vendor/system-2001/`) is present: its `release-2001-promh.mcr` and its
/// sources' `sys/ubin/promh.mcr`, assembled from muir-sys's `promh.text`
/// at the release's commit; its symbols and error table say version 2001,
/// 3721 octal, and put `GO`, the end and the halts of [`HALTS_2001`] where
/// this file's constants say.
#[test]
fn prom_2001_is_the_release_s() {
    let Some(asset) = support::quux_release(&["release-2001-promh.mcr"]) else { return };
    let Some(dir) = support::quux_release(&["sys", "ubin"]) else { return };
    let ours = include_bytes!("../data/quux-promh.mcr");
    assert!(std::fs::read(&asset).unwrap() == ours, "the release's release-2001-promh.mcr");
    let bytes = std::fs::read(dir.join("promh.mcr")).unwrap();
    assert!(bytes == ours, "the release's sys/ubin/promh.mcr");
    let locs = std::fs::read_to_string(dir.join("promh.locs")).unwrap();
    assert!(locs.contains(&format!("(I-MEM {:o})", LAST_2001 + 1)), "{locs}");
    let tbl = std::fs::read_to_string(dir.join("promh.tbl")).unwrap();
    let sym = std::fs::read_to_string(dir.join("promh.sym")).unwrap();
    assert!(
        tbl.contains(&format!("MICROCODE-ERROR-TABLE-VERSION-NUMBER {:o})", 2001)),
        "version 2001 in promh.tbl"
    );
    assert!(sym.contains(&format!(" VERSION-NUMBER {:o} ", 2001)), "version 2001 in promh.sym");
    assert!(sym.contains(&format!("GO I-MEM {GO:o} ")), "GO in promh.sym");
    for (at, name) in HALTS_2001 {
        assert!(tbl.contains(&format!("({at:o} {name})")), "{name} at {at:o} in promh.tbl");
        assert!(sym.contains(&format!("{name} I-MEM {at:o} ")), "{name} at {at:o} in promh.sym");
    }
}
