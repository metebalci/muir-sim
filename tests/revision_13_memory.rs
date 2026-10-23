// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **QUUX's memory and devices at 40 bits** (contract G1 §3-§4, G2 §3-§4,
//! appendix A1.10-A1.13), through the machine's register page and its
//! parts, on revision 16: packed storage, block-disk's packed and 4-byte
//! transfers, the file device's tags and 28-bit addresses, and the
//! checkpoint at 40 bits.

use std::path::PathBuf;

use muir::block_disk::BlockDisk;
use muir::checkpoint::Writer;
use muir::disk_image::Disk;
use muir::engine::Engine;
use muir::file_device::{Mounts, op};
use muir::machine::{Geometry, Machine, Word};
use muir::micro::Micro;

mod support;

/// The register page's bus address.
const PAGE: u32 = support::PAGE_16;
/// A 40-bit word from its tag and field.
const fn w(tag: u64, field: u64) -> Word {
    tag << 32 | (field & 0xffff_ffff)
}
/// The unboxed tag a 4-byte read and the file device write (G1 §2.6).
const FIX: Word = 0o005 << 32;

fn data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data").join(name)
}

/// A directory of this test's own, removed afterwards.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let p = std::env::temp_dir().join(format!("muir-rev13-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Scratch(p)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Revision 16 with `words` of main memory.
fn rev16(words: usize) -> Machine {
    let mut m = support::quux_16();
    m.main = vec![0; words];
    m
}

/// G1 §4.1's packed storage of `words`, by hand: each word five bytes,
/// `<7:0>` first and the tag `<39:32>` last.
fn packed(words: &[Word]) -> Vec<u8> {
    let mut b = Vec::new();
    for &x in words {
        for k in 0..5 {
            b.push((x >> (8 * k)) as u8);
        }
    }
    b
}

/// A page's words, each different in every byte, the tag included.
fn pattern(seed: u64) -> Vec<Word> {
    (0..1024u64)
        .map(|k| {
            let v = (seed + k).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 20;
            v & ((1 << 40) - 1)
        })
        .collect()
}

/// **The CADR's device addresses are nothing on QUUX** when main memory
/// does not reach them (G1 §3.2): its last page, `17777400`, and its TV's
/// buffer, `17000000`, fail with word 101's NXM bit; the register page
/// answers.
#[test]
fn the_cadr_s_device_addresses_are_nothing_on_revision_13() {
    let mut m = rev16(2 << 20);
    for phys in [0o17777400, 0o17777500, 0o17000000, 2 << 20] {
        m.bus_error = 0;
        assert_eq!(m.bus_read(phys), 0, "{phys:o}");
        assert_eq!(m.bus_read(PAGE + 0o101), 1, "{phys:o}: NXM");
    }
    let id = Geometry::QUUX.machine_id.unwrap();
    assert_eq!(m.bus_read(PAGE), id.into(), "the page's MACHINE-ID");
}

// --- block-disk ---------------------------------------------------------------

/// Block-disk's registers on the register page (A1.10).
const CMD: u32 = PAGE + 0o200;
const CLP: u32 = PAGE + 0o201;
const DA: u32 = PAGE + 0o202;
const START: u32 = PAGE + 0o203;
const READ: u32 = 0;
const WRITE: u32 = 0o11;
/// Command `<12>`: the 4-byte transfer (A1.11).
const FOUR_BYTE: u32 = 1 << 12;

/// A raw disk of `blocks` 1 KiB blocks of zeros in `dir`, attached to `m`'s
/// block-disk; its path.
fn attach_disk(m: &mut Machine, dir: &Scratch, blocks: usize) -> PathBuf {
    let path = dir.path("disk.img");
    std::fs::write(&path, vec![0u8; blocks * 1024]).unwrap();
    let mut d = BlockDisk::new(muir::block_disk::BLOCK_NS);
    d.attach(Disk::open_rw(&path).unwrap());
    m.block_disk = Some(d);
    path
}

/// A transfer: `cmd` over the command list at `clp` from block `da`, run
/// to done. The status, word 201 and the disk address after it.
fn transfer(m: &mut Machine, cmd: u32, clp: u32, da: u32) -> (u32, u32, u32) {
    m.bus_write(CMD, cmd.into());
    m.bus_write(CLP, clp.into());
    m.bus_write(DA, da.into());
    m.bus_write(START, 0);
    m.ns += m.time_base().per_s();
    let status = m.bus_read(CMD) as u32;
    assert_eq!(status & 1, 1, "not active after a second");
    (status, m.bus_read(CLP) as u32, m.bus_read(DA) as u32)
}

/// **The packed transfer, 5 blocks a page** (G1 §4.3-§4.4, A1.11): a
/// page is written as main memory holds it, 5,120 bytes, each word 5
/// bytes low first, the tag last --- so the words at a line's offsets 1,
/// 3, 4 and 6, which span two beats, are whole on the disk --- and read
/// back the same. An entry names its page by `<27:10>`, `<9:1>` ignored;
/// the disk address is left at the last block moved and word 201 at the
/// last word.
#[test]
fn the_packed_transfer_moves_a_page_as_5_blocks() {
    let dir = Scratch::new("packed");
    let mut m = rev16(0o20004000);
    let path = attach_disk(&mut m, &dir, 64);
    let (p1, p2) = (0o4000usize, 0o20000000usize);
    let (a, b) = (pattern(1), pattern(7));
    m.main[p1..p1 + 1024].copy_from_slice(&a);
    m.main[p2..p2 + 1024].copy_from_slice(&b);
    // The list above 22 bits too: page 1 with More and <9:1> set, then page
    // 2.
    let list = 0o20002000;
    m.main[list] = (p1 as Word) | 0o776 | 1;
    m.main[list + 1] = w(0o377, p2 as u64);
    let (status, last, da) = transfer(&mut m, WRITE, list as u32, 3);
    assert_eq!(status & (1 << 13), 0, "no error: status {status:o}");
    assert_eq!(da, 3 + 10 - 1, "the disk address at the last block moved");
    assert_eq!(last, (p2 + 1023) as u32, "word 201 at the last word moved");
    let file = std::fs::read(&path).unwrap();
    assert_eq!(file[3 * 1024..8 * 1024], packed(&a)[..], "page 1 in blocks 3-7, packed");
    assert_eq!(file[8 * 1024..13 * 1024], packed(&b)[..], "page 2 in blocks 8-12, packed");
    // A word spanning two beats, by hand: line 0's word 1 at bytes 5-9.
    let x = a[1];
    let at = 3 * 1024 + 5;
    assert_eq!(
        file[at..at + 5],
        [x as u8, (x >> 8) as u8, (x >> 16) as u8, (x >> 24) as u8, (x >> 32) as u8]
    );
    // Back into two other pages, tags and all.
    let (q1, q2) = (0o10000usize, 0o12000usize);
    m.main[list] = q1 as Word | 1;
    m.main[list + 1] = q2 as Word;
    let (status, _, _) = transfer(&mut m, READ, list as u32, 3);
    assert_eq!(status & (1 << 13), 0);
    assert_eq!(m.main[q1..q1 + 1024], a[..], "page 1 read back");
    assert_eq!(m.main[q2..q2 + 1024], b[..], "page 2 read back");
}

/// **The 4-byte transfer, 4 blocks a page** (G2 §4.2, A1.11): command
/// `<12>`; word w's `<31:0>` at bytes 4w to 4w + 3, low first; a write
/// drops the tag, a read writes tag `005`.
#[test]
fn the_4_byte_transfer_moves_a_page_as_4_blocks() {
    let dir = Scratch::new("four");
    let mut m = rev16(0o100000);
    let path = attach_disk(&mut m, &dir, 64);
    let a = pattern(3);
    m.main[0o4000..0o6000].copy_from_slice(&a);
    m.main[0o100] = 0o4000;
    let (status, last, da) = transfer(&mut m, WRITE | FOUR_BYTE, 0o100, 5);
    assert_eq!(status & (1 << 13), 0, "status {status:o}");
    assert_eq!(da, 5 + 4 - 1, "the disk address at the last block moved");
    assert_eq!(last, 0o4000 + 1023);
    let file = std::fs::read(&path).unwrap();
    let four: Vec<u8> = a.iter().flat_map(|&x| (x as u32).to_le_bytes()).collect();
    assert_eq!(file[5 * 1024..9 * 1024], four[..], "blocks 5-8, 4 bytes a word");
    assert!(file[9 * 1024..].iter().all(|&x| x == 0), "nothing past block 8");
    m.main[0o100] = 0o6000;
    transfer(&mut m, READ | FOUR_BYTE, 0o100, 5);
    let want: Vec<Word> = a.iter().map(|&x| FIX | (x & 0xffff_ffff)).collect();
    assert_eq!(m.main[0o6000..0o6000 + 1024], want[..], "the field back, tag 005");
}

/// **The GPT fixture through a 4-byte transfer and an 8-bit view** (G2
/// §4.2): the PROM and Lisp read the GPT as 4 bytes a word, byte i in
/// word i/4 at `8(i mod 4)`. Read so, the fixture's first 4 KiB are the
/// file's bytes, `EFI PART` at 512, every word tag `005`. A packed read of
/// the same blocks would put every fifth byte in a tag.
#[test]
fn the_gpt_reads_through_a_4_byte_transfer() {
    let mut m = rev16(0o100000);
    let mut d = BlockDisk::new(muir::block_disk::BLOCK_NS);
    d.attach(Disk::open(data("quux-disk.img")).unwrap());
    m.block_disk = Some(d);
    m.main[0o100] = 0o2000;
    let (status, _, _) = transfer(&mut m, READ | FOUR_BYTE, 0o100, 0);
    assert_eq!(status & (1 << 13), 0);
    let view: Vec<u8> =
        (0..4096).map(|i| (m.main[0o2000 + i / 4] >> (8 * (i % 4))) as u8).collect();
    let file = std::fs::read(data("quux-disk.img")).unwrap();
    assert_eq!(&view[512..520], b"EFI PART", "the header's signature");
    let differ = (0..4096).filter(|&i| view[i] != file[i]).count();
    assert_eq!(differ, 0, "bytes of the 8-bit view unlike the file's");
    assert!(m.main[0o2000..0o3000].iter().all(|&x| x >> 32 == 0o005), "every word 005");
}

/// **A page outside main memory stops the transfer with NXM** (A1.11):
/// `<20>` and stopped by error, word 201 at the entry's address; a page
/// in the frame buffer window is not main memory either.
#[test]
fn a_page_outside_main_memory_is_nxm() {
    let dir = Scratch::new("nxm");
    let mut m = rev16(0o100000);
    attach_disk(&mut m, &dir, 64);
    for page in [0o100000u64, 0o1760000000] {
        m.main[0o100] = page;
        let (status, last, _) = transfer(&mut m, READ, 0o100, 0);
        assert_ne!(status & (1 << 20), 0, "page {page:o}: NXM, status {status:o}");
        assert_ne!(status & (1 << 13), 0, "page {page:o}: stopped by error");
        assert_eq!(last, 0o100, "page {page:o}: word 201 at the entry");
    }
}

/// **The command list and its pages above 24 bits** (A1.10): the command
/// list pointer and word 201 hold 28-bit addresses.
#[test]
fn the_command_list_takes_28_bit_addresses() {
    let dir = Scratch::new("clp28");
    let mut m = rev16(0o100004000);
    attach_disk(&mut m, &dir, 64);
    let (list, page) = (0o100002000usize, 0o100000000usize);
    m.main[list] = page as Word;
    let (status, last, _) = transfer(&mut m, READ | FOUR_BYTE, list as u32, 0);
    assert_eq!(status & (1 << 13), 0, "status {status:o}");
    assert_eq!(last, (page + 1023) as u32, "word 201 at the page's last word");
    assert!(m.main[page..page + 1024].iter().all(|&x| x == FIX), "the page read, tag 005");
}

// --- the file device ------------------------------------------------------------

/// The file device's registers on the register page (A1.10).
const FD_CONTROL: u32 = PAGE + 0o160;
const FD_STATUS: u32 = PAGE + 0o161;
const CMD_BASE: u32 = PAGE + 0o162;
const CMD_SIZE: u32 = PAGE + 0o163;
const CMD_PROD: u32 = PAGE + 0o164;
const RESP_BASE: u32 = PAGE + 0o166;
const RESP_SIZE: u32 = PAGE + 0o167;
const RESP_PROD: u32 = PAGE + 0o170;

/// **The file device at 28 bits** (G1 §4.4, G2 §4.3, A1.10): rings and
/// buffers above 24 bits, on an 8-word line --- a ring on a 4-word line is
/// refused --- the words it fills tagged `005`, its responses too, and a
/// name read from words whose tags are anything.
#[test]
fn the_file_device_takes_28_bit_addresses_and_writes_fixnums() {
    let dir = Scratch::new("files");
    std::fs::write(dir.path("hello"), b"hello, forty bits").unwrap();
    let mut mounts = Mounts::default();
    mounts.add(&dir.0.display().to_string()).unwrap();
    let base = 0o100000000usize;
    let mut m = rev16(base + 0o10000);
    m.file_device.mounts = mounts;
    let (cmd, resp, a, b) = (base, base + 0o100, base + 0o200, base + 0o400);
    // A ring on a 4-word line: refused.
    m.bus_write(CMD_BASE, (cmd + 4) as Word);
    m.bus_write(CMD_SIZE, 1);
    m.bus_write(RESP_BASE, resp as Word);
    m.bus_write(RESP_SIZE, 1);
    m.bus_write(FD_CONTROL, 1);
    assert_eq!(m.bus_read(FD_STATUS) & 0b101, 0b100, "a ring off an 8-word line is refused");
    m.bus_write(CMD_BASE, cmd as Word);
    m.bus_write(FD_CONTROL, 1);
    assert_eq!(m.bus_read(FD_STATUS) & 0b101, 1, "enabled");
    assert_eq!(m.bus_read(CMD_BASE), cmd as Word, "the base reads back, 28 bits");
    // OPEN "/hello" for reading, its name in words tagged 377.
    let name = b"/hello";
    for (k, c) in name.chunks(4).enumerate() {
        let mut x = [0u8; 4];
        x[..c.len()].copy_from_slice(c);
        m.main[a + k] = w(0o377, u32::from_le_bytes(x).into());
    }
    let post = |m: &mut Machine, slot: usize, words: [u64; 8], prod: u64| {
        for (k, x) in words.into_iter().enumerate() {
            m.main[cmd + 8 * slot + k] = x;
        }
        m.bus_write(CMD_PROD, prod);
        let start = m.ns;
        while m.bus_read(RESP_PROD) != prod {
            m.ns += 1_000;
            assert!(m.ns - start < 20_000_000_000, "no response");
        }
    };
    post(&mut m, 0, [1 | (op::OPEN as u64) << 16, 0, a as u64, name.len() as u64, 0, 0, 0, 0], 1);
    let r: Vec<Word> = m.main[resp..resp + 8].to_vec();
    assert_eq!((r[0] >> 16) & 0xff, 0, "OPEN succeeded: {r:?}");
    assert!(r.iter().all(|&x| x >> 32 == 0o005), "the response's words are fixnums: {r:?}");
    let handle = r[2] & 0xffff_ffff;
    post(&mut m, 1, [2 | (op::READ as u64) << 16, handle, 0, 0, b as u64, 64, 0, 0], 2);
    let r: Vec<Word> = m.main[resp + 8..resp + 16].to_vec();
    assert_eq!((r[0] >> 16) & 0xff, 0, "READ succeeded: {r:?}");
    let n = (r[1] & 0xffff_ffff) as usize;
    assert_eq!(n, 17);
    let got: Vec<u8> = (0..n).map(|i| (m.main[b + i / 4] >> (8 * (i % 4))) as u8).collect();
    assert_eq!(got, b"hello, forty bits");
    assert!(m.main[b..b + n.div_ceil(4)].iter().all(|&x| x >> 32 == 0o005), "buffer B tagged 005");
    assert_eq!(m.main[b + n.div_ceil(4)], 0, "and nothing past the bytes");
}

// --- the checkpoint -----------------------------------------------------------

/// Line 307 of main memory, bytes 12,280-12,319 of packed storage, crosses
/// a 4 KiB boundary (G1 §4.1: `40L mod 4096` = 4088).
const LINE_4K: usize = 307 * 8;

/// A QUUX machine on `micro` with words at every offset of line 0 and of
/// [`LINE_4K`], tags and all.
fn with_lines() -> (Micro, Vec<Word>) {
    let mut m = rev16(0o100000);
    let words: Vec<Word> = pattern(11)[..16].to_vec();
    m.main[..8].copy_from_slice(&words[..8]);
    m.main[LINE_4K..LINE_4K + 8].copy_from_slice(&words[8..]);
    let mut e = Micro::new(m);
    e.boot();
    (e, words)
}

/// **A 40-bit machine's checkpoint** (A1.13): main memory in packed storage,
/// the bytes of G1 §4.1 by hand, line 0 and a line across 4 KiB; written to
/// a file as a new version, read back and resumed with every word whole.
#[test]
fn a_40_bit_checkpoint_is_packed_and_round_trips() {
    let (e, words) = with_lines();
    assert_eq!(e.machine().checkpoint_refusal(), None, "a 40-bit machine writes its checkpoint");
    let mut w = Writer::new();
    e.save(&mut w);
    let body = w.finish();
    let find = |needle: &[u8]| body.windows(needle.len()).position(|x| x == needle);
    let line0 = find(&packed(&words[..8])).expect("line 0 packed");
    let line4k = find(&packed(&words[8..])).expect("the 4 KiB line packed");
    assert_eq!(line4k - line0, 5 * LINE_4K, "five bytes a word between them");
    let dir = Scratch::new("ckpt");
    let path = dir.path("m.chk");
    let version = muir::checkpoint::version_for(&e.machine().geometry);
    muir::checkpoint::write_version(&path, "micro", e.machine().memory_boards(), version, &body)
        .unwrap();
    let c = muir::checkpoint::read(&path).unwrap();
    assert_eq!(c.word_bits, 40, "the file says its words are 40 bits");
    assert_eq!(c.version, muir::checkpoint::VERSION_16, "a version of its own");
    let mut back = Micro::new(rev16(0o100000));
    back.load(&mut c.reader()).unwrap();
    assert_eq!(back.machine().main, e.machine().main, "main memory back");
    assert_eq!(back.machine().geometry, Geometry::QUUX);
}

/// **A 32-bit checkpoint, the CADR's, is refused on QUUX** (G2 §2.8), and
/// a 32-bit machine's file keeps its version.
#[test]
fn a_32_bit_checkpoint_is_refused_on_revision_13() {
    let mut e = Micro::new(Machine::new());
    e.boot();
    let mut w = Writer::new();
    e.save(&mut w);
    let body = w.finish();
    let dir = Scratch::new("refuse");
    let path = dir.path("cadr.chk");
    muir::checkpoint::write(&path, "micro", e.machine().memory_boards(), 32, &body).unwrap();
    let c = muir::checkpoint::read(&path).unwrap();
    assert_eq!((c.version, c.word_bits), (muir::checkpoint::VERSION, 32));
    let mut back = Micro::new(rev16(0o100000));
    let err = back.load(&mut c.reader()).expect_err("refused");
    assert!(err.to_string().contains("32-bit words, the CADR's"), "{err}");
}

/// **The prompt's `mem` takes 28-bit physical addresses on QUUX**
/// (G1 §3.2), and 22-bit ones on a 32-bit machine as ever.
#[test]
fn the_prompts_mem_takes_28_bit_addresses() {
    let high = 0o20000000;
    let dump = |bits| muir::prompt::main_dump_wide(high, 1, high + 1, |_| Some(1), bits);
    assert!(dump(40).is_ok(), "a 28-bit address on a 40-bit machine");
    assert!(dump(32).is_err(), "past the Xbus's 22 bits on a 32-bit one");
}
