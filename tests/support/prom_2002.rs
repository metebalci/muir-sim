// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! PROM 2002, QUUX's built-in boot PROM, run from the boot on a disk: to
//! the microcode's location 6, with what it stored and moved on the way,
//! or to one of its halts. `tests/quux_prom_saves_nothing.rs`,
//! `tests/quux_prom_halts.rs` and `tests/system_2002.rs` use it.

use std::path::Path;

use muir::block_disk::{BLOCK_NS, BlockDisk, Transfer};
use muir::engine::Engine;
use muir::machine::{Geometry, Machine, Word};
use muir::micro::Micro;
use muir::pipeline::Pipeline;

/// Physical page 3 of 1024 words, the PROM's buffer and the microcode's
/// symbol area.
pub const PAGE_3: std::ops::RangeInclusive<u32> = 0o6000..=0o7777;

/// The word the PROM's disk transfers take their command list from.
pub const CCW: u32 = 0o2377;

/// Where the PROM jumps when it is done: the microcode's location 6.
const LOCATION_6: u16 = 6;

/// More than the PROM takes to load a microcode: for microcode 2002, under
/// a million microcycles on `micro` and under two million on `rtl`
/// (measured).
pub const LIMIT: u64 = 20_000_000;

/// QUUX of MACHINE-ID `id` with its built-in PROM and `pack` on
/// block-disk, the bus's stores and the disk's transfers recorded.
pub fn quux(pack: &Path, id: u32) -> Machine {
    let geometry = Geometry { machine_id: Some(id), ..Geometry::QUUX };
    let mut m = Machine::with_geometry(geometry, 32);
    m.load_prom(&muir::prom::quux_boot_prom());
    let mut d = BlockDisk::new(BLOCK_NS);
    d.attach(muir::disk_image::Disk::open_rw(pack).expect("the pack"));
    d.log = Some(Vec::new());
    m.block_disk = Some(d);
    m.store_log = Some(Vec::new());
    m
}

/// MACHINE-ID with hardware revision `r` in `<15:4>`, as
/// `Geometry::QUUX`'s carries 16.
pub fn machine_id(r: u32) -> u32 {
    let id = Geometry::QUUX.machine_id.unwrap();
    id & !(0o7777 << 4) | r << 4
}

/// What the boot did before the microcode's location 6 ran.
pub struct Run {
    pub microcycles: u64,
    pub stores: Vec<u32>,
    pub transfers: Vec<Transfer>,
    /// Physical memory 6000-7777 at 6.
    pub page: Vec<Word>,
}

/// Boots `e` and steps it until the microinstruction at 6 has run and the
/// machine has gone on in the microcode, and returns what was done before
/// the step that ran it. The PC alone does not say when the PROM is done:
/// on `rtl` the PC passes 6 while the PROM clears the control store
/// (`CLEAR-I-MEMORY`), a control store write taking its address through
/// the PC, and the next microcycle is back in the PROM. After the PROM's
/// jump to 6 the next one is below 36000.
pub fn to_six<E: Engine>(mut e: E, name: &str) -> Run {
    e.boot();
    let mut n = 0u64;
    loop {
        let m = e.machine();
        let stores = m.store_log.as_ref().unwrap().len();
        let transfers = m.block_disk.as_ref().unwrap().log.as_ref().unwrap().len();
        e.step().unwrap();
        if e.machine().opc == LOCATION_6 && e.pc() < 0o36000 {
            let m = e.machine_mut();
            let mut run = Run {
                microcycles: n,
                stores: m.store_log.take().unwrap(),
                transfers: m.block_disk.as_mut().unwrap().log.take().unwrap(),
                page: m.main[*PAGE_3.start() as usize..=*PAGE_3.end() as usize].to_vec(),
            };
            run.stores.truncate(stores);
            run.transfers.truncate(transfers);
            return run;
        }
        n += 1;
        assert!(n < LIMIT, "{name}: the PC is at {:o} after {n} microcycles, not at 6", e.pc());
    }
}

/// Whether the files at `a` and `b` hold the same bytes, read a megabyte
/// at a time: a release disk is too large to hold twice.
pub fn same_bytes(a: &Path, b: &Path) -> bool {
    use std::io::Read;
    let (mut a, mut b) = (std::fs::File::open(a).unwrap(), std::fs::File::open(b).unwrap());
    let (mut x, mut y) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let n = a.read(&mut x).unwrap();
        let mut m = 0;
        while m < n {
            let k = b.read(&mut y[m..n]).unwrap();
            if k == 0 {
                return false;
            }
            m += k;
        }
        if n == 0 {
            return b.read(&mut y[..1]).unwrap() == 0;
        }
        if x[..n] != y[..n] {
            return false;
        }
    }
}

/// Addresses in octal, for a failure message.
fn octal(a: &[u32]) -> String {
    a.iter().map(|a| format!("{a:o}")).collect::<Vec<_>>().join(" ")
}

/// Boots `pack` on both engines, each on a fresh copy of it in `dir`, and
/// holds the run to the PROM's promise (contract Q8): no block written, the
/// disk file unchanged byte for byte, main memory stored only in page 3
/// and word 2377, every block read into page 3, and page 3 at 6 holding
/// `mcr`'s symbol area, which the PROM loads last. `ext` names the copy for
/// what it is; muir tells a VHD by its footer, not its name.
pub fn saves_nothing(pack: &Path, ext: &str, mcr: &[u8], dir: &Path) {
    let parsed = muir::mcr::parse_quux(mcr, muir::mcr::Holds::Microcode).unwrap();
    let (start, symbols) = parsed.symbol_area.expect("a symbol area");
    assert_eq!((start, symbols.len()), (0o6000, 1024), "the symbol area is page 3");
    let id = Geometry::QUUX.machine_id.unwrap();
    for name in ["micro", "rtl"] {
        let copy = dir.join(format!("{name}.{ext}"));
        std::fs::copy(pack, &copy).unwrap();
        let run = match name {
            "micro" => to_six(Micro::new(quux(&copy, id)), name),
            _ => to_six(Pipeline::new(quux(&copy, id)), name),
        };
        let writes = run.transfers.iter().filter(|t| t.write).count();
        let reads: Vec<&Transfer> = run.transfers.iter().filter(|t| !t.write).collect();
        let stray_stores: Vec<u32> =
            run.stores.iter().copied().filter(|a| *a != CCW && !PAGE_3.contains(a)).collect();
        // A block is read into a page, 1024 words written from it on.
        let stray_reads: Vec<u32> = reads
            .iter()
            .filter(|t| !(PAGE_3.contains(&t.page) && PAGE_3.contains(&(t.page + 1023))))
            .map(|t| t.page)
            .collect();
        eprintln!(
            "{name}: at 6 after {} microcycles; {} blocks read, {writes} written; \
             {} stores, {} outside page 3 and word 2377; {} block reads outside page 3",
            run.microcycles,
            reads.len(),
            run.stores.len(),
            stray_stores.len(),
            stray_reads.len(),
        );
        assert!(!reads.is_empty() && !run.stores.is_empty(), "{name}: the record is kept");
        assert_eq!(
            writes,
            0,
            "{name}: blocks written: {:?}",
            run.transfers.iter().filter(|t| t.write).collect::<Vec<_>>()
        );
        assert!(stray_stores.is_empty(), "{name}: stored at {}", octal(&stray_stores));
        assert!(stray_reads.is_empty(), "{name}: blocks read into {}", octal(&stray_reads));
        assert!(same_bytes(&copy, pack), "{name}: the disk file unchanged");
        assert!(
            run.page == symbols,
            "{name}: page 3 holds the microcode's symbol area, loaded last"
        );
        std::fs::remove_file(&copy).unwrap();
    }
}

/// Where a run is at one of the PROM's halts: its PC, and whether the
/// machine stopped itself or runs on in place.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Halt {
    pub pc: u16,
    /// `ERR` up under `ERRSTOP`, the clock stopped. A halt before the PROM
    /// sets error stop through the register page raises `ERR` alike, and
    /// the clock runs on, the halt's jump to itself holding the PC there.
    pub stopped: bool,
}

/// Boots `e` and runs it until it is at a halt, `HALT-CONS` having raised
/// `ERR` with `RUN` still set, or `limit` microcycles pass: `None` then.
/// A stopped machine's `step` goes on returning `Ok`, so `FLAG-1` is what
/// says it. Under `ERRSTOP` the machine stops itself, and the PC is read
/// once the pipeline has drained and the PC stands still; without it the
/// PC is held at the halt by its jump to itself --- on the pipeline the
/// fetch's PC, the halt and the word after it, the word after while the
/// jump is in flight. Either is held for a thousand microcycles.
pub fn halt<E: Engine>(e: &mut E, limit: u64) -> Option<Halt> {
    e.boot();
    for _ in 0..limit {
        e.step().unwrap();
        let f = muir::spy::Flag1::of(e.spy_read(muir::spy::FLAG_1));
        if f.srun && f.err {
            let stopped = e.machine().mode.errstop;
            (0..100).for_each(|_| e.step().unwrap());
            let mut seen = std::collections::BTreeSet::new();
            for _ in 0..1000 {
                e.step().unwrap();
                seen.insert(e.pc());
            }
            let pc = *seen.first().unwrap();
            let held = if stopped { vec![pc] } else { vec![pc, pc + 1] };
            assert!(seen.iter().all(|p| held.contains(p)), "at the halt, the PC held: {seen:?}");
            return Some(Halt { pc, stopped });
        }
    }
    None
}
