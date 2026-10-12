// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! MIT's own test of the Chaosnet interface, `CHAOS:CHATST` in
//! `sys/network/chaos/chatst.lisp`, run by System 100 on `rtl`.
//!
//! For each of its six bit patterns CHATST sends a packet looped back
//! inside the interface, and then one on the cable --- to the interface's
//! own address, with Receive All off (`CHATST-PREP NIL` writes 10 to the
//! CSR) --- and busy-waits for Receive Done (`CHATST-RCV T`) before it
//! goes on. So the cable half passes only on an interface that takes its
//! own frame off the cable, as the netlist board does:
//! `the_board_takes_its_own_frame_off_the_cable_on_both_alike` in
//! `tests/chaos_rtl.rs` measures the board, and the behavioral interface
//! is held to it there. An interface that does not hangs CHATST at its
//! first packet on the cable.

use muir::chaos::ether::Event;
use muir::engine::Engine;
use muir::rtl::Rtl;
use muir::terminal::keyboard::Keyboard;

mod support;
use support::{
    CHAOS_100, ChaosServer, file_root, machine_with_pack, pack_100, time, type_at,
    wait_for_the_prompt,
};

/// The frames the machine sent to its own address on the cable.
fn to_itself<E: Engine>(e: &E) -> usize {
    let ether = e.machine().ioboard.chaos.as_ref().unwrap().ether().unwrap();
    ether
        .log
        .iter()
        .filter(|ev| {
            matches!(ev, Event::Sent(_, from, b) if *from == CHAOS_100.0 && b.last() == Some(&CHAOS_100.0))
        })
        .count()
}

/// **CHATST's cable half passes**: `(chaos:chatst 1 1)`, one packet a
/// pattern each way, sends all six of its packets on the cable, each
/// after the last came back. Fails an interface that does not take its
/// own frame: CHATST waits for the first for ever, and one frame goes out.
#[test]
fn chatst_sends_all_six_patterns_on_the_cable() {
    let (Some(pack), Some(root)) = (pack_100(), file_root()) else {
        return;
    };
    let mut e = Rtl::new(machine_with_pack(&pack));
    e.boot();
    let m = e.machine_mut();
    m.chaos.address = CHAOS_100.0;
    ChaosServer::new(CHAOS_100.1).serving(root).at_time(time::TEST_UNIVERSAL).plug(m, 0);
    m.ioboard.chaos.as_mut().unwrap().ether_mut().unwrap().keep_log(true);
    wait_for_the_prompt(&mut e);
    let before = to_itself(&e);
    let mut k = Keyboard::new();
    type_at(&mut e, &mut k, "(chaos:chatst 1 1)");
    // Six patterns, each a sixtieth of a second's sleep for the looped
    // back packet and a moment for the one on the cable.
    let mut ran = 0u64;
    while to_itself(&e) - before < 6 && ran < 200_000_000 {
        for _ in 0..1_000_000 {
            e.step().expect("the machine halted");
        }
        ran += 1_000_000;
    }
    let png = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vendor/run/chatst.png");
    std::fs::write(&png, e.machine().tv.png()).unwrap();
    eprintln!("CHATST's screen at {}, {ran} microcycles on", png.display());
    assert_eq!(to_itself(&e) - before, 6, "CHATST's six packets to itself on the cable");
}
