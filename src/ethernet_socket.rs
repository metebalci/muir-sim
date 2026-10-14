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
//! not taken, dropping what arrives beyond them.
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
use std::io::{BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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

struct Plain {
    out: UnixStream,
    inbox: Arc<Inbox>,
    say: Say,
}

impl Plain {
    fn new(stream: UnixStream, say: Say) -> std::io::Result<Plain> {
        let mut input = stream.try_clone()?;
        let inbox = Arc::new(Inbox {
            frames: Mutex::new(VecDeque::new()),
            count: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            full: AtomicBool::new(false),
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
        Ok(Plain { out: stream, inbox, say })
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

    fn sent(&mut self, _at: u64, frame: Vec<u8>) {
        if self.inbox.closed.load(Ordering::SeqCst) {
            return;
        }
        let mut r = Vec::with_capacity(4 + frame.len());
        r.extend_from_slice(&(frame.len() as u32).to_be_bytes());
        r.extend_from_slice(&frame);
        if let Err(e) = self.out.write_all(&r) {
            self.inbox.close(&self.say, &failed(&e));
        }
    }

    fn every_edge(&self) -> bool {
        true
    }
}

impl Drop for Plain {
    /// The reading thread ends with the socket, saying nothing.
    fn drop(&mut self) {
        self.inbox.closed.store(true, Ordering::SeqCst);
        let _ = self.out.shutdown(std::net::Shutdown::Both);
    }
}

// --- clocked mode --------------------------------------------------------

struct Clocked {
    input: BufReader<UnixStream>,
    out: UnixStream,
    /// The records to send at the next "reached".
    pending: Vec<u8>,
    /// The peer's Ethernet frames not yet delivered, by their times in the
    /// machine's units, then in the order they came.
    queue: VecDeque<(u64, Vec<u8>)>,
    /// The time muir may run to, in the machine's units.
    limit: u64,
    per_ns: u64,
    closed: bool,
    say: Say,
}

impl Clocked {
    fn new(stream: UnixStream, per_ns: u64, say: Say) -> std::io::Result<Clocked> {
        Ok(Clocked {
            input: BufReader::new(stream.try_clone()?),
            out: stream,
            pending: Vec::new(),
            queue: VecDeque::new(),
            limit: 0,
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

    /// The socket ended: said, and nothing more delivered.
    fn close(&mut self, why: &str) {
        self.closed = true;
        self.queue.clear();
        self.pending.clear();
        let _ = self.out.shutdown(std::net::Shutdown::Both);
        (self.say)(&ended(why));
    }

    /// One record from the peer: its time in the machine's units and its
    /// Ethernet frame, empty for "run until".
    fn read(&mut self) -> Result<(u64, Vec<u8>), String> {
        let mut head = [0u8; 12];
        self.input.read_exact(&mut head).map_err(|e| failed(&e))?;
        let n = length(head[..4].try_into().unwrap())?;
        let t = u64::from_be_bytes(head[4..].try_into().unwrap());
        let mut f = vec![0u8; n];
        self.input.read_exact(&mut f).map_err(|e| failed(&e))?;
        Ok((t.saturating_mul(self.per_ns), f))
    }
}

impl Peer for Clocked {
    fn arrival(&mut self, now: u64) -> Option<u64> {
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

    fn edge(&mut self, now: u64) {
        while !self.closed && now >= self.limit {
            self.record(now, &[]);
            let sent = self.out.write_all(&self.pending);
            self.pending.clear();
            if let Err(e) = sent {
                return self.close(&failed(&e));
            }
            loop {
                match self.read() {
                    Ok((t, f)) if f.is_empty() => {
                        self.limit = t;
                        break;
                    }
                    Ok((t, f)) => {
                        let k =
                            self.queue.iter().position(|(u, _)| *u > t).unwrap_or(self.queue.len());
                        self.queue.insert(k, (t, f));
                    }
                    Err(why) => return self.close(&why),
                }
            }
        }
    }
}
