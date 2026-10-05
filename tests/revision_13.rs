// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **QUUX revision 13's datapath** (contract G2 §2, with its appendix A1),
//! on hand-written microcode, on `micro` and `rtl`: the fields, the ALU,
//! the rotator and the masker, LC byte mode, the jump conditions, the
//! dispatch memory and the map, as [`Geometry::QUUX`] has them. Every
//! program runs on both engines, and each engine's M memory must be what
//! the program's comments say.
//!
//! The words are 40 bits, G1's: the cdr code `<39:38>`, the data type
//! `<37:32>` and the field `<31:0>`. A byte's rotate for an LDB is (40 −
//! position) mod 40: the type is the byte at 32, rotate 8, and a byte at
//! 8 is rotate 32 (G2 §2.3). `tests/word_width.rs` carries words with
//! `<39:32>` set through every word register.

use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{
    ADD, ALU, ALWAYS, AND, BYTE, DEP, DISPATCH, DMEM_WRITE, DPB, INVERT, IOR, JUMP, LDB, M_PLUS_C,
    MD, N, OB_RIGHT, POPJ, Q_LOAD, SETA, SETCM, SETM, SETO, SRC_MD, START_READ, START_WRITE, SUB,
    XOR, a_dest, a_src, filler, m_dest, m_src, src, target,
};
use muir::machine::{Geometry, Machine, Word, macro_dispatch};
use muir::micro::Micro;
use muir::rtl::Rtl;
use muir::tv::Board;

mod support;

/// Revision 13.
const REV13: Geometry = Geometry::QUUX;

/// A memory's constants: 0, 1, 2, and words the programs take as A.
const ZERO: u64 = 0o40;
const ONE: u64 = 0o41;
const TWO: u64 = 0o42;
/// M memory's 0 and 1, which [`Prog::taken`] writes with BYTE words, so
/// that no ALU word loads the overflow flag between a test and its jump.
const M_ZERO: u64 = 0o34;
const M_ONE: u64 = 0o35;

/// The program's last word, a jump to itself.
const STOP: u64 = 0o1000;

/// A 40-bit word from its tag `<39:32>` and field `<31:0>`.
const fn w(tag: u64, field: u64) -> Word {
    tag << 32 | (field & 0xffff_ffff)
}

/// A functional destination, with M's address 36 as the scratch word.
const fn fd(code: u64) -> u64 {
    code << 19 | 0o36 << 14
}
/// Destination 1, LOCATION-COUNTER, and 2, INTERRUPT-CONTROL.
const LC: u64 = fd(1);
const INTCTL: u64 = fd(2);
/// Destination 23, `VMA` with the map write a microcycle later.
const WRITE_MAP: u64 = fd(0o23);

/// Functional sources: 0 the dispatch constant, 2 the PDL pointer, 6 OPC,
/// 11 `MAP(MD)`, 13 the location counter, 16 MACHINE-ID, 17 unassigned.
const SRC_DC: u64 = src(0);
const SRC_PDLP: u64 = src(2);
const SRC_MAP: u64 = src(0o11);
const SRC_LC: u64 = src(0o13);
const SRC_ID: u64 = src(0o16);
const SRC_17: u64 = src(0o17);

/// A BYTE word: function, rotate `IR<5:0>` and length − 1 `IR<11:6>`
/// (A1.1).
fn byte(func: u64, rotate: u64, len: u64) -> u64 {
    BYTE | func | (len - 1) << 6 | rotate
}
/// `IR<24>` on an LDB: LC byte mode (A1.2).
const LC_MODE: u64 = 1 << 24;
/// `IR<11:10>` = 3 on a JUMP or DISPATCH: LC byte mode.
const MISC_LC: u64 = 3 << 10;
/// A rotate of JUMP or DISPATCH, `{IR<47>, IR<4:0>}` (A1.1).
fn rot6(r: u64) -> u64 {
    (r >> 5) << 47 | (r & 0o37)
}
/// A JUMP that tests bit 0 of M rotated by `r`.
fn jbit(r: u64) -> u64 {
    JUMP | rot6(r)
}
/// A JUMP on condition `code`, `IR<4:0>` with `IR<5>` (A1.3).
fn jcond(code: u64) -> u64 {
    JUMP | 1 << 5 | code
}
/// A DISPATCH: address `IR<23:12>`, length `IR<7:5>`, rotate.
fn disp(addr: u64, len: u64, r: u64) -> u64 {
    DISPATCH | addr << 12 | len << 5 | rot6(r)
}
/// The map bits of a DISPATCH, `IR<9:8>`: 1 takes the level-2 entry's
/// `<22>`, 2 its `<23>` (A1.7).
const MAP_22: u64 = 1 << 8;
const MAP_23: u64 = 2 << 8;

/// A program under construction: its words from control store 0, with
/// A-memory constants the setup gives.
#[derive(Default)]
struct Prog {
    words: Vec<u64>,
    amem: Vec<(u64, Word)>,
    mmem: Vec<(u64, Word)>,
    dmem: Vec<(usize, u32)>,
}

impl Prog {
    fn op(&mut self, w: u64) -> &mut Self {
        self.words.push(w);
        self
    }
    fn at(&self) -> u64 {
        self.words.len() as u64
    }
    /// A in A memory at `a`.
    fn a(&mut self, a: u64, v: Word) -> &mut Self {
        self.amem.push((a, v));
        self
    }
    /// M memory `m` (and the A memory word it shadows).
    fn m(&mut self, m: u64, v: Word) -> &mut Self {
        self.mmem.push((m, v));
        self
    }
    /// M `slot` <- 1 if the JUMP `jump` is taken, 0 if not: the jump goes
    /// to the word after the next with `N`, which the jump inhibits when
    /// taken and runs when not. The two writes are LDBs of the whole word,
    /// BYTE words, which leave the overflow flag.
    fn taken(&mut self, jump: u64, slot: u64) -> &mut Self {
        self.op(byte(LDB, 0, 40) | m_src(M_ONE) | m_dest(slot));
        let next = self.at() + 2;
        self.op(jump | target(next) | N);
        self.op(byte(LDB, 0, 40) | m_src(M_ZERO) | m_dest(slot))
    }
    /// Jumps to the stop.
    fn stop(&mut self) -> &mut Self {
        self.op(JUMP | target(STOP) | ALWAYS | N);
        self.op(filler().raw())
    }
}

/// The machine for `p`, revision 13, with page 0 of 1024 words mapped to
/// physical page 0, readable and writable, `setup` last.
fn machine(p: &Prog, setup: &dyn Fn(&mut Machine)) -> Machine {
    let mut prom: Vec<Insn> = p.words.iter().map(|&w| Insn::new(w)).collect();
    assert!(prom.len() <= STOP as usize, "the program runs into its stop");
    prom.resize(STOP as usize, filler());
    prom.push(Insn::new(JUMP | target(STOP) | ALWAYS | N));
    prom.resize(1024, filler());
    let mut m = Machine::new();
    m.geometry = REV13;
    m.load_prom(&prom);
    support::prom_program_in_ram(&mut m);
    m.amem[ZERO as usize] = 0;
    m.amem[ONE as usize] = 1;
    m.amem[TWO as usize] = 2;
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
    m.l2_map[0] = (1 << 27) | (1 << 26);
    setup(&mut m);
    m
}

/// Runs to the stop, and eight microcycles on.
fn finish<E: Engine>(e: &mut E, name: &str) {
    for _ in 0..4000 {
        if e.machine().opc == STOP as u16 {
            e.run(8);
            return;
        }
        e.step().unwrap();
    }
    panic!("{name}: the program never reached its stop");
}

/// Both engines' machines after `p`, `setup` applied before the boot and
/// `after_boot` after it.
fn run_with(
    p: &Prog,
    setup: &dyn Fn(&mut Machine),
    after_boot: &dyn Fn(&mut Machine),
) -> [(&'static str, Machine); 2] {
    let mut u = Micro::new(machine(p, setup));
    u.boot();
    after_boot(u.machine_mut());
    finish(&mut u, "micro");
    let mut r = Rtl::new(machine(p, setup));
    r.boot();
    after_boot(r.machine_mut());
    finish(&mut r, "rtl");
    [("micro", u.machine().clone()), ("rtl", r.machine().clone())]
}

fn run(p: &Prog) -> [(&'static str, Machine); 2] {
    run_with(p, &|_| {}, &|_| {})
}

/// M memory's `slot`s are `want`, on both engines, each named by `what`.
fn expect(ms: &[(&str, Machine)], rows: &[(u64, Word, &str)]) {
    for (engine, m) in ms {
        for &(slot, want, what) in rows {
            let got = m.mmem[slot as usize];
            assert_eq!(got, want, "{engine}: M {slot:o}, {what}: {got:#012x}, not {want:#012x}");
        }
    }
}

// --- the ALU ---------------------------------------------------------------

/// **The ALU on tags** (G2 §2.2): a logical function acts on all 40 bits;
/// an arithmetic one on `<31:0>`, the result's `<39:32>` M's; the output
/// bus's right shift on `<31:0>`, M's tag above.
#[test]
fn logical_functions_act_on_40_bits_and_arithmetic_keeps_ms_tag() {
    let x = w(0o245, 0x8000_00f0);
    let y = w(0o012, 0x7fff_ff0f);
    let mut p = Prog::default();
    p.m(1, x).a(0o50, y);
    let ops = [
        (AND, x & y),
        (IOR, x | y),
        (XOR, x ^ y),
        (SETCM, !x & ((1 << 40) - 1)),
        (SETO, (1 << 40) - 1),
        (ADD, w(0o245, 0x8000_00f0 + 0x7fff_ff0f)),
        (SUB, w(0o245, 0x8000_00f0u64.wrapping_sub(0x7fff_ff0f + 1))),
    ];
    for (k, (f, _)) in ops.iter().enumerate() {
        p.op(ALU | f | m_src(1) | a_src(0o50) | m_dest(2 + k as u64));
    }
    // M + 1 with the carry: arithmetic, M's tag, the field wrapping.
    p.m(0o20, w(0o377, 0xffff_ffff));
    p.op(ALU | M_PLUS_C | muir::isa::asm::CARRY_IN | m_src(0o20) | m_dest(0o21));
    // The right shift takes bit 32 of the field's 33-bit sum in at the top.
    p.op(OB_RIGHT | ADD | m_src(1) | a_src(0o50) | m_dest(0o22));
    p.stop();
    let mut rows: Vec<(u64, Word, &str)> =
        ops.iter().enumerate().map(|(k, &(_, v))| (2 + k as u64, v, "M 1 op A 50")).collect();
    rows.push((0o21, w(0o377, 0), "M + 1 wraps the field, keeps the tag"));
    // 0x800000f0 + 0x7fffff0f = 0xffffffff, sign-extended: 33-bit 1_ffff_ffff.
    rows.push((0o22, w(0o245, 0xffff_ffff), "the right shift keeps M's tag"));
    expect(&run(&p), &rows);
}

/// **A=M sees the tag; M < A and M ≤ A compare the fields, signed**
/// (G2 §2.2, A1.3): condition 1 on equal fields with different tags is
/// false, where the CADR's M < A, `NOT AEQM AND` bit 32, would be true
/// once `AEQM` saw the tags.
#[test]
fn equality_sees_the_tag_and_ordering_the_field() {
    let mut p = Prog::default();
    p.m(1, w(0o005, 7)).a(0o50, w(0o003, 7)).a(0o51, w(0o005, 7)).a(0o52, w(0o001, 8));
    p.m(2, w(0o377, 0xffff_ffff)).a(0o53, w(0, 0));
    p.taken(jcond(3) | m_src(1) | a_src(0o50), 0o10); // equal fields, tags differ
    p.taken(jcond(3) | m_src(1) | a_src(0o51), 0o11); // all 40 bits equal
    p.taken(jcond(1) | m_src(1) | a_src(0o50), 0o12); // M < A, equal fields
    p.taken(jcond(2) | m_src(1) | a_src(0o50), 0o13); // M ≤ A, equal fields
    p.taken(jcond(1) | m_src(1) | a_src(0o52), 0o14); // 7 < 8, tag 5 above 1
    p.taken(jcond(1) | m_src(2) | a_src(0o53), 0o15); // -1 < 0, tag 377 above 0
    p.taken(jcond(1) | INVERT | m_src(1) | a_src(0o52), 0o16);
    p.stop();
    expect(
        &run(&p),
        &[
            (0o10, 0, "A=M on equal fields, different tags"),
            (0o11, 1, "A=M on the same 40 bits"),
            (0o12, 0, "M<A on equal fields, different tags"),
            (0o13, 1, "M<=A on equal fields, different tags"),
            (0o14, 1, "M<A by field though M's tag is higher"),
            (0o15, 1, "M<A signed: -1 < 0 whatever the tags"),
            (0o16, 0, "inverted M<A"),
        ],
    );
}

/// **The fixnum overflow flag and condition 10** (G2 §2.2, A1.3): set by
/// `ADD` at 2^31 − 1 + 1 and by `SUB` at −2^31 − 1, whatever the tags;
/// clear after `AND` and after an `ADD` that does not overflow; not loaded
/// by an inhibited word; kept across a JUMP and a DISPATCH.
#[test]
fn the_overflow_flag_is_the_fields_signed_overflow() {
    let mut p = Prog::default();
    p.m(1, w(0o005, 0x7fff_ffff)).m(2, w(0o005, 0x8000_0000)).m(3, w(0o005, 5));
    p.a(ONE, 1).a(0o50, w(0o005, 0x7fff_ffff));
    // ADD 2^31 - 1 + 1: overflow.
    p.op(ALU | ADD | m_src(1) | a_src(ONE) | m_dest(0o20));
    p.taken(jcond(0o10), 0o21);
    // AND: clear.
    p.op(ALU | AND | m_src(1) | a_src(ONE) | m_dest(0o20));
    p.taken(jcond(0o10), 0o22);
    // SUB -2^31 - 1: overflow.
    p.op(ALU | SUB | muir::isa::asm::CARRY_IN | m_src(2) | a_src(ONE) | m_dest(0o20));
    p.taken(jcond(0o10), 0o23);
    // ADD 5 + 1: clear.
    p.op(ALU | ADD | m_src(3) | a_src(ONE) | m_dest(0o20));
    p.taken(jcond(0o10), 0o24);
    // ADD 2^31 - 1 + 1 again, then an inhibited AND: still set.
    p.op(ALU | ADD | m_src(1) | a_src(ONE) | m_dest(0o20));
    let over = p.at() + 2;
    p.op(JUMP | ALWAYS | target(over) | N);
    p.op(ALU | AND | m_src(1) | a_src(ONE) | m_dest(0o20));
    p.taken(jcond(0o10), 0o25);
    // A JUMP and a DISPATCH leave it: a jump on bit 1 of 5, not taken,
    // and a dispatch on 5's low three bits to entry 5, which goes to the
    // word after the next, N set.
    p.op(jbit(39) | m_src(3) | target(STOP) | N);
    let after = p.at() + 2;
    p.dmem.push((5, 1 << 14 | after as u32));
    p.op(disp(0, 3, 0) | m_src(3));
    p.op(filler().raw());
    p.taken(jcond(0o10), 0o26);
    // The logical functions of 40-bit words never set it.
    p.op(ALU | XOR | m_src(1) | a_src(0o50) | m_dest(0o20));
    p.taken(jcond(0o10), 0o27);
    // Nor do the special functions (40-77): a multiply step with `Q<0>`
    // set adds, 2^31 - 1 + 1 over the field, and clears the flag the ADD
    // before it set.
    p.op(ALU | SETA | a_src(ONE) | Q_LOAD);
    p.op(ALU | ADD | m_src(1) | a_src(ONE) | m_dest(0o20));
    p.op(ALU | 0o40 << 3 | m_src(1) | a_src(ONE) | m_dest(0o20));
    p.taken(jcond(0o10), 0o30);
    p.stop();
    expect(
        &run(&p),
        &[
            (0o21, 1, "ADD 2^31 - 1 + 1"),
            (0o22, 0, "AND"),
            (0o23, 1, "SUB -2^31 - 1"),
            (0o24, 0, "ADD 5 + 1"),
            (0o25, 1, "an inhibited AND loads nothing"),
            (0o26, 1, "a JUMP and a DISPATCH load nothing"),
            (0o27, 0, "XOR"),
            (0o30, 0, "a multiply step"),
        ],
    );
}

/// **Condition 11, M < A unsigned on the fields** (contract G2 §2.2 as
/// amended, A1.3): 0 against `37777777777` is below it unsigned and not
/// signed, which is what makes `(aref a -1)` fail a range check by
/// ordering.
#[test]
fn unsigned_less_than_compares_the_fields_unsigned() {
    let mut p = Prog::default();
    p.m(1, w(0o005, 0)).a(0o50, w(0, 0xffff_ffff)).m(2, w(0o005, 0xffff_ffff));
    p.a(0o51, w(0o377, 0));
    p.taken(jcond(0o11) | m_src(1) | a_src(0o50), 0o20); // 0 <u 37777777777
    p.taken(jcond(1) | m_src(1) | a_src(0o50), 0o21); // 0 < -1 signed: no
    p.taken(jcond(0o11) | m_src(2) | a_src(0o51), 0o22); // 37777777777 <u 0: no
    p.taken(jcond(0o11) | INVERT | m_src(2) | a_src(0o51), 0o23); // >= unsigned
    p.taken(jcond(0o11) | m_src(1) | a_src(ZERO), 0o24); // 0 <u 0: no
    // A reserved number decodes as its `IR<2:0>`: 17 as 7, always.
    p.taken(jcond(0o17) | m_src(1) | a_src(ZERO), 0o25);
    p.stop();
    expect(
        &run(&p),
        &[
            (0o20, 1, "0 <u 37777777777"),
            (0o21, 0, "0 < -1 signed"),
            (0o22, 0, "37777777777 <u 0"),
            (0o23, 1, "37777777777 >=u 0"),
            (0o24, 0, "0 <u 0"),
            (0o25, 1, "condition 17 decodes as 7"),
        ],
    );
}

// --- BYTE, the rotator and the masker ---------------------------------------

/// The word the byte tests take apart: cdr code 2, type 25 (octal), and a
/// field whose bytes are all different.
const T: Word = 2 << 38 | 0o25 << 32 | 0x8765_4321;

/// `<39:0>` of `v` rotated left by `n` mod 40: what the tests expect of the
/// ring (G2 §2.3).
fn ring(v: Word, n: u64) -> Word {
    let n = n % 40;
    if n == 0 { v } else { ((v << n) | (v >> (40 - n))) & ((1 << 40) - 1) }
}

/// **An LDB of the type and of a byte at 8, and bytes at rotates 32-39
/// and of lengths 33-40** (G2 §2.3, A1.2): the type is rotate 8, length
/// 6; the byte at 8 is rotate 32; the ring of 40 carries the tag into the
/// field and the field into the tag.
#[test]
fn ldb_rotates_in_a_ring_of_40() {
    let a = w(0o111, 0x1111_1111);
    let mut p = Prog::default();
    p.m(1, T).a(0o50, a);
    let cases: Vec<(u64, u64, Word, &str)> = vec![
        (8, 6, 0o25, "the type, the byte at 32"),
        (2, 2, 2, "the cdr code, the byte at 38"),
        (32, 8, 0x43, "the byte at 8"),
        (35, 8, (0x8765_4321 >> 5) & 0xff, "the byte at 5, rotate 35"),
        (39, 8, (0x8765_4321 >> 1) & 0xff, "the byte at 1, rotate 39"),
        (8, 40, ring(T, 8), "the whole word rotated by 8: the tag in <7:0>"),
        (0, 40, T, "length 40 takes every bit"),
        (0o50, 40, T, "rotate 40 is 0"),
        (0o77, 40, ring(T, 23), "rotate 63 is 23"),
        (4, 33, ring(T, 4) & ((1 << 33) - 1), "length 33"),
        (0, 41, a, "length 41 fits nowhere: A"),
    ];
    for (k, &(r, len, _, _)) in cases.iter().enumerate() {
        p.op(byte(LDB, r, len) | m_src(1) | a_src(0o50) | m_dest(2 + k as u64));
    }
    p.stop();
    let mut rows = vec![];
    for (k, &(_, len, v, what)) in cases.iter().enumerate() {
        // An LDB's mask starts at 0: above it, A shows through.
        let mask: Word = if len > 40 { 0 } else { (1 << len) - 1 };
        let want = if len > 40 { a } else { (v & mask) | (a & !mask & ((1 << 40) - 1)) };
        rows.push((2 + k as u64, want, what));
    }
    expect(&run(&p), &rows);
}

/// **DPB and selective deposit at 40 bits** (A1.2): the mask is
/// bits rotate to rotate + length − 1 if that fits in 0-39, and none
/// otherwise, so that A shows through: rotate 32 with length 8 fits,
/// rotate 33 with length 8 does not, nor do rotates 40 and 63.
#[test]
fn a_deposit_that_does_not_fit_in_40_bits_gives_a() {
    let a = w(0o111, 0x1111_1111);
    let mut p = Prog::default();
    p.m(1, w(0, 0xa5)).a(0o50, a).m(0o30, T);
    let cases = [(32, 8), (33, 8), (0o50, 1), (0o77, 1), (38, 2), (0, 40), (1, 40)];
    for (k, &(r, len)) in cases.iter().enumerate() {
        p.op(byte(DPB, r, len) | m_src(1) | a_src(0o50) | m_dest(2 + k as u64));
    }
    // Selective deposit: M's bits at the rotate, not rotated.
    p.op(byte(DEP, 32, 8) | m_src(0o30) | a_src(0o50) | m_dest(0o20));
    p.op(byte(DEP, 33, 8) | m_src(0o30) | a_src(0o50) | m_dest(0o21));
    p.stop();
    let dpb = |r: u64, len: u64| {
        if r + len > 40 {
            a
        } else {
            let mask: Word = ((1 << len) - 1) << r;
            (ring(w(0, 0xa5), r) & mask) | (a & !mask)
        }
    };
    let mut rows: Vec<(u64, Word, &str)> =
        cases.iter().enumerate().map(|(k, &(r, len))| (2 + k as u64, dpb(r, len), "DPB")).collect();
    rows.push((0o20, (a & !(0xff << 32)) | (T & 0xff << 32), "selective deposit at 32"));
    rows.push((0o21, a, "selective deposit at 33, length 8: A"));
    let ms = run(&p);
    expect(&ms, &rows);
    assert_eq!(dpb(32, 8), w(0xa5, 0x1111_1111), "the byte at 32 is the tag");
    assert_eq!(dpb(33, 8), a);
}

/// **LC byte mode by `IR<24>`, on an LDB** (A1.2): `M-INST-ADR-*2+X`,
/// `(BYTE-FIELD 12 47)` at 40 bits, is rotate 1 and length 10, which in
/// LC byte mode rotates 1 for halfword 0 (`LC<1>` = 1) and 25 for
/// halfword 1 (`LC<1>` = 0): the halfword's `<8:0>` doubled, the bit
/// below the halfword wrapping into `<0>`, bit 39 for halfword 0 and bit
/// 15 for halfword 1. A DPB with `IR<24>` ignores LC: LC byte mode is
/// LDB's, JUMP's and DISPATCH's alone.
#[test]
fn lc_byte_mode_takes_the_halfword_by_ir_24() {
    // Halfword 1 0o123456, halfword 0 0o100765, cdr code 2 (bit 39 set).
    let word = 2 << 38 | 0o25 << 32 | 0o123456 << 16 | 0o100765;
    let mut p = Prog::default();
    p.m(1, word).a(0o50, 0).a(0o51, 2).a(0o52, 0);
    // LC <- 2: LC<1> = 1, halfword 0.
    p.op(ALU | SETA | a_src(0o51) | LC);
    p.op(byte(LDB, 1, 10) | LC_MODE | m_src(1) | a_src(ZERO) | m_dest(2));
    // An LDB without IR<24> is plain rotate 1.
    p.op(byte(LDB, 1, 10) | m_src(1) | a_src(ZERO) | m_dest(3));
    // A DPB with IR<24> deposits at rotate 1, LC or no LC.
    p.op(byte(DPB, 1, 10) | LC_MODE | m_src(1) | a_src(ZERO) | m_dest(4));
    // LC <- 0: LC<1> = 0, halfword 1.
    p.op(ALU | SETA | a_src(0o52) | LC);
    p.op(byte(LDB, 1, 10) | LC_MODE | m_src(1) | a_src(ZERO) | m_dest(5));
    p.op(byte(DPB, 1, 10) | LC_MODE | m_src(1) | a_src(ZERO) | m_dest(6));
    p.stop();
    let hw0: Word = 0o100765;
    let hw1: Word = 0o123456;
    expect(
        &run(&p),
        &[
            (2, (hw0 & 0o777) << 1 | 1, "halfword 0, bit 39 in <0>"),
            (3, (hw0 & 0o777) << 1 | 1, "no LC byte mode: rotate 1"),
            (4, (word & 0o1777) << 1, "DPB at rotate 1, halfword 0's LC"),
            (5, (hw1 & 0o777) << 1 | (hw0 >> 15), "halfword 1, bit 15 in <0>"),
            (6, (word & 0o1777) << 1, "DPB at rotate 1, halfword 1's LC"),
        ],
    );
}

/// A counter with bits above `LC<25:0>`: revision 13's is `LC<29:0>`.
const LC_HIGH: u64 = 0o1_234_000_000;

/// **LC byte mode's bytes** (A1.2): in byte mode `LC` = 1, 2, 3, 0 take
/// bytes 0, 1, 2, 3 of the word, an LDB of rotate 0 and length 8. The
/// flag is INTERRUPT-CONTROL's `<37>`, and the location counter reads it
/// back at `<37>` with the counter in `<29:0>` (A1.6).
#[test]
fn lc_byte_mode_takes_bytes_in_stream_order() {
    let word = w(0o005, 0x4433_2211);
    let mut p = Prog::default();
    p.m(1, word).a(0o50, 1 << 37).a(0o51, 0);
    p.op(ALU | SETA | a_src(0o50) | INTCTL);
    for (k, lc) in [1u64, 2, 3, 0].into_iter().enumerate() {
        p.a(0o60 + k as u64, LC_HIGH | lc);
        p.op(ALU | SETA | a_src(0o60 + k as u64) | LC);
        p.op(byte(LDB, 0, 8) | LC_MODE | m_src(1) | a_src(ZERO) | m_dest(2 + k as u64));
    }
    // The location counter as the source reads it: NEED-FETCH (written
    // with it) in <39>, byte mode in <37>, the counter in <29:0>.
    p.op(ALU | SETM | SRC_LC | m_dest(0o10));
    p.op(ALU | SETA | a_src(0o51) | INTCTL);
    p.op(ALU | SETM | SRC_LC | m_dest(0o11));
    p.stop();
    expect(
        &run(&p),
        &[
            (2, 0x11, "LC 1: byte 0"),
            (3, 0x22, "LC 2: byte 1"),
            (4, 0x33, "LC 3: byte 2"),
            (5, 0x44, "LC 0: byte 3"),
            (0o10, 1 << 39 | 1 << 37 | LC_HIGH, "LC in byte mode"),
            (0o11, 1 << 39 | LC_HIGH, "LC in halfword mode"),
        ],
    );
}

/// **A BYTE word's `IR<11:10>` are length bits, not misc** (A1.1): an LDB
/// of length 17 has `IR<11:10>` = 1, which halts any other word, and runs
/// on with `ERRSTOP` set; one of length 33 has 2, and one of length 49 3,
/// which is not LC byte mode. An ALU word's output select 0 takes its
/// length from `IR<9:6>`, rotates by `IR<5:0>`, and has no LC byte mode.
#[test]
fn a_byte_words_misc_bits_are_its_length() {
    let a = w(0o111, 0x1111_1111);
    let mut p = Prog::default();
    p.m(1, T).a(0o50, a).a(0o51, 0);
    p.op(ALU | SETA | a_src(0o51) | LC);
    p.op(byte(LDB, 0, 17) | m_src(1) | a_src(0o50) | m_dest(2));
    p.op(byte(LDB, 4, 33) | m_src(1) | a_src(0o50) | m_dest(3));
    p.op(byte(LDB, 4, 49) | m_src(1) | a_src(0o50) | m_dest(4));
    // Output select 0 (IR<13:12> = 0): rotate 36, length 4 (IR<9:6> = 3),
    // with IR<11:10> = 3, which on a 32-bit machine, the CADR, would be LC
    // byte mode.
    p.op(36 | 3 << 6 | 3 << 10 | m_src(1) | a_src(0o50) | m_dest(5));
    p.stop();
    let ms = run_with(&p, &|_| {}, &|m| m.mode.errstop = true);
    let ldb = |r: u64, len: u64| {
        let mask: Word = (1 << len) - 1;
        (ring(T, r) & mask) | (a & !mask)
    };
    let mask: Word = 0o17 << 36;
    expect(
        &ms,
        &[
            (2, ldb(0, 17), "length 17 does not halt"),
            (3, ldb(4, 33), "length 33"),
            (4, a, "length 49 fits nowhere: A, and no LC byte mode"),
            (5, (ring(T, 36) & mask) | (a & !mask), "output select 0, 4 bits at 36"),
        ],
    );
}

// --- JUMP and DISPATCH -------------------------------------------------------

/// **A JUMP tests a bit with a 6-bit rotate, `IR<47>` its bit 5** (A1.1):
/// bit 5 is rotate 35, `IR<47>` set; bit 37 is rotate 3. Taken without
/// `IR<47>`, rotate 35 would test bit 37.
#[test]
fn a_jump_tests_bits_5_and_37() {
    let mut p = Prog::default();
    p.m(1, 1 << 5).m(2, 1 << 37);
    p.taken(jbit(35) | m_src(1), 0o10); // bit 5 of 1<<5
    p.taken(jbit(3) | m_src(1), 0o11); // bit 37 of 1<<5
    p.taken(jbit(35) | m_src(2), 0o12); // bit 5 of 1<<37
    p.taken(jbit(3) | m_src(2), 0o13); // bit 37 of 1<<37
    p.taken(jbit(0o50 + 5) | m_src(2), 0o14); // rotate 45 is 5: bit 35: no
    p.taken(jbit(0o75) | m_src(1), 0o15); // rotate 61 is 21: bit 19: no
    p.stop();
    expect(
        &run(&p),
        &[
            (0o10, 1, "bit 5 of a word with bit 5"),
            (0o11, 0, "bit 37 of a word with bit 5"),
            (0o12, 0, "bit 5 of a word with bit 37"),
            (0o13, 1, "bit 37 of a word with bit 37"),
            (0o14, 0, "rotate 45"),
            (0o15, 0, "rotate 61"),
        ],
    );
}

/// **LC byte mode on a JUMP** (A1.2): `IR<11:10>` = 3 adds 24 for
/// halfword 1, so a test of bit 3 (rotate 37) tests bit 19.
#[test]
fn a_jump_in_lc_byte_mode_tests_the_halfword() {
    let mut p = Prog::default();
    p.m(1, 1 << 19).a(0o50, 2).a(0o51, 0);
    p.op(ALU | SETA | a_src(0o50) | LC);
    p.taken(jbit(37) | MISC_LC | m_src(1), 0o10); // halfword 0: bit 3
    p.op(ALU | SETA | a_src(0o51) | LC);
    p.taken(jbit(37) | MISC_LC | m_src(1), 0o11); // halfword 1: bit 19
    p.stop();
    expect(&run(&p), &[(0o10, 0, "halfword 0's bit 3"), (0o11, 1, "halfword 1's bit 3")]);
}

/// The dispatch memory's landing pads: every entry goes to `BAD`, which
/// writes 2 to M 10 and stops; `good` goes to `GOOD`, which writes 1. A
/// dispatch to an entry whose N is set inhibits the word after it.
const GOOD: u64 = 0o700;
const BAD: u64 = 0o710;

fn landing(p: &mut Prog) {
    while p.at() < GOOD {
        p.op(filler().raw());
    }
    p.op(ALU | SETA | a_src(ONE) | m_dest(0o10)).stop();
    while p.at() < BAD {
        p.op(filler().raw());
    }
    p.op(ALU | SETA | a_src(TWO) | m_dest(0o10)).stop();
}

/// A program that runs `body`, then lands; every dispatch-memory entry
/// but `good` sends it to `BAD`.
fn dispatch_program(good: usize, body: &dyn Fn(&mut Prog)) -> Prog {
    let mut p = Prog::default();
    for k in 0..4096 {
        p.dmem.push((k, 1 << 14 | BAD as u32));
    }
    p.dmem.push((good, 1 << 14 | GOOD as u32));
    body(&mut p);
    p.op(filler().raw()).stop();
    landing(&mut p);
    p
}

fn landed(p: &Prog, setup: &dyn Fn(&mut Machine), what: &str) {
    expect(&run_with(p, setup, &|_| {}), &[(0o10, 1, what)]);
}

/// **A DISPATCH on the 6-bit type into entries above 2,047** (G2 §2.4):
/// the address `IR<23:12>`, the type the byte at 32, rotate 8, length 6.
#[test]
fn a_dispatch_on_the_type_reaches_entries_above_2047() {
    let p = dispatch_program(0o4000 + 0o25, &|p| {
        p.m(1, T);
        p.op(disp(0o4000, 6, 8) | m_src(1));
    });
    landed(&p, &|_| {}, "the type's entry at 4025");
    // The last entry, 7777, from the address alone.
    let p = dispatch_program(0o7777, &|p| {
        p.op(disp(0o7777, 0, 0));
    });
    landed(&p, &|_| {}, "entry 7777");
    // The byte at 5, rotate 35: `IR<47>` set. Rotate 3, without it, would
    // take bits 37-39, the type's top and the cdr code.
    let p = dispatch_program(0o6005, &|p| {
        p.m(1, 2 << 38 | 5 << 5);
        p.op(disp(0o6000, 3, 35) | m_src(1));
    });
    landed(&p, &|_| {}, "the byte at 5's entry at 6005");
    // The cdr code, the byte at 38, rotate 2, length 2, at 7774.
    let p = dispatch_program(0o7776, &|p| {
        p.m(1, T);
        p.op(disp(0o7774, 2, 2) | m_src(1));
    });
    landed(&p, &|_| {}, "the cdr code's entry at 7776");
}

/// **A 7-bit (type, map bit) dispatch reaching an entry above 2,047**
/// (G2 §2.4, A1.7): the transporter's field, the type and the bit below
/// it, the byte at 31, rotate 9, length 7, with the map bit of `MD`'s
/// page in place of its bit 0: `IR<9>` takes the level-2 entry's `<23>`,
/// `IR<8>` its `<22>`.
#[test]
fn a_type_and_map_bit_dispatch_reaches_entries_above_2047() {
    // MD: type 25, a pointer to word 3 of virtual page 5, whose level-2
    // entry has <23> set and <22> clear.
    let pointer = 5 << 10 | 3;
    let md = w(0o25, pointer);
    let setup = |m: &mut Machine| {
        m.md = md;
        m.l2_map[5] = 1 << 27 | 1 << 23 | 7;
    };
    let p = dispatch_program(0o7400 + (0o25 << 1 | 1), &|p| {
        p.op(disp(0o7400, 7, 9) | MAP_23 | SRC_MD);
    });
    landed(&p, &setup, "(type, <23>) at 7453");
    let p = dispatch_program(0o7400 + (0o25 << 1), &|p| {
        p.op(disp(0o7400, 7, 9) | MAP_22 | SRC_MD);
    });
    landed(&p, &setup, "(type, <22>) at 7452");
}

/// **A DISPATCH in LC byte mode on both halfwords** (A1.2): the field at
/// `<13:9>`, rotate 31 for halfword 0, 55 for halfword 1.
#[test]
fn a_dispatch_in_lc_byte_mode_takes_either_halfword() {
    let word = w(0, 0o21 << 25 | 0o13 << 9);
    for (lc, op) in [(2u64, 0o13usize), (0, 0o21)] {
        let p = dispatch_program(0o5000 + op, &|p| {
            p.m(1, word).a(0o50, lc);
            p.op(ALU | SETA | a_src(0o50) | LC);
            p.op(disp(0o5000, 5, 31) | MISC_LC | m_src(1));
        });
        landed(&p, &|_| {}, &format!("LC {lc}: opcode {op:o}"));
    }
}

/// **A dispatch-memory write at an entry above 2,047, and the last
/// entry's fall-through** (A1.4): misc 2 writes `A<16:0>` at `IR<23:12>`
/// OR the field; `LAST-DMEM-LOCATION`, `7777`, is P and R, which is
/// neither a call nor a return but the next word.
#[test]
fn the_dispatch_memory_is_4096_entries() {
    let mut p = Prog::default();
    p.a(0o50, 1 << 16 | 1 << 15);
    p.op(disp(0o7777, 0, 0) | DMEM_WRITE | a_src(0o50));
    p.op(filler().raw());
    // The dispatch at 7777 falls through to the next word.
    p.op(disp(0o7777, 0, 0));
    p.op(ALU | SETA | a_src(ONE) | m_dest(0o10));
    p.stop();
    let ms = run(&p);
    expect(&ms, &[(0o10, 1, "the fall-through ran the next word")]);
    for (engine, m) in &ms {
        assert_eq!(m.dmem[0o7777], 1 << 16 | 1 << 15, "{engine}: entry 7777 written");
        assert_eq!(m.dmem[0o3777], 0, "{engine}: entry 3777 not written");
    }
    let word_6 = muir::machine::REGISTER_PAGE_13 | 6;
    assert_eq!(REV13.feature_word(word_6), Some(4096), "feature word 6");
}

// --- sources and constants ---------------------------------------------------

/// **Numeric sources read `<39:32>` as 0; an unassigned one all 40 bits
/// as ones; MACHINE-ID says revision 13** (G2 §2.5, A1.6); and a 32-bit
/// constant is zero-extended, so that an arithmetic result with
/// `37777777777` as M has tag 000, and with SETO's ones tag 377 (A1.5).
#[test]
fn numeric_sources_and_constants_are_zero_extended() {
    let mut p = Prog::default();
    p.m(1, w(0, 0xffff_ffff)).a(0o50, w(0o005, 0x10));
    let after = p.at() + 2;
    p.dmem.push((0, 1 << 14 | after as u32));
    p.op(disp(0, 0, 0) | 0o1777 << 32);
    p.op(filler().raw());
    p.op(ALU | SETM | SRC_DC | m_dest(2));
    p.op(ALU | SETM | SRC_PDLP | m_dest(3));
    p.op(ALU | SETM | SRC_ID | m_dest(4));
    p.op(ALU | SETM | SRC_17 | m_dest(5));
    p.op(ALU | ADD | m_src(1) | a_src(0o50) | m_dest(6));
    p.op(ALU | SETO | m_dest(7));
    p.op(ALU | ADD | m_src(7) | a_src(0o50) | m_dest(0o10));
    p.stop();
    let ms = run_with(&p, &|_| {}, &|m| m.pdl_pointer = 0o37777);
    expect(
        &ms,
        &[
            (2, 0o1777, "the dispatch constant"),
            (3, 0o37777, "the PDL pointer"),
            (4, (0x5155 << 16) | (13 << 4) | 4, "MACHINE-ID"),
            (5, (1 << 40) - 1, "source 17"),
            (6, 0xf, "-1 zero-extended as M: tag 000"),
            (0o10, w(0o377, 0xf), "SETO's ones as M: tag 377"),
        ],
    );
}

// --- the map -----------------------------------------------------------------

/// A 28-bit virtual address whose every level-1 and level-2 bit matters:
/// `VA<27:15>` 12345 (octal), `VA<14:10>` 26, `VA<9:0>` 1234.
const VA: u32 = 0o12345 << 15 | 0o26 << 10 | 0o1234;
const L1_INDEX: usize = 0o12345;
const BLOCK: u32 = 0o123;
const L2_INDEX: usize = (0o123 << 5) | 0o26;
/// The physical page it maps to, and so the word.
const PAGE: u32 = 0o1357;
const PHYS: usize = (0o1357 << 10) | 0o1234;

/// The map for [`VA`]: level 1 at `VA<27:15>` names block 123, whose entry
/// at `VA<14:10>` is page 1357, readable and writable.
fn map_va(m: &mut Machine) {
    m.l1_map[L1_INDEX] = BLOCK;
    m.l2_map[L2_INDEX] = 1 << 27 | 1 << 26 | PAGE;
    m.main[PHYS] = w(0o005, 0x600d);
    m.main[PHYS & !0o1400] = w(0o005, 0xbad);
}

/// **The map at 28 bits with 1024-word pages** (G2 §2.6, A1.7): `VA<27:15>`
/// indexes level 1, `{L1, VA<14:10>}` level 2, and the word is
/// `{L2<17:0>, VA<9:0>}`; `VA<9:8>`, the offset's high bits at 1024-word
/// pages, and `<39:32>` are not in the map. An address with `<31:28>` not
/// zero reads block 177, whose entries are 0: a page fault.
#[test]
fn the_map_translates_28_bits_at_1024_word_pages() {
    let mut p = Prog::default();
    p.a(0o50, VA as Word).a(0o51, w(0o245, VA as u64)).a(0o52, (1 << 28 | VA) as Word);
    p.a(0o53, (VA & !0o1400) as Word);
    for (k, a) in [0o50u64, 0o51, 0o53].into_iter().enumerate() {
        p.op(ALU | SETA | a_src(a) | START_READ);
        p.op(filler().raw()).op(filler().raw());
        p.op(ALU | SETM | SRC_MD | m_dest(2 + k as u64));
    }
    // <31:28> set: the read is refused, and condition 4, page fault, holds.
    p.op(ALU | SETA | a_src(0o52) | START_READ);
    p.taken(jcond(4), 5);
    p.stop();
    expect(
        &run_with(&p, &map_va, &|_| {}),
        &[
            (2, w(0o005, 0x600d), "VA"),
            (3, w(0o005, 0x600d), "VA with <39:32> set"),
            (4, w(0o005, 0xbad), "VA with <9:8> clear"),
            (5, 1, "<31:28> set: a page fault"),
        ],
    );
}

/// **`MAP(MD)` and a map write, round trip** (A1.7): the write's word in
/// `VMA`, `<38:32>` level 1 with `<29>`, `<27:0>` level 2 with `<28>`; its
/// address in `MD`. Level 1 177 and level-2 entry 7777 read back through
/// `MAP(MD)`: `<38:32>` level 1, `<27:0>` level 2. A write whose address
/// has `<31:28>` set leaves both levels alone, and a write with both
/// enables writes level 1 only. `MAP(MD)` of an address with `<31:28>` set
/// reads block 177.
#[test]
fn map_md_and_the_map_write_round_trip() {
    // MD<27:15> 7777, MD<14:10> 37.
    let addr: Word = 0o7777 << 15 | 0o37 << 10;
    let l2: Word = 1 << 27 | 1 << 26 | 0o7654321;
    let mut p = Prog::default();
    p.a(0o50, addr).a(0o51, 0o177 << 32 | 1 << 29).a(0o52, 1 << 28 | l2);
    p.a(0o53, addr | 1 << 28).a(0o54, 0o55 << 32 | 1 << 29 | 1 << 28 | 0o1111);
    p.a(0o55, 0o1234 << 15).a(0o56, 0o22 << 32 | 1 << 29 | 1 << 28 | 0o3333);
    p.a(0o57, 1 << 28 | 0o4444);
    // MD <- the address; level 1 <- 177; level 2 at {177, 37} <- l2.
    p.op(ALU | SETA | a_src(0o50) | MD);
    p.op(ALU | SETA | a_src(0o51) | WRITE_MAP);
    p.op(filler().raw());
    p.op(ALU | SETA | a_src(0o52) | WRITE_MAP);
    p.op(filler().raw()).op(filler().raw());
    p.op(ALU | SETM | SRC_MAP | m_dest(2));
    // An address with <31:28> set: MAP(MD) reads block 177, and a write
    // there, both enables, leaves both levels.
    p.op(ALU | SETA | a_src(0o53) | MD);
    p.op(filler().raw());
    p.op(ALU | SETM | SRC_MAP | m_dest(3));
    p.op(ALU | SETA | a_src(0o54) | WRITE_MAP);
    p.op(filler().raw()).op(filler().raw());
    p.op(ALU | SETA | a_src(0o57) | WRITE_MAP);
    p.op(filler().raw()).op(filler().raw());
    // Both enables at 1234 << 15: level 1 only.
    p.op(ALU | SETA | a_src(0o55) | MD);
    p.op(ALU | SETA | a_src(0o56) | WRITE_MAP);
    p.op(filler().raw()).op(filler().raw());
    p.stop();
    let ms = run_with(&p, &|_| {}, &|_| {});
    // The last memory cycle was the PROM's jump's none: the fault bits
    // are the latch's, the same on both engines, and not the point here.
    for (engine, m) in &ms {
        let map = |slot: usize| m.mmem[slot] & !(3 << 30);
        assert_eq!(map(2), 0o177 << 32 | l2, "{engine}: MAP(MD) after the writes");
        assert_eq!(map(3), 0o177 << 32 | l2, "{engine}: MAP(MD) of <31:28> set: block 177");
        // The writes at an address with <31:28> set would have put 55 in
        // level 1 at 7777 and 4444 in level 2 at {177, 37}.
        assert_eq!(m.l1_map[0o7777], 0o177, "{engine}: level 1 written, and not at <31:28>");
        assert_eq!(m.l2_map[0o7777], l2 as u32, "{engine}: level 2 written, and not at <31:28>");
        assert_eq!(m.l1_map[0o1234], 0o22, "{engine}: both enables: level 1");
        assert_eq!(m.l2_map[0o22 << 5], 0, "{engine}: and not level 2 at the new block");
        assert_eq!(m.l2_map[0], 0o1 << 27 | 1 << 26, "{engine}: nor at block 0");
    }
    assert_eq!(ms[0].1.mmem[2], ms[1].1.mmem[2], "the engines' MAP(MD)");
}

// --- H8a's fused return at 40 bits -------------------------------------------

/// **The fused return in the ring of 40** (G2 §2.7, A1.2): the MACRO
/// DISPATCH MEMORY's index is M 31 rotated by 34 = 40 − 6 under LC byte
/// mode's addend, 18 for halfword 1, which brings the halfword's `<15:6>`
/// to `<9:0>` and its delta `<5:0>` to `<39:34>`; a LOCAL operand's
/// address is `A-LOCALP` + delta in PDL-INDEX when the handler starts.
///
/// The program returns into its main loop twice: the first return needs a
/// fetch and runs the loop, which takes the fetched word into M 31 with
/// the counter on the word's halfword 1; the second needs none and fuses
/// to the handler, which records PDL-INDEX and stops. A return that took
/// another halfword, or another index, would find no entry's handler and
/// never stop.
#[test]
fn the_fused_return_takes_the_halfword_in_the_ring_of_40() {
    const QMLP: u64 = 0o100;
    const HANDLER: u64 = 0o300;
    const CODE: u64 = 0o400;
    const LOCALP_AT: u64 = 0o432;
    const LOCALP: u64 = 0o1000;
    // Halfword 1: opcode 13, register 5 (LOCAL), delta 27; halfword 0
    // another opcode with register 6; the word with a tag, which the
    // index never sees.
    let hw1: u64 = 0o13 << 9 | 5 << 6 | 0o27;
    let hw0: u64 = 0o21 << 9 | 6 << 6 | 0o11;
    let word = w(0o025, hw1 << 16 | hw0);
    let index = hw1 >> 6;
    let main = 1 << 14 | QMLP;
    let mut p = Prog::default();
    p.a(0o60, macro_dispatch::word(QMLP as u16, LOCALP_AT as u16, 0o21) as Word);
    p.a(0o61, index).a(0o62, (macro_dispatch::OPERAND as u64) | HANDLER);
    p.a(0o63, LOCALP).a(0o64, 4 * CODE).a(0o65, main);
    p.op(ALU | SETA | a_src(ZERO) | INTCTL);
    p.op(ALU | SETA | a_src(0o60) | fd(5));
    p.op(ALU | SETA | a_src(0o61) | fd(6));
    p.op(ALU | SETA | a_src(0o62) | fd(7));
    p.op(ALU | SETA | a_src(0o63) | a_dest(LOCALP_AT));
    p.op(ALU | SETA | a_src(0o64) | LC);
    p.op(ALU | SETA | a_src(0o65) | fd(0o15));
    // The first return: LC was written, so it fetches.
    p.op(filler().raw() | POPJ);
    while p.at() < QMLP {
        p.op(filler().raw());
    }
    // The main loop: M 31 <- the fetched word, the return pushed back,
    // and the second return, which fuses.
    p.op(filler().raw()).op(filler().raw());
    p.op(ALU | SETM | SRC_MD | m_dest(0o31));
    p.op(ALU | SETA | a_src(0o65) | fd(0o15));
    p.op(filler().raw() | POPJ);
    while p.at() < HANDLER {
        p.op(filler().raw());
    }
    p.op(ALU | SETM | src(3) | m_dest(0o10));
    p.stop();
    let setup = |m: &mut Machine| m.main[CODE as usize] = word;
    let ms = run_with(&p, &setup, &|_| {});
    expect(&ms, &[(0o31, word, "M 31"), (0o10, LOCALP + 0o27, "PDL-INDEX: A-LOCALP + delta")]);
    for (engine, m) in &ms {
        assert_eq!(m.macro_dispatch.fused, 1, "{engine}: one fused return");
    }
}

// --- the physical space at 28 bits (G1 §3.2, G2 §3-§4) --------------------------

/// Physical addresses the space tests reach, each through a virtual page of
/// its own: the register page, `1777777400`; the CADR's last page,
/// `17777400`; the frame buffer window, `1760000000`; main memory above 22
/// bits, `20000000`; the first word past main memory; and `17773000`, which
/// the CADR's decode of its Unibus window takes for the diagnostic
/// registers.
const REGISTER_PAGE: u32 = 0o1777777400;
const OLD_REGISTER_PAGE: u32 = 0o17777400;
const WINDOW: u32 = 0o1760000000;
const HIGH: u32 = 0o20000000;
/// Main memory for the space tests: 4MW and a page, so that [`HIGH`]
/// and the old Unibus window are in it.
const MAIN_13: usize = 0o20002000;
const SPY: u32 = 0o17773000;

/// The virtual address that reaches physical `phys` through virtual page
/// `vpage` (below 32, so in level-1 block 0), and its map entry: readable,
/// writable, the page `phys<27:10>`.
fn through(m: &mut Machine, vpage: u32, phys: u32) -> Word {
    m.l2_map[vpage as usize] = 1 << 27 | 1 << 26 | phys >> 10;
    ((vpage << 10) | (phys & 0o1777)) as Word
}

impl Prog {
    /// M `slot` <- the word at the virtual address in A `a`.
    fn read_to(&mut self, a: u64, slot: u64) -> &mut Self {
        self.op(ALU | SETA | a_src(a) | START_READ);
        self.op(filler().raw()).op(filler().raw());
        self.op(ALU | SETM | SRC_MD | m_dest(slot))
    }
    /// The word in A `word` to the virtual address in A `a`.
    fn write_from(&mut self, word: u64, a: u64) -> &mut Self {
        self.op(ALU | SETA | a_src(word) | MD);
        self.op(ALU | SETA | a_src(a) | START_WRITE);
        self.op(filler().raw()).op(filler().raw())
    }
}

/// **The physical space is 28 bits** (G1 §3.2, G2 §4.1, A1.10): the
/// register page at `1777777400`, whose feature words say revision 13 and
/// its sizes, the video controller's buffer at `1760000000`; main memory,
/// not a register, at the CADR's last page, `17777400`, when there is that
/// much of it (`tests/revision_13_memory.rs` has it nothing when there is
/// not); main memory whole above 22 bits, up to its end, and nothing past
/// it, which fails with word 101's NXM bit; and main memory, not the
/// diagnostic registers, at `17773000`.
#[test]
fn the_physical_space_is_28_bits() {
    let va = |m: &mut Machine| {
        [
            through(m, 1, REGISTER_PAGE),
            through(m, 2, OLD_REGISTER_PAGE),
            through(m, 4, HIGH),
            through(m, 5, MAIN_13 as u32),
            through(m, 6, SPY),
        ]
    };
    let mut p = Prog::default();
    let mut probe = Machine::new();
    let [reg, old, high, past, spy] = va(&mut probe);
    for (k, off) in [0u64, 1, 2, 6, 0o13].into_iter().enumerate() {
        p.a(0o60 + k as u64, reg + off);
        p.read_to(0o60 + k as u64, 0o10 + k as u64);
    }
    // The CADR's last page: main memory, and word 101 has no NXM.
    p.a(0o65, old).a(0o66, reg + 0o101);
    p.read_to(0o65, 0o15).read_to(0o66, 0o16);
    // Main memory above 22 bits, a whole word; past its end nothing.
    p.a(0o67, high + 5).a(0o70, w(0o025, 0x1357_9bdf)).a(0o71, past);
    p.write_from(0o70, 0o67).read_to(0o67, 0o20).read_to(0o71, 0o21).read_to(0o66, 0o22);
    // A write of word 101 clears it.
    p.write_from(ZERO, 0o66).read_to(0o66, 0o17);
    // The old Unibus window's diagnostic registers are main memory here.
    p.a(0o72, spy + 5);
    p.read_to(0o72, 0o23);
    p.stop();
    let setup = |m: &mut Machine| {
        m.main = vec![0; MAIN_13];
        va(m);
        m.main[SPY as usize + 5] = w(0o031, 0xab_cdef);
        m.main[OLD_REGISTER_PAGE as usize] = w(0o031, 0o400);
    };
    let ms = run_with(&p, &setup, &|_| {});
    expect(
        &ms,
        &[
            (0o10, (0x5155 << 16) | (13 << 4) | 4, "word 0, MACHINE-ID"),
            (0o11, 7, "word 1, the level-1 entry's bits"),
            (0o12, 4096, "word 2, the level-2 entries"),
            (0o13, 4096, "word 6, the dispatch memory's entries"),
            (0o14, WINDOW as Word, "word 13, the video controller's buffer"),
            (0o15, w(0o031, 0o400), "17777400: main memory"),
            (0o16, 0, "word 101: no NXM"),
            (0o17, 0, "word 101 cleared by its write"),
            (0o20, w(0o025, 0x1357_9bdf), "main memory at 20000005"),
            (0o21, 0, "past main memory: nothing"),
            (0o22, 1, "word 101: NXM"),
            (0o23, w(0o031, 0xab_cdef), "main memory at 17773005"),
        ],
    );
    for (engine, m) in &ms {
        assert_eq!(m.main[HIGH as usize + 5], w(0o025, 0x1357_9bdf), "{engine}: main memory");
    }
}

/// **The frame buffer window, 4 bytes a word** (G1 §4.2): from `1760000000`,
/// a write stores the field and drops the tag, and a read returns the field
/// with the unboxed tag `005`; past the buffer's end, nothing.
#[test]
fn the_frame_buffer_window_holds_the_field() {
    let va = |m: &mut Machine| [through(m, 3, WINDOW), through(m, 7, WINDOW + 0o2000)];
    let mut p = Prog::default();
    let mut probe = Machine::new();
    let [window, _] = va(&mut probe);
    p.a(0o60, window + 7).a(0o61, w(0o025, 0x1234_5678)).a(0o62, window + 0o1777);
    p.write_from(0o61, 0o60).read_to(0o60, 0o10);
    p.write_from(0o61, 0o62).read_to(0o62, 0o11);
    p.stop();
    let setup = |m: &mut Machine| {
        va(m);
    };
    let ms = run_with(&p, &setup, &|_| {});
    expect(
        &ms,
        &[
            (0o10, w(0o005, 0x1234_5678), "the window: the field, tag 005"),
            (0o11, w(0o005, 0x1234_5678), "the window's word 1777"),
        ],
    );
    for (engine, m) in &ms {
        assert_eq!(m.tv.read_buffer(7), 0x1234_5678, "{engine}: the buffer holds the field");
        assert_eq!(m.tv.read_buffer(0o1777), 0x1234_5678, "{engine}: and at 1777");
    }
}

/// **A store to a word that spans two beats** (G1 §4.1): the words at a
/// line's offsets 1, 3, 4 and 6 lie across two of its five 64-bit beats in
/// packed storage. Stored on both engines, each is whole in main memory, and
/// the machine's checkpoint holds the line as packed storage lays it out,
/// 5 bytes a word, `<7:0>` first and the tag last.
#[test]
fn a_store_to_a_word_spanning_two_beats_is_whole() {
    let words = [
        w(0o101, 0x0102_0304),
        w(0o303, 0x0506_0708),
        w(0o104, 0x090a_0b0c),
        w(0o306, 0x0d0e_0f10),
    ];
    let mut p = Prog::default();
    for (k, (&x, off)) in words.iter().zip([1u64, 3, 4, 6]).enumerate() {
        p.a(0o60 + k as u64, x).a(0o70 + k as u64, 0o400 + off);
        p.write_from(0o60 + k as u64, 0o70 + k as u64);
    }
    p.stop();
    let mut line = [0 as Word; 8];
    for (&x, off) in words.iter().zip([1usize, 3, 4, 6]) {
        line[off] = x;
    }
    let packed: Vec<u8> =
        line.iter().flat_map(|&x| (0..5).map(move |k| (x >> (8 * k)) as u8)).collect();
    for (engine, m) in run(&p) {
        assert_eq!(m.main[0o400..0o410], line, "{engine}: the words whole");
        let mut wr = muir::checkpoint::Writer::new();
        m.save(&mut wr);
        let body = wr.finish();
        assert!(
            body.windows(40).any(|x| x == packed),
            "{engine}: the line packed in the checkpoint"
        );
    }
}

/// **On `rtl` main memory above 22 bits and the frame buffer window go
/// through the cache** (G2 §3; G1 §3.2): a second read of either word
/// hits, where an address the port does not decode as memory would not be
/// cached at all.
#[test]
fn on_rtl_high_memory_and_the_window_are_cached() {
    let va = |m: &mut Machine| [through(m, 3, WINDOW), through(m, 4, HIGH)];
    let mut p = Prog::default();
    let mut probe = Machine::new();
    let [window, high] = va(&mut probe);
    p.a(0o60, window + 3).a(0o61, high + 3);
    for slot in [0o10, 0o11] {
        p.read_to(0o60, slot);
    }
    for slot in [0o12, 0o13] {
        p.read_to(0o61, slot);
    }
    p.stop();
    let setup = |m: &mut Machine| {
        m.main = vec![0; MAIN_13];
        va(m);
    };
    let mut r = Rtl::new(machine(&p, &setup));
    r.boot();
    let before = r.cache().unwrap().hits;
    finish(&mut r, "rtl");
    let c = r.cache().unwrap();
    assert!(c.hits - before >= 2, "the second reads hit: {} hits, {} misses", c.hits, c.misses);
    assert!(c.holds(WINDOW + 3) && c.holds(HIGH + 3), "both lines held");
}

// --- full HD and the board name ---------------------------------------------

/// The video controller at full HD, 1920 by 1080: 60 words a line, 64,800
/// words, `1760000000`-`1760176437`.
const FULL_HD: (usize, usize) = (1920, 1080);
const FULL_HD_WORDS: u32 = 64_800;

/// The video controller fitted at `size`.
fn video_at(m: &mut Machine, (w, h): (usize, usize)) {
    m.tv.set_board(Board::Video);
    m.tv.set_video_size(w, h);
}

/// **At 1920 by 1080 the window ends at its 64,800th word** (G2 §4.3, the
/// full-HD contract §1.2): the last word, `1760176437`, is written and read
/// back with the unboxed tag; the next, `1760176440`, is nothing, and sets
/// word 101 `<0>`, which the reads before it have left clear; feature words
/// 11-13 say 1920 by 1080, one bit a pixel and 60 words a line, and the
/// window's address.
#[test]
fn full_hd_s_window_ends_at_64800_words() {
    assert_eq!(FULL_HD_WORDS, 0o176440);
    let last = WINDOW + FULL_HD_WORDS - 1;
    let va = |m: &mut Machine| [through(m, 1, REGISTER_PAGE), through(m, 3, last & !0o1777)];
    let mut p = Prog::default();
    let mut probe = Machine::new();
    let [reg, page] = va(&mut probe);
    let off = |phys: u32| page + (phys & 0o1777) as Word;
    for (k, word) in [0o11u64, 0o12, 0o13].into_iter().enumerate() {
        p.a(0o60 + k as u64, reg + word);
        p.read_to(0o60 + k as u64, 0o10 + k as u64);
    }
    p.a(0o63, off(last)).a(0o64, w(0o025, 0x1234_5678)).a(0o65, off(last + 1));
    p.a(0o66, reg + 0o101);
    p.write_from(0o64, 0o63).read_to(0o63, 0o13).read_to(0o66, 0o14);
    p.read_to(0o65, 0o15).read_to(0o66, 0o16);
    p.stop();
    let setup = |m: &mut Machine| {
        va(m);
        video_at(m, FULL_HD);
    };
    let ms = run_with(&p, &setup, &|_| {});
    expect(
        &ms,
        &[
            (0o10, 1920 << 16 | 1080, "word 11: 1920 by 1080"),
            (0o11, 1 << 16 | 60, "word 12: one bit, 60 words a line"),
            (0o12, WINDOW as Word, "word 13: the window"),
            (0o13, w(0o005, 0x1234_5678), "1760176437: the field, tag 005"),
            (0o14, 0, "word 101: no NXM yet"),
            (0o15, 0, "1760176440: nothing"),
            (0o16, 1, "word 101: NXM"),
        ],
    );
    for (engine, m) in &ms {
        assert_eq!(m.tv.buffer_words(), FULL_HD_WORDS, "{engine}: the buffer's words");
        let last_word = m.tv.read_buffer(FULL_HD_WORDS - 1);
        assert_eq!(last_word, 0x1234_5678, "{engine}: the buffer's last word holds the field");
    }
}

/// The feature words of a name, 4 characters a word in `<31:0>`, the low
/// byte first, zero after its end: words 20-24, and word 25.
fn name_words(name: &str) -> [Word; 6] {
    let mut words = [0; 6];
    for (i, c) in name.bytes().enumerate() {
        words[i / 4] |= Word::from(c) << (8 * (i % 4));
    }
    words
}

/// Feature words 20-25 on both engines, read by a program after `setup`,
/// into M 10-15.
fn board_name_words(setup: &dyn Fn(&mut Machine)) -> [(&'static str, Machine); 2] {
    let va = |m: &mut Machine| through(m, 1, REGISTER_PAGE);
    let mut p = Prog::default();
    let reg = va(&mut Machine::new());
    for k in 0..6u64 {
        p.a(0o60 + k, reg + 0o20 + k);
        p.read_to(0o60 + k, 0o10 + k);
    }
    p.stop();
    run_with(
        &p,
        &|m| {
            va(m);
            setup(m);
        },
        &|_| {},
    )
}

/// The board name the engines' M 10-15 hold, as `expect` checks it.
fn expect_name(ms: &[(&str, Machine)], name: &str) {
    let words = name_words(name);
    let rows: Vec<(u64, Word, String)> = (0..6)
        .map(|k| (0o10 + k as u64, words[k], format!("word {:o} of {name:?}", 0o20 + k)))
        .collect();
    let rows: Vec<(u64, Word, &str)> = rows.iter().map(|(s, v, t)| (*s, *v, t.as_str())).collect();
    expect(ms, &rows);
}

/// **The board name, feature words 20-24** (the full-HD contract §6.4,
/// Q-HD7): on both engines muir-sim names itself "muir-sim", 4 characters
/// a word in `<31:0>`, the low byte first, ended by a zero byte, and word
/// 25 reads 0. Below revision 13 the words read 0
/// (`tests/quux_registers.rs`).
#[test]
fn the_board_name_is_muir_sim_on_both_engines() {
    let ms = board_name_words(&|_| {});
    assert_eq!(name_words("muir-sim")[0], 0x7269_756d, "\"muir\", m in <7:0>");
    expect_name(&ms, "muir-sim");
    for (engine, m) in &ms {
        assert_eq!(m.mmem[0o12], 0, "{engine}: word 22 ends the name with zero bytes");
    }
}

/// **`set_board_name` sets the name a fabric is built with** (§6.6): 20
/// characters fill words 20-24 with no zero byte, word 24 holding
/// characters 17-20; 9 characters end in word 22, whose `<15:8>` is the
/// zero byte, and every byte after it is zero. Both engines read it.
#[test]
fn set_board_name_fills_words_20_to_24() {
    let twenty = "QUUX on a test board";
    assert_eq!(twenty.len(), 20);
    let ms = board_name_words(&|m| m.set_board_name(twenty).unwrap());
    expect_name(&ms, twenty);
    for (engine, m) in &ms {
        let bytes: Vec<u8> = (0o10..0o15).flat_map(|k| (m.mmem[k] as u32).to_le_bytes()).collect();
        assert_eq!(bytes, twenty.as_bytes(), "{engine}: the 20 characters, no zero byte");
        assert_eq!(m.mmem[0o14], Word::from(u32::from_le_bytes(*b"oard")), "{engine}: 17-20");
        assert_eq!(m.mmem[0o15], 0, "{engine}: word 25");
    }

    let ms = board_name_words(&|m| m.set_board_name("DE25-Nano").unwrap());
    expect_name(&ms, "DE25-Nano");
    for (engine, m) in &ms {
        assert_eq!(m.mmem[0o12], Word::from(b'o'), "{engine}: word 22, the ninth character");
        assert_eq!(m.mmem[0o12] >> 8 & 0o377, 0, "{engine}: word 22 <15:8>, the end");
        assert_eq!((m.mmem[0o13], m.mmem[0o14]), (0, 0), "{engine}: words 23 and 24");
    }
}

/// **A name is at most 20 characters of printable ASCII, `040`-`176`**
/// (§6.4): longer, or with any other byte, it is refused and the name
/// stays as it was; `040` and `176` themselves are taken.
#[test]
fn set_board_name_refuses_what_does_not_fit() {
    let mut m = Machine::new();
    m.geometry = REV13;
    let page = |m: &mut Machine| -> Vec<u32> {
        (0o20..=0o25).map(|k| m.bus_read(REGISTER_PAGE + k) as u32).collect()
    };
    let default = page(&mut m);
    for bad in ["QUUX on a test board!", "tab\there", "nul\0", "del\x7f", "newline\n", "caf\u{e9}"]
    {
        let e = m.set_board_name(bad).unwrap_err();
        assert!(!e.is_empty(), "{bad:?} refused with a reason");
        assert_eq!(page(&mut m), default, "{bad:?}: the name stays");
    }
    m.set_board_name(" ~").unwrap();
    assert_eq!(page(&mut m)[0], 0x7e20, "040 and 176 are taken");
}
