// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The profile harness's time line (`examples/profile.rs`, through
//! `tests/support/profile.rs`) against the engines' clocks: a microcycle
//! count leaves the memory's waits out, so the line gives the microcycles,
//! the memory's time and the time in all side by side, and the time in all
//! is the clock's.

use muir::cache::MemoryTiming;
use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{
    ALU, ALWAYS, JUMP, N, SETM, SRC_MD, START_READ, a_dest, filler, m_src, target,
};
use muir::machine::{Geometry, Machine};
use muir::micro::Micro;
use muir::rtl::Rtl;

mod support;

use support::profile::Span;

/// QUUX's microcycle: four ticks of 10 ns, one period whatever the
/// instruction (`sync`, no `ILONG` ticks).
const QUUX_PERIOD_NS: u64 = 40;

/// A loop reading main memory a line apart, 64 lines round, so that
/// with QUUX's 4K-word cache fitted the first round misses on every read
/// and waits for its line, and the later rounds hit.
fn reading() -> Machine {
    let mut prom = vec![
        Insn::new(ALU | SETM | m_src(1) | START_READ),
        // The microinstruction after a start may not read `MD`.
        filler(),
        Insn::new(ALU | SETM | SRC_MD | a_dest(0o200)),
    ];
    prom.push(Insn::new(JUMP | target(0) | ALWAYS | N));
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
    let mut words = vec![filler(); 1024];
    words[..prom.len()].copy_from_slice(&prom);
    m.load_prom(&words);
    support::prom_program_in_ram(&mut m);
    // Virtual page 1 onto physical page 4, of 1024 words; M 1 its first
    // word.
    m.l2_map[1] = (1 << 27) | (1 << 26) | 4;
    m.mmem[1] = 1 << 10;
    m
}

/// Runs `e` `n` steps, moving M 1 a line of 8 on, round 64 lines of the
/// page, after each read so the next read is another line's.
fn run<E: Engine>(e: &mut E, n: usize) {
    for _ in 0..n {
        e.step().unwrap();
        let m = e.machine_mut();
        if m.opc == 0 {
            m.mmem[1] = (1 << 10) | ((m.mmem[1] + 8) & 0o777);
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

/// **On `rtl` the time in all is the microcycles' time and the stalls
/// together**, and the line says each: with the Arty Z7-20's memory, a span
/// that misses in the cache waits, so its clock is more than 40 ns times
/// its microcycles by exactly the time stalled, and the line's "ns in all"
/// is the clock's.
#[test]
fn rtl_s_time_in_all_has_the_stalls_in_it() {
    let mut e = Rtl::new(reading());
    e.set_memory_timing(Some(MemoryTiming::ARTY_Z7_20));
    e.boot();
    run(&mut e, 100);
    let (start, ns0) = (Span::of_rtl(&e), e.ns());
    run(&mut e, 2000);
    let (mid, ns1) = (Span::of_rtl(&e), e.ns());
    run(&mut e, 2000);
    let (end, ns2) = (Span::of_rtl(&e), e.ns());
    let first = mid.since(start);
    let both = first.plus(end.since(mid));

    for (span, clock, what) in [(first, ns1 - ns0, "a span"), (both, ns2 - ns0, "two spans added")]
    {
        let line = span.line();
        eprintln!("{what}: {line}");
        let microcycles = after(&line, "time:");
        let microcycle_ns = after(&line, " microcycles in");
        let stalled = after(&line, "stalled on memory");
        let in_all = before(&line, " ns in all");
        assert!(stalled > 0, "{what}: the reads missed and waited: {line}");
        assert_eq!(in_all, clock, "{what}: the line's time in all is the clock's: {line}");
        assert_eq!(
            microcycle_ns,
            QUUX_PERIOD_NS * microcycles,
            "{what}: the microcycles' time is their period's: {line}"
        );
        assert_ne!(
            in_all,
            QUUX_PERIOD_NS * microcycles,
            "{what}: a microcycle count leaves the stalls out: {line}"
        );
        assert_eq!(in_all, microcycle_ns + stalled, "{what}: the time in all is both: {line}");
        let share = 100.0 * stalled as f64 / clock as f64;
        assert!(
            line.ends_with(&format!("stalled {share:.2}% of the time in all")),
            "{what}: the share names what it is of: {line}"
        );
    }
}

/// **On `micro` the memory's time is a charge, and the line says so**:
/// `micro` has no waits, and its clock is the microcycles' time and a
/// fixed charge a memory cycle.
#[test]
fn micro_s_memory_time_is_named_a_charge() {
    let mut e = Micro::new(reading());
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
    assert_eq!(in_all, e.machine().ns - ns0, "the line's time in all is the clock's: {line}");
    assert_eq!(in_all, QUUX_PERIOD_NS * microcycles + charged, "{line}");
    assert!(line.contains("% of the time in all"), "{line}");
}

/// **Under the time-neutral harness `micro` charges no memory time, and the
/// line says so** (MP2b ruling Q12: neutral time is `Machine::cycles` times
/// the period, with no memory charge): revision 14's machine reading main
/// memory through the physical memory window, at the pipeline's revision-14
/// period of 9 ns, its clock 9 ns a microcycle and nothing more. The line's
/// time in all is the clock's, all of it the microcycles', none charged.
/// It subtracted a charge never made and printed an underflowed time.
#[test]
fn micro_under_neutral_time_charges_no_memory_time() {
    const PHYS: u64 = 0o36000000000;
    let prom = [
        Insn::new(ALU | muir::isa::asm::SETA | muir::isa::asm::a_src(0o100) | START_READ),
        filler(),
        Insn::new(ALU | SETM | SRC_MD | a_dest(0o200)),
        Insn::new(JUMP | target(0) | ALWAYS | N),
        filler(),
    ];
    let mut m = Machine::with_geometry(Geometry::QUUX_14, 1);
    let mut words = vec![filler(); 1024];
    words[..prom.len()].copy_from_slice(&prom);
    m.load_prom(&words);
    support::prom_program_in_ram(&mut m);
    m.amem[0o100] = PHYS | 0o4000;
    let mut e = Micro::new(m);
    e.neutral = true;
    e.period = muir::pipeline::PERIOD_14;
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
    assert_eq!(in_all, e.machine().ns - ns0, "the line's time in all is the clock's: {line}");
    assert_eq!(in_all, muir::pipeline::PERIOD_14 * microcycles, "{line}");
    assert_eq!(microcycle_ns, in_all, "all of it the microcycles': {line}");
    assert!(line.contains("neutral time, no memory charged"), "{line}");
}
