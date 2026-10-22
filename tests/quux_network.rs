// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's network device (contract Q4): the CADR's Chaosnet interface,
//! its registers in the same order and with the same bits, at words
//! 140-145 of the register page (`1777777400`), where the CADR has them at
//! Unibus `764140`-`764152`: word 140 + k is Unibus `764140` + 2k. Only
//! its five real registers are decoded (contract Q13): 144, 146 and 147
//! are reserved, where the CADR's board answers aliases. Its interrupt is
//! word 100's `<6>` (contract Q13).

use muir::chaos::interface::{self, csr};
use muir::chaos::packet::{Packet, op};
use muir::machine::{Geometry, Machine, bus_error};

mod support;

const PAGE: u32 = muir::machine::REGISTER_PAGE_13;
const INTERRUPTS: u32 = PAGE + 0o100;
const NET: u32 = PAGE + 0o140;

/// Unibus `u`'s physical address, as the CADR reaches it.
fn unibus(u: u32) -> u32 {
    muir::busint::unibus_physical(u)
}

fn quux() -> Machine {
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
    m
}

/// **Each word is its Unibus register**: the same writes and reads, made
/// on a CADR through its Unibus and on QUUX through the page (QUUX having
/// no Unibus, contract Q5), give
/// the same answers --- the CSR's writable bits, the address, the write
/// buffer's words and the bit count they make, the read buffer, and START
/// --- at the five words QUUX decodes, 140-143 and 145.
#[test]
fn each_word_is_its_unibus_register() {
    let (mut a, mut b) = (Machine::new(), quux());
    let at = |k: u32| (unibus(interface::CSR + 2 * k), NET + k);
    let mut both = |k: u32, write: Option<u32>| {
        let (u, p) = at(k);
        match write {
            Some(v) => {
                a.bus_write(u, v.into());
                b.bus_write(p, v.into());
                (0, 0)
            }
            None => (a.bus_read(u) & 0xffff, b.bus_read(p)),
        }
    };
    let enables = (csr::RECEIVE_INT_ENABLE | csr::TRANSMIT_INT_ENABLE) as u32;
    let mut reads = Vec::new();
    both(0, Some(enables));
    for k in [0, 1, 2, 3, 5] {
        reads.push(both(k, None));
    }
    for w in [0o400, 4, 0o3060, 0, 0o3050, 0o21, 1, 0, 0o3060] {
        both(1, Some(w));
    }
    for k in [0, 1, 2, 3, 5] {
        reads.push(both(k, None));
    }
    for (n, (u, p)) in reads.iter().enumerate() {
        assert_eq!(u, p, "read {n}: Unibus {u:o}, page {p:o}");
    }
    assert_ne!(reads[0].1 & u64::from(enables), 0, "the enables read back");
    assert_eq!(
        reads[1].1,
        (a.ioboard.chaos.as_ref().unwrap().address() as u32).into(),
        "my address"
    );
    assert_eq!(b.bus_error & bus_error::XBUS_NXM, 0, "the page answered");
}

/// **A frame goes out and its answer comes back through the page**: a
/// STATUS request to muir's own Chaosnet server, its words written to word
/// 141 and sent by a read of 145, is answered; `RECEIVE DONE` comes up in
/// word 140, word 100's `<6>` with the receive enable, and the answer's
/// words come out of 142.
#[test]
fn a_frame_goes_out_and_its_answer_comes_back() {
    let (me, host) = (0o177201u16, 0o177200u16);
    let mut m = quux();
    m.chaos.address = me;
    let root = support::scratch("quux-network");
    support::ChaosServer::new(host)
        .serving(root.path().to_path_buf())
        .at_time(support::time::TEST_UNIVERSAL)
        .plug(&mut m, 0);
    m.bus_write(NET, ((csr::RESET | csr::CLEAR_RECEIVER) as u32).into());
    m.bus_write(NET, (csr::RECEIVE_INT_ENABLE as u32).into());
    let rfc = Packet {
        opcode: op::RFC,
        forward: 0,
        dest: host,
        dest_index: 0,
        source: me,
        source_index: 0o21,
        number: 1,
        ack: 0,
        data: b"STATUS".to_vec(),
    };
    for w in rfc.to_buffer(host) {
        m.bus_write(NET + 1, (w as u32).into());
    }
    m.bus_read(NET + 5);
    let mut done = false;
    for _ in 0..20_000 {
        m.ns += 5_000;
        m.ioboard.advance(m.ns);
        if m.bus_read(NET) & u64::from(csr::RECEIVE_DONE as u32) != 0 {
            done = true;
            break;
        }
    }
    assert!(done, "no answer came");
    assert_ne!(m.bus_read(INTERRUPTS) & 1 << 6, 0, "the network's interrupt");
    // And it interrupts the processor through word 100 alone, as every
    // bit there does, with the Unibus interrupt's enable (766040) never
    // written: muir-sys's microcode for Q5 writes no Unibus address.
    assert_eq!(
        m.interrupt_status & muir::busint::interrupt_status::ENABLE_UB_INTS,
        0,
        "no Unibus interrupt enable"
    );
    assert!(m.interrupt(), "the processor's interrupt pending");
    let bits = m.bus_read(NET + 3) as usize + 1;
    let words: Vec<u16> = (0..bits / 16).map(|_| m.bus_read(NET + 2) as u16).collect();
    // The received buffer ends in the destination, the source and the
    // check word (AIM-628); the packet is what comes before the last two.
    let (ans, _) = Packet::from_buffer(&words[..words.len() - 2]).expect("a packet");
    assert_eq!((ans.opcode, ans.source, ans.dest), (op::ANS, host, me), "{ans:?}");
}

/// **Only the five real registers are decoded** (contract Q13): 144,
/// 146 and 147 read 0, where the CADR's board answers the CSR at `764150`
/// and the bit count at `764156`; and a write of 142, 143, 144, 145, 146 or
/// 147 changes nothing in the machine, where the board takes the CSR at
/// `764150` and the write buffer at `764152`. 140 and 141 are written: the
/// CSR's enables read back, and a word written to 141 changes the machine.
#[test]
fn only_the_five_registers_are_decoded() {
    let state = |m: &Machine| {
        let mut w = muir::checkpoint::Writer::new();
        m.save(&mut w);
        w.finish()
    };
    let mut m = quux();
    m.bus_write(NET, (csr::RECEIVE_INT_ENABLE as u32).into());
    let csr_now = m.bus_read(NET);
    assert_ne!(csr_now, 0, "the CSR reads something to alias");
    for k in [4, 6, 7] {
        assert_eq!(m.bus_read(NET + k), 0, "word {:o} is reserved", 0o140 + k);
    }
    for k in [2, 3, 4, 5, 6, 7] {
        let s = state(&m);
        m.bus_write(NET + k, 0o177777);
        assert!(state(&m) == s, "a write of word {:o} changed the machine", 0o140 + k);
    }
    assert_eq!(m.bus_read(NET), csr_now, "the CSR as it was");
    assert_eq!(m.bus_error, 0, "every word answered");
    let s = state(&m);
    m.bus_write(NET + 1, 0o1234);
    assert!(state(&m) != s, "141 written: the write buffer took the word");
    m.bus_write(NET, 0);
    assert_eq!(m.bus_read(NET) & u64::from(csr::RECEIVE_INT_ENABLE as u32), 0, "140 written");
}

/// **The CADR has none of it on the page**: word 140 is in its Unibus
/// window, where nothing answers, its Chaosnet being at `764140`.
#[test]
fn the_cadr_has_it_on_the_unibus_only() {
    let mut m = Machine::new();
    m.bus_read(NET);
    assert_eq!(m.bus_error, bus_error::UNIBUS_NXM);
}

// --- Revision 15: the network's times on the clock's grid ----------------

/// Revision 15's register page, its network at words 140-147.
const NET_15: u32 = muir::tlb::REGISTER_PAGE_BUS | 0o140;

/// The bits of a three-word frame on the cable: the words, the source and
/// the check word, sixteen bits each, and the zero after them
/// (`muir::chaos::packet::frame`).
const BITS: u64 = 5 * 16 + 1;

/// The clocks at which a revision-15 machine's CSR, read at every clock's
/// instant as the engines read it, first shows the cable busy (bit 14),
/// Transmit Done back and Receive Done, for a three-word frame to itself
/// under Loop Back: the CSR written at clock 1, the words at clocks 2-4,
/// START read at clock `start`. The CSR is read at every clock from
/// `start + 1` to `to`; a bit not seen by then is `None`.
fn looped_back_15(period: u64, start: u64, to: u64) -> [Option<u64>; 3] {
    let mut m = Machine::with_geometry(Geometry::QUUX_15, 1);
    m.period = period;
    let me = m.bus_read(NET_15 + 1) as u16;
    m.ns = period;
    m.bus_write(NET_15, u64::from(csr::LOOP_BACK));
    for (k, w) in [0o1234u16, 0o5670, me].into_iter().enumerate() {
        m.ns = (2 + k as u64) * period;
        m.bus_write(NET_15 + 1, u64::from(w));
    }
    m.ns = start * period;
    assert_eq!(m.bus_read(NET_15 + 5) as u16, me, "START reads the address");
    let mut seen = [None; 3];
    for clock in start + 1..=to {
        m.ns = clock * period;
        let r = m.bus_read(NET_15) as u16;
        for (k, bit) in
            [csr::CRC_ERROR, csr::TRANSMIT_DONE, csr::RECEIVE_DONE].into_iter().enumerate()
        {
            if r & bit != 0 && seen[k].is_none() {
                seen[k] = Some(clock);
            }
        }
        if seen.iter().all(Option::is_some) {
            break;
        }
    }
    seen
}

/// MP2b ruling Q19 as written, in units of 0.5 ns at a period of `p`
/// units: the turn timer's count whose true edge is `edge` acts at the
/// first clock at or after it; the frame starts ⌈2·468/P⌉ clocks after
/// that (`TURN_START_NS`); Transmit Done comes ⌈2(bits·250 − 500)/P⌉
/// clocks after the frame's start and Receive Done ⌈2(bits·250 + 375)/P⌉
/// (`TDONE_BEFORE_END_NS`, `CBLBSY_OFF_NS`), each rounded once from the
/// frame's own start. The clocks at which the cable goes busy, Transmit
/// Done and Receive Done come.
fn by_the_rule(p: u64, edge: u64) -> [Option<u64>; 3] {
    let after = |start: u64, units: u64| start + units.div_ceil(p) * p;
    let acts = edge.div_ceil(p) * p;
    let first_edge = after(acts, 2 * 468);
    let tdone = after(first_edge, 2 * (BITS * 250 - 500));
    let rdone = after(first_edge, 2 * (BITS * 250 + 375));
    [first_edge / p, tdone / p, rdone / p].map(Some)
}

/// The true edge of the turn timer's `k`th count, in units of 0.5 ns, on
/// an interface powered at 0: 4,250 ns and every 500 from there
/// (`TURN_FIRST_TC_NS`, `TURN_TC_NS`). Bit 7 rises on counts 0, 512, 1024
/// and so on while the cable is idle.
fn turn_edge(k: u64) -> u64 {
    2 * (4_250 + 500 * k)
}

/// **A frame's delivery at 8.5 ns lands at the clock the rule gives**
/// (MP2b ruling Q19's test): START read at clock 11 (93.5 ns, an odd
/// unit), the turn timer's first count at 4,250 ns, clock 500, the frame
/// on the cable 56 clocks later, Transmit Done 2,324 clocks after its
/// start and Receive Done 2,427: clocks 556, 2,880 and 2,983. **Fails**
/// a time in whole ns, which brings both a clock early (2,879, 2,982).
#[test]
fn a_frame_s_delivery_at_8_5_ns_lands_at_the_rule_s_clock() {
    let want = by_the_rule(17, turn_edge(0));
    assert_eq!(want, [Some(556), Some(2_880), Some(2_983)], "the rule's clocks");
    assert_eq!(looped_back_15(17, 11, 4_000), want);
}

/// **The same at 19 ns**, a period of 38 units, where the first count's
/// edge at 4,250 ns falls between clocks 223 and 224: the frame on the
/// cable at clock 249, Transmit Done at 1,289 and Receive Done at 1,335.
/// **Fails** a time in whole ns (1,288 and 1,334).
#[test]
fn a_frame_s_delivery_at_19_ns_lands_at_the_rule_s_clock() {
    let want = by_the_rule(38, turn_edge(0));
    assert_eq!(want, [Some(249), Some(1_289), Some(1_335)], "the rule's clocks");
    assert_eq!(looped_back_15(38, 5, 2_000), want);
}

/// **The same at 48 and 63.5 ns**, periods of 96 and 127 units, 127 the
/// longest the fabric's period field holds (A15b.12). The count is bit
/// 7's second rise, count 512, at 260,250 ns, between clocks at both, with
/// START read early enough to be ready by it. At 48 ns the frame on
/// the cable at clock 5,432, Transmit Done at 5,844 and Receive Done at
/// 5,862; at 63.5 ns at 4,107, 4,419 and 4,432.
#[test]
fn a_frame_s_delivery_at_48_and_63_5_ns_lands_at_the_rule_s_clock() {
    for (p, start, want) in
        [(96, 5_300, [5_432, 5_844, 5_862]), (127, 4_000, [4_107, 4_419, 4_432])]
    {
        let edge = turn_edge(512);
        assert_ne!(edge % p, 0, "{p}: the edge falls between clocks");
        assert_eq!(by_the_rule(p, edge), want.map(Some), "{p}: the rule's clocks");
        assert_eq!(looped_back_15(p, start, want[2] + 100), want.map(Some), "at {p} units");
    }
}

/// **The turn timer acts at the first clock at or after its edge**, here
/// one that falls between clocks: at 8.5 ns bit 7's second rise is count
/// 512, whose edge at 260,250 ns is 6 units before clock 30,618. START
/// read at clock 30,129 is ready 489 clocks on (`TSR_READY_NS`), at clock
/// 30,618 itself: after the edge but not after the clock the count is
/// acted on, so the frame goes at this count, on the cable 56 clocks
/// later at 30,674, with Transmit Done and Receive Done from there.
/// **Fails** a timer acting at the true edge, which finds the frame not
/// ready and holds it a round, and a time in whole ns (the frame on the
/// cable at 30,673).
#[test]
fn the_turn_timer_acts_at_the_first_clock_at_or_after_its_edge() {
    let edge = turn_edge(512);
    assert_eq!(edge % 17, 11, "the edge falls between clocks");
    assert_eq!(30_129 * 17 + 489 * 17, edge.div_ceil(17) * 17, "ready at the count's clock");
    let want = by_the_rule(17, edge);
    assert_eq!(want, [Some(30_674), Some(32_998), Some(33_101)], "the rule's clocks");
    assert_eq!(looped_back_15(17, 30_129, 34_000), want);
}

/// **The frame goes on the cable at the clock the rule gives**: at 8.5 ns,
/// with a cable plugged in and no Loop Back, START read at clock 11, the
/// turn timer's first count at clock 500, the frame's first edge 56
/// clocks on at clock 556, 9,452 units, on the cable's own record; its
/// Transmit Done at clock 2,880 as looped back. **Fails** a start delay
/// added exactly from the count (9,436, between clocks), and a cable in
/// whole ns.
#[test]
fn the_frame_goes_on_the_cable_at_the_rule_s_clock() {
    use muir::chaos::ether::Event;
    let mut m = Machine::with_geometry(Geometry::QUUX_15, 1);
    m.period = 17;
    m.plug_chaos(0);
    let c = m.ioboard.chaos.as_mut().unwrap();
    c.ether_mut().unwrap().keep_log(true);
    let me = m.bus_read(NET_15 + 1) as u16;
    m.ns = 17;
    m.bus_write(NET_15, 0);
    let words = [0o1234u16, 0o5670, me];
    for (k, w) in words.into_iter().enumerate() {
        m.ns = (2 + k as u64) * 17;
        m.bus_write(NET_15 + 1, u64::from(w));
    }
    m.ns = 11 * 17;
    m.bus_read(NET_15 + 5);
    let mut transmit_done = None;
    for clock in 12..4_000u64 {
        m.ns = clock * 17;
        if m.bus_read(NET_15) as u16 & csr::TRANSMIT_DONE != 0 {
            transmit_done = Some(clock);
            break;
        }
    }
    let want = by_the_rule(17, turn_edge(0));
    let log = &m.ioboard.chaos.as_ref().unwrap().ether().unwrap().log;
    let sent: Vec<u64> = log
        .iter()
        .filter_map(|e| match e {
            Event::Sent(at, from, buffer) if *from == me && buffer[..] == words => Some(*at),
            _ => None,
        })
        .collect();
    assert_eq!(sent, [want[0].unwrap() * 17], "the frame's first edge, in units");
    assert_eq!(transmit_done, want[1], "Transmit Done");
}

/// **A delay started at an odd unit ends ⌈2d/P⌉ clocks after its start,
/// not a clock early.** At 21.5 ns, 43 units, START's 4,150 ns
/// (`TSR_READY_NS`) are ⌈8,300/43⌉ = 194 clocks. Bit 7 rises at count
/// 14,848, whose edge is on a clock, 7,428,250 ns. START read 194 clocks
/// before it is ready at the count and goes then; read 193 clocks before,
/// at an odd unit, it is ready a clock after the count and waits for the
/// next rise, count 15,360. **Fails** a time in whole ns, where the
/// START at the odd unit is ready 0.5 ns early, at the count itself, and
/// goes 512 counts too soon.
#[test]
fn a_delay_started_at_an_odd_unit_does_not_end_a_clock_early() {
    let p = 43;
    let edge = turn_edge(14_848);
    assert_eq!(edge % p, 0, "the count's edge is on a clock");
    let count = edge / p;
    // Read 194 clocks before the count: the frame goes at it.
    let early = count - 194;
    let want = by_the_rule(p, edge);
    let seen = looped_back_15(p, early, want[0].unwrap());
    assert_eq!(seen[0], want[0], "START 194 clocks before the count: the frame goes at it");
    // Read 193 clocks before, at an odd unit: it goes at the next rise.
    let odd = count - 193;
    assert_eq!(odd * p % 2, 1, "START at an odd unit");
    let next = by_the_rule(p, turn_edge(15_360));
    let seen = looped_back_15(p, odd, next[0].unwrap());
    assert_eq!(seen[0], next[0], "START 193 clocks before the count: the frame waits a round");
}

// --- Revision 16: no Chaosnet interface (QUUX's Ethernet contract, §8 R4) --

/// The register page of a machine of `g`: revision 13's or, from 14, the
/// bus's.
fn page_of(g: Geometry) -> u32 {
    if g.paged() { muir::tlb::REGISTER_PAGE_BUS } else { muir::machine::REGISTER_PAGE_13 }
}

/// A machine of `g` as `quux` builds one: its Chaosnet plugged in by
/// [`Machine::plug_chaos`], at revision 15's period from 15 on.
fn plugged(g: Geometry) -> Machine {
    let mut m = Machine::with_geometry(g, 1);
    if g.extended() {
        m.period = 17;
    }
    m.plug_chaos(0);
    m
}

/// A machine built as the CADR and given revision 16's geometry after, its
/// I/O board's Chaosnet interface still on it: `quux`'s debuggee in the
/// lashup is built so before [`Machine::plug_chaos`]. 16's page and word
/// 100 must not reach the interface even then.
fn regeared_16() -> Machine {
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX_16;
    m.period = 17;
    assert!(m.ioboard.chaos.is_some(), "the CADR's board has its interface");
    m
}

fn body(m: &Machine) -> Vec<u8> {
    let mut w = muir::checkpoint::Writer::new();
    m.save(&mut w);
    w.finish()
}

/// What words 140-157 and word 100 `<6>` show of a machine of `g`: each
/// of 140-157 read at power-on; then the CSR written with Loop Back and
/// both interrupt enables, read back with word 100 `<6>` and the
/// processor's interrupt; then a three-word frame to itself written to
/// 141, START read, and the machine's time stepped a microsecond at a
/// time until the CSR shows Receive Done or 2 ms have passed, with the
/// bit count, word 100 `<6>` and the interrupt after it.
fn network_signature(g: Geometry) -> Vec<u32> {
    signature_of(plugged(g))
}

/// [`network_signature`] of the machine `m`.
fn signature_of(mut m: Machine) -> Vec<u32> {
    let page = page_of(m.geometry);
    let mut out: Vec<u32> = (0o140..=0o157).map(|k| m.bus_read(page + k) as u32).collect();
    let net = page + 0o140;
    let enables = csr::LOOP_BACK | csr::RECEIVE_INT_ENABLE | csr::TRANSMIT_INT_ENABLE;
    m.bus_write(net, u64::from(enables));
    out.push(m.bus_read(net) as u32);
    out.push((m.bus_read(page + 0o100) as u32) >> 6 & 1);
    out.push(m.interrupt() as u32);
    let me = m.bus_read(net + 1) as u16;
    for w in [0o1234u16, 0o5670, me] {
        m.bus_write(net + 1, u64::from(w));
    }
    out.push(m.bus_read(net + 5) as u32);
    let per_us = 1_000 * m.time_base().per_ns;
    let mut steps = 0;
    while steps < 2_000 && m.bus_read(net) as u16 & csr::RECEIVE_DONE == 0 {
        m.ns += per_us;
        m.advance_ioboard();
        steps += 1;
    }
    out.push(steps);
    out.push(m.bus_read(net) as u32);
    out.push(m.bus_read(net + 3) as u32);
    out.push((m.bus_read(page + 0o100) as u32) >> 6 & 1);
    out.push(m.interrupt() as u32);
    out
}

/// **Revisions 13-15 keep their Chaosnet interface as released** (QUUX's
/// Ethernet contract, §8: "Revisions 13-15 and the CADR keep their Chaos
/// as released"): words 140-157 and word 100 `<6>` show the same on all
/// three, and what the build before R4 showed: 141 the address 177001,
/// the CSR's enables read back with `<6>` down, and the frame looped back
/// as Receive Done after 26 us, with a bit count of 117 and `<6>` up and
/// the processor interrupted. **Fails** a revision below 16 that lost the interface,
/// or any of its words.
#[test]
fn revisions_13_to_15_keep_140_to_157_and_bit_6_as_before() {
    #[rustfmt::skip]
    const BEFORE: [u32; 25] = [
        // 140-157 at power-on.
        0o200, 0o177001, 1, 0, 0, 0o177001, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0,
        // The CSR after the enables, <6>, the interrupt; START.
        0o62, 0, 0, 0o177001,
        // Microseconds to Receive Done, the CSR, the bit count, <6>, the interrupt.
        26, 0o100262, 0o117, 1, 1,
    ];
    for g in [Geometry::QUUX, Geometry::QUUX_14, Geometry::QUUX_15] {
        assert_eq!(network_signature(g), BEFORE, "{g:?}");
    }
}

/// Checks that words `words` of the machines `make` makes behave as
/// reserved: each reads 0 and answers, and writes of all ones, 1 and 0x101
/// change nothing in the machine and leave it reading 0.
fn reserved(make: impl Fn() -> Machine, words: std::ops::RangeInclusive<u32>) {
    for k in words {
        let mut m = make();
        let (g, page) = (m.geometry, page_of(m.geometry));
        assert_eq!(m.bus_read(page + k), 0, "{g:?}: word {k:o} reads 0");
        let s = body(&m);
        for v in [!0u32, 1, 0x101] {
            m.bus_write(page + k, u64::from(v));
        }
        assert!(body(&m) == s, "{g:?}: a write of word {k:o} changed the machine");
        assert_eq!(m.bus_read(page + k), 0, "{g:?}: word {k:o} after the writes");
        assert_eq!(m.bus_error, 0, "{g:?}: word {k:o} answered");
    }
}

/// **On revision 16, 140-157 are reserved, as 240-257 are on 15** (QUUX's
/// Ethernet contract, §8 R4, §10): each reads 0 and answers, and a write
/// changes nothing; the same check holds 240-257 on 15. On a machine
/// given 16's geometry with the interface still on its board too.
/// **Fails** a revision-16 page that still decodes the interface's words,
/// read or written.
#[test]
fn on_revision_16_140_to_157_are_reserved_as_240_to_257_are_on_15() {
    reserved(|| plugged(Geometry::QUUX_15), 0o240..=0o257);
    reserved(|| plugged(Geometry::QUUX_16), 0o140..=0o157);
    reserved(regeared_16, 0o140..=0o157);
}

/// **On revision 16 nothing sets word 100 `<6>`** (§8 R4): there is no
/// interface on the I/O board, even after [`Machine::plug_chaos`]; the
/// enables and a frame written to 140 and 141 raise neither `<6>` nor the
/// processor's interrupt over 2 ms; and every word of 140-157 reads 0
/// throughout. And on a machine given 16's geometry with the interface
/// still on its board, whose request, raised by the enables there, reaches
/// neither `<6>` nor the processor. **Fails** an interface left on 16's
/// board, or `<6>` or the interrupt raised by one.
#[test]
fn on_revision_16_nothing_sets_word_100_bit_6() {
    let m = plugged(Geometry::QUUX_16);
    assert!(m.ioboard.chaos.is_none(), "revision 16's I/O board has no Chaosnet interface");
    let mut want = vec![0u32; 0o20];
    // The CSR, <6>, the interrupt and START all 0; Receive Done never
    // comes; the CSR, the bit count, <6> and the interrupt 0 after.
    want.extend([0, 0, 0, 0, 2_000, 0, 0, 0, 0]);
    assert_eq!(network_signature(Geometry::QUUX_16), want);
    // The interface on the board, its transmit interrupt enabled
    // directly: its request is up, and still not <6>.
    let mut m = regeared_16();
    let now = m.ns;
    let r = muir::chaos::interface::CSR;
    m.ioboard.write(r, csr::TRANSMIT_INT_ENABLE | csr::RECEIVE_INT_ENABLE, now);
    m.ns += 1_000_000;
    m.advance_ioboard();
    assert!(
        m.ioboard.chaos.as_ref().unwrap().interrupt_request().is_some(),
        "the interface requests an interrupt"
    );
    assert_eq!(m.interrupt_sources() >> 6 & 1, 0, "word 100 <6>");
    assert!(!m.interrupt(), "the processor's interrupt");
    assert_eq!(signature_of(regeared_16()), want);
}
