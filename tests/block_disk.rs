// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's block-disk, `--disk-controller block-disk`: the CADR disk
//! controller's four registers and command list, with a linear block
//! number for the disk address, read and write the only commands, and a
//! fixed time a block. These take the 4-byte transfer, 4 blocks a page;
//! `tests/revision_13_memory.rs` holds the packed one.

use muir::block_disk::{self, BlockDisk};
use muir::disk_image::Disk;
use muir::disk_unit::{BLOCK_WORDS, Geometry};
use muir::machine::{Geometry as MachineGeometry, Machine, UNBOXED_TAG, Word, bus_error};

/// A blank disk of a T-300's blocks with block `k` holding `k << 16 |
/// word`.
fn pack() -> Disk {
    let mut d = Disk::blank(Geometry::T300.blocks());
    for k in 0..16u32 {
        let block: [u32; BLOCK_WORDS] = std::array::from_fn(|w| k << 16 | w as u32);
        assert!(d.write_block(k, &block));
    }
    d
}

/// A disk with `pack` and the command list at 100: pages 2000 and 4000,
/// the first with More set.
fn disk_and_memory() -> (BlockDisk, Vec<Word>) {
    let mut d = BlockDisk::new(block_disk::BLOCK_NS);
    d.attach(pack());
    let mut main = vec![0; 1 << 16];
    main[0o100] = 0o2000 | 1;
    main[0o101] = 0o4000;
    (d, main)
}

const READ: u32 = 0;
const WRITE: u32 = 0o11;
const FOUR_BYTE: u32 = 1 << 12;
const DONE_INTERRUPT: u32 = 1 << 11;

/// **A read moves the blocks the list names into memory**, 4 a page, the
/// disk address counting up from the one given and left at the last block
/// moved, and the controller is busy for a block's time each.
#[test]
fn a_read_moves_the_listed_blocks() {
    let (mut d, mut main) = disk_and_memory();
    d.advance(0);
    d.write(block_disk::CLP, 0o100, &mut main);
    d.write(block_disk::DA, 3, &mut main);
    d.write(block_disk::COMMAND, READ | FOUR_BYTE | DONE_INTERRUPT, &mut main);
    d.write(block_disk::START, 0, &mut main);
    let word = |k: u32, w: u32| UNBOXED_TAG | Word::from(k << 16 | w);
    assert_eq!(main[0o2000], word(3, 0));
    assert_eq!(main[0o2377], word(3, 0o377));
    assert_eq!(main[0o2400], word(4, 0));
    assert_eq!(main[0o3777], word(6, 0o377));
    assert_eq!(main[0o4000 + 5], word(7, 5));
    assert_eq!(d.read(block_disk::DA), 10, "the last block moved");
    assert_eq!(d.read(block_disk::STATUS) & 1, 0, "busy");
    assert!(!d.interrupt());
    d.advance(8 * block_disk::BLOCK_NS - 1);
    assert_eq!(d.read(block_disk::STATUS) & 1, 0, "busy until eight blocks' time");
    d.advance(8 * block_disk::BLOCK_NS);
    let status = d.read(block_disk::STATUS);
    assert_eq!(status & 1, 1, "not active");
    assert_eq!(status & (1 << 13), 0, "no error");
    assert!(d.interrupt(), "the done interrupt, enabled");
    assert_ne!(status & (1 << 3), 0, "and the status says so");
}

/// **A write moves memory's pages to the blocks**, which read back.
#[test]
fn a_write_moves_the_pages_to_the_blocks() {
    let (mut d, mut main) = disk_and_memory();
    for (k, w) in main[0o2000..0o6000].iter_mut().enumerate() {
        *w = UNBOXED_TAG | (0o7000000 + k as Word);
    }
    d.write(block_disk::CLP, 0o100, &mut main);
    d.write(block_disk::DA, 100, &mut main);
    d.write(block_disk::COMMAND, WRITE | FOUR_BYTE, &mut main);
    d.write(block_disk::START, 0, &mut main);
    let u = d.disk_mut().unwrap();
    assert_eq!(u.read_block(100).unwrap()[7], 0o7000007);
    assert_eq!(u.read_block(101).unwrap()[0], 0o7000400);
}

/// **Past the end of the pack a transfer stops by error**: the blocks
/// before the end move, and the status has `<17>`, as the CADR's does for
/// "an attempt ... to continue a read or write operation past the end of
/// the disk", and `<13>`, transfer aborted.
#[test]
fn past_the_end_stops_by_error() {
    let (mut d, mut main) = disk_and_memory();
    let last = Geometry::T300.blocks() - 1;
    d.write(block_disk::CLP, 0o100, &mut main);
    d.write(block_disk::DA, last, &mut main);
    d.write(block_disk::COMMAND, READ | FOUR_BYTE, &mut main);
    d.write(block_disk::START, 0, &mut main);
    d.advance(u64::MAX / 2);
    let status = d.read(block_disk::STATUS);
    assert_ne!(status & (1 << 17), 0, "past the end");
    assert_ne!(status & (1 << 13), 0, "stopped by error");
    assert_eq!(d.read(block_disk::DA), last + 1, "at the block that failed");
}

/// **A command list outside memory is the NXM error**, `<20>`.
#[test]
fn a_list_outside_memory_is_nxm() {
    let (mut d, mut main) = disk_and_memory();
    d.write(block_disk::CLP, 1 << 20, &mut main);
    d.write(block_disk::COMMAND, READ | FOUR_BYTE, &mut main);
    d.write(block_disk::START, 0, &mut main);
    d.advance(u64::MAX / 2);
    let status = d.read(block_disk::STATUS);
    assert_ne!(status & (1 << 20), 0, "NXM");
    assert_ne!(status & (1 << 13), 0, "stopped by error");
}

/// **Any other command stops by error**, moving nothing: the CADR's seek,
/// at ease, Read All and the rest have no drive to act on.
#[test]
fn any_other_command_stops_by_error() {
    let (mut d, mut main) = disk_and_memory();
    d.write(block_disk::CLP, 0o100, &mut main);
    d.write(block_disk::COMMAND, 0o04, &mut main);
    d.write(block_disk::START, 0, &mut main);
    assert_ne!(d.read(block_disk::STATUS) & (1 << 13), 0);
    assert_eq!(main[0o2000], 0, "nothing moved");
}

/// **On QUUX the registers are block-disk's, at words 200-203 of the
/// register page**, `1777777600` (contract Q13): 200 status and command,
/// 201 the last memory address and the command list pointer, 202 the disk
/// address, 203 START. A read through them lands the page, and nothing
/// fails; the CADR controller's place, `17377774`, is nothing there on a
/// machine of 2MW, past its main memory.
#[test]
fn on_quux_the_registers_are_the_block_disk_s() {
    const REGS: u32 = 0o1777777600;
    assert_eq!(block_disk::REGS, REGS);
    let mut m = Machine::with_geometry(MachineGeometry::QUUX, 32);
    let mut d = BlockDisk::new(block_disk::BLOCK_NS);
    d.attach(pack());
    m.block_disk = Some(d);
    m.main[0o777] = 0o2000;
    m.bus_write(REGS + 1, 0o777);
    m.bus_write(REGS + 2, 2);
    m.bus_write(REGS, (READ | FOUR_BYTE).into());
    m.bus_write(REGS + 3, 0);
    assert_eq!(m.main[0o2000 + 9], UNBOXED_TAG | 2 << 16 | 9);
    assert_eq!(m.bus_read(REGS + 2), 5, "the disk address");
    assert_eq!(m.bus_read(REGS + 1), 0o3777, "the last memory address");
    assert_eq!(m.bus_read(REGS + 3), 0, "START reads 0");
    assert_eq!(m.bus_error & bus_error::XBUS_NXM, 0);
    assert!(m.dma_written, "the cache is told memory was written");
    m.ns += 4 * block_disk::BLOCK_NS;
    assert_eq!(m.bus_read(REGS) & 1, 1, "not active");
    for old in 0o17377774..=0o17377777 {
        m.bus_error = 0;
        assert_eq!(m.bus_read(old), 0, "{old:o}");
        assert_eq!(m.bus_error, bus_error::XBUS_NXM, "{old:o}: nothing there");
    }
}

/// **A checkpoint keeps it**: registers, the transfer's end, and the blocks
/// written.
#[test]
fn a_checkpoint_keeps_it() {
    use muir::checkpoint::{Reader, Writer};
    let (mut d, mut main) = disk_and_memory();
    d.write(block_disk::CLP, 0o100, &mut main);
    d.write(block_disk::DA, 50, &mut main);
    d.write(block_disk::COMMAND, WRITE | FOUR_BYTE, &mut main);
    d.write(block_disk::START, 0, &mut main);
    let mut w = Writer::new();
    d.save(&mut w);
    let body = w.finish();
    let mut back = BlockDisk::new(block_disk::BLOCK_NS);
    back.attach(Disk::blank(Geometry::T300.blocks()));
    back.load(&mut Reader::new(&body)).unwrap();
    for status_at in [0, 8 * block_disk::BLOCK_NS] {
        d.advance(status_at);
        back.advance(status_at);
        assert_eq!(back.read(block_disk::STATUS), d.read(block_disk::STATUS));
    }
    assert_eq!(back.read(block_disk::DA), 57);
}
