// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's register page (contracts Q2 and Q13): 256 words at physical
//! `1777777400`-`1777777777`, the last page of the 28-bit physical space
//! (contract G2 §4.1).
//! Words 0-77 are the feature page, 100 the interrupt status, 101 the error
//! status, 102 the mode, 103 the real-time clock and 104 reset devices;
//! then the interval timers (110-115), the keyboard and the mouse
//! (120-123), the network (140-145), the file device (160-171),
//! block-disk (200-203) and the video controller (210). Reserved words
//! read 0 and ignore writes. It takes the place of the Unibus interrupt
//! vector (`766040`), the error status (`766044`) and the mode register's
//! error stop (`766012`) on QUUX.
//!
//! [`every_word_of_the_page`] holds all 256 words in one table.

use muir::block_disk::{BLOCK_NS, BlockDisk};
use muir::chaos::interface::csr;
use muir::disk_unit::Geometry as Pack;
use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{
    ALU, ALWAYS, JUMP, MD, N, SETM, SRC_MD, START_READ, START_WRITE, a_dest, filler, m_src, target,
};
use muir::machine::{Geometry, Machine, bus_error};
use muir::micro::Micro;
use muir::quux_input::KeyboardMouse;
use muir::rtl::Rtl;

mod support;

const PAGE: u32 = muir::machine::REGISTER_PAGE_13;
const INTERRUPTS: u32 = PAGE + 0o100;
const ERRORS: u32 = PAGE + 0o101;
const MODE: u32 = PAGE + 0o102;

/// A QUUX machine as `quux` builds one: the video controller at its
/// default size, and block-disk with a disk.
fn quux() -> Machine {
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
    m.tv.set_board(muir::tv::Board::Video);
    let mut d = BlockDisk::new(BLOCK_NS);
    d.attach(muir::disk_image::Disk::blank(Pack::T300.blocks()));
    m.block_disk = Some(d);
    m
}

/// The machine's whole state, as a checkpoint has it.
fn state(m: &Machine) -> Vec<u8> {
    let mut w = muir::checkpoint::Writer::new();
    m.save(&mut w);
    w.finish()
}

// --- word 100 --------------------------------------------------------------

/// The eight sources of word 100, in its order (contract Q13, section 2),
/// each raised alone on a machine of its own at power-on: what it takes,
/// and the bit.
fn raise(source: usize) -> (Machine, &'static str) {
    let mut m = quux();
    let name = match source {
        // Timer k at 50 us, turned on under its interrupt enable.
        k @ 0..=2 => {
            m.bus_write(PAGE + 0o111 + 2 * k as u32, 50);
            m.bus_write(PAGE + 0o110 + 2 * k as u32, 0o401);
            m.ns = 50_000;
            ["timer 0", "timer 1", "timer 2"][k]
        }
        // Block-disk's done, under command <11>: a page of 5 blocks.
        3 => {
            m.main[0o100] = 0o2000;
            m.bus_write(PAGE + 0o201, 0o100);
            m.bus_write(PAGE + 0o200, 1 << 11);
            m.bus_write(PAGE + 0o203, 0);
            m.ns += 5 * BLOCK_NS;
            "block-disk"
        }
        // A key word waiting, under 120 <8>.
        4 => {
            m.quux_input.press(0o101);
            m.bus_write(PAGE + 0o120, 1 << 8);
            "keyboard"
        }
        // The mouse moved, under 123 <8>.
        5 => {
            m.quux_input.mouse_move(1, 0);
            m.bus_write(PAGE + 0o123, 1 << 8);
            "mouse"
        }
        // The Chaosnet interface's transmit done, under its enable.
        6 => {
            m.bus_write(
                PAGE + 0o140,
                ((csr::CLEAR_TRANSMITTER | csr::TRANSMIT_INT_ENABLE) as u32).into(),
            );
            "network"
        }
        // A response waiting, under 160 <8>: a command of no opcode, which
        // the device answers with a failure.
        7 => {
            file_device_response(&mut m);
            "file device"
        }
        _ => unreachable!(),
    };
    (m, name)
}

/// The file device's rings at `100000` and `110000`, four entries each, the
/// device enabled under its interrupt enable, one command of opcode 0
/// posted, and the clock moved on until it is answered.
fn file_device_response(m: &mut Machine) {
    m.bus_write(PAGE + 0o162, 0o100000);
    m.bus_write(PAGE + 0o163, 2);
    m.bus_write(PAGE + 0o166, 0o110000);
    m.bus_write(PAGE + 0o167, 2);
    m.bus_write(PAGE + 0o160, 0x101);
    m.main[0o100000] = 1;
    m.bus_write(PAGE + 0o164, 1);
    while m.bus_read(PAGE + 0o170) == 0 {
        m.ns += 1_000;
        assert!(m.ns < 1_000_000_000, "the file device never answered");
    }
}

/// **Word 100 says who interrupted, a bit each in Q13's order**: `<0>`-`<2>`
/// timers 0-2, `<3>` block-disk, `<4>` the keyboard, `<5>` the mouse, `<6>`
/// the network, `<7>` the file device. Each source raised alone reads
/// exactly its own bit, and the processor's interrupt is pending with it.
#[test]
fn word_100_says_who_interrupted() {
    assert_eq!(quux().bus_read(INTERRUPTS), 0, "nothing at power-on");
    assert!(!quux().interrupt());
    for bit in 0..8 {
        let (mut m, name) = raise(bit);
        assert_eq!(m.bus_read(INTERRUPTS), 1 << bit, "{name} alone");
        assert_eq!(m.interrupt_sources(), 1 << bit, "{name}: interrupt_sources");
        assert!(m.interrupt(), "{name}: the processor's interrupt");
    }
}

/// **A stray's handler turns its source off with one write** (contract Q13,
/// section 2): what microcode 2001 writes for a bit it does not serve drops
/// that bit, and only it. Timers 1 and 2: 112 and 114 written 0. Block-disk
/// with the disk idle: 200 written 0. The mouse: 123 written 0. The file
/// device: 160 read and written back with `<8>` clear and `<0>` kept, the
/// device staying enabled with its response still waiting.
#[test]
fn a_stray_s_write_turns_its_bit_off() {
    for (bit, word) in [(1, 0o112), (2, 0o114), (3, 0o200), (5, 0o123), (7, 0o160)] {
        let (mut m, name) = raise(bit);
        assert_eq!(m.bus_read(INTERRUPTS), 1 << bit, "{name} up");
        let v = if word == 0o160 { m.bus_read(PAGE + 0o160) & !(1 << 8) } else { 0 };
        m.bus_write(PAGE + word, v);
        assert_eq!(m.bus_read(INTERRUPTS), 0, "{name}: off after {word:o} <- {v:o}");
        assert!(!m.interrupt(), "{name}: no interrupt pending");
        if word == 0o160 {
            assert_eq!(m.bus_read(PAGE + 0o160), 1, "the file device still enabled");
            assert_ne!(m.bus_read(PAGE + 0o170), m.bus_read(PAGE + 0o171), "its response waits");
        }
    }
}

/// **Word 101 is the error status**: the bus errors, which a write clears,
/// as a write of `766044` does.
#[test]
fn word_101_is_the_error_status() {
    let mut m = quux();
    assert_eq!(m.bus_read(ERRORS), 0);
    m.bus_read(0o17376000); // an empty Xbus I/O address
    assert_ne!(m.bus_error & bus_error::XBUS_NXM, 0, "the read timed out");
    assert_eq!(m.bus_read(ERRORS), (m.bus_error as u32).into());
    assert_ne!(m.bus_read(ERRORS) & u64::from(bus_error::XBUS_NXM as u32), 0);
    m.bus_write(ERRORS, 0);
    assert_eq!((m.bus_error, m.bus_read(ERRORS)), (0, 0), "cleared");
}

/// **Word 102 is the mode: `<0>` error stop**, the bit `(si:%halt)` relies
/// on, which the CADR has in its mode register.
#[test]
fn word_102_is_error_stop() {
    let mut m = quux();
    assert_eq!(m.bus_read(MODE), 0);
    m.bus_write(MODE, 1);
    assert!(m.mode.errstop);
    assert_eq!(m.bus_read(MODE), 1);
    m.bus_write(MODE, 0);
    assert!(!m.mode.errstop);
    // The host sets it as it sets the spy's mode register, and the word
    // shows it.
    m.mode.errstop = true;
    assert_eq!(m.bus_read(MODE), 1);
}

// --- the page as a whole ----------------------------------------------

/// How a word answers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    /// Read; a write changes nothing.
    ReadOnly,
    /// Read and written.
    ReadWrite,
    /// Written; reads 0.
    WriteOnly,
    /// Reads 0; a write changes nothing.
    Reserved,
}
use Class::*;

/// Every word of the page: its class, and what it reads at power-on on
/// [`quux`]'s machine. Word 103, the real-time clock, reads the host's
/// time, and [`POWER_ON_103`] stands for it.
fn table() -> [(Class, u32); 256] {
    let id = Geometry::QUUX.machine_id.unwrap();
    let mut t = [(Reserved, 0); 256];
    // The feature page, 0-77: 0-17 the machine's, 20-24 the board name,
    // the rest 0.
    for (w, v) in [id, 7, 4096, 16384, 16384, 1024, 4096, 3, 1].into_iter().enumerate() {
        t[w] = (ReadOnly, v);
    }
    t[0o11] = (ReadOnly, 1280 << 16 | 1024);
    t[0o12] = (ReadOnly, 1 << 16 | 40);
    t[0o13] = (ReadOnly, muir::machine::WINDOW_13);
    t[0o14] = (ReadOnly, 1);
    t[0o15] = (ReadOnly, 3);
    t[0o16] = (ReadOnly, 3);
    t[0o17] = (ReadOnly, 1024);
    t[0o20..=0o77].fill((ReadOnly, 0));
    // muir-sim's board name, 4 characters a word, the first in `<7:0>`, and
    // zero bytes after it.
    t[0o20] = (ReadOnly, u32::from_le_bytes(*b"muir"));
    t[0o21] = (ReadOnly, u32::from_le_bytes(*b"-sim"));
    // The page's own words.
    t[0o100] = (ReadOnly, 0);
    t[0o101] = (ReadWrite, 0);
    t[0o102] = (ReadWrite, 0);
    t[0o103] = (ReadOnly, POWER_ON_103);
    t[0o104] = (WriteOnly, 0);
    // The interval timers: control and period of each, all off.
    t[0o110..=0o115].fill((ReadWrite, 0));
    // The keyboard and the mouse.
    t[0o120] = (ReadWrite, 0);
    t[0o121] = (ReadOnly, 0);
    t[0o122] = (ReadOnly, 0);
    t[0o123] = (ReadWrite, 0);
    // The network: the CSR, my address and the write buffer, the read
    // buffer, the bit count, START.
    t[0o140] = (ReadWrite, POWER_ON_CSR);
    t[0o141] = (ReadWrite, POWER_ON_MY_ADDRESS);
    t[0o142] = (ReadOnly, 0);
    t[0o143] = (ReadOnly, POWER_ON_BIT_COUNT);
    // START reads the address, as the CADR's board does at `764152`.
    t[0o145] = (ReadOnly, POWER_ON_MY_ADDRESS);
    // The file device.
    for w in [0o160, 0o162, 0o163, 0o164, 0o166, 0o167, 0o171] {
        t[w] = (ReadWrite, 0);
    }
    t[0o161] = (ReadOnly, 2); // quiet
    t[0o165] = (ReadOnly, 0);
    t[0o170] = (ReadOnly, 0);
    // Block-disk: status and command, the last memory address and the
    // command list pointer, the disk address, START.
    t[0o200] = (ReadWrite, 1); // not active
    t[0o201] = (ReadWrite, 0);
    t[0o202] = (ReadWrite, 0);
    t[0o203] = (WriteOnly, 0);
    // The video controller's mode.
    t[0o210] = (ReadWrite, 0);
    t
}

/// Stands for the host's time in [`table`]'s word 103.
const POWER_ON_103: u32 = u32::MAX;
/// What the network's CSR reads at power-on: transmit done, and the lost
/// count's field as the board leaves it.
const POWER_ON_CSR: u32 = 0o200;
/// The machine's own Chaosnet address at power-on, `--chaos-address`'s
/// default 177001.
const POWER_ON_MY_ADDRESS: u32 = 0o177001;
/// The bit count at power-on: nothing received.
const POWER_ON_BIT_COUNT: u32 = 0;

/// Whether a read or a write has the effect.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Effect {
    Read,
    Write,
}

/// **The words whose read or write does something**, each with the test
/// that holds what it does: file and function.
const EFFECTS: &[(u32, Effect, &str, &str)] = &[
    (0o101, Effect::Write, "quux_registers.rs", "word_101_is_the_error_status"),
    (0o102, Effect::Write, "quux_registers.rs", "word_102_is_error_stop"),
    (
        0o104,
        Effect::Write,
        "quux_reset_devices.rs",
        "m5_reset_devices_does_what_revision_9_s_28_did_and_resets_the_timers",
    ),
    (
        0o110,
        Effect::Write,
        "interval_timers.rs",
        "m1_a_periodic_timer_rises_every_period_on_its_start_s_grid",
    ),
    (
        0o111,
        Effect::Write,
        "interval_timers.rs",
        "m1_a_periodic_timer_rises_every_period_on_its_start_s_grid",
    ),
    (0o112, Effect::Write, "interval_timers.rs", "m3_a_write_touches_its_timer_alone"),
    (0o113, Effect::Write, "interval_timers.rs", "m3_a_write_touches_its_timer_alone"),
    (0o114, Effect::Write, "interval_timers.rs", "m3_a_write_touches_its_timer_alone"),
    (0o115, Effect::Write, "interval_timers.rs", "m3_a_write_touches_its_timer_alone"),
    (0o120, Effect::Write, "quux_input.rs", "each_interrupts_under_its_enable"),
    (0o121, Effect::Read, "quux_input.rs", "keys_come_out_in_order"),
    (0o122, Effect::Read, "quux_input.rs", "the_mouse_counts_and_its_buttons"),
    (0o123, Effect::Write, "quux_input.rs", "each_interrupts_under_its_enable"),
    (0o140, Effect::Write, "quux_network.rs", "each_word_is_its_unibus_register"),
    (0o141, Effect::Write, "quux_network.rs", "a_frame_goes_out_and_its_answer_comes_back"),
    (0o142, Effect::Read, "quux_network.rs", "a_frame_goes_out_and_its_answer_comes_back"),
    (0o145, Effect::Read, "quux_network.rs", "a_frame_goes_out_and_its_answer_comes_back"),
    (
        0o160,
        Effect::Write,
        "quux_file_device.rs",
        "the_registers_read_and_write_as_the_layout_says",
    ),
    (
        0o162,
        Effect::Write,
        "quux_file_device.rs",
        "the_registers_read_and_write_as_the_layout_says",
    ),
    (
        0o163,
        Effect::Write,
        "quux_file_device.rs",
        "the_registers_read_and_write_as_the_layout_says",
    ),
    (0o164, Effect::Write, "quux_file_device.rs", "commands_run_one_at_a_time_in_order"),
    (
        0o166,
        Effect::Write,
        "quux_file_device.rs",
        "the_registers_read_and_write_as_the_layout_says",
    ),
    (
        0o167,
        Effect::Write,
        "quux_file_device.rs",
        "the_registers_read_and_write_as_the_layout_says",
    ),
    (0o171, Effect::Write, "quux_file_device.rs", "a_full_response_ring_holds_the_next_command"),
    (0o200, Effect::Write, "block_disk.rs", "on_quux_the_registers_are_the_block_disk_s"),
    (0o201, Effect::Write, "block_disk.rs", "on_quux_the_registers_are_the_block_disk_s"),
    (0o202, Effect::Write, "block_disk.rs", "on_quux_the_registers_are_the_block_disk_s"),
    (0o203, Effect::Write, "block_disk.rs", "on_quux_the_registers_are_the_block_disk_s"),
    (0o210, Effect::Write, "video.rs", "it_keeps_black_on_white_and_never_interrupts"),
];

/// **Every word of the page, in one table**: at power-on each reads
/// what [`table`] says, with no NXM; a write of all ones to a reserved or
/// read-only word leaves the machine's whole state as it was, byte for
/// byte; and the words with an effect ([`EFFECTS`]) are exactly the ones the
/// table says are written, each named with a test that exists.
#[test]
fn every_word_of_the_page() {
    let t = table();
    let before =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
            as u32;
    let mut m = quux();
    for (w, &(class, want)) in t.iter().enumerate() {
        let got = m.bus_read(PAGE + w as u32);
        if want == POWER_ON_103 {
            assert!(
                got >= before.into() && got <= (before + 5).into(),
                "word 103 is the host's time: {got}"
            );
        } else {
            assert_eq!(got, want.into(), "word {w:o} ({class:?}) at power-on");
        }
        assert_eq!(m.bus_error, 0, "word {w:o} answered");
    }
    // Writes of all ones.
    for (w, &(class, _)) in t.iter().enumerate() {
        if matches!(class, Reserved | ReadOnly) {
            let mut m = quux();
            let s = state(&m);
            m.bus_write(PAGE + w as u32, !0);
            assert_eq!(m.bus_error, 0, "word {w:o} answered the write");
            assert!(state(&m) == s, "word {w:o} ({class:?}): a write changed the machine");
        }
    }
    // The effects and the table agree.
    for (w, &(class, _)) in t.iter().enumerate() {
        let w = w as u32;
        let written = EFFECTS.iter().any(|e| e.0 == w && e.1 == Effect::Write);
        assert_eq!(
            written,
            matches!(class, ReadWrite | WriteOnly),
            "word {w:o} ({class:?}): a write's effect listed as the table says"
        );
    }
    for &(w, effect, file, test) in EFFECTS {
        let class = t[w as usize].0;
        if effect == Effect::Read {
            assert!(
                matches!(class, ReadOnly | ReadWrite),
                "word {w:o}: a read effect on {class:?}"
            );
        }
        let path = format!("{}/tests/{file}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(text.contains(&format!("fn {test}(")), "word {w:o}: no test {test} in {file}");
    }
}

/// Where a program built by [`program`] stops: a jump to itself.
const STOP: u16 = 0o500;

/// A program that reads `words`' physical addresses in turn, each into A
/// 200 up, and after each reads word 101 into A 240 up and writes it,
/// clearing it; then stops at [`STOP`]. Virtual page `k + 1` maps the
/// `k`th word's page, and M `k + 1` holds its virtual address; virtual page
/// 30 and M 30 word 101.
fn program(m: &mut Machine, words: &[u32]) {
    assert!(words.len() <= 24 && 8 * words.len() < STOP as usize);
    let mut prom = Vec::new();
    for k in 0..words.len() as u64 {
        prom.push(Insn::new(ALU | SETM | m_src(1 + k) | START_READ));
        prom.push(filler());
        prom.push(Insn::new(ALU | SETM | SRC_MD | a_dest(0o200 + k)));
        prom.push(Insn::new(ALU | SETM | m_src(30) | START_READ));
        prom.push(filler());
        prom.push(Insn::new(ALU | SETM | SRC_MD | a_dest(0o240 + k)));
        prom.push(Insn::new(ALU | SETM | m_src(30) | START_WRITE));
        prom.push(filler());
    }
    let mut code = vec![filler(); 1024];
    code[..prom.len()].copy_from_slice(&prom);
    code[STOP as usize] = Insn::new(JUMP | target(STOP as u64) | ALWAYS | N);
    code[prom.len()] = Insn::new(JUMP | target(STOP as u64) | ALWAYS | N);
    m.load_prom(&code);
    support::prom_program_in_ram(m);
    for (k, &p) in words.iter().enumerate() {
        m.mmem[1 + k] = support::quux_map(m, 1 + k as u32, p).into();
    }
    m.mmem[30] = support::quux_map(m, 30, ERRORS).into();
    for k in 0..words.len() {
        m.amem[0o200 + k] = 0o525252;
        m.amem[0o240 + k] = 0o525252;
    }
}

/// Runs `m` on `micro` and on `rtl` to [`STOP`]: each engine's machine,
/// and `rtl`'s time.
fn on_both(m: Machine) -> [(&'static str, Machine, u64); 2] {
    fn run<E: Engine>(mut e: E, ns: impl Fn(&E) -> u64) -> (Machine, u64) {
        e.boot();
        for _ in 0..20_000 {
            if e.machine().opc == STOP {
                return (e.machine().clone(), ns(&e));
            }
            e.step().unwrap();
        }
        panic!("the program never reached its end");
    }
    let (e, _) = run(Micro::new(m.clone()), |_| 0);
    let (r, ns) = run(Rtl::new(m), Rtl::ns);
    [("micro", e, 0), ("rtl", r, ns)]
}

/// **Both engines read every word of the page through the map**:
/// what each reads is the table's power-on value, and none of the 256
/// reads sets word 101.
#[test]
fn both_engines_read_every_word_through_the_map() {
    let t = table();
    for batch in (0..256u32).collect::<Vec<_>>().chunks(16) {
        let words: Vec<u32> = batch.iter().map(|w| PAGE + w).collect();
        let mut m = quux();
        program(&mut m, &words);
        for (name, m, _) in on_both(m) {
            for (k, &w) in batch.iter().enumerate() {
                let (got, errors) = (m.amem[0o200 + k], m.amem[0o240 + k]);
                let want = t[w as usize].1;
                if want != POWER_ON_103 {
                    assert_eq!(got, want.into(), "{name}: word {w:o}");
                }
                assert_eq!(errors, 0, "{name}: word {w:o} set word 101");
            }
        }
    }
}

// --- nothing there ------------------------------------------------------------

/// The CADR's addresses QUUX's devices once had: the page `17377000`-
/// `17377377`, and the device registers after it, the display's at
/// `17377760` and the disk's at `17377774`; and the first and the last
/// words of the CADR's Unibus window below its last page. On a machine of
/// 2MW, as [`quux`]'s, they are past main memory.
fn old_addresses() -> Vec<u32> {
    (0o17377000..=0o17377777).chain([0o17400000, 0o17777377]).collect()
}

/// **Every old address is nothing there** (contract Q13, section 1; G2
/// §4.1): a read
/// gives 0 and sets word 101 `<0>`, the Xbus NXM bit and no other; a write
/// of all ones sets it too and changes nothing else in the machine.
#[test]
fn the_old_addresses_answer_nothing() {
    let mut m = quux();
    for a in old_addresses() {
        m.bus_error = 0;
        assert_eq!(m.bus_read(a), 0, "{a:o} read");
        assert_eq!(m.bus_error, bus_error::XBUS_NXM, "{a:o}: a read sets <0>");
        m.bus_error = 0;
        let s = state(&m);
        m.bus_write(a, 0xffff_ffff);
        assert_eq!(m.bus_error, bus_error::XBUS_NXM, "{a:o}: a write sets <0>");
        m.bus_error = 0;
        assert!(state(&m) == s, "{a:o}: a write changed the machine");
    }
}

/// **On both engines too, each at once**: every old address read through
/// the map gives 0 and word 101 `<0>`, which the program reads after each
/// and clears; and `rtl` takes no timeout over them, a batch of 24
/// failing reads and 48 accesses of word 101 ending well inside one
/// CADR timeout of 4.25 us each.
#[test]
fn both_engines_find_nothing_at_the_old_addresses() {
    for batch in old_addresses().chunks(24) {
        let mut m = quux();
        program(&mut m, batch);
        for (name, m, ns) in on_both(m) {
            for (k, &a) in batch.iter().enumerate() {
                assert_eq!(m.amem[0o200 + k], 0, "{name}: {a:o} reads 0");
                assert_eq!(
                    m.amem[0o240 + k],
                    (bus_error::XBUS_NXM as u32).into(),
                    "{name}: {a:o}: 101"
                );
            }
            assert!(ns < 4_250 * batch.len() as u64, "{name}: {ns} ns, a timeout");
        }
    }
}

/// **The CADR has no such page**: the same words of its last page,
/// `17777400`, are in its Unibus window, where nothing answers there, and
/// a read of word 100 sets the Unibus NXM bit, not the Xbus one.
#[test]
fn the_cadr_has_no_register_page() {
    let mut m = Machine::new();
    for w in [0, 0o100, 0o200, 0o210, 0o377] {
        m.bus_error = 0;
        assert_eq!(m.bus_read(0o17777400 + w), 0, "word {w:o}");
        assert_eq!(m.bus_error, bus_error::UNIBUS_NXM, "word {w:o}: the Unibus NXM");
    }
}

/// **The microcode reaches the page through the bus on both engines**: a
/// store of 1 to word 102 sets error stop, and a read of it gives 1 back.
/// Virtual page 1 maps the register page.
#[test]
fn both_engines_write_and_read_the_page() {
    let mut prom =
        vec![Insn::new(ALU | SETM | m_src(2) | MD), Insn::new(ALU | SETM | m_src(1) | START_WRITE)];
    prom.extend([filler(); 12]);
    prom.push(Insn::new(ALU | SETM | m_src(1) | START_READ));
    prom.extend([filler(); 12]);
    prom.push(Insn::new(ALU | SETM | SRC_MD | a_dest(0o200)));
    let machine = || {
        let mut m = quux();
        let mut words = vec![filler(); 1024];
        words[..prom.len()].copy_from_slice(&prom);
        m.load_prom(&words);
        m.mmem[1] = support::quux_map(&mut m, 1, MODE).into();
        m.mmem[2] = 1;
        m
    };
    let mut e = Micro::new(machine());
    e.boot();
    let mut r = Rtl::new(machine());
    r.boot();
    for _ in 0..400 {
        e.step().unwrap();
        r.step().unwrap();
    }
    for (name, m) in [("micro", e.machine()), ("rtl", r.machine())] {
        assert!(m.mode.errstop, "{name}: error stop set");
        assert_eq!(m.amem[0o200], 1, "{name}: word 102 read back");
        assert_eq!(m.bus_error & bus_error::XBUS_NXM, 0, "{name}: no timeout");
    }
}
