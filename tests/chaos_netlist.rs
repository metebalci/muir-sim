// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The I/O board's Chaosnet half, run as MIT built it: the thirteen
//! `chaos/lispm` pages of `data/CADRIO.netlist`, driven through the
//! interface's registers by a Unibus master, and held to AIM-628 §7.
//!
//! These are the checks that come before any cable model: that the board
//! knows its own address, and that a packet written into its transmitter
//! comes out of its receiver with the check word right, looped back
//! inside the board with the cable unused.

use muir::chaos::interface::{self as chaos, csr};
use muir::netlist::{self, Netlist};
use muir::part::Level;
use muir::unibus::UnibusMaster;

mod support;
use support::quiet;

const CADRIO: &str = include_str!("../data/CADRIO.netlist");

fn cadrio() -> Netlist {
    netlist::parse_wired(CADRIO).unwrap()
}

/// This machine's address in the band's host table, `MIT-LISPM-1`.
const MY_ADDRESS: u16 = 0o3050;

/// A board with its switches set to [`MY_ADDRESS`], reset and settled.
fn board(n: &Netlist) -> UnibusMaster<'_> {
    let mut b = UnibusMaster::new(n, 10_000, &quiet());
    for (reference, closed) in chaos::switches(MY_ADDRESS) {
        b.chip.set_switches(reference, closed);
    }
    b.run(b.now + 2_000);
    b
}

/// **The address switches read back as the address.** `MY ADDRESS` at
/// 764142 is "the network address of this interface (which is contained
/// in a set of DIP switches on the board)"; set for 3050, it reads 3050,
/// which also settles which way round the switches are.
#[test]
fn the_address_switches_read_back() {
    let n = cadrio();
    let mut b = board(&n);
    let (took, word) = b.cycle(chaos::MY_ADDRESS, None);
    eprintln!("MY ADDRESS reads {word:#o} in {took} ns");
    assert_eq!(word, MY_ADDRESS, "the address the switches were set to");
    // And another address, so it is not a coincidence of the bit pattern.
    for (reference, closed) in chaos::switches(0o3060) {
        b.chip.set_switches(reference, closed);
    }
    b.run(b.now + 2_000);
    let (_, word) = b.cycle(chaos::MY_ADDRESS, None);
    assert_eq!(word, 0o3060, "switched to 3060");
    // The start-transmission address reads the same address.
    // (Not read here: it would start a transmission of an empty buffer.)
}

/// The words of a packet as the software writes them: the eight header
/// words, the data, then the destination.
fn rfc_time(from: u16, to: u16) -> Vec<u16> {
    let text = b"TIME";
    let mut words = vec![
        1 << 8,            // Operation: RFC, opcode 1 in the high byte
        text.len() as u16, // Count: forwarding count 0, byte count
        to,                // Destination address
        0,                 // Destination index: none yet
        from,              // Source address
        0o21,              // Source index
        1,                 // Packet number
        0,                 // Acknowledgement
    ];
    for pair in text.chunks(2) {
        words.push(pair[0] as u16 | (pair.get(1).copied().unwrap_or(0) as u16) << 8);
    }
    words.push(to);
    words
}

/// **A packet loops back through the board.** With Loop Back set, "the
/// cable and transceiver are not used and the interface is looped back to
/// itself." A packet written into the outgoing buffer and started comes
/// out of the incoming buffer: Transmit Done and Receive Done both up, no
/// CRC error, the bit count the packet's, and every word read back as
/// written, followed by the destination, the source the hardware put in,
/// and the check word.
/// Resets the interface, sets loop back, clears the receiver, writes
/// `words` into the transmitter and starts it. Returns when it started.
fn start_loopback(b: &mut UnibusMaster, words: &[u16]) -> u64 {
    b.cycle(chaos::CSR, Some(csr::RESET));
    b.run(b.now + 2_000);
    b.cycle(chaos::CSR, Some(csr::LOOP_BACK | csr::CLEAR_RECEIVER));
    let (_, csr0) = b.cycle(chaos::CSR, None);
    eprintln!("CSR after reset, loop back and clear receiver: {csr0:#08o}");
    assert!(csr0 & csr::LOOP_BACK != 0, "loop back reads back");
    assert!(csr0 & csr::RECEIVE_DONE == 0, "nothing received yet");
    for &w in words {
        b.cycle(chaos::WRITE_BUFFER, Some(w));
    }
    let (_, csr1) = b.cycle(chaos::CSR, None);
    assert!(csr1 & csr::TRANSMIT_DONE == 0, "writing a word clears Transmit Done: {csr1:#08o}");
    let started = b.now;
    let (_, me) = b.cycle(chaos::START, None);
    assert_eq!(me, MY_ADDRESS, "START reads the interface's own address");
    started
}

#[test]
fn a_packet_loops_back_through_the_board() {
    let n = cadrio();
    let mut b = board(&n);
    let words = rfc_time(MY_ADDRESS, MY_ADDRESS);
    let started = start_loopback(&mut b, &words);

    // Wait for the transmitter, then the receiver.
    let mut done = None;
    let mut received = None;
    while b.now < started + 3_000_000 {
        b.run(b.now + 5_000);
        let (_, c) = b.cycle(chaos::CSR, None);
        if done.is_none() && c & csr::TRANSMIT_DONE != 0 {
            done = Some((b.now - started, c));
        }
        if c & csr::RECEIVE_DONE != 0 {
            received = Some((b.now - started, c));
            break;
        }
    }
    let (t_done, c_done) = done.expect("Transmit Done never came up");
    eprintln!("Transmit Done after {t_done} ns, CSR {c_done:#08o}");
    assert!(c_done & csr::TRANSMIT_ABORT == 0, "the transmission was aborted");
    let (t_rcv, c_rcv) = received.expect("Receive Done never came up");
    eprintln!("Receive Done after {t_rcv} ns, CSR {c_rcv:#08o}");
    assert!(c_rcv & csr::CRC_ERROR == 0, "CRC error on the fresh packet: {c_rcv:#08o}");

    let (_, bits) = b.cycle(chaos::BIT_COUNT, None);
    let expect_words = words.len() + 2;
    eprintln!(
        "bit count {bits} ({} words + source + check = {} bits)",
        words.len(),
        expect_words * 16
    );
    assert_eq!(bits as usize, expect_words * 16 - 1, "the bit count minus one");

    let mut back = Vec::new();
    for _ in 0..expect_words {
        let (_, w) = b.cycle(chaos::READ_BUFFER, None);
        back.push(w);
    }
    eprintln!("read back: {:?}", back.iter().map(|w| format!("{w:#o}")).collect::<Vec<_>>());
    let (_, after) = b.cycle(chaos::BIT_COUNT, None);
    let (_, c_after) = b.cycle(chaos::CSR, None);
    eprintln!("after reading out: bit count {after:#o}, CSR {c_after:#08o}");
    assert_eq!(&back[..words.len()], &words[..], "the packet as written, destination last");
    assert_eq!(back[words.len()], MY_ADDRESS, "the source address the hardware put in");
    eprintln!("check word {:#o}", back[words.len() + 1]);
    assert_eq!(
        back[words.len() + 1],
        muir::chaos::packet::check_word(&back[..words.len() + 1]),
        "the check word the board made is the one the packet layer computes"
    );
    assert_eq!(after, 0o7777, "the bit count once read out");
    assert!(c_after & csr::CRC_ERROR == 0, "CRC error after reading out: {c_after:#08o}");
}

/// **The board's two 9401s are not wired alike, and the receiver's
/// select pin settles which pin is which.** `data/CADRIO.netlist`: the
/// transmit generator at LMTBUF `0C09` has pins 3, 5 and 8 all on `GND`,
/// so its code is 0 --- CRC-16 --- whichever way round the three are
/// named. The receive generator at LMRBUF `0C07` grounds only 5 and 8 and
/// takes **`RACT` on pin 3**, with `CWE` (pin 10) tied to `HI1` so it
/// divides whenever it is clocked, and `ER` (pin 13) on `CRCERR`, which
/// is the CSR's CRC Error bit through the 74LS244 at LMDATP `0D17`. So
/// the pin in dispute is the one pin of the three carrying a signal.
///
/// `mit/cadrio/iob.wlr` names those pins the other way round from
/// `src/part.rs`: it has pin 3 `S2`, pin 5 `S1` and pin 8 `S0`, on both
/// parts. **The wiring is not in dispute** --- the wire list and the
/// netlist give the same nets on the same pin numbers, and the wire list
/// names pin 11 `D` on both, which is what settles pin 11 --- only the
/// three select names, which the data sheet's own connection diagram and
/// logic symbol disagree about as well.
///
/// What is measured here is the board deciding it, on a frame it
/// transmitted itself. `RACT` is up for every one of the clock edges
/// that enter the frame and none fall outside it, so the code is not 0
/// during the division under either naming; and the register reaches all
/// zero on the last of those edges, `CRCERR` falling with it. That only
/// happens with pin 3 as the **low** bit of the code: `RACT` up is then
/// code 1, the sheet's "CRC-16 reverse", which is the reciprocal of the
/// transmitter's CRC-16 --- and the reciprocal is what this stream needs,
/// because AIM-628 §2.5 puts a frame on the cable in reverse bit order
/// (`chaos::packet::frame` reverses the bits, and a reversed message is
/// divisible by the reciprocal of the polynomial that divides the
/// original). Under the wire list's names the code would be 4, a degree-8
/// polynomial unrelated to CRC-16: with the model's mapping flipped to
/// `(pin 3 << 2) | (pin 5 << 1) | pin 8` this test fails at `CRCERR`, and
/// `a_packet_loops_back_through_the_board` reports CRC Error on a frame
/// the board had just sent.
///
/// **Unverified: that a real 9401 carries the pin names `src/part.rs`
/// gives it.** What is held here is narrower and does not need them --- a
/// board that cannot check its own frame is not the board MIT ran, so the
/// wire list's select names and this board's behavior cannot both be
/// right. What would settle the names: a Fairchild 9401 sheet whose
/// connection diagram and logic symbol agree, or the part in hand.
#[test]
fn the_receive_generator_divides_by_the_reciprocal_polynomial() {
    fn net_of(pins: &[(u8, String)], pin: u8) -> &str {
        pins.iter().find(|(p, _)| *p == pin).map_or("(unwired)", |(_, net)| net.as_str())
    }
    let n = cadrio();
    let generator = |page: &str, reference: &str| -> Vec<(u8, String)> {
        let mut v: Vec<(u8, String)> = n
            .parts
            .iter()
            .filter(|p| p.page == page && p.reference == reference && p.kind == "9401")
            .flat_map(|p| p.pins.iter().map(|&(pin, net)| (pin, n.net(net).to_string())))
            .collect();
        v.sort();
        v
    };

    // The two generators as the netlist wires them.
    let tx = generator("LMTBUF", "0C09");
    let rx = generator("LMRBUF", "0C07");
    eprintln!("LMTBUF 0C09 (transmit): {tx:?}");
    eprintln!("LMRBUF 0C07 (receive):  {rx:?}");
    for pin in [3, 5, 8] {
        assert_eq!(net_of(&tx, pin), "GND", "the transmit generator's select pin {pin}");
    }
    assert_eq!(net_of(&rx, 3), "RACT", "the receive generator's pin 3 is not grounded");
    assert_eq!(net_of(&rx, 5), "GND", "the receive generator's pin 5");
    assert_eq!(net_of(&rx, 8), "GND", "the receive generator's pin 8");
    assert_eq!(net_of(&rx, 10), "HI1", "CWE tied high: it divides whenever it is clocked");
    assert_eq!(net_of(&rx, 13), "CRCERR", "ER is where the CSR's CRC Error bit comes from");
    assert!(
        net_of(&rx, 12).starts_with("NC"),
        "and Q is unused: the receiver never shifts a check word out, it only divides"
    );

    // The receive generator's own clock pin, rather than a net named
    // nearby: pin 1 is the open-collector 74S00 at LMRBUF 0D07 gating
    // ROCLK^ with RICLK^. Being open collector it swings between Low and
    // Z, Z pulled up as the high state, so the high-to-low transition the
    // 9401 clocks on is the one into Low --- which is how `fell` reads it.
    let clock = n
        .parts
        .iter()
        .find(|p| p.page == "LMRBUF" && p.reference == "0C07" && p.kind == "9401")
        .and_then(|p| p.pins.iter().find(|&&(pin, _)| pin == 1).map(|&(_, net)| net))
        .expect("the receive generator's clock pin");

    let mut b = board(&n);
    let words = rfc_time(MY_ADDRESS, MY_ADDRESS);
    let started = start_loopback(&mut b, &words);
    let mut cp = b.chip.net(clock);
    let (mut clocked_active, mut clocked_idle) = (0usize, 0usize);
    let mut last_edge = 0u64;
    let mut crcerr_at_last_edge = Level::X;
    let mut crcerr_rose = false;
    while b.now < started + 400_000 {
        b.run(b.now + 25);
        let now = b.chip.net(clock);
        if cp != Level::Low && now == Level::Low {
            if b.level("RACT") == Level::High {
                clocked_active += 1;
            } else {
                clocked_idle += 1;
            }
            last_edge = b.now;
            crcerr_at_last_edge = b.level("CRCERR");
        }
        cp = now;
        if b.level("CRCERR") == Level::High {
            crcerr_rose = true;
        }
    }
    let bits = (words.len() + 2) * 16;
    eprintln!(
        "{clocked_active} clock edges with RACT up, {clocked_idle} with it down; \
         the last at {} ns, CRCERR {crcerr_at_last_edge:?}",
        last_edge - started
    );
    assert_eq!(clocked_active, bits, "every bit of the frame is clocked in with RACT up");
    assert_eq!(clocked_idle, 0, "and none with it down, so the code is never 0 while it divides");
    assert!(crcerr_rose, "the register held a remainder during the frame: CRCERR never went up");
    assert_eq!(
        crcerr_at_last_edge,
        Level::Low,
        "the receive generator reached all zero on the frame's last bit"
    );
    let (_, c) = b.cycle(chaos::CSR, None);
    eprintln!("CSR {c:#08o}");
    assert!(c & csr::CRC_ERROR == 0, "so the software reads no CRC error: {c:#08o}");
}

/// A read of 764142 done by hand, printing the decode chain at each step.
/// Run alone with `--ignored --nocapture`; it is a diagnostic, not a check.
#[test]
#[ignore = "a diagnostic, not a check: --ignored --nocapture"]
fn diagnose_the_chaos_reply() {
    let n = cadrio();
    let set = std::env::var("SET_SWITCHES").is_ok();
    let mut b = UnibusMaster::new(&n, 10_000, &quiet());
    if set {
        for (reference, closed) in chaos::switches(MY_ADDRESS) {
            b.chip.set_switches(reference, closed);
        }
        b.run(b.now + 2_000);
    }
    eprintln!("switches set: {set}");
    let watch = [
        "-A1*",
        "-A5*",
        "-A6*",
        "-A11*",
        "-MSYN*",
        "-SSYN*",
        "'CHAOS SSYN'",
        "-BOARD.SELECT",
        "'BOARD.SELECT FOR REAL'",
        "-SELECT.764140",
        "UBADDR1",
        "UBADDR2",
        "UBADDR3",
        "UBADDR4",
        "UBADDR5",
        "UBADDR6",
        "-11RCSR",
        "-11WCSR",
        "-11RRBTCT",
        "-TSR.SSYN",
        "-READ.DONE",
        "'-LOAD INTERVAL'",
        "HI2",
        "-C1*",
    ];
    let show = |b: &UnibusMaster, tag: &str| {
        let s: Vec<String> = watch
            .iter()
            .filter(|w| n.by_name_id(w).is_some())
            .map(|w| format!("{}={:?}", w.trim_matches('\''), b.level(w)))
            .collect();
        eprintln!("[{tag} @ {} ns] {}", b.now, s.join(" "));
    };
    show(&b, "rest");
    let uaddr: u32 = chaos::MY_ADDRESS;
    // Address bits 1..17 on -A1*..-A17*, active low; read: -C1* released.
    for k in 1..18u32 {
        let net = b.net(&format!("-A{k}*"));
        if (uaddr >> k) & 1 != 0 { b.chip.drive(net, Level::Low) } else { b.chip.pull_up(net) }
    }
    let c1 = b.net("-C1*");
    b.chip.pull_up(c1);
    b.chip.transition(b.now);
    b.run(b.now + 100);
    show(&b, "address up");
    let msyn = b.net("-MSYN*");
    b.chip.drive(msyn, Level::Low);
    b.chip.transition(b.now);
    show(&b, "MSYN");
    for _ in 0..3 {
        b.run(b.now + 250);
        show(&b, "wait");
    }
    // the bus receivers for the address
    for p in n.parts.iter().filter(|p| p.page == "IOBXCV" && p.reference == "0F07") {
        let s: Vec<String> = p
            .pins
            .iter()
            .map(|&(pin, net)| format!("p{pin}={}={:?}", n.net(net), b.chip.net(net)))
            .collect();
        eprintln!("IOBXCV 0F07 {}: {}", p.kind, s.join(" "));
    }
    // the decoder's outputs, whatever they are called
    for pg_ref in ["0C18", "0D15"] {
        for p in n.parts.iter().filter(|p| p.page == "LMUCON" && p.reference == pg_ref) {
            let s: Vec<String> = p
                .pins
                .iter()
                .map(|&(pin, net)| format!("p{pin}={}={:?}", n.net(net), b.chip.net(net)))
                .collect();
            eprintln!("LMUCON {pg_ref} {}: {}", p.kind, s.join(" "));
        }
    }
}

/// The transmitter after START, printing every watched net that moved.
/// A diagnostic: `--ignored --nocapture`.
#[test]
#[ignore = "a diagnostic, not a check: --ignored --nocapture"]
fn diagnose_the_transmitter() {
    let n = cadrio();
    let mut b = board(&n);
    let words = rfc_time(MY_ADDRESS, MY_ADDRESS);
    let watch = [
        "LOOP.BACK",
        "-11XMT",
        "TSREMPTY",
        "-TSRLOAD",
        "-TSR.SSYN",
        "-TDONE",
        "-TRESET",
        "-RESET",
        "SR_UNIBUS",
        "CW",
        "TIWEND^",
        "TDATA",
        "-TTL.D.OUT",
        "TTL.D.IN",
        "-RCVR.DATA.IN",
        "RDATA",
        "MY.TURN^",
        "-LOAD.MY.TURN",
        "-CBLBSY",
        "'-CBLBSY A'",
        "'MATCH SO FAR'",
        "LOCKOUT",
        "-EDGE",
        "-INTERFERENCE",
        "GENCLK",
        "SAMPLE",
    ];
    let clocks = ["MCLK^", "'FCLK^'", "FCLK/2^", "TICLK^", "TOCLK^", "'MY.TURN CLK^'", "'PRICLK^'"];
    let mut last: Vec<Level> = watch.iter().map(|w| b.level(w)).collect();
    let tbct = |b: &UnibusMaster| -> u16 {
        (0..12).map(|k| ((b.level(&format!("TBCT{k}")) == Level::High) as u16) << k).sum()
    };
    let diff = |b: &UnibusMaster| -> u16 {
        (0..12)
            .map(|k| ((b.level(&format!("'HOST ADR DIFF.{k}'")) == Level::High) as u16) << k)
            .sum()
    };
    let started = start_loopback(&mut b, &words);
    eprintln!(
        "started at {started}; clocks now: {}",
        clocks.iter().map(|c| format!("{c}={:?}", b.level(c))).collect::<Vec<_>>().join(" ")
    );
    let mut edges = vec![0u32; clocks.len()];
    let mut prev_clk: Vec<Level> = clocks.iter().map(|c| b.level(c)).collect();
    let (mut last_tbct, mut last_diff) = (tbct(&b), diff(&b));
    eprintln!("[{}] TBCT={last_tbct} DIFF={last_diff}", b.now - started);
    let end = started + 120_000;
    while b.now < end {
        let next = b.chip.next_tap().map_or(b.now + 50, |t| t.max(b.now + 1)).min(b.now + 500);
        b.run(next);
        let t = b.now - started;
        let mut changes = Vec::new();
        for (k, w) in watch.iter().enumerate() {
            let l = b.level(w);
            if l != last[k] {
                changes.push(format!("{}={:?}", w.trim_matches('\''), l));
                last[k] = l;
            }
        }
        for (k, c) in clocks.iter().enumerate() {
            let l = b.level(c);
            if l == Level::High && prev_clk[k] != Level::High {
                edges[k] += 1;
            }
            prev_clk[k] = l;
        }
        let (tb, df) = (tbct(&b), diff(&b));
        if tb != last_tbct || df != last_diff {
            changes.push(format!("TBCT={tb} DIFF={df}"));
            last_tbct = tb;
            last_diff = df;
        }
        if !changes.is_empty() {
            eprintln!("[{t:>7}] {}", changes.join(" "));
        }
    }
    eprintln!(
        "rising edges seen in {} ns: {}",
        end - started,
        clocks
            .iter()
            .zip(&edges)
            .map(|(c, e)| format!("{}={e}", c.trim_matches('\'')))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let (_, c) = b.cycle(chaos::CSR, None);
    eprintln!("CSR at the end: {c:#08o}");
}

/// The receiver during a looped-back transmission: slow nets printed as
/// they move, fast ones counted. A diagnostic: `--ignored --nocapture`.
#[test]
#[ignore = "a diagnostic, not a check: --ignored --nocapture"]
fn diagnose_the_receiver() {
    let n = cadrio();
    let mut b = board(&n);
    let words = rfc_time(MY_ADDRESS, MY_ADDRESS);
    let slow = [
        "'D OUT'",
        "-TTL.D.OUT",
        "CBLBSY",
        "'-CBLBSY A'",
        "PRACT",
        "RACT",
        "START^",
        "RRESET",
        "-RDONE",
        "'RCV BUSY'",
        "-GOT.ONE",
        "ITS.ME",
        "'DEST MATCH'",
        "'MATCH SO FAR'",
        "'SRC STB'",
        "PRIWBEG",
        "CRCERR",
        "ABORTSIG",
        "ABORTDN",
        "'11 RD BF'",
        "-READ.DONE",
        "-ROWEND",
        "LOCKOUT",
        "RDATA",
        "-TDONE",
        "MATCH.ANY.DEST",
        "-CRC.PRE",
        "-RACT",
    ];
    let fast = [
        "TTL.D.IN",
        "-EDGE",
        "GENCLK",
        "'GENCLK END'",
        "SDLYD",
        "SDLYD2",
        "SAMPLE",
        "'LOCKOUT END'",
        "RICLK^",
        "'PRICLK^'",
        "ROCLK^",
        "RO.FIRST^",
        "TOCLK^",
    ];
    let counter = |b: &UnibusMaster, name: &str, bits: u32| -> u16 {
        (0..bits).map(|k| ((b.level(&format!("{name}{k}")) == Level::High) as u16) << k).sum()
    };
    // The detector's parts: every pin's net, its name, level and drivers.
    for (page, reference) in [
        ("LMDETC", "0B02"),
        ("LMDETC", "0C02"),
        ("LMDETC", "0E08"),
        ("LMDETC", "0C01"),
        ("LMDETC", "0B01"),
        ("LMDETC", "0E03"),
    ] {
        for p in n.parts.iter().filter(|p| p.page == page && p.reference == reference) {
            let s: Vec<String> = p
                .pins
                .iter()
                .map(|&(pin, net)| {
                    format!(
                        "p{pin}=[{net}]{}={:?}/{}drv",
                        n.net(net),
                        b.chip.net(net),
                        b.chip.drivers_on(net).len()
                    )
                })
                .collect();
            eprintln!("{page} {reference} {}: {}", p.kind, s.join(" "));
        }
    }
    let started = start_loopback(&mut b, &words);
    let mut last: Vec<Level> = slow.iter().map(|w| b.level(w)).collect();
    eprintln!(
        "after START: {}",
        slow.iter()
            .zip(&last)
            .map(|(w, l)| format!("{}={:?}", w.trim_matches('\''), l))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let mut prev_fast: Vec<Level> = fast.iter().map(|c| b.level(c)).collect();
    let mut rises = vec![0u32; fast.len()];
    let (mut last_rbct, mut last_prbct) = (counter(&b, "RBCT", 12), counter(&b, "PRBCT", 6));
    eprintln!("RBCT={last_rbct} PRBCT={last_prbct}");
    let mut line_edges_shown = 0;
    let end = started + 400_000;
    let mut done_at = None;
    while b.now < end {
        let next = b.chip.next_tap().map_or(b.now + 50, |t| t.max(b.now + 1)).min(b.now + 500);
        b.run(next);
        let t = b.now - started;
        let mut changes = Vec::new();
        for (k, w) in slow.iter().enumerate() {
            let l = b.level(w);
            if l != last[k] {
                changes.push(format!("{}={:?}", w.trim_matches('\''), l));
                last[k] = l;
            }
        }
        for (k, c) in fast.iter().enumerate() {
            let l = b.level(c);
            if l != prev_fast[k] {
                if l == Level::High {
                    rises[k] += 1;
                }
                if k < 8 && line_edges_shown < 60 {
                    changes.push(format!("{}={:?}", c.trim_matches('\''), l));
                    line_edges_shown += 1;
                }
            }
            prev_fast[k] = l;
        }
        let (rb, pb) = (counter(&b, "RBCT", 12), counter(&b, "PRBCT", 6));
        if rb != last_rbct || pb != last_prbct {
            changes.push(format!("RBCT={rb} PRBCT={pb}"));
            last_rbct = rb;
            last_prbct = pb;
        }
        if !changes.is_empty() {
            eprintln!("[{t:>7}] {}", changes.join(" "));
        }
        if done_at.is_none() && b.level("-TDONE") == Level::Low {
            done_at = Some(t);
        }
        if let Some(d) = done_at
            && t > d + 20_000
        {
            break;
        }
    }
    eprintln!(
        "rising edges: {}",
        fast.iter()
            .zip(&rises)
            .map(|(c, e)| format!("{}={e}", c.trim_matches('\'')))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let (_, c) = b.cycle(chaos::CSR, None);
    let (_, bits) = b.cycle(chaos::BIT_COUNT, None);
    eprintln!("CSR at the end: {c:#08o}, bit count {bits:#o}");
}

use muir::chaos::ether::{Capture, Ether, Event};

/// The board reset and its receiver cleared, with the cable in use: no
/// loop back.
fn on_the_cable(b: &mut UnibusMaster) {
    b.cycle(chaos::CSR, Some(csr::RESET));
    b.run(b.now + 2_000);
    b.cycle(chaos::CSR, Some(csr::CLEAR_RECEIVER));
}

/// Runs until `bit` is up in the CSR, or `for_ns` have passed; the CSR.
fn wait_for(b: &mut UnibusMaster, bit: u16, for_ns: u64) -> u16 {
    let until = b.now + for_ns;
    loop {
        b.run(b.now + 5_000);
        let (_, c) = b.cycle(chaos::CSR, None);
        if c & bit != 0 || b.now >= until {
            return c;
        }
    }
}

/// **The model hears what the board sends.** With a capture node at 3060
/// on the ether and no loop back, the board transmits a packet for 3060.
/// The ether decodes it off the board's own line driver --- the biphase
/// cells, the reversed word order, the check word --- and it is the
/// packet as written, from 3050, checking good. And the board, which
/// hears its own packet as every transceiver does, does not receive it:
/// it is not addressed to 3050.
#[test]
fn the_model_hears_what_the_board_sends() {
    let n = cadrio();
    let mut b = board(&n);
    on_the_cable(&mut b);
    let mut ether = Ether::new();
    ether.keep_log(true);
    ether.attach(Box::new(Capture::new(0o3060)));
    b.plug_chaos(ether);
    let words = rfc_time(MY_ADDRESS, 0o3060);
    for &w in &words {
        b.cycle(chaos::WRITE_BUFFER, Some(w));
    }
    let started = b.now;
    b.cycle(chaos::START, None);
    let c = wait_for(&mut b, csr::TRANSMIT_DONE, 3_000_000);
    eprintln!("Transmit Done after {} ns, CSR {c:#08o}", b.now - started);
    assert!(c & csr::TRANSMIT_DONE != 0, "the board transmitted");
    assert!(c & csr::TRANSMIT_ABORT == 0, "and was not aborted");
    b.run(b.now + 5_000);
    let log = &b.chaos.as_ref().unwrap().ether.log;
    eprintln!("ether log: {log:?}");
    let heard: Vec<_> = log
        .iter()
        .filter_map(|e| if let Event::Heard(t, f) = e { Some((t, f)) } else { None })
        .collect();
    assert_eq!(heard.len(), 1, "one packet went by");
    let (_, f) = heard[0];
    assert_eq!(f.buffer, words, "the packet as the board's software wrote it");
    assert_eq!(f.source, MY_ADDRESS, "from the board");
    assert!(f.check_ok, "and its check word is good: {:#o}", f.check);
    assert!(!log.iter().any(|e| matches!(e, Event::Collision(_))), "no collision");
    let (_, c) = b.cycle(chaos::CSR, None);
    assert!(c & csr::RECEIVE_DONE == 0, "not for 3050, so not received: {c:#08o}");
}

/// **The board receives what the model sends.** A packet for 3050 from a
/// node at 3060, framed and coded by the ether, arrives in the board's
/// receive buffer: Receive Done, no CRC error, the bit count, and the
/// words read back as sent, then the destination, the source 3060 and a
/// check word the board itself accepted.
#[test]
fn the_board_receives_what_the_model_sends() {
    let n = cadrio();
    let mut b = board(&n);
    on_the_cable(&mut b);
    let words = rfc_time(0o3060, MY_ADDRESS);
    let mut node = Capture::new(0o3060);
    node.to_send.push_back(words.clone());
    let mut ether = Ether::new();
    ether.attach(Box::new(node));
    let started = b.now;
    b.plug_chaos(ether);
    let c = wait_for(&mut b, csr::RECEIVE_DONE, 1_000_000);
    eprintln!("Receive Done after {} ns, CSR {c:#08o}", b.now - started);
    assert!(c & csr::RECEIVE_DONE != 0, "the board received");
    assert!(c & csr::CRC_ERROR == 0, "with its check good: {c:#08o}");
    let (_, bits) = b.cycle(chaos::BIT_COUNT, None);
    assert_eq!(bits as usize, (words.len() + 2) * 16 - 1, "the bit count");
    let mut back = Vec::new();
    for _ in 0..words.len() + 2 {
        let (_, w) = b.cycle(chaos::READ_BUFFER, None);
        back.push(w);
    }
    eprintln!("read back: {:?}", back.iter().map(|w| format!("{w:#o}")).collect::<Vec<_>>());
    assert_eq!(&back[..words.len()], &words[..], "the packet as sent");
    assert_eq!(back[words.len()], 0o3060, "from the node");
    assert_eq!(back[words.len() + 1], muir::chaos::packet::check_word(&back[..words.len() + 1]));
    let (_, c) = b.cycle(chaos::CSR, None);
    assert!(c & csr::CRC_ERROR == 0, "and still good once read out: {c:#08o}");
}

/// **The board takes broadcasts and leaves other hosts' packets alone.**
/// AIM-628 §2.5: the destination "is compared by each receiver against
/// its own address. If they match, or if the destination is zero ... the
/// packet is placed in the receive packet buffer".
#[test]
fn the_board_takes_broadcasts_and_not_others_packets() {
    let n = cadrio();
    let mut b = board(&n);
    on_the_cable(&mut b);
    let mut node = Capture::new(0o3060);
    node.to_send.push_back(rfc_time(0o3060, 0o3070));
    let mut ether = Ether::new();
    ether.keep_log(true);
    ether.attach(Box::new(node));
    b.plug_chaos(ether);
    let c = wait_for(&mut b, csr::RECEIVE_DONE, 400_000);
    assert!(c & csr::RECEIVE_DONE == 0, "a packet for 3070 is not received: {c:#08o}");
    assert_eq!(
        b.chaos.as_ref().unwrap().ether.log.iter().filter(|e| matches!(e, Event::Sent(..))).count(),
        1,
        "it did go by"
    );
    // Now a broadcast.
    let words = rfc_time(0o3060, 0);
    let mut node = Capture::new(0o3060);
    node.to_send.push_back(words.clone());
    let mut ether = Ether::new();
    ether.attach(Box::new(node));
    b.plug_chaos(ether);
    let c = wait_for(&mut b, csr::RECEIVE_DONE, 1_000_000);
    assert!(c & csr::RECEIVE_DONE != 0, "a broadcast is received: {c:#08o}");
    assert!(c & csr::CRC_ERROR == 0);
    let mut back = Vec::new();
    for _ in 0..words.len() + 2 {
        let (_, w) = b.cycle(chaos::READ_BUFFER, None);
        back.push(w);
    }
    assert_eq!(&back[..words.len()], &words[..]);
}

use muir::chaos::packet::{Packet, op};
use support::server::Server;
use support::time::Time;

/// **TIME, over the board.** What the Lisp Machine's `SERVER-TIME` does
/// through these registers, done through them: an RFC to `TIME` at 3060
/// written into the transmitter and started; the host on the ether
/// answers; the board receives the ANS, and its four data bytes are the
/// universal time, least significant first. The whole stack between the
/// software and the service, with MIT's board in the middle.
#[test]
fn the_board_asks_the_time_and_is_answered() {
    let n = cadrio();
    let mut b = board(&n);
    on_the_cable(&mut b);
    let mut server = Server::new(0o3060);
    server.serve(Box::new(Time::fixed(0x1234_5678)));
    let mut ether = Ether::new();
    ether.keep_log(true);
    ether.attach(Box::new(server));
    b.plug_chaos(ether);
    let ask = Packet {
        opcode: op::RFC,
        forward: 0,
        dest: 0o3060,
        dest_index: 0,
        source: MY_ADDRESS,
        source_index: 0o21,
        number: 0o1234,
        ack: 0,
        data: b"TIME".to_vec(),
    };
    let words = ask.to_buffer(0o3060);
    for &w in &words {
        b.cycle(chaos::WRITE_BUFFER, Some(w));
    }
    let started = b.now;
    b.cycle(chaos::START, None);
    let c = wait_for(&mut b, csr::RECEIVE_DONE, 3_000_000);
    eprintln!("Receive Done after {} ns, CSR {c:#08o}", b.now - started);
    assert!(c & csr::TRANSMIT_DONE != 0, "the RFC went out");
    assert!(c & csr::RECEIVE_DONE != 0, "and an answer came back: {c:#08o}");
    assert!(c & csr::CRC_ERROR == 0);
    let (_, bits) = b.cycle(chaos::BIT_COUNT, None);
    let count = (bits as usize + 1) / 16;
    let mut back = Vec::new();
    for _ in 0..count {
        let (_, w) = b.cycle(chaos::READ_BUFFER, None);
        back.push(w);
    }
    let (ans, _) = Packet::from_buffer(&back[..count - 2]).unwrap();
    eprintln!("answer: {ans:?}");
    assert_eq!(ans.opcode, op::ANS);
    assert_eq!((ans.source, ans.dest, ans.dest_index), (0o3060, MY_ADDRESS, 0o21));
    assert_eq!(ans.ack, 0o1234);
    assert_eq!(ans.data, [0x78, 0x56, 0x34, 0x12], "the universal time, low byte first");
    assert_eq!(back[count - 2], 0o3060, "from the host");
    let log = &b.chaos.as_ref().unwrap().ether.log;
    assert!(!log.iter().any(|e| matches!(e, Event::Collision(_))), "no collision: {log:?}");
}

/// **The board aborts its transmission on interference.** With its frame
/// on the cable, a transmitter that has not heard the cable starts
/// another: the model's, at its own instant. The transceiver reports
/// interference the moment both drive high, `COLLISION` is `TBUSY` with
/// it (the 74S02 at LMMODU 0B08), the `ABORT` flip-flop at 0A09 takes it
/// on `-FCLK^`, and from there the driver is off, `TABORTED` is up and
/// Transmit Done comes: the CSR reads Transmit Done and Transmit Abort.
/// The model transmitter stops too, [`ABORT_NS`] after the interference,
/// and nothing whole was on the cable. The board's nets are watched
/// through it, for the instants the behavioral interface is held to.
#[test]
fn the_board_aborts_its_transmission_on_interference() {
    use muir::chaos::ether::{ABORT_NS, Capture, Ether, Event};
    let n = cadrio();
    let mut b = board(&n);
    let mut ether = Ether::new();
    ether.keep_log(true);
    ether.attach(Box::new(Capture::new(0o3060)));
    b.plug_chaos(ether);
    on_the_cable(&mut b);
    let words = rfc_time(MY_ADDRESS, 0o3060);
    for &w in &words {
        b.cycle(chaos::WRITE_BUFFER, Some(w));
    }
    let started = b.now;
    b.cycle(chaos::START, None);
    // Until the board's frame is on the cable.
    while !b.chaos.as_ref().unwrap().ether.busy(b.now) {
        b.run(b.now + 250);
        assert!(b.now < started + 3_000_000, "the board transmitted");
    }
    let on = b.now;
    eprintln!("the board's frame on the cable {} ns after START", on - started);
    b.run(on + 5_000);
    let at = b.now;
    // For a third host, so that the board's receiver has no say in this.
    b.chaos.as_mut().unwrap().ether.send_now(at, 0o3060, rfc_time(0o3060, 0o3070));
    // The board's nets from there, as they change.
    let names = ["'INTERFERENCE IN'", "COLLISION", "ABORT", "TBUSY", "TABORTED", "TDONE"];
    let nets: Vec<_> = names.iter().map(|name| b.net(name)).collect();
    let read = |b: &UnibusMaster| -> Vec<Level> { nets.iter().map(|&id| b.chip.net(id)).collect() };
    let mut last = read(&b);
    let mut driving = b.chaos.as_ref().unwrap().board_tx(&b.chip);
    let mut seen = Vec::new();
    while b.now < at + 20_000 {
        b.run(b.now + 25);
        let now = read(&b);
        for (k, (was, is)) in last.iter().zip(&now).enumerate() {
            if was != is {
                eprintln!("  +{:>6} ns: {} {:?}", b.now - at, names[k], is);
                seen.push((b.now - at, names[k], *is));
            }
        }
        last = now;
        let d = b.chaos.as_ref().unwrap().board_tx(&b.chip);
        if d != driving {
            eprintln!("  +{:>6} ns: the driver {}", b.now - at, if d { "on" } else { "off" });
            driving = d;
        }
    }
    for ev in &b.chaos.as_ref().unwrap().ether.log {
        match ev {
            Event::Sent(t, s, _) => eprintln!("  ether: {s:o} sent at +{}", *t as i64 - at as i64),
            Event::Collision(t) => eprintln!("  ether: collision at +{}", *t as i64 - at as i64),
            Event::Heard(t, f) => {
                eprintln!("  ether: heard from {:o} at +{}", f.source, *t as i64 - at as i64)
            }
        }
    }
    let up = |name: &str| {
        seen.iter().find(|&&(_, n, l)| n == name && l == Level::High).map(|&(t, _, _)| t)
    };
    let interference = up("'INTERFERENCE IN'").expect("the transceiver reported interference");
    let abort = up("ABORT").expect("ABORT set");
    let taborted = up("TABORTED").expect("TABORTED set");
    let tdone = up("TDONE").expect("TDONE set");
    eprintln!(
        "interference at +{interference}, ABORT +{abort}, TABORTED +{taborted}, TDONE +{tdone}: \
         TDONE {} ns after ABORT",
        tdone - abort
    );
    assert!(
        abort - interference <= 16 * ABORT_NS,
        "ABORT within a few cells of the first interference: an edge of -FCLK^ found it"
    );
    let c = wait_for(&mut b, csr::TRANSMIT_DONE, 3_000_000);
    assert!(c & csr::TRANSMIT_DONE != 0, "Transmit Done: {c:#08o}");
    assert!(c & csr::TRANSMIT_ABORT != 0, "and Transmit Abort: {c:#08o}");
    let log = &b.chaos.as_ref().unwrap().ether.log;
    assert!(
        log.iter().any(|e| matches!(e, Event::Collision(_))),
        "a collision on the cable: {log:?}"
    );
    assert!(
        !log.iter().any(|e| matches!(e, Event::Heard(_, f) if f.check_ok)),
        "nothing whole was heard: {log:?}"
    );
    // The other transmitter, with no detector, runs its frame out alone.
    b.run(b.now + 200_000);
    assert!(!b.chaos.as_ref().unwrap().ether.busy(b.now), "the cable is idle again");
}

/// **The board receives wreckage addressed to it, and says so.** A frame
/// for the board is on the cable when a transmitter that has not heard
/// it starts another, sixty-four cells in: the check, source and
/// destination words are past and the rest is two frames over each
/// other. The receiver matched the destination and takes what arrives:
/// Receive Done with CRC Error, and the bit count as received, which the
/// software reads back as it would a packet's. What the model makes of
/// the same bits is held to this in `tests/chaos_rtl.rs`.
#[test]
fn the_board_receives_wreckage_addressed_to_it() {
    use muir::chaos::ether::{Capture, Ether};
    use muir::chaos::wire;
    let n = cadrio();
    let mut b = board(&n);
    on_the_cable(&mut b);
    let mut ether = Ether::new();
    ether.keep_log(true);
    ether.attach(Box::new(Capture::new(0o3060)));
    b.plug_chaos(ether);
    let at = b.now;
    b.chaos.as_mut().unwrap().ether.send_now(at, 0o3060, rfc_time(0o3060, MY_ADDRESS));
    b.run(at + 64 * wire::CELL_NS);
    b.chaos.as_mut().unwrap().ether.send_now(b.now, 0o3061, rfc_time(0o3061, 0o3070));
    let c = wait_for(&mut b, csr::RECEIVE_DONE, 1_000_000);
    eprintln!("Receive Done after {} ns, CSR {c:#08o}", b.now - at);
    assert!(c & csr::RECEIVE_DONE != 0, "the board took the wreckage: {c:#08o}");
    assert!(c & csr::CRC_ERROR != 0, "with its check bad: {c:#08o}");
    let (_, bits) = b.cycle(chaos::BIT_COUNT, None);
    let words = (bits as usize + 1) / 16;
    let mut back = Vec::new();
    for _ in 0..words {
        let (_, w) = b.cycle(chaos::READ_BUFFER, None);
        back.push(w);
    }
    eprintln!(
        "bit count {bits} ({words} whole words{}): {:?}",
        if (bits as usize + 1).is_multiple_of(16) { "" } else { " and a part" },
        back.iter().map(|w| format!("{w:#o}")).collect::<Vec<_>>()
    );
    assert!(bits as usize + 1 > 3 * 16, "more than the three header words came");
    assert_ne!(bits, 0o7777);
}

/// **CRC Error reads set while a frame is coming in.** AIM-628 says the
/// bit is "only valid at two times"; between them the board's check
/// register is mid-packet and the bit reads 1, at every poll of a good
/// packet for the board until it lands, and 0 then. The behavioral
/// interface shows the same, which `tests/chaos_rtl.rs` holds it to.
#[test]
fn crc_error_while_a_frame_comes_in() {
    use muir::chaos::ether::{Capture, Ether};
    let n = cadrio();
    let mut b = board(&n);
    on_the_cable(&mut b);
    let mut node = Capture::new(0o3060);
    node.to_send.push_back(rfc_time(0o3060, MY_ADDRESS));
    let mut ether = Ether::new();
    ether.attach(Box::new(node));
    let started = b.now;
    b.plug_chaos(ether);
    let mut seen = Vec::new();
    loop {
        b.run(b.now + 5_000);
        let busy = b.chaos.as_ref().unwrap().ether.busy(b.now);
        let (_, c) = b.cycle(chaos::CSR, None);
        seen.push((b.now - started, busy, c & csr::CRC_ERROR != 0, c & csr::RECEIVE_DONE != 0));
        if c & csr::RECEIVE_DONE != 0 || b.now > started + 1_000_000 {
            break;
        }
    }
    for &(t, busy, crc, done) in &seen {
        eprintln!(
            "  +{t:>7}: cable {}, CRC Error {}, Receive Done {}",
            if busy { "busy" } else { "idle" },
            crc as u8,
            done as u8
        );
    }
    let last = seen.last().unwrap();
    assert!(last.3 && !last.2, "the packet landed with its check good");
    let during: Vec<bool> = seen.iter().filter(|s| s.1 && !s.3).map(|s| s.2).collect();
    eprintln!("CRC Error while the frame was on the cable: {during:?}");
    assert!(
        during.len() >= 5 && during.iter().all(|&c| c),
        "set at every poll with the frame coming in"
    );
}

/// The turn timer's nets from START to Transmit Done under Loop Back, two
/// packets running: `MY.TURN^`, the loads, the cable-busy line and the
/// transmitter's start and end.  Run alone with `--ignored --nocapture`;
/// it is a diagnostic, not a check.
#[test]
#[ignore = "a diagnostic, not a check: --ignored --nocapture"]
fn diagnose_the_turn_timer() {
    let n = cadrio();
    let mut b = board(&n);
    b.cycle(chaos::CSR, Some(csr::RESET));
    b.run(b.now + 2_000);
    b.cycle(chaos::CSR, Some(csr::LOOP_BACK | csr::CLEAR_RECEIVER));
    let words = rfc_time(MY_ADDRESS, MY_ADDRESS);
    let watch = [
        "MY.TURN^",
        "-LOAD.MY.TURN",
        "-CBLBSY",
        "'-CBLBSY A'",
        "-TDONE",
        "TSREMPTY",
        "-11XMT",
        "LOCKOUT",
        "-INTERFERENCE",
        "-TRESET",
        "-RESET",
        "SR_UNIBUS",
    ];
    for round in 0..2 {
        for &w in &words {
            b.cycle(chaos::WRITE_BUFFER, Some(w));
        }
        let started = b.now;
        b.cycle(chaos::START, None);
        let mut last: Vec<Level> = watch.iter().map(|w| b.level(w)).collect();
        let mut out_last = b.level("-TTL.D.OUT");
        let (mut out_edges, mut out_first, mut out_final) = (0u64, None, None);
        let mut turn_clk_last = b.level("'MY.TURN CLK^'");
        let mut turn_clk = 0u64;
        let diff = |b: &UnibusMaster| -> u16 {
            (0..8)
                .map(|k| ((b.level(&format!("'HOST ADR DIFF.{k}'")) == Level::High) as u16) << k)
                .sum()
        };
        eprintln!("round {round}: START at {started}; HOST ADR DIFF {}", diff(&b));
        while b.now < started + 320_000 {
            b.run(b.now + 50);
            let now: Vec<Level> = watch.iter().map(|w| b.level(w)).collect();
            for (k, w) in watch.iter().enumerate() {
                if now[k] != last[k] {
                    eprintln!(
                        "  [{:>7}] {w} -> {:?}  (turn clocks {turn_clk}, diff {})",
                        b.now - started,
                        now[k],
                        diff(&b)
                    );
                }
            }
            last = now;
            let o = b.level("-TTL.D.OUT");
            if o != out_last {
                out_edges += 1;
                out_first.get_or_insert(b.now - started);
                out_final = Some(b.now - started);
            }
            out_last = o;
            let t = b.level("'MY.TURN CLK^'");
            if t != turn_clk_last && t == Level::High {
                turn_clk += 1;
            }
            turn_clk_last = t;
            if b.level("-TDONE") == Level::Low {
                eprintln!(
                    "  -TDONE low at {}; -TTL.D.OUT edges {out_edges} from {out_first:?} to {out_final:?}",
                    b.now - started
                );
                break;
            }
        }
        eprintln!(
            "  -TTL.D.OUT edges {out_edges} from {out_first:?} to {out_final:?}; turn clocks {turn_clk}"
        );
        b.run(b.now + 5_000);
        let (_, c) = b.cycle(chaos::CSR, None);
        eprintln!("  CSR {c:#08o}");
        let (_, bits) = b.cycle(chaos::BIT_COUNT, None);
        for _ in 0..(bits as usize + 1) / 16 {
            b.cycle(chaos::READ_BUFFER, None);
        }
        b.cycle(chaos::CSR, Some(csr::LOOP_BACK | csr::CLEAR_RECEIVER));
    }
}

/// **The CSR's Timer Interrupt Enable reaches no gate on this board.**
/// AIM-628 has the bit "for the interval timer present in some versions
/// of the interface", and this is not one of them. In `cadrio/iob.wlr`
/// `TIMER.IEN` has two pins --- the 74LS174 at LMUCON B20 the CSR write
/// loads, pin 12, and the 74LS244 at LMDATP D16 that reads it back, pin
/// 17 --- and the net has the same two in the netlist. The 74S51 at
/// LMUCON E05 that makes the interrupt has both of its AND pairs taken,
/// `RDONE` with `RIEN` and `TDONE` with `TIEN`, and a '51 has no third
/// input: TI's sheet marks pins 11 and 12 "make no external connection".
/// MIT's own `chatst.lisp`, on the bit: "This bit doesnt seem to do
/// anything." Issue 96.
#[test]
fn the_timer_interrupt_enable_reaches_no_gate() {
    let n = cadrio();
    let net = n.by_name_id("TIMER.IEN").expect("TIMER.IEN");
    let mut on_net: Vec<(String, String, u8)> = n
        .parts
        .iter()
        .flat_map(|p| {
            p.pins
                .iter()
                .filter(move |&&(_, id)| id == net)
                .map(move |&(pin, _)| (p.page.clone(), p.reference.clone(), pin))
        })
        .collect();
    on_net.sort();
    let want =
        |page: &str, reference: &str, pin: u8| (page.to_string(), reference.to_string(), pin);
    assert_eq!(on_net, [want("LMDATP", "0D16", 17), want("LMUCON", "0B20", 12)]);

    let wires = support::wire_list(&n, &["cadrio", "iob.wlr"]);
    let s = wires.iter().find(|s| s.names.iter().any(|x| x == "TIMER.IEN")).expect("in iob.wlr");
    let mut pins: Vec<(String, u8)> =
        s.pins.iter().map(|p| (p.location.clone(), p.number)).collect();
    pins.sort();
    assert_eq!(pins, [("B20".to_string(), 12), ("D16".to_string(), 17)]);

    let aoi: Vec<&netlist::Part> =
        n.parts.iter().filter(|p| p.reference == "0E05" && p.kind == "74S51").collect();
    let mut pages: Vec<&str> = aoi.iter().map(|p| p.page.as_str()).collect();
    pages.sort();
    assert_eq!(pages, ["LMRBUF", "LMUCON"], "the two gates of the 74S51, one on each page");
    let mut used: Vec<u8> = aoi.iter().flat_map(|p| p.pins.iter().map(|&(pin, _)| pin)).collect();
    used.sort();
    assert_eq!(used, [1, 2, 3, 4, 5, 6, 8, 9, 10, 13], "no pin 11 or 12, and none spare");
    let id = |name: &str| n.by_name_id(name).unwrap_or_else(|| panic!("no net {name}"));
    let interrupt = aoi.iter().find(|p| p.pins.iter().any(|&(pin, _)| pin == 8)).unwrap();
    for (pin, name) in [(1, "RDONE"), (13, "RIEN"), (10, "TDONE"), (9, "TIEN")] {
        let &(_, on) = interrupt.pins.iter().find(|&&(k, _)| k == pin).unwrap();
        assert_eq!(on, id(name), "pin {pin} is {name}");
    }
}

// --- The busy receiver, AIM-628 §2.5's hardware flow control -------------

use muir::chaos::wire::CELL_NS;

/// The four `LSTCNT` bits, the 74LS161 at LMMYNM 0F04, read off the nets
/// rather than through the CSR: a Unibus cycle takes microseconds, and
/// these are watched while a frame is coming in.
fn lost_count(b: &UnibusMaster) -> u16 {
    (0..4).map(|k| ((b.level(&format!("LSTCNT{k}")) == Level::High) as u16) << k).sum()
}

/// A board at [`MY_ADDRESS`] with one packet in its buffer and the
/// receiver not cleared, and a node at 3060 with a second frame for
/// `dest` queued behind the first: the ether gives it its turn, one slot
/// after the cable goes idle. Returns with Receive Done up and the second
/// frame not yet started.
fn buffer_full(b: &mut UnibusMaster, dest: u16) {
    let mut node = Capture::new(0o3060);
    node.to_send.push_back(rfc_time(0o3060, MY_ADDRESS));
    node.to_send.push_back(rfc_time(0o3060, dest));
    let mut ether = Ether::new();
    ether.keep_log(true);
    ether.attach(Box::new(node));
    b.plug_chaos(ether);
    let until = b.now + 1_000_000;
    while b.level("RDONE") == Level::Low {
        let next = b.chip.next_tap().map_or(b.now + 50, |t| t.max(b.now + 1)).min(b.now + 50);
        b.run(next);
        assert!(b.now < until, "the first packet never landed");
    }
}

/// When the second frame started on the cable, and the board's line
/// driver's first run: `(sent, on, off)`, from the ether's log and the
/// board's own transceiver, watched for `for_ns` from here.
fn watch_the_driver(b: &mut UnibusMaster, for_ns: u64) -> (Option<u64>, Option<u64>, Option<u64>) {
    let until = b.now + for_ns;
    let (mut on, mut off) = (None, None);
    let mut tx = b.chaos.as_ref().unwrap().board_tx(&b.chip);
    while b.now < until {
        let next = b.chip.next_tap().map_or(b.now + 25, |t| t.max(b.now + 1)).min(b.now + 25);
        b.run(next);
        let d = b.chaos.as_ref().unwrap().board_tx(&b.chip);
        if d != tx {
            if d {
                on.get_or_insert(b.now);
            } else if on.is_some() {
                off.get_or_insert(b.now);
            }
            tx = d;
        }
    }
    let sent = b
        .chaos
        .as_ref()
        .unwrap()
        .ether
        .log
        .iter()
        .filter_map(|e| if let Event::Sent(t, _, _) = e { Some(*t) } else { None })
        .nth(1);
    (sent, on, off)
}

/// **The board aborts a frame addressed to it while its buffer is full.**
/// AIM-628 §2.5, memo page 5: "When a receiving interface determines that
/// an incoming packet is addressed to it, but its receive buffer already
/// contains a packet, it sends an abort signal which causes the
/// transmitter to stop."
///
/// The board as wired does it. `RACT` is the 74S74 at LMRCLK 0C06 taking
/// `-RDONE` on `START^`, so with Receive Done up the receiver does not go
/// active for the next frame; the destination word then matches all the
/// same, in the 74S287 at LMMYNM 0D01, and the 74S10 at LMMYNM 0D02 makes
/// `-LOST.ONE` from `MATCH SO FAR`, `ITS.ME` and `-RACT`. That wire runs
/// to pin 4 of the 74S112 at LMMODU 0A09, which `cadrio/iob.wlr` names
/// `-SET1`: the preset of the `ABORT` flip-flop. `ABORT` holds
/// `-TTL.D.OUT` low through the 74S02 at LMMODU 0B08, and the 26LS31 at
/// LMLNDR 0A02 puts that on the cable --- the ether held high with no
/// transitions, which is the abort signal.
///
/// Measured here: the driver goes on 12,260 ns after the frame's first
/// edge, the bit cell after the destination word, which is the third word
/// on the wire ("in the order check, source, destination", §2.5, memo
/// page 6); it holds four bit cells --- 1,033 ns here, and up to 1,109 as
/// the frame falls against the board's 8 MHz `FCLK^`, since `ABORTDN`
/// ends it --- which is the length §2.5 gives an abort signal. The frame is counted in Lost Count, the
/// packet in the buffer is untouched, and the transmitter stops with its
/// frame wreckage on the cable.
#[test]
fn the_busy_receiver_aborts_a_frame_addressed_to_it() {
    let n = cadrio();
    let mut b = board(&n);
    on_the_cable(&mut b);
    buffer_full(&mut b, MY_ADDRESS);
    assert_eq!(lost_count(&b), 0, "nothing lost yet");
    let abort = b.net("ABORT");
    let lost_one = b.net("-LOST.ONE");
    let (sent, on, off) = watch_the_driver(&mut b, 100_000);
    let sent = sent.expect("the second frame went on the cable");
    let on = on.expect("the board drove the cable back");
    let off = off.expect("and let it go again");
    eprintln!(
        "the second frame's first edge at {sent}; the driver on at {on}, {} ns ({} cells) after \
         it, and off at {off}, {} ns ({} cells) later",
        on - sent,
        (on - sent) / CELL_NS,
        off - on,
        (off - on) / CELL_NS
    );
    assert!(
        (48 * CELL_NS..50 * CELL_NS).contains(&(on - sent)),
        "the abort starts in the cell after the destination word, 48 cells in"
    );
    assert!((1_000..1_250).contains(&(off - on)), "four bit cells of abort signal");
    assert_eq!(b.chip.net(abort), Level::Low, "ABORT is over");
    assert_eq!(b.chip.net(lost_one), Level::High, "and so is -LOST.ONE");
    assert_eq!(lost_count(&b), 1, "the frame is counted lost");

    let (_, c) = b.cycle(chaos::CSR, None);
    eprintln!("CSR after the abort: {c:#08o}");
    assert_eq!((c & csr::LOST_COUNT) >> 9, 1, "Lost Count reads 1: {c:#08o}");
    assert!(c & csr::RECEIVE_DONE != 0, "the first packet is still there: {c:#08o}");
    assert!(c & csr::CRC_ERROR == 0, "and its check is still good: {c:#08o}");
    let words = rfc_time(0o3060, MY_ADDRESS);
    let (_, bits) = b.cycle(chaos::BIT_COUNT, None);
    assert_eq!(bits as usize, (words.len() + 2) * 16 - 1, "the bit count is the first packet's");
    let mut back = Vec::new();
    for _ in 0..words.len() + 2 {
        let (_, w) = b.cycle(chaos::READ_BUFFER, None);
        back.push(w);
    }
    assert_eq!(&back[..words.len()], &words[..], "the first packet, unharmed by the abort");

    let log = &b.chaos.as_ref().unwrap().ether.log;
    assert!(
        log.iter().any(|e| matches!(e, Event::Collision(t) if *t >= on)),
        "the transmitter found the abort signal: {log:?}"
    );
    let second = log
        .iter()
        .filter_map(|e| if let Event::Heard(_, f) = e { Some(f) } else { None })
        .nth(1)
        .expect("the second frame ended on the cable");
    assert!(!second.check_ok, "it stopped before the end: {second:?}");
}

/// **A receiver whose buffer is full aborts only what is addressed to
/// it.** AIM-628 §2.5, memo page 6: "Note that a receiver whose packet
/// buffer is full will only generate an abort signal if the packet was
/// specifically addressed to it."
///
/// That is the `MATCH SO FAR` term on the 74S10 at LMMYNM 0D02, the one
/// input `-GOT.ONE`'s 74S00 at 0E04 has not got: `DEST MATCH` out of the
/// 74S287 at 0D01 is the destination matching *or* being zero *or* the
/// software spying, and `MATCH SO FAR` is the bit-by-bit comparison
/// alone. So a broadcast to a full buffer, and any packet at all with Spy
/// set, is counted in Lost Count and not aborted; another station's
/// packet is neither. Measured on the board for all three.
#[test]
fn the_busy_receiver_aborts_only_what_is_addressed_to_it() {
    for (what, dest, spy, lost) in [
        ("another station's packet", 0o3070, false, 0),
        ("a broadcast", 0, false, 1),
        ("another station's packet, spying", 0o3070, true, 1),
    ] {
        let n = cadrio();
        let mut b = board(&n);
        b.cycle(chaos::CSR, Some(csr::RESET));
        b.run(b.now + 2_000);
        b.cycle(chaos::CSR, Some(csr::CLEAR_RECEIVER | if spy { csr::SPY } else { 0 }));
        buffer_full(&mut b, dest);
        let (sent, on, _) = watch_the_driver(&mut b, 100_000);
        assert!(sent.is_some(), "{what}: the second frame went on the cable");
        let (_, c) = b.cycle(chaos::CSR, None);
        eprintln!(
            "{what}: the driver {on:?}, CSR {c:#08o}, Lost Count {}",
            (c & csr::LOST_COUNT) >> 9
        );
        assert_eq!(on, None, "{what}: the board does not drive the cable");
        assert!(c & csr::RECEIVE_DONE != 0, "{what}: the first packet is still there");
        assert_eq!(
            (c & csr::LOST_COUNT) >> 9,
            lost,
            "{what}: what the board would have taken is counted lost, and nothing else"
        );
    }
}

/// **Lost Count wraps at sixteen.** AIM-628 §7 has the field as four bits
/// of packets "which would have been received if the incoming packet
/// buffer had not been busy", and the board counts them in the 74LS161 at
/// LMMYNM 0F04, clocked by `ITS.ME` falling with `-RACT` up and cleared
/// by Clear Receiver. A '161 wraps: eighteen frames for the board with
/// the buffer taken by the first leave seventeen counted and the field
/// reading 1. The behavioral interface wraps with it, which
/// `the_lost_count_wraps_at_sixteen_on_both_alike` in `tests/chaos_rtl.rs`
/// holds.
#[test]
fn the_lost_count_wraps_at_sixteen() {
    let n = cadrio();
    let mut b = board(&n);
    on_the_cable(&mut b);
    let mut node = Capture::new(0o3060);
    for _ in 0..18 {
        node.to_send.push_back(rfc_time(0o3060, MY_ADDRESS));
    }
    let mut ether = Ether::new();
    ether.keep_log(true);
    ether.attach(Box::new(node));
    b.plug_chaos(ether);
    let start = b.now;
    let mut counted = 0;
    let mut last = 0;
    while b.now < start + 1_400_000 {
        let next = b.chip.next_tap().map_or(b.now + 50, |t| t.max(b.now + 1)).min(b.now + 50);
        b.run(next);
        let l = lost_count(&b);
        if l != last {
            counted += 1;
            last = l;
        }
    }
    let sent =
        b.chaos.as_ref().unwrap().ether.log.iter().filter(|e| matches!(e, Event::Sent(..))).count();
    let (_, c) = b.cycle(chaos::CSR, None);
    eprintln!("{sent} frames, Lost Count stepped {counted} times, CSR {c:#08o}");
    assert_eq!(sent, 18, "eighteen frames went by");
    assert_eq!(counted, 17, "seventeen of them were counted lost");
    assert_eq!((c & csr::LOST_COUNT) >> 9, 1, "and the field wrapped: {c:#08o}");
    assert!(c & csr::RECEIVE_DONE != 0, "the first one is still in the buffer");
}

/// The busy-receiver path net by net while a second frame for the board
/// comes in with the first still in the buffer: the destination matching,
/// `-LOST.ONE`, `ABORT`, the line driver, the lost counter, and what the
/// transmitter on the cable made of it. A diagnostic: `--ignored
/// --nocapture`.
#[test]
#[ignore = "a diagnostic, not a check: --ignored --nocapture"]
fn diagnose_the_busy_receiver() {
    let n = cadrio();
    let mut b = board(&n);
    on_the_cable(&mut b);
    buffer_full(&mut b, MY_ADDRESS);
    let watch = [
        "ITS.ME",
        "'DEST MATCH'",
        "'MATCH SO FAR'",
        "-LOST.ONE",
        "ABORT",
        "ABORTSIG",
        "ABORTDN",
        "RACT",
        "PRACT",
        "'RCV BUSY'",
        "RDONE",
        "CBLBSY",
        "'INTERFERENCE IN'",
        "-TTL.D.OUT",
        "START^",
        "'SRC STB'",
        "-LOAD.MY.TURN",
    ];
    let start = b.now;
    let mut last: Vec<Level> = watch.iter().map(|w| b.level(w)).collect();
    let mut lost = lost_count(&b);
    let mut tx = b.chaos.as_ref().unwrap().board_tx(&b.chip);
    eprintln!("the first packet is in the buffer at {start}; Lost Count {lost}");
    while b.now < start + 100_000 {
        let next = b.chip.next_tap().map_or(b.now + 25, |t| t.max(b.now + 1)).min(b.now + 25);
        b.run(next);
        let mut changes = Vec::new();
        for (k, w) in watch.iter().enumerate() {
            let l = b.level(w);
            if l != last[k] {
                changes.push(format!("{}={:?}", w.trim_matches('\''), l));
                last[k] = l;
            }
        }
        let now_lost = lost_count(&b);
        if now_lost != lost {
            changes.push(format!("LSTCNT={now_lost}"));
            lost = now_lost;
        }
        let d = b.chaos.as_ref().unwrap().board_tx(&b.chip);
        if d != tx {
            changes.push(format!("the line driver {}", if d { "ON" } else { "off" }));
            tx = d;
        }
        if !changes.is_empty() {
            eprintln!("[{:>7}] {}", b.now - start, changes.join(" "));
        }
    }
    for ev in &b.chaos.as_ref().unwrap().ether.log {
        match ev {
            Event::Sent(t, s, _) => {
                eprintln!("  ether: {s:o} sent at {}", *t as i64 - start as i64)
            }
            Event::Collision(t) => {
                eprintln!("  ether: collision at {}", *t as i64 - start as i64)
            }
            Event::Heard(t, f) => eprintln!(
                "  ether: heard from {:o} for {:o} at {}, check {}",
                f.source,
                f.buffer.last().copied().unwrap_or(0),
                *t as i64 - start as i64,
                f.check_ok
            ),
        }
    }
    let (_, c) = b.cycle(chaos::CSR, None);
    eprintln!("CSR at the end: {c:#08o}, Lost Count {}", (c & csr::LOST_COUNT) >> 9);
}

// --- The turn after one's own packet, AIM-628 §2.6 -----------------------

/// **Two packets written back to back go a whole round apart all the
/// same.** The turn after the board's own packet comes at the first count
/// after the cable idles --- the counter is loaded with the difference
/// between the source word and its own address, which for its own packet
/// is zero, measured at four addresses by
/// `the_turn_timer_loads_the_address_difference_bit_reversed` in
/// `tests/chaos_rtl.rs` --- and the software cannot be there. Transmit
/// Done comes 250 ns before the frame's nominal end, writing eleven words
/// and reading `START` takes tens of microseconds, and the transmitter is
/// loaded some 4,150 ns later still ([`tsr_empty`]). So the count that would have
/// started the second frame has gone by, and `MY.TURN^`, bit 7 of the
/// 74LS193s at LMTURN 0A17 and 0A18, rises again 256 counts on. Measured:
/// 257,875 ns between the last edge of the first frame and the first edge
/// of the second, 257 slots of 1,000 ns --- the one missed and the 256 of
/// a whole round.
///
/// Which is what AIM-628 §2.6, memo page 7, describes --- "it continues
/// down the cable, passing every other interface, giving them each a
/// chance to transmit before letting the first interface transmit a
/// second packet" --- but by another route, and only for an interface
/// whose software takes a microsecond or two to reload. The board itself
/// puts its own turn first, not last. So an ether node that is ready at
/// its own turn --- a host handing muir a burst over CHUDP --- gets the
/// cable back one slot after it let it go, and a machine on the other end
/// has that one slot to empty its buffer. Issue 110.
#[test]
fn two_packets_back_to_back_wait_a_whole_round() {
    use muir::chaos::board::tsr_empty;
    let n = cadrio();
    let mut b = board(&n);
    on_the_cable(&mut b);
    let mut ether = Ether::new();
    ether.keep_log(true);
    ether.attach(Box::new(Capture::new(0o3060)));
    b.plug_chaos(ether);
    let words = rfc_time(MY_ADDRESS, 0o3060);
    let mut edges: Vec<u64> = Vec::new();
    let mut starts: Vec<u64> = Vec::new();
    for round in 0..2 {
        for &w in &words {
            b.cycle(chaos::WRITE_BUFFER, Some(w));
        }
        let at = b.now;
        let (took, _) = b.cycle(chaos::START, None);
        starts.push(at + muir::busint::UNIBUS_ADDRESS_NS + took);
        let deadline = b.now + 700_000;
        let mut tx = b.chaos.as_ref().unwrap().board_tx(&b.chip);
        let was = edges.len();
        loop {
            let next = b.chip.next_tap().map_or(b.now + 25, |t| t.max(b.now + 1)).min(b.now + 25);
            b.run(next);
            let d = b.chaos.as_ref().unwrap().board_tx(&b.chip);
            if d != tx {
                edges.push(b.now);
                tx = d;
            }
            if edges.len() > was
                && b.level("TDONE") == Level::High
                && b.level("CBLBSY") == Level::Low
            {
                break;
            }
            assert!(b.now < deadline, "round {round}: the frame never went out");
        }
    }
    // The frames are the runs of driver edges: nothing within a frame is
    // more than a bit cell apart.
    let mut runs: Vec<(u64, u64)> = Vec::new();
    let mut from = edges[0];
    for w in edges.windows(2) {
        if w[1] - w[0] > 2 * CELL_NS {
            runs.push((from, w[0]));
            from = w[1];
        }
    }
    runs.push((from, *edges.last().unwrap()));
    for &(s, e) in &runs {
        eprintln!("a frame on the cable from {s} to {e}");
    }
    assert_eq!(runs.len(), 2, "two frames went out: {runs:?}");
    let gap = runs[1].0 - runs[0].1;
    let ready = tsr_empty(starts[1], 2, 0);
    eprintln!(
        "START answered at {:?}; the transmitter loaded at {ready}, {} ns after the cable idled; \
         the gap is {gap} ns, {} slots",
        starts,
        ready as i64 - runs[0].1 as i64,
        gap / muir::chaos::ether::SLOT_NS
    );
    assert!(ready > runs[0].1 + muir::chaos::ether::SLOT_NS, "the first turn was missed");
    assert_eq!(gap / muir::chaos::ether::SLOT_NS, 257, "a missed turn and a whole round: {gap} ns");
}

/// **The instants the behavioral interface keeps are the netlist board's.**
/// `chaos::board` times a frame from its constants rather than its
/// counters; here the board runs a looped-back frame and each instant is
/// measured on its nets and held to the constant that stands for it:
///
/// - `TSREMPTY` where [`tsr_empty`] puts it after the START read is
///   answered, at whatever phase of the board's clocks the read comes ---
///   4,150 ns after an answer that waited for the last word's shift, which
///   a read up to 1,750 ns after the last write's cycle does, and 4,101 to
///   4,225 ns after one that did not, by the phase --- and `rtl` hands the
///   read to the model at the answer;
/// - the frame's first edge on `-TTL.D.OUT` [`TURN_START_NS`] after
///   `MY.TURN^` rises;
/// - `-TDONE` [`TDONE_BEFORE_END_NS`] before the frame's nominal end, its
///   first edge plus a cell for each of its bits, and `-CBLBSY` lifting,
///   `RDONE` with it, [`CBLBSY_OFF_NS`] after;
/// - `MY.TURN CLK^` rising [`TURN_FIRST_TC_NS`] after power-on and every
///   two terminal counts from there.
#[test]
fn the_models_instants_are_the_boards() {
    use muir::chaos::board::{
        CBLBSY_OFF_NS, TDONE_BEFORE_END_NS, TURN_FIRST_TC_NS, TURN_START_NS, TURN_TC_NS, tsr_empty,
    };
    use muir::chaos::packet::frame;
    use muir::chaos::wire::CELL_NS;
    let n = cadrio();
    let words = rfc_time(MY_ADDRESS, MY_ADDRESS);
    let tap = |b: &mut UnibusMaster| {
        let next = b.chip.next_tap().map_or(b.now + 50, |t| t.max(b.now + 1)).min(b.now + 500);
        b.run(next);
    };
    for delay in [0, 375, 750, 1125, 1500, 1750, 1800, 1850, 1900, 1950, 2000, 2050] {
        let mut b = board(&n);
        b.cycle(chaos::CSR, Some(csr::RESET));
        b.run(b.now + 2_000);
        b.cycle(chaos::CSR, Some(csr::LOOP_BACK | csr::CLEAR_RECEIVER));
        for &w in &words {
            b.cycle(chaos::WRITE_BUFFER, Some(w));
        }
        b.run(b.now + delay);
        let begin = b.now;
        let (took, _) = b.cycle(chaos::START, None);
        let answered = begin + muir::busint::UNIBUS_ADDRESS_NS + took;
        while b.level("TSREMPTY") != Level::High {
            tap(&mut b);
        }
        assert_eq!(
            b.now,
            tsr_empty(answered, 2, 0),
            "TSREMPTY after START, {delay} ns later, answered in {took} ns"
        );
        eprintln!(
            "START {delay} ns later: answered in {took} ns, TSREMPTY {} ns after",
            b.now - answered
        );
        if delay != 0 {
            continue;
        }
        // The rest of the frame, on the one run.
        let (mut turn, mut first, mut last, mut tdone, mut off) = (None, None, None, None, None);
        let mut clk_rises = Vec::new();
        let mut was = [
            b.level("MY.TURN^"),
            b.level("-TTL.D.OUT"),
            b.level("-TDONE"),
            b.level("-CBLBSY"),
            b.level("'MY.TURN CLK^'"),
        ];
        while off.is_none() {
            tap(&mut b);
            let now = [
                b.level("MY.TURN^"),
                b.level("-TTL.D.OUT"),
                b.level("-TDONE"),
                b.level("-CBLBSY"),
                b.level("'MY.TURN CLK^'"),
            ];
            if now[0] == Level::High && was[0] != Level::High && first.is_none() {
                turn = Some(b.now);
            }
            if now[1] != was[1] {
                first.get_or_insert(b.now);
                last = Some(b.now);
            }
            if now[2] == Level::Low && was[2] != Level::Low {
                tdone = Some(b.now);
            }
            if now[3] == Level::High && was[3] != Level::High && first.is_some() {
                off = Some(b.now);
            }
            if now[4] == Level::High && was[4] != Level::High {
                clk_rises.push(b.now);
            }
            was = now;
        }
        let (turn, first, _last) = (turn.unwrap(), first.unwrap(), last.unwrap());
        let end = first + frame(&words, MY_ADDRESS).len() as u64 * CELL_NS;
        assert_eq!(first - turn, TURN_START_NS, "the first edge after the turn");
        assert_eq!(end - tdone.unwrap(), TDONE_BEFORE_END_NS, "-TDONE before the nominal end");
        assert_eq!(off.unwrap() - end, CBLBSY_OFF_NS, "-CBLBSY lifting after the nominal end");
        for r in clk_rises {
            assert_eq!((r - TURN_FIRST_TC_NS) % (2 * TURN_TC_NS), 0, "MY.TURN CLK^ at {r}");
        }
    }
}

/// One register cycle on the netlist board and the same on the
/// behavioral interface at the instant the board's cycle ended; the words
/// read, the board's first.
fn on_both(
    b: &mut UnibusMaster,
    m: &mut muir::chaos::board::Interface,
    uaddr: u32,
    write: Option<u16>,
) -> (u16, u16) {
    let (_, wb) = b.cycle(uaddr, write);
    let wm = match write {
        Some(v) => {
            m.write(uaddr, v, b.now);
            0
        }
        None => m.read(uaddr, b.now),
    };
    (wb, wm)
}

/// **A START read with Transmit Done up lowers it until the frame ends.**
/// AIM-628 §7 has Transmit Done "cleared to 0 when a word is written into
/// the outgoing packet buffer", and that is the one way software reaches
/// a START with it down. With no word written since the last frame, or
/// since a Reset, it is up when START is read, and the board lowers it.
///
/// Measured: `TDONE`, the 74S74 at LMMODU 0D08, is cleared by `TRESET`
/// out of the 74S00s at LMTBFC 0C15, which `UB>TSR` raises for a read of
/// START or a write of the buffer only while `-TDONE` is low, through the
/// 74S32 at 0D15 --- so it falls as `-MSYN*` of the START read falls,
/// asynchronously, and `TRESET` holds it, with the transmit counter and
/// the transmit 9401 cleared, until `-MSYN*` lifts. The source word and
/// its check word go in, and the frame --- those two words and the zero,
/// 33 cells --- goes at the turn timer's next count, as any frame does.
/// `TDONE` rises when `-TBUSY` does, clocking 0D08: one `FCLK^` period
/// after `-TODN`, the borrow of the 25LS193 at LMTBUF 0C13, and
/// [`muir::chaos::board::TDONE_BEFORE_END_NS`] before the frame's nominal
/// end, the same instant a frame of words gets. A START read after words
/// were written finds Transmit Done down already, `TRESET` is not raised,
/// and nothing changes.
///
/// The behavioral interface lowers it at the read and raises it at the
/// same instant: Transmit Done at the read's answer, the frame's first
/// edge, the instant before `TDONE` rises and that instant, on both.
#[test]
fn an_empty_start_lowers_transmit_done_until_its_frame_ends() {
    use muir::chaos::board::{Interface, TDONE_BEFORE_END_NS};
    use muir::chaos::packet::frame;
    let n = cadrio();
    let mut b = board(&n);
    let mut m = Interface::new(MY_ADDRESS, None, 0, false);
    on_both(&mut b, &mut m, chaos::CSR, Some(csr::RESET));
    b.run(b.now + 2_000);
    m.advance(b.now);
    on_both(&mut b, &mut m, chaos::CSR, Some(csr::LOOP_BACK | csr::CLEAR_RECEIVER));
    let (cb, cm) = on_both(&mut b, &mut m, chaos::CSR, None);
    assert!(cb & csr::TRANSMIT_DONE != 0, "Transmit Done up after the reset: {cb:#08o}");
    assert_eq!(cm, cb, "the CSR before START");
    let (me, _) = on_both(&mut b, &mut m, chaos::START, None);
    assert_eq!(me, MY_ADDRESS, "START reads the interface's own address");
    // Transmit Done on both, at instants of the board's own: once START
    // is answered, at the frame's first edge, the last instant before
    // `TDONE` rises, and that instant.
    let mut samples: Vec<(&str, u64, bool)> = vec![("START answered", b.now, false)];
    let tdone = |b: &UnibusMaster| b.level("TDONE") == Level::High;
    samples[0].2 = tdone(&b);
    let (mut first, mut before) = (None, b.now);
    let mut was = b.level("-TTL.D.OUT");
    let deadline = b.now + 3_000_000;
    while !tdone(&b) {
        before = b.now;
        let next = b.chip.next_tap().map_or(b.now + 50, |t| t.max(b.now + 1)).min(b.now + 500);
        b.run(next);
        let d = b.level("-TTL.D.OUT");
        if d != was && first.is_none() {
            first = Some(b.now);
            samples.push(("the frame's first edge", b.now, tdone(&b)));
        }
        was = d;
        assert!(b.now < deadline, "Transmit Done never came back");
    }
    let first = first.expect("a frame went out");
    let rise = b.now;
    samples.push(("before TDONE rose", before, false));
    samples.push(("TDONE rose", rise, true));
    let end = first + frame(&[], MY_ADDRESS).len() as u64 * CELL_NS;
    eprintln!(
        "the frame from {first}, its nominal end {end}; TDONE up at {rise}, {} ns before it",
        end - rise
    );
    assert_eq!(end - rise, TDONE_BEFORE_END_NS, "TDONE before the empty frame's nominal end");
    let board: Vec<bool> = samples.iter().map(|s| s.2).collect();
    assert_eq!(board, [false, false, false, true], "the board's Transmit Done: {samples:?}");
    let model: Vec<bool> = samples
        .iter()
        .map(|&(_, t, _)| {
            m.advance(t);
            m.csr() & csr::TRANSMIT_DONE != 0
        })
        .collect();
    assert_eq!(model, board, "the model's Transmit Done at {samples:?}");
}

/// **A START read with no word written sends the same frame from both:
/// the source and its check word.** The board's transmit counter and its
/// 9401 are cleared by the START read itself (above), and the frame is
/// the source word and its CRC-16, then the zero: 33 cells, which is
/// `chaos::packet::frame` of an empty buffer, and the check word is the
/// right one for the source. The behavioral interface puts the same
/// frame on the cable.
///
/// Neither interface's receiver takes it. The ether reads it back as two
/// words, which `chaos::packet::unframe_any` does not call a good packet
/// --- fewer than the three hardware words. The board's own receiver,
/// under Spy, which takes its own frames of words off the cable, sets
/// neither Receive Done nor CRC Error for it, and Lost Count stays 0; and
/// the behavioral interface at 3060, under Spy on the same cable as the
/// board, does the same.
#[test]
fn an_empty_start_sends_the_source_and_its_check_word_from_both() {
    use muir::chaos::board::Interface;
    use muir::chaos::packet::{Framed, check_word, frame};
    let n = cadrio();
    let mut b = board(&n);
    b.cycle(chaos::CSR, Some(csr::RESET));
    b.run(b.now + 2_000);
    b.cycle(chaos::CSR, Some(csr::SPY | csr::CLEAR_RECEIVER));
    let mut ether = Ether::new();
    ether.keep_log(true);
    ether.attach(Box::new(Capture::new(0o3060)));
    b.plug_chaos(ether);
    b.cycle(chaos::START, None);
    let mut tx = b.chaos.as_ref().unwrap().board_tx(&b.chip);
    let mut edges = Vec::new();
    let deadline = b.now + 3_000_000;
    loop {
        let next = b.chip.next_tap().map_or(b.now + 25, |t| t.max(b.now + 1)).min(b.now + 25);
        b.run(next);
        let d = b.chaos.as_ref().unwrap().board_tx(&b.chip);
        if d != tx {
            edges.push((b.now, d));
            tx = d;
        }
        if !edges.is_empty() && b.level("TDONE") == Level::High && b.level("-CBLBSY") == Level::High
        {
            break;
        }
        assert!(b.now < deadline, "the frame never went out");
    }
    b.run(b.now + 5_000);
    let runs = muir::chaos::wire::decode(&edges);
    assert_eq!(runs, [frame(&[], MY_ADDRESS)], "the board's line driver: the source and its check");
    let empty = Framed {
        buffer: Vec::new(),
        source: MY_ADDRESS,
        check: check_word(&[MY_ADDRESS]),
        check_ok: false,
    };
    let heard = |log: &[Event]| -> Vec<Framed> {
        log.iter()
            .filter_map(|e| if let Event::Heard(_, f) = e { Some(f.clone()) } else { None })
            .collect()
    };
    let board_heard = heard(&b.chaos.as_ref().unwrap().ether.log);
    assert_eq!(board_heard, std::slice::from_ref(&empty), "the board's frame as the ether read it");
    let (_, c) = b.cycle(chaos::CSR, None);
    eprintln!("the board's CSR after its empty frame: {c:#08o}");
    assert!(c & (csr::RECEIVE_DONE | csr::CRC_ERROR | csr::LOST_COUNT) == 0, "not taken: {c:#08o}");

    let mut ether = Ether::new();
    ether.keep_log(true);
    ether.attach(Box::new(Capture::new(0o3060)));
    let mut m = Interface::new(MY_ADDRESS, Some(ether), 0, false);
    let mut now = 10_000;
    m.write(chaos::CSR, csr::RESET, now);
    m.write(chaos::CSR, csr::SPY | csr::CLEAR_RECEIVER, now);
    m.read(chaos::START, now);
    while now < 1_000_000 {
        now += 1_000;
        m.advance(now);
    }
    assert_eq!(
        heard(&m.ether().unwrap().log),
        std::slice::from_ref(&empty),
        "the model's frame as the ether read it"
    );

    // The board's frame into the behavioral interface at 3060, which keeps
    // the cable; the board reaches it through its transceiver.
    use muir::chaos::cable::Transceiver;
    let mut b = board(&n);
    b.cycle(chaos::CSR, Some(csr::RESET));
    b.run(b.now + 2_000);
    b.cycle(chaos::CSR, Some(csr::CLEAR_RECEIVER));
    let mut ether = Ether::new();
    ether.keep_log(true);
    let mut m = Interface::new(0o3060, Some(ether), 0, false);
    m.write(chaos::CSR, csr::RESET, b.now);
    m.write(chaos::CSR, csr::SPY | csr::CLEAR_RECEIVER, b.now);
    let mut t = Transceiver::new(&n);
    t.apply(&mut b.chip, m.ether_mut().unwrap(), b.now);
    b.cycle(chaos::START, None);
    let until = b.now + 1_000_000;
    while b.now < until {
        let now = b.now;
        let cable = m.ether().unwrap().next_due().map(|x| x.max(now));
        let next =
            [b.chip.next_tap(), cable].into_iter().flatten().min().map_or(until, |x| x.min(until));
        b.now = next.max(now);
        b.chip.transition(b.now);
        m.advance(b.now);
        t.apply(&mut b.chip, m.ether_mut().unwrap(), b.now);
    }
    assert_eq!(heard(&m.ether().unwrap().log), [empty], "the board's frame on the model's cable");
    let c = m.read(chaos::CSR, b.now);
    eprintln!("the model's CSR after the board's empty frame: {c:#08o}");
    assert!(c & (csr::RECEIVE_DONE | csr::CRC_ERROR | csr::LOST_COUNT) == 0, "not taken: {c:#08o}");
}

// --- The transmitter's shift register holds the Unibus -------------------

/// **A word written while the last one is still shifting is answered when
/// the shift register is empty, as the board answers it, and so is
/// START.** The board loads each word into its transmit shift register at
/// the first edge of its 8 MHz clock at least 101 ns after the write's
/// answer --- `-MSYN` lifting 100 ns after `-SSYN`, and the load on the
/// clock --- and shifts its sixteen bits into the buffer, 2,000 ns; a
/// WRITE BUFFER or a START that comes before `TSREMPTY` is up again is
/// answered 350 ns after it is. Six words and a START, spaced from 0 to
/// 2,500 ns after each cycle's end and at five phases of the clock: every
/// answer on the board is the one `muir::busint::IoBoardTiming` gives the
/// same cycles, which is how `rtl` answers them.
#[test]
fn a_word_written_while_the_last_shifts_is_answered_as_the_board_answers() {
    use muir::busint::{IoBoardTiming, UNIBUS_ADDRESS_NS};
    let n = cadrio();
    let mut wrong = Vec::new();
    for gap in [0u64, 500, 1_000, 1_500, 1_750, 2_000, 2_500] {
        for phase in [0u64, 25, 50, 75, 100] {
            let mut b = board(&n);
            let mut timing = IoBoardTiming::default();
            let mut cycle = |b: &mut UnibusMaster, uaddr: u32, write: Option<u16>| {
                let msyn = b.now + UNIBUS_ADDRESS_NS;
                let (took, _) = b.cycle(uaddr, write);
                (msyn + took, timing.answer_cycle(uaddr, write.is_some(), msyn))
            };
            cycle(&mut b, chaos::CSR, Some(csr::RESET));
            b.run(b.now + 2_000 + phase);
            cycle(&mut b, chaos::CSR, Some(csr::LOOP_BACK | csr::CLEAR_RECEIVER));
            for (k, w) in [0o125252u16, 0o052525, 0o177400, 0o000377, 0o007417, MY_ADDRESS]
                .into_iter()
                .enumerate()
            {
                let (board, model) = cycle(&mut b, chaos::WRITE_BUFFER, Some(w));
                if model != board {
                    wrong.push(format!(
                        "gap {gap} phase {phase}: word {k} answered at {board}, the model {model}"
                    ));
                }
                b.run(b.now + gap);
            }
            let (board, model) = cycle(&mut b, chaos::START, None);
            if model != board {
                wrong.push(format!(
                    "gap {gap} phase {phase}: START answered at {board}, the model {model}"
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{} answers unlike the board's:\n{}", wrong.len(), wrong.join("\n"));
}
