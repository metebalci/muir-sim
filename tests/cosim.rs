// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The engines against each other, microinstruction by microinstruction.
//!
//! `micro` models the architecturally visible pipeline; `rtl` computes the
//! whole datapath on the machine's own clock; `chip` resolves the 1243 parts
//! of `data/CADR.netlist`.  Three readings of the same drawings by three
//! different roads, so where two of them agree about something neither was
//! told, that is evidence.
//!
//! The weight is not equal.  `micro` and `rtl` share the ALU and the jump
//! conditions (`crate::ttl`), so those two agreeing about an ALU result says
//! less than either agreeing with `chip`, which computes it from the 74S181s
//! themselves.  `tests/chip.rs` is where that comparison lives; this file is
//! `micro` against `rtl`, which is what catches a pipeline modeled wrong.
//!
//! Only executed cycles are compared.  A cycle the pipeline inhibits runs on
//! the board and retires nothing, so counting it would make the two engines
//! disagree about a number neither is wrong about.

use std::path::{Path, PathBuf};

use muir::engine::Engine;
use muir::isa::Insn;
use muir::machine::Machine;
use muir::micro::Micro;
use muir::rtl::Rtl;

mod support;
use support::machine_with_pack;

/// The System 100 pack, which these comparisons need for a reason worth
/// stating: they run past `PROM-DISABLE`, so the disk must have loaded
/// microcode 323 by then. Without a pack both engines would spin in
/// `DISK-RECALIBRATE` in step with each other and agree about nothing.
fn pack() -> Option<PathBuf> {
    support::pack_100()
}

/// A machine with the System 100 pack, `pack`, on unit 0.
///
/// Both engines must have the same pack on the same unit, or they diverge on
/// the first thing the boot PROM reads off it rather than on anything either
/// computes.
fn booted<E>(make: impl Fn(Machine) -> E, boot: impl Fn(&mut E), pack: &Path) -> E {
    let mut e = make(machine_with_pack(pack));
    boot(&mut e);
    e
}

/// **The strongest thing here.** After the same number of executed
/// instructions, both engines must hold the same memories.  Agreeing about
/// which instruction runs next only shows control flow agreeing; this shows
/// the data does too, and control flow reaches data only where something
/// branches on it.
///
/// It runs to just short of `PROM-DISABLE`, so what it compares is the
/// machine the boot PROM is about to hand over --- every word of the band
/// read off the pack a block at a time and written by `WRITE-I-MEM`,
/// `WRITE-DISPATCH-RAM` and the A-memory loop. Two engines agreeing on 12,449
/// control store words that came through the disk controller is the strongest
/// thing the disk work has to stand on.
///
/// **`A-DISK-STATUS` is what stops the count being pushed further**, and
/// this runs to the last instruction before it: at 1,271,908 the word
/// parts and never comes back. Its top eight bits are the disk's block
/// counter, which is where the pack has turned to, and the two engines'
/// clocks are not the same clock --- `micro` charges a mean memory wait
/// and has no bus to wait on, `rtl` counts its stalls --- so from the
/// first status read the microcode makes with the band running, the two
/// see the pack at different sectors. The tail of this test holds that.
///
/// **`M-GARBAGE` is not what stops it**, which is what this comment used
/// to say. `M 0` disagrees at many counts and so do `M 5`, `M 14`, `M 21`
/// and `M 33` and a word of main memory: one engine has made a store the
/// other has not, and a stop one instruction later has them agreeing
/// again. It is the same stop artifact as the dead stack slot below, and
/// `M 0` is only the most conspicuous of them because every discarded
/// result goes there. Measured, not argued: every one of them is
/// transient, and `A-DISK-STATUS` is the only word that parts for good.
#[test]
fn engines_agree_on_memory() {
    let Some(p) = pack() else { return };
    // The last executed instruction at which every memory, `VMA`, `MD` and
    // `LC` agree; `PROM-DISABLE` was at 1,262,276, so this is a little way
    // into microcode 323 running.
    const N: usize = 1_271_901;
    /// Where `A-DISK-STATUS` lives: A-memory 323, `sys/ubin/ucadr.sym`.
    const A_DISK_STATUS: usize = 0o323;

    let mut a = booted(Micro::new, Micro::boot, &p);
    let mut b = booted(Rtl::new, Rtl::boot, &p);
    let mut n = 0;
    while n < N {
        a.step().expect("micro halted early");
        if a.executed().is_some() {
            n += 1;
        }
    }
    let mut n = 0;
    while n < N {
        b.step().expect("rtl halted early");
        if b.executed().is_some() {
            n += 1;
        }
    }

    // `LC` is not in `Machine` on both engines, so it is asked for rather
    // than read out of the two machines: `Engine::lc`, issue #32. Here it
    // is 0 on both --- the band has not fetched a macroinstruction yet, and
    // does not until 1,844,875 --- so what this says is that neither
    // engine has touched it. A counter that steps is held by `traces`, an
    // instruction at a time, and by
    // `the_engines_hold_the_same_lc_until_their_clocks_part`.
    assert_eq!(a.lc(), b.lc(), "LC");
    let (x, y) = (a.machine(), b.machine());
    assert!(x.imem == y.imem, "control store");
    assert_eq!(x.mmem, y.mmem, "M memory");
    assert_eq!(x.amem, y.amem, "A memory");
    assert_eq!(x.pdl, y.pdl, "PDL buffer");
    // The live stack must agree.  One dead slot above the pointer does not.
    // `WRITE-I-MEM` is a JUMP with both `IR<9>` and `IR<8>` set, so it pushes
    // the return address and the `POPJ` in the cycle CONTRL 3D26's `IWRITED`
    // drives pops it straight back.  The slot above the pointer is left
    // holding that dead return address, and which microcycle each engine
    // writes it in decides what is in it.  Same control flow, one stale slot.
    assert_eq!(x.spcptr, y.spcptr, "microcode stack pointer");
    assert_eq!(x.spc[x.spcptr as usize], y.spc[y.spcptr as usize], "top of microcode stack");
    assert_eq!(x.dmem, y.dmem, "dispatch memory");
    assert_eq!(x.l1_map, y.l1_map, "level-1 map");
    assert_eq!(x.l2_map, y.l2_map, "level-2 map");
    assert_eq!(x.q, y.q, "Q register");
    assert_eq!(x.vma, y.vma, "VMA");
    assert_eq!(x.md, y.md, "MD");
    assert!(x.main == y.main, "main memory");

    // **And what parts next**: thirteen instructions on, at 1,271,914,
    // `A-DISK-STATUS` is the one word in any memory the two engines
    // disagree about, and they disagree only in the eight bits above 23
    // --- the block counter, which is where the pack has turned to. That
    // is the ceiling on the count above: it cannot be raised past it, and
    // the reason is the clock rather than anything either engine
    // computes. Thirteen and not one, because a store in flight at the
    // stop puts a word or two out for an instruction at a time; by here
    // every one of those has landed on both.
    let mut n = 0;
    while n < 13 {
        a.step().expect("micro halted early");
        n += a.executed().is_some() as usize;
    }
    let mut n = 0;
    while n < 13 {
        b.step().expect("rtl halted early");
        n += b.executed().is_some() as usize;
    }
    let (x, y) = (a.machine(), b.machine());
    let (p, q) = (x.amem[A_DISK_STATUS], y.amem[A_DISK_STATUS]);
    assert_ne!(p, q, "A-DISK-STATUS has parted: {p:o} and {q:o}");
    assert_eq!(p & 0o77777777, q & 0o77777777, "and only above bit 23: {p:o} and {q:o}");
    let mut but_that_word = x.amem;
    but_that_word[A_DISK_STATUS] = q;
    assert_eq!(but_that_word, y.amem, "A memory but that one word");
    assert_eq!(x.mmem, y.mmem, "M memory");
    assert!(x.imem == y.imem, "control store");
    assert!(x.main == y.main, "main memory");
    assert_eq!(x.md, y.md, "MD");
}

/// **A moving `LC`, held between the engines for as long as they are the
/// same machine.**
///
/// `engines_agree_on_memory` compares the counter at a point where it is
/// 0 on both, which says only that neither engine has touched it. This
/// runs the two in lockstep from the button and compares it at **every**
/// executed instruction, so a counter that moved early, or by the wrong
/// step, or on one engine and not the other, is caught wherever it
/// happens.
///
/// **The band does not fetch a macroinstruction until 1,844,875**, half a
/// million instructions past `PROM-DISABLE`: the boot PROM never moves the
/// counter, and microcode 323 spends that long initializing in microcode
/// before it runs any macrocode. So `LC` is 0 for all but the last few
/// hundred instructions of this run, and the assertion is worth making
/// over all of them because 0 is what it should be.
///
/// **What ends it is the clock, and it is 483 instructions later.** At
/// 1,845,358 the two engines execute different microinstructions for the
/// first time. Both are at `INTRX0+2`, which MIT's interrupt microcode
/// writes as `(JUMP-IF-BIT-CLEAR (BYTE-FIELD 1 4) READ-MEMORY-DATA
/// INTRX1)` --- a branch on bit 4 of the word read from `A-TV-REGS-BASE`,
/// which is the display board's vertical flag, and what MIT's comment two
/// lines below calls "the roughly-60-cycle clock interrupt handler". `rtl`
/// has the bit set and `micro` has not: the flag is up once a frame of
/// [`muir::tv::FRAME_NS`] counted from power-on, and after 1.8
/// million instructions `micro`'s clock stands 8 ms from `rtl`'s ---
/// `micro` charges a mean memory wait and has no bus to wait on, `rtl`
/// counts its stalls. So the two are on different sides of a frame
/// boundary, `rtl` runs the clock handler and `micro` goes on to the disk.
/// Neither is wrong; they are no longer the same machine.
///
/// **So a long comparison of a moving `LC` is not gettable between these
/// two engines**, and the 483 instructions here are the whole of it. It
/// wants two engines with the same clock --- `rtl` and `chip` --- over the
/// band.
#[test]
fn the_engines_hold_the_same_lc_until_their_clocks_part() {
    let Some(p) = pack() else { return };
    /// The executed instruction at which `LC` first leaves zero: the band's
    /// first macroinstruction fetch.
    const LC_FIRST_MOVES: usize = 1_844_875;
    /// The executed instruction at which the two engines first execute
    /// different microinstructions.
    const CLOCKS_PART: usize = 1_845_358;

    let mut a = booted(Micro::new, Micro::boot, &p);
    // Where the clocks part turns on what `micro` charges a memory cycle,
    // so this test names its own charge rather than taking the band's mean
    // (`MEMORY_ACCESS_NS`, 520 ns): with 460 the two stay on the same side
    // of a frame until `CLOCKS_PART`, after the counter first moves; with
    // 520 they part at 1,842,835, before it does.
    a.memory_cycle_ns = 460;
    let mut b = booted(Rtl::new, Rtl::boot, &p);
    let mut first_moved = 0;
    let mut steps = Vec::new();
    let mut last = 0;
    for n in 1..CLOCKS_PART {
        let ea = loop {
            a.step().expect("micro halted early");
            if let Some(e) = a.executed() {
                break e;
            }
        };
        let eb = loop {
            b.step().expect("rtl halted early");
            if let Some(e) = b.executed() {
                break e;
            }
        };
        assert_eq!(ea, eb, "the same microinstruction at {n}");
        let lc = a.lc();
        assert_eq!(lc, b.lc(), "LC at instruction {n}, micro PC {ea:o}");
        if lc != 0 && first_moved == 0 {
            first_moved = n;
        }
        if lc != last {
            steps.push(lc as i64 - last as i64);
            last = lc;
        }
    }
    assert_eq!(first_moved, LC_FIRST_MOVES, "where the counter first leaves zero");
    // It left zero, and then stepped. `LC<25:2>` is the fetch address and
    // the two bits below it are the halfword, so a step of a whole
    // macroinstruction is 4 and of a halfword 2; nothing else is a step
    // this counter makes going forwards.
    assert!(steps.len() > 1, "the counter moved more than once: {steps:?}");
    for step in &steps[1..] {
        assert!(*step == 2 || *step == 4, "a halfword or a word: {steps:?}");
    }

    // And the parting itself: both at `INTRX0+2` with different words in
    // `MD`, differing in the vertical flag, because the two engines'
    // clocks have put them on different sides of a frame --- and in the
    // sync bits, which are the program's at two different instants.
    let (x, y) = (a.machine(), b.machine());
    let sync = muir::tv::mode::VSYNC | muir::tv::mode::HSYNC;
    assert_eq!(
        (x.md ^ y.md) & u64::from(!sync),
        muir::tv::mode::VERT.into(),
        "MD differs in the vertical flag, the sync bits aside"
    );
    assert_ne!(x.tv.vert_flag(x.ns), y.tv.vert_flag(y.ns), "and the flag itself is what differs");
    assert_ne!(x.ns, y.ns, "the clocks have parted");
    // The branch goes two ways: `micro` to `INTRX1`, `rtl` on into the
    // clock handler at `INTRX0+3`. Addresses of microcode 323, named by
    // `sys/ubin/ucadr.sym`.
    let ea = loop {
        a.step().expect("micro halted early");
        if let Some(e) = a.executed() {
            break e;
        }
    };
    let eb = loop {
        b.step().expect("rtl halted early");
        if let Some(e) = b.executed() {
            break e;
        }
    };
    assert_eq!(ea, 0o25755, "micro takes INTRX1");
    assert_eq!(eb, 0o25740, "rtl falls through to INTRX0+3");
}

/// **`micro`'s clock is the machine's periods.** Each of its microcycles is
/// as long as the speed bits and `ILONG` make it, 220 ns through the boot
/// and 145 after microcode 323 writes the mode register, and nothing more:
/// it has no bus to wait on. `rtl`'s clock is the same periods plus its
/// waits and hangs, which it counts, so at every executed instruction the
/// two agree once `rtl`'s stalls are taken off. Held from the button to
/// well past the mode register write.
#[test]
fn micro_keeps_the_machines_periods() {
    let Some(p) = pack() else { return };
    const N: usize = 1_300_000;
    let mut a = booted(Micro::new, Micro::boot, &p);
    // The periods alone: off with the mean wait `micro` charges a memory
    // cycle, which `micro_charges_the_mean_memory_wait` checks by itself.
    a.memory_cycle_ns = 0;
    let mut b = booted(Rtl::new, Rtl::boot, &p);
    // From the first executed instruction: the two start up differently,
    // `rtl` charging one more cycle before it, and that is the button, not
    // the clock.
    let (mut a0, mut b0) = (0, 0);
    let mut n = 0;
    let mut speeds = 0;
    while n < N {
        a.step().expect("micro halted early");
        if a.executed().is_none() {
            continue;
        }
        n += 1;
        loop {
            b.step().expect("rtl halted early");
            if b.executed().is_some() {
                break;
            }
        }
        if n == 1 {
            (a0, b0) = (a.machine().ns, b.ns() - b.stalled_ns());
        }
        let (x, y) = (a.machine().ns - a0, b.ns() - b.stalled_ns() - b0);
        assert_eq!(
            x,
            y,
            "instruction {n} at micro PC {:o}, rtl PC {:o}: micro's clock {x} ns, rtl's less its stalls {y}",
            a.pc(),
            b.pc()
        );
        if a.machine().mode.speed1 {
            speeds += 1;
        }
    }
    assert!(speeds > 0, "the window never saw the speed change");
    eprintln!(
        "{N} instructions, clocks agree: micro {} ns, rtl {} ns of which {} stalled over {} memory cycles, {} ns a cycle",
        a.machine().ns,
        b.ns(),
        b.stalled_ns(),
        b.bus_cycles(),
        b.stalled_ns() / b.bus_cycles().max(1)
    );
    // And the band's mean, from `rtl` alone: the number `micro` charges a
    // memory cycle is the band's, not the boot's.
    let (s0, c0, n0) = (b.stalled_ns(), b.bus_cycles(), n);
    while n < 15_000_000 {
        b.step().expect("rtl halted in the band");
        if b.executed().is_some() {
            n += 1;
        }
    }
    let (s, c) = (b.stalled_ns() - s0, b.bus_cycles() - c0);
    eprintln!(
        "band, instructions {n0} to {n}: {s} ns stalled over {c} memory cycles, {} ns a cycle",
        s / c.max(1)
    );
    // `micro::MEMORY_ACCESS_NS` is that mean, rounded to ten nanoseconds.
    let mean = s as f64 / c.max(1) as f64;
    assert_eq!(
        ((mean / 10.0).round() * 10.0) as u64,
        muir::micro::MEMORY_ACCESS_NS,
        "the band's mean wait is {mean:.1} ns a memory cycle"
    );
}

/// **`micro` charges the measured mean wait per memory cycle and nothing
/// else.** Two `micro` runs to the same instruction, one with the charge
/// off, differ by exactly the cycles started times `MEMORY_ACCESS_NS`.
#[test]
fn micro_charges_the_mean_memory_wait() {
    let Some(p) = pack() else { return };
    // Past the boot PROM's self-tests, which touch no memory for the first
    // 536,000 microcycles, and into its disk reads.
    const N: usize = 1_000_000;
    let run = |charge: u64| {
        let mut e = booted(Micro::new, Micro::boot, &p);
        e.memory_cycle_ns = charge;
        let mut n = 0;
        while n < N {
            e.step().expect("micro halted early");
            if e.executed().is_some() {
                n += 1;
            }
        }
        (e.machine().ns, e.memory_cycles())
    };
    let (plain, cycles) = run(0);
    let (charged, cycles2) = run(muir::micro::MEMORY_ACCESS_NS);
    assert_eq!(cycles, cycles2, "the charge changes no cycle");
    assert!(cycles > 0);
    assert_eq!(
        charged - plain,
        cycles * muir::micro::MEMORY_ACCESS_NS,
        "the charge is the mean per cycle, {cycles} cycles"
    );
}

/// What the location counter does to the shifter in the two byte modes,
/// on `micro` and `rtl` alike, against what CC's diagnostics expect.
///
/// `CC-TEST-LC-DP` in `sys/cc/diags.lisp` writes `LC` and reads the
/// shifter's output of an all-ones word with `IR<11:10>` = 3: "Select byte
/// (initially rightmost, LC=current+1)", `LC` = 1 selecting byte 0 and
/// `LC` = 4 byte 3, and in word mode `LC` = 2 the low halfword and 4 the
/// high one. That is the drawings' `SH4` and `SH3` on page LC, which `rtl`
/// computes gate by gate. `micro` had a formula that inverts both bits in
/// byte mode, and nothing saw it because the boot never leaves word mode:
/// `CC-TEST-LC-DP` is the first thing that asks.
#[test]
fn the_lc_shift_selects_the_byte_the_diagnostics_expect() {
    fn shifter_output<E: Engine>(
        make: impl Fn(Machine) -> E,
        boot: impl Fn(&mut E),
        byte_mode: bool,
        lc: u32,
        width: u32,
    ) -> u32 {
        let mut m = Machine::new();
        // The constants the program takes from A memory, and the all-ones
        // word it shifts, in M memory 2.
        m.amem[10] = u64::from((byte_mode as u32) << 29);
        m.amem[11] = u64::from(lc);
        m.amem[12] = 0;
        m.mmem[2] = 0xffff_ffff;
        m.amem[2] = 0xffff_ffff;
        // `tests/chip.rs`'s encoding: `IR<13:12>` = 1 puts the ALU on OB,
        // `IR<8:3>` = 5 is SETA, the A address is `IR<41:32>` and a
        // functional destination is `IR<23:19>` with `IR<25>` clear.
        let alu = 1u64 << 12;
        let seta = 5u64 << 3;
        let a_src = |a: u64| a << 32;
        let int_control = (0o2u64 << 19) | (0o37 << 14);
        let lc_dest = (0o1u64 << 19) | (0o37 << 14);
        // A BYTE-class word with `IR<11:10>` = 3, MR and SR (`IR<13:12>`),
        // `width` bits from position 0, M memory 2 against A memory 12,
        // into A memory 200.
        let byte_op = (3u64 << 43)
            | (1 << 25)
            | (0o200 << 14)
            | a_src(12)
            | (2 << 26)
            | (3 << 12)
            | (3 << 10)
            | ((width as u64 - 1) << 5);
        m.load_prom(
            &[a_src(10) | int_control | seta | alu, a_src(11) | lc_dest | seta | alu, byte_op]
                .map(Insn::new),
        );
        let mut e = make(m);
        boot(&mut e);
        // The trap and its delay slot, the three words, and the write-back
        // microcycle behind the last of them, on either engine.
        for _ in 0..8 {
            e.step().unwrap();
        }
        support::low(e.machine().amem[0o200])
    }
    let cases = [
        (true, 1, 8, 0xffu32),
        (true, 2, 8, 0xff00),
        (true, 3, 8, 0xff_0000),
        (true, 4, 8, 0xff00_0000),
        (false, 2, 16, 0xffff),
        (false, 4, 16, 0xffff_0000),
    ];
    for (byte_mode, lc, width, want) in cases {
        let mode = if byte_mode { "byte" } else { "word" };
        let r = shifter_output(Rtl::new, Rtl::boot, byte_mode, lc, width);
        assert_eq!(r, want, "rtl: LC {lc} in {mode} mode, {width} bits");
        let u = shifter_output(Micro::new, Micro::boot, byte_mode, lc, width);
        assert_eq!(u, want, "micro: LC {lc} in {mode} mode, {width} bits");
    }
}

// --- micro against rtl, one construct at a time ------------------------------

use muir::isa::asm::{
    ALU, ALWAYS, BYTE, DISPATCH, JUMP, MD, N, P, POPJ, R, SETM, SETO, SETZ, SRC_MD, START_READ,
    a_dest, a_src, d_addr, filler, m_src, src, target, width,
};

/// The functional destination `LOCATION-COUNTER`, 0o1, with the harmless
/// M word every functional destination here also writes.
const LC: u64 = (0o1 << 19) | (0o37 << 14);

/// Both engines on the same boot PROM --- `prom` at word 0, fillers behind
/// it --- with `set` having prepared the machine, run `steps` microcycles
/// each.
fn both(prom: &[Insn], set: &dyn Fn(&mut Machine), steps: usize) -> (Micro, Rtl) {
    let make = || {
        let mut m = Machine::new();
        let mut words = vec![filler(); 512];
        words[..prom.len()].copy_from_slice(prom);
        m.load_prom(&words);
        set(&mut m);
        m
    };
    let mut e = Micro::new(make());
    e.boot();
    for _ in 0..steps {
        e.step().unwrap();
    }
    let mut r = Rtl::new(make());
    r.boot();
    for _ in 0..steps {
        r.step().unwrap();
    }
    (e, r)
}

/// **A control-store write pushes and pops, leaving the pointer alone and
/// a word above it.**
///
/// `P` and `R` together on a JUMP write the control store rather than
/// jumping, and the board does not special-case the push. Page CONTRL: the
/// 74S64 at 3E26 makes `-SPUSH` from four AND groups, the first
/// `IRJUMP AND -IR6 AND IR8 AND JCOND`, and none of the four has `IWRITE`
/// in it --- `IWRITE` is decoded alone at the 74S11 3E29 as
/// `IRJUMP AND IR8 AND IR9`. A write is a jump-always with `P`, so the
/// group is satisfied and the machine pushes; on the next cycle `IWRITED`
/// off the 74S175 at 3D26 makes `POPJ` through the open-collector 74S08 at
/// 3D21, and it pops.
///
/// So the stack pointer ends where it began with the pushed word still in
/// the slot above it. That word is what a console reading the stack sees,
/// and it is the whole observable difference. Read off `data/CADR.netlist`
/// and confirmed against MIT's own wire list `cadrwd/cadr4.wlr`.
///
/// `micro` used to neither push nor pop here, which left a different word
/// above the pointer from the one the board leaves.
#[test]
fn a_control_store_write_pushes_and_pops_on_both() {
    // Write word 0o250 of the control store, then read the stack back.
    let prom =
        [filler(), Insn::new(JUMP | ALWAYS | P | R | target(0o250)), filler(), filler(), filler()];
    let (e, r) = both(&prom, &|_| {}, 20);
    let (em, rm) = (e.machine(), r.machine());
    assert_eq!(em.spcptr, rm.spcptr, "the stack pointer ends in the same place");
    let above = |p: u8| (p as usize + 1) & 0o37;
    assert_eq!(
        em.spc[above(em.spcptr)],
        rm.spc[above(rm.spcptr)],
        "and both leave the same word in the slot above it"
    );
    assert_eq!(em.imem[0o250].raw(), rm.imem[0o250].raw(), "both wrote the same word");
}

/// **A microinstruction that pushes and pops at once counts the stack
/// pointer up once and pops the old top**, on both. Page CONTRL: `-SPCNT`
/// is the open-collector 74S08 at 4D09 over `-SPUSH` and `-SPOP`, and page
/// SPC's 74S169s at 4F23 and 4F28 take it on `-ENT` and `SPUSH` on `U/-D`:
/// one count, up. The pop takes the stack's word at the pointer as it
/// stands; the pushed word lands a microcycle later at the pointer counted
/// up. A dispatch whose entry pushes does not pop for its M source at all,
/// `IGNPOPJ` holding the pop off at the 74S64 at CONTRL 3E28.
/// `a_push_and_a_pop_at_once_count_up_and_pop_the_old_top` in
/// `tests/chip.rs` runs the same programs on the netlist, which gives the
/// same words.
///
/// Each case pushes `X`, then runs its microinstruction; where control
/// lands --- `X`, `Y` or straight on --- records the SPC pointer and word.
#[test]
fn a_push_and_a_pop_at_once_count_up_and_pop_the_old_top() {
    use muir::isa::asm::DMEM_WRITE;
    let (x, y) = (60, 80);
    let here = |k: u64| Insn::new(JUMP | ALWAYS | target(k));
    let ret = 6;
    let cases: [(&str, u64, [u32; 3]); 5] = [
        ("a POPJ with a push", ALU | SETM | m_src(2) | fdest(0o15) | POPJ, [y, 0, 0]),
        ("the pop source into the push", ALU | SETM | src(0o14) | fdest(0o15), [0, 0, x]),
        ("a call with the POPJ bit", JUMP | ALWAYS | P | target(y as u64) | POPJ, [ret, 0, 0]),
        (
            "a call off the pop source",
            JUMP | ALWAYS | P | src(0o14) | target(y as u64),
            [0, ret, 0],
        ),
        (
            "a dispatch pushing, off the pop source",
            DISPATCH | src(0o14) | d_addr(0o10),
            [0, ret, 0],
        ),
    ];
    for (what, insn, want) in cases {
        let mut prom = vec![filler(); 90];
        // The dispatch entry at 10: P, to Y.
        prom[0] = Insn::new(DISPATCH | DMEM_WRITE | a_src(0o60) | d_addr(0o10));
        prom[2] = Insn::new(ALU | SETM | m_src(1) | fdest(0o15));
        prom[4] = Insn::new(insn);
        prom[6] = Insn::new(ALU | SETM | src(0o1) | a_dest(0o203));
        prom[7] = here(7);
        prom[x as usize] = Insn::new(ALU | SETM | src(0o1) | a_dest(0o201));
        prom[x as usize + 1] = here(x as u64 + 1);
        prom[y as usize] = Insn::new(ALU | SETM | src(0o1) | a_dest(0o202));
        prom[y as usize + 1] = here(y as u64 + 1);
        let set = |m: &mut Machine| {
            m.mmem[1] = u64::from(x);
            m.mmem[2] = u64::from(y);
            m.amem[0o60] = u64::from((1 << 15) | y);
        };
        let (e, r) = both(&prom, &set, 40);
        // Pointer 2 over the word, wherever it is recorded.
        let want = want.map(|w| if w == 0 { 0 } else { (2 << 24) | w });
        let got = |m: &Machine| [m.amem[0o201], m.amem[0o202], m.amem[0o203]].map(support::low);
        assert_eq!(got(r.machine()), want, "{what}: rtl");
        assert_eq!(got(e.machine()), want, "{what}: micro");
        assert_eq!(e.machine().spcptr, r.machine().spcptr, "{what}: the pointer");
        // The 19 bits the 82S21s at page SPC hold.
        let stack = |m: &Machine| m.spc.map(|w| w & 0o1777777);
        assert_eq!(stack(e.machine()), stack(r.machine()), "{what}: the stack");
    }
}

/// **Two pops in one microinstruction count the stack pointer down once,
/// and a dispatch whose entry has no R pops nothing**, on both. Page
/// CONTRL: `-SPOP` is one output, the 74S64 at 3E28, whatever pulls it,
/// and `-SPCNT` at 4D09 one count; both pops take the word at the pointer
/// as it stands. `IGNPOPJ`, `IRDISP AND -DR`, holds off the POPJ bit and
/// the functional source's pop together at that 74S64.
/// `two_pops_at_once_count_down_once_and_a_dispatch_without_r_pops_nothing`
/// in `tests/chip.rs` runs the same programs on the netlist, which gives
/// the same words.
///
/// Each case pushes `Y` and `X`, then runs its microinstruction; where
/// control lands --- `X`, `Y` or straight on --- records the SPC pointer
/// and word.
#[test]
fn two_pops_at_once_count_down_once_and_a_dispatch_without_r_pops_nothing() {
    use muir::isa::asm::DMEM_WRITE;
    let (x, y) = (60, 80);
    let here = |k: u64| Insn::new(JUMP | ALWAYS | target(k));
    // A pop lands at X, or goes straight on for a dispatch falling
    // through, with pointer 1 over Y; no pop lands at Y, or goes straight
    // on for a write, with pointer 2 over X.
    let popped = [(1 << 24) | y, 0, 0];
    let kept = [0, (2 << 24) | x, 0];
    let straight = [0, 0, (2 << 24) | x];
    let cases: [(&str, u64, [u32; 3]); 8] = [
        ("a POPJ off the pop source", ALU | SETM | src(0o14) | POPJ, popped),
        ("a return off the pop source", JUMP | ALWAYS | R | src(0o14), popped),
        ("a dispatch returning, off the pop source", DISPATCH | src(0o14) | d_addr(0o10), popped),
        (
            "a dispatch falling through, off the pop source",
            DISPATCH | src(0o14) | d_addr(0o12),
            [0, 0, (1 << 24) | y],
        ),
        ("a dispatch without R, off the pop source", DISPATCH | src(0o14) | d_addr(0o11), kept),
        (
            "a dispatch without R, off the pop source, with the POPJ bit",
            DISPATCH | src(0o14) | d_addr(0o11) | POPJ,
            kept,
        ),
        (
            "a dispatch-memory write off the pop source",
            DISPATCH | DMEM_WRITE | src(0o14) | a_src(0o61) | d_addr(0o11),
            straight,
        ),
        (
            "a control-store write off the pop source",
            JUMP | ALWAYS | P | R | src(0o14) | target(0o250),
            straight,
        ),
    ];
    for (what, insn, want) in cases {
        let mut prom = vec![filler(); 90];
        // The dispatch entries: at 10 R, at 11 neither R nor P, to Y, and
        // at 12 both.
        prom[0] = Insn::new(DISPATCH | DMEM_WRITE | a_src(0o60) | d_addr(0o10));
        prom[1] = Insn::new(DISPATCH | DMEM_WRITE | a_src(0o61) | d_addr(0o11));
        prom[2] = Insn::new(DISPATCH | DMEM_WRITE | a_src(0o62) | d_addr(0o12));
        prom[3] = Insn::new(ALU | SETM | m_src(2) | fdest(0o15));
        prom[4] = Insn::new(ALU | SETM | m_src(1) | fdest(0o15));
        prom[5] = Insn::new(insn);
        prom[7] = Insn::new(ALU | SETM | src(0o1) | a_dest(0o203));
        prom[8] = here(8);
        prom[x as usize] = Insn::new(ALU | SETM | src(0o1) | a_dest(0o201));
        prom[x as usize + 1] = here(x as u64 + 1);
        prom[y as usize] = Insn::new(ALU | SETM | src(0o1) | a_dest(0o202));
        prom[y as usize + 1] = here(y as u64 + 1);
        let set = |m: &mut Machine| {
            m.mmem[1] = u64::from(x);
            m.mmem[2] = u64::from(y);
            m.amem[0o60] = 1 << 16;
            m.amem[0o61] = u64::from(y);
            m.amem[0o62] = (1 << 16) | (1 << 15);
        };
        let (e, r) = both(&prom, &set, 40);
        let got = |m: &Machine| [m.amem[0o201], m.amem[0o202], m.amem[0o203]].map(support::low);
        assert_eq!(got(r.machine()), want, "{what}: rtl");
        assert_eq!(got(e.machine()), want, "{what}: micro");
        assert_eq!(e.machine().spcptr, r.machine().spcptr, "{what}: the pointer");
        // The 19 bits the 82S21s at page SPC hold.
        let stack = |m: &Machine| m.spc.map(|w| w & 0o1777777);
        assert_eq!(stack(e.machine()), stack(r.machine()), "{what}: the stack");
    }
}

/// **A return pop whose word has bit 14 up arms `NEXT INSTR` only when
/// the functional source does not pop too**, on both. Page LCC:
/// `NEXT.INSTR` is `SPOP AND SPC14 AND NOT SRCSPCPOPREAL`, the 74S02 at
/// 3E17 over `-SPOP` and the 74S00 at 3E07, registered at 3E12; it steps
/// LC and, with `NEEDFETCH` up, fetches.
/// `a_return_pop_with_the_pop_source_arms_no_fetch` in `tests/chip.rs`
/// runs the same programs on the netlist, which gives the same words and
/// no fetch.
///
/// Each case pushes `Y` and `X` with bit 14 up, writes LC, and runs its
/// microinstruction; where control lands --- `X` or straight on ---
/// records the SPC word and LC.
#[test]
fn a_return_pop_with_the_pop_source_arms_no_fetch() {
    use muir::isa::asm::DMEM_WRITE;
    let (x, y) = (60, 80);
    let here = |k: u64| Insn::new(JUMP | ALWAYS | target(k));
    // Pointer 1 over Y after one pop, pointer 3 over the pushed X after a
    // push and a pop; LC as written, with `NEEDFETCH`, `LC<31>` of the source, up.
    let popped = (1 << 24) | y;
    let lc = (1 << 31) | (0o20 << 2);
    let at_x = [popped, lc, 0, 0];
    let cases: [(&str, u64, [u32; 4]); 5] = [
        ("a POPJ off the pop source", ALU | SETM | src(0o14) | POPJ, at_x),
        ("a return off the pop source", JUMP | ALWAYS | R | src(0o14), at_x),
        ("a dispatch returning, off the pop source", DISPATCH | src(0o14) | d_addr(0o10), at_x),
        (
            "a dispatch falling through, off the pop source",
            DISPATCH | src(0o14) | d_addr(0o12),
            [0, 0, popped, lc],
        ),
        (
            "a POPJ with the pop source into the push",
            ALU | SETM | src(0o14) | fdest(0o15) | POPJ,
            [(3 << 24) | x | (1 << 14), lc, 0, 0],
        ),
    ];
    for (what, insn, want) in cases {
        let mut prom = vec![filler(); 90];
        // The dispatch entries: at 10 R, at 12 R and P.
        prom[0] = Insn::new(DISPATCH | DMEM_WRITE | a_src(0o60) | d_addr(0o10));
        prom[1] = Insn::new(DISPATCH | DMEM_WRITE | a_src(0o62) | d_addr(0o12));
        prom[2] = Insn::new(ALU | SETM | m_src(2) | fdest(0o15));
        prom[3] = Insn::new(ALU | SETM | m_src(1) | fdest(0o15));
        prom[4] = Insn::new(ALU | SETM | m_src(3) | LC);
        prom[5] = Insn::new(insn);
        prom[7] = Insn::new(ALU | SETM | src(0o1) | a_dest(0o203));
        prom[8] = Insn::new(ALU | SETM | src(0o13) | a_dest(0o213));
        prom[9] = here(9);
        prom[x as usize] = Insn::new(ALU | SETM | src(0o1) | a_dest(0o201));
        prom[x as usize + 1] = Insn::new(ALU | SETM | src(0o13) | a_dest(0o211));
        prom[x as usize + 2] = here(x as u64 + 2);
        let set = |m: &mut Machine| {
            m.mmem[1] = u64::from(x | (1 << 14));
            m.mmem[2] = u64::from(y);
            m.mmem[3] = 0o20 << 2;
            m.amem[0o60] = 1 << 16;
            m.amem[0o62] = (1 << 16) | (1 << 15);
        };
        let (e, r) = both(&prom, &set, 40);
        let got = |m: &Machine| {
            [m.amem[0o201], m.amem[0o211], m.amem[0o203], m.amem[0o213]].map(support::low)
        };
        assert_eq!(got(r.machine()), want, "{what}: rtl");
        assert_eq!(got(e.machine()), want, "{what}: micro");
        assert_eq!(e.machine().spcptr, r.machine().spcptr, "{what}: the pointer");
        assert_eq!(e.memory_cycles(), 0, "{what}: micro fetches nothing");
    }
}

/// **A read the map refuses leaves MD alone.** On the board a cycle starts
/// only under `VMAOK`: `MBUSY` is set from `MEMSTART AND VMAOK` on page
/// VCTL1, so nothing is requested and `MD` keeps its word, and `-VMAOK` in
/// the flags is how the microcode learns of the fault. With no map entry
/// every page is refused, so the read below fetches nothing on either engine.
#[test]
fn a_refused_read_leaves_md_alone() {
    let prom = [
        Insn::new(ALU | SETO | MD),
        Insn::new(ALU | SETZ | START_READ),
        filler(),
        filler(),
        filler(),
        Insn::new(ALU | SETM | SRC_MD | a_dest(0o200)),
    ];
    let (e, r) = both(&prom, &|_| {}, 40);
    assert_eq!(r.machine().amem[0o200], 0xffff_ffff, "rtl: MD kept its word");
    assert_eq!(e.machine().amem[0o200], 0xffff_ffff, "micro: MD kept its word");
}

/// **An instruction fetch is a memory cycle.** `IFETCH` is one of `MEMOP`'s
/// terms on page VCTL1, so the fetch the location counter asks for goes
/// through the map like any read: the latch at VMEMDR 1D14 takes the page's
/// map word, and the `-PFR` and `-PFW` bits of `MAP(MD)` --- functional
/// source 0o11, off that latch through the 74S240 at VMEMDR 1A01 --- show
/// the fetched page's permissions afterwards. Here a permitted read of page
/// 1 is followed by a fetch from page 2, which no map entry permits, and
/// the fault bit is up on both engines.
#[test]
fn an_instruction_fetch_goes_through_the_map() {
    let prom = [
        Insn::new(ALU | SETM | m_src(1) | LC),
        Insn::new(ALU | SETM | m_src(2) | START_READ),
        filler(),
        filler(),
        filler(),
        Insn::new(DISPATCH | d_addr(7) | (1 << 24)),
        filler(),
        filler(),
        Insn::new(ALU | SETM | src(0o11) | a_dest(0o200)),
    ];
    let set = |m: &mut Machine| {
        // LC counts bytes: the first word of page 2, and page 1's first word
        // for the read. Level 1 is all zeros, so page 1 is level-2 entry 1.
        m.mmem[1] = (2 << 8) << 2;
        m.mmem[2] = 1 << 8;
        m.l2_map[1] = (1 << 23) | (1 << 22) | 0o100;
        // The dispatch lands on word 8.
        m.dmem[7] = 8;
    };
    let (e, r) = both(&prom, &set, 40);
    let map = r.machine().amem[0o200];
    assert_ne!(map & (1 << 30), 0, "rtl: -PFR up, the fetched page refused");
    assert_eq!(e.machine().amem[0o200], map, "micro reads MAP(MD) as rtl does");
}

/// **A dispatch entry with R, P and N together falls through.** Page
/// CONTRL: `DFALL = DR AND DP` takes the return-and-push entry out of both
/// `PCS` selects, so the next address is `PC + 1`; nothing is pushed or
/// popped, and `N` still inhibits the instruction behind. That holds with
/// `IR<25>`, which only chooses `LPC` as the address a push would take.
#[test]
fn a_dispatch_to_a_return_and_push_entry_falls_through() {
    let prom = [
        filler(),
        Insn::new(DISPATCH | d_addr(7) | (1 << 25)),
        Insn::new(ALU | SETO | a_dest(0o201)),
        Insn::new(ALU | SETO | a_dest(0o202)),
        Insn::new(ALU | SETO | a_dest(0o203)),
    ];
    let set = |m: &mut Machine| m.dmem[7] = (1 << 16) | (1 << 15) | (1 << 14) | 0o100;
    let (e, r) = both(&prom, &set, 40);
    let got = |m: &Machine| [m.amem[0o201], m.amem[0o202], m.amem[0o203]];
    assert_eq!(
        got(r.machine()),
        [0, 0xffff_ffff, 0xffff_ffff],
        "rtl: the slot inhibited, then straight on"
    );
    assert_eq!(got(e.machine()), [0, 0xffff_ffff, 0xffff_ffff], "micro likewise");
}

/// **The POPJ bit wins over a taken jump.** Page CONTRL's next-address
/// select: `PCS0` is `NOT(POPJ OR ...)` and `PCS1` is low for a taken jump,
/// so with `IR<42>` up a taken JUMP without `R` still takes its next address
/// off the microcode stack and not from `IR<25:12>`.
#[test]
fn the_popj_bit_wins_over_a_taken_jump() {
    let prom = [
        filler(),
        Insn::new(JUMP | target(4) | ALWAYS | POPJ),
        filler(),
        filler(),
        Insn::new(ALU | SETO | a_dest(0o201)),
        filler(),
        Insn::new(ALU | SETO | a_dest(0o202)),
    ];
    let set = |m: &mut Machine| {
        m.spcptr = 1;
        m.spc[1] = 6;
    };
    let (e, r) = both(&prom, &set, 40);
    let got = |m: &Machine| (m.amem[0o201], m.amem[0o202]);
    assert_eq!(got(r.machine()), (0, 0xffff_ffff), "rtl pops to word 6");
    assert_eq!(got(e.machine()), (0, 0xffff_ffff), "micro pops to word 6");
}

/// **The functional sources MIT leaves unassigned read as all ones, and
/// `IR<30>` is not in the decode.** Page SOURCE decodes `IR<31>`, `IR<29>`
/// and `IR<28:26>` through two 74S138s, and the one for sources 10 to 17
/// has its outputs for 0o15, 0o16 and 0o17 unconnected: no part drives the
/// M bus for them, and an undriven TTL bus reads high. `IR<30>` reaches only
/// page PDLCTL, where it reads the PDL buffer by pointer rather than index,
/// so source 0o26 is source 6, the OPC. Microcode 323 reads 0o15 at 0o20535
/// and 0o26 at 0o20705.
#[test]
fn an_unassigned_functional_source_reads_all_ones() {
    // The OPC runs eight microcycles behind the PC on the board, so the
    // reads come after ten fillers, once it has moved off its power-on zero.
    let mut prom = vec![filler(); 10];
    prom.extend([
        Insn::new(ALU | SETM | src(0o15) | a_dest(0o201)),
        Insn::new(ALU | SETM | src(0o16) | a_dest(0o202)),
        Insn::new(ALU | SETM | src(0o17) | a_dest(0o203)),
        Insn::new(ALU | SETM | src(0o26) | a_dest(0o204)),
        Insn::new(ALU | SETM | src(0o6) | a_dest(0o205)),
    ]);
    let (e, r) = both(&prom, &|_| {}, 40);
    let ones = |m: &Machine| [m.amem[0o201], m.amem[0o202], m.amem[0o203]];
    assert_eq!(ones(r.machine()), [0xffff_ffff; 3], "rtl");
    assert_eq!(ones(e.machine()), [0xffff_ffff; 3], "micro");
    // The OPC moves with every microcycle, so the two reads of it are one
    // apart on each engine; the engines differ in how far behind the PC the
    // OPC runs, which is not what is checked here.
    for (name, m) in [("rtl", r.machine()), ("micro", e.machine())] {
        assert_ne!(m.amem[0o204], 0xffff_ffff, "{name}: source 26 is driven");
        assert_eq!(m.amem[0o205], m.amem[0o204] + 1, "{name}: 26 is the OPC, as 6 is");
    }
}

/// **A BYTE with function 0 deposits without rotating.** Page SMCTL: `MR`
/// is `IR<13>` and `SR` is `IR<12>` on a BYTE, so with both clear the
/// rotate is zero and the mask starts at bit 0; and the output bus carries
/// the mask network's word on every BYTE, `OSEL` being 0 --- the M source's
/// low `IR<9:5>` + 1 bits laid into the A source.
#[test]
fn a_byte_with_function_zero_deposits_without_rotating() {
    let prom = [Insn::new(BYTE | width(4) | m_src(1) | a_src(2) | a_dest(0o201))];
    let set = |m: &mut Machine| {
        m.mmem[1] = 0xdead_beef;
        m.amem[2] = 0x1111_1111;
    };
    let (e, r) = both(&prom, &set, 40);
    assert_eq!(r.machine().amem[0o201], 0x1111_111f, "rtl");
    assert_eq!(e.machine().amem[0o201], 0x1111_111f, "micro");
}

/// **The map write lands on the cycle after the store, in both engines.**
///
/// `mit/cadr/ir.bits`, on destination `23`, which MIT writes `VMA, MAP(MD)VMA`:
/// "The write actually occurs on the cycle following the store into
/// destination WRITE-MAP, and the VMA must not be disturbed during this cycle
/// for proper operation."
///
/// `rtl` has always had the delay --- `WMAPD` is registered at VCTL2 1C15 and
/// the write is one of `Rtl::write_phase`'s, a microcycle behind the level
/// that asked for it. `micro` wrote at the store, so the entry appeared to
/// the microcycle in between, which on the board still reads the old map.
///
/// Two instructions in the boot PROM, so this needs no pack: one to put the
/// virtual address in `MD`, one to store the map word through `VMA`. The
/// store is the microcycle `VMA` becomes the stored word; the write is the
/// microcycle the level-2 entry appears. They must be one apart.
#[test]
fn the_map_write_lands_the_cycle_after_the_store() {
    // ALU class, `SETA`, A source, and the two functional destinations.
    // `ir.bits`: the ALU function table is "shift left 3", so `SETA` is 5 at
    // `IR<8:3>`; `IR<13:12>` = 1 takes the ALU output; the functional
    // destination is `IR<23:19>` with `IR<25>` clear, and `IR<18:14>` is the
    // M address a functional write also lands in.
    let alu = 1u64 << 12;
    let seta = 5u64 << 3;
    let a_src = |a: u64| a << 32;
    let functional = |d: u64| (d << 19) | (0o37 << 14);

    // Virtual address `0o2000000`: `MD<23:13>` is `0o100`, the level-1 index,
    // and `MD<12:8>` is 0, so with `1` in the level-1 entry the level-2 index
    // is `1 << 5`. The word stored has `VMA<25>` set, which is the level-2
    // write enable, and `VMA<26>` clear, so the level-1 entry is left alone.
    const VIRTUAL: u32 = 0o2000000;
    const MAP_WORD: u32 = (1 << 25) | 0o12345;
    const L1: usize = 0o100;
    const L2: usize = 1 << 5;

    let machine = || {
        let mut m = Machine::new();
        m.l1_map[L1] = 1;
        m.amem[0o100] = u64::from(MAP_WORD);
        m.amem[0o101] = u64::from(VIRTUAL);
        m.load_prom(&[
            // ((MD) SETA A-MEM 101)
            Insn::new(alu | seta | a_src(0o101) | functional(0o30)),
            // ((VMA-WRITE-MAP) SETA A-MEM 100), MIT's `VMA, MAP(MD)VMA`
            Insn::new(alu | seta | a_src(0o100) | functional(0o23)),
            Insn::new(0),
            Insn::new(0),
            Insn::new(0),
            Insn::new(0),
        ]);
        m
    };

    /// The executed microcycle the store lands in, and the one the map entry
    /// appears in.
    fn watch(e: &mut dyn Engine) -> (usize, usize) {
        let (mut stored, mut written) = (None, None);
        let mut n = 0;
        while written.is_none() {
            assert!(n < 64, "the map entry never appeared");
            e.step().unwrap();
            n += 1;
            if stored.is_none() && e.machine().vma == MAP_WORD.into() {
                stored = Some(n);
            }
            if e.machine().l2_map[L2] == (MAP_WORD & 0o77777777) {
                written = Some(n);
            }
        }
        (stored.expect("VMA never took the map word"), written.unwrap())
    }

    let mut a = Micro::new(machine());
    a.boot();
    let mut b = Rtl::new(machine());
    b.boot();
    let (a_stored, a_written) = watch(&mut a);
    let (b_stored, b_written) = watch(&mut b);

    assert_eq!(a_written, a_stored + 1, "micro: one microcycle after the store");
    assert_eq!(b_written, b_stored + 1, "rtl: one microcycle after the store");
    assert_eq!(a.machine().l2_map, b.machine().l2_map, "and the same entry");
    assert_eq!(a.machine().l1_map, b.machine().l1_map, "with the level-1 map untouched");
    assert_eq!(a.machine().l1_map[L1], 1, "VMA<26> was clear");
}

/// **A map write whose store pops into a fetch still lands the store's
/// word.** The write takes its address and data from `VMA` and `MD` a
/// microcycle after the store, and `ir.bits` warns that "the VMA must not
/// be disturbed during this cycle".  Here the store is a `POPJ` to a stack
/// word with bit 14 up, `NEED-FETCH` pending, so an instruction fetch
/// follows and page VCTL1's `VMAS` multiplexer loads `VMA` with `LC<25:2>`
/// --- a microcycle after the `POPJ`, `NEXT INSTRD`, on both engines
/// since issue #21, which is the write's own microcycle.  The write still
/// takes the store's word because it lands at the head of that microcycle,
/// before the fetch moves `VMA`.  Both engines write the entry and both
/// end with the fetch address in `VMA`.
#[test]
fn a_map_write_whose_store_pops_into_a_fetch_still_lands() {
    const WRITE_MAP: u64 = (0o23 << 19) | (0o37 << 14);
    const VIRTUAL: u32 = 0o2000000;
    const MAP_WORD: u32 = (1 << 25) | 0o12345;
    const FETCH_WORD: u32 = 0o1000;
    const L2: usize = 1 << 5;
    let prom = [
        Insn::new(ALU | SETM | m_src(1) | MD),
        Insn::new(ALU | SETM | m_src(2) | LC),
        Insn::new(ALU | SETM | m_src(3) | WRITE_MAP | POPJ),
        filler(),
        filler(),
        filler(),
    ];
    let set = |m: &mut Machine| {
        m.mmem[1] = u64::from(VIRTUAL);
        // LC counts bytes.
        m.mmem[2] = u64::from(FETCH_WORD << 2);
        m.mmem[3] = u64::from(MAP_WORD);
        m.l1_map[VIRTUAL as usize >> 13] = 1;
        // The return address for the POPJ, with bit 14: fetch on the way out.
        m.spcptr = 1;
        m.spc[1] = 3 | (1 << 14);
    };
    let (e, r) = both(&prom, &set, 12);
    assert_eq!(r.machine().vma, FETCH_WORD.into(), "rtl: VMA is the fetch address by the end");
    assert_eq!(r.machine().l2_map[L2], MAP_WORD & 0o77777777, "rtl: the map word landed");
    assert_eq!(e.machine().vma, r.machine().vma, "micro: VMA");
    // The counter the fetch address came from, which each engine keeps in
    // its own place: `Engine::lc`, issue #32.
    assert_eq!(e.lc(), r.lc(), "micro: LC");
    assert_eq!(r.lc(), (FETCH_WORD << 2) + 2, "rtl: the counter stepped once, by a halfword");
    assert_eq!(e.machine().l2_map, r.machine().l2_map, "micro: the level-2 map");
    assert_eq!(e.machine().l1_map, r.machine().l1_map, "micro: the level-1 map");
}

/// Both engines' `VMA`, `MD` and `LC` after each of `steps` executed
/// instructions.
///
/// `LC` comes through [`Engine::lc`] and not out of [`Machine`], because
/// `rtl` keeps the location counter in a field of its own: issue #32,
/// which this helper's want of it is what found. It goes last in the line
/// so that a test looking for a fetch address can still read the front of
/// it.
///
/// Executed instructions and not microcycles, for the reason
/// `engines_agree_on_memory` gives: a cycle the pipeline inhibits runs on
/// the board and retires nothing, and the two engines do not agree about
/// how many of those there are at a boot. What they do agree about is the
/// machine after each instruction retires, so that is where they are
/// compared --- and it is still fine enough to see a fetch a microcycle
/// early, which lands in the retiring instruction before the one that
/// should carry it.
fn traces(prom: &[Insn], set: &dyn Fn(&mut Machine), steps: usize) -> Vec<(String, String)> {
    let make = || {
        let mut m = Machine::new();
        let mut words = vec![filler(); 512];
        words[..prom.len()].copy_from_slice(prom);
        m.load_prom(&words);
        set(&mut m);
        m
    };
    let show = |e: &dyn Engine| {
        let m = e.machine();
        format!("VMA {:o} MD {:o} LC {:o}", m.vma, m.md, e.lc())
    };
    let mut e = Micro::new(make());
    e.boot();
    let mut r = Rtl::new(make());
    r.boot();
    let mut out = Vec::new();
    for _ in 0..steps {
        while {
            e.step().unwrap();
            e.executed().is_none()
        } {}
        while {
            r.step().unwrap();
            r.executed().is_none()
        } {}
        out.push((show(&e), show(&r)));
    }
    out
}

/// **A `POPJ`'s instruction fetch comes the microcycle after the `POPJ`,
/// on both engines.**
///
/// `rtl` registers the popped word's bit 14 as `NEXT INSTRD` at the
/// `POPJ`'s clock edge and makes `IFETCH` from it in the microcycle after
/// --- `lcinc = next_instrd || (irdisp && IR<24>)`, `ifetch = needfetch &&
/// lcinc` --- so `VMA` takes `LC<25:2>` and the counter steps at the end
/// of that following microcycle, not the `POPJ`'s.  `micro` did it in the
/// `POPJ`'s own step, which put the fetch address in `VMA`, the stepped
/// counter in `LC` and the fetched word in `MD` a microcycle early for the
/// whole instruction stream.  Issue #21.
///
/// A `DISPATCH` with `IR<24>` fetches in its own microcycle on both, and
/// `an_instruction_fetch_goes_through_the_map` covers that route; only the
/// `POPJ` route is at issue here.
#[test]
fn a_popjs_fetch_comes_the_microcycle_after_the_popj() {
    const FETCH_WORD: u32 = 0o1000;
    let prom = [
        // The counter, which sets NEED-FETCH; then a POPJ to a stack word
        // with bit 14 up, so the return asks for the fetch.
        Insn::new(ALU | SETM | m_src(2) | LC),
        Insn::new(ALU | SETZ | POPJ | a_dest(0o100)),
        filler(),
        filler(),
        filler(),
        filler(),
    ];
    let set = |m: &mut Machine| {
        // LC counts bytes, and page 1 is mapped so the fetch is answered.
        m.mmem[2] = u64::from(FETCH_WORD << 2);
        m.l2_map[FETCH_WORD as usize >> 8] = (1 << 23) | (1 << 22) | 0o100;
        m.spcptr = 1;
        m.spc[1] = 4 | (1 << 14);
    };
    let t = traces(&prom, &set, 10);
    for (i, (micro, rtl)) in t.iter().enumerate() {
        assert_eq!(micro, rtl, "instruction {i}: micro {micro:?}, rtl {rtl:?}\nall: {t:#?}");
    }
    // And the fetch did happen, so the test is not passing on two engines
    // that both did nothing.
    assert!(
        t.iter().any(|(_, rtl)| rtl.starts_with(&format!("VMA {FETCH_WORD:o} "))),
        "the fetch address reached VMA: {t:#?}"
    );
}

/// **The fetch lands in the microcycle after the `POPJ` even when that
/// microcycle is inhibited.**
///
/// `IFETCH` is `NEEDFETCH AND LCINC` on page VCTL1 and `LCINC` is `NEXT
/// INSTRD`, a register --- nothing about it is decoded from `IR`, so a
/// microcycle the pipeline has nopped carries the fetch as any other
/// does. This is the common case and not a corner: `(JUMP R N)` is how
/// microcode 323 returns, and it both arms the fetch and inhibits the
/// microcycle that must carry it. Deferring the fetch without this stops
/// the band booting at all.
#[test]
fn a_fetch_armed_by_a_return_lands_in_the_inhibited_microcycle() {
    const FETCH_WORD: u32 = 0o1000;
    let prom = [
        Insn::new(ALU | SETM | m_src(2) | LC),
        // The return: pops, asks for the fetch, and inhibits the next.
        Insn::new(JUMP | R | N | ALWAYS),
        filler(),
        filler(),
        filler(),
        filler(),
    ];
    let set = |m: &mut Machine| {
        m.mmem[2] = u64::from(FETCH_WORD << 2);
        m.l2_map[FETCH_WORD as usize >> 8] = (1 << 23) | (1 << 22) | 0o100;
        m.spcptr = 1;
        m.spc[1] = 4 | (1 << 14);
    };
    let t = traces(&prom, &set, 8);
    for (i, (micro, rtl)) in t.iter().enumerate() {
        assert_eq!(micro, rtl, "instruction {i}: micro {micro:?}, rtl {rtl:?}\nall: {t:#?}");
    }
    assert!(
        t.iter().any(|(_, rtl)| rtl.starts_with(&format!("VMA {FETCH_WORD:o} "))),
        "the fetch address reached VMA: {t:#?}"
    );
}

/// **And it lands with the instruction it belongs to even when that
/// instruction costs extra microcycles.** A `WRITE-I-MEM` is two nopped
/// microcycles and then its own, and a fetch armed by the `POPJ` before
/// it must still arrive in the same executed instruction on both engines.
#[test]
fn a_fetch_armed_before_a_write_i_mem_lands_with_it() {
    const FETCH_WORD: u32 = 0o1000;
    let prom = [
        Insn::new(ALU | SETM | m_src(2) | LC),
        Insn::new(ALU | SETZ | POPJ | a_dest(0o100)),
        // A JUMP with both R and P is WRITE-I-MEM, which costs two nopped
        // microcycles before its own.
        Insn::new(JUMP | R | P | target(6) | ALWAYS),
        filler(),
        filler(),
        filler(),
        filler(),
        filler(),
    ];
    let set = |m: &mut Machine| {
        m.mmem[2] = u64::from(FETCH_WORD << 2);
        m.l2_map[FETCH_WORD as usize >> 8] = (1 << 23) | (1 << 22) | 0o100;
        m.spcptr = 1;
        m.spc[1] = 2 | (1 << 14);
    };
    let t = traces(&prom, &set, 8);
    for (i, (micro, rtl)) in t.iter().enumerate() {
        assert_eq!(micro, rtl, "instruction {i}: micro {micro:?}, rtl {rtl:?}\nall: {t:#?}");
    }
}

// --- corners the review of 22 September found micro alone got wrong ---------

use muir::isa::asm::{VMA, m_dest};

/// A functional destination, with the harmless M word 31 every one here
/// also writes.
const fn fdest(d: u64) -> u64 {
    (d << 19) | (0o37 << 14)
}

/// **The PDL buffer is written a microcycle late, and nothing passes the
/// word around.** `Rtl::write_phase` writes it under the registered
/// `PDLWRITED`, in the write phase of the *next* microcycle, after that
/// microcycle has read the buffer with `CLK` high; the A and M memories have
/// pass-around paths on pages ACTL and MCTL, and the PDL buffer has none. So
/// an instruction that reads the buffer right after a push gets the word
/// that was there. `pdl_read_right_after_a_push_on_the_board` holds `chip`
/// to the same.
#[test]
fn a_pdl_read_right_after_a_push_gets_the_old_word() {
    let prom = [
        Insn::new(ALU | SETZ | fdest(0o14)),
        Insn::new(ALU | SETO | fdest(0o11)),
        Insn::new(ALU | SETM | src(0o25) | a_dest(0o201)),
        Insn::new(ALU | SETM | src(0o25) | a_dest(0o202)),
    ];
    let (e, r) = both(&prom, &|_| {}, 40);
    let got = |m: &Machine| (m.amem[0o201], m.amem[0o202]);
    assert_eq!(got(r.machine()), (0, 0xffff_ffff), "rtl");
    assert_eq!(got(e.machine()), (0, 0xffff_ffff), "micro");
}

/// **The same for the SPC stack's M source.** The pointer moves at the edge
/// and the word is written in the next write phase; the pass-around,
/// `SPCWPASS`, feeds the next-address path and not `M`, as the comment in
/// `Rtl::read_phase` on page SPC says. So the instruction after a push reads
/// the stale word at the new pointer.
#[test]
fn an_spc_read_right_after_a_push_gets_the_old_word() {
    let prom = [
        Insn::new(ALU | SETO | fdest(0o15)),
        Insn::new(ALU | SETM | src(0o14) | m_dest(3)),
        Insn::new(ALU | SETZ | fdest(0o15)),
        Insn::new(ALU | SETM | src(0o1) | a_dest(0o201)),
        Insn::new(ALU | SETM | src(0o1) | a_dest(0o202)),
    ];
    let (e, r) = both(&prom, &|_| {}, 40);
    let got = |m: &Machine| (m.amem[0o201], m.amem[0o202]);
    let want = ((1 << 24) | 0o1777777, 1 << 24);
    assert_eq!(got(r.machine()), want, "rtl");
    assert_eq!(got(e.machine()), want, "micro");
}

/// **A return right after a push takes the pushed word**: that is what the
/// pass-around is for, and making the stack's write late must not lose it.
#[test]
fn a_return_right_after_a_push_takes_the_pushed_word() {
    let mut prom = vec![
        Insn::new(ALU | SETM | m_src(1) | fdest(0o15)),
        Insn::new(ALU | SETZ | POPJ),
        filler(),
        Insn::new(ALU | SETO | a_dest(0o201)),
    ];
    prom.resize(0o20, filler());
    prom[0o10] = Insn::new(ALU | SETO | a_dest(0o202));
    let (e, r) = both(&prom, &|m| m.mmem[1] = 0o10, 40);
    let got = |m: &Machine| (m.amem[0o201], m.amem[0o202]);
    assert_eq!(got(r.machine()), (0, 0xffff_ffff), "rtl returns to 10");
    assert_eq!(got(e.machine()), (0, 0xffff_ffff), "micro returns to 10");
}

/// **While a memory cycle starts, the map is addressed by `VMA`.** `MAPI`
/// is the 74S258s at VMAS 1C16 and 1C20, whose select is `-MEMSTART`: `VMA`
/// through the microcycle after a memory operation, `MD` otherwise. So
/// `MAP(MD)` read right after a start gives `VMA`'s page, and once the
/// cycle is under way `MD`'s.
#[test]
fn map_md_right_after_a_start_reads_the_vma_page() {
    let prom = [
        Insn::new(ALU | SETM | m_src(1) | MD),
        Insn::new(ALU | SETM | m_src(2) | START_READ),
        Insn::new(ALU | SETM | src(0o11) | a_dest(0o200)),
    ];
    let set = |m: &mut Machine| {
        m.mmem[1] = 1 << 8;
        m.mmem[2] = 2 << 8;
        m.l2_map[1] = (1 << 23) | (1 << 22) | 0o100;
        m.l2_map[2] = (1 << 23) | (1 << 22) | 0o200;
    };
    let (e, r) = both(&prom, &set, 40);
    let page = |m: &Machine| m.amem[0o200] & 0o37777;
    assert_eq!(page(r.machine()), 0o200, "rtl");
    assert_eq!(page(e.machine()), 0o200, "micro");
}

/// **A dispatch on a map bit right after a start takes `VMA`'s bit**, for
/// the same multiplexer.
#[test]
fn a_map_dispatch_right_after_a_start_takes_the_vma_page_bit() {
    let mut prom = vec![
        Insn::new(ALU | SETM | m_src(2) | MD),
        Insn::new(ALU | SETM | m_src(1) | START_READ),
        Insn::new(DISPATCH | d_addr(0o10) | (1 << 8)),
        filler(),
        filler(),
        filler(),
    ];
    prom.resize(0o60, filler());
    prom[0o50] = Insn::new(ALU | SETO | a_dest(0o210));
    prom[0o51] = Insn::new(ALU | SETO | a_dest(0o211));
    let set = |m: &mut Machine| {
        m.l2_map[1] = (1 << 23) | (1 << 22) | 0o100 | (1 << 18);
        m.l2_map[2] = 0o200;
        m.mmem[1] = 1 << 8;
        m.mmem[2] = 2 << 8;
        for i in 0o10..0o14 {
            m.dmem[i] = 0o40 + i as u32;
        }
    };
    let (e, r) = both(&prom, &set, 60);
    let got = |m: &Machine| (m.amem[0o210], m.amem[0o211]);
    assert_eq!(got(r.machine()), (0, 0xffff_ffff), "rtl goes to 51");
    assert_eq!(got(e.machine()), (0, 0xffff_ffff), "micro goes to 51");
}

/// **A jump's bit test takes the location counter's byte select** when
/// `IR<11:10>` is 3. Page SMCTL makes `SH4` and `SH3` for every class, and
/// the rotate a JUMP tests bit 0 of is the shifter's, `SR` being up
/// whenever the instruction is not a BYTE.
#[test]
fn a_jump_bit_test_takes_the_lc_rotate() {
    let mut prom = vec![
        Insn::new(ALU | SETM | m_src(2) | LC),
        filler(),
        filler(),
        Insn::new(JUMP | m_src(1) | (3 << 10) | N | target(0o40)),
        filler(),
        Insn::new(ALU | SETO | a_dest(0o201)),
    ];
    prom.resize(0o50, filler());
    prom[0o40] = Insn::new(ALU | SETO | a_dest(0o202));
    let set = |m: &mut Machine| {
        m.mmem[1] = 1 << 16;
        m.mmem[2] = 4;
    };
    let (e, r) = both(&prom, &set, 20);
    let got = |m: &Machine| (m.amem[0o201], m.amem[0o202]);
    assert_eq!(got(r.machine()), (0, 0xffff_ffff), "rtl takes it");
    assert_eq!(got(e.machine()), (0, 0xffff_ffff), "micro takes it");
}

/// **Functional source 6 is the OPC shift register's last stage**, eight
/// microcycles behind: page OPCD drives `MF<13:0>` from `OPC<13:0>`.
#[test]
fn the_opc_source_is_eight_microcycles_back() {
    let mut prom = vec![filler(); 10];
    prom.push(Insn::new(ALU | SETM | src(0o6) | a_dest(0o201)));
    let (e, r) = both(&prom, &|_| {}, 40);
    assert_eq!(r.machine().amem[0o201], 3, "rtl");
    assert_eq!(e.machine().amem[0o201], 3, "micro");
}

/// **The functional destinations MIT leaves unassigned decode as the
/// board decodes them.** Page SOURCE: 3 to 7 are the low group with no
/// decoder output, so only M is written; `IR<21>` is not in the memory
/// group's decode, so 24 to 27 are 20 to 23 and 34 to 37 are 30 to 33; and
/// `IR<24>`, "xx" in `ir.bits`, is in no decode at all.
#[test]
fn unassigned_destinations_decode_as_the_board_does() {
    for (d, what) in [
        (0o3, "nowhere"),
        (0o7, "nowhere"),
        (0o24, "VMA"),
        (0o34, "MD"),
        (0o20 | 0o40, "VMA, with IR<24>"),
    ] {
        let prom = [Insn::new(ALU | SETO | fdest(d)), filler(), filler()];
        let (e, r) = both(&prom, &|_| {}, 20);
        let got = |m: &Machine| (m.vma, m.md, m.mmem[0o37]);
        assert_eq!(got(e.machine()), got(r.machine()), "destination {d:o}, {what}");
    }
}

/// `chip`, on the PDL case: the netlist gives the old word too.
#[test]
fn pdl_read_right_after_a_push_on_the_board() {
    use muir::benchmark::{self, Program, Stop};
    let mut prom = vec![
        Insn::new(ALU | SETZ | fdest(0o11)),
        Insn::new(ALU | SETZ | fdest(0o14)),
        Insn::new(ALU | SETO | fdest(0o11)),
        Insn::new(ALU | SETM | src(0o25) | VMA),
        Insn::new(JUMP | target(4) | ALWAYS),
    ];
    prom.resize(512, filler());
    let p = Program { name: "pdl", prom, top: 4, cycles_per_iteration: 1 };
    let n = support::netlists();
    let (mut c, mut clk, mut far) = support::chip(&n);
    benchmark::boot_chip(&mut c, &n.cpu, &mut far, &mut clk, &p);
    let got = benchmark::run_chip(&mut c, &n.cpu, &mut far, &mut clk, &p, Stop::Cycles(4));
    assert_eq!(got.iterations, 0);
}

/// **The trap cycle after a boot is long when `IR` asks, on both.**
/// `-ILONG` is `NAND(IR45, -NOPA)` at FLAG 3E07, and the trap is in `NOP`
/// but not in `NOPA` (CONTRL 3E14, 3E23): the cycle is nopped and still
/// long. `the_trap_cycle_is_long_and_counted_when_ir_asks` in
/// `tests/rtl.rs` has `rtl`'s half; this holds `micro`'s clock to it.
#[test]
fn the_trap_cycle_is_long_on_both() {
    let long = Insn::new(filler().raw() | 1 << 45);
    let prom = vec![long; 512];
    let (mut e, mut r) = both(&prom, &|_| {}, 10);
    for (name, eng) in [("micro", &mut e as &mut dyn Engine), ("rtl", &mut r as &mut dyn Engine)] {
        // Normal speed, through the two synchronizer stages, and then the
        // boot, whose reset clears the mode register but not the stages.
        eng.machine_mut().mode.speed1 = true;
        for _ in 0..4 {
            eng.step().unwrap();
        }
        eng.boot();
        eng.machine_mut().mode.speed1 = true;
        let ns = eng.machine().ns;
        eng.step().unwrap();
        assert_eq!(eng.machine().ns - ns, 185, "{name}: the trap cycle");
    }
}

// --- WRITE-I-MEM of the word after it, and in a delay slot -------------------

/// Where the WRITE-I-MEM programs sit, above the CADR's PROM, and their
/// stop.
const WIM_AT: u64 = 0o4000;
const WIM_STOP: u64 = WIM_AT + 0o1000;

/// `micro` and `rtl` on `geometry`, the control store holding `words` at
/// [`WIM_AT`] and fillers behind, the PROM jumping there; A 41 is 1 and A
/// 100 and 101 the word a write writes, `w`. Each run to the stop, and its
/// M 21, 22 and 23, its SPC pointer, and the machine's time less `rtl`'s
/// stalls.
fn wim_both(geometry: muir::machine::Geometry, words: &[u64], w: u64) -> [([u64; 3], u8, u64); 2] {
    let make = || {
        let mut m = Machine::with_geometry(geometry, 1);
        for k in 0..0o2000usize {
            m.imem[WIM_AT as usize + k] = filler();
        }
        for (k, &x) in words.iter().enumerate() {
            m.imem[WIM_AT as usize + k] = Insn::extended(x);
        }
        m.imem[WIM_STOP as usize] = Insn::new(JUMP | target(WIM_STOP) | ALWAYS | N);
        m.load_prom(&[Insn::new(JUMP | target(WIM_AT) | ALWAYS | N), filler(), filler()]);
        m.amem[0o41] = 1;
        m.amem[0o100] = w & 0xffff_ffff;
        m.amem[0o101] = w >> 32;
        m
    };
    fn run<E: Engine>(mut e: E, stalled: &dyn Fn(&E) -> u64) -> ([u64; 3], u8, u64) {
        e.boot();
        let mut k = 0;
        while e.machine().opc != WIM_STOP as u16 {
            e.step().unwrap();
            k += 1;
            assert!(k < 20_000, "the program never reached its stop");
        }
        let m = e.machine();
        ([m.mmem[0o21], m.mmem[0o22], m.mmem[0o23]], m.spcptr, m.ns - stalled(&e))
    }
    [run(Micro::new(make()), &|_| 0), run(Rtl::new(make()), &|r: &Rtl| r.stalled_ns())]
}

/// The WRITE-I-MEM word: MIT's form, `IR<9:0>` 1647 (`cadsym.lisp`: R, P,
/// and `JUMP-OP`'s N), or CC's, without N; writing A 101 and M 20 to `at`.
fn wim(n: bool, at: u64) -> u64 {
    JUMP | P | R | ALWAYS | target(at) | a_src(0o101) | m_src(0o20) | if n { N } else { 0 }
}

/// **A WRITE-I-MEM of the word after it** (WRITE-I-MEM ruling): in MIT's
/// form, with N, the board nops the word it fetched during the write and
/// `IWRITED`'s POPJ returns to it, fetched again from the store after the
/// write, so the new word runs; in CC's form, without N, the word fetched
/// before the write runs as the slot, the old word. On the CADR and on
/// revision 14, `micro` as `rtl`, the netlist's engine. **Fails** a `micro`
/// that runs the word it prefetched after MIT's form.
#[test]
fn a_write_i_mem_of_the_word_after_it_runs_as_the_board_runs_it() {
    use muir::isa::asm::ADD;
    use muir::machine::Geometry;
    let inc = |m: u64| ALU | ADD | a_src(0o41) | m_src(m) | m_dest(m);
    let set = ALU | muir::isa::asm::SETA | a_src(0o100) | m_dest(0o20);
    let stop = JUMP | ALWAYS | N | target(WIM_STOP);
    let fil = filler().raw();
    for g in [Geometry::CADR, Geometry::QUUX_14] {
        for (n, new) in [(true, 1), (false, 0)] {
            let words = [set, wim(n, WIM_AT + 2), fil, fil, stop, fil];
            let [u, r] = wim_both(g, &words, inc(0o22));
            let what = format!("{:?}, N {n}", g.revision());
            assert_eq!((u.0, u.1), (r.0, r.1), "{what}: micro as rtl");
            assert_eq!(r.0[1], new, "{what}: the new word runs with N, the old without");
        }
    }
}

/// **A WRITE-I-MEM in a jump's delay slot goes on at the jump's target**
/// (WRITE-I-MEM ruling): MIT's form pushes the target under N and returns
/// to it, fetched again after the write; a write of the target runs its new
/// word. On the CADR and revision 14, `micro` as `rtl`. **Fails** a
/// `micro` that runs the target's word as it prefetched it.
#[test]
fn a_write_i_mem_in_a_delay_slot_goes_on_at_the_jump_s_target() {
    use muir::isa::asm::ADD;
    use muir::machine::Geometry;
    let inc = |m: u64| ALU | ADD | a_src(0o41) | m_src(m) | m_dest(m);
    let set = ALU | muir::isa::asm::SETA | a_src(0o100) | m_dest(0o20);
    let stop = JUMP | ALWAYS | N | target(WIM_STOP);
    let fil = filler().raw();
    for g in [Geometry::CADR, Geometry::QUUX_14] {
        // Another word written, then the target itself.
        for (at, want) in [(WIM_AT + 0o20, [0, 0, 1]), (WIM_AT + 0o10, [0, 1, 0])] {
            let mut words = vec![
                set,
                JUMP | ALWAYS | target(WIM_AT + 0o10),
                wim(true, at),
                inc(0o21),
                stop,
                fil,
            ];
            words.resize(0o10, fil);
            words.extend([inc(0o23), stop, fil]);
            let [u, r] = wim_both(g, &words, inc(0o22));
            let what = format!("{:?}, writing {at:o}", g.revision());
            assert_eq!((u.0, u.1), (r.0, r.1), "{what}: micro as rtl");
            assert_eq!(r.0, want, "{what}: M 21, 22, 23");
        }
    }
}

/// **A WRITE-I-MEM costs what it costs on the board** (WRITE-I-MEM ruling,
/// W5): two nopped microcycles in MIT's form, `IWRITED`'s N after its own,
/// and one in CC's, without N. Against the same program without the
/// write, `micro`'s clock moves as `rtl`'s less its stalls, on the CADR and
/// revision 14.
#[test]
fn a_write_i_mem_costs_its_nopped_microcycles_on_micro_as_on_the_board() {
    use muir::machine::Geometry;
    let set = ALU | muir::isa::asm::SETA | a_src(0o100) | m_dest(0o20);
    let stop = JUMP | ALWAYS | N | target(WIM_STOP);
    let fil = filler().raw();
    for g in [Geometry::CADR, Geometry::QUUX_14] {
        let base = wim_both(g, &[set, fil, fil, fil, stop, fil], fil);
        for n in [true, false] {
            let with = wim_both(g, &[set, wim(n, WIM_AT + 0o20), fil, fil, stop, fil], fil);
            let (du, dr) = (with[0].2 - base[0].2, with[1].2 - base[1].2);
            assert_eq!(du, dr, "{:?}, N {n}: micro +{du} ns, rtl +{dr}", g.revision());
        }
    }
}
