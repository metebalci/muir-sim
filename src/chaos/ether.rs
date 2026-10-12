// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The ether: the cable as a medium, and the nodes on its model side.
//!
//! AIM-628 §2.3: the transceiver "impresses [the interface's signal] onto
//! the cable as a level of about 8 volts for a 1, or 0 volts (open
//! circuit) for a 0 ... When the cable is idle it is held at 0 volts by
//! the terminations." So the cable's level is the OR of everything
//! driving it, every transceiver hears the whole cable including its own
//! transmission, and one "detects interference (another transceiver
//! transmitting at the same time as this one) and informs the
//! interface."
//!
//! One side of this ether is the netlist board, through
//! [`super::cable::OnCable`]; the other is any number of [`Node`]s ---
//! the CHUDP link that carries the hosts off this machine, and whatever a
//! caller in this process puts there ---
//! which see every packet and offer packets to send. The ether frames,
//! codes and decodes, and keeps the etiquette of §2.6: a node takes its
//! turn after a packet, and goes if the cable is idle then. A transmitter
//! that has committed --- the board at its turn timer's instant, a
//! transmitter that has not heard the cable --- goes whether or not the
//! cable is busy, and two driving high at once is §2.3's interference: a
//! transmitter whose clock edge finds it aborts and holds the cable high
//! for [`ABORT_HOLD_NS`], the abort signal, and what was left on the
//! cable reaches every receiver as wreckage, failing its check.
//!
//! Two things the stations on this side of the cable do because the
//! hardware does them, and both are timed here rather than in the nodes:
//!
//! - **The behavioral board's busy receiver aborts**, AIM-628 §2.5: a
//!   frame addressed to it while its buffer is full is counted and the
//!   sender stopped, [`BUSY_ABORT_NS`] into the frame. The netlist board
//!   drives its own cable and does this in its gates; a behavioral board
//!   has the ether do it for it, from the receiver's state it is given
//!   through [`Ether::board_receiver`].  The frame it decides on is a
//!   model transmitter's or the netlist board's alike: the board makes
//!   its own bits and has no `Transmission` here, so its frame is timed
//!   from its transceiver's first edge and its addresses are read off
//!   the cells that have gone by.
//! - **A station cannot offer its next frame at once.** Its host refills
//!   the outgoing buffer a word at a time and reads `START`, which is
//!   [`refill`], and its turn then comes on the turn timer's round,
//!   [`ROUND_NS`]. [`Node::station`] says which nodes stand for
//!   stations; [`Capture`] is an instrument and does not.

use super::packet::{Framed, Received, frame, header_on_the_wire, unframe_any};
use super::wire::{self, Decoder};
use std::collections::{BTreeMap, VecDeque};

/// Something on the model side of the cable.
pub trait Node: Send {
    /// The node's network address.
    fn address(&self) -> u16;
    /// A packet taken off the cable at `now`, whoever it was for. Nodes
    /// filter by [`Framed::buffer`]'s destination as the hardware does.
    fn receive(&mut self, now: u64, packet: &Framed);
    /// A packet to put on the cable, asked when the cable is free and it
    /// is the node's turn: the words as the software would write them,
    /// cable destination last. The ether adds the source and the check
    /// word.
    fn transmit(&mut self, now: u64) -> Option<Vec<u16>>;
    /// The node's frame stopped at `now`, aborted, its transceiver having
    /// heard another: the words as given to [`Node::transmit`], back, to
    /// offer again at the next turn or not. An interface's driver retries
    /// on Transmit Abort; the default forgets the frame, which a host
    /// whose transport retransmits can afford.
    fn aborted(&mut self, _now: u64, _buffer: Vec<u16>) {}
    /// Whether this node stands for a station of its own on the cable,
    /// with an interface and a host behind it. A station's host must
    /// refill the outgoing buffer a word at a time before the next frame
    /// can go ([`refill`]), and its turn then comes on the turn timer's
    /// round ([`ROUND_NS`]), so two frames of its own are hundreds of
    /// microseconds apart however fast they were handed in. Nodes are
    /// stations unless they say otherwise; [`Capture`] is the one that
    /// does.
    fn station(&self) -> bool {
        true
    }
}

/// The time-slot of the turn-taking, AIM-628 §2.6: one count of the
/// board's `MY.TURN CLK^`, measured at 1,000 ns on the netlist board
/// (`MY.TURN^`, bit 7 of the counter it clocks, toggles every 128,000 ns).
pub const SLOT_NS: u64 = 1_000;

/// How often a transmitter looks for interference: one period of the
/// board's 8 MHz `FCLK^`. On the board `COLLISION` is `TBUSY` with
/// `INTERFERENCE`, the open-collector 74S02 at LMMODU 0B08, and the
/// `ABORT` flip-flop, the 74S112 at 0A09, takes it on `-FCLK^` --- so
/// interference that is not there at an edge is missed, and the
/// transmitter goes on, and collides on, until an edge finds it; `D OUT`
/// is `TTL.D.OUT AND -ABORT` at the same 0B08, so the driver is off from
/// the edge that does. A model transmitter samples its transceiver the
/// same way, at this period from its frame's first edge, and stops at
/// the first sample that finds interference. `tests/chaos_netlist.rs`
/// watches the board's `ABORT` follow its transceiver's interference:
/// 75 ns after it, the next edge.
pub const ABORT_NS: u64 = 125;

/// How long `ABORT` stands, from the edge that sets it to `ABORTDN`, and
/// the driver holds the cable high throughout: AIM-628 §2.5's abort
/// signal, "if the ether remains high for about two bit cells". Measured
/// on the netlist board, `tests/chaos_netlist.rs`: the driver on from
/// `ABORT` to 1,000 ns after it, four cells. A model transmitter aborted
/// holds the cable as long.
pub const ABORT_HOLD_NS: u64 = 1_000;

/// From a frame's first edge to the busy receiver's abort signal.  The
/// three hardware words go first, "in the order check, source,
/// destination" (AIM-628 §2.5), which is 48 cells of [`wire::CELL_NS`],
/// and the driver comes on in the cell after them.  Measured on the
/// netlist board, `the_busy_receiver_aborts_a_frame_addressed_to_it` in
/// `tests/chaos_netlist.rs`: `DEST MATCH` at 12,060 ns, the line driver
/// on at 12,260 --- the cell and the gate delays through `-LOST.ONE`,
/// the `ABORT` flip-flop and the 26LS31 --- and Lost Count at 12,310.
/// One instant serves for the driver and the count here: a Unibus cycle
/// is microseconds, so no register read can fall in the 50 ns between
/// them.
pub const BUSY_ABORT_NS: u64 = 12_260;

/// From a frame's first edge to `START^`, where the receiver decides
/// whether it is active for the frame.  `RACT` is the second half of the
/// 74S74 at LMRCLK 0C06: `-RDONE` on its D input (pin 12), `START^` its
/// clock (pin 11), its preset tied high, and its clear the 74S02 at
/// LMRCLK 0B08 NORing `RRESET` with `-CBLBSY`.  So it takes Receive Done
/// as it stands at `START^` and holds it while the cable stays busy; an
/// `RRESET` can only clear it.  A frame that finds the buffer full at
/// `START^` is not received even if the software clears the receiver
/// before its destination word, and it is still counted, and aborted if
/// it was addressed by name, because `-LOST.ONE` takes `-RACT` and not
/// `-RDONE`.  Measured on the netlist board: `START^` 60 ns after the
/// first edge (`diagnose_the_busy_receiver` in `tests/chaos_netlist.rs`),
/// held by `a_clear_receiver_inside_a_frame_does_not_let_it_in_on_both_alike`
/// in `tests/chaos_rtl.rs`.
pub const RACT_NS: u64 = 60;

/// One whole round of the turn timer: `MY.TURN^` is bit 7 of the
/// 74LS193s at LMTURN 0A17 and 0A18, so once a station's turn has gone
/// by, the next comes 256 counts on while the cable stays idle.
/// `two_packets_back_to_back_wait_a_whole_round` in
/// `tests/chaos_netlist.rs` measures 257 slots between two packets a
/// board sends back to back: the one count its software missed and the
/// 256 of a round.
pub const ROUND_NS: u64 = 256 * SLOT_NS;

/// What one sixteen-bit word costs a station's host to write into the
/// outgoing buffer: microcode 323's `CHAOS-XMT-2` loop in
/// `uc-chaos.lisp`, which reads a pair of halfwords out of memory and
/// writes each into the hardware, held by the board's transmit shift
/// register while the last word shifts in
/// ([`crate::busint::IoBoardTiming::answer_cycle`]). On `rtl` the loop
/// writes a pair's two halves 1,305 ns apart and the pairs 2,755 ns apart,
/// and the register holds every second half to 2,451-2,576 ns after the
/// first: 2,634 ns a word on the mean. Measured on `rtl` over System 100's
/// boot, its login and a file probe, packets of W = 11 to 34 words, the
/// first word written to the read of `START` fits `2,631 ns × W − 278`;
/// the constant is not charged. `micro` answers every cycle at once, holds
/// nothing, and writes faster than the board takes words.
///
/// **The CADR's own driver is the station modeled here.** The machine at
/// the other end of a CHUDP link is not modeled at all --- muir has no
/// model of an ITS or a `cbridge` host --- so what stands in for its
/// interface is the one interface whose software this project can read:
/// the CADR's. That fixes the order of magnitude, which is what the
/// spacing of a burst turns on: hundreds of microseconds between frames
/// rather than the one slot a queue would take.
pub const REFILL_WORD_NS: u64 = 2_630;

/// What a station takes from its last frame's end to having the next one
/// ready to go: `words` writes down the Unibus at [`REFILL_WORD_NS`]
/// each and the read of `START`, then [`super::board::TSR_READY_NS`] for
/// the source and check words to be shifted in behind the packet, as they
/// are behind a CADR driver's START, which waits for the last word ---
/// less [`super::board::TDONE_BEFORE_END_NS`], because Transmit Done,
/// which is what lets the host start writing, comes that much before the
/// frame's nominal end.
///
/// `words` is the buffer as a node offers it, whose last word is the
/// cable destination: exactly the words the software writes, AIM-628 §7.
pub fn refill(words: usize) -> u64 {
    words as u64 * REFILL_WORD_NS + super::board::TSR_READY_NS - super::board::TDONE_BEFORE_END_NS
}

/// What the I/O board's turn counter is loaded with when it hears a
/// packet from `source`: page LMMYNM computes `source - me` bit-serially
/// as the source word goes by, low bit first, in the 74S287 at 0D01 ---
/// `MATCH SO FAR` the difference bit, `RS2` the borrow --- and the
/// 74LS164s at LMTURN 0B18 and 0B19 shift the difference bits in as they are
/// made, so that at `SRC STB`, when the word is over, they hold bits 14
/// down to 3 of the difference, the newest first; the low byte the
/// counter takes is bits 14 to 7, bit 14 at the bottom.  The turn comes
/// that many counts and one after the cable goes idle.  Measured on the
/// netlist board for four addresses against a host at 3060: `16`, `64`,
/// `127`, `255` loaded and 17, 65, 128, 256 counts waited
/// against the netlist board.
///
/// **A station's own packet loads zero, so its own turn is the first
/// count of the round and not the last.**  AIM-628 §2.6, memo page 7, has
/// it the other way: "When an interface transmits, the token stops moving
/// and remains at that interface until the end of the packet, whereupon
/// it continues down the cable, passing every other interface, giving
/// them each a chance to transmit before letting the first interface
/// transmit a second packet."  The board as wired hears its own
/// transmission as it hears any frame --- it must, for its transceiver to
/// find a collision --- and loads the difference between that source word
/// and its own address, which is zero, so `MY.TURN^` rises on the first
/// count after the cable goes idle: measured at four addresses by
/// `the_turn_timer_loads_the_address_difference_bit_reversed` in
/// `tests/chaos_rtl.rs`.  The board is followed.  What keeps the memo's
/// picture on a real machine is that the software cannot reload the
/// transmitter within one slot, which
/// `two_packets_back_to_back_wait_a_whole_round` in
/// `tests/chaos_netlist.rs` measures at 257 slots.  A node that stands
/// for a station is held to the same: [`Ether::turn`] charges it
/// [`refill`] after a frame of its own and gives it the first turn of
/// the round at or after that, which for two short frames is the same
/// 257 slots.
pub fn turn_byte(source: u16, me: u16) -> u8 {
    let d = source.wrapping_sub(me);
    (0..8).map(|m| (((d >> (14 - m)) & 1) as u8) << m).sum()
}

/// One event on the ether, for the record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A packet went by, taken off the cable at this time.
    Heard(u64, Framed),
    /// A node put a packet on the cable, starting at this time.
    Sent(u64, u16, Vec<u16>),
    /// Two transceivers drove high at once at this time, for the first
    /// time since the cable was idle: interference, AIM-628 §2.3, which
    /// every transmitter high at one of its own clock edges stops on.
    Collision(u64),
}

/// A behavioral interface on the board side of the cable ---
/// [`crate::chaos::board::Interface`], `rtl`'s I/O board --- which sends
/// and receives whole frames.  Its turn and its time on the cable are the
/// ether's, as a node's are; what it hears is kept for it to take.
struct Board {
    address: u16,
    /// A frame given to send, and the instant its turn timer starts it.
    pending: Option<(u64, Vec<u16>)>,
    /// When the frame being sent ends, once it has started.
    sending_until: Option<u64>,
    /// The receiver as the interface last left it, [`Ether::board_receiver`]:
    /// whether its buffer is full (Receive Done) and whether it is
    /// spying.  Receive Done is wanted at a frame's `START^`
    /// ([`RACT_NS`]) and Spy as its destination word goes by, both long
    /// before the interface hears of the frame at all.
    receive_done: bool,
    spy: bool,
    /// Whether the board's line driver is holding the cable high with the
    /// busy receiver's abort signal, and until when.
    aborting: bool,
    abort_until: Option<u64>,
    /// Frames the board counted in Lost Count, oldest first: when, whose,
    /// and whether it aborted the sender.  [`Ether::board_lost`].
    lost: VecDeque<(u64, u16, bool)>,
    /// Whether the run of bits now on the cable found the board's
    /// receiver inactive.  `RACT`, the 74S74 at LMRCLK 0C06, takes
    /// `-RDONE` on `START^`, so with Receive Done up the receiver never
    /// goes active for the frame and nothing of it reaches the buffer:
    /// the frame is counted and that is all.  Set at the `START^` of a
    /// frame that finds the buffer full, cleared when the run is
    /// delivered.
    not_taken: bool,
    /// What the board heard, as a receiver takes it, and whether its
    /// receiver was active for it.
    heard: VecDeque<(u64, Received, bool)>,
    /// Every frame that started on the cable, the board's own included:
    /// its first edge, its source and its nominal end, for the board's
    /// turn timer, which watches the cable as the receiver does.
    frames: VecDeque<(u64, u16, u64)>,
    /// Transmitters on the cable that aborted, oldest first: when, whose
    /// frame, and when its abort signal ends and the cable is let go.
    collisions: VecDeque<(u64, u16, u64)>,
}

/// A frame a model transmitter has on the cable.
struct Transmission {
    source: u16,
    /// The frame's destination word, the buffer's last, and the instant
    /// it has gone by on the cable --- where a busy receiver decides,
    /// [`BUSY_ABORT_NS`] --- until that instant is past.  `None` with no
    /// behavioral board on the cable: the netlist board decides in its
    /// own gates.
    dest: u16,
    dest_at: Option<u64>,
    /// The instant of the frame's `START^` on the behavioral board,
    /// [`RACT_NS`] after its first edge, until it is past; and whether the
    /// receiver found its buffer full then, so that `RACT` is off for the
    /// rest of the frame.  `None` with no behavioral board, as `dest_at`.
    ract_at: Option<u64>,
    ract_off: bool,
    /// The words as the node gave them, to hand back if it is aborted.
    buffer: Vec<u16>,
    /// The level changes still to come, in time.
    waveform: VecDeque<(u64, bool)>,
    /// What the transmitter drives now.
    level: bool,
    /// The transmitter's next clock edge, [`ABORT_NS`] apart from the
    /// frame's first: where it looks for interference.
    tick: u64,
    /// A transmitter with no interference detector: it sends the frame
    /// whole whatever is on the cable. [`Ether::send_now`]'s.
    deaf: bool,
}

/// A frame the netlist board's transceiver is putting on the cable, as
/// the behavioral board's receiver has to take it.  A model
/// transmitter's frame is a [`Transmission`] and carries the two
/// instants the receiver decides at; the netlist board's is only a level
/// its transceiver drives, so they are timed from that level's first
/// edge and the addresses come off what the decoder has taken.
struct FromCable {
    /// The frame's `START^`, [`RACT_NS`] in, until it is past.
    ract_at: Option<u64>,
    /// The instant its destination word has gone by, [`BUSY_ABORT_NS`]
    /// in, until it is past.
    dest_at: Option<u64>,
    /// Whether the receiver found its buffer full at `START^`.
    ract_off: bool,
}

/// A frame waiting to start.
struct Waiting {
    /// When it may start.
    at: u64,
    source: u16,
    buffer: Vec<u16>,
    /// Whether `at` is the transmitter's own instant --- the board's turn
    /// timer's, or a transmitter's that has not heard the cable --- which
    /// goes then, busy or not, rather than a turn, which goes then if the
    /// cable is idle.
    committed: bool,
    deaf: bool,
    /// Whether the frame is a station's, [`Node::station`], and waits for
    /// its host to refill.
    station: bool,
    /// Which node offered it, if a node did.  A node is not asked for
    /// another frame while one of its own waits --- it has one
    /// transmitter --- but a station waiting for its turn holds up
    /// nobody else's.
    node: Option<usize>,
    /// The cable's last edge, and the source of the last frame off it,
    /// when `at` was reckoned.  A station's turn counter is loaded by
    /// every frame it hears and counts only while the cable is idle, so a
    /// frame that went by while this one waited moves its turn:
    /// [`Ether::at`] reckons it again when, and only when, either of
    /// these is no longer what the cable says.  Both are wanted --- the
    /// edge for when the counting starts, the source for what was loaded
    /// --- and the source is not known until the run of bits ends, which
    /// is after the edge.
    reckoned: (Option<u64>, Option<u16>),
}

pub struct Ether {
    nodes: Vec<Box<dyn Node>>,
    board: Option<Board>,
    /// What the board's transceiver drives, as last read.
    board_tx: bool,
    /// A frame the netlist board's transceiver is putting on the cable,
    /// while the behavioral board's receiver still has to decide on it.
    from_cable: Option<FromCable>,
    /// The model transmitters' frames on the cable, any number at once.
    sending: Vec<Transmission>,
    waiting: Vec<Waiting>,
    level: bool,
    decoder: Decoder,
    last_edge: Option<u64>,
    last_source: Option<u16>,
    /// Whether two transceivers have driven high at once since the cable
    /// was last idle: one [`Event::Collision`] an overlap.
    collided: bool,
    /// When each station on the cable last had the cable to itself: its
    /// last frame's end, or the end of its abort signal if it was
    /// stopped.  Its host begins writing the next frame from there, which
    /// is [`refill`] of that frame's own words: a station of this process
    /// is no faster than an interface, [`Node::station`].
    sent_until: BTreeMap<u16, u64>,
    /// Whether [`Ether::log`] is kept.
    logging: bool,
    /// The last [`LOG_CAP`] events on the cable, for a test to read back.
    pub log: Vec<Event>,
    /// Units a nanosecond, which every instant here is counted in: 1, or
    /// 2 on revision 15, where the cable's times double into units of
    /// 0.5 ns, unchanged (MP2b ruling Q19). [`Ether::set_per_ns`].
    per_ns: u64,
}

/// How many events [`Ether::log`] keeps; the oldest half goes when it is
/// full.  Enough for the acceptance test's load of CC over the network to
/// stay whole, and small enough that a day's traffic is not.
pub const LOG_CAP: usize = 1 << 16;

impl Ether {
    fn record(&mut self, event: Event) {
        // A normal run keeps nothing: the log holds whole packet buffers,
        // tens of megabytes over a day's traffic, and only a test reads
        // it back. It is kept when [`Ether::keep_log`] asks.
        if !self.logging {
            return;
        }
        crate::keep_recent(&mut self.log, LOG_CAP);
        self.log.push(event);
    }

    pub fn new() -> Ether {
        Ether {
            nodes: Vec::new(),
            board: None,
            board_tx: false,
            from_cable: None,
            sending: Vec::new(),
            waiting: Vec::new(),
            level: false,
            decoder: Decoder::new(),
            last_edge: None,
            last_source: None,
            collided: false,
            sent_until: BTreeMap::new(),
            // Kept by default so a board or the machine's own ether can be
            // read back by a test as it always could; a long-running,
            // run that wants it turns it on with [`Ether::keep_log`].
            logging: false,
            log: Vec::new(),
            per_ns: 1,
        }
    }

    /// Counts every instant in units of `1/per_ns` ns from here on: the
    /// machine's own, which the interface on this cable keeps
    /// ([`crate::chaos::board::Interface::set_time_base`]).
    pub fn set_per_ns(&mut self, per_ns: u64) {
        self.per_ns = per_ns;
        self.decoder.set_per_ns(per_ns);
    }

    /// `ns` nanoseconds in this cable's units.
    fn ns(&self, ns: u64) -> u64 {
        ns * self.per_ns
    }

    /// Whether to keep [`Ether::log`]. Off for a normal run, which reads it
    /// back never: the log holds whole packet buffers, tens of megabytes
    /// over a day's traffic.
    pub fn keep_log(&mut self, keep: bool) {
        self.logging = keep;
    }

    pub fn attach(&mut self, node: Box<dyn Node>) {
        self.nodes.push(node);
    }

    /// Puts a behavioral interface at `address` on the board side of the
    /// cable.  One at most; the netlist board drives the cable itself.
    pub fn attach_board(&mut self, address: u16) {
        self.board = Some(Board {
            address,
            pending: None,
            sending_until: None,
            receive_done: false,
            spy: false,
            aborting: false,
            abort_until: None,
            lost: VecDeque::new(),
            not_taken: false,
            heard: VecDeque::new(),
            frames: VecDeque::new(),
            collisions: VecDeque::new(),
        });
    }

    /// The behavioral board's frame to send, cable destination last, and
    /// the instant its turn timer starts it: it goes then, busy cable or
    /// not, and [`Ether::board_sending_until`] says when it ends.
    pub fn board_send(&mut self, buffer: Vec<u16>, start: u64) {
        if let Some(b) = self.board.as_mut() {
            b.pending = Some((start, buffer));
            b.sending_until = None;
        }
    }

    /// A frame put on the cable at `now`, busy or not, by a transmitter
    /// that has not heard it and has no interference detector of its own:
    /// the frame goes whole, whatever else is on the cable. It is what a
    /// test uses to try this board's detector, and what a transmitter with
    /// a length of cable between it and the rest --- a bridge to a wider
    /// Chaosnet --- looks like from here.
    pub fn send_now(&mut self, now: u64, source: u16, buffer: Vec<u16>) {
        self.waiting.push(Waiting {
            at: now,
            source,
            buffer,
            committed: true,
            deaf: true,
            station: false,
            node: None,
            reckoned: (self.last_edge, self.last_source),
        });
        self.at(now);
    }

    /// A frame that started on the cable, oldest first: its first edge, its
    /// source and its nominal end.
    pub fn board_cable_frame(&mut self) -> Option<(u64, u16, u64)> {
        self.board.as_mut().and_then(|b| b.frames.pop_front())
    }

    /// A transmitter on the cable that aborted, oldest first, for the
    /// behavioral board: the instant its `ABORT` set, whose frame, and
    /// when it lets the cable go, [`ABORT_HOLD_NS`] on --- the frame's
    /// end from then, in place of its nominal one. The board's own frame
    /// among them is its Transmit Abort.
    pub fn board_collision(&mut self) -> Option<(u64, u16, u64)> {
        self.board.as_mut().and_then(|b| b.collisions.pop_front())
    }

    /// When the behavioral board's frame on the cable ends, if one has
    /// started and not ended.
    pub fn board_sending_until(&self) -> Option<u64> {
        self.board.as_ref().and_then(|b| b.sending_until)
    }

    /// A frame the behavioral board heard, oldest first, as a receiver
    /// takes it: everything on the cable, wreckage included, for the
    /// interface to filter as the receiver does --- **its own frame
    /// among them**. The transceiver hears the whole cable, its own
    /// transmission included (AIM-628 §2.3), and the receiver judges that
    /// frame by `DEST MATCH` as any other: the netlist board takes its own
    /// frame to its own address, to zero or under Spy into a free buffer
    /// (`the_board_takes_its_own_frame_off_the_cable_on_both_alike` in
    /// `tests/chaos_rtl.rs`).  The flag is
    /// whether the receiver was active for it: false, and the board's
    /// buffer was full when the frame's destination word went by, so
    /// `RACT` never came up, nothing of the frame reached the buffer, and
    /// what there was to count is in [`Ether::board_lost`] already.
    pub fn board_heard(&mut self) -> Option<(u64, Received, bool)> {
        self.board.as_mut().and_then(|b| b.heard.pop_front())
    }

    /// The behavioral board's receiver as the interface has it now:
    /// whether its buffer is full and whether it is spying.  The ether
    /// decides with this at the instant a frame's destination word goes
    /// by, AIM-628 §2.5, which is microseconds before the frame lands, so
    /// the interface hands it over whenever either changes.
    pub fn board_receiver(&mut self, receive_done: bool, spy: bool) {
        if let Some(b) = self.board.as_mut() {
            b.receive_done = receive_done;
            b.spy = spy;
        }
    }

    /// A frame the behavioral board counted in Lost Count, oldest first:
    /// when, whose it was, and whether the board aborted the sender ---
    /// AIM-628 §7's packets "which would have been received if the
    /// incoming packet buffer had not been busy".
    pub fn board_lost(&mut self) -> Option<(u64, u16, bool)> {
        self.board.as_mut().and_then(|b| b.lost.pop_front())
    }

    pub fn nodes(&self) -> &[Box<dyn Node>] {
        &self.nodes
    }

    pub fn nodes_mut(&mut self) -> &mut [Box<dyn Node>] {
        &mut self.nodes
    }

    /// The cable's level: high while anything drives it.
    pub fn level(&self) -> bool {
        self.level
    }

    /// Whether the behavioral board's line driver is on: the busy
    /// receiver's abort signal, AIM-628 §2.5, which is the only thing
    /// that board drives the cable with of its own accord --- its frames
    /// are model transmitters here.  The netlist board's own driver is
    /// read off `TRANS.DATA+` instead, [`super::cable::OnCable::board_tx`].
    pub fn board_driving(&self) -> bool {
        self.board.as_ref().is_some_and(|b| b.aborting)
    }

    /// What the model transmitters drive: high while any of them does.
    fn model_tx(&self) -> bool {
        self.sending.iter().any(|s| s.level)
    }

    /// What the board's line driver puts on the cable: the netlist
    /// board's transceiver as it last read, and the behavioral board's
    /// busy-receiver abort signal while it stands.
    fn board_high(&self) -> bool {
        self.board_tx || self.board.as_ref().is_some_and(|b| b.aborting)
    }

    /// Whether the netlist board's transceiver would report
    /// interference: it is driving high and so is another transceiver on
    /// the cable.  AIM-628 §2.3, a transceiver "detects interference
    /// (another transceiver transmitting at the same time as this one)
    /// and informs the interface" --- so every other driver counts and
    /// not only a frame.  The behavioral board's busy-receiver abort
    /// signal is one of them, which is §2.6's "the transmitter does not
    /// distinguish receiver-busy aborts from real collisions" (memo page
    /// 8); `a_busy_model_receiver_stops_the_netlist_board_which_reads_transmit_abort`
    /// in `tests/chaos_two_boards.rs` holds it.  The one driver that is
    /// not interference is this transceiver's own: it hears itself, which
    /// is why the cable's level takes it in and this does not.
    pub fn interference(&self) -> bool {
        self.board_tx && (self.model_tx() || self.board.as_ref().is_some_and(|b| b.aborting))
    }

    /// Whether the cable is busy: something has been on it within the
    /// idle time.
    pub fn busy(&self, now: u64) -> bool {
        self.level || self.last_edge.is_some_and(|e| now < e + self.ns(wire::IDLE_NS))
    }

    fn set_level(&mut self, now: u64, level: bool) {
        if level != self.level {
            self.level = level;
            self.last_edge = Some(now);
            self.decoder.edge(now, level);
        }
    }

    /// How many transceivers drive high: two or more is interference to
    /// each of them, AIM-628 §2.3.
    fn high(&self) -> usize {
        self.board_high() as usize + self.sending.iter().filter(|s| s.level).count()
    }

    /// Interference at `now`, for the record: once an overlap. The
    /// board's own detector is [`Ether::interference`]; a model
    /// transmitter's is its clock in [`Ether::play`].
    fn interfere(&mut self, now: u64) {
        if self.high() >= 2 && !self.collided {
            self.collided = true;
            self.record(Event::Collision(now));
        }
    }

    /// The board's transceiver is driving `tx` at `now`. Returns whether
    /// the cable's level changed.
    pub fn board(&mut self, now: u64, tx: bool) -> bool {
        let before = self.level;
        // What the model transmitters did up to `now`, with the board as
        // it was driving until then.
        self.play(now);
        // A frame's first edge from the netlist board: its transceiver
        // taking an idle cable high.  The board makes its own bits, so
        // there is no [`Transmission`] to carry the instants the
        // behavioral board's receiver decides at, and they are timed from
        // here.  Its driver coming on inside a frame is its own busy
        // receiver's abort signal instead, and starts nothing.
        if tx && !self.board_tx && !self.busy(now) && self.board.is_some() {
            self.from_cable = Some(FromCable {
                ract_at: Some(now + self.ns(RACT_NS)),
                dest_at: Some(now + self.ns(BUSY_ABORT_NS)),
                ract_off: false,
            });
        }
        self.board_tx = tx;
        self.interfere(now);
        self.set_level(now, self.board_high() || self.model_tx());
        self.level != before
    }

    /// Whether a model transmitter could hear interference: another
    /// transmitter is on the cable. Its clock edges matter only then.
    fn contended(&self) -> bool {
        self.board_high() || self.sending.len() >= 2
    }

    /// When the ether next has something of its own to do.
    pub fn next_due(&self) -> Option<u64> {
        let contended = self.contended();
        let edges = self.sending.iter().filter_map(|s| s.waveform.front().map(|&(t, _)| t));
        let ticks = self.sending.iter().filter_map(|s| (contended && !s.deaf).then_some(s.tick));
        let dests = self.sending.iter().flat_map(|s| [s.ract_at, s.dest_at]).flatten();
        let from_cable = self.from_cable.iter().flat_map(|f| [f.ract_at, f.dest_at]).flatten();
        let starts = self.waiting.iter().map(|w| w.at);
        let pending = self.board.as_ref().and_then(|b| b.pending.as_ref().map(|&(t, _)| t));
        let abort = self.board.as_ref().and_then(|b| b.abort_until);
        edges
            .chain(ticks)
            .chain(dests)
            .chain(from_cable)
            .chain(starts)
            .chain(pending)
            .chain(abort)
            .chain(self.decoder.next_due())
            .min()
    }

    /// A frame's destination word has gone by at `t`, [`BUSY_ABORT_NS`]
    /// into it: what the behavioral board's receiver makes of it,
    /// AIM-628 §2.5's hardware flow control as the netlist board wires
    /// it.  `RACT`, the 74S74 at LMRCLK 0C06, is off for a frame that
    /// found Receive Done up at its `START^` ([`RACT_NS`], `ract_off`),
    /// so the receiver takes nothing of the frame; the 74S10 at
    /// LMMYNM 0D02 makes `-LOST.ONE` from `MATCH SO FAR`, `ITS.ME` and
    /// `-RACT`, which steps Lost Count and presets the `ABORT` flip-flop
    /// at LMMODU 0A09, and the 26LS31 at LMLNDR 0A02 holds the cable
    /// high for [`ABORT_HOLD_NS`] --- the abort signal, which stops the
    /// sender at its next clock edge as a collision would.
    ///
    /// `MATCH SO FAR` is the bit-by-bit comparison alone, so only what is
    /// **specifically addressed** to the board is aborted; `DEST MATCH`
    /// is "mine, or zero, or spying", so a broadcast and anything under
    /// Spy is counted and not aborted, and another station's frame is
    /// neither.  Held to the board by
    /// `the_busy_receiver_aborts_only_what_is_addressed_to_it` in
    /// `tests/chaos_netlist.rs`.
    fn busy_receiver(&mut self, t: u64, source: u16, dest: u16, ract_off: bool) {
        let hold = self.ns(ABORT_HOLD_NS);
        let Some(b) = self.board.as_mut() else { return };
        if !ract_off {
            return;
        }
        if dest != b.address && dest != 0 && !b.spy {
            return;
        }
        let abort = dest == b.address;
        b.lost.push_back((t, source, abort));
        if abort {
            b.aborting = true;
            b.abort_until = Some(t + hold);
        }
    }

    /// The model transmitters' edges and clock edges due by `now`, in
    /// their order, with the cable's level and any interference at each.
    /// At a transmitter's clock edge with interference standing --- it
    /// and another driving high --- it aborts, as `ABORT` aborts the
    /// board's: the frame is given up, its words handed back and the
    /// board told, and the cable held high for [`ABORT_HOLD_NS`]. A
    /// transmitter whose frame, or abort signal, is over leaves the cable.
    fn play(&mut self, now: u64) {
        let (tick, hold) = (self.ns(ABORT_NS), self.ns(ABORT_HOLD_NS));
        loop {
            let contended = self.contended();
            let due = self
                .sending
                .iter()
                .flat_map(|s| {
                    [
                        s.waveform.front().map(|&(t, _)| t),
                        (contended && !s.deaf).then_some(s.tick),
                        s.ract_at,
                        s.dest_at,
                    ]
                })
                .flatten()
                .chain(self.board.as_ref().and_then(|b| b.abort_until))
                .chain(self.from_cable.iter().flat_map(|f| [f.ract_at, f.dest_at]).flatten())
                .min();
            let Some(t) = due.filter(|&t| t <= now) else { break };
            // As the levels stood up to `t`: what a clock edge at `t`
            // finds.
            let high = self.high();
            let mut aborted = Vec::new();
            let mut over = Vec::new();
            for (k, s) in self.sending.iter_mut().enumerate() {
                let mut abort = false;
                while s.tick <= t {
                    if s.tick == t && contended && !s.deaf && s.level && high >= 2 {
                        abort = true;
                    }
                    s.tick += tick;
                }
                if abort {
                    s.waveform = VecDeque::from([(t + hold, false)]);
                    s.level = true;
                    s.deaf = true;
                    aborted.push((s.source, std::mem::take(&mut s.buffer)));
                    continue;
                }
                while let Some(&(at, level)) = s.waveform.front()
                    && at <= t
                {
                    s.waveform.pop_front();
                    s.level = level;
                }
                if s.waveform.is_empty() {
                    over.push(k);
                }
            }
            // The board's abort signal run out, then the destination
            // words that have gone by at `t`: the driver those turn on is
            // found by the senders at their next clock edge, not at this
            // one, which is why the decisions come after the edges above.
            if let Some(b) = self.board.as_mut()
                && b.abort_until == Some(t)
            {
                b.aborting = false;
                b.abort_until = None;
            }
            // `START^` for the frames that began [`RACT_NS`] ago: `RACT`
            // takes Receive Done as it stands now and keeps it for the
            // frame, whatever the software writes after.
            // The board's own frame is no exception: its receiver hears it
            // as it hears any, so with the buffer full it is counted, and
            // to its own address its own busy receiver aborts it.
            let full = self.board.as_ref().is_some_and(|b| b.receive_done);
            let mut inactive = false;
            for s in self.sending.iter_mut().filter(|s| s.ract_at == Some(t)) {
                s.ract_at = None;
                s.ract_off = full;
                inactive |= s.ract_off;
            }
            if inactive && let Some(b) = self.board.as_mut() {
                b.not_taken = true;
            }
            // And `START^` for a frame the netlist board is putting on the
            // cable, timed from its first edge.
            if self.from_cable.as_ref().is_some_and(|f| f.ract_at == Some(t)) {
                let full = self.board.as_ref().is_some_and(|b| b.receive_done);
                if let Some(f) = self.from_cable.as_mut() {
                    f.ract_at = None;
                    f.ract_off = full;
                }
                if full && let Some(b) = self.board.as_mut() {
                    b.not_taken = true;
                }
            }
            let arrived: Vec<(u16, u16, bool)> = self
                .sending
                .iter_mut()
                .filter(|s| s.dest_at == Some(t))
                .map(|s| {
                    s.dest_at = None;
                    (s.source, s.dest, s.ract_off)
                })
                .collect();
            for (source, dest, ract_off) in arrived {
                self.busy_receiver(t, source, dest, ract_off);
            }
            // The netlist board's frame the same way, its two addresses
            // read off the cells that have gone by: the behavioral
            // board's receiver decides on what it has heard, as the
            // netlist board's receiver does.  A frame cut short before
            // its destination word --- wreckage, or one aborted --- has
            // nothing to decide on and is let be.
            if self.from_cable.as_ref().is_some_and(|f| f.dest_at == Some(t)) {
                let ract_off = self.from_cable.take().unwrap().ract_off;
                if let Some((source, dest)) = header_on_the_wire(self.decoder.bits()) {
                    self.busy_receiver(t, source, dest, ract_off);
                }
            }
            self.interfere(t);
            self.set_level(t, self.board_high() || self.model_tx());
            for (source, buffer) in aborted {
                // The frame is over at the end of its abort signal, and
                // its station's host starts writing again from there.
                self.sent_until.insert(source, t + hold);
                if let Some(b) = self.board.as_mut() {
                    b.collisions.push_back((t, source, t + hold));
                }
                if let Some(n) = self.nodes.iter_mut().find(|n| n.address() == source) {
                    n.aborted(t, buffer);
                }
            }
            for k in over.into_iter().rev() {
                let s = self.sending.remove(k);
                if let Some(b) = self.board.as_mut()
                    && b.address == s.source
                {
                    b.sending_until = None;
                }
            }
        }
        // A clock edge that was not looked at --- the transmitter alone on
        // the cable, nothing to hear --- is past: the next is the first
        // after `now`, on the same grid.
        for s in &mut self.sending {
            while s.tick <= now {
                s.tick += tick;
            }
        }
    }

    /// A frame starts on the cable at `now`.
    fn start(&mut self, now: u64, me: u16, buffer: Vec<u16>, deaf: bool) {
        let bits = frame(&buffer, me);
        if self.logging {
            self.record(Event::Sent(now, me, buffer.clone()));
        }
        let mut waveform: VecDeque<(u64, bool)> = wire::encode_in(&bits, self.per_ns)
            .into_iter()
            .map(|(offset, level)| (now + offset, level))
            .collect();
        // The last change brings the line low; then it is idle.
        let end = now + bits.len() as u64 * self.ns(wire::CELL_NS);
        waveform.push_back((end, false));
        if let Some(b) = self.board.as_mut() {
            b.frames.push_back((now, me, end));
            if b.address == me {
                b.sending_until = Some(end);
            }
        }
        // Its host writes the next frame from this one's end; an abort
        // moves the end in, and moves that with it.
        self.sent_until.insert(me, end);
        let dest = buffer.last().copied().unwrap_or(0);
        let (dest_after, ract_after) = (self.ns(BUSY_ABORT_NS), self.ns(RACT_NS));
        self.sending.push(Transmission {
            source: me,
            dest,
            dest_at: self.board.as_ref().map(|_| now + dest_after),
            ract_at: self.board.as_ref().map(|_| now + ract_after),
            ract_off: false,
            buffer,
            waveform,
            level: false,
            tick: now,
            deaf,
        });
    }

    /// The time is `now`: the model transmitters' edges due are made, the
    /// decoder is given the time, a packet that ended is delivered, and
    /// the frames due start --- the board's at its turn timer's instant,
    /// a node's at its turn if the cable is idle then. Returns whether the
    /// cable's level changed.
    pub fn at(&mut self, now: u64) -> bool {
        let before = self.level;
        self.play(now);
        if !self.busy(now) {
            self.collided = false;
        }
        // A run of bits over: a packet, or a collision's wreckage, which
        // every receiver takes as it takes a packet and judges by its check
        // word and bit count. The turn timers load from the source word as
        // it came, whatever the check says, as the board's does.
        if let Some(bits) = self.decoder.at(now) {
            let r = unframe_any(&bits);
            let from = r.framed.source;
            if r.bits >= 32 {
                self.last_source = Some(from);
            }
            for n in &mut self.nodes {
                if n.address() != from {
                    n.receive(now, &r.framed);
                }
            }
            if let Some(b) = self.board.as_mut() {
                // Whether its receiver was active for this run of bits.
                let taken = !std::mem::take(&mut b.not_taken);
                b.heard.push_back((now, r.clone(), taken));
            }
            self.record(Event::Heard(now, r.framed));
        }
        // The board's frame goes at its turn timer's instant, cable busy
        // or not: the timer took the cable idle at its terminal count,
        // and the transmitter is committed from there.
        if let Some(b) = self.board.as_mut()
            && b.pending.as_ref().is_some_and(|&(t, _)| t <= now)
        {
            let (at, buffer) = b.pending.take().unwrap();
            let source = b.address;
            self.waiting.push(Waiting {
                at,
                source,
                buffer,
                committed: true,
                deaf: false,
                station: false,
                node: None,
                reckoned: (self.last_edge, self.last_source),
            });
        }
        // A node is asked when the cable is free and it has no frame of
        // its own waiting: one node is one transmitter, and a station
        // whose turn is a long way off --- its host still refilling ---
        // does not keep the others from theirs.
        if !self.busy(now) && self.sending.is_empty() {
            for k in 0..self.nodes.len() {
                if self.waiting.iter().any(|w| w.node == Some(k)) {
                    continue;
                }
                if let Some(buffer) = self.nodes[k].transmit(now) {
                    let source = self.nodes[k].address();
                    let station = self.nodes[k].station();
                    let at = now.max(self.turn(now, source, station, buffer.len()));
                    self.waiting.push(Waiting {
                        at,
                        source,
                        buffer,
                        committed: false,
                        deaf: false,
                        station,
                        node: Some(k),
                        reckoned: (self.last_edge, self.last_source),
                    });
                }
            }
        }
        // A frame that has gone by since a waiting station's turn was
        // reckoned has loaded its counter afresh, so its turn is reckoned
        // again from the frame it heard last.  The harness's instrument
        // keeps the instant it was given, as it always has.
        let again: Vec<Option<u64>> = self
            .waiting
            .iter()
            .map(|w| {
                (!w.committed && w.station && w.reckoned != (self.last_edge, self.last_source))
                    .then(|| now.max(self.turn(now, w.source, w.station, w.buffer.len())))
            })
            .collect();
        let cable = (self.last_edge, self.last_source);
        for (w, at) in self.waiting.iter_mut().zip(again) {
            if let Some(at) = at {
                w.at = at;
                w.reckoned = cable;
            }
        }
        // Frames due: a transmitter's own instant goes; a turn goes if the
        // cable is idle at it, and holds otherwise. Judged before any of
        // them starts, so frames due together start together, and
        // collide.
        let busy = self.busy(now);
        let (due, rest): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.waiting).into_iter().partition(|w| w.at <= now);
        self.waiting = rest;
        for w in due {
            if w.committed || !busy {
                self.start(now, w.source, w.buffer, w.deaf);
            } else {
                // Someone took the cable first: wait for it to clear.
                self.waiting.push(Waiting { at: now + self.ns(1), ..w });
            }
        }
        // The first edges of what just started are at `now`.
        self.play(now);
        self.level != before
    }

    /// When a node `me` may next transmit: after the cable has been idle,
    /// its turn comes [`turn_byte`] counts and one later, as the board's
    /// timer would have it --- the nodes stand in for interfaces built to
    /// the same rule, without a timer's phase of their own.
    ///
    /// **A station's turn comes round again every [`ROUND_NS`] while the
    /// cable stays idle**, and it takes the first that is not before its
    /// host has this frame ready: its last frame's end and [`refill`] of
    /// the `words` this one takes to write.  So a station that has just
    /// sent, whose counter its own source word loaded with zero, has its
    /// turn at the first count after the cable idles --- and misses it,
    /// because no host can write a packet into the buffer in a
    /// microsecond --- and goes a whole round later: 257 slots for a
    /// short frame, as the netlist board takes them
    /// (`two_packets_back_to_back_wait_a_whole_round` in
    /// `tests/chaos_netlist.rs`), and 769 for a full one, whose 253 words
    /// take more than two rounds to write at the pace the board's transmit
    /// shift register takes them.
    ///
    /// **And the counter runs free.**  While the cable is idle it counts
    /// down on every other terminal count whether anyone is ready or not
    /// ([`super::board`]'s `Turn::tc` and `Turn::idle_run`), so a station
    /// that is asked long after the cable went idle has missed the turns
    /// in between and takes the next of the same cadence: the first
    /// `idle + (turn_byte + 1) × SLOT_NS + k × ROUND_NS` that is at or
    /// after both `now` and its refill, and never the instant it happened
    /// to be asked.
    ///
    /// A node that is not a station, [`Node::station`], is the harness's
    /// instrument and keeps the ether's old rule: its turn counts and one
    /// from when the cable went idle or from when it was asked, whichever
    /// is later.
    fn turn(&self, now: u64, me: u16, station: bool, words: usize) -> u64 {
        let idle_at = self.last_edge.map_or(now, |e| e + self.ns(wire::IDLE_NS));
        let slots = turn_byte(self.last_source.unwrap_or(me), me) as u64 + 1;
        let (slot, round) = (self.ns(SLOT_NS), self.ns(ROUND_NS));
        if !station {
            return idle_at.max(now) + slots * slot;
        }
        let first = idle_at + slots * slot;
        let ready = self.sent_until.get(&me).map_or(0, |&e| e + self.ns(refill(words))).max(now);
        if first >= ready { first } else { first + (ready - first).div_ceil(round) * round }
    }
}

impl Default for Ether {
    fn default() -> Self {
        Ether::new()
    }
}

/// A node that keeps what it hears and sends what it is given: the
/// harness's end of the cable.
#[derive(Default)]
pub struct Capture {
    pub address: u16,
    pub heard: Vec<(u64, Framed)>,
    pub to_send: VecDeque<Vec<u16>>,
}

impl Capture {
    pub fn new(address: u16) -> Capture {
        Capture { address, ..Default::default() }
    }
}

impl Node for Capture {
    fn address(&self) -> u16 {
        self.address
    }
    /// **Not a station.**  This is the harness's end of the cable: the
    /// tests that use it put frames on back to back on purpose, to see
    /// what a board makes of a second frame arriving while it is busy
    /// with the first, which is exactly what a station's refill would
    /// prevent.
    fn station(&self) -> bool {
        false
    }
    fn receive(&mut self, now: u64, packet: &Framed) {
        self.heard.push((now, packet.clone()));
    }
    fn transmit(&mut self, _now: u64) -> Option<Vec<u16>> {
        self.to_send.pop_front()
    }
}
