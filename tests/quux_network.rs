// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX has no Chaosnet interface (QUUX's Ethernet contract, §8 R4): the
//! register page's words 140-157 are reserved, never reused, and nothing
//! sets word 100 `<6>`. The CADR's interface, on its Unibus, is
//! `tests/chaos*.rs`'s.

use muir::chaos::interface::csr;
use muir::machine::{Geometry, Machine};

mod support;

/// The register page.
const PAGE: u32 = muir::tlb::REGISTER_PAGE_BUS;

/// QUUX as `quux` builds it: its Chaosnet plugged in by
/// [`Machine::plug_chaos`], which leaves none, at a period of 8.5 ns.
fn plugged() -> Machine {
    let mut m = Machine::with_geometry(Geometry::QUUX, 1);
    m.period = 17;
    m.plug_chaos(0);
    m
}

/// A machine built as the CADR and given QUUX's geometry after, its
/// I/O board's Chaosnet interface still on it: `quux`'s debuggee in the
/// lashup is built so before [`Machine::plug_chaos`]. QUUX's page and word
/// 100 must not reach the interface even then.
fn regeared_16() -> Machine {
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
    m.period = 17;
    assert!(m.ioboard.chaos.is_some(), "the CADR's board has its interface");
    m
}

fn body(m: &Machine) -> Vec<u8> {
    let mut w = muir::checkpoint::Writer::new();
    m.save(&mut w);
    w.finish()
}

/// What words 140-157 and word 100 `<6>` show of QUUX: each
/// of 140-157 read at power-on; then the CSR written with Loop Back and
/// both interrupt enables, read back with word 100 `<6>` and the
/// processor's interrupt; then a three-word frame to itself written to
/// 141, START read, and the machine's time stepped a microsecond at a
/// time until the CSR shows Receive Done or 2 ms have passed, with the
/// bit count, word 100 `<6>` and the interrupt after it.
fn network_signature() -> Vec<u32> {
    signature_of(plugged())
}

/// [`network_signature`] of the machine `m`.
fn signature_of(mut m: Machine) -> Vec<u32> {
    let page = PAGE;
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

/// Checks that words `words` of the machines `make` makes behave as
/// reserved: each reads 0 and answers, and writes of all ones, 1 and 0x101
/// change nothing in the machine and leave it reading 0.
fn reserved(make: impl Fn() -> Machine, words: std::ops::RangeInclusive<u32>) {
    for k in words {
        let mut m = make();
        let (g, page) = (m.geometry, PAGE);
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

/// **Words 140-157 are reserved** (QUUX's Ethernet contract, §8 R4, §10):
/// each reads 0 and answers, and a write changes nothing; on a machine
/// given QUUX's geometry with the interface still on its board too. The
/// control: the check refuses words 240-257, the Ethernet device's.
/// **Fails** a page that still decodes the interface's words, read or
/// written.
#[test]
fn words_140_to_157_are_reserved() {
    reserved(plugged, 0o140..=0o157);
    reserved(regeared_16, 0o140..=0o157);
    let ethernet = std::panic::catch_unwind(|| reserved(plugged, 0o240..=0o257));
    assert!(ethernet.is_err(), "the check passes the Ethernet device's words");
}

/// **Nothing sets word 100 `<6>`** (§8 R4): there is no
/// interface on the I/O board, even after [`Machine::plug_chaos`]; the
/// enables and a frame written to 140 and 141 raise neither `<6>` nor the
/// processor's interrupt over 2 ms; and every word of 140-157 reads 0
/// throughout. And on a machine given QUUX's geometry with the interface
/// still on its board, whose request, raised by the enables there, reaches
/// neither `<6>` nor the processor. **Fails** an interface left on QUUX's
/// board, or `<6>` or the interrupt raised by one.
#[test]
fn nothing_sets_word_100_bit_6() {
    let m = plugged();
    assert!(m.ioboard.chaos.is_none(), "QUUX's I/O board has no Chaosnet interface");
    let mut want = vec![0u32; 0o20];
    // The CSR, <6>, the interrupt and START all 0; Receive Done never
    // comes; the CSR, the bit count, <6> and the interrupt 0 after.
    want.extend([0, 0, 0, 0, 2_000, 0, 0, 0, 0]);
    assert_eq!(network_signature(), want);
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
