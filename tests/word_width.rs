// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **The word width as a parameter of the engines** (contract G2 §8.1):
//! `micro`, `rtl`, the machine, main memory and the checkpoint carry a word
//! of 32 or 40 bits, as [`Geometry::word_bits`] says. The CADR is 32;
//! QUUX, revision 13, [`Geometry::QUUX`], is 40.
//!
//! Revision 13's geometry runs a hand program that moves words with `<39:32>` set through every
//! word register G2 §2.1 widens: main memory into `MD`, `MD` into M, M
//! into A, A back into M on the A pass-around, M into M on the M
//! pass-around, `Q`, the PDL buffer, `VMA`, and `MD` back into main memory
//! at an address whose `<39:32>` are set too. Both engines must leave
//! every word whole, and so must a checkpoint taken at any microcycle of
//! the run and resumed.
//!
//! What the ALU does to `<39:32>` is G2 §2.2's output rule: a logical
//! function acts on all 40 bits, and an arithmetic one leaves M's. The
//! program uses `SETM` and `SETA`, which are logical, to move the words, and
//! one `XOR` and one `ADD` to show the rule. The rest of revision 13's
//! datapath, its fields, rotator, conditions and map, is
//! `tests/revision_13.rs`'s.

use muir::checkpoint::{Reader, Writer};
use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{
    ADD, ALU, ALWAYS, JUMP, MD, N, Q_LOAD, SETA, SETM, SRC_MD, SRC_Q, START_READ, START_WRITE, VMA,
    XOR, a_dest, a_src, filler, m_dest, m_src, src, target,
};
use muir::machine::{Geometry, Machine, Word};
use muir::micro::Micro;
use muir::rtl::Rtl;

mod support;

/// QUUX's 40-bit geometry, revision 13.
const WIDE: Geometry = Geometry::QUUX;

/// The words the program moves, each with `<39:32>` set, and `W1`'s and
/// `W3`'s `<31>` too so that a sign extension would show. `W2`'s `<31:0>`
/// is an address on the mapped page, so that it can stand in `VMA` for a
/// store: revision 13's map translates `<27:0>`, ignores `<39:32>` and
/// faults on `<31:28>` (contract G2 §2.6).
const W1: Word = 0xa5 << 32 | 0x8000_0001;
const W2: Word = 0x3c << 32 | 0o411;
const W3: Word = 0xc3 << 32 | 0x8765_4321;

/// Where the words are in main memory, on page 0, which the map sends to
/// physical page 0.
const IN_W1: u32 = 0o400;
const IN_W2: u32 = 0o401;
const IN_W3: u32 = 0o402;
const OUT_W1: u32 = 0o410;
const OUT_W3: u32 = 0o411;

/// The last instruction, a jump to itself.
const STOP: u16 = 0o60;

/// A functional destination, with M's address 37 as the scratch word.
fn fd(code: u64) -> u64 {
    code << 19 | 0o37 << 14
}

/// The program: each line says what it moves where. A read's word is in
/// `MD` two microcycles after its start on `micro`, so `MD` is read two
/// microcycles on.
fn program() -> Vec<Insn> {
    let p: Vec<(u64, &str)> = vec![
        (ALU | SETA | a_src(0o40) | START_READ, "VMA <- 400, read: MD <- W1"),
        (filler().raw(), ""),
        (filler().raw(), ""),
        (ALU | SETM | SRC_MD | m_dest(1), "M1 <- MD"),
        (ALU | SETM | m_src(1) | a_dest(0o61), "A61 <- M1, on the M pass-around"),
        (ALU | SETA | a_src(0o61) | m_dest(2), "M2 <- A61, on the A pass-around"),
        (ALU | SETM | m_src(2) | Q_LOAD | m_dest(3), "Q and M3 <- M2, on the M pass-around"),
        (ALU | SETM | SRC_Q | m_dest(4), "M4 <- Q"),
        (ALU | SETM | m_src(4) | fd(0o11), "push M4 on the PDL buffer"),
        (filler().raw(), ""),
        (ALU | SETM | src(0o25) | m_dest(5), "M5 <- the PDL buffer at the pointer"),
        (ALU | SETA | a_src(0o41) | START_READ, "VMA <- 401, read: MD <- W2"),
        (filler().raw(), ""),
        (filler().raw(), ""),
        (ALU | SETM | SRC_MD | a_dest(0o62), "A62 <- MD"),
        (ALU | SETM | SRC_MD | VMA, "VMA <- MD, W2"),
        (ALU | SETM | src(0o10) | m_dest(6), "M6 <- VMA"),
        (ALU | SETA | a_src(0o42) | START_READ, "VMA <- 402, read: MD <- W3"),
        (filler().raw(), ""),
        (filler().raw(), ""),
        (ALU | SETM | SRC_MD | m_dest(7), "M7 <- MD"),
        (ALU | SETM | m_src(5) | MD, "MD <- M5, W1"),
        (ALU | SETA | a_src(0o43) | START_WRITE, "VMA <- 410, write MD"),
        (filler().raw(), ""),
        (filler().raw(), ""),
        (ALU | SETM | m_src(7) | MD, "MD <- M7, W3"),
        (ALU | SETM | m_src(6) | START_WRITE, "VMA <- M6, W2, whose <23:0> is 411: write MD"),
        (filler().raw(), ""),
        (filler().raw(), ""),
        (ALU | XOR | m_src(1) | a_src(0o62) | m_dest(0o10), "M10 <- M1 xor A62"),
        (ALU | ADD | m_src(1) | a_src(0o62) | m_dest(0o11), "M11 <- M1 + A62"),
        (ALU | SETA | a_src(0o43) | START_READ, "VMA <- 410, read"),
        (filler().raw(), ""),
        (filler().raw(), ""),
        (ALU | SETM | SRC_MD | m_dest(0o12), "M12 <- MD, W1 back"),
        (ALU | SETA | a_src(0o44) | START_READ, "VMA <- 411, read"),
        (filler().raw(), ""),
        (filler().raw(), ""),
        (ALU | SETM | SRC_MD | m_dest(0o13), "M13 <- MD, W3 back"),
    ];
    let mut prom: Vec<Insn> = p.iter().map(|&(w, _)| Insn::new(w)).collect();
    prom.resize(STOP as usize, filler());
    prom.push(Insn::new(JUMP | target(STOP as u64) | ALWAYS | N));
    prom.resize(1024, filler());
    prom
}

/// The machine the program runs on, with its words in main memory and its
/// addresses in A memory, and the page they are on mapped to itself,
/// readable and writable: on revision 13 page 0 of 1024 words, its
/// level-2 entry's access bits `<27:26>` (appendix A1.7); on the CADR
/// page 1 of 256, `<23:22>`.
fn machine(geometry: Geometry) -> Machine {
    let mut m = Machine::new();
    m.geometry = geometry;
    m.load_prom(&program());
    support::prom_program_in_ram(&mut m);
    m.main[IN_W1 as usize] = W1;
    m.main[IN_W2 as usize] = W2;
    m.main[IN_W3 as usize] = W3;
    m.amem[0o40] = IN_W1 as Word;
    m.amem[0o41] = IN_W2 as Word;
    m.amem[0o42] = IN_W3 as Word;
    m.amem[0o43] = OUT_W1 as Word;
    m.amem[0o44] = OUT_W3 as Word;
    if geometry.wide() {
        m.l2_map[0] = (1 << 27) | (1 << 26);
    } else {
        m.l2_map[1] = (1 << 23) | (1 << 22) | 1;
    }
    m
}

/// Runs to the stop, and eight microcycles on, for what the last ones
/// started to land.
fn finish<E: Engine>(e: &mut E) {
    for _ in 0..2000 {
        if e.machine().opc == STOP {
            e.run(8);
            return;
        }
        e.step().unwrap();
    }
    panic!("the program never reached its end");
}

/// The words the program leaves, named.
fn words(m: &Machine) -> Vec<(&'static str, Word)> {
    vec![
        ("M1, MD read from main memory", m.mmem[1]),
        ("A61, written from M on the M pass-around", m.amem[0o61]),
        ("M2, from A on the A pass-around", m.mmem[2]),
        ("M3", m.mmem[3]),
        ("Q", m.q),
        ("M4, from Q", m.mmem[4]),
        ("the PDL buffer's word", m.pdl[m.pdl_pointer as usize]),
        ("M5, from the PDL buffer", m.mmem[5]),
        ("A62, from MD", m.amem[0o62]),
        ("M6, from VMA", m.mmem[6]),
        ("M7", m.mmem[7]),
        ("M10, M1 xor A62", m.mmem[0o10]),
        ("M11, M1 + A62", m.mmem[0o11]),
        ("main memory 410", m.main[OUT_W1 as usize]),
        ("main memory 411, stored at VMA with <39:32> set", m.main[OUT_W3 as usize]),
        ("M12, main memory 410 read back", m.mmem[0o12]),
        ("M13, main memory 411 read back", m.mmem[0o13]),
        ("VMA", m.vma),
        ("MD", m.md),
    ]
}

/// What [`words`] must be on the 40-bit geometry.
fn expected() -> Vec<Word> {
    let low = |w: Word| w & 0xffff_ffff;
    vec![
        W1,
        W1,
        W1,
        W1,
        W1,
        W1,
        W1,
        W1,
        W2,
        W2,
        W3,
        W1 ^ W2,
        // Arithmetic on <31:0>, M's <39:32> (G2 §2.2).
        (W1 & 0xff << 32) | (low(W1) + low(W2)) & 0xffff_ffff,
        W1,
        W3,
        W1,
        W3,
        OUT_W3 as Word,
        W3,
    ]
}

fn check(engine: &str, m: &Machine, expect: &[Word]) {
    for ((what, got), want) in words(m).into_iter().zip(expect) {
        assert_eq!(got, *want, "{engine}: {what}: {got:#012x}, not {want:#012x}");
    }
}

/// **Both engines carry `<39:32>` through every word register** on the
/// 40-bit geometry.
#[test]
fn both_engines_carry_40_bit_words() {
    let mut e = Micro::new(machine(WIDE));
    e.boot();
    finish(&mut e);
    check("micro", e.machine(), &expected());
    let mut e = Rtl::new(machine(WIDE));
    e.boot();
    finish(&mut e);
    check("rtl", e.machine(), &expected());
}

/// A checkpoint of `e`, as the resume reads it: at the machine's width.
fn round_trip<E: Engine>(e: &E, mut into: E) -> E {
    let mut w = Writer::new();
    e.save(&mut w);
    let body = w.finish();
    into.load(&mut Reader::for_word_bits(&body, e.machine().geometry.word_bits)).unwrap();
    into
}

/// **A checkpoint keeps the 40-bit words**: taken at every microcycle of
/// the run, on both engines, and resumed into a fresh engine, it finishes
/// with the words an uninterrupted run leaves, so no register or latch on
/// the way, the machine's or the engine's, is written at 32 bits.
#[test]
fn a_checkpoint_keeps_40_bit_words_at_every_microcycle() {
    fn every<E: Engine>(name: &str, new: fn(Machine) -> E) {
        let expect = expected();
        let mut n = 0;
        loop {
            let mut e = new(machine(WIDE));
            e.boot();
            for _ in 0..n {
                e.step().unwrap();
            }
            let stopped = e.machine().opc == STOP;
            let mut fresh = new(machine(WIDE));
            fresh.boot();
            let mut back = round_trip(&e, fresh);
            finish(&mut back);
            check(&format!("{name}, resumed after {n} microcycles"), back.machine(), &expect);
            if stopped {
                break;
            }
            n += 1;
            assert!(n < 2000, "{name}: the program never reached its end");
        }
    }
    every("micro", Micro::new);
    every("rtl", Rtl::new);
}

/// **On a 32-bit machine, the CADR, the same program leaves 32-bit words**: main
/// memory holds `<31:0>` of what was put there, and nothing above bit 31
/// appears anywhere. So a 32-bit machine's words stay 32 bits, whatever the
/// type that holds them.
#[test]
fn a_32_bit_machine_keeps_32_bits() {
    let low = |w: Word| w & 0xffff_ffff;
    fn run<E: Engine>(mut e: E) -> Machine {
        let m = e.machine_mut();
        for w in m.main.iter_mut() {
            *w &= 0xffff_ffff;
        }
        e.boot();
        finish(&mut e);
        e.machine().clone()
    }
    let expect: Vec<Word> = expected()
        .into_iter()
        .enumerate()
        .map(|(k, w)| if k == 12 { (low(W1) + low(W2)) & 0xffff_ffff } else { low(w) })
        .collect();
    check("micro", &run(Micro::new(machine(Geometry::CADR))), &expect);
    check("rtl", &run(Rtl::new(machine(Geometry::CADR))), &expect);
}

/// **The CADR is 32 bits wide, and QUUX 40.**
#[test]
fn the_cadr_is_32_bits_wide_and_quux_40() {
    assert_eq!(Geometry::CADR.word_bits, 32);
    assert_eq!(Geometry::CADR.word_mask(), 0xffff_ffff);
    assert_eq!(WIDE.word_bits, 40);
    assert_eq!(WIDE.word_mask(), 0xff_ffff_ffff);
}

/// **A checkpoint file is written of a 40-bit machine too**: its header's
/// version says the width (`tests/revision_13_memory.rs` has the file).
#[test]
fn a_40_bit_machine_writes_a_checkpoint_file() {
    assert_eq!(machine(WIDE).checkpoint_refusal(), None);
    assert_eq!(machine(Geometry::CADR).checkpoint_refusal(), None);
}

/// **A 32-bit machine's checkpoint is the bytes it always was**: every
/// word four bytes, so that a checkpoint written before the width was a
/// parameter reads the same, and the 40-bit machine's is longer by a byte
/// a word, and by revision 13's larger dispatch memory and map and its
/// overflow flag (contract G2 §2.4, §2.6, §2.2).
#[test]
fn a_32_bit_checkpoint_writes_four_bytes_a_word() {
    let body = |g: Geometry| {
        let mut w = Writer::new();
        machine(g).save(&mut w);
        w.finish().len()
    };
    let words = 1024 + 32 + muir::machine::PDL_WORDS + 3 + muir::machine::MAIN_WORDS;
    let entries = (4096 - 2048) + (8192 - 2048) + (4096 - 2048);
    assert_eq!(
        body(WIDE) - body(Geometry::CADR),
        words + 1 + 1 + 4 * entries,
        "a byte a word, the width, the overflow flag, and 4 bytes a new entry"
    );
}
