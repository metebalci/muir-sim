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

use super::ether::{Ether, turn_byte};
use super::interface::{self, csr};
use super::packet::{Framed, Received, check_word, frame};
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

/// From the read of START answered, `-SSYN`, to `TSREMPTY`: the source
/// and check words shifted into the buffer behind the packet.  Measured on
/// the netlist board, 4,150 at every phase of its clocks, where the answer
/// itself comes 350 to 2,100 ns after `-MSYN`.  `rtl` hands the read to
/// this model at the answer.
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
    /// A packet started: `TSREMPTY` at, and its words.
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
    /// The outgoing buffer, as written, destination last.
    xmit: Vec<u16>,
    /// The incoming buffer --- the words as written, destination, source,
    /// check --- and how far it has been read.
    rcv: Vec<u16>,
    rcv_at: usize,
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
            rcv: self.rcv.clone(),
            rcv_at: self.rcv_at,
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
            rcv: Vec::new(),
            rcv_at: 0,
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
            | if self.crc_error || self.turn.busy_at(self.now, self.time) {
                csr::CRC_ERROR
            } else {
                0
            }
            | if self.receive_done { csr::RECEIVE_DONE } else { 0 }
    }

    /// "The number of bits in the incoming packet buffer, minus one.
    /// After the whole packet has been read out, it will contain 7777."
    /// Before any packet, and after a Reset or a Clear Receiver, the
    /// netlist board reads 0 (`tests/cadrio_netlist.rs`). The count comes
    /// down by what each read takes: sixteen bits a word, and only the
    /// bits there are for the partial word wreckage puts at the top ---
    /// the netlist board reads 15 with one whole word left of 222 bits
    /// (`tests/chaos_rtl.rs`).
    pub fn bit_count(&self) -> u16 {
        if !self.receive_done {
            return 0;
        }
        let top = self.rcv_bits - (self.rcv.len().saturating_sub(1)) * 16;
        let read = match self.rcv_at {
            0 => 0,
            k => top + (k - 1) * 16,
        };
        match self.rcv_bits.saturating_sub(read) {
            0 => 0o7777,
            left => (left as u16 - 1) & 0o7777,
        }
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
            let due = [Some(tc), cable, landing, self.tdone_at]
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
            let others = [cable, landing, self.tdone_at].into_iter().flatten().min();
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
        self.next_event = [Some(self.turn.next_tc(time)), cable, poll, self.tdone_at]
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
    /// Back, or into the void with no cable.
    fn launch(&mut self, start: u64, t: u64) {
        let Some((ready_at, buffer)) = self.turn.ready.take() else { return };
        let time = self.time;
        let bits = frame(&buffer, self.address).len() as u64;
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
                let mut over = buffer.clone();
                over.push(self.address);
                let check = check_word(&over);
                let bits = (buffer.len() + 2) * 16;
                let framed = Framed { buffer, source: self.address, check, check_ok: true };
                let r = Received { framed, bits };
                self.incoming.push_back((time.after(start, length + time.ns(CBLBSY_OFF_NS)), r));
                self.incoming.make_contiguous().sort_by_key(|&(t, _)| t);
            }
        } else if let Some(e) = self.ether.as_mut() {
            e.board_send(buffer, start);
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
    fn arrive(&mut self, now: u64, r: &Received) {
        let Some(&dest) = r.framed.buffer.last() else { return };
        let mine = dest == self.address || dest == 0 || self.csr & csr::SPY != 0;
        if !mine {
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
        self.rcv = f.buffer.clone();
        self.rcv.push(f.source);
        self.rcv.push(f.check);
        self.rcv_at = 0;
        self.rcv_bits = bits;
        self.crc_error = !f.check_ok;
        self.receive_done = true;
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
                        self.rcv.len()
                    );
                }
                self.csr()
            }
            interface::MY_ADDRESS => self.address,
            interface::READ_BUFFER => {
                let w = self.rcv.get(self.rcv_at).copied().unwrap_or(0);
                if self.rcv_at < self.rcv.len() {
                    self.rcv_at += 1;
                }
                w
            }
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
                self.csr = v & WRITABLE;
                if v & csr::RESET != 0 {
                    self.reset();
                }
                if v & csr::CLEAR_RECEIVER != 0 {
                    if self.trace {
                        eprintln!(
                            "chaos {now:>6}: interface {:o} cleared its receiver, {} of {} words read{}",
                            self.address,
                            self.rcv_at,
                            self.rcv.len(),
                            if self.receive_done { "" } else { ", buffer already free" }
                        );
                    }
                    self.receive_done = false;
                    self.crc_error = false;
                    self.rcv.clear();
                    self.rcv_at = 0;
                    self.lost = 0;
                }
                if v & csr::CLEAR_TRANSMITTER != 0 {
                    self.xmit.clear();
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
                // The outgoing buffer is the 2147 at LMTBUF 0C10, 4K by 1,
                // addressed by `TBCT<11:0>` from the three 25LS193s at
                // 0C11-0C13: 4,096 bits, 256 sixteen-bit words, and a word
                // past that has nowhere to go.  The incoming buffer is its
                // twin, the 2147 at LMRBUF 0C04 on `RBCT<11:0>`.
                if self.xmit.len() < 256 {
                    self.xmit.push(v);
                }
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
    pub fn reset(&mut self) {
        self.invalidate();
        self.csr = 0;
        self.transmit_done = true;
        self.transmit_abort = false;
        self.receive_done = false;
        self.crc_error = false;
        self.lost = 0;
        self.xmit.clear();
        self.rcv.clear();
        self.rcv_at = 0;
        self.tdone_at = None;
        self.incoming.clear();
        self.turn.ready = None;
        self.tell_the_cable();
    }

    /// A read of [`interface::START`]: the buffer, with this address as
    /// its source, is ready to go [`TSR_READY_NS`] on, and goes at the
    /// turn timer's next rising edge of bit 7 with the cable idle
    /// ([`Turn`]).
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
    fn start(&mut self, now: u64) {
        self.transmit_done = false;
        let buffer = std::mem::take(&mut self.xmit);
        if self.trace {
            eprintln!(
                "chaos {now:>6}: interface {:o} read START, {} words in its transmit buffer",
                self.address,
                buffer.len()
            );
        }
        self.turn.ready = Some((self.time.after(now, self.time.ns(TSR_READY_NS)), buffer));
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
            rcv,
            rcv_at,
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
        } = self;
        w.u16(*address);
        w.u16(*csr);
        w.bool(*transmit_done);
        w.bool(*transmit_abort);
        w.bool(*receive_done);
        w.bool(*crc_error);
        w.u8(*lost);
        w.u16s(xmit);
        w.u16s(rcv);
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
        turn.save(w);
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
        self.xmit = r.u16s()?;
        self.rcv = r.u16s()?;
        self.rcv_at = r.u64()? as usize;
        self.rcv_bits = r.u64()? as usize;
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
        self.turn.load(r)?;
        self.polled = r.u64()?;
        self.next_event = None;
        self.tell_the_cable();
        Ok(())
    }
}

impl Turn {
    fn save(&self, w: &mut crate::checkpoint::Writer) {
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
        if let Some((at, words)) = ready {
            w.u64(*at);
            w.u16s(words);
        }
    }

    fn load(&mut self, r: &mut crate::checkpoint::Reader) -> std::io::Result<()> {
        self.powered_at = r.u64()?;
        self.taken = r.u64()?;
        self.q = r.bool()?;
        self.low = r.u8()?;
        let n = r.u64()?;
        self.frames =
            (0..n).map(|_| Ok((r.u64()?, r.u16()?, r.u64()?))).collect::<std::io::Result<_>>()?;
        self.ready = if r.bool()? { Some((r.u64()?, r.u16s()?)) } else { None };
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
    /// at 8.5 and 19 ns, where a count is taken at the first clock at or
    /// after its edge: a run to the clock of its last count, and one to
    /// the instant before the next count's clock, take the same counts.
    #[test]
    fn a_batched_run_of_terminal_counts_is_a_stepped_one() {
        for time in [TimeBase::NS, TimeBase::half_ns(17), TimeBase::half_ns(38)] {
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
