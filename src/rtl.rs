// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `rtl` engine: the CADR datapath, on the machine's own clock.
//!
//! A microcycle is two phases, not six states. `-CLK0` is `-TPCLK AND
//! MACHRUN` at CLOCK2 1D10 and `CLK1..CLK5` are `NOT(-CLK0)` through the 7428
//! buffers at 1D05, 1C01 and 1C11, so every edge-triggered register on the
//! board takes **one edge per microcycle**, at the cycle boundary.
//! `tests/rtl.rs` measures that on the running board rather than assuming it.
//!
//! Between edges the clock is a level, and the board reads it as one:
//!
//! - **read phase**, `CLK` high. The scratchpad address multiplexers at ACTL
//!   3A16, MCTL 4B19 and PDLCTL 4C22 select the instruction's own source
//!   fields, the 74S373s at ALATCH, MLATCH, PLATCH and SPCLCH follow the
//!   memories, `-TSE1..4` let the sources drive, and the ALU settles.
//! - **write phase**, `CLK` low. The multiplexers select `WADR`, the latches
//!   hold what they caught, and `-WP1..4` write.
//!
//! So one microcycle is `Rtl::read_phase`, which evaluates the whole
//! combinational network once, then `Rtl::write_phase`, then
//! `Rtl::clock_edge`, where every register takes what the read phase held.
//!
//! Three things about the board are easy to model one step out of place, and
//! all three are read off `data/CADR.netlist` here.
//!
//! **The scratchpads are asynchronous and the latches are the state.** They
//! are 93425As with no clock pin, so what holds a word between phases is the
//! 74S373 at ALATCH, MLATCH, PLATCH or SPCLCH and not the memory. Modeling
//! the memory as a register loaded early is the 74S373 drawn one state too
//! soon.
//!
//! **A scratchpad write is one microcycle behind the instruction that
//! computed it.** `WADR`, `DESTD`, `DESTMD`, `PDLWRITED` and `WMAPD` are all
//! registered on the clock edge, so the pulse that stores a word belongs to
//! the cycle after the one that produced it. What hides that from the
//! microcode is the pass-around: the comparators at ACTL 3B21/3B27 and MCTL
//! 4B18 match the instruction's source address against the pending `WADR`
//! and put `L` on the bus instead of the memory's output. Writing
//! immediately with no pass-around gives the same answer to every program,
//! so nothing the microcode can do tells the two apart --- only the
//! diagnostic interface, reading a scratchpad between the two cycles.
//!
//! **There is no MMU state.** Both map levels are asynchronous and chained
//! combinationally: VMEM0 1C14 is addressed by `MAPI13..23` and drives
//! `-VMAP4..0`, which are VMEM1 1E04's address along with `-MAPI8A..12A`. A
//! lookup is a ripple through two rams inside one cycle. `MAPI` itself is
//! `VMA` while `MEMSTART` is up and `MD` otherwise, off the 74S258s at VMAS
//! 1C20 and its fellows.
//!
//! **The memory cycle is the board's too**, from MIT's own specification of
//! it: `MEMPREPARE` to `MEMSTART` to `-MEMRQ`, `MBUSY` waiting on `-MEMACK`
//! from the bus interface in `src/busint.rs`, `-LOADMD` strobing `MD` when
//! the interface says and `READ IN PROGRESS` falling 140 ns after it,
//! with `WAIT` and `HANG` holding the clock generator off meanwhile. A stall
//! costs time and not a microcycle. `Rtl::stall` has the two gates and
//! `Rtl::after_memack` the two delays.

use crate::busint::{self, Busint, Responder};

/// How long after `-MEMACK` `READ IN PROGRESS` falls, and with it `-HANG`.
///
/// `-RDFINISH` is `-MFINISH` through the TD50 at VCTL1 1D23 and then the
/// TD250 at 1D22; on the tap ordering `src/part.rs` records that is 40 ns
/// plus the second tap of 250, so 140. MIT's own interface specification says
/// "MEMACK delayed by about 150 ns", which is the same number rounded.
/// Nothing in the boot turns on which of the two it is.
const RD_FINISH_NS: u64 = 140;

/// How far into a generator cycle `SPEEDCLK` clocks the speed synchronizer
/// at OLORD1 1A01: `src/clock.rs` has the derivation.
const SPEEDCLK_NS: u64 = 60;

/// How long after `-MEMACK` `MBUSY` clears.
///
/// The 74S74 at VCTL1 1D21 that holds `MBUSY` is cleared by `-MFINISHD`:
/// `-MEMACK` through the 74S08 at 1D28 and the 30 ns tap of the TD50 at
/// 1D23, the tap `tests/busint.rs` measures. `MBUSY.SYNC` is `MEMRQ` ---
/// `MBUSY` among its terms --- registered at the master clock, so an
/// acknowledgement inside the last 30 ns of a generator cycle clears
/// `MBUSY.SYNC` one edge later than one before them, and a `WAIT` on it
/// lasts a cycle longer. Clearing `MBUSY` in the instant of the
/// acknowledgement instead is invisible at 220 ns, where nothing falls in
/// those 30 --- and shows at 145, in `GET-AREA-ORIGINS`, as a cycle the
/// board waits and the model does not.
const MFINISHD_NS: u64 = busint::MFINISHD_NS;

// What the latch at VMEMDR comes up holding, as `chip` has it,
// `Geometry::lvmo_at_power_on`.
//
// `Chip::power_on` puts every register's outputs low, and the latch's are
// the active-low `-LVMO23`, `-LVMO22` and `-PMA21..8`, so the positive word
// is the two permission bits and the page all ones. The board's own
// power-on state is undefined, so this is a convention shared with `chip`
// and not a fact about the hardware; what it decides is bit 30 of the first
// `MAP(MD)` the boot PROM reads, which nothing uses.

/// Why the clock generator is being held off.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stall {
    /// `-WAIT`: the cpu clock stops and the master clock runs on.
    Wait,
    /// `-HANG`: both clocks stop.
    Hang,
}
use crate::clock::{Speed, TimingModel};
use crate::engine::Engine;
use crate::machine::{Halt, IMEM_WORDS, Machine, Word};
use crate::spy;
use crate::ttl;

pub struct Rtl {
    pub m: Machine,

    /// The datapath during the read phase of the last microcycle; see
    /// [`Rtl::signals`].
    trace: [u64; 12],
    /// The diagnostic flags over the same microcycle; see [`Rtl::spy`].
    flags: [u64; 12],

    // --- IREG 3C01-3D07, 25S09 on CLK3A and CLK3B ---
    ir: u64,
    // --- IWR 4B01-4C05 on CLK4C, 1F12/1F14 on CLK2C ---
    iwr: u64,

    // --- ACTL 3B26 74S174 and 3B28/3B29 25S09, all on CLK3D ---
    /// The destination address of the instruction *before* this one, which is
    /// the address the write pulse in this cycle will use.
    wadr: u16,
    /// `DESTD`: that instruction had a destination, so the A memory is
    /// written this cycle. Gates `-AWPA` at ACTL 3B30.
    destd: bool,
    /// `DESTMD`: it was an M destination. Gates `-MWPA` at MCTL 4B22, and is
    /// the sixth bit the M pass-around comparator at 4B18 matches on.
    destmd: bool,

    // --- L 3C26-3C29 74S374 on CLK3F ---
    l: Word,

    // --- NPC 4E04/4E05 74S374 and LPC 4F06-4F08 25S07, both on CLK4B ---
    pc: u16,
    lpc: u16,

    // --- LC 1A26-2C05 74S169 counters.  The byte-mode flags are FLAG's, on
    // --- page FLAG, and not bits of this counter.
    lc: u64,

    // --- PDLCTL 4C11 74S175 on CLK4F ---
    /// `PWIDX`: the pending PDL write is to the index rather than the
    /// pointer, which is what addresses the PDL during the write phase.
    pwidx: bool,
    /// `PDLWRITED`, gating `-PWPA` at 4D20.
    pdlwrited: bool,

    // --- CONTRL 3D26 74S175 on CLK3C ---
    inop: bool,
    iwrited: bool,

    // --- LCC 3E12 74S175 on CLK3C and 1C27 25S09 on CLK2A ---
    newlc: bool,
    sintr: bool,
    next_instrd: bool,

    // --- FLAG 3E08 25LS2519 on CLK3C ---
    lc_byte_mode: bool,
    int_enable: bool,
    sequence_break: bool,
    prog_unibus_reset: bool,

    // --- TRAP and OLORD1 ---
    boot_trap: bool,
    /// `PROMDISABLE` a second time: the 74S174 at OLORD1 1A10 registers the
    /// mode register's bit on `MCLK5A`, and it is this copy that gates the
    /// boot PROM over the bottom of the control store.
    promdisabled: bool,
    /// `SRUN`, `SSTEP` and `SSDONE`: the console's `RUN` and `STEP` as the
    /// 74S174 at OLORD1 1A10 registers them on `MCLK5A`, `STEP` twice over.
    /// `MACHRUN` is up while `SRUN` is, or for the one master clock in
    /// which `SSTEP` is up and `SSDONE` is not.
    srun: bool,
    sstep: bool,
    ssdone: bool,
    /// The edge that clocked the instruction standing in `IR` in, moved on
    /// to the end of each generator cycle the MD interlock holds it: QUUX's
    /// divider's count started there. The CADR has no divider; the
    /// instant is kept in its checkpoint's format.
    div_from_ns: u64,
    /// This microcycle's write pulse has fired already: a `-HANG` holding
    /// it fires the pulse before the hold ([`Rtl::step`]).
    pulsed: bool,
    /// `STATSTOP`, the 74S374 at OLORD2 1A05 on `CLK5A`: the statistics
    /// counter's carry out, `STAT.OVF`, registered.  Under `STATHENB` it is
    /// `STATHALT`.
    statstop: bool,
    /// `HALTED`, the 74S374 at OLORD2 1A05 on `CLK5A`: `-HALT` registered.
    /// `-HALT` is misc function 1 --- `-FUNCT1` off the 74S139 at SOURCE
    /// 3D05, `HALT-CONS` in MIT's assembler --- carried between the two
    /// processor boards on connector pins 3CJ1-19 and 1CJ2-21
    /// (`cadrwd/cadr4.wlr`, `cadrwd/icmem3.wlr`). It is `ERR` through the
    /// 74S133 at 1A02, and under `ERRSTOP` the halt.
    halted: bool,
    /// The level on the OPCS shift registers' clock as of the last edge
    /// this engine ran, for finding the rising edge a console makes by
    /// lowering `OPCCLK` on a halted machine.  See [`Rtl::opc_clock`].
    opc_ck: bool,
    /// Nanoseconds spent halted --- the master clock running and the cpu's
    /// held off --- for [`Rtl::halted_ns`].
    halted_ns: u64,

    // --- VCTL1 1E20 74S175 on MCLK1A and 1C23 74S175 on CLK2A ---
    /// `MEMSTART`, the registered `MEMPREPARE`. While it is up the map is
    /// addressed by `VMA` rather than `MD`, and the latch at VMEMDR 1D14 is
    /// transparent.
    memstart: bool,
    mbusy: bool,
    rdcyc: bool,
    wrcyc: bool,
    /// The bus interface at the far end of the cables, and the cycle it is
    /// running: the
    /// address and the word are held by it for the whole cycle, which is
    /// why they are captured when `MBUSY` rises rather than read again when
    /// the answer comes back.
    bus: Box<Busint>,
    /// `MBUSY.SYNC`, the second flip flop of the 74S175 at VCTL1 1E20:
    /// `MEMRQ` registered on `MCLK1A`. "Since you must wait during the first
    /// half of a clock cycle, the busy condition (MEMRQ) must be
    /// synchronized."
    mbusy_sync: bool,
    bus_addr: u32,
    bus_data: Word,
    /// The word of the cycle in flight has reached its slave, at
    /// [`Busint::answered_at`].
    bus_written: bool,
    /// The cycle in flight reads one of the sixteen diagnostic registers,
    /// and which: the word is this engine's own state on `SPY<15:0>` under
    /// `-DBREAD`, sampled at the interface's register strobe rather than
    /// fetched from [`Machine`] at the acknowledgement.
    bus_spy: Option<u8>,
    /// That sample has been taken.
    bus_sampled: bool,
    /// The write in flight is to the mode register, and the leading edge of
    /// its pulse has been taken: `-PROG.RESET` and `PROG.BOOT` are asserted
    /// [`busint::REGISTER_PULSE_NS`] before the register loads.
    bus_pulsed: bool,
    bus_responder: Responder,
    /// `READ IN PROGRESS`, the 74S74 at VCTL1 1D21: "loads from MEMSTART and
    /// VMAOK and RDCYC or self", and cleared by `MEMACK` delayed.
    rd_in_progress: bool,
    /// When that delay ends. See [`RD_FINISH_NS`].
    rd_finish_at: u64,
    /// When `-MFINISHD` clears `MBUSY`, or `u64::MAX` when no
    /// acknowledgement is waiting to.
    mbusy_clear_at: u64,
    /// The cycle in flight has been acknowledged and its word taken; what
    /// is left is `-MFINISHD`.
    bus_acked: bool,

    // --- the debug cable on DBGIN, the other master of the Unibus ---
    /// The word this side puts on `DBD<15:0>` for the debugger's read, once
    /// the slave has given it: the cpu's own register off `SPY<15:0>`
    /// through the 8304s at DIAG 0A20 and 0A21 onto `UDO` and the ones at
    /// DBGOUT 0B21 and 0B22 onto the cable, or one of the interface's
    /// other registers by the same `UDO`.  `None` while nothing drives
    /// the bus.
    debug_word: Option<u16>,
    /// The read's word has been sampled at the slave's answer.
    debug_sampled: bool,
    /// The write's word has reached its register.
    debug_written: bool,
    /// The leading edge of the write pulse has been taken for a write of
    /// the mode register, as [`Rtl::land_write`] takes it.
    debug_pulsed: bool,
    /// The answer to the last request on the cable: when `DEBUG ACK` rose,
    /// and the word on `DBD`.  Kept until the next request.
    debug_answer: Option<(u64, Option<u16>)>,
    /// [`busint::debug_modifier::RESET`] as last seen, for its edges.
    debug_reset: bool,
    /// Debug cycles this machine has put on the cable as the debugger:
    /// its writes and reads of `766100`, not the register strobes.
    debug_cycles: u64,
    /// The clock generator held at `-TPR0` by that reset: no master clock
    /// edge, no microcycle, until the bit is lifted, and then a cycle from
    /// that instant --- as the netlist's clock does under `RESET`.
    clock_held: bool,

    // --- the debug cable on DBGOUT, this machine as the debugger ---
    /// The word the other machine put on `DBD<15:0>` for the debug-block
    /// read in flight, as the 8304s at DBGOUT 0B21 and 0B22 carry it onto
    /// `UDO`, the 8838s on page UBD onto `-UBD`, and the receivers into
    /// `BUS` under `-UB16>BUS`.  All ones for a `DBD` nothing drove, a
    /// floating TTL input reading high.  The board leaves it floating:
    /// `cadr1/busint.wlr` gives `DBD0` six pins and no terminator among
    /// them --- the two cable connectors J05-01 and J06-01, the 8304 bus
    /// drivers at REQERR B15-19 and DBGOUT B21-12, and the LS inputs at
    /// DBGIN A16-16 and A18-18 --- so with both 8304s off there is nothing
    /// to pull the line down, and the fifteen other bits are wired the same
    /// way.  What has not been done is the cross-check: a `chip` debug read
    /// of an address nothing answers, through the netlist interface's
    /// DBGOUT, would show the engines agreeing on it.
    debug_out_word: u16,
    /// [`Rtl::step_until`]'s bound on the step in progress.
    step_limit: u64,
    /// `WMAPD`, gating both map write pulses at VCTL2 1C15.
    wmapd: bool,

    /// `-LVMO`, the 74S373s at VMEMDR 1D14 and 1D15. Transparent while
    /// `MEMSTART` is up, so it holds the map word the last memory cycle saw.
    /// `-PFR` and `-PFW` come from this and not from the live map output.
    ///
    /// Held positive here. What it comes up holding is undefined on the
    /// board, and it is read once before any memory cycle has loaded it: the
    /// `MEMORY-MAP-DATA` at `SET-UP-THE-MAP`, which keeps only `-VMAP<4:0>`
    /// of what it gets. See [`crate::machine::LVMO_AT_POWER_ON`].
    lvmo: u32,
    /// Nanoseconds spent in waits and hangs, for [`Rtl::stalled_ns`].
    stalled_ns: u64,
    /// Memory cycles started, for [`Rtl::bus_cycles`].
    bus_cycles: u64,

    /// `SPUSHD` at CONTRL 3D26: a push is pending, so the write pulse stores
    /// `SPCW` this cycle and `SPCWPASS` puts it on the bus meanwhile.
    spushd: bool,
    /// `DESTSPCD` at PDLCTL 4C11, which selects the 74S157s at SPCW 4E12-4E14:
    /// `SPCW` is `L` for a `DESTSPC` and the return address otherwise.
    destspcd: bool,
    /// `RETA`, the 25S09s at SPCW 4F11-4F14: a register whose input mux takes
    /// `WPC` when `N` and `IPC` otherwise. Registered, so the address it
    /// carries belongs to the instruction whose push is pending.
    reta: u16,

    /// `IMODD` at PDLCTL 4C11. Nothing in the datapath reads it: it
    /// suppresses the control-store parity check at 4E03 and the console
    /// reads it over the SPY bus.
    imodd: bool,

    // --- OPCS 1F06-1F13 ---
    /// The 9328s are dual *eight-bit* shift registers with `PC` shifted in
    /// and only the last stage brought out, so `OPC<13:0>` --- what page
    /// OPCD drives onto `MF` for a `SRCOPC` --- is the PC of eight
    /// microcycles ago. Newest first. A single register one cycle behind is
    /// the easy thing to write and is not what OPCS holds; `tests/rtl.rs`
    /// measures the real depth on the board.
    opc: [u16; 8],

    // --- STAT 1B01-1C05 ---
    /// Eight 74S169s chained into a 32-bit counter, clocked by `CLK5A` and
    /// enabled by `-STATBIT`, so it counts the microcycles whose instruction
    /// has `IR<46>` set. It counts *up*: the 74S169s' `U/D` pins, pin 1 of
    /// 1B02 to 1B05 on page STAT, are on `HI1`. `ir.bits` calls it "a 32-bit
    /// down-counter" that stops the machine at zero; the board is followed.
    /// Presettable from `IWR` by `-LDSTAT` and read out over SPY through the
    /// 74LS244s at 1B06-1B09. Nothing the microcode can read reaches it ---
    /// only CC, over the diagnostic bus.
    stat: u32,

    // --- OLORD1 1A01, the mode register's speed bits ---
    /// `SSPEED1, SSPEED0`, which pick the delay-line tap that ends the read
    /// phase. Cleared at reset, so the machine comes up extra slow; the
    /// console writes them, and so does microcode 323, with `46`, as soon as
    /// it runs. The 74S174 at 1A01 is a two-stage synchronizer on
    /// `SPEEDCLK`, which is `-TPR60` inverted: `SPEED` to `SPEEDA` sixty
    /// nanoseconds into one generator cycle and `SPEEDA` to `SSPEED` into
    /// the next, waits included, and the 74S151 that picks the tap switches
    /// before the earliest tap at 75, so that second cycle already runs at
    /// the new length. See [`Rtl::speedclk`].
    speed: Speed,
    /// `SPEED1A, SPEED0A`, the first stage.
    speed_a: Speed,
    /// Whose nanoseconds [`Rtl::ns`] counts: the board's, or muir-fpga's
    /// grid. See [`TimingModel`].
    timing: TimingModel,
    /// Simulated nanoseconds since reset, off the delay-line taps. The
    /// engine's own rate has nothing to do with it.
    ns: u64,

    // --- the far end of the cables ---
    /// `MEM<31:0>`: the word the bus interface holds after a read.
    busint_bus: Word,
    /// When `-LOADMD` strobes that word into `MD`, or `u64::MAX` when no
    /// read is waiting to land. "Equals MEMACK and RDCYC ... Loads MD from
    /// MEM, asynchronous with clock": with the acknowledgement for an Xbus
    /// word, before it for a Unibus one, as [`busint::Ack`] says.
    loadmd_at: u64,

    executed: Option<u16>,
    /// Whether the last microcycle started a macroinstruction fetch, and if
    /// so whether for the wrong word (`LC` written) rather than for the
    /// next in sequence. For the profile harness; not in a checkpoint,
    /// being cleared every microcycle.
    fetch_started: Option<bool>,
    /// `MEMSTART`'s cycle is the stream's macroinstruction fetch:
    /// registered with `MEMSTART`, and kept in a checkpoint as it is.
    memstart_fetch: bool,
}

/// The combinational network during the read phase.
///
/// Only what something downstream reads, rather than every named net of the
/// drawings. The diagnostic interface wants names, and it takes them off the
/// nets it actually needs.
struct Read {
    nop: bool,
    irdisp: bool,
    /// `-HALT`: misc function 1 decoded from `IR<11:10>`, unless nopped.
    halt: bool,

    /// The next instruction, off the control store at `PC`.
    i: u64,

    a: Word,
    m: Word,
    alu: u64,
    r: Word,
    ob: Word,

    n: bool,
    npc: u16,

    dest: bool,
    destm: bool,
    destmdr: bool,
    destlc: bool,
    destintctl: bool,
    destimod0: bool,
    destimod1: bool,
    destpdlp: bool,
    destpdlx: bool,
    destpdl_x: bool,
    destspc: bool,

    wadr_in: u16,
    srcpdlpop: bool,
    wmap: bool,
    memwr: bool,
    memop: bool,
    /// `USE.MD` is `NOR(-SRCMD, NOPA)` at VCTL1 3F18: this instruction reads
    /// `MD` and is not nopped.  It is half of `-HANG`.
    use_md: bool,
    /// `DESTMEM`, and `NEEDFETCH`: two of the three terms of `-WAIT`.
    destmem: bool,
    needfetch: bool,
    /// `HAVE WRONG WORD`, page LCC: `LC` was written since the last fetch.
    have_wrong_word: bool,
    lcinc: bool,

    spush: bool,
    spcnt: bool,
    spcw: u32,
    reta_in: u16,

    pdlwrite: bool,
    pdlcnt: bool,

    dispwr: bool,
    dadr: u16,

    vmas: Word,
    vmaenb: bool,
    vmo: u32,

    /// `ILONG` is `IR<45>`, suppressed when the cycle is nopped by a jump or
    /// the console but not by the trap: `-ILONG` is `NAND(IR45, -NOPA)` at
    /// FLAG 3E07. It stretches the read phase by 40 ns and changes nothing
    /// else.
    ilong: bool,
    /// `STATBIT` is `IR<46>` the same way, off the other half of 3E07.
    statbit: bool,
    /// `IMOD`, the 74S10 at PDLCTL 4D10: `NAND(-DESTIMOD1, -IDEBUG, x)`
    /// where `x` is `AND(-DESTIMOD0, -IWRITED)` off the 74S08 at SOURCE
    /// 3E05 --- so a `WRITE-I-MEM` raises it too; MIT's wire list has that
    /// gate's inputs on `-DESTIMOD0` and `-IWRITED`. `IDEBUG` is the console's:
    /// the debug IR carries no parity bit, so its word is exempted from the
    /// check the same way a written one is.
    imod: bool,
    jcond: bool,
    pcs1: bool,
    pcs0: bool,

    next_instr: bool,
    newlc_in: bool,
    qs1: bool,
    qs0: bool,
    iwrite: bool,
    vmaok: bool,
    machrun: bool,
    /// The single-step term of `MACHRUN`, `SSTEP AND -SSDONE`, which does
    /// not go through `-WAIT`.
    stepping: bool,
}

/// A word's `<31:0>`: the CADR's whole word, and what the arithmetic,
/// the shifts and the rotator act on in a 40-bit one (contract G2 §2.2).
const LOW: Word = 0xffff_ffff;

fn bit(v: u64, n: u32) -> bool {
    (v >> n) & 1 != 0
}

fn field(v: u64, hi: u32, lo: u32) -> u32 {
    ((v >> lo) & ((1u64 << (hi - lo + 1)) - 1)) as u32
}

impl Rtl {
    pub fn new(m: Machine) -> Self {
        // QUUX's `rtl` is the pipeline (`crate::pipeline`): this engine is
        // the CADR's single-edge machine.
        assert!(!m.geometry.quux(), "rtl runs the CADR; QUUX's rtl is the pipeline");
        let lvmo_at_power_on = m.geometry.lvmo_at_power_on();
        let m_bus = Box::new(Busint::with_timing_model(m.memory_boards(), TimingModel::Cadr));
        Rtl {
            m,
            trace: [0; 12],
            flags: [0; 12],
            ir: 0,
            iwr: 0,
            wadr: 0,
            destd: false,
            destmd: false,
            l: 0,
            pc: 0,
            lpc: 0,
            lc: 0,
            pwidx: false,
            pdlwrited: false,
            inop: false,
            iwrited: false,
            newlc: false,
            sintr: false,
            next_instrd: false,
            lc_byte_mode: false,
            int_enable: false,
            sequence_break: false,
            prog_unibus_reset: false,
            boot_trap: false,
            promdisabled: false,
            srun: false,
            sstep: false,
            ssdone: false,
            div_from_ns: 0,
            pulsed: false,
            statstop: false,
            halted: false,
            opc_ck: false,
            halted_ns: 0,
            memstart: false,
            mbusy: false,
            bus: m_bus,
            mbusy_sync: false,
            bus_addr: 0,
            bus_data: 0,
            bus_written: false,
            bus_spy: None,
            bus_sampled: false,
            bus_pulsed: false,
            bus_responder: Responder::Memory(0),
            rd_in_progress: false,
            rd_finish_at: u64::MAX,
            mbusy_clear_at: u64::MAX,
            bus_acked: false,
            debug_word: None,
            debug_sampled: false,
            debug_written: false,
            debug_pulsed: false,
            debug_answer: None,
            debug_reset: false,
            debug_cycles: 0,
            clock_held: false,
            debug_out_word: 0xffff,
            step_limit: u64::MAX,
            rdcyc: false,
            wrcyc: false,
            wmapd: false,
            lvmo: lvmo_at_power_on,
            stalled_ns: 0,
            bus_cycles: 0,
            spushd: false,
            destspcd: false,
            reta: 0,
            imodd: false,
            opc: [0; 8],
            stat: 0,
            speed: Speed::ExtraSlow,
            speed_a: Speed::ExtraSlow,
            timing: TimingModel::Cadr,
            ns: 0,
            busint_bus: 0,
            loadmd_at: u64::MAX,
            executed: None,
            fetch_started: None,
            memstart_fetch: false,
        }
    }

    /// `-RESET`: what the 74S37 at OLORD2 1A06 clears, of what this engine
    /// keeps.  The console's registers at OLORD1 1A04, 1A08 and 1A09; the
    /// 74S175s at CONTRL 3D26, LCC 3E12, PDLCTL 4C11 and VCTL1 1E20 and
    /// 1C23; the 25LS2519 at FLAG 3E08 and the 74S174 at ACTL 3B26; and
    /// `MBUSY`, through `-MFINISH` at VCTL1 1D28.  Not `RUN`, `SRUN`, the
    /// speed synchronizer or `PROMDISABLED`, which clear on `-CLOCK RESET
    /// A`, the power-on reset.  A bus cycle in flight runs to completion:
    /// the interface has a reset of its own and this is not it.
    pub fn reset(&mut self) {
        self.m.reset_console_registers();
        // CONTRL 3D26
        self.inop = false;
        self.spushd = false;
        self.iwrited = false;
        // LCC 3E12
        self.newlc = false;
        self.sintr = false;
        self.next_instrd = false;
        // PDLCTL 4C11
        self.pwidx = false;
        self.pdlwrited = false;
        self.destspcd = false;
        self.imodd = false;
        // VCTL1 1E20 and 1C23, and `MBUSY` through 1D28
        self.memstart = false;
        self.mbusy_sync = false;
        self.rdcyc = false;
        self.wrcyc = false;
        self.mbusy = false;
        // FLAG 3E08
        self.lc_byte_mode = false;
        self.int_enable = false;
        self.sequence_break = false;
        if self.prog_unibus_reset {
            self.bus.unibus_reset(self.ns, false);
        }
        self.prog_unibus_reset = false;
        // ACTL 3B26
        self.wadr = 0;
        self.destd = false;
        self.destmd = false;
    }

    /// The time this engine has spent halted by the console --- `RUN` down,
    /// or a single step done --- with the master clock running and the cpu
    /// clock held off at CLOCK2 1D10.  Neither a microcycle nor a stall.
    pub fn halted_ns(&self) -> u64 {
        self.halted_ns
    }

    /// `ERR`, the 74S133 at OLORD2 1A02: any of the ten parity-error flags,
    /// or `-HALTED`.  None of the parity checks is modeled here --- every
    /// memory this engine holds is a value with no parity to get wrong ---
    /// so `ERR` is `HALTED` alone: misc function 1, `HALT-CONS`, which
    /// microcode 323 writes at `ZERO`, `ILLOP` and `%HALT`.  `-HALT`
    /// crosses between the two processor boards on a connector pin, under
    /// the processor's name `-FUNCT1` and the ICMEM board's `-HALT`, which
    /// `Netlist::EXPLICIT_ALIASES` joins, so `chip` halts on it too.
    ///
    /// **What the ten would cost, measured.**  The flags are `-APE`,
    /// `-MPE`, `-PDLPE`, `-DPE`, `-IPE`, `-SPE`, `-HIGHERR`, `-MEMPE`,
    /// `-V0PE` and `-V1PE`, each the `CLK5A` register of a checker's
    /// verdict in the 74S374s at OLORD2 1A03 and 1A05, and CC reads all
    /// ten by name in `cc/ccwhy.lisp` to say why a machine stopped.
    /// Carrying one verdict bit through [`Read`] and registering it at the
    /// edge, with no checking behind it at all, costs **1.9%** of a band
    /// run; checking two of the ten --- A memory and M memory --- costs
    /// **8.5%** with a parity bit stored beside each word and **9.8%** with
    /// a bitmap of the words never written, on 200,000,000 microcycles
    /// from the same boot, best of three each way.  The two shapes cost the
    /// same because what this path is sensitive to is state carried, not
    /// arithmetic done.  Eight flags would remain.
    ///
    /// **And the parity bit is not always the board's to generate.**  A
    /// memory's is: its RAM at MCTL 3B06 takes `LPARITY`, off the 93S48
    /// generator at 4C09, so no program can choose it.  The dispatch
    /// memory's travels as data --- PRAID's `P-D-MEM-D` in
    /// `ucadr/praid.lisp` works out odd parity itself and writes it
    /// through A memory location 0 as `CONS-DISP-PARITY-BIT` --- so that
    /// one a program can get wrong on purpose, which is what a test of
    /// `-DPE` would need.  `ucadr/mmtest.lisp`'s own "now turn on parity
    /// checking" is commented out in System 100.
    ///
    /// Modeling any of it also means every way a word reaches a memory
    /// without the machine writing it --- the boot PROM's programming
    /// image, a band loaded from a pack, a checkpoint resumed --- carrying
    /// parity too: a first attempt that missed the control store halted
    /// the boot at 1,413,120 microcycles.
    fn err(&self) -> bool {
        self.halted
    }

    /// The datapath of the microcycle just executed, under the names the
    /// drawings give the nets, for comparing against the `chip` engine.
    ///
    /// `PC` and `IR` are first, and callers index them; anything new goes
    /// on the end.
    ///
    /// Recorded in the read phase, where the sources drive and the ALU result
    /// is up but nothing has been written back. `chip` has no notion of an
    /// instruction, only of nets, so this is the only vocabulary the two
    /// engines share.
    pub fn signals(&self) -> Vec<(&'static str, u64)> {
        const NAMES: [&str; 12] =
            ["PC", "IR", "Q", "A", "M", "ALU", "R", "OB", "DC", "OPC", "ST", "LC"];
        NAMES.iter().copied().zip(self.trace).collect()
    }

    pub fn ir(&self) -> u64 {
        self.ir
    }

    /// Simulated nanoseconds since reset.
    ///
    /// Accumulated from the delay-line taps a microcycle at a time, so it
    /// says what the hardware would have taken and nothing about how long
    /// this engine took to work it out.
    /// Runs on `model`'s time from here: `muir` sets it before the machine
    /// boots.
    pub fn set_timing_model(&mut self, model: TimingModel) {
        assert_eq!(self.ns, 0, "a timing model is chosen before the machine runs");
        self.timing = model;
        // The bus interface keeps the model's time in its boards' twins,
        // and is built for it.
        let cache = self.bus.cache().map(|c| c.config);
        let memory = self.bus.memory_timing();
        let mut n = Busint::with_timing_model(self.m.memory_boards(), model);
        n.set_cache(cache);
        n.set_memory_timing(memory);
        *self.bus = n;
        self.m.disk.set_timing_model(model);
    }

    /// A cache in front of the memory boards ([`crate::cache`]), or none:
    /// a measurement's, not the machine's. Chosen before the machine runs,
    /// like the timing model.
    pub fn set_cache(&mut self, config: Option<crate::cache::CacheConfig>) {
        assert_eq!(self.ns, 0, "a cache is fitted before the machine runs");
        self.bus.set_cache(config);
    }

    /// The memory boards timed by other figures
    /// ([`crate::cache::MemoryTiming`]), or not: chosen before the machine
    /// runs.
    pub fn set_memory_timing(&mut self, timing: Option<crate::cache::MemoryTiming>) {
        assert_eq!(self.ns, 0, "a memory is fitted before the machine runs");
        self.bus.set_memory_timing(timing);
    }

    /// When the cycle running was answered, if it has been: the slave took
    /// or gave the word then.
    pub fn bus_answered_at(&self) -> Option<u64> {
        self.bus.answered_at()
    }

    /// When the cycle running is or was acknowledged, `-MEMACK`, if it has
    /// been granted: the bus interface's.
    pub fn bus_ack_at(&self) -> Option<u64> {
        self.bus.ack_at()
    }

    /// Whether the processor holds a granted cycle, `-MEMGRANT` low: from
    /// the grant until the processor lets the cycle go, acknowledged or
    /// not.
    pub fn bus_granted(&self) -> bool {
        self.bus.granted()
    }

    /// The memory cache, if one is fitted ([`Rtl::set_cache`]).
    pub fn cache(&self) -> Option<&crate::cache::Cache> {
        self.bus.cache()
    }

    /// The memory boards' own timing, if they have one
    /// ([`Rtl::set_memory_timing`]).
    pub fn memory_timing(&self) -> Option<crate::cache::MemoryTiming> {
        self.bus.memory_timing()
    }

    /// Whose time this engine keeps.
    pub fn timing_model(&self) -> TimingModel {
        self.timing
    }

    pub fn ns(&self) -> u64 {
        self.ns
    }

    /// The statistics counter, `ST<31:0>`.
    pub fn stat(&self) -> u32 {
        self.stat
    }

    /// `LPC`, the 25S07s at LPC 4F06-4F08: the PC of the instruction before
    /// the one in `IR`, unless the console's `LPC.HOLD` has frozen it.
    pub fn lpc(&self) -> u16 {
        self.lpc
    }

    /// The PCs the OPC shift register holds, newest first. `OPC<13:0>` ---
    /// what a `SRCOPC` reads --- is the last of them.
    pub fn opc_history(&self) -> [u16; 8] {
        self.opc
    }

    /// What the console reads off the SPY bus, under the names the drawings
    /// give the nets.
    ///
    /// The six on SPY2 3F15 are the write-pipeline enables --- registered
    /// because a scratchpad write lands a microcycle late, so they exist here
    /// for the datapath's sake rather than the console's. They are also,
    /// near enough, the flags this engine never raises.
    ///
    /// This is the *state* the interface reads, not the interface: the
    /// console's own bus, its writes and its halt and step control are
    /// `crate::spy`'s and the bus interface's job.
    pub fn spy(&self) -> Vec<(&'static str, u64)> {
        const NAMES: [&str; 12] = [
            "WMAPD",
            "DESTSPCD",
            "IWRITED",
            "IMODD",
            "PDLWRITED",
            "SPUSHD",
            "NOP",
            "-VMAOK",
            "JCOND",
            "PCS1",
            "PCS0",
            "SRUN",
        ];
        NAMES.iter().copied().zip(self.flags).collect()
    }

    /// The control store address executed in the last microcycle, or `None`
    /// if it was nopped. A nopped cycle runs on the board and retires
    /// nothing, so it is not an executed instruction.
    pub fn executed(&self) -> Option<u16> {
        self.executed
    }

    /// `Some(wrong_word)` when the last microcycle started a macroinstruction
    /// fetch (`NEEDFETCH` and `LCINC`, page LCC): `true` when `LC` had been
    /// written, `false` when the word is the next in sequence.
    pub fn fetch_started(&self) -> Option<bool> {
        self.fetch_started
    }
}

impl Rtl {
    /// Where the map is being looked up, both levels.
    ///
    /// `MAPI` is `VMA` while `MEMSTART` is up and `MD` otherwise, off the
    /// 74S258s at VMAS 1C20 and its fellows, whose select is `-MEMSTART`.
    /// The two levels are asynchronous rams with the first's output in the
    /// second's address, so this is a ripple and not a cycle of its own.
    fn map_address(&self) -> (u16, u16) {
        let mapi =
            if self.memstart { (self.m.vma >> 8) & 0xffff } else { (self.m.md >> 8) & 0xffff };
        let adr0 = ((mapi >> 5) & 0o3777) as u16;
        let adr1 =
            self.m.geometry.l2_index(self.m.l1_map[adr0 as usize], (mapi << 8) as u32) as u16;
        (adr0, adr1)
    }

    /// The word address a fetch takes from `LC`: `LC<25:2>`.
    fn fetch_mask(&self) -> u64 {
        u64::from(crate::machine::LC_COUNTER) >> 2
    }

    /// The whole combinational network, once, with `CLK` high.
    #[allow(clippy::nonminimal_bool)]
    fn read_phase(&self) -> Read {
        let ir = self.ir;

        // page TRAP, CONTRL.  `-NOPA` is the open-collector 74S08 at CONTRL
        // 3E14, `AND(-NOP11, -INOP)`, `-INOP` being the 74S175's own `-Q` at
        // 3D26: the console's `NOP11` nops every microcycle.  `NOP` adds the
        // trap, `NAND(-TRAP, -NOPA)` at 3E23 --- and `ILONG`, `STATBIT` and
        // `USE.MD` are gated by `NOPA`, not by `NOP`, so the trap cycle can
        // be long and counted.
        let trap = self.boot_trap;
        let nopa = self.inop | self.m.clock_control.nop11;
        let nop = trap | nopa;

        // page SOURCE
        let (irbyte, irdisp, irjump, iralu) = if nop {
            (false, false, false, false)
        } else {
            match field(ir, 44, 43) {
                0 => (false, false, false, true),
                1 => (false, false, true, false),
                2 => (false, true, false, false),
                _ => (true, false, false, false),
            }
        };
        let funct: u8 = if nop { 0 } else { 1 << field(ir, 11, 10) };

        let src = field(ir, 28, 26);
        let group_a = bit(ir, 31) && !bit(ir, 29);
        let group_b = bit(ir, 31) && bit(ir, 29);
        let srcdc = group_a && src == 0;
        let srcspc = group_a && src == 1;
        let srcpdlptr = group_a && src == 2;
        let srcpdlidx = group_a && src == 3;
        let srcpdlpop = group_a && src == 4;
        let srcpdltop = group_a && src == 5;
        let srcopc = group_a && src == 6;
        let srcq = group_a && src == 7;
        let srcvma = group_b && src == 0;
        let srcmap = group_b && src == 1;
        let srcmd = group_b && src == 2;
        let srclc = group_b && src == 3;
        let srcspcpop = group_b && src == 4;

        let dest = iralu | irbyte;
        let destm = dest && !bit(ir, 25);
        let destmem = destm && bit(ir, 23);
        let destvma = destmem && !bit(ir, 22);
        let destmdr = destmem && bit(ir, 22);

        let d19 = field(ir, 21, 19);
        let low_group = destm && !bit(ir, 23) && !bit(ir, 22);
        let destlc = low_group && d19 == 1;
        let destintctl = low_group && d19 == 2;
        // The low group decodes no 3 to 7: only M is written.
        let mid_group = destm && !bit(ir, 23) && bit(ir, 22);
        let destpdltop = mid_group && d19 == 0;
        let destpdl_p = mid_group && d19 == 1;
        let destpdl_x = mid_group && d19 == 2;
        let destpdlx = mid_group && d19 == 3;
        let destpdlp = mid_group && d19 == 4;
        let destspc = mid_group && d19 == 5;
        let destimod0 = mid_group && d19 == 6;
        let destimod1 = mid_group && d19 == 7;

        let d2019 = field(ir, 20, 19);
        let (wmap, memwr, memrd) =
            if !destmem { (false, false, false) } else { (d2019 == 3, d2019 == 2, d2019 == 1) };

        // page ACTL: the write address this instruction will hand on.  The
        // 25S09s at 3B28 and 3B29 select on DESTM, which is what shortens an
        // M destination to five bits.
        let wadr_in = if destm { field(ir, 18, 14) as u16 } else { field(ir, 23, 14) as u16 };

        // page ACTL: the A memory, addressed by IR<41:32> with CLK high, and
        // passed around when the write still pending in L is to this very
        // address.  `-AMEMENB` is `NAND(-APASS, TSE3A)` at 3B16 and
        // `APASSENB` is `AND(APASS1, APASS2, TSE4A)` at 4B11.
        let aadr = field(ir, 41, 32) as u16;
        let apass = self.destd && self.wadr == aadr;
        let a = if apass { self.l } else { self.m.amem[aadr as usize] };

        // page MCTL: the same again, five bits wide.  `MPASS` is the 93S46 at
        // 4B18, which matches `DESTMD` as its sixth bit.
        let madr = field(ir, 30, 26) as u8;
        let mpass = self.destmd && (self.wadr & 0o37) as u8 == madr;
        let mmem = if mpass { self.l } else { self.m.mmem[(madr & 0o37) as usize] };

        // page ALATCH / MLATCH: which driver has the M bus.
        let mpassm = !bit(ir, 31);
        let spcenb = srcspc | srcspcpop;
        let pdlenb = srcpdlpop | srcpdltop;
        let mfenb = !mpassm && !(spcenb | pdlenb);

        // page LCC --- needed by the MF mux
        let lc0b = bit(self.lc, 0) && self.lc_byte_mode;
        let have_wrong_word = self.newlc | destlc;
        let last_byte_in_word = !bit(self.lc, 1) && !lc0b;
        let needfetch = have_wrong_word | last_byte_in_word;
        let lcinc = self.next_instrd || (irdisp && bit(ir, 24));
        let ifetch = needfetch && lcinc;
        let newlc_in = have_wrong_word && !lcinc;

        // pages VMAS, VMEM0 and VMEM1.
        let (mut vmap, mut vmo) = {
            let (adr0, adr1) = self.map_address();
            (self.m.l1_map[adr0 as usize], self.m.l2_map[adr1 as usize])
        };
        // A map store's write lands in this microcycle's write pulse: the
        // word the edge registers is the one the RAM shows after it, as
        // `chip` has it.
        if self.wmapd {
            if bit(self.m.vma, 26) {
                vmap = self.m.geometry.l1_from_vma(self.m.vma as u32);
            }
            if bit(self.m.vma, 25) {
                vmo = self.m.vma as u32 & 0o77777777;
            }
        }

        // page VMEMDR 1D14: a 74S373 transparent while `MEMSTART`, so on such
        // a cycle it is already following the word the map is putting out.
        let lvmo = if self.memstart { vmo } else { self.lvmo };

        // pages VCTL1 and VCTL2, as the nets they name.  `-PFR` is `-LVMO23`
        // inverted with no `WRCYC` in it, and `-PFW` is a NAND, so these read
        // the opposite way round to the names: `-PFR` is high when the read
        // is *permitted*.
        //
        //     VCTL2 1D26  74S04A  -PFR = NOT(-LVMO23)
        //     VCTL1 1D17  74S00   -PFW = NAND(-LVMO22, WRCYC)
        //     VCTL1 1D17  74S00O  -VMAOK = NAND(-PFR, -PFW)
        let pfr = bit(lvmo as u64, 23);
        let pfw = !(!bit(lvmo as u64, 22) && self.wrcyc);
        let vmaok = pfr && pfw;

        // mux MF
        let mf: Word = if srclc {
            ((needfetch as u32) << 31
                | (self.lc_byte_mode as u32) << 29
                | (self.prog_unibus_reset as u32) << 28
                | (self.int_enable as u32) << 27
                | (self.sequence_break as u32) << 26
                | (self.lc as u32 & 0x03ff_fffe)
                | lc0b as u32)
                .into()
        } else if srcopc {
            // Page OPCD drives `MF<13:0>` from `OPC<13:0>`, which is eight
            // microcycles back, not one.
            self.opc[7].into()
        } else if srcdc {
            self.m.dispatch_constant.into()
        } else if srcpdlptr {
            self.m.pdl_pointer.into()
        } else if srcpdlidx {
            self.m.pdl_index.into()
        } else if srcq {
            self.m.q
        } else if srcmd {
            self.m.md
        } else if srcvma {
            self.m.vma
        } else if srcmap {
            // Bit 29 is **zero**, not one. VMEMDR 1A01 puts `-PFW`, `-PFR`,
            // `HI12` and `-VMAP<4:0>` onto `MF<31:24>` through a 74S240,
            // which *inverts*, and a pull-up on the input of an inverting
            // buffer is a hard zero on its output.  A one there would be
            // right for a '241, which is what this is easy to mistake it for.
            ((!pfw as u32) << 31
                | (!pfr as u32) << 30
                | (vmap & self.m.geometry.l1_mask()) << 24
                | (vmo & 0o77777777))
                .into()
        } else {
            // Functional sources 0o15, 0o16 and 0o17: the 74S138 that decodes
            // `IR<28:26>` under `IR<31>` and `IR<29>` has those three outputs
            // unconnected, so nothing on page MF drives the bus, and an
            // undriven TTL bus reads high. No instruction means to read
            // them; a control-store word being written back does, for the
            // nopped microcycle its `IR` holds it, and `chip` shows the ones.
            self.m.geometry.word_mask()
        };

        // page PDLCTL: `PDLP` is `(CLK AND IR30) OR (-CLK AND -PWIDX)` off
        // the 74S51 at 4D07, so with CLK high the PDL is addressed by IR<30>.
        let pdla = if bit(ir, 30) { self.m.pdl_pointer } else { self.m.pdl_index };
        let pdl = self.m.pdl[pdla as usize];

        // page SPC: the 82S21s at 4E21-4E23 read asynchronously at the
        // pointer, and their output `SPCO<18:0>` is what the 74S373s at
        // SPCLCH 4A07/4A09/4A10 put on `M<18:0>` under `-SPCDRIVE`, with
        // `SPCPTR<4:0>` on `M<28:24>` through the 74S241 at 4B10.  The
        // pass-around is not on that road: while a push is pending
        // `SPCWPASS` at CONTRL 3D21 turns the 74S241s at 4E17/4E18 on and
        // `-SPCPASS` turns the latch at 4F18-4F20 off, so the word about to
        // be written stands on the `SPC` bus --- which goes to the PC's
        // next-address path and the parity check, and not to `M`.  So the
        // instruction after a push reads the RAM's stale word at the new
        // pointer as its M source, and `chip` shows it.
        // page SPCW: the 74S157s at 4E12-4E14 select on `DESTSPCD`, the
        // *registered* `DESTSPC`, over `L` and the *registered* `RETA`. So
        // the word is known before the ALU is, and the pass-around closes no
        // loop.
        let spcw =
            if self.destspcd { self.l as u32 & crate::machine::SPC_WORD } else { self.reta as u32 };
        let spco = self.m.spc[self.m.spcptr as usize];
        let spc = if self.spushd { spcw } else { spco };

        let m = if mpassm {
            mmem
        } else if pdlenb {
            pdl
        } else if spcenb {
            ((self.m.spcptr as u32) << 24 | (spco & 0o1777777)).into()
        } else if mfenb {
            mf
        } else {
            0
        };

        // page SMCTL --- shift and mask amounts, with the LC byte-mode tweak
        let lc_modifies_mrot = bit(ir, 10) && bit(ir, 11);
        let inst_in_left_half = !((bit(self.lc, 1) ^ lc0b) || !lc_modifies_mrot);
        let sh4 = !(inst_in_left_half ^ !bit(ir, 4));
        let inst_in_2nd_or_4th_quarter =
            !(bit(self.lc, 0) || !lc_modifies_mrot) && self.lc_byte_mode;
        let sh3 = !(!bit(ir, 3) ^ inst_in_2nd_or_4th_quarter);

        let mr = !irbyte || bit(ir, 13);
        let sr = !irbyte || bit(ir, 12);
        let bits =
            |b4: bool, b3: bool| -> u32 { (b4 as u32) << 4 | (b3 as u32) << 3 | field(ir, 2, 0) };
        let mskr = if mr { bits(sh4, sh3) } else { 0 };
        let shift = if sr { bits(sh4, sh3) } else { 0 };
        let mskl = mskr.wrapping_add(field(ir, 9, 5)) & 0o37;

        // page SHIFT0-1: a 32-bit rotate left by `shift`, of `M<31:0>`;
        // page MSKG4.
        let r = Word::from((m as u32).rotate_left(shift));
        let msk = Word::from((!0u32 >> (31 - mskl)) & (!0u32 << mskr));

        // page ALU0-1 / ALUC4
        let ctl = ttl::alu_control(ir, bit(self.m.q, 0), bit(a, 31), iralu, irjump);
        let alu_out = ttl::alu(m as u32, a as u32, ctl.aluf, ctl.alumode, ctl.cin);
        let alu = alu_out.f;
        let aeqm = alu_out.aeqm;

        // page MO
        let mo = (msk & r) | (a & !msk);
        let osel = (bit(ir, 13) && iralu) as u32 * 2 + (bit(ir, 12) && iralu) as u32;
        let ob = match osel {
            0 => mo,
            1 => alu & LOW,
            2 => (alu >> 1) & LOW,
            _ => Word::from(((alu << 1) as u32 & !1) | (self.m.q as u32 >> 31)),
        };

        // page FLAG.
        let aluneg = !aeqm && bit(alu, 32);
        let sint = self.sintr && self.int_enable;
        let pgf_or_int = !vmaok || sint;
        let pgf_or_int_or_sb = pgf_or_int || self.sequence_break;
        let conds = if bit(ir, 5) { field(ir, 2, 0) } else { 0 };
        let jcond = match conds {
            0 => bit(r, 0),
            1 => aluneg,
            2 => bit(alu, 32),
            3 => aeqm,
            4 => !vmaok,
            5 => pgf_or_int,
            6 => pgf_or_int_or_sb,
            _ => true,
        };

        // page DRAM / DSPCTL: the dispatch memory, asynchronous like the rest.
        //
        // Bit 0 of the address is the 74S64s at 2F24, 2F05 and 2F23:
        //
        //     -DADR0 = NAND-OR(VMO18 AND IR8, VMO19 AND IR9,
        //                      -DMAPBENB AND DMASK0 AND R0, IR12)
        //     -DMAPBENB = NOR(IR8, IR9)                    at 3F14
        //
        // so a dispatch that takes a map bit takes it *instead of* `R0`, not
        // as well. ORing the two is the easy misreading of that NAND-OR. The
        // microcode's
        // `TRANSPORT` is `Q-DATA-TYPE-PLUS-ONE-BIT` for exactly this reason:
        // the field is one bit wider than the type so that the type lands at
        // `DADR<5:1>` and the map bit has bit 0 to itself.
        let len = field(ir, 7, 5);
        let dmask = (1u32 << len) - 1;
        let map = bit(ir, 8) || bit(ir, 9);
        let daddr0 = (bit(ir, 8) && bit(vmo as u64, 18))
            || (bit(ir, 9) && bit(vmo as u64, 19))
            || (!map && (dmask & 1 != 0) && bit(r, 0))
            || bit(ir, 12);
        let dhigh = field(ir, 22, 13);
        let dadr = (((dhigh << 1) | daddr0 as u32) | (dmask & r as u32 & 0o176)) as u16;
        let dispwr = irdisp && (funct & 4) != 0;
        // A dispatch memory write with `POPJ` reads the word it writes: the
        // one written, as `chip` has it.
        let dram_q = if dispwr { a as u32 & 0o377777 } else { self.m.dmem[dadr as usize] };

        // page CONTRL --- sequencing
        let dr = bit(dram_q as u64, 16);
        let dp = bit(dram_q as u64, 15);
        let dn = bit(dram_q as u64, 14);
        let dpc = (dram_q & 0o37777) as u16;
        let dfall = dr && dp;
        let dispenb = irdisp && (funct & 4) == 0;
        let ignpopj = irdisp && !dr;
        let jfalse = irjump && bit(ir, 6);
        let jcalf = jfalse && bit(ir, 8);
        let jret = irjump && !bit(ir, 8) && bit(ir, 9);
        let jretf = jret && bit(ir, 6);
        let iwrite = irjump && bit(ir, 8) && bit(ir, 9);
        let ipopj = bit(ir, 42) && !nop;
        let popj = ipopj | self.iwrited;
        let srcspcpopreal = srcspcpop && !nop;

        let spop = ((srcspcpopreal || popj) && !ignpopj)
            || (dispenb && dr && !dp)
            || (jret && !bit(ir, 6) && jcond)
            || (jretf && !jcond);
        let spush = destspc
            || (jcalf && !jcond)
            || (dispenb && dp && !dr)
            || (irjump && !bit(ir, 6) && bit(ir, 8) && jcond);

        let next_instr = spop && (!srcspcpopreal && bit(spc as u64, 14));
        let spcnt = spush | spop;

        let n = trap
            || self.iwrited
            || (dispenb && dn)
            || (jfalse && !jcond && bit(ir, 7))
            || (irjump && !bit(ir, 6) && jcond && bit(ir, 7));

        let pcs1 = !((popj && !ignpopj)
            || (jfalse && !jcond)
            || (irjump && !bit(ir, 6) && jcond)
            || (dispenb && dr && !dp));
        let pcs0 =
            !(popj || (dispenb && !dfall) || (jretf && !jcond) || (jret && !bit(ir, 6) && jcond));

        let ipc = self.pc.wrapping_add(1) & 0o37777;
        let wpc = if irdisp && bit(ir, 25) { self.lpc } else { self.pc };
        let reta_in = if n { wpc } else { ipc };

        let spcmung = bit(spc as u64, 14) && !needfetch;
        let spc1a = spcmung || bit(spc as u64, 1);
        let spc_target = ((spc as u16) & 0o37774) | (spc1a as u16) << 1 | (spc as u16 & 1);
        let npc = if trap {
            self.m.reset_pc()
        } else {
            match (pcs1 as u8) * 2 + pcs0 as u8 {
                0 => spc_target,
                1 => field(ir, 25, 12) as u16,
                2 => dpc,
                _ => ipc,
            }
        };

        // page PDLCTL
        let pdlwrite = destpdltop | destpdl_x | destpdl_p;
        let pdlcnt = (!nop && srcpdlpop) || destpdl_p;

        // page VCTRL1 / VCTRL2
        let memop = memrd | memwr | ifetch;
        let vmaenb = destvma | ifetch;
        let vmas = if !ifetch { ob } else { (self.lc >> 2) & self.fetch_mask() };

        // page ICTL: the boot PROM overlays the bottom of the control store,
        // and `-PROMENABLE` is off while `IWRITEDA` is up.  In that cycle
        // the write pulse --- `-IWEA = NAND(WP5A, IWRITEDA)` at 1B13 ---
        // lands in the write phase, before the edge that loads `IR`, so
        // what the RAM puts on the I bus at that edge is the word just
        // written and not the one it held.  The cycle is nopped, so nothing
        // runs it; `chip` and CC both see it.
        //
        // With `IDEBUG` up the six 74S374s on page DEBUG drive the I bus
        // instead: `RAMDISABLE` is `IDEBUG OR ...` at ICTL 1A15 and `-IDEBUG`
        // is an input of `-PROMENABLE` at PCTL 1C19.
        let idebug = self.m.clock_control.idebug;
        // `BOTTOM.1K` at PCTL 1D18 is the top four PC bits all clear:
        // `crate::machine::PROM_WORDS` says how the 1K is decoded.
        let in_prom = (self.pc as usize) < crate::machine::PROM_WORDS && !self.promdisabled;
        let promenable = in_prom && !self.iwrited && !idebug;
        let i = if idebug {
            self.m.debug_ir
        } else if promenable {
            self.m.prom[self.pc as usize].raw()
        } else if self.iwrited {
            self.iwr & 0xffff_ffff_ffff
        } else {
            self.m.imem[self.pc as usize & (IMEM_WORDS - 1)].raw()
        };

        // page OLORD1: the 9S42 at 1A15, `MACHRUN = (SSTEP AND -SSDONE) OR
        // (SRUN AND -ERRHALT AND -WAIT AND -STATHALT)`.  A single step runs
        // its one microcycle whatever the bus is doing; a running machine is
        // stopped by an error under `ERRSTOP`, by the statistics counter
        // under `STATHENB`, or by `WAIT` --- which is [`Rtl::stall`]'s to
        // apply, since the generator runs on through it.
        let stepping = self.sstep && !self.ssdone;
        let errhalt = self.m.mode.errstop && self.err();
        let stathalt = self.m.mode.stathenb && self.statstop;
        let machrun = stepping || (self.srun && !errhalt && !stathalt);

        Read {
            nop,
            irdisp,
            halt: funct & 2 != 0,
            i,
            a,
            m,
            alu,
            r,
            ob,
            n,
            npc,
            dest,
            destm,
            destmdr,
            destlc,
            destintctl,
            destimod0,
            destimod1,
            destpdlp,
            destpdlx,
            destpdl_x,
            destspc,
            wadr_in,
            srcpdlpop,
            wmap,
            memwr,
            memop,
            // `USE.MD` is `NOR(-SRCMD, NOPA)` at VCTL2 3F18.
            use_md: srcmd && !nopa,
            destmem,
            needfetch,
            have_wrong_word,
            lcinc,
            spush,
            spcnt,
            spcw,
            reta_in,
            pdlwrite,
            pdlcnt,
            dispwr,
            dadr,
            vmas,
            vmaenb,
            vmo,
            ilong: bit(ir, 45) && !nopa,
            statbit: bit(ir, 46) && !nopa,
            imod: destimod0 || destimod1 || self.iwrited || idebug,
            jcond,
            pcs1,
            pcs0,
            next_instr,
            newlc_in,
            qs1: bit(ir, 1) && iralu,
            qs0: bit(ir, 0) && iralu,
            iwrite,
            vmaok,
            machrun,
            stepping,
        }
    }

    /// `CLK` low: the address multiplexers have switched to `WADR` and the
    /// write pulses fire.
    ///
    /// Everything written here belongs to the *previous* instruction, and
    /// every enable is a registered one.
    fn write_phase(&mut self, r: &Read) {
        // The copies of `A-LOCALP` and `M-AP` take the word where the
        // MACRO-DISPATCH register names its address, with the write pulse
        // (`machine::macro_dispatch`), as on every engine: the CADR has no
        // register, and its copies follow A and M location 0, which its
        // checkpoint keeps.
        if self.destd {
            self.m.amem[self.wadr as usize] = self.l;
            self.m.macro_dispatch.a_written(self.wadr as usize, self.l);
        }
        if self.destmd {
            self.m.mmem[(self.wadr & 0o37) as usize] = self.l;
            self.m.macro_dispatch.m_written((self.wadr & 0o37) as usize, self.l);
        }
        if self.pdlwrited {
            // Addressed by `PWIDX` rather than by `IR<30>`: with CLK low the
            // 74S51 at PDLCTL 4D07 gives `PDLP = -PWIDX`, so the write goes
            // where the instruction that is being written back said.
            let adr = if self.pwidx { self.m.pdl_index } else { self.m.pdl_pointer };
            self.m.pdl[adr as usize] = self.l;
        }
        // The one write that is not a microcycle late: `-DWEA` is
        // `NAND(WP2, DISPWR)` at DRAM 2F03, gated by the live signal and not
        // by a registered one, so the address and the data are this
        // instruction's.  Which is why there is no dispatch pass-around.
        if r.dispwr {
            self.m.dmem[r.dadr as usize] = r.a as u32 & 0o377777;
        }
        if self.wmapd {
            // `MAPWR0D` is `WMAPD AND VMA26` and `MAPWR1D` is `WMAPD AND
            // VMA25` at VCTL2 1C15, and both pulses are `-WP1`, so the two
            // levels are written in the same write phase.  Address and data
            // are the live ones: nothing latches them.
            //
            // Written together, level 1's 93425As hold their outputs in high
            // impedance for the pulse and `-VMAP` floats high: level 2 is
            // then addressed with its top bits zero (`Machine::write_map`
            // has the whole account); its low five, `MAPI<12:8>`, come
            // through the same 74S258s either way.
            let (adr0, adr1) = self.map_address();
            let both = bit(self.m.vma, 26) && bit(self.m.vma, 25);
            let adr1 = if both { adr1 & 0o37 } else { adr1 };
            if bit(self.m.vma, 26) {
                self.m.l1_map[adr0 as usize] = self.m.geometry.l1_from_vma(self.m.vma as u32);
            }
            if bit(self.m.vma, 25) {
                self.m.l2_map[adr1 as usize] = self.m.vma as u32 & 0o77777777;
            }
        }
        // `-IWEA` is `NAND(WP5A, IWRITEDA)` at ICTL 1B13, so the control
        // store is written on the pulse after `WRITE-I-MEM`, at the address
        // the PC has moved to.  One register stage, not two.
        if self.iwrited {
            self.m.write_imem(self.pc, crate::isa::Insn::new(self.iwr));
        }

        // The stack is written from SPCW, at the pointer the edge has
        // already moved to: the 82S21s at SPC 4E22/4E23 are addressed by
        // `SPCPTR0..4` with no offset, and `-SWPA` is `NAND(WP4C, SPUSHD)`
        // at 4E30.
        if self.spushd {
            self.m.spc[self.m.spcptr as usize] = r.spcw & crate::machine::SPC_WORD;
        }
    }

    /// Carries the bus cycle forward to `now`, and finishes it if `-MEMACK`
    /// has come back by then.
    ///
    /// The word itself comes from [`Machine`], which holds the memories and
    /// the devices; [`Busint`] says only when. A cycle nothing answers ends
    /// too --- after the interface's timeout, [`busint::TIMEOUT_NS`] --- which is how the boot
    /// PROM survives running one word past the end of page 0.
    fn bus_cycle(&mut self, now: u64) {
        self.land_write(now);
        self.sample_spy(now);
        self.debug_cycle(now);
        if self.bus_acked {
            return;
        }
        let Some(ack) = self.bus.poll(now, self.bus_responder) else { return };
        self.bus_acked = true;
        // The word was made when the device answered, before the
        // acknowledgement: the clocks on the I/O board are read then.
        self.m.ns = ack.answered_at;
        if !self.wrcyc {
            // `-LOADMD` "equals MEMACK and RDCYC ... Loads MD from MEM,
            // asynchronous with clock ... the high-going edge loads MD.  It
            // takes care of deskewing the data."  A debug-block read's word
            // is the other machine's, off the cable.
            if self.bus_spy.is_none() {
                self.busint_bus = match self.bus_responder {
                    // Given up on, the cable is whatever nothing drives.
                    Responder::Debug(_) if ack.timed_out => 0xffff,
                    Responder::Debug(_) => self.debug_out_word.into(),
                    _ => self.m.bus_read(self.bus_addr),
                };
            }
            self.loadmd_at = ack.loadmd_at;
        }
        // A debug cycle the other machine never answered: this interface's
        // own `NXM TIMEOUT` sets `UB NXM ERROR` in the 74276 at REQERR 0B02
        // as for any Unibus cycle; [`Machine::device`] flags nothing for
        // the block, whose word is not the machine's to give.
        if ack.timed_out && matches!(self.bus_responder, Responder::Debug(_)) {
            self.m.bus_error |= crate::machine::bus_error::UNIBUS_NXM;
        }
        // "cleared by MEMACK delayed by about 150 ns.  This delay is
        // sufficient time for the data in MD to get through the data paths."
        // A read the memory cache answered ran no bus cycle: its word is at
        // the data paths with the acknowledgement, and nothing is left to
        // release ([`crate::cache`]).
        //
        // `-RDFINISH` is a pulse off every acknowledgement, and the first
        // to reach a `READ IN PROGRESS` still up clears it: a write's
        // acknowledgement, behind a read whose own pulse is already on its
        // way down the delay line, does not put the read's off.
        let (finish, release) = if ack.cached { (0, 0) } else { (RD_FINISH_NS, MFINISHD_NS) };
        let finish_at = ack.at + finish;
        self.rd_finish_at =
            if self.rd_in_progress { self.rd_finish_at.min(finish_at) } else { finish_at };
        // "Clears on MEMACK or RESET" --- through [`MFINISHD_NS`].
        self.mbusy_clear_at = ack.at + release;
        // `-MEMRQ` goes with `MBUSY`, and `-XBUS RQ` with it; a memory
        // board is not idle until then.
        self.bus.released(self.mbusy_clear_at);
    }

    /// The word of a write reaches its slave when [`Busint::answered_at`]
    /// says, which for one of the bus interface's own registers is 250 ns
    /// before the acknowledgement. It matters for one register: the mode
    /// register sets the machine's speed, and the speed synchronizer samples
    /// it [`SPEEDCLK_NS`] into every generator cycle, so this is carried to
    /// there before [`Rtl::speedclk`] as well as to the edge.
    ///
    /// A write of the mode register is two things at two times.  The
    /// register loads at the trailing edge of the write pulse, which is
    /// [`Busint::answered_at`]; but `-PROG.RESET` and `PROG.BOOT` are the
    /// pulse itself gated with two data bits, up from its leading edge,
    /// [`busint::REGISTER_PULSE_NS`] earlier, and a cpu edge inside the pulse
    /// takes the trap.  So the two pulses are handed to [`Machine`] from the
    /// leading edge and kept out of the word that lands at the trailing one.
    fn land_write(&mut self, now: u64) {
        if !self.wrcyc || self.bus_written {
            return;
        }
        let Some(at) = self.bus.answered_at() else { return };
        let mode = self.bus_spy.is_some_and(|e| spy::write_strobe(e) == spy::MODE);
        let pulses = (spy::MODE_RESET | spy::MODE_BOOT) as u32;
        if mode && !self.bus_pulsed && now + busint::REGISTER_PULSE_NS >= at {
            self.bus_pulsed = true;
            self.m.prog_reset |= self.bus_data & spy::MODE_RESET as Word != 0;
            self.m.prog_boot |= self.bus_data & spy::MODE_BOOT as Word != 0;
        }
        if now >= at {
            self.m.ns = at;
            let data = if mode { self.bus_data & !Word::from(pulses) } else { self.bus_data };
            self.m.bus_write(self.bus_addr, data);
            self.bus_written = true;
        }
    }

    /// A read of one of the sixteen diagnostic registers is answered by this
    /// engine, not by [`Machine`]: the bus interface decodes the address,
    /// pulls `-DBREAD` down with `EADR<3:0>` on the cable, and what comes
    /// back on `SPY<15:0>` is the cpu's own `PC`, `IR`, `OB` or flags at
    /// that moment --- `-SPY READ` at DIAG 0A11, [`Busint::answered_at`] for
    /// a register cycle.  Sampled at the first boundary after it, as
    /// [`Rtl::land_write`] lands a write, so the word is the machine's within
    /// a microcycle of when the board's buffers would have carried it.
    fn sample_spy(&mut self, now: u64) {
        if let Some(eadr) = self.bus_spy
            && !self.wrcyc
            && !self.bus_sampled
            && let Some(at) = self.bus.answered_at()
            && now >= at
        {
            self.busint_bus = self.spy_read(eadr).into();
            self.bus_sampled = true;
        }
    }

    /// The debug master's cycle on this machine's Unibus, carried to `now`:
    /// the word of a read sampled when the slave answers, the word of a
    /// write landed as [`Rtl::land_write`] lands the processor's, the
    /// acknowledgement recorded for the debugger, and the modifier
    /// register's reset bit acted on at its edges.
    ///
    /// What a read can bring back is what reaches `UDO`, since the 8304s at
    /// DBGOUT 0B21 and 0B22 put `UDO` on the cable: the cpu's diagnostic
    /// registers through the 8304s at DIAG 0A20 and 0A21, and the
    /// interface's own registers --- the error status through the 74LS244
    /// at REQERR 0C16, the interrupt status and the Unibus map through
    /// theirs.  A slave on the Unibus proper answers on `-UBD`, which the
    /// 8838s on page UBD bring in as `UDI`, and `UDI` goes to `BUS` alone
    /// under `-UB16>BUS`, for the processor's reads; nothing carries it to
    /// `UDO`, so the debugger reads an undriven `DBD` --- though the slave
    /// was read, with whatever that does to it.  The Xbus through the
    /// Unibus map, which is how CC reads the debuggee's memory, goes by
    /// [`crate::machine::Machine::mapped_read`] and
    /// [`crate::machine::Machine::mapped_write`].
    fn debug_cycle(&mut self, now: u64) {
        self.bus.debug_advance(now);
        // `-DEBUGEE RESET`: the interface's `RESET`, which is `-UB INIT` and
        // `-XBUS INIT`, and the cpu's power-on reset over the cables.
        let reset = self.bus.debug_modifier() & busint::debug_modifier::RESET != 0;
        if reset != self.debug_reset {
            self.debug_reset = reset;
            self.bus.unibus_reset(now, reset);
            if reset {
                self.clock_reset();
                self.m.bus_reset();
            }
            // `RESET` clears the clock ring, CLOCK1 and CLOCK2's 74S10s under
            // `-CLOCK RESET B`; it runs again from `-TPR0` when the bit is
            // lifted, and `now` is then the start of a cycle.
            self.clock_held = reset;
        }
        let Some(req) = self.bus.debug_last_request() else { return };
        if req.strobe == busint::DEBUG_CYCLE
            && let Some(at) = self.bus.debug_answered_at()
        {
            let uaddr = self.bus.debug_unibus_address();
            let phys = busint::unibus_physical(uaddr);
            let responder = self.bus.debug_responder();
            // Through the map: the buffers, the mapped Xbus word, or the
            // refusal that sets `UB MAP ERROR` and answers nothing.
            if let Some(access) = busint::map_access(uaddr) {
                if now >= at && !self.debug_written && !self.debug_sampled {
                    self.m.ns = at;
                    if req.write {
                        self.m.mapped_write(access, req.dbd);
                        self.debug_written = true;
                    } else {
                        self.debug_word = self.m.mapped_read(access);
                        self.debug_sampled = true;
                    }
                }
            } else {
                let answers = match responder {
                    Responder::Interface => true,
                    Responder::Unibus(u) => crate::ioboard::answers(u, req.write).is_some(),
                    _ => false,
                };
                let eadr = spy::register(uaddr);
                if req.write && answers {
                    let mode = eadr.is_some_and(|e| spy::write_strobe(e) == spy::MODE);
                    let pulses = spy::MODE_RESET | spy::MODE_BOOT;
                    if mode && !self.debug_pulsed && now + busint::REGISTER_PULSE_NS >= at {
                        self.debug_pulsed = true;
                        self.m.prog_reset |= req.dbd & spy::MODE_RESET != 0;
                        self.m.prog_boot |= req.dbd & spy::MODE_BOOT != 0;
                    }
                    if !self.debug_written && now >= at {
                        self.m.ns = at;
                        let data = if mode { req.dbd & !pulses } else { req.dbd };
                        self.m.bus_write(phys, data.into());
                        self.debug_written = true;
                    }
                } else if !req.write && !self.debug_sampled && now >= at {
                    self.debug_word = match responder {
                        Responder::Interface => Some(match eadr {
                            Some(e) => self.spy_read(e),
                            None => {
                                self.m.ns = at;
                                self.m.bus_read(phys) as u16
                            }
                        }),
                        // A slave on the Unibus proper answers on `-UBD`,
                        // which reaches `BUS` and never `UDO`; but it was
                        // read, and what the I/O board does on being read
                        // --- clear `KBD READY`, latch the microsecond
                        // clock --- it does for this master as for the
                        // processor.
                        Responder::Unibus(_) => {
                            self.m.ns = at;
                            self.m.bus_read(phys);
                            None
                        }
                        _ => None,
                    };
                    self.debug_sampled = true;
                }
            }
        }
        if self.debug_answer.is_none()
            && let Some(ack) = self.bus.debug_ack_at()
            && now >= ack
        {
            self.debug_answer = Some((ack, self.debug_word));
        }
    }

    /// `-CLOCK RESET A` and `-CLOCK RESET B`, the power-on reset, which the
    /// debugger asserts too through `-BUSINT LM RESET`: OLORD2 1B10 inverts
    /// the cable's wire into the 74S02s at 1A11 that make both.  `-CLOCK
    /// RESET B` is an input of `RESET` at OLORD2 1C08 beside `-BOOT` and
    /// `-PROG.RESET`, so everything [`Rtl::reset`] clears; `-CLOCK RESET A`
    /// clears what that leaves --- `RUN` at OLORD1 1A14, so the machine
    /// halts; `SRUN`, `SSTEP`, `SSDONE` and `PROMDISABLED` at 1A10; the
    /// speed synchronizer at 1A01, so it comes up extra slow --- and
    /// presets the 74LS109 at OLORD2 1A18, `BOOT.TRAP` down.
    pub fn clock_reset(&mut self) {
        self.reset();
        self.m.clock_control.run = false;
        self.srun = false;
        self.sstep = false;
        self.ssdone = false;
        self.promdisabled = false;
        self.speed = Speed::ExtraSlow;
        self.speed_a = Speed::ExtraSlow;
        self.boot_trap = false;
    }

    /// What `-MEMACK` does to the cpu without waiting for its clock:
    /// `-LOADMD` strobes the word into `MD` when the bus interface says,
    /// `MBUSY` falls at `-MFINISHD`, 30 ns after it, and `READ IN PROGRESS`
    /// at `-RDFINISH`, 140 ns after it.
    ///
    /// The strobe belongs here and not on the clock edge. `MD`'s clock is the
    /// 74S51 at MD 1D16, `(DESTMDR AND -CLK2C) OR LOADMD`, so a word off the
    /// bus lands when the bus says and not when the next microcycle ends.
    /// Landing it at the edge instead is a microcycle late whenever
    /// `-RDFINISH` falls inside the same microcycle as the acknowledgement:
    /// `READ IN PROGRESS` was already down when the instruction reading `MD`
    /// arrived, so it did not hang, and it read the previous word. At the
    /// 220 ns the machine boots in that is every read whose `MD` is used
    /// three instructions on rather than two --- `GET-AREA-ORIGINS` in
    /// microcode 323, which filled its table one entry behind.
    fn after_memack(&mut self, now: u64) {
        if now >= self.loadmd_at {
            self.m.md = self.busint_bus;
            self.loadmd_at = u64::MAX;
        }
        if now >= self.rd_finish_at {
            self.rd_in_progress = false;
        }
        // `MBUSY` falls, `MEMRQ` with it, and the bus interface lets go:
        // "MEMRQ drops when MEMACK rises, which causes MEMACK to drop."
        if now >= self.mbusy_clear_at {
            self.mbusy = false;
            self.mbusy_clear_at = u64::MAX;
            self.bus.finish();
            self.bus_acked = false;
        }
    }

    /// Why the clock generator is being held off, if it is.
    ///
    /// Both gates are on VCTL1, and this is what the boot PROM's first pair
    /// of bus cycles runs into on a board with no bus interface: `chip`
    /// stops there for ever, `MACHRUN` low and `-HANG` never asserted.
    ///
    /// ```text
    /// 3F16  74S64  -WAIT = NOR((DESTMEM AND MBUSY.SYNC),
    ///                          (USE.MD AND MBUSY AND -MEMGRANT),
    ///                          (LCINC AND NEEDFETCH AND MBUSY.SYNC))
    /// 3F17  74S10  -HANG = NAND(RD.IN.PROGRESS, USE.MD, -CLK3G)
    /// ```
    ///
    /// `WAIT` stops the cpu clock and lets the master clock run, which is what
    /// lets the bus interface start the cycle being waited for; `HANG` stops
    /// both, which is why it must not happen before `-MEMGRANT` --- MIT's own
    /// warning, and the second `-WAIT` term is the gate that enforces it.
    ///
    /// **No term holds a start in the microcycle right after a start.**
    /// `MBUSY.SYNC` is `MEMRQ` registered at the edge, and `MEMRQ` is low
    /// until the edge ending the first start raises `MEMSTART`, so the
    /// second start runs: at the edge ending it the first cycle goes out
    /// with the second's direction and `VMA<7:0>`, the second start asks
    /// for nothing more, and the first is lost ([`Rtl::start_bus_cycle`]).
    /// The board does exactly that
    /// (`on_the_board_a_start_right_after_a_start_loses_the_first`,
    /// `tests/chip.rs`).
    fn stall(&self, r: &Read) -> Option<Stall> {
        let wait = (r.destmem && self.mbusy_sync)
            || (r.use_md && self.mbusy && !self.bus.granted())
            || (r.lcinc && r.needfetch && self.mbusy_sync);
        if wait {
            return Some(Stall::Wait);
        }
        if r.use_md && self.rd_in_progress {
            return Some(Stall::Hang);
        }
        None
    }

    /// The MD interlock: the instruction reads `MD` while a read is in
    /// progress, and waits for its word --- `-WAIT`'s `USE.MD AND MBUSY AND
    /// -MEMGRANT` before the grant, `-HANG`'s `RD.IN.PROGRESS AND USE.MD`
    /// after it, [`Rtl::stall`].
    fn md_interlock(&self, r: &Read) -> bool {
        r.use_md && ((self.mbusy && !self.bus.granted()) || self.rd_in_progress)
    }

    /// Holds the microcycle off until the bus has moved on.
    ///
    /// Neither stall runs a microcycle: the cycle happens once, later. That
    /// is why the instruction counts do not move when this is switched on and
    /// only [`Rtl::ns`] does.
    fn stall_for(&mut self, stall: Stall, r: &Read) -> bool {
        let before = self.ns;
        match stall {
            // The master clock keeps running, so the bus interface goes on
            // arbitrating and can grant the cycle being waited for --- and
            // `MBUSY.SYNC` goes on following `MEMRQ`, which is what ends the
            // wait: "MEMACK arrives, and at the next clock (0-200 ns)
            // MBUSY.SYNC clears."
            // The generator cycle the cpu sits out is as long as the
            // instruction standing in `IR` asks: `-ILONG` is `NAND(IR45,
            // -NOPA)`, and the waiting instruction is not nopped.
            Stall::Wait => {
                self.master_clock_cycle(r);
            }
            // Both clocks stop. The Xbus cycle is already running and
            // `-MEMACK` is asynchronous, so it arrives regardless.
            //
            // `-HANG` is sampled at `-TPR0`, so what it holds off is the edge
            // that *ends* the cycle reading `MD`, until `-RDFINISH`. The
            // cycle runs once here, so the stretch is charged before it and
            // the word is in `MD` for the read phase, which is what the
            // stretch is for. A read that finishes inside the cycle's own
            // length stretches nothing: MIT's "may be just barely in time to
            // avoid a HANG". Holding the whole cycle off until `-RDFINISH`
            // and then running it charges up to a microcycle too much ---
            // 100 ns on the parity loop's timed-out read, where the board
            // takes no hang at all.
            Stall::Hang => {
                let cycle = self.timing.cycle_ns(self.speed, r.ilong) as u64;
                let finish = self
                    .bus
                    .ack_at()
                    .map_or(self.rd_finish_at, |ack| ack.saturating_add(RD_FINISH_NS));
                // Between a read's start and its grant neither is known,
                // and `-WAIT` --- `USE.MD AND MBUSY AND -MEMGRANT` --- is
                // what keeps a hang from being taken there: MIT's "do not
                // hang when this line is high".  A hang with no end in
                // sight --- the cycle's timeout inhibited by the debugger,
                // [`busint::debug_modifier::TIMEOUT_INHIBIT`] --- stops
                // both clocks until something off the cpu ends it, so it
                // passes a generator cycle at a time, the bus and the cable
                // carried with it and the master clock not; the caller ends
                // the step there, as for a halted machine.
                if finish == u64::MAX {
                    self.ns += cycle;
                    self.bus_cycle(self.ns);
                    self.after_memack(self.ns);
                    self.stalled_ns += cycle;
                    return true;
                }
                // A hang whose microcycle would start past the caller's
                // bound ([`Rtl::step_until`]) is carried only to the bound,
                // and the step ends there with the hang still on: the
                // microcycle runs, at the same instant it would have, on a
                // later step.
                if finish - cycle > self.step_limit {
                    let to = self.step_limit.max(self.ns);
                    if to > self.ns {
                        self.bus_cycle(to);
                        self.after_memack(to);
                        self.stalled_ns += to - self.ns;
                        self.ns = to;
                    }
                    return true;
                }
                let end = finish.max(self.ns + cycle);
                self.ns = end - cycle;
                self.bus_cycle(end);
                self.after_memack(end);
            }
        }
        self.stalled_ns += self.ns - before;
        false
    }

    /// One generator cycle with the cpu clock held off: a `WAIT`, or the
    /// halted machine.  The master clock runs on, so the bus interface goes
    /// on arbitrating and can grant the cycle being waited for, `MBUSY.SYNC`
    /// goes on following `MEMRQ`, the speed synchronizer takes up a new
    /// mode register, and the edge shifts the run and step synchronizers on
    /// OLORD1.  The generator cycle is as long as the instruction standing
    /// in `IR` asks: `-ILONG` is `NAND(IR45, -NOPA)`.
    fn master_clock_cycle(&mut self, r: &Read) -> u64 {
        let before = self.ns;
        let speedclk = self.speedclk_at();
        if let Some(at) = speedclk {
            self.land_write(at);
        }
        // The debug master's write of the mode register is carried to
        // `SPEEDCLK` too, not only to the edge.  The processor's own write
        // is, on the line above, for the reason [`Rtl::land_write`] gives:
        // the speed synchronizer samples the register sixty nanoseconds
        // into every generator cycle.  A write over the debug cable lands
        // in [`Rtl::debug_cycle`] instead, and landing it only at the edge
        // made this engine take a speed change **one generator cycle later
        // than the board does**.  Measured against the netlist: the mode
        // register loads at 5093 ns, `SPEED1A` follows at the 5100
        // `SPEEDCLK` and `SSPEED1` at the 5320 one, so the board's cycle
        // beginning 5260 already runs at the new length --- while `rtl`,
        // having landed the write only at 5260, was still an extra-slow
        // cycle behind and ran 220 where the board ran 145.  The two then
        // stood 75 ns apart, which is 220 less 145, for the rest of the
        // run.  `chip_and_rtl_take_a_speed_change_on_the_same_cycle` holds
        // it.
        //
        // Only when there is a debug write still to land: this runs every
        // microcycle, and a machine with nothing on the cable answers
        // `None` here without touching the state machine.
        if let Some(at) = speedclk
            && !self.debug_written
            && self.bus.debug_answered_at().is_some()
        {
            self.debug_cycle(at);
        }
        self.speedclk();
        self.ns += self.timing.cycle_ns(self.speed, r.ilong) as u64;
        self.bus_cycle(self.ns);
        self.after_memack(self.ns);
        // `MEMSTART`'s D is `MEMPREPARE`, `NOR(CLK2C, -MEMOP)` at VCTL1
        // 1D27, which is low with the cpu clock held: the edge that starts a
        // pending cycle also clears it.
        let memstart = self.memstart;
        // The cpu clock is held, so `WRCYC` holds too.
        self.start_bus_cycle(r, self.wrcyc);
        self.memstart = false;
        self.bus.mclk_edge(self.ns, self.bus_responder);
        self.mbusy_sync = (memstart && r.vmaok) || self.mbusy;
        self.mclk_edge();
        self.ns - before
    }

    /// `MEMSTART` taken at a master clock edge: the 74S175 at ACTL 1E20
    /// that holds it, and the 74S74s at 1D21 for `MBUSY` and `READ IN
    /// PROGRESS`, are all clocked by `MCLK1A`, the master clock, and not by
    /// the cpu clock.  A cycle started by the last microcycle therefore goes
    /// out at the next master clock edge whether or not the cpu clock runs
    /// again, which is what a halted or single-stepped machine sees.
    ///
    /// `wrcyc` is `WRCYC` as the edge leaves it, which is what the bus is
    /// given: the Xbus's write line comes off the flip-flop through the bus
    /// interface, which lets it "just pass straight through"
    /// (`mit/cadr/busint.erface`). `READ IN PROGRESS`, clocked at the same
    /// edge, takes `RDCYC` from before it.
    ///
    /// **A start in the microcycle right after a start** sees the first
    /// cycle go out at the edge ending the second start's microcycle, with
    /// the second's direction, and finds `MBUSY` up at the next: nothing more
    /// is asked of the bus, one cycle goes out for the two, and the first
    /// is lost. The map's latch at VMEMDR 1D14 is transparent while
    /// `MEMSTART` is up, and `MEMSTART` is up through both microcycles, so
    /// the cycle's page follows `VMA` to the second's; the address and a
    /// write's `MD` are taken again here, and `READ IN PROGRESS` comes up
    /// for a read. Measured on `chip`, the netlist bus interface and memory
    /// boards: one grant, the second start's direction, page and word
    /// (`on_the_board_a_start_right_after_a_start_loses_the_first`,
    /// `a_start_right_after_a_start_goes_out_as_the_second`,
    /// `a_fetch_right_after_a_write_loses_the_write`, `tests/chip.rs`).
    /// The bus interface keeps the responder it decoded for the first
    /// address; **unverified** what the board does when the second start's
    /// address is on another responder, which a start on main memory then
    /// one on the Unibus would settle.
    /// The cycle's physical address, from the latched map word and `VMA`:
    /// the page `VMO<13:0>` and `VMA<7:0>`.
    fn physical(&self) -> u32 {
        (self.lvmo & 0x3fff) << 8 | (self.m.vma as u32 & 0xff)
    }

    fn start_bus_cycle(&mut self, r: &Read, wrcyc: bool) {
        if self.memstart {
            self.lvmo = r.vmo;
            if r.vmaok && self.mbusy {
                self.bus_addr = self.physical();
                self.bus_data = self.m.md;
                if self.rdcyc {
                    self.rd_in_progress = true;
                    if !self.bus_acked {
                        self.rd_finish_at = u64::MAX;
                    }
                }
            } else if r.vmaok {
                self.mbusy = true;
                // The bus interface holds the address and, on a write, the
                // word, until the cycle ends: `xspec.text.3` requires the
                // master to keep them stable from 80 ns before `-XBUS.RQ`
                // "until the -XBUS.ACK signal drops".  So they are captured
                // here, at the edge `-MEMRQ` goes out on, not read again when
                // the answer arrives. That edge ends the microcycle after the
                // start, and `MD` is written above before this runs, so an
                // `MD` loaded in that microcycle is the word written: the
                // board's too, `MD` passing through the bus interface with
                // no latch (`the_engines_write_the_md_of_the_microcycle_after_the_start`,
                // `tests/chip.rs`).
                self.bus_addr = self.physical();
                self.bus_data = self.m.md;
                self.bus_responder = busint::decode_for(
                    self.bus_addr,
                    self.m.main.len(),
                    self.m.color_tv.is_some(),
                    self.m.tv.buffer_words(),
                    self.m.tv.control_registers(),
                );
                if std::mem::take(&mut self.m.dma_written) {
                    self.bus.invalidate_cache();
                }
                self.bus.request_at(wrcyc, self.bus_addr);
                self.bus_cycles += 1;
                self.bus_written = false;
                self.bus_spy = busint::unibus_address(self.bus_addr).and_then(spy::register);
                self.bus_sampled = false;
                self.bus_pulsed = false;
                // `READ IN PROGRESS` comes up on the same edge, for a read,
                // and has no falling time until `-MEMACK` gives it one.
                if self.rdcyc {
                    self.rd_in_progress = true;
                    self.rd_finish_at = u64::MAX;
                }
            }
        }
    }

    /// The OPCS shift registers' clock, and what a rising edge on it does.
    ///
    /// The 9328s at OPCS 1F06-1F13 clock on either of two pins: their own,
    /// which OPCS 1F10 drives with `OPCINH`, and the common `OPCCLKA-C`,
    /// which the 74S02 at 1F14 makes as `NOR(OPCCLK, -CLK5)`.  So the clock
    /// is `OPCINH OR (CLK5 AND -OPCCLK)`: with both control bits down it is
    /// the cpu clock and the history shifts every microcycle; `OPCINH` up
    /// holds it high and freezes the history; and with the machine halted
    /// --- `-CLK0` held low at CLOCK2 1D10, so `CLK5` high --- lowering
    /// `OPCCLK` is a rising edge, which is how CC's `CC-SAVE-OPCS` reads the
    /// eight PCs out one at a time.  What shifts in is `PC`, pins 6 and 11.
    fn opc_clock(&mut self, clk5: bool, pc: u16) {
        let o = self.m.opc_control;
        let ck = o.opcinh || (clk5 && !o.opcclk);
        if ck && !self.opc_ck {
            // A shift of eight halfwords, spelled out: `rotate_right` on an
            // array this small is a call and two `memmove`s, and it was
            // seven per cent of an `rtl` run.
            self.opc.copy_within(0..7, 1);
            self.opc[0] = pc;
        }
        self.opc_ck = ck;
    }

    /// The time this engine has spent stalled on the bus, waits and hangs
    /// together.  On a machine that runs throughout, its clock less this is
    /// the sum of the microcycles' own periods, which is `micro`'s clock,
    /// and `tests/cosim.rs` holds the two to each other; time spent halted
    /// ([`Rtl::halted_ns`]) or with the clock ring held by the debug cable's
    /// reset is in neither.
    pub fn stalled_ns(&self) -> u64 {
        self.stalled_ns
    }

    /// How many memory cycles this engine has started: with
    /// [`Rtl::stalled_ns`], the mean the machine waits per cycle, which is
    /// what `micro` charges per cycle when asked to include memory access.
    pub fn bus_cycles(&self) -> u64 {
        self.bus_cycles
    }

    /// How many debug cycles this machine, as the debugger, has put on the
    /// cable: CC's `DBG-READ`s and `DBG-WRITE`s, one each.
    pub fn debug_cycles(&self) -> u64 {
        self.debug_cycles
    }

    /// The clock edge: every register takes what the read phase held.
    fn clock_edge(&mut self, r: &Read) {
        // What the edge captures is what stood before it, so the instruction
        // being replaced is still needed after `IR` has moved on.
        let was_ir = self.ir;
        // page IREG: the OA registers substitute fields as the word loads
        let iob = r.i | ((r.ob & 0x003f_ffff) << 26) | (r.ob & 0x03ff_ffff);
        let mut ir = r.i;
        if r.destimod1 {
            ir = (ir & !(0x1ffu64 << 40) & !0x1fff_fc00_0000) | (iob & 0x1_ffff_fc00_0000);
            ir &= !(1u64 << 48);
        }
        if r.destimod0 {
            ir = (ir & !0x03ff_ffff) | (iob & 0x03ff_ffff);
        }

        // page ACTL: the pending write, handed to the next microcycle.
        self.wadr = r.wadr_in;
        self.destd = r.dest;
        self.destmd = r.destm;
        // page L
        self.l = r.ob;
        // page PDLCTL
        self.pwidx = r.destpdl_x;
        self.pdlwrited = r.pdlwrite;
        // page VCTL2: the map write is delayed, gated by `WMAPD`.
        self.wmapd = r.wmap;

        // pages CONTRL, PDLCTL and SPCW: what the stack write will stand on.
        // `SPCW` itself is not latched --- the 74S157s at 4E12-4E14 build it
        // from these three.
        self.spushd = r.spush;
        self.destspcd = r.destspc;
        self.reta = r.reta_in;

        // page STAT: `-STATBIT` is the count enable on the first 74S169 and
        // the rest chain its carry; the console's `-LDSTAT` is their LOAD,
        // from `IWR<31:0>` as it stands before this edge, IWR clocking on the
        // same edge; and `STAT.OVF`, the last carry out, is what OLORD2 1A05
        // registers as `STATSTOP`.
        self.statstop = self.stat == u32::MAX && r.statbit;
        // The same register takes `-HALT`: the instruction in `IR` asks
        // for the halt, and `IR` holds it while the machine is halted, so
        // `HALTED` stays until a step brings the next instruction in.
        self.halted = r.halt;
        if self.m.clock_control.ldstat {
            self.stat = self.iwr as u32;
        } else if r.statbit {
            self.stat = self.stat.wrapping_add(1);
        }

        // page IWR
        // `IWR<47:32>` from `A<15:0>` and `IWR<31:0>` from `M<31:0>`.
        self.iwr = ((r.a & 0xffff) << 32) | (r.m & LOW);

        // page NPC / LPC / OPCS.  `Machine::opc` is the PC of the
        // instruction that just executed, which is what LPC holds; the OPC
        // the microcode reads is the shift register, eight deep.  The
        // console can hold LPC and can hold or step the shift register.
        let old_pc = self.pc;
        self.pc = r.npc;
        if !self.m.opc_control.lpc_hold {
            self.lpc = old_pc;
        }
        self.m.opc = old_pc;
        self.opc_clock(false, old_pc);
        self.opc_clock(true, old_pc);
        self.ir = ir;
        // page PDLCTL 4C11
        self.imodd = r.imod;

        // The microcycle's own length, off the delay-line taps.  Nothing on
        // the board samples it --- `ILONG` and the speed bits only choose
        // which tap ends the read phase --- so it is a number and not a
        // mechanism.
        self.ns += self.timing.cycle_ns(self.speed, r.ilong) as u64;
        self.div_from_ns = self.ns;

        // page LC
        let counter = self.m.geometry.lc_counter();
        if r.destlc {
            self.lc = r.ob & counter;
        } else {
            let inc = (r.lcinc && !self.lc_byte_mode) as u64 + r.lcinc as u64;
            self.lc = (self.lc & counter).wrapping_add(inc) & counter;
        }
        // page FLAG
        if r.destintctl {
            self.lc_byte_mode = bit(r.ob, 29);
            let reset = bit(r.ob, 28);
            if reset != self.prog_unibus_reset {
                // The interface puts it on the backplane as `-XBUS INIT`
                // and `-UB INIT`: the memory boards are held by it, and the
                // model I/O boards clear what their reset pins clear,
                // `Machine::bus_reset`.
                self.bus.unibus_reset(self.ns, reset);
                if reset {
                    self.m.bus_reset();
                }
            }
            self.prog_unibus_reset = reset;
            self.int_enable = bit(r.ob, 27);
            self.sequence_break = bit(r.ob, 26);
        }

        // page Q
        // The steps and the shifts on `Q<31:0>`, whatever is above; a load
        // takes the whole word, as the output bus has it for an ALU
        // function.
        let q_high = self.m.q & !LOW;
        let q_low = self.m.q as u32;
        if r.qs1 || r.qs0 {
            self.m.q = match (r.qs1 as u8) * 2 + r.qs0 as u8 {
                1 => q_high | Word::from((q_low << 1) | (!bit(r.alu, 31)) as u32),
                2 => q_high | Word::from(((r.alu as u32 & 1) << 31) | (q_low >> 1)),
                _ => r.alu & LOW,
            };
        }

        // page SPC pointer
        if r.spcnt {
            self.m.spcptr = if r.spush {
                (self.m.spcptr + 1) & 0o37
            } else {
                self.m.spcptr.wrapping_sub(1) & 0o37
            };
        }

        // page PDLPTR
        if r.destpdlx {
            self.m.pdl_index = r.ob as u16 & self.m.geometry.pdl_mask();
        }
        if r.destpdlp {
            self.m.pdl_pointer = r.ob as u16 & self.m.geometry.pdl_mask();
        } else if r.pdlcnt {
            self.m.pdl_pointer = if r.srcpdlpop {
                self.m.pdl_pointer.wrapping_sub(1) & self.m.geometry.pdl_mask()
            } else {
                (self.m.pdl_pointer + 1) & self.m.geometry.pdl_mask()
            };
        }

        // page MD / VMA: MDSEL is `DESTMDR AND -CLK2C` at VCTL2 1D27. This
        // is the instruction's half of `MDCLK`; the bus's half is
        // [`Rtl::after_memack`].
        if r.destmdr {
            self.m.md = r.ob;
        }
        if r.vmaenb {
            self.m.vma = r.vmas;
        }
        // page VCTL1: the memory cycle.  MEMPREPARE is the write phase's
        // level, MEMSTART its registered copy, so a cycle prepared here runs
        // over the next microcycle. The bus is given `WRCYC` as this edge
        // leaves it, below: a cycle going out at the edge that starts
        // another takes the new one's direction.
        self.start_bus_cycle(r, if r.memop { r.memwr } else { self.wrcyc });
        // `WRCYC` and `RDCYC` are one flip-flop: 1C23's 74S175 on `CLK2A`,
        // whose D comes off the 74S51 at 1D16 as
        // `NOT((MEMPREPARE AND -MEMWR) OR (-MEMPREPARE AND RDCYC))`.  With
        // `MEMPREPARE` up that is `MEMWR`, and with it down it is `WRCYC`
        // again --- load or hold.  So the direction is the *starting*
        // instruction's and it stands until the next cycle starts.
        //
        // Taking it from the instruction standing one microcycle later is
        // the trap: that instruction is the page-fault check MIT puts after
        // every store, so `MEMWR` reads false and **every write cycle is
        // performed as a read**.  Nothing catches it early --- the only thing
        // the boot PROM writes before the disk is page 0, which it fills with
        // the zeros already there.
        if r.memop {
            self.wrcyc = r.memwr;
            self.rdcyc = !r.memwr;
        }
        self.m.vmaok = r.vmaok;
        // `MEMRQ` off the 9S42 at 1E25 is `MEMSTART AND VMAOK OR MBUSY`, and
        // `MBUSY.SYNC` is it registered here.
        self.mbusy_sync = (self.memstart && r.vmaok) || self.mbusy;
        self.memstart = r.memop;
        self.memstart_fetch = r.needfetch && r.lcinc;
        // The master clock edge the bus interface runs on. `MEMRQ` is a level
        // during the cycle and "the bus interface only looks at it towards
        // the end of the cycle", which is here.
        self.bus.mclk_edge(self.ns, self.bus_responder);

        // page CONTRL / LCC
        // page DSPCTL 3C14/3C15: 25S07s enabled by `-IRDISP` and clocked by
        // `CLK3E`, so they take `IR<41:32>` of the DISPATCH itself --- the
        // word standing before the edge, not the one it loads. Reading the
        // new `IR` here is a cycle early.
        if r.irdisp {
            self.m.dispatch_constant = field(was_ir, 41, 32) as u16;
        }
        self.inop = r.n;
        self.iwrited = r.iwrite;
        self.newlc = r.newlc_in;
        self.next_instrd = r.next_instr;
        // page LCC: `SINTR` is `INT` off the cables, registered by the 74S175
        // at 3E12 on `CLK3C`; `SINT` is it under `INT.ENABLE` at 4D09.
        self.sintr = self.m.interrupt_at(self.ns);
    }

    /// `SPEEDCLK`, sixty nanoseconds into a generator cycle: the two-stage
    /// synchronizer at OLORD1 1A01 shifts, and the tap this cycle ends on is
    /// chosen from what it then holds. Run at the start of every generator
    /// cycle, waited or not, and before the bus is carried forward, since an
    /// acknowledgement lands no earlier than 80 ns in and a mode register it
    /// loads is seen by the next cycle's edge and not this one's.
    /// `SPEEDCLK`'s instant in this generator cycle, which writes are
    /// carried to so that the speed synchronizer sees a new mode.
    fn speedclk_at(&self) -> Option<u64> {
        Some(self.ns + SPEEDCLK_NS)
    }

    fn speedclk(&mut self) {
        self.speed = self.speed_a;
        self.speed_a = match (self.m.mode.speed1, self.m.mode.speed0) {
            (false, false) => Speed::ExtraSlow,
            (false, true) => Speed::Slow,
            (true, false) => Speed::Normal,
            (true, true) => Speed::Fast,
        };
    }

    /// `MCLK` runs whether or not `MACHRUN` is up, so the mode register and
    /// the trap follow the console even with the machine halted: the 74S174
    /// at OLORD1 1A10 takes `RUN` into `SRUN`, `STEP` into `SSTEP`, `SSTEP`
    /// into `SSDONE` and `PROMDISABLE` into `PROMDISABLED`, and the 74LS109
    /// at OLORD2 1A18 drops `BOOT.TRAP` at the first edge with `SRUN` up.
    ///
    /// Run after the bus has been carried to the edge, so that a register
    /// the strobe loaded in this generator cycle is what the edge samples,
    /// as on the board.  The two pulses a mode-register write can make are
    /// taken here too: `-PROG.RESET` clears the registers, and `PROG.BOOT`
    /// presets `RUN` and clears the 74LS109 into `BOOT.TRAP` --- both at
    /// the strobe, before the edge.  Returns whether a boot pulse landed,
    /// with `SRUN` as it stood before the edge: the caller decides whether
    /// this edge is the trap's, and the 74LS109 clocks the trap away at the
    /// first edge whose `J`, `SRUN`, was up.
    fn mclk_edge(&mut self) -> Option<bool> {
        let boot = std::mem::take(&mut self.m.prog_boot);
        let reset = std::mem::take(&mut self.m.prog_reset) || boot;
        if reset {
            self.reset();
        }
        if boot {
            self.m.clock_control.run = true;
            self.boot_trap = true;
        }
        let was_srun = self.srun;
        if was_srun && !boot {
            self.boot_trap = false;
        }
        self.ssdone = self.sstep;
        self.sstep = self.m.clock_control.step;
        self.srun = self.m.clock_control.run;
        self.promdisabled = self.m.mode.prom_disable;
        boot.then_some(was_srun)
    }
}

impl Rtl {
    /// The bus interface as this engine has it: what a far end built beside
    /// a resumed board must be given, because the interface keeps state
    /// between cycles --- it holds the Unibus once it has had it.
    pub fn busint(&self) -> Option<&Busint> {
        Some(&self.bus)
    }

    /// Sets the clock before the machine has run: the instant it is powered
    /// on, for a machine powered beside another whose time it shares.  The
    /// memory boards' twins reckon their crystal from zero of this clock, as
    /// the netlist boards reckon theirs from zero of the board's, so two
    /// machines compared cycle for cycle must agree on it to agree on the
    /// instant a memory board answers.
    pub fn set_clock(&mut self, ns: u64) {
        assert_eq!(self.m.cycles, 0, "the clock is set before the machine runs");
        self.ns = ns;
        self.m.ns = ns;
    }

    /// The debugger puts a request on this machine's DBGIN connector at
    /// `at`, which is now or later: `-DEBUG IN REQ` down with the wires in
    /// `req`, lifted `req.hold_ns` after the acknowledgement.  A register
    /// strobe is acknowledged at once and loads its latch when lifted; a
    /// cycle goes through the Unibus arbitration and runs at the latched
    /// address --- see [`busint::DebugRequest`] and [`Rtl::debug_ack`].
    /// The status a [`busint::DEBUG_STATUS`] strobe drives onto `DBD` is
    /// read here, at the strobe.
    pub fn debug_request(&mut self, at: u64, req: busint::DebugRequest) {
        self.try_debug_request(at, req).unwrap_or_else(|e| panic!("{e}"));
    }

    /// [`Rtl::debug_request`], refusing rather than panicking on a request
    /// the cable cannot take: one while another is on it, or one in this
    /// machine's past by more than a cycle.  In process that is the
    /// lashup's scheduling error; from a peer on a stream it is the peer's,
    /// and [`crate::lashup::Remote`] ends the run with it.
    pub fn try_debug_request(&mut self, at: u64, req: busint::DebugRequest) -> Result<(), String> {
        // With both cables carrying cycles at once the lashup may let a
        // machine run less than a cycle past the other's event
        // (`lashup::Lashup::step`); such an event is taken as of now.
        // Anything later than that is a scheduling error.
        let at = Self::recent(at, self.ns, "request")?;
        self.bus.debug_advance(at);
        if self.bus.debug_pending().is_some() {
            return Err(format!("a debug request at {at} ns while one is already on the cable"));
        }
        let uaddr = self.bus.debug_unibus_address();
        // Through the Unibus map, the responder is the entry's: the buffer
        // half a register cycle, the Xbus half a cycle at the mapped
        // physical address, a load of `MD` for a write through a page whose
        // high five bits are ones, or a refusal.
        let responder = match busint::map_access(uaddr) {
            // In write-through mode the low half's write is an Xbus write
            // too, on the upper eight pages: `-UBPN3A` into the 74S32 at
            // UBCYC 0E04 with `-WRITE THROUGH ENB`.
            Some(access)
                if access.high != req.write
                    && !(req.write && self.m.write_through && access.page >= 0o10) =>
            {
                Responder::MapBuffer
            }
            Some(access) => match self.m.map_entry(access.page) {
                Some((page, true)) if req.write && busint::map_to_md(page) => Responder::MapMd,
                Some((page, writable)) if !req.write || writable => {
                    Responder::MapXbus((page << 8) | access.word)
                }
                _ => Responder::MapRefused,
            },
            None => busint::decode(busint::unibus_physical(uaddr), self.m.main.len()),
        };
        // The 8304 at REQERR 0B15 drives `DBD<7:0>`; the high byte is the
        // open cable, which the debugger's 8304s read as ones.
        self.debug_word = (req.strobe == busint::DEBUG_STATUS)
            .then(|| 0xff00 | self.m.debug_status(self.bus.busy()));
        self.debug_sampled = false;
        self.debug_written = false;
        self.debug_pulsed = false;
        // A register strobe is acknowledged the instant it is made, the
        // 74S10 at DBGIN 0A14, and the debugger may be told so at once.
        self.debug_answer = (req.strobe != busint::DEBUG_CYCLE).then_some((at, self.debug_word));
        self.bus.debug_request(at, req, responder);
        Ok(())
    }

    /// The answer to the last request: when `DEBUG ACK` rose, and the word
    /// on `DBD<15:0>` if this side drives it --- the register read, or the
    /// status --- as of that instant.  `None` until the machine has run to
    /// the acknowledgement; a cycle at an address nothing answers is never
    /// acknowledged, there being no timeout for this master.
    pub fn debug_ack(&self) -> Option<(u64, Option<u16>)> {
        self.debug_answer
    }

    /// The debugger lifts the request at `at` without an acknowledgement:
    /// [`Busint::debug_release`].
    pub fn debug_release(&mut self, at: u64) {
        self.try_debug_release(at).unwrap_or_else(|e| panic!("{e}"));
    }

    /// [`Rtl::debug_release`], refusing a release in this machine's past
    /// as [`Rtl::try_debug_request`] refuses a request.
    pub fn try_debug_release(&mut self, at: u64) -> Result<(), String> {
        let at = Self::recent(at, self.ns, "release")?;
        self.bus.debug_release(at);
        Ok(())
    }

    /// `at` if it is not in this machine's past, `now` if it is by less
    /// than a generator cycle at the slowest speed, and a refusal beyond.
    fn recent(at: u64, now: u64, what: &str) -> Result<u64, String> {
        let slack = Speed::ExtraSlow.cycle_ns(true) as u64;
        if at.saturating_add(slack) < now {
            return Err(format!(
                "a debug {what} at {at} ns, before the machine's {now} ns by more than a cycle"
            ));
        }
        Ok(at.max(now))
    }

    /// A request is on the cable, or `DBUB MASTER` is still up after one.
    pub fn debug_busy(&self) -> bool {
        self.bus.debug_pending().is_some() || self.bus.debug_master()
    }

    /// What the debuggee's side promises the debugger: no `DEBUG ACK`
    /// before this instant.  [`Busint::debug_in_promise`].
    pub fn debug_in_promise(&self) -> u64 {
        self.bus.debug_in_promise(self.ns)
    }

    // --- DBGOUT: this machine as the debugger ---

    /// A debug cable is plugged into this machine's DBGOUT, with another
    /// machine's DBGIN on it: a cycle into `766100`-`766136` waits for that
    /// machine's `DEBUG ACK`, [`Rtl::debug_out_answer`], instead of the
    /// pull-up's.
    pub fn attach_debug_cable(&mut self) {
        self.bus.attach_debug_cable();
    }

    /// The event this machine's DBGOUT has put on the cable, if one is
    /// waiting to be carried: a request with the cpu's direction and word
    /// --- `DEBUG OUT WR` is `-UBRD` and `DBD` is `UDO` through the 8304s
    /// at DBGOUT 0B21 and 0B22 --- lifted [`busint::UNIBUS_STROBE_NS`]
    /// after the acknowledgement, as `SELECT DEBUG` drops with `-UB MSYN`
    /// at `SSYN T100`; or the release of a request this side timed out.
    pub fn debug_out_take(&mut self) -> Option<busint::CableEvent> {
        Some(match self.bus.debug_out_take()? {
            busint::DebugOut::Request { at, strobe } => {
                if strobe == busint::DEBUG_CYCLE {
                    self.debug_cycles += 1;
                }
                busint::CableEvent::Request {
                    at,
                    request: busint::DebugRequest {
                        strobe,
                        write: self.wrcyc,
                        dbd: self.bus_data as u16,
                        hold_ns: busint::UNIBUS_STROBE_NS,
                    },
                }
            }
            busint::DebugOut::Release { at } => busint::CableEvent::Release { at },
        })
    }

    /// The other machine's answer to the request on the cable: `DEBUG ACK`
    /// at `ack_at`, and the word on `DBD<15:0>` if it drove one.  Returns
    /// whether this interface was still waiting for it; after its own
    /// timeout it is not.
    pub fn debug_out_answer(&mut self, ack_at: u64, word: Option<u16>) -> bool {
        if !self.bus.debug_out_answer(ack_at) {
            return false;
        }
        self.debug_out_word = word.unwrap_or(0xffff);
        true
    }

    /// What this machine promises the other about the cable: no request
    /// and no release before this instant.  [`Busint::debug_out_promise`].
    pub fn debug_out_promise(&self) -> u64 {
        self.bus.debug_out_promise(self.ns)
    }
}

impl Rtl {
    /// [`Engine::step`] with a bound: a stall that would carry the clock
    /// past `limit` --- a hang stretched to an acknowledgement far off ---
    /// is carried only to `limit`, the bus and the cable with it, and the
    /// step ends there with no microcycle run; the caller steps again.  A
    /// microcycle or a generator cycle begun before `limit` may still end
    /// after it, by less than one cycle at the slowest speed.  For the
    /// two-machine lashup, where each machine may run only as far as the
    /// other has promised: [`crate::lashup::Lashup`].
    pub fn step_until(&mut self, limit: u64) -> Result<(), Halt> {
        self.step_limit = limit;
        let r = self.step_body();
        self.step_limit = u64::MAX;
        r
    }

    fn step_body(&mut self) -> Result<(), Halt> {
        self.executed = None;
        self.fetch_started = None;
        // The ring held by the debug cable's reset: time passes and nothing
        // else --- to the strobe that may lift it, else a cycle's worth,
        // and never past the caller's limit.
        let period = self.timing.cycle_ns(self.speed, false) as u64;
        if self.clock_held {
            let mut to = (self.ns + period).min(self.step_limit.max(self.ns));
            if let Some(lift) = self.bus.debug_strobe_until()
                && lift > self.ns
            {
                to = to.min(lift);
            }
            self.ns = to;
            self.m.ns = to;
            self.debug_cycle(to);
            return Ok(());
        }
        // A modifier strobe carrying the reset bit lifts inside the cycle
        // about to run: the ring is cleared then, mid-cycle, and the edge
        // that would have ended the cycle never comes.  The strobe is on
        // the cable already, so its lift is known; go to it and no further.
        if let Some(lift) = self.bus.debug_strobe_until()
            && lift > self.ns
            && lift < self.ns + period
            && self.bus.debug_pending().is_some_and(|q| {
                q.strobe == busint::DEBUG_MODIFIER
                    && q.write
                    && q.dbd & busint::debug_modifier::RESET != 0
            })
        {
            self.ns = lift;
            self.m.ns = lift;
            self.debug_cycle(lift);
            return Ok(());
        }
        // `PROG.RESET` the same way: the debug master's write of the mode
        // register with bit 6 holds `RESET` for the write pulse, `-SPY
        // WRITE` at REQU, [`busint::REGISTER_PULSE_NS`] up to the instant
        // the register loads, and the ring runs again from that instant
        // --- measured on the netlist board, `tests/chip.rs`.  The pulse's
        // leading edge inside this cycle cuts the cycle; the machine goes
        // to the load and the next cycle starts there.  The processor's own
        // write of its mode register with the bit is **not** modeled this
        // way; nothing does that.
        if let Some(at) = self.bus.debug_answered_at()
            && at > busint::REGISTER_PULSE_NS
            && (self.ns..self.ns + period).contains(&(at - busint::REGISTER_PULSE_NS))
            && at > self.ns
            && self.bus.debug_last_request().is_some_and(|q| {
                q.strobe == busint::DEBUG_CYCLE && q.write && q.dbd & spy::MODE_RESET != 0
            })
            && spy::register(self.bus.debug_unibus_address())
                .is_some_and(|e| spy::write_strobe(e) == spy::MODE)
            && !self.debug_written
        {
            self.ns = at;
            self.m.ns = at;
            self.debug_cycle(at);
            return Ok(());
        }
        // The machine's clock, for whatever reads it between bus cycles: the
        // disk controller's done, when a run gives the disk its time.
        self.m.ns = self.ns;
        self.m.disk.advance(self.ns);
        if let Some(d) = self.m.block_disk.as_mut() {
            d.advance(self.ns);
        }
        self.m.ioboard.advance(self.ns);
        // A stall resolves within the bus timeout, `busint::TIMEOUT_NS`, so a
        // microcycle that never starts is a bug in this engine rather than
        // something the board would do. Say so loudly.
        let mut stalls = 0;
        loop {
            let r = self.read_phase();
            self.trace = [
                self.pc as u64,
                self.ir,
                self.m.q,
                r.a,
                r.m,
                r.alu & 0xffff_ffff,
                r.r,
                r.ob,
                self.m.dispatch_constant as u64,
                self.opc[7] as u64,
                self.stat as u64,
                // `LC<25:0>`, the 74S169s on page LC, masked as
                // [`Engine::lc`] masks them: this engine keeps the
                // byte-mode flags beside the counter and the netlist's
                // `LC0..LC25` are the counter alone. Issue 71.
                self.lc & u64::from(crate::machine::LC_COUNTER),
            ];
            self.flags = [
                self.wmapd as u64,
                self.destspcd as u64,
                self.iwrited as u64,
                self.imodd as u64,
                self.pdlwrited as u64,
                self.spushd as u64,
                r.nop as u64,
                // The net is `-VMAOK`, `NAND(-PFR, -PFW)` at VCTL1 1D17:
                // *low* when the access is permitted, the opposite of the
                // logical `vmaok` the jump conditions and `MEMRQ` take.
                // Reporting it in the logical sense is an error that a latch
                // coming up denying access exactly cancels, so the pair of
                // them is invisible until the first memory cycle.
                !r.vmaok as u64,
                r.jcond as u64,
                r.pcs1 as u64,
                r.pcs0 as u64,
                self.srun as u64,
            ];
            // Halted, the generator keeps running but `-CLK0` is held off at
            // CLOCK2 1D10 and the write pulses at `MACHRUNA`, so nothing
            // moves but what `MCLK` clocks --- the bus interface, the
            // synchronizers on OLORD1, and the OPC clock the console works
            // by hand --- which is how the console gets the machine started
            // again.  One master clock cycle a step, and no microcycle.
            if !r.machrun {
                self.halted_ns += self.master_clock_cycle(&r);
                self.opc_clock(true, self.pc);
                return Ok(());
            }
            // The bus stops the clock two ways, and the microcycle does not
            // start until it lets go.  A stall costs time and not a
            // microcycle: the cycle runs once, later.  [`Rtl::stall`] has the
            // two gates.  A single step's `MACHRUN` does not go through
            // `-WAIT`, so its microcycle runs with the bus still busy;
            // `-HANG` holds the generator itself and is not bypassed.
            if let Some(stall) = self.stall(&r)
                && !(stall == Stall::Wait && r.stepping)
            {
                // `-HANG` holds off the start of the next cycle and not this
                // one's write pulse: `-TPR0` is `NAND(-HANG, -CLOCK RESET B,
                // CYCLECOMPLETED)` at CLOCK1 1C08, and the pulse runs its
                // course inside the hung cycle. What it writes is what its
                // address and data are as it ends, which the clock cuts at
                // the cycle's boundary (`clock.rs`, `WP_OFF_NS`): MD as the
                // bus has left it then --- the word read if it has landed,
                // the old one if not. So the pending late writes and a
                // dispatch write land then, once. A `-WAIT` writes nothing:
                // `TPWP` is `NOR(latch, -MACHRUNA)` at CLOCK2 1C10, and
                // `-WAIT` drops `MACHRUN`. `tests/dispatch_write_order.rs`
                // holds both on the netlist, the hang at four distances
                // from the read, two landing before the pulse ends and two
                // after.
                if stall == Stall::Hang && !self.pulsed {
                    let at = self.ns + self.timing.cycle_ns(self.speed, r.ilong) as u64;
                    self.bus_cycle(at);
                    self.after_memack(at);
                    let now = self.read_phase();
                    self.write_phase(&now);
                    self.pulsed = true;
                }
                let md_wait = self.md_interlock(&r);
                let open = self.stall_for(stall, &r);
                // The divider's count starts in the generator cycle after
                // the MD interlock lets go.
                if md_wait {
                    self.div_from_ns = self.ns;
                }
                // A cycle with no acknowledgement due --- its timeout
                // inhibited from the debug cable, or the other machine's
                // answer not yet come --- is waited for one generator cycle
                // a step, so that the cable can be worked meanwhile; a hang
                // carried to the caller's bound ends the step too; and so
                // does a wait that has carried the clock to the bound, else
                // a run of waits overran it by a generator cycle each and a
                // debuggee ran past the debugger's promise.
                if open || self.bus.ack_at() == Some(u64::MAX) || self.ns >= self.step_limit {
                    return Ok(());
                }
                stalls += 1;
                assert!(
                    stalls < 1_000,
                    "the bus never let go at PC {:o}: {stall:?}, MBUSY {}, granted {}",
                    self.pc,
                    self.mbusy,
                    self.bus.granted()
                );
                continue;
            }
            if !r.nop {
                self.executed = Some(self.m.opc);
            }
            self.fetch_started = (r.needfetch && r.lcinc).then_some(r.have_wrong_word);
            if !std::mem::take(&mut self.pulsed) {
                self.write_phase(&r);
            }
            // The debug master's mode-register write reaches `SPEEDCLK`
            // here as the processor's does; see [`Rtl::master_clock_cycle`].
            if let Some(at) = self.speedclk_at() {
                self.land_write(at);
                if !self.debug_written && self.bus.debug_answered_at().is_some() {
                    self.debug_cycle(at);
                }
            }
            self.speedclk();
            // "Note that the bus interface interface must work whether the
            // cpu is stopped or not.  Once a cycle is started it goes to
            // completion even if the machine is then stopped."  So the bus is
            // carried forward every microcycle and not only while one is
            // being waited for --- `JUMP-TO-6` starts the write that turns
            // the boot PROM off and then waits for it in a loop that touches
            // no memory at all, so nothing ever stalls.
            //
            // And it is carried forward *to the edge*, before the edge: an
            // acknowledgement inside this microcycle has cleared `MBUSY`
            // before the edge registers `MBUSY.SYNC` from it --- "MEMACK
            // arrives, and at the next clock (0-200 ns) MBUSY.SYNC clears."
            // Carried forward after the edge, the instruction three after a
            // memory start saw `MBUSY.SYNC` still up and waited a microcycle
            // the board does not: `DISK-RECALIBRATE` in microcode 323, two
            // register writes three instructions apart, where `chip` took no
            // wait. The boot PROM has no such pair.
            let edge = self.ns + self.timing.cycle_ns(self.speed, r.ilong) as u64;
            self.bus_cycle(edge);
            self.after_memack(edge);
            // The master clock edge, after the bus has landed what it will
            // in this cycle.  A `PROG.BOOT` that landed is `-BOOT` within the
            // cycle: `BOOT.TRAP` is up before this edge, `TRAP` nops the
            // instruction and forces `NPC` to 0, and the edge takes it --- so
            // the read phase is taken again with the trap in it, and the
            // 74LS109 at OLORD2 1A18 clocks the trap away at this same edge
            // if `SRUN` was already up.
            let booted = self.mclk_edge();
            if let Some(was_srun) = booted {
                let r = self.read_phase();
                if was_srun {
                    self.boot_trap = false;
                }
                self.clock_edge(&r);
            } else {
                self.clock_edge(&r);
            }
            self.m.cycles += 1;
            return Ok(());
        }
    }
}

impl Engine for Rtl {
    fn nominal_cycle_ns(&self) -> u64 {
        self.timing.cycle_ns(Speed::Normal, false) as u64
    }

    /// `-BOOT` from the button: it presets `RUN` at OLORD1 1A14, clears the
    /// 74LS109 at OLORD2 1A18 into `BOOT.TRAP`, and is one of the three
    /// inputs of `RESET` at 1C08.  A finger holds it for many master clocks,
    /// so `SRUN` has followed `RUN` by the time it is released and the first
    /// microcycle after this is the trap's.  `PROG.BOOT` from the console
    /// is a pulse instead, and `Rtl::mclk_edge` takes it.
    fn boot(&mut self) {
        self.reset();
        self.m.clock_control.run = true;
        self.srun = true;
        self.boot_trap = true;
        self.promdisabled = false;
    }
    fn save(&self, w: &mut crate::checkpoint::Writer) {
        let Rtl {
            m,
            trace,
            flags,
            ir,
            iwr,
            wadr,
            destd,
            destmd,
            l,
            pc,
            lpc,
            lc,
            pwidx,
            pdlwrited,
            inop,
            iwrited,
            newlc,
            sintr,
            next_instrd,
            lc_byte_mode,
            int_enable,
            sequence_break,
            prog_unibus_reset,
            boot_trap,
            promdisabled,
            srun,
            sstep,
            ssdone,
            div_from_ns,
            pulsed,
            statstop,
            halted,
            opc_ck,
            halted_ns,
            memstart,
            mbusy,
            rdcyc,
            wrcyc,
            bus,
            mbusy_sync,
            bus_addr,
            bus_data,
            bus_written,
            bus_spy,
            bus_sampled,
            bus_pulsed,
            bus_responder,
            rd_in_progress,
            rd_finish_at,
            mbusy_clear_at,
            bus_acked,
            debug_word,
            debug_sampled,
            debug_written,
            debug_pulsed,
            debug_answer,
            debug_reset,
            debug_cycles,
            clock_held,
            debug_out_word,
            step_limit,
            wmapd,
            lvmo,
            stalled_ns,
            bus_cycles,
            spushd,
            destspcd,
            reta,
            imodd,
            opc,
            stat,
            speed,
            speed_a,
            timing,
            ns,
            busint_bus,
            loadmd_at,
            executed,
            // Cleared every microcycle, and only the profile reads it.
            fetch_started: _,
            memstart_fetch,
        } = self;
        m.save(w);
        w.u64s(trace);
        w.u64s(flags);
        w.u64(*ir);
        w.u64(*iwr);
        w.u16(*wadr);
        w.bool(*destd);
        w.bool(*destmd);
        w.word(*l);
        w.u16(*pc);
        w.u16(*lpc);
        w.u32(*lc as u32);
        for bit in [
            pwidx,
            pdlwrited,
            inop,
            iwrited,
            newlc,
            sintr,
            next_instrd,
            lc_byte_mode,
            int_enable,
            sequence_break,
            prog_unibus_reset,
            boot_trap,
            promdisabled,
            srun,
            sstep,
            ssdone,
            statstop,
            halted,
            opc_ck,
        ] {
            w.bool(*bit);
        }
        w.u64(*halted_ns);
        w.u64(*div_from_ns);
        w.bool(*pulsed);
        w.bool(*memstart);
        w.bool(*mbusy);
        w.bool(*rdcyc);
        w.bool(*wrcyc);
        // The bus's kind, 0 the bus interface's, as the format has it.
        w.u8(0);
        bus.save(w);
        w.bool(*mbusy_sync);
        w.u32(*bus_addr);
        w.word(*bus_data);
        w.bool(*bus_written);
        w.opt(*bus_spy, crate::checkpoint::Writer::u8);
        w.bool(*bus_sampled);
        w.bool(*bus_pulsed);
        bus_responder.save(w);
        w.bool(*rd_in_progress);
        w.u64(*rd_finish_at);
        w.u64(*mbusy_clear_at);
        w.bool(*bus_acked);
        w.opt(*debug_word, crate::checkpoint::Writer::u16);
        w.bool(*debug_sampled);
        w.bool(*debug_written);
        w.bool(*debug_pulsed);
        w.opt(*debug_answer, |w, (at, word)| {
            w.u64(at);
            w.opt(word, crate::checkpoint::Writer::u16);
        });
        w.bool(*debug_reset);
        w.u64(*debug_cycles);
        w.bool(*clock_held);
        w.u16(*debug_out_word);
        w.u64(*step_limit);
        w.bool(*wmapd);
        w.u32(*lvmo);
        w.u64(*stalled_ns);
        w.u64(*bus_cycles);
        w.bool(*spushd);
        w.bool(*destspcd);
        w.u16(*reta);
        w.bool(*imodd);
        w.u16s(opc);
        w.u32(*stat);
        w.speed(*speed);
        w.speed(*speed_a);
        match *timing {
            TimingModel::Cadr => w.u8(0),
            TimingModel::Fpga => w.u8(1),
        }
        w.u64(*ns);
        w.word(*busint_bus);
        w.u64(*loadmd_at);
        w.opt(*executed, crate::checkpoint::Writer::u16);
        w.bool(*memstart_fetch);
    }

    fn load(&mut self, r: &mut crate::checkpoint::Reader) -> std::io::Result<()> {
        use crate::checkpoint::Reader;
        self.m.load(r)?;
        r.u64s_into(&mut self.trace)?;
        r.u64s_into(&mut self.flags)?;
        self.ir = r.u64()?;
        self.iwr = r.u64()?;
        self.wadr = r.u16()?;
        self.destd = r.bool()?;
        self.destmd = r.bool()?;
        self.l = r.word()?;
        self.pc = r.u16()?;
        self.lpc = r.u16()?;
        self.lc = u64::from(r.u32()?);
        for bit in [
            &mut self.pwidx,
            &mut self.pdlwrited,
            &mut self.inop,
            &mut self.iwrited,
            &mut self.newlc,
            &mut self.sintr,
            &mut self.next_instrd,
            &mut self.lc_byte_mode,
            &mut self.int_enable,
            &mut self.sequence_break,
            &mut self.prog_unibus_reset,
            &mut self.boot_trap,
            &mut self.promdisabled,
            &mut self.srun,
            &mut self.sstep,
            &mut self.ssdone,
            &mut self.statstop,
            &mut self.halted,
            &mut self.opc_ck,
        ] {
            *bit = r.bool()?;
        }
        self.halted_ns = r.u64()?;
        self.div_from_ns = r.u64()?;
        self.pulsed = r.bool()?;
        self.memstart = r.bool()?;
        self.mbusy = r.bool()?;
        self.rdcyc = r.bool()?;
        self.wrcyc = r.bool()?;
        self.bus = match r.u8()? {
            0 => {
                let mut b = Busint::with_timing_model(self.m.memory_boards(), self.timing);
                b.load(r)?;
                Box::new(b)
            }
            k => return Err(crate::checkpoint::bad(format!("bus kind {k}"))),
        };
        self.mbusy_sync = r.bool()?;
        self.bus_addr = r.u32()?;
        self.bus_data = r.word()?;
        self.bus_written = r.bool()?;
        self.bus_spy = r.opt(Reader::u8)?;
        self.bus_sampled = r.bool()?;
        self.bus_pulsed = r.bool()?;
        self.bus_responder = Responder::load(r)?;
        self.rd_in_progress = r.bool()?;
        self.rd_finish_at = r.u64()?;
        self.mbusy_clear_at = r.u64()?;
        self.bus_acked = r.bool()?;
        self.debug_word = r.opt(Reader::u16)?;
        self.debug_sampled = r.bool()?;
        self.debug_written = r.bool()?;
        self.debug_pulsed = r.bool()?;
        self.debug_answer = r.opt(|r| Ok((r.u64()?, r.opt(Reader::u16)?)))?;
        self.debug_reset = r.bool()?;
        self.debug_cycles = r.u64()?;
        self.clock_held = r.bool()?;
        self.debug_out_word = r.u16()?;
        self.step_limit = r.u64()?;
        self.wmapd = r.bool()?;
        self.lvmo = r.u32()?;
        self.stalled_ns = r.u64()?;
        self.bus_cycles = r.u64()?;
        self.spushd = r.bool()?;
        self.destspcd = r.bool()?;
        self.reta = r.u16()?;
        self.imodd = r.bool()?;
        r.u16s_into(&mut self.opc)?;
        self.stat = r.u32()?;
        self.speed = r.speed()?;
        self.speed_a = r.speed()?;
        self.timing = match r.u8()? {
            0 => TimingModel::Cadr,
            1 => TimingModel::Fpga,
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "checkpoint: not a timing model",
                ));
            }
        };
        self.ns = r.u64()?;
        self.busint_bus = r.word()?;
        self.loadmd_at = r.u64()?;
        self.executed = r.opt(Reader::u16)?;
        self.memstart_fetch = r.bool()?;
        self.bus.keep_timing_model(self.timing);
        self.m.disk.set_timing_model(self.timing);
        Ok(())
    }

    /// One microcycle.
    ///
    /// It cannot fail. An address nothing answers is timed out by the bus
    /// and sets an NXM bit, as the board does, so there is nothing left to
    /// halt on.
    fn step(&mut self) -> Result<(), Halt> {
        self.step_body()
    }

    fn pc(&self) -> u16 {
        self.pc
    }

    fn pc_is_a_write(&self) -> bool {
        self.iwrited
    }

    /// The 74S169 counters at LC 1A26-2C05, which is where this engine
    /// keeps the location counter: `Machine::lc` is never written here.
    fn lc_wide(&self) -> u64 {
        self.lc & self.m.geometry.lc_counter()
    }

    /// The sixteen registers off this engine's state between two
    /// microcycles.  `OB`, `A`, `M` and the four combinational flags are the
    /// read phase of the instruction standing in `IR`, computed afresh: the
    /// datapath is combinational, so a halted machine shows the console the
    /// result of the instruction it has not yet executed, which is what
    /// CC's `CC-EXECUTE-R` relies on.  `IR48` is the control store's parity
    /// bit, which this engine does not carry, and reads as zero; no parity
    /// error is ever flagged, since no parity check is modeled.
    fn spy_read(&self, eadr: u8) -> u16 {
        let r = self.read_phase();
        let half = |v: u64, k: u8| (v >> (16 * k as u32)) as u16;
        match eadr {
            spy::IR_LOW | spy::IR_MED | spy::IR_HIGH => half(self.ir, eadr),
            spy::OPC => self.opc[7] & 0x3fff,
            spy::PC => self.pc & 0x3fff,
            spy::OB_LOW => r.ob as u16,
            spy::OB_HIGH => (r.ob >> 16) as u16,
            spy::FLAG_1 => spy::Flag1 {
                wait: self.stall(&r) == Some(Stall::Wait),
                promdisable: self.m.mode.prom_disable,
                stathalt: self.m.mode.stathenb && self.statstop,
                err: self.err(),
                ssdone: self.ssdone,
                srun: self.srun,
                ..Default::default()
            }
            .word(),
            spy::FLAG_2 => spy::Flag2 {
                wmapd: self.wmapd,
                destspcd: self.destspcd,
                iwrited: self.iwrited,
                imodd: self.imodd,
                pdlwrited: self.pdlwrited,
                spushd: self.spushd,
                ir48: false,
                nop: r.nop,
                vmaok: r.vmaok,
                jcond: r.jcond,
                pcs1: r.pcs1,
                pcs0: r.pcs0,
            }
            .word(),
            spy::M_LOW => r.m as u16,
            spy::M_HIGH => (r.m >> 16) as u16,
            spy::A_LOW => r.a as u16,
            spy::A_HIGH => (r.a >> 16) as u16,
            spy::STAT_LOW => self.stat as u16,
            spy::STAT_HIGH => (self.stat >> 16) as u16,
            _ => spy::OPEN_READ,
        }
    }

    fn machine(&self) -> &Machine {
        &self.m
    }

    fn machine_mut(&mut self) -> &mut Machine {
        &mut self.m
    }
}
