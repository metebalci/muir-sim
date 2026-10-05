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
//! keeps its own.

use muir::cache::{CacheConfig, MemoryTiming};
use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{
    ALU, ALWAYS, JUMP, MD, N, POPJ, SETA, SETM, SRC_MD, START_READ, START_WRITE, a_dest, a_src,
    filler, m_src, target,
};
use muir::machine::{Geometry, Machine, bus_error};
use muir::micro::Micro;
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
    let mut m = Machine::new();
    m.geometry = geometry;
    let mut words = vec![filler(); 1024];
    words[..prom.len()].copy_from_slice(prom);
    m.load_prom(&words);
    support::prom_program_in_ram(&mut m);
    for (k, &p) in addresses.iter().enumerate() {
        m.mmem[1 + k] = if geometry.wide() {
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

/// Boots `e` and runs it well past the program's last jump.
fn run<E: Engine>(e: &mut E) {
    e.boot();
    for _ in 0..4000 {
        e.step().unwrap();
    }
}

/// **QUUX has its memory port and no bus interface**: the cache fitted at
/// its shape and main memory at the nominal timing; the CADR keeps its bus
/// interface and has neither.
#[test]
fn quux_has_a_memory_port_and_no_bus_interface() {
    let quux = Rtl::new(reading(Geometry::QUUX, &[0o1000]));
    assert!(quux.busint().is_none(), "no bus interface");
    let fitted = CacheConfig { line_words: 8, ..CacheConfig::with_words(4096) };
    assert_eq!(quux.cache().map(|c| c.config), Some(fitted));
    assert_eq!(quux.memory_timing(), Some(MemoryTiming::NOMINAL));
    assert_eq!(MemoryTiming::NOMINAL, MemoryTiming { read_ns: 380, write_ns: 290 });
    let cadr = Rtl::new(reading(Geometry::CADR, &[0o1000]));
    assert!(cadr.busint().is_some(), "the CADR's bus interface");
    assert!(cadr.cache().is_none() && cadr.memory_timing().is_none());
}

/// **A miss is a line fill at the nominal time, a hit the cache's**: the
/// word read first misses and waits 410 ns for its line of 5 beats; the
/// next word of the line hits and waits 20. Both are main memory's words.
#[test]
fn a_miss_fills_its_line_at_the_nominal_time() {
    let mut e = Rtl::new(reading(Geometry::QUUX, &[0o1000, 0o1001]));
    e.boot();
    let mut at = Vec::new();
    for _ in 0..4000 {
        e.step().unwrap();
        at.push(e.ns());
    }
    let m = e.machine();
    assert_eq!([m.amem[0o200], m.amem[0o201]], [0o1001000, 0o1001001]);
    let c = e.cache().unwrap();
    assert_eq!((c.hits, c.misses), (1, 1), "a miss, then a hit in its line");
    // The microcycles ended 40 ns apart but where one waited for MD, in
    // whole microcycles: the miss's line fill, and the hit's 20 ns.
    let waits: Vec<u64> = at.windows(2).map(|w| w[1] - w[0]).filter(|&d| d > 40).collect();
    assert_eq!(waits.len(), 2, "the miss's and the hit's: {waits:?}");
    assert!((410..=410 + 80).contains(&waits[0]), "the line fill: {waits:?}");
    assert!(waits[1] <= 80, "the hit, a microcycle at most: {waits:?}");
}

/// **A device register is never cached**: two reads of the keyboard's data word take
/// two key words out of the FIFO, and the cache sees neither.
#[test]
fn a_device_register_is_never_cached() {
    use muir::quux_input::KeyboardMouse;
    let data = muir::machine::REGISTER_PAGE_13 | 0o121;
    let mut m = reading(Geometry::QUUX, &[data, data]);
    m.quux_input.press(0o101);
    m.quux_input.press(0o102);
    let mut e = Rtl::new(m);
    run(&mut e);
    let m = e.machine();
    assert_eq!([m.amem[0o200], m.amem[0o201]], [0o101, 0o102], "each read reached the FIFO");
    let c = e.cache().unwrap();
    assert_eq!(c.hits + c.misses, 0, "no lookup for a register");
}

/// **Past main memory's end is nothing, and nothing is cached there**: a
/// write and a read-back at the first word past it both fail with the NXM bit,
/// and the read gives 0, not the word written --- what the microcode's
/// memory-size probe (`MEM-SIZE-LOOP`, `uc-cold-disk.lisp`) relies on. On
/// both engines.
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
    let mut r = Rtl::new(setup());
    run(&mut r);
    let mut e = Micro::new(setup());
    run(&mut e);
    for (name, m) in [("rtl", r.machine()), ("micro", e.machine())] {
        assert_eq!(m.amem[0o200], 0, "{name}: the write did not read back");
        assert_ne!(m.bus_error & bus_error::XBUS_NXM, 0, "{name}: the NXM bit");
        assert_eq!(m.bus_error & bus_error::UNIBUS_NXM, 0, "{name}");
    }
    let c = r.cache().unwrap();
    assert_eq!(c.hits + c.misses, 0, "no lookup past main memory");
    assert!(!c.holds(end));
}

/// **After the disk writes main memory, no read hits the old word**: a line
/// held, the disk's transfer changing its word, the next read misses and
/// reads the disk's word.
#[test]
fn after_the_disk_writes_no_read_hits_the_old_word() {
    let mut e = Rtl::new(reading(Geometry::QUUX, &[0o1000, 0o1000]));
    e.boot();
    // Up to the first read's word in A 200: its line is held.
    for _ in 0..200 {
        if e.machine().amem[0o200] == 0o1001000 {
            break;
        }
        e.step().unwrap();
    }
    assert_eq!(e.machine().amem[0o200], 0o1001000, "the first read");
    assert!(e.cache().unwrap().holds(0o1000));
    // The disk's transfer, as block-disk makes it: the word, and the flag.
    e.machine_mut().main[0o1000] = 0o7654321;
    e.machine_mut().dma_written = true;
    for _ in 0..200 {
        e.step().unwrap();
    }
    assert_eq!(e.machine().amem[0o201], 0o7654321, "the disk's word");
    assert_eq!(e.cache().unwrap().misses, 2, "the second read missed");
}

/// **A checkpoint on QUUX has no bus interface in it**: saved mid-run and
/// resumed, the run goes on as the one saved, cache and all.
#[test]
fn a_checkpoint_keeps_the_memory_port() {
    use muir::checkpoint::{Reader, Writer};
    let reads: Vec<u32> = (0..6).map(|k| 0o1000 + 5 * k).collect();
    let mut e = Rtl::new(reading(Geometry::QUUX, &reads));
    e.boot();
    for _ in 0..30 {
        e.step().unwrap();
    }
    let mut w = Writer::new();
    e.save(&mut w);
    let body = w.finish();
    let mut back = Rtl::new(reading(Geometry::QUUX, &reads));
    back.load(&mut Reader::for_word_bits(&body, 40)).unwrap();
    for _ in 0..300 {
        e.step().unwrap();
        back.step().unwrap();
    }
    assert_eq!(e.ns(), back.ns());
    assert_eq!(e.machine().amem[0o200..0o206], back.machine().amem[0o200..0o206]);
    let (a, b) = (e.cache().unwrap(), back.cache().unwrap());
    assert_eq!((a.hits, a.misses), (b.hits, b.misses));
}

/// One bus cycle as `rtl` shows it from the step that took it: the edge it
/// was taken at, when it was answered and when acknowledged; how many
/// later step boundaries fell before the acknowledgement, and whether
/// `-MEMGRANT` was low at all of them.
#[derive(Debug)]
struct Cycle {
    edge: u64,
    answered: u64,
    ack: u64,
    inside: u32,
    held: bool,
}

/// Runs `e` to the end of its program in steps of at most 20 ns, so that a
/// hang is seen from inside, noting each bus cycle as the step that took it
/// left it.
fn cycles(e: &mut Rtl) -> Vec<Cycle> {
    e.boot();
    let mut out: Vec<Cycle> = Vec::new();
    let mut was = false;
    for _ in 0..4000 {
        e.step_until(e.ns() + 20).unwrap();
        let granted = e.bus_granted();
        assert_eq!(granted, e.bus_ack_at().is_some(), "the acknowledgement is due while granted");
        assert_eq!(granted, e.bus_answered_at().is_some());
        if granted && !was {
            out.push(Cycle {
                edge: e.ns(),
                answered: e.bus_answered_at().unwrap(),
                ack: e.bus_ack_at().unwrap(),
                inside: 0,
                held: true,
            });
        } else if let Some(c) = out.last_mut()
            && e.ns() < c.ack
        {
            c.inside += 1;
            c.held &= granted && e.bus_ack_at() == Some(c.ack);
        }
        was = granted;
    }
    out
}

/// **`rtl` shows the running cycle's acknowledgement and grant on QUUX**:
/// `bus_granted` is `-MEMGRANT` low, `bus_ack_at` when `-MEMACK` is due,
/// forwarded from the memory port. A read miss is acknowledged as it is
/// answered, a line fill (410 ns, 5 beats) after the edge that took it; a hit the
/// hit time (20 ns) after; a device register is answered at the edge and
/// acknowledged a microcycle (40 ns at K=4) later; an empty address is
/// answered and acknowledged at the edge.
#[test]
fn rtl_shows_the_memory_port_s_acknowledgement_and_grant() {
    const REGISTER: u32 = muir::machine::REGISTER_PAGE_13;
    const EMPTY: u32 = 0o17377400;
    let mut e = Rtl::new(reading(Geometry::QUUX, &[0o1000, 0o1001, REGISTER, EMPTY]));
    let cs = cycles(&mut e);
    assert_eq!(cs.len(), 4, "{cs:?}");
    let m = e.machine();
    assert_eq!(
        m.amem[0o200..0o204],
        [0o1001000, 0o1001001, Geometry::QUUX.machine_id.unwrap().into(), 0]
    );
    let (miss, hit, register, empty) = (&cs[0], &cs[1], &cs[2], &cs[3]);
    for c in &cs {
        assert!(c.held, "granted until acknowledged: {c:?}");
    }
    assert!(miss.inside > 0, "the line fill's hang seen from inside: {miss:?}");
    assert_eq!((miss.answered - miss.edge, miss.ack - miss.edge), (410, 410), "miss: {miss:?}");
    assert_eq!((hit.answered - hit.edge, hit.ack - hit.edge), (20, 20), "hit: {hit:?}");
    assert_eq!(register.answered, register.edge, "register: {register:?}");
    assert_eq!(register.ack, register.answered + 40, "register: {register:?}");
    assert_eq!((empty.answered, empty.ack), (empty.edge, empty.edge), "empty: {empty:?}");
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

/// **A write carries the `MD` of the microcycle after its start**, as on
/// the CADR (`the_engines_write_the_md_of_the_microcycle_after_the_start`,
/// `tests/chip.rs`): QUUX's port takes the word at the same edge, and `MD`
/// loaded a microcycle later is not written. `rtl` and `micro` alike.
#[test]
fn a_write_carries_the_md_of_the_microcycle_after_its_start() {
    let prom = [
        Insn::new(ALU | SETA | a_src(0o110) | MD),
        Insn::new(ALU | SETM | m_src(1) | START_WRITE),
        Insn::new(ALU | SETA | a_src(0o111) | MD),
        filler(),
        filler(),
        Insn::new(ALU | SETA | a_src(0o110) | MD),
        Insn::new(ALU | SETM | m_src(2) | START_WRITE),
        filler(),
        Insn::new(ALU | SETA | a_src(0o111) | MD),
    ];
    let setup = || {
        let mut m = machine(Geometry::QUUX, &prom, &[0o1000, 0o1001]);
        m.amem[0o110] = 0o1111;
        m.amem[0o111] = 0o2222;
        m
    };
    let mut r = Rtl::new(setup());
    run(&mut r);
    let mut e = Micro::new(setup());
    run(&mut e);
    for (name, m) in [("rtl", r.machine()), ("micro", e.machine())] {
        assert_eq!(m.main[0o1000], 0o2222, "{name}: MD loaded in the microcycle after the start");
        assert_eq!(m.main[0o1001], 0o1111, "{name}: and not a microcycle later");
    }
}

/// **A start held behind a write's start loads its `MD` after the write
/// has gone out.** A write of 1111 at 1000, then, in the next microcycle,
/// `MD` <- 2222 with a read of 1000: QUUX's `-WAIT` term `MEMSTART AND
/// MEMOP` holds the whole second microcycle, its `MD` load with it, until
/// the write has gone out with 1111 (`a_start_right_after_a_start_waits_for_it`,
/// `tests/quux_device_registers.rs`); the read then gets 1111. On `rtl`
/// and `micro` alike.
#[test]
fn a_start_held_behind_a_write_loads_md_after_the_write() {
    // Functional destination 31, `MD` with a read started.
    let md_start_read = (0o31 << 19) | (0o37 << 14);
    let prom = [
        Insn::new(ALU | SETA | a_src(0o110) | MD),
        Insn::new(ALU | SETM | m_src(1) | START_WRITE),
        Insn::new(ALU | SETA | a_src(0o111) | md_start_read),
        filler(),
        Insn::new(ALU | SETM | SRC_MD | a_dest(0o200)),
    ];
    let setup = || {
        let mut m = machine(Geometry::QUUX, &prom, &[0o1000]);
        m.amem[0o110] = 0o1111;
        m.amem[0o111] = 0o2222;
        m
    };
    let mut r = Rtl::new(setup());
    run(&mut r);
    let mut e = Micro::new(setup());
    run(&mut e);
    for (name, m) in [("rtl", r.machine()), ("micro", e.machine())] {
        assert_eq!(m.main[0o1000], 0o1111, "{name}: the write, with the MD of before the hold");
        assert_eq!(m.amem[0o200], 0o1111, "{name}: the read, after it");
    }
}

/// **An instruction fetch right after a write's start waits for the
/// write**, as any start does on QUUX: a `POPJ` whose return asks for a
/// fetch, on the write's own instruction, puts the fetch in the next
/// microcycle, which also loads `MD` with 2222. The hold keeps that load
/// back until the write has gone out with 1111, and the fetch then reads
/// its word into `MD`. On the CADR the write is lost to the fetch
/// (`a_fetch_right_after_a_write_loses_the_write`, `tests/chip.rs`). On
/// `rtl` and `micro` alike.
#[test]
fn a_fetch_right_after_a_write_waits_for_it() {
    let spc_push = (0o15 << 19) | (0o37 << 14);
    let lc = (0o1 << 19) | (0o37 << 14);
    let mut prom = vec![
        Insn::new(ALU | SETA | a_src(0o110) | MD),
        Insn::new(ALU | SETA | a_src(0o112) | spc_push),
        Insn::new(ALU | SETA | a_src(0o113) | lc),
        Insn::new(ALU | SETM | m_src(1) | START_WRITE | POPJ),
        Insn::new(ALU | SETA | a_src(0o111) | MD),
    ];
    prom.resize(20, filler());
    prom.extend([filler(), filler(), filler(), Insn::new(ALU | SETM | SRC_MD | a_dest(0o200))]);
    let setup = || {
        let mut m = machine(Geometry::QUUX, &prom, &[0o1000, 0o1020]);
        m.amem[0o110] = 0o1111;
        m.amem[0o111] = 0o2222;
        // The return to 20, asking for a fetch, and LC at the second word.
        m.amem[0o112] = 20 | (1 << 14);
        m.amem[0o113] = m.mmem[2] << 2;
        m
    };
    let mut r = Rtl::new(setup());
    run(&mut r);
    let mut e = Micro::new(setup());
    run(&mut e);
    for (name, m) in [("rtl", r.machine()), ("micro", e.machine())] {
        assert_eq!(m.main[0o1000], 0o1111, "{name}: the write, with the MD of before the hold");
        assert_eq!(m.amem[0o200], 0o1001020, "{name}: the fetched word");
    }
}
