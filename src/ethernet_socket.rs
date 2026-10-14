// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **How muir reaches a network** (QUUX's Ethernet contract, §3): the
//! Ethernet device's host side ([`crate::ethernet_device::Peer`]) over a
//! UNIX stream socket, `--ethernet-socket PATH[,clocked]`. muir connects
//! as a client; what listens is the peer.
//!
//! **Plain mode.** Each Ethernet frame, either way, is a record: its length
//! in bytes, 32 bits big-endian, and its bytes. That is QEMU's `-netdev
//! stream` format. A record of length 0 carries nothing and is skipped. A
//! received Ethernet frame is delivered, or dropped, at the first edge
//! after it arrives (§2.8), so its time is the host's: a thread reads the
//! socket, and holds at most [`WAITING`] Ethernet frames the machine has
//! not taken, dropping what arrives beyond them. Another writes what the
//! device sends, in order, as the socket takes it, and holds at most
//! [`WAITING`] the socket has not taken, dropping what is sent beyond
//! them, so that a peer that stops reading never stalls the machine. Each
//! hold says once that it is full, until it is taken from; a frame dropped
//! from either is counted nowhere in the device. At the run's end what is
//! still held is written if the socket takes it within a second, and what
//! it does not take is said.
//!
//! **Clocked mode.** Every record carries machine time:
//!
//! ```text
//! length   u32, big-endian: the Ethernet frame's bytes, 0 to 65,535
//! time     u64, big-endian: machine time in ns
//! frame    `length` bytes
//! ```
//!
//! A record of length 0 is a control record: from the peer "run until
//! `time`", from muir "reached `time`". muir reads the socket only while it
//! waits, so the band's time and every delivery are independent of the
//! host's scheduling, and a script rerun on the same band gives the same
//! records bit for bit.
//!
//! **muir never waits deaf.** The wait wakes every [`WAKE`] of the host's
//! time, and at a signal, without a "run until": between two microcycles
//! the run loop then looks at ^C, the prompt and its stops before it waits
//! again ([`Peer::ready`]), and the microcycles and every record are those
//! of an unbroken wait. A wait the pipeline's clocks reach inside a
//! microcycle goes on until a ^C: the microcycle then ends without the
//! peer, and the run loop takes the ^C at once after it. Held there and
//! continued, the run waits for the peer again, what the peer sent for
//! the rest of that microcycle delivered at the next edge.
//!
//! - muir runs until the time the peer last named, 0 at the start. At the
//!   first edge at or after it --- the first of the run, at the start ---
//!   it sends the Ethernet frames it sent since its last "reached", each
//!   with the time it was sent, in order; then "reached" with the edge's
//!   time; then it reads records until a "run until".
//! - An Ethernet frame record from the peer is delivered at the first
//!   clock at or after its time, never earlier than the time already
//!   reached; records of one time are delivered in the order they came.
//! - A "run until" a time already reached is answered at once with
//!   "reached" and the same time.
//! - Times are whole nanoseconds. The machine's are units of 0.5 ns on
//!   revision 16, so a time muir sends is its own rounded down to the
//!   nanosecond, and one it reads is exact.
//!
//! **The peer closing**, or a record longer than 65,535 bytes, ends the
//! socket, in either mode: muir says so once and runs on as without the
//! flag, its transmitted Ethernet frames dropped and none arriving. A
//! clocked run no longer waits.

use crate::ethernet_device::{Peer, Say};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// The longest record muir takes: a longer one is not an Ethernet frame,
/// and ends the socket.
pub const MAX_RECORD: usize = 65_535;

/// The Ethernet frames a plain socket holds that the machine has not taken.
pub const WAITING: usize = 1_024;

/// `--ethernet-socket`'s mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The host's time: QEMU's `-netdev stream` records.
    Plain,
    /// The machine's time: records with times, and "run until" and
    /// "reached".
    Clocked,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Plain => "plain",
            Mode::Clocked => "clocked",
        }
    }
}

/// `--ethernet-socket PATH[,clocked]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Socket {
    pub path: PathBuf,
    pub mode: Mode,
}

/// `--ethernet-socket`'s word: a path, with `,clocked` after it for
/// clocked mode. A path may hold a comma; only a last `,clocked` is the
/// mode.
pub fn parse_socket(arg: &str) -> Result<Socket, String> {
    let (path, mode) = match arg.strip_suffix(",clocked") {
        Some(path) => (path, Mode::Clocked),
        None => (arg, Mode::Plain),
    };
    if path.is_empty() {
        return Err("--ethernet-socket wants PATH[,clocked], the path of a UNIX stream socket that a peer listens at".into());
    }
    Ok(Socket { path: PathBuf::from(path), mode })
}

/// `--ethernet-mac`'s word: six pairs of hex digits by colons, in either
/// case. A group address, `<0>` of its first byte set, is refused: a
/// station's own address is never one (IEEE 802).
pub fn parse_mac(arg: &str) -> Result<[u8; 6], String> {
    let wants =
        || format!("--ethernet-mac wants XX:XX:XX:XX:XX:XX, six pairs of hex digits: {arg:?}");
    let parts: Vec<&str> = arg.split(':').collect();
    if parts.len() != 6 {
        return Err(wants());
    }
    let mut mac = [0u8; 6];
    for (b, p) in mac.iter_mut().zip(parts) {
        if p.len() != 2 || !p.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(wants());
        }
        *b = u8::from_str_radix(p, 16).map_err(|_| wants())?;
    }
    if mac[0] & 1 != 0 {
        return Err(format!(
            "--ethernet-mac {arg}: a group address, <0> of its first byte set; the device's own address is a station's"
        ));
    }
    Ok(mac)
}

/// A MAC address as the flag writes it.
pub fn mac_text(mac: [u8; 6]) -> String {
    mac.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

/// The peer at `path`, connected, in `mode`. `per_ns` is the machine's
/// units a nanosecond; `say` takes the socket's notices.
pub fn connect(path: &Path, mode: Mode, per_ns: u64, say: Say) -> std::io::Result<Box<dyn Peer>> {
    over(UnixStream::connect(path)?, mode, per_ns, say)
}

/// The peer at the other end of `stream`, in `mode`.
pub fn over(
    stream: UnixStream,
    mode: Mode,
    per_ns: u64,
    say: Say,
) -> std::io::Result<Box<dyn Peer>> {
    Ok(match mode {
        Mode::Plain => Box::new(Plain::new(stream, say)?),
        Mode::Clocked => Box::new(Clocked::new(stream, per_ns, say)?),
    })
}

/// What the socket's end is said as: the peer's close, or why muir ended
/// it.
fn ended(why: &str) -> String {
    format!("ethernet: {why}; from here on transmitted Ethernet frames are dropped and none arrive")
}

/// Why a read or a write ended the socket.
fn failed(e: &std::io::Error) -> String {
    use std::io::ErrorKind::*;
    match e.kind() {
        UnexpectedEof | BrokenPipe | ConnectionReset => "the peer closed the socket".into(),
        _ => format!("the socket failed: {e}"),
    }
}

/// A record's length, read.
fn length(head: [u8; 4]) -> Result<usize, String> {
    let n = u32::from_be_bytes(head) as usize;
    if n > MAX_RECORD {
        Err(format!(
            "the peer sent a record of {n} bytes, longer than {MAX_RECORD}; muir closed the socket"
        ))
    } else {
        Ok(n)
    }
}

// --- plain mode ----------------------------------------------------------

/// What the reading thread hands the machine.
struct Inbox {
    frames: Mutex<VecDeque<Vec<u8>>>,
    /// `frames`' length, read at every edge without the lock.
    count: AtomicUsize,
    closed: AtomicBool,
    /// [`WAITING`] reached, and said, since the machine last took one.
    full: AtomicBool,
}

impl Inbox {
    /// The socket ended, said once.
    fn close(&self, say: &Say, why: &str) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            say(&ended(why));
        }
    }
}

/// What the machine hands the writing thread: its sent Ethernet frames,
/// as records, until the socket takes them.
struct Outbox {
    held: Mutex<Held>,
    /// Records to write, or the hold emptied.
    moved: Condvar,
}

struct Held {
    records: VecDeque<Vec<u8>>,
    /// Records the writing thread has taken and the socket has not.
    writing: usize,
    /// [`WAITING`] reached, and said, since the socket last took some.
    full: bool,
    /// The machine is gone: what is held is written, and the thread ends.
    ending: bool,
}

impl Held {
    fn count(&self) -> usize {
        self.records.len() + self.writing
    }
}

/// How long the end of a plain run gives the socket to take what muir
/// still holds for it.
const LAST_WRITES: Duration = Duration::from_secs(1);

struct Plain {
    out: UnixStream,
    inbox: Arc<Inbox>,
    outbox: Arc<Outbox>,
    say: Say,
}

impl Plain {
    fn new(stream: UnixStream, say: Say) -> std::io::Result<Plain> {
        let mut input = stream.try_clone()?;
        let mut output = stream.try_clone()?;
        let inbox = Arc::new(Inbox {
            frames: Mutex::new(VecDeque::new()),
            count: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            full: AtomicBool::new(false),
        });
        let outbox = Arc::new(Outbox {
            held: Mutex::new(Held {
                records: VecDeque::new(),
                writing: 0,
                full: false,
                ending: false,
            }),
            moved: Condvar::new(),
        });
        let (from, said) = (inbox.clone(), say.clone());
        std::thread::Builder::new().name("ethernet-socket".into()).spawn(move || {
            let why = loop {
                let mut head = [0u8; 4];
                if let Err(e) = input.read_exact(&mut head) {
                    break failed(&e);
                }
                let n = match length(head) {
                    Ok(n) => n,
                    Err(why) => {
                        let _ = input.shutdown(std::net::Shutdown::Both);
                        break why;
                    }
                };
                let mut f = vec![0u8; n];
                if let Err(e) = input.read_exact(&mut f) {
                    break failed(&e);
                }
                if n == 0 || from.closed.load(Ordering::SeqCst) {
                    continue;
                }
                let mut q = from.frames.lock().unwrap();
                if q.len() < WAITING {
                    q.push_back(f);
                    from.count.store(q.len(), Ordering::Release);
                } else if !from.full.swap(true, Ordering::SeqCst) {
                    said(&format!(
                        "ethernet: {WAITING} Ethernet frames wait for the machine; what arrives is dropped until it takes them"
                    ));
                }
            };
            from.close(&said, &why);
        })?;
        let (to, closing, said) = (outbox.clone(), inbox.clone(), say.clone());
        std::thread::Builder::new().name("ethernet-socket-out".into()).spawn(move || {
            loop {
                let batch = {
                    let mut h = to.held.lock().unwrap();
                    while h.records.is_empty() && !h.ending {
                        h = to.moved.wait(h).unwrap();
                    }
                    if h.records.is_empty() {
                        return;
                    }
                    h.writing = h.records.len();
                    h.records.drain(..).collect::<Vec<_>>().concat()
                };
                let wrote = output.write_all(&batch);
                let mut h = to.held.lock().unwrap();
                h.writing = 0;
                h.full = false;
                if wrote.is_err() {
                    h.records.clear();
                }
                to.moved.notify_all();
                drop(h);
                if let Err(e) = wrote {
                    closing.close(&said, &failed(&e));
                    return;
                }
            }
        })?;
        Ok(Plain { out: stream, inbox, outbox, say })
    }
}

impl Peer for Plain {
    fn arrival(&mut self, now: u64) -> Option<u64> {
        (self.inbox.count.load(Ordering::Acquire) > 0).then_some(now)
    }

    fn take(&mut self) -> Vec<u8> {
        let mut q = self.inbox.frames.lock().unwrap();
        let f = q.pop_front().unwrap_or_default();
        self.inbox.count.store(q.len(), Ordering::Release);
        self.inbox.full.store(false, Ordering::SeqCst);
        f
    }

    /// Held for the writing thread, at most [`WAITING`], so that a peer
    /// that stops reading never stalls the machine.
    fn sent(&mut self, _at: u64, frame: Vec<u8>) {
        if self.inbox.closed.load(Ordering::SeqCst) {
            return;
        }
        let mut r = Vec::with_capacity(4 + frame.len());
        r.extend_from_slice(&(frame.len() as u32).to_be_bytes());
        r.extend_from_slice(&frame);
        let mut h = self.outbox.held.lock().unwrap();
        if h.count() < WAITING {
            h.records.push_back(r);
            self.outbox.moved.notify_all();
        } else if !h.full {
            h.full = true;
            drop(h);
            (self.say)(&format!(
                "ethernet: {WAITING} Ethernet frames wait for the peer; what the device sends is dropped until it takes them"
            ));
        }
    }

    fn every_edge(&self) -> bool {
        true
    }
}

impl Drop for Plain {
    /// What muir holds is written, if the socket takes it within
    /// [`LAST_WRITES`], and what it does not is said; then both threads end
    /// with the socket, saying nothing more.
    fn drop(&mut self) {
        let mut h = self.outbox.held.lock().unwrap();
        h.ending = true;
        self.outbox.moved.notify_all();
        let until = Instant::now() + LAST_WRITES;
        while h.count() > 0 && !self.inbox.closed.load(Ordering::SeqCst) {
            let Some(left) = until.checked_duration_since(Instant::now()) else { break };
            h = self.outbox.moved.wait_timeout(h, left).unwrap().0;
        }
        let left = h.count();
        drop(h);
        if left > 0 && !self.inbox.closed.swap(true, Ordering::SeqCst) {
            (self.say)(&format!(
                "ethernet: {left} Ethernet frames the peer had not taken are dropped as the run ends"
            ));
        }
        self.inbox.closed.store(true, Ordering::SeqCst);
        let _ = self.out.shutdown(std::net::Shutdown::Both);
    }
}

// --- clocked mode --------------------------------------------------------

/// How often a clocked wait wakes, without a "run until", for the run loop
/// to look at ^C, the prompt and its stops ([`Peer::ready`]).
pub const WAKE: Duration = Duration::from_millis(100);

struct Clocked {
    /// Read with a timeout of [`WAKE`]; written as the machine reaches.
    stream: UnixStream,
    /// What has been read and is not yet a whole record.
    input: Vec<u8>,
    /// The records to send at the next "reached".
    pending: Vec<u8>,
    /// The peer's Ethernet frames not yet delivered, by their times in the
    /// machine's units, then in the order they came.
    queue: VecDeque<(u64, Vec<u8>)>,
    /// The time muir may run to, in the machine's units.
    limit: u64,
    /// "Reached" sent, at this time, and "run until" not yet read; and the
    /// ^Cs counted then ([`crate::cli::interrupts_counted`]).
    waits: Option<(u64, u32)>,
    per_ns: u64,
    closed: bool,
    say: Say,
}

impl Clocked {
    fn new(stream: UnixStream, per_ns: u64, say: Say) -> std::io::Result<Clocked> {
        stream.set_read_timeout(Some(WAKE))?;
        Ok(Clocked {
            stream,
            input: Vec::new(),
            pending: Vec::new(),
            queue: VecDeque::new(),
            limit: 0,
            waits: None,
            per_ns,
            closed: false,
            say,
        })
    }

    fn record(&mut self, units: u64, frame: &[u8]) {
        self.pending.extend_from_slice(&(frame.len() as u32).to_be_bytes());
        self.pending.extend_from_slice(&(units / self.per_ns).to_be_bytes());
        self.pending.extend_from_slice(frame);
    }

    /// The socket ended: said, and nothing more delivered or waited for.
    fn close(&mut self, why: &str) {
        self.closed = true;
        self.waits = None;
        self.queue.clear();
        self.pending.clear();
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        (self.say)(&ended(why));
    }

    /// What was sent since the last "reached", and "reached" at `now`.
    fn reached(&mut self, now: u64) {
        self.record(now, &[]);
        let sent = self.stream.write_all(&self.pending);
        self.pending.clear();
        match sent {
            Ok(()) => self.waits = Some((now, crate::cli::interrupts_counted())),
            Err(e) => self.close(&failed(&e)),
        }
    }

    /// One record from the peer: its time in the machine's units and its
    /// Ethernet frame, empty for "run until". `None` when the socket had
    /// none whole within [`WAKE`], or a signal came; a record cut by either
    /// is kept and finished by the next read.
    fn read(&mut self) -> Result<Option<(u64, Vec<u8>)>, String> {
        use std::io::ErrorKind::*;
        loop {
            if self.input.len() >= 4 {
                let n = length(self.input[..4].try_into().unwrap())?;
                if self.input.len() >= 12 + n {
                    let t = u64::from_be_bytes(self.input[4..12].try_into().unwrap());
                    let f = self.input[12..12 + n].to_vec();
                    self.input.drain(..12 + n);
                    return Ok(Some((t.saturating_mul(self.per_ns), f)));
                }
            }
            let mut chunk = [0u8; 16_384];
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(failed(&UnexpectedEof.into())),
                Ok(k) => self.input.extend_from_slice(&chunk[..k]),
                Err(e) if matches!(e.kind(), WouldBlock | TimedOut | Interrupted) => {
                    return Ok(None);
                }
                Err(e) => return Err(failed(&e)),
            }
        }
    }

    /// The wait after "reached": records read until a "run until" past the
    /// time reached. Whether it is over. Between microcycles (`!block`) it
    /// is not when [`WAKE`] passes without one, and the run loop asks
    /// again. Inside one (`block`) it goes on until a ^C has come since
    /// "reached": the microcycle then ends without the peer, and the wait
    /// is the run loop's after it, which acts on the ^C.
    fn wait(&mut self, block: bool) -> bool {
        while let Some((at, asked)) = self.waits {
            if block && crate::cli::interrupts_counted() != asked {
                return false;
            }
            match self.read() {
                Ok(None) if block => {}
                Ok(None) => return false,
                Ok(Some((t, f))) if f.is_empty() => {
                    self.limit = t;
                    self.waits = None;
                    if at >= t {
                        self.reached(at);
                    }
                }
                Ok(Some((t, f))) => {
                    let k = self.queue.iter().position(|(u, _)| *u > t).unwrap_or(self.queue.len());
                    self.queue.insert(k, (t, f));
                }
                Err(why) => self.close(&why),
            }
        }
        true
    }
}

impl Peer for Clocked {
    /// A wait still open is finished first, the machine standing: the run
    /// loop finishes it before the next microcycle ([`Peer::ready`]), and
    /// any other driver of the machine here.
    fn arrival(&mut self, now: u64) -> Option<u64> {
        if self.waits.is_some() {
            self.wait(true);
        }
        self.queue.front().map(|(t, _)| *t).filter(|&t| t <= now)
    }

    fn take(&mut self) -> Vec<u8> {
        self.queue.pop_front().map(|(_, f)| f).unwrap_or_default()
    }

    fn sent(&mut self, at: u64, frame: Vec<u8>) {
        if !self.closed {
            self.record(at, &frame);
        }
    }

    fn every_edge(&self) -> bool {
        true
    }

    fn edge(&mut self, now: u64) -> bool {
        if self.waits.is_some() {
            self.wait(true);
        }
        if self.waits.is_none() && !self.closed && now >= self.limit {
            self.reached(now);
        }
        self.waits.is_some()
    }

    fn ready(&mut self) -> bool {
        self.wait(false)
    }
}
