// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **How muir reaches a network** (QUUX's Ethernet contract, §3):
//! `--ethernet-socket PATH[,clocked]`, `--ethernet-mac`, `--ip-address`,
//! `--ip-gateway` and `--ethernet-trace` ([`muir::ethernet_socket`]), and
//! the guard on a resume (§3, "The two guards").
//!
//! The library's tests put the device's host side on one end of a socket
//! pair and play the peer at the other, moving the machine's clock an edge
//! at a time as an engine does. The executable's tests listen on a socket
//! of their own and run `quux` against it, on a revision-16 boot PROM that
//! loops, or on a checkpoint whose device a test enabled.

mod support;

use muir::ethernet_device::{self as eth, Say};
use muir::ethernet_socket::{self as sock, Mode};
use muir::machine::{Geometry, Machine, Word};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PAGE: u32 = muir::tlb::REGISTER_PAGE_BUS;

/// Revision 16's period in tests: 17 units of 0.5 ns, 8.5 ns.
const PERIOD: u64 = 17;

/// The driver's rings, its transmit frames and its buffers, on lines: up
/// to 8 entries a ring, a place of 512 words for each slot.
const TX_RING: u32 = 0o1000;
const RX_RING: u32 = 0o2000;
const FRAMES: u32 = 0o20000;
const BUFFERS: u32 = 0o40000;
const ROOM: u32 = 1536;

/// The device's default MAC address, another station's, and the peer's.
const OWN: [u8; 6] = [0x02, 0, 0, 0, 0, 0x01];
const OTHER: [u8; 6] = [0x02, 0, 0, 0, 0, 0x03];
const PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 0x02];

/// How long a test waits on the host's side before it fails.
const WALL: Duration = Duration::from_secs(20);

fn rev16() -> Machine {
    let mut m = Machine::with_geometry(Geometry::QUUX_16, 1);
    m.period = PERIOD;
    m
}

fn get(m: &mut Machine, w: u32) -> u32 {
    m.bus_read(PAGE + w) as u32
}

fn put(m: &mut Machine, w: u32, v: u32) {
    m.bus_write(PAGE + w, Word::from(v));
}

/// One edge, as an engine makes it: the clock a period on, the device
/// advanced at it.
fn edge(m: &mut Machine) {
    m.ns += PERIOD;
    m.advance_ethernet_at_edge();
}

/// Edges until `done`, or the host's [`WALL`].
fn edges_until(m: &mut Machine, what: &str, mut done: impl FnMut(&mut Machine) -> bool) {
    let t = Instant::now();
    while !done(m) {
        assert!(t.elapsed() < WALL, "{what}: not by {} units", m.ns);
        for _ in 0..64 {
            edge(m);
        }
    }
}

/// A [`Say`] that keeps its lines.
fn lines() -> (Say, Arc<Mutex<Vec<String>>>) {
    let kept = Arc::new(Mutex::new(Vec::new()));
    let k = kept.clone();
    (Arc::new(move |l: &str| k.lock().unwrap().push(l.to_string())), kept)
}

fn said(kept: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    kept.lock().unwrap().clone()
}

/// An Ethernet frame of `n` bytes to `dst` from the peer, of `ethertype`,
/// its bytes after the header counting from `seed`.
fn frame(dst: [u8; 6], ethertype: u16, n: usize, seed: u8) -> Vec<u8> {
    let mut f = vec![0u8; n];
    f[..6].copy_from_slice(&dst);
    f[6..12].copy_from_slice(&PEER);
    f[12..14].copy_from_slice(&ethertype.to_be_bytes());
    for (k, b) in f[14..].iter_mut().enumerate() {
        *b = seed.wrapping_add(k as u8);
    }
    f
}

fn arp(n: usize, seed: u8) -> Vec<u8> {
    frame(OWN, 0x0806, n, seed)
}

/// The rings configured, 2^`tx` and 2^`rx` entries, and the device
/// enabled.
fn enable(m: &mut Machine, tx: u32, rx: u32) {
    configure(m, tx, rx);
    put(m, eth::CONTROL, 1);
    assert_eq!(get(m, eth::STATUS) & 0xf, 1, "enabled");
}

fn configure(m: &mut Machine, tx: u32, rx: u32) {
    put(m, eth::TX_BASE, TX_RING);
    put(m, eth::TX_SIZE, tx);
    put(m, eth::RX_BASE, RX_RING);
    put(m, eth::RX_SIZE, rx);
}

fn put_bytes(m: &mut Machine, at: u32, bytes: &[u8]) {
    for (k, chunk) in bytes.chunks(4).enumerate() {
        let mut w = [0u8; 4];
        w[..chunk.len()].copy_from_slice(chunk);
        m.main[at as usize + k] = Word::from(u32::from_le_bytes(w));
    }
}

fn bytes_at(m: &Machine, at: u32, n: usize) -> Vec<u8> {
    (0..n).map(|k| (m.main[at as usize + k / 4] >> (8 * (k % 4))) as u8).collect()
}

/// Transmit entry `index` written, of `len` bytes from `f`, and 244 moved
/// past it.
fn post_len(m: &mut Machine, index: u16, f: &[u8], len: u32) {
    let slot = u32::from(index) % (1 << get(m, eth::TX_SIZE));
    let addr = FRAMES + 0o1000 * slot;
    put_bytes(m, addr, f);
    let e = (TX_RING + 8 * slot) as usize;
    m.main[e] = Word::from(addr);
    m.main[e + 1] = Word::from(len);
    put(m, eth::TX_PROD, index.wrapping_add(1).into());
}

fn post(m: &mut Machine, index: u16, f: &[u8]) {
    post_len(m, index, f, f.len() as u32);
}

/// Receive entry `index` given a buffer of `room` bytes, and 250 moved past
/// it.
fn give_room(m: &mut Machine, index: u16, room: u32) {
    let slot = u32::from(index) % (1 << get(m, eth::RX_SIZE));
    let e = (RX_RING + 8 * slot) as usize;
    m.main[e] = Word::from(BUFFERS + 0o1000 * slot);
    m.main[e + 1] = Word::from(room);
    m.main[e + 2] = 0;
    put(m, eth::RX_PROD, index.wrapping_add(1).into());
}

fn give(m: &mut Machine, index: u16) {
    give_room(m, index, ROOM);
}

/// The Ethernet frame in receive entry `index`.
fn received(m: &Machine, index: u16, rx: u32) -> Vec<u8> {
    let slot = u32::from(index) % (1 << rx);
    let e = (RX_RING + 8 * slot) as usize;
    let n = m.main[e + 2] as u32 as usize;
    bytes_at(m, m.main[e] as u32, n)
}

// --- the records ---------------------------------------------------------

/// A plain record: the length, 32 bits big-endian, and the bytes.
fn plain(f: &[u8]) -> Vec<u8> {
    let mut r = (f.len() as u32).to_be_bytes().to_vec();
    r.extend_from_slice(f);
    r
}

/// A clocked record: the length, the machine time in ns, 64 bits
/// big-endian, and the bytes; none for "run until".
fn timed(t: u64, f: &[u8]) -> Vec<u8> {
    let mut r = (f.len() as u32).to_be_bytes().to_vec();
    r.extend_from_slice(&t.to_be_bytes());
    r.extend_from_slice(f);
    r
}

fn run_until(t: u64) -> Vec<u8> {
    timed(t, &[])
}

fn read_plain(s: &mut UnixStream) -> Vec<u8> {
    let mut n = [0u8; 4];
    s.read_exact(&mut n).unwrap();
    let mut f = vec![0u8; u32::from_be_bytes(n) as usize];
    s.read_exact(&mut f).unwrap();
    f
}

/// A clocked record from muir, and its bytes as they came.
fn read_timed(s: &mut impl Read) -> (u64, Vec<u8>, Vec<u8>) {
    let mut head = [0u8; 12];
    s.read_exact(&mut head).unwrap();
    let n = u32::from_be_bytes(head[..4].try_into().unwrap()) as usize;
    let t = u64::from_be_bytes(head[4..].try_into().unwrap());
    let mut f = vec![0u8; n];
    s.read_exact(&mut f).unwrap();
    let mut raw = head.to_vec();
    raw.extend_from_slice(&f);
    (t, f, raw)
}

/// Records from muir up to and with its next "reached": the Ethernet
/// frames, with their times, and the time reached.
fn until_reached(s: &mut impl Read, raw: &mut Vec<u8>) -> (Vec<(u64, Vec<u8>)>, u64) {
    let mut frames = Vec::new();
    loop {
        let (t, f, r) = read_timed(s);
        raw.extend_from_slice(&r);
        if f.is_empty() {
            return (frames, t);
        }
        frames.push((t, f));
    }
}

fn pair() -> (UnixStream, UnixStream) {
    let (a, b) = UnixStream::pair().unwrap();
    b.set_read_timeout(Some(WALL)).unwrap();
    (a, b)
}

// --- plain mode ----------------------------------------------------------

/// **Plain mode carries each Ethernet frame as a 32-bit big-endian length
/// and its bytes, both ways** (§3, QEMU's `-netdev stream`): a 14-byte and
/// a 1514-byte transmit back to back reach the peer whole and in order;
/// two records the peer writes at once, and a third written in three
/// pieces that split its length and its bytes, land in three buffers.
/// **Fails** a length of another width or order, a transmit lost behind
/// another, or a read that takes a short read for a record.
#[test]
fn plain_framing_carries_ethernet_frames_both_ways() {
    let (ours, mut theirs) = pair();
    let (say, kept) = lines();
    let mut m = rev16();
    m.attach_ethernet_peer(sock::over(ours, Mode::Plain, 2, say).unwrap());
    enable(&mut m, 2, 2);
    let short = frame(PEER, 0x0806, 14, 0);
    let long = frame(PEER, 0x0806, 1514, 7);
    post(&mut m, 0, &short);
    post(&mut m, 1, &long);
    edges_until(&mut m, "two sent", |m| get(m, eth::TX_CONS) == 2);
    assert_eq!(read_plain(&mut theirs), short, "the 14-byte Ethernet frame");
    assert_eq!(read_plain(&mut theirs), long, "the 1514-byte one");

    for k in 0..4 {
        give(&mut m, k);
    }
    let (a, b, c) = (arp(14, 1), arp(1514, 2), arp(60, 3));
    let mut both = plain(&a);
    both.extend(plain(&b));
    theirs.write_all(&both).unwrap();
    let r = plain(&c);
    for piece in [&r[..2], &r[2..30], &r[30..]] {
        theirs.write_all(piece).unwrap();
        theirs.flush().unwrap();
        std::thread::sleep(Duration::from_millis(30));
    }
    edges_until(&mut m, "three received", |m| get(m, eth::RX_CONS) == 3);
    assert_eq!(received(&m, 0, 2), a, "the 14-byte Ethernet frame");
    assert_eq!(received(&m, 1, 2), b, "the 1514-byte one, back to back with it");
    assert_eq!(received(&m, 2, 2), c, "the one split across three writes");
    for _ in 0..1000 {
        edge(&mut m);
    }
    assert_eq!(get(&mut m, eth::RX_CONS), 3, "and nothing more");
    assert!(said(&kept).is_empty(), "no notice: {:?}", said(&kept));
}

/// **The peer closing leaves the device as without a socket** (a stated
/// choice; §3 says nothing of it): muir says so once, the run goes on,
/// transmits go nowhere with 245 advancing, and nothing arrives. **Fails** a
/// run that stops or panics on the close, a notice for every transmit, or
/// none.
#[test]
fn the_peer_closing_leaves_the_device_as_without_a_socket() {
    let (ours, theirs) = pair();
    let (say, kept) = lines();
    let mut m = rev16();
    m.attach_ethernet_peer(sock::over(ours, Mode::Plain, 2, say).unwrap());
    enable(&mut m, 2, 2);
    give(&mut m, 0);
    drop(theirs);
    edges_until(&mut m, "the close said", |_| !said(&kept).is_empty());
    for k in 0..3u16 {
        post(&mut m, k, &frame(PEER, 0x0806, 60, k as u8));
        edges_until(&mut m, "sent", |m| get(m, eth::TX_CONS) == u32::from(k) + 1);
    }
    let s = said(&kept);
    assert_eq!(s.len(), 1, "one notice: {s:?}");
    assert!(s[0].contains("the peer closed the socket"), "{s:?}");
    assert_eq!(get(&mut m, eth::RX_CONS), 0, "nothing arrived");
    assert_eq!(get(&mut m, eth::STATUS) >> 16, 0, "nothing dropped or refused");
}

/// The ARP Ethernet frame numbered `k`, its number in bytes 14-15.
fn numbered(k: u16) -> Vec<u8> {
    let mut f = frame(PEER, 0x0806, 60, 0);
    f[14..16].copy_from_slice(&k.to_be_bytes());
    f
}

/// The socket's buffer filled from muir's end before muir has it, so that
/// what waits for the peer is muir's hold and nothing more; the bytes put
/// in it.
fn filled(ours: &UnixStream) -> usize {
    ours.set_nonblocking(true).unwrap();
    let mut junk = 0;
    for piece in [4096, 1] {
        loop {
            match (&*ours).write(&vec![0u8; piece]) {
                Ok(n) => junk += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => panic!("{e}"),
            }
        }
    }
    ours.set_nonblocking(false).unwrap();
    junk
}

/// **At a plain run's end, what muir holds for the peer is written if the
/// socket takes it within a second, and what it does not take is said**
/// (§3): the socket's buffer full and eight transmits sent and held, the
/// machine is dropped. With the peer reading, the eight come in order and
/// nothing is said; with the peer reading nothing, the end takes a second
/// and says the eight are dropped. **Fails** an end that shuts the socket
/// on what it holds, or one that waits on the peer without end.
#[test]
fn a_plain_run_ends_writing_what_it_holds_within_a_second() {
    for reads in [true, false] {
        let (ours, mut theirs) = pair();
        let junk = filled(&ours);
        let (say, kept) = lines();
        let mut m = rev16();
        m.attach_ethernet_peer(sock::over(ours, Mode::Plain, 2, say).unwrap());
        enable(&mut m, 3, 0);
        // On a thread of its own, so that a stall fails the test rather
        // than hanging it.
        let (done, machine) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for k in 0..8 {
                post(&mut m, k, &numbered(k));
            }
            edges_until(&mut m, "sent", |m| get(m, eth::TX_CONS) == 8);
            let _ = done.send(m);
        });
        let m =
            machine.recv_timeout(WALL).expect("the machine stalled on a peer that reads nothing");
        // The peer's end kept open either way: one that closed would end
        // the socket instead.
        let reader = if reads {
            Some(std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                let mut j = vec![0u8; junk];
                theirs.read_exact(&mut j).unwrap();
                let frames = (0..8).map(|_| read_plain(&mut theirs)).collect::<Vec<_>>();
                (frames, theirs)
            }))
        } else {
            None
        };
        let t = Instant::now();
        drop(m);
        let took = t.elapsed();
        let s = said(&kept);
        if let Some(reader) = reader {
            // The peer's end comes back and is held past the checks, so
            // that its close never races the run's end.
            let (frames, _theirs) = reader.join().unwrap();
            assert_eq!(frames, (0..8).map(numbered).collect::<Vec<_>>(), "the eight, in order");
            assert!(s.is_empty(), "nothing said: {s:?}");
        } else {
            assert!(
                (Duration::from_millis(900)..Duration::from_secs(3)).contains(&took),
                "the end took {took:?}"
            );
            assert_eq!(
                s,
                vec![
                    "ethernet: 8 Ethernet frames the peer had not taken are dropped as the run ends"
                ]
            );
        }
    }
}

/// **Plain mode never stalls the machine on a peer that stops reading**
/// (§3): the socket's own buffer filled first and the peer reading
/// nothing, the driver posts 1,100 transmits, one sent every 20 us, and
/// the machine runs 30 ms of its time, within 10 s of the host's. When the
/// peer reads, the first 1,024 come, whole and in order, and nothing after
/// them: the other 76 were dropped beyond the hold, said once, and counted
/// nowhere in the device, whose 245 passed all 1,100. **Fails** a write
/// on the machine's thread, a hold of another size, Ethernet frames
/// reordered, or a drop said for each.
#[test]
fn plain_mode_holds_1024_frames_for_a_peer_that_reads_nothing_and_runs_on() {
    let (ours, mut theirs) = pair();
    let junk = filled(&ours);
    let (say, kept) = lines();
    let mut m = rev16();
    m.attach_ethernet_peer(sock::over(ours, Mode::Plain, 2, say).unwrap());
    enable(&mut m, 3, 0);
    const SENT: u16 = 1_100;
    // The machine on a thread of its own, so that a stall fails the test
    // rather than hanging it.
    let (done, machine) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut posted = 0u16;
        while m.ns < 2 * 30_000_000 {
            for _ in 0..64 {
                edge(&mut m);
            }
            while posted < SENT && posted.wrapping_sub(get(&mut m, eth::TX_CONS) as u16) < 8 {
                post(&mut m, posted, &numbered(posted));
                posted += 1;
            }
        }
        let _ = done.send(m);
    });
    let mut m = machine
        .recv_timeout(Duration::from_secs(10))
        .expect("the machine stalled on a peer that reads nothing");
    assert_eq!(get(&mut m, eth::TX_CONS), u32::from(SENT), "245 passed every one");
    let s: Vec<String> = said(&kept).into_iter().filter(|l| l.contains("wait for")).collect();
    assert_eq!(
        s,
        vec![
            "ethernet: 1024 Ethernet frames wait for the peer; what the device sends is dropped until it takes them"
        ],
        "said once"
    );
    let mut j = vec![0u8; junk];
    theirs.read_exact(&mut j).unwrap();
    for k in 0..1_024u16 {
        assert_eq!(read_plain(&mut theirs), numbered(k), "Ethernet frame {k}");
    }
    std::thread::sleep(Duration::from_millis(300));
    theirs.set_nonblocking(true).unwrap();
    let mut more = [0u8; 1];
    let after = theirs.read(&mut more);
    assert!(
        after.as_ref().is_err_and(|e| e.kind() == std::io::ErrorKind::WouldBlock),
        "nothing after the 1,024th: {after:?}"
    );
    let status = get(&mut m, eth::STATUS);
    assert_eq!(status >> 4, 0, "no counter moved: {status:x}");
}

/// **In plain mode an Ethernet frame is delivered at the first edge after
/// it arrives** (§2.8), so one that arrives while the device is disabled is
/// dropped at an edge, with no register read: the trace says so before the
/// driver touches the device, and enabling it later takes nothing.
/// **Fails** a plain peer polled only while the device is enabled, which
/// keeps the frame for the enable.
#[test]
fn in_plain_mode_a_frame_arriving_while_disabled_is_dropped_at_an_edge() {
    let (ours, mut theirs) = pair();
    let (say, kept) = lines();
    let mut m = rev16();
    m.ethernet.trace = Some(say.clone());
    m.attach_ethernet_peer(sock::over(ours, Mode::Plain, 2, say).unwrap());
    configure(&mut m, 0, 0);
    theirs.write_all(&plain(&arp(60, 9))).unwrap();
    edges_until(&mut m, "the drop traced", |_| {
        said(&kept).iter().any(|l| l.contains("dropped: the device is disabled"))
    });
    put(&mut m, eth::CONTROL, 1);
    give(&mut m, 0);
    for _ in 0..1000 {
        edge(&mut m);
    }
    assert_eq!(get(&mut m, eth::RX_CONS), 0, "nothing kept for the enable");
}

/// **A band that never touches 240-257 never holds what a socket sends**
/// (QUUX's Ethernet contract, §2.7: an Ethernet
/// frame arriving while disabled is discarded and counted nowhere). 3,000
/// Ethernet frames sent to a disabled device, in plain mode and in clocked,
/// are each dropped at an edge with no register access; a later enable
/// takes none of them, and no counter moved. In plain mode, with the
/// machine standing still, at most [`sock::WAITING`] are held: the rest are
/// dropped as they arrive, said once. **Fails** a transport drained only at
/// a register access, or a plain queue without a bound.
#[test]
fn a_disabled_device_discards_what_a_socket_sends_and_holds_no_more() {
    const N: usize = 3_000;
    for mode in [Mode::Plain, Mode::Clocked] {
        let (ours, mut theirs) = pair();
        let (say, kept) = lines();
        let mut m = rev16();
        m.ethernet.trace = Some(say.clone());
        m.attach_ethernet_peer(sock::over(ours, mode, 2, say).unwrap());
        configure(&mut m, 0, 0);
        let writer = std::thread::spawn(move || {
            let mut w = Vec::new();
            for k in 0..N {
                let f = arp(60, k as u8);
                w.extend(match mode {
                    Mode::Plain => plain(&f),
                    Mode::Clocked => timed(1_000 + k as u64, &f),
                });
            }
            if mode == Mode::Clocked {
                let mut raw = Vec::new();
                until_reached(&mut theirs, &mut raw);
                w.extend(run_until(10_000));
                theirs.write_all(&w).unwrap();
            } else {
                // In pieces, so that the bound below is not what drops
                // them on a loaded host.
                for piece in w.chunks(64 * 200) {
                    theirs.write_all(piece).unwrap();
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            theirs
        });
        let dropped = |k: &Arc<Mutex<Vec<String>>>| {
            said(k).iter().filter(|l| l.contains("dropped: the device is disabled")).count()
        };
        edges_until(&mut m, "every one dropped", |_| dropped(&kept) == N);
        // Closed, so that a clocked run goes on past its "run until".
        drop(writer.join().unwrap());
        put(&mut m, eth::CONTROL, 1);
        give(&mut m, 0);
        for _ in 0..1000 {
            edge(&mut m);
        }
        assert_eq!(get(&mut m, eth::RX_CONS), 0, "{mode:?}: nothing kept for the enable");
        assert_eq!(get(&mut m, eth::STATUS) >> 4 & 0xf, 0, "{mode:?}: no type drop");
        assert_eq!(get(&mut m, eth::STATUS) >> 9, 0, "{mode:?}: nothing counted");
    }

    // The machine standing still: no edge while 3,000 arrive.
    let (ours, mut theirs) = pair();
    let (say, kept) = lines();
    let mut m = rev16();
    m.ethernet.trace = Some(say.clone());
    m.attach_ethernet_peer(sock::over(ours, Mode::Plain, 2, say).unwrap());
    configure(&mut m, 0, 0);
    let mut w = Vec::new();
    for k in 0..N {
        w.extend(plain(&arp(60, k as u8)));
    }
    theirs.write_all(&w).unwrap();
    let t = Instant::now();
    while !said(&kept).iter().any(|l| l.contains("wait for the machine")) {
        assert!(t.elapsed() < WALL, "the bound never said");
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(200));
    for _ in 0..1000 {
        edge(&mut m);
    }
    let s = said(&kept);
    let held = s.iter().filter(|l| l.contains("dropped: the device is disabled")).count();
    assert_eq!(held, sock::WAITING, "only so many held");
    assert_eq!(s.iter().filter(|l| l.contains("wait for the machine")).count(), 1, "said once");
}

// --- clocked mode --------------------------------------------------------

/// **Clocked mode delivers each Ethernet frame at the time it names, never
/// before the time reached, and waits at each "reached"** (§3, §2.8). muir
/// says "reached" at its first edge, 17 units, 8 ns. The peer sends an
/// Ethernet frame for 50,000 ns, one for 0, already passed, and "run until
/// 100,000": the second is delivered at the time reached, 17 units, the
/// first at the first clock at or after 100,000 units; muir says "reached"
/// at its first edge at or after 200,000 units, 100,002 ns, and stands
/// there while the peer is silent. "Run until" a time passed is answered at
/// once with the same time. **Fails** a frame delivered when its record
/// arrived, a frame from the past delivered before the time reached, a
/// machine that runs on past T, or a "reached" with the wrong time.
#[test]
fn clocked_frames_arrive_at_their_times_and_muir_waits_at_each_reached() {
    let (ours, mut theirs) = pair();
    let (say, kept) = lines();
    let mut m = rev16();
    m.attach_ethernet_peer(sock::over(ours, Mode::Clocked, 2, say).unwrap());
    enable(&mut m, 0, 2);
    for k in 0..4 {
        give(&mut m, k);
    }
    let (a, b) = (arp(60, 1), arp(61, 2));
    let now = Arc::new(AtomicU64::new(0));
    let seen = now.clone();
    let (fa, fb) = (a.clone(), b.clone());
    let peer = std::thread::spawn(move || {
        let mut raw = Vec::new();
        let (none, r0) = until_reached(&mut theirs, &mut raw);
        assert!(none.is_empty());
        let mut w = timed(50_000, &fa);
        w.extend(timed(0, &fb));
        w.extend(run_until(100_000));
        theirs.write_all(&w).unwrap();
        let (_, r1) = until_reached(&mut theirs, &mut raw);
        std::thread::sleep(Duration::from_millis(200));
        let held = seen.load(Ordering::SeqCst);
        theirs.write_all(&run_until(50)).unwrap();
        let (_, again) = until_reached(&mut theirs, &mut raw);
        theirs.write_all(&run_until(200_000)).unwrap();
        let (_, r2) = until_reached(&mut theirs, &mut raw);
        (r0, r1, held, again, r2)
    });
    let mut deliveries = Vec::new();
    let mut cons = 0;
    while m.ns < 2 * 250_000 {
        m.ns += PERIOD;
        now.store(m.ns, Ordering::SeqCst);
        m.advance_ethernet_at_edge();
        let c = get(&mut m, eth::RX_CONS);
        if c != cons {
            cons = c;
            deliveries.push(m.ethernet.last_delivery().unwrap());
        }
    }
    let (r0, r1, held, again, r2) = peer.join().unwrap();
    assert_eq!(r0, 8, "the first edge, 17 units");
    assert_eq!(deliveries, vec![17, 100_011], "the passed one at the time reached, then 50 us's");
    assert_eq!(received(&m, 0, 2), b);
    assert_eq!(received(&m, 1, 2), a);
    assert_eq!(r1, 100_002, "the first edge at or after 100,000 ns: 200,005 units");
    assert_eq!(held, 200_005, "the machine stood at it while the peer was silent");
    assert_eq!(again, r1, "run until a time passed: reached at once");
    assert_eq!(r2, 200_005, "the first edge at or after 400,000 units, 400,010");
    assert!(said(&kept).iter().any(|l| l.contains("the peer closed the socket")));
}

/// **Clocked mode reports each transmit with the time it was sent, before
/// the "reached" that covers it** (§3): two transmits posted at the first
/// edge at or after 20,000 units, 20,009, are sent at 60,010 and 100,011
/// units, 30,005 and 50,005 ns, and come in order before "reached".
/// **Fails** a transmit time not the machine's, or a transmit held past the
/// "reached" it falls before.
#[test]
fn clocked_mode_reports_each_transmit_with_its_time_before_reached() {
    let (ours, mut theirs) = pair();
    let (say, _kept) = lines();
    let mut m = rev16();
    m.attach_ethernet_peer(sock::over(ours, Mode::Clocked, 2, say).unwrap());
    enable(&mut m, 1, 0);
    let (f1, f2) = (frame(PEER, 0x0806, 60, 1), frame(PEER, 0x0806, 1514, 2));
    let peer = std::thread::spawn(move || {
        let mut raw = Vec::new();
        until_reached(&mut theirs, &mut raw);
        theirs.write_all(&run_until(100_000)).unwrap();
        until_reached(&mut theirs, &mut raw)
    });
    let mut posted = false;
    while m.ns < 2 * 150_000 {
        edge(&mut m);
        if !posted && m.ns >= 20_000 {
            assert_eq!(m.ns, 20_009);
            post(&mut m, 0, &f1);
            post(&mut m, 1, &f2);
            posted = true;
        }
    }
    let (frames, reached) = peer.join().unwrap();
    assert_eq!(frames, vec![(30_005, f1), (50_005, f2)]);
    assert_eq!(reached, 100_002);
}

/// One run of the echo script: the peer sends six Ethernet frames at named
/// times and three "run until"s, writing them at once or, `slow`, a few
/// bytes at a time with pauses, some past [`sock::WAKE`] inside a record;
/// the driver sends each frame it receives
/// back to the peer. Every byte muir sent.
fn echo_run(slow: bool) -> Vec<u8> {
    let (ours, mut theirs) = pair();
    let (say, _kept) = lines();
    let mut m = rev16();
    m.attach_ethernet_peer(sock::over(ours, Mode::Clocked, 2, say).unwrap());
    enable(&mut m, 2, 2);
    for k in 0..4 {
        give(&mut m, k);
    }
    let peer = std::thread::spawn(move || {
        let mut raw = Vec::new();
        until_reached(&mut theirs, &mut raw);
        let mut script = Vec::new();
        for k in 0..6u8 {
            script.push(timed(10_000 + 25_000 * u64::from(k), &arp(60 + usize::from(k), k)));
        }
        for t in [50_000, 100_000, 160_000] {
            script.push(run_until(t));
        }
        let mut sent = 0;
        for (k, r) in script.iter().enumerate() {
            if slow {
                for (j, piece) in r.chunks(5).enumerate() {
                    theirs.write_all(piece).unwrap();
                    // Past the clocked wait's wake, in the middle of a
                    // record, now and then.
                    let pause = if j == 1 && k % 3 == 0 {
                        3 * sock::WAKE / 2
                    } else {
                        Duration::from_millis(1)
                    };
                    std::thread::sleep(pause);
                }
            } else {
                theirs.write_all(r).unwrap();
            }
            if k >= 6 {
                until_reached(&mut theirs, &mut raw);
                sent += 1;
            }
        }
        assert_eq!(sent, 3);
        raw
    });
    let (mut cons, mut tx) = (0u16, 0u16);
    while m.ns < 2 * 170_000 {
        edge(&mut m);
        let c = get(&mut m, eth::RX_CONS) as u16;
        while cons != c {
            let mut f = received(&m, cons, 2);
            f[..6].copy_from_slice(&PEER);
            f[6..12].copy_from_slice(&OWN);
            post(&mut m, tx, &f);
            tx += 1;
            cons += 1;
            put(&mut m, eth::RX_TAKEN, cons.into());
            give(&mut m, cons + 3);
        }
    }
    assert_eq!(cons, 6, "six received");
    peer.join().unwrap()
}

/// **Two runs of one clocked script give the same bytes, bit for bit**
/// (§3), whatever the host's timing: the peer writes it at once in one run
/// and a few bytes at a time in the other, pausing past the wait's wake
/// inside a record, and every record muir sent --- the six echoes with
/// their times, and each "reached" --- is the same. **Fails** a clocked
/// peer that reads the socket as bytes arrive rather than at its waits,
/// which delivers by the host's clock, or one that loses a record a wake
/// cut.
#[test]
fn two_runs_of_one_clocked_script_give_the_same_bytes() {
    let fast = echo_run(false);
    let slow = echo_run(true);
    let mut r = &fast[..];
    let mut echoes = 0;
    while !r.is_empty() {
        let (_, f, _) = read_timed(&mut r);
        echoes += usize::from(!f.is_empty());
    }
    assert_eq!(echoes, 6, "the six echoes are in it");
    assert!(fast == slow, "the two runs differ");
}

// --- no socket, the trace, the MAC address -------------------------------

/// **Without a socket, transmitted Ethernet frames go nowhere, 245 still
/// advances, and none arrive** (§3): nothing counted, and the trace says
/// each went to nothing. **Fails** a transmit that waits for a peer.
#[test]
fn without_a_socket_transmits_go_nowhere_and_245_advances() {
    let (say, kept) = lines();
    let mut m = rev16();
    m.ethernet.trace = Some(say);
    enable(&mut m, 2, 2);
    give(&mut m, 0);
    post(&mut m, 0, &frame(PEER, 0x0806, 60, 0));
    post(&mut m, 1, &frame(PEER, 0x0806, 60, 1));
    edges_until(&mut m, "both passed", |m| get(m, eth::TX_CONS) == 2);
    for _ in 0..1000 {
        edge(&mut m);
    }
    assert_eq!(get(&mut m, eth::RX_CONS), 0, "nothing arrived");
    assert_eq!(get(&mut m, eth::STATUS) >> 4, 0, "nothing counted");
    let s = said(&kept);
    assert_eq!(s.iter().filter(|l| l.contains("sent 60 bytes")).count(), 2, "{s:?}");
    assert!(s.iter().all(|l| l.contains("nothing behind the device")), "{s:?}");
}

/// **`--ethernet-trace` prints each Ethernet frame sent and received, and
/// why one is refused or dropped**, with `ethernet_offload`'s reasons
/// (§3): a transmit sent; one of type 86DD refused, and one of 13 bytes;
/// a received one taken into its entry; one to another MAC address, one
/// of type 86DD, one failing the IPv4 checks, one longer than its buffer,
/// and, after a short one fills that buffer, one with no buffer given,
/// dropped. Each line has the machine's time.
/// **Fails** a trace that misses a kind, or gives no reason.
#[test]
fn the_trace_says_what_is_sent_received_refused_and_dropped() {
    let (say, kept) = lines();
    let mut m = rev16();
    m.ethernet.trace = Some(say);
    let s = eth::Script::default();
    m.attach_ethernet_peer(Box::new(s.clone()));
    enable(&mut m, 2, 1);
    give(&mut m, 0);
    give_room(&mut m, 1, 32);
    post(&mut m, 0, &frame(PEER, 0x0806, 60, 0));
    post(&mut m, 1, &frame(PEER, 0x86dd, 60, 0));
    post_len(&mut m, 2, &frame(PEER, 0x0806, 60, 0), 13);
    s.arrive(1_000, arp(60, 1));
    s.arrive(2_000, frame(OTHER, 0x0806, 60, 0));
    s.arrive(3_000, frame(OWN, 0x86dd, 60, 0));
    s.arrive(4_000, frame(OWN, 0x0800, 60, 0x45));
    s.arrive(5_000, arp(100, 2));
    s.arrive(6_000, arp(20, 3));
    s.arrive(7_000, arp(60, 4));
    edges_until(&mut m, "all done", |m| get(m, eth::TX_CONS) == 3 && s.waiting() == 0);
    let t = said(&kept);
    let has = |what: &str| {
        assert!(t.iter().any(|l| l.starts_with("ethernet ") && l.contains(what)), "{what}:\n{t:#?}")
    };
    has("sent 60 bytes, 02:00:00:00:00:02 from 02:00:00:00:00:02, type 0806");
    has("transmit entry 1 refused: type not IPv4 or ARP");
    has("transmit entry 2 refused: length 13, outside 14-1514");
    has("received 60 bytes, 02:00:00:00:00:01 from 02:00:00:00:00:02, type 0806, into entry 0");
    has("dropped: not this device's MAC address");
    has("type 86dd, dropped: type not IPv4 or ARP");
    has("dropped: IPv4 total length past the frame");
    has(
        "received 100 bytes, 02:00:00:00:00:01 from 02:00:00:00:00:02, type 0806, dropped: entry 1's buffer of 32 bytes is too short",
    );
    has("received 20 bytes, 02:00:00:00:00:01 from 02:00:00:00:00:02, type 0806, into entry 1");
    has(
        "received 60 bytes, 02:00:00:00:00:01 from 02:00:00:00:00:02, type 0806, dropped: no buffer given",
    );
    // 1,000 units on the grid of 17: 1,003, 501.5 ns.
    assert!(
        t.iter().any(|l| l.starts_with("ethernet 501.5: received 60 bytes")),
        "the time:\n{t:#?}"
    );
    assert_eq!(t.len(), 10, "one line each:\n{t:#?}");
}

/// **`--ethernet-mac`'s words**: six pairs of hex digits by colons, any
/// case, and not a group address, whose `<0>` of byte 0 is set: a station's
/// own address is never one. **Fails** a parse that takes five pairs, a
/// digit too many, or a group address.
#[test]
fn ethernet_mac_takes_six_hex_pairs_and_no_group_address() {
    assert_eq!(sock::parse_mac("02:00:00:00:00:01"), Ok(OWN));
    assert_eq!(sock::parse_mac("0A:bc:De:f0:12:34"), Ok([0x0a, 0xbc, 0xde, 0xf0, 0x12, 0x34]));
    for bad in [
        "02:00:00:00:00",
        "02:00:00:00:00:01:02",
        "02:00:00:00:00:001",
        "02:00:00:00:00:0g",
        "02-00-00-00-00-01",
        "020000000001",
        "",
        "01:00:5e:00:00:01",
        "ff:ff:ff:ff:ff:ff",
    ] {
        assert!(sock::parse_mac(bad).is_err(), "{bad:?} taken");
    }
    assert_eq!(sock::mac_text([0x02, 0xab, 0, 0, 0, 0x01]), "02:ab:00:00:00:01");
    assert_eq!(
        sock::parse_socket("a/b"),
        Ok(sock::Socket { path: "a/b".into(), mode: Mode::Plain })
    );
    assert_eq!(
        sock::parse_socket("a,b,clocked"),
        Ok(sock::Socket { path: "a,b".into(), mode: Mode::Clocked })
    );
    assert!(sock::parse_socket("").is_err());
    assert!(sock::parse_socket(",clocked").is_err());
}

// --- the executable ------------------------------------------------------

use support::{Run, quux, scratch, text};

/// Revision 16's boot PROM that loops: a revision-15 `.mcr`, section 6
/// saying 15, and a jump to itself at 36000.
fn prom_16(dir: &std::path::Path) -> std::path::PathBuf {
    use muir::mcr;
    let jump = muir::isa::asm::JUMP
        | muir::isa::asm::target(0o36000)
        | muir::isa::asm::ALWAYS
        | muir::isa::asm::N;
    let mut w = vec![mcr::FORMAT_WORD, 2];
    w.extend_from_slice(&[6, 1, 32, 32, 0, 0, 0, 0, 15]);
    w.extend_from_slice(&[1, 1, 64, 64, 0o36000, 0, 0, 0, jump as u32, (jump >> 32) as u32]);
    let mut b: Vec<u8> = w.iter().flat_map(|x| x.to_le_bytes()).collect();
    b.resize(b.len().div_ceil(mcr::BLOCK_BYTES) * mcr::BLOCK_BYTES, 0);
    let path = dir.join("prom-16.mcr");
    std::fs::write(&path, b).unwrap();
    path
}

const FLAGS: [(&str, &str); 5] = [
    ("--ethernet-socket", "/nowhere/socket"),
    ("--ethernet-mac", "02:00:00:00:00:05"),
    ("--ip-address", "192.0.2.6/30"),
    ("--ip-gateway", "192.0.2.5"),
    ("--ethernet-trace", ""),
];

fn refused(out: &std::process::Output, saying: &str) {
    let t = text(out);
    assert_eq!(out.status.code(), Some(2), "not a usage error:\n{t}");
    assert!(t.contains(saying), "{saying:?}:\n{t}");
}

/// **The CADR and revisions below 16 refuse the five flags, by name**
/// (§3, "Flags"): `cadr` says they are quux's, and revisions 13, 14 and 15
/// that they are revision 16's, before anything is connected. **Fails** a
/// flag taken, or ignored, where there is no device.
#[test]
fn the_flags_are_refused_on_the_cadr_and_below_revision_16() {
    for (flag, value) in FLAGS {
        let args: Vec<&str> = [flag, value].into_iter().filter(|a| !a.is_empty()).collect();
        let out = support::cadr().args(&args).args(["--stop-after", "1"]).run();
        refused(&out, &format!("{flag} is quux's, not cadr's"));
        for revision in [None, Some("14"), Some("15")] {
            let mut c = quux();
            if let Some(r) = revision {
                c.env("MUIR_QUUX_REVISION", r);
            }
            let out = c.args(&args).args(["--stop-after", "1"]).run();
            refused(&out, &format!("{flag} is QUUX revision 16's: MUIR_QUUX_REVISION=16"));
        }
    }
}

/// The Chaosnet's five flags, each with a value `quux` takes below 16.
const CHAOS_FLAGS: [(&str, &[&str]); 5] = [
    ("--chaos-address", &["--chaos-address", "3050"]),
    ("--chaos-trace", &["--chaos-trace"]),
    ("--chaos-udp", &["--chaos-udp", "127.0.0.1:0"]),
    ("--chaos-udp-peer", &["--chaos-udp", "127.0.0.1:0", "--chaos-udp-peer", "3060@127.0.0.1:9"]),
    (
        "--chaos-udp-default-peer",
        &["--chaos-udp", "127.0.0.1:0", "--chaos-udp-default-peer", "127.0.0.1:9"],
    ),
];

/// **Revision 16 refuses the Chaosnet's flags, by name, and 13-15 take
/// them** (QUUX's Ethernet contract, §8 R4: revision 16 loses the
/// Chaosnet interface): on 16 each of `--chaos-address`, `--chaos-trace`,
/// `--chaos-udp`, `--chaos-udp-peer` and `--chaos-udp-default-peer` is a
/// usage error naming it and saying QUUX has no Chaosnet interface; on
/// 13, 14 and 15 the same arguments run, and the start names the
/// Chaosnet. 16's start has no chaosnet line. **Fails** a flag taken, or
/// ignored, on 16, or one refused below it.
#[test]
fn the_chaos_flags_are_refused_on_revision_16_and_taken_below() {
    let dir = scratch("chaos-flags");
    let prom = prom_16(&dir);
    let run = |revision: &str, args: &[&str]| {
        let mut c = quux();
        c.env("MUIR_QUUX_REVISION", revision).args([
            "--micro",
            "--main-memory-size",
            "2MW",
            "--stop-after",
            "1",
        ]);
        if revision != "13" && revision != "14" {
            c.arg("--prom").arg(&prom);
        }
        c.args(args).run()
    };
    for (flag, args) in CHAOS_FLAGS {
        // The flag alone, with its value: 16 refuses it where it stands,
        // before any check of the flags it goes with.
        let alone = &args[args.iter().position(|a| *a == flag).unwrap()..];
        refused(&run("16", alone), &format!("{flag}: QUUX has no Chaosnet interface"));
        for revision in ["13", "14", "15"] {
            let out = run(revision, args);
            let t = text(&out);
            assert!(out.status.success(), "{revision} {args:?}:\n{t}");
            assert!(t.contains("chaosnet: "), "{revision}: {t}");
        }
    }
    let out = run("16", &[]);
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(!t.lines().any(|l| l.starts_with("chaosnet")), "{t}");
}

/// **The flags' values are checked before the run**: a MAC address that
/// is not six pairs, or is a group address; a socket with no path; a socket
/// muir cannot connect to, named with the error. **Fails** a bad value
/// taken, or a connection refused late.
#[test]
fn the_flags_values_are_checked_before_the_run() {
    let dir = scratch("ethernet-values");
    let prom = prom_16(&dir);
    let run = |args: &[&str]| {
        quux()
            .env("MUIR_QUUX_REVISION", "16")
            .args(["--micro", "--main-memory-size", "2MW", "--stop-after", "1", "--prom"])
            .arg(&prom)
            .args(args)
            .run()
    };
    refused(&run(&["--ethernet-mac", "02:00:00:00:00"]), "--ethernet-mac wants");
    refused(&run(&["--ethernet-mac", "03:00:00:00:00:01"]), "a group address");
    refused(&run(&["--ethernet-mac"]), "--ethernet-mac wants");
    refused(&run(&["--ethernet-socket"]), "--ethernet-socket wants");
    refused(&run(&["--ethernet-socket", ",clocked"]), "--ethernet-socket wants");
    let gone = dir.join("no-such-socket");
    let out = run(&["--ethernet-socket", gone.to_str().unwrap()]);
    refused(&out, &format!("--ethernet-socket {}: ", gone.display()));
    let out = run(&[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("ethernet: MAC address 02:00:00:00:00:01, IPv4 address 169.254.100.2/30, gateway 169.254.100.1; no socket: transmitted Ethernet frames are dropped and none arrive"),
        "{}",
        text(&out)
    );
}

/// `quux` started on revision 16 with `args`, and the connection it makes
/// to a listener at a socket of the test's own, taken.
fn started(
    dir: &std::path::Path,
    name: &str,
    args: &[&str],
    clocked: bool,
) -> (support::Child, UnixStream) {
    started_from(quux(), dir, name, args, clocked)
}

/// [`started`], from the command `c`.
fn started_from(
    mut c: std::process::Command,
    dir: &std::path::Path,
    name: &str,
    args: &[&str],
    clocked: bool,
) -> (support::Child, UnixStream) {
    let path = dir.join(format!("{name}.sock"));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();
    let spec = format!("{}{}", path.display(), if clocked { ",clocked" } else { "" });
    let child = c
        .env("MUIR_QUUX_REVISION", "16")
        .args(["--main-memory-size", "2MW"])
        .args(args)
        .args(["--ethernet-socket", &spec])
        .start();
    listener.set_nonblocking(true).unwrap();
    let t = Instant::now();
    let stream = loop {
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(t.elapsed() < WALL, "quux never connected");
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => panic!("{e}"),
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(WALL)).unwrap();
    (child, stream)
}

/// One clocked run of `quux` on the looping PROM: "run until" 20,000 ns,
/// an Ethernet frame for 1,000 ns on the way, then the peer closes. Every
/// byte muir sent, and what it said.
fn clocked_run(dir: &std::path::Path, engine: &str, prom: &std::path::Path) -> (Vec<u8>, String) {
    let p = prom.to_str().unwrap();
    let (child, mut s) = started(
        dir,
        engine,
        &[engine, "--prom", p, "--ethernet-trace", "--stop-after", "5000"],
        true,
    );
    let mut raw = Vec::new();
    let (none, r0) = until_reached(&mut s, &mut raw);
    assert!(none.is_empty());
    let mut w = timed(1_000, &arp(60, 0));
    w.extend(run_until(20_000));
    s.write_all(&w).unwrap();
    let (none, r1) = until_reached(&mut s, &mut raw);
    assert!(none.is_empty());
    assert!(r0 < 100, "{engine}: the first edge: {r0}");
    assert!((20_000..21_000).contains(&r1), "{engine}: reached {r1}");
    drop(s);
    let out = child.wait();
    let t = text(&out);
    assert!(out.status.success(), "{engine}:\n{t}");
    (raw, t)
}

/// **`quux` on revision 16 waits in clocked mode on both engines, and two
/// runs give the same bytes** (§3): its first "reached" at its first edge,
/// the next within a microcycle of the 20,000 ns asked, and the Ethernet
/// frame on the way dropped and traced while the device is disabled; the
/// close said, and the run on to its stop. **Fails** an engine whose edge
/// does not wait, or a run whose bytes depend on the host.
#[test]
fn quux_waits_in_clocked_mode_on_both_engines_and_repeats_itself() {
    let dir = scratch("ethernet-clocked");
    let prom = prom_16(&dir);
    for engine in ["--micro", "--rtl"] {
        let (a, t) = clocked_run(&dir, engine, &prom);
        let (b, _) = clocked_run(&dir, engine, &prom);
        assert!(a == b, "{engine}: two runs differ");
        assert!(t.contains("; socket "), "{engine}: the setup:\n{t}");
        assert!(t.contains(", clocked mode"), "{engine}: the setup:\n{t}");
        assert!(
            t.lines().any(|l| l.starts_with("ethernet ")
                && l.contains("received 60 bytes")
                && l.contains("dropped: the device is disabled")),
            "{engine}: the trace:\n{t}"
        );
        assert!(t.contains("the peer closed the socket"), "{engine}:\n{t}");
    }
}

/// Whether `child` has closed its stdout and stderr, which it does as it
/// ends, within `within`.
fn ends_within(child: &support::Child, within: Duration) -> bool {
    let t = Instant::now();
    while t.elapsed() < within {
        if child.stdout().closed() && child.stderr().closed() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

/// **A ^C ends a run that waits on a silent clocked peer as it ends any
/// run** (§3), on both engines: muir has said "reached" at its first edge
/// and the peer sends nothing more; `kill -INT`, and the run quits as one
/// without a socket quits at a ^C --- "quit at PC", the same exit status
/// --- within a second. **Fails** a wait that reads the socket until the
/// peer sends, deaf to ^C.
#[test]
fn a_c_ends_a_run_waiting_on_a_silent_clocked_peer_within_a_second() {
    let dir = scratch("ethernet-interrupt");
    let prom = prom_16(&dir);
    let p = prom.to_str().unwrap();
    for engine in ["--micro", "--rtl"] {
        let normal = quux()
            .env("MUIR_QUUX_REVISION", "16")
            .args(["--main-memory-size", "2MW", engine, "--prom", p])
            .args(["--terminal", "127.0.0.1:0"])
            .start();
        let setup = |t: &str| t.contains("ethernet: MAC address");
        assert!(
            normal.stdout().wait_for(setup, WALL) || normal.stderr().wait_for(setup, WALL),
            "{engine}: no setup"
        );
        // The handler is put on as the run loop starts, after the setup.
        std::thread::sleep(Duration::from_millis(300));
        normal.interrupt();
        let normal = normal.wait();
        let n = text(&normal);
        assert!(n.contains("quit at PC"), "{engine}: a run without a socket, at ^C:\n{n}");

        let (child, mut s) =
            started(&dir, "interrupt", &[engine, "--prom", p, "--terminal", "127.0.0.1:0"], true);
        let mut raw = Vec::new();
        let (_, r0) = until_reached(&mut s, &mut raw);
        assert!(r0 < 100, "{engine}: the first edge: {r0}");
        std::thread::sleep(Duration::from_millis(300));
        child.interrupt();
        let ended = ends_within(&child, Duration::from_secs(1));
        if !ended {
            let so_far = format!("{}{}", child.stdout().so_far(), child.stderr().so_far());
            child.kill();
            panic!("{engine}: still waiting on the peer a second after ^C:\n{so_far}");
        }
        let out = child.wait();
        let t = text(&out);
        assert_eq!(out.status.code(), normal.status.code(), "{engine}: the exit status:\n{t}");
        assert!(t.contains("quit at PC"), "{engine}: quit as at a ^C:\n{t}");
        drop(s);
    }
}

/// A clocked script on the looping PROM: "run until" 20,000, 40,000, 60,000
/// and 80,000 ns, an Ethernet frame 500 ns after each time reached, then
/// the close. `held`: before the third "run until" the peer is silent, and
/// the run is ^C'd, held at the prompt, and continued. Every byte muir
/// sent, and its trace.
fn script_run(
    dir: &std::path::Path,
    engine: &str,
    prom: &str,
    held: bool,
) -> (Vec<u8>, Vec<String>) {
    let mut c = quux();
    c.stdin(std::process::Stdio::piped());
    let args = [engine, "--prom", prom, "--ethernet-trace", "--stop-after", "40000"];
    let (mut child, mut s) =
        started_from(c, dir, "held", &[&args[..], &["--terminal", "127.0.0.1:0"]].concat(), true);
    let mut typed = child.stdin();
    let mut raw = Vec::new();
    let (_, mut r) = until_reached(&mut s, &mut raw);
    for (k, t) in [20_000, 40_000, 60_000, 80_000].into_iter().enumerate() {
        if held && k == 2 {
            std::thread::sleep(Duration::from_millis(300));
            child.interrupt();
            assert!(
                child.stdout().wait_for(|t| t.contains("held at ^C"), WALL),
                "{engine}: not held:\n{}",
                child.stdout().so_far()
            );
            std::thread::sleep(Duration::from_millis(300));
            typed.write_all(b"continue\n").unwrap();
        }
        let mut w = timed(r + 500, &arp(60, k as u8));
        w.extend(run_until(t));
        s.write_all(&w).unwrap();
        r = until_reached(&mut s, &mut raw).1;
    }
    drop(s);
    drop(typed);
    let out = child.wait();
    let t = text(&out);
    assert!(out.status.success(), "{engine}:\n{t}");
    (raw, t.lines().filter(|l| l.starts_with("ethernet ")).map(String::from).collect())
}

/// **A clocked run held at ^C while it waits on its peer, and continued,
/// sends the bytes of one never held** (§3), on both engines: the wait
/// woken for the run loop's turns is the same wait, its records and its
/// deliveries the machine's, however long the hold. **Fails** a run that
/// goes on past the time named while ^C waits for the run loop, or a wait
/// whose wakes move a record.
#[test]
fn a_clocked_run_held_at_c_and_continued_sends_the_same_bytes() {
    let dir = scratch("ethernet-held");
    let prom = prom_16(&dir);
    let p = prom.to_str().unwrap();
    for engine in ["--micro", "--rtl"] {
        let (a, ta) = script_run(&dir, engine, p, false);
        let (b, tb) = script_run(&dir, engine, p, true);
        assert_eq!(ta.len(), 4, "{engine}: the four frames traced: {ta:?}");
        assert!(a == b, "{engine}: the held run's bytes differ");
        assert_eq!(ta, tb, "{engine}: the held run's trace differs");
    }
}

/// A revision-16 `micro` checkpoint whose device is enabled, with a buffer
/// of 1,536 bytes given and an ARP Ethernet frame and one of type 86DD
/// posted, made from a short run of the looping PROM with `--ethernet-mac
/// 02:00:00:00:00:07`.
fn enabled_checkpoint(dir: &std::path::Path, prom: &std::path::Path) -> std::path::PathBuf {
    use muir::engine::Engine;
    let base = dir.join("base.chk");
    let out = quux()
        .env("MUIR_QUUX_REVISION", "16")
        .args(["--micro", "--main-memory-size", "2MW", "--stop-after", "10", "--prom"])
        .arg(prom)
        .args(["--ethernet-mac", "02:00:00:00:00:07"])
        .arg("--checkpoint")
        .arg(&base)
        .run();
    assert!(out.status.success(), "{}", text(&out));
    let c = muir::checkpoint::read(&base).unwrap();
    let g = Machine::checkpointed_geometry_at(&c.body, c.word_bits).unwrap();
    let mut m = Machine::with_geometry(g, c.memory_boards);
    m.block_disk = Some(muir::block_disk::BlockDisk::new(muir::block_disk::BLOCK_NS));
    let mut e = muir::micro::Micro::new(m);
    e.load(&mut c.reader()).unwrap();
    let m = e.machine_mut();
    enable(m, 1, 0);
    give(m, 0);
    post(m, 0, &frame(PEER, 0x0806, 60, 4));
    post(m, 1, &frame(PEER, 0x86dd, 60, 5));
    let mut w = muir::checkpoint::Writer::new();
    e.save(&mut w);
    let path = dir.join("enabled.chk");
    muir::checkpoint::write_version(&path, "micro", c.memory_boards, c.version, &w.finish())
        .unwrap();
    path
}

/// **On a resumed machine whose device is enabled, `--ethernet-mac` is the
/// filter's address and the trace says what is sent, refused, received and
/// dropped**, in clocked mode and in plain: the ARP Ethernet frame posted
/// reaches the peer, the 86DD one is refused; of two the peer sends, the
/// one to `--ethernet-mac`'s address is taken into the buffer, and the one
/// to the default address is dropped and counted nowhere. The checkpoint
/// resumed was made under the same `--ethernet-mac`, as a resume must be,
/// and the one at the end holds the frame taken. **Fails** a MAC address that does not
/// reach the device, or a resume that loses the socket.
#[test]
fn a_resumed_device_sends_and_receives_through_the_socket_by_its_mac_address() {
    let dir = scratch("ethernet-resumed");
    let prom = prom_16(&dir);
    let chk = enabled_checkpoint(&dir, &prom);
    let mine = [0x02, 0, 0, 0, 0, 0x07];
    let to_me = frame(mine, 0x0806, 60, 8);
    for clocked in [true, false] {
        let end = dir.join("end.chk");
        let _ = std::fs::remove_file(&end);
        let (c, e) = (chk.to_str().unwrap(), end.to_str().unwrap());
        let (child, mut s) = started(
            &dir,
            if clocked { "resumed-clocked" } else { "resumed-plain" },
            &[
                "--micro",
                "--resume",
                c,
                "--ethernet-mac",
                "02:00:00:00:00:07",
                "--ethernet-trace",
                "--stop-after",
                if clocked { "20000" } else { "1000000" },
                "--checkpoint",
                e,
            ],
            clocked,
        );
        let sent = if clocked {
            let mut raw = Vec::new();
            let (_, r0) = until_reached(&mut s, &mut raw);
            let mut w = timed(r0 + 1_000, &frame(OWN, 0x0806, 60, 9));
            w.extend(timed(r0 + 2_000, &to_me));
            w.extend(run_until(r0 + 60_000));
            s.write_all(&w).unwrap();
            let (frames, _) = until_reached(&mut s, &mut raw);
            frames.into_iter().map(|(_, f)| f).collect::<Vec<_>>()
        } else {
            let mut w = plain(&frame(OWN, 0x0806, 60, 9));
            w.extend(plain(&to_me));
            s.write_all(&w).unwrap();
            vec![read_plain(&mut s)]
        };
        assert_eq!(sent, vec![frame(PEER, 0x0806, 60, 4)], "clocked {clocked}: the ARP one only");
        // A plain run is 8.5 ms of machine time, long enough for the two
        // frames written at its start to arrive while it runs.
        drop(s);
        let out = child.wait();
        let t = text(&out);
        assert!(out.status.success(), "clocked {clocked}:\n{t}");
        for what in [
            "ethernet: MAC address 02:00:00:00:00:07, IPv4 address 169.254.100.2/30, gateway 169.254.100.1; socket ",
            "sent 60 bytes, 02:00:00:00:00:02 from 02:00:00:00:00:02, type 0806",
            "transmit entry 1 refused: type not IPv4 or ARP",
            "received 60 bytes, 02:00:00:00:00:01 from 02:00:00:00:00:02, type 0806, dropped: not this device's MAC address",
            "received 60 bytes, 02:00:00:00:00:07 from 02:00:00:00:00:02, type 0806, into entry 0",
        ] {
            assert!(t.contains(what), "clocked {clocked}: {what:?}:\n{t}");
        }
        let c = muir::checkpoint::read(&end).unwrap();
        let g = Machine::checkpointed_geometry_at(&c.body, c.word_bits).unwrap();
        let mut m = Machine::with_geometry(g, c.memory_boards);
        m.block_disk = Some(muir::block_disk::BlockDisk::new(muir::block_disk::BLOCK_NS));
        let mut e = muir::micro::Micro::new(m);
        muir::engine::Engine::load(&mut e, &mut c.reader()).unwrap();
        let m = muir::engine::Engine::machine_mut(&mut e);
        assert_eq!(get(m, eth::RX_CONS), 1, "clocked {clocked}: one taken");
        let status = get(m, eth::STATUS);
        assert_eq!(status >> 4 & 0xf, 0, "clocked {clocked}: no type drop");
        assert_eq!(status >> 9 & 0x7f, 0, "clocked {clocked}: no IPv4 drop");
        assert_eq!(status >> 16 & 0xff, 0, "clocked {clocked}: no receive drop");
        assert_eq!(get(m, eth::STATUS) >> 24, 1, "clocked {clocked}: one refused");
        assert_eq!(received(m, 0, 0), to_me, "clocked {clocked}");
    }
}

// --- the addresses and the guard ------------------------------------------

/// A revision-16 checkpoint's words 253-257, as a resume reads them: the
/// machine's part of the body, which either engine's starts with.
fn host_words(chk: &std::path::Path) -> [u32; 5] {
    let c = muir::checkpoint::read(chk).unwrap();
    let g = Machine::checkpointed_geometry_at(&c.body, c.word_bits).unwrap();
    let mut m = Machine::with_geometry(g, c.memory_boards);
    m.block_disk = Some(muir::block_disk::BlockDisk::new(muir::block_disk::BLOCK_NS));
    m.load(&mut c.reader()).unwrap();
    [eth::MAC_HIGH, eth::MAC_LOW, eth::IP_ADDRESS, eth::NETMASK, eth::GATEWAY]
        .map(|w| get(&mut m, w))
}

/// A short revision-16 run on `engine` with `args`: of the looping PROM
/// at `prom`, or with none for a resume, which takes the checkpoint's.
fn run_16(engine: &str, prom: Option<&std::path::Path>, args: &[&str]) -> std::process::Output {
    let mut c = quux();
    c.env("MUIR_QUUX_REVISION", "16").args([
        engine,
        "--main-memory-size",
        "2MW",
        "--stop-after",
        "10",
    ]);
    if let Some(p) = prom {
        c.arg("--prom").arg(p);
    }
    c.args(args).run()
}

/// **`--ip-address` and `--ip-gateway` are words 255-257, and the defaults
/// are QUUX's side of `quux0`** (§2.2; §3, "Flags"): with no flag 255-257
/// hold 169.254.100.2, the /30's netmask and 169.254.100.1, and the start
/// says so; with the flags, on both engines, the checkpoint's words are
/// theirs, the MAC address's beside them. **Fails** a default that is not
/// quux0's, or a flag that does not reach the words.
#[test]
fn the_address_flags_are_words_255_to_257_and_default_to_quux0_s() {
    let dir = scratch("ethernet-addresses");
    let prom = prom_16(&dir);
    let chk = dir.join("a.chk");
    let c = chk.to_str().unwrap();
    let out = run_16("--micro", Some(&prom), &["--checkpoint", c]);
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(
        t.contains("ethernet: MAC address 02:00:00:00:00:01, IPv4 address 169.254.100.2/30, gateway 169.254.100.1; "),
        "{t}"
    );
    assert_eq!(host_words(&chk), [0x0200, 0x0000_0001, 0xa9fe_6402, 0xffff_fffc, 0xa9fe_6401]);
    for engine in ["--micro", "--rtl"] {
        let out = run_16(
            engine,
            Some(&prom),
            &[
                "--ethernet-mac",
                "02:00:00:00:00:07",
                "--ip-address",
                "192.0.2.6/29",
                "--ip-gateway",
                "192.0.2.1",
                "--checkpoint",
                c,
            ],
        );
        let t = text(&out);
        assert!(out.status.success(), "{engine}: {t}");
        assert!(
            t.contains("ethernet: MAC address 02:00:00:00:00:07, IPv4 address 192.0.2.6/29, gateway 192.0.2.1; "),
            "{engine}: {t}"
        );
        assert_eq!(
            host_words(&chk),
            [0x0200, 0x0000_0007, 0xc000_0206, 0xffff_fff8, 0xc000_0201],
            "{engine}"
        );
    }
}

/// **The address flags' values are checked before the run** (§3,
/// "Flags"). `--ip-address` wants a host's dotted address and a prefix
/// length from 1 to 30: none, or 0, 31, 32 and up, are refused, as are the
/// subnet's network and broadcast addresses, and addresses no host has
/// (RFC 1122 §3.2.1.3: this network, loopback, multicast and reserved).
/// There is always a gateway: `--ip-gateway` is refused when it is
/// 0.0.0.0, QUUX's own address, off `--ip-address`'s subnet (RFC 1122
/// §3.3.1), or the subnet's network or broadcast address, and the default
/// gateway is held to the same rule when `--ip-address` moves. **Fails** a
/// gateway check that takes an off-subnet gateway, or a bad address taken.
#[test]
fn the_address_flags_values_are_checked_before_the_run() {
    let dir = scratch("ethernet-address-values");
    let prom = prom_16(&dir);
    let run = |args: &[&str]| run_16("--micro", Some(&prom), args);
    let wants_address = "--ip-address wants A.B.C.D/N";
    for bad in [
        "192.0.2.6",
        "192.0.2.6/",
        "192.0.2.6/x",
        "192.0.2/30",
        "192.0.2.256/30",
        "192.0.02.6/30",
        "192.0.2.6/30/30",
        " 192.0.2.6/30",
    ] {
        refused(&run(&["--ip-address", bad]), wants_address);
    }
    refused(&run(&["--ip-address"]), wants_address);
    for n in ["0", "31", "32", "33"] {
        refused(
            &run(&["--ip-address", &format!("192.0.2.6/{n}")]),
            &format!("--ip-address 192.0.2.6/{n}: a prefix length of {n}; "),
        );
    }
    refused(&run(&["--ip-address", "192.0.2.4/30"]), "the subnet's network address");
    refused(&run(&["--ip-address", "192.0.2.7/30"]), "the subnet's broadcast address");
    for (bad, what) in [
        ("0.0.2.6/30", "this network"),
        ("127.0.2.6/30", "loopback"),
        ("224.0.2.6/30", "multicast"),
        ("240.0.2.6/30", "reserved"),
    ] {
        refused(&run(&["--ip-address", bad, "--ip-gateway", "192.0.2.5"]), what);
    }
    let wants_gateway = "--ip-gateway wants A.B.C.D";
    for bad in ["192.0.2.5/30", "192.0.2", "192.0.2.5.1", "x"] {
        refused(&run(&["--ip-gateway", bad]), wants_gateway);
    }
    refused(&run(&["--ip-gateway"]), wants_gateway);
    refused(&run(&["--ip-gateway", "0.0.0.0"]), "--ip-gateway 0.0.0.0: there is always a gateway");
    refused(
        &run(&["--ip-gateway", "169.254.100.2"]),
        "--ip-gateway 169.254.100.2 is QUUX's own address",
    );
    refused(
        &run(&["--ip-gateway", "192.0.2.5"]),
        "--ip-gateway 192.0.2.5 is off --ip-address 169.254.100.2/30's subnet",
    );
    refused(
        &run(&["--ip-address", "192.0.2.6/30", "--ip-gateway", "192.0.2.9"]),
        "--ip-gateway 192.0.2.9 is off --ip-address 192.0.2.6/30's subnet",
    );
    refused(
        &run(&["--ip-address", "192.0.2.6/30"]),
        "--ip-gateway 169.254.100.1, its default, is off --ip-address 192.0.2.6/30's subnet",
    );
    refused(
        &run(&["--ip-address", "192.0.2.6/29", "--ip-gateway", "192.0.2.0"]),
        "--ip-gateway 192.0.2.0 is the subnet's network address",
    );
    refused(
        &run(&["--ip-address", "192.0.2.6/29", "--ip-gateway", "192.0.2.7"]),
        "--ip-gateway 192.0.2.7 is the subnet's broadcast address",
    );
    for good in [
        &["--ip-address", "192.0.2.6/30", "--ip-gateway", "192.0.2.5"][..],
        &["--ip-address", "192.0.2.5/30", "--ip-gateway", "192.0.2.6"],
        &["--ip-address", "192.0.2.200/24", "--ip-gateway", "192.0.2.1"],
        &["--ip-gateway", "169.254.100.1", "--ip-address", "169.254.100.2/30"],
        &["--ip-address", "169.254.101.2/30", "--ip-gateway", "169.254.101.1"],
    ] {
        let out = run(good);
        assert!(out.status.success(), "{good:?}: {}", text(&out));
    }
}

/// Flags and their values, as arguments.
fn given<'a>(f: &[(&'a str, &'a str)]) -> Vec<&'a str> {
    f.iter().flat_map(|&(k, v)| [k, v]).collect()
}

/// **A resume keeps the checkpoint's addresses and refuses others by the
/// flag's name** (§2.9; §3, "The two guards"), as `--rtc`'s: a revision-16
/// checkpoint resumed under its own `--ethernet-mac`, `--ip-address` and
/// `--ip-gateway` runs on and writes them again; under another value of
/// any one, given or by default, it is refused naming that flag and only
/// that flag. **Fails** a resume check that misses one flag, or that
/// takes the run's flags over the checkpoint's.
#[test]
fn a_resume_under_other_addresses_is_refused_by_the_flag_s_name() {
    let dir = scratch("ethernet-resume-guard");
    let prom = prom_16(&dir);
    let flags = [
        ("--ethernet-mac", "02:00:00:00:00:07"),
        ("--ip-address", "192.0.2.6/29"),
        ("--ip-gateway", "192.0.2.1"),
    ];
    let chk = dir.join("own.chk");
    let (c, again) = (chk.to_str().unwrap(), dir.join("again.chk"));
    let mut args = given(&flags);
    args.extend(["--checkpoint", c]);
    let out = run_16("--micro", Some(&prom), &args);
    assert!(out.status.success(), "{}", text(&out));
    let words = host_words(&chk);
    assert_eq!(words, [0x0200, 0x0000_0007, 0xc000_0206, 0xffff_fff8, 0xc000_0201]);
    for engine in ["--micro", "--rtl"] {
        let mut args = given(&flags);
        let own = dir.join(format!("own{engine}.chk"));
        let o = own.to_str().unwrap();
        let out = run_16(engine, Some(&prom), &[&args[..], &["--checkpoint", o]].concat());
        assert!(out.status.success(), "{engine}: {}", text(&out));
        args.extend(["--resume", o, "--checkpoint", again.to_str().unwrap()]);
        let out = run_16(engine, None, &args);
        assert!(out.status.success(), "{engine}: its own resumes: {}", text(&out));
        assert_eq!(host_words(&again), words, "{engine}: kept through the resume");
    }
    // Each flag in turn another, given or by default, the others the
    // checkpoint's.
    let others: [(usize, &str, &str); 6] = [
        (0, "02:00:00:00:00:08", "02:00:00:00:00:07, and here 02:00:00:00:00:08"),
        (0, "", "02:00:00:00:00:07, and here 02:00:00:00:00:01"),
        (1, "192.0.2.6/28", "192.0.2.6/29, and here 192.0.2.6/28"),
        (1, "192.0.2.5/29", "192.0.2.6/29, and here 192.0.2.5/29"),
        (2, "192.0.2.2", "192.0.2.1, and here 192.0.2.2"),
        (2, "192.0.2.3", "192.0.2.1, and here 192.0.2.3"),
    ];
    for (k, value, says) in others {
        let mut f = flags.to_vec();
        if value.is_empty() {
            f.remove(k);
        } else {
            f[k].1 = value;
        }
        let mut args = given(&f);
        args.extend(["--resume", c]);
        let out = run_16("--micro", None, &args);
        let t = text(&out);
        let flag = flags[k].0;
        refused(&out, &format!("--resume {c}: the checkpoint's {flag} is {says}"));
        for (j, (other, _)) in flags.iter().enumerate() {
            if j != k {
                assert!(!t.contains(&format!("the checkpoint's {other}")), "{flag}: {other}:\n{t}");
            }
        }
    }
    // The defaults' checkpoint under every flag moved, and each named.
    let base = dir.join("defaults.chk");
    let b = base.to_str().unwrap();
    let out = run_16("--micro", Some(&prom), &["--checkpoint", b]);
    assert!(out.status.success(), "{}", text(&out));
    let mut args = given(&flags);
    args.extend(["--resume", b]);
    let out = run_16("--micro", None, &args);
    let t = text(&out);
    for says in [
        "the checkpoint's --ethernet-mac is 02:00:00:00:00:01, and here 02:00:00:00:00:07",
        "the checkpoint's --ip-address is 169.254.100.2/30, and here 192.0.2.6/29",
        "the checkpoint's --ip-gateway is 169.254.100.1, and here 192.0.2.1",
    ] {
        refused(&out, says);
        assert!(t.contains(says), "{t}");
    }
}
