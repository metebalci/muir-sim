// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The multiply and divide steps, on all three engines.
//!
//! `MUS`, `DVS1`, `DVS` and `DVREM` are ALU functions 40, 51, 41 and 45 in
//! the ALU FUNCTIONS table of `mit/cadr/ir.bits`, and page ALUC4 decides
//! what the 74S181s do on each from `Q<0>` and the A source's sign
//! (`crate::ttl::alu_control`, shared by `micro` and `rtl`). The boot PROM
//! never runs them, and microcode 323 runs them in every `MPY` and `DIV`.
//! Here a whole 32-step multiply and a whole 33-step divide, each on three
//! pairs of operands, leave the same high word and the same `Q` on `micro`,
//! on `rtl` and on the netlist.
//!
//! The steps are laid out as `uc-arith.lisp`'s loops lay them: the
//! multiply shifts the output bus and `Q` right, the divide shifts both
//! left, `DVS1` first and `DVREM` last.  Operands are built in M memory by
//! doubling and adding the carry, so that nothing but the ALU is used.

use muir::benchmark::{self, Program, Stop};
use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{
    ALU, ALWAYS, CARRY_IN, JUMP, M_PLUS_M, OB_LEFT, OB_RIGHT, Q_LEFT, Q_LOAD, Q_RIGHT, SETM, SETZ,
    SRC_MD, SRC_Q, START_READ, VMA, a_dest, a_src, filler, m_dest, m_src, target,
};
use muir::machine::{Geometry, Machine};
use muir::micro::Micro;
use muir::muldiv;
use muir::rtl::Rtl;

mod support;

const MUS: u64 = 0o40 << 3;
const DVS: u64 = 0o41 << 3;
const DVS1: u64 = 0o51 << 3;
const DVREM: u64 = 0o45 << 3;

/// `v` into M memory `r`: zero, then one doubling a bit, with the bit as the
/// carry in.
fn constant(v: u32, r: u64, out: &mut Vec<Insn>) {
    out.push(Insn::new(ALU | SETZ | m_dest(r)));
    for b in (0..32).rev() {
        let c = if (v >> b) & 1 != 0 { CARRY_IN } else { 0 };
        out.push(Insn::new(ALU | M_PLUS_M | c | m_src(r) | a_src(r) | m_dest(r)));
    }
}

/// `body`, then the result into `VMA`, then a jump to itself.
fn program(mut body: Vec<Insn>, result_in_q: bool) -> Program {
    body.push(Insn::new(ALU | SETM | if result_in_q { SRC_Q } else { m_src(5) } | VMA));
    let top = body.len() as u16;
    body.push(Insn::new(JUMP | target(top as u64) | ALWAYS));
    body.resize(512, filler());
    Program { name: "muldiv", prom: body, top, cycles_per_iteration: 1 }
}

fn multiply(x: u32, y: u32) -> Vec<Insn> {
    let mut b = Vec::new();
    constant(x, 1, &mut b);
    constant(y, 2, &mut b);
    b.push(Insn::new(ALU | SETM | m_src(2) | a_dest(0o100)));
    b.push(Insn::new(ALU | SETM | m_src(1) | Q_LOAD | m_dest(3)));
    b.push(Insn::new(ALU | SETZ | m_dest(5)));
    for _ in 0..32 {
        b.push(Insn::new(OB_RIGHT | MUS | Q_RIGHT | m_src(5) | a_src(0o100) | m_dest(5)));
    }
    b
}

fn divide(hi: u32, lo: u32, d: u32) -> Vec<Insn> {
    let mut b = Vec::new();
    constant(lo, 1, &mut b);
    constant(d, 2, &mut b);
    constant(hi, 5, &mut b);
    b.push(Insn::new(ALU | SETM | m_src(2) | a_dest(0o100)));
    b.push(Insn::new(ALU | SETM | m_src(1) | Q_LOAD | m_dest(3)));
    b.push(Insn::new(OB_LEFT | DVS1 | Q_LEFT | m_src(5) | a_src(0o100) | m_dest(5)));
    for _ in 0..31 {
        b.push(Insn::new(OB_LEFT | DVS | Q_LEFT | m_src(5) | a_src(0o100) | m_dest(5)));
    }
    b.push(Insn::new(ALU | DVREM | m_src(5) | a_src(0o100) | m_dest(5)));
    b
}

/// Every case, with its name: three multiplies and three divides, each read
/// twice, once for the high word and once for `Q`.
fn cases() -> Vec<(String, Program)> {
    let mut all = Vec::new();
    for q in [false, true] {
        let half = if q { "Q" } else { "M" };
        for (x, y) in [(12345u32, 0x9abcd), (0xffff_fff3, 7), (0x8000_0001, 0x7fff_ffff)] {
            all.push((format!("{x:x} * {y:x}, {half}"), program(multiply(x, y), q)));
        }
        for (hi, lo, d) in [(0u32, 100_000u32, 7u32), (0, 0x8765_4321, 0x1234), (!0, !0 - 15, 3)] {
            all.push((format!("{hi:x}:{lo:x} / {d:x}, {half}"), program(divide(hi, lo, d), q)));
        }
    }
    all
}

fn on_engine<E: Engine>(new: fn(Machine) -> E, boot: fn(&mut E), p: &Program) -> u32 {
    let mut m = Machine::new();
    m.load_prom(&p.prom);
    support::prom_program_in_ram(&mut m);
    let mut e = new(m);
    boot(&mut e);
    benchmark::run_engine(&mut e, p, Stop::Cycles(4)).iterations as u32
}

/// **The steps leave the same words on `micro`, `rtl` and the board.**
#[test]
fn the_multiply_and_divide_steps_agree_on_all_three_engines() {
    let n = support::netlists();
    for (name, p) in cases() {
        let (mut c, mut clk, mut far) = support::chip(&n);
        benchmark::boot_chip(&mut c, &n.cpu, &mut far, &mut clk, &p);
        let board = benchmark::run_chip(&mut c, &n.cpu, &mut far, &mut clk, &p, Stop::Cycles(4))
            .iterations as u32;
        assert_eq!(on_engine(Rtl::new, Rtl::boot, &p), board, "rtl: {name}");
        assert_eq!(on_engine(Micro::new, Micro::boot, &p), board, "micro: {name}");
    }
}

// --- QUUX's one-instruction multiply and divide, ALU functions 42 and 43 ---

const MUL: u64 = 0o42 << 3;
const DIV: u64 = 0o43 << 3;

/// Operands: the corners, then a spread from a fixed linear congruential
/// generator.
fn operands(n: usize) -> Vec<u32> {
    let mut v = vec![0, 1, 2, 3, 7, !0, !0 - 1, 0x8000_0000, 0x7fff_ffff, 0x8000_0001, 0xffff];
    let mut x: u32 = 0x1234_5678;
    while v.len() < n {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        v.push(x);
    }
    v
}

/// `multiply`'s setup, then one `MUL` with `extra` in its other fields.
fn one_multiply(x: u32, y: u32, extra: u64) -> Vec<Insn> {
    let mut b = multiply(x, y);
    b.truncate(b.len() - 32);
    b.push(Insn::new(extra | MUL | m_src(5) | a_src(0o100) | m_dest(5)));
    b
}

/// `divide`'s setup and its first `steps` steps: `DVS1`, then `DVS`.
fn divide_steps(hi: u32, lo: u32, d: u32, steps: usize) -> Vec<Insn> {
    let mut b = divide(hi, lo, d);
    b.truncate(b.len() - 33 + steps);
    b
}

/// `divide`'s setup, then one `DIV` with `extra` in its other fields.
fn one_divide(hi: u32, lo: u32, d: u32, extra: u64) -> Vec<Insn> {
    let mut b = divide_steps(hi, lo, d, 0);
    b.push(Insn::new(extra | DIV | m_src(5) | a_src(0o100) | m_dest(5)));
    b
}

fn on_machine<E: Engine>(
    geometry: Geometry,
    new: fn(Machine) -> E,
    boot: fn(&mut E),
    body: &[Insn],
    q: bool,
) -> u32 {
    let p = program(body.to_vec(), q);
    let mut m = Machine::new();
    m.geometry = geometry;
    m.load_prom(&p.prom);
    support::prom_program_in_ram(&mut m);
    let mut e = new(m);
    boot(&mut e);
    benchmark::run_engine(&mut e, &p, Stop::Cycles(4)).iterations as u32
}

/// The high word and `Q` a body leaves, on both engines as `geometry`: the
/// two must agree.
fn words(geometry: Geometry, body: &[Insn], name: &str) -> (u32, u32) {
    let r = |q| on_machine(geometry, Rtl::new, Rtl::boot, body, q);
    let e = |q| on_machine(geometry, Micro::new, Micro::boot, body, q);
    let (rm, rq) = (r(false), r(true));
    assert_eq!((e(false), e(true)), (rm, rq), "micro against rtl: {name}");
    (rm, rq)
}

/// **On QUUX, `MUL` leaves what 32 multiply steps leave**: the same high
/// word on the output bus and the same low word in `Q`, whatever the output
/// selector and Q control say. The steps are run on the CADR, where the
/// test above holds them to the netlist.
#[test]
fn quux_s_mul_is_32_multiply_steps() {
    let ops = operands(24);
    for (k, &x) in ops.iter().enumerate() {
        let y = ops[(k * 7 + 3) % ops.len()];
        let name = format!("{x:x} * {y:x}");
        let steps = words(Geometry::CADR, &multiply(x, y), &name);
        assert_eq!(muldiv::run(muldiv::Op::Mul, 0, y, x), steps, "muldiv::run: {name}");
        for extra in [ALU, OB_RIGHT | Q_RIGHT, OB_LEFT | Q_LOAD] {
            assert_eq!(words(Geometry::QUUX, &one_multiply(x, y, extra), &name), steps, "{name}");
        }
    }
}

/// **On QUUX, `DIV` leaves what a first divide step and 31 divide steps
/// leave**, whatever the output selector and Q control say; and the first
/// step's quotient bit, the one `uc-arith.lisp`'s `DIV` tests for overflow
/// straight after `DVS1`, is in `Q<31>`.
#[test]
fn quux_s_div_is_the_first_and_31_divide_steps() {
    let ops = operands(24);
    let mut cases: Vec<(u32, u32, u32)> = (0..ops.len())
        .map(|k| (ops[k] >> 3, ops[(k * 5 + 1) % ops.len()], ops[(k * 11 + 2) % ops.len()]))
        .collect();
    // Overflow: a divisor of 0, and a high word not below the divisor.
    cases.extend([(0, 100, 0), (5, 0, 3), (7, !0, 7), (0, 100_000, 7)]);
    for (hi, lo, d) in cases {
        let name = format!("{hi:x}:{lo:x} / {d:x}");
        let steps = words(Geometry::CADR, &divide_steps(hi, lo, d, 32), &name);
        assert_eq!(muldiv::run(muldiv::Op::Div, hi, d, lo), steps, "muldiv::run: {name}");
        for extra in [ALU, OB_LEFT | Q_LEFT, OB_RIGHT | Q_RIGHT] {
            assert_eq!(
                words(Geometry::QUUX, &one_divide(hi, lo, d, extra), &name),
                steps,
                "{name}"
            );
        }
        let first = words(Geometry::CADR, &divide_steps(hi, lo, d, 1), &name).1 & 1;
        assert_eq!(steps.1 >> 31, first, "the first quotient bit in Q<31>: {name}");
    }
}

/// **On the CADR, functions 42 and 43 are what the board makes of them**:
/// the 74S139's outputs 2 and 3 are unconnected, so neither multiplies nor
/// divides. `micro` and `rtl` as the CADR agree with the netlist, and QUUX
/// does something else.
#[test]
fn on_the_cadr_42_and_43_are_the_board_s() {
    let n = support::netlists();
    for (name, body, quux) in [
        (
            "42",
            one_multiply(0x1234_5678, 0x9abc, OB_RIGHT | Q_RIGHT),
            one_multiply(0x1234_5678, 0x9abc, ALU),
        ),
        (
            "43",
            one_divide(3, 0x8765_4321, 0x1234, OB_LEFT | Q_LEFT),
            one_divide(3, 0x8765_4321, 0x1234, ALU),
        ),
    ] {
        let mut board = [0; 2];
        for (i, q) in [false, true].into_iter().enumerate() {
            let p = program(body.clone(), q);
            let (mut c, mut clk, mut far) = support::chip(&n);
            benchmark::boot_chip(&mut c, &n.cpu, &mut far, &mut clk, &p);
            board[i] = benchmark::run_chip(&mut c, &n.cpu, &mut far, &mut clk, &p, Stop::Cycles(4))
                .iterations as u32;
        }
        let cadr = words(Geometry::CADR, &body, name);
        assert_eq!([cadr.0, cadr.1], board, "{name}");
        assert_ne!(words(Geometry::QUUX, &quux, name), cadr, "{name}: QUUX's differs");
    }
}

/// When an engine first reaches the program's loop.
fn ns_to_top<E: Engine>(mut e: E, top: u16, ns: fn(&E) -> u64) -> u64 {
    while e.pc() != top {
        e.step().unwrap();
    }
    ns(&e)
}

/// The time to reach the loop, the mode register's speed bits set to
/// `speed` at the start if given.
fn time_on<E: Engine>(
    new: fn(Machine) -> E,
    boot: fn(&mut E),
    ns: fn(&E) -> u64,
    speed: Option<u16>,
    body: &[Insn],
) -> u64 {
    let p = program(body.to_vec(), false);
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
    m.load_prom(&p.prom);
    support::prom_program_in_ram(&mut m);
    let mut e = new(m);
    boot(&mut e);
    if let Some(bits) = speed {
        e.spy_write(muir::spy::MODE, bits);
    }
    ns_to_top(e, p.top, ns)
}

/// **A `DIV` of a register holds its microcycle for `DIV_CYCLES` generator
/// cycles from when it entered `IR`, and a `MUL` holds nothing.** Nine of
/// QUUX's 40 ns (`sync`, four ticks; QUUX drops the delay lines), the speed
/// bits written or not: a register is ready at once, so QUUX's rule,
/// ten microcycles, the `DIV`'s own and nine held after the
/// operands are ready, counts from the `DIV`'s start: 400 ns from `IR` to
/// its closing edge, as muir-fpga's fabric has it.
/// Measured as the time the program takes to reach its loop, against the
/// same program with the one instruction replaced, on both engines.
#[test]
fn a_div_is_held_for_div_cycles_and_a_mul_is_not() {
    fn check<E: Engine>(name: &str, new: fn(Machine) -> E, boot: fn(&mut E), ns: fn(&E) -> u64) {
        // QUUX has no speed bits: one rate, whatever is written there.
        for (speed, want) in [(None, 40), (Some(2), 40), (Some(3), 40)] {
            check_at(name, new, boot, ns, speed, want);
        }
    }
    fn check_at<E: Engine>(
        name: &str,
        new: fn(Machine) -> E,
        boot: fn(&mut E),
        ns: fn(&E) -> u64,
        speed: Option<u16>,
        want_cycle: u64,
    ) {
        let t = |b: &[Insn]| time_on(new, boot, ns, speed, b);
        // `body` with its last instruction an ordinary one.
        let ordinary = |body: &[Insn]| {
            let mut b = body.to_vec();
            *b.last_mut().unwrap() = Insn::new(ALU | SETZ | m_dest(5));
            b
        };
        let mul = one_multiply(5, 7, ALU);
        let mut longer = ordinary(&mul);
        longer.push(Insn::new(ALU | SETZ | m_dest(6)));
        let cycle = t(&longer) - t(&ordinary(&mul));
        assert_eq!(cycle, want_cycle, "{name}: speed {speed:?}");
        assert_eq!(t(&mul), t(&ordinary(&mul)), "{name}: MUL is one ordinary microcycle");
        let div = one_divide(0, 100, 7, ALU);
        let held = t(&div) - t(&ordinary(&div));
        assert_eq!(held, muldiv::DIV_CYCLES * cycle, "{name}: a cycle {cycle} ns");
    }
    check("rtl", Rtl::new, Rtl::boot, Rtl::ns);
    check("micro", Micro::new, Micro::boot, |e: &Micro| e.machine().ns);
}

/// The word main memory holds at 1000, which [`md_program`] reads, and the
/// divisor and low dividend it divides by and into.
const WORD: u32 = 5;
const DIVISOR: u32 = 7;
const LOW: u32 = 0x1234_5678;

/// A program that reads main memory word 1000 and gives `MD` to `insn` in
/// the second microcycle after the read's start, the first `micro` has the
/// word in (`Micro::start_read`), and on `rtl` a miss, the cache's first
/// read, so `insn` waits for the word to land. Q 0 before `insn` is
/// [`LOW`], A 101 [`DIVISOR`]; `insn` writes M 5, and the instruction
/// after it copies `Q` to M 6. Instruction 3 is `insn`, 5 the loop.
fn md_program(insn: Insn) -> Machine {
    let prom = [
        Insn::new(ALU | SETM | m_src(2) | Q_LOAD | m_dest(3)),
        Insn::new(ALU | SETM | m_src(1) | START_READ),
        filler(),
        insn,
        Insn::new(ALU | SETM | SRC_Q | m_dest(6)),
        Insn::new(JUMP | target(5) | ALWAYS),
    ];
    let mut m = Machine::new();
    let mut words = vec![filler(); 512];
    words[..prom.len()].copy_from_slice(&prom);
    m.load_prom(&words);
    m.geometry = Geometry::QUUX;
    support::prom_program_in_ram(&mut m);
    // Level-2 entries 0 to 7: virtual pages onto physical pages 0 to 7,
    // readable and writable.
    for p in 0..8u32 {
        m.l2_map[p as usize] = (1 << 27) | (1 << 26) | p;
    }
    m.main[0o1000] = u64::from(WORD);
    m.mmem[1] = 0o1000;
    m.mmem[2] = u64::from(LOW);
    m.amem[0o101] = u64::from(DIVISOR);
    m
}

/// What [`md_program`] leaves with `insn`: the time from the end of the
/// read's start to the end of `insn`, and M 5 and M 6, the output bus and
/// `Q` of `insn`.
fn md_run<E: Engine>(
    new: fn(Machine) -> E,
    boot: fn(&mut E),
    ns: fn(&E) -> u64,
    executed: fn(&E) -> Option<u16>,
    insn: Insn,
) -> (u64, u32, u32) {
    let mut e = new(md_program(insn));
    boot(&mut e);
    let at = |pc: u16, e: &mut E| {
        for _ in 0..100 {
            e.step().unwrap();
            if executed(e) == Some(pc) {
                return ns(e);
            }
        }
        panic!("{pc} never ran");
    };
    let from = at(1, &mut e);
    let to = at(3, &mut e);
    at(5, &mut e);
    (to - from, support::low(e.machine().mmem[5]), support::low(e.machine().mmem[6]))
}

/// **A `DIV` of `MD` divides the word the read brought, and is held nine
/// microcycles counted from when the word landed**: QUUX's rule,
/// that a `DIV` takes ten microcycles, its own and nine
/// held after its operands are ready, and an `MD` operand is ready when the MD interlock lets go, as
/// for any instruction reading `MD`. muir-fpga's `quux_divmd`, a `DIV` of
/// `MD` in the microcycle after a read's word landed, found `rtl` dividing
/// the new word at once, its hold having run while it waited. A `MUL` of
/// `MD` takes one microcycle after the same wait.
///
/// Measured against the same instruction as a plain copy of `MD`, whose
/// microcycle is the first after the wait: `rtl` waits for a miss, `micro`
/// has the word with no wait; on both a `DIV` ends 9 microcycles of 40 ns
/// after it, ten after the wait, and a `MUL` with it. On `rtl` the read's start ends, the
/// filler runs, the copy ends 13 microcycles later, the line's fill of 5
/// beats taking 410 ns at the nominal timing, and the `DIV` 22.
#[test]
fn a_div_of_md_is_held_nine_microcycles_after_the_word_lands() {
    let div = Insn::new(ALU | DIV | SRC_MD | a_src(0o101) | m_dest(5));
    let mul = Insn::new(ALU | MUL | SRC_MD | a_src(0o101) | m_dest(5));
    let copy = Insn::new(ALU | SETM | SRC_MD | m_dest(5));
    let want_div = muldiv::run(muldiv::Op::Div, WORD, DIVISOR, LOW);
    let want_mul = muldiv::run(muldiv::Op::Mul, WORD, DIVISOR, LOW);
    assert_ne!(want_div, muldiv::run(muldiv::Op::Div, 0, DIVISOR, LOW), "the old MD differs");
    let check = |name: &str, run: &dyn Fn(Insn) -> (u64, u32, u32), copy_cycles: u64| {
        let (t_copy, word, _) = run(copy);
        assert_eq!(word, WORD, "{name}: the copy has the word read");
        assert_eq!(t_copy, copy_cycles * 40, "{name}: the copy's wait");
        let (t_div, ob, q) = run(div);
        assert_eq!((ob, q), want_div, "{name}: DIV of the word read");
        assert_eq!(t_div, t_copy + 9 * 40, "{name}: DIV held nine microcycles after the wait");
        let (t_mul, ob, q) = run(mul);
        assert_eq!((ob, q), want_mul, "{name}: MUL of the word read");
        assert_eq!(t_mul, t_copy, "{name}: MUL one microcycle after the wait");
    };
    check("rtl", &|i| md_run(Rtl::new, Rtl::boot, Rtl::ns, Rtl::executed, i), 13);
    // `micro`'s stand-in for the bus's waits is taken off: its time is
    // the microcycles alone.
    let micro = |m| {
        let mut e = Micro::new(m);
        e.memory_cycle_ns = 0;
        e
    };
    let ns = |e: &Micro| e.machine().ns;
    check("micro", &|i| md_run(micro, Micro::boot, ns, Micro::executed, i), 2);
}

/// **At another microcycle length the divider's count is still nine**: the
/// rule counts microcycles, not nanoseconds, and a change of
/// `--sync-cycle-ticks` is left to a later contract to recount. At three
/// ticks a `DIV` of a register is held nine microcycles of 30 ns, on both
/// engines.
#[test]
fn a_div_is_held_nine_microcycles_at_three_ticks() {
    let body = one_divide(0, 100, 7, ALU);
    let mut ordinary = body.clone();
    *ordinary.last_mut().unwrap() = Insn::new(ALU | SETZ | m_dest(5));
    let quux = |body: &[Insn]| {
        let p = program(body.to_vec(), false);
        let mut m = Machine::new();
        m.geometry = Geometry::QUUX;
        m.load_prom(&p.prom);
        support::prom_program_in_ram(&mut m);
        (m, p.top)
    };
    let rtl = |body: &[Insn]| {
        let (m, top) = quux(body);
        let mut e = Rtl::new(m);
        e.set_timing_model(muir::clock::TimingModel::Sync { cycle_ticks: 3, ilong_ticks: 0 });
        e.boot();
        ns_to_top(e, top, Rtl::ns)
    };
    let micro = |body: &[Insn]| {
        let (m, top) = quux(body);
        let mut e = Micro::new(m);
        e.sync_cycle_ns = 30;
        e.boot();
        ns_to_top(e, top, |e: &Micro| e.machine().ns)
    };
    assert_eq!(rtl(&body) - rtl(&ordinary), 9 * 30, "rtl");
    assert_eq!(micro(&body) - micro(&ordinary), 9 * 30, "micro");
}

/// **On the CADR, function 43 is held for nothing**: an ordinary
/// microcycle, on both engines, where QUUX's divider holds it. What it
/// computes, and the steps, are the netlist's, above.
#[test]
fn on_the_cadr_43_is_not_held() {
    let body = one_divide(0, 100, 7, ALU);
    let mut ordinary = body.clone();
    *ordinary.last_mut().unwrap() = Insn::new(ALU | SETZ | m_dest(5));
    let cadr = |body: &[Insn], rtl: bool| {
        let p = program(body.to_vec(), false);
        let mut m = Machine::new();
        m.load_prom(&p.prom);
        support::prom_program_in_ram(&mut m);
        if rtl {
            let mut e = Rtl::new(m);
            e.boot();
            ns_to_top(e, p.top, Rtl::ns)
        } else {
            let mut e = Micro::new(m);
            e.boot();
            ns_to_top(e, p.top, |e: &Micro| e.machine().ns)
        }
    };
    for rtl in [true, false] {
        assert_eq!(cadr(&body, rtl), cadr(&ordinary, rtl), "rtl: {rtl}");
    }
}

/// **A `DIV` the console stops and then steps finishes right.** The halt
/// comes while the `DIV` stands in `IR`; every microcycle after it is a
/// single step, which the hold does not stop, as it does not stop `-WAIT`:
/// by then the divider has been done for longer than `DIV_CYCLES`.
#[test]
fn a_div_stopped_and_stepped_finishes_right() {
    use muir::spy;
    let (hi, lo, d) = (0x12, 0x3456_789a, 0x9876);
    let want = muldiv::run(muldiv::Op::Div, hi, d, lo);
    for q in [false, true] {
        let body = one_divide(hi, lo, d, ALU);
        let at = body.len() as u16 - 1;
        let p = program(body, q);
        let mut m = Machine::new();
        m.geometry = Geometry::QUUX;
        m.load_prom(&p.prom);
        support::prom_program_in_ram(&mut m);
        let mut e = Rtl::new(m);
        e.boot();
        while e.pc() != at + 1 {
            e.step().unwrap();
        }
        e.spy_write(spy::CLK, 0);
        for _ in 0..8 {
            e.step().unwrap();
        }
        assert_eq!(e.pc(), at + 1, "halted with the DIV in IR");
        // CC-CLOCK, 2 then 0, until the loop's jump has run.
        let mut ran = Vec::new();
        while !ran.contains(&p.top) {
            for clk in [2, 0] {
                e.spy_write(spy::CLK, clk);
                for _ in 0..2 {
                    e.step().unwrap();
                    ran.extend(e.executed());
                }
            }
            assert!(ran.len() < 20, "the steps never reached the loop: {ran:?}");
        }
        assert_eq!(ran[0], at, "the first step runs the DIV");
        let got = e.machine().vma;
        assert_eq!(got, if q { want.1 } else { want.0 }.into(), "Q: {q}");
    }
}

/// **A checkpoint taken with a `DIV` in `IR` resumes with the same hold**:
/// `rtl` keeps when the divider's count started, the edge that loaded `IR`
/// for a `DIV` of a register, checkpoint format 31.
#[test]
fn a_checkpoint_keeps_the_divider_s_time() {
    use muir::checkpoint::{Reader, Writer};
    let body = one_divide(0, 100, 7, ALU);
    let at = body.len() as u16 - 1;
    let p = program(body, false);
    let make = || {
        let mut m = Machine::new();
        m.geometry = Geometry::QUUX;
        m.load_prom(&p.prom);
        support::prom_program_in_ram(&mut m);
        let mut e = Rtl::new(m);
        e.boot();
        e
    };
    let mut e = make();
    while e.pc() != at + 1 {
        e.step().unwrap();
    }
    let mut w = Writer::new();
    e.save(&mut w);
    let body = w.finish();
    let mut resumed = make();
    resumed.load(&mut Reader::for_word_bits(&body, 40)).unwrap();
    assert_eq!(ns_to_top(resumed, p.top, Rtl::ns), ns_to_top(e, p.top, Rtl::ns));
}
