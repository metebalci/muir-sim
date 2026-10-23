// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX without the Unibus (contract Q5). The CADR's Unibus window, its
//! physical pages 37000 up, is no window on QUUX: those are main memory's
//! bus addresses when there is that much of it (contract G3 revision 14,
//! A14.1), and on a machine of 2MW, as here, past its end, where a read
//! or a write fails as any empty address does, the Xbus NXM bit set, and
//! changes nothing. The I/O board's registers, the bus interface's, the
//! Unibus map's and the diagnostic registers are all there; the Unibus
//! interrupt does not reach QUUX's processor; and the debug cable, a Unibus
//! master, is refused. QUUX's own devices are on the register page at
//! `35777777400` (contracts Q2-Q4, Q13). The CADR keeps all of it.

use muir::busint::{interrupt_status, unibus_physical};
use muir::machine::{Geometry, Machine, bus_error};

mod support;

fn quux() -> Machine {
    support::quux_16()
}

/// The machine's whole state, as a checkpoint has it.
fn state(m: &Machine) -> Vec<u8> {
    let mut w = muir::checkpoint::Writer::new();
    m.save(&mut w);
    w.finish()
}

/// The Unibus registers QUUX's software once used, and the rest of the
/// window's kinds: the keyboard and mouse, the I/O board's status, the
/// microsecond clock, the Chaosnet interface, the serial port, the
/// diagnostic mode register, the interrupt control and error status, and
/// the Unibus map.
const WINDOW: [u32; 12] = [
    0o764100, 0o764104, 0o764112, 0o764120, 0o764140, 0o764142, 0o764144, 0o764160, 0o766012,
    0o766040, 0o766044, 0o766140,
];

/// **Every Unibus address is nothing on QUUX**: a read times out with the
/// Xbus NXM bit, not the Unibus one, and reads 0; a write times out and
/// changes nothing --- the mode register and the interrupt control as they
/// were, and the whole machine but the error bits.
#[test]
fn every_unibus_address_is_nothing_on_quux() {
    let mut m = quux();
    let before = state(&m);
    for u in WINDOW {
        let p = unibus_physical(u);
        m.bus_error = 0;
        assert_eq!(m.bus_read(p), 0, "{u:o} read");
        assert_eq!(m.bus_error, bus_error::XBUS_NXM, "{u:o}: read times out as an Xbus address");
        m.bus_error = 0;
        m.bus_write(p, 0o177777);
        assert_eq!(m.bus_error, bus_error::XBUS_NXM, "{u:o}: write times out");
    }
    assert!(!m.mode.errstop && !m.mode.prom_disable, "766012 wrote nothing");
    assert_eq!(m.interrupt_status & interrupt_status::ENABLE_UB_INTS, 0, "766040 wrote nothing");
    m.bus_error = 0;
    assert!(state(&m) == before, "the writes changed the machine");
}

/// **The device window's last page is the register page, and the CADR's
/// last page is not**: word 0 at `35777777400` is the MACHINE-ID with no
/// NXM; the word below it, past the frame buffer's end, fails, and so does
/// `17777400`, the CADR's last page, past the end of 2MW.
#[test]
fn the_space_s_last_page_is_the_register_page() {
    let mut m = quux();
    let page = support::PAGE_16;
    assert_eq!(m.bus_read(page), Geometry::QUUX_16.machine_id.unwrap().into());
    assert_eq!(m.bus_error, 0, "the page answers");
    for nothing in [page - 1, 0o17777400] {
        m.bus_error = 0;
        assert_eq!(m.bus_read(nothing), 0, "{nothing:o}");
        assert_eq!(m.bus_error, bus_error::XBUS_NXM, "{nothing:o} is nothing there");
    }
}

/// **The CADR keeps its Unibus**: the same reads answer, with no timeout.
#[test]
fn the_cadr_keeps_its_unibus() {
    let mut m = Machine::new();
    for u in [0o764120, 0o764140, 0o766040, 0o766044] {
        m.bus_error = 0;
        m.bus_read(unibus_physical(u));
        assert_eq!(m.bus_error, 0, "{u:o} answers on the CADR");
    }
}

/// **The Unibus interrupt does not reach QUUX's processor**: an I/O board
/// request under an interrupt control that asks for it --- set directly,
/// as no write reaches them on QUUX --- interrupts the CADR and not QUUX.
#[test]
fn the_unibus_interrupt_does_not_reach_quux() {
    use muir::ioboard::{self, csr};
    for (geometry, want) in [(Geometry::CADR, true), (Geometry::QUUX_16, false)] {
        let mut m = Machine::with_geometry(geometry, 32);
        m.interrupt_status |= interrupt_status::ENABLE_UB_INTS;
        m.ioboard.write(ioboard::CSR, csr::CLOCK_INT_ENABLE, 0);
        m.ioboard.write(ioboard::CLOCK, 1, 0);
        m.ns = 1_000_000;
        assert_eq!(m.interrupt(), want, "{geometry:?}");
    }
}

/// **Both engines' bus times out there too**: a program that reads the
/// microsecond clock's old Unibus address and the Chaosnet CSR's gets 0
/// and the Xbus NXM bit on `micro` and `rtl` alike.
#[test]
fn both_engines_time_out_on_the_unibus_window() {
    use muir::engine::Engine;
    use muir::isa::Insn;
    use muir::isa::asm::{ALU, SETM, SRC_MD, START_READ, a_dest, filler, m_src};
    use muir::micro::Micro;
    use muir::pipeline::Pipeline;
    let mut prom = Vec::new();
    for k in 0..2u64 {
        prom.push(Insn::new(ALU | SETM | m_src(1 + k) | START_READ));
        prom.extend([filler(); 12]);
        prom.push(Insn::new(ALU | SETM | SRC_MD | a_dest(0o200 + k)));
    }
    let machine = || {
        let mut m = quux();
        let mut words = vec![filler(); 1024];
        words[..prom.len()].copy_from_slice(&prom);
        m.load_prom(&words);
        support::prom_program_in_ram(&mut m);
        // The two addresses through the physical memory window.
        for (k, u) in [0o764120u32, 0o764140].into_iter().enumerate() {
            m.mmem[1 + k] = u64::from(support::quux_va(unibus_physical(u)));
        }
        m.amem[0o200] = 0o525252;
        m.amem[0o201] = 0o525252;
        m
    };
    let mut e = Micro::new(machine());
    e.boot();
    let mut r = Pipeline::new(machine());
    r.boot();
    for _ in 0..400 {
        e.step().unwrap();
        r.step().unwrap();
    }
    for (name, m) in [("micro", e.machine()), ("rtl", r.machine())] {
        assert_eq!([m.amem[0o200], m.amem[0o201]], [0, 0], "{name}: nothing answered");
        assert_eq!(m.bus_error & bus_error::UNIBUS_NXM, 0, "{name}: not a Unibus timeout");
        assert_ne!(m.bus_error & bus_error::XBUS_NXM, 0, "{name}: an Xbus timeout");
    }
}
