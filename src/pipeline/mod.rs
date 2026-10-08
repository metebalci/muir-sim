// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **`rtl` on QUUX revision 15: the pipelined micro-engine** (contract G3
//! revision 15, §3, §5, §6, §9 and §12.1; appendix A15b).
//!
//! Four stages of one clock each, so that the microcycle is the clock:
//!
//! - **CS**: the address into the control store; the word out by the next
//!   edge. The A memory's and the PDL buffer's read addresses leave CS with
//!   the word, so both are read at that edge, from the RAMs as they stand
//!   before the edge's writes. An SH word ORs OA-REG-HIGH into its A
//!   source's address here.
//! - **RD**: decode; the M operand, the bypasses d2 and d3 into the A and
//!   M operands and the PDL buffer's forwards; and the next address for
//!   jumps, calls, POPJ and the fused return, which RD resolves, and the
//!   predictions of conditional jumps and dispatches, from RD's own
//!   speculative copies of the micro stack's pointer and top, the PDL
//!   pointer and index, and LC ([`Copies`]).
//! - **EX**: the word runs --- the ALU, the shifter and masker, the
//!   conditions, the dispatch memory --- with d1 into its operands; the
//!   functional registers are written at its end; a conditional jump or a
//!   dispatch is decided and its prediction checked; a memory start leaves
//!   for WB.
//! - **WB**: A, M and the PDL buffer written; a start's translation, its
//!   walk as a hold of WB, its write-back, and the port's grant
//!   ([`port`]).
//!
//! **What each word does is `micro`'s** ([`crate::micro`], the
//! specification): the EX stage runs a microcycle as `micro` runs it, in
//! the same order of microcycles, a delay slot that N inhibits included,
//! so that `rtl` equals `micro` at every boundary. What the pipeline adds
//! is when: the holds, the bubbles, the memory's waits, and the
//! speculation of RD, whose copies and predictions decide which words
//! enter it. A word on a wrong path leaves nothing: nothing a word does is
//! architectural before its EX (the contract's §5, Choice 1).
//!
//! Time is the machine's on revision 15: a u64 of 0.5 ns units, the
//! period a whole number of them, every clock's instant a multiple of it
//! ([`crate::clock::TimeBase`]). Under neutral time ([`Pipeline::neutral`],
//! `MUIR_TIME_NEUTRAL=1`, MP2b ruling Q12) every device and the interrupts
//! see the microcycle's number times the period instead, as `micro` does,
//! and the clocks run on for the meters alone.
//!
//! Revision 14's machine runs here too, as a measurement aid and nothing
//! of the contract's: its IMOD is the word after an OA write held until the
//! write commits and ORed with it ([`Pipeline::new`]).

mod control;
mod exec;
pub mod port;
mod stages;

use crate::clock::TimeBase;
use crate::engine::Engine;
use crate::isa::{Insn, Op};
use crate::machine::{Halt, Machine, Word};
pub use port::{Port, PortTiming};
pub use stages::registers_of;

/// **D's L** (A15b.9): the clocks from the fetched word's arrival to the
/// handler's CS, beyond a fused return on a prefetched word. M8e measured
/// D's arriving word into the store's address 0.308 ns short of the 0.35
/// the rule asks at the Kria's 8.5 ns, so the word is registered first.
pub const L: u64 = 1;

/// A cache of [`Pipeline::new`]'s size when no flag says: the Kria
/// KR260's 64K words (contract §12.1; MP2b ruling Q14).
pub const CACHE_WORDS: u32 = 65_536;

/// The period revision 14 runs at on the pipeline, a measurement aid: in
/// its time's units, ns.
pub const PERIOD_14: u64 = 9;

/// The longest a word may wait in one stage before the engine gives up, in
/// clocks: above a sweep of the largest TLB, a file device's sweep and the
/// late-answering memory model's waits (MP2b ruling Q9).
pub const STALL_BOUND: u64 = 131_072;

/// Where an A or M write lands: A memory's address, and M memory's when
/// the destination is M's (a functional destination writes both).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AmWrite {
    pub a: Option<u16>,
    pub m: Option<u8>,
    pub word: Word,
    /// The writer, for the macro-dispatch copies' snoop.
    pub seq: u64,
}

/// A PDL buffer write: the address, as committed at WB, and the word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PdlAt {
    /// At this address, the pointer the word captured.
    At(u16),
    /// At PDL-INDEX as it stands at WB: the word's own write and an
    /// operand address loaded at its EX's end count, the next word's does
    /// not (A15b.3).
    Index,
}

/// A word in the pipeline.
#[derive(Clone, Debug)]
pub(crate) struct Slot {
    /// Program order.
    pub seq: u64,
    pub pc: u16,
    /// The word as the control store holds it.
    pub word: Insn,
    /// An inhibited microcycle: a delay slot N squashed in RD. It runs no
    /// word and counts as a microcycle, as `micro`'s nopped one.
    pub nop: bool,
    /// RD squashed it ahead of the transfer's EX: a taken transfer with N
    /// in front of it, as RD resolved or predicted it.
    pub pre_nop: bool,
    /// Revision 14's IMOD: OA-REG-LOW and -HIGH ORed into the word, as
    /// the OA write before it left them pending.
    pub imod_low: bool,
    pub imod_high: bool,
    /// The clocks it has spent in RD: d3 is its first's.
    pub rd_clocks: u64,
    /// The word as RD decoded it: `IR<47:0>`, with SH's OR into its A and
    /// M source on revision 15, or IMOD's on revision 14.
    pub ir: u64,
    /// The A operand: its address as CS sent it, its word with the
    /// bypasses so far.
    pub a_addr: u16,
    pub a_val: Word,
    /// The M operand, when M memory's.
    pub m_addr: u8,
    pub m_val: Word,
    /// The PDL buffer's word at the address CS sent, with its forwards.
    pub pdl_addr: u16,
    pub pdl_val: Word,
    /// The address RD chose for the word two after this one: the word
    /// after its delay slot. `None` before RD.
    pub next2: Option<u16>,
    /// EX checks [`Slot::next2`]: a prediction, or a transfer RD left to EX.
    pub ex_resolved: bool,
    /// RD's prediction, which EX checks as well as the address (A15b.2): a
    /// conditional jump's taken or not, a dispatch's entry's P and R.
    pub pred_taken: Option<bool>,
    pub pred_pr: Option<(bool, bool)>,
    /// The microcycle's number, [`Machine::cycles`] once it is counted.
    pub mc: u64,
    /// Clocks spent in EX so far.
    pub ex_clocks: u64,
    /// The clock it entered EX in.
    pub ex_entered: u64,
    /// The boot's trap: a nopped microcycle at the PROM's first word, which
    /// that word follows.
    pub trap: bool,
    /// The late squash held it its clock (A15b.3).
    pub late_held: bool,
    /// A map write landed at its head, and a start in it waits a clock.
    pub map_held: bool,
    /// What WB writes.
    pub am: Vec<AmWrite>,
    pub pdl_w: Option<(PdlAt, Word)>,
    pub start: Option<exec::Start>,
    /// The clock the operands' d1 forward was last taken in.
    pub wb_state: exec::WbState,
}

impl Slot {
    fn new(seq: u64, pc: u16, word: Insn) -> Slot {
        Slot {
            seq,
            pc,
            word,
            nop: false,
            pre_nop: false,
            imod_low: false,
            imod_high: false,
            rd_clocks: 0,
            ir: 0,
            a_addr: 0,
            a_val: 0,
            m_addr: 0,
            m_val: 0,
            pdl_addr: 0,
            pdl_val: 0,
            next2: None,
            ex_resolved: false,
            pred_taken: None,
            pred_pr: None,
            mc: 0,
            ex_clocks: 0,
            ex_entered: 0,
            trap: false,
            late_held: false,
            map_held: false,
            am: Vec::new(),
            pdl_w: None,
            start: None,
            wb_state: exec::WbState::Fresh,
        }
    }
}

/// **RD's speculative copies** (the contract's §5, Choice 2; A15b.3,
/// A15b.16): what RD's next-address and address choices read, ahead of
/// the architectural state EX writes. A word passing RD applies its decoded
/// effects; a squash restores them from the architectural state
/// ([`Pipeline::restore_copies`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Copies {
    /// The micro stack's pointer and top; after a pop, the top is the
    /// stack's word below, read from its RAM ([`Pipeline::top`]).
    pub spc_ptr: u8,
    pub spc_top: u32,
    pub spc_top_below: bool,
    /// The top is a data push (destination 15) not yet committed, by the
    /// word of this sequence: a return that needs it is held (A15b.16).
    pub spc_top_pending: Option<u64>,
    /// The PDL pointer and index, and a write of either from the ALU not
    /// yet committed, by the word of this sequence.
    pub pdl_ptr: u16,
    pub pdl_idx: u16,
    pub pdl_ptr_pending: Option<u64>,
    pub pdl_idx_pending: Option<u64>,
    /// LC's counter, NEEDFETCH, and LC byte mode; a write not yet
    /// committed (HAVE WRONG WORD), by the word of this sequence.
    pub lc: u64,
    pub needfetch: bool,
    pub byte_mode: bool,
    pub lc_pending: Option<u64>,
    /// A pop of a word with `<14>` in the word before: the step of LC this
    /// microcycle takes (`NEXT INSTRD`).
    pub next_instr: bool,
    /// A fused return's operand address, to be loaded into PDL-INDEX after
    /// the word that follows it; `Some(None)` when its bases are still
    /// being written, so that the index is the word's EX's.
    pub operand_after: Option<Option<u16>>,
}

/// The pipeline's meters (the contract's §12.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Meters {
    /// Clocks run, and microcycles retired from WB.
    pub clocks: u64,
    pub retired: u64,
    /// Starts right after a start, by the two starts' kinds, each a read,
    /// a write or an instruction fetch, in that order: a start in the word
    /// right after the word that made the first, or two in one word (MP2b
    /// rulings 2, Q2's measurement). A read after a read is a site that
    /// loses a read on the CADR.
    pub consecutive_starts: [[u64; 3]; 3],
    /// Bubbles: words squashed on a wrong prediction, by kind (a
    /// conditional jump, a dispatch, other words EX resolves), and by the
    /// late squash.
    pub mispredicted: [u64; 3],
    pub squashed: u64,
    pub late_squashes: u64,
    /// Holds, in clocks: the OA-REG-HIGH hold, RD's guard hold, the PDL
    /// address wait, the wait for `MD`, a start behind a start, the
    /// divider and multiplier, D's wait, WB's walk and redirect.
    pub oa_hold: u64,
    pub guard_hold: u64,
    pub pdl_wait: u64,
    pub md_wait: u64,
    pub start_wait: u64,
    pub muldiv: u64,
    pub d_wait: u64,
    pub wb_hold: u64,
    pub map_hold: u64,
    /// CMD_PROD's waits for the queue to drain, in clocks.
    pub cmd_prod_wait: u64,
    /// SL jumps, which RD cannot resolve.
    pub sl_jumps: u64,
    /// RD-resolved transfers whose address `micro`'s semantics in EX would
    /// have chosen otherwise: 0 on a right engine (a test aid).
    pub rd_disagreements: u64,
    /// Fused returns, and D's.
    pub fused: u64,
}

/// **The pipeline**: `rtl` on revision 15.
pub struct Pipeline {
    pub m: Machine,
    /// The clock's period in the machine's units.
    period: u64,
    /// Neutral time (MP2b ruling Q12).
    pub neutral: bool,
    /// Clocks since power-on.
    clock: u64,
    pub port: Port,
    // The stages.
    cs: Option<Slot>,
    rd: Option<Slot>,
    ex: Option<Slot>,
    wb: Option<Slot>,
    /// The address CS loads at next, and the one after it when a squash
    /// has fixed it (a delay slot fetched again before its transfer's
    /// target).
    npc: u16,
    npc_after: Option<u16>,
    /// The next word's sequence number.
    seq: u64,
    /// The A and M writes that landed at the last edge: d3's.
    held_am: Vec<AmWrite>,
    /// The PDL write that landed at the last edge, its address and writer.
    held_pdl: Option<(u16, Word, u64)>,
    copies: Copies,
    /// The executor's state, `micro`'s between microcycles.
    pub(crate) x: exec::Exec,
    /// CS loads nothing before this clock: a control-store write's
    /// refetch, D's word (A15b.9).
    cs_wait_until: u64,
    /// CS loads nothing until the stream's fetch brings D's word.
    d_wait: bool,
    /// While D waits, its delay slot is still to be fetched: after a halt
    /// that squashed it, the word that makes the stream's fetch.
    d_slot_pending: bool,
    /// The microcycles committed in EX, to number the next.
    committed: u64,
    /// `SRUN`, `SSTEP`, `SSDONE`, as `micro` has them.
    srun: bool,
    sstep: bool,
    ssdone: bool,
    /// Draining for a halt: CS and RD squashed, EX and WB completing.
    draining: bool,
    /// The machine is halted, drained.
    halted: bool,
    /// A single step's one word is still to be fetched.
    single_step: bool,
    /// A single step's word is fetched: no other follows it, and the
    /// machine halts when it is done.
    stepping: bool,
    /// The engine stopped the machine: the halt it reports.
    pending_halt: Option<Halt>,
    pub meters: Meters,
    /// The TLB's sweep holds starts and port-B lookups until this clock.
    tlb_sweep_until: u64,
    /// The time-neutral harness's boundary (MP2b ruling Q12 (e)): when
    /// [`Machine::lc_steps`] reaches it, the word that stepped LC is the
    /// last to run before a halt, which drains, so that the state is the
    /// single-edge machine's after that microcycle. The harness reads it and
    /// the run goes on at the next clock.
    pub boundary_at: Option<u64>,
    /// Halted at a boundary, or at the harness's action point.
    boundary_halt: bool,
    action_halt: bool,
    /// The harness's action point: the microcycle to halt after
    /// ([`Engine::stop_after_microcycle`]).
    boundary_mc: Option<u64>,
    /// The address of the microcycle counted last, `None` for a nopped
    /// one: what [`Pipeline::executed`] says.
    last_counted: Option<u16>,
    /// Test aids: a mutation planted by a test.
    pub mutation: Mutation,
    /// Every microcycle committed, in order, when a test asks for the record
    /// by setting it to `Some`: its address, `None` for a nopped one.
    pub trace: Option<Vec<Option<u16>>>,
    /// Each executed word's A and M operands and its output, in order, when
    /// a test asks for the record.
    pub operands: Option<Vec<(u16, Word, Word, Word)>>,
    /// The registers EX writes, after each microcycle, when a test asks.
    pub registers: Option<Vec<[u64; 8]>>,
    /// What happened at which clock, when a test asks ([`Event`]).
    pub events: Option<Vec<(u64, Event)>>,
    /// WB's and the port's state ([`stages::Back`]).
    pub(crate) b: stages::Back,
}

/// What [`Pipeline::events`] records, with the clock it happened in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A word committed at EX's end: its address, `None` for a nopped
    /// microcycle.
    Commit(Option<u16>),
    /// A start granted at WB: its bus address, and whether a write.
    Grant(u32, bool),
    /// A read's word landed in `MD`.
    Md(Word),
    /// A device register taken: its bus address.
    Register(u32),
}

/// **A planted fault**, for the tests that show a check catches it. A run
/// has none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mutation {
    #[default]
    None,
    /// The A memory's d3 bypass left out.
    NoD3,
    /// A squash leaves RD's micro stack copies as the squashed words left
    /// them.
    NoSpcRestore,
    /// A squash leaves RD's LC copy as the squashed words left it.
    NoLcRestore,
    /// RD's LC copy not stepped by the word before's step.
    LcNotStepped,
    /// RD's LC copy not marked written by an LC destination.
    LcNotMarked,
    /// RD's guard hold removed.
    NoGuard,
    /// A squashed word's memory start made.
    SquashedStart,
    /// A squashed word's port-B lookup made, a miss walked and filled.
    SquashedLookup,
    /// Block-disk's START leaves the posted writes queued and in flight.
    NoLandAtStart,
    /// A PDL read through a pointer or index the word before writes from
    /// the ALU does not wait for it.
    NoPdlWait,
    /// The d1 bypass, WB's write into the word in EX, left out.
    NoD1,
    /// The d2 bypass, WB's write into RD's operand, left out.
    NoD2,
    /// A push's SPC word written at once, so that an M read of the stack
    /// in the next microcycle reads it.
    SpcWriteAtOnce,
    /// `MAP(MD)` right after a map store reads the new entry.
    MapSeenNew,
    /// The words behind a `WRITE-I-MEM` not fetched again.
    NoImemRefetch,
    /// The words behind a `WRITE-I-MEM` fetched again from the write's own
    /// address + 1, not from the word after it in execution order.
    ImemRefetchAfterItsAddress,
    /// A write carries `MD` as its start left it, whatever the microcycle
    /// after loads.
    WriteMdAtStart,
    /// The word right after a read start waits for the read's word, as
    /// every later word that uses `MD` does.
    SuccessorWaitsForMd,
    /// CMD_PROD taken at once, its earlier writes still queued or in
    /// flight.
    NoCmdProdWait,
    /// The file device's completion sweep leaves the set of the line last
    /// filled.
    SweepMissesASet,
    /// Block-disk's transfer leaves the sets of the words it writes.
    NoClearSet,
    /// A read's word that lands while a start is held behind the read is
    /// dropped, as `micro` dropped it.
    FirstReadDropped,
    /// A delay slot N inhibits counted twice.
    NopCountedTwice,
    /// The OA-REG-HIGH hold removed, or one clock short.
    NoOaHold,
    OaHoldShort,
    /// A wrong-path OA writer loads its register.
    SquashedOaLoads,
    /// The late squash lets the check in EX commit as predicted.
    NoLateSquash,
    /// The hint read inverted.
    HintInverted,
    /// The PDL buffer's forward from WB left out.
    NoPdlForward,
    /// A check samples the interrupt in RD, the clock before it entered
    /// EX, and not in its last EX clock (MP2b ruling Q13).
    InterruptSampledInRd,
}

impl Pipeline {
    /// The pipeline running `m`: revision 15, or revision 14 as a
    /// measurement aid. The Kria's period, memory timing and cache unless
    /// [`Pipeline::configure`] says otherwise (MP2b ruling Q14).
    pub fn new(m: Machine) -> Pipeline {
        assert!(m.geometry.paged(), "the pipeline runs QUUX revisions 14 and 15");
        let period = if m.geometry.extended() { crate::clock::PERIOD_15 } else { PERIOD_14 };
        let time = Self::time_of(&m, period);
        let mut p = Pipeline {
            port: Port::new(PortTiming::KRIA, time, CACHE_WORDS),
            m,
            period,
            neutral: false,
            clock: 0,
            cs: None,
            rd: None,
            ex: None,
            wb: None,
            npc: 0,
            npc_after: None,
            seq: 1,
            held_am: Vec::new(),
            held_pdl: None,
            copies: Copies::default(),
            x: exec::Exec::default(),
            cs_wait_until: 0,
            d_wait: false,
            d_slot_pending: false,
            committed: 0,
            srun: false,
            sstep: false,
            ssdone: false,
            draining: false,
            halted: false,
            single_step: false,
            stepping: false,
            pending_halt: None,
            meters: Meters::default(),
            tlb_sweep_until: 0,
            boundary_at: None,
            boundary_halt: false,
            action_halt: false,
            boundary_mc: None,
            last_counted: None,
            mutation: Mutation::None,
            trace: None,
            operands: None,
            registers: None,
            events: None,
            b: stages::Back::default(),
        };
        p.m.period = if p.m.geometry.extended() { period } else { 0 };
        p.x.lvmo = p.m.geometry.lvmo_at_power_on();
        p
    }

    fn time_of(m: &Machine, period: u64) -> TimeBase {
        if m.geometry.extended() {
            TimeBase::half_ns(period)
        } else {
            TimeBase { per_ns: 1, period }
        }
    }

    /// The machine's time base with this engine's period.
    pub fn time(&self) -> TimeBase {
        Self::time_of(&self.m, self.period)
    }

    /// The period, the memory's timing and the cache's words, before the
    /// machine runs (`--microcycle-ns`, `--memory-timing`, `--cache`).
    pub fn configure(&mut self, period: u64, timing: PortTiming, cache_words: u32) {
        self.period = period;
        if self.m.geometry.extended() {
            self.m.period = period;
        }
        self.port = Port::new(timing, self.time(), cache_words);
    }

    pub fn period(&self) -> u64 {
        self.period
    }

    /// Clocks run since power-on.
    pub fn clock(&self) -> u64 {
        self.clock
    }

    /// The instant clock `k` ends at, in the machine's units.
    fn instant(&self, k: u64) -> u64 {
        k * self.period
    }

    /// The machine's time for a device access by the microcycle `mc` made
    /// in this clock: neutral time, or the clock's own instant.
    fn device_time(&self, mc: u64) -> u64 {
        if self.neutral { self.instant(mc) } else { self.instant(self.clock) }
    }

    /// Whether the pipeline holds nothing: no word in any stage and the
    /// port drained.
    pub fn drained(&self) -> bool {
        self.cs.is_none()
            && self.rd.is_none()
            && self.ex.is_none()
            && self.wb.is_none()
            && self.port.idle(self.clock)
    }

    /// The stages' words, for a test: CS, RD, EX and WB's addresses, a
    /// nopped slot as `None` within `Some`.
    pub fn stages(&self) -> [Option<Option<u16>>; 4] {
        let f = |s: &Option<Slot>| s.as_ref().map(|s| (!s.nop).then_some(s.pc));
        [f(&self.cs), f(&self.rd), f(&self.ex), f(&self.wb)]
    }

    // --- The clock -------------------------------------------------------------

    /// **One clock** ([`Pipeline::clock_once`]).
    pub fn tick(&mut self) -> Result<(), Halt> {
        self.clock_once()
    }
}

impl Engine for Pipeline {
    fn nominal_cycle_ns(&self) -> u64 {
        // The run's report and `--pace` read nanoseconds.
        (self.period / self.time().per_ns).max(1)
    }

    fn boot(&mut self) {
        self.boot_15();
    }

    fn step(&mut self) -> Result<(), Halt> {
        self.step_15()
    }

    fn pc(&self) -> u16 {
        self.pc_15()
    }

    fn spy_read(&self, eadr: u8) -> u16 {
        self.spy_15(eadr)
    }

    fn stop_after_microcycle(&mut self, mc: u64) {
        // A microcycle already committed is past stopping after: the next.
        self.boundary_mc = Some(mc.max(self.committed + 1));
    }

    fn settle_for_checkpoint(&mut self) -> Result<(), Halt> {
        self.halt_between_microcycles()
    }

    fn stands_between_microcycles(&self) -> bool {
        self.is_halted() && (self.action_halt || self.boundary_halt)
    }

    fn machine(&self) -> &Machine {
        &self.m
    }

    fn machine_mut(&mut self) -> &mut Machine {
        &mut self.m
    }

    fn save(&self, w: &mut crate::checkpoint::Writer) {
        self.save_15(w);
    }

    fn load(&mut self, r: &mut crate::checkpoint::Reader) -> std::io::Result<()> {
        self.load_15(r)
    }
}

/// `IR<hi:lo>` of `ir`.
pub(crate) fn field(ir: u64, pos: u32, len: u32) -> u32 {
    ((ir >> pos) & ((1u64 << len) - 1)) as u32
}

/// Whether `ir` is a word of class `op`.
pub(crate) fn class(ir: u64) -> Op {
    Insn::new(ir).op()
}
