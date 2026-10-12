// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The ether under interference. AIM-628 §2.3: the transceiver "detects
//! interference (another transceiver transmitting at the same time as
//! this one) and informs the interface". Frames due together start
//! together; a frame put on a busy cable by a transmitter that has not
//! heard it --- the board at its turn timer's instant, a transmitter with
//! cable between it and the rest --- overlaps what is there; two driving
//! high at once is interference, a transmitter whose clock edge finds it
//! aborts and holds the cable high for [`ABORT_HOLD_NS`], and what was
//! left on the cable reaches every receiver as wreckage, failing its check.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use muir::chaos::ether::{
    ABORT_HOLD_NS, ABORT_NS, BUSY_ABORT_NS, Ether, Event, Node, SLOT_NS, refill, turn_byte,
};
use muir::chaos::packet::Framed;
use muir::chaos::wire;

/// What happened to a host, in time.
#[derive(Debug, PartialEq, Eq)]
enum Happened {
    /// A frame heard: from whom, and whether its check word was good.
    Heard(u64, u16, bool),
    /// A frame handed back, aborted.
    Aborted(u64, Vec<u16>),
}

/// A host on the cable: sends what it is given, hears everything, and
/// offers an aborted frame again if told to, as an interface's driver
/// retries on Transmit Abort.
struct Host {
    address: u16,
    to_send: VecDeque<Vec<u16>>,
    retry: bool,
    log: Arc<Mutex<Vec<Happened>>>,
}

impl Node for Host {
    fn address(&self) -> u16 {
        self.address
    }
    fn receive(&mut self, now: u64, packet: &Framed) {
        self.log.lock().unwrap().push(Happened::Heard(now, packet.source, packet.check_ok));
    }
    fn transmit(&mut self, _now: u64) -> Option<Vec<u16>> {
        self.to_send.pop_front()
    }
    fn aborted(&mut self, now: u64, buffer: Vec<u16>) {
        self.log.lock().unwrap().push(Happened::Aborted(now, buffer.clone()));
        if self.retry {
            self.to_send.push_front(buffer);
        }
    }
}

fn host(
    address: u16,
    frames: Vec<Vec<u16>>,
    retry: bool,
) -> (Box<Host>, Arc<Mutex<Vec<Happened>>>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let h = Host { address, to_send: frames.into(), retry, log: log.clone() };
    (Box::new(h), log)
}

/// A packet buffer as the software writes it, `n` data words long: the
/// eight header words, the data, the cable destination last.
fn packet(from: u16, to: u16, n: usize) -> Vec<u16> {
    let mut w = vec![0o400, (2 * n) as u16, to, 0, from, 0o21, 1, 0];
    w.extend((0..n).map(|k| k as u16));
    w.push(to);
    w
}

/// Runs the ether from `now` to `until`, at every instant it has
/// something to do.
fn run(e: &mut Ether, mut now: u64, until: u64) {
    e.at(now);
    loop {
        match e.next_due() {
            Some(d) if d <= now => {
                now += 1;
                e.at(now);
            }
            Some(d) if d < until => {
                now = d;
                e.at(now);
            }
            _ => {
                e.at(until);
                return;
            }
        }
    }
}

fn sent(e: &Ether) -> Vec<(u64, u16)> {
    e.log
        .iter()
        .filter_map(|ev| if let Event::Sent(t, s, _) = ev { Some((*t, *s)) } else { None })
        .collect()
}

fn collisions(e: &Ether) -> Vec<u64> {
    e.log
        .iter()
        .filter_map(|ev| if let Event::Collision(t) = ev { Some(*t) } else { None })
        .collect()
}

fn heard(e: &Ether) -> Vec<(u64, u16, bool)> {
    e.log
        .iter()
        .filter_map(|ev| {
            if let Event::Heard(t, f) = ev { Some((*t, f.source, f.check_ok)) } else { None }
        })
        .collect()
}

/// **Two frames due together start together, and collide.** Two hosts
/// with nothing heard before them have the same turn --- one slot after
/// the cable is idle, for both --- so both start at that instant; their
/// first edges are high together, which is interference, and both stop
/// [`ABORT_NS`] on with their words handed back. Nothing came of it on
/// the cable: no frame decodes, and the cable is idle again after it.
#[test]
fn two_frames_due_together_start_together_and_collide() {
    let mut e = Ether::new();
    e.keep_log(true);
    let (a, log_a) = host(0o3060, vec![packet(0o3060, 0o3062, 4)], false);
    let (b, log_b) = host(0o3061, vec![packet(0o3061, 0o3062, 4)], false);
    let (c, log_c) = host(0o3062, vec![], false);
    e.attach(a);
    e.attach(b);
    e.attach(c);
    run(&mut e, 0, 100_000);
    let sent = sent(&e);
    eprintln!("sent: {sent:?}; collisions: {:?}", collisions(&e));
    assert_eq!(sent.len(), 2, "both went: {sent:?}");
    let t0 = sent[0].0;
    assert_eq!(sent[1].0, t0, "at the same instant");
    assert_eq!(collisions(&e), [t0], "and collided there: both first edges high");
    let stop = t0 + ABORT_NS;
    assert_eq!(
        log_a.lock().unwrap()[0],
        Happened::Aborted(stop, packet(0o3060, 0o3062, 4)),
        "3060 stopped and got its words back"
    );
    assert_eq!(
        log_b.lock().unwrap()[0],
        Happened::Aborted(stop, packet(0o3061, 0o3062, 4)),
        "and so did 3061"
    );
    // What was left on the cable reaches every receiver as wreckage, its
    // check bad, as a receiver takes it.
    let whole = |log: &[Happened]| log.iter().any(|h| matches!(h, Happened::Heard(_, _, true)));
    assert!(
        !whole(&log_c.lock().unwrap()),
        "3062 heard nothing whole: {:?}",
        log_c.lock().unwrap()
    );
    let h = heard(&e);
    assert!(h.len() == 1 && !h[0].2, "the wreckage, once, its check bad: {h:?}");
    assert!(!e.level(), "the cable is low");
    assert!(!e.busy(100_000), "and idle");
}

/// **A frame put on a busy cable collides, both stop, and the first is
/// offered again.** A host is a long way into a frame when a transmitter
/// that has not heard the cable starts another. Within a cell both are
/// high: interference, and the host's frame stops [`ABORT_NS`] on, its
/// words handed back; the wreckage decodes to nothing. The host offers
/// the frame again at its next turn, and this time it is heard whole.
#[test]
fn a_frame_put_on_a_busy_cable_collides_and_the_first_is_offered_again() {
    let mut e = Ether::new();
    e.keep_log(true);
    let long = packet(0o3060, 0o3062, 100);
    let (a, log_a) = host(0o3060, vec![long.clone()], true);
    let (c, log_c) = host(0o3062, vec![], false);
    e.attach(a);
    e.attach(c);
    run(&mut e, 0, 20_000);
    let [(s, 0o3060)] = sent(&e)[..] else { panic!("3060 went once: {:?}", sent(&e)) };
    assert!(e.busy(20_000), "a long frame is still going");

    let at = s + 50_000;
    run(&mut e, 20_000, at);
    e.send_now(at, 0o3061, packet(0o3061, 0o3062, 4));
    run(&mut e, at, at + 5_000);
    let sent_now = sent(&e);
    assert_eq!(sent_now.len(), 2, "the second went at once: {sent_now:?}");
    assert_eq!(sent_now[1], (at, 0o3061), "at its own instant, the cable busy");
    let [c] = collisions(&e)[..] else { panic!("one collision: {:?}", collisions(&e)) };
    assert!(
        (at..=at + wire::CELL_NS).contains(&c),
        "interference within a cell of the second's first edge: {c} for {at}"
    );
    assert_eq!(
        log_a.lock().unwrap()[..],
        [Happened::Aborted(c + ABORT_NS, long.clone())],
        "3060's frame stopped and came back"
    );
    // The transmitter that never heard the cable has no detector either:
    // its frame runs on alone, its head wreckage, and decodes to nothing;
    // 3060 offers its frame again at its turn once the cable is idle.
    let other_ends = at + 15 * 16 * wire::CELL_NS;
    run(&mut e, at + 5_000, other_ends);
    assert_eq!(sent(&e).len(), 2, "3060 waited for the cable: {:?}", sent(&e));
    run(&mut e, other_ends, at + 1_000_000);
    let again = sent(&e);
    assert_eq!(again.len(), 3, "3060 went again: {again:?}");
    assert_eq!(again[2].1, 0o3060);
    assert!(again[2].0 > other_ends, "after the other frame ran out: {again:?}");
    assert_eq!(collisions(&e).len(), 1, "with nothing in the way");
    let h = heard(&e);
    assert_eq!(h.len(), 2, "the wreckage was heard, then the frame: {h:?}");
    assert!(!h[0].2, "the wreckage, its check bad");
    assert_eq!((h[1].1, h[1].2), (0o3060, true), "the frame whole, its check word good");
    assert!(
        matches!(
            log_c.lock().unwrap()[..],
            [Happened::Heard(_, _, false), Happened::Heard(_, 0o3060, true)]
        ),
        "3062 took the wreckage as such, then the frame: {:?}",
        log_c.lock().unwrap()
    );
}

/// **The board's frame goes at its turn timer's instant, and the board is
/// told of the collision.** The behavioral board's turn timer decided at
/// its terminal count and its frame starts at the instant given, whether
/// or not a host has taken the cable since. Both stop; the board is told
/// when, and that its own frame was one of them, so its busy line and its
/// Transmit Abort can follow.
#[test]
fn the_boards_frame_goes_at_its_instant_and_the_board_is_told() {
    let mut e = Ether::new();
    e.keep_log(true);
    e.attach_board(0o3050);
    let (a, log_a) = host(0o3060, vec![packet(0o3060, 0o3050, 100)], false);
    e.attach(a);
    run(&mut e, 0, 20_000);
    let [(s, 0o3060)] = sent(&e)[..] else { panic!("3060 went once: {:?}", sent(&e)) };
    assert_eq!(
        e.board_cable_frame().map(|f| (f.0, f.1)),
        Some((s, 0o3060)),
        "the board heard it start"
    );

    let start = s + 50_000;
    e.board_send(packet(0o3050, 0o3060, 4), start);
    run(&mut e, 20_000, start + 5_000);
    let sent_now = sent(&e);
    assert_eq!(
        sent_now.get(1),
        Some(&(start, 0o3050)),
        "the board's frame went at its instant: {sent_now:?}"
    );
    let [c] = collisions(&e)[..] else { panic!("one collision: {:?}", collisions(&e)) };
    assert!(
        (start..=start + wire::CELL_NS).contains(&c),
        "interference within a cell: {c} for {start}"
    );
    let mut told = Vec::new();
    while let Some(x) = e.board_collision() {
        told.push(x);
    }
    told.sort();
    let (at, until) = (c + ABORT_NS, c + ABORT_NS + ABORT_HOLD_NS);
    assert_eq!(
        told,
        [(at, 0o3050, until), (at, 0o3060, until)],
        "both aborted, one of them the board's own, and hold the cable through their abort signal"
    );
    assert_eq!(e.board_sending_until(), None, "the board's frame is over");
    assert_eq!(
        e.board_cable_frame().map(|f| (f.0, f.1)),
        Some((start, 0o3050)),
        "and it started, for the turn timer"
    );
    assert!(matches!(log_a.lock().unwrap()[0], Happened::Aborted(t, _) if t == c + ABORT_NS));
    run(&mut e, start + 5_000, start + 200_000);
    let h = heard(&e);
    assert!(h.len() == 1 && !h[0].2, "the wreckage, its check bad: {h:?}");
    let (_, r, taken) = e.board_heard().expect("the board heard the wreckage too");
    assert!(taken, "with its buffer free, its receiver was active for it");
    assert!(!r.framed.check_ok, "and can judge it by its check, and by its {} bits", r.bits);
}

/// **Interference, for the netlist board, is its transceiver and a model
/// transmitter driving high at once.** With a host's frame on the cable,
/// the board's transceiver driving high while the host's is high is
/// interference and a collision, and the host stops [`ABORT_NS`] on; the
/// board driving high while the host's transmitter is low, between
/// cells, is not.
#[test]
fn interference_is_two_transceivers_driving_high_at_once() {
    let mut e = Ether::new();
    e.keep_log(true);
    let (a, log_a) = host(0o3060, vec![packet(0o3060, 0o3050, 100)], false);
    e.attach(a);
    run(&mut e, 0, 1_000);
    let [(s, 0o3060)] = sent(&e)[..] else { panic!("3060 went once: {:?}", sent(&e)) };
    assert!(e.level(), "a frame's first edge takes the line high");
    // The first cell's mid-cell transition, if there is one, is at 125 ns;
    // a cell whose bit differs from the last has none. Find an instant in
    // the first few cells with the line low.
    let mut t = s;
    while e.level() {
        t += 5;
        e.at(t);
        assert!(t < s + 4 * wire::CELL_NS, "a low within four cells");
    }
    assert!(e.board(t, true), "the board driving high while the host is low: the cable goes high");
    assert!(!e.interference(), "and it is no interference");
    assert!(collisions(&e).is_empty());
    e.board(t, false);
    // Then high together.
    while !e.level() {
        t += 5;
        e.at(t);
        assert!(t < s + 8 * wire::CELL_NS, "a high within eight cells");
    }
    e.board(t, true);
    assert!(e.interference(), "both high: interference");
    assert_eq!(collisions(&e), [t]);
    run(&mut e, t, t + ABORT_NS);
    let x = match log_a.lock().unwrap()[..] {
        [Happened::Aborted(x, _)] => x,
        ref other => panic!("the host aborted once: {other:?}"),
    };
    assert!(x > t && x <= t + ABORT_NS, "at its next clock edge: {x} for {t}");
    assert!(e.interference(), "and holds the cable high through its abort signal");
    run(&mut e, t + ABORT_NS, x + ABORT_HOLD_NS);
    assert!(!e.interference(), "then lets go");
    e.board(x + ABORT_HOLD_NS, false);
    assert!(!e.level(), "and the line is low with the board's driver off too");
}

// --- A station on the cable, and the turn timer's round ------------------

/// The words of a packet with `data` data words, and the bits it takes on
/// the cable: the buffer, the source and check words the interface adds,
/// and the zero bit at the end.
fn on_the_cable(data: usize) -> (usize, u64) {
    let words = 9 + data;
    (words, ((words + 2) * 16 + 1) as u64 * wire::CELL_NS)
}

/// **A station's next frame waits for its host to refill the buffer, and
/// then for its turn to come round.** A station's software cannot reload
/// the transmitter within a slot: it writes the packet a sixteen-bit word
/// at a time down the Unibus and reads `START`, which is [`refill`], and
/// by then the one count after the cable idled --- its own turn, its own
/// source word having loaded the counter with zero --- has gone by. Bit 7
/// of the counter comes round again 256 counts later, so two short frames
/// from one station are 257 slots apart, as the netlist board's are
/// (`two_packets_back_to_back_wait_a_whole_round` in
/// `tests/chaos_netlist.rs`). Two full ones are 769: the board takes a
/// word no faster than every 2,451 ns, its transmit shift register holding
/// the next write while the last word shifts in
/// (`a_word_written_while_the_last_shifts_is_answered_as_the_board_answers`
/// there), so 253 words take more than two whole rounds to write.
#[test]
fn a_stations_next_frame_waits_for_its_host_to_refill() {
    for (what, data, slots) in [("short frames", 4usize, 257u64), ("full packets", 244, 769)] {
        let mut e = Ether::new();
        e.keep_log(true);
        let frames = vec![packet(0o3060, 0o3062, data), packet(0o3060, 0o3062, data)];
        let (a, _) = host(0o3060, frames, false);
        e.attach(a);
        run(&mut e, 0, 5_000_000);
        let sent = sent(&e);
        assert_eq!(sent.len(), 2, "{what}: both went: {sent:?}");
        let (words, bits) = on_the_cable(data);
        let end = sent[0].0 + bits;
        let gap = sent[1].0 - end;
        eprintln!(
            "{what}: {words} words, the first frame {} to {end}, the second at {}: {gap} ns, \
             {} slots; the refill is {} ns",
            sent[0].0,
            sent[1].0,
            gap / SLOT_NS,
            refill(words)
        );
        assert!(sent[1].0 >= end + refill(words), "{what}: not before its host had it ready");
        assert_eq!(gap / SLOT_NS, slots, "{what}: {gap} ns after its own frame");
    }
}

/// **A station's refill holds up its own next frame and nobody else's.**
/// The turn timer is one counter a station, loaded by every frame that
/// station hears: 3060 sends, and its second frame waits for its host;
/// 3057, whose address is one below the source it last heard and whose
/// turn is therefore the first count after the cable idles, is asked
/// before that count comes and takes the cable in the meantime. 3060's turn is then reckoned from the frame it
/// heard last, 3057's --- 255 counts and one --- and not from its own.
#[test]
fn a_stations_refill_holds_up_only_its_own_next_frame() {
    let mut e = Ether::new();
    e.keep_log(true);
    let frames = vec![packet(0o3060, 0o3062, 4), packet(0o3060, 0o3062, 4)];
    let (a, _) = host(0o3060, frames, false);
    e.attach(a);
    run(&mut e, 0, 62_500);
    let [(first, 0o3060)] = sent(&e)[..] else { panic!("3060 went once: {:?}", sent(&e)) };
    let (_, bits) = on_the_cable(4);
    assert!(!e.busy(62_500), "its frame is over and its second is waiting");

    // A station that hears 3060's frame gets its turn at the first count.
    let (c, _) = host(0o3057, vec![packet(0o3057, 0o3062, 4)], false);
    e.attach(c);
    run(&mut e, 62_500, 5_000_000);
    let sent = sent(&e);
    eprintln!("3060's frame at {first}, ending {}; then {:?}", first + bits, &sent[1..]);
    assert_eq!(sent.len(), 3, "all three frames went: {sent:?}");
    assert_eq!(sent[1].1, 0o3057, "3057 took the cable while 3060's host refilled");
    assert_eq!(sent[2].1, 0o3060, "and 3060's second frame came after it");
    // 3060's turn is reckoned from the frame it heard last, 3057's, whose
    // source word loaded its counter with 3057 - 3060: a whole round of
    // 255 counts and one, measured from the cable going idle a bit cell
    // or so after the frame's nominal end.
    let round = (turn_byte(0o3057, 0o3060) as u64 + 1) * SLOT_NS;
    let after = sent[2].0 - (sent[1].0 + bits);
    eprintln!("3060 went {after} ns after 3057's frame ended, a round being {round} ns");
    assert!(
        (round.saturating_sub(wire::CELL_NS)..round + wire::IDLE_NS).contains(&after),
        "3060 waited a whole round after 3057's frame: {sent:?}"
    );
}

/// **A receiver whose buffer is full aborts what is addressed to it, and
/// counts what it would have taken.** AIM-628 §2.5's hardware flow
/// control, which the netlist board does in its gates and the ether does
/// for a behavioral board: [`BUSY_ABORT_NS`] into the frame, the bit cell
/// after the destination word, the board's driver goes on for
/// [`ABORT_HOLD_NS`] and the sender stops at its next clock edge with its
/// words handed back. A broadcast is counted and not aborted; either way
/// the receiver was never active for the frame, so nothing of it is
/// offered to the board.
#[test]
fn a_full_buffer_aborts_what_is_addressed_to_it_and_counts_it() {
    for (what, dest, abort) in [("addressed to it", 0o3050u16, true), ("a broadcast", 0, false)] {
        let mut e = Ether::new();
        e.keep_log(true);
        e.attach_board(0o3050);
        // A packet in the buffer already, and not spying.
        e.board_receiver(true, false);
        let (a, log_a) = host(0o3060, vec![packet(0o3060, dest, 4)], false);
        e.attach(a);
        run(&mut e, 0, 200_000);
        let [(s, 0o3060)] = sent(&e)[..] else { panic!("{what}: one frame: {:?}", sent(&e)) };
        let at = s + BUSY_ABORT_NS;
        let mut lost = Vec::new();
        while let Some(x) = e.board_lost() {
            lost.push(x);
        }
        eprintln!("{what}: the frame at {s}, counted {lost:?}, the host {:?}", log_a.lock());
        assert_eq!(lost, [(at, 0o3060, abort)], "{what}: counted once, at the destination word");
        // At its next clock edge with its own driver high: interference
        // that is not there at an edge is missed, and the sender goes on
        // until one finds it, which is within the cell.
        let stopped = log_a.lock().unwrap().iter().any(|h| {
            matches!(h, Happened::Aborted(t, _)
            if (at..=at + wire::CELL_NS).contains(t))
        });
        assert_eq!(stopped, abort, "{what}: whether the sender was stopped within a cell");
        let (_, r, taken) =
            e.board_heard().unwrap_or_else(|| panic!("{what}: the board heard it go by"));
        assert!(!taken, "{what}: its receiver was never active for it");
        assert_eq!(r.framed.check_ok, !abort, "{what}: aborted, it ends as wreckage");
        assert!(e.board_heard().is_none(), "{what}: and nothing else came");
    }
}

use muir::chaos::ether::{Capture, RACT_NS, ROUND_NS};

/// **A station asked for a frame long after the cable went idle goes on
/// its turn timer's cadence, not at the instant it was asked.** The board's
/// counter runs free while the cable is idle (`Turn::tc` and
/// `Turn::idle_run` in `src/chaos/board.rs`), so a turn missed is gone and
/// the next is a whole round on. 3060 sends; long after, 3057 --- whose turn
/// after 3060's frame is the first count --- is given a frame. Asked twice in
/// one round, at 700,000 and 700,300 ns, it goes at the same instant; asked a
/// round later, a round later; and every time one count after the cable went
/// idle, modulo a round. `Capture`, the harness's instrument, keeps the
/// ether's old rule and goes one slot after it was asked.
#[test]
fn a_station_after_a_long_idle_goes_on_the_turn_timers_cadence() {
    let (_, bits) = on_the_cable(4);
    let go = |asked: u64, station: bool| -> (u64, u64) {
        let mut e = Ether::new();
        e.keep_log(true);
        let (a, _) = host(0o3060, vec![packet(0o3060, 0o3062, 4)], false);
        e.attach(a);
        run(&mut e, 0, asked);
        if station {
            let (c, _) = host(0o3057, vec![packet(0o3057, 0o3062, 4)], false);
            e.attach(c);
        } else {
            let mut c = Capture::new(0o3057);
            c.to_send.push_back(packet(0o3057, 0o3062, 4));
            e.attach(Box::new(c));
        }
        run(&mut e, asked, asked + 2 * ROUND_NS);
        let sent = sent(&e);
        assert_eq!(sent.len(), 2, "asked at {asked}: both frames went: {sent:?}");
        (sent[0].0, sent[1].0)
    };
    let mut starts = Vec::new();
    for asked in [700_000, 700_300, 700_000 + ROUND_NS] {
        let (first, at) = go(asked, true);
        let since = (at - (first + bits)) % ROUND_NS;
        eprintln!(
            "asked at {asked}: 3060's frame at {first}, 3057's at {at}, {since} ns past a round"
        );
        assert!(at >= asked && at - asked < ROUND_NS, "asked at {asked}, it went at {at}");
        assert!(
            (SLOT_NS..SLOT_NS + wire::IDLE_NS + 1).contains(&since),
            "one count after the cable went idle, modulo a round: {since} ns"
        );
        starts.push(at);
    }
    assert_eq!(starts[0], starts[1], "two asks in one round go at the same instant");
    assert_eq!(starts[2] - starts[0], ROUND_NS, "asked a round later, it goes a round later");
    let (_, at) = go(700_000, false);
    assert_eq!(at, 700_000 + SLOT_NS, "the harness's instrument goes one slot after it was asked");
}

/// **The receiver decides at `START^`, and keeps it for the frame.** `RACT`,
/// the 74S74 at LMRCLK 0C06, clocks `-RDONE` in on `START^`, [`RACT_NS`]
/// after the frame's first edge, and holds it while the cable is busy. A
/// Clear Receiver just before that instant lets the frame in; just after,
/// or anywhere before the destination word, it does not: nothing is stored,
/// and the frame is counted and aborted as if the buffer were still full.
#[test]
fn the_receiver_decides_at_start_and_keeps_it_for_the_frame() {
    for (what, cleared, taken) in [
        ("cleared before START^", SLOT_NS + RACT_NS - 1, true),
        ("cleared at START^'s own instant, after it", SLOT_NS + RACT_NS + 1, false),
        ("cleared well inside the frame", SLOT_NS + 5_000, false),
    ] {
        let mut e = Ether::new();
        e.keep_log(true);
        e.attach_board(0o3050);
        e.board_receiver(true, false);
        let (a, log_a) = host(0o3060, vec![packet(0o3060, 0o3050, 4)], false);
        e.attach(a);
        run(&mut e, 0, cleared);
        e.board_receiver(false, false);
        run(&mut e, cleared, 200_000);
        let [(s, 0o3060)] = sent(&e)[..] else { panic!("{what}: one frame: {:?}", sent(&e)) };
        assert_eq!(s, SLOT_NS, "{what}: the frame at its first turn");
        let mut lost = Vec::new();
        while let Some(x) = e.board_lost() {
            lost.push(x);
        }
        let aborted = log_a.lock().unwrap().iter().any(|h| matches!(h, Happened::Aborted(..)));
        let (_, r, took) =
            e.board_heard().unwrap_or_else(|| panic!("{what}: the board heard it go by"));
        eprintln!("{what}: counted {lost:?}, aborted {aborted}, taken {took}");
        assert_eq!(took, taken, "{what}: whether the receiver was active for it");
        assert_eq!(aborted, !taken, "{what}: aborted when it was not taken");
        assert_eq!(r.framed.check_ok, taken, "{what}: whole, or wreckage");
        let want: Vec<(u64, u16, bool)> =
            if taken { Vec::new() } else { vec![(s + BUSY_ABORT_NS, 0o3060, true)] };
        assert_eq!(lost, want, "{what}: counted when it was not taken");
    }
}
