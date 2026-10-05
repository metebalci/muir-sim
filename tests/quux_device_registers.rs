// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's device registers (contracts Q7 and Q13). There is no device
//! bus: the processor's register decode reaches the device registers ---
//! the register page at `1777777400`, with
//! the feature page, the video controller's and block-disk's words on it
//! --- at their addresses, never cached, a register access taking one
//! microcycle more than a failed one. An address nothing answers fails at once, reads 0 and
//! sets word 101's NXM bit: no timeout. The frame buffer, in its window
//! at `1760000000` (contract G2 §4.1), is on the memory bus with main
//! memory, through the cache.

use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{
    ALU, ALWAYS, JUMP, MD, N, SETA, SETM, SRC_MD, START_READ, START_WRITE, a_dest, a_src, filler,
    m_src, target,
};
use muir::machine::{Geometry, Machine, bus_error};
use muir::micro::Micro;
use muir::rtl::Rtl;

mod support;

/// Where the program stops: a jump to itself.
const STOP: u16 = 0o77;

/// A program that reads each of `reads`' physical words into A 200 up, a
/// filler after each start, then stops at [`STOP`]; virtual page `k + 1`
/// maps each word's page, and M `k + 1` holds its virtual address.
fn reading(reads: &[u32]) -> Machine {
    let mut prom = Vec::new();
    for k in 0..reads.len() as u64 {
        prom.push(Insn::new(ALU | SETM | m_src(1 + k) | START_READ));
        prom.push(filler());
        prom.push(Insn::new(ALU | SETM | SRC_MD | a_dest(0o200 + k)));
    }
    machine(&prom, reads)
}

fn machine(prom: &[Insn], addresses: &[u32]) -> Machine {
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
    m.tv.set_board(muir::tv::Board::Video);
    let mut words = vec![filler(); 1024];
    words[..prom.len()].copy_from_slice(prom);
    words[STOP as usize] = Insn::new(JUMP | target(STOP as u64) | ALWAYS | N);
    words[prom.len()] = Insn::new(JUMP | target(STOP as u64) | ALWAYS | N);
    m.load_prom(&words);
    support::prom_program_in_ram(&mut m);
    for (k, &p) in addresses.iter().enumerate() {
        m.mmem[1 + k] = support::quux_map(&mut m, 1 + k as u32, p).into();
    }
    for k in 0..6 {
        m.amem[0o200 + k] = 0o525252;
    }
    m
}

/// Runs an engine to [`STOP`]: the machine, and the instant it got there.
fn run<E: Engine>(mut e: E, ns: impl Fn(&E) -> u64) -> (Machine, u64) {
    e.boot();
    for _ in 0..4000 {
        if e.machine().opc == STOP {
            return (e.machine().clone(), ns(&e));
        }
        e.step().unwrap();
    }
    panic!("the program never reached its end");
}

fn rtl(m: Machine) -> (Machine, u64) {
    run(Rtl::new(m), Rtl::ns)
}

/// The feature page's word 0, the MACHINE-ID: a register that answers.
const REGISTER: u32 = muir::machine::REGISTER_PAGE_13;
/// The register page's last word, reserved: a register that answers 0.
const RESERVED: u32 = REGISTER | 0o377;

/// Addresses nothing answers (contracts Q13, G2 §4.1): past main memory's
/// end, 2MW here; the CADR's addresses QUUX's devices once had, past it:
/// the color TV's buffer at `17200000`, the old register page at
/// `17377000` and the page after it, the display's old mode register at
/// `17377760`, between it and the disk's old registers, those at
/// `17377774`, and the CADR's Unibus window's first word and its last
/// below its last page; past the frame buffer in its window; and the
/// word below the register page.
const EMPTY: [u32; 11] = [
    0o10000000,
    0o17200000,
    0o17377000,
    0o17377400,
    0o17377760,
    0o17377770,
    0o17377774,
    0o17400000,
    0o17777377,
    muir::machine::WINDOW_13 + muir::tv::VIDEO_WORDS,
    REGISTER - 1,
];

/// **A register access takes a microcycle more than a failed one, and a
/// failed one is at once**: the same program, reading the MACHINE-ID or an
/// empty address, ends one microcycle later for the register; and the
/// empty address costs no timeout, the whole program ending well inside
/// the CADR's 4.25 us.
#[test]
fn a_register_takes_a_microcycle_more_than_nothing() {
    let (m, answered) = rtl(reading(&[REGISTER]));
    assert_eq!(m.amem[0o200], Geometry::QUUX.machine_id.unwrap().into(), "the register's word");
    assert_eq!(m.bus_error & bus_error::XBUS_NXM, 0);
    for empty in EMPTY {
        let (m, failed) = rtl(reading(&[empty]));
        assert_eq!(m.amem[0o200], 0, "{empty:o} reads 0");
        assert_ne!(m.bus_error & bus_error::XBUS_NXM, 0, "{empty:o}: the NXM bit");
        assert_eq!(answered - failed, 40, "{empty:o}: a microcycle more for the register");
        assert!(failed < 1_000, "{empty:o}: no timeout, the program ended at {failed} ns");
    }
}

/// **Every empty address fails the same on both engines**: 0 read, the NXM
/// bit set, a write going nowhere.
#[test]
fn nothing_answers_on_either_engine() {
    for empty in EMPTY {
        let prom = [
            Insn::new(ALU | SETA | a_src(0o100) | MD),
            Insn::new(ALU | SETM | m_src(1) | START_WRITE),
            filler(),
            filler(),
            Insn::new(ALU | SETM | m_src(1) | START_READ),
            filler(),
            Insn::new(ALU | SETM | SRC_MD | a_dest(0o200)),
        ];
        let setup = || {
            let mut m = machine(&prom, &[empty]);
            m.amem[0o100] = 0o37;
            m
        };
        let (r, _) = rtl(setup());
        let (e, _) = run(Micro::new(setup()), |_| 0);
        for (name, m) in [("rtl", r), ("micro", e)] {
            assert_eq!(m.amem[0o200], 0, "{name}, {empty:o}: the write did not read back");
            assert_ne!(m.bus_error & bus_error::XBUS_NXM, 0, "{name}, {empty:o}");
        }
    }
}

/// **A reserved register reads 0 and answers**: no NXM there.
#[test]
fn a_reserved_register_answers_0() {
    let (m, _) = rtl(reading(&[RESERVED]));
    assert_eq!(m.amem[0o200], 0);
    assert_eq!(m.bus_error & bus_error::XBUS_NXM, 0);
}

/// **The frame buffer is on the memory bus, through the cache**: a word
/// written reaches the display, and read twice it misses once and hits
/// once, each read a fixnum, tag `005` --- where a register is never looked
/// up.
#[test]
fn the_frame_buffer_is_cached() {
    let fb = muir::machine::WINDOW_13 + 0o100;
    let prom = [
        Insn::new(ALU | SETA | a_src(0o100) | MD),
        Insn::new(ALU | SETM | m_src(1) | START_WRITE),
        filler(),
        filler(),
        Insn::new(ALU | SETM | m_src(1) | START_READ),
        filler(),
        Insn::new(ALU | SETM | SRC_MD | a_dest(0o200)),
        Insn::new(ALU | SETM | m_src(1) | START_READ),
        filler(),
        Insn::new(ALU | SETM | SRC_MD | a_dest(0o201)),
        Insn::new(ALU | SETM | m_src(2) | START_READ),
        filler(),
        Insn::new(ALU | SETM | SRC_MD | a_dest(0o202)),
    ];
    let mut m = machine(&prom, &[fb, REGISTER]);
    m.amem[0o100] = 0o123456;
    let mut e = Rtl::new(m);
    e.boot();
    while e.machine().opc != STOP {
        e.step().unwrap();
    }
    let m = e.machine();
    assert_eq!(m.tv.read_buffer(0o100), 0o123456, "the display has the word");
    let word = muir::machine::UNBOXED_TAG | 0o123456;
    assert_eq!([m.amem[0o200], m.amem[0o201]], [word, word]);
    let c = e.cache().unwrap();
    assert_eq!((c.misses, c.hits), (1, 1), "the buffer's two reads; the register none");
}

/// **A memory start in the microcycle right after another start waits for
/// it** (QUUX's own interlock). A write of the MACHINE-ID, which goes
/// nowhere, then a read of main memory in the next microcycle; the same
/// after a write of the video controller's mode; a write of main memory, then
/// a read of the mode register in the next. Each first cycle lands, with
/// its own address, direction and word, and each second reads its own
/// word, on both engines. On the CADR the board has no interlock there and
/// the first cycle is lost
/// (`on_the_board_a_start_right_after_a_start_loses_the_first` in
/// `tests/chip.rs`).
#[test]
fn a_start_right_after_a_start_waits_for_it() {
    const MODE: u32 = REGISTER | 0o210;
    const WORD: u32 = 0o1000;
    let read = |m: u64, a: u64| {
        [
            Insn::new(ALU | SETM | m_src(m) | START_READ),
            filler(),
            Insn::new(ALU | SETM | SRC_MD | a_dest(a)),
        ]
    };
    let mut prom = vec![
        Insn::new(ALU | SETA | a_src(0o110) | MD),
        Insn::new(ALU | SETM | m_src(1) | START_WRITE),
    ];
    prom.extend(read(3, 0o200));
    prom.push(Insn::new(ALU | SETM | m_src(2) | START_WRITE));
    prom.extend(read(3, 0o201));
    prom.push(Insn::new(ALU | SETA | a_src(0o111) | MD));
    prom.push(Insn::new(ALU | SETM | m_src(3) | START_WRITE));
    prom.extend(read(2, 0o202));
    prom.extend(read(3, 0o203));
    let setup = || {
        let mut m = machine(&prom, &[REGISTER, MODE, WORD]);
        // Black-on-white, the one bit of the mode register that reads back.
        m.amem[0o110] = 4;
        m.amem[0o111] = 0o707070;
        m.main[WORD as usize] = 0o123456;
        m
    };
    let (r, _) = rtl(setup());
    let (e, _) = run(Micro::new(setup()), |_| 0);
    for (name, m) in [("rtl", r), ("micro", e)] {
        let got = [m.amem[0o200], m.amem[0o201], m.amem[0o202], m.amem[0o203]];
        assert_eq!(got, [0o123456, 0o123456, 4, 0o707070], "{name}: the words read");
        assert_eq!(m.main[WORD as usize], 0o707070, "{name}: the memory write landed");
        assert_eq!(m.bus_error & bus_error::XBUS_NXM, 0, "{name}: every cycle answered");
    }
}

/// **A register write started right behind a register read holds `MD` no
/// longer than a main memory write there**: a read of the MACHINE-ID, a
/// write of the register page's reserved word 105 in the next microcycle
/// (held until the read has gone out), a filler, then `MD` read. The read's
/// word is in `MD` when its own `READ IN PROGRESS` falls, which the write's
/// acknowledgement does not put off: the microcycle reading `MD` takes two
/// microcycles, 80 ns, as with a write of main memory behind the read ---
/// muir-fpga's fabric's figure, which muir takes (Q7
/// fixes a single register access at two microcycles and says nothing of one
/// behind another). Both engines read the word and set no NXM bit; `micro`
/// has no wait for `MD` to time.
#[test]
fn a_register_write_right_after_a_register_read_holds_md_no_longer() {
    const WORD_105: u32 = REGISTER | 0o105;
    const MEMORY: u32 = 0o1000;
    let prom = [
        Insn::new(ALU | SETM | m_src(1) | START_READ),
        Insn::new(ALU | SETM | m_src(2) | START_WRITE),
        filler(),
        Insn::new(ALU | SETM | SRC_MD | a_dest(0o200)),
    ];
    // How long the microcycle reading `MD`, at 3, takes on `rtl`.
    let md_microcycle = |behind: u32| {
        let mut e = Rtl::new(machine(&prom, &[REGISTER, behind]));
        e.boot();
        let (mut from, mut to) = (None, None);
        for _ in 0..4000 {
            let (pc, ns) = (e.machine().opc, e.ns());
            match pc {
                3 => _ = from.get_or_insert(ns),
                4 => _ = to.get_or_insert(ns),
                STOP => return (e.machine().clone(), to.unwrap() - from.unwrap()),
                _ => {}
            }
            e.step().unwrap();
        }
        panic!("the program never reached its end");
    };
    let (r, page) = md_microcycle(WORD_105);
    let (_, memory) = md_microcycle(MEMORY);
    assert_eq!(memory, 80, "main memory behind the read: MD read in two microcycles");
    assert_eq!(page, 80, "word 105 behind the read: MD read in two microcycles");
    let (e, _) = run(Micro::new(machine(&prom, &[REGISTER, WORD_105])), |_| 0);
    for (name, m) in [("rtl", r), ("micro", e)] {
        assert_eq!(
            m.amem[0o200],
            Geometry::QUUX.machine_id.unwrap().into(),
            "{name}: the word read"
        );
        assert_eq!(m.bus_error & bus_error::XBUS_NXM, 0, "{name}: every cycle answered");
    }
}
