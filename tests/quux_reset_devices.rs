// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's reset devices (contract Q11, revision 10): word 104 `<0>` of the
//! register page. A write with `<0>` set resets every device, at the
//! instant the write is taken --- the interval timers to their reset state,
//! the file device, block-disk and the network as the CADR's
//! `PROG.UNIBUS.RESET` resets them, the video controller with nothing to show for it,
//! and the keyboard and mouse not at all; a write with `<0>` clear does
//! nothing; the word reads 0. On QUUX `INTERRUPT-CONTROL<28>`,
//! `PROG.UNIBUS.RESET`, drives nothing.

use muir::block_disk::{BLOCK_NS, BlockDisk};
use muir::disk_unit::Geometry as Pack;
use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{
    ALU, ALWAYS, JUMP, MD, N, SETM, SETO, START_WRITE, filler, m_dest, m_src, src, target,
};
use muir::machine::{Geometry, IntervalTimer, Machine};
use muir::micro::Micro;
use muir::quux_input::KeyboardMouse;
use muir::rtl::Rtl;
use muir::tv::Board;

mod support;

const PAGE: u32 = muir::machine::REGISTER_PAGE_13;
const INTERRUPTS: u32 = PAGE + 0o100;
const RESET_DEVICES: u32 = PAGE + 0o104;
const CHAOS_CSR: u32 = PAGE + 0o140;
const KBD_STATUS: u32 = PAGE + 0o120;
const KBD_DATA: u32 = PAGE + 0o121;
const MOUSE_STATUS: u32 = PAGE + 0o123;
const FDEV_CONTROL: u32 = PAGE + 0o160;
const FDEV_STATUS: u32 = PAGE + 0o161;
const FDEV_CMD_PROD: u32 = PAGE + 0o164;
/// Block-disk's command, command list pointer and START, words 200, 201
/// and 203 (contract Q13).
const DISK_COMMAND: u32 = PAGE + 0o200;
const DISK_CLP: u32 = PAGE + 0o201;
const DISK_START: u32 = PAGE + 0o203;

const fn control(k: usize) -> u32 {
    PAGE + 0o110 + 2 * k as u32
}
const fn period(k: usize) -> u32 {
    PAGE + 0o111 + 2 * k as u32
}

/// The file device's rings, and the response ring's first slot.
const CMD_RING: u32 = 0o100000;
const RESP_RING: u32 = 0o110000;

fn state(m: &Machine) -> Vec<u8> {
    let mut w = muir::checkpoint::Writer::new();
    m.save(&mut w);
    w.finish()
}

/// A QUUX machine with every device busy at `ns`: timers 0 to 2 on under
/// their interrupt enables, timer 1 up; block-disk in the middle of a
/// transfer under its interrupt enable; the Chaosnet interface's
/// interrupt enables and loop back set; the file device enabled with a
/// command queued whose due time is a millisecond on; the video controller; and the
/// keyboard's FIFO holding two key words and the mouse moved, both under
/// their interrupt enables.
fn busy() -> Machine {
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
    make_busy(&mut m);
    m
}

fn make_busy(m: &mut Machine) {
    m.tv.set_board(Board::Video);
    let mut d = BlockDisk::new(BLOCK_NS);
    d.attach(muir::disk_image::Disk::blank(Pack::T300.blocks()));
    m.block_disk = Some(d);
    m.ns = 1_000_000;
    for (k, p, v) in [(0, 16_667, 0o401), (1, 10, 0o401), (2, 500, 0o405)] {
        m.bus_write(period(k), p);
        m.bus_write(control(k), v);
    }
    // Block-disk: four blocks from word 100's list, 400 us, the done
    // interrupt on.
    m.main[0o100..0o104].copy_from_slice(&[0o10 << 8 | 1, 0o11 << 8 | 1, 0o12 << 8 | 1, 0o13 << 8]);
    m.bus_write(DISK_CLP, 0o100);
    m.bus_write(DISK_COMMAND, 1 << 11);
    m.bus_write(DISK_START, 0);
    // The Chaosnet interface: receive and transmit interrupt enables, loop
    // back.
    m.bus_write(CHAOS_CSR, 0o62);
    // The keyboard and the mouse.
    m.quux_input.press(0o1234);
    m.quux_input.press(0o4321);
    m.quux_input.mouse_move(5, -3);
    m.bus_write(KBD_STATUS, 1 << 8);
    m.bus_write(MOUSE_STATUS, 1 << 8);
    m.ns += 100_000;
    // The file device: rings, enabled under its interrupt enable, an OPEN
    // queued, due some 20 us on.
    m.bus_write(PAGE + 0o162, CMD_RING.into());
    m.bus_write(PAGE + 0o163, 2);
    m.bus_write(PAGE + 0o166, RESP_RING.into());
    m.bus_write(PAGE + 0o167, 2);
    m.bus_write(FDEV_CONTROL, 0x101);
    m.main[CMD_RING as usize..CMD_RING as usize + 8].copy_from_slice(&[
        (0o101 | muir::file_device::op::OPEN << 16).into(),
        0,
        0o120000,
        1,
        0,
        0,
        0,
        0,
    ]);
    m.main[0o120000] = u64::from(b'/' as u32);
    m.bus_write(FDEV_CMD_PROD, 1);
    // What is up: timer 1, the keyboard and the mouse; block-disk is not
    // yet done.
    let up = m.bus_read(INTERRUPTS);
    assert_eq!(up & 0o3, 0o2, "timer 1 up, word 100 {up:o}");
    assert_eq!(up & 0o10, 0, "block-disk in flight");
    assert_ne!(m.bus_read(FDEV_STATUS) & 1, 0, "the file device enabled");
    assert_eq!(up & 0o60, 0o60, "the keyboard and the mouse");
}

/// **M5, word 104 reads 0**, before a write and after one of 0 and of 1.
#[test]
fn m5_word_104_reads_0() {
    let mut m = busy();
    assert_eq!(m.bus_read(RESET_DEVICES), 0);
    m.bus_write(RESET_DEVICES, 0);
    assert_eq!(m.bus_read(RESET_DEVICES), 0);
    m.bus_write(RESET_DEVICES, 1);
    assert_eq!(m.bus_read(RESET_DEVICES), 0);
    m.bus_write(RESET_DEVICES, !0);
    assert_eq!(m.bus_read(RESET_DEVICES), 0);
}

/// **M5, a write of 0 does nothing**, nor one of every bit but `<0>`: the
/// whole machine's state is what it was.
#[test]
fn m5_a_write_with_bit_0_clear_changes_nothing() {
    for v in [0u32, !1] {
        let mut m = busy();
        let before = state(&m);
        m.bus_write(RESET_DEVICES, v.into());
        assert!(state(&m) == before, "a write of {v:o} changed the machine");
    }
}

/// **M5, reset devices against revision 9's `<28>`**: from the same busy
/// machine, a write of word 104 with 1 leaves every device as revision 9's
/// `PROG.UNIBUS.RESET` left it, `Machine::bus_reset` --- block-disk, the
/// network, the video controller and the file device, the whole machine's state compared
/// --- and every timer in its reset state besides, which `<28>` never
/// reached. The keyboard's FIFO, the mouse's counts and both their
/// interrupt enables are kept: `bus_reset` never reached them either, and
/// they are what they were before the write.
#[test]
fn m5_reset_devices_does_what_revision_9_s_28_did_and_resets_the_timers() {
    let reference = {
        let mut m = busy();
        m.bus_reset();
        m
    };
    let mut m = busy();
    let input_before = {
        let mut w = muir::checkpoint::Writer::new();
        m.quux_input.save(&mut w);
        w.finish()
    };
    m.bus_write(RESET_DEVICES, 1);
    assert_eq!(m.timers.timer, [IntervalTimer::RESET; 3], "every timer reset");
    assert_ne!(reference.timers, m.timers, "revision 9's <28> reached no timer");
    let mut with_timers = reference.clone();
    with_timers.timers = m.timers;
    assert!(state(&with_timers) == state(&m), "every device as <28> left it");
    // Something was reset: the file device, block-disk, the network.
    assert_eq!(m.bus_read(FDEV_STATUS) & 1, 0, "the file device disabled");
    assert_eq!(m.bus_read(DISK_COMMAND) & (1 << 11), 0);
    assert_eq!(m.bus_read(CHAOS_CSR) & 0o62, 0, "the network's enables");
    // Kept.
    let mut w = muir::checkpoint::Writer::new();
    m.quux_input.save(&mut w);
    assert!(w.finish() == input_before, "the keyboard and the mouse kept");
    assert_eq!(m.bus_read(INTERRUPTS), 0o60, "the keyboard's and the mouse's alone");
    assert_eq!(m.bus_read(KBD_DATA), 0o1234, "the FIFO kept");
}

/// **M5, what the file device had due by the write's instant runs first**,
/// as for a write of 160, and what was due later never runs: the queued
/// OPEN's response is in the ring when the reset is written at its due
/// time, and not when it is written a nanosecond before it.
#[test]
fn m5_what_the_file_device_had_due_runs_first() {
    let due = {
        let mut m = busy();
        let mut at = m.ns;
        while m.main[RESP_RING as usize] == 0 {
            at += 1_000;
            m.ns = at;
            m.advance_file_device();
        }
        // The first microsecond it had answered by: the due time is in
        // (at - 1000, at].
        let (mut lo, mut hi) = (at - 1_000, at);
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            let mut m = busy();
            m.ns = mid;
            m.advance_file_device();
            if m.main[RESP_RING as usize] == 0 { lo = mid } else { hi = mid }
        }
        hi
    };
    for (at, answered) in [(due, true), (due - 1, false)] {
        let mut m = busy();
        m.ns = at;
        m.bus_write(RESET_DEVICES, 1);
        assert_eq!(
            m.main[RESP_RING as usize] != 0,
            answered,
            "reset at due {:+}",
            at as i64 - due as i64
        );
        m.ns = at + 1_000_000_000;
        m.advance_file_device();
        assert_eq!(m.main[RESP_RING as usize] != 0, answered, "and never after");
    }
}

/// The register page's word `w` through virtual page 1, the page being at
/// word 1400 of its 1024-word frame.
const fn va(w: u32) -> u64 {
    (1 << 10 | PAGE & 0o1777 | w) as u64
}

/// `INTERRUPT-CONTROL`'s `INT.ENABLE`, `<35>` on QUUX (contract G2
/// appendix A1.6), where the CADR has `<27>`.
const INT_ENABLE: u64 = 1 << 35;

const INTERRUPT_CONTROL: u64 = (2 << 19) | (0o37 << 14);
const DEST_3: u64 = (3 << 19) | (0o37 << 14);
const PGF_OR_INT: u64 = (1 << 5) | 5;

fn engine_machine(prom: &[Insn], m_words: &[(usize, u64)]) -> Machine {
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
    let mut words = vec![filler(); 1024];
    words[..prom.len()].copy_from_slice(prom);
    m.load_prom(&words);
    support::prom_program_in_ram(&mut m);
    support::quux_map(&mut m, 0, 0);
    support::quux_map(&mut m, 1, PAGE);
    for &(k, v) in m_words {
        m.mmem[k] = v;
    }
    m
}

/// **M4, a destination 3 write by a reset-devices write** (`rtl`): Q1's
/// turn-on, 1, written to destination 3 at the edge that takes a
/// reset-devices write, or a microcycle later, leaves timer 0 off:
/// destination 3 writes only M at revision 10.
#[test]
fn m4_a_destination_3_write_by_reset_devices_turns_nothing_on() {
    for gap in [0usize, 1] {
        let mut prom = vec![
            Insn::new(ALU | SETM | m_src(2) | MD),
            Insn::new(ALU | SETM | m_src(1) | START_WRITE),
        ];
        prom.extend(vec![filler(); gap]);
        let dest_3 = Insn::new(ALU | SETM | m_src(3) | DEST_3);
        prom.push(dest_3);
        let mut r = Rtl::new(engine_machine(&prom, &[(1, va(0o104)), (2, 1), (3, 1)]));
        r.boot();
        let mut edge = None;
        for _ in 0..40 {
            let ir = r.ir();
            let taken = r.bus_answered_at();
            r.step().unwrap();
            if ir == dest_3.raw() && edge.is_none() {
                edge = Some(r.ns());
                if gap == 0 {
                    assert_eq!(r.bus_answered_at(), Some(r.ns()), "the reset taken at the edge");
                } else {
                    assert!(taken.is_some_and(|t| t < r.ns()), "the reset taken before");
                }
            }
        }
        assert!(edge.is_some());
        let m = r.machine();
        assert_eq!(m.timers.timer[0], IntervalTimer::RESET, "destination 3 {gap} microcycles on");
        assert_eq!(m.mmem[0o37], 1, "destination 3 {gap} microcycles on wrote M 37");
    }
}

/// **M5, `SINTR` at the reset-devices write's edge** (`rtl`): a term the
/// reset takes down, timer 1's flag under its interrupt enable, is still
/// in the `SINTR` the edge that takes the write registers, and not in the
/// next edge's: the jump right after that edge is taken, one a microcycle
/// later is not. With the reset written as 0 both are taken.
#[test]
fn m5_sintr_at_the_reset_s_edge_still_has_what_it_resets() {
    for (reset, jump_after, taken) in [(1, 0usize, true), (1, 1, false), (0, 1, true)] {
        let mut prom = vec![
            Insn::new(ALU | SETM | m_src(6) | INTERRUPT_CONTROL),
            Insn::new(ALU | SETM | m_src(2) | MD),
            Insn::new(ALU | SETM | m_src(1) | START_WRITE),
            filler(),
        ];
        prom.extend(vec![filler(); jump_after]);
        let at = prom.len() as u64;
        prom.push(Insn::new(JUMP | target(at + 3) | PGF_OR_INT | N));
        prom.push(Insn::new(JUMP | target(at + 1) | ALWAYS | N));
        prom.push(filler());
        prom.push(Insn::new(ALU | SETO | m_dest(5)));
        prom.push(Insn::new(JUMP | target(at + 4) | ALWAYS | N));
        let mut r = Rtl::new(engine_machine(&prom, &[(1, va(0o104)), (2, reset), (6, INT_ENABLE)]));
        r.boot();
        r.machine_mut().timers.timer[1] = IntervalTimer {
            on: true,
            one_shot: false,
            interrupt_enable: true,
            period_us: 1000,
            deadline_ns: 0,
        };
        for _ in 0..40 {
            r.step().unwrap();
        }
        assert_eq!(
            r.machine().mmem[5] == Geometry::QUUX.word_mask(),
            taken,
            "reset {reset}, the jump {jump_after} microcycles after the write's edge"
        );
    }
}

/// **M5, on QUUX a `PROG.UNIBUS.RESET` rise resets nothing**, on both
/// engines: a program that raises and drops `INTERRUPT-CONTROL`'s
/// `PROG.UNIBUS.RESET`, `<36>` on QUUX where the CADR has `<28>` (contract G2
/// appendix A1.6), leaves the timers, the file device, block-disk and the
/// network as they were.
#[test]
fn m5_prog_unibus_reset_drives_nothing_on_quux() {
    // <36> up, `LOCATION-COUNTER` (source 13, which carries the flags)
    // into M 4, <36> down, and into M 5.
    let prom = [
        Insn::new(ALU | SETM | m_src(1) | INTERRUPT_CONTROL),
        Insn::new(ALU | SETM | src(0o13) | m_dest(4)),
        Insn::new(ALU | SETM | m_src(2) | INTERRUPT_CONTROL),
        Insn::new(ALU | SETM | src(0o13) | m_dest(5)),
        Insn::new(JUMP | target(4) | ALWAYS | N),
    ];
    let look = |m: &mut Machine| {
        [
            m.bus_read(FDEV_CONTROL),
            m.bus_read(FDEV_STATUS) & 0xff_00ff,
            m.bus_read(DISK_COMMAND),
            m.bus_read(CHAOS_CSR) & 0o62,
            m.bus_read(control(0)) & !2,
            m.bus_read(control(1)) & !2,
            m.bus_read(control(2)) & !2,
            m.bus_read(period(1)),
        ]
    };
    for engine in ["micro", "rtl"] {
        let m = engine_machine(&prom, &[(1, 1 << 36), (2, 0)]);
        let mut e: Box<dyn Engine> = match engine {
            "micro" => Box::new(Micro::new(m)),
            _ => Box::new(Rtl::new(m)),
        };
        e.boot();
        make_busy(e.machine_mut());
        let before = look(e.machine_mut());
        let timers = e.machine().timers;
        for _ in 0..20 {
            e.step().unwrap();
        }
        let m = e.machine();
        assert_eq!((m.mmem[4] >> 36 & 1, m.mmem[5] >> 36 & 1), (1, 0), "{engine}: <36> up, down");
        assert_eq!(e.machine().timers, timers, "{engine}: the timers");
        let after = look(e.machine_mut());
        assert_eq!(after, before, "{engine}: <36> reset something");
    }
}
