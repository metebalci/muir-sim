// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `--timing-model`: `rtl` on the CADR's own timing, `cadr`, or on the
//! 10 ns grid muir-fpga's fabric runs on, `fpga`, so that the references
//! muir-fpga takes from `rtl` come out as its fabric runs.
//!
//! The grid's two rules are muir-fpga's. A delay that something starts is
//! rounded up on its own, from its own start, to the next tick. A clock
//! that runs freely from power-on keeps its phase exactly, and each of its
//! edges is acted on at the first tick at or after it, so rounding never
//! drifts it. The microcycle is the read phase's tap rounded up, the
//! `ILONG` taps as one sum, and the restart rounded up on its own.

use muir::checkpoint::{Reader, Writer};
use muir::clock::{Speed, TimingModel};
use muir::engine::Engine;
use muir::isa::asm::filler;
use muir::machine::Machine;
use muir::rtl::Rtl;
use muir::spy;

const TAPS: [(Speed, bool); 8] = [
    (Speed::Fast, false),
    (Speed::Fast, true),
    (Speed::Normal, false),
    (Speed::Normal, true),
    (Speed::Slow, false),
    (Speed::Slow, true),
    (Speed::ExtraSlow, false),
    (Speed::ExtraSlow, true),
];

/// **`cadr` is the board.** Its microcycle is `Speed::cycle_ns`, the
/// delay-line tap and the sixty nanoseconds of restart, which
/// `tests/clock.rs` holds to the generator.
#[test]
fn cadr_is_the_tap_table() {
    assert_eq!(TimingModel::default(), TimingModel::Cadr);
    for (speed, ilong) in TAPS {
        assert_eq!(
            TimingModel::Cadr.cycle_ns(speed, ilong),
            speed.cycle_ns(ilong),
            "{speed:?} ilong={ilong}"
        );
    }
}

/// **`fpga` rounds the tap and the restart up, each on its own.** Fast is
/// 80 + 60 and normal 90 + 60; the `ILONG` taps round the sum, 75 + 40 to
/// 120 and 85 + 40 to 130; slow and extra slow are on the grid already.
#[test]
fn fpga_rounds_the_tap_and_the_restart_each_up() {
    let want = [
        (Speed::Fast, false, 140),
        (Speed::Fast, true, 180),
        (Speed::Normal, false, 150),
        (Speed::Normal, true, 190),
        (Speed::Slow, false, 160),
        (Speed::Slow, true, 200),
        (Speed::ExtraSlow, false, 220),
        (Speed::ExtraSlow, true, 220),
    ];
    for (speed, ilong, ns) in want {
        assert_eq!(TimingModel::Fpga.cycle_ns(speed, ilong), ns, "{speed:?} ilong={ilong}");
    }
}

/// **The two rules, on numbers the machine has.** Triggered: the
/// microsecond counter's low half answering 313 ns after its edge, the
/// receive buffer's 33 ns of setup, the refresh one-shot. Free-running:
/// the timeout clock's edges at 425 ns intervals, `FCLK^`'s at 125, the
/// half-microsecond clock's phase at 203. `cadr` moves none of them.
#[test]
fn fpga_puts_triggered_delays_and_free_running_edges_on_the_next_tick() {
    let f = TimingModel::Fpga;
    assert_eq!(f.triggered(313), 320);
    assert_eq!(f.triggered(33), 40);
    assert_eq!(f.triggered(12_033), 12_040);
    assert_eq!(f.triggered(60), 60, "a delay on the grid stays");

    assert_eq!(f.free_running(425), 430);
    assert_eq!(f.free_running(850), 850);
    assert_eq!(f.free_running(1_275), 1_280);
    assert_eq!(f.free_running(125), 130);
    assert_eq!(f.free_running(250), 250);
    assert_eq!(f.free_running(203), 210);

    for ns in [0, 5, 33, 125, 203, 313, 425, 12_033] {
        assert_eq!(TimingModel::Cadr.triggered(ns), ns);
        assert_eq!(TimingModel::Cadr.free_running(ns), ns);
    }
}

/// A machine that runs straight on, in the model given, at the speed the
/// mode register's two low bits name.
fn straight_line(model: TimingModel, speed_bits: u16) -> Rtl {
    let mut m = Machine::new();
    m.load_prom(&vec![filler(); 512]);
    let mut e = Rtl::new(m);
    e.set_timing_model(model);
    e.boot();
    e.spy_write(spy::MODE, speed_bits);
    // Past the synchronizer's stages, whichever cycle the write loaded in.
    for _ in 0..4 {
        e.step().unwrap();
    }
    e
}

/// **`rtl` runs its microcycles on the model's time.** Eight at normal
/// speed: 145 ns each on the board, 150 on the grid.
#[test]
fn rtl_runs_a_normal_microcycle_on_the_models_time() {
    for (model, ns) in [(TimingModel::Cadr, 145), (TimingModel::Fpga, 150)] {
        let mut e = straight_line(model, 2);
        let from = e.ns();
        for _ in 0..8 {
            e.step().unwrap();
        }
        assert_eq!(e.ns() - from, 8 * ns, "{model:?}");
    }
}

/// **Under `fpga` every instant `rtl` reaches is on the grid,** at every
/// speed, from power-on. A delay or an edge that reached the processor's
/// time without its rule would put it between ticks.
#[test]
fn every_instant_rtl_reaches_under_fpga_is_on_the_grid() {
    for bits in [3, 2, 1, 0] {
        let mut e = straight_line(TimingModel::Fpga, bits);
        for _ in 0..2_000 {
            e.step().unwrap();
            assert_eq!(e.ns() % 10, 0, "speed bits {bits}: {} ns", e.ns());
        }
    }
}

/// **A checkpoint keeps the model it was run in,** so a run resumed is
/// resumed on the time it left.
#[test]
fn a_checkpoint_keeps_the_timing_model() {
    for model in [TimingModel::Cadr, TimingModel::Fpga] {
        let e = straight_line(model, 2);
        let mut w = Writer::new();
        e.save(&mut w);
        let body = w.finish();

        let mut m = Machine::new();
        m.load_prom(&vec![filler(); 512]);
        let mut back = Rtl::new(m);
        let mut r = Reader::new(&body);
        back.load(&mut r).unwrap();
        r.done().unwrap();
        assert_eq!(back.timing_model(), model);
        assert_eq!(back.busint().unwrap().timing_model(), model, "and the bus interface with it");
        assert_eq!(back.machine().disk.timing_model(), model, "and the disk controller");
        assert_eq!(back.ns(), e.ns());
    }
}

/// The model's names on the command line.
#[test]
fn the_timing_model_names_itself() {
    assert_eq!(TimingModel::Cadr.name(), "cadr");
    assert_eq!(TimingModel::Fpga.name(), "fpga");
    assert_eq!(TimingModel::parse("cadr"), Some(TimingModel::Cadr));
    assert_eq!(TimingModel::parse("fpga"), Some(TimingModel::Fpga));
    assert_eq!(TimingModel::parse("fast"), None);
}

/// A machine that reads the word at `word` of physical `page` a dozen
/// times, each read left forty fillers to be answered in, so the dozen
/// fall at a dozen phases of the clocks the answer waits for.
fn reading(page: u32, word: u32) -> Machine {
    use muir::isa::Insn;
    use muir::isa::asm::{ALU, SETM, SRC_MD, START_READ, a_dest, a_src, m_src};
    let mut m = Machine::new();
    m.l1_map[0] = 0;
    m.l2_map[0] = (1 << 23) | page;
    m.mmem[1] = u64::from(word);
    let mut prom = vec![filler(); 512];
    for k in 0..12 {
        prom[k * 42] = Insn::new(ALU | SETM | m_src(1) | a_src(3) | START_READ);
        prom[k * 42 + 41] = Insn::new(ALU | SETM | SRC_MD | a_src(3) | a_dest(0o101));
    }
    m.load_prom(&prom);
    m
}

/// **What the bus interface answers with is on the grid under `fpga`.**
/// The instants it gives a cycle --- the acknowledgement, the strobe that
/// loads `MD`, the answer --- and the memory board's next refresh, watched
/// after every microcycle. On the board each lands between ticks somewhere
/// in the dozen reads: the microsecond counter's low half 313 ns after its
/// clock's edge, the receive buffer on an `FCLK^` edge, the serial port on
/// the half-microsecond clock's, a read of nothing on the timeout clock's,
/// and main memory on its board's crystal. That is what shows this test can
/// fail; under `fpga` none does.
#[test]
fn the_bus_interfaces_answers_land_on_the_grid_under_fpga() {
    let reads = [
        ("the microsecond counter's low half", 0o37764, 0o50, 0),
        ("the Chaosnet receive buffer", 0o37764, 0o62, 0),
        ("the serial port", 0o37764, 0o70, 0),
        ("an address nothing answers", 0o37760, 0, 0),
        ("main memory at fast speed", 0o100, 0, 3),
    ];
    for (what, page, word, speed_bits) in reads {
        for model in [TimingModel::Cadr, TimingModel::Fpga] {
            let mut e = Rtl::new(reading(page, word));
            e.set_timing_model(model);
            e.boot();
            e.spy_write(spy::MODE, speed_bits);
            let (mut seen, mut off_grid) = (0, Vec::new());
            for _ in 0..512 {
                e.step().unwrap();
                let b = e.busint().unwrap();
                // `next_change` is an instant and one: the edge that changes
                // the board comes strictly after it.
                let refresh = b.memory_boards().first().map(|m| m.next_change().saturating_sub(1));
                for at in
                    [b.ack_at(), b.loadmd_at(), b.answered_at(), refresh].into_iter().flatten()
                {
                    if at >= u64::MAX - 1 {
                        continue;
                    }
                    seen += 1;
                    if at % 10 != 0 {
                        off_grid.push(at);
                    }
                }
            }
            assert!(e.bus_cycles() >= 12, "{what}, {model:?}: {} reads", e.bus_cycles());
            assert!(seen > 0, "{what}, {model:?}: no instant to look at");
            match model {
                TimingModel::Cadr => {
                    assert!(!off_grid.is_empty(), "{what}: never off the grid on the board")
                }
                TimingModel::Fpga => {
                    let first = &off_grid[..off_grid.len().min(8)];
                    assert!(off_grid.is_empty(), "{what}: off the grid under fpga at {first:?}")
                }
            }
        }
    }
}

/// **The I/O board's answers, to the nanosecond, on both.** `-MSYN` on a
/// tick, and each answer on the board's own clocks:
///
/// - the microsecond counter's low half: the clock's edge after 1,000 is
///   1,890, then 313 ns, rounded to 320 on the grid;
/// - the serial port: the half-microsecond clock's edge after 1,000 is at
///   1,203, taken at 1,210 on the grid, then 750;
/// - the receive buffer, with `-MSYN` at 1,090: its 33 ns of setup reach
///   1,123 on the board and the first `FCLK^` at or after that is 1,125. On
///   the grid the setup is 40 and reaches 1,130; the edge at 1,125 is taken
///   at the tick of 1,130, which meets it, so the answer is that edge's and
///   not the next one's 125 ns later.
#[test]
fn the_io_boards_answers_to_the_nanosecond() {
    use muir::busint::IoBoardTiming;
    use muir::chaos::interface::READ_BUFFER;
    use muir::ioboard::{SERIAL_FIRST, USEC_LOW};
    let cadr = IoBoardTiming::with_timing_model(TimingModel::Cadr);
    let fpga = IoBoardTiming::with_timing_model(TimingModel::Fpga);
    assert_eq!(cadr.answer(USEC_LOW, false, 1_000), 1_890 + 313);
    assert_eq!(fpga.answer(USEC_LOW, false, 1_000), 1_890 + 320);
    assert_eq!(cadr.answer(SERIAL_FIRST, false, 1_000), 1_203 + 750);
    assert_eq!(fpga.answer(SERIAL_FIRST, false, 1_000), 1_210 + 750);
    assert_eq!(cadr.answer(READ_BUFFER, false, 1_090), 1_125 + 250);
    assert_eq!(fpga.answer(READ_BUFFER, false, 1_090), 1_130 + 250);
}

/// **A seek on the grid is done, and attends, on the tick its countdown
/// reaches.** muir-fpga's controller counts a span down by the tick, so a
/// span that is not a whole number of ticks ends at the next one. Two
/// cylinders is 6,060,271 ns: on the board the controller is busy until
/// then and the attention comes then; on the grid both wait for 6,060,280.
/// The register offsets and bits are MIT's, as `tests/disk.rs` names them.
#[test]
fn a_seek_ends_on_the_tick_its_countdown_reaches_under_fpga() {
    use muir::disk_controller::Controller;
    use muir::disk_unit::{Geometry, Unit, seek_ns};
    const COMMAND: u32 = 0;
    const DISK_ADDRESS: u32 = 2;
    const START: u32 = 3;
    const NOT_ACTIVE: u32 = 1;
    const ATTENTION: u32 = 1 << 2;

    let span = seek_ns(2);
    assert_eq!(span, 6_060_271, "not a whole number of ticks");
    for (model, done) in [(TimingModel::Cadr, span), (TimingModel::Fpga, 6_060_280)] {
        let mut d = Controller::default();
        d.set_timing_model(model);
        d.attach(0, Unit::blank(Geometry::T300));
        d.timed = true;
        let mut main = vec![0u32; 1 << 16];
        let now = 1_000;
        d.advance(now);
        d.write(COMMAND, 0o4, &mut main);
        d.write(DISK_ADDRESS, 2 << 16, &mut main);
        d.write(START, 0, &mut main);
        d.advance(now + done - 1);
        assert_eq!(d.status() & (NOT_ACTIVE | ATTENTION), 0, "{model:?}: busy a ns before");
        d.advance(now + done);
        assert_eq!(
            d.status() & (NOT_ACTIVE | ATTENTION),
            NOT_ACTIVE | ATTENTION,
            "{model:?}: at {done}"
        );
    }
}
