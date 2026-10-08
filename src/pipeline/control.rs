// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **The pipeline's console**: the boot, `-RESET`, run and halt, the
//! microcode single step, the checkpoint and the readout (contract G3
//! revision 15, §9; A15b.13), and the cache-only prefetch (H8a §3.5).
//!
//! **The halt drains** (A15b.13): the words in EX and WB complete, CS and
//! RD are squashed, and the queue and the in-flight list empty. The state
//! is then a single-edge machine's between two microcycles: the next word
//! to run is the first one squashed, and the readout reads committed
//! state.

use super::{Copies, Pipeline, STALL_BOUND};
use crate::machine::{Halt, Word};
use crate::spy;

impl Pipeline {
    /// The boot (`micro`'s `boot`): `-RESET`, the console's registers
    /// cleared, `-BOOT` presetting RUN, and the PROM's first word next, the
    /// pipeline empty.
    pub(crate) fn boot_15(&mut self) {
        self.m.vmaok = false;
        self.m.reset_console_registers();
        self.m.timers = crate::machine::Timers::new();
        self.m.macro_dispatch.reset();
        self.m.clock_control.run = true;
        self.srun = true;
        self.reset_pipeline();
        self.npc = self.m.reset_pc();
        self.x.npc_prev = self.npc;
        self.trap();
        self.m.reset_memory_system(self.m.ns);
        self.tlb_sweep_until = self.clock + self.m.tlb.len() as u64;
        self.x.oa_low = 0;
        self.x.oa_high = 0;
        self.x.imod = [false; 2];
        self.halted = false;
        self.draining = false;
        self.stepping = false;
    }

    /// The boot's trap: the word that stood in the pipeline is nopped, a
    /// microcycle that runs nothing, as `micro`'s boot inhibits it.
    fn trap(&mut self) {
        let mut s = super::Slot::new(self.seq, self.npc, crate::isa::Insn::new(0));
        s.nop = true;
        s.trap = true;
        self.seq += 1;
        self.cs = Some(s);
    }

    /// The pipeline emptied: every stage and the port, the copies from the
    /// architectural state.
    fn reset_pipeline(&mut self) {
        self.cs = None;
        self.rd = None;
        self.ex = None;
        self.wb = None;
        self.npc_after = None;
        self.port.drain_now(&mut self.m);
        self.b = super::stages::Back::default();
        self.x.write_pending = None;
        self.x.write_new = false;
        self.x.map_write = None;
        self.x.map_write_d = None;
        self.x.spc_write = None;
        self.d_wait = false;
        self.d_slot_pending = false;
        self.cs_wait_until = 0;
        self.held_am.clear();
        self.held_pdl = None;
        self.copies = Copies::default();
        self.restore_copies();
    }

    /// The master clock's edge as the console sees it (`micro`'s
    /// `mclk_edge`): `-RESET` and `-BOOT` from the register page, and the
    /// run and step synchronizers.
    pub(crate) fn console_edge(&mut self) {
        let boot = std::mem::take(&mut self.m.prog_boot);
        let reset = std::mem::take(&mut self.m.prog_reset) || boot;
        if reset {
            self.m.reset_console_registers();
            self.m.timers = crate::machine::Timers::new();
            self.m.macro_dispatch.reset();
            self.port.drain_now(&mut self.m);
            self.m.reset_memory_system(self.m.ns);
            self.tlb_sweep_until = self.clock + self.m.tlb.len() as u64;
            if self.m.geometry.extended() {
                self.x.oa_low = 0;
                self.x.oa_high = 0;
            }
        }
        if boot {
            self.m.vmaok = false;
            self.m.clock_control.run = true;
            self.reset_pipeline();
            self.npc = self.m.reset_pc();
            self.x.npc_prev = self.npc;
            self.trap();
            self.x.imod = [false; 2];
            self.halted = false;
            self.draining = false;
            self.stepping = false;
        }
        self.ssdone = self.sstep;
        self.sstep = self.m.clock_control.step;
        let run = self.m.clock_control.run;
        if self.srun && !run && !self.halted {
            self.begin_drain();
        }
        if run && self.halted && !(self.m.mode.errstop && self.x.halted) {
            self.halted = false;
            self.boundary_halt = false;
            self.action_halt = false;
        }
        if self.sstep && !self.ssdone && self.halted {
            // A single step: one word through the four stages.
            self.halted = false;
            self.single_step = true;
        }
        self.srun = run;
    }

    /// Whether CS may load a word: the machine runs, or a single step's one
    /// word is still to come.
    pub(crate) fn fetching(&mut self) -> bool {
        if self.single_step {
            self.single_step = false;
            self.stepping = true;
            return true;
        }
        self.srun && !self.halted
    }

    /// The halt begins: CS and RD squashed, the next word to run the first
    /// of them, its fetch's order kept.
    pub(crate) fn begin_drain(&mut self) {
        self.begin_drain_from(false);
    }

    /// The halt begins, and with `ex` the word in EX, not yet run, is
    /// squashed too, so that the machine stops after the word that has just
    /// committed: the boundary halt of the time-neutral harness (MP2b
    /// ruling Q12 (e)).
    ///
    /// The squashed words are fetched again in order. The first's address
    /// and the second's were chosen by words that have committed, so both
    /// are kept: the second fetched, or the address the next fetch would
    /// have taken. Every later address is the first's own choice, made
    /// again in RD.
    pub(crate) fn begin_drain_from(&mut self, ex: bool) {
        if self.draining {
            return;
        }
        self.draining = true;
        let ex = if ex { self.ex.take() } else { None };
        let words: Vec<super::Slot> =
            [ex, self.rd.take(), self.cs.take()].into_iter().flatten().collect();
        if let Some(f) = words.first() {
            let next = self.npc;
            self.npc = f.pc;
            self.b.nop_next = f.nop;
            self.b.pre_nop_next = f.pre_nop && !f.nop;
            self.npc_after = Some(match words.get(1) {
                Some(s) => s.pc,
                // The trap again, its word after it.
                None if f.trap => f.pc,
                None => next,
            });
        }
        self.seq =
            [&self.ex, &self.wb].into_iter().flatten().map(|w| w.seq).max().unwrap_or(self.seq) + 1;
        self.restore_copies();
    }

    /// Whether the drain is done: nothing in EX or WB, the port drained,
    /// nothing left for a register.
    pub(crate) fn drained_now(&self) -> bool {
        self.ex.is_none()
            && self.wb.is_none()
            && self.cs.is_none()
            && self.rd.is_none()
            && self.port.idle(self.clock)
            && self.b.md.is_none()
            && !self.b.md_fill
            && self.b.register_write.is_none()
            && self.b.cmd_prod.is_none()
            && self.b.starts.is_empty()
    }

    /// **`Engine::step`**: clocks until a microcycle is counted, or one
    /// clock of a halted machine.
    pub(crate) fn step_15(&mut self) -> Result<(), Halt> {
        let cycles = self.m.cycles;
        for _ in 0..STALL_BOUND {
            self.clock_once()?;
            if self.halted || self.m.cycles != cycles {
                return Ok(());
            }
        }
        panic!(
            "the pipeline counted no microcycle in {STALL_BOUND} clocks: stages {:?}",
            self.stages()
        );
    }

    /// Runs clocks until the machine is drained and halted, at most
    /// `limit` of them: a halt asked for by the console.
    pub fn halt_and_drain(&mut self, limit: u64) -> Result<(), Halt> {
        self.m.clock_control.run = false;
        for _ in 0..limit {
            if self.halted {
                return Ok(());
            }
            self.clock_once()?;
        }
        Ok(())
    }

    /// The TLB's sweep taken as done (a test aid): -RESET's sweep holds
    /// every start 4,096 clocks (MP2b ruling Q9), which a hand-built program
    /// that starts at once would spend waiting.
    pub fn skip_sweep(&mut self) {
        self.tlb_sweep_until = 0;
    }

    /// The control-store address of the microcycle the last step counted,
    /// `None` when it was nopped ([`crate::micro::Micro::executed`]).
    pub fn executed(&self) -> Option<u16> {
        self.last_counted
    }

    /// Whether the pipeline is halted at the harness's boundary
    /// ([`Pipeline::boundary_at`]), drained.
    pub fn at_boundary(&self) -> bool {
        self.halted && self.boundary_halt
    }

    /// OA-REG-LOW and OA-REG-HIGH ([`crate::micro::Micro::oa_registers`]).
    pub fn oa_registers(&self) -> (u64, u64) {
        (self.x.oa_low, self.x.oa_high)
    }

    /// Whether the memory side is quiet: every write answered, no read on
    /// its way, nothing for a register (a test aid, for a comparison at the
    /// end of a run).
    pub fn quiet(&self) -> bool {
        self.port.idle(self.clock)
            && self.b.md.is_none()
            && !self.b.md_fill
            && self.b.register_write.is_none()
            && self.b.cmd_prod.is_none()
            && self.b.starts.is_empty()
    }

    /// The machine with the SPC stack's and the PDL buffer's writes the
    /// last microcycle handed the next one landed: the state between two
    /// microcycles ([`crate::micro::Micro::landed`]).
    pub fn landed(&self) -> crate::machine::Machine {
        let mut m = self.m.clone();
        self.pending().land(&mut m);
        m
    }

    /// What [`Pipeline::landed`] lands, without a copy of the machine.
    pub fn pending(&self) -> crate::machine::Pending {
        crate::machine::Pending { md: None, pdl: self.b.pdl_pending, spc: self.x.spc_write }
    }

    /// Whether the machine is halted, drained.
    pub fn is_halted(&self) -> bool {
        self.halted
    }

    /// The next word to run: the oldest word not committed, or the next
    /// fetch.
    pub(crate) fn pc_15(&self) -> u16 {
        [&self.ex, &self.rd, &self.cs]
            .into_iter()
            .flatten()
            .filter(|s| !s.nop)
            .map(|s| s.pc)
            .next()
            .unwrap_or(self.npc)
    }

    /// The readout (A15b.13), of committed state: `IR` the next word to
    /// run, `PC` its address.
    pub(crate) fn spy_15(&self, eadr: u8) -> u16 {
        let next = self.m.fetch(self.pc_15()).raw();
        let half = |v: u64, k: u8| (v >> (16 * k as u32)) as u16;
        match eadr {
            spy::IR_LOW | spy::IR_MED | spy::IR_HIGH => half(next, eadr),
            spy::IR_EXT if self.m.geometry.extended() => half(next, eadr),
            spy::OPC => self.x.opc[7] & 0x3fff,
            spy::PC => self.pc_15() & 0x3fff,
            spy::FLAG_1 => spy::Flag1 {
                promdisable: self.m.mode.prom_disable,
                err: self.x.halted,
                ssdone: self.ssdone,
                srun: self.srun,
                ..Default::default()
            }
            .word(),
            spy::FLAG_2 => spy::Flag2 { vmaok: self.m.vmaok, ..Default::default() }.word(),
            spy::STAT_LOW | spy::STAT_HIGH => 0,
            _ => spy::OPEN_READ,
        }
    }

    /// **The checkpoint**, taken halted and so drained (A15b.13): the
    /// machine, the period, the port's timing and cache, and the
    /// executor's state between microcycles; nothing of RD's speculation
    /// and no queued write.
    pub(crate) fn save_15(&self, w: &mut crate::checkpoint::Writer) {
        assert!(self.drained_now() || self.halted, "a checkpoint of the pipeline is taken drained");
        self.m.save(w);
        w.u64(self.period);
        self.port.save(w);
        w.u16(self.npc);
        w.opt(self.npc_after, crate::checkpoint::Writer::u16);
        w.bool(self.b.pre_nop_next);
        w.bool(self.b.nop_next);
        w.opt(self.b.pdl_pending, |w, (a, v)| {
            w.u16(a);
            w.word(v);
        });
        w.opt(self.b.md_old.map(|(_, v)| v), crate::checkpoint::Writer::word);
        w.bool(self.d_wait);
        w.bool(self.d_slot_pending);
        w.u64(self.x.oa_low);
        w.u64(self.x.oa_high);
        // Revision 14's IMOD flags; revision 15 has no pending OA flag
        // (A15b.13; MP4 ruling Q3).
        if !self.m.geometry.extended() {
            w.bool(self.x.imod[0]);
            w.bool(self.x.imod[1]);
        }
        w.bool(self.x.next_instr);
        w.bool(self.x.next_instrd);
        w.u32(self.x.lvmo);
        w.bool(self.x.wrcyc);
        w.opt(self.x.spc_write, |w, (p, v)| {
            w.u8(p);
            w.u32(v);
        });
        w.opt(self.x.map_write_d, |w, (a, b)| {
            w.word(a);
            w.word(b);
        });
        w.u16s(&self.x.opc);
        w.bool(self.x.halted);
        w.bool(self.halted);
        w.u64(self.committed);
        w.u16(self.x.npc_prev);
    }

    /// Back from a checkpoint (A15b.13): refused at another period,
    /// memory timing or cache (MP2b ruling Q16); the pipeline empty, the
    /// TLB swept.
    pub(crate) fn load_15(&mut self, r: &mut crate::checkpoint::Reader) -> std::io::Result<()> {
        self.m.load(r)?;
        let period = r.u64()?;
        if period != self.period {
            return Err(crate::checkpoint::bad(format!(
                "a period of {} ns, and this run is at {} ns: --microcycle-ns {}",
                crate::clock::microcycle_ns_text(period),
                crate::clock::microcycle_ns_text(self.period),
                crate::clock::microcycle_ns_text(period)
            )));
        }
        // The clock goes on from the machine's time, which the devices'
        // deadlines are in.
        self.clock = self.m.ns / self.period;
        let m = self.m.clone();
        self.port.load(&m, r)?;
        self.reset_pipeline();
        self.npc = r.u16()?;
        self.npc_after = r.opt(crate::checkpoint::Reader::u16)?;
        self.b.pre_nop_next = r.bool()?;
        self.b.nop_next = r.bool()?;
        self.b.pdl_pending = r.opt(|r| Ok((r.u16()?, r.word()?)))?;
        // The next word fetched is the one after the read start.
        self.b.md_old =
            r.opt(crate::checkpoint::Reader::word)?.map(|v| (self.seq.wrapping_sub(1), v));
        self.d_wait = r.bool()?;
        self.d_slot_pending = r.bool()?;
        self.x.oa_low = r.u64()?;
        self.x.oa_high = r.u64()?;
        // A revision-15 file of version 50 has revision 14's two flags too,
        // false on revision 15.
        self.x.imod =
            if !self.m.geometry.extended() || r.version() == Some(crate::checkpoint::VERSION_40) {
                [r.bool()?, r.bool()?]
            } else {
                [false; 2]
            };
        self.x.next_instr = r.bool()?;
        self.x.next_instrd = r.bool()?;
        self.x.lvmo = r.u32()?;
        self.x.wrcyc = r.bool()?;
        self.x.spc_write = r.opt(|r| Ok((r.u8()?, r.u32()?)))?;
        self.x.map_write_d = r.opt(|r| Ok((r.word()?, r.word()?)))?;
        r.u16s_into(&mut self.x.opc)?;
        self.x.halted = r.bool()?;
        self.halted = r.bool()?;
        self.committed = r.u64()?;
        self.x.npc_prev = r.u16()?;
        self.m.period = if self.m.geometry.extended() { self.period } else { 0 };
        self.m.tlb.sweep();
        self.tlb_sweep_until = self.clock + self.m.tlb.len() as u64;
        self.restore_copies();
        Ok(())
    }

    /// The prefetch's word dropped (H8a §3.5): by a write of LC, a store to
    /// its word, a transfer, a map write, `-RESET`.
    pub(crate) fn drop_prefetch(&mut self) {
        self.b.prefetched = None;
    }

    /// **The prefetch** (H8a §3.5, with page reach, G2 §9): a fetch read
    /// from main memory at physical word `bus`, of virtual word `va`,
    /// takes the next word into the one-word buffer when the cache holds
    /// it, in the same page.
    pub(crate) fn prefetch_after(&mut self, bus: u32, va: u32) {
        self.b.prefetched = None;
        if bus & crate::tlb::DEVICE != 0 {
            return;
        }
        let next = bus + 1;
        if next.is_multiple_of(super::port::LINE_WORDS * 128)
            || next as usize >= self.m.main.len()
            || !self.port.cache.holds(next)
        {
            return;
        }
        let word: Word = self.port.coherent(&self.m, next);
        self.b.prefetched = Some((va.wrapping_add(1), next, word));
    }
}
