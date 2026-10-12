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
