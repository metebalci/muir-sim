// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **QUUX revision 14** (contract G3 revision 14, with its appendix A14),
//! slice S1a, on hand-written microcode on `micro` and `rtl`: the address
//! space and its windows, the page table and its walk, the TLB and its
//! operations, `MAP(MD)` and the dispatches on map bits, the location
//! counter at 34 bits with its adder, jump condition 12, and the memory
//! system's register-page words. Every program runs on both engines, and
//! each engine's M memory must be what the program's comments say.
//!
//! Each test names, in its comment, the partial implementation it fails.
//!
//! The words are 40 bits, G1's: the data type `<37:32>` over the field
//! `<31:0>`. Addresses are octal, as A14 writes them.

use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{
    ADD, ALU, ALWAYS, CARRY_IN, DISPATCH, JUMP, LDB, M_PLUS_C, MD, N, OB_LEFT, POPJ, SETA, SETM,
    SRC_MD, START_READ, START_WRITE, SUB, a_dest, a_src, filler, m_dest, m_src, src, target,
};
use muir::machine::{Geometry, Machine, Word};
use muir::micro::Micro;
use muir::rtl::Rtl;
use muir::tlb;

mod support;

/// Revision 14.
const REV14: Geometry = Geometry::QUUX_14;

/// A memory's constants: 0, 1, 2.
const ZERO: u64 = 0o40;
const ONE: u64 = 0o41;
/// M memory's 0 and 1, which [`Prog::taken`] writes with BYTE words.
const M_ZERO: u64 = 0o34;
const M_ONE: u64 = 0o35;

/// The program's last word, a jump to itself.
const STOP: u64 = 0o1000;

/// The fixnum's data type, `005`, and a list pointer's, `016` (DTP-LIST in
/// `qcom.lisp`); NULL's is 0.
const FIX: Word = 0o005 << 32;
const LIST: Word = 0o016 << 32;

/// A 40-bit word from its tag `<39:32>` and field `<31:0>`.
const fn w(tag: u64, field: u64) -> Word {
    tag << 32 | (field & 0xffff_ffff)
}

/// A functional destination, with M's address 36 as the scratch word.
const fn fd(code: u64) -> u64 {
    code << 19 | 0o36 << 14
}
/// Destination 1, LOCATION-COUNTER; 15, the SPC push; 23, `VMA` with the
/// `WRITE-MAP` operation a microcycle later.
const LC: u64 = fd(1);
const SPC_PUSH: u64 = fd(0o15);
const WRITE_MAP: u64 = fd(0o23);

/// Functional sources 11, `MAP(MD)`, and 13, the location counter.
const SRC_MAP: u64 = src(0o11);
const SRC_LC: u64 = src(0o13);

/// `M-1`: the 74S181's function 15 with no carry, `IR<8:3>` 23.
const M_MINUS_1: u64 = 0o23 << 3;

/// A BYTE word: function, rotate `IR<5:0>` and length − 1 `IR<11:6>`.
fn byte(func: u64, rotate: u64, len: u64) -> u64 {
    muir::isa::asm::BYTE | func | (len - 1) << 6 | rotate
}
/// A JUMP on condition `code`, `IR<4:0>` with `IR<5>`.
fn jcond(code: u64) -> u64 {
    JUMP | 1 << 5 | code
}
/// A DISPATCH: address `IR<23:12>`, length `IR<7:5>`, rotate `{IR<47>,
/// IR<4:0>}`.
fn disp(addr: u64, len: u64, r: u64) -> u64 {
    DISPATCH | addr << 12 | len << 5 | (r >> 5) << 47 | (r & 0o37)
}
/// The map bits of a DISPATCH, `IR<9:8>`: 1 the entry's `<22>`, 2 its
/// `<23>`, oldspace.
const MAP_23: u64 = 2 << 8;

/// A program under construction.
#[derive(Default)]
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
    /// An A-memory constant, from 100 up: its address.
    fn k(&mut self, v: Word) -> u64 {
        let a = 0o100 + self.next_a;
        self.next_a += 1;
        self.amem.push((a, v));
        a
    }
    /// M memory `m` (and the A memory word it shadows).
    fn m(&mut self, m: u64, v: Word) -> &mut Self {
        self.mmem.push((m, v));
        self
    }
    /// M `slot` <- 1 if the JUMP `jump` is taken, 0 if not.
    fn taken(&mut self, jump: u64, slot: u64) -> &mut Self {
        self.op(byte(LDB, 0, 40) | m_src(M_ONE) | m_dest(slot));
        let next = self.at() + 2;
        self.op(jump | target(next) | N);
        self.op(byte(LDB, 0, 40) | m_src(M_ZERO) | m_dest(slot))
    }
    /// M `slot` <- the word at the virtual address `va`, and M `fault` <-
    /// 1 if the read faulted (condition 4), 0 if not.
    fn read(&mut self, va: Word, slot: u64, fault: u64) -> &mut Self {
        let a = self.k(va);
        self.op(ALU | SETA | a_src(a) | START_READ);
        self.taken(jcond(4), fault);
        self.op(ALU | SETM | SRC_MD | m_dest(slot))
    }
    /// The word `word` to the virtual address `va`; M `fault` <- 1 if the
    /// write faulted.
    fn write(&mut self, word: Word, va: Word, fault: u64) -> &mut Self {
        let (wa, aa) = (self.k(word), self.k(va));
        self.op(ALU | SETA | a_src(wa) | MD);
        self.op(ALU | SETA | a_src(aa) | START_WRITE);
        self.taken(jcond(4), fault);
        self.fill(1)
    }
    /// A `WRITE-MAP` operation `op` (A14.4) at `MD` = `va` with `VMA`'s
    /// `<29:0>` `entry`, landed by the time the next word runs.
    fn tlb_op(&mut self, op: u64, va: Word, entry: Word) -> &mut Self {
        let (aa, oa) = (self.k(va), self.k(op << 32 | entry));
        self.op(ALU | SETA | a_src(aa) | MD);
        self.op(ALU | SETA | a_src(oa) | WRITE_MAP);
        self.fill(2)
    }
    /// M `slot` <- `MAP(MD)` with `MD` = `va`.
    fn map(&mut self, va: Word, slot: u64) -> &mut Self {
        let a = self.k(va);
        self.op(ALU | SETA | a_src(a) | MD);
        self.op(ALU | SETM | SRC_MAP | m_dest(slot))
    }
    /// Jumps to the stop.
    fn stop(&mut self) -> &mut Self {
        self.op(JUMP | target(STOP) | ALWAYS | N);
        self.op(filler().raw())
    }
}

// --- the page table ------------------------------------------------------

/// The directory's first frame: frames 4-7, a multiple of 4 (A14.3).
const DIR: u32 = 4;
/// Page-table pages from frame 10 up.
const FIRST_TABLE: u32 = 0o10;

/// A page entry (A14.2), a fixnum: status 4, access `11`, not oldspace
/// and not extra PDL (`<23:22>` 11), at `frame`.
const fn rw(frame: u32) -> Word {
    FIX | 1 << 27 | 1 << 26 | 3 << 22 | frame as Word
}
/// Status 4 with access `11` and `<23:22>` as `meta`.
const fn rw_meta(frame: u32, meta: u64) -> Word {
    FIX | 1 << 27 | 1 << 26 | meta << 22 | frame as Word
}
/// A page entry of status `status` and access code `access`, `<23:22>`
/// 11, the frame or slot `frame`.
const fn entry(status: u64, access: u64, frame: u32) -> Word {
    FIX | access << 26 | status << 24 | 3 << 22 | frame as Word
}

/// Main memory's word for the page-table entry of `va` in `m`'s tables,
/// with a directory entry and a table page made for it if there is none.
fn entry_at(m: &mut Machine, va: u32) -> usize {
    let d = (DIR as usize) << 10 | (va >> 20) as usize;
    if tlb::status(m.main[d]) != 4 {
        let used = (0..4096).filter(|&k| tlb::status(m.main[(DIR as usize) << 10 | k]) == 4);
        let frame = FIRST_TABLE + used.count() as u32;
        m.main[d] = FIX | 1 << 26 | frame as Word;
    }
    let frame = m.main[d] as usize & 0o777777;
    frame << 10 | (va >> 10 & 0o1777) as usize
}

/// Maps `va`'s page with the page entry `e`.
fn map_page(m: &mut Machine, va: u32, e: Word) {
    let at = entry_at(m, va);
    m.main[at] = e;
}

/// The machine for `p`, revision 14, `setup` last.
fn machine(p: &Prog, setup: &dyn Fn(&mut Machine)) -> Machine {
    let mut prom: Vec<Insn> = p.words.iter().map(|&w| Insn::new(w)).collect();
    assert!(prom.len() <= STOP as usize, "the program runs into its stop");
    prom.resize(STOP as usize, filler());
    prom.push(Insn::new(JUMP | target(STOP) | ALWAYS | N));
    prom.resize(1024, filler());
    let mut m = Machine::new();
    m.geometry = REV14;
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

/// Runs to the stop, and eight microcycles on.
fn finish<E: Engine>(e: &mut E, name: &str) {
    for _ in 0..20_000 {
        if e.machine().opc == STOP as u16 {
            e.run(8);
            return;
        }
        e.step().unwrap();
    }
    panic!("{name}: the program never reached its stop");
}

/// The directory base set after the boot, which clears it (A14.9).
fn with_directory(m: &mut Machine) {
    m.memory_words.directory = DIR;
}

/// Both engines' machines after `p`: `setup` before the boot, the
/// directory base and `after_boot` after it.
fn run_with(
    p: &Prog,
    setup: &dyn Fn(&mut Machine),
    after_boot: &dyn Fn(&mut Machine),
) -> [(&'static str, Machine); 2] {
    let mut u = Micro::new(machine(p, setup));
    u.boot();
    with_directory(u.machine_mut());
    after_boot(u.machine_mut());
    finish(&mut u, "micro");
    let mut r = Rtl::new(machine(p, setup));
    r.boot();
    with_directory(r.machine_mut());
    after_boot(r.machine_mut());
    finish(&mut r, "rtl");
    [("micro", u.machine().clone()), ("rtl", r.machine().clone())]
}

/// M memory's `slot`s are `want`, on both engines.
fn expect(ms: &[(&str, Machine)], rows: &[(u64, Word, &str)]) {
    for (engine, m) in ms {
        for &(slot, want, what) in rows {
            let got = m.mmem[slot as usize];
            assert_eq!(got, want, "{engine}: M {slot:o}, {what}: {got:#012x}, not {want:#012x}");
        }
    }
}

/// The walks each engine made.
fn walks(ms: &[(&str, Machine)]) -> [u64; 2] {
    [ms[0].1.tlb.walks, ms[1].1.tlb.walks]
}

// --- the windows (A14.1) ---------------------------------------------------

/// The physical memory window's address of main memory's word `phys`.
const fn phys_va(phys: u32) -> Word {
    (tlb::PHYSICAL_WINDOW | phys) as Word
}

/// **The windows never touch the TLB** (A14.1): `VA<31:28>` = `1110` and
/// `1111` decode as windows; a TLB holding a planted entry for each address,
/// to another frame, is not consulted, and nothing walks. **Fails** a decode
/// that sends a window to the TLB, or one that decodes on `VA<27:0>` alone.
#[test]
fn the_windows_never_touch_the_tlb() {
    const WORD: u32 = 0o2000 + 7;
    let mut p = Prog::default();
    p.read(phys_va(WORD), 0o10, 0o11);
    // The register page's word 0 through the device window.
    p.read(tlb::REGISTER_PAGE as Word, 0o12, 0o13);
    p.stop();
    let setup = |m: &mut Machine| {
        m.main[WORD as usize] = w(0o031, 0x600d);
        m.main[0o3000 + 7] = w(0o031, 0xbad);
    };
    let plant = |m: &mut Machine| {
        // Entries for both window addresses, to frame 3: the TLB would
        // give word 3007 for the first.
        m.tlb.load(tlb::PHYSICAL_WINDOW | WORD, rw(3) as u32);
        m.tlb.load(tlb::REGISTER_PAGE, rw(3) as u32);
    };
    let ms = run_with(&p, &setup, &plant);
    expect(
        &ms,
        &[
            (0o10, w(0o031, 0x600d), "the physical memory window's word"),
            (0o11, 0, "no fault"),
            (0o12, (0x5155 << 16) | (14 << 4) | 4, "MACHINE-ID through the device window"),
            (0o13, 0, "no fault"),
        ],
    );
    assert_eq!(walks(&ms), [0, 0], "nothing walked");
}

/// **The physical memory window aliases a translated frame, coherently**
/// (A14.1): a word written through a page translated to frame F reads back
/// through `36000000000` + F's address, and the reverse; a write through
/// the window sets no bit in a page's entry, where the translated
/// references set accessed and modified. **Fails** a window that is not
/// physical = `VA<27:0>`, or one that sets accessed or modified.
#[test]
fn the_physical_memory_window_aliases_a_translated_frame() {
    const VA: u32 = 0o12345 << 10;
    const FRAME: u32 = 0o200;
    const OTHER_VA: u32 = 0o12346 << 10;
    const OTHER: u32 = 0o201;
    let mut p = Prog::default();
    p.write(w(0o025, 0x1111), (VA | 5) as Word, 0o10);
    p.read(phys_va(FRAME << 10 | 5), 0o11, 0o12);
    p.write(w(0o025, 0x2222), phys_va(FRAME << 10 | 6), 0o13);
    p.read((VA | 6) as Word, 0o14, 0o15);
    // A frame whose page is never referenced, written and read through the
    // window alone.
    p.write(w(0o025, 0x3333), phys_va(OTHER << 10 | 7), 0o16);
    p.read(phys_va(OTHER << 10 | 7), 0o17, 0o16);
    p.stop();
    let setup = |m: &mut Machine| {
        map_page(m, VA, rw(FRAME));
        map_page(m, OTHER_VA, rw(OTHER));
    };
    let ms = run_with(&p, &setup, &|_| {});
    expect(
        &ms,
        &[
            (0o10, 0, "the translated write"),
            (0o11, w(0o025, 0x1111), "read through the window"),
            (0o12, 0, "no fault"),
            (0o13, 0, "the window's write"),
            (0o14, w(0o025, 0x2222), "read through the page"),
            (0o15, 0, "no fault"),
            (0o16, 0, "no fault"),
            (0o17, w(0o025, 0x3333), "the other frame through the window"),
        ],
    );
    for (engine, mut m) in ms {
        let at = entry_at(&mut m, OTHER_VA);
        assert_eq!(m.main[at], rw(OTHER), "{engine}: the window set no bit in its entry");
        let at = entry_at(&mut m, VA);
        let am = Word::from(tlb::ACCESSED | tlb::MODIFIED);
        assert_eq!(
            m.main[at],
            rw(FRAME) | am,
            "{engine}: the translated page's, accessed and modified"
        );
    }
}

/// The device window's frame buffer, slice 0.
const FRAME_BUFFER: u32 = tlb::DEVICE_WINDOW;

/// **The device window** (A14.1): frame buffer 0 from the window's base
/// stores the field and reads it back with tag `005`; the register page at
/// `35777777400` + w, its feature words 0, 1, 2 and 13 revision 14's
/// (A14.9). **Fails** a decode that keeps revision 13's physical space.
#[test]
fn the_device_window_and_its_register_page() {
    let mut p = Prog::default();
    let reg = |k: u32| (tlb::REGISTER_PAGE + k) as Word;
    p.write(w(0o025, 0x1234_5678), (FRAME_BUFFER + 7) as Word, 0o10);
    p.read((FRAME_BUFFER + 7) as Word, 0o11, 0o12);
    for (k, word) in [0u32, 1, 2, 0o13].into_iter().enumerate() {
        p.read(reg(word), 0o20 + k as u64, 0o13);
    }
    p.stop();
    let ms = run_with(&p, &|_| {}, &|_| {});
    expect(
        &ms,
        &[
            (0o10, 0, "the frame buffer's write"),
            (0o11, w(0o005, 0x1234_5678), "the frame buffer: the field, tag 005"),
            (0o12, 0, "no fault"),
            (0o20, (0x5155 << 16) | (14 << 4) | 4, "word 0, MACHINE-ID revision 14"),
            (0o21, 0, "word 1, no level-1 map"),
            (0o22, 4096, "word 2, the TLB's entries"),
            (0o23, 0o34000000000, "word 13, the buffer's device-window address"),
            (0o13, 0, "no fault"),
        ],
    );
    for (engine, m) in &ms {
        assert_eq!(m.tv.read_buffer(7), 0x1234_5678, "{engine}: the buffer holds the field");
    }
}

/// **NXM** (A14.1): the physical memory window past main memory's end is
/// nothing there, a read giving 0 and word 101's NXM bit set; so are
/// slices 1-3 (frame buffers to come), 4-14 (devices), the gap past A
/// memory's window and the reserved register pages; main memory's last
/// word through the window is no NXM. **Fails** a decode that keeps
/// revision 13's physical space, or that gives a reserved slice to
/// something.
#[test]
fn nothing_there_past_main_memory_and_in_the_reserved_slices() {
    const MAIN: u32 = 2 * 1024 * 1024;
    let reg = |k: u32| (tlb::REGISTER_PAGE + k) as Word;
    let mut p = Prog::default();
    let places = [
        phys_va(MAIN),
        0o34100000000,
        0o34377777777,
        0o34400000000,
        0o35677777777,
        0o35700002000,
        0o35777600000,
        phys_va(MAIN - 1),
    ];
    for (k, va) in places.into_iter().enumerate() {
        p.write(0, reg(0o101), 0o32);
        p.read(va, 0o10 + k as u64, 0o32);
        p.read(reg(0o101), 0o20 + k as u64, 0o32);
    }
    p.stop();
    let setup = |m: &mut Machine| m.main[(MAIN - 1) as usize] = w(0o031, 0o777);
    let ms = run_with(&p, &setup, &|_| {});
    let mut rows = vec![(0o32, 0, "no fault anywhere")];
    for k in 0..7 {
        rows.push((0o10 + k, 0, "nothing there reads 0"));
        rows.push((0o20 + k, 1, "word 101's NXM bit"));
    }
    rows.push((0o17, w(0o031, 0o777), "main memory's last word"));
    rows.push((0o27, 0, "no NXM"));
    expect(&ms, &rows);
}

/// **A memory's window faults on every reference, status 7** (A14.1,
/// A14.5): a read and a write at `35700000000` + a fault, and `MAP(MD)`
/// there reads the fixed entry `11`, `0760`, access `01`. **Fails** an A
/// memory window that reaches the bus or the TLB.
#[test]
fn a_memory_s_window_faults_with_status_7() {
    let mut p = Prog::default();
    p.read(0o35700000017, 0o10, 0o11);
    p.write(w(0o025, 1), 0o35700000017, 0o12);
    p.map(0o35700000017, 0o13);
    p.stop();
    let ms = run_with(&p, &|_| {}, &|_| {});
    expect(&ms, &[(0o11, 1, "a read faults"), (0o12, 1, "a write faults")]);
    for (engine, m) in &ms {
        let map = m.mmem[0o13] & tlb::ENTRY_BITS as Word;
        assert_eq!(map, tlb::A_MEMORY_WINDOW_ENTRY as Word, "{engine}: MAP(MD)");
        assert_eq!(tlb::status(map), 7, "{engine}: status 7");
        assert_eq!(map >> 26 & 3, 1, "{engine}: access 01");
    }
}

// --- the walk and the TLB (A14.2-A14.6) -----------------------------------

/// **Two pages that share a TLB index, alternately** (A14.4; S1's check):
/// `VA<21:10>` equal and the tags different, read in turn twice, each
/// reaches its own frame; four walks, one a reference, since each evicts
/// the other. **Fails** a TLB that ignores the tag (the second read gives
/// the first page's word), or one that keys on the index alone.
#[test]
fn two_pages_sharing_an_index_translate_to_their_own_frames() {
    const A: u32 = 0o5 << 10;
    const B: u32 = A | 1 << 22;
    let mut p = Prog::default();
    for k in 0..2 {
        p.read((A | 3) as Word, 0o10 + 2 * k, 0o20);
        p.read((B | 3) as Word, 0o11 + 2 * k, 0o20);
    }
    p.stop();
    let setup = |m: &mut Machine| {
        map_page(m, A, rw(0o200));
        map_page(m, B, rw(0o201));
        m.main[0o200 << 10 | 3] = w(0o025, 0xa);
        m.main[0o201 << 10 | 3] = w(0o025, 0xb);
    };
    let ms = run_with(&p, &setup, &|_| {});
    expect(
        &ms,
        &[
            (0o10, w(0o025, 0xa), "A"),
            (0o11, w(0o025, 0xb), "B, same index"),
            (0o12, w(0o025, 0xa), "A again"),
            (0o13, w(0o025, 0xb), "B again"),
            (0o20, 0, "no fault"),
        ],
    );
    assert_eq!(walks(&ms), [4, 4], "each reference walked, the other's entry evicted");
}

/// **Across 2^31 words** (A14 revision 3): the pages at `17777776000`,
/// the last below 2^31, `20000000000`, the first above, and `27777776000`,
/// above 2^31 with the first's index and another tag, read in turn, each
/// translate to their own frame. **Fails** a signed compare or sign
/// extension anywhere in the decode, index or tag, and a TLB that ignores
/// the tag's top bits.
#[test]
fn pages_either_side_of_2_31_translate_to_their_own_frames() {
    const BELOW: u32 = 0o17777776000;
    const ABOVE: u32 = 0o20000000000;
    const SAME_INDEX: u32 = 0o27777776000;
    let mut p = Prog::default();
    for k in 0..2 {
        p.read(BELOW as Word, 0o10 + 3 * k, 0o20);
        p.read(ABOVE as Word, 0o11 + 3 * k, 0o20);
        p.read(SAME_INDEX as Word, 0o12 + 3 * k, 0o20);
    }
    p.stop();
    let setup = |m: &mut Machine| {
        map_page(m, BELOW, rw(0o200));
        map_page(m, ABOVE, rw(0o201));
        map_page(m, SAME_INDEX, rw(0o202));
        m.main[0o200 << 10] = w(0o025, 1);
        m.main[0o201 << 10] = w(0o025, 2);
        m.main[0o202 << 10] = w(0o025, 3);
    };
    let ms = run_with(&p, &setup, &|_| {});
    expect(
        &ms,
        &[
            (0o10, w(0o025, 1), "below 2^31"),
            (0o11, w(0o025, 2), "above 2^31"),
            (0o12, w(0o025, 3), "above 2^31, the same index as below"),
            (0o13, w(0o025, 1), "below again"),
            (0o14, w(0o025, 2), "above again"),
            (0o15, w(0o025, 3), "the same index again"),
            (0o20, 0, "no fault"),
        ],
    );
}

/// **A stale entry is kept until its invalidation** (A14.4; S1's check): a
/// page read once, its entry then changed in memory to another frame
/// through the physical memory window, reads the old frame; after an
/// invalidation `WRITE-MAP` operation at its address, the new one. **Fails**
/// a TLB that does not hold contents (walking every time reads the new
/// frame at once), and an invalidation that does nothing.
#[test]
fn a_stale_entry_is_kept_until_it_is_invalidated() {
    const VA: u32 = 0o4321 << 10;
    let mut p = Prog::default();
    p.read(VA as Word, 0o10, 0o20);
    let at = {
        let mut probe = Machine::new();
        entry_at(&mut probe, VA) as u32
    };
    p.write(rw(0o201), phys_va(at), 0o20);
    p.read(VA as Word, 0o11, 0o20);
    p.tlb_op(2, VA as Word, 0);
    p.read(VA as Word, 0o12, 0o20);
    p.stop();
    let setup = |m: &mut Machine| {
        map_page(m, VA, rw(0o200));
        m.main[0o200 << 10] = w(0o025, 1);
        m.main[0o201 << 10] = w(0o025, 2);
    };
    let ms = run_with(&p, &setup, &|_| {});
    expect(
        &ms,
        &[
            (0o10, w(0o025, 1), "the old frame"),
            (0o11, w(0o025, 1), "the stale entry: still the old frame"),
            (0o12, w(0o025, 2), "after the invalidation, the new frame"),
            (0o20, 0, "no fault"),
        ],
    );
    assert_eq!(walks(&ms), [2, 2], "a walk before, and one after the invalidation");
}

/// **A direct write and an empty** (A14.4): a direct-write operation loads
/// an entry the table does not hold, `MD`'s tag and `VMA<29:0>`, and the
/// page reads through it with no walk; an empty clears it, so the next read
/// walks and faults on the table's no-entry. A revision-13 map-write word,
/// `<33:32>` 0, and any operation at a window address do nothing. **Fails**
/// an operation decoded from revision 13's enables, and an empty that leaves
/// entries.
#[test]
fn a_direct_write_loads_and_an_empty_clears() {
    const VA: u32 = 0o7654 << 10;
    let mut p = Prog::default();
    // Revision 13's map write: level 1 with <29>, level 2 with <28>.
    p.tlb_op(0, VA as Word, (1 << 29 | 1 << 28 | rw(0o200)) & 0o7777777777);
    p.read(VA as Word, 0o10, 0o11);
    // A direct write at a window address.
    p.tlb_op(1, phys_va(0), rw(0o200) & tlb::ENTRY_BITS as Word);
    p.tlb_op(1, VA as Word, rw(0o200) & tlb::ENTRY_BITS as Word);
    p.read(VA as Word, 0o12, 0o13);
    p.tlb_op(3, 0, 0);
    p.read(VA as Word, 0o14, 0o15);
    p.stop();
    let setup = |m: &mut Machine| m.main[0o200 << 10] = w(0o025, 7);
    let ms = run_with(&p, &setup, &|_| {});
    expect(
        &ms,
        &[
            (0o11, 1, "a revision-13 map write loads nothing: no entry, a fault"),
            (0o12, w(0o025, 7), "through the directly written entry"),
            (0o13, 0, "no fault"),
            (0o15, 1, "after the empty: no entry, a fault"),
        ],
    );
    assert_eq!(walks(&ms), [2, 2], "the two misses walked, the direct write's hit did not");
    for (engine, m) in &ms {
        assert!(m.tlb.sweeps >= 2, "{engine}: the reset's sweep and the empty's");
        assert!(m.tlb.lookup(VA).is_none(), "{engine}: swept");
    }
}

/// **An empty takes N ticks on `rtl`** (A14.4): the same program under
/// `--tlb 4096` and `--tlb 8192` takes 4,096 ticks of 10 ns more, its read
/// after the empty waiting for the sweep; on `micro`, which sweeps at once,
/// the two take the same time. **Fails** a sweep that is not timed, or one
/// not N ticks long.
#[test]
fn an_empty_takes_n_ticks_on_rtl() {
    let mut p = Prog::default();
    p.read(phys_va(0), 0o10, 0o11);
    p.tlb_op(3, 0, 0);
    p.read(phys_va(0), 0o10, 0o11);
    p.stop();
    let time = |entries: usize| {
        let setup = move |m: &mut Machine| m.set_tlb_entries(entries);
        let mut r = Rtl::new(machine(&p, &setup));
        r.boot();
        finish(&mut r, "rtl");
        let mut u = Micro::new(machine(&p, &setup));
        u.boot();
        finish(&mut u, "micro");
        (r.ns(), u.machine().ns)
    };
    let (r4, u4) = time(4096);
    let (r8, u8) = time(8192);
    // The reset's sweep is N ticks as well, and the program's first start
    // waits for it: two sweeps of the difference.
    assert_eq!(r8 - r4, 2 * 4096 * 10, "rtl: the reset's and the empty's sweeps");
    assert_eq!(u8, u4, "micro sweeps at once");
}

/// **No directory, and the no-entry results** (A14.3, A14.6): with base 0
/// a paged read faults and loads nothing, with no read of memory; a
/// directory entry of status 0, a page entry of status 0, an in-core entry
/// whose frame is past main memory, and a status 7 entry found in a table
/// each fault and load nothing, so a second reference walks again; a status
/// 3 entry, retired, loads as in core. **Fails** a walk that loads a
/// no-entry result, that skips the frame check, or that takes status 7 for
/// A memory's window.
#[test]
fn no_entry_faults_and_loads_nothing() {
    const NO_DIR: u32 = 0o1 << 20 | 0o5 << 10;
    const ZERO_ENTRY: u32 = 0o2 << 20 | 0o5 << 10;
    const PAST: u32 = 0o2 << 20 | 0o6 << 10;
    const SEVEN: u32 = 0o2 << 20 | 0o7 << 10;
    const THREE: u32 = 0o2 << 20 | 0o10 << 10;
    let mut p = Prog::default();
    for (k, va) in [NO_DIR, ZERO_ENTRY, PAST, SEVEN].into_iter().enumerate() {
        for j in 0..2 {
            p.read(va as Word, 0o33, 0o10 + 2 * k as u64 + j);
        }
    }
    p.read(THREE as Word, 0o20, 0o21);
    p.stop();
    let setup = |m: &mut Machine| {
        map_page(m, ZERO_ENTRY, FIX);
        map_page(m, PAST, rw(4096));
        map_page(m, SEVEN, entry(7, 3, 0o200));
        // Status 3 shares `<26>` with the access code: read-only, `10`.
        map_page(m, THREE, entry(3, 2, 0o200));
        m.main[0o200 << 10] = w(0o025, 3);
    };
    let ms = run_with(&p, &setup, &|_| {});
    let mut rows: Vec<(u64, Word, &str)> = (0o10..0o20).map(|s| (s, 1, "a fault")).collect();
    rows.push((0o20, w(0o025, 3), "status 3 loads as in core"));
    rows.push((0o21, 0, "no fault"));
    expect(&ms, &rows);
    assert_eq!(walks(&ms), [9, 9], "every no-entry reference walked again");
    for (engine, m) in &ms {
        for va in [NO_DIR, ZERO_ENTRY, PAST, SEVEN] {
            assert!(m.tlb.lookup(va).is_none(), "{engine}: {va:o} not loaded");
        }
    }
}

/// **Base 0 reads nothing** (A14.3): the walk with no directory makes no
/// read; with one, a missing directory entry is one read and a present one
/// two. **Fails** a walk that reads the directory at frame 0.
#[test]
fn a_walk_with_no_directory_reads_nothing() {
    let mut m = Machine::new();
    m.geometry = REV14;
    let va = 0o1234 << 10;
    map_page(&mut m, va, rw(0o200));
    assert_eq!(tlb::walk(&m.main, 0, va).reads, [None, None]);
    assert_eq!(tlb::walk(&m.main, DIR, va).reads.iter().flatten().count(), 2);
    assert_eq!(tlb::walk(&m.main, DIR, 0o3 << 20).reads.iter().flatten().count(), 1);
    assert_eq!(tlb::walk(&m.main, DIR, va).entry, Some(rw(0o200) as u32 & tlb::ENTRY_BITS));
}

/// **A status-1 entry is loaded** (A14.6): a read through it faults, its
/// access being `00`, and `MAP(MD)` of the page then reads its meta bits
/// and slot from the TLB with no further walk. **Fails** a walk that loads
/// only in-core entries.
#[test]
fn a_not_in_core_entry_faults_and_map_md_reads_it_without_a_walk() {
    const VA: u32 = 0o3333 << 10;
    let e = FIX | 1 << 24 | 2 << 22 | 0o123456;
    let mut p = Prog::default();
    p.read(VA as Word, 0o33, 0o10);
    p.map(VA as Word, 0o11);
    p.stop();
    let setup = |m: &mut Machine| map_page(m, VA, e);
    let ms = run_with(&p, &setup, &|_| {});
    expect(&ms, &[(0o10, 1, "a fault")]);
    for (engine, m) in &ms {
        assert_eq!(
            m.mmem[0o11] & tlb::ENTRY_BITS as Word,
            e & tlb::ENTRY_BITS as Word,
            "{engine}: MAP(MD) reads the entry"
        );
    }
    assert_eq!(walks(&ms), [1, 1], "MAP(MD) found it loaded");
}

// --- MAP(MD) and the dispatches (A14.5) -----------------------------------

/// **`MAP(MD)` always looks up** (A14.5): on a fixnum-tagged address it
/// walks and reads the page's entry in `<29:0>`, `<39:32>` 0; and every
/// fixed entry: the physical memory window's (`11`, `1460`, `VA<27:10>`),
/// the device window's (`11`, `1460`, 0), and no entry's (`00`, `0060`, 0).
/// **Fails** a source that looks up only pointer types, or that takes
/// revision 13's layout.
#[test]
fn map_md_reads_the_entry_and_the_fixed_entries() {
    const VA: u32 = 0o2222 << 10;
    let mut p = Prog::default();
    p.map(FIX | VA as Word, 0o10);
    p.map(phys_va(0o1234567), 0o11);
    p.map(0o34000000123, 0o12);
    p.map(0o1111 << 10, 0o13);
    p.stop();
    let setup = |m: &mut Machine| map_page(m, VA, rw_meta(0o300, 1));
    let ms = run_with(&p, &setup, &|_| {});
    let rw_4 = 0b11 << 28 | 0o1460 << 18;
    for (engine, m) in &ms {
        let map = |slot: usize| m.mmem[slot] & !(3 << 30);
        assert_eq!(map(0o10), rw_meta(0o300, 1) & tlb::ENTRY_BITS as Word, "{engine}: the entry");
        assert_eq!(map(0o11), rw_4 | 0o1234567 >> 10, "{engine}: the physical window");
        assert_eq!(map(0o12), rw_4, "{engine}: the device window");
        assert_eq!(map(0o13), 0o60 << 18, "{engine}: no entry");
    }
    assert_eq!(walks(&ms), [2, 2], "the fixnum's page and the unmapped one walked");
}

/// A dispatch on `MD`'s data type and the oldspace map bit (`IR<9>`, the
/// entry's `<23>`): the address is `{type, map bit}`, `MD<37:32>` rotated
/// to `<6:1>`. It writes 1 to M `slot` at entry `{type, 0}` and M 33, 2,
/// at `{type, 1}`.
fn transport(p: &mut Prog, md: Word, slot: u64) {
    let a = p.k(md);
    p.op(ALU | SETA | a_src(a) | MD);
    // Rotate 9: <37:32> to <6:1>; 7 bits.
    p.op(disp(0, 7, 9) | MAP_23 | src(0o12));
    p.fill(1);
    let ty = (md >> 32 & 0o77) as usize;
    let after = p.at() + 6;
    for (bit, value) in [(0, M_ONE), (1, 0o33)] {
        let here = p.at();
        p.dmem.push((ty << 1 | bit, 1 << 14 | here as u32));
        p.op(byte(LDB, 0, 40) | m_src(value) | m_dest(slot));
        p.op(JUMP | ALWAYS | target(after) | N);
        p.fill(1);
    }
}

/// **A dispatch on map bits looks up only for a pointer type** (A14.5; S1's
/// check): a TRANSPORT on a fixnum whose field names an unmapped page walks
/// nothing and takes map bits 1 and 1; on a list pointer, its type set in
/// the pointer-type register (word 222), to a page not in the TLB, it walks
/// once and takes the page's oldspace bit, 0; and on a NULL, type 0, set in
/// the register too, the same (A14.5 revision 1: NULL must be in it).
/// **Fails** a dispatch that looks up every type, and one that ignores the
/// register.
#[test]
fn a_transport_on_a_fixnum_walks_nothing() {
    const OLD: u32 = 0o4444 << 10;
    const UNMAPPED: u32 = 0o5555 << 10;
    let mut p = Prog::default();
    p.m(0o33, 2);
    transport(&mut p, FIX | UNMAPPED as Word, 0o10);
    transport(&mut p, LIST | OLD as Word, 0o11);
    transport(&mut p, (OLD | 1) as Word, 0o12);
    p.stop();
    let setup = |m: &mut Machine| map_page(m, OLD, rw_meta(0o300, 1));
    let types = |m: &mut Machine| m.memory_words.pointer_types = 1 << 0o16 | 1;
    let ms = run_with(&p, &setup, &types);
    expect(
        &ms,
        &[
            (0o10, 2, "fixnum: map bit 1, not oldspace"),
            (0o11, 1, "list: the page's oldspace bit, 0"),
            (0o12, 1, "NULL: the same page, oldspace"),
        ],
    );
    assert_eq!(walks(&ms), [1, 1], "the list pointer walked; the fixnum did not; NULL hit");
}

// --- the location counter (A14.11, L1-L8) ---------------------------------

/// The location counter's 34 bits.
const COUNTER: Word = (1 << 34) - 1;

/// LC <- `lc`, a 34-bit byte address, by a logical write, which takes the
/// word's `<33:32>`.
fn set_lc(p: &mut Prog, lc: Word) {
    let a = p.k(lc);
    p.op(ALU | SETA | a_src(a) | LC);
}
/// M `slot` <- LC's counter, `<33:0>` of the source.
fn read_lc(p: &mut Prog, slot: u64) {
    p.op(ALU | SETM | SRC_LC | m_dest(slot));
}
/// LC <- LC `op` A `offset`: QBRLZ2's and the handlers' branch, `((LC) ADD
/// LC A-offset)`.
fn lc_arith(p: &mut Prog, op: u64, offset: Word) {
    let a = p.k(offset);
    p.op(ALU | op | SRC_LC | a_src(a) | LC);
}

/// The counters each engine's M `slot`s hold, `<33:0>`.
fn expect_lc(ms: &[(&str, Machine)], rows: &[(u64, Word, &str)]) {
    for (engine, m) in ms {
        for &(slot, want, what) in rows {
            let got = m.mmem[slot as usize] & COUNTER;
            assert_eq!(got, want, "{engine}: {what}: LC {got:o}, not {want:o}");
        }
    }
}

const TWO_32: Word = 1 << 32;
const TWO_33: Word = 1 << 33;

/// **L1-L3, L7 and the 2^31 checks**: branches across 2^30 and 2^31 words
/// as LC's adder carries them (A14.11). L1, a carry: 2^32 − 2 bytes plus 4
/// reads 2^32 + 2. L2, a borrow, short and long: 2^32 + 2 minus 4 reads
/// 2^32 − 2, and 2^32 + 2 − 2^20 the same below. L3: `SUB 2` at
/// `LC<31:0>` = 0 reads `LC<33:32>` one lower. L7: `((LC) LC)`, a logical
/// write, keeps `<33:32>`. Across 2^31 words: 2^33 − 2 plus 4 reads 2^33 +
/// 2, and minus 4 back; `M+1` and `M-1` carry too. **Fails** an adder without
/// the carry (L1), without the A side's sign (L2), one that carries into
/// `<32>` and not on into `<33>` (2^31), and a 32-bit LC (all).
#[test]
fn lc_s_adder_carries_branches_across_2_30_and_2_31_words() {
    let mut p = Prog::default();
    let cases: [(Word, u64, Word, Word, &str); 9] = [
        (TWO_32 - 2, ADD, 4, TWO_32 + 2, "L1: a carry"),
        (TWO_32 + 2, ADD, 0xffff_fffc, TWO_32 - 2, "L2: a borrow, short"),
        (TWO_32 + 2, ADD, (1u64 << 32) - (1 << 20), TWO_32 + 2 - (1 << 20), "L2: long"),
        (TWO_33, SUB | CARRY_IN, 2, TWO_33 - 2, "L3: SUB 2 at <31:0> 0"),
        (TWO_33 - 2, ADD, 4, TWO_33 + 2, "2^31: a carry into <33>"),
        (TWO_33 + 2, ADD, 0xffff_fffc, TWO_33 - 2, "2^31: and back"),
        (3 * TWO_32 - 1, M_PLUS_C | CARRY_IN, 0, 3 * TWO_32, "M+1 carries"),
        (3 * TWO_32, M_MINUS_1, 0, 3 * TWO_32 - 2, "M-1 borrows, bit 0 cleared"),
        (COUNTER - 1, ADD, 4, 2, "mod 2^34"),
    ];
    for (k, &(from, op, offset, _, _)) in cases.iter().enumerate() {
        if op == M_PLUS_C | CARRY_IN || op == M_MINUS_1 {
            // M+1 and M-1 of an M word: an odd one, which LC could not
            // hold, and an even one.
            p.m(0o20 + k as u64, from);
            p.op(ALU | op | m_src(0o20 + k as u64) | LC);
        } else {
            set_lc(&mut p, from);
            lc_arith(&mut p, op, offset);
        }
        read_lc(&mut p, 0o10 + k as u64);
    }
    // L7: a logical write of LC from LC.
    set_lc(&mut p, 3 * TWO_32 + 0o100);
    p.op(ALU | SETM | SRC_LC | LC);
    read_lc(&mut p, 0o30);
    p.stop();
    let ms = run_with(&p, &|_| {}, &|_| {});
    let mut rows: Vec<(u64, Word, &str)> = cases
        .iter()
        .enumerate()
        .map(|(k, &(_, _, _, want, what))| (0o10 + k as u64, want, what))
        .collect();
    rows.push((0o30, 3 * TWO_32 + 0o100, "L7: a logical write keeps <33:32>"));
    expect_lc(&ms, &rows);
}

/// **L4 and L6: the FEF's address on M, the offset on A** (A14.11; the
/// contract's §10.4). L4: a relative PC, LC less an FEF address 8 bytes
/// below 2^32, saved and LC rebuilt from it, reads LC again. L6: `:1325`'s
/// write and `:3030`'s swapped one, the FEF at `30000000000` words, its
/// address deposited at `<33:2>` (`<33:32>` 11) on M plus a small offset on
/// A, give `LC<33:32>` = 11; the unswapped `:3030`, the address on A, gives
/// 00. **Fails** an adder that takes A's `<33:32>` or ignores M's.
#[test]
fn lc_rebuilt_from_a_relative_pc_and_an_fef_address() {
    const FEF: Word = TWO_32 - 8;
    const PC: Word = TWO_32 + 8;
    // The FEF at 30000000000 words: 4 x that is 3 x 2^32, <31:0> zero.
    const HIGH_FEF: Word = 3 * TWO_32;
    let mut p = Prog::default();
    let fef = p.k(FEF);
    p.m(0o20, FEF);
    // rel <- LC - FEF; LC <- FEF + rel.
    set_lc(&mut p, PC);
    p.op(ALU | SUB | CARRY_IN | SRC_LC | a_src(fef) | m_dest(0o21));
    p.op(ALU | ADD | m_src(0o20) | a_src(0o21) | LC);
    read_lc(&mut p, 0o10);
    // L6, swapped (address on M) and unswapped (on A).
    p.m(0o22, HIGH_FEF).m(0o23, 0o40);
    let (high, rel) = (p.k(HIGH_FEF), p.k(0o40));
    p.op(ALU | ADD | m_src(0o22) | a_src(rel) | LC);
    read_lc(&mut p, 0o11);
    p.op(ALU | ADD | m_src(0o23) | a_src(high) | LC);
    read_lc(&mut p, 0o12);
    p.stop();
    let ms = run_with(&p, &|_| {}, &|_| {});
    expect_lc(
        &ms,
        &[
            (0o10, PC, "L4: LC rebuilt across 2^30 words"),
            (0o11, HIGH_FEF + 0o40, "L6: the address on M"),
            (0o12, 0o40, "L6: the address on A, <33:32> 00"),
        ],
    );
    for (engine, m) in &ms {
        assert_eq!(m.mmem[0o21] & 0xffff_ffff, 16, "{engine}: L4: the relative PC, 16 bytes");
    }
}

/// **L5: QLENX's left-shifted write** (A14.11): LC <- (M + A) shifted left,
/// M twice the FEF's address, `<33:32>` holding its top bits as the
/// deposits leave them, and A the start PC: an FEF at `20000000000` words
/// gives `LC<33:32>` = 10, and one at `10000000000` 01. **Fails** an adder
/// without the shift's bit, which gives 01 and 00, and one that ignores the
/// shift, 00 and 00.
#[test]
fn qlenx_s_shifted_write_takes_the_sum_s_carry() {
    const PC: Word = 0o24;
    let mut p = Prog::default();
    for (k, fef) in [1u64 << 31, 1 << 30].into_iter().enumerate() {
        p.m(0o20 + k as u64, 2 * fef);
        let a = p.k(PC);
        p.op(ALU | OB_LEFT | ADD | m_src(0o20 + k as u64) | a_src(a) | LC);
        read_lc(&mut p, 0o10 + k as u64);
    }
    p.stop();
    let ms = run_with(&p, &|_| {}, &|_| {});
    expect_lc(
        &ms,
        &[(0o10, TWO_33 + 2 * PC, "an FEF at 2^31 words"), (0o11, TWO_32 + 2 * PC, "at 2^30")],
    );
}

/// The program's halfword return into the next word, `SPC<14>` set: a
/// POPJ that steps the counter and, NEED-FETCH up, fetches.
fn step_and_fetch(p: &mut Prog) {
    let next = p.at() + 2;
    let a = p.k(1 << 14 | next);
    p.op(ALU | SETA | a_src(a) | SPC_PUSH);
    p.op(filler().raw() | POPJ);
}

/// **L8: the stepper carries, and a fetch's `VMA` is `LC<33:2>`** (A14.11):
/// LC written at 2^32 − 2 bytes fetches the word at 2^30 − 1 and steps to
/// 2^32, `<33:32>` 01; the next step fetches the word at 2^30 words; across
/// 2^31 words the same from 2^33 − 2, reading the word at 2^31. On `rtl`
/// with the prefetch fitted. **Fails** a 32-bit stepper, one that carries
/// into `<32>` alone, and a fetch that takes revision 13's `LC<29:2>`.
#[test]
fn the_stepper_carries_and_a_fetch_reads_lc_33_2() {
    const W30: u32 = 1 << 30;
    const W31: u32 = 1 << 31;
    let mut p = Prog::default();
    for (k, lc) in [TWO_32 - 2, TWO_33 - 2].into_iter().enumerate() {
        let k = k as u64;
        set_lc(&mut p, lc);
        step_and_fetch(&mut p);
        p.fill(2);
        p.op(ALU | SETM | SRC_MD | m_dest(0o10 + 4 * k));
        step_and_fetch(&mut p);
        p.fill(2);
        p.op(ALU | SETM | SRC_MD | m_dest(0o11 + 4 * k));
        read_lc(&mut p, 0o12 + 4 * k);
    }
    p.stop();
    let setup = |m: &mut Machine| {
        for (k, base) in [W30, W31].into_iter().enumerate() {
            let k = k as u32;
            map_page(m, base - 1024, rw(0o200 + 2 * k));
            map_page(m, base, rw(0o201 + 2 * k));
            m.main[((0o200 + 2 * k) << 10 | 0o1777) as usize] = w(0o025, 0x10 + k as u64);
            m.main[((0o201 + 2 * k) << 10) as usize] = w(0o025, 0x20 + k as u64);
        }
    };
    let ms = run_with(&p, &setup, &|_| {});
    expect(
        &ms,
        &[
            (0o10, w(0o025, 0x10), "the word below 2^30"),
            (0o11, w(0o025, 0x20), "the word at 2^30"),
            (0o14, w(0o025, 0x11), "the word below 2^31"),
            (0o15, w(0o025, 0x21), "the word at 2^31"),
        ],
    );
    expect_lc(
        &ms,
        &[(0o12, TWO_32 + 2, "stepped across 2^30 words"), (0o16, TWO_33 + 2, "across 2^31")],
    );
}

// --- condition 12 (A14.10) --------------------------------------------------

/// **Condition 12, M ≤ A on the fields, unsigned** (A14.10): at 0 and
/// `37777777777` both ways, at equal fields with different tags, and at
/// `17777777777` and `20000000000` both ways, the reverse of a signed
/// compare. **Fails** revision 13's decode of 12 as condition 2, M ≤ A
/// signed, and a compare that sees the tags.
#[test]
fn condition_12_is_m_at_most_a_unsigned() {
    let mut p = Prog::default();
    let pairs: [(Word, Word, Word); 7] = [
        (0, 0o37777777777, 1),
        (0o37777777777, 0, 0),
        (w(0o005, 7), w(0o003, 7), 1),
        (0o17777777777, 0o20000000000, 1),
        (0o20000000000, 0o17777777777, 0),
        (5, 5, 1),
        (6, 5, 0),
    ];
    for (k, &(m, a, _)) in pairs.iter().enumerate() {
        p.m(0o20 + k as u64, m);
        let a = p.k(a);
        p.taken(jcond(0o12) | m_src(0o20 + k as u64) | a_src(a), 0o10 + k as u64);
    }
    p.stop();
    let ms = run_with(&p, &|_| {}, &|_| {});
    let rows: Vec<(u64, Word, &str)> = pairs
        .iter()
        .enumerate()
        .map(|(k, &(_, _, want))| (0o10 + k as u64, want, "M <= A"))
        .collect();
    expect(&ms, &rows);
}

// --- the register page's memory system words (A14.9) ------------------------

/// **Words 220-224** (A14.9): the directory base `<17:0>`, the enable
/// `<0>` and the pointer-type register read back as written; word 224 reads
/// its count and a write clears it; 225-227 read 0. The program sets the
/// directory base itself, and a paged read through it then translates.
/// A boot clears them. **Fails** words that are not there, or a base the
/// walk does not take.
#[test]
fn the_memory_system_words_read_back_and_set_the_directory() {
    const VA: u32 = 0o6543 << 10;
    let reg = |k: u32| (tlb::REGISTER_PAGE + k) as Word;
    let mut p = Prog::default();
    p.write(DIR as Word, reg(0o220), 0o33);
    p.write(0o777777777777, reg(0o221), 0o33);
    p.write(0o12345670123, reg(0o222), 0o33);
    p.write(0o32101234567, reg(0o223), 0o33);
    p.write(0, reg(0o224), 0o33);
    for (k, word) in [0o220u32, 0o221, 0o222, 0o223, 0o224, 0o225].into_iter().enumerate() {
        p.read(reg(word), 0o10 + k as u64, 0o33);
    }
    p.read(VA as Word, 0o20, 0o21);
    p.stop();
    let setup = |m: &mut Machine| {
        map_page(m, VA, rw(0o200));
        m.main[0o200 << 10] = w(0o025, 0o42);
    };
    let ms = run_with(&p, &setup, &|m| {
        m.memory_words.directory = 0;
        m.memory_words.refused = 5;
    });
    expect(
        &ms,
        &[
            (0o10, DIR as Word, "220, the directory base"),
            (0o11, 1, "221, the enable's <0>"),
            (0o12, 0o12345670123, "222"),
            (0o13, 0o32101234567, "223"),
            (0o14, 0, "224, cleared by its write"),
            (0o15, 0, "225, reserved"),
            (0o20, w(0o025, 0o42), "the page through the directory the program set"),
            (0o21, 0, "no fault"),
            (0o33, 0, "no fault"),
        ],
    );
    // A boot clears them and sweeps the TLB.
    for (engine, m) in ms {
        let mut u = Micro::new(m);
        u.boot();
        assert_eq!(u.machine().memory_words, tlb::Words::default(), "{engine}: cleared by a boot");
        assert!(u.machine().tlb.lookup(VA).is_none(), "{engine}: swept by a boot");
    }
}

// --- the checkpoint (A14.14) ------------------------------------------------

/// **A revision-14 checkpoint** (A14.14): the location counter's 34 bits
/// and the memory system's words come back on both engines; the TLB does
/// not, a resume starting with it swept; and the geometry it records is
/// revision 14's. **Fails** a checkpoint that keeps revision 13's 32-bit
/// counter word alone.
#[test]
fn a_checkpoint_keeps_the_34_bit_counter_and_the_words() {
    const VA: u32 = 0o1212 << 10;
    let mut p = Prog::default();
    set_lc(&mut p, 3 * TWO_32 + 0o4444);
    p.read(VA as Word, 0o10, 0o11);
    p.stop();
    let setup = |m: &mut Machine| map_page(m, VA, rw(0o200));
    let words = |m: &mut Machine| {
        m.memory_words.pointer_types = 0o123 << 32 | 0o456;
        m.pdl_copies = tlb::PdlCopies { base: 0o27654321000, head: 0o12345 };
    };
    for engine in ["micro", "rtl"] {
        let (body, saved_lc) = match engine {
            "micro" => {
                let mut u = Micro::new(machine(&p, &setup));
                u.boot();
                with_directory(u.machine_mut());
                words(u.machine_mut());
                finish(&mut u, engine);
                let mut wr = muir::checkpoint::Writer::new();
                u.save(&mut wr);
                (wr.finish(), u.lc_wide())
            }
            _ => {
                let mut r = Rtl::new(machine(&p, &setup));
                r.boot();
                with_directory(r.machine_mut());
                words(r.machine_mut());
                finish(&mut r, engine);
                let mut wr = muir::checkpoint::Writer::new();
                r.save(&mut wr);
                (wr.finish(), r.lc_wide())
            }
        };
        assert_eq!(saved_lc >> 32, 3, "{engine}: <33:32> before the save");
        let g = Machine::checkpointed_geometry_at(&body, 40).unwrap();
        assert_eq!(g.revision(), Some(14), "{engine}: the revision recorded");
        let fresh = machine(&p, &setup);
        let (lc, m) = if engine == "micro" {
            let mut u = Micro::new(fresh);
            u.load(&mut muir::checkpoint::Reader::for_word_bits(&body, 40)).unwrap();
            (u.lc_wide(), u.machine().clone())
        } else {
            let mut r = Rtl::new(fresh);
            r.load(&mut muir::checkpoint::Reader::for_word_bits(&body, 40)).unwrap();
            (r.lc_wide(), r.machine().clone())
        };
        assert_eq!(lc, saved_lc, "{engine}: LC<33:0> resumed");
        assert_eq!(m.memory_words.directory, DIR, "{engine}: word 220 resumed");
        assert_eq!(m.memory_words.pointer_types, 0o123 << 32 | 0o456, "{engine}: 222-223");
        assert_eq!(
            m.pdl_copies,
            tlb::PdlCopies { base: 0o27654321000, head: 0o12345 },
            "{engine}: the redirect's copies"
        );
        assert!(m.tlb.lookup(VA).is_none(), "{engine}: the TLB not kept");
    }
}

/// **A revision-13 checkpoint is not a revision-14 machine's** (A14.13,
/// decisions.md:255): the geometry each records is its own revision, which
/// `quux` refuses by (`tests/quux_revision.rs`).
#[test]
fn a_revision_13_checkpoint_records_revision_13() {
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
    let mut wr = muir::checkpoint::Writer::new();
    m.save(&mut wr);
    let g = Machine::checkpointed_geometry_at(&wr.finish(), 40).unwrap();
    assert_eq!(g.revision(), Some(13));
}

// --- the write-backs and the setter (A14.6, A14.8; slice S1b) ---------------

/// Accessed, modified and ephemeral-reference as page-entry bits.
const A: Word = tlb::ACCESSED as Word;
const M: Word = tlb::MODIFIED as Word;
const E: Word = tlb::EPHEMERAL as Word;

/// The table's entry for `va` on each engine.
fn table(ms: &[(&str, Machine)], va: u32) -> [Word; 2] {
    let at = |m: &Machine| {
        let mut m = m.clone();
        entry_at(&mut m, va)
    };
    [ms[0].1.main[at(&ms[0].1)], ms[1].1.main[at(&ms[1].1)]]
}

/// Write-backs each engine made.
fn write_backs(ms: &[(&str, Machine)]) -> [u64; 2] {
    [ms[0].1.tlb.write_backs, ms[1].1.tlb.write_backs]
}

/// **Accessed is set by the first reference that does not fault** (A14.6):
/// `MAP(MD)`'s walk loads the entry with accessed 0 and writes nothing back;
/// the first read then sets it in the TLB entry and in the table, and a
/// second read writes nothing back; a write that faults on a read-only page
/// sets nothing. **Fails** accessed set by the walk, a write-back on every
/// reference, and one by a faulting reference.
#[test]
fn accessed_is_set_by_the_first_reference_that_does_not_fault() {
    const PAGE: u32 = 0o1001 << 10;
    const RO: u32 = 0o1002 << 10;
    let mut p = Prog::default();
    p.map(PAGE as Word, 0o10);
    p.read(PAGE as Word, 0o33, 0o11);
    p.map(PAGE as Word, 0o12);
    p.read(PAGE as Word, 0o33, 0o11);
    p.write(w(0o025, 1), RO as Word, 0o13);
    p.stop();
    let setup = |m: &mut Machine| {
        map_page(m, PAGE, rw(0o200));
        map_page(m, RO, entry(2, 2, 0o201));
    };
    let ms = run_with(&p, &setup, &|_| {});
    expect(&ms, &[(0o11, 0, "the reads do not fault"), (0o13, 1, "the write faults")]);
    for (engine, m) in &ms {
        assert_eq!(m.mmem[0o10] & A, 0, "{engine}: MAP(MD)'s walk sets no accessed");
        assert_eq!(m.mmem[0o12] & A, A, "{engine}: the read's accessed, in the TLB entry");
    }
    assert_eq!(table(&ms, PAGE), [rw(0o200) | A; 2], "accessed written back");
    assert_eq!(table(&ms, RO), [entry(2, 2, 0o201); 2], "nothing for the faulting write");
    assert_eq!(write_backs(&ms), [1, 1], "one write-back, the first read's");
}

/// **Modified is set by the first write, and the write-back is an OR**
/// (A14.6; S1's check): a read sets accessed, a write modified, a second
/// write nothing. A TLB entry written directly with `<18>` 0 over a table
/// entry with `<18>` 1, and one with access `11` over a read-only table
/// entry, as FORCE-WR-RDONLY does, leave the table's `<18>` and access code
/// as they were after a write sets accessed and modified. **Fails** a
/// write-back that copies the TLB entry into the table, and modified set by
/// a read.
#[test]
fn modified_is_set_by_the_first_write_and_the_table_keeps_its_bits() {
    const PAGE: u32 = 0o1011 << 10;
    const PLANTED: u32 = 0o1012 << 10;
    const FORCED: u32 = 0o1013 << 10;
    let table_18 = rw(0o202) | 1 << 18;
    let read_only = entry(2, 2, 0o203);
    let mut p = Prog::default();
    p.read(PAGE as Word, 0o33, 0o10);
    p.map(PAGE as Word, 0o11);
    p.write(w(0o025, 1), PAGE as Word, 0o10);
    p.write(w(0o025, 2), PAGE as Word, 0o10);
    p.tlb_op(1, PLANTED as Word, rw(0o202) & tlb::ENTRY_BITS as Word);
    p.write(w(0o025, 3), PLANTED as Word, 0o10);
    p.tlb_op(1, FORCED as Word, (read_only | 3 << 26) & tlb::ENTRY_BITS as Word);
    p.write(w(0o025, 4), FORCED as Word, 0o10);
    p.stop();
    let setup = |m: &mut Machine| {
        map_page(m, PAGE, rw(0o200));
        map_page(m, PLANTED, table_18);
        map_page(m, FORCED, read_only);
    };
    let ms = run_with(&p, &setup, &|_| {});
    expect(&ms, &[(0o10, 0, "no fault")]);
    for (engine, m) in &ms {
        assert_eq!(m.mmem[0o11] & (A | M), A, "{engine}: the read set accessed alone");
    }
    assert_eq!(table(&ms, PAGE), [rw(0o200) | A | M; 2], "accessed, then modified");
    assert_eq!(table(&ms, PLANTED), [table_18 | A | M; 2], "the table's <18> kept");
    assert_eq!(table(&ms, FORCED), [read_only | A | M; 2], "the table's access code kept");
    assert_eq!(write_backs(&ms), [4, 4], "the read, the first write, and one each planted");
}

/// **The guard** (A14.6; S1's planted race): a page read, so its entry is
/// in the TLB with accessed set, then its table entry made status 1 in
/// memory with no invalidation; a write through the stale TLB entry writes
/// nothing to the table and counts one in word 224. Another page's entry
/// moved to another frame the same way counts a second. A write of word 224
/// clears it. **Fails** a write-back with no guard, which ORs modified into
/// the status-1 entry's slot, and one that compares no frame.
#[test]
fn the_guard_refuses_a_write_through_a_stale_entry() {
    const SWAPPED: u32 = 0o1021 << 10;
    const MOVED: u32 = 0o1022 << 10;
    let not_in_core = FIX | 1 << 24 | 3 << 22 | 0o12345;
    let moved = rw(0o205);
    let reg = |k: u32| (tlb::REGISTER_PAGE + k) as Word;
    let at = |va: u32| {
        let mut probe = Machine::new();
        entry_at(&mut probe, va) as u32
    };
    let mut p = Prog::default();
    p.read(SWAPPED as Word, 0o33, 0o10);
    p.read(MOVED as Word, 0o33, 0o10);
    p.write(not_in_core, phys_va(at(SWAPPED)), 0o10);
    p.write(moved, phys_va(at(MOVED)), 0o10);
    p.write(w(0o025, 1), SWAPPED as Word, 0o10);
    p.read(reg(0o224), 0o11, 0o10);
    p.write(w(0o025, 2), MOVED as Word, 0o10);
    p.read(reg(0o224), 0o12, 0o10);
    p.write(0, reg(0o224), 0o10);
    p.read(reg(0o224), 0o13, 0o10);
    p.stop();
    let setup = |m: &mut Machine| {
        map_page(m, SWAPPED, rw(0o200));
        map_page(m, MOVED, rw(0o201));
    };
    let ms = run_with(&p, &setup, &|_| {});
    expect(
        &ms,
        &[
            (0o10, 0, "no fault: the stale entries still permit"),
            (0o11, 1, "word 224: one refused"),
            (0o12, 2, "two refused"),
            (0o13, 0, "cleared by its write"),
        ],
    );
    assert_eq!(table(&ms, SWAPPED), [not_in_core; 2], "the status-1 entry untouched");
    assert_eq!(table(&ms, MOVED), [moved; 2], "the moved entry untouched");
}

/// The setter's program: the enable, then a list pointer and a fixnum into
/// ephemeral space and list pointers below it and past 2^31, each stored to
/// a page of its own, the first twice; and a store through the physical
/// memory window to a mapped page's frame.
const SET_PAGES: [u32; 5] = [0o1031 << 10, 0o1032 << 10, 0o1033 << 10, 0o1034 << 10, 0o1035 << 10];

fn setter(enable: Word) -> Prog {
    let reg = |k: u32| (tlb::REGISTER_PAGE + k) as Word;
    let mut p = Prog::default();
    p.write(enable, reg(0o221), 0o10);
    p.write(1 << 0o16, reg(0o222), 0o10);
    let young = LIST | 0o32000000000;
    for (word, page) in [
        (young, SET_PAGES[0]),
        (young | 7, SET_PAGES[0]),
        (FIX | 0o32000000000, SET_PAGES[1]),
        (LIST | 0o31777777777, SET_PAGES[2]),
        (LIST | 0o20000000000, SET_PAGES[3]),
    ] {
        p.write(word, page as Word, 0o10);
    }
    p.write(young, phys_va(0o204 << 10), 0o10);
    p.stop();
    p
}

fn setter_tables(m: &mut Machine) {
    for (k, &page) in SET_PAGES.iter().enumerate() {
        map_page(m, page, rw(0o200 + k as u32));
    }
}

/// **The ephemeral-reference setter** (A14.8): with the enable 1 and the
/// list type in the pointer-type register, a list pointer to
/// `32000000000` stored sets `<19>` with accessed and modified in one
/// write-back, and a second store writes nothing back; a fixnum whose field
/// is `32000000000`, a list pointer to `31777777777` and one to
/// `20000000000` (`MD<31:28>` = `1000`, past 2^31) set nothing; a store
/// through the physical memory window sets no bit at all. With the enable
/// 0, nothing. **Fails** a setter that ignores the pointer-type register,
/// the `1101` compare (or compares magnitudes), or the enable, and one
/// write-back per bit.
#[test]
fn the_setter_marks_a_store_of_an_ephemeral_pointer() {
    let ms = run_with(&setter(1), &setter_tables, &|_| {});
    expect(&ms, &[(0o10, 0, "no fault")]);
    let am = rw(0o200) | A | M;
    assert_eq!(table(&ms, SET_PAGES[0]), [am | E; 2], "the young list pointer marks");
    for (k, what) in [(1, "a fixnum"), (2, "below ephemeral space"), (3, "past 2^31, 1000")] {
        let want = rw(0o200 + k as u32) | A | M;
        assert_eq!(table(&ms, SET_PAGES[k]), [want; 2], "{what} marks nothing");
    }
    assert_eq!(table(&ms, SET_PAGES[4]), [rw(0o204); 2], "the window's store sets nothing");
    assert_eq!(write_backs(&ms), [4, 4], "one write-back a page, the second store none");
    for (engine, m) in &ms {
        assert_eq!(m.tlb.written_bits, [4, 4, 1], "{engine}: accessed, modified, <19>");
    }
    let ms = run_with(&setter(0), &setter_tables, &|_| {});
    assert_eq!(table(&ms, SET_PAGES[0]), [am; 2], "the enable 0: nothing");
}

/// **On `rtl` a write-back holds the reference** (A14.6): a page read once,
/// its line then in the cache, its table entry rewritten through the
/// physical memory window with accessed 0 or 1 and its TLB entry
/// invalidated, read again right after a write: with accessed 0 the walk
/// is followed by a write-back, and the read, a hit in the cache, waits
/// behind its write, which waits for the write buffer; `micro` takes the
/// same time either way. **Fails** a
/// write-back the reference's cycle does not wait for.
#[test]
fn a_write_back_holds_the_reference_on_rtl() {
    const PAGE: u32 = 0o1041 << 10;
    let at = {
        let mut probe = Machine::new();
        entry_at(&mut probe, PAGE) as u32
    };
    let time = |e: Word| {
        let mut p = Prog::default();
        p.read(PAGE as Word, 0o33, 0o10);
        p.write(rw(0o200) | e, phys_va(at), 0o10);
        p.tlb_op(2, PAGE as Word, 0);
        // A write to another word just before, so that the write buffer is
        // full when the write-back's write comes.
        let (junk, page) = (p.k(phys_va(0o300 << 10)), p.k(PAGE as Word));
        p.op(ALU | SETA | a_src(junk) | START_WRITE);
        p.op(ALU | SETA | a_src(page) | START_READ);
        p.fill(1);
        p.op(ALU | SETM | SRC_MD | m_dest(0o11));
        p.stop();
        let setup = |m: &mut Machine| map_page(m, PAGE, rw(0o200) | A);
        let mut r = Rtl::new(machine(&p, &setup));
        r.boot();
        with_directory(r.machine_mut());
        finish(&mut r, "rtl");
        let mut u = Micro::new(machine(&p, &setup));
        u.boot();
        with_directory(u.machine_mut());
        finish(&mut u, "micro");
        (r.ns(), u.machine().ns, r.machine().tlb.write_backs)
    };
    let (r0, u0, wb0) = time(0);
    let (r1, u1, wb1) = time(A);
    assert_eq!((wb0, wb1), (1, 0), "the second read writes accessed back, or not");
    assert!(r0 > r1, "rtl: the write-back holds the read: {r0} ns against {r1}");
    assert_eq!(u0, u1, "micro times no write-back");
}

// --- the PDL buffer redirect (A14.7) -----------------------------------------

/// A status-5 page entry: access `01`, every reference faulting, at
/// `frame`.
const fn pdl_entry(frame: u32) -> Word {
    entry(5, 1, frame)
}

/// The PDL buffer's word `k` as the tests plant it.
const fn pdl_word(k: u64) -> Word {
    w(0o031, 0o7000 + k)
}

impl Prog {
    /// The redirect's copies: A 430 <- `base`, A 431 <- `head`.
    fn copies(&mut self, base: u32, head: u64) -> &mut Self {
        let (b, h) = (self.k(base as Word), self.k(head));
        self.op(ALU | SETA | a_src(b) | a_dest(tlb::A_PDL_BUFFER_VIRTUAL_ADDRESS as u64));
        self.op(ALU | SETA | a_src(h) | a_dest(tlb::A_PDL_BUFFER_HEAD as u64))
    }
}

/// The PDL buffer planted, its pointer at `pp` after the boot.
fn pdl_at(pp: u16) -> impl Fn(&mut Machine) {
    move |m: &mut Machine| m.pdl_pointer = pp
}
fn plant_pdl(m: &mut Machine) {
    for k in 0..m.pdl.len() {
        m.pdl[k] = pdl_word(k as u64);
    }
}

/// **The redirect at off = 0, n − 1, n and n + 1** (A14.7; S1's check, the
/// word one past PP): with the head at 100 and PP at 107, n = 10 (octal);
/// reads at the base plus 0, 7 and 10 take the PDL buffer's words 100, 107
/// and 110 with no fault, and plus 11 goes to memory as if its access code
/// were `11`. A write inside puts its word in the buffer and nothing in
/// memory; one outside goes to memory. Outside references set accessed and
/// modified, inside ones nothing; `MAP(MD)` reads the entry as stored.
/// **Fails** a redirect that is not built (status 5 faults), a test of off
/// < n, which sends the word one past PP to memory, and a redirect that
/// writes back or reaches memory from inside.
#[test]
fn the_redirect_takes_the_buffer_up_to_the_word_past_pp() {
    const PAGE: u32 = 0o2001 << 10;
    const BASE: u32 = PAGE | 0o20;
    let mut p = Prog::default();
    p.copies(BASE, 0o100);
    for (k, off) in [0u32, 7, 8].into_iter().enumerate() {
        p.read((BASE + off) as Word, 0o10 + k as u64, 0o20);
    }
    p.write(w(0o025, 0o111), (BASE + 1) as Word, 0o20);
    // The inside references have set nothing, in the TLB entry either.
    p.map(BASE as Word, 0o22);
    p.read((BASE + 9) as Word, 0o13, 0o20);
    p.write(w(0o025, 0o222), (BASE + 9) as Word, 0o20);
    p.map(BASE as Word, 0o21);
    p.stop();
    let setup = |m: &mut Machine| {
        plant_pdl(m);
        map_page(m, PAGE, pdl_entry(0o200));
        m.main[0o200 << 10 | 0o31] = w(0o025, 0x600d);
    };
    let ms = run_with(&p, &setup, &pdl_at(0o107));
    expect(
        &ms,
        &[
            (0o10, pdl_word(0o100), "off 0: the head"),
            (0o11, pdl_word(0o107), "off n - 1: PP"),
            (0o12, pdl_word(0o110), "off n: the word one past PP"),
            (0o13, w(0o025, 0x600d), "off n + 1: memory"),
            (0o20, 0, "no fault"),
        ],
    );
    for (engine, m) in &ms {
        assert_eq!(m.pdl[0o101], w(0o025, 0o111), "{engine}: the inside write in the buffer");
        assert_eq!(m.main[0o200 << 10 | 0o21], 0, "{engine}: and not in memory");
        assert_eq!(m.main[0o200 << 10 | 0o31], w(0o025, 0o222), "{engine}: the outside write");
        assert_eq!(
            m.mmem[0o21] & tlb::ENTRY_BITS as Word,
            (pdl_entry(0o200) | A | M) & tlb::ENTRY_BITS as Word,
            "{engine}: MAP(MD) as stored, status 5, with the outside references' bits"
        );
        assert_eq!(m.tlb.redirects, [4, 2], "{engine}: four inside, two outside");
        assert_eq!(m.mmem[0o22] & (A | M), 0, "{engine}: inside: no accessed, no modified");
    }
    assert_eq!(table(&ms, PAGE), [pdl_entry(0o200) | A | M; 2], "outside: accessed, modified");
    assert_eq!(write_backs(&ms), [2, 2], "the outside read's and write's, none inside");
}

/// **A start right after a write of A 431 uses the new head** (A14.7; S1's
/// check): A 431 written in microcycle n, the start in n + 1 reads the
/// buffer at the new head. **Fails** copies taken later than A memory's
/// write pulse, or read from A memory at the redirect's own time.
#[test]
fn a_start_right_after_a_write_of_a_431_uses_the_new_head() {
    const PAGE: u32 = 0o2002 << 10;
    let mut p = Prog::default();
    p.copies(PAGE, 0o100);
    p.fill(2);
    let (head, va) = (p.k(0o200), p.k(PAGE as Word));
    p.op(ALU | SETA | a_src(head) | a_dest(tlb::A_PDL_BUFFER_HEAD as u64));
    p.op(ALU | SETA | a_src(va) | START_READ);
    p.fill(2);
    p.op(ALU | SETM | SRC_MD | m_dest(0o10));
    p.stop();
    let setup = |m: &mut Machine| {
        plant_pdl(m);
        map_page(m, PAGE, pdl_entry(0o200));
    };
    let ms = run_with(&p, &setup, &pdl_at(0o107));
    expect(&ms, &[(0o10, pdl_word(0o200), "the word at the new head")]);
}

/// **An access-`11` entry of status 5 goes to memory** (A14.7): the
/// microcode's fiddle, a direct write of the entry with access `11`, does
/// not fault, so the redirect does not fire. **Fails** a redirect keyed on
/// the status alone.
#[test]
fn an_access_11_status_5_entry_goes_to_memory() {
    const PAGE: u32 = 0o2003 << 10;
    let mut p = Prog::default();
    p.copies(PAGE, 0o100);
    p.tlb_op(1, PAGE as Word, entry(5, 3, 0o200) & tlb::ENTRY_BITS as Word);
    p.read(PAGE as Word, 0o10, 0o11);
    p.stop();
    let setup = |m: &mut Machine| {
        plant_pdl(m);
        map_page(m, PAGE, pdl_entry(0o200));
        m.main[0o200 << 10] = w(0o025, 0x600d);
    };
    let ms = run_with(&p, &setup, &pdl_at(0o107));
    expect(&ms, &[(0o10, w(0o025, 0x600d), "memory's word"), (0o11, 0, "no fault")]);
    for (engine, m) in &ms {
        assert_eq!(m.tlb.redirects, [0, 0], "{engine}: no redirect");
    }
}

/// **The redirect across 2^31 words** (A14 revision 3): the base 8 words
/// below 2^31 and PP 12 (octal) past the head, n = 13, so the buffer runs
/// to `20000000003`: off 0, n − 1 and n inside, n + 1, `20000000004`,
/// through memory. **Fails** a signed compare of the address with the base.
#[test]
fn the_redirect_across_2_31_words() {
    const LOW: u32 = 0o17777776000;
    const HIGH: u32 = 0o20000000000;
    const BASE: u32 = 0o17777777770;
    let mut p = Prog::default();
    p.copies(BASE, 0o100);
    for (k, off) in [0u32, 10, 11, 12].into_iter().enumerate() {
        p.read(BASE.wrapping_add(off) as Word, 0o10 + k as u64, 0o20);
    }
    p.stop();
    let setup = |m: &mut Machine| {
        plant_pdl(m);
        map_page(m, LOW, pdl_entry(0o200));
        map_page(m, HIGH, pdl_entry(0o201));
        m.main[0o201 << 10 | 4] = w(0o025, 0x600d);
    };
    let ms = run_with(&p, &setup, &pdl_at(0o112));
    expect(
        &ms,
        &[
            (0o10, pdl_word(0o100), "off 0, below 2^31"),
            (0o11, pdl_word(0o112), "off n - 1, above"),
            (0o12, pdl_word(0o113), "off n"),
            (0o13, w(0o025, 0x600d), "off n + 1: memory"),
            (0o20, 0, "no fault"),
        ],
    );
}

/// **On `rtl` a redirect inside holds one microcycle** (A14.7's timing):
/// a second read inside the buffer, its page's entry in the TLB, adds one
/// microcycle of hold and no memory cycle; `micro` holds nothing. **Fails**
/// a redirect that is not timed, or that runs a memory cycle.
#[test]
fn a_redirect_inside_holds_one_microcycle_on_rtl() {
    const PAGE: u32 = 0o2004 << 10;
    let run = |reads: usize| {
        let mut p = Prog::default();
        p.copies(PAGE, 0o100);
        for _ in 0..reads {
            p.read(PAGE as Word, 0o10, 0o11);
        }
        p.stop();
        let setup = |m: &mut Machine| {
            plant_pdl(m);
            map_page(m, PAGE, pdl_entry(0o200));
        };
        let mut r = Rtl::new(machine(&p, &setup));
        r.boot();
        with_directory(r.machine_mut());
        r.machine_mut().pdl_pointer = 0o107;
        finish(&mut r, "rtl");
        assert_eq!(r.machine().mmem[0o10], pdl_word(0o100), "rtl: the buffer's word");
        (r.machine().tlb.held_ns, r.bus_cycles())
    };
    let (held1, cycles1) = run(1);
    let (held2, cycles2) = run(2);
    assert_eq!(held2 - held1, 40, "one microcycle of 40 ns held for the second");
    assert_eq!(cycles2, cycles1, "no memory cycle inside the buffer");
}
