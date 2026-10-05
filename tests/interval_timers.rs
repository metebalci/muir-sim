// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's interval timers (contract Q11, revision 10): timer 0, 1 and 2 on
//! the register page, each with its control and status at word 110 + 2k and
//! its period at 111 + 2k; word 100 `<0>`, `<1>` and `<2>` their
//! interrupts, each the flag under the timer's interrupt enable. Q1's
//! functional destinations 3 and 4 write only M and its source 17 reads all
//! ones, as on the CADR: no timer is reached but through the page.
//!
//! The rules are held on [`Machine`] at exact instants, where a word is
//! read or written at [`Machine::ns`]; the instants the engines give those
//! reads and writes (contract Q11, rule 10) are held on `rtl`, and the
//! outcomes on `micro` too.

use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{
    ALU, ALWAYS, JUMP, MD, N, SETM, SETO, SETZ, SRC_MD, START_READ, START_WRITE, a_dest, filler,
    m_dest, m_src, src, target,
};
use muir::machine::{Geometry, IntervalTimer, Machine};
use muir::micro::Micro;
use muir::rtl::Rtl;

mod support;

/// An engine and its own time: `micro`'s is the machine's, `rtl`'s its
/// clock, which the machine's follows only at bus cycles.
trait Timed: Engine {
    fn now(&self) -> u64;
}
impl Timed for Micro {
    fn now(&self) -> u64 {
        self.machine().ns
    }
}
impl Timed for Rtl {
    fn now(&self) -> u64 {
        self.ns()
    }
}

fn engine(name: &str, m: Machine) -> Box<dyn Timed> {
    match name {
        "micro" => Box::new(Micro::new(m)),
        _ => Box::new(Rtl::new(m)),
    }
}

const PAGE: u32 = muir::machine::REGISTER_PAGE_13;
const INTERRUPTS: u32 = PAGE + 0o100;
const RESET_DEVICES: u32 = PAGE + 0o104;

/// Timer `k`'s control and status word, and its period word.
const fn control(k: usize) -> u32 {
    PAGE + 0o110 + 2 * k as u32
}
const fn period(k: usize) -> u32 {
    PAGE + 0o111 + 2 * k as u32
}

/// Word 100's bit for timer `k`: `<0>`, `<1>`, `<2>` (contract Q13).
const BIT: [u32; 3] = [1 << 0, 1 << 1, 1 << 2];

/// The control word's bits.
const ON: u32 = 1;
const FLAG: u32 = 2;
const CLEAR: u32 = 2;
const ONE_SHOT: u32 = 4;
const IE: u32 = 1 << 8;

const US: u64 = 1000;

fn quux() -> Machine {
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
    m
}

/// Timer `k`'s control word and word 100's bit for it, at `ns`.
fn look(m: &mut Machine, k: usize, ns: u64) -> (u32, bool) {
    m.ns = ns;
    let w = support::low(m.bus_read(control(k)));
    (w, m.bus_read(INTERRUPTS) & u64::from(BIT[k]) != 0)
}

/// Whether timer `k`'s flag reads up at `ns`, in its word and, under its
/// interrupt enable, in word 100 --- asserting that the two agree.
fn up(m: &mut Machine, k: usize, ns: u64) -> bool {
    let (w, bit) = look(m, k, ns);
    let flag = w & FLAG != 0;
    let ie = w & IE != 0;
    assert_eq!(bit, flag && ie, "timer {k} at {ns}: word 100 against the word {w:o}");
    assert_eq!(m.interrupt_at(ns), bit, "timer {k} at {ns}: the interrupt against word 100");
    flag
}

fn write(m: &mut Machine, at: u64, word: u32, v: u32) {
    m.ns = at;
    m.bus_write(word, v.into());
}

/// **M1, periodic, each timer**: turned on by a write of its control word
/// after its period is written, its flag first reads 1, in the word and in
/// word 100 with `<8>` set, at the turn-on write's instant plus 1, 2 and 3
/// periods, cleared between; a late clear keeps the grid; a period written
/// while the flag is up takes it down and the next rise is a period after
/// that write; period 0 never rises; off takes the flag down; the period
/// reads back what was written, `<23:0>`, and 0 after a reset.
#[test]
fn m1_a_periodic_timer_rises_every_period_on_its_start_s_grid() {
    for k in 0..3 {
        let mut m = quux();
        assert_eq!(m.bus_read(period(k)), 0, "timer {k}: period 0 at power-on");
        write(&mut m, 1_000, period(k), 50);
        assert_eq!(m.bus_read(period(k)), 50, "timer {k}: the period reads back");
        let (t0, p) = (7_003, 50 * US);
        write(&mut m, t0, control(k), ON | IE);
        assert_eq!(m.bus_read(control(k)), (ON | IE).into(), "timer {k}: on, periodic, enabled");
        for n in 1..=3 {
            assert!(!up(&mut m, k, t0 + n * p - 1), "timer {k}: rise {n} early");
            assert!(up(&mut m, k, t0 + n * p), "timer {k}: rise {n} at t0 + {n} periods");
            write(&mut m, t0 + n * p + 5, control(k), ON | IE | CLEAR);
            assert!(!up(&mut m, k, t0 + n * p + 5), "timer {k}: cleared after rise {n}");
        }
        // Rise 4 is left up past rise 5's instant, and the late clear keeps
        // the grid: the next rise is at 6 periods.
        assert!(up(&mut m, k, t0 + 4 * p));
        write(&mut m, t0 + 5 * p + p / 2, control(k), ON | IE | CLEAR);
        assert!(!up(&mut m, k, t0 + 6 * p - 1), "timer {k}: the late clear kept the grid");
        assert!(up(&mut m, k, t0 + 6 * p), "timer {k}: on the grid after a late clear");
        // A period written while the flag is up takes it down, and the next
        // rise is a period after the write.
        let t1 = t0 + 6 * p + 333;
        write(&mut m, t1, period(k), 30);
        assert_eq!(
            m.bus_read(control(k)) & u64::from(FLAG),
            0,
            "timer {k}: the period write took it down"
        );
        assert!(!up(&mut m, k, t1 + 30 * US - 1));
        assert!(up(&mut m, k, t1 + 30 * US), "timer {k}: a period after the period write");
        // Off takes the flag down.
        write(&mut m, t1 + 30 * US, control(k), IE);
        assert_eq!(look(&mut m, k, t1 + 30 * US), (IE, false), "timer {k}: off, flag down");
        assert!(!up(&mut m, k, t1 + 10_000 * US), "timer {k}: off, nothing rises");
        // Period 0, on, never rises.
        write(&mut m, t1 + 40 * US, period(k), 0);
        write(&mut m, t1 + 40 * US, control(k), ON | IE);
        assert!(!up(&mut m, k, t1 + 10_000_000 * US), "timer {k}: period 0 rose");
        // The period is `<23:0>`; the rest reads 0.
        write(&mut m, t1 + 50 * US, period(k), !0);
        assert_eq!(m.bus_read(period(k)), 0o77777777, "timer {k}: <23:0>");
        // 0 after a reset.
        write(&mut m, t1 + 60 * US, RESET_DEVICES, 1);
        assert_eq!(m.bus_read(period(k)), 0, "timer {k}: period 0 after a reset");
    }
}

/// **M2, one-shot, each timer**: one rise, a period after the start, and
/// none at 2 and 3 periods, with the flag left up and with it cleared; a
/// period write while the flag is up takes it down and gives one rise a
/// period after the write; a turn-on write with `<2>` set to a periodic
/// timer already on changes nothing; off then on with `<2>` set is
/// one-shot; `<2>` reads the mode. A one-shot that has risen and been
/// cleared reads on, flag down, as one that is armed.
#[test]
fn m2_a_one_shot_timer_rises_once() {
    for k in 0..3 {
        let p = 40 * US;
        // Cleared at once after its rise.
        let mut m = quux();
        write(&mut m, 0, period(k), 40);
        let t0 = 2_001;
        write(&mut m, t0, control(k), ON | ONE_SHOT | IE);
        assert_eq!(
            m.bus_read(control(k)),
            (ON | ONE_SHOT | IE).into(),
            "timer {k}: one-shot, armed"
        );
        assert!(!up(&mut m, k, t0 + p - 1));
        assert!(up(&mut m, k, t0 + p), "timer {k}: the one rise");
        write(&mut m, t0 + p + 1, control(k), ON | ONE_SHOT | IE | CLEAR);
        assert_eq!(
            m.bus_read(control(k)),
            (ON | ONE_SHOT | IE).into(),
            "timer {k}: risen and cleared reads as armed"
        );
        for n in [2, 3, 10] {
            assert!(!up(&mut m, k, t0 + n * p), "timer {k}: cleared, a rise at {n} periods");
        }
        // Left up through 3 periods, then cleared: nothing was counted under
        // the raised flag.
        let mut m = quux();
        write(&mut m, 0, period(k), 40);
        write(&mut m, t0, control(k), ON | ONE_SHOT | IE);
        for n in [1, 2, 3] {
            assert!(up(&mut m, k, t0 + n * p), "timer {k}: up at {n} periods");
        }
        write(&mut m, t0 + 3 * p + 1, control(k), ON | IE | CLEAR);
        assert_eq!(
            m.bus_read(control(k)),
            (ON | ONE_SHOT | IE).into(),
            "timer {k}: a write keeps the mode"
        );
        for n in [4, 5, 10] {
            assert!(!up(&mut m, k, t0 + n * p), "timer {k}: left up, a rise at {n} periods");
        }
        // A period write while the flag is up takes it down and gives one
        // rise a period after it.
        let mut m = quux();
        write(&mut m, 0, period(k), 40);
        write(&mut m, t0, control(k), ON | ONE_SHOT | IE);
        assert!(up(&mut m, k, t0 + p));
        let t1 = t0 + p + 77;
        write(&mut m, t1, period(k), 20);
        assert_eq!(m.bus_read(control(k)) & u64::from(FLAG), 0, "timer {k}: taken down");
        assert!(!up(&mut m, k, t1 + 20 * US - 1));
        assert!(up(&mut m, k, t1 + 20 * US), "timer {k}: a period after the period write");
        write(&mut m, t1 + 20 * US, control(k), ON | IE | CLEAR);
        assert!(!up(&mut m, k, t1 + 40 * US), "timer {k}: once");
        assert!(!up(&mut m, k, t1 + 60 * US), "timer {k}: once");
        // A turn-on write with <2> to a periodic timer already on changes
        // nothing: it still rises at 2 periods.
        let mut m = quux();
        write(&mut m, 0, period(k), 40);
        write(&mut m, t0, control(k), ON | IE);
        write(&mut m, t0 + p / 2, control(k), ON | ONE_SHOT | IE);
        assert_eq!(m.bus_read(control(k)) & u64::from(ONE_SHOT), 0, "timer {k}: still periodic");
        assert!(up(&mut m, k, t0 + p));
        write(&mut m, t0 + p, control(k), ON | ONE_SHOT | IE | CLEAR);
        assert!(!up(&mut m, k, t0 + 2 * p - 1));
        assert!(up(&mut m, k, t0 + 2 * p), "timer {k}: periodic still, at 2 periods");
        // Off, then on with <2>: one-shot from the new start.
        write(&mut m, t0 + 2 * p, control(k), IE);
        assert_eq!(m.bus_read(control(k)), IE.into(), "timer {k}: off keeps periodic");
        let t2 = t0 + 2 * p + 9;
        write(&mut m, t2, control(k), ON | ONE_SHOT | IE);
        assert_eq!(m.bus_read(control(k)), (ON | ONE_SHOT | IE).into(), "timer {k}: one-shot now");
        assert!(up(&mut m, k, t2 + p));
        write(&mut m, t2 + p, control(k), ON | IE | CLEAR);
        assert!(!up(&mut m, k, t2 + 2 * p), "timer {k}: one-shot, once");
        // Off keeps the mode until the next turn-on.
        write(&mut m, t2 + 3 * p, control(k), 0);
        assert_eq!(m.bus_read(control(k)), ONE_SHOT.into(), "timer {k}: off, the mode as it was");
    }
}

/// Every write a timer's two words can take, for [`m3_a_write_touches_its_timer_alone`].
const WRITES: [(bool, u32); 9] = [
    (false, ON | IE),
    (false, ON | IE | CLEAR),
    (false, ON | ONE_SHOT),
    (false, 0),
    (false, !0),
    (true, 25),
    (true, 0),
    (true, !0),
    (false, IE),
];

/// **M3, independence**: every write of timer k's words leaves every other
/// timer's words and next rise as they were; a write of the reserved words
/// 105-107 and 116-117 changes no timer, and they read 0.
#[test]
fn m3_a_write_touches_its_timer_alone() {
    let setup = || {
        let mut m = quux();
        // Timer 0 periodic and up, 1 one-shot and armed, 2 periodic, its
        // interrupt enable clear.
        write(&mut m, 0, period(0), 10);
        write(&mut m, 0, control(0), ON | IE);
        write(&mut m, 0, period(1), 70);
        write(&mut m, 0, control(1), ON | ONE_SHOT | IE);
        write(&mut m, 0, period(2), 33);
        write(&mut m, 0, control(2), ON);
        m.ns = 15 * US;
        assert_eq!(m.bus_read(INTERRUPTS), BIT[0].into(), "timer 0 up");
        m
    };
    for k in 0..3 {
        for (is_period, v) in WRITES {
            let mut m = setup();
            let before = m.timers.timer;
            let words: Vec<u32> = (0..3)
                .flat_map(|j| [control(j), period(j)])
                .map(|w| support::low(m.bus_read(w)))
                .collect();
            m.bus_write(if is_period { period(k) } else { control(k) }, v.into());
            for j in (0..3).filter(|&j| j != k) {
                assert_eq!(
                    m.timers.timer[j],
                    before[j],
                    "a write of {v:o} to timer {k}'s {} touched timer {j}",
                    if is_period { "period" } else { "control" }
                );
                assert_eq!(m.bus_read(control(j)), words[2 * j].into(), "timer {j}'s control");
                assert_eq!(m.bus_read(period(j)), words[2 * j + 1].into(), "timer {j}'s period");
            }
        }
    }
    let mut m = setup();
    let before = m.timers;
    for w in [0o105, 0o106, 0o107, 0o116, 0o117] {
        m.bus_write(PAGE + w, !0);
        assert_eq!(m.bus_read(PAGE + w), 0, "reserved word {w:o}");
        assert_eq!(m.timers, before, "reserved word {w:o} changed a timer");
    }
}

/// The state every timer resets to.
const RESET_STATE: IntervalTimer = IntervalTimer::RESET;

fn every_timer_on(m: &mut Machine) {
    let at = m.ns;
    for (k, mode) in [(0, 0), (1, ONE_SHOT), (2, 0)] {
        write(m, at, period(k), 5);
        write(m, at, control(k), ON | IE | mode);
    }
    // Up, all three.
    m.ns = at + 5 * US;
    for k in 0..3 {
        assert_ne!(m.bus_read(control(k)) & u64::from(FLAG), 0);
    }
}

/// **M4, reset**: `-RESET`, `-BOOT` and reset devices each leave every timer
/// off, flag down, periodic, interrupt enable 0 and period 0, on `micro` and
/// `rtl`; the microsecond clock moves on through all three; and timer 0
/// turned on after them, with no period written, never rises.
#[test]
fn m4_every_reset_puts_every_timer_in_its_reset_state() {
    for how in ["-RESET", "-BOOT", "reset devices"] {
        for engine_name in ["micro", "rtl"] {
            let m = engine_machine(&[Insn::new(JUMP | target(0) | ALWAYS | N)], &[]);
            let mut e = engine(engine_name, m);
            e.boot();
            for _ in 0..50 {
                e.step().unwrap();
            }
            every_timer_on(e.machine_mut());
            let ns = e.now();
            match how {
                "-RESET" => e.spy_write(muir::spy::MODE, muir::spy::MODE_RESET),
                "-BOOT" => e.boot(),
                _ => e.machine_mut().bus_write(RESET_DEVICES, 1),
            }
            for _ in 0..4 {
                e.step().unwrap();
            }
            assert!(e.now() > ns, "{how}, {engine_name}: the microsecond clock moved on");
            let m = e.machine_mut();
            assert_eq!(m.timers.timer, [RESET_STATE; 3], "{how}, {engine_name}");
            for k in 0..3 {
                assert_eq!(
                    (m.bus_read(control(k)), m.bus_read(period(k))),
                    (0, 0),
                    "{how}, {engine_name}: timer {k}'s words"
                );
            }
            // Timer 0 on, with no period written: it never rises.
            let at = m.ns;
            write(m, at, control(0), ON | IE);
            assert!(!up(m, 0, at + 10_000_000 * US), "{how}, {engine_name}: period 0 rose");
        }
    }
}

/// A QUUX machine for a program: the program in the control store, virtual
/// page 0 main memory page 0 and virtual page 1 the register page, both
/// readable and writable, and M memory as `m_words` says.
fn engine_machine(prom: &[Insn], m_words: &[(usize, u64)]) -> Machine {
    let mut m = quux();
    let mut words = vec![filler(); 1024];
    words[..prom.len()].copy_from_slice(prom);
    m.load_prom(&words);
    support::prom_program_in_ram(&mut m);
    support::quux_map(&mut m, 0, 0);
    support::quux_map(&mut m, 1, PAGE);
    for &(k, v) in m_words {
        m.mmem[k] = v;
    }
    m
}

/// The register page's word `w` through virtual page 1, the page being at
/// word 1400 of its 1024-word frame.
const fn va(w: u32) -> u64 {
    (1 << 10 | PAGE & 0o1777 | w) as u64
}

/// `INTERRUPT-CONTROL`'s `INT.ENABLE`, `<35>` on QUUX (contract G2
/// appendix A1.6), where the CADR has `<27>`.
const INT_ENABLE: u64 = 1 << 35;

/// A word of ones, QUUX's 40 bits.
const ONES: u64 = (1 << 40) - 1;

/// Functional destinations 2 and 3, and 4, M 37 written too.
const INTERRUPT_CONTROL: u64 = (2 << 19) | (0o37 << 14);
const DEST_3: u64 = (3 << 19) | (0o37 << 14);
const DEST_4: u64 = (4 << 19) | (0o37 << 14);
/// Jump condition 5: a page fault or an interrupt pending.
const PGF_OR_INT: u64 = (1 << 5) | 5;

/// **M6, interrupt, each timer**: with its interrupt enable set, a timer's
/// rise is word 100's bit and takes jump condition 5 on both engines; with
/// it clear the flag rises in the word, and neither the bit nor the
/// condition does; off, nothing.
#[test]
fn m6_a_timer_interrupts_under_its_interrupt_enable() {
    // INT.ENABLE, a read of main memory for VMAOK, then a jump on
    // condition 5 to 5, which sets M 5.
    let prom = [
        Insn::new(ALU | SETM | m_src(6) | INTERRUPT_CONTROL),
        Insn::new(ALU | SETZ | START_READ),
        Insn::new(JUMP | target(4) | PGF_OR_INT | N),
        Insn::new(JUMP | target(2) | ALWAYS | N),
        Insn::new(ALU | SETO | m_dest(5)),
        Insn::new(JUMP | target(5) | ALWAYS | N),
    ];
    for (k, &bit) in BIT.iter().enumerate() {
        for (v, want) in [(ON | IE, true), (ON, false), (IE, false), (ON | ONE_SHOT | IE, true)] {
            for engine_name in ["micro", "rtl"] {
                let m = engine_machine(&prom, &[(6, INT_ENABLE)]);
                let mut e = engine(engine_name, m);
                e.boot();
                let m = e.machine_mut();
                let at = m.ns;
                write(m, at, period(k), 20);
                write(m, at, control(k), v);
                for _ in 0..2_000 {
                    e.step().unwrap();
                }
                assert!(e.now() > at + 20 * US, "{engine_name}: ran past the rise");
                let m = e.machine_mut();
                assert_eq!(m.mmem[5] == ONES, want, "timer {k}, {v:o}, {engine_name}: condition 5");
                let now = at + 100 * US;
                m.ns = now;
                assert_eq!(
                    m.bus_read(control(k)) & u64::from(FLAG) != 0,
                    v & ON != 0,
                    "timer {k}, {v:o}, {engine_name}: the flag, ungated, at {now}"
                );
                assert_eq!(
                    m.bus_read(INTERRUPTS) & u64::from(bit) != 0,
                    want,
                    "timer {k}, {v:o}, {engine_name}: word 100"
                );
                assert_eq!(m.bus_read(INTERRUPTS) & u64::from(!bit), 0, "timer {k}: no other bit");
            }
        }
    }
}

/// **M6, a rise during a wait for `MD`, each timer**: seen by the jump
/// after it, on `rtl` under the FPGA's grid and under `sync` --- `SINTR` is
/// registered at the edge that ends the waiting microcycle, with the flags
/// as they stand at that edge. The program: a read of main memory, `MD`
/// into A 720 (which waits for the word), and a jump on condition 5 that
/// sets M 5; the timer on with a period of `p` µs before `f` fillers. For
/// every `p` and `f`, the jump is taken exactly when the flag rose at or
/// before the edge that ends the waiting microcycle.
#[test]
fn m6_a_rise_during_a_wait_for_md_is_seen_by_the_jump_after() {
    use muir::clock::TimingModel;
    let md_read = Insn::new(ALU | SETM | SRC_MD | a_dest(0o720));
    for k in 0..3 {
        let mut cases = [0; 2];
        for timing in [
            TimingModel::Sync { cycle_ticks: 4, ilong_ticks: 0 },
            TimingModel::Sync { cycle_ticks: 3, ilong_ticks: 0 },
        ] {
            for p in [1u32, 2] {
                for f in 0..40usize {
                    let mut prom = vec![Insn::new(ALU | SETM | m_src(6) | INTERRUPT_CONTROL)];
                    prom.extend(vec![filler(); f]);
                    prom.push(Insn::new(ALU | SETM | m_src(7) | START_READ));
                    prom.push(filler());
                    prom.push(md_read);
                    let jump_at = prom.len() as u64;
                    prom.push(Insn::new(JUMP | target(jump_at + 3) | PGF_OR_INT | N));
                    prom.push(Insn::new(JUMP | target(jump_at + 2) | ALWAYS | N));
                    prom.push(Insn::new(JUMP | target(jump_at + 2) | ALWAYS | N));
                    prom.push(Insn::new(ALU | SETO | m_dest(5)));
                    prom.push(Insn::new(JUMP | target(jump_at + 4) | ALWAYS | N));
                    // Virtual word 405: level-2 entry 4, physical page 100.
                    let mut m = engine_machine(&prom, &[(6, INT_ENABLE)]);
                    m.mmem[7] = support::quux_map(&mut m, 4, (0o100 << 10) | 5).into();
                    m.main[(0o100 << 10) | 5] = 0o777;
                    let mut r = Rtl::new(m);
                    r.set_timing_model(timing);
                    r.boot();
                    let m = r.machine_mut();
                    let at = m.ns;
                    write(m, at, period(k), p);
                    write(m, at, control(k), ON | IE);
                    let rise = r.machine().timers.timer[k].deadline_ns;
                    let mut ends = None;
                    for _ in 0..400 {
                        let ir = r.ir();
                        r.step().unwrap();
                        if ir == md_read.raw() && ends.is_none() {
                            ends = Some(r.ns());
                        }
                    }
                    let ends = ends.unwrap();
                    let taken = r.machine().mmem[5] == ONES;
                    cases[taken as usize] += 1;
                    assert_eq!(
                        taken,
                        rise <= ends,
                        "timer {k}, {timing:?}, {p} µs, {f} fillers: the flag rises at {rise}, \
                         the MD read ends at {ends}"
                    );
                }
            }
        }
        assert!(cases[0] > 0 && cases[1] > 0, "timer {k}: taken and not taken: {cases:?}");
    }
}

/// **M7, the layout**: word 104 reads 0; words 110-115 as the contract has
/// them, their reserved bits 0; feature word 16 is 3, the number of
/// interval timers; MACHINE-ID is revision 13 (contract G2); and on QUUX, on both
/// engines, destination 4 writes only M and source 17 reads all ones.
#[test]
fn m7_the_page_s_layout_and_q1_s_codes_at_revision_10() {
    let mut m = quux();
    assert_eq!(m.bus_read(RESET_DEVICES), 0);
    for k in 0..3 {
        m.bus_write(control(k), !0);
        assert_eq!(
            m.bus_read(control(k)) & u64::from(!(ON | FLAG | ONE_SHOT | IE)),
            0,
            "timer {k}"
        );
        assert_eq!(
            m.bus_read(control(k)),
            (ON | ONE_SHOT | IE).into(),
            "timer {k}: on, one-shot, <8>"
        );
        m.bus_write(period(k), !0);
        assert_eq!(m.bus_read(period(k)), 0o77777777, "timer {k}'s period");
    }
    assert_eq!(m.bus_read(RESET_DEVICES), 0, "word 104 reads 0");
    assert_eq!(Geometry::QUUX.feature_word(PAGE + 0o16), Some(3), "feature word 16");
    assert_eq!(m.bus_read(PAGE + 0o16), 3);
    let id = Geometry::QUUX.machine_id.unwrap();
    assert_eq!((id >> 16, (id >> 4) & 0o7777, id & 0o17), (0x5155, 13, 4), "MACHINE-ID");
    // Destination 4 with a period, then source 17: M 3 gets destination
    // 4's word through M 37, and A 200 all ones.
    let prom = [
        Insn::new(ALU | SETM | m_src(1) | DEST_4),
        Insn::new(ALU | SETM | m_src(0o37) | m_dest(3)),
        Insn::new(ALU | SETM | src(0o17) | a_dest(0o200)),
        Insn::new(JUMP | target(3) | ALWAYS | N),
    ];
    for engine_name in ["micro", "rtl"] {
        let m = engine_machine(&prom, &[(1, 100)]);
        let mut e = engine(engine_name, m);
        e.boot();
        for _ in 0..20 {
            e.step().unwrap();
        }
        let m = e.machine();
        assert_eq!(m.mmem[3], 100, "{engine_name}: destination 4 wrote M");
        assert_eq!(m.amem[0o200], ONES, "{engine_name}: source 17");
        assert_eq!(m.timers.timer, [RESET_STATE; 3], "{engine_name}: destination 4 set no timer");
    }
}

/// **M8, a checkpoint**: taken in the middle of a period of each timer,
/// with a one-shot that has risen and one timer's `<8>` clear, it resumes
/// to the same rises, on both engines. The program clears timer 0 through
/// word 110 and polls word 112 in a loop, so that timer 0 rises again and
/// again; the resumed and the original machines' timers are compared after
/// every microcycle.
#[test]
fn m8_a_checkpoint_resumes_to_the_same_rises() {
    use muir::checkpoint::{Reader, Writer};
    let prom = [
        Insn::new(ALU | SETM | m_src(2) | MD),
        Insn::new(ALU | SETM | m_src(4) | START_WRITE),
        filler(),
        Insn::new(ALU | SETM | m_src(1) | START_READ),
        filler(),
        filler(),
        Insn::new(ALU | SETM | SRC_MD | a_dest(0o200)),
        Insn::new(JUMP | target(0) | ALWAYS | N),
    ];
    for engine_name in ["micro", "rtl"] {
        let words = [(1, va(0o112)), (2, (ON | CLEAR | IE).into()), (4, va(0o110))];
        let make = || engine(engine_name, engine_machine(&prom, &words));
        let mut e = make();
        e.boot();
        let m = e.machine_mut();
        let at = m.ns;
        write(m, at, period(0), 7);
        write(m, at, control(0), ON | IE);
        write(m, at, period(1), 3);
        write(m, at, control(1), ON | ONE_SHOT | IE);
        write(m, at, period(2), 11);
        write(m, at, control(2), ON);
        // Past timer 1's one rise, in the middle of the others' periods.
        while e.machine().ns < at + 4 * US + 500 {
            e.step().unwrap();
        }
        let m = e.machine();
        let now = m.ns;
        assert!(m.timers.timer[1].flag(now), "timer 1, one-shot, has risen");
        for k in [0, 2] {
            let t = m.timers.timer[k];
            assert!(t.deadline_ns > now && t.deadline_ns < now + t.period_us as u64 * US);
        }
        let mut w = Writer::new();
        e.save(&mut w);
        let body = w.finish();
        let mut resumed = make();
        resumed.load(&mut Reader::for_word_bits(&body, 40)).unwrap();
        let mut rises = 0;
        let mut last = e.machine().timers.timer[0].deadline_ns;
        for n in 0..20_000 {
            e.step().unwrap();
            resumed.step().unwrap();
            let (a, b) = (e.machine(), resumed.machine());
            assert_eq!((a.ns, a.timers), (b.ns, b.timers), "{engine_name}: microcycle {n}");
            if a.timers.timer[0].deadline_ns != last {
                last = a.timers.timer[0].deadline_ns;
                rises += 1;
            }
        }
        assert!(rises > 10, "{engine_name}: timer 0 rose and was cleared {rises} times");
    }
}

/// Runs `r` for `steps` microcycles, and gives the machine's time after the
/// microcycle that executed `marker`, the first time it did.
fn run_marking(r: &mut Rtl, marker: Insn, steps: usize) -> u64 {
    let mut at = None;
    for _ in 0..steps {
        let ir = r.ir();
        r.step().unwrap();
        if ir == marker.raw() && at.is_none() {
            at = Some(r.ns());
            // The page's cycle is taken at this very edge.
            assert_eq!(r.bus_answered_at(), Some(r.ns()), "the cycle taken at the marker's edge");
        }
    }
    at.expect("the marker ran")
}

/// **M13, the shared edge, writes** (`rtl`): destination 3 written in the
/// microcycle whose ending edge takes a write of word 110 changes nothing
/// at that edge either, timer 0 being what the register write alone makes
/// it. Word 110 written with `401` against destination 3 with 0 turns it
/// on, periodic under its interrupt enable, its period from that edge;
/// written with 0 against destination 3 with 1, Q1's turn-on, it stays
/// off; and on and up under its interrupt enable, written with `401`
/// against destination 3 with 3, Q1's clear, its flag stays up.
#[test]
fn m13_at_a_shared_edge_destination_3_changes_nothing() {
    let dest_3 = Insn::new(ALU | SETM | m_src(3) | DEST_3);
    let prom = [
        Insn::new(ALU | SETM | m_src(2) | MD),
        Insn::new(ALU | SETM | m_src(1) | START_WRITE),
        dest_3,
    ];
    for (up, word, dest3) in [(false, 0o401, 0), (false, 0, 1), (true, 0o401, 3)] {
        let mut r =
            Rtl::new(engine_machine(&prom, &[(1, va(0o110)), (2, word.into()), (3, dest3)]));
        r.boot();
        let m = r.machine_mut();
        let at = m.ns;
        write(m, at, period(0), 100);
        if up {
            m.timers.timer[0] = IntervalTimer {
                on: true,
                one_shot: false,
                interrupt_enable: true,
                period_us: 100,
                deadline_ns: 0,
            };
        }
        let edge = run_marking(&mut r, dest_3, 40);
        let t = r.machine().timers.timer[0];
        let what = format!("word 110 {word:o} against destination 3 {dest3}, up {up}");
        assert_eq!(t.on, word & ON != 0, "{what}");
        if up {
            assert!(t.flag(r.ns()), "{what}: the flag still up");
        } else if t.on {
            assert_eq!(t.deadline_ns, edge + 100 * US, "{what}: the period from the edge");
            assert!(t.interrupt_enable && !t.one_shot, "{what}");
        }
    }
}

/// **M13, `SINTR` at the shared edge** (`rtl`): with timer 0's flag up
/// under its interrupt enable, destination 3 written with 3, Q1's clear, at
/// the edge that takes a write of word 110 leaves the flag in that edge's
/// `SINTR`: the jump after it is taken, as it is with 1.
#[test]
fn m13_sintr_at_the_shared_edge_keeps_what_destination_3_does_not_clear() {
    let dest_3 = Insn::new(ALU | SETM | m_src(3) | DEST_3);
    let prom = [
        Insn::new(ALU | SETM | m_src(6) | INTERRUPT_CONTROL),
        Insn::new(ALU | SETM | m_src(2) | MD),
        Insn::new(ALU | SETM | m_src(1) | START_WRITE),
        dest_3,
        Insn::new(JUMP | target(7) | PGF_OR_INT | N),
        Insn::new(JUMP | target(5) | ALWAYS | N),
        filler(),
        Insn::new(ALU | SETO | m_dest(5)),
        Insn::new(JUMP | target(8) | ALWAYS | N),
    ];
    for dest3 in [3, 1] {
        let mut r = Rtl::new(engine_machine(
            &prom,
            &[(1, va(0o110)), (2, 0o401), (3, dest3), (6, INT_ENABLE)],
        ));
        r.boot();
        r.machine_mut().timers.timer[0] = IntervalTimer {
            on: true,
            one_shot: false,
            interrupt_enable: true,
            period_us: 1000,
            deadline_ns: 0,
        };
        run_marking(&mut r, dest_3, 40);
        assert_eq!(r.machine().mmem[5], ONES, "destination 3 written with {dest3}: taken");
    }
}

/// **M13, reads at the shared edge** (`rtl`), each for word 100 and word
/// 110, with timer 0 on under its interrupt enable. A read gives the flags
/// as they stood at the edge that takes its cycle:
/// - its flag up, destination 3 written with 3, Q1's clear, or with 1, in
///   the microcycle whose ending edge takes the read: up, destination 3
///   clearing nothing;
/// - a rise one fabric tick, 10 ns, after that edge: down;
/// - a rise on that edge itself: up.
#[test]
fn m13_a_read_gives_the_flags_as_they_stood_at_its_edge() {
    let dest_3 = Insn::new(ALU | SETM | m_src(3) | DEST_3);
    let mut prom = vec![Insn::new(ALU | SETM | m_src(1) | START_READ), dest_3];
    prom.extend([filler(); 12]);
    prom.push(Insn::new(ALU | SETM | SRC_MD | a_dest(0o200)));
    prom.push(Insn::new(JUMP | target(prom.len() as u64) | ALWAYS | N));
    let flag_of = |word: u32, v: u32| if word == 0o100 { v & 1 } else { v >> 1 & 1 };
    for word in [0o100, 0o110] {
        let timer = |deadline_ns| IntervalTimer {
            on: true,
            one_shot: false,
            interrupt_enable: true,
            period_us: 1000,
            deadline_ns,
        };
        let run = |dest3: u32, deadline: u64| {
            let mut r = Rtl::new(engine_machine(&prom, &[(1, va(word)), (3, dest3.into())]));
            r.boot();
            r.machine_mut().timers.timer[0] = timer(deadline);
            let edge = run_marking(&mut r, dest_3, 60);
            (edge, flag_of(word, support::low(r.machine().amem[0o200])))
        };
        assert_eq!(run(3, 0).1, 1, "word {word:o}: destination 3 with 3, the flag up");
        assert_eq!(run(1, 0).1, 1, "word {word:o}: destination 3 with 1, the flag up");
        let (edge, _) = run(1, u64::MAX);
        assert_eq!(run(1, edge + 10), (edge, 0), "word {word:o}: a rise 10 ns after the edge");
        assert_eq!(run(1, edge), (edge, 1), "word {word:o}: a rise on the edge");
    }
}

/// **Destination 3 writes only M** at revision 10, on both engines, as on
/// the CADR: whatever timer 0's state and whatever the word --- Q1's turn-on
/// 1 with `<3:2>` set, 1, 0 and the clear 3 --- every timer is left exactly
/// as it was, flag and next rise included, and M 37 gets the word.
#[test]
fn destination_3_writes_only_m_at_revision_10() {
    let prom =
        [Insn::new(ALU | SETM | m_src(3) | DEST_3), Insn::new(JUMP | target(1) | ALWAYS | N)];
    for engine_name in ["micro", "rtl"] {
        for (before, v) in [
            // Timer 0 before: on, one-shot, <8>, deadline; the write.
            ((false, true, false, u64::MAX), 1 | 0o14),
            ((false, false, false, u64::MAX), 1),
            ((true, false, false, 0), 1),
            ((true, false, true, 0), 0),
            ((true, true, false, 0), 3),
            ((true, false, true, 0), 3),
        ] {
            let m = engine_machine(&prom, &[(3, v)]);
            let mut e = engine(engine_name, m);
            e.boot();
            let m = e.machine_mut();
            m.timers.timer[0] = IntervalTimer {
                on: before.0,
                one_shot: before.1,
                interrupt_enable: before.2,
                period_us: 1000,
                deadline_ns: before.3,
            };
            let timers = m.timers;
            for _ in 0..10 {
                e.step().unwrap();
            }
            let m = e.machine();
            assert_eq!(m.timers, timers, "{engine_name}: {before:?} written with {v:o}");
            assert_eq!(m.mmem[0o37], v, "{engine_name}: destination 3 wrote M 37");
        }
    }
}
