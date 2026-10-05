// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's disk file (contract Q8): raw, a fixed VHD or a dynamic VHD, told
//! apart by the VHD footer, with a GPT inside.
//!
//! The fixtures, `data/quux-disk*`, are made by `tools/quux-disk-fixtures.sh`
//! with sgdisk, dd, qemu-img and qemu-io, never by muir: they are what muir's
//! reader is held to. The VHD fields this file reads for itself are
//! Microsoft's *Virtual Hard Disk Image Format Specification*: the footer's
//! cookie `conectix` at 0, data offset at 16, current size at 48, disk type
//! at 60 and checksum at 64, big-endian, the checksum the one's complement
//! of the byte sum with the field as zero; the dynamic header's table offset
//! at 16, entries at 28 and block size at 32. The GPT fields are UEFI's:
//! the header's `EFI PART` at LBA 1 with the entry array's LBA at 72, entry
//! count at 80 and entry size at 84; an entry's type GUID at 0, first and
//! last LBA at 32 and 40, attributes at 48 and UTF-16LE name at 56.

use std::path::{Path, PathBuf};

use muir::block_disk::{self, BlockDisk};
use muir::disk_image::{Disk, Format};
use muir::disk_unit::{BLOCK_WORDS, Geometry, Unit};

const BLOCK: usize = 1024;

fn data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data").join(name)
}

/// A directory of this test's own, removed afterwards.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let p = std::env::temp_dir().join(format!("muir-quux-disk-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Scratch(p)
    }

    /// A copy of a fixture to write into.
    fn copy(&self, fixture: &str) -> PathBuf {
        let to = self.0.join(fixture);
        std::fs::copy(data(fixture), &to).unwrap();
        to
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Block `n` of a raw file as the machine's words, low byte first.
fn raw_block(raw: &[u8], n: usize) -> [u32; BLOCK_WORDS] {
    std::array::from_fn(|w| {
        u32::from_le_bytes(raw[n * BLOCK + 4 * w..n * BLOCK + 4 * w + 4].try_into().unwrap())
    })
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
}

fn be64(b: &[u8], at: usize) -> u64 {
    u64::from_be_bytes(b[at..at + 8].try_into().unwrap())
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn le64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// The VHD checksum over a footer or a dynamic header, the field at `at`
/// counted as zero, computed here rather than by muir.
fn vhd_checksum(b: &[u8], at: usize) -> u32 {
    let sum = b
        .iter()
        .enumerate()
        .filter(|(k, _)| !(at..at + 4).contains(k))
        .fold(0u32, |s, (_, &x)| s.wrapping_add(x as u32));
    !sum
}

/// Every block of `disk` equal to the same block of the raw file.
fn equal_block_for_block(disk: &mut Disk, raw: &[u8], what: &str) {
    assert_eq!(disk.blocks() as usize, raw.len() / BLOCK, "{what}: the size in blocks");
    for n in 0..raw.len() / BLOCK {
        assert_eq!(disk.read_block(n as u32), Some(raw_block(raw, n)), "{what}: block {n}");
    }
    assert_eq!(disk.read_block(disk.blocks()), None, "{what}: past the end");
}

/// **Each fixture reads back through muir's disk layer as its raw twin,
/// block for block**, and its format is the one its footer says.
#[test]
fn each_fixture_reads_as_its_raw_twin() {
    let raw = std::fs::read(data("quux-disk.img")).unwrap();
    let grown = std::fs::read(data("quux-disk-grown.img")).unwrap();
    assert_eq!(raw.len(), 8 << 20);
    assert_ne!(raw, grown, "the writes changed the disk");
    for (file, format, twin) in [
        ("quux-disk.img", Format::Raw, &raw),
        ("quux-disk-fixed.vhd", Format::FixedVhd, &raw),
        ("quux-disk-dynamic.vhd", Format::DynamicVhd, &raw),
        ("quux-disk-grown.img", Format::Raw, &grown),
        ("quux-disk-dynamic-grown.vhd", Format::DynamicVhd, &grown),
    ] {
        let mut d = Disk::open(data(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_eq!(d.format(), Some(format), "{file}");
        equal_block_for_block(&mut d, twin, file);
    }
}

/// **The format is the footer's, not the file's name**: a fixed VHD called
/// `.img` is still one, and its footer is not a block of the disk.
#[test]
fn the_format_is_the_footer_s_not_the_name() {
    let s = Scratch::new("name");
    let named = s.0.join("fixed.img");
    std::fs::copy(data("quux-disk-fixed.vhd"), &named).unwrap();
    let mut d = Disk::open(&named).unwrap();
    assert_eq!(d.format(), Some(Format::FixedVhd));
    assert_eq!(d.blocks(), 8192, "the footer's current size, not the file's");
    let raw = std::fs::read(data("quux-disk.img")).unwrap();
    equal_block_for_block(&mut d, &raw, "fixed.img");
}

/// The dynamic VHD's allocation table: each 2 MiB block's sector, or none.
fn bat(vhd: &[u8]) -> Vec<u32> {
    let header = be64(&vhd[vhd.len() - 512..], 16) as usize;
    let table = be64(vhd, header + 16) as usize;
    let entries = be32(vhd, header + 28) as usize;
    (0..entries).map(|k| be32(vhd, table + 4 * k)).collect()
}

/// A footer at the end that says `conectix`, checks, and is the same as
/// the copy at 0.
fn footer_is_good(vhd: &[u8], what: &str) {
    let footer = &vhd[vhd.len() - 512..];
    assert_eq!(&footer[..8], b"conectix", "{what}: the footer is at the end");
    assert_eq!(be32(footer, 64), vhd_checksum(footer, 64), "{what}: the footer checks");
    assert_eq!(&vhd[..512], footer, "{what}: the copy at 0 is the footer");
}

/// The writes `tools/quux-disk-fixtures.sh` makes with qemu-io, as blocks:
/// 1,024 bytes of 0x4c at sector 5120, 4,096 of 0x50 at 7168 and 8,192 of
/// 0x46 at 9216: the first sectors of LOD2, PAGE and FILE.
const QEMU_IO_WRITES: [(u32, u32, u8); 3] = [(2560, 1, 0x4c), (3584, 4, 0x50), (4608, 8, 0x46)];

/// **A write to an unallocated block of a dynamic VHD allocates it**: a
/// sector bitmap and a 2 MiB block where the footer was, the footer after
/// them and still checking, the copy at 0 unchanged, and the table entry.
/// Given qemu-io's writes, muir makes qemu-io's file byte for byte.
#[test]
fn a_write_grows_a_dynamic_vhd_as_qemu_does() {
    let s = Scratch::new("grow");
    let path = s.copy("quux-disk-dynamic.vhd");
    let before = std::fs::read(&path).unwrap();
    assert_eq!(bat(&before)[1..3], [u32::MAX, u32::MAX], "blocks 1 and 2 unallocated");
    {
        let mut d = Disk::open_rw(&path).unwrap();
        for (first, count, byte) in QEMU_IO_WRITES {
            let words = [u32::from_le_bytes([byte; 4]); BLOCK_WORDS];
            for n in first..first + count {
                assert!(d.write_block(n, &words), "block {n} written");
            }
        }
    }
    let after = std::fs::read(&path).unwrap();
    assert_eq!(after.len(), before.len() + 2 * (512 + (2 << 20)), "two blocks and their bitmaps");
    footer_is_good(&after, "grown by muir");
    let table = bat(&after);
    assert_eq!(table[0], bat(&before)[0]);
    assert_eq!(table[3], bat(&before)[3]);
    for k in [1, 2] {
        let at = table[k] as usize * 512;
        assert_ne!(table[k], u32::MAX, "block {k} allocated");
        assert!(at + 512 + (2 << 20) <= after.len() - 512, "block {k} before the footer");
        assert!(after[at..at + 512].iter().all(|&b| b == 0xff), "block {k}'s bitmap all ones");
    }
    let mut d = Disk::open(&path).unwrap();
    let grown = std::fs::read(data("quux-disk-grown.img")).unwrap();
    equal_block_for_block(&mut d, &grown, "grown by muir, read by muir");
    assert!(
        after == std::fs::read(data("quux-disk-dynamic-grown.vhd")).unwrap(),
        "muir's file is qemu-io's, byte for byte"
    );
}

/// qemu-img, from `QEMU_IMG` or the path, if it is there.
fn qemu_img() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("QEMU_IMG") {
        return Some(PathBuf::from(p));
    }
    let found = std::process::Command::new("qemu-img").arg("--version").output();
    found.is_ok_and(|o| o.status.success()).then(|| PathBuf::from("qemu-img"))
}

/// **qemu reads a dynamic VHD muir grew as muir's own reader does**: it
/// compares equal to the raw file given the same writes. (qemu-img has no
/// `check` for VHD: "This image format does not support checks".)
/// Skipped, saying so, where qemu-img is not installed.
#[test]
fn qemu_reads_what_muir_grew() {
    let Some(qemu_img) = qemu_img() else {
        eprintln!("skipped: no qemu-img on the path and no QEMU_IMG");
        return;
    };
    let s = Scratch::new("qemu");
    let vhd = s.copy("quux-disk-dynamic.vhd");
    let raw = s.copy("quux-disk.img");
    let mut raw_bytes = std::fs::read(&raw).unwrap();
    {
        let mut d = Disk::open_rw(&vhd).unwrap();
        // Blocks in LOD2, PAGE and FILE, and one across nothing written
        // before, each a different word.
        for (k, n) in [2630u32, 3700, 4700, 6000, 7000].into_iter().enumerate() {
            let words: [u32; BLOCK_WORDS] =
                std::array::from_fn(|w| (k as u32 + 1) << 24 | n << 8 | w as u32);
            assert!(d.write_block(n, &words));
            for (w, v) in words.iter().enumerate() {
                let at = n as usize * BLOCK + 4 * w;
                raw_bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
    }
    std::fs::write(&raw, &raw_bytes).unwrap();
    let compare = std::process::Command::new(&qemu_img)
        .args(["compare", "-f", "vpc", "-F", "raw"])
        .arg(&vhd)
        .arg(&raw)
        .output()
        .unwrap();
    assert!(
        compare.status.success(),
        "qemu-img compare: {}{}",
        String::from_utf8_lossy(&compare.stdout),
        String::from_utf8_lossy(&compare.stderr)
    );
    footer_is_good(&std::fs::read(&vhd).unwrap(), "grown by muir");
}

/// **A write to a fixed VHD lands in place**: the file keeps its size and
/// its footer, and the block is where the raw disk has it.
#[test]
fn a_write_to_a_fixed_vhd_lands_in_place() {
    let s = Scratch::new("fixed");
    let path = s.copy("quux-disk-fixed.vhd");
    let before = std::fs::read(&path).unwrap();
    let words: [u32; BLOCK_WORDS] = std::array::from_fn(|w| 0o7000000 | w as u32);
    assert!(Disk::open_rw(&path).unwrap().write_block(4000, &words));
    let after = std::fs::read(&path).unwrap();
    assert_eq!(after.len(), before.len());
    assert_eq!(after[after.len() - 512..], before[before.len() - 512..], "the footer untouched");
    assert_eq!(raw_block(&after, 4000), words);
    assert_eq!(after[..4000 * BLOCK], before[..4000 * BLOCK]);
    assert_eq!(after[4001 * BLOCK..], before[4001 * BLOCK..]);
}

/// **Opened read-only, nothing reaches the file**: a written block reads
/// back for the run, and the file is as it was.
#[test]
fn opened_read_only_nothing_reaches_the_file() {
    let s = Scratch::new("ro");
    for fixture in ["quux-disk.img", "quux-disk-fixed.vhd", "quux-disk-dynamic.vhd"] {
        let path = s.copy(fixture);
        let before = std::fs::read(&path).unwrap();
        let words = [0o1234567u32; BLOCK_WORDS];
        let mut d = Disk::open(&path).unwrap();
        assert!(!d.writable());
        assert!(d.write_block(5000, &words));
        assert_eq!(d.read_block(5000), Some(words), "{fixture}: read back for the run");
        assert_eq!(d.written_blocks(), 1);
        assert!(std::fs::read(&path).unwrap() == before, "{fixture}: the file unchanged");
    }
}

/// **A dynamic VHD whose footer at the end was lost opens by the copy at
/// 0**, which is what the copy is for; its next block goes at the file's
/// end and the footer is written after it again.
#[test]
fn a_dynamic_vhd_without_its_trailing_footer_opens_by_the_copy() {
    let s = Scratch::new("torn");
    let path = s.copy("quux-disk-dynamic.vhd");
    let whole = std::fs::read(&path).unwrap();
    std::fs::write(&path, &whole[..whole.len() - 512]).unwrap();
    let raw = std::fs::read(data("quux-disk.img")).unwrap();
    let mut d = Disk::open_rw(&path).unwrap();
    assert_eq!(d.format(), Some(Format::DynamicVhd));
    equal_block_for_block(&mut d, &raw, "without its footer");
    assert!(d.write_block(3700, &[7; BLOCK_WORDS]));
    drop(d);
    let after = std::fs::read(&path).unwrap();
    assert_eq!(after.len(), whole.len() + 512 + (2 << 20), "a block after the old data");
    footer_is_good(&after, "the footer written again");
}

/// **What is not a VHD muir reads is refused, and says why**: a footer
/// that does not check, a differencing VHD, a VHDX.
#[test]
fn what_muir_does_not_read_is_refused() {
    let s = Scratch::new("refused");
    let refused = |path: &Path, saying: &str| {
        let e = Disk::open(path).err().unwrap_or_else(|| panic!("{} opened", path.display()));
        assert!(e.to_string().contains(saying), "{}: {e}", path.display());
    };
    // A footer whose checksum does not check, at the end of a fixed VHD.
    let path = s.copy("quux-disk-fixed.vhd");
    let mut b = std::fs::read(&path).unwrap();
    let n = b.len();
    b[n - 512 + 48] ^= 1;
    std::fs::write(&path, &b).unwrap();
    refused(&path, "checksum");
    // A differencing VHD: disk type 4, the checksum made good.
    let path = s.copy("quux-disk-dynamic.vhd");
    let mut b = std::fs::read(&path).unwrap();
    let n = b.len();
    for at in [0, n - 512] {
        b[at + 63] = 4;
        let c = vhd_checksum(&b[at..at + 512], 64);
        b[at + 64..at + 68].copy_from_slice(&c.to_be_bytes());
    }
    std::fs::write(&path, &b).unwrap();
    refused(&path, "differencing");
    // A VHDX begins `vhdxfile`.
    let path = s.0.join("x.vhdx");
    let mut b = vec![0u8; 1 << 20];
    b[..8].copy_from_slice(b"vhdxfile");
    std::fs::write(&path, &b).unwrap();
    refused(&path, "VHDX");
}

/// **A raw disk of any size opens on QUUX**, its blocks whatever the file
/// holds, a trailing part block unreachable; past the end is block-disk's
/// error, as it is at a T-300's end.
#[test]
fn a_raw_disk_of_any_size_opens() {
    let s = Scratch::new("size");
    let path = s.0.join("small.img");
    let mut bytes: Vec<u8> = (0..100 * BLOCK).map(|k| (k / BLOCK) as u8).collect();
    bytes.extend_from_slice(&[0xee; 512]);
    std::fs::write(&path, &bytes).unwrap();
    let mut d = Disk::open_rw(&path).unwrap();
    assert_eq!(d.format(), Some(Format::Raw));
    assert_eq!(d.blocks(), 100);
    assert_eq!(d.read_block(99).unwrap()[0], 0x63636363);
    assert_eq!(d.read_block(100), None);
    assert!(!d.write_block(100, &[0; BLOCK_WORDS]));
    let mut bd = BlockDisk::new(block_disk::BLOCK_NS);
    bd.attach(d);
    // Two pages by the 4-byte transfer, 4 blocks a page: 96-99, then 100,
    // past the end, which leaves its page as it was.
    let mut main = vec![0u64; 1 << 16];
    main[0o100] = 0o2000 | 1;
    main[0o101] = 0o4000;
    bd.write(block_disk::CLP, 0o100, &mut main);
    bd.write(block_disk::DA, 96, &mut main);
    bd.write(block_disk::COMMAND, 1 << 12, &mut main);
    bd.write(block_disk::START, 0, &mut main);
    bd.advance(u64::MAX / 2);
    assert_eq!(main[0o2000 + 3 * 256] as u32, 0x63636363, "block 99 moved");
    assert_eq!(main[0o4000], 0, "the page past the end left as it was");
    let status = bd.read(block_disk::STATUS);
    assert_ne!(status & (1 << 17), 0, "block 100 is past the end");
    assert_eq!(bd.read(block_disk::DA), 100);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), bytes.len() as u64, "nothing added");
}

/// **The CADR's drive takes its pack and nothing else, as before**: none of
/// QUUX's disks is the size of a T-300's pack, and each is refused with the
/// same words as any other file of the wrong size.
#[test]
fn the_cadr_refuses_quux_s_disks_as_before() {
    for file in [
        "quux-disk.img",
        "quux-disk-fixed.vhd",
        "quux-disk-dynamic.vhd",
        "quux-disk-dynamic-grown.vhd",
    ] {
        let len = std::fs::metadata(data(file)).unwrap().len();
        let e = Unit::open(data(file), Geometry::T300).err().expect("refused");
        let want = Geometry::T300.blocks() as u64 * 1024;
        assert!(e.to_string().ends_with(&format!(": {len} bytes, expected {want}")), "{file}: {e}");
    }
}

/// **A checkpoint keeps the disk's written blocks and its size**, and one
/// of a disk of another size is refused.
#[test]
fn a_checkpoint_keeps_the_disk() {
    use muir::checkpoint::{Reader, Writer};
    let mut d = Disk::open(data("quux-disk-dynamic.vhd")).unwrap();
    assert!(d.write_block(3700, &[0o5; BLOCK_WORDS]));
    let mut w = Writer::new();
    d.save(&mut w);
    let body = w.finish();
    let mut back = Disk::open(data("quux-disk-dynamic.vhd")).unwrap();
    back.load(&mut Reader::new(&body)).unwrap();
    assert_eq!(back.read_block(3700), Some([0o5; BLOCK_WORDS]));
    assert_eq!(back.written_blocks(), 1);
    let mut other = Disk::blank(Geometry::T300.blocks());
    let e = other.load(&mut Reader::new(&body)).expect_err("refused");
    assert!(e.to_string().contains("8192"), "{e}");
}

// --- the GPT inside -----------------------------------------------------------

/// One GPT entry.
#[derive(Debug)]
struct Entry {
    kind: String,
    first: u64,
    last: u64,
    attributes: u64,
    name: String,
}

/// A GUID as it is written, from its mixed-endian bytes.
fn guid(b: &[u8]) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{}",
        le32(b, 0),
        u16::from_le_bytes([b[4], b[5]]),
        u16::from_le_bytes([b[6], b[7]]),
        b[8],
        b[9],
        b[10..16].iter().map(|x| format!("{x:02x}")).collect::<String>()
    )
}

/// The partitions, read through muir's disk layer: blocks 0 onward.
fn partitions(d: &mut Disk) -> Vec<Entry> {
    let bytes = |d: &mut Disk, n: u32| -> Vec<u8> {
        d.read_block(n).unwrap().iter().flat_map(|w| w.to_le_bytes()).collect()
    };
    let header = bytes(d, 0)[512..].to_vec();
    assert_eq!(&header[..8], b"EFI PART", "the header at LBA 1");
    let (array, count, size) =
        (le64(&header, 72), le32(&header, 80) as u64, le32(&header, 84) as u64);
    let mut table = Vec::new();
    for n in (array / 2) as u32..=((array * 512 + count * size - 1) / 1024) as u32 {
        table.extend(bytes(d, n));
    }
    let table = &table[(array as usize % 2) * 512..];
    (0..count as usize)
        .map(|k| &table[k * size as usize..(k + 1) * size as usize])
        .filter(|e| e[..16].iter().any(|&b| b != 0))
        .map(|e| Entry {
            kind: guid(&e[..16]),
            first: le64(e, 32),
            last: le64(e, 40),
            attributes: le64(e, 48),
            name: String::from_utf16(
                &e[56..128]
                    .chunks(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .take_while(|&u| u != 0)
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        })
        .collect()
}

const MICROCODE: &str = "9e318cf5-a95b-4b3b-b2ad-9ae306b0e2da";
const BAND: &str = "a3b30470-c5d4-41c1-87a8-d26590424cb8";
const PAGE: &str = "4652bea5-06af-4bd9-b2bb-3541370151c8";
const FILE: &str = "7afa9532-75de-409f-8dc8-fef9763511d5";
/// Retired with the TEMP partition Q8 dropped: no partition may carry it.
const RETIRED_TEMP: &str = "445976f2-34e4-4583-b750-75d28a080cba";

/// **The fixtures' GPT is Q8's**, read through each of muir's three
/// readers: our type GUIDs, names of a four-character name and a comment of
/// up to 31, bit 48 on the current microcode and band only, and every
/// partition whole blocks --- first LBA even, last odd --- and its contents
/// where the table says, each block saying which it is. There is no TEMP
/// partition, and no partition carries its retired type GUID.
#[test]
fn the_fixtures_gpt_is_q8_s() {
    let want = [
        (MICROCODE, 2048, 2559, "MCR1 UCADR 2000", true),
        (MICROCODE, 2560, 3071, "MCR2 UCADR 999", false),
        (BAND, 3072, 5119, "LOD1 System 2000.1", true),
        (BAND, 5120, 7167, "LOD2 A comment that is 31 characters", false),
        (PAGE, 7168, 9215, "PAGE", false),
        (FILE, 9216, 16349, "FILE", false),
    ];
    for file in ["quux-disk.img", "quux-disk-fixed.vhd", "quux-disk-dynamic.vhd"] {
        let mut d = Disk::open(data(file)).unwrap();
        let got = partitions(&mut d);
        for e in &got {
            assert_ne!(e.kind, RETIRED_TEMP, "{file}: {}: the retired TEMP type", e.name);
            assert!(!e.name.starts_with("TEMP"), "{file}: {}: no TEMP partition", e.name);
        }
        assert_eq!(got.len(), want.len(), "{file}: {got:?}");
        for (e, (kind, first, last, name, current)) in got.iter().zip(want) {
            assert_eq!(
                (e.kind.as_str(), e.first, e.last, e.name.as_str()),
                (kind, first, last, name)
            );
            assert_eq!(e.attributes >> 48 & 1 == 1, current, "{file}: {name}: bit 48");
            assert_eq!((e.first % 2, e.last % 2), (0, 1), "{file}: {name}: whole blocks");
            let (lisp, comment) = e.name.split_at(4);
            assert!(lisp.chars().all(|c| c.is_ascii_alphanumeric()), "{name}");
            assert!(comment.is_empty() || comment.starts_with(' '), "{name}");
            assert!(comment.chars().count() <= 32, "{name}: a comment of at most 31");
        }
        for (e, filled) in got.iter().zip([20, 4, 64, 0, 0, 0]) {
            let lisp = &e.name[..4];
            for k in 0..filled.max(1) {
                let block = d.read_block((e.first / 2) as u32 + k).unwrap();
                let text: Vec<u8> = block.iter().flat_map(|w| w.to_le_bytes()).collect();
                if filled == 0 {
                    assert!(text.iter().all(|&b| b == 0), "{file}: {lisp} empty");
                } else {
                    let line = format!("{lisp} block {k:04} ");
                    assert_eq!(&text[..16], line.as_bytes(), "{file}: {lisp} block {k}");
                    assert_eq!(&text[1008..], line.as_bytes(), "{file}: {lisp} block {k}");
                }
            }
        }
    }
}
