// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The diagnostic bus, and the registers on it.
//!
//! The five flat cables between the cpu and the bus interface carry two
//! busses.  `src/cable.rs` is the memory one; this is the other:
//!
//! > One of the busses is the diagnostic bus.  The bus interface is the
//! > master of this bus.  The signals are
//!
//! ```text
//! SPY<15:0>   Bi-directional data lines.
//! EADR<3:0>   Select one of 16 16-bit diagnostic registers,
//!             see the prints for which addresses do what.
//!             Read and write at the same address are
//!             uncorrelated.
//! -DBREAD     When low, reading is occurring and the cpu drives
//!             SPY<15:0>.
//! -DBWRITE    When low, writing is occurring, and
//!             the bus interface drives SPY<15:0>.  The trailing
//!             edge of -DBWRITE clocks the data into the register,
//!             so you must hold SPY longer.
//! ```
//!
//! --- `cadr/busint.erface`, which also gives the address: "The EADR<3:0>
//! lines just follow the Unibus address <4:1>."
//!
//! So the sixteen registers are Unibus `0o766000`-`0o766036`, and the prints
//! it tells us to see are three 74S138s on page SPY0 of
//! `data/CADR.netlist` --- one decoding writes and two decoding reads.  The
//! registers themselves are on the cpu board, not on the bus interface.
//!
//! **Three sources name the sixteen registers and agree**, reached by three
//! routes: the decoders and buffers on pages SPY0, SPY1, SPY2, SPY4 and STAT
//! of the netlist; CC's own symbols in `lcadrd.lisp` (System 46,
//! `SPY-IR-LOW` to `SPY-STAT-HIGH`, `SPY-CLK`, `SPY-OPC-CONTROL` and
//! `SPY-MODE`), which is the program the interface was built for; and MIT's
//! `mit/cadr/ir.bits`.  The names here are CC's.  `tests/spy.rs` reads the whole
//! map off the netlist part by part and pin by pin.  Where `ir.bits`
//! differs from the board, the board wins and the difference is
//! named where it bites.
//!
//! **Reads are the engine's, writes are [`crate::machine::Machine`]'s.**  What
//! a read returns is the cpu's own `PC`, `IR`, `OB`, `A`, `M`, flags or
//! statistics counter --- live state every engine keeps its own way --- so
//! [`crate::engine::Engine::spy_read`] answers them.  What a write loads is a
//! flip flop the datapath reads: the mode register, the clock control
//! register, the OPC control register and the debug IR live in `Machine` and
//! [`crate::machine::Machine::spy_write`] loads them, as the trailing edge of
//! `-DBWRITE` does, from a Unibus cycle or from a console with no bus at all.

/// Unibus address of the first of the sixteen registers.
pub const BASE: u32 = 0o766000;

/// `EADR<3:0>`: which diagnostic register a Unibus address selects, or `None`
/// if the address is outside the block.
///
/// "The EADR<3:0> lines just follow the Unibus address <4:1>."
pub fn register(uaddr: u32) -> Option<u8> {
    (BASE..BASE + 0o40).contains(&uaddr).then_some(((uaddr >> 1) & 0o17) as u8)
}

// --- Reading: the two 74S138s at SPY0 1F01 and 1F02, split by `EADR3` ---

/// `-SPY.IRL`, CC's `SPY-IR-LOW`: `IR<15:0>` through the 74LS244s at SPY1
/// 3E01 and 3E03.
pub const IR_LOW: u8 = 0;
/// `-SPY.IRM`, `SPY-IR-MED`: `IR<31:16>`, SPY1 3F25 and 3F23.
pub const IR_MED: u8 = 1;
/// `-SPY.IRH`, `SPY-IR-HIGH`: `IR<47:32>`, SPY1 3F21 and 3E06.
pub const IR_HIGH: u8 = 2;
/// `SPY-IR-EXT` (a proposed name, after CC's `SPY-IR-LOW`, `-MED` and
/// `-HIGH`): on QUUX revision 15 alone, `IR<63:48>`, the 64-bit word's
/// extension (MP4 ruling Q4). On the CADR and revisions 13 and 14 nothing
/// is decoded there and it reads [`OPEN_READ`].
pub const IR_EXT: u8 = 3;
/// `-SPY.OPC`, `SPY-OPC`: `OPC<13:0>`, the last stage of the OPCS shift
/// registers, through SPY4 1E07 and 1E06.  Bits 15 and 14 are grounded.
pub const OPC: u8 = 4;
/// `-SPY.PC`, `SPY-PC`: `PC<13:0>`, SPY4 1D07 and 1D06.  Bits 15 and 14 are
/// grounded.
pub const PC: u8 = 5;
/// `-SPY.OBL`, `SPY-OB-LOW`: `OB<15:0>`, SPY1 2C17 and 2C18.
pub const OB_LOW: u8 = 6;
/// `-SPY.OBH`, `SPY-OB-HIGH`: `OB<31:16>`, SPY1 3C23 and 3C24.
pub const OB_HIGH: u8 = 7;
/// `-SPY.FLAG1`, `SPY-FLAG-1`: see [`Flag1`].
pub const FLAG_1: u8 = 8;
/// `-SPY.FLAG2`, `SPY-FLAG-2`: see [`Flag2`].
pub const FLAG_2: u8 = 9;
/// `-SPY.ML`, `SPY-M-LOW`: `M<15:0>`, SPY2 4A15 and 4A13.
pub const M_LOW: u8 = 10;
/// `-SPY.MH`, `SPY-M-HIGH`: `M<31:16>`, SPY2 4B13 and 4B17.
pub const M_HIGH: u8 = 11;
/// `-SPY.AL`, `SPY-A-LOW`: `AA<15:0>`, the A bus's low half under the name
/// ALATCH gives it, SPY2 1F13 and 1F11.
pub const A_LOW: u8 = 12;
/// `-SPY.AH`, `SPY-A-HIGH`: `A<31:16>`, SPY2 3A27 and 3A26.
pub const A_HIGH: u8 = 13;
/// `-SPY.STL`, `SPY-STAT-LOW`: `ST<15:0>`, the statistics counter, STAT 1B09
/// and 1B08.
pub const STAT_LOW: u8 = 14;
/// `-SPY.STH`, `SPY-STAT-HIGH`: `ST<31:16>`, STAT 1B07 and 1B06.
pub const STAT_HIGH: u8 = 15;

// --- Writing: the 74S138 at SPY0 1F03, which `EADR3` does not reach ---

/// `-LDDBIRL`, `-LDDBIRM`, `-LDDBIRH`: the three halves of the debug IR,
/// written at [`IR_LOW`], [`IR_MED`] and [`IR_HIGH`] --- six 74S374s on page
/// DEBUG, `SPY<15:0>` straight into `I<15:0>`, `I<31:16>` and `I<47:32>`,
/// whose outputs drive the I bus while `IDEBUG` is up.  See [`ClockControl`].
///
/// `-LDCLK`, `EADR` 3, Unibus `0o766006`: the clock control register's write
/// strobe, CC's `SPY-CLK`.
pub const CLK: u8 = 3;
/// `-LDOPC`, `EADR` 4, Unibus `0o766010`: the OPC control register's write
/// strobe, CC's `SPY-OPC-CONTROL`.
pub const OPC_CONTROL: u8 = 4;
/// `-LDMODE`, `EADR` 5, Unibus `0o766012`: the mode register's write strobe,
/// CC's `SPY-MODE`.
///
/// Y5 of the 74S138 at SPY0 1F03, whose address is `EADR<2:0>` and whose
/// enables are `-DBWRITE`, `GND` and `HI1`.  **`EADR3` does not reach that
/// decoder**, so a write to `0o766032` loads the mode register too; the
/// read decoders are the ones split by `EADR3`.  [`write_strobe`] has the
/// aliasing.
pub const MODE: u8 = 5;

/// `-LDDBIRX` (a proposed name), `EADR` 6 and its alias 14: on QUUX
/// revision 15 alone, the debug IR's `<63:48>`, so that a console's word
/// runs with its extension (MP4 ruling Q4). A console writes all four
/// halves, the extension's too, so that no stale extension rides along.
/// On the CADR and revisions 13 and 14, Y6 is not connected.
pub const LDDBIRX: u8 = 6;

/// Which write strobe a register number fires: `EADR<2:0>`, since the write
/// decoder's `G1` is `HI1` and not `EADR3`.  Y6 and Y7 are not connected, so
/// 6, 7, 14 and 15 load nothing, but on revision 15, whose 6 and 14 are
/// [`LDDBIRX`].
pub fn write_strobe(eadr: u8) -> u8 {
    eadr & 7
}

/// The mode register, OLORD1 1A04: a 74S174 hex D flip flop clocked by
/// `-LDMODE` and cleared by `-RESET`.
///
/// Its six D inputs are `SPY<5:0>` in order and its six Q outputs are the
/// fields below in the same order, read off `data/CADR.netlist`.
/// `tests/spy.rs` checks that order against `muir::part::pinout`, and against
/// MIT's own two sentences about it in `mit/sys/ucadr/promh.text`:
///
/// > Writing 4 in Unibus location 766012, which is at virtual address 1005,
/// > turns on ERROR-STOP-ENABLE.  If we aren't really in a PROM, we write 44
/// > which also turns on (leaves on) PROM-DISABLE.
///
/// Two more bits of the word are decoded but not stored: `SPY6` and `SPY7`
/// are gated with the strobe on OLORD2, so a write with them set is a pulse
/// and not a setting --- and a pulse that starts at the write pulse's
/// leading edge, 100 ns before the register loads at its trailing one.  See
/// [`Machine::spy_write`](crate::machine::Machine::spy_write) and
/// [`crate::busint::REGISTER_PULSE_NS`].
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Mode {
    /// `SPEED0`, D pin 3 to Q pin 2.  With `SPEED1`, which tap of the delay
    /// line ends the microcycle.
    pub speed0: bool,
    /// `SPEED1`, D pin 4 to Q pin 5.
    pub speed1: bool,
    /// `ERRSTOP`, D pin 6 to Q pin 7.  MIT's `ERROR-STOP-ENABLE`: `-ERRHALT`
    /// is `NAND(ERRSTOP, ERR)` at OLORD2 1C09, and `ERR` is the 74S133 at
    /// 1A02 over the ten parity-error flags and `-HALTED`.  It drops
    /// `MACHRUN` through the 9S42 at OLORD1 1A15.
    pub errstop: bool,
    /// `STATHENB`, D pin 11 to Q pin 10.  `-STATHALT` is `NAND(STATHENB,
    /// STATSTOP)` at OLORD1 1C09, and `STATSTOP` is the statistics counter's
    /// carry out registered on `CLK5A` at OLORD2 1A05: with this set the
    /// machine halts after the counted instruction that carries the counter
    /// past all ones.
    pub stathenb: bool,
    /// `TRAPENB`, D pin 13 to Q pin 12.  Enables the memory parity error
    /// trap on page TRAP.
    pub trapenb: bool,
    /// `PROMDISABLE`, D pin 14 to Q pin 15.  MIT's `PROM-DISABLE`: takes the
    /// boot PROM out of the bottom 1K words of the control store,
    /// [`crate::machine::PROM_WORDS`].  It is
    /// registered again by the 74S174 at OLORD1 1A10 on `MCLK5A` before it
    /// gates the PROM, which `src/rtl.rs` models and `src/micro.rs` does not.
    pub prom_disable: bool,
}

/// `SPY6` on a mode-register write: `-PROG.RESET`, `NAND(LDMODE, SPY6)` at
/// OLORD2 1C09, one of the three inputs of the 74S10 at 1C08 that makes
/// `RESET`.  CC's `CC-RESET-MACH` writes `100` here --- "RESET HIGH" --- and
/// then rewrites the mode register, since the reset has just cleared it.
/// `ir.bits` lists it as "100 RESET".
pub const MODE_RESET: u16 = 1 << 6;
/// `SPY7` on a mode-register write: `PROG.BOOT`, `AND(LDMODE, SPY7)` at
/// OLORD2 1D10, which the 74S32 at 1C18 ORs with the two boot buttons into
/// `-BOOT`.  Read off the netlist; neither `ir.bits` nor CC names it.
pub const MODE_BOOT: u16 = 1 << 7;

impl Mode {
    /// The trailing edge of `-LDMODE` clocks `SPY<5:0>` in.  The Unibus is 16
    /// bits wide and the register is six of them; bits 6 and 7 are the two
    /// pulses above and the rest go nowhere.
    pub fn write(&mut self, spy: u16) {
        let bit = |n: u16| spy & (1 << n) != 0;
        *self = Mode {
            speed0: bit(0),
            speed1: bit(1),
            errstop: bit(2),
            stathenb: bit(3),
            trapenb: bit(4),
            prom_disable: bit(5),
        };
    }
}

/// The clock control register, CC's `SPY-CLK`: `RUN` in the 74S74 at OLORD1
/// 1A14 and the other four in the 74S175 at 1A09, all clocked by `-LDCLK`.
///
/// `ir.bits`, under "Writable diagnostic registers":
///
/// ```text
/// 06  Clock control:
///       1  Run
///       2  Step (raising step clocks the machine once)
///       4  NOP (prevents side-effects from instruction in IR)
///      10  IDEBUG (causes IR to load from DEBUG-IR instead of control memory)
///      20  LDSTAT (causes Statistics counter to load from IWR)
/// ```
///
/// CC's `CC-DEBUG-CLOCK` writes `12` then `0`, `CC-NOOP-DEBUG-CLOCK` `16`
/// then `0` and `CC-CLOCK` `2` then `0`, which is the same assignment from
/// the other end.  The board's pins agree; `tests/spy.rs` has them.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct ClockControl {
    /// `RUN`, `SPY0` into 1A14's D.  Preset by `-BOOT` and cleared by
    /// `-CLOCK RESET A`, not by `-RESET`: a reset from the console leaves
    /// it as it was.  `SRUN` is it registered on `MCLK5A` at 1A10, and
    /// `SRUN` is what holds `MACHRUN` up.
    pub run: bool,
    /// `STEP`, `SPY1`.  Registered twice on `MCLK5A` at 1A10, into `SSTEP`
    /// and then `SSDONE`; `MACHRUN` is up for the one master clock in
    /// which `SSTEP` is set and `SSDONE` is not, so raising this runs one
    /// microcycle.  It must be lowered again before the next.
    pub step: bool,
    /// `NOP11`, `SPY2`: nops every microcycle, through the open-collector
    /// 74S08 at CONTRL 3E14 that pulls `-INOP` down.  CC raises it with
    /// `IDEBUG` to load an instruction into `IR` without executing the one
    /// there.
    pub nop11: bool,
    /// `IDEBUG`, `SPY3`: the debug IR drives the I bus in place of the
    /// control store, and `IMOD` is asserted so that the parity check
    /// ignores the word.
    pub idebug: bool,
    /// `LDSTAT`, `SPY4`: `-LDSTAT` is the LOAD of the eight 74S169s on
    /// page STAT, so while it is up the counter takes `IWR<31:0>` at every
    /// `CLK5A`.
    pub ldstat: bool,
}

impl ClockControl {
    /// The trailing edge of `-LDCLK` clocks `SPY<4:0>` in.
    pub fn write(&mut self, spy: u16) {
        let bit = |n: u16| spy & (1 << n) != 0;
        *self = ClockControl {
            run: bit(0),
            step: bit(1),
            nop11: bit(2),
            idebug: bit(3),
            ldstat: bit(4),
        };
    }
}

/// The OPC control register, CC's `SPY-OPC-CONTROL`: three bits of the
/// 74S175 at OLORD1 1A08, clocked by `-LDOPC` and cleared by `-RESET`.
///
/// `ir.bits`:
///
/// ```text
/// 10  OPC control:
///       1  LPC.HOLD (prevents LPC from changing, normally it loads from PC)
///       2  OPCCLK (raising and lowering this clocks the OPCs if the
///                  machine is stopped)
///       4  OPCINH (prevents main clock from clocking the OPCs.)
/// ```
///
/// The fourth flip flop, `SPY3`, drives nothing.  CC's `CC-SAVE-OPCS` reads
/// `SPY-OPC` and writes `2` then `0` here, eight times over.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct OpcControl {
    /// `LPC.HOLD`, `SPY0`: pin 1 of the 25S07s at LPC 4F06-4F08, whose
    /// clock enable it is.
    pub lpc_hold: bool,
    /// `OPCCLK`, `SPY1`: the 9328s' common clock is `NOR(OPCCLK, -CLK5)`
    /// at OPCS 1F14, so with the machine halted --- `CLK5` held high ---
    /// lowering this is the clock's rising edge, and `PC` shifts in.
    pub opcclk: bool,
    /// `OPCINH`, `SPY2`: wired to the 9328s' own clock pins through OPCS
    /// 1F10, so holding it high masks the common clock and freezes the
    /// history.
    pub opcinh: bool,
}

impl OpcControl {
    /// The trailing edge of `-LDOPC` clocks `SPY<2:0>` in.
    pub fn write(&mut self, spy: u16) {
        let bit = |n: u16| spy & (1 << n) != 0;
        *self = OpcControl { lpc_hold: bit(0), opcclk: bit(1), opcinh: bit(2) };
    }
}

/// Loads one 16-bit half of the debug IR, as `-LDDBIRL`, `-LDDBIRM` and
/// `-LDDBIRH` do: `half` is 0, 1 or 2; 3 on revision 15, [`LDDBIRX`].
pub fn write_debug_ir(debug_ir: &mut u64, half: u8, spy: u16) {
    let shift = 16 * half as u32;
    *debug_ir = (*debug_ir & !(0xffffu64 << shift)) | (spy as u64) << shift;
}

/// `SPY-FLAG-1`, register 8: what the 74LS244 at SPY4 1A12 and the 74LS240
/// at 1A13 put on the bus.  Every field is in its logical sense --- `true`
/// means the machine is waiting, the error is present, the halt is on ---
/// and [`Flag1::word`] applies the board's polarities.
///
/// `ir.bits`:
///
/// ```text
/// 20  FLAG-1  15-8 -> (-WAIT,-V1PE,-V0PE,PROMDISABLE,-STATHALT,ERR,SSDONE,SRUN)
///              7-0 -> (-HIGHERR,-MEMPE,-IPE,-DPE,-SPE,-PDLPE,-MPE,-APE)
/// ```
///
/// **The low byte comes through an inverting buffer**, so it reads *high*
/// for an error.  CC's `CC-PRINT-ERROR-STATUS` says so in as many words ---
/// "NOTE THAT THE BUS DRIVER WHICH DRIVES THE LOW ORDER 8 BITS IS AN
/// INVERTING BUS FRYER" --- and lists bit 0 up as `A-MEM-PAR M-MEM-PAR
/// PDL-BUF-PAR SPC-PAR DISP-PAR C-MEM-PAR MN-MEM-PAR HIGH-ERR`.  That byte
/// is easy to get upside down: the inverting driver is what decides it.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Flag1 {
    /// `-WAIT`, bit 15: the cpu clock is stopped for the bus.
    pub wait: bool,
    /// `-V1PE`, bit 14: level-2 map parity error.
    pub v1pe: bool,
    /// `-V0PE`, bit 13: level-1 map parity error.
    pub v0pe: bool,
    /// `PROMDISABLE`, bit 12, off the mode register.  CC prints it as
    /// `(NOT PROM-ENABLE)`.
    pub promdisable: bool,
    /// `-STATHALT`, bit 11: halted by the statistics counter.
    pub stathalt: bool,
    /// `ERR`, bit 10: any of the ten parity errors, or `-HALTED`.
    pub err: bool,
    /// `SSDONE`, bit 9: the single step asked for has run.
    pub ssdone: bool,
    /// `SRUN`, bit 8: `RUN` as the master clock has registered it.
    pub srun: bool,
    /// `-HIGHERR`, bit 7: the two A-memory address parity checks.
    pub higherr: bool,
    /// `-MEMPE`, bit 6: main memory parity error.
    pub mempe: bool,
    /// `-IPE`, bit 5: control store parity error.
    pub ipe: bool,
    /// `-DPE`, bit 4: dispatch memory parity error.
    pub dpe: bool,
    /// `-SPE`, bit 3: microcode stack parity error.
    pub spe: bool,
    /// `-PDLPE`, bit 2: PDL buffer parity error.
    pub pdlpe: bool,
    /// `-MPE`, bit 1: M memory parity error.
    pub mpe: bool,
    /// `-APE`, bit 0: A memory parity error.
    pub ape: bool,
}

impl Flag1 {
    /// The word on `SPY<15:0>` under `-SPY.FLAG1`.
    pub fn word(&self) -> u16 {
        let hi = [
            !self.wait,
            !self.v1pe,
            !self.v0pe,
            self.promdisable,
            !self.stathalt,
            self.err,
            self.ssdone,
            self.srun,
        ];
        let lo = [
            self.higherr,
            self.mempe,
            self.ipe,
            self.dpe,
            self.spe,
            self.pdlpe,
            self.mpe,
            self.ape,
        ];
        let byte = |bits: [bool; 8], top: u16| {
            bits.iter().enumerate().fold(0u16, |w, (k, &b)| w | (b as u16) << (top - k as u16))
        };
        byte(hi, 15) | byte(lo, 7)
    }

    /// The register read back: [`Flag1::word`]'s inverse, applying the same
    /// polarities.  What a console has to do to make anything of `FLAG-1`,
    /// and what the run loop does to see that the machine has stopped
    /// itself.
    pub fn of(w: u16) -> Self {
        let up = |bit: u16| w & (1 << bit) != 0;
        Flag1 {
            wait: !up(15),
            v1pe: !up(14),
            v0pe: !up(13),
            promdisable: up(12),
            stathalt: !up(11),
            err: up(10),
            ssdone: up(9),
            srun: up(8),
            higherr: up(7),
            mempe: up(6),
            ipe: up(5),
            dpe: up(4),
            spe: up(3),
            pdlpe: up(2),
            mpe: up(1),
            ape: up(0),
        }
    }
}

/// `SPY-FLAG-2`, register 9: the two 74LS244s at SPY2 3F15 and 3E16.
///
/// `ir.bits`:
///
/// ```text
/// 22  FLAG-2  15-8 -> (NC,NC,WMAPD,DESTSPC,IWRITED,IMODD,PDLWRITED,SPUSHD)
///              7-0 -> (NC,NC,IR48,NOP,-VMAOK,JCOND,PCS1,PSC0)
/// ```
///
/// The four `NC` inputs float, and a floating TTL input is a one, so bits
/// 15, 14, 7 and 6 read as ones on the board.  Reading them as zeros is the
/// natural assumption and is wrong.
/// `-VMAOK` is the net at VCTL1 1D17, *low* when the access is permitted;
/// [`Flag2::vmaok`] is the logical sense and [`Flag2::word`] inverts it.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Flag2 {
    /// Bit 13.
    pub wmapd: bool,
    /// Bit 12.
    pub destspcd: bool,
    /// Bit 11.
    pub iwrited: bool,
    /// Bit 10.
    pub imodd: bool,
    /// Bit 9.
    pub pdlwrited: bool,
    /// Bit 8.
    pub spushd: bool,
    /// Bit 5: the control store's parity bit, as `IR` holds it.
    pub ir48: bool,
    /// Bit 4.
    pub nop: bool,
    /// Bit 3, in the logical sense: the last mapped access was permitted.
    pub vmaok: bool,
    /// Bit 2.
    pub jcond: bool,
    /// Bit 1.
    pub pcs1: bool,
    /// Bit 0.
    pub pcs0: bool,
}

impl Flag2 {
    /// The four unconnected buffer inputs, read as ones.
    pub const OPEN: u16 = 0xc0c0;

    /// The word on `SPY<15:0>` under `-SPY.FLAG2`.
    pub fn word(&self) -> u16 {
        Self::OPEN
            | (self.wmapd as u16) << 13
            | (self.destspcd as u16) << 12
            | (self.iwrited as u16) << 11
            | (self.imodd as u16) << 10
            | (self.pdlwrited as u16) << 9
            | (self.spushd as u16) << 8
            | (self.ir48 as u16) << 5
            | (self.nop as u16) << 4
            | (!self.vmaok as u16) << 3
            | (self.jcond as u16) << 2
            | (self.pcs1 as u16) << 1
            | self.pcs0 as u16
    }
}

/// What a read of register 3 gives: no read select is decoded there --- Y3
/// of SPY0 1F01 is not connected --- so no buffer drives `SPY<15:0>` and the
/// bus interface's 8304s read the floating bus as all ones.
pub const OPEN_READ: u16 = 0xffff;

// --- Checkpoints ------------------------------------------------------------

impl Mode {
    /// The register into a checkpoint, a flag a bit.
    pub fn save(&self, w: &mut crate::checkpoint::Writer) {
        let Mode { speed0, speed1, errstop, stathenb, trapenb, prom_disable } = *self;
        for bit in [speed0, speed1, errstop, stathenb, trapenb, prom_disable] {
            w.bool(bit);
        }
    }

    pub fn load(r: &mut crate::checkpoint::Reader) -> std::io::Result<Mode> {
        Ok(Mode {
            speed0: r.bool()?,
            speed1: r.bool()?,
            errstop: r.bool()?,
            stathenb: r.bool()?,
            trapenb: r.bool()?,
            prom_disable: r.bool()?,
        })
    }
}

impl ClockControl {
    pub fn save(&self, w: &mut crate::checkpoint::Writer) {
        let ClockControl { run, step, nop11, idebug, ldstat } = *self;
        for bit in [run, step, nop11, idebug, ldstat] {
            w.bool(bit);
        }
    }

    pub fn load(r: &mut crate::checkpoint::Reader) -> std::io::Result<ClockControl> {
        Ok(ClockControl {
            run: r.bool()?,
            step: r.bool()?,
            nop11: r.bool()?,
            idebug: r.bool()?,
            ldstat: r.bool()?,
        })
    }
}

impl OpcControl {
    pub fn save(&self, w: &mut crate::checkpoint::Writer) {
        let OpcControl { lpc_hold, opcclk, opcinh } = *self;
        for bit in [lpc_hold, opcclk, opcinh] {
            w.bool(bit);
        }
    }

    pub fn load(r: &mut crate::checkpoint::Reader) -> std::io::Result<OpcControl> {
        Ok(OpcControl { lpc_hold: r.bool()?, opcclk: r.bool()?, opcinh: r.bool()? })
    }
}
