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
    START_WRITE, a_src, filler, m_dest, m_src, pdl_field, predicted, src, target,
};
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
/// of 0.5 ns** (A15b.1): `micro`'s QUUX microcycle is 40 ns, so 80; 60 ns
/// gives 120. On revision 14 word 25 reads 0, as every unassigned feature
/// word does (the control). **Fails** a MACHINE-ID left at 14, and a word 25
/// that is not the engine's period.
#[test]
fn feature_words_0_and_25_say_revision_15_and_its_period() {
    let mut p = Prog::default();
    p.read(REGISTER_PAGE, 0o20).read(REGISTER_PAGE | 0o25, 0o21).stop();
    let m = run(&p);
    expect(&m, &[(0o20, 0x5155_00f4, "MACHINE-ID"), (0o21, 80, "word 25 at 40 ns")]);
    let mut u = Micro::new(machine(&p, REV15, &|_| {}));
    u.sync_cycle_ns = 60;
    u.boot();
    while u.machine().opc != STOP as u16 {
        u.step().unwrap();
    }
    u.run(16);
    expect(u.machine(), &[(0o21, 120, "word 25 at 60 ns")]);
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
    p.op(JUMP | P | R | ALWAYS | target(0o700) | a_src(a) | m_src(m));
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
/// revision-15 PROM: the start names it, and the checkpoint records it.
/// Refused: `--rtl`, and a run without `--prom`, since no revision-15 PROM
/// is built in and PROM 2001 is MIT's sections. **Fails** a switch that
/// builds revision 14, or lets `rtl` run revision 15 as 14.
#[test]
fn the_switch_runs_revision_15_on_micro() {
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
    let out = quux()
        .env("MUIR_QUUX_REVISION", "15")
        .args(["--rtl", "--stop-after", "10", "--prom"])
        .arg(&prom)
        .run();
    let t = text(&out);
    assert_eq!(out.status.code(), Some(2), "{t}");
    assert!(t.contains("QUUX revision 15 runs on --micro alone"), "{t}");
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
