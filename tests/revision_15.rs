// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **QUUX revision 15** (contract G3 revision 15, with its appendix A15b),
//! on `micro`, the one engine that runs it: the 64-bit microinstruction and
//! its extension (A15b.2), `WRITE-I-MEM` at 64 bits (A15b.4), its `.mcr` and
//! every refusal of its reader (A15b.7), its identity, feature word 25 and
//! register-page word 225 (A15b.1), and its checkpoint (A15b.13). Hand-written
//! programs and hand-built files: no microcode or PROM for revision 15
//! exists in this tree.
//!
//! Each test names, in its comment, the partial implementation it fails.
//! The words are 40 bits, G1's: the data type `<37:32>` over the field
//! `<31:0>`. Addresses are octal, as A15b writes them.

use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{
    ALU, ALWAYS, DISPATCH, JUMP, LDB, MD, N, P, POPJ, R, SETA, SETM, SRC_MD, START_READ,
    START_WRITE, a_dest, a_src, filler, m_dest, m_src, pdl_field, predicted, src, target,
};
use muir::isa::asm::{CARRY_IN, DMEM_WRITE, M_PLUS_C, OA_HIGH_SELECT as SH, OA_LOW_SELECT as SL};
use muir::machine::{Geometry, Halt, Machine, Word};
use muir::mcr::{self, Holds};
use muir::micro::Micro;

mod support;

const REV15: Geometry = Geometry::QUUX_15;

/// A memory's constants: 0, 1.
const ZERO: u64 = 0o40;
const ONE: u64 = 0o41;
/// M memory's 0 and 1.
const M_ZERO: u64 = 0o34;
const M_ONE: u64 = 0o35;

/// The program's last word, a jump to itself.
const STOP: u64 = 0o1000;

/// The fixnum's data type, `005`.
const FIX: Word = 0o005 << 32;

/// The register page's virtual address on revisions 14 and 15, in the
/// device window: `35777777400`.
const REGISTER_PAGE: Word = 0o35777777400;

/// A functional destination, with M's address 36 as the scratch word.
const fn fd(code: u64) -> u64 {
    code << 19 | 0o36 << 14
}
/// Destination 13, PDL-INDEX; 14, the PDL pointer.
const PDL_INDEX: u64 = fd(0o13);
const PDL_POINTER: u64 = fd(0o14);

/// A BYTE word: function, rotate `IR<5:0>` and length − 1 `IR<11:6>`.
fn byte(func: u64, rotate: u64, len: u64) -> u64 {
    muir::isa::asm::BYTE | func | (len - 1) << 6 | rotate
}
/// A JUMP on condition `code`, `IR<4:0>` with `IR<5>`.
fn jcond(code: u64) -> u64 {
    JUMP | 1 << 5 | code
}
/// A DISPATCH: address `IR<23:12>`, no bits of M.
fn disp(addr: u64) -> u64 {
    DISPATCH | addr << 12
}

/// A program under construction: 64-bit words.
#[derive(Default, Clone)]
struct Prog {
    words: Vec<u64>,
    amem: Vec<(u64, Word)>,
    mmem: Vec<(u64, Word)>,
    dmem: Vec<(usize, u32)>,
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
    /// An A-memory constant, from 101 up, above A 100, which every
    /// [`filler`] writes: its address.
    fn k(&mut self, v: Word) -> u64 {
        let a = 0o101 + self.next_a;
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
    /// M `slot` <- 1 if the JUMP `jump` is taken, 0 if not.
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
    /// The same program with every word's extension cleared.
    fn without_extension(&self) -> Prog {
        let mut p = self.clone();
        for w in &mut p.words {
            *w &= (1 << 48) - 1;
        }
        p
    }
}

/// The machine of `geometry` for `p`, `setup` last.
fn machine(p: &Prog, geometry: Geometry, setup: &dyn Fn(&mut Machine)) -> Machine {
    let mut prom: Vec<Insn> = p.words.iter().map(|&w| Insn::extended(w)).collect();
    assert!(prom.len() <= STOP as usize, "the program runs into its stop");
    prom.resize(STOP as usize, filler());
    prom.push(Insn::new(JUMP | target(STOP) | ALWAYS | N));
    prom.resize(1024, filler());
    let mut m = Machine::new();
    m.geometry = geometry;
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
    setup(&mut m);
    m
}

/// Runs `p` on `micro` to its stop, and 16 microcycles on, `after_boot`
/// applied after the boot: the machine, or the halt it stopped at.
fn run_on(
    p: &Prog,
    geometry: Geometry,
    after_boot: &dyn Fn(&mut Machine),
) -> Result<Micro, (Halt, Box<Micro>)> {
    let mut u = Micro::new(machine(p, geometry, &|_| {}));
    u.boot();
    after_boot(u.machine_mut());
    for _ in 0..20_000 {
        if u.machine().opc == STOP as u16 {
            let (_, halt) = u.run(16);
            return match halt {
                Some(h) => Err((h, Box::new(u))),
                None => Ok(u),
            };
        }
        if let Err(h) = u.step() {
            return Err((h, Box::new(u)));
        }
    }
    panic!("the program never reached its stop");
}

/// Runs `p` on revision 15 to its stop, expecting no halt.
fn run(p: &Prog) -> Machine {
    match run_on(p, REV15, &|_| {}) {
        Ok(u) => u.machine().clone(),
        Err((h, _)) => panic!("halted: {h:?}"),
    }
}

/// M memory's `slot`s are `want`.
fn expect(m: &Machine, rows: &[(u64, Word, &str)]) {
    for &(slot, want, what) in rows {
        let got = m.mmem[slot as usize];
        assert_eq!(got, want, "M {slot:o}, {what}: {got:#012x}, not {want:#012x}");
    }
}

// --- identity (A15b.1) --------------------------------------------------------

/// **Feature word 0 says revision 15, and word 25 the microcycle in units
/// of 0.5 ns** (A15b.1): `micro`'s period is the Kria's 8.5 ns when nothing
/// says otherwise (MP2b ruling Q14), so 17; 40 ns gives 80 (Q15). On
/// revision 14 word 25 reads 0, as every unassigned feature word does (the
/// control). **Fails** a MACHINE-ID left at 14, a word 25 that is not the
/// engine's period, and one in whole ns doubled, which cannot say 17.
#[test]
fn feature_words_0_and_25_say_revision_15_and_its_period() {
    let mut p = Prog::default();
    p.read(REGISTER_PAGE, 0o20).read(REGISTER_PAGE | 0o25, 0o21).stop();
    let m = run(&p);
    expect(&m, &[(0o20, 0x5155_00f4, "MACHINE-ID"), (0o21, 17, "word 25 at 8.5 ns")]);
    let mut u = Micro::new(machine(&p, REV15, &|_| {}));
    u.period = 80;
    u.boot();
    while u.machine().opc != STOP as u16 {
        u.step().unwrap();
    }
    u.run(16);
    expect(u.machine(), &[(0o21, 80, "word 25 at 40 ns")]);
    let m = run_on(&p, Geometry::QUUX_14, &|_| {}).ok().unwrap();
    expect(m.machine(), &[(0o20, 0x5155_00e4, "revision 14"), (0o21, 0, "no word 25")]);
}

/// **Register-page word 225** (A15b.1, A15b.5): the posted writes answered
/// with an error, read, and cleared by a write; `-RESET` clears it, as word
/// 224. `micro` posts nothing, so a count is planted after the boot. On
/// revision 14 word 225 is reserved and reads 0 whatever the count (the
/// control). **Fails** a word 225 that does not read the count, or that a
/// write leaves.
#[test]
fn word_225_reads_the_errors_and_a_write_clears_it() {
    let mut p = Prog::default();
    p.read(REGISTER_PAGE | 0o225, 0o20).write(0, REGISTER_PAGE | 0o225);
    p.read(REGISTER_PAGE | 0o225, 0o21).stop();
    let plant = |m: &mut Machine| m.posted_write_errors = 3;
    let u = run_on(&p, REV15, &plant).ok().unwrap();
    expect(u.machine(), &[(0o20, 3, "the count"), (0o21, 0, "cleared by a write")]);
    let u = run_on(&p, Geometry::QUUX_14, &plant).ok().unwrap();
    expect(u.machine(), &[(0o20, 0, "reserved on 14")]);
    let mut u = Micro::new(machine(&p, REV15, &|_| {}));
    u.machine_mut().posted_write_errors = 3;
    u.boot();
    assert_eq!(u.machine().posted_write_errors, 0, "-RESET clears it");
}

// --- the extension (A15b.2) ------------------------------------------------

/// A program of every class with its extension's fields set: hinted
/// conditional jumps taken and not, dispatches predicted as each of A15b.2's
/// four rows, right and wrong, and ALU and BYTE words with a correct PDL
/// address field on each base.
fn every_class_extended() -> Prog {
    let mut p = Prog::default();
    // The bases: the PDL pointer 100, PDL-INDEX 200.
    let (ptr, idx) = (p.k(0o100), p.k(0o200));
    p.op(ALU | SETA | a_src(ptr) | PDL_POINTER);
    p.op(ALU | SETA | a_src(idx) | PDL_INDEX);
    // Conditional jumps, hinted each way: M 0 = 0 taken, M 1 = 0 not.
    p.taken(jcond(3) | m_src(M_ZERO) | a_src(ZERO) | muir::isa::asm::HINT, 0o20);
    p.taken(jcond(3) | m_src(M_ONE) | a_src(ZERO) | muir::isa::asm::HINT, 0o21);
    p.taken(jcond(3) | m_src(M_ONE) | a_src(ZERO), 0o22);
    // A dispatch whose entry jumps, with N, predicted wrong (a return), and
    // one whose entry calls a subroutine that returns, predicted right.
    let jump_to = p.at() + 4;
    p.dmem.push((0o100, jump_to as u32 | 1 << 14));
    p.op(disp(0o100) | predicted(0o1234, false, true));
    p.fill(1);
    p.set(1, 0o23);
    p.set(2, 0o23);
    // Here, at jump_to.
    p.set(3, 0o24);
    let sub = 0o600;
    p.dmem.push((0o101, sub as u32 | 1 << 15 | 1 << 14));
    p.op(disp(0o101) | predicted(sub, true, false));
    p.fill(1);
    // A drop-through, predicted as a call, and one predicted as one.
    p.dmem.push((0o102, 1 << 16 | 1 << 15));
    p.op(disp(0o102) | predicted(0o777, true, false));
    p.op(disp(0o102));
    // PDL address fields, right on each base: M-AP and A-LOCALP are the
    // machine's copies, 0 after the boot.
    let (a110, a70, a5) = (p.k(0o110), p.k(0o70), p.k(5));
    p.op(ALU | SETA | a_src(a110) | PDL_INDEX | pdl_field(2, 0o10));
    p.op(ALU | SETA | a_src(a70) | PDL_INDEX | pdl_field(3, -0o20));
    p.op(ALU | SETA | a_src(a5) | PDL_INDEX | pdl_field(0, 5));
    p.op(byte(LDB, 0, 32) | m_src(M_ONE) | PDL_INDEX | pdl_field(1, 1));
    // M 25 <- PDL-INDEX.
    p.op(ALU | SETM | src(0o3) | m_dest(0o25));
    p.stop();
    // The subroutine: mark, and return.
    while p.at() < sub {
        p.fill(1);
    }
    p.set(4, 0o26);
    p.op(ALU | SETA | a_src(ZERO) | m_dest(0o27) | POPJ);
    p.fill(1);
    p
}

/// **The extension changes no result on `micro`** (A15b.2, A15b.8): a
/// program of every class with its fields set ends as the same program with
/// every extension zero, M, A, the PDL buffer and its registers, the micro
/// stack, `MD` and `VMA` alike; and it ran its paths. **Fails** an engine
/// on which a hint, a predicted target or a right PDL address field changes
/// a result. `micro` clears the extension before the word runs, and every
/// decoder it has masks its own field, so leaving the extension in changes
/// nothing today either: the test holds the property, not that line.
#[test]
fn the_extension_changes_no_result() {
    let p = every_class_extended();
    assert!(p.words.iter().any(|w| w >> 48 != 0), "the program carries an extension");
    let (with, without) = (run(&p), run(&p.without_extension()));
    expect(
        &with,
        &[
            (0o20, 1, "a hinted jump taken"),
            (0o21, 0, "a hinted jump not taken"),
            (0o22, 0, "an unhinted jump not taken"),
            (0o23, 0, "the dispatch's N"),
            (0o24, 3, "the dispatch's jump"),
            (0o26, 4, "the dispatch's call"),
            (0o25, 1, "PDL-INDEX from the last field"),
        ],
    );
    assert_eq!(with.mmem, without.mmem, "M");
    assert_eq!(with.amem, without.amem, "A");
    assert_eq!(with.pdl, without.pdl, "the PDL buffer");
    assert_eq!(
        (with.pdl_pointer, with.pdl_index, with.spcptr, with.spc, with.md, with.vma),
        (
            without.pdl_pointer,
            without.pdl_index,
            without.spcptr,
            without.spc,
            without.md,
            without.vma
        ),
        "the registers and the micro stack"
    );
}

/// **A PDL address field planted wrong halts at PDL-FIELD-MISMATCH**
/// (A15b.2), after its word, on each base: the displacement one off. The
/// right one runs to the end (the control, above and here). **Fails** an
/// engine that ignores the field, and one that checks against the base
/// after the word rather than before it.
#[test]
fn a_wrong_pdl_field_halts_at_pdl_field_mismatch() {
    for (base, displacement, value) in [(2, 0o10, 0o110), (3, -0o20, 0o160), (0, 5, 5)] {
        for off in [0, 1] {
            let mut p = Prog::default();
            let (ptr, idx) = (p.k(0o100), p.k(0o200));
            p.op(ALU | SETA | a_src(ptr) | PDL_POINTER);
            p.op(ALU | SETA | a_src(idx) | PDL_INDEX);
            let at = p.at();
            let v = p.k(value);
            p.op(ALU | SETA | a_src(v) | PDL_INDEX | pdl_field(base, displacement + off));
            p.stop();
            match (off, run_on(&p, REV15, &|_| {})) {
                (0, Ok(u)) => assert_eq!(u.machine().pdl_index, value as u16, "base {base}"),
                (1, Err((h, _))) => assert_eq!(
                    h,
                    Halt::PdlFieldMismatch {
                        pc: at as u16,
                        formed: (value + 1) as u16 & 0o37777,
                        written: value as u16
                    },
                    "base {base}"
                ),
                (_, Ok(_)) => panic!("base {base}: a wrong field ran"),
                (_, Err((h, _))) => panic!("base {base}: a right field halted: {h:?}"),
            }
        }
    }
}

// --- the control store's writes (A15b.4) --------------------------------------

/// **`WRITE-I-MEM` at 64 bits** (A15b.4): `IWR<63:32>` from `A<31:0>` and
/// `IWR<31:0>` from `M<31:0>`, so a word with `<63:48>` set reads back whole,
/// and a fixnum's tag left in A, `005` in `<37:32>`, reaches no bit of the
/// extension. Revision 14 takes `A<15:0>` (the control). **Fails** a 48-bit
/// path, and one that takes A's tag.
#[test]
fn write_i_mem_writes_64_bits_and_no_tag() {
    let high: Word = FIX | 0x2301_4567;
    let low: Word = 0x89ab_cdef;
    let mut p = Prog::default();
    let (a, m) = (p.k(high), 0o30);
    p.mmem.push((m, low));
    // A JUMP with P and R writes the control store at its target.
    p.op(JUMP | P | R | N | ALWAYS | target(0o700) | a_src(a) | m_src(m));
    p.fill(2);
    p.stop();
    let u = run_on(&p, REV15, &|_| {}).ok().unwrap();
    assert_eq!(u.machine().imem[0o700].raw(), 0x2301_4567_89ab_cdef, "revision 15");
    let u = run_on(&p, Geometry::QUUX_14, &|_| {}).ok().unwrap();
    assert_eq!(u.machine().imem[0o700].raw(), 0x4567_89ab_cdef, "revision 14");
}

// --- the checkpoint (A15b.13) ---------------------------------------------------

/// **A revision-15 checkpoint records revision 15 and keeps the control
/// store's 64 bits** (A15b.13), word 225 and the extension of the words in
/// the pipeline; a revision-14 one records 14. **Fails** a checkpoint that
/// records revision 15 as 14, or truncates a word to 48 bits.
#[test]
fn a_checkpoint_records_revision_15_and_keeps_64_bits() {
    let mut p = Prog::default();
    p.fill(4).stop();
    let mut u = Micro::new(machine(&p, REV15, &|_| {}));
    u.boot();
    u.machine_mut().imem[0o700] = Insn::extended(0xc123_4567_89ab_cdef);
    u.run(3);
    u.machine_mut().posted_write_errors = 7;
    let mut wr = muir::checkpoint::Writer::new();
    u.save(&mut wr);
    let body = wr.finish();
    let g = Machine::checkpointed_geometry_at(&body, 40).unwrap();
    assert_eq!(g.revision(), Some(15), "the revision recorded");
    let mut v = Micro::new(machine(&p, REV15, &|_| {}));
    v.load(&mut muir::checkpoint::Reader::for_word_bits(&body, 40)).unwrap();
    assert_eq!(v.machine().imem[0o700].raw(), 0xc123_4567_89ab_cdef, "64 bits");
    assert_eq!(v.machine().posted_write_errors, 7, "word 225");
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX_14;
    let mut wr = muir::checkpoint::Writer::new();
    m.save(&mut wr);
    let g = Machine::checkpointed_geometry_at(&wr.finish(), 40).unwrap();
    assert_eq!(g.revision(), Some(14), "revision 14's");
}

// --- the `.mcr` (A15b.7) -------------------------------------------------------

/// A section of a revision-15 `.mcr`: its header and its items, each as
/// the words it is stored in, least significant first.
#[derive(Clone)]
struct Section {
    header: [u32; 8],
    items: Vec<Vec<u32>>,
}

/// A section of `kind` from `start`, its items at `actual` bits in
/// `storage`.
fn section(kind: u32, start: u32, actual: u32, storage: u32, items: &[u64]) -> Section {
    let per = (storage / 32) as usize;
    Section {
        header: [kind, items.len() as u32, actual, storage, start, 0, 0, 0],
        items: items
            .iter()
            .map(|&v| (0..per).map(|i| if i < 2 { (v >> (32 * i)) as u32 } else { 0 }).collect())
            .collect(),
    }
}

/// The file: the format word, `count` (the number of sections unless
/// given), the sections, and zeros to a whole number of 1,024-byte blocks.
fn file(sections: &[Section], count: Option<u32>) -> Vec<u8> {
    let mut w = vec![mcr::FORMAT_WORD, count.unwrap_or(sections.len() as u32)];
    for s in sections {
        w.extend_from_slice(&s.header);
        for i in &s.items {
            w.extend_from_slice(i);
        }
    }
    let mut b: Vec<u8> = w.iter().flat_map(|x| x.to_le_bytes()).collect();
    b.resize(b.len().div_ceil(mcr::BLOCK_BYTES) * mcr::BLOCK_BYTES, 0);
    b
}

/// A JUMP to itself at `pc`, with a hint in its extension, which changes
/// nothing on `micro`.
fn jump_self(pc: u64) -> u64 {
    JUMP | target(pc) | ALWAYS | N | muir::isa::asm::HINT
}

/// The sections of a revision-15 microcode file: the revision, a control
/// store whose words have extensions, a dispatch memory entry with odd
/// parity, the symbol area's fixnum 6022 at 6000, and A memory's version
/// at 40.
fn microcode_sections() -> Vec<Section> {
    vec![
        section(6, 0, 32, 32, &[15]),
        section(1, 0o100, 64, 64, &[jump_self(0o100), 0xfff0_0000_0000_0001]),
        section(2, 0o20, 18, 32, &[0o12345 | 1 << 17]),
        section(3, 0o6000, 40, 64, &[FIX | 0o6022]),
        section(4, 0o40, 40, 64, &[FIX | 2002, 0o377_1234_5670]),
    ]
}

/// The sections of a revision-15 boot PROM: the revision and its words from
/// 36000.
fn prom_sections() -> Vec<Section> {
    vec![section(6, 0, 32, 32, &[15]), section(1, 0o36000, 64, 64, &[jump_self(0o36000)])]
}

/// **A revision-15 `.mcr` reads whole** (A15b.7): every section's items at
/// their addresses, the control store's 64 bits and the symbol area's
/// fixnum among them, for microcode; the PROM's words at 36000. These are
/// the controls of the refusals below. **Fails** a reader that drops the
/// extension, the symbol area or the parity bit.
#[test]
fn a_revision_15_mcr_reads_whole() {
    let m = mcr::parse_quux_microcode(&file(&microcode_sections(), None), REV15).unwrap();
    assert_eq!(m.hardware_revision, Some(15));
    assert_eq!(m.format, Some(1));
    assert_eq!(m.imem_start, 0o100);
    let imem: Vec<u64> = m.imem.iter().map(|i| i.raw()).collect();
    assert_eq!(imem, [jump_self(0o100), 0xfff0_0000_0000_0001]);
    assert_eq!((m.dmem_start, m.dmem.clone()), (0o20, vec![0o12345 | 1 << 17]));
    assert_eq!(m.symbol_area, Some((0o6000, vec![FIX | 0o6022])));
    assert_eq!((m.amem_start, m.amem.clone()), (0o40, vec![FIX | 2002, 0o377_1234_5670]));
    assert_eq!(m.version(), Some(2002));
    let prom = muir::prom::parse_quux_mcr(&file(&prom_sections(), None), REV15).unwrap();
    assert_eq!(prom.len(), 1024);
    assert_eq!(prom[0].raw(), jump_self(0o36000));
    assert!(prom[1..].iter().all(|w| w.raw() == 0));
}

/// The refusal `bytes` must meet as microcode on revision 15, naming
/// `says`.
fn refused(bytes: &[u8], holds: Holds, says: &str) {
    let e = match holds {
        Holds::Microcode => mcr::parse_quux_microcode(bytes, REV15).map(|_| ()),
        Holds::Prom => muir::prom::parse_quux_mcr(bytes, REV15).map(|_| ()),
    }
    .expect_err(says);
    assert!(e.contains(says), "the refusal says {says:?}: {e}");
}

/// The microcode's sections with `f` applied, as a file.
fn planted(f: impl Fn(&mut Vec<Section>)) -> Vec<u8> {
    let mut s = microcode_sections();
    f(&mut s);
    file(&s, None)
}

/// **Every refusal of A15b.7, each on a planted file**, against the whole
/// file above that reads (`a_revision_15_mcr_reads_whole`): the format word;
/// a first section other than 6, and section 6 of another shape or another
/// revision; an unknown type (0, 5, 7), a type twice, the order broken; an
/// actual width not the machine's, a storage width not a multiple of 32 or
/// below the actual; a padding bit in an item, in the item's word and in a
/// word past it; a non-zero unused header word; a section past its memory,
/// microcode at or past the PROM's 36000, a PROM not at 36000 or past its
/// 2000 words; sections that do not end where the count says, a non-zero
/// word after the last, a length not a whole number of blocks. **Fails** a
/// reader that leaves any one out.
#[test]
fn every_mcr_refusal_on_a_planted_file() {
    use Holds::{Microcode, Prom};
    let mut b = file(&microcode_sections(), None);
    b[0] ^= 2;
    refused(&b, Microcode, "format word 51550003, not 51550001");
    let b = planted(|s| s.swap(0, 1));
    refused(&b, Microcode, "the first section is type 1");
    let b = planted(|s| s[0] = section(6, 0, 32, 32, &[14]));
    refused(&b, Microcode, "section 6 says hardware revision 14, and this is revision 15");
    let b = planted(|s| s[0] = section(6, 0, 32, 32, &[15, 15]));
    refused(&b, Microcode, "section 6 starts at 0 for 2 items");
    let b = planted(|s| s[0] = section(6, 1, 32, 32, &[15]));
    refused(&b, Microcode, "section 6 starts at 1");
    for t in [0, 5, 7] {
        let b = planted(|s| s[2].header[0] = t);
        refused(&b, Microcode, &format!("unknown type {t}"));
    }
    let b = planted(|s| {
        let d = s[2].clone();
        s.insert(3, d);
    });
    refused(&b, Microcode, "type 2 after type 2");
    let b = planted(|s| s.swap(1, 2));
    refused(&b, Microcode, "type 1 after type 2");
    let b = planted(|s| s[1] = section(1, 0o100, 48, 64, &[1]));
    refused(&b, Microcode, "actual width 48 bits, and the machine's is 64");
    let b = planted(|s| s[4] = section(4, 0o40, 32, 32, &[1]));
    refused(&b, Microcode, "actual width 32 bits, and the machine's is 40");
    let b = planted(|s| s[2] = section(2, 0o20, 18, 48, &[1]));
    refused(&b, Microcode, "storage width 48 bits");
    let b = planted(|s| s[1] = section(1, 0o100, 64, 32, &[1]));
    refused(&b, Microcode, "storage width 32 bits");
    let b = planted(|s| s[2].items[0][0] |= 1 << 18);
    refused(&b, Microcode, "section type 2's item 0: a padding bit above bit 17");
    let b = planted(|s| s[3].items[0][1] |= 1 << 8);
    refused(&b, Microcode, "section type 3's item 0: a padding bit above bit 39");
    let b = planted(|s| {
        s[1] = section(1, 0o100, 64, 96, &[1]);
        s[1].items[0][2] = 1;
    });
    refused(&b, Microcode, "section type 1's item 0: a padding bit");
    for k in 5..8 {
        let b = planted(|s| s[4].header[k] = 1);
        refused(&b, Microcode, &format!("header word {k} is 1, unused and not zero"));
    }
    let b = planted(|s| s[2].header[4] = 0o10000);
    refused(&b, Microcode, "past the dispatch memory's 10000");
    let b = planted(|s| s[4].header[4] = 0o1777);
    refused(&b, Microcode, "past A memory's 2000");
    let b = planted(|s| s[1].header[4] = 0o35777);
    refused(&b, Microcode, "past the PROM's 36000");
    let b = planted(|s| s[1].header[4] = 0o36000);
    refused(&b, Microcode, "past the PROM's 36000");
    // The PROM's own file.
    let mut s = prom_sections();
    s[1].header[4] = 0o35000;
    refused(&file(&s, None), Prom, "the PROM's own file starts at 36000");
    let words: Vec<u64> = (0..1025).map(|_| filler().raw()).collect();
    let s = vec![section(6, 0, 32, 32, &[15]), section(1, 0o36000, 64, 64, &words)];
    refused(&file(&s, None), Prom, "past the PROM's 40000");
    // Where the count says the sections end.
    let s = microcode_sections();
    refused(&file(&s, Some(4)), Microcode, "a non-zero word at offset");
    refused(&file(&s, Some(6)), Microcode, "unknown type 0");
    let mut b = file(&s, None);
    b.truncate(b.len() - 4);
    refused(&b, Microcode, "not a whole number of 1024-byte blocks");
    let mut b = file(&s, None);
    let last = b.len() - 4;
    b[last] = 1;
    refused(&b, Microcode, "a non-zero word at offset");
    let big: Vec<u64> = vec![0; 300];
    let mut s = microcode_sections();
    s[1] = section(1, 0o100, 64, 64, &big);
    let mut b = file(&s, None);
    b.truncate(mcr::BLOCK_BYTES);
    refused(&b, Microcode, "runs past the file's end");
}

/// **The format names the revision** (A15b.1): a revision-15 file is
/// refused on revisions 13 and 14 and on the CADR's PROM path, and MIT's
/// sections --- revision 14's microcode with section 6, PROM 2001 without
/// one --- on revision 15, each refusal naming the format and the revision.
/// The controls: each file is taken by its own machine. **Fails** a reader
/// that reads one format on the other's machine, or refuses without naming
/// them.
#[test]
fn each_format_is_refused_on_the_other_s_revisions() {
    let r15 = file(&microcode_sections(), None);
    for (g, n) in [(Geometry::QUUX, 13), (Geometry::QUUX_14, 14)] {
        let e = mcr::parse_quux_microcode(&r15, g).unwrap_err();
        assert!(
            e.contains("a revision-15 .mcr, format word 51550001, for hardware revision 15")
                && e.contains(&format!("this is revision {n}, which reads MIT's sections")),
            "{e}"
        );
        let e = muir::prom::parse_quux_mcr(&file(&prom_sections(), None), g).unwrap_err();
        assert!(e.contains(&format!("this is revision {n}")), "{e}");
    }
    // Revision 14's microcode: MIT's sections in partition order, section 6
    // first saying 14, then a control store word.
    let mut w14: Vec<u32> = vec![6, 0, 1, 14, 1, 0, 1, 0, 0, 4, 0o40, 1, 5];
    w14.resize(256, 0);
    let swap = |w: &[u32]| -> Vec<u8> { w.iter().flat_map(|x| x.to_le_bytes()).collect() };
    let b14 = swap(&w14);
    assert!(mcr::parse_quux_microcode(&b14, Geometry::QUUX_14).is_ok(), "14 takes its own");
    let e = mcr::parse_quux_microcode(&b14, REV15).unwrap_err();
    assert!(
        e.contains("MIT's sections, a file for hardware revision 14, and this is revision 15"),
        "{e}"
    );
    let e =
        muir::prom::parse_quux_mcr(include_bytes!("../data/quux-promh.mcr"), REV15).unwrap_err();
    assert!(
        e.contains("MIT's sections, a file for revision 13 or below, and this is revision 15"),
        "{e}"
    );
    assert!(mcr::parse_quux_microcode(&r15, REV15).is_ok(), "15 takes its own");
}

// --- the switch (A15b.1) ---------------------------------------------------------

/// The revision-15 PROM of [`prom_sections`] in a scratch file.
fn prom_file(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("prom-15.mcr");
    std::fs::write(&path, file(&prom_sections(), None)).unwrap();
    path
}

/// **`MUIR_QUUX_REVISION=15` runs revision 15 on `micro`** with a
/// revision-15 PROM: the start names it, and the checkpoint records it; and
/// on `rtl`, its pipeline. Refused: a run without `--prom`, since no
/// revision-15 PROM is built in and PROM 2001 is MIT's sections. **Fails**
/// a switch that builds revision 14, or an `rtl` that is not the pipeline.
#[test]
fn the_switch_runs_revision_15_on_both_engines() {
    use support::{Run, quux, scratch, text};
    let dir = scratch("revision-15");
    let prom = prom_file(&dir);
    let chk = dir.join("micro.chk");
    let out = quux()
        .env("MUIR_QUUX_REVISION", "15")
        .args(["--micro", "--stop-after", "10", "--prom"])
        .arg(&prom)
        .arg("--checkpoint")
        .arg(&chk)
        .run();
    let t = text(&out);
    assert!(out.status.success(), "the run failed:\n{t}");
    assert!(t.contains("machine: quux, revision 15: "), "{t}");
    let c = muir::checkpoint::read(&chk).unwrap();
    let g = Machine::checkpointed_geometry_at(&c.body, c.word_bits).unwrap();
    assert_eq!(g, REV15);
    // `rtl` runs it too: its pipeline (MP2b).
    let out = quux()
        .env("MUIR_QUUX_REVISION", "15")
        .args(["--rtl", "--stop-after", "10", "--prom"])
        .arg(&prom)
        .run();
    let t = text(&out);
    assert!(out.status.success(), "the rtl run failed:\n{t}");
    assert!(t.contains("rtl is its four-stage pipeline"), "{t}");
    let out = quux().env("MUIR_QUUX_REVISION", "15").args(["--micro", "--stop-after", "10"]).run();
    let t = text(&out);
    assert_eq!(out.status.code(), Some(2), "{t}");
    assert!(t.contains("QUUX revision 15 has no built-in boot PROM"), "{t}");
}

/// **Revision 15 refuses revisions 13's and 14's checkpoints, and they
/// refuse its** (A15b.1, A15b.13), naming the revision that wrote each; each
/// resumes its own. **Fails** a checkpoint that does not say 15, or a
/// refusal that lets 14 resume it.
#[test]
fn revision_15_and_the_others_refuse_each_other_s_checkpoints() {
    use support::{Run, quux, scratch, text};
    let dir = scratch("revision-15-checkpoint");
    let prom = prom_file(&dir);
    let chk = |r: &str| dir.join(format!("{r}.chk"));
    // A resume takes the checkpoint's PROM, so only a start names one.
    let start = |r: &str, fresh: bool| {
        let mut c = quux();
        c.env("MUIR_QUUX_REVISION", r).arg("--micro");
        if r == "15" && fresh {
            c.arg("--prom").arg(&prom);
        }
        c
    };
    for r in ["13", "14", "15"] {
        let out = start(r, true).args(["--stop-after", "100", "--checkpoint"]).arg(chk(r)).run();
        assert!(out.status.success(), "{r}: {}", text(&out));
        // Revision 15's file is version 51 (MP4 ruling Q3), the others' 50.
        let version = muir::checkpoint::read(&chk(r)).unwrap().version;
        assert_eq!(version, if r == "15" { 51 } else { 50 }, "{r}: the version");
    }
    for (ours, theirs) in [("15", "13"), ("15", "14"), ("13", "15"), ("14", "15")] {
        let c = chk(theirs);
        let out = start(ours, false).args(["--stop-after", "10", "--resume"]).arg(&c).run();
        let t = text(&out);
        let p = c.display();
        assert_eq!(out.status.code(), Some(2), "{ours} resumed {theirs}:\n{t}");
        assert!(
            t.contains(&format!(
                "--resume {p} is revision {theirs}'s, and this is revision {ours}: MUIR_QUUX_REVISION={theirs} quux --resume {p}"
            )),
            "{t}"
        );
    }
    let out = start("15", false).args(["--stop-after", "10", "--resume"]).arg(chk("15")).run();
    assert!(out.status.success(), "15 resumes 15: {}", text(&out));
}

// --- the OA registers and selects (A15b.15) -----------------------------------------

/// Destinations 16 and 17: OA-REG-LOW and OA-REG-HIGH, writing M 36 too.
const OA_LOW: u64 = fd(0o16);
const OA_HIGH: u64 = fd(0o17);

impl Prog {
    /// OA-REG-LOW (`high` false) or OA-REG-HIGH <- `v`.
    fn oa(&mut self, high: bool, v: Word) -> &mut Self {
        let a = self.k(v);
        self.op(ALU | SETA | a_src(a) | if high { OA_HIGH } else { OA_LOW })
    }
}

/// Runs `p` on `geometry` with the OA select check `check`: the machine,
/// and the halt if it stopped at one.
fn run_checked(p: &Prog, geometry: Geometry, check: bool) -> (Micro, Option<Halt>) {
    let mut u = Micro::new(machine(p, geometry, &|_| {}));
    u.oa_select_check = check;
    u.boot();
    for _ in 0..20_000 {
        if u.machine().opc == STOP as u16 {
            let (_, halt) = u.run(16);
            return (u, halt);
        }
        if let Err(h) = u.step() {
            return (u, Some(h));
        }
    }
    panic!("the program never reached its stop");
}

/// **An OA write followed by a word without a select runs that word
/// unmodified** (A15b.15): on revision 15 OA-REG-LOW's `<18:14>` leaves the
/// next word's M destination alone, where revision 14's IMOD ORs it in and
/// writes M 25 for M 20. The shadow check is off: the pair is the breach it
/// halts on (`the_shadow_check_halts_on_each_breach`). **Fails** a revision
/// 15 that keeps IMOD.
#[test]
fn an_oa_write_leaves_a_word_without_a_select_alone() {
    let mut p = Prog::default();
    p.oa(false, 0o5 << 14);
    p.op(ALU | SETA | a_src(ONE) | m_dest(0o20));
    p.stop();
    let (u, halt) = run_checked(&p, REV15, false);
    assert_eq!(halt, None);
    expect(u.machine(), &[(0o20, 1, "revision 15: M 20"), (0o25, 0, "revision 15: not M 25")]);
    let (u, halt) = run_checked(&p, Geometry::QUUX_14, false);
    assert_eq!(halt, None);
    expect(u.machine(), &[(0o20, 0, "revision 14: not M 20"), (0o25, 1, "revision 14: M 25")]);
}

/// **Each of the nine fields is taken through its select** (A15b.15), each
/// word right after its register's write, with the shadow check on: SL into
/// an ALU word's A destination and M destination, its ALU function, a BYTE
/// word's rotate and length, a JUMP's target and `WRITE-I-MEM`'s address,
/// a dispatch-memory write's address; SH into the A source and the M
/// source. Each result is the one the ORed field gives and the word alone
/// does not. **Fails** an OR left out of any one field, and a check that
/// halts on a right pair.
#[test]
fn each_of_the_nine_fields_is_taken_through_its_select() {
    let mut p = Prog::default();
    let (v777, vab, vff, vword, ventry) = (p.k(0o777), 0o30, 0o31, p.k(0o1234_5670), p.k(0o12345));
    p.mmem.push((vab, 0xab00));
    p.mmem.push((vff, 0xff));
    p.amem.push((0o307, 0o7654));
    // A destination: A 200 | 43, a bit above the M destination's.
    p.oa(false, 0o43 << 14).op(ALU | SETA | a_src(ONE) | a_dest(0o200) | SL);
    // M destination: M 10 | 5.
    p.oa(false, 0o5 << 14).op(ALU | SETA | a_src(ONE) | m_dest(0o10) | SL);
    // The ALU function: SETZ | 5, SETA.
    p.oa(false, 0o5 << 3).op(ALU | a_src(v777) | m_dest(0o21) | SL);
    // The rotate: an LDB of <15:8>, rotate 32.
    p.oa(false, 0o40).op(byte(LDB, 0, 8) | m_src(vab) | m_dest(0o22) | SL);
    // The length - 1: 0 | 7.
    p.oa(false, 0o7 << 6).op(byte(LDB, 0, 1) | m_src(vff) | m_dest(0o23) | SL);
    // The A source: A 0 | 307.
    p.oa(true, 0o307 << 6).op(ALU | SETA | a_src(0) | m_dest(0o24) | SH);
    // The M source: M 0 | 30.
    p.oa(true, vab).op(ALU | SETM | m_src(0) | m_dest(0o25) | SH);
    // A dispatch-memory write at 0 | 123.
    p.oa(false, 0o123 << 12).op(disp(0) | DMEM_WRITE | a_src(ventry) | SL);
    // WRITE-I-MEM at 0 | 650: A 0, M 0's word.
    p.op(ALU | SETA | a_src(vword) | m_dest(0));
    p.oa(false, 0o650 << 12).op(JUMP
        | P
        | R
        | N
        | ALWAYS
        | target(0)
        | a_src(ZERO)
        | m_src(0)
        | SL);
    p.fill(2);
    // The target: 0 | 700, which marks M 26.
    p.oa(false, 0o700 << 12).op(JUMP | ALWAYS | target(0) | N | SL);
    p.fill(1);
    p.stop();
    while p.at() < 0o700 {
        p.fill(1);
    }
    p.set(1, 0o26);
    p.stop();
    let (u, halt) = run_checked(&p, REV15, true);
    assert_eq!(halt, None, "a right pair halted");
    let m = u.machine();
    assert_eq!(m.amem[0o243], 1, "the A destination");
    expect(
        m,
        &[
            (0o10, 0, "not M 10"),
            (0o15, 1, "the M destination"),
            (0o21, 0o777, "the ALU function"),
            (0o22, 0xab, "the rotate"),
            (0o23, 0xff, "the length"),
            (0o24, 0o7654, "the A source"),
            (0o25, 0xab00, "the M source"),
            (0o26, 1, "the jump's target"),
        ],
    );
    assert_eq!(m.dmem[0o123], 0o12345, "the dispatch-memory write's address");
    assert_eq!(m.imem[0o650].raw(), 0o1234_5670, "WRITE-I-MEM's address");
}

/// **A select takes a value written three words before** (A15b.15): the
/// registers keep their word until the next write. The shadow check is off,
/// since IMOD would have dropped the value. **Fails** a register valid for
/// one word, or cleared by its reader.
#[test]
fn a_select_takes_a_value_written_three_words_before() {
    let mut p = Prog::default();
    p.oa(false, 0o5 << 14).fill(2);
    p.op(ALU | SETA | a_src(ONE) | m_dest(0o10) | SL);
    p.op(ALU | SETA | a_src(ONE) | m_dest(0o20) | SL);
    p.stop();
    let (u, halt) = run_checked(&p, REV15, false);
    assert_eq!(halt, None);
    expect(u.machine(), &[(0o15, 1, "three words on"), (0o25, 1, "and again")]);
}

/// **OA-OUTSIDE-FIELDS** (A15b.15): a register bit that is in neither the
/// word nor its class's fields halts before the word commits --- OA-REG-LOW
/// `<13>`, the output bus select, into an ALU word; OA-REG-HIGH's `<5>`,
/// `IR<31>`, which would make an M-memory source functional; OA-REG-HIGH's
/// M-source bits into a word reading a functional source. The controls: a
/// bit already set in the word, `<12>` of every ALU word, does not fire,
/// and the same M-source bits into an M-memory source are taken. **Fails**
/// a halt left out, and one that does not count the word's own bits.
#[test]
fn oa_outside_fields_fires_on_a_bit_outside_and_not_on_one_in_the_word() {
    let pair = |high: bool, v: Word, word: u64| {
        let mut p = Prog::default();
        p.oa(high, v);
        let at = p.at() as u16;
        p.op(word);
        p.stop();
        (at, run_checked(&p, REV15, true))
    };
    let (at, (u, halt)) = pair(false, 1 << 13, ALU | SETA | a_src(ONE) | m_dest(0o10) | SL);
    assert_eq!(halt, Some(Halt::OaOutsideFields { pc: at, bits: 1 << 13 }));
    assert_eq!(u.machine().mmem[0o10], 0, "the word did not commit");
    let (_, (u, halt)) = pair(false, 1 << 12, ALU | SETA | a_src(ONE) | m_dest(0o10) | SL);
    assert_eq!(halt, None, "a bit in the word");
    expect(u.machine(), &[(0o10, 1, "the word ran as written")]);
    let (at, (_, halt)) = pair(true, 1 << 5, ALU | SETM | m_src(0) | m_dest(0o10) | SH);
    assert_eq!(halt, Some(Halt::OaOutsideFields { pc: at, bits: 1 << 31 }));
    let functional = ALU | SETM | src(0o7) | m_dest(0o10) | SH;
    let (at, (_, halt)) = pair(true, 0o10, functional);
    assert_eq!(halt, Some(Halt::OaOutsideFields { pc: at, bits: 0o10 << 26 }));
    let (_, (_, halt)) = pair(true, 0o10, ALU | SETM | m_src(0o20) | m_dest(0o10) | SH);
    assert_eq!(halt, None, "into an M-memory source");
}

/// **`-RESET` clears both registers** on revision 15 (A15b.15); on revision
/// 14 the boot drops IMOD's pending flags and leaves the registers (the
/// control). **Fails** a reset that leaves revision 15's registers.
#[test]
fn reset_clears_the_oa_registers() {
    let mut p = Prog::default();
    p.oa(false, 0o5 << 14).op(ALU | SETA | a_src(ONE) | m_dest(0o10) | SL);
    p.oa(true, 0o7).op(ALU | SETM | m_src(0o20) | m_dest(0o11) | SH);
    p.stop();
    for (g, kept) in [(REV15, false), (Geometry::QUUX_14, true)] {
        let (mut u, halt) = run_checked(&p, g, g == REV15);
        assert_eq!(halt, None);
        assert_eq!(u.oa_registers(), (0o5 << 14, 0o7), "{g:?}: loaded");
        u.boot();
        let want = if kept { (0o5 << 14, 0o7) } else { (0, 0) };
        assert_eq!(u.oa_registers(), want, "{g:?}: after the boot's -RESET");
    }
}

/// **A halt and a checkpoint taken between a writer and its consuming word
/// resume to the same end** (A15b.13, A15b.15): the register is kept, and
/// so is the check's shadow, which a resume reads back. **Fails** a
/// checkpoint without the registers, and one that drops the shadow (the
/// consuming word then halts at the check).
#[test]
fn a_halt_and_a_checkpoint_between_a_write_and_its_select_resume() {
    let mut p = Prog::default();
    p.fill(2);
    let writer = p.at() as u16;
    p.oa(false, 0o5 << 14).op(ALU | SETA | a_src(ONE) | m_dest(0o10) | SL);
    p.stop();
    let (whole, halt) = run_checked(&p, REV15, true);
    assert_eq!(halt, None);
    let to_writer = || {
        let mut u = Micro::new(machine(&p, REV15, &|_| {}));
        u.boot();
        while u.executed() != Some(writer) {
            u.step().unwrap();
        }
        u
    };
    let finish = |u: &mut Micro| {
        for _ in 0..1000 {
            if u.machine().opc == STOP as u16 {
                return u.run(16).1;
            }
            if let Err(h) = u.step() {
                return Some(h);
            }
        }
        panic!("never stopped");
    };
    let mut halted = to_writer();
    halted.machine_mut().clock_control.run = false;
    halted.run(5);
    halted.machine_mut().clock_control.run = true;
    assert_eq!(finish(&mut halted), None, "after a halt");
    expect(halted.machine(), &[(0o15, 1, "after a halt")]);
    let saved = to_writer();
    let mut wr = muir::checkpoint::Writer::new();
    saved.save(&mut wr);
    let body = wr.finish();
    let mut resumed = Micro::new(machine(&p, REV15, &|_| {}));
    resumed.load(&mut muir::checkpoint::Reader::for_word_bits(&body, 40)).unwrap();
    assert_eq!(finish(&mut resumed), None, "after a resume");
    assert_eq!(resumed.machine().mmem, whole.machine().mmem, "the same end");
}

/// **The shadow check halts on each breach, and honours N** (A15b.15): a
/// select with no write before it, a select two words after its write, a
/// write followed by a word without its select, a write of one register
/// and a select of the other; the console's NOP11 on the word after a
/// write, which drops the write as it drops IMOD's, so that a select on the
/// word after that halts and a word without one runs; a write
/// in the slot of a taken jump with N is nopped, so the jump's target needs
/// no select and one there halts; a write in the slot of a conditional jump
/// with N not taken is followed by the fall-through, and a write in the
/// slot of a jump with N clear by its target, each with its select running
/// clean. **Fails** a check that counts a nopped word, or one that ignores
/// either breach.
#[test]
fn the_shadow_check_halts_on_each_breach() {
    let check = |p: &Prog| run_checked(p, REV15, true).1;
    let consumer = ALU | SETA | a_src(ONE) | m_dest(0o10);
    // A select with no write before it.
    let mut p = Prog::default();
    p.fill(1);
    let at = p.at() as u16;
    p.op(consumer | SL).stop();
    assert_eq!(check(&p), Some(Halt::OaSelectWithoutWrite { pc: at, high: false }));
    // A select two words after its write: the word between breaks it.
    let mut p = Prog::default();
    p.oa(false, 0);
    let at = p.at() as u16;
    p.fill(1).op(consumer | SL).stop();
    assert_eq!(check(&p), Some(Halt::OaWriteWithoutSelect { pc: at, high: false }));
    // A write followed by a word without its select.
    let mut p = Prog::default();
    p.oa(true, 0);
    let at = p.at() as u16;
    p.op(consumer).stop();
    assert_eq!(check(&p), Some(Halt::OaWriteWithoutSelect { pc: at, high: true }));
    // One register written, the other selected.
    let mut p = Prog::default();
    p.oa(false, 0);
    let at = p.at() as u16;
    p.op(consumer | SH).stop();
    assert_eq!(check(&p), Some(Halt::OaWriteWithoutSelect { pc: at, high: false }));
    // A taken jump with N: its slot's write is nopped.
    for (select, want) in [(0, None), (SL, Some(()))] {
        let mut p = Prog::default();
        let t = p.at() + 3;
        p.op(JUMP | ALWAYS | target(t) | N);
        p.oa(false, 0);
        p.fill(1);
        p.op(consumer | select).stop();
        let got = check(&p);
        assert_eq!(got.is_some(), want.is_some(), "a nopped write, select {select:o}: {got:?}");
        if let Some(h) = got {
            assert_eq!(h, Halt::OaSelectWithoutWrite { pc: t as u16, high: false });
        }
    }
    // The console's NOP11 nops the word after a write, which drops IMOD's
    // pending flag and the shadow with it: the next word runs unchecked,
    // and a select there has no write before it.
    for (select, clean) in [(0, true), (SL, false)] {
        let mut p = Prog::default();
        let writer = p.at() as u16;
        p.oa(false, 0o5 << 14);
        p.op(consumer | SL);
        let at = p.at() as u16;
        p.op(consumer | m_dest(0o20) | select).stop();
        let mut u = Micro::new(machine(&p, REV15, &|_| {}));
        u.boot();
        while u.executed() != Some(writer) {
            u.step().unwrap();
        }
        u.machine_mut().clock_control.nop11 = true;
        u.step().unwrap();
        u.machine_mut().clock_control.nop11 = false;
        let got = (0..16).find_map(|_| u.step().err());
        assert_eq!(got.is_none(), clean, "after a nopped word: {got:?}");
        if !clean {
            assert_eq!(got, Some(Halt::OaSelectWithoutWrite { pc: at, high: false }));
        }
    }
    // A conditional jump with N, not taken: the write in its slot runs,
    // followed by the fall-through.
    for (select, clean) in [(SL, true), (0, false)] {
        let mut p = Prog::default();
        p.op(jcond(3) | m_src(M_ONE) | a_src(ZERO) | target(0o700) | N);
        p.oa(false, 0o5 << 14);
        let at = p.at() as u16;
        p.op(consumer | select).stop();
        let got = check(&p);
        assert_eq!(got.is_none(), clean, "a write followed by the fall-through: {got:?}");
        if !clean {
            assert_eq!(got, Some(Halt::OaWriteWithoutSelect { pc: at, high: false }));
        }
    }
    // A jump with N clear: the write in its slot, followed by the target.
    for (select, clean) in [(SL, true), (0, false)] {
        let mut p = Prog::default();
        let t = p.at() + 4;
        p.op(JUMP | ALWAYS | target(t));
        p.oa(false, 0o5 << 14);
        p.op(consumer | m_dest(0o20)).fill(1);
        p.op(consumer | select).stop();
        let got = check(&p);
        assert_eq!(got.is_none(), clean, "a write followed by the target: {got:?}");
        if !clean {
            assert_eq!(got, Some(Halt::OaWriteWithoutSelect { pc: t as u16, high: false }));
        }
    }
}

/// **The WRITE-I-MEM check** (proposed name; WRITE-I-MEM ruling), on
/// `micro` under the OA select check: a WRITE-I-MEM runs only in MIT's
/// form, `IR<9:0>` 1647 without POPJ, and never as a delay slot. Each
/// breach halts at the word, with its control: MIT's form clean, after an
/// ALU word, a jump with N, and another WRITE-I-MEM; without N, inverted,
/// conditional, with POPJ; and as the slot of a jump with N clear, of a
/// word with POPJ, of a dispatch. With the check off nothing halts.
/// **Fails** a check that misses a form or a slot, or halts on MIT's.
#[test]
fn the_write_i_mem_check_halts_outside_mit_s_form_and_in_a_slot() {
    use muir::isa::asm::INVERT;
    let mit = JUMP | P | R | N | ALWAYS | target(0o700) | a_src(ZERO) | m_src(0);
    assert_eq!(mit & 0o1777, 0o1647, "MIT's form");
    let run = |before: Option<u64>, w: u64, check: bool| {
        let mut p = Prog::default();
        let t = p.at() + 6;
        if let Some(b) = before {
            p.op(b | target(t));
        }
        let at = p.at() as u16;
        p.op(w);
        p.fill(2).stop();
        while p.at() < t {
            p.fill(1);
        }
        p.fill(1).stop();
        let mut u = Micro::new(machine(&p, REV15, &|_| {}));
        u.oa_select_check = check;
        u.boot();
        let halt = (0..2_000).find_map(|_| u.step().err());
        (at, halt)
    };
    let alu = ALU | SETA | a_src(ONE) | m_dest(0o10);
    for (what, before, w, refused) in [
        ("MIT's form", None, mit, None),
        ("after an ALU word", Some(alu & !(0o37777 << 12)), mit, None),
        ("after a jump with N", Some(JUMP | ALWAYS | N), mit, None),
        ("after a WRITE-I-MEM", Some(mit & !(0o37777 << 12)), mit, None),
        ("without N", None, mit & !N, Some(false)),
        ("inverted", None, mit | INVERT, Some(false)),
        ("conditional", None, mit & !0o40, Some(false)),
        ("with POPJ", None, mit | POPJ, Some(false)),
        ("in a jump's slot", Some(JUMP | ALWAYS), mit, Some(true)),
        ("after a POPJ word", Some(alu & !(0o37777 << 12) | POPJ), mit, Some(true)),
        ("after a dispatch", Some(DISPATCH), mit, Some(true)),
    ] {
        let (at, got) = run(before, w, true);
        let want = refused.map(|in_slot| Halt::WriteImemRefused { pc: at, in_slot });
        assert_eq!(got, want, "{what}");
        if refused.is_some() {
            let (_, off) = run(before, w, false);
            assert!(!matches!(off, Some(Halt::WriteImemRefused { .. })), "{what}: the check off");
        }
    }
}

// --- D, the dispatch from the fetched word (A15b.9) -----------------------------------

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
    let mut m = Machine::new();
    m.geometry = geometry;
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

/// Runs to opcode 7's stop: the machine, the addresses executed in order,
/// and how many times the main loop's first word ran.
fn d_run(m: Machine) -> (Machine, Vec<u16>, usize) {
    let mut u = Micro::new(m);
    u.boot();
    let mut trace = Vec::new();
    for _ in 0..20_000 {
        if u.machine().opc == OP_7 as u16 {
            u.run(8);
            let qmlp = trace.iter().filter(|&&pc| pc == QMLP as u16).count();
            return (u.machine().clone(), trace, qmlp);
        }
        u.step().unwrap();
        trace.extend(u.executed());
    }
    panic!("the program never reached its stop");
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

/// The state the two paths must agree on: M and A memory but M and A 37,
/// where every functional destination here writes, and A 51, the
/// register's word; the micro stack, the location counter, the PDL's
/// registers and its first words, where opcode 1's microcycle after its
/// return pushes M 31 as it finds it, `MD`.
fn d_state(m: &Machine) -> impl PartialEq + std::fmt::Debug {
    let (mut mmem, mut amem) = (m.mmem, m.amem);
    (mmem[0o37], amem[0o37], amem[0o51]) = (0, 0, 0);
    let pdl = m.pdl[..0o40].to_vec();
    (mmem, amem.to_vec(), m.spc, m.spcptr, m.lc, m.pdl_pointer, m.pdl_index, m.md, pdl)
}

/// **D dispatches a return that needs a fetch** (A15b.9, A15.2's checks):
/// with the register's `<31>` and `<30>` set, every return needing a fetch
/// goes to its entry's handler, and the state at the end is the main
/// loop's, M 31 the last word fetched and the old word in the microcycle
/// after each return; the main loop runs not once. With
/// `<30>` clear the main loop runs for each of them. **Fails** a revision 15
/// without D, a D that leaves M 31 or the stack as the main loop would
/// not, and one that loads M 31 a microcycle early.
#[test]
fn d_dispatches_a_return_that_needs_a_fetch() {
    let run = |d: bool| d_run(d_machine(REV15, d_register(d), &ONES, CODE, false, false));
    let (with, _, qmlp_with) = run(true);
    let (without, _, qmlp_without) = run(false);
    assert_eq!(qmlp_with, 0, "D: the main loop runs not once");
    assert_eq!(qmlp_without, 6, "without D: the first return and the five");
    expect(&with, &[(1, 11, "opcode 1 eleven times")]);
    assert_eq!(with.mmem[0o31], with.main[0o405], "M 31: the last word fetched");
    assert_eq!(d_state(&with), d_state(&without), "the main loop's state");
}

/// **D's enable clear is revision 14, microcycle for microcycle** (A15.2's
/// checks): the same program executes the same addresses in the same order
/// on revision 15 with `<30>` clear and on revision 14, which keeps no
/// `<30>`, written or not. **Fails** a D that acts on `<31>` alone.
#[test]
fn d_s_enable_clear_is_revision_14_microcycle_for_microcycle() {
    let (_, r15, _) = d_run(d_machine(REV15, d_register(false), &ONES, CODE, false, false));
    let (_, r14, _) =
        d_run(d_machine(Geometry::QUUX_14, d_register(true), &ONES, CODE, false, false));
    assert_eq!(r15, r14);
}

/// **Condition 6 true, a fetch that faults, and an entry with P go to the
/// main loop** (A15b.9, A15.2's checks): with a sequence break standing,
/// the main loop runs for every return that needs a fetch and calls
/// condition 6's handler; a fetch from an unmapped page, which faults,
/// goes to the main loop, which takes the fault; and a halfword whose entry
/// has P is the main loop's to dispatch, once, while the others' are D's.
/// **Fails** a D that does not test condition 6, the fetch's fault, or the
/// entry's P.
#[test]
fn condition_6_a_faulting_fetch_and_p_go_to_the_main_loop() {
    let (m, _, qmlp) = d_run(d_machine(REV15, d_register(true), &ONES, CODE, true, false));
    assert_eq!(qmlp, 6, "a sequence break: the main loop");
    expect(&m, &[(0o10, 6, "condition 6's calls"), (1, 11, "opcode 1")]);
    let (m, _, qmlp) = d_run(d_machine(REV15, d_register(true), &ONES, UNMAPPED, false, true));
    assert_eq!(qmlp, 1, "a faulting fetch: the main loop");
    expect(&m, &[(0o10, 1, "the fault taken"), (1, 0, "nothing dispatched")]);
    let mut program = ONES;
    program[4] = hw(6, 0, 0);
    let (m, _, qmlp) = d_run(d_machine(REV15, d_register(true), &program, CODE, false, false));
    assert_eq!(qmlp, 1, "an entry with P: the main loop, once");
    expect(&m, &[(6, 1, "opcode 6"), (1, 10, "opcode 1")]);
}

/// **M 31 and the operand address after D's dispatch are a fused
/// return's** (A15b.9, A15.2): opcode 10, whose entry has the operand bit,
/// in a word's first halfword with LOCAL and delta 5: its handler's first
/// microinstruction finds M 31 the fetched word, as the main loop leaves
/// it, and PDL-INDEX `A-LOCALP` + 5, which the main loop's path, with no
/// operand microcode here, leaves at the start's. **Fails** a D that does
/// not arm M 31 or the operand address.
#[test]
fn m31_and_the_operand_address_after_d_are_a_fused_return_s() {
    let program = [hw(1, 0, 0), hw(1, 0, 0), hw(0o10, 5, 5), hw(7, 0, 0)];
    let run = |d: bool| d_run(d_machine(REV15, d_register(d), &program, CODE, false, false)).0;
    let (with, without) = (run(true), run(false));
    let word = with.main[0o401];
    // After opcode 1's two pushes, from the microcycles after its returns.
    assert_eq!(with.pdl[3..5], [word, LOCALP + 5], "D: M 31 and the operand address");
    assert_eq!(without.pdl[3..5], [word, u64::from(SENTINEL)], "the main loop's");
}

// --- CMD_PROD (A15b.5) -----------------------------------------------------------------

/// **CMD_PROD moves once every earlier write is answered** (A15b.5,
/// amendment 6): a program writes a command entry's word 0 to main memory
/// and the producer index, register-page word 164, in its next memory
/// cycle; when the device takes the index, the word is in main memory, and
/// the command it runs is the one written, its tag and opcode echoed.
/// `micro` posts no writes: a write goes out by the end of the microcycle
/// after its start, and a start right after it waits for it, so the rule
/// holds here as it stands, and the check that discriminates, a command
/// still queued when 164 is written, needs a queue that posts writes,
/// which is `rtl`'s to model. **Fails** an engine that loses the word's
/// write or lets the index's pass it.
#[test]
fn cmd_prod_is_taken_after_every_earlier_write() {
    use muir::file_device::op;
    const RING: u32 = 0o100000;
    const RESP: u32 = 0o110000;
    const TAG: u32 = 0o4321;
    let page = muir::tlb::REGISTER_PAGE_BUS;
    let mut p = Prog::default();
    let entry = Word::from(TAG | op::READ << 16);
    let (word, slot) = (p.k(entry), p.k(0o36000000000 | Word::from(RING)));
    let (prod, cmd_prod) = (p.k(1), p.k(REGISTER_PAGE | 0o164));
    // The word the write carries is `MD` of the microcycle after its start,
    // so the index's `MD` waits a microcycle.
    p.op(ALU | SETA | a_src(word) | MD);
    p.op(ALU | SETA | a_src(slot) | START_WRITE);
    p.fill(1);
    p.op(ALU | SETA | a_src(prod) | MD);
    p.op(ALU | SETA | a_src(cmd_prod) | START_WRITE);
    p.fill(2).stop();
    let mut u = Micro::new(machine(&p, REV15, &|_| {}));
    u.boot();
    let m = u.machine_mut();
    for (k, v) in [(0o162, RING), (0o163, 2), (0o166, RESP), (0o167, 2), (0o160, 1)] {
        m.bus_write(page | k, Word::from(v));
    }
    m.register_log = Some(Vec::new());
    let mut seen = false;
    for _ in 0..20_000 {
        u.step().unwrap();
        let m = u.machine();
        let taken = m.register_log.as_ref().unwrap().iter().any(|&(at, _, _)| at == page | 0o164);
        if taken && !seen {
            seen = true;
            assert_eq!(m.main[RING as usize], entry, "word 0 is in");
        }
        if m.file_device.response_producer() == 1 {
            let r = m.main[RESP as usize] as u32;
            assert_eq!((r & 0xffff, r >> 24 & 0xff), (TAG, op::READ), "the command run");
            return;
        }
    }
    panic!("no response (the index taken: {seen})");
}

/// **The symbol area lies in main memory** (A15b.7): a start or an extent
/// past the largest main memory, 64MW, is refused by the reader, and one
/// past a machine's own by `Mcr::check_main_memory`; the area that ends at
/// the last word is taken (the controls). **Fails** a reader that bounds
/// the area by the physical space alone.
#[test]
fn the_symbol_area_lies_in_main_memory() {
    const MAX: u32 = muir::machine::MAX_MAIN_WORDS_13 as u32;
    let with_area =
        |start: u32, n: usize| planted(|s| s[3] = section(3, start, 40, 64, &vec![FIX | 1; n]));
    for (start, n) in [(MAX, 1), (MAX - 1, 2), (0o37777777777, 1)] {
        let e = mcr::parse_quux_microcode(&with_area(start, n), REV15).unwrap_err();
        assert!(e.contains("past main memory's 400000000"), "{start:o}+{n}: {e}");
    }
    let m = mcr::parse_quux_microcode(&with_area(MAX - 2, 2), REV15).unwrap();
    assert_eq!(m.check_main_memory(MAX as usize), Ok(()));
    let small = 32 << 20;
    assert_eq!(m.check_main_memory(small).map_err(|e| e.contains("past main memory's")), Err(true));
    let m = mcr::parse_quux_microcode(&with_area(small as u32 - 1, 1), REV15).unwrap();
    assert_eq!(m.check_main_memory(small), Ok(()), "the last word");
    let m = mcr::parse_quux_microcode(&with_area(small as u32 - 1, 2), REV15).unwrap();
    let e = m.check_main_memory(small).unwrap_err();
    assert!(e.contains("past main memory's"), "an extent past the last word: {e}");
}

// --- A checkpoint of the pipeline from the command line (A15b.13) -----------

/// A PROM that writes and reads main memory in a loop through the physical
/// memory window: M 2 the window's base, `0o36000000000`; M 3 counts; each
/// turn writes the count to the base plus the count, reads it back into M
/// 4 and adds it to M 5. So a stop finds writes queued and in flight and a
/// read on its way, as often as not.
fn memory_loop_prom(dir: &std::path::Path) -> std::path::PathBuf {
    use muir::isa::asm::{ADD, BYTE, DPB, HINT, SETO, SETZ};
    const AT: u64 = 0o36000;
    let words = [
        ALU | SETO | m_dest(1),
        BYTE | DPB | 3 << 6 | 28 | m_src(1) | a_src(ZERO) | m_dest(2),
        ALU | SETZ | m_dest(3),
        ALU | SETZ | m_dest(5),
        // The loop, at AT + 4.
        ALU | M_PLUS_C | CARRY_IN | m_src(3) | m_dest(3),
        ALU | SETM | m_src(3) | MD,
        ALU | ADD | a_src(2) | m_src(3) | START_WRITE,
        filler().raw(),
        ALU | ADD | a_src(2) | m_src(3) | START_READ,
        filler().raw(),
        ALU | SETM | SRC_MD | m_dest(4),
        ALU | ADD | a_src(5) | m_src(4) | m_dest(5),
        JUMP | target(AT + 4) | ALWAYS | N | HINT,
        filler().raw(),
    ];
    let sections = vec![section(6, 0, 32, 32, &[15]), section(1, AT as u32, 64, 64, &words)];
    let path = dir.join("memory-loop-15.mcr");
    std::fs::write(&path, file(&sections, None)).unwrap();
    path
}

/// The machine a checkpoint `c` loads onto, as `--resume` builds it.
fn machine_for(c: &muir::checkpoint::Checkpoint) -> Machine {
    let geometry = Machine::checkpointed_geometry_at(&c.body, c.word_bits).unwrap();
    let mut m = Machine::with_geometry(geometry, c.memory_boards);
    m.block_disk = Some(muir::block_disk::BlockDisk::new(muir::block_disk::BLOCK_NS));
    m
}

/// The checkpoint at `path`, loaded on its engine: its microcycles and the
/// time-neutral harness's whole digest of the state between two
/// microcycles (`tests/support/neutral.rs`).
fn digest_of(path: &std::path::Path) -> (u64, u64) {
    use support::neutral::digest;
    let c = muir::checkpoint::read(path).unwrap();
    let m = machine_for(&c);
    match c.engine.as_str() {
        "rtl" => {
            let mut e = muir::pipeline::Pipeline::new(m);
            e.load(&mut c.reader()).unwrap();
            (e.machine().cycles, digest(e.machine(), e.pending(), e.oa_registers(), true))
        }
        "micro" => {
            let mut e = Micro::new(m);
            e.load(&mut c.reader()).unwrap();
            // What its halt would send out, as the harness does.
            e.send_out_waiting_write();
            (e.machine().cycles, digest(e.machine(), e.pending(), e.oa_registers(), true))
        }
        other => panic!("{other}"),
    }
}

/// `quux` on revision 15 with `engine`, its other flags `args`; a fresh
/// run with 2MW of main memory, which keeps the digests quick.
fn quux_15(engine: &str, args: &[&std::ffi::OsStr]) -> std::process::Command {
    let mut c = support::quux();
    c.env("MUIR_QUUX_REVISION", "15").arg(format!("--{engine}")).args(args);
    if !args.iter().any(|a| *a == "--resume") {
        c.args(["--main-memory-size", "2MW"]);
    }
    c
}

/// **`--stop-after` with `--checkpoint` on revision 15's `rtl` drains the
/// pipeline first**, as the halt and the harness's action point do (A15b.13):
/// stopped at microcycles that leave words in the stages, writes queued
/// and in flight and a read on its way, it writes a checkpoint of the
/// state between two microcycles. `micro`, run to the same microcycle,
/// writes one that digests the same; each resumed and run on, and
/// checkpointed again, digests the same again. **Fails** a stop that writes
/// the pipeline as it stands, which panicked.
#[test]
fn an_rtl_stop_on_revision_15_drains_before_its_checkpoint() {
    use support::{Run, scratch, text};
    let dir = scratch("revision-15-rtl-checkpoint");
    let prom = memory_loop_prom(&dir);
    let os = |s: &str| std::ffi::OsString::from(s);
    for stop in [37u64, 41, 46, 52, 59, 700] {
        let (r, u) = (dir.join(format!("rtl-{stop}.chk")), dir.join(format!("micro-{stop}.chk")));
        let out = quux_15(
            "rtl",
            &[
                &os("--stop-after"),
                &os(&stop.to_string()),
                &os("--prom"),
                prom.as_os_str(),
                &os("--checkpoint"),
                r.as_os_str(),
            ],
        )
        .run();
        assert!(out.status.success(), "rtl, stop {stop}:\n{}", text(&out));
        let (cycles, rd) = digest_of(&r);
        assert!(cycles == stop || cycles == stop + 1, "stop {stop}: at {cycles}");
        let out = quux_15(
            "micro",
            &[
                &os("--stop-after"),
                &os(&cycles.to_string()),
                &os("--prom"),
                prom.as_os_str(),
                &os("--checkpoint"),
                u.as_os_str(),
            ],
        )
        .run();
        assert!(out.status.success(), "micro, stop {stop}:\n{}", text(&out));
        assert_eq!(digest_of(&u), (cycles, rd), "stop {stop}: micro's at the same microcycle");
        // Each resumed, run on and checkpointed again.
        let (r2, u2) =
            (dir.join(format!("rtl-{stop}-2.chk")), dir.join(format!("micro-{stop}-2.chk")));
        let out = quux_15(
            "rtl",
            &[
                &os("--resume"),
                r.as_os_str(),
                &os("--stop-after"),
                &os("23"),
                &os("--checkpoint"),
                r2.as_os_str(),
            ],
        )
        .run();
        assert!(out.status.success(), "rtl resumed, stop {stop}:\n{}", text(&out));
        let (cycles2, rd2) = digest_of(&r2);
        assert!(cycles2 >= cycles + 23, "stop {stop}: resumed and ran on to {cycles2}");
        let on = (cycles2 - cycles).to_string();
        let out = quux_15(
            "micro",
            &[
                &os("--resume"),
                u.as_os_str(),
                &os("--stop-after"),
                &os(&on),
                &os("--checkpoint"),
                u2.as_os_str(),
            ],
        )
        .run();
        assert!(out.status.success(), "micro resumed, stop {stop}:\n{}", text(&out));
        assert_eq!(digest_of(&u2), (cycles2, rd2), "stop {stop}: resumed, run on alike");
    }
}

/// **The prompt's `checkpoint` on revision 15's `rtl` drains the pipeline
/// first, and the run goes on**: held, stepped, checkpointed, stepped again
/// and quit with `--checkpoint`; both files digest as `micro`'s at the
/// same microcycles. **Fails** a prompt that writes the pipeline as it
/// stands, which panicked.
#[test]
fn the_prompt_s_checkpoint_on_revision_15_s_rtl_drains_and_goes_on() {
    use std::io::Write;
    use support::{Run, scratch, text};
    let dir = scratch("revision-15-prompt-checkpoint");
    let prom = memory_loop_prom(&dir);
    let (held, end) = (dir.join("held.chk"), dir.join("end.chk"));
    let mut c = quux_15("rtl", &[]);
    c.args(["--stop-after", "1000000000", "--prom"]).arg(&prom).arg("--checkpoint").arg(&end);
    c.stdin(std::process::Stdio::piped());
    let mut child = c.start();
    let mut stdin = child.stdin();
    write!(stdin, "hold\nstep 9\ncheckpoint {}\nstep 5\nq\n", held.display()).unwrap();
    drop(stdin);
    let out = child.wait();
    let t = text(&out);
    assert!(out.status.success(), "rtl:\n{t}");
    let ((c1, d1), (c2, d2)) = (digest_of(&held), digest_of(&end));
    assert!(c2 > c1, "the run went on after the checkpoint: {c1}, {c2}");
    for (cycles, d, name) in [(c1, d1, "held"), (c2, d2, "end")] {
        let u = dir.join(format!("micro-{name}.chk"));
        let out = quux_15("micro", &[])
            .args(["--stop-after", &cycles.to_string(), "--prom"])
            .arg(&prom)
            .arg("--checkpoint")
            .arg(&u)
            .run();
        assert!(out.status.success(), "micro:\n{}", text(&out));
        assert_eq!(digest_of(&u), (cycles, d), "{name}: micro's at the same microcycle");
    }
}
