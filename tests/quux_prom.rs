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
use muir::pipeline::Pipeline;

mod support;

/// Where QUUX's PROM starts.
const BASE: u16 = 0o36000;

/// A QUUX with `prom` at 36000, the RAM word `ram` at 6 and at 36005.
fn quux(prom: &[Insn], ram: Insn) -> Machine {
    let mut m = support::quux_16();
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

/// Both engines booted and run `steps` microcycles, `rtl` the pipeline;
/// M 1 to M 3 and the PCs each executed.
fn run(m: Machine, steps: usize) -> [([Word; 3], Vec<u16>); 2] {
    let mut e = Micro::new(m.clone());
    e.boot();
    let mut pe = Vec::new();
    for _ in 0..steps {
        pe.push(e.pc());
        e.step().unwrap();
    }
    let mut r = Pipeline::new(m);
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

/// PROM 2002's SHA-256, as muir-sys's pre-release `handover-2002-2999694`
/// gives it for its `promh.mcr` (`data/README.md`).
const PROM_2002_SHA256: &str = "3d93319a8e851f5f14a2fcd66cd1ed21abb33049c355edf8cc893c791fb4b124";

/// **The built-in PROM is PROM 2002**: `data/quux-promh.mcr` has the
/// hand-over's digest (`tests/system_2002.rs` holds it to the hand-over's
/// file itself, where that is fetched), reads as QUUX's `.mcr` with
/// section 6 saying 15, and is what `quux` loads: its first word at 36000
/// is the file's first control-store word. **Fails** another file in
/// `data/`, or a built-in PROM that is not it.
#[test]
fn the_built_in_prom_is_prom_2002() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/data/quux-promh.mcr");
    let out = std::process::Command::new("sha256sum").arg(path).output().expect("sha256sum");
    let sum = String::from_utf8_lossy(&out.stdout);
    assert_eq!(sum.split_whitespace().next(), Some(PROM_2002_SHA256));
    let bytes = std::fs::read(path).unwrap();
    let mcr = muir::mcr::parse_quux(&bytes, muir::mcr::Holds::Prom).unwrap();
    assert_eq!(mcr.hardware_revision, Some(15), "section 6");
    assert_eq!(mcr.imem_start, BASE as u32, "assembled at 36000");
    let built_in = muir::prom::quux_boot_prom();
    assert_eq!(built_in[0], mcr.imem[0], "the built-in PROM's first word");
    assert_eq!(built_in[..mcr.imem.len()], mcr.imem[..], "word for word");
}
