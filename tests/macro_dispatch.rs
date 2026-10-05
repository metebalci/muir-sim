// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's MACRO-DISPATCH register, its MACRO DISPATCH MEMORY, the fused
//! return and the operand address (contract H8a), on hand-written
//! microcode.
//!
//! Functional destination 5 writes the register (`<13:0>` the main loop's
//! address, `<31>` the enable), 6 the memory's index and 7 the entry at it.
//! With the register enabled, a return that pops the main loop's word
//! where no instruction fetch is needed goes straight to the handler the
//! memory names for the next halfword's `<15:6>`, if the entry has R and P
//! clear: the main loop's dispatch and the push of its return are not run,
//! two microcycles, and the popped word stays on the stack unless the
//! entry's N is set. Every other return runs the main loop as it always
//! has. When the entry has the operand bit and the halfword's register is
//! LOCAL or ARG, PDL-INDEX is loaded with the operand's address at the end
//! of the microcycle after the return.
//!
//! On `rtl`, QUUX has the cache-only prefetch too, with the page's reach
//! (contract H8a §3.5, G2 §9): a return that needs the next word in
//! sequence fuses when the fetch before left it in the buffer. `micro` has
//! no cache and fuses only the returns that need no fetch, so the two
//! engines fuse different returns and take different microcycles, and leave
//! the same state wherever the handlers keep §3.3's rule; the tests that
//! compare them count each engine's fused returns ([`fused_on`]).
//!
//! The main loop here is made as microcode 2001's `QMLP` is
//! (`uc-macrocode.lisp:9-13`): the condition-6 call, `M-INST-BUFFER <- MD`,
//! `(DISPATCH-XCT-NEXT M-INST-OP OPDTB)` on the halfword's `<13:9>`, and the
//! push of `A-MAIN-DISPATCH` back.

use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{
    ALU, ALWAYS, CARRY_IN, DISPATCH, JUMP, M_PLUS_C, N, P, POPJ, R, SETA, SETM, SRC_MD, a_dest,
    a_src, d_addr, d_len, filler, m_dest, m_src, rot, src, target,
};
use muir::machine::{Geometry, Machine, Operand, macro_dispatch};
use muir::micro::Micro;
use muir::rtl::Rtl;

mod support;

use support::macro_dispatch::{Checked, Counts, Executes, PdlAt, Write, fill_generic, writes};

/// The main loop, at an address with `<1:0>` clear as the stream hardware
/// requires (`uc-macrocode.lisp:6`).
const QMLP: u64 = 0o100;
/// The opcode table in dispatch memory.
const OPDTB: u64 = 0o2300;
/// A dispatch table whose two entries return, as `QMDTBD`'s `D-PDL` does
/// for `QIMOVE1` (`uc-parameters.lisp:1283-1289`).
const RETURNS: u64 = 0o2200;
/// A-MAIN-DISPATCH, the main loop's return: `<14>` and the address.
const MAIN: u32 = 1 << 14 | QMLP as u32;
/// Where the macroinstructions are, as a word address; its page is mapped.
const CODE: u32 = 0o400;
/// Where opcode 7, the last, goes: a jump to itself.
const STOP: u16 = 0o177;
/// The handler a specialised entry names ([`specialised`]): it counts in
/// M 7.
const SPECIAL: u64 = 0o300;
/// The main loop's `<13:9>` rotate, `IR<4:0>`: the field at `<9>` brought
/// to `<0>`, 23 in the CADR's ring of 32, and 31 in QUUX's ring of 40, LC
/// byte mode adding 24 for halfword 0 (contract G2 appendix A1.2).
fn op_rotate(geometry: Geometry) -> u64 {
    geometry.word_bits as u64 - 9
}

/// A halfword: `<13:9>` the opcode and `<8:6>` the register.
const fn hw(op: u32, reg: u32) -> u32 {
    op << 9 | reg << 6
}

/// A halfword with `<5:0>` the delta too.
const fn hwd(op: u32, reg: u32, delta: u32) -> u32 {
    hw(op, reg) | delta
}

/// Where the register names `A-LOCALP`, A memory 432 as in microcode 2001
/// (`uc-parameters.lisp:1192`), and `M-AP`, M memory 21
/// (`uc-parameters.lisp:475`).
const LOCALP_AT: u64 = 0o432;
const AP_AT: u64 = 0o21;
/// What they hold at the start: `A-LOCALP` near the top of the PDL
/// buffer's fourteen bits, so that a large delta wraps.
const LOCALP: u32 = 0o37760;
const AP: u32 = 0o200;
/// PDL-INDEX where no operand address has been loaded: the start's, and
/// what [`RECORD`]'s handler leaves. In A memory 54.
const SENTINEL: u32 = 0o3777;
const SENTINEL_AT: u64 = 0o54;
/// Opcode 10 pushes PDL-INDEX as its first microinstruction finds it,
/// sets it to [`SENTINEL`], and returns by a POPJ whose microcycle after,
/// its [`Setup::slot`], pushes PDL-INDEX again.
const RECORD: u32 = 0o10;
/// Its handler, and the microcycle after its return.
const RECORD_AT: u64 = 0o230;
const RECORD_SLOT: u64 = RECORD_AT + 3;
/// Opcode 11 steps `M-AP` in the POPJ that returns.
const SETAP: u32 = 0o11;
const SETAP_AT: u64 = 0o240;
/// Opcode 12 sets `A-LOCALP` to [`LOCALP_2`], from A memory 56, in the POPJ
/// that returns.
const SETLOCALP: u32 = 0o12;
const SETLOCALP_AT: u64 = 0o250;
const LOCALP_2: u32 = 0o1000;

/// The operand program: [`RECORD`] with LOCAL, ARG and another register,
/// each once in a word's first halfword, which needs a fetch and is not
/// fused, and once in its second, which is; one after [`SETAP`]'s return
/// and one after [`SETLOCALP`]'s; and deltas of 0 to 77.
const OPERANDS: [u32; 14] = [
    hwd(RECORD, 5, 3),
    hwd(RECORD, 5, 7),
    hwd(RECORD, 6, 0),
    hwd(RECORD, 6, 5),
    hw(SETAP, 0),
    hwd(RECORD, 6, 2),
    hwd(RECORD, 4, 1),
    hwd(RECORD, 4, 1),
    hwd(RECORD, 5, 0o77),
    hwd(RECORD, 5, 0o77),
    hw(SETLOCALP, 0),
    hwd(RECORD, 5, 1),
    hw(7, 0),
    hw(7, 0),
];

/// What [`RECORD`] pushes over [`OPERANDS`] with the operand bit in its
/// entries, on `engine`: in pairs, PDL-INDEX at the handler's first
/// microcycle and in the microcycle after its return. The second halfwords
/// with LOCAL or ARG find the operand's address, masked to fourteen bits;
/// on `rtl` so do the second word's first halfword, ARG with delta 0, and
/// the fifth's, LOCAL with delta 77, whose returns fuse on the prefetched
/// word (each word's fetch left the next, in the first word's line of
/// eight, in the buffer). Every other push finds [`SENTINEL`], the
/// microcycle after a return included.
fn operand_records(engine: &str) -> Vec<u32> {
    let s = SENTINEL;
    vec![
        s,
        s,
        (LOCALP + 7) & 0o37777,
        s,
        if engine == "rtl" { AP + 1 } else { s },
        s,
        AP + 1 + 5,
        s,
        // After SETAP: `M-AP` stepped by the return itself.
        AP + 1 + 1 + 2,
        s,
        s,
        s,
        s,
        s,
        if engine == "rtl" { (LOCALP + 0o77) & 0o37777 } else { s },
        s,
        (LOCALP + 0o77) & 0o37777,
        s,
        // After SETLOCALP: `A-LOCALP` written by the return itself.
        LOCALP_2 + 1,
        s,
    ]
}

/// Whether the return by opcode `op`'s handler can fuse: every one but a
/// jump with R's (opcode 2) while [`macro_dispatch::JUMP_RETURNS_FUSE`] takes
/// jumps out.
fn fuses(op: u32) -> bool {
    op != 2 || macro_dispatch::JUMP_RETURNS_FUSE
}

/// The program's halfwords in the order they run: a word's `<15:0>`, then
/// its `<31:16>`. Opcodes 1 to 4 are plain jumps, returning by a POPJ
/// (1), a jump with R (2), a dispatch with R (3), and a POPJ after pushing
/// the return back, their entry having N (4); 5's entry falls through (R
/// and P) and 6's pushes (P and N, as a trap's); 7 stops.
const PROGRAM: [u32; 22] = [
    hw(1, 0),
    hw(2, 5),
    hw(3, 1),
    hw(4, 0),
    hw(1, 2),
    hw(2, 0),
    hw(6, 0),
    hw(5, 0),
    hw(1, 0),
    hw(1, 0),
    hw(3, 0),
    hw(6, 0),
    hw(5, 0),
    hw(2, 5),
    hw(4, 3),
    hw(4, 0),
    hw(2, 5),
    hw(3, 7),
    hw(3, 0),
    hw(2, 5),
    hw(7, 0),
    hw(7, 0),
];

/// How many times each opcode 1 to 6 runs in [`PROGRAM`].
const COUNTS: [u32; 6] = [4, 5, 4, 3, 2, 2];

/// What can keep a return from fusing, put into opcode 1's POPJ.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Pop {
    /// A plain POPJ.
    Plain,
    /// The POPJ writes M 31, the same word back.
    WritesM31,
    /// The POPJ pushes the main loop's word too.
    Pushes,
    /// The POPJ writes INTERRUPT-CONTROL, as it stands.
    WritesInterruptControl,
}

/// The run's settings.
#[derive(Clone, Copy)]
struct Setup {
    geometry: Geometry,
    /// The register's word, written by destination 5 at the start.
    register: u32,
    sequence_break: bool,
    pop: Pop,
    /// The MACRO DISPATCH MEMORY holds [`specialised`]'s entry for opcode
    /// 2 with register 5.
    specialised: bool,
    /// The halfwords run.
    program: &'static [u32],
    /// [`RECORD`]'s entries have the operand bit, whatever the register.
    operand: bool,
    /// What runs in the microcycle after [`RECORD`]'s return, in place of
    /// its push of PDL-INDEX.
    slot: Option<u64>,
    /// What [`RECORD`]'s handler runs first, in place of its push of
    /// PDL-INDEX.
    first: Option<u64>,
    /// Where the program's first word is, a word address in the mapped
    /// pages 1 and 2.
    code: u32,
    /// Handlers and words more, put in after the rest.
    patch: Option<fn(&mut Machine)>,
}

impl Setup {
    fn off() -> Setup {
        Setup {
            geometry: Geometry::QUUX,
            register: 0,
            sequence_break: false,
            pop: Pop::Plain,
            specialised: false,
            program: &PROGRAM,
            operand: false,
            slot: None,
            first: None,
            code: CODE,
            patch: None,
        }
    }

    /// The operand program, enabled, with the operand bit.
    fn operands() -> Setup {
        Setup { program: &OPERANDS, operand: true, ..Setup::on() }
    }

    fn on() -> Setup {
        Setup { register: enabled(), ..Setup::off() }
    }
}

/// A functional destination, with M's address 37 as the scratch word.
fn fd(code: u64) -> u64 {
    code << 19 | 0o37 << 14
}

/// The register's word for this program's main loop and bases, enabled.
fn enabled() -> u32 {
    macro_dispatch::word(QMLP as u16, LOCALP_AT as u16, AP_AT as u8)
}

/// The entry `specialised` sets: opcode 2 with register 5.
fn specialised_index() -> usize {
    (hw(2, 5) >> 6) as usize
}

/// The program's control store, dispatch memory, MACRO DISPATCH MEMORY
/// (the generic handlers, OPDTB's entry for each index's opcode), A
/// memory and map.
fn machine(s: Setup) -> Machine {
    let mut m = Machine::new();
    m.geometry = s.geometry;
    let mut prom = vec![filler(); 1024];
    let put = |prom: &mut Vec<Insn>, at: u64, w: u64| prom[at as usize] = Insn::new(w);
    // Start: MACRO-DISPATCH, LC, INTERRUPT-CONTROL (the sequence break),
    // the main loop's return pushed, `A-LOCALP` and `M-AP` written over
    // themselves after destination 5, as the microcode is to write them
    // (the register's write does not load the base copies), and a return
    // into the main loop.
    put(&mut prom, 0, ALU | SETA | a_src(0o51) | fd(5));
    put(&mut prom, 1, ALU | SETA | a_src(0o52) | fd(1));
    put(&mut prom, 2, ALU | SETA | a_src(0o53) | fd(2));
    put(&mut prom, 3, ALU | SETA | a_src(0o50) | fd(0o15));
    put(&mut prom, 4, ALU | SETA | a_src(LOCALP_AT) | a_dest(LOCALP_AT));
    put(&mut prom, 5, ALU | SETM | m_src(AP_AT) | m_dest(AP_AT) | POPJ);
    // The main loop.
    put(&mut prom, QMLP, JUMP | target(0o110) | P | 1 << 5 | 6);
    put(&mut prom, QMLP + 1, ALU | SETM | SRC_MD | m_dest(0o31));
    put(
        &mut prom,
        QMLP + 2,
        DISPATCH | m_src(0o31) | 3 << 10 | rot(op_rotate(s.geometry)) | d_len(5) | d_addr(OPDTB),
    );
    put(&mut prom, QMLP + 3, ALU | SETA | a_src(0o50) | fd(0o15));
    // Opcode 5's entry falls through (R and P): it lands here after the
    // push.
    put(&mut prom, QMLP + 4, ALU | M_PLUS_C | CARRY_IN | m_src(5) | m_dest(5) | POPJ);
    // Condition 6's call, counted in M 10: back to QMLP + 1.
    put(&mut prom, 0o110, ALU | M_PLUS_C | CARRY_IN | m_src(0o10) | m_dest(0o10) | POPJ);
    // Opcode 1: count in M 1 and return by a POPJ, the one `Pop` changes.
    // A return runs the microinstruction after it first, a filler.
    let pop = match s.pop {
        Pop::Plain => ALU | SETA | a_src(3) | m_dest(0o36),
        Pop::WritesM31 => ALU | SETM | m_src(0o31) | m_dest(0o31),
        Pop::Pushes => ALU | SETA | a_src(0o50) | fd(0o15),
        Pop::WritesInterruptControl => ALU | SETA | a_src(0o53) | fd(2),
    };
    put(&mut prom, 0o204, ALU | M_PLUS_C | CARRY_IN | m_src(1) | m_dest(1));
    put(&mut prom, 0o205, pop | POPJ);
    // Opcode 2: count in M 2 and return by a jump with R.
    put(&mut prom, 0o210, ALU | M_PLUS_C | CARRY_IN | m_src(2) | m_dest(2));
    put(&mut prom, 0o211, JUMP | R | ALWAYS);
    // Opcode 3: count in M 3 and return by a dispatch whose entry has R.
    put(&mut prom, 0o214, ALU | M_PLUS_C | CARRY_IN | m_src(3) | m_dest(3));
    put(&mut prom, 0o215, DISPATCH | m_src(2) | d_len(1) | d_addr(RETURNS));
    // Opcode 4, its entry with N: the main loop's push is not run, so it
    // pushes the return back itself before its POPJ.
    put(&mut prom, 0o220, ALU | M_PLUS_C | CARRY_IN | m_src(4) | m_dest(4));
    put(&mut prom, 0o221, ALU | SETA | a_src(0o50) | fd(0o15));
    put(&mut prom, 0o222, filler().raw() | POPJ);
    // Opcode 6's entry pushes and jumps (P and N): drop the pushed word,
    // count, and put the main loop's return back.
    put(&mut prom, 0o120, ALU | SETM | src(0o14) | m_dest(0o20));
    put(&mut prom, 0o121, ALU | M_PLUS_C | CARRY_IN | m_src(6) | m_dest(6));
    put(&mut prom, 0o122, ALU | SETA | a_src(0o50) | fd(0o15));
    put(&mut prom, 0o124, filler().raw() | POPJ);
    // The specialised handler: count in M 7 and return.
    put(&mut prom, SPECIAL, ALU | M_PLUS_C | CARRY_IN | m_src(7) | m_dest(7) | POPJ);
    // RECORD: push PDL-INDEX (functional source 3, destination 11), set it
    // to the sentinel (destination 13), and return, pushing it again in
    // the microcycle after the POPJ unless the slot says otherwise.
    let push_index = ALU | SETM | src(3) | fd(0o11);
    put(&mut prom, RECORD_AT, s.first.unwrap_or(push_index));
    put(&mut prom, RECORD_AT + 1, ALU | SETA | a_src(SENTINEL_AT) | fd(0o13));
    put(&mut prom, RECORD_AT + 2, filler().raw() | POPJ);
    put(&mut prom, RECORD_SLOT, s.slot.unwrap_or(push_index));
    // SETAP: `M-AP` + 1, in the POPJ itself.
    put(&mut prom, SETAP_AT, ALU | M_PLUS_C | CARRY_IN | m_src(AP_AT) | m_dest(AP_AT) | POPJ);
    // SETLOCALP: `A-LOCALP` from A memory 56, in the POPJ itself.
    put(&mut prom, SETLOCALP_AT, ALU | SETA | a_src(0o56) | a_dest(LOCALP_AT) | POPJ);
    // Opcode 7 stops: a jump to itself.
    put(&mut prom, STOP as u64, JUMP | target(STOP as u64) | ALWAYS | N);
    m.load_prom(&prom);
    support::prom_program_in_ram(&mut m);

    // Dispatch memory's words: `<16>` R, `<15>` P, `<14>` N, `<13:0>` the
    // address.
    m.dmem[OPDTB as usize + 1] = 0o204;
    m.dmem[OPDTB as usize + 2] = 0o210;
    m.dmem[OPDTB as usize + 3] = 0o214;
    m.dmem[OPDTB as usize + 4] = 1 << 14 | 0o220;
    m.dmem[OPDTB as usize + 5] = 1 << 16 | 1 << 15;
    m.dmem[OPDTB as usize + 6] = 1 << 15 | 1 << 14 | 0o120;
    m.dmem[OPDTB as usize + 7] = STOP as u32;
    m.dmem[OPDTB as usize + RECORD as usize] = RECORD_AT as u32;
    m.dmem[OPDTB as usize + SETAP as usize] = SETAP_AT as u32;
    m.dmem[OPDTB as usize + SETLOCALP as usize] = SETLOCALP_AT as u32;
    m.dmem[RETURNS as usize] = 1 << 16;
    m.dmem[RETURNS as usize + 1] = 1 << 16;
    // The MACRO DISPATCH MEMORY: every index OPDTB's entry for its opcode,
    // `<13:9>` of the halfword being the index's `<7:3>`.
    for (k, e) in m.macro_dispatch.entries.iter_mut().enumerate() {
        *e = m.dmem[OPDTB as usize + (k >> 3 & 0o37)];
    }
    if s.specialised {
        m.macro_dispatch.entries[specialised_index()] = SPECIAL as u32;
    }
    if s.operand {
        for (k, e) in m.macro_dispatch.entries.iter_mut().enumerate() {
            if k >> 3 & 0o37 == RECORD as usize {
                *e |= macro_dispatch::OPERAND;
            }
        }
    }

    m.amem[0o50] = u64::from(MAIN);
    m.amem[0o51] = u64::from(s.register);
    m.amem[0o52] = u64::from(s.code * 4);
    // `SEQUENCE.BREAK`: `<26>` on the CADR, `<34>` on QUUX (contract G2
    // appendix A1.6).
    let sequence_break = if s.geometry.wide() { 1 << 34 } else { 1 << 26 };
    m.amem[0o53] = if s.sequence_break { sequence_break } else { 0 };
    m.amem[SENTINEL_AT as usize] = u64::from(SENTINEL);
    m.amem[LOCALP_AT as usize] = u64::from(LOCALP);
    m.amem[0o56] = u64::from(LOCALP_2);
    m.mmem[AP_AT as usize] = u64::from(AP);
    m.amem[AP_AT as usize] = u64::from(AP);
    m.pdl_index = SENTINEL as u16;
    // QUUX's virtual pages 0 and 1, of 1024 words, onto physical pages 0
    // and 1 (contract G2 §2.6); the CADR's 1 and 2, of 256.
    if s.geometry.wide() {
        let rw = (1 << 27) | (1 << 26);
        m.l2_map[0] = rw;
        m.l2_map[1] = rw | 1;
    } else {
        let rw = (1 << 23) | (1 << 22);
        m.l2_map[1] = rw | 1;
        m.l2_map[2] = rw | 2;
    }
    for (k, pair) in s.program.chunks(2).enumerate() {
        m.main[s.code as usize + k] = u64::from(pair[0] | pair[1] << 16);
    }
    if let Some(patch) = s.patch {
        patch(&mut m);
    }
    m
}

/// Runs an engine until opcode 7's handler has run, and gives the machine
/// back with the microcycles it took. The machine is taken eight
/// microcycles later, in opcode 7's loop, which lets what the last
/// microcycles started land: a fetch a fused return on `rtl`'s fetch path
/// started, into MD, and the write of the microcycle after a return into
/// opcode 7, into the PDL buffer.
fn run<E: Engine>(mut e: E) -> (u64, Machine) {
    e.boot();
    for n in 0..20_000 {
        if e.machine().opc == STOP {
            e.run(8);
            return (n, e.machine().clone());
        }
        e.step().unwrap();
    }
    panic!("the program never reached its end");
}

fn both(s: Setup) -> [(&'static str, u64, Machine); 2] {
    let (n, m) = run(Micro::new(machine(s)));
    let (rn, rm) = run(Rtl::new(machine(s)));
    [("micro", n, m), ("rtl", rn, rm)]
}

/// The counts of opcodes 1 to 6, and M 7, the specialised handler's.
fn counts(m: &Machine) -> [u32; 7] {
    std::array::from_fn(|k| support::low(m.mmem[1 + k]))
}

/// The architectural state a program leaves: M and A memory, the SPC
/// stack and its pointer, the location counter, the PDL's pointer and
/// index, Q, VMA, MD and the dispatch constant. Not the words that differ
/// by the register's word alone: A 51, which holds it, and M 37, where
/// every functional destination here writes too.
fn state(m: &Machine) -> impl PartialEq + std::fmt::Debug {
    let mut mmem = m.mmem;
    let mut amem = m.amem;
    mmem[0o37] = 0;
    amem[0o37] = 0;
    amem[0o51] = 0;
    (
        mmem,
        amem.to_vec(),
        m.spc,
        m.spcptr,
        m.lc,
        (m.pdl_pointer, m.pdl_index, m.q, m.vma, m.md, m.dispatch_constant),
    )
}

/// How many returns fuse in [`PROGRAM`] with no fetch: a return into the
/// second halfword of a word, which needs no fetch, from a handler whose
/// return fuses (opcode 7's never returns), into an entry with R and P
/// clear (not 5 or 6). Both engines fuse these.
fn fusing(returns_fuse: impl Fn(u32) -> bool) -> u64 {
    let op = |h: u32| h >> 9 & 0o37;
    (1..PROGRAM.len())
        .filter(|&k| k % 2 == 1 && op(PROGRAM[k - 1]) != 7)
        .filter(|&k| returns_fuse(op(PROGRAM[k - 1])) && !matches!(op(PROGRAM[k]), 5 | 6))
        .count() as u64
}

/// **The returns into [`PROGRAM`] that fuse on the prefetched word**, by
/// halfword: into a word's first halfword, which needs a fetch, where the
/// word is not the first of its line of eight (the line the fetch before it
/// filled holds it, and the next line is not yet held), from a
/// handler whose return fuses into an entry with R and P clear. `rtl`
/// fuses these as well; `micro`, with no cache, does not.
fn fusing_prefetched(returns_fuse: impl Fn(u32) -> bool) -> Vec<usize> {
    let op = |h: u32| h >> 9 & 0o37;
    (2..PROGRAM.len())
        .step_by(2)
        .filter(|&k| !(CODE as usize + k / 2).is_multiple_of(8))
        .filter(|&k| op(PROGRAM[k - 1]) != 7 && returns_fuse(op(PROGRAM[k - 1])))
        .filter(|&k| !matches!(op(PROGRAM[k]), 5 | 6))
        .collect()
}

/// How many returns in [`PROGRAM`] fuse on `engine`: those
/// that need no fetch, and on `rtl` those its prefetch holds the word for.
fn fused_on(engine: &str, returns_fuse: impl Fn(u32) -> bool + Copy) -> u64 {
    let prefetched = if engine == "rtl" { fusing_prefetched(returns_fuse).len() } else { 0 };
    fusing(returns_fuse) + prefetched as u64
}

/// The microcycles the fused returns of [`fused_on`] save on `engine`: two
/// for one that needs no fetch (`QMLP+2` and `QMLP+3`), four for one on the
/// fetch path (`QMLP` to `QMLP+3`).
fn saved_on(engine: &str, returns_fuse: impl Fn(u32) -> bool + Copy) -> u64 {
    let prefetched = if engine == "rtl" { fusing_prefetched(returns_fuse).len() } else { 0 };
    2 * fusing(returns_fuse) + 4 * prefetched as u64
}

/// **Destinations 5 to 7 write the register, the index and the entry**
/// on QUUX, each as well as M, as every functional destination does: the
/// register keeps `<31>` and `<28:0>`, the index its ten bits and the
/// entry its eighteen. On the CADR they write only M.
#[test]
fn destinations_5_to_7_write_the_register_the_index_and_the_entry() {
    let prom = [
        Insn::new(ALU | SETA | a_src(0o51) | fd(5)),
        Insn::new(ALU | SETA | a_src(0o52) | fd(6)),
        Insn::new(ALU | SETA | a_src(0o53) | fd(7)),
        Insn::new(ALU | SETA | a_src(0o54) | fd(6)),
        Insn::new(ALU | SETA | a_src(0o55) | fd(7)),
    ];
    let program = |geometry: Geometry| {
        let mut m = Machine::new();
        m.geometry = geometry;
        m.load_prom(&prom);
        support::prom_program_in_ram(&mut m);
        m.amem[0o51] = 0xffff_ffff;
        m.amem[0o52] = 0o7771234;
        m.amem[0o53] = 0o7654321;
        m.amem[0o54] = 0o17;
        m.amem[0o55] = 0o1234567;
        m
    };
    for geometry in [Geometry::QUUX, Geometry::CADR] {
        let mut e = Micro::new(program(geometry));
        e.boot();
        e.run(12);
        let mut r = Rtl::new(program(geometry));
        r.boot();
        r.run(12);
        for (name, m) in [("micro", e.machine()), ("rtl", r.machine())] {
            let d = &m.macro_dispatch;
            let written = (d.register, d.index, d.entries[0o1234], d.entries[0o17]);
            if geometry.macro_dispatch {
                let want = (1 << 31 | 0o3777777777, 0o17, 0o654321, 0o234567);
                assert_eq!(written, want, "{geometry:?}, {name}");
            } else {
                assert_eq!(written, (0, 0, 0, 0), "{geometry:?}, {name}: nothing written");
            }
            assert_eq!(m.mmem[0o37], 0o1234567, "{geometry:?}, {name}: M written too");
        }
    }
}

/// **A fused return skips the main loop's dispatch and push**, on both
/// engines: the program leaves the same state with the MACRO DISPATCH
/// MEMORY holding the generic handlers as without it, two microcycles
/// fewer for each fused return that needs no fetch, and on `rtl` four
/// fewer for each on the fetch path, which its prefetch fuses. A POPJ, a
/// jump with R (while [`macro_dispatch::JUMP_RETURNS_FUSE`] says so) and a
/// dispatch whose entry has R all fuse; an entry with N fuses and pops the
/// word, as the main loop's nopped push would have left it popped.
#[test]
fn a_fused_return_skips_the_dispatch_and_the_push() {
    let off = both(Setup::off());
    let on = both(Setup::on());
    let fused = fusing(fuses);
    assert!(fused >= 6, "the program fuses {fused}");
    for ((name, n_off, m_off), (_, n_on, m_on)) in off.iter().zip(on.iter()) {
        assert_eq!(counts(m_off)[..6], COUNTS, "{name}: every macroinstruction ran once");
        assert_eq!(state(m_on), state(m_off), "{name}: the same state");
        assert_eq!(m_off.macro_dispatch.fused, 0, "{name}: nothing fused while disabled");
        assert_eq!(m_on.macro_dispatch.fused, fused_on(name, fuses), "{name}");
        assert_eq!(n_off - n_on, saved_on(name, fuses), "{name}");
    }
    // The engines leave the same state, `rtl` in fewer microcycles. `rtl`
    // keeps the location counter in its own counters, not in the machine.
    let (mut micro, mut rtl) = (on[0].2.clone(), on[1].2.clone());
    (micro.lc, rtl.lc) = (0, 0);
    assert_eq!(state(&micro), state(&rtl), "micro and rtl");
    assert!(on[1].1 < on[0].1, "rtl fuses the returns micro does and more");
}

/// **A dispatch's R return fuses** (contract H8a): the returns from
/// opcode 3, whose handler returns through a dispatch entry with R, are
/// among those fused, which a POPJ-only fused return fails.
#[test]
fn a_dispatch_return_fuses() {
    let without_3 = fusing(|op| op != 3 && fuses(op));
    assert!(fusing(fuses) > without_3, "the program has dispatch returns to fuse");
    for (name, _, m) in both(Setup::on()) {
        assert_eq!(counts(&m)[2], COUNTS[2], "{name}");
        assert_eq!(m.macro_dispatch.fused, fused_on(name, fuses), "{name}");
    }
}

/// **The fused return takes the MACRO DISPATCH MEMORY's entry**, indexed
/// by the whole `<15:6>` and not by the opcode alone: with opcode 2 and
/// register 5 given a handler of its own, its fused occurrences run it and
/// every other occurrence of opcode 2 the generic one.
#[test]
fn the_entry_for_the_halfword_is_taken() {
    // Opcode 2 with register 5 in a word's second halfword, after a
    // handler whose return fuses; and on `rtl`, in a first halfword whose
    // return fuses on the prefetched word (none here: the one first
    // halfword that has it starts a line).
    let special: u64 =
        (1..PROGRAM.len()).filter(|&k| k % 2 == 1 && PROGRAM[k] == hw(2, 5)).count() as u64;
    let prefetched =
        fusing_prefetched(fuses).into_iter().filter(|&k| PROGRAM[k] == hw(2, 5)).count() as u64;
    assert!(special >= 2, "the program has the halfword where it fuses");
    for (name, _, m) in both(Setup { specialised: true, ..Setup::on() }) {
        let c = counts(&m);
        let special = special + if name == "rtl" { prefetched } else { 0 };
        assert_eq!(c[6] as u64, special, "{name}: the specialised handler");
        assert_eq!(c[1] + c[6], COUNTS[1], "{name}: opcode 2 ran as often");
        assert_eq!(c[..1], COUNTS[..1], "{name}");
        assert_eq!(c[2..6], COUNTS[2..6], "{name}");
    }
    // Disabled, the entry is never read.
    for (name, _, m) in both(Setup { specialised: true, ..Setup::off() }) {
        assert_eq!(counts(&m)[6], 0, "{name}");
    }
}

/// **Each case that is not fused runs today's path**: a return that pops
/// the main loop's word while its own microinstruction writes M 31,
/// pushes, or writes INTERRUPT-CONTROL is not fused, on the fetch path
/// either, and the run is the disabled one's, microcycle for microcycle,
/// less what each return that still fuses saves. The needed fetch and the
/// entries with R or P are in every run: [`fused_on`] counts them out.
#[test]
fn a_return_that_writes_m31_pushes_or_writes_interrupt_control_is_not_fused() {
    for pop in [Pop::WritesM31, Pop::Pushes, Pop::WritesInterruptControl] {
        let off = both(Setup { pop, ..Setup::off() });
        let on = both(Setup { pop, ..Setup::on() });
        let not_1 = |op| op != 1 && fuses(op);
        for ((name, n_off, m_off), (_, n_on, m_on)) in off.iter().zip(on.iter()) {
            assert_eq!(counts(m_off)[..6], COUNTS, "{pop:?}, {name}");
            assert_eq!(state(m_on), state(m_off), "{pop:?}, {name}: the same state");
            assert_eq!(m_on.macro_dispatch.fused, fused_on(name, not_1), "{pop:?}, {name}");
            assert_eq!(n_off - n_on, saved_on(name, not_1), "{pop:?}, {name}");
        }
    }
}

/// **Condition 6 is tested on the fetch path only** (contract H8a):
/// with the sequence break up, the main loop's call on condition 6 is
/// taken at every fetch with the fused return as without it, and the
/// returns that need no fetch fuse all the same, as today's main loop goes
/// to `QMLP+2` for them without testing it. So `rtl`'s prefetch fuses
/// none here, and the engines fuse the same returns.
#[test]
fn condition_6_is_tested_on_the_fetch_path() {
    let off = both(Setup { sequence_break: true, ..Setup::off() });
    let on = both(Setup { sequence_break: true, ..Setup::on() });
    let fetches = PROGRAM.len() as u32 / 2;
    for ((name, n_off, m_off), (_, n_on, m_on)) in off.iter().zip(on.iter()) {
        assert_eq!(counts(m_off)[..6], COUNTS, "{name}");
        assert_eq!(m_off.mmem[0o10], fetches.into(), "{name}: the call at every fetch");
        assert_eq!(state(m_on), state(m_off), "{name}");
        assert_eq!(m_on.macro_dispatch.fused, fusing(fuses), "{name}");
        assert_eq!(n_off - n_on, 2 * fusing(fuses), "{name}");
    }
}

/// **A return to another address is not fused**: the register naming
/// another main loop, as `DMLP`'s word is to `QMLP`'s, changes nothing.
#[test]
fn another_main_loop_is_not_fused() {
    let other = macro_dispatch::word(QMLP as u16 + 4, 0, 0);
    let off = both(Setup::off());
    let on = both(Setup { register: other, ..Setup::off() });
    for ((name, n_off, m_off), (_, n_on, m_on)) in off.iter().zip(on.iter()) {
        assert_eq!(state(m_on), state(m_off), "{name}");
        assert_eq!(m_on.macro_dispatch.fused, 0, "{name}");
        assert_eq!(n_on, n_off, "{name}");
    }
}

/// **The CADR has none of it**: the register written with the enable
/// changes nothing there, destination 5 writing only M, and `rtl` has no
/// prefetch.
#[test]
fn the_cadr_has_none_of_it() {
    let geometry = Geometry::CADR;
    let r = Rtl::new(machine(Setup { geometry, ..Setup::on() }));
    assert_eq!(r.prefetch(), None);
    for (name, _, m_on) in both(Setup { geometry, ..Setup::on() }) {
        assert_eq!(counts(&m_on)[..6], COUNTS, "{name}");
        assert_eq!(m_on.macro_dispatch.register, 0, "{name}");
        assert_eq!(m_on.macro_dispatch.fused, 0, "{name}");
    }
}

/// **-RESET clears the enable and keeps the rest**: the index, the
/// entries, the register's other bits and the base copies, on both
/// engines.
#[test]
fn reset_clears_the_enable_only() {
    let check = |name: &str, m: &Machine, entries: &[u32]| {
        let d = &m.macro_dispatch;
        assert_eq!(d.register, enabled() & !macro_dispatch::ENABLE, "{name}: the enable cleared");
        assert_eq!(d.index, 0o17, "{name}: the index kept");
        assert_eq!(d.entries, entries, "{name}: the entries kept");
        assert_eq!((d.localp, d.ap), (LOCALP, AP), "{name}: the base copies kept");
    };
    let mut m = machine(Setup::on());
    m.macro_dispatch.index = 0o17;
    let entries = m.macro_dispatch.entries.clone();
    let mut e = Micro::new(m.clone());
    e.boot();
    e.run(10);
    assert_eq!(e.machine().macro_dispatch.register, enabled(), "micro: written by destination 5");
    e.machine_mut().prog_reset = true;
    e.run(2);
    check("micro", e.machine(), &entries);
    let mut r = Rtl::new(m);
    r.boot();
    r.run(10);
    assert_eq!(r.machine().macro_dispatch.register, enabled(), "rtl: written by destination 5");
    r.machine_mut().prog_reset = true;
    r.run(2);
    check("rtl", r.machine(), &entries);
}

/// **A control-store write clears the enable**, wherever it lands, and
/// keeps the rest: a new microcode never runs on the entries the old one
/// left (contract H8a §3.6). The register is enabled by destination 5, and
/// a `WRITE-I-MEM` --- a jump with P and R --- follows.
#[test]
fn a_control_store_write_clears_the_enable() {
    let prom = [
        Insn::new(ALU | SETA | a_src(0o51) | fd(5)),
        Insn::new(filler().raw()),
        Insn::new(filler().raw()),
        Insn::new(JUMP | target(0o700) | P | R | ALWAYS | m_src(1) | a_src(0o52)),
        Insn::new(filler().raw()),
        Insn::new(filler().raw()),
    ];
    let program = || {
        let mut m = Machine::new();
        m.geometry = Geometry::QUUX;
        m.load_prom(&prom);
        support::prom_program_in_ram(&mut m);
        m.amem[0o51] = u64::from(enabled());
        m.mmem[1] = 0o1234;
        m.amem[0o52] = 0o5670;
        m.macro_dispatch.entries[5] = 0o4321;
        m
    };
    fn check<E: Engine>(name: &str, mut e: E) {
        e.boot();
        let mut n = 0;
        while e.machine().macro_dispatch.register != enabled() {
            e.step().unwrap();
            n += 1;
            assert!(n < 20, "{name}: destination 5 never written");
        }
        e.run(8);
        let m = e.machine();
        assert_eq!(m.imem[0o700].raw(), 0o5670 << 32 | 0o1234, "{name}: the word written");
        let d = &m.macro_dispatch;
        assert_eq!(d.register, enabled() & !macro_dispatch::ENABLE, "{name}: the enable cleared");
        assert_eq!(d.entries[5], 0o4321, "{name}: the entries kept");
    }
    check("micro", Micro::new(program()));
    check("rtl", Rtl::new(program()));
}

/// **A checkpoint keeps the register, the index and the entries**, the
/// base copies as they stand, and an armed operand address (contract H8a
/// §3.6): nothing is loaded from A and M memory at restore. The count of
/// fused returns is not the machine's.
#[test]
fn a_checkpoint_keeps_the_register_and_the_memory() {
    use muir::checkpoint::{Reader, Writer};
    let geometry = Geometry::QUUX;
    let mut m = machine(Setup { geometry, ..Setup::on() });
    m.macro_dispatch.register = enabled();
    m.macro_dispatch.index = 0o1001;
    m.macro_dispatch.entries[0o1777] = 0o777777;
    m.macro_dispatch.fused = 5;
    // An operand address armed, and base copies that A and M memory do not
    // hold, as they stand after destination 5 and before the microcode
    // writes `A-LOCALP` and `M-AP`: the file carries both.
    m.macro_dispatch.operand = Some(Operand { arg: true, delta: 0o52 });
    // And a prefetched word armed for M 31 (the prefetch's (a)).
    m.macro_dispatch.m31 = Some(0o12345670123);
    m.macro_dispatch.localp = 1;
    m.macro_dispatch.ap = 2;
    let mut w = Writer::new();
    m.save(&mut w);
    let body = w.finish();
    let mut back = Machine::new();
    back.load(&mut Reader::for_word_bits(&body, 40)).unwrap();
    assert_eq!(back.geometry, geometry);
    assert_eq!(back.macro_dispatch.register, enabled());
    assert_eq!(back.macro_dispatch.index, 0o1001);
    assert_eq!(back.macro_dispatch.entries, m.macro_dispatch.entries);
    assert_eq!(back.macro_dispatch.fused, 0);
    assert_eq!(Machine::checkpointed_geometry_at(&body, 40).unwrap(), geometry);
    assert_eq!(back.macro_dispatch.operand, m.macro_dispatch.operand);
    assert_eq!(back.macro_dispatch.m31, Some(0o12345670123));
    assert_eq!(
        (back.macro_dispatch.localp, back.macro_dispatch.ap),
        (1, 2),
        "the base copies kept, not loaded from A and M memory"
    );
}

/// **Feature word 17 says the MACRO DISPATCH MEMORY's 1,024 entries**;
/// the CADR has no feature page.
#[test]
fn feature_word_17_says_so() {
    let word_17 = muir::machine::REGISTER_PAGE_13 | 0o17;
    assert_eq!(Geometry::QUUX.feature_word(word_17), Some(1024));
    assert_eq!(Geometry::CADR.feature_word(word_17), None);
}

/// **A jump with R fuses exactly as [`macro_dispatch::JUMP_RETURNS_FUSE`]
/// says**, on both engines: the returns from opcode 2, whose handler
/// returns by a jump with R, are counted among the fused only while it is
/// set. The one switch reaches both engines.
#[test]
fn a_jump_return_fuses_as_the_switch_says() {
    let jumps = fusing(|_| true) - fusing(|op| op != 2);
    assert!(jumps > 0, "the program has jump returns to fuse");
    for (name, _, m) in both(Setup::on()) {
        let jumps = fused_on(name, |_| true) - fused_on(name, |op| op != 2);
        let all = fused_on(name, |_| true);
        let want = all - if macro_dispatch::JUMP_RETURNS_FUSE { 0 } else { jumps };
        assert_eq!(m.macro_dispatch.fused, want, "{name}");
    }
}

/// [`RECORD`]'s pushes over a run: the PDL buffer from word 1 up, as its
/// pointer has counted them.
fn records(m: &Machine) -> Vec<u32> {
    m.pdl[1..=m.pdl_pointer as usize].iter().map(|&w| support::low(w)).collect()
}

/// [`RECORD`]'s returns over [`OPERANDS`] that fuse on `engine`, and of
/// them those into [`RECORD`] again. On `micro` the four into second
/// halfwords, each into [`RECORD`]. On `rtl` six more on the fetch path,
/// its prefetch holding the second to the seventh words, all in the first
/// word's line of eight: of those, the second's, the fourth's and the
/// fifth's first halfwords are [`RECORD`].
fn record_returns(engine: &str) -> (u64, u64) {
    if engine == "rtl" { (4 + 6, 4 + 3) } else { (4, 4) }
}

/// **The operand address** (contract H8a §3.4, §6 item 4), on both engines:
/// a fused return whose entry has the operand bit loads PDL-INDEX with
/// `A-LOCALP` + delta for LOCAL and `M-AP` + 1 + delta for ARG, masked to
/// its fourteen bits, at the end of the microcycle after the return --- the
/// handler's first microinstruction finds it there, and the microcycle
/// after does not. The bases are the copies the register's write loaded
/// from A and M memory, and `M-AP` or `A-LOCALP` written by the popping
/// microinstruction itself is the new one. A register other than LOCAL and ARG, and a
/// return that is not fused, load nothing; and the run takes the same
/// microcycles as without the operand bit. On `rtl` the returns into the
/// first halfwords of the second to the seventh words fuse too, on the
/// prefetched word (all seven are in one line of eight), and two of them,
/// ARG and LOCAL, load.
#[test]
fn a_fused_return_loads_the_operand_address() {
    let plain = both(Setup { operand: false, ..Setup::operands() });
    for ((name, n, m), (_, n_plain, _)) in both(Setup::operands()).iter().zip(plain.iter()) {
        assert_eq!(records(m), operand_records(name), "{name}");
        let fused = if *name == "rtl" { 6 + 6 } else { 6 };
        assert_eq!(m.macro_dispatch.fused, fused, "{name}: the six second halfwords");
        assert_eq!(n, n_plain, "{name}: no microcycle more or less");
    }
}

/// **Without the operand bit PDL-INDEX is left alone**, on both engines:
/// with the same fused returns, every push finds the sentinel, as it does
/// with the MACRO DISPATCH MEMORY disabled.
#[test]
fn without_the_operand_bit_pdl_index_is_unchanged() {
    let sentinels = vec![SENTINEL; operand_records("micro").len()];
    for setup in
        [Setup { operand: false, ..Setup::operands() }, Setup { register: 0, ..Setup::operands() }]
    {
        for (name, _, m) in both(setup) {
            assert_eq!(records(&m), sentinels, "{name}");
        }
    }
}

/// Runs `s` on both engines under the checker, to opcode 7.
fn checked(s: Setup) -> [(&'static str, Counts); 2] {
    fn go<E: Executes>(e: E) -> Counts {
        let (_, counts) = run_checked(Checked::new(e));
        counts
    }
    [("micro", go(Micro::new(machine(s)))), ("rtl", go(Rtl::new(machine(s))))]
}

fn run_checked<E: Executes>(mut e: Checked<E>) -> (u64, Counts) {
    e.boot();
    for n in 0..20_000 {
        if e.machine().opc == STOP {
            return (n, e.checker.counts.clone());
        }
        e.step().unwrap();
    }
    panic!("the program never reached its end");
}

/// **The checkers find these programs clean** (contract H8a §6 items 4 and
/// 5), on both engines: after every fused return the handler the main loop
/// would have dispatched to runs next, the micro stack is where the return
/// left it, PDL-INDEX is the operand address where one is armed, and the
/// base copies equal `A-LOCALP` and `M-AP` after every microcycle, `M-AP`
/// stepping under them. `rtl` fuses more, on the prefetched word
/// ([`a_fused_return_loads_the_operand_address`] for the operands).
#[test]
fn the_checkers_find_the_programs_clean() {
    for (s, fused, loads) in [
        (Setup::on(), [fused_on("micro", fuses), fused_on("rtl", fuses)], [0, 0]),
        (Setup::operands(), [6, 6 + 6], [5, 7]),
        (Setup::off(), [0, 0], [0, 0]),
    ] {
        for (k, (name, c)) in checked(s).into_iter().enumerate() {
            assert_eq!(c.problems(), 0, "{name}: {}", c.report(|_| String::new()));
            assert_eq!(c.fused, fused[k], "{name}");
            assert_eq!(c.operand_loads, loads[k], "{name}");
            assert!(c.unarmed.is_empty(), "{name}");
        }
    }
}

/// **The rule checker's decode** (contract H8a §3.3): each write the
/// microcycle after a fused return may not make, and none where there is
/// none. M 31 and the location counter are written by an M destination
/// or functional destination 1, `A-LOCALP` by an A destination at the
/// register's `<23:14>` (or an M one, where that is below 40), `M-AP` by
/// an M destination at its `<28:24>` and not by an A destination there.
#[test]
fn the_rule_checker_decodes_each_forbidden_write() {
    let reg = enabled();
    let i = |w: u64| Insn::new(w);
    let cases: [(u64, &[Write]); 14] = [
        (filler().raw(), &[]),
        (ALU | SETA | a_src(3) | fd(1), &[Write::Lc]),
        (ALU | SETA | a_src(3) | m_dest(0o31), &[Write::M31]),
        (ALU | SETA | a_src(3) | a_dest(0o31), &[]),
        (ALU | SETA | a_src(3) | fd(2), &[Write::InterruptControl]),
        (ALU | SETA | a_src(3) | fd(5), &[Write::MacroDispatch]),
        (ALU | SETA | a_src(3) | fd(6), &[Write::MacroDispatch]),
        (ALU | SETA | a_src(3) | fd(7), &[Write::MacroDispatch]),
        (ALU | SETA | a_src(3) | fd(0o13), &[Write::PdlIndex]),
        (ALU | SETA | a_src(3) | fd(0o12), &[Write::PdlAtIndex]),
        (ALU | SETA | a_src(3) | a_dest(LOCALP_AT), &[Write::Localp]),
        (ALU | SETA | a_src(3) | m_dest(AP_AT), &[Write::Ap]),
        (ALU | SETA | a_src(3) | a_dest(AP_AT), &[]),
        (JUMP | target(0o100) | ALWAYS | m_dest(0o31) | fd(1), &[]),
    ];
    for (w, want) in cases {
        assert_eq!(writes(i(w), reg), want, "{w:o}");
    }
    // A-LOCALP below 40 is written by an M destination too.
    let low = macro_dispatch::word(QMLP as u16, 0o25, AP_AT as u8);
    assert_eq!(writes(i(ALU | SETA | a_src(3) | m_dest(0o25)), low), [Write::Localp]);
    // A BYTE instruction writes as an ALU one does.
    assert_eq!(
        writes(i(muir::isa::asm::BYTE | a_src(3) | 1 << 19 | m_dest(0o31)), reg),
        [Write::Lc, Write::M31]
    );
}

/// **The rule checker finds each forbidden write in a run** (contract H8a
/// §3.3, §6 item 5), on both engines: with the microcycle after
/// [`RECORD`]'s return writing INTERRUPT-CONTROL, the MACRO-DISPATCH
/// register, PDL-INDEX, the PDL buffer at PDL-INDEX, `A-LOCALP` or `M-AP`
/// --- each with what it held, or the sentinel, so
/// that the program runs on --- every one of the fused returns from that
/// handler is a violation of that kind, four on `micro` and nine on `rtl`
/// ([`record_returns`]); the last three are allowed, and counted apart,
/// where the entry has no operand bit: without it, and on `rtl` in the
/// returns into the handlers that are not [`RECORD`].
#[test]
fn the_rule_checker_finds_each_forbidden_write() {
    let cases = [
        (ALU | SETA | a_src(0o53) | fd(2), Write::InterruptControl),
        (ALU | SETA | a_src(0o51) | fd(5), Write::MacroDispatch),
        (ALU | SETA | a_src(SENTINEL_AT) | fd(0o13), Write::PdlIndex),
        (ALU | SETA | a_src(SENTINEL_AT) | fd(0o12), Write::PdlAtIndex),
        (ALU | SETA | a_src(LOCALP_AT) | a_dest(LOCALP_AT), Write::Localp),
        (ALU | SETM | m_src(AP_AT) | m_dest(AP_AT), Write::Ap),
    ];
    for (slot, kind) in cases {
        for operand in [true, false] {
            let s = Setup { slot: Some(slot), operand, ..Setup::operands() };
            for (name, c) in checked(s) {
                let (returns, into_record) = record_returns(name);
                let (forbidden, allowed) = if !kind.operand_only() {
                    (returns, 0)
                } else if operand {
                    (into_record, returns - into_record)
                } else {
                    (0, returns)
                };
                let want = (kind, RECORD_AT as u16 + 2, RECORD_SLOT as u16);
                let at = |n: u64| if n == 0 { vec![] } else { vec![(want, n)] };
                let what = format!("{kind:?}, operand bit {operand}, {name}");
                assert_eq!(
                    c.violations.clone().into_iter().collect::<Vec<_>>(),
                    at(forbidden),
                    "{what}"
                );
                assert_eq!(
                    c.unarmed.clone().into_iter().collect::<Vec<_>>(),
                    at(allowed),
                    "{what}"
                );
                assert_eq!(c.problems() - c.violations.values().sum::<u64>(), 0, "{name}");
            }
        }
    }
}

/// **A PDL buffer write by PDL-INDEX in the microcycle after a fused return
/// lands at the operand address**, on both engines alike: the word is
/// written in the next microcycle's write phase at PDL-INDEX as it then
/// stands (`PWIDX`), and the operand address was loaded at the edge before.
/// This is why the rule of §3.3 forbids it when the entry has the operand
/// bit, as `QSTLOC` and `QSTARG` (`uc-macrocode.lisp:327-334`) do it: the
/// store to a local would go to the next instruction's operand instead.
/// Without the operand bit it lands where it was aimed. On `rtl` the
/// return into the second word's first halfword, ARG with delta 0, fuses
/// too, on the prefetched word: its handler finds PDL-INDEX at `M-AP` + 1
/// and pushes it, and the write lands there. The engines differ in those
/// two words, and only there, the handler observing PDL-INDEX and breaking
/// the rule.
#[test]
fn a_pdl_write_by_index_after_a_fused_return_lands_at_the_operand_address() {
    let mark = 0o525252;
    let slot = ALU | SETA | a_src(0o55) | fd(0o12);
    let loaded = [(LOCALP + 7) & 0o37777, AP + 1 + 5, (LOCALP + 0o77) & 0o37777];
    let prefetched = AP + 1;
    for operand in [true, false] {
        let s = Setup { slot: Some(slot), operand, ..Setup::operands() };
        // The mark in A memory 55, which `machine` leaves zero.
        let with_mark = |mut m: Machine| {
            m.amem[0o55] = mark;
            m
        };
        let (_, micro) = run(Micro::new(with_mark(machine(s))));
        let (_, rtl) = run(Rtl::new(with_mark(machine(s))));
        let want = if operand { mark } else { 0 };
        for (name, m) in [("micro", &micro), ("rtl", &rtl)] {
            for at in loaded {
                assert_eq!(m.pdl[at as usize], want, "operand bit {operand}, {name}: PDL {at:o}");
            }
            assert_eq!(m.pdl[SENTINEL as usize], mark, "operand bit {operand}, {name}");
        }
        assert_eq!(micro.pdl[prefetched as usize], 0, "operand bit {operand}");
        assert_eq!(rtl.pdl[prefetched as usize], want, "operand bit {operand}");
        // The third push, the second word's first halfword's, and the
        // eighth, the fifth word's.
        for (at, loads) in [(3, AP + 1), (8, (LOCALP + 0o77) & 0o37777)] {
            let pushed = if operand { loads } else { SENTINEL };
            assert_eq!(
                (support::low(micro.pdl[at]), support::low(rtl.pdl[at])),
                (SENTINEL, pushed),
                "operand bit {operand}, push {at}"
            );
        }
        let (mut a, mut b) = (micro.pdl, rtl.pdl);
        for at in [prefetched as usize, 3, 8] {
            (a[at], b[at]) = (0, 0);
        }
        assert!(a == b, "operand bit {operand}: the engines agree elsewhere");
    }
}

/// **Destination 5 does not load the base copies** (contract H8a §3.4), on
/// both engines: the copies are written only by an A write at the
/// register's `<23:14>` and an M write at its `<28:24>`, with their write
/// pulse. After destination 5 names `A-LOCALP` and `M-AP`, whose memory
/// holds other words, the copies keep what they held, and an A write at
/// `M-AP`'s address number, which is not an M write, leaves the copy of
/// `M-AP`; the writes of `A-LOCALP` and `M-AP` that the microcode makes
/// after destination 5 then load them.
#[test]
fn destination_5_does_not_load_the_base_copies() {
    let (stale_localp, stale_ap) = (0o1111, 0o2222);
    let (new_localp, new_ap): (u32, u32) = (0o1234, 0o4321);
    let program = |and_write: bool| {
        let mut prom = vec![
            Insn::new(ALU | SETA | a_src(0o51) | fd(5)),
            Insn::new(filler().raw()),
            Insn::new(ALU | SETA | a_src(0o52) | a_dest(AP_AT)),
        ];
        if and_write {
            prom.push(Insn::new(ALU | SETA | a_src(0o53) | a_dest(LOCALP_AT)));
            prom.push(Insn::new(ALU | SETA | a_src(0o54) | m_dest(AP_AT)));
        }
        prom.extend([Insn::new(filler().raw()); 4]);
        let mut m = Machine::new();
        m.geometry = Geometry::QUUX;
        m.load_prom(&prom);
        support::prom_program_in_ram(&mut m);
        m.amem[0o51] = u64::from(enabled());
        m.amem[0o52] = 0o7777;
        m.amem[0o53] = new_localp.into();
        m.amem[0o54] = new_ap.into();
        m.amem[LOCALP_AT as usize] = u64::from(LOCALP);
        m.mmem[AP_AT as usize] = u64::from(AP);
        m.amem[AP_AT as usize] = u64::from(AP);
        m.macro_dispatch.localp = stale_localp;
        m.macro_dispatch.ap = stale_ap;
        m
    };
    for (and_write, want) in [(false, (stale_localp, stale_ap)), (true, (new_localp, new_ap))] {
        let mut e = Micro::new(program(and_write));
        e.boot();
        e.run(12);
        let mut r = Rtl::new(program(and_write));
        r.boot();
        r.run(12);
        for (name, m) in [("micro", e.machine()), ("rtl", r.machine())] {
            let d = &m.macro_dispatch;
            assert_eq!(d.register, enabled(), "{name}: destination 5 written");
            // The M write at `M-AP` writes the shadowing A word too.
            let a = if and_write { new_ap } else { 0o7777 };
            assert_eq!(support::low(m.amem[AP_AT as usize]), a, "{name}: the A write made");
            assert_eq!((d.localp, d.ap), want, "{name}, A-LOCALP and M-AP written {and_write}");
        }
    }
}

/// **The checker finds a PDL read, by a handler's first microinstruction,
/// at an address the microcycle after the return writes** (contract H8a
/// §3.3), on both engines. The PDL buffer has no pass-around: that write
/// lands in the handler's own write pulse, after its read, where today's
/// path runs the main loop's dispatch and push in between. With
/// [`RECORD`]'s handler reading the PDL buffer first, by the pointer or
/// by the index, and the microcycle after its return pushing, writing at
/// the pointer, or writing by PDL-INDEX, each fused return from that
/// handler into it is counted where the addresses meet (four on `micro`,
/// six on `rtl`, [`record_returns`]), with the operand bit and without; a
/// read at the other address is not.
#[test]
fn the_checker_finds_a_pdl_read_of_the_write_after_a_return() {
    let read_pointer = ALU | SETM | src(0o25) | m_dest(0o36);
    let read_index = ALU | SETM | src(0o5) | m_dest(0o36);
    let push = ALU | SETM | src(3) | fd(0o11);
    let at_pointer = ALU | SETA | a_src(SENTINEL_AT) | fd(0o10);
    let at_index = ALU | SETA | a_src(SENTINEL_AT) | fd(0o12);
    let cases = [
        (push, read_pointer, Some(PdlAt::Pointer)),
        (at_pointer, read_pointer, Some(PdlAt::Pointer)),
        (at_index, read_index, Some(PdlAt::Index)),
        (push, read_index, None),
        (at_index, read_pointer, None),
    ];
    for (slot, first, found) in cases {
        for operand in [true, false] {
            let s = Setup { slot: Some(slot), first: Some(first), operand, ..Setup::operands() };
            for (name, c) in checked(s) {
                let what = format!("{found:?}, operand bit {operand}, {name}");
                let (returns, into_record) = record_returns(name);
                assert_eq!(c.pdl_writes_after, returns, "{what}: every write seen");
                let sites: Vec<_> = c.pdl_reads_of_writes.iter().collect();
                match found {
                    Some(at) => {
                        let want = (at, RECORD_AT as u16 + 2, RECORD_SLOT as u16, RECORD_AT as u16);
                        assert_eq!(sites, [(&want, &into_record)], "{what}");
                    }
                    None => assert!(sites.is_empty(), "{what}"),
                }
                let violations = c.violations.values().sum::<u64>();
                let reads = c.pdl_reads_of_writes.values().sum::<u64>();
                assert_eq!(
                    c.problems(),
                    violations + reads,
                    "{what}: {}",
                    c.report(|_| String::new())
                );
            }
        }
    }
}

/// **The generic fill gives the operand bit only where the halfword's
/// `<8:0>` is a register and a delta**: CALL to ND3 and their twins, and
/// of ND4 (16, 36), whose sub-opcode is `<15:13>` (`M-INST-SUB-OPCODE`,
/// `uc-parameters.lisp`), only `PUSH-CDR-IF-CAR-EQUAL` (5), which fetches
/// its operand by `QADCM4`, and `PUSH-CDR-STORE-CAR-IF-CONS` (6), which
/// stores by `STOCYC`'s `QADCM2` (`uc-macrocode.lisp`, `D-ND4`). Not
/// `PUSH-NUMBER` (3), whose `<8:0>` is an immediate, the stack-closure
/// sub-opcodes 0, 1, 2 and 4, whose `<8:0>` is a local's offset
/// (`M-INST-ADR`), or 7, `ILLOP`; nor BRANCH, MISC, AREFI-NEW or the
/// unused codes.
#[test]
fn the_generic_fill_arms_only_a_register_and_a_delta() {
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
    fill_generic(&mut m, QMLP as u16, OPDTB as u16, LOCALP_AT as u16, AP_AT as u8, true);
    let armed = |half: u32| {
        m.macro_dispatch.entries[(half >> 6) as usize & 0o1777] & macro_dispatch::OPERAND != 0
    };
    for reg in 0..8 {
        for sub in 0..8u32 {
            // ND4's `<13:9>` is 16 with `<13>`, the sub-opcode's low bit,
            // clear, and 36 with it set.
            let half = sub << 13 | 0o16 << 9 | reg << 6;
            assert_eq!(armed(half), matches!(sub, 5 | 6), "ND4 sub-opcode {sub}, register {reg}");
        }
        for op in 0..0o40u32 {
            if op & 0o17 == 0o16 {
                continue;
            }
            let want = matches!(op, 0..=0o13 | 0o31..=0o33);
            for dest in 0..4 {
                let half = dest << 14 | op << 9 | reg << 6;
                assert_eq!(armed(half), want, "opcode {op:o}, register {reg}, dest {dest}");
            }
        }
    }
}

// --- The cache-only prefetch (contract H8a §3.5, `rtl` alone) ---

use muir::memory_port::{Drop, PrefetchCounts, Reach};

/// The prefetch's two reaches: the line's, a measurement's, and the
/// page's, QUUX's.
const FORMS: [Reach; 2] = [Reach::Line, Reach::Page];

/// A change made to the machine once, after the first microcycle that
/// executes an address.
type Touch = Option<(u16, fn(&mut Machine))>;

/// What a run on `rtl` leaves: its microcycles, the machine, the
/// prefetch's counts, the memory cycles it started, and the checkers'
/// counts.
struct Ran {
    n: u64,
    m: Machine,
    counts: PrefetchCounts,
    bus_cycles: u64,
    checked: Counts,
}

/// Runs `s` on `rtl` under the checkers, the prefetch fitted with the reach
/// `prefetch` gives or taken out, until opcode 7's handler has run, and then eight microcycles more
/// in its loop, which let a fetch the fused return started land in MD (a
/// handler that writes MD or starts a cycle waits for it, `-WAIT`'s
/// `DESTMEM AND MBUSY.SYNC`). The microcycles counted are those to the
/// handler. `touch`, when given, runs once after the first microcycle that
/// executes its address, with the machine.
fn run_prefetched(s: Setup, prefetch: Option<Reach>, touch: Touch) -> Ran {
    let mut r = Rtl::new(machine(s));
    r.set_prefetch(prefetch);
    let mut e = Checked::new(r);
    e.boot();
    let mut touch = touch;
    for n in 0..20_000 {
        if e.machine().opc == STOP {
            e.run(8);
            let r = &e.engine;
            return Ran {
                n,
                m: r.machine().clone(),
                counts: r.prefetch_counts().unwrap(),
                bus_cycles: r.bus_cycles(),
                checked: e.checker.counts.clone(),
            };
        }
        e.step().unwrap();
        if let Some((at, f)) = touch
            && e.engine.executed() == Some(at)
        {
            f(e.machine_mut());
            touch = None;
        }
    }
    panic!("the program never reached its end");
}

/// **With the prefetch, a return that needs the next word in sequence
/// fuses** when the buffer holds it: the program leaves the state it
/// leaves without the prefetch, four microcycles fewer for each such
/// return (`QMLP` to `QMLP+3`), with no memory cycle of the prefetch's
/// own, and the checkers find M 31 main memory's word after each. The
/// page's reach fuses the same here: the next line is in the cache on no
/// first pass. A fused return that follows a handler a fused return ran is
/// counted as that handler's own return; without the prefetch there is
/// none, the second halfword's successor always needing a fetch.
#[test]
fn the_prefetch_fuses_a_return_that_needs_a_fetch() {
    let off = run_prefetched(Setup::on(), None, None);
    assert_eq!(counts(&off.m)[..6], COUNTS);
    assert_eq!(off.checked.handler_returns.values().sum::<u64>(), 0, "none without it");
    for form in FORMS {
        let on = run_prefetched(Setup::on(), Some(form), None);
        let expected = fusing_prefetched(fuses);
        let k = expected.len() as u64;
        assert!(k >= 4, "{form:?}: the program fuses {k} on the fetch path");
        assert_eq!(state(&on.m), state(&off.m), "{form:?}: the same state");
        assert_eq!(on.m.macro_dispatch.fused, fusing(fuses) + k, "{form:?}");
        assert_eq!(off.n - on.n, 4 * k, "{form:?}: four microcycles each");
        assert_eq!(on.counts.used, k, "{form:?}");
        assert_eq!(on.bus_cycles, off.bus_cycles, "{form:?}: no memory cycle of its own");
        assert_eq!(on.checked.prefetched, k, "{form:?}");
        assert_eq!(on.checked.problems(), 0, "{form:?}: {:?}", on.checked.first);
        // A handler's own return: into a halfword a fused return ran,
        // from a handler a fused return ran.
        let op = |h: u32| h >> 9 & 0o37;
        let fused_into: Vec<usize> = (1..PROGRAM.len())
            .filter(|&j| j % 2 == 1 && op(PROGRAM[j - 1]) != 7)
            .filter(|&j| fuses(op(PROGRAM[j - 1])) && !matches!(op(PROGRAM[j]), 5 | 6))
            .chain(expected.iter().copied())
            .collect();
        let own = fused_into.iter().filter(|&&j| fused_into.contains(&(j - 1))).count() as u64;
        assert!(own >= k, "{form:?}");
        assert_eq!(on.checked.handler_returns.values().sum::<u64>(), own, "{form:?}");
        assert_eq!(on.counts.refused, [0, 0], "{form:?}");
    }
}

/// **The microcycle after the return reads the old M 31**, as it does on
/// the path the return skips: M 31 is a register beside M memory, loaded
/// with the prefetched word at the edge ending that microcycle. With that
/// microcycle of opcode 3's dispatch return adding M 31 into A 57, every
/// word is as without the prefetch; the checkers count those reads.
#[test]
fn the_microcycle_after_reads_the_old_m31() {
    fn slot_reads_m31(m: &mut Machine) {
        m.imem[0o216] =
            Insn::new(ALU | muir::isa::asm::ADD | m_src(0o31) | a_src(0o57) | a_dest(0o57));
    }
    let s = Setup { patch: Some(slot_reads_m31), ..Setup::on() };
    let off = run_prefetched(s, None, None);
    for form in FORMS {
        let on = run_prefetched(s, Some(form), None);
        assert!(on.checked.m31_reads_after >= 1, "{form:?}: opcode 3 returns on the fetch path");
        assert_eq!(off.m.amem, on.m.amem, "{form:?}");
    }
}

/// **Condition 6 true, a return on the fetch path does not fuse**: with
/// the sequence break up, the main loop's call is taken at every fetch
/// as without the prefetch, and the returns that would have fused on the
/// buffered word are counted as refused for it.
#[test]
fn condition_6_refuses_the_prefetched_word() {
    let s = Setup { sequence_break: true, ..Setup::on() };
    let off = run_prefetched(s, None, None);
    for form in FORMS {
        let on = run_prefetched(s, Some(form), None);
        assert_eq!(state(&on.m), state(&off.m), "{form:?}");
        assert_eq!(on.m.macro_dispatch.fused, fusing(fuses), "{form:?}");
        assert_eq!(on.n, off.n, "{form:?}");
        assert_eq!(on.counts.used, 0, "{form:?}");
        assert_eq!(on.counts.refused, [fusing_prefetched(fuses).len() as u64, 0], "{form:?}");
    }
}

/// The opcodes and handlers of the invalidation programs, and their words
/// in A memory: [`INVALIDATE`]'s second halfword runs one of them, and the
/// return from it needs the next word, which the buffer holds.
const STORE: u32 = 0o13;
const STORE_AT: u64 = 0o260;
const STORE_LATE: u32 = 0o14;
const STORE_LATE_AT: u64 = 0o264;
const MAP: u32 = 0o15;
const MAP_AT: u64 = 0o270;
const LCW: u32 = 0o16;
const LCW_AT: u64 = 0o274;
const TOUCH: u32 = 0o17;
const TOUCH_AT: u64 = 0o310;
/// A 60: the word stored over the program's second word; A 61 its
/// address; A 62 the program's page's virtual address; A 63 the map word
/// sending it to physical page 2 ([`page_2`]); A 64 the location counter
/// of the program's second word; A 65 the word [`TOUCH`] reads.
const NEW_WORD: u32 = hw(2, 0) | hw(2, 0) << 16;

/// Word 0 runs opcode 1 and then the opcode under test; word 1, opcode 1
/// twice unless something has put [`NEW_WORD`], opcode 2 twice, in its
/// place; word 2 stops.
const INVALIDATE: [[u32; 6]; 5] = [
    [hw(1, 0), hw(STORE, 0), hw(1, 0), hw(1, 0), hw(7, 0), hw(7, 0)],
    [hw(1, 0), hw(STORE_LATE, 0), hw(1, 0), hw(1, 0), hw(7, 0), hw(7, 0)],
    [hw(1, 0), hw(MAP, 0), hw(1, 0), hw(1, 0), hw(7, 0), hw(7, 0)],
    [hw(1, 0), hw(LCW, 0), hw(1, 0), hw(1, 0), hw(7, 0), hw(7, 0)],
    // A transfer, [`transfer`], while opcode 3 runs.
    [hw(1, 0), hw(3, 0), hw(1, 0), hw(1, 0), hw(7, 0), hw(7, 0)],
];

/// Where [`CODE`] lands once [`MAP`]'s write has sent its page to
/// physical page 2: on QUUX pages of 1024 words, on the CADR of 256.
fn page_2(m: &Machine) -> usize {
    if m.geometry.wide() {
        2 << 10 | CODE as usize & 0o1777
    } else {
        2 << 8 | CODE as usize & 0o377
    }
}

/// The handlers the invalidation programs run, their words, and physical
/// page 2 for [`MAP`].
fn invalidators(m: &mut Machine) {
    handlers(m);
    // Physical page 2, where the map write sends the program's page: the
    // program again, with the new word second.
    let at = page_2(m);
    for k in 0..3 {
        m.main[at + k] = m.main[CODE as usize + k];
    }
    m.main[at + 1] = u64::from(NEW_WORD);
}

/// The handlers the invalidation programs run, and their words.
fn handlers(m: &mut Machine) {
    let put = |m: &mut Machine, at: u64, w: u64| m.imem[at as usize] = Insn::new(w);
    // STORE: MD, then `VMA-START-WRITE` of the next word, and the return
    // in the microcycle after the start, before the cycle goes out.
    put(m, STORE_AT, ALU | SETA | a_src(0o60) | muir::isa::asm::MD);
    put(m, STORE_AT + 1, ALU | SETA | a_src(0o61) | muir::isa::asm::START_WRITE);
    put(m, STORE_AT + 2, filler().raw() | POPJ);
    // STORE_LATE: the same with a microcycle between, so that the cycle
    // has gone out when the return runs.
    put(m, STORE_LATE_AT, ALU | SETA | a_src(0o60) | muir::isa::asm::MD);
    put(m, STORE_LATE_AT + 1, ALU | SETA | a_src(0o61) | muir::isa::asm::START_WRITE);
    put(m, STORE_LATE_AT + 2, filler().raw());
    put(m, STORE_LATE_AT + 3, filler().raw() | POPJ);
    // MAP: MD the page's address, then `VMA-WRITE-MAP` (functional
    // destination 23) with level 2's enable, `VMA<28>` on QUUX and
    // `VMA<25>` on the CADR, and the entry, readable and writable.
    put(m, MAP_AT, ALU | SETA | a_src(0o62) | muir::isa::asm::MD);
    put(m, MAP_AT + 1, ALU | SETA | a_src(0o63) | fd(0o23));
    put(m, MAP_AT + 2, filler().raw() | POPJ);
    // LCW: the location counter written with the second word's, as it
    // stands after the step: the stream fetches it again, as the wrong
    // word.
    put(m, LCW_AT, ALU | SETA | a_src(0o64) | fd(1));
    put(m, LCW_AT + 1, filler().raw() | POPJ);
    // TOUCH: a read of A 65's word, into M 35, which fills its line.
    put(m, TOUCH_AT, ALU | SETA | a_src(0o65) | muir::isa::asm::START_READ);
    put(m, TOUCH_AT + 1, ALU | SETM | SRC_MD | m_dest(0o35));
    put(m, TOUCH_AT + 2, filler().raw() | POPJ);
    for (op, at) in [
        (STORE, STORE_AT),
        (STORE_LATE, STORE_LATE_AT),
        (MAP, MAP_AT),
        (LCW, LCW_AT),
        (TOUCH, TOUCH_AT),
    ] {
        m.dmem[OPDTB as usize + op as usize] = at as u32;
    }
    for (k, e) in m.macro_dispatch.entries.iter_mut().enumerate() {
        *e = m.dmem[OPDTB as usize + (k >> 3 & 0o37)];
    }
    m.amem[0o60] = u64::from(NEW_WORD);
    m.amem[0o61] = u64::from(CODE + 1);
    m.amem[0o62] = u64::from(CODE);
    m.amem[0o63] = if m.geometry.wide() {
        1 << 28 | 1 << 27 | 1 << 26 | 2
    } else {
        1 << 25 | 1 << 23 | 1 << 22 | 2
    };
    m.amem[0o64] = u64::from((CODE + 1) * 4 + 2);
}

/// A transfer's write of [`NEW_WORD`] over the program's second word, as
/// block-disk's and the file device's are: behind the processor's back,
/// raising the flag that invalidates the cache (`tests/block_disk.rs`,
/// `tests/quux_file_device.rs` hold that both raise it).
fn transfer(m: &mut Machine) {
    m.main[CODE as usize + 1] = u64::from(NEW_WORD);
    m.dma_written = true;
}

/// The same write without the flag: what a missed invalidation leaves.
fn unflagged(m: &mut Machine) {
    m.main[CODE as usize + 1] = u64::from(NEW_WORD);
}

/// **A store to the buffered word, a map write, a transfer and a write of
/// the location counter drop it**, and the return that needs the word
/// runs the fetch path, which finds the new one: each invalidation program
/// leaves the state it leaves without the prefetch --- opcode 2 run twice
/// where the old word ran opcode 1 --- and counts its drop. A store is
/// caught both before its cycle has gone out (the return refused) and
/// after (the word dropped as the cycle goes out).
#[test]
fn a_store_a_map_write_a_transfer_and_an_lc_write_drop_the_word() {
    let cases: [(usize, Touch, Drop); 5] = [
        (0, None, Drop::Store),
        (1, None, Drop::Store),
        (2, None, Drop::MapWrite),
        (3, None, Drop::LcWrite),
        (4, Some((0o214, transfer)), Drop::Dma),
    ];
    for (k, touch, why) in cases {
        let program: &'static [u32] = &INVALIDATE[k];
        let s = Setup { program, patch: Some(invalidators), ..Setup::on() };
        let off = run_prefetched(s, None, touch);
        let c = counts(&off.m);
        if why != Drop::LcWrite {
            assert_eq!((c[0], c[1]), (1, 2), "program {k}: the new word ran without the prefetch");
        }
        for form in FORMS {
            let on = run_prefetched(s, Some(form), touch);
            assert_eq!(state(&on.m), state(&off.m), "program {k}, {form:?}");
            assert_eq!(on.checked.problems(), 0, "program {k}, {form:?}: {:?}", on.checked.first);
            let dropped = on.counts.dropped[why as usize] + (k == 0) as u64 * on.counts.refused[1];
            assert!(dropped >= 1, "program {k}, {form:?}: {:?}", on.counts);
        }
    }
}

/// **The checkers find a word the prefetch should have dropped**: main
/// memory's second word changed behind the engine's back with no flag
/// raised, the buffer keeps the old one, the fused return takes it, and
/// the checkers count M 31 against main memory as a problem.
#[test]
fn the_checkers_find_a_stale_prefetched_word() {
    let program: &'static [u32] = &INVALIDATE[4];
    let s = Setup { program, patch: Some(invalidators), ..Setup::on() };
    let on = run_prefetched(s, Some(Reach::Line), Some((0o214, unflagged)));
    assert!(on.checked.stale_words >= 1, "{:?}", on.checked);
    assert!(on.checked.problems() >= 1);
}

/// **The page's reach takes the next line's word when the cache holds it,
/// the line's does not, and neither looks past the page**: a program whose
/// fourth word ends a line of eight, at 404, reads the fifth's line first
/// ([`TOUCH`]), and the return into the fifth fuses under [`Reach::Page`]
/// alone. Put at the end of a page of 1024 words, at 1774, the same
/// program's return into the next page's first word fuses under neither,
/// the cache holding it: the prefetch never looks past the page, which is
/// what keeps it from ever needing the map.
#[test]
fn the_page_reach_takes_the_next_line_and_never_the_next_page() {
    const LINES: [u32; 12] = [
        hw(1, 0),
        hw(TOUCH, 0),
        hw(1, 0),
        hw(1, 0),
        hw(1, 0),
        hw(1, 0),
        hw(1, 0),
        hw(1, 0),
        hw(1, 0),
        hw(1, 0),
        hw(7, 0),
        hw(7, 0),
    ];
    fn touch_next_line(m: &mut Machine) {
        handlers(m);
        m.amem[0o65] = u64::from(CODE + 8);
    }
    fn touch_next_page(m: &mut Machine) {
        handlers(m);
        m.amem[0o65] = 0o2000;
    }
    for (code, patch, page_end) in
        [(CODE + 4, touch_next_line as fn(&mut Machine), false), (0o1774, touch_next_page, true)]
    {
        let s = Setup { program: &LINES, code, patch: Some(patch), ..Setup::on() };
        let off = run_prefetched(s, None, None);
        let page = run_prefetched(s, Some(Reach::Page), None);
        let line = run_prefetched(s, Some(Reach::Line), None);
        for (form, on) in [("page", &page), ("line", &line)] {
            assert_eq!(state(&on.m), state(&off.m), "{code:o}, {form}");
            assert_eq!(on.checked.problems(), 0, "{code:o}, {form}: {:?}", on.checked.first);
        }
        assert_eq!(line.counts.next_line, 0, "{code:o}");
        if page_end {
            assert_eq!(page.counts.next_line, 0, "{code:o}: not past the page");
            assert!(page.counts.page_end >= 1, "{code:o}");
            assert_eq!(page.m.macro_dispatch.fused, line.m.macro_dispatch.fused, "{code:o}");
        } else {
            assert_eq!(page.counts.next_line, 1, "{code:o}: the fifth word, TOUCH's line");
            assert_eq!(page.m.macro_dispatch.fused, line.m.macro_dispatch.fused + 1, "{code:o}");
        }
    }
}

/// **-RESET drops the buffered word.**
#[test]
fn reset_drops_the_prefetched_word() {
    let mut r = Rtl::new(machine(Setup::on()));
    r.boot();
    let mut n = 0;
    while r.prefetched().is_none() {
        r.step().unwrap();
        n += 1;
        assert!(n < 200, "the first fetch fills the buffer");
    }
    assert_eq!(r.prefetched().unwrap().phys, CODE + 1);
    r.machine_mut().prog_reset = true;
    r.run(2);
    assert_eq!(r.prefetched(), None);
    assert_eq!(r.prefetch_counts().unwrap().dropped[Drop::Reset as usize], 1);
}

/// **QUUX has the prefetch, with the page's reach, and the CADR has
/// not**: `rtl` fits it from the start, and runs [`PROGRAM`] exactly as
/// with [`Reach::Page`] fitted by hand, fusing the returns
/// [`fusing_prefetched`] names on top of those `micro` fuses, in four
/// microcycles fewer each; the CADR has none, its buffer never filled.
#[test]
fn quux_takes_the_prefetch() {
    let r = Rtl::new(machine(Setup::on()));
    assert_eq!(r.prefetch(), Some(Reach::Page));
    let page = run_prefetched(Setup::on(), Some(Reach::Page), None);
    let off = run_prefetched(Setup::on(), None, None);
    let (n, m) = run(Rtl::new(machine(Setup::on())));
    assert_eq!(n, page.n, "the default is the page's reach");
    assert_eq!(m.macro_dispatch.fused, page.m.macro_dispatch.fused);
    let k = fusing_prefetched(fuses).len() as u64;
    assert!(k >= 4);
    assert_eq!(m.macro_dispatch.fused, fusing(fuses) + k);
    assert_eq!(off.n - n, 4 * k);
    let (_, micro) = run(Micro::new(machine(Setup::on())));
    assert_eq!(micro.macro_dispatch.fused, fusing(fuses), "micro has no prefetch");
    let s = Setup { geometry: Geometry::CADR, ..Setup::on() };
    let mut r = Rtl::new(machine(s));
    assert_eq!(r.prefetch(), None);
    r.boot();
    while r.machine().opc != STOP {
        r.step().unwrap();
        assert_eq!(r.prefetched(), None);
    }
}

/// **A checkpoint keeps the prefetched word, a fetch the port is yet to
/// answer, and M 31's word armed** (contract H8a §3.6): `rtl` saved at
/// every microcycle of [`PROGRAM`] on QUUX, and loaded into another,
/// saves the same file and runs on to the same end, in the same
/// microcycles, as the run never saved; among those microcycles are ones
/// with a word buffered and ones with M 31's word armed.
#[test]
fn a_checkpoint_keeps_the_prefetched_word_and_the_armed_m31() {
    use muir::checkpoint::{Reader, Writer};
    fn to_the_end(mut e: Rtl) -> (u64, Vec<u8>) {
        let mut n = 0;
        while e.machine().opc != STOP {
            e.step().unwrap();
            n += 1;
            assert!(n < 20_000);
        }
        e.run(8);
        let mut w = Writer::new();
        e.save(&mut w);
        (n, w.finish())
    }
    let (total, _) = run(Rtl::new(machine(Setup::on())));
    let (mut buffered, mut armed) = (0, 0);
    for at in 1..total {
        let mut straight = Rtl::new(machine(Setup::on()));
        straight.boot();
        straight.run(at);
        buffered += straight.prefetched().is_some() as u32;
        armed += straight.machine().macro_dispatch.m31.is_some() as u32;
        let mut w = Writer::new();
        straight.save(&mut w);
        let body = w.finish();
        let mut resumed = Rtl::new(machine(Setup::on()));
        resumed.boot();
        let mut r = Reader::for_word_bits(&body, 40);
        resumed.load(&mut r).unwrap();
        r.done().unwrap();
        assert_eq!(resumed.prefetched(), straight.prefetched(), "at {at}");
        let mut w = Writer::new();
        resumed.save(&mut w);
        assert!(w.finish() == body, "at {at}: loads and saves as itself");
        assert!(to_the_end(straight) == to_the_end(resumed), "at {at}: the same end");
    }
    assert!(buffered > 0 && armed > 0, "{buffered} {armed}");
}

/// **A call in the microcycle after a fused return returns to the
/// handler**: `XTFIXP` returned by `(POPJ-AFTER-NEXT ...)` with
/// `(CALL-NOT-EQUAL M-TEM A-4 XFALSE)` after it, before the microcode moved
/// the call out (the two lines are a comment in microcode 2001's
/// `uc-fctns.lisp:1143-1144`), a call with N
/// whose return is the POPJ's target, the main loop or, fused, the
/// handler. With opcode 1's POPJ followed by such a call, counting in
/// M 27, the program leaves the state it leaves without the fused return
/// but for the dead words above the micro stack's pointer, the microcycles
/// [`saved_on`] says fewer; the checkers count the stack moved and
/// the handler not next, which is what they found at `XTFIXP+10` in
/// bignum with the prefetch's (a).
#[test]
fn a_call_after_a_fused_return_returns_to_the_handler() {
    fn call_after_opcode_1(m: &mut Machine) {
        m.imem[0o206] = Insn::new(JUMP | target(0o330) | P | N | ALWAYS);
        m.imem[0o330] = Insn::new(ALU | M_PLUS_C | CARRY_IN | m_src(0o27) | m_dest(0o27) | POPJ);
    }
    let off = both(Setup { patch: Some(call_after_opcode_1), ..Setup::off() });
    let on = both(Setup { patch: Some(call_after_opcode_1), ..Setup::on() });
    // The words above the micro stack's pointer are dead: the call's push
    // leaves the handler's address there where the main loop's leaves its
    // own.
    let live = |m: &Machine| {
        let mut m = m.clone();
        for w in &mut m.spc[m.spcptr as usize + 1..] {
            *w = 0;
        }
        m
    };
    for ((name, n_off, m_off), (_, n_on, m_on)) in off.iter().zip(on.iter()) {
        assert_eq!(counts(m_off)[..6], COUNTS, "{name}");
        assert_eq!(m_off.mmem[0o27], COUNTS[0].into(), "{name}: the call after every opcode 1");
        let (a, b) = (live(m_on), live(m_off));
        assert_eq!(state(&a), state(&b), "{name}: the same live state");
        assert_eq!(m_on.macro_dispatch.fused, fused_on(name, fuses), "{name}");
        assert_eq!(n_off - n_on, saved_on(name, fuses), "{name}");
    }
    for (name, c) in checked(Setup { patch: Some(call_after_opcode_1), ..Setup::on() }) {
        assert!(c.stack_moved >= 1 && c.wrong_handler == c.stack_moved, "{name}: {c:?}");
    }
}
