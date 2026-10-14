// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's Ethernet device (QUUX's Ethernet contract, §2; revision 16):
//! words 240-257 of the register page, word 100 `<8>` and feature word 15
//! `<2>`; a transmit ring and a receive ring in main memory
//! ([`muir::ethernet_device`]).
//!
//! A scripted driver writes the rings' entries into main memory, writes the
//! indexes, moves the machine's clock on, and reads the registers back,
//! against a scripted peer attached in the process
//! ([`muir::ethernet_device::Script`]). The engines' tests, the posted
//! writes and the cache, are in `tests/revision_15_rtl.rs`.

mod support;

use muir::ethernet_device::{self as eth, Coherence, Rules, Script};
use muir::ethernet_offload::RxCounter;
use muir::machine::{Geometry, Machine, Word};

const PAGE: u32 = muir::tlb::REGISTER_PAGE_BUS;
const INTERRUPTS: u32 = PAGE + 0o100;
const RESET_DEVICES: u32 = PAGE + 0o104;

/// Revision 16's period in tests: 17 units of 0.5 ns, 8.5 ns.
const PERIOD: u64 = 17;

/// Where the driver keeps its rings, frames and buffers: physical word
/// addresses, each on a line, apart for rings of 256 entries. A frame or a
/// buffer of up to 128 bytes has a place of 32 words for each slot; a
/// longer one, of up to 2,048 or 1,536 bytes, one for each of 8 or 64 slots.
const TX_RING: u32 = 0o1000;
const RX_RING: u32 = 0o6000;
const FRAMES: u32 = 0o12000;
const LONG_FRAMES: u32 = 0o32000;
const BUFFERS: u32 = 0o42000;
const LONG_BUFFERS: u32 = 0o62000;
/// A long buffer's room: 384 words, two to a page.
const BUFFER_BYTES: u32 = 1536;
const BUFFER_WORDS: u32 = BUFFER_BYTES / 4;
/// A short buffer's room.
const SHORT_BYTES: u32 = 128;

/// The fixnum tag every word the device writes carries.
const FIXNUM: Word = 0o005 << 32;

/// The device's own MAC address by default, and broadcast.
const OWN: [u8; 6] = [0x02, 0, 0, 0, 0, 0x01];
const BROADCAST: [u8; 6] = [0xff; 6];

/// 20 us in revision 16's units.
const SEND: u64 = 2 * 20_000;

fn rev(geometry: Geometry) -> Machine {
    let mut m = Machine::with_geometry(geometry, 1);
    m.period = PERIOD;
    m
}

fn rev16() -> Machine {
    rev(Geometry::QUUX_16)
}

fn get(m: &mut Machine, w: u32) -> u32 {
    m.bus_read(PAGE + w) as u32
}

fn put(m: &mut Machine, w: u32, v: u32) {
    m.bus_write(PAGE + w, Word::from(v));
}

/// The machine's clock moved to `units`, on the grid, and the device with it.
fn at(m: &mut Machine, units: u64) {
    m.ns = m.time_base().grid(units);
    m.advance_ethernet();
}

fn state(m: &Machine) -> Vec<u8> {
    let mut w = muir::checkpoint::Writer::new();
    m.save(&mut w);
    w.finish()
}

/// An Ethernet frame of `n` bytes to `dst`, of type `ethertype`, its bytes
/// after the header counting from `seed`.
fn frame(dst: [u8; 6], ethertype: u16, n: usize, seed: u8) -> Vec<u8> {
    let mut f = vec![0u8; n];
    f[..6].copy_from_slice(&dst);
    f[6..12].copy_from_slice(&[0x02, 0, 0, 0, 0, 0x02]);
    f[12..14].copy_from_slice(&ethertype.to_be_bytes());
    for (k, b) in f[14..].iter_mut().enumerate() {
        *b = seed.wrapping_add(k as u8);
    }
    f
}

fn arp(n: usize, seed: u8) -> Vec<u8> {
    frame(OWN, 0x0806, n, seed)
}

/// Byte k of a buffer is `<8(k mod 4)+7:8(k mod 4)>` of word k/4.
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

/// The rings at [`TX_RING`] and [`RX_RING`], of 2^`tx` and 2^`rx`
/// entries, the device enabled under its interrupt enable, a scripted peer
/// attached.
fn enabled(tx: u32, rx: u32) -> (Machine, Script) {
    let mut m = rev16();
    let s = Script::default();
    m.attach_ethernet_peer(Box::new(s.clone()));
    put(&mut m, eth::TX_BASE, TX_RING);
    put(&mut m, eth::TX_SIZE, tx);
    put(&mut m, eth::RX_BASE, RX_RING);
    put(&mut m, eth::RX_SIZE, rx);
    put(&mut m, eth::CONTROL, 0x101);
    assert_eq!(get(&mut m, eth::STATUS) & 0xf, 1, "enabled, not refused");
    (m, s)
}

/// Transmit entry `index`: the frame at its slot's place, the entry at its
/// slot; the producer not written.
fn entry_tx(m: &mut Machine, index: u16, f: &[u8]) {
    let log2 = get(m, eth::TX_SIZE);
    let slot = (index as u32) % (1 << log2);
    let addr = if f.len() as u32 <= SHORT_BYTES {
        FRAMES + 32 * slot
    } else {
        LONG_FRAMES + 0o1000 * (slot % 8)
    };
    put_bytes(m, addr, f);
    let e = (TX_RING + 8 * slot) as usize;
    m.main[e] = Word::from(addr);
    m.main[e + 1] = f.len() as Word;
}

/// Transmit entry `index` posted: the entry, then 244 moved past it.
fn post(m: &mut Machine, index: u16, f: &[u8]) {
    entry_tx(m, index, f);
    put(m, eth::TX_PROD, index.wrapping_add(1).into());
}

/// Receive entry `index`, its buffer the slot's own, of `room` bytes,
/// given: the entry, then 250 moved past it.
fn give(m: &mut Machine, index: u16, room: u32) {
    let log2 = get(m, eth::RX_SIZE);
    let slot = (index as u32) % (1 << log2);
    let e = (RX_RING + 8 * slot) as usize;
    let buffer = if room <= SHORT_BYTES {
        BUFFERS + 32 * slot
    } else {
        LONG_BUFFERS + BUFFER_WORDS * (slot % 64)
    };
    m.main[e] = Word::from(buffer);
    m.main[e + 1] = Word::from(room);
    m.main[e + 2] = 0;
    put(m, eth::RX_PROD, index.wrapping_add(1).into());
}

fn rx_buffer(m: &Machine, index: u16, log2: u32) -> u32 {
    let slot = (index as u32) % (1 << log2);
    m.main[(RX_RING + 8 * slot) as usize] as u32
}

fn rx_length(m: &Machine, index: u16, log2: u32) -> Word {
    let slot = (index as u32) % (1 << log2);
    m.main[(RX_RING + 8 * slot) as usize + 2]
}

/// The four counters of 241: the type filter's `<7:4>`, the IPv4 checks'
/// `<15:9>`, receive drops `<23:16>`, transmit refusals `<31:24>`.
fn counters(m: &mut Machine) -> [u32; 4] {
    let s = get(m, eth::STATUS);
    [s >> 4 & 0xf, s >> 9 & 0x7f, s >> 16 & 0xff, s >> 24]
}

// --- the page --------------------------------------------------------------

/// **Revision 16 has words 240-257, and revision 15 reserves them** (§2.1,
/// §2.2, §10): on 16 each reads its power-on value --- 241 quiet, 253-257
/// the host's MAC address, IPv4 address, netmask and gateway, by default
/// `02:00:00:00:00:01` and 169.254.0.2/30 by 169.254.0.1 --- and a write of
/// all ones to a read-only word changes nothing; on 15 every one reads 0
/// and a write of any of them changes nothing. Feature word 0 says 16 and
/// feature word 15 reads 7 there, 3 on 15; every other word of the page
/// reads alike on both. **Fails** a device on 15, or one missing on 16.
#[test]
fn the_page_has_240_to_257_on_revision_16_and_reserves_them_on_15() {
    use Class::*;
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Class {
        ReadOnly,
        ReadWrite,
    }
    let table: [(u32, Class, u32); 16] = [
        (0o240, ReadWrite, 0),
        (0o241, ReadOnly, 2),
        (0o242, ReadWrite, 0),
        (0o243, ReadWrite, 0),
        (0o244, ReadWrite, 0),
        (0o245, ReadOnly, 0),
        (0o246, ReadWrite, 0),
        (0o247, ReadWrite, 0),
        (0o250, ReadWrite, 0),
        (0o251, ReadOnly, 0),
        (0o252, ReadWrite, 0),
        (0o253, ReadOnly, 0x0200),
        (0o254, ReadOnly, 0x0000_0001),
        (0o255, ReadOnly, 0xa9fe_0002),
        (0o256, ReadOnly, 0xffff_fffc),
        (0o257, ReadOnly, 0xa9fe_0001),
    ];
    for &(w, class, want) in &table {
        let mut m = rev16();
        assert_eq!(get(&mut m, w), want, "revision 16: word {w:o} at power-on");
        assert_eq!(m.bus_error, 0, "revision 16: word {w:o} answered");
        if class == ReadOnly {
            let s = state(&m);
            put(&mut m, w, !0);
            assert!(state(&m) == s, "revision 16: word {w:o} is read only");
            assert_eq!(get(&mut m, w), want, "revision 16: word {w:o} after a write");
        }
        let mut m = rev(Geometry::QUUX_15);
        assert_eq!(get(&mut m, w), 0, "revision 15: word {w:o} is reserved");
        let s = state(&m);
        for v in [!0, 1, 0x101] {
            put(&mut m, w, v);
        }
        assert!(state(&m) == s, "revision 15: a write of word {w:o} changed the machine");
        assert_eq!(m.bus_error, 0, "revision 15: word {w:o} answered");
    }
    let (mut a, mut b) = (rev16(), rev(Geometry::QUUX_15));
    assert_eq!(get(&mut a, 0) >> 4 & 0o7777, 16, "MACHINE-ID says revision 16");
    assert_eq!(get(&mut b, 0) >> 4 & 0o7777, 15);
    assert_eq!(get(&mut a, 0o15), 7, "feature word 15: the clock, the file device, Ethernet");
    assert_eq!(get(&mut b, 0o15), 3, "revision 15's feature word 15 unchanged");
    for w in (1..0o400).filter(|&w| !matches!(w, 0o15 | 0o103 | 0o240..=0o257)) {
        assert_eq!(get(&mut a, w), get(&mut b, w), "word {w:o} alike on 15 and 16");
    }
}

/// **The read-only words ignore writes** and the configuration words take
/// theirs while disabled (§2.2, §2.3): 242 `<27:0>`, 243 `<3:0>`, 246 and
/// 247 the same, each read back; 244, 250 and 252 go nowhere while
/// disabled, and the indexes read 0.
#[test]
fn the_configuration_words_read_back_and_the_indexes_go_nowhere_while_disabled() {
    let mut m = rev16();
    put(&mut m, eth::TX_BASE, !0);
    put(&mut m, eth::TX_SIZE, !0);
    put(&mut m, eth::RX_BASE, 0o1234567);
    put(&mut m, eth::RX_SIZE, 3);
    assert_eq!(get(&mut m, eth::TX_BASE), 0o1777777777);
    assert_eq!(get(&mut m, eth::TX_SIZE), 0o17);
    assert_eq!(get(&mut m, eth::RX_BASE), 0o1234567);
    assert_eq!(get(&mut m, eth::RX_SIZE), 3);
    let s = state(&m);
    for w in [eth::TX_PROD, eth::RX_PROD, eth::RX_TAKEN] {
        put(&mut m, w, 5);
        assert_eq!(get(&mut m, w), 0, "word {w:o} while disabled");
    }
    assert!(state(&m) == s, "the index writes went nowhere");
    assert_eq!(get(&mut m, eth::STATUS) & 8, 0, "and set no index fault");
}

// --- enable, and its refusal ------------------------------------------------

/// **The enable checks the rings** (§2.3): a base off a line, a size over
/// 8, or a ring past main memory is refused, status `<2>`, and the device
/// stays disabled; a write of 240 clears `<2>`; the good rings enable, and
/// then 242, 243, 246 and 247 ignore writes. **Fails** an enable that
/// checks one ring only, or none.
#[test]
fn the_enable_refuses_a_ring_off_a_line_too_big_or_past_memory() {
    let words = 1u32 << 16;
    let good =
        [(eth::TX_BASE, TX_RING), (eth::TX_SIZE, 8), (eth::RX_BASE, RX_RING), (eth::RX_SIZE, 8)];
    let bad = [
        (eth::TX_BASE, TX_RING + 1, "transmit base off a line"),
        (eth::RX_BASE, RX_RING + 4, "receive base off a line"),
        (eth::TX_SIZE, 9, "transmit ring over 256 entries"),
        (eth::RX_SIZE, 15, "receive ring over 256 entries"),
        (eth::TX_BASE, words - 8 * 255, "transmit ring past main memory"),
        (eth::RX_BASE, words - 8, "receive ring past main memory"),
    ];
    for (w, v, what) in bad {
        let mut m = rev16();
        for (g, gv) in good {
            put(&mut m, g, gv);
        }
        put(&mut m, w, v);
        put(&mut m, eth::CONTROL, 0x101);
        let s = get(&mut m, eth::STATUS);
        assert_eq!(s & 7, 0b110, "{what}: refused, disabled and quiet");
        assert_eq!(get(&mut m, eth::CONTROL), 0, "{what}: 240 reads disabled");
        put(&mut m, eth::CONTROL, 0);
        assert_eq!(get(&mut m, eth::STATUS) & 4, 0, "{what}: a write of 240 clears <2>");
    }
    // At the very end of memory, a ring fits.
    let mut m = rev16();
    put(&mut m, eth::TX_BASE, words - 8 * 256);
    put(&mut m, eth::TX_SIZE, 8);
    put(&mut m, eth::RX_BASE, words - 8);
    put(&mut m, eth::RX_SIZE, 0);
    put(&mut m, eth::CONTROL, 1);
    assert_eq!(get(&mut m, eth::STATUS) & 7, 1, "rings to the last word enable");
    for (w, v) in
        [(eth::TX_BASE, 0o100), (eth::TX_SIZE, 1), (eth::RX_BASE, 0o200), (eth::RX_SIZE, 2)]
    {
        let before = get(&mut m, w);
        put(&mut m, w, v);
        assert_eq!(get(&mut m, w), before, "word {w:o} ignores a write while enabled");
    }
}

// --- index faults ----------------------------------------------------------

/// **An index write that breaks the rings' rules is ignored and sets
/// status `<3>`** (§2.3): 244 past 245 by more than the transmit ring, 250
/// past 252 by more than the receive ring, 252 past 251, and any index
/// moved backwards; a write of 240 clears `<3>`. The same value written
/// again is no move and no fault. **Fails** a rule left out.
#[test]
fn an_index_write_against_the_rules_is_ignored_and_faults() {
    type Case = (&'static str, fn(&mut Machine, &Script), u32, u32);
    let cases: [Case; 7] = [
        ("244 past the transmit ring", |_, _| {}, eth::TX_PROD, 5),
        ("250 past the receive ring", |_, _| {}, eth::RX_PROD, 3),
        ("252 past 251", |_, _| {}, eth::RX_TAKEN, 1),
        (
            "244 backwards",
            |m, _| {
                entry_tx(m, 0, &arp(60, 0));
                entry_tx(m, 1, &arp(60, 1));
                put(m, eth::TX_PROD, 2);
            },
            eth::TX_PROD,
            1,
        ),
        ("250 backwards", |m, _| put(m, eth::RX_PROD, 2), eth::RX_PROD, 1),
        (
            "252 backwards",
            |m, s| {
                give(m, 0, BUFFER_BYTES);
                give(m, 1, BUFFER_BYTES);
                s.arrive(0, arp(60, 0));
                s.arrive(0, arp(60, 1));
                at(m, 10);
                put(m, eth::RX_TAKEN, 2);
            },
            eth::RX_TAKEN,
            1,
        ),
        ("244 backwards past 2^16", |_, _| {}, eth::TX_PROD, 0xffff),
    ];
    for (what, setup, w, v) in cases {
        let (mut m, s) = enabled(2, 1);
        setup(&mut m, &s);
        assert_eq!(get(&mut m, eth::STATUS) & 8, 0, "{what}: no fault before");
        let before = get(&mut m, w);
        put(&mut m, w, v);
        assert_eq!(get(&mut m, eth::STATUS) & 8, 8, "{what}: <3>");
        assert_eq!(get(&mut m, w), before, "{what}: ignored");
        put(&mut m, w, before);
        assert_eq!(get(&mut m, w), before, "{what}: the same value");
        put(&mut m, eth::CONTROL, 0x101);
        assert_eq!(get(&mut m, eth::STATUS) & 9, 1, "{what}: a write of 240 clears <3>, enabled");
        put(&mut m, w, before);
        assert_eq!(get(&mut m, eth::STATUS) & 8, 0, "{what}: the same value is no fault");
    }
    // The bounds themselves are allowed: a full transmit ring, every buffer
    // given, 252 up to 251.
    let (mut m, s) = enabled(2, 1);
    for k in 0..4 {
        entry_tx(&mut m, k, &arp(60, k as u8));
    }
    put(&mut m, eth::TX_PROD, 4);
    give(&mut m, 0, BUFFER_BYTES);
    give(&mut m, 1, BUFFER_BYTES);
    s.arrive(0, arp(60, 9));
    at(&mut m, 10);
    put(&mut m, eth::RX_TAKEN, 1);
    assert_eq!(get(&mut m, eth::STATUS) & 8, 0, "the bounds are allowed");
    assert_eq!(
        [get(&mut m, eth::TX_PROD), get(&mut m, eth::RX_PROD), get(&mut m, eth::RX_TAKEN)],
        [4, 2, 1]
    );
}

// --- the rings ---------------------------------------------------------------

/// The ring of 2^`log2` entries run through `rounds` times its size each
/// way: each transmit read from its slot and sent as written, each received
/// Ethernet frame in its slot's buffer with its length, 245, 251 and 252
/// moving by one an entry.
fn run_rings(log2: u32, total: u32) {
    let (mut m, s) = enabled(log2, log2);
    let size = 1u32 << log2;
    let mut now = 0;
    let mut k: u32 = 0;
    while k < total {
        let n = size.min(total - k);
        for j in 0..n {
            let i = (k + j) as u16;
            entry_tx(&mut m, i, &arp(60 + (i as usize % 7), i as u8));
            give(&mut m, i, SHORT_BYTES);
            s.arrive(now, arp(61 + (i as usize % 5), !(i as u8)));
        }
        put(&mut m, eth::TX_PROD, (k + n) as u16 as u32);
        now += SEND * (n as u64 + 1);
        at(&mut m, now);
        assert_eq!(get(&mut m, eth::TX_CONS), (k + n) & 0xffff, "2^{log2}: 245");
        assert_eq!(get(&mut m, eth::RX_CONS), (k + n) & 0xffff, "2^{log2}: 251");
        for j in 0..n {
            let i = (k + j) as u16;
            let f = arp(61 + (i as usize % 5), !(i as u8));
            let buf = rx_buffer(&m, i, log2);
            assert_eq!(bytes_at(&m, buf, f.len()), f, "2^{log2}: received {i}");
            assert_eq!(rx_length(&m, i, log2), FIXNUM | f.len() as Word, "2^{log2}: length {i}");
        }
        put(&mut m, eth::RX_TAKEN, (k + n) as u16 as u32);
        assert_eq!(get(&mut m, eth::STATUS) & 8, 0, "2^{log2}: no fault");
        k += n;
    }
    let sent = s.sent();
    assert_eq!(sent.len(), total as usize, "2^{log2}: every transmit sent");
    for (i, (_, f)) in sent.iter().enumerate() {
        let i = i as u16;
        assert_eq!(*f, arp(60 + (i as usize % 7), i as u8), "2^{log2}: transmit {i}");
    }
    assert_eq!(counters(&mut m), [0; 4], "2^{log2}: nothing dropped or refused");
}

/// **The rings wrap at 1, 2 and 256 entries** (§2.3): an entry's slot is
/// its index mod the ring's size, three times round each ring. **Fails** a
/// slot taken from an index not reduced, or reduced by the wrong size.
#[test]
fn the_rings_wrap_at_1_2_and_256_entries() {
    for log2 in [0, 1, 8] {
        run_rings(log2, 3 << log2);
    }
}

/// **The indexes run free across 2^16** (§2.3): 65,540 Ethernet frames each
/// way through rings of 256 and of 1, the indexes wrapping to 4 and the
/// slots following. **Fails** an index kept wider than 16 bits, or a
/// fault at the wrap.
#[test]
fn the_indexes_run_free_across_2_to_the_16() {
    run_rings(8, 65_540);
    run_rings(0, 65_540);
}

/// **A full receive ring drops and counts** (§2.3): with two buffers given
/// a third Ethernet frame is dropped, counted in 241 `<23:16>`, and takes
/// nothing; one longer than its buffer is dropped and counted the same and
/// leaves the buffer to the next, which fits. The count wraps at 256.
/// **Fails** a drop that is not counted, or a buffer taken by a frame that
/// does not fit.
#[test]
fn a_full_receive_ring_drops_and_counts() {
    let (mut m, s) = enabled(0, 2);
    give(&mut m, 0, BUFFER_BYTES);
    give(&mut m, 1, 64);
    for k in 0..3 {
        s.arrive(0, arp(60, k));
    }
    at(&mut m, 10);
    assert_eq!(get(&mut m, eth::RX_CONS), 2, "two buffers filled");
    assert_eq!(counters(&mut m)[2], 1, "the third dropped and counted");
    assert_eq!(bytes_at(&m, rx_buffer(&m, 1, 2), 60), arp(60, 1), "the second, not the third");
    put(&mut m, eth::RX_TAKEN, 2);
    // Longer than its buffer.
    give(&mut m, 2, 64);
    let before = m.main.clone();
    s.arrive(0, arp(65, 7));
    at(&mut m, 20);
    assert_eq!(get(&mut m, eth::RX_CONS), 2, "65 bytes do not fit in 64");
    assert_eq!(counters(&mut m)[2], 2, "counted");
    assert!(m.main == before, "and wrote nothing");
    s.arrive(0, arp(64, 8));
    at(&mut m, 30);
    assert_eq!(get(&mut m, eth::RX_CONS), 3, "64 bytes fit");
    assert_eq!(bytes_at(&m, rx_buffer(&m, 2, 2), 64), arp(64, 8));
    // The count wraps at 256.
    for k in 0..255u32 {
        s.arrive(0, arp(60, k as u8));
    }
    at(&mut m, 40);
    assert_eq!(counters(&mut m)[2], 1, "257 drops read 1");
}

/// **A received Ethernet frame longer than 1,514 bytes is dropped as too
/// long** (QUUX's Ethernet contract, §2.3): counted in
/// `<23:16>`, nothing written and 251 staying, though its buffer has room;
/// one of 1,514 bytes is delivered into the same buffer. **Fails** a device
/// that delivers whatever fits its buffer.
#[test]
fn a_received_frame_over_1514_bytes_is_dropped_and_counted() {
    let (mut m, s) = enabled(0, 0);
    give(&mut m, 0, BUFFER_BYTES);
    let before = m.main.clone();
    for n in [1515, 1536] {
        s.arrive(0, arp(n, 1));
    }
    at(&mut m, 10);
    assert_eq!(get(&mut m, eth::RX_CONS), 0, "neither taken");
    assert_eq!(counters(&mut m), [0, 0, 2, 0], "both counted in <23:16>");
    assert!(m.main == before, "nothing written");
    s.arrive(0, arp(1514, 2));
    at(&mut m, 20);
    assert_eq!(get(&mut m, eth::RX_CONS), 1, "1,514 bytes taken");
    assert_eq!(bytes_at(&m, rx_buffer(&m, 0, 0), 1514), arp(1514, 2));
}

/// **A buffer off a line, or running past main memory, is taken as too
/// short** (QUUX's Ethernet contract, §2.3): the Ethernet frame
/// is dropped and counted in `<23:16>`, nothing is written, and 251 stays,
/// so the next drops too. **Fails** a device that writes a buffer it
/// cannot hold whole, or skips the bad entry.
#[test]
fn a_buffer_off_a_line_or_past_memory_is_too_short() {
    let words = rev16().main.len() as u32;
    for (buffer, what) in [(BUFFERS + 1, "off a line"), (words - 8, "past main memory")] {
        let (mut m, s) = enabled(0, 1);
        let e = RX_RING as usize;
        m.main[e] = Word::from(buffer);
        m.main[e + 1] = Word::from(BUFFER_BYTES);
        put(&mut m, eth::RX_PROD, 1);
        give(&mut m, 1, BUFFER_BYTES);
        let before = m.main.clone();
        s.arrive(0, arp(60, 1));
        s.arrive(0, arp(60, 2));
        at(&mut m, 10);
        assert_eq!(get(&mut m, eth::RX_CONS), 0, "{what}: 251 stays");
        assert_eq!(counters(&mut m), [0, 0, 2, 0], "{what}: both counted");
        assert!(m.main == before, "{what}: nothing written");
    }
}

/// **A refused transmit entry takes its 20 µs as a sent one does** (QUUX's
/// Ethernet contract, §2.8): transmits are serial, so an
/// entry after a refused one is due 20 µs after the refusal, rounded once
/// to the grid. **Fails** a refusal that passes the entry at once.
#[test]
fn a_refused_transmit_entry_takes_20_us() {
    let (mut m, s) = enabled(2, 0);
    let grid = m.time_base();
    m.ns = grid.grid(3 * PERIOD);
    entry_tx(&mut m, 0, &arp(60, 0));
    m.main[TX_RING as usize + 1] = 13;
    put(&mut m, eth::TX_PROD, 1);
    post(&mut m, 1, &arp(60, 1));
    let refused = grid.grid(m.ns + SEND);
    let sent = grid.grid(refused + SEND);
    at(&mut m, sent - PERIOD);
    assert_eq!(get(&mut m, eth::TX_CONS), 1, "the refusal passed, the next not yet due");
    assert_eq!(counters(&mut m)[3], 1, "refused");
    at(&mut m, sent);
    assert_eq!(s.sent(), vec![(sent, arp(60, 1))], "20 us after the refusal");
}

/// **A received Ethernet frame is written as the file device writes**
/// (§2.2, §2.3): byte k in `<8(k mod 4)+7:8(k mod 4)>` of word k/4, every
/// word a fixnum, the bytes past its end in the last word 0, no word past
/// it, and its length in the entry's word 2. **Fails** a write of whole
/// words past the frame, or of untagged words.
#[test]
fn a_received_frame_is_written_as_fixnums_with_its_tail_zero() {
    let (mut m, s) = enabled(0, 0);
    give(&mut m, 0, BUFFER_BYTES);
    let buf = rx_buffer(&m, 0, 0) as usize;
    for k in 0..20 {
        m.main[buf + k] = 0o7_7777_7777_7777;
    }
    let f = arp(61, 3);
    s.arrive(0, f.clone());
    at(&mut m, 10);
    for k in 0..15 {
        let want = u32::from_le_bytes(f[4 * k..4 * k + 4].try_into().unwrap());
        assert_eq!(m.main[buf + k], FIXNUM | Word::from(want), "word {k}");
    }
    assert_eq!(m.main[buf + 15], FIXNUM | Word::from(f[60]), "the last word's tail 0");
    assert_eq!(m.main[buf + 16], 0o7_7777_7777_7777, "no word past the frame");
    assert_eq!(rx_length(&m, 0, 0), FIXNUM | 61);
}

// --- coherence -----------------------------------------------------------------

/// **A delivery asks the cache for the sweep by default, and for its lines
/// under line clearing** (§2.5): the sweep is `Machine::dma_written`, as a
/// file device completion's is; line clearing names the buffer's lines it
/// wrote, ceil(n/4)/8 of them, and the entry's line, and sweeps nothing. A
/// transmit asks for neither. **Fails** a mechanism that asks for the
/// other's work, or misses a line.
#[test]
fn a_delivery_asks_for_the_sweep_or_for_its_lines() {
    for (coherence, n) in
        [(Coherence::Sweep, 61), (Coherence::LineClearing, 61), (Coherence::LineClearing, 1514)]
    {
        let (mut m, s) = enabled(0, 0);
        m.ethernet.coherence = coherence;
        give(&mut m, 0, BUFFER_BYTES);
        post(&mut m, 0, &arp(60, 0));
        m.dma_written = false;
        at(&mut m, 2 * SEND);
        assert!(!m.dma_written, "{coherence:?}: a transmit sweeps nothing");
        assert!(m.ethernet.written.lines.is_empty(), "{coherence:?}: a transmit clears nothing");
        s.arrive(m.ns, arp(n, 1));
        at(&mut m, 3 * SEND);
        let buf = rx_buffer(&m, 0, 0);
        match coherence {
            Coherence::Sweep => {
                assert!(m.dma_written, "the sweep");
                assert!(m.ethernet.written.lines.is_empty());
            }
            Coherence::LineClearing => {
                assert!(!m.dma_written, "no sweep");
                let words = (n as u32).div_ceil(4);
                let mut want: Vec<u32> = (0..words.div_ceil(8)).map(|k| buf + 8 * k).collect();
                want.push(RX_RING);
                assert_eq!(
                    m.ethernet.written.lines, want,
                    "{n} bytes: the buffer's lines and the entry's"
                );
            }
        }
    }
}

// --- the MAC address filter ----------------------------------------------------

/// **The device takes Ethernet frames to its own MAC address and to
/// broadcast** (§2.3, §2.4.6): another unicast address, a multicast one,
/// and a frame too short to hold an address are dropped with no count and
/// take no buffer. There is no promiscuous bit: 240 written all ones takes
/// no other address. The address is 253-254's. **Fails** a filter that
/// takes everything, or counts what it drops.
#[test]
fn the_mac_address_filter_takes_its_own_address_and_broadcast() {
    let (mut m, s) = enabled(0, 3);
    for k in 0..4 {
        give(&mut m, k, BUFFER_BYTES);
    }
    put(&mut m, eth::CONTROL, !0);
    let other = [0x02, 0, 0, 0, 0, 0x03];
    let multicast = [0x01, 0x00, 0x5e, 0, 0, 1];
    let before = m.main.clone();
    for dst in [other, multicast] {
        s.arrive(0, frame(dst, 0x0800, 60, 0));
    }
    s.arrive(0, vec![0x02, 0, 0]);
    at(&mut m, 10);
    assert_eq!(get(&mut m, eth::RX_CONS), 0, "none taken");
    assert_eq!(counters(&mut m), [0; 4], "none counted");
    assert!(m.main == before, "nothing written");
    s.arrive(0, frame(OWN, 0x0806, 60, 1));
    s.arrive(0, frame(BROADCAST, 0x0806, 60, 2));
    at(&mut m, 20);
    assert_eq!(get(&mut m, eth::RX_CONS), 2, "its own and broadcast taken");
    assert_eq!(bytes_at(&m, rx_buffer(&m, 0, 3), 60), frame(OWN, 0x0806, 60, 1));
    assert_eq!(bytes_at(&m, rx_buffer(&m, 1, 3), 60), frame(BROADCAST, 0x0806, 60, 2));
    // Another address of the host's own.
    let (mut m, s) = enabled(0, 0);
    m.ethernet.host.mac = other;
    give(&mut m, 0, BUFFER_BYTES);
    assert_eq!([get(&mut m, eth::MAC_HIGH), get(&mut m, eth::MAC_LOW)], [0x0200, 0x0000_0003]);
    s.arrive(0, frame(OWN, 0x0806, 60, 1));
    at(&mut m, 10);
    assert_eq!(get(&mut m, eth::RX_CONS), 0, "the default address is no longer its own");
    s.arrive(0, frame(other, 0x0806, 60, 1));
    at(&mut m, 20);
    assert_eq!(get(&mut m, eth::RX_CONS), 1, "its own address");
}

// --- §2.4's counts, through the rules' interface -----------------------------

/// The rules' double: on receive, a frame whose byte 14 is 1 is dropped
/// by the type filter and 2 by the IPv4 checks; on transmit, byte 14 of 1
/// is refused, and every frame sent has byte 15 set to 0xAA, as a filled
/// checksum is.
fn double() -> Rules {
    Rules {
        transmit: |f| match f[14] {
            1 => Err("the double refuses it"),
            _ => {
                f[15] = 0xaa;
                Ok(())
            }
        },
        receive: |f| match f[14] {
            1 => Err((RxCounter::OtherType, "the double's type")),
            2 => Err((RxCounter::Ipv4Check, "the double's IPv4 check")),
            _ => Ok(()),
        },
    }
}

/// A frame for [`double`] whose byte 14 is `verdict`.
fn judged(verdict: u8, seed: u8) -> Vec<u8> {
    let mut f = arp(60, seed);
    f[14] = verdict;
    f
}

/// **241 counts each drop of the type filter in `<7:4>`, mod 16, and of
/// the IPv4 checks in `<15:9>`, mod 128, and neither takes a buffer**
/// (§2.4.3, §2.4.5): through the rules' interface, 17 of one and 145 of
/// the other, interleaved with frames it delivers; the buffers go to the
/// delivered ones alone, in order, and `<23:16>` stays 0. **Fails** a
/// counter in the wrong field, a drop that takes a buffer, or one counted
/// in `<23:16>`.
#[test]
fn the_type_filter_and_the_ipv4_checks_count_and_take_no_buffer() {
    let (mut m, s) = enabled(0, 8);
    m.ethernet.rules = double();
    for k in 0..4 {
        give(&mut m, k, BUFFER_BYTES);
    }
    let before: Vec<Word> = m.main.clone();
    let mut delivered = Vec::new();
    for k in 0..145u32 {
        if k < 17 {
            s.arrive(0, judged(1, k as u8));
        }
        s.arrive(0, judged(2, k as u8));
        if k % 40 == 0 {
            let f = judged(0, k as u8);
            delivered.push(f.clone());
            s.arrive(0, f);
        }
    }
    at(&mut m, 10);
    assert_eq!(counters(&mut m), [1, 17, 0, 0], "17 mod 16 and 145 mod 128, no other count");
    assert_eq!(get(&mut m, eth::RX_CONS), 4, "four delivered");
    for (k, f) in delivered.iter().enumerate() {
        assert_eq!(bytes_at(&m, rx_buffer(&m, k as u16, 8), 60), *f, "buffer {k} its own");
    }
    // Nothing else was written: the dropped frames took no buffer.
    let written: Vec<usize> = (0..m.main.len()).filter(|&a| m.main[a] != before[a]).collect();
    let mut allowed: Vec<usize> = Vec::new();
    for k in 0..4u16 {
        let b = rx_buffer(&m, k, 8) as usize;
        allowed.extend(b..b + 15);
        allowed.push((RX_RING + 8 * k as u32) as usize + 2);
    }
    assert!(written.iter().all(|a| allowed.contains(a)), "a drop wrote memory: {written:?}");
    // With every buffer used, a drop by the rules is still theirs alone.
    s.arrive(0, judged(1, 0));
    s.arrive(0, judged(0, 0));
    at(&mut m, 20);
    assert_eq!(counters(&mut m), [2, 17, 1, 0], "the type filter first, then no buffer");
}

/// **241 `<31:24>` counts each transmit refused, mod 256, and 245 passes
/// it** (§2.3, §2.4.2): a length under 14 or over 1514, a frame off a line
/// or past main memory, and a frame the rules refuse; none is sent, and
/// the good ones around them are. **Fails** a refusal that is sent, not
/// counted, or that holds 245.
#[test]
fn transmit_refusals_are_counted_and_passed() {
    let (mut m, s) = enabled(3, 0);
    m.ethernet.rules = double();
    let words = m.main.len() as u32;
    let mut i = 0u16;
    let mut post_raw = |m: &mut Machine, addr: u32, len: u32| {
        let e = (TX_RING + 8 * (i as u32 % 8)) as usize;
        m.main[e] = Word::from(addr);
        m.main[e + 1] = Word::from(len);
        i += 1;
        put(m, eth::TX_PROD, i.into());
    };
    put_bytes(&mut m, FRAMES, &judged(0, 1));
    put_bytes(&mut m, FRAMES + 32, &judged(1, 2));
    let mut wanted = 0;
    let rounds = 52;
    for round in 0..rounds {
        for (addr, len) in [
            (FRAMES, 13),
            (FRAMES, 1515),
            (FRAMES + 1, 60),
            (words - 8, 60),
            (FRAMES + 32, 60),
            (FRAMES, 60),
            (words - 16, 60),
        ] {
            post_raw(&mut m, addr, len);
            let due = m.ns + SEND;
            at(&mut m, due);
        }
        wanted += 5;
        assert_eq!(counters(&mut m)[3], wanted % 256, "round {round}");
    }
    assert_eq!(wanted, 260, "the count wrapped");
    assert_eq!(get(&mut m, eth::TX_CONS), 7 * rounds, "245 passed every one");
    let sent = s.sent();
    assert_eq!(sent.len(), 2 * rounds as usize, "the good ones sent");
    let mut want = judged(0, 1);
    want[15] = 0xaa;
    assert!(sent.iter().all(|(_, f)| f.len() == 60), "60 bytes each");
    assert_eq!(sent[0].1, want, "with the rules' change");
    // 14 and 1514 are allowed.
    let (mut m, s) = enabled(1, 0);
    let long = arp(1514, 0);
    post(&mut m, 0, &arp(14, 0));
    post(&mut m, 1, &long);
    at(&mut m, 3 * SEND);
    assert_eq!(counters(&mut m)[3], 0);
    assert_eq!(s.sent().iter().map(|(_, f)| f.len()).collect::<Vec<_>>(), [14, 1514]);
}

/// **A transmit writes no memory** (§2.3, §2.4.1): the rules fill the
/// frame they send, never the buffer; the frame resent from the same buffer
/// is filled afresh. Main memory is the same word for word after every
/// transmit, sent or refused. **Fails** a device that fills the buffer.
#[test]
fn a_transmit_writes_no_memory() {
    let (mut m, s) = enabled(2, 0);
    m.ethernet.rules = double();
    let f = judged(0, 5);
    entry_tx(&mut m, 0, &f);
    for k in 0..4u16 {
        let e = (TX_RING + 8 * (k as u32 % 4)) as usize;
        m.main[e] = m.main[TX_RING as usize];
        m.main[e + 1] = m.main[TX_RING as usize + 1];
    }
    let before_posted = m.main.clone();
    put(&mut m, eth::TX_PROD, 4);
    at(&mut m, 10 * SEND);
    assert!(m.main == before_posted, "a transmit wrote memory");
    let sent = s.sent();
    assert_eq!(sent.len(), 4);
    let mut want = f.clone();
    want[15] = 0xaa;
    assert!(sent.iter().all(|(_, g)| *g == want), "each filled afresh from the same buffer");
}

/// **The device calls `ethernet_offload`'s rules by default** (§2.4): an
/// IPv6 Ethernet frame, a Chaosnet one, an ARP one and an IPv4 one with a
/// wrong header checksum are each received, and transmitted, with the
/// verdict the rules give for it: delivered or counted in the counter
/// they name, sent or counted in `<31:24>`.
#[test]
fn the_default_rules_are_ethernet_offload_s() {
    let mut ipv4 = frame(OWN, 0x0800, 60, 0);
    ipv4[14..34].copy_from_slice(&[
        0x45, 0, 0, 46, 0, 0, 0, 0, 64, 17, 0x12, 0x34, 169, 254, 0, 1, 169, 254, 0, 2,
    ]);
    let frames =
        [frame(OWN, 0x86dd, 60, 0), frame(OWN, 0x0804, 60, 0), frame(OWN, 0x0806, 60, 0), ipv4];
    for f in frames {
        let (mut m, s) = enabled(0, 0);
        give(&mut m, 0, BUFFER_BYTES);
        s.arrive(0, f.clone());
        at(&mut m, 10);
        let mut c = counters(&mut m);
        match muir::ethernet_offload::receive(&f) {
            Ok(()) => assert_eq!(get(&mut m, eth::RX_CONS), 1, "delivered"),
            Err(d) => {
                assert_eq!(get(&mut m, eth::RX_CONS), 0, "{}: dropped", d.reason());
                let k = match d.counter() {
                    RxCounter::OtherType => 0,
                    RxCounter::Ipv4Check => 1,
                };
                assert_eq!(c[k], 1, "{}: counted", d.reason());
                c[k] = 0;
            }
        }
        assert_eq!(c[..3], [0, 0, 0], "no other receive count");
        let posted = m.ns;
        post(&mut m, 0, &f);
        at(&mut m, 10 + 2 * SEND);
        let mut sent = f.clone();
        match muir::ethernet_offload::transmit(&mut sent) {
            Ok(()) => assert_eq!(s.sent(), [(m.time_base().grid(posted + SEND), sent)]),
            Err(r) => {
                assert!(s.sent().is_empty(), "{}: not sent", r.reason());
                assert_eq!(counters(&mut m)[3], 1, "{}: counted", r.reason());
            }
        }
    }
}

// --- disable, reset and quiet ---------------------------------------------------

/// **Disable is the reset** (§2.7): with transmits waiting and a frame
/// waiting, 240 written 0 drops the transmits, receives nothing more, puts
/// the indexes and the interrupt enable at 0 and keeps the bases, the
/// sizes and 253-257; quiet, 241 `<1>`, is up at once. Frames arriving
/// while disabled are not taken and not counted. **Fails** a disable that
/// sends what was waiting, or keeps an index.
#[test]
fn disable_drops_what_waits_and_keeps_the_configuration() {
    let (mut m, s) = enabled(2, 2);
    give(&mut m, 0, BUFFER_BYTES);
    give(&mut m, 1, BUFFER_BYTES);
    s.arrive(0, arp(60, 0));
    at(&mut m, 10);
    for k in 0..3 {
        post(&mut m, k, &arp(60, k as u8));
    }
    assert_eq!(get(&mut m, eth::STATUS) & 0x103, 0x101, "enabled, busy, a frame waiting");
    put(&mut m, eth::CONTROL, 0);
    assert_eq!(get(&mut m, eth::STATUS) & 0x103, 2, "disabled, quiet, nothing waiting");
    assert_eq!(get(&mut m, eth::CONTROL), 0, "the interrupt enable 0");
    for w in [eth::TX_PROD, eth::TX_CONS, eth::RX_PROD, eth::RX_CONS, eth::RX_TAKEN] {
        assert_eq!(get(&mut m, w), 0, "word {w:o} 0");
    }
    let kept =
        [(eth::TX_BASE, TX_RING), (eth::TX_SIZE, 2), (eth::RX_BASE, RX_RING), (eth::RX_SIZE, 2)];
    for (w, v) in kept {
        assert_eq!(get(&mut m, w), v, "word {w:o} kept");
    }
    assert_eq!(get(&mut m, eth::IP_ADDRESS), 0xa9fe_0002, "253-257 kept");
    s.arrive(m.ns, arp(60, 5));
    at(&mut m, 100 * SEND);
    assert!(s.sent().is_empty(), "nothing waiting was sent");
    assert_eq!(counters(&mut m), [0; 4], "nothing received or counted while disabled");
    // Enabled again: the rings start at 0.
    put(&mut m, eth::CONTROL, 1);
    assert_eq!(get(&mut m, eth::STATUS) & 3, 1);
    post(&mut m, 0, &arp(60, 9));
    at(&mut m, 102 * SEND);
    assert_eq!(s.sent().len(), 1, "sent from slot 0 again");
    assert_eq!(s.sent()[0].1, arp(60, 9));
}

/// **`RESET-DEVICES` disables the device and clears status `<2>` and
/// `<3>`** (§2.7), and leaves the configuration and the counters; at
/// power-on it is disabled. `PROG.UNIBUS.RESET` does not reach it.
/// **Fails** a reset that leaves it enabled, or keeps a fault bit.
#[test]
fn reset_devices_disables_it_and_clears_the_fault_bits() {
    let (mut m, s) = enabled(1, 1);
    m.ethernet.rules = double();
    post(&mut m, 0, &judged(1, 0));
    at(&mut m, 2 * SEND);
    put(&mut m, eth::TX_PROD, 7);
    post(&mut m, 1, &arp(60, 1));
    assert_eq!(get(&mut m, eth::STATUS) & 0xf, 0b1001, "enabled, an index fault");
    m.bus_write(RESET_DEVICES, 1);
    let st = get(&mut m, eth::STATUS);
    assert_eq!(st & 0xf, 0b0010, "disabled, quiet, <2> and <3> clear");
    assert_eq!(st >> 24, 1, "the refusal still counted");
    assert_eq!(get(&mut m, eth::TX_BASE), TX_RING);
    at(&mut m, 100 * SEND);
    assert!(s.sent().is_empty(), "the waiting transmit dropped");
    // A refusal at the enable, cleared by the reset.
    put(&mut m, eth::TX_SIZE, 9);
    put(&mut m, eth::CONTROL, 1);
    assert_eq!(get(&mut m, eth::STATUS) & 4, 4);
    m.bus_write(RESET_DEVICES, 1);
    assert_eq!(get(&mut m, eth::STATUS) & 4, 0);
    // PROG.UNIBUS.RESET is not RESET-DEVICES.
    let (mut m, _s) = enabled(1, 1);
    m.bus_reset();
    assert_eq!(get(&mut m, eth::STATUS) & 1, 1, "still enabled");
}

// --- the interrupt ------------------------------------------------------------

/// **Word 100 `<8>` is up while 240 `<8>` is on and 251 ≠ 252** (§2.6): a
/// level, cleared by writing 252 up to 251, by `<8>` off, by a disable and
/// by `RESET-DEVICES`; status `<8>` is the same, ungated; the processor's
/// interrupt is pending with it. Revision 15 never raises it. **Fails** an
/// edge, a bit ungated, or a bit left up.
#[test]
fn word_100_bit_8_is_a_frame_waiting_under_the_interrupt_enable() {
    let raised = || {
        let (mut m, s) = enabled(0, 1);
        give(&mut m, 0, BUFFER_BYTES);
        give(&mut m, 1, BUFFER_BYTES);
        assert_eq!(m.bus_read(INTERRUPTS), 0, "nothing before");
        s.arrive(0, arp(60, 0));
        s.arrive(0, arp(60, 1));
        at(&mut m, 10);
        assert_eq!(m.bus_read(INTERRUPTS), 1 << 8, "word 100 <8> alone");
        assert!(m.interrupt(), "the interrupt pending");
        assert_eq!(get(&mut m, eth::STATUS) & 0x100, 0x100);
        m
    };
    let mut m = raised();
    put(&mut m, eth::RX_TAKEN, 1);
    assert_eq!(m.bus_read(INTERRUPTS), 1 << 8, "one still waiting");
    put(&mut m, eth::RX_TAKEN, 2);
    assert_eq!(m.bus_read(INTERRUPTS), 0, "252 up to 251");
    assert!(!m.interrupt());
    let mut m = raised();
    put(&mut m, eth::CONTROL, 1);
    assert_eq!(m.bus_read(INTERRUPTS), 0, "<8> off");
    assert_eq!(get(&mut m, eth::STATUS) & 0x101, 0x101, "status <8> ungated, still enabled");
    put(&mut m, eth::CONTROL, 0x101);
    assert_eq!(m.bus_read(INTERRUPTS), 1 << 8, "<8> on again");
    put(&mut m, eth::CONTROL, 0);
    assert_eq!(m.bus_read(INTERRUPTS), 0, "a disable");
    let mut m = raised();
    m.bus_write(RESET_DEVICES, 1);
    assert_eq!(m.bus_read(INTERRUPTS), 0, "RESET-DEVICES");
    // Revision 15 has no bit 8.
    let mut m = rev(Geometry::QUUX_15);
    m.attach_ethernet_peer(Box::new(Script::default()));
    for (w, v) in [(0o242, TX_RING), (0o246, RX_RING), (0o240, 0x101), (0o250, 1)] {
        put(&mut m, w, v);
    }
    at(&mut m, 10);
    assert_eq!(m.bus_read(INTERRUPTS), 0);
}

// --- time ------------------------------------------------------------------------

/// **Time is on revision 16's grid** (§2.8): a transmit is sent 20 µs after
/// it is due, at the first clock at or after that, and the next one 20 µs
/// after the first is sent; a received Ethernet frame is delivered at the
/// first clock at or after it arrives, never before. Each instant is
/// checked a clock before and at it. **Fails** a time off the grid, or a
/// device a clock early.
#[test]
fn time_is_on_the_grid() {
    let (mut m, s) = enabled(2, 0);
    let grid = m.time_base();
    let start = 5 * PERIOD + 3;
    m.ns = grid.grid(start);
    post(&mut m, 0, &arp(60, 0));
    post(&mut m, 1, &arp(60, 1));
    let first = grid.grid(m.ns + SEND);
    let second = grid.grid(first + SEND);
    at(&mut m, first - PERIOD);
    assert_eq!(get(&mut m, eth::TX_CONS), 0, "a clock before the first");
    at(&mut m, first);
    assert_eq!(get(&mut m, eth::TX_CONS), 1, "the first at its clock");
    at(&mut m, second - PERIOD);
    assert_eq!(get(&mut m, eth::TX_CONS), 1);
    at(&mut m, 10 * SEND);
    assert_eq!(s.sent().iter().map(|(t, _)| *t).collect::<Vec<_>>(), [first, second]);
    assert!(first % PERIOD == 0 && second % PERIOD == 0, "on the grid");
    // Receive: an arrival between two clocks is delivered at the next.
    give(&mut m, 0, BUFFER_BYTES);
    let arrives = 12 * SEND + 5;
    let delivered = grid.grid(arrives);
    assert_ne!(arrives, delivered, "the arrival is off the grid");
    s.arrive(arrives, arp(60, 2));
    at(&mut m, delivered - PERIOD);
    assert_eq!(get(&mut m, eth::RX_CONS), 0, "a clock before it");
    at(&mut m, delivered);
    assert_eq!(get(&mut m, eth::RX_CONS), 1, "at the first clock after it arrives");
    assert_eq!(m.ethernet.last_delivery(), Some(delivered), "at that clock's instant");
    // An arrival named before the time reached is delivered now.
    put(&mut m, eth::RX_TAKEN, 1);
    give(&mut m, 1, BUFFER_BYTES);
    at(&mut m, 14 * SEND);
    s.arrive(13 * SEND, arp(60, 3));
    assert_eq!(get(&mut m, eth::RX_CONS), 2, "late, at once");
    let t = m.ethernet.last_delivery();
    assert_eq!(t, Some(grid.grid(14 * SEND)), "at the time already reached");
}

/// **Revision 16's time is the file device's rule**: the 20 µs is reckoned
/// exactly in units of 0.5 ns, at every period of revision 15's range, and
/// on a machine of whole nanoseconds it is 20,000 of them.
#[test]
fn a_transmit_takes_20_us_at_every_period() {
    for period in [10, 17, 33, 127] {
        let (mut m, s) = enabled(0, 0);
        m.period = period;
        m.ns = 7 * period;
        post(&mut m, 0, &arp(60, 0));
        at(&mut m, 10 * SEND);
        let want = (7 * period + SEND).div_ceil(period) * period;
        assert_eq!(s.sent()[0].0, want, "period {period}");
    }
}

// --- the checkpoint ------------------------------------------------------------

/// **A checkpoint with transmits waiting resumes and sends them** (§2.9):
/// it is never refused on the device's account; the registers, the
/// counters and the due time come back, and the frames go out at the times
/// an unbroken run sends them. The file is revision 16's version, and
/// revision 15's stays 51. **Fails** a checkpoint that drops the due time,
/// or refuses.
#[test]
fn a_checkpoint_with_transmits_waiting_resumes_and_sends_them() {
    let run = |checkpoint_at: Option<u64>| {
        let (mut m, s) = enabled(2, 1);
        m.ethernet.rules = double();
        give(&mut m, 0, BUFFER_BYTES);
        s.arrive(0, judged(2, 0));
        at(&mut m, 10);
        post(&mut m, 0, &judged(1, 0));
        for k in 1..4 {
            post(&mut m, k, &arp(60, 10 + k as u8));
        }
        at(&mut m, SEND + SEND / 2);
        let (mut m, s) = match checkpoint_at {
            None => (m, s),
            Some(_) => {
                assert_eq!(m.checkpoint_refusal(), None, "never refused on its account");
                let mut w = muir::checkpoint::Writer::new();
                m.save(&mut w);
                let body = w.finish();
                let mut r = muir::checkpoint::Reader::for_word_bits(&body, 40);
                let mut back = Machine::with_geometry(Geometry::QUUX_16, 1);
                back.period = PERIOD;
                back.ethernet.rules = double();
                back.load(&mut r).unwrap();
                assert!(state(&back) == body, "the state back as it was");
                let s = Script::default();
                back.attach_ethernet_peer(Box::new(s.clone()));
                (back, s)
            }
        };
        assert_eq!(get(&mut m, eth::TX_PROD), 4);
        assert_eq!(get(&mut m, eth::TX_CONS), 1);
        at(&mut m, 10 * SEND);
        (s.sent(), counters(&mut m), get(&mut m, eth::RX_CONS))
    };
    let (straight, broken) = (run(None), run(Some(0)));
    assert_eq!(straight.0.len(), 3, "three sent after the refusal");
    assert_eq!(straight.1, [0, 1, 0, 1]);
    assert_eq!(broken, straight, "resumed, the same frames at the same times");
    assert_eq!(muir::checkpoint::version_for(&Geometry::QUUX_16), muir::checkpoint::VERSION_16);
    assert_eq!(muir::checkpoint::version_for(&Geometry::QUUX_15), 51);
    assert_ne!(muir::checkpoint::VERSION_16, 51);
    let mut w = muir::checkpoint::Writer::new();
    rev16().save(&mut w);
    let g = Machine::checkpointed_geometry_at(&w.finish(), 40).unwrap();
    assert_eq!(g, Geometry::QUUX_16, "the body says revision 16");
}

/// **Revision 15's machine state is what it was**: a checkpoint body of a
/// revision-15 machine has no Ethernet device in it, byte for byte the
/// same whatever the device's state, and loads back as revision 15.
#[test]
fn revision_15_s_checkpoint_has_no_ethernet_device() {
    let mut m = rev(Geometry::QUUX_15);
    let s = state(&m);
    m.ethernet.host.mac = [9; 6];
    put(&mut m, 0o242, 0o1000);
    put(&mut m, 0o240, 1);
    assert!(state(&m) == s, "revision 15's body does not carry the device");
    let g = Machine::checkpointed_geometry_at(&s, 40).unwrap();
    assert_eq!(g, Geometry::QUUX_15);
}

// --- the switch -------------------------------------------------------------------

/// A revision-15 `.mcr` whose section 6 says `revision`: a boot PROM, a
/// jump to itself at 36000, or microcode, a jump to itself at 100 and A
/// memory's version at 40.
fn mcr_saying(revision: u32, prom: bool) -> Vec<u8> {
    use muir::mcr;
    let at = if prom { 0o36000 } else { 0o100 };
    let jump = muir::isa::asm::JUMP
        | muir::isa::asm::target(at)
        | muir::isa::asm::ALWAYS
        | muir::isa::asm::N;
    let mut w = vec![mcr::FORMAT_WORD, if prom { 2 } else { 3 }];
    w.extend_from_slice(&[6, 1, 32, 32, 0, 0, 0, 0, revision]);
    w.extend_from_slice(&[1, 1, 64, 64, at as u32, 0, 0, 0, jump as u32, (jump >> 32) as u32]);
    if !prom {
        w.extend_from_slice(&[4, 1, 40, 64, 0o40, 0, 0, 0, 2002, 0o005]);
    }
    let mut b: Vec<u8> = w.iter().flat_map(|x| x.to_le_bytes()).collect();
    b.resize(b.len().div_ceil(mcr::BLOCK_BYTES) * mcr::BLOCK_BYTES, 0);
    b
}

/// Revision 16's boot PROM: a revision-15 `.mcr`, section 6 saying 15,
/// a jump to itself at 36000.
fn prom_16(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("prom-16.mcr");
    std::fs::write(&path, mcr_saying(15, true)).unwrap();
    path
}

/// **Revision 16 loads revision 15's `.mcr`, and only that** (QUUX's
/// Ethernet contract, §10): section 6 names the microinstruction's
/// revision, so one microcode runs on 15 and 16. On 16 a PROM and a
/// microcode file saying 15 are taken, and ones saying 16 or 14 are
/// refused, naming 15; on 15, 15 is taken and 16 refused, as before.
/// **Fails** a reader that takes 15 on the PROM only, or that takes 15 or
/// 16 on 16.
#[test]
fn revision_16_loads_a_revision_15_mcr_and_refuses_others() {
    use muir::mcr::parse_quux_microcode;
    use muir::prom::parse_quux_mcr;
    let (g15, g16) = (Geometry::QUUX_15, Geometry::QUUX_16);
    for g in [g15, g16] {
        let m = parse_quux_microcode(&mcr_saying(15, false), g).unwrap();
        assert_eq!((m.hardware_revision, m.version()), (Some(15), Some(2002)), "{g:?}");
        parse_quux_mcr(&mcr_saying(15, true), g).unwrap();
    }
    for r in [16, 14] {
        for e in [
            parse_quux_microcode(&mcr_saying(r, false), g16).map(|_| ()).unwrap_err(),
            parse_quux_mcr(&mcr_saying(r, true), g16).map(|_| ()).unwrap_err(),
        ] {
            assert!(
                e.contains(&format!("section 6 says hardware revision {r}, and this is revision 16, which loads revision 15's"), ),
                "{r}: {e}"
            );
        }
    }
    for e in [
        parse_quux_microcode(&mcr_saying(16, false), g15).map(|_| ()).unwrap_err(),
        parse_quux_mcr(&mcr_saying(16, true), g15).map(|_| ()).unwrap_err(),
    ] {
        assert!(e.contains("section 6 says hardware revision 16, and this is revision 15"), "{e}");
    }
}

/// **`MUIR_QUUX_REVISION=16` runs revision 16 on both engines**, from a
/// boot PROM whose section 6 says 15 (QUUX's Ethernet contract, §10): the start
/// names it, the checkpoint records it at its own version, and revisions
/// 15 and 16 refuse each other's checkpoints, naming the revision.
#[test]
fn the_switch_runs_revision_16_and_15_and_16_refuse_each_other_s_checkpoints() {
    use support::{Run, quux, scratch, text};
    let dir = scratch("revision-16");
    let prom = prom_16(&dir);
    let chk = dir.join("16.chk");
    for engine in ["--micro", "--rtl"] {
        let out = quux()
            .env("MUIR_QUUX_REVISION", "16")
            .args([engine, "--main-memory-size", "2MW", "--stop-after", "10", "--prom"])
            .arg(&prom)
            .arg("--checkpoint")
            .arg(&chk)
            .run();
        let t = text(&out);
        assert!(out.status.success(), "{engine}: the run failed:\n{t}");
        assert!(t.contains("machine: quux, revision 16: "), "{engine}: {t}");
        let c = muir::checkpoint::read(&chk).unwrap();
        assert_eq!(c.version, muir::checkpoint::VERSION_16, "{engine}");
        let g = Machine::checkpointed_geometry_at(&c.body, c.word_bits).unwrap();
        assert_eq!(g, Geometry::QUUX_16, "{engine}");
    }
    let out = quux()
        .env("MUIR_QUUX_REVISION", "15")
        .args(["--rtl", "--main-memory-size", "2MW", "--stop-after", "10", "--resume"])
        .arg(&chk)
        .run();
    let t = text(&out);
    assert_eq!(out.status.code(), Some(2), "15 resumed 16:\n{t}");
    assert!(t.contains("is revision 16's, and this is revision 15"), "{t}");
    let out = quux()
        .env("MUIR_QUUX_REVISION", "16")
        .args(["--rtl", "--main-memory-size", "2MW", "--stop-after", "10", "--resume"])
        .arg(&chk)
        .run();
    assert!(out.status.success(), "16 resumes 16: {}", text(&out));
}
