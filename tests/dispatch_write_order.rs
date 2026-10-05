// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Writes that land inside the microcycle that reads the same memory, and
//! writes whose microcycle is held, on the board, `rtl` and `micro`.
//!
//! Each test is a small boot-PROM program, run on `chip` (MIT's netlist),
//! `rtl` and `micro` from the same memories, and the end states compared.
//! The programs end in a jump to themselves, so the end state does not
//! depend on how many microcycles each engine is given past that point.

mod support;

use muir::cable::FarEnd;
use muir::chip::{Chip, MEMS, Ram};
use muir::clock::{Behavioral, Clock};
use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::*;
use muir::machine::Machine;
use muir::part::Level;

/// `v` into M memory `r` (and the A memory word it shadows): zero, then one
/// doubling a bit, with the bit as the carry in. Nothing but the ALU.
fn constant(v: u32, r: u64, out: &mut Vec<Insn>) {
    out.push(Insn::new(ALU | SETZ | m_dest(r)));
    for b in (0..32).rev() {
        let c = if (v >> b) & 1 != 0 { CARRY_IN } else { 0 };
        out.push(Insn::new(ALU | M_PLUS_M | c | m_src(r) | a_src(r) | m_dest(r)));
    }
}

/// `M[to] <- M[from]`.
fn copy(from: u64, to: u64) -> Insn {
    Insn::new(ALU | SETM | m_src(from) | a_src(3) | m_dest(to))
}

/// A jump to itself.
fn halt_here(at: usize) -> Insn {
    Insn::new(JUMP | target(at as u64) | ALWAYS)
}

/// Functional destination 23, `VMA-WRITE-MAP` (`IR<23:19>`), also into M 37.
const WRITE_MAP: u64 = (0o23 << 19) | (0o37 << 14);
/// Functional source 11, `MAP(MD)`: the map's output for `MAPI`.
const SRC_MAP: u64 = src(0o11);

/// What a run leaves behind, read off whichever engine ran it.
#[derive(Debug, PartialEq, Eq, Clone)]
struct End {
    mmem: Vec<u32>,
    dmem: Vec<u32>,
    l2: Vec<u32>,
    pdl: Vec<u32>,
    spc: Vec<u32>,
    spcptr: u8,
}

/// The PC in each microcycle the processor ran, for the failure message.
type Trace = Vec<u16>;

/// One generator cycle of the board, as `tests/chip.rs` steps it: whether
/// the cpu clock ran in it, and how many times each of `-WP1`..`-WP5` went
/// low in it.
fn generator_cycle(
    c: &mut Chip,
    far: &mut FarEnd,
    clk: &mut Behavioral,
    clk0: muir::netlist::NetId,
    wps: &[muir::netlist::NetId],
    pulses: &mut [u32],
) -> bool {
    let from = clk.time_ns();
    let mut ran = false;
    let mut was: Vec<Level> = wps.iter().map(|&w| c.net(w)).collect();
    loop {
        far.tick_with(c, clk);
        ran |= c.net(clk0) == Level::High;
        for (k, &w) in wps.iter().enumerate() {
            let now = c.net(w);
            if now == Level::Low && was[k] != Level::Low {
                pulses[k] += 1;
            }
            was[k] = now;
        }
        if clk.phase_ns() == 0 && clk.time_ns() > from {
            return ran;
        }
        assert!(clk.time_ns() - from < 60_000, "the generator has not come round");
    }
}

/// What the board did, generator cycle by generator cycle: the PC, whether
/// the cpu clock ran, and the write pulses that fired.
#[derive(Debug, Clone)]
struct Cycle {
    pc: u16,
    ran: bool,
    pulses: [u32; 5],
    /// Nanoseconds from this generator cycle's start to the next's: longer
    /// than the others under `-HANG`, which stops the generator.
    ns: u64,
}

/// The program and memories of `m` on the board --- the PROM as a
/// programming image, every scratchpad and both map levels stored into the
/// RAM cells, `main` put on the memory boards --- booted and run for
/// `cycles` generator cycles.
fn on_chip(m: &Machine, main: &[(u32, u32)], cycles: usize) -> (End, Vec<Cycle>) {
    let mut b = board(m, main);
    let clk0 = b.n.cpu.by_name_id("-CLK0").unwrap();
    let wps: Vec<_> = ["-WP1", "-WP2", "-WP3", "-WP4", "-WP5"]
        .iter()
        .map(|w| b.n.cpu.by_name_id(w).unwrap())
        .collect();
    let mut log = Vec::new();
    for _ in 0..cycles {
        let pc = b.c.bus(&b.n.cpu, "PC", 14) as u16;
        let mut pulses = [0; 5];
        let t0 = b.clk.time_ns();
        let ran = generator_cycle(&mut b.c, &mut b.far, &mut b.clk, clk0, &wps, &mut pulses);
        log.push(Cycle { pc, ran, pulses, ns: b.clk.time_ns() - t0 });
    }
    (board_end(&b), log)
}

/// The board, its clock and the far end of its cables.
struct Board {
    n: support::Netlists,
    c: Chip,
    clk: Behavioral,
    far: FarEnd,
}

/// The program and memories of `m` on the board --- the PROM as a
/// programming image, every scratchpad and both map levels stored into the
/// RAM cells, `main` put on the memory boards --- booted, up to the first
/// microcycle whose `PC` is not 0.
fn board(m: &Machine, main: &[(u32, u32)]) -> Board {
    let n = support::netlists();
    let (mut c, mut clk, mut far) = support::chip(&n);
    let image: Vec<u64> = m.prom.iter().map(|&i| muir::prom::programming(i)).collect();
    c.power_on();
    c.load_prom(&n.cpu, &image);
    let ram = |k: usize, c: &Chip| Ram::new(c, &n.cpu, &MEMS[k]);
    let (a, mm, pdl, spc, dm, l1, l2) =
        (ram(0, &c), ram(1, &c), ram(2, &c), ram(3, &c), ram(4, &c), ram(5, &c), ram(6, &c));
    for (k, &v) in m.amem.iter().enumerate() {
        a.store(&mut c, k, support::low(v));
    }
    for (k, &v) in m.mmem.iter().enumerate() {
        mm.store(&mut c, k, support::low(v));
    }
    for (k, &v) in m.pdl[..pdl.len()].iter().enumerate() {
        pdl.store(&mut c, k, support::low(v));
    }
    for (k, &v) in m.spc.iter().enumerate() {
        spc.store(&mut c, k, v & 0o1777777);
    }
    for (k, &v) in m.dmem[..m.geometry.dmem_words()].iter().enumerate() {
        dm.store(&mut c, k, v & 0o377777);
    }
    for (k, &v) in m.l1_map[..l1.len()].iter().enumerate() {
        l1.store(&mut c, k, v & 0o37);
    }
    for (k, &v) in m.l2_map[..l2.len()].iter().enumerate() {
        l2.store(&mut c, k, v & 0o77777777);
    }
    for &(p, w) in main {
        far.xbus.poke(p, w);
    }
    c.settle();
    far.join(&mut c, clk.time_ns());
    let boot = n.cpu.by_name_id("-BOOT2").unwrap();
    c.set_net(boot, Level::Low);
    c.settle();
    for _ in 0..20 {
        c.tick(&mut clk);
    }
    c.set_net(boot, Level::High);
    let mut skipped = 0;
    while c.bus(&n.cpu, "PC", 14) == 0 && skipped < 40 {
        c.microcycle(&mut clk);
        skipped += 1;
    }
    Board { n, c, clk, far }
}

/// What the board holds at the end of a run.
fn board_end(b: &Board) -> End {
    let (n, c) = (&b.n, &b.c);
    let ram = |k: usize| Ram::new(c, &n.cpu, &MEMS[k]);
    let (mm, dm, l2, pdl, spc) = (ram(1), ram(4), ram(6), ram(2), ram(3));
    End {
        mmem: (0..32).map(|k| mm.word(c, k)).collect(),
        dmem: (0..dm.len()).map(|k| dm.word(c, k)).collect(),
        l2: (0..l2.len()).map(|k| l2.word(c, k)).collect(),
        pdl: (0..pdl.len()).map(|k| pdl.word(c, k)).collect(),
        spc: (0..32).map(|k| spc.word(c, k) & 0o1777777).collect(),
        spcptr: c.bus(&n.cpu, "SPCPTR", 5) as u8,
    }
}

/// The same on an engine, `steps` microcycles.
fn on_engine<E: Engine>(
    new: fn(Machine) -> E,
    boot: fn(&mut E),
    m: &Machine,
    main: &[(u32, u32)],
    steps: usize,
) -> (End, Trace) {
    let mut m = m.clone();
    for &(p, w) in main {
        m.main[p as usize] = u64::from(w);
    }
    let mut e = new(m);
    boot(&mut e);
    let mut trace = Vec::new();
    for _ in 0..steps {
        trace.push(e.pc());
        e.step().unwrap();
    }
    let mm = e.machine();
    let end = End {
        mmem: mm.mmem.iter().map(|&w| support::low(w)).collect(),
        dmem: mm.dmem[..mm.geometry.dmem_words()].iter().map(|&w| w & 0o377777).collect(),
        l2: mm.l2_map[..1024].iter().map(|&w| w & 0o77777777).collect(),
        pdl: mm.pdl[..1024].iter().map(|&w| support::low(w)).collect(),
        spc: mm.spc.iter().map(|&w| w & 0o1777777).collect(),
        spcptr: mm.spcptr,
    };
    (end, trace)
}

fn rtl(m: &Machine, main: &[(u32, u32)], steps: usize) -> (End, Trace) {
    on_engine(muir::rtl::Rtl::new, muir::rtl::Rtl::boot, m, main, steps)
}

fn micro(m: &Machine, main: &[(u32, u32)], steps: usize) -> (End, Trace) {
    on_engine(muir::micro::Micro::new, muir::micro::Micro::boot, m, main, steps)
}

/// PCs in octal, for a failure message.
fn octal(pcs: &[u16]) -> String {
    pcs.iter().map(|p| format!("{p:o}")).collect::<Vec<_>>().join(" ")
}

/// The PCs of the microcycles the board ran.
fn ran_pcs(log: &[Cycle]) -> Vec<u16> {
    log.iter().filter(|c| c.ran).map(|c| c.pc).collect()
}

/// The three engines' ends, and the board's generator cycles and `rtl`'s PCs.
struct Three {
    chip: End,
    log: Vec<Cycle>,
    rtl: End,
    rtl_pcs: Trace,
    micro: End,
}

fn run3(m: &Machine, main: &[(u32, u32)], cycles: usize) -> Three {
    let (chip, log) = on_chip(m, main, cycles);
    let (rtl, rtl_pcs) = rtl(m, main, cycles);
    let (micro, _) = micro(m, main, cycles);
    Three { chip, log, rtl, rtl_pcs, micro }
}

fn program(p: Vec<Insn>) -> Machine {
    let mut p = p;
    p.resize(512, filler());
    let mut m = Machine::new();
    m.load_prom(&p);
    m
}

// --- Q3: a dispatch memory write in the instruction that also pops ---------

/// The dispatch memory word written and read.
const D: u64 = 0o1200;
/// The dispatch word's `R`, `P` and `N` bits, `DR`, `DP`, `DN` (the data
/// nets of pages DRAM0-2 as `muir::chip::MEMS` reads them).
const DR: u32 = 1 << 16;
const DP: u32 = 1 << 15;
const DN: u32 = 1 << 14;
/// What `M5` ends holding: the subroutine returned, or it went to the old
/// word's `DPC`, or to the new word's.
const RETURNED: u32 = 0o1111;
const AT_OLD_DPC: u32 = 0o2222;
const AT_NEW_DPC: u32 = 0o3333;
/// Where the subroutine is, and the two words' `DPC`.
const SUB: usize = 0o400;
const OLD_DPC: u32 = 0o440;
const NEW_DPC: u32 = 0o460;

/// The program: the dispatch word at [`D`] set to `old` by a plain write, a
/// call to [`SUB`], and there **one** instruction that both writes `new`
/// over that word (`DISPATCH` with `IR<11:10>` = 2, `DISPWR`) and has
/// `POPJ` (`IR<42>`), the write's address being the dispatch's.
///
/// `IGNPOPJ` is `DISPATCH AND NOT DR` on page CONTRL. With `DR` set the
/// `POPJ` is taken and the caller's return sets `M5` to [`RETURNED`]. With
/// it clear the pop is not taken, `PCS1` stays up and `PCS0` goes down
/// (`POPJ` is in `PCS0`'s NOR and, suppressed, not in `PCS1`'s), which
/// selects `DPC`: the machine goes to the dispatch word's PC field, where
/// [`OLD_DPC`] sets [`AT_OLD_DPC`] and [`NEW_DPC`] sets [`AT_NEW_DPC`]. So
/// `M5` and the stack pointer say both which `R` and which `DPC` were used.
fn popj_program(old: u32, new: u32) -> Machine {
    let mut p = vec![filler()];
    constant(old, 1, &mut p);
    constant(new, 2, &mut p);
    constant(RETURNED, 6, &mut p);
    constant(AT_OLD_DPC, 7, &mut p);
    constant(AT_NEW_DPC, 8, &mut p);
    p.push(Insn::new(ALU | SETZ | m_dest(5)));
    p.push(Insn::new(DISPATCH | DMEM_WRITE | a_src(1) | d_addr(D)));
    p.push(filler());
    p.push(filler());
    p.push(Insn::new(JUMP | P | ALWAYS | target(SUB as u64)));
    // The call's delay slot runs before the subroutine and sets nothing; the
    // return comes to the instruction after it.
    p.push(filler());
    p.push(copy(6, 5));
    let here = p.len();
    p.push(halt_here(here));
    assert!(p.len() < SUB);
    p.resize(512, filler());
    p[SUB] = Insn::new(DISPATCH | DMEM_WRITE | POPJ | a_src(2) | d_addr(D));
    for (at, from) in [(OLD_DPC as usize, 7), (NEW_DPC as usize, 8)] {
        p[at] = copy(from, 5);
        p[at + 1] = halt_here(at + 1);
    }
    program(p)
}

const POPJ_CYCLES: usize = 240;

/// `(M5, SPCPTR)` on the board, `rtl` and `micro` for `old` then `new`, the
/// new word being in place afterwards on all three.
fn popj_case(old: u32, new: u32) -> [(u32, u8); 3] {
    let t = run3(&popj_program(old, new), &[], POPJ_CYCLES);
    let tail = |t: &[u16]| octal(&t[t.len().saturating_sub(6)..]);
    eprintln!(
        "old {old:6o} new {new:6o}: chip M5 {:o} SPCPTR {} [{}]; rtl M5 {:o} SPCPTR {} [{}]; micro M5 {:o} SPCPTR {}",
        t.chip.mmem[5],
        t.chip.spcptr,
        tail(&ran_pcs(&t.log)),
        t.rtl.mmem[5],
        t.rtl.spcptr,
        tail(&t.rtl_pcs),
        t.micro.mmem[5],
        t.micro.spcptr
    );
    for (name, e) in [("chip", &t.chip), ("rtl", &t.rtl), ("micro", &t.micro)] {
        assert_eq!(e.dmem[D as usize], new, "{name}: the new word is written");
    }
    [
        (t.chip.mmem[5], t.chip.spcptr),
        (t.rtl.mmem[5], t.rtl.spcptr),
        (t.micro.mmem[5], t.micro.spcptr),
    ]
}

/// The four cases and what the board does in each: old and new word, and
/// the board's `(M5, SPCPTR)`. The stack pointer starts at 0 and the call
/// takes it to 1; a pop takes it back.
const POPJ_CASES: [(u32, u32, (u32, u8)); 4] = [
    (DR | OLD_DPC, NEW_DPC, (AT_NEW_DPC, 1)),
    (OLD_DPC, DR | NEW_DPC, (RETURNED, 0)),
    (OLD_DPC, NEW_DPC, (AT_NEW_DPC, 1)),
    (DR | DP | OLD_DPC, DR | DN | NEW_DPC, (RETURNED, 0)),
];

/// **On `chip`, a `DISPATCH` that writes the dispatch memory and has `POPJ`
/// takes `R`, and the `DPC` it goes to when `R` is clear, from the word it
/// is writing** --- the new word, not the one the RAM held when the
/// microcycle began. This is the netlist model's answer and not a fact
/// about the board: it rests on the clock ending the write pulse before the
/// edge ([`on_chip_the_dispatch_ram_floats_during_its_write_and_the_pulse_ends_at_the_edge`]),
/// where the board's pulse runs ten nanoseconds past it and the RAM's
/// output floats while written, a race. **Unverified** on a CADR. QUUX
/// defines the old word ([`on_quux_rtl_and_micro_take_the_old_word`]); on
/// the CADR `rtl` and `micro` follow the netlist
/// ([`on_the_cadr_rtl_and_micro_take_the_word_chip_does`]).
///
/// The dispatch memory is 93425A 1K x 1 RAMs (pages DRAM0-DRAM2), written
/// by `-DWEA`/`-DWEB`, `NAND(WP2, DISPWR)` at DRAM0 2F03: live `DISPWR`, so
/// the pulse is in this instruction's own microcycle, and the address,
/// `DADR`, is this instruction's throughout. The 93425A's output is high
/// impedance while written and shows the cell it addresses after, so by the
/// clock edge that ends the microcycle --- the edge that registers `PC`
/// and steps the stack pointer --- `DR` and `DPC` are the new word's.
///
/// `P` and `N` do nothing here: `DISPENB` is `DISPATCH AND NOT DISPWR`, and
/// the fourth case, `R` set in both and `P`, `N` and `DPC` all different,
/// returns.
#[test]
fn on_chip_a_popj_that_writes_its_own_dispatch_word_uses_the_new_word() {
    for (old, new, want) in POPJ_CASES {
        let [chip, _, _] = popj_case(old, new);
        assert_eq!(chip, want, "chip, old {old:o} new {new:o}");
    }
}

/// `m` as QUUX: the same program and memories on `quux`, its virtual page
/// 0, of 1024 words, mapped onto physical page 0, so that [`VADDR`] is
/// [`PHYS_13`].
fn as_quux(m: &Machine) -> Machine {
    let mut m = m.clone();
    m.geometry = muir::machine::Geometry::QUUX;
    support::prom_program_in_ram(&mut m);
    m.l2_map[0] = (1 << 27) | (1 << 26);
    m
}

/// **On the CADR, `rtl` and `micro` take the word the board does**: the
/// netlist is the reference, including in a race the board itself does not
/// settle ([`on_chip_a_popj_that_writes_its_own_dispatch_word_uses_the_new_word`]).
#[test]
fn on_the_cadr_rtl_and_micro_take_the_word_chip_does() {
    for (old, new, _) in POPJ_CASES {
        let [chip, rtl, micro] = popj_case(old, new);
        assert_eq!(rtl, chip, "rtl, old {old:o} new {new:o}");
        assert_eq!(micro, chip, "micro, old {old:o} new {new:o}");
    }
}

/// **QUUX defines the word a `POPJ` in a dispatch write sees as the old
/// one**: `R` and `DPC` from the word standing before the write, on `rtl`
/// and `micro`, as a RAM read in the cycle that writes it gives on the
/// FPGA. With `R` set in the old word it returns; with it clear it goes to
/// the old word's `DPC` and pops nothing.
#[test]
fn on_quux_rtl_and_micro_take_the_old_word() {
    for (old, new, _) in POPJ_CASES {
        let want = if old & DR != 0 { (RETURNED, 0) } else { (AT_OLD_DPC, 1) };
        let m = as_quux(&popj_program(old, new));
        for (name, (e, _)) in
            [("rtl", rtl(&m, &[], POPJ_CYCLES)), ("micro", micro(&m, &[], POPJ_CYCLES))]
        {
            assert_eq!((e.mmem[5], e.spcptr), want, "{name}, old {old:o} new {new:o}");
            assert_eq!(e.dmem[D as usize], new, "{name}: the new word is written");
        }
    }
}

/// **What the board's answer rests on**, measured on `chip` in the
/// `DISPWR`-with-`POPJ` microcycle: while `-DWEA` is low the dispatch RAM's
/// `DR` output is high impedance (the 93425A's output is off while written),
/// and the pulse ends at the same nanosecond as the clock edge that ends the
/// microcycle, ordered before it. That order is `src/clock.rs`'s: it cuts
/// `TPWP` at the cycle boundary (`WP_OFF_NS` limited to
/// `RESTART_AFTER_READ_NS`) and schedules the end first. On the board the
/// pulse runs to `-TPW70`, ten nanoseconds past `-TPW60`/`-TPDONE` where the
/// next `-TPR0` starts, and the 93425A needs time after `-WE` rises before
/// its output is valid; so on the physical machine the registers at that
/// edge may see the RAM's output still floating. **Unverified**: settled
/// only by gate-level timing (the 93425A's write recovery against the
/// `-TPR0` to register-clock path), which `chip` does not model.
#[test]
fn on_chip_the_dispatch_ram_floats_during_its_write_and_the_pulse_ends_at_the_edge() {
    let m = popj_program(DR | OLD_DPC, NEW_DPC);
    let n = support::netlists();
    let (mut c, mut clk, mut far) = support::chip(&n);
    let image: Vec<u64> = m.prom.iter().map(|&i| muir::prom::programming(i)).collect();
    c.power_on();
    c.load_prom(&n.cpu, &image);
    c.settle();
    far.join(&mut c, clk.time_ns());
    let boot = n.cpu.by_name_id("-BOOT2").unwrap();
    c.set_net(boot, Level::Low);
    c.settle();
    for _ in 0..20 {
        c.tick(&mut clk);
    }
    c.set_net(boot, Level::High);
    let net = |s: &str| n.cpu.by_name_id(s).unwrap();
    let (clk0, dwea, dr) = (net("-CLK0"), net("-DWEA"), net("DR"));
    // (time, -CLK0, -DWEA, DR) at every tick while `IR` holds the
    // subroutine's instruction, which is while `PC` is SUB + 1.
    let mut seen = Vec::new();
    for _ in 0..400_000 {
        far.tick_with(&mut c, &mut clk);
        let pc = c.bus(&n.cpu, "PC", 14) as usize;
        if pc == SUB + 1 {
            seen.push((clk.time_ns(), c.net(clk0), c.net(dwea), c.net(dr)));
        } else if !seen.is_empty() {
            seen.push((clk.time_ns(), c.net(clk0), c.net(dwea), c.net(dr)));
            break;
        }
    }
    let during: Vec<_> = seen.iter().filter(|s| s.2 == Level::Low).collect();
    assert!(!during.is_empty(), "the dispatch write pulse fired");
    assert!(during.iter().all(|s| s.3 == Level::Z), "DR floats while written: {during:?}");
    let end = seen.iter().position(|s| s.2 == Level::Low).unwrap() + during.len();
    let (after, edge) = (seen[end], seen[end + 1]);
    assert_eq!(after.2, Level::High, "the pulse has ended");
    assert_eq!(after.1, Level::High, "before the edge: -CLK0 still high");
    assert_eq!(after.3, Level::Low, "DR shows the new word's R, clear");
    assert_eq!(edge.1, Level::Low, "then the edge");
    assert_eq!(after.0, edge.0, "at the same nanosecond");
}

// --- Q3b: a map write, and the instruction straight after it ---------------

/// `MD` for the map writes: level-1 index 100, level-2 low bits 3. Level 1
/// holds 0 there, so level 2 is addressed at 3. Its low three bits are 2.
const MAP_MD: u32 = (0o100 << 13) | (3 << 8) | 2;
/// Level 2's word 3 before and after: bit 18 clear before and set after,
/// for the dispatch on it.
const OLD_L2: u32 = 0o1234567 & !(1 << 18);
const NEW_L2: u32 = 0o7654321;
/// The store: `VMA<25>`, level 2 only, and the new word.
const MAP_STORE: u32 = (1 << 25) | NEW_L2;

/// QUUX's map, revision 13's (contract G2 appendix A1.7): `MD` for the map
/// writes, `VA<27:15>` 100, whose level-1 entry is 0, and `VA<14:10>` 3, so
/// that level 2 is addressed at 3 in block 0; the level-2 words, `<22>`,
/// the map bit a dispatch's `IR<8>` takes, clear before and set after; and
/// the store, `VMA<28>`, level 2 only, and the new word.
const MAP_MD_13: u32 = (0o100 << 15) | (3 << 10) | 2;
const OLD_L2_13: u32 = OLD_L2;
const NEW_L2_13: u32 = NEW_L2 | 1 << 22;
const MAP_STORE_13: u32 = (1 << 28) | NEW_L2_13;

/// `MD` set to [`MAP_MD`], `VMA-WRITE-MAP` of [`MAP_STORE`], and `then`
/// straight after it; level 2's word 3 holds [`OLD_L2`] to begin with.
fn map_write_then(then: Vec<Insn>) -> Machine {
    assert_eq!(OLD_L2 & (1 << 18), 0);
    assert_ne!(NEW_L2 & (1 << 18), 0);
    map_write_with(MAP_MD, MAP_STORE, OLD_L2, then)
}

/// [`map_write_then`] on QUUX, with [`MAP_MD_13`], [`MAP_STORE_13`] and
/// [`OLD_L2_13`].
fn quux_map_write_then(then: Vec<Insn>) -> Machine {
    assert_eq!(OLD_L2_13 & (1 << 22), 0);
    assert_ne!(NEW_L2_13 & (1 << 22), 0);
    as_quux(&map_write_with(MAP_MD_13, MAP_STORE_13, OLD_L2_13, then))
}

/// `MD` set to `md`, `VMA-WRITE-MAP` of `store`, and `then` straight after
/// it; level 2's word 3 holds `old` to begin with.
fn map_write_with(md: u32, store: u32, old: u32, then: Vec<Insn>) -> Machine {
    let mut p = vec![filler()];
    constant(md, 1, &mut p);
    constant(store, 2, &mut p);
    constant(AT_OLD_DPC, 7, &mut p);
    constant(AT_NEW_DPC, 8, &mut p);
    p.push(Insn::new(ALU | SETZ | m_dest(5)));
    p.push(Insn::new(ALU | SETM | m_src(1) | a_src(3) | MD));
    p.push(filler());
    p.push(filler());
    p.push(Insn::new(ALU | SETM | m_src(2) | a_src(3) | WRITE_MAP));
    p.extend(then);
    let here = p.len();
    p.push(halt_here(here));
    assert!(p.len() < OLD_DPC as usize);
    p.resize(512, filler());
    for (at, from) in [(OLD_DPC as usize, 7), (NEW_DPC as usize, 8)] {
        p[at] = copy(from, 5);
        p[at + 1] = halt_here(at + 1);
    }
    let mut m = program(p);
    m.l2_map[3] = old;
    m
}

/// Reads `MAP(MD)` into `M10` in the instruction after the store and into
/// `M11` in the one after that.
fn map_source_after_a_map_write() -> Three {
    let m = map_write_then(vec![
        Insn::new(ALU | SETM | SRC_MAP | a_src(3) | m_dest(10)),
        Insn::new(ALU | SETM | SRC_MAP | a_src(3) | m_dest(11)),
    ]);
    let t = run3(&m, &[], 240);
    for (name, e) in [("chip", &t.chip), ("rtl", &t.rtl), ("micro", &t.micro)] {
        eprintln!("{name}: M10 {:o} M11 {:o} l2[3] {:o}", e.mmem[10], e.mmem[11], e.l2[3]);
    }
    t
}

/// **On `chip`, the instruction right after a map write reads the new word
/// from `MAP(MD)`** --- the netlist model's answer, resting on the same
/// pulse timing as the dispatch RAM's, and **unverified** on a CADR. `WMAPD` puts the level-2 pulse, `-VM1WPA/B`
/// = `NAND(MAPWR1D, WP1B)` at VCTL2 1D07, in the next microcycle's write
/// phase, and that microcycle is the reader; functional source 11 goes
/// through VMEMDR onto `MF` and the M bus unlatched (MLATCH's 74S373s latch
/// only M memory), so the word the edge registers is the one the RAM shows
/// after the pulse. Same caveat as the dispatch RAM:
/// [`on_chip_the_dispatch_ram_floats_during_its_write_and_the_pulse_ends_at_the_edge`].
#[test]
fn on_chip_the_instruction_after_a_map_write_reads_the_new_map_word() {
    let t = map_source_after_a_map_write();
    assert_eq!(t.chip.l2[3], NEW_L2, "chip: the map is written");
    assert_eq!(
        t.chip.mmem[10] & 0o77777777,
        NEW_L2,
        "chip: the next instruction reads the new word"
    );
    assert_eq!(t.chip.mmem[11], t.chip.mmem[10], "chip: and so does the one after");
}

/// **On the CADR, `rtl` and `micro` read the word the board does.**
#[test]
fn on_the_cadr_rtl_and_micro_read_the_map_word_chip_does_after_a_map_write() {
    let t = map_source_after_a_map_write();
    for (name, e) in [("rtl", &t.rtl), ("micro", &t.micro)] {
        assert_eq!((e.mmem[10], e.mmem[11]), (t.chip.mmem[10], t.chip.mmem[11]), "{name}");
    }
}

/// **QUUX defines it as the old word**: the instruction right after the
/// store reads the level-2 word from before the write, on `rtl` and on
/// `micro`, and the one after that reads the new one, `MAP(MD)<27:0>`
/// (contract G2 appendix A1.7).
#[test]
fn on_quux_rtl_and_micro_read_the_old_map_word_after_a_map_write() {
    let m = quux_map_write_then(vec![
        Insn::new(ALU | SETM | SRC_MAP | a_src(3) | m_dest(10)),
        Insn::new(ALU | SETM | SRC_MAP | a_src(3) | m_dest(11)),
    ]);
    for (name, (e, _)) in [("rtl", rtl(&m, &[], 240)), ("micro", micro(&m, &[], 240))] {
        assert_eq!(e.l2[3], NEW_L2_13, "{name}: the map is written");
        assert_eq!(e.mmem[10] & 0o1777777777, OLD_L2_13, "{name}: the next instruction");
        assert_eq!(e.mmem[11] & 0o1777777777, NEW_L2_13, "{name}: the one after");
    }
}

/// A dispatch on map bit 18 in the instruction right after the store:
/// dispatch words 1300 and 1301 go to [`OLD_DPC`] and [`NEW_DPC`], and
/// `IR<8>` makes `DADR<0>` `VMO18` (the 74S64s at DSPCTL 2F24, 2F05 and
/// 2F23). The old level-2 word has bit 18 clear, the new one set.
fn map_dispatch_after_a_map_write() -> Three {
    const E: u64 = 0o1300;
    let mut m = map_write_then(vec![
        Insn::new(DISPATCH | (1 << 8) | a_src(3) | m_src(3) | d_addr(E)),
        filler(),
    ]);
    m.dmem[E as usize] = OLD_DPC;
    m.dmem[E as usize + 1] = NEW_DPC;
    let t = run3(&m, &[], 240);
    for (name, e) in [("chip", &t.chip), ("rtl", &t.rtl), ("micro", &t.micro)] {
        eprintln!("{name}: M5 {:o}, l2[3] {:o}", e.mmem[5], e.l2[3]);
    }
    t
}

/// **On `chip` the dispatch takes the new word's bit 18**, the netlist
/// model's answer (see above).
#[test]
fn on_chip_a_dispatch_on_a_map_bit_after_a_map_write_takes_the_new_word() {
    let t = map_dispatch_after_a_map_write();
    assert_eq!(t.chip.l2[3], NEW_L2, "chip: the map is written");
    assert_eq!(t.chip.mmem[5], AT_NEW_DPC, "chip: dispatched on the new word's bit 18");
}

/// **On the CADR, `rtl` and `micro` dispatch on the bit the board does.**
#[test]
fn on_the_cadr_rtl_and_micro_dispatch_on_the_map_bit_chip_does() {
    let t = map_dispatch_after_a_map_write();
    assert_eq!((t.rtl.mmem[5], t.micro.mmem[5]), (t.chip.mmem[5], t.chip.mmem[5]));
}

/// **QUUX defines it as the old word's map bit**, `<22>` on revision 13
/// (contract G2 appendix A1.4), on `rtl` and `micro`.
#[test]
fn on_quux_rtl_and_micro_dispatch_on_the_old_map_bit() {
    const E: u64 = 0o1300;
    let mut m = quux_map_write_then(vec![
        Insn::new(DISPATCH | (1 << 8) | a_src(3) | m_src(3) | d_addr(E)),
        filler(),
    ]);
    m.dmem[E as usize] = OLD_DPC;
    m.dmem[E as usize + 1] = NEW_DPC;
    let (r, _) = rtl(&m, &[], 240);
    let (u, _) = micro(&m, &[], 240);
    assert_eq!((r.mmem[5], u.mmem[5]), (AT_OLD_DPC, AT_OLD_DPC));
}

// --- Q4: writes pending across a held microcycle ----------------------------

/// Virtual word `(1 << 8) | 5`: level-2 entry 1, physical page 100. On
/// QUUX ([`as_quux`]) in virtual page 0 onto physical page 0, at
/// [`PHYS_13`].
const VADDR: u32 = (1 << 8) | 5;
const PHYS: u32 = (0o100 << 8) | 5;
const PHYS_13: u32 = VADDR;
/// The word the read brings into `MD`: level-1 index 100 and level-2 low
/// bits 7 for a map write, low three bits 5 for a dispatch.
const READ_WORD: u32 = (0o100 << 13) | (7 << 8) | 5;
/// `MD` before the read lands: level-2 low bits 3, low three bits 2.
const MD_BEFORE: u32 = MAP_MD;
const HELD_CYCLES: usize = 400;
/// Functional destinations 20 (`VMA`), 14 (PDL pointer), 10 (PDL at the
/// pointer), 15 (SPC push), each with M 37.
const PDL_POINTER: u64 = (0o14 << 19) | (0o37 << 14);
const PDL_TOP: u64 = (0o10 << 19) | (0o37 << 14);
const SPC_PUSH: u64 = (0o15 << 19) | (0o37 << 14);

/// `M1` = [`MD_BEFORE`] into `MD`, `M12` = [`VADDR`], the `(value,
/// register)` constants of `setup`, the PDL pointer set to 20, then
/// `VMA-START-READ` of [`VADDR`] and `then`, fillers and a jump to itself.
/// [`PHYS`] holds [`READ_WORD`].
fn read_then(setup: &[(u32, u64)], then: Vec<Insn>) -> Machine {
    read_then_with(MD_BEFORE, setup, then)
}

/// [`read_then`] with `md` into `MD` before the read.
fn read_then_with(md: u32, setup: &[(u32, u64)], then: Vec<Insn>) -> Machine {
    let mut p = vec![filler()];
    constant(md, 1, &mut p);
    constant(VADDR, 12, &mut p);
    constant(0o20, 14, &mut p);
    for &(v, r) in setup {
        constant(v, r, &mut p);
    }
    p.push(Insn::new(ALU | SETM | m_src(1) | a_src(3) | MD));
    p.push(Insn::new(ALU | SETM | m_src(14) | a_src(3) | PDL_POINTER));
    p.push(filler());
    p.push(Insn::new(ALU | SETM | m_src(12) | a_src(3) | START_READ));
    p.extend(then);
    for _ in 0..4 {
        p.push(filler());
    }
    let here = p.len();
    p.push(halt_here(here));
    let mut m = program(p);
    m.l2_map[1] = (1 << 23) | (1 << 22) | 0o100;
    m
}

/// The board's generator cycles in which the cpu clock did not run
/// (`-WAIT`), the write pulses fired in them, and the generator cycles
/// stretched past 220 ns (`-HANG`, which stops the generator itself).
fn holds(log: &[Cycle]) -> (usize, [u32; 5], usize) {
    let mut sum = [0; 5];
    let mut k = 0;
    for c in log.iter().filter(|c| !c.ran) {
        k += 1;
        for (s, p) in sum.iter_mut().zip(c.pulses) {
            *s += p;
        }
    }
    (k, sum, log.iter().filter(|c| c.ns > 220).count())
}

/// **No write pulse fires in a generator cycle `-WAIT` holds, and a late
/// write pending across one lands once, alike on all three.** `TPWP` is
/// `NOR(latch, -MACHRUNA)` at CLOCK2 1C10, `-MACHRUNA` being `NOT MACHRUN`
/// at 1C10 too, and `TPWPIRAM` is gated the same way: so `-WP1..-WP5`, the
/// 7428s at 1C02 and 1C11, are all gated by `MACHRUN`, the 9S42-1 at OLORD1
/// 1A15 that takes `-WAIT`. The generator runs on through a wait; the pulses
/// do not.
///
/// The program: a PDL write (destination 10, at the pointer, 20) or an SPC
/// push (destination 15) right after a `VMA-START-READ`, then a `VMA` store,
/// which `-WAIT` holds (`DESTMEM AND MBUSY.SYNC`) while the read lands in
/// `MD`, then `MD` into `M13`. The PDL or SPC write is pending --- its pulse
/// is in the microcycle after its own --- across the held cycles. Its
/// address is a register and its data `L`, which the stopped cpu clock
/// holds, so even a repeated pulse would land on the same word; what the
/// test holds is that none fired and the engines agree.
///
/// A **map** write cannot be pending across a `-WAIT` on the CADR: the
/// store is `DESTMEM` and waits for `MBUSY.SYNC` itself, and nothing but a
/// memory cycle raises `MBUSY` again before the next instruction. The
/// store's own wait is reached here instead
/// ([`a_map_store_held_by_wait_writes_at_the_md_it_ends_with`]).
#[test]
fn pdl_and_spc_writes_pending_across_a_wait_land_once_alike() {
    for (what, dest) in [("PDL", PDL_TOP), ("SPC", SPC_PUSH)] {
        let m = read_then(
            &[(0o765432, 4)],
            vec![
                Insn::new(ALU | SETM | m_src(4) | a_src(3) | dest),
                Insn::new(ALU | SETM | m_src(12) | a_src(3) | VMA),
                Insn::new(ALU | SETM | SRC_MD | a_src(3) | m_dest(13)),
            ],
        );
        let t = run3(&m, &[(PHYS, READ_WORD)], HELD_CYCLES);
        let (k, pulses, _) = holds(&t.log);
        eprintln!("{what}: chip {k} cycles held by -WAIT, write pulses in them {pulses:?}");
        for (name, e) in [("chip", &t.chip), ("rtl", &t.rtl), ("micro", &t.micro)] {
            eprintln!(
                "  {name}: M13 {:o} pdl[20] {:o} spcptr {} spc[1] {:o}",
                e.mmem[13], e.pdl[0o20], e.spcptr, e.spc[1]
            );
        }
        assert!(k > 0, "{what}: chip: the VMA store was held by -WAIT");
        assert_eq!(pulses, [0; 5], "{what}: chip: no write pulse in a held cycle");
        assert_eq!(t.chip.mmem[13], READ_WORD, "{what}: chip: the read landed");
        let (pdl, spc) = if what == "PDL" { (0o765432, 0) } else { (0, 0o765432) };
        assert_eq!((t.chip.pdl[0o20], t.chip.spc[1]), (pdl, spc), "{what}: chip");
        for (name, e) in [("rtl", &t.rtl), ("micro", &t.micro)] {
            assert_eq!(
                (e.mmem[13], e.pdl[0o20], e.spcptr, e.spc.clone()),
                (t.chip.mmem[13], t.chip.pdl[0o20], t.chip.spcptr, t.chip.spc.clone()),
                "{what}: {name}"
            );
        }
    }
}

/// **A map store held by `-WAIT` while the read lands writes at the `MD` it
/// ends with**, alike on all three. The store comes one instruction after
/// the `VMA-START-READ`, `-WAIT` holds it, `MD` goes from [`MD_BEFORE`]
/// (level-2 index 3) to [`READ_WORD`] (7) meanwhile, and the level-2 write
/// in the microcycle after lands at 7, once.
#[test]
fn a_map_store_held_by_wait_writes_at_the_md_it_ends_with() {
    let m = read_then(
        &[(MAP_STORE, 2)],
        vec![
            filler(),
            Insn::new(ALU | SETM | m_src(2) | a_src(3) | WRITE_MAP),
            Insn::new(ALU | SETM | SRC_MD | a_src(3) | m_dest(13)),
        ],
    );
    let t = run3(&m, &[(PHYS, READ_WORD)], HELD_CYCLES);
    let (k, pulses, _) = holds(&t.log);
    eprintln!("chip: {k} cycles held by -WAIT, write pulses in them {pulses:?}");
    assert!(k > 0, "chip: the store was held by -WAIT");
    assert_eq!(pulses, [0; 5], "chip: no write pulse in a held cycle");
    assert_eq!(t.chip.mmem[13], READ_WORD, "chip: the read landed");
    assert_eq!((t.chip.l2[3], t.chip.l2[7]), (0, NEW_L2), "chip: written at the new MD, once");
    for (name, e) in [("rtl", &t.rtl), ("micro", &t.micro)] {
        assert_eq!((e.mmem[13], e.l2.clone()), (t.chip.mmem[13], t.chip.l2.clone()), "{name}");
    }
}

/// A map store straight after the `VMA-START-READ` (not held: `MBUSY.SYNC`
/// is not yet up), then an instruction using `MD`, which `-HANG` holds.
/// The store's pending level-2 write is in that hung microcycle. The store
/// also rewrites `VMA` under the read in flight, and the read brings back
/// 0 on the board and `rtl` alike.
fn map_write_pending_into_a_hang() -> Three {
    let m = read_then(
        &[(MAP_STORE, 2)],
        vec![
            Insn::new(ALU | SETM | m_src(2) | a_src(3) | WRITE_MAP),
            Insn::new(ALU | SETM | SRC_MD | a_src(3) | m_dest(13)),
        ],
    );
    let t = run3(&m, &[(PHYS, READ_WORD)], HELD_CYCLES);
    let (k, pulses, hung) = holds(&t.log);
    eprintln!("chip: {k} cycles held by -WAIT, pulses {pulses:?}; {hung} hung");
    for (name, e) in [("chip", &t.chip), ("rtl", &t.rtl), ("micro", &t.micro)] {
        let nz: Vec<_> =
            e.l2.iter()
                .enumerate()
                .filter(|&(_, &w)| w != 0)
                .map(|(k, &w)| format!("{k:o}:{w:o}"))
                .collect();
        eprintln!("  {name}: M13 {:o} l2 {nz:?}", e.mmem[13]);
    }
    t
}

/// **`-HANG` is not `-WAIT`: a hung microcycle's write pulses fire before
/// the hold, at `MD` as it was.** `-TPR0` is `NAND(-HANG, -CLOCK RESET B,
/// CYCLECOMPLETED)` at CLOCK1 1C08, so `-HANG` holds off the *next*
/// cycle's start; `CYCLECOMPLETED` is set by `-TPDONE` (`-TPW60`), after the
/// write pulse has begun at `-TPW30`. So the hung microcycle's read phase
/// and write pulses run, then the generator stops until `-RDFINISH`, and
/// the edge that ends the cycle registers the new `MD`. On the board the
/// pending map write lands at level-2 index 3, [`MD_BEFORE`]'s; `M13` gets
/// the read's word, 0.
#[test]
fn a_map_write_pending_into_a_hang_lands_at_the_md_before_the_hang_on_the_board() {
    let t = map_write_pending_into_a_hang();
    let (_, pulses, hung) = holds(&t.log);
    assert_eq!(pulses, [0; 5], "chip: nothing held by -WAIT fired");
    assert!(hung > 0, "chip: a microcycle was hung");
    assert_eq!(t.chip.mmem[13], 0, "chip: what the read brought back");
    assert_eq!((t.chip.l2[0], t.chip.l2[3]), (0, NEW_L2), "chip: written at MD before the hang");
}

/// `rtl` resolves a `Stall::Hang` before running the microcycle at all
/// (`Rtl::step`: `stall_for`, then `read_phase` again, then `write_phase`),
/// which wrote the pending map write at `MD` after the read had landed.
/// It now fires the hung cycle's write pulse as the cycle ends, as the
/// board does, and agrees.
#[test]
fn rtl_lands_a_map_write_pending_into_a_hang_as_the_board_does() {
    let t = map_write_pending_into_a_hang();
    assert_eq!((t.rtl.mmem[13], t.rtl.l2.clone()), (t.chip.mmem[13], t.chip.l2.clone()), "rtl");
}

/// Dispatch word 1300 plus `MD<2:0>` written with [`DISPATCH_WORD`] by a
/// `DISPWR` whose `M` source is `MD` (three bits, rotate 0), `gap` fillers
/// after a `VMA-START-READ`.
const DISPATCH_WORD: u32 = 0o123456;
fn dispatch_write_on_md(gap: usize) -> Three {
    const E: u64 = 0o1300;
    let mut then = vec![filler(); gap];
    then.push(Insn::new(DISPATCH | DMEM_WRITE | SRC_MD | d_len(3) | a_src(2) | d_addr(E)));
    let m = read_then(&[(DISPATCH_WORD, 2)], then);
    let t = run3(&m, &[(PHYS, READ_WORD)], HELD_CYCLES);
    let (k, pulses, hung) = holds(&t.log);
    eprintln!("gap {gap}: chip {k} cycles held by -WAIT, pulses {pulses:?}; {hung} hung");
    for (name, e) in [("chip", &t.chip), ("rtl", &t.rtl), ("micro", &t.micro)] {
        let w: Vec<_> = (0..8).map(|k| format!("{:o}", e.dmem[E as usize + k])).collect();
        eprintln!("  {name}: dmem 1300..1307 {w:?}");
    }
    t
}

/// Where each gap's write lands on the board: 1302 is `MD<2:0>` = 2,
/// [`MD_BEFORE`]; 1305 is 5, [`READ_WORD`]. Gap 0 is not held at all and
/// reads the old `MD`. Gaps 1 and 2 are hung, and the pulse fires before
/// the hang ends, at the old `MD`. Gap 3 is hung too, for less time, and
/// writes at the new `MD`: the word is in `MD` before its pulse, though
/// `READ IN PROGRESS` is still up. `rtl` agrees on gaps 0 and 3.
const DISPATCH_GAPS: [(usize, usize); 4] = [(0, 0o1302), (1, 0o1302), (2, 0o1302), (3, 0o1305)];

/// **A dispatch write addressed by `MD` in a hung microcycle writes at `MD`
/// as it was before the hang, on the board** --- the same order as
/// [`a_map_write_pending_into_a_hang_lands_at_the_md_before_the_hang_on_the_board`],
/// for a write that is this instruction's own.
#[test]
fn a_dispatch_write_addressed_by_md_in_a_hang_lands_at_the_md_before_it_on_the_board() {
    for (gap, at) in DISPATCH_GAPS {
        let t = dispatch_write_on_md(gap);
        let (_, pulses, _) = holds(&t.log);
        assert_eq!(pulses, [0; 5], "gap {gap}: chip: nothing held by -WAIT fired");
        let written: Vec<_> = (0..2048).filter(|&k| t.chip.dmem[k] != 0).collect();
        assert_eq!(written, vec![at], "gap {gap}: chip");
        assert_eq!(t.chip.dmem[at], DISPATCH_WORD, "gap {gap}: chip");
    }
}

/// `rtl` fires a hung cycle's write pulse as the cycle ends, with `MD` as
/// the bus has left it (`Rtl::step`), and agrees at every gap.
#[test]
fn rtl_lands_a_dispatch_write_in_a_hang_as_the_board_does() {
    for (gap, _) in DISPATCH_GAPS {
        let t = dispatch_write_on_md(gap);
        assert_eq!(t.rtl.dmem, t.chip.dmem, "gap {gap}: rtl");
    }
}

/// **`micro` writes a dispatch word where the dispatch would read**,
/// the `M` source's bits in the address as `DADR` has them. It has no bus
/// timing --- a read lands at once and nothing hangs --- so it matches the
/// board where nothing is hung, gap 0, and writes at the word read after.
#[test]
fn micro_writes_a_dispatch_word_at_the_address_the_dispatch_reads() {
    for (gap, _) in DISPATCH_GAPS {
        let t = dispatch_write_on_md(gap);
        assert_eq!(t.micro.dmem[0o1300], 0, "gap {gap}: not at the field alone");
        if gap == 0 {
            assert_eq!(t.micro.dmem, t.chip.dmem, "gap {gap}: micro");
        }
    }
}

// --- QUUX: no hung microcycle ------------------------------------------------
//
// QUUX has no `-HANG`. A microcycle that reads `MD` while a read is in
// flight does not run its read phase and write pulses and then hold: it
// waits, as for `-WAIT`, whole microcycles with no write pulse, and runs
// once the word is in `MD` ([`muir::machine::Geometry::hangs`]). `micro`,
// where a read lands at once, has always run it so; on QUUX `rtl` agrees
// with it.

/// The QUUX run of [`dispatch_write_on_md`]'s program on `rtl`, under
/// `timing`, and on `micro`.
fn quux_dispatch_write_on_md(gap: usize, timing: TimingModel) -> (End, Vec<Row>, End) {
    const E: u64 = 0o1300;
    let mut then = vec![filler(); gap];
    then.push(Insn::new(DISPATCH | DMEM_WRITE | SRC_MD | d_len(3) | a_src(2) | d_addr(E)));
    let m = as_quux(&read_then(&[(DISPATCH_WORD, 2)], then));
    let main = [(PHYS_13, READ_WORD)];
    let (r, rows) = rtl_timed(&m, &main, HELD_CYCLES, timing, None);
    let (u, _) = micro(&m, &main, HELD_CYCLES);
    (r, rows, u)
}

/// **On QUUX a dispatch write addressed by `MD` waits for the word read**
/// and writes where `micro` does, at every gap: 1302 at gap 0, which
/// nothing holds, and 1305, [`READ_WORD`]'s, where the CADR's board hangs
/// and writes at the old `MD`
/// ([`a_dispatch_write_addressed_by_md_in_a_hang_lands_at_the_md_before_it_on_the_board`]).
#[test]
fn on_quux_a_dispatch_write_addressed_by_md_waits_for_the_word_read() {
    for (gap, want) in [(0, 0o1302), (1, 0o1305), (2, 0o1305), (3, 0o1305)] {
        for timing in [
            TimingModel::Sync { cycle_ticks: 4, ilong_ticks: 0 },
            TimingModel::Sync { cycle_ticks: 3, ilong_ticks: 0 },
        ] {
            let (r, _, u) = quux_dispatch_write_on_md(gap, timing);
            let written: Vec<_> = (0..2048).filter(|&k| r.dmem[k] != 0).collect();
            assert_eq!(written, vec![want], "gap {gap}, {timing:?}: rtl");
            assert_eq!(r.dmem, u.dmem, "gap {gap}, {timing:?}: rtl and micro");
        }
    }
}

/// **On QUUX the wait is whole microcycles**: under `sync`, every
/// microcycle of the run, the held one included, is a whole number of
/// four-tick microcycles, and the held one is longer than one.
#[test]
fn on_quux_the_wait_for_md_is_whole_microcycles() {
    let sync = TimingModel::Sync { cycle_ticks: 4, ilong_ticks: 0 };
    let (_, rows, _) = quux_dispatch_write_on_md(1, sync);
    let x = Insn::new(DISPATCH | DMEM_WRITE | SRC_MD | d_len(3) | a_src(2) | d_addr(0o1300)).raw();
    let r = row_of(&rows, x);
    assert!(r.to - r.from > 40, "held: {}..{}", r.from, r.to);
    for w in &rows {
        assert_eq!((w.to - w.from) % 40, 0, "{}..{}", w.from, w.to);
    }
}

/// **On QUUX a map write pending into the wait lands at the word read**:
/// the store's pulse is in the microcycle that runs, once the word is in
/// `MD`, where the CADR's hung microcycle fires it at the old `MD`, level-2
/// index 3 ([`a_map_write_pending_into_a_hang_lands_at_the_md_before_the_hang_on_the_board`]).
/// The store rewrites `VMA` under the read in flight, and the read brings
/// back 0 as on the board, so the word read addresses level-2 index 0, and
/// the old `MD`, [`MAP_MD_13`], index 3 (contract G2 appendix A1.7).
/// (`micro` lands a read at once, before the store, and is not compared.)
#[test]
fn on_quux_a_map_write_pending_into_the_wait_lands_at_the_word_read() {
    let m = as_quux(&read_then_with(
        MAP_MD_13,
        &[(MAP_STORE_13, 2)],
        vec![
            Insn::new(ALU | SETM | m_src(2) | a_src(3) | WRITE_MAP),
            Insn::new(ALU | SETM | SRC_MD | a_src(3) | m_dest(13)),
        ],
    ));
    let (r, _) = rtl(&m, &[(PHYS_13, READ_WORD)], HELD_CYCLES);
    assert_eq!(r.mmem[13], 0, "rtl: what the read brought back");
    assert_eq!((r.l2[0], r.l2[3]), (NEW_L2_13, 0), "rtl: at the word read, not the old MD");
}

// --- Q5: a read finishing inside the microcycle that waits for it, and edges
//     that an acknowledgement lands on ---------------------------------------
//
// muir-fpga's fabric parts from `rtl` in these programs, which its sessions
// named `x-popj-*`, `t-*`, `w-*` and `n-*`; the builders below are theirs,
// the speed set by the program itself where the board has to run it at
// normal speed. The board is watched net by net, each transition with its
// nanosecond, and each microcycle's boundary (`-CLK0` falling) with the `PC`,
// `IR` and `MD` after it.

/// A net's transition on the board.
#[derive(Debug, Clone, Copy)]
struct Event {
    ns: u64,
    net: &'static str,
    level: Level,
}

/// A boundary of the cpu clock, `-CLK0` falling, and what stands after it.
#[derive(Debug, Clone, Copy)]
struct Edge {
    ns: u64,
    ir: u64,
    md: u32,
}

/// The nets watched: the memory cycle's end on VCTL1 (`-MEMACK`, the TD50
/// and TD250 taps `-MFINISHD` and `-RDFINISH`, `MBUSY`, `RD.IN.PROGRESS`),
/// the two holds, `MBUSY.SYNC` and its clock `MCLK1A` (the 74S175 at VCTL1
/// 1E20), `MD`'s strobe `-LOADMD`, and the dispatch RAM's write pulse
/// `-DWEA`.
const WATCH: [&str; 11] = [
    "-MEMACK",
    "-MFINISHD",
    "-RDFINISH",
    "-HANG",
    "-WAIT",
    "MBUSY",
    "MBUSY.SYNC",
    "RD.IN.PROGRESS",
    "-LOADMD",
    "-DWEA",
    "MCLK1A",
];

/// The board run for `edges` microcycles, every transition of [`WATCH`]
/// and every boundary recorded.
fn watch(b: &mut Board, edges: usize) -> (Vec<Edge>, Vec<Event>) {
    let clk0 = b.n.cpu.by_name_id("-CLK0").unwrap();
    let ids: Vec<_> = WATCH.iter().map(|s| b.n.cpu.by_name_id(s).unwrap()).collect();
    let mut was: Vec<Level> = ids.iter().map(|&i| b.c.net(i)).collect();
    let mut clk_was = b.c.net(clk0);
    let (mut es, mut ev) = (Vec::new(), Vec::new());
    while es.len() < edges {
        b.far.tick_with(&mut b.c, &mut b.clk);
        let now = b.clk.time_ns();
        for (k, &i) in ids.iter().enumerate() {
            let l = b.c.net(i);
            if l != was[k] {
                ev.push(Event { ns: now, net: WATCH[k], level: l });
                was[k] = l;
            }
        }
        let l = b.c.net(clk0);
        if l == Level::Low && clk_was != Level::Low {
            es.push(Edge {
                ns: now,
                ir: b.c.bus(&b.n.cpu, "IR", 48),
                md: !(b.c.bus(&b.n.cpu, "-MD", 32) as u32),
            });
        }
        clk_was = l;
    }
    (es, ev)
}

/// The first transition of `net` to `level` at or after `from`.
fn first(ev: &[Event], net: &str, level: Level, from: u64) -> Option<u64> {
    ev.iter().find(|e| e.net == net && e.level == level && e.ns >= from).map(|e| e.ns)
}

/// The microcycle in which `ir` stands in `IR`: its first and last
/// nanosecond on the board.
fn microcycle_of(es: &[Edge], ir: u64) -> (u64, u64) {
    let k = es.iter().position(|e| e.ir == ir).expect("the instruction ran on the board");
    (es[k].ns, es[k + 1].ns)
}

/// One microcycle on `rtl`: from and to, in its own nanoseconds, and what
/// stood in `IR`.
#[derive(Debug, Clone, Copy)]
struct Row {
    from: u64,
    to: u64,
    ir: u64,
}

/// `rtl` under `timing`, `steps` microcycles. With `speed`, the mode
/// register's speed bits are set after the boot, as muir-fpga's generator
/// sets them; without, the program sets them itself.
fn rtl_timed(
    m: &Machine,
    main: &[(u32, u32)],
    steps: usize,
    timing: TimingModel,
    speed: Option<u16>,
) -> (End, Vec<Row>) {
    let mut m = m.clone();
    for &(p, w) in main {
        m.main[p as usize] = u64::from(w);
    }
    let mut e = muir::rtl::Rtl::new(m);
    e.set_timing_model(timing);
    e.boot();
    if let Some(s) = speed {
        e.machine_mut().mode.write(s);
    }
    let mut rows = Vec::new();
    for _ in 0..steps {
        let (from, ir) = (e.ns(), e.ir());
        e.step().unwrap();
        rows.push(Row { from, to: e.ns(), ir });
    }
    let mm = e.machine();
    let end = End {
        mmem: mm.mmem.iter().map(|&w| support::low(w)).collect(),
        dmem: mm.dmem[..mm.geometry.dmem_words()].iter().map(|&w| w & 0o377777).collect(),
        l2: mm.l2_map[..1024].iter().map(|&w| w & 0o77777777).collect(),
        pdl: mm.pdl[..1024].iter().map(|&w| support::low(w)).collect(),
        spc: mm.spc.iter().map(|&w| w & 0o1777777).collect(),
        spcptr: mm.spcptr,
    };
    (end, rows)
}

/// The row in which `ir` stands in `IR`.
fn row_of(rows: &[Row], ir: u64) -> Row {
    *rows.iter().find(|r| r.ir == ir).expect("the instruction ran on rtl")
}

use muir::clock::TimingModel;

/// `IR<45>`, `ILONG`.
fn ilong(i: Insn) -> Insn {
    Insn::new(i.raw() | (1 << 45))
}

/// Unibus 766012, the mode register, is physical 17773005: reached from
/// VMA 1005 through level-2 entry 2, as the boot PROM reaches it
/// (`promh.9`).
const MODE_VADDR: u32 = 0o1005;
const MODE_PAGE: u32 = (1 << 23) | (1 << 22) | 0o37766;
/// `{SPEED1, SPEED0}` = 2, normal: 145 ns a microcycle on the board, 185
/// under `ILONG`.
const NORMAL: u16 = 2;

/// The processor writes `speed` into the mode register's speed bits and
/// runs on, past the two stages of the synchronizer at OLORD1 1A01.
fn speed_prologue(speed: u16, p: &mut Vec<Insn>) {
    constant(MODE_VADDR, 20, p);
    constant(speed as u32, 21, p);
    p.push(Insn::new(ALU | SETM | m_src(21) | a_src(3) | MD));
    p.push(filler());
    p.push(Insn::new(ALU | SETM | m_src(20) | a_src(3) | START_WRITE));
    for _ in 0..12 {
        p.push(filler());
    }
}

/// Where the `POPJ` returns, and the two dispatch words' `DPC`: past the
/// program, which the speed prologue lengthens.
const H_OLD_DPC: u32 = 0o700;
const H_NEW_DPC: u32 = 0o720;
const RET_PC: u32 = 0o740;
/// The dispatch words written and read, 1300 plus `MD<2:0>`.
const E: u64 = 0o1300;

/// `DISPATCH | DMEM_WRITE | POPJ` with its address from `MD<2:0>`, writing
/// `DR | 1234` from A 2; under `ILONG` if `long`.
fn hung_popj_insn(long: bool) -> Insn {
    let x = Insn::new(DISPATCH | DMEM_WRITE | POPJ | SRC_MD | d_len(3) | a_src(2) | d_addr(E));
    if long { ilong(x) } else { x }
}

/// muir-fpga's `hung_popj(pre, n, i, xl)`: a return address pushed, `MD` set
/// to [`MD_BEFORE`] (`MD<2:0>` = 2), `pre` fillers, then `VMA-START-READ` of
/// [`VADDR`], whose word is 0; `n` fillers, the first `i` under `ILONG`;
/// then [`hung_popj_insn`], which uses `MD` and so is hung while the read
/// is in progress.
///
/// Word 1300 holds [`H_OLD_DPC`] with `DR` clear and 1301 [`H_NEW_DPC`].
/// With the read landed before the write pulse, the write goes to 1300,
/// the word the dispatch also reads. If the edge takes the new word, `DR`
/// is set, the `POPJ` returns to [`RET_PC`], `M5` = [`AT_NEW_DPC`] and the
/// stack pointer is back at 0. If it takes the old one, the machine goes to
/// its `DPC`, `M5` = [`AT_OLD_DPC`] and the pointer stays at 1.
///
/// `own` has the program set normal speed itself, which the board needs;
/// without, the speed is left to the caller, as muir-fpga's generator sets
/// it into `rtl`'s mode register after the boot.
fn hung_popj(pre: usize, n: usize, i: usize, xl: bool, own: bool) -> Machine {
    let mut p = vec![filler()];
    if own {
        speed_prologue(NORMAL, &mut p);
    }
    constant(MD_BEFORE, 1, &mut p);
    constant(VADDR, 12, &mut p);
    constant(AT_OLD_DPC, 7, &mut p);
    constant(AT_NEW_DPC, 8, &mut p);
    constant(DR | 0o1234, 2, &mut p);
    constant(RET_PC, 15, &mut p);
    p.push(Insn::new(ALU | SETM | m_src(15) | a_src(3) | SPC_PUSH));
    p.push(Insn::new(ALU | SETZ | m_dest(5)));
    for _ in 0..pre {
        p.push(filler());
    }
    p.push(Insn::new(ALU | SETM | m_src(1) | a_src(3) | MD));
    p.push(filler());
    p.push(Insn::new(ALU | SETM | m_src(12) | a_src(3) | START_READ));
    for k in 0..n {
        p.push(if k < i { ilong(filler()) } else { filler() });
    }
    p.push(hung_popj_insn(xl));
    p.push(filler());
    p.push(copy(7, 5));
    let here = p.len();
    p.push(halt_here(here));
    assert!(p.len() < H_OLD_DPC as usize);
    p.resize(512, filler());
    for (at, from) in [(RET_PC, 8), (H_NEW_DPC, 8), (H_OLD_DPC, 7)] {
        p[at as usize] = copy(from, 5);
        p[at as usize + 1] = halt_here(at as usize + 1);
    }
    let mut m = program(p);
    m.l2_map[1] = (1 << 23) | (1 << 22) | 0o100;
    m.l2_map[2] = MODE_PAGE;
    m.dmem[E as usize] = H_OLD_DPC;
    m.dmem[E as usize + 1] = H_NEW_DPC;
    m
}

const X_CYCLES: usize = 700;

/// Three shapes whose read finishes inside the microcycle of
/// [`hung_popj_insn`] on the board, `(pre, n, i, xl)`, each with what
/// `-HANG` does: `(2, 6, 0, 0)`, `-RDFINISH` 50 ns in and `-HANG` never
/// asserted; `(2, 5, 3, 0)`, 116 ns in, `-HANG` asserted at the read phase's
/// end and released 31 ns later; `(5, 5, 1, 1)`, under `ILONG`, 178 ns into
/// 185, `-HANG` asserted 53 ns before its release.
const INSIDE: [((usize, usize, usize, bool), bool); 3] =
    [((2, 6, 0, false), false), ((2, 5, 3, false), true), ((5, 5, 1, true), true)];

/// **A `POPJ` in a dispatch write whose read finishes inside its own hung
/// microcycle takes the new word, on the board and on `rtl`.** The hold
/// does not lengthen the microcycle: `-HANG`, `NAND(RD.IN.PROGRESS, USE.MD,
/// -CLK3G)` at VCTL1 3F17, can assert only once `-CLK3G` is up at the end of
/// the read phase (measured: 85 ns into a 145 ns microcycle, 125 into a 185
/// one), and what it holds off is the next `-TPR0` (CLOCK1 1C08); the read
/// finishes before `CYCLECOMPLETED`, so the edge comes at the normal time.
/// `MD` has the word read since `-LOADMD` (the acknowledgement), before the
/// microcycle began, so `DADR` is 1300 throughout, and the dispatch write
/// pulse `-DWEA` (`NAND(WP2, DISPWR)`, DRAM0 2F03) ends at the edge.
///
/// So this is, on the board, the microcycle of
/// [`on_chip_a_popj_that_writes_its_own_dispatch_word_uses_the_new_word`]:
/// a pulse that ends at the edge, which `chip` orders before it. The answer
/// is the netlist model's and **unverified** on a CADR for the same reason
/// as there. On the CADR `rtl` takes the word `chip` does, hung or not.
/// QUUX has no hung microcycle and defines the old word
/// ([`on_quux_a_popj_dispatch_write_takes_the_old_word_waiting_or_not`]).
#[test]
fn a_popj_dispatch_write_whose_read_finishes_inside_its_hung_microcycle_takes_the_new_word() {
    for ((pre, n, i, xl), hangs) in INSIDE {
        let m = hung_popj(pre, n, i, xl, true);
        let x = hung_popj_insn(xl).raw();
        let mut b = board(&m, &[]);
        let (es, ev) = watch(&mut b, X_CYCLES);
        let chip = board_end(&b);
        let (s, t) = microcycle_of(&es, x);
        let rdfinish = first(&ev, "-RDFINISH", Level::Low, s).unwrap();
        let hang = first(&ev, "-HANG", Level::Low, s).filter(|&h| h < t);
        let dwea = first(&ev, "-DWEA", Level::High, s).unwrap();
        let what = format!(
            "({pre}, {n}, {i}, {xl}): microcycle {s}..{t}, -RDFINISH {rdfinish}, -HANG {hang:?}"
        );
        eprintln!("{what}; chip M5 {:o} SPCPTR {}", chip.mmem[5], chip.spcptr);
        assert_eq!(t - s, if xl { 185 } else { 145 }, "{what}: not lengthened");
        assert!(s < rdfinish && rdfinish < t, "{what}: the read finishes inside");
        assert_eq!(hang.is_some(), hangs, "{what}: -HANG");
        assert_eq!(dwea, t, "{what}: the write pulse ends at the edge");
        assert_eq!((chip.mmem[5], chip.spcptr), (AT_NEW_DPC, 0), "{what}: chip");
        let (rtl, rows) = rtl_timed(&m, &[], X_CYCLES, TimingModel::Cadr, None);
        let r = row_of(&rows, x);
        assert_eq!((r.from, r.to), (s, t), "{what}: rtl's microcycle");
        assert_eq!((rtl.mmem[5], rtl.spcptr), (AT_NEW_DPC, 0), "{what}: rtl");
    }
}

/// A [`hung_popj`] program on QUUX whose read brings back a word with the
/// same low three bits as [`MD_BEFORE`], so that the dispatch writes and
/// reads one word, `DADR` being the same before the read lands and after;
/// that word holds [`H_OLD_DPC`] with `R` clear. The old word goes there
/// and leaves the stack pointer at 1; the new one, `DR` set, returns to
/// [`RET_PC`] and pops it to 0.
fn quux_hung_popj_one_word(pre: usize, n: usize, i: usize, xl: bool) -> (Machine, [(u32, u32); 1]) {
    let low = MD_BEFORE & 7;
    let mut m = as_quux(&hung_popj(pre, n, i, xl, false));
    m.dmem[E as usize] = 0;
    m.dmem[E as usize + 1] = 0;
    m.dmem[(E as u32 + low) as usize] = H_OLD_DPC;
    (m, [(PHYS_13, low)])
}

/// **On QUUX a `POPJ` in a dispatch write takes the old word, waiting for
/// `MD` or not**: the same shapes on `quux`, the dispatch writing
/// and reading one word ([`quux_hung_popj_one_word`]). Among them are
/// microcycles that wait for the word read where the CADR's would hang;
/// QUUX runs them once, whole, after the wait.
#[test]
fn on_quux_a_popj_dispatch_write_takes_the_old_word_waiting_or_not() {
    let mut stretched = 0;
    for pre in 0..6 {
        for n in 3..8 {
            for i in 0..=n.min(3) {
                for xl in [false, true] {
                    let (m, main) = quux_hung_popj_one_word(pre, n, i, xl);
                    let (e, rows) = rtl_timed(
                        &m,
                        &main,
                        X_CYCLES,
                        TimingModel::Sync { cycle_ticks: 4, ilong_ticks: 0 },
                        None,
                    );
                    let r = row_of(&rows, hung_popj_insn(xl).raw());
                    stretched += (r.to - r.from > 40) as usize;
                    let what = format!("({pre}, {n}, {i}, {xl})");
                    assert_eq!((e.mmem[5], e.spcptr), (AT_OLD_DPC, 1), "{what}: rtl");
                    let (u, _) = micro(&m, &main, X_CYCLES);
                    assert_eq!((u.mmem[5], u.spcptr), (AT_OLD_DPC, 1), "{what}: micro");
                }
            }
        }
    }
    assert!(stretched > 0, "some dispatch microcycle waited for MD");
}

/// **A checkpoint inside the wait for `MD` keeps the old word on QUUX**:
/// the program of [`on_quux_a_popj_dispatch_write_takes_the_old_word_waiting_or_not`]
/// ([`quux_hung_popj_one_word`]) whose dispatch waits longest, run in steps
/// of 10 ns (a step ends inside the wait at its bound) and saved and loaded
/// into a fresh engine after each, ends as the straight run does.
#[test]
fn on_quux_a_checkpoint_inside_the_wait_keeps_the_old_word() {
    use muir::checkpoint::{Reader, Writer};
    use muir::rtl::Rtl;
    let mut longest = None;
    for pre in 0..6 {
        for n in 3..8 {
            for i in 0..=n.min(3) {
                for xl in [false, true] {
                    let (m, main) = quux_hung_popj_one_word(pre, n, i, xl);
                    let (_, rows) = rtl_timed(
                        &m,
                        &main,
                        X_CYCLES,
                        TimingModel::Sync { cycle_ticks: 4, ilong_ticks: 0 },
                        None,
                    );
                    let r = row_of(&rows, hung_popj_insn(xl).raw());
                    let over = (r.to - r.from).saturating_sub(40);
                    if longest.as_ref().is_none_or(|&(o, _, _, _)| over > o) {
                        longest = Some((over, m, main, r));
                    }
                }
            }
        }
    }
    let (over, mut m, main, r) = longest.unwrap();
    assert!(over > 0, "a dispatch that waits for MD");
    for (p, w) in main {
        m.main[p as usize] = u64::from(w);
    }
    let mut e = Rtl::new(m);
    e.boot();
    let mut inside = 0;
    while e.ns() < r.to + 2_000 {
        let t = e.ns();
        e.step_until(t + 10).unwrap();
        inside += (e.ns() > r.from && e.ns() < r.to) as usize;
        let mut w = Writer::new();
        e.save(&mut w);
        let body = w.finish();
        let mut back = Rtl::new(as_quux(&Machine::new()));
        back.load(&mut Reader::for_word_bits(&body, 40)).unwrap();
        e = back;
    }
    assert!(inside > 0, "saved inside the wait ({}..{})", r.from, r.to);
    let mm = e.machine();
    assert_eq!((support::low(mm.mmem[5]), mm.spcptr), (AT_OLD_DPC, 1));
}

/// **muir-fpga's three programs of this shape, on `rtl` under its grid,
/// take the new word** --- the answer the fabric is held to, and the board's
/// in [`a_popj_dispatch_write_whose_read_finishes_inside_its_hung_microcycle_takes_the_new_word`].
/// Built as that generator builds them, the speed set after the boot and
/// the words' `DPC` moved past a longer program, which moves no microcycle:
/// the dispatch runs at 32170..32320, 32100..32250 and, under `ILONG`,
/// 32020..32210, and the read is acknowledged at 32060, so `-RDFINISH`
/// falls at 32200, 30, 100 and 180 ns into it.
#[test]
fn rtl_on_the_fpga_grid_takes_the_new_word_in_muir_fpgas_hung_popj_programs() {
    for ((pre, n, i, xl), (s, t)) in [
        ((2, 6, 0, false), (32170, 32320)),
        ((2, 5, 2, false), (32100, 32250)),
        ((2, 5, 0, true), (32020, 32210)),
    ] {
        let m = hung_popj(pre, n, i, xl, false);
        let (rtl, rows) = rtl_timed(&m, &[], 400, TimingModel::Fpga, Some(NORMAL));
        let r = row_of(&rows, hung_popj_insn(xl).raw());
        assert_eq!((r.from, r.to), (s, t), "x-popj-{pre}-{n}-{i}-{}", xl as u8);
        assert_eq!((rtl.mmem[5], rtl.spcptr), (AT_NEW_DPC, 0), "x-popj-{pre}-{n}-{i}-{}", xl as u8);
    }
}

/// Which instruction waits on `MBUSY.SYNC` after the read.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Waits {
    /// `VMA<-` (muir-fpga's `t-`).
    Vma,
    /// `VMA-WRITE-MAP<-`, then `M13<-MD` (its `w-`).
    WriteMap,
}

/// muir-fpga's `read_then_pre` at extra slow speed: the constants of
/// [`read_then`], `pre` fillers, `MD<-`, the PDL pointer, a filler,
/// `VMA-START-READ` of [`VADDR`], `n` fillers, then the instruction that
/// waits, returned beside the machine. Both waiting instructions are
/// `DESTMEM`, and `-WAIT` holds them on `DESTMEM AND MBUSY.SYNC` (VCTL1
/// 3F16).
fn waits_on_the_read(waits: Waits, pre: usize, n: usize) -> (Machine, u64) {
    waits_on_the_read_at(waits, 0, pre, n, 0)
}

/// [`waits_on_the_read`] with the program setting `speed` itself first
/// when it is not extra slow, and the first `il` of the `pre` fillers under
/// `ILONG`, as muir-fpga's `n-` programs have them.
fn waits_on_the_read_at(
    waits: Waits,
    speed: u16,
    pre: usize,
    n: usize,
    il: usize,
) -> (Machine, u64) {
    let mut p = vec![filler()];
    if speed != 0 {
        speed_prologue(speed, &mut p);
    }
    constant(MD_BEFORE, 1, &mut p);
    constant(VADDR, 12, &mut p);
    constant(0o20, 14, &mut p);
    if waits == Waits::WriteMap {
        constant(MAP_STORE, 2, &mut p);
    }
    for k in 0..pre {
        p.push(if k < il { ilong(filler()) } else { filler() });
    }
    p.push(Insn::new(ALU | SETM | m_src(1) | a_src(3) | MD));
    p.push(Insn::new(ALU | SETM | m_src(14) | a_src(3) | PDL_POINTER));
    p.push(filler());
    p.push(Insn::new(ALU | SETM | m_src(12) | a_src(3) | START_READ));
    for _ in 0..n {
        p.push(filler());
    }
    let x = match waits {
        Waits::Vma => Insn::new(ALU | SETM | m_src(12) | a_src(3) | VMA),
        Waits::WriteMap => Insn::new(ALU | SETM | m_src(2) | a_src(3) | WRITE_MAP),
    };
    p.push(x);
    if waits == Waits::WriteMap {
        p.push(Insn::new(ALU | SETM | SRC_MD | a_src(3) | m_dest(13)));
    }
    for _ in 0..4 {
        p.push(filler());
    }
    let here = p.len();
    p.push(halt_here(here));
    let mut m = program(p);
    m.l2_map[1] = (1 << 23) | (1 << 22) | 0o100;
    m.l2_map[2] = MODE_PAGE;
    (m, x.raw())
}

/// What the board and `rtl` do with the wait of [`waits_on_the_read`]:
/// the board's `-MEMACK`, `-MFINISHD`, the `MCLK1A` edge at or after it and
/// `MBUSY.SYNC`'s fall; and the waiting microcycle's length on the board,
/// on `rtl` under the board's timing and on `rtl` under muir-fpga's grid.
struct Wait {
    memack: u64,
    mfinishd: u64,
    mclk: u64,
    sync_falls: u64,
    chip: u64,
    cadr: u64,
    fpga: u64,
}

fn wait_of(waits: Waits, pre: usize, n: usize) -> Wait {
    let (m, x) = waits_on_the_read(waits, pre, n);
    let main = [(PHYS, READ_WORD)];
    let mut b = board(&m, &main);
    let (es, ev) = watch(&mut b, 480);
    let (s, t) = microcycle_of(&es, x);
    let start =
        microcycle_of(&es, Insn::new(ALU | SETM | m_src(12) | a_src(3) | START_READ).raw()).0;
    let memack = first(&ev, "-MEMACK", Level::Low, start).unwrap();
    let mfinishd = first(&ev, "-MFINISHD", Level::Low, start).unwrap();
    let mclk = first(&ev, "MCLK1A", Level::High, mfinishd).unwrap();
    let sync_falls = first(&ev, "MBUSY.SYNC", Level::Low, start).unwrap();
    let len = |tm| {
        let (_, rows) = rtl_timed(&m, &main, 480, tm, None);
        let r = row_of(&rows, x);
        r.to - r.from
    };
    let w = Wait {
        memack,
        mfinishd,
        mclk,
        sync_falls,
        chip: t - s,
        cadr: len(TimingModel::Cadr),
        fpga: len(TimingModel::Fpga),
    };
    eprintln!(
        "{waits:?} pre {pre} n {n}: -MEMACK {memack}, -MFINISHD {mfinishd}, MCLK1A {mclk}, MBUSY.SYNC falls {sync_falls}; waiting microcycle chip {} rtl {} fpga-grid {}",
        w.chip, w.cadr, w.fpga
    );
    w
}

/// **`-MFINISHD` a few nanoseconds before a master clock edge ends the wait
/// at that edge**, on the board and on `rtl` under both timings. These are
/// muir-fpga's `t-0-2-1` and `w-0-1-1`, built as its generator builds them.
/// On the board the acknowledgement is at 24601 and 31643 ns, so `MBUSY` is
/// cleared (the 74S74 at VCTL1 1D21, by `-MFINISHD` off the TD50 at 1D23)
/// 9 and 7 ns before the `MCLK1A` edge that clocks `MBUSY.SYNC` (the 74S175
/// at 1E20, D = `MEMRQ`), and `MBUSY.SYNC` falls at that edge: the waiting
/// microcycle is four generator cycles, 880 ns.
///
/// Under muir-fpga's 10 ns grid the acknowledgement is rounded up to the
/// next tick, 24610 and 31650, which puts `-MFINISHD` exactly on the edge.
/// `rtl` takes a change due at an edge as made before it (`Rtl::bus_cycle`
/// and `Rtl::after_memack` run to the edge before the edge's registers),
/// and so gives the board's 880. A grid that counts the tie as after the
/// edge waits one generator cycle more, 1100, where the board does not.
/// The `VMA-WRITE-MAP` store waits through the same `-WAIT` term, `DESTMEM
/// AND MBUSY.SYNC`, and gives the same answer.
#[test]
fn mfinishd_just_before_a_master_clock_edge_ends_the_wait_at_that_edge() {
    for (waits, pre, n, memack, edge) in
        [(Waits::Vma, 2, 1, 24601, 24640), (Waits::WriteMap, 1, 1, 31643, 31680)]
    {
        let w = wait_of(waits, pre, n);
        assert_eq!(
            (w.memack, w.mfinishd),
            (memack, memack + 30),
            "{waits:?}: the board's acknowledgement"
        );
        assert_eq!(
            (w.mclk, w.sync_falls),
            (edge, edge),
            "{waits:?}: MBUSY.SYNC falls at the next edge"
        );
        assert!(edge - w.mfinishd < 10, "{waits:?}: which the grid's round-up lands on");
        assert_eq!(
            (w.chip, w.cadr, w.fpga),
            (880, 880, 880),
            "{waits:?}: chip, rtl, rtl on the grid"
        );
    }
}

/// **`-MFINISHD` exactly on the master clock edge is taken as before it**,
/// on the board as `chip` has it and on `rtl`. The board's own timing
/// reaches the tie: in muir-fpga's `w-0-4-1`, `-4-2` and `-4-3` the read is
/// acknowledged at 32310, `-MFINISHD` falls at 32340, and `MCLK1A` rises at
/// 32340. `MBUSY.SYNC` falls at that edge and the wait ends there, on
/// `chip` and on `rtl` under both timings.
///
/// What `chip` does is its own event order and **not the board's**:
/// `Chip::transition` fires the delay-line taps due at an instant before it
/// clocks the parts, so `MBUSY` is already clear when the 74S175 at VCTL1
/// 1E20 samples `MEMRQ`. On a CADR the tie is a setup violation on that
/// flip-flop, and either outcome, or a late one, is the hardware's. It
/// is a convention, shared by `chip` and `rtl`: a change due at an edge is
/// made before the edge.
#[test]
fn mfinishd_on_the_master_clock_edge_itself_is_taken_as_before_it() {
    for n in 1..4 {
        let w = wait_of(Waits::WriteMap, 4, n);
        assert_eq!(
            (w.memack, w.mfinishd, w.mclk),
            (32310, 32340, 32340),
            "n {n}: the tie on the board"
        );
        assert_eq!(w.sync_falls, 32340, "n {n}: MBUSY.SYNC falls at the edge");
        assert_eq!(w.chip, w.cadr, "n {n}: rtl");
        assert_eq!(w.chip, w.fpga, "n {n}: rtl on the grid");
    }
}

/// **A word `-LOADMD` puts in `MD` at the instant of an edge is seen by the
/// microcycle the edge starts, and not by the one it ends.** muir-fpga's
/// `t-0-11-0` with its tail replaced by `MAP(MD)` read into M 16..27, one a
/// microcycle: `MAP(MD)` is functional source 11 and not `USE.MD`, so
/// nothing holds it while the read is in progress, and it shows which `MD`
/// the microcycle used. Level 2's word 3 (for [`MD_BEFORE`]) is 1111111 and
/// word 7 (for [`READ_WORD`]) 2222222.
///
/// On the board the memory acknowledges at 27060 ns, which is an edge: the
/// tie is the board's own timing, not a grid's. The microcycle ending there
/// registers the old `MD`'s word and the next the new one, on `chip` and on
/// `rtl` under both timings. On a CADR the second is defined --- the new `MD`
/// has the whole microcycle to reach the map and the ALU --- and so is the
/// first: `MD` moving at the edge cannot reach the registers that edge
/// clocks. Only `MD`'s own value at the instant is a tie, and nothing but a
/// trace samples it there.
#[test]
fn md_loaded_on_an_edge_is_seen_by_the_microcycle_it_starts() {
    let mut p = vec![filler()];
    constant(MD_BEFORE, 1, &mut p);
    constant(VADDR, 12, &mut p);
    constant(0o20, 14, &mut p);
    for _ in 0..11 {
        p.push(filler());
    }
    p.push(Insn::new(ALU | SETM | m_src(1) | a_src(3) | MD));
    p.push(Insn::new(ALU | SETM | m_src(14) | a_src(3) | PDL_POINTER));
    p.push(filler());
    p.push(Insn::new(ALU | SETM | m_src(12) | a_src(3) | START_READ));
    p.push(Insn::new(ALU | SETM | m_src(12) | a_src(3) | VMA));
    for r in 16..28 {
        p.push(Insn::new(ALU | SETM | SRC_MAP | a_src(3) | m_dest(r)));
    }
    let here = p.len();
    p.push(halt_here(here));
    let mut m = program(p);
    m.l2_map[1] = (1 << 23) | (1 << 22) | 0o100;
    m.l2_map[3] = 0o1111111;
    m.l2_map[7] = 0o2222222;
    let main = [(PHYS, READ_WORD)];
    let mut b = board(&m, &main);
    let (es, ev) = watch(&mut b, 480);
    let chip = board_end(&b);
    let loadmd = first(&ev, "-LOADMD", Level::High, 0).unwrap();
    assert_eq!(loadmd, 27060, "the board's acknowledgement");
    let k = es.iter().position(|e| e.ns == loadmd).expect("on an edge");
    assert_eq!((es[k - 1].md, es[k].md), (MD_BEFORE, READ_WORD), "MD across the edge");
    let want: Vec<u32> = (16..28).map(|r| if r <= 20 { 0o1111111 } else { 0o2222222 }).collect();
    assert_eq!(chip.mmem[16..28], want[..], "chip: M 20's microcycle ends at the edge");
    for tm in [TimingModel::Cadr, TimingModel::Fpga] {
        let (rtl, _) = rtl_timed(&m, &main, 480, tm, None);
        assert_eq!(rtl.mmem[16..28], want[..], "rtl under {tm:?}");
    }
}

/// The sweeps behind the tests above, printed: muir-fpga's `x-popj` shapes
/// at normal speed, and its `t-`, `w-` and `n-` shapes at both speeds, on
/// the board and on `rtl` under both timings. Slow (minutes), so run by
/// hand: `cargo test --test dispatch_write_order sweep -- --ignored
/// --nocapture`.
#[test]
#[ignore]
fn sweep_the_hung_popj_and_wait_shapes() {
    let mut jobs: Vec<(char, u16, usize, usize, usize, bool)> = Vec::new();
    for pre in 0..6 {
        for n in 3..8 {
            for i in 0..=n.min(3) {
                for xl in [false, true] {
                    jobs.push(('x', NORMAL, pre, n, i, xl));
                }
            }
        }
    }
    for speed in [0, NORMAL] {
        for pre in 0..24 {
            for n in 0..4 {
                jobs.push(('t', speed, pre, n, 0, false));
                jobs.push(('w', speed, pre, n, 0, false));
                for il in 0..6.min(pre + 1) {
                    jobs.push(('n', speed, pre, n, il, false));
                }
            }
        }
    }
    let jobs = std::sync::Mutex::new(jobs.into_iter().enumerate().collect::<Vec<_>>());
    let out = std::sync::Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                loop {
                    let Some((k, job)) = jobs.lock().unwrap().pop() else { break };
                    let line = sweep_one(job);
                    out.lock().unwrap().push((k, line));
                }
            });
        }
    });
    let mut out = out.into_inner().unwrap();
    out.sort();
    for (_, line) in out {
        println!("{line}");
    }
}

/// One program of [`sweep_the_hung_popj_and_wait_shapes`], as a line.
fn sweep_one((kind, speed, pre, n, i, xl): (char, u16, usize, usize, usize, bool)) -> String {
    if kind == 'x' {
        let m = hung_popj(pre, n, i, xl, true);
        let x = hung_popj_insn(xl).raw();
        let mut b = board(&m, &[]);
        let (es, ev) = watch(&mut b, X_CYCLES);
        let chip = board_end(&b);
        let (s, t) = microcycle_of(&es, x);
        let normal = if xl { 185 } else { 145 };
        let start =
            microcycle_of(&es, Insn::new(ALU | SETM | m_src(12) | a_src(3) | START_READ).raw()).0;
        let rdfinish = first(&ev, "-RDFINISH", Level::Low, start).unwrap();
        let at = rdfinish as i64 - s as i64;
        let place = if at < 0 {
            "before"
        } else if at <= normal {
            "inside"
        } else {
            "after"
        };
        let mut line = format!(
            "x-popj-{pre}-{n}-{i}-{} -RDFINISH {at:+} ns into {s}..{t} ({place}); chip M5 {:o}",
            xl as u8, chip.mmem[5]
        );
        for tm in [TimingModel::Cadr, TimingModel::Fpga] {
            let (rtl, rows) = rtl_timed(&m, &[], X_CYCLES, tm, None);
            let r = row_of(&rows, x);
            line += &format!("; {tm:?} {} ns M5 {:o}", r.to - r.from, rtl.mmem[5]);
        }
        return line;
    }
    let waits = if kind == 'w' { Waits::WriteMap } else { Waits::Vma };
    let (mut m, x) = waits_on_the_read_at(waits, speed, pre, n, i);
    if kind == 'n' {
        // Level-2 entry 1 at physical page 20000, past the memory: the read
        // is ended by the bus interface's NXM timer.
        m.l2_map[1] = (1 << 23) | (1 << 22) | 0o20000;
    }
    let main: Vec<(u32, u32)> = if kind == 'n' { vec![] } else { vec![(PHYS, READ_WORD)] };
    let mut b = board(&m, &main);
    let (es, ev) = watch(&mut b, 520);
    let (s, t) = microcycle_of(&es, x);
    let start =
        microcycle_of(&es, Insn::new(ALU | SETM | m_src(12) | a_src(3) | START_READ).raw()).0;
    let memack = first(&ev, "-MEMACK", Level::Low, start).unwrap();
    let mfinishd = first(&ev, "-MFINISHD", Level::Low, start).unwrap();
    let mclk = first(&ev, "MCLK1A", Level::High, mfinishd).unwrap();
    let loadmd_on_edge =
        first(&ev, "-LOADMD", Level::High, start).is_some_and(|l| es.iter().any(|e| e.ns == l));
    let mut line = format!(
        "{kind}-{speed}-{pre}-{n}{} -MEMACK {memack} -MFINISHD {} ns before MCLK1A{}; waits chip {}",
        if kind == 'n' { format!("-{i}") } else { String::new() },
        mclk - mfinishd,
        if loadmd_on_edge { ", -LOADMD on an edge" } else { "" },
        t - s
    );
    for tm in [TimingModel::Cadr, TimingModel::Fpga] {
        let (_, rows) = rtl_timed(&m, &main, 520, tm, None);
        let r = row_of(&rows, x);
        line += &format!("; {tm:?} {}", r.to - r.from);
    }
    line
}
