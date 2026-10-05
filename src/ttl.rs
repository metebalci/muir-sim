// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The 74S181 ALU slice and its carry-lookahead, as behavior.
//!
//! The CADR ALU is nine 74S181s and three 74S182s: eight
//! slices covering `alu<31:0>`, plus a ninth fed with `m[31]` and `a[31]`
//! again, which sign-extends both operands to 33 bits.  That is why the JUMP
//! comparisons are signed.
//!
//! Here the whole 33-bit array is computed at once, from the datasheet's
//! active-high function table (Table 2 --- CADR uses active-high data).  The
//! `chip` engine uses a gate-level 74S181 instead, `s181` in `src/part.rs`;
//! that is the point of having both, and `tests/behaviour.rs` holds the two
//! to each other.

/// 33 bits of ones.
const M33: u64 = (1 << 33) - 1;

/// The result of the CADR's 33-bit ALU array.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Alu {
    /// `alu<32:0>`.  Bit 32 is the sign of the 33-bit result.
    pub f: u64,
    /// The open-collector A=B chain across the low eight slices.
    pub aeqm: bool,
}

/// `m` and `a` are the ALU's A and B inputs respectively --- note that
/// the drawings wire the M bus to `A` and the A bus to `B`.
pub fn alu(m: u32, a: u32, aluf: u8, alumode: bool, cin: bool) -> Alu {
    // Sign-extend to 33 bits, which is what the ninth slice does.
    let x = ((m as u64) | (((m >> 31) as u64) << 32)) & M33;
    let y = ((a as u64) | (((a >> 31) as u64) << 32)) & M33;

    let f = if alumode {
        // Logic, M = H.
        match aluf & 0xf {
            0x0 => !x,
            0x1 => !(x | y),
            0x2 => !x & y,
            0x3 => 0,
            0x4 => !(x & y),
            0x5 => !y,
            0x6 => x ^ y,
            0x7 => x & !y,
            0x8 => !x | y,
            0x9 => !(x ^ y),
            0xa => y,
            0xb => x & y,
            0xc => !0,
            0xd => x | !y,
            0xe => x | y,
            _ => x,
        }
    } else {
        // Arithmetic, M = L.  Every function is `p + q + Cn` for some p, q
        // drawn from A and B; see the datasheet table.
        let (p, q) = addends(aluf, x, y);
        p.wrapping_add(q).wrapping_add(cin as u64)
    } & M33;

    // Each slice pulls AEB low unless its four result bits are all ones; the
    // eight slices are wired together open-collector.  In subtract mode that
    // is exactly A = B.
    Alu { f, aeqm: (f & 0xffff_ffff) == 0xffff_ffff }
}

/// The two addends of an arithmetic function, `F = p + q + Cn`, on the
/// sign-extended 33-bit operands: the 74S181's active-high table.
fn addends(aluf: u8, x: u64, y: u64) -> (u64, u64) {
    let (p, q) = match aluf & 0xf {
        0x0 => (x, 0),
        0x1 => (x | y, 0),
        0x2 => (x | !y, 0),
        0x3 => (M33, 0),
        0x4 => (x, x & !y),
        0x5 => (x | y, x & !y),
        0x6 => (x, !y),
        0x7 => (x & !y, M33),
        0x8 => (x, x & y),
        0x9 => (x, y),
        0xa => (x | !y, x & y),
        0xb => (x & y, M33),
        0xc => (x, x),
        0xd => (x | y, x),
        0xe => (x | !y, x),
        _ => (x, M33),
    };
    (p & M33, q & M33)
}

/// **Revision 14's LC adder** (contract G3 revision 14, appendix A14.11):
/// `LC<33:32>` as a write of the location counter by an arithmetic ALU
/// function takes them. `m` is the whole M word, `a` the A operand's
/// `<31:0>`; the array computes `F = p + q + Cn` on 33 bits, `q32` is bit
/// 32 of its second addend (q's sign) and `c32 = F32 ^ p32 ^ q32` the carry
/// into bit 32, and `E = (M<33:32> + 3 q32 + c32) mod 4`: M's 34 bits plus
/// or minus A's 32, sign-extended. Output select 1, the ALU, gives `E`;
/// select 3, the left shift, `{E<0>, F<31>}`, the 34-bit sum shifted as the
/// bus's `<31:0>` are `{F<30:0>, Q<31>}`.
pub fn lc_high(m: u64, a: u32, aluf: u8, cin: bool, left_shift: bool) -> u64 {
    let ext = |v: u32| ((v as u64) | (((v >> 31) as u64) << 32)) & M33;
    let (p, q) = addends(aluf, ext(m as u32), ext(a));
    let f = p.wrapping_add(q).wrapping_add(cin as u64) & M33;
    let bit32 = |v: u64| (v >> 32) & 1;
    let c32 = bit32(f) ^ bit32(p) ^ bit32(q);
    let e = ((m >> 32) & 3).wrapping_add(3 * bit32(q)).wrapping_add(c32) & 3;
    if left_shift { (e & 1) << 1 | ((f >> 31) & 1) } else { e }
}

/// `<39:32>` of a 40-bit word's ALU output, in place above bit 31
/// (contract G2 §2.2): a logical function acts on them as on every other
/// bit, the 74S181's table in logic mode, and an arithmetic one leaves M's,
/// the array's arithmetic being the CADR's 33 bits over `<31:0>`. `m` and
/// `a` are the whole words; a 32-bit word has nothing there and gives 0.
pub fn alu_tag(m: u64, a: u64, aluf: u8, alumode: bool) -> u64 {
    const TAG: u64 = 0xff << 32;
    if alumode {
        // The same table, slice by slice: the tag's bits are the low eight
        // of the logic function of the tags.
        let f = alu((m >> 32) as u32 & 0xff, (a >> 32) as u32 & 0xff, aluf, true, false).f;
        (f & 0xff) << 32
    } else {
        m & TAG
    }
}

/// What page ALUC4 puts on the ALU's control pins.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Control {
    /// `ALUF<3:0>`, the 74S181 function select.
    pub aluf: u8,
    /// `ALUMODE`, high for logic and low for arithmetic.
    pub alumode: bool,
    /// The carry into the low slice, active high.  The board's net is
    /// `-CIN0`, so this is its complement.
    pub cin: bool,
    /// `ALUSUB` and `ALUADD`, the two nets that select which column of the
    /// table above is in force.
    pub alusub: bool,
    pub aluadd: bool,
}

/// The ALU control multiplexers on page ALUC4, read off the netlist.
///
/// Three 74S153s --- 2B16, 2B17 and 2B18 --- select on `{ALUSUB, ALUADD}`.
/// Their wiring is the table below; `tests/alu.rs` checks it by evaluating
/// the three parts themselves out of `data/CADR.netlist`, so this is the
/// drawings' answer rather than one taken from another emulator:
///
/// | select | `ALUF3` | `ALUF2` | `ALUF1` | `ALUF0` | `ALUMODE` | carry in |
/// |---|---|---|---|---|---|---|
/// | 00 normal | `IR3` | `IR4` | `!IR6` | `!IR5` | `!IR7` | `IR2` |
/// | 01 add | 1 | 0 | 0 | 1 | 0 | 0 |
/// | 10 subtract | 0 | 1 | 1 | 0 | 0 | `!IRJUMP` |
/// | 11 both | 1 | 1 | 1 | 1 | 1 | 1 |
///
/// `ALUADD` and `ALUSUB` are read off the netlist too, from six parts:
///
/// | part | type | what it makes |
/// |---|---|---|
/// | SOURCE 3D02 | 74S00 | `-SPECALU` = `IR8 NAND IRALU` |
/// | SOURCE 3D04 | 74S139 | `-MUL`, `-DIV`, decoding `IR<4:3>` under `-SPECALU` |
/// | ALUC4 2C10 | 74S02 | `-DIVPOSLASTTIME`, `DIVSUBCOND`, `DIVADDCOND` |
/// | ALUC4 2D15 | 74S32O | `-MULNOP` = `-MUL OR Q0` |
/// | ALUC4 2C15 | 74S00 | the four `DIV*COND` and `A31` products |
/// | ALUC4 2C20 | 74S20O | `ALUADD` and `ALUSUB` |
///
/// The 74S139's `y2` and `y3` outputs are **unconnected**, so an instruction
/// with `IR8` and `IR4` both set asserts neither `MUL` nor `DIV`. Asserting
/// `DIV` there is the natural guess and is not what the '139 does. Only
/// undefined special-ALU opcodes reach it --- all four defined ones, `MUS`,
/// `DVS`, `DVS1` and `DVREM`, have `IR4` clear.
pub fn alu_control(ir: u64, q0: bool, a31: bool, iralu: bool, irjump: bool) -> Control {
    let b = |n: u32| ir >> n & 1 != 0;

    // SOURCE 3D02 and 3D04: IR<8> picks the multiply and divide steps out of
    // the ALU class, and IR<4:3> says which.
    let specalu = b(8) && iralu;
    let sel = (b(4) as u8) << 1 | b(3) as u8;
    let mul = specalu && sel == 0;
    let div = specalu && sel == 1;

    // ALUC4 2C10 and 2D15.
    let divpos = q0 || b(6);
    let divsub = div && divpos;
    let divadd = div && (b(5) || !divpos);
    let mulnop = mul && !q0;

    // ALUC4 2C15 into 2C20.
    let aluadd = (divadd && !a31) || (divsub && a31) || mul;
    let alusub = mulnop || (divsub && !a31) || (divadd && a31) || irjump;

    let (aluf, alumode, cin) = match (alusub, aluadd) {
        (false, false) => {
            ((b(3) as u8) << 3 | (b(4) as u8) << 2 | (!b(6) as u8) << 1 | !b(5) as u8, !b(7), b(2))
        }
        (false, true) => (0b1001, false, false),
        (true, false) => (0b0110, false, !irjump),
        (true, true) => (0b1111, true, true),
    };
    Control { aluf, alumode, cin, alusub, aluadd }
}
