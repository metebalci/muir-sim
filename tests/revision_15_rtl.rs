// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **QUUX on `rtl`: the pipeline** (contract G3 revision 15, §3, §5, §6,
//! §9, §12.1; appendix A15b; its MP2b rulings), on revision 16, held to
//! `micro`, the specification, on hand-built programs, and to the clock
//! counts the contract gives each row. Three tests keep revisions 14 and
//! 15's own: revision 15's checkpoint versions, and revision 14's engines.
//!
//! Each test says the partial implementation it fails. Programs run from
//! control store 0, the PROM's first word jumping there; the words are
//! 64-bit, the data 40-bit. Addresses are octal, as A15b writes them.

use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{
    ALU, ALWAYS, DISPATCH, JUMP, LDB, MD, N, P, POPJ, R, SETA, SETM, SRC_MD, START_READ,
    START_WRITE, a_dest, a_src, filler, m_dest, m_src, src, target,
};
use muir::machine::{Geometry, Halt, Machine, Word};
use muir::micro::Micro;
use muir::pipeline::{Mutation, Pipeline};

mod support;

/// Revision 16, the machine the pipeline runs.
const REV16: Geometry = Geometry::QUUX_16;

/// A memory's constants 0 and 1, and M memory's.
const ZERO: u64 = 0o40;
const ONE: u64 = 0o41;
const M_ZERO: u64 = 0o34;
const M_ONE: u64 = 0o35;

/// The program's last word, a jump to itself.
const STOP: u64 = 0o1000;

/// The register page's virtual address on revisions 14 and 15.
const REGISTER_PAGE: Word = 0o35777777400;

/// The physical memory window: main memory at `VA<27:0>`.
const PHYS: Word = 0o36000000000;

/// A functional destination, M 36 as the scratch word.
const fn fd(code: u64) -> u64 {
    code << 19 | 0o36 << 14
}

/// A BYTE word.
fn byte(func: u64, rotate: u64, len: u64) -> u64 {
    muir::isa::asm::BYTE | func | (len - 1) << 6 | rotate
}

/// A JUMP on condition `code`, `IR<4:0>` with `IR<5>`.
fn jcond(code: u64) -> u64 {
    JUMP | 1 << 5 | code
}

/// A DISPATCH at `IR<23:12>`, no bits of M.
fn disp(addr: u64) -> u64 {
    DISPATCH | addr << 12
}

/// A program: 64-bit words, and the A, M and dispatch memories' words.
#[derive(Default, Clone)]
struct Prog {
    words: Vec<u64>,
    amem: Vec<(u64, Word)>,
    mmem: Vec<(u64, Word)>,
    dmem: Vec<(usize, u32)>,
    main: Vec<(usize, Word)>,
    next_a: u64,
}

impl Prog {
    fn op(&mut self, w: u64) -> &mut Self {
        self.words.push(w);
        self
    }
    fn at(&self) -> u64 {
        self.words.len() as u64
    }
    fn fill(&mut self, n: usize) -> &mut Self {
        for _ in 0..n {
            self.op(filler().raw());
        }
        self
    }
    /// An A-memory constant, from 101 up: its address.
    fn k(&mut self, v: Word) -> u64 {
        let a = 0o101 + self.next_a;
        assert!(a < 0o1700, "A memory's constants run into the scratch words");
        self.next_a += 1;
        self.amem.push((a, v));
        a
    }
    /// M `slot` <- the A constant `v`.
    fn set(&mut self, v: Word, slot: u64) -> &mut Self {
        let a = self.k(v);
        self.op(ALU | SETA | a_src(a) | m_dest(slot))
    }
    /// M `slot` <- the word at the virtual address `va`.
    fn read(&mut self, va: Word, slot: u64) -> &mut Self {
        let a = self.k(va);
        self.op(ALU | SETA | a_src(a) | START_READ);
        self.fill(1);
        self.op(ALU | SETM | SRC_MD | m_dest(slot))
    }
    /// The word `word` to the virtual address `va`.
    fn write(&mut self, word: Word, va: Word) -> &mut Self {
        let (wa, aa) = (self.k(word), self.k(va));
        self.op(ALU | SETA | a_src(wa) | MD);
        self.op(ALU | SETA | a_src(aa) | START_WRITE);
        self.fill(2)
    }
    /// Jumps to the stop.
    fn stop(&mut self) -> &mut Self {
        self.op(JUMP | target(STOP) | ALWAYS | N);
        self.op(filler().raw())
    }
}

/// The machine of `geometry` for `p`.
fn machine(p: &Prog, geometry: Geometry) -> Machine {
    let mut prom: Vec<Insn> = p.words.iter().map(|&w| Insn::extended(w)).collect();
    assert!(prom.len() <= STOP as usize, "the program runs into its stop");
    prom.resize(STOP as usize, filler());
    prom.push(Insn::new(JUMP | target(STOP) | ALWAYS | N));
    prom.resize(1024, filler());
    // 64K words of main memory: the programs use the low words.
    let mut m = Machine::with_geometry(geometry, 1);
    m.load_prom(&prom);
    support::prom_program_in_ram(&mut m);
    m.amem[ZERO as usize] = 0;
    m.amem[ONE as usize] = 1;
    for (k, v) in [(M_ZERO, 0), (M_ONE, 1)] {
        m.mmem[k as usize] = v;
        m.amem[k as usize] = v;
    }
    for &(a, v) in &p.amem {
        m.amem[a as usize] = v;
    }
    for &(k, v) in &p.mmem {
        m.mmem[k as usize] = v;
        m.amem[k as usize] = v;
    }
    for &(k, v) in &p.dmem {
        m.dmem[k] = v;
    }
    for &(k, v) in &p.main {
        m.main[k] = v;
    }
    m
}

/// The pipeline on from its stop until its memory side is quiet, so that its
/// state compares with micro's, whose accesses land at once.
fn settled(mut e: Pipeline) -> Result<Pipeline, (Halt, Box<Pipeline>)> {
    for _ in 0..10_000 {
        if e.quiet() {
            break;
        }
        if let Err(h) = e.step() {
            return Err((h, Box::new(e)));
        }
    }
    Ok(e)
}

/// Runs an engine to the stop and 16 microcycles on.
fn run_engine<E: Engine>(mut e: E) -> Result<E, (Halt, Box<E>)> {
    e.boot();
    for _ in 0..200_000 {
        if e.machine().opc == STOP as u16 {
            for _ in 0..16 {
                if let Err(h) = e.step() {
                    return Err((h, Box::new(e)));
                }
            }
            return Ok(e);
        }
        if let Err(h) = e.step() {
            return Err((h, Box::new(e)));
        }
    }
    panic!("the program never reached its stop: PC {:o}", e.pc());
}

fn micro(p: &Prog) -> Micro {
    match run_engine(Micro::new(machine(p, REV16))) {
        Ok(u) => u,
        Err((h, _)) => panic!("micro halted: {h:?}"),
    }
}

fn pipeline_with(p: &Prog, set: impl FnOnce(&mut Pipeline)) -> Pipeline {
    let mut e = Pipeline::new(machine(p, REV16));
    set(&mut e);
    match run_engine(e).and_then(settled) {
        Ok(e) => e,
        Err((h, _)) => panic!("the pipeline halted: {h:?}"),
    }
}

fn pipeline(p: &Prog) -> Pipeline {
    pipeline_with(p, |_| {})
}

/// The architectural state two engines must agree on.
fn state(m: &Machine) -> impl PartialEq + std::fmt::Debug {
    (
        (m.mmem, m.amem, m.pdl[..512].to_vec(), m.spc, m.spcptr),
        (m.pdl_pointer, m.pdl_index, m.q, m.vma, m.md, m.lc, m.interrupt_control),
        (m.dmem[..512].to_vec(), m.main[..4096].to_vec()),
    )
}

/// `p` ends the same on both engines.
fn same(p: &Prog) -> Pipeline {
    let u = micro(p);
    let e = pipeline(p);
    assert_eq!(state(e.machine()), state(u.machine()), "the pipeline against micro");
    assert_eq!(e.machine().posted_write_errors, 0, "word 225 stays 0");
    e
}

#[test]
fn straight_line_alu_words_end_as_on_micro() {
    let mut p = Prog::default();
    p.set(5, 0o20).set(7, 0o21);
    // Each word reads the one before's result: d1, d2, d3.
    p.op(ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(0o21) | m_dest(0o22));
    p.op(ALU | muir::isa::asm::ADD | a_src(0o22) | m_src(0o22) | m_dest(0o23));
    p.op(ALU | muir::isa::asm::ADD | a_src(0o22) | m_src(0o23) | m_dest(0o24));
    p.op(ALU | muir::isa::asm::ADD | a_src(0o22) | m_src(0o24) | m_dest(0o25));
    p.op(ALU | muir::isa::asm::ADD | a_src(0o23) | m_src(0o25) | a_dest(0o200));
    p.op(ALU | SETA | a_src(0o200) | m_dest(0o26));
    p.stop();
    let e = same(&p);
    assert_eq!(e.machine().mmem[0o26], 16 + 32);
    differs(&p, Mutation::NoD3);
    differs(&p, Mutation::NoD2);
    differs(&p, Mutation::NoD1);
}

/// `p` ends otherwise than on `micro` with `mutation` planted: the check
/// catches it.
fn differs(p: &Prog, mutation: Mutation) {
    let u = micro(p);
    let mut e = Pipeline::new(machine(p, REV16));
    e.mutation = mutation;
    match run_engine(e).and_then(settled) {
        Ok(e) => assert_ne!(
            state(e.machine()),
            state(u.machine()),
            "{mutation:?} planted, and the program ends as on micro"
        ),
        Err((h, _)) => eprintln!("{mutation:?} planted: the pipeline halted at {h:?}"),
    }
}

#[allow(dead_code)]
fn _uses() {
    let _ = (DISPATCH, P, POPJ, R, src(0), disp(0), jcond(0), REGISTER_PAGE, PHYS, fd(0));
    let _ = Mutation::None;
}

/// Conditional jumps hinted each way, taken and not, with N and without:
/// every combination ends as on `micro`.
#[test]
fn conditional_jumps_hinted_each_way_end_as_on_micro() {
    use muir::isa::asm::HINT;
    let mut p = Prog::default();
    let mut slot = 0o20;
    for hint in [0, HINT] {
        for m in [M_ZERO, M_ONE] {
            for n in [0, N] {
                // Taken when M = 0 (A 0 equal).
                p.op(byte(LDB, 0, 40) | m_src(M_ONE) | m_dest(slot));
                let next = p.at() + 2;
                p.op(jcond(3) | m_src(m) | a_src(ZERO) | target(next + 1) | hint | n);
                // The delay slot: counts.
                p.op(ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(slot) | m_dest(slot));
                p.op(ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(slot) | m_dest(slot));
                // The target.
                p.op(ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(slot) | m_dest(slot));
                slot += 1;
            }
        }
    }
    p.stop();
    let e = same(&p);
    assert_eq!(e.meters.rd_disagreements, 0);
    assert!(e.meters.mispredicted[0] > 0, "some hints were wrong");
}

/// Calls, returns, a POPJ after the next word, a jump with R: as `micro`.
#[test]
fn calls_and_returns_end_as_on_micro() {
    let mut p = Prog::default();
    let sub1 = 0o400;
    let sub2 = 0o420;
    p.set(1, 0o20);
    // Call with N (return to the delay slot's address), and without.
    p.op(JUMP | ALWAYS | P | N | target(sub1));
    p.op(ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(0o20) | m_dest(0o20));
    p.op(JUMP | ALWAYS | P | target(sub1));
    p.op(ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(0o20) | m_dest(0o20));
    p.op(ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(0o20) | m_dest(0o20));
    // A nested call.
    p.op(JUMP | ALWAYS | P | N | target(sub2));
    p.fill(1);
    p.op(ALU | SETA | a_src(ZERO) | m_dest(0o23));
    p.stop();
    while p.at() < sub1 {
        p.fill(1);
    }
    // sub1: count in M 21, POPJ after the next word.
    p.op(ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(0o21) | m_dest(0o21) | POPJ);
    p.op(ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(0o21) | m_dest(0o21));
    while p.at() < sub2 {
        p.fill(1);
    }
    // sub2: calls sub1, then returns by a jump with R.
    p.op(JUMP | ALWAYS | P | N | target(sub1));
    p.fill(1);
    p.op(ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(0o22) | m_dest(0o22));
    p.op(JUMP | ALWAYS | R | N);
    p.fill(1);
    let e = same(&p);
    assert_eq!(e.meters.rd_disagreements, 0);
}

/// Dispatches predicted as each of A15b.2's four rows, right and wrong by
/// address, by P and by R: as `micro`.
#[test]
fn dispatches_predicted_each_way_end_as_on_micro() {
    use muir::isa::asm::predicted;
    let mut p = Prog::default();
    let sub = 0o600;
    let jump_to = 0o640;
    let back = 0o660;
    // Entries: 100 jump, 101 call, 102 drop-through, 103 return, 104 jump
    // with N.
    p.dmem.push((0o100, jump_to as u32));
    p.dmem.push((0o101, sub as u32 | 1 << 15));
    p.dmem.push((0o102, 1 << 16 | 1 << 15));
    p.dmem.push((0o103, 1 << 16));
    p.dmem.push((0o104, back as u32 | 1 << 14));
    p.set(0, 0o20);
    let preds = [
        (0u64, false, false),
        (jump_to, false, false),
        (sub, true, false),
        (0, true, true),
        (0, false, true),
        (0o777, false, false),
    ];
    for (k, &(addr, pp, rr)) in preds.iter().enumerate() {
        for entry in 0o100..=0o102u64 {
            // A call whose return comes back here, for the return entry.
            let here = p.at();
            let _ = here;
            p.op(disp(entry) | predicted(addr, pp, rr));
            p.op(ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(0o20) | m_dest(0o20));
            if entry == 0o100 {
                // jump_to returns here by a call/return pair: emulate with a
                // jump back.
                p.dmem.push((0o110 + k, p.at() as u32));
            }
        }
    }
    p.stop();
    while p.at() < sub {
        p.fill(1);
    }
    p.op(ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(0o21) | m_dest(0o21) | POPJ);
    p.fill(1);
    while p.at() < jump_to {
        p.fill(1);
    }
    p.op(ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(0o22) | m_dest(0o22));
    p.op(JUMP | ALWAYS | N | target(STOP));
    p.fill(1);
    let _ = same(&p);
}

// --- Programs made at random, rule-abiding -----------------------------------

/// A fixed sequence of numbers per seed: xorshift64*.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0.max(1);
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 16
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, k: u64, of: u64) -> bool {
        self.below(of) < k
    }
}

/// The subroutines' addresses.
const SUBS: [u64; 4] = [0o700, 0o720, 0o740, 0o760];

/// What a random program may use beyond data, branches, calls, dispatches
/// and memory.
const DATA_PUSH: u32 = 1;
const PDL_FIELD: u32 = 2;
const OA: u32 = 4;
const MULDIV: u32 = 8;
const REGISTERS: u32 = 16;
const FAULTS: u32 = 32;
const DMEM_WRITES: u32 = 64;
const ALL: u32 = 127;

/// **A program at random** that keeps MIT's microcode rules, so that
/// `micro` and `rtl` must end alike: forward branches only, calls to
/// subroutines that return, no start in a delay slot, `MD` read two words
/// after its read's start at the soonest.
fn random_program(seed: u64, features: u32) -> Prog {
    use muir::isa::asm::{ADD, AND, HINT, IOR, Q_LOAD, SUB, XOR, predicted};
    let mut rng = Rng(seed);
    let mut p = Prog::default();
    // Scratch: M 20-27, A 1700-1707; constants in A 101 up.
    let consts: Vec<u64> = (0..8).map(|k| p.k(rng.next() & 0o7777777777 | (k << 32))).collect();
    for (k, &c) in consts.iter().enumerate().take(4) {
        p.op(ALU | SETA | a_src(c) | m_dest(0o20 + k as u64));
    }
    // The PDL buffer's registers somewhere in its middle.
    let pdl_base = p.k(0o1000);
    p.op(ALU | SETA | a_src(pdl_base) | fd(0o14));
    p.op(ALU | SETA | a_src(pdl_base) | fd(0o13));
    let body_end = 0o600;
    let funcs = [ADD, SUB, AND, IOR, XOR, SETA, SETM];
    let m_srcs = |rng: &mut Rng| -> u64 {
        match rng.below(10) {
            0 => src(0o7),  // Q
            1 => src(0o3),  // PDL-INDEX
            2 => src(0o2),  // the PDL pointer
            3 => src(0o5),  // PDL by the index
            4 => src(0o25), // PDL by the pointer
            5 => src(0o24), // pop
            6 => src(0o10), // VMA
            _ => m_src(0o20 + rng.below(8)),
        }
    };
    let a_srcs = |rng: &mut Rng| -> u64 {
        if rng.chance(1, 2) {
            a_src(0o1700 + rng.below(8))
        } else {
            a_src(consts[rng.below(8) as usize])
        }
    };
    let dests = |rng: &mut Rng| -> u64 {
        match rng.below(12) {
            0 => fd(0o11),             // PDL push
            1 => fd(0o10),             // PDL top
            2 => fd(0o12),             // PDL by the index
            3 => fd(0o13) | 0o1 << 14, // PDL-INDEX, M 1
            4 => fd(0o20),             // VMA
            5 | 6 => a_dest(0o1700 + rng.below(8)),
            _ => m_dest(0o20 + rng.below(8)),
        }
    };
    let data = |rng: &mut Rng, p: &mut Prog| {
        let w = if rng.chance(1, 4) {
            byte(
                if rng.chance(1, 2) { LDB } else { muir::isa::asm::DPB },
                rng.below(40),
                1 + rng.below(39),
            ) | m_srcs(rng)
                | a_srcs(rng)
                | dests(rng)
        } else {
            ALU | funcs[rng.below(funcs.len() as u64) as usize]
                | m_srcs(rng)
                | a_srcs(rng)
                | dests(rng)
                | if rng.chance(1, 8) { Q_LOAD } else { 0 }
        };
        p.op(w);
    };
    // Dispatch tables: jump forward entries are filled per site below.
    let mut table = 0o200usize;
    let mut last_transfer = false;
    // Every construct's first word, which forward branches are moved to,
    // so that none lands inside a pair that must run whole; and what
    // names a forward target: JUMP words, dispatch entries, A constants.
    let mut boundaries = Vec::new();
    let mut jumps = Vec::new();
    let mut entries = Vec::new();
    let mut constants = Vec::new();
    while p.at() < body_end - 24 {
        let here = p.at();
        boundaries.push(here);
        let kind = rng.below(25);
        let fwd = |rng: &mut Rng| here + 3 + rng.below(8);
        match kind {
            // A conditional jump forward, hinted at random, N or not.
            0..=2 if !last_transfer => {
                let cond = match rng.below(4) {
                    0 => 1 << 5 | 3,
                    1 => 1 << 5 | 1,
                    2 => 1 << 5 | 2,
                    _ => rng.below(40) & 0o37,
                };
                let hint = if rng.chance(1, 2) { HINT } else { 0 };
                let n = if rng.chance(1, 2) { N } else { 0 };
                jumps.push(p.words.len());
                p.op(JUMP
                    | cond
                    | m_srcs(&mut rng)
                    | a_srcs(&mut rng)
                    | target(fwd(&mut rng))
                    | hint
                    | n);
                last_transfer = true;
                continue;
            }
            // A call, a jump.
            3 if !last_transfer => {
                let sub = SUBS[rng.below(4) as usize];
                let n = if rng.chance(1, 2) { N } else { 0 };
                p.op(JUMP | ALWAYS | P | target(sub) | n);
                last_transfer = true;
                continue;
            }
            4 if !last_transfer => {
                let n = if rng.chance(1, 2) { N } else { 0 };
                jumps.push(p.words.len());
                p.op(JUMP | ALWAYS | target(fwd(&mut rng)) | n);
                last_transfer = true;
                continue;
            }
            // A dispatch on M's low two bits into a table of four.
            5 if !last_transfer => {
                for k in 0..4 {
                    let e = match rng.below(4) {
                        0 => 1 << 16 | 1 << 15,
                        1 => fwd(&mut rng) as u32,
                        2 => SUBS[rng.below(4) as usize] as u32 | 1 << 15,
                        _ => fwd(&mut rng) as u32 | 1 << 14,
                    };
                    if e & 1 << 15 == 0 {
                        entries.push(p.dmem.len());
                    }
                    p.dmem.push((table + k, e));
                }
                let pr = match rng.below(5) {
                    0 => predicted(0, false, false),
                    1 => predicted(fwd(&mut rng), false, false),
                    2 => predicted(SUBS[rng.below(4) as usize], true, false),
                    3 => predicted(0o777, false, false),
                    _ => 0,
                };
                p.op(disp(table as u64) | 2 << 5 | m_src(0o20 + rng.below(8)) | pr);
                table += 4;
                last_transfer = true;
                continue;
            }
            // A read: its start, a word, then MD.
            6 if !last_transfer => {
                let a = p.k(PHYS | rng.below(64));
                p.op(ALU | SETA | a_src(a) | START_READ);
                data(&mut rng, &mut p);
                if rng.chance(1, 2) {
                    data(&mut rng, &mut p);
                }
                p.op(ALU | SETM | SRC_MD | m_dest(0o20 + rng.below(8)));
            }
            // A write: MD, its start, two words.
            7 if !last_transfer => {
                let a = p.k(PHYS | rng.below(64));
                p.op(ALU | SETM | m_src(0o20 + rng.below(8)) | MD);
                p.op(ALU | SETA | a_src(a) | START_WRITE);
                data(&mut rng, &mut p);
                data(&mut rng, &mut p);
            }
            // A return address pushed as data, and a POPJ to it.
            8 if !last_transfer && features & DATA_PUSH != 0 => {
                let k = rng.below(3);
                let popj_at = here + 1 + k;
                let to = popj_at + 2 + rng.below(3);
                let a = p.k(to);
                p.op(ALU | SETA | a_src(a) | fd(0o15));
                for _ in 0..k {
                    data(&mut rng, &mut p);
                }
                p.op(ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(0o27) | m_dest(0o27) | POPJ);
                data(&mut rng, &mut p);
                while p.at() < to {
                    p.fill(1);
                }
            }
            // A word pushed as data and popped by the functional source.
            9 if !last_transfer && features & DATA_PUSH != 0 => {
                p.op(ALU | SETM | m_src(0o20 + rng.below(8)) | fd(0o15));
                if rng.chance(1, 2) {
                    data(&mut rng, &mut p);
                }
                p.op(ALU | SETM | src(0o14) | m_dest(0o20 + rng.below(8)));
            }
            // The PDL address field: PDL-INDEX <- the pointer or the index
            // plus a constant, read through at once.
            10 if !last_transfer && features & PDL_FIELD != 0 => {
                let d = rng.below(40) as i64 - 20;
                let a = p.k((d as u64) & 0o377777777777);
                let (base, m) = if rng.chance(1, 2) { (2, src(0o2)) } else { (3, src(0o3)) };
                p.op(ALU
                    | ADD
                    | m
                    | a_src(a)
                    | fd(0o13)
                    | muir::isa::asm::pdl_field(base, d as i8));
                p.op(ALU | SETM | src(0o5) | m_dest(0o20 + rng.below(8)));
            }
            // OA-REG-HIGH and a word that selects it into its sources.
            11 if !last_transfer && features & OA != 0 => {
                let (aoff, moff) = (rng.below(8), rng.below(8));
                let a = p.k(aoff << 6 | moff);
                p.op(ALU | SETA | a_src(a) | fd(0o17));
                p.op(ALU
                    | ADD
                    | a_src(0o1700)
                    | m_src(0o20)
                    | m_dest(0o20 + rng.below(8))
                    | muir::isa::asm::OA_HIGH_SELECT);
            }
            // OA-REG-LOW and a jump that takes its target from it.
            12 if !last_transfer && features & OA != 0 => {
                let to = here + 4 + rng.below(6);
                let a = p.k(to << 12);
                constants.push((p.amem.len() - 1, 12));
                p.op(ALU | SETA | a_src(a) | fd(0o16));
                p.op(JUMP
                    | ALWAYS
                    | target(0)
                    | if rng.chance(1, 2) { N } else { 0 }
                    | muir::isa::asm::OA_LOW_SELECT);
                last_transfer = true;
                continue;
            }
            // MUL and DIV.
            13 if features & MULDIV != 0 => {
                let f = if rng.chance(1, 2) { 0o42 << 3 } else { 0o43 << 3 };
                p.op(ALU | f | m_srcs(&mut rng) | a_srcs(&mut rng) | m_dest(0o20 + rng.below(8)));
            }
            // A register's read: MACHINE-ID, the microsecond clock's
            // feature word.
            14 if !last_transfer && features & REGISTERS != 0 => {
                let a = p.k(REGISTER_PAGE | if rng.chance(1, 2) { 0 } else { 0o14 });
                p.op(ALU | SETA | a_src(a) | START_READ);
                data(&mut rng, &mut p);
                p.op(ALU | SETM | SRC_MD | m_dest(0o20 + rng.below(8)));
            }
            // A start that faults, and the check right after it.
            15 if !last_transfer && features & FAULTS != 0 => {
                let a = p.k(0o1000 + rng.below(64));
                let start = if rng.chance(1, 2) { START_READ } else { START_WRITE };
                if start == START_WRITE {
                    p.op(ALU | SETM | m_src(0o20) | MD);
                }
                p.op(ALU | SETA | a_src(a) | start);
                let n = if rng.chance(1, 2) { N } else { 0 };
                p.op(jcond(4) | P | target(SUBS[rng.below(4) as usize]) | n);
                last_transfer = true;
                continue;
            }
            // Two starts in a row: the second held until the first is
            // acknowledged.
            17 if !last_transfer => {
                let (a, b) = (p.k(PHYS | rng.below(64)), p.k(PHYS | rng.below(64)));
                let first = if rng.chance(1, 2) { START_READ } else { START_WRITE };
                if first == START_WRITE {
                    p.op(ALU | SETM | m_src(0o20 + rng.below(8)) | MD);
                }
                p.op(ALU | SETA | a_src(a) | first);
                p.op(ALU | SETA | a_src(b) | START_READ);
                data(&mut rng, &mut p);
                p.op(ALU | SETM | SRC_MD | m_dest(0o20 + rng.below(8)));
            }
            // A dispatch-memory write, and a dispatch that reads it next.
            16 if !last_transfer && features & DMEM_WRITES != 0 => {
                let entry = if rng.chance(1, 2) {
                    1 << 16 | 1 << 15
                } else {
                    (here + 4 + rng.below(4)) as u32
                };
                let a = p.k(entry as u64);
                if entry & 1 << 15 == 0 {
                    constants.push((p.amem.len() - 1, 0));
                }
                // A drop-through until written, for a branch past the write.
                p.dmem.push((table, 1 << 16 | 1 << 15));
                p.op(DISPATCH | muir::isa::asm::DMEM_WRITE | a_src(a) | (table as u64) << 12);
                p.op(disp(table as u64));
                table += 1;
                last_transfer = true;
                continue;
            }
            _ => data(&mut rng, &mut p),
        }
        last_transfer = false;
    }
    boundaries.push(p.at());
    while p.at() < body_end {
        p.fill(1);
    }
    p.op(ALU | SETA | a_src(ZERO) | m_dest(0o30));
    p.stop();
    // Forward targets moved to the next construct's first word.
    let to = |t: u64| boundaries.iter().copied().find(|&b| b >= t).unwrap_or(body_end);
    for k in jumps {
        let t = p.words[k] >> 12 & 0o37777;
        p.words[k] = p.words[k] & !(0o37777 << 12) | target(to(t));
    }
    for k in entries {
        let (adr, e) = p.dmem[k];
        p.dmem[k] = (adr, e & !0o37777 | to(u64::from(e & 0o37777)) as u32);
    }
    for (k, shift) in constants {
        let (adr, v) = p.amem[k];
        let t = (v >> shift) & 0o37777;
        p.amem[k] = (adr, v & !(0o37777 << shift) | to(t) << shift);
    }
    // The subroutines: a few words, POPJ after the next.
    for &sub in &SUBS {
        while p.at() < sub {
            p.fill(1);
        }
        for _ in 0..rng.below(5) {
            data(&mut rng, &mut p);
        }
        match rng.below(4) {
            // A conditional return, hinted at random; then the POPJ.
            0 => {
                let hint = if rng.chance(1, 2) { muir::isa::asm::HINT } else { 0 };
                p.op(jcond(rng.below(40) & 0o37)
                    | m_src(0o20 + rng.below(8))
                    | R
                    | hint
                    | if rng.chance(1, 2) { N } else { 0 });
                data(&mut rng, &mut p);
            }
            // A dispatch whose entries return or drop through.
            1 => {
                for k in 0..4 {
                    p.dmem.push((
                        table + k,
                        if rng.chance(1, 2) { 1 << 16 } else { 1 << 16 | 1 << 15 },
                    ));
                }
                let pr = if rng.chance(1, 2) { predicted(0, false, true) } else { 0 };
                p.op(disp(table as u64) | 2 << 5 | m_src(0o20 + rng.below(8)) | pr);
                table += 4;
                data(&mut rng, &mut p);
            }
            _ => {}
        }
        p.op(ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(0o27) | m_dest(0o27) | POPJ);
        data(&mut rng, &mut p);
    }
    p
}

/// The microcycles `micro` runs, an executed one's address and a nopped
/// one's `None`, to the stop.
type Traces = (Vec<Option<u16>>, Vec<(u16, u32, u32)>, Vec<[u64; 8]>);

fn micro_trace(p: &Prog) -> (Micro, Traces) {
    let mut u = Micro::new(machine(p, REV16));
    u.boot();
    let mut t = Vec::new();
    let mut ops = Vec::new();
    let mut regs = Vec::new();
    let w = |u: &Micro, lo: u8, hi: u8| u.spy_read(lo) as u32 | (u.spy_read(hi) as u32) << 16;
    for _ in 0..200_000 {
        if u.machine().opc == STOP as u16 {
            break;
        }
        u.step().unwrap_or_else(|h| panic!("micro halted: {h:?}"));
        t.push(u.executed());
        regs.push(muir::pipeline::registers_of(u.machine()));
        if let Some(pc) = u.executed() {
            use muir::spy::{A_HIGH, A_LOW, M_HIGH, M_LOW};
            ops.push((pc, w(&u, A_LOW, A_HIGH), w(&u, M_LOW, M_HIGH)));
        }
    }
    (u, (t, ops, regs))
}

fn oct(r: &[u64; 8]) -> String {
    r.iter().map(|v| format!("{v:o}")).collect::<Vec<_>>().join(" ")
}

/// `micro` and the pipeline on `p`: the same microcycles, the same state.
/// The pipeline's meters, for the coverage.
fn compare(p: &Prog, seed: u64) -> muir::pipeline::Meters {
    let (u, (ut, uops, uregs)) = micro_trace(p);
    let mut e = Pipeline::new(machine(p, REV16));
    e.trace = Some(Vec::new());
    e.operands = Some(Vec::new());
    e.registers = Some(Vec::new());
    e.boot();
    e.skip_sweep();
    for _ in 0..2_000_000 {
        if e.machine().opc == STOP as u16 {
            break;
        }
        e.step().unwrap_or_else(|h| panic!("seed {seed}: the pipeline halted: {h:?}"));
    }
    let et = e.trace.take().unwrap();
    let eops = e.operands.take().unwrap();
    let eregs = e.registers.take().unwrap();
    for (k, (a, b)) in uregs.iter().zip(&eregs).enumerate() {
        // `MD` lands when the memory answers on `rtl`, two microcycles
        // after the start on `micro`: compared where a word reads it.
        let (mut a, mut b) = (*a, *b);
        (a[5], b[5]) = (0, 0);
        if a != b {
            panic!(
                "seed {seed}: after microcycle {k} ({:?}): registers micro {}, rtl {}; before {}",
                ut.get(k).map(|o| o.map(|p| format!("{p:o}"))),
                oct(&a),
                oct(&b),
                uregs.get(k.wrapping_sub(1)).map_or(String::new(), oct)
            );
        }
    }
    for (k, (a, b)) in uops.iter().zip(&eops).enumerate() {
        let b = (b.0, b.1 as u32, b.2 as u32);
        if *a != b {
            let before: Vec<String> =
                uops[k.saturating_sub(8)..k].iter().map(|o| format!("{:o}", o.0)).collect();
            panic!(
                "seed {seed}: executed word {k} at {:o}: micro A {:o} M {:o}, rtl A {:o} M {:o}; after {}",
                a.0,
                a.1,
                a.2,
                b.1,
                b.2,
                before.join(" ")
            );
        }
    }
    // The pipeline counts the boot's first microcycle too.
    let n = ut.len().min(et.len());
    if let Some(k) = (0..n).find(|&k| ut[k] != et[k]) {
        panic!(
            "seed {seed}: microcycle {k} differs: micro {:?}, rtl {:?}; micro {:?}, rtl {:?}",
            ut[k],
            et[k],
            &ut[k.saturating_sub(4)..(k + 4).min(n)],
            &et[k.saturating_sub(4)..(k + 4).min(n)]
        );
    }
    assert_eq!(
        ut.len(),
        et.len(),
        "seed {seed}: microcycles to the stop; micro's last {:?}, rtl's last {:?}",
        &ut[ut.len().saturating_sub(6)..],
        &et[et.len().saturating_sub(6)..]
    );
    // Drained.
    for _ in 0..64 {
        e.step().unwrap();
    }
    let mut u = u;
    for _ in 0..64 {
        u.step().unwrap();
    }
    diff_state(e.machine(), u.machine(), seed);
    assert_eq!(
        e.machine().cycles,
        e.meters.retired,
        "seed {seed}: Machine::cycles counts each microcycle once"
    );
    e.meters
}

/// The first differences between two machines' states, or nothing.
fn diff_state(e: &Machine, u: &Machine, seed: u64) {
    let mut out = Vec::new();
    let mut each = |what: &str, a: &[u64], b: &[u64]| {
        for (k, (x, y)) in a.iter().zip(b).enumerate() {
            if x != y && out.len() < 12 {
                out.push(format!("{what}[{k:o}]: rtl {x:o}, micro {y:o}"));
            }
        }
    };
    each("M", &e.mmem, &u.mmem);
    each("A", &e.amem, &u.amem);
    each("PDL", &e.pdl[..4096], &u.pdl[..4096]);
    let spc = |m: &Machine| m.spc.iter().map(|&v| u64::from(v)).collect::<Vec<_>>();
    each("SPC", &spc(e), &spc(u));
    let dm = |m: &Machine| m.dmem[..1024].iter().map(|&v| u64::from(v)).collect::<Vec<_>>();
    each("DMEM", &dm(e), &dm(u));
    each("main", &e.main[..4096], &u.main[..4096]);
    let regs = |m: &Machine| muir::pipeline::registers_of(m).to_vec();
    each("reg", &regs(e), &regs(u));
    assert!(out.is_empty(), "seed {seed}: the states differ: {}", out.join("; "));
}

/// **Programs at random end as on `micro`**, microcycle for microcycle.
#[test]
fn random_programs_end_as_on_micro() {
    let mut total = muir::pipeline::Meters::default();
    let mut add = |m: muir::pipeline::Meters| {
        total.clocks += m.clocks;
        for k in 0..3 {
            total.mispredicted[k] += m.mispredicted[k];
        }
        total.squashed += m.squashed;
        total.late_squashes += m.late_squashes;
        total.oa_hold += m.oa_hold;
        total.guard_hold += m.guard_hold;
        total.pdl_wait += m.pdl_wait;
        total.md_wait += m.md_wait;
        total.start_wait += m.start_wait;
        total.muldiv += m.muldiv;
        total.wb_hold += m.wb_hold;
        total.map_hold += m.map_hold;
        total.rd_disagreements += m.rd_disagreements;
        total.restores_without_redirect += m.restores_without_redirect;
        total.restores_changed += m.restores_changed;
        total.restores_changed_planned += m.restores_changed_planned;
        total.p1_holds += m.p1_holds;
        total.p2_holds += m.p2_holds;
    };
    for seed in 1..=3000 {
        add(compare(&random_program(seed, 0), seed));
    }
    for seed in 1..=3000 {
        add(compare(&random_program(seed, ALL), seed));
    }
    eprintln!("{total:?}");
    assert_eq!(total.rd_disagreements, 0, "RD resolved a transfer otherwise than micro");
    // M-3 (the MP4 timing review §5): a restore of RD's copies without a
    // redirect leaves their values as they stand.
    eprintln!(
        "restores without a redirect: {}, changed: {}, under a word RD plans: {}",
        total.restores_without_redirect, total.restores_changed, total.restores_changed_planned
    );
    assert_eq!(total.restores_changed_planned, 0, "a restore changed the copies RD plans from");
    for (what, n) in [
        ("mispredicted jumps", total.mispredicted[0]),
        ("mispredicted dispatches", total.mispredicted[1]),
        ("late squashes", total.late_squashes),
        ("OA holds", total.oa_hold),
        ("guard holds", total.guard_hold),
        ("PDL waits", total.pdl_wait),
        ("MD waits", total.md_wait),
        ("starts behind starts", total.start_wait),
        ("DIV and MUL", total.muldiv),
    ] {
        assert!(n > 0, "the programs never made {what}");
    }
}

// --- The main loop: fused returns, D, RD's return inputs ---------------------

/// The main loop, at an address with `<1:0>` clear, made as microcode
/// 2001's `QMLP` is (`uc-macrocode.lisp:9-13`): the condition-6 call,
/// `M-INST-BUFFER <- MD`, the dispatch on the halfword's `<13:9>` with the
/// push of the main loop's return in its slot.
const QMLP: u64 = 0o100;
/// Condition 6's call: counts in M 10.
const COND_6: u64 = 0o110;
/// The opcode table in dispatch memory.
const OPDTB: u64 = 0o2300;
/// The main loop's return, `<14>` and its address.
const MAIN: u32 = 1 << 14 | QMLP as u32;
/// Opcode 1's handler, counting in M 1; opcode 6's, whose entry has P and
/// N, counting in M 6; opcode 7's, a jump to itself; opcode 10's, which
/// pushes M 31 and PDL-INDEX as its first two microinstructions find them.
const OP_1: u64 = 0o204;
const OP_6: u64 = 0o120;
const OP_7: u64 = 0o177;
const OP_10: u64 = 0o230;
/// Where the register names `A-LOCALP` and `M-AP`, and what they hold.
const LOCALP_AT: u64 = 0o432;
const AP_AT: u64 = 0o21;
const LOCALP: Word = 0o1000;
/// PDL-INDEX at the start, where no operand address is loaded.
const SENTINEL: u16 = 0o3777;
/// The macroinstructions, in the physical memory window: word 400.
const CODE: Word = 0o36000000400;
/// A paged address with no page table, where a fetch faults: the walk
/// finds no directory (A14.3), and the translation, no entry, is in main
/// memory's frame 0 without access.
const UNMAPPED: Word = 0o1000;

/// A halfword: `<13:9>` the opcode, `<8:6>` the register, `<5:0>` delta.
const fn hw(op: u32, reg: u32, delta: u32) -> u32 {
    op << 9 | reg << 6 | delta
}

/// The main loop's machine, of `geometry`, running the halfwords
/// `program` from `code`, with the MACRO-DISPATCH register `register` and
/// INTERRUPT-CONTROL `<34>`, the sequence break, as `sequence_break`
/// says. Opcode 10's entries have the operand bit. Condition 6's call
/// returns, or with `fault_stops` jumps to opcode 7's stop.
fn d_machine(
    geometry: Geometry,
    register: u32,
    program: &[u32],
    code: Word,
    sequence_break: bool,
    fault_stops: bool,
) -> Machine {
    let mut words = vec![filler().raw(); 1024];
    let mut put = |at: u64, w: u64| words[at as usize] = w;
    let fd = |c: u64| c << 19 | 0o37 << 14;
    use muir::isa::asm::{CARRY_IN, M_PLUS_C};
    let inc = |m: u64| ALU | M_PLUS_C | CARRY_IN | m_src(m) | m_dest(m);
    // A read of main memory first, which takes down the `-VMAOK` the boot
    // leaves, so that condition 6 is false at the first return.
    put(0, ALU | SETA | a_src(0o55) | START_READ);
    put(1, ALU | SETA | a_src(0o51) | fd(5));
    put(2, ALU | SETA | a_src(0o52) | fd(1));
    put(3, ALU | SETA | a_src(0o53) | fd(2));
    put(4, ALU | SETA | a_src(0o50) | fd(0o15));
    put(5, ALU | SETA | a_src(LOCALP_AT) | a_dest(LOCALP_AT));
    put(6, ALU | SETM | m_src(AP_AT) | m_dest(AP_AT) | POPJ);
    put(QMLP, JUMP | target(COND_6) | P | 1 << 5 | 6);
    put(QMLP + 1, ALU | SETM | SRC_MD | m_dest(0o31));
    put(QMLP + 2, DISPATCH | m_src(0o31) | 3 << 10 | 31 | 5 << 5 | OPDTB << 12);
    put(QMLP + 3, ALU | SETA | a_src(0o50) | fd(0o15));
    if fault_stops {
        put(COND_6, inc(0o10));
        put(COND_6 + 1, JUMP | target(OP_7) | ALWAYS | N);
    } else {
        put(COND_6, inc(0o10) | POPJ);
    }
    put(OP_1, inc(1));
    put(OP_1 + 1, filler().raw() | POPJ);
    // The microcycle after its return pushes M 31 as it finds it.
    put(OP_1 + 2, ALU | SETM | m_src(0o31) | fd(0o11));
    put(OP_6, ALU | SETM | src(0o14) | m_dest(0o20));
    put(OP_6 + 1, inc(6));
    put(OP_6 + 2, ALU | SETA | a_src(0o50) | fd(0o15));
    put(OP_6 + 4, filler().raw() | POPJ);
    put(OP_7, JUMP | target(OP_7) | ALWAYS | N);
    put(OP_10, ALU | SETM | m_src(0o31) | fd(0o11));
    put(OP_10 + 1, ALU | SETM | src(3) | fd(0o11));
    put(OP_10 + 2, ALU | SETA | a_src(0o54) | fd(0o13) | POPJ);
    let mut m = Machine::with_geometry(geometry, 1);
    m.load_prom(&words.iter().map(|&w| Insn::extended(w)).collect::<Vec<_>>());
    support::prom_program_in_ram(&mut m);
    for (op, at) in [(1, OP_1), (6, 1 << 15 | 1 << 14 | OP_6), (7, OP_7), (0o10, OP_10)] {
        m.dmem[OPDTB as usize + op] = at as u32;
    }
    for (k, e) in m.macro_dispatch.entries.iter_mut().enumerate() {
        *e = m.dmem[OPDTB as usize + (k >> 3 & 0o37)];
        if k >> 3 & 0o37 == 0o10 {
            *e |= muir::machine::macro_dispatch::OPERAND;
        }
    }
    m.amem[0o50] = u64::from(MAIN);
    m.amem[0o51] = u64::from(register);
    m.amem[0o52] = code * 4;
    m.amem[0o53] = if sequence_break { 1 << 34 } else { 0 };
    m.amem[0o54] = u64::from(SENTINEL);
    m.amem[0o55] = 0o36000000000;
    m.amem[LOCALP_AT as usize] = LOCALP;
    m.pdl_index = SENTINEL;
    let base = (code & 0o1777777777) as usize;
    for (k, pair) in program.chunks(2).enumerate().filter(|_| code == CODE) {
        m.main[base + k] = u64::from(pair[0] | pair.get(1).copied().unwrap_or(0) << 16);
    }
    m
}

/// The register's word: the main loop at `QMLP`, the bases, enabled, and
/// D's enable as `d` says.
fn d_register(d: bool) -> u32 {
    let d = if d { muir::machine::macro_dispatch::D_ENABLE } else { 0 };
    muir::machine::macro_dispatch::word(QMLP as u16, LOCALP_AT as u16, AP_AT as u8) | d
}

/// Opcode 1 eleven times, then 7: five returns need a fetch, into each
/// word's first halfword but the first.
const ONES: [u32; 12] = [
    hw(1, 0, 0),
    hw(1, 0, 0),
    hw(1, 0, 0),
    hw(1, 0, 0),
    hw(1, 0, 0),
    hw(1, 0, 0),
    hw(1, 0, 0),
    hw(1, 0, 0),
    hw(1, 0, 0),
    hw(1, 0, 0),
    hw(1, 0, 0),
    hw(7, 0, 0),
];

/// The main-loop machine on both engines: the same microcycles and state.
fn ml_same(m: Machine) -> Pipeline {
    let mut u = Micro::new(m.clone());
    u.boot();
    let mut ut: Vec<Option<u16>> = Vec::new();
    for _ in 0..200_000 {
        if u.machine().opc == OP_7 as u16 {
            break;
        }
        u.step().unwrap();
        ut.push(u.executed());
    }
    let mut e = Pipeline::new(m);
    e.trace = Some(Vec::new());
    e.boot();
    e.skip_sweep();
    for _ in 0..2_000_000 {
        if e.machine().opc == OP_7 as u16 {
            break;
        }
        e.step().unwrap();
    }
    let mut et = e.trace.take().unwrap();
    // Both to the stop's first run: the pipeline may have committed past it
    // when its step counted the microcycle before.
    let cut = |t: &mut Vec<Option<u16>>| {
        if let Some(k) = t.iter().position(|&p| p == Some(OP_7 as u16)) {
            t.truncate(k + 1);
        }
    };
    cut(&mut ut);
    cut(&mut et);
    let n = ut.len().min(et.len());
    if let Some(k) = (0..n).find(|&k| ut[k] != et[k]) {
        panic!(
            "microcycle {k}: micro {:?}, rtl {:?}; micro {:?}, rtl {:?}",
            ut[k],
            et[k],
            &ut[k.saturating_sub(6)..(k + 4).min(n)],
            &et[k.saturating_sub(6)..(k + 4).min(n)]
        );
    }
    assert_eq!(ut.len(), et.len(), "microcycles to the stop");
    // Drained past the last write's response: the Kria's 215 ns is 22
    // clocks at the default 10 ns, 26 at 8.5 ns; 32 microcycles cover both.
    for _ in 0..32 {
        e.step().unwrap();
        u.step().unwrap();
    }
    diff_state(e.machine(), u.machine(), 0);
    e
}

/// **D and the fused return on the pipeline end as on `micro`**, D on and
/// off, microcycle for microcycle (A15b.9; the contract's §8).
#[test]
fn d_and_the_fused_return_end_as_on_micro() {
    for d in [false, true] {
        let e = ml_same(d_machine(REV16, d_register(d), &ONES, CODE, false, false));
        assert_eq!(e.meters.rd_disagreements, 0);
    }
}

/// **The main loop's other paths end as on `micro`** (A15b.9): a sequence
/// break, a faulting fetch, an entry with P, and opcode 10's operand
/// address after D, D on and off.
#[test]
fn the_main_loop_s_paths_end_as_on_micro() {
    for d in [false, true] {
        ml_same(d_machine(REV16, d_register(d), &ONES, CODE, true, false));
        ml_same(d_machine(REV16, d_register(d), &ONES, UNMAPPED, false, true));
        let mut program = ONES;
        program[4] = hw(6, 0, 0);
        ml_same(d_machine(REV16, d_register(d), &program, CODE, false, false));
        let program = [hw(1, 0, 0), hw(1, 0, 0), hw(0o10, 5, 5), hw(7, 0, 0)];
        ml_same(d_machine(REV16, d_register(d), &program, CODE, false, false));
        let program = [hw(0o10, 5, 5), hw(0o10, 6, 3), hw(1, 0, 0), hw(0o10, 5, 1), hw(7, 0, 0)];
        ml_same(d_machine(REV16, d_register(d), &program, CODE, false, false));
    }
}

/// **D held off on the main-loop machines** (MP2b ruling Q12(d),
/// `MUIR_D=off`): the machines whose microcode writes D's enable run, D
/// held off, as the machines whose microcode leaves it clear, microcycle for
/// microcycle and state for state, on both engines, where D on runs
/// otherwise; D's enable stays clear.
#[test]
fn d_held_off_runs_the_main_loop_machines_as_d_off() {
    use muir::machine::DSwitch;
    let e_cycles = |e: &Pipeline| e.machine().cycles;
    let programs: [&[u32]; 2] =
        [&ONES, &[hw(0o10, 5, 5), hw(0o10, 6, 3), hw(1, 0, 0), hw(0o10, 5, 1), hw(7, 0, 0)]];
    for program in programs {
        let mut held = d_machine(REV16, d_register(true), program, CODE, false, false);
        held.macro_dispatch.d = DSwitch::Off;
        let off = d_machine(REV16, d_register(false), program, CODE, false, false);
        let (eh, eo) = (ml_same(held), ml_same(off));
        let fused = |e: &Pipeline| e.machine().macro_dispatch.fused;
        assert_eq!(e_cycles(&eh), e_cycles(&eo), "the microcycles, D held off and D off");
        assert_eq!(fused(&eh), fused(&eo), "the fused returns, D held off and D off");
        // The end: all but A memory's constant, the register's word.
        let end = |m: &Machine| (m.mmem, m.pdl_index, m.spcptr, m.lc, m.macro_dispatch.register);
        assert_eq!(end(eh.machine()), end(eo.machine()), "the end, D held off and D off");
        let d = muir::machine::macro_dispatch::D_ENABLE;
        assert_eq!(eh.machine().macro_dispatch.register & d, 0, "D's enable clear");
        // The control: D on, as written, runs other microcycles.
        let on = ml_same(d_machine(REV16, d_register(true), program, CODE, false, false));
        assert!(fused(&on) > fused(&eo), "D on fuses more: {} {}", fused(&on), fused(&eo));
    }
}

// --- Time (A15b.12; MP2b rulings Q1-Q4, Q9, Q10) -------------------------------

/// Runs `p` on the pipeline at the period `period`, the TLB's sweep
/// skipped, to the stop.
fn pipeline_at(p: &Prog, period: u64) -> Pipeline {
    let mut e = Pipeline::new(machine(p, REV16));
    e.configure(period, muir::pipeline::PortTiming::KRIA, 65_536);
    e.boot();
    e.skip_sweep();
    for _ in 0..2_000_000 {
        if e.machine().opc == STOP as u16 {
            for _ in 0..16 {
                e.step().unwrap();
            }
            return e;
        }
        e.step().unwrap();
    }
    panic!("never stopped");
}

/// **Word 25 reads the period on `rtl` too** (A15b.1; MP2b ruling Q1): 17
/// at 8.5 ns, 20 at 10 ns, the default, and 80 at 40 ns. **Fails** a word
/// 25 of whole ns doubled, which cannot say 17, and one that is `micro`'s
/// alone.
#[test]
fn word_25_reads_the_period_on_rtl() {
    let mut p = Prog::default();
    p.read(REGISTER_PAGE | 0o25, 0o20).stop();
    assert_eq!(pipeline_at(&p, 17).machine().mmem[0o20], 17);
    assert_eq!(pipeline_at(&p, 80).machine().mmem[0o20], 80);
    assert_eq!(pipeline(&p).machine().mmem[0o20], 20, "10 ns when nothing says");
}

/// **A timer's rise is acted on at the first clock at or after it** (MP2b
/// ruling Q1): at 8.5 ns a 1 µs periodic timer started at clock 1, its flag
/// cleared at each rise, rises the 14th time at clock 1,649; with the
/// instant floored to whole ns it would be 1,648. Read through the register
/// page at each clock's instant, as the engines read it. **Fails** a time in
/// whole ns.
#[test]
fn a_1_us_timer_rises_at_clock_1649_the_14th_time() {
    let mut m = Machine::with_geometry(REV16, 1);
    m.period = 17;
    let page = muir::tlb::REGISTER_PAGE_BUS;
    m.ns = 17;
    m.bus_write(page | 0o113, 1);
    m.bus_write(page | 0o112, 1);
    let mut rises = Vec::new();
    for clock in 2..2000u64 {
        m.ns = clock * 17;
        if m.bus_read(page | 0o112) & 2 != 0 {
            rises.push(clock);
            m.bus_write(page | 0o112, 1 | 2);
        }
    }
    assert_eq!(rises[13], 1649, "the 14th rise; the first are {:?}", &rises[..4]);
}

/// **The accumulators keep true time over a simulated second at 7, 8, 8.5,
/// 11, 19, 48 and 63.5 ns, within one period** (A15b.12), the last the
/// longest the fabric's period field holds: the microsecond clock reads
/// k from the first clock at or after k µs, and k − 1 at the clock before;
/// timer 0 at the tick's period rises at the first clock at or after each
/// of its edges; the counted real-time clock turns at the first clock at or
/// after its second. Every edge acted on is less than a period past its
/// true instant. **Fails** a time that drifts, one rounded per clock, and
/// one in whole ns at an odd period.
#[test]
fn the_accumulators_keep_true_time_over_a_second() {
    for period in [14u64, 16, 17, 22, 38, 96, 127] {
        let mut m = Machine::with_geometry(REV16, 1);
        m.period = period;
        let first = |t: u64| t.div_ceil(period);
        // The microsecond clock.
        for k in (1..=1_000_000u64).step_by(997) {
            let c = first(k * 2000);
            m.ns = c * period;
            assert_eq!(m.microseconds(), k as u32, "{period}: µs {k} at clock {c}");
            assert!(c * period - k * 2000 < period, "within a period");
            m.ns = (c - 1) * period;
            assert_eq!(m.microseconds(), (k - 1) as u32, "{period}: µs {k} before clock {c}");
        }
        // Timer 0 at the tick's period, started at clock 0.
        let page = muir::tlb::REGISTER_PAGE_BUS;
        m.ns = 0;
        m.bus_write(page | 0o111, muir::machine::Timers::TICK_PERIOD_US.into());
        m.bus_write(page | 0o110, 1);
        for k in 1..=59u64 {
            let edge = k * 16_667 * 2000;
            let c = first(edge);
            m.ns = (c - 1) * period;
            assert_eq!(m.bus_read(page | 0o110) & 2, 0, "{period}: tick {k} not before clock {c}");
            m.ns = c * period;
            assert_eq!(m.bus_read(page | 0o110) & 2, 2, "{period}: tick {k} at clock {c}");
            m.bus_write(page | 0o110, 1 | 2);
        }
        // The real-time clock, counted from 1000.
        m.rtc = muir::machine::Rtc::Counted { start: 1000, base_ns: 0 };
        let c = first(2_000_000_000);
        m.ns = (c - 1) * period;
        assert_eq!(m.bus_read(page | 0o103), 1000);
        m.ns = c * period;
        assert_eq!(m.bus_read(page | 0o103), 1001);
    }
}

/// **A duration is rounded up to clocks once, from its own start** (MP2b
/// ruling Q2): a file device command of 43 bytes taken at 8.5 ns completes
/// 2,847 clocks after its start; rounding each term to clocks gives 2,848,
/// and so does rounding to whole ns first. A command taken at an odd unit
/// starts on its clock. **Fails** either rounding.
#[test]
fn a_command_s_time_is_rounded_to_clocks_once() {
    use muir::clock::TimeBase;
    use muir::file_device::due_on;
    let t = TimeBase::half_ns(17);
    let start = 1000 * 17;
    assert_eq!(due_on(t, start, 43, 0), start + 2847 * 17);
    let per_term = (40_000u64).div_ceil(17) + (2 * 43 * 100_000u64).div_ceil(1024 * 17);
    assert_eq!(per_term, 2848, "rounded per term");
    let via_ns = (2 * (20_000 + (43u64 * 100_000).div_ceil(1024))).div_ceil(17);
    assert_eq!(via_ns, 2848, "rounded to ns first");
    // Revisions 13 and 14 keep ns.
    assert_eq!(due_on(TimeBase::NS, 100, 43, 0), muir::file_device::due(100, 43, 0));
}

/// A DIV's or MUL's clocks in EX on the pipeline at `period`, and the
/// state against `micro`'s.
fn muldiv_clocks(f: u64, period: u64, of_md: bool) -> (u64, Pipeline) {
    let mut p = Prog::default();
    let (n, d) = (p.k(1_000_000), p.k(7));
    p.op(ALU | SETA | a_src(n) | m_dest(0o20));
    if of_md {
        let a = p.k(PHYS | 0o200);
        p.main.push((0o200, 12_345));
        p.op(ALU | SETA | a_src(a) | START_READ);
        p.fill(1);
        p.op(ALU | f << 3 | SRC_MD | a_src(d) | m_dest(0o21));
    } else {
        p.op(ALU | f << 3 | m_src(0o20) | a_src(d) | m_dest(0o21));
    }
    p.stop();
    let u = micro(&p);
    let e = pipeline_at(&p, period);
    diff_state(e.machine(), u.machine(), 0);
    (e.meters.muldiv, e)
}

/// **DIV stays in EX 18 clocks and MUL 5, at every period** (A15b.3; MP2b
/// ruling Q3), 17 and 4 of them held; a DIV of `MD` counts from the wait
/// for `MD`'s end, so its read's fill adds to its time and not to its
/// count. Results as `micro`'s. **Fails** a count in ns, which differs at
/// 8.5 and 19 ns, and one counted from EX's entry.
#[test]
fn div_stays_18_clocks_in_ex_and_mul_5() {
    for period in [17, 38, 127] {
        assert_eq!(muldiv_clocks(0o43, period, false).0, 17, "DIV at {period}");
        assert_eq!(muldiv_clocks(0o42, period, false).0, 4, "MUL at {period}");
        let (held, e) = muldiv_clocks(0o43, period, true);
        assert_eq!(held, 17, "DIV of MD at {period}: counted from the landing");
        assert!(e.meters.md_wait > 0, "the DIV waited for MD first");
    }
}

/// The pipeline on `p` at `period` and `timing`, its events recorded.
fn events(
    p: &Prog,
    period: u64,
    timing: muir::pipeline::PortTiming,
) -> (Pipeline, Vec<(u64, muir::pipeline::Event)>) {
    let mut e = Pipeline::new(machine(p, REV16));
    e.configure(period, timing, 65_536);
    e.events = Some(Vec::new());
    e.boot();
    e.skip_sweep();
    for _ in 0..2_000_000 {
        if e.machine().opc == STOP as u16 {
            break;
        }
        e.step().unwrap();
    }
    let ev = e.events.take().unwrap();
    (e, ev)
}

/// The clocks from each grant to the next `MD` landing, in order.
fn grant_to_md(ev: &[(u64, muir::pipeline::Event)]) -> Vec<u64> {
    use muir::pipeline::Event;
    let mut out = Vec::new();
    let mut granted = None;
    for &(clock, e) in ev {
        match e {
            Event::Grant(_, false) => granted = Some(clock),
            Event::Md(_) => {
                if let Some(g) = granted.take() {
                    out.push(clock - g);
                }
            }
            _ => {}
        }
    }
    out
}

/// **A hit lands two clocks after its grant, and a miss its line's fill
/// later** (A15b.5, A15b.6; MP2b rulings Q6, Q7): at 8.5 ns the Kria's
/// 344 ns fill is 41 clocks, after the clock the tags are compared in; the
/// DE25's 600 ns at 11 ns is 55. **Fails** a fill with the beats term (44
/// at 8.5 ns), and a hit that is not Dmd 3's.
#[test]
fn a_hit_lands_two_clocks_after_its_grant_and_a_miss_its_fill_later() {
    use muir::pipeline::PortTiming;
    let mut p = Prog::default();
    p.read(PHYS | 0o100, 0o20).read(PHYS | 0o101, 0o21).stop();
    let (_, ev) = events(&p, 17, PortTiming::KRIA);
    assert_eq!(grant_to_md(&ev), [1 + 41, 2], "kria at 8.5 ns: a miss, then a hit");
    let (_, ev) = events(&p, 22, PortTiming::DE25);
    assert_eq!(grant_to_md(&ev), [1 + 55, 2], "de25 at 11 ns");
}

/// **A fill is never shorter than the port can do**: the 64-bit port reads
/// a line in 5 beats, and the fill lands the clock after the last, so 6
/// clocks at the least, on every board's period and every memory timing.
/// A timing under that is held to 6: 100 ns at the Kria's 20 ns, 120 at
/// the DE25-Nano's 25 and 247 at the Arty's 50 ns, each 5 clocks
/// unfloored. One over it keeps its own: the presets at the boards'
/// periods, 18, 24 and 6 clocks, 300 ns at 50 ns, 6, and 400, 8. Read off
/// the run, a miss's grant to its `MD`, and off the port's clocks.
/// **Fails** a fill of ⌈r/P⌉ clocks with no floor.
#[test]
fn a_fill_is_never_shorter_than_the_port_s_beats_and_its_landing() {
    use muir::pipeline::PortTiming;
    let mut p = Prog::default();
    p.read(PHYS | 0o100, 0o20).read(PHYS | 0o101, 0o21).stop();
    let read = |t: PortTiming, ns: u64| PortTiming { read_ns: ns, ..t };
    let (kria, de25, arty) = (PortTiming::KRIA, PortTiming::DE25, PortTiming::ARTY);
    let mut wrong = Vec::new();
    for (board, period, timing, fill) in [
        ("kria", 40, read(kria, 100), 6),
        ("de25", 50, read(de25, 120), 6),
        ("arty", 100, read(arty, 247), 6),
        ("kria", 40, kria, 18),
        ("de25", 50, de25, 24),
        ("arty", 100, arty, 6),
        ("arty", 100, read(arty, 300), 6),
        ("arty", 100, read(arty, 400), 8),
    ] {
        let (e, ev) = events(&p, period, timing);
        let got = (e.port.clocks().read, grant_to_md(&ev));
        if got != (fill, vec![1 + fill, 2]) {
            wrong.push(format!(
                "{board} at {} ns, a fill of {} ns: the port's {} clocks and grant to MD {:?}, \
                 not {fill} and [{}, 2]",
                period / 2,
                timing.read_ns,
                got.0,
                got.1,
                1 + fill
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// **The `de25` preset is the DE25-Nano's measured port** (MP2b ruling
/// Q5): a fill 600 ns, a write 485, the occupancy 40; at the board's
/// 22.5 ns, 27, 22 and 2 clocks. **Fails** the model's figures carried
/// before the board was measured, 410 and 290.
#[test]
fn the_de25_preset_is_the_de25_nano_s_measured_port() {
    use muir::clock::TimeBase;
    use muir::pipeline::PortTiming;
    let de25 = PortTiming { read_ns: 600, write_ns: 485, occupancy_ns: 40 };
    assert_eq!(PortTiming::DE25, de25);
    assert_eq!(PortTiming::parse("de25"), Some(de25));
    let c = de25.clocks(TimeBase::half_ns(45));
    assert_eq!((c.read, c.write, c.occupancy), (27, 22, 2), "at 22.5 ns");
}

/// **The `kria` preset is the Kria KR260's measured port** (MP2b ruling
/// Q5): a fill 344 ns, a write 215, the occupancy 10; at the board's
/// 19 ns, 19, 12 and 1 clocks. **Fails** the idle 100 MHz figures carried
/// before the board was measured on revision 15, 247 and 130.
#[test]
fn the_kria_preset_is_the_kria_kr260_s_measured_port() {
    use muir::clock::TimeBase;
    use muir::pipeline::PortTiming;
    let kria = PortTiming { read_ns: 344, write_ns: 215, occupancy_ns: 10 };
    assert_eq!(PortTiming::KRIA, kria);
    assert_eq!(PortTiming::parse("kria"), Some(kria));
    let c = kria.clocks(TimeBase::half_ns(38));
    assert_eq!((c.read, c.write, c.occupancy), (19, 12, 1), "at 19 ns");
}

/// **A register takes a clock more than nothing** (A15b.3; MP2b ruling
/// Q4): a register's word is in `MD` two clocks after its grant, as a
/// hit's, and an empty address's zero one clock after. **Fails** the
/// CADR's delay lines kept, and a register answered as a miss.
#[test]
fn a_register_takes_a_clock_more_than_nothing() {
    let mut p = Prog::default();
    p.read(REGISTER_PAGE, 0o20).read(PHYS | 0o7777777, 0o21).stop();
    let (e, ev) = events(&p, 17, muir::pipeline::PortTiming::KRIA);
    assert_eq!(grant_to_md(&ev), [2, 1], "a register, then nothing there");
    assert_eq!(e.machine().mmem[0o20], REV16.machine_id.unwrap().into(), "MACHINE-ID");
    assert_eq!(e.machine().mmem[0o21], 0, "nothing there reads 0");
}

/// **An empty sweeps the TLB one entry a clock** (MP2b ruling Q9): a start
/// after a `WRITE-MAP` empty is granted no sooner than N clocks after the
/// empty lands, at 8.5, 19 and 63.5 ns alike. **Fails** N × 10 ns.
#[test]
fn an_empty_sweeps_the_tlb_one_entry_a_clock() {
    use muir::pipeline::Event;
    let mut p = Prog::default();
    let empty = p.k(3 << 32);
    p.op(ALU | SETA | a_src(empty) | fd(0o23));
    p.fill(1);
    p.read(PHYS | 3, 0o20).stop();
    for period in [17, 38, 127] {
        let (e, ev) = events(&p, period, muir::pipeline::PortTiming::KRIA);
        let landed =
            ev.iter().position(|&(_, x)| x == Event::Commit(Some(1))).map(|k| ev[k].0).unwrap();
        let grant = ev.iter().find(|(_, x)| matches!(x, Event::Grant(..))).unwrap().0;
        let n = e.machine().tlb.len() as u64;
        assert!(
            grant >= landed + n && grant <= landed + n + 4,
            "at {period}: the empty landed at {landed}, the grant at {grant}"
        );
    }
}

// --- Holds and squashes (A15b.3, A15b.15, A15b.16) -----------------------------

/// `micro` on `p` with its OA select check off: a select may stand words
/// after its write here, as the pipeline's hold is measured on.
fn micro_unchecked(p: &Prog) -> Micro {
    let mut u = Micro::new(machine(p, REV16));
    u.oa_select_check = false;
    match run_engine(u) {
        Ok(u) => u,
        Err((h, _)) => panic!("micro halted: {h:?}"),
    }
}

/// An OA-REG-HIGH write `before` words ahead of the word that selects it
/// into its A and M sources.
fn oa_high_program(before: usize) -> Prog {
    use muir::isa::asm::{ADD, OA_HIGH_SELECT};
    let mut p = Prog::default();
    p.set(0o11, 0o23).set(0o22, 0o24);
    p.amem.push((0o1703, 0o33));
    // A 1700 | 3, M 20 | 4.
    let v = p.k(3 << 6 | 4);
    p.op(ALU | SETA | a_src(v) | fd(0o17));
    p.fill(before - 1);
    p.op(ALU | ADD | a_src(0o1700) | m_src(0o20) | m_dest(0o25) | OA_HIGH_SELECT);
    p.stop();
    p
}

/// **The OA-REG-HIGH hold is 2, 1 and 0 clocks for a writer 1, 2 and 3
/// words before** (A15b.15): CS waits while the writer is in RD or EX, so
/// the selecting word's A address, which leaves CS, takes the register
/// written. Results as `micro`'s (its select check off for the words
/// between). The hold removed, or a clock short, reads the old register:
/// both are caught.
#[test]
fn the_oa_high_hold_is_2_1_0_clocks() {
    for (before, hold) in [(1, 2), (2, 1), (3, 0)] {
        let p = oa_high_program(before);
        let u = micro_unchecked(&p);
        let e = pipeline(&p);
        diff_state(e.machine(), u.machine(), 0);
        assert_eq!(e.meters.oa_hold, hold, "a writer {before} words before");
        assert_eq!(u.machine().mmem[0o25], 0o22 + 0o33, "the select's sources");
    }
    let p = oa_high_program(1);
    let u = micro_unchecked(&p);
    for mutation in [Mutation::NoOaHold, Mutation::OaHoldShort] {
        let e = pipeline_with(&p, |e| e.mutation = mutation);
        assert_ne!(e.machine().mmem[0o25], u.machine().mmem[0o25], "{mutation:?} caught");
    }
}

/// **The OA meter shows SL jumps and SH holds alone** (A15b.15): a program
/// of two SH words, each right after its writer, and three SL jumps, which
/// RD cannot resolve: the meter counts the jumps, and a hold of two clocks
/// for each SH word, nothing else. **Fails** a hold on SL, and SL jumps
/// counted as predictions.
#[test]
fn the_oa_meter_shows_sl_jumps_and_sh_holds_alone() {
    use muir::isa::asm::{ADD, OA_HIGH_SELECT, OA_LOW_SELECT};
    let mut p = Prog::default();
    for _ in 0..2 {
        let v = p.k(1 << 6 | 1);
        p.op(ALU | SETA | a_src(v) | fd(0o17));
        p.op(ALU | ADD | a_src(0o1700) | m_src(0o20) | m_dest(0o25) | OA_HIGH_SELECT);
    }
    for _ in 0..3 {
        let to = p.at() + 4;
        let v = p.k(to << 12);
        p.op(ALU | SETA | a_src(v) | fd(0o16));
        p.op(JUMP | ALWAYS | N | OA_LOW_SELECT);
        p.fill(2);
    }
    p.stop();
    let e = same(&p);
    assert_eq!(e.meters.sl_jumps, 3);
    assert_eq!(e.meters.oa_hold, 2 * 2);
}

/// Opcode 2's handler: a return at its first word, so that the word before
/// the return is the microcycle after the last return, which steps LC.
const OP_2: u64 = 0o240;
/// Opcode 3's handler: a word planted before its return.
const OP_3: u64 = 0o250;

/// The main-loop machine with opcodes 2 and 3 added: opcode 3's handler
/// the words `body`, then its return.
fn ml_machine(d: bool, program: &[u32], body: &[u64], setup: &dyn Fn(&mut Machine)) -> Machine {
    let mut m = d_machine(REV16, d_register(d), program, CODE, false, false);
    let mut put = |at: u64, w: u64| m.imem[at as usize] = Insn::extended(w);
    put(OP_2, filler().raw() | POPJ);
    put(OP_2 + 1, ALU | SETM | m_src(0o31) | fd(0o11));
    let n = body.len() as u64;
    for (k, &w) in body.iter().enumerate() {
        put(OP_3 + k as u64, w);
    }
    put(OP_3 + n, filler().raw() | POPJ);
    put(OP_3 + n + 1, ALU | SETM | m_src(0o31) | fd(0o11));
    m.dmem[OPDTB as usize + 2] = OP_2 as u32;
    m.dmem[OPDTB as usize + 3] = OP_3 as u32;
    for (k, e) in m.macro_dispatch.entries.iter_mut().enumerate() {
        match k >> 3 & 0o37 {
            2 => *e = OP_2 as u32,
            3 => *e = OP_3 as u32,
            _ => {}
        }
    }
    setup(&mut m);
    m
}

/// `m` on both engines, and on the pipeline with each of `mutations`, each
/// of which must change the end.
fn ml_same_and_caught(m: Machine, mutations: &[Mutation], what: &str) -> Pipeline {
    let e = ml_same(m.clone());
    let u = {
        let mut u = Micro::new(m.clone());
        u.boot();
        for _ in 0..200_000 {
            if u.machine().opc == OP_7 as u16 {
                break;
            }
            u.step().unwrap();
        }
        for _ in 0..16 {
            u.step().unwrap();
        }
        u
    };
    for &mutation in mutations {
        let mut x = Pipeline::new(m.clone());
        x.mutation = mutation;
        x.boot();
        x.skip_sweep();
        let mut stopped = false;
        for _ in 0..200_000 {
            if x.machine().opc == OP_7 as u16 {
                stopped = true;
                break;
            }
            if x.step().is_err() {
                break;
            }
        }
        if stopped {
            for _ in 0..16 {
                let _ = x.step();
            }
            assert_ne!(
                state(x.machine()),
                state(u.machine()),
                "{what}: {mutation:?} planted, and the end is micro's"
            );
        }
    }
    e
}

/// The halfwords: opcode 2 (a return at its handler's first word) and 3
/// (the planted word before its return), then 7.
const TWOS_THREES: [u32; 10] = [
    hw(1, 0, 0),
    hw(2, 0, 0),
    hw(3, 0, 0),
    hw(2, 0, 0),
    hw(1, 0, 0),
    hw(3, 0, 0),
    hw(2, 0, 0),
    hw(3, 0, 0),
    hw(1, 0, 0),
    hw(7, 0, 0),
];

/// Opcode 11's handler: reads the PDL buffer through PDL-INDEX as its first
/// word finds it, the operand address its fused return loaded.
const OP_11: u64 = 0o270;

/// **A fused return's operand address over a base the word before writes**
/// (A15b.9, A15b.16): opcode 3's return, right after a word that writes
/// A-LOCALP with its own word, fuses on opcode 11, LOCAL 2, whose handler
/// reads the PDL buffer through the index the address loads, in its first
/// word. Both engines run it to the stop and end alike, D off and on; RD's
/// pending mark on its index copy, made for the operand address, clears
/// when the address loads, where a mark that waited for a sequence nothing
/// commits held CS for ever.
#[test]
fn an_operand_address_over_a_base_the_word_before_writes() {
    for d in [false, true] {
        let mut m = ml_machine(
            d,
            &[hw(3, 0, 0), hw(0o11, 5, 2), hw(1, 0, 0), hw(7, 0, 0)],
            &[ALU | SETA | a_src(LOCALP_AT) | a_dest(LOCALP_AT)],
            &|_| {},
        );
        let mut put = |at: u64, w: u64| m.imem[at as usize] = Insn::extended(w);
        put(OP_11, ALU | SETM | src(0o5) | m_dest(0o27));
        put(OP_11 + 1, filler().raw() | POPJ);
        put(OP_11 + 2, ALU | SETM | m_src(0o31) | fd(0o11));
        m.dmem[OPDTB as usize + 0o11] = OP_11 as u32;
        for (k, e) in m.macro_dispatch.entries.iter_mut().enumerate() {
            if k >> 3 & 0o37 == 0o11 {
                *e = OP_11 as u32 | muir::machine::macro_dispatch::OPERAND;
            }
        }
        // The PDL buffer's word at LOCALP + 2, which the handler reads.
        m.pdl[LOCALP as usize + 2] = 0o4242;
        let e = ml_same(m);
        assert_eq!(e.machine().mmem[0o27], 0o4242, "D {d}: the operand read through the index");
    }
}

/// **RD's return inputs on planted pairs** (A15b.16): a return right after
/// the microcycle that steps LC (`NEXT INSTRD`), and right after a write of
/// LC, with D on and off: as `micro`. RD's LC copy not stepped, or not
/// marked written, is caught.
#[test]
fn rd_s_lc_copy_on_planted_pairs() {
    for d in [false, true] {
        // With D on every return that needs a fetch waits for D, whose
        // decision takes RD's copies from the committed registers: the
        // faults are planted with D off, where RD's copy decides.
        let caught = |m: &[Mutation]| if d { Vec::new() } else { m.to_vec() };
        let m = ml_machine(d, &TWOS_THREES, &[], &|_| {});
        let e = ml_same_and_caught(m, &caught(&[Mutation::LcNotStepped]), "a step before a return");
        assert!(e.meters.fused > 0, "returns fused");
        // LC written by the word before the return: its value LC's own.
        let m = ml_machine(d, &TWOS_THREES, &[ALU | SETM | src(0o13) | fd(1)], &|_| {});
        ml_same_and_caught(m, &caught(&[Mutation::LcNotMarked]), "an LC write before a return");
    }
}

/// **Each case of the guard** (A15b.16), planted before a return: the word
/// before it writes INTERRUPT-CONTROL, M 31, the MACRO-DISPATCH register or
/// entry, or pushes SPC data; the word two or three before writes M 31.
/// Each ends as on `micro`, the guard holding RD a clock, and without the
/// guard each that changes what the return reads is caught.
#[test]
fn each_case_of_the_guard_on_planted_pairs() {
    use muir::machine::macro_dispatch;
    let mut p = Prog::default();
    // M 31 a word of two opcode-7 halfwords: the next dispatch stops.
    let sevens = p.k(u64::from(hw(7, 0, 0) | hw(7, 0, 0) << 16));
    let to_7 = p.k(OP_7);
    let disabled = p.k(u64::from(d_register(false) & !macro_dispatch::ENABLE));
    let entry_7 = p.k(OP_7);
    let consts = |m: &mut Machine| {
        for &(a, v) in &p.amem {
            m.amem[a as usize] = v;
        }
    };
    let f = filler().raw();
    let m31 = ALU | SETA | a_src(sevens) | m_dest(0o31);
    // The handler's words before its return; whether the guard holds; and
    // whether the return reads what the case writes, so that the guard
    // removed is caught.
    let cases: [(&str, Vec<u64>, bool, bool); 7] = [
        ("INTERRUPT-CONTROL", vec![ALU | SETA | a_src(0o53) | fd(2)], true, false),
        ("M 31", vec![m31], true, true),
        ("the MACRO-DISPATCH register", vec![ALU | SETA | a_src(disabled) | fd(5)], true, true),
        (
            "an entry",
            vec![ALU | SETA | a_src(0o40) | fd(6), ALU | SETA | a_src(entry_7) | fd(7)],
            true,
            false,
        ),
        ("an SPC data push", vec![ALU | SETA | a_src(to_7) | fd(0o15)], true, true),
        ("M 31 two before", vec![m31, f], true, true),
        ("M 31 three before", vec![m31, f, f], false, false),
    ];
    for (what, body, holds, caught) in cases {
        for d in [false, true] {
            let m = ml_machine(d, &TWOS_THREES, &body, &consts);
            let mutations: &[Mutation] = if caught { &[Mutation::NoGuard] } else { &[] };
            let e = ml_same_and_caught(m, mutations, what);
            assert_eq!(e.meters.guard_hold > 0, holds, "{what}, D {d}: the guard");
        }
    }
}

// --- The speculation matrix (the contract's §5; A15b's checks) ---------------

/// The delay slot's kinds in the matrix.
#[derive(Clone, Copy, Debug)]
enum SlotKind {
    Plain,
    Popj,
    Call,
    Push,
    Pop,
    Start,
    MapMd,
    OaWrite,
    Transfer,
}

const SLOTS: [SlotKind; 9] = [
    SlotKind::Plain,
    SlotKind::Popj,
    SlotKind::Call,
    SlotKind::Push,
    SlotKind::Pop,
    SlotKind::Start,
    SlotKind::MapMd,
    SlotKind::OaWrite,
    SlotKind::Transfer,
];

/// The branch's kinds in the matrix.
#[derive(Clone, Copy, Debug)]
enum Branch {
    Jump,
    Call,
    Popj,
    Conditional { hint: bool, taken: bool },
    Dispatch { entry: u32, predicted: (u64, bool, bool) },
    Sl,
}

/// Where the matrix's pieces sit: the case at 100, its target at 200, a
/// subroutine at 300, the slot's call at 340, the landing at 400.
const CASE: u64 = 0o100;
const TARGET: u64 = 0o200;
const SUB: u64 = 0o300;
const SLOT_SUB: u64 = 0o340;
const LANDING: u64 = 0o400;

/// One case: the driver calls the case (so that a POPJ has somewhere to
/// go), the case's branch and its delay slot, markers counting in M 20-27
/// on each path.
fn matrix_case(branch: Branch, n: bool, slot: SlotKind, lpc: bool) -> Prog {
    use muir::isa::asm::{ADD, HINT, OA_HIGH_SELECT, OA_LOW_SELECT, predicted};
    let mut p = Prog::default();
    let mark = |m: u64| ALU | ADD | a_src(ONE) | m_src(m) | m_dest(m);
    let nb = if n { N } else { 0 };
    // M 30: 0 for the condition taken (AEQM of M 34 and A 40).
    let (pdl, here) = (p.k(0o2000), p.k(0o1000));
    // Spare returns to the landing, for the paths that pop more than they
    // push.
    let landing = p.k(LANDING);
    for _ in 0..4 {
        p.op(ALU | SETA | a_src(landing) | fd(0o15));
    }
    p.op(ALU | SETA | a_src(pdl) | fd(0o14));
    p.op(ALU | SETA | a_src(pdl) | fd(0o13));
    p.op(ALU | SETA | a_src(here) | MD);
    // The driver: call the case; the case's paths come back to the landing.
    // The word before the case: SL's writer, or a filler.
    p.op(JUMP | ALWAYS | P | N | target(CASE - 1));
    p.fill(1);
    p.op(mark(0o27));
    p.op(JUMP | ALWAYS | N | target(LANDING));
    p.fill(1);
    while p.at() < CASE {
        p.fill(1);
    }
    let oa = p.k(1 << 6 | 1);
    let to = p.k(TARGET << 12);
    if matches!(branch, Branch::Sl) {
        // SL's writer, right before the jump.
        while p.at() < CASE - 1 {
            p.fill(1);
        }
        let _ = p.words.pop();
        p.op(ALU | SETA | a_src(to) | fd(0o16));
    }
    match branch {
        Branch::Jump => p.op(JUMP | ALWAYS | target(TARGET) | nb),
        Branch::Call => p.op(JUMP | ALWAYS | P | target(SUB) | nb),
        Branch::Popj => p.op(mark(0o20) | POPJ),
        Branch::Conditional { hint, taken } => {
            let m = if taken { M_ZERO } else { M_ONE };
            p.op(jcond(3)
                | m_src(m)
                | a_src(ZERO)
                | target(TARGET)
                | nb
                | if hint { HINT } else { 0 })
        }
        Branch::Dispatch { entry, predicted: (addr, pp, rr) } => {
            p.dmem.push((0o100, entry | if n { 1 << 14 } else { 0 }));
            p.op(disp(0o100) | predicted(addr, pp, rr) | if lpc { 1 << 25 } else { 0 })
        }
        Branch::Sl => p.op(JUMP | ALWAYS | target(0) | nb | OA_LOW_SELECT),
    };
    // The delay slot.
    let a = p.k(PHYS | 0o10);
    match slot {
        SlotKind::Plain => p.op(mark(0o21)),
        SlotKind::Popj => p.op(mark(0o21) | POPJ),
        SlotKind::Call => p.op(JUMP | ALWAYS | P | target(SLOT_SUB) | N),
        SlotKind::Push => p.op(ALU | SETA | a_src(here) | fd(0o15)),
        SlotKind::Pop => p.op(ALU | SETM | src(0o14) | m_dest(0o26)),
        SlotKind::Start => p.op(ALU | SETA | a_src(a) | START_READ),
        SlotKind::MapMd => p.op(ALU | SETM | src(0o11) | m_dest(0o26)),
        SlotKind::OaWrite => p.op(ALU | SETA | a_src(oa) | fd(0o17)),
        SlotKind::Transfer => p.op(JUMP | ALWAYS | N | target(TARGET + 4)),
    };
    // The fall-through: a selecting word when the slot writes OA-REG-HIGH.
    let select = if matches!(slot, SlotKind::OaWrite) { OA_HIGH_SELECT } else { 0 };
    p.op(ALU | ADD | a_src(0o1700) | m_src(0o22) | m_dest(0o22) | select);
    p.op(mark(0o23) | POPJ);
    p.fill(1);
    while p.at() < TARGET {
        p.fill(1);
    }
    // The target: a selecting word too.
    p.op(ALU | ADD | a_src(0o1700) | m_src(0o24) | m_dest(0o24) | select);
    p.op(mark(0o25) | POPJ);
    p.fill(2);
    p.op(mark(0o25) | POPJ);
    p.fill(1);
    let two = p.k(2);
    while p.at() < SUB {
        p.fill(1);
    }
    // The subroutine: out to the landing the second time, for a return
    // under N with LPC, which comes back to its call.
    p.op(mark(0o20));
    p.op(jcond(3) | m_src(0o20) | a_src(two) | target(LANDING) | N);
    p.fill(1);
    p.op(filler().raw() | POPJ);
    p.fill(1);
    while p.at() < SLOT_SUB {
        p.fill(1);
    }
    p.op(mark(0o26));
    p.op(filler().raw() | POPJ);
    p.fill(1);
    while p.at() < LANDING {
        p.fill(1);
    }
    p.op(mark(0o27));
    p.stop();
    p
}

/// Every case of the matrix.
fn matrix() -> Vec<(String, Prog)> {
    let mut cases = Vec::new();
    let entries = [
        (TARGET as u32, "jump"),
        (SUB as u32 | 1 << 15, "call"),
        (1 << 16, "return"),
        (1 << 16 | 1 << 15, "drop-through"),
    ];
    let predictions = [
        ((0, true, true), "drop"),
        ((TARGET, false, false), "jump"),
        ((SUB, true, false), "call"),
        ((0, false, true), "return"),
        ((0o777, false, false), "elsewhere"),
    ];
    for slot in SLOTS {
        for n in [false, true] {
            let mut branches =
                vec![(Branch::Jump, "jump".to_string()), (Branch::Call, "call".into())];
            branches.push((Branch::Popj, "POPJ".into()));
            branches.push((Branch::Sl, "SL jump".into()));
            for hint in [false, true] {
                for taken in [false, true] {
                    branches.push((
                        Branch::Conditional { hint, taken },
                        format!("jump hint {hint} taken {taken}"),
                    ));
                }
            }
            for (entry, e) in entries {
                for (pr, pn) in predictions {
                    branches.push((
                        Branch::Dispatch { entry, predicted: pr },
                        format!("dispatch {e} predicted {pn}"),
                    ));
                }
            }
            for (b, what) in branches {
                for lpc in [false, true] {
                    if lpc && !matches!(b, Branch::Dispatch { entry, .. } if entry & 1 << 15 != 0) {
                        continue;
                    }
                    cases.push((
                        format!("{what}, N {n}, LPC {lpc}, slot {slot:?}"),
                        matrix_case(b, n, slot, lpc),
                    ));
                }
            }
        }
    }
    cases
}

/// The end of `p` on `micro` (its select check off) and on the pipeline
/// with `mutation`.
fn ends(p: &Prog, mutation: Mutation) -> (Micro, Result<Pipeline, Halt>) {
    let u = micro_unchecked(p);
    let mut e = Pipeline::new(machine(p, REV16));
    e.mutation = mutation;
    e.boot();
    e.skip_sweep();
    for _ in 0..200_000 {
        if e.machine().opc == STOP as u16 {
            for _ in 0..16 {
                if let Err(h) = e.step() {
                    return (u, Err(h));
                }
            }
            while !e.quiet() {
                if let Err(h) = e.step() {
                    return (u, Err(h));
                }
            }
            return (u, Ok(e));
        }
        if let Err(h) = e.step() {
            return (u, Err(h));
        }
    }
    (u, Err(Halt::UnknownDest { pc: e.pc(), dest: 0 }))
}

/// **The speculation matrix** (the contract's §5): jump, call, POPJ, a
/// conditional jump, a dispatch and an SL jump, × N × the prediction × the
/// outcome × the delay slot's kind (a plain word, POPJ, a call, a push, a
/// pop, a start, `MAP(MD)`, an OA write, a transfer), with a selecting
/// word at the transfer's target and in the fall-through when the slot
/// writes OA-REG-HIGH, and a dispatch's return address under N and under N
/// with LPC: each ends as on `micro`. A restore of RD's stack copy left out
/// is caught.
#[test]
fn the_speculation_matrix_ends_as_on_micro() {
    let cases = matrix();
    let mut caught = 0;
    let (mut restores, mut changed, mut planned) = (0, 0, 0);
    for (what, p) in &cases {
        if std::env::var("MATRIX").is_ok() {
            eprintln!("{what}");
        }
        let (u, e) = ends(p, Mutation::None);
        let e = e.unwrap_or_else(|h| panic!("{what}: the pipeline halted: {h:?}"));
        let (ms, us) = (e.machine(), u.machine());
        if state(ms) != state(us) {
            diff_state(ms, us, 0);
        }
        assert_eq!(e.meters.rd_disagreements, 0, "{what}");
        restores += e.meters.restores_without_redirect;
        changed += e.meters.restores_changed;
        planned += e.meters.restores_changed_planned;
        let (u, e) = ends(p, Mutation::NoSpcRestore);
        if e.map_or(true, |e| state(e.machine()) != state(u.machine())) {
            caught += 1;
        }
    }
    assert!(caught > 0, "a stack copy not restored is never caught in {} cases", cases.len());
    // M-3 (the MP4 timing review §5): a restore of RD's copies without a
    // redirect leaves the values RD plans from in that clock as they stand.
    // A dispatch call whose entry sets N changes the stack's top, the pushed
    // return being the word after the dispatch rather than after its slot;
    // RD's word in that clock is the slot, which N nops, and plans from no
    // copy's value.
    eprintln!(
        "{} cases: restores without a redirect {restores}, changed {changed}, of them under a word RD plans {planned}",
        cases.len()
    );
    assert!(restores > 0, "the matrix never restores without a redirect");
    assert!(changed > 0, "the matrix never changes a copy by a restore");
    assert_eq!(planned, 0, "a restore without a redirect changed the copies RD plans from");
}

// --- The late squash, faulting starts, the mutations named by §13 ------------

/// A start that faults, read or write, then its check of condition 4 with N
/// or not, which calls a handler counting in M 26; the delay slot counts in
/// M 21 and the fall-through in M 22.
fn fault_check(write: bool, n: bool, faults: bool) -> Prog {
    let mut p = Prog::default();
    let mark = |m: u64| ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(m) | m_dest(m);
    let va = p.k(if faults { 0o1000 } else { PHYS | 0o20 });
    if write {
        p.op(ALU | SETM | m_src(M_ONE) | MD);
    }
    p.op(ALU | SETA | a_src(va) | if write { START_WRITE } else { START_READ });
    p.op(jcond(4) | P | target(SUBS[0]) | if n { N } else { 0 });
    p.op(mark(0o21));
    p.op(mark(0o22));
    p.stop();
    while p.at() < SUBS[0] {
        p.fill(1);
    }
    p.op(mark(0o26));
    p.op(filler().raw() | POPJ);
    p.fill(1);
    p
}

/// **The page-fault check right after a start that faults** (the
/// contract's §5, rule 5; A15b.3): read and write, the check's N clear and
/// set: the delay slot runs or not as N says, the words after leave
/// nothing, the call is made, and the state is `micro`'s; the check is held
/// a clock in EX, two bubbles with the redirect. A start that does not
/// fault costs nothing. A late squash that lets the check commit as
/// predicted is caught. A faulting start makes no queue entry and no fill.
#[test]
fn the_late_squash_after_a_faulting_start() {
    for write in [false, true] {
        for n in [false, true] {
            let p = fault_check(write, n, true);
            let e = same(&p);
            assert_eq!(e.meters.late_squashes, 1, "write {write}, N {n}");
            assert_eq!(e.machine().mmem[0o26], 1, "the handler ran");
            // The slot runs before the call with N clear; with N set it is
            // nopped and the call returns to it.
            assert_eq!(e.machine().mmem[0o21], 1, "the delay slot once");
            assert_eq!(
                (e.port.meters.writes, e.port.meters.fills),
                (0, 0),
                "nothing reached the port"
            );
            differs(&p, Mutation::NoLateSquash);
            let p = fault_check(write, n, false);
            let e = same(&p);
            assert_eq!(e.meters.late_squashes, 0, "no fault, no squash");
        }
    }
}

/// The clocks from a start's commit to its check's handler's first commit,
/// N clear: the late squash's two bubbles beyond the word in sequence.
#[test]
fn the_late_squash_costs_two_bubbles() {
    use muir::pipeline::Event;
    let p = fault_check(false, false, true);
    let (_, ev) = events(&p, 17, muir::pipeline::PortTiming::KRIA);
    let commit = |pc: u64| {
        ev.iter().find(|(_, x)| *x == Event::Commit(Some(pc as u16))).map(|e| e.0).unwrap()
    };
    let (start, handler, slot) = (commit(0), commit(SUBS[0]), commit(2));
    // In sequence the word after the slot would commit at start + 3. The
    // slot waits a clock in RD after the redirect (P2, the MP4 timing
    // review §4), so the bubble lies before it.
    eprintln!("start {start} slot {slot} handler {handler}");
    assert_eq!(slot, start + 4, "the check held a clock, its slot a clock after the redirect");
    assert_eq!(handler, start + 5, "two bubbles past the word in sequence");
}

/// **A wrong-path start leaves nothing** (the contract's §5, Choice 1): a
/// conditional jump predicted taken onto a write start, not taken. The
/// start made all the same is caught.
#[test]
fn a_squashed_start_is_caught() {
    use muir::isa::asm::HINT;
    let mut p = Prog::default();
    let (va, word) = (p.k(PHYS | 0o30), p.k(0o4321));
    p.op(ALU | SETA | a_src(word) | MD);
    // Predicted taken to the write, and not taken.
    let to = p.at() + 4;
    p.op(jcond(3) | m_src(M_ONE) | a_src(ZERO) | target(to) | HINT);
    p.fill(1);
    p.stop();
    assert_eq!(p.at(), to);
    p.op(ALU | SETA | a_src(va) | START_WRITE);
    p.fill(2);
    p.stop();
    let e = same(&p);
    assert_eq!(e.machine().main[0o30], 0);
    differs(&p, Mutation::SquashedStart);
}

/// **A delay slot N inhibits counts once** (MP2b ruling Q12): `Machine::cycles`
/// is the microcycles retired, the nopped ones once each. Counted twice, the
/// harness's comparison at the same `Machine::cycles` is off, caught here.
#[test]
fn a_nopped_slot_counts_once() {
    let mut p = Prog::default();
    for _ in 0..4 {
        let to = p.at() + 2;
        p.op(JUMP | ALWAYS | N | target(to));
        p.fill(1);
    }
    p.stop();
    let e = same(&p);
    assert_eq!(e.machine().cycles, e.meters.retired);
    let e = pipeline_with(&p, |e| e.mutation = Mutation::NopCountedTwice);
    assert_ne!(e.machine().cycles, e.meters.retired, "counted twice, caught");
}

/// **A squash restores RD's LC and stack copies** (A15b.16, the contract's
/// §5, Choice 2): opcode 3's handler holds a conditional return, hinted
/// taken and never taken, of the main loop's word, which arms RD's step of
/// LC on its prediction. As `micro`; a restore that leaves either copy as
/// the prediction left it is caught.
#[test]
fn a_squash_restores_rd_s_lc_and_stack_copies() {
    use muir::isa::asm::HINT;
    // Never taken: bit 0 of M 2, which stays 0.
    let never = JUMP | m_src(2) | R | HINT;
    for d in [false, true] {
        let m = ml_machine(d, &TWOS_THREES, &[never, filler().raw()], &|_| {});
        // With D on, D's decision takes RD's copies from the committed
        // registers: the faults are planted with D off.
        let mutations: &[Mutation] =
            if d { &[] } else { &[Mutation::NoLcRestore, Mutation::NoSpcRestore] };
        let e = ml_same_and_caught(m, mutations, "a wrong return");
        assert!(e.meters.mispredicted[0] > 0, "the hint was wrong");
    }
}

// --- The interrupt at every clock around a check (MP2b ruling Q13) ----------

/// Where the check stands, for [`interrupt_program`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Around {
    /// After a start that does not fault.
    Plain,
    /// Its M source `MD`, a word after the start, so that it waits in EX
    /// for the read's fill.
    HeldOnMd,
    /// On the fall-through of a conditional jump hinted taken and not
    /// taken: behind the squash.
    BehindWrongPrediction,
    /// After a start that faults: the late squash.
    AfterFaultingStart,
}

/// Three checks of condition 5 (page fault or interrupt), each after a
/// read start, each calling the handler at `SUBS[0]` that counts in M 26;
/// the first stands as `case` says, N set or clear. Its address with the
/// program.
fn interrupt_program(case: Around, n: bool) -> (Prog, u64) {
    use muir::isa::asm::{ADD, HINT};
    let mut p = Prog::default();
    let mark = |m: u64| ALU | ADD | a_src(ONE) | m_src(m) | m_dest(m);
    let ok = p.k(PHYS | 0o20);
    let bad = p.k(0o1000);
    let nbit = if n { N } else { 0 };
    if case == Around::BehindWrongPrediction {
        p.op(jcond(3) | m_src(M_ONE) | a_src(ZERO) | target(STOP) | HINT);
        p.fill(1);
    }
    let first = if case == Around::AfterFaultingStart { bad } else { ok };
    p.op(ALU | SETA | a_src(first) | START_READ);
    if case == Around::HeldOnMd {
        // The word right after the start reads the MD from before it and
        // waits for nothing; the one after that waits.
        p.fill(1);
    }
    let check = p.at();
    let md = if case == Around::HeldOnMd { SRC_MD } else { 0 };
    p.op(jcond(5) | P | target(SUBS[0]) | nbit | md);
    p.op(mark(0o21));
    for k in 0..2 {
        p.op(ALU | SETA | a_src(ok) | START_READ);
        p.op(jcond(5) | P | target(SUBS[0]) | nbit);
        p.op(mark(0o22 + k));
    }
    p.stop();
    while p.at() < SUBS[0] {
        p.fill(1);
    }
    p.op(mark(0o26));
    p.op(filler().raw() | POPJ);
    p.fill(1);
    (p, check)
}

/// Interrupts enabled, and timer 0's flag up from `at` on under its
/// interrupt enable: the test hook that raises the interrupt.
fn raise_at(m: &mut Machine, at: u64) {
    m.interrupt_control |= 1 << 27;
    m.timers.timer[0] = muir::machine::IntervalTimer {
        on: true,
        one_shot: true,
        interrupt_enable: true,
        period_us: 0,
        deadline_ns: at,
    };
}

/// The pipeline on `p` with the interrupt raised at clock `c` (never, for
/// `None`): its trace of microcycles to the stop and its end.
fn rtl_raised(p: &Prog, c: Option<u64>, mutation: Mutation) -> (Vec<Option<u16>>, Pipeline) {
    let mut e = Pipeline::new(machine(p, REV16));
    e.mutation = mutation;
    e.trace = Some(Vec::new());
    e.boot();
    e.skip_sweep();
    let at = c.map_or(u64::MAX, |c| c * e.period());
    raise_at(e.machine_mut(), at);
    for _ in 0..200_000 {
        if e.machine().opc == STOP as u16 {
            break;
        }
        e.tick().unwrap();
    }
    let t = e.trace.take().unwrap();
    for _ in 0..16 {
        e.step().unwrap();
    }
    let e = settled(e).unwrap_or_else(|(h, _)| panic!("halted settling: {h:?}"));
    (t, e)
}

/// `micro` on `p` with the interrupt raised before microcycle `m`: its
/// trace to the stop and its end.
fn micro_raised(p: &Prog, mc: u64) -> (Vec<Option<u16>>, Micro) {
    let mut u = Micro::new(machine(p, REV16));
    u.boot();
    raise_at(u.machine_mut(), u64::MAX);
    let mut t = Vec::new();
    for _ in 0..200_000 {
        if u.machine().cycles + 1 == mc {
            raise_at(u.machine_mut(), 0);
        }
        if u.machine().opc == STOP as u16 {
            break;
        }
        u.step().unwrap();
        t.push(u.executed());
    }
    for _ in 0..16 {
        u.step().unwrap();
    }
    (t, u)
}

/// Both traces to the stop's first run.
fn to_stop(mut t: Vec<Option<u16>>) -> Vec<Option<u16>> {
    if let Some(k) = t.iter().position(|&p| p == Some(STOP as u16)) {
        t.truncate(k + 1);
    }
    t
}

/// For each clock `c` from 8 before the check enters CS to 8 after it
/// leaves WB: `rtl` with the interrupt raised at `c`, against `micro` with
/// it raised before m(c), the microcycle of the first word whose last EX
/// clock is at or after `c`, read from `rtl`'s own run without it. The
/// number of clocks at which `mutation`'s end differs from `micro`'s.
fn interrupt_around(p: &Prog, check: u64, mutation: Mutation) -> (usize, usize) {
    // The reference run: the stages and the commits, clock by clock.
    let mut e = Pipeline::new(machine(p, REV16));
    e.trace = Some(Vec::new());
    e.boot();
    e.skip_sweep();
    raise_at(e.machine_mut(), u64::MAX);
    let (mut in_cs, mut in_wb) = (None, None);
    let mut commits: Vec<(u64, u64)> = Vec::new();
    for _ in 0..200_000 {
        if e.machine().opc == STOP as u16 {
            break;
        }
        let committed = e.trace.as_ref().unwrap().len();
        e.tick().unwrap();
        let k = e.clock();
        let s = e.stages();
        if s[0] == Some(Some(check as u16)) && in_cs.is_none() {
            in_cs = Some(k);
        }
        if s[3] == Some(Some(check as u16)) {
            in_wb = Some(k);
        }
        // The microcycle's number is its place in the trace, nopped ones
        // included: `Machine::cycles` as `micro` counts it.
        let now = e.trace.as_ref().unwrap().len();
        if now != committed {
            commits.push((k, now as u64));
        }
    }
    let (from, to) = (
        in_cs.expect("the check in CS").saturating_sub(8).max(1),
        in_wb.expect("the check in WB") + 1 + 8,
    );
    let mut differ = 0;
    for c in from..=to {
        let mc = commits.iter().find(|&&(k, _)| k >= c).expect("a word commits after c").1;
        let (ut, u) = micro_raised(p, mc);
        let (et, e) = rtl_raised(p, Some(c), mutation);
        let same =
            to_stop(ut.clone()) == to_stop(et.clone()) && state(e.machine()) == state(u.machine());
        if mutation == Mutation::None {
            assert_eq!(to_stop(et), to_stop(ut), "clock {c}, m(c) {mc}: the microcycles");
            assert_eq!(state(e.machine()), state(u.machine()), "clock {c}, m(c) {mc}: the end");
        }
        if !same {
            differ += 1;
        }
    }
    ((to - from + 1) as usize, differ)
}

/// **An interrupt raised at every clock around a check** (the contract's
/// §5 matrix, MP2b ruling Q13): the check held on `MD`, behind a wrong
/// prediction, after a faulting start, N clear and set; at each clock `rtl`
/// ends as `micro` raised before the matching microcycle. A check that
/// samples in RD is caught.
#[test]
fn an_interrupt_at_every_clock_around_a_check() {
    for case in
        [Around::Plain, Around::HeldOnMd, Around::BehindWrongPrediction, Around::AfterFaultingStart]
    {
        for n in [false, true] {
            let (p, check) = interrupt_program(case, n);
            let (clocks, differ) = interrupt_around(&p, check, Mutation::None);
            assert_eq!(differ, 0, "{case:?}, N {n}");
            let (_, caught) = interrupt_around(&p, check, Mutation::InterruptSampledInRd);
            eprintln!("{case:?}, N {n}: {clocks} clocks; sampled in RD differs at {caught}");
            assert!(caught > 0, "{case:?}, N {n}: sampling in RD is caught");
            if case == Around::HeldOnMd {
                assert!(caught > clocks / 2, "{case:?}, N {n}: through the hold");
            }
        }
    }
}

// --- The halt, the checkpoint and the single step (A15b.13) ------------------

/// The pipeline on `p` to its stop: its trace and end, from a run halted
/// at clock `halt` (none for `None`) and, with `checkpoint`, carried on
/// from a checkpoint taken there in a pipeline of its own. At the halt the
/// state is `micro`'s at the same `Machine::cycles`.
fn halted_run(
    p: &Prog,
    halt: Option<u64>,
    checkpoint: bool,
    u: &mut Micro,
) -> (Vec<Option<u16>>, Pipeline) {
    halted_run_with(p, halt, checkpoint, u, &|_| {})
}

/// [`halted_run`] with each pipeline set by `set`.
fn halted_run_with(
    p: &Prog,
    halt: Option<u64>,
    checkpoint: bool,
    u: &mut Micro,
    set: &dyn Fn(&mut Pipeline),
) -> (Vec<Option<u16>>, Pipeline) {
    let mut e = Pipeline::new(machine(p, REV16));
    set(&mut e);
    e.trace = Some(Vec::new());
    e.boot();
    e.skip_sweep();
    let mut t = Vec::new();
    let mut stopped = false;
    for _ in 0..20_000 {
        if e.machine().opc == STOP as u16 {
            stopped = true;
            break;
        }
        if Some(e.clock() + 1) == halt {
            e.machine_mut().clock_control.run = false;
            for _ in 0..10_000 {
                if e.is_halted() {
                    break;
                }
                e.tick().unwrap();
            }
            assert!(
                e.is_halted(),
                "the halt at clock {halt:?}, checkpoint {checkpoint}, drained: stages {:?}, port quiet {}",
                e.stages(),
                e.quiet()
            );
            // Drained: the state between two microcycles, micro's.
            assert!(u.machine().cycles <= e.machine().cycles, "halts in order");
            while u.machine().cycles < e.machine().cycles {
                u.step().unwrap();
            }
            let (el, ul) = (e.landed(), u.landed());
            if state(&el) != state(&ul) {
                eprintln!(
                    "halted at clock {halt:?}: against micro's state at {} microcycles",
                    e.machine().cycles
                );
                diff_state(&el, &ul, 0);
                panic!(
                    "the states differ, though diff_state shows nothing: {:?}",
                    (
                        e.machine().vma,
                        u.machine().vma,
                        e.machine().md,
                        u.machine().md,
                        e.machine().lc,
                        u.machine().lc,
                        e.machine().interrupt_control,
                        u.machine().interrupt_control,
                        e.machine().q,
                        u.machine().q
                    )
                );
            }
            if checkpoint {
                let mut w = muir::checkpoint::Writer::new();
                e.save(&mut w);
                let bytes = w.finish();
                t.extend(e.trace.take().unwrap());
                let mut f = Pipeline::new(machine(p, REV16));
                set(&mut f);
                f.load(&mut muir::checkpoint::Reader::for_word_bits(&bytes, 40)).unwrap();
                assert_eq!(f.clock(), e.clock(), "the clock goes on from the checkpoint's time");
                f.skip_sweep();
                f.trace = Some(Vec::new());
                e = f;
            }
            e.machine_mut().clock_control.run = true;
        }
        e.tick().unwrap();
    }
    assert!(
        stopped,
        "halted at clock {halt:?}, checkpoint {checkpoint}: the program reached its stop"
    );
    t.extend(e.trace.take().unwrap());
    for _ in 0..16 {
        e.step().unwrap();
    }
    let e = settled(e).unwrap_or_else(|(h, _)| panic!("halted settling: {h:?}"));
    (to_stop(t), e)
}

/// **A halt at every clock resumes to the same end** (A15b.13): random
/// programs halted at each clock of their run, drained to `micro`'s state
/// at the same microcycle, run on, end as the run without the halt, the
/// microcycles in the same order; and so from a checkpoint taken at the
/// halt into a pipeline of its own.
#[test]
fn a_halt_at_every_clock_resumes_to_the_same_end() {
    let mut halts = 0;
    for seed in 1..=12 {
        let p = random_program(seed, ALL);
        let fresh = || {
            let mut u = Micro::new(machine(&p, REV16));
            u.boot();
            u
        };
        let (t0, e0) = halted_run(&p, None, false, &mut fresh());
        let clocks = e0.clock();
        for checkpoint in [false, true] {
            let mut u = fresh();
            for c in 1..clocks {
                if checkpoint && c % 3 != 0 {
                    continue;
                }
                let (t, e) = halted_run(&p, Some(c), checkpoint, &mut u);
                if let Some(k) = (0..t.len().min(t0.len())).find(|&k| t[k] != t0[k]) {
                    panic!(
                        "seed {seed}, halted at clock {c}, checkpoint {checkpoint}: microcycle {k}: {:?} against {:?}",
                        &t[k.saturating_sub(4)..(k + 4).min(t.len())],
                        &t0[k.saturating_sub(4)..(k + 4).min(t0.len())]
                    );
                }
                assert_eq!(t.len(), t0.len(), "seed {seed}, halted at clock {c}: the microcycles");
                if state(e.machine()) != state(e0.machine()) {
                    eprintln!(
                        "seed {seed}, halted at {c}, checkpoint {checkpoint}: the end differs from the run without the halt"
                    );
                    diff_state(e.machine(), e0.machine(), seed);
                    panic!("the ends differ, though diff_state shows nothing");
                }
                halts += 1;
            }
        }
    }
    eprintln!("{halts} halts");
}

/// **The microcode single step** (A15b.13): from a halt, each step runs one
/// word through the four stages and halts, one microcycle, the state
/// `micro`'s after as many; a run on from the steps ends as one without.
#[test]
fn a_single_step_runs_one_microcycle() {
    for seed in 1..=6 {
        let p = random_program(seed, ALL);
        let (t0, e0) = halted_run(&p, None, false, &mut Micro::new(machine(&p, REV16)));
        let mut u = Micro::new(machine(&p, REV16));
        u.boot();
        let mut e = Pipeline::new(machine(&p, REV16));
        e.trace = Some(Vec::new());
        e.boot();
        e.skip_sweep();
        e.machine_mut().clock_control.run = false;
        while !e.is_halted() {
            e.tick().unwrap();
        }
        let mut steps = 0;
        while e.machine().opc != STOP as u16 && steps < 400 {
            let before = e.trace.as_ref().unwrap().len();
            e.machine_mut().clock_control.step = true;
            e.tick().unwrap();
            e.machine_mut().clock_control.step = false;
            for _ in 0..10_000 {
                if e.is_halted() {
                    break;
                }
                e.tick().unwrap();
            }
            assert!(e.is_halted(), "seed {seed}, step {steps}: the step halted");
            assert_eq!(
                e.trace.as_ref().unwrap().len(),
                before + 1,
                "seed {seed}, step {steps}: one microcycle"
            );
            while u.machine().cycles < e.machine().cycles {
                u.step().unwrap();
            }
            let (el, ul) = (e.landed(), u.landed());
            if state(&el) != state(&ul) {
                diff_state(&el, &ul, seed);
            }
            steps += 1;
        }
        assert!(steps > 20, "seed {seed}: the steps reached the stop");
        e.machine_mut().clock_control.run = true;
        for _ in 0..20_000 {
            if e.machine().opc == STOP as u16 {
                break;
            }
            e.tick().unwrap();
        }
        let t = to_stop(e.trace.take().unwrap());
        assert_eq!(t, t0, "seed {seed}: the microcycles");
        for _ in 0..16 {
            e.step().unwrap();
        }
        let e = settled(e).unwrap_or_else(|(h, _)| panic!("halted settling: {h:?}"));
        if state(e.machine()) != state(e0.machine()) {
            diff_state(e.machine(), e0.machine(), seed);
        }
    }
}

// --- The PDL buffer (A15b.10), the TLB's rows (A15b.3) ----------------------

/// **The PDL buffer across its wrap** (A15b.10; A15b.3's row): from the
/// pointer two below the top, a push a word, each followed by reads at the
/// pointer one, two and three words after it: the first reads the old word,
/// the second takes WB's forward, the third the forward that replaces a
/// read of the address written in the same clock; and a word that reads
/// and pushes each clock, across the wrap. Functional source 25 is
/// `C-PDL-BUFFER-POINTER`. As `micro`; the forward left out
/// is caught.
#[test]
fn the_pdl_buffer_across_its_wrap() {
    use muir::isa::asm::{ADD, src};
    let top = Word::from(REV16.pdl_mask());
    let mut p = Prog::default();
    let start = p.k(top - 2);
    p.op(ALU | SETA | a_src(start) | fd(0o14));
    p.fill(2);
    for k in 0..5u64 {
        let v = p.k(0o1000 + k);
        p.op(ALU | SETA | a_src(v) | fd(0o11));
        for j in 0..3 {
            p.op(ALU | SETM | src(0o25) | a_dest(0o1600 + 4 * k + j));
        }
    }
    p.op(ALU | SETA | a_src(start) | fd(0o14));
    p.fill(2);
    for k in 0..6 {
        p.op(ALU | ADD | a_src(ONE) | src(0o25) | fd(0o11));
        p.op(ALU | SETM | src(0o25) | a_dest(0o1640 + k));
    }
    p.stop();
    let e = same(&p);
    let m = e.machine();
    // Pushed at top - 1, top, 0, 1, 2: the third read of each push its word.
    for k in 0..5u64 {
        assert_eq!(
            m.amem[(0o1600 + 4 * k + 2) as usize],
            0o1000 + k,
            "push {k}'s word, read two words on"
        );
    }
    assert_eq!(m.pdl_pointer, 3, "the pointer wrapped: two below the top, six pushes");
    differs(&p, Mutation::NoPdlForward);
}

/// A TLB entry, read and write, status 4, for `frame` (A14.5's format).
const fn rw_entry(frame: u64) -> Word {
    0b11 << 28 | 0o1460 << 18 | frame
}

/// The page-table walk's tables for `va`: the directory at frame 8 (word
/// 220 written by the program), its page table at frame 9, the page at
/// `frame`.
fn walkable(p: &mut Prog, va: Word, frame: u64) {
    p.main.push(((8 << 10) + (va >> 20) as usize, rw_entry(9)));
    p.main.push(((9 << 10) + ((va >> 10) & 0o1777) as usize, rw_entry(frame)));
}

/// The program's first words: word 220, the directory base, at frame 8.
fn set_directory(p: &mut Prog) {
    let (dir, at) = (p.k(8), p.k(REGISTER_PAGE | 0o220));
    p.op(ALU | SETA | a_src(dir) | MD);
    p.op(ALU | SETA | a_src(at) | START_WRITE);
    p.fill(2);
}

/// A `WRITE-MAP` operation's direct write of `va`'s entry, `frame`.
fn direct_write(p: &mut Prog, va: Word, frame: u64) {
    let (a, e) = (p.k(va), p.k(1 << 32 | rw_entry(frame)));
    p.op(ALU | SETA | a_src(a) | MD);
    p.op(ALU | SETA | a_src(e) | fd(0o23));
}

/// **A start right after a `WRITE-MAP` operation translates through the
/// new entry** (A15b.3's row): held a clock, it reads the word of the
/// frame just written; one a word later is not held. As `micro`.
#[test]
fn a_start_after_a_map_write_translates_through_the_new_entry() {
    for gap in [0, 1] {
        let mut p = Prog::default();
        let x: Word = 0o4000;
        p.main.push(((3 << 10) + 5, 0o777));
        direct_write(&mut p, x, 3);
        p.fill(gap);
        let a = p.k(x + 5);
        p.op(ALU | SETA | a_src(a) | START_READ);
        p.fill(2);
        p.op(ALU | SETM | SRC_MD | m_dest(0o26));
        p.stop();
        let e = same(&p);
        assert_eq!(e.machine().mmem[0o26], 0o777, "gap {gap}: the new entry's frame");
        assert_eq!(e.meters.map_hold, if gap == 0 { 1 } else { 0 }, "gap {gap}: the hold");
    }
}

/// **A wrong-path `MAP(MD)` inside a fiddle's window evicts nothing**
/// (A15b's checks; the contract's §5, rule 3): an entry written directly,
/// then a conditional jump hinted taken onto a `MAP(MD)` of an address that
/// walks into the same TLB index, not taken; the read through the direct
/// entry finds it. A squashed word's lookup made is caught.
#[test]
fn a_wrong_path_map_md_evicts_nothing() {
    use muir::isa::asm::{HINT, src};
    let mut p = Prog::default();
    let x: Word = 0o4000;
    let entries = machine(&Prog::default(), REV16).tlb.len() as Word;
    let y: Word = x + (entries << 10);
    p.main.push(((3 << 10) + 5, 0o777));
    walkable(&mut p, y, 5);
    set_directory(&mut p);
    direct_write(&mut p, x, 3);
    p.fill(1);
    let ya = p.k(y);
    p.op(ALU | SETA | a_src(ya) | MD);
    let t = p.at() + 8;
    p.op(jcond(3) | m_src(M_ONE) | a_src(ZERO) | target(t) | HINT);
    p.fill(1);
    let a = p.k(x + 5);
    p.op(ALU | SETA | a_src(a) | START_READ);
    p.fill(2);
    p.op(ALU | SETM | SRC_MD | m_dest(0o26));
    p.stop();
    while p.at() < t {
        p.fill(1);
    }
    p.op(ALU | SETM | src(0o11) | m_dest(0o25));
    p.stop();
    let e = same(&p);
    assert_eq!(e.machine().mmem[0o26], 0o777, "the direct entry's frame");
    let evicted: u64 = e.machine().tlb.evicted.iter().flatten().sum();
    assert_eq!(evicted, 0, "nothing evicted");
    assert!(e.meters.mispredicted[0] > 0, "the hint was wrong");
    differs(&p, Mutation::SquashedLookup);
}

// --- Posted writes (A15b.5) ----------------------------------------------------

/// `p` on `micro` and on the pipeline, each machine set up by `setup` and
/// the pipeline by `set`: the same end. The pipeline.
fn same_on(p: &Prog, setup: &dyn Fn(&mut Machine), set: &dyn Fn(&mut Pipeline)) -> Pipeline {
    let mut m = machine(p, REV16);
    setup(&mut m);
    let u = match run_engine(Micro::new(m.clone())) {
        Ok(u) => u,
        Err((h, _)) => panic!("micro halted: {h:?}"),
    };
    let e = ends_on(m, set);
    if state(e.machine()) != state(u.machine()) {
        diff_state(e.machine(), u.machine(), 0);
    }
    e
}

/// The pipeline on `m`, set by `set`, to its stop and settled.
fn ends_on(m: Machine, set: &dyn Fn(&mut Pipeline)) -> Pipeline {
    let mut e = Pipeline::new(m);
    set(&mut e);
    match run_engine(e).and_then(settled) {
        Ok(e) => e,
        Err((h, _)) => panic!("the pipeline halted: {h:?}"),
    }
}

/// The seeded memory model that answers each write 1 to 50 clocks late.
fn late(seed: u64) -> muir::pipeline::port::LateModel {
    muir::pipeline::port::LateModel { seed, most: 50, errors: 0 }
}

/// **The read rule** (A15b.5): a write, then a read miss of its line, the
/// word itself and its neighbour, against the model that answers B 1 to 50
/// clocks late and lets a read pass a write: the read returns the word
/// written, at every seed. A rule that waits only for the write's issue
/// returns main memory's old word at some.
#[test]
fn the_read_rule_returns_the_word_written() {
    let mut p = Prog::default();
    p.main.push((0o41, 0o1111));
    p.write(0o5252, PHYS | 0o40);
    p.read(PHYS | 0o40, 0o26);
    p.read(PHYS | 0o41, 0o27);
    p.stop();
    let mut stale = 0;
    for seed in 1..=100 {
        let e = same_on(&p, &|_| {}, &|e| e.port.model = Some(late(seed)));
        assert_eq!(e.machine().mmem[0o26], 0o5252, "seed {seed}");
        assert!(e.port.meters.read_rule_reads > 0, "seed {seed}: the read waited");
        let e = ends_on(machine(&p, REV16), &|e| {
            e.port.model = Some(late(seed));
            e.port.read_rule_waits_for_responses = false;
        });
        if e.machine().mmem[0o26] != 0o5252 {
            stale += 1;
        }
    }
    eprintln!("a rule waiting for the issue alone read stale at {stale} seeds of 100");
    assert!(stale > 0, "the broken rule is caught");
}

/// A port of the Kria's period, its writes answered after `write_ns`.
fn port(write_ns: u64) -> muir::pipeline::port::Port {
    let time = Pipeline::new(machine(&Prog::default(), REV16)).time();
    let timing = muir::pipeline::PortTiming { read_ns: 247, write_ns, occupancy_ns: 10 };
    muir::pipeline::port::Port::new(timing, time, 65_536)
}

/// **Nine writes then a stall** (A15b.5): the queue holds eight; the ninth
/// waits for the first's acceptance, which frees its entry.
#[test]
fn the_ninth_write_waits_for_the_first_s_acceptance() {
    let mut m = machine(&Prog::default(), REV16);
    let mut port = port(130);
    for k in 0..8 {
        assert!(port.write(0, 0o100 + 8 * k, k.into()), "the queue has room");
    }
    assert!(!port.write(0, 0o200, 0o7), "the ninth waits");
    assert_eq!(port.depths(), (8, 0));
    port.tick(&mut m, 1);
    assert_eq!(port.depths(), (7, 1), "the first accepted");
    assert!(port.write(1, 0o200, 0o7), "the ninth goes in");
}

/// **Seventeen writes accepted, none answered** (A15b.5): the in-flight
/// list holds sixteen; the seventeenth is not issued until the first's
/// response.
#[test]
fn the_seventeenth_write_is_not_issued() {
    let mut m = machine(&Prog::default(), REV16);
    let mut port = port(100_000);
    let mut now = 0;
    for k in 0..17u32 {
        while !port.write(now, 0o100 + 8 * k, k.into()) {
            now += 1;
            port.tick(&mut m, now);
        }
    }
    for _ in 0..200 {
        now += 1;
        port.tick(&mut m, now);
    }
    assert_eq!(port.depths(), (1, 16), "sixteen in flight, the seventeenth queued");
    assert_eq!(port.meters.writes, 16);
    // The first answers: the seventeenth goes.
    let first = port.clocks().write;
    while port.depths().0 == 1 {
        now += 1;
        port.tick(&mut m, now);
        assert!(now < 400 + first, "the seventeenth issued after the first's response");
    }
    assert!(now >= first, "not before the first's response");
}

/// A port at the period `period`, in 0.5 ns, and the timing `timing`.
fn port_at(period: u64, timing: muir::pipeline::PortTiming) -> muir::pipeline::port::Port {
    let mut e = Pipeline::new(machine(&Prog::default(), REV16));
    e.configure(period, timing, 65_536);
    muir::pipeline::port::Port::new(timing, e.time(), 65_536)
}

/// The clocks from the first write's accept to the second's, both queued
/// with their words before the first clock, at bus addresses `a` and `b`;
/// the port after.
fn accept_gap(
    period: u64,
    timing: muir::pipeline::PortTiming,
    a: u32,
    b: u32,
) -> (u64, muir::pipeline::port::Port) {
    let mut m = machine(&Prog::default(), REV16);
    let mut port = port_at(period, timing);
    for (bus, word) in [(a, 0o1111), (b, 0o2222)] {
        assert!(port.write(0, bus, word), "the queue has room");
    }
    let mut accepted = Vec::new();
    for now in 1..20 {
        let before = port.meters.writes;
        port.tick(&mut m, now);
        if port.meters.writes > before {
            accepted.push(now);
        }
    }
    assert_eq!(accepted.len(), 2, "both accepted");
    (accepted[1] - accepted[0], port)
}

/// **A packed word that crosses an 8-byte boundary is two data beats**
/// (A15b.5; MP4 ruling Q1, C1): word `w`'s five bytes start at byte `5w`,
/// and run past their first beat when `5w mod 8` is 4 to 7, a word that
/// crosses 4 KiB among them. The write channel takes the next accept no
/// sooner than max(occupancy, beats) clocks after a write's accept: one
/// clock more behind a two-beat write at the Kria's 10 ns and the Arty's
/// 20 ns, where the occupancy is a clock, and none at the Kria's 8.5 ns
/// nor the DE25's 15 ns, where it is two and three. A word of the frame
/// buffer's window is four bytes and one beat. The meter counts the
/// accepts the beats delayed and the clocks.
#[test]
fn a_two_beat_write_holds_the_next_accept_by_its_second_beat() {
    use muir::pipeline::PortTiming;
    let fb = muir::tlb::DEVICE | 1;
    // (period, timing, the occupancy in clocks).
    for (period, timing, o) in [
        (20, PortTiming::KRIA, 1),
        (40, PortTiming::ARTY, 1),
        (17, PortTiming::KRIA, 2),
        (30, PortTiming::DE25, 3),
    ] {
        for (first, beats) in
            [(0o100, 1), (0o101, 2), (0o102, 1), (0o103, 2), (0o104, 2), (819, 2), (fb, 1)]
        {
            let what = format!("period {period}, first write at {first:o}");
            let (gap, port) = accept_gap(period, timing, first, 0o200);
            assert_eq!(gap, o.max(beats), "{what}: the clocks between the accepts");
            let delayed = u64::from(beats > o);
            assert_eq!(port.meters.beat_delays, delayed, "{what}: accepts delayed by beats");
            assert_eq!(port.meters.beat_delay_clocks, delayed, "{what}: their clocks");
            assert_eq!(port.meters.two_beat_writes, u64::from(beats == 2), "{what}: two-beat");
        }
    }
}

/// **The response comes `w` after the accept, at its first beat** (MP4
/// ruling Q1, C1): a two-beat write's main memory word lands as a one-beat
/// write's does, `⌈w/P⌉` clocks after its accept.
#[test]
fn a_two_beat_write_is_answered_from_its_accept() {
    use muir::pipeline::PortTiming;
    for first in [0o100, 0o101] {
        let mut m = machine(&Prog::default(), REV16);
        let mut port = port_at(20, PortTiming::KRIA);
        assert!(port.write(0, first, 0o4321), "room");
        let mut landed = None;
        for now in 1..40 {
            port.tick(&mut m, now);
            if landed.is_none() && m.main[first as usize] == 0o4321 {
                landed = Some(now);
            }
        }
        assert_eq!(landed, Some(1 + port.clocks().write), "word {first:o}");
    }
}

/// **Consecutive words written at 10 ns** (MP4 ruling Q1: slice 3's
/// goldens on consecutive words): forty writes to consecutive words, half
/// of them two-beat, against the model that answers each write 1 to 200
/// clocks late, so that the in-flight list fills and its writes are
/// released together: they end as on `micro` at every seed, and the meter
/// counts two-beat writes and the accepts their second beats delayed. At
/// 8.5 ns, and with every write taken as one beat, none is delayed.
#[test]
fn consecutive_words_written_at_10_ns_wait_for_the_second_beats() {
    use muir::pipeline::PortTiming;
    let mut p = Prog::default();
    for k in 0..40 {
        p.write(0o1000 + k, PHYS | (0o2000 + k));
    }
    p.read(PHYS | 0o2047, 0o26);
    p.stop();
    let at = |period: u64, one_beat: bool, seed: u64| {
        same_on(&p, &|_| {}, &|e: &mut Pipeline| {
            e.configure(period, PortTiming::KRIA, 65_536);
            e.port.one_beat_writes = one_beat;
            e.port.model = Some(muir::pipeline::port::LateModel { seed, most: 200, errors: 0 });
        })
    };
    let (mut delays, mut clocks) = (0, 0);
    for seed in 1..=10 {
        let e = at(20, false, seed);
        assert_eq!(e.machine().mmem[0o26], 0o1047, "seed {seed}: the last word read back");
        let m = e.port.meters;
        assert_eq!(m.two_beat_writes, 20, "seed {seed}: half of forty consecutive words");
        delays += m.beat_delays;
        clocks += m.beat_delay_clocks;
        assert_eq!(m.beat_delays, m.beat_delay_clocks, "seed {seed}: one clock each at 10 ns");
        assert_eq!(at(20, true, seed).port.meters.beat_delays, 0, "seed {seed}: one beat each");
        assert_eq!(at(17, false, seed).port.meters.beat_delays, 0, "seed {seed}: 8.5 ns");
    }
    eprintln!("at 10 ns, ten seeds: {delays} accepts delayed by beats, {clocks} clocks");
    assert!(delays > 0, "two-beat writes with a write behind them");
}

/// **A write to a line whose fill is in flight lands after the fill**
/// (A15b.5): a table read's miss fills a line (the processor's own reads
/// hold every start behind them, so a fill not for `MD` is the case), and
/// a write to the line waits for the fill; a read of the word after hits
/// with the word written, and main memory takes it at the response. A
/// write that goes ahead of the fill leaves the line's old word cached.
#[test]
fn a_write_behind_a_fill_lands_after_it() {
    use muir::pipeline::port::{Answer, Reader};
    for ahead in [false, true] {
        let mut m = machine(&Prog::default(), REV16);
        m.main[0o101] = 0o1111;
        // A write answered after a fill's whole time, so that one gone
        // ahead reaches main memory after the fill has read it.
        let mut port = port(400);
        port.write_waits_for_fill = !ahead;
        let mut now = 1;
        assert_eq!(port.read(now, 0o100, Reader::Table), Answer::Filling);
        // The fill issued: the read rule has nothing to wait for.
        now += 1;
        port.tick(&mut m, now);
        while !port.write(now, 0o101, 0o5252) {
            now += 1;
            port.tick(&mut m, now);
        }
        if !ahead {
            assert!(now > port.clocks().read, "the write waited for the fill");
        }
        for _ in 0..200 {
            now += 1;
            port.tick(&mut m, now);
        }
        let hit = match port.read(now, 0o101, Reader::Processor) {
            Answer::At(_, w) => w,
            a => panic!("a hit: {a:?}"),
        };
        assert_eq!(m.main[0o101], 0o5252, "ahead {ahead}: main memory took the write");
        if ahead {
            assert_ne!(hit, 0o5252, "a write ahead of the fill leaves the old word cached: caught");
        } else {
            assert_eq!(hit, 0o5252, "the cache holds the word written");
        }
    }
}

/// **Word 225** (A15b.1, A15b.5): 0 over a run of writes; a planted error
/// response counts there, the program reads it, and a write clears it.
#[test]
fn word_225_counts_error_responses() {
    let mut p = Prog::default();
    for k in 0..4 {
        p.write(0o100 + k, PHYS | (0o200 + 8 * k));
    }
    p.fill(40);
    p.read(REGISTER_PAGE | 0o225, 0o26);
    p.write(0, REGISTER_PAGE | 0o225);
    p.read(REGISTER_PAGE | 0o225, 0o27);
    p.stop();
    let e = ends_on(machine(&p, REV16), &|e| e.port.model = Some(late(7)));
    assert_eq!((e.machine().mmem[0o26], e.machine().mmem[0o27]), (0, 0), "no error, 0");
    let e = ends_on(machine(&p, REV16), &|e| {
        e.port.model = Some(muir::pipeline::port::LateModel { errors: 1, ..late(7) })
    });
    assert_eq!(e.machine().mmem[0o26], 1, "the planted error counted");
    assert_eq!(e.machine().mmem[0o27], 0, "and cleared by the write");
}

/// Block-disk with a pack whose block `k` holds `k << 16 | word`.
fn with_block_disk(m: &mut Machine) {
    use muir::block_disk::{self, BlockDisk};
    use muir::disk_unit::{BLOCK_WORDS, Geometry};
    let mut d = muir::disk_image::Disk::blank(Geometry::T300.blocks());
    for k in 0..32u32 {
        let block: [u32; BLOCK_WORDS] = std::array::from_fn(|w| k << 16 | w as u32);
        assert!(d.write_block(k, &block));
    }
    let mut b = BlockDisk::new(block_disk::BLOCK_NS);
    b.attach(d);
    m.block_disk = Some(b);
}

/// Block-disk's registers written by the program: the command list at
/// 777, the disk address `da`, the command, START.
fn block_disk_command(p: &mut Prog, list: u64, da: u64, command: u64) {
    p.write(list, REGISTER_PAGE | 0o201);
    p.write(da, REGISTER_PAGE | 0o202);
    p.write(command, REGISTER_PAGE | 0o200);
    p.write(0, REGISTER_PAGE | 0o203);
}

/// **Block-disk behind the posted writes** (A15b.5; MP2b ruling Q11): a
/// processor write to a word still queued at START, a disk read into it,
/// the model delaying the write: after the transfer the word is the
/// disk's, and the processor reads it so. A disk write of a page the
/// processor has just written takes every word written before START. A
/// START that leaves the writes behind it is caught.
#[test]
fn block_disk_s_start_lands_the_posted_writes() {
    const READ: u64 = 0;
    const WRITE: u64 = 0o11;
    const FOUR_BYTE: u64 = 1 << 12;
    let disk_word = |k: u64, w: u64| muir::machine::UNBOXED_TAG | k << 16 | w;
    // A read into a word the processor has just written.
    let mut p = Prog::default();
    p.main.push((0o777, 0o2000));
    p.write(0o4321, PHYS | 0o2005);
    block_disk_command(&mut p, 0o777, 3, READ | FOUR_BYTE);
    p.read(PHYS | 0o2005, 0o26);
    p.stop();
    let mut caught = 0;
    for seed in 1..=20 {
        let e = same_on(&p, &with_block_disk, &|e| e.port.model = Some(late(seed)));
        assert_eq!(e.machine().mmem[0o26], disk_word(3, 5), "seed {seed}: the disk's word read");
        assert_eq!(e.machine().main[0o2005], disk_word(3, 5), "seed {seed}");
        let mut m = machine(&p, REV16);
        with_block_disk(&mut m);
        let e = ends_on(m, &|e| {
            e.port.model = Some(late(seed));
            e.mutation = Mutation::NoLandAtStart;
        });
        if e.machine().main[0o2005] != disk_word(3, 5) {
            caught += 1;
        }
    }
    assert!(caught > 0, "a START behind the writes is caught");
    // A disk write of words just written, read back into another page.
    let mut p = Prog::default();
    p.main.push((0o776, 0o4000));
    p.main.push((0o775, 0o6000));
    for k in 0..6 {
        p.write(0o100 + k, PHYS | (0o4000 + 0o200 * k));
    }
    block_disk_command(&mut p, 0o776, 20, WRITE | FOUR_BYTE);
    p.stop();
    let on_disk = |e: &Pipeline| -> Vec<u64> {
        let mut m = e.machine().clone();
        let d = m.block_disk.as_mut().unwrap().disk_mut().unwrap();
        (0..6u32)
            .map(|k| u64::from(d.read_block(20 + k / 2).unwrap()[(0o200 * k as usize) % 256]))
            .collect()
    };
    let written: Vec<u64> = (0..6).map(|k| 0o100 + k).collect();
    let e = same_on(&p, &with_block_disk, &|e| e.port.model = Some(late(3)));
    assert_eq!(on_disk(&e), written, "every word written before START is on the disk");
    let mut m = machine(&p, REV16);
    with_block_disk(&mut m);
    let e = ends_on(m, &|e| {
        e.port.model = Some(late(3));
        e.mutation = Mutation::NoLandAtStart;
    });
    assert_ne!(on_disk(&e), written, "a START behind the writes is caught");
}

/// **A halt and a checkpoint mid-burst lose no write** (A15b.5, A15b.13):
/// twelve writes back to back against a port that accepts one each 200 ns,
/// so that the queue fills; halted at every clock of the run, drained to
/// `micro`'s state, and run on, from a checkpoint too: every write lands,
/// the end the run's without the halt.
#[test]
fn a_halt_mid_burst_loses_no_write() {
    let mut p = Prog::default();
    for k in 0..12u64 {
        let (a, v) = (p.k(PHYS | (0o300 + 0o10 * k)), p.k(0o7000 + k));
        p.op(ALU | SETA | a_src(a) | fd(0o20));
        p.op(ALU | SETA | a_src(v) | fd(0o32));
    }
    p.fill(1);
    p.read(PHYS | 0o300, 0o26);
    p.stop();
    let slow = |e: &mut Pipeline| {
        let t = muir::pipeline::PortTiming { read_ns: 247, write_ns: 130, occupancy_ns: 200 };
        e.configure(e.period(), t, 65_536);
    };
    let fresh = || {
        let mut u = Micro::new(machine(&p, REV16));
        u.boot();
        u
    };
    let (t0, e0) = halted_run_with(&p, None, false, &mut fresh(), &slow);
    assert!(e0.port.meters.queue_full_clocks > 0, "the queue filled");
    for k in 0..12 {
        assert_eq!(e0.machine().main[0o300 + 0o10 * k], 0o7000 + k as u64);
    }
    for checkpoint in [false, true] {
        let mut u = fresh();
        for c in 1..e0.clock() {
            let (t, e) = halted_run_with(&p, Some(c), checkpoint, &mut u, &slow);
            assert_eq!(t, t0, "halted at {c}, checkpoint {checkpoint}: the microcycles");
            if state(e.machine()) != state(e0.machine()) {
                diff_state(e.machine(), e0.machine(), c);
            }
        }
    }
}

/// **A checkpoint keeps the period, the port's timing and the cache**
/// (A15b.13; MP2b ruling Q16): a resume at another of any is refused, the
/// refusal naming the flag that would match it; at the same, it loads. The
/// checkpoint is written at the default, 10 ns, 20 units, so the period's
/// refusal names 10, and the timing's and the cache's are asked at 20, a
/// period that matches.
#[test]
fn a_checkpoint_refuses_another_period_timing_or_cache() {
    let p = random_program(1, 0);
    let mut e = Pipeline::new(machine(&p, REV16));
    e.boot();
    e.skip_sweep();
    for _ in 0..50 {
        e.tick().unwrap();
    }
    e.machine_mut().clock_control.run = false;
    while !e.is_halted() {
        e.tick().unwrap();
    }
    let mut w = muir::checkpoint::Writer::new();
    e.save(&mut w);
    let bytes = w.finish();
    let load = |set: &dyn Fn(&mut Pipeline)| {
        let mut f = Pipeline::new(machine(&p, REV16));
        set(&mut f);
        f.load(&mut muir::checkpoint::Reader::for_word_bits(&bytes, 40)).map_err(|e| e.to_string())
    };
    let kria = muir::pipeline::PortTiming::KRIA;
    assert!(load(&|_| {}).is_ok());
    let why = load(&|f| f.configure(18, kria, 65_536)).unwrap_err();
    assert!(why.contains("--microcycle-ns 10"), "{why}");
    let why = load(&|f| f.configure(20, muir::pipeline::PortTiming::ARTY, 65_536)).unwrap_err();
    assert!(why.contains("--memory-timing 344,215,10"), "{why}");
    let why = load(&|f| f.configure(20, kria, 16_384)).unwrap_err();
    assert!(why.contains("--cache 65536"), "{why}");
}

// --- The rows on revision 15's rtl (A15b.3) -----------------------------------
//
// The single-edge contract's rows (`docs/quux.md`, "The single-edge
// contract"), each on revision 15's `rtl` against `micro`, its own rule
// read off the end, and its fault planted. The rows the tests above hold
// already: the delay slot (`the_speculation_matrix_ends_as_on_micro`), A
// and M memory (`straight_line_alu_words_end_as_on_micro`), the PDL buffer
// (`the_pdl_buffer_across_its_wrap`), the OA registers
// (`the_oa_high_hold_is_2_1_0_clocks`), RD's return decision
// (`each_case_of_the_guard_on_planted_pairs`), conditions 4-6 after a start
// (`the_late_squash_after_a_faulting_start`,
// `an_interrupt_at_every_clock_around_a_check`), a faulting start, `MD` from
// memory and a device register
// (`a_hit_lands_two_clocks_after_its_grant_and_a_miss_its_fill_later`,
// `a_register_takes_a_clock_more_than_nothing`), a start after a map write
// (`a_start_after_a_map_write_translates_through_the_new_entry`), DIV and
// MUL (`div_stays_18_clocks_in_ex_and_mul_5`).

/// **The registers, a PDL read through a pointer just written** (A15b.3's
/// second row): the word after a write of the PDL pointer from the ALU reads
/// the buffer at the new pointer, held a clock for it; a read that does not
/// wait reads at the old.
#[test]
fn row_a_pdl_read_through_a_pointer_just_written_waits_for_it() {
    use muir::isa::asm::src;
    let mut p = Prog::default();
    let (four, seven, five, v) = (p.k(4), p.k(7), p.k(5), p.k(0o6543));
    p.op(ALU | SETA | a_src(four) | fd(0o14));
    p.fill(2);
    p.op(ALU | SETA | a_src(v) | fd(0o11));
    p.fill(2);
    p.op(ALU | SETA | a_src(seven) | fd(0o14));
    p.fill(2);
    p.op(ALU | SETA | a_src(five) | fd(0o14));
    p.op(ALU | SETM | src(0o25) | m_dest(0o26));
    p.stop();
    let e = same(&p);
    assert_eq!(e.machine().mmem[0o26], 0o6543, "the word at the new pointer");
    assert!(e.meters.pdl_wait > 0, "held a clock");
    differs(&p, Mutation::NoPdlWait);
}

/// **The PDL buffer written by the index** (A15b.3's PDL row): a write by
/// PDL-INDEX lands at the index its word left, not at the one the next word
/// writes.
#[test]
fn row_a_write_by_the_index_takes_its_own_word_s_index() {
    let mut p = Prog::default();
    let (ten, twenty, v) = (p.k(0o10), p.k(0o20), p.k(0o4444));
    p.op(ALU | SETA | a_src(ten) | fd(0o13));
    p.fill(2);
    p.op(ALU | SETA | a_src(v) | fd(0o12));
    p.op(ALU | SETA | a_src(twenty) | fd(0o13));
    p.fill(3);
    p.stop();
    let e = same(&p);
    assert_eq!(e.machine().pdl[0o10], 0o4444, "at its own index");
    assert_eq!(e.machine().pdl[0o20], 0, "not at the next word's");
}

/// **The SPC stack, a push** (A15b.3's SPC row): an M read of the stack in
/// the word after a push reads the old word at the new pointer, the word
/// after that the new; and a POPJ right after a push returns to the word
/// pushed, RD's top serving it. A push written at once is caught.
#[test]
fn row_an_spc_push_is_read_old_by_the_next_word_and_new_after() {
    use muir::isa::asm::src;
    let mut p = Prog::default();
    let v = p.k(0o1234);
    p.op(ALU | SETA | a_src(v) | fd(0o15));
    p.op(ALU | SETM | src(0o1) | m_dest(0o26));
    p.op(ALU | SETM | src(0o1) | m_dest(0o27));
    p.fill(1);
    let to = p.at() + 6;
    let t = p.k(to);
    p.op(ALU | SETA | a_src(t) | fd(0o15));
    p.op(filler().raw() | POPJ);
    p.fill(1);
    p.stop();
    while p.at() < to {
        p.fill(1);
    }
    let mark = ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(0o25) | m_dest(0o25);
    p.op(mark);
    p.stop();
    let e = same(&p);
    let m = e.machine();
    assert_eq!(m.mmem[0o26] & 0o1777777, 0, "the old word at the new pointer");
    assert_eq!(m.mmem[0o27] & 0o1777777, 0o1234, "the new word a word later");
    assert_eq!(m.mmem[0o25], 1, "the POPJ returned to the word pushed");
    differs(&p, Mutation::SpcWriteAtOnce);
}

/// **The TLB, a `WRITE-MAP` operation** (A15b.3's TLB row): `MAP(MD)` in
/// the word after the operation reads the old entry, the word after that
/// the new. A `MAP(MD)` that reads the new at once is caught.
#[test]
fn row_map_md_right_after_a_map_write_reads_the_old_entry() {
    use muir::isa::asm::src;
    let mut p = Prog::default();
    direct_write(&mut p, 0o4000, 3);
    p.op(ALU | SETM | src(0o11) | m_dest(0o26));
    p.op(ALU | SETM | src(0o11) | m_dest(0o27));
    p.stop();
    let e = same(&p);
    let m = e.machine();
    assert_ne!(m.mmem[0o26], m.mmem[0o27], "old, then new");
    assert_eq!(m.mmem[0o27] & 0o777777, 3, "the new entry's frame");
    differs(&p, Mutation::MapSeenNew);
}

/// **`WRITE-I-MEM` goes on at the word after it in execution order**
/// (WRITE-I-MEM ruling; A15b.3, A15b.4), in MIT's form: a write of the
/// word right after it runs the new word; in a jump's delay slot, a write
/// of another word goes on at the jump's target, and a write of the target
/// runs the target's new word. As on `micro`, whose WRITE-I-MEM check is
/// off for the slot (it refuses one). A refetch from the write's own
/// address + 1 is caught by the delay slot.
#[test]
fn write_i_mem_goes_on_at_the_word_after_it_in_execution_order() {
    let inc = |m: u64| ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(m) | m_dest(m);
    let word = inc(0o22);
    let wim = |p: &mut Prog, at: u64| {
        let hi = p.k(word >> 32);
        p.mmem.push((0o30, word & 0xffff_ffff));
        p.op(JUMP | P | R | ALWAYS | N | target(at) | a_src(hi) | m_src(0o30));
    };
    // The word right after it.
    let mut p = Prog::default();
    let at = p.at() + 1;
    wim(&mut p, at);
    p.op(inc(0o21));
    p.fill(1);
    p.stop();
    let e = same(&p);
    assert_eq!((e.machine().mmem[0o21], e.machine().mmem[0o22]), (0, 1), "the new word ran");
    // In a delay slot: another word, then the target.
    for (other, want) in [(true, [0, 0, 1]), (false, [0, 1, 0])] {
        let mut p = Prog::default();
        let jump = p.at();
        let t = jump + 0o10;
        let written = if other { jump + 0o20 } else { t };
        p.op(JUMP | ALWAYS | target(t));
        wim(&mut p, written);
        p.op(inc(0o21));
        p.stop();
        while p.at() < t {
            p.fill(1);
        }
        p.op(inc(0o23));
        p.stop();
        while p.at() < jump + 0o20 {
            p.fill(1);
        }
        p.fill(2);
        p.stop();
        // `micro` with its checks off: the WRITE-I-MEM check refuses a
        // write in a slot, which only a word written at run time can make.
        let marks = |m: &Machine| [m.mmem[0o21], m.mmem[0o22], m.mmem[0o23]];
        let u = micro_unchecked(&p);
        let e = pipeline(&p);
        assert_eq!(marks(u.machine()), want, "micro, other word {other}");
        assert_eq!(marks(e.machine()), want, "the pipeline, other word {other}");
        diff_state(e.machine(), u.machine(), 0);
        let x = pipeline_with(&p, |x| x.mutation = Mutation::ImemRefetchAfterItsAddress);
        if other {
            assert_ne!(state(x.machine()), state(u.machine()), "the refetch at its address + 1");
        }
    }
}

/// **The dispatch memory, a write** (A15b.3's dispatch row): the dispatch
/// in the word after a dispatch-memory write reads the new entry, its
/// predicted target, the old entry's, checked against it in EX.
#[test]
fn row_a_dispatch_after_a_dispatch_write_takes_the_new_entry() {
    use muir::isa::asm::{DMEM_WRITE, predicted};
    const T: u64 = 0o40;
    let mut p = Prog::default();
    let (old_to, new_to) = (0o300, 0o320);
    p.dmem.push((T as usize, old_to as u32));
    let e = p.k(new_to);
    p.op(DISPATCH | DMEM_WRITE | a_src(e) | T << 12);
    p.op(disp(T) | predicted(old_to, false, false));
    p.fill(1);
    p.stop();
    let mark = |m: u64| ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(m) | m_dest(m);
    for (at, slot) in [(old_to, 0o26), (new_to, 0o27)] {
        while p.at() < at {
            p.fill(1);
        }
        p.op(mark(slot));
        p.stop();
    }
    let e = same(&p);
    let m = e.machine();
    assert_eq!((m.mmem[0o26], m.mmem[0o27]), (0, 1), "the new entry's target");
    assert_eq!(e.meters.mispredicted[1], 1, "the old entry's prediction checked");
}

/// **The control store, `WRITE-I-MEM`** (A15b.3's control-store row; A15b.4),
/// in MIT's form, with N: a write of the word two ahead, already fetched:
/// that word runs as written, the words behind the write fetched again. Not
/// fetched again, the old word runs: caught.
#[test]
fn row_a_word_written_by_write_i_mem_runs_as_written() {
    let mut p = Prog::default();
    let (old, new) = (p.k(0o1111), p.k(0o2222));
    let at = p.at() + 2;
    let word = ALU | SETA | a_src(new) | m_dest(0o26);
    let hi = p.k(word >> 32);
    p.mmem.push((0o30, word & 0xffff_ffff));
    p.op(JUMP | P | R | ALWAYS | N | target(at) | a_src(hi) | m_src(0o30));
    p.fill(1);
    assert_eq!(p.at(), at);
    p.op(ALU | SETA | a_src(old) | m_dest(0o26));
    p.fill(1);
    p.stop();
    let e = same(&p);
    assert_eq!(e.machine().mmem[0o26], 0o2222, "the word written ran");
    differs(&p, Mutation::NoImemRefetch);
}

// --- MD after a start (the MD-after-read and MD-after-write rulings) -------

/// `p` ends the same on both engines, `micro` with its OA select check
/// off and the checks under it, the MD-after-start check among them
/// ([`micro_unchecked`]): words no assembler made run, as the debug IR's
/// and the random programs' do.
fn same_unchecked(p: &Prog) -> Pipeline {
    let u = micro_unchecked(p);
    let e = pipeline(p);
    diff_state(e.machine(), u.machine(), 0);
    assert_eq!(state(e.machine()), state(u.machine()), "the pipeline against micro");
    e
}

/// `p` ends otherwise than on unchecked `micro` with `mutation` planted.
fn differs_unchecked(p: &Prog, mutation: Mutation) -> bool {
    let u = micro_unchecked(p);
    let e = pipeline_with(p, |e| e.mutation = mutation);
    state(e.machine()) != state(u.machine())
}

/// Unchecked `micro` on `p`, halted by the clock control after `k`
/// microcycles for three master clocks, run on to the stop and 16
/// microcycles more.
fn micro_halted_at(p: &Prog, k: u64) -> Micro {
    let mut u = Micro::new(machine(p, REV16));
    u.oa_select_check = false;
    u.boot();
    while u.machine().cycles < k && u.machine().opc != STOP as u16 {
        u.step().unwrap();
    }
    u.machine_mut().clock_control.run = false;
    for _ in 0..3 {
        u.step().unwrap();
    }
    u.machine_mut().clock_control.run = true;
    for _ in 0..200_000 {
        if u.machine().opc == STOP as u16 {
            for _ in 0..16 {
                u.step().unwrap();
            }
            return u;
        }
        u.step().unwrap();
    }
    panic!("the program never reached its stop after a halt at {k}");
}

/// **A halt anywhere resumes to the same end** (A15b.13), on `p`: the
/// pipeline halted at every clock, drained to unchecked `micro`'s state at
/// the same microcycle, and `micro` halted after every microcycle, each run
/// on to the end of the run without a halt.
fn halts_end_alike(p: &Prog, what: &str) {
    let u0 = micro_unchecked(p);
    let fresh = || {
        let mut u = Micro::new(machine(p, REV16));
        u.oa_select_check = false;
        u.boot();
        u
    };
    let (t0, e0) = halted_run(p, None, false, &mut fresh());
    diff_state(e0.machine(), u0.machine(), 0);
    let mut u = fresh();
    for c in 1..e0.clock() {
        let (t, e) = halted_run(p, Some(c), false, &mut u);
        assert_eq!(t, t0, "{what}: the pipeline halted at clock {c}: the microcycles");
        if state(e.machine()) != state(e0.machine()) {
            eprintln!("{what}: the pipeline halted at clock {c}");
            diff_state(e.machine(), e0.machine(), c);
        }
    }
    for k in 1..u0.machine().cycles {
        let u = micro_halted_at(p, k);
        if state(u.machine()) != state(u0.machine()) {
            eprintln!("{what}: micro halted after microcycle {k}");
            diff_state(u.machine(), u0.machine(), k);
        }
    }
}

/// **A write carries `MD` as its start's own microcycle leaves it** on
/// revision 15 (the MD-after-write ruling, amendment 10; `docs/quux.md`):
/// VMA-START-WRITE and MD-START-WRITE, then a word loading `MD` with 5555:
/// the word written is the start's `MD`, and `MD` after is 5555; a miss and
/// a hit; a write the start of a register-page register's read held behind
/// it (word 201) as well. The single-edge rule, which takes the next word's
/// `MD`, is caught, and the MD-after-start check halts `micro` on the word.
#[test]
fn row_a_write_carries_its_start_s_md() {
    let mut caught = 0;
    for hit in [false, true] {
        for md_start in [false, true] {
            let mut p = Prog::default();
            let (a, v1, v3, v5) = (p.k(PHYS | 0o50), p.k(0o1111), p.k(0o3333), p.k(0o5555));
            if hit {
                p.read(PHYS | 0o50, 0o20);
            }
            p.op(ALU | SETA | a_src(v1) | MD);
            let at = p.at();
            if md_start {
                p.op(ALU | SETA | a_src(a) | muir::isa::asm::VMA);
                p.op(ALU | SETA | a_src(v3) | fd(0o32));
            } else {
                p.op(ALU | SETA | a_src(a) | START_WRITE);
            }
            p.op(ALU | SETA | a_src(v5) | MD);
            p.fill(2);
            p.op(ALU | SETM | SRC_MD | m_dest(0o26));
            p.stop();
            let what = format!("hit {hit}, MD-START-WRITE {md_start}");
            let e = same_unchecked(&p);
            let m = e.machine();
            let start_md = if md_start { 0o3333 } else { 0o1111 };
            assert_eq!(m.main[0o50], start_md, "{what}: the start's MD written");
            assert_eq!(m.mmem[0o26], 0o5555, "{what}: MD after, the next word's");
            caught += usize::from(differs_unchecked(&p, Mutation::WriteMdOfNextWord));
            halts_end_alike(&p, &what);
            // The check: the word right after the start writes MD.
            let mut u = Micro::new(machine(&p, REV16));
            u.boot();
            let mut halt = None;
            for _ in 0..2000 {
                if let Err(h) = u.step() {
                    halt = Some(h);
                    break;
                }
            }
            let next = at + 1 + u64::from(md_start);
            assert_eq!(halt, Some(Halt::MdAfterStart { pc: next as u16 }), "{what}: the check");
        }
    }
    assert_eq!(caught, 4, "the next word's MD written is caught in every case");
}

/// **A register write carries its start's `MD`** (the MD-after-write
/// ruling; MP2b rulings 2, Q1): the memory system's word 222, the pointer
/// types' low half (A14.9), written by VMA-START-WRITE, then a word loading
/// `MD`: the register takes the start's word, read back, on both engines.
/// The single-edge rule is caught.
#[test]
fn a_register_write_carries_its_start_s_md() {
    let mut p = Prog::default();
    let (r, v1, v5) = (p.k(REGISTER_PAGE | 0o222), p.k(0o1111), p.k(0o5555));
    p.op(ALU | SETA | a_src(v1) | MD);
    p.op(ALU | SETA | a_src(r) | START_WRITE);
    p.op(ALU | SETA | a_src(v5) | MD);
    p.fill(2);
    p.op(ALU | SETM | SRC_MD | m_dest(0o26));
    p.read(REGISTER_PAGE | 0o222, 0o27);
    p.stop();
    let e = same_unchecked(&p);
    assert_eq!(e.machine().mmem[0o26], 0o5555, "MD after, the next word's");
    assert_eq!(e.machine().mmem[0o27], 0o1111, "the register took the start's MD");
    assert!(differs_unchecked(&p, Mutation::WriteMdOfNextWord), "the next word's MD caught");
}

/// **A write redirected into the PDL buffer carries its start's `MD`**
/// (the MD-after-write ruling; A14.7): a write inside the buffer (status 5,
/// its copies at A 430 and 431, the head at 100 and PP at 107), then a PDL
/// read of that word by the index in the next word, or one or two words
/// later, or an `MD` load in the next word: the buffer takes the start's
/// word, and the read reads it, on both engines.
#[test]
fn a_write_redirected_into_the_pdl_buffer_carries_its_start_s_md() {
    const PAGE: Word = 0o2001 << 10;
    const BASE: Word = PAGE | 0o20;
    for case in 0..4 {
        let load_md = case == 3;
        let mut p = Prog::default();
        set_directory(&mut p);
        p.main.push(((8 << 10) + (PAGE >> 20) as usize, rw_entry(9)));
        // Status 5, access 01: every reference faults, so it redirects.
        let status_5 = 0b11 << 28 | 1 << 26 | 1 << 24 | 3 << 22 | 0o20;
        p.main.push(((9 << 10) + ((PAGE >> 10) & 0o1777) as usize, status_5));
        let (b, h, pp, ix) = (p.k(BASE), p.k(0o100), p.k(0o107), p.k(0o101));
        p.op(ALU | SETA | a_src(b) | a_dest(muir::tlb::A_PDL_BUFFER_VIRTUAL_ADDRESS as u64));
        p.op(ALU | SETA | a_src(h) | a_dest(muir::tlb::A_PDL_BUFFER_HEAD as u64));
        p.op(ALU | SETA | a_src(pp) | fd(0o14));
        p.op(ALU | SETA | a_src(ix) | fd(0o13));
        p.fill(2);
        let (a, v1, v5) = (p.k(BASE + 1), p.k(0o1111), p.k(0o5555));
        p.op(ALU | SETA | a_src(v1) | MD);
        p.op(ALU | SETA | a_src(a) | START_WRITE);
        if load_md {
            p.op(ALU | SETA | a_src(v5) | MD);
        } else {
            p.fill(case);
            p.op(ALU | SETM | src(0o5) | m_dest(0o27));
        }
        p.fill(3);
        p.op(ALU | SETM | src(0o5) | m_dest(0o26));
        p.stop();
        let what = if load_md { "MD loaded".to_string() } else { format!("read at gap {case}") };
        let e = same_unchecked(&p);
        let m = e.machine();
        assert_eq!(m.tlb.redirects[0], 1, "{what}: one redirect, inside");
        assert_eq!(m.pdl[0o101], 0o1111, "{what}: the buffer took the start's MD");
        assert_eq!(m.mmem[0o26], 0o1111, "{what}: read back by the index");
        if !load_md {
            assert_eq!(m.mmem[0o27], 0o1111, "{what}: the read after the write");
        }
        assert_eq!(m.main[0o20 << 10 | 0o21], 0, "{what}: and not memory");
    }
}

/// **A write started right after a read start writes the read's word**
/// (the MD-after-read ruling, rule (A); the MD-after-write ruling): `MD`
/// 2, a read of 123321 at word 40, then S, right after it: MD-START-WRITE
/// 4444 (its M scratch word 36, or M 31 as MP4's program), which writes at
/// `VMA` as the read left it, word 40, or VMA-START-WRITE of word 50; after
/// S a filler, a word loading `MD` with 5555, or a read start held behind
/// the write; a miss and a hit. The word written is 123321 in every case,
/// and `MD` after is the read's word, 5555, or the second read's. The
/// fallback taken before the read's word lands is caught, and the cases
/// with a filler resume to the same end halted anywhere.
#[test]
fn row_a_write_right_after_a_read_start_writes_the_read_s_word() {
    let mut caught = 0;
    let mut cases = 0;
    for hit in [false, true] {
        for s in 0..3 {
            for after in 0..3 {
                let mut p = Prog::default();
                p.main.push((0o40, 0o123321));
                p.main.push((0o60, 0o707070));
                let (two, a, w) = (p.k(2), p.k(PHYS | 0o40), p.k(PHYS | 0o50));
                let (v4, v5, b) = (p.k(0o4444), p.k(0o5555), p.k(PHYS | 0o60));
                if hit {
                    p.read(PHYS | 0o40, 0o20);
                }
                p.op(ALU | SETA | a_src(two) | MD);
                p.op(ALU | SETA | a_src(a) | START_READ);
                match s {
                    0 => p.op(ALU | SETA | a_src(v4) | fd(0o32)),
                    1 => p.op(ALU | SETA | a_src(v4) | 0o32 << 19 | 0o31 << 14),
                    _ => p.op(ALU | SETA | a_src(w) | START_WRITE),
                };
                match after {
                    0 => p.fill(1),
                    1 => p.op(ALU | SETA | a_src(v5) | MD),
                    _ => p.op(ALU | SETA | a_src(b) | START_READ),
                };
                p.fill(2);
                p.op(ALU | SETM | SRC_MD | m_dest(0o23));
                p.fill(1);
                p.stop();
                let what = format!("hit {hit}, S {s}, after {after}");
                let e = same_unchecked(&p);
                let m = e.machine();
                let written = if s < 2 { 0o40 } else { 0o50 };
                assert_eq!(m.main[written], 0o123321, "{what}: the read's word written");
                let md_after = [0o123321, 0o5555, 0o707070][after];
                assert_eq!(m.mmem[0o23], md_after, "{what}: MD after");
                if s == 1 {
                    assert_eq!(m.mmem[0o31], 0o4444, "{what}: S's own M write stands");
                }
                cases += 1;
                if differs_unchecked(&p, Mutation::WriteWordBeforeTheRead) {
                    caught += 1;
                }
                if after == 0 && !hit && s != 1 {
                    halts_end_alike(&p, &what);
                }
            }
        }
    }
    eprintln!("the word before the read's caught in {caught} of {cases}");
    assert_eq!(caught, cases, "the word taken before the read's word lands is caught");
}

/// **What else the word right after a read start may do with `MD`**
/// (the MD-after-read ruling, T4): a plain `MD` write, MD-START-READ and
/// MD-WRITE-MAP right after a read start end alike on both engines, a miss
/// and a hit.
#[test]
fn md_writes_right_after_a_read_start_end_alike() {
    for hit in [false, true] {
        for code in [0o30, 0o31, 0o33] {
            let mut p = Prog::default();
            p.main.push((0o40, 0o123321));
            let (two, a, v4) = (p.k(2), p.k(PHYS | 0o40), p.k(PHYS | 0o4444));
            if hit {
                p.read(PHYS | 0o40, 0o20);
            }
            p.op(ALU | SETA | a_src(two) | MD);
            p.op(ALU | SETA | a_src(a) | START_READ);
            p.op(ALU | SETA | a_src(v4) | fd(code));
            p.fill(3);
            p.op(ALU | SETM | SRC_MD | m_dest(0o23));
            p.fill(1);
            p.stop();
            same_unchecked(&p);
        }
    }
}

/// **The MD-after-start check** (the MD-after-read and MD-after-write
/// rulings): on revision 15 `micro` halts on a word that writes `MD` ---
/// destination 30, 31 (MD-START-READ), 32 or 33, or 34 decoding as 30 ---
/// in the microcycle right after a read start, a write start or an
/// instruction fetch's start, before it does anything; one a word later, or
/// with the check off, runs.
#[test]
fn the_md_after_start_check_halts_micro() {
    let run = |p: &Prog, check: bool| {
        let mut u = Micro::new(machine(p, REV16));
        u.oa_select_check = check;
        u.boot();
        for _ in 0..2000 {
            if let Err(h) = u.step() {
                return Some(h);
            }
            if u.machine().opc == STOP as u16 {
                return None;
            }
        }
        panic!("never stopped");
    };
    // The starts: a read, a write, and LC's fetch, NEXT-INSTR after a
    // return with SPC<14>, as the main loop's.
    for start in 0..3 {
        for code in [0o30, 0o31, 0o32, 0o33, 0o34] {
            for gap in [0, 1] {
                let mut p = Prog::default();
                let (a, v) = (p.k(PHYS | 0o40), p.k(0o4444));
                let at = match start {
                    0 => p.op(ALU | SETA | a_src(a) | START_READ).at(),
                    1 => p.op(ALU | SETA | a_src(a) | START_WRITE).at(),
                    _ => {
                        // LC written, NEEDFETCH up: a dispatch with IR<24>
                        // fetches.
                        let lc = p.k(0o400);
                        p.op(ALU | SETA | a_src(lc) | fd(0o1));
                        p.fill(2);
                        p.op(disp(0o10) | 1 << 24);
                        // Its entry: the word after its slot, N clear.
                        p.dmem.push((0o10, p.at() as u32 + 1));
                        p.at()
                    }
                };
                p.fill(gap);
                p.op(ALU | SETA | a_src(v) | fd(code));
                p.fill(2);
                p.stop();
                let what = format!("start {start}, code {code:o}, gap {gap}");
                let want = (gap == 0).then_some(Halt::MdAfterStart { pc: at as u16 });
                assert_eq!(run(&p, true), want, "{what}");
                assert_eq!(run(&p, false), None, "{what}: the check off");
            }
        }
    }
}

/// **`MD` from memory** (A15b.3's `MD` row; the single-edge contract: a
/// cycle goes out at the edge ending the microcycle after its start): the
/// word right after a read start reads `MD` as the start found it and
/// waits for nothing, a write of `MD` there giving way to the read's word;
/// every later word that uses `MD` waits for the word read. Read, written
/// and through `MAP(MD)`, at gaps 0 to 2, a miss and a hit; and
/// a halt at every clock of the gap-0 run. A word right after the start
/// that waits for the read's word is caught. `micro` runs with its checks
/// off: the MD-after-start check halts on the `MD` write.
#[test]
fn row_the_word_after_a_read_start_reads_the_old_md() {
    use muir::isa::asm::src;
    let user = |p: &mut Prog, k: usize| {
        let nine = p.k(0o11);
        match k {
            0 => p.op(ALU | SETM | SRC_MD | m_dest(0o26)),
            1 => p.op(ALU | SETA | a_src(nine) | MD),
            _ => p.op(ALU | SETM | src(0o11) | m_dest(0o26)),
        };
    };
    let mut caught = 0;
    let mut cases = 0;
    for hit in [false, true] {
        for gap in 0..3 {
            for k in 0..3 {
                let mut p = Prog::default();
                p.main.push((0o40, 5));
                let (two, a) = (p.k(2), p.k(PHYS | 0o40));
                if hit {
                    p.op(ALU | SETA | a_src(a) | START_READ);
                    p.fill(3);
                }
                p.op(ALU | SETA | a_src(two) | MD);
                p.op(ALU | SETA | a_src(a) | START_READ);
                p.fill(gap);
                user(&mut p, k);
                p.op(ALU | SETM | SRC_MD | m_dest(0o27));
                p.fill(2);
                p.stop();
                let e = same_unchecked(&p);
                if gap == 0 && k == 0 {
                    assert_eq!(e.machine().mmem[0o26], 2, "hit {hit}: the MD before");
                }
                cases += 1;
                let u = micro_unchecked(&p);
                let x = pipeline_with(&p, |x| x.mutation = Mutation::SuccessorWaitsForMd);
                if state(x.machine()) != state(u.machine()) {
                    caught += 1;
                }
                if gap == 0 && k == 0 && !hit {
                    let fresh = || {
                        let mut u = Micro::new(machine(&p, REV16));
                        u.oa_select_check = false;
                        u.boot();
                        u
                    };
                    let (t0, e0) = halted_run(&p, None, false, &mut fresh());
                    for checkpoint in [false, true] {
                        let mut u = fresh();
                        for c in 1..e0.clock() {
                            let (t, e) = halted_run(&p, Some(c), checkpoint, &mut u);
                            assert_eq!(t, t0, "halted at {c}, checkpoint {checkpoint}");
                            if state(e.machine()) != state(e0.machine()) {
                                diff_state(e.machine(), e0.machine(), c);
                            }
                        }
                    }
                }
            }
        }
    }
    eprintln!(
        "the word after the start waiting for the read: {caught} of {cases} cases end otherwise"
    );
    assert!(caught > 0, "caught");
}

/// **Port B right after a read start looks up the `MD` its word reads, and
/// a walk keeps its address** (A15b.3's `MD` row; A14.5, A14.6). `MD` holds
/// a list pointer to X; a read start of a word holding a list pointer to Y;
/// in the word right after it, `MAP(MD)` or a map-bit dispatch on `MD`,
/// with X's page not in the TLB. X, Y and Z have their directory entries
/// in different regions, and X's page table holds decoy entries at Y's and
/// Z's page indexes. The read is a hit or a miss, and Y's page is in the
/// TLB already or not. Then Y's word, or not, and Z's and X's words are
/// read. On `micro` the word looks X up; `rtl` must too, the walk
/// finishing for X while the read's word lands mid-walk: the same words
/// read, and the TLB holds what `micro`'s does for X, Y and Z. A lookup of
/// the `MD` that lands loads Y, or Y's decoy; a walk that takes Y's page
/// index under X's directory entry loads the decoy, and one left half
/// done gives Z its decoy.
#[test]
fn row_port_b_right_after_a_read_start_walks_the_md_it_reads() {
    const LIST: Word = 0o016 << 32;
    let x: Word = 0o4000;
    let y: Word = 1 << 20 | 5 << 10;
    let z: Word = 2 << 20 | 7 << 10;
    let page = |va: Word| ((va >> 10) & 0o1777) as usize;
    let mut failed = Vec::new();
    let mut cases = 0;
    for (hit, dispatch, y_held, y_read) in
        (0..16).map(|k| (k & 8 != 0, k & 4 != 0, k & 2 != 0, k & 1 != 0))
    {
        let what = format!("hit {hit}, dispatch {dispatch}, Y held {y_held}, Y read {y_read}");
        cases += 1;
        let mut p = Prog::default();
        // The directory at frame 8: X under the page table at frame 9, Y at
        // 10, Z at 11.
        for (va, table) in [(x, 9), (y, 10), (z, 11)] {
            p.main.push(((8 << 10) + (va >> 20) as usize, rw_entry(table)));
        }
        p.main.push(((9 << 10) + page(x), rw_entry(3)));
        p.main.push(((10 << 10) + page(y), rw_entry(4)));
        p.main.push(((11 << 10) + page(z), rw_entry(5)));
        // The decoys: Y's and Z's page indexes under X's table.
        p.main.push(((9 << 10) + page(y), rw_entry(6)));
        p.main.push(((9 << 10) + page(z), rw_entry(7)));
        for frame in 3..8 {
            p.main.push((frame << 10, 0o1000 + frame as Word));
        }
        p.main.push((0o40, LIST | y));
        set_directory(&mut p);
        // DTP-LIST a pointer type (word 222).
        p.write(1 << 0o16, REGISTER_PAGE | 0o222);
        if hit {
            p.read(PHYS | 0o40, 0o31);
        }
        if y_held {
            p.read(y, 0o31);
        }
        let (xa, at) = (p.k(LIST | x), p.k(PHYS | 0o40));
        p.op(ALU | SETA | a_src(xa) | MD);
        p.op(ALU | SETA | a_src(at) | START_READ);
        if dispatch {
            // Map bit 2, the entry's <23>, at IR<0>: both entries go on.
            const T: u64 = 0o40;
            let next = p.at() + 2;
            p.dmem.push((T as usize, next as u32));
            p.dmem.push((T as usize | 1, next as u32));
            p.op(disp(T) | 2 << 8 | SRC_MD | muir::isa::asm::predicted(next, false, false));
            p.fill(1);
        } else {
            p.op(ALU | SETM | src(0o11) | m_dest(0o25));
        }
        p.op(ALU | SETM | SRC_MD | m_dest(0o32));
        if y_read {
            p.read(y, 0o26);
        }
        p.read(z, 0o27);
        p.read(x, 0o30);
        p.stop();
        let u = micro(&p);
        let e = pipeline(&p);
        let (m, um) = (e.machine(), u.machine());
        assert_eq!(um.mmem[0o32], LIST | y, "{what}: micro read Y's pointer");
        assert_eq!(
            [um.mmem[0o26], um.mmem[0o27], um.mmem[0o30]],
            [if y_read { 0o1004 } else { 0 }, 0o1005, 0o1003],
            "{what}: micro reads Y, Z and X through their own tables"
        );
        assert_eq!(um.tlb.lookup(y as u32).is_some(), y_held || y_read, "{what}: micro's Y");
        let tlb = |m: &Machine| [x, y, z].map(|va| m.tlb.lookup(va as u32));
        if tlb(m) != tlb(um) || state(m) != state(um) {
            failed.push(format!(
                "{what}: rtl reads {:o} {:o} {:o}, its TLB {:?}, micro's {:?}",
                m.mmem[0o26],
                m.mmem[0o27],
                m.mmem[0o30],
                tlb(m),
                tlb(um)
            ));
        }
    }
    assert!(failed.is_empty(), "{} of {cases} cases differ:\n{}", failed.len(), failed.join("\n"));
}

// --- The file device behind the posted writes (A15b.5, A15b.6) -----------------

/// The file device's rings and buffer A, physical word addresses, each on a
/// line: one command entry, one response entry.
const CMD_RING: u64 = 0o1000;
const RESP_RING: u64 = 0o1100;
const BUF_A: u64 = 0o1200;
const LOG_TAG: u64 = 0o4321;

/// A program that sets the file device's rings up through its registers,
/// writes a LOG command of "hello" into the ring and buffer A, word 0 last,
/// reads the response's word 0 into M 25 so that its line is cached, writes
/// CMD_PROD, waits for the response index to move, and reads the response's
/// word 0 into M 26.
fn file_device_log_program() -> Prog {
    use muir::file_device::{CMD_BASE, CMD_PROD, CONTROL, RESP_PROD, op};
    let mut p = Prog::default();
    let reg = |w: u32| REGISTER_PAGE | Word::from(w);
    p.write(CMD_RING, reg(CMD_BASE));
    p.write(0, reg(CMD_BASE + 1));
    p.write(RESP_RING, reg(CMD_BASE + 4));
    p.write(0, reg(CMD_BASE + 5));
    p.write(1, reg(CONTROL));
    for k in 1..8 {
        let v = match k {
            2 => BUF_A,
            3 => 5,
            _ => 0,
        };
        p.write(v, PHYS | (CMD_RING + k));
    }
    p.write(u64::from(u32::from_le_bytes(*b"hell")), PHYS | BUF_A);
    p.write(u64::from(b'o'), PHYS | (BUF_A + 1));
    p.read(PHYS | RESP_RING, 0o25);
    p.write(LOG_TAG | u64::from(op::LOG) << 16, PHYS | CMD_RING);
    p.write(1, reg(CMD_PROD));
    // Round until the response index is 1.
    let (prod, one) = (p.k(reg(RESP_PROD)), p.k(1));
    let round = p.at();
    p.op(ALU | SETA | a_src(prod) | START_READ);
    p.fill(1);
    p.op(jcond(3) | 1 << 6 | SRC_MD | a_src(one) | target(round));
    p.fill(1);
    p.read(PHYS | RESP_RING, 0o26);
    p.stop();
    p
}

/// The file device's log kept, for the line the command logged.
fn with_device_log(m: &mut Machine) {
    m.file_device.log = Some(Vec::new());
}

/// **A command whose ring entry is still in the posted writes when CMD_PROD
/// is written is read whole** (A15b.5), and **its response is read after
/// it by the sweep** (A15b.6): main memory answers a write in 25 us, so
/// the entry is still going in when the producer write is made; the device
/// takes it once every earlier write is answered, logs "hello", and the
/// program reads the response it wrote over a line it had cached. CMD_PROD
/// taken at once is caught; a sweep that misses the response's set is
/// caught.
#[test]
fn the_file_device_reads_its_entry_whole_and_the_sweep_shows_its_response() {
    let p = file_device_log_program();
    let slow = |e: &mut Pipeline| {
        let t = muir::pipeline::PortTiming { read_ns: 247, write_ns: 25_000, occupancy_ns: 10 };
        e.configure(e.period(), t, 65_536);
    };
    let response = LOG_TAG | u64::from(muir::file_device::op::LOG) << 24;
    let e = same_on(&p, &with_device_log, &slow);
    let m = e.machine();
    assert_eq!(m.mmem[0o25], 0, "the response's line cached before the command");
    assert_eq!(m.mmem[0o26] & 0xffff_ffff, response, "status 0, the program's word 0 read");
    assert_eq!(
        m.file_device.log.as_deref(),
        Some(&[b"hello".to_vec()][..]),
        "the entry read whole"
    );
    assert!(e.meters.cmd_prod_wait > 0, "CMD_PROD waited for the writes");
    assert!(e.port.meters.sweeps > 0, "the completion swept");
    for (mutation, what) in [
        (Mutation::NoCmdProdWait, "CMD_PROD taken at once"),
        (Mutation::SweepMissesASet, "a sweep that misses a set"),
    ] {
        let mut m = machine(&p, REV16);
        with_device_log(&mut m);
        let e = ends_on(m, &|e| {
            slow(e);
            e.mutation = mutation;
        });
        eprintln!(
            "{what}: the response word {:o}, the log {:?}",
            e.machine().mmem[0o26],
            e.machine().file_device.log
        );
        assert_ne!(e.machine().mmem[0o26] & 0xffff_ffff, response, "{what}: caught");
    }
}

/// **A block-disk transfer over words the cache holds** (A15b.6): the
/// processor has a word of the page and a word elsewhere cached; a disk read
/// into the page; after it the processor reads the disk's word, and the word
/// elsewhere still hits: no whole-cache invalidation. A transfer that leaves
/// the sets it writes is caught.
#[test]
fn a_block_disk_transfer_clears_the_sets_it_writes_and_no_more() {
    const READ: u64 = 0;
    const FOUR_BYTE: u64 = 1 << 12;
    let disk_word = |k: u64, w: u64| muir::machine::UNBOXED_TAG | k << 16 | w;
    let mut p = Prog::default();
    p.main.push((0o777, 0o2000));
    p.main.push((0o2005, 0o1111));
    p.main.push((0o5000, 0o2222));
    p.read(PHYS | 0o2005, 0o24);
    p.read(PHYS | 0o5000, 0o25);
    block_disk_command(&mut p, 0o777, 3, READ | FOUR_BYTE);
    p.read(PHYS | 0o2005, 0o26);
    p.read(PHYS | 0o5000, 0o27);
    p.stop();
    let e = same_on(&p, &with_block_disk, &|_| {});
    let m = e.machine();
    assert_eq!((m.mmem[0o24], m.mmem[0o25]), (0o1111, 0o2222), "before");
    assert_eq!(m.mmem[0o26], disk_word(3, 5), "the disk's word after the transfer");
    assert_eq!(m.mmem[0o27], 0o2222);
    let c = &e.port.cache;
    assert_eq!((c.misses, c.hits), (3, 1), "the page's word missed again, the other hit");
    let mut m = machine(&p, REV16);
    with_block_disk(&mut m);
    let e = ends_on(m, &|e| e.mutation = Mutation::NoClearSet);
    assert_eq!(
        e.machine().mmem[0o26],
        0o1111,
        "a transfer that leaves its sets: the old word, caught"
    );
}

// --- The time-neutral harness (MP2b ruling Q12) ---------------------------------

/// The digests of `m` run to opcode 7's stop under neutral time, every
/// `every` steps of LC: on `micro`, and on the pipeline with `mutation`
/// planted (`tests/support/neutral.rs`).
fn digest_lines(m: &Machine, every: u64, mutation: Mutation) -> (Vec<String>, Vec<String>) {
    use support::neutral::{Digests, Neutral};
    fn run<E: Neutral>(mut e: Digests<E>) -> Vec<String> {
        e.engine.time_neutral();
        e.boot();
        for _ in 0..20_000 {
            if e.machine().opc == OP_7 as u16 {
                break;
            }
            if e.step().is_err() {
                break;
            }
        }
        e.lines
    }
    let u = run(Digests::new(Micro::new(m.clone()), every));
    let mut x = Pipeline::new(m.clone());
    x.mutation = mutation;
    x.skip_sweep();
    let mut d = Digests::new(x, every);
    d.engine.skip_sweep();
    let e = {
        d.engine.time_neutral();
        d.boot();
        d.engine.skip_sweep();
        for _ in 0..20_000 {
            if d.machine().opc == OP_7 as u16 {
                break;
            }
            if d.step().is_err() {
                break;
            }
        }
        d.lines
    };
    (u, e)
}

/// The main-loop machines the harness's check runs, D on and off: the main
/// loop's paths, and opcode 3's handler with a squash to restore.
fn harness_machines() -> Vec<(String, Machine)> {
    use muir::isa::asm::HINT;
    let mut out = Vec::new();
    for d in [false, true] {
        out.push((
            format!("ones, D {d}"),
            d_machine(REV16, d_register(d), &ONES, CODE, false, false),
        ));
        let program = [hw(0o10, 5, 5), hw(0o10, 6, 3), hw(1, 0, 0), hw(0o10, 5, 1), hw(7, 0, 0)];
        out.push((
            format!("operands, D {d}"),
            d_machine(REV16, d_register(d), &program, CODE, false, false),
        ));
        let never = JUMP | m_src(2) | R | HINT;
        out.push((
            format!("a wrong return, D {d}"),
            ml_machine(d, &TWOS_THREES, &[never, filler().raw()], &|_| {}),
        ));
        // Opcode 3's handler: an A write read three words on (d3), and a
        // jump hinted onto a write start, not taken, the word the read
        // after it returns into M 27.
        // Past the handler's return and its slot, which ml_machine puts
        // after the body.
        let at = OP_3 + 9 + 2;
        let body = [
            ALU | SETM | m_src(0o31) | a_dest(0o1650),
            filler().raw(),
            filler().raw(),
            ALU | SETA | a_src(0o1650) | m_dest(0o26),
            JUMP | m_src(2) | HINT | target(at),
            filler().raw(),
            ALU | SETA | a_src(0o1700) | START_READ,
            filler().raw(),
            ALU | SETM | SRC_MD | m_dest(0o27),
        ];
        assert_eq!(OP_3 + body.len() as u64 + 2, at, "the hinted target is past the handler");
        let mut m = ml_machine(d, &TWOS_THREES, &body, &|m| m.amem[0o1700] = PHYS | 0o100);
        // The target, past the handler's return: a write start, and back.
        let put = |m: &mut Machine, k: u64, w: u64| m.imem[(at + k) as usize] = Insn::extended(w);
        put(&mut m, 0, ALU | SETA | a_src(0o1700) | START_WRITE);
        put(&mut m, 1, filler().raw() | POPJ);
        put(&mut m, 2, filler().raw());
        out.push((format!("d3 and a wrong-path write, D {d}"), m));
    }
    out
}

/// **The time-neutral harness** (MP2b ruling Q12): under neutral time the
/// pipeline, halted and drained at every boundary, digests as `micro`
/// does at the same `Machine::cycles`, every step of LC and every third, D
/// on and off. A speculative pop not restored and a delay slot counted
/// twice fail it.
#[test]
fn the_harness_digests_the_pipeline_as_micro_at_every_boundary() {
    let machines = harness_machines();
    let mut boundaries = 0;
    for (what, m) in &machines {
        for every in [1, 3] {
            let (u, e) = digest_lines(m, every, Mutation::None);
            assert!(!u.is_empty(), "{what}: boundaries");
            assert_eq!(e, u, "{what}, every {every}: the digests");
            boundaries += u.len();
        }
    }
    eprintln!("{boundaries} boundaries digested alike");
    for mutation in
        [Mutation::NoD3, Mutation::NoSpcRestore, Mutation::SquashedStart, Mutation::NopCountedTwice]
    {
        let caught = machines.iter().filter(|(_, m)| {
            let (u, e) = digest_lines(m, 1, mutation);
            e != u
        });
        let n = caught.count();
        eprintln!("{mutation:?}: {n} of {} machines' digests differ", machines.len());
        assert!(n > 0, "{mutation:?} planted, the harness passes");
    }
}

/// **Revision 14's IMOD on the pipeline, the measurement aid**: an
/// OA-REG-LOW write is ORed into the word after it and into no other. The
/// word after that, an unconditional jump RD resolves, whose target the
/// value would change, jumps where it says. A decode that ORs the value
/// into every word leaving CS before the flags are spent is caught here.
#[test]
fn revision_14_s_imod_reaches_the_next_word_alone() {
    let mut p = Prog::default();
    let to = 0o300u64;
    // OA-REG-LOW's value, in the jump target's place: 2 << 12, a bit the
    // target lacks.
    let v = p.k(2 << 12);
    p.op(ALU | SETA | a_src(v) | fd(0o16));
    p.fill(1);
    p.op(JUMP | ALWAYS | N | target(to));
    p.fill(1);
    let mark = |m: u64| ALU | muir::isa::asm::ADD | a_src(ONE) | m_src(m) | m_dest(m);
    for (at, slot) in [(to, 0o26), (to | 2, 0o27)] {
        while p.at() < at {
            p.fill(1);
        }
        p.op(mark(slot));
        p.stop();
    }
    let g = Geometry::QUUX_14;
    let u = match run_engine(Micro::new(machine(&p, g))) {
        Ok(u) => u,
        Err((h, _)) => panic!("micro halted: {h:?}"),
    };
    let e = match run_engine(Pipeline::new(machine(&p, g))).and_then(settled) {
        Ok(e) => e,
        Err((h, _)) => panic!("the pipeline halted: {h:?}"),
    };
    assert_eq!(
        (u.machine().mmem[0o26], u.machine().mmem[0o27]),
        (1, 0),
        "micro: the target as written"
    );
    assert_eq!(state(e.machine()), state(u.machine()), "the pipeline against micro");
}

/// The pipeline on the main-loop machine `m` to opcode 7's stop, halted at
/// clock `halt` and run on, from a checkpoint with `checkpoint`: its trace
/// and its end. At the halt the state is `micro`'s, `u`, at the same
/// `Machine::cycles`.
fn ml_halted_run(
    m: &Machine,
    halt: Option<u64>,
    checkpoint: bool,
    u: &mut Micro,
) -> (Vec<Option<u16>>, Pipeline) {
    let mut e = Pipeline::new(m.clone());
    e.trace = Some(Vec::new());
    e.boot();
    e.skip_sweep();
    let mut t = Vec::new();
    for _ in 0..20_000 {
        if e.machine().opc == OP_7 as u16 {
            break;
        }
        if Some(e.clock() + 1) == halt {
            e.machine_mut().clock_control.run = false;
            for _ in 0..10_000 {
                if e.is_halted() {
                    break;
                }
                e.tick().unwrap();
            }
            assert!(e.is_halted(), "halted at {halt:?}");
            while u.machine().cycles < e.machine().cycles {
                u.step().unwrap();
            }
            let (el, ul) = (e.landed(), u.landed());
            if state(&el) != state(&ul) {
                eprintln!("halted at clock {halt:?}, checkpoint {checkpoint}");
                diff_state(&el, &ul, 0);
            }
            if checkpoint {
                let mut w = muir::checkpoint::Writer::new();
                e.save(&mut w);
                let bytes = w.finish();
                t.extend(e.trace.take().unwrap());
                let mut f = Pipeline::new(m.clone());
                f.load(&mut muir::checkpoint::Reader::for_word_bits(&bytes, 40)).unwrap();
                f.skip_sweep();
                f.trace = Some(Vec::new());
                e = f;
            }
            e.machine_mut().clock_control.run = true;
        }
        e.tick().unwrap();
    }
    t.extend(e.trace.take().unwrap());
    if e.machine().opc != OP_7 as u16 {
        return (t, e);
    }
    if let Some(k) = t.iter().position(|&p| p == Some(OP_7 as u16)) {
        t.truncate(k + 1);
    }
    for _ in 0..16 {
        e.step().unwrap();
    }
    let e = settled(e).unwrap_or_else(|(h, _)| panic!("halted settling: {h:?}"));
    (t, e)
}

/// **A halt at every clock of the main-loop machines** (A15b.13): D on and
/// off, fused returns, D's wait, a squash to restore, a wrong-path write;
/// halted at each clock, drained to `micro`'s state, run on, plain and
/// from a checkpoint, each ends as the run without the halt.
#[test]
fn a_halt_at_every_clock_of_the_main_loop_machines() {
    let mut halts = 0;
    for (what, m) in harness_machines() {
        let fresh = || {
            let mut u = Micro::new(m.clone());
            u.boot();
            u
        };
        let (t0, e0) = ml_halted_run(&m, None, false, &mut fresh());
        for checkpoint in [false, true] {
            let mut u = fresh();
            for c in 1..e0.clock() {
                let (t, e) = ml_halted_run(&m, Some(c), checkpoint, &mut u);
                if let Some(k) = (0..t.len().min(t0.len())).find(|&k| t[k] != t0[k]) {
                    let o = |t: &[Option<u16>]| {
                        t[k.saturating_sub(12)..(k + 4).min(t.len())]
                            .iter()
                            .map(|p| p.map_or("-".into(), |p| format!("{p:o}")))
                            .collect::<Vec<_>>()
                            .join(" ")
                    };
                    panic!(
                        "{what}, halted at {c}, checkpoint {checkpoint}: microcycle {k} differs:\n  {}\n  {}",
                        o(&t),
                        o(&t0)
                    );
                }
                assert_eq!(t.len(), t0.len(), "{what}, halted at {c}: the microcycles");
                if state(e.machine()) != state(e0.machine()) {
                    eprintln!("{what}, halted at {c}, checkpoint {checkpoint}: the end");
                    diff_state(e.machine(), e0.machine(), c);
                }
                halts += 1;
            }
        }
    }
    eprintln!("{halts} halts");
}

// --- The drain's restart, one squashed word (A15b.13) ---------------------------

/// **A halt with CS's word alone squashed keeps the address after it**
/// (A15b.13): an OA-REG-HIGH write, an unconditional jump RD resolves,
/// and in its delay slot a word that selects OA-REG-HIGH, held in CS while
/// the writer is in EX, so that the jump leaves RD for EX with its slot
/// behind and RD empty. A halt then squashes the slot alone; the jump's
/// target, which RD chose for the word after the slot and which no word
/// in EX checks, must follow it again. At every clock, plain and from a
/// checkpoint, the run ends as the one without the halt and as `micro`'s
/// (its select check off: the select is a word after its write's next).
#[test]
fn a_halt_that_squashes_one_word_keeps_the_address_after_it() {
    use muir::isa::asm::{ADD, OA_HIGH_SELECT};
    let mut p = Prog::default();
    let zero = p.k(0);
    let to = 0o40;
    p.op(ALU | SETA | a_src(zero) | fd(0o17));
    p.op(JUMP | ALWAYS | target(to));
    p.op(ALU | ADD | a_src(ONE) | m_src(0o26) | m_dest(0o26) | OA_HIGH_SELECT);
    // The word after the slot in sequence: the wrong path.
    p.op(ALU | ADD | a_src(ONE) | m_src(0o27) | m_dest(0o27));
    p.stop();
    while p.at() < to {
        p.fill(1);
    }
    p.op(ALU | ADD | a_src(ONE) | m_src(0o25) | m_dest(0o25));
    p.stop();
    let fresh = || {
        let mut u = Micro::new(machine(&p, REV16));
        u.oa_select_check = false;
        u.boot();
        u
    };
    let (t0, e0) = halted_run(&p, None, false, &mut fresh());
    let u = micro_unchecked(&p);
    assert_eq!(state(e0.machine()), state(u.machine()), "the run against micro");
    let m = e0.machine();
    assert_eq!(
        (m.mmem[0o25], m.mmem[0o26], m.mmem[0o27]),
        (1, 1, 0),
        "the slot ran, then the target"
    );
    assert!(e0.meters.oa_hold > 0, "the slot was held in CS");
    for checkpoint in [false, true] {
        let mut u = fresh();
        for c in 1..e0.clock() {
            let (t, e) = halted_run(&p, Some(c), checkpoint, &mut u);
            assert_eq!(t, t0, "halted at {c}, checkpoint {checkpoint}: the microcycles");
            assert_eq!(
                state(e.machine()),
                state(e0.machine()),
                "halted at {c}, checkpoint {checkpoint}: the end"
            );
        }
    }
}

/// **A halt keeps D's wait** (A15b.9, A15b.13): opcode 1's handler
/// returns by D, its return word writing the PDL pointer and the delay
/// slot reading the buffer through it, so that the slot waits in CS while
/// the return is in EX and is still in RD when the return commits. A halt
/// then squashes the slot, which makes the stream's fetch D waits for; on
/// the run's resumption the slot is fetched again and the handler only
/// once D's word is here, as without the halt. At every clock, plain and
/// from a checkpoint, the run ends as the one without the halt.
#[test]
fn a_halt_keeps_d_s_wait_for_the_stream_s_word() {
    use muir::isa::asm::src;
    let mut m = d_machine(REV16, d_register(true), &ONES, CODE, false, false);
    m.amem[0o56] = 0o100;
    m.pdl[0o100] = 0o4242;
    m.imem[(OP_1 + 1) as usize] = Insn::extended(ALU | SETA | a_src(0o56) | fd(0o14) | POPJ);
    m.imem[(OP_1 + 2) as usize] = Insn::extended(ALU | SETM | src(0o25) | m_dest(0o27));
    let fresh = || {
        let mut u = Micro::new(m.clone());
        u.boot();
        u
    };
    let (t0, e0) = ml_halted_run(&m, None, false, &mut fresh());
    let u = {
        let mut u = fresh();
        while u.machine().opc != OP_7 as u16 {
            u.step().unwrap();
        }
        for _ in 0..16 {
            u.step().unwrap();
        }
        u
    };
    assert_eq!(state(e0.machine()), state(u.machine()), "the run against micro");
    assert_eq!(e0.machine().mmem[0o27], 0o4242, "the slot read the buffer");
    assert!(e0.meters.pdl_wait > 0, "the slot waited in CS");
    assert!(e0.machine().macro_dispatch.fused > 0, "D fused returns");
    for checkpoint in [false, true] {
        let mut u = fresh();
        for c in 1..e0.clock() {
            let (t, e) = ml_halted_run(&m, Some(c), checkpoint, &mut u);
            if let Some(k) = (0..t.len().min(t0.len())).find(|&k| t[k] != t0[k]) {
                panic!("halted at {c}, checkpoint {checkpoint}: microcycle {k} differs");
            }
            assert_eq!(
                t.len(),
                t0.len(),
                "halted at {c}, checkpoint {checkpoint}: the microcycles"
            );
            assert_eq!(
                state(e.machine()),
                state(e0.machine()),
                "halted at {c}, checkpoint {checkpoint}: the end"
            );
        }
    }
}

// --- A register write's interrupt, a read after a read (MP2b rulings 2) --------

/// Interrupts enabled: INTERRUPT-CONTROL `<27>`, which a 40-bit machine's
/// destination 2 takes from the word's `<35>` (A1.6).
fn enable_interrupts(p: &mut Prog) {
    let v = p.k(1 << 35);
    p.op(ALU | SETA | a_src(v) | fd(0o2));
}

/// Block-disk's command, register-page word 200, written with `v`; its
/// `<11>` with the disk idle raises word 100 `<3>`, the interrupt.
fn block_disk_command_start(p: &mut Prog, v: Word) {
    let (w, at) = (p.k(v), p.k(REGISTER_PAGE | 0o200));
    p.op(ALU | SETA | a_src(w) | MD);
    p.op(ALU | SETA | a_src(at) | START_WRITE);
}

/// Two checks of condition 5 (page fault or interrupt), N set, calling
/// the handlers at `SUBS[0]`, which counts in M 26, and `SUBS[1]`, in M 27.
fn two_checks(p: &mut Prog) {
    p.op(jcond(5) | P | N | target(SUBS[0]));
    p.op(jcond(5) | P | N | target(SUBS[1]));
}

/// The handlers [`two_checks`] call, after the program's stop.
fn check_handlers(p: &mut Prog) {
    use muir::isa::asm::ADD;
    for (k, slot) in [(0, 0o26), (1, 0o27)] {
        while p.at() < SUBS[k] {
            p.fill(1);
        }
        p.op(ALU | ADD | a_src(ONE) | m_src(slot) | m_dest(slot));
        p.op(filler().raw() | POPJ);
        p.fill(1);
    }
}

/// **A register write's interrupt is taken by the next check that tests
/// it, not the one right after the start** (MP2b rulings 2, Q1, test 1): a
/// write of block-disk's command with `<11>`, the disk idle, raises word 100
/// `<3>`; the check right after the start does not call, the one after it
/// does. On both engines, plain and under neutral time.
#[test]
fn a_register_write_s_interrupt_is_seen_by_the_check_after_the_next() {
    let mut p = Prog::default();
    enable_interrupts(&mut p);
    block_disk_command_start(&mut p, 1 << 11);
    two_checks(&mut p);
    p.fill(2);
    p.stop();
    check_handlers(&mut p);
    for neutral in [false, true] {
        let u = {
            let mut m = machine(&p, REV16);
            with_block_disk(&mut m);
            let mut u = Micro::new(m);
            u.neutral = neutral;
            match run_engine(u) {
                Ok(u) => u,
                Err((h, _)) => panic!("micro halted: {h:?}"),
            }
        };
        let mut m = machine(&p, REV16);
        with_block_disk(&mut m);
        let e = ends_on(m, &|e| e.neutral = neutral);
        for (what, m) in [("micro", u.machine()), ("rtl", e.machine())] {
            assert_eq!(
                (m.mmem[0o26], m.mmem[0o27]),
                (0, 1),
                "neutral {neutral}, {what}: the second check calls"
            );
        }
        assert_eq!(state(e.machine()), state(u.machine()), "neutral {neutral}: the ends");
    }
}

/// **A register write waits for the word right after its start to leave
/// EX** (MP2b rulings 2, Q1; the MD-after-write ruling's release): the
/// check right after the start selects OA-REG-HIGH, written two words
/// before it, so CS holds it a clock (A15b.15) and it reaches EX after the
/// write's grant; the write still waits for it, so it does not call and the
/// check after it does, as on `micro` (its select check off). A write taken
/// the clock after its grant, whatever the word after it, is caught.
#[test]
fn a_register_write_waits_for_the_word_after_its_start_held_in_cs() {
    use muir::isa::asm::OA_HIGH_SELECT;
    let mut p = Prog::default();
    enable_interrupts(&mut p);
    let (w, at, z) = (p.k(1 << 11), p.k(REGISTER_PAGE | 0o200), p.k(0));
    p.op(ALU | SETA | a_src(w) | MD);
    p.op(ALU | SETA | a_src(z) | fd(0o17));
    p.op(ALU | SETA | a_src(at) | START_WRITE);
    p.op(jcond(5) | P | N | target(SUBS[0]) | OA_HIGH_SELECT);
    p.op(jcond(5) | P | N | target(SUBS[1]));
    p.fill(2);
    p.stop();
    check_handlers(&mut p);
    let mut m = machine(&p, REV16);
    with_block_disk(&mut m);
    let mut u = Micro::new(m.clone());
    u.oa_select_check = false;
    let u = run_engine(u).unwrap_or_else(|(h, _)| panic!("micro halted: {h:?}"));
    // The reset's sweep skipped, so that WB grants the write at once and
    // the check, held in CS, reaches EX after the grant.
    let mut e = Pipeline::new(m);
    e.boot();
    e.skip_sweep();
    while e.machine().opc != STOP as u16 {
        e.step().unwrap();
    }
    let e = settled(e).unwrap_or_else(|(h, _)| panic!("the pipeline halted: {h:?}"));
    assert_eq!(e.meters.oa_hold, 1, "the check held a clock in CS");
    for (what, m) in [("micro", u.machine()), ("rtl", e.machine())] {
        assert_eq!((m.mmem[0o26], m.mmem[0o27]), (0, 1), "{what}: the second check calls");
    }
    assert_eq!(state(e.machine()), state(u.machine()), "the ends");
}

/// **The reverse** (MP2b rulings 2, Q1, test 2): with the level up, a write
/// of the command with 0 lowers it; the check right after the start still
/// calls, and the check after it, once the handler has returned, does not.
#[test]
fn a_register_write_that_lowers_the_level_is_seen_by_the_check_after_the_next() {
    let mut p = Prog::default();
    enable_interrupts(&mut p);
    block_disk_command_start(&mut p, 1 << 11);
    p.fill(4);
    block_disk_command_start(&mut p, 0);
    two_checks(&mut p);
    p.fill(2);
    p.stop();
    check_handlers(&mut p);
    let e = same_on(&p, &with_block_disk, &|_| {});
    assert_eq!((e.machine().mmem[0o26], e.machine().mmem[0o27]), (1, 0));
}

/// **A register read right after a register write reads the device as the
/// write left it** (MP2b rulings 2, F1; A15b.3, "A start right after a
/// start": both land): the write of test 1, then a read of word 100 in the
/// next word; `<3>` reads up on both engines.
#[test]
fn a_register_read_right_after_a_register_write_reads_it_written() {
    let mut p = Prog::default();
    enable_interrupts(&mut p);
    block_disk_command_start(&mut p, 1 << 11);
    let word_100 = p.k(REGISTER_PAGE | 0o100);
    p.op(ALU | SETA | a_src(word_100) | START_READ);
    p.fill(1);
    p.op(ALU | SETM | SRC_MD | m_dest(0o26));
    p.stop();
    let e = same_on(&p, &with_block_disk, &|_| {});
    assert_eq!(e.machine().mmem[0o26] & 1 << 3, 1 << 3, "word 100 <3> up");
}

/// **A harness boundary between a register write's start and the next
/// word** (MP2b rulings 2, Q1, test 4): a return with `SPC<14>` arms the
/// LC step in its delay slot, which starts the write of test 1, so that
/// the harness's boundary falls right after the start. `micro` sends the
/// waiting write out at the boundary, as its halt does, and the pipeline's
/// drain takes it: the digests are alike, and so are the ends.
#[test]
fn a_boundary_between_a_register_write_and_the_next_word_digests_alike() {
    use support::neutral::{Digests, Neutral};
    let mut p = Prog::default();
    enable_interrupts(&mut p);
    let (w, reg) = (p.k(1 << 11), p.k(REGISTER_PAGE | 0o200));
    p.op(ALU | SETA | a_src(w) | MD);
    // A return to `at` + 2 with `SPC<14>`: LC steps in the microcycle
    // after it, its delay slot, which starts the write; the checks are at
    // the return's target.
    let at = 0o100;
    let ret = p.k(at | 1 << 14);
    p.op(ALU | SETA | a_src(ret) | fd(0o15));
    p.fill(1);
    p.op(filler().raw() | POPJ);
    p.op(ALU | SETA | a_src(reg) | START_WRITE);
    while p.at() < at + 2 {
        p.fill(1);
    }
    two_checks(&mut p);
    p.fill(2);
    p.stop();
    check_handlers(&mut p);
    fn run<E: Neutral>(mut e: Digests<E>) -> Digests<E> {
        e.engine.time_neutral();
        e.boot();
        for _ in 0..20_000 {
            if e.machine().opc == STOP as u16 {
                break;
            }
            e.step().unwrap();
        }
        for _ in 0..16 {
            e.step().unwrap();
        }
        e
    }
    let mut m = machine(&p, REV16);
    with_block_disk(&mut m);
    let u = run(Digests::new(Micro::new(m.clone()), 1));
    let mut x = Pipeline::new(m);
    x.skip_sweep();
    let mut d = Digests::new(x, 1);
    d.engine.time_neutral();
    d.boot();
    d.engine.skip_sweep();
    for _ in 0..20_000 {
        if d.machine().opc == STOP as u16 {
            break;
        }
        d.step().unwrap();
    }
    for _ in 0..16 {
        d.step().unwrap();
    }
    let e = settled(d.engine).unwrap_or_else(|(h, _)| panic!("halted settling: {h:?}"));
    assert_eq!(u.lines.len(), 1, "one boundary, at the start: {:?}", u.lines);
    // Sent out at the boundary, the write is seen by the check right after
    // it, and the level still up by the one after, on both engines alike.
    assert_eq!((e.machine().mmem[0o26], e.machine().mmem[0o27]), (1, 1), "both checks call");
    assert_eq!(d.lines, u.lines, "the digests");
    assert_eq!(state(e.machine()), state(u.engine.machine()), "the ends");
}

// --- A register read's side effect (MP2b ruling Q4) ----------------------------

/// The keyboard's interrupt up: a key word waiting under 120's enable.
fn key_waiting(m: &mut Machine) {
    use muir::quux_input::{ENABLE, KBD_STATUS, KeyboardMouse};
    m.quux_input.write(KBD_STATUS, ENABLE);
    m.quux_input.press(0o101);
}

/// The mouse's interrupt up: a move under 123's enable.
fn mouse_moved(m: &mut Machine) {
    use muir::quux_input::{ENABLE, KeyboardMouse, MOUSE_STATUS};
    m.quux_input.write(MOUSE_STATUS, ENABLE);
    m.quux_input.mouse_move(1, 0);
}

/// A read of register-page word `word`, `gap` words, then [`two_checks`].
fn read_then_checks(word: Word, gap: usize) -> Prog {
    let mut p = Prog::default();
    enable_interrupts(&mut p);
    let at = p.k(REGISTER_PAGE | word);
    p.op(ALU | SETA | a_src(at) | START_READ);
    p.fill(gap);
    two_checks(&mut p);
    p.fill(2);
    p.stop();
    check_handlers(&mut p);
    p
}

/// **A register read's side effect counts from the clock after its grant**
/// (MP2b ruling Q4): with the keyboard's interrupt up, a read of 121 takes
/// the only key word and lowers it; with the mouse's up, a read of 122
/// clears 123 `<0>` and lowers it. The check right after the read's start
/// still sees the level up and calls; a check one word later sees it down
/// and does not. On both engines, plain and under neutral time. A side
/// effect seen in the grant's own clock fails the first half; one held a
/// clock too long, the second.
#[test]
fn a_register_read_s_side_effect_is_seen_from_the_clock_after_its_grant() {
    type Setup = fn(&mut Machine);
    let cases: [(&str, Word, Setup); 2] =
        [("121", 0o121, key_waiting), ("122", 0o122, mouse_moved)];
    let mut wrong = Vec::new();
    for (what, word, setup) in cases {
        for (gap, calls) in [(0, (1, 0)), (1, (0, 0))] {
            let p = read_then_checks(word, gap);
            for neutral in [false, true] {
                let mut m = machine(&p, REV16);
                setup(&mut m);
                assert_ne!(m.quux_input.interrupts(), 0, "{what}: the level up before the read");
                let u = {
                    let mut u = Micro::new(m.clone());
                    u.neutral = neutral;
                    match run_engine(u) {
                        Ok(u) => u,
                        Err((h, _)) => panic!("micro halted: {h:?}"),
                    }
                };
                let e = ends_on(m, &|e| e.neutral = neutral);
                for (engine, m) in [("micro", u.machine()), ("rtl", e.machine())] {
                    let got = (m.mmem[0o26], m.mmem[0o27]);
                    if got != calls {
                        wrong.push(format!(
                            "read of {what}, gap {gap}, neutral {neutral}, {engine}: \
                             calls {got:?}, not {calls:?}"
                        ));
                    }
                    if m.quux_input.interrupts() != 0 {
                        wrong.push(format!("read of {what}, {engine}: the level still up"));
                    }
                }
                if state(e.machine()) != state(u.machine()) {
                    wrong.push(format!(
                        "read of {what}, gap {gap}, neutral {neutral}: the ends differ"
                    ));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// [`two_checks`]' calls on `micro` and on the pipeline set by `set`, the
/// machine set up by `setup`, both neutral or not; whether the ends agree;
/// and the pipeline's events.
#[allow(clippy::type_complexity)]
fn read_calls(
    p: &Prog,
    setup: fn(&mut Machine),
    neutral: bool,
    set: &dyn Fn(&mut Pipeline),
) -> ((Word, Word), (Word, Word), bool, Vec<(u64, muir::pipeline::Event)>) {
    let mut m = machine(p, REV16);
    setup(&mut m);
    let mut u = Micro::new(m.clone());
    u.neutral = neutral;
    let u = run_engine(u).unwrap_or_else(|(h, _)| panic!("micro halted: {h:?}"));
    let (e, ev) = {
        let mut e = Pipeline::new(m);
        e.neutral = neutral;
        set(&mut e);
        e.events = Some(Vec::new());
        // The reset's sweep skipped, so that WB grants the read at once.
        e.boot();
        e.skip_sweep();
        while e.machine().opc != STOP as u16 {
            e.step().unwrap();
        }
        let mut e = settled(e).unwrap_or_else(|(h, _)| panic!("the pipeline halted: {h:?}"));
        let ev = e.events.take().unwrap();
        (e, ev)
    };
    let calls = |m: &Machine| (m.mmem[0o26], m.mmem[0o27]);
    let same = state(e.machine()) == state(u.machine());
    (calls(u.machine()), calls(e.machine()), same, ev)
}

/// The clock the register read at `bus` was granted, and the clock the
/// word at `pc` first committed after it.
fn grant_and_commit(ev: &[(u64, muir::pipeline::Event)], bus: u32, pc: u64) -> (u64, u64) {
    use muir::pipeline::Event;
    let grant = ev
        .iter()
        .find(|(_, e)| matches!(e, Event::Grant(b, false) if *b == bus))
        .map(|&(c, _)| c)
        .expect("the read granted");
    let commit = ev
        .iter()
        .find(|(c, e)| *c >= grant && matches!(e, Event::Commit(Some(a)) if u64::from(*a) == pc))
        .map(|&(c, _)| c)
        .expect("the check committed");
    (grant, commit)
}

/// A conditional jump decided in EX, taken (M 0 equals A 0), hinted by
/// `hint`, N clear: its delay slot reads register-page word `word`; its
/// target, [`SLOT_TARGET`], is [`two_checks`].
fn read_in_a_slot(word: Word, hint: u64) -> Prog {
    let mut p = Prog::default();
    enable_interrupts(&mut p);
    // The register page's entry in the TLB first, so that the read is
    // granted as it reaches WB.
    p.read(REGISTER_PAGE | 0o120, 0o25);
    let at = p.k(REGISTER_PAGE | word);
    p.fill(3);
    p.op(jcond(3) | m_src(M_ZERO) | a_src(ZERO) | target(SLOT_TARGET) | hint);
    p.op(ALU | SETA | a_src(at) | START_READ);
    p.stop();
    while p.at() < SLOT_TARGET {
        p.fill(1);
    }
    two_checks(&mut p);
    p.fill(2);
    p.stop();
    check_handlers(&mut p);
    p
}

/// [`read_in_a_slot`]'s target.
const SLOT_TARGET: u64 = 0o200;

/// The register page's bus address of word `word`.
fn page_bus(word: Word) -> u32 {
    muir::tlb::REGISTER_PAGE_BUS | word as u32
}

/// **A register read's side effect is held until the word right after its
/// start has left EX, however late that word reaches EX** (register read
/// ruling; A15b.3, "A device register"): a read of 121 or 122 in the delay
/// slot of a conditional jump decided in EX, its target the two checks; the
/// first check calls and the second does not, on both engines, the hint
/// right or wrong, with one bubble or two (A15b.14's fallback), plain and
/// neutral, the ends alike. A wrong hint with two bubbles brings the target
/// to EX after the grant, which the events show; with one the slot waits a
/// clock in RD and the target follows it at once. The change counted at the
/// clock after the grant is caught where the target is late.
#[test]
fn a_register_read_in_a_mispredicted_slot_holds_the_level_for_the_target() {
    use muir::isa::asm::HINT;
    type Setup = fn(&mut Machine);
    let cases: [(&str, Word, Setup); 2] =
        [("121", 0o121, key_waiting), ("122", 0o122, mouse_moved)];
    let mut wrong = Vec::new();
    for (what, word, setup) in cases {
        for (hinted, hint) in [("right", HINT), ("wrong", 0)] {
            let p = read_in_a_slot(word, hint);
            for bubbles in [1, 2] {
                for neutral in [false, true] {
                    let (u, e, same, ev) = read_calls(&p, setup, neutral, &|e| e.bubbles = bubbles);
                    let (grant, commit) = grant_and_commit(&ev, page_bus(word), SLOT_TARGET);
                    let case = format!(
                        "{what}, hint {hinted}, {bubbles} bubble(s), neutral {neutral}: \
                         granted at {grant}, the target committed at {commit}"
                    );
                    // One bubble moves it ahead of the slot, which then
                    // waits a clock in RD (P2), so the target follows the
                    // start; with two the target reaches EX after the grant.
                    let late = hinted == "wrong" && bubbles == 2;
                    if late != (commit > grant) {
                        wrong.push(format!("{case}: the target late {}", commit > grant));
                    }
                    for (engine, calls) in [("micro", u), ("rtl", e)] {
                        if calls != (1, 0) {
                            wrong.push(format!("{case}, {engine}: calls {calls:?}, not (1, 0)"));
                        }
                    }
                    if !same {
                        wrong.push(format!("{case}: the ends differ"));
                    }
                    eprintln!("{case}; micro {u:?}, rtl {e:?}");
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// **A register read's side effect is held for the word right after its
/// start, held in CS** (register read ruling): as
/// [`a_register_write_waits_for_the_word_after_its_start_held_in_cs`], a
/// read of 121 or 122 whose next word, the first check, selects OA-REG-HIGH
/// written two words before, so CS holds it a clock and it reaches EX after
/// the read's grant; it still calls, and the check after it does not, as on
/// `micro` (its select check off). The change counted at the clock after
/// the grant is caught.
#[test]
fn a_register_read_holds_the_level_for_the_word_after_its_start_held_in_cs() {
    use muir::isa::asm::OA_HIGH_SELECT;
    type Setup = fn(&mut Machine);
    let cases: [(&str, Word, Setup); 2] =
        [("121", 0o121, key_waiting), ("122", 0o122, mouse_moved)];
    for (what, word, setup) in cases {
        let mut p = Prog::default();
        enable_interrupts(&mut p);
        let (at, z) = (p.k(REGISTER_PAGE | word), p.k(0));
        p.op(ALU | SETA | a_src(z) | fd(0o17));
        p.op(ALU | SETA | a_src(at) | START_READ);
        p.op(jcond(5) | P | N | target(SUBS[0]) | OA_HIGH_SELECT);
        p.op(jcond(5) | P | N | target(SUBS[1]));
        p.fill(2);
        p.stop();
        check_handlers(&mut p);
        let mut m = machine(&p, REV16);
        setup(&mut m);
        let mut u = Micro::new(m.clone());
        u.oa_select_check = false;
        let u = run_engine(u).unwrap_or_else(|(h, _)| panic!("micro halted: {h:?}"));
        let mut e = Pipeline::new(m);
        e.events = Some(Vec::new());
        e.boot();
        e.skip_sweep();
        while e.machine().opc != STOP as u16 {
            e.step().unwrap();
        }
        let mut e = settled(e).unwrap_or_else(|(h, _)| panic!("the pipeline halted: {h:?}"));
        let ev = e.events.take().unwrap();
        assert_eq!(e.meters.oa_hold, 1, "{what}: the check held a clock in CS");
        let (grant, commit) = grant_and_commit(&ev, page_bus(word), 3);
        assert!(commit > grant, "{what}: the check in EX after the grant ({grant}, {commit})");
        for (engine, m) in [("micro", u.machine()), ("rtl", e.machine())] {
            assert_eq!((m.mmem[0o26], m.mmem[0o27]), (1, 0), "{what}, {engine}: the first calls");
        }
        assert_eq!(state(e.machine()), state(u.machine()), "{what}: the ends");
    }
}

/// **A harness boundary between a register read's start and the next
/// word** (register read ruling; as
/// [`a_boundary_between_a_register_write_and_the_next_word_digests_alike`]):
/// a return with `SPC<14>` arms the LC step in its delay slot, which reads
/// 121, so that the boundary falls right after the start. `micro` counts
/// the read's change at the boundary, as at a halt, and the pipeline's
/// drain does: neither check calls, the digests are alike, and so are the
/// ends.
#[test]
fn a_boundary_between_a_register_read_and_the_next_word_digests_alike() {
    use support::neutral::{Digests, Neutral};
    let mut p = Prog::default();
    enable_interrupts(&mut p);
    let (kbd, ret) = (p.k(REGISTER_PAGE | 0o121), p.k(0o100 | 1 << 14));
    p.op(ALU | SETA | a_src(ret) | fd(0o15));
    p.fill(1);
    p.op(filler().raw() | POPJ);
    p.op(ALU | SETA | a_src(kbd) | START_READ);
    while p.at() < 0o100 + 2 {
        p.fill(1);
    }
    two_checks(&mut p);
    p.fill(2);
    p.stop();
    check_handlers(&mut p);
    fn to_stop<E: Neutral>(d: &mut Digests<E>) {
        for _ in 0..20_000 {
            if d.machine().opc == STOP as u16 {
                break;
            }
            d.step().unwrap();
        }
        for _ in 0..16 {
            d.step().unwrap();
        }
    }
    let mut m = machine(&p, REV16);
    key_waiting(&mut m);
    let mut u = Digests::new(Micro::new(m.clone()), 1);
    u.engine.time_neutral();
    u.boot();
    to_stop(&mut u);
    let mut x = Pipeline::new(m);
    x.skip_sweep();
    let mut d = Digests::new(x, 1);
    d.engine.time_neutral();
    d.boot();
    d.engine.skip_sweep();
    to_stop(&mut d);
    let e = settled(d.engine).unwrap_or_else(|(h, _)| panic!("halted settling: {h:?}"));
    assert_eq!(u.lines.len(), 1, "one boundary, at the start: {:?}", u.lines);
    assert_eq!((e.machine().mmem[0o26], e.machine().mmem[0o27]), (0, 0), "neither check calls");
    assert_eq!(d.lines, u.lines, "the digests");
    assert_eq!(state(e.machine()), state(u.engine.machine()), "the ends");
}

// --- A halt between a register access's start and the next instruction ----------

/// The three register accesses: a read of 121 with a key waiting, a read of
/// 122 with the mouse moved, each then [`two_checks`]; and the write of
/// block-disk's command that raises word 100 `<3>`, then [`two_checks`].
/// Each with its machine's setup, its end unhalted, and its end halted
/// between the start and the next instruction.
#[allow(clippy::type_complexity)]
fn halt_accesses() -> Vec<(&'static str, Prog, fn(&mut Machine), (Word, Word), (Word, Word))> {
    let mut write = Prog::default();
    enable_interrupts(&mut write);
    block_disk_command_start(&mut write, 1 << 11);
    two_checks(&mut write);
    write.fill(2);
    write.stop();
    check_handlers(&mut write);
    vec![
        ("read of 121", read_then_checks(0o121, 0), key_waiting, (1, 0), (0, 0)),
        ("read of 122", read_then_checks(0o122, 0), mouse_moved, (1, 0), (0, 0)),
        ("write of 200", write, with_block_disk, (0, 1), (1, 1)),
    ]
}

/// What a halted access's run is compared by: [`two_checks`]' calls, the
/// architectural state, and the input's interrupt level.
fn access_end(m: &Machine) -> impl PartialEq + std::fmt::Debug {
    ((m.mmem[0o26], m.mmem[0o27]), state(m), m.quux_input.interrupts())
}

/// The pipeline on `m` to its stop, halted at clock `halt` and run on, from
/// a checkpoint taken there with `checkpoint`; settled. The microcycles
/// completed at the halt (none if the stop came first), the state halted,
/// and the end.
fn pipeline_access_run(
    m: &Machine,
    halt: Option<u64>,
    checkpoint: bool,
) -> (Option<u64>, Option<Machine>, Pipeline) {
    let mut e = Pipeline::new(m.clone());
    e.boot();
    e.skip_sweep();
    let (mut at, mut halted) = (None, None);
    for _ in 0..20_000 {
        if e.machine().opc == STOP as u16 {
            break;
        }
        if Some(e.clock() + 1) == halt {
            e.machine_mut().clock_control.run = false;
            for _ in 0..10_000 {
                if e.is_halted() {
                    break;
                }
                e.tick().unwrap();
            }
            assert!(e.is_halted(), "the halt at clock {halt:?} drained");
            at = Some(e.machine().cycles);
            halted = Some(e.landed());
            if checkpoint {
                let mut w = muir::checkpoint::Writer::new();
                e.save(&mut w);
                let bytes = w.finish();
                let mut f = Pipeline::new(m.clone());
                f.load(&mut muir::checkpoint::Reader::for_word_bits(&bytes, 40)).unwrap();
                f.skip_sweep();
                e = f;
            }
            e.machine_mut().clock_control.run = true;
        }
        e.tick().unwrap();
    }
    assert_eq!(e.machine().opc, STOP as u16, "halted at clock {halt:?}: the stop reached");
    for _ in 0..16 {
        e.step().unwrap();
    }
    let e = settled(e).unwrap_or_else(|(h, _)| panic!("halted settling: {h:?}"));
    (at, halted, e)
}

/// `micro` on `m` to its stop, the clock control's RUN dropped once
/// `halt` microcycles have run, so that it halts after the next, which
/// latches it; set again a few master clock cycles later; with `checkpoint`, run on from a checkpoint taken while halted.
/// The microcycles completed at the halt (none if the stop came first),
/// the state halted, and the end.
fn micro_access_run(
    m: &Machine,
    halt: Option<u64>,
    checkpoint: bool,
) -> (Option<u64>, Option<Machine>, Micro) {
    let mut u = Micro::new(m.clone());
    u.boot();
    let (mut at, mut halted) = (None, None);
    for _ in 0..20_000 {
        if u.machine().opc == STOP as u16 {
            break;
        }
        if Some(u.machine().cycles) == halt && at.is_none() {
            u.spy_write(muir::spy::CLK, 0);
            for _ in 0..4 {
                u.step().unwrap();
            }
            let cycles = u.machine().cycles;
            u.step().unwrap();
            assert_eq!(u.machine().cycles, cycles, "halted after {halt:?} microcycles");
            at = Some(cycles);
            halted = Some(u.landed());
            if checkpoint {
                let mut w = muir::checkpoint::Writer::new();
                u.save(&mut w);
                let bytes = w.finish();
                let mut f = Micro::new(m.clone());
                f.load(&mut muir::checkpoint::Reader::for_word_bits(&bytes, 40)).unwrap();
                u = f;
            }
            u.spy_write(muir::spy::CLK, 1);
            continue;
        }
        u.step().unwrap();
    }
    assert_eq!(u.machine().opc, STOP as u16, "halted at {halt:?}: the stop reached");
    for _ in 0..16 {
        u.step().unwrap();
    }
    (at, halted, u)
}

/// **A halt between a register access's start and the next instruction
/// sends the access out** (A15b.13, "The halt"; the halt-release
/// clarification): over a read of 121, a read of 122 and a write of
/// block-disk's command, each followed by [`two_checks`], the pipeline is
/// halted at every clock and `micro` after every microcycle, and each run
/// resumes to its stop. Every end is the unhalted one, (1, 0) for the
/// reads and (0, 1) for the write, except at exactly one halt point on
/// each engine, the one between the start and the next instruction: there
/// the read's change counts and the write is taken at the halt, so the
/// check right after the start sees the level as the access leaves it,
/// (0, 0) for the reads and (1, 1) for the write. At every halt point the
/// engines agree, halted and at the end; a checkpoint taken at that one
/// point resumes to the same end. A halt that keeps a read's hold past the
/// next instruction is caught at that point; one that completes before a
/// write it started is taken, by the checkpoint taken there.
#[test]
fn a_halt_between_a_register_access_s_start_and_the_next_instruction_sends_it_out() {
    let mut wrong = Vec::new();
    for (what, p, setup, unhalted, sent) in halt_accesses() {
        let mut m = machine(&p, REV16);
        setup(&mut m);
        let (_, _, e0) = pipeline_access_run(&m, None, false);
        let (_, _, u0) = micro_access_run(&m, None, false);
        let calls = |m: &Machine| (m.mmem[0o26], m.mmem[0o27]);
        assert_eq!(calls(e0.machine()), unhalted, "{what}: rtl unhalted");
        assert_eq!(calls(u0.machine()), unhalted, "{what}: micro unhalted");
        assert_eq!(access_end(e0.machine()), access_end(u0.machine()), "{what}: unhalted ends");
        // micro, halted after each microcycle: its end and its halted state
        // by the microcycles completed.
        let mut micro_at = std::collections::BTreeMap::new();
        let mut micro_differ = Vec::new();
        for k in 0..u0.machine().cycles {
            let (at, halted, u) = micro_access_run(&m, Some(k), false);
            let Some(at) = at else { continue };
            if access_end(u.machine()) != access_end(u0.machine()) {
                micro_differ.push((k, at, calls(u.machine())));
            }
            micro_at.insert(at, (halted.unwrap(), u));
        }
        // The pipeline, halted at each clock.
        let mut rtl_differ = Vec::new();
        for c in 1..e0.clock() {
            let (at, halted, e) = pipeline_access_run(&m, Some(c), false);
            let Some(at) = at else { continue };
            if access_end(e.machine()) != access_end(e0.machine()) {
                rtl_differ.push((c, at, calls(e.machine())));
            }
            // Halts before the first microcycle completes, ahead of the
            // access, have no `micro` halt to match: `micro` latches RUN at
            // the end of a microcycle.
            if at == 0 {
                continue;
            }
            match micro_at.get(&at) {
                None => wrong.push(format!(
                    "{what}: rtl halted at clock {c}, {at} microcycles, no micro halt there"
                )),
                Some((uh, u)) => {
                    let eh = halted.unwrap();
                    if (state(&eh), eh.quux_input.interrupts())
                        != (state(uh), uh.quux_input.interrupts())
                    {
                        wrong.push(format!("{what}: halted at clock {c}, {at} microcycles: the halted states differ"));
                    }
                    if access_end(e.machine()) != access_end(u.machine()) {
                        wrong.push(format!(
                            "{what}: halted at clock {c}, {at} microcycles: rtl ends {:?}, micro {:?}",
                            calls(e.machine()),
                            calls(u.machine())
                        ));
                    }
                }
            }
        }
        eprintln!(
            "{what}: unhalted {unhalted:?}; rtl differs at {rtl_differ:?}; micro at {micro_differ:?}"
        );
        match (&rtl_differ[..], &micro_differ[..]) {
            ([(c, at, got)], [(j, k, ugot)]) => {
                if at != k {
                    wrong.push(format!("{what}: rtl's point at {at} microcycles, micro's at {k}"));
                }
                for (engine, got) in [("rtl", got), ("micro", ugot)] {
                    if *got != sent {
                        wrong.push(format!("{what}, {engine}: the halt's end {got:?}, not {sent:?}"));
                    }
                }
                // A checkpoint taken at that point resumes to the same end.
                let (_, _, e) = pipeline_access_run(&m, Some(*c), false);
                let (_, _, f) = pipeline_access_run(&m, Some(*c), true);
                if access_end(f.machine()) != access_end(e.machine()) {
                    wrong.push(format!("{what}: rtl's checkpoint at clock {c} ends otherwise"));
                }
                let (_, _, u) = micro_access_run(&m, Some(*j), false);
                let (_, _, v) = micro_access_run(&m, Some(*j), true);
                if calls(u.machine()) != sent || access_end(v.machine()) != access_end(u.machine()) {
                    wrong.push(format!("{what}: micro's checkpoint after {k} microcycles ends otherwise"));
                }
            }
            _ => wrong.push(format!(
                "{what}: not one differing halt point on each engine: rtl {rtl_differ:?}, micro {micro_differ:?}"
            )),
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// **A register read right after a read of 121 finds the key word taken**
/// (MP2b ruling Q4; A15b.3, "A start right after a start"): the second
/// start, of word 100, is held behind the first and taken after its side
/// effect counts, so `<4>` reads down, on both engines. The held level
/// read by word 100 as well as by the checks is caught.
#[test]
fn a_register_read_right_after_a_read_of_121_finds_the_key_taken() {
    let mut p = Prog::default();
    let (kbd, word_100) = (p.k(REGISTER_PAGE | 0o121), p.k(REGISTER_PAGE | 0o100));
    p.op(ALU | SETA | a_src(kbd) | START_READ);
    p.op(ALU | SETA | a_src(word_100) | START_READ);
    p.fill(1);
    p.op(ALU | SETM | SRC_MD | m_dest(0o26));
    p.stop();
    for neutral in [false, true] {
        let e = same_on(&p, &key_waiting, &|e| e.neutral = neutral);
        assert_eq!(e.machine().mmem[0o26] & 1 << 4, 0, "neutral {neutral}: word 100 <4> down");
    }
}

/// **A read start right after a read start: both land** (A15b.3, "A start
/// right after a start"; MP2b rulings 2, Q2): the second start is held
/// behind the first; the word right after the second start reads the
/// first's word, and the word after that the second's. The second a read
/// of main memory, missing and hitting, or of a register. On both engines;
/// the first word dropped is caught.
#[test]
fn row_a_read_start_right_after_a_read_start_both_land() {
    let mut caught = 0;
    for (kind, second) in
        [("a miss", PHYS | 0o50), ("a hit", PHYS | 0o50), ("a register", REGISTER_PAGE)]
    {
        let mut p = Prog::default();
        p.main.push((0o40, 5));
        p.main.push((0o50, 7));
        let (two, a, b) = (p.k(2), p.k(PHYS | 0o40), p.k(second));
        if kind == "a hit" {
            p.read(PHYS | 0o40, 0o20);
            p.read(PHYS | 0o50, 0o21);
        }
        p.op(ALU | SETA | a_src(two) | MD);
        p.op(ALU | SETA | a_src(a) | START_READ);
        p.op(ALU | SETA | a_src(b) | START_READ);
        p.op(ALU | SETM | SRC_MD | m_dest(0o26));
        p.op(ALU | SETM | SRC_MD | m_dest(0o27));
        p.fill(2);
        p.stop();
        let e = same(&p);
        let m = e.machine();
        let second_word = if kind == "a register" { m.mmem[0o27] } else { 7 };
        assert_eq!(m.mmem[0o26], 5, "{kind}: the word after the second start reads the first's");
        assert_eq!(m.mmem[0o27], second_word, "{kind}: the word after it the second's");
        if kind == "a register" {
            assert_ne!(second_word, 5, "{kind}: a word of its own");
        }
        assert_eq!(
            e.meters.consecutive_starts[0][0], 1,
            "{kind}: one read right after a read counted"
        );
        let u = micro(&p);
        let x = pipeline_with(&p, |x| x.mutation = Mutation::FirstReadDropped);
        if state(x.machine()) != state(u.machine()) {
            caught += 1;
        }
    }
    assert_eq!(caught, 3, "the first word dropped is caught in every case");
}

/// **On revision 14 too, both reads land** (MP2b rulings 2, Q2: the hold is
/// QUUX's on every revision): the program of
/// [`row_a_read_start_right_after_a_read_start_both_land`] on revision 14's
/// `rtl`, the single-edge engine, and `micro`: the word after the second
/// start reads the first's word on both. A checkpoint of `micro` taken
/// between the two starts' landings carries the second word.
#[test]
fn revision_14_s_engines_land_both_reads() {
    let mut p = Prog::default();
    p.main.push((0o40, 5));
    p.main.push((0o50, 7));
    let (two, a, b) = (p.k(2), p.k(PHYS | 0o40), p.k(PHYS | 0o50));
    p.op(ALU | SETA | a_src(two) | MD);
    p.op(ALU | SETA | a_src(a) | START_READ);
    p.op(ALU | SETA | a_src(b) | START_READ);
    p.op(ALU | SETM | SRC_MD | m_dest(0o26));
    p.op(ALU | SETM | SRC_MD | m_dest(0o27));
    p.fill(2);
    p.stop();
    let g = Geometry::QUUX_14;
    let u = match run_engine(Micro::new(machine(&p, g))) {
        Ok(u) => u,
        Err((h, _)) => panic!("micro halted: {h:?}"),
    };
    let r = match run_engine(muir::rtl::Rtl::new(machine(&p, g))) {
        Ok(r) => r,
        Err((h, _)) => panic!("rtl halted: {h:?}"),
    };
    for (what, m) in [("micro", u.machine()), ("rtl", r.machine())] {
        assert_eq!((m.mmem[0o26], m.mmem[0o27]), (5, 7), "{what}");
    }
    // micro's checkpoint at every microcycle of the run, resumed: the end.
    for k in 1..40 {
        let mut v = Micro::new(machine(&p, g));
        v.boot();
        for _ in 0..k {
            v.step().unwrap();
        }
        let mut w = muir::checkpoint::Writer::new();
        v.save(&mut w);
        let bytes = w.finish();
        let mut x = Micro::new(machine(&p, g));
        x.load(&mut muir::checkpoint::Reader::for_word_bits(&bytes, 40)).unwrap();
        while x.machine().opc != STOP as u16 {
            x.step().unwrap();
        }
        assert_eq!((x.machine().mmem[0o26], x.machine().mmem[0o27]), (5, 7), "resumed after {k}");
    }
}

/// **Under neutral time a device sees a register write at its start's
/// instant** (MP2b rulings 1, Q12(c); rulings 2, Q1), however late the
/// write goes out: timer 0, one-shot, a microsecond, its interrupt enabled,
/// then a run of checks of condition 5; the check that first calls, named
/// by its return address in M 26, is the same on both engines.
#[test]
fn under_neutral_time_a_device_sees_a_write_at_its_start_s_instant() {
    use muir::isa::asm::src;
    let mut p = Prog::default();
    enable_interrupts(&mut p);
    p.write(1, REGISTER_PAGE | 0o111);
    p.write(1 | 4 | 1 << 8, REGISTER_PAGE | 0o110);
    for _ in 0..160 {
        p.op(jcond(5) | P | N | target(SUBS[0]));
    }
    p.stop();
    while p.at() < SUBS[0] {
        p.fill(1);
    }
    let off = p.k(0);
    p.op(ALU | SETM | src(0o1) | m_dest(0o26));
    p.op(ALU | SETA | a_src(off) | fd(0o2));
    p.op(filler().raw() | POPJ);
    p.fill(1);
    let u = {
        let mut u = Micro::new(machine(&p, REV16));
        u.neutral = true;
        match run_engine(u) {
            Ok(u) => u,
            Err((h, _)) => panic!("micro halted: {h:?}"),
        }
    };
    let e = ends_on(machine(&p, REV16), &|e| e.neutral = true);
    assert_ne!(u.machine().mmem[0o26], 0, "a check called");
    assert_eq!(e.machine().mmem[0o26], u.machine().mmem[0o26], "the same check");
    assert_eq!(state(e.machine()), state(u.machine()), "the ends");
}

/// **The harness acts between two microcycles on both engines** (MP2b
/// rulings 1, Q12(b)): a program writes its round's count to the frame
/// buffer's first word, as a listener echoes to the screen, and reads the
/// keyboard's status, register-page word 120, until a key is waiting. The
/// harness looks at the screen at the action point its schedule gives, `n`
/// microcycles in, for each `n` over a span of the loop's, and presses a
/// key there. The pipeline halts after that microcycle and drains, its
/// posted writes landed, so that the screen and the count are `micro`'s at
/// every `n`; looking at the pipeline as it stands, its writes still in the
/// queue, is caught.
#[test]
fn the_harness_acts_between_two_microcycles_on_both_engines() {
    use muir::isa::asm::ADD;
    use muir::quux_input::KeyboardMouse;
    use support::neutral::{restart_schedule, run_for_under};
    let mut p = Prog::default();
    // The keyboard enabled: word 120 <8>.
    p.write(1 << 8, REGISTER_PAGE | 0o120);
    let (status, screen) = (p.k(REGISTER_PAGE | 0o120), p.k(Word::from(muir::tlb::DEVICE_WINDOW)));
    let round = p.at();
    p.op(ALU | ADD | a_src(ONE) | m_src(0o27) | m_dest(0o27));
    p.op(ALU | SETM | m_src(0o27) | MD);
    p.op(ALU | SETA | a_src(screen) | START_WRITE);
    p.fill(1);
    p.op(ALU | SETA | a_src(status) | START_READ);
    p.fill(1);
    p.op(ALU | SETM | SRC_MD | m_dest(0o26));
    // Bit 0 of M 26 clear: round again.
    p.op(JUMP | m_src(0o26) | 1 << 6 | target(round) | N);
    p.fill(1);
    p.stop();
    let video = |p: &Prog| {
        let mut m = machine(p, REV16);
        m.tv.set_board(muir::tv::Board::Video);
        m
    };
    fn run<E: Engine>(mut e: E, n: u64, act: bool) -> (u32, u64) {
        restart_schedule();
        e.boot();
        let mut step = |e: &mut E| e.step().unwrap();
        if act {
            run_for_under(true, &mut e, n, &mut step);
        } else {
            for _ in 0..n {
                step(&mut e);
            }
        }
        let seen = e.machine().tv.read_buffer(0);
        e.machine_mut().quux_input.press(0o123);
        for _ in 0..20_000 {
            if e.machine().opc == STOP as u16 {
                break;
            }
            step(&mut e);
        }
        (seen, e.machine().mmem[0o27])
    }
    let mut caught = 0;
    for n in 200..240 {
        let mut u = Micro::new(video(&p));
        u.neutral = true;
        let want = run(u, n, true);
        assert_ne!(want.0, 0, "n {n}: the screen written");
        let mut x = Pipeline::new(video(&p));
        x.neutral = true;
        let got = run(x, n, true);
        assert_eq!(got, want, "n {n}: the screen seen and the rounds counted");
        let mut x = Pipeline::new(video(&p));
        x.neutral = true;
        if run(x, n, false) != want {
            caught += 1;
        }
    }
    eprintln!("looking at the pipeline as it stands: {caught} of 40 action points see otherwise");
    assert!(caught > 0, "acting without the halt is caught");
}

// --- The console on revision 15 (A15b.13; MP4 rulings Q3, Q4) ------------------

/// Two jumps in a loop, each with the hint, `IR<48>`: every word to run
/// has `IR<63:48>` 1.
fn hinted_loop() -> Prog {
    use muir::isa::asm::HINT;
    let mut p = Prog::default();
    p.op(JUMP | target(1) | ALWAYS | N | HINT);
    p.op(JUMP | target(0) | ALWAYS | N | HINT);
    p
}

/// `micro` on `p` at `geometry`, run a while and halted by the clock
/// control register.
fn micro_halted(p: &Prog, geometry: Geometry) -> Micro {
    let mut u = Micro::new(machine(p, geometry));
    u.boot();
    for _ in 0..20 {
        u.step().unwrap();
    }
    u.spy_write(muir::spy::CLK, 0);
    u.step().unwrap();
    u
}

/// The pipeline on `p`, run a while and halted by the clock control
/// register, drained.
fn pipeline_halted(p: &Prog) -> Pipeline {
    pipeline_halted_on(p, REV16)
}

/// [`pipeline_halted`] on `geometry`.
fn pipeline_halted_on(p: &Prog, geometry: Geometry) -> Pipeline {
    let mut e = Pipeline::new(machine(p, geometry));
    e.boot();
    e.skip_sweep();
    for _ in 0..40 {
        e.tick().unwrap();
    }
    e.spy_write(muir::spy::CLK, 0);
    while !e.is_halted() {
        e.tick().unwrap();
    }
    e
}

/// **Spy register 3 reads `IR<63:48>` and write strobe 6, with its alias
/// 14, loads the debug IR's `<63:48>`, on revision 15 only** (MP4 ruling
/// Q4: `SPY-IR-EXT` and `-LDDBIRX`). Strobe 7 and 15 load
/// nothing; on revision 14 and the CADR, 6 and 14 load nothing and
/// register 3 is open. Read on `micro` and on the pipeline, halted at a
/// word whose extension is 1.
#[test]
fn spy_register_3_and_write_strobe_6_are_ir_s_extension_on_revision_15() {
    use muir::spy;
    for (what, mut m, connected) in [
        ("revision 15", Machine::with_geometry(REV16, 1), true),
        ("revision 14", Machine::with_geometry(Geometry::QUUX_14, 1), false),
        ("the CADR", Machine::new(), false),
    ] {
        m.debug_ir = 0o1234;
        m.spy_write(spy::LDDBIRX, 0xabcd);
        let want = if connected { 0xabcd << 48 | 0o1234 } else { 0o1234 };
        assert_eq!(m.debug_ir, want, "{what}: strobe 6");
        m.spy_write(14, 0x1234);
        let want = if connected { 0x1234 << 48 | 0o1234 } else { 0o1234 };
        assert_eq!(m.debug_ir, want, "{what}: strobe 14, 6's alias");
        m.spy_write(7, 0xffff);
        m.spy_write(15, 0xffff);
        assert_eq!(m.debug_ir, want, "{what}: 7 and 15 load nothing");
    }
    let p = hinted_loop();
    let u = micro_halted(&p, REV16);
    assert_eq!(u.spy_read(spy::IR_EXT), 1, "micro: IR<63:48>");
    let e = pipeline_halted(&p);
    assert_eq!(e.spy_read(spy::IR_EXT), 1, "the pipeline: IR<63:48>");
    assert_eq!(e.spy_read(spy::IR_HIGH), u.spy_read(spy::IR_HIGH));
    let u = micro_halted(&p, Geometry::QUUX_14);
    assert_eq!(u.spy_read(spy::IR_EXT), spy::OPEN_READ, "revision 14: open");
}

/// The debug IR loaded through the spy as a console writes it, its four
/// halves (MP4 ruling Q4), then one step with `IDEBUG` up, as CC's
/// `CC-EXECUTE` makes it.
fn execute_debug_ir(e: &mut dyn Engine, ir: u64, tick: &mut dyn FnMut(&mut dyn Engine) -> bool) {
    use muir::spy;
    for (eadr, half) in [(spy::IR_LOW, 0), (spy::IR_MED, 1), (spy::IR_HIGH, 2), (spy::LDDBIRX, 3)] {
        e.spy_write(eadr, (ir >> (16 * half)) as u16);
    }
    e.spy_write(spy::CLK, 0o12);
    let _ = tick(e);
    e.spy_write(spy::CLK, 0o10);
    for _ in 0..10_000 {
        if tick(e) {
            break;
        }
    }
    e.spy_write(spy::CLK, 0);
}

/// **The pipeline runs the debug IR** (A15b.13: "the debug IR's word runs
/// as a single step does"; MP4 ruling Q4), all 64 bits: halted among
/// fillers, the console runs two words through the debug IR, one step
/// each with `IDEBUG` up: a write of OA-REG-HIGH, then an ADD whose SH,
/// `IR<61>`, ORs it into its A and M sources, A 1700 and M 20 becoming A
/// 1703 and M 24. Each runs in place of the word at PC, as on `micro`: a
/// microcycle each, M 25 the sum of A 1703 and M 24, the same state, and
/// the next word to run two after PC. A pipeline that runs the control
/// store's word, or a debug IR of 48 bits, is caught.
#[test]
fn the_pipeline_runs_the_debug_ir_s_64_bits_as_micro_does() {
    use muir::isa::asm::{ADD, OA_HIGH_SELECT};
    let mut p = Prog::default();
    p.set(0o22, 0o24);
    p.amem.push((0o1703, 0o33));
    let v = p.k(3 << 6 | 4);
    let words = [
        ALU | SETA | a_src(v) | fd(0o17),
        ALU | ADD | a_src(0o1700) | m_src(0o20) | m_dest(0o25) | OA_HIGH_SELECT,
    ];
    let mut u = micro_halted(&p, REV16);
    let (pc, cycles) = (u.pc(), u.machine().cycles);
    for ir in words {
        execute_debug_ir(&mut u, ir, &mut |e| {
            e.step().unwrap();
            false
        });
    }
    assert_eq!(u.machine().cycles, cycles + 2, "micro: a microcycle each");
    assert_eq!(u.pc(), pc + 2, "micro: the words at PC replaced");
    let mut e = pipeline_halted(&p);
    let (pc, cycles) = (e.pc(), e.machine().cycles);
    for ir in words {
        execute_debug_ir(&mut e, ir, &mut |e| {
            e.step().unwrap();
            e.spy_read(muir::spy::FLAG_1) & 0x100 == 0
        });
    }
    let (um, em) = (u.landed(), e.landed());
    assert_eq!(um.mmem[0o25], 0o55, "micro: A 1703 plus M 24");
    assert_eq!(em.mmem[0o25], 0o55, "the pipeline: A 1703 plus M 24");
    assert_eq!(em.cycles, cycles + 2, "the pipeline: a microcycle each");
    assert_eq!(e.pc(), pc + 2, "the pipeline: the words at PC replaced");
    diff_state(&em, &um, 0);
}

/// **A revision-15 checkpoint keeps no pending OA flag** (A15b.13: "no
/// pending OA flag"; MP4 ruling Q3), and is version 56; one of version 55,
/// written with revision 14's two IMOD flags after the OA registers, still
/// loads. Halted among fillers, the pipeline's checkpoint and the same with
/// two zero flags put back resume alike, run on alike, and the second
/// written again is the first: 45 bytes follow the flags when no SPC or map
/// write is pending.
#[test]
fn a_revision_15_checkpoint_keeps_no_imod_flag_and_a_version_55_one_loads() {
    use muir::checkpoint::{Checkpoint, VERSION_15, VERSION_40, Writer, version_for};
    const REV15: Geometry = Geometry::QUUX_15;
    assert_eq!(version_for(&REV15), VERSION_15);
    assert_eq!(VERSION_15, 56);
    assert_eq!(version_for(&Geometry::QUUX_14), VERSION_40);
    let p = Prog::default();
    let e = pipeline_halted_on(&p, REV15);
    let saved = |e: &Pipeline| {
        let mut w = Writer::new();
        e.save(&mut w);
        w.finish()
    };
    let body = saved(&e);
    let tail = body.len() - 45;
    let mut old = body[..tail].to_vec();
    old.extend([0, 0]);
    old.extend(&body[tail..]);
    let resume = |bytes: &[u8], version: u32| {
        let c = Checkpoint {
            version,
            word_bits: 40,
            engine: "rtl".into(),
            memory_boards: 1,
            body: bytes.to_vec(),
        };
        let mut f = Pipeline::new(machine(&p, REV15));
        f.load(&mut c.reader()).expect("it loads");
        f
    };
    let (mut a, mut b) = (resume(&body, VERSION_15), resume(&old, VERSION_40));
    assert_eq!(saved(&b), body, "version 55's, written again, is version 56's");
    for f in [&mut a, &mut b] {
        f.skip_sweep();
        f.spy_write(muir::spy::CLK, 1);
        for _ in 0..200 {
            f.tick().unwrap();
        }
    }
    assert_eq!((a.pc(), a.machine().cycles), (b.pc(), b.machine().cycles));
    diff_state(a.machine(), b.machine(), 0);
}

/// **A dispatch-memory write loads the dispatch constant** on revision 15,
/// as on the CADR's page DSPCTL (`tests/dispatch_write_order.rs`): the
/// word after it reads functional source 0, `DISPATCH-CONSTANT`, as the
/// write's `IR<41:32>`, on `micro` and the pipeline alike. **Fails** an
/// engine that loads the constant only on a dispatch that dispatches.
#[test]
fn a_dispatch_memory_write_loads_the_dispatch_constant_on_revision_15() {
    use muir::isa::asm::DMEM_WRITE;
    let mut p = Prog::default();
    p.op(DISPATCH | DMEM_WRITE | a_src(0o525) | 0o40 << 12);
    p.op(ALU | SETM | src(0) | m_dest(0o26));
    p.stop();
    let e = same(&p);
    assert_eq!(e.machine().mmem[0o26], 0o525, "the write's IR<41:32>");
    assert_eq!(micro(&p).machine().mmem[0o26], 0o525, "micro");
}

// --- The MP4 timing review's model changes (P1-P3, two bubbles) ----------------

/// The pipeline on `p` with `set` applied, its events recorded, run to the
/// stop and settled; and the clock each address first commits at.
fn commits(p: &Prog, set: &dyn Fn(&mut Pipeline)) -> (Pipeline, Vec<(u64, muir::pipeline::Event)>) {
    let mut e = Pipeline::new(machine(p, REV16));
    set(&mut e);
    e.events = Some(Vec::new());
    let e = match run_engine(e).and_then(settled) {
        Ok(e) => e,
        Err((h, _)) => panic!("the pipeline halted: {h:?}"),
    };
    let mut e = e;
    let ev = e.events.take().unwrap();
    (e, ev)
}

fn commit_at(ev: &[(u64, muir::pipeline::Event)], pc: u64) -> u64 {
    ev.iter()
        .find(|(_, x)| *x == muir::pipeline::Event::Commit(Some(pc as u16)))
        .map(|e| e.0)
        .unwrap_or_else(|| panic!("{pc:o} never committed"))
}

/// **P1: port B uses its walk's fill a clock later** (the MP4 timing review
/// §4; the MP4 rulings' Q2 fallback). `MAP(MD)` of a page not in the TLB,
/// its directory and page entries already in the cache: the word commits a
/// clock later than with the fill used in the clock it lands, the word
/// before it at the same clock, and the run ends as on `micro`. The fill
/// used at once is caught by the clock.
#[test]
fn p1_port_b_uses_its_walk_s_fill_a_clock_later() {
    let x: Word = 0o4000;
    let mut p = Prog::default();
    walkable(&mut p, x, 3);
    p.main.push(((3 << 10), 0o777));
    set_directory(&mut p);
    // The tables' lines into the cache.
    p.read(PHYS | (8 << 10), 0o31);
    p.read(PHYS | ((9 << 10) + 2), 0o31);
    let xa = p.k(x);
    p.op(ALU | SETA | a_src(xa) | MD);
    p.fill(2);
    let before = p.at();
    p.fill(1);
    let map = p.at();
    p.op(ALU | SETM | src(0o11) | m_dest(0o25));
    p.stop();
    same(&p);
    let (e, ev) = commits(&p, &|_| {});
    let (o, ov) = commits(&p, &|e| e.mutation = Mutation::PortBFillSameClock);
    assert_eq!(e.meters.p1_holds, 1, "one walk on port B");
    assert_eq!(commit_at(&ev, before), commit_at(&ov, before), "the word before at the same clock");
    assert_eq!(commit_at(&ev, map), commit_at(&ov, map) + 1, "the map word a clock later");
    assert_eq!(o.meters.p1_holds, 0);
}

/// A conditional jump hinted taken and not taken, its slot and the word
/// after it each counting in M 26-30, so that a squashed word shows.
fn mispredicted_jump(n: u64) -> (Prog, u64, u64, u64) {
    use muir::isa::asm::{ADD, HINT};
    let mut p = Prog::default();
    let inc = |m: u64| ALU | ADD | a_src(ONE) | m_src(m) | m_dest(m);
    p.fill(3);
    let jump = p.at();
    let t = jump + 0o10;
    // Hinted taken; M 1 is not A 0, so not taken.
    p.op(jcond(3) | m_src(M_ONE) | a_src(ZERO) | target(t) | HINT | n);
    let slot = p.at();
    p.op(inc(0o26));
    let after = p.at();
    p.op(inc(0o27));
    p.fill(2);
    p.stop();
    while p.at() < t {
        p.fill(1);
    }
    p.op(inc(0o30));
    p.stop();
    (p, jump, slot, after)
}

/// **P2: after a wrong prediction the delay slot waits a clock in RD** (the
/// MP4 timing review §4): a jump hinted taken and not taken, N clear: the
/// slot commits a clock later than when RD plans it in the redirect's
/// clock, and the word after it at the same clock: the bubble moves ahead
/// of the slot and none is added. The run ends as on `micro`. The slot
/// planned at once is caught by the clock.
#[test]
fn p2_the_slot_waits_a_clock_in_rd_after_a_wrong_prediction() {
    let (p, jump, slot, after) = mispredicted_jump(0);
    let e = same(&p);
    assert_eq!(e.machine().mmem[0o26..=0o30], [1, 1, 0], "the slot and the word after ran");
    assert_eq!(e.meters.mispredicted[0], 1);
    let (e, ev) = commits(&p, &|_| {});
    let (o, ov) = commits(&p, &|e| e.mutation = Mutation::SlotPlansAtRedirect);
    assert_eq!(e.meters.p2_holds, 1);
    assert_eq!(o.meters.p2_holds, 0);
    assert_eq!(commit_at(&ev, jump), commit_at(&ov, jump));
    assert_eq!(commit_at(&ev, slot), commit_at(&ev, jump) + 2, "a clock in RD, then EX");
    assert_eq!(commit_at(&ov, slot), commit_at(&ov, jump) + 1, "planted: at once");
    assert_eq!(commit_at(&ev, after), commit_at(&ov, after), "the word after at the same clock");
}

/// **Two bubbles, A15b.14's fallback** (`Pipeline::bubbles`): the same
/// wrong prediction fetches the word after the slot a clock later, and the
/// slot does not wait in RD (P2 is moot); with N set too. The speculation
/// matrix and the random programs end as on `micro` with two bubbles.
#[test]
fn two_bubbles_fetch_the_target_a_clock_later() {
    for n in [0, N] {
        let (p, jump, slot, after) = mispredicted_jump(n);
        let one = commits(&p, &|_| {});
        let two = commits(&p, &|e| e.bubbles = 2);
        let (e1, ev1) = (&one.0, &one.1);
        let (e2, ev2) = (&two.0, &two.1);
        assert_eq!(state(e2.machine()), state(e1.machine()), "N {n}: the same end");
        assert_eq!(e2.meters.second_bubbles, 1, "N {n}");
        assert_eq!(e2.meters.p2_holds, 0, "N {n}: no wait in RD");
        assert_eq!(commit_at(ev2, jump), commit_at(ev1, jump));
        if n == 0 {
            assert_eq!(commit_at(ev2, slot), commit_at(ev2, jump) + 1, "the slot at once");
        }
        assert_eq!(commit_at(ev2, after), commit_at(ev1, after) + 1, "N {n}: a clock later");
    }
    for (what, p) in &matrix() {
        let u = micro_unchecked(p);
        let mut e = Pipeline::new(machine(p, REV16));
        e.bubbles = 2;
        let e = match run_engine(e).and_then(settled) {
            Ok(e) => e,
            Err((h, _)) => panic!("{what}: halted {h:?}"),
        };
        if state(e.machine()) != state(u.machine()) {
            diff_state(e.machine(), u.machine(), 0);
        }
    }
    for seed in 1..=300 {
        let p = random_program(seed, ALL);
        let u = micro(&p);
        let e = pipeline_with(&p, |e| e.bubbles = 2);
        if state(e.machine()) != state(u.machine()) {
            diff_state(e.machine(), u.machine(), seed);
        }
    }
}

/// **P3: writes are answered in order, at most one a clock** (one AXI ID
/// for every write: A15b.5; the MP4 timing review §6), under the model that
/// answers each write 1 to 50 clocks late: over forty seeds, the in-flight
/// list never loses two writes in one clock, and each lands no earlier than
/// its model's draw. Writes answered as drawn, out of order, are caught: some
/// clock lands two.
#[test]
fn p3_writes_are_answered_in_order_one_a_clock() {
    let run = |seed: u64, in_order: bool| {
        let mut m = machine(&Prog::default(), REV16);
        let mut port = port(130);
        port.model = Some(late(seed));
        // The default answers in order; the planted run turns it off.
        if !in_order {
            port.writes_in_order = false;
        }
        let mut most = 0;
        let mut now = 0;
        let mut k = 0u32;
        let mut before = port.depths();
        while k < 24 || port.depths() != (0, 0) {
            // A write a clock while the queue has room.
            if k < 24 && port.write(now, 0o100 + 8 * k, k.into()) {
                k += 1;
                before.0 += 1;
            }
            now += 1;
            port.tick(&mut m, now);
            let after = port.depths();
            // Accepts add to the list; responses take from it.
            let accepted = before.0 - after.0;
            let answered = before.1 + accepted - after.1;
            most = most.max(answered);
            before = after;
            assert!(now < 10_000);
        }
        most
    };
    let mut caught = 0;
    for seed in 1..=40 {
        assert!(run(seed, true) <= 1, "seed {seed}: two answers in one clock");
        if run(seed, false) > 1 {
            caught += 1;
        }
    }
    eprintln!("writes answered as drawn: two in a clock at {caught} seeds of 40");
    assert!(caught > 0, "answers out of order are caught");
}

/// **A frame-buffer word written while its line is cached reads back as the
/// frame buffer holds it** (G1 §4.2: the window stores the field and drops
/// the tag; a read comes back tagged 005): a word of the window read, so
/// that its line is in the cache, then written with a list pointer's tag,
/// then read again, a hit. On `micro` and the pipeline alike, the field
/// with tag 005.
#[test]
fn row_a_cached_frame_buffer_word_written_reads_back_as_the_window_holds_it() {
    let window = muir::tlb::DEVICE_WINDOW as Word;
    let list: Word = 0o016 << 32;
    let mut p = Prog::default();
    p.read(window | 0o12, 0o25);
    p.write(list | 0o1234, window | 0o12);
    p.read(window | 0o12, 0o26);
    p.read(window | 0o13, 0o27);
    p.stop();
    let u = micro(&p);
    assert_eq!(u.machine().mmem[0o26], muir::machine::UNBOXED_TAG | 0o1234, "micro");
    let e = pipeline(&p);
    assert_eq!(e.machine().mmem[0o26], muir::machine::UNBOXED_TAG | 0o1234, "the pipeline's hit");
    same(&p);
}

/// **A data push keeps the stack's 19 bits** (the CADR's `SPCW<18:0>` into
/// the 82S21s on page SPC, `SPCO<18:0>` out; revision 15 keeps the width):
/// a word with bits above `<18>` pushed by destination 15, then read back by
/// source 1, which reads the pointer and `SPC<18:0>`. The stack holds the
/// word's 19 bits on `micro` and the pipeline, and so does a checkpoint of
/// each, loaded again. It held all 32, a field the read never returns.
#[test]
fn a_data_push_keeps_the_stack_s_19_bits() {
    let mut p = Prog::default();
    let w: Word = 0o37766554321;
    let k = p.k(w);
    p.op(ALU | SETA | a_src(k) | fd(0o15));
    p.fill(2);
    p.op(ALU | SETM | src(0o1) | m_dest(0o26));
    p.stop();
    let low = w & 0o1777777;
    let u = micro(&p);
    let mut e = pipeline(&p);
    e.halt_between_microcycles().unwrap();
    let body = |save: &dyn Fn(&mut muir::checkpoint::Writer)| {
        let mut wr = muir::checkpoint::Writer::new();
        save(&mut wr);
        wr.finish()
    };
    let (ub, eb) = (body(&|wr| u.save(wr)), body(&|wr| e.save(wr)));
    let mut v = Micro::new(machine(&p, REV16));
    v.load(&mut muir::checkpoint::Reader::for_word_bits(&ub, 40)).unwrap();
    let mut f = Pipeline::new(machine(&p, REV16));
    f.load(&mut muir::checkpoint::Reader::for_word_bits(&eb, 40)).unwrap();
    for (what, m) in [
        ("micro", u.machine()),
        ("the pipeline", e.machine()),
        ("micro's checkpoint", v.machine()),
        ("the pipeline's checkpoint", f.machine()),
    ] {
        let ptr = m.spcptr as usize;
        assert_eq!(m.mmem[0o26] & 0o1777777, low, "{what}: the read");
        assert_eq!(u64::from(m.spc[ptr]), low, "{what}: the stack's word");
    }
}

// --- The profile (A15b.8) ------------------------------------------------------

/// **The profile's lines for a program whose branches are known** (A15b.8;
/// `muir::pipeline::profile`, its format in `docs/quux.md`): on the
/// pipeline,
/// - a loop's conditional jump back at LOOP+1, three times, transferring
///   twice: hint 1;
/// - a conditional jump at TWO+10 that never transfers: hint 0;
/// - a dispatch at DLOOP, three times, calling SUB twice and jumping to OUT
///   once: a call of SUB, keyed AAA (below);
/// - a dispatch at SUB, also labelled AAA, whose entry returns, twice: a
///   return, keyed by the alphabetically first of the two labels.
///
/// The unconditional jumps and the stop are no sites. The coverage counts
/// the four sites, all executed, and the executions the lines predict.
#[test]
fn the_profile_of_a_program_whose_branches_are_known() {
    use muir::isa::asm::INVERT;
    use muir::pipeline::profile;
    const M_MINUS_1: u64 = 0o23 << 3;
    const SUB: u64 = 0o600;
    let mut p = Prog::default();
    p.set(3, 0o20);
    let lp = p.at();
    p.op(ALU | M_MINUS_1 | m_src(0o20) | m_dest(0o20));
    p.op(jcond(3) | INVERT | m_src(0o20) | a_src(ZERO) | target(lp) | N);
    p.fill(1);
    // TWO labels the word eight before the jump: the offset is octal.
    let two = p.at();
    p.fill(8);
    p.op(jcond(3) | m_src(M_ONE) | a_src(ZERO) | target(STOP) | N);
    p.fill(1);
    p.set(2, 0o22);
    let dloop = p.at();
    // M 22's low two bits: 2 and 1 call SUB, 0 jumps to OUT.
    p.op(disp(0o100) | 2 << 5 | m_src(0o22));
    p.op(ALU | M_MINUS_1 | m_src(0o22) | m_dest(0o22));
    p.op(JUMP | ALWAYS | N | target(dloop));
    p.fill(1);
    let out = p.at();
    p.stop();
    p.dmem.push((0o102, SUB as u32 | 1 << 15));
    p.dmem.push((0o101, SUB as u32 | 1 << 15));
    p.dmem.push((0o100, out as u32));
    p.dmem.push((0o104, 1 << 16));
    while p.at() < SUB {
        p.fill(1);
    }
    p.op(disp(0o104));
    p.fill(1);
    let e = pipeline_with(&p, |e| e.outcomes = Some(Default::default()));
    let o = e.outcomes.clone().expect("the record");
    let syms = muir::sym::parse(&format!(
        "-2 START I-MEM 0\nLOOP I-MEM {lp:o}\nTWO I-MEM {two:o}\nDLOOP I-MEM {dloop:o}\n\
         OUT I-MEM {out:o}\nSUB I-MEM {SUB:o}\nAAA I-MEM {SUB:o}\n-1\n"
    ))
    .unwrap();
    let (lines, unkeyed) = profile::lines(&o, &syms);
    assert!(unkeyed.is_empty(), "every site has a label at or below it");
    assert_eq!(
        lines,
        [
            "jump LOOP+1 1 3",
            "jump TWO+10 0 1",
            "dispatch DLOOP+0 call AAA+0 3",
            "dispatch AAA+0 return - 2",
        ]
    );
    let c = profile::coverage(&o, &e.machine().imem);
    assert_eq!((c.jump_sites, c.jump_sites_executed), (2, 2), "{c:?}");
    assert_eq!((c.jump_executions, c.jumps_predicted), (4, 3), "{c:?}");
    assert_eq!((c.dispatch_sites, c.dispatch_sites_executed), (2, 2), "{c:?}");
    assert_eq!((c.dispatch_executions, c.dispatches_predicted), (5, 4), "{c:?}");
    // The program ran as micro runs it.
    let u = micro(&p);
    assert_eq!(state(e.machine()), state(u.machine()));
}

/// **A transferring dispatch takes no OA select** (A15b.2): its `<61:60>`
/// are its predicted address's `<13:12>`. A dispatch predicting a jump to
/// 10000, 20000 or 37777 octal, wrong each time, after OA-REG-HIGH was
/// written and spent by an SH word, runs on `micro` with the OA select
/// check on, and the pipeline ends as `micro` does: neither reads the bits
/// as SL or SH, which would halt `micro` at OA-SELECT-WITHOUT-WRITE and OR
/// OA-REG-HIGH into the pipeline's M source, a dispatch on another word. A
/// dispatch-memory write's SL still selects.
#[test]
fn a_dispatch_predicting_an_address_above_7777_takes_no_select() {
    use muir::isa::asm::{ADD, OA_HIGH_SELECT, OA_LOW_SELECT, predicted};
    for addr in [0o10000u64, 0o20000, 0o37777] {
        let mut p = Prog::default();
        p.set(1, 0o20);
        // OA-REG-HIGH 3: M source 3 more, were it ORed into a word.
        let three = p.k(3);
        p.op(ALU | SETA | a_src(three) | fd(0o17));
        p.op(ALU | ADD | a_src(ZERO) | m_src(0o20) | m_dest(0o25) | OA_HIGH_SELECT);
        p.fill(3);
        // M 20's low two bits: 1 jumps to ONE_AT, 3 to THREE_AT.
        p.op(disp(0o100) | 2 << 5 | m_src(0o20) | predicted(addr, false, false));
        p.fill(1);
        let back = p.at();
        p.stop();
        let (one_at, three_at) = (0o600u64, 0o640u64);
        p.dmem.push((0o101, one_at as u32));
        p.dmem.push((0o103, three_at as u32));
        while p.at() < one_at {
            p.fill(1);
        }
        p.set(0o111, 0o26);
        p.op(JUMP | ALWAYS | N | target(back));
        p.fill(1);
        while p.at() < three_at {
            p.fill(1);
        }
        p.set(0o333, 0o26);
        p.op(JUMP | ALWAYS | N | target(back));
        p.fill(1);
        let e = same(&p);
        assert_eq!(e.machine().mmem[0o26], 0o111, "predicted {addr:o}: the entry M 20 names");
    }
    // A dispatch-memory write with SL still takes OA-REG-LOW into its
    // address: entry 101 | 6 = 107 written.
    let mut p = Prog::default();
    let (six, word) = (p.k(6 << 12), p.k(0o4321));
    p.op(ALU | SETA | a_src(six) | fd(0o16));
    p.op(disp(0o101) | 2 << 10 | a_src(word) | OA_LOW_SELECT);
    p.fill(1);
    p.stop();
    let e = same(&p);
    assert_eq!(e.machine().dmem[0o107], 0o4321, "the dispatch-memory write's SL");
}

/// **A PDL address field reads M-AP and A-LOCALP as the word finds them**
/// (A15b.2), the word right before it having written them: the register's
/// M-AP is M 0 and its A-LOCALP A 0 after the boot, written 40 and 60 and
/// read at once, and a word later, by a field whose index is 40 + 5 and
/// 60 − 3, and the PDL buffer read through the index in the next word. The
/// check holds the formed index to the index written on both engines; one
/// formed from the base copies before the word in WB lands halts the
/// pipeline at PDL-FIELD-MISMATCH where `micro` runs on.
#[test]
fn a_pdl_field_reads_a_base_the_word_before_wrote() {
    use muir::isa::asm::pdl_field;
    for gap in [0, 1] {
        let mut p = Prog::default();
        let (k40, k60, k45, k55) = (p.k(0o40), p.k(0o60), p.k(0o45), p.k(0o55));
        // The PDL buffer's words 45 and 55, to read through the index.
        for (at, v) in [(k45, 0o4545), (k55, 0o5555)] {
            let kv = p.k(v);
            p.op(ALU | SETA | a_src(at) | fd(0o13));
            p.op(ALU | SETA | a_src(kv) | fd(0o12));
        }
        p.fill(2);
        p.op(ALU | SETA | a_src(k40) | m_dest(0));
        p.fill(gap);
        p.op(ALU | SETA | a_src(k45) | fd(0o13) | pdl_field(0, 5));
        p.op(ALU | SETM | src(0o5) | m_dest(0o26));
        p.op(ALU | SETA | a_src(k60) | a_dest(0));
        p.fill(gap);
        p.op(ALU | SETA | a_src(k55) | fd(0o13) | pdl_field(1, -3));
        p.op(ALU | SETM | src(0o5) | m_dest(0o27));
        p.fill(1);
        p.stop();
        let e = same(&p);
        let m = e.machine();
        assert_eq!(
            (m.mmem[0o26], m.mmem[0o27]),
            (0o4545, 0o5555),
            "gap {gap}: read through the index"
        );
    }
}

/// **D's switch, `MUIR_D`** (MP2b ruling Q12(d)): a write of the
/// MACRO-DISPATCH register keeps D's enable `<30>` as written; held off
/// (`MUIR_D=off`) it stays clear though the word sets it; on (`MUIR_D=on`)
/// it is set with the enable `<31>` though the word leaves it clear, and
/// not without `<31>`. On both engines.
#[test]
fn d_s_switch_keeps_or_clears_d_s_enable() {
    use muir::machine::DSwitch;
    use muir::machine::macro_dispatch::{D_ENABLE, ENABLE};
    for (written, d, want) in [
        (ENABLE | D_ENABLE, DSwitch::AsWritten, true),
        (ENABLE, DSwitch::AsWritten, false),
        (ENABLE | D_ENABLE, DSwitch::Off, false),
        (ENABLE, DSwitch::On, true),
        (0, DSwitch::On, false),
    ] {
        let mut p = Prog::default();
        let v = p.k(Word::from(written));
        p.op(ALU | SETA | a_src(v) | fd(5));
        p.fill(2);
        p.stop();
        let mut m = machine(&p, REV16);
        m.macro_dispatch.d = d;
        let u = run_engine(Micro::new(m.clone())).unwrap_or_else(|(h, _)| panic!("micro: {h:?}"));
        let e = ends_on(m, &|_| {});
        for (what, r) in [
            ("micro", u.machine().macro_dispatch.register),
            ("rtl", e.machine().macro_dispatch.register),
        ] {
            let what = format!("{what}, {written:o} written, {d:?}");
            assert_eq!(r & ENABLE, written & ENABLE, "{what}: the enable");
            assert_eq!(r & D_ENABLE != 0, want, "{what}: D's enable");
        }
    }
}

// --- D decides at its word's arrival (the D-timing ruling) ---------------------

/// The main-loop machine `m` on the pipeline, a commit's clock and address
/// each, to the stop, and the pipeline.
fn commits_to_the_stop(m: Machine, set: &dyn Fn(&mut Pipeline)) -> (Vec<(u64, u16)>, Pipeline) {
    let mut e = Pipeline::new(m);
    set(&mut e);
    e.events = Some(Vec::new());
    e.boot();
    e.skip_sweep();
    for _ in 0..20_000 {
        if e.machine().opc == OP_7 as u16 {
            break;
        }
        e.tick().unwrap();
    }
    let commits = e
        .events
        .take()
        .unwrap()
        .into_iter()
        .filter_map(|(c, ev)| match ev {
            muir::pipeline::Event::Commit(Some(pc)) => Some((c, pc)),
            _ => None,
        })
        .collect();
    (commits, e)
}

/// **D decides on its fetch's own word** (the D-timing ruling, R3, R6): a
/// store of another word into the macroinstruction word the return's fetch
/// reads --- two opcode-7 halfwords where opcode 1's were --- started by the
/// word before opcode 3's return, by the return itself, and by its delay
/// slot. With D on the run ends as with D off, on both engines: the stored
/// halfword's handler runs, the stop, and M 31 is the stored word. A D that
/// dispatched on the word before the store went its way, ran opcode 1.
#[test]
fn d_decides_on_the_fetch_s_own_word() {
    let program = [hw(1, 0, 0), hw(3, 0, 0), hw(1, 0, 0), hw(1, 0, 0), hw(7, 0, 0), hw(7, 0, 0)];
    let sevens = Word::from(hw(7, 0, 0) | hw(7, 0, 0) << 16);
    // The constants at A 60 and 61: the stored word, and its address, the
    // program's second word.
    let (word_at, addr_at) = (0o60u64, 0o61u64);
    let setup = |m: &mut Machine| {
        m.amem[word_at as usize] = sevens;
        m.amem[addr_at as usize] = CODE + 1;
    };
    for by in ["the word before", "the return", "the slot"] {
        let mut ends = Vec::new();
        for d in [false, true] {
            let body = match by {
                "the word before" => vec![
                    ALU | SETA | a_src(word_at) | MD,
                    ALU | SETA | a_src(addr_at) | START_WRITE,
                ],
                _ => vec![ALU | SETA | a_src(word_at) | MD],
            };
            let mut m = ml_machine(d, &program, &body, &setup);
            let n = body.len() as u64;
            match by {
                "the return" => {
                    m.imem[(OP_3 + n) as usize] =
                        Insn::extended(ALU | SETA | a_src(addr_at) | START_WRITE | POPJ)
                }
                "the slot" => {
                    m.imem[(OP_3 + n + 1) as usize] =
                        Insn::extended(ALU | SETA | a_src(addr_at) | START_WRITE)
                }
                _ => {}
            }
            let e = ml_same(m);
            let m = e.machine();
            assert_eq!(m.mmem[0o31], sevens, "{by}, D {d}: M 31 the stored word");
            assert_eq!(m.main[0o401], sevens, "{by}, D {d}: the store landed");
            ends.push((m.mmem[1], m.mmem[0o31], m.spcptr, m.pdl_index));
        }
        assert_eq!(ends[0], ends[1], "{by}: D on ends as D off");
    }
}

/// **D's effects wait for its word** (the D-timing ruling, R3), on the
/// pipeline clock by clock, on the machines whose returns D takes, opcode
/// 10's operand address among them: from the return's commit, where D's wait
/// begins, to its word's arrival, PDL-INDEX, M 31 and the micro stack's
/// pointer keep what the return left; at the arrival all three take D's.
#[test]
fn d_s_effects_wait_for_its_word() {
    let programs: [&[u32]; 2] =
        [&ONES, &[hw(0o10, 5, 5), hw(0o10, 6, 3), hw(1, 0, 0), hw(0o10, 5, 1), hw(7, 0, 0)]];
    for program in programs {
        let mut e = Pipeline::new(d_machine(REV16, d_register(true), program, CODE, false, false));
        e.boot();
        e.skip_sweep();
        let regs = |e: &Pipeline| {
            let m = e.machine();
            (m.pdl_index, m.mmem[0o31], m.spcptr)
        };
        let mut waits = 0;
        let mut was: Option<(u16, Word, u8)> = None;
        for _ in 0..20_000 {
            if e.machine().opc == OP_7 as u16 {
                break;
            }
            e.tick().unwrap();
            match (was, e.d_waiting()) {
                (None, true) => was = Some(regs(&e)),
                (Some(w), true) => assert_eq!(regs(&e), w, "held from the return to the word"),
                (Some(w), false) => {
                    assert_ne!(regs(&e), w, "D's effects at its word's arrival");
                    waits += 1;
                    was = None;
                }
                (None, false) => {}
            }
        }
        assert!(waits > 0, "D took returns");
    }
}

/// **A faulting fetch ends D's wait** (the D-timing ruling, R4): on the
/// machine whose code is unmapped, each return D waits on goes to the main
/// loop at its fetch's WB, and `QMLP`'s condition 6 takes the fault, as on
/// `micro`. A wait the fault does not end hangs the pipeline.
#[test]
fn a_faulting_fetch_ends_d_s_wait() {
    let m = d_machine(REV16, d_register(true), &ONES, UNMAPPED, false, true);
    let e = ml_same(m.clone());
    assert_eq!(e.machine().mmem[0o10], 1, "condition 6 took the fault");
    let hung = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut e = Pipeline::new(m);
        e.mutation = Mutation::DWaitsForAFault;
        e.boot();
        e.skip_sweep();
        for _ in 0..20_000 {
            if e.machine().opc == OP_7 as u16 {
                return false;
            }
            e.step().unwrap();
        }
        true
    }));
    assert!(hung.is_err() || hung.unwrap(), "a wait the fault does not end hangs");
}

/// **D's slot does not wait in RD** (the D-timing ruling, R5): with a hit,
/// a return D takes commits, its slot after it, and the handler commits 7
/// clocks after the return, with no word behind the slot committed between;
/// a D that redirects to the main loop and restores RD's copies takes 8.
#[test]
fn d_s_slot_does_not_wait_in_rd() {
    let gaps = |mutation: Mutation| {
        let m = d_machine(REV16, d_register(true), &ONES, CODE, false, false);
        let (commits, _) = commits_to_the_stop(m, &|e| e.mutation = mutation);
        // Opcode 1's return at OP_1 + 1, its slot, then the next handler.
        commits
            .windows(3)
            .filter(|w| w[0].1 == OP_1 as u16 + 1 && w[1].1 == OP_1 as u16 + 2)
            .map(|w| (w[2].1, w[2].0 - w[0].0))
            .collect::<Vec<_>>()
    };
    let g = gaps(Mutation::None);
    eprintln!("return to the next word's commit, D on: {g:?}");
    assert!(g.iter().all(|&(pc, _)| pc == OP_1 as u16 || pc == OP_7 as u16), "the handler next");
    assert!(g.iter().any(|&(_, c)| c == 7), "7 clocks on a hit");
    let r = gaps(Mutation::DRedirects);
    eprintln!("with a redirect: {r:?}");
    assert!(r.iter().any(|&(_, c)| c == 8), "8 with a redirect");
    assert!(r.iter().all(|&(_, c)| c != 7), "and never 7");
}

/// **D's slot sees the stack popped** (the D-timing ruling, R2, R6): the
/// slot after opcode 3's return reads the micro stack's pointer, D on and
/// off, on both engines: popped, as the return left it. A keep made at the
/// return is caught.
#[test]
fn d_s_slot_sees_the_stack_popped() {
    let program = [hw(1, 0, 0), hw(3, 0, 0), hw(1, 0, 0), hw(7, 0, 0)];
    let mut read = Vec::new();
    for d in [false, true] {
        let mut m = ml_machine(d, &program, &[], &|_| {});
        // The slot: M 26 <- the stack's pointer and word.
        m.imem[(OP_3 + 1) as usize] = Insn::extended(ALU | SETM | src(0o1) | m_dest(0o26));
        let e = ml_same(m.clone());
        read.push(e.machine().mmem[0o26] >> 24 & 0o37);
        if d {
            let (_, x) = commits_to_the_stop(m, &|x| x.mutation = Mutation::DKeepAtReturn);
            assert_ne!(x.machine().mmem[0o26] >> 24 & 0o37, read[1], "a keep at the return");
        }
    }
    assert_eq!(read[0], read[1], "the pointer the slot reads, D off and on");
}

/// **D decides on its own slot's fetch, never on an earlier fetch still in
/// flight** (the D-timing ruling, R3): a return that needs a fetch while
/// the sequence break stands goes to the main loop without D, its fetch a
/// miss still filling when condition 6's call, taken with N as microcode
/// 2002's `QMLP` takes it, does what `SB-DEFER` does: clears the break,
/// writes LC back with NEEDFETCH, and returns to the main loop again, where
/// D waits. The earlier fill lands during that wait. D decides on the word
/// its slot's fetch reads, LC stepped, as on `micro`: opcode 1, the
/// halfword at LC + 2, runs once, then the stop. A D that decided on the
/// earlier fill, LC not stepped, ran the halfword at LC, opcode 7's stop,
/// at once.
#[test]
fn d_decides_on_its_slot_s_fetch_not_an_earlier_one() {
    let program = [hw(1, 0, 0), hw(7, 0, 0), hw(1, 0, 0), hw(7, 0, 0)];
    let mut m = d_machine(REV16, d_register(true), &program, CODE, true, false);
    use muir::isa::asm::{CARRY_IN, M_PLUS_C};
    let mut put = |at: u64, w: u64| m.imem[at as usize] = Insn::extended(w);
    // The call with N, as microcode 2002's `QMLP` makes it: its slot, which
    // waits for MD, does not run when the call is taken.
    put(QMLP, JUMP | target(COND_6) | P | N | 1 << 5 | 6);
    put(COND_6, ALU | M_PLUS_C | CARRY_IN | m_src(0o10) | m_dest(0o10));
    // The break cleared, the call's return popped and the main loop's
    // pushed, LC written back to the word's first halfword, NEEDFETCH set.
    put(COND_6 + 1, ALU | SETA | a_src(ZERO) | fd(2));
    put(COND_6 + 2, ALU | SETM | src(0o14) | m_dest(0o27));
    put(COND_6 + 3, ALU | SETA | a_src(0o50) | fd(0o15));
    put(COND_6 + 4, ALU | SETA | a_src(0o52) | fd(1));
    put(COND_6 + 5, filler().raw() | POPJ);
    let e = ml_same(m);
    assert_eq!(e.machine().mmem[0o10], 1, "condition 6 called once");
    assert_eq!(e.machine().mmem[1], 1, "opcode 1, at LC stepped, ran once");
    assert!(e.machine().macro_dispatch.fused > 0, "D took the return");
}

/// **D's wait is metered** (the contract's §12.1): on the main-loop
/// machine of opcode 1 eleven times, D on, the boot's return and the five
/// returns that need a fetch each begin a wait, `Meters::d_waits`, six, and
/// D dispatches each: six fused returns more than D off. `Meters::d_wait`
/// is the clocks with no commit between each such return's slot and the
/// next word, the handler: the gap from the return's commit to the
/// handler's less the two words, 59 on the first, a miss, and 5 on each
/// hit (`d_s_slot_does_not_wait_in_rd`), 84 in all. D off, neither counts.
#[test]
fn d_s_wait_is_metered() {
    let run = |d: bool| ml_same(d_machine(REV16, d_register(d), &ONES, CODE, false, false)).meters;
    let (off, on) = (run(false), run(true));
    assert_eq!((off.d_waits, off.d_wait), (0, 0), "no wait with D off");
    assert_eq!(on.d_waits, 6, "the boot's return and the five that need a fetch");
    assert_eq!(on.fused - off.fused, on.d_waits, "D dispatched each");
    let m = d_machine(REV16, d_register(true), &ONES, CODE, false, false);
    let (commits, e) = commits_to_the_stop(m, &|_| {});
    // Each return D waits on, its slot, and the next word committed.
    let gaps: Vec<u64> = commits
        .windows(3)
        .filter(|w| {
            (w[0].1 == 6 && w[1].1 == 7)
                || (w[0].1 == OP_1 as u16 + 1 && w[1].1 == OP_1 as u16 + 2 && w[2].0 - w[0].0 > 2)
        })
        .map(|w| w[2].0 - w[0].0 - 2)
        .collect();
    assert_eq!(gaps.len(), 6, "the six waits: {gaps:?}");
    assert_eq!(gaps.iter().filter(|&&g| g == 5).count(), 5, "5 clocks on a hit: {gaps:?}");
    assert_eq!(e.meters.d_wait, gaps.iter().sum::<u64>(), "the meter, against the commits");
    assert_eq!(on.d_wait, e.meters.d_wait, "the same run");
}

// --- Revision 16's Ethernet device behind the posted writes and the cache ------

/// The Ethernet device's rings, buffers and frame, physical word
/// addresses, each on a line: one entry a ring; buffers A and B; a word
/// elsewhere.
const ETH_TX: u64 = 0o1000;
const ETH_RX: u64 = 0o1100;
const ETH_A: u64 = 0o1200;
const ETH_B: u64 = 0o1300;
const ETH_FRAME: u64 = 0o1400;
const ELSEWHERE: u64 = 0o5000;

/// A 60-byte ARP Ethernet frame to the device's own MAC address, its bytes
/// from 14 on counting from `seed`.
fn eth_frame(seed: u8) -> Vec<u8> {
    let mut f = vec![0x02, 0, 0, 0, 0, 0x01, 0x02, 0, 0, 0, 0, 0x02, 0x08, 0x06];
    f.extend((0..46).map(|k| seed.wrapping_add(k)));
    f
}

/// Word `k` of `f` as the device writes it: bytes 4k to 4k + 3, the first
/// in `<7:0>`.
fn eth_word(f: &[u8], k: usize) -> Word {
    Word::from(u32::from_le_bytes(f[4 * k..4 * k + 4].try_into().unwrap()))
}

/// The register-page word `w`'s virtual address.
fn eth_reg(w: u32) -> Word {
    REGISTER_PAGE | Word::from(w)
}

/// The rings set up through the registers and the device enabled.
fn eth_enable(p: &mut Prog) {
    use muir::ethernet_device::{CONTROL, RX_BASE, RX_SIZE, TX_BASE, TX_SIZE};
    p.write(ETH_TX, eth_reg(TX_BASE));
    p.write(0, eth_reg(TX_SIZE));
    p.write(ETH_RX, eth_reg(RX_BASE));
    p.write(0, eth_reg(RX_SIZE));
    p.write(1, eth_reg(CONTROL));
}

/// Rounds until the register-page word `w` reads `v`.
fn eth_wait(p: &mut Prog, w: u32, v: Word) {
    let (at, want) = (p.k(eth_reg(w)), p.k(v));
    let round = p.at();
    p.op(ALU | SETA | a_src(at) | START_READ);
    p.fill(1);
    p.op(jcond(3) | 1 << 6 | SRC_MD | a_src(want) | target(round));
    p.fill(1);
}

/// The receive program: the device enabled; receive entry 0 given buffer
/// A of 64 bytes; buffer A's word 3, the entry's word 2 and the word
/// elsewhere read into M 24, 25 and 27, so that their lines are cached;
/// 250 written 1; a round until 251 reads 1; the three read again into M
/// 26, 30 and 31.
fn eth_receive_program() -> Prog {
    let mut p = Prog::default();
    p.main.push((ETH_A as usize + 3, 0o1111));
    p.main.push((ETH_RX as usize + 2, 0o2222));
    p.main.push((ELSEWHERE as usize, 0o3333));
    eth_enable(&mut p);
    p.write(ETH_A, PHYS | ETH_RX);
    p.write(64, PHYS | (ETH_RX + 1));
    p.read(PHYS | (ETH_A + 3), 0o24);
    p.read(PHYS | (ETH_RX + 2), 0o25);
    p.read(PHYS | ELSEWHERE, 0o27);
    p.write(1, eth_reg(muir::ethernet_device::RX_PROD));
    eth_wait(&mut p, muir::ethernet_device::RX_CONS, 1);
    p.read(PHYS | (ETH_A + 3), 0o26);
    p.read(PHYS | (ETH_RX + 2), 0o30);
    p.read(PHYS | ELSEWHERE, 0o31);
    p.stop();
    p
}

/// `p`'s machine on revision 16, its scripted peer sending `frames` at
/// their times.
fn eth_machine(p: &Prog, frames: &[(u64, Vec<u8>)]) -> (Machine, muir::ethernet_device::Script) {
    let mut m = machine(p, REV16);
    let s = muir::ethernet_device::Script::default();
    for (t, f) in frames {
        s.arrive(*t, f.clone());
    }
    m.attach_ethernet_peer(Box::new(s.clone()));
    (m, s)
}

/// **A received Ethernet frame over lines the cache holds is read after
/// 251 moves** (QUUX's Ethernet contract, §2.5 (ii)): the processor has
/// caught buffer A's word, the entry's word 2 and a word elsewhere; the
/// frame arrives while it rounds on 251; after it, it reads the frame's
/// word and its length, on the pipeline as on `micro`. Line clearing
/// swept nothing: the two lines it cleared missed again, and the word
/// elsewhere still hit.
#[test]
fn a_received_frame_is_seen_after_251_moves() {
    let p = eth_receive_program();
    let f = eth_frame(0x40);
    let frames = [(200_000, f.clone())];
    let (m, _) = eth_machine(&p, &frames);
    let u = match run_engine(Micro::new(m)) {
        Ok(u) => u,
        Err((h, _)) => panic!("micro halted: {h:?}"),
    };
    let (m, _) = eth_machine(&p, &frames);
    let e = ends_on(m, &|_| {});
    let (m, um) = (e.machine(), u.machine());
    for (name, m) in [("micro", um), ("rtl", m)] {
        assert_eq!(
            (m.mmem[0o24], m.mmem[0o25], m.mmem[0o27]),
            (0o1111, 0o2222, 0o3333),
            "{name}: before"
        );
        assert_eq!(m.mmem[0o26] & 0xffff_ffff, eth_word(&f, 3), "{name}: the frame");
        assert_eq!(m.mmem[0o26] >> 32, 0o005, "{name}: a fixnum");
        assert_eq!(m.mmem[0o30] & 0xffff_ffff, 60, "{name}: its length");
        assert_eq!(m.mmem[0o31], 0o3333, "{name}: the word elsewhere");
    }
    assert_eq!(state(m), state(um), "the pipeline against micro");
    assert_eq!(e.port.meters.sweeps, 0, "no sweep");
    assert_eq!(e.port.meters.engine_reads, 1, "the entry read once");
    assert_eq!(
        e.port.cache.misses,
        3 + 2,
        "three lines caught, two of them cleared and missed again"
    );
}

/// **A transmit entry still in the posted writes when 244 is written is
/// read whole** (QUUX's Ethernet contract, §2.5): main memory answers a
/// write in 25 µs, so the entry and the frame are still going in when the
/// producer write is made; the device takes 244 once every earlier write
/// is answered, and sends the frame as written.
#[test]
fn a_transmit_entry_in_the_posted_writes_is_read_whole() {
    use muir::ethernet_device::{TX_CONS, TX_PROD};
    let f = eth_frame(0x50);
    let mut p = Prog::default();
    eth_enable(&mut p);
    for k in 0..15 {
        p.write(eth_word(&f, k), PHYS | (ETH_FRAME + k as u64));
    }
    p.write(ETH_FRAME, PHYS | ETH_TX);
    p.write(60, PHYS | (ETH_TX + 1));
    p.write(1, eth_reg(TX_PROD));
    eth_wait(&mut p, TX_CONS, 1);
    p.stop();
    let slow = |e: &mut Pipeline| {
        e.skip_sweep();
        let t = muir::pipeline::PortTiming { read_ns: 247, write_ns: 25_000, occupancy_ns: 10 };
        e.configure(e.period(), t, 65_536);
    };
    let (m, s) = eth_machine(&p, &[]);
    let e = ends_on(m, &slow);
    assert_eq!(s.sent().len(), 1, "one frame sent");
    assert_eq!(s.sent()[0].1, f, "the entry and the frame read whole");
    assert!(e.meters.cmd_prod_wait > 0, "244 waited for the writes");
}

/// **A receive entry still in the posted writes when 250 is written is not
/// taken before it lands** (QUUX's Ethernet contract, §2.5): entry 0 names
/// buffer A, and the program posts a transmit, then writes B over the
/// entry and 250 at once, with main memory answering a write in 25 µs. The
/// peer answers the transmit, sent 20 µs after 244, at once: that frame
/// finds no buffer given and is dropped; its second answer, 100 µs later,
/// goes into B. A device that took 250 at once would put the first into A.
#[test]
fn a_receive_entry_in_the_posted_writes_is_taken_once_it_lands() {
    use muir::ethernet_device::{RX_CONS, RX_PROD, STATUS, TX_PROD};
    let (one, two) = (eth_frame(0x11), eth_frame(0x22));
    let mut p = Prog::default();
    p.main.push((ETH_RX as usize, ETH_A));
    p.main.push((ETH_RX as usize + 1, 64));
    p.main.push((ETH_A as usize + 3, 0o1111));
    p.main.push((ETH_B as usize + 3, 0o2222));
    p.main.push((ETH_TX as usize, ETH_FRAME));
    p.main.push((ETH_TX as usize + 1, 60));
    for (k, w) in (0..15).map(|k| (k, eth_word(&eth_frame(0x33), k))) {
        p.main.push((ETH_FRAME as usize + k, w));
    }
    eth_enable(&mut p);
    p.write(1, eth_reg(TX_PROD));
    p.write(ETH_B, PHYS | ETH_RX);
    p.write(1, eth_reg(RX_PROD));
    eth_wait(&mut p, RX_CONS, 1);
    p.read(PHYS | (ETH_B + 3), 0o26);
    p.read(PHYS | (ETH_A + 3), 0o27);
    p.read(eth_reg(STATUS), 0o30);
    p.stop();
    let slow = |e: &mut Pipeline| {
        e.skip_sweep();
        let t = muir::pipeline::PortTiming { read_ns: 247, write_ns: 25_000, occupancy_ns: 10 };
        e.configure(e.period(), t, 65_536);
    };
    let (m, s) = eth_machine(&p, &[]);
    s.reply(0, one);
    s.reply(200_000, two.clone());
    let e = ends_on(m, &slow);
    assert_eq!(s.sent().len(), 1, "the transmit sent");
    let m = e.machine();
    assert_eq!(m.mmem[0o26] & 0xffff_ffff, eth_word(&two, 3), "the second frame in B");
    assert_eq!(m.mmem[0o27], 0o1111, "nothing in A");
    assert_eq!(m.mmem[0o30] >> 16 & 0xff, 1, "the first dropped, no buffer given");
    assert!(e.meters.cmd_prod_wait > 0, "250 waited for the write");
}

// --- Line clearing: 251 after the last set (QUUX's Ethernet contract, §2.5 (ii)) --

/// A buffer for the longest Ethernet frame, on a line: 1,514 bytes are 379
/// words, 48 lines.
const ETH_BIG: u64 = 0o10000;

/// An ARP Ethernet frame of `n` bytes to the device's own MAC address.
fn eth_frame_of(n: usize, seed: u8) -> Vec<u8> {
    let mut f = vec![0x02, 0, 0, 0, 0, 0x01, 0x02, 0, 0, 0, 0, 0x02, 0x08, 0x06];
    f.extend((0..n - 14).map(|k| seed.wrapping_add(k as u8)));
    f
}

/// Counts M 26 down from `n`: a delay that touches no memory.
fn eth_delay(p: &mut Prog, n: u64) {
    let k = p.k(n);
    p.op(ALU | SETA | a_src(k) | m_dest(0o26));
    let top = p.at();
    p.op(ALU | muir::isa::asm::SUB | a_src(ONE) | m_src(0o26) | m_dest(0o26));
    p.op(jcond(3) | 1 << 6 | a_src(ZERO) | m_src(0o26) | target(top) | N);
    p.fill(1);
}

/// The receive program for line clearing: the device enabled, receive
/// entry 0 given the buffer at `buf` of `room` bytes, a word of each line
/// in `cached` read, so that the cache holds it, 250 written 1, then
/// `tail`.
fn eth_lc_program(buf: u64, room: u64, cached: &[u64], tail: impl FnOnce(&mut Prog)) -> Prog {
    let mut p = Prog::default();
    eth_enable(&mut p);
    p.write(buf, PHYS | ETH_RX);
    p.write(room, PHYS | (ETH_RX + 1));
    for &a in cached {
        p.main.push((a as usize, 0o5252525252 ^ a));
        p.read(PHYS | a, 0o24);
    }
    p.write(1, eth_reg(muir::ethernet_device::RX_PROD));
    tail(&mut p);
    p.stop();
    p
}

/// What a delivery did, clock by clock: the clock the device filled the
/// entry in (the host's RX_CONS moved), the clock after which the machine's
/// 251 had moved (status `<8>`), the clock each line of `lines` stopped
/// being held, and the run.
struct EthClocks {
    landed: u64,
    moved: u64,
    cleared: Vec<Option<u64>>,
    e: Pipeline,
}

/// `p` on revision 16 at `period` and the Kria's memory timing, `frames`
/// arriving at their clocks, run clock by clock until 251 has moved and
/// `after` clocks more.
fn eth_clocks(
    p: &Prog,
    period: u64,
    frames: &[(u64, Vec<u8>)],
    lines: &[u64],
    after: u64,
) -> EthClocks {
    eth_clocks_of(p, period, frames, lines, after, 1)
}

/// [`eth_clocks`] for the delivery that fills entry `index` - 1, 252
/// taken up to the one before it first.
fn eth_clocks_of(
    p: &Prog,
    period: u64,
    frames: &[(u64, Vec<u8>)],
    lines: &[u64],
    after: u64,
    index: u32,
) -> EthClocks {
    use muir::ethernet_device::RX_CONS;
    let mut m = machine(p, REV16);
    let s = muir::ethernet_device::Script::default();
    for (clock, f) in frames {
        s.arrive(clock * period, f.clone());
    }
    m.attach_ethernet_peer(Box::new(s));
    let mut e = Pipeline::new(m);
    e.configure(period, muir::pipeline::PortTiming::KRIA, 65_536);
    e.events = Some(Vec::new());
    e.boot();
    e.skip_sweep();
    let (mut landed, mut moved) = (None, None);
    let mut cleared = vec![None; lines.len()];
    for _ in 0..400_000 {
        let before: Vec<bool> = lines.iter().map(|&a| e.port.cache.holds(a as u32)).collect();
        e.tick().expect("no halt");
        let c = e.clock();
        let d = &e.machine().ethernet;
        if landed.is_none() && d.read(RX_CONS) == index {
            landed = Some(c);
        }
        if moved.is_none() && landed.is_some() && d.waiting() {
            moved = Some(c);
        }
        for (k, &a) in lines.iter().enumerate() {
            if before[k] && !e.port.cache.holds(a as u32) && cleared[k].is_none() {
                cleared[k] = Some(c);
            }
        }
        if moved.is_some_and(|m| c >= m + after) {
            let (landed, moved) = (landed.unwrap(), moved.unwrap());
            return EthClocks { landed, moved, cleared, e };
        }
    }
    panic!("251 never moved: landed {landed:?}");
}

/// The clock of each event `want` picks, in order.
fn eth_event_clocks(e: &Pipeline, want: impl Fn(&muir::pipeline::Event) -> bool) -> Vec<u64> {
    e.events.as_ref().unwrap().iter().filter(|(_, ev)| want(ev)).map(|&(c, _)| c).collect()
}

/// The buffer's lines for `n` bytes at `buf`: one word of each, at its
/// line's own offset.
fn eth_lines(buf: u64, n: u64) -> Vec<u64> {
    (0..n.div_ceil(4).div_ceil(8)).map(|l| buf + 8 * l + (l & 7)).collect()
}

/// **251 moves at the edge of clock T = R + 9 + L after the delivery's
/// clock, as the fabric's line clearing moves it** (QUUX's Ethernet
/// contract, §2.5 (ii)), R the port's read latency in clocks and L the
/// buffer's lines cleared: at the Kria's memory timing a 42-byte Ethernet
/// frame into an aligned buffer (L = 2) and a 1,514-byte one (L = 48) give
/// T = 46 and 92 at 10 ns (R = 35), 52 and 98 at 8.5 ns (R = 41), and 31
/// and 77 at 18 ns (R = 20), the fabric's measured figures. The machine's
/// reads of 251 granted before clock T read the old index and those from T
/// the new one. **Fails** a device whose 251 moves at the delivery (T =
/// 0), or an engine a clock off anywhere.
#[test]
fn line_clearing_moves_251_at_r_plus_9_plus_l() {
    use muir::pipeline::Event;
    let page_251 = muir::tlb::REGISTER_PAGE_BUS | muir::ethernet_device::RX_CONS;
    for (period, r, want) in [(20, 35, [46, 92]), (17, 41, [52, 98]), (36, 20, [31, 77])] {
        for ((n, buf, room), want) in [(42, ETH_A, 64), (1514, ETH_BIG, 1536)].into_iter().zip(want)
        {
            let p =
                eth_lc_program(buf, room, &[], |p| eth_wait(p, muir::ethernet_device::RX_CONS, 1));
            let run = eth_clocks(&p, period, &[(3000, eth_frame_of(n, 7))], &[], 16);
            assert_eq!(run.e.port.clocks().read, r, "period {period}: R");
            let l = (n as u64).div_ceil(4).div_ceil(8);
            assert_eq!(run.moved - run.landed, want, "period {period}, {n} bytes: T");
            assert_eq!(want, r + 9 + l, "period {period}, {n} bytes: R + 9 + L");
            // The poll's reads of 251, each with the word it returned.
            let evs = run.e.events.as_ref().unwrap();
            let mut reads = Vec::new();
            for (k, &(c, ev)) in evs.iter().enumerate() {
                if ev == Event::Register(page_251)
                    && let Some(&(_, Event::Md(w))) =
                        evs[k + 1..].iter().find(|(_, e)| matches!(e, Event::Md(_)))
                {
                    reads.push((c, w));
                }
            }
            let at = run.landed + want;
            assert!(reads.iter().any(|&(c, _)| c < at) && reads.iter().any(|&(c, _)| c >= at));
            for (c, w) in reads {
                assert_eq!(
                    w & 0xffff,
                    Word::from(c >= at),
                    "period {period}, {n} bytes: 251 read at {c}"
                );
            }
        }
    }
}

/// **The sets are cleared one a clock, the buffer's lines first and the
/// entry's last, and 251 moves only after the last** (QUUX's Ethernet
/// contract, §2.5 (ii)): every one of the 48 lines a 1,514-byte Ethernet
/// frame fills, and the entry's, is held in the cache before the delivery;
/// buffer line i stops being held at clock R + 9 + i after the delivery's,
/// the entry's at R + 9 + 48 = T, the clock 251 moves at; a line elsewhere
/// stays. The same for a 42-byte frame's 2 lines. **Fails** a sweep, sets
/// cleared all at once, out of order or after 251, or the entry's line
/// left.
#[test]
fn line_clearing_clears_the_sets_line_by_line_before_251_moves() {
    for (n, buf, room) in [(42, ETH_A, 64), (1514, ETH_BIG, 1536)] {
        let mut lines = eth_lines(buf, n);
        let l = lines.len() as u64;
        lines.push(ETH_RX + 2);
        lines.push(ELSEWHERE);
        let p =
            eth_lc_program(buf, room, &lines, |p| eth_wait(p, muir::ethernet_device::RX_CONS, 1));
        let run = eth_clocks(&p, 20, &[(6000, eth_frame_of(n as usize, 9))], &lines, 16);
        let r = run.e.port.clocks().read;
        let t = run.moved - run.landed;
        assert_eq!(t, r + 9 + l, "{n} bytes: T");
        for (i, c) in run.cleared[..l as usize].iter().enumerate() {
            assert_eq!(*c, Some(run.landed + r + 9 + i as u64), "{n} bytes: buffer line {i}");
        }
        assert_eq!(
            run.cleared[l as usize],
            Some(run.moved),
            "{n} bytes: the entry's line, at 251's move"
        );
        assert_eq!(run.cleared[l as usize + 1], None, "{n} bytes: the line elsewhere stays");
    }
}

/// **Lookups never wait for line clearing** (QUUX's Ethernet contract,
/// §2.5 (ii)): the processor reads a line elsewhere, cached, over and over
/// while a 1,514-byte Ethernet frame is delivered; its lookups go on in the
/// clocks the sets are cleared in, every one a hit, and no sweep is made.
/// **Fails** the whole-cache sweep, or clears that hold the lookups.
#[test]
fn line_clearing_never_holds_a_lookup() {
    use muir::pipeline::Event;
    let p = eth_lc_program(ETH_BIG, 1536, &[ELSEWHERE], |p| {
        let k = p.k(1500);
        p.op(ALU | SETA | a_src(k) | m_dest(0o26));
        let top = p.at();
        p.read(PHYS | ELSEWHERE, 0o27);
        p.op(ALU | muir::isa::asm::SUB | a_src(ONE) | m_src(0o26) | m_dest(0o26));
        p.op(jcond(3) | 1 << 6 | a_src(ZERO) | m_src(0o26) | target(top) | N);
        p.fill(1);
    });
    let run = eth_clocks(&p, 20, &[(3000, eth_frame_of(1514, 3))], &[ELSEWHERE], 16);
    let r = run.e.port.clocks().read;
    let misses = run.e.port.cache.misses;
    let reads = eth_event_clocks(&run.e, |ev| *ev == Event::Grant(ELSEWHERE as u32, false));
    let clears = run.moved - 48..=run.moved;
    let during = reads.iter().filter(|c| clears.contains(c)).count();
    assert_eq!((run.e.port.meters.sweeps, run.e.port.meters.sweep_clocks), (0, 0), "no sweep");
    assert!(run.moved - run.landed >= r + 9 + 48, "T no shorter than R + 9 + L");
    assert!(during >= 4, "lookups in the clears' clocks: {during}");
    assert_eq!(run.cleared[0], None, "the line elsewhere stays");
    let e = run.e;
    let mut e = e;
    for _ in 0..200 {
        e.tick().unwrap();
    }
    assert_eq!(e.port.cache.misses, misses, "every read of the line elsewhere a hit");
    assert_eq!(e.machine().mmem[0o27], 0o5252525252 ^ ELSEWHERE, "its word");
}

/// **A processor's fill delays the entry's read, and a fill's install
/// delays a clear by a clock** (QUUX's Ethernet contract, §2.5 (ii)): the
/// engine reads the entry through the port's one fill, so its read waits
/// while a fill of the processor's is in flight, or a lookup is made in the
/// clock or was in the clock before; and a set is not cleared in the clock
/// a fill installs. A program makes one read that misses, at clock g, and
/// an Ethernet frame is delivered at each clock across it, so that the
/// engine's read, wanted from clock c = delivery + 5, comes before the
/// processor's, at its grant, while its fill is in flight, or after:
/// - **42 bytes:** the entry's fill goes at c, or at g + 1 + R when c is in
///   g..=g + R, and T = (that clock − the delivery) + R + 4 + L;
/// - **1,514 bytes,** the processor's read after the entry's: T = R + 9 +
///   L, one more when the processor's fill installs at the edge of a clock
///   a set is cleared in.
///
/// **Fails** an engine that reads beside a processor's fill, ignores a
/// lookup's clocks, or clears beside an install.
#[test]
fn line_clearing_waits_for_a_processor_s_fill_and_its_install() {
    use muir::pipeline::Event;
    let program = |buf: u64| {
        eth_lc_program(buf, 1536, &[], |p| {
            eth_delay(p, 400);
            p.read(PHYS | ELSEWHERE, 0o27);
        })
    };
    let grants =
        |e: &Pipeline| eth_event_clocks(e, |ev| *ev == Event::Grant(ELSEWHERE as u32, false));
    // The read's grant, with the frame long after it.
    let quiet = eth_clocks(&program(ETH_A), 20, &[(40_000, eth_frame_of(42, 1))], &[], 4);
    let g = grants(&quiet.e)[0];
    let r = quiet.e.port.clocks().read;
    assert!(g > 100 && g + 4 * r < 40_000, "the read at {g}, well before the frame");
    let mut waited = 0;
    for d in -4..=r as i64 + 4 {
        let (c, l) = ((g as i64 + d) as u64, 2);
        let run = eth_clocks(&program(ETH_A), 20, &[(c - 5, eth_frame_of(42, 5))], &[], 4);
        assert_eq!(run.landed + 5, c, "the delivery at its clock");
        let go = if (g..=g + r).contains(&c) { g + 1 + r } else { c };
        assert_eq!(run.moved - run.landed, go - run.landed + r + 4 + l, "42 bytes, c = g {d:+}: T");
        waited += u64::from(go > c);
    }
    assert_eq!(waited, r + 1, "the entry's fill waited for the processor's");
    let mut held = 0;
    for d in r - 2..=r + 60 {
        let (c, l) = (g - d, 48);
        let run = eth_clocks(&program(ETH_BIG), 20, &[(c - 5, eth_frame_of(1514, 5))], &[], 4 * r);
        assert_eq!(run.landed + 5, c, "the delivery at its clock");
        // The processor's fill installs at the edge of the clock before its
        // word lands in MD.
        let first = grants(&run.e)[0];
        let evs = run.e.events.as_ref().unwrap();
        let md = evs.iter().find(|&&(k, ev)| k > first && matches!(ev, Event::Md(_))).unwrap().0;
        let clears = c + r + 4..=c + r + 4 + l;
        let extra = u64::from(clears.contains(&(md - 1)));
        assert_eq!(run.moved - run.landed, r + 9 + l + extra, "1,514 bytes, c = g - {d}: T");
        held += extra;
    }
    // The processor's read granted at g, c + d, installs at the edge of
    // g + R, a clear's clock for d in 4..=4 + L: 17 of d in R + 1..=R + 60;
    // and for d in R - 2..=R, its read busy behind the entry's fill, it is
    // granted as that fill lands and installs inside the clears too.
    assert_eq!(held, (4..=4 + 48).filter(|d| *d > r).count() as u64 + 3, "installs held a clear");
}

/// **Each delivery's entry is read again** (QUUX's Ethernet contract, §2.5
/// (ii)): through a ring of one entry, a 42-byte Ethernet frame and then,
/// the entry taken back and written again, a 1,514-byte one; the engine
/// drops the line it read the first time and reads the entry again, so the
/// second's T is R + 9 + 48 and its 48 lines are cleared. **Fails** an
/// engine that keeps its line from one entry to the next, which reads the
/// first entry's words.
#[test]
fn line_clearing_reads_the_entry_again_for_each_delivery() {
    use muir::ethernet_device::{RX_CONS, RX_PROD, RX_TAKEN};
    let lines = eth_lines(ETH_BIG, 1514);
    let p = eth_lc_program(ETH_A, 64, &[], |p| {
        eth_wait(p, RX_CONS, 1);
        p.write(1, eth_reg(RX_TAKEN));
        p.write(ETH_BIG, PHYS | ETH_RX);
        p.write(1536, PHYS | (ETH_RX + 1));
        for &a in &lines {
            p.main.push((a as usize, 0o5252525252 ^ a));
            p.read(PHYS | a, 0o24);
        }
        p.write(2, eth_reg(RX_PROD));
        eth_wait(p, RX_CONS, 2);
    });
    let frames = [(3000, eth_frame_of(42, 1)), (12_000, eth_frame_of(1514, 2))];
    let first = eth_clocks_of(&p, 20, &frames, &[], 4, 1);
    let r = first.e.port.clocks().read;
    assert_eq!(first.moved - first.landed, r + 9 + 2, "the first: T");
    let second = eth_clocks_of(&p, 20, &frames, &lines, 4, 2);
    assert_eq!(second.moved - second.landed, r + 9 + 48, "the second: T");
    assert_eq!(second.e.port.meters.engine_reads, 2, "the entry read twice");
    for (i, c) in second.cleared.iter().enumerate() {
        assert_eq!(*c, Some(second.landed + r + 9 + i as u64), "the second's line {i}");
    }
}

/// **The halt's drain waits for line clearing** (QUUX's Ethernet contract,
/// §2.5 (ii)): a halt asked for ten clocks after a 1,514-byte Ethernet
/// frame is delivered ends only once 251 has moved, at the delivery's
/// clock + T or after, so that a checkpoint taken in the halt has 251 at
/// the entries filled. **Fails** a drain that leaves the engine at work.
#[test]
fn the_halt_s_drain_waits_for_line_clearing() {
    use muir::ethernet_device::RX_CONS;
    let p = eth_lc_program(ETH_BIG, 1536, &[], |p| eth_wait(p, RX_CONS, 1));
    let mut m = machine(&p, REV16);
    let s = muir::ethernet_device::Script::default();
    s.arrive(3000 * 20, eth_frame_of(1514, 4));
    m.attach_ethernet_peer(Box::new(s));
    let mut e = Pipeline::new(m);
    e.configure(20, muir::pipeline::PortTiming::KRIA, 65_536);
    e.boot();
    e.skip_sweep();
    while e.machine().ethernet.read(RX_CONS) != 1 {
        e.tick().unwrap();
    }
    let landed = e.clock();
    for _ in 0..10 {
        e.tick().unwrap();
    }
    assert!(!e.machine().ethernet.waiting(), "251 not moved yet");
    e.halt_and_drain(10_000).unwrap();
    assert!(e.is_halted(), "halted");
    let t = e.port.clocks().read + 9 + 48;
    assert!(e.clock() >= landed + t, "halted at {} for a delivery at {landed}, T {t}", e.clock());
    assert!(e.machine().ethernet.waiting(), "251 moved before the halt");
}

/// **The engine's read waits for a lookup made in its clock or the clock
/// before** (QUUX's Ethernet contract, §2.5 (ii); the fabric's transfer
/// channel, whose fill is made only when no lookup is answered or asked
/// for): the processor reads a cached line every third clock, each a hit,
/// and a 42-byte Ethernet frame is delivered at each of six clocks across
/// that rhythm. The entry's read goes at the first clock at or after the
/// one it is wanted in with no lookup in it or the clock before, and T
/// follows. **Fails** an engine that reads beside a lookup, or after one a
/// clock too soon.
#[test]
fn line_clearing_waits_for_a_lookup_s_clocks() {
    use muir::pipeline::Event;
    let program = || {
        eth_lc_program(ETH_A, 64, &[ELSEWHERE], |p| {
            let va = p.k(PHYS | ELSEWHERE);
            for _ in 0..300 {
                p.op(ALU | SETA | a_src(va) | START_READ);
            }
        })
    };
    let reads =
        |e: &Pipeline| eth_event_clocks(e, |ev| *ev == Event::Grant(ELSEWHERE as u32, false));
    let quiet = eth_clocks(&program(), 20, &[(40_000, eth_frame_of(42, 1))], &[], 4);
    let r = quiet.e.port.clocks().read;
    let rs = reads(&quiet.e);
    assert!(rs[2..250].windows(2).all(|w| w[1] - w[0] == 3), "a hit every third clock");
    let mut waited = 0;
    for phase in 0..6 {
        let c = rs[100] + phase;
        let run = eth_clocks(&program(), 20, &[(c - 5, eth_frame_of(42, 2))], &[], 4);
        assert_eq!(run.landed + 5, c, "the delivery at its clock");
        let g = reads(&run.e);
        let go = (c..).find(|x| !g.contains(x) && !g.contains(&(x - 1))).unwrap();
        assert_eq!(
            run.moved - run.landed,
            go - run.landed + r + 4 + 2,
            "wanted at {c}, read at {go}"
        );
        waited += go - c;
    }
    // Wanted in a lookup's clock, two clocks late; in the clock after, one.
    assert_eq!(waited, 2 * (2 + 1), "the waits of two rhythms");
}
