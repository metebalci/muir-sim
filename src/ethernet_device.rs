// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **QUUX's Ethernet device** (QUUX's Ethernet contract, §2; revision
//! 16): Ethernet frames between main memory and the host side, through a
//! transmit ring and a receive ring the processor keeps in main memory.
//! The registers are words 240-257 of the register page, its interrupt is
//! word 100 `<8>` and its feature bit is word 15 `<2>`.
//!
//! **The registers** ([`CONTROL`] to [`GATEWAY`]): 240 control, `<0>`
//! enable and `<8>` interrupt enable; 241 status, read only, `<0>` enabled,
//! `<1>` quiet, `<2>` configuration refused, `<3>` index fault, `<7:4>`
//! received Ethernet frames dropped by the type filter mod 16, `<8>` an
//! Ethernet frame waiting (251 ≠ 252), `<15:9>` received Ethernet frames
//! dropped by the IPv4 checks mod 128, `<23:16>` received Ethernet frames
//! dropped mod 256, `<31:24>` transmit entries refused mod 256; 242 and 243
//! the transmit ring's base, a physical word address on an 8-word line, and
//! the log2 of its entries, 0-8; 244 the transmit producer (the
//! processor's) and 245 the transmit consumer (the device's, read only);
//! 246 and 247 the receive ring's base and size; 250 the receive producer,
//! buffers given (the processor's); 251 the receive consumer, buffers
//! filled (the device's, read only); 252 receive taken, filled entries
//! taken back (the processor's); 253 the MAC address's bytes 0-1, byte 0 in
//! `<15:8>`, and 254 its bytes 2-5, byte 2 in `<31:24>`; 255 the IPv4
//! address, 256 the netmask and 257 the gateway, a.b.c.d as `a<<24 | b<<16
//! | c<<8 | d`, 0 for no gateway. 253-257 are the host's ([`Host`]): read
//! only, and no reset changes them. Writes of the read-only words are
//! ignored.
//!
//! **The rings.** Indexes are 16 bits and run free; an entry's slot is its
//! index mod the ring's size, and an entry is 8 words. A transmit entry is
//! word 0 the Ethernet frame's address, `<27:0>`, on a line, and word 1 its
//! length in bytes, 14 to 1514. A receive entry is word 0 the buffer's
//! address, on a line, word 1 its length in bytes, both the processor's,
//! and word 2 the received Ethernet frame's length, the device's. Bytes are
//! in the file device's order: byte k is `<8(k mod 4)+7:8(k mod 4)>` of
//! word k/4. Every word the device writes is a fixnum, tag `005`, and it
//! reads `<31:0>` of what it reads.
//!
//! **Configuration** is the file device's: 242, 243, 246 and 247 are written
//! while disabled and ignored while enabled; the enable checks them, and a
//! base off a line, a size over 8, or a ring past main memory is refused,
//! status `<2>`, the device staying disabled; while disabled the indexes
//! read 0 and writes of 244, 250 and 252 go nowhere; a write of 240 clears
//! `<2>` and `<3>`. An index write is ignored, and sets `<3>`, if it makes
//! 244 − 245 more than the transmit ring's size, or 250 − 252 more than the
//! receive ring's, takes 252 past 251, or moves an index backwards.
//!
//! **Transmit.** The device takes entries from 245 up to 244, in order, and
//! sends each Ethernet frame to the host side, advancing 245. It writes no
//! memory: the rules ([`Rules`], `crate::ethernet_offload`) fill the
//! checksums in the copy it sends. An entry whose length is outside
//! 14-1514, whose Ethernet frame is off a line or past main memory, or
//! that the rules refuse, is not sent; it is counted in status `<31:24>`,
//! and 245 passes it.
//!
//! **Receive.** The device takes an Ethernet frame addressed to its MAC
//! address or to broadcast, and drops any other with no count; then the
//! rules' type filter, counted in `<7:4>`, and their IPv4 checks, counted in
//! `<15:9>`; then a buffer: none given (250 = 251), or one shorter than the
//! Ethernet frame, drops it, counted in `<23:16>`, as does an Ethernet
//! frame longer than 1,514 bytes. A dropped Ethernet frame
//! takes no buffer and writes nothing. A taken one is written into the
//! buffer at slot 251, the bytes past its end in the last word 0 and no
//! word past it; its length goes into the entry's word 2, the cache is made
//! coherent ([`Coherence`]), and 251 advances. A buffer whose address is off
//! a line or that runs past main memory is taken as too short: the device
//! never writes outside main memory.
//!
//! **Time** (§2.8). A transmit entry is sent 20 µs ([`SEND_NS`]) after it
//! is due: after the latest of the producer write that posts it and the
//! previous entry's sending, rounded up once to the first clock at or after
//! the exact end, as the file device's commands are. A received Ethernet
//! frame is delivered at the first clock at or after it arrives, and never
//! before the time the device has been advanced to. Each happens at its
//! own instant, in order, whenever the device is next advanced. The 20 µs
//! is **unverified**: the file device's estimate of the boards' round trip,
//! until the boards measure their own.
//!
//! **Reset** (§2.7). Disable is the reset: transmit entries not yet sent
//! are dropped, nothing more is received, the indexes and the interrupt
//! enable go to 0, and the bases, the sizes and 253-257 stay.
//! `RESET-DEVICES` disables it and clears status `<2>` and `<3>`. muir's
//! device is quiet as soon as it is disabled. The four counters run free
//! from power-on.
//!
//! **The host side** is a [`Peer`], attached in the process: a test's
//! scripted peer ([`Script`]), or the transport a run's flags name. With
//! none, transmitted Ethernet frames go nowhere (245 still advances) and
//! none arrive.

use crate::clock::TimeBase;
use crate::ethernet_offload::RxCounter;
use crate::machine::MemoryWord;
use std::sync::{Arc, Mutex};

/// The registers, by their word on the register page.
pub const CONTROL: u32 = 0o240;
pub const STATUS: u32 = 0o241;
pub const TX_BASE: u32 = 0o242;
pub const TX_SIZE: u32 = 0o243;
pub const TX_PROD: u32 = 0o244;
pub const TX_CONS: u32 = 0o245;
pub const RX_BASE: u32 = 0o246;
pub const RX_SIZE: u32 = 0o247;
pub const RX_PROD: u32 = 0o250;
pub const RX_CONS: u32 = 0o251;
pub const RX_TAKEN: u32 = 0o252;
pub const MAC_HIGH: u32 = 0o253;
pub const MAC_LOW: u32 = 0o254;
pub const IP_ADDRESS: u32 = 0o255;
pub const NETMASK: u32 = 0o256;
pub const GATEWAY: u32 = 0o257;

/// The device's bit in word 100.
pub const INTERRUPT_BIT: u32 = 8;
/// The device's bit in feature word 15.
pub const FEATURE_BIT: u32 = 2;

/// A transmit entry's time, from when it is due to when it is sent.
/// **Unverified**: the file device's estimate of the boards' round trip
/// ([`crate::file_device::COMMAND_NS`]), until the boards measure it.
pub const SEND_NS: u64 = 20_000;

/// The shortest and the longest Ethernet frame a transmit entry may name.
pub const MIN_FRAME: u32 = 14;
pub const MAX_FRAME: u32 = 1514;

/// The broadcast address.
pub const BROADCAST: [u8; 6] = [0xff; 6];

/// The machine's addresses and words, as the device takes them: 28-bit
/// physical word addresses on an 8-word line, and every word it writes a
/// fixnum, tag `005` (as the file device's, `crate::file_device`).
const ADDRESS: u32 = 0o1777777777;
const LINE: u32 = 7;
const FIXNUM: u8 = 0o005;

/// What the host gives the machine in 253-257: `--ethernet-mac`,
/// `--ip-address` and `--ip-gateway`'s, read only to the machine. The run's
/// say, not a checkpoint's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Host {
    pub mac: [u8; 6],
    /// a.b.c.d as `a<<24 | b<<16 | c<<8 | d`.
    pub address: u32,
    pub netmask: u32,
    /// 0 for none.
    pub gateway: u32,
}

impl Default for Host {
    /// The defaults: `02:00:00:00:00:01`, a locally administered address,
    /// and 169.254.0.2/30 by 169.254.0.1, a link-local /30.
    fn default() -> Host {
        Host {
            mac: [0x02, 0, 0, 0, 0, 0x01],
            address: u32::from_be_bytes([169, 254, 0, 2]),
            netmask: u32::MAX << 2,
            gateway: u32::from_be_bytes([169, 254, 0, 1]),
        }
    }
}

/// A received Ethernet frame's drop by the rules: the counter it is
/// counted in, and why.
pub type RxDropped = (RxCounter, &'static str);

/// The rules of §2.4 the device applies: [`Rules::OFFLOAD`],
/// `crate::ethernet_offload`'s, unless a test puts its own.
#[derive(Clone, Copy, Debug)]
pub struct Rules {
    /// Fills an Ethernet frame about to be sent, or says why it is refused.
    pub transmit: fn(&mut [u8]) -> Result<(), &'static str>,
    /// Whether a received Ethernet frame may take a buffer.
    pub receive: fn(&[u8]) -> Result<(), RxDropped>,
}

impl Rules {
    pub const OFFLOAD: Rules = Rules {
        transmit: |f| crate::ethernet_offload::transmit(f).map_err(|r| r.reason()),
        receive: |f| crate::ethernet_offload::receive(f).map_err(|d| (d.counter(), d.reason())),
    };
}

/// **How a delivery makes the cache coherent** before 251 moves (§2.5).
/// M-refill measures the two and chooses one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Coherence {
    /// The whole-cache sweep, as for a file device completion: the default.
    #[default]
    Sweep,
    /// Line clearing, as block-disk's: the sets of the buffer's lines it
    /// wrote and of the entry's line. Behind the measurement switch, this
    /// field.
    LineClearing,
}

/// What the last advance asks of the cache, for the engine that has one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Written {
    /// A delivery under [`Coherence::Sweep`].
    pub sweep: bool,
    /// The physical word addresses of the lines written under
    /// [`Coherence::LineClearing`], one a line.
    pub lines: Vec<u32>,
}

/// **The host side of the link**: what sits behind the device. Times are
/// the machine's own ([`crate::machine::Machine::ns`]: units of 0.5 ns on
/// revision 16).
pub trait Peer: Send {
    /// When the next Ethernet frame for the device arrived, if one has by
    /// `now`. Asked whenever the device is advanced, until it says none.
    fn arrival(&mut self, now: u64) -> Option<u64>;
    /// That Ethernet frame, taken.
    fn take(&mut self) -> Vec<u8>;
    /// An Ethernet frame the device sent at `at`.
    fn sent(&mut self, at: u64, frame: Vec<u8>);
    /// Whether the device is to be advanced at every edge of an engine,
    /// enabled or not, and the peer told of each ([`Peer::edge`]): a
    /// socket's peer, whose Ethernet frames are delivered or dropped at the
    /// first edge after they arrive (§2.8), and whose clocked mode waits at
    /// an edge. A scripted peer is asked only while the device is enabled
    /// and at its register accesses.
    fn every_edge(&self) -> bool {
        false
    }
    /// The machine at an engine's edge, the device advanced to `now`: where
    /// a clocked peer waits once `now` has reached the time it may run to.
    fn edge(&mut self, _now: u64) {}
}

/// Where a line of `--ethernet-trace`, or a transport's notice, goes.
pub type Say = Arc<dyn Fn(&str) + Send + Sync>;

/// [`Say`] to stderr.
pub fn stderr() -> Say {
    Arc::new(|line: &str| eprintln!("{line}"))
}

/// **A scripted peer, in the process**: Ethernet frames to arrive at the
/// times given, or at a time after the device's next transmit, and a
/// record of what the device sent, each with its time. A clone shares the
/// script, so a test keeps one and attaches another.
#[derive(Clone, Default)]
pub struct Script(Arc<Mutex<ScriptState>>);

#[derive(Default)]
struct ScriptState {
    /// Ordered by time, then by the order given.
    arrivals: std::collections::VecDeque<(u64, Vec<u8>)>,
    /// To arrive this long after the next transmit.
    replies: Vec<(u64, Vec<u8>)>,
    sent: Vec<(u64, Vec<u8>)>,
}

impl ScriptState {
    fn arrive(&mut self, at: u64, frame: Vec<u8>) {
        let k = self.arrivals.iter().position(|(t, _)| *t > at).unwrap_or(self.arrivals.len());
        self.arrivals.insert(k, (at, frame));
    }
}

impl Script {
    /// `frame` arrives at `at`, after every frame already given for `at`
    /// or before.
    pub fn arrive(&self, at: u64, frame: Vec<u8>) {
        self.0.lock().unwrap().arrive(at, frame);
    }

    /// `frame` arrives `after` the device's next transmit is sent.
    pub fn reply(&self, after: u64, frame: Vec<u8>) {
        self.0.lock().unwrap().replies.push((after, frame));
    }

    /// What the device has sent, in order.
    pub fn sent(&self) -> Vec<(u64, Vec<u8>)> {
        self.0.lock().unwrap().sent.clone()
    }

    /// The frames still to arrive.
    pub fn waiting(&self) -> usize {
        self.0.lock().unwrap().arrivals.len()
    }
}

impl Peer for Script {
    fn arrival(&mut self, now: u64) -> Option<u64> {
        self.0.lock().unwrap().arrivals.front().map(|(t, _)| *t).filter(|&t| t <= now)
    }
    fn take(&mut self) -> Vec<u8> {
        self.0.lock().unwrap().arrivals.pop_front().map(|(_, f)| f).unwrap_or_default()
    }
    fn sent(&mut self, at: u64, frame: Vec<u8>) {
        let mut s = self.0.lock().unwrap();
        s.sent.push((at, frame));
        for (after, f) in std::mem::take(&mut s.replies) {
            s.arrive(at + after, f);
        }
    }
}

/// The Ethernet device: its registers, its rings' indexes, its counters,
/// and the host side behind it.
pub struct EthernetDevice {
    /// 253-257.
    pub host: Host,
    /// The rules of §2.4.
    pub rules: Rules,
    /// §2.5's mechanism.
    pub coherence: Coherence,
    /// What the last advance wrote, for the cache. Not in a checkpoint.
    pub written: Written,
    /// `--ethernet-trace`'s lines: each Ethernet frame sent and received,
    /// and why one is refused or dropped. Not in a checkpoint.
    pub trace: Option<Say>,
    enabled: bool,
    /// Whether an engine's edge advances the device: enabled, or a peer
    /// that asks for every edge ([`Peer::every_edge`]). One test an edge.
    edges: bool,
    /// The peer asks for every edge.
    peer_edges: bool,
    interrupt_enable: bool,
    refused: bool,
    index_fault: bool,
    tx_base: u32,
    tx_log2: u32,
    rx_base: u32,
    rx_log2: u32,
    tx_prod: u16,
    tx_cons: u16,
    rx_prod: u16,
    rx_cons: u16,
    rx_taken: u16,
    /// 241's counters, each kept to its field's width.
    type_drops: u32,
    ipv4_drops: u32,
    rx_drops: u32,
    tx_refusals: u32,
    /// When the entry at 245 is sent, once it is due.
    head_due: Option<u64>,
    /// The time the device has been advanced to. Not in a checkpoint.
    reached: u64,
    /// When the last Ethernet frame was delivered. Not in a checkpoint.
    delivered_at: Option<u64>,
    /// Not in a checkpoint, and not in a copy: a copy of a machine has
    /// nothing behind its device.
    peer: Option<Box<dyn Peer>>,
}

impl Clone for EthernetDevice {
    fn clone(&self) -> EthernetDevice {
        EthernetDevice {
            host: self.host,
            rules: self.rules,
            coherence: self.coherence,
            written: self.written.clone(),
            trace: self.trace.clone(),
            enabled: self.enabled,
            edges: self.enabled,
            peer_edges: false,
            interrupt_enable: self.interrupt_enable,
            refused: self.refused,
            index_fault: self.index_fault,
            tx_base: self.tx_base,
            tx_log2: self.tx_log2,
            rx_base: self.rx_base,
            rx_log2: self.rx_log2,
            tx_prod: self.tx_prod,
            tx_cons: self.tx_cons,
            rx_prod: self.rx_prod,
            rx_cons: self.rx_cons,
            rx_taken: self.rx_taken,
            type_drops: self.type_drops,
            ipv4_drops: self.ipv4_drops,
            rx_drops: self.rx_drops,
            tx_refusals: self.tx_refusals,
            head_due: self.head_due,
            reached: self.reached,
            delivered_at: self.delivered_at,
            peer: None,
        }
    }
}

impl std::fmt::Debug for EthernetDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthernetDevice")
            .field("enabled", &self.enabled)
            .field("tx", &(self.tx_prod, self.tx_cons))
            .field("rx", &(self.rx_prod, self.rx_cons, self.rx_taken))
            .field("head_due", &self.head_due)
            .finish_non_exhaustive()
    }
}

impl Default for EthernetDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl EthernetDevice {
    /// At power-on: disabled, the rings at 0, nothing behind it.
    pub fn new() -> EthernetDevice {
        EthernetDevice {
            host: Host::default(),
            rules: Rules::OFFLOAD,
            coherence: Coherence::default(),
            written: Written::default(),
            trace: None,
            enabled: false,
            edges: false,
            peer_edges: false,
            interrupt_enable: false,
            refused: false,
            index_fault: false,
            tx_base: 0,
            tx_log2: 0,
            rx_base: 0,
            rx_log2: 0,
            tx_prod: 0,
            tx_cons: 0,
            rx_prod: 0,
            rx_cons: 0,
            rx_taken: 0,
            type_drops: 0,
            ipv4_drops: 0,
            rx_drops: 0,
            tx_refusals: 0,
            head_due: None,
            reached: 0,
            delivered_at: None,
            peer: None,
        }
    }

    /// The host side, from now on.
    pub fn attach(&mut self, peer: Box<dyn Peer>) {
        self.peer_edges = peer.every_edge();
        self.peer = Some(peer);
        self.set_enabled(self.enabled);
    }

    /// 240 `<0>` set or cleared, and with it whether an edge advances the
    /// device.
    fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        self.edges = on || self.peer_edges;
    }

    fn tx_entries(&self) -> u16 {
        1 << self.tx_log2
    }

    fn rx_entries(&self) -> u16 {
        1 << self.rx_log2
    }

    /// Transmit entries posted and not yet sent.
    pub fn queued(&self) -> u16 {
        self.tx_prod.wrapping_sub(self.tx_cons)
    }

    /// When the entry at 245 is sent, if it is due.
    pub fn head_due(&self) -> Option<u64> {
        self.head_due
    }

    /// When the last Ethernet frame was delivered.
    pub fn last_delivery(&self) -> Option<u64> {
        self.delivered_at
    }

    /// 240 `<0>`.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Whether an engine's edge advances the device: while it is enabled,
    /// and at every edge for a peer that asks ([`Peer::every_edge`]).
    #[inline]
    pub fn at_edges(&self) -> bool {
        self.edges
    }

    /// The peer told of an engine's edge at `now`, the device advanced to
    /// it ([`Peer::edge`]).
    pub fn edge(&mut self, now: u64) {
        if let Some(p) = self.peer.as_mut() {
            p.edge(now);
        }
    }

    /// Status `<8>`: a filled entry not taken back.
    pub fn waiting(&self) -> bool {
        self.rx_cons != self.rx_taken
    }

    /// Word 100 `<8>`: an Ethernet frame waiting, under the interrupt
    /// enable. A level.
    pub fn interrupt(&self) -> bool {
        self.interrupt_enable && self.waiting()
    }

    /// A register's word, the device having been advanced to now.
    pub fn read(&self, word: u32) -> u32 {
        let on = self.enabled;
        let mac = self.host.mac;
        match word {
            CONTROL => on as u32 | (self.interrupt_enable as u32) << 8,
            STATUS => {
                on as u32
                    | (!on as u32) << 1
                    | (self.refused as u32) << 2
                    | (self.index_fault as u32) << 3
                    | (self.type_drops & 0xf) << 4
                    | (self.waiting() as u32) << 8
                    | (self.ipv4_drops & 0x7f) << 9
                    | (self.rx_drops & 0xff) << 16
                    | (self.tx_refusals & 0xff) << 24
            }
            TX_BASE => self.tx_base,
            TX_SIZE => self.tx_log2,
            RX_BASE => self.rx_base,
            RX_SIZE => self.rx_log2,
            TX_PROD if on => self.tx_prod.into(),
            TX_CONS if on => self.tx_cons.into(),
            RX_PROD if on => self.rx_prod.into(),
            RX_CONS if on => self.rx_cons.into(),
            RX_TAKEN if on => self.rx_taken.into(),
            MAC_HIGH => u32::from_be_bytes([0, 0, mac[0], mac[1]]),
            MAC_LOW => u32::from_be_bytes([mac[2], mac[3], mac[4], mac[5]]),
            IP_ADDRESS => self.host.address,
            NETMASK => self.host.netmask,
            GATEWAY => self.host.gateway,
            _ => 0,
        }
    }

    /// A register written at `now`, the device having been advanced to it;
    /// `main` is main memory, for the enable's check.
    pub fn write<W: MemoryWord>(
        &mut self,
        word: u32,
        v: u32,
        now: u64,
        time: TimeBase,
        main: &[W],
    ) {
        let on = self.enabled;
        match word {
            CONTROL => {
                self.refused = false;
                self.index_fault = false;
                let (enable, ie) = (v & 1 != 0, v & 0x100 != 0);
                match (on, enable) {
                    (false, true) => {
                        let fits = |base: u32, log2: u32| {
                            base & LINE == 0
                                && log2 <= 8
                                && base as usize + (8 << log2) <= main.len()
                        };
                        if fits(self.tx_base, self.tx_log2) && fits(self.rx_base, self.rx_log2) {
                            self.set_enabled(true);
                            self.interrupt_enable = ie;
                        } else {
                            self.refused = true;
                        }
                    }
                    (true, false) => self.disable(),
                    (true, true) => self.interrupt_enable = ie,
                    (false, false) => {}
                }
            }
            TX_BASE if !on => self.tx_base = v & ADDRESS,
            TX_SIZE if !on => self.tx_log2 = v & 0o17,
            RX_BASE if !on => self.rx_base = v & ADDRESS,
            RX_SIZE if !on => self.rx_log2 = v & 0o17,
            TX_PROD if on => {
                let new = v as u16;
                let claimed = new.wrapping_sub(self.tx_cons);
                if claimed > self.tx_entries() || claimed < self.queued() {
                    self.index_fault = true;
                } else {
                    self.tx_prod = new;
                    self.take(now, time);
                }
            }
            RX_PROD if on => {
                let new = v as u16;
                let given = new.wrapping_sub(self.rx_taken);
                if given > self.rx_entries() || given < self.rx_prod.wrapping_sub(self.rx_taken) {
                    self.index_fault = true;
                } else {
                    self.rx_prod = new;
                }
            }
            RX_TAKEN if on => {
                let new = v as u16;
                if new.wrapping_sub(self.rx_taken) > self.rx_cons.wrapping_sub(self.rx_taken) {
                    self.index_fault = true;
                } else {
                    self.rx_taken = new;
                }
            }
            _ => {}
        }
    }

    /// Disable, which is the reset: the transmits waiting dropped, the
    /// indexes and the interrupt enable 0. Quiet at once.
    pub fn disable(&mut self) {
        self.set_enabled(false);
        self.interrupt_enable = false;
        (self.tx_prod, self.tx_cons) = (0, 0);
        (self.rx_prod, self.rx_cons, self.rx_taken) = (0, 0, 0);
        self.head_due = None;
    }

    /// `RESET-DEVICES`: the disable, and the two fault bits cleared.
    pub fn reset(&mut self) {
        self.disable();
        self.refused = false;
        self.index_fault = false;
    }

    /// The entry at 245 taken, if there is one and none is taken, due 20 µs
    /// after `start` at the first clock at or after it.
    fn take(&mut self, start: u64, time: TimeBase) {
        if self.head_due.is_none() && self.enabled && self.queued() > 0 {
            self.head_due = Some(time.grid(start + time.ns(SEND_NS)));
        }
    }

    /// **The device advanced to `now`**: every transmit due and every
    /// Ethernet frame arrived by then, each at its own instant, in order;
    /// whether main memory was written. [`EthernetDevice::written`] says
    /// what the cache must do.
    pub fn advance<W: MemoryWord>(&mut self, now: u64, main: &mut [W], time: TimeBase) -> bool {
        self.written.sweep = false;
        self.written.lines.clear();
        let mut wrote = false;
        loop {
            let tx = self.head_due.filter(|&d| d <= now);
            let rx = match self.peer.as_mut().and_then(|p| p.arrival(now)) {
                Some(t) => Some(time.grid(t).max(self.reached)).filter(|&t| t <= now),
                None => None,
            };
            match (tx, rx) {
                (Some(d), r) if r.is_none_or(|r| d <= r) => {
                    self.reached = self.reached.max(d);
                    self.head_due = None;
                    self.send(d, main, time);
                    self.take(d, time);
                }
                (_, Some(r)) => {
                    self.reached = self.reached.max(r);
                    let f = self.peer.as_mut().map(|p| p.take()).unwrap_or_default();
                    wrote |= self.deliver(r, &f, main, time);
                }
                _ => break,
            }
        }
        self.reached = self.reached.max(now);
        wrote
    }

    /// A line of the trace at `at`, if the trace is on: `line` is made only
    /// then.
    fn say(&self, at: u64, time: TimeBase, line: impl FnOnce() -> String) {
        if let Some(t) = &self.trace {
            t(&format!("ethernet {}: {}", time.ns_text(at), line()));
        }
    }

    /// The entry at 245 sent, or refused, at `at`; 245 passes it.
    fn send<W: MemoryWord>(&mut self, at: u64, main: &[W], time: TimeBase) {
        let index = self.tx_cons;
        let e = self.tx_base as usize + 8 * (self.tx_cons % self.tx_entries()) as usize;
        let (addr, len) = (main[e].low() & ADDRESS, main[e + 1].low());
        self.tx_cons = self.tx_cons.wrapping_add(1);
        let at_word = addr as usize;
        let placed = if !(MIN_FRAME..=MAX_FRAME).contains(&len) {
            Err(format!("length {len}, outside {MIN_FRAME}-{MAX_FRAME}"))
        } else if addr & LINE != 0 {
            Err(format!("its Ethernet frame at {addr:o}, off a line"))
        } else if at_word + (len as usize).div_ceil(4) > main.len() {
            Err(format!("its Ethernet frame at {addr:o}, past main memory"))
        } else {
            Ok(())
        };
        if let Err(why) = placed {
            self.tx_refusals = (self.tx_refusals + 1) & 0xff;
            self.say(at, time, || format!("transmit entry {index} refused: {why}"));
            return;
        }
        let mut frame: Vec<u8> = (0..len as usize)
            .map(|k| (main[at_word + k / 4].low() >> (8 * (k % 4))) as u8)
            .collect();
        if let Err(why) = (self.rules.transmit)(&mut frame) {
            self.tx_refusals = (self.tx_refusals + 1) & 0xff;
            self.say(at, time, || format!("transmit entry {index} refused: {why}"));
            return;
        }
        let behind = if self.peer.is_some() { "" } else { ", nothing behind the device" };
        self.say(at, time, || format!("sent {}{behind}", described(&frame)));
        if let Some(p) = self.peer.as_mut() {
            p.sent(at, frame);
        }
    }

    /// An Ethernet frame arrived, at `at`: dropped or delivered. Whether
    /// main memory was written.
    fn deliver<W: MemoryWord>(
        &mut self,
        at: u64,
        f: &[u8],
        main: &mut [W],
        time: TimeBase,
    ) -> bool {
        let dropped = |d: &Self, why: &str| {
            d.say(at, time, || format!("received {}, dropped: {why}", described(f)));
        };
        if !self.enabled {
            dropped(self, "the device is disabled");
            return false;
        }
        let dst = f.get(..6);
        if dst != Some(&self.host.mac[..]) && dst != Some(&BROADCAST[..]) {
            dropped(self, "not this device's MAC address");
            return false;
        }
        if let Err((counter, why)) = (self.rules.receive)(f) {
            match counter {
                RxCounter::OtherType => self.type_drops = (self.type_drops + 1) & 0xf,
                RxCounter::Ipv4Check => self.ipv4_drops = (self.ipv4_drops + 1) & 0x7f,
            }
            dropped(self, why);
            return false;
        }
        if self.rx_prod == self.rx_cons {
            self.rx_drops = (self.rx_drops + 1) & 0xff;
            dropped(self, "no buffer given");
            return false;
        }
        let index = self.rx_cons;
        let e = self.rx_base as usize + 8 * (self.rx_cons % self.rx_entries()) as usize;
        let (buf, room) = (main[e].low() & ADDRESS, main[e + 1].low());
        let n = f.len();
        let words = n.div_ceil(4);
        if n > MAX_FRAME as usize
            || n > room as usize
            || buf & LINE != 0
            || buf as usize + words > main.len()
        {
            self.rx_drops = (self.rx_drops + 1) & 0xff;
            let why = if n > MAX_FRAME as usize {
                format!("longer than {MAX_FRAME} bytes")
            } else if n > room as usize {
                format!("entry {index}'s buffer of {room} bytes is too short")
            } else if buf & LINE != 0 {
                format!("entry {index}'s buffer at {buf:o}, off a line")
            } else {
                format!("entry {index}'s buffer at {buf:o}, past main memory")
            };
            dropped(self, &why);
            return false;
        }
        let buf = buf as usize;
        for (k, chunk) in f.chunks(4).enumerate() {
            let mut w = [0u8; 4];
            w[..chunk.len()].copy_from_slice(chunk);
            main[buf + k] = W::tagged(u32::from_le_bytes(w), FIXNUM);
        }
        main[e + 2] = W::tagged(n as u32, FIXNUM);
        match self.coherence {
            Coherence::Sweep => self.written.sweep = true,
            Coherence::LineClearing => {
                let lines = &mut self.written.lines;
                lines.extend((buf..buf + words).step_by(8).map(|a| a as u32));
                lines.push(e as u32);
            }
        }
        self.rx_cons = self.rx_cons.wrapping_add(1);
        self.delivered_at = Some(at);
        self.say(at, time, || format!("received {}, into entry {index}", described(f)));
        true
    }

    pub fn save(&self, w: &mut crate::checkpoint::Writer) {
        for b in [self.enabled, self.interrupt_enable, self.refused, self.index_fault] {
            w.bool(b);
        }
        for v in [self.tx_base, self.tx_log2, self.rx_base, self.rx_log2] {
            w.u32(v);
        }
        for v in [self.tx_prod, self.tx_cons, self.rx_prod, self.rx_cons, self.rx_taken] {
            w.u16(v);
        }
        for v in [self.type_drops, self.ipv4_drops, self.rx_drops, self.tx_refusals] {
            w.u32(v);
        }
        w.opt(self.head_due, |w, d| w.u64(d));
    }

    /// Back from a checkpoint: the registers, the counters and the
    /// transmit due. The host's words and the peer stay the run's.
    pub fn load(&mut self, r: &mut crate::checkpoint::Reader) -> std::io::Result<()> {
        let on = r.bool()?;
        self.set_enabled(on);
        self.interrupt_enable = r.bool()?;
        self.refused = r.bool()?;
        self.index_fault = r.bool()?;
        self.tx_base = r.u32()?;
        self.tx_log2 = r.u32()?;
        self.rx_base = r.u32()?;
        self.rx_log2 = r.u32()?;
        self.tx_prod = r.u16()?;
        self.tx_cons = r.u16()?;
        self.rx_prod = r.u16()?;
        self.rx_cons = r.u16()?;
        self.rx_taken = r.u16()?;
        self.type_drops = r.u32()? & 0xf;
        self.ipv4_drops = r.u32()? & 0x7f;
        self.rx_drops = r.u32()? & 0xff;
        self.tx_refusals = r.u32()? & 0xff;
        self.head_due = r.opt(|r| r.u64())?;
        self.reached = 0;
        self.delivered_at = None;
        if self.tx_log2 > 0o17 || self.rx_log2 > 0o17 {
            return Err(crate::checkpoint::bad("an Ethernet ring's size wider than 243's bits"));
        }
        Ok(())
    }

    /// Whether the state loaded fits a main memory of `words`: an enabled
    /// device's rings are what its enable checked.
    pub fn check(&self, words: usize) -> std::io::Result<()> {
        let fits = |base: u32, log2: u32| {
            base & LINE == 0 && log2 <= 8 && base as usize + (8 << log2) <= words
        };
        if self.enabled && !(fits(self.tx_base, self.tx_log2) && fits(self.rx_base, self.rx_log2)) {
            return Err(crate::checkpoint::bad(
                "an enabled Ethernet device whose rings its enable would refuse",
            ));
        }
        Ok(())
    }
}

/// An Ethernet frame as the trace names it: its length, its destination
/// and source and its type, as far as it has them.
fn described(f: &[u8]) -> String {
    if f.len() < 14 {
        return format!("{} bytes", f.len());
    }
    let mac = |m: &[u8]| m.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":");
    let kind = u16::from_be_bytes([f[12], f[13]]);
    format!("{} bytes, {} from {}, type {kind:04x}", f.len(), mac(&f[..6]), mac(&f[6..12]))
}
