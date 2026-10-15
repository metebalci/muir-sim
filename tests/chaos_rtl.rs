// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Chaosnet interface as `rtl`'s I/O board has it, held to the netlist
//! board: the same register sequences run on both --- the `lm*` pages of
//! `data/CADRIO.netlist` under a Unibus master, and
//! `chaos::board::Interface` under the same reads and writes --- and every
//! CSR read, every word read back and every bit count must agree.  Times
//! are reported, not held: the behavioral interface takes a frame's bits
//! at the cable's rate and its turn, the board takes what its counters
//! take.

use muir::chaos::ether::{Capture, Ether, Event, turn_byte};
use muir::chaos::interface::{self as chaos, csr};
use muir::chaos::packet::{Packet, op};
use muir::chaos::wire;
use muir::ioboard::IoBoard;
use muir::netlist::Netlist;
use muir::part::Level;
use muir::unibus::UnibusMaster;

mod support;
use support::server::Server;
use support::time::Time;
use support::{cadrio, quiet};

const MY_ADDRESS: u16 = 0o3050;
const SERVER: u16 = 0o3060;

fn board(n: &Netlist) -> UnibusMaster<'_> {
    let mut b = UnibusMaster::new(n, 10_000, &quiet());
    for (reference, closed) in chaos::switches(MY_ADDRESS) {
        b.chip.set_switches(reference, closed);
    }
    b.run(b.now + 2_000);
    b
}

/// The behavioral interface at the same address, with `ether` on its
/// cable or none, and a clock of its own.
struct Model {
    io: IoBoard,
    now: u64,
}

impl Model {
    fn new(ether: Option<Ether>) -> Model {
        let mut io = IoBoard::default();
        io.plug_chaos(MY_ADDRESS, ether, 0, false);
        Model { io, now: 10_000 }
    }

    /// A register cycle at `at`, the instant the board's own cycle ended:
    /// the two are held to one clock, so that what depends on the instant
    /// --- the turn timer --- can be compared.  Returns the word read.
    fn cycle(&mut self, uaddr: u32, write: Option<u16>, at: u64) -> u16 {
        self.now = at;
        match write {
            Some(v) => {
                self.io.write(uaddr, v, self.now);
                0
            }
            None => self.io.read(uaddr, self.now),
        }
    }

    fn run(&mut self, until: u64) {
        self.now = until;
        self.io.advance(self.now);
    }
}

/// One register cycle on both, the words compared when read.
fn both(b: &mut UnibusMaster, m: &mut Model, uaddr: u32, write: Option<u16>, what: &str) -> u16 {
    let (_, wb) = b.cycle(uaddr, write);
    let wm = m.cycle(uaddr, write, b.now);
    if write.is_none() {
        assert_eq!(wm, wb, "{what}: the board reads {wb:#o}, the model {wm:#o}");
    }
    wb
}

/// Waits on both until `bit` is up in the CSR, polling as CC's driver
/// would, the two polled at the same instants; the CSRs then, and the
/// time each took --- the same poll, if the model has the board's timing.
fn wait_for(
    b: &mut UnibusMaster,
    m: &mut Model,
    bit: u16,
    for_ns: u64,
) -> ((u64, u16), (u64, u16)) {
    let t0 = b.now;
    let (mut cb, mut cm) = (None, None);
    loop {
        b.run(b.now + 5_000);
        let (_, c) = b.cycle(chaos::CSR, None);
        let now = b.now;
        let c2 = m.cycle(chaos::CSR, None, now);
        if cb.is_none() && (c & bit != 0 || now >= t0 + for_ns) {
            cb = Some((now - t0, c));
        }
        if cm.is_none() && (c2 & bit != 0 || now >= t0 + for_ns) {
            cm = Some((now - t0, c2));
        }
        if let (Some(x), Some(y)) = (cb, cm) {
            return (x, y);
        }
    }
}

fn rfc_time(from: u16, to: u16) -> Vec<u16> {
    Packet {
        opcode: op::RFC,
        forward: 0,
        dest: to,
        dest_index: 0,
        source: from,
        source_index: 0o21,
        number: 1,
        ack: 0,
        data: b"TIME".to_vec(),
    }
    .to_buffer(to)
}

/// **A packet looped back comes out of both the same.** Reset, Loop Back
/// with Clear Receiver, the words in, START: Transmit Done and Receive
/// Done, no abort, no CRC error, the same bit count, the same words back
/// --- the packet, the destination, the source the interface put in, the
/// check word --- and the count `7777` once read out.
#[test]
fn a_packet_loops_back_through_both_alike() {
    let n = cadrio();
    let mut b = board(&n);
    let mut m = Model::new(None);
    let words = rfc_time(MY_ADDRESS, MY_ADDRESS);

    both(&mut b, &mut m, chaos::CSR, Some(csr::RESET), "reset");
    b.run(b.now + 2_000);
    m.run(b.now);
    let after_reset = both(&mut b, &mut m, chaos::CSR, None, "the CSR after reset");
    eprintln!("CSR after reset: {after_reset:#08o}");
    both(&mut b, &mut m, chaos::CSR, Some(csr::LOOP_BACK | csr::CLEAR_RECEIVER), "loop back");
    let c0 = both(&mut b, &mut m, chaos::CSR, None, "the CSR with loop back on");
    assert!(c0 & csr::LOOP_BACK != 0 && c0 & csr::RECEIVE_DONE == 0);
    both(&mut b, &mut m, chaos::MY_ADDRESS, None, "the address");
    for &w in &words {
        both(&mut b, &mut m, chaos::WRITE_BUFFER, Some(w), "a word in");
    }
    let c1 = both(&mut b, &mut m, chaos::CSR, None, "the CSR with a packet written");
    assert!(c1 & csr::TRANSMIT_DONE == 0, "writing clears Transmit Done: {c1:#08o}");
    let me = both(&mut b, &mut m, chaos::START, None, "START");
    assert_eq!(me, MY_ADDRESS);

    let ((tb, cb), (tm, cm)) = wait_for(&mut b, &mut m, csr::RECEIVE_DONE, 3_000_000);
    eprintln!("Receive Done: board after {tb} ns ({cb:#08o}), model after {tm} ns ({cm:#08o})");
    assert_eq!(cm, cb, "the CSR at Receive Done");
    assert_eq!(tm, tb, "Receive Done at the same poll on both: the turn timer's wait");
    assert!(
        cb & csr::TRANSMIT_DONE != 0 && cb & csr::TRANSMIT_ABORT == 0 && cb & csr::CRC_ERROR == 0
    );
    let bits = both(&mut b, &mut m, chaos::BIT_COUNT, None, "the bit count");
    assert_eq!(bits as usize, (words.len() + 2) * 16 - 1);
    let mut back = Vec::new();
    for k in 0..words.len() + 2 {
        back.push(both(&mut b, &mut m, chaos::READ_BUFFER, None, &format!("word {k} back")));
    }
    assert_eq!(&back[..words.len()], &words[..]);
    assert_eq!(back[words.len()], MY_ADDRESS, "the source both put in");
    assert_eq!(both(&mut b, &mut m, chaos::BIT_COUNT, None, "the count read out"), 0o7777);
    let c2 = both(&mut b, &mut m, chaos::CSR, None, "the CSR read out");
    assert!(c2 & csr::CRC_ERROR == 0);
    both(&mut b, &mut m, chaos::CSR, Some(csr::CLEAR_RECEIVER), "clear receiver");
    let c3 = both(&mut b, &mut m, chaos::CSR, None, "the CSR cleared");
    assert!(c3 & csr::RECEIVE_DONE == 0);
}

/// **A request for the time is answered on both.** A host at 3060 with a
/// fixed time on each cable; the interface sends `RFC TIME` and the answer
/// comes back into its receiver: the same CSR, the same bit count, the
/// same words --- an `ANS` from 3060 with the time, low byte first ---
/// and the server's address as the source.
#[test]
fn the_time_is_asked_and_answered_on_both_alike() {
    let n = cadrio();
    let mut b = board(&n);
    let ether_for = || {
        let mut server = Server::new(SERVER);
        server.serve(Box::new(Time::fixed(0x1234_5678)));
        let mut e = Ether::new();
        e.keep_log(true);
        e.attach(Box::new(server));
        e
    };
    b.plug_chaos(ether_for());
    let mut m = Model::new(Some(ether_for()));
    both(&mut b, &mut m, chaos::CSR, Some(csr::RESET), "reset");
    b.run(b.now + 2_000);
    m.run(b.now);
    both(&mut b, &mut m, chaos::CSR, Some(csr::CLEAR_RECEIVER), "clear receiver");
    let words = rfc_time(MY_ADDRESS, SERVER);
    for &w in &words {
        both(&mut b, &mut m, chaos::WRITE_BUFFER, Some(w), "a word in");
    }
    both(&mut b, &mut m, chaos::START, None, "START");
    let ((tb, cb), (tm, cm)) = wait_for(&mut b, &mut m, csr::RECEIVE_DONE, 3_000_000);
    eprintln!("the answer: board after {tb} ns ({cb:#08o}), model after {tm} ns ({cm:#08o})");
    if cm != cb {
        let e = m.io.chaos.as_ref().unwrap().ether().unwrap();
        for ev in &e.log {
            eprintln!("  model ether: {ev:?}");
        }
        eprintln!(
            "  model ether next due {:?}, sending until {:?}",
            e.next_due(),
            e.board_sending_until()
        );
    }
    assert_eq!(cm, cb, "the CSR at Receive Done");
    assert_eq!(tm, tb, "the answer at the same poll on both: both turn timers' waits");
    assert!(
        cb & csr::TRANSMIT_DONE != 0 && cb & csr::RECEIVE_DONE != 0 && cb & csr::CRC_ERROR == 0
    );
    let bits = both(&mut b, &mut m, chaos::BIT_COUNT, None, "the bit count");
    let count = (bits as usize + 1) / 16;
    let mut back = Vec::new();
    for k in 0..count {
        back.push(both(&mut b, &mut m, chaos::READ_BUFFER, None, &format!("word {k} back")));
    }
    let (ans, _) = Packet::from_buffer(&back[..count - 2]).unwrap();
    assert_eq!(ans.opcode, op::ANS);
    assert_eq!((ans.source, ans.dest, ans.dest_index), (SERVER, MY_ADDRESS, 0o21));
    assert_eq!(ans.data, [0x78, 0x56, 0x34, 0x12], "the universal time, low byte first");
    assert_eq!(back[count - 2], SERVER, "from the host");
    assert_eq!(both(&mut b, &mut m, chaos::BIT_COUNT, None, "the count read out"), 0o7777);
}

/// **A collision aborts the transmission on both alike.** Both interfaces
/// send the same packet; once both frames are on the cable, a transmitter
/// that has not heard either cable starts the same frame on each, at the
/// same instant. Both read Transmit Done with Transmit Abort, at the same
/// poll, and neither received anything.
#[test]
fn a_collision_aborts_the_transmission_on_both_alike() {
    let n = cadrio();
    let mut b = board(&n);
    let ether_for = || {
        let mut e = Ether::new();
        e.keep_log(true);
        e.attach(Box::new(Capture::new(SERVER)));
        e
    };
    b.plug_chaos(ether_for());
    let mut m = Model::new(Some(ether_for()));
    both(&mut b, &mut m, chaos::CSR, Some(csr::RESET), "reset");
    b.run(b.now + 2_000);
    m.run(b.now);
    both(&mut b, &mut m, chaos::CSR, Some(csr::CLEAR_RECEIVER), "clear receiver");
    let words = rfc_time(MY_ADDRESS, SERVER);
    for &w in &words {
        both(&mut b, &mut m, chaos::WRITE_BUFFER, Some(w), "a word in");
    }
    both(&mut b, &mut m, chaos::START, None, "START");
    let t0 = b.now;
    loop {
        b.run(b.now + 1_000);
        m.run(b.now);
        let on_b = b.chaos.as_ref().unwrap().ether.busy(b.now);
        let on_m = m.io.chaos.as_ref().unwrap().ether().unwrap().busy(b.now);
        if on_b && on_m {
            break;
        }
        assert!(b.now < t0 + 3_000_000, "both transmitted: board {on_b}, model {on_m}");
    }
    let at = b.now;
    eprintln!("both frames on the cable {} ns after START", at - t0);
    // For a third host, so that neither receiver has a say in this.
    let other = rfc_time(SERVER, 0o3070);
    b.chaos.as_mut().unwrap().ether.send_now(at, SERVER, other.clone());
    m.io.chaos.as_mut().unwrap().ether_mut().unwrap().send_now(at, SERVER, other);
    let ((tb, cb), (tm, cm)) = wait_for(&mut b, &mut m, csr::TRANSMIT_DONE, 3_000_000);
    eprintln!("Transmit Done: board after {tb} ns ({cb:#08o}), model after {tm} ns ({cm:#08o})");
    for ev in &m.io.chaos.as_ref().unwrap().ether().unwrap().log {
        eprintln!("  model ether: {ev:?}");
    }
    assert!(cb & csr::TRANSMIT_ABORT != 0, "the board's transmission was aborted: {cb:#08o}");
    // CRC Error is up on both: the receiver is in the wreckage.
    assert_eq!(cm, cb, "the CSR at Transmit Done");
    assert_eq!(tm, tb, "at the same poll on both");
    // The other frame runs out; neither took it, and the CSRs agree whole.
    b.run(b.now + 300_000);
    m.run(b.now);
    let c = both(&mut b, &mut m, chaos::CSR, None, "the CSR with the cable idle again");
    assert_eq!(
        c & (csr::RECEIVE_DONE | csr::TRANSMIT_DONE | csr::TRANSMIT_ABORT),
        csr::TRANSMIT_DONE | csr::TRANSMIT_ABORT
    );
}

/// **Wreckage addressed to the board lands on both alike.** The same two
/// frames, from transmitters that heard neither cable, go on each at the
/// same instants: one for the board, and sixty-four cells into it another
/// over it. Both boards take what arrives --- Receive Done with CRC Error
/// --- and read back the same bit count and the same words.
#[test]
fn wreckage_addressed_to_the_board_lands_on_both_alike() {
    use muir::chaos::wire;
    let n = cadrio();
    let mut b = board(&n);
    let ether_for = || {
        let mut e = Ether::new();
        e.keep_log(true);
        e.attach(Box::new(Capture::new(SERVER)));
        e
    };
    b.plug_chaos(ether_for());
    let mut m = Model::new(Some(ether_for()));
    both(&mut b, &mut m, chaos::CSR, Some(csr::RESET), "reset");
    b.run(b.now + 2_000);
    m.run(b.now);
    both(&mut b, &mut m, chaos::CSR, Some(csr::CLEAR_RECEIVER), "clear receiver");
    let at = b.now;
    let first = rfc_time(SERVER, MY_ADDRESS);
    b.chaos.as_mut().unwrap().ether.send_now(at, SERVER, first.clone());
    m.io.chaos.as_mut().unwrap().ether_mut().unwrap().send_now(at, SERVER, first);
    b.run(at + 64 * wire::CELL_NS);
    m.run(b.now);
    let over = rfc_time(0o3061, 0o3070);
    b.chaos.as_mut().unwrap().ether.send_now(b.now, 0o3061, over.clone());
    m.io.chaos.as_mut().unwrap().ether_mut().unwrap().send_now(b.now, 0o3061, over);
    let ((tb, cb), (tm, cm)) = wait_for(&mut b, &mut m, csr::RECEIVE_DONE, 1_000_000);
    eprintln!("Receive Done: board after {tb} ns ({cb:#08o}), model after {tm} ns ({cm:#08o})");
    assert!(
        cb & csr::RECEIVE_DONE != 0 && cb & csr::CRC_ERROR != 0,
        "the board took wreckage: {cb:#08o}"
    );
    assert_eq!(cm, cb, "the CSR at Receive Done");
    assert_eq!(tm, tb, "at the same poll on both");
    let bits = both(&mut b, &mut m, chaos::BIT_COUNT, None, "the bit count");
    // Every word the buffer holds, the partial one at the top included,
    // and the count as it comes down with each.
    let words = (bits as usize + 1).div_ceil(16);
    eprintln!("bit count {bits}: {words} words, the top one partial");
    let (mut from_board, mut from_model) = (Vec::new(), Vec::new());
    for k in 0..words {
        let (_, wb) = b.cycle(chaos::READ_BUFFER, None);
        from_board.push(wb);
        from_model.push(m.cycle(chaos::READ_BUFFER, None, b.now));
        both(&mut b, &mut m, chaos::BIT_COUNT, None, &format!("the count after word {k}"));
    }
    let octal = |v: &[u16]| v.iter().map(|w| format!("{w:#o}")).collect::<Vec<_>>();
    eprintln!("read back from the board: {:?}", octal(&from_board));
    eprintln!("read back from the model: {:?}", octal(&from_model));
    assert_eq!(from_model, from_board, "the words back, wreckage and all");
    assert_eq!(both(&mut b, &mut m, chaos::BIT_COUNT, None, "the count read out"), 0o7777);
    both(&mut b, &mut m, chaos::CSR, Some(csr::CLEAR_RECEIVER), "clear receiver");
    let c = both(&mut b, &mut m, chaos::CSR, None, "the CSR cleared");
    assert!(c & csr::RECEIVE_DONE == 0);
}

/// **After Reset alone, before any Clear Receiver, does a packet for this
/// address land?**  AIM-628 says the clear-receiver bit "enables the
/// receiver to receive another packet"; whether a reset interface's
/// receiver is enabled to begin with is the board's to say, and the model
/// must say the same.  A node on each cable sends the interface a packet
/// right after the reset.
#[test]
fn a_reset_interface_receives_or_not_before_clear_receiver_alike() {
    let n = cadrio();
    let mut b = board(&n);
    let words = rfc_time(SERVER, MY_ADDRESS);
    let ether_for = || {
        let mut node = Capture::new(SERVER);
        node.to_send.push_back(words.clone());
        let mut e = Ether::new();
        e.attach(Box::new(node));
        e
    };
    b.cycle(chaos::CSR, Some(csr::RESET));
    b.run(b.now + 2_000);
    let mut m = Model::new(None);
    m.cycle(chaos::CSR, Some(csr::RESET), m.now + 250);
    m.run(b.now);
    b.plug_chaos(ether_for());
    m.io.chaos.as_mut().unwrap().plug(ether_for());
    let ((tb, cb), (tm, cm)) = wait_for(&mut b, &mut m, csr::RECEIVE_DONE, 1_000_000);
    eprintln!("after reset alone: board {cb:#08o} after {tb} ns, model {cm:#08o} after {tm} ns");
    assert_eq!(cm & csr::RECEIVE_DONE, cb & csr::RECEIVE_DONE, "whether the packet landed");
    assert_eq!(cm & csr::LOST_COUNT, cb & csr::LOST_COUNT, "and whether it counted as lost");
}

/// **The turn timer loads the address difference, bit-reversed, and waits
/// that many counts and one.**  The board at four addresses sends the
/// host at 3060 an RFC and hears its answer; the 74LS164s' `SERVER ADR
/// DIFF` at each `-LOAD.MY.TURN` is read off the nets, and the counts of
/// `MY.TURN CLK^` from the cable going idle to `MY.TURN^` rising are
/// counted: for the board's own packet the load is 0 and the turn comes at
/// the first count; for the server's the low byte loaded is
/// `turn_byte(3060, me)` --- bits 14 to 7 of `3060 - me`, bit 14 at the
/// bottom --- and the turn comes that many counts and one after.
#[test]
fn the_turn_timer_loads_the_address_difference_bit_reversed() {
    let n = cadrio();
    for me in [0o1050u16, 0o2450, 0o3450, 0o3250] {
        let mut b = UnibusMaster::new(&n, 10_000, &quiet());
        for (reference, closed) in chaos::switches(me) {
            b.chip.set_switches(reference, closed);
        }
        b.run(b.now + 2_000);
        let mut server = Server::new(SERVER);
        server.serve(Box::new(Time::fixed(0x1234_5678)));
        let mut e = Ether::new();
        e.attach(Box::new(server));
        b.plug_chaos(e);
        b.cycle(chaos::CSR, Some(csr::RESET));
        b.run(b.now + 2_000);
        b.cycle(chaos::CSR, Some(csr::CLEAR_RECEIVER));
        let words = rfc_time(me, SERVER);
        for &w in &words {
            b.cycle(chaos::WRITE_BUFFER, Some(w));
        }
        let started = b.now;
        b.cycle(chaos::START, None);
        let diff = |b: &UnibusMaster| -> u16 {
            (0..12)
                .map(|k| ((b.level(&format!("'HOST ADR DIFF.{k}'")) == Level::High) as u16) << k)
                .sum()
        };
        let (mut cbl_was, mut turn_was, mut clk_was, mut load_was) = (
            b.level("-CBLBSY"),
            b.level("MY.TURN^"),
            b.level("'MY.TURN CLK^'"),
            b.level("-LOAD.MY.TURN"),
        );
        let mut loads = Vec::new();
        let mut counts: Vec<u32> = Vec::new();
        let mut idle_at = None;
        let mut busy_at = None;
        let mut rdone_was = b.level("RDONE");
        let mut counts_since_idle = 0u32;
        let mut packets = 0;
        let mut report = String::new();
        while b.now < started + 900_000 {
            b.run(b.now + 10);
            let ld = b.level("-LOAD.MY.TURN");
            if ld != load_was && ld == Level::Low {
                loads.push(diff(&b));
            }
            load_was = ld;
            let cbl = b.level("-CBLBSY");
            if cbl != cbl_was {
                if cbl == Level::High {
                    idle_at = Some(b.now);
                    counts_since_idle = 0;
                    packets += 1;
                } else {
                    busy_at = Some(b.now);
                }
                cbl_was = cbl;
            }
            let rd = b.level("RDONE");
            if rd != rdone_was
                && rd == Level::High
                && let Some(bz) = busy_at
            {
                report += &format!(" [RDONE {} ns after the frame's first edge]", b.now - bz);
            }
            rdone_was = rd;
            let ck = b.level("'MY.TURN CLK^'");
            if ck != clk_was && ck == Level::High && idle_at.is_some() {
                counts_since_idle += 1;
            }
            clk_was = ck;
            let tn = b.level("MY.TURN^");
            if tn != turn_was
                && tn == Level::High
                && let Some(i) = idle_at
            {
                report += &format!(
                    " [packet {packets}: MY.TURN^ up {} ns after idle, count {counts_since_idle}]",
                    b.now - i
                );
                counts.push(counts_since_idle);
                idle_at = None;
            }
            turn_was = tn;
        }
        let want = turn_byte(SERVER, me);
        eprintln!(
            "me {me:o} host {SERVER:o}: loads (DIFF at load) {:?}; turn_byte {want:#04x};{report}",
            loads.iter().map(|d| format!("{d:#05x}")).collect::<Vec<_>>(),
        );
        assert_eq!(loads.len(), 2, "a load for the board's own packet and one for the server's");
        assert_eq!(loads[0] & 0xff, 0, "the board's own packet loads 0");
        assert_eq!((loads[1] & 0xff) as u8, want, "the server's packet loads turn_byte");
        assert_eq!(counts, [1, want as u32 + 1], "the counts to the turn after each");
    }
}

// --- The busy receiver, AIM-628 §2.5's hardware flow control -------------

/// A cable with a node at [`SERVER`] and these frames queued in it.
/// [`Capture`] is the harness's instrument and not a station, so the
/// frames go one after another at the cable's own turn, which is how a
/// second frame is made to arrive while the board is busy with the first.
fn ether_with(frames: &[Vec<u16>]) -> Ether {
    let mut node = Capture::new(SERVER);
    node.to_send.extend(frames.iter().cloned());
    let mut e = Ether::new();
    e.keep_log(true);
    e.attach(Box::new(node));
    e
}

/// Both interfaces reset, spying or not, their receivers cleared and a
/// cable of their own with `frames` queued on it: the first for this
/// address, so that it fills the buffer, and the rest behind it.  Returns
/// with the first packet in both buffers.
fn buffer_full(b: &mut UnibusMaster, m: &mut Model, spy: bool, frames: &[Vec<u16>]) {
    both(b, m, chaos::CSR, Some(csr::RESET), "reset");
    b.run(b.now + 2_000);
    m.run(b.now);
    let spying = if spy { csr::SPY } else { 0 };
    both(b, m, chaos::CSR, Some(csr::CLEAR_RECEIVER | spying), "clear receiver");
    b.plug_chaos(ether_with(frames));
    m.io.chaos.as_mut().unwrap().plug(ether_with(frames));
    // Watched off the board's `RDONE` and the model's CSR without a Unibus
    // cycle, so that this returns as the first packet lands, before the
    // second frame has started on either cable.
    let until = b.now + 1_000_000;
    loop {
        let next = b.chip.next_tap().map_or(b.now + 50, |t| t.max(b.now + 1)).min(b.now + 50);
        b.run(next);
        m.run(b.now);
        let model = m.io.chaos.as_ref().unwrap().csr();
        if b.level("RDONE") == Level::High && model & csr::RECEIVE_DONE != 0 {
            return;
        }
        assert!(b.now < until, "the first packet landed: model {model:#08o}");
    }
}

/// What each board's line driver did while `for_ns` passed, and when the
/// second frame started on each cable: `(sent, on, off)` a side, the
/// netlist board's driver read off `TRANS.DATA+` and the model's off its
/// ether.  Both are stepped to the same instants.
#[allow(clippy::type_complexity)]
fn watch_the_drivers(
    b: &mut UnibusMaster,
    m: &mut Model,
    for_ns: u64,
) -> [(Option<u64>, Option<u64>, Option<u64>); 2] {
    let until = b.now + for_ns;
    let mut edges = [(None, None, None), (None, None, None)];
    let mut was = [
        b.chaos.as_ref().unwrap().board_tx(&b.chip),
        m.io.chaos.as_ref().unwrap().ether().unwrap().board_driving(),
    ];
    while b.now < until {
        let next = b.chip.next_tap().map_or(b.now + 25, |t| t.max(b.now + 1)).min(b.now + 25);
        b.run(next);
        m.run(b.now);
        let now = [
            b.chaos.as_ref().unwrap().board_tx(&b.chip),
            m.io.chaos.as_ref().unwrap().ether().unwrap().board_driving(),
        ];
        for k in 0..2 {
            if now[k] != was[k] {
                if now[k] {
                    edges[k].1.get_or_insert(b.now);
                } else if edges[k].1.is_some() {
                    edges[k].2.get_or_insert(b.now);
                }
                was[k] = now[k];
            }
        }
    }
    let second = |e: &Ether| {
        e.log
            .iter()
            .filter_map(|ev| if let Event::Sent(t, _, _) = ev { Some(*t) } else { None })
            .nth(1)
    };
    edges[0].0 = second(&b.chaos.as_ref().unwrap().ether);
    edges[1].0 = second(m.io.chaos.as_ref().unwrap().ether().unwrap());
    edges
}

/// The frames each cable carried whole or in pieces: whether each check
/// word was good.
fn heard(e: &Ether) -> Vec<bool> {
    e.log
        .iter()
        .filter_map(|ev| if let Event::Heard(_, f) = ev { Some(f.check_ok) } else { None })
        .collect()
}

/// **A frame addressed to a full buffer is aborted on both alike.**
/// AIM-628 §2.5: "When a receiving interface determines that an incoming
/// packet is addressed to it, but its receive buffer already contains a
/// packet, it sends an abort signal which causes the transmitter to
/// stop."  The netlist board does it in its gates --- `-LOST.ONE` out of
/// the 74S10 at LMMYNM 0D02 presetting the `ABORT` flip-flop at LMMODU
/// 0A09, measured by `the_busy_receiver_aborts_a_frame_addressed_to_it`
/// in `tests/chaos_netlist.rs` --- and the behavioral interface has the
/// ether do it for it.  Held here to the same four things: the driver on
/// in the bit cell after the destination word, held four bit cells, Lost
/// Count at one, the packet already in the buffer untouched, and the
/// sender's frame ending as wreckage.
#[test]
fn a_frame_addressed_to_a_full_buffer_is_aborted_on_both_alike() {
    let n = cadrio();
    let mut b = board(&n);
    let mut m = Model::new(None);
    let first = rfc_time(SERVER, MY_ADDRESS);
    buffer_full(&mut b, &mut m, false, &[first.clone(), rfc_time(SERVER, MY_ADDRESS)]);
    let [netlist, model] = watch_the_drivers(&mut b, &mut m, 100_000);
    for (what, (sent, on, off)) in [("board", netlist), ("model", model)] {
        let sent = sent.unwrap_or_else(|| panic!("{what}: the second frame went on the cable"));
        let on = on.unwrap_or_else(|| panic!("{what}: the driver went on"));
        let off = off.unwrap_or_else(|| panic!("{what}: and off again"));
        eprintln!(
            "{what}: the second frame at {sent}, the driver on at {on} ({} ns, {} cells, in) and \
             off {} ns later",
            on - sent,
            (on - sent) / wire::CELL_NS,
            off - on
        );
        assert!(
            (48 * wire::CELL_NS..50 * wire::CELL_NS).contains(&(on - sent)),
            "{what}: the abort starts in the cell after the destination word"
        );
        assert!((1_000..1_250).contains(&(off - on)), "{what}: four bit cells of abort signal");
    }
    let c = both(&mut b, &mut m, chaos::CSR, None, "the CSR after the abort");
    eprintln!("CSR after the abort: {c:#08o}");
    assert_eq!((c & csr::LOST_COUNT) >> 9, 1, "the frame is counted lost: {c:#08o}");
    assert!(c & csr::RECEIVE_DONE != 0, "the first packet is still there: {c:#08o}");
    assert!(c & csr::CRC_ERROR == 0, "and its check is still good: {c:#08o}");
    let bits = both(&mut b, &mut m, chaos::BIT_COUNT, None, "the bit count");
    assert_eq!(bits as usize, (first.len() + 2) * 16 - 1, "the bit count is the first packet's");
    let mut back = Vec::new();
    for k in 0..first.len() + 2 {
        back.push(both(&mut b, &mut m, chaos::READ_BUFFER, None, &format!("word {k} back")));
    }
    assert_eq!(&back[..first.len()], &first[..], "the first packet, unharmed by the abort");
    for (what, e) in [
        ("board", &b.chaos.as_ref().unwrap().ether),
        ("model", m.io.chaos.as_ref().unwrap().ether().unwrap()),
    ] {
        let checks = heard(e);
        eprintln!("{what}: frames off the cable, check good: {checks:?}");
        assert_eq!(checks.len(), 2, "{what}: two frames went by");
        assert!(checks[0], "{what}: the first whole");
        assert!(!checks[1], "{what}: the second stopped before its end, its check bad");
    }
}

/// **A full buffer aborts only what is addressed to it, on both alike.**
/// AIM-628 §2.5: "Note that a receiver whose packet buffer is full will
/// only generate an abort signal if the packet was specifically addressed
/// to it."  On the board that is the `MATCH SO FAR` term on the 74S10 at
/// LMMYNM 0D02, the bit-by-bit comparison alone, against `DEST MATCH`'s
/// "mine, or zero, or spying"; so a broadcast, and anything at all under
/// Spy, is counted in Lost Count and not aborted, and another station's
/// packet is neither.  `the_busy_receiver_aborts_only_what_is_addressed_to_it`
/// in `tests/chaos_netlist.rs` measures the board; the model must say the
/// same of all three.
#[test]
fn a_full_buffer_aborts_only_what_is_addressed_to_it_on_both_alike() {
    for (what, dest, spy, lost) in [
        ("another station's packet", 0o3070, false, 0),
        ("a broadcast", 0, false, 1),
        ("another station's packet, spying", 0o3070, true, 1),
    ] {
        let n = cadrio();
        let mut b = board(&n);
        let mut m = Model::new(None);
        buffer_full(&mut b, &mut m, spy, &[rfc_time(SERVER, MY_ADDRESS), rfc_time(SERVER, dest)]);
        let [netlist, model] = watch_the_drivers(&mut b, &mut m, 100_000);
        eprintln!("{what}: board {netlist:?}, model {model:?}");
        assert!(netlist.0.is_some() && model.0.is_some(), "{what}: the second frame went out");
        assert_eq!(netlist.1, None, "{what}: the board does not drive the cable");
        assert_eq!(model.1, None, "{what}: nor does the model");
        let c = both(&mut b, &mut m, chaos::CSR, None, &format!("{what}: the CSR"));
        eprintln!("{what}: CSR {c:#08o}, Lost Count {}", (c & csr::LOST_COUNT) >> 9);
        assert!(c & csr::RECEIVE_DONE != 0, "{what}: the first packet is still there");
        assert_eq!(
            (c & csr::LOST_COUNT) >> 9,
            lost,
            "{what}: what would have been received is counted lost, and nothing else"
        );
    }
}

/// **Lost Count wraps at sixteen on both alike.** AIM-628 §7 has the
/// field as four bits of packets "which would have been received if the
/// incoming packet buffer had not been busy"; the board counts them in
/// the 74LS161 at LMMYNM 0F04, and a '161 wraps.  Eighteen frames with
/// the buffer taken by the first leave seventeen counted and the field
/// reading 1 --- measured on the board by `the_lost_count_wraps_at_sixteen`
/// in `tests/chaos_netlist.rs`, and the behavioral interface's count
/// wraps with it.
#[test]
fn the_lost_count_wraps_at_sixteen_on_both_alike() {
    let n = cadrio();
    let mut b = board(&n);
    let mut m = Model::new(None);
    let frames: Vec<Vec<u16>> = (0..18).map(|_| rfc_time(SERVER, MY_ADDRESS)).collect();
    buffer_full(&mut b, &mut m, false, &frames);
    let until = b.now + 1_400_000;
    while b.now < until {
        b.run(b.now + 500);
        m.run(b.now);
    }
    let sent = |e: &Ether| e.log.iter().filter(|ev| matches!(ev, Event::Sent(..))).count();
    let on_board = sent(&b.chaos.as_ref().unwrap().ether);
    let on_model = sent(m.io.chaos.as_ref().unwrap().ether().unwrap());
    let c = both(&mut b, &mut m, chaos::CSR, None, "the CSR after eighteen frames");
    eprintln!(
        "frames on the cable: board {on_board}, model {on_model}; CSR {c:#08o}, Lost Count {}",
        (c & csr::LOST_COUNT) >> 9
    );
    assert_eq!((on_board, on_model), (18, 18), "eighteen frames went by on each");
    assert_eq!((c & csr::LOST_COUNT) >> 9, 1, "seventeen counted, the field wrapped: {c:#08o}");
    assert!(c & csr::RECEIVE_DONE != 0, "the first one is still in the buffer");
}

/// **A Clear Receiver written inside a frame does not let that frame in,
/// on both alike.** `RACT`, the 74S74 at LMRCLK 0C06, clocks `-RDONE` in on
/// `START^` and holds it while the cable is busy, its clear being `RRESET`
/// or `-CBLBSY` through the 74S02 at 0B08; and `-LOST.ONE` at LMMYNM 0D02
/// takes `-RACT`, not `-RDONE`. So with the buffer full at a frame's
/// `START^`, a Clear Receiver written before its destination word leaves
/// the receiver off for it: nothing is stored, Receive Done stays down, and
/// the frame is counted in the Lost Count the clear just emptied --- and
/// aborted if it was addressed to the board by name, a broadcast not.
/// `START^` is watched on the board as well, [`RACT_NS`] after the frame's
/// first edge, which is where the model samples.
#[test]
fn a_clear_receiver_inside_a_frame_does_not_let_it_in_on_both_alike() {
    use muir::chaos::ether::{BUSY_ABORT_NS, RACT_NS};
    for (what, dest, abort) in [("addressed to it", MY_ADDRESS, true), ("a broadcast", 0, false)] {
        let n = cadrio();
        let mut b = board(&n);
        let mut m = Model::new(None);
        buffer_full(&mut b, &mut m, false, &[rfc_time(SERVER, MY_ADDRESS), rfc_time(SERVER, dest)]);
        let second = |e: &Ether| {
            e.log
                .iter()
                .filter_map(|ev| if let Event::Sent(t, _, _) = ev { Some(*t) } else { None })
                .nth(1)
        };
        // Until the second frame is past `START^` on both cables.
        let until = b.now + 1_000_000;
        let mut was = b.level("START^");
        let mut rise = None;
        let (on_board, on_model) = loop {
            let next = b.chip.next_tap().map_or(b.now + 25, |t| t.max(b.now + 1)).min(b.now + 25);
            b.run(next);
            m.run(b.now);
            let sb = second(&b.chaos.as_ref().unwrap().ether);
            let sm = second(m.io.chaos.as_ref().unwrap().ether().unwrap());
            let level = b.level("START^");
            if level != was {
                if level == Level::High && sb.is_some() {
                    rise.get_or_insert(b.now);
                }
                was = level;
            }
            if let (Some(x), Some(y)) = (sb, sm)
                && rise.is_some()
                && b.now >= x.max(y) + RACT_NS + 25
            {
                break (x, y);
            }
            assert!(b.now < until, "{what}: the second frame went out on both: {sb:?} {sm:?}");
        };
        let rise = rise.unwrap();
        eprintln!(
            "{what}: the second frame on the board at {on_board}, START^ {} ns after it; on the \
             model at {on_model}",
            rise - on_board
        );
        assert!(rise.abs_diff(on_board + RACT_NS) <= 25, "{what}: START^ where the model samples");
        both(&mut b, &mut m, chaos::CSR, Some(csr::CLEAR_RECEIVER), &format!("{what}: clear"));
        let into = b.now - on_board.max(on_model);
        eprintln!("{what}: Clear Receiver written, {into} ns into the frame");
        assert!(
            b.now < on_board.min(on_model) + BUSY_ABORT_NS,
            "{what}: the write landed before the destination word"
        );
        let [netlist, model] = watch_the_drivers(&mut b, &mut m, 150_000);
        for (side, (sent, on, off)) in [("board", netlist), ("model", model)] {
            let sent = sent.unwrap();
            match (on, off) {
                (Some(on), Some(off)) if abort => {
                    assert!(
                        (48 * wire::CELL_NS..50 * wire::CELL_NS).contains(&(on - sent)),
                        "{what}, {side}: the abort in the cell after the destination word"
                    );
                    assert!((1_000..1_250).contains(&(off - on)), "{what}, {side}: four cells");
                }
                (None, _) if !abort => {}
                other => panic!("{what}, {side}: the driver {other:?}"),
            }
        }
        let c = both(&mut b, &mut m, chaos::CSR, None, &format!("{what}: the CSR after"));
        eprintln!("{what}: CSR {c:#08o}");
        assert!(c & csr::RECEIVE_DONE == 0, "{what}: nothing was stored: {c:#08o}");
        assert_eq!((c & csr::LOST_COUNT) >> 9, 1, "{what}: and the frame was counted: {c:#08o}");
    }
}

// --- A clear inside a frame the receiver has taken -----------------------

/// How the receiver is cleared: a CSR write of Clear Receiver, or of
/// Reset, or `-UB INIT` on the Unibus.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Clear {
    Receiver,
    Reset,
    Init,
}

/// What one side showed of a frame with a clear inside it: the CSR read
/// just after the clear; under Loop Back, the poll Transmit Done came up
/// at, polled every 5 µs from the clear; the CSR once the frame is over
/// and the cable idle again, the bit count then and the words read back if
/// Receive Done is up; and the busy receiver's abort signal if its line
/// driver came on --- bit cells from the frame's first edge to the driver
/// on, and cells held.
#[derive(Debug, Default, PartialEq)]
struct Cleared {
    after: u16,
    done: Option<u64>,
    over: u16,
    bits: u16,
    words: Vec<u16>,
    abort: Option<(u64, u64)>,
}

/// One frame of `words` words to `dest` and a clear of kind `clear` `at`
/// nanoseconds after its first edge, negative for before it.
#[derive(Clone, Copy, Debug)]
struct Inside {
    words: usize,
    dest: u16,
    spy: bool,
    loop_back: bool,
    clear: Clear,
    at: i64,
}

impl Inside {
    /// The frame's length on the line: its words, the source and the
    /// check, and the zero after them.
    fn length(&self) -> u64 {
        ((self.words as u64 + 2) * 16 + 1) * wire::CELL_NS
    }

    /// What the board's receive control makes of it, by its gates on
    /// pages LMRCLK, LMRCTL and LMMYNM: `RACT` up at `START^` with the
    /// buffer free; `RRESET` from the clear takes it down, and `START^`
    /// does not come again in the frame. Before the destination word the
    /// frame then takes the busy receiver's path --- Lost Count for
    /// `DEST MATCH`, the abort for `MATCH SO FAR` --- and after it nothing
    /// at all; Receive Done never rises. A clear before the frame, or after
    /// it landed, is an ordinary one. A Reset or `-UB INIT` under Loop Back
    /// also takes Loop Back off and sets Transmit Done, and the frame
    /// reaches neither gate. Receive Done, Lost Count and the abort.
    fn rule(&self) -> (bool, u16, bool) {
        use muir::chaos::ether::BUSY_ABORT_NS;
        let mine = self.dest == MY_ADDRESS || self.dest == 0 || self.spy;
        let land = self.length() as i64 + 375;
        if self.loop_back && self.clear != Clear::Receiver {
            return (false, 0, false);
        }
        match self.at {
            at if at < 0 => (mine, 0, false),
            at if at < BUSY_ABORT_NS as i64 => (false, mine as u16, self.dest == MY_ADDRESS),
            at if at < land => (false, 0, false),
            _ => (false, 0, false),
        }
    }
}

/// A write of `v` to the CSR on both, the model's at the instant the
/// board's write strobe `WCSR` rises inside its cycle: the cycle as
/// [`UnibusMaster::cycle`] makes it, its lines driven here so that the
/// strobe can be watched.  A clear's instant is the strobe's on the board
/// --- `-11.SAYS.GO` and `-11.ZOT` are `WCSR` with a data bit --- and the
/// two are held to one instant so that what a clear leaves inside a frame
/// can be compared bit for bit.
fn write_at_strobe(b: &mut UnibusMaster, m: &mut Model, v: u16) {
    exact(b, m, chaos::CSR, Some(v));
}

/// A cycle on both as [`write_at_strobe`] makes a write: the board's cycle
/// with its lines driven here, and the model's at the instant the board
/// acts --- a write at `WCSR`'s rise, a read at `-SSYN*`'s fall, where the
/// master takes the word.  Returns the board's word and the model's, for a
/// read.
fn exact(b: &mut UnibusMaster, m: &mut Model, uaddr: u32, write: Option<u16>) -> (u16, u16) {
    use muir::busint::{UNIBUS_ADDRESS_NS, UNIBUS_STROBE_NS};
    let lines = |b: &UnibusMaster, prefix: &str, bits: std::ops::Range<u32>| {
        bits.map(|k| b.net(&format!("-{prefix}{k}*"))).collect::<Vec<_>>()
    };
    let put = |b: &mut UnibusMaster, nets: &[muir::netlist::NetId], word: u32| {
        for (k, &net) in nets.iter().enumerate() {
            if (word >> k) & 1 != 0 {
                b.chip.drive(net, Level::Low);
            } else {
                b.chip.pull_up(net);
            }
        }
    };
    let (addr, data) = (lines(b, "A", 1..18), lines(b, "D", 0..16));
    let (c1, msyn, ssyn) = (b.net("-C1*"), b.net("-MSYN*"), b.net("-SSYN*"));
    put(b, &addr, uaddr >> 1);
    match write {
        Some(v) => {
            b.chip.drive(c1, Level::Low);
            put(b, &data, v as u32);
        }
        None => {
            b.chip.pull_up(c1);
            put(b, &data, 0);
        }
    }
    b.chip.transition(b.now);
    b.run(b.now + UNIBUS_ADDRESS_NS);
    b.chip.drive(msyn, Level::Low);
    b.chip.transition(b.now);
    let mut strobe = None;
    let step = |b: &mut UnibusMaster| {
        let next = b.chip.next_tap().map_or(b.now + 5, |t| t.max(b.now + 1));
        b.run(next);
    };
    while b.chip.net(ssyn) != Level::Low {
        step(b);
        if strobe.is_none() && write.is_some() && b.level("WCSR") == Level::High {
            strobe = Some(b.now);
        }
    }
    let (wb, wm) = match write {
        Some(v) => {
            m.cycle(uaddr, Some(v), strobe.expect("WCSR rose before -SSYN*"));
            (0, 0)
        }
        None => (b.word(), m.cycle(uaddr, None, b.now)),
    };
    b.run(b.now + UNIBUS_STROBE_NS);
    b.chip.pull_up(msyn);
    b.chip.transition(b.now);
    while b.chip.net(ssyn) == Level::Low {
        step(b);
    }
    put(b, &addr, 0);
    put(b, &data, 0);
    b.chip.pull_up(c1);
    b.chip.transition(b.now);
    b.run(b.now + 200);
    m.run(b.now);
    (wb, wm)
}

/// The clear on both: the board's write, or its `-INIT*` held down a
/// microsecond, and the model's at the instant the board's strobe rose
/// or its `-INIT*` fell.
fn clear_both(b: &mut UnibusMaster, m: &mut Model, clear: Clear, keep: u16) {
    match clear {
        Clear::Receiver => write_at_strobe(b, m, csr::CLEAR_RECEIVER | keep),
        Clear::Reset => write_at_strobe(b, m, csr::RESET),
        Clear::Init => {
            let init = b.net("-INIT*");
            b.chip.drive(init, Level::Low);
            b.chip.transition(b.now);
            m.run(b.now);
            m.io.unibus_init();
            b.run(b.now + 1_000);
            b.chip.pull_up(init);
            b.chip.transition(b.now);
            m.run(b.now);
        }
    }
}

/// The frame of `c` on both, from [`SERVER`] on each cable or the board's
/// own under Loop Back, cleared inside it as `c` says. Returns what the
/// board and the model showed.
fn clear_inside(c: &Inside) -> (Cleared, Cleared) {
    use muir::chaos::ether::RACT_NS;
    let n = cadrio();
    let (mut b, mut m) = on_a_cable(&n);
    let keep = if c.spy { csr::SPY } else { 0 } | if c.loop_back { csr::LOOP_BACK } else { 0 };
    both(&mut b, &mut m, chaos::CSR, Some(csr::CLEAR_RECEIVER | keep), "clear receiver");
    // From a cycle's start to its write strobe, to put the clear where it
    // is wanted: the address's 100 ns and the decode.
    let lead = match c.clear {
        Clear::Init => 0,
        _ => 125,
    };
    let words = numbered(c.words, c.dest);
    let mut after = None;
    let t0 = if c.loop_back {
        for &w in &words {
            both(&mut b, &mut m, chaos::WRITE_BUFFER, Some(w), "a word in");
        }
        both(&mut b, &mut m, chaos::START, None, "START");
        // The frame's first edge, [`RACT_NS`] before the board's `START^`.
        let until = b.now + 1_000_000;
        let mut was = b.level("START^");
        loop {
            let next = b.chip.next_tap().map_or(b.now + 25, |t| t.max(b.now + 1)).min(b.now + 25);
            b.run(next);
            m.run(b.now);
            let level = b.level("START^");
            if level == Level::High && was == Level::Low {
                break b.now - RACT_NS;
            }
            was = level;
            assert!(b.now < until, "{c:?}: the frame looped back");
        }
    } else {
        let t0 = b.now + 20_000 + c.at.unsigned_abs();
        if c.at < 0 {
            b.run(t0 - c.at.unsigned_abs() - lead);
            m.run(b.now);
            clear_both(&mut b, &mut m, c.clear, keep);
            after = Some(pair(&mut b, &mut m, chaos::CSR));
        }
        b.run(t0);
        m.run(t0);
        b.chaos.as_mut().unwrap().ether.send_now(t0, SERVER, words.clone());
        m.io.chaos.as_mut().unwrap().ether_mut().unwrap().send_now(t0, SERVER, words.clone());
        t0
    };
    if c.at >= 0 {
        let at = t0 + c.at as u64 - lead;
        assert!(b.now <= at, "{c:?}: the clear comes after the frame's first edge was found");
        b.run(at);
        m.run(b.now);
        clear_both(&mut b, &mut m, c.clear, keep);
        after = Some(pair(&mut b, &mut m, chaos::CSR));
    }
    let (after_b, after_m) = after.unwrap();
    let (done_b, done_m) = if c.loop_back {
        let ((tb, _), (tm, _)) = wait_for(&mut b, &mut m, csr::TRANSMIT_DONE, 1_000_000);
        (Some(tb), Some(tm))
    } else {
        (None, None)
    };
    // The frame out, the busy receiver's driver watched on both.
    let until = t0 + c.length() + 30_000;
    let mut was = [false, false];
    let mut edges = [(None, None), (None, None)];
    while b.now < until {
        let next = b.chip.next_tap().map_or(b.now + 25, |t| t.max(b.now + 1)).min(b.now + 25);
        b.run(next);
        m.run(b.now);
        let now = [
            b.chaos.as_ref().unwrap().board_tx(&b.chip),
            m.io.chaos.as_ref().unwrap().ether().unwrap().board_driving(),
        ];
        for k in 0..2 {
            if now[k] != was[k] {
                if now[k] {
                    edges[k].0.get_or_insert(b.now);
                } else if edges[k].0.is_some() {
                    edges[k].1.get_or_insert(b.now);
                }
                was[k] = now[k];
            }
        }
    }
    let abort = |e: (Option<u64>, Option<u64>)| {
        e.0.map(|on| ((on - t0) / wire::CELL_NS, (e.1.unwrap_or(on) - on) / wire::CELL_NS))
    };
    let (over_b, over_m) = pair(&mut b, &mut m, chaos::CSR);
    let (bits_b, bits_m) = pair(&mut b, &mut m, chaos::BIT_COUNT);
    // With Receive Done down nothing is read: what the board's read gives
    // then is not this test's.
    let read = |b: &mut UnibusMaster, m: &mut Model, c: u16, bits: u16, board: bool| {
        let n = if c & csr::RECEIVE_DONE != 0 { (bits as usize + 16) / 16 } else { 0 };
        (0..n)
            .map(|_| {
                if board {
                    b.cycle(chaos::READ_BUFFER, None).1
                } else {
                    m.cycle(chaos::READ_BUFFER, None, b.now)
                }
            })
            .collect::<Vec<u16>>()
    };
    let words_b = read(&mut b, &mut m, over_b, bits_b, true);
    let words_m = read(&mut b, &mut m, over_m, bits_m, false);
    (
        Cleared {
            after: after_b,
            done: done_b,
            over: over_b,
            bits: bits_b,
            words: words_b,
            abort: abort(edges[0]),
        },
        Cleared {
            after: after_m,
            done: done_m,
            over: over_m,
            bits: bits_m,
            words: words_m,
            abort: abort(edges[1]),
        },
    )
}

/// Runs every case, holds the board to [`Inside::rule`] and the model to
/// the board, and says how many differ.
fn clear_inside_all(cases: &[Inside]) {
    let (mut gates, mut wrong) = (Vec::new(), Vec::new());
    for c in cases {
        let (board, model) = clear_inside(c);
        let what = format!(
            "{} words to {:o}{}{}, {:?} at {} ns",
            c.words,
            c.dest,
            if c.spy { ", spy" } else { "" },
            if c.loop_back { ", loop back" } else { "" },
            c.clear,
            c.at
        );
        eprintln!("{what}: board {board:?}");
        // Looped back, the abort is this transmitter's own, and reads back
        // as Transmit Abort; the line driver stays off the cable.
        let seen = (
            board.over & csr::RECEIVE_DONE != 0,
            (board.over & csr::LOST_COUNT) >> 9,
            board.abort.is_some() || c.loop_back && board.over & csr::TRANSMIT_ABORT != 0,
        );
        if seen != c.rule() {
            gates.push(format!("{what}: the board {seen:?}, its gates {:?}", c.rule()));
        }
        if model != board {
            wrong.push(format!("{what}:\n  board {board:?}\n  model {model:?}"));
        }
    }
    for w in &wrong {
        eprintln!("UNLIKE {w}");
    }
    assert!(gates.is_empty(), "the board unlike its gates:\n{}", gates.join("\n"));
    assert!(
        wrong.is_empty(),
        "{} of {} cases unlike the board:\n{}",
        wrong.len(),
        cases.len(),
        wrong.join("\n")
    );
}

/// The instants of the sweep for a frame of `length` ns: before it, in
/// its check, source and destination words, past the destination word,
/// halfway, near its end, after its last edge before it lands, and after
/// it landed.
fn sweep(length: u64) -> Vec<i64> {
    let l = length as i64;
    vec![-5_000, 2_000, 6_000, 10_000, 15_000, l / 2, l - 3_000, l + 250, l + 20_000]
}

/// **A Clear Receiver inside a frame the receiver has taken drops the
/// rest of it, on both alike.** With the buffer free at the frame's
/// `START^`, `RACT`, the 74S74 at LMRCLK 0C06, comes up and the receiver
/// stores the frame's bits as they come. Clear Receiver is `-11.SAYS.GO`,
/// which through `-CLR RCV BUSY` at LMRCTL 0E10 makes `RRESET` (the 74S00
/// at 0E03): that clears `RACT` through the 74S02 at LMRCLK 0B08 and
/// `RBCT` with it, and resets `RCV BUSY`, the 74279 at LMRCTL 0D09, so
/// Receive Done, `RCV BUSY` and not `RACT`, never rises for the frame.
/// `START^` does not come again inside it, so the receiver stays off to
/// its end. Before the destination word the frame then finds `-RACT` up
/// where the busy receiver's gates look: Lost Count steps for
/// `DEST MATCH` --- its own address, zero, or anything under Spy --- and
/// a frame to its own address is aborted on the cable, the
/// `MATCH SO FAR` term of `-LOST.ONE` at LMMYNM 0D02. After it, nothing.
/// Swept over frames of 44 and 256 words with the source and check, to
/// its own address, to zero, and to another station under Spy, at nine
/// instants from before the frame to after it landed: every CSR, bit
/// count, word and abort signal on the model the board's.
#[test]
fn a_clear_receiver_inside_a_frame_taken_drops_it_on_both_alike() {
    let mut cases = Vec::new();
    for words in [42, 254] {
        for (dest, spy) in [(MY_ADDRESS, false), (0, false), (0o3070, true)] {
            let c = Inside { words, dest, spy, loop_back: false, clear: Clear::Receiver, at: 0 };
            for at in sweep(c.length()) {
                cases.push(Inside { at, ..c });
            }
        }
    }
    clear_inside_all(&cases);
}

/// **Reset and `-UB INIT` inside a frame the receiver has taken drop it
/// as Clear Receiver does, on both alike.** Both make `-INIT`, the 74S08
/// at LMUCON 0B10, which is the other input of `-CLR RCV BUSY` at LMRCTL
/// 0E10: the same `RRESET`. A frame of 44 words to its own address and to
/// zero, each cleared before its destination word, after it, and after
/// its last edge before it lands; and the board's own frame to its own
/// address looped back, before and after the destination word.
#[test]
fn a_reset_inside_a_frame_taken_drops_it_on_both_alike() {
    let mut cases = Vec::new();
    for clear in [Clear::Reset, Clear::Init] {
        for dest in [MY_ADDRESS, 0] {
            let c = Inside { words: 42, dest, spy: false, loop_back: false, clear, at: 0 };
            for at in [6_000, 15_000, c.length() as i64 + 250] {
                cases.push(Inside { at, ..c });
            }
        }
        let c = Inside { words: 42, dest: MY_ADDRESS, spy: false, loop_back: true, clear, at: 0 };
        for at in [6_000, 15_000] {
            cases.push(Inside { at, ..c });
        }
    }
    clear_inside_all(&cases);
}

/// A frame of `words` words from [`SERVER`] to `dest`, its words
/// marked apart from [`numbered`]'s and their low four bits zero --- so
/// that each bit more or less stored over a buffer never written, which
/// reads as ones, shows --- cleared `at` ns after its first edge;
/// then a frame of 12 numbered words lands over what it left, and is read
/// `reads` times on both. Returns what the board and the model read.
fn stored_then_read_round(words: usize, dest: u16, at: u64, reads: usize) -> (ReadOut, ReadOut) {
    let n = cadrio();
    let (mut b, mut m) = on_a_cable(&n);
    let mut first: Vec<u16> = (0..words - 1).map(|k| 0o040000 | (k as u16) << 4).collect();
    first.push(dest);
    let t0 = b.now + 20_000;
    b.run(t0);
    m.run(t0);
    b.chaos.as_mut().unwrap().ether.send_now(t0, SERVER, first.clone());
    m.io.chaos.as_mut().unwrap().ether_mut().unwrap().send_now(t0, SERVER, first);
    b.run(t0 + at - 125);
    m.run(b.now);
    write_at_strobe(&mut b, &mut m, csr::CLEAR_RECEIVER);
    let length = ((words as u64 + 2) * 16 + 1) * wire::CELL_NS;
    b.run(t0 + length + 30_000);
    m.run(b.now);
    let t1 = b.now;
    let second = numbered(12, MY_ADDRESS);
    b.chaos.as_mut().unwrap().ether.send_now(t1, SERVER, second.clone());
    m.io.chaos.as_mut().unwrap().ether_mut().unwrap().send_now(t1, SERVER, second);
    read_out(&mut b, &mut m, reads)
}

/// **The bits stored before a clear stay in the buffer, on both alike.**
/// `RRESET` clears `RBCT` and takes `RACT` down; it does not touch the
/// 2147 at LMRBUF 0C04, so what the receiver stored of the frame before
/// the clear is still there, below where the next frame's bits go.  A
/// frame of 44 and of 256 words with the source and check, cleared before
/// its destination word, halfway through --- once a little before the
/// cell's sample and once a little after it --- and after its last edge,
/// and swept at 20 ns steps across two cells of a frame of 44; and one for
/// another station, which the receiver takes from `START^` as any frame
/// and stores until it is cleared, its destination word gone by and not
/// matched; then a frame of 14 words lands from `RBCT` 0 and is read round the
/// buffer, 262 reads: past its own words, what the dropped frame stored
/// and, above it, what the buffer held before.  Every count, word and CSR
/// on the model the board's.
#[test]
fn the_bits_stored_before_a_clear_stay_in_the_buffer_on_both_alike() {
    let mut wrong = Vec::new();
    let mut cases: Vec<(usize, u16, u64)> = vec![
        (42, MY_ADDRESS, 6_125),
        (42, MY_ADDRESS, 176_500),
        (254, MY_ADDRESS, 700_125),
        (254, MY_ADDRESS, 700_225),
        (42, 0o3070, 100_125),
    ];
    cases.extend((100_000..100_500).step_by(20).map(|at| (42, MY_ADDRESS, at)));
    for &(words, dest, at) in &cases {
        let (board, model) = stored_then_read_round(words, dest, at, 262);
        let what = format!("{} words to {dest:o} cleared at {at} ns", words + 2);
        let octal = |v: &[u16]| v.iter().map(|w| format!("{w:o}")).collect::<Vec<_>>().join(" ");
        eprintln!("{what}: board landed {:#08o}, words {}", board.landed, octal(&board.words));
        assert_eq!(board.counts[0], 14 * 16 - 1, "{what}: the second frame landed whole");
        if let Some(how) = model.unlike(&board) {
            eprintln!("{what}: model words {}", octal(&model.words));
            wrong.push(format!("{what}: the model {how}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} unlike the board:\n{}",
        wrong.len(),
        cases.len(),
        wrong.join("\n")
    );
}

/// **A Reset leaves Lost Count where it was, on both alike.** Lost Count
/// is the 74LS161 at LMMYNM 0F04, whose clear, pin 1, is `-11.SAYS.GO`: a
/// CSR write with Clear Receiver, and nothing else (`mit/cadrio/iob.wlr`,
/// `-11.SAYS.GO` to `F04-01`). Reset's `-11.ZOT` and `-UB INIT` make
/// `-INIT`, the 74S08 at LMUCON 0B10, which goes to `-CLR RCV BUSY` and
/// Transmit Done and not to the count. Measured here: a frame lost to a
/// full buffer, then a Reset, then `-UB INIT`; Lost Count reads 1 after
/// each, and Clear Receiver alone takes it to 0.
#[test]
fn a_reset_leaves_lost_count_where_it_was_on_both_alike() {
    let n = cadrio();
    let mut b = board(&n);
    let mut m = Model::new(None);
    buffer_full(&mut b, &mut m, false, &[rfc_time(SERVER, MY_ADDRESS), rfc_time(SERVER, 0)]);
    b.run(b.now + 100_000);
    m.run(b.now);
    let lost = |c: u16| (c & csr::LOST_COUNT) >> 9;
    let c = both(&mut b, &mut m, chaos::CSR, None, "the CSR with one lost");
    assert_eq!(lost(c), 1, "a frame counted lost: {c:#08o}");
    let (mut board, mut model) = (Vec::new(), Vec::new());
    for clear in [Clear::Reset, Clear::Init, Clear::Receiver] {
        clear_both(&mut b, &mut m, clear, 0);
        b.run(b.now + 2_000);
        m.run(b.now);
        let (cb, cm) = pair(&mut b, &mut m, chaos::CSR);
        eprintln!(
            "after {clear:?}: Lost Count on the board {}, on the model {}",
            lost(cb),
            lost(cm)
        );
        board.push(lost(cb));
        model.push(lost(cm));
    }
    assert_eq!(board, [1, 1, 0], "the board's Lost Count after Reset, -UB INIT, Clear Receiver");
    assert_eq!(model, board, "the model's");
}

/// **A frame the receiver takes and does not land leaves its bits in the
/// buffer, on both alike.** `RACT` comes up at every frame's `START^` with
/// the buffer free, whatever the frame turns out to be for, and the
/// receiver writes every bit into the 2147 at LMRBUF 0C04 as it comes; the
/// destination word decides only whether `RCV BUSY` is set and the frame
/// lands. One that does not land is let go at its end --- `RRESET` comes
/// with `-CBLBSY` lifting and `RCV BUSY` down, and clears `RBCT` --- and
/// its bits stay below where the next frame goes. Measured with a frame of
/// 44 words for another station; then a frame of 14 words lands over it
/// and is read 262 times round the buffer. Every count, word and CSR on
/// the model the board's. A run of 48 bits or fewer, which does not land
/// either, lies wholly below the shortest frame that does, and is held by
/// the read of the buffer's first bit in
/// `a_read_with_receive_done_down_is_the_read_paths_on_both_alike`.
#[test]
fn a_frame_taken_and_not_landed_leaves_its_bits_on_both_alike() {
    let mut wrong = Vec::new();
    let marked = |n: usize, dest: u16| {
        let mut v: Vec<u16> = (0..n - 1).map(|k| 0o040000 | (k as u16) << 4).collect();
        v.push(dest);
        v
    };
    let cases = [("44 words for another station", marked(42, 0o3070))];
    for (what, first) in &cases {
        let n = cadrio();
        let (mut b, mut m) = on_a_cable(&n);
        let t0 = b.now + 20_000;
        b.run(t0);
        m.run(t0);
        b.chaos.as_mut().unwrap().ether.send_now(t0, SERVER, first.clone());
        m.io.chaos.as_mut().unwrap().ether_mut().unwrap().send_now(t0, SERVER, first.clone());
        b.run(t0 + 250_000);
        m.run(b.now);
        let (cb, cm) = pair(&mut b, &mut m, chaos::CSR);
        assert_eq!(cb & csr::RECEIVE_DONE, 0, "{what}: nothing landed on the board");
        let t1 = b.now;
        let second = numbered(12, MY_ADDRESS);
        b.chaos.as_mut().unwrap().ether.send_now(t1, SERVER, second.clone());
        m.io.chaos.as_mut().unwrap().ether_mut().unwrap().send_now(t1, SERVER, second);
        let (board, model) = read_out(&mut b, &mut m, 262);
        let octal = |v: &[u16]| v.iter().map(|w| format!("{w:o}")).collect::<Vec<_>>().join(" ");
        eprintln!(
            "{what}: board CSR {cb:06o}, model {cm:06o}; board words {}",
            octal(&board.words)
        );
        assert!(
            board.words[14..256].iter().any(|&w| w != 0o177777),
            "{what}: its bits on the board"
        );
        if cm != cb {
            wrong.push(format!("{what}: the CSR after it, board {cb:06o}, model {cm:06o}"));
        }
        if let Some(how) = model.unlike(&board) {
            wrong.push(format!("{what}: the model {how}"));
        }
    }
    assert!(wrong.is_empty(), "{} unlike the board:\n{}", wrong.len(), wrong.join("\n"));
}

/// Reads with Receive Done down, `n` of them, on both: each the word, the
/// bit count and the CSR after it.
fn reads_down(b: &mut UnibusMaster, m: &mut Model, n: usize, what: &str, wrong: &mut Vec<String>) {
    for k in 0..n {
        let word = exact(b, m, chaos::READ_BUFFER, None);
        let count = exact(b, m, chaos::BIT_COUNT, None);
        let c = exact(b, m, chaos::CSR, None);
        eprintln!(
            "{what}, read {k}: board {:06o} count {:o} CSR {:06o}; model {:06o} {:o} {:06o}",
            word.0, count.0, c.0, word.1, count.1, c.1
        );
        assert_eq!(c.0 & csr::RECEIVE_DONE, 0, "{what}: Receive Done down on the board");
        if (word.1, count.1, c.1) != (word.0, count.0, c.0) {
            wrong.push(format!(
                "{what}, read {k}: board {:06o} {:o} {:06o}, model {:06o} {:o} {:06o}",
                word.0, count.0, c.0, word.1, count.1, c.1
            ));
        }
    }
}

/// From the frame's first edge, sent on both cables from [`SERVER`].
fn send_both(b: &mut UnibusMaster, m: &mut Model, words: &[u16]) -> u64 {
    let t0 = b.now + 5_000;
    b.run(t0);
    m.run(t0);
    b.chaos.as_mut().unwrap().ether.send_now(t0, SERVER, words.to_vec());
    m.io.chaos.as_mut().unwrap().ether_mut().unwrap().send_now(t0, SERVER, words.to_vec());
    t0
}

/// **A read of the buffer with Receive Done down is the read path's, on
/// both alike.** The read register is the two 74164s at LMRBUF 0A07 and
/// 0B07, shifted on `ROCLK^` with the 2147's bit at `RBCT` coming in at the
/// bottom, `RBCT` counting down on each shift (the 25LS193s at 0D04-0D06),
/// until the count's low four bits borrow, `-ROWEND` on the 74279 at
/// LMRCTL 0D09 ending the read; Bit Count is `RBCT` itself, through the
/// 74LS244s at LMDATP 0C17. With the cable idle and no frame landed,
/// `RRESET` holds `RBCT` at 0, so each read shifts once: the word is the
/// last one read moved up a bit with the buffer's bit 0 below it, Bit Count
/// 0, and the check register held clear. Inside a frame the receiver has
/// been cleared off, `RRESET` is not held: `RBCT` counts down from 0 as it
/// does reading a frame out, the first read one bit and the rest sixteen,
/// and the check register runs on what is read. Measured: on a board that
/// has received nothing, after a frame read out and cleared, after one
/// cleared unread, inside and after a dropped frame, after a frame for
/// another station, and after runs of 48 bits to this address and to zero,
/// which do not land. Every word, count and CSR on the model the board's,
/// the model's cycle at the instant the board answers.
#[test]
fn a_read_with_receive_done_down_is_the_read_paths_on_both_alike() {
    let n = cadrio();
    let mut wrong = Vec::new();
    let (mut b, mut m) = on_a_cable(&n);
    reads_down(&mut b, &mut m, 4, "nothing received", &mut wrong);

    let (mut b, mut m) = on_a_cable(&n);
    send_both(&mut b, &mut m, &numbered(12, MY_ADDRESS));
    wait_for(&mut b, &mut m, csr::RECEIVE_DONE, 3_000_000);
    for k in 0..15 {
        both(&mut b, &mut m, chaos::READ_BUFFER, None, &format!("word {k}"));
    }
    write_at_strobe(&mut b, &mut m, csr::CLEAR_RECEIVER);
    reads_down(&mut b, &mut m, 5, "read out and cleared", &mut wrong);

    let (mut b, mut m) = on_a_cable(&n);
    let mut f: Vec<u16> = (0..20).map(|k| 0o125252 ^ k).collect();
    f.push(MY_ADDRESS);
    send_both(&mut b, &mut m, &f);
    wait_for(&mut b, &mut m, csr::RECEIVE_DONE, 3_000_000);
    write_at_strobe(&mut b, &mut m, csr::CLEAR_RECEIVER);
    reads_down(&mut b, &mut m, 5, "cleared unread", &mut wrong);

    let (mut b, mut m) = on_a_cable(&n);
    let t0 = send_both(&mut b, &mut m, &numbered(42, MY_ADDRESS));
    b.run(t0 + 30_000);
    m.run(b.now);
    write_at_strobe(&mut b, &mut m, csr::CLEAR_RECEIVER);
    reads_down(&mut b, &mut m, 4, "inside a dropped frame", &mut wrong);
    b.run(t0 + 200_000);
    m.run(b.now);
    reads_down(&mut b, &mut m, 4, "after it", &mut wrong);

    let (mut b, mut m) = on_a_cable(&n);
    f[20] = 0o3070;
    send_both(&mut b, &mut m, &f);
    b.run(b.now + 200_000);
    m.run(b.now);
    reads_down(&mut b, &mut m, 4, "after a frame for another station", &mut wrong);

    for dest in [MY_ADDRESS, 0] {
        let (mut b, mut m) = on_a_cable(&n);
        send_both(&mut b, &mut m, &[dest]);
        b.run(b.now + 50_000);
        m.run(b.now);
        reads_down(&mut b, &mut m, 3, &format!("after 48 bits to {dest:o}"), &mut wrong);
    }
    assert!(wrong.is_empty(), "{} reads unlike the board:\n{}", wrong.len(), wrong.join("\n"));
}

/// **Bit Count while a frame comes in, on both alike**: `RBCT` counting
/// up the bits stored, a step on each `RICLK^`, 60 ns after a cell's
/// opening edge. Polled every 5 µs through a frame of 44 words, the
/// model's read at the instant the board answers; the CSR too, CRC Error
/// aside.
#[test]
fn the_bit_count_while_a_frame_comes_in_on_both_alike() {
    let n = cadrio();
    let (mut b, mut m) = on_a_cable(&n);
    let t0 = send_both(&mut b, &mut m, &numbered(42, MY_ADDRESS));
    let mut wrong = Vec::new();
    while b.now < t0 + 200_000 {
        b.run(b.now + 5_000);
        m.run(b.now);
        let (nb, nm) = exact(&mut b, &mut m, chaos::BIT_COUNT, None);
        // CRC Error aside: the board's is the check register as it runs,
        // which a good frame brings to 0 with its last bit.
        let (cb, cm) = exact(&mut b, &mut m, chaos::CSR, None);
        let (cb, cm) = (cb & !csr::CRC_ERROR, cm & !csr::CRC_ERROR);
        if (nb, cb) != (nm, cm) {
            wrong.push(format!("+{}: board {nb:o} {cb:06o}, model {nm:o} {cm:06o}", b.now - t0));
        }
    }
    assert!(wrong.is_empty(), "{} polls unlike the board:\n{}", wrong.len(), wrong.join("\n"));
}

/// **A frame that finds the buffer full at `START^` is not taken, on the
/// cable and looped back alike, on both.** `RACT` takes `-RDONE` at
/// `START^`: with a packet in the buffer it stays down for the frame, the
/// check register is not clocked, and CRC Error reads the packet's, not
/// the cable's business. Looped back the frame reaches the same gates: at
/// its destination word Lost Count steps for `DEST MATCH`, and a frame to
/// the board's own address presets its own transmitter's `ABORT` through
/// `-LOST.ONE`, so it reads Transmit Abort with Transmit Done early.
/// Polled every 5 µs from START for 600 µs: every CSR on the model the
/// board's, then the bit count of the packet still in the buffer; and the
/// next frame after the abort lands at the board's poll.
#[test]
fn a_frame_at_a_full_buffer_is_not_taken_looped_back_on_both_alike() {
    let n = cadrio();
    let mut wrong = Vec::new();
    for (dest, spy) in [(MY_ADDRESS, false), (0, false), (0o3070, true), (0o3070, false)] {
        let what = format!("to {dest:o}{}", if spy { ", spy" } else { "" });
        let (mut b, mut m) = on_a_cable(&n);
        let keep = csr::LOOP_BACK | if spy { csr::SPY } else { 0 };
        both(&mut b, &mut m, chaos::CSR, Some(csr::CLEAR_RECEIVER | keep), "loop back");
        for &w in &numbered(12, MY_ADDRESS) {
            both(&mut b, &mut m, chaos::WRITE_BUFFER, Some(w), "a word in");
        }
        both(&mut b, &mut m, chaos::START, None, "START");
        wait_for(&mut b, &mut m, csr::RECEIVE_DONE, 3_000_000);
        for &w in &numbered(42, dest) {
            both(&mut b, &mut m, chaos::WRITE_BUFFER, Some(w), "a word in");
        }
        both(&mut b, &mut m, chaos::START, None, "START");
        let t = b.now;
        let mut seen = Vec::new();
        while b.now < t + 600_000 {
            b.run(b.now + 5_000);
            m.run(b.now);
            let (cb, cm) = pair(&mut b, &mut m, chaos::CSR);
            if seen.last().is_none_or(|&(_, x, y)| (x, y) != (cb, cm)) {
                seen.push((b.now - t, cb, cm));
            }
            if cb != cm {
                wrong.push(format!("{what}: +{}: board {cb:06o}, model {cm:06o}", b.now - t));
            }
        }
        let seen: Vec<String> =
            seen.iter().map(|(t, cb, cm)| format!("+{t} {cb:06o}/{cm:06o}")).collect();
        eprintln!("{what}: board/model {}", seen.join(", "));
        let (last, _) = pair(&mut b, &mut m, chaos::CSR);
        assert_eq!(
            ((last & csr::LOST_COUNT) >> 9, last & csr::TRANSMIT_ABORT != 0),
            ((dest == MY_ADDRESS || dest == 0 || spy) as u16, dest == MY_ADDRESS),
            "{what}: the board's Lost Count and Transmit Abort"
        );
        let (nb, nm) = pair(&mut b, &mut m, chaos::BIT_COUNT);
        assert_eq!(nb, 14 * 16 - 1, "{what}: the first packet still in the board's buffer");
        if nb != nm {
            wrong.push(format!("{what}: bit count board {nb:o}, model {nm:o}"));
        }
        if dest == MY_ADDRESS {
            write_at_strobe(&mut b, &mut m, csr::CLEAR_RECEIVER | keep);
            for &w in &numbered(12, MY_ADDRESS) {
                both(&mut b, &mut m, chaos::WRITE_BUFFER, Some(w), "a word in");
            }
            both(&mut b, &mut m, chaos::START, None, "START");
            let ((tb, cb), (tm, cm)) = wait_for(&mut b, &mut m, csr::RECEIVE_DONE, 3_000_000);
            if (tb, cb) != (tm, cm) {
                wrong.push(format!(
                    "{what}: the next frame, board {tb} {cb:06o}, model {tm} {cm:06o}"
                ));
            }
        }
    }
    // And on the cable: a frame for another station at a full buffer.
    let mut b = board(&n);
    let mut m = Model::new(None);
    buffer_full(&mut b, &mut m, false, &[rfc_time(SERVER, MY_ADDRESS), numbered(42, 0o3070)]);
    let t = b.now;
    while b.now < t + 400_000 {
        b.run(b.now + 5_000);
        m.run(b.now);
        let (cb, cm) = pair(&mut b, &mut m, chaos::CSR);
        if cb != cm {
            wrong.push(format!("on the cable: +{}: board {cb:06o}, model {cm:06o}", b.now - t));
        }
    }
    assert!(wrong.is_empty(), "{} unlike the board:\n{}", wrong.len(), wrong.join("\n"));
}

/// **Under Loop Back a Clear Receiver inside the board's own frame drops
/// it the same, on both alike.** The receiver takes the transmitter's
/// bits instead of the cable's, through the same receive control. A frame
/// of 44 words to its own address, to zero and to another station under
/// Spy, cleared before the destination word, after it, halfway and after
/// its last edge.
#[test]
fn a_clear_receiver_inside_a_frame_looped_back_drops_it_on_both_alike() {
    let mut cases = Vec::new();
    for (dest, spy) in [(MY_ADDRESS, false), (0, false), (0o3070, true)] {
        let c = Inside { words: 42, dest, spy, loop_back: true, clear: Clear::Receiver, at: 0 };
        let l = c.length() as i64;
        for at in [6_000, 15_000, l / 2, l + 250] {
            cases.push(Inside { at, ..c });
        }
    }
    clear_inside_all(&cases);
}

// --- The board's own frame, back off the cable ----------------------------

/// One read on both, not compared: the board's word and the model's.
fn pair(b: &mut UnibusMaster, m: &mut Model, uaddr: u32) -> (u16, u16) {
    let (_, wb) = b.cycle(uaddr, None);
    (wb, m.cycle(uaddr, None, b.now))
}

/// What one side read: the CSR and, with Receive Done up, the words.
type Seen = (u16, Vec<u16>);

/// The board's own frame to `dest` on a cable of its own, on both: reset,
/// Clear Receiver with Spy or not, and, when `full`, a first frame to its
/// own address first; then the frame to `dest`. Returns what the board
/// and the model read: the CSR and, with Receive Done up, the words in
/// the buffer.
fn own_frame(dest: u16, spy: bool, full: bool) -> (Seen, Seen) {
    let n = cadrio();
    let mut b = board(&n);
    let cable = || {
        let mut e = Ether::new();
        e.keep_log(true);
        e.attach(Box::new(Capture::new(SERVER)));
        e
    };
    b.plug_chaos(cable());
    let mut m = Model::new(Some(cable()));
    both(&mut b, &mut m, chaos::CSR, Some(csr::RESET), "reset");
    b.run(b.now + 2_000);
    m.run(b.now);
    let spying = if spy { csr::SPY } else { 0 };
    both(&mut b, &mut m, chaos::CSR, Some(csr::CLEAR_RECEIVER | spying), "clear receiver");
    let send = |b: &mut UnibusMaster, m: &mut Model, to: u16| {
        for &w in &rfc_time(MY_ADDRESS, to) {
            both(b, m, chaos::WRITE_BUFFER, Some(w), "a word in");
        }
        both(b, m, chaos::START, None, "START");
        let ((tb, _), (tm, _)) = wait_for(b, m, csr::TRANSMIT_DONE, 3_000_000);
        assert_eq!(tm, tb, "to {to:o}: Transmit Done at the same poll on both");
        // Past the frame's end, `-CBLBSY` and `RDONE` on both.
        b.run(b.now + 10_000);
        m.run(b.now);
    };
    if full {
        send(&mut b, &mut m, MY_ADDRESS);
    }
    send(&mut b, &mut m, dest);
    let (cb, cm) = pair(&mut b, &mut m, chaos::CSR);
    let (bits_b, bits_m) = pair(&mut b, &mut m, chaos::BIT_COUNT);
    let words = |b: &mut UnibusMaster, m: &mut Model, c: u16, bits: u16, board: bool| -> Vec<u16> {
        if c & csr::RECEIVE_DONE == 0 {
            return Vec::new();
        }
        (0..(bits as usize + 1) / 16)
            .map(|_| {
                if board {
                    b.cycle(chaos::READ_BUFFER, None).1
                } else {
                    m.cycle(chaos::READ_BUFFER, None, b.now)
                }
            })
            .collect()
    };
    let wb = words(&mut b, &mut m, cb, bits_b, true);
    let wm = words(&mut b, &mut m, cm, bits_m, false);
    ((cb, wb), (cm, wm))
}

/// **The board takes its own frame off the cable when it is for it, on
/// both alike.** Its transceiver hears the whole cable, its own frame
/// included, and its receiver judges that frame as any other:
/// `DEST MATCH` --- its own address, zero, or Spy --- takes it into a free
/// buffer, and another address is let go. MIT's `chatst.lisp` relies on
/// it: its cable test sends to its own address and waits for Receive
/// Done. With the buffer full the frame is counted in Lost Count, and one
/// to its own address is aborted by its own busy receiver, which reads
/// back as Transmit Abort. Measured on the board here, each case held to
/// what the board does, and the model to the board.
#[test]
fn the_board_takes_its_own_frame_off_the_cable_on_both_alike() {
    let other = 0o3070;
    let mut wrong = Vec::new();
    for (dest, spy, full, done, lost, abort) in [
        (MY_ADDRESS, false, false, true, 0, false),
        (0, false, false, true, 0, false),
        (other, true, false, true, 0, false),
        (other, false, false, false, 0, false),
        (MY_ADDRESS, false, true, true, 1, true),
        (0, false, true, true, 1, false),
    ] {
        let ((cb, wb), (cm, wm)) = own_frame(dest, spy, full);
        let what = format!("to {dest:o}, spy {spy}, full {full}");
        eprintln!("{what}: board CSR {cb:#08o}, model {cm:#08o}");
        let bits = |c: u16| {
            (c & csr::RECEIVE_DONE != 0, (c & csr::LOST_COUNT) >> 9, c & csr::TRANSMIT_ABORT != 0)
        };
        assert_eq!(bits(cb), (done, lost, abort), "{what}: the board, as measured");
        if done {
            // The buffer holds the frame taken first: with the buffer
            // full, the first one, to this address.
            let mut sent = rfc_time(MY_ADDRESS, if full { MY_ADDRESS } else { dest });
            sent.push(MY_ADDRESS);
            assert_eq!(wb[..sent.len()], sent[..], "{what}: the board's words back");
        }
        if (cm, &wm) != (cb, &wb) {
            wrong.push(format!("{what}: the board {cb:#08o} {wb:?}, the model {cm:#08o} {wm:?}"));
        }
    }
    assert!(wrong.is_empty(), "{} of 6 cases unlike the board:\n{}", wrong.len(), wrong.join("\n"));
}

// --- The buffers: 4K by 1, round and round -------------------------------

/// What one side read of a frame in its buffer: the CSR as it landed, and
/// for each read the bit count before it, the word, and the CSR after it.
#[derive(Debug, Default, PartialEq)]
struct ReadOut {
    landed: u16,
    counts: Vec<u16>,
    words: Vec<u16>,
    csrs: Vec<u16>,
}

impl ReadOut {
    /// The first read at which two read-outs differ, and what each saw.
    fn unlike(&self, other: &ReadOut) -> Option<String> {
        let at = |r: &ReadOut, k: usize| (r.counts[k], r.words[k], r.csrs[k]);
        if self.landed != other.landed {
            return Some(format!("landed {:#08o} against {:#08o}", self.landed, other.landed));
        }
        (0..self.words.len())
            .find(|&k| at(self, k) != at(other, k))
            .map(|k| format!("read {k}: {:?} against {:?}", at(self, k), at(other, k)))
    }
}

/// A frame of `n` words, dest last: the rest numbered, so that each word
/// says where it was.
fn numbered(n: usize, dest: u16) -> Vec<u16> {
    let mut v: Vec<u16> = (0..n - 1).map(|k| 0o100000 | k as u16).collect();
    v.push(dest);
    v
}

/// Both interfaces reset, receivers cleared, a cable of their own each.
fn on_a_cable(n: &Netlist) -> (UnibusMaster<'_>, Model) {
    let mut b = board(n);
    let cable = || {
        let mut e = Ether::new();
        e.keep_log(true);
        e.attach(Box::new(Capture::new(SERVER)));
        e
    };
    b.plug_chaos(cable());
    let mut m = Model::new(Some(cable()));
    both(&mut b, &mut m, chaos::CSR, Some(csr::RESET), "reset");
    b.run(b.now + 2_000);
    m.run(b.now);
    both(&mut b, &mut m, chaos::CSR, Some(csr::CLEAR_RECEIVER), "clear receiver");
    (b, m)
}

/// `earlier`, if given, from [`SERVER`] on each cable, landed and cleared
/// unread; then `first` from [`SERVER`] and, `over` cells into it, a
/// second frame put over it by a transmitter that heard neither cable.
/// Whatever lands is read `reads` times on both. Returns what the board and
/// the model read.
fn read_round(
    earlier: Option<&[u16]>,
    first: &[u16],
    over: Option<u64>,
    reads: usize,
) -> (ReadOut, ReadOut) {
    let n = cadrio();
    let (mut b, mut m) = on_a_cable(&n);
    if let Some(words) = earlier {
        let at = b.now;
        b.chaos.as_mut().unwrap().ether.send_now(at, SERVER, words.to_vec());
        m.io.chaos.as_mut().unwrap().ether_mut().unwrap().send_now(at, SERVER, words.to_vec());
        wait_for(&mut b, &mut m, csr::RECEIVE_DONE, 3_000_000);
        both(&mut b, &mut m, chaos::CSR, Some(csr::CLEAR_RECEIVER), "clear receiver, unread");
        b.run(b.now + 20_000);
        m.run(b.now);
    }
    let at = b.now;
    b.chaos.as_mut().unwrap().ether.send_now(at, SERVER, first.to_vec());
    m.io.chaos.as_mut().unwrap().ether_mut().unwrap().send_now(at, SERVER, first.to_vec());
    if let Some(cells) = over {
        b.run(at + cells * wire::CELL_NS);
        m.run(b.now);
        let other = rfc_time(0o3061, 0o3070);
        b.chaos.as_mut().unwrap().ether.send_now(b.now, 0o3061, other.clone());
        m.io.chaos.as_mut().unwrap().ether_mut().unwrap().send_now(b.now, 0o3061, other);
    }
    read_out(&mut b, &mut m, reads)
}

/// Waits on both for Receive Done and reads what landed `reads` times:
/// the bit count, the word and the CSR each time.
fn read_out(b: &mut UnibusMaster, m: &mut Model, reads: usize) -> (ReadOut, ReadOut) {
    let ((_, cb), (_, cm)) = wait_for(b, m, csr::RECEIVE_DONE, 3_000_000);
    let (mut rb, mut rm) = (ReadOut { landed: cb, ..ReadOut::default() }, ReadOut::default());
    rm.landed = cm;
    for _ in 0..reads {
        for (uaddr, of) in [(chaos::BIT_COUNT, 0), (chaos::READ_BUFFER, 1), (chaos::CSR, 2)] {
            let (wb, wm) = pair(b, m, uaddr);
            for (r, w) in [(&mut rb, wb), (&mut rm, wm)] {
                [&mut r.counts, &mut r.words, &mut r.csrs][of].push(w);
            }
        }
    }
    (rb, rm)
}

/// The board's CRC Error after each read is the check of the words read
/// so far: [`muir::chaos::packet::check_word`] over them not zero.
fn crc_is_the_check_of_the_words_read(r: &ReadOut, what: &str) {
    use muir::chaos::packet::check_word;
    for k in 0..r.words.len() {
        assert_eq!(
            r.csrs[k] & csr::CRC_ERROR != 0,
            check_word(&r.words[..=k]) != 0,
            "{what}: the board's CRC Error after read {k} is the check of the words read"
        );
    }
}

/// **A frame longer than the receive buffer keeps its last 4,096 bits, on
/// both alike.** The buffer is the 2147 at LMRBUF `0C04`, 4K by 1, on the
/// twelve bits of `RBCT`: the receiver stores every bit that comes, and
/// past the 4,096th the count wraps and the bits go over the first ones.
/// A frame comes check word first, so what is lost is what the software
/// reads last. Measured on the board here, with frames from another
/// station of 256, 257 and 258 words with the source and check, and a
/// 272-word frame with another put over it, wreckage whose top word is
/// partial:
///
/// - It lands as any frame does: Receive Done, CRC Error only for the
///   wreckage --- the check is made on the bits as they come --- and the
///   bit count the number of bits less one in twelve bits, so 15 for 257
///   words.
/// - The first 256 words read are the frame's first 256, the partial word
///   with zeros above its bits as for any wreckage; the words past them
///   are gone, the check word of a 257-word frame and the source and check
///   of a 258-word one. Reading on goes round the buffer: read 256 is the
///   first word's sixteen bits as the buffer holds them, and the count
///   keeps coming down sixteen a read from where the first read left it,
///   in twelve bits.
/// - CRC Error once reading starts is the check of the words read so far,
///   so a frame that lost its check word reads out with it set.
///
/// The model is held to every count, word and CSR.
#[test]
fn a_frame_longer_than_the_buffer_keeps_its_last_4096_bits_on_both_alike() {
    use muir::chaos::packet::check_word;
    let reads = 262;
    let mut wrong = Vec::new();
    for (buffer, over, landed, count) in [
        (254, None, csr::RECEIVE_DONE | csr::TRANSMIT_DONE, 0o7777),
        (255, None, csr::RECEIVE_DONE | csr::TRANSMIT_DONE, 0o17),
        (256, None, csr::RECEIVE_DONE | csr::TRANSMIT_DONE, 0o37),
        (270, Some(4_105), csr::RECEIVE_DONE | csr::TRANSMIT_DONE | csr::CRC_ERROR, 0o301),
    ] {
        let sent = numbered(buffer, MY_ADDRESS);
        let (board, model) = read_round(None, &sent, over, reads);
        // The words as the software would read them back whole: the
        // buffer, the source, the check word.
        let mut whole = sent.clone();
        whole.push(SERVER);
        whole.push(check_word(&whole));
        let what =
            format!("{} words{}", buffer + 2, if over.is_some() { " under wreckage" } else { "" });
        eprintln!("{what}: board landed {:#08o} count {:#o}", board.landed, board.counts[0]);
        assert_eq!(
            (board.landed, board.counts[0]),
            (landed, count),
            "{what}: the board, as measured"
        );
        // A whole frame: its first 256 words, and then round again. What
        // the wreckage's words are is the bits that came, which the model
        // is held to below.
        if over.is_none() {
            assert_eq!(board.words[..256], whole[..256], "{what}: the board's first 256 words");
            assert_eq!(board.words[256..258], board.words[..2], "{what}: reading goes round");
        }
        crc_is_the_check_of_the_words_read(&board, &what);
        if let Some(how) = model.unlike(&board) {
            wrong.push(format!("{what}: the model {how}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of 4 frames unlike the board:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// **Reading on past a frame that fits goes round the buffer, and CRC
/// Error is the check of the words read, on both alike.** Nothing on the
/// board stops a read at the frame's end: `RBCT` keeps coming down sixteen
/// a read in twelve bits, past `7777`, and the words are what the buffer
/// holds there --- whatever an earlier frame left, or, never written, what
/// the netlist's RAM holds at power-up, which reads as all ones. The check
/// run over the read-out is the same as for a frame that filled the
/// buffer: CRC Error from the first read on is the check of the words read
/// so far, so it is up partway through a good frame and down once the
/// frame is read out, the one time AIM-628 §7 gives it a meaning there.
/// Measured on the board with a 255-word frame from another station read
/// 262 times, and with wreckage of 16 words whose top word is partial,
/// landed over an earlier frame of ones left unread: the first read takes
/// the partial word with zeros above its bits even though the buffer holds
/// the earlier frame's ones there, and reading on gives the earlier frame's
/// words. The model is held to every count, word and CSR.
#[test]
fn reading_on_goes_round_the_buffer_on_both_alike() {
    let mut ones = vec![0o177777u16; 252];
    ones.push(MY_ADDRESS);
    let mut wrong = Vec::new();
    for (what, earlier, first, over, reads, landed, count) in [
        (
            "255 words",
            None,
            numbered(253, MY_ADDRESS),
            None,
            262,
            csr::RECEIVE_DONE | csr::TRANSMIT_DONE,
            0o7757,
        ),
        (
            "wreckage over an earlier frame",
            Some(&ones[..]),
            numbered(12, MY_ADDRESS),
            Some(64),
            300,
            csr::RECEIVE_DONE | csr::TRANSMIT_DONE | csr::CRC_ERROR,
            0o333,
        ),
    ] {
        let (board, model) = read_round(earlier, &first, over, reads);
        eprintln!("{what}: board landed {:#08o} count {:#o}", board.landed, board.counts[0]);
        assert_eq!(
            (board.landed, board.counts[0]),
            (landed, count),
            "{what}: the board, as measured"
        );
        crc_is_the_check_of_the_words_read(&board, what);
        let n = (board.counts[0] as usize + 16) / 16;
        assert_eq!(board.counts[n], 0o7777, "{what}: read out");
        assert_eq!(board.counts[n + 1], 0o7757, "{what}: and the count comes on down");
        if over.is_none() {
            assert_eq!(board.csrs[n - 2] & csr::CRC_ERROR, csr::CRC_ERROR, "{what}: CRC partway");
            assert_eq!(board.csrs[n - 1] & csr::CRC_ERROR, 0, "{what}: and not once read out");
            assert_eq!(board.words[n], 0o177777, "{what}: what the netlist's RAM held");
        } else {
            assert!(board.words[0] < 0o20000, "{what}: zeros above the partial word's bits");
            assert_eq!(board.words[n], 0o177777, "{what}: the earlier frame's ones");
        }
        if let Some(how) = model.unlike(&board) {
            wrong.push(format!("{what}: the model {how}"));
        }
    }
    assert!(wrong.is_empty(), "{} of 2 unlike the board:\n{}", wrong.len(), wrong.join("\n"));
}

/// What one side did with its own frame: Transmit Done's poll, the CSR
/// then, the bit count and the words in the buffer if it landed, and the
/// frames its cable carried (words, source, check, whether good).
type OwnSeen = (u64, u16, u16, Vec<u16>, Vec<(Vec<u16>, u16, u16, bool)>);

/// The board's own frame of `n` words, `numbered` and to its own address,
/// sent on both, on the cable or under Loop Back.
fn own_written(n: usize, loop_back: bool) -> (OwnSeen, OwnSeen) {
    let nl = cadrio();
    let (mut b, mut m) = on_a_cable(&nl);
    if loop_back {
        both(&mut b, &mut m, chaos::CSR, Some(csr::LOOP_BACK | csr::CLEAR_RECEIVER), "loop back");
    }
    for &w in &numbered(n, MY_ADDRESS) {
        both(&mut b, &mut m, chaos::WRITE_BUFFER, Some(w), "a word in");
    }
    both(&mut b, &mut m, chaos::START, None, "START");
    let ((tb, _), (tm, _)) = wait_for(&mut b, &mut m, csr::TRANSMIT_DONE, 3_000_000);
    b.run(b.now + 10_000);
    m.run(b.now);
    let (cb, cm) = pair(&mut b, &mut m, chaos::CSR);
    let (nb, nm) = pair(&mut b, &mut m, chaos::BIT_COUNT);
    let words = |b: &mut UnibusMaster, m: &mut Model, c: u16, bits: u16, board: bool| -> Vec<u16> {
        if c & csr::RECEIVE_DONE == 0 {
            return Vec::new();
        }
        (0..(bits as usize + 16) / 16)
            .map(|_| {
                if board {
                    b.cycle(chaos::READ_BUFFER, None).1
                } else {
                    m.cycle(chaos::READ_BUFFER, None, b.now)
                }
            })
            .collect()
    };
    let wb = words(&mut b, &mut m, cb, nb, true);
    let wm = words(&mut b, &mut m, cm, nm, false);
    let heard = |e: &Ether| {
        e.log
            .iter()
            .filter_map(|ev| match ev {
                Event::Heard(_, f) => Some((f.buffer.clone(), f.source, f.check, f.check_ok)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let hb = heard(&b.chaos.as_ref().unwrap().ether);
    let hm = heard(m.io.chaos.as_ref().unwrap().ether().unwrap());
    ((tb, cb, nb, wb, hb), (tm, cm, nm, wm, hm))
}

/// **A frame written past the transmit buffer wraps, on both alike.** The
/// outgoing buffer is the 2147 at LMTBUF `0C10`, 4K by 1, on `TBCT<11:0>`
/// off the 25LS193s at `0C11`-`0C13`: each word written goes in at the
/// count and takes it on sixteen, START shifts the source and check words
/// in behind them, and the twelve-bit count wraps, so a word past the
/// 256th goes over the first ones. The check is made on every word shifted
/// in, the source last. The frame on the line is the count's bits, played
/// down from it: `(16n + 32) mod 4096` for `n` words written. Measured on
/// the board here, its own frames to its own address on the cable and
/// under Loop Back:
///
/// - 253 words: 4,080 bits, the longest a board sends, landing whole.
/// - 254: nothing. The count is 0; `TSTART` comes at the turn, `TBUSY`
///   93 ns later and Transmit Done 125 ns after that, and nothing is on
///   the line.
/// - 255 to 258: 16 to 64 bits, the check word and what the wrap left
///   below it. Only the 64 bits of 258 land, as wreckage.
///
/// Transmit Done at the same poll on both, the same CSR, the same words
/// back, and the same frames on each cable.
#[test]
fn a_frame_written_past_the_buffer_wraps_on_both_alike() {
    let mut wrong = Vec::new();
    for loop_back in [false, true] {
        for (n, bits) in [
            (253, Some(4_080)),
            (254, None),
            (255, None),
            (256, None),
            (257, None),
            (258, Some(64)),
        ] {
            let (board, model) = own_written(n, loop_back);
            let what = format!("{n} words written{}", if loop_back { ", loop back" } else { "" });
            eprintln!(
                "{what}: board {board:?}",
                board = (board.0, board.1, board.2, board.4.len())
            );
            let landed = board.1 & csr::RECEIVE_DONE != 0;
            assert_eq!(
                landed.then_some(board.2 as usize + 1),
                bits,
                "{what}: what landed on the board"
            );
            if !loop_back {
                // A frame of one word reads as a check word alone.
                let words = |f: &(Vec<u16>, u16, u16, bool)| {
                    if f.0.is_empty() && f.1 == 0 { 1 } else { f.0.len() + 2 }
                };
                let on_line = board.4.iter().map(words).sum::<usize>();
                let want = (16 * n + 32) % 4096 / 16;
                assert_eq!(on_line, want, "{what}: the board's frame on the line, in words");
            }
            if model != board {
                wrong.push(format!("{what}: the board {board:?}\n  the model {model:?}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{} of 12 unlike the board:\n{}", wrong.len(), wrong.join("\n"));
}

// --- The transmitter's shift register and the turn ----------------------

/// **An empty START near a rise of `MY.TURN^` makes the turn the board
/// makes, on both alike.** The board's `TSREMPTY` comes up on its 8 MHz
/// clock, at the first edge at or after the read's answer plus 4,101 ns, so
/// how long after the answer depends on the clock's phase there, 4,101 to
/// 4,225 ns; the frame goes at a rise of bit 7 only if `TSREMPTY` is up by
/// then. Answers swept across the 150 ns before a rise at 10 ns steps,
/// looped back: the board's frame makes the rise for an answer 4,101 ns
/// or more before it and waits a round for one later, and the model's
/// Transmit Done comes at the same poll as the board's at every step.
#[test]
fn an_empty_start_makes_the_turn_the_board_makes_on_both_alike() {
    // `MY.TURN^`'s second rise after power-on with the cable idle,
    // measured on the board below.
    let rise = 260_250u64;
    let mut wrong = Vec::new();
    let mut made = Vec::new();
    for before in (4_050..=4_200u64).rev().step_by(10) {
        let n = cadrio();
        let mut b = board(&n);
        let mut m = Model::new(None);
        both(&mut b, &mut m, chaos::CSR, Some(csr::RESET), "reset");
        b.run(b.now + 2_000);
        m.run(b.now);
        both(&mut b, &mut m, chaos::CSR, Some(csr::LOOP_BACK | csr::CLEAR_RECEIVER), "loop back");
        // The read's answer at `rise - before`: `-MSYN` the address time
        // after the cycle begins, and an empty START answered 350 ns on.
        let aim = rise - before - muir::busint::UNIBUS_ADDRESS_NS - 350;
        b.run(aim);
        m.run(aim);
        let begin = b.now;
        let (took, _) = b.cycle(chaos::START, None);
        let answered = begin + muir::busint::UNIBUS_ADDRESS_NS + took;
        assert_eq!(answered, rise - before, "the START answered where aimed");
        m.cycle(chaos::START, None, answered);
        // The board's own record of the turn: `MY.TURN^` rising at `rise`.
        let mut was = b.level("MY.TURN^");
        let mut rose = None;
        while b.now < rise + 1_000 {
            let next = b.chip.next_tap().map_or(b.now + 25, |t| t.max(b.now + 1)).min(b.now + 25);
            b.run(next);
            let l = b.level("MY.TURN^");
            if l == Level::High && was != Level::High {
                rose.get_or_insert(b.now);
            }
            was = l;
        }
        m.run(b.now);
        assert_eq!(rose, Some(rise), "MY.TURN^ rises where the sweep is aimed");
        let ((tb, _), (tm, _)) = wait_for(&mut b, &mut m, csr::TRANSMIT_DONE, 1_000_000);
        // A frame that made the rise is done within 20 us of it.
        if tb < 20_000 {
            made.push(before);
        }
        eprintln!(
            "START answered {before} ns before the rise: board's Transmit Done after {tb} ns, model's after {tm} ns"
        );
        if tm != tb {
            wrong.push(format!("{before} ns before: board {tb} ns, model {tm} ns"));
        }
    }
    assert_eq!(
        made,
        (4_110..=4_200).rev().step_by(10).collect::<Vec<u64>>(),
        "the board makes the rise from 4,101 ns before it"
    );
    assert!(
        wrong.is_empty(),
        "{} of 16 answers unlike the board:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}
