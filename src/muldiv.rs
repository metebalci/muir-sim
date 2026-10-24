// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's one-instruction multiply and divide: ALU functions 42 and 43.
//!
//! The CADR multiplies and divides a bit a microinstruction. `ir.bits`' ALU
//! FUNCTIONS table has `MUS` at 40, `DVS` at 41, `DVS1` at 51 and `DVREM` at
//! 45, and `uc-arith.lisp`'s `MPY` runs 32 multiply steps and its `DIV` a
//! first step and 32 more. On page SOURCE the 74S139 at 3D04 decodes
//! `IR<4:3>` under `-SPECALU` into `-MUL` and `-DIV` and leaves its outputs
//! 2 and 3 unconnected, so on the CADR functions 42 and 43 are the ordinary
//! 74S181 functions their bits select ([`crate::ttl::alu_control`]).
//!
//! QUUX gives those two codes a unit of their own:
//!
//! - **`MUL`, 42**: what 32 multiply steps leave, each step's M operand the
//!   output bus of the step before and the first the instruction's M
//!   source, the A source the multiplicand throughout, `Q` the multiplier.
//!   The output bus carries the high word and `Q` is loaded with the low.
//!   Done in the microcycle, like any ALU function.
//! - **`DIV`, 43**: what a first divide step and 31 divide steps leave, each
//!   shifting the output bus and `Q` left as `DIVIDE-STEP` does, the M
//!   source the high dividend and the partial remainder, `Q` the low
//!   dividend and the quotient, the A source the divisor. `DIVIDE-LAST-STEP`
//!   and `DVREM` stay separate instructions. The first step's quotient bit,
//!   which `DIV` tests for overflow straight after `DVS1`, ends in `Q<31>`.
//!
//! Both decode on `IR<8>` and `IR<4:3>` as the '139 does, `IR<7:5>` not
//! looked at; both drive the output bus and load `Q` whatever the output
//! selector `IR<13:12>` and the Q control `IR<1:0>` say.
//!
//! Each is defined as the steps, and computed by running them: the step here
//! is [`crate::ttl::alu_control`] and [`crate::ttl::alu`], the model both
//! engines run a CADR step on and that `tests/muldiv.rs` holds to the
//! netlist, and the shifts are the output bus's and `Q`'s. So one
//! instruction is bit for bit the sequence it replaces, for any operands.

use crate::ttl;

/// One of QUUX's two.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Mul,
    Div,
}

/// `IR<8:3>` of `MUS`, `DVS` and `DVS1` in `ir.bits`' ALU FUNCTIONS table.
const MUS: u64 = 0o40 << 3;
const DVS: u64 = 0o41 << 3;
const DVS1: u64 = 0o51 << 3;

/// **Revision 15's divider and multiplier, in clocks** (contract G3
/// revision 15, A15b.3's row DIV, MUL; its MP2b ruling Q3): a `DIV` stays in
/// EX 18 clocks from the clock its operands are ready, 17 of them held, and
/// a `MUL` 5, 4 held, at every period. The counts are the stage fit's core
/// (M8e), whose divider is the fabric's: two quotient steps a clock, the
/// words there 17 clocks after the load. A register operand is ready at
/// once; an operand from `MD` when the wait for `MD` ends.
pub const DIV_CLOCKS_15: u64 = 18;
pub const MUL_CLOCKS_15: u64 = 5;

/// Which of the two `ir` asks for, on a machine that has them: an ALU-class
/// instruction, `IR<44:43>` 0, with `IR<8>` set and `IR<4:3>` 2 or 3.
pub fn decode(ir: u64) -> Option<Op> {
    let specalu = ir >> 43 & 3 == 0 && ir >> 8 & 1 != 0;
    match (specalu, ir >> 3 & 3) {
        (true, 2) => Some(Op::Mul),
        (true, 3) => Some(Op::Div),
        _ => None,
    }
}

/// The output bus and `Q` after `op` on M source `m`, A source `a` and `Q`
/// `q`.
pub fn run(op: Op, m: u32, a: u32, q: u32) -> (u32, u32) {
    match op {
        Op::Mul => (0..32).fold((m, q), |(m, q), _| mul_step(m, a, q)),
        Op::Div => (0..31).fold(div_step(DVS1, m, a, q), |(m, q), _| div_step(DVS, m, a, q)),
    }
}

/// `MULTIPLY-STEP`: `MUS`, the output bus shifted right with bit 32 of the
/// array in at the top, `Q` shifted right with the ALU's bit 0 in at the top.
fn mul_step(m: u32, a: u32, q: u32) -> (u32, u32) {
    let alu = step(MUS, m, a, q);
    ((alu >> 1) as u32, ((alu as u32 & 1) << 31) | (q >> 1))
}

/// `DIVIDE-FIRST-STEP` or `DIVIDE-STEP`: the output bus shifted left with
/// `Q<31>` in at the bottom, `Q` shifted left with the complement of the
/// ALU's sign in at the bottom.
fn div_step(code: u64, m: u32, a: u32, q: u32) -> (u32, u32) {
    let alu = step(code, m, a, q) as u32;
    ((alu << 1) | (q >> 31), (q << 1) | (!alu >> 31))
}

/// The 33-bit array's output on one step: page ALUC4's decision from `Q<0>`
/// and the A source's sign, then the 74S181s.
fn step(code: u64, m: u32, a: u32, q: u32) -> u64 {
    let ctl = ttl::alu_control(code, q & 1 != 0, a >> 31 != 0, true, false);
    ttl::alu(m, a, ctl.aluf, ctl.alumode, ctl.cin).f
}
