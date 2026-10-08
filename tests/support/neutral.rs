// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **The time-neutral harness** (contract G3 revision 15, §13; MP2b ruling
//! Q12), under `MUIR_TIME_NEUTRAL=1`.
//!
//! - **The time** is `Machine::cycles` times the period on both engines
//!   (`Micro::neutral`, `Pipeline::neutral`): the timers, the RTC, the
//!   interrupts' sampling and the devices see it, and nothing else.
//! - **The harness's own actions**, its keys, screen checks and marker
//!   polls, are on an absolute schedule of `Machine::cycles`
//!   ([`run_for`]), and the host's dates are fixed ([`FIXED_UNIX`]), so
//!   that two runs, on either engine, see the same.
//! - **The digest** ([`digest`]) is taken at every [`EVERY`]th
//!   macroinstruction boundary, a step of LC (`Machine::lc_steps`):
//!   `micro` after the microcycle that steps it, the pipeline halted after
//!   that word, drained (`Pipeline::boundary_at`). Each says
//!   `Machine::cycles`, so two runs' lines compare as they are. At a
//!   workload's end the digest takes main memory too.
//!
//! Outside the switch nothing changes.

use std::cell::Cell;

use muir::engine::Engine;
use muir::machine::{Halt, Machine, Pending, Word};
use muir::micro::Micro;
use muir::pipeline::Pipeline;

/// The boundaries between two digests.
pub const EVERY: u64 = 65_536;

/// The host's date under the harness, 1 September 2026, the one M8d fixed.
pub const FIXED_UNIX: u64 = super::time::FIXED_UNIX;

/// Whether the harness is on: `MUIR_TIME_NEUTRAL=1`.
pub fn on() -> bool {
    super::time::neutral()
}

thread_local! {
    /// The schedule's next point, in `Machine::cycles`.
    static SCHEDULE: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Runs `step` `n` times; under the harness, until the machine's
/// microcycles reach a schedule that moves on by `n` at each call whatever
/// the steps took, so that the harness acts at the same `Machine::cycles`
/// on every engine.
pub fn run_for<E: Engine>(e: &mut E, n: u64, step: &mut dyn FnMut(&mut E)) {
    if !on() {
        for _ in 0..n {
            step(e);
        }
        return;
    }
    let now = e.machine().cycles;
    let at = SCHEDULE.with(|s| {
        let at = s.get().unwrap_or(now) + n;
        s.set(Some(at));
        at
    });
    let mut k = 0u64;
    while e.machine().cycles < at {
        step(e);
        k += 1;
        assert!(k < 4 * n + 1_000_000, "the schedule's microcycles never came");
    }
}

/// What the harness needs of an engine beyond [`Engine`].
pub trait Neutral: Engine {
    /// The writes handed to the next microcycle, which the state between
    /// two microcycles has landed.
    fn pending(&self) -> Pending;
    /// OA-REG-LOW and OA-REG-HIGH.
    fn oa(&self) -> (u64, u64);
    /// Asks for the digest at the step of LC that brings
    /// `Machine::lc_steps` to `at`.
    fn arm(&mut self, at: u64);
    /// Whether the state is the one to digest for `at`.
    fn reached(&self, at: u64) -> bool;
    /// Neutral time on.
    fn time_neutral(&mut self);
}

impl Neutral for Micro {
    fn pending(&self) -> Pending {
        Micro::pending(self)
    }
    fn oa(&self) -> (u64, u64) {
        self.oa_registers()
    }
    fn arm(&mut self, _: u64) {}
    fn reached(&self, at: u64) -> bool {
        self.machine().lc_steps >= at
    }
    fn time_neutral(&mut self) {
        self.neutral = true;
    }
}

impl Neutral for Pipeline {
    fn pending(&self) -> Pending {
        Pipeline::pending(self)
    }
    fn oa(&self) -> (u64, u64) {
        self.oa_registers()
    }
    fn arm(&mut self, at: u64) {
        self.boundary_at = Some(at);
    }
    fn reached(&self, _: u64) -> bool {
        self.at_boundary()
    }
    fn time_neutral(&mut self) {
        self.neutral = true;
    }
}

/// FNV-1a, 64 bits.
struct Fnv(u64);

impl Fnv {
    fn word(&mut self, v: Word) {
        for b in v.to_le_bytes() {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
        }
    }
    fn words(&mut self, v: impl IntoIterator<Item = Word>) {
        for w in v {
            self.word(w);
        }
    }
}

/// **The digest** (MP2b ruling Q12 (e)): M8d's fields without the time,
/// with `Machine::cycles`, the macroinstruction count, the OA registers,
/// the MACRO-DISPATCH state and D's armed word; with `whole`, main memory
/// and the dispatch memory too.
pub fn digest(m: &Machine, p: Pending, oa: (u64, u64), whole: bool) -> u64 {
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    let md = p.md.unwrap_or(m.md);
    h.words([m.cycles, m.lc_steps, m.lc, m.vma, md, m.q]);
    h.words([m.pdl_pointer.into(), m.pdl_index.into(), m.spcptr.into()]);
    h.words([m.interrupt_control.into(), m.vmaok.into(), oa.0, oa.1]);
    h.words(m.mmem.iter().copied());
    h.words(m.amem.iter().copied());
    h.words(m.pdl.iter().enumerate().map(|(k, &w)| match p.pdl {
        Some((a, v)) if a as usize == k => v,
        _ => w,
    }));
    h.words(m.spc.iter().enumerate().map(|(k, &w)| match p.spc {
        Some((a, v)) if a as usize == k => Word::from(v),
        _ => Word::from(w),
    }));
    let d = &m.macro_dispatch;
    h.words([d.register.into(), d.index.into(), d.localp.into(), d.ap.into()]);
    h.words([
        d.operand.map_or(0, |o| 1 | u64::from(o.arg) << 1 | u64::from(o.delta) << 2),
        d.m31.map_or(0, |w| 1 | w << 1),
    ]);
    if whole {
        h.words(m.main.iter().copied());
        h.words(m.dmem.iter().map(|&v| Word::from(v)));
        h.words(d.entries.iter().map(|&v| Word::from(v)));
    }
    h.0
}

/// An engine whose run is digested at every [`Digests::every`]th step of
/// LC, and at a workload's end ([`Digests::end_of`]).
pub struct Digests<E> {
    pub engine: E,
    /// The steps of LC between two digests: [`EVERY`] for the harness.
    pub every: u64,
    next: u64,
    /// The workload whose end is digested at the next step of LC.
    end: Option<String>,
    /// The digests' lines, in order, when kept.
    pub lines: Vec<String>,
    /// Whether each line is printed too.
    pub print: bool,
}

impl<E: Neutral> Digests<E> {
    /// `engine` digested every `every` steps of LC, from its count now.
    pub fn new(mut engine: E, every: u64) -> Digests<E> {
        let next = engine.machine().lc_steps + every;
        engine.arm(next);
        Digests { engine, every, next, end: None, lines: Vec::new(), print: false }
    }

    /// The end of the workload `name`: digested whole at the next step of
    /// LC, the run stepped to it.
    pub fn end_of(&mut self, name: &str) {
        self.end = Some(name.to_string());
        self.next = self.engine.machine().lc_steps + 1;
        self.engine.arm(self.next);
        let mut k = 0u64;
        while self.end.is_some() {
            self.step().expect("halted before the workload's end was digested");
            k += 1;
            assert!(k < 100_000_000, "no step of LC after the workload");
        }
    }

    fn after_step(&mut self) {
        if !self.engine.reached(self.next) {
            return;
        }
        let m = self.engine.machine();
        let pending = self.engine.pending();
        let (what, whole) = match self.end.take() {
            Some(name) => (format!("end {name}"), true),
            None => (format!("boundary {}", self.next / self.every), false),
        };
        let line = format!(
            "digest {what}: cycles {} lc_steps {} {:016x}",
            m.cycles,
            m.lc_steps,
            digest(m, pending, self.engine.oa(), whole)
        );
        if self.print {
            println!("{line}");
        }
        self.lines.push(line);
        self.next = (m.lc_steps / self.every + 1) * self.every;
        self.engine.arm(self.next);
    }
}

impl<E: Neutral + super::macro_dispatch::Executes> super::macro_dispatch::Executes for Digests<E> {
    fn executed(&self) -> Option<u16> {
        self.engine.executed()
    }
    fn fetch_started(&self) -> Option<bool> {
        self.engine.fetch_started()
    }
}

impl<E: Neutral> Engine for Digests<E> {
    fn boot(&mut self) {
        self.engine.boot();
        self.next = self.engine.machine().lc_steps + self.every;
        self.engine.arm(self.next);
    }
    fn keyboard_boot(&mut self) -> bool {
        self.engine.keyboard_boot()
    }
    fn step(&mut self) -> Result<(), Halt> {
        let r = self.engine.step();
        self.after_step();
        r
    }
    fn nominal_cycle_ns(&self) -> u64 {
        self.engine.nominal_cycle_ns()
    }
    fn pc(&self) -> u16 {
        self.engine.pc()
    }
    fn pc_is_a_write(&self) -> bool {
        self.engine.pc_is_a_write()
    }
    fn lc(&self) -> u32 {
        self.engine.lc()
    }
    fn lc_wide(&self) -> u64 {
        self.engine.lc_wide()
    }
    fn spy_read(&self, eadr: u8) -> u16 {
        self.engine.spy_read(eadr)
    }
    fn spy_write(&mut self, eadr: u8, v: u16) {
        self.engine.spy_write(eadr, v)
    }
    fn machine(&self) -> &Machine {
        self.engine.machine()
    }
    fn machine_mut(&mut self) -> &mut Machine {
        self.engine.machine_mut()
    }
    fn save(&self, w: &mut muir::checkpoint::Writer) {
        self.engine.save(w)
    }
    fn load(&mut self, r: &mut muir::checkpoint::Reader) -> std::io::Result<()> {
        self.engine.load(r)
    }
}
