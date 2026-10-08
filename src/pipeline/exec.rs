// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **EX's microcycle**: what a word does, as `micro` does it
//! ([`crate::micro`], the specification), with the pipeline's operands ---
//! A and M from RD's reads and the bypasses, the PDL buffer's word from its
//! read at CS --- its A, M and PDL buffer writes left for WB, and its memory
//! starts left for WB and the port. The rest is `micro`'s line for line
//! for revisions 14 and 15: the functional sources and destinations, the
//! ALU, BYTE, the conditions, JUMP and DISPATCH, the micro stack, LC and
//! its fetch, the fused return and D, the map's operations, and the
//! microcycle's end.

use super::{AmWrite, PdlAt, Pipeline, Slot, field};
use crate::isa::{Insn, Op};
use crate::machine::{Halt, Word};
use crate::muldiv;
use crate::ttl;

/// A word's `<31:0>`.
const LOW: Word = 0xffff_ffff;

/// A memory start a microcycle made, for WB (A15b.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Start {
    pub write: bool,
    /// `VMA` as the start had it.
    pub va: u32,
    /// The microcycle that made it.
    pub mc: u64,
    /// An instruction fetch's (`IFETCH`).
    pub fetch: bool,
    /// A write's word, once the microcycle after the start has fixed it
    /// (`docs/quux.md`'s single-edge contract: the `MD` of the microcycle
    /// after the start).
    pub word: Option<Word>,
}

/// Where WB is with its word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WbState {
    /// Not begun.
    Fresh,
    /// Done: the word leaves at the clock's end.
    Done,
}

/// `micro`'s state between microcycles that the executor keeps
/// ([`crate::micro::Micro`]'s fields of the same names).
#[derive(Clone, Debug, Default)]
pub(crate) struct Exec {
    pub next_instr: bool,
    pub next_instrd: bool,
    /// OA-REG-LOW and OA-REG-HIGH, `IR<25:0>` and `IR<47:26>`'s
    /// positions less 26.
    pub oa_low: u64,
    pub oa_high: u64,
    /// Revision 14's IMOD: the registers ORed into the next microcycle's
    /// word.
    pub imod: [bool; 2],
    pub map_write: Option<(Word, Word)>,
    pub map_write_d: Option<(Word, Word)>,
    pub map_seen: Option<crate::machine::Translation>,
    pub lvmo: u32,
    pub wrcyc: bool,
    pub spc_write: Option<(u8, u32)>,
    pub operand: Option<crate::machine::Operand>,
    pub d_m31: Option<Word>,
    pub opc: [u16; 8],
    pub halted: bool,
    /// A write started in the microcycle before, whose word this one fixes:
    /// the start's sequence and `MD` as that microcycle left it.
    pub write_pending: Option<(u64, Word)>,
    /// A write this microcycle starts.
    pub write_new: bool,
    /// The interrupt as a planted mutation samples it off the clock's own.
    pub interrupt_sample: Option<bool>,
    // Set and used within one microcycle, as `micro`'s.
    pub pushed: bool,
    pub spc_pushed: bool,
    pub spc_popped: bool,
    pub macro_write: Option<(u32, u32)>,
    pub lc_adder: Option<u64>,
    pub popj: bool,
    pub inhibit: bool,
    /// The address of the word two after this one, as the microcycle leaves
    /// it: `micro`'s `npc`.
    pub npc: u16,
    /// `micro`'s `npc` as the microcycle before left it: the address of
    /// this microcycle's next word, whatever transferred to it. Kept
    /// between microcycles, and in a checkpoint.
    pub npc_prev: u16,
    /// This microcycle's next word's address plus one: its `npc` when
    /// nothing transfers.
    pub npc_seq: u16,
    pub iwr: u64,
    pub ir: u64,
    pub pc: u16,
    pub adata: Word,
    pub mdata: Word,
    pub maddr: u8,
    pub out: Word,
    /// This microcycle's starts.
    pub starts: Vec<Start>,
    /// This microcycle's writes for WB.
    pub am: Vec<AmWrite>,
    pub pdl_w: Option<(PdlAt, Word)>,
    pub seq: u64,
    pub mc: u64,
    /// The PDL buffer's word the word read, from its slot.
    pub pdl_val: Word,
    /// The microcycle wrote the control store: the words behind it are
    /// fetched again.
    pub wrote_imem: bool,
    /// D or a fused return on a fetched word: the word.
    pub d_fused: bool,
    /// What the microcycle decided, for EX's check of RD's prediction: a
    /// conditional jump's condition, a dispatch's entry's P and R.
    pub taken: bool,
    pub entry_pr: (bool, bool),
}

/// Revision 13's rotator, a ring of 40.
fn rol40(v: Word, n: u32) -> Word {
    const RING: Word = (1 << 40) - 1;
    let v = v & RING;
    match n % 40 {
        0 => v,
        k => (v << k | v >> (40 - k)) & RING,
    }
}

/// Revision 13's masker.
fn mask40(right: u32, n: u32) -> Word {
    if right + n > 39 { 0 } else { ((1 << (n + 1)) - 1) << right }
}

/// **The fields the OA selects reach** (A15b.15), `micro`'s `oa_fields`.
pub(crate) fn oa_fields(ir: u64) -> [u64; 2] {
    let insn = Insn::new(ir);
    let dest = if ir >> 25 & 1 != 0 { 0o1777 << 14 } else { 0o37 << 14 };
    let source = 0o1777 << 32 | if ir >> 31 & 1 == 0 { 0o37 << 26 } else { 0 };
    match insn.op() {
        Op::Alu => [dest | 0o17 << 3, source],
        Op::Byte => [dest | 0o7777, source],
        Op::Jump => [0o37777 << 12, source],
        Op::Dispatch if insn.misc() == 2 => [0o7777 << 12, 0],
        Op::Dispatch => [0, 0],
    }
}

impl Pipeline {
    fn ir(&self, pos: u32, len: u32) -> u32 {
        field(self.x.ir, pos, len)
    }

    /// LC byte mode's rotation, revision 13's (`micro`'s `lc_rotation`).
    pub(crate) fn lc_rotation_at(byte_mode: bool, lc: u64, rotate: u32) -> u32 {
        let lc = lc as u32;
        let add = if byte_mode {
            [16, 0, 32, 24][(lc & 3) as usize]
        } else if lc & 2 != 0 {
            0
        } else {
            24
        };
        (rotate + add) % 40
    }

    fn lc_rotation(&self, lc: u64, rotate: u32) -> u32 {
        Self::lc_rotation_at(self.m.byte_mode(), lc, rotate)
    }

    fn needfetch(&self) -> bool {
        self.m.lc & self.m.geometry.need_fetch() != 0
    }

    fn pops_by_source(&self) -> bool {
        self.x.ir >> 31 & 1 != 0 && self.x.maddr & 0o17 == 0o14
    }

    fn writes_m31_or_interrupt_control(&self) -> bool {
        matches!(Insn::new(self.x.ir).op(), Op::Alu | Op::Byte)
            && self.ir(25, 1) == 0
            && (self.ir(14, 5) == 0o31 || self.ir(19, 5) == 0o2)
    }

    fn push_spc(&mut self, word: u32) {
        self.x.pushed = true;
        if self.x.spc_popped {
            self.m.spcptr = (self.m.spcptr + 1) & 0o37;
        }
        self.x.spc_pushed = true;
        self.m.spcptr = (self.m.spcptr + 1) & 0o37;
        self.x.spc_write = Some((self.m.spcptr, word));
    }

    fn pop_spc(&mut self) -> u32 {
        if self.x.spc_pushed {
            self.x.spc_popped = true;
            return self.m.spc[(self.m.spcptr.wrapping_sub(1) & 0o37) as usize];
        }
        if self.x.spc_popped {
            return self.m.spc[((self.m.spcptr + 1) & 0o37) as usize];
        }
        let ptr = self.m.spcptr;
        let v = match self.x.spc_write {
            Some((p, word)) if p == ptr => word,
            _ => self.m.spc[ptr as usize],
        };
        self.m.spcptr = ptr.wrapping_sub(1) & 0o37;
        self.x.spc_popped = true;
        v
    }

    fn ignpopj(&mut self, entry: u32) {
        if (entry >> 16) & 1 == 0 && self.x.spc_popped {
            self.m.spcptr = (self.m.spcptr + 1) & 0o37;
            self.x.spc_popped = false;
        }
    }

    /// The word D may dispatch on (`micro`'s `d_word`): main memory's word
    /// at `LC<33:2>` as the stream's fetch will read it, every write the
    /// processor made before it in it, when D is enabled and condition 6
    /// is false.
    fn d_word(&self) -> Option<Word> {
        use crate::machine::macro_dispatch::{D_ENABLE, ENABLE};
        let register = self.m.macro_dispatch.register;
        if !self.m.geometry.extended() || register & (ENABLE | D_ENABLE) != ENABLE | D_ENABLE {
            return None;
        }
        let int_enabled = self.m.interrupt_control & (1 << 27) != 0;
        let sequence_break = self.m.interrupt_control & (1 << 26) != 0;
        if !self.m.vmaok || (int_enabled && self.interrupt_now()) || sequence_break {
            return None;
        }
        let at = ((self.m.lc & self.m.geometry.lc_counter()) >> 2) as u32;
        let t = self.m.translate(at);
        let main = t.physical & crate::tlb::DEVICE == 0;
        if !(t.access_permitted && main) || t.physical as usize >= self.m.main.len() {
            return None;
        }
        Some(self.coherent(t.physical))
    }

    fn pop_asks_for_a_fetch(&mut self, word: u32) -> u32 {
        if !self.pops_by_source() {
            self.x.next_instr = true;
        }
        if self.needfetch() { word } else { word | 2 }
    }

    /// `micro`'s `main_loop_return`.
    fn main_loop_return(&mut self, word: u32, advance: bool) -> u32 {
        let fetched = if self.needfetch() { self.d_word() } else { None };
        let target = self.pop_asks_for_a_fetch(word);
        if !self.m.geometry.macro_dispatch
            || (self.needfetch() && fetched.is_none())
            || advance
            || self.x.next_instrd
            || self.x.pushed
            || self.pops_by_source()
            || self.writes_m31_or_interrupt_control()
        {
            return target;
        }
        let inc = if self.m.byte_mode() { 1 } else { 2 };
        let counter = self.m.geometry.lc_counter();
        let stepped = (self.m.lc & counter).wrapping_add(inc) & counter;
        let index_rotate = crate::machine::macro_dispatch::index_rotate(true);
        let rotate = self.lc_rotation(stepped, index_rotate);
        let m31 = fetched.unwrap_or(self.m.mmem[0o31]);
        let rotated = rol40(m31, rotate);
        match self.m.macro_dispatch.fused_return(word, rotated, index_rotate) {
            Some(f) => {
                if f.keep {
                    self.m.spcptr = (self.m.spcptr + 1) & 0o37;
                }
                self.m.macro_dispatch.fused += 1;
                self.meters.fused += 1;
                self.x.operand = f.operand;
                self.x.d_m31 = fetched;
                self.x.d_fused = fetched.is_some();
                f.handler as u32
            }
            None => target,
        }
    }

    fn jump_return(&mut self, word: u32) -> u32 {
        if crate::machine::macro_dispatch::JUMP_RETURNS_FUSE {
            self.main_loop_return(word, false)
        } else {
            self.pop_asks_for_a_fetch(word)
        }
    }

    /// `micro`'s `step_lc`: the counter steps, and a fetch starts when
    /// `NEEDFETCH` asks.
    fn step_lc(&mut self) {
        let counter = self.m.geometry.lc_counter();
        let fetch_from = ((self.m.lc & counter) >> 2) as u32;
        let inc = if self.m.byte_mode() { 1 } else { 2 };
        let lc = (self.m.lc & counter).wrapping_add(inc) & counter;
        self.m.lc = (self.m.lc & !counter) | lc;
        if self.needfetch() {
            self.m.lc &= !self.m.geometry.need_fetch();
            self.m.vma = fetch_from.into();
            self.start(false, true);
        }
        let lc0b = self.m.byte_mode() && (self.m.lc & 1 != 0);
        let last_byte_in_word = !lc0b && (self.m.lc & 2 == 0);
        if last_byte_in_word {
            self.m.lc |= self.m.geometry.need_fetch();
        }
    }

    /// A memory start: `VMA` as it stands, for WB to translate and the
    /// port to take (A15b.3). A read's `MD` is the port's to land; a write
    /// waits for its word ([`Pipeline::fix_write`]).
    fn start(&mut self, write: bool, fetch: bool) {
        let va = self.m.vma as u32;
        self.x.starts.push(Start { write, va, mc: self.x.mc, fetch, word: None });
        if write {
            self.x.write_new = true;
        }
    }

    /// The interrupt as a word in EX samples it (A15b.3; MP2b ruling Q13):
    /// at the clock's instant, or the word's under neutral time.
    pub(crate) fn interrupt_now(&self) -> bool {
        self.x.interrupt_sample.unwrap_or_else(|| self.m.interrupt_at(self.m.ns))
    }

    /// Functional sources (`micro`'s `read_functional`), the PDL buffer's
    /// word from the slot's read.
    fn read_functional(&mut self, source: u8) -> Word {
        let spc_word = |m: &crate::machine::Machine| {
            ((m.spcptr as u32) << 24) | (m.spc[m.spcptr as usize] & 0o1777777)
        };
        match source & 0o17 {
            0o0 => self.m.dispatch_constant.into(),
            0o1 => spc_word(&self.m).into(),
            0o2 => (self.m.pdl_pointer & self.m.geometry.pdl_mask()).into(),
            0o3 => (self.m.pdl_index & self.m.geometry.pdl_mask()).into(),
            0o4 => {
                let v = self.x.pdl_val;
                self.m.pdl_pointer =
                    self.m.pdl_pointer.wrapping_sub(1) & self.m.geometry.pdl_mask();
                v
            }
            0o5 => self.x.pdl_val,
            0o6 => self.x.opc[7].into(),
            0o7 => self.m.q,
            0o10 => self.m.vma,
            0o11 => {
                let t = self.x.map_seen.unwrap_or_else(|| self.m.translate(self.m.md as u32));
                let pfr = (self.x.lvmo >> 27) & 1 != 0;
                let pfw = !((self.x.lvmo >> 26) & 1 == 0 && self.x.wrcyc);
                Word::from((!pfw as u32) << 31 | (!pfr as u32) << 30 | t.l2_data)
            }
            0o12 => self.m.md,
            0o13 => {
                let counter = self.m.lc & self.m.geometry.lc_counter();
                let counter = if self.m.byte_mode() { counter } else { counter & !1 };
                Word::from(self.needfetch()) << 39
                    | Word::from(self.m.interrupt_control & (0o17 << 26)) << 8
                    | counter
            }
            0o14 => {
                let v = spc_word(&self.m);
                self.m.spcptr = self.m.spcptr.wrapping_sub(1) & 0o37;
                self.x.spc_popped = true;
                v.into()
            }
            0o16 => self.m.geometry.machine_id.unwrap_or(!0).into(),
            0o15 => self.m.microseconds().into(),
            _ => self.m.geometry.word_mask(),
        }
    }

    /// Functional destinations (`micro`'s `write_functional`).
    fn write_functional(&mut self, dest: u16, word: Word) {
        let data = word as u32;
        let code = (dest >> 5) & 0o37;
        let code = if code & 0o20 != 0 { code & !0o4 } else { code };
        match code {
            0o0 => {}
            0o1 => {
                let counter = self.m.geometry.lc_counter();
                let high = self.x.lc_adder.unwrap_or(word >> 32 & 3);
                let value = high << 32 | Word::from(data);
                self.m.lc = (self.m.lc & !counter) | (value & counter);
                if !self.m.byte_mode() {
                    self.m.lc &= !1;
                }
                self.m.lc |= self.m.geometry.need_fetch();
                // The prefetch's word is dropped by a write of LC.
            }
            0o2 => {
                self.m.interrupt_control = (word >> 8) as u32 & (0o17 << 26);
            }
            0o5..=0o7 if self.m.geometry.macro_dispatch => {
                self.x.macro_write = Some((code as u32, data));
            }
            0o10 => self.x.pdl_w = Some((PdlAt::At(self.m.pdl_pointer), word)),
            0o11 => {
                self.m.pdl_pointer = (self.m.pdl_pointer + 1) & self.m.geometry.pdl_mask();
                self.x.pdl_w = Some((PdlAt::At(self.m.pdl_pointer), word));
            }
            0o12 => self.x.pdl_w = Some((PdlAt::Index, word)),
            0o13 => self.m.pdl_index = data as u16 & self.m.geometry.pdl_mask(),
            0o14 => self.m.pdl_pointer = data as u16 & self.m.geometry.pdl_mask(),
            0o15 => self.push_spc(data),
            0o16 => {
                self.x.oa_low = data as u64 & 0o377777777;
                if !self.m.geometry.extended() {
                    self.x.imod[0] = true;
                }
            }
            0o17 => {
                self.x.oa_high = data as u64 & 0o37777777;
                if !self.m.geometry.extended() {
                    self.x.imod[1] = true;
                }
            }
            0o20 => self.m.vma = word,
            0o21 => {
                self.m.vma = word;
                self.start(false, false);
            }
            0o22 => {
                self.m.vma = word;
                self.start(true, false);
            }
            0o23 => {
                self.m.vma = word;
                self.x.map_write = Some((self.m.vma, self.m.md));
            }
            0o30 => self.m.md = word,
            0o31 => {
                self.m.md = word;
                self.start(false, false);
            }
            0o32 => {
                self.m.md = word;
                self.start(true, false);
            }
            0o33 => {
                self.m.md = word;
                self.x.map_write = Some((self.m.vma, self.m.md));
            }
            _ => {}
        }
    }

    /// `micro`'s `write_dest`: A and M are written at WB, the machine's
    /// copies of `A-LOCALP`, `M-AP` and the redirect's two with them.
    fn write_dest(&mut self, dest: u16) {
        let out = self.x.out;
        if dest & 0o4000 != 0 {
            let adr = dest & 0o1777;
            self.x.am.push(AmWrite { a: Some(adr), m: None, word: out, seq: self.x.seq });
        } else {
            self.write_functional(dest, out);
            let adr = dest & 0o37;
            self.x.am.push(AmWrite {
                a: Some(adr),
                m: Some(adr as u8),
                word: out,
                seq: self.x.seq,
            });
        }
    }

    fn muldiv(&self) -> Option<muldiv::Op> {
        if !self.m.geometry.muldiv {
            return None;
        }
        muldiv::decode(self.x.ir)
    }

    fn alu(&mut self) {
        let dest = self.ir(14, 12) as u16;
        let ctl = ttl::alu_control(
            self.x.ir,
            self.m.q & 1 != 0,
            self.x.adata & 0x8000_0000 != 0,
            true,
            false,
        );
        let alu =
            ttl::alu(self.x.mdata as u32, self.x.adata as u32, ctl.aluf, ctl.alumode, ctl.cin);
        let mtag = self.x.mdata & !LOW;
        let tag = ttl::alu_tag(self.x.mdata, self.x.adata, ctl.aluf, ctl.alumode);
        let alu_out = (alu.f & LOW) | tag;
        let old_q = self.m.q;
        let arithmetic = self.ir(3, 6) & 0o60 == 0o20;
        self.m.overflow = arithmetic && (alu.f >> 32 & 1) != (alu.f >> 31 & 1);
        let osel = self.ir(12, 2);
        self.x.lc_adder = (!ctl.alumode && self.muldiv().is_none() && (osel == 1 || osel == 3))
            .then(|| ttl::lc_high(self.x.mdata, self.x.adata as u32, ctl.aluf, ctl.cin, osel == 3));
        if let Some(op) = self.muldiv() {
            let (out, q) =
                muldiv::run(op, self.x.mdata as u32, self.x.adata as u32, self.m.q as u32);
            self.m.q = (self.m.q & !LOW) | Word::from(q);
            self.x.out = mtag | Word::from(out);
            return self.write_dest(dest);
        }
        let q_low = self.m.q as u32;
        match self.ir(0, 2) {
            1 => {
                let low = q_low << 1 | (alu_out & 0x8000_0000 == 0) as u32;
                self.m.q = (self.m.q & !LOW) | Word::from(low);
            }
            2 => {
                let low = q_low >> 1 | (alu_out as u32 & 1) << 31;
                self.m.q = (self.m.q & !LOW) | Word::from(low);
            }
            3 => self.m.q = alu_out,
            _ => {}
        }
        self.x.out = match self.ir(12, 2) {
            0 => {
                let rotate = self.ir(0, 6);
                let mask = mask40(rotate, self.ir(6, 4));
                (rol40(self.x.mdata, rotate) & mask) | (self.x.adata & !mask)
            }
            1 => alu_out,
            2 => mtag | ((alu.f >> 1) & LOW),
            _ => mtag | Word::from((alu_out as u32) << 1 | (old_q as u32) >> 31),
        };
        self.write_dest(dest)
    }

    fn byte(&mut self) {
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
        let m = if func & 1 != 0 { rol40(self.x.mdata, pos) } else { self.x.mdata };
        self.x.out = (m & mask) | (self.x.adata & !mask);
        self.write_dest(dest)
    }

    /// `micro`'s `jump_condition_13`: `early` reads `VMAOK` as the late
    /// squash's prediction gives it, no fault (A15b.3).
    fn jump_condition(&mut self, vmaok: bool) -> bool {
        let mut rotate = self.ir(47, 1) << 5 | self.ir(0, 5);
        if self.ir(10, 2) == 3 {
            rotate = self.lc_rotation(self.m.lc, rotate);
        }
        let r = rol40(self.x.mdata, rotate);
        if self.ir(5, 1) == 0 {
            self.x.mdata = r;
            return r & 1 != 0;
        }
        let ctl = ttl::alu_control(
            self.x.ir,
            self.m.q & 1 != 0,
            self.x.adata & 0x8000_0000 != 0,
            false,
            true,
        );
        let alu =
            ttl::alu(self.x.mdata as u32, self.x.adata as u32, ctl.aluf, ctl.alumode, ctl.cin);
        let alu32 = alu.f >> 32 & 1 != 0;
        let int_enabled = self.m.interrupt_control & (1 << 27) != 0;
        let code = match self.ir(0, 5) {
            c @ (0o10 | 0o11) => c,
            0o12 => 0o12,
            c => c & 7,
        };
        let pending = || int_enabled && self.interrupt_now();
        match code {
            0 => r & 1 != 0,
            1 => !alu.aeqm && alu32,
            2 => alu32,
            3 => alu.aeqm && self.x.mdata >> 32 == self.x.adata >> 32,
            4 => !vmaok,
            5 => !vmaok || pending(),
            6 => !vmaok || pending() || (self.m.interrupt_control & (1 << 26) != 0),
            0o10 => self.m.overflow,
            0o11 => (self.x.mdata as u32) < (self.x.adata as u32),
            0o12 => (self.x.mdata as u32) <= (self.x.adata as u32),
            _ => true,
        }
    }

    /// Whether the word's JUMP condition reads `VMAOK`: conditions 4 to 6.
    pub(crate) fn condition_reads_vmaok(ir: u64) -> bool {
        Insn::new(ir).op() == Op::Jump
            && field(ir, 5, 1) == 1
            && matches!(field(ir, 0, 5), 4..=6 | 0o14..=0o16 | 0o24..=0o26 | 0o34..=0o36)
    }

    fn jump(&mut self, vmaok: bool) {
        let mut target = self.ir(12, 14) as u16;
        let r = self.ir(9, 1) != 0;
        let p = self.ir(8, 1) != 0;
        let n = self.ir(7, 1) != 0;
        let invert = self.ir(6, 1) != 0;
        if p && r {
            // WRITE-I-MEM: the control store at the address, from IWR; the
            // words behind it are fetched again (A15b.3).
            let at = target;
            self.m.write_imem(at, Insn::extended(self.x.iwr));
            self.x.wrote_imem = true;
            if !invert && self.jump_condition(vmaok) {
                let ret = if n { self.x.npc.wrapping_sub(1) } else { self.x.npc } & 0o37777;
                self.push_spc(ret as u32);
                self.x.spc_pushed = false;
                self.x.spc_popped = false;
                self.pop_spc();
            }
            return;
        }
        let cond = self.jump_condition(vmaok) != invert;
        self.x.taken = cond;
        if p && cond {
            let ret = if n { self.x.npc.wrapping_sub(1) } else { self.x.npc } & 0o37777;
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
                self.x.inhibit = true;
            }
            self.x.npc = target;
            if r {
                self.x.popj = false;
            }
        }
    }

    fn dispatch(&mut self) {
        let mut pos = self.ir(47, 1) << 5 | self.ir(0, 5);
        let len = self.ir(5, 3);
        let map = self.ir(8, 2);
        let mut addr = self.ir(12, 12);
        let dmem_mask = self.m.geometry.dmem_words() as u32 - 1;
        let use_lpc = self.ir(25, 1) != 0;
        let advance = self.ir(24, 1) != 0;
        let write = self.ir(10, 2) == 2;
        if self.ir(10, 2) == 3 {
            pos = self.lc_rotation(self.m.lc, pos);
        }
        let m = rol40(self.x.mdata, pos) as u32;
        let mask = if len == 0 { 0 } else { !0u32 >> (31 - ((len - 1) & 0o37)) };
        if map != 0 {
            let bits = if self.m.memory_words.pointer_type(self.m.md) {
                self.x.map_seen.unwrap_or_else(|| self.m.translate(self.m.md as u32)).l2_data
            } else {
                3 << 22
            };
            let b18 = (bits >> 22) & 1;
            let b19 = (bits >> 23) & 1;
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
            let new = self.x.adata as u32 & 0o377777;
            self.m.dmem[(addr & dmem_mask) as usize] = new;
            self.ignpopj(entry);
            if self.x.popj && (entry >> 16) & 1 == 0 {
                self.x.npc = (entry & 0o37777) as u16;
                self.x.popj = false;
            }
            return;
        }
        self.m.dispatch_constant = self.ir(32, 10) as u16;
        let mut target = entry & 0o37777;
        let n = (entry >> 14) & 1 != 0;
        let p = (entry >> 15) & 1 != 0;
        let r = (entry >> 16) & 1 != 0;
        self.x.entry_pr = (p, r);
        self.ignpopj(entry);
        let ret = if n {
            let pc = self.x.npc.wrapping_sub(1);
            if use_lpc { pc.wrapping_sub(1) } else { pc }
        } else {
            self.x.npc
        } & 0o37777;
        if advance {
            self.step_lc();
        }
        if n {
            self.x.inhibit = true;
        }
        if p && r {
            return;
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
        self.x.npc = target as u16;
        self.x.popj = false;
    }

    /// **The OA selects** (A15b.15): `word`'s `IR<47:0>` with OA-REG-LOW
    /// ORed into the fields SL names and OA-REG-HIGH into SH's, or the halt
    /// OA-OUTSIDE-FIELDS. Revision 14's IMOD ORs what is pending.
    fn oa_selected(&mut self, word: Insn, pc: u16) -> Result<u64, Halt> {
        if !self.m.geometry.extended() {
            let mut ir = word.raw();
            if std::mem::take(&mut self.x.imod[0]) {
                ir |= self.x.oa_low;
            }
            if std::mem::take(&mut self.x.imod[1]) {
                ir |= self.x.oa_high << 26;
            }
            return Ok(ir & ((1 << 48) - 1));
        }
        let selects = [word.oa_low_select(), word.oa_high_select()];
        let mut ir = word.low_48().raw();
        let fields = oa_fields(ir);
        for (select, (bits, fields)) in
            selects.into_iter().zip([(self.x.oa_low, fields[0]), (self.x.oa_high << 26, fields[1])])
        {
            if !select {
                continue;
            }
            let outside = bits & !ir & !fields;
            if outside != 0 {
                return Err(Halt::OaOutsideFields { pc, bits: outside });
            }
            ir |= bits;
        }
        Ok(ir)
    }

    /// `micro`'s `pdl_field_index`.
    fn pdl_field_index(&self, (base, displacement): (u8, i8)) -> u16 {
        let b = match base {
            0 => self.m.macro_dispatch.ap,
            1 => self.m.macro_dispatch.localp,
            2 => u32::from(self.m.pdl_pointer),
            _ => u32::from(self.m.pdl_index),
        };
        (b.wrapping_add(displacement as i32 as u32) & 0o37777) as u16
    }

    /// **One microcycle, `micro`'s**, for the word in `slot` (or its nopped
    /// microcycle), numbered `mc`. `vmaok` is `VMAOK` as the word's
    /// conditions read it. The writes for WB and the starts are left in
    /// [`Exec`]; the architectural registers are written here, at EX's end.
    pub(crate) fn execute(&mut self, slot: &Slot, mc: u64, vmaok: bool) -> Result<(), Halt> {
        self.x.seq = slot.seq;
        self.x.mc = mc;
        self.x.starts.clear();
        self.x.am.clear();
        self.x.pdl_w = None;
        self.x.wrote_imem = false;
        self.x.d_fused = false;
        self.x.write_new = false;
        self.x.pc = slot.pc;
        self.x.lc_adder = None;
        self.x.inhibit = false;
        self.x.pdl_val = slot.pdl_val;
        // The address of the word after next: `micro`'s npc, one past the
        // next word's, which the microcycle before chose.
        self.x.npc = self.x.npc_prev.wrapping_add(1) & 0o37777;
        self.x.npc_seq = self.x.npc;
        // The head of the microcycle: a map store's write lands, MAP(MD)
        // reading the word from before it (`micro`'s
        // `head_of_microcycle_14`).
        self.x.map_seen = self.x.map_write_d.is_some().then(|| self.m.translate(self.m.md as u32));
        if let Some((vma, md)) = self.x.map_write_d.take() {
            let now = self.m.ns;
            if self.m.write_map_14(vma, md, now) {
                self.drop_prefetch();
                if let crate::tlb::Operation::Empty = crate::tlb::Operation::of(vma, md) {
                    self.tlb_sweep_until = self.clock + self.m.tlb.len() as u64;
                }
            }
        }
        if slot.nop {
            self.x.imod = [false; 2];
            self.x.halted = false;
            self.land_spc_write();
            self.end_of_microcycle(false, false);
            self.x.npc_prev = self.x.npc;
            return Ok(());
        }
        let ir = self.oa_selected(slot.word, slot.pc)?;
        self.x.ir = ir;
        let pdl_field =
            if matches!(slot.word.op(), Op::Alu | Op::Byte) && self.m.geometry.extended() {
                slot.word.pdl_field().map(|f| self.pdl_field_index(f))
            } else {
                None
            };
        self.x.popj = Insn::new(ir).popj();
        self.x.pushed = false;
        self.x.spc_pushed = false;
        self.x.spc_popped = false;
        self.x.macro_write = None;
        self.x.maddr = self.ir(26, 5) as u8;
        self.x.mdata =
            if ir >> 31 & 1 != 0 { self.read_functional(self.x.maddr) } else { slot.m_val };
        self.x.adata = slot.a_val;
        self.land_spc_write();
        let high = if self.m.geometry.extended() { LOW } else { 0o177777 };
        self.x.iwr = ((self.x.adata & high) << 32) | (self.x.mdata & LOW);
        match Insn::new(ir).op() {
            Op::Alu => self.alu(),
            Op::Jump => self.jump(vmaok),
            Op::Dispatch => self.dispatch(),
            Op::Byte => self.byte(),
        }
        self.x.halted = self.ir(10, 2) == 1 && Insn::new(ir).op() != Op::Byte;
        if self.x.popj {
            let mut t = self.pop_spc();
            if (t >> 14) & 1 != 0 {
                t = self.main_loop_return(t, false);
            }
            self.x.npc = (t & 0o37777) as u16;
        }
        if let Some((code, data)) = self.x.macro_write.take() {
            self.m.macro_dispatch.write(code, data);
            if code == 5 && self.m.geometry.extended() {
                self.m.macro_dispatch.register |= data & crate::machine::macro_dispatch::D_ENABLE;
            }
        }
        let mismatch = pdl_field.filter(|&formed| formed != self.m.pdl_index).map(|formed| {
            Halt::PdlFieldMismatch { pc: slot.pc, formed, written: self.m.pdl_index }
        });
        let loads_md = matches!(Insn::new(ir).op(), Op::Alu | Op::Byte)
            && field(ir, 25, 1) == 0
            && matches!(
                {
                    let c = field(ir, 19, 5);
                    if c & 0o20 != 0 { c & !0o4 } else { c }
                },
                0o30..=0o33
            );
        self.end_of_microcycle(true, loads_md);
        self.x.npc_prev = self.x.npc;
        if let Some(h) = mismatch {
            self.pending_halt = Some(h);
        }
        Ok(())
    }

    /// The SPC write the microcycle before handed this one lands, after
    /// its reads (`micro`'s `land_writes`, the stack's half).
    fn land_spc_write(&mut self) {
        if let Some((ptr, word)) = self.x.spc_write.take() {
            self.m.spc[ptr as usize] = word;
        }
    }

    /// **The microcycle's end** (`micro`'s `fetch_and_clock`): an operand
    /// address armed by the microcycle before loads, and M 31's word; the
    /// write started before takes its word; LC steps and fetches; the map
    /// store's write moves to the next microcycle.
    fn end_of_microcycle(&mut self, executed: bool, loads_md: bool) {
        if let Some(o) = self.m.macro_dispatch.operand.take() {
            let adr = self.m.macro_dispatch.operand_address(o);
            self.m.pdl_index = adr as u16 & self.m.geometry.pdl_mask();
        }
        self.m.macro_dispatch.operand = self.x.operand.take();
        if let Some(w) = self.m.macro_dispatch.m31.take() {
            self.x.am.push(AmWrite { a: Some(0o31), m: Some(0o31), word: w, seq: self.x.seq });
        }
        self.m.macro_dispatch.m31 = self.x.d_m31.take();
        // A write started in the microcycle before: its word is `MD` as
        // this microcycle leaves it when it loads `MD` and starts nothing,
        // and otherwise `MD` as the start left it (`micro`'s
        // `next_microcycle_holds_the_write`).
        let starts = !self.x.starts.is_empty() || (self.x.next_instrd && self.needfetch());
        if let Some((seq, md)) = self.x.write_pending.take() {
            let word = if executed && loads_md && !starts { self.m.md } else { md };
            self.fix_write(seq, word);
        }
        if std::mem::take(&mut self.x.write_new) {
            // Fixed by the next microcycle, `MD` as this one leaves it
            // standing for the word unless that one loads `MD`.
            self.x.write_pending = Some((self.x.seq, self.m.md));
        }
        if self.x.next_instrd {
            self.step_lc();
        }
        self.x.next_instrd = std::mem::take(&mut self.x.next_instr);
        self.x.map_write_d = self.x.map_write.take();
        // OPC: the PC of the microcycle, eight deep.
        if executed {
            self.x.opc.copy_within(0..7, 1);
            self.x.opc[0] = self.x.pc;
            self.m.opc = self.x.pc;
        }
    }

    /// The word coherent memory holds at physical `phys` for D's decision:
    /// main memory's, as every write the processor made has left it.
    fn coherent(&self, phys: u32) -> Word {
        self.port.coherent(&self.m, phys)
    }
}
