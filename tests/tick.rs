// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's clocks: the tick, timer 0 of the interval timers on the register
//! page (contract Q11, `tests/interval_timers.rs`) as the machine's
//! 60-cycle clock in place of the display's vertical interrupt; and the
//! microsecond clock in the processor (contract Q1), functional source 15.
//!
//! The tick is reached through the page alone: word 110 its control, word
//! 111 its period, which the boot PROM writes. Source 15 reads the
//! microseconds since power-on, 32 bits, wrapping. Q1's destinations 3 and
//! 4 write only M and its source 17 reads all ones, on QUUX as on the
//! CADR, where source 15 too reads all ones.

use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{ALU, ALWAYS, JUMP, N, SETM, bit, filler, m_dest, m_src, src, target};
use muir::machine::{Geometry, Machine};
use muir::micro::Micro;
use muir::rtl::Rtl;

mod support;

/// Functional destinations 3 and 4, M 37 written too.
const CLOCK_CONTROL: u64 = (3 << 19) | (0o37 << 14);
/// Q1's interval period, destination 4, which on QUUX writes only M, as on
/// the CADR, and as destination 3 does.
const INTERVAL_PERIOD: u64 = (4 << 19) | (0o37 << 14);

/// Q1's destination 3 words: the tick on; on and its flag cleared.
const TICK_ON: u32 = 1;
const TICK_CLEAR: u32 = 3;

/// Writes the interval period, M 1, then the control word M 2, waits for
/// status bit `flag` reading source 17 into M 3, then writes the control
/// word M 4 and reads source 17 into M 5, and stops at 7.
fn wait_and_clear(flag: u64) -> Vec<Insn> {
    vec![
        Insn::new(ALU | SETM | m_src(1) | INTERVAL_PERIOD),
        Insn::new(ALU | SETM | m_src(2) | CLOCK_CONTROL),
        Insn::new(ALU | SETM | src(0o17) | m_dest(3)),
        Insn::new(JUMP | target(5) | bit(flag) | m_src(3) | N),
        Insn::new(JUMP | target(2) | ALWAYS | N),
        Insn::new(ALU | SETM | m_src(4) | CLOCK_CONTROL),
        Insn::new(ALU | SETM | src(0o17) | m_dest(5)),
        Insn::new(JUMP | target(7) | ALWAYS | N),
    ]
}

fn machine(geometry: Geometry, prom: &[Insn], period_us: u32, control: [u32; 2]) -> Machine {
    let mut m = Machine::new();
    let mut words = vec![filler(); 512];
    words[..prom.len()].copy_from_slice(prom);
    m.load_prom(&words);
    m.geometry = geometry;
    support::prom_program_in_ram(&mut m);
    m.mmem[1] = u64::from(period_us);
    m.mmem[2] = u64::from(control[0]);
    m.mmem[4] = u64::from(control[1]);
    m.mmem[6] = 1 << 27;
    m.l2_map[0] = (1 << 23) | (1 << 22);
    m
}

/// Runs until `pc` is reached or `limit` steps; the time then, if reached.
/// A few more steps after, so that the last writes have landed: `rtl`
/// writes M a microcycle late.
fn until<E: Engine>(e: &mut E, pc: u16, limit: u64, ns: fn(&E) -> u64) -> Option<u64> {
    for _ in 0..limit {
        if e.pc() == pc {
            let t = ns(e);
            for _ in 0..4 {
                e.step().unwrap();
            }
            return Some(t);
        }
        e.step().unwrap();
    }
    None
}

fn micro_ns(e: &Micro) -> u64 {
    e.machine().ns
}

/// Both engines, the time each reached `pc` in, and their M memories.
fn both(m: impl Fn() -> Machine, pc: u16, limit: u64) -> [(Option<u64>, Vec<u32>); 2] {
    let mut e = Micro::new(m());
    e.boot();
    let te = until(&mut e, pc, limit, micro_ns);
    let mut r = Rtl::new(m());
    r.boot();
    let tr = until(&mut r, pc, limit, Rtl::ns);
    let low = |m: &Machine| m.mmem.iter().map(|&w| support::low(w)).collect::<Vec<u32>>();
    [(te, low(e.machine())), (tr, low(r.machine()))]
}

/// A JUMP's test of bit `n` on QUUX: M rotated by (40 - n) mod 40 in the
/// ring of 40, `{IR<47>, IR<4:0>}` (contract G2 appendix A1.1).
fn bit_40(n: u64) -> u64 {
    let r = (40 - n) % 40;
    (r >> 5) << 47 | (r & 0o37)
}

/// Timer 0's control and period words on the register page, through
/// virtual page 1, the page at word 1400 of its 1024-word frame.
const TIMER_0_CONTROL: u32 = (1 << 10) | 0o1400 | 0o110;
const TIMER_0_PERIOD: u32 = (1 << 10) | 0o1400 | 0o111;

/// **The tick is timer 0 on the page, and destination 3 does not reach
/// it**: with timer 0's period written as the boot PROM writes it, 16,667
/// µs, a write of 1 to destination 3, Q1's turn-on, leaves word 110 at 0;
/// a write of `401` to word 110 turns it on and its flag rises 16,667 µs
/// later; a write of `403` clears it. Word 110 reads on, periodic and its
/// interrupt enable set, `403` with the flag up, `401` cleared.
#[test]
fn the_tick_is_timer_0_on_the_page_and_not_destination_3() {
    use muir::isa::asm::{MD, SRC_MD, START_READ, START_WRITE};
    let prom = vec![
        // Word 111 gets 16,667, the period.
        Insn::new(ALU | SETM | m_src(1) | MD),
        Insn::new(ALU | SETM | m_src(7) | START_WRITE),
        filler(),
        filler(),
        filler(),
        // Q1's turn-on, to destination 3; word 110 read into M 9.
        Insn::new(ALU | SETM | m_src(2) | CLOCK_CONTROL),
        Insn::new(ALU | SETM | m_src(8) | START_READ),
        filler(),
        filler(),
        Insn::new(ALU | SETM | SRC_MD | m_dest(9)),
        // Word 110 gets 401: on, periodic, its interrupt enable.
        Insn::new(ALU | SETM | m_src(10) | MD),
        Insn::new(ALU | SETM | m_src(8) | START_WRITE),
        // Word 110 read into M 3 until its flag is up.
        Insn::new(ALU | SETM | m_src(8) | START_READ),
        filler(),
        filler(),
        Insn::new(ALU | SETM | SRC_MD | m_dest(3)),
        Insn::new(JUMP | target(18) | bit_40(1) | m_src(3) | N),
        Insn::new(JUMP | target(12) | ALWAYS | N),
        // Cleared by word 110 with 403; read again into M 5.
        Insn::new(ALU | SETM | m_src(11) | MD),
        Insn::new(ALU | SETM | m_src(8) | START_WRITE),
        filler(),
        Insn::new(ALU | SETM | m_src(8) | START_READ),
        filler(),
        filler(),
        Insn::new(ALU | SETM | SRC_MD | m_dest(5)),
        Insn::new(JUMP | target(25) | ALWAYS | N),
    ];
    let m = || {
        let mut m = machine(Geometry::QUUX, &prom, 16_667, [TICK_ON, TICK_CLEAR]);
        assert_eq!(
            support::quux_map(&mut m, 1, muir::machine::REGISTER_PAGE_13 | 0o110),
            TIMER_0_CONTROL
        );
        m.mmem[7] = u64::from(TIMER_0_PERIOD);
        m.mmem[8] = u64::from(TIMER_0_CONTROL);
        m.mmem[10] = 0o401;
        m.mmem[11] = 0o403;
        m
    };
    // `pc()` is the address being fetched, one ahead of the instruction
    // executing: 12 is the turn-on's write executing, 19 the clear's MD,
    // 26 the end.
    let t = both(m, 19, 200_000_000);
    let quick = both(m, 12, 1_000);
    let done = both(m, 26, 200_000_000);
    for (k, name) in ["micro", "rtl"].into_iter().enumerate() {
        let on = quick[k].0.expect(name);
        let seen = t[k].0.expect(name) - on;
        // The poll loop is six microcycles with a page read.
        assert!(seen.abs_diff(16_667_000) <= 30 * 60, "{name}: the first tick after {seen} ns");
        assert_eq!(done[k].1[9], 0, "{name}: word 110 after destination 3's turn-on");
        assert_eq!(
            (done[k].1[3], done[k].1[5]),
            (0o403, 0o401),
            "{name}: word 110 up, then cleared"
        );
    }
}

/// **Source 15 is the microseconds since power-on**, on both engines, and
/// under `sync` on `rtl`: read in a loop, it is the engine's own time over
/// a thousand, within a microsecond; and it wraps at 32 bits.
#[test]
fn source_15_counts_microseconds() {
    use muir::clock::TimingModel;
    let prom = vec![
        Insn::new(ALU | SETM | src(0o15) | m_dest(1)),
        Insn::new(JUMP | target(0) | ALWAYS | N),
    ];
    let quux = |start_ns: u64| {
        let mut m = machine(Geometry::QUUX, &prom, 0, [0, 0]);
        m.ns = start_ns;
        m
    };
    let wrap = (1u64 << 32) * 1000 - 3_000;
    for start in [0, wrap] {
        let mut e = Micro::new(quux(start));
        e.boot();
        for _ in 0..40_000 {
            e.step().unwrap();
        }
        let want = (e.machine().ns / 1000) as u32;
        assert!(
            want.wrapping_sub(support::low(e.machine().mmem[1])) <= 1,
            "micro from {start}: {} against {want}",
            e.machine().mmem[1]
        );
        for model in [
            TimingModel::Sync { cycle_ticks: 4, ilong_ticks: 0 },
            TimingModel::Sync { cycle_ticks: 3, ilong_ticks: 0 },
        ] {
            let mut r = Rtl::new(quux(start));
            r.set_timing_model(model);
            r.boot();
            for _ in 0..40_000 {
                r.step().unwrap();
            }
            let want = (r.ns() / 1000) as u32;
            let got = support::low(r.machine().mmem[1]);
            assert!(
                want.wrapping_sub(got) <= 1,
                "rtl {model:?} from {start}: {got} against {want}"
            );
            if start == wrap {
                assert!(got < 100_000, "rtl {model:?}: wrapped, {got}");
            }
        }
    }
}

/// **The CADR has none of it**: sources 15 and 17 read all ones and
/// destinations 3 and 4 write only M, so nothing ever pends.
#[test]
fn the_cadr_has_no_clocks_in_the_processor() {
    let prom = {
        let mut p = wait_and_clear(0);
        p[6] = Insn::new(ALU | SETM | src(0o15) | m_dest(5));
        p
    };
    for (k, (t, m)) in
        both(|| machine(Geometry::CADR, &prom, 100, [TICK_ON, TICK_CLEAR]), 8, 100_000)
            .into_iter()
            .enumerate()
    {
        let name = ["micro", "rtl"][k];
        assert!(t.is_some(), "{name}: the all-ones source is a flag at once");
        assert_eq!((m[3], m[5]), (!0, !0), "{name}: sources 17 and 15");
        assert_eq!(m[0o37], TICK_CLEAR, "{name}: destination 3 wrote M 37");
    }
}
