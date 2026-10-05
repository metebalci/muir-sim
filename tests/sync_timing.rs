// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! H1a: QUUX's synchronous microcycle, `--timing-model sync`.
//!
//! On the CADR a microcycle is the delay line's read tap and a 60 ns
//! restart, 145 ns at normal speed; muir-fpga's fabric replays that as 15
//! ticks of its 10 ns clock. Under `sync` a QUUX microcycle is a fixed
//! number of those ticks, `--sync-cycle-ticks`, which a board's fit proves
//! its longest path settles in, and an `ILONG` instruction takes
//! `ilong_ticks` more. Every register is clocked at the one edge as before
//! and the bus keeps its own time, so nothing the microcode sees changes
//! but how long a microcycle takes.

use muir::clock::TimingModel;
use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{ALU, SETZ, filler, m_dest};
use muir::machine::{Geometry, Machine};
use muir::rtl::Rtl;

mod support;

const fn sync(cycle_ticks: u8, ilong_ticks: u8) -> TimingModel {
    TimingModel::Sync { cycle_ticks, ilong_ticks }
}

/// QUUX on `model`, running `prom` from the boot.
fn quux(model: TimingModel, prom: &[Insn]) -> Rtl {
    let mut m = Machine::new();
    let mut words = vec![filler(); 512];
    words[..prom.len()].copy_from_slice(prom);
    m.load_prom(&words);
    m.geometry = Geometry::QUUX;
    support::prom_program_in_ram(&mut m);
    let mut e = Rtl::new(m);
    e.set_timing_model(model);
    e.boot();
    e
}

/// The time of `n` microcycles from the fifth.
fn time_of(e: &mut Rtl, n: usize) -> u64 {
    for _ in 0..4 {
        e.step().unwrap();
    }
    let from = e.ns();
    for _ in 0..n {
        e.step().unwrap();
    }
    e.ns() - from
}

/// **A microcycle is `cycle_ticks` ticks**, from the boot on: 40 ns at 4,
/// 30 at 3. QUUX drops the delay lines: an engine made for a QUUX machine
/// is on four ticks without being told, and the CADR's timings are not
/// QUUX's to take.
#[test]
fn a_microcycle_is_its_ticks() {
    for (model, ns) in [(sync(4, 0), 40), (sync(3, 0), 30)] {
        let mut e = quux(model, &[]);
        assert_eq!(time_of(&mut e, 8), 8 * ns, "{model:?}");
    }
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
    let e = Rtl::new(m);
    assert_eq!(e.timing_model(), sync(4, 0), "the default");
    for cadr in [TimingModel::Cadr, TimingModel::Fpga] {
        let refused = std::panic::catch_unwind(|| {
            let mut m = Machine::new();
            m.geometry = Geometry::QUUX;
            Rtl::new(m).set_timing_model(cadr);
        });
        assert!(refused.is_err(), "{cadr:?} refused on QUUX");
    }
}

/// **`ILONG` adds `ilong_ticks`**: an instruction with `IR<45>` takes 4 + 1
/// ticks at `sync(4, 1)`, and no more than any other at `sync(4, 0)`.
#[test]
fn ilong_adds_its_ticks() {
    let long = Insn::new(ALU | SETZ | m_dest(1) | 1 << 45);
    let prom = vec![long; 64];
    for (model, ns) in [(sync(4, 0), 40), (sync(4, 1), 50)] {
        let mut e = quux(model, &prom);
        assert_eq!(time_of(&mut e, 8), 8 * ns, "{model:?}");
    }
}

/// **Every instant is on the grid**, stalls and bus included, as under
/// `fpga`: the fabric has nothing between its ticks.
#[test]
fn every_instant_is_on_the_grid() {
    let mut e = quux(sync(4, 0), &[]);
    for _ in 0..2_000 {
        e.step().unwrap();
        assert_eq!(e.ns() % 10, 0, "{} ns", e.ns());
    }
}

/// **A checkpoint keeps the model with its ticks**, and a resumed run goes
/// on at the same rate.
#[test]
fn a_checkpoint_keeps_the_ticks() {
    use muir::checkpoint::{Reader, Writer};
    let mut e = quux(sync(3, 2), &[]);
    time_of(&mut e, 10);
    let mut w = Writer::new();
    e.save(&mut w);
    let body = w.finish();
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
    let mut back = Rtl::new(m);
    back.load(&mut Reader::for_word_bits(&body, 40)).unwrap();
    assert_eq!(back.timing_model(), sync(3, 2));
    assert_eq!(time_of(&mut back, 8), 8 * 30);
}

/// **The divider's hold is counted in `sync`'s microcycles**: nine, of 40
/// ns at four ticks and of 30 at three, against an ordinary instruction in
/// the same place. QUUX's rule counts microcycles,
/// not nanoseconds, and leaves a change of the microcycle's length to the
/// contract that makes it.
#[test]
fn a_div_is_held_in_sync_cycles() {
    let div = Insn::new(ALU | (0o43 << 3) | m_dest(1));
    let plain = Insn::new(ALU | SETZ | m_dest(1));
    for (ticks, ns) in [(4, 40), (3, 30)] {
        let time = |i: Insn| {
            let mut prom = vec![filler(); 8];
            prom[6] = i;
            let mut e = quux(sync(ticks, 0), &prom);
            for _ in 0..12 {
                e.step().unwrap();
            }
            e.ns()
        };
        assert_eq!(time(div) - time(plain), muir::muldiv::DIV_CYCLES * ns, "{ticks} ticks");
    }
}

/// **A write lands no earlier than it is answered.** Under the CADR's
/// timing `rtl` carries a pending write to `SPEEDCLK`, 60 ns into the cycle,
/// for the speed synchronizer; a `sync` microcycle is shorter than that and
/// has no synchronizer, so a write lands at the edge. Here the program
/// writes the video controller's first buffer word over and over, and each time it
/// changes the machine's clock has reached the answer.
#[test]
fn a_write_lands_no_earlier_than_its_answer() {
    use muir::isa::asm::{ALU, ALWAYS, CARRY_IN, JUMP, M_PLUS_C, MD, START_WRITE, m_src, target};
    let prom = [
        Insn::new(ALU | M_PLUS_C | CARRY_IN | m_src(2) | m_dest(2)),
        Insn::new(ALU | muir::isa::asm::SETM | m_src(2) | MD),
        Insn::new(ALU | muir::isa::asm::SETM | m_src(1) | START_WRITE),
        Insn::new(JUMP | target(0) | ALWAYS),
    ];
    let mut m = Machine::new();
    let mut words = vec![filler(); 512];
    words[..prom.len()].copy_from_slice(&prom);
    m.load_prom(&words);
    m.geometry = Geometry::QUUX;
    support::prom_program_in_ram(&mut m);
    m.tv.set_board(muir::tv::Board::Video);
    m.mmem[1] = support::quux_map(&mut m, 1, muir::machine::WINDOW_13).into();
    let mut e = Rtl::new(m);
    e.set_timing_model(sync(4, 0));
    e.boot();
    let mut landed = 0;
    for _ in 0..3_000 {
        let before = e.machine().tv.read_buffer(0);
        let answer = e.bus_answered_at();
        e.step().unwrap();
        if e.machine().tv.read_buffer(0) != before {
            landed += 1;
            let at = answer.expect("a write landed with none answered");
            assert!(e.ns() >= at, "landed by {} ns, answered at {at}", e.ns());
        }
    }
    assert!(landed > 20, "{landed} writes landed");
}
