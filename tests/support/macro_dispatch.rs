// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **Checkers for QUUX's fused return and operand address** (contract H8a,
//! §6 items 4 and 5), outside the engines: they watch a run
//! microcycle by microcycle and count what breaks the contract, and the
//! engines never consult them.
//!
//! - **The rule of §3.3.** The microcycle after a fused return, the one an
//!   XCT-NEXT runs or a nopped one, must not write the location counter,
//!   M 31, INTERRUPT-CONTROL (whose byte mode chooses the halfword) or
//!   functional destinations 5 to 7, and, when the entry has the operand
//!   bit, PDL-INDEX, `A-LOCALP` or `M-AP`: the hardware has chosen the
//!   handler and the operand address by then. The instruction that ran
//!   there is decoded from the control store, independently of the
//!   engines' own decode ([`writes`]).
//! - **What the main loop would have done.** After that microcycle the
//!   handler the MACRO DISPATCH MEMORY names for the halfword the main
//!   loop's dispatch would take --- M 31 by the location counter's `<1>`,
//!   as stepped --- is the next to run, the micro stack's pointer is where
//!   the return left it, and, with the operand bit, PDL-INDEX holds
//!   `A-LOCALP` + delta or `M-AP` + 1 + delta read from A and M memory.
//! - **The base copies** equal A memory at the register's `<23:14>` and
//!   M memory at its `<28:24>`, fourteen bits, after every microcycle. The
//!   register's write does not load them, so after it names another
//!   address a copy is held to its memory once the first write of that word
//!   has landed ([`writes`] decodes it), from the microcycle after the one
//!   that wrote it.
//! - **No PDL read of the write after a return.** The handler's first
//!   microinstruction does not read the PDL buffer at the address the
//!   microcycle after the return writes, at the pointer or by PDL-INDEX:
//!   the buffer has no pass-around, and that word lands in the handler's
//!   own write pulse, after its read, where today's path runs the main
//!   loop's dispatch and push in between.
//!
//! - **The prefetched word** (contract H8a §3.5, on `rtl`): a
//!   fused return that needs a fetch took its word from the prefetch's
//!   buffer, and the microcycle after it starts the stream's fetch of that
//!   word. After it, M 31 holds main memory's word at the
//!   fetch's address, translated through the map as it then stands: a
//!   buffer that missed a store, a transfer or a map write fails this.
//! - **Returns by a handler a fused return ran**, with no main loop
//!   between, are counted by that handler: the returns that let the rule's
//!   check reach a specialised handler's own XCT-NEXT slot.
//!
//! [`Checked`] wraps an engine and runs the checker after every step.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use muir::engine::Engine;
use muir::isa::{Insn, Op};
use muir::machine::{Halt, Machine, macro_dispatch};
use muir::micro::Micro;
use muir::rtl::Rtl;

/// An engine that says which control-store address it executed in its last
/// step, `None` when that microcycle was nopped, and whether that step
/// started a macroinstruction fetch, where the engine says.
pub trait Executes: Engine {
    fn executed(&self) -> Option<u16>;
    fn fetch_started(&self) -> Option<bool> {
        None
    }
}

impl Executes for Micro {
    fn executed(&self) -> Option<u16> {
        Micro::executed(self)
    }
}

impl Executes for muir::pipeline::Pipeline {
    fn executed(&self) -> Option<u16> {
        muir::pipeline::Pipeline::executed(self)
    }
}

impl Executes for Rtl {
    fn executed(&self) -> Option<u16> {
        Rtl::executed(self)
    }
    fn fetch_started(&self) -> Option<bool> {
        Rtl::fetch_started(self)
    }
}

/// A write the microcycle after a fused return may not make (contract H8a
/// §3.3). The last three only when the entry has the operand bit.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Write {
    /// Functional destination 1, the location counter.
    Lc,
    /// M memory 31, `M-INST-BUFFER`.
    M31,
    /// Functional destination 2, INTERRUPT-CONTROL, whose `<29>` is the
    /// byte mode.
    InterruptControl,
    /// Functional destinations 5 to 7, the MACRO-DISPATCH register and the
    /// MACRO DISPATCH MEMORY.
    MacroDispatch,
    /// Functional destination 13, PDL-INDEX.
    PdlIndex,
    /// Functional destination 12, the PDL buffer at PDL-INDEX: the word is
    /// written in the next microcycle's write phase, at PDL-INDEX as it
    /// then stands (`PWIDX`, `Rtl::write_phase`), after the operand
    /// address has been loaded.
    PdlAtIndex,
    /// A memory at the register's `<23:14>`, `A-LOCALP`.
    Localp,
    /// M memory at the register's `<28:24>`, `M-AP`.
    Ap,
}

impl Write {
    /// Forbidden only when the entry has the operand bit.
    pub fn operand_only(self) -> bool {
        matches!(self, Write::PdlIndex | Write::PdlAtIndex | Write::Localp | Write::Ap)
    }
}

/// **The writes of [`Write`]'s kinds that instruction `i` makes**, with the
/// MACRO-DISPATCH register at `register` naming the bases. Decoded as
/// `mit/cadr/ir.bits` lays the word out: only the ALU and BYTE classes
/// write; `IR<25>` set is an A destination at `IR<23:14>`, and clear is an
/// M destination at `IR<18:14>`, which writes the shadowing A word too,
/// with the functional destination `IR<23:19>` (`IR<24>` in no decode).
/// Functional destinations 1 and 2 are the location counter and
/// INTERRUPT-CONTROL, 13 PDL-INDEX (`ir.bits`' `FUNCTIONAL DESTINATIONS`),
/// and 5 to 7 QUUX's.
pub fn writes(i: Insn, register: u32) -> Vec<Write> {
    let mut w = Vec::new();
    if !matches!(i.op(), Op::Alu | Op::Byte) {
        return w;
    }
    let ir = i.raw();
    let localp = macro_dispatch::localp_address(register) as u64;
    let ap = macro_dispatch::ap_address(register) as u64;
    if ir >> 25 & 1 != 0 {
        if ir >> 14 & 0o1777 == localp {
            w.push(Write::Localp);
        }
        return w;
    }
    match ir >> 19 & 0o37 {
        0o1 => w.push(Write::Lc),
        0o2 => w.push(Write::InterruptControl),
        0o5..=0o7 => w.push(Write::MacroDispatch),
        0o12 => w.push(Write::PdlAtIndex),
        0o13 => w.push(Write::PdlIndex),
        _ => {}
    }
    let m = ir >> 14 & 0o37;
    if m == 0o31 {
        w.push(Write::M31);
    }
    if m == localp {
        w.push(Write::Localp);
    }
    if m == ap {
        w.push(Write::Ap);
    }
    w
}

/// Where a write was made: the return's address and the address of the
/// microcycle after it.
pub type Site = (Write, u16, u16);

/// How the microcycle after a fused return addresses its PDL buffer write:
/// functional destination 10 or 11, the push, at the pointer, or 12 at
/// PDL-INDEX (`ir.bits`' `FUNCTIONAL DESTINATIONS`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum PdlAt {
    Pointer,
    Index,
}

/// Where a handler's first microinstruction read the PDL buffer at the
/// address the microcycle before it wrote: how that write was addressed,
/// the return's address, the microcycle after it, and the handler's.
pub type ReadSite = (PdlAt, u16, u16, u16);

/// What the checker has counted.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Counts {
    /// Fused returns seen.
    pub fused: u64,
    /// Microcycles after a fused return that were nopped.
    pub nopped_after: u64,
    /// Fused returns whose entry had the operand bit and a LOCAL or ARG
    /// register, whose PDL-INDEX was checked.
    pub operand_loads: u64,
    /// Fused returns into a halfword [`has_an_operand`] names, with a LOCAL
    /// or ARG register, whatever the entry: those the operand bit could
    /// serve.
    pub operand_candidates: u64,
    /// Forbidden writes in the microcycle after a fused return, by kind and
    /// site: the rule of §3.3 broken.
    pub violations: BTreeMap<Site, u64>,
    /// Writes of PDL-INDEX, `A-LOCALP` or `M-AP` there whose entry had no
    /// operand bit: allowed, and what a specialised handler with the bit
    /// would break.
    pub unarmed: BTreeMap<Site, u64>,
    /// The micro stack's pointer moved in the microcycle after.
    pub stack_moved: u64,
    /// The next microcycle was not the handler the main loop's dispatch
    /// would have reached.
    pub wrong_handler: u64,
    /// PDL-INDEX was not the operand address after the microcycle after.
    pub wrong_operand: u64,
    /// Microcycles after which a base copy differed from its memory.
    pub copies_differ: u64,
    /// PDL buffer writes made in the microcycle after a fused return.
    pub pdl_writes_after: u64,
    /// Reads of the PDL buffer, by the handler's first microinstruction, at
    /// the address the microcycle after the return wrote, by site: the word
    /// lands after the read, which finds the old one.
    pub pdl_reads_of_writes: BTreeMap<ReadSite, u64>,
    /// Fused returns on the fetch path, which took the prefetched word, and
    /// of them those after which M 31 was not main memory's word at the
    /// fetch's address.
    pub prefetched: u64,
    pub stale_words: u64,
    /// Microcycles after a fused return on the fetch path that read A or
    /// M 31: they find the old word, as on today's path, M 31 being loaded
    /// at the end of that microcycle.
    pub m31_reads_after: u64,
    /// Fused returns made by a handler a fused return ran, with no main
    /// loop between, by the handler's address, and by the returning
    /// microinstruction's.
    pub handler_returns: BTreeMap<u16, u64>,
    pub handler_return_sites: BTreeMap<u16, u64>,
    /// The first problem found, said in words.
    pub first: Option<String>,
}

impl Counts {
    /// Everything that breaks the contract: zero on a run that keeps it.
    pub fn problems(&self) -> u64 {
        self.violations.values().sum::<u64>()
            + self.stack_moved
            + self.wrong_handler
            + self.wrong_operand
            + self.copies_differ
            + self.pdl_reads_of_writes.values().sum::<u64>()
            + self.stale_words
    }

    /// What was counted after `earlier`, a clone of these counts.
    pub fn since(&self, earlier: &Counts) -> Counts {
        fn diff<K: Ord + Copy>(
            now: &BTreeMap<K, u64>,
            then: &BTreeMap<K, u64>,
        ) -> BTreeMap<K, u64> {
            now.iter()
                .map(|(k, &n)| (*k, n - then.get(k).copied().unwrap_or(0)))
                .filter(|&(_, n)| n > 0)
                .collect()
        }
        Counts {
            fused: self.fused - earlier.fused,
            nopped_after: self.nopped_after - earlier.nopped_after,
            operand_loads: self.operand_loads - earlier.operand_loads,
            operand_candidates: self.operand_candidates - earlier.operand_candidates,
            violations: diff(&self.violations, &earlier.violations),
            unarmed: diff(&self.unarmed, &earlier.unarmed),
            stack_moved: self.stack_moved - earlier.stack_moved,
            wrong_handler: self.wrong_handler - earlier.wrong_handler,
            wrong_operand: self.wrong_operand - earlier.wrong_operand,
            copies_differ: self.copies_differ - earlier.copies_differ,
            pdl_writes_after: self.pdl_writes_after - earlier.pdl_writes_after,
            pdl_reads_of_writes: diff(&self.pdl_reads_of_writes, &earlier.pdl_reads_of_writes),
            prefetched: self.prefetched - earlier.prefetched,
            stale_words: self.stale_words - earlier.stale_words,
            m31_reads_after: self.m31_reads_after - earlier.m31_reads_after,
            handler_returns: diff(&self.handler_returns, &earlier.handler_returns),
            handler_return_sites: diff(&self.handler_return_sites, &earlier.handler_return_sites),
            first: if earlier.first.is_none() { self.first.clone() } else { None },
        }
    }

    /// The counts in a few lines, with `label` naming a control-store
    /// address.
    pub fn report(&self, label: impl Fn(u16) -> String) -> String {
        let mut s = format!(
            "fused {}, nopped after {}, operand loads {}, LOCAL or ARG operands {}, violations {}, stack moved {}, wrong handler {}, wrong operand {}, copies differ {}, PDL writes after {}, handler's PDL reads of them {}",
            self.fused,
            self.nopped_after,
            self.operand_loads,
            self.operand_candidates,
            self.violations.values().sum::<u64>(),
            self.stack_moved,
            self.wrong_handler,
            self.wrong_operand,
            self.copies_differ,
            self.pdl_writes_after,
            self.pdl_reads_of_writes.values().sum::<u64>()
        );
        if self.prefetched > 0 || self.stale_words > 0 {
            let _ = write!(
                s,
                ", prefetched {}, stale words {}, M 31 read after {}",
                self.prefetched, self.stale_words, self.m31_reads_after
            );
        }
        let _ = write!(s, ", handlers' own returns {}", self.handler_returns.values().sum::<u64>());
        for (what, map) in [("violation", &self.violations), ("unarmed", &self.unarmed)] {
            for ((w, ret, after), n) in map {
                let _ = write!(
                    s,
                    "\n     {what} {w:?}: return at {ret:o} ({}), after it {after:o} ({}): {n}",
                    label(*ret),
                    label(*after)
                );
            }
        }
        for ((at, ret, after, handler), n) in &self.pdl_reads_of_writes {
            let _ = write!(
                s,
                "\n     PDL read of the write by {at:?}: return at {ret:o} ({}), after it {after:o} ({}), handler {handler:o} ({}): {n}",
                label(*ret),
                label(*after),
                label(*handler)
            );
        }
        if let Some(f) = &self.first {
            let _ = write!(s, "\n     first: {f}");
        }
        s
    }
}

/// A PDL buffer write the microcycle after a fused return made, which lands
/// in the handler's write pulse.
#[derive(Clone, Copy)]
struct Pending {
    at: PdlAt,
    /// The return's address and the microcycle after it.
    ret: u16,
    after: u16,
    /// Where it lands: the pointer or PDL-INDEX as that microcycle left
    /// them, the operand address loaded.
    adr: u16,
    /// The pointer and PDL-INDEX the handler's read finds.
    pointer: u16,
    index: u16,
}

/// **The PDL buffer write instruction `i` makes**, and how it is addressed:
/// functional destination 10 (`C-PDL-BUFFER-POINTER`) and 11 (the push) at
/// the pointer, 12 (`C-PDL-BUFFER-INDEX`) at PDL-INDEX, in the ALU and BYTE
/// classes with `IR<25>` clear, as [`writes`] decodes them.
pub fn pdl_write(i: Insn) -> Option<PdlAt> {
    let ir = i.raw();
    if !matches!(i.op(), Op::Alu | Op::Byte) || ir >> 25 & 1 != 0 {
        return None;
    }
    match ir >> 19 & 0o37 {
        0o10 | 0o11 => Some(PdlAt::Pointer),
        0o12 => Some(PdlAt::Index),
        _ => None,
    }
}

/// **The PDL buffer read instruction `i` makes**, and how it is addressed:
/// functional source 5 (`C-PDL-BUFFER-INDEX`) or 4 (MIT's illegal pop by
/// the index), and 25 and 24 (`C-PDL-BUFFER-POINTER` and its pop), read by
/// the pointer: `IR<31>` with `IR<29:26>` 4 or 5, `IR<30>` choosing the
/// pointer (page PDLCTL's `PDLP`). Every class reads its M source.
pub fn pdl_read(i: Insn) -> Option<PdlAt> {
    if !i.m_src_functional() || !matches!(i.m_src() & 0o17, 4 | 5) {
        return None;
    }
    Some(if i.m_src() & 0o20 != 0 { PdlAt::Pointer } else { PdlAt::Index })
}

/// The fused return the next microcycle follows.
#[derive(Clone, Copy)]
struct After {
    /// The return's address, or `u16::MAX` if the engine did not say.
    at: u16,
    /// The micro stack's pointer after it.
    spcptr: u8,
}

/// **Whether instruction `i` reads A or M memory 31**, `M-INST-BUFFER`: its
/// M source `IR<30:26>` when `IR<31>` is clear, or its A source
/// `IR<41:32>` in every class but DISPATCH, which has none.
pub fn reads_m31(i: Insn) -> bool {
    (!i.m_src_functional() && i.m_src() == 0o31) || (i.op() != Op::Dispatch && i.a_src() == 0o31)
}

/// The checker's state between two microcycles.
#[derive(Clone, Default)]
pub struct Checker {
    cycles: u64,
    fused: u64,
    after: Option<After>,
    /// The handler the next microcycle must run.
    handler: Option<u16>,
    /// The PDL buffer write the microcycle after a fused return made.
    pending: Option<Pending>,
    /// The handler a fused return ran, while it runs: from its first
    /// microinstruction to the main loop's.
    in_handler: Option<u16>,
    /// The addresses the register named when the copies were last seen;
    /// whether each copy is held to its memory; and, where it is not yet,
    /// the microcycle from which it is, the one after the first write of
    /// its word.
    bases: Option<(usize, usize)>,
    localp_held: bool,
    ap_held: bool,
    localp_due: Option<u64>,
    ap_due: Option<u64>,
    /// How many problems have been said on standard error.
    said: u32,
    pub counts: Counts,
}

impl Checker {
    /// A checker for `m` as it stands: nothing it did before is counted.
    pub fn new(m: &Machine) -> Checker {
        Checker {
            cycles: m.cycles,
            fused: m.macro_dispatch.fused,
            localp_held: true,
            ap_held: true,
            ..Checker::default()
        }
    }

    /// The first problem is kept, and the first few said on standard
    /// error as they are found, so that a run that goes on to fail says
    /// what the checkers saw first.
    fn problem(&mut self, what: impl FnOnce() -> String) {
        if self.counts.first.is_none() || self.said < 5 {
            let w = what();
            eprintln!("checkers: microcycle {}: {w}", self.cycles);
            self.said += 1;
            self.counts.first.get_or_insert(w);
        }
    }

    /// After a step of `e`: nothing unless it completed a microcycle.
    pub fn after_step<E: Executes>(&mut self, e: &E) {
        let m = e.machine();
        if m.cycles == self.cycles {
            return;
        }
        self.cycles = m.cycles;
        if !m.geometry.macro_dispatch {
            return;
        }
        let d = &m.macro_dispatch;
        let base = macro_dispatch::BASE_BITS;
        let localp_at = macro_dispatch::localp_address(d.register);
        let ap_at = macro_dispatch::ap_address(d.register);
        let (localp, ap) = (m.amem[localp_at] & u64::from(base), m.mmem[ap_at] & u64::from(base));
        if self.bases.is_some_and(|b| b != (localp_at, ap_at)) {
            // The register names other words: its write loaded nothing.
            (self.localp_held, self.ap_held) = (false, false);
            (self.localp_due, self.ap_due) = (None, None);
        }
        self.bases = Some((localp_at, ap_at));
        // A write lands by the end of the next microcycle, on `rtl` in its
        // write pulse.
        self.localp_held |= self.localp_due.is_some_and(|c| m.cycles >= c);
        self.ap_held |= self.ap_due.is_some_and(|c| m.cycles >= c);
        if let Some(pc) = e.executed() {
            for w in writes(m.imem[pc as usize], d.register) {
                match w {
                    Write::Localp if self.localp_due.is_none() => {
                        self.localp_due = Some(m.cycles + 1)
                    }
                    Write::Ap if self.ap_due.is_none() => self.ap_due = Some(m.cycles + 1),
                    _ => {}
                }
            }
        }
        if (self.localp_held && u64::from(d.localp) != localp)
            || (self.ap_held && u64::from(d.ap) != ap)
        {
            self.counts.copies_differ += 1;
            let (cl, ca) = (d.localp, d.ap);
            self.problem(|| {
                format!(
                    "after microcycle {}: copies {cl:o} {ca:o}, memory {localp:o} {ap:o}",
                    m.cycles
                )
            });
        }
        if let Some(p) = self.pending.take()
            && let Some(pc) = e.executed()
            && let Some(read) = pdl_read(m.imem[pc as usize])
            && p.adr == if read == PdlAt::Pointer { p.pointer } else { p.index }
        {
            *self.counts.pdl_reads_of_writes.entry((p.at, p.ret, p.after, pc)).or_default() += 1;
            self.problem(|| {
                format!(
                    "the handler at {pc:o} reads the PDL at {:o}, which {:o} after the return at {:o} writes",
                    p.adr, p.after, p.ret
                )
            });
        }
        let main = macro_dispatch::main(d.register) as u16;
        if e.executed().is_some_and(|pc| pc == main || pc == main + 2) {
            self.in_handler = None;
        }
        if let Some(h) = self.handler.take() {
            if e.executed() == Some(h) {
                self.in_handler = Some(h);
            } else {
                self.counts.wrong_handler += 1;
                let ran = e.executed().map_or("nothing".into(), |pc| format!("{pc:o}"));
                let cycles = m.cycles;
                self.problem(|| format!("handler {h:o} expected, {ran} ran, microcycle {cycles}"));
            }
        }
        if let Some(a) = self.after.take() {
            // The halfword the main loop's dispatch would take: M 31, by
            // the counter as the return's `NEXT INSTR` has stepped it, `<1>`
            // set for the word's `<15:0>` (the profile's decode, held by its
            // `decode agrees` count).
            let word = m.mmem[0o31];
            let half = if e.lc() & 2 != 0 { word & 0xffff } else { word >> 16 };
            let index = (half >> 6) as usize & (macro_dispatch::ENTRIES - 1);
            let entry = d.entries[index];
            let register = half >> 6 & 7;
            let armed = entry & macro_dispatch::OPERAND != 0
                && (register == macro_dispatch::LOCAL.into()
                    || register == macro_dispatch::ARG.into());
            self.handler = Some((entry & 0o37777) as u16);
            if (register == macro_dispatch::LOCAL.into() || register == macro_dispatch::ARG.into())
                && has_an_operand(index)
            {
                self.counts.operand_candidates += 1;
            }
            // On the fetch path the word came from the prefetch, and the
            // stream's fetch of it has just started: M 31 is to be main
            // memory's word there, through the map as it stands.
            if e.fetch_started() == Some(false) {
                self.counts.prefetched += 1;
                let phys = m.translate(m.vma as u32).physical as usize;
                let held = m.main.get(phys).copied();
                if held != Some(word) {
                    self.counts.stale_words += 1;
                    let vma = m.vma;
                    self.problem(|| {
                        format!(
                            "M 31 {word:o} after the return at {:o}, main memory {held:?} at VMA {vma:o}",
                            a.at
                        )
                    });
                }
                if e.executed().is_some_and(|pc| reads_m31(m.imem[pc as usize])) {
                    self.counts.m31_reads_after += 1;
                }
            }
            match e.executed() {
                None => self.counts.nopped_after += 1,
                Some(pc) => {
                    if let Some(at) = pdl_write(m.imem[pc as usize]) {
                        self.counts.pdl_writes_after += 1;
                        let (pointer, index) = (m.pdl_pointer, m.pdl_index);
                        let adr = if at == PdlAt::Pointer { pointer } else { index };
                        self.pending =
                            Some(Pending { at, ret: a.at, after: pc, adr, pointer, index });
                    }
                    for w in writes(m.imem[pc as usize], d.register) {
                        let site = (w, a.at, pc);
                        if w.operand_only() && entry & macro_dispatch::OPERAND == 0 {
                            *self.counts.unarmed.entry(site).or_default() += 1;
                        } else {
                            *self.counts.violations.entry(site).or_default() += 1;
                            self.problem(|| {
                                format!(
                                    "{w:?} in the microcycle after the return at {:o}, at {pc:o}",
                                    a.at
                                )
                            });
                        }
                    }
                }
            }
            if m.spcptr != a.spcptr {
                self.counts.stack_moved += 1;
                let now = m.spcptr;
                let after = e.executed().map_or("nothing".into(), |pc| format!("{pc:o}"));
                self.problem(|| {
                    format!(
                        "stack pointer {:o} after the return at {:o}, {now:o} after {after}",
                        a.spcptr, a.at
                    )
                });
            }
            if armed {
                self.counts.operand_loads += 1;
                let delta = half & 0o77;
                let want =
                    if register == macro_dispatch::ARG.into() { ap + 1 } else { localp } + delta;
                let want = want as u16 & m.geometry.pdl_mask();
                if m.pdl_index != want {
                    self.counts.wrong_operand += 1;
                    let got = m.pdl_index;
                    self.problem(|| {
                        format!("PDL-INDEX {got:o} for halfword {half:o}, {want:o} expected")
                    });
                }
            }
        }
        if d.fused != self.fused {
            assert_eq!(d.fused, self.fused + 1, "one fused return a microcycle");
            self.fused = d.fused;
            self.counts.fused += 1;
            if let Some(h) = self.in_handler.take() {
                *self.counts.handler_returns.entry(h).or_default() += 1;
                let at = e.executed().unwrap_or(u16::MAX);
                *self.counts.handler_return_sites.entry(at).or_default() += 1;
            }
            self.after = Some(After { at: e.executed().unwrap_or(u16::MAX), spcptr: m.spcptr });
        }
    }
}

/// **Microcode 2001's opcodes whose halfword's `<8:0>` is a register and a
/// delta** whatever its sub-opcode: CALL to ND3, 0 to 13, and their twins
/// with `<13>` set, 31 to 33 (`OPDTB`, `uc-macrocode.lisp:95-126`). Not
/// BRANCH (14, 34) or MISC (15, 35), whose `<8:0>` is a displacement and a
/// function number, nor AREFI-NEW (20) or the unused codes. ND4 (16, 36)
/// has one only for some sub-opcodes, [`ND4_OPERAND_SUB_OPCODES`].
pub const OPERAND_OPCODES: [u32; 15] =
    [0, 1, 2, 3, 4, 5, 6, 7, 0o10, 0o11, 0o12, 0o13, 0o31, 0o32, 0o33];

/// ND4's opcodes, 16 and 36: `<13>` is the low bit of the sub-opcode.
pub const ND4_OPCODES: [u32; 2] = [0o16, 0o36];

/// **ND4's sub-opcodes, `<15:13>`, whose `<8:0>` is a register and a
/// delta** (`D-ND4`, `uc-macrocode.lisp`; `M-INST-SUB-OPCODE`,
/// `uc-parameters.lisp`): `PUSH-CDR-IF-CAR-EQUAL` (5), which fetches its
/// operand by `QADCM4`, and `PUSH-CDR-STORE-CAR-IF-CONS` (6), which stores
/// by `STOCYC`'s `QADCM2`. Not `PUSH-NUMBER` (3), whose `<8:0>` is an
/// immediate; not `STACK-CLOSURE-DISCONNECT` (0), `-UNSHARE` (1),
/// `MAKE-STACK-CLOSURE` (2) or `STACK-CLOSURE-DISCONNECT-FIRST` (4), whose
/// `<8:0>` is a local's offset, `M-INST-ADR` (`uc-stack-closure.lisp`); not
/// 7, `ILLOP`.
pub const ND4_OPERAND_SUB_OPCODES: [u32; 2] = [5, 6];

/// Whether the halfword whose `<15:6>` is `index` has a register and a
/// delta in its `<8:0>`: what [`fill_generic`] gives the operand bit.
pub fn has_an_operand(index: usize) -> bool {
    let op = (index >> 3 & 0o37) as u32;
    OPERAND_OPCODES.contains(&op)
        || (ND4_OPCODES.contains(&op)
            && ND4_OPERAND_SUB_OPCODES.contains(&((index >> 7) as u32 & 7)))
}

/// **The register written from outside the datapath**, as a test or the
/// profile writes it, with the base copies set from A and M memory as they
/// stand: the machine's copies follow only the writes of `A-LOCALP` and
/// `M-AP`, and the register's write loads nothing (contract H8a §3.4).
pub fn set_register(m: &mut Machine, word: u32) {
    let base = macro_dispatch::BASE_BITS;
    let d = &mut m.macro_dispatch;
    d.register = word;
    d.localp = m.amem[macro_dispatch::localp_address(word)] as u32 & base;
    d.ap = m.mmem[macro_dispatch::ap_address(word)] as u32 & base;
}

/// **The MACRO DISPATCH MEMORY filled with the generic handlers**, every
/// index `OPDTB`'s entry for its `<13:9>` opcode, and the register enabled
/// with the main loop at `qmlp` and the bases at A memory `localp` and M
/// memory `ap`, the copies set from them: what the microcode is to do at
/// every start (contract H8a §4), where it writes `A-LOCALP` and `M-AP`
/// after destination 5. With `operand`, the entries [`has_an_operand`]
/// names have the operand bit, so that a fused return into
/// one of them with a LOCAL or ARG operand loads PDL-INDEX before its
/// generic handler computes the same address itself (`QADLOC1`,
/// `QADARG1`, `uc-macrocode.lisp:237-245`).
pub fn fill_generic(m: &mut Machine, qmlp: u16, opdtb: u16, localp: u16, ap: u8, operand: bool) {
    for (k, e) in m.macro_dispatch.entries.iter_mut().enumerate() {
        let op = (k >> 3 & 0o37) as u32;
        *e = m.dmem[opdtb as usize + op as usize];
        if operand && has_an_operand(k) {
            *e |= macro_dispatch::OPERAND;
        }
    }
    set_register(m, macro_dispatch::word(qmlp, localp, ap));
}

/// **An engine with the checker run after its every step.** Everything
/// else is the engine's own.
pub struct Checked<E> {
    pub engine: E,
    pub checker: Checker,
}

impl<E: Executes> Checked<E> {
    pub fn new(engine: E) -> Checked<E> {
        let checker = Checker::new(engine.machine());
        Checked { engine, checker }
    }
}

impl<E: Executes> Engine for Checked<E> {
    fn boot(&mut self) {
        self.engine.boot();
    }
    fn keyboard_boot(&mut self) -> bool {
        self.engine.keyboard_boot()
    }
    fn step(&mut self) -> Result<(), Halt> {
        let r = self.engine.step();
        self.checker.after_step(&self.engine);
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
    fn stop_after_microcycle(&mut self, mc: u64) {
        self.engine.stop_after_microcycle(mc)
    }
    fn stands_between_microcycles(&self) -> bool {
        self.engine.stands_between_microcycles()
    }
    fn settle_for_harness(&mut self) {
        self.engine.settle_for_harness()
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

impl<E: Executes> Executes for Checked<E> {
    fn executed(&self) -> Option<u16> {
        self.engine.executed()
    }
}
