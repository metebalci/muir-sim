// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **QUUX revision 13's memory and devices** (contract G1 §3-§4, G2 §3-§4,
//! appendix A1.10-A1.13), through the machine's register page and its
//! parts: packed storage, block-disk's packed and 4-byte transfers, the
//! file device's tags and 28-bit addresses, the checkpoint at 40 bits, the
//! `.mcr` at 40 bits, and the cache's 8-word lines with the prefetch's
//! page reach. `tests/revision_13.rs` runs the physical space itself on
//! both engines.

use std::path::PathBuf;

use muir::block_disk::BlockDisk;
use muir::busint::Responder;
use muir::checkpoint::Writer;
use muir::disk_image::Disk;
use muir::engine::Engine;
use muir::file_device::{Mounts, op};
use muir::machine::{Geometry, Machine, Word};
use muir::memory_port::{MemoryPort, Reach};
use muir::micro::Micro;

mod support;

/// The register page at 28 bits (G1 §3.2).
const PAGE: u32 = 0o1777777400;
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

/// Revision 13 with `words` of main memory.
fn rev13(words: usize) -> Machine {
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
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

/// **The CADR's device addresses are nothing on revision 13** when main
/// memory does not reach them (G1 §3.2): its last page, `17777400`, and its
/// TV's buffer, `17000000`, fail with word 101's NXM bit; the register
/// page answers.
#[test]
fn the_cadr_s_device_addresses_are_nothing_on_revision_13() {
    let mut m = rev13(2 << 20);
    for phys in [0o17777400, 0o17777500, 0o17000000, 2 << 20] {
        m.bus_error = 0;
        assert_eq!(m.bus_read(phys), 0, "{phys:o}");
        assert_eq!(m.bus_read(PAGE + 0o101), 1, "{phys:o}: NXM");
    }
    assert_eq!(m.bus_read(PAGE), (0x5155 << 16) | (13 << 4) | 4, "the page's MACHINE-ID");
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
    m.ns += 1_000_000_000;
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
    let mut m = rev13(0o20004000);
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
    let mut m = rev13(0o100000);
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
    let mut m = rev13(0o100000);
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
    let mut m = rev13(0o100000);
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
    let mut m = rev13(0o100004000);
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
    let mut m = rev13(base + 0o10000);
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
            assert!(m.ns - start < 10_000_000_000, "no response");
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

/// A revision-13 machine on `micro` with words at every offset of line 0
/// and of [`LINE_4K`], tags and all.
fn with_lines() -> (Micro, Vec<Word>) {
    let mut m = rev13(0o100000);
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
    let bits = e.machine().geometry.word_bits;
    muir::checkpoint::write(&path, "micro", e.machine().memory_boards(), bits, &body).unwrap();
    let c = muir::checkpoint::read(&path).unwrap();
    assert_eq!(c.word_bits, 40, "the file says its words are 40 bits");
    assert_eq!(c.version, muir::checkpoint::VERSION_40, "a version of its own");
    let mut back = Micro::new(rev13(0o100000));
    back.load(&mut c.reader()).unwrap();
    assert_eq!(back.machine().main, e.machine().main, "main memory back");
    assert_eq!(back.machine().geometry, Geometry::QUUX);
}

/// **A 32-bit checkpoint, the CADR's, is refused on revision 13** (G2
/// §2.8), and a 32-bit machine's file keeps its version. A checkpoint of
/// revision 12 is refused by the machine it records
/// (`a_revision_12_checkpoint_is_refused_by_its_machine` in
/// `tests/quux_revision.rs`).
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
    let mut back = Micro::new(rev13(0o100000));
    let err = back.load(&mut c.reader()).expect_err("refused");
    assert!(err.to_string().contains("32-bit words, the CADR's"), "{err}");
}

/// **`rtl` resumes revision 13 on revision 13's memory port** (G2 §3): a
/// program reading main memory a line of 8 apart, every read a miss and a
/// fill of 5 beats, saved at its microcycle 200 and loaded into a fresh
/// engine, runs on to the same end as the run never saved: the same
/// nanoseconds, and the checkpoint at the end byte for byte. A port the
/// restore rebuilt with 4-word lines' layout would fill in 2 beats and end
/// sooner.
#[test]
fn rtl_resumes_revision_13_on_its_own_memory_port() {
    use muir::checkpoint::Reader;
    use muir::isa::Insn;
    use muir::isa::asm::{
        ADD, ALU, ALWAYS, JUMP, SETM, SRC_MD, START_READ, a_dest, a_src, filler, m_dest, m_src,
        target,
    };
    use muir::rtl::Rtl;
    let prom = [
        Insn::new(ALU | SETM | m_src(1) | START_READ),
        filler(),
        filler(),
        Insn::new(ALU | ADD | SRC_MD | a_src(3) | a_dest(3)),
        Insn::new(ALU | ADD | m_src(1) | a_src(5) | m_dest(1)),
        Insn::new(JUMP | target(0) | ALWAYS),
    ];
    let make = || {
        let mut m = rev13(1 << 16);
        let mut words = vec![filler(); 1024];
        words[..prom.len()].copy_from_slice(&prom);
        m.load_prom(&words);
        support::prom_program_in_ram(&mut m);
        // Virtual pages 0-31, of 1024 words, onto the same physical pages.
        for p in 0..32u32 {
            m.l2_map[p as usize] = 1 << 27 | 1 << 26 | p;
        }
        m.amem[5] = 8;
        let mut e = Rtl::new(m);
        e.boot();
        e
    };
    let end = |e: &Rtl| {
        let mut w = Writer::new();
        e.save(&mut w);
        (e.ns(), w.finish())
    };
    let mut straight = make();
    straight.run(200);
    let mut w = Writer::new();
    straight.save(&mut w);
    let body = w.finish();
    let mut resumed = make();
    resumed.load(&mut Reader::for_word_bits(&body, 40)).unwrap();
    straight.run(2000);
    resumed.run(2000);
    let (a, b) = (end(&straight), end(&resumed));
    assert!(straight.cache().unwrap().misses > 100, "the reads missed");
    assert_eq!(b.0, a.0, "the resumed run's nanoseconds");
    assert!(b.1 == a.1, "the resumed run ends as the straight one, byte for byte");
}

// --- the .mcr -------------------------------------------------------------------

/// A 32-bit word as MIT's `.mcr` puts it out: the high half, then the low,
/// each little-endian.
fn out32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&((v >> 16) as u16).to_le_bytes());
    b.extend_from_slice(&(v as u16).to_le_bytes());
}

/// **The `.mcr` at 40 bits** (A1.4, A1.12): a D-MEM section of `10000`
/// entries, and section 5, A memory at 40 bits, two 32-bit words a
/// location, `<31:0>` then `<39:32>` in `<7:0>`; in partition order as in
/// MIT's.
#[test]
fn the_mcr_reads_4096_dmem_entries_and_a_40_bit_a_memory() {
    let mut b = Vec::new();
    for v in [1, 0, 1] {
        out32(&mut b, v);
    }
    for half in [0u16, 0o1234, 0o5670, 0o1357] {
        b.extend_from_slice(&half.to_le_bytes());
    }
    for v in [2, 0, 0o10000] {
        out32(&mut b, v);
    }
    for k in 0..0o10000u32 {
        out32(&mut b, if k == 0o7777 { 0o7654 } else { 0 });
    }
    for v in [3, 4, 0o200, 0o6000] {
        out32(&mut b, v);
    }
    for v in [5, 0o40, 2] {
        out32(&mut b, v);
    }
    for (lo, hi) in [(0o1234567_u32, 0o025_u32), (0xffff_ffff, 0o377)] {
        out32(&mut b, lo);
        out32(&mut b, hi);
    }
    b.resize(b.len().next_multiple_of(1024), 0);
    let check = |m: muir::mcr::Mcr, what: &str| {
        assert_eq!(m.dmem.len(), 0o10000, "{what}: D-MEM entries");
        assert_eq!(m.dmem[0o7777], 0o7654, "{what}: entry 7777");
        assert!(m.amem_wide, "{what}: section 5");
        assert_eq!(m.amem_start, 0o40);
        assert_eq!(m.amem, [w(0o025, 0o1234567), w(0o377, 0xffff_ffff)], "{what}: A memory");
        assert_eq!(m.version(), Some(0o1234567), "{what}: A-VERSION's field");
    };
    check(muir::mcr::parse(&b).unwrap(), "MIT's order");
    let partition = muir::mcr::swap_halves(&b).unwrap();
    check(muir::mcr::parse_partition_order(&partition).unwrap(), "partition order");
}

// --- the cache and the prefetch -----------------------------------------------

/// Answers a read of `phys` on `p` at `now`, for the fetch of virtual
/// `vaddr` if it is one; when it was answered.
fn read(p: &mut MemoryPort, now: u64, phys: u32, fetch: Option<u32>, main: &[Word]) -> u64 {
    p.request_at(false, phys);
    if let Some(v) = fetch {
        p.mark_fetch(v);
    }
    p.mclk_edge(now, Responder::Memory(0));
    let at = p.ack_at().unwrap();
    p.poll(at, Responder::Memory(0)).unwrap();
    p.read_answered(main);
    p.finish();
    at
}

/// **Revision 13's port** (G2 §3, M7): 8-word lines, a fill of 5 beats
/// taking three ticks more than today's 2-beat line, and the prefetch with
/// the page's reach over 1024-word pages and 28-bit virtual addresses;
/// line reach selectable.
#[test]
fn revision_13s_port_has_8_word_lines_and_page_reach() {
    let main = vec![0 as Word; 0o100000];
    let mut p = MemoryPort::new();
    assert_eq!(p.cache().config.line_words, 8);
    assert_eq!(p.prefetch(), Some(Reach::Page), "revision 13's reach");
    let t = p.memory_timing();
    let at = read(&mut p, 1000, 0o400, None, &main);
    assert_eq!(at, 1000 + t.read_ns + 30, "a miss: the fill of 5 beats");
    assert_eq!(read(&mut p, 5000, 0o407, None, &main), 5000 + 20, "the line's last word hits");
    assert_eq!(p.cache().misses, 1);
    // The next line held: a fetch of 0o377, the end of a 256-word page but
    // not of a 1024-word one, takes 0o400 from it.
    read(&mut p, 9000, 0o370, None, &main);
    read(&mut p, 13000, 0o377, Some(0x0abc_d377), &main);
    let got = p.prefetched().expect("the next line's word");
    assert_eq!((got.phys, got.vaddr), (0o400, 0x0abc_d378), "28 bits of virtual address");
    // The end of a 1024-word page stops it.
    read(&mut p, 17000, 0o2000, None, &main);
    read(&mut p, 21000, 0o1777, Some(0o1777), &main);
    assert_eq!(p.prefetched(), None, "not past the page");
    // Line reach, for the fabric to take back.
    p.set_prefetch(Some(Reach::Line));
    read(&mut p, 25000, 0o377, Some(0o377), &main);
    assert_eq!(p.prefetched(), None, "line reach: not into the next line");
}

/// **The prompt's `mem` takes 28-bit physical addresses on revision 13**
/// (G1 §3.2), and 22-bit ones on a 32-bit machine as ever.
#[test]
fn the_prompts_mem_takes_28_bit_addresses() {
    let high = 0o20000000;
    let dump = |bits| muir::prompt::main_dump_wide(high, 1, high + 1, |_| Some(1), bits);
    assert!(dump(40).is_ok(), "a 28-bit address on a 40-bit machine");
    assert!(dump(32).is_err(), "past the Xbus's 22 bits on a 32-bit one");
}
