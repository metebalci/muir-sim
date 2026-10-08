// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The state every engine shares: the memories, the registers, and the
//! two-level virtual memory map.
//!
//! The map geometry is read off the VMEM pages of the netlist, and the field
//! positions off `mit/cadr/ir.bits`; each is named where it is used.  Where the
//! drawings and the MIT documentation disagree, the drawings win and the
//! disagreement is named where it bites.

use crate::busint;
use crate::disk_controller::{self, Controller};
use crate::ioboard::{self, IoBoard};
use crate::isa::Insn;
use crate::spy;
use crate::tv::{self, Tv};

/// Control store size: 16K words, fourteen bits of PC.
pub const IMEM_WORDS: usize = 16 * 1024;
/// Boot PROM size: 1K words, overlaying the bottom of the control store
/// until the mode register's `PROMDISABLE` bit is set.
///
/// Twelve 74S472s, 512 by 8 each, on pages PROM0 and PROM1 of
/// `data/CADR.netlist` hold two banks of 512 words, and page PCTL decodes
/// them: `BOTTOM.1K = NOR(PC13, PC12, PC11, PC10)` at the 74S260 1D18,
/// `-PROMENABLE = NAND(BOTTOM.1K, -IDEBUG, -PROMDISABLED, -IWRITEDA)` at
/// the 74S20 1C19, and the two chip enables at the 74S32 1C18, `-PROMCE0 =
/// OR(-PROMENABLE, PC9)` for the first bank and `-PROMCE1 =
/// OR(-PROMENABLE, -PROMPC9)` for the second. `mit/cadr/ir.bits` says the
/// same of `PROMDISABLE`: "0 first 1K I memory is PROM". MIT's image fills
/// 454 words of the first bank and the boot never leaves it; the second
/// bank goes unused, and every engine reads the rest of the 1K as zero.
pub const PROM_WORDS: usize = 1024;

/// Where QUUX's boot PROM sits in the control store (contract Q2): 1K words
/// at 36000-37777, never overlaid, the PC starting there at reset.
pub const QUUX_PROM_BASE: u16 = 0o36000;
/// Main memory by default, in 32-bit words: thirty-two boards of 64K.
/// [`Machine::with_memory_boards`] builds a machine with another count.
/// Physical pages above the memory are devices, or nothing.
pub const MAIN_WORDS: usize = 2 * 1024 * 1024;

/// Revision 13's main memory by default: 32MW, what both boards
/// have (contract G2 §3), 512 boards of 64K.
pub const MAIN_WORDS_13: usize = 32 * 1024 * 1024;

/// `words` of main memory in megawords, as QUUX says its memory and
/// `--main-memory-size` takes it: `32MW`. An amount that is not whole MW, which
/// only a checkpoint older than the flag can hold, is said with its
/// fraction, `2.0625MW`.
pub fn megawords(words: usize) -> String {
    if words.is_multiple_of(1 << 20) {
        format!("{}MW", words >> 20)
    } else {
        format!("{}MW", words as f64 / f64::from(1 << 20))
    }
}

/// The most main memory revision 13 has: 64MW, 1,024 boards of 64K
/// (contract G1 §3.2, "32 M words to begin, up to 64 M"). Its physical
/// space has nothing from there to the frame buffer window.
pub const MAX_MAIN_WORDS_13: usize = 64 * 1024 * 1024;

/// A word of the datapath and of main memory: 32 bits on the CADR, 40 on
/// QUUX, whose [`Geometry::word_bits`] says so (contract G2 §2.1), held in 64 bits either way. A, M, the PDL
/// buffer, `Q`, `VMA`, `MD` and main memory are words; the numeric
/// registers beside them --- the SPC stack, the location counter, the
/// map --- keep their own widths.
pub type Word = u64;

/// A word of main memory as the devices that move main memory see it ---
/// the disk controller, block-disk and the file device --- which carry
/// 32 bits: they read `<31:0>` of a word and write a word with 0 above bit
/// 31. Main memory is [`Word`]s; a test's memory may be `u32`s.
pub trait MemoryWord: Copy + PartialEq {
    /// `<31:0>`.
    fn low(self) -> u32;
    /// A word with `<31:0>` `v` and 0 above.
    fn of(v: u32) -> Self;
    /// A word with `<31:0>` `v` and the tag `<39:32>` `tag`, where the
    /// word has them; a 32-bit word is `v`.
    fn tagged(v: u32, tag: u8) -> Self;
}

impl MemoryWord for u32 {
    fn low(self) -> u32 {
        self
    }
    fn of(v: u32) -> Self {
        v
    }
    fn tagged(v: u32, _: u8) -> Self {
        v
    }
}

impl MemoryWord for u64 {
    fn low(self) -> u32 {
        self as u32
    }
    fn of(v: u32) -> Self {
        v.into()
    }
    fn tagged(v: u32, tag: u8) -> Self {
        u64::from(tag) << 32 | u64::from(v)
    }
}

/// Bits of the bus error status, which the console reads over SPY and the
/// microcode tests. Three bits can be set here --- the two NXM bits and the
/// Unibus map error; the rest of the register is parity errors, which
/// cannot happen here. The wording and the bit values are MIT's own, the
/// register's description at the head of System 100's `sys/cc/ldbg.lisp`.
pub mod bus_error {
    /// "Xbus NXM Error. Set when an Xbus cycle times out for lack of
    /// response." --- `ldbg.lisp`, bit value 1.  `XB NXM ERROR` on the
    /// 74276 at the interface's REQERR 0B02, set by `-NXM TIMEOUT` with `XBUS
    /// REQUEST`, and read through the 8304 at 0B15.
    pub const XBUS_NXM: u16 = 0o1;
    /// "Unibus NXM Error. Set when a Unibus cycle times out for lack of
    /// response." --- `ldbg.lisp`, bit value 10.  `UB NXM ERROR` on the
    /// same 74276, with `UNIBUS REQUEST`, and the same 8304.
    pub const UNIBUS_NXM: u16 = 0o10;
    /// "Unibus Map Error. Set when an attempt to perform an Xbus cycle
    /// through the Unibus map is refused because the map specifies invalid
    /// or write-protected." --- the head of `ldbg.lisp`.  The 74LS74 at
    /// REQERR 0D03, clocked by `UB XBUS T100`.
    pub const UB_MAP_ERROR: u16 = 0o40;
}

/// The widths of the map and the PDL buffer: what differs between the
/// machines the two executables are, `cadr` and `quux`.
///
/// The CADR's are MIT's (`SIZE-OF-HARDWARE-LEVEL-1-MAP`, `-LEVEL-2-MAP` and
/// `-PDL-BUFFER` in System 100's `sys/cold/qcom.lisp`; the netlist's RAMs,
/// `chip_and_rtl_hold_the_same_memories` in `tests/chip.rs`): a level-1
/// entry of five bits, the number of a block of 32 level-2 entries, and a
/// PDL pointer and index of ten.  A level-2 block is 32 entries on every
/// machine, `VMA<12:8>` choosing one in it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Geometry {
    /// Bits in a word ([`Word`]): 32 on the CADR; 40 on QUUX revision 13,
    /// [`Geometry::QUUX`], whose fields,
    /// rotator, jump conditions, dispatch memory, location counter and map
    /// come with the word (contract G2 §2-§3, appendix A1): what
    /// [`Geometry::wide`] says.
    pub word_bits: u32,
    /// Bits in a level-1 map entry.
    pub l1_bits: u32,
    /// Bits in the PDL buffer's pointer and index.
    pub pdl_bits: u32,
    /// What the machine answers in functional source 16, if anything: QUUX's
    /// MACHINE-ID. The CADR drives nothing there and reads all ones.
    pub machine_id: Option<u32>,
    /// Whether ALU functions 42 and 43 are QUUX's one-instruction multiply
    /// and divide ([`crate::muldiv`]) rather than the CADR's.
    pub muldiv: bool,
    /// Whether the machine has QUUX's clocks ([`Timers`]): the interval
    /// timers on the register page, words 110-115, their interrupts in word
    /// 100 and reset devices at word 104 (contract Q11); and the
    /// microsecond clock, functional source 15 (contract Q1).
    pub tick: bool,
    /// Whether the mode register has `SPEED1` and `SPEED0`, which choose the
    /// delay-line tap that ends the read phase (`mit/cadr/ir.bits`). The
    /// CADR's do; QUUX runs at one rate and has no such bits.
    pub speed_bits: bool,
    /// Whether a RAM read in the microcycle that writes the same word ---
    /// a `POPJ` in a dispatch memory write, the map read in the microcycle
    /// its store's write lands --- gives the word from before the write.
    /// QUUX defines it so, as an FPGA's block RAM gives it. On the CADR the
    /// RAM's output floats while written and the board races; muir takes
    /// the netlist's answer there, the word written
    /// (`tests/dispatch_write_order.rs`).
    pub old_word_while_written: bool,
    /// Whether a microcycle that reads `MD` while a read is in flight is
    /// hung, as on the CADR: its read phase and write pulses run, then
    /// `-HANG` holds the next cycle's start until `-RDFINISH`. QUUX has no
    /// hung microcycle: such a microcycle waits, as for `-WAIT`, whole
    /// microcycles with no write pulse, and runs once the word is in `MD`.
    pub hangs: bool,
    /// Where the boot PROM sits: on the CADR over control store 0 until
    /// `PROMDISABLE` (`None`); on QUUX at [`QUUX_PROM_BASE`], read only and
    /// never overlaid, the reset PC, with no disable bit.
    pub prom_base: Option<u16>,
    /// Whether the machine has the Unibus (contract Q5). The CADR does: its
    /// I/O board, the bus interface's registers, the Unibus map and the
    /// debug cable are on it. QUUX does not: every address from physical
    /// page 37000 up answers nothing, a read or a write timing out as an
    /// empty Xbus address does, and its devices are on the register page.
    pub unibus: bool,
    /// Whether the machine has QUUX's real-time clock (contract Q9,
    /// revision 9): register page word 103, [`Rtc`].
    pub rtc: bool,
    /// Whether the machine has QUUX's file device (contract Q9, revision
    /// 9): register page words 160-171 and word 100 `<6>`,
    /// [`crate::file_device`].
    pub file_device: bool,
    /// Whether the machine has QUUX's MACRO-DISPATCH register, its MACRO
    /// DISPATCH MEMORY and the fused return (contract H8a):
    /// functional destinations 5 to 7, [`macro_dispatch`]. Without it those
    /// destinations write only M, as on the CADR.
    pub macro_dispatch: bool,
}

impl Geometry {
    /// The CADR's.
    pub const CADR: Geometry = Geometry {
        word_bits: 32,
        l1_bits: 5,
        pdl_bits: 10,
        machine_id: None,
        muldiv: false,
        tick: false,
        speed_bits: true,
        old_word_while_written: false,
        hangs: true,
        prom_base: None,
        unibus: true,
        rtc: false,
        file_device: false,
        macro_dispatch: false,
    };

    /// **QUUX's, revision 13** (contract G2, with its appendix A1): a
    /// 40-bit word (G2 §2.1) and what comes with it, all keyed on
    /// [`Geometry::wide`]. The BYTE fields are 6 bits, the JUMP and
    /// DISPATCH rotates take `IR<47>` as their bit 5, the DISPATCH address
    /// is `IR<23:12>` into a dispatch memory of 4,096 entries (A1.1,
    /// A1.4); the rotator is a ring of 40 and the masker empty for a byte
    /// that does not fit in bits 0-39 (A1.2); the conditions compare
    /// fields, equality sees the tag, and two conditions are new, fixnum
    /// overflow and unsigned less than (A1.3); the location counter has 30
    /// bits and its flags above bit 31 (A1.6); and the map is two levels
    /// of 8,192 7-bit and 4,096 28-bit entries over 1024-word pages, a
    /// 28-bit virtual address and an 18-bit physical page (A1.7).
    ///
    /// With it: the MACRO-DISPATCH register, the MACRO DISPATCH MEMORY and
    /// the fused return (contract H8a, [`macro_dispatch`]), functional
    /// destinations 5 to 7; the register page at the last page of the
    /// physical space, [`REGISTER_PAGE_13`], with block-disk and the video
    /// controller on it and word 100 in its final order (contract Q13,
    /// [`Machine::interrupt_sources`]); three interval timers and reset
    /// devices on the register page (contract Q11, [`Timers`],
    /// [`Machine::reset_devices`]); a real-time clock and a file device on
    /// the register page (contract Q9, [`Rtc`], [`crate::file_device`]);
    /// main memory and the frame buffer on its own port through its cache,
    /// with no bus interface (contract Q6), and its devices reached by
    /// their registers alone, with no bus (contract Q7,
    /// [`crate::memory_port`]); its boot PROM at control store 36000
    /// (contract Q2, [`Geometry::prom_base`]); the microsecond clock in the
    /// processor, functional source 15, and timer 0 of the interval timers
    /// the tick ([`Timers`]); `MUL` and `DIV` in one instruction each, ALU
    /// functions 42 and 43 ([`crate::muldiv`]); and a PDL buffer of 16K
    /// words, its pointer and index 14 bits. The rest of the machine is
    /// the CADR's.
    ///
    /// It says so in functional source 16, its MACHINE-ID, which no
    /// microcode of MIT's reads and nothing on the CADR drives: the
    /// signature `0x5155` in bits 31:16, the hardware revision in 15:4,
    /// 13, and the processor type, 4, in 3:0. Feature words 1, 2 and 6
    /// give the sizes ([`Geometry::feature_word`]). A CADR's open bus
    /// reads all ones there, which can never carry the signature.
    pub const QUUX: Geometry = Geometry {
        word_bits: 40,
        l1_bits: 7,
        pdl_bits: 14,
        machine_id: Some((0x5155 << 16) | (13 << 4) | 4),
        muldiv: true,
        tick: true,
        speed_bits: false,
        old_word_while_written: true,
        hangs: false,
        prom_base: Some(QUUX_PROM_BASE),
        unibus: false,
        rtc: true,
        file_device: true,
        macro_dispatch: true,
    };

    /// **QUUX's, revision 14** (contract G3 revision 14, appendix A14):
    /// revision 13 with a 32-bit virtual address space, its two windows,
    /// and a page table walked by hardware behind a TLB ([`crate::tlb`])
    /// in place of the two map levels, so no level-1 entry (feature word 1
    /// reads 0); the location counter at 34 bits with its adder (A14.11);
    /// jump condition 12; the memory system's register-page words 220-224
    /// (A14.9). All of it keyed on [`Geometry::paged`].
    pub const QUUX_14: Geometry =
        Geometry { l1_bits: 0, machine_id: Some((0x5155 << 16) | (14 << 4) | 4), ..Geometry::QUUX };

    /// **QUUX's, revision 15** (contract G3 revision 15, appendix A15b), on
    /// `micro`: revision 14's machine with a 64-bit microinstruction, MIT's
    /// 48 bits and an extension (A15b.2); the OA registers read only through
    /// a word's OA select, in place of IMOD (A15b.15); its own `.mcr`
    /// (A15b.7); feature word 25 and register-page word 225 (A15b.1). All of
    /// it keyed on [`Geometry::extended`]. `rtl` does not run it.
    pub const QUUX_15: Geometry =
        Geometry { machine_id: Some((0x5155 << 16) | (15 << 4) | 4), ..Geometry::QUUX_14 };

    /// Whether the machine translates through the TLB, revision 14's
    /// ([`Geometry::QUUX_14`]), rather than through two map levels.
    pub fn paged(self) -> bool {
        self.revision().is_some_and(|r| r >= 14)
    }

    /// Whether the microinstruction is 64 bits with the OA selects in place
    /// of IMOD, revision 15's ([`Geometry::QUUX_15`]).
    pub fn extended(self) -> bool {
        self.revision().is_some_and(|r| r >= 15)
    }

    /// The level-1 entry a CADR's map store writes: `VMA<31:27>`
    /// (`mit/cadr/ir.bits`, "VMA<26>=1 writes the level 1 map from
    /// VMA<31-27>"). Revision 13 writes its own ([`Machine::write_map_13`]).
    pub fn l1_from_vma(self, vma: u32) -> u32 {
        (vma >> 27) & self.l1_mask()
    }

    /// QUUX's revision, from its MACHINE-ID's `<15:4>`: 13 or 14, and
    /// `None` on the CADR, which has no MACHINE-ID.
    pub fn revision(self) -> Option<u32> {
        self.machine_id.map(|id| id >> 4 & 0o7777)
    }

    /// Main memory by default, in 64K-word boards: [`MAIN_WORDS`], and
    /// [`MAIN_WORDS_13`] on a 40-bit machine.
    pub fn default_memory_boards(self) -> usize {
        (if self.wide() { MAIN_WORDS_13 } else { MAIN_WORDS }) >> 16
    }

    /// The most main memory, in 64K-word boards: sixty where the Xbus I/O
    /// space begins, [`busint::MAX_MEMORY_BOARDS`], and
    /// [`MAX_MAIN_WORDS_13`] on a 40-bit machine.
    pub fn max_memory_boards(self) -> usize {
        if self.wide() { MAX_MAIN_WORDS_13 >> 16 } else { busint::MAX_MEMORY_BOARDS }
    }

    /// QUUX's main memory in whole megawords, as `--main-memory-size` takes
    /// it: from 1MW to as many whole MW as [`Geometry::max_memory_boards`]
    /// holds, 64MW.
    pub fn main_memory_mw(self) -> std::ops::RangeInclusive<usize> {
        1..=self.max_memory_boards() >> 4
    }

    /// A word's bits, [`Geometry::word_bits`] of ones.
    pub fn word_mask(self) -> Word {
        (1 << self.word_bits) - 1
    }

    /// Whether a word is wider than 32 bits, `<39:32>` above the CADR's:
    /// revision 13, and with it its fields, rotator, conditions, dispatch
    /// memory, location counter and map ([`Geometry::QUUX`]).
    pub fn wide(self) -> bool {
        self.word_bits > 32
    }

    /// The dispatch memory's entries: 2,048 (`IR<22:12>`), and 4,096
    /// (`IR<23:12>`) on revision 13 (contract G2 §2.4, A1.4).
    pub fn dmem_words(self) -> usize {
        if self.wide() { DMEM_WORDS } else { 2048 }
    }

    /// The location counter's own bits, the counter without the flags an
    /// engine keeps beside it: `LC<25:0>` ([`LC_COUNTER`]), and on revision
    /// 13 `LC<29:0>`, a byte address of a 28-bit word address (A1.6), and
    /// on revision 14 `LC<33:0>`, of a 32-bit one (A14.11).
    pub fn lc_counter(self) -> u64 {
        if self.paged() {
            LC_COUNTER_14
        } else if self.wide() {
            u64::from(LC_COUNTER_13)
        } else {
            u64::from(LC_COUNTER)
        }
    }

    /// Where `micro` carries `NEEDFETCH` in [`Machine::lc`]: bit 31, above
    /// the counter, and on revision 14, whose counter has bit 31, bit 40.
    pub fn need_fetch(self) -> u64 {
        if self.paged() { 1 << 40 } else { 1 << 31 }
    }

    /// The map word the latch at VMEMDR 1D14 holds before any memory cycle
    /// has loaded it: [`LVMO_AT_POWER_ON`], and on revision 13 the same
    /// with its access bits at `<27:26>` and its page 18 bits (A1.7).
    pub fn lvmo_at_power_on(self) -> u32 {
        if self.wide() { (1 << 27) | (1 << 26) | 0o777777 } else { LVMO_AT_POWER_ON }
    }

    /// A level-1 entry's bits.
    pub fn l1_mask(self) -> u32 {
        (1 << self.l1_bits) - 1
    }

    /// The level-2 entry a level-1 entry and an address select: the block
    /// the entry names, and `VMA<12:8>` in it.
    pub fn l2_index(self, l1: u32, addr: u32) -> usize {
        (((l1 & self.l1_mask()) << 5) | ((addr >> 8) & 0o37)) as usize
    }

    /// The PDL pointer's and index's bits.
    pub fn pdl_mask(self) -> u16 {
        (1 << self.pdl_bits) - 1
    }

    /// Whether this machine has the register page, and with it QUUX's
    /// decode of its physical space ([`busint::decode_quux_13`]): main
    /// memory, the frame buffer window, the page, and nothing else.
    pub fn has_register_page(self) -> bool {
        self.machine_id.is_some()
    }

    /// The word of the feature page at physical address `phys`, if this
    /// machine has one and `phys` is on it: the MACHINE-ID, then the
    /// level-1 entry's bits, the level-2 map's entries, the PDL buffer's
    /// words, the control store's, A memory's and dispatch memory's, and
    /// which of `MUL` (bit 0) and `DIV` (bit 1) it has, and whether it has
    /// the tick, timer 0 (1); words 11 to 13, the main screen, are the
    /// display's ([`Machine::bus_read`]); word 14 the microsecond clock;
    /// word 15 the optional devices, a bit each, `<0>` the real-time clock
    /// and `<1>` the file device, a later optional device taking the next
    /// bit; word 16 the number of interval timers, 3; word 17 the MACRO
    /// DISPATCH MEMORY's entries, 1,024; words 20 to 24 the board name, the
    /// machine's ([`Machine::set_board_name`]); every other word 0.
    /// Read-only, as every word 0-77 is.
    ///
    /// The page is at `1777777400`, the last page of the 28-bit physical
    /// space ([`REGISTER_PAGE_13`]), and on revision 14 at bus address
    /// [`crate::tlb::REGISTER_PAGE_BUS`], with the same offsets.
    pub fn feature_word(self, phys: u32) -> Option<u32> {
        let id = self.machine_id?;
        let page = if self.paged() { crate::tlb::REGISTER_PAGE_BUS } else { REGISTER_PAGE_13 };
        if phys & !0o377 != page {
            return None;
        }
        Some(match phys & 0o377 {
            0 => id,
            1 => self.l1_bits,
            2 => 32 << self.l1_bits,
            3 => 1 << self.pdl_bits,
            4 => IMEM_WORDS as u32,
            5 => 1024,
            6 => self.dmem_words() as u32,
            7 => (self.muldiv as u32) * 3,
            // The tick, timer 0.
            0o10 => self.tick as u32,
            // The microsecond clock (revision 5).
            0o14 => self.tick as u32,
            // The optional devices, a bit each (contracts Q9, Q13): `<0>`
            // the real-time clock, `<1>` the file device.
            0o15 => self.rtc as u32 | (self.file_device as u32) << 1,
            // The number of interval timers (contract Q11, revision 10).
            0o16 => self.tick as u32 * Timers::COUNT,
            // The MACRO DISPATCH MEMORY's entries (contract H8a).
            0o17 => self.macro_dispatch as u32 * macro_dispatch::ENTRIES as u32,
            _ => 0,
        })
    }
}

/// **QUUX's MACRO-DISPATCH register and MACRO DISPATCH MEMORY, the fused
/// return and the operand address** (contract H8a). Functional
/// destinations 5 to 7, which the CADR leaves to its low group with no
/// decoder output, so that there they write only M.
///
/// **The register**, destination 5: `<13:0>` the main loop's address,
/// microcode 2001's `QMLP`; `<23:14>` the A-memory address of `A-LOCALP`
/// (`uc-parameters.lisp:1192`) and `<28:24>` the M-memory address of `M-AP`
/// (`uc-parameters.lisp:475`), the operand address's two bases; `<31>` the
/// enable. `<30:29>` are reserved, written 0 and kept 0 here. -RESET clears
/// the enable and nothing else, and so does every control-store write, so
/// that no microcode runs on the entries another one left.
///
/// **The memory**, 1,024 entries indexed by the halfword's `<15:6>`, its
/// destination, opcode and register together. Destination 6 writes the
/// index, `<9:0>`, and destination 7 the entry at the index, `<17:0>`:
/// D-MEM's word, `<13:0>` the handler's address, `<14>` N, `<15>` P and
/// `<16>` R, and `<17>` the operand bit. A reset leaves the index and the
/// entries, as it leaves D-MEM.
///
/// **The fused return.** A microinstruction that pops the micro stack as a
/// return --- a POPJ, a dispatch whose entry has R, or, while
/// [`JUMP_RETURNS_FUSE`] says so, a jump with R --- and pops a word with
/// `<14>` set and `<13:0>` the register's, with the register enabled and no
/// instruction fetch needed, would go to the main loop's dispatch today:
/// `QMLP+2`, where the stream hardware's `SPCMUNG` sends it
/// (`uc-macrocode.lisp:6-7`). That dispatch is `(DISPATCH-XCT-NEXT
/// M-INST-OP OPDTB)` and its XCT-NEXT the push of `A-MAIN-DISPATCH` back
/// (`uc-macrocode.lisp:9-13`). When the entry for the next halfword has R
/// and P clear, the return goes to the entry's address instead, two
/// microcycles sooner: the popped word stays on the stack, as the push
/// would put it back, unless the entry's N is set, which would have nopped
/// the push. The next halfword is the one the main loop's dispatch would
/// take: M 31, `M-INST-BUFFER`, rotated as `IR<11:10>` = 3 rotates it, by
/// the location counter as the pop's `NEXT INSTR` steps it in the
/// microcycle after, [`INDEX_ROTATE`]. The step, and the fetch it may
/// start, are the stream's as ever.
///
/// Condition 6 is not tested: with no fetch the main loop goes to `QMLP+2`
/// and does not test it either. The return runs today's path, `QMLP` or
/// `QMLP+2`, when the pop is the functional source's, when the same
/// microinstruction pushes, writes M 31 or INTERRUPT-CONTROL (whose byte
/// mode chooses the halfword), or steps the location counter itself
/// (`LCINC`: a `NEXT INSTR` the microcycle before, or a dispatch's
/// `IR<24>`), and when the entry has R or P. A return that needs a fetch
/// fuses on the word QUUX's cache-only prefetch holds
/// (`crate::memory_port`, contract H8a §3.5), when that is the next word
/// in sequence and condition 6, which the main loop tests on the fetch
/// path, is false; M 31, a register beside M memory, takes the word at the
/// end of the microcycle after the return, [`MacroDispatch::m31`]. The
/// prefetch looks in the cache, and `micro` has no cache: there a
/// return that needs a fetch never fuses. So `rtl` fuses
/// more returns than `micro` does and takes fewer microcycles, and the two
/// leave the same state wherever the microcode keeps the rule below.
///
/// **The operand address** (contract H8a §3.4). When a fused return's entry
/// has the operand bit and the halfword's register, `<8:6>`, is LOCAL (5)
/// or ARG (6) (`QADCM1`, `uc-macrocode.lisp:129-137`), PDL-INDEX is loaded
/// at the end of the microcycle after the return with `A-LOCALP` + delta or
/// `M-AP` + 1 + delta, as `QADLOC1` and `QADARG1` compute them
/// (`uc-macrocode.lisp:237-245`), delta being the halfword's `<5:0>` and
/// the sum masked to PDL-INDEX's bits. So the handler's first
/// microinstruction finds its operand at `C-PDL-BUFFER-INDEX`. The two
/// bases are the machine's own copies, [`MacroDispatch::localp`] and
/// [`MacroDispatch::ap`], fourteen bits each, written only with an A write
/// at the address the register's `<23:14>` names and an M write at its
/// `<28:24>`, with their write pulse. The register's write does not load
/// them, so the microcode writes `A-LOCALP` and `M-AP` after destination
/// 5 (contract H8a §3.4, §4); -RESET leaves them, and a checkpoint keeps
/// them.
///
/// The microcycle after a fused return must not write the location
/// counter, M 31, INTERRUPT-CONTROL or destinations 5 to 7, and, when the
/// entry has the operand bit, PDL-INDEX, `A-LOCALP`, `M-AP` or the PDL
/// buffer by PDL-INDEX (contract H8a §3.3): the handler and the operand
/// address are chosen by then, and a PDL buffer write lands at PDL-INDEX
/// as it stands in the next microcycle, after the load. Nor may the
/// handler's first microinstruction read the PDL buffer at the address
/// that microcycle writes, at the pointer or by PDL-INDEX: the buffer has
/// no pass-around, and the word lands in the handler's own write pulse,
/// after the read, where today's path has the main loop's two microcycles
/// in between. A microcode that breaks these rules is changed, never
/// covered by the hardware; `tests/support/macro_dispatch.rs` checks them.
pub mod macro_dispatch {
    /// `<31>` of the register, the enable.
    pub const ENABLE: u32 = 1 << 31;
    /// `<30>` of the register on revision 15, D's enable, with `<31>`
    /// (contract G3 revision 15, A15b.9): a return that would fuse but for
    /// a needed fetch dispatches on the fetched word. Below revision 15 the
    /// bit is reserved and not kept.
    pub const D_ENABLE: u32 = 1 << 30;
    /// The register's bits kept: `<31>` and `<28:0>`.
    pub const REGISTER_BITS: u32 = ENABLE | 0o3777777777;
    /// The MACRO DISPATCH MEMORY's entries.
    pub const ENTRIES: usize = 1024;
    /// An entry's bits: D-MEM's seventeen and the operand bit.
    pub const ENTRY_BITS: u32 = 0o777777;
    /// `<14>` of an entry, N.
    pub const N: u32 = 1 << 14;
    /// `<15>` of an entry, P.
    pub const P: u32 = 1 << 15;
    /// `<16>` of an entry, R.
    pub const R: u32 = 1 << 16;
    /// `<17>` of an entry, the operand bit.
    pub const OPERAND: u32 = 1 << 17;
    /// The rotate, as `IR<4:0>` under `IR<11:10>` = 3, that brings the
    /// halfword's `<6>` to `<0>`, so that its `<15:6>` is the index: the
    /// location counter chooses the halfword as it does for the main loop's
    /// dispatch, whose `IR<4:0>` is 23 for `M-INST-OP`'s `<13:9>`.
    pub const INDEX_ROTATE: u32 = 32 - 6;

    /// [`INDEX_ROTATE`] in a ring of 32, and on revision 13 in the ring of
    /// 40: 34, which for halfword 1, under LC byte mode's addend of 24,
    /// gives 18 and brings the halfword's `<6>`, bit 22, to `<0>`
    /// (contract G2 appendix A1.2).
    pub fn index_rotate(wide: bool) -> u32 {
        if wide { 40 - 6 } else { INDEX_ROTATE }
    }
    /// The halfword's register, `<8:6>`, that names the local block:
    /// `QADLOC` in `QADCM1` (`uc-macrocode.lisp:129-137`).
    pub const LOCAL: u32 = 5;
    /// The halfword's register that names the argument block, `QADARG`.
    pub const ARG: u32 = 6;
    /// The base copies' width, fourteen bits (contract H8a §7).
    pub const BASE_BITS: u32 = 0o37777;

    /// **Whether a return by a jump with R fuses**, as one by a POPJ or a
    /// dispatch whose entry has R does. Microcode 2001 returns that way
    /// with `POPJ-LESS-THAN` and its fellows. Contract H8a §3.3 names POPJs
    /// and dispatches only; this is the one switch that takes jumps out, on
    /// both engines.
    pub const JUMP_RETURNS_FUSE: bool = true;

    /// The register's word for a main loop at `main`, `A-LOCALP` at A
    /// memory's `localp` and `M-AP` at M memory's `ap`, enabled.
    pub fn word(main: u16, localp: u16, ap: u8) -> u32 {
        ENABLE | (ap as u32 & 0o37) << 24 | (localp as u32 & 0o1777) << 14 | main as u32 & 0o37777
    }

    /// The main loop's address, `<13:0>`.
    pub fn main(register: u32) -> u32 {
        register & 0o37777
    }

    /// `A-LOCALP`'s A-memory address, `<23:14>`.
    pub fn localp_address(register: u32) -> usize {
        (register >> 14 & 0o1777) as usize
    }

    /// `M-AP`'s M-memory address, `<28:24>`.
    pub fn ap_address(register: u32) -> usize {
        (register >> 24 & 0o37) as usize
    }

    /// The operand address a fused return arms for `rotated`, the halfword
    /// rotated by `index_rotate` ([`index_rotate`]), whose entry is
    /// `entry`: the register from `<2:0>` of the index and delta from the
    /// six bits at `index_rotate`, `<31:26>` in a ring of 32 and `<39:34>`
    /// in one of 40, the halfword's `<8:6>` and `<5:0>`.
    pub fn operand(entry: u32, rotated: super::Word, index_rotate: u32) -> Option<super::Operand> {
        let register = rotated as u32 & 7;
        (entry & OPERAND != 0 && (register == LOCAL || register == ARG)).then_some(super::Operand {
            arg: register == ARG,
            delta: (rotated >> index_rotate) as u8 & 0o77,
        })
    }
}

/// A fused return's operand address (contract H8a §3.4), armed by the
/// return and loaded into PDL-INDEX at the end of the microcycle after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Operand {
    /// ARG, `M-AP` + 1 + delta, rather than LOCAL, `A-LOCALP` + delta.
    pub arg: bool,
    /// The halfword's `<5:0>`.
    pub delta: u8,
}

/// A fused return, as [`MacroDispatch::fused_return`] gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fused {
    /// The handler's address, the entry's `<13:0>`.
    pub handler: u16,
    /// Whether the popped word stays on the stack: the entry's N is clear.
    pub keep: bool,
    /// The operand address it arms, if the entry has the operand bit and
    /// the register is LOCAL or ARG.
    pub operand: Option<Operand>,
}

/// The writes an engine's last microcycle handed the next, between two
/// microcycles: a read's word for `MD`, the PDL buffer's write, the SPC
/// stack's (`Micro::pending`, `Pipeline::pending`). For a comparison of the
/// state between microcycles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pending {
    pub md: Option<Word>,
    pub pdl: Option<(u16, Word)>,
    pub spc: Option<(u8, u32)>,
}

impl Pending {
    /// Lands them in `m`.
    pub fn land(self, m: &mut Machine) {
        if let Some(w) = self.md {
            m.md = w;
        }
        if let Some((adr, w)) = self.pdl {
            m.pdl[adr as usize] = w;
        }
        if let Some((ptr, w)) = self.spc {
            m.spc[ptr as usize] = w;
        }
    }
}

/// QUUX's MACRO-DISPATCH register and MACRO DISPATCH MEMORY
/// ([`macro_dispatch`]).
#[derive(Clone)]
pub struct MacroDispatch {
    /// The register, destination 5.
    pub register: u32,
    /// The index destination 6 writes and destination 7 writes at.
    pub index: u16,
    /// The entries.
    pub entries: Vec<u32>,
    /// The copy of `A-LOCALP`, fourteen bits: the last A write at the
    /// register's `<23:14>` ([`macro_dispatch`]). Kept in a checkpoint.
    pub localp: u32,
    /// The copy of `M-AP`, fourteen bits: the last M write at the
    /// register's `<28:24>`. Kept in a checkpoint.
    pub ap: u32,
    /// The operand address armed by a fused return, loaded into PDL-INDEX
    /// at the end of the next microcycle.
    pub operand: Option<Operand>,
    /// The word a fused return on the fetch path arms for M 31, a register
    /// beside M memory: `rtl`'s prefetched word (`crate::memory_port`), or
    /// the word revision 15's D dispatched on, on `micro`; loaded into it at
    /// the end of the next microcycle. Kept in a checkpoint.
    pub m31: Option<Word>,
    /// How many returns have been fused: a count for the profile and the
    /// tests, not kept in a checkpoint.
    pub fused: u64,
}

impl Default for MacroDispatch {
    fn default() -> Self {
        MacroDispatch {
            register: 0,
            index: 0,
            entries: vec![0; macro_dispatch::ENTRIES],
            localp: 0,
            ap: 0,
            operand: None,
            m31: None,
            fused: 0,
        }
    }
}

impl MacroDispatch {
    /// -RESET, and every control-store write: the enable cleared, the rest
    /// kept.
    pub fn disable(&mut self) {
        self.register &= !macro_dispatch::ENABLE;
    }

    /// -RESET: the enable cleared, and an armed operand address and M 31
    /// word dropped; the base copies kept.
    pub fn reset(&mut self) {
        self.disable();
        self.operand = None;
        self.m31 = None;
    }

    /// A write of functional destination 5, 6 or 7 (`code`). The
    /// register's write leaves the base copies as they are: they follow the
    /// writes of the words it names from then on.
    pub fn write(&mut self, code: u32, data: u32) {
        match code {
            5 => self.register = data & macro_dispatch::REGISTER_BITS,
            6 => self.index = (data & (macro_dispatch::ENTRIES as u32 - 1)) as u16,
            7 => self.entries[self.index as usize] = data & macro_dispatch::ENTRY_BITS,
            _ => unreachable!("destination {code:o} is not the MACRO DISPATCH MEMORY's"),
        }
    }

    /// A write of A memory at `adr`: the copy of `A-LOCALP` takes it when
    /// the register names that address. An M destination writes A as well,
    /// and calls this too.
    pub fn a_written(&mut self, adr: usize, v: Word) {
        if adr == macro_dispatch::localp_address(self.register) {
            self.localp = v as u32 & macro_dispatch::BASE_BITS;
        }
    }

    /// A write of M memory at `adr`: the copy of `M-AP` takes it when the
    /// register names that address.
    pub fn m_written(&mut self, adr: usize, v: Word) {
        if adr == macro_dispatch::ap_address(self.register) {
            self.ap = v as u32 & macro_dispatch::BASE_BITS;
        }
    }

    /// The operand address `o` makes from the base copies, before PDL-INDEX's
    /// mask: `A-LOCALP` + delta, or `M-AP` + 1 + delta.
    pub fn operand_address(&self, o: Operand) -> u32 {
        (if o.arg { self.ap + 1 } else { self.localp }) + o.delta as u32
    }

    /// The fused return ([`macro_dispatch`]) for a pop of `popped`, with
    /// `rotated` the word M 31 gives rotated by `index_rotate`
    /// ([`macro_dispatch::index_rotate`]) under the location counter as
    /// the main loop's dispatch would see it:
    /// the handler's address, whether the popped word stays on the stack,
    /// and the operand address it arms. The engine has checked everything
    /// the microinstruction does; `None` is today's return.
    pub fn fused_return(&self, popped: u32, rotated: Word, index_rotate: u32) -> Option<Fused> {
        use macro_dispatch::{ENABLE, N, P, R, main};
        if self.register & ENABLE == 0
            || popped & (1 << 14) == 0
            || popped & 0o37777 != main(self.register)
        {
            return None;
        }
        let entry = self.entries[(rotated as usize) & (macro_dispatch::ENTRIES - 1)];
        if entry & (R | P) != 0 {
            return None;
        }
        Some(Fused {
            handler: (entry & 0o37777) as u16,
            keep: entry & N == 0,
            operand: macro_dispatch::operand(entry, rotated, index_rotate),
        })
    }

    fn save(&self, w: &mut crate::checkpoint::Writer) {
        w.u32(self.register);
        w.u16(self.index);
        w.u32s(&self.entries);
        w.u32(self.localp);
        w.u32(self.ap);
        w.opt(self.operand, |w, o| {
            w.bool(o.arg);
            w.u8(o.delta);
        });
        w.opt(self.m31, |w, v| w.word(v));
    }

    fn load(&mut self, r: &mut crate::checkpoint::Reader) -> std::io::Result<()> {
        self.register = r.u32()?;
        self.index = r.u16()?;
        r.u32s_into(&mut self.entries)?;
        self.localp = r.u32()? & macro_dispatch::BASE_BITS;
        self.ap = r.u32()? & macro_dispatch::BASE_BITS;
        self.operand = r.opt(|r| Ok(Operand { arg: r.bool()?, delta: r.u8()? & 0o77 }))?;
        self.m31 = r.opt(|r| r.word())?;
        if self.index as usize >= macro_dispatch::ENTRIES {
            return Err(crate::checkpoint::bad(format!(
                "MACRO DISPATCH MEMORY index {:o}, wider than its ten bits",
                self.index
            )));
        }
        Ok(())
    }
}

/// **QUUX's real-time clock** (contract Q9, revision 9): register page word
/// 103, the real time in whole seconds since 1970-01-01 UTC, unsigned 32
/// bits, good to 2106. Read-only to the machine: a write goes nowhere, as a
/// write of any read-only word on the page does, and the host keeps the
/// time. Finer time is the microsecond clock's ([`Timers::microseconds`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Rtc {
    /// Live, the default: the host's clock at each read, as a real RTC keeps
    /// real time on its own crystal whatever the processor does.
    #[default]
    Host,
    /// `--rtc <start>`, for runs that repeat: `start` when the machine's
    /// clock read `base_ns`, and a second more for each 10^9 ns of the
    /// machine's own time since. It holds at 2^32-1 rather than wrap to 0,
    /// which would read as no RTC at all.
    Counted { start: u32, base_ns: u64 },
}

impl Rtc {
    /// The word 103 reads when the machine's clock is at `ns`.
    pub fn seconds(self, ns: u64) -> u32 {
        self.seconds_at(ns, 1_000_000_000)
    }

    /// The word 103 reads when the machine's clock, of `per_s` units a
    /// second ([`Machine::time_base`]), is at `now`: on revision 15
    /// `base_ns` and `now` are in its units of 0.5 ns.
    pub fn seconds_at(self, now: u64, per_s: u64) -> u32 {
        let s = match self {
            Rtc::Host => std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            Rtc::Counted { start, base_ns } => start as u64 + now.saturating_sub(base_ns) / per_s,
        };
        s.min(u32::MAX as u64) as u32
    }

    pub fn save(self, w: &mut crate::checkpoint::Writer) {
        w.opt(
            match self {
                Rtc::Host => None,
                Rtc::Counted { start, base_ns } => Some((start, base_ns)),
            },
            |w, (start, base_ns)| {
                w.u32(start);
                w.u64(base_ns);
            },
        );
    }

    pub fn load(r: &mut crate::checkpoint::Reader) -> std::io::Result<Rtc> {
        Ok(match r.bool()? {
            false => Rtc::Host,
            true => Rtc::Counted { start: r.u32()?, base_ns: r.u64()? },
        })
    }
}

/// One of QUUX's interval timers (contract Q11, revision 10): timer 0, 1
/// or 2 on the register page, each with its control and status word at
/// 110 + 2k and its period at 111 + 2k.
///
/// | Word | Read | Write |
/// |---|---|---|
/// | 110 + 2k | `<0>` on, `<1>` its flag, `<2>` its mode (0 periodic, 1 one-shot), `<8>` its interrupt enable; the rest 0 | `<0>` on; `<1>` set clears the flag; `<2>` the mode, taken only by a write that turns it on; `<8>` the interrupt enable, taken by every write; the rest ignored |
/// | 111 + 2k | the period in µs, `<23:0>`, as last written | `<23:0>` the period; the rest ignored |
///
/// A write that turns the timer on starts a period from itself and takes
/// its mode; one that leaves it on starts nothing and changes no mode; one
/// that turns it off takes its flag down. A period written while it is on
/// starts a period from the write, and so takes the flag down: a rise up
/// and not yet taken is lost. **Periodic**, the flag rises a period after
/// the start and every period after, on the start's grid, whether or not
/// it was cleared between; while it is up, further rises merge into it.
/// **One-shot**, it rises once, a period after the start, and the timer
/// stays on with nothing to count until a period write or an off-then-on
/// starts it again; a clear starts nothing. A timer on at period 0 never
/// rises. The flag rises whatever the interrupt enable says; under it, the
/// flag is the timer's bit of word 100 ([`Machine::interrupt_sources`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntervalTimer {
    pub on: bool,
    /// The mode, taken at the turn-on: one-shot, or periodic.
    pub one_shot: bool,
    /// `<8>`: whether the flag is word 100's bit and an interrupt.
    pub interrupt_enable: bool,
    /// `<23:0>` of the last period written, µs.
    pub period_us: u32,
    /// When the flag next rises, or rose and has not been cleared;
    /// `u64::MAX`, never.
    pub deadline_ns: u64,
}

impl IntervalTimer {
    /// Where power-on, `-RESET`, `-BOOT` and reset devices put every timer:
    /// off, flag down, periodic, interrupt enable 0, period 0.
    pub const RESET: IntervalTimer = IntervalTimer {
        on: false,
        one_shot: false,
        interrupt_enable: false,
        period_us: 0,
        deadline_ns: u64::MAX,
    };

    /// The control word's bits.
    pub const ON: u32 = 1;
    pub const FLAG: u32 = 2;
    pub const ONE_SHOT: u32 = 4;
    pub const INTERRUPT_ENABLE: u32 = 1 << 8;

    /// The flag, at `now`: a rise at `now` counts as before it.
    pub fn flag(self, now: u64) -> bool {
        now >= self.deadline_ns
    }

    /// The timer's bit of word 100 at `now`: the flag under the interrupt
    /// enable.
    pub fn interrupt(self, now: u64) -> bool {
        self.interrupt_enable && self.flag(now)
    }

    /// Its control and status word at `now`.
    pub fn status(self, now: u64) -> u32 {
        self.on as u32
            | (self.flag(now) as u32) << 1
            | (self.one_shot as u32) << 2
            | if self.interrupt_enable { Self::INTERRUPT_ENABLE } else { 0 }
    }

    /// A deadline a period after `now`, or never if the period is 0, in a
    /// time of `per_us` units a microsecond ([`Machine::time_base`]).
    fn after(now: u64, period_us: u32, per_us: u64) -> u64 {
        if period_us == 0 { u64::MAX } else { now + period_us as u64 * per_us }
    }

    /// A write of its control word at `now`, in a time of `per_us` units a
    /// microsecond.
    pub fn write_control(&mut self, now: u64, v: u32, per_us: u64) {
        let on = v & Self::ON != 0;
        if on && !self.on {
            self.one_shot = v & Self::ONE_SHOT != 0;
            self.deadline_ns = Self::after(now, self.period_us, per_us);
        } else if v & Self::FLAG != 0 && self.flag(now) {
            self.deadline_ns = if self.one_shot || self.period_us == 0 {
                // A one-shot counts nothing more once it has risen.
                u64::MAX
            } else {
                // The next boundary of the start's grid after `now`.
                let p = self.period_us as u64 * per_us;
                self.deadline_ns + (now - self.deadline_ns) / p * p + p
            };
        }
        if !on {
            self.deadline_ns = u64::MAX;
        }
        self.on = on;
        self.interrupt_enable = v & Self::INTERRUPT_ENABLE != 0;
    }

    /// A write of its period word at `now`, in a time of `per_us` units a
    /// microsecond.
    pub fn write_period(&mut self, now: u64, v: u32, per_us: u64) {
        self.period_us = v & 0o77777777;
        if self.on {
            self.deadline_ns = Self::after(now, self.period_us, per_us);
        }
    }
}

/// QUUX's clocks: the three interval timers of contract Q11 on the
/// register page ([`IntervalTimer`], revision 10), timer 0 of them the tick
/// that takes the place of the CADR display's vertical interrupt, and the
/// microsecond clock of contract Q1 in the processor, functional source 15
/// ([`Timers::microseconds`]).
///
/// Every timer is reached through the register page alone. Q1's
/// functional destinations 3 and 4 write only M and its source 17 reads all
/// ones, as on the CADR: Q1's tick control, interval timer and status are
/// gone, and the page's timers take their place.
///
/// None of this is the CADR's: page SOURCE decodes no destination 3 or 4
/// and no source 15 or 17. Microcode 323 neither writes the one nor reads
/// the other, by a scan of every control-store word and by running it with
/// every executed word read as the OA registers left it
/// (`tests/unused_codes.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timers {
    pub timer: [IntervalTimer; 3],
}

impl Timers {
    /// Timer 0's period for a 60-cycle tick, 16,667 µs, what the display's
    /// vertical interrupt was; the boot PROM writes it (contract Q11). No
    /// timer resets to it.
    pub const TICK_PERIOD_US: u32 = 16_667;

    /// Word 100's bit for each timer: `<0>`, `<1>`, `<2>` (contract Q13).
    pub const INTERRUPT_BITS: [u32; 3] = [1 << 0, 1 << 1, 1 << 2];

    /// How many there are, feature word 16.
    pub const COUNT: u32 = 3;

    /// Every timer in its reset state.
    pub const fn new() -> Timers {
        Timers { timer: [IntervalTimer::RESET; 3] }
    }

    /// Word 100's timer bits at `now`.
    pub fn interrupt_sources(&self, now: u64) -> u32 {
        (0..3).filter(|&k| self.timer[k].interrupt(now)).map(|k| Self::INTERRUPT_BITS[k]).sum()
    }

    /// Whether any timer's flag, under its interrupt enable, is up at `now`.
    pub fn pending(&self, now: u64) -> bool {
        self.timer.iter().any(|t| t.interrupt(now))
    }

    /// Which timer, and whether its period, register page word `word`
    /// names, if it is one of 110-115.
    fn word(word: u32) -> Option<(usize, bool)> {
        (0o110..=0o115).contains(&word).then(|| (((word - 0o110) / 2) as usize, word & 1 != 0))
    }

    /// A read of register page word `word` at `now`, if it is a timer's.
    pub fn read(&self, word: u32, now: u64) -> Option<u32> {
        let (k, period) = Self::word(word)?;
        let t = self.timer[k];
        Some(if period { t.period_us } else { t.status(now) })
    }

    /// A write of register page word `word` at `now`, in a time of
    /// `per_us` units a microsecond: whether it is a timer's.
    pub fn write(&mut self, word: u32, v: u32, now: u64, per_us: u64) -> bool {
        let Some((k, period)) = Self::word(word) else { return false };
        let t = &mut self.timer[k];
        if period {
            t.write_period(now, v, per_us)
        } else {
            t.write_control(now, v, per_us)
        }
        true
    }

    /// Source 15 at `now`: the microseconds since power-on, 32 bits,
    /// wrapping.
    pub fn microseconds(now: u64) -> u32 {
        (now / 1000) as u32
    }

    pub fn save(&self, w: &mut crate::checkpoint::Writer) {
        for t in &self.timer {
            w.bool(t.on);
            w.bool(t.one_shot);
            w.bool(t.interrupt_enable);
            w.u32(t.period_us);
            w.u64(t.deadline_ns);
        }
    }

    pub fn load(r: &mut crate::checkpoint::Reader) -> std::io::Result<Timers> {
        let mut timers = Timers::new();
        for t in &mut timers.timer {
            *t = IntervalTimer {
                on: r.bool()?,
                one_shot: r.bool()?,
                interrupt_enable: r.bool()?,
                period_us: r.u32()?,
                deadline_ns: r.u64()?,
            };
        }
        Ok(timers)
    }
}

impl Default for Timers {
    fn default() -> Timers {
        Timers::new()
    }
}

/// The PDL buffer's words on the largest machine: a QUUX with a 14-bit
/// pointer, 16K words.
pub const PDL_WORDS: usize = 16 * 1024;

/// The level-2 map's words on the largest machine, QUUX revision 13: 128
/// blocks of 32 (contract G2 §2.6, A1.7). The CADR has 32 of them.
pub const L2_MAP_WORDS: usize = 4096;

/// The level-1 map's entries on the largest machine, QUUX revision 13,
/// indexed by `VA<27:15>` (A1.7). The CADR has 2,048, indexed by
/// `VMA<23:13>`.
pub const L1_MAP_WORDS: usize = 8192;

/// The dispatch memory's entries on the largest machine, QUUX revision
/// 13 (contract G2 §2.4, A1.4); [`Geometry::dmem_words`].
pub const DMEM_WORDS: usize = 4096;

/// What revision 15's checkpoint writes in the level-1 entry's byte, which
/// revision 14's leaves 0 (A15b.13): its revision with `<7>` set, a value
/// no level-1 entry's width takes.
pub const REVISION_15_MARK: u8 = 0o200 | 15;

/// Why a microcycle could not complete.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Halt {
    /// A functional destination an engine does not implement.  No engine
    /// raises it now: every code decodes as page SOURCE decodes it, the
    /// unassigned ones included.
    UnknownDest { pc: u16, dest: u16 },
    /// **PDL-FIELD-MISMATCH** (revision 15, A15b.2): the word at `pc`
    /// carries a PDL address field whose index, `formed`, (B + D) AND
    /// 37777 of its base as the word found it, is not the index it wrote,
    /// `written`. Only a wrong assembly makes one; the word has committed.
    PdlFieldMismatch { pc: u16, formed: u16, written: u16 },
    /// **OA-OUTSIDE-FIELDS** (revision 15, A15b.15): the word at `pc`
    /// selects an OA register with `bits` set, in the word's `IR`
    /// positions, that are neither in the word nor in the fields its class
    /// takes from that register: where the CADR would run a word outside its
    /// fields, revision 15 stops before the word commits.
    OaOutsideFields { pc: u16, bits: u64 },
    /// **The OA select check's shadow** (revision 15, A15b.15), under
    /// `Micro::oa_select_check`: the word at `pc` selects OA-REG-HIGH
    /// (`high`) or OA-REG-LOW, and the word executed before it did not
    /// write that register, so IMOD would not have modified it.
    OaSelectWithoutWrite { pc: u16, high: bool },
    /// The same check: the word executed before the one at `pc` wrote
    /// OA-REG-HIGH (`high`) or OA-REG-LOW, and the word at `pc` does not
    /// select it, where IMOD would have modified it.
    OaWriteWithoutSelect { pc: u16, high: bool },
    /// **The WRITE-I-MEM check** (proposed name; revision 15, WRITE-I-MEM
    /// ruling), under `Micro::oa_select_check`: the word at `pc` is a
    /// WRITE-I-MEM outside MIT's form, `IR<9:0>` 1647 without POPJ, or run
    /// as a delay slot (`in_slot`).
    WriteImemRefused { pc: u16, in_slot: bool },
}

/// The location counter itself, `LC<25:0>`: the 74S169 counters on page LC
/// and nothing else.
///
/// Both engines keep other things in the same word or beside it ---
/// `Machine::lc` carries NEED-FETCH in bit 31 and the interrupt-control
/// flags in 29:26, and `rtl` holds the byte-mode flags on page FLAG --- so
/// this is the mask that leaves the register the engines can be held to.
pub const LC_COUNTER: u32 = 0o377777777;

/// Revision 13's location counter, `LC<29:0>`: a byte address, the word
/// `<29:2>`, the halfword `<1>` and the byte `<0>` (contract G2 §2.1,
/// appendix A1.6). [`Geometry::lc_counter`].
pub const LC_COUNTER_13: u32 = (1 << 30) - 1;

/// Revision 14's location counter, `LC<33:0>`: a byte address, the word
/// `<33:2>`, any 32-bit word address (appendix A14.11).
pub const LC_COUNTER_14: u64 = (1 << 34) - 1;

#[derive(Clone)]
pub struct Machine {
    pub prom: Vec<Insn>,
    pub imem: Vec<Insn>,
    /// The diagnostic bus's mode register, OLORD1 1A04.  `PROMDISABLE` is the
    /// bit that ends the boot.
    pub mode: spy::Mode,
    /// The clock control register, OLORD1 1A14 and 1A09: `RUN`, `STEP`,
    /// `NOP11`, `IDEBUG` and `LDSTAT`, which is how a console halts, steps
    /// and drives the machine.  See [`spy::ClockControl`].
    pub clock_control: spy::ClockControl,
    /// The OPC control register, OLORD1 1A08.  See [`spy::OpcControl`].
    pub opc_control: spy::OpcControl,
    /// The debug IR, the six 74S374s on page DEBUG: 48 bits a console loads
    /// in three halves, which `IDEBUG` puts on the I bus in place of the
    /// control store.
    pub debug_ir: u64,
    /// `-PROG.RESET` has been pulsed by a mode-register write with
    /// [`spy::MODE_RESET`] set, and the engine has not yet taken it: the
    /// pulse is asynchronous on the board, and the engines act on it at the
    /// master clock edge that ends the cycle it falls in.  `rtl` raises it
    /// from the write pulse's leading edge; `micro`, whose writes land at
    /// once, through [`Machine::spy_write`].
    pub prog_reset: bool,
    /// `PROG.BOOT` likewise, [`spy::MODE_BOOT`].
    pub prog_boot: bool,

    // The sizes of these five and of the two map levels are MIT's own
    // `SIZE-OF-HARDWARE-*` constants in System 100's `sys/cold/qcom.lisp`,
    // in octal: A memory 2000, M memory 40 on the CADR, dispatch memory 4000,
    // PDL buffer 2000, micro stack 40, level-1 map 4000, level-2 map 2000.
    // `chip_and_rtl_hold_the_same_memories` in `tests/chip.rs` holds them to
    // the netlist's RAMs.
    pub amem: [Word; 1024],
    pub mmem: [Word; 32],
    /// The dispatch memory: 2,048 entries, and room for revision 13's
    /// 4,096 ([`Geometry::dmem_words`]). On the heap, as the maps are.
    pub dmem: Box<[u32; DMEM_WORDS]>,
    /// The PDL buffer: 1,024 words on the CADR, and room for the largest
    /// QUUX's, [`PDL_WORDS`]; [`Geometry::pdl_bits`] says how much of it the
    /// machine has. On the heap, as main memory is: 16K words would make
    /// the machine too big for a thread's stack.
    pub pdl: Vec<Word>,
    pub spc: [u32; 32],

    /// 5 bits.
    pub spcptr: u8,
    /// 10 bits.
    pub pdl_pointer: u16,
    /// 10 bits.
    pub pdl_index: u16,
    pub q: Word,
    /// The PC of the instruction that just executed.
    pub opc: u16,
    /// Location counter.  The 26 bits of address [`LC_COUNTER`] leaves, plus
    /// NEED-FETCH in bit 31 and the interrupt-control flags mirrored in bits
    /// 29:26. On revision 13 the 30 bits of [`LC_COUNTER_13`] and
    /// NEED-FETCH in bit 31, the flags not mirrored: the location counter
    /// source reads them from [`Machine::interrupt_control`] (A1.6). On
    /// revision 14 the 34 bits of [`LC_COUNTER_14`] and NEED-FETCH in bit
    /// 40 ([`Geometry::need_fetch`]; A14.11).
    pub lc: u64,
    pub vma: Word,
    pub md: Word,
    /// The four flags of INTERRUPT-CONTROL at `<29:26>`, as the CADR's
    /// output bus carries them: LC byte mode, `PROG.UNIBUS.RESET`,
    /// `INT.ENABLE` and `SEQUENCE.BREAK`. Revision 13 takes them from
    /// `<37:34>` and reads them there (A1.6), and keeps them here at
    /// `<29:26>` all the same.
    pub interrupt_control: u32,
    /// `IR<41:32>` of the last DISPATCH, readable as functional source 0.
    pub dispatch_constant: u16,
    /// Revision 13's fixnum overflow flag (contract G2 §2.2, A1.3): loaded
    /// by every ALU-class microinstruction that executes, 1 when its
    /// function is arithmetic (`IR<8:3>` 20-37) and the 33-bit result over
    /// the sign-extended `M<31:0>` and `A<31:0>` has bit 32 unlike bit 31,
    /// 0 otherwise; JUMP, DISPATCH and BYTE words and an inhibited word
    /// leave it. Jump condition 10 tests it. Never set on a 32-bit machine.
    pub overflow: bool,
    /// Revision 14's TLB ([`crate::tlb`]): its contents, which both engines
    /// look up and fill, and its counts. Not in a checkpoint: a resume
    /// starts with it swept (A14.14).
    pub tlb: crate::tlb::Tlb,
    /// Revision 14's memory-system words of the register page, 220-224
    /// (A14.9).
    pub memory_words: crate::tlb::Words,
    /// Revision 14's PDL buffer redirect's copies (A14.7): the base, 32
    /// bits, and the head, 14 bits, taken in A memory's write pulse at
    /// [`crate::tlb::A_PDL_BUFFER_VIRTUAL_ADDRESS`] and
    /// [`crate::tlb::A_PDL_BUFFER_HEAD`] ([`Machine::a_written_14`]).
    pub pdl_copies: crate::tlb::PdlCopies,

    /// The widths of the map and the PDL buffer, [`Geometry::CADR`] unless
    /// the run chose another machine.
    pub geometry: Geometry,
    /// 2048 five-bit entries, addressed by `VMA<23:13>`; on QUUX 8,192
    /// seven-bit ones, addressed by `VA<27:15>` ([`Machine::translate`]).
    pub l1_map: Box<[u32; L1_MAP_WORDS]>,
    /// 24-bit entries, addressed by the level-1 output and `VMA<12:8>`:
    /// 1024 on the CADR; on QUUX 4,096 28-bit ones, addressed by the
    /// level-1 output and `VA<14:10>` ([`Machine::translate`]).
    pub l2_map: Box<[u32; L2_MAP_WORDS]>,
    pub main: Vec<Word>,
    /// What the last bus cycles left in the error register; see
    /// [`bus_error`]. A write of the error status register clears it,
    /// in `Machine::interface_write`.
    pub bus_error: u16,
    /// The bus interface's interrupt status register, `766040` and
    /// `766042`: [`busint::interrupt_status`] has the bits.
    pub interrupt_status: u16,
    /// `WRITE THROUGH ENB`, bit 7 of the error status register.
    pub write_through: bool,
    /// The sixteen Unibus map registers at `766140`-`766176`: 29701s at
    /// UBMAP 0E12-0E15, read back through the 74LS244s at 0E16 and 0E17.
    /// "Bit 15 of the register signifies that mapping of that page is
    /// turned on ... Bit 14 enables write access from the unibus. The
    /// remainder of the register is the page number" --- `unaddr.text`.
    /// The one other master of the Unibus, the debug cable's
    /// ([`busint::DebugRequest`]), goes through it: [`Machine::mapped_read`]
    /// and [`Machine::mapped_write`].  The processor's own Unibus cycles do
    /// not; `busint::decode` still gives `140000`-`177777` to nothing for
    /// them.  Only the debug master's cycles are mapped.
    pub unibus_map: [u16; 16],
    /// The read buffers, the 29701s at RBUF 0D23-0D26: one per mapped
    /// Unibus page, holding `BUS<31:16>` of the last Xbus word read through
    /// that page, which the read of the page's odd Unibus word gives
    /// without another Xbus cycle.  "The bus interface actually stores half
    /// of the xbus word and usually accesses the main memory only once for
    /// each pair of unibus operations" --- `unaddr.text`.
    pub read_buffer: [u16; 16],
    /// The write buffers, the 29701s on page WBUF: the low half written to
    /// the even Unibus word, waiting for the odd one to make the Xbus word.
    pub write_buffer: [u16; 16],
    /// Whether the last mapped access was permitted.
    pub vmaok: bool,
    /// The disk controller, at `0o17377774`-`0o17377777` on the CADR's Xbus.
    /// The board is always there; whether a drive is plugged into it is
    /// [`Controller::attach`].
    pub disk: Controller,
    /// The Chaosnet: this machine's address, and the link its cable
    /// reaches the rest of the network over.
    pub chaos: crate::chaos::Config,
    /// The display board, whichever of the two `--tv-board` named:
    /// [`crate::tv::Board`].
    pub tv: Tv,
    /// **The color TV**, the second display board, when `--color-tv`
    /// fitted one: a LISPM TV strapped to [`tv::COLOR_TV`], `17200000` and
    /// `17377750`.  `None` is a machine with one screen, which is what a
    /// CADR has unless somebody plugged a second board in, and is what
    /// `COLOR-EXISTS-P` finds when it probes.
    pub color_tv: Option<Tv>,
    /// The keyboard, the mouse and the clocks.
    pub ioboard: IoBoard,
    /// QUUX's keyboard and mouse on the register page (contract Q3,
    /// [`crate::quux_input`]).
    pub quux_input: crate::quux_input::QuuxInput,

    pub cycles: u64,
    /// The steps of LC, the macroinstruction boundaries: a meter both
    /// engines count where LC steps, for the time-neutral harness's digests
    /// (MP2b ruling Q12). Not in a checkpoint.
    pub lc_steps: u64,
    /// Simulated nanoseconds, kept by the engine that owns this machine:
    /// `rtl` and `chip`'s cable set it to the instant a bus cycle is
    /// acknowledged, off their own clocks, and `micro`, which has no clock,
    /// to its microcycles at the nominal 145 ns. The I/O board's microsecond
    /// and 60-cycle clocks count it. Microcycles times a constant, kept
    /// inside the board, is not time at any speed but one, and is not
    /// advanced at all by the far end of `chip`'s cables.
    ///
    /// **On QUUX revision 15 it counts units of 0.5 ns**, not nanoseconds,
    /// in both engines ([`Machine::time_base`], contract G3 revision 15's
    /// MP2b ruling Q1); every instant there is a multiple of the clock's
    /// period. The name is kept for the CADR's and revisions 13 and 14's
    /// sake, whose nanoseconds it is; every conversion of a real duration
    /// into it goes through [`Machine::time_base`].
    pub ns: u64,
    /// QUUX's interval timers, where the geometry has them.
    pub timers: Timers,
    /// QUUX's MACRO-DISPATCH register and MACRO DISPATCH MEMORY, where the
    /// geometry has them ([`Geometry::macro_dispatch`]).
    pub macro_dispatch: MacroDispatch,
    /// QUUX's real-time clock's setting, where the geometry has one
    /// ([`Geometry::rtc`]): live, or counted from `--rtc`'s start.
    pub rtc: Rtc,
    /// The disk controller has been written since the engine last looked,
    /// and may have written main memory: what invalidates QUUX's memory
    /// cache ([`crate::cache`]).
    pub dma_written: bool,
    /// QUUX's block-disk, when it is fitted in the CADR controller's place
    /// ([`crate::block_disk`]).
    pub block_disk: Option<crate::block_disk::BlockDisk>,
    /// QUUX's file device ([`crate::file_device`]), which answers where the
    /// geometry has one ([`Geometry::file_device`]).
    pub file_device: crate::file_device::FileDevice,
    /// When the processor's write buffer is next empty, on the machine's
    /// clock: `rtl` sets it before each write it lands, from QUUX's memory
    /// port; `micro` has no buffer and leaves it 0. The file device takes a
    /// new command producer index only from then (contract Q9: the device
    /// reads memory once the processor's writes are out of the buffer). Not
    /// kept in a checkpoint: it is set afresh before it is read.
    pub write_buffer_empty_at: u64,
    /// Revision 15's clock period, in units of 0.5 ns, as the engine
    /// running the machine keeps it: what feature word 25 says (A15b.1),
    /// and the grid its devices act on ([`Machine::time_base`]). The engine
    /// sets it; 0 on every other machine. Not kept in the machine's part of
    /// a checkpoint: the engine records it with its own.
    pub period: u64,
    /// Revision 15's register-page word 225 (A15b.1, A15b.5): posted
    /// writes answered with an error, a count, read, and cleared by a
    /// write. `micro` has no posted writes and answers none so; kept in a
    /// checkpoint of revision 15.
    pub posted_write_errors: u32,
    /// The physical address of every word of main memory a bus cycle
    /// stores, in order, when a test or a trace asks for the record by
    /// setting it to `Some`. A disk transfer's words are not in it: the
    /// block-disk keeps its own record ([`crate::block_disk::BlockDisk::log`]).
    /// Not kept in a checkpoint.
    pub store_log: Option<Vec<u32>>,
    /// Every write of a device register on QUUX's register page or of the
    /// disk's registers, in order, when a test or a trace asks for the
    /// record by setting it to `Some`: the physical address, the word, and
    /// [`Machine::cycles`] then. Not kept in a checkpoint.
    pub register_log: Option<Vec<(u32, u32, u64)>>,
    /// The board name, feature words 20-24 on revision 13: up to
    /// [`BOARD_NAME_CHARS`] characters, zero bytes after them;
    /// [`BOARD_NAME`] unless [`Machine::set_board_name`] sets another. Not
    /// kept in a checkpoint.
    board_name: [u8; BOARD_NAME_CHARS],
}

impl Machine {
    /// A machine with [`MAIN_WORDS`] of main memory: thirty-two boards.
    pub fn new() -> Self {
        Self::with_memory_boards(MAIN_WORDS >> 16)
    }

    /// How many 64K-word memory boards this machine has.
    pub fn memory_boards(&self) -> usize {
        self.main.len() >> 16
    }

    /// A machine with `boards` memory boards of 64K words each, which is
    /// what `--main-memory-boards` sets on every engine. From one to
    /// [`busint::MAX_MEMORY_BOARDS`], where the Xbus I/O space begins.
    pub fn with_memory_boards(boards: usize) -> Self {
        Self::with_geometry(Geometry::CADR, boards)
    }

    /// A machine of `geometry` with `boards` 64K-word boards of main
    /// memory, from one to [`Geometry::max_memory_boards`]: past the
    /// CADR's sixty on revision 13.
    pub fn with_geometry(geometry: Geometry, boards: usize) -> Self {
        assert!(
            (1..=geometry.max_memory_boards()).contains(&boards),
            "{boards} memory boards: the machine holds 1 to {}",
            geometry.max_memory_boards()
        );
        Machine {
            prom: vec![Insn::new(0); PROM_WORDS],
            imem: vec![Insn::new(0); IMEM_WORDS],
            mode: spy::Mode::default(),
            clock_control: spy::ClockControl::default(),
            opc_control: spy::OpcControl::default(),
            debug_ir: 0,
            prog_reset: false,
            prog_boot: false,
            amem: [0; 1024],
            mmem: [0; 32],
            dmem: Box::new([0; DMEM_WORDS]),
            pdl: vec![0; PDL_WORDS],
            spc: [0; 32],
            spcptr: 0,
            pdl_pointer: 0,
            pdl_index: 0,
            q: 0,
            opc: 0,
            lc: 0,
            vma: 0,
            md: 0,
            interrupt_control: 0,
            dispatch_constant: 0,
            overflow: false,
            tlb: crate::tlb::Tlb::default(),
            memory_words: crate::tlb::Words::default(),
            pdl_copies: crate::tlb::PdlCopies::default(),
            geometry,
            timers: Timers::new(),
            macro_dispatch: MacroDispatch::default(),
            rtc: Rtc::Host,
            dma_written: false,
            block_disk: None,
            file_device: crate::file_device::FileDevice::new(),
            write_buffer_empty_at: 0,
            period: 0,
            posted_write_errors: 0,
            store_log: None,
            register_log: None,
            board_name: {
                let mut n = [0; BOARD_NAME_CHARS];
                n[..BOARD_NAME.len()].copy_from_slice(BOARD_NAME.as_bytes());
                n
            },
            l1_map: Box::new([0; L1_MAP_WORDS]),
            l2_map: Box::new([0; L2_MAP_WORDS]),
            main: vec![0; boards << 16],
            bus_error: 0,
            interrupt_status: busint::interrupt_status::LOCAL_ENABLE,
            write_through: false,
            unibus_map: [0; 16],
            read_buffer: [0; 16],
            write_buffer: [0; 16],
            vmaok: true,
            disk: Controller::default(),
            chaos: crate::chaos::Config::default(),
            // The I/O board has its Chaosnet interface whether or not a
            // cable is plugged in: the switches at the configured address,
            // nothing on the cable until [`Machine::plug_chaos`].
            quux_input: crate::quux_input::QuuxInput::new(),
            ioboard: {
                let mut b = ioboard::IoBoard::default();
                b.plug_chaos(crate::chaos::Config::default().address, None, 0, false);
                b
            },
            tv: Tv::default(),
            color_tv: None,
            cycles: 0,
            lc_steps: 0,
            ns: 0,
        }
    }

    /// Puts the color TV on the backplane, which is what `--color-tv`
    /// does where an engine builds its machine.  The board is the
    /// backplane's and does not come and go under a running machine.
    pub fn fit_color_tv(&mut self) {
        self.color_tv = Some(Tv::color());
    }

    /// Sets the board name, feature words 20-24 on revision 13 (contract HD
    /// §6): what a fabric under test was built with, so that its register
    /// page and muir-sim's read the same. At most [`BOARD_NAME_CHARS`] characters, each printable
    /// ASCII, `040`-`176`, the codes the Lisp Machine's character set shares
    /// with ASCII; anything else is refused and the name is left as it was.
    /// Not a flag, and not kept in a checkpoint: a name says what runs.
    pub fn set_board_name(&mut self, name: &str) -> Result<(), String> {
        if name.len() > BOARD_NAME_CHARS {
            return Err(format!(
                "the board name {name:?} is {} characters, over {BOARD_NAME_CHARS}",
                name.len()
            ));
        }
        if let Some(b) = name.bytes().find(|b| !(0o40..=0o176).contains(b)) {
            return Err(format!("the board name {name:?} has the byte {b:o}, outside 040-176"));
        }
        self.board_name = [0; BOARD_NAME_CHARS];
        self.board_name[..name.len()].copy_from_slice(name.as_bytes());
        Ok(())
    }

    /// Feature word 20 + `k` of the board name, `k` from 0 to 4: characters
    /// 4k to 4k + 3, the first in `<7:0>`; zero bytes after the name's end.
    fn board_name_word(&self, k: u32) -> u32 {
        let k = k as usize * 4;
        u32::from_le_bytes(self.board_name[k..k + 4].try_into().unwrap())
    }

    /// **The unit [`Machine::ns`] counts and the grid devices act on**:
    /// nanoseconds with no grid, and on revision 15 units of 0.5 ns on its
    /// clock's period ([`crate::clock::TimeBase`]).
    pub fn time_base(&self) -> crate::clock::TimeBase {
        if self.geometry.extended() {
            crate::clock::TimeBase::half_ns(self.period)
        } else {
            crate::clock::TimeBase::NS
        }
    }

    /// The microsecond clock, functional source 15, at the machine's time:
    /// the microseconds since power-on, 32 bits, wrapping
    /// ([`Timers::microseconds`] in nanoseconds; on revision 15 its units).
    pub fn microseconds(&self) -> u32 {
        (self.ns / self.time_base().per_us()) as u32
    }

    /// The machine's time in whole nanoseconds, as the Chaosnet interface
    /// and the displays keep it: [`Machine::ns`] itself but on revision 15,
    /// whose units it halves, the half dropped. **Unverified** for revision
    /// 15's network, whose times the contract has double into units and act
    /// on the clock's grid (MP2b ruling Q19): a started delay can end a
    /// clock early here when its start is at an odd unit.
    pub fn device_ns(&self) -> u64 {
        self.ns / self.time_base().per_ns
    }

    /// Loads the boot PROM.  Words past the end of the image stay zero.
    pub fn load_prom(&mut self, words: &[Insn]) {
        for (i, w) in words.iter().take(PROM_WORDS).enumerate() {
            self.prom[i] = *w;
        }
    }

    /// QUUX's interrupt status, the register page's word 100, in the order
    /// of contract Q13: `<0>`, `<1>` and `<2>` timers 0 to 2, each its flag
    /// under its interrupt enable (contract Q11); `<3>` block-disk's done
    /// under its enable, command `<11>`; `<4>` the keyboard and `<5>` the
    /// mouse ([`crate::quux_input`]); `<6>` the network, the Chaosnet
    /// interface's request (contract Q4); `<7>` the file device, a response
    /// waiting under its interrupt enable (contract Q9). Every bit is a
    /// level cleared at its source, so one write turns each off: a stray's
    /// handler in the microcode relies on it (contract Q13, section 2).
    pub fn interrupt_sources(&self) -> u32 {
        (if self.geometry.tick { self.timers.interrupt_sources(self.ns) } else { 0 })
            | (self.block_disk.as_ref().is_some_and(|d| d.interrupt_at(self.ns)) as u32) << 3
            | if self.geometry.machine_id.is_some() { self.quux_input.interrupts() } else { 0 }
            | (self.ioboard.chaos.as_ref().is_some_and(|c| c.interrupt_request().is_some()) as u32)
                << 6
            | ((self.geometry.file_device && self.file_device.interrupt_at(self.ns)) as u32)
                << crate::file_device::INTERRUPT_BIT
    }

    /// Runs the file device's commands due by the machine's clock: what
    /// each engine calls at the edge between two microcycles, so that a
    /// command completes before the processor's next cycle begins. A command
    /// that wrote main memory invalidates QUUX's cache before that cycle, as
    /// a disk transfer does.
    pub fn advance_file_device(&mut self) {
        let time = self.time_base();
        if self.geometry.file_device && self.file_device.advance(self.ns, &mut self.main, time) {
            self.dma_written = true;
        }
    }

    /// Why a checkpoint of this machine cannot be written now, if it
    /// cannot: the file device with a handle open or a command queued. A
    /// 40-bit machine's file says its width by its version
    /// ([`crate::checkpoint::VERSION_40`]).
    pub fn checkpoint_refusal(&self) -> Option<String> {
        self.geometry.file_device.then(|| self.file_device.checkpoint_refusal()).flatten()
    }

    /// The debug IR as the word it runs: its 64 bits on revision 15, 48
    /// elsewhere.
    pub fn debug_insn(&self) -> Insn {
        if self.geometry.extended() {
            Insn::extended(self.debug_ir)
        } else {
            Insn::new(self.debug_ir)
        }
    }

    /// Fetches from the control store, honoring the PROM overlay on the
    /// CADR and the PROM's own addresses on QUUX.
    pub fn fetch(&self, pc: u16) -> Insn {
        let pc = pc as usize & (IMEM_WORDS - 1);
        match self.geometry.prom_base {
            Some(base) if pc >= base as usize => self.prom[pc - base as usize],
            Some(_) => self.imem[pc],
            None if self.mode.prom_disable || pc >= PROM_WORDS => self.imem[pc],
            None => self.prom[pc],
        }
    }

    /// Whether the program at `pc` is the boot PROM's rather than the
    /// microcode's, as a run's stops and its last word count it: on the
    /// CADR while `PROMDISABLE` is clear, the PROM lying over control store
    /// 0 up until it is set; on QUUX at the PROM's own addresses,
    /// [`QUUX_PROM_BASE`] up, which have no disable (contract Q2).
    pub fn in_prom(&self, pc: u16) -> bool {
        match self.geometry.prom_base {
            Some(base) => pc >= base,
            None => !self.mode.prom_disable,
        }
    }

    /// Where the boot starts: 0 on the CADR, the PROM's base on QUUX.
    pub fn reset_pc(&self) -> u16 {
        self.geometry.prom_base.unwrap_or(0)
    }

    /// A control-store write, `WRITE-I-MEM`: the RAM at `pc`, except where
    /// QUUX's PROM is, which nothing writes.
    ///
    /// Every control-store write clears QUUX's MACRO-DISPATCH enable,
    /// wherever it lands ([`macro_dispatch`]): the entries name control-store
    /// addresses, and a new microcode must not run on the old one's,
    /// whether the PROM loaded it or a `%DISK-RESTORE`, which does not pass
    /// through the PROM (contract H8a §3.6).
    pub fn write_imem(&mut self, pc: u16, w: Insn) {
        self.macro_dispatch.disable();
        let pc = pc as usize & (IMEM_WORDS - 1);
        if self.geometry.prom_base.is_some_and(|base| pc >= base as usize) {
            return;
        }
        self.imem[pc] = w;
    }

    /// LC byte mode, `interrupt_control<29>`: the 25LS2519 at FLAG 3E08
    /// takes `OB29` to `LC BYTE MODE` under `-DESTINTCTL`, beside `OB28` to
    /// `PROG.UNIBUS.RESET`, `OB27` to `INT.ENABLE` and `OB26` to
    /// `SEQUENCE.BREAK`.
    pub fn byte_mode(&self) -> bool {
        self.interrupt_control & (1 << 29) != 0
    }

    pub fn push_spc(&mut self, pc: u32) {
        self.spcptr = (self.spcptr + 1) & 0o37;
        self.spc[self.spcptr as usize] = pc;
    }

    pub fn pop_spc(&mut self) -> u32 {
        let v = self.spc[self.spcptr as usize];
        self.spcptr = self.spcptr.wrapping_sub(1) & 0o37;
        v
    }

    /// Translates a 24-bit virtual address through the two map levels.
    ///
    /// The geometry is the netlist's, read off the VMEM pages; `tests/chip.rs`
    /// drives the same RAMs from that wiring and compares them word for word:
    ///
    /// | level | pages | words | width | addressed by |
    /// |---|---|---|---|---|
    /// | 1 | VMEM0 | 2048 | 5 | `VMA<23:13>` |
    /// | 2 | VMEM1, VMEM2 | 1024 | 24 | `{VMAP<4:0>, VMA<12:8>}` |
    ///
    /// So a level-1 entry picks one block of 32 level-2 entries and
    /// `VMA<12:8>` picks the entry within it.  `VMA<7:0>` never reaches the
    /// map at all; it is the offset within the page.
    ///
    /// Both levels are wired active low --- VMEM0 reads back as `-VMAP`, level
    /// 2 as `-VMO`.  That matters to `src/chip.rs`, which holds cells; this
    /// holds values, so it does not appear here.
    pub fn translate(&self, vaddr: u32) -> Translation {
        if self.geometry.paged() {
            return self.translate_14(vaddr);
        }
        if self.geometry.wide() {
            return self.translate_13(vaddr);
        }
        // Only VMA<23:0> reaches MAPI; `ir.bits` writes level 2 from the same
        // 24 bits.
        let vaddr = vaddr & 0x00ff_ffff;
        let l1_data = self.l1_map[(vaddr >> 13) as usize & 0o3777] & self.geometry.l1_mask();
        let l2_data = self.l2_map[self.geometry.l2_index(l1_data, vaddr)];
        // `VMO<13:0>` is the physical page: 14 bits, which is what the boot
        // PROM's own `SET-UP-FOUR-PAGES` needs to name page 0o37766.
        let page = l2_data & 0x3fff;
        Translation {
            physical: (page << 8) | (vaddr & 0xff),
            page,
            l1_data,
            l2_data,
            // VMEMDR 1D14 latches `-VMO23` and `-VMO22`; VCTL2 1D26 makes
            // `-PFR` from the first, VCTL1 1D17 makes `-PFW` from the second
            // with `WRCYC`.
            write_permitted: l2_data & (1 << 22) != 0,
            access_permitted: l2_data & (1 << 23) != 0,
        }
    }

    /// **Revision 13's map** (contract G2 §2.6, appendix A1.7): two levels,
    /// as today, over a 28-bit virtual address and 1024-word pages.
    ///
    /// | level | entries | width | addressed by |
    /// |---|---|---|---|
    /// | 1 | 8,192 | 7 | `VA<27:15>` |
    /// | 2 | 4,096 | 28 | `{L1<6:0>, VA<14:10>}` |
    ///
    /// The physical word address is `{L2<17:0>, VA<9:0>}`, 28 bits. The
    /// level-2 entry is PHT word 2's: `<27>` read access, `<26>` write
    /// access (the access code `<27:26>`), `<23:22>` the meta bits a
    /// DISPATCH takes, `<17:0>` the physical page. An address with
    /// `<31:28>` not zero reads block `177`, the invalid block, whatever
    /// level 1 holds there, so that the map-miss path runs and nothing
    /// aliases the low 256 M words; `<39:32>` never reaches the map. The
    /// appendix's own words for the gate: "the level-1 output reads `177`,
    /// whatever the RAM holds".
    fn translate_13(&self, vaddr: u32) -> Translation {
        let l1_data = self.map_level_1_13(vaddr);
        let l2_data = self.l2_map[map_level_2_index_13(l1_data, vaddr)];
        let page = l2_data & 0o777777;
        Translation {
            physical: (page << 10) | (vaddr & 0o1777),
            page,
            l1_data,
            l2_data,
            write_permitted: l2_data & (1 << 26) != 0,
            access_permitted: l2_data & (1 << 27) != 0,
        }
    }

    /// **Revision 14's translation** (A14.1, A14.5, A14.6) of `vaddr`: a
    /// window's fixed entry from the decode; a paged address's TLB entry,
    /// or on a miss what the walk would find, the no-entry word if
    /// nothing. Nothing is loaded and no walk is counted here: the engines
    /// call [`Machine::tlb_fill`] where the hardware walks, before they
    /// look, so that a miss is filled at the moment the port would fill it.
    /// `physical` is the 29-bit bus address ([`crate::tlb::bus_address`]),
    /// `l2_data` the entry's `<29:0>`, and the permissions its access code,
    /// `<27>` read and `<26>` write.
    fn translate_14(&self, vaddr: u32) -> Translation {
        let entry = self.entry_14(vaddr);
        Translation {
            physical: crate::tlb::bus_address(vaddr, entry),
            page: entry & 0o777777,
            l1_data: 0,
            l2_data: entry,
            write_permitted: entry & (1 << 26) != 0,
            access_permitted: entry & (1 << 27) != 0,
        }
    }

    /// The entry `<29:0>` revision 14 gives `vaddr`, as
    /// [`Machine::translate_14`] says.
    pub fn entry_14(&self, vaddr: u32) -> u32 {
        if let Some(e) = crate::tlb::fixed_entry(vaddr) {
            return e;
        }
        self.tlb.lookup(vaddr).unwrap_or_else(|| {
            crate::tlb::walk(&self.main, self.memory_words.directory, vaddr)
                .entry
                .unwrap_or(crate::tlb::NO_ENTRY)
        })
    }

    /// **The walk on a miss** (A14.6): when `vaddr` is paged and the TLB
    /// does not hold it, the walk is made, counted, and what it found
    /// loaded through `port`, unless it found no entry. Returns the walk,
    /// for `rtl` to time its reads; `None` when nothing walked.
    pub fn tlb_fill(&mut self, vaddr: u32, port: crate::tlb::Port) -> Option<crate::tlb::Walk> {
        if crate::tlb::region(vaddr) != crate::tlb::Region::Paged
            || self.tlb.lookup(vaddr).is_some()
        {
            return None;
        }
        let walk = crate::tlb::walk(&self.main, self.memory_words.directory, vaddr);
        self.tlb.walks += 1;
        if let Some(e) = walk.entry {
            self.tlb.fill(vaddr, e, port);
        }
        Some(walk)
    }

    /// **A reference's write-back** (A14.6, A14.8), as its cycle goes out
    /// and before it: a reference to the paged address `va` that did not
    /// fault, through the TLB entry `entry` it latched, a write when
    /// `write` with `md` the word written. The bits it sets are ORed into
    /// the TLB entry at once and into the table by the guarded
    /// read-modify-write, a refusal counted in word 224. A window's address
    /// writes nothing back. `None` when it asked for no bit.
    pub fn write_back(
        &mut self,
        va: u32,
        entry: u32,
        write: bool,
        md: Word,
    ) -> Option<crate::tlb::WriteBack> {
        if crate::tlb::region(va) != crate::tlb::Region::Paged {
            return None;
        }
        let words = self.memory_words;
        let ephemeral = words.ephemeral
            && words.pointer_type(md)
            && (md as u32) >> 28 == crate::tlb::EPHEMERAL_SPACE;
        let bits = crate::tlb::write_back_bits(entry, write, ephemeral);
        if bits == 0 {
            return None;
        }
        self.tlb.or(va, bits);
        let wb = crate::tlb::write_back(&mut self.main, words.directory, va, entry, bits);
        self.tlb.write_backs += 1;
        for (k, bit) in [crate::tlb::ACCESSED, crate::tlb::MODIFIED, crate::tlb::EPHEMERAL]
            .into_iter()
            .enumerate()
        {
            self.tlb.written_bits[k] += u64::from(bits & bit != 0);
        }
        if wb.write.is_none() {
            self.tlb.refusals += 1;
            self.memory_words.refused = self.memory_words.refused.wrapping_add(1);
        }
        Some(wb)
    }

    /// **Revision 14's snoop of A memory's write pulse** (A14.7): a write of
    /// A location `adr` with `word` takes the redirect's base at 430 and its
    /// head at 431. Nothing on another revision.
    pub fn a_written_14(&mut self, adr: usize, word: Word) {
        if self.geometry.paged() {
            self.pdl_copies.a_written(adr, word);
        }
    }

    /// **The PDL buffer redirect's decision** (A14.7) for a start to `va`
    /// through the TLB entry `entry`, a write when `write`: the entry the
    /// reference proceeds with, and the redirect if it fires. It fires on a
    /// paged address whose entry has status 5 and an access code that
    /// faults the reference; it then proceeds as if the access code were
    /// `11`, inside the buffer at the index it gives or outside it through
    /// memory. The test is against the PDL buffer pointer as it stands.
    pub fn redirect_14(
        &self,
        va: u32,
        entry: u32,
        write: bool,
    ) -> (u32, Option<crate::tlb::Redirect>) {
        let permitted = entry & 1 << 27 != 0 && (!write || entry & 1 << 26 != 0);
        if crate::tlb::region(va) != crate::tlb::Region::Paged
            || crate::tlb::status(u64::from(entry)) != 5
            || permitted
        {
            return (entry, None);
        }
        let r = self.pdl_copies.test(va, self.pdl_pointer);
        (entry | 3 << 26, Some(r))
    }

    /// **A `WRITE-MAP` operation** landing (A14.4), at `now`: a direct
    /// write, an invalidation, or an empty, which sweeps the TLB and, on
    /// `rtl`, holds starts and port-B lookups until `now` + N ticks.
    /// Whether it was an operation, which drops the prefetch's word.
    pub fn write_map_14(&mut self, vma: Word, md: Word, now: u64) -> bool {
        match crate::tlb::Operation::of(vma, md) {
            crate::tlb::Operation::None => return false,
            crate::tlb::Operation::Write { va, entry } => self.tlb.load(va, entry),
            crate::tlb::Operation::Invalidate { va } => self.tlb.invalidate(va),
            crate::tlb::Operation::Empty => self.sweep_tlb(now),
        }
        true
    }

    /// The sweep, at `now` (A14.4): every entry cleared, and on `rtl` the
    /// sweep's end, N ticks on, which starts wait for.
    pub fn sweep_tlb(&mut self, now: u64) {
        self.tlb.sweep();
        self.tlb.sweep_until = now + self.tlb.sweep_ns();
    }

    /// `-RESET` on revision 14 (A14.4, A14.9), at `now`: the TLB swept and
    /// the memory system's words cleared. Nothing on another revision.
    pub fn reset_memory_system(&mut self, now: u64) {
        if self.geometry.paged() {
            self.sweep_tlb(now);
            self.memory_words = crate::tlb::Words::default();
            // Revision 15's word 225 with them, as word 224 (A15b.1).
            self.posted_write_errors = 0;
        }
    }

    /// A TLB of `entries`, `--tlb` (A14.4): a power of two from 1,024 to
    /// 32,768, swept; before the machine runs.
    pub fn set_tlb_entries(&mut self, entries: usize) {
        self.tlb = crate::tlb::Tlb::new(entries);
    }

    /// Revision 13's level-1 output for the address `addr` (A1.7): the
    /// entry at `VA<27:15>`, or block `177` when `<31:28>` is not zero.
    pub fn map_level_1_13(&self, addr: u32) -> u32 {
        if addr >> 28 != 0 {
            MAP_INVALID_BLOCK_13
        } else {
            self.l1_map[(addr >> 15) as usize & (L1_MAP_WORDS - 1)] & 0o177
        }
    }

    /// Revision 13's map write (A1.7), from the word in `VMA` at the
    /// address `addr`, which is `MD` (or `VMA` in the microcycle after a
    /// memory start, as `MAPI` is on every machine): `VMA<29>` writes level
    /// 1 at `addr<27:15>` with `VMA<38:32>`; `VMA<28>` writes level 2 at
    /// level 1's output for `addr` and `addr<14:10>` with `VMA<27:0>`. With
    /// both, level 1 only: the CADR's write of level 2 at block 0 is its
    /// RAMs' artefact, and on QUUX block 0 is wired. An address with
    /// `<31:28>` not zero writes neither, or it would land in the invalid
    /// block or map an alias.
    pub fn write_map_13(&mut self, vma: Word, addr: u32) {
        if addr >> 28 != 0 {
            return;
        }
        if vma & (1 << 29) != 0 {
            self.l1_map[(addr >> 15) as usize & (L1_MAP_WORDS - 1)] = (vma >> 32) as u32 & 0o177;
        } else if vma & (1 << 28) != 0 {
            let l1 = self.map_level_1_13(addr);
            self.l2_map[map_level_2_index_13(l1, addr)] = vma as u32 & MAP_LEVEL_2_13;
        }
    }

    /// Writes the map, as the WRITE-MAP destinations do.  `VMA<26>` enables the
    /// level-1 write from `VMA<31:27>`; `VMA<25>` enables the level-2 write
    /// from `VMA<23:0>`.  Both are indexed by MD.  Those four field positions
    /// are `mit/cadr/ir.bits` in MIT's own words.
    ///
    /// The hardware performs the write on the cycle *after* the store, and
    /// `ir.bits` warns that VMA must not be disturbed meanwhile.  That delay
    /// is the caller's: `micro` holds the write in `WMAPD` and calls this at
    /// the start of the next microcycle, `rtl` writes the two levels itself
    /// in its write phase, and `chip` has the registers.
    ///
    /// **A store with both `VMA<26>` and `VMA<25>` up writes level 2 with
    /// the level-1 bits of its address zero.**  The two write pulses are one,
    /// `-WP1` through the 74S37 at VCTL2 1D07 (`-VM0WPA/B` and `-VM1WPA/B`).
    /// Level 1 is 93425As, and "During writing, the output is held in the
    /// high impedance state" (Fairchild, 1977 Bipolar Memory Data Book,
    /// 93425/93425A, page 7-120, within 20 ns of `WE` falling); `-VMAP<4:0>`
    /// has no other driver and no pull-up, so the TTL inputs of the 74S240s
    /// at VMEM1 1D08 and VMEM2 1C10 read it high and their outputs, level
    /// 2's top five address bits, go low.  The new level-1 entry never
    /// reaches that address during the 40 ns pulse, and the old one is there
    /// for less than the part's guaranteed write, 20 ns.  `chip`, which has
    /// the RAMs and the buffers, gives the same, and
    /// `chip_rtl_and_micro_write_both_map_levels_alike` holds the three.
    /// **Unverified:** whether the old entry takes a partial write in its
    /// first nanoseconds; a CADR running the two-instruction store would
    /// settle it.  Microcode 323 writes the levels in separate stores
    /// (`LEVEL-1-MAP-MISS` in `uc-page-fault.lisp`), so the band never asks.
    pub fn write_map(&mut self, vma: Word, md: Word) {
        if self.geometry.paged() {
            self.write_map_14(vma, md, self.ns);
            return;
        }
        if self.geometry.wide() {
            return self.write_map_13(vma, md as u32);
        }
        let (vma, md) = (vma as u32, md as u32);
        let l1_index = (md >> 13) as usize & 0o3777;
        if vma & (1 << 26) != 0 {
            self.l1_map[l1_index] = self.geometry.l1_from_vma(vma);
        }
        if vma & (1 << 25) != 0 {
            let l1_data = if vma & (1 << 26) != 0 { 0 } else { self.l1_map[l1_index] };
            self.l2_map[self.geometry.l2_index(l1_data, md)] = vma & 0o77777777;
        }
    }

    /// Reads virtual memory, setting [`Machine::vmaok`].  A denied access is
    /// not an error: the microcode tests for it with the page-fault jump
    /// conditions, and `-VMAOK` is one of the FLAG bits `ir.bits` lists.
    ///
    /// `VMAOK` is `(-PFR) AND (-PFW)`, so a read needs access permission and
    /// a write needs both.
    pub fn vm_read(&mut self, vaddr: u32) -> Word {
        let t = self.translate(vaddr);
        self.vmaok = t.access_permitted;
        if !self.vmaok {
            return 0;
        }
        self.bus_read(t.physical)
    }

    pub fn vm_write(&mut self, vaddr: u32, value: Word) {
        let t = self.translate(vaddr);
        self.vmaok = t.access_permitted && t.write_permitted;
        if self.vmaok {
            self.bus_write(t.physical, value);
        }
    }

    /// One bus cycle, by physical address.
    ///
    /// **An address nothing answers is not an error.** The board times the
    /// cycle out, sets an NXM bit and carries on, and the microcode relies on
    /// that: `PAGE-0-PARITY-FIX` at `00310` deliberately runs one word past
    /// the end of its loop --- "This does one extra location, too bad" ---
    /// into the top of the Xbus I/O region, where nothing lives. Faulting
    /// there stops the machine before it ever reaches the disk.
    ///
    /// The regions are MIT's own, from the bus interface specification:
    /// Xbus memory up to page `0o35777`, Xbus I/O `0o36000`-`0o36777`, and
    /// the Unibus `0o37000`-`0o37777`. What answers is [`Machine::bus_read`]'s
    /// list: the disk controller and the display on the Xbus, the bus
    /// interface's own registers and the I/O board on the Unibus. Every
    /// other I/O address times out.
    ///
    /// The CADR's alone: QUUX decodes by [`busint::decode_quux_13`].
    fn device(&mut self, phys: u32) -> Option<usize> {
        match busint::decode_for(
            phys,
            self.main.len(),
            self.color_tv.is_some(),
            self.tv.buffer_words(),
            self.tv.control_registers(),
        ) {
            busint::Responder::Memory(_) => Some(phys as usize),
            busint::Responder::Device
            | busint::Responder::Interface
            | busint::Responder::Unibus(_) => None,
            // The debug block's word is the other machine's, over the cable;
            // `rtl` carries it.  Here, with no cable, a read is nothing and a
            // write goes nowhere, and neither is an error: the pull-up on
            // `DEBUG OUT ACK` answers.
            busint::Responder::Debug(_) => None,
            // The map's responders are the debug master's,
            // [`Machine::mapped_read`] and [`Machine::mapped_write`]; the
            // processor's `decode` never makes them.
            busint::Responder::MapBuffer
            | busint::Responder::MapXbus(_)
            | busint::Responder::MapRefused
            | busint::Responder::MapMd => None,
            busint::Responder::NoXbus => {
                self.bus_error |= bus_error::XBUS_NXM;
                None
            }
            busint::Responder::NoUnibus => {
                self.bus_error |= bus_error::UNIBUS_NXM;
                None
            }
        }
    }

    /// `INT` on the cables: the interrupt line the bus interface presents to
    /// the cpu, which the 74S175 at LCC 3E12 registers as `SINTR` and the
    /// page-fault-or-interrupt jump conditions take under `INT.ENABLE`.
    /// `LM INT` is `UB INT OR XBUS INTR IN` at UBINTC 0E04: the disk
    /// controller's request on the Xbus, or a Unibus interrupt taken or
    /// simulated. The I/O board's keyboard interrupt comes over the Unibus
    /// and is taken by [`Machine::unibus_interrupt`].
    ///
    /// On QUUX, the interval timers too, each while its flag is up under
    /// its interrupt enable, at [`Machine::ns`].
    pub fn interrupt(&self) -> bool {
        self.interrupt_at(self.ns)
    }

    /// The interrupt pending at `now`, which an engine whose own clock has
    /// moved on since the machine's gives: QUUX's clocks are read at `now`
    /// (a flag that rises while a microcycle waits for `MD` is up at the
    /// edge that ends it, `tests/tick.rs`); the CADR's devices keep the
    /// machine's time.
    pub fn interrupt_at(&self, now: u64) -> bool {
        self.xbus_interrupt()
            || (self.geometry.unibus && self.unibus_interrupt().is_some())
            || (self.geometry.tick && self.timers.pending(now.max(self.ns)))
            || (self.geometry.machine_id.is_some() && self.quux_input.interrupts() != 0)
            // The network's request, word 100's <5> (contract Q4), reaches
            // the processor as every bit of the register page's does, not
            // only through the Unibus interrupt that 766040 enables.
            || (self.geometry.machine_id.is_some()
                && self.ioboard.chaos.as_ref().is_some_and(|c| c.interrupt_request().is_some()))
            // The file device's, word 100's <6> (contract Q9), from its due
            // times.
            || (self.geometry.file_device && self.file_device.interrupt_at(now.max(self.ns)))
    }

    /// `XBUS INTR IN`: the disk controller's request, or either display's
    /// vertical interrupt, on the one Xbus line.
    ///
    /// **Both display boards drive the same wire.** On each of them the
    /// 74S08 at 0D10 ands `MODE INTR ENB` with `VERT FLAG` into `SEND
    /// INTR`, and the 26S10 at 0F14 --- an open-collector bus transceiver
    /// --- puts that on `-XBUS.INTR`; the nets are the same on both
    /// boards' netlists.  So the line is the boards ORed, and a second
    /// board fitted adds its own `SEND INTR` to it.  Nothing in
    /// System 100 turns the color board's on: `COLOR:SETUP` starts its
    /// sync with `(SI:START-SYNC 3 0 36.)`, and `CC-TV-START-SYNC` in
    /// `sys/cc/dmon.lisp` --- the same call, written out --- writes the
    /// mode as `(+ (LSH BOW 2) CLOCK)`, which is 3: the clock mode alone,
    /// with [`tv::mode::INTERRUPT_ENABLE`] clear.  That matters because
    /// `INTRX0` in microcode 323 reads `A-TV-REGS-BASE`, the normal TV's
    /// register, and clears the flag there; a color-board interrupt would
    /// have nothing to take it.
    pub fn xbus_interrupt(&self) -> bool {
        // The controller was told the time at the last bus access; a run's
        // engine keeps `ns` current between them (`rtl` every microcycle).
        self.disk.interrupt()
            || self.block_disk.as_ref().is_some_and(|d| d.interrupt_at(self.ns))
            || self.tv.interrupt(self.ns)
            || self.color_tv.as_ref().is_some_and(|tv| tv.interrupt(self.ns))
    }

    /// The Unibus interrupt the interface has taken, as `UB INT` and the
    /// vector, if it has one: what a read of `766040` shows in bits 15 and
    /// 2-9.
    ///
    /// One taken by writing the bit stays until the bit is written clear.
    /// One from a device is taken while
    /// [`busint::interrupt_status::ENABLE_UB_INTS`] is set --- the grant
    /// the interface gives a request on the bus --- and its vector is the
    /// device's. The model has no grant cycle to latch at, so the vector
    /// is read off the requesting board at the time of the read: the same
    /// value, since the request holds until the microcode has read the
    /// device's data, which is the one thing that clears `KBD READY`.
    /// Dismissal is the microcode's write of zero to `766042`,
    /// `UB-INTR-RET-0` in `uc-interrupt.lisp`, by which time the request
    /// is gone.
    pub fn unibus_interrupt(&self) -> Option<u16> {
        use busint::interrupt_status::{ENABLE_UB_INTS, UB_INT, VECTOR_MASK};
        if self.interrupt_status & UB_INT != 0 {
            return Some(UB_INT | (self.interrupt_status & VECTOR_MASK));
        }
        if self.interrupt_status & ENABLE_UB_INTS == 0 {
            return None;
        }
        self.ioboard.interrupt_request(self.ns).map(|vector| UB_INT | (vector & VECTOR_MASK))
    }

    /// Plugs the Chaosnet in as [`Machine::chaos`] describes it: the
    /// interface on the I/O board at the configured address, and on its
    /// cable the Chaosnet hosts that are not in this process, if a CHUDP
    /// link was bound. Powered at `powered_at` on the machine's clock.
    ///
    /// **Nothing else is on that cable.** A CADR carries no file or time
    /// server, so neither does this; what a band calls for its files and
    /// the date is a host on the network, reached over the link.
    /// [`Machine::attach_chaos_node`] is how a caller in this process ---
    /// the test harness, with its Chaosnet server --- puts one there
    /// instead.
    pub fn plug_chaos(&mut self, powered_at: u64) {
        let mut ether = crate::chaos::ether::Ether::new();
        if let Some(node) = self.chaos.udp_node() {
            ether.attach(node);
        }
        self.ioboard.plug_chaos(self.chaos.address, Some(ether), powered_at, self.chaos.trace);
    }

    /// Puts `node` on this machine's Chaosnet cable, beside the CHUDP
    /// link's if there is one: a station like any other, heard by the
    /// interface and taking its turn to transmit.
    ///
    /// After [`Machine::plug_chaos`], which is what makes the cable; a
    /// machine with none has nothing to attach to and this panics rather
    /// than dropping the node on the floor.
    pub fn attach_chaos_node(&mut self, node: Box<dyn crate::chaos::ether::Node>) {
        self.ioboard
            .chaos
            .as_mut()
            .and_then(|c| c.ether_mut())
            .expect("a Chaosnet cable: plug_chaos first")
            .attach(node);
    }

    /// The bus interface's own registers, as the board reads them back.
    fn interface_read(&self, r: busint::Register) -> u16 {
        use busint::{Register, error_status, interrupt_status};
        match r {
            // A diagnostic register is the cpu's own state on `SPY<15:0>`
            // under `-DBREAD`, and the engine answers it before the bus is
            // asked: [`crate::engine::Engine::spy_read`].  Here, where no
            // engine is, the bus reads as if the cpu had driven nothing.
            Register::Diagnostic(_) => spy::OPEN_READ,
            Register::InterruptControl2 | Register::Unused => 0,
            Register::InterruptControl => {
                let live = if self.xbus_interrupt() { interrupt_status::XBUS_INTR } else { 0 };
                let taken = self.unibus_interrupt().unwrap_or(0);
                let stored = self.interrupt_status
                    & !(interrupt_status::XBUS_INTR
                        | interrupt_status::UB_INT
                        | interrupt_status::VECTOR_MASK);
                stored | live | taken
            }
            // The 74LS244 at REQERR 0C16 drives eight bits of `UDO`; the
            // high byte is the Unibus pulled up, and reads as ones ---
            // **measured on the netlist board**, the processor's own read
            // of the register in `tests/chip.rs`.
            Register::ErrorStatus => {
                0xff00
                    | self.bus_error
                    | error_status::NOT_FREE
                    | if self.write_through { error_status::WRITE_THROUGH } else { 0 }
            }
            Register::Map(k) => self.unibus_map[k as usize],
        }
    }

    /// A Unibus map entry as the 29701s at UBMAP 0E12-0E15 hold it: the
    /// physical page and whether writes are allowed, or `None` if the
    /// page's `MAPVALID` is down.  "Bit 15 of the register signifies that
    /// mapping of that page is turned on; otherwise, that section of the
    /// unibus is non-existent memory.  Bit 14 enables write access from the
    /// unibus.  The remainder of the register is the page number" ---
    /// `unaddr.text`; `UDI15` to `MAPVALID`, `UDI14` to `WRITEOK`,
    /// `UDI<13:0>` to `UBMA<21:8>` on the netlist.
    pub fn map_entry(&self, page: u8) -> Option<(u32, bool)> {
        let e = self.unibus_map[page as usize & 0o17];
        (e & 0x8000 != 0).then_some(((e & 0x3fff) as u32, e & 0x4000 != 0))
    }

    /// A Unibus master's read through the map, [`busint::map_access`].  The
    /// even word is an Xbus read of the mapped word --- `-UB READ XBUS` ---
    /// whose low half is the answer and whose high half goes into the
    /// page's read buffer; the odd word is the buffer, `-UB READ BUFFER`,
    /// with no Xbus cycle.  `None` when the page is invalid, which is
    /// `UB MAP ERROR` and no answer.
    pub fn mapped_read(&mut self, access: busint::MapAccess) -> Option<u16> {
        let k = access.page as usize;
        if access.high {
            return Some(self.read_buffer[k]);
        }
        let Some((page, _)) = self.map_entry(access.page) else {
            self.bus_error |= bus_error::UB_MAP_ERROR;
            return None;
        };
        let word = self.bus_read((page << 8) | access.word);
        self.read_buffer[k] = (word >> 16) as u16;
        Some(word as u16)
    }

    /// A Unibus master's write through the map: the even word into the
    /// page's write buffer, `-UB WRITE BUFFER`; the odd word an Xbus write
    /// of the two halves, `-UB WR XBUS`, if the page is valid and writable
    /// --- else `UB MAP ERROR`, no write and no answer, `false`.  In
    /// write-through mode, bit 7 of the error status register, the even
    /// word's write on the upper eight pages is an Xbus write as well, of
    /// the Unibus word with zeros above it --- **measured on the netlist
    /// board**, `tests/chip.rs`.
    pub fn mapped_write(&mut self, access: busint::MapAccess, v: u16) -> bool {
        let k = access.page as usize;
        if !access.high {
            self.write_buffer[k] = v;
            // Write-through mode, `WRITE THROUGH ENB` at UBCYC 0B08, on the
            // upper eight pages: the low half's write goes to the Xbus at
            // once, and the 74LS244s at BUSSEL under `-UB16>BUS` put the
            // Unibus word on `BUS<15:0>` and ground on `BUS<31:16>`.
            if !(self.write_through && access.page >= 0o10) {
                return true;
            }
        }
        match self.map_entry(access.page) {
            Some((page, true)) => {
                let word = if access.high {
                    (v as u32) << 16 | self.write_buffer[k] as u32
                } else {
                    v as u32
                };
                // A page with its high five bits ones is `MD`, not the
                // Xbus: `-UB TO MD`, CC's `CC-WRITE-MD`.
                if busint::map_to_md(page) {
                    self.md = word as Word;
                } else {
                    self.bus_write((page << 8) | access.word, word as Word);
                }
                true
            }
            _ => {
                self.bus_error |= bus_error::UB_MAP_ERROR;
                false
            }
        }
    }

    /// The error status register's eight bits as the 8304 at REQERR 0B15
    /// puts them on the debug cable under `-DB READ STATUS`: [`bus_error`]'s
    /// bits, `-FREE` in bit 6 as it stands --- which a read of `766044` by
    /// the processor always finds up, the interface being busy with that
    /// read, and which the debugger's strobe finds as `busy` says --- and
    /// `WRITE THROUGH ENB` in bit 7.  CC's `DBG-PRINT-STATUS` names the
    /// low six and ignores the rest.
    pub fn debug_status(&self, busy: bool) -> u16 {
        use busint::error_status;
        self.bus_error
            | if busy { error_status::NOT_FREE } else { 0 }
            | if self.write_through { error_status::WRITE_THROUGH } else { 0 }
    }

    fn interface_write(&mut self, r: busint::Register, v: u16) {
        use busint::{Register, error_status, interrupt_status};
        match r {
            Register::Diagnostic(r) => self.spy_write(r, v),
            Register::Unused => {}
            Register::InterruptControl => {
                self.interrupt_status = (self.interrupt_status & !interrupt_status::CONTROL_MASK)
                    | (v & interrupt_status::CONTROL_MASK);
            }
            Register::InterruptControl2 => {
                self.interrupt_status = (self.interrupt_status & !interrupt_status::CONTROL2_MASK)
                    | (v & interrupt_status::CONTROL2_MASK);
            }
            // "Writing this location ignores the data written and clears
            // the status bits" --- all but the one the drawings clock from it.
            Register::ErrorStatus => {
                self.bus_error = 0;
                self.write_through = v & error_status::WRITE_THROUGH != 0;
            }
            Register::Map(k) => self.unibus_map[k as usize] = v,
        }
    }

    /// Loads one of the console's registers, as the trailing edge of the
    /// write strobe does --- from a Unibus write into `766000`-`766036`, or
    /// from a console that has no bus.  `eadr` is `EADR<3:0>`; only
    /// `EADR<2:0>` reach the write decoder, so 8 to 15 alias 0 to 7.  See
    /// [`crate::spy`].
    ///
    /// Two bits of a mode-register write are pulses and not settings:
    /// [`spy::MODE_RESET`] and [`spy::MODE_BOOT`] are gated with the strobe
    /// on OLORD2, so they are recorded here for the engine to act on and
    /// nothing is stored.  `RESET` clears the mode register itself, which is
    /// why CC's `CC-RESET-MACH` writes `100` and then the mode it wants.
    pub fn spy_write(&mut self, eadr: u8, v: u16) {
        match spy::write_strobe(eadr) {
            half @ (spy::IR_LOW | spy::IR_MED | spy::IR_HIGH) => {
                spy::write_debug_ir(&mut self.debug_ir, half, v)
            }
            spy::LDDBIRX if self.geometry.extended() => {
                spy::write_debug_ir(&mut self.debug_ir, 3, v)
            }
            spy::CLK => self.clock_control.write(v),
            spy::OPC_CONTROL => self.opc_control.write(v),
            spy::MODE => {
                self.mode.write(v);
                // QUUX's mode register has no speed bits: they go nowhere.
                if !self.geometry.speed_bits {
                    self.mode.speed0 = false;
                    self.mode.speed1 = false;
                }
                self.prog_reset |= v & spy::MODE_RESET != 0;
                self.prog_boot |= v & spy::MODE_BOOT != 0;
            }
            _ => {}
        }
    }

    /// What `-RESET` clears of the console's registers: the mode register,
    /// the OPC control register and the 74S175 half of the clock control
    /// register.  `RUN` is cleared by `-CLOCK RESET A`, the power-on reset,
    /// and not by this, so a console that resets the machine leaves it
    /// running or halted as it was.
    pub fn reset_console_registers(&mut self) {
        self.mode = spy::Mode::default();
        self.opc_control = spy::OpcControl::default();
        self.clock_control =
            spy::ClockControl { run: self.clock_control.run, ..Default::default() };
    }

    /// `RESET` on the bus interface, which it puts on the backplane as
    /// `-XBUS INIT` (the 26S10 at XA 0F21) and `-UB INIT` (the 8838 at
    /// 0F06).  The 74S10 at DBGIN 0A14 makes it from three inputs: `-LM
    /// UNIBUS RESET`, the processor's `PROG.UNIBUS.RESET` ---
    /// `INTERRUPT-CONTROL<28>`, the 25LS2519 at FLAG 3E08, over the cable
    /// as `-BUS.RESET` --- which microcode 323 holds for about 80
    /// microseconds at its start, "RESET THE BUS INTERFACE AND I/O DEVS";
    /// `-DEBUGEE RESET`, the debug modifier's reset bit from the debug
    /// cable; and `UNIBUS INIT IN` from another master of the Unibus.
    ///
    /// Each model board clears what its own reset pin clears and nothing
    /// more: [`Tv::xbus_init`], [`Controller::xbus_init`],
    /// [`IoBoard::unibus_init`].  The netlist boards on `chip` take the wire
    /// itself, and behind the netlist interface [`crate::buses::Buses`]
    /// calls this as the wire is asserted.  The memory boards are the
    /// engine's twins, held by the same wire in
    /// [`busint::Busint::unibus_reset`].  Power-on is the constructor:
    /// every board here is built in its reset state.
    /// The model boards clear what their drawings say on the interface's
    /// `RESET` --- `-XBUS INIT` and `-UB INIT`.  Three things drive it: the
    /// machine's own `PROG.UNIBUS.RESET`, from `Rtl` as the `INTERRUPT-
    /// CONTROL` write lands and from `Micro` at the functional destination;
    /// the debug cable's `-DEBUGEE RESET` (`Rtl::debug_cycle`); and, on the
    /// `chip` engine, `-XBUS INIT` off the netlist (`crate::buses`).
    /// Microcode 323 pulses its own only as it starts --- the boot PROM's
    /// "RESET THE BUS INTERFACE AND I/O DEVS" and `RESET-MACHINE` in
    /// `uc-cold-disk.lisp`, with nothing in flight --- and cannot do so by
    /// accident later: every other write of `INTERRUPT-CONTROL` is `IOR`
    /// or `ANDCA` of `LOCATION-COUNTER` with `1_26.`, and the `LC` source
    /// hands the flop back as `MF28` (the 74S241 at LC 1A16, pin 8 to pin
    /// 12), so bit 28 is rewritten as it stands.  Watched over a
    /// two-machine run of CC's diagnostics: three pulses at A's boot and
    /// one at B's, none after.
    pub fn bus_reset(&mut self) {
        self.tv.xbus_init(self.ns);
        // `-XBUS INIT` is a bused line and reaches every board on it, the
        // second display board with the rest.
        if let Some(tv) = self.color_tv.as_mut() {
            tv.xbus_init(self.ns);
        }
        self.disk.xbus_init();
        if let Some(d) = self.block_disk.as_mut() {
            d.xbus_init();
        }
        // QUUX's file device: disabled, its queue dropped and its handles
        // closed, status `<2>` and `<3>` cleared (contract Q9).
        self.file_device.reset();
        self.ioboard.unibus_init();
    }

    /// **Reset devices** (contract Q11, revision 10): a write of the
    /// register page's word 104 with `<0>` set, at [`Machine::ns`], the
    /// instant the write is taken. What the file device had due by then
    /// runs first, as for a write of its word 160; then every device is
    /// reset --- block-disk, the network (the I/O board's `-UB INIT`, its
    /// Chaosnet interface and serial line) and the file device as the
    /// CADR's `PROG.UNIBUS.RESET` resets its boards, [`Machine::bus_reset`],
    /// which on QUUX nothing else calls; the video controller, which has no vertical flag
    /// and no interrupt, with nothing to show for it --- and every interval
    /// timer to its reset state ([`IntervalTimer::RESET`]). The keyboard and
    /// the mouse are not reset: a warm boot's key word is read by the
    /// microcode's location 6 after the PROM, which writes this, has run.
    /// Nothing is held off `SINTR` at the write's edge: a register write
    /// lands after the `SINTR` of the edge that takes it (contract Q11,
    /// section 4).
    pub fn reset_devices(&mut self) {
        self.advance_file_device();
        self.bus_reset();
        self.timers = Timers::new();
    }

    /// A read of a diagnostic register through this alone reads the open
    /// bus; the engines answer them.  See [`crate::spy`].
    ///
    /// **On QUUX the register page is looked at first** (contract Q13),
    /// then the frame buffer window and main memory; everything else is
    /// nothing there ([`busint::decode_quux_13`]), the CADR's Unibus
    /// window included, which QUUX does not have (contract Q5).
    ///
    /// **Only main memory holds a whole word** (contract G2 §2.5): every
    /// other thing on the bus reads 0 above bit 31.
    pub fn bus_read(&mut self, phys: u32) -> Word {
        if let Some(w) = self.geometry.feature_word(phys) {
            // Words 11 to 13 are the main screen, from the board fitted:
            // width in 31:16 and height in 15:0; bits a pixel in 31:16 and
            // words a line in 15:0; and the buffer's first physical address.
            let (width, height, words_per_line) = self.tv.screen();
            let paged = self.geometry.paged();
            return Word::from(match phys & 0o377 {
                // Revision 14 (A14.9): word 2 the TLB's entries, word 13
                // the buffer's device-window address, 220-227 the memory
                // system's words.
                0o2 if paged => self.tlb.len() as u32,
                0o13 if paged => crate::tlb::DEVICE_WINDOW,
                // Revision 15 (A15b.1): word 25 the microcycle in units of
                // 0.5 ns, word 225 the posted writes answered with an error.
                0o25 if self.geometry.extended() => self.period as u32,
                0o225 if self.geometry.extended() => self.posted_write_errors,
                k @ 0o220..=0o227 if paged => self.memory_words.read(k).unwrap_or(0),
                0o11 => (width as u32) << 16 | height as u32,
                0o12 => 1 << 16 | words_per_line as u32,
                0o13 => WINDOW_13,
                // The board name (contract HD §6.4).
                k @ 0o20..=0o24 => self.board_name_word(k - 0o20),
                // The register page (contract Q2): who interrupted, the
                // bus errors, and the mode.
                0o100 => self.interrupt_sources(),
                0o101 => self.bus_error as u32,
                0o102 => self.mode.errstop as u32,
                // The real-time clock (contract Q9).
                0o103 if self.geometry.rtc => {
                    self.rtc.seconds_at(self.ns, self.time_base().per_s())
                }
                // The interval timers (contract Q11), at the machine's
                // clock.
                k @ 0o110..=0o115 if self.geometry.tick => {
                    self.timers.read(k, self.ns).unwrap_or(0)
                }
                // The file device (contract Q9), at the machine's clock.
                k @ 0o160..=0o171 if self.geometry.file_device => {
                    self.advance_file_device();
                    self.file_device.read(k, self.ns)
                }
                // The network (contract Q4): the Chaosnet interface's five
                // registers (contract Q13).
                k @ 0o140..=0o147 => match network_register(k, false) {
                    Some(r) => {
                        let ns = self.device_ns();
                        self.ioboard.read(r, ns) as u32
                    }
                    None => 0,
                },
                // Block-disk (contract Q13), when it is fitted.
                k @ 0o200..=0o203 => match self.block_disk.as_mut() {
                    Some(d) => {
                        d.advance(self.ns);
                        d.read(k - 0o200)
                    }
                    None => 0,
                },
                // The video controller's mode (contract Q13).
                0o210 if self.tv.board() == tv::Board::Video => {
                    self.tv.read_control(0, self.device_ns())
                }
                k => self.quux_input.read(k).unwrap_or(w),
            });
        }
        if self.geometry.paged() {
            return match self.space_14(phys) {
                Space13::Window(off) => UNBOXED_TAG | Word::from(self.tv.read_buffer(off)),
                Space13::Main(a) => self.main[a],
                Space13::Nothing => 0,
            };
        }
        if self.geometry.has_register_page() {
            return match self.space_13(phys) {
                Space13::Window(off) => UNBOXED_TAG | Word::from(self.tv.read_buffer(off)),
                Space13::Main(a) => self.main[a],
                Space13::Nothing => 0,
            };
        }
        if let Some(off) = self.tv.buffer_offset(phys) {
            return self.tv.read_buffer(off).into();
        }
        if let Some(r) = disk_controller::register(phys) {
            self.disk.advance(self.ns);
            return self.disk.read(r).into();
        }
        if let Some(r) = self.tv_register(phys) {
            return self.tv.read_control(r, self.ns).into();
        }
        // The color TV, when one is fitted: the same board at the other
        // strap.  `device` has already given the NXM when it is not.
        if let Some(tv) = self.color_tv.as_ref() {
            if let Some(off) = tv::COLOR_TV.buffer_offset(phys) {
                return tv.read_buffer(off).into();
            }
            if let Some(r) = tv::COLOR_TV.control_register(phys) {
                return tv.read_control(r, self.ns).into();
            }
        }
        // The Unibus carries 16 bits, in the bottom of one Lisp machine word.
        if let Some(r) = busint::unibus_address(phys).and_then(busint::register) {
            return self.interface_read(r).into();
        }
        if let Some(r) = busint::unibus_address(phys).and_then(|u| ioboard::answers(u, false)) {
            return self.ioboard.read(r, self.ns).into();
        }
        match self.device(phys) {
            Some(a) => self.main[a],
            None => 0,
        }
    }

    /// Which of the main display's control registers a physical address
    /// names, if it is one the board has.
    fn tv_register(&self, phys: u32) -> Option<u32> {
        tv::control_register(phys).filter(|&r| self.tv.control_registers() >> r & 1 != 0)
    }

    /// Revision 13, past the register page: the window's word, main
    /// memory's, or nothing there, which sets word 101 `<0>`
    /// ([`busint::decode_quux_13`]).
    fn space_13(&mut self, phys: u32) -> Space13 {
        match busint::decode_quux_13(phys, self.main.len(), self.tv.buffer_words()) {
            busint::Responder::Memory(_) if phys >= WINDOW_13 => Space13::Window(phys - WINDOW_13),
            busint::Responder::Memory(_) => Space13::Main(phys as usize),
            _ => {
                self.bus_error |= bus_error::XBUS_NXM;
                Space13::Nothing
            }
        }
    }

    /// Revision 14, past the register page: the bus address `bus` in main
    /// memory, the frame buffer in the device window's slice 0, or nothing
    /// there, which sets word 101 `<0>` ([`busint::decode_quux_14`]).
    fn space_14(&mut self, bus: u32) -> Space13 {
        match busint::decode_quux_14(bus, self.main.len(), self.tv.buffer_words()) {
            busint::Responder::Memory(_) if bus & crate::tlb::DEVICE != 0 => {
                Space13::Window(bus & !crate::tlb::DEVICE)
            }
            busint::Responder::Memory(_) => Space13::Main(bus as usize),
            _ => {
                self.bus_error |= bus_error::XBUS_NXM;
                Space13::Nothing
            }
        }
    }

    /// **Only main memory takes a whole word** (contract G2 §2.5): every
    /// other thing on the bus takes `<31:0>` of it.
    pub fn bus_write(&mut self, phys: u32, word: Word) {
        let value = word as u32;
        if let Some(log) = self.register_log.as_mut()
            && (self.geometry.feature_word(phys).is_some()
                || (!self.geometry.has_register_page()
                    && disk_controller::register(phys).is_some()))
        {
            log.push((phys, value, self.cycles));
        }
        // QUUX's register page (contract Q2): a write of word 101 clears the
        // bus errors, as a write of `766044` does, and word 102 `<0>` is
        // error stop; the features, the real-time clock (word 103) and the
        // reserved words ignore writes.
        if self.geometry.feature_word(phys).is_some() {
            match phys & 0o377 {
                0o101 => self.bus_error = 0,
                0o102 => self.mode.errstop = value & 1 != 0,
                // Reset devices (contract Q11): `<0>` set resets every
                // device; clear, nothing.
                0o104 if self.geometry.tick => {
                    if value & 1 != 0 {
                        self.reset_devices();
                    }
                }
                // The interval timers (contract Q11).
                k @ 0o110..=0o115 if self.geometry.tick => {
                    let per_us = self.time_base().per_us();
                    self.timers.write(k, value, self.ns, per_us);
                }
                // The file device (contract Q9): what was due is done
                // first, and a producer index is taken from when the write
                // buffer is empty.
                k @ 0o160..=0o171 if self.geometry.file_device => {
                    self.advance_file_device();
                    let (ns, drained) = (self.ns, self.write_buffer_empty_at);
                    let time = self.time_base();
                    self.file_device.write(k, value, (ns, time), drained, &self.main);
                }
                // The network: the CSR and the write buffer; writes of its
                // other words are ignored (contract Q13).
                k @ 0o140..=0o147 => {
                    if let Some(r) = network_register(k, true) {
                        let ns = self.device_ns();
                        self.ioboard.write(r, value as u16, ns);
                    }
                }
                // Block-disk (contract Q13): a transfer is a bus master
                // reading and writing physical memory directly, which is
                // why it is handed it, and it writes main memory behind
                // the processor's back, so QUUX's cache is invalidated
                // before its next cycle.
                k @ 0o200..=0o203 => {
                    let unit = self.time_base().per_ns;
                    if let Some(d) = self.block_disk.as_mut() {
                        d.unit = unit;
                        d.advance(self.ns);
                        d.write(k - 0o200, value, &mut self.main);
                        self.dma_written = true;
                    }
                }
                // The video controller's mode (contract Q13).
                0o210 if self.tv.board() == tv::Board::Video => {
                    let ns = self.device_ns();
                    self.tv.write_control(0, value, ns);
                }
                // Revision 15's word 225: a write clears it (A15b.1).
                0o225 if self.geometry.extended() => self.posted_write_errors = 0,
                // Revision 14's memory system's words (A14.9).
                k @ 0o220..=0o227 if self.geometry.paged() => {
                    self.memory_words.write(k, value);
                }
                k => {
                    self.quux_input.write(k, value);
                }
            }
            return;
        }
        if self.geometry.has_register_page() {
            let space =
                if self.geometry.paged() { self.space_14(phys) } else { self.space_13(phys) };
            match space {
                // The window stores the field and drops the tag (G1 §4.2).
                Space13::Window(off) => self.tv.write_buffer(off, value),
                Space13::Main(a) => {
                    self.main[a] = word;
                    if let Some(log) = self.store_log.as_mut() {
                        log.push(a as u32);
                    }
                }
                Space13::Nothing => {}
            }
            return;
        }
        if let Some(off) = self.tv.buffer_offset(phys) {
            self.tv.write_buffer(off, value);
            return;
        }
        if let Some(r) = disk_controller::register(phys) {
            // A transfer is a bus master reading and writing physical memory
            // directly, which is why the controller is handed it.
            self.disk.advance(self.ns);
            self.disk.write(r, value, &mut self.main);
            self.dma_written = true;
            return;
        }
        if let Some(r) = self.tv_register(phys) {
            self.tv.write_control(r, value, self.ns);
            return;
        }
        // The color TV, when one is fitted.
        let ns = self.ns;
        if let Some(tv) = self.color_tv.as_mut() {
            if let Some(off) = tv::COLOR_TV.buffer_offset(phys) {
                tv.write_buffer(off, value);
                return;
            }
            if let Some(r) = tv::COLOR_TV.control_register(phys) {
                tv.write_control(r, value, ns);
                return;
            }
        }
        // The Unibus carries 16 bits, in the bottom of one Lisp machine word.
        if let Some(r) = busint::unibus_address(phys).and_then(busint::register) {
            self.interface_write(r, value as u16);
            return;
        }
        if let Some(r) = busint::unibus_address(phys).and_then(|u| ioboard::answers(u, true)) {
            self.ioboard.write(r, value as u16, self.ns);
            return;
        }
        if let Some(a) = self.device(phys) {
            self.main[a] = word;
            if let Some(log) = self.store_log.as_mut() {
                log.push(a as u32);
            }
        }
    }
}

impl Default for Machine {
    fn default() -> Self {
        Self::new()
    }
}

/// What the 74S373 latch at VMEMDR 1D14 holds at power-on: the map word of
/// the last memory cycle, which there has been none of. Permission bits
/// up, so that nothing faults before the first cycle, and a page number of
/// all ones. `rtl` and `micro` both start their latch from it.
///
/// The board has no answer to give. A 74S373 has no clear, and what it is
/// transparent onto is the second-level map, whose 93425As on VMEM0 to
/// VMEM2 come up holding whatever they come up holding; so this is a
/// modeling choice and not a fact about the hardware. What it has to be
/// is unobservable, and it is:
/// the only read of the map before the first memory cycle is
/// `SET-UP-THE-MAP` in `sys/ucadr/promh.text`, which takes
/// `MEMORY-MAP-DATA` and keeps `(BYTE-FIELD 5 24.)`, and AIM-528 gives
/// `MAP<28-24>` as the first-level map --- written to zero two cycles
/// earlier by the `VMA-WRITE-MAP` above it. This latch reaches only
/// `MAP<31-30>`, the fault bits of the last memory cycle, which that
/// microinstruction discards. What the constant is really for is keeping
/// `rtl` and `micro` on the same number as `chip`, and a `chip` read of
/// `MAP[MD]` before any memory cycle is what would fix it.
pub const LVMO_AT_POWER_ON: u32 = (1 << 23) | (1 << 22) | 0x3fff;

/// **Revision 13's physical space** (contract G1 §3.2, G2 §4.1): 28-bit
/// word addresses, main memory from 0, the frame buffer window from
/// [`WINDOW_13`], the register page at the last page, and nothing
/// anywhere else.
///
/// The register page, `1777777400`-`1777777777` ([`Geometry::feature_word`]).
pub const REGISTER_PAGE_13: u32 = 0o1777777400;

/// The board name's most characters, 4 to a word in feature words 20-24
/// (contract HD §6.4).
pub const BOARD_NAME_CHARS: usize = 20;

/// muir-sim's board name, on both engines.
pub const BOARD_NAME: &str = "muir-sim";

/// The frame buffer window, `1760000000`-`1777775777`: the video
/// controller's buffer from its base, 4 bytes a word, the field only (G1
/// §4.2). Main memory ends below it.
pub const WINDOW_13: u32 = 0o1760000000;

/// The tag a word read from the window, a 4-byte transfer or the file
/// device's buffers carries: `005`, the unboxed fixnum's (G1 §2.6).
pub const UNBOXED_TAG: Word = 0o005 << 32;

/// Revision 13's invalid level-2 block, whose 32 entries the software
/// keeps 0: what level 1 reads for an address with `<31:28>` not zero
/// (A1.7; today's `A-LEVEL-1-MAP-INVALID`, `77`).
pub const MAP_INVALID_BLOCK_13: u32 = 0o177;

/// Revision 13's level-2 entry: 28 bits, the 18-bit physical page and the
/// ten bits above it (A1.7, which corrects G1 §3.3's 29).
pub const MAP_LEVEL_2_13: u32 = (1 << 28) - 1;

/// Revision 13's level-2 index: the block level 1 gives, and `VA<14:10>`
/// in it (A1.7).
pub fn map_level_2_index_13(l1: u32, addr: u32) -> usize {
    (((l1 & 0o177) << 5) | ((addr >> 10) & 0o37)) as usize
}

/// Where a revision-13 physical address lands past the register page.
enum Space13 {
    /// The frame buffer window, at this word of the buffer.
    Window(u32),
    /// Main memory, at this word.
    Main(usize),
    Nothing,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Translation {
    /// 22-bit physical address; 28 bits on revision 13.
    pub physical: u32,
    /// 14-bit physical page number; 18 bits on revision 13, of 1024 words.
    pub page: u32,
    pub l1_data: u32,
    pub l2_data: u32,
    pub write_permitted: bool,
    pub access_permitted: bool,
}

// --- Checkpoints ------------------------------------------------------------

impl Machine {
    /// The machine into a checkpoint: every memory and register, the disk
    /// controller with its drives, the display, the I/O board, and its
    /// clocks.  Not [`Machine::chaos`], which is the flags' say and a
    /// resume has again.
    pub fn save(&self, w: &mut crate::checkpoint::Writer) {
        let Machine {
            prom,
            imem,
            mode,
            clock_control,
            opc_control,
            debug_ir,
            prog_reset,
            prog_boot,
            amem,
            mmem,
            dmem,
            pdl,
            spc,
            spcptr,
            pdl_pointer,
            pdl_index,
            q,
            opc,
            lc,
            vma,
            md,
            interrupt_control,
            dispatch_constant,
            overflow,
            // Not in a checkpoint: a resume starts with it swept (A14.14).
            tlb: _,
            memory_words,
            pdl_copies,
            geometry,
            timers,
            macro_dispatch,
            rtc,
            dma_written,
            block_disk,
            file_device,
            write_buffer_empty_at: _,
            period: _,
            posted_write_errors,
            store_log: _,
            register_log: _,
            board_name: _,
            l1_map,
            l2_map,
            main,
            bus_error,
            interrupt_status,
            write_through,
            unibus_map,
            read_buffer,
            write_buffer,
            vmaok,
            disk,
            chaos: _,
            tv,
            color_tv,
            ioboard,
            quux_input,
            cycles,
            lc_steps: _,
            ns,
        } = self;
        // Words from here on, the engine's after the machine's, at the
        // machine's width: four bytes on a 32-bit machine, as always.
        w.set_word_bits(geometry.word_bits);
        w.u64s(&prom.iter().map(|i| i.raw()).collect::<Vec<_>>());
        w.u64s(&imem.iter().map(|i| i.raw()).collect::<Vec<_>>());
        mode.save(w);
        clock_control.save(w);
        opc_control.save(w);
        w.u64(*debug_ir);
        w.bool(*prog_reset);
        w.bool(*prog_boot);
        w.words(amem);
        w.words(mmem);
        // The dispatch memory and the two map levels at the machine's
        // sizes: a 32-bit machine's checkpoint keeps the bytes it had.
        let wide = geometry.wide();
        w.u32s(&dmem[..geometry.dmem_words()]);
        w.words(pdl);
        w.u32s(spc);
        w.u8(*spcptr);
        w.u16(*pdl_pointer);
        w.u16(*pdl_index);
        w.word(*q);
        w.u16(*opc);
        // `<31:0>` with NEED-FETCH in bit 31 to revision 13, as always;
        // revision 14's counter's `<33:32>` and its NEED-FETCH go at the
        // end, below.
        w.u32(*lc as u32);
        w.word(*vma);
        w.word(*md);
        w.u32(*interrupt_control);
        w.u16(*dispatch_constant);
        w.u32s(if wide { &l1_map[..] } else { &l1_map[..2048] });
        // The level-1 entry's bits, 0 on revision 14, which has no level-1
        // map; revision 15, which has none either, writes its revision
        // with `<7>` set, [`REVISION_15_MARK`] (A15b.13).
        w.u8(if geometry.extended() { REVISION_15_MARK } else { geometry.l1_bits as u8 });
        w.u8(geometry.pdl_bits as u8);
        w.bool(geometry.muldiv);
        w.bool(geometry.tick);
        w.bool(geometry.macro_dispatch);
        // A wider word says so, which a 32-bit machine's checkpoint never
        // has: its bytes stay what they were.
        if wide {
            w.u8(geometry.word_bits as u8);
            w.bool(*overflow);
        }
        timers.save(w);
        macro_dispatch.save(w);
        rtc.save(w);
        file_device.save(w);
        w.bool(*dma_written);
        w.u32s(if wide { &l2_map[..] } else { &l2_map[..2048] });
        w.u32(self.memory_boards() as u32);
        w.words(main);
        w.u16(*bus_error);
        w.u16(*interrupt_status);
        w.bool(*write_through);
        w.u16s(unibus_map);
        w.u16s(read_buffer);
        w.u16s(write_buffer);
        w.bool(*vmaok);
        disk.save(w);
        w.opt(block_disk.as_ref(), |w, d| d.save(w));
        tv.save(w);
        // Whether the color TV was on the backplane, and if it was, the
        // board: a resume onto a machine `--color-tv` says otherwise about
        // is refused by the flag's name.
        w.bool(color_tv.is_some());
        if let Some(tv) = color_tv {
            tv.save(w);
        }
        ioboard.save(w);
        quux_input.save(w);
        w.u64(*cycles);
        w.u64(*ns);
        // Revision 14's own (A14.14), after everything revision 13 has:
        // `LC<40:32>`, NEED-FETCH and the counter's top two bits, and the
        // memory system's words. The TLB is not kept; the latched entry
        // behind `MAP(MD)`'s fault bits is the engine's.
        if geometry.paged() {
            w.u16((*lc >> 32) as u16);
            memory_words.save(w);
            w.u32(pdl_copies.base);
            w.u16(pdl_copies.head);
        }
        // Revision 15's own (A15b.13), after revision 14's: word 225. The
        // control store's 64 bits are in its words above.
        if geometry.extended() {
            w.u32(*posted_write_errors);
        }
    }

    /// The first part of [`Machine::load`]: everything a checkpoint holds
    /// up to and including the machine's geometry, which is how
    /// [`Machine::checkpointed_geometry`] learns whose a checkpoint is
    /// without reading the rest of it.
    fn load_to_geometry(&mut self, r: &mut crate::checkpoint::Reader) -> std::io::Result<()> {
        fn insns(r: &mut crate::checkpoint::Reader, into: &mut [Insn]) -> std::io::Result<()> {
            let mut raw = vec![0u64; into.len()];
            r.u64s_into(&mut raw)?;
            // All 64 bits, revision 15's extension among them; a machine
            // of 48-bit words drops the rest once its geometry is read.
            for (i, w) in into.iter_mut().zip(raw) {
                *i = Insn::extended(w);
            }
            Ok(())
        }
        insns(r, &mut self.prom)?;
        insns(r, &mut self.imem)?;
        self.mode = spy::Mode::load(r)?;
        self.clock_control = spy::ClockControl::load(r)?;
        self.opc_control = spy::OpcControl::load(r)?;
        self.debug_ir = r.u64()?;
        self.prog_reset = r.bool()?;
        self.prog_boot = r.bool()?;
        r.words_into(&mut self.amem)?;
        r.words_into(&mut self.mmem)?;
        // Revision 13's sizes on a 40-bit machine, the only one there is.
        let wide = r.word_bits() > 32;
        let sizes = |big: usize| if wide { big } else { 2048 };
        r.u32s_into(&mut self.dmem[..sizes(DMEM_WORDS)])?;
        r.words_into(&mut self.pdl)?;
        r.u32s_into(&mut self.spc)?;
        // Pointers into the SPC stack and the PDL buffer: `SPCPTR<4:0>`,
        // five bits for the 32 words of the 82S21s on page SPC, and for the
        // PDL as many bits as the machine's geometry gives it --- ten on the
        // CADR, all page PDLPTR keeps of a write to either register --- which
        // is checked once the geometry is read, below.  The engines index
        // the arrays above with them, so a wider value is a corrupt
        // checkpoint and is refused rather than loaded.
        let spcptr = r.u8()?;
        if spcptr > 0o37 {
            return Err(crate::checkpoint::bad(format!(
                "SPC pointer {spcptr:o}, wider than the five bits of SPCPTR<4:0>"
            )));
        }
        let pdl_pointer = r.u16()?;
        let pdl_index = r.u16()?;
        self.spcptr = spcptr;
        self.pdl_pointer = pdl_pointer;
        self.pdl_index = pdl_index;
        self.q = r.word()?;
        self.opc = r.u16()?;
        self.lc = u64::from(r.u32()?);
        self.vma = r.word()?;
        self.md = r.word()?;
        self.interrupt_control = r.u32()?;
        self.dispatch_constant = r.u16()?;
        r.u32s_into(&mut self.l1_map[..sizes(L1_MAP_WORDS)])?;
        let (l1_bits, pdl_bits, muldiv) = (r.u8()? as u32, r.u8()? as u32, r.bool()?);
        let tick = r.bool()?;
        let fused = r.bool()?;
        // The word's width is the reader's to know; a wider machine's
        // checkpoint says it too, and the two must agree.
        let word_bits = r.word_bits();
        if word_bits != 32 {
            let said = r.u8()? as u32;
            if said != word_bits {
                return Err(crate::checkpoint::bad(format!(
                    "{said}-bit words, read as {word_bits}-bit ones"
                )));
            }
            self.overflow = r.bool()?;
        }
        self.timers = Timers::load(r)?;
        self.macro_dispatch.load(r)?;
        self.rtc = Rtc::load(r)?;
        self.file_device.load(r)?;
        self.dma_written = r.bool()?;
        // The CADR, or QUUX, revision 13 or 14, with a PDL buffer of 1K to 16K
        // words. A 32-bit QUUX is a retired revision's, 12 with the fused
        // return and 11 without, which wrote the CADR's format version and
        // is known by these fields: refused as what it is.
        self.geometry = match (word_bits, l1_bits, pdl_bits, muldiv, tick, fused) {
            (32, 5, 10, false, false, false) => Geometry::CADR,
            (40, 7, 10..=14, true, true, true) => Geometry { pdl_bits, ..Geometry::QUUX },
            // Revision 14 (A14.14): no level-1 map, its entry 0 bits.
            (40, 0, 10..=14, true, true, true) => Geometry { pdl_bits, ..Geometry::QUUX_14 },
            // Revision 15 (A15b.13): its mark in the level-1 entry's place.
            (40, l1, 10..=14, true, true, true) if l1 == u32::from(REVISION_15_MARK) => {
                Geometry { pdl_bits, ..Geometry::QUUX_15 }
            }
            (32, 6, 10..=14, true, true, fused) => {
                let revision = if fused { 12 } else { 11 };
                return Err(crate::checkpoint::bad(format!(
                    "a checkpoint of QUUX revision {revision}, which this build no longer runs: it runs the CADR and QUUX revisions 13 to 15, and a revision-{revision} checkpoint resumes only on an earlier muir-sim"
                )));
            }
            _ => {
                return Err(crate::checkpoint::bad(format!(
                    "{word_bits}-bit words, a map of {l1_bits}-bit level-1 entries and a {pdl_bits}-bit PDL buffer, multiply and divide {muldiv}, tick {tick}, fused return {fused}, is no machine's"
                )));
            }
        };
        if !self.geometry.extended() {
            for w in self.prom.iter_mut().chain(self.imem.iter_mut()) {
                *w = w.low_48();
            }
        }
        for (what, v) in [("PDL pointer", pdl_pointer), ("PDL index", pdl_index)] {
            if v > self.geometry.pdl_mask() {
                return Err(crate::checkpoint::bad(format!(
                    "{what} {v:o}, wider than the {pdl_bits} bits that address the PDL"
                )));
            }
        }
        Ok(())
    }

    /// **Which machine a `micro` or `rtl` checkpoint's body was written
    /// of**, the CADR or a QUUX, read as [`Machine::load`] reads it and no
    /// further: both engines' bodies begin with the machine, and its
    /// geometry is read before main memory. So a resume can say whose a
    /// checkpoint is before it builds anything, or before an engine's
    /// refusal would say something less to the point.
    pub fn checkpointed_geometry(body: &[u8]) -> std::io::Result<Geometry> {
        Self::checkpointed_geometry_at(body, 32)
    }

    /// [`Machine::checkpointed_geometry`] of a body of `word_bits`-bit
    /// words, as its file's version says
    /// ([`crate::checkpoint::Checkpoint::word_bits`]).
    pub fn checkpointed_geometry_at(body: &[u8], word_bits: u32) -> std::io::Result<Geometry> {
        let mut m = Machine::with_memory_boards(1);
        m.load_to_geometry(&mut crate::checkpoint::Reader::for_word_bits(body, word_bits))?;
        Ok(m.geometry)
    }

    /// Back from a checkpoint, into a machine built as the flags say: the
    /// same memory, the same pack under it, the same Chaosnet on it.
    ///
    /// **A checkpoint of 32-bit words is refused by QUUX** (contract G2
    /// §2.8): it is the CADR's, and its map, dispatch memory and devices
    /// are the CADR's. One of retired QUUX revision 12 is refused as that,
    /// by the machine its body records.
    pub fn load(&mut self, r: &mut crate::checkpoint::Reader) -> std::io::Result<()> {
        if self.geometry.wide() && r.word_bits() == 32 {
            Machine::with_memory_boards(1).load_to_geometry(r)?;
            return Err(crate::checkpoint::bad(
                "a checkpoint of 32-bit words, the CADR's, and this is QUUX, revision 13",
            ));
        }
        self.load_to_geometry(r)?;
        let l2 = if self.geometry.wide() { L2_MAP_WORDS } else { 2048 };
        r.u32s_into(&mut self.l2_map[..l2])?;
        let boards = r.u32()? as usize;
        if boards != self.memory_boards() {
            // QUUX's main memory is an amount, the CADR's boards.
            return Err(crate::checkpoint::bad(if self.geometry.revision().is_some() {
                let (had, has) = (megawords(boards << 16), megawords(self.main.len()));
                format!(
                    "{had} of main memory, and this machine has {has}: --main-memory-size {had}"
                )
            } else {
                format!(
                    "{boards} memory boards, and this machine has {}: --main-memory-boards {boards}",
                    self.memory_boards()
                )
            }));
        }
        r.words_into(&mut self.main)?;
        self.bus_error = r.u16()?;
        self.interrupt_status = r.u16()?;
        self.write_through = r.bool()?;
        r.u16s_into(&mut self.unibus_map)?;
        r.u16s_into(&mut self.read_buffer)?;
        r.u16s_into(&mut self.write_buffer)?;
        self.vmaok = r.bool()?;
        self.disk.load(r)?;
        match (r.bool()?, self.block_disk.as_mut()) {
            (true, Some(d)) => d.load(r)?,
            (false, None) => {}
            (saved, _) => {
                return Err(crate::checkpoint::bad(format!(
                    "a checkpoint {} block-disk, onto a machine {}",
                    if saved { "with" } else { "without" },
                    if saved { "without one" } else { "with one" }
                )));
            }
        }
        self.tv.load(r)?;
        self.color_tv = match r.bool()? {
            true => {
                let mut tv = Tv::color();
                tv.load(r)?;
                Some(tv)
            }
            false => None,
        };
        self.ioboard.load(r)?;
        self.quux_input.load(r)?;
        self.cycles = r.u64()?;
        self.ns = r.u64()?;
        if self.geometry.paged() {
            self.lc |= u64::from(r.u16()?) << 32;
            self.memory_words = crate::tlb::Words::load(r)?;
            self.pdl_copies =
                crate::tlb::PdlCopies { base: r.u32()?, head: r.u16()? & crate::tlb::PDL_INDEX };
            // The TLB is not kept: the resume starts with it swept.
            self.tlb = self.tlb.swept_copy();
        }
        if self.geometry.extended() {
            self.posted_write_errors = r.u32()?;
        }
        Ok(())
    }
}

/// The Chaosnet interface's register a word of QUUX's register page names,
/// read or written (contract Q4; contract Q13): 140 the CSR, read and
/// written; 141 my address read and the write buffer written; 142 the read
/// buffer, 143 the bit count and 145 START, each read only. Only these five
/// are decoded: 144, 146 and 147 are reserved, where the CADR's board
/// answers the CSR at `764150` and the bit count at `764156` as aliases,
/// and a write of 142, 143 or 145 goes nowhere, where the board takes one
/// of `764152` as the write buffer's ([`ioboard::answers`]).
fn network_register(word: u32, write: bool) -> Option<u32> {
    use crate::chaos::interface as chaos;
    match (word, write) {
        (0o140, _) => Some(chaos::CSR),
        (0o141, false) => Some(chaos::MY_ADDRESS),
        (0o141, true) => Some(chaos::WRITE_BUFFER),
        (0o142, false) => Some(chaos::READ_BUFFER),
        (0o143, false) => Some(chaos::BIT_COUNT),
        (0o145, false) => Some(chaos::START),
        _ => None,
    }
}
