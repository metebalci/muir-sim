// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The bus interface, as the CPU sees it across the cables.
//!
//! The Lisp Machine Bus Interface is its own board --- MIT's parts list
//! `cadr1/busint.prt` heads itself "LISPM Bus Interface", 33 sheets, board
//! type `LG684` against the processor's `MPG216` --- and it is not in the
//! processor's backplane:
//!
//! > The bus interface plugs into the Xbus and the Unibus, and is connected
//! > to the cpu by 5 40-wire (alternating grounds) flat cables, which carry
//! > two independent busses and the master clock, which is supplied by the
//! > cpu to the bus interface.
//!
//! --- `cadr/busint.erface`, "Interface to the Bus Interface". This module is
//! the far end of the memory bus among those cables: what answers `-MEMRQ`,
//! and **when**. It does not decide what the answer *is*; that is
//! [`crate::machine::Machine`], which holds the memories and the devices.
//! Time here, data there.
//!
//! Two MIT documents specify it, both **primary** and both shipped with the
//! drawings rather than found on a web page:
//!
//! - `cadr/busint.erface` --- the cpu-to-busint interface: the signals on the
//!   cables, `WAIT` against `HANG`, and what each flip flop on VCTL1 is for.
//! - `cadr1/xspec.text.3` --- the Xbus specification: the read and write
//!   cycles, the deskew delays, and the arbitration between the processor and
//!   the other masters on the bus, of which the disk is one.
//!

use crate::clock::{GRID_NS, TimingModel};
use crate::{disk_controller, ioboard, spy, tv};

/// "it is the responsibility of the bus master to assert good address, write,
/// and data lines 80 ns. prior to asserting -XBUS.RQ" --- `xspec.text.3`.
pub const SETUP_NS: u64 = 80;

/// "masters should delay 50 ns. after receiving -XBUS.ACK before dropping
/// -XBUS.RQ and strobing the data" --- `xspec.text.3`. The specification's
/// figure; the board's is [`XBUS_ACK_NS`].
pub const STROBE_NS: u64 = 50;

/// The deskew the board actually performs on a read: the 74S64 at REQLM
/// 0C11 makes `XACK` from `XBUS ACK IN` at once for a write and through the
/// 60 ns tap of the TD100 at 0C09 for a read, and `-LMACK` and the rising
/// edge of `-LOADMD` follow `XACK`. So `MD` takes the word with the
/// acknowledgement, 60 ns after the device answered. Measured on the
/// netlist bus interface against `rtl` in `tests/cables.rs`, where a read
/// used two instructions on hung 60 ns longer than the model allowed.
pub const XBUS_ACK_NS: u64 = 60;

/// How long the bus interface waits before giving up on an address.
///
/// **There are two timers and they are not the same.** MIT's
/// From the first edge of the timeout counter's clock after the grant to
/// the edge that registers `NXM TIMEOUT`: five intervals of the 74LS124 at
/// REQTIM 0A01, [`NXM_VCO_NS`] each. Where the first edge falls against
/// the grant is the clock's business, and [`nxm_timeout_at`] has it.
///
/// `sys/doc/disk.text` says a device that "failed to respond within 15
/// microseconds" is a nonexistent-memory error --- but that is the *disk
/// controller* timing out its own Xbus cycles. The one that times out the
/// processor's is a PROM on the bus interface, and it is dumped:
/// `cadr1/reqtim.prom`, "REQTIM NXM TIMEOUT PROM (74S288)", a 32x8 table
/// walked one state per clock, whose own comments give
///
/// ```text
///                 Normal      When referencing other processor
/// NXM timeout     10 uSec     30 uSec
/// HUNG timeout    20 uSec     32 uSec
/// ```
///
/// **What the PROM fixes is the count, and its microseconds are another
/// board's.** The register it drives --- the 74LS273 at REQTIM 0B01, held
/// clear by `INT BUSY` until the grant and clocked by the oscillator's
/// output --- walks one state per rising edge; state 5 is where the table
/// raises `PROM NXM TIMEOUT`, and the flag is registered on the edge after,
/// the sixth. The header's "This Assumes Roughly 2 uSec clock intervals"
/// is the clock of the board after `cadr1/busint.eco` item 5 of February
/// 1981 put an S part at 1000 pF there; the board the netlist is built
/// from has the LS part at 100 pF, which its own sheet puts near a
/// microsecond ([`crate::chip::VCO_PERIOD`] has the evidence, and the
/// band). The oscillator has run since power-on and its output is gated,
/// not started, by the grant, so the sixth edge is between five and a half
/// intervals and six and a half after it: the PROM's 10 microseconds is
/// 4.7 to 5.5 here, and `rtl` gives a cycle up at the instant the board
/// does, to the nanosecond (`tests/chip.rs`). The 80 ns of setup before
/// `-XBUS RQ` are inside the wait, not before it: `INT BUSY` rises with
/// `LMX GRANT` or `LMUB GRANT`. The second column is the debug cable's,
/// [`DEBUG_TIMEOUT_NS`] --- and the PROM's table says 26, not the 30 its
/// header says.
pub const TIMEOUT_NS: u64 = 5 * NXM_VCO_NS;

/// The interval of the 74LS124 at REQTIM 0A01 that clocks the timeout
/// counter: [`crate::chip::VCO_PERIOD`], 850 ns, in whole nanoseconds.
///
/// The oscillator runs from power-on and the grant only opens its output,
/// which `rtl` reckons as `chip` runs it, through [`crate::chip::gated_rise`]
/// from `t = 0`. The board showed the counter is let go the same way when
/// the debugger's timeout inhibit is lifted: a cycle held open by the
/// inhibit and released at instants a microsecond apart was given up on at
/// instants an interval apart --- [`TIMEOUT_NS`] after the first of the
/// output's rising edges past the release, the edges those of the clock
/// from power-on (`chip_and_rtl_hold_an_unanswered_cycle_under_the_timeout_inhibit_alike`).
pub const NXM_VCO_NS: u64 = crate::chip::VCO_PERIOD.0 / crate::chip::VCO_PERIOD.1;

/// When a cycle granted at `granted_at` is given up on if nothing answers
/// it: `INT BUSY` lets the counter go at the grant, and `NXM TIMEOUT` is
/// registered on the sixth rising edge of the oscillator's output after
/// that, [`TIMEOUT_NS`] after the first.
pub fn nxm_timeout_at(granted_at: u64) -> u64 {
    crate::chip::gated_rise(crate::chip::VCO_PERIOD, granted_at, 1) + TIMEOUT_NS
}

/// When the debugger's own interface gives up a debug cycle granted at
/// `granted_at` that the other machine has not answered: the fourteenth
/// edge, [`DEBUG_TIMEOUT_NS`] after the first.
pub fn debug_timeout_at(granted_at: u64) -> u64 {
    crate::chip::gated_rise(crate::chip::VCO_PERIOD, granted_at, 1) + DEBUG_TIMEOUT_NS
}

/// An ideal device: it answers as early as the bus allows.
///
/// `xspec.text.3` permits it --- responding devices "are allowed to assert
/// -XBUS.ACK at the same time they drive read data onto the -XBUS lines" ---
/// so a device that answers in no time of its own still obeys every rule of
/// the protocol. The 80 ns of setup before `-XBUS.RQ` and the 50 ns before
/// the master strobes are the bus's, not the device's, and they are charged
/// anyway.
///
/// This is a **choice**, not a gap being papered over, but it has a
/// consequence worth keeping in mind: nothing in the material we hold gives a
/// real memory board's access time --- the Xbus specification is a protocol,
/// `busint.erface` is the cpu's side, and the memory boards' drawings are not
/// among the recovered files --- so no measurement can currently replace it,
/// and **every figure that depends on it is a lower bound on what the
/// hardware took**. It is a field on [`Busint`] rather than a constant, so a
/// slower device is one line and a test.
pub const IDEAL_DEVICE_NS: u64 = 0;

/// From the grant going out on the Unibus to `SACK` coming back: `NPG1 IN`
/// through two sections of the MTD100 at UBMAST 0D01, `LM UB GRANTED`
/// clocked off one section and `LM UB SELECTED` off the other.
///
/// Measured on the netlist bus interface in `tests/busint_netlist.rs`. The
/// MTD100 is 100 ns a section, and MIT's own files say so twice over: the
/// body it is drawn from, in `cadr/bodies.drw`, reads "MTD100 / 14 Pin /
/// 100 NS DELAY", and `cadr1/busint.wlr` names the two nets the sections
/// make `NPG1 IN T100` and `NPG2 IN T100`. The part is Engineered
/// Components' MTTLDL-100: three separate, individually buffered delay
/// lines in one 14-pin DIP, `IN1` pin 1 to `OUT1` pin 12, `IN2` 3 to `OUT2`
/// 10 and `IN3` 5 to `OUT3` 8, the -100 calibrated at 100.0 +/- 3.0 ns
/// (`mttldl.pdf`). `busint.wlr` gives 0D01's pins those same six `USE`
/// codes: `NPG1 IN` on `IN2` and `NPG1 IN T100` off `OUT2`, `NPG1 OUT` on
/// `IN1` and `NPG2 IN T100` off `OUT1`.
pub const UNIBUS_SELECT_NS: u64 = 200;

/// From the request synchronizer's grant to `-UB MSYN`: the address is on
/// the Unibus at `LMUB GRANT`, and `MSYN OUT` is `UNIBUS REQUEST`, the
/// 74S74 at REQUB 0B10 clocked by `INT BUSY T100`, the TD100 at 0B06.
pub const UNIBUS_ADDRESS_NS: u64 = 100;

/// From a mapped Unibus cycle's start to its Xbus request.  `UB XBUS T0`
/// is `NAND(-UB WRITE XBUS, -UB READ XBUS)` at REQU 0F03, up with `-UB
/// MSYN` for the Xbus half of a mapped access; `-UBXRQ` at 0C05 wants it
/// and `UB XBUS T100`, the MTD100 at 0C07 pins 5 to 8, and the map entry
/// valid.  `UBXRQ` is then registered at RQSYNC 0A08 on the master clock,
/// and `UBX GRANT` at 0A06 on the edge after, when the interface is free
/// and the processor is not asking for the Xbus.
pub const UB_XBUS_REQUEST_NS: u64 = 100;

/// From `-UBACK` --- `UBX GRANT AND XACK` at REQU 0F03 --- to `SSYN OUT`
/// for a mapped *read*: the 74S260 at REQU 0E06 takes `NOR(-UB READ XBUS,
/// -UBACK T100)`, the 100 ns tap of the TD100 at 0F01, so that the word is
/// in the read buffer --- `-RBUFWE` runs from `-UBACK` to its 60 ns tap ---
/// before the master strobes; a write's is `NOR(-UBACK, -UB WRITE XBUS)`,
/// at once.
pub const UB_XBUS_READ_ACK_NS: u64 = 100;

/// From `-MEMACK` to the cpu's `-MEMRQ` rising: `-MFINISHD`, the 74S74 at
/// VCTL1 1D21 that holds `MBUSY` cleared 30 ns after the acknowledgement,
/// and `-MEMRQ` with it.  Measured on the board: `-UB SSYN` at 1230 ns
/// after a debug request, `-MEMRQ` up at 1410, [`UNIBUS_ACK_NS`] and
/// this between them.
pub const MFINISHD_NS: u64 = 30;

/// What an interface with no cycle in its arbitration promises about its
/// next debug request: `-MEMRQ` is sampled at an edge, the request
/// synchronizer at the next, and the grant, `SACK`, mastery and `-UB
/// MSYN` come after that.  On the board, from `MEMRQ` rising to `-UB MSYN`,
/// at the least 760 ns at extra slow, 535 at normal and 505 at fast
/// (`the_interfaces_idle_promise_is_kept_on_the_board` in
/// `tests/chip.rs`, which holds the promise under all three).  Two
/// generator cycles at the slowest speed is the promise made, so that two
/// machines each promising the other, cabled both ways, can both run:
/// each may step while a cycle of slack fits under the other's promise.
pub const IDLE_OUT_PROMISE_NS: u64 = 440;

/// From the processor's `-MEMRQ` rising to `DBUB MASTER` when the debug
/// master's grant was still out on the chain --- its `SACK` made while the
/// processor's own cycle was in progress, so that `SACKD` never withdrew
/// it: `NPG1 IN` comes down through its TD100 tap, `NPG1 IN T100`, and
/// `BUS READY` then lets the master set.  **Measured on the board**:
/// `-MEMRQ` up at 970 ns after the debug request, `NPG1 IN` down and
/// `DBUB MASTER` up at 1080.  With the grant already withdrawn the master
/// sets the instant `-MEMRQ` rises.
///
/// **Held to the nanosecond** by
/// `chip_and_rtl_arbitrate_a_debug_cycle_against_a_running_processor_alike`
/// in `tests/chip.rs`, which runs a debug request against a processor whose
/// own Unibus cycle is in flight: 109 fails there and so does 111.
///
/// Do not conclude from a sweep that deletes this arm that nothing checks
/// it.  At 110 the arm is *behavior-preserving*: `-MEMRQ` plus 110 is
/// exactly the instant the long way round --- `Granted` to `Selected` at
/// the next master clock edge --- reaches, so removing the arm leaves every
/// sequence in that file passing while changing the constant by one
/// nanosecond does not.  The two are different questions.  Whether the arm
/// is distinguishable from the edge route at all is a question about some
/// speed other than extra slow, which is the only one the board tests run.
pub const GRANT_WITHDRAW_NS: u64 = 110;

/// A mapped write into `MD`, [`Responder::MapMd`]: `-LOADMD ACK` and with
/// it `SSYN`, this long after the master clock edge that grants the cycle
/// and loads `MD`.  **Measured on the netlist board**, `tests/chip.rs`.
pub const UB_MD_ACK_NS: u64 = 100;

/// How long the board's own registers take to answer `-UB MSYN` with
/// `-UB SSYN`: page UBCYC runs `UB REG CYC T0` down the TD250 at 0F04 for
/// any of them, strobes the register between the 50 and 150 ns taps, and
/// answers at the last.
pub const DIAGNOSTIC_NS: u64 = 250;

/// From `-UB SSYN` to `-LMACK`: the 74S51 at REQLM 0B11 acknowledges a
/// Unibus cycle with `LMUB GRANT AND SSYN T150`, the 150 ns tap of the
/// TD250 at REQU 0B09. A cycle nothing answers is acknowledged the same
/// way: `SSYN T0` is `SSYN IN OR NXM TIMEOUT` at 0A09.
pub const UNIBUS_ACK_NS: u64 = 150;

/// From `-UB SSYN` to the edge that loads `MD` on a Unibus read: `MSYN OUT`
/// drops at `SSYN T100` and `-LOADMD` rises with it, so the word lands 50 ns
/// *before* the acknowledgement, where an Xbus word lands with it.
/// Measured in `tests/busint_netlist.rs`.
pub const UNIBUS_STROBE_NS: u64 = 100;

/// From `-UB MSYN` to the edge that writes one of the board's own
/// registers: page UBCYC's `UB REG WRITE PULSE` runs from the 50 ns tap of
/// the TD250 at 0F04 to the 150, and its end is the clock --- on the
/// diagnostic bus it is the trailing edge of `-DBWRITE`, "which causes a
/// clock". So a write of the mode register has changed the machine's speed
/// [`DIAGNOSTIC_NS`] plus [`UNIBUS_ACK_NS`] minus this before the cpu is
/// told the cycle is over, and the generator cycle after the acknowledgement
/// already runs at the new speed. Found in `tests/cables.rs` at the cold
/// boot's write of `46`.
pub const REGISTER_STROBE_NS: u64 = 150;

/// How long `UB REG WRITE PULSE` is up: from the 50 ns tap of the TD250 at
/// UBCYC 0F04 to the 150.  A register loads at its trailing edge, but the
/// two things a mode-register write can *do* rather than store ---
/// `-PROG.RESET` and `PROG.BOOT`, the pulse gated with `SPY6` and `SPY7` on
/// OLORD2 --- are asserted from its leading edge, this much before
/// [`REGISTER_STROBE_NS`].  The 74LS109's clear that makes `BOOT.TRAP` is
/// asynchronous, so a cpu clock edge inside the pulse takes the trap: seen
/// on the boards in `tests/chip.rs`, one microcycle before the register
/// loaded.
pub const REGISTER_PULSE_NS: u64 = 100;

/// From `NPG1 IN`, the grant arriving on the chain, to `DB UB SELECTED`:
/// one section of the MTD100 at UBMAST 0D01, pin 3 to pin 10, clocking the
/// 74LS74 at 0D02.  The debug master is the first position on the board's
/// stretch of the grant chain and the processor the second --- `NPG1 OUT`
/// is passed on a section later, through the 74S10 at 0D06 when
/// `-DBUB GRANTED` is still up, and `LM UB SELECTED` comes a section after
/// that, [`UNIBUS_SELECT_NS`].  The MTD100's 100 ns a section is MIT's own
/// figure, as there: the net off `OUT2` is `NPG1 IN T100`.
pub const DEBUG_SELECT_NS: u64 = 100;

/// From `DBUB MASTER` to `-UB MSYN` for the debug master.  `MSYN OUT` at
/// REQUB 0D20 is `UNIBUS REQUEST OR (DB NEED UB AND DBUB MASTER T100)`
/// through the 74S51 at 0C06, the delay being the MTD100 at REQUB 0C07
/// between pins 1 and 12; the address latches at DBGIN 0A18 and 0A19 are
/// output-enabled by `-DBUB MASTER` and the 8838s on page UBA by
/// `-UBADRIVE`, `NOR(LMUB MASTER, DBUB MASTER)` at DATCTL 0B17, so the
/// address has been on the Unibus that long.  The processor's `UNIBUS
/// REQUEST` gives its `-UB MSYN` the same 100 ns, [`UNIBUS_ADDRESS_NS`].
pub const DEBUG_MSYN_NS: u64 = 100;

/// From `-DEBUG IN REQ` rising to `DBUB MASTER` clearing.  The 74S74 at
/// UBMAST 0D04 has ground on its D and `-DB NEED UB` through the MTD100 at
/// 0D01, pins 5 to 8, on its clock, so it clocks itself clear a section
/// after the request ends, and `-UB BBSY` and the address drivers go with
/// it.  `MSYN OUT` drops with `DB NEED UB` itself.  The processor's `LMUB
/// MASTER` has no such clock and keeps the bus.
pub const DEBUG_RELEASE_NS: u64 = 100;

/// From `-UB MSYN` of the debugger's own cycle into its debug block to
/// `-DEBUG OUT REQ` on the cable: `SELECT DEBUG` rises with `MSYN IN`
/// through the 74S139 at UBCYC 0E07, and the request is `NAND(SELECT DEBUG,
/// SELECT DEBUG DLYD)` at DBGOUT 0A11, the delay the MTD100 at 0A10 --- so
/// `DBD`, the two address bits and the write flag have been on the cable a
/// section before the request.  The request lifts when `SELECT DEBUG`
/// drops, with `-UB MSYN` at `SSYN T100`, [`UNIBUS_STROBE_NS`] after the
/// acknowledgement.
pub const DEBUG_OUT_REQUEST_NS: u64 = 100;

/// How long the debugger's interface waits for the other machine before
/// giving up on a debug cycle: the REQTIM PROM's second table, selected by
/// `DEBUG REQUEST ACTIVE` --- `SELECT DEBUG` registered in the 74LS273 at
/// REQTIM 0B01 --- which raises `NXM TIMEOUT` at count 13, "35 36 ;26 usec
/// NXM timeout", thirteen of the 74LS124's intervals after the first edge
/// of its output past the grant, as the first table's count 5 is
/// [`TIMEOUT_NS`]; [`debug_timeout_at`] has the instant.  **The PROM's own
/// header says 30 microseconds** ("When referencing other processor: NXM
/// timeout 30 uSec, HUNG timeout 32 uSec"); its table says 26 and 30.  The
/// table is what is burned (discrepancy 67), and the microseconds are the
/// later board's: on this one the wait from the grant is between 11.5 and
/// 12.3.  Measured on the netlist board in `tests/chip.rs`.
pub const DEBUG_TIMEOUT_NS: u64 = 13 * NXM_VCO_NS;

/// The memory board's timing, as `rtl` runs it: a behavioral twin of the
/// control logic of `data/CADRM.netlist`, measured on the netlist board in
/// `tests/cadrm_netlist.rs` and held to it by `chip_agrees_with_rtl`.
///
/// The board is synchronous to its own 24 MHz crystal, whose rising edges
/// from power-on are [`rising_edge`]. A request that finds the board idle
/// is taken at the first edge strictly after it, acknowledged
/// [`MEMORY_CYCLE_STAGES`] edges later, and the board is busy
/// [`MEMORY_BUSY_STAGES`] edges from that edge; one that finds it busy
/// waits for the idle and is taken at the edge after that. Refresh is a
/// one-shot of [`REFRESH_NS`] restarted at every refresh cycle's first
/// stage, where `-REFRESH NOW` falls --- first at the board's second
/// power-on refresh, [`MEMORY_POWER_ON_EDGE`] --- whose end passes
/// through two stages clocked by the Xbus clock, the master clock the
/// interface puts on the backplane, before it is a request; a refresh
/// request is taken as a cycle is, holds the board the same time, and
/// goes first when both are waiting at an edge. Every board is the same
/// netlist powered on at the same time, so one twin stands for all
/// thirty-two.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryBoard {
    /// When the board is idle again: the end of the cycle in flight,
    /// `-BUSY` up at its twelfth edge.
    idle_at: u64,
    /// When the master lifted, or will lift, the request of the cycle in
    /// flight: `IDLE` waits for that as well as for `-BUSY`, and a refresh
    /// waiting behind the cycle is taken at the edge after both. The
    /// interface lifts it 30 ns after the acknowledgement as the
    /// processor sees it, which for a read is 60 ns after the board's.
    release_at: u64,
    /// When the one-shot's pulse ends and `TIME FOR REFRESH` rises.
    refresh_time_at: u64,
    /// When `TIME FOR REFRESH` last fell, or will: the end of the refresh
    /// cycle's busy, where the one-shot is restarted.
    time_off_at: u64,
    /// The synchronizer: 0 waiting for the time, 1 one stage in, 2
    /// `REFRESH RQ` up since `refresh_rq_at`.
    refresh_stage: u8,
    refresh_rq_at: u64,
    /// Held in reset by `-XBUS INIT`, which the interface asserts for as
    /// long as the processor's Unibus reset is on.
    in_reset: bool,
    /// Whose time the twin keeps. Not in a checkpoint: the interface that
    /// owns the board says, [`Busint::keep_timing_model`].
    timing: TimingModel,
}

/// How many memory boards the machine has by default: 2M words, 64K a
/// board, which is what the machine the band was built for had.
/// `--main-memory-boards` sets another count, up to [`MAX_MEMORY_BOARDS`].
pub const MEMORY_BOARDS: usize = 32;

/// The most memory boards the backplane can hold: sixty, because the Xbus
/// I/O space begins at page `0o36000` --- physical `0o17000000`, the
/// sixty-first board's first word --- and [`decode`] answers the frame
/// buffer, the display's registers and the disk's there before it looks
/// for memory. The board's address switch has six bits, which would reach
/// sixty-four; the last four places are the Xbus I/O space's and the
/// Unibus's.
pub const MAX_MEMORY_BOARDS: usize = (0o36000 << 8) >> 16;

/// The `j`-th rising edge of the board's oscillator from power-on, which
/// is what clocks the `-T0`..`-T640` chain: every 125/3 ns, to the
/// nanosecond below, as `chip` runs the 24 MHz crystal (see
/// [`crate::chip::DIP_OSCILLATOR_PERIOD`]). The chain's taps are named as
/// if a stage were 40 ns.
pub fn rising_edge(j: u64) -> u64 {
    crate::chip::toggle_at(crate::chip::DIP_OSCILLATOR_PERIOD, 2 * j)
}

/// The first rising edge strictly after `t`, by number.
pub fn rising_edge_after(t: u64) -> u64 {
    let (num, den) = crate::chip::DIP_OSCILLATOR_PERIOD;
    let mut j = t * den / num;
    while rising_edge(j) <= t {
        j += 1;
    }
    while j > 0 && rising_edge(j - 1) > t {
        j -= 1;
    }
    j
}

/// From the edge that takes a request to `XACK`, in chain stages: `-T440`,
/// the eleventh, measured.
pub const MEMORY_CYCLE_STAGES: u64 = 11;

/// From the edge that takes a cycle, refresh included, to the board being
/// idle again for a request, in stages: `-BUSY` lifts at `-T480`, the
/// twelfth, and a waiting request is taken at the edge after that.
/// Measured on the first memory board in the boot's parity loop, where a
/// cycle landing in a refresh was taken one edge later than the eleventh
/// stage had it, and on the board alone in `tests/cadrm_netlist.rs`.
pub const MEMORY_BUSY_STAGES: u64 = 12;

/// The refresh one-shot's pulse, from the drawing's timing components
/// through the Am26S02's formula. See [`crate::chip::MEMCTL_REFRESH_NS`].
pub const REFRESH_NS: u64 = crate::chip::MEMCTL_REFRESH_NS;

/// From the edge that takes a refresh cycle to `-REFRESH NOW` falling,
/// which triggers the one-shot, in stages: it falls with `-BUSY`, at the
/// edge after. Measured on the first board from cold.
pub const REFRESH_TRIGGER_STAGES: u64 = 1;

/// From the oscillator edge after a reset's release to the end of the
/// refresh cycle the reset had stuck, in stages: `-T40` up at that edge
/// and `-T440` ten stages on. See [`MemoryBoard::unibus_reset`].
/// `a_refresh_stuck_by_a_reset_ends_where_the_model_says` in
/// `tests/cadrm_netlist.rs` holds it to the netlist board at four phases.
pub const MEMORY_RELEASE_STAGES: u64 = 10;

/// The edge that takes the refresh cycle the boards' one-shot first runs
/// from after the button. Powered with the machine, the netlist board
/// finds `TIME FOR REFRESH` up from power-on and makes two refresh cycles
/// while the button is held; `-REFRESH NOW` is low through the first, so
/// only the second's fall of it, at its twenty-third edge, triggers the
/// one-shot, and every refresh after that is [`REFRESH_NS`] and the
/// synchronizer from the one before. Measured on the first board in
/// `chip_agrees_with_rtl`.
pub const MEMORY_POWER_ON_EDGE: u64 = 22;

impl Default for MemoryBoard {
    fn default() -> Self {
        MemoryBoard::with_timing_model(TimingModel::Cadr)
    }
}

impl MemoryBoard {
    /// The board at power-on, on `model`'s time: its crystal is a clock
    /// that runs freely from power-on and the refresh one-shot a delay its
    /// cycle starts, [`TimingModel`].
    pub fn with_timing_model(model: TimingModel) -> MemoryBoard {
        let edge = |j| model.free_running(rising_edge(j));
        let up = edge(MEMORY_POWER_ON_EDGE + MEMORY_BUSY_STAGES);
        let off = edge(MEMORY_POWER_ON_EDGE + REFRESH_TRIGGER_STAGES);
        MemoryBoard {
            idle_at: up,
            refresh_time_at: off + model.triggered(REFRESH_NS),
            time_off_at: off,
            release_at: 0,
            refresh_stage: 0,
            refresh_rq_at: 0,
            in_reset: false,
            timing: model,
        }
    }

    /// The `j`-th rising edge of the board's crystal as the twin takes it:
    /// [`rising_edge`], at the first tick at or after it on the grid.
    fn edge(&self, j: u64) -> u64 {
        self.timing.free_running(rising_edge(j))
    }
}

impl MemoryBoard {
    /// An Xbus clock edge: the refresh synchronizer's two stages. A refresh
    /// cycle due since the last edge runs whether or not anything asked.
    /// An edge in the same instant as the one-shot's end does not count:
    /// the flop samples `TIME FOR REFRESH` as it was, which the band
    /// showed at 1,465,003, where the two coincided to the nanosecond.
    pub fn xbus_clock(&mut self, now: u64) {
        self.refresh_before(now);
        if self.refresh_stage < 2 && now > self.refresh_time_at {
            self.refresh_stage += 1;
            if self.refresh_stage == 2 {
                self.refresh_rq_at = now;
            }
        }
    }

    /// The processor's Unibus reset going on or off at `now`: the interface
    /// puts it on `-XBUS INIT`, and the board's `-RESET` holds its `BUSY`
    /// flop clear. A refresh the synchronizer asks for meanwhile starts its
    /// chain, which shifts through once and sticks; on release the chain
    /// shifts back, and the cycle ends, `-BUSY` up and the one-shot
    /// restarted, 400 ns after the first oscillator edge past the release.
    /// Measured on the first board across the boot PROM's reset.
    pub fn unibus_reset(&mut self, now: u64, on: bool) {
        if on {
            self.in_reset = true;
            return;
        }
        self.in_reset = false;
        if self.refresh_stage == 2 {
            self.idle_at = self.edge(rising_edge_after(now) + MEMORY_RELEASE_STAGES);
            // `-REFRESH NOW` is `REFRESH CYC` and the board not idle; the
            // reset held `IDLE A`, so it falls as the release lets the
            // cycle go, and the one-shot runs from the release.
            self.time_off_at = now;
            self.refresh_time_at = now + self.timing.triggered(REFRESH_NS);
            self.refresh_stage = 0;
        }
    }

    /// Runs the refresh cycle the synchronizer has requested, if its edge
    /// comes before `before`.
    fn refresh_before(&mut self, before: u64) -> bool {
        // A request in flight whose release has not been seen keeps the
        // board from idle, and so from a refresh.
        if self.refresh_stage != 2 || self.in_reset || self.release_at == u64::MAX {
            return false;
        }
        // `IDLE` returns when `-BUSY` lifts and the request is gone.
        let idle = self.idle_at.max(self.release_at);
        let taken = rising_edge_after(self.refresh_rq_at.max(idle));
        if self.edge(taken) >= before {
            return false;
        }
        self.idle_at = self.edge(taken + MEMORY_BUSY_STAGES);
        // `TIME FOR REFRESH` falls as the cycle begins, and the one-shot
        // runs from there.
        self.time_off_at = self.edge(taken + REFRESH_TRIGGER_STAGES);
        self.refresh_time_at = self.time_off_at + self.timing.triggered(REFRESH_NS);
        self.refresh_stage = 0;
        true
    }

    /// The master lifts the request of the cycle in flight at `at`.
    pub fn released(&mut self, at: u64) {
        self.release_at = at;
    }

    /// The earliest instant at which an Xbus clock edge would change this
    /// twin, as [`MemoryBoard::xbus_clock`] stands: an edge before it is
    /// a no-op, and [`Busint::mclk_edge`] skips the twins until then. What
    /// changes it otherwise --- a request, a release, the reset --- goes
    /// through `Busint`, which asks again.
    pub fn next_change(&self) -> u64 {
        if self.refresh_stage < 2 {
            // The synchronizer shifts at the first edge *after* the
            // one-shot's end.
            return self.refresh_time_at.saturating_add(1);
        }
        if self.in_reset || self.release_at == u64::MAX {
            return u64::MAX;
        }
        // `refresh_before` runs the cycle at the first edge after the one
        // that takes it.
        let idle = self.idle_at.max(self.release_at);
        let taken = rising_edge_after(self.refresh_rq_at.max(idle));
        self.edge(taken).saturating_add(1)
    }

    /// Whether `TIME FOR REFRESH` is up at `now`: from the one-shot's end
    /// until the refresh cycle it asks for has run. For holding the twin
    /// to the board.
    pub fn time_for_refresh(&self, now: u64) -> bool {
        now >= self.refresh_time_at || now < self.time_off_at
    }

    /// A request on the Xbus at `rq`: when the board acknowledges it.
    pub fn request(&mut self, rq: u64) -> u64 {
        loop {
            let taken = rising_edge_after(rq.max(self.idle_at));
            // A refresh request already up at that edge goes first.
            if self.refresh_before(self.edge(taken) + 1) {
                continue;
            }
            self.idle_at = self.edge(taken + MEMORY_BUSY_STAGES);
            // Until the master lifts the request the board is not idle,
            // however long the cycle's busy has been over: a refresh waiting
            // in that gap waits for the release. `rtl` says when the release
            // will be as soon as it knows the acknowledgement; a far end on
            // real wires says so when `-XBUS RQ` lifts, and between the two
            // the twin must not guess. Found with main memory as twins on
            // `chip`'s far end, at 536,673 in the boot.
            self.release_at = u64::MAX;
            return self.edge(taken + MEMORY_CYCLE_STAGES);
        }
    }
}

/// The I/O board's timing, as `rtl` runs it: a behavioral twin of the
/// answer of `data/CADRIO.netlist`, measured on the netlist board in
/// `tests/cadrio_netlist.rs` (`the_answer_rule_for_the_twin`).
///
/// The board runs on a microsecond clock divided from its 32 MHz crystal,
/// whose rising edges come [`IOB_FIRST_USEC_EDGE_NS`] after power-on and
/// every 1,000 ns from there, whatever the Unibus does: the 74S163 at
/// IOBCLK 0C21 has its clear and load tied high, and so has the divider
/// at LMTCLK 0A06 its preset and clear, so a Unibus reset does not touch
/// the clock (`the_microsecond_clock_runs_free_of_the_reset`). The clocks, the GPIO and the
/// microsecond counter's high half answer `-MSYN` through the TD250 at
/// IOBADR 0E09, [`IOB_STRAIGHT_NS`] on; the keyboard, mouse, status and
/// beep registers select through two stages on the microsecond clock and
/// answer [`IOB_STRAIGHT_NS`] after the second edge strictly after
/// `-MSYN`; the counter's low half, which latches the count, takes one
/// edge and [`IOB_USEC_LOW_NS`].
#[derive(Clone, Debug, Default)]
pub struct IoBoardTiming {
    timing: TimingModel,
}

/// From power-on to the first rising edge of the board's `1 USEC CLK`:
/// the 74S163 at IOBCLK 0C21 counting the 16 MHz `MCLK^` from its
/// power-on state. Measured; and measured again in the far end, where
/// the board had lost seven edges at the button before
/// `Unibus::transition_due` replayed them.
pub const IOB_FIRST_USEC_EDGE_NS: u64 = ioboard::FIRST_USEC_EDGE_NS;

/// From `-MSYN`, or from the microsecond edge that lets a register go,
/// to `-SSYN`: the TD250 at IOBADR 0E09. Measured.
pub const IOB_STRAIGHT_NS: u64 = 250;

/// From the microsecond edge to `-SSYN` for the counter's low half, which
/// latches the count on its way: the next edge of the 16 MHz `MCLK^`,
/// 62.5 ns on and lying at whole nanoseconds as the crystal's edges do,
/// then the TD250. Measured at 315 first, on a harness that looked every
/// five nanoseconds; the band's first read of the register, at 2,087,406,
/// took 313 on the netlist.
pub const IOB_USEC_LOW_NS: u64 = 313;

/// From `-MSYN` to `-SSYN` for the Chaosnet transmit buffer's write and
/// for START, through the transmitter's `-TSR.SSYN`.  Measured, 350 at
/// every phase of the board's clocks.
pub const IOB_CHAOS_BUFFER_NS: u64 = 350;

/// The I/O board's `FCLK^`, 8 MHz from the 32 MHz crystal through the
/// 74S163 at LMTCLK 0B03: an edge every 125 ns, at multiples of 125 from
/// power-on on the netlist board.
pub const IOB_FCLK_NS: u64 = 125;

/// The Chaosnet receive buffer's read: `-MSYN` taken at the first `FCLK^`
/// edge at least this long after it, the word out of the buffer's RAM and
/// `-SSYN` a TD250 later.  Measured: the answer stepped from 400 down to
/// 289 ns as `-MSYN` moved 111 ns later, and back to 377 at the next
/// edge; 33 and 34 are the two setups the 2 ns steps allow.
pub const IOB_RBUF_SETUP_NS: u64 = 33;

/// The half-microsecond clock the serial port's select is synchronized
/// to, through the two 74LS74s at IOBSER 0F29: an edge every 500 ns, at
/// 203 modulo 500 from power-on on the netlist board.  Measured; which
/// divider makes it is not identified.
pub const IOB_HALF_USEC_NS: u64 = 500;
pub const IOB_HALF_USEC_PHASE_NS: u64 = 203;

/// From the first half-microsecond edge strictly after `-MSYN` to the
/// serial port's `-SSYN`: the second edge and a TD250.  Measured, 853 and
/// 1,353 ns from `-MSYN` on the two sides of an edge, read or written.
pub const IOB_SERIAL_NS: u64 = 750;

impl IoBoardTiming {
    /// The board's answer times on `model`'s time: its clocks run freely
    /// from power-on and its delay lines are delays a strobe starts,
    /// [`TimingModel`].
    pub fn with_timing_model(model: TimingModel) -> IoBoardTiming {
        IoBoardTiming { timing: model }
    }

    /// The first rising edge of the microsecond clock strictly after `t`.
    fn usec_edge_after(&self, t: u64) -> u64 {
        let first = IOB_FIRST_USEC_EDGE_NS;
        if t < first {
            return first;
        }
        first + ((t - first) / 1_000 + 1) * 1_000
    }

    /// The first `FCLK^` edge taken at or after `t`. On the grid an edge is
    /// taken at the first tick at or after it, so with `t` on a tick that is
    /// the first edge less than a tick before `t`.
    fn fclk_edge_at_or_after(&self, t: u64) -> u64 {
        let from = match self.timing {
            TimingModel::Cadr => t,
            TimingModel::Fpga | TimingModel::Sync { .. } => t.saturating_sub(GRID_NS - 1),
        };
        from.div_ceil(IOB_FCLK_NS) * IOB_FCLK_NS
    }

    /// The first half-microsecond edge strictly after `t`.
    fn half_usec_edge_after(t: u64) -> u64 {
        let k = t.saturating_sub(IOB_HALF_USEC_PHASE_NS) / IOB_HALF_USEC_NS + 1;
        IOB_HALF_USEC_PHASE_NS + k * IOB_HALF_USEC_NS
    }

    /// When the board answers `-MSYN` at `msyn` for register `uaddr` ---
    /// the register the decoder reaches, [`ioboard::answers`] --- read, or
    /// written when `write`.
    pub fn answer(&self, uaddr: u32, write: bool, msyn: u64) -> u64 {
        use crate::chaos::interface as chaos;
        let t = self.timing;
        match uaddr {
            ioboard::USEC_LOW => {
                t.free_running(self.usec_edge_after(msyn)) + t.triggered(IOB_USEC_LOW_NS)
            }
            ioboard::USEC_HIGH | ioboard::CLOCK | ioboard::GPIO => {
                msyn + t.triggered(IOB_STRAIGHT_NS)
            }
            // The Chaosnet interface's registers answer straight off ---
            // `MY ADDRESS` read in 250 ns on the netlist board --- but for
            // the transmit buffer's write and START, through the
            // transmitter, and the receive buffer's read, through its RAM
            // on `FCLK^`.
            chaos::WRITE_BUFFER if write => msyn + t.triggered(IOB_CHAOS_BUFFER_NS),
            chaos::START => msyn + t.triggered(IOB_CHAOS_BUFFER_NS),
            chaos::READ_BUFFER => {
                let edge = self.fclk_edge_at_or_after(msyn + t.triggered(IOB_RBUF_SETUP_NS));
                t.free_running(edge) + t.triggered(IOB_STRAIGHT_NS)
            }
            u if ioboard::chaos_register(u) => msyn + t.triggered(IOB_STRAIGHT_NS),
            ioboard::SERIAL_FIRST..=ioboard::SERIAL_LAST => {
                t.free_running(Self::half_usec_edge_after(msyn)) + t.triggered(IOB_SERIAL_NS)
            }
            _ => t.free_running(self.usec_edge_after(msyn) + 1_000) + t.triggered(IOB_STRAIGHT_NS),
        }
    }
}

/// What answers at a physical address.
///
/// The regions are MIT's own, from the bus interface specification: Xbus memory
/// below page `0o36000`, Xbus I/O `0o36000`-`0o36777`, and the Unibus from
/// `0o37000`. `Machine` decodes with this too, so the untimed path and the
/// timed one cannot drift apart.
///
/// Which bus it is on decides the cycle's shape as well as its address: an
/// Xbus cycle is granted at the master clock that samples `-MEMRQ`, and a
/// Unibus cycle is arbitrated first. See [`Busint::mclk_edge`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Responder {
    /// A memory board, by its place on the Xbus: address bits 16-21.
    Memory(u8),
    /// A device on the Xbus: the disk controller or the display.
    Device,
    /// One of the bus interface's own registers, which the board answers
    /// itself: the diagnostic block, the interrupt and error status and the
    /// Unibus map. See [`register`].
    Interface,
    /// The I/O board, by Unibus address.
    Unibus(u32),
    /// The debug block, [`debug_register`]: a request on the debug cable to
    /// the other machine, which answers when its DBGIN does --- or never,
    /// and the cycle is given up on at [`DEBUG_TIMEOUT_NS`].  With no cable
    /// plugged in, `DEBUG OUT ACK` is pulled up by the SIP at DBGIN 0A22
    /// and the cycle is acknowledged at once.
    Debug(u8),
    /// A Unibus master's mapped access answered from the board's buffers
    /// --- the odd word of a read, the even word of a write --- as a
    /// register cycle, `-UB READ BUFFER` and `-UB WRITE BUFFER` into the
    /// 74S133 at UBCYC 0F05 like the interface's own registers.  The debug
    /// master's; see [`map_access`].
    MapBuffer,
    /// A Unibus master's mapped access that is an Xbus cycle at the
    /// physical address, the map entry valid: the even word of a read, the
    /// odd word of a write.
    MapXbus(u32),
    /// A mapped Xbus access the map refuses, the entry invalid or the page
    /// write-protected: `UB MAP ERROR`, and no answer.
    MapRefused,
    /// A Unibus master's mapped write whose page has its high five bits
    /// ones, [`map_to_md`]: the word is not written to the Xbus but loaded
    /// into the processor's `MD`.  `-UB TO MD` is the 74S133 at REQU 0D12,
    /// `NAND(UBMA<21:17>, UBXRQ, -UBRD, MSYN IN)`; it holds the Xbus
    /// request off at REQLM 0E09, and `UB MD LOAD`, `NOR(-UB TO MD, -UBX
    /// GRANT)` at REQLM 0B17, is a term of `-LOADMD` at 0C10 and of
    /// `-LOADMD ACK` at 0A11, which acknowledges the cycle.  CC's
    /// `CC-WRITE-MD`.
    MapMd,
    /// Xbus I/O with nothing at that address: the cycle times out and sets
    /// the Xbus NXM bit.
    NoXbus,
    /// The Unibus, at an address nothing built answers: the cycle times out
    /// and sets the Unibus NXM bit.
    NoUnibus,
}

impl Responder {
    /// Whether the cycle goes out on the Unibus, through the arbitration.
    pub fn on_unibus(self) -> bool {
        matches!(
            self,
            Responder::Interface
                | Responder::Unibus(_)
                | Responder::Debug(_)
                | Responder::MapBuffer
                | Responder::MapXbus(_)
                | Responder::MapRefused
                | Responder::MapMd
                | Responder::NoUnibus
        )
    }
}

/// The Unibus address a physical address names, or `None` if it is not on the
/// Unibus.
///
/// > The unibus and xbus can be referred to through the virtual memory of the
/// > Lisp machine.  When this is done, the unibus runs from address 77400000
/// > to address 77777777.  The unibus is made of 16-bit words, and each word
/// > is stored in the least significant bits of one Lisp machine word.  So,
/// > to convert a unibus address to a Lisp machine virtual address, you must
/// > divide by two (bytes to words) and then add 77400000.
///
/// --- MIT's own specification, which then does the
/// conversion from the page number rather than that constant, because the
/// boot PROM reaches the Unibus through a map entry of its own making: the
/// mode register is virtual `0o1005` and physical `0o17773005`, and only the
/// page number says it is Unibus `0o766012`. The Unibus is 18 bits and byte
/// addressed, so an Xbus word offset doubles.
pub fn unibus_address(phys: u32) -> Option<u32> {
    let page = (phys >> 8) & 0o37777;
    (page >= 0o37000).then(|| (((page - 0o37000) << 8) | (phys & 0xff)) << 1)
}

/// The physical address at which a Unibus location is reached: the inverse
/// of [`unibus_address`], for a master on the Unibus itself --- the debug
/// cable's --- whose address is the 18-bit Unibus one.  Bit 0 is dropped,
/// as the debug master's `UAO<17:1>` drop it: "Bit 0 of the address is
/// always zero", `ldbg.lisp`.
pub fn unibus_physical(uaddr: u32) -> u32 {
    let word = (uaddr >> 1) & 0o377777;
    ((0o37000 + (word >> 8)) << 8) | (word & 0xff)
}

/// A Unibus master's access through the Unibus map, `140000`-`177777`:
/// `UB17-14=MAP` at UBCYC 0E06, the 74S258s at UBMAP 0E10 and 0E11 taking
/// `UBA<13:10>` as the map's address, `UBA<9:2>` the word within the
/// mapped page --- `XAO<7:0>` --- and `UBA1` which half: the second half
/// of the 74S139 at UBCYC 0E07 decodes it with `C1 IN` into `-UB READ
/// XBUS`, `-UB READ BUFFER`, `-UB WRITE BUFFER` and `-UB WR XBUS`.  "Each
/// Lisp machine memory word is accessed as two unibus words; the low half
/// has the lower unibus address" --- `unaddr.text`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MapAccess {
    /// `UBA<13:10>`: which of the sixteen map registers.
    pub page: u8,
    /// `UBA<9:2>`: the word within the Xbus page.
    pub word: u32,
    /// `UBA1`: the high half, which is the buffer's on a read and the Xbus
    /// cycle's on a write.
    pub high: bool,
}

/// Whether a map entry's page addresses `MD` rather than the Xbus: its
/// high five bits ones, `UBMA<21:17>` into the 74S133 at REQU 0D12.
/// `unaddr.text`: "if high 5 bits of page=1, writes MD register"; CC's
/// `CC-WRITE-MD` loads map register `16` with `177000` for it.
pub fn map_to_md(page: u32) -> bool {
    page >> 9 == 0o37
}

/// The mapped access a Unibus address is, if it is in the map's range.
pub fn map_access(uaddr: u32) -> Option<MapAccess> {
    (0o140000..=0o177777).contains(&uaddr).then_some(MapAccess {
        page: ((uaddr >> 10) & 0o17) as u8,
        word: (uaddr >> 2) & 0xff,
        high: uaddr & 2 != 0,
    })
}

/// The debug block, `766100`-`766136`: `-SELECT DEBUG`, Y2 of the 74S139 at
/// UBCYC 0E07 (address bits 6 and 5 = `10`), and which of the four
/// registers --- address bits 3 and 2, which go out on the cable as `DEBUG
/// OUT A1` and `A0` through the 74S241 at DBGOUT 0A17 and are the strobe
/// the other machine's DBGIN decodes: [`DEBUG_CYCLE`] at `766100`,
/// [`DEBUG_STATUS`] at `766104`, [`DEBUG_MODIFIER`] at `766110`,
/// [`DEBUG_ADDRESS`] at `766114`.  Bits 4 and 1 are not decoded, so the
/// four repeat through `766136`.
pub fn debug_register(uaddr: u32) -> Option<u8> {
    (0o766100..=0o766137).contains(&uaddr).then_some(((uaddr >> 2) & 3) as u8)
}

/// The bus interface's own Unibus block, `0o766000` to `0o766176`.
///
/// The 74S133 at UBCYC 0E08 decodes `766000`-`766176` and the 74S139 at
/// 0E07 splits it four ways on address bits 6 and 5: the diagnostic block
/// (page DIAG, [`crate::spy`]), the interrupt block (UBINTC and REQERR), the
/// debug block (DBGOUT, the two-machine lashup) and the Unibus map (UBMAP).
/// MIT's `unaddr.text` lists them the same way: "CADR UNIBUS interrupt
/// status" at `766040`, "CADR XBUS error status" at `766044`, "Debuggee's
/// selected UNIBUS location" from `766100`, and "`XBUS<-> UNIBUS` mapping
/// registers" at `766140`-`766176`. The board answers all but the debug
/// block with the same register cycle, [`DIAGNOSTIC_NS`] after `-UB MSYN`;
/// a debug register is a cycle on the other machine's bus,
/// [`Responder::Debug`] and [`debug_register`], answered over the cable.
///
/// Within the interrupt block the 74S138 at 0E03 looks only at address bits
/// 2 and 1, so the four registers repeat every eight bytes, as the
/// diagnostic block's do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Register {
    /// One of the sixteen diagnostic registers, by [`crate::spy`]'s number.
    Diagnostic(u8),
    /// `766040`: the interrupt status register, `-LOAD INT CTL REG` on a
    /// write, which the microcode calls `INTERRUPT-CONTROL`.
    InterruptControl,
    /// `766042`: the same register's other half, `-LOAD INT CTL2 REG`,
    /// which the microcode calls `CLEAR-INTERRUPT`.
    InterruptControl2,
    /// `766044`: the error status register, `-UB ERR DRIVE` on a read and
    /// `-RESET ERR` on a write.
    ErrorStatus,
    /// `766046`: decoded, and wired to nothing.
    Unused,
    /// `766140`-`766176`: one of the sixteen Unibus map registers.
    Map(u8),
}

/// The bus interface's own register at a Unibus address, if it is one.
pub fn register(uaddr: u32) -> Option<Register> {
    if let Some(r) = spy::register(uaddr) {
        return Some(Register::Diagnostic(r));
    }
    match uaddr {
        0o766040..=0o766076 => Some(match (uaddr >> 1) & 3 {
            0 => Register::InterruptControl,
            1 => Register::InterruptControl2,
            2 => Register::ErrorStatus,
            _ => Register::Unused,
        }),
        0o766140..=0o766176 => Some(Register::Map(((uaddr - 0o766140) >> 1) as u8)),
        _ => None,
    }
}

/// A request on the debuggee's DBGIN connector, as the debugger --- machine
/// A's DBGOUT page, or a harness playing it as the PDP-11 once did ---
/// drives the cable: `-DEBUG IN REQ` down with `DEBUG IN A<1:0>`, `DEBUG IN
/// WR` and `DBD<15:0>`, and lifted `hold_ns` after `DEBUG IN ACK` comes
/// back.  The wires are held for the whole request, as the debugger's own
/// Unibus cycle holds them: the 8304s at DBGOUT 0B21 and 0B22 drive `DBD`
/// from `UDO` for as long as `DEBUG ACTIVE` is up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DebugRequest {
    /// `DEBUG IN A<1:0>`: which of the four strobes the 74S139 at DBGIN 0A15
    /// makes of it, [`DEBUG_CYCLE`] to [`DEBUG_ADDRESS`].
    pub strobe: u8,
    /// `DEBUG IN WR`.  `C1` on the debuggee's Unibus for a cycle --- `C1
    /// OUT` is `(DBUB MASTER AND DEBUG IN WR) OR (LMUB GRANT AND -LMRD)`
    /// at DATCTL 0C14 --- and the direction of the 8304s; the three
    /// register strobes load or drive whatever it says.
    pub write: bool,
    /// `DBD<15:0>` as the debugger drives them: what a latch takes at its
    /// strobe's trailing edge, and what a write cycle puts on the Unibus.
    pub dbd: u16,
    /// How long after the acknowledgement the debugger lifts the request.
    /// For machine A's DBGOUT it is A's own `-UB MSYN` dropping
    /// [`UNIBUS_STROBE_NS`] after `DEBUG SSYN`; a harness chooses.
    pub hold_ns: u64,
}

/// `DEBUG IN A<1:0>` = 0, Y0 of the 74S139 at DBGIN 0A15: `-DB NEED UB`, a
/// cycle on the debuggee's Unibus at the latched address.  The two address
/// bits are the debugger's Unibus address bits 2 and 3 --- `DEBUG OUT A0`
/// is `UBA2` and `DEBUG OUT A1` is `UBA3` through the 74S241 at DBGOUT
/// 0A17 --- so the four strobes are CC's four registers in order, and this
/// one is `766100`, "Reads or writes the debuggee-Unibus location
/// addressed by the registers below" (`ldbg.lisp`).
pub const DEBUG_CYCLE: u8 = 0;
/// `DEBUG IN A<1:0>` = 1, Y1: `-DB READ STATUS`, which enables the 8304 at
/// REQERR 0B15 to put the error status on `DBD<7:0>` --- the same eight
/// bits a read of `766044` gives, [`crate::machine::bus_error`] and
/// [`error_status`], the bus's `-FREE` in bit 6 as it stands with no
/// cycle of the debuggee's own in flight.  CC's `766104`.
pub const DEBUG_STATUS: u8 = 1;
/// `DEBUG IN A<1:0>` = 2, Y2: `-DB ADR1 CLK`, whose trailing edge clocks
/// `DBD<2:0>` into the 25LS2519 at DBGIN 0A16, [`debug_modifier`].  CC's
/// `766110`.
pub const DEBUG_MODIFIER: u8 = 2;
/// `DEBUG IN A<1:0>` = 3, Y3: `-DB ADR0 CLK`, whose trailing edge clocks
/// `DBD<15:0>` into the two 74LS374s at DBGIN 0A18 and 0A19 as
/// `UAO<16:1>`, bits 1 to 16 of the debuggee-Unibus address.  CC's
/// `766114`.
pub const DEBUG_ADDRESS: u8 = 3;

/// The modifier register, the 25LS2519 at DBGIN 0A16: three bits of
/// `DBD` taken at `-DB ADR1 CLK`, output-enabled by `-DBUB MASTER` where
/// they go to the Unibus, and cleared by `-DEBUG RESET`.  The bits are
/// the head of `ldbg.lisp`'s for `766110`, and the pins agree: `DBD0` on
/// pin 16 to `UAO17` on pin 14, `DBD1` on pin 13 to `-DEBUGEE RESET` on
/// pin 12, `DBD2` on pin 4 to `-DEBUG TIMEOUT INH` on pin 5, the last two
/// through the part's polarity-controlled outputs and so active low.
pub mod debug_modifier {
    /// "1  Bit 17 of the debuggee-Unibus address."
    pub const ADDRESS_17: u16 = 1;
    /// "2  Resets the debuggee's Unibus and bus interface.  Write a 1 here
    /// then write a 0."  `-DEBUGEE RESET` is an input of the interface's
    /// own `RESET` at DBGIN 0A14 beside `-LM UNIBUS RESET`, and of
    /// `-BUSINT LM RESET` at 0C04, which crosses the cables to OLORD2 1B10
    /// and is `-CLOCK RESET A` and `-CLOCK RESET B` there: the processor's
    /// power-on reset, which clears `RUN` and so halts the machine.
    pub const RESET: u16 = 2;
    /// "4  Timeout inhibit.  This turns off the NXM timeout for all Xbus
    /// and Unibus cycles done by the debuggee's bus interface (not just
    /// those by the debugger)."  The 74LS273 at REQTIM 0B01 that counts
    /// the timeout is held clear unless `INT BUSY AND -DEBUG TIMEOUT INH`,
    /// the 74S08 at 0B16.
    pub const TIMEOUT_INHIBIT: u16 = 4;
}

/// The interrupt status register as the board reads it back, bit by bit
/// off the drawings: `DISABLE INT GRANT` in bit 0 and `LOCAL ENABLE` in
/// bit 1 through the 74LS240 at UBINTC 0D18, the vector in bits 2 to 9 from
/// the 74LS374 at 0D17, `ENABLE UB INTS`, `INT STOPS GRANTS` and the two
/// level bits in 10 to 13 from the 25LS2519 at 0D16, `XBUS INTR IN` in bit
/// 14 and `UB INT` in bit 15. MIT: writing `766040` "writes
/// into bits 0 and 10-13 (mask 36001)", writing `766042` "writes into bits
/// 2-9 and 15 (mask 101774)".
pub mod interrupt_status {
    pub const DISABLE_INT_GRANT: u16 = 0o1;
    /// A jumper, pulled up on the board: this machine arbitrates its own
    /// Unibus.
    pub const LOCAL_ENABLE: u16 = 0o2;
    /// `ENABLE UB INTS`, bit 10 of the 25LS2519 at UBINTC 0D16: the
    /// interface takes a Unibus interrupt only while this is set. The
    /// microcode writes `6000` here --- this and bit 11 --- at the end of
    /// the cold boot, "Enable Unibus interrupts" in `uc-cold-disk.lisp`,
    /// and again at the end of every interrupt, "Enable one more Unibus
    /// interrupt" in `uc-interrupt.lisp`; before the first of those no
    /// Unibus channel exists and an interrupt taken would `ILLOP`.
    pub const ENABLE_UB_INTS: u16 = 0o2000;
    /// The vector of the interrupt taken, in place: bits 2 to 9, the
    /// 74LS374 at 0D17. A Unibus vector is a multiple of 4, so the field
    /// holds it unshifted, and `uc-interrupt.lisp` reads it back with
    /// `(BYTE-FIELD 8 2)` deposited into a zero.
    pub const VECTOR_MASK: u16 = 0o1774;
    /// `XBUS INTR IN`: the Xbus interrupt line, live.
    pub const XBUS_INTR: u16 = 0o40000;
    /// `UB INT`: a Unibus interrupt has been taken, or simulated by writing
    /// this bit.
    pub const UB_INT: u16 = 0o100000;
    /// What a write of `766040` reaches.
    pub const CONTROL_MASK: u16 = 0o36001;
    /// What a write of `766042` reaches.
    pub const CONTROL2_MASK: u16 = 0o101774;
}

/// The error status register's two bits above the ones
/// [`crate::machine::bus_error`] sets: `-FREE` in bit 6 and `WRITE THROUGH
/// ENB` in bit 7, off the 74LS244 at REQERR 0C16. The interface is busy
/// with the read that fetches the register, so bit 6 reads as one; bit 7 is
/// the 74S74 at UBCYC 0B08, which `-RESET ERR` --- a write of `766044` ---
/// clocks from data bit 7. MIT's `unaddr.text` says write-through mode "is
/// turned on by bit 7 (200) in the bus interface's Error Status register".
pub mod error_status {
    pub const NOT_FREE: u16 = 0o100;
    pub const WRITE_THROUGH: u16 = 0o200;
}

/// Which side of the bus an address is on, and whether anything lives
/// there, on a backplane with no color TV on it.
///
/// [`decode_with`] is the one that takes the second display board into
/// account; this is it with none, which is every machine `--color-tv` did
/// not fit one to.
pub fn decode(phys: u32, memory_words: usize) -> Responder {
    decode_with(phys, memory_words, false)
}

/// Which side of the bus an address is on, and whether anything lives
/// there.  `color_tv` is whether the second display board, the color TV
/// at `17200000` and `17377750`, is on the backplane.
///
/// **The color ranges answer only when the board is fitted.**
/// `COLOR-EXISTS-P` in `sys/window/color.lisp` is how System 100 finds out
/// whether a machine has one: it writes 1 into the first buffer word with
/// the error stop off and reads it back, and a machine with no board there
/// has to give it the NXM.
pub fn decode_with(phys: u32, memory_words: usize, color_tv: bool) -> Responder {
    decode_for(phys, memory_words, color_tv, tv::BUFFER_WORDS, 0xff)
}

/// [`decode_with`] with the main display's buffer `tv_words` long, and
/// `tv_regs` its control registers that answer, a bit each
/// (`Tv::control_registers`). QUUX does not decode by this, but by
/// [`decode_quux_13`].
/// **QUUX's decode** (contracts Q5, Q7 and Q13; G1 §3.2, G2 §3-§4): 28-bit
/// physical word addresses. The register page,
/// [`crate::machine::REGISTER_PAGE_13`], a device; the frame buffer window
/// from [`crate::machine::WINDOW_13`], `tv_words` of it, on the memory bus
/// with main memory; main memory from 0 up to `memory_words`, below the
/// window; and nothing else. An access to nothing fails at once. The
/// CADR's addresses from `17000000` up, its Unibus window among them, are
/// main memory's when there is that much of it.
pub fn decode_quux_13(phys: u32, memory_words: usize, tv_words: u32) -> Responder {
    use crate::machine::{REGISTER_PAGE_13, WINDOW_13};
    if phys & !0o377 == REGISTER_PAGE_13 {
        Responder::Device
    } else if phys.wrapping_sub(WINDOW_13) < tv_words
        || (phys < WINDOW_13 && (phys as usize) < memory_words)
    {
        Responder::Memory(0)
    } else {
        Responder::NoXbus
    }
}

/// **Revision 14's decode** (contract G3 revision 14, A14.1) of a 29-bit
/// bus address ([`crate::tlb::bus_address`]): with `<28>` clear, main
/// memory up to `memory_words` and nothing past it; with `<28>` set, the
/// device window at `<27:0>`: the register page at `1777777400`, a device;
/// frame buffer 0 from the window's base, `tv_words` of it, on the memory
/// bus with main memory; and nothing else, the reserved slices and pages
/// included. A memory's window never reaches here: every reference to it
/// faults.
pub fn decode_quux_14(bus: u32, memory_words: usize, tv_words: u32) -> Responder {
    use crate::tlb::{DEVICE, REGISTER_PAGE_BUS};
    if bus & !0o377 == REGISTER_PAGE_BUS {
        Responder::Device
    } else if bus & DEVICE != 0 && (bus & !DEVICE) < tv_words
        || bus & DEVICE == 0 && (bus as usize) < memory_words
    {
        Responder::Memory(0)
    } else {
        Responder::NoXbus
    }
}

pub fn decode_for(
    phys: u32,
    memory_words: usize,
    color_tv: bool,
    tv_words: u32,
    tv_regs: u8,
) -> Responder {
    let page = (phys >> 8) & 0o37777;
    if page >= 0o37000 {
        // On the Unibus: the diagnostic bus's sixteen registers, of which
        // `0o766012` turns the boot PROM off, and the I/O board with its
        // Chaosnet interface.
        let Some(u) = unibus_address(phys) else { return Responder::NoUnibus };
        if register(u).is_some() {
            Responder::Interface
        } else if let Some(k) = debug_register(u) {
            Responder::Debug(k)
        } else if ioboard::answers(u, false).is_some() || ioboard::answers(u, true).is_some() {
            Responder::Unibus(u)
        } else {
            Responder::NoUnibus
        }
    } else if page >= 0o36000 {
        // The frame buffer is the bottom of Xbus I/O space; the display's
        // mode register and the disk's four share the top page with it.
        let main_tv = phys.wrapping_sub(tv::BUFFER) < tv_words
            || tv::NORMAL_TV.control_register(phys).is_some_and(|r| tv_regs >> r & 1 != 0);
        let built = main_tv
            || (color_tv && tv::COLOR_TV.answers(phys))
            || disk_controller::register(phys).is_some();
        if built { Responder::Device } else { Responder::NoXbus }
    } else if (phys as usize) < memory_words {
        Responder::Memory((phys >> 16) as u8)
    } else {
        Responder::NoXbus
    }
}

/// Where a bus cycle has got to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    /// No cycle. `-MEMRQ` is high.
    Idle,
    /// `-MEMRQ` is low and the priority logic has not sampled it yet.
    ///
    /// "MEMRQ is synchronous; it starts in the middle of a cycle ... but the
    /// bus interface only looks at it towards the end of the cycle.  It runs
    /// it ... into its priority logic, and clocks into flip flops the
    /// decision as to whether the cpu gets this cycle or not.  XBUSRQ is
    /// derived from MEMRQ, starting at this clock."
    Requested,
    /// A Unibus cycle, in the arbitration. `-MEMGRANT` stays high through
    /// it: the board is the Unibus arbiter in local mode and its own request
    /// goes through the priority PROM, the grant chain and `SACK` like any
    /// other master's, then through the request synchronizer on RQSYNC.
    /// `stage` is how far the master clock has taken it, and `sack_at` when
    /// the grant comes back as `SACK`, which the arbiter sees at the edge
    /// after. See [`Busint::mclk_edge`].
    Arbitrating { stage: u8, sack_at: u64 },
    /// The processor has the bus: `-MEMGRANT` is low and the cycle is
    /// running. `ack` is when `-LMACK` comes, from `-XBUS.ACK` or `-UB SSYN`
    /// or the timeout; `loadmd` when `-LOADMD` rises for a read; `answered`
    /// when the slave gives or takes the word, which is when a clock on the
    /// I/O board is read or a register on the board is written.
    Granted { ack: u64, loadmd: u64, answered: u64, timed_out: bool },
    /// `-MEMACK` is up and the word is on the bus. It stays until the CPU
    /// drops `-MEMRQ`: "The responding device then asserts -XBUS.ACK, which
    /// remains asserted until the -XBUS.RQ signal is removed by the master."
    Acked { at: u64, loadmd: u64, answered: u64, timed_out: bool },
}

/// What the debugger's DBGOUT puts on the cable, as its own cpu's cycles
/// into the debug block make it: a request, or the request lifted without
/// an acknowledgement when the cycle timed out.  [`Busint`] knows the
/// times and the strobe; the word and the direction are the cpu's, and
/// `rtl` fills them into the [`DebugRequest`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DebugOut {
    /// `-DEBUG OUT REQ` down at `at` with `DEBUG OUT A<1:0>` = `strobe`.
    Request { at: u64, strobe: u8 },
    /// `-DEBUG OUT REQ` up at `at` with no acknowledgement had: the
    /// debugger's `NXM TIMEOUT`, and `-UB MSYN` dropping [`UNIBUS_STROBE_NS`]
    /// after it.
    Release { at: u64 },
}

/// An event on the debug cable, as the debugger's machine makes it and the
/// transport carries it to the debuggee's DBGIN: [`DebugOut`] with the
/// cpu's direction and word filled in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CableEvent {
    /// `-DEBUG IN REQ` goes down at `at` with the wires in `request`; the
    /// debuggee answers it with [`crate::rtl::Rtl::debug_ack`].
    Request { at: u64, request: DebugRequest },
    /// `-DEBUG IN REQ` goes up at `at` with no acknowledgement had.
    Release { at: u64 },
}

/// Where the debug master's request has got to on the debuggee: the DBGIN
/// page and its place on UBMAST.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Debug {
    /// `-DEBUG IN REQ` is high.
    Idle,
    /// One of the three register strobes is down: acknowledged at once ---
    /// `DEBUG ACK` is `(DBUB MASTER AND SSYN T0) OR NAND(-DB ADR1 CLK, -DB
    /// ADR0 CLK, -DB READ STATUS)`, the 74S08, 74S10 and 74S32 at DBGIN
    /// 0A12, 0A14 and 0A09 --- and lifted at `until`, when the latch it
    /// strobes takes `DBD`.
    Strobe { until: u64 },
    /// `-DB NEED UB` is down, and with it `-DB BUS REQ` at UBMAST 0D06 and
    /// `NPR` on the Unibus through the 74S00 at 0D05 and the 8838 at
    /// UPRIOR 0F15; the synchronizer at UPRIOR 0D10 has not registered it.
    NeedUb,
    /// `NPRD` registered; waiting for the priority PROM's grant.
    Nprd,
    /// `NPG1 IN` came at an edge and the 74LS74 at UBMAST 0D02 took it,
    /// `DB UB GRANTED`; `DB UB SELECTED` and `-UB SACK` from `sack_at`.
    Granted { sack_at: u64 },
    /// `SACKD` has withdrawn the grant and `SACK` is still up; `-DB UB SET
    /// MASTER` is `NAND(BUS READY, DB UB SELECTED)` at 0E01 and `BUS
    /// READY` waits for the processor to let go of `-UB BBSY`.
    Selected,
    /// `DBUB MASTER` since `since`: the address on the Unibus, `-UB MSYN`
    /// at `msyn`, the slave's `-UB SSYN` and with it `DEBUG ACK` at `ack`
    /// --- `u64::MAX` for an address nothing answers, since no timeout
    /// runs for this master --- the word given or taken at `answered`,
    /// and the request lifted at `until`, `-UB MSYN` with it.  `xbus` is
    /// how far a mapped access's Xbus cycle has got: 0 nothing yet, 1
    /// `UBXRQS` registered, 2 granted and the answer known.
    Master { since: u64, msyn: u64, ack: u64, answered: u64, until: u64, xbus: u8 },
    /// The request lifted; `DBUB MASTER` clears at `until`,
    /// [`DEBUG_RELEASE_NS`] on, and `-UB BBSY` with it.
    Releasing { until: u64 },
}

/// The memory bus between the CPU and the bus interface.
#[derive(Clone)]
pub struct Busint {
    state: State,
    write: bool,
    /// The memory boards' timing, one twin a board, since each board's
    /// refresh waits only for its own cycles; see [`MemoryBoard`].
    memory: Vec<MemoryBoard>,
    /// The I/O board's timing; see [`IoBoardTiming`].
    pub io: IoBoardTiming,
    /// The board the cycle in flight is on, if it is on one.
    board: Option<u8>,
    /// How long a device takes to answer, from `-XBUS.RQ` to `-XBUS.ACK` or
    /// from `-UB MSYN` to `-UB SSYN`. See [`IDEAL_DEVICE_NS`].
    pub device_ns: u64,
    /// `LMUB MASTER` at UBMAST 0D04: the board keeps the Unibus once it has
    /// had it. Its clear is `NAND(-LM NEED UB, SACK IN OR -LOCAL ENABLE)`,
    /// so in local mode --- where this board is the arbiter --- it lets go
    /// only when another master acknowledges a grant while the processor
    /// has no cycle of its own up. The one other master is the debug
    /// cable's, [`Debug`](enum@Debug); every Unibus cycle after the first skips the
    /// arbitration until it has been through.
    unibus_master: bool,
    /// When the processor's last cycle let `-MEMRQ` rise, if a debug master
    /// may be waiting on it: [`MFINISHD_NS`] after its acknowledgement.
    memrq_up_at: Option<u64>,
    /// When the debug master's `SACK` last went up: a rise of `-MEMRQ`
    /// before it clears nothing.
    debug_sack_at: u64,
    /// Whether the processor's `-UB MSYN` was down as this edge came: a
    /// cycle granted at this very edge has its `MSYN` still to come.
    msyn_down_before_edge: bool,
    /// `LM NEED UB`, the 74S74 at REQUB 0B10: `-MEMRQ` for a Unibus
    /// address, registered at the master clock.  What keeps `LMUB MASTER`
    /// from clearing while a cycle of the processor's is up.
    lm_need_ub: bool,
    /// Master clock edges before the priority PROM may grant again after a
    /// `SACK` dropped: `SACKD` at UPRIOR 0D10 holds `-CLEAR GRANT` on the
    /// grant register at 0D07 until the edge that samples `SACK` low, and
    /// the register loads at the one after.
    grant_hold: u8,
    /// The debug master on DBGIN, and the request it is serving.
    debug: Debug,
    debug_request: Option<DebugRequest>,
    /// When the debugger put the request on the cable.
    debug_requested_at: u64,
    /// When the processor's cycle in flight was granted: where the timeout
    /// counter's oscillator started.  See [`NXM_VCO_NS`].
    granted_at: u64,
    /// What answers the cycle, decoded from the latched address when the
    /// request came.
    debug_responder: Responder,
    /// The two 74LS374s at DBGIN 0A18 and 0A19, `UAO<16:1>`, as
    /// [`DEBUG_ADDRESS`] loads them.
    debug_address: u16,
    /// The 25LS2519 at DBGIN 0A16, as [`DEBUG_MODIFIER`] loads it:
    /// [`debug_modifier`].  Cleared by `-DEBUG RESET`, which in local mode
    /// is `-LM POWER RESET` alone --- the 74S08 at DBGIN 0A12 ANDs it with
    /// `NAND(-LOCAL ENABLE, UNIBUS INIT IN)`, and `-LOCAL ENABLE` is low.
    debug_modifier: u16,
    /// When `DBUB MASTER` last cleared, for the processor waiting on
    /// `BUS READY` behind it.
    debug_freed_at: u64,
    /// When `DEBUG ACK` rose for the last request, and when its slave gave
    /// or took the word; `u64::MAX` until they do.  Kept past the request's
    /// end, since a strobe's whole life fits inside a microcycle.
    debug_ack: u64,
    debug_answered: u64,

    // --- DBGOUT: this machine as the debugger ---
    /// A debug cable is plugged into DBGOUT, with another machine's DBGIN
    /// on it.  Without one, `DEBUG OUT ACK` is the SIP's pull-up and a
    /// cycle into the debug block is acknowledged the instant it selects.
    debug_cable: bool,
    /// The event on the cable this side has made and the far side has not
    /// yet been given: [`DebugOut`].
    debug_out: Option<DebugOut>,
    /// A request is on the cable and the far machine has not answered.
    debug_out_pending: bool,
    /// When the debugger's interface gives up on it: [`debug_timeout_at`]
    /// the grant, where `INT BUSY` let the counter go.
    debug_out_timeout_at: u64,
    /// The earliest instant at which a master clock edge changes any twin,
    /// [`MemoryBoard::next_change`] over all of them: an edge before it
    /// leaves the twins alone. Clocking all thirty-two at every edge, when
    /// one moves a few times in twelve microseconds, was a quarter of an
    /// `rtl` run. Never later than the truth: whatever moves a twin lowers
    /// it to that twin's, and the edge that reaches it recomputes it.
    memory_next: u64,
    /// Whose time the interface keeps, its memory boards' and I/O board's
    /// twins with it. Not in a checkpoint: the engine that owns it says,
    /// [`Busint::keep_timing_model`].
    timing: TimingModel,
    /// QUUX's memory cache, if the machine has one ([`crate::cache`]).
    cache: Option<crate::cache::Cache>,
    /// The physical address of the cycle requested, for the cache.
    addr: u32,
    /// The cycle running hit the cache: acknowledged with no bus cycle.
    cached: bool,
    /// When the memory board finishes the write in the cache's write
    /// buffer, and the buffer is free again.
    buffer_free_at: u64,
    /// QUUX's main memory's timing, in place of the boards' twins
    /// ([`crate::cache::MemoryTiming`]), and when it is free again.
    memory_timing: Option<crate::cache::MemoryTiming>,
    memory_free_at: u64,
}

impl Default for Busint {
    fn default() -> Self {
        Busint::new(MEMORY_BOARDS)
    }
}

impl Busint {
    /// The interface with `boards` memory boards' twins on its Xbus: as
    /// many as the machine has, [`crate::machine::Machine::memory_boards`].
    pub fn new(boards: usize) -> Busint {
        Busint::with_timing_model(boards, TimingModel::Cadr)
    }

    /// The interface at power-on on `model`'s time, [`TimingModel`].
    pub fn with_timing_model(boards: usize, model: TimingModel) -> Busint {
        Busint {
            state: State::Idle,
            write: false,
            memory: vec![MemoryBoard::with_timing_model(model); boards],
            io: IoBoardTiming::with_timing_model(model),
            board: None,
            device_ns: IDEAL_DEVICE_NS,
            unibus_master: false,
            memrq_up_at: None,
            debug_sack_at: u64::MAX,
            msyn_down_before_edge: false,
            lm_need_ub: false,
            grant_hold: 0,
            debug: Debug::Idle,
            debug_request: None,
            debug_requested_at: 0,
            granted_at: 0,
            debug_responder: Responder::NoUnibus,
            debug_address: 0,
            debug_modifier: 0,
            debug_freed_at: 0,
            debug_ack: u64::MAX,
            debug_answered: u64::MAX,
            debug_cable: false,
            debug_out: None,
            debug_out_pending: false,
            debug_out_timeout_at: u64::MAX,
            memory_next: MemoryBoard::with_timing_model(model).next_change(),
            timing: model,
            cache: None,
            addr: 0,
            cached: false,
            buffer_free_at: 0,
            memory_timing: None,
            memory_free_at: 0,
        }
    }

    /// Fits QUUX's memory cache, or takes it out.
    pub fn set_cache(&mut self, config: Option<crate::cache::CacheConfig>) {
        self.cache = config.map(crate::cache::Cache::new);
    }

    /// QUUX's memory timing, if it is fitted.
    pub fn memory_timing(&self) -> Option<crate::cache::MemoryTiming> {
        self.memory_timing
    }

    /// Times main memory as QUUX's own, or as the CADR's boards (`None`).
    pub fn set_memory_timing(&mut self, timing: Option<crate::cache::MemoryTiming>) {
        self.memory_timing = timing;
    }

    /// The memory cache, if there is one.
    pub fn cache(&self) -> Option<&crate::cache::Cache> {
        self.cache.as_ref()
    }

    /// Drops everything the cache holds: main memory was written by
    /// something other than the processor.
    pub fn invalidate_cache(&mut self) {
        if let Some(c) = self.cache.as_mut() {
            c.invalidate();
        }
    }

    /// Whose time the interface keeps.
    pub fn timing_model(&self) -> TimingModel {
        self.timing
    }

    /// Says whose time the interface and its twins keep, without starting
    /// them over: for a checkpoint read back, whose instants are already on
    /// that time.
    pub fn keep_timing_model(&mut self, model: TimingModel) {
        self.timing = model;
        self.io.timing = model;
        for m in &mut self.memory {
            m.timing = model;
        }
    }
}

/// What the CPU learns when `-MEMACK` rises.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ack {
    /// The cycle timed out: nothing answered. The word is meaningless and one
    /// of the two NXM bits belongs in the bus error register.
    pub timed_out: bool,
    /// Which bus timed out, for the error bit.
    pub responder: Responder,
    /// When `-MEMACK` rose.
    pub at: u64,
    /// When the word is stable in `MD`: an Xbus word lands with the
    /// acknowledgement, which [`XBUS_ACK_NS`] has already deskewed, and a
    /// Unibus word [`UNIBUS_STROBE_NS`] after `-UB SSYN`, before it.
    pub loadmd_at: u64,
    /// When the slave answered, which is when the word was made: a read of
    /// the I/O board's microsecond clock is the microsecond then, not at
    /// the acknowledgement 150 ns on. See [`Busint::answered_at`].
    pub answered_at: u64,
    /// The cache answered it: no bus cycle ran, and the word needs none of
    /// the bus's time to reach the data paths.
    pub cached: bool,
}

impl Busint {
    /// The CPU raises `-MEMRQ`. On the board this is `MEMSTART AND VMAOK`
    /// through the 9S42 at VCTL1 1E25.
    pub fn request(&mut self, write: bool) {
        self.request_at(write, 0);
    }

    /// [`Busint::request`], at physical address `phys`, which a cache in
    /// front of main memory looks up.
    pub fn request_at(&mut self, write: bool, phys: u32) {
        debug_assert_eq!(self.state, State::Idle, "a cycle is already running");
        self.state = State::Requested;
        self.write = write;
        self.addr = phys;
        self.cached = false;
        // Until the priority logic says where this one goes, its release
        // is nobody's: a Unibus cycle after a memory cycle held the last
        // board's refresh at 2,545,496 in the band.
        self.board = None;
    }

    /// The twin of memory board `k`, for holding it to the netlist board.
    pub fn memory_board(&self, k: usize) -> &MemoryBoard {
        &self.memory[k]
    }

    /// All the twins, for a far end that runs main memory as twins too
    /// (`chip` with `--main-memory model`) and is handed this engine's on
    /// resume, as it is handed the machine.
    pub fn memory_boards(&self) -> &[MemoryBoard] {
        &self.memory
    }

    /// The processor will lift `-MEMRQ`, and the interface `-XBUS RQ`, at
    /// `at`: the board the cycle is on learns when it is idle again.
    pub fn released(&mut self, at: u64) {
        if let Some(k) = self.board {
            self.memory[k as usize].released(at);
            self.memory_next = self.memory_next.min(self.memory[k as usize].next_change());
        }
    }

    /// The processor's Unibus reset going on or off: every memory board
    /// is held by it. See [`MemoryBoard::unibus_reset`].
    pub fn unibus_reset(&mut self, now: u64, on: bool) {
        for b in &mut self.memory {
            b.unibus_reset(now, on);
        }
        self.memory_next =
            self.memory.iter().map(MemoryBoard::next_change).min().unwrap_or(u64::MAX);
    }

    /// A master clock edge. The priority logic samples `-MEMRQ` here, which
    /// is why `MCLK7` is one of the signals the CPU sends across the cables.
    ///
    /// An Xbus cycle starts at this edge: `LMX GRANT` is registered from the
    /// request and `-MEMGRANT` goes low with it. A Unibus cycle is
    /// arbitrated first, and each step is a register on the master clock or
    /// a delay line, read off the netlist bus interface and measured in
    /// `tests/busint_netlist.rs`:
    ///
    /// 1. `LM NEED UB` at REQU 0B10 takes the request, and `-LM BUS REQ`
    ///    puts `NPR` on the Unibus.
    /// 2. The synchronizer at UPRIOR 0D10 registers `NPRD`, and the
    ///    priority PROM grants.
    /// 3. The grant register at 0D07 puts `NPG` out on the chain, which in
    ///    local mode comes straight back to the board. Two sections of
    ///    delay line on UBMAST later --- [`UNIBUS_SELECT_NS`] --- the
    ///    board is `LM UB SELECTED` and answers `SACK`.
    /// 4. At the first edge after that, `SACKD` withdraws the grant, the
    ///    bus is ready, and `LMUB MASTER` sets.
    /// 5. The request synchronizer at RQSYNC 0A08 registers `LMUBRQS`.
    /// 6. The grant register at 0A06 sets `LMUB GRANT`: `-MEMGRANT` goes low
    ///    and the address goes out on the Unibus. `-UB MSYN` follows
    ///    [`UNIBUS_ADDRESS_NS`] later, the slave answers `-UB SSYN`, and
    ///    `-LMACK` comes [`UNIBUS_ACK_NS`] after that.
    ///
    /// At the 220 ns the machine boots in that is 1.8 microseconds for a
    /// write of the mode register, against 300 for an Xbus write. The board
    /// then keeps the Unibus --- `LMUB MASTER` stays set --- and every
    /// Unibus cycle after the first starts at step 5.
    pub fn mclk_edge(&mut self, now: u64, responder: Responder) {
        self.mclk_edge_inner(now, responder);
    }

    fn mclk_edge_inner(&mut self, now: u64, responder: Responder) {
        self.msyn_down_before_edge =
            matches!(self.state, State::Granted { .. } | State::Acked { .. });
        // The Xbus clock is this clock, and the memory boards' refresh
        // synchronizers run on it --- when an edge can move one at all.
        if now >= self.memory_next {
            for b in &mut self.memory {
                b.xbus_clock(now);
            }
            self.memory_next =
                self.memory.iter().map(MemoryBoard::next_change).min().unwrap_or(u64::MAX);
        }
        // The debug master's asynchronous events up to this edge, and the
        // priority PROM's hold after a `SACK`.
        self.debug_advance(now);
        self.grant_hold = self.grant_hold.saturating_sub(1);
        // `LM NEED UB` at REQUB 0B10 registers `-MEMRQ` here: a request
        // made at this edge is sampled at the next.
        self.lm_need_ub = !matches!(self.state, State::Idle | State::Requested);
        // `LMUB MASTER`'s clear at UBMAST 0D05: `-LM NEED UB` with `SACK
        // IN`.  The debug master's `SACK` is the only other one on the bus,
        // and it takes the Unibus from an idle processor by it.
        if self.unibus_master && !self.lm_need_ub && self.debug_sack_up(now) {
            self.unibus_master = false;
        }
        match self.state {
            State::Requested if responder.on_unibus() => {
                // With `LMUB MASTER` still set from the last cycle, `LMUB RQ`
                // goes straight to the request synchronizer.
                let stage = if self.unibus_master { 4 } else { 1 };
                self.state = State::Arbitrating { stage, sack_at: u64::MAX };
            }
            State::Requested => {
                let answers = matches!(responder, Responder::Memory(_) | Responder::Device);
                // An Xbus slave gives or takes the word at `-XBUS RQ`; a
                // write is acknowledged at once and a read deskewed. Memory
                // answers when the board's own clock says.
                let rq = now + SETUP_NS;
                self.board = None;
                // A read the cache holds is answered from it, with no bus
                // cycle and none of the bus's setup and deskew; a miss
                // fills its line and runs the board's cycle as before.
                let hit = matches!(responder, Responder::Memory(_))
                    && !self.write
                    && self.cache.as_mut().is_some_and(|c| c.read(self.addr));
                if hit {
                    let at = now + self.cache.as_ref().map_or(0, |c| c.config.hit_ns);
                    self.cached = true;
                    self.state =
                        State::Granted { ack: at, loadmd: at, answered: at, timed_out: false };
                    return;
                }
                // QUUX's own memory: a fixed time from the request, one
                // operation at a time, off the Xbus; a buffered write is
                // acknowledged after the hit time and runs behind.
                if let (Responder::Memory(_), Some(t)) = (responder, self.memory_timing) {
                    let start = now.max(self.memory_free_at);
                    let done = start + if self.write { t.write_ns } else { t.read_ns };
                    self.memory_free_at = done;
                    let buffered =
                        self.write && self.cache.as_ref().is_some_and(|c| c.config.write_buffer);
                    let at = if buffered {
                        let hit = self.cache.as_ref().map_or(0, |c| c.config.hit_ns);
                        let at = (now + hit).max(self.buffer_free_at);
                        self.buffer_free_at = done;
                        at
                    } else {
                        done
                    };
                    self.cached = true;
                    self.state =
                        State::Granted { ack: at, loadmd: at, answered: at, timed_out: false };
                    return;
                }
                // A write into the write buffer: acknowledged after the hit
                // time, or when the buffer's last write is done, and the
                // board runs it behind the processor. The word is memory's
                // from the acknowledgement.
                if let (Responder::Memory(k), true, Some(c)) =
                    (responder, self.write, self.cache.as_ref())
                    && c.config.write_buffer
                {
                    let at = (now + c.config.hit_ns).max(self.buffer_free_at);
                    let done = self.memory[k as usize].request(rq.max(at));
                    self.memory_next = self.memory_next.min(self.memory[k as usize].next_change());
                    self.board = Some(k);
                    self.buffer_free_at = done;
                    self.cached = true;
                    self.state =
                        State::Granted { ack: at, loadmd: at, answered: at, timed_out: false };
                    return;
                }
                let answered = match responder {
                    Responder::Memory(k) => {
                        self.board = Some(k);
                        let at = self.memory[k as usize].request(rq);
                        self.memory_next =
                            self.memory_next.min(self.memory[k as usize].next_change());
                        at
                    }
                    _ => rq + self.device_ns,
                };
                let ack = if !answers {
                    self.timeout(now)
                } else if self.write {
                    answered
                } else {
                    answered + XBUS_ACK_NS
                };
                self.state = State::Granted { ack, loadmd: ack, answered, timed_out: !answers };
            }
            State::Arbitrating { stage, sack_at } => {
                self.state = match stage {
                    1 => State::Arbitrating { stage: 2, sack_at },
                    // The PROM grants when no grant is out and no `SACK` is
                    // up or was at the last edge; the debug master, if it
                    // is asking too, is first on the chain and takes it.
                    2 if self.grant_hold == 0
                        && !self.debug_grant_out()
                        && !self.debug_asking() =>
                    {
                        State::Arbitrating { stage: 3, sack_at: now + UNIBUS_SELECT_NS }
                    }
                    2 => self.state,
                    // `SACKD` withdraws the grant.  `-LM UB SET MASTER` is
                    // `NAND(BUS READY, LM UB SELECTED)` at UBMAST 0E01, and
                    // `BUS READY` is `NOR(BBSY IN, SSYN IN, NPG1 IN)` at
                    // 0C03: the debug master, if it has the bus, holds
                    // `-UB BBSY` and this waits behind it.
                    3 if now > sack_at => {
                        if self.debug_holds_bbsy(now) {
                            State::Arbitrating { stage: 7, sack_at }
                        } else {
                            self.unibus_master = true;
                            self.sack_off(now, now);
                            State::Arbitrating { stage: 4, sack_at }
                        }
                    }
                    3 => self.state,
                    4 => State::Arbitrating { stage: 5, sack_at },
                    // Selected and waiting for the debug master to let go.
                    // `LMUB MASTER` sets the instant `-UB BBSY` lifts, and
                    // `LMUB RQ` with it; the request synchronizer at RQSYNC
                    // 0A08 registers it at the first edge after --- this
                    // one, if the bus came free before it.
                    7 if !self.debug_holds_bbsy(now) => {
                        self.unibus_master = true;
                        let freed = self.debug_freed_at;
                        self.sack_off(now, freed);
                        State::Arbitrating { stage: if freed < now { 5 } else { 4 }, sack_at }
                    }
                    7 => self.state,
                    _ => {
                        let msyn = now + UNIBUS_ADDRESS_NS;
                        // A Unibus slave gives the word with `-UB SSYN` and
                        // takes it at `-UB MSYN`; the board's own registers
                        // take one at the register strobe.
                        let (ssyn, answered, timed_out) = match responder {
                            Responder::Interface => (
                                msyn + DIAGNOSTIC_NS,
                                if self.write {
                                    msyn + REGISTER_STROBE_NS
                                } else {
                                    msyn + DIAGNOSTIC_NS
                                },
                                false,
                            ),
                            Responder::Unibus(u) => match ioboard::answers(u, self.write) {
                                Some(r) => {
                                    let ssyn = self.io.answer(r, self.write, msyn);
                                    // The counter's low half is the count as
                                    // it stood at `-MSYN`: the board latches
                                    // it on the way to answering, before the
                                    // edge that answers has counted. The
                                    // band's first read of it, at 2,087,406,
                                    // read one less on the netlist than a
                                    // count taken at the answer.
                                    let made = if r == ioboard::USEC_LOW { msyn } else { ssyn };
                                    (ssyn, made, false)
                                }
                                // A write the decoder takes nowhere --- the
                                // microsecond counter's halves, the Chaosnet
                                // buffers --- is never acknowledged.
                                None => (self.timeout(now), self.timeout(now), true),
                            },
                            // The debug block: `-UB SSYN` is the other
                            // machine's `DEBUG ACK` --- `DEBUG SSYN` is
                            // `DEBUG OUT ACK AND SELECT DEBUG` at DBGOUT
                            // 0A12, one of the five sources the 74S260 at
                            // REQU 0E06 clocks `SSYN OUT` from --- and comes
                            // when it comes: [`Busint::debug_out_answer`],
                            // or [`Busint::poll`]'s timeout.  With no cable
                            // the pull-up answers at `-UB MSYN`.
                            Responder::Debug(strobe) => {
                                if self.debug_cable {
                                    self.debug_out = Some(DebugOut::Request {
                                        at: msyn + DEBUG_OUT_REQUEST_NS,
                                        strobe,
                                    });
                                    self.debug_out_pending = true;
                                    self.debug_out_timeout_at =
                                        self.timing.free_running(debug_timeout_at(now));
                                    (u64::MAX, u64::MAX, false)
                                } else {
                                    (msyn, msyn, false)
                                }
                            }
                            _ => (self.timeout(now), self.timeout(now), true),
                        };
                        State::Granted {
                            ack: ssyn.saturating_add(UNIBUS_ACK_NS),
                            loadmd: ssyn.saturating_add(UNIBUS_STROBE_NS),
                            answered,
                            timed_out,
                        }
                    }
                };
            }
            _ => {}
        }
        self.debug_edge(now);
    }

    /// When a cycle nothing answers is given up on: [`nxm_timeout_at`] the
    /// grant, or never while the debugger holds
    /// [`debug_modifier::TIMEOUT_INHIBIT`] --- the counter at REQTIM 0B01
    /// is held clear.
    fn timeout(&mut self, now: u64) -> u64 {
        self.granted_at = now;
        if self.debug_modifier & debug_modifier::TIMEOUT_INHIBIT != 0 {
            u64::MAX
        } else {
            self.timing.free_running(nxm_timeout_at(now))
        }
    }

    /// A `SACK` dropped at `at`, this edge being `now`: `SACKD` samples it
    /// low at the first edge strictly after, `-CLEAR GRANT` lifts then, and
    /// the grant register loads at the edge after that.
    fn sack_off(&mut self, now: u64, at: u64) {
        self.grant_hold = if at < now { 1 } else { 2 };
    }

    // --- DBGOUT: this machine as the debugger ---

    /// A debug cable is plugged into this machine's DBGOUT: a cycle into
    /// the debug block waits for the other machine's `DEBUG ACK` instead of
    /// the pull-up.
    pub fn attach_debug_cable(&mut self) {
        self.debug_cable = true;
    }

    /// Takes the event this side has put on the cable, if one is waiting to
    /// be carried to the other machine.
    pub fn debug_out_take(&mut self) -> Option<DebugOut> {
        self.debug_out.take()
    }

    /// A request is out on the cable, unanswered.
    pub fn debug_out_pending(&self) -> bool {
        self.debug_out_pending
    }

    /// The other machine's `DEBUG ACK` rose at `ack_at`: `-UB SSYN` here,
    /// the word strobed [`UNIBUS_STROBE_NS`] on and `-LMACK`
    /// [`UNIBUS_ACK_NS`] on, as for any Unibus slave.  An acknowledgement
    /// after this side's timeout is nothing: `SELECT DEBUG` is down.
    /// Returns whether it was taken.
    pub fn debug_out_answer(&mut self, ack_at: u64) -> bool {
        if !self.debug_out_pending || ack_at >= self.debug_out_timeout_at {
            return false;
        }
        let State::Granted { ack: u64::MAX, .. } = self.state else { return false };
        self.debug_out_pending = false;
        self.state = State::Granted {
            ack: ack_at + UNIBUS_ACK_NS,
            loadmd: ack_at + UNIBUS_STROBE_NS,
            answered: ack_at,
            timed_out: false,
        };
        true
    }

    /// What this side promises the other about the cable, at `now`: the
    /// earliest instant a new request or a release can appear on it.  With
    /// a request out, the release at the timeout; otherwise `LMUB GRANT`
    /// at an edge after `now`, `-UB MSYN` [`UNIBUS_ADDRESS_NS`] on and the
    /// request [`DEBUG_OUT_REQUEST_NS`] after that.  The other machine may
    /// run up to this and no further.
    pub fn debug_out_promise(&self, now: u64) -> u64 {
        if self.debug_out_pending {
            self.debug_out_timeout_at.saturating_add(UNIBUS_STROBE_NS)
        } else if matches!(self.state, State::Idle | State::Requested) {
            now + IDLE_OUT_PROMISE_NS
        } else {
            // A cycle in the arbitration is announced at the edge that
            // grants it, which is a microcycle away at the least --- `now`
            // is a boundary whose edge has been taken --- and its request
            // follows `-UB MSYN` [`UNIBUS_ADDRESS_NS`] and
            // [`DEBUG_OUT_REQUEST_NS`] after that edge.  Promising it from
            // `now` alone, as this did, left two machines cabled both ways
            // each promising the other less than a cycle, and neither
            // could move.
            now + crate::clock::Speed::Fast.cycle_ns(false) as u64
                + UNIBUS_ADDRESS_NS
                + DEBUG_OUT_REQUEST_NS
        }
    }

    /// What the debuggee's side promises the debugger, at `now`: the
    /// earliest instant `DEBUG ACK` can rise for the request on the cable.
    /// A cycle still in the arbitration is at least `DBUB MASTER` at an
    /// edge after `now`, `-UB MSYN` and the quickest slave away; a master
    /// knows its acknowledgement; a strobe was acknowledged as it came, and
    /// an empty cable promises nothing to wait for.
    pub fn debug_in_promise(&self, now: u64) -> u64 {
        match self.debug {
            // `NPRD` registers at an edge after the request came, whether or
            // not this machine has run that far yet.
            Debug::NeedUb | Debug::Nprd | Debug::Granted { .. } | Debug::Selected => {
                now.max(self.debug_requested_at) + DEBUG_MSYN_NS + DIAGNOSTIC_NS
            }
            // A mapped access whose Xbus cycle is not yet granted: `UBX
            // GRANT` at an edge after `now`, the request `SETUP_NS` on ---
            // or, for a write into `MD`, the acknowledgement that soon.
            Debug::Master { ack: u64::MAX, xbus: 0 | 1, .. }
                if matches!(self.debug_responder, Responder::MapXbus(_) | Responder::MapMd) =>
            {
                now + SETUP_NS.min(UB_MD_ACK_NS)
            }
            Debug::Master { ack, .. } => ack,
            Debug::Idle | Debug::Strobe { .. } | Debug::Releasing { .. } => u64::MAX,
        }
    }

    // --- The debug master, DBGIN and its place on UBMAST ---

    /// The debugger puts a request on the cable at `at`: `-DEBUG IN REQ`
    /// down with the wires in `req`.  `responder` is what answers a cycle
    /// at the latched address, [`decode`] of [`unibus_physical`] of
    /// [`Busint::debug_unibus_address`]; the caller decodes, knowing the
    /// memory size.  One request at a time, as the debugger's own Unibus
    /// cycle is one at a time.
    pub fn debug_request(&mut self, at: u64, req: DebugRequest, responder: Responder) {
        // A request whose life ended before `at` --- a strobe lifted, a
        // master released --- is over, whether or not the machine has been
        // run to see it go.
        self.debug_advance(at);
        // Not a peer's to reach.  A request off a cable, in process or from
        // a stream, comes through `Rtl::try_debug_request`, which advances
        // this state to `at` the same way and refuses the request while
        // `debug_pending` says one is on the cable --- `debug` anything but
        // `Idle`, since `debug_request` is `Some` from the first request
        // on --- so what arrives here is idle already.  The assertion is
        // for a caller in this crate that skips that check.
        assert_eq!(self.debug, Debug::Idle, "a debug request is already on the cable at {at} ns");
        self.debug_request = Some(req);
        self.debug_requested_at = at;
        self.debug_responder = responder;
        self.debug_answered = u64::MAX;
        self.debug = if req.strobe == DEBUG_CYCLE {
            self.debug_ack = u64::MAX;
            Debug::NeedUb
        } else {
            self.debug_ack = at;
            Debug::Strobe { until: at.saturating_add(req.hold_ns) }
        };
    }

    /// The last request put on the cable, whether or not it is still there.
    pub fn debug_last_request(&self) -> Option<DebugRequest> {
        self.debug_request
    }

    /// The debugger lifts `-DEBUG IN REQ` at `at` without waiting for the
    /// acknowledgement: its own interface giving up on a cycle nothing
    /// answers, [`DEBUG_TIMEOUT_NS`] on.  A master's `-UB MSYN` drops at
    /// once and `DBUB MASTER` [`DEBUG_RELEASE_NS`] later; a request still
    /// in the arbitration is withdrawn --- `-DB RESET` at UBMAST 0B16 is
    /// `DB NEED UB AND -DBUB MASTER`, and drops to reset the two flops at
    /// 0D02; a strobe loads its latch.
    pub fn debug_release(&mut self, at: u64) {
        match self.debug {
            Debug::Idle | Debug::Releasing { .. } => {}
            Debug::Strobe { .. } => {
                self.debug_latch();
                self.debug = Debug::Idle;
            }
            Debug::Master { ack, answered, .. } => {
                // `-UB MSYN` drops with the request: an answer not yet
                // given never comes, a word not yet taken is not taken.
                if at < ack {
                    self.debug_ack = u64::MAX;
                }
                if at < answered {
                    self.debug_answered = u64::MAX;
                }
                self.debug_freed_at = at + DEBUG_RELEASE_NS;
                self.debug = Debug::Releasing { until: self.debug_freed_at };
            }
            _ => self.debug = Debug::Idle,
        }
    }

    /// The request on the cable, if one is.
    pub fn debug_pending(&self) -> Option<DebugRequest> {
        (self.debug != Debug::Idle).then_some(self.debug_request).flatten()
    }

    /// The 18-bit Unibus address the latches hold: `UAO<16:1>` from the
    /// two 74LS374s and `UAO17` from the modifier register, bit 0 zero.
    pub fn debug_unibus_address(&self) -> u32 {
        ((self.debug_modifier & debug_modifier::ADDRESS_17) as u32) << 17
            | (self.debug_address as u32) << 1
    }

    /// The modifier register as it stands, [`debug_modifier`].
    pub fn debug_modifier(&self) -> u16 {
        self.debug_modifier
    }

    /// When `DEBUG ACK` rose for the last request, if it has: at once for
    /// a register strobe, with `-UB SSYN` for a cycle.  A cycle at an
    /// address nothing answers never has one.
    pub fn debug_ack_at(&self) -> Option<u64> {
        (self.debug_ack != u64::MAX).then_some(self.debug_ack)
    }

    /// When the slave of the last debug cycle gives or takes the word, if
    /// the cycle has been granted: as [`Busint::answered_at`] for the
    /// processor's.
    pub fn debug_answered_at(&self) -> Option<u64> {
        (self.debug_answered != u64::MAX).then_some(self.debug_answered)
    }

    /// `DBUB MASTER`: the debug master has the debuggee's Unibus.
    pub fn debug_master(&self) -> bool {
        matches!(self.debug, Debug::Master { .. } | Debug::Releasing { .. })
    }

    /// When the register strobe on the cable lifts and latches, if one is
    /// on it: the instant the modifier's reset bit can change.
    pub fn debug_strobe_until(&self) -> Option<u64> {
        match self.debug {
            Debug::Strobe { until } => Some(until),
            _ => None,
        }
    }

    /// The debug master's asynchronous events up to `now`: a register
    /// strobe's trailing edge loading its latch, the request lifted after
    /// the acknowledgement, `DBUB MASTER` clearing after that.
    pub fn debug_advance(&mut self, now: u64) {
        // Nothing on the cable, nothing to do.  Every arm below asks for a
        // `Debug` other than `Idle` --- the block that follows acts on
        // `Selected` and `Granted`, the loop on `Strobe`, `Master` and
        // `Releasing` --- so from `Idle` this function reaches none of
        // them and returns having changed nothing.  Worth saying out loud
        // because the engines call it twice a microcycle and a machine
        // with no debugger on it is in `Idle` for every one of them: 400
        // million calls over 200,000,000 microcycles of the System 100
        // band, and `Debug::Idle` on all 400 million.
        //
        // The state and not `unibus_master`.  That flag is `LMUB MASTER`,
        // which is cleared only where the debug master's `SACK` takes the
        // Unibus away, so with no cable the processor becomes master once
        // and stays it: true on 99.7% of those calls, and a condition that
        // asks for it to be false is a condition that never fires.
        if matches!(self.debug, Debug::Idle) {
            return;
        }
        // The processor's cycle ending while the debug master waits with its
        // `SACK` up: `LM NEED UB`, the 74S74 at REQUB 0B10, is cleared the
        // instant `-MEMRQ` rises --- `LMNEEDUB (EARLY)` on its clear ---
        // and `NAND(-LM NEED UB, SACK IN)` at UBMAST 0D05 clears `LMUB
        // MASTER` and `DBUB MASTER` sets, all of it then and not at an edge.
        // **Measured on the board**: the processor's `-UB SSYN` at 1230 ns
        // after the debug request, `-MEMRQ` and `LMUB MASTER` at 1410, the
        // debug master's `-UB MSYN` at 1510 and its acknowledgement at 1760,
        // where waiting for the edge had made it 1870.
        if self.unibus_master
            && let Some(t) = self.memrq_up_at
            && t >= self.debug_sack_at
            && now >= t
        {
            let took = match self.debug {
                Debug::Selected => Some(t),
                Debug::Granted { .. } => Some(t + GRANT_WITHDRAW_NS),
                _ => None,
            };
            if let Some(took) = took {
                self.unibus_master = false;
                self.lm_need_ub = false;
                self.memrq_up_at = None;
                self.debug_set_master(took);
            }
        }
        // Every step whose instant has come, not one: a debuggee that has
        // stood still since it acknowledged --- on a stream, one whose
        // thread is late --- is asked again at an instant past the lift and
        // the release both, and must find the cable idle by then.
        // Each arm leaves `self.debug` a different variant from the one it
        // matched, so an arm taken is a step made and the arm that matches
        // nothing is the end of the run.  Saying that with `break` rather
        // than comparing the state against a copy of itself keeps a
        // 48-byte enum and its derived `PartialEq` off a path the engines
        // take twice a microcycle.
        loop {
            match self.debug {
                Debug::Strobe { until } if now >= until => {
                    self.debug_latch();
                    self.debug = Debug::Idle;
                }
                Debug::Master { until, .. } if now >= until => {
                    self.debug_freed_at = until + DEBUG_RELEASE_NS;
                    self.debug = Debug::Releasing { until: self.debug_freed_at };
                }
                Debug::Releasing { until } if now >= until => self.debug = Debug::Idle,
                _ => break,
            }
        }
    }

    /// The trailing edge of a register strobe: the 74LS374s take
    /// `DBD<15:0>` under `-DB ADR0 CLK`, the 25LS2519 `DBD<2:0>` under
    /// `-DB ADR1 CLK`; `-DB READ STATUS` loads nothing.
    fn debug_latch(&mut self) {
        let Some(req) = self.debug_request else { return };
        match req.strobe {
            DEBUG_ADDRESS => self.debug_address = req.dbd,
            DEBUG_MODIFIER => {
                let was = self.debug_modifier;
                self.debug_modifier = req.dbd & 7;
                // The timeout counter, held clear while the inhibit was up,
                // counts from its release --- from the first rising edge
                // of the oscillator's output past the release, the output
                // open since the grant: a cycle of the processor's that
                // nothing answers is given up on [`TIMEOUT_NS`] after that
                // edge. Measured on the netlist board; see [`NXM_VCO_NS`].
                let inhibit = debug_modifier::TIMEOUT_INHIBIT;
                if was & inhibit != 0
                    && self.debug_modifier & inhibit == 0
                    && let State::Granted { ack, loadmd, answered, timed_out: true } = self.state
                    && ack == u64::MAX
                {
                    let _ = (loadmd, answered);
                    let lift = self.debug_requested_at.saturating_add(req.hold_ns);
                    let edge = self.timing.free_running(crate::chip::gated_rise_after(
                        crate::chip::VCO_PERIOD,
                        self.granted_at,
                        lift.max(self.granted_at),
                    ));
                    // The same shape as a Unibus cycle timing out unheld.
                    let nxm = edge + TIMEOUT_NS;
                    self.state = State::Granted {
                        ack: nxm + UNIBUS_ACK_NS,
                        loadmd: nxm + UNIBUS_STROBE_NS,
                        answered: nxm,
                        timed_out: true,
                    };
                }
            }
            _ => {}
        }
    }

    /// The debug master's synchronous steps, at a master clock edge.
    fn debug_edge(&mut self, now: u64) {
        match self.debug {
            // The synchronizer at UPRIOR 0D10 registers `NPR` --- at an edge
            // after the request came, not one this machine has yet to run
            // to when a request in its future is posted.  A grant already
            // out on the chain for the processor's own request, its `SACK`
            // not yet made, is this master's: the 74LS74 at UBMAST 0D02 is
            // first on `NPG1 IN` and takes it at this edge, and the
            // processor's request waits behind `-UB BBSY` for the release.
            // **Measured on the board**: granted at the first edge after
            // the request, `SACK` 100 ns on, master at the next edge and
            // acknowledged 790 ns after the request, the processor master
            // again at the edge after the release.
            Debug::NeedUb if now > self.debug_requested_at => {
                if let State::Arbitrating { stage: 3, sack_at } = self.state
                    && now <= sack_at
                {
                    // Never selected, the processor's request is back to
                    // waiting for a grant, which the PROM gives it at the
                    // first edge the debug master no longer asks --- while
                    // it still holds `-UB BBSY`, so its `SACK` and mastery
                    // follow the release.
                    self.state = State::Arbitrating { stage: 2, sack_at: u64::MAX };
                    self.debug_sack_at = now + DEBUG_SELECT_NS;
                    self.debug = Debug::Granted { sack_at: self.debug_sack_at };
                } else {
                    self.debug = Debug::Nprd;
                }
            }
            // The PROM grants; the 74LS74 at UBMAST 0D02 takes `NPG1 IN`
            // at once, being first on the chain, and `DB UB SELECTED`
            // answers `SACK` a delay-line section on.
            Debug::Nprd if self.grant_hold == 0 && !self.lm_grant_out() => {
                self.debug_sack_at = now + DEBUG_SELECT_NS;
                self.debug = Debug::Granted { sack_at: self.debug_sack_at };
            }
            // `SACKD`: the grant withdrawn, `NPG1 IN` down; `BUS READY`
            // unless the processor holds `-UB BBSY`.  With the processor's
            // own cycle in progress --- its `-UB MSYN` down as the edge
            // came; one granted at this edge has its `MSYN` still to come
            // --- the grant stays out until that cycle's end,
            // [`GRANT_WITHDRAW_NS`].
            Debug::Granted { sack_at } if now > sack_at && !self.msyn_down_before_edge => {
                if self.unibus_master {
                    self.debug = Debug::Selected;
                } else {
                    self.debug_set_master(now);
                }
            }
            // The processor let go at this edge --- `LM NEED UB` down with
            // the debug master's `SACK` up cleared `LMUB MASTER` above.
            Debug::Selected if !self.unibus_master => self.debug_set_master(now),
            Debug::Master { .. } => self.debug_xbus_edge(now),
            _ => {}
        }
    }

    /// `DBUB MASTER` sets at `now`: the address goes out, `-UB MSYN`
    /// follows, and the slave answers as it answers the processor ---
    /// or never.  `INT BUSY`, which starts the timeout counter, is
    /// `NAND(-UBX GRANT, -LMX GRANT, -LMUB GRANT)` at RQSYNC 0C13 and
    /// knows nothing of this master, so a cycle nothing answers waits for
    /// the debugger to give up, which its own interface does at
    /// [`debug_timeout_at`] its grant, 11.5 to 12.3 microseconds on.
    /// `SACK` drops with the master set.
    fn debug_set_master(&mut self, now: u64) {
        let req = self.debug_request.expect("a debug master with no request");
        let msyn = now + DEBUG_MSYN_NS;
        let (ack, answered) = match self.debug_responder {
            Responder::Interface => (
                msyn + DIAGNOSTIC_NS,
                if req.write { msyn + REGISTER_STROBE_NS } else { msyn + DIAGNOSTIC_NS },
            ),
            Responder::Unibus(u) => match ioboard::answers(u, req.write) {
                Some(r) => {
                    let ssyn = self.io.answer(r, req.write, msyn);
                    (ssyn, if r == ioboard::USEC_LOW { msyn } else { ssyn })
                }
                // A write the decoder takes nowhere is never acknowledged;
                // the debugger's own timeout ends it.
                None => (u64::MAX, u64::MAX),
            },
            // The buffers answer as a register cycle; the Xbus half waits
            // for its edges, [`Busint::debug_xbus_edge`]; a refused access
            // sets `UB MAP ERROR` at `UB XBUS T100` and is never answered.
            Responder::MapBuffer => (
                msyn + DIAGNOSTIC_NS,
                if req.write { msyn + REGISTER_STROBE_NS } else { msyn + DIAGNOSTIC_NS },
            ),
            Responder::MapRefused => (u64::MAX, msyn + UB_XBUS_REQUEST_NS),
            _ => (u64::MAX, u64::MAX),
        };
        let until = ack.saturating_add(req.hold_ns);
        self.debug = Debug::Master { since: now, msyn, ack, answered, until, xbus: 0 };
        self.debug_ack = ack;
        self.debug_answered = answered;
        self.sack_off(now, now);
    }

    /// The Xbus half of a mapped access, at a master clock edge: `UBXRQ`
    /// up [`UB_XBUS_REQUEST_NS`] after `-UB MSYN` and registered at the
    /// first edge after; `UBX GRANT` at the next, the interface free and
    /// the processor not asking for the Xbus; then the Xbus cycle as the
    /// processor's run, `-XBUS RQ` [`SETUP_NS`] on, the memory board's
    /// twin saying when it answers, `XACK` deskewed for a read, and `SSYN`
    /// [`UB_XBUS_READ_ACK_NS`] after `-UBACK` for a read and with it for a
    /// write.  The processor asking for the Xbus meanwhile, and a mapped
    /// page nothing answers, are **not modeled**: the debuggee CC works on
    /// is halted, and its map points at memory.
    fn debug_xbus_edge(&mut self, now: u64) {
        let Debug::Master { since, msyn, until, xbus, .. } = self.debug else { return };
        let phys = match self.debug_responder {
            Responder::MapXbus(phys) => Some(phys),
            Responder::MapMd => None,
            _ => return,
        };
        match xbus {
            0 if now > msyn + UB_XBUS_REQUEST_NS => {
                self.debug = Debug::Master {
                    since,
                    msyn,
                    ack: u64::MAX,
                    answered: u64::MAX,
                    until,
                    xbus: 1,
                };
            }
            // `UBX GRANT` with `-UB TO MD`: `UB MD LOAD` at this edge, no
            // Xbus cycle; `-LOADMD ACK` acknowledges [`UB_MD_ACK_NS`] on.
            1 if phys.is_none()
                && !matches!(self.state, State::Granted { .. } | State::Acked { .. }) =>
            {
                let req = self.debug_request.expect("a debug master with no request");
                let ack = now + UB_MD_ACK_NS;
                let until = ack.saturating_add(req.hold_ns);
                self.debug = Debug::Master { since, msyn, ack, answered: now, until, xbus: 2 };
                self.debug_ack = ack;
                self.debug_answered = now;
            }
            1 if !matches!(self.state, State::Granted { .. } | State::Acked { .. }) => {
                let phys = phys.expect("an Xbus access has a physical address");
                let rq = now + SETUP_NS;
                let write = self.debug_request.is_some_and(|r| r.write);
                let answered = match decode(phys, self.memory.len() << 16) {
                    Responder::Memory(k) => {
                        let at = self.memory[k as usize].request(rq);
                        self.memory_next =
                            self.memory_next.min(self.memory[k as usize].next_change());
                        at
                    }
                    _ => rq + self.device_ns,
                };
                let xack = if write { answered } else { answered + XBUS_ACK_NS };
                let ack = if write { xack } else { xack + UB_XBUS_READ_ACK_NS };
                let req = self.debug_request.expect("a debug master with no request");
                let until = ack.saturating_add(req.hold_ns);
                self.debug = Debug::Master { since, msyn, ack, answered, until, xbus: 2 };
                self.debug_ack = ack;
                self.debug_answered = answered;
                // The processor lifts `-XBUS RQ` when the master's cycle
                // ends; the board is not idle before.
                if let Responder::Memory(k) = decode(phys, self.memory.len() << 16) {
                    self.memory[k as usize].released(ack + UNIBUS_STROBE_NS);
                    self.memory_next = self.memory_next.min(self.memory[k as usize].next_change());
                }
            }
            _ => {}
        }
    }

    /// What answers the debug master's cycle, as the request came with it.
    pub fn debug_responder(&self) -> Responder {
        self.debug_responder
    }

    /// `-UB SACK` from the debug master: up from `DB UB SELECTED` until
    /// `DBUB MASTER` sets.
    fn debug_sack_up(&self, now: u64) -> bool {
        match self.debug {
            Debug::Granted { sack_at } => now >= sack_at,
            Debug::Selected => true,
            _ => false,
        }
    }

    /// A grant to the debug master is out, or its `SACK` is up: the PROM
    /// does not grant again meanwhile.
    fn debug_grant_out(&self) -> bool {
        matches!(self.debug, Debug::Granted { .. } | Debug::Selected)
    }

    /// The debug master has `NPRD` registered and will take this edge's
    /// grant.
    fn debug_asking(&self) -> bool {
        self.debug == Debug::Nprd
    }

    /// A grant to the processor is out or its `SACK` is up: stage 3 of
    /// [`Busint::mclk_edge`], and stage 7 waiting behind the debug master.
    fn lm_grant_out(&self) -> bool {
        matches!(self.state, State::Arbitrating { stage: 3 | 7, .. })
    }

    /// `-UB BBSY` held by the debug master at `now`.
    fn debug_holds_bbsy(&self, now: u64) -> bool {
        match self.debug {
            Debug::Master { .. } => true,
            Debug::Releasing { until } => now < until,
            _ => false,
        }
    }

    /// `LMUB MASTER`: the board has had the Unibus and keeps it, so the
    /// next Unibus cycle skips the arbitration --- until the debug master
    /// takes it, [`Busint::debug_master`].
    pub fn holds_the_unibus(&self) -> bool {
        self.unibus_master
    }

    /// `-MEMGRANT`: "Low when the processor has the bus.  Do not hang when
    /// this line is high!"
    pub fn granted(&self) -> bool {
        matches!(self.state, State::Granted { .. } | State::Acked { .. })
    }

    /// Whether a cycle is outstanding at all, which is what `MBUSY` follows.
    pub fn busy(&self) -> bool {
        self.state != State::Idle
    }

    /// `WRCYC` for the cycle being run: "high to write, low to read".
    pub fn writing(&self) -> bool {
        self.write
    }

    /// Advances to `now` and reports `-MEMACK` if it has risen by then.
    ///
    /// Calling this repeatedly is how the caller waits: `-MEMACK` is
    /// asynchronous to the clock, so the CPU sees it whenever it looks.
    pub fn poll(&mut self, now: u64, responder: Responder) -> Option<Ack> {
        // The debugger's `NXM TIMEOUT` on a debug cycle the other machine
        // has not answered: `SSYN T0` is `SSYN IN OR NXM TIMEOUT`, so the
        // cycle is acknowledged as one nothing answers is, and `-UB MSYN`
        // drops at `SSYN T100`, `SELECT DEBUG` and the request with it.
        if self.debug_out_pending && now >= self.debug_out_timeout_at {
            let at = self.debug_out_timeout_at;
            self.debug_out_pending = false;
            self.debug_out = Some(DebugOut::Release { at: at + UNIBUS_STROBE_NS });
            self.state = State::Granted {
                ack: at + UNIBUS_ACK_NS,
                loadmd: at + UNIBUS_STROBE_NS,
                answered: at,
                timed_out: true,
            };
        }
        if let State::Granted { ack, loadmd, answered, timed_out } = self.state
            && now >= ack
        {
            self.state = State::Acked { at: ack, loadmd, answered, timed_out };
        }
        match self.state {
            State::Acked { at, loadmd, answered, timed_out } => Some(Ack {
                timed_out,
                responder,
                at,
                loadmd_at: loadmd,
                answered_at: answered,
                cached: self.cached,
            }),
            _ => None,
        }
    }

    /// When `-MEMACK` is due, if a cycle is running and has been granted.
    /// The clock generator needs this to know how long a hang lasts.
    pub fn ack_at(&self) -> Option<u64> {
        match self.state {
            State::Granted { ack, .. } => Some(ack),
            State::Acked { at, .. } => Some(at),
            _ => None,
        }
    }

    /// When `-LOADMD` rises, if a cycle is running and has been granted.
    pub fn loadmd_at(&self) -> Option<u64> {
        match self.state {
            State::Granted { loadmd, .. } | State::Acked { loadmd, .. } => Some(loadmd),
            _ => None,
        }
    }

    /// When the slave gives or takes the word, if a cycle is running and
    /// has been granted: at `-XBUS RQ` on the Xbus, at `-UB SSYN` on the
    /// Unibus, and at [`REGISTER_STROBE_NS`] for a write of the board's own
    /// registers --- before the acknowledgement in every case. A clock on
    /// the I/O board is read then, and a written register changes then.
    pub fn answered_at(&self) -> Option<u64> {
        match self.state {
            State::Granted { answered, .. } | State::Acked { answered, .. } => Some(answered),
            _ => None,
        }
    }

    /// The CPU drops `-MEMRQ`, which drops `-MEMACK`: "MEMRQ drops when
    /// MEMACK rises, which causes MEMACK to drop.  This is all
    /// asynchronous."
    pub fn finish(&mut self) {
        if let State::Acked { at, .. } = self.state {
            self.memrq_up_at = Some(at + MFINISHD_NS);
        }
        self.state = State::Idle;
    }
}

// --- Checkpoints ------------------------------------------------------------

impl Responder {
    pub fn save(self, w: &mut crate::checkpoint::Writer) {
        match self {
            Responder::Memory(b) => {
                w.u8(0);
                w.u8(b);
            }
            Responder::Device => w.u8(1),
            Responder::Interface => w.u8(2),
            Responder::Unibus(a) => {
                w.u8(3);
                w.u32(a);
            }
            Responder::Debug(b) => {
                w.u8(4);
                w.u8(b);
            }
            Responder::MapBuffer => w.u8(5),
            Responder::MapXbus(a) => {
                w.u8(6);
                w.u32(a);
            }
            Responder::MapRefused => w.u8(7),
            Responder::MapMd => w.u8(8),
            Responder::NoXbus => w.u8(9),
            Responder::NoUnibus => w.u8(10),
        }
    }

    pub fn load(r: &mut crate::checkpoint::Reader) -> std::io::Result<Responder> {
        Ok(match r.u8()? {
            0 => Responder::Memory(r.u8()?),
            1 => Responder::Device,
            2 => Responder::Interface,
            3 => Responder::Unibus(r.u32()?),
            4 => Responder::Debug(r.u8()?),
            5 => Responder::MapBuffer,
            6 => Responder::MapXbus(r.u32()?),
            7 => Responder::MapRefused,
            8 => Responder::MapMd,
            9 => Responder::NoXbus,
            10 => Responder::NoUnibus,
            v => return Err(crate::checkpoint::bad(format!("{v} for a responder"))),
        })
    }
}

impl State {
    fn save(self, w: &mut crate::checkpoint::Writer) {
        match self {
            State::Idle => w.u8(0),
            State::Requested => w.u8(1),
            State::Arbitrating { stage, sack_at } => {
                w.u8(2);
                w.u8(stage);
                w.u64(sack_at);
            }
            State::Granted { ack, loadmd, answered, timed_out } => {
                w.u8(3);
                w.u64(ack);
                w.u64(loadmd);
                w.u64(answered);
                w.bool(timed_out);
            }
            State::Acked { at, loadmd, answered, timed_out } => {
                w.u8(4);
                w.u64(at);
                w.u64(loadmd);
                w.u64(answered);
                w.bool(timed_out);
            }
        }
    }

    fn load(r: &mut crate::checkpoint::Reader) -> std::io::Result<State> {
        Ok(match r.u8()? {
            0 => State::Idle,
            1 => State::Requested,
            2 => State::Arbitrating { stage: r.u8()?, sack_at: r.u64()? },
            3 => State::Granted {
                ack: r.u64()?,
                loadmd: r.u64()?,
                answered: r.u64()?,
                timed_out: r.bool()?,
            },
            4 => State::Acked {
                at: r.u64()?,
                loadmd: r.u64()?,
                answered: r.u64()?,
                timed_out: r.bool()?,
            },
            v => return Err(crate::checkpoint::bad(format!("{v} for a bus state"))),
        })
    }
}

impl Debug {
    fn save(self, w: &mut crate::checkpoint::Writer) {
        match self {
            Debug::Idle => w.u8(0),
            Debug::Strobe { until } => {
                w.u8(1);
                w.u64(until);
            }
            Debug::NeedUb => w.u8(2),
            Debug::Nprd => w.u8(3),
            Debug::Granted { sack_at } => {
                w.u8(4);
                w.u64(sack_at);
            }
            Debug::Selected => w.u8(5),
            Debug::Master { since, msyn, ack, answered, until, xbus } => {
                w.u8(6);
                w.u64(since);
                w.u64(msyn);
                w.u64(ack);
                w.u64(answered);
                w.u64(until);
                w.u8(xbus);
            }
            Debug::Releasing { until } => {
                w.u8(7);
                w.u64(until);
            }
        }
    }

    fn load(r: &mut crate::checkpoint::Reader) -> std::io::Result<Debug> {
        Ok(match r.u8()? {
            0 => Debug::Idle,
            1 => Debug::Strobe { until: r.u64()? },
            2 => Debug::NeedUb,
            3 => Debug::Nprd,
            4 => Debug::Granted { sack_at: r.u64()? },
            5 => Debug::Selected,
            6 => Debug::Master {
                since: r.u64()?,
                msyn: r.u64()?,
                ack: r.u64()?,
                answered: r.u64()?,
                until: r.u64()?,
                xbus: r.u8()?,
            },
            7 => Debug::Releasing { until: r.u64()? },
            v => return Err(crate::checkpoint::bad(format!("{v} for the debug master's state"))),
        })
    }
}

impl DebugRequest {
    fn save(self, w: &mut crate::checkpoint::Writer) {
        let DebugRequest { strobe, write, dbd, hold_ns } = self;
        w.u8(strobe);
        w.bool(write);
        w.u16(dbd);
        w.u64(hold_ns);
    }

    fn load(r: &mut crate::checkpoint::Reader) -> std::io::Result<DebugRequest> {
        Ok(DebugRequest { strobe: r.u8()?, write: r.bool()?, dbd: r.u16()?, hold_ns: r.u64()? })
    }
}

impl DebugOut {
    fn save(self, w: &mut crate::checkpoint::Writer) {
        match self {
            DebugOut::Request { at, strobe } => {
                w.u8(0);
                w.u64(at);
                w.u8(strobe);
            }
            DebugOut::Release { at } => {
                w.u8(1);
                w.u64(at);
            }
        }
    }

    fn load(r: &mut crate::checkpoint::Reader) -> std::io::Result<DebugOut> {
        Ok(match r.u8()? {
            0 => DebugOut::Request { at: r.u64()?, strobe: r.u8()? },
            1 => DebugOut::Release { at: r.u64()? },
            v => return Err(crate::checkpoint::bad(format!("{v} for a DBGOUT state"))),
        })
    }
}

impl MemoryBoard {
    pub(crate) fn save(&self, w: &mut crate::checkpoint::Writer) {
        let MemoryBoard {
            idle_at,
            release_at,
            refresh_time_at,
            time_off_at,
            refresh_stage,
            refresh_rq_at,
            in_reset,
            timing: _,
        } = *self;
        w.u64(idle_at);
        w.u64(release_at);
        w.u64(refresh_time_at);
        w.u64(time_off_at);
        w.u8(refresh_stage);
        w.u64(refresh_rq_at);
        w.bool(in_reset);
    }

    pub(crate) fn load(r: &mut crate::checkpoint::Reader) -> std::io::Result<MemoryBoard> {
        Ok(MemoryBoard {
            idle_at: r.u64()?,
            release_at: r.u64()?,
            refresh_time_at: r.u64()?,
            time_off_at: r.u64()?,
            refresh_stage: r.u8()?,
            refresh_rq_at: r.u64()?,
            in_reset: r.bool()?,
            timing: TimingModel::Cadr,
        })
    }
}

impl Busint {
    /// The board's model into a checkpoint: the cycle in progress, the
    /// memory boards' timers, and the debug cable's side of the bus.
    pub fn save(&self, w: &mut crate::checkpoint::Writer) {
        let Busint {
            state,
            write,
            memory,
            io: _,
            board,
            device_ns,
            unibus_master,
            memrq_up_at,
            debug_sack_at,
            msyn_down_before_edge,
            lm_need_ub,
            grant_hold,
            debug,
            debug_request,
            debug_requested_at,
            granted_at,
            debug_responder,
            debug_address,
            debug_modifier,
            debug_freed_at,
            debug_ack,
            debug_answered,
            debug_cable,
            debug_out,
            debug_out_pending,
            debug_out_timeout_at,
            memory_next,
            timing: _,
            cache,
            addr,
            cached,
            buffer_free_at,
            memory_timing,
            memory_free_at,
        } = self;
        state.save(w);
        w.bool(*write);
        w.u64(memory.len() as u64);
        for m in memory {
            m.save(w);
        }
        w.opt(*board, crate::checkpoint::Writer::u8);
        w.u64(*device_ns);
        w.bool(*unibus_master);
        w.opt(*memrq_up_at, crate::checkpoint::Writer::u64);
        w.u64(*debug_sack_at);
        w.bool(*msyn_down_before_edge);
        w.bool(*lm_need_ub);
        w.u8(*grant_hold);
        debug.save(w);
        w.opt(*debug_request, |w, d| d.save(w));
        w.u64(*debug_requested_at);
        w.u64(*granted_at);
        debug_responder.save(w);
        w.u16(*debug_address);
        w.u16(*debug_modifier);
        w.u64(*debug_freed_at);
        w.u64(*debug_ack);
        w.u64(*debug_answered);
        w.bool(*debug_cable);
        w.opt(*debug_out, |w, d| d.save(w));
        w.bool(*debug_out_pending);
        w.u64(*debug_out_timeout_at);
        w.u64(*memory_next);
        w.opt(cache.as_ref(), |w, c| c.save(w));
        w.u32(*addr);
        w.bool(*cached);
        w.u64(*buffer_free_at);
        w.opt(*memory_timing, |w, t| {
            w.u64(t.read_ns);
            w.u64(t.write_ns);
        });
        w.u64(*memory_free_at);
    }

    /// Back from a checkpoint, into a model with as many memory boards.
    pub fn load(&mut self, r: &mut crate::checkpoint::Reader) -> std::io::Result<()> {
        self.state = State::load(r)?;
        self.write = r.bool()?;
        let n = r.u64()? as usize;
        if n != self.memory.len() {
            return Err(crate::checkpoint::bad(format!(
                "{n} memory boards, and this machine has {}",
                self.memory.len()
            )));
        }
        for m in self.memory.iter_mut() {
            *m = MemoryBoard::load(r)?;
            m.timing = self.timing;
        }
        self.board = r.opt(crate::checkpoint::Reader::u8)?;
        self.device_ns = r.u64()?;
        self.unibus_master = r.bool()?;
        self.memrq_up_at = r.opt(crate::checkpoint::Reader::u64)?;
        self.debug_sack_at = r.u64()?;
        self.msyn_down_before_edge = r.bool()?;
        self.lm_need_ub = r.bool()?;
        self.grant_hold = r.u8()?;
        self.debug = Debug::load(r)?;
        self.debug_request = r.opt(DebugRequest::load)?;
        self.debug_requested_at = r.u64()?;
        self.granted_at = r.u64()?;
        self.debug_responder = Responder::load(r)?;
        self.debug_address = r.u16()?;
        self.debug_modifier = r.u16()?;
        self.debug_freed_at = r.u64()?;
        self.debug_ack = r.u64()?;
        self.debug_answered = r.u64()?;
        self.debug_cable = r.bool()?;
        self.debug_out = r.opt(DebugOut::load)?;
        self.debug_out_pending = r.bool()?;
        self.debug_out_timeout_at = r.u64()?;
        self.memory_next = r.u64()?;
        self.cache = r.opt(crate::cache::Cache::load)?;
        self.addr = r.u32()?;
        self.cached = r.bool()?;
        self.buffer_free_at = r.u64()?;
        self.memory_timing =
            r.opt(|r| Ok(crate::cache::MemoryTiming { read_ns: r.u64()?, write_ns: r.u64()? }))?;
        self.memory_free_at = r.u64()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything the board's model holds, as the checkpoint writes it:
    /// [`Busint::save`] destructures the whole struct and writes every
    /// field, so two equal snapshots are two equal models.
    fn snapshot(b: &Busint) -> Vec<u8> {
        let mut w = crate::checkpoint::Writer::new();
        b.save(&mut w);
        w.finish()
    }

    /// A board in the shape a machine with no debugger on it is in: the
    /// cable idle, and the processor holding the Unibus with a cycle just
    /// finished, which is where it sits for almost every microcycle of a
    /// band run.
    fn no_debugger() -> Busint {
        let mut b = Busint::new(1);
        b.unibus_master = true;
        b.memrq_up_at = Some(1_000);
        assert_eq!(b.debug, Debug::Idle);
        b
    }

    /// A state to put the board in, with a name for the failure message.
    type Step = (&'static str, fn(&mut Busint));

    fn request() -> DebugRequest {
        DebugRequest { strobe: DEBUG_ADDRESS, write: true, dbd: 0o377, hold_ns: 100 }
    }

    /// **`debug_advance` changes nothing while the cable is idle**, at any
    /// instant and however often it is called.  This is what the early
    /// return at the top of it claims, and the engines lean on it twice a
    /// microcycle: over 200,000,000 microcycles of the System 100 band the
    /// function is entered 400 million times and the state is `Idle` on
    /// every one.
    #[test]
    fn debug_advance_changes_nothing_while_the_cable_is_idle() {
        let mut b = no_debugger();
        let before = snapshot(&b);
        for now in [0, 999, 1_000, 1_001, 10_000, 1_000_000, u64::MAX / 2] {
            b.debug_advance(now);
            assert_eq!(snapshot(&b), before, "at {now} ns");
        }
        // And with the cable's `SACK` where a debugger would have put it,
        // which is the only thing that makes the first block's conditions
        // true, it is still `Idle` that decides.
        b.debug_sack_at = 0;
        let before = snapshot(&b);
        b.debug_advance(2_000);
        assert_eq!(snapshot(&b), before, "the SACK is not what holds it");
    }

    /// **Every state that is not `Idle` is a state `debug_advance` steps.**
    /// So the early return may test for `Idle` and nothing wider: widen it
    /// to any of these and the step it owes is skipped.
    ///
    /// `tests/chip.rs` catches four of the five against the netlist board.
    /// `Granted` it does not --- the debug master is `Selected` when the
    /// processor's cycle ends in every sequence that test runs --- so this
    /// is the only thing holding that arm.
    #[test]
    fn debug_advance_steps_every_state_that_is_not_idle() {
        let steps: [Step; 5] = [
            ("Strobe", |b| b.debug = Debug::Strobe { until: 1_500 }),
            ("Master", |b| {
                b.debug =
                    Debug::Master { since: 0, msyn: 0, ack: 0, answered: 0, until: 1_500, xbus: 0 }
            }),
            ("Releasing", |b| b.debug = Debug::Releasing { until: 1_500 }),
            ("Selected", |b| {
                b.debug_sack_at = 0;
                b.debug = Debug::Selected;
            }),
            ("Granted", |b| {
                b.debug_sack_at = 0;
                b.debug = Debug::Granted { sack_at: 0 };
            }),
        ];
        for (what, set) in steps {
            let mut b = no_debugger();
            b.debug_request = Some(request());
            set(&mut b);
            let before = snapshot(&b);
            b.debug_advance(2_000);
            assert_ne!(snapshot(&b), before, "{what} is a step and must be taken");
        }
    }
}
