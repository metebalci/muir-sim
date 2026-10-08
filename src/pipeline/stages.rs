// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **The stages' control**: RD's choices from its copies, WB's work and
//! holds, EX's holds and its check of RD's choices, the squash and the
//! restore, CS's reads, and the edge with its bypasses (contract G3
//! revision 15, §5; A15b.3, A15b.15, A15b.16).
//!
//! A clock is reckoned in this order, every stage from what the clock
//! began with: RD's choices for its word; WB; EX, which may squash RD and
//! CS or turn RD's word into a nopped microcycle; the front end's squash
//! and restore or RD's choices made; CS's reads of the A memory and the
//! PDL buffer, before this edge's writes; and the edge, where the RAMs
//! take WB's writes, the bypasses reach the words that stay, and every
//! stage moves.

use std::collections::VecDeque;

use super::exec::Start;
use super::port::{Answer, Reader};
use super::{AmWrite, Copies, Mutation, PdlAt, Pipeline, Slot, class, field};
use crate::isa::Op;
use crate::machine::{Halt, Word};

/// WB's and the port's state between clocks.
#[derive(Clone, Debug, Default)]
pub(crate) struct Back {
    /// The WB word's starts still to be granted, in order.
    pub starts: VecDeque<Start>,
    /// The starts EX's last word made beyond its first.
    pub more: Vec<Start>,
    /// This clock's RAM writes, landing at the edge.
    pub landing: Landing,
    /// A start was translated in WB this clock, and `VMAOK` stood so
    /// before it: what the late squash reads (A15b.3).
    pub fault_now: bool,
    pub vmaok_before: bool,
    /// The last start's acknowledgment, a clock.
    pub ack_at: u64,
    /// A processor read's word on its way to `MD`, and the clock it lands;
    /// or the fill that brings it.
    pub md: Option<(u64, Word)>,
    pub md_fill: bool,
    /// That read is the stream's fetch: its bus address and its virtual
    /// word address.
    pub fetch: Option<(u32, u32)>,
    /// A table read of a walk or a write-back: its address, its word, and
    /// its fill.
    pub table_at: Option<u32>,
    pub table_word: Option<(u64, Word)>,
    pub table_fill: bool,
    pub walk: WalkStep,
    /// The virtual address the walk in progress walks, kept from its first
    /// read to its fill: a lookup that asks for another address meanwhile
    /// waits for it to end.
    pub walk_va: u32,
    pub write_back: WalkStep,
    pub write_back_bits: u32,
    pub write_back_page: u32,
    /// A write granted whose word the next microcycle fixes.
    pub pending_word: Option<PendingWord>,
    /// A register's write, taken the clock after its grant.
    pub register_write: Option<RegisterWrite>,
    /// CMD_PROD's write, waiting for every earlier write (A15b.5): the
    /// word and its microcycle.
    pub cmd_prod: Option<(u32, u64)>,
    /// The prefetch's word (H8a §3.5): its virtual and physical word
    /// addresses and the word.
    pub prefetched: Option<(u32, u32, Word)>,
    /// The next word CS loads is a delay slot RD has squashed on its
    /// prediction, which EX confirms.
    pub pre_nop_next: bool,
    /// The next word CS loads is a delay slot EX has squashed already, or
    /// the boot's trap again after a halt: a nopped microcycle.
    pub nop_next: bool,
    /// The PDL buffer's last write landed: its address, the word it
    /// replaced, its word, and its microcycle's sequence.
    pub last_pdl: Option<(u16, Word, Word, u64)>,
    /// The sequence of the last word committed.
    pub last_seq: u64,
    /// After a halt, the last microcycle's PDL buffer write, which lands
    /// after the next word's read as on a single-edge machine: its
    /// address and word.
    pub pdl_pending: Option<(u16, Word)>,
    /// The word that reached [`Pipeline::boundary_at`] has committed this
    /// clock; or the word of the harness's action point.
    pub boundary_now: bool,
    pub action_now: bool,
    /// The last word that made a start, and the kind of its last start
    /// ([`super::Meters::consecutive_starts`]).
    pub last_start: Option<(u64, usize)>,
    /// A read start's sequence and `MD` as it stood at the grant: the word
    /// right after the start reads that `MD`, the read's word landing in
    /// the microcycle after it (the single-edge contract: a cycle goes out
    /// at the edge ending the microcycle after its start).
    pub md_old: Option<(u64, Word)>,
}

/// What RD would do with its word this clock, reckoned from the clock's
/// start ([`Pipeline::plan_for`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct RdPlan {
    /// RD holds its word this clock: the guard (A15b.16).
    pub hold: bool,
    /// The word's effects on the copies, in order.
    pub effects: Vec<Effect>,
    /// The address of the word two after RD's, when RD resolved or
    /// predicted a transfer.
    pub next2: Option<u16>,
    /// EX checks the word's next address.
    pub ex_resolved: bool,
    /// The word's delay slot is nopped: a transfer taken with N, as RD
    /// resolved or predicted it.
    pub kills_slot: bool,
    /// The prediction, for EX's check.
    pub pred_taken: Option<bool>,
    pub pred_pr: Option<(bool, bool)>,
}

/// A word's effect on RD's copies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// LC steps: `NEXT INSTRD`, or a dispatch's `IR<24>`.
    LcStep,
    /// LC written, its value the word's EX's (HAVE WRONG WORD).
    LcWritten(u64),
    /// A return asks the next microcycle to step LC.
    NextInstr,
    SpcPush(u32),
    SpcPushData(u64),
    SpcPop,
    /// A fused return keeps the popped word.
    SpcKeep(u32),
    PdlPointer(i16),
    PdlPointerWritten(u64),
    PdlIndexWritten(u64),
    PdlIndex(u16),
    /// A fused return's operand address, loaded after the next word.
    OperandAfter(Option<u16>),
    /// The operand address armed before, loaded now.
    OperandLoad,
}

/// What EX did this clock.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ExResult {
    /// The word committed and leaves for WB; its sequence.
    pub committed: bool,
    pub seq: u64,
    pub pc: u16,
    /// EX redirects the front end to this address.
    pub redirect: Option<u16>,
    /// The delay slot in RD becomes a nopped microcycle: a taken transfer
    /// with N.
    pub kill_rd: bool,
    /// The words behind a control-store write are fetched again once WB
    /// has written it.
    pub refetch: bool,
    /// RD's copies restored without a squash: a transfer EX decided nopped
    /// its delay slot, and a dispatch's call pushed the return under its
    /// entry's N, which RD could not see.
    pub restore: bool,
    /// HALT-CONS under ERRSTOP: the machine halts once drained.
    pub halt: bool,
}

/// The A, M and PDL buffer writes WB lands at the clock's edge.
#[derive(Clone, Debug, Default)]
pub(crate) struct Landing {
    pub am: Vec<AmWrite>,
    pub pdl: Option<(u16, Word, u64)>,
}

/// Where a walk or a write-back is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum WalkStep {
    #[default]
    Idle,
    Dir,
    Page,
}

/// Where a write's word goes once it is fixed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WordTo {
    Port(u64),
    Register,
    Pdl(u16),
}

/// A write granted whose word the next microcycle fixes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingWord {
    pub seq: u64,
    pub to: WordTo,
}

/// A device register's write, taken the clock after its grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RegisterWrite {
    pub bus: u32,
    pub word: Option<Word>,
    pub mc: u64,
    pub at_clock: u64,
}

/// The functional destination code of an ALU or BYTE word, 34-37 decoding
/// as 30-33 and 24-27 as 20-23 (`micro`'s `write_functional`); `None` for
/// an A destination or another class.
pub(crate) fn dest_code(ir: u64) -> Option<u32> {
    if !matches!(class(ir), Op::Alu | Op::Byte) || field(ir, 25, 1) == 1 {
        return None;
    }
    let code = field(ir, 19, 5);
    Some(if code & 0o20 != 0 { code & !0o4 } else { code })
}

/// The word's M destination's address, when it writes M.
fn m_dest(ir: u64) -> Option<u8> {
    dest_code(ir).map(|_| field(ir, 14, 5) as u8)
}

/// The word's A destination's address, when it writes A (an M
/// destination writes A's low 32 too).
fn a_dest(ir: u64) -> Option<u16> {
    if !matches!(class(ir), Op::Alu | Op::Byte) {
        return None;
    }
    Some(if field(ir, 25, 1) == 1 { field(ir, 14, 10) as u16 } else { field(ir, 14, 5) as u16 })
}

/// The M source's functional source, when the word reads one.
fn source(ir: u64) -> Option<u8> {
    (field(ir, 31, 1) == 1).then(|| field(ir, 26, 5) as u8)
}

/// Whether the word reads the PDL buffer: functional sources 4, 5, 24 and
/// 25; by the pointer when `<30>`.
fn reads_pdl(ir: u64) -> Option<bool> {
    let s = source(ir)?;
    matches!(s & 0o17, 0o4 | 0o5).then_some(s & 0o20 != 0)
}

/// Whether the word reads or writes `MD`: functional source 12,
/// `MAP(MD)` (11), a dispatch on map bits, or a write of `MD`.
fn uses_md(ir: u64) -> bool {
    matches!(source(ir).map(|s| s & 0o17), Some(0o11 | 0o12))
        || (class(ir) == Op::Dispatch && field(ir, 8, 2) != 0 && field(ir, 10, 2) != 2)
        || matches!(dest_code(ir), Some(0o30..=0o33))
}

/// Whether the word starts a memory cycle by its destination.
fn starts_cycle(ir: u64) -> bool {
    matches!(dest_code(ir), Some(0o21 | 0o22 | 0o31 | 0o32))
}

/// Whether a JUMP's condition is always true (`Some(true)`) or never
/// (`Some(false)`): condition 7, not inverted or inverted
/// (`micro`'s `jump_condition_13`: `<2:0>` 7 when `<4:0>` is not 10-12).
fn unconditional(ir: u64) -> Option<bool> {
    if field(ir, 5, 1) == 0 {
        return None;
    }
    let code = field(ir, 0, 5);
    if matches!(code, 0o10..=0o12) || code & 7 != 7 {
        return None;
    }
    Some(field(ir, 6, 1) == 0)
}

/// Whether a word with destination `d` and source `s` changes the micro
/// stack beyond what RD decodes into a pop or a push of a known word: a
/// data push, or a pop by the functional source.
fn spc_by_data(ir: u64) -> bool {
    dest_code(ir) == Some(0o15) || source(ir).is_some_and(|s| s & 0o17 == 0o14)
}

impl Pipeline {
    // --- The clock ----------------------------------------------------------

    /// **One clock**, in the order the module says.
    pub(crate) fn clock_once(&mut self) -> Result<(), Halt> {
        self.clock += 1;
        self.meters.clocks += 1;
        let now = self.clock;
        if !self.neutral {
            self.m.ns = self.instant(now);
        }
        self.console_edge();
        self.port.tick(&mut self.m, now);
        let errors = self.port.take_errors();
        self.m.posted_write_errors = self.m.posted_write_errors.wrapping_add(errors);
        self.port_events(now);
        if !self.neutral {
            self.advance_devices();
        }
        if self.halted {
            return Ok(());
        }
        let oa_high = self.x.oa_high;
        let plan = self.rd_plan();
        let wb_leaves = self.wb_stage();
        let ex = self.ex_stage(wb_leaves)?;
        self.front_and_edge(plan, ex, wb_leaves, oa_high);
        let (lc, action) =
            (std::mem::take(&mut self.b.boundary_now), std::mem::take(&mut self.b.action_now));
        if lc || action {
            self.boundary_halt |= lc;
            self.action_halt |= action;
            self.begin_drain_from(true);
        }
        if ex.halt {
            self.begin_drain();
        }
        let ending = self.draining || self.stepping;
        if ending
            && self.ex.is_none()
            && self.rd.is_none()
            && self.cs.is_none()
            && let Some((seq, _)) = self.x.write_pending.take()
        {
            // A write whose word the next microcycle would fix, and the
            // halt squashed it: the word is `MD` as it stands. MIT's
            // microcode never loads `MD` in the word after a write start
            // (the contract's §5 table), so that is the word it writes.
            let md = self.m.md;
            self.fix_write(seq, md);
        }
        if ending && self.drained_now() {
            self.draining = false;
            self.stepping = false;
            self.halted = true;
            // RD's copies from the state the drained words left: the words
            // in EX and WB committed after the drain began.
            self.restore_copies();
            // D still waits for the stream's word: its fetch is made by the
            // word after it, which the halt squashed and which is fetched
            // again ahead of the wait.
            self.d_slot_pending = self.d_wait;
            // The last microcycle's PDL buffer write waits for the next
            // word's read, which a single-edge machine makes before it
            // lands: no word reads its predecessor's PDL write (the
            // forward's `seq + 2`).
            // So does the `MD` the next word reads after a read start.
            if self.b.md_old.is_some_and(|(s, _)| s != self.b.last_seq) {
                self.b.md_old = None;
            }
            if let Some((adr, old, word, seq)) = self.b.last_pdl
                && seq == self.b.last_seq
                && self.b.pdl_pending.is_none()
            {
                self.m.pdl[adr as usize] = old;
                self.b.pdl_pending = Some((adr, word));
                self.b.last_pdl = None;
            }
            // A boundary's halt keeps RUN: the run goes on at the next
            // clock, once the harness has read the state.
            if !(self.boundary_halt || self.action_halt) {
                self.m.clock_control.run = false;
                self.srun = false;
            }
        }
        if let Some(h) = self.pending_halt.take() {
            return Err(h);
        }
        Ok(())
    }

    // --- The port's events ------------------------------------------------------

    /// What the port lands this clock: a read's word in `MD`, a table
    /// read's word; and CMD_PROD once every earlier write is answered.
    pub(crate) fn port_events(&mut self, now: u64) {
        if self.b.md_fill
            && !self.port.filling(Reader::Processor)
            && let Some((at, w)) = self.port.filled()
        {
            self.b.md_fill = false;
            self.b.md = Some((at.max(now), w));
        }
        if self.b.table_fill
            && !self.port.filling(Reader::Table)
            && let Some((at, w)) = self.port.filled()
        {
            self.b.table_fill = false;
            self.b.table_word = Some((at.max(now), w));
        }
        if let Some((at, w)) = self.b.md
            && at <= now
        {
            self.b.md = None;
            let held_start = self.ex.as_ref().is_some_and(|e| !e.nop && starts_cycle(e.ir));
            if !(self.mutation == Mutation::FirstReadDropped && held_start) {
                self.m.md = w;
            }
            self.event(super::Event::Md(w));
            if let Some((bus, va)) = self.b.fetch.take() {
                self.prefetch_after(bus, va);
                if self.d_wait {
                    // D's word is here: the handler's CS after L (A15b.9).
                    self.d_wait = false;
                    self.cs_wait_until = self.cs_wait_until.max(now + super::L);
                }
            }
        }
        // CMD_PROD, once every earlier write is answered (A15b.5).
        if let Some((v, mc)) = self.b.cmd_prod {
            if self.port.empty() || self.mutation == Mutation::NoCmdProdWait {
                self.b.cmd_prod = None;
                let keep = self.m.ns;
                self.m.ns = self.device_time(mc);
                let bus = crate::tlb::REGISTER_PAGE_BUS | crate::file_device::CMD_PROD;
                self.m.bus_write(bus, Word::from(v));
                self.m.ns = keep;
            } else {
                self.meters.cmd_prod_wait += 1;
            }
        }
    }

    /// The devices at the clock's instant, or the microcycle's under
    /// neutral time: the I/O board's clocks and the file device, whose
    /// completion sweeps the cache (A15b.6).
    pub(crate) fn advance_devices(&mut self) {
        let ns = self.m.device_ns();
        self.m.ioboard.advance(ns);
        self.m.dma_written = false;
        self.m.advance_file_device();
        if std::mem::take(&mut self.m.dma_written) {
            let missing = (self.mutation == Mutation::SweepMissesASet)
                .then_some(self.port.last_filled)
                .flatten();
            self.port.sweep(self.clock, missing);
            self.drop_prefetch();
        }
    }

    // --- RD -------------------------------------------------------------------

    /// RD's choices for its word, from the clock's start.
    fn rd_plan(&self) -> RdPlan {
        match self.rd.as_ref() {
            Some(r) => self.plan_for(r, &self.copies),
            None => RdPlan::default(),
        }
    }

    /// **RD's choices for `r`** with the copies `c0`: the guard, the
    /// word's effects on the copies, and its next address.
    pub(crate) fn plan_for(&self, r: &Slot, c0: &Copies) -> RdPlan {
        let mut plan = RdPlan::default();
        let mut c = *c0;
        // The microcycle's own step and operand load, executed or nopped.
        if c.next_instr && self.mutation != Mutation::LcNotStepped {
            plan.effects.push(Effect::LcStep);
        }
        if c.operand_after.is_some() {
            plan.effects.push(Effect::OperandLoad);
        }
        if r.nop || r.pre_nop {
            for e in &plan.effects {
                apply_effect(&mut c, *e);
            }
            return plan;
        }
        for e in &plan.effects {
            apply_effect(&mut c, *e);
        }
        let ir = r.ir;
        let op = class(ir);
        let popj = field(ir, 42, 1) != 0;
        let dest = dest_code(ir);
        let src = source(ir);
        let mut returns = false;
        let push = |c: &mut Copies, plan: &mut RdPlan, e: Effect| {
            apply_effect(c, e);
            plan.effects.push(e);
        };
        // The word's own sources and destinations.
        c.spc_top = self.top(&c);
        c.spc_top_below = false;
        if src.is_some_and(|s| s & 0o17 == 0o14) && op != Op::Dispatch {
            push(&mut c, &mut plan, Effect::SpcPop);
        }
        if src.is_some_and(|s| s & 0o17 == 0o4) {
            push(&mut c, &mut plan, Effect::PdlPointer(-1));
        }
        match dest {
            Some(0o1) if self.mutation != Mutation::LcNotMarked => {
                push(&mut c, &mut plan, Effect::LcWritten(r.seq))
            }
            Some(0o11) => push(&mut c, &mut plan, Effect::PdlPointer(1)),
            Some(0o13) => {
                // The PDL address field forms the index early (A15b.2),
                // unless its base is still being written.
                let early = if self.m.geometry.extended() {
                    r.word.pdl_field().and_then(|f| self.early_index(f, &c))
                } else {
                    None
                };
                match early {
                    Some(i) => push(&mut c, &mut plan, Effect::PdlIndex(i)),
                    None => push(&mut c, &mut plan, Effect::PdlIndexWritten(r.seq)),
                }
            }
            Some(0o14) => push(&mut c, &mut plan, Effect::PdlPointerWritten(r.seq)),
            Some(0o15) => push(&mut c, &mut plan, Effect::SpcPushData(r.seq)),
            _ => {}
        }
        match op {
            Op::Jump => {
                let (rbit, pbit, nbit) =
                    (field(ir, 9, 1) != 0, field(ir, 8, 1) != 0, field(ir, 7, 1) != 0);
                let sl = self.m.geometry.extended() && r.word.oa_low_select();
                let follower = self.follower(r);
                let ret = if nbit { follower } else { follower + 1 } & 0o37777;
                if pbit && rbit {
                    // WRITE-I-MEM: EX writes the store and fetches again.
                    plan.ex_resolved = true;
                } else if sl || popj {
                    // A jump whose target comes from OA-REG-LOW, and a jump
                    // with POPJ: EX transfers (A15b.15). RD fetches on in
                    // sequence and pushes a call's return.
                    plan.ex_resolved = true;
                    if pbit && sl && !popj {
                        push(&mut c, &mut plan, Effect::SpcPush(ret as u32));
                    }
                } else {
                    let taken = match unconditional(ir) {
                        Some(t) => t,
                        None => {
                            plan.ex_resolved = true;
                            let hint = self.m.geometry.extended() && r.word.hint();
                            let taken = hint != (self.mutation == Mutation::HintInverted);
                            plan.pred_taken = Some(taken);
                            taken
                        }
                    };
                    if taken {
                        if pbit {
                            push(&mut c, &mut plan, Effect::SpcPush(ret as u32));
                        }
                        let mut target = Some(field(ir, 12, 14) as u16);
                        if rbit {
                            returns = true;
                            target = self.rd_return(&mut c, &mut plan, r, false);
                            if target.is_none() {
                                plan.ex_resolved = true;
                            }
                        }
                        plan.next2 = target;
                        plan.kills_slot = nbit;
                    }
                }
            }
            Op::Dispatch => {
                let write = field(ir, 10, 2) == 2;
                let advance = field(ir, 24, 1) != 0;
                if advance {
                    push(&mut c, &mut plan, Effect::LcStep);
                }
                if write {
                    // A dispatch-memory write transfers only through
                    // IGNPOPJ, with POPJ: EX decides.
                    plan.ex_resolved = popj;
                } else {
                    plan.ex_resolved = true;
                    let (addr, p, rr) = if self.m.geometry.extended() {
                        r.word.predicted_target()
                    } else {
                        (0, true, true)
                    };
                    plan.pred_pr = Some((p, rr));
                    if src.is_some_and(|s| s & 0o17 == 0o14) && rr {
                        push(&mut c, &mut plan, Effect::SpcPop);
                    }
                    match (p, rr) {
                        (true, true) if popj => {
                            returns = true;
                            plan.next2 = self.rd_return(&mut c, &mut plan, r, advance);
                        }
                        (true, true) => {}
                        (false, false) => plan.next2 = Some(addr),
                        (true, false) => {
                            let ret = (self.follower(r) + 1) & 0o37777;
                            push(&mut c, &mut plan, Effect::SpcPush(ret as u32));
                            plan.next2 = Some(addr);
                        }
                        (false, true) => {
                            returns = true;
                            plan.next2 = self.rd_return(&mut c, &mut plan, r, advance);
                        }
                    }
                }
            }
            Op::Alu | Op::Byte => {
                if popj {
                    returns = true;
                    if spc_by_data(ir) {
                        plan.ex_resolved = true;
                    } else {
                        plan.next2 = self.rd_return(&mut c, &mut plan, r, false);
                        if plan.next2.is_none() {
                            plan.ex_resolved = true;
                        }
                    }
                }
            }
        }
        // **The guard** (A15b.16): a one-clock decode hold for a return RD
        // resolves or predicts, when a word before wrote what it reads.
        if returns && self.mutation != Mutation::NoGuard && self.guard(r) {
            plan.hold = true;
        }
        plan
    }

    /// The address of the word that runs after `r`: the CS word's, or the
    /// next fetch's.
    fn follower(&self, r: &Slot) -> u16 {
        match &self.cs {
            Some(c) if c.seq == r.seq + 1 => c.pc,
            _ => self.npc,
        }
    }

    /// The PDL address field's index formed in RD (A15b.2): (B + D) AND
    /// 37777 from B's copy, or `None` when B is still being written.
    fn early_index(&self, (base, displacement): (u8, i8), c: &Copies) -> Option<u16> {
        let md = &self.m.macro_dispatch;
        let b = match base {
            0 => {
                let adr = crate::machine::macro_dispatch::ap_address(md.register) as u8;
                if self.base_in_flight(None, Some(adr)) {
                    return None;
                }
                md.ap
            }
            1 => {
                let adr = crate::machine::macro_dispatch::localp_address(md.register) as u16;
                if self.base_in_flight(Some(adr), None) {
                    return None;
                }
                md.localp
            }
            2 => {
                c.pdl_ptr_pending.is_none().then_some(())?;
                u32::from(c.pdl_ptr)
            }
            _ => {
                c.pdl_idx_pending.is_none().then_some(())?;
                u32::from(c.pdl_idx)
            }
        };
        Some((b.wrapping_add(displacement as i32 as u32) & 0o37777) as u16)
    }

    /// Whether a word in EX or WB writes A at `a` or M at `m`: a base copy
    /// still being written.
    fn base_in_flight(&self, a: Option<u16>, m: Option<u8>) -> bool {
        [&self.ex, &self.wb].into_iter().flatten().any(|s| {
            !s.nop && (a.is_some() && a_dest(s.ir) == a || m.is_some() && m_dest(s.ir) == m)
        }) || self.b.landing.am.iter().any(|w| a.is_some() && w.a == a || m.is_some() && w.m == m)
    }

    /// **RD's return** (`micro`'s `main_loop_return` on RD's copies): the
    /// copy popped, and the target, or `None` when RD cannot know it --- a
    /// return that needs a fetch, which D or the main loop takes, EX's to
    /// decide.
    fn rd_return(&self, c: &mut Copies, plan: &mut RdPlan, r: &Slot, advance: bool) -> Option<u16> {
        let popped = self.top(c);
        apply_effect(c, Effect::SpcPop);
        plan.effects.push(Effect::SpcPop);
        if popped >> 14 & 1 == 0 {
            return Some((popped & 0o37777) as u16);
        }
        let ir = r.ir;
        let pops_by_source = source(ir).is_some_and(|s| s & 0o17 == 0o14);
        if !pops_by_source {
            apply_effect(c, Effect::NextInstr);
            plan.effects.push(Effect::NextInstr);
        }
        if c.lc_pending.is_some() || c.needfetch {
            // The fetch path: D, or the main loop.
            return None;
        }
        let target = ((popped | 2) & 0o37777) as u16;
        let pushes = dest_code(ir) == Some(0o15)
            || (class(ir) == Op::Jump && field(ir, 8, 1) != 0 && field(ir, 9, 1) != 0)
            || plan.effects.iter().any(|e| matches!(e, Effect::SpcPush(_)));
        let writes = matches!(class(ir), Op::Alu | Op::Byte)
            && field(ir, 25, 1) == 0
            && (field(ir, 14, 5) == 0o31 || field(ir, 19, 5) == 0o2);
        let stepping = plan.effects.first() == Some(&Effect::LcStep);
        if advance || stepping || pushes || pops_by_source || writes {
            return Some(target);
        }
        let md = &self.m.macro_dispatch;
        let inc = if c.byte_mode { 1 } else { 2 };
        let counter = self.m.geometry.lc_counter();
        let stepped = (c.lc & counter).wrapping_add(inc) & counter;
        let index_rotate = crate::machine::macro_dispatch::index_rotate(true);
        let rotate = Self::lc_rotation_at(c.byte_mode, stepped, index_rotate);
        // M 31 from its register, or D's armed word, never forwarded
        // (A15b.16).
        let m31 = md.m31.unwrap_or(self.m.mmem[0o31]);
        let rotated = {
            const RING: Word = (1 << 40) - 1;
            let v = m31 & RING;
            match rotate % 40 {
                0 => v,
                k => (v << k | v >> (40 - k)) & RING,
            }
        };
        match md.fused_return(popped, rotated, index_rotate) {
            Some(f) => {
                if f.keep {
                    apply_effect(c, Effect::SpcKeep(popped));
                    plan.effects.push(Effect::SpcKeep(popped));
                }
                let operand = f.operand.and_then(|o| {
                    let (a, m) = (
                        crate::machine::macro_dispatch::localp_address(md.register) as u16,
                        crate::machine::macro_dispatch::ap_address(md.register) as u8,
                    );
                    (!self.base_in_flight(Some(a), Some(m)))
                        .then(|| md.operand_address(o) as u16 & self.m.geometry.pdl_mask())
                });
                if f.operand.is_some() {
                    apply_effect(c, Effect::OperandAfter(operand));
                    plan.effects.push(Effect::OperandAfter(operand));
                }
                Some(f.handler)
            }
            None => Some(target),
        }
    }

    /// **The guard** (A15b.16), for the word `r` in RD: held while a word
    /// before it in EX writes INTERRUPT-CONTROL or destinations 5-7, pushes
    /// SPC data, or writes LC in a microcycle that steps it, which EX
    /// writes at its end; and while a write of M 31 by a word before it is
    /// in EX or WB, not yet in the register RD reads. In the stream that is
    /// the contract's cases, the word before writing (a clock, two for M
    /// 31) and the word two before writing M 31 (a clock). The word three
    /// before has landed M 31 by RD's clock, so this model reads it without
    /// a hold.
    fn guard(&self, r: &Slot) -> bool {
        let writes_m31 = |ir: u64| m_dest(ir) == Some(0o31);
        if let Some(e) = self.ex.as_ref().filter(|e| e.seq < r.seq && !e.nop) {
            let d = dest_code(e.ir);
            if matches!(d, Some(0o2 | 0o5 | 0o6 | 0o7 | 0o15)) || writes_m31(e.ir) {
                return true;
            }
            let steps =
                self.x.next_instrd || (class(e.ir) == Op::Dispatch && field(e.ir, 24, 1) != 0);
            if d == Some(0o1) && steps {
                return true;
            }
        }
        self.wb.as_ref().is_some_and(|w| {
            w.seq < r.seq && !w.nop && writes_m31(w.ir) && w.wb_state == super::exec::WbState::Fresh
        })
    }

    // --- WB -----------------------------------------------------------------

    /// **WB**: the word's A, M and PDL buffer writes land at its first
    /// clock's edge; its starts are translated and granted. Whether it is
    /// done and leaves at the edge.
    fn wb_stage(&mut self) -> bool {
        self.b.fault_now = false;
        self.b.vmaok_before = self.m.vmaok;
        let Some(mut w) = self.wb.take() else { return true };
        if w.wb_state == super::exec::WbState::Fresh {
            self.b.landing.am = std::mem::take(&mut w.am);
            if let Some((at, word)) = w.pdl_w.take() {
                let adr = match at {
                    PdlAt::At(a) => a,
                    PdlAt::Index => self.m.pdl_index,
                };
                self.b.landing.pdl = Some((adr & self.m.geometry.pdl_mask(), word, w.seq));
            }
            w.wb_state = super::exec::WbState::Done;
            self.b.starts.extend(w.start.take());
            self.b.starts.extend(std::mem::take(&mut self.b.more));
        }
        while let Some(s) = self.b.starts.front().copied() {
            if !self.start_at_wb(s, w.seq) {
                self.meters.wb_hold += 1;
                self.wb = Some(w);
                return false;
            }
            self.b.starts.pop_front();
        }
        self.m.cycles += 1;
        self.meters.retired += 1;
        self.last_counted = (!w.nop).then_some(w.pc);
        if w.nop && self.mutation == Mutation::NopCountedTwice {
            self.m.cycles += 1;
        }
        self.wb = Some(w);
        true
    }

    /// **A start at WB** (A15b.3, A15b.5, A15b.6): the translation, a walk
    /// on a miss, the fault, the redirect, the write-back, and the port's
    /// grant. Whether it is done.
    fn start_at_wb(&mut self, s: Start, seq: u64) -> bool {
        let now = self.clock;
        if self.tlb_sweep_until > now
            || self.b.ack_at > now
            || self.b.md.is_some()
            || self.b.md_fill
        {
            return false;
        }
        let va = s.va;
        if crate::tlb::region(va) == crate::tlb::Region::Paged
            && self.m.tlb.lookup(va).is_none()
            && self.walk(va, crate::tlb::Port::A).is_none()
        {
            return false;
        }
        let mut entry = self.m.translate(va).l2_data;
        if self.b.write_back == WalkStep::Idle {
            let (e2, redirect) = self.m.redirect_14(va, entry, s.write);
            entry = e2;
            self.x.lvmo = entry;
            self.x.wrcyc = s.write;
            let access = entry & 1 << 27 != 0;
            let write_ok = entry & 1 << 26 != 0;
            self.m.vmaok = access && (!s.write || write_ok);
            self.b.fault_now = true;
            if !self.m.vmaok {
                // A faulting start leaves nothing (A15b.3).
                return true;
            }
            if let Some(crate::tlb::Redirect::Inside(i)) = redirect {
                self.m.tlb.redirects[0] += 1;
                // Inside the PDL buffer: no memory cycle, and one held clock
                // (A14.7).
                if s.write {
                    match s.word {
                        Some(word) => self.m.pdl[i as usize] = word,
                        None => self.b.pending_word = Some(PendingWord { seq, to: WordTo::Pdl(i) }),
                    }
                } else {
                    self.b.md_old = Some((seq, self.m.md));
                    self.b.md = Some((now + 3, self.m.pdl[i as usize]));
                }
                self.b.ack_at = now + 3;
                return true;
            }
            if redirect.is_some() {
                self.m.tlb.redirects[1] += 1;
            }
        }
        // A register access waits while a register write before it is still
        // to be taken: both land, in order, so that a read right after a
        // write reads the device as the write left it (A15b.3, "A start
        // right after a start"; MP2b rulings 2, F1).
        let device = |bus: u32, m: &crate::machine::Machine| {
            crate::busint::decode_quux_14(bus, m.main.len(), m.tv.buffer_words())
                == crate::busint::Responder::Device
        };
        if self.b.register_write.is_some() && device(crate::tlb::bus_address(va, entry), &self.m) {
            return false;
        }
        // The write-back, ahead of the reference's own cycle (A14.6).
        let md = s.word.unwrap_or(self.m.md);
        if self.write_back_at_wb(va, entry, s.write, md) == Some(false) {
            return false;
        }
        let bus = crate::tlb::bus_address(va, entry);
        self.event(super::Event::Grant(bus, s.write));
        let responder =
            crate::busint::decode_quux_14(bus, self.m.main.len(), self.m.tv.buffer_words());
        match responder {
            crate::busint::Responder::Memory(_) => {
                if s.write {
                    let Some(tag) = self.port.write(now, bus) else { return false };
                    if self.b.prefetched.is_some_and(|p| p.1 == bus) {
                        self.drop_prefetch();
                    }
                    match s.word {
                        Some(word) => self.port.word(tag, word),
                        None => {
                            self.b.pending_word = Some(PendingWord { seq, to: WordTo::Port(tag) })
                        }
                    }
                    self.b.ack_at = now + 2;
                } else {
                    match self.port.read(now, bus, Reader::Processor) {
                        Answer::At(at, word) => self.b.md = Some((at, word)),
                        Answer::Filling => self.b.md_fill = true,
                        Answer::Busy => return false,
                    }
                    self.b.md_old = Some((seq, self.m.md));
                    if s.fetch {
                        self.b.fetch = Some((bus, va));
                    }
                }
            }
            crate::busint::Responder::Device => {
                // A device register: taken the clock after its grant, its
                // word in MD two clocks after it (A15b.3; MP2b ruling Q4).
                if s.write {
                    self.b.register_write =
                        Some(RegisterWrite { bus, word: s.word, mc: s.mc, at_clock: now + 1 });
                    if s.word.is_none() {
                        self.b.pending_word = Some(PendingWord { seq, to: WordTo::Register });
                    }
                } else {
                    let keep = self.m.ns;
                    let at = if self.neutral { self.instant(s.mc) } else { self.instant(now + 1) };
                    self.m.ns = at;
                    if !self.neutral {
                        self.advance_devices();
                    }
                    let word = self.m.bus_read(bus);
                    self.m.ns = keep;
                    self.event(super::Event::Register(bus));
                    self.b.md_old = Some((seq, self.m.md));
                    self.b.md = Some((now + 2, word));
                }
                self.b.ack_at = now + 2;
            }
            _ => {
                // Nothing there: one clock after the grant, MD zero, the
                // NXM bit (A15b.3).
                if !s.write {
                    self.b.md_old = Some((seq, self.m.md));
                    self.b.md = Some((now + 1, 0));
                }
                self.m.bus_error |= crate::machine::bus_error::XBUS_NXM;
                self.b.ack_at = now + 1;
            }
        }
        true
    }

    /// **A walk** for `va` through `port` (A14.6): the directory entry and
    /// the page entry read through the cache, each a hit or a fill with the
    /// read rule; what it finds loaded. `None` while it reads.
    ///
    /// The walk keeps the address it began with to its fill (MP4 ruling
    /// Q2, M2): asked for another address while it reads, it finishes its
    /// own and answers `None`, and the caller looks its address up again.
    pub(crate) fn walk(&mut self, va: u32, port: crate::tlb::Port) -> Option<()> {
        let base = self.m.memory_words.directory;
        let frames = (self.m.main.len() >> 10) as u32;
        if self.b.walk == WalkStep::Idle {
            self.b.walk_va = va;
        }
        let ours = self.b.walk_va == va;
        let va = self.b.walk_va;
        loop {
            match self.b.walk {
                WalkStep::Idle => {
                    if base & 0o777777 == 0 {
                        self.m.tlb.walks += 1;
                        return Some(());
                    }
                    self.b.walk = WalkStep::Dir;
                    self.table_read_start((base & 0o777774) << 10 | va >> 20);
                }
                WalkStep::Dir => {
                    let dir = self.table_read_done()?;
                    let frame = dir as u32 & 0o777777;
                    if crate::tlb::status(dir) != 4 || frame >= frames {
                        self.b.walk = WalkStep::Idle;
                        self.m.tlb.walks += 1;
                        return ours.then_some(());
                    }
                    self.b.walk = WalkStep::Page;
                    self.table_read_start(frame << 10 | (va >> 10 & 0o1777));
                }
                WalkStep::Page => {
                    let page = self.table_read_done()?;
                    self.b.walk = WalkStep::Idle;
                    self.m.tlb.walks += 1;
                    let entry = match crate::tlb::status(page) {
                        0 | 7 => None,
                        1 => Some(page as u32 & crate::tlb::ENTRY_BITS),
                        _ if page as u32 & 0o777777 >= frames => None,
                        _ => Some(page as u32 & crate::tlb::ENTRY_BITS),
                    };
                    if let Some(e) = entry {
                        self.m.tlb.fill(va, e, port);
                    }
                    return ours.then_some(());
                }
            }
        }
    }

    fn table_read_start(&mut self, phys: u32) {
        self.b.table_word = None;
        self.b.table_at = Some(phys);
    }

    /// The table read's word, once it has come.
    fn table_read_done(&mut self) -> Option<Word> {
        let now = self.clock;
        if let Some((at, w)) = self.b.table_word {
            if at <= now {
                self.b.table_word = None;
                self.b.table_at = None;
                return Some(w);
            }
            return None;
        }
        if self.b.table_fill {
            return None;
        }
        let phys = self.b.table_at?;
        match self.port.read(now, phys, Reader::Table) {
            Answer::At(at, w) => self.b.table_word = Some((at, w)),
            Answer::Filling => self.b.table_fill = true,
            Answer::Busy => {}
        }
        None
    }

    /// **The write-back** at WB (A14.6, A14.8): the bits ORed into the TLB
    /// entry at once and into the table by a posted write, after the
    /// tables are read again. `None` when there is nothing more to do;
    /// `Some(false)` while it reads or waits for the queue.
    fn write_back_at_wb(&mut self, va: u32, entry: u32, write: bool, md: Word) -> Option<bool> {
        if crate::tlb::region(va) != crate::tlb::Region::Paged {
            return None;
        }
        let words = self.m.memory_words;
        let base = words.directory;
        let frames = (self.m.main.len() >> 10) as u32;
        loop {
            match self.b.write_back {
                WalkStep::Idle => {
                    let ephemeral = words.ephemeral
                        && words.pointer_type(md)
                        && (md as u32) >> 28 == crate::tlb::EPHEMERAL_SPACE;
                    let bits = crate::tlb::write_back_bits(entry, write, ephemeral);
                    if bits == 0 {
                        return None;
                    }
                    self.b.write_back_bits = bits;
                    self.m.tlb.or(va, bits);
                    self.m.tlb.write_backs += 1;
                    for (k, bit) in
                        [crate::tlb::ACCESSED, crate::tlb::MODIFIED, crate::tlb::EPHEMERAL]
                            .into_iter()
                            .enumerate()
                    {
                        self.m.tlb.written_bits[k] += u64::from(bits & bit != 0);
                    }
                    if base & 0o777777 == 0 {
                        self.refused();
                        return None;
                    }
                    self.b.write_back = WalkStep::Dir;
                    self.table_read_start((base & 0o777774) << 10 | va >> 20);
                }
                WalkStep::Dir => {
                    let Some(dir) = self.table_read_done() else { return Some(false) };
                    let frame = dir as u32 & 0o777777;
                    if crate::tlb::status(dir) != 4 || frame >= frames {
                        self.b.write_back = WalkStep::Idle;
                        self.refused();
                        return None;
                    }
                    self.b.write_back_page = frame << 10 | (va >> 10 & 0o1777);
                    self.b.write_back = WalkStep::Page;
                    self.table_read_start(self.b.write_back_page);
                }
                WalkStep::Page => {
                    let page = if let Some((_, w)) =
                        self.b.table_word.filter(|_| self.b.table_at.is_none())
                    {
                        w
                    } else {
                        let Some(w) = self.table_read_done() else { return Some(false) };
                        // Kept for a retry when the queue is full.
                        self.b.table_word = Some((0, w));
                        w
                    };
                    let ok = (2..=6).contains(&crate::tlb::status(page))
                        && page as u32 & 0o777777 == entry & 0o777777;
                    if !ok {
                        self.b.table_word = None;
                        self.b.write_back = WalkStep::Idle;
                        self.refused();
                        return None;
                    }
                    if !self
                        .port
                        .post(self.b.write_back_page, page | u64::from(self.b.write_back_bits))
                    {
                        return Some(false);
                    }
                    self.b.table_word = None;
                    self.b.write_back = WalkStep::Idle;
                    return None;
                }
            }
        }
    }

    /// A test's record of `e`, at this clock.
    pub(crate) fn event(&mut self, e: super::Event) {
        let clock = self.clock;
        if let Some(v) = self.events.as_mut() {
            v.push((clock, e));
        }
    }

    fn refused(&mut self) {
        self.m.tlb.refusals += 1;
        self.m.memory_words.refused = self.m.memory_words.refused.wrapping_add(1);
    }

    /// **A write's word, fixed** by the microcycle after its start
    /// (A15b.5): the start's, if WB has not granted it, or the queue's
    /// entry, the register's write, the PDL buffer's word.
    pub(crate) fn fix_write(&mut self, seq: u64, word: Word) {
        if let Some(w) = self.wb.as_mut().filter(|w| w.seq == seq)
            && let Some(st) = w.start.as_mut().filter(|s| s.write)
        {
            st.word = Some(word);
            return;
        }
        if let Some(st) = self.b.starts.iter_mut().find(|s| s.write && s.word.is_none()) {
            st.word = Some(word);
            return;
        }
        match self.b.pending_word.take() {
            Some(PendingWord { seq: s, to }) if s == seq => match to {
                WordTo::Port(tag) => self.port.word(tag, word),
                WordTo::Register => {
                    if let Some(r) = self.b.register_write.as_mut() {
                        r.word = Some(word);
                    }
                }
                WordTo::Pdl(i) => self.m.pdl[i as usize] = word,
            },
            other => self.b.pending_word = other,
        }
    }

    // --- EX -----------------------------------------------------------------

    /// **EX**: the word's holds, then its microcycle, and its check of RD's
    /// choices (A15b.2, A15b.3). The word leaves for WB only when WB is
    /// free at the edge.
    fn ex_stage(&mut self, wb_leaves: bool) -> Result<ExResult, Halt> {
        let mut res = ExResult::default();
        self.register_write_now();
        let Some(mut e) = self.ex.take() else { return Ok(res) };
        if !wb_leaves {
            self.ex = Some(e);
            return Ok(res);
        }
        // d1: the word in WB this clock into the operands.
        if self.mutation != Mutation::NoD1 {
            apply_am(&mut e, &self.b.landing.am);
        }
        let ir = if e.nop { 0 } else { e.ir };
        let now = self.clock;
        let needfetch = self.m.lc & self.m.geometry.need_fetch() != 0;
        let will_start = (!e.nop && starts_cycle(ir))
            || (self.x.next_instrd && needfetch)
            || (!e.nop
                && class(ir) == Op::Dispatch
                && field(ir, 24, 1) != 0
                && field(ir, 10, 2) != 2
                && needfetch);
        let read_in_flight = self.b.md.is_some() || self.b.md_fill;
        // The word right after a read start reads `MD` as the start found
        // it, and waits for nothing.
        let successor = self.b.md_old.is_some_and(|(s, _)| s + 1 == e.seq)
            && self.mutation != Mutation::SuccessorWaitsForMd;
        if !e.nop && uses_md(ir) && read_in_flight && !successor {
            self.meters.md_wait += 1;
            self.ex = Some(e);
            return Ok(res);
        }
        if will_start && (self.b.ack_at > now || read_in_flight || !self.b.starts.is_empty()) {
            self.meters.start_wait += 1;
            self.ex = Some(e);
            return Ok(res);
        }
        if will_start && self.x.map_write_d.is_some() && !e.map_held {
            // A start in the microcycle a map store's write lands in waits a
            // clock, to translate through the new entry (A15b.3).
            e.map_held = true;
            self.meters.map_hold += 1;
            self.ex = Some(e);
            return Ok(res);
        }
        // Port B looks up the `MD` the word reads: the word right after a
        // read start reads `MD` as the start found it, whenever the read's
        // word lands (MP4 ruling Q2, M2).
        let md_read = match self.b.md_old {
            Some((_, old)) if successor => old,
            _ => self.m.md,
        };
        if !e.nop && reads_port_b(ir, md_read, &self.m) {
            if self.tlb_sweep_until > now {
                self.ex = Some(e);
                return Ok(res);
            }
            let va = md_read as u32;
            if crate::tlb::region(va) == crate::tlb::Region::Paged
                && self.m.tlb.lookup(va).is_none()
                && self.walk(va, crate::tlb::Port::B).is_none()
            {
                self.meters.wb_hold += 1;
                self.ex = Some(e);
                return Ok(res);
            }
        }
        if !e.nop && self.m.geometry.muldiv {
            let clocks = match crate::muldiv::decode(ir) {
                Some(crate::muldiv::Op::Div) => crate::muldiv::DIV_CLOCKS_15,
                Some(crate::muldiv::Op::Mul) => crate::muldiv::MUL_CLOCKS_15,
                None => 1,
            };
            e.ex_clocks += 1;
            if e.ex_clocks < clocks {
                self.meters.muldiv += 1;
                self.ex = Some(e);
                return Ok(res);
            }
        }
        // **The late squash** (A15b.3): a check of conditions 4-6 in EX
        // while the start before it is translated in WB, whose fault comes
        // too late for the branch: predicted not to fault, and held a clock
        // when it did.
        let mut vmaok = self.m.vmaok;
        if !e.nop && self.b.fault_now && Self::condition_reads_vmaok(ir) && !e.late_held {
            // Predicted not to fault.
            let early = true;
            if early != vmaok {
                if self.mutation == Mutation::NoLateSquash {
                    vmaok = early;
                } else {
                    e.late_held = true;
                    self.meters.late_squashes += 1;
                    self.ex = Some(e);
                    return Ok(res);
                }
            }
        }
        let mc = self.committed + 1;
        if self.neutral {
            self.m.ns = self.instant(mc);
            self.advance_devices();
        }
        self.x.interrupt_sample = None;
        if self.mutation == Mutation::InterruptSampledInRd && !self.neutral {
            let now = self.m.ns;
            self.m.ns = self.instant(e.ex_entered.saturating_sub(1));
            self.x.interrupt_sample = Some(self.m.interrupt_at(self.m.ns));
            self.m.ns = now;
        }
        // The read landed while the word after its start waited for
        // something else: that word reads the `MD` from before, and the
        // read's word stands after it, whatever it wrote, as it lands in
        // the microcycle after.
        let landed = match self.b.md_old {
            Some((_, old)) if successor && !read_in_flight => {
                Some(std::mem::replace(&mut self.m.md, old))
            }
            _ => None,
        };
        let done = self.execute(&e, mc, vmaok);
        if let Some(w) = landed {
            self.m.md = w;
        }
        if let Err(h) = done {
            self.ex = Some(e);
            return Err(h);
        }
        if self.b.md_old.is_some_and(|(s, _)| e.seq > s) {
            self.b.md_old = None;
        }
        if self.boundary_at.is_some_and(|t| self.m.lc_steps >= t) {
            self.boundary_at = None;
            self.b.boundary_now = true;
        }
        if self.boundary_mc == Some(mc) {
            self.boundary_mc = None;
            self.b.action_now = true;
        }
        self.committed = mc;
        e.mc = mc;
        if let Some(t) = self.trace.as_mut() {
            t.push((!e.nop).then_some(e.pc));
        }
        self.event(super::Event::Commit((!e.nop).then_some(e.pc)));
        if let Some(t) = self.registers.as_mut() {
            t.push(registers_of(&self.m));
        }
        if !e.nop
            && let Some(t) = self.operands.as_mut()
        {
            t.push((e.pc, self.x.adata, self.x.mdata, self.x.out));
        }
        e.am = std::mem::take(&mut self.x.am);
        e.pdl_w = self.x.pdl_w.take();
        let kind = |s: &Start| if s.fetch { 2 } else { s.write as usize };
        let kinds: Vec<usize> = self.x.starts.iter().map(kind).collect();
        if let (Some(&first), Some((seq, last))) = (kinds.first(), self.b.last_start)
            && seq + 1 == e.seq
        {
            self.meters.consecutive_starts[last][first] += 1;
        }
        for pair in kinds.windows(2) {
            self.meters.consecutive_starts[pair[0]][pair[1]] += 1;
        }
        if let Some(&last) = kinds.last() {
            self.b.last_start = Some((e.seq, last));
        }
        let mut starts = std::mem::take(&mut self.x.starts).into_iter();
        e.start = starts.next();
        self.b.more = starts.collect();
        res.committed = true;
        res.seq = e.seq;
        self.b.last_seq = e.seq;
        res.pc = e.pc;
        if !e.nop && class(ir) == Op::Jump && self.m.geometry.extended() && e.word.oa_low_select() {
            self.meters.sl_jumps += 1;
        }
        let kind = match class(ir) {
            Op::Jump => 0,
            Op::Dispatch => 1,
            _ => 2,
        };
        // The check of RD's choices: the address of the word after the
        // delay slot.
        let actual = self.x.npc;
        if !e.nop {
            let predicted = e.next2.unwrap_or(self.x.npc_seq);
            res.kill_rd = self.x.inhibit;
            res.restore = e.ex_resolved && self.x.inhibit;
            if self.x.wrote_imem && self.mutation != Mutation::NoImemRefetch {
                // The word after the write in execution order, fetched again
                // after the write: the next in sequence, or the target of the
                // transfer whose slot it fills, the address it pushes under N
                // (WRITE-I-MEM ruling).
                res.redirect = Some(if self.mutation == Mutation::ImemRefetchAfterItsAddress {
                    (e.pc + 1) & 0o37777
                } else {
                    self.x.npc.wrapping_sub(1) & 0o37777
                });
                res.refetch = true;
            } else if e.ex_resolved {
                // Wrong by the address, or by what was predicted of it: a
                // jump's condition, a dispatch's P and R (A15b.2), which RD's
                // copies took. A return to the next macroinstruction has no
                // address in RD, so the address alone misses a wrong one.
                let wrong_kind = e.pred_taken.is_some_and(|t| t != self.x.taken)
                    || e.pred_pr.is_some_and(|pr| pr != self.x.entry_pr);
                if actual != predicted || wrong_kind {
                    res.redirect = Some(actual);
                    self.meters.mispredicted[kind] += 1;
                }
            } else if actual != predicted {
                self.meters.rd_disagreements += 1;
            }
            // RD nopped the delay slot on a prediction, and it runs.
            if res.redirect.is_none() && !self.x.inhibit && self.slot_pre_nopped(e.seq + 1) {
                res.redirect = Some(actual);
            }
        }
        if self.x.d_fused {
            self.d_wait = true;
        }
        if self.x.halted && self.m.mode.errstop {
            res.halt = true;
        }
        self.ex = Some(e);
        Ok(res)
    }

    /// Whether the word of sequence `seq`, in RD or CS, was nopped by RD.
    fn slot_pre_nopped(&self, seq: u64) -> bool {
        [&self.rd, &self.cs].into_iter().flatten().any(|w| w.seq == seq && w.pre_nop)
            || (self.b.pre_nop_next
                && ![&self.rd, &self.cs].into_iter().flatten().any(|w| w.seq == seq))
    }

    /// RD's copy of the stack's top: the word it holds, or after a pop the
    /// stack's word below, read from the stack's RAM as the word in EX
    /// leaves it (its push lands in the next microcycle, `SPCWPASS`).
    pub(crate) fn top(&self, c: &Copies) -> u32 {
        if !c.spc_top_below {
            return c.spc_top;
        }
        match self.x.spc_write {
            Some((p, w)) if p == c.spc_ptr => w,
            _ => self.m.spc[c.spc_ptr as usize],
        }
    }

    /// The register write granted last clock, taken now.
    fn register_write_now(&mut self) {
        let Some(r) = self.b.register_write else { return };
        if r.at_clock > self.clock {
            return;
        }
        let Some(word) = r.word else { return };
        self.b.register_write = None;
        let page = crate::tlb::REGISTER_PAGE_BUS;
        let (on_page, off) = (r.bus & !0o377 == page, r.bus & 0o377);
        if on_page && off == crate::file_device::CMD_PROD {
            // CMD_PROD waits for every earlier write (A15b.5).
            self.b.cmd_prod = Some((word as u32, r.mc));
            return;
        }
        let keep = self.m.ns;
        self.m.ns = self.device_time(r.mc);
        self.event(super::Event::Register(r.bus));
        if on_page && (0o200..=0o203).contains(&off) {
            self.block_disk_write(r.bus, word);
        } else {
            self.m.bus_write(r.bus, word);
        }
        self.m.ns = keep;
    }

    /// A block-disk register's write: every write queued or in flight lands
    /// first, then the transfer moves, each word it writes clearing its set
    /// (MP2b ruling Q11; coherence rule 2).
    fn block_disk_write(&mut self, bus: u32, word: Word) {
        if self.mutation != Mutation::NoLandAtStart {
            self.port.land_all(&mut self.m);
        }
        let logged = self.m.block_disk.as_ref().is_some_and(|d| d.log.is_some());
        if !logged && let Some(d) = self.m.block_disk.as_mut() {
            d.log = Some(Vec::new());
        }
        let before = self.m.block_disk.as_ref().and_then(|d| d.log.as_ref()).map_or(0, Vec::len);
        self.m.bus_write(bus, word);
        self.m.dma_written = false;
        let pages: Vec<u32> = self
            .m
            .block_disk
            .as_ref()
            .and_then(|d| d.log.as_ref())
            .map(|l| l[before..].iter().filter(|t| !t.write).map(|t| t.page).collect())
            .unwrap_or_default();
        if !logged && let Some(d) = self.m.block_disk.as_mut() {
            d.log = None;
        }
        for page in pages {
            for w in (page..page + 1024).step_by(super::port::LINE_WORDS as usize) {
                if self.mutation != Mutation::NoClearSet {
                    self.port.cache.clear_set(w);
                }
            }
            self.drop_prefetch();
        }
    }

    // --- The front end and the edge ---------------------------------------------

    /// **The front end and the edge**: EX's squash and the restore, or
    /// RD's choices made; CS's reads; then the RAMs' writes, the bypasses
    /// into the words that stay, and every stage's move.
    fn front_and_edge(&mut self, plan: RdPlan, ex: ExResult, wb_leaves: bool, oa_high: u64) {
        let ex_free = self.ex.is_none() || ex.committed;
        if ex.committed {
            self.refresh_pending(ex.seq);
        }
        let mut rd_moves = false;
        let slot_seq = ex.seq + 1;
        if let Some(to) = ex.redirect {
            // Every word after the delay slot leaves nothing; the delay slot
            // stays, nopped under N, wherever it is.
            let mut refetch = None;
            for st in [&mut self.rd, &mut self.cs] {
                match st.as_mut() {
                    Some(w) if w.seq == slot_seq && !ex.refetch => {
                        if ex.kill_rd {
                            w.nop = true;
                            w.pre_nop = false;
                        } else if w.pre_nop {
                            // RD nopped it on a prediction; it runs after all.
                            refetch = Some(w.pc);
                            *st = None;
                            self.meters.squashed += 1;
                        }
                    }
                    Some(w) => {
                        let ir = w.word.low_48().raw();
                        if self.mutation == Mutation::SquashedStart && !w.nop && starts_cycle(ir) {
                            // Planted: the squashed word's start made, at the
                            // address its A source names.
                            let write = matches!(dest_code(ir), Some(0o22 | 0o32));
                            let va = self.m.amem[field(ir, 32, 10) as usize] as u32;
                            let s = Start { write, va, mc: 0, fetch: false, word: Some(0o7777) };
                            self.b.starts.push_back(s);
                        }
                        if self.mutation == Mutation::SquashedLookup
                            && !w.nop
                            && reads_port_b(ir, self.m.md, &self.m)
                        {
                            // Planted: the squashed word's port-B lookup made,
                            // a miss walked and filled.
                            let va = self.m.md as u32;
                            if crate::tlb::region(va) == crate::tlb::Region::Paged
                                && self.m.tlb.lookup(va).is_none()
                            {
                                self.m.tlb_fill(va, crate::tlb::Port::B);
                            }
                        }
                        *st = None;
                        self.meters.squashed += 1;
                    }
                    None => {}
                }
            }
            let slot_here = [&self.rd, &self.cs].into_iter().flatten().any(|w| w.seq == slot_seq);
            self.b.pre_nop_next = false;
            self.b.nop_next = false;
            if ex.refetch {
                self.npc = to;
                self.npc_after = None;
                self.cs_wait_until = self.cs_wait_until.max(self.clock + 2);
            } else if let Some(pc) = refetch {
                self.npc = pc;
                self.npc_after = Some(to);
            } else if slot_here {
                self.npc = to;
                self.npc_after = None;
            } else {
                // The delay slot not fetched yet: it comes first.
                self.npc = (ex.pc + 1) & 0o37777;
                self.npc_after = Some(to);
                self.b.nop_next = ex.kill_rd;
            }
            self.restore_copies();
            // Sequence numbers follow the order words run in: the next
            // fetched word follows the last one kept.
            self.seq = [&self.rd, &self.cs]
                .into_iter()
                .flatten()
                .map(|w| w.seq)
                .max()
                .unwrap_or(ex.seq + if refetch.is_some() || !slot_here { 0 } else { 1 })
                + 1;
            if let Some(r) = self.rd.clone()
                && ex_free
            {
                let p = self.plan_for(&r, &self.copies.clone());
                if !p.hold {
                    rd_moves = self.make_plan(&p);
                } else {
                    self.meters.guard_hold += 1;
                }
            }
        } else {
            let mut p = plan;
            if ex.restore && self.mutation != Mutation::NoSpcRestore {
                self.restore_copies();
                if let Some(r) = self.rd.clone() {
                    p = self.plan_for(&r, &self.copies.clone());
                }
            }
            if ex.kill_rd {
                let mut found = false;
                for st in [&mut self.rd, &mut self.cs].into_iter().flatten() {
                    if st.seq == slot_seq {
                        st.nop = true;
                        st.pre_nop = false;
                        found = true;
                    }
                }
                if !found {
                    self.b.nop_next = true;
                    self.b.pre_nop_next = false;
                }
                if let Some(r) = self.rd.clone()
                    && r.seq == slot_seq
                {
                    p = self.plan_for(&r, &self.copies.clone());
                }
            }
            if self.rd.is_some() && ex_free && !p.hold {
                rd_moves = self.make_plan(&p);
            } else if self.rd.is_some() && p.hold {
                self.meters.guard_hold += 1;
            }
        }
        // CS: can its word move to RD?
        let rd_free = self.rd.is_none() || rd_moves;
        let cs_hold = self.cs_hold();
        let cs_moves = self.cs.is_some() && rd_free && !cs_hold;
        if cs_moves {
            self.cs_reads(oa_high);
        }
        // The edge.
        let landing = std::mem::take(&mut self.b.landing);
        // d3 and d2 into RD's word, and the PDL buffer's forwards.
        if let Some(r) = self.rd.as_mut() {
            if r.rd_clocks == 0 {
                let held = std::mem::take(&mut self.held_am);
                apply_a(r, &held);
                if let Some((adr, word, wseq)) = self.held_pdl
                    && adr == r.pdl_addr
                    && wseq + 2 <= r.seq
                {
                    r.pdl_val = word;
                }
                self.held_am = held;
            }
            if self.mutation != Mutation::NoD2 {
                apply_am(r, &landing.am);
            }
            if let Some((adr, word, wseq)) = landing.pdl
                && adr == r.pdl_addr
                && wseq + 2 <= r.seq
                && self.mutation != Mutation::NoPdlForward
            {
                r.pdl_val = word;
            }
            r.rd_clocks += 1;
        }
        // d1 into a word EX holds.
        if !ex.committed
            && let Some(e) = self.ex.as_mut()
        {
            apply_am(e, &landing.am);
        }
        // The RAMs take WB's writes.
        for w in &landing.am {
            if let Some(a) = w.a {
                self.m.amem[a as usize] = w.word;
                self.m.macro_dispatch.a_written(a as usize, w.word);
                self.m.a_written_14(a as usize, w.word);
            }
            if let Some(m) = w.m {
                self.m.mmem[m as usize] = w.word;
                self.m.macro_dispatch.m_written(m as usize, w.word);
            }
        }
        if let Some((adr, word, seq)) = landing.pdl {
            self.b.last_pdl = Some((adr, self.m.pdl[adr as usize], word, seq));
            self.m.pdl[adr as usize] = word;
        }
        self.held_am = if self.mutation == Mutation::NoD3 { Vec::new() } else { landing.am };
        self.held_pdl = landing.pdl;
        // The moves.
        if wb_leaves {
            self.wb = None;
        }
        if ex.committed {
            self.wb = self.ex.take();
        }
        if rd_moves {
            self.ex = self.rd.take();
            if let Some(e) = self.ex.as_mut() {
                e.ex_entered = self.clock + 1;
            }
        }
        if cs_moves {
            let mut c = self.cs.take().expect("the CS word");
            if c.ir & 1 << 31 == 0 {
                c.m_addr = field(c.ir, 26, 5) as u8;
                c.m_val = self.m.mmem[c.m_addr as usize];
            }
            self.rd = Some(c);
        }
        // CS loads at NPC.
        if self.cs.is_none()
            && !self.draining
            && !self.stepping
            && self.clock + 1 >= self.cs_wait_until
            && (!self.d_wait || self.d_slot_pending)
            && self.fetching()
        {
            self.d_slot_pending = false;
            let pc = self.npc;
            // With `IDEBUG` up the debug IR's word is loaded in place of the
            // store's, as a single step runs it (A15b.13; MP4 ruling Q4).
            let word =
                if self.m.clock_control.idebug { self.m.debug_insn() } else { self.m.fetch(pc) };
            let mut s = Slot::new(self.seq, pc, word);
            self.seq += 1;
            s.nop = std::mem::take(&mut self.b.nop_next);
            s.pre_nop = std::mem::take(&mut self.b.pre_nop_next) && !s.nop;
            self.npc = self.npc_after.take().unwrap_or((pc + 1) & 0o37777);
            self.cs = Some(s);
        }
    }

    /// RD's choices made: the copies take the word's effects, and the word
    /// after the CS word is RD's choice. Whether the word moves to EX.
    fn make_plan(&mut self, plan: &RdPlan) -> bool {
        let cs_follows = match (&self.rd, &self.cs) {
            (Some(r), Some(c)) => c.seq == r.seq + 1,
            _ => false,
        };
        let Some(r) = self.rd.as_mut() else { return false };
        r.next2 = plan.next2;
        r.ex_resolved = plan.ex_resolved;
        r.pred_taken = plan.pred_taken;
        r.pred_pr = plan.pred_pr;
        for e in &plan.effects {
            apply_effect(&mut self.copies, *e);
        }
        if let Some(to) = plan.next2 {
            if cs_follows {
                self.npc = to;
                self.npc_after = None;
            } else {
                self.npc_after = Some(to);
            }
        }
        if plan.kills_slot {
            match self.cs.as_mut() {
                Some(c) if cs_follows => c.pre_nop = true,
                _ => self.b.pre_nop_next = true,
            }
        }
        true
    }

    /// Whether CS holds its word this clock: the OA-REG-HIGH hold
    /// (A15b.15), and the PDL buffer's address waiting for a pointer or an
    /// index the word before writes (A15b.3).
    fn cs_hold(&mut self) -> bool {
        let Some(c) = self.cs.as_ref() else { return false };
        if self.draining {
            return true;
        }
        // The OA hold: while a word writing OA-REG-HIGH is in RD or EX, the
        // word that selects it waits in CS, its A address not yet sent
        // (A15b.15: 2, 1 and 0 clocks for a writer 1, 2 and 3 words before).
        // Revision 14's IMOD: the word after a write of either register,
        // which takes it, whatever it is.
        let selects = !self.m.geometry.extended() || c.word.oa_high_select();
        let writer = |s: &Slot| {
            let d = dest_code(s.ir);
            !s.nop
                && if self.m.geometry.extended() {
                    d == Some(0o17)
                } else {
                    matches!(d, Some(0o16 | 0o17))
                }
        };
        let in_rd = self.rd.as_ref().is_some_and(writer);
        let in_ex = self.ex.as_ref().is_some_and(writer);
        let hold = selects
            && match self.mutation {
                Mutation::NoOaHold => false,
                Mutation::OaHoldShort => in_rd,
                _ => in_rd || in_ex,
            };
        if hold {
            self.meters.oa_hold += 1;
            return true;
        }
        // The PDL address: its copy written from the ALU by a word not yet
        // committed.
        let ir = c.word.low_48().raw();
        if let Some(by_pointer) = reads_pdl(ir) {
            let pending =
                if by_pointer { self.copies.pdl_ptr_pending } else { self.copies.pdl_idx_pending };
            if pending.is_some() && self.mutation != Mutation::NoPdlWait {
                self.meters.pdl_wait += 1;
                return true;
            }
        }
        false
    }

    /// **CS's reads**: the A memory at the CS word's address, SH ORing
    /// OA-REG-HIGH as it stood at the clock's start into it, and the PDL
    /// buffer at RD's copy of the pointer or the index, both before this
    /// edge's writes (A15b.3). The word as RD will decode it.
    fn cs_reads(&mut self, oa_high: u64) {
        let extended = self.m.geometry.extended();
        let (imod, oa_low, x_oa_high) = (self.x.imod, self.x.oa_low, self.x.oa_high);
        let mut c = self.cs.take().expect("the CS word");
        let mut ir = c.word.low_48().raw();
        if extended {
            if c.word.oa_high_select() {
                let fields = 0o1777 << 32 | if ir >> 31 & 1 == 0 { 0o37 << 26 } else { 0 };
                ir |= (oa_high << 26) & fields;
            }
        } else if self.x.imod_seq == c.seq {
            // Revision 14's IMOD: the word after an OA write, which the hold
            // let in only once the write committed; no word after it, which
            // leaves CS before the flags are spent.
            c.imod_low = imod[0];
            c.imod_high = imod[1];
            if imod[0] {
                ir |= oa_low;
            }
            if imod[1] {
                ir |= x_oa_high << 26;
            }
        }
        c.ir = ir & ((1 << 48) - 1);
        c.a_addr = field(c.ir, 32, 10) as u16;
        c.a_val = self.m.amem[c.a_addr as usize];
        let by_pointer = field(c.ir, 30, 1) != 0;
        let adr = if by_pointer { self.copies.pdl_ptr } else { self.copies.pdl_idx };
        c.pdl_addr = adr & self.m.geometry.pdl_mask();
        c.pdl_val = self.m.pdl[c.pdl_addr as usize];
        self.cs = Some(c);
        // The write the halt held lands after the first word's read.
        if let Some((adr, word)) = self.b.pdl_pending.take() {
            self.m.pdl[adr as usize] = word;
        }
    }

    /// **A squash's restore** (the contract's §5, Choice 2): RD's copies
    /// from the architectural state, as the word in EX has left it.
    pub(crate) fn restore_copies(&mut self) {
        let keep = self.copies;
        let c = &mut self.copies;
        c.spc_ptr = self.m.spcptr;
        c.spc_top = match self.x.spc_write {
            Some((p, w)) if p == self.m.spcptr => w,
            _ => self.m.spc[self.m.spcptr as usize],
        };
        c.spc_top_pending = None;
        c.spc_top_below = false;
        c.pdl_ptr = self.m.pdl_pointer;
        c.pdl_idx = self.m.pdl_index;
        c.pdl_ptr_pending = None;
        c.pdl_idx_pending = None;
        c.lc = self.m.lc & self.m.geometry.lc_counter();
        c.needfetch = self.m.lc & self.m.geometry.need_fetch() != 0;
        c.byte_mode = self.m.byte_mode();
        c.lc_pending = None;
        c.next_instr = self.x.next_instrd;
        c.operand_after = self.m.macro_dispatch.operand.map(|o| {
            Some(self.m.macro_dispatch.operand_address(o) as u16 & self.m.geometry.pdl_mask())
        });
        match self.mutation {
            Mutation::NoSpcRestore => {
                c.spc_ptr = keep.spc_ptr;
                c.spc_top = keep.spc_top;
                c.spc_top_below = keep.spc_top_below;
                c.spc_top_pending = keep.spc_top_pending;
            }
            Mutation::NoLcRestore => {
                c.lc = keep.lc;
                c.needfetch = keep.needfetch;
                c.next_instr = keep.next_instr;
            }
            _ => {}
        }
    }

    /// The copies a word wrote from the ALU, refreshed as it commits.
    fn refresh_pending(&mut self, seq: u64) {
        let (ptr, idx, lc, nf, bm) = (
            self.m.pdl_pointer,
            self.m.pdl_index,
            self.m.lc & self.m.geometry.lc_counter(),
            self.m.lc & self.m.geometry.need_fetch() != 0,
            self.m.byte_mode(),
        );
        let top = match self.x.spc_write {
            Some((p, w)) if p == self.m.spcptr => w,
            _ => self.m.spc[self.m.spcptr as usize],
        };
        let c = &mut self.copies;
        if c.pdl_idx_pending == Some(seq) {
            c.pdl_idx_pending = None;
            c.pdl_idx = idx;
        }
        if c.pdl_ptr_pending == Some(seq) {
            c.pdl_ptr_pending = None;
            c.pdl_ptr = ptr;
        }
        if c.spc_top_pending == Some(seq) {
            c.spc_top_pending = None;
            c.spc_top = top;
            c.spc_top_below = false;
        }
        if c.lc_pending == Some(seq) {
            c.lc_pending = None;
            c.lc = lc;
            c.needfetch = nf;
            c.byte_mode = bm;
        }
        // INTERRUPT-CONTROL's byte mode, which the guard holds behind.
        c.byte_mode = bm;
    }
}

/// Whether `ir` makes a port-B lookup of `md`, the `MD` it reads:
/// `MAP(MD)`, or a map-bit dispatch on a pointer.
fn reads_port_b(ir: u64, md: Word, m: &crate::machine::Machine) -> bool {
    let map_source = source(ir).is_some_and(|s| s & 0o17 == 0o11);
    let map_dispatch = class(ir) == Op::Dispatch
        && field(ir, 8, 2) != 0
        && field(ir, 10, 2) != 2
        && m.memory_words.pointer_type(md);
    map_source || map_dispatch
}

/// d1 or d2: writes landing at this edge into a word's A and M operands.
pub(crate) fn apply_am(s: &mut Slot, writes: &[AmWrite]) {
    for w in writes {
        if w.a == Some(s.a_addr) {
            s.a_val = w.word;
        }
        if w.m.is_some() && w.m == Some(s.m_addr) && s.ir & 1 << 31 == 0 {
            s.m_val = w.word;
        }
    }
}

/// d3: the writes that landed at the edge the word's A read was made at,
/// into its A operand.
fn apply_a(s: &mut Slot, writes: &[AmWrite]) {
    for w in writes {
        if w.a == Some(s.a_addr) {
            s.a_val = w.word;
        }
    }
}

/// One effect on RD's copies.
pub(crate) fn apply_effect(c: &mut Copies, e: Effect) {
    match e {
        Effect::LcStep => {
            c.next_instr = false;
            if c.lc_pending.is_some() {
                return;
            }
            let counter = crate::machine::LC_COUNTER_14;
            let inc = if c.byte_mode { 1 } else { 2 };
            c.lc = (c.lc & counter).wrapping_add(inc) & counter;
            c.needfetch = false;
            let lc0b = c.byte_mode && c.lc & 1 != 0;
            if !lc0b && c.lc & 2 == 0 {
                c.needfetch = true;
            }
        }
        Effect::LcWritten(seq) => {
            c.lc_pending = Some(seq);
            c.needfetch = true;
        }
        Effect::NextInstr => c.next_instr = true,
        Effect::SpcPush(w) => {
            c.spc_ptr = (c.spc_ptr + 1) & 0o37;
            c.spc_top = w;
            c.spc_top_below = false;
            c.spc_top_pending = None;
        }
        Effect::SpcPushData(seq) => {
            c.spc_ptr = (c.spc_ptr + 1) & 0o37;
            c.spc_top_pending = Some(seq);
            c.spc_top_below = false;
        }
        Effect::SpcPop => {
            c.spc_ptr = c.spc_ptr.wrapping_sub(1) & 0o37;
            c.spc_top_below = true;
            c.spc_top_pending = None;
        }
        Effect::SpcKeep(w) => {
            c.spc_ptr = (c.spc_ptr + 1) & 0o37;
            c.spc_top = w;
            c.spc_top_below = false;
        }
        Effect::PdlPointer(d) => c.pdl_ptr = c.pdl_ptr.wrapping_add(d as u16) & 0o37777,
        Effect::PdlPointerWritten(seq) => c.pdl_ptr_pending = Some(seq),
        Effect::PdlIndexWritten(seq) => c.pdl_idx_pending = Some(seq),
        Effect::PdlIndex(i) => {
            c.pdl_idx = i;
            c.pdl_idx_pending = None;
        }
        Effect::OperandAfter(adr) => c.operand_after = Some(adr),
        Effect::OperandLoad => {
            if let Some(adr) = c.operand_after.take() {
                match adr {
                    Some(a) => {
                        c.pdl_idx = a;
                        c.pdl_idx_pending = None;
                    }
                    None => c.pdl_idx_pending = Some(u64::MAX),
                }
            }
        }
    }
}

/// The registers EX writes, for a test's comparison: the PDL pointer and
/// index, the micro stack's pointer, Q, VMA, MD, LC and INTERRUPT-CONTROL.
pub fn registers_of(m: &crate::machine::Machine) -> [u64; 8] {
    [
        m.pdl_pointer.into(),
        m.pdl_index.into(),
        m.spcptr.into(),
        m.q,
        m.vma,
        m.md,
        m.lc,
        m.interrupt_control.into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::super::Pipeline;
    use crate::machine::{Geometry, Machine, Word};
    use crate::tlb::Port;

    /// **A walk keeps the address it began with** (MP4 ruling Q2, M2): a
    /// walk for X, its directory read a miss, is asked meanwhile for Y,
    /// whose directory entry is in another region and whose page index
    /// holds a decoy under X's table. X's walk ends with X's entry and
    /// answers `None`; Y's own walk follows and loads Y's. A walker that
    /// takes the address it is called with loads the decoy for Y.
    #[test]
    fn a_walk_keeps_the_address_it_began_with() {
        let entry = |frame: u32| 0b11 << 28 | 0o1460 << 18 | frame;
        let mut p = Pipeline::new(Machine::with_geometry(Geometry::QUUX_15, 1));
        let (x, y): (u32, u32) = (0o4000, 1 << 20 | 5 << 10);
        let page = |va: u32| (va >> 10 & 0o1777) as usize;
        p.m.memory_words.directory = 8;
        for (at, frame) in [
            ((8 << 10) + (x >> 20) as usize, 9),
            ((8 << 10) + (y >> 20) as usize, 10),
            ((9 << 10) + page(x), 3),
            ((10 << 10) + page(y), 4),
            ((9 << 10) + page(y), 6),
        ] {
            p.m.main[at] = Word::from(entry(frame));
        }
        assert_eq!(p.walk(x, Port::B), None, "X's directory read is a miss");
        let mut answered = None;
        for _ in 0..1_000 {
            p.clock += 1;
            let now = p.clock;
            p.port.tick(&mut p.m, now);
            p.port_events(now);
            if p.walk(y, Port::B).is_some() {
                answered = Some(now);
                break;
            }
        }
        assert!(answered.is_some(), "Y's walk ends");
        assert_eq!(p.m.tlb.lookup(x), Some(entry(3)), "X's walk loaded X's entry");
        assert_eq!(p.m.tlb.lookup(y), Some(entry(4)), "Y's own entry, not the decoy");
        assert_eq!(p.m.tlb.walks, 2, "two walks");
    }
}
