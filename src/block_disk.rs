// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's block-disk: a disk of numbered blocks behind the CADR disk
//! controller's own programming interface, `--disk-controller block-disk`.
//!
//! Its four registers are words 200-203 of the register page, `1777777600`
//! to `1777777603` (contract Q13).
//!
//! What stays the CADR's (`sys/doc/disk.text`, and `crate::disk_controller`,
//! which models MIT's board): the four registers --- status and command,
//! the command list pointer, the disk address, and START, in the order the
//! CADR has them at Xbus `17377774` --- and the command list itself, one
//! command word a page, its `<0>` More; the done interrupt enable, command
//! `<11>`; and the disk address left at the last block moved, or at the
//! one that failed.
//!
//! What goes: cylinders, heads and sectors, seeks, the ECC, Read All and
//! Write All, the drive's own states. The disk address is a block number
//! from the start of the disk, `<27:0>`; the commands are read, 0, and
//! write, 11; anything else stops by error, and so does a transfer that
//! runs past the end of the disk. The disk is a file of any size, raw or a
//! VHD, [`crate::disk_image::Disk`]: block `n` is its 512-byte sectors `2n`
//! and `2n + 1` (contract Q8).
//!
//! The words move inside the store to START, as the CADR model's do; the
//! controller then stays busy for [`BLOCK_NS`] a block moved, which is when
//! it goes not-active and the done interrupt comes.
//!
//! **Pages** (contract G2 §4.2, appendix A1.11): a transfer moves 1024-word
//! pages of 40-bit words, one a command list entry, `<27:10>` the page and
//! `<0>` More, `<9:1>` and `<39:28>` ignored; the registers take 28-bit
//! addresses. Command `<12>` chooses the transfer: 0 the **packed
//! transfer**, 5 blocks a page, the 5,120 bytes as main memory holds them
//! (G1 §4.1: word w at bytes 5w to 5w + 4, `<7:0>` first and the tag
//! last); 1 the **4-byte transfer**, 4 blocks a page, `<31:0>` of word w at
//! bytes 4w to 4w + 3, a read writing tag `005` and a write dropping the
//! tag. A page is read whole before memory is written, so a transfer that
//! runs past the end of the disk leaves the page it stopped in as it was.

use crate::disk_image::Disk;
use crate::disk_unit::BLOCK_WORDS;

/// The registers' first physical address: word 200 of the register page
/// (contract Q13).
pub const REGS: u32 = 0o1777777600;
/// The four registers, by number. Status reads and command writes are the
/// first; START is written, and reads 0.
pub const STATUS: u32 = 0;
pub const COMMAND: u32 = 0;
pub const CLP: u32 = 1;
pub const DA: u32 = 2;
pub const START: u32 = 3;

/// A block's time, the default: 100 us, what an SD card in four-bit mode
/// at 25 MHz takes for a kilobyte and its command, near enough.
/// **Unverified**: an estimate until muir-fpga measures its disk path.
pub const BLOCK_NS: u64 = 100_000;

/// One block moved, as [`BlockDisk::log`] records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transfer {
    /// A write of the disk; a read otherwise, which writes main memory.
    pub write: bool,
    /// The block, from the start of the disk.
    pub block: u32,
    /// The first of the 1024 words of physical memory, the page, it moved
    /// to or from.
    pub page: u32,
}

/// The block-disk: its registers, its error flags, and its disk.
#[derive(Clone)]
pub struct BlockDisk {
    cmd: u32,
    clp: u32,
    da: u32,
    /// The last address the command list made the disk read or write.
    last_memory_address: u32,
    /// When the transfer in flight is done.
    done_at: u64,
    now: u64,
    /// A block's time, in ns.
    pub block_ns: u64,
    /// Units of the machine's time a nanosecond: 1, or 2 on QUUX revision
    /// 15, which counts 0.5 ns ([`crate::clock::TimeBase`]). The machine
    /// sets it as it hands the disk a register's write; not kept in a
    /// checkpoint, being the machine's.
    pub unit: u64,
    past_end: bool,
    nxm: bool,
    bad_command: bool,
    disk: Option<Disk>,
    /// Every block moved, in order, when a test or a trace asks for the
    /// record by setting it to `Some`. Not kept in a checkpoint.
    pub log: Option<Vec<Transfer>>,
}

impl BlockDisk {
    pub fn new(block_ns: u64) -> BlockDisk {
        BlockDisk {
            cmd: 0,
            clp: 0,
            da: 0,
            last_memory_address: 0,
            done_at: 0,
            now: 0,
            block_ns,
            unit: 1,
            past_end: false,
            nxm: false,
            bad_command: false,
            disk: None,
            log: None,
        }
    }

    pub fn attach(&mut self, disk: Disk) {
        self.disk = Some(disk);
    }

    pub fn disk_mut(&mut self) -> Option<&mut Disk> {
        self.disk.as_mut()
    }

    /// The machine's clock, told before the disk is read, written or asked
    /// for its interrupt.
    pub fn advance(&mut self, now: u64) {
        self.now = now;
    }

    fn not_active(&self) -> bool {
        self.now >= self.done_at
    }

    fn error(&self) -> bool {
        self.past_end || self.nxm || self.bad_command
    }

    /// The CADR's done interrupt: not active with command `<11>` set.
    pub fn interrupt(&self) -> bool {
        self.interrupt_at(self.now)
    }

    /// The done interrupt at `now`, for a machine whose clock has moved on
    /// since it last spoke to the disk.
    pub fn interrupt_at(&self, now: u64) -> bool {
        now >= self.done_at && self.cmd & (1 << 11) != 0
    }

    /// The status word: `<0>` not active, `<3>` interrupt request, `<9>` no
    /// pack, `<13>` stopped by error, `<17>` past the end of the pack,
    /// `<20>` NXM, the CADR's bits for the same things.
    pub fn status(&self) -> u32 {
        let mut v = 0;
        if self.not_active() {
            v |= 1;
        }
        if self.interrupt() {
            v |= 1 << 3;
        }
        if self.disk.is_none() {
            v |= 1 << 9;
        }
        if self.error() {
            v |= 1 << 13;
        }
        if self.past_end {
            v |= 1 << 17;
        }
        if self.nxm {
            v |= 1 << 20;
        }
        v
    }

    pub fn read(&self, register: u32) -> u32 {
        match register & 3 {
            STATUS => self.status(),
            CLP => self.last_memory_address,
            DA => self.da,
            _ => 0,
        }
    }

    /// A register written ([module docs](self)): the command list pointer
    /// and the disk address 28 bits, and START a transfer of pages. `main`
    /// is physical memory, main memory's words from 0, which a transfer
    /// reads and writes directly, the disk being a bus master.
    pub fn write(&mut self, register: u32, v: u32, main: &mut [crate::machine::Word]) {
        match register & 3 {
            COMMAND => {
                self.cmd = v;
                self.past_end = false;
                self.nxm = false;
                self.bad_command = false;
            }
            CLP => self.clp = v & 0o1777777777,
            DA => self.da = v & 0o1777777777,
            _ => self.start(main),
        }
    }

    fn start(&mut self, main: &mut [crate::machine::Word]) {
        use crate::machine::{UNBOXED_TAG, Word};
        const PAGE: usize = 1024;
        self.past_end = false;
        self.nxm = false;
        self.bad_command = false;
        let read = match self.cmd & 0o17 {
            0o00 => true,
            0o11 => false,
            _ => {
                self.bad_command = true;
                return;
            }
        };
        // Blocks a page, and a page's bytes.
        let four = self.cmd & (1 << 12) != 0;
        let (per_page, bytes) = if four { (4usize, 4usize) } else { (5, 5) };
        let Some(mut disk) = self.disk.take() else { return };
        let mut moved = 0u64;
        let mut n = 0u32;
        let mut block = self.da;
        loop {
            // "Only bits <15:0> of the CLP can count", as on the CADR.
            let clp = self.clp & !0xffff | (self.clp.wrapping_add(n)) & 0xffff;
            self.last_memory_address = clp;
            let Some(&ccw) = main.get(clp as usize) else {
                self.nxm = true;
                break;
            };
            // Main memory, which ends below the frame buffer window (G1
            // §3.2): a page in the window is outside it.
            let page = (ccw & 0o1777776000) as usize;
            if page + PAGE > main.len() {
                self.nxm = true;
                break;
            }
            let mut failed = None;
            if read {
                let mut buf = Vec::with_capacity(per_page * 1024);
                for k in 0..per_page as u32 {
                    match disk.read_block(block + k) {
                        Some(b) => buf.extend(b.iter().flat_map(|w| w.to_le_bytes())),
                        None => {
                            failed = Some(block + k);
                            break;
                        }
                    }
                }
                if failed.is_none() {
                    for (w, m) in main[page..page + PAGE].iter_mut().enumerate() {
                        let b = &buf[bytes * w..bytes * w + bytes];
                        *m = if four {
                            UNBOXED_TAG | Word::from(u32::from_le_bytes(b.try_into().unwrap()))
                        } else {
                            b.iter().rev().fold(0, |x, &y| x << 8 | Word::from(y))
                        };
                    }
                }
            } else {
                let buf: Vec<u8> = main[page..page + PAGE]
                    .iter()
                    .flat_map(|&w| w.to_le_bytes().into_iter().take(bytes))
                    .collect();
                for k in 0..per_page {
                    let b: [u32; BLOCK_WORDS] = std::array::from_fn(|j| {
                        let at = 1024 * k + 4 * j;
                        u32::from_le_bytes(buf[at..at + 4].try_into().unwrap())
                    });
                    if !disk.write_block(block + k as u32, &b) {
                        failed = Some(block + k as u32);
                        break;
                    }
                }
            }
            if let Some(at) = failed {
                self.past_end = true;
                moved += (at - block) as u64;
                block = at;
                break;
            }
            if let Some(log) = self.log.as_mut() {
                for k in 0..per_page as u32 {
                    log.push(Transfer { write: !read, block: block + k, page: page as u32 });
                }
            }
            self.last_memory_address = (page + PAGE - 1) as u32;
            moved += per_page as u64;
            block += per_page as u32 - 1;
            if ccw & 1 == 0 {
                break;
            }
            n += 1;
            block += 1;
        }
        // The last block moved, or the one that failed.
        self.da = block;
        self.disk = Some(disk);
        // On revision 15 DONE is START's clock plus ⌈blocks·200,000/P⌉
        // clocks (MP2b ruling Q2): START is at a clock, and the status is
        // read at clocks, so the exact end is enough.
        self.done_at = self.now + moved * self.block_ns * self.unit;
    }

    /// `-XBUS INIT`: the command and the errors cleared, as a reset does.
    pub fn xbus_init(&mut self) {
        self.cmd = 0;
        self.past_end = false;
        self.nxm = false;
        self.bad_command = false;
        self.done_at = self.now;
    }

    pub fn save(&self, w: &mut crate::checkpoint::Writer) {
        for v in [self.cmd, self.clp, self.da, self.last_memory_address] {
            w.u32(v);
        }
        w.u64(self.done_at);
        w.u64(self.now);
        w.u64(self.block_ns);
        w.bool(self.past_end);
        w.bool(self.nxm);
        w.bool(self.bad_command);
        w.opt(self.disk.as_ref(), |w, d| d.save(w));
    }

    /// Back from a checkpoint, onto a block-disk whose disk is already
    /// attached.
    pub fn load(&mut self, r: &mut crate::checkpoint::Reader) -> std::io::Result<()> {
        self.cmd = r.u32()?;
        self.clp = r.u32()?;
        self.da = r.u32()?;
        self.last_memory_address = r.u32()?;
        self.done_at = r.u64()?;
        self.now = r.u64()?;
        self.block_ns = r.u64()?;
        self.past_end = r.bool()?;
        self.nxm = r.bool()?;
        self.bad_command = r.bool()?;
        let has_disk = r.bool()?;
        match (has_disk, self.disk.as_mut()) {
            (true, Some(d)) => d.load(r)?,
            (false, None) => {}
            _ => return Err(crate::checkpoint::bad("block-disk: a disk in one and not the other")),
        }
        Ok(())
    }
}
