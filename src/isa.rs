// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Decoding of the 48-bit CADR microinstruction.
//!
//! Bit positions are written `IR<hi:lo>` and follow `mit/cadr/ir.bits`, the field
//! diagram taken from the MIT CADR design files, and each was checked against
//! the drawing that decodes it.

/// A microinstruction.  Only the low 48 bits are significant, except on
/// QUUX revision 15, whose word is 64 bits: MIT's 48 in `IR<47:0>` and the
/// extension in `IR<63:48>` ([`Insn::extended`]).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Insn(u64);

/// Microinstruction class, `IR<44:43>`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Alu,
    Jump,
    Dispatch,
    Byte,
}

impl Insn {
    /// Truncates to 48 bits.
    pub fn new(raw: u64) -> Self {
        Insn(raw & 0xffff_ffff_ffff)
    }

    /// **QUUX revision 15's 64-bit word** (contract G3 revision 15, appendix
    /// A15b.2), all 64 bits kept: MIT's 48 in `IR<47:0>`, meaning what they
    /// mean on every other machine, and the extension in `IR<63:48>`, read
    /// by the class `IR<44:43>`. An all-zero extension is the 48-bit word.
    pub fn extended(raw: u64) -> Self {
        Insn(raw)
    }

    /// The word without its extension, `IR<47:0>`: what every class's
    /// fields below are read from.
    pub fn low_48(self) -> Self {
        Insn::new(self.0)
    }

    pub fn raw(self) -> u64 {
        self.0
    }

    /// `IR<63:48>` --- revision 15's extension (A15b.2); 0 on every other
    /// machine, whose words are 48 bits.
    pub fn extension(self) -> u16 {
        self.field(48, 16) as u16
    }

    /// `IR<60>` --- revision 15's `oa-low-select`, SL (A15b.2, A15b.15): on
    /// an ALU, BYTE or JUMP word, and on a dispatch-memory write
    /// (`IR<11:10>` = 2), the word reads OA-REG-LOW into its fields. On a
    /// dispatch that transfers it is no select: `IR<61:48>` is the
    /// predicted address, `<60>` its `<12>`.
    pub fn oa_low_select(self) -> bool {
        match self.op() {
            Op::Dispatch => self.field(10, 2) == 2 && self.field(60, 1) != 0,
            _ => self.field(60, 1) != 0,
        }
    }

    /// `IR<61>` --- revision 15's `oa-high-select`, SH (A15b.2, A15b.15): on
    /// an ALU, BYTE or JUMP word, the word reads OA-REG-HIGH into its A and
    /// M source. No dispatch takes it: on one that transfers `<61>` is the
    /// predicted address's `<13>`.
    pub fn oa_high_select(self) -> bool {
        self.op() != Op::Dispatch && self.field(61, 1) != 0
    }

    /// `IR<48>` on a JUMP --- revision 15's hint bit, H (A15b.2): 1 predicts
    /// that a conditional jump transfers. A prediction only; no result
    /// depends on it.
    pub fn hint(self) -> bool {
        self.field(48, 1) != 0
    }

    /// `IR<61:48>`, `IR<62>` and `IR<63>` on a DISPATCH --- revision 15's
    /// predicted target (A15b.2): the predicted entry's address, and its P
    /// and R stored inverted, all zeros being a drop-through. A prediction
    /// only; no result depends on it.
    pub fn predicted_target(self) -> (u16, bool, bool) {
        (self.field(48, 14) as u16, self.field(62, 1) == 0, self.field(63, 1) == 0)
    }

    /// `IR<58:48>` on an ALU or BYTE word --- revision 15's PDL address
    /// field (A15b.2), if `IR<48>`, E, says it is present: the base B,
    /// `IR<50:49>` (0 `M-AP`, 1 `A-LOCALP`, 2 the PDL pointer, 3
    /// PDL-INDEX), and the displacement D, `IR<58:51>`, signed. The word
    /// writes PDL-INDEX with B + D.
    pub fn pdl_field(self) -> Option<(u8, i8)> {
        (self.field(48, 1) != 0).then(|| (self.field(49, 2) as u8, self.field(51, 8) as u8 as i8))
    }

    fn field(self, pos: u32, len: u32) -> u32 {
        ((self.0 >> pos) & ((1u64 << len) - 1)) as u32
    }

    /// `IR<46>` --- counts this instruction in the statistics counter.
    pub fn statistics(self) -> bool {
        self.field(46, 1) != 0
    }

    /// `IR<45>` --- run the read phase of the clock long.
    pub fn ilong(self) -> bool {
        self.field(45, 1) != 0
    }

    /// `IR<44:43>`
    pub fn op(self) -> Op {
        match self.field(43, 2) {
            0 => Op::Alu,
            1 => Op::Jump,
            2 => Op::Dispatch,
            _ => Op::Byte,
        }
    }

    /// `IR<42>` --- pop the microcode stack after this instruction, in
    /// every class.  `mit/cadr/ir.bits` spells `POPJ` down the column for
    /// bit 42, one letter in each of the ALU, JUMP, DISP and BYTE rows.
    pub fn popj(self) -> bool {
        self.field(42, 1) != 0
    }

    /// `IR<41:32>` --- A memory address.  For DISPATCH this field is instead
    /// the dispatch constant, so use [`Insn::dispatch`] there.
    pub fn a_src(self) -> u16 {
        self.field(32, 10) as u16
    }

    /// `IR<31>` --- read the M source from a functional source rather than
    /// from M memory.
    pub fn m_src_functional(self) -> bool {
        self.field(31, 1) != 0
    }

    /// `IR<30:26>` --- M memory address, or functional source number.
    pub fn m_src(self) -> u8 {
        self.field(26, 5) as u8
    }

    /// `IR<11:10>` --- meaning depends on the class.
    pub fn misc(self) -> u8 {
        self.field(10, 2) as u8
    }

    /// The ALU class's fields, as the `ALU` row of `mit/cadr/ir.bits`
    /// lays them out; each field's `IR<hi:lo>` is on the struct's field.
    pub fn alu(self) -> Alu {
        Alu {
            dest: Dest(self.field(14, 12) as u16),
            ob_select: self.field(12, 2) as u8,
            func: self.field(3, 6) as u8,
            carry_in: self.field(2, 1) != 0,
            q_control: self.field(0, 2) as u8,
        }
    }

    /// The JUMP class's fields, as the `JUMP` row of `mit/cadr/ir.bits`
    /// lays them out; each field's `IR<hi:lo>` is on the struct's field.
    pub fn jump(self) -> Jump {
        Jump {
            target: self.field(12, 14) as u16,
            r: self.field(9, 1) != 0,
            p: self.field(8, 1) != 0,
            n: self.field(7, 1) != 0,
            invert: self.field(6, 1) != 0,
            internal_cond: self.field(5, 1) != 0,
            cond: self.field(0, 3) as u8,
            rotate: self.field(0, 5) as u8,
        }
    }

    /// The DISPATCH class's fields, as the `DISP` row of `mit/cadr/ir.bits`
    /// lays them out; each field's `IR<hi:lo>` is on the struct's field.
    pub fn dispatch(self) -> Dispatch {
        Dispatch {
            constant: self.field(32, 10) as u16,
            use_lpc: self.field(25, 1) != 0,
            advance_lc: self.field(24, 1) != 0,
            addr: self.field(12, 11) as u16,
            map: self.field(8, 2) as u8,
            len: self.field(5, 3) as u8,
            rotate: self.field(0, 5) as u8,
        }
    }

    /// The BYTE class's fields, as the `BYTE` row of `mit/cadr/ir.bits`
    /// lays them out; each field's `IR<hi:lo>` is on the struct's field.
    pub fn byte(self) -> Byte {
        Byte {
            dest: Dest(self.field(14, 12) as u16),
            func: self.field(12, 2) as u8,
            width_minus_1: self.field(5, 5) as u8,
            rotate: self.field(0, 5) as u8,
        }
    }
}

impl std::fmt::Debug for Insn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:016o}", self.0)
    }
}

/// `IR<25:14>` --- where an ALU or BYTE result is written.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Dest(u16);

impl Dest {
    pub fn raw(self) -> u16 {
        self.0
    }

    /// `IR<25>` --- write to A memory rather than to a functional destination.
    pub fn is_a_mem(self) -> bool {
        self.0 & 0o4000 != 0
    }

    /// `IR<23:14>` --- A memory address, valid when [`Dest::is_a_mem`].
    /// `IR<24>` is not an address bit: `ir.bits` marks it "xx", and on page
    /// ACTL the 25S09s at 3B28 and 3B29 make the write address from
    /// `IR<23:14>` alone.
    pub fn a_addr(self) -> u16 {
        self.0 & 0o1777
    }

    /// `IR<18:14>` --- M memory address.  A functional-destination write also
    /// lands in M memory (and the shadowed low 32 words of A memory).
    pub fn m_addr(self) -> u8 {
        (self.0 & 0o37) as u8
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Alu {
    /// `IR<25:14>`; see [`Dest`].
    pub dest: Dest,
    /// `IR<13:12>` --- selects ALU output, shifted right, or shifted left.
    pub ob_select: u8,
    /// `IR<8:3>` --- see the ALU FUNCTIONS table in `mit/cadr/ir.bits`.
    pub func: u8,
    /// `IR<2>`
    pub carry_in: bool,
    /// `IR<1:0>`
    pub q_control: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Jump {
    /// `IR<25:12>`
    pub target: u16,
    /// `IR<9>` --- return: pop the target off the microcode stack.
    pub r: bool,
    /// `IR<8>` --- push the return address.  `p && r` on a JUMP means "write
    /// the control store" rather than jumping, but the push is **not**
    /// special-cased on the board: the 74S64 at CONTRL 3E26 makes `-SPUSH`
    /// with no `IWRITE` in any of its four AND groups, so a write pushes
    /// like any other call and the registered `IWRITED` pops it on the next
    /// cycle.  The pointer ends where it began with the pushed word still
    /// above it.
    pub p: bool,
    /// `IR<7>` --- inhibit the next instruction.
    pub n: bool,
    /// `IR<6>` --- invert the sense of the condition.
    pub invert: bool,
    /// `IR<5>` --- use an internal condition rather than a bit of the M source.
    pub internal_cond: bool,
    /// `IR<2:0>` --- internal condition number.  Three bits: on page FLAG
    /// the 74S08 at 3E14 ANDs `IR0`, `IR1` and `IR2` with `IR5` for the
    /// condition mux's select, and `IR3` and `IR4` reach no part on the
    /// page; `ir.bits` draws `COND` over all five columns of the rotate.
    pub cond: u8,
    /// `IR<4:0>` --- M source rotate, when testing a bit.  With `IR<11:10>`
    /// = 3 the location counter's byte select takes the place of `IR<4:3>`,
    /// as it does for every class (page SMCTL).
    pub rotate: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Dispatch {
    /// `IR<41:32>`
    pub constant: u16,
    /// `IR<25>` --- push `LPC` rather than `PC`.
    pub use_lpc: bool,
    /// `IR<24>` --- step the instruction-sequence hardware.
    pub advance_lc: bool,
    /// `IR<22:12>`
    pub addr: u16,
    /// `IR<9:8>` --- OR in level-2 map bits.
    pub map: u8,
    /// `IR<7:5>`
    pub len: u8,
    /// `IR<4:0>`
    pub rotate: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Byte {
    /// `IR<25:14>`; see [`Dest`].
    pub dest: Dest,
    /// `IR<13:12>` --- 1 = LDB, 2 = selective deposit, 3 = DPB, and 0 a
    /// deposit that does not rotate.  On page SMCTL `IR<13>` is `MR`, which
    /// rotates the mask, and `IR<12>` is `SR`, which rotates the source; the
    /// names are MIT's from `ir.bits`.
    pub func: u8,
    /// `IR<9:5>`
    pub width_minus_1: u8,
    /// `IR<4:0>`
    pub rotate: u8,
}

/// Hand-assembled microinstructions.
///
/// Each item names the field it writes as `IR<hi:lo>`; the positions are the
/// ones the accessors above read, from `mit/cadr/ir.bits`.  These are the
/// encodings `src/benchmark.rs`, `tests/chip.rs` and `tests/lashup.rs` assemble
/// their programs with, kept here so there is one copy.
pub mod asm {
    use super::Insn;

    /// `IR<13:12>` = 1 --- the ALU output goes to the destination.
    pub const ALU: u64 = 1 << 12;
    /// `IR<44:43>` = 1 --- the JUMP class.
    pub const JUMP: u64 = 1 << 43;

    /// `IR<8:3>` --- ALU function 3, `SETM`, from `ir.bits`.
    pub const SETM: u64 = 3 << 3;
    /// `IR<8:3>` --- ALU function 5, `SETA`.
    pub const SETA: u64 = 5 << 3;
    /// `IR<8:3>` --- ALU function 0, `SETZ`.
    pub const SETZ: u64 = 0;
    /// `IR<8:3>` --- ALU function 31, `ADD`: M plus A, plus [`CARRY_IN`].
    pub const ADD: u64 = 0o31 << 3;
    /// `IR<2>` --- the ALU's carry in.
    pub const CARRY_IN: u64 = 1 << 2;

    /// The rest of `ir.bits`' ALU FUNCTIONS table, `IR<8:3>` in the octal
    /// the table gives.  The names are MIT's; the operations are the
    /// 74S181's, with the M source as the array's A input and the A source
    /// as its B, which is the way round `SETM` and `SETA` fix.
    pub const AND: u64 = 0o1 << 3;
    /// M and not A.
    pub const ANDCA: u64 = 0o2 << 3;
    /// Not M and A.
    pub const ANDCM: u64 = 0o4 << 3;
    pub const XOR: u64 = 0o6 << 3;
    /// Inclusive or.
    pub const IOR: u64 = 0o7 << 3;
    /// Neither: not (M or A).
    pub const ANDCB: u64 = 0o10 << 3;
    /// Not (M xor A).
    pub const EQV: u64 = 0o11 << 3;
    /// Not A.
    pub const SETCA: u64 = 0o12 << 3;
    /// M or not A.
    pub const ORCA: u64 = 0o13 << 3;
    /// Not M.
    pub const SETCM: u64 = 0o14 << 3;
    /// Not M or A.
    pub const ORCM: u64 = 0o15 << 3;
    /// Not (M and A).
    pub const ORCB: u64 = 0o16 << 3;
    /// All ones.
    pub const SETO: u64 = 0o17 << 3;
    /// M minus A, minus one, plus [`CARRY_IN`] --- which is the same
    /// subtraction the jump conditions are computed with.
    pub const SUB: u64 = 0o26 << 3;
    /// M plus M, plus [`CARRY_IN`].
    pub const M_PLUS_M: u64 = 0o37 << 3;
    /// M plus [`CARRY_IN`], the A source ignored.
    pub const M_PLUS_C: u64 = 0o34 << 3;

    /// Two more values of `IR<13:12>`, the output bus select, of which
    /// [`ALU`] is the second: the ALU output shifted right, and shifted
    /// left.  The right shift takes bit 32 of the 33-bit array in at the top
    /// and the left shift takes `Q<31>` in at the bottom, so a left shift on
    /// an instruction that also drives [`Q_LEFT`] is the multiply step's
    /// shift of the double-length product.
    ///
    /// The fourth value, zero, is not the ALU at all but the mask and rotate
    /// network's output, the same one the BYTE class drives, with the mask
    /// taken from `IR<9:5>` and `IR<4:0>` --- which in this class are the ALU
    /// function and the rotate.  It has no name here because nothing
    /// assembles it.
    pub const OB_RIGHT: u64 = 2 << 12;
    pub const OB_LEFT: u64 = 3 << 12;

    /// `IR<1:0>` --- what the Q register does this microcycle: shift left
    /// taking the complement of the ALU's sign in, shift right taking the
    /// ALU's low bit in, or load the ALU output.  Zero leaves it alone.
    ///
    /// `IR<1:0>` is the low end of the rotate field, so an ALU-class
    /// instruction driving the mask and rotate network (output bus select
    /// 0) by anything but a multiple of four drives Q as well.
    pub const Q_LEFT: u64 = 1;
    pub const Q_RIGHT: u64 = 2;
    pub const Q_LOAD: u64 = 3;

    /// `IR<42>` --- pop the microcode stack after this instruction, whatever
    /// its class: `mit/cadr/ir.bits` spells `POPJ` down the column for bit
    /// 42, one letter in each class's row.
    pub const POPJ: u64 = 1 << 42;

    /// `IR<44:43>` = 2 and = 3 --- the DISPATCH and BYTE classes.
    pub const DISPATCH: u64 = 2 << 43;
    pub const BYTE: u64 = 3 << 43;

    /// `IR<13:12>` on a BYTE --- load a byte, deposit into the A source
    /// without rotating, deposit the rotated M source.
    pub const LDB: u64 = 1 << 12;
    pub const DEP: u64 = 2 << 12;
    pub const DPB: u64 = 3 << 12;

    /// `IR<11:10>` = 2 on a DISPATCH --- write the dispatch memory from the
    /// A source instead of dispatching.
    pub const DMEM_WRITE: u64 = 2 << 10;

    /// `IR<5>` with `IR<2:0>` = 3 --- the internal condition `AEQM`, the A
    /// source equal to the M source.
    pub const AEQM: u64 = (1 << 5) | 3;
    /// `IR<46>` --- the statistics bit.
    pub const STAT: u64 = 1 << 46;

    /// `IR<31>` set with `IR<30:26>` --- read the M source from functional
    /// source `n` rather than from M memory.
    pub const fn src(n: u64) -> u64 {
        (1 << 31) | (n << 26)
    }
    /// `IR<31>`, `IR<29>`, `IR<27>` --- functional source 12, `MD`.
    pub const SRC_MD: u64 = src(0o12);
    /// Functional source 7, `Q`.
    pub const SRC_Q: u64 = src(0o7);
    /// Functional destinations, `IR<23:19>` with `IR<25>` clear --- page
    /// SOURCE decodes `IR<23>`, `IR<22>` and `IR<21:19>`, `ir.bits` drawing
    /// the field over bits 24 to 14; the write also lands in M memory 37.
    pub const START_READ: u64 = (0o21 << 19) | (0o37 << 14);
    pub const START_WRITE: u64 = (0o22 << 19) | (0o37 << 14);
    pub const MD: u64 = (0o30 << 19) | (0o37 << 14);
    /// Functional destination 20, `VMA`, likewise.
    pub const VMA: u64 = (0o20 << 19) | (0o37 << 14);

    /// `IR<9>` on a JUMP --- return: the target is popped off the stack.
    pub const R: u64 = 1 << 9;
    /// `IR<8>` on a JUMP --- push the return address.
    pub const P: u64 = 1 << 8;
    /// `IR<7>` on a JUMP --- inhibit the next instruction when taken.
    pub const N: u64 = 1 << 7;
    /// `IR<6>` on a JUMP --- invert the condition.
    pub const INVERT: u64 = 1 << 6;
    /// `IR<5>` with `IR<2:0>` = 7 --- the internal condition that is
    /// always true.
    pub const ALWAYS: u64 = (1 << 5) | 7;

    /// `IR<41:32>` --- the A memory address read.
    pub fn a_src(a: u64) -> u64 {
        a << 32
    }
    /// `IR<30:26>` --- the M memory address read.
    pub fn m_src(m: u64) -> u64 {
        m << 26
    }
    /// `IR<25>` set, `IR<23:14>` --- the A memory address written.
    pub fn a_dest(a: u64) -> u64 {
        (1 << 25) | (a << 14)
    }
    /// `IR<25>` clear, `IR<23:19>` = 0 (nowhere), `IR<18:14>` --- the M
    /// memory address written, and with it the shadowing A memory word.
    pub fn m_dest(m: u64) -> u64 {
        m << 14
    }
    /// `IR<25:12>` on a JUMP --- the target.
    pub fn target(pc: u64) -> u64 {
        pc << 12
    }
    /// `IR<5>` clear, `IR<4:0>` --- test bit `bit` of the M source: the
    /// source is rotated left by `IR<4:0>` and bit 0 of the result is the
    /// condition, so bit `b` lands there under a rotate of `32 - b`.
    pub fn bit(bit: u64) -> u64 {
        (32 - bit) & 0o37
    }
    /// `IR<4:0>` --- how far left the M source is rotated, for the mask and
    /// rotate network under output bus select 0 and for the BYTE class's
    /// byte position.
    pub fn rot(n: u64) -> u64 {
        n & 0o37
    }
    /// `IR<9:5>` on a BYTE --- the field is `n` bits wide, the hardware
    /// taking one less than that.
    pub fn width(n: u64) -> u64 {
        (n - 1) << 5
    }
    /// `IR<22:12>` on a DISPATCH --- the dispatch memory address, which the
    /// rotated M source's low [`d_len`] bits are ored into.
    pub fn d_addr(a: u64) -> u64 {
        a << 12
    }
    /// `IR<7:5>` on a DISPATCH --- how many bits of the M source join the
    /// address.
    pub fn d_len(n: u64) -> u64 {
        n << 5
    }

    /// `IR<60>` --- revision 15's `oa-low-select` (A15b.2).
    pub const OA_LOW_SELECT: u64 = 1 << 60;
    /// `IR<61>` --- revision 15's `oa-high-select` (A15b.2).
    pub const OA_HIGH_SELECT: u64 = 1 << 61;
    /// `IR<48>` on a JUMP --- revision 15's hint bit (A15b.2).
    pub const HINT: u64 = 1 << 48;

    /// `IR<58:48>` on an ALU or BYTE word --- revision 15's PDL address
    /// field (A15b.2): E `IR<48>`, the base `IR<50:49>`, the displacement
    /// `IR<58:51>`.
    pub fn pdl_field(base: u64, displacement: i8) -> u64 {
        1 << 48 | (base & 3) << 49 | (displacement as u8 as u64) << 51
    }

    /// `IR<61:48>`, `IR<62>`, `IR<63>` on a DISPATCH --- revision 15's
    /// predicted target (A15b.2): the address, and P and R stored inverted.
    pub fn predicted(addr: u64, p: bool, r: bool) -> u64 {
        (addr & 0o37777) << 48 | (!p as u64) << 62 | (!r as u64) << 63
    }

    /// `((A-MEM 100) SETA A-MEM-3)`: puts A memory 3 on the A bus and the
    /// OB, and writes it somewhere harmless.  Every filler instruction is
    /// this one, so that the A bus, the OB and `IR<47:32>` read the same
    /// whatever instruction is standing when a console looks.
    pub fn filler() -> Insn {
        Insn::new(ALU | SETA | a_src(3) | a_dest(0o100))
    }
}
