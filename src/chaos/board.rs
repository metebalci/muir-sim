// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Chaosnet interface as a behavior, for the engines whose I/O board
//! is `rtl`'s model: the registers of [`super::interface`] over an
//! [`Ether`], so that the stations on the cable answer `rtl` as they
//! answer the netlist board.  The netlist board is the authority:
//! `tests/chaos_rtl.rs` runs the same register sequences on both, and
//! holds the instants.
//!
//! What the software does with it is microcode 323's `uc-chaos.lisp`, the
//! Unibus interrupt at vector 270: with Receive Done up it reads the bit
//! count, the packet's words out of the read buffer, the CSR again, and
//! writes the CSR with Clear Receiver; to transmit it writes the words and
//! reads 764152; it dismisses by writing the two interrupt enables.
//!
//! When a frame goes out is the **turn timer's**, page LMTURN of the I/O
//! board: a twelve-bit counter of 74LS193s counting down on a microsecond
//! clock that runs only while the cable is idle, loaded with the address
//! difference of every packet heard ([`turn_byte`]), and the transmitter
//! starting on the rising edge of its bit 7 --- `Turn`, with every
//! interval measured on the netlist board.

use super::ether::{BUSY_ABORT_NS, Ether, RACT_NS, turn_byte};
use super::interface::{self, csr};
use super::packet::{Framed, Received, check_on, check_word, line, unframe_any};
use super::wire;
use crate::clock::TimeBase;
use std::collections::VecDeque;

/// The Unibus interrupt vector, `uc-interrupt.lisp`: "JUMP-EQUAL M-B
/// (A-CONSTANT 270) CHAOS-INTR --- Chaos net has special handler".
pub const VECTOR: u16 = 0o270;

/// The bits of the CSR the software writes and reads back.
const WRITABLE: u16 = csr::TIMER_INT_ENABLE
    | csr::LOOP_BACK
    | csr::SPY
    | csr::RECEIVE_INT_ENABLE
    | csr::TRANSMIT_INT_ENABLE;

/// Both buffers are a 2147, 4K by 1: the outgoing one at LMTBUF 0C10 on
/// the twelve bits of `TBCT`, off the 25LS193s at 0C11-0C13, and the
/// incoming one at LMRBUF 0C04 on `RBCT`. The bits each holds, and the
/// sixteen-bit words. Each count wraps, so neither buffer has an end: a
/// word written past the 256th goes over the first, and so does a bit
/// received past the 4,096th, and the reads go round
/// (`tests/chaos_rtl.rs`: `a_frame_written_past_the_buffer_wraps_on_both_alike`,
/// `a_frame_longer_than_the_buffer_keeps_its_last_4096_bits_on_both_alike`,
/// `reading_on_goes_round_the_buffer_on_both_alike`).
const BUFFER_BITS: usize = 4_096;
const BUFFER_WORDS: usize = BUFFER_BITS / 16;

/// What a word of the incoming buffer never written reads as: the
/// netlist's 2147 starts with its cells clear, which the read path gives
/// back as ones (`reading_on_goes_round_the_buffer_on_both_alike` reads it
/// past a short frame). A real 2147 holds nothing defined at power-up;
/// what a CADR's read there is **unverified**, and only a board would say.
const NEVER_WRITTEN: u16 = 0o177777;

/// The incoming buffer's word that the last bit of `bits` received is in,
/// counting from the word the first bit went into: where `RBCT` leaves the
/// first read.
fn top_word(bits: usize) -> usize {
    (bits + BUFFER_BITS - 1) % BUFFER_BITS / 16
}

/// The words of a frame as its transmit buffer would hold them, for
/// [`super::packet::line`]: what the software reads back of `r` --- its
/// words, the source and the check word --- in the order written, the
/// last of them partial for a run that is not whole words.
fn wire_words(r: &Received) -> Vec<u16> {
    let mut words = r.framed.buffer.clone();
    words.push(r.framed.source);
    words.push(r.framed.check);
    words.split_off(words.len().saturating_sub(r.bits.div_ceil(16)))
}

/// The check register, [`check_on`], carried over one bit.
fn check_bit(check: u16, bit: bool) -> u16 {
    let fb = bit ^ (check >> 15 != 0);
    let next = check << 1;
    if fb { next ^ (1 | 1 << 2 | 1 << 15) } else { next }
}

/// From a frame's turn to Transmit Done when the transmit count is 0 and
/// there is nothing to send --- 254 words written, the source and check
/// words taking the count round to 0: `TSTART` at the count, `TBUSY` 93 ns
/// on and `TDONE` 125 ns after that, and nothing on the line. Measured on
/// the netlist board.
pub const EMPTY_TDONE_NS: u64 = 218;

/// The most words a frame can be written with and go out whole, the
/// source and check words after them: 253, 4,080 bits.
const LONGEST_WRITTEN: usize = BUFFER_WORDS - 3;

/// How often the cable is looked at while nothing is known to be due: a
/// node with a frame to send is asked only then, so this bounds the extra
/// delay before a host's answer starts, well under the board's own.
const POLL_NS: u64 = 10_000;

// --- The turn timer, page LMTURN, as the netlist board runs it -----------

/// The divider at LMTURN 0A16, a 74LS161 on `FCLK/2^` reloading 14 from
/// its own terminal count: a terminal count every 500 ns.  The 74S112 at
/// 0D14 toggles on each while the cable is idle (`J` high, `K` on
/// `-CBLBSY`), and is held set while the cable is busy and by the load, so
/// `MY.TURN CLK^` rises at every second terminal count of an idle stretch
/// and the 74LS193s count down on it.
pub const TURN_TC_NS: u64 = 500;

/// From the I/O board's power-on to its first terminal count, the divider
/// counting from its power-on state.  Measured on the netlist board:
/// `MY.TURN CLK^` rises at 4,250 ns and every 1,000 from there while the
/// cable is idle, and `MY.TURN^`, bit 7 of the counter, falls at 132,250,
/// the 129th count.
pub const TURN_FIRST_TC_NS: u64 = 4_250;

/// From a frame's first edge on the cable to `-LOAD.MY.TURN`: `SRC STB`
/// at the 33rd bit cell, the destination and the source words received.
/// Measured, 8,250.
pub const TURN_LOAD_NS: u64 = 33 * wire::CELL_NS;

/// From the count that raises bit 7 to the frame's first edge on the
/// cable, `TSTART` through the transmit clock.  Measured on the netlist
/// board, 468; `the_models_instants_are_the_boards` in
/// `tests/chaos_netlist.rs` holds this and the next three.
pub const TURN_START_NS: u64 = 468;

/// From a frame's nominal end --- its first edge plus a cell for each bit
/// of [`crate::chaos::packet::frame`], the zero appended at the end
/// included --- to `-CBLBSY` lifting: the last edge 125 before that end,
/// the busy one-shot 500 after the last edge.  Measured.  `RDONE` rises
/// with it.
pub const CBLBSY_OFF_NS: u64 = 375;

/// Transmit Done comes this much before the frame's nominal end, as
/// [`CBLBSY_OFF_NS`] reckons it: `-TDONE` 375 ns before the last edge,
/// which is 125 before that end.  Measured.
pub const TDONE_BEFORE_END_NS: u64 = 500;

/// From a WRITE BUFFER's or a START's answer, `-SSYN`, to the earliest
/// instant the transmitter's shift register takes what the cycle gives
/// it: the register loads (`UB>TSR`, `TSREMPTY` falling) at the first edge
/// of the board's 8 MHz `FCLK^` at least this long after the answer ---
/// `-MSYN` lifts 100 ns after `-SSYN`, and the load waits for the clock.
/// Measured on the netlist board at every nanosecond of an edge's period:
/// an answer 101 ns before an edge loads at it, one 100 ns before loads at
/// the next.
pub const TSR_LOAD_NS: u64 = 101;

/// What one word takes to shift from the register into the buffer, its
/// sixteen bits on `FCLK^`: `TSREMPTY` rises this long after the load.
/// Measured on the netlist board.
pub const TSR_WORD_NS: u64 = 2_000;

/// When the transmitter's shift register is empty again, `TSREMPTY` up,
/// after a cycle answered at `answer` loads `words` words into it: one for
/// WRITE BUFFER, the source and check words for START. The load is at the
/// first edge of `FCLK^` at or after the answer plus [`TSR_LOAD_NS`], the
/// clock running free from `powered_at` with an edge every
/// [`crate::busint::IOB_FCLK_NS`], and each word takes [`TSR_WORD_NS`]. So
/// after an empty START, `TSREMPTY` comes 4,101 to 4,225 ns after the
/// answer, by the clock's phase there, and after one that waited for the
/// last word, whose answer is on the clock, always 4,150. In nanoseconds:
/// the CADR's I/O board's, which revisions 13 and 14 keep. Revision 15's
/// interface is QUUX's own and readies START [`TSR_READY_NS`] after the
/// read instead (MP2b ruling Q19).
///
/// `TSREMPTY` is what a frame waits for at its turn: `TSTART` is clocked
/// from `TSREMPTY AND CW AND -CBLBSY`. Measured on the netlist board by
/// `an_empty_start_makes_the_turn_the_board_makes_on_both_alike` in
/// `tests/chaos_rtl.rs`, answers swept at 10 ns steps across a rise of
/// bit 7, and by `the_models_instants_are_the_boards` in
/// `tests/chaos_netlist.rs`.
pub fn tsr_empty(answer: u64, words: u64, powered_at: u64) -> u64 {
    let edge = crate::busint::IOB_FCLK_NS;
    let from = (answer + TSR_LOAD_NS).saturating_sub(powered_at);
    powered_at + from.div_ceil(edge) * edge + words * TSR_WORD_NS
}

/// Revision 15's START: ready this long after the read, a delay started
/// at the read and ended ⌈2d/P⌉ clocks after it (MP2b ruling Q19). It is
/// the CADR board's `TSREMPTY` after a START that waited for the last
/// word's shift, as MIT's driver's always does ([`tsr_empty`]); QUUX's
/// interface is its own hardware and does not follow the board's 8 MHz
/// clock.
pub const TSR_READY_NS: u64 = 4_150;

/// From `ABORT` at LMMODU 0A09 setting --- the transmitter's next clock
/// edge after its transceiver's interference, `TABORTED` with it --- to
/// `TBUSY` down and Transmit Done.  Measured on the netlist board,
/// `tests/chaos_netlist.rs`: 125.
pub const TDONE_AFTER_ABORT_NS: u64 = 125;

/// The turn timer.  A terminal count at a time: the counter is loaded
/// with [`turn_byte`] of every frame's source 33 cells into the frame,
/// held while the cable is busy, counted down on every second terminal
/// count while it is idle, and a frame ready starts [`TURN_START_NS`]
/// after the count that carries the low byte from 0 to 255 --- bit 7's
/// rise, the 74S74 at LMMODU 0B15 clocking `TSTART` from `TSREMPTY AND CW AND
/// -CBLBSY`.  Only the low byte matters to bit 7, so only it is kept.
#[derive(Clone, Debug)]
struct Turn {
    powered_at: u64,
    /// Terminal counts taken; the next is at
    /// `powered_at + TURN_FIRST_TC_NS + TURN_TC_NS * taken`.
    taken: u64,
    /// `MY.TURN CLK^`.
    q: bool,
    /// The counter's low byte.
    low: u8,
    /// Frames seen starting on the cable, this interface's own included:
    /// first edge, source, nominal end.  Kept while they can matter.
    frames: Vec<(u64, u16, u64)>,
    /// A packet started: `TSREMPTY` at, and the words its transmit buffer
    /// will play out, the source and check word last
    /// ([`super::packet::line`]).
    ready: Option<(u64, Vec<u16>)>,
}

impl Turn {
    fn new(powered_at: u64) -> Turn {
        Turn { powered_at, taken: 0, q: false, low: 0, frames: Vec::new(), ready: None }
    }

    /// The true edge of terminal count `k`, in the machine's units.
    fn edge(&self, k: u64, time: TimeBase) -> u64 {
        self.powered_at + time.ns(TURN_FIRST_TC_NS + TURN_TC_NS * k)
    }

    /// When the next terminal count is acted on: its true edge, and on
    /// revision 15 the first clock at or after it, the divider running
    /// free from power-on and keeping its phase (MP2b ruling Q19).
    fn next_tc(&self, time: TimeBase) -> u64 {
        time.grid(self.edge(self.taken, time))
    }

    /// `-CBLBSY` down at `t`: a frame's first edge is past and its busy
    /// one-shot has not run out.
    fn busy_at(&self, t: u64, time: TimeBase) -> bool {
        self.frames.iter().any(|&(s, _, e)| s <= t && t < e + time.ns(CBLBSY_OFF_NS))
    }

    /// Takes the terminal count at `t`, for the interface at `me`: the
    /// instant [`Turn::next_tc`] gave, where the count is acted on.
    /// Returns the instant a frame starts, if this count starts one:
    /// [`TURN_START_NS`] on, a delay the count starts, so on revision 15
    /// ⌈2·468/P⌉ clocks after the count's clock (MP2b ruling Q19).
    fn tc(&mut self, t: u64, me: u16, time: TimeBase) -> Option<u64> {
        let prev = self.taken.checked_sub(1).map(|k| time.grid(self.edge(k, time)));
        for &(s, source, _) in &self.frames {
            let load = s + time.ns(TURN_LOAD_NS);
            if load <= t && prev.is_none_or(|p| load > p) {
                self.low = turn_byte(source, me);
                self.q = true;
            }
        }
        let mut starts = None;
        if self.busy_at(t, time) {
            self.q = true;
        } else {
            self.q = !self.q;
            if self.q {
                let rises = self.low == 0;
                self.low = self.low.wrapping_sub(1);
                if rises && self.ready.as_ref().is_some_and(|&(at, _)| at <= t) {
                    starts = Some(time.after(t, time.ns(TURN_START_NS)));
                }
            }
        }
        self.taken += 1;
        self.frames.retain(|&(_, _, e)| e + time.ns(CBLBSY_OFF_NS) >= t);
        starts
    }

    /// Every terminal count due at or before `now` at once, when a count
    /// can do nothing but arithmetic: returns whether it took any.
    ///
    /// With no frame on the cable there is nothing to load the counter
    /// from and `-CBLBSY` is down, and with nothing ready to send a carry
    /// out of the low byte starts nothing.  So all a count does is toggle
    /// `Q` and, on the counts that leave it set, take the low byte down
    /// one --- which over a run of `n` counts is `Q` toggled `n` times and
    /// the byte down by however many of them left `Q` set.  Nothing can
    /// observe the counts in between: what the counter holds is read only
    /// through when a frame starts, and a frame cannot start from here.
    ///
    /// This is worth doing because the count is due most of the time.
    /// [`TURN_TC_NS`] is 500 ns against a microcycle of about 180, so an
    /// engine calling [`Interface::advance`] every microcycle takes a
    /// terminal count about twice every five of them whatever else is
    /// happening.  The unit tests below hold a batched run to a stepped
    /// one and hold the refusals to refusing.
    ///
    /// On revision 15 a count is taken at the first clock at or after its
    /// edge, so the counts due by `now` are those whose edges are at or
    /// before the last clock at or before it.
    fn idle_run(&mut self, now: u64, time: TimeBase) -> bool {
        if !self.frames.is_empty() || self.ready.is_some() || self.next_tc(time) > now {
            return false;
        }
        let first = self.edge(self.taken, time);
        let n = (time.grid_floor(now) - first) / time.ns(TURN_TC_NS) + 1;
        let down = if self.q { n / 2 } else { n.div_ceil(2) };
        self.low = self.low.wrapping_sub(down as u8);
        self.q ^= n % 2 == 1;
        self.taken += n;
        true
    }
}

/// A frame the receiver had taken and a clear dropped before it landed:
/// a Clear Receiver, a Reset or `-UB INIT` written after the frame's
/// `START^` and before Receive Done would have risen
/// ([`Interface::dropped`]). The receiver takes every frame from its
/// `START^` with the buffer free and judges its destination word as it goes
/// by, so a frame for another interface cleared inside is one of these
/// too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dropped {
    /// The clear's instant.
    pub at: u64,
    /// The frame's source word as sent, as the turn timer saw the frame
    /// start; `None` for a frame the interface's turn timer did not see
    /// start, the netlist board's on the same cable.
    pub source: Option<u16>,
    /// When the frame would have landed --- Receive Done rising with
    /// `-CBLBSY` lifting, [`CBLBSY_OFF_NS`] after its nominal end --- had
    /// it not been cleared and had it been for this interface; `None` as
    /// for `source`.
    pub would_land: Option<u64>,
    /// How many of its bits the receiver had stored, which stay in the
    /// buffer.
    pub bits: usize,
}

pub struct Interface {
    /// The switches: [`interface::MY_ADDRESS`].
    address: u16,
    /// The read/write bits, [`WRITABLE`].
    csr: u16,
    transmit_done: bool,
    transmit_abort: bool,
    receive_done: bool,
    crc_error: bool,
    /// Packets that came while the buffer was full, four bits.
    lost: u8,
    /// The outgoing buffer, word `k` of a frame at `k mod 256`: the words
    /// written so far, round the buffer once 256 have been. How many have
    /// been written, and the check of every one of them, which the checker
    /// takes as each is shifted in.
    xmit: Vec<u16>,
    xmit_n: usize,
    xmit_check: u16,
    /// The incoming buffer, all 256 words, in the order the frame in it
    /// is read --- its words, destination, source, check, and on round the
    /// buffer to what was there before --- and how many reads it has had,
    /// and the check of the words read.
    rcv: Vec<u16>,
    rcv_at: usize,
    rcv_check: u16,
    /// The bits the receiver took for what is in the buffer, the trailing
    /// zero stripped: a whole number of words for a packet, and whatever
    /// came for wreckage, whose last partial word is read back padded.
    rcv_bits: usize,
    /// When Transmit Done comes for the frame being sent.
    tdone_at: Option<u64>,
    /// Frames off the cable, or looped back, each due at its `RDONE`, as
    /// the receiver takes them.
    incoming: VecDeque<(u64, Received)>,
    turn: Turn,
    /// When the cable was last looked at.
    polled: u64,
    /// The earliest instant anything this interface keeps time for can
    /// happen, as [`Interface::advance`] last worked it out: its turn
    /// timer's next terminal count, the cable's next due instant, the
    /// next frame landing, Transmit Done, and the next look at the cable.
    /// An `advance` short of it has nothing to do and says so at once,
    /// rather than asking all five again.  `None` is "not known", which
    /// is what everything that can bring an event forward leaves it as.
    next_event: Option<u64>,
    /// The time as last advanced to, for what the registers show of the
    /// cable at the instant they are read.
    now: u64,
    /// The cable, with whatever stations are on it; none, and a frame sent
    /// goes nowhere and nothing ever comes.
    ether: Option<Box<Ether>>,
    /// `--chaos-trace`: say what the receiver takes and drops, and when
    /// the transmitter's turn comes.
    trace: bool,
    /// The last frame a clear dropped, [`Interface::dropped`].
    dropped: Option<Dropped>,
    /// Whether the receiver has refused the run now on the cable, `RACT`
    /// down from its `START^` or since a clear, as the ether keeps it
    /// ([`Ether::board_refused`]), brought here at each of the cable's
    /// instants so that a copy without a cable still knows it. CRC Error
    /// does not read the cable busy then: the check register, the 9401 at
    /// LMRBUF 0C07, is clocked by `RICLK^`, which is `RACT` with the cell
    /// clock, and cleared by `RRESET`.
    cable_refused: bool,
    /// A frame looped back whose `START^` is to come: when, and when it
    /// lands if it is taken ([`Interface::looped_start`]).
    looped_start: Option<(u64, u64)>,
    /// The read register, the two 74164s at LMRBUF 0A07 and 0B07: the
    /// last word read, which a read with Receive Done down shifts on
    /// ([`Interface::read_down`]).
    rd: u16,
    /// A read with Receive Done down inside a frame the receiver is not
    /// taking, `RRESET` not held: the frame's first edge, `RBCT` as the
    /// reads have counted it down from 0, and the check of what they read.
    down: Option<(u64, u16, u16)>,
    /// A frame looped back that a clear took the receiver off before its
    /// destination word: when the word goes by, and the word, for the
    /// busy receiver's gates (`Interface::busy_looped`).
    looped_busy: Option<(u64, u16)>,
    /// The machine's unit of time and its clock's grid, which every
    /// instant here is counted in: nanoseconds with no grid, and on
    /// revision 15 units of 0.5 ns on its period (MP2b ruling Q19).
    /// [`Interface::set_time_base`]; not kept in a checkpoint, the
    /// machine setting it before it hands the interface a time.
    time: TimeBase,
}

impl Clone for Interface {
    /// The registers clone; the cable does not.  A copy of a machine has
    /// an interface with nothing on its cable.
    fn clone(&self) -> Interface {
        Interface {
            address: self.address,
            csr: self.csr,
            transmit_done: self.transmit_done,
            transmit_abort: self.transmit_abort,
            receive_done: self.receive_done,
            crc_error: self.crc_error,
            lost: self.lost,
            xmit: self.xmit.clone(),
            xmit_n: self.xmit_n,
            xmit_check: self.xmit_check,
            rcv: self.rcv.clone(),
            rcv_at: self.rcv_at,
            rcv_check: self.rcv_check,
            rcv_bits: self.rcv_bits,
            tdone_at: self.tdone_at,
            incoming: self.incoming.clone(),
            turn: self.turn.clone(),
            polled: self.polled,
            now: self.now,
            next_event: None,
            ether: None,
            trace: self.trace,
            time: self.time,
            dropped: self.dropped,
            cable_refused: self.cable_refused,
            looped_start: self.looped_start,
            rd: self.rd,
            down: self.down,
            looped_busy: self.looped_busy,
        }
    }
}

impl Interface {
    /// An interface with its switches at `address`, on `ether` if given,
    /// powered at `powered_at` --- where its turn timer's divider starts
    /// --- and as at power-up: the read/write bits zero, Transmit Done up
    /// so that the first transmission may start --- `CHAOS-XMT-INTR` waits
    /// for it --- and the receiver ready for a packet, as the netlist
    /// board's is after a reset alone (`tests/chaos_rtl.rs`): the driver's
    /// `INTERFACE-RESET-AND-ENABLE` writes Reset and the interrupt enables
    /// and nothing else.
    pub fn new(address: u16, ether: Option<Ether>, powered_at: u64, trace: bool) -> Interface {
        let mut ether = ether.map(Box::new);
        if let Some(e) = ether.as_mut() {
            e.attach_board(address);
        }
        Interface {
            address,
            csr: 0,
            transmit_done: true,
            transmit_abort: false,
            receive_done: false,
            crc_error: false,
            lost: 0,
            xmit: Vec::new(),
            xmit_n: 0,
            xmit_check: 0,
            rcv: vec![NEVER_WRITTEN; BUFFER_WORDS],
            rcv_at: 0,
            rcv_check: 0,
            rcv_bits: 0,
            tdone_at: None,
            incoming: VecDeque::new(),
            turn: Turn::new(powered_at),
            polled: powered_at,
            now: powered_at,
            next_event: None,
            ether,
            trace,
            time: TimeBase::NS,
            dropped: None,
            cable_refused: false,
            looped_start: None,
            rd: 0,
            down: None,
            looped_busy: None,
        }
    }

    /// Counts every instant in the machine's units on its clock's grid,
    /// `time`, from here on, and its cable with it. Nanoseconds with no
    /// grid until told otherwise, which is the CADR's and revisions 13
    /// and 14's.
    ///
    /// **On revision 15** (MP2b ruling Q19) the constants above and the
    /// cable's double into units of 0.5 ns, unchanged; the turn timer's
    /// terminal counts are acted on at the first clock at or after each
    /// true edge; and a delay the interface starts ends ⌈2d/P⌉ clocks
    /// after its start, rounded up once from its own start and its exact
    /// value ([`TimeBase::after`]): START's [`TSR_READY_NS`], the count's
    /// [`TURN_START_NS`], and from its own frame's first edge Transmit
    /// Done, the end of `-CBLBSY` and, looped back, Receive Done. A delay
    /// whose start the cable sets between clocks --- a frame from another
    /// station's Receive Done, busy window and load of the turn counter,
    /// Transmit Done after an abort --- ends at the first clock at or
    /// after its exact start plus its exact value, rounded up once and
    /// not first the start to a clock (MP2b ruling Q19, its clarification
    /// of a delay started off the clock's grid): kept here exactly, and
    /// acted on at the first clock that reaches it.
    pub fn set_time_base(&mut self, time: TimeBase) {
        if self.time != time {
            self.time = time;
            self.invalidate();
            if let Some(e) = self.ether.as_mut() {
                e.set_per_ns(time.per_ns);
            }
        }
    }

    pub fn address(&self) -> u16 {
        self.address
    }

    pub fn ether(&self) -> Option<&Ether> {
        self.ether.as_deref()
    }

    /// The cable, to be configured: [`Ether::keep_log`] is off by default,
    /// and a test that reads the log turns it on through here.
    pub fn ether_mut(&mut self) -> Option<&mut Ether> {
        self.invalidate();
        self.ether.as_deref_mut()
    }

    /// Plugs `ether` into this interface's cable, in place of whatever was
    /// there.
    pub fn plug(&mut self, mut ether: Ether) {
        ether.attach_board(self.address);
        ether.set_per_ns(self.time.per_ns);
        self.ether = Some(Box::new(ether));
        self.invalidate();
        self.tell_the_cable();
    }

    /// The CSR as read: AIM-628 §7's bits.
    pub fn csr(&self) -> u16 {
        self.csr
            | if self.transmit_abort { csr::TRANSMIT_ABORT } else { 0 }
            | if self.transmit_done { csr::TRANSMIT_DONE } else { 0 }
            | ((self.lost as u16) << 9) & csr::LOST_COUNT
            | if self.crc_error
                || self.taking()
                || self.down_here().is_some_and(|(_, _, check)| check != 0)
            {
                csr::CRC_ERROR
            } else {
                0
            }
            | if self.receive_done { csr::RECEIVE_DONE } else { 0 }
    }

    /// "The number of bits in the incoming packet buffer, minus one.
    /// After the whole packet has been read out, it will contain 7777."
    /// Before any packet, and after a Reset or a Clear Receiver, the
    /// netlist board reads 0 (`tests/cadrio_netlist.rs`).
    ///
    /// It is `RBCT` less one, in twelve bits. The receiver leaves `RBCT` at
    /// the bits it took, round the buffer if there were more than 4,096 ---
    /// 15 for 4,112. The first read takes it down to the word boundary
    /// below the last bit, the bits there are for a partial word at the
    /// top --- the netlist board reads 15 with one whole word left of 222
    /// bits (`tests/chaos_rtl.rs`) --- and each read after takes sixteen,
    /// past `7777` and round, never stopping
    /// (`reading_on_goes_round_the_buffer_on_both_alike`).
    ///
    /// **With Receive Done down** it is `RBCT` as it stands, which the
    /// 74LS244s at LMDATP 0C17 put on the bus: the bits stored so far of a
    /// frame the receiver is taking, [`Interface::stored_now`]; inside a
    /// frame it is not taking, what reads have counted it down to; and 0
    /// otherwise, `RRESET` holding it
    /// (`a_read_with_receive_done_down_is_the_read_paths_on_both_alike`,
    /// `the_bit_count_while_a_frame_comes_in_on_both_alike`).
    pub fn bit_count(&self) -> u16 {
        if !self.receive_done {
            if self.taking() {
                return (self.stored_now() % BUFFER_BITS) as u16;
            }
            return self.down_here().map_or(0, |(_, hw, _)| hw);
        }
        let rbct = match self.rcv_at {
            0 => self.rcv_bits % BUFFER_BITS,
            k => {
                (16 * top_word(self.rcv_bits) + BUFFER_BITS - 16 * ((k - 1) % BUFFER_WORDS))
                    % BUFFER_BITS
            }
        };
        ((rbct + BUFFER_BITS - 1) % BUFFER_BITS) as u16
    }

    /// Read `k` of the frame in the incoming buffer: the buffer's words
    /// round and round from the frame's first, the first read with zeros
    /// above the frame's last bit when that is inside a word. The bits
    /// above it in the buffer --- an earlier frame's, or the same frame's a
    /// lap before --- the first read does not see, and read 256 does
    /// (`reading_on_goes_round_the_buffer_on_both_alike`,
    /// `a_frame_longer_than_the_buffer_keeps_its_last_4096_bits_on_both_alike`).
    fn rcv_word(&self, k: usize) -> u16 {
        let w = self.rcv[k % BUFFER_WORDS];
        let pad = (16 - self.rcv_bits % 16) % 16;
        if k == 0 { w & (u16::MAX >> pad) } else { w }
    }

    /// A frame of `bits` bits, whose words read back whole are `words`,
    /// into the incoming buffer. Every bit goes in at `RBCT`, from 0, and
    /// takes it on one, so past the 4,096th a bit goes over the first
    /// ones and the buffer holds the frame's last 4,096. A frame comes check
    /// word first and the words the software reads first last, so those
    /// are what is kept; and a partial top word goes over only the bits it
    /// has, the rest of that word holding what it held.
    fn land(&mut self, words: &[u16], bits: usize) {
        let was = top_word(self.rcv_bits);
        let mut ram: Vec<u16> =
            (0..BUFFER_WORDS).map(|j| self.rcv[(was + BUFFER_WORDS - j) % BUFFER_WORDS]).collect();
        let n = words.len();
        let pad = (16 - bits % 16) % 16;
        for k in (0..n).rev() {
            let j = (n - 1 - k) % BUFFER_WORDS;
            ram[j] = if k == 0 && pad > 0 {
                let low = u16::MAX >> pad;
                (ram[j] & !low) | (words[0] & low)
            } else {
                words[k]
            };
        }
        let top = top_word(bits);
        self.rcv =
            (0..BUFFER_WORDS).map(|k| ram[(top + BUFFER_WORDS - k) % BUFFER_WORDS]).collect();
        self.rcv_bits = bits;
        self.rcv_at = 0;
        self.rcv_check = 0;
    }

    /// The Unibus interrupt this interface is requesting, if it is.
    pub fn interrupt_request(&self) -> Option<u16> {
        let receive = self.receive_done && self.csr & csr::RECEIVE_INT_ENABLE != 0;
        let transmit = self.transmit_done && self.csr & csr::TRANSMIT_INT_ENABLE != 0;
        (receive || transmit).then_some(VECTOR)
    }

    /// Time passes to `now`, an event at a time in their order: the
    /// cable's, the turn timer's terminal counts, frames landing, Transmit
    /// Done; then a look at the cable when one is owed.
    ///
    /// The engines call this every microcycle, so it says as early as it
    /// can that there is nothing to do: `next_event` is the
    /// earliest of the five instants below as they last stood, and an
    /// `advance` short of it returns without asking any of them again.
    /// Everything that can bring one of the five forward leaves it `None`.
    pub fn advance(&mut self, now: u64) {
        self.now = self.now.max(now);
        if self.next_event.is_some_and(|t| now < t) {
            return;
        }
        let time = self.time;
        loop {
            let tc = self.turn.next_tc(time);
            let cable = self.ether.as_ref().and_then(|e| e.next_due());
            let landing = self.incoming.front().map(|&(t, _)| t);
            let looped = [self.looped_busy.map(|(t, _)| t), self.looped_start.map(|(t, _)| t)]
                .into_iter()
                .flatten()
                .min();
            let due = [Some(tc), cable, landing, self.tdone_at, looped]
                .into_iter()
                .flatten()
                .filter(|&t| t <= now)
                .min();
            let Some(t) = due else { break };
            // A run of terminal counts is arithmetic and nothing else:
            // [`Turn::idle_run`] takes it whole, stopping short of
            // whatever else is coming.  Another event at `t` itself
            // leaves nothing to batch --- the run would have to end at
            // `t - 1`, before the count --- so `idle_run` refuses and the
            // arms below take the instant as they always did.
            let others = [cable, landing, self.tdone_at, looped].into_iter().flatten().min();
            if tc == t
                && self.turn.idle_run(others.map_or(now, |u| u.saturating_sub(1).min(now)), time)
            {
                continue;
            }
            if cable == Some(t)
                && let Some(e) = self.ether.as_mut()
            {
                e.at(t);
                self.take_from_cable();
            }
            if landing == Some(t) {
                let (_, r) = self.incoming.pop_front().unwrap();
                self.arrive(t, &r);
            }
            if self.looped_start.is_some_and(|(at, _)| at == t) {
                let (_, land) = self.looped_start.take().unwrap();
                self.looped_start(t, land);
            }
            if self.looped_busy.is_some_and(|(at, _)| at == t) {
                let (_, dest) = self.looped_busy.take().unwrap();
                self.busy_looped(t, dest);
            }
            if self.tdone_at == Some(t) {
                self.tdone_at = None;
                self.transmit_done = true;
            }
            if tc == t
                && let Some(start) = self.turn.tc(t, self.address, time)
            {
                self.launch(start, t);
            }
        }
        if let Some(e) = self.ether.as_mut()
            && now >= self.polled + time.ns(POLL_NS)
        {
            e.at(now);
            self.polled = now;
            self.take_from_cable();
        }
        // The five, as they stand now.  The look at the cable is one of
        // them: it is how a node with a frame to send is asked, so an
        // interface with a cable is never left with nothing due.
        let cable = self.ether.as_ref().and_then(|e| e.next_due());
        let poll = self.ether.as_ref().map(|_| self.polled + time.ns(POLL_NS));
        let looped = [self.looped_busy.map(|(t, _)| t), self.looped_start.map(|(t, _)| t)]
            .into_iter()
            .flatten()
            .min();
        self.next_event = [Some(self.turn.next_tc(time)), cable, poll, self.tdone_at, looped]
            .into_iter()
            .flatten()
            .chain(self.incoming.front().map(|&(t, _)| t))
            .min();
    }

    /// The cable is told how the receiver stands.  The busy receiver's
    /// answer to a frame --- counted, and aborted if it was addressed to
    /// this interface by name --- is decided as the frame's destination
    /// word goes by, microseconds before the interface hears of the frame
    /// at all, so the ether is given Receive Done and Spy whenever either
    /// moves: a packet taken, a Clear Receiver, a CSR write, a reset.
    /// [`Interface::read`] and [`Interface::write`] advance the cable
    /// before they change anything, so what the ether decided up to that
    /// instant it decided with the state that stood then.
    fn tell_the_cable(&mut self) {
        let (receive_done, spy) = (self.receive_done, self.csr & csr::SPY != 0);
        if let Some(e) = self.ether.as_mut() {
            e.board_receiver(receive_done, spy);
        }
    }

    /// Whatever the interface last worked out about when it is next due is
    /// no longer to be trusted: something outside [`Interface::advance`]
    /// has touched the cable, the buffers or the turn timer.
    fn invalidate(&mut self) {
        self.next_event = None;
    }

    /// What the cable has for this interface: frames that started, for the
    /// turn timer and for Transmit Done of its own; frames decoded, to land
    /// at their `RDONE`.  Under Loop Back the receiver is on the
    /// transmitter, not the cable.
    fn take_from_cable(&mut self) {
        let time = self.time;
        let Some(e) = self.ether.as_mut() else { return };
        self.cable_refused = e.board_refused();
        let loop_back = self.csr & csr::LOOP_BACK != 0;
        while let Some((start, source, end)) = e.board_cable_frame() {
            if source == self.address {
                // From the frame's first edge, which the turn timer put
                // on a clock: rounded once from there, never from the
                // nominal end.
                let d = (end - start).saturating_sub(time.ns(TDONE_BEFORE_END_NS));
                self.tdone_at = Some(time.after(start, d));
                if self.trace {
                    eprintln!(
                        "chaos {start:>6}: interface {:o} put its frame on the cable, to {end}",
                        self.address
                    );
                }
            }
            if !loop_back || source == self.address {
                self.turn.frames.push((start, source, end));
            }
        }
        // A transmitter aborted at `t`: its frame ends with its abort
        // signal, and `-CBLBSY` lifts from there rather than from the
        // nominal end.  The board's own, if it is: `ABORT` ends `TBUSY`,
        // `TABORTED` reads back as Transmit Abort, and Transmit Done comes
        // [`TDONE_AFTER_ABORT_NS`] on.
        while let Some((t, source, end)) = e.board_collision() {
            if let Some(f) =
                self.turn.frames.iter_mut().rev().find(|f| f.1 == source && f.0 <= t && f.2 > t)
            {
                f.2 = end;
            }
            if source == self.address {
                self.transmit_abort = true;
                self.tdone_at = Some(t + time.ns(TDONE_AFTER_ABORT_NS));
                if self.trace {
                    eprintln!(
                        "chaos {t:>6}: interface {:o}'s frame aborted on interference",
                        self.address
                    );
                }
            }
        }
        // What the busy receiver counted while the interface was not
        // looking: the ether times it at the instant the frame's
        // destination word went by, [`super::ether::BUSY_ABORT_NS`], and
        // drives the abort signal for what was addressed to this
        // interface by name.  Lost Count is the 74LS161 at LMMYNM 0F04
        // and wraps at sixteen (`the_lost_count_wraps_at_sixteen` in
        // `tests/chaos_netlist.rs`).
        let mut counted = Vec::new();
        while let Some(x) = e.board_lost() {
            counted.push(x);
        }
        for (t, source, aborted) in counted {
            self.lost = (self.lost + 1) & 0o17;
            if self.trace {
                eprintln!(
                    "chaos {t:>6}: interface {:o} {} a frame from {:o}, its buffer full ({} lost)",
                    self.address,
                    if aborted { "aborted" } else { "lost" },
                    source,
                    self.lost
                );
            }
        }
        let Some(e) = self.ether.as_mut() else { return };
        while let Some((at, r, taken)) = e.board_heard() {
            // A frame that found the receiver inactive --- `RACT` off with
            // Receive Done up --- puts nothing in the buffer and has been
            // counted already.
            if loop_back || !taken {
                continue;
            }
            let from = r.framed.source;
            // `RDONE` with `-CBLBSY` lifting, after the frame's end.
            let end = self
                .turn
                .frames
                .iter()
                .filter(|&&(s, source, _)| source == from && s <= at)
                .map(|&(_, _, e)| e)
                .max();
            let land = end.map_or(at, |e| e + time.ns(CBLBSY_OFF_NS));
            self.incoming.push_back((land, r));
            self.incoming.make_contiguous().sort_by_key(|&(t, _)| t);
        }
    }

    /// The turn has come at terminal count `t`: the packet ready goes out
    /// from `start`, on the cable, or back to this receiver under Loop
    /// Back, or into the void with no cable. With nothing in the buffer to
    /// send, nothing goes and Transmit Done comes [`EMPTY_TDONE_NS`] after
    /// the count.
    fn launch(&mut self, start: u64, t: u64) {
        let Some((ready_at, line)) = self.turn.ready.take() else { return };
        let time = self.time;
        if line.is_empty() {
            self.tdone_at = Some(time.after(t, time.ns(EMPTY_TDONE_NS)));
            if self.trace {
                eprintln!(
                    "chaos {t:>6}: interface {:o}'s turn, its frame ready since {ready_at}, and nothing to send",
                    self.address
                );
            }
            return;
        }
        let cells = super::packet::line(&line);
        let bits = cells.len() as u64;
        let length = bits * time.ns(wire::CELL_NS);
        let end = start + length;
        if self.trace {
            eprintln!(
                "chaos {t:>6}: interface {:o}'s turn, its frame ready since {ready_at}, {bits} bits from {start}",
                self.address
            );
        }
        if self.csr & csr::LOOP_BACK != 0 || self.ether.is_none() {
            self.turn.frames.push((start, self.address, end));
            self.tdone_at = Some(time.after(start, length - time.ns(TDONE_BEFORE_END_NS)));
            if self.csr & csr::LOOP_BACK != 0 {
                let r = unframe_any(&cells);
                let land = time.after(start, length + time.ns(CBLBSY_OFF_NS));
                self.incoming.push_back((land, r));
                self.incoming.make_contiguous().sort_by_key(|&(t, _)| t);
                self.looped_start = Some((time.after(start, time.ns(RACT_NS)), land));
            }
        } else if let Some(e) = self.ether.as_mut() {
            e.board_send(line, start);
        }
    }

    /// A frame off the cable, wreckage included: taken if it is for this
    /// interface --- its address, a broadcast, or anything under Spy ---
    /// and the buffer is free.  The destination is matched as it came on
    /// the wire; wreckage too short to carry one matched nothing.
    ///
    /// **A frame that finds the buffer full never gets here off the
    /// cable.**  The board decides that as the frame's destination word
    /// goes by, twelve microseconds in and long before it lands:
    /// `RACT` is off with Receive Done up, so the receiver stores nothing,
    /// the frame is counted in Lost Count, and one addressed to this
    /// interface by name is aborted on the cable
    /// ([`super::ether::Ether::board_lost`], and
    /// `the_busy_receiver_aborts_a_frame_addressed_to_it` in
    /// `tests/chaos_netlist.rs`).  That holds whoever put the frame
    /// there: a model transmitter's carries the instants itself, and a
    /// netlist board's on the same cable is timed from its transceiver's
    /// first edge, which
    /// `a_busy_model_receiver_stops_the_netlist_board_which_reads_transmit_abort`
    /// in `tests/chaos_two_boards.rs` measures.  What can still arrive
    /// with the buffer full is a frame looped back inside the interface,
    /// where the cable is unused and the ether sees nothing: it is
    /// counted here.
    ///
    /// **A run of 48 bits or fewer is not taken**, the check, source and
    /// destination words and nothing after them, though its destination
    /// matched: the board's own frame of 48 bits, 257 words written, does
    /// not land, and of 64, 258 written, does
    /// (`a_frame_written_past_the_buffer_wraps_on_both_alike` in
    /// `tests/chaos_rtl.rs`).
    ///
    /// **A frame taken that does not land leaves its bits in the buffer**:
    /// the receiver stored them as they came, from `RBCT` 0, and the
    /// destination word decided only whether it lands
    /// (`a_frame_taken_and_not_landed_leaves_its_bits_on_both_alike`).
    fn arrive(&mut self, now: u64, r: &Received) {
        let lands = r.bits > 48
            && r.framed
                .buffer
                .last()
                .is_some_and(|&dest| dest == self.address || dest == 0 || self.csr & csr::SPY != 0);
        if !lands {
            if !self.receive_done {
                self.land(&wire_words(r), r.bits);
            }
            return;
        }
        let (f, bits) = (&r.framed, r.bits);
        if self.receive_done {
            // Off the cable this cannot happen; looped back it can, and
            // Lost Count is a 74LS161 that wraps at sixteen.
            self.lost = (self.lost + 1) & 0o17;
            if self.trace {
                eprintln!(
                    "chaos {now:>6}: interface {:o} lost a frame from {:o}, its buffer full ({} lost)",
                    self.address, f.source, self.lost
                );
            }
            return;
        }
        if self.trace {
            eprintln!(
                "chaos {now:>6}: interface {:o} took a frame from {:o}, {} words{}",
                self.address,
                f.source,
                f.buffer.len(),
                if self.csr & csr::RECEIVE_INT_ENABLE != 0 {
                    ", interrupting"
                } else {
                    ", no interrupt enabled"
                }
            );
        }
        let mut words = f.buffer.clone();
        words.push(f.source);
        words.push(f.check);
        self.land(&words, bits);
        self.crc_error = !f.check_ok;
        self.receive_done = true;
        // `RO.FIRST^`, the 26S02 at LMRCTL 0A04 on Receive Done, clears the
        // read register.
        self.rd = 0;
        self.tell_the_cable();
    }

    /// A read of one of the registers at `now`.
    pub fn read(&mut self, uaddr: u32, now: u64) -> u16 {
        self.advance(now);
        self.invalidate();
        match uaddr {
            interface::CSR => {
                if self.trace && self.receive_done {
                    eprintln!(
                        "chaos {now:>6}: interface {:o} read its CSR with a packet waiting, {} of {} words read",
                        self.address,
                        self.rcv_at,
                        self.rcv_bits.div_ceil(16)
                    );
                }
                self.csr()
            }
            interface::MY_ADDRESS => self.address,
            // **CRC Error from the first read on is the check of the words
            // read**, [`check_word`] over them not zero: the read-out runs
            // the buffer through the checker, which AIM-628 §7 makes right
            // "when the packet has been completely read out". So it is up
            // partway through a good frame and down at its end, and a
            // frame that lost its check word to the wrap reads out with it
            // up. The check is carried from read to read.
            interface::READ_BUFFER if self.receive_done => {
                let w = self.rcv_word(self.rcv_at);
                self.rcv_at += 1;
                self.rcv_check = check_on(self.rcv_check, &[w]);
                self.crc_error = self.rcv_check != 0;
                self.rd = w;
                w
            }
            interface::READ_BUFFER => self.read_down(),
            interface::BIT_COUNT => self.bit_count(),
            interface::START => {
                self.start(now);
                self.address
            }
            _ => 0o177777,
        }
    }

    /// A write of one of the registers at `now`.
    pub fn write(&mut self, uaddr: u32, v: u16, now: u64) {
        self.advance(now);
        self.invalidate();
        match uaddr {
            interface::CSR => {
                let looped = self.csr & csr::LOOP_BACK != 0;
                self.csr = v & WRITABLE;
                if v & csr::RESET != 0 {
                    self.reset_from(looped);
                }
                if v & csr::CLEAR_RECEIVER != 0 {
                    self.rreset(looped);
                    if self.trace {
                        eprintln!(
                            "chaos {now:>6}: interface {:o} cleared its receiver, {} of {} words read{}",
                            self.address,
                            self.rcv_at,
                            self.rcv_bits.div_ceil(16),
                            if self.receive_done { "" } else { ", buffer already free" }
                        );
                    }
                    self.receive_done = false;
                    self.crc_error = false;
                    self.rcv_at = 0;
                    self.rcv_check = 0;
                    self.lost = 0;
                }
                if v & csr::CLEAR_TRANSMITTER != 0 {
                    self.xmit.clear();
                    self.xmit_n = 0;
                    self.xmit_check = 0;
                    self.turn.ready = None;
                    self.tdone_at = None;
                    self.transmit_abort = false;
                    self.transmit_done = true;
                }
            }
            interface::WRITE_BUFFER => {
                if self.trace && self.xmit.is_empty() {
                    eprintln!(
                        "chaos {now:>6}: interface {:o} began filling its transmit buffer",
                        self.address
                    );
                }
                // The word goes in at `TBCT`, which takes it on sixteen in
                // twelve bits: past the 256th word it goes over the first
                // ([`BUFFER_WORDS`]). The checker takes it as it shifts in.
                let at = self.xmit_n % BUFFER_WORDS;
                if at < self.xmit.len() {
                    self.xmit[at] = v;
                } else {
                    self.xmit.push(v);
                }
                self.xmit_n += 1;
                self.xmit_check = check_on(self.xmit_check, &[v]);
                self.transmit_done = false;
                self.transmit_abort = false;
            }
            _ => {}
        }
        self.tell_the_cable();
    }

    /// Reset (write only): "completely resets the interface, just as at
    /// power up and Unibus Initialize."  The turn timer is not in it: the
    /// counter's clear is grounded and the divider runs free.
    ///
    /// **A frame the receiver is taking is dropped**, as by Clear Receiver
    /// ([`Interface::rreset`]): Reset and `-UB INIT` make `-INIT`, the
    /// 74S08 at LMUCON 0B10, which is the other input of `-CLR RCV BUSY`.
    /// The interface acts at the instant it was last advanced to.
    ///
    /// **Lost Count stays as it is**: its 74LS161 at LMMYNM 0F04 clears on
    /// `-11.SAYS.GO` alone, Clear Receiver, and `-INIT` reaches only
    /// `-CLR RCV BUSY` and Transmit Done
    /// (`a_reset_leaves_lost_count_where_it_was_on_both_alike` in
    /// `tests/chaos_rtl.rs`).
    pub fn reset(&mut self) {
        self.reset_from(self.csr & csr::LOOP_BACK != 0);
    }

    /// [`Interface::reset`] from a CSR that had Loop Back as `looped`.
    fn reset_from(&mut self, looped: bool) {
        self.invalidate();
        self.rreset(looped);
        self.looped_busy = None;
        self.looped_start = None;
        // Loop Back goes off with the CSR: the receiver no longer hears the
        // transmitter, and the frame looped back is over for it at once.
        if looped {
            let (now, off, me) = (self.now, self.time.ns(CBLBSY_OFF_NS), self.address);
            for f in
                self.turn.frames.iter_mut().filter(|f| f.1 == me && f.0 <= now && now < f.2 + off)
            {
                f.2 = now.saturating_sub(off).max(f.0);
            }
        }
        self.csr = 0;
        self.transmit_done = true;
        self.transmit_abort = false;
        self.receive_done = false;
        self.crc_error = false;
        self.xmit.clear();
        self.xmit_n = 0;
        self.xmit_check = 0;
        self.rcv_at = 0;
        self.rcv_check = 0;
        self.tdone_at = None;
        self.incoming.clear();
        self.turn.ready = None;
        self.tell_the_cable();
    }

    /// Whether the receiver is taking the frame on the line now: there is
    /// one, and it has not refused it --- off the cable as the ether has
    /// it, looped back the frame still waiting to land. Before `START^`
    /// the frame counts as taken, as CRC Error has always read it.
    fn taking(&self) -> bool {
        self.frame_now().is_some()
            && if self.csr & csr::LOOP_BACK != 0 {
                !self.incoming.is_empty()
            } else {
                !self.cable_refused
            }
    }

    /// The frame on the line now, from the interface's turn timer: its
    /// first edge and nominal end.
    fn frame_now(&self) -> Option<(u64, u64)> {
        let off = self.time.ns(CBLBSY_OFF_NS);
        self.turn
            .frames
            .iter()
            .rev()
            .find(|&&(s, _, e)| s <= self.now && self.now < e + off)
            .map(|&(s, _, e)| (s, e))
    }

    /// `RBCT` while the receiver takes the frame on the line: a step on
    /// each `RICLK^`, which is the cell clock with `RACT` and comes
    /// [`RACT_NS`] after each cell's opening edge as `START^` does --- one
    /// for every cell after the first, the trailing zero's last. Measured
    /// on the netlist board, `RBCT0` stepping 310, 560, 810 ns after the
    /// frame's first edge, and held by
    /// `the_bit_count_while_a_frame_comes_in_on_both_alike`.
    fn stored_now(&self) -> usize {
        let (cell, late) = (self.time.ns(wire::CELL_NS), self.time.ns(RACT_NS));
        self.frame_now().map_or(0, |(s, e)| {
            let k = if self.now >= s + late { (self.now - s - late) / cell } else { 0 };
            (k as usize).min(((e - s) / cell) as usize - 1)
        })
    }

    /// The reads' count-down inside the frame on the line, if that is
    /// where the last of them was: `RRESET` is held whenever the receiver
    /// is neither taking a frame nor inside one.
    fn down_here(&self) -> Option<(u64, u16, u16)> {
        let (s, _) = self.frame_now()?;
        self.down.filter(|&(at, _, _)| at == s && !self.receive_done)
    }

    /// A read of the buffer with Receive Done down: the read register
    /// shifted on `ROCLK^`, the 2147's bit at `RBCT` coming in at the
    /// bottom and `RBCT` counting down on each shift (the 25LS193s at
    /// LMRBUF 0D04-0D06), until the count's low four bits borrow and
    /// `-ROWEND` ends the read (the 74279 at LMRCTL 0D09). The check
    /// register runs on each bit, the reads being made with `RACT` down.
    ///
    /// With the line idle `RRESET` holds `RBCT` at 0 and the check register
    /// clear, so a read shifts once: the last word read moved up a bit,
    /// with the buffer's first bit below it. Inside a frame the receiver is
    /// not taking --- cleared off it --- `RRESET` is not held, and the
    /// reads count `RBCT` down from 0 as a frame is read out: the first one
    /// bit, the rest sixteen. Measured by
    /// `a_read_with_receive_done_down_is_the_read_paths_on_both_alike`.
    ///
    /// **A read while the receiver is taking a frame is not modelled**: on
    /// the netlist board its cycle is not answered until the frame has
    /// ended, `ROCLK^` and `RICLK^` counting `RBCT` against each other, and
    /// what lands is spoiled. That reads as 0 here.
    fn read_down(&mut self) -> u16 {
        if self.taking() {
            return 0;
        }
        let Some((s, _)) = self.frame_now() else {
            self.down = None;
            self.rd = self.rd << 1 | self.ram_bit(0) as u16;
            return self.rd;
        };
        let (_, mut hw, mut check) = self.down_here().unwrap_or((s, 0, 0));
        loop {
            let bit = self.ram_bit(hw as usize);
            self.rd = self.rd << 1 | bit as u16;
            check = check_bit(check, bit);
            hw = (hw + BUFFER_BITS as u16 - 1) % BUFFER_BITS as u16;
            if hw % 16 == 15 {
                break;
            }
        }
        self.down = Some((s, hw, check));
        self.rd
    }

    /// The incoming buffer's bit at `RBCT` value `at`, as the read path
    /// gives it: the bits of the last frame stored are from `RBCT` 0 up.
    fn ram_bit(&self, at: usize) -> bool {
        let word = self.rcv[(top_word(self.rcv_bits) + BUFFER_WORDS - at / 16) % BUFFER_WORDS];
        word >> (at % 16) & 1 != 0
    }

    /// `START^` of a frame looped back, landing at `land` if taken:
    /// `RACT` takes `-RDONE`. With the buffer full the receiver stays off
    /// for the frame --- nothing stored, CRC Error not clocked --- and its
    /// destination word goes to the busy receiver's gates, as off the
    /// cable (`a_frame_at_a_full_buffer_is_not_taken_looped_back_on_both_alike`).
    fn looped_start(&mut self, t: u64, land: u64) {
        if !self.receive_done {
            return;
        }
        let Some(k) = self.incoming.iter().position(|&(l, _)| l == land) else { return };
        let (_, r) = self.incoming.remove(k).unwrap();
        let start = t - self.time.ns(RACT_NS);
        if r.bits >= 48 {
            let dest_at = self.time.after(start, self.time.ns(BUSY_ABORT_NS));
            self.looped_busy = r.framed.buffer.last().map(|&dest| (dest_at, dest));
        }
    }

    /// The last frame a Clear Receiver, a Reset or `-UB INIT` dropped
    /// before it landed, if any has been: when, whose, when it would have
    /// landed and how many of its bits stayed in the buffer. A frame
    /// dropped never raises Receive Done, so this is how a caller knows of
    /// it. Kept in a clone; not in a checkpoint.
    pub fn dropped(&self) -> Option<Dropped> {
        self.dropped
    }

    /// `RRESET` at the instant last advanced to, for a clear written with
    /// Loop Back as `looped`: the receiver stops taking whatever frame it
    /// is taking.
    ///
    /// On the netlist board `RRESET`, the 74S00 at LMRCTL 0E03, comes on
    /// `-CLR RCV BUSY`, the 74S08 at 0E10 on Clear Receiver's
    /// `-11.SAYS.GO` and on `-INIT`; it takes `RACT` down through the
    /// 74S02 at LMRCLK 0B08,
    /// clears `RBCT` and resets `RCV BUSY`, the 74279 at LMRCTL 0D09;
    /// Receive Done is `RCV BUSY` and not `RACT`, and only `START^` sets
    /// `RACT`, so a frame taken from its `START^` to its landing is taken
    /// no further and never raises Receive Done. Before its destination
    /// word the busy receiver's gates find `-RACT` up: Lost Count steps
    /// for `DEST MATCH` --- this address, zero, or Spy --- and a frame to
    /// this address by name is aborted, `MATCH SO FAR` on `-LOST.ONE` at
    /// LMMYNM 0D02. After it, nothing more. The bits stored before the
    /// clear stay in the buffer. Measured on the netlist board by
    /// `a_clear_receiver_inside_a_frame_taken_drops_it_on_both_alike` and
    /// the tests after it in `tests/chaos_rtl.rs`.
    ///
    /// Off the cable the ether keeps the receive control's state for the
    /// frame, [`Ether::board_rreset`], and decides at its destination word;
    /// looped back the frame is in the interface's own queue of frames to
    /// land, and the interface decides, `Interface::busy_looped`.
    fn rreset(&mut self, looped: bool) {
        let (now, time) = (self.now, self.time);
        let full = self.receive_done;
        let off = time.ns(CBLBSY_OFF_NS);
        // Under Loop Back the cable is not the receiver's, and its frames
        // are left as they were.
        let stored = self.ether.as_mut().filter(|_| !looped).and_then(|e| e.board_rreset());
        self.cable_refused = self.ether.as_ref().is_some_and(|e| e.board_refused());
        self.down = None;
        if let Some(bits) = stored {
            let frame = self
                .turn
                .frames
                .iter()
                .rev()
                .find(|&&(s, _, e)| s <= now && now < e + off)
                .copied();
            let land = frame.map(|(_, _, e)| e + off);
            if land.is_none_or(|l| now < l) {
                self.drop_frame(now, frame.map(|f| f.1), land, &bits);
            } else {
                // Landed already, its run not yet delivered: a clear of
                // a frame in the buffer, which holds its bits.
                self.store(&bits);
            }
        }
        if !looped {
            return;
        }
        let me = self.address;
        let mut keep = VecDeque::new();
        for (land, r) in std::mem::take(&mut self.incoming) {
            let start = self
                .turn
                .frames
                .iter()
                .find(|&&(s, source, e)| source == me && time.after(s, e - s + off) == land)
                .map(|&(s, _, _)| s);
            let Some(start) = start.filter(|&s| now >= time.after(s, time.ns(RACT_NS))) else {
                // `START^` still to come: `RACT` takes `-RDONE` then.
                keep.push_back((land, r));
                continue;
            };
            // What the receiver stored, if `RACT` came up at `START^` ---
            // it did with the buffer free then, and nothing could fill it
            // since: every cell before the one now open, as off the cable
            // ([`wire::Decoder::stored`]), the trailing zero never.
            let cells = line(&wire_words(&r));
            let opened = ((now - start) / time.ns(wire::CELL_NS)) as usize;
            let bits = if full { &[][..] } else { &cells[..opened.min(r.bits)] };
            self.drop_frame(now, Some(me), Some(land), bits);
            let dest_at = time.after(start, time.ns(BUSY_ABORT_NS));
            if now < dest_at && r.bits >= 48 {
                self.looped_busy = r.framed.buffer.last().map(|&dest| (dest_at, dest));
            }
        }
        self.incoming = keep;
    }

    /// A frame the receiver was taking, dropped at `now`: `bits` of it
    /// stored, and `land` when it would have landed.
    fn drop_frame(&mut self, now: u64, source: Option<u16>, land: Option<u64>, bits: &[bool]) {
        if self.trace {
            eprintln!(
                "chaos {now:>6}: interface {:o} cleared its receiver inside a frame{}, {} bits in, \
                 and drops it",
                self.address,
                source.map_or(String::new(), |s| format!(" from {s:o}")),
                bits.len()
            );
        }
        self.store(bits);
        self.dropped = Some(Dropped { at: now, source, would_land: land, bits: bits.len() });
    }

    /// The bits `wire` --- a frame's first bits in the order they came
    /// --- into the incoming buffer from its first cell, as the receiver
    /// stores them, without Receive Done: the buffer's words as
    /// [`Interface::land`] takes them.
    fn store(&mut self, wire: &[bool]) {
        let n = wire.len().div_ceil(16);
        let mut words = vec![0u16; n];
        for (i, &b) in wire.iter().enumerate() {
            words[n - 1 - i / 16] |= (b as u16) << (i % 16);
        }
        self.land(&words, wire.len());
    }

    /// The destination word of a frame looped back goes by at `t` with
    /// the receiver off, a clear having taken it off after `START^`: what
    /// the busy receiver's gates make of it, as the ether's
    /// `busy_receiver` does for a frame off the cable. Lost Count steps for `DEST MATCH`;
    /// for this address by name `-LOST.ONE` presets the `ABORT`
    /// flip-flop at LMMODU 0A09, which is this transmitter's own, so the
    /// frame reads back as Transmit Abort and Transmit Done comes
    /// [`TDONE_AFTER_ABORT_NS`] on, the frame held for the abort signal
    /// as an aborted frame on the cable is ([`super::ether::ABORT_HOLD_NS`]).
    /// When the turn timer sees the line idle after such an abort is
    /// **unverified** under Loop Back; the next frame's turn after one,
    /// measured on the netlist board, would settle it.
    fn busy_looped(&mut self, t: u64, dest: u16) {
        let me = self.address;
        if dest != me && dest != 0 && self.csr & csr::SPY == 0 {
            return;
        }
        self.lost = (self.lost + 1) & 0o17;
        if dest == me {
            let time = self.time;
            self.transmit_abort = true;
            self.tdone_at = Some(time.after(t, time.ns(TDONE_AFTER_ABORT_NS)));
            let hold = t + time.ns(super::ether::ABORT_HOLD_NS);
            if let Some(f) =
                self.turn.frames.iter_mut().rev().find(|f| f.1 == me && f.0 <= t && f.2 > hold)
            {
                f.2 = hold;
            }
        }
        if self.trace {
            eprintln!(
                "chaos {t:>6}: interface {:o} {} its own frame looped back, its receiver cleared \
                 ({} lost)",
                me,
                if dest == me { "aborted" } else { "lost" },
                self.lost
            );
        }
    }

    /// A read of [`interface::START`] answered at `now`: the buffer, with
    /// this address as its source, is ready to go once the source and check
    /// words are shifted in behind it ([`tsr_empty`]; on revision 15
    /// [`TSR_READY_NS`] on), and goes at the turn timer's next rising edge
    /// of bit 7 with the cable idle ([`Turn`]).
    ///
    /// **Transmit Done falls at the read if it is up**, which it is when no
    /// word was written since the last frame or a Reset. On the netlist
    /// board the read raises `TRESET`, the 74S00s at LMTBFC 0C15, only
    /// while `-TDONE` is low (the 74S32 at 0D15), and `TRESET` clears
    /// `TDONE`, the 74S74 at LMMODU 0D08, as `-MSYN*` falls; it rises with
    /// the frame's end like any other, [`TDONE_BEFORE_END_NS`] before it
    /// (`an_empty_start_lowers_transmit_done_until_its_frame_ends` in
    /// `tests/chaos_netlist.rs`). On revision 15 the fall is at the read's
    /// own clock, `now`, with no delay to round.
    ///
    /// **What goes out is what the buffer holds below `TBCT`.** The source
    /// and the check word --- the check of every word written and the
    /// source --- go in at the count like the words, so for `n` words
    /// written the frame is `(16n + 32) mod 4096` bits: the words, the
    /// source and the check for up to 253, nothing for 254, and from 255
    /// the check word and what the wrap left below it
    /// (`a_frame_written_past_the_buffer_wraps_on_both_alike` in
    /// `tests/chaos_rtl.rs`).
    fn start(&mut self, now: u64) {
        self.transmit_done = false;
        let n = std::mem::take(&mut self.xmit_n);
        let check = check_on(std::mem::take(&mut self.xmit_check), &[self.address]);
        let mut line = std::mem::take(&mut self.xmit);
        for (k, w) in [(n, self.address), (n + 1, check)] {
            let at = k % BUFFER_WORDS;
            if at < line.len() {
                line[at] = w;
            } else {
                line.push(w);
            }
        }
        line.truncate((n + 2) % BUFFER_WORDS);
        if self.trace {
            eprintln!(
                "chaos {now:>6}: interface {:o} read START, {n} words in its transmit buffer",
                self.address,
            );
        }
        let ready = if self.time.period == 0 {
            tsr_empty(now, 2, self.turn.powered_at)
        } else {
            self.time.after(now, self.time.ns(TSR_READY_NS))
        };
        self.turn.ready = Some((ready, line));
    }
}

// --- Checkpoints ------------------------------------------------------------

impl Interface {
    /// The interface into a checkpoint: its switches and registers, both
    /// buffers, the frames due off the cable and the turn timer.  Not the
    /// cable, which a resume plugs in afresh with whatever the flags put
    /// on it, and not the trace switch.
    pub fn save(&self, w: &mut crate::checkpoint::Writer) {
        let Interface {
            address,
            csr,
            transmit_done,
            transmit_abort,
            receive_done,
            crc_error,
            lost,
            xmit,
            xmit_n,
            xmit_check,
            rcv,
            rcv_at,
            rcv_check: _,
            rcv_bits,
            tdone_at,
            incoming,
            turn,
            polled,
            now: _,
            next_event: _,
            ether: _,
            trace: _,
            time: _,
            dropped: _,
            cable_refused: _,
            looped_start: _,
            rd: _,
            down: _,
            looped_busy: _,
        } = self;
        w.u16(*address);
        w.u16(*csr);
        w.bool(*transmit_done);
        w.bool(*transmit_abort);
        w.bool(*receive_done);
        w.bool(*crc_error);
        w.u8(*lost);
        // The outgoing buffer as the words written, as it always was while
        // there are 256 or fewer; past that, its 256 words, then how many
        // were written past a multiple of 256 and the check of them all,
        // which nothing shorter could be. Every field keeps its type, so a
        // checkpoint written by a board, which writes an empty buffer,
        // still loads.
        if *xmit_n <= BUFFER_WORDS {
            w.u16s(xmit);
        } else {
            let mut v = xmit.clone();
            v.push((xmit_n % BUFFER_WORDS) as u16);
            v.push(*xmit_check);
            w.u16s(&v);
        }
        // The incoming buffer whole, its 256 words in the order the frame
        // in it is read; the check of the words read is made again on load.
        // A buffer never written is written empty, as it always was, which
        // keeps a checkpoint of a machine that never took a frame the bytes
        // it had (`a_cadr_checkpoint_keeps_version_49_s_bytes`).
        if rcv.iter().all(|&w| w == NEVER_WRITTEN) {
            w.u16s(&[]);
        } else {
            w.u16s(rcv);
        }
        w.u64(*rcv_at as u64);
        w.u64(*rcv_bits as u64);
        w.opt(*tdone_at, crate::checkpoint::Writer::u64);
        w.u64(incoming.len() as u64);
        for (at, r) in incoming {
            w.u64(*at);
            w.u16s(&r.framed.buffer);
            w.u16(r.framed.source);
            w.u16(r.framed.check);
            w.bool(r.framed.check_ok);
            w.u64(r.bits as u64);
        }
        turn.save(w, *address);
        w.u64(*polled);
    }

    /// Back from a checkpoint, into an interface plugged in at the same
    /// switches.
    pub fn load(&mut self, r: &mut crate::checkpoint::Reader) -> std::io::Result<()> {
        let address = r.u16()?;
        if address != self.address {
            return Err(crate::checkpoint::bad(format!(
                "the Chaosnet interface's switches read {address:o}, this machine's {:o}",
                self.address
            )));
        }
        self.csr = r.u16()?;
        self.transmit_done = r.bool()?;
        self.transmit_abort = r.bool()?;
        self.receive_done = r.bool()?;
        self.crc_error = r.bool()?;
        self.lost = r.u8()?;
        let xmit = r.u16s()?;
        if xmit.len() <= BUFFER_WORDS {
            self.xmit_n = xmit.len();
            self.xmit_check = check_word(&xmit);
            self.xmit = xmit;
        } else if xmit.len() == BUFFER_WORDS + 2 {
            self.xmit_n = 2 * BUFFER_WORDS + xmit[BUFFER_WORDS] as usize % BUFFER_WORDS;
            self.xmit_check = xmit[BUFFER_WORDS + 1];
            self.xmit = xmit[..BUFFER_WORDS].to_vec();
        } else {
            return Err(crate::checkpoint::bad(format!(
                "a Chaosnet transmit buffer of {} words",
                xmit.len()
            )));
        }
        let rcv = r.u16s()?;
        let rcv_at = r.u64()? as usize;
        let rcv_bits = r.u64()? as usize;
        if rcv.len() == BUFFER_WORDS {
            self.rcv = rcv;
            self.rcv_bits = rcv_bits;
        } else {
            // Written before the buffer was kept whole: the frame's words
            // alone, landed in a buffer never written.
            self.rcv = vec![NEVER_WRITTEN; BUFFER_WORDS];
            self.rcv_bits = 0;
            if !rcv.is_empty() {
                self.land(&rcv, rcv_bits);
            }
            self.rcv_bits = rcv_bits;
        }
        self.rcv_at = rcv_at;
        self.rcv_check = (0..rcv_at).fold(0, |c, k| check_on(c, &[self.rcv_word(k)]));
        self.tdone_at = r.opt(crate::checkpoint::Reader::u64)?;
        let n = r.u64()?;
        let mut incoming = VecDeque::new();
        for _ in 0..n {
            let at = r.u64()?;
            let buffer = r.u16s()?;
            let source = r.u16()?;
            let check = r.u16()?;
            let check_ok = r.bool()?;
            let bits = r.u64()? as usize;
            let framed = Framed { buffer, source, check, check_ok };
            incoming.push_back((at, Received { framed, bits }));
        }
        self.incoming = incoming;
        self.turn.load(r, self.address)?;
        self.polled = r.u64()?;
        self.next_event = None;
        self.dropped = None;
        self.cable_refused = false;
        self.looped_start = None;
        self.down = None;
        self.looped_busy = None;
        // The read register is the last word read, if a packet is being
        // read; otherwise nothing kept says what it held.
        self.rd =
            if self.receive_done && self.rcv_at > 0 { self.rcv_word(self.rcv_at - 1) } else { 0 };
        self.tell_the_cable();
        Ok(())
    }
}

impl Turn {
    /// A frame ready is written as the words written when it is a frame
    /// of them, the source and the check word --- as it always was --- and
    /// otherwise as its words and 512 more, which no buffer of words
    /// written ever was: what a buffer written past its end holds.
    fn save(&self, w: &mut crate::checkpoint::Writer, me: u16) {
        let Turn { powered_at, taken, q, low, frames, ready } = self;
        w.u64(*powered_at);
        w.u64(*taken);
        w.bool(*q);
        w.u8(*low);
        w.u64(frames.len() as u64);
        for (a, b, c) in frames {
            w.u64(*a);
            w.u16(*b);
            w.u64(*c);
        }
        w.bool(ready.is_some());
        if let Some((at, line)) = ready {
            w.u64(*at);
            let m = line.len();
            let written = m >= 2
                && m - 2 <= LONGEST_WRITTEN
                && line[m - 2] == me
                && line[m - 1] == check_word(&line[..m - 1]);
            if written {
                w.u16s(&line[..m - 2]);
            } else {
                let mut v = line.clone();
                v.resize(m + 2 * BUFFER_WORDS, 0);
                w.u16s(&v);
            }
        }
    }

    fn load(&mut self, r: &mut crate::checkpoint::Reader, me: u16) -> std::io::Result<()> {
        self.powered_at = r.u64()?;
        self.taken = r.u64()?;
        self.q = r.bool()?;
        self.low = r.u8()?;
        let n = r.u64()?;
        self.frames =
            (0..n).map(|_| Ok((r.u64()?, r.u16()?, r.u64()?))).collect::<std::io::Result<_>>()?;
        self.ready = if r.bool()? {
            let at = r.u64()?;
            let mut v = r.u16s()?;
            if v.len() >= 2 * BUFFER_WORDS {
                v.truncate(v.len() - 2 * BUFFER_WORDS);
            } else {
                v.push(me);
                v.push(check_word(&v));
            }
            Some((at, v))
        } else {
            None
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet(q: bool, low: u8) -> Turn {
        Turn { powered_at: 0, taken: 0, q, low, frames: Vec::new(), ready: None }
    }

    /// **A batched run of terminal counts is a stepped one.**
    /// [`Turn::idle_run`] takes at once what [`Turn::tc`] takes one at a
    /// time, and the two must leave `Q`, the counter's low byte and the
    /// count taken exactly where the other does.  Held over both phases
    /// of `Q`, the counter values either side of its wrap, and runs from
    /// one count to a thousand --- more than the byte's own period, so
    /// the wrap is crossed several times over. And on revision 15's grid,
    /// at 8.5, 19 and 63.5 ns, where a count is taken at the first clock
    /// at or after its edge: a run to the clock of its last count, and one
    /// to the instant before the next count's clock, take the same counts.
    #[test]
    fn a_batched_run_of_terminal_counts_is_a_stepped_one() {
        let times =
            [TimeBase::NS, TimeBase::half_ns(17), TimeBase::half_ns(38), TimeBase::half_ns(127)];
        for time in times {
            for q in [false, true] {
                for low in [0u8, 1, 2, 127, 128, 254, 255] {
                    for n in [1u64, 2, 3, 4, 5, 17, 254, 255, 256, 257, 511, 512, 1_000] {
                        let mut stepped = quiet(q, low);
                        for _ in 0..n {
                            let t = stepped.next_tc(time);
                            assert_eq!(stepped.tc(t, 0o1440, time), None, "nothing ready");
                        }
                        let fresh = quiet(q, low);
                        let last = time.grid(fresh.edge(n - 1, time));
                        let before_next = time.grid(fresh.edge(n, time)) - 1;
                        for until in [last, before_next] {
                            let mut batched = quiet(q, low);
                            assert!(batched.idle_run(until, time), "{n} counts due at {until}");
                            assert_eq!(
                                (batched.q, batched.low, batched.taken),
                                (stepped.q, stepped.low, stepped.taken),
                                "{time:?}: Q {q}, counter {low}, {n} counts, to {until}"
                            );
                            assert!(batched.next_tc(time) > until, "and the run is over");
                        }
                    }
                }
            }
        }
    }

    /// **CRC Error over a long read-out costs a read apiece.** The check
    /// of the words read is carried from read to read, not made again over
    /// all of them: 100,000 reads of a looped-back packet, round and round
    /// the buffer, take well under the bound, and CRC Error after them is
    /// the check of every word read.
    #[test]
    fn a_long_read_out_keeps_its_check_as_it_goes() {
        let mut i = Interface::new(0o3050, None, 0, false);
        let mut now = 12_000;
        i.write(interface::CSR, csr::RESET, now);
        i.write(interface::CSR, csr::LOOP_BACK | csr::CLEAR_RECEIVER, now);
        for w in [0o125252, 0o052525, 0o177400, 0o000377, 0o007417, 0o3050] {
            i.write(interface::WRITE_BUFFER, w, now);
        }
        i.read(interface::START, now);
        while i.csr() & csr::RECEIVE_DONE == 0 {
            now += 1_000;
            i.advance(now);
            assert!(now < 3_000_000, "the packet looped back");
        }
        let started = std::time::Instant::now();
        let read: Vec<u16> = (0..100_000).map(|_| i.read(interface::READ_BUFFER, now)).collect();
        let took = started.elapsed();
        assert!(took < std::time::Duration::from_secs(5), "100,000 reads took {took:?}");
        assert_eq!(
            i.read(interface::CSR, now) & csr::CRC_ERROR != 0,
            check_word(&read) != 0,
            "CRC Error after them is the check of the words read"
        );
    }

    /// The interface through a checkpoint and back, onto one at the same
    /// switches.
    fn through_a_checkpoint(i: &Interface) -> Interface {
        let mut w = crate::checkpoint::Writer::new();
        i.save(&mut w);
        let bytes = w.finish();
        let mut j = Interface::new(i.address, None, 0, false);
        j.load(&mut crate::checkpoint::Reader::new(&bytes)).unwrap();
        j
    }

    /// **A checkpoint keeps both buffers as they are, round and all.** A
    /// transmit buffer written past its end, the frame START left ready
    /// from one, and a received frame partway read out: each resumed goes
    /// on as it would have.
    #[test]
    fn a_checkpoint_keeps_both_buffers_round_and_all() {
        let mut i = Interface::new(0o3050, None, 0, false);
        let now = 12_000;
        i.write(interface::CSR, csr::RESET, now);
        for k in 0..300u16 {
            i.write(interface::WRITE_BUFFER, 0o100000 | k, now);
        }
        let mut j = through_a_checkpoint(&i);
        assert_eq!(
            (&j.xmit, j.xmit_n % 256, j.xmit_check),
            (&i.xmit, i.xmit_n % 256, i.xmit_check)
        );
        i.read(interface::START, now);
        j.read(interface::START, now);
        assert_eq!(j.turn.ready, i.turn.ready, "START from the resumed buffer");
        assert_eq!(i.turn.ready.as_ref().map(|r| r.1.len()), Some(46), "302 words, 46 past 256");
        let k = through_a_checkpoint(&i);
        assert_eq!(k.turn.ready, i.turn.ready, "the frame ready, written past its end");

        let mut i = Interface::new(0o3050, None, 0, false);
        let mut now = 12_000;
        i.write(interface::CSR, csr::RESET, now);
        i.write(interface::CSR, csr::LOOP_BACK | csr::CLEAR_RECEIVER, now);
        for w in [0o125252, 0o052525, 0o177400, 0o3050] {
            i.write(interface::WRITE_BUFFER, w, now);
        }
        i.read(interface::START, now);
        while i.csr() & csr::RECEIVE_DONE == 0 {
            now += 1_000;
            i.advance(now);
        }
        for _ in 0..3 {
            i.read(interface::READ_BUFFER, now);
        }
        let mut j = through_a_checkpoint(&i);
        for k in 0..300 {
            let a = (i.bit_count(), i.read(interface::READ_BUFFER, now), i.csr());
            let b = (j.bit_count(), j.read(interface::READ_BUFFER, now), j.csr());
            assert_eq!(b, a, "read {k} after the resume");
        }
    }

    /// **A count that could do more than arithmetic is not batched.**
    /// With a frame on the cable a count loads the counter from its
    /// source and holds `Q` set rather than toggling it, and with a packet
    /// ready a carry out of the low byte starts the transmitter: neither
    /// is what [`Turn::idle_run`] does, so it must refuse and leave the
    /// count to [`Turn::tc`].  A run refused changes nothing at all.
    #[test]
    fn a_run_is_refused_when_a_count_could_do_more_than_arithmetic() {
        let far = TURN_FIRST_TC_NS + TURN_TC_NS * 1_000;
        let heard = {
            let mut t = quiet(false, 0);
            t.frames.push((5_000, 0o3040, 15_000));
            t
        };
        let ready = {
            let mut t = quiet(false, 0);
            t.ready = Some((5_000, vec![1, 2, 0o3040]));
            t
        };
        for (what, start) in [("a frame on the cable", heard), ("a packet ready", ready)] {
            let mut turn = start.clone();
            assert!(!turn.idle_run(far, TimeBase::NS), "{what}: the run must be refused");
            assert_eq!(
                (turn.q, turn.low, turn.taken),
                (start.q, start.low, start.taken),
                "{what}: and change nothing"
            );
        }
        // And a count that is not due yet is not taken early.
        for time in [TimeBase::NS, TimeBase::half_ns(17)] {
            let mut turn = quiet(false, 0);
            assert!(!turn.idle_run(turn.next_tc(time) - 1, time));
            assert_eq!(turn.taken, 0);
        }
    }
}
