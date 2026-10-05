// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Checkpoints: the file's packing and header, and a run picked up from
//! one being the run that was never stopped.  The engine tests boot the
//! System 100 pack, so they skip and say so without `vendor/`.

use std::path::PathBuf;

use muir::checkpoint::{self, Reader, Writer};
use muir::clock::Behavioral;
use muir::disk_unit::{Geometry, Unit};
use muir::engine::Engine;
use muir::machine::Machine;
use muir::micro::Micro;
use muir::rtl::Rtl;

mod support;

/// **Packing keeps every byte and costs a run of zeros almost nothing.**
#[test]
fn packing_keeps_the_bytes_and_shrinks_the_zeros() {
    let mut mixed = vec![0u8; 10];
    mixed.extend([1, 2, 0, 3, 0, 0, 4]);
    mixed.extend([0u8; 100]);
    mixed.extend([5, 0, 0, 0, 0, 6]);
    for raw in [vec![], vec![1, 2, 3], vec![0; 10], vec![0, 0, 0, 7], vec![7, 0, 0, 0], mixed] {
        assert_eq!(checkpoint::unpack(&checkpoint::pack(&raw)).unwrap(), raw, "{raw:?}");
    }
    let empty_memory = vec![0u8; 8 << 20];
    assert!(checkpoint::pack(&empty_memory).len() < 8, "eight megabytes of zeros in a few bytes");
    assert!(checkpoint::unpack(&[3, 5, 1]).is_err(), "literals that run off the end");
}

/// **A writer's fields read back in order, and a reader says when they do
/// not fit.**
#[test]
fn the_fields_read_back_in_order() {
    let mut w = Writer::new();
    w.u8(7);
    w.u16(0x1234);
    w.u32(0xdead_beef);
    w.u64(u64::MAX - 1);
    w.bool(true);
    w.opt(Some(9u8), Writer::u8);
    w.opt(None::<u16>, Writer::u16);
    w.u32s(&[1, 2, 3]);
    w.bytes(b"pack");
    w.speed(muir::clock::Speed::Fast);
    let body = w.finish();
    let mut r = Reader::new(&body);
    assert_eq!(r.u8().unwrap(), 7);
    assert_eq!(r.u16().unwrap(), 0x1234);
    assert_eq!(r.u32().unwrap(), 0xdead_beef);
    assert_eq!(r.u64().unwrap(), u64::MAX - 1);
    assert!(r.bool().unwrap());
    assert_eq!(r.opt(Reader::u8).unwrap(), Some(9));
    assert_eq!(r.opt(Reader::u16).unwrap(), None);
    let mut three = [0u32; 3];
    r.u32s_into(&mut three).unwrap();
    assert_eq!(three, [1, 2, 3]);
    assert_eq!(r.bytes().unwrap(), b"pack");
    assert_eq!(r.speed().unwrap(), muir::clock::Speed::Fast);
    r.done().unwrap();
    assert!(r.u8().is_err(), "nothing left to read");

    let mut r = Reader::new(&[2]);
    assert!(r.bool().is_err(), "2 is no flag");
    let mut w = Writer::new();
    w.u32s(&[1, 2]);
    let body = w.finish();
    let mut into = [0u32; 3];
    assert!(Reader::new(&body).u32s_into(&mut into).is_err(), "two words where three belong");
    assert!(Reader::new(&body).done().is_err(), "bytes left over");
}

/// **The file names the engine that wrote it, and anything else is
/// refused.**
#[test]
fn the_file_names_its_engine_and_refuses_other_files() {
    let dir = std::env::temp_dir().join(format!("muir-checkpoint-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.chk");
    let body: Vec<u8> =
        (0..300u32).map(|i| if i % 7 == 0 { i } else { 0 }).flat_map(|v| v.to_le_bytes()).collect();
    let size = checkpoint::write(&path, "micro", 4, 32, &body).unwrap();
    assert_eq!(size, std::fs::metadata(&path).unwrap().len());
    let back = checkpoint::read(&path).unwrap();
    assert_eq!(back.engine, "micro");
    assert_eq!(back.memory_boards, 4);
    assert_eq!(back.body, body);
    std::fs::write(&path, b"GIF89a not a checkpoint at all").unwrap();
    let err = checkpoint::read(&path).unwrap_err().to_string();
    assert!(err.contains("not a muir checkpoint"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

/// **A file of another version is refused by number**, and the version
/// this build writes is the one this test's name carries --- the name is
/// where the number lives, so that a bump has to be deliberate and the
/// prose below cannot go stale behind it, as it did between versions 11
/// and 13.  The version is bumped whenever a type changes what it writes,
/// so a file from another build is read wrong or not at all; this pins
/// which it is, and that the refusal names both versions. Version 3 added
/// the serial port's registers to the I/O board's, version 4 the instant
/// the interval timer was loaded, version 5 `micro`'s pending map write,
/// version 6 the speaker's flip-flop, version 7 the mouse interface's
/// latches and clock with the encoders on its lines, version 8 the bit
/// count of what the Chaosnet interface received or has landing, and
/// version 9 `micro`'s `NEXT INSTR` and `NEXT INSTRD`, the two stages
/// of the fetch a `POPJ` asks for, version 10 the disk controller's
/// overrun, version 11 the drives on a netlist disk controller's
/// cable and the multiplexor between them, version 12 one format for the
/// harness and the binary (`a2f4aa2`), version 13 the disk
/// controller's header ECC error, version 14 `LC` among the signals
/// `rtl` records for the cosimulation to compare, version 15 the instant
/// a drive's attention comes rather than whether it has come, version 16
/// the header each sector of a pack carries where it is not the one its
/// address implies, version 17 the header compare error that carrying
/// them makes possible, version 18 the checkword written after each of
/// those headers, which is the other half of what a formatter lays down, version 19 the spurious sector pulse a drive can be told to emit,
/// version 20 the checkword written after each data field with the two
/// ECC errors a bad one gives, version 21 the transitions on their way
/// down a netlist board's delay lines, which is what a machine has to be
/// saved with to be saved while it is busy, version 22 when the
/// display's sync program last started, its timing being that program
/// run rather than a fixed frame, version 23 which display board the
/// machine has and the color map the LISPM TV's register 4 writes, and
/// version 24 the color TV, the second display board, with whether the
/// machine had one at all, version 25 that board as a netlist on
/// `chip`'s backplane: which kind of color board a `chip` checkpoint was
/// taken with is a word at the front of it, and a netlist one is a second
/// device board in the file after it --- and version 26 the sync bits a
/// display was holding when its sync program last started, which are what
/// its mode register reads until that program's first instruction lands,
/// and version 27 whose time an `rtl` run keeps, `--timing-model`, which
/// `tests/timing_model.rs` and `tests/muir_checkpoint.rs` hold, and
/// version 28 what `micro` carries across a microcycle edge besides:
/// `MEMSTART`, the PDL and SPC writes still to land, the OPC shift
/// register and whether the standing inhibit is the boot's trap, which
/// `micro_carries_its_late_writes_across_a_checkpoint` and
/// `micro_picks_up_where_the_checkpoint_left_off` hold, and version 29
/// which machine it was, the map's geometry, with a level-2 map that has
/// room for QUUX's, which `a_resume_has_the_checkpoint_s_machine` in
/// `tests/muir_checkpoint.rs` holds, and version 30 a PDL buffer with room
/// for a 16K-word QUUX's, the pointer and index checked against the
/// geometry's width, which `a_pointer_wider_than_its_register_is_refused`
/// holds, and version 31 whether the machine has QUUX's multiply and
/// divide, and when QUUX's divider's count started on `rtl` --- the edge
/// that loaded `IR`, or for a `DIV` of `MD` the end of the MD interlock ---
/// which `a_checkpoint_keeps_the_divider_s_time` in `tests/muldiv.rs`
/// holds, and version 32 QUUX's tick, which `a_checkpoint_keeps_the_tick`
/// in `tests/tick.rs` holds, and version 33 the video controller's size, which
/// `another_size_is_followed_everywhere` in `tests/video.rs` holds, and
/// version 34 `sync`'s ticks with the timing model, which
/// `a_checkpoint_keeps_the_ticks` in `tests/sync_timing.rs` holds, and
/// version 35 whether `rtl`'s write pulse has fired in a microcycle a
/// `-HANG` holds (`tests/dispatch_write_order.rs`), and version 36 QUUX's
/// memory cache and write buffer and the disk's flag that invalidates it
/// (`tests/cache.rs`), and version 37 QUUX's block-disk
/// (`tests/block_disk.rs`), and version 38 QUUX's clocks of contract Q1,
/// the interval timer beside the fixed tick, which
/// `a_checkpoint_keeps_the_clocks` in `tests/tick.rs` holds, and version 39
/// QUUX's keyboard FIFO and mouse, which `a_checkpoint_keeps_it` in
/// `tests/quux_input.rs` holds, and version 40 QUUX's memory port in place
/// of the bus interface, which `a_checkpoint_keeps_the_memory_port` in
/// `tests/quux_memory_port.rs` holds, and version 41 QUUX's disk of any
/// size in place of a T-300's drive under block-disk, its size in blocks
/// and the blocks written, which `a_checkpoint_keeps_the_disk` in
/// `tests/quux_disk.rs` holds, and version 42 QUUX's real-time clock, live
/// or counted from `--rtc`'s start, which `a_checkpoint_keeps_the_count` in
/// `tests/quux_rtc.rs` holds, and version 43 QUUX's file device, its
/// registers and its rings' indexes, which
/// `a_checkpoint_waits_for_an_idle_device_and_keeps_its_registers` in
/// `tests/quux_file_device.rs` holds, and version 44 `micro`'s write not
/// yet gone out, which `micro_carries_a_write_not_yet_gone_out_across_a_checkpoint`
/// holds, and version 45 QUUX's three interval timers in place of Q1's
/// tick and interval timer (contract Q11), which
/// `m8_a_checkpoint_resumes_to_the_same_rises` in `tests/interval_timers.rs`
/// holds, and version 46 destination 3 no longer timer 0's control at
/// revision 10, so that a version-45 checkpoint whose timer 0 a band turned
/// on through destination 3 is refused rather than resumed with nothing to
/// clear it, which `destination_3_writes_only_m_at_revision_10` in
/// `tests/interval_timers.rs` holds, and version 47 QUUX's register page at
/// `17777400` with block-disk and the video controller on it, word 100 in
/// its final order and the display board named `video` (contract Q13,
/// revision 11), so that a version-46 checkpoint of a revision-10 machine is
/// refused rather than resumed on a page it does not know, and version 48
/// QUUX's MACRO-DISPATCH register and MACRO DISPATCH MEMORY, and whether the
/// machine has them (contract H8a), and version 49 the operand address's
/// two base copies and the operand address a fused return has armed, which
/// `a_checkpoint_keeps_the_register_and_the_memory` in
/// `tests/macro_dispatch.rs` holds, with a `micro` PDL buffer write by
/// PDL-INDEX taking the index where it lands; version 50 is a 40-bit
/// machine's, QUUX revision 13's ([`checkpoint::VERSION_40`],
/// `tests/revision_13_memory.rs`). QUUX revision 12, retired, wrote version
/// 49 too, and its checkpoint is refused by the machine its body records
/// (`a_revision_12_checkpoint_is_refused_by_its_machine` in
/// `tests/quux_revision.rs`).
#[test]
fn the_format_is_version_49_and_another_version_is_refused() {
    assert_eq!(checkpoint::VERSION, 49, "a new version needs its own tests");
    assert_eq!(checkpoint::VERSION_40, 50);
    let dir = std::env::temp_dir().join(format!("muir-checkpoint-version-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.chk");
    checkpoint::write(&path, "micro", 4, 32, &[1, 2, 3]).unwrap();
    let good = std::fs::read(&path).unwrap();
    // The version is the four bytes after the magic line.
    let at = b"muir checkpoint\n".len();
    assert_eq!(&good[at..at + 4], 49u32.to_le_bytes());
    for other in (1u32..checkpoint::VERSION).chain([51, u32::MAX]) {
        let mut file = good.clone();
        file[at..at + 4].copy_from_slice(&other.to_le_bytes());
        std::fs::write(&path, &file).unwrap();
        let err = checkpoint::read(&path).unwrap_err().to_string();
        assert!(err.contains(&format!("format version {other}")), "{err}");
        assert!(err.contains("reads 49 and 50"), "{err}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// **A run of zeros longer than any body is refused, not allocated.** The
/// body's size is not in the header, so a count is checked against the
/// most a body can be: a corrupt `--resume` file whose count says
/// eighteen exabytes, or a terabyte, is an error before a byte of it is
/// made room for.
#[test]
fn a_run_of_zeros_longer_than_any_body_is_refused() {
    // A count as an LEB128 varint, then a count of no literals.
    let packed = |zeros: u64| {
        let mut out = Vec::new();
        let mut v = zeros;
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
        out.push(0);
        out
    };
    assert_eq!(checkpoint::unpack(&packed(8 << 20)).unwrap(), vec![0u8; 8 << 20]);
    for zeros in [u64::MAX, 1 << 40] {
        let err = checkpoint::unpack(&packed(zeros)).unwrap_err().to_string();
        assert!(err.contains("zeros"), "{err}");
    }
}

/// **A pointer wider than its register is a corrupt checkpoint, and is
/// refused** before an engine indexes a stack with it. The SPC pointer is
/// `SPCPTR<4:0>`, five bits addressing the 32-word SPC stack; the PDL
/// pointer and index are ten bits addressing the 1K-word PDL buffer; the
/// engines mask every write to them, so a saved value is never wider, and
/// a wider one has been corrupted on the way.
#[test]
fn a_pointer_wider_than_its_register_is_refused() {
    let mut spc = Machine::with_memory_boards(1);
    spc.spcptr = 0o40;
    let mut pointer = Machine::with_memory_boards(1);
    pointer.pdl_pointer = 0o2000;
    let mut index = Machine::with_memory_boards(1);
    index.pdl_index = 0o2000;
    for (what, m) in [("SPC pointer", spc), ("PDL pointer", pointer), ("PDL index", index)] {
        let mut w = Writer::new();
        m.save(&mut w);
        let body = w.finish();
        let mut back = Machine::with_memory_boards(1);
        let err = back.load(&mut Reader::new(&body)).unwrap_err().to_string();
        assert!(err.contains(what), "{what}: {err}");
    }
    // The widest value each register holds loads as itself.
    let mut m = Machine::with_memory_boards(1);
    m.spcptr = 0o37;
    m.pdl_pointer = 0o1777;
    m.pdl_index = 0o1777;
    let mut w = Writer::new();
    m.save(&mut w);
    let body = w.finish();
    let mut back = Machine::with_memory_boards(1);
    let mut r = Reader::new(&body);
    back.load(&mut r).unwrap();
    r.done().unwrap();
    assert_eq!((back.spcptr, back.pdl_pointer, back.pdl_index), (0o37, 0o1777, 0o1777));
}

/// **A count of clock events with nothing behind it is an error, not an
/// allocation.** The `chip` clock saves its pending events behind a
/// count; the count is read from the file, and a corrupt one saying four
/// billion has to fail on the first event that is not there rather than
/// make room for all of them first.
#[test]
fn a_clock_with_more_events_than_the_file_holds_is_refused() {
    let mut file = b"CADRCLK1".to_vec();
    // The time, the cycle's start, the read phase and the four outputs.
    file.extend([0u8; 8 + 8 + 4 + 4]);
    file.extend(u32::MAX.to_le_bytes());
    assert!(Behavioral::load(&mut file.as_slice()).is_err());
    // And a clock as saved loads as itself.
    let clock = Behavioral::default();
    let mut saved = Vec::new();
    clock.save(&mut saved).unwrap();
    let back = Behavioral::load(&mut saved.as_slice()).unwrap();
    let mut again = Vec::new();
    back.save(&mut again).unwrap();
    assert_eq!(again, saved);
}

/// **A checkpoint names how much memory its machine had, and loads onto
/// no other.** Four boards' worth into a machine of thirty-two is refused
/// by name, before any word of memory is read.
#[test]
fn a_checkpoint_loads_onto_a_machine_with_as_much_memory() {
    let mut small = Micro::new(Machine::with_memory_boards(4));
    small.boot();
    let mut w = Writer::new();
    small.save(&mut w);
    let body = w.finish();
    let mut big = Micro::new(Machine::with_memory_boards(32));
    big.boot();
    let err = big.load(&mut Reader::new(&body)).unwrap_err().to_string();
    assert!(err.contains("4 memory boards") && err.contains("32"), "{err}");
    let mut same = Micro::new(Machine::with_memory_boards(4));
    same.boot();
    let mut r = Reader::new(&body);
    same.load(&mut r).unwrap();
    r.done().unwrap();
}

/// A machine booting the pack, as `muir` builds one.
fn machine(pack: &PathBuf) -> Machine {
    let mut m = Machine::new();
    m.load_prom(&muir::prom::boot_prom());
    m.disk.attach(0, Unit::open(pack, Geometry::T300).expect("the System 100 pack"));
    m
}

/// Runs `straight` to `at`, checkpoints it into `resumed`, then runs both
/// `more` microcycles in step: the same PC every microcycle, and the same
/// checkpoint at the end.
fn resumes<E: Engine>(name: &str, mut straight: E, mut resumed: E, at: u64, more: u64) {
    let (ran, halt) = straight.run(at);
    assert_eq!((ran, halt), (at, None), "{name}: the straight run to the checkpoint");
    let mut w = Writer::new();
    straight.save(&mut w);
    let body = w.finish();
    let mut r = Reader::new(&body);
    resumed.load(&mut r).unwrap_or_else(|e| panic!("{name}: loading the checkpoint: {e}"));
    r.done().unwrap();
    let mut w = Writer::new();
    resumed.save(&mut w);
    assert_eq!(w.finish(), body, "{name}: the checkpoint loads and saves as itself");
    assert_eq!(resumed.machine().cycles, straight.machine().cycles);
    for n in 0..more {
        straight.step().unwrap();
        resumed.step().unwrap();
        assert_eq!(resumed.pc(), straight.pc(), "{name}: the PC {n} microcycles on");
    }
    let (mut a, mut b) = (Writer::new(), Writer::new());
    straight.save(&mut a);
    resumed.save(&mut b);
    assert_eq!(a.finish(), b.finish(), "{name}: the same state {more} microcycles on");
    assert!(
        resumed.machine().mode.prom_disable,
        "{name}: the window reaches past the PROM into the band"
    );
}

/// **`micro` picks up where the checkpoint left off.** Into the band's
/// microcode load, where the PROM has handed over and the disk has been
/// read; then a hundred thousand microcycles more, in step.
#[test]
fn micro_picks_up_where_the_checkpoint_left_off() {
    let Some(pack) = support::pack_100() else { return };
    let mut straight = Micro::new(machine(&pack));
    straight.boot();
    let mut resumed = Micro::new(machine(&pack));
    resumed.boot();
    resumes("micro", straight, resumed, 1_600_000, 100_000);
}

/// **`rtl` picks up where the checkpoint left off.** The same, with the
/// Chaosnet interface plugged in, so its state goes and comes too.
#[test]
fn rtl_picks_up_where_the_checkpoint_left_off() {
    let Some(pack) = support::pack_100() else { return };
    let build = || {
        let mut m = machine(&pack);
        m.plug_chaos(0);
        let mut e = Rtl::new(m);
        e.boot();
        e
    };
    resumes("rtl", build(), build(), 1_600_000, 100_000);
}

/// FNV-1a over 64 bits: a file's bytes as one number to pin.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

/// The CADR's boot PROM on one memory board, run `at` microcycles from
/// power-on and checkpointed into a file of `engine`'s; the file's bytes.
fn cadr_file<E: Engine>(engine: &str, mut e: E, at: u64, path: &std::path::Path) -> Vec<u8> {
    e.boot();
    e.run(at);
    let mut w = Writer::new();
    e.save(&mut w);
    checkpoint::write(path, engine, 1, 32, &w.finish()).unwrap();
    std::fs::read(path).unwrap()
}

/// **A checkpoint of the CADR keeps format version 49's bytes**, as
/// written before QUUX's 32-bit revision 12 was retired: the retirement
/// left the CADR's format alone, so that a CADR checkpoint written then,
/// by `cadr` or by a board, still resumes. The digests are of the files
/// that build wrote of this same run, on each engine; the file resumes
/// here and runs on as the straight run does.
#[test]
fn a_cadr_checkpoint_keeps_version_49_s_bytes() {
    let dir = std::env::temp_dir().join(format!("muir-checkpoint-49-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let machine = || {
        let mut m = Machine::with_memory_boards(1);
        m.load_prom(&muir::prom::boot_prom());
        m
    };
    let (at, more) = (20_000, 20_000);
    let path = dir.join("micro.chk");
    let file = cadr_file("micro", Micro::new(machine()), at, &path);
    eprintln!("micro: {} bytes, digest {:#018x}", file.len(), fnv(&file));
    assert_eq!(fnv(&file), MICRO_49, "micro: version 49's bytes");
    let (mut straight, mut resumed) = (Micro::new(machine()), Micro::new(machine()));
    straight.boot();
    straight.run(at);
    resumed.boot();
    resumed.load(&mut checkpoint::read(&path).unwrap().reader()).unwrap();
    straight.run(more);
    resumed.run(more);
    let (mut a, mut b) = (Writer::new(), Writer::new());
    straight.save(&mut a);
    resumed.save(&mut b);
    assert_eq!(a.finish(), b.finish(), "micro: the resumed run is the straight one");
    let path = dir.join("rtl.chk");
    let file = cadr_file("rtl", Rtl::new(machine()), at, &path);
    eprintln!("rtl: {} bytes, digest {:#018x}", file.len(), fnv(&file));
    assert_eq!(fnv(&file), RTL_49, "rtl: version 49's bytes");
    let (mut straight, mut resumed) = (Rtl::new(machine()), Rtl::new(machine()));
    straight.boot();
    straight.run(at);
    resumed.boot();
    resumed.load(&mut checkpoint::read(&path).unwrap().reader()).unwrap();
    straight.run(more);
    resumed.run(more);
    let (mut a, mut b) = (Writer::new(), Writer::new());
    straight.save(&mut a);
    resumed.save(&mut b);
    assert_eq!(a.finish(), b.finish(), "rtl: the resumed run is the straight one");
    std::fs::remove_dir_all(&dir).ok();
}

/// The digests of [`a_cadr_checkpoint_keeps_version_49_s_bytes`]'s files.
const MICRO_49: u64 = 0x91ce_5d6b_229c_2e47;
const RTL_49: u64 = 0x110f_e8dc_822f_2ce8;

// --- chip -------------------------------------------------------------------

const CPU: &str = include_str!("../data/CADR.netlist");
const BUSINT: &str = include_str!("../data/BUSINT.netlist");
const CADRM: &str = include_str!("../data/CADRM.netlist");
const CADRIO: &str = include_str!("../data/CADRIO.netlist");
const SIMPLETV: &str = include_str!("../data/SIMPLETV.netlist");

/// How many memory boards the netlist machines here have. Fewer than the
/// thirty-two `muir` gives one: the round trip is the same whatever the
/// count, and each board is a netlist to build and a quarter of a megabyte
/// of cells to compare.
const BOARDS: usize = 4;

/// A netlist machine as `cadr --chip` builds one, with the boot PROM in the
/// processor and the boards `--chip` runs by default on the backplane ---
/// the memory, the I/O board and the display as netlists, the disk
/// controller as the machine's model. `press` is the boot button: a run
/// from power-on presses it, and a resume does not, because a checkpoint
/// replaces everything the button and the power-on set.
fn chip_machine(press: bool) -> (muir::chip::Chip, Behavioral, muir::cable::FarEnd) {
    use muir::netlist;
    use muir::part::Level;
    let n = netlist::parse(CPU).unwrap();
    let bus_n = netlist::parse(BUSINT).unwrap();
    let mem_n = netlist::parse(CADRM).unwrap();
    let io_n = netlist::parse(CADRIO).unwrap();
    let tv_n = netlist::parse(SIMPLETV).unwrap();
    let mut c = muir::chip::Chip::new(&n);
    c.power_on();
    c.load_prom(&n, &muir::prom::boot_prom_image());
    c.settle();
    let mut clk = Behavioral::new();
    let boards = muir::cable::Boards {
        memory: BOARDS,
        io: Some(&io_n),
        tv: Some(&tv_n),
        ..Default::default()
    };
    let mut far = muir::cable::FarEnd::new(
        &n,
        &bus_n,
        &mem_n,
        boards,
        0,
        Machine::with_memory_boards(BOARDS),
    );
    far.join(&mut c, muir::clock::Clock::time_ns(&clk));
    if press {
        // The button, as `muir` presses it: `-BOOT1` held down, the board
        // settled with it down, and twenty master clocks before it rises.
        let boot = n.by_name_id("-BOOT1").unwrap();
        c.set_net(boot, Level::Low);
        c.settle();
        for _ in 0..20 {
            c.tick(&mut clk);
        }
        c.set_net(boot, Level::High);
    }
    (c, clk, far)
}

/// Runs `at_least` microcycles and then on to the first point a checkpoint
/// may be taken at: the interface between cycles and no memory request up.
/// Returns how many microcycles that took.  A tap in flight is not asked
/// about --- it is saved; see [`chip_checkpoints_a_busy_machine`].
fn run_to_quiet(
    c: &mut muir::chip::Chip,
    clk: &mut Behavioral,
    far: &mut muir::cable::FarEnd,
    at_least: u64,
) -> u64 {
    use muir::clock::Clock;
    let n = muir::netlist::parse(CPU).unwrap();
    let memrq = n.by_name_id("MEMRQ").unwrap();
    let mut ran = 0;
    let mut last = clk.phase_ns();
    loop {
        far.tick_with(c, clk);
        let p = clk.phase_ns();
        let wrapped = p < last;
        last = p;
        if !wrapped {
            continue;
        }
        ran += 1;
        if ran >= at_least && far.quiet() && c.net(memrq) != muir::part::Level::High {
            return ran;
        }
        assert!(ran < at_least + 1000, "no quiet microcycle within a thousand of {at_least}");
    }
}

/// A netlist machine's whole state as a checkpoint holds it, in the four
/// pieces it is written in, each named: a difference is reported as the
/// piece it is in and how far into it, because the whole is megabytes of
/// nets and cells.
fn chip_pieces(
    c: &muir::chip::Chip,
    clk: &Behavioral,
    far: &muir::cable::FarEnd,
) -> Vec<(&'static str, Vec<u8>)> {
    let piece = |f: &dyn Fn(&mut Writer)| {
        let mut w = Writer::new();
        f(&mut w);
        w.finish()
    };
    vec![
        ("the processor", piece(&|w| c.save(w).unwrap())),
        ("the clock", piece(&|w| clk.save(w).unwrap())),
        ("the boards", piece(&|w| far.save(w).unwrap())),
        ("what is behind the buses", piece(&|w| far.buses.save(w))),
    ]
}

/// The pieces run together, which is what a checkpoint's body is.
fn chip_body(c: &muir::chip::Chip, clk: &Behavioral, far: &muir::cable::FarEnd) -> Vec<u8> {
    let mut w = Writer::new();
    c.save(&mut w).unwrap();
    clk.save(&mut w).unwrap();
    far.checkpoint(&mut w).expect("the boards this machine has are all in a checkpoint");
    w.finish()
}

/// The two machines' states piece by piece, saying which piece differs and
/// where rather than printing megabytes of them.
fn same_state(
    what: &str,
    a: (&muir::chip::Chip, &Behavioral, &muir::cable::FarEnd),
    b: (&muir::chip::Chip, &Behavioral, &muir::cable::FarEnd),
) {
    for ((name, x), (_, y)) in chip_pieces(a.0, a.1, a.2).iter().zip(chip_pieces(b.0, b.1, b.2)) {
        assert_eq!(x.len(), y.len(), "{what}: {name} is a different length");
        if let Some(at) = x.iter().zip(&y).position(|(p, q)| p != q) {
            let end = (at + 16).min(x.len());
            panic!(
                "{what}: {name} differs at byte {at} of {}: {:?} against {:?}",
                x.len(),
                &x[at..end],
                &y[at..end]
            );
        }
    }
}

/// **`chip` picks up where the checkpoint left off.** The netlist machine
/// is not an [`Engine`] and its state is not arrays: the processor's
/// scratchpads and control store are the RAM chips' own cells, and the
/// rest is every net's level, every part's bits, and the oscillators and
/// one-shots of five boards mid-pulse. So this is the same check
/// [`resumes`] makes of the other two engines, made of the pieces `muir
/// --chip` runs: saved at a quiet microcycle in the boot PROM, loaded onto
/// a machine built and not booted, and the two the same board a thousand
/// microcycles later.
///
/// The boot PROM alone, so nothing here needs `vendor/`.
#[test]
fn chip_picks_up_where_the_checkpoint_left_off() {
    use muir::clock::Clock;
    let (mut c, mut clk, mut far) = chip_machine(true);
    let at = run_to_quiet(&mut c, &mut clk, &mut far, 400);
    let body = chip_body(&c, &clk, &far);

    let (mut c2, _, mut far2) = chip_machine(false);
    let mut r = Reader::new(&body);
    c2.load(&mut r).unwrap();
    let mut clk2 = Behavioral::load(&mut r).unwrap();
    far2.resume(&mut r).unwrap();
    r.done().unwrap();
    same_state("the checkpoint loads and saves as itself", (&c, &clk, &far), (&c2, &clk2, &far2));
    assert_eq!(chip_body(&c2, &clk2, &far2), body, "the checkpoint loads and saves as itself");
    // The cables joined, as a resume joins them: each board holds what
    // the others are driving onto it, and that is the checkpoint's, so
    // this carries nothing and moves no board.
    far2.join(&mut c2, clk2.time_ns());

    // The same board, microcycle for microcycle, a thousand on. The PC is
    // read off the nets: there is no `Engine::pc` here.
    let n = muir::netlist::parse(CPU).unwrap();
    let pc_nets = c.bus_nets(&n, "PC", 14);
    let mut last = (clk.phase_ns(), clk2.phase_ns());
    let mut ran = 0;
    while ran < 1000 {
        far.tick_with(&mut c, &mut clk);
        far2.tick_with(&mut c2, &mut clk2);
        let p = (clk.phase_ns(), clk2.phase_ns());
        if p.0 < last.0 {
            ran += 1;
            assert_eq!(
                c2.read(&pc_nets),
                c.read(&pc_nets),
                "the PC {ran} microcycles past the checkpoint at {at}"
            );
        }
        assert_eq!(p, (p.0, p.0), "the two clocks in step at microcycle {ran}");
        last = p;
    }
    same_state("the same state 1000 microcycles on", (&c, &clk, &far), (&c2, &clk2, &far2));
}

/// One microcycle of a netlist machine: transitions until the clock's
/// phase wraps, which is what a microcycle is here.
fn one_microcycle(c: &mut muir::chip::Chip, clk: &mut Behavioral, far: &mut muir::cable::FarEnd) {
    use muir::clock::Clock;
    let mut last = clk.phase_ns();
    loop {
        far.tick_with(c, clk);
        let p = clk.phase_ns();
        if p < last {
            return;
        }
        last = p;
    }
}

/// **A checkpoint may be taken while the machine is busy.**
///
/// Up to format 20 it could not.  The boards' delay lines carry
/// transitions between an input and its taps, the format had no field for
/// one in flight, and [`muir::chip::Chip::load`] cleared whatever the
/// board it loaded onto had --- so a checkpoint had to be taken at a
/// microcycle with none, and a machine polling a device register every few
/// microcycles may never present one.  That is issue 89, met on a `chip`
/// run held after two and a half hours, which is exactly when a
/// checkpoint is worth having.
///
/// So this takes a checkpoint at boundaries whatever is in flight there,
/// and holds each to the standard a quiet one is held to by
/// [`chip_picks_up_where_the_checkpoint_left_off`]: it loads and saves as
/// itself, and the machine loaded from it is the machine that was never
/// stopped.  The boundaries are counted by what was in flight at each and
/// the count is printed and asserted, because a run of them that happened
/// to have nothing in flight would pass for the wrong reason.
///
/// **What it does not reach is a bus cycle.** Every boundary sampled here
/// has `INT BUSY` low and `MEMRQ` low --- the boot PROM's own reads hold
/// the processor's clock while they are outstanding, so the machine has
/// no microcycle boundary inside one --- which is why
/// [`muir::cable::FarEnd::quiet`] still asks about the cycle and this
/// says nothing about whether it needs to.  The count is printed so that
/// a window that did reach one is not mistaken for this.
///
/// The boot PROM alone, so nothing here needs `vendor/`.
#[test]
fn chip_checkpoints_a_busy_machine() {
    use muir::clock::Clock;
    let (mut c, mut clk, mut far) = chip_machine(true);
    let n = muir::netlist::parse(CPU).unwrap();
    let memrq = n.by_name_id("MEMRQ").unwrap();
    let pc_nets = c.bus_nets(&n, "PC", 14);
    // Into the PROM's own work, where the boards are exchanging.
    for _ in 0..400 {
        one_microcycle(&mut c, &mut clk, &mut far);
    }
    // The machine loaded into, built once and loaded into again at each
    // boundary: a load replaces everything a fresh board holds, which is
    // what `chip_picks_up_where_the_checkpoint_left_off` shows.
    let (mut c2, _, mut far2) = chip_machine(false);
    let (mut busy, mut with_taps) = (0, 0);
    const ON: u64 = 60;
    for boundary in 0..24u64 {
        one_microcycle(&mut c, &mut clk, &mut far);
        // What the quiet rule asks, so that the test can say what it took
        // a checkpoint across rather than hope.
        with_taps += u64::from(
            c.taps_pending()
                || far.board.taps_pending()
                || far.xbus.taps_pending()
                || far.unibus.as_ref().is_some_and(|u| u.taps_pending()),
        );
        busy += u64::from(!far.quiet() || c.net(memrq) == muir::part::Level::High);

        let body = chip_body(&c, &clk, &far);
        let mut r = Reader::new(&body);
        c2.load(&mut r).unwrap();
        let mut clk2 = Behavioral::load(&mut r).unwrap();
        far2.resume(&mut r).unwrap();
        r.done().unwrap();
        same_state(
            &format!("the checkpoint at boundary {boundary} loads and saves as itself"),
            (&c, &clk, &far),
            (&c2, &clk2, &far2),
        );
        far2.join(&mut c2, clk2.time_ns());

        // And the machine loaded from it runs on as the one that was
        // never stopped, for many times the longest line on any board.
        // The two run in step, so the next boundary sampled is `ON` on
        // from this one.
        for on in 1..=ON {
            one_microcycle(&mut c, &mut clk, &mut far);
            one_microcycle(&mut c2, &mut clk2, &mut far2);
            assert_eq!(
                c2.read(&pc_nets),
                c.read(&pc_nets),
                "the PC {on} microcycles past the checkpoint at boundary {boundary}"
            );
        }
        same_state(
            &format!("the same state {ON} microcycles past boundary {boundary}"),
            (&c, &clk, &far),
            (&c2, &clk2, &far2),
        );
    }
    eprintln!(
        "24 boundaries: {with_taps} with a tap in flight, {busy} with a bus cycle or a \
         memory request"
    );
    assert!(with_taps > 0, "no boundary here had a tap in flight, so this proved nothing");
}

/// **A `micro` checkpoint taken between a write and the write phase that
/// lands it keeps the write.** A push to the PDL buffer and one to the SPC
/// stack, checkpointed in the microcycle after each, before either lands;
/// the resumed engine lands them, reads the stale words the board reads,
/// and ends where an engine run straight through ends.
#[test]
fn micro_carries_its_late_writes_across_a_checkpoint() {
    use muir::isa::Insn;
    use muir::isa::asm::{ALU, SETM, SETO, a_dest, filler, src};
    let fdest = |d: u64| (d << 19) | (0o37 << 14);
    let mut prom = vec![filler(); 10];
    prom.extend([
        Insn::new(ALU | SETO | fdest(0o15)),
        Insn::new(ALU | SETO | fdest(0o11)),
        Insn::new(ALU | SETM | src(0o25) | a_dest(0o201)),
        Insn::new(ALU | SETM | src(0o1) | a_dest(0o202)),
        Insn::new(ALU | SETM | src(0o25) | a_dest(0o203)),
        Insn::new(ALU | SETM | src(0o6) | a_dest(0o204)),
    ]);
    prom.resize(512, filler());
    let make = || {
        let mut m = Machine::new();
        m.load_prom(&prom);
        let mut e = Micro::new(m);
        e.boot();
        e
    };
    let mut straight = make();
    // Up to the microcycle that has just executed the PDL push.
    while straight.executed() != Some(11) {
        straight.step().unwrap();
    }
    let mut w = Writer::new();
    straight.save(&mut w);
    let body = w.finish();
    let mut resumed = make();
    let mut r = Reader::new(&body);
    resumed.load(&mut r).unwrap();
    r.done().unwrap();
    for _ in 0..20 {
        straight.step().unwrap();
        resumed.step().unwrap();
    }
    let got = |e: &Micro| {
        let m = e.machine();
        (m.amem[0o201], m.amem[0o202], m.amem[0o203], m.amem[0o204])
    };
    assert_eq!(got(&resumed), got(&straight));
    assert_eq!(got(&straight).0, 0, "the stale PDL word");
    assert_eq!(got(&straight).2, 0xffff_ffff, "the PDL word once landed");
}

/// **A `micro` checkpoint taken between a write's start and the edge it
/// goes out on keeps the write.** `MD` <- 1111, a write at word 1000, and
/// `MD` <- 2222 in the microcycle after it, checkpointed after the start:
/// the resumed engine writes 2222, the `MD` of the microcycle after the
/// start, as the one run straight through does
/// (`the_engines_write_the_md_of_the_microcycle_after_the_start`,
/// `tests/chip.rs`).
#[test]
fn micro_carries_a_write_not_yet_gone_out_across_a_checkpoint() {
    use muir::isa::Insn;
    use muir::isa::asm::{ALU, MD, SETM, START_WRITE, filler, m_src};
    let mut prom = vec![filler(); 10];
    prom.extend([
        Insn::new(ALU | SETM | m_src(1) | MD),
        Insn::new(ALU | SETM | m_src(2) | START_WRITE),
        Insn::new(ALU | SETM | m_src(3) | MD),
    ]);
    prom.resize(512, filler());
    let make = || {
        let mut m = Machine::new();
        m.load_prom(&prom);
        m.l2_map[0] = (1 << 23) | (1 << 22) | 1;
        m.mmem[1] = 0o1111;
        m.mmem[2] = 0o10;
        m.mmem[3] = 0o2222;
        let mut e = Micro::new(m);
        e.boot();
        e
    };
    let mut straight = make();
    while straight.executed() != Some(11) {
        straight.step().unwrap();
    }
    let mut w = Writer::new();
    straight.save(&mut w);
    let body = w.finish();
    let mut resumed = make();
    let mut r = Reader::new(&body);
    resumed.load(&mut r).unwrap();
    r.done().unwrap();
    assert_eq!(resumed.machine().main[0o410], 0, "not written at the checkpoint");
    for _ in 0..10 {
        straight.step().unwrap();
        resumed.step().unwrap();
    }
    assert_eq!(straight.machine().main[0o410], 0o2222, "run straight through");
    assert_eq!(resumed.machine().main[0o410], 0o2222, "resumed");
}
