// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `micro` engine: one step per microinstruction.
//!
//! It models the architecturally visible pipeline --- the two-deep fetch, the
//! inhibited cycle after a jump, the OA register merge, the two-cycle memory
//! data delay --- but not the clock phases or any signal that only the
//! diagnostic interface can see.  Those are what `rtl` and `chip` are for.
//!
//! Field positions and the functional source and destination codes are
//! `mit/cadr/ir.bits`, MIT's own tables.  The ALU is the drawings': page ALUC4
//! decides what the 74S181s are asked to do and page ALU0-1 does it, both
//! shared with the `rtl` engine; see `crate::ttl`.  There is one carry-out and
//! it is bit 32 of the 33-bit array.
//!
//! The jump conditions are the ALU's too --- page FLAG selects `AEQM` and
//! bit 32 of the 33-bit array, which is where the signedness comes from.
//!
//! Two things here are the board's rather than the obvious reading: a
//! dispatch on the map takes the map bit *instead of* the field's bit 0, off
//! the 74S64s at 2F24/2F05/2F23, and `MAP(MD)`'s permission bits come from
//! the map word of the last memory cycle, latched at VMEMDR 1D14, rather than
//! from `MD`'s page live.  `rtl` is this engine's reference
//! (`tests/cosim.rs`). Its clock is the
//! sum of the machine's microcycle periods --- the speed bits and `ILONG`,
//! two stages behind the mode register --- and not the waits and hangs on
//! the bus, which it does not have: a word is in `MD` two instructions
//! after the cycle starts whatever the memory is doing. `rtl` counts what
//! it waited, and `micro_keeps_the_machines_periods` holds the two clocks
//! to each other with the waits taken off; over the boot they are five
//! per cent of `rtl`'s time. `memory_cycle_ns` stands in for them: the
//! band's mean wait rounded, [`MEMORY_ACCESS_NS`], charged on every memory cycle so
//! that this clock runs near the machine's; a test that wants the periods
//! alone sets it to zero.
//!
//! So this engine has a clock but no timing model: nothing in it happens
//! at an instant or waits for one. Whatever the machine does that is a
//! matter of when --- the bus's waits and hangs, the interface's
//! arbitration and timeouts, a device answering late, the debug cable,
//! which is the interface's timing and nothing else --- is `rtl`'s and
//! `chip`'s, and this engine has no end of the cable.
//!
//! The pipeline and the memory timing are the part not written from the
//! drawings, and this engine is the wrong place to settle them: `rtl` and
//! `chip` compute them from the wiring, and `tests/cosim.rs` holds the
//! engines to each other.  The multiply and divide steps are not exercised by
//! the boot PROM; `tests/muldiv.rs` holds them to `rtl` and the netlist.

use crate::busint;
use crate::clock::Speed;
use crate::engine::Engine;
use crate::isa::{Insn, Op};
use crate::machine::{Halt, Machine, Word};
use crate::muldiv;
use crate::spy;
use crate::ttl;

/// What this engine charges its clock per memory cycle: the machine's mean
/// wait on the bus in the band, rounded to ten nanoseconds. `rtl` measures
/// 471,007,483 ns stalled over 910,304 memory cycles from executed
/// instruction 1,300,000 to 15,000,000 on the System 100 pack, 517 ns a
/// cycle, and `micro_keeps_the_machines_periods` in `tests/cosim.rs` holds
/// this to that measurement. The boot PROM's own mean is 545, over a run
/// too short to matter.
pub const MEMORY_ACCESS_NS: u64 = 520;

pub struct Micro {
    pub m: Machine,

    /// The instruction being executed, and the one already fetched behind it.
    p0: Insn,
    p0_pc: u16,
    p1: Insn,
    p1_pc: u16,
    npc: u16,

    inhibit: bool,
    popj: bool,
    /// A pop this microcycle whose word asks for an instruction fetch:
    /// `NEXT INSTR` on page CONTRL, which the edge registers.
    next_instr: bool,
    /// The same, registered: the fetch belongs to *this* microcycle.
    /// `lcinc = next_instrd || (irdisp && IR<24>)` in `rtl`.
    next_instrd: bool,

    oal: bool,
    oah: bool,
    oa_low: u64,
    oa_high: u64,

    new_md: Word,
    new_md_delay: u8,

    aaddr: u16,
    maddr: u8,
    adata: Word,
    mdata: Word,
    alu_out: Word,
    old_q: Word,
    out: Word,
    iwr: u64,

    executed: Option<u16>,

    /// `WMAP`, the level a `MAP(MD)VMA` store puts up, with `VMA` and `MD`
    /// as it leaves them: what the register below takes at the edge.
    map_write: Option<(Word, Word)>,
    /// `WMAPD`, the 74S374 at VCTL2 1C15 --- the same level a microcycle
    /// later, which is what gates the two write pulses.
    /// [`Micro::land_map_write`] is that write phase.
    map_write_d: Option<(Word, Word)>,
    /// The map as this microcycle's instruction reads it, when a map write
    /// landed at its start: the word from before the write.
    map_seen: Option<crate::machine::Translation>,
    /// The map word of the last memory cycle, as the 74S373 at VMEMDR 1D14
    /// holds it: what `MAP(MD)`'s permission bits are read from.
    lvmo: u32,
    /// Whether that cycle was a write, which is what makes a write
    /// permission fault a fault.
    wrcyc: bool,
    /// The speed the clock runs at, and the one the mode register asks for
    /// next: OLORD1 1A01 is two stages on `-TPR60`, so a new speed takes
    /// two microcycles to reach the tap select, as in `rtl`.
    speed: Speed,
    speed_a: Speed,
    /// A fixed charge to the clock for every memory cycle this engine
    /// starts, in nanoseconds: a stand-in for the waits the machine takes on
    /// the bus, which this engine does not model. [`MEMORY_ACCESS_NS`], the
    /// band's mean; a test sets it to zero to see the periods alone.
    pub memory_cycle_ns: u64,
    /// QUUX's microcycle, which has no delay lines: `sync`'s K ticks of
    /// 10 ns, in nanoseconds, in place of the speed's period; 0 on the
    /// CADR. [`Micro::new`] gives a QUUX machine four ticks, and `muir`
    /// sets `--sync-cycle-ticks`'.
    pub sync_cycle_ns: u64,
    /// Memory cycles started, for [`Micro::memory_cycles`].
    memory_cycles: u64,
    /// Microcycles the board spends nopped after a control-store write,
    /// still to be charged.
    nopped: u8,
    /// `SRUN`, `SSTEP` and `SSDONE`, the 74S174 at OLORD1 1A10 as `rtl` has
    /// it: the console's `RUN` and `STEP` one master clock behind, `STEP`
    /// twice.  A microcycle runs while `SRUN` is up, or for the one master
    /// clock in which `SSTEP` is up and `SSDONE` is not.
    srun: bool,
    sstep: bool,
    ssdone: bool,
    /// `HALTED`: the instruction executed asked for misc function 1,
    /// `HALT-CONS`, and `IR` holds it while the machine is halted.  Under
    /// `ERRSTOP` it stops the machine, as the 74S374 at OLORD2 1A05, the
    /// 74S133 at 1A02 and the run logic at OLORD1 1A15 do; see
    /// [`crate::rtl::Rtl`], which has the register itself.
    halted: bool,
    /// `MEMSTART`: a memory operation started in the microcycle before,
    /// which swings the map's address multiplexer from `MD` to `VMA`.
    memstart: bool,
    /// A memory operation started in this microcycle, for `memstart`.
    memop: bool,
    /// A write started and not yet gone out: the physical address, and
    /// when it goes out ([`Micro::start_write`]).
    write_out: Option<(u32, WriteOut)>,
    /// The PDL buffer write an instruction hands to the next microcycle's
    /// write phase, `PDLWRITED`: the address and the word, the address
    /// [`PDL_AT_INDEX`] for a write by PDL-INDEX.
    pdl_write: Option<(u16, Word)>,
    /// The SPC write the same way, `SPUSHD`: the pointer and the word.
    spc_write: Option<(u8, u32)>,
    /// The OPC shift register on page OPCS, eight deep, and its clock's
    /// last level; see [`Micro::opc_clock`].
    opc: [u16; 8],
    opc_ck: bool,
    /// The inhibit standing is the boot's trap and not a jump's `N`: the
    /// trap is in `NOP` but not in `NOPA` (CONTRL 3E14, 3E23), so the
    /// cycle it kills can still be long.
    trap: bool,
    /// The instruction being executed has pushed onto the SPC stack, which
    /// keeps its return from being fused ([`Micro::main_loop_return`]).
    /// Set and used within one step.
    pushed: bool,
    /// This microinstruction has pushed onto the SPC stack, and has popped
    /// it. The pointer counts once a microcycle, so a push and a pop in one
    /// microinstruction count it up once ([`Micro::push_spc`],
    /// [`Micro::pop_spc`]). Set and used within one step.
    spc_pushed: bool,
    spc_popped: bool,
    /// A write of destination 5, 6 or 7 (`crate::machine::macro_dispatch`),
    /// made at the end of the step: `rtl` writes them at the edge, so the
    /// instruction that writes one reads the register and the memory as
    /// they stood. Set and used within one step.
    macro_write: Option<(u32, u32)>,
    /// The operand address a fused return in this microcycle arms
    /// (`crate::machine::macro_dispatch`), handed to the machine at its
    /// edge. Set and used within one step.
    operand: Option<crate::machine::Operand>,
}

impl Micro {
    pub fn new(m: Machine) -> Self {
        // QUUX drops the delay lines: four ticks of 10 ns a microcycle.
        let sync_cycle_ns = if m.geometry.machine_id.is_some() {
            crate::clock::SYNC_CYCLE_TICKS as u64 * crate::clock::GRID_NS
        } else {
            0
        };
        let lvmo = m.geometry.lvmo_at_power_on();
        Micro {
            m,
            p0: Insn::new(0),
            p0_pc: 0,
            p1: Insn::new(0),
            p1_pc: 0,
            npc: 0,
            inhibit: false,
            popj: false,
            next_instr: false,
            next_instrd: false,
            oal: false,
            oah: false,
            oa_low: 0,
            oa_high: 0,
            new_md: 0,
            new_md_delay: 0,
            aaddr: 0,
            maddr: 0,
            adata: 0,
            mdata: 0,
            alu_out: 0,
            old_q: 0,
            out: 0,
            iwr: 0,
            executed: None,
            map_write: None,
            map_write_d: None,
            map_seen: None,
            lvmo,
            wrcyc: false,
            speed: Speed::ExtraSlow,
            speed_a: Speed::ExtraSlow,
            memory_cycle_ns: MEMORY_ACCESS_NS,
            sync_cycle_ns,
            memory_cycles: 0,
            nopped: 0,
            srun: false,
            sstep: false,
            ssdone: false,
            halted: false,
            memstart: false,
            memop: false,
            write_out: None,
            pdl_write: None,
            spc_write: None,
            opc: [0; 8],
            opc_ck: false,
            trap: false,
            pushed: false,
            spc_pushed: false,
            spc_popped: false,
            macro_write: None,
            operand: None,
        }
    }

    /// A microcycle's length: QUUX's `sync` period, or on the CADR the
    /// speed bits' and `ILONG`'s.
    fn cycle_ns(&self, ilong: bool) -> u64 {
        if self.sync_cycle_ns > 0 { self.sync_cycle_ns } else { self.speed.cycle_ns(ilong) as u64 }
    }

    /// The control store address executed in the last [`Engine::step`], or
    /// `None` if that cycle was inhibited.  An inhibited cycle runs on the
    /// board and retires nothing, so it is not an executed instruction.
    pub fn executed(&self) -> Option<u16> {
        self.executed
    }

    /// The master clock edge, as far as this engine has one: the run and
    /// step synchronizers, and the two pulses a mode-register write can
    /// make.  See `Rtl::mclk_edge`, which this follows.
    fn mclk_edge(&mut self) {
        // The I/O board's own clock runs on between the processor's
        // references to it: the mouse's lines under `KB CLK^`, the serial
        // port's characters, and the Chaosnet's frames off the cable.  A
        // register access advances the device it names by itself, but the
        // Chaosnet's `RECEIVE DONE` and `Transmit Done` are set nowhere
        // else, and `IoBoard::interrupt_request` only reads them --- so an
        // engine that does not do this is an engine whose Chaosnet can
        // never interrupt.  `tests/ioboard_clock.rs` holds every engine to
        // it.  What this engine's clock is worth is said above: the
        // machine's periods and `MEMORY_ACCESS_NS`, not the measured
        // waits, so the board's clock runs a little fast against `rtl`'s.
        self.m.ioboard.advance(self.m.ns);
        // QUUX's file device completes what is due at this edge, before
        // the next microcycle begins (contract Q9).
        self.m.advance_file_device();
        let boot = std::mem::take(&mut self.m.prog_boot);
        let reset = std::mem::take(&mut self.m.prog_reset) || boot;
        if reset {
            self.m.reset_console_registers();
            // `-RESET` and `-BOOT` put every interval timer in its reset
            // state (contract Q11).
            self.m.timers = crate::machine::Timers::new();
            // `-RESET` clears QUUX's MACRO-DISPATCH enable, and drops an
            // armed operand address (contract H8a).
            self.m.macro_dispatch.reset();
            // `-RESET` clears `MEMSTART` (`Rtl::reset`).
            self.write_out = None;
        }
        if boot {
            self.m.vmaok = false;
            self.m.clock_control.run = true;
            self.npc = self.m.reset_pc();
            self.inhibit = true;
            self.trap = true;
            // The trap nops the instruction a pending OA write modified,
            // and `-RESET` clears `IMODD` at PDLCTL 4C11 (`Rtl::reset`):
            // no modification outlives the boot, as in `boot` below.
            self.oal = false;
            self.oah = false;
        }
        self.ssdone = self.sstep;
        self.sstep = self.m.clock_control.step;
        self.srun = self.m.clock_control.run;
    }

    /// The speed select, two stages behind the mode register, as `rtl` has
    /// it off OLORD1 1A01.
    fn speedclk(&mut self) {
        // QUUX runs at one rate, the CADR's normal until its own timing
        // model says otherwise; it has no speed bits to synchronize.
        if !self.m.geometry.speed_bits {
            (self.speed, self.speed_a) = (Speed::Normal, Speed::Normal);
            return;
        }
        self.speed = self.speed_a;
        self.speed_a = match (self.m.mode.speed1, self.m.mode.speed0) {
            (false, false) => Speed::ExtraSlow,
            (false, true) => Speed::Slow,
            (true, false) => Speed::Normal,
            (true, true) => Speed::Fast,
        };
    }

    /// `IR<pos+len-1:pos>` of the instruction being executed.
    fn ir(&self, pos: u32, len: u32) -> u32 {
        ((self.p0.raw() >> pos) & ((1u64 << len) - 1)) as u32
    }

    fn advance_pipeline(&mut self) {
        // `IR` loads from the debug IR instead of the control store while
        // the console holds `IDEBUG` up.
        self.p0 = if self.m.clock_control.idebug { Insn::new(self.m.debug_ir) } else { self.p1 };
        self.p0_pc = self.p1_pc;
        self.p1 = self.m.fetch(self.npc);
        self.p1_pc = self.npc;
        self.npc = if self.npc == 0o37777 { 0 } else { self.npc + 1 };
        self.m.opc = self.p0_pc;
        self.opc_clock(false, self.p0_pc);
        self.opc_clock(true, self.p0_pc);
    }

    /// The OPCS shift registers' clock, as `Rtl::opc_clock` has it: the
    /// 9328s at OPCS 1F06-1F13 clock on `OPCINH OR (CLK5 AND -OPCCLK)`, so
    /// with both control bits down the history shifts every microcycle,
    /// `OPCINH` freezes it, and on a halted machine the console steps it by
    /// lowering `OPCCLK`.  What shifts in is the PC.
    fn opc_clock(&mut self, clk5: bool, pc: u16) {
        let o = self.m.opc_control;
        let ck = o.opcinh || (clk5 && !o.opcclk);
        if ck && !self.opc_ck {
            self.opc.copy_within(0..7, 1);
            self.opc[0] = pc;
        }
        self.opc_ck = ck;
    }

    /// Where the map is looked up for `MAP(MD)` and a dispatch on a map
    /// bit: `MAPI` is the 74S258s at VMAS 1C16 and 1C20, whose select is
    /// `-MEMSTART`, so it is `VMA` through the microcycle after a memory
    /// operation and `MD` otherwise.
    fn map_address(&self) -> u32 {
        (if self.memstart { self.m.vma } else { self.m.md }) as u32
    }

    /// The write phase of this microcycle, which writes what the one
    /// before it asked for: the PDL buffer under `PDLWRITED` and the SPC
    /// stack under `SPUSHD`, both registered, after this microcycle has read
    /// them with `CLK` high (`Rtl::write_phase`).  There is no pass-around
    /// into `M` for either, so an instruction that reads one right after a
    /// write to it gets the word that was there.
    ///
    /// A write by PDL-INDEX goes where PDL-INDEX stands when it lands, as
    /// `PWIDX` addresses it (`Rtl::write_phase`): on the CADR nothing can
    /// move the index in between, and on QUUX a fused return's operand
    /// address can (`crate::machine::macro_dispatch`).
    fn land_writes(&mut self) {
        if let Some((adr, word)) = self.pdl_write.take() {
            let adr = if adr == PDL_AT_INDEX { self.m.pdl_index } else { adr };
            self.m.pdl[adr as usize] = word;
        }
        if let Some((ptr, word)) = self.spc_write.take() {
            self.m.spc[ptr as usize] = word;
        }
    }

    /// A push onto the SPC stack: the pointer moves at the edge and the word
    /// waits for the next write phase.
    ///
    /// The pointer counts once a microcycle, and up when the microcycle
    /// pushes, whatever else pops in it. Page CONTRL: `-SPCNT` is the
    /// open-collector 74S08 at 4D09 over `-SPUSH` and `-SPOP`, and page
    /// SPC's 74S169s at 4F23 and 4F28 take it on `-ENT` and `SPUSH` on
    /// `U/-D` (`cadrwd/cadr4.wlr`, nets `-SPCNT` and `SPUSH`). So a pop
    /// already taken in this microinstruction, by the functional source,
    /// is no count of its own, and the push counts up from where the edge
    /// found the pointer.
    fn push_spc(&mut self, word: u32) {
        self.pushed = true;
        if self.spc_popped {
            self.m.spcptr = (self.m.spcptr + 1) & 0o37;
        }
        self.spc_pushed = true;
        self.m.spcptr = (self.m.spcptr + 1) & 0o37;
        self.spc_write = Some((self.m.spcptr, word));
    }

    /// A pop for the next address.  A word still waiting to be written is
    /// what the stack gives: `SPCWPASS` at CONTRL 3D21 puts it on the `SPC`
    /// bus, which feeds the next-address path.
    ///
    /// After a push in this microinstruction the pointer has counted up
    /// and does not count down ([`Micro::push_spc`]), and the pop takes the
    /// word at the pointer the edge found, below the pushed one:
    /// `SPCWPASS` is `SPUSHD`'s, the push of the microcycle *before*,
    /// registered by the 74S175 at CONTRL 3D26, and the pushed word is
    /// written only in the next microcycle.
    ///
    /// After the functional source's pop in this microinstruction the
    /// pointer has counted down once and does not count again, and this
    /// pop takes the word the source read, the old top: page CONTRL's
    /// `-SPOP` is one output, the 74S64 at 3E28, pulled by `POPJ OR
    /// SRCSPCPOPREAL` (the 74S00 at 3E23) as by the returns, and `-SPCNT`
    /// at 4D09 one count (`cadrwd/cadr4.wlr`, nets `-SPOP` and `-SPCNT`);
    /// both read the one SPC bus.
    fn pop_spc(&mut self) -> u32 {
        if self.spc_pushed {
            self.spc_popped = true;
            return self.m.spc[(self.m.spcptr.wrapping_sub(1) & 0o37) as usize];
        }
        if self.spc_popped {
            return self.m.spc[((self.m.spcptr + 1) & 0o37) as usize];
        }
        let ptr = self.m.spcptr;
        let v = match self.spc_write {
            Some((p, word)) if p == ptr => word,
            _ => self.m.spc[ptr as usize],
        };
        self.m.spcptr = ptr.wrapping_sub(1) & 0o37;
        self.spc_popped = true;
        v
    }

    /// Byte position for the LC byte modes, `IR<11:10> == 3`: `IR<4:3>`
    /// with the location counter's low two bits folded in, so that the
    /// instruction picks the halfword or the byte `LC` points at.
    ///
    /// The gates are on page LC of `data/CADR.netlist`, and `rtl` computes
    /// them net for net:
    ///
    /// ```text
    /// 3E11  74S00   -LC MODIFIES MROT = NAND(IR10, IR11)
    /// 2E05  74S86   INST IN LEFT HALF = NOR(-LC MODIFIES MROT, LC1 XOR LC0B)
    /// 2E05  74S86   -SH4 = INST IN LEFT HALF XOR -IR4
    /// 2E30  74S02O  INST IN 2ND OR 4TH QUARTER
    ///                 = AND(NOR(-LC MODIFIES MROT, LC0), LC BYTE MODE)
    /// 2E05  74S86   -SH3 = -IR3 XOR INST IN 2ND OR 4TH QUARTER
    /// ```
    ///
    /// with `LC0B` being `LC0 AND LC BYTE MODE`. So `SH4` is `IR4` unless
    /// `LC1 XOR LC0B` is up, and `SH3` is `IR3` unless `LC0` is *down* in
    /// byte mode: `LC` = 1 selects byte 0, 2 byte 1, 3 byte 2 and 4 byte 3,
    /// which is what CC's `CC-TEST-LC-DP` in `sys/cc/diags.lisp` expects
    /// ("Select byte (initially rightmost, LC=current+1)"). The boot never
    /// enters byte mode; `tests/cosim.rs` holds the engines to each other
    /// on it.
    fn lc_byte_mode(&self) -> u32 {
        self.lc_rotation(self.m.lc, self.ir(0, 5))
    }

    /// The rotate `IR<4:0>` = `rotate` gives under `IR<11:10>` = 3 with the
    /// location counter at `lc`: [`Micro::lc_byte_mode`]'s gates.
    ///
    /// **Revision 13** adds instead, in the ring of 40 (contract G2 §2.3,
    /// appendix A1.2): `rotate`, the word's 6-bit rotate, plus 0 or 24 for
    /// halfwords 0 and 1, keyed on `LC<1>` = 1 and 0; and in byte mode 0,
    /// 32, 24 and 16 for `LC<1:0>` = 1, 2, 3 and 0, bytes 0 to 3 in stream
    /// order; mod 40.
    fn lc_rotation(&self, lc: u32, rotate: u32) -> u32 {
        if self.m.geometry.wide() {
            let add = if self.m.byte_mode() {
                [16, 0, 32, 24][(lc & 3) as usize]
            } else if lc & 2 != 0 {
                0
            } else {
                24
            };
            return (rotate + add) % 40;
        }
        let ir4 = (rotate >> 4) & 1;
        let ir3 = (rotate >> 3) & 1;
        let lc1 = (lc >> 1) & 1;
        let lc0 = lc & 1;
        if self.m.byte_mode() {
            (rotate & 7) | ((ir4 ^ lc1 ^ lc0 ^ 1) << 4) | ((ir3 ^ lc0 ^ 1) << 3)
        } else {
            (rotate & 0o17) | ((ir4 ^ lc1 ^ 1) << 4)
        }
    }

    /// A pop with `<14>` up: to the handler the MACRO DISPATCH MEMORY names
    /// if the return is fused (`crate::machine::macro_dispatch`), and
    /// otherwise as [`Micro::pop_asks_for_a_fetch`] says. `advance` is a
    /// dispatch's `IR<24>`, which has stepped the counter already.
    ///
    /// Fused only where today's return goes to the main loop's dispatch,
    /// no fetch needed (this engine has no cache, so not QUUX's
    /// prefetch, which `rtl` fuses on: the two differ in timing there, not
    /// in results, where the microcode keeps the rule after a fused
    /// return), and nothing in this microinstruction changes what
    /// that dispatch would see: no push, no pop by the functional source,
    /// no write of M 31 or INTERRUPT-CONTROL, and no step of the counter in
    /// this microcycle (`LCINC`: `NEXT INSTRD`, or the dispatch's
    /// `IR<24>`). The counter steps a microcycle later, so the halfword is
    /// chosen by it as stepped.
    fn main_loop_return(&mut self, word: u32, advance: bool) -> u32 {
        let target = self.pop_asks_for_a_fetch(word);
        if !self.m.geometry.macro_dispatch
            || self.needfetch()
            || advance
            || self.next_instrd
            || self.pushed
            || self.pops_by_source()
            || self.writes_m31_or_interrupt_control()
        {
            return target;
        }
        let inc = if self.m.byte_mode() { 1 } else { 2 };
        let counter = self.m.geometry.lc_counter();
        let stepped = (self.m.lc & counter).wrapping_add(inc) & counter;
        let wide = self.m.geometry.wide();
        let index_rotate = crate::machine::macro_dispatch::index_rotate(wide);
        let rotate = self.lc_rotation(stepped, index_rotate);
        let rotated = if wide {
            rol40(self.m.mmem[0o31], rotate)
        } else {
            rol(self.m.mmem[0o31] as u32, rotate).into()
        };
        match self.m.macro_dispatch.fused_return(word, rotated, index_rotate) {
            Some(f) => {
                if f.keep {
                    self.m.spcptr = (self.m.spcptr + 1) & 0o37;
                }
                self.m.macro_dispatch.fused += 1;
                self.operand = f.operand;
                f.handler as u32
            }
            None => target,
        }
    }

    /// A pop by a jump with R, fused as [`Micro::main_loop_return`] says
    /// only while `macro_dispatch::JUMP_RETURNS_FUSE` does
    /// (`crate::machine::macro_dispatch`).
    fn jump_return(&mut self, word: u32) -> u32 {
        if crate::machine::macro_dispatch::JUMP_RETURNS_FUSE {
            self.main_loop_return(word, false)
        } else {
            self.pop_asks_for_a_fetch(word)
        }
    }

    /// Whether the instruction reads functional source 14, the SPC's pop:
    /// `IR<31>`, and `IR<29:26>` 14 (`IR<30>` is in no decode).
    fn pops_by_source(&self) -> bool {
        self.p0.m_src_functional() && self.maddr & 0o17 == 0o14
    }

    /// Whether the instruction, an ALU or BYTE one with `IR<25>` clear,
    /// writes M 31 (`IR<18:14>`) or INTERRUPT-CONTROL (`IR<23:19>` 2).
    fn writes_m31_or_interrupt_control(&self) -> bool {
        matches!(self.p0.op(), Op::Alu | Op::Byte)
            && self.ir(25, 1) == 0
            && (self.ir(14, 5) == 0o31 || self.ir(19, 5) == 0o2)
    }

    /// Steps the location counter, fetching the next instruction word when
    /// `NEEDFETCH` calls for one.
    ///
    /// The hardware derives `NEEDFETCH` rather than storing it, and the
    /// netlist names every term of it on page LC:
    ///
    /// ```text
    /// 1E07  74S08   LC0B = 'LC BYTE MODE' AND LC0
    /// 3E17  74S02O  'LAST BYTE IN WORD' = NOR(LC1, LC0B)
    /// 3E09  74S32   NEEDFETCH = 'HAVE WRONG WORD' OR 'LAST BYTE IN WORD'
    /// ```
    ///
    /// This engine has no `'HAVE WRONG WORD'`: that term is `-NEWLC` NAND
    /// `-DESTLC` at 3E11, which only a write to the counter raises. So
    /// `NEEDFETCH` is carried as `LC<31>` instead --- set when the step lands
    /// on the last byte of a word, cleared by the fetch it asks for. `rtl`
    /// and `chip` compute the gates themselves, and `tests/cosim.rs` holds
    /// the three engines to one trace.
    ///
    /// The end of a microcycle: the fetch the microcycle before armed,
    /// then the edge that registers this one's `NEXT INSTR` and `WMAP`.
    ///
    /// The fetch comes after everything the instruction did, because page
    /// VCTL1's `VMAS` multiplexer gives `IFETCH` the last word on `VMA`:
    /// `vmaenb = destvma | ifetch` and `vmas` is `LC<25:2>` whenever
    /// `IFETCH`, whatever the instruction wanted to put there.
    ///
    /// A write started in this microcycle goes out first, if the next
    /// microcycle leaves `MD` alone, and one due goes out last, with `MD`
    /// as this microcycle leaves it ([`Micro::start_write`]).
    fn fetch_and_clock(&mut self) {
        // QUUX's operand address: armed by a fused return at the edge
        // ending its microcycle, loaded into PDL-INDEX at the edge ending
        // the next, after anything that microcycle wrote there
        // (`crate::machine::macro_dispatch`).
        if let Some(o) = self.m.macro_dispatch.operand.take() {
            let adr = self.m.macro_dispatch.operand_address(o);
            self.m.pdl_index = adr as u16 & self.m.geometry.pdl_mask();
        }
        self.m.macro_dispatch.operand = self.operand.take();
        if let Some((physical, WriteOut::Started)) = self.write_out {
            if self.next_microcycle_holds_the_write() {
                self.write_out = Some((physical, WriteOut::Next));
            } else {
                self.write_goes_out();
            }
        }
        if self.next_instrd {
            self.step_lc();
        }
        self.next_instrd = std::mem::take(&mut self.next_instr);
        self.clock_map_write();
        match self.write_out {
            Some((_, WriteOut::Due)) => self.write_goes_out(),
            Some((physical, WriteOut::Next)) => self.write_out = Some((physical, WriteOut::Due)),
            _ => {}
        }
    }

    /// A pop whose word has bit 14 up: the counter steps and, if
    /// `NEEDFETCH` is pending, the instruction fetch happens --- but a
    /// microcycle later, so this arms `NEXT INSTR` rather than doing it.
    /// `rtl`: `next_instr = spop && !srcspcpopreal && SPC<14>`, registered
    /// at this edge, and `lcinc = next_instrd || (irdisp && IR<24>)` with
    /// `ifetch = needfetch && lcinc` the microcycle after.
    ///
    /// What does happen here is `SPCMUNG`: with the bit up and no fetch
    /// needed, the return address comes back with bit 1 forced, which
    /// steps the return over the two instructions of the fetch. MIT's own
    /// `uc-macrocode.lisp` says so of the main loop --- "QMLP MUST BE AT
    /// LOC WITH BIT 1=0.  QMLP AND QMLP+1 ARE SKIPPED BY STREAM HARDWARE
    /// AUTOMATICALLY IF NO FETCH REQUIRED" --- and `rtl` makes it in the
    /// `POPJ`'s own read phase, off the counter before the edge steps it:
    /// `spcmung = SPC<14> && !needfetch`, `spc1a = spcmung || SPC<1>`.
    ///
    /// A pop the functional source takes too arms nothing, though the
    /// return still takes `SPCMUNG`: page LCC's `NEXT.INSTR` is the 74S02
    /// at 3E17 over `-SPOP` and the 74S00 at 3E07's `NAND(SPC14,
    /// -SRCSPCPOPREAL)`, so `SPOP AND SPC14 AND NOT SRCSPCPOPREAL`,
    /// registered as `NEXT.INSTRD` by the 74S175 at 3E12
    /// (`cadrwd/cadr4.wlr`, nets `NEXT.INSTR`, `SPC14` and
    /// `-SRCSPCPOPREAL`).
    fn pop_asks_for_a_fetch(&mut self, word: u32) -> u32 {
        if !self.pops_by_source() {
            self.next_instr = true;
        }
        if self.needfetch() { word } else { word | 2 }
    }

    /// `NEEDFETCH` as this engine carries it: `LC<31>`, set when the last
    /// step landed on the last byte of a word and cleared by the fetch it
    /// asks for.  `rtl` and `chip` compute the gates.
    fn needfetch(&self) -> bool {
        self.m.lc & (1 << 31) != 0
    }

    fn step_lc(&mut self) {
        // LC counts bytes and a word is four of them, so the word to fetch is
        // the counter *before* the step, shifted down by two. The counter is
        // `LC<25:0>` (page LC); the flags this engine keeps above it stay.
        // Revision 13's counter is `LC<29:0>` (A1.6), and a fetch takes
        // `LC<29:2>`.
        let counter = self.m.geometry.lc_counter();
        let fetch_from = (self.m.lc & counter) >> 2;
        let inc = if self.m.byte_mode() { 1 } else { 2 };
        let lc = (self.m.lc & counter).wrapping_add(inc) & counter;
        self.m.lc = (self.m.lc & !counter) | lc;

        if self.needfetch() {
            self.m.lc &= !(1 << 31);
            // `IFETCH` is a term of `MEMOP` on page VCTL1 and `VMAS` is
            // `LC<25:2>` under it, so the fetch is a memory cycle like any
            // read: the map word latched, the clock charged, `MD` loaded
            // only if the map permits.
            self.m.vma = fetch_from.into();
            self.start_read();
        }

        // 1E07 and 3E17, on the counter as stepped.
        let lc0b = self.m.byte_mode() && (self.m.lc & 1 != 0);
        let last_byte_in_word = !lc0b && (self.m.lc & 2 == 0);
        if last_byte_in_word {
            self.m.lc |= 1 << 31;
        }
    }

    /// Functional sources, `IR<30:26>` when `IR<31>` is set.
    ///
    /// The codes and the names below are MIT's own `FUNCTIONAL SOURCES` table
    /// in `mit/cadr/ir.bits`, which reads, in octal:
    ///
    /// ```text
    /// 0 Dispatch Constant     4 Illegal (Pdl)   10 VMA       14 SPC ptr & data, pop
    /// 1 SPC pointer and data  5 Pdl Buffer (X)  11 MAP(MD)   15 -
    /// 2 Pdl Buffer Pointer    6 OPC             12 MD        16 -
    /// 3 Pdl Buffer Index      7 Q               13 Location Counter    17 -
    ///                        24 Pdl Buffer Pop
    ///                        25 Pdl Buffer (P)
    /// ```
    ///
    /// `(X)` is the PDL addressed by the index and `(P)` by the pointer.
    /// Code 4 MIT calls illegal, and 15 to 17 it leaves unassigned.
    fn read_functional(&mut self, source: u8) -> Result<Word, Halt> {
        // The SPC word carries the pointer above the entry it selects.
        let spc_word =
            |m: &Machine| ((m.spcptr as u32) << 24) | (m.spc[m.spcptr as usize] & 0o1777777);
        // Page SOURCE decodes `IR<31>`, `IR<29>` and `IR<28:26>`: two
        // 74S138s, one under `-IR29` for sources 0 to 7 and one under `IR29`
        // for 10 to 17. `IR<30>` is not in the decode; on page PDLCTL it is
        // `PDLP` while `CLK` is up, which reads the PDL buffer by its pointer
        // rather than by its index. So 24 and 25 are 4 and 5 read by the
        // pointer, and 26 is 6, the OPC, with a bit that does nothing there.
        let by_pointer = source & 0o20 != 0;
        let pdl_at =
            |m: &Machine| if by_pointer { m.pdl_pointer as usize } else { m.pdl_index as usize };
        Ok(match source & 0o17 {
            // Dispatch Constant
            0o0 => self.m.dispatch_constant.into(),
            // SPC pointer and data
            0o1 => spc_word(&self.m).into(),
            // Pdl Buffer Pointer, Pdl Buffer Index
            0o2 => (self.m.pdl_pointer & self.m.geometry.pdl_mask()).into(),
            0o3 => (self.m.pdl_index & self.m.geometry.pdl_mask()).into(),
            // Pdl Buffer Pop: by the pointer as 24, or by the index as 4,
            // MIT's `Illegal (Pdl)`, the pointer counting down either way
            // (`PDLCNT` on page PDLCTL).
            0o4 => {
                let v = self.m.pdl[pdl_at(&self.m)];
                self.m.pdl_pointer =
                    self.m.pdl_pointer.wrapping_sub(1) & self.m.geometry.pdl_mask();
                v
            }
            // Pdl Buffer (P) as 25, Pdl Buffer (X) as 5.
            0o5 => self.m.pdl[pdl_at(&self.m)],
            // OPC, Q.  Page OPCD drives `MF<13:0>` from the shift register's
            // last stage, eight microcycles back.
            0o6 => self.opc[7].into(),
            0o7 => self.m.q,
            // VMA
            0o10 => self.m.vma,
            // MAP(MD).  Bit 29 is **zero**: VMEMDR 1A01 drives it from `HI12`
            // through a 74S240, and the '240 inverts; read off the drawing
            // without the buffer it would be a one.
            //
            // Bits 31 and 30 are `-PFW` and `-PFR`, and on the board they
            // come off the 74S373 at VMEMDR 1D14: the map word of the *last
            // memory cycle*, not of `MD`'s page now, a write fault being one
            // only if that cycle was a write. Computing them from `MD` live
            // is the easy misreading; `rtl` and `chip` have the latch, and
            // `TRANS-OLD0`'s `DISPATCH L2-MAP-STATUS-CODE` reads exactly the
            // bits of the last cycle's page. The rest is live, at whatever
            // `MAPI` addresses: `VMA` just after a start, `MD` otherwise.
            // Revision 13's layout is A1.7's: the level-1 entry in
            // `<38:32>`, the two fault bits in `<31:30>` from the latched
            // entry's `<27:26>`, and the 28-bit level-2 entry in `<27:0>`.
            0o11 if self.m.geometry.wide() => {
                let t = self.map_seen.unwrap_or_else(|| self.m.translate(self.map_address()));
                let pfr = (self.lvmo >> 27) & 1 != 0;
                let pfw = !((self.lvmo >> 26) & 1 == 0 && self.wrcyc);
                Word::from(t.l1_data & 0o177) << 32
                    | Word::from((!pfw as u32) << 31 | (!pfr as u32) << 30)
                    | Word::from(t.l2_data & crate::machine::MAP_LEVEL_2_13)
            }
            0o11 => {
                let t = self.map_seen.unwrap_or_else(|| self.m.translate(self.map_address()));
                let pfr = (self.lvmo >> 23) & 1 != 0;
                let pfw = !((self.lvmo >> 22) & 1 == 0 && self.wrcyc);
                (((!pfw as u32) << 31)
                    | ((!pfr as u32) << 30)
                    | ((t.l1_data & self.m.geometry.l1_mask()) << 24)
                    | (t.l2_data & 0o77777777))
                    .into()
            }
            // MD
            0o12 => self.m.md,
            // Location Counter.  Bit 0 only means anything in byte mode.
            // Revision 13's layout is A1.6's: NEED-FETCH in `<39>`, the
            // four flags in `<37:34>`, the counter in `<29:0>`.
            0o13 if self.m.geometry.wide() => {
                let counter = self.m.lc & crate::machine::LC_COUNTER_13;
                let counter = if self.m.byte_mode() { counter } else { counter & !1 };
                Word::from(self.needfetch()) << 39
                    | Word::from(self.m.interrupt_control & (0o17 << 26)) << 8
                    | Word::from(counter)
            }
            0o13 => (if self.m.byte_mode() { self.m.lc } else { self.m.lc & !1 }).into(),
            // SPC ptr & data, pop
            0o14 => {
                let v = spc_word(&self.m);
                self.m.spcptr = self.m.spcptr.wrapping_sub(1) & 0o37;
                self.spc_popped = true;
                v.into()
            }
            // QUUX's MACHINE-ID, where it has one (`Geometry::QUUX`).
            0o16 if self.m.geometry.machine_id.is_some() => {
                self.m.geometry.machine_id.unwrap_or(!0).into()
            }
            // QUUX's microsecond clock (`machine::Timers`).
            0o15 if self.m.geometry.tick => crate::machine::Timers::microseconds(self.m.ns).into(),
            // Functional sources 0o15, 0o16 and 0o17: the 74S138 for the
            // upper eight has those three outputs unconnected, so no part
            // drives the M bus and an undriven TTL bus reads high, as `chip`
            // shows. Microcode 323 reads 0o15 once, at 0o20535. On QUUX,
            // 0o17 too (contract Q11). All the word's bits: 32 on the CADR,
            // 40 on QUUX.
            _ => self.m.geometry.word_mask(),
        })
    }

    /// Functional destinations, `dest >> 5` of `IR<25:14>`.
    ///
    /// MIT's own `FUNCTIONAL DESTINATIONS` table in `mit/cadr/ir.bits`, in octal:
    ///
    /// ```text
    ///  0 Nowhere            10 Pdl Buffer Top      20 VMA
    ///  1 Location Counter   11 Pdl Buffer Push     21 VMA, start read
    ///  2 Interrupt Control  12 Pdl Buffer (Index)  22 VMA, start write
    ///  3 -                  13 Pdl Buffer Index    23 VMA, MAP(MD)VMA
    ///  4 -                  14 Pdl Buffer Pointer  30 MD
    ///  5 -                  15 SPC, push           31 MD, start read (!)
    ///  6 -                  16 IMOD<25:0>          32 MD, start write
    ///  7 -                  17 IMOD<47:26>         33 MD, MAP(MD)VMA
    /// ```
    ///
    /// **The map write is a microcycle late.**  `ir.bits`, on destination
    /// `23`: "The write actually occurs on the cycle following the store
    /// into destination WRITE-MAP, and the VMA must not be disturbed during
    /// this cycle for proper operation."  So the store arms it here with
    /// `VMA` and `MD` as it leaves them --- which is what the board's own
    /// registers hold for the whole of the next microcycle, `-WP1` firing
    /// before the edge that would change them --- and
    /// [`Micro::land_map_write`] performs it at the head of that microcycle.
    /// A memory reference in between reads the old map, as it does on the
    /// board and in `rtl`.
    ///
    /// `MEMSTART` swings the map's address multiplexer from `MD` to `VMA`
    /// through the microcycle after a memory operation, which is what MIT's
    /// warning is about.  The readers of the map follow it
    /// ([`Micro::map_address`]); the write has no case to decide.  The one
    /// memory operation a `MAP(MD)VMA` store's
    /// microcycle can carry is an instruction fetch, and page VCTL1's
    /// `VMAS` multiplexer then loads `VMA` with the fetch address ---
    /// `LC<25:2>`, twenty-four bits, with neither write enable, `VMA<26>`
    /// nor `VMA<25>` in it --- so the pulses find nothing to write at
    /// whichever address they are given.
    ///
    /// The latch is not what keeps that case straight, though it once was:
    /// a `POPJ`'s fetch comes the microcycle *after* the `POPJ` on every
    /// engine now (issue #21), which is the write's own microcycle, and
    /// [`Micro::land_map_write`] runs at the head of it, before anything
    /// in it can move `VMA`.  What the latch is for is simply holding the
    /// store's `VMA` and `MD` across the edge, which is what the board's
    /// registers do: `-WP1` fires before the edge that would change them.
    /// Held to `rtl` in `tests/cosim.rs`.
    fn arm_map_write(&mut self) {
        self.map_write = Some((self.m.vma, self.m.md));
    }

    /// `MAPWR0D` and `MAPWR1D` fire from `WMAPD`, in the microcycle after
    /// the store: before this microcycle's instruction, so that a memory
    /// reference in it goes through the new entry, as `rtl` has it --- there
    /// the write is `Rtl::write_phase`'s and the lookup of a cycle prepared
    /// in the same microcycle is a read phase later still.  After the store's
    /// own microcycle, so that nothing between the two sees the entry.
    fn land_map_write(&mut self) {
        if let Some((vma, md)) = self.map_write_d.take() {
            self.m.write_map(vma, md);
        }
    }

    /// The edge at the end of a microcycle, taking `WMAP` into `WMAPD`.
    /// Two stores running back to back each get their own write, one
    /// microcycle behind them, as the register gives them.
    fn clock_map_write(&mut self) {
        self.map_write_d = self.map_write.take();
    }

    /// The `(!)` on 31 is MIT's, not ours.
    ///
    /// The codes MIT leaves unassigned decode as page SOURCE decodes them
    /// (`Rtl::read_phase`): `IR<24>`, "xx" in `ir.bits`, is in no decode;
    /// 3 to 7 are the low group with no decoder output, so only M is
    /// written; and the memory group decodes `IR<20:19>` without `IR<21>`,
    /// so 24 to 27 are 20 to 23 and 34 to 37 are 30 to 33.
    fn write_functional(&mut self, dest: u16, word: Word) -> Result<(), Halt> {
        // The destinations that are not words take `<31:0>`.
        let data = word as u32;
        let code = (dest >> 5) & 0o37;
        let code = if code & 0o20 != 0 { code & !0o4 } else { code };
        match code {
            // Nowhere
            0o0 => {}
            // LOCATION-COUNTER.  Writing it always sets NEED-FETCH.
            0o1 => {
                let counter = self.m.geometry.lc_counter();
                self.m.lc = (self.m.lc & !counter) | (data & counter);
                if !self.m.byte_mode() {
                    self.m.lc &= !1;
                }
                self.m.lc |= 1 << 31;
            }
            // INTERRUPT-CONTROL.  Bit 28 is `PROG.UNIBUS.RESET`, the 25LS2519
            // at FLAG 3E08: as it rises the model I/O boards are reset,
            // `Machine::bus_reset`; this engine has no bus interface or
            // memory boards for it to hold.  The flag bits are mirrored
            // into LC. QUUX has no `PROG.UNIBUS.RESET` (contract Q11): the
            // bit is kept and read back, and drives nothing; its devices
            // are reset by the register page's word 104.
            // Revision 13 takes the four flags from `<37:34>` (A1.6), and
            // the location counter source reads them from here.
            0o2 if self.m.geometry.wide() => {
                self.m.interrupt_control = (word >> 8) as u32 & (0o17 << 26);
            }
            0o2 => {
                let was = self.m.interrupt_control & (1 << 28) != 0;
                self.m.interrupt_control = data;
                self.m.lc = (self.m.lc & !(0o17 << 26)) | (data & (0o17 << 26));
                if !was && data & (1 << 28) != 0 && self.m.geometry.unibus {
                    self.m.bus_reset();
                }
            }
            // Destinations 3 and 4 write only M, on the CADR and on QUUX
            // (contract Q11): the register page's timers take the place of
            // Q1's tick control and interval period (`machine::Timers`).
            // Destinations 5 to 7 are QUUX's MACRO-DISPATCH register and
            // MACRO DISPATCH MEMORY's index and entry
            // (`machine::macro_dispatch`), written at the end of the step;
            // on the CADR they too write only M.
            0o5..=0o7 if self.m.geometry.macro_dispatch => {
                self.macro_write = Some((code as u32, data));
            }
            // Pdl Buffer Top, Push, (Index), Index, Pointer
            // The word is written in the next microcycle's write phase,
            // [`Micro::land_writes`].
            0o10 => self.pdl_write = Some((self.m.pdl_pointer, word)),
            0o11 => {
                self.m.pdl_pointer = (self.m.pdl_pointer + 1) & self.m.geometry.pdl_mask();
                self.pdl_write = Some((self.m.pdl_pointer, word));
            }
            0o12 => self.pdl_write = Some((PDL_AT_INDEX, word)),
            0o13 => self.m.pdl_index = data as u16 & self.m.geometry.pdl_mask(),
            0o14 => self.m.pdl_pointer = data as u16 & self.m.geometry.pdl_mask(),
            // SPC, push
            0o15 => self.push_spc(data),
            // IMOD<25:0> and IMOD<47:26>: the OA register merge into the
            // next instruction.
            0o16 => {
                self.oa_low = data as u64 & 0o377777777;
                self.oal = true;
            }
            0o17 => {
                self.oa_high = data as u64 & 0o37777777;
                self.oah = true;
            }
            // VMA, and the three that start a cycle with it
            0o20 => self.m.vma = word,
            0o21 => {
                self.m.vma = word;
                self.start_read();
            }
            0o22 => {
                self.m.vma = word;
                self.start_write();
            }
            0o23 => {
                self.m.vma = word;
                self.arm_map_write();
            }
            // MD, and the three that start a cycle with it
            0o30 => self.m.md = word,
            0o31 => {
                self.m.md = word;
                self.start_read();
            }
            0o32 => {
                self.m.md = word;
                self.start_write();
            }
            0o33 => {
                self.m.md = word;
                self.arm_map_write();
            }
            _ => {}
        }
        Ok(())
    }

    /// A read, with the diagnostic block answered by this engine and not by
    /// [`Machine`]: what the cpu drives onto `SPY<15:0>` under `-DBREAD` is
    /// its own state, which `Machine` does not have.
    fn read(&mut self, vma: u32) -> Word {
        let t = self.m.translate(vma);
        // QUUX has no Unibus window, and its 28-bit space puts main memory
        // where the CADR's decode finds the diagnostic registers.
        if t.access_permitted
            && !self.m.geometry.wide()
            && let Some(eadr) = busint::unibus_address(t.physical).and_then(spy::register)
        {
            self.m.vmaok = true;
            return self.spy_read(eadr).into();
        }
        self.m.vm_read(vma)
    }

    /// A memory cycle starts: the latch at VMEMDR 1D14 takes the map word
    /// of the page `VMA` is on, the cycle's direction is kept for the
    /// permission bits, and the clock is charged the mean wait for a
    /// cycle. A read's word moves at once, this engine having no bus to
    /// wait on, only a clock to keep; a write's waits for `MD`
    /// ([`Micro::start_write`]).
    ///
    /// **A start in the microcycle right after a start** is two things.
    /// On the CADR nothing holds it, and the one cycle that goes out is the
    /// second's: the first cycle goes out at the edge ending the second
    /// start's microcycle with its direction, page and word, `MBUSY` is up
    /// by the second's own edge, and nothing more is asked of the bus. So
    /// the first is lost here --- a write not written, a read's word never
    /// in `MD` --- and a second write goes out at the end of its own
    /// microcycle, the clock charged one cycle for the two. Measured on
    /// `chip` and held with `rtl` (`on_the_board_a_start_right_after_a_start_loses_the_first`,
    /// `a_start_right_after_a_start_goes_out_as_the_second`,
    /// `a_fetch_right_after_a_write_loses_the_write`, `tests/chip.rs`).
    /// **Not modelled here**: a first read lost has been read all the
    /// same, so a device register a read changes is changed, where on the
    /// board it is not. QUUX holds the second start until the first has
    /// gone out, so a first write has gone out already, with `MD` as it
    /// stood before the second start's microcycle
    /// ([`Micro::next_microcycle_holds_the_write`]), and both land
    /// (`a_start_right_after_a_start_waits_for_it`,
    /// `tests/quux_device_registers.rs`; `a_start_held_behind_a_write_loads_md_after_the_write`
    /// and `a_fetch_right_after_a_write_waits_for_it`,
    /// `tests/quux_memory_port.rs`).
    fn start_cycle(&mut self, write: bool) {
        let lost = self.memstart && self.m.geometry.unibus;
        if lost {
            self.write_out = None;
            self.new_md_delay = 0;
        } else if self.memstart {
            self.write_goes_out();
        }
        self.memop = true;
        self.lvmo = self.m.translate(self.m.vma as u32).l2_data;
        self.wrcyc = write;
        if !lost {
            self.m.ns += self.memory_cycle_ns;
            self.memory_cycles += 1;
        }
    }

    /// A write cycle starts at `VMA`: the cycle's bookkeeping as
    /// [`Micro::start_cycle`] does it, and `-VMAOK` from the map at once,
    /// for the page-fault check in the next microcycle. The word waits:
    /// the cycle goes out at the edge ending the microcycle after the
    /// start (`mit/cadr/busint.erface`: "The next clock (3) terminates
    /// MEMSTART and starts XBUSRQ"), and `MD` reaches the bus through the
    /// bus interface with no latch, the 8304 transceivers of
    /// `mit/cadr1/lmdata.drw` --- "address, data, ack, and wrcyc lines
    /// just pass straight through". So the word written is `MD` as it
    /// stands after that microcycle, and an `MD` load later still waits
    /// on `MBUSY.SYNC` for the cycle to end, as it does in `rtl`
    /// (`the_engines_write_the_md_of_the_microcycle_after_the_start`,
    /// `tests/chip.rs`; `a_write_carries_the_md_of_the_microcycle_after_its_start`,
    /// `tests/quux_memory_port.rs`).
    ///
    /// When the next microcycle cannot change `MD`, its word is the one
    /// `MD` holds already, and the write goes out at the end of the
    /// start's own microcycle ([`Micro::fetch_and_clock`]): the moment this
    /// engine has always given a device its write, which is what keeps its
    /// clock on `rtl`'s less the waits it has no model of --- a mode
    /// register write's speed bits are taken a microcycle after the start
    /// on both (`micro_keeps_the_machines_periods`, `tests/cosim.rs`). One
    /// right after a start, on the CADR, goes out at the end of its own
    /// microcycle ([`Micro::start_cycle`]). **Not modelled**: `VMA<7:0>`
    /// is also taken at the edge the cycle goes out on, so a `VMA` written
    /// in the microcycle after the start moves the word on the board and
    /// `rtl`, and not here.
    fn start_write(&mut self) {
        let after_a_start = self.memstart;
        self.start_cycle(true);
        let t = self.m.translate(self.m.vma as u32);
        self.m.vmaok = t.access_permitted && t.write_permitted;
        if self.m.vmaok {
            let when = if after_a_start && self.m.geometry.unibus {
                WriteOut::Due
            } else {
                WriteOut::Started
            };
            self.write_out = Some((t.physical, when));
        }
    }

    /// The write waiting to go out goes out, with `MD` as it stands.
    fn write_goes_out(&mut self) {
        if let Some((physical, _)) = self.write_out.take() {
            let md = self.m.md;
            self.m.bus_write(physical, md);
        }
    }

    /// Whether a write started in this microcycle waits for the next: it
    /// does when the next loads `MD`, whose word is then the one written,
    /// and, on the CADR, when the next starts a cycle, which the write is
    /// lost to ([`Micro::start_cycle`]). On QUUX a start there is held
    /// until the write has gone out, `MEMSTART AND MEMOP`, `MD` load and
    /// all, so the write goes out now with the `MD` it has.
    ///
    /// The next microcycle is the instruction in the pipeline, with the OA
    /// registers ORed in, or nothing if it is nopped; what changes `MD` in
    /// it is functional destinations 30 to 33 (34 to 37 decoding as those,
    /// [`Micro::write_functional`]) and a read's word landing at its head;
    /// what starts a cycle is destinations 21, 22, 31 and 32, and an
    /// instruction fetch, `NEXT INSTRD`'s or a `DISPATCH`'s with `IR<24>`,
    /// when `NEEDFETCH` is up (`IFETCH`, page VCTL1).
    fn next_microcycle_holds_the_write(&self) -> bool {
        let nopped = self.inhibit || self.m.clock_control.nop11;
        let mut ir = if self.m.clock_control.idebug { self.m.debug_ir } else { self.p1.raw() };
        if self.oal {
            ir |= self.oa_low;
        }
        if self.oah {
            ir |= self.oa_high << 26;
        }
        let next = Insn::new(ir);
        let field = |pos: u32, len: u32| ((ir >> pos) & ((1u64 << len) - 1)) as u32;
        let dest = match next.op() {
            Op::Alu | Op::Byte if !nopped && field(25, 1) == 0 => {
                let code = field(19, 5);
                if code & 0o20 != 0 { code & !0o4 } else { code }
            }
            _ => 0,
        };
        let loads_md = self.new_md_delay == 1 || (0o30..=0o33).contains(&dest);
        let fetches = self.needfetch()
            && (self.next_instr
                || (!nopped
                    && next.op() == Op::Dispatch
                    && field(24, 1) != 0
                    && field(10, 2) != 2));
        let starts = fetches || matches!(dest, 0o21 | 0o22 | 0o31 | 0o32);
        if self.m.geometry.unibus { loads_md || starts } else { loads_md && !starts }
    }

    /// A read cycle starts at `VMA`: the cycle's bookkeeping as
    /// [`Micro::start_cycle`] does it, and the word two microcycles later
    /// --- but only if the map permits. On the board `MBUSY` is set from
    /// `MEMSTART AND VMAOK` (page VCTL1), so a refused read requests nothing
    /// and `MD` keeps its word; `-VMAOK` in the flags is what the microcode
    /// tests for the fault.
    fn start_read(&mut self) {
        self.start_cycle(false);
        if self.m.translate(self.m.vma as u32).access_permitted {
            self.new_md = self.read(self.m.vma as u32);
            self.new_md_delay = 2;
        } else {
            self.m.vmaok = false;
        }
    }

    /// How many memory cycles this engine has started.
    pub fn memory_cycles(&self) -> u64 {
        self.memory_cycles
    }

    /// `IR<25>` picks A memory, and the address is `IR<23:14>`: page ACTL's
    /// 25S09s at 3B28 and 3B29 make `WADR<9:0>` from those ten bits, and
    /// `IR24`, which `ir.bits` marks "xx", reaches nothing but the
    /// instruction register's latch at IREG 3C17 and the parity generator
    /// at IPAR 3F24. Otherwise the write goes to a functional destination
    /// *and* to M memory, which shadows the low 32 words of A.
    ///
    /// QUUX's copies of `A-LOCALP` and `M-AP` take the word where the
    /// MACRO-DISPATCH register names its address
    /// (`crate::machine::macro_dispatch`).
    fn write_dest(&mut self, dest: u16) -> Result<(), Halt> {
        if dest & 0o4000 != 0 {
            let adr = (dest & 0o1777) as usize;
            self.m.amem[adr] = self.out;
            self.m.macro_dispatch.a_written(adr, self.out);
        } else {
            self.write_functional(dest, self.out)?;
            let adr = (dest & 0o37) as usize;
            self.m.mmem[adr] = self.out;
            self.m.amem[adr] = self.out;
            self.m.macro_dispatch.a_written(adr, self.out);
            self.m.macro_dispatch.m_written(adr, self.out);
        }
        Ok(())
    }
}

/// When a write started on `micro` goes out ([`Micro::start_write`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WriteOut {
    /// Started in this microcycle: out at its end, or at the end of the
    /// next if that one holds it.
    Started = 0,
    /// Out at the end of the next microcycle.
    Next = 1,
    /// Out at the end of this microcycle.
    Due = 2,
}

/// [`Micro`]'s `pdl_write` address for a write by PDL-INDEX, resolved when
/// the write lands: no PDL address, which is fourteen bits at most.
const PDL_AT_INDEX: u16 = u16::MAX;

/// A word's `<31:0>`: the CADR's whole word, and what the arithmetic,
/// the shifts and the rotator act on in a 40-bit one (contract G2 §2.2).
const LOW: Word = 0xffff_ffff;

/// Rotate left, the machine's only shifter primitive.
fn rol(v: u32, n: u32) -> u32 {
    v.rotate_left(n & 31)
}

/// Revision 13's rotator, a ring of 40: `<39:0>` of `v` rotated left by
/// `n` mod 40, for any 6-bit `n` (contract G2 §2.3, appendix A1.2).
fn rol40(v: Word, n: u32) -> Word {
    const RING: Word = (1 << 40) - 1;
    let v = v & RING;
    match n % 40 {
        0 => v,
        k => (v << k | v >> (40 - k)) & RING,
    }
}

/// Revision 13's masker (A1.2): bits `right` to `right + n`, if that
/// fits in bits 0-39, and none otherwise, so that the A source shows
/// through; at 32 bits it is the CADR's rule, whose mask is empty when
/// `right + n` passes bit 31.
fn mask40(right: u32, n: u32) -> Word {
    if right + n > 39 { 0 } else { ((1 << (n + 1)) - 1) << right }
}

impl Micro {
    fn alu(&mut self) -> Result<(), Halt> {
        let dest = self.ir(14, 12) as u16;
        // Page ALUC4 decides what the 74S181s are asked to do; page ALU0-1
        // does it.  Both are shared with the `rtl` engine, so the two cannot
        // drift, and the ALU is checked against its own gate model rather
        // than against either engine.
        let ctl = ttl::alu_control(
            self.p0.raw(),
            self.m.q & 1 != 0,
            self.adata & 0x8000_0000 != 0,
            true,
            false,
        );
        let alu = ttl::alu(self.mdata as u32, self.adata as u32, ctl.aluf, ctl.alumode, ctl.cin);
        // A 40-bit word's `<39:32>`: M's, or a logical function's of both
        // (contract G2 §2.2). Every shift and step below acts on `<31:0>`.
        let mtag = self.mdata & !LOW;
        let tag = if self.m.geometry.wide() {
            ttl::alu_tag(self.mdata, self.adata, ctl.aluf, ctl.alumode)
        } else {
            0
        };
        self.alu_out = (alu.f & LOW) | tag;
        self.old_q = self.m.q;
        // Revision 13's fixnum overflow flag, loaded by every ALU-class
        // word that executes: an arithmetic function's (`IR<8:3>` 20-37)
        // 33-bit result with bit 32 unlike bit 31 (A1.3).
        if self.m.geometry.wide() {
            let arithmetic = self.ir(3, 6) & 0o60 == 0o20;
            self.m.overflow = arithmetic && (alu.f >> 32 & 1) != (alu.f >> 31 & 1);
        }

        // QUUX's multiply and divide drive the output bus and load Q
        // whatever IR<13:12> and IR<1:0> say: on `<31:0>`, the output's
        // `<39:32>` M's and Q's its own.
        if let Some(op) = self.muldiv() {
            let (out, q) = muldiv::run(op, self.mdata as u32, self.adata as u32, self.m.q as u32);
            self.m.q = (self.m.q & !LOW) | Word::from(q);
            self.out = mtag | Word::from(out);
            return self.write_dest(dest);
        }

        // Q control, IR<1:0>: the shifts on `Q<31:0>`, whatever is above.
        let q_low = self.m.q as u32;
        match self.ir(0, 2) {
            1 => {
                let low = q_low << 1 | (self.alu_out & 0x8000_0000 == 0) as u32;
                self.m.q = (self.m.q & !LOW) | Word::from(low);
            }
            2 => {
                let low = q_low >> 1 | (self.alu_out as u32 & 1) << 31;
                self.m.q = (self.m.q & !LOW) | Word::from(low);
            }
            3 => self.m.q = self.alu_out,
            _ => {}
        }

        // Output bus select, IR<13:12>.  The shift-right input is bit 32 of
        // the 33-bit array --- the ninth slice's sign extension --- which is
        // the one carry-out the array has.
        self.out = match self.ir(12, 2) {
            // Not the ALU: the mask and rotate network's output, pages
            // SMCTL, SHIFT0-1, MSKG4 and MO, as the BYTE class drives it.
            // The network reads its rotate from IR<4:0> and its byte length
            // from IR<9:5> whatever the class, so here it rotates and
            // masks by the ALU function, the carry and the Q control, with
            // the A source showing outside the mask.  Microcode 323 has
            // eleven such words, all zero and with no destination;
            // `tests/output_bus.rs` holds all three engines to the network's
            // word on one with a destination.
            // Revision 13's: the rotate `IR<5:0>` in the ring of 40, the
            // length − 1 `IR<9:6>`, and no LC byte mode, `IR<11:10>` being
            // misc in an ALU word (A1.2).
            0 if self.m.geometry.wide() => {
                let rotate = self.ir(0, 6);
                let mask = mask40(rotate, self.ir(6, 4));
                (rol40(self.mdata, rotate) & mask) | (self.adata & !mask)
            }
            0 => {
                let mut rotate = self.ir(0, 5);
                if self.ir(10, 2) == 3 {
                    rotate = self.lc_byte_mode();
                }
                let left = (rotate + self.ir(5, 5)) & 0o37;
                let mask = (!0u32 >> (31 - left)) & (!0u32 << rotate);
                Word::from(rol(self.mdata as u32, rotate) & mask) | (self.adata & !Word::from(mask))
            }
            1 => self.alu_out,
            2 => mtag | ((alu.f >> 1) & LOW),
            _ => mtag | Word::from((self.alu_out as u32) << 1 | (self.old_q as u32) >> 31),
        };

        self.write_dest(dest)
    }

    /// Which of QUUX's multiply and divide the ALU-class instruction in `IR`
    /// is, on a machine that has them.
    fn muldiv(&self) -> Option<muldiv::Op> {
        if !self.m.geometry.muldiv {
            return None;
        }
        muldiv::decode(self.p0.raw())
    }

    /// Page FLAG's condition mux: `IR<5>` chooses between a bit of the
    /// shifted M source and one of seven conditions, selected by `IR<2:0>`.
    ///
    /// The comparisons are the ALU's, not Rust's.  A jump asserts `ALUSUB`
    /// with no carry in, so the array computes `m - a - 1`, and the mux
    /// selects `AEQM` and bit 32 of the 33-bit result --- the ninth slice's
    /// sign extension, which is what makes the comparisons signed.  Writing
    /// them as signed `i32` comparisons reaches the same answer without the
    /// hardware, and hides where the signedness comes from.
    ///
    /// The bit tested is bit 0 of the shifter's rotate, which takes the
    /// location counter's byte select when `IR<11:10>` is 3: page SMCTL
    /// makes `SH4` and `SH3` for every class, `SR` being up whenever the
    /// instruction is not a BYTE.
    fn jump_condition(&mut self) -> bool {
        if self.m.geometry.wide() {
            return self.jump_condition_13();
        }
        let rotate = if self.ir(10, 2) == 3 { self.lc_byte_mode() } else { self.ir(0, 5) };
        let r = rol(self.mdata as u32, rotate);
        if self.ir(5, 1) == 0 {
            self.mdata = (self.mdata & !LOW) | Word::from(r);
            return r & 1 != 0;
        }
        let ctl = ttl::alu_control(
            self.p0.raw(),
            self.m.q & 1 != 0,
            self.adata & 0x8000_0000 != 0,
            false,
            true,
        );
        let alu = ttl::alu(self.mdata as u32, self.adata as u32, ctl.aluf, ctl.alumode, ctl.cin);
        let alu32 = alu.f >> 32 & 1 != 0;
        // `INT.ENABLE` and `SEQUENCE.BREAK` are bits 27 and 26 of
        // INTERRUPT-CONTROL, the 25LS2519 at FLAG 3E08 (`Machine::byte_mode`
        // has the four).
        let int_enabled = self.m.interrupt_control & (1 << 27) != 0;
        let pending = int_enabled && self.m.interrupt();
        match self.ir(0, 3) {
            0 => r & 1 != 0,
            1 => !alu.aeqm && alu32,
            2 => alu32,
            3 => alu.aeqm,
            4 => !self.m.vmaok,
            5 => !self.m.vmaok || pending,
            6 => !self.m.vmaok || pending || (self.m.interrupt_control & (1 << 26) != 0),
            _ => true,
        }
    }

    /// **Revision 13's conditions** (contract G2 §2.2, appendix A1.3). The
    /// bit tested is bit 0 of M rotated in the ring of 40 by `{IR<47>,
    /// IR<4:0>}`, or by LC byte mode's rotation under `IR<11:10>` = 3. In
    /// condition mode the number is `IR<4:0>`: 1 is M < A on the fields,
    /// signed, with the fields' equality in place of `AEQM`; 2 is bit 32 of
    /// the fields' 33-bit M − A − 1; 3 is M = A over all 40 bits, the
    /// fields' `AEQM` and the tags alike; 10 is the fixnum overflow flag;
    /// 11 is M < A on the fields, unsigned; every other number decodes as
    /// its `IR<2:0>`.
    fn jump_condition_13(&mut self) -> bool {
        let mut rotate = self.ir(47, 1) << 5 | self.ir(0, 5);
        if self.ir(10, 2) == 3 {
            rotate = self.lc_rotation(self.m.lc, rotate);
        }
        let r = rol40(self.mdata, rotate);
        if self.ir(5, 1) == 0 {
            self.mdata = r;
            return r & 1 != 0;
        }
        let ctl = ttl::alu_control(
            self.p0.raw(),
            self.m.q & 1 != 0,
            self.adata & 0x8000_0000 != 0,
            false,
            true,
        );
        let alu = ttl::alu(self.mdata as u32, self.adata as u32, ctl.aluf, ctl.alumode, ctl.cin);
        let alu32 = alu.f >> 32 & 1 != 0;
        let int_enabled = self.m.interrupt_control & (1 << 27) != 0;
        let pending = int_enabled && self.m.interrupt();
        let code = match self.ir(0, 5) {
            c @ (0o10 | 0o11) => c,
            c => c & 7,
        };
        match code {
            0 => r & 1 != 0,
            1 => !alu.aeqm && alu32,
            2 => alu32,
            3 => alu.aeqm && self.mdata >> 32 == self.adata >> 32,
            4 => !self.m.vmaok,
            5 => !self.m.vmaok || pending,
            6 => !self.m.vmaok || pending || (self.m.interrupt_control & (1 << 26) != 0),
            0o10 => self.m.overflow,
            0o11 => (self.mdata as u32) < (self.adata as u32),
            _ => true,
        }
    }

    fn jump(&mut self) -> Result<(), Halt> {
        let mut target = self.ir(12, 14) as u16;
        let r = self.ir(9, 1) != 0;
        let p = self.ir(8, 1) != 0;
        let n = self.ir(7, 1) != 0;
        let invert = self.ir(6, 1) != 0;

        // `IR<11:10>` = 1 here is `HALT-CONS`, `cadsym.lisp`'s `1_10.`,
        // which microcode 323 writes at `ZERO`, `ILLOP` and `%HALT`: the
        // jump is taken as any other, and the halt is `HALTED`'s, set after
        // the instruction ([`Micro::step`]).
        // P and R together on a JUMP is not a jump: it writes the control
        // store from the A and M sources.
        //
        // **The board pushes and then pops, and so does this.** Page CONTRL:
        // the 74S64 at 3E26 makes `-SPUSH` from four AND groups, the first
        // `IRJUMP AND -IR6 AND IR8 AND JCOND`, and **no group has `IWRITE`
        // in it** --- `IWRITE` is decoded on its own at the 74S11 3E29,
        // `IRJUMP AND IR8 AND IR9`, and never reaches 3E26. A control-store
        // write is a jump-always with P, so that first group is satisfied
        // and the machine pushes. On the next cycle the 74S175 at 3D26 has
        // `IWRITED`, the open-collector 74S08 at 3D21 gives
        // `-POPJ = -IPOPJ AND -IWRITED`, and it pops.
        //
        // So the stack pointer ends where it began with the pushed word
        // still in the slot above it, which is what a console reading the
        // stack sees. Read off `data/CADR.netlist` and confirmed pin for pin
        // against MIT's own wire list `cadrwd/cadr4.wlr`; no other
        // implementation is cited, and none is needed.
        //
        // It really costs two microcycles and this engine spends neither in
        // the pipeline: `IWRITED` drives `N` as well as `POPJ`, so the board
        // loses one cycle to this instruction's `N` and another to
        // `IWRITED`'s. Inhibiting two cycles here would kill the two
        // instructions after the write rather than returning to the first of
        // them, so the cycles are charged to the clock instead and the
        // pipeline is left alone.
        if p && r {
            self.m.write_imem(target, Insn::new(self.iwr));
            if !invert && self.jump_condition() {
                let ret = if n { self.npc.wrapping_sub(1) } else { self.npc } & 0o37777;
                self.push_spc(ret as u32);
                // The pop is the next microcycle's, under `IWRITED`, and
                // takes the pushed word through `SPCWPASS`: nothing of
                // this microcycle's counting is its.
                self.spc_pushed = false;
                self.spc_popped = false;
                self.pop_spc();
            }
            // The two microcycles the board spends on it, both nopped and
            // so never long, go on the clock at the next step, where the
            // board spends them: `micro_keeps_the_machines_periods` parted
            // from `rtl` by exactly two boot-speed cycles at
            // `CLEAR-I-MEMORY` without them.
            self.nopped = 2;
            return Ok(());
        }

        let cond = self.jump_condition() != invert;
        if p && cond {
            let ret = if n { self.npc.wrapping_sub(1) } else { self.npc } & 0o37777;
            self.push_spc(ret as u32);
        }
        if r && cond {
            let mut t = self.pop_spc();
            if (t >> 14) & 1 != 0 {
                t = self.jump_return(t);
            }
            target = (t & 0o37777) as u16;
        }
        if cond {
            if n {
                self.inhibit = true;
            }
            self.npc = target;
            // Page CONTRL: `PCS0` is `NOT(POPJ OR ...)`, so with the POPJ bit
            // up a taken jump still takes its next address off the stack,
            // which `step` does after the instruction; a return has popped
            // it already.
            if r {
                self.popj = false;
            }
        }
        Ok(())
    }

    fn dispatch(&mut self) -> Result<(), Halt> {
        // Revision 13: the rotate `{IR<47>, IR<4:0>}` in the ring of 40, and
        // the address `IR<23:12>` into 4,096 entries (A1.1, A1.4).
        let wide = self.m.geometry.wide();
        let mut pos = if wide { self.ir(47, 1) << 5 | self.ir(0, 5) } else { self.ir(0, 5) };
        let len = self.ir(5, 3);
        let map = self.ir(8, 2);
        let mut addr = if wide { self.ir(12, 12) } else { self.ir(12, 11) };
        let dmem_mask = self.m.geometry.dmem_words() as u32 - 1;
        let use_lpc = self.ir(25, 1) != 0;
        let advance = self.ir(24, 1) != 0;

        // Misc 2 writes the dispatch memory instead of dispatching.
        //
        // Seventeen bits, not thirty-two: the DRAM pages hold `DPC<13:0>`,
        // `DN`, `DP` and `DR`, which `tests/chip.rs` reads off the netlist
        // and enforces against `rtl`.
        //
        // **The board does hold the eighteenth bit, and this comment used
        // to say it did not.** The 93425As at DRAM 1F16 and 1F17 take
        // `AA17` --- A bus bit 17 --- and give `DPAR`, which is the parity
        // bit `CC-WRITE-D-MEM` and PRAID's `P-D-MEM-D` both compute as odd
        // and write through A memory location 0.
        //
        // Dropping it is right all the same, because **nothing can read it
        // back**. `DPAR` goes only to the parity checker, the 74S280 at
        // 4F09; `CC-READ-D-MEM` in `cc/lcadrd.lisp` does not read the word
        // at all but reconstructs the seventeen data bits from the machine's
        // behavior --- the PC-select bits and the noop and `SPUSHD` flags
        // out of `SPY-FLAG-2`, and `DPC` from `CC-READ-PC`. The one thing
        // that observes the stored bit is `-DPE`, and that can only differ
        // from correct if a chip has failed.
        let write = self.ir(10, 2) == 2;
        if self.ir(10, 2) == 3 {
            pos = if wide { self.lc_rotation(self.m.lc, pos) } else { self.lc_byte_mode() };
        }

        let m = if wide { rol40(self.mdata, pos) as u32 } else { rol(self.mdata as u32, pos) };
        let mask = if len == 0 { 0 } else { !0u32 >> (31 - ((len - 1) & 0o37)) };

        // Level-2 map bits.  The CADR documentation says 14 and 15; the
        // hardware uses 18 and 19 (discrepancy 3).
        //
        // A map bit takes address bit 0 *instead of* the field's: the
        // 74S64s at 2F24, 2F05 and 2F23 make `-DADR0` from `VMO18 AND IR8`,
        // `VMO19 AND IR9`, `-DMAPBENB AND DMASK0 AND R0` and `IR12`, with
        // `-DMAPBENB = NOR(IR8, IR9)` at 3F14 gating the field's bit 0 out
        // whenever a map bit is selected.  ORing the map bit into the
        // field's is the easy misreading of that NAND-OR; the bit takes
        // the place, as `rtl` and `chip` show.
        // Revision 13's are the entry's `<22>` and `<23>`, the meta bits
        // moved up by 4 with the rest (A1.7).
        if map != 0 {
            let bits =
                self.map_seen.unwrap_or_else(|| self.m.translate(self.map_address())).l2_data;
            let at = if wide { 22 } else { 18 };
            let b18 = (bits >> at) & 1;
            let b19 = (bits >> (at + 1)) & 1;
            addr |= (m & mask & !1)
                | match map {
                    1 => b18,
                    2 => b19,
                    _ => b18 | b19,
                };
        } else {
            addr |= m & mask;
        }

        let entry = self.m.dmem[(addr & dmem_mask) as usize];
        if write {
            // The write goes to the address the dispatch would have read:
            // `DADR` takes the field and the M-source bits whatever the
            // function (page DSPCTL), and `-DWEA` is `NAND(WP2, DISPWR)`.
            // `DISPENB` is `DISPATCH AND NOT DISPWR`, so nothing is
            // dispatched; but a POPJ in the same instruction is still
            // `IGNPOPJ`'s, which reads R: with R clear the POPJ is a jump to
            // the word's DPC and pops nothing (`Rtl::read_phase`'s `pcs`).
            // Which word: on the CADR the RAM races and muir takes the
            // netlist's answer, the word written; QUUX defines the one
            // standing before ([`Geometry::old_word_while_written`],
            // `tests/dispatch_write_order.rs`).
            let new = self.adata as u32 & 0o377777;
            self.m.dmem[(addr & dmem_mask) as usize] = new;
            let entry = if self.m.geometry.old_word_while_written { entry } else { new };
            self.ignpopj(entry);
            if self.popj && (entry >> 16) & 1 == 0 {
                self.npc = (entry & 0o37777) as u16;
                self.popj = false;
            }
            return Ok(());
        }
        self.m.dispatch_constant = self.ir(32, 10) as u16;

        let mut target = entry & 0o37777;
        let n = (entry >> 14) & 1 != 0;
        let p = (entry >> 15) & 1 != 0;
        let r = (entry >> 16) & 1 != 0;
        self.ignpopj(entry);

        // The address a push would save --- page CONTRL's `RETA`: `PC + 1`,
        // under `N` the inhibited slot's own address, and with `IR<25>` the
        // `LPC` of the instruction-stream hardware, one behind that.
        let ret = if n {
            let pc = self.npc.wrapping_sub(1);
            if use_lpc { pc.wrapping_sub(1) } else { pc }
        } else {
            self.npc
        } & 0o37777;
        if advance {
            self.step_lc();
        }
        if n {
            self.inhibit = true;
        }
        // `DFALL = DR AND DP` on page CONTRL: R and P together are neither,
        // and the next address is `PC + 1`, nothing pushed or popped.
        if p && r {
            return Ok(());
        }
        if p {
            self.push_spc(ret as u32);
        }
        if r {
            let mut t = self.pop_spc();
            if (t >> 14) & 1 != 0 {
                t = self.main_loop_return(t, advance);
            }
            target = t & 0o37777;
        }
        self.npc = target as u16;
        self.popj = false;
        Ok(())
    }

    /// `IGNPOPJ`: a dispatch whose entry has no R pops nothing for the
    /// functional source, which has read the SPC word all the same. Page
    /// CONTRL: `-IGNPOPJ` is `DR OR -IRDISP`, the 74S32 at 3E18, and the
    /// 74S64 at 3E28 ANDs it into `POPJ OR SRCSPCPOPREAL` in making `-SPOP`
    /// (`cadrwd/cadr4.wlr`, nets `-IGNPOPJ` and `-SPOP`). So the pop
    /// [`Micro::read_functional`] took for source 14 is given back; the
    /// POPJ bit the dispatch clears itself.
    fn ignpopj(&mut self, entry: u32) {
        if (entry >> 16) & 1 == 0 && self.spc_popped {
            self.m.spcptr = (self.m.spcptr + 1) & 0o37;
            self.spc_popped = false;
        }
    }

    /// **Revision 13's BYTE** (contract G2 §2.3, appendix A1.1-A1.2): the
    /// rotate `IR<5:0>`, the length − 1 `IR<11:6>`, in the ring of 40, with
    /// the masker's rule at 40 bits ([`mask40`]); LC byte mode by `IR<24>`,
    /// on an LDB alone; and `IR<11:10>`, length bits here, decode no misc
    /// function.
    fn byte_13(&mut self) -> Result<(), Halt> {
        let dest = self.ir(14, 12) as u16;
        let func = self.ir(12, 2);
        let rotate = self.ir(0, 6);
        let pos = if func == 1 && self.ir(24, 1) != 0 {
            self.lc_rotation(self.m.lc, rotate)
        } else {
            rotate
        };
        let right = if func & 2 != 0 { rotate } else { 0 };
        let mask = mask40(right, self.ir(6, 6));
        let m = if func & 1 != 0 { rol40(self.mdata, pos) } else { self.mdata };
        self.out = (m & mask) | (self.adata & !mask);
        self.write_dest(dest)
    }

    fn byte(&mut self) -> Result<(), Halt> {
        if self.m.geometry.wide() {
            return self.byte_13();
        }
        let dest = self.ir(14, 12) as u16;
        let func = self.ir(12, 2);
        let mut pos = self.ir(0, 5);
        if self.ir(10, 2) == 3 {
            pos = self.lc_byte_mode();
        }

        let width_minus_1 = self.ir(5, 5);
        let right = if func & 2 != 0 { pos } else { 0 };
        let left = (right + width_minus_1) & 0o37;
        let mask = (!0u32 >> (31 - left)) & (!0u32 << right);

        // Page SMCTL: `IR<12>` rotates (`SR`) and `IR<13>` places the mask
        // (`MR`), so LDB and DPB rotate, selective deposit and function 0 do
        // not; and every BYTE puts the mask network's word on the bus, `OSEL`
        // being 0 on the class.
        let m = if func & 1 != 0 { rol(self.mdata as u32, pos) } else { self.mdata as u32 };
        self.out = Word::from(m & mask) | (self.adata & !Word::from(mask));
        self.write_dest(dest)
    }
}

impl Engine for Micro {
    fn nominal_cycle_ns(&self) -> u64 {
        if self.sync_cycle_ns > 0 { self.sync_cycle_ns } else { crate::ioboard::CYCLE_NS }
    }

    /// The boot sequence: reset, enable the PROM, then trap to control store
    /// location 0.  The trap forces NPC to zero and inhibits the unfetched
    /// instruction still sitting in the pipeline, exactly as a parity trap
    /// would.
    fn boot(&mut self) {
        self.m.vmaok = false;
        // `-RESET` clears the console's registers, the mode register among
        // them, which is where `PROMDISABLE` lives; the PROM is back over the
        // bottom of the control store.  `-BOOT` presets `RUN`.
        self.m.reset_console_registers();
        self.m.timers = crate::machine::Timers::new();
        self.m.macro_dispatch.reset();
        self.m.clock_control.run = true;
        self.srun = true;
        self.npc = self.m.reset_pc();
        self.inhibit = true;
        self.trap = true;
        // A pending OA write dies with the instruction it modified, the one
        // the trap nops, as it does in `IR` on the board (page IREG;
        // `Rtl::clock_edge`); and `-RESET` clears `IMODD` at PDLCTL 4C11
        // (`Rtl::reset`). Held over, the modification would land on the
        // PROM's first word and send the boot somewhere else
        // (`tests/oa_boot.rs`).
        self.oal = false;
        self.oah = false;
        // `-RESET` clears `MEMSTART` (`Rtl::reset`): a write not yet gone
        // out never does.
        self.write_out = None;
    }
    fn save(&self, w: &mut crate::checkpoint::Writer) {
        let Micro {
            m,
            p0,
            p0_pc,
            p1,
            p1_pc,
            npc,
            inhibit,
            popj,
            next_instr,
            next_instrd,
            oal,
            oah,
            oa_low,
            oa_high,
            new_md,
            new_md_delay,
            aaddr,
            maddr,
            adata,
            mdata,
            alu_out,
            old_q,
            out,
            iwr,
            executed,
            map_write,
            map_write_d,
            // Set and used within one step.
            map_seen: _,
            lvmo,
            wrcyc,
            speed,
            speed_a,
            memory_cycle_ns,
            // The flags set it again on a resume, as the timing model is.
            sync_cycle_ns: _,
            memory_cycles,
            nopped,
            srun,
            sstep,
            ssdone,
            halted,
            memstart,
            memop,
            write_out,
            pdl_write,
            spc_write,
            opc,
            opc_ck,
            trap,
            // Set and used within one step.
            pushed: _,
            spc_pushed: _,
            spc_popped: _,
            macro_write: _,
            operand: _,
        } = self;
        m.save(w);
        w.u64(p0.raw());
        w.u16(*p0_pc);
        w.u64(p1.raw());
        w.u16(*p1_pc);
        w.u16(*npc);
        w.bool(*inhibit);
        w.bool(*popj);
        w.bool(*next_instr);
        w.bool(*next_instrd);
        w.bool(*oal);
        w.bool(*oah);
        w.u64(*oa_low);
        w.u64(*oa_high);
        w.word(*new_md);
        w.u8(*new_md_delay);
        w.u16(*aaddr);
        w.u8(*maddr);
        w.word(*adata);
        w.word(*mdata);
        w.word(*alu_out);
        w.word(*old_q);
        w.word(*out);
        w.u64(*iwr);
        w.opt(*executed, crate::checkpoint::Writer::u16);
        let map = |w: &mut crate::checkpoint::Writer, (vma, md): (Word, Word)| {
            w.word(vma);
            w.word(md);
        };
        w.opt(*map_write, map);
        w.opt(*map_write_d, map);
        w.u32(*lvmo);
        w.bool(*wrcyc);
        w.speed(*speed);
        w.speed(*speed_a);
        w.u64(*memory_cycle_ns);
        w.u64(*memory_cycles);
        w.u8(*nopped);
        w.bool(*srun);
        w.bool(*sstep);
        w.bool(*ssdone);
        w.bool(*halted);
        w.bool(*memstart);
        w.bool(*memop);
        w.opt(*write_out, |w, (physical, when)| {
            w.u32(physical);
            w.u8(when as u8);
        });
        w.opt(*pdl_write, |w, (adr, word)| {
            w.u16(adr);
            w.word(word);
        });
        w.opt(*spc_write, |w, (ptr, word)| {
            w.u8(ptr);
            w.u32(word);
        });
        w.u16s(opc);
        w.bool(*opc_ck);
        w.bool(*trap);
    }

    fn load(&mut self, r: &mut crate::checkpoint::Reader) -> std::io::Result<()> {
        self.m.load(r)?;
        self.p0 = Insn::new(r.u64()?);
        self.p0_pc = r.u16()?;
        self.p1 = Insn::new(r.u64()?);
        self.p1_pc = r.u16()?;
        self.npc = r.u16()?;
        self.inhibit = r.bool()?;
        self.popj = r.bool()?;
        self.next_instr = r.bool()?;
        self.next_instrd = r.bool()?;
        self.oal = r.bool()?;
        self.oah = r.bool()?;
        self.oa_low = r.u64()?;
        self.oa_high = r.u64()?;
        self.new_md = r.word()?;
        self.new_md_delay = r.u8()?;
        self.aaddr = r.u16()?;
        self.maddr = r.u8()?;
        self.adata = r.word()?;
        self.mdata = r.word()?;
        self.alu_out = r.word()?;
        self.old_q = r.word()?;
        self.out = r.word()?;
        self.iwr = r.u64()?;
        self.executed = r.opt(crate::checkpoint::Reader::u16)?;
        self.map_write = r.opt(|r| Ok((r.word()?, r.word()?)))?;
        self.map_write_d = r.opt(|r| Ok((r.word()?, r.word()?)))?;
        self.lvmo = r.u32()?;
        self.wrcyc = r.bool()?;
        self.speed = r.speed()?;
        self.speed_a = r.speed()?;
        self.memory_cycle_ns = r.u64()?;
        self.memory_cycles = r.u64()?;
        self.nopped = r.u8()?;
        self.srun = r.bool()?;
        self.sstep = r.bool()?;
        self.ssdone = r.bool()?;
        self.halted = r.bool()?;
        self.memstart = r.bool()?;
        self.memop = r.bool()?;
        self.write_out = r.opt(|r| {
            let physical = r.u32()?;
            let when = match r.u8()? {
                0 => WriteOut::Started,
                1 => WriteOut::Next,
                2 => WriteOut::Due,
                k => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("a write going out at {k}"),
                    ));
                }
            };
            Ok((physical, when))
        })?;
        self.pdl_write = r.opt(|r| Ok((r.u16()?, r.word()?)))?;
        self.spc_write = r.opt(|r| Ok((r.u8()?, r.u32()?)))?;
        r.u16s_into(&mut self.opc)?;
        self.opc_ck = r.bool()?;
        self.trap = r.bool()?;
        Ok(())
    }

    fn step(&mut self) -> Result<(), Halt> {
        self.executed = None;
        // `MACHRUN`, less the statistics halt this engine cannot raise, and
        // with the one `ERR` it can: no parity check, but `HALTED` under
        // `ERRSTOP`.  Halted, a step is one master clock cycle and no
        // microcycle.
        let errhalt = self.m.mode.errstop && self.halted;
        let machrun = (self.sstep && !self.ssdone) || (self.srun && !errhalt);
        if !machrun {
            // A write started before the halt goes out at the master
            // clock's edge, which runs on: `MEMSTART` is clocked by
            // `MCLK1A` (`Rtl::start_bus_cycle`).
            self.write_goes_out();
            self.speedclk();
            self.m.ns += self.cycle_ns(false);
            self.mclk_edge();
            self.opc_clock(true, self.p1_pc);
            return Ok(());
        }
        // No clock phases here, but the machine's periods: each microcycle
        // is as long as the speed bits and `ILONG` make it, 220 ns through
        // the boot and 145 once microcode 323 writes the mode register, and
        // an inhibited instruction is nopped and so never long. What is not
        // here is the time the machine waits on the bus; `rtl` has that,
        // and `memory_cycle_ns` stands in for it with the band's mean.
        while self.nopped > 0 {
            self.speedclk();
            self.m.ns += self.cycle_ns(false);
            self.nopped -= 1;
            self.mclk_edge();
        }
        self.speedclk();
        // `-ILONG` is `NAND(IR45, -NOPA)` at FLAG 3E07, and `NOPA` is a
        // jump's inhibit or the console's `NOP11` (CONTRL 3E14) --- not the
        // boot's trap, which is in `NOP` alone.
        let nopa = (self.inhibit && !self.trap) || self.m.clock_control.nop11;
        let ilong = !nopa && self.p1.raw() >> 45 & 1 != 0;
        self.trap = false;
        // QUUX's divider holds the microcycle off for `DIV_CYCLES`
        // generator cycles after its operands are ready, as `rtl` does; not
        // a nopped one, and not a single step, which `-WAIT` does not stop
        // either. This engine has no MD interlock --- the word is in `MD`
        // two instructions after the read's start, whatever the memory is
        // doing --- so the operands are ready as the `DIV` enters `IR`, and
        // the count starts there, what `rtl` does for a `DIV` whose read
        // has landed.
        let stepping = self.sstep && !self.ssdone;
        if self.m.geometry.muldiv
            && !nopa
            && !stepping
            && muldiv::decode(self.p1.raw()) == Some(muldiv::Op::Div)
        {
            // The master clock runs through the wait, and a write started
            // in the microcycle before goes out at its first edge, ahead of
            // the `DIV`'s own destination.
            self.write_goes_out();
            for _ in 0..muldiv::DIV_CYCLES {
                self.speedclk();
                self.m.ns += self.cycle_ns(ilong);
                self.mclk_edge();
            }
        }
        self.m.ns += self.cycle_ns(ilong);
        self.mclk_edge();
        self.advance_pipeline();
        self.memstart = std::mem::take(&mut self.memop);
        // A map store's write lands here, before the instruction runs, so
        // that its memory access translates through the new map as `rtl`'s
        // does. What the instruction reads of the map itself --- `MAP(MD)`
        // and a dispatch on its bits --- is on the CADR the word written, the
        // netlist's answer to a race on the board, and on QUUX the word from
        // before the write, as QUUX defines it
        // ([`Geometry::old_word_while_written`],
        // `tests/dispatch_write_order.rs`).
        self.map_seen = (self.m.geometry.old_word_while_written && self.map_write_d.is_some())
            .then(|| self.m.translate(self.map_address()));
        self.land_map_write();

        if self.new_md_delay > 0 {
            self.new_md_delay -= 1;
            if self.new_md_delay == 0 {
                self.m.md = self.new_md;
            }
        }

        // A jump with N set kills the instruction already in the pipeline,
        // and the console's `NOP11` kills every one.
        if self.inhibit || self.m.clock_control.nop11 {
            self.inhibit = false;
            // An OA register write modifies the instruction it is ORed into
            // and nothing after it: on the board the word goes into `IR` as
            // `IR` loads (page IREG; `Rtl::clock_edge`), so nopping that
            // instruction --- a jump's `N`, `NOP11`, the boot's trap ---
            // throws the modification away with it. Held over, it would
            // land on whatever runs next, which after a boot is the PROM's
            // first word (`tests/oa_boot.rs`).
            self.oal = false;
            self.oah = false;
            // Nopped, the instruction's misc field decodes to nothing.
            self.halted = false;
            self.land_writes();
            // But an armed fetch still happens in it. `IFETCH` is
            // `NEEDFETCH AND LCINC` on page VCTL1 and `LCINC` is `NEXT
            // INSTRD` --- a register, not anything decoded from `IR` ---
            // so a nopped microcycle carries the fetch as any other does.
            // `POPJ-AFTER-NEXT` makes this the common case rather than a
            // corner: the microcycle a `POPJ` arms is the inhibited one.
            self.fetch_and_clock();
            self.m.cycles += 1;
            return Ok(());
        }

        self.executed = Some(self.p0_pc);

        // The OA registers modify the instruction as it is executed.
        if self.oal {
            self.oal = false;
            self.p0 = Insn::new(self.p0.raw() | self.oa_low);
        }
        if self.oah {
            self.oah = false;
            self.p0 = Insn::new(self.p0.raw() | (self.oa_high << 26));
        }

        self.popj = self.p0.popj();
        self.pushed = false;
        self.spc_pushed = false;
        self.spc_popped = false;
        self.macro_write = None;
        self.aaddr = self.ir(32, 10) as u16;
        self.maddr = self.ir(26, 5) as u8;
        self.mdata = if self.p0.m_src_functional() {
            self.read_functional(self.maddr)?
        } else {
            self.m.mmem[self.maddr as usize]
        };
        self.adata = self.m.amem[self.aaddr as usize];
        self.land_writes();
        // `IWR<47:32>` from `A<15:0>` and `IWR<31:0>` from `M<31:0>`.
        self.iwr = ((self.adata & 0o177777) << 32) | (self.mdata & LOW);

        match self.p0.op() {
            Op::Alu => self.alu()?,
            Op::Jump => self.jump()?,
            Op::Dispatch => self.dispatch()?,
            Op::Byte => self.byte()?,
        }

        // `IR<11:10>` = 1 on any class is misc function 1, `HALT-CONS`:
        // `-FUNCT1` off the 74S139 at SOURCE 3D05, which the 74S374 at
        // OLORD2 1A05 registers as `HALTED` at the next edge.
        // Revision 13's BYTE words decode no misc function: `IR<11:10>`
        // are length bits there (A1.1).
        self.halted = self.ir(10, 2) == 1 && !(self.m.geometry.wide() && self.p0.op() == Op::Byte);

        if self.popj {
            let mut t = self.pop_spc();
            if (t >> 14) & 1 != 0 {
                t = self.main_loop_return(t, false);
            }
            self.npc = (t & 0o37777) as u16;
        }
        if let Some((code, data)) = self.macro_write.take() {
            self.m.macro_dispatch.write(code, data);
        }
        self.fetch_and_clock();
        self.m.cycles += 1;
        Ok(())
    }

    fn pc(&self) -> u16 {
        self.npc
    }

    /// What this engine can answer of the sixteen, which is less than
    /// `rtl`: it has no read phase apart from execution, so `OB`, `A` and
    /// `M` are the last microcycle's and not the standing instruction's;
    /// there is no statistics counter, no write pipeline behind the six
    /// registered flags, and no `JCOND` or `PCS` outside a jump.  `IR` and
    /// `PC` are the instruction waiting to execute and the address after
    /// it, which is what `rtl` holds in `IR` and `PC` between microcycles.
    fn spy_read(&self, eadr: u8) -> u16 {
        let half = |v: u64, k: u8| (v >> (16 * k as u32)) as u16;
        match eadr {
            spy::IR_LOW | spy::IR_MED | spy::IR_HIGH => half(self.p1.raw(), eadr),
            spy::OPC => self.opc[7] & 0x3fff,
            spy::PC => self.npc & 0x3fff,
            spy::OB_LOW => self.out as u16,
            spy::OB_HIGH => (self.out >> 16) as u16,
            spy::FLAG_1 => spy::Flag1 {
                promdisable: self.m.mode.prom_disable,
                err: self.halted,
                ssdone: self.ssdone,
                srun: self.srun,
                ..Default::default()
            }
            .word(),
            spy::FLAG_2 => spy::Flag2 {
                nop: self.inhibit || self.m.clock_control.nop11,
                vmaok: self.m.vmaok,
                ..Default::default()
            }
            .word(),
            spy::M_LOW => self.mdata as u16,
            spy::M_HIGH => (self.mdata >> 16) as u16,
            spy::A_LOW => self.adata as u16,
            spy::A_HIGH => (self.adata >> 16) as u16,
            spy::STAT_LOW | spy::STAT_HIGH => 0,
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
