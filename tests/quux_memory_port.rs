// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's memory port (contract Q6). Main memory is off the Xbus: the
//! processor reaches it through its own port, through the cache --- always
//! fitted, 4K words in lines of 8 (contract G2 §3), 2-way, a hit in 20 ns,
//! the write buffer --- to main memory at one nominal timing, a write in
//! 290 ns and a line fill in 380 ns for two 64-bit beats and a tick more
//! for each of the three more a line of 40 bytes takes, 410 ns. Device
//! registers are never cached; an address nothing answers, past main
//! memory's end, fails at once with the NXM bit
//! (`tests/quux_device_registers.rs`). QUUX has no bus interface; the CADR
//! keeps its own. That port, and its rule that a write carries the `MD`
//! of the microcycle after its start, are revisions 13's and 14's, on the
//! single-edge `rtl`; revision 16 halts on an `MD` load there, and its own
//! port is `tests/revision_15_rtl.rs`'s. A read past main memory's end is
//! held on revision 16 too, on `micro` and the pipeline.

use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{
    ALU, ALWAYS, JUMP, MD, N, SETA, SETM, SRC_MD, START_READ, START_WRITE, a_dest, a_src, filler,
    m_src, target,
};
use muir::machine::{Geometry, Machine, bus_error};
use muir::micro::Micro;
use muir::pipeline::Pipeline;
use muir::rtl::Rtl;

mod support;

/// A program that reads the physical words `reads` in turn into A 200 up,
/// then stops on a jump to itself at [`STOP`]. Virtual page `k + 1` maps
/// onto each word's physical page; M `k + 1` holds its virtual address.
fn reading(geometry: Geometry, reads: &[u32]) -> Machine {
    let mut prom = Vec::new();
    for k in 0..reads.len() as u64 {
        prom.push(Insn::new(ALU | SETM | m_src(1 + k) | START_READ));
        // The microinstruction after a start may not read `MD`.
        prom.push(filler());
        prom.push(Insn::new(ALU | SETM | SRC_MD | a_dest(0o200 + k)));
    }
    prom.push(Insn::new(JUMP | target(prom.len() as u64) | ALWAYS | N));
    machine(geometry, &prom, reads)
}

fn machine(geometry: Geometry, prom: &[Insn], addresses: &[u32]) -> Machine {
    let mut m = if geometry == Geometry::QUUX {
        support::quux_16()
    } else {
        let mut m = Machine::new();
        m.geometry = geometry;
        m
    };
    let mut words = vec![filler(); 1024];
    words[..prom.len()].copy_from_slice(prom);
    m.load_prom(&words);
    support::prom_program_in_ram(&mut m);
    for (k, &p) in addresses.iter().enumerate() {
        m.mmem[1 + k] = if geometry.quux() {
            support::quux_va(p).into()
        } else if geometry.wide() {
            support::quux_map(&mut m, 1 + k as u32, p).into()
        } else {
            m.l2_map[1 + k] = (1 << 23) | (1 << 22) | (p >> 8);
            u64::from(((1 + k as u32) << 8) | (p & 0xff))
        };
    }
    for (k, w) in m.main[..0o4000].iter_mut().enumerate() {
        *w = u64::from(0o1000000 + k as u32);
    }
    m
}

/// Boots `e` and runs it well past the program's last jump, its writes
/// landed.
fn run<E: Engine>(e: &mut E) {
    e.boot();
    for _ in 0..4000 {
        e.step().unwrap();
    }
    e.settle_for_checkpoint().unwrap();
}

/// **Past main memory's end is nothing, and nothing is cached there**: a
/// write and a read-back at the first word past it both fail with the NXM bit,
/// and the read gives 0, not the word written --- what the microcode's
/// memory-size probe (`MEM-SIZE-LOOP`, `uc-cold-disk.lisp`) relies on. On
/// both engines, revision 16.
#[test]
fn past_main_memory_s_end_nothing_reads_back() {
    let end = muir::machine::MAIN_WORDS as u32;
    let prom = [
        Insn::new(ALU | SETA | a_src(0o100) | MD),
        Insn::new(ALU | SETM | m_src(1) | START_WRITE),
        filler(),
        filler(),
        Insn::new(ALU | SETM | m_src(1) | START_READ),
        filler(),
        Insn::new(ALU | SETM | SRC_MD | a_dest(0o200)),
        Insn::new(JUMP | target(7) | ALWAYS | N),
    ];
    let setup = || {
        let mut m = machine(Geometry::QUUX, &prom, &[end]);
        m.amem[0o100] = 0o37;
        m.amem[0o200] = 0o525252;
        m
    };
    let mut r = Pipeline::new(setup());
    run(&mut r);
    let mut e = Micro::new(setup());
    run(&mut e);
    for (name, m) in [("rtl", r.machine()), ("micro", e.machine())] {
        assert_eq!(m.amem[0o200], 0, "{name}: the write did not read back");
        assert_ne!(m.bus_error & bus_error::XBUS_NXM, 0, "{name}: the NXM bit");
        assert_eq!(m.bus_error & bus_error::UNIBUS_NXM, 0, "{name}");
    }
}

/// **On the CADR the same accessors are the bus interface's**: at every
/// step, `bus_ack_at` and `bus_granted` are [`muir::busint::Busint`]'s own.
#[test]
fn rtl_shows_the_bus_interface_s_acknowledgement_and_grant() {
    let mut e = Rtl::new(reading(Geometry::CADR, &[0o1000, 0o1001]));
    e.boot();
    let mut seen = 0;
    for _ in 0..4000 {
        e.step().unwrap();
        let b = e.busint().unwrap();
        assert_eq!(e.bus_ack_at(), b.ack_at());
        assert_eq!(e.bus_granted(), b.granted());
        seen += e.bus_granted() as u32;
    }
    assert!(seen > 0, "a granted cycle was seen");
}
