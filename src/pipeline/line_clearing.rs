// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **Revision 16's line clearing** (QUUX's Ethernet contract, §2.5 (ii)):
//! the cache made coherent with what the Ethernet device delivered before
//! the machine sees 251 move, as muir-fpga's fabric makes it
//! (`rtl/machine/quux16_ethernet.sv`'s engine, `quux15_port.sv`'s transfer
//! channel and valid maps), clock for clock.
//!
//! The device fills a receive entry at once, and the host's side reads
//! RX_CONS moved ([`crate::ethernet_device::EthernetDevice::filled`]). 251,
//! which the machine reads, follows it an entry at a time, by an engine of
//! these steps, one clock each but where it waits:
//!
//! - **idle**: in the clock after an entry is filled, 251 behind;
//! - **grant**: the port's transfer channel asked for, and the engine's at
//!   the next clock's edge (2 clocks): granted beside block-disk's walk,
//!   which on muir moves its transfer at START and never holds the channel;
//! - **start**: the engine's line dropped, so that the entry's words are
//!   main memory's;
//! - **word 0**: the entry's line read through the port's one fill
//!   ([`super::port::Reader::Engine`]), installed nowhere: R + 2 clocks, R
//!   the port's read latency, while nothing else holds the fill; then word
//!   0, the buffer;
//! - **word 1** and **word 2**: the room and the length, from the line;
//! - **clear**: the set of each buffer line from the buffer's first word's
//!   to its ceil(n/4)th's, n the length clamped to the room and to 1,514
//!   bytes (none when n is 0), one a clock, in a clock no fill installs in;
//! - **entry**: the entry's line's set cleared the same way, and 251 moved
//!   at that clock's edge, so that a read granted in it reads the new 251.
//!
//! So with nothing else on the port, 251 moves at the edge of clock T = R +
//! 9 + L after the clock the entry was filled in, L the buffer's lines
//! cleared. Lookups never wait for the clears; they wait for the engine's
//! fill as for any other. A disable drops what is under way, and the halt's
//! drain waits for the engine.
//!
//! **Under neutral time, and on `micro`, 251 moves with the entry filled**
//! (the device's own rule without an engine), as every other wait of the
//! port's is `rtl`'s alone (MP2b ruling Q12): under neutral time the sets
//! are cleared at once ([`Pipeline::ethernet_at_once`]).

use super::Pipeline;
use super::port::LINE_WORDS;
use crate::machine::Word;

/// The clocks the engine's read waits before it is let in: block-disk's
/// transfer's patience (muir-fpga `quux15_port.sv`'s `D_PATIENCE`).
const PATIENCE: u8 = 16;

/// The longest Ethernet frame whose lines are cleared.
const MAX_FRAME: u32 = crate::ethernet_device::MAX_FRAME;

/// A physical word address, and a line number of one.
const ADDRESS: u32 = 0o1777777777;
const LINE: u32 = ADDRESS >> 3;

/// The engine's step in a clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Step {
    #[default]
    Idle,
    Grant,
    Start,
    Word0,
    Word1,
    Word2,
    Clear,
    Entry,
}

/// The engine between two clocks.
#[derive(Clone, Debug, Default)]
pub(crate) struct LineClearing {
    /// The step of the clock under way, and of the next.
    step: Step,
    next: Step,
    /// The step's word read from the engine's line this clock.
    read_now: bool,
    /// The channel the engine's: it asked in the clock before.
    granted: bool,
    /// The entries filled as the last clock left them.
    target: u16,
    /// The entry's words 0 and 1; the line to clear and the buffer's last.
    buf: u32,
    room: u32,
    line: u32,
    last: u32,
    /// Clocks its read has waited.
    waited: u8,
}

impl LineClearing {
    /// Idle, the entries filled `filled`: at power-on, or a resume's.
    pub(crate) fn idle_at(filled: u16) -> LineClearing {
        LineClearing { target: filled, ..LineClearing::default() }
    }
}

impl Pipeline {
    /// Whether the engine runs: not under neutral time.
    fn line_clearing_runs(&self) -> bool {
        !self.neutral
    }

    /// **The engine's work in the clock under way that comes before the
    /// stages**, after the port's tick: its word from its line, the set it
    /// clears, so that a lookup granted now sees it cleared, and 251's
    /// move, so that a register read granted now reads it.
    pub(crate) fn line_clearing_start(&mut self) {
        if !self.line_clearing_runs() {
            return;
        }
        let now = self.clock;
        self.lc.read_now = false;
        self.lc.next = self.lc.step;
        if !self.m.ethernet.enabled() {
            self.lc.step = Step::Idle;
            self.lc.next = Step::Idle;
            return;
        }
        let entry = self.m.ethernet.entry_at_251();
        let word =
            |port: &super::port::Port, k: u32| port.line_word(entry + k).map(|w: Word| w as u32);
        let lc = &mut self.lc;
        let mut cleared = None;
        match lc.step {
            Step::Idle => {
                if self.m.ethernet.rx_cons() != lc.target {
                    lc.next = Step::Grant;
                }
            }
            Step::Grant => {
                if lc.granted {
                    lc.next = Step::Start;
                }
            }
            Step::Start => {
                self.port.drop_line_buffer();
                lc.next = Step::Word0;
            }
            Step::Word0 => {
                if let Some(w) = word(&self.port, 0) {
                    (lc.buf, lc.read_now, lc.next) = (w & ADDRESS, true, Step::Word1);
                }
            }
            Step::Word1 => {
                if let Some(w) = word(&self.port, 1) {
                    (lc.room, lc.read_now, lc.next) = (w, true, Step::Word2);
                }
            }
            Step::Word2 => {
                if let Some(w) = word(&self.port, 2) {
                    let n = w.min(lc.room).min(MAX_FRAME);
                    let words = n.div_ceil(4);
                    lc.line = lc.buf >> 3;
                    lc.last = ((lc.buf + words).wrapping_sub(1) >> 3) & LINE;
                    lc.read_now = true;
                    lc.next = if words == 0 { Step::Entry } else { Step::Clear };
                }
            }
            Step::Clear => {
                if !self.port.installs_at(now + 1) {
                    cleared = Some(lc.line * LINE_WORDS);
                    if lc.line == lc.last {
                        lc.next = Step::Entry;
                    }
                    lc.line = (lc.line + 1) & LINE;
                }
            }
            Step::Entry => {
                if !self.port.installs_at(now + 1) {
                    cleared = Some(entry);
                    lc.next = Step::Idle;
                    self.m.ethernet.move_251();
                }
            }
        }
        if let Some(bus) = cleared {
            self.port.cache.clear_set(bus);
            self.drop_prefetch();
        }
    }

    /// **The engine's work at the clock's end**, the stages' lookups made:
    /// its read of the entry let go, or its wait counted, and the step the
    /// next clock takes.
    pub(crate) fn line_clearing_end(&mut self) {
        if !self.line_clearing_runs() {
            return;
        }
        let now = self.clock;
        let reading = matches!(self.lc.step, Step::Word0 | Step::Word1 | Step::Word2);
        let want = reading && !self.lc.read_now && !self.port.filling(super::port::Reader::Engine);
        let go = want && self.port.engine_may_read(now);
        if go {
            let entry = self.m.ethernet.entry_at_251();
            self.port.engine_read(now, entry);
        }
        let lc = &mut self.lc;
        self.port.force = want && !go && lc.waited + 1 >= PATIENCE;
        lc.waited = if want && !go { (lc.waited + 1).min(63) } else { 0 };
        // On muir no walk holds the channel: it is the engine's from the
        // clock after it asks.
        lc.granted = matches!(lc.step, Step::Grant | Step::Start) || reading;
        lc.step = if self.m.ethernet.enabled() { lc.next } else { Step::Idle };
        lc.target = self.m.ethernet.filled();
    }

    /// Whether the engine is at work or owes some: what the halt's drain
    /// waits for.
    pub(crate) fn line_clearing_busy(&self) -> bool {
        self.line_clearing_runs()
            && self.m.ethernet.enabled()
            && (self.lc.step != Step::Idle || self.m.ethernet.rx_cons() != self.lc.target)
    }

    /// **Under neutral time, every entry filled cleared and passed at
    /// once**: the sets of its buffer's lines and its own, read as the
    /// engine reads them, then 251 moved. Called after anything that may
    /// have advanced the device. The engine does it clock by clock
    /// otherwise.
    pub(crate) fn ethernet_at_once(&mut self) {
        if !self.neutral {
            return;
        }
        while self.m.ethernet.enabled() && self.m.ethernet.rx_cons() != self.m.ethernet.filled() {
            let entry = self.m.ethernet.entry_at_251();
            let word = |k: u32| super::port::peek(&self.m, entry + k) as u32;
            let (buf, room, len) = (word(0) & ADDRESS, word(1), word(2));
            let words = len.min(room).min(MAX_FRAME).div_ceil(4);
            if words > 0 {
                let (first, last) = (buf >> 3, (buf + words - 1) >> 3 & LINE);
                let mut line = first;
                loop {
                    self.port.cache.clear_set(line * LINE_WORDS);
                    if line == last {
                        break;
                    }
                    line = (line + 1) & LINE;
                }
            }
            self.port.cache.clear_set(entry);
            self.m.ethernet.move_251();
            self.drop_prefetch();
        }
    }
}
