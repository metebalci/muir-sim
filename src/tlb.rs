// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **QUUX revision 14's TLB and the address space it serves** (contract G3
//! revision 14, appendix A14.1-A14.6), on both engines: the contents are
//! modelled, so a stale entry changes what a reference reaches, not only
//! when.
//!
//! **The address space** by `VA<31:28>` (A14.1): `0000`-`1101` paged,
//! through the TLB; `1110` the device window; `1111` the physical memory
//! window, physical = `VA<27:0>`. The decode is `VA<31:29>` = `111` for
//! the two windows and `VA<28>` between them; `VA<39:32>` is never looked
//! at.
//!
//! **The TLB** is direct-mapped, N entries, a power of two from 1,024 to
//! 32,768 (`--tlb`, 4,096 by default; A14.4). With k = log2 N, the index
//! is `VA<9+k:10>` and the tag `VA<31:10+k>`; an entry is a valid bit, the
//! tag and the page entry's `<29:0>`. A miss on a paged address walks the
//! page table ([`walk`]) and loads what it finds, unless that is no entry.
//!
//! **The bus address** a translated reference goes out on is 29 bits, the
//! cache's key (A14.1): `<28>` = 0 and the physical word address for main
//! memory, reached through the TLB or the physical memory window; `<28>` =
//! 1 and `VA<27:0>` for the device window.

/// The fewest and the most entries `--tlb` takes, and the default
/// (decisions.md:265; A14.4).
pub const MIN_ENTRIES: usize = 1024;
pub const MAX_ENTRIES: usize = 32 * 1024;
pub const DEFAULT_ENTRIES: usize = 4096;

/// The sweep clears one entry a tick of 10 ns (A14.4).
pub const SWEEP_TICK_NS: u64 = 10;

/// A page entry's `<29:0>`, what the TLB holds of it.
pub const ENTRY_BITS: u32 = (1 << 30) - 1;

/// `<29:18>` = `11` and `1460`: PGF-MM0's "RW access, status=4, no area
/// traps" (`uc-page-fault.lisp:574`), with accessed and modified set so that
/// no write-back is ever asked for (A14.5).
const RW_STATUS_4: u32 = 0b11 << 28 | 0o1460 << 18;

/// The fixed entry of the device window, every address but A memory's
/// window's (A14.5).
pub const DEVICE_WINDOW_ENTRY: u32 = RW_STATUS_4;
/// The fixed entry of A memory's window: `11` and `0760`, access `01`,
/// status 7, so that every reference faults (A14.5).
pub const A_MEMORY_WINDOW_ENTRY: u32 = 0b11 << 28 | 0o760 << 18;
/// The no-entry word (A14.5, A14.6): `00` and `0060`, access `00`, status
/// 0, not oldspace, not extra PDL.
pub const NO_ENTRY: u32 = 0o60 << 18;

/// The device window, `34000000000`-`35777777777`.
pub const DEVICE_WINDOW: u32 = 0o34000000000;
/// The physical memory window, `36000000000`-`37777777777`.
pub const PHYSICAL_WINDOW: u32 = 0o36000000000;
/// A memory's window in the device window, `35700000000`-`35700001777`,
/// A location `VA<9:0>` (A14.1).
pub const A_MEMORY_WINDOW: u32 = 0o35700000000;
/// The register page in the device window, `35777777400` (A14.1): at the
/// device window's offset `1777777400`, revision 13's physical address of
/// it.
pub const REGISTER_PAGE: u32 = 0o35777777400;
/// The bus address's device bit, `<28>` (A14.1).
pub const DEVICE: u32 = 1 << 28;
/// The register page's bus address.
pub const REGISTER_PAGE_BUS: u32 = DEVICE | (REGISTER_PAGE & 0o1777777777);

/// Where a virtual address lies (A14.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    /// `0000`-`1101`: through the TLB.
    Paged,
    /// `1110`, except A memory's window.
    Device,
    /// A memory's window: every reference faults.
    AMemory,
    /// `1111`: physical = `VA<27:0>`.
    Physical,
}

/// The region of `va`: the decode on `VA<31:29>` and `VA<28>`.
pub fn region(va: u32) -> Region {
    if va >> 29 != 0b111 {
        Region::Paged
    } else if va & (1 << 28) != 0 {
        Region::Physical
    } else if va & !0o1777 == A_MEMORY_WINDOW {
        Region::AMemory
    } else {
        Region::Device
    }
}

/// The fixed entry a window address reads, from the decode and never the
/// TLB (A14.5); `None` for a paged address. The physical memory window's
/// frame is `VA<27:10>`.
pub fn fixed_entry(va: u32) -> Option<u32> {
    match region(va) {
        Region::Paged => None,
        Region::Physical => Some(RW_STATUS_4 | (va >> 10 & 0o777777)),
        Region::Device => Some(DEVICE_WINDOW_ENTRY),
        Region::AMemory => Some(A_MEMORY_WINDOW_ENTRY),
    }
}

/// The 29-bit bus address a reference to `va` through `entry` goes out on
/// (A14.1): the device window's `<28>` = 1 and `VA<27:0>`; otherwise main
/// memory at `{entry<17:0>, VA<9:0>}`, which for the physical memory window,
/// whose fixed entry's frame is `VA<27:10>`, is `VA<27:0>`.
pub fn bus_address(va: u32, entry: u32) -> u32 {
    match region(va) {
        Region::Device | Region::AMemory => DEVICE | (va & 0o1777777777),
        _ => (entry & 0o777777) << 10 | (va & 0o1777),
    }
}

/// The status of a page entry, `<26:24>` (A14.2).
pub fn status(entry: u64) -> u32 {
    (entry >> 24) as u32 & 7
}

/// What a walk read and found (A14.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Walk {
    /// The physical words it read, in order: none with no directory, the
    /// directory entry, and the page entry when the directory entry is
    /// present with its frame in main memory.
    pub reads: [Option<u32>; 2],
    /// The page entry's `<29:0>` to load, or `None`: no entry.
    pub entry: Option<u32>,
}

/// **The walk** for the paged address `va` (A14.3, A14.6), over main memory
/// `main` with the directory base `base` (register-page word 220):
///
/// 1. base 0 is no directory: no entry, no read;
/// 2. the directory entry at `{base<17:2>, VA<31:20>}`: present when its
///    status `<26:24>` is 4 and its frame `<17:0>` is below main memory's
///    end, and no entry otherwise;
/// 3. the page entry at `{frame, VA<19:10>}`: status 0 is no entry; status
///    7, A memory's window's, is never in a table and is no entry too; a
///    status 1 entry (not in core) is loaded as it is; any other status is
///    in core, and no entry when its frame is at or past main memory's end.
///
/// A word past main memory reads 0, and the walk never faults or sets the
/// NXM bit: the tables are wired frames read by physical address.
pub fn walk(main: &[crate::machine::Word], base: u32, va: u32) -> Walk {
    let frames = (main.len() >> 10) as u32;
    let read = |a: u32| main.get(a as usize).copied().unwrap_or(0);
    if base & 0o777777 == 0 {
        return Walk { reads: [None, None], entry: None };
    }
    let dir_at = (base & 0o777774) << 10 | va >> 20;
    let dir = read(dir_at);
    let frame = dir as u32 & 0o777777;
    if status(dir) != 4 || frame >= frames {
        return Walk { reads: [Some(dir_at), None], entry: None };
    }
    let page_at = frame << 10 | (va >> 10 & 0o1777);
    let page = read(page_at);
    let entry = match status(page) {
        0 | 7 => None,
        1 => Some(page as u32 & ENTRY_BITS),
        _ if page as u32 & 0o777777 >= frames => None,
        _ => Some(page as u32 & ENTRY_BITS),
    };
    Walk { reads: [Some(dir_at), Some(page_at)], entry }
}

/// A page entry's accessed bit, `<28>` (A14.2, A14.6).
pub const ACCESSED: u32 = 1 << 28;
/// Its modified bit, `<29>`.
pub const MODIFIED: u32 = 1 << 29;
/// Its ephemeral-reference bit, `<19>` (A14.8).
pub const EPHEMERAL: u32 = 1 << 19;

/// The data types' field of the ephemeral space, `VA<31:28>` = `1101`
/// (A14.1, A14.8).
pub const EPHEMERAL_SPACE: u32 = 0b1101;

/// **The bits a reference through `entry` asks to write back** (A14.6,
/// A14.8), `entry` being the TLB entry the reference latched: accessed when
/// it is 0; on a write, modified when it is 0, and ephemeral-reference when
/// it is 0 and `ephemeral`, the setter's other conditions on the word
/// written and the enable. A faulting reference asks for none: the caller
/// asks only for one that does not fault.
pub fn write_back_bits(entry: u32, write: bool, ephemeral: bool) -> u32 {
    let mut bits = ACCESSED & !entry;
    if write {
        bits |= MODIFIED & !entry;
        if ephemeral {
            bits |= EPHEMERAL & !entry;
        }
    }
    bits
}

/// What a write-back read and wrote, for `rtl` to time (A14.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteBack {
    /// The directory entry and the page entry re-read, as a walk reads them.
    pub reads: [Option<u32>; 2],
    /// The page entry written, or `None` when the guard refused.
    pub write: Option<u32>,
}

/// **The write-back's read-modify-write** of the table for `va` in `main`
/// (A14.6): the directory entry and the page entry read again; the page
/// entry written with `bits` ORed in only when it is in core, status 2 to
/// 6, with the frame `frame` of the TLB entry the reference went through.
/// Every other bit of the word stays the table's: an OR, never a copy of
/// the TLB entry. Whether it wrote is [`WriteBack::write`].
pub fn write_back(
    main: &mut [crate::machine::Word],
    base: u32,
    va: u32,
    frame: u32,
    bits: u32,
) -> WriteBack {
    let w = walk(main, base, va);
    let write = w.reads[1].filter(|&at| {
        let page = main[at as usize];
        (2..=6).contains(&status(page)) && page as u32 & 0o777777 == frame & 0o777777
    });
    if let Some(at) = write {
        main[at as usize] |= u64::from(bits);
    }
    WriteBack { reads: w.reads, write }
}

/// The TLB's contents and its counts.
#[derive(Clone, Debug)]
pub struct Tlb {
    /// Each entry: `<63>` valid, the tag from `<32>` up, `<29:0>` the page
    /// entry's.
    entries: Vec<u64>,
    /// log2 of the entries.
    k: u32,
    /// Walks made, for the profile and the tests: each miss on a paged
    /// address that walked, whatever it found. Not in a checkpoint.
    pub walks: u64,
    /// Sweeps made, at reset and for an empty. Not in a checkpoint.
    pub sweeps: u64,
    /// Write-backs made (A14.6), and of them those carrying accessed,
    /// modified and ephemeral-reference, and those the guard refused. Not
    /// in a checkpoint: the profile's.
    pub write_backs: u64,
    pub written_bits: [u64; 3],
    pub refusals: u64,
    /// On `rtl`, the time the processor was held for walks and sweeps. Not
    /// in a checkpoint.
    pub held_ns: u64,
    /// On `rtl`, the instant the sweep in progress ends: a memory start or
    /// a port-B lookup waits for it (A14.4). `micro` sweeps at once.
    pub sweep_until: u64,
}

const VALID: u64 = 1 << 63;

impl Default for Tlb {
    fn default() -> Self {
        Self::new(DEFAULT_ENTRIES)
    }
}

impl Tlb {
    /// An empty TLB of `entries`, a power of two from [`MIN_ENTRIES`] to
    /// [`MAX_ENTRIES`].
    pub fn new(entries: usize) -> Tlb {
        assert!(
            entries.is_power_of_two() && (MIN_ENTRIES..=MAX_ENTRIES).contains(&entries),
            "a TLB of {entries} entries: a power of two from {MIN_ENTRIES} to {MAX_ENTRIES}"
        );
        Tlb {
            entries: vec![0; entries],
            k: entries.trailing_zeros(),
            walks: 0,
            sweeps: 0,
            write_backs: 0,
            written_bits: [0; 3],
            refusals: 0,
            held_ns: 0,
            sweep_until: 0,
        }
    }

    /// Its entries, N.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Never: N is at least [`MIN_ENTRIES`].
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The index of `va`, `VA<9+k:10>`.
    pub fn index(&self, va: u32) -> usize {
        (va >> 10) as usize & (self.entries.len() - 1)
    }

    /// The tag of `va`, `VA<31:10+k>`.
    pub fn tag(&self, va: u32) -> u64 {
        u64::from(va >> (10 + self.k))
    }

    /// The entry for `va`, if the TLB holds it.
    pub fn lookup(&self, va: u32) -> Option<u32> {
        let e = self.entries[self.index(va)];
        (e & VALID != 0 && (e >> 32) & 0x7fff_ffff == self.tag(va)).then_some(e as u32 & ENTRY_BITS)
    }

    /// The entry at `index`, valid or not, as `(valid, tag, <29:0>)`: for
    /// the tests.
    pub fn at(&self, index: usize) -> (bool, u64, u32) {
        let e = self.entries[index];
        (e & VALID != 0, (e >> 32) & 0x7fff_ffff, e as u32 & ENTRY_BITS)
    }

    /// Loads `entry`'s `<29:0>` for `va`: a walk's fill, or a direct write.
    pub fn load(&mut self, va: u32, entry: u32) {
        let i = self.index(va);
        self.entries[i] = VALID | self.tag(va) << 32 | u64::from(entry & ENTRY_BITS);
    }

    /// ORs `bits` into the entry for `va`, if the TLB holds it.
    pub fn or(&mut self, va: u32, bits: u32) {
        if self.lookup(va).is_some() {
            let i = self.index(va);
            self.entries[i] |= u64::from(bits & ENTRY_BITS);
        }
    }

    /// Clears the entry at `va`'s index, whatever its tag.
    pub fn invalidate(&mut self, va: u32) {
        let i = self.index(va);
        self.entries[i] = 0;
    }

    /// Clears every entry's valid bit: the sweep, all at once. On `rtl` the
    /// caller times it, [`Tlb::sweep_until`].
    pub fn sweep(&mut self) {
        self.entries.iter_mut().for_each(|e| *e = 0);
        self.sweeps += 1;
    }

    /// How long a sweep takes: N ticks.
    pub fn sweep_ns(&self) -> u64 {
        self.entries.len() as u64 * SWEEP_TICK_NS
    }
}

/// **A `WRITE-MAP` operation** (A14.4): the operation in `VMA<33:32>`, 0
/// none, 1 direct write, 2 invalidate, 3 empty; the address `MD`'s. An
/// operation whose `MD<31:29>` is `111`, a window, does nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    None,
    Write { va: u32, entry: u32 },
    Invalidate { va: u32 },
    Empty,
}

impl Operation {
    pub fn of(vma: crate::machine::Word, md: crate::machine::Word) -> Operation {
        let va = md as u32;
        if va >> 29 == 0b111 {
            return Operation::None;
        }
        match (vma >> 32) & 3 {
            1 => Operation::Write { va, entry: vma as u32 & ENTRY_BITS },
            2 => Operation::Invalidate { va },
            3 => Operation::Empty,
            _ => Operation::None,
        }
    }
}

/// **The memory system's words** of the register page, 220-227 (A14.9):
/// the memory port's, not a device's, so RESET-DEVICES (word 104) leaves
/// them; `-RESET` clears them, as it sweeps the TLB.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Words {
    /// Word 220, the directory base: `<17:0>` the directory's first frame,
    /// a multiple of 4; 0 is no directory. The walk takes `<17:2>`.
    pub directory: u32,
    /// Word 221 `<0>`, the ephemeral-reference enable. Read and written
    /// here; the setter that reads it is S1b's.
    pub ephemeral: bool,
    /// Words 222 and 223, the pointer-type register: `<k>` of 222 type k,
    /// of 223 type 32 + k.
    pub pointer_types: u64,
    /// Word 224, the write-backs the guard refused: read, and cleared by a
    /// write. Counted by S1b's write-backs.
    pub refused: u32,
}

impl Words {
    /// Register-page word `k`, if it is one of these (220-227); 225-227
    /// are reserved and read 0.
    pub fn read(&self, k: u32) -> Option<u32> {
        Some(match k {
            0o220 => self.directory,
            0o221 => self.ephemeral as u32,
            0o222 => self.pointer_types as u32,
            0o223 => (self.pointer_types >> 32) as u32,
            0o224 => self.refused,
            0o225..=0o227 => 0,
            _ => return None,
        })
    }

    /// A write of word `k`; whether it is one of these.
    pub fn write(&mut self, k: u32, v: u32) -> bool {
        match k {
            0o220 => self.directory = v & 0o777777,
            0o221 => self.ephemeral = v & 1 != 0,
            0o222 => self.pointer_types = (self.pointer_types & !0xffff_ffff) | u64::from(v),
            0o223 => self.pointer_types = (self.pointer_types & 0xffff_ffff) | u64::from(v) << 32,
            0o224 => self.refused = 0,
            0o225..=0o227 => {}
            _ => return false,
        }
        true
    }

    /// Whether data type `t` (`<37:32>` of a word) is in the pointer-type
    /// register.
    pub fn pointer_type(&self, word: crate::machine::Word) -> bool {
        self.pointer_types >> ((word >> 32) & 0o77) & 1 != 0
    }

    pub fn save(&self, w: &mut crate::checkpoint::Writer) {
        let Words { directory, ephemeral, pointer_types, refused } = *self;
        w.u32(directory);
        w.bool(ephemeral);
        w.u64(pointer_types);
        w.u32(refused);
    }

    pub fn load(r: &mut crate::checkpoint::Reader) -> std::io::Result<Words> {
        Ok(Words {
            directory: r.u32()?,
            ephemeral: r.bool()?,
            pointer_types: r.u64()?,
            refused: r.u32()?,
        })
    }
}
