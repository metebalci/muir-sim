// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The cable's coding: AIM-628 §2.5, "Upright Biphase NRZI".
//!
//! "Each bit cell, which is approximately 250 nanoseconds long, begins
//! with a transition in state, from high to low or from low to high.
//! This transition marks the beginning of a bit cell and provides
//! self-clocking. 3/4 of the way through the bit cell, the state of the
//! cable is sampled; high represents a 1 and low represents a 0. If the
//! bit being represented is the same as the previous bit, there will be
//! one transition at the beginning of the bit cell and a second in the
//! middle of the bit cell. If the bit being represented is the opposite
//! of the previous bit, there will be no transition in the middle of the
//! bit cell since the clock transition will have set the cable to the
//! desired state."
//!
//! "If the ether remains low for more than about two bit cells, it is
//! considered to be not-busy. This condition marks the end of a packet."
//! "If the ether remains high for about two bit cells, this is an 'abort
//! signal'."

/// The bit cell: 250 ns, the 4 MHz `FCLK/2^` of the board.
pub const CELL_NS: u64 = 250;
/// Where in the cell the level is read. AIM-628 says "3/4 of the way
/// through", which of a 250 ns cell would be 187; the board's own chain
/// puts it at 175, and the board is followed --- see [`LOCKOUT_NS`] for the
/// chain and where its tap choices come from. `SAMPLE` is the TD25NC at
/// LMDETC 0A11's 15 ns tap: 160 ns of chain to reach that line, then 15
/// along it. It is a *sibling* of `LOCKOUT END`, not a stage before or
/// after it --- both hang off 0A11's input.
pub const SAMPLE_NS: u64 = 175;
/// How long after a cell's opening edge a further edge is the mid-cell
/// one and not the next cell's: the board's `LOCKOUT`, the 74S74 at LMDETC
/// 0E08, preset by `-EDGE` and clocked clear by `LOCKOUT END`. That is the
/// edge through three delay lines: `GENCLK`, preset by the same edge, into
/// the TD100NC at LMDETC 0B04, whose `SDLYD` feeds the TD100NC at 0B11,
/// whose `SDLYD2` feeds the TD25NC at 0A11, whose `LOCKOUT END` clocks the
/// flip-flop --- 100 + 60 + 10 ns, which falls between the mid-cell
/// transition at 125 and the next cell at 250.
///
/// **The chain forks at 0A11**, which takes `SDLYD2` in and drives two taps
/// off it: `LOCKOUT END` at 10 ns and [`SAMPLE_NS`]'s `SAMPLE` at 15. They
/// are siblings, both 160 ns into the chain; neither is downstream of the
/// other, and one cannot be derived from the other by adding a stage.
///
/// Those three tap choices are not on the drawing. The `NC` bodies carry
/// two wire-wrap posts the DIP has not got, and `cadrio/iob.eco`'s
/// consolidation of 2/18/81 straps a tap to each: MIT's own comments give
/// them as 100 ns, 60 ns and 10 ns. `src/chip.rs` lists the four straps and
/// how their post numbers map onto pins.
pub const LOCKOUT_NS: u64 = 170;
/// A line quiet this long is idle: "more than about two bit cells"
/// (AIM-628 §2.5), where a decoder on the cable ends the packet.  Two and a
/// half cells is this model's choice within that.  The netlist board's own
/// busy one-shot lifts `-CBLBSY` 500 ns after the last edge, exactly two
/// cells (`the_models_instants_are_the_boards` measures it), and the board's
/// receiver is held to that by [`crate::chaos::board::CBLBSY_OFF_NS`]; this
/// is the cable's decoders, which no board sets.
pub const IDLE_NS: u64 = 2 * CELL_NS + CELL_NS / 2;

/// The level changes that put `bits` on a line that is idle low, as
/// `(offset_ns, level)` from the first edge. The line is low again
/// after the last, which is why [`crate::chaos::packet::frame`] ends in
/// a zero.
pub fn encode(bits: &[bool]) -> Vec<(u64, bool)> {
    encode_in(bits, 1)
}

/// [`encode`] with the offsets in units of `1/per_ns` ns: nanoseconds at
/// 1, and revision 15's 0.5 ns at 2, where every time here doubles (MP2b
/// ruling Q19).
pub fn encode_in(bits: &[bool], per_ns: u64) -> Vec<(u64, bool)> {
    let cell = CELL_NS * per_ns;
    let mut out = Vec::with_capacity(bits.len() * 2);
    let mut level = false;
    let mut prev = false;
    for (k, &bit) in bits.iter().enumerate() {
        let start = k as u64 * cell;
        level = !level;
        out.push((start, level));
        if bit == prev {
            level = !level;
            out.push((start + cell / 2, level));
        }
        prev = bit;
    }
    out
}

/// A receiver on the line: fed each edge as it happens and asked what is
/// due, it delivers a packet's bits when the line has gone idle. It does
/// what the board's detector does with its lockout and its sample delay,
/// which is what makes the two agree on which edges are cells.
#[derive(Clone, Debug)]
pub struct Decoder {
    /// Units a nanosecond, which every time here is counted in: 1, or 2
    /// on revision 15 ([`encode`]).
    per_ns: u64,
    level: bool,
    /// The opening edge of the current cell, if in a packet.
    cell: Option<u64>,
    /// When the current cell's sample is due, if not taken yet.
    sample: Option<u64>,
    bits: Vec<bool>,
    /// The last edge seen.
    last_edge: Option<u64>,
}

impl Default for Decoder {
    fn default() -> Decoder {
        Decoder {
            per_ns: 1,
            level: false,
            cell: None,
            sample: None,
            bits: Vec::new(),
            last_edge: None,
        }
    }
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder::default()
    }

    /// Counts its times in units of `1/per_ns` ns from here on.
    pub fn set_per_ns(&mut self, per_ns: u64) {
        self.per_ns = per_ns;
    }

    /// [`IDLE_NS`] in this decoder's units.
    fn idle(&self) -> u64 {
        IDLE_NS * self.per_ns
    }

    /// Whether a packet is in progress: the line has had an edge and has
    /// not yet been idle for [`IDLE_NS`].
    pub fn busy(&self) -> bool {
        self.cell.is_some()
    }

    /// The line changed to `level` at `t`.
    pub fn edge(&mut self, t: u64, level: bool) {
        if level == self.level {
            return;
        }
        self.level = level;
        self.last_edge = Some(t);
        match self.cell {
            Some(start) if t < start + LOCKOUT_NS * self.per_ns => {
                // The mid-cell transition: locked out.
            }
            _ => {
                self.cell = Some(t);
                self.sample = Some(t + SAMPLE_NS * self.per_ns);
            }
        }
    }

    /// The cell now open, if the line is in a run: its opening edge, and
    /// whether its sample has been taken.
    pub fn open_cell(&self) -> Option<(u64, bool)> {
        self.cell.map(|c| (c, self.sample.is_none()))
    }

    /// The bits sampled so far of the run now on the line: what a
    /// receiver on the cable has taken of it. A receiver decides on a
    /// frame's destination word long before the run ends
    /// ([`crate::chaos::ether::BUSY_ABORT_NS`]), and for a frame the
    /// netlist board is putting on the cable these bits are all the ether
    /// has of it.
    pub fn bits(&self) -> &[bool] {
        &self.bits
    }

    /// The bits a receiver on the line has stored of the run now on it:
    /// every cell sampled but the one now open.  The board writes a bit
    /// into its buffer on the next cell's opening edge, not at its sample,
    /// so a clear inside a frame leaves the bits up to the cell before the
    /// open one, and a frame's trailing zero, which no cell follows, is
    /// never stored.  Measured on the netlist board by
    /// `the_bits_stored_before_a_clear_stay_in_the_buffer_on_both_alike`
    /// in `tests/chaos_rtl.rs`, clears swept across a cell.
    pub fn stored(&self) -> &[bool] {
        let n = self.bits.len();
        if self.cell.is_some() && self.sample.is_none() {
            &self.bits[..n.saturating_sub(1)]
        } else {
            &self.bits
        }
    }

    /// When the decoder next needs to be told the time: the sample due,
    /// or the moment the line will have been idle long enough. A line
    /// held high --- an abort signal --- is not idle and has no such
    /// moment until it falls.
    pub fn next_due(&self) -> Option<u64> {
        let idle =
            self.last_edge.filter(|_| self.cell.is_some() && !self.level).map(|e| e + self.idle());
        match (self.sample, idle) {
            (Some(s), Some(i)) => Some(s.min(i)),
            (s, i) => s.or(i),
        }
    }

    /// The time is `t`: takes a sample that is due, and ends the packet
    /// if the line has been idle since its last edge. Returns the
    /// packet's bits when it ends.
    pub fn at(&mut self, t: u64) -> Option<Vec<bool>> {
        if let Some(s) = self.sample
            && t >= s
        {
            self.bits.push(self.level);
            self.sample = None;
        }
        if let (Some(_), Some(e)) = (self.cell, self.last_edge)
            && t >= e + self.idle()
            && !self.level
        {
            self.cell = None;
            self.sample = None;
            return Some(std::mem::take(&mut self.bits));
        }
        None
    }
}

/// Runs a whole waveform through a [`Decoder`]: the packets it delivers.
pub fn decode(changes: &[(u64, bool)]) -> Vec<Vec<bool>> {
    let mut d = Decoder::new();
    let mut out = Vec::new();
    let mut t = 0;
    for &(at, level) in changes {
        while let Some(due) = d.next_due()
            && due <= at
        {
            t = due;
            if let Some(p) = d.at(t) {
                out.push(p);
            }
        }
        t = t.max(at);
        d.edge(at, level);
    }
    while let Some(due) = d.next_due() {
        t = due.max(t);
        if let Some(p) = d.at(t) {
            out.push(p);
        }
    }
    out
}
