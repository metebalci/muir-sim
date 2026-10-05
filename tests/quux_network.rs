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
