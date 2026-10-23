// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The profile harness's time line (`examples/profile.rs`, through
//! `tests/support/profile.rs`) against the engines' clocks: a microcycle
//! count leaves the memory's waits out, so the line gives the microcycles,
//! the memory's time and the time in all side by side, and the time in all
//! is the clock's.

use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{
    ALU, ALWAYS, JUMP, N, SETM, SRC_MD, START_READ, a_dest, filler, m_src, target,
};
use muir::machine::{Geometry, Machine};
use muir::micro::Micro;

mod support;

use support::profile::Span;

/// A loop reading main memory a line apart, 64 lines round, page 4
/// through the physical memory window.
fn reading_16() -> Machine {
    let prom = [
        Insn::new(ALU | SETM | m_src(1) | START_READ),
        // The microinstruction after a start may not read `MD`.
        filler(),
        Insn::new(ALU | SETM | SRC_MD | a_dest(0o200)),
        Insn::new(JUMP | target(0) | ALWAYS | N),
    ];
    let mut q = support::quux_16();
    let mut words = vec![filler(); 1024];
    words[..prom.len()].copy_from_slice(&prom);
    q.load_prom(&words);
    support::prom_program_in_ram(&mut q);
    q.mmem[1] = PHYS | 4 << 10;
    q
}

/// The physical memory window (contract G3 revision 14, A14.1).
const PHYS: u64 = 0o36000000000;

/// Runs `e` `n` steps, moving M 1 a line of 8 on, round 64 lines of the
/// page, after each read so the next read is another line's.
fn run<E: Engine>(e: &mut E, n: usize) {
    for _ in 0..n {
        e.step().unwrap();
        let m = e.machine_mut();
        if m.opc == 0 {
            m.mmem[1] = (m.mmem[1] & !0o777) | ((m.mmem[1] + 8) & 0o777);
        }
    }
}

/// The number in `line` just before `label`.
fn before(line: &str, label: &str) -> u64 {
    let at = line.find(label).unwrap_or_else(|| panic!("no {label:?} in {line:?}"));
    let word = line[..at].trim_end().rsplit([' ', '(']).next().unwrap();
    word.parse().unwrap_or_else(|_| panic!("no number before {label:?} in {line:?}"))
}

/// The number in `line` just after `label`.
fn after(line: &str, label: &str) -> u64 {
    let at = line.find(label).unwrap_or_else(|| panic!("no {label:?} in {line:?}")) + label.len();
    let word = line[at..].trim_start().split(' ').next().unwrap();
    word.parse().unwrap_or_else(|_| panic!("no number after {label:?} in {line:?}"))
}

/// **On `micro` the memory's time is a charge, and the line says so**:
/// `micro` has no waits, and its clock is the microcycles' time and a
/// fixed charge a memory cycle. On revision 16, at its period of 10 ns.
#[test]
fn micro_s_memory_time_is_named_a_charge() {
    let mut e = Micro::new(reading_16());
    e.boot();
    run(&mut e, 100);
    let start = Span::of_micro(&e);
    let (ns0, mc0) = (e.machine().ns, e.memory_cycles());
    run(&mut e, 2000);
    let span = Span::of_micro(&e).since(start);
    let line = span.line();
    eprintln!("{line}");
    let microcycles = after(&line, "time:");
    let charged = after(&line, "memory charged");
    let each = before(&line, " ns a memory cycle, fixed; micro does not stall)");
    let in_all = before(&line, " ns in all");
    assert_eq!(each, e.memory_cycle_ns);
    assert!(e.memory_cycles() > mc0, "the loop reads");
    assert_eq!(charged, (e.memory_cycles() - mc0) * each);
    let per_ns = e.machine().time_base().per_ns;
    assert_eq!(
        in_all,
        (e.machine().ns - ns0) / per_ns,
        "the line's time in all is the clock's: {line}"
    );
    let period_ns = muir::clock::PERIOD_15 / 2;
    assert_eq!(in_all, period_ns * microcycles + charged, "{line}");
    assert!(line.contains("% of the time in all"), "{line}");
}

/// **Under the time-neutral harness `micro` charges no memory time, and the
/// line says so** (MP2b ruling Q12: neutral time is `Machine::cycles` times
/// the period, with no memory charge): revision 16's machine reading main
/// memory through the physical memory window, at its period of 10 ns, its
/// clock 10 ns a microcycle and nothing more. The line's
/// time in all is the clock's, all of it the microcycles', none charged.
/// It subtracted a charge never made and printed an underflowed time.
#[test]
fn micro_under_neutral_time_charges_no_memory_time() {
    let prom = [
        Insn::new(ALU | muir::isa::asm::SETA | muir::isa::asm::a_src(0o100) | START_READ),
        filler(),
        Insn::new(ALU | SETM | SRC_MD | a_dest(0o200)),
        Insn::new(JUMP | target(0) | ALWAYS | N),
        filler(),
    ];
    let mut m = Machine::with_geometry(Geometry::QUUX, 1);
    let mut words = vec![filler(); 1024];
    words[..prom.len()].copy_from_slice(&prom);
    m.load_prom(&words);
    support::prom_program_in_ram(&mut m);
    m.amem[0o100] = PHYS | 0o4000;
    let mut e = Micro::new(m);
    e.neutral = true;
    e.period = muir::clock::PERIOD_15;
    e.boot();
    run(&mut e, 100);
    let start = Span::of_micro(&e);
    let (ns0, mc0) = (e.machine().ns, e.memory_cycles());
    run(&mut e, 2000);
    let span = Span::of_micro(&e).since(start);
    let line = span.line();
    eprintln!("{line}");
    assert!(e.memory_cycles() > mc0, "the loop reads");
    let microcycles = after(&line, "time:");
    let microcycle_ns = after(&line, " microcycles in");
    let in_all = before(&line, " ns in all");
    let per_ns = e.machine().time_base().per_ns;
    assert_eq!(
        in_all,
        (e.machine().ns - ns0) / per_ns,
        "the line's time in all is the clock's: {line}"
    );
    assert_eq!(in_all, muir::clock::PERIOD_15 / 2 * microcycles, "{line}");
    assert_eq!(microcycle_ns, in_all, "all of it the microcycles': {line}");
    assert!(line.contains("neutral time, no memory charged"), "{line}");
}

/// **The pipeline's line names its clock's period in ns**, in units of
/// 0.5 ns, at 8.5, set, so that the half shows. The line's time in all is
/// clocks times that period. A line that reads the period as whole ns
/// says 17.
#[test]
fn the_pipeline_s_line_names_its_period_in_ns() {
    use muir::pipeline::{CACHE_WORDS, Pipeline, PortTiming};
    let mut m = Machine::with_geometry(Geometry::QUUX, 1);
    let mut words = vec![filler(); 1024];
    words[0] = Insn::new(JUMP | target(0) | ALWAYS | N);
    m.load_prom(&words);
    support::prom_program_in_ram(&mut m);
    let mut e = Pipeline::new(m);
    e.configure(17, PortTiming::KRIA, CACHE_WORDS);
    e.boot();
    e.skip_sweep();
    let start = Span::of_pipeline(&e);
    for _ in 0..1000 {
        e.step().unwrap();
    }
    let span = Span::of_pipeline(&e).since(start);
    let line = span.line();
    eprintln!("{line}");
    assert!(line.contains("clocks of 8.5 ns"), "{line}");
    let clocks = before(&line, "clocks of");
    assert_eq!(before(&line, " ns in all"), clocks * 85 / 10, "{line}");
}
