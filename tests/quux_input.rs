// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's keyboard and mouse (contract Q3), on the register page at
//! `1777777400`: word 120 the keyboard's status, 121 its data, 122 the mouse,
//! 123 the mouse's status. No CADR keyboard timing and no quadrature: a
//! FIFO of 64 of the key words the CADR's keyboard gives, and the CADR's
//! twelve-bit mouse counts with the host's motion added to them.

use muir::machine::{Geometry, Machine, bus_error};
use muir::quux_input::KeyboardMouse;

const PAGE: u32 = muir::machine::REGISTER_PAGE_13;
const INTERRUPTS: u32 = PAGE + 0o100;
const KBD_STATUS: u32 = PAGE + 0o120;
const KBD_DATA: u32 = PAGE + 0o121;
const MOUSE: u32 = PAGE + 0o122;
const MOUSE_STATUS: u32 = PAGE + 0o123;
const ENABLE: u32 = 1 << 8;

fn quux() -> Machine {
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
    m
}

/// **Key words come out of word 121 in the order they went in**, each the
/// word itself in `<23:0>`, `<31:24>` reading 0 (contract Q13); word 120's
/// `<0>` says one is waiting, and an empty FIFO reads 0 there and in 121.
#[test]
fn keys_come_out_in_order() {
    let mut m = quux();
    assert_eq!((m.bus_read(KBD_STATUS) & 1, m.bus_read(KBD_DATA)), (0, 0), "empty");
    for w in [0o101, 0o1234567, 0o15, 0xff12_3456] {
        m.quux_input.press(w);
    }
    assert_eq!(m.bus_read(KBD_STATUS) & 1, 1);
    assert!(m.quux_input.key_waiting(), "as the host sees it too");
    let got: Vec<u32> = (0..4).map(|_| support::low(m.bus_read(KBD_DATA))).collect();
    assert!(!m.quux_input.key_waiting());
    assert_eq!(got, [0o101, 0o1234567, 0o15, 0x12_3456]);
    assert_eq!(m.bus_read(KBD_STATUS) & 1, 0, "taken");
    assert_eq!(m.bus_error & bus_error::XBUS_NXM, 0);
}

/// **It holds 64**; the 65th is dropped and word 120's `<1>` says so, until
/// a write of 120.
#[test]
fn it_holds_64_and_says_when_it_overflowed() {
    let mut m = quux();
    for k in 0..65 {
        m.quux_input.press(k);
    }
    assert_eq!(m.bus_read(KBD_STATUS) & 3, 3, "waiting, overflowed");
    let got: Vec<u32> = (0..64).map(|_| support::low(m.bus_read(KBD_DATA))).collect();
    assert_eq!(got, (0..64).collect::<Vec<u32>>(), "the first 64");
    assert_eq!(m.bus_read(KBD_STATUS) & 3, 2, "empty, still overflowed");
    m.bus_write(KBD_STATUS, 0);
    assert_eq!(m.bus_read(KBD_STATUS) & 3, 0, "cleared by a write");
}

/// **The mouse counts wrap at 4096, and the buttons sit in `<14:12>`**:
/// X in `<11:0>`, Y in `<27:16>`, the host's motion added, as the CADR's
/// counters count. Word 123's `<0>` is set by a move or a button change and
/// cleared by a read of 122.
#[test]
fn the_mouse_counts_and_its_buttons() {
    let mut m = quux();
    assert_eq!(m.bus_read(MOUSE_STATUS) & 1, 0);
    m.quux_input.mouse_move(5, -3);
    assert_eq!(m.bus_read(MOUSE_STATUS) & 1, 1, "moved");
    assert_eq!(m.bus_read(MOUSE), 4093 << 16 | 5);
    assert_eq!(m.bus_read(MOUSE_STATUS) & 1, 0, "read");
    m.quux_input.mouse_move(-4096 - 6, 3);
    assert_eq!(m.bus_read(MOUSE), 4095, "both wrapped: Y 0, X 4095");
    m.quux_input.mouse_buttons(0o5);
    assert_eq!(m.bus_read(MOUSE_STATUS) & 1, 1, "a button");
    assert_eq!(m.bus_read(MOUSE) >> 12 & 7, 0o5);
    assert_eq!(m.quux_input.mouse_buttons_held(), 0o5);
}

/// **Each interrupts under its own enable, in word 100**: `<4>` the
/// keyboard, a word waiting under 120's `<8>`; `<5>` the mouse, a change
/// under 123's `<8>`. The processor's interrupt pending follows.
#[test]
fn each_interrupts_under_its_enable() {
    let mut m = quux();
    m.quux_input.press(0o101);
    m.quux_input.mouse_move(1, 0);
    assert_eq!(m.bus_read(INTERRUPTS) & 0o60, 0, "neither enabled");
    assert!(!m.interrupt());
    m.bus_write(KBD_STATUS, ENABLE.into());
    assert_eq!(m.bus_read(INTERRUPTS) & 0o60, 0o20, "the keyboard");
    assert!(m.interrupt());
    m.bus_write(MOUSE_STATUS, ENABLE.into());
    assert_eq!(m.bus_read(INTERRUPTS) & 0o60, 0o60, "and the mouse");
    m.bus_read(KBD_DATA);
    m.bus_read(MOUSE);
    assert_eq!(m.bus_read(INTERRUPTS) & 0o60, 0, "both taken");
    assert!(!m.interrupt());
    assert_eq!(m.bus_read(KBD_STATUS) & u64::from(ENABLE), ENABLE.into(), "the enable reads back");
}

/// **The terminal's keyboard and mouse deliver into it** on QUUX, as they
/// deliver to the I/O board on the CADR: what is typed is in the FIFO, a
/// word each, and the mouse's counts are added.
#[test]
fn the_terminal_delivers_into_it() {
    use muir::terminal::keyboard::{Keyboard, keysym};
    use muir::terminal::mouse::Mouse;
    let mut m = quux();
    let mut k = Keyboard::new();
    k.key(keysym::RETURN, true);
    k.key(keysym::RETURN, false);
    let typed = k.pending();
    assert!(typed > 0);
    while k.pending() > 0 {
        assert!(k.deliver(&mut m.quux_input), "the FIFO takes it");
    }
    let mut n = 0;
    while m.bus_read(KBD_STATUS) & 1 != 0 {
        m.bus_read(KBD_DATA);
        n += 1;
    }
    assert_eq!(n, typed, "a word each");
    let mut mouse = Mouse::new();
    mouse.pointer(0o1, 10, 20);
    mouse.pointer(0o1, 13, 16);
    mouse.deliver(&mut m.quux_input);
    let w = m.bus_read(MOUSE);
    assert_eq!(w >> 12 & 7, 0o1, "the button");
    assert_ne!(w & 0o7777, 0, "moved");
}

/// **The keyboard's boot word boots QUUX**, as it boots the CADR through
/// the I/O board: it is taken once.
#[test]
fn the_boot_word_boots() {
    let mut m = quux();
    // `<13:6>` 360, as `ioboard::boot_word` decodes it; bit 16 the keyboard's.
    let boot = 1 << 16 | 0o360 << 6;
    assert!(muir::ioboard::boot_word(boot));
    m.quux_input.press(boot);
    assert!(m.quux_input.take_boot());
    assert!(!m.quux_input.take_boot());
}

/// **A checkpoint keeps the FIFO, the counts and the enables.**
#[test]
fn a_checkpoint_keeps_it() {
    use muir::checkpoint::{Reader, Writer};
    let mut m = quux();
    m.quux_input.press(0o101);
    m.quux_input.press(0o102);
    m.quux_input.mouse_move(7, 9);
    m.bus_write(KBD_STATUS, ENABLE.into());
    let mut w = Writer::new();
    m.quux_input.save(&mut w);
    let body = w.finish();
    let mut back = quux();
    back.quux_input.load(&mut Reader::new(&body)).unwrap();
    for addr in [KBD_STATUS, MOUSE_STATUS, MOUSE, KBD_DATA, KBD_DATA, KBD_STATUS] {
        assert_eq!(back.bus_read(addr), m.bus_read(addr), "{addr:o}");
    }
}

/// **The CADR has none of it**: words 120-123 time out, as the page does,
/// in the CADR's Unibus window.
#[test]
fn the_cadr_has_none_of_it() {
    let mut m = Machine::new();
    m.bus_read(KBD_DATA);
    assert_eq!(m.bus_error, bus_error::UNIBUS_NXM);
}

mod support;

/// **The microcode takes key words through the bus on both engines**: two
/// reads of word 121 give the two words typed, in order. Virtual page 1
/// maps the register page.
#[test]
fn both_engines_read_the_fifo() {
    use muir::engine::Engine;
    use muir::isa::Insn;
    use muir::isa::asm::{ALU, SETM, SRC_MD, START_READ, a_dest, filler, m_src};
    use muir::micro::Micro;
    use muir::rtl::Rtl;
    let mut prom = Vec::new();
    for k in 0..2 {
        prom.push(Insn::new(ALU | SETM | m_src(1) | START_READ));
        prom.extend([filler(); 12]);
        prom.push(Insn::new(ALU | SETM | SRC_MD | a_dest(0o200 + k)));
    }
    let machine = || {
        let mut m = quux();
        let mut words = vec![filler(); 1024];
        words[..prom.len()].copy_from_slice(&prom);
        m.load_prom(&words);
        support::prom_program_in_ram(&mut m);
        m.mmem[1] = support::quux_map(&mut m, 1, PAGE | 0o121).into();
        m.quux_input.press(0o101);
        m.quux_input.press(0o102);
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
        assert_eq!([m.amem[0o200], m.amem[0o201]], [0o101, 0o102], "{name}");
    }
}
