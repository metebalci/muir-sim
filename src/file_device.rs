// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **QUUX's file device** (contract Q9, revision 9): folders of the host
//! served to the machine under one pathname host, `HOST`, through two rings
//! in main memory. The processor writes commands into the command ring and
//! the device answers each, in order, with an entry in the response ring;
//! the bytes move by DMA between main memory and the host's files. The
//! registers are words 160-171 of the register page, and its interrupt is
//! word 100 `<7>` (contract Q13). QUUX has it and the CADR does not.
//!
//! The registers ([`CONTROL`] to [`RESP_CONS`]): 160 control, `<0>` enable
//! and `<8>` interrupt enable; 161 status, `<0>` enabled, `<1>` quiet,
//! `<2>` configuration refused, `<3>` index fault, `<8>` a response waiting,
//! `<23:16>` the handles open; 162 and 163 the command ring's base, a
//! physical word address on an 8-word line, and the log2 of its entries,
//! 0-8; 164 the command producer index (the processor's) and 165 the
//! command consumer (the device's); 166 and 167 the response ring's base
//! and size; 170 the response producer (the device's) and 171 the response
//! consumer (the processor's). Indexes are 16 bits and free-running; a
//! slot is the index mod the size. An entry is 8 words, the layout's
//! (`execute`'s comments say which word is which).
//!
//! **Addresses and words** (contract G1 §4.4, G2 §4.3, appendix A1.10): the
//! rings' bases and the buffers' addresses are 28-bit physical word
//! addresses on an 8-word line; a buffer holds 4 bytes a word, in `<31:0>`; every
//! word the device writes, a buffer's or a response's, is a fixnum, tag
//! `005`; and it reads `<31:0>` of what it reads, whatever the tag.
//!
//! **Time.** A command completes at its due time ([`due`]): 20 us and
//! 100 us a KiB of the buffer lengths its entry names, after the latest of
//! its producer write landing (once the processor's write buffer is empty),
//! the previous command's completion, and a response slot coming free. At
//! that instant, in one step, the device reads the entry and buffer A, does
//! the host's operation, writes buffer B and the response entry, and moves
//! 165 and 170: nothing in memory changes before, and nothing completes
//! within the producer write. The constants are **unverified**, estimates
//! until muir-fpga measures its path.
//!
//! **Names** are absolute, printable ASCII (040-176), components of 1-255
//! bytes without `.` and `..`, at most 1,024 bytes; case is exact. Bytes
//! move as they are: the device never translates.
//!
//! **Mounts** ([`Mounts`], `--file-root`): a default folder is HOST's `/`,
//! and named folders are the top-level directories of their names,
//! overriding the default folder's entry of that name; each may be
//! read-only. With no default folder `/` holds the mounts alone and is
//! read-only.

use crate::machine::MemoryWord;
use std::collections::BTreeMap;
use std::fs::{self, File, Metadata};
use std::io;
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The registers, by their word on the register page.
pub const CONTROL: u32 = 0o160;
pub const STATUS: u32 = 0o161;
pub const CMD_BASE: u32 = 0o162;
pub const CMD_SIZE: u32 = 0o163;
pub const CMD_PROD: u32 = 0o164;
pub const CMD_CONS: u32 = 0o165;
pub const RESP_BASE: u32 = 0o166;
pub const RESP_SIZE: u32 = 0o167;
pub const RESP_PROD: u32 = 0o170;
pub const RESP_CONS: u32 = 0o171;

/// The device's bit in word 100 (contract Q13).
pub const INTERRUPT_BIT: u32 = 7;

/// A command's own time, before its buffers'. **Unverified**: an estimate
/// of the boards' round trip --- the fabric raising Linux, a daemon doing a
/// syscall, a doorbell back --- until muir-fpga measures it. Nonzero, so
/// that nothing completes within the producer write.
pub const COMMAND_NS: u64 = 20_000;
/// Each KiB of the buffer lengths a command names. **Unverified**:
/// block-disk's time for a 1 KiB block ([`crate::block_disk::BLOCK_NS`]),
/// itself an estimate.
pub const KIB_NS: u64 = 100_000;

/// The handles, numbered 1 to this.
pub const MAX_HANDLES: usize = 64;
/// The largest buffer, in bytes.
pub const MAX_BUFFER: u32 = 65_536;
/// The least room DIRECTORY takes: a record of the longest name.
pub const MIN_DIRECTORY_BUFFER: u32 = 272;
/// The longest name, and the longest component of one.
pub const MAX_NAME: usize = 1024;
pub const MAX_COMPONENT: usize = 255;
/// The longest line LOG takes.
pub const MAX_LOG: u32 = 1024;
/// How a write's temporary file's name begins: DIRECTORY and COMPLETE
/// never show such a name.
pub const TEMP_PREFIX: &str = ".quux-write-";

/// The opcodes, word 0 `<23:16>` of a command.
pub mod op {
    pub const OPEN: u32 = 1;
    pub const READ: u32 = 2;
    pub const WRITE: u32 = 3;
    pub const CLOSE: u32 = 4;
    pub const DIRECTORY: u32 = 5;
    pub const COMPLETE: u32 = 6;
    pub const DELETE: u32 = 7;
    pub const RENAME: u32 = 8;
    pub const CREATE_DIRECTORY: u32 = 9;
    pub const LOG: u32 = 10;
}

/// The statuses, word 0 `<23:16>` of a response: the Lisp system's file
/// errors by their three letters, and three driver faults.
pub mod status {
    pub const OK: u32 = 0;
    /// The last component does not exist.
    pub const FNF: u32 = 1;
    /// A directory on the way does not exist or is a file; an unmounted name.
    pub const DNF: u32 = 2;
    /// OPEN write with if-exists error; CREATE-DIRECTORY on a file.
    pub const FAE: u32 = 3;
    /// RENAME onto an existing name.
    pub const REF: u32 = 4;
    /// The host refuses; a symlink leaving its mount; a loop; a mount's root
    /// deleted or renamed.
    pub const ACC: u32 = 5;
    /// A write under a read-only mount.
    pub const ATF: u32 = 6;
    /// CREATE-DIRECTORY of an existing directory.
    pub const DAE: u32 = 7;
    /// DELETE of a directory that is not empty.
    pub const DNE: u32 = 8;
    /// The host is full.
    pub const NMR: u32 = 9;
    /// OPEN read or write of a directory.
    pub const IOD: u32 = 10;
    /// Neither file nor directory; a file of 2^32 bytes or more; DIRECTORY
    /// of a file.
    pub const WKF: u32 = 11;
    /// A name against the rules.
    pub const IPS: u32 = 12;
    /// All 64 handles open.
    pub const NER: u32 = 13;
    /// An opcode not among the ten.
    pub const UOP: u32 = 14;
    /// The host's I/O error, and any error not named here.
    pub const DAT: u32 = 15;
    /// READ past the end; WRITE leaving a hole or past 2^32 - 1.
    pub const FOR: u32 = 16;
    /// RENAME across mounts.
    pub const RAD: u32 = 17;
    /// Not open, or the wrong kind for the command.
    pub const BAD_HANDLE: u32 = 64;
    /// Off a line, over 65,536 bytes, or outside main memory.
    pub const BAD_BUFFER: u32 = 65;
    /// Flags out of range, a DIRECTORY buffer under 272 bytes, a LOG line
    /// over 1,024 bytes, a completion longer than B.
    pub const BAD_ARGUMENT: u32 = 66;
}

/// When a command taken at `start` naming buffers of `a` and `b` bytes
/// completes: [`COMMAND_NS`] and [`KIB_NS`] a KiB, rounded up to the ns. A
/// length past [`MAX_BUFFER`], which answers bad buffer, is charged as
/// [`MAX_BUFFER`].
pub fn due(start: u64, a: u32, b: u32) -> u64 {
    due_on(crate::clock::TimeBase::NS, start, a, b)
}

/// [`due`] in the machine's time `time` ([`crate::machine::Machine::time_base`]):
/// on revision 15 the duration is reckoned exactly, rounded up to its
/// units of 0.5 ns, and the completion falls at the first clock at or after
/// it, so the whole duration is rounded up to clocks once, from its own
/// start, never per term or via whole ns (contract G3 revision 15, §9; MP2b
/// ruling Q2): 20 µs and 100 µs a KiB is ⌈(40,000·1,024 + 200,000·bytes) /
/// (1,024·P)⌉ clocks at a period of P units.
pub fn due_on(time: crate::clock::TimeBase, start: u64, a: u32, b: u32) -> u64 {
    let bytes = a.min(MAX_BUFFER) as u64 + b.min(MAX_BUFFER) as u64;
    let units = (COMMAND_NS * 1024 + bytes * KIB_NS) * time.per_ns;
    time.grid(start + units.div_ceil(1024))
}

// --- mounts -------------------------------------------------------------------------

/// A host folder served to the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    /// The folder, as given.
    pub given: PathBuf,
    /// The folder with its symlinks resolved: nothing below it resolves
    /// outside it.
    pub path: PathBuf,
    /// Every write under it answers ATF.
    pub ro: bool,
}

/// HOST's folders: `--file-root`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mounts {
    /// HOST's `/`, if a folder is.
    pub default: Option<Mount>,
    /// The top-level directories mounted by name.
    pub named: BTreeMap<String, Mount>,
}

impl Mounts {
    /// Takes one `--file-root` value: `<name>=<folder>[,ro]` when the text
    /// before its first `=` is a valid component, else `<folder>[,ro]`, the
    /// default folder. The folder must be one. A name given twice, and a
    /// second default folder, are refused.
    pub fn add(&mut self, spec: &str) -> Result<(), String> {
        let (spec, ro) = match spec.strip_suffix(",ro") {
            Some(s) => (s, true),
            None => (spec, false),
        };
        let (name, folder) = match spec.split_once('=') {
            Some((n, f)) if valid_component(n.as_bytes()) => (Some(n), f),
            _ => (None, spec),
        };
        let given = PathBuf::from(folder);
        let path = fs::canonicalize(&given).map_err(|e| format!("{}: {e}", given.display()))?;
        if !path.is_dir() {
            return Err(format!("{} is not a folder", given.display()));
        }
        let mount = Mount { given, path, ro };
        match name {
            Some(n) => {
                if self.named.contains_key(n) {
                    return Err(format!("{n} is mounted twice"));
                }
                self.named.insert(n.to_string(), mount);
            }
            None => {
                if self.default.is_some() {
                    return Err("a second default folder: there is one default, HOST's /".into());
                }
                self.default = Some(mount);
            }
        }
        Ok(())
    }

    /// What the start says: a line a mount.
    pub fn describe(&self) -> Vec<String> {
        let rw = |ro: bool| if ro { "read-only" } else { "read-write" };
        let mut out = Vec::new();
        match &self.default {
            Some(m) => out.push(format!("/ is {}, {}", m.given.display(), rw(m.ro))),
            None if self.named.is_empty() => {
                out.push("/ is empty and read-only: nothing is mounted".into())
            }
            None => out.push("/ holds the mounts alone, read-only".into()),
        }
        for (n, m) in &self.named {
            out.push(format!("/{n} is {}, {}", m.given.display(), rw(m.ro)));
        }
        out
    }
}

/// A name's component: 1-255 bytes in 040-176 other than `/`, and not `.`
/// or `..`.
fn valid_component(c: &[u8]) -> bool {
    (1..=MAX_COMPONENT).contains(&c.len())
        && c.iter().all(|&b| (0o40..=0o176).contains(&b) && b != b'/')
        && c != b"."
        && c != b".."
}

/// A name's components, or IPS: absolute, at most 1,024 bytes, a single
/// trailing `/` allowed.
fn parse_name(bytes: &[u8]) -> Result<Vec<String>, u32> {
    if bytes.is_empty() || bytes.len() > MAX_NAME || bytes[0] != b'/' {
        return Err(status::IPS);
    }
    let mut body = &bytes[1..];
    if let Some(b) = body.strip_suffix(b"/") {
        body = b;
    }
    if body.is_empty() {
        return Ok(Vec::new());
    }
    body.split(|&b| b == b'/')
        .map(|c| {
            if valid_component(c) {
                Ok(String::from_utf8(c.to_vec()).expect("printable ASCII"))
            } else {
                Err(status::IPS)
            }
        })
        .collect()
}

/// A host error's status: the table's, EINVAL a name the host refuses.
pub fn host_status(e: &io::Error) -> u32 {
    use io::ErrorKind as K;
    // ELOOP, a symlink loop: Linux's number, and the BSDs'.
    const ELOOP: i32 = if cfg!(target_os = "linux") { 40 } else { 62 };
    if e.raw_os_error() == Some(ELOOP) {
        return status::ACC;
    }
    match e.kind() {
        K::NotFound => status::FNF,
        K::NotADirectory => status::DNF,
        K::PermissionDenied => status::ACC,
        K::ReadOnlyFilesystem => status::ATF,
        K::StorageFull | K::QuotaExceeded => status::NMR,
        K::InvalidInput | K::InvalidFilename => status::IPS,
        K::DirectoryNotEmpty => status::DNE,
        K::AlreadyExists => status::FAE,
        K::IsADirectory => status::IOD,
        _ => status::DAT,
    }
}

/// A file's modification time in whole Unix seconds, 0 before 1970 and
/// held at 2^32-1 after 2106.
fn mtime_of(m: &Metadata) -> u32 {
    m.mtime().clamp(0, u32::MAX as i64) as u32
}

/// Whether `name`, found under `dir` with metadata `found`, is spelled
/// there as asked. A file system that folds case finds a name spelled
/// otherwise, and so does the name's case-flipped twin, as the same file:
/// then the folder's own list decides. **Unverified** on a folding file
/// system: muir's tests run on Linux's, which fold nothing.
fn exact_case(dir: &Path, name: &str, found: &Metadata) -> bool {
    if !name.bytes().any(|b| b.is_ascii_alphabetic()) {
        return true;
    }
    let flipped: String =
        name.chars()
            .map(|c| {
                if c.is_ascii_lowercase() { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() }
            })
            .collect();
    match fs::symlink_metadata(dir.join(&flipped)) {
        Ok(m) if (m.dev(), m.ino()) == (found.dev(), found.ino()) => fs::read_dir(dir)
            .map(|rd| rd.flatten().any(|e| e.file_name().as_encoded_bytes() == name.as_bytes()))
            .unwrap_or(false),
        _ => true,
    }
}

/// A rename that never replaces what is at `to`: `renameat2`'s
/// `RENAME_NOREPLACE` on Linux, atomic; elsewhere, or where the file
/// system has no such flag, a look and then a rename.
fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::ffi::{CString, c_char, c_int, c_uint};
        use std::os::unix::ffi::OsStrExt;
        unsafe extern "C" {
            fn renameat2(
                olddirfd: c_int,
                oldpath: *const c_char,
                newdirfd: c_int,
                newpath: *const c_char,
                flags: c_uint,
            ) -> c_int;
        }
        const AT_FDCWD: c_int = -100;
        const RENAME_NOREPLACE: c_uint = 1;
        let f = CString::new(from.as_os_str().as_bytes())?;
        let t = CString::new(to.as_os_str().as_bytes())?;
        // SAFETY: two NUL-terminated paths that outlive the call.
        let r = unsafe { renameat2(AT_FDCWD, f.as_ptr(), AT_FDCWD, t.as_ptr(), RENAME_NOREPLACE) };
        if r == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        // EINVAL: a file system without the flag (or a folder into itself,
        // which the rename below refuses the same way); ENOSYS: no call.
        if !matches!(e.raw_os_error(), Some(22 | 38)) {
            return Err(e);
        }
    }
    if fs::symlink_metadata(to).is_ok() {
        return Err(io::Error::from(io::ErrorKind::AlreadyExists));
    }
    fs::rename(from, to)
}

/// The text LOG writes: `log: ` and the line, a byte outside 040-176 as a
/// backslash and three octal digits.
pub fn log_line(bytes: &[u8]) -> String {
    let mut s = String::from("log: ");
    for &b in bytes {
        if (0o40..=0o176).contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("\\{b:03o}"));
        }
    }
    s
}

// --- handles ---------------------------------------------------------------------

/// A write's temporary file, beside its target. Removed when the last
/// clone of the machine holding it goes, if it is still there: a run that
/// ends with a write open leaves nothing behind.
#[derive(Debug)]
struct Temp {
    path: PathBuf,
    file: File,
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// An open handle. The host file is shared by a clone of the machine.
#[derive(Clone, Debug)]
enum Handle {
    Read(Arc<File>),
    Write {
        temp: Arc<Temp>,
        /// Where CLOSE puts it.
        target: PathBuf,
        /// If-exists error: CLOSE refuses a name that appeared meanwhile.
        noreplace: bool,
        /// The bytes the temporary file holds.
        held: u64,
    },
}

static TEMPS: AtomicU64 = AtomicU64::new(0);

// --- where a name is ------------------------------------------------------------

/// Which mount a name is under.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Key {
    Default,
    Named(String),
    /// No default folder: `/` itself, read-only.
    Bare,
}

/// A name, placed.
struct Place {
    key: Key,
    /// The mount's folder, resolved; none for [`Key::Bare`].
    root: Option<PathBuf>,
    ro: bool,
    /// The components below the mount's folder.
    rel: Vec<String>,
}

impl Place {
    fn is_mount_root(&self) -> bool {
        self.rel.is_empty()
    }
}

/// A name looked up in its mount.
struct Found {
    /// The folder it is in, resolved.
    parent: PathBuf,
    /// Its path, a symlink at the end followed if asked.
    path: PathBuf,
    /// What is there, if anything.
    meta: Option<Metadata>,
}

/// A directory entry as DIRECTORY and COMPLETE give it.
#[derive(Clone, Debug)]
struct Entry {
    name: String,
    dir: bool,
    ro: bool,
    len: u64,
    mtime: u32,
}

fn entry_of(name: String, m: &Metadata, ro: bool, date: Option<u32>) -> Entry {
    Entry {
        name,
        dir: m.is_dir(),
        ro,
        len: if m.is_dir() { 0 } else { m.len() },
        mtime: date.unwrap_or_else(|| mtime_of(m)),
    }
}

/// A folder's entries that DIRECTORY lists: files and directories, sorted
/// bytewise, dot files with them; not `.quux-write-` files, not a name the
/// rules refuse, not a symlink that leaves `root` or leads nowhere, and not
/// anything else.
fn list_folder(dir: &Path, root: &Path, ro: bool, date: Option<u32>) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for e in fs::read_dir(dir)? {
        let e = e?;
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if !valid_component(name.as_bytes()) || name.starts_with(TEMP_PREFIX) {
            continue;
        }
        let p = e.path();
        let Ok(lm) = fs::symlink_metadata(&p) else { continue };
        let m = if lm.file_type().is_symlink() {
            match fs::canonicalize(&p) {
                Ok(real) if real.starts_with(root) => match fs::metadata(&real) {
                    Ok(m) => m,
                    Err(_) => continue,
                },
                _ => continue,
            }
        } else {
            lm
        };
        if m.is_file() || m.is_dir() {
            out.push(entry_of(name.to_string(), &m, ro, date));
        }
    }
    out.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
    Ok(out)
}

/// A buffer an entry names: its first word and its length in bytes.
#[derive(Clone, Copy)]
struct Buf {
    at: usize,
    len: u32,
}

fn buffer<W: MemoryWord>(main: &[W], addr: u32, len: u32, layout: Layout) -> Result<Buf, u32> {
    let at = (addr & layout.address) as usize;
    if at & layout.line != 0 || len > MAX_BUFFER || at + (len as usize).div_ceil(4) > main.len() {
        return Err(status::BAD_BUFFER);
    }
    Ok(Buf { at, len })
}

/// Byte k of a buffer is `<8(k mod 4)+7 : 8(k mod 4)>` of word k/4.
fn bytes_of<W: MemoryWord>(main: &[W], b: Buf) -> Vec<u8> {
    (0..b.len as usize).map(|k| (main[b.at + k / 4].low() >> (8 * (k % 4))) as u8).collect()
}

/// `data` into the buffer at `at`: `ceil(n/4)` words, the bytes past n in
/// the last one 0, and no other word; each word tagged `tag`.
fn put_bytes<W: MemoryWord>(main: &mut [W], at: usize, data: &[u8], tag: u8) {
    for (k, chunk) in data.chunks(4).enumerate() {
        let mut w = [0u8; 4];
        w[..chunk.len()].copy_from_slice(chunk);
        main[at + k] = W::tagged(u32::from_le_bytes(w), tag);
    }
}

/// The machine's addresses and words, as the device takes them (G1 §4.4,
/// G2 §4.3, appendix A1.10): 28-bit addresses on an 8-word line, and every
/// word it writes a fixnum, tag `005`.
#[derive(Clone, Copy)]
struct Layout {
    address: u32,
    line: usize,
    tag: u8,
}

const LAYOUT: Layout = Layout { address: 0o1777777777, line: 7, tag: 0o005 };

/// The words of a response past word 0.
#[derive(Clone, Copy, Default)]
struct Reply([u32; 7]);

impl Reply {
    fn count(&mut self, v: u32) {
        self.0[0] = v;
    }
    fn handle(&mut self, v: u32) {
        self.0[1] = v;
    }
    fn len(&mut self, v: u64) {
        self.0[2] = v.min(u32::MAX as u64) as u32;
    }
    fn mtime(&mut self, v: u32) {
        self.0[3] = v;
    }
    fn flags(&mut self, v: u32) {
        self.0[4] = v;
    }
    fn w6(&mut self, v: u32) {
        self.0[5] = v;
    }
}

// --- the device --------------------------------------------------------------------

/// The file device: its registers, its rings' indexes, its handles, and the
/// folders it serves.
#[derive(Clone, Debug)]
pub struct FileDevice {
    /// The folders, `--file-root`'s: the flags' say, not a checkpoint's.
    pub mounts: Mounts,
    enabled: bool,
    interrupt_enable: bool,
    refused: bool,
    index_fault: bool,
    cmd_base: u32,
    cmd_log2: u32,
    resp_base: u32,
    resp_log2: u32,
    cmd_prod: u16,
    /// The command consumer, and the response producer: one response a
    /// command, in order, so the two are the same count.
    cmd_cons: u16,
    resp_cons: u16,
    /// The due time of the command at the head of the ring, once taken.
    head_due: Option<u64>,
    handles: Vec<Option<Handle>>,
    /// Every line LOG printed, in order, when a test asks for the record by
    /// setting it to `Some`. Not kept in a checkpoint.
    pub log: Option<Vec<Vec<u8>>>,
    /// **The date every modification time is reported as**, in Unix
    /// seconds, when set: a listing's, OPEN's and CLOSE's, in place of the
    /// host file's. The time-neutral harness sets it to its fixed host date,
    /// as it fixes the RTC and the file server's dates, so that the words a
    /// run writes do not depend on when its files were made. `None`, the
    /// host's, otherwise. The run's say, as the mounts are: not kept in a
    /// checkpoint.
    pub fixed_date: Option<u32>,
}

impl Default for FileDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl FileDevice {
    /// At power-on: disabled, the rings at 0, no handle, nothing mounted.
    pub fn new() -> FileDevice {
        FileDevice {
            mounts: Mounts::default(),
            enabled: false,
            interrupt_enable: false,
            refused: false,
            index_fault: false,
            cmd_base: 0,
            cmd_log2: 0,
            resp_base: 0,
            resp_log2: 0,
            cmd_prod: 0,
            cmd_cons: 0,
            resp_cons: 0,
            head_due: None,
            handles: vec![None; MAX_HANDLES],
            log: None,
            fixed_date: None,
        }
    }

    fn cmd_entries(&self) -> u16 {
        1 << self.cmd_log2
    }

    fn resp_entries(&self) -> u16 {
        1 << self.resp_log2
    }

    /// The response producer, 170.
    pub fn response_producer(&self) -> u16 {
        self.cmd_cons
    }

    /// When the command at the head of the ring completes, if it has been
    /// taken.
    pub fn head_due(&self) -> Option<u64> {
        self.head_due
    }

    /// Handles open.
    pub fn handles_open(&self) -> usize {
        self.handles.iter().filter(|h| h.is_some()).count()
    }

    /// Commands posted and not yet answered.
    pub fn queued(&self) -> u16 {
        self.cmd_prod.wrapping_sub(self.cmd_cons)
    }

    /// A response waiting at `now`: one written and not consumed, or one due
    /// by then.
    pub fn waiting_at(&self, now: u64) -> bool {
        self.cmd_cons != self.resp_cons || self.head_due.is_some_and(|d| d <= now)
    }

    /// Word 100 `<7>` at `now`: a response waiting, under the interrupt
    /// enable. A level.
    pub fn interrupt_at(&self, now: u64) -> bool {
        self.interrupt_enable && self.waiting_at(now)
    }

    /// Why a checkpoint cannot be taken now, if it cannot: a handle's host
    /// file and a command's host effect are outside the machine.
    pub fn checkpoint_refusal(&self) -> Option<String> {
        let (h, q) = (self.handles_open(), self.queued());
        if h == 0 && q == 0 {
            return None;
        }
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        Some(format!(
            "the file device has {} and {}; a checkpoint waits until every handle is closed and every command answered",
            plural(h, "handle open", "handles open"),
            plural(q as usize, "command queued", "commands queued"),
        ))
    }

    /// A register's word at `now`, the device having been advanced to it.
    pub fn read(&self, word: u32, now: u64) -> u32 {
        let on = self.enabled;
        match word {
            CONTROL => on as u32 | (self.interrupt_enable as u32) << 8,
            STATUS => {
                on as u32
                    | (!on as u32) << 1
                    | (self.refused as u32) << 2
                    | (self.index_fault as u32) << 3
                    | (self.waiting_at(now) as u32) << 8
                    | (self.handles_open() as u32) << 16
            }
            CMD_BASE => self.cmd_base,
            CMD_SIZE => self.cmd_log2,
            RESP_BASE => self.resp_base,
            RESP_SIZE => self.resp_log2,
            CMD_PROD if on => self.cmd_prod as u32,
            CMD_CONS | RESP_PROD if on => self.cmd_cons as u32,
            RESP_CONS if on => self.resp_cons as u32,
            _ => 0,
        }
    }

    /// A register written at `now`. A producer write is taken from when the
    /// processor's write buffer is empty, `drained_at`, if that is later;
    /// `main` is read for the command's lengths as it is taken.
    pub fn write<W: MemoryWord>(
        &mut self,
        word: u32,
        v: u32,
        (now, time): (u64, crate::clock::TimeBase),
        drained_at: u64,
        main: &[W],
    ) {
        let on = self.enabled;
        match word {
            CONTROL => {
                self.refused = false;
                self.index_fault = false;
                let (enable, ie) = (v & 1 != 0, v & 0x100 != 0);
                match (on, enable) {
                    (false, true) => {
                        let line = LAYOUT.line;
                        let fits = |base: u32, log2: u32| {
                            base as usize & line == 0
                                && log2 <= 8
                                && base as usize + (8 << log2) <= main.len()
                        };
                        if fits(self.cmd_base, self.cmd_log2)
                            && fits(self.resp_base, self.resp_log2)
                        {
                            self.enabled = true;
                            self.interrupt_enable = ie;
                            (self.cmd_prod, self.cmd_cons, self.resp_cons) = (0, 0, 0);
                        } else {
                            self.refused = true;
                        }
                    }
                    (true, false) => self.disable(),
                    (true, true) => self.interrupt_enable = ie,
                    (false, false) => {}
                }
            }
            CMD_BASE if !on => self.cmd_base = v & LAYOUT.address,
            CMD_SIZE if !on => self.cmd_log2 = v & 0o17,
            RESP_BASE if !on => self.resp_base = v & LAYOUT.address,
            RESP_SIZE if !on => self.resp_log2 = v & 0o17,
            CMD_PROD if on => {
                let new = v as u16;
                let claimed = new.wrapping_sub(self.cmd_cons);
                if claimed > self.cmd_entries() || claimed < self.queued() {
                    self.index_fault = true;
                } else {
                    self.cmd_prod = new;
                    self.take(now.max(drained_at), main, time);
                }
            }
            RESP_CONS if on => {
                let new = v as u16;
                if new.wrapping_sub(self.resp_cons) > self.cmd_cons.wrapping_sub(self.resp_cons) {
                    self.index_fault = true;
                } else {
                    self.resp_cons = new;
                    self.take(now, main, time);
                }
            }
            _ => {}
        }
    }

    /// Disable, which is the reset: the queue dropped, every handle closed
    /// and every write discarded, the indexes and the interrupt enable 0.
    /// Quiet at once: muir's device is never in the middle of a copy.
    pub fn disable(&mut self) {
        self.enabled = false;
        self.interrupt_enable = false;
        (self.cmd_prod, self.cmd_cons, self.resp_cons) = (0, 0, 0);
        self.head_due = None;
        for h in self.handles.iter_mut() {
            if let Some(Handle::Write { temp, .. }) = h.take() {
                let _ = fs::remove_file(&temp.path);
            }
        }
    }

    /// A machine reset, `-XBUS INIT`: the disable, and the two fault bits
    /// cleared. The rings' bases and sizes stay.
    pub fn reset(&mut self) {
        self.disable();
        self.refused = false;
        self.index_fault = false;
    }

    /// Takes the command at the head of the ring, if there is one and the
    /// response ring has room, from `start`: its due time from the lengths
    /// its entry names.
    fn take<W: MemoryWord>(&mut self, start: u64, main: &[W], time: crate::clock::TimeBase) {
        if self.head_due.is_some() || !self.enabled || self.queued() == 0 {
            return;
        }
        if self.cmd_cons.wrapping_sub(self.resp_cons) >= self.resp_entries() {
            return;
        }
        let e = self.cmd_base as usize + 8 * (self.cmd_cons % self.cmd_entries()) as usize;
        self.head_due = Some(due_on(time, start, main[e + 3].low(), main[e + 5].low()));
    }

    /// Runs every command due by `now`, each at its own due time, and takes
    /// the next. Whether main memory was written.
    pub fn advance<W: MemoryWord>(
        &mut self,
        now: u64,
        main: &mut [W],
        time: crate::clock::TimeBase,
    ) -> bool {
        let mut wrote = false;
        while let Some(at) = self.head_due.filter(|&d| d <= now) {
            self.head_due = None;
            self.execute(main);
            wrote = true;
            self.take(at, main, time);
        }
        wrote
    }

    /// The command at the head: its entry and buffers read, the host's
    /// operation done, buffer B and the response written, and the indexes
    /// moved, in one step.
    ///
    /// A command is word 0 `<15:0>` the tag, `<23:16>` the opcode, `<31:24>`
    /// flags; 1 a handle; 2 and 3 buffer A's address and length, 4 and 5
    /// buffer B's; 6 an offset or DIRECTORY's cookie; 7 CLOSE's date. A
    /// response is word 0 the tag, `<23:16>` the status and `<31:24>` the
    /// opcode; 1 a count; 2 a handle; 3 a length; 4 an mtime; 5 flags; 6
    /// DIRECTORY's next cookie or COMPLETE's matches; 7 0. A failed
    /// command's response is word 0 alone.
    fn execute<W: MemoryWord>(&mut self, main: &mut [W]) {
        let e = self.cmd_base as usize + 8 * (self.cmd_cons % self.cmd_entries()) as usize;
        let c: [u32; 8] = std::array::from_fn(|k| main[e + k].low());
        let (tag, opcode, flags) = (c[0] & 0xffff, (c[0] >> 16) & 0xff, c[0] >> 24);
        let mut reply = Reply::default();
        let st = match self.command(opcode, flags, &c, main, &mut reply) {
            Ok(()) => status::OK,
            Err(s) => {
                reply = Reply::default();
                s
            }
        };
        let r = self.resp_base as usize + 8 * (self.cmd_cons % self.resp_entries()) as usize;
        let fix = LAYOUT.tag;
        main[r] = W::tagged(tag | st << 16 | opcode << 24, fix);
        for (m, &v) in main[r + 1..r + 8].iter_mut().zip(&reply.0) {
            *m = W::tagged(v, fix);
        }
        self.cmd_cons = self.cmd_cons.wrapping_add(1);
    }

    fn command<W: MemoryWord>(
        &mut self,
        opcode: u32,
        flags: u32,
        c: &[u32; 8],
        main: &mut [W],
        r: &mut Reply,
    ) -> Result<(), u32> {
        let allowed = match opcode {
            op::OPEN => 0b1_1111,
            op::CLOSE => 0b11,
            op::READ..=op::LOG => 0,
            _ => return Err(status::UOP),
        };
        if flags & !allowed != 0 {
            return Err(status::BAD_ARGUMENT);
        }
        let layout = LAYOUT;
        let a = || buffer(main, c[2], c[3], layout);
        let b = || buffer(main, c[4], c[5], layout);
        match opcode {
            op::OPEN => {
                let a = a()?;
                self.open(flags, &bytes_of(main, a), r)
            }
            op::READ => {
                let file = match self.handle(c[1])? {
                    Handle::Read(f) => f.clone(),
                    Handle::Write { .. } => return Err(status::BAD_HANDLE),
                };
                let b = b()?;
                let len = file.metadata().map_err(|e| host_status(&e))?.len();
                let offset = c[6] as u64;
                if offset > len {
                    return Err(status::FOR);
                }
                let n = (b.len as u64).min(len - offset) as usize;
                let mut data = vec![0u8; n];
                file.read_exact_at(&mut data, offset).map_err(|e| host_status(&e))?;
                put_bytes(main, b.at, &data, layout.tag);
                r.count(n as u32);
                Ok(())
            }
            op::WRITE => {
                let a = a()?;
                let Handle::Write { temp, held, .. } = self.handle_mut(c[1])? else {
                    return Err(status::BAD_HANDLE);
                };
                let offset = c[6] as u64;
                if offset > *held || offset + a.len as u64 > u32::MAX as u64 {
                    return Err(status::FOR);
                }
                temp.file.write_all_at(&bytes_of(main, a), offset).map_err(|e| host_status(&e))?;
                *held = (*held).max(offset + a.len as u64);
                r.count(a.len);
                Ok(())
            }
            op::CLOSE => self.close(c[1], flags, c[7], r),
            op::DIRECTORY => {
                let (a, b) = (a()?, b()?);
                if b.len < MIN_DIRECTORY_BUFFER {
                    return Err(status::BAD_ARGUMENT);
                }
                let comps = parse_name(&bytes_of(main, a))?;
                let entries = self.list(&comps)?;
                let mut words = Vec::new();
                let mut next = c[6] as usize;
                while let Some(e) = entries.get(next) {
                    let rec = record(e);
                    if (words.len() + rec.len()) * 4 > b.len as usize {
                        break;
                    }
                    words.extend(rec);
                    next += 1;
                }
                for (m, &v) in main[b.at..b.at + words.len()].iter_mut().zip(&words) {
                    *m = W::tagged(v, layout.tag);
                }
                r.count(4 * words.len() as u32);
                r.w6(if next < entries.len() { next as u32 } else { 0 });
                Ok(())
            }
            op::COMPLETE => {
                let (a, b) = (a()?, b()?);
                let text = bytes_of(main, a);
                let slash = text.iter().rposition(|&x| x == b'/').ok_or(status::IPS)?;
                let (dir, prefix) = (&text[..=slash], &text[slash + 1..]);
                // The prefix is bytes of a component, and may be empty.
                if prefix.len() > MAX_COMPONENT
                    || !prefix.iter().all(|&x| (0o40..=0o176).contains(&x) && x != b'/')
                {
                    return Err(status::IPS);
                }
                let comps = parse_name(dir)?;
                let entries = self.list(&comps)?;
                let matches: Vec<&Entry> =
                    entries.iter().filter(|e| e.name.as_bytes().starts_with(prefix)).collect();
                let Some(first) = matches.first() else { return Ok(()) };
                let mut lcp = first.name.as_bytes();
                for m in &matches[1..] {
                    let n = lcp.iter().zip(m.name.as_bytes()).take_while(|(x, y)| x == y).count();
                    lcp = &lcp[..n];
                }
                if lcp.len() > b.len as usize {
                    return Err(status::BAD_ARGUMENT);
                }
                put_bytes(main, b.at, lcp, layout.tag);
                r.count(lcp.len() as u32);
                r.w6(matches.len() as u32);
                if let Some(exact) = matches.iter().find(|e| e.name.as_bytes() == lcp) {
                    r.flags(0b100 | (exact.dir as u32) << 3);
                }
                Ok(())
            }
            op::DELETE => {
                let a = a()?;
                self.delete(&bytes_of(main, a))
            }
            op::RENAME => {
                let (a, b) = (a()?, b()?);
                self.rename(&bytes_of(main, a), &bytes_of(main, b))
            }
            op::CREATE_DIRECTORY => {
                let a = a()?;
                self.create_directory(&bytes_of(main, a))
            }
            op::LOG => {
                let a = a()?;
                if a.len > MAX_LOG {
                    return Err(status::BAD_ARGUMENT);
                }
                let line = bytes_of(main, a);
                eprintln!("{}", log_line(&line));
                if let Some(log) = self.log.as_mut() {
                    log.push(line);
                }
                Ok(())
            }
            _ => Err(status::UOP),
        }
    }

    fn handle(&self, h: u32) -> Result<&Handle, u32> {
        (h as usize)
            .checked_sub(1)
            .and_then(|k| self.handles.get(k))
            .and_then(|h| h.as_ref())
            .ok_or(status::BAD_HANDLE)
    }

    fn handle_mut(&mut self, h: u32) -> Result<&mut Handle, u32> {
        (h as usize)
            .checked_sub(1)
            .and_then(|k| self.handles.get_mut(k))
            .and_then(|h| h.as_mut())
            .ok_or(status::BAD_HANDLE)
    }

    fn free_handle(&self) -> Result<usize, u32> {
        self.handles.iter().position(|h| h.is_none()).ok_or(status::NER)
    }

    /// Where a name's components put it: under a named mount, the default
    /// folder, or, with no default folder, the bare read-only `/`.
    fn place(&self, comps: &[String]) -> Place {
        if let Some(first) = comps.first()
            && let Some(m) = self.mounts.named.get(first)
        {
            let (key, root) = (Key::Named(first.clone()), Some(m.path.clone()));
            return Place { key, root, ro: m.ro, rel: comps[1..].to_vec() };
        }
        match &self.mounts.default {
            Some(m) => Place {
                key: Key::Default,
                root: Some(m.path.clone()),
                ro: m.ro,
                rel: comps.to_vec(),
            },
            None => Place { key: Key::Bare, root: None, ro: true, rel: comps.to_vec() },
        }
    }

    /// A placed name looked up: every directory on the way resolved inside
    /// the mount's folder (DNF if one is missing or a file, ACC if a
    /// symlink leaves or loops), and the last one followed if `follow`.
    /// A name that is spelled otherwise than asked is not there.
    fn lookup(&self, p: &Place, follow: bool) -> Result<Found, u32> {
        let Some(root) = p.root.as_deref() else {
            // Nothing is under the bare `/` but the mounts.
            return match p.rel.len() {
                0 => Err(status::IOD),
                1 => Ok(Found { parent: PathBuf::new(), path: PathBuf::new(), meta: None }),
                _ => Err(status::DNF),
            };
        };
        let mut cur = root.to_path_buf();
        if p.rel.is_empty() {
            let meta = fs::metadata(root).map_err(|e| host_status(&e))?;
            return Ok(Found { parent: cur.clone(), path: cur, meta: Some(meta) });
        }
        for (k, c) in p.rel.iter().enumerate() {
            let last = k + 1 == p.rel.len();
            let here = cur.join(c);
            let lm = match fs::symlink_metadata(&here) {
                Ok(m) if exact_case(&cur, c, &m) => Some(m),
                Ok(_) => None,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) =>
                {
                    None
                }
                Err(e) => return Err(host_status(&e)),
            };
            let Some(lm) = lm else {
                return if last {
                    Ok(Found { parent: cur, path: here, meta: None })
                } else {
                    Err(status::DNF)
                };
            };
            let (path, meta) = if lm.file_type().is_symlink() && (follow || !last) {
                match fs::canonicalize(&here) {
                    Ok(real) if real.starts_with(root) => {
                        let m = fs::metadata(&real).map_err(|e| host_status(&e))?;
                        (real, m)
                    }
                    Ok(_) => return Err(status::ACC),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        return if last {
                            Ok(Found { parent: cur, path: here, meta: None })
                        } else {
                            Err(status::DNF)
                        };
                    }
                    Err(e) => return Err(host_status(&e)),
                }
            } else {
                // A symlink kept as itself must still stay inside.
                if lm.file_type().is_symlink()
                    && !fs::canonicalize(&here).is_ok_and(|r| r.starts_with(root))
                {
                    return Err(status::ACC);
                }
                (here, lm)
            };
            if last {
                return Ok(Found { parent: cur, path, meta: Some(meta) });
            }
            if !meta.is_dir() {
                return Err(status::DNF);
            }
            cur = path;
        }
        unreachable!("the last component returns")
    }

    /// OPEN: read, write or probe.
    fn open(&mut self, flags: u32, name: &[u8], r: &mut Reply) -> Result<(), u32> {
        let (mode, if_exists, if_none_error) = (flags & 3, (flags >> 2) & 3, flags >> 4 & 1 != 0);
        if mode == 3 || if_exists == 3 {
            return Err(status::BAD_ARGUMENT);
        }
        let comps = parse_name(name)?;
        let p = self.place(&comps);
        let ro_bit = (p.ro as u32) << 1;
        if mode == 1 {
            return self.open_write(&p, if_exists, if_none_error, r);
        }
        // `/` with no default folder: a directory of the mounts.
        if p.key == Key::Bare && p.rel.is_empty() {
            if mode == 0 {
                return Err(status::IOD);
            }
            r.flags(1 | ro_bit);
            return Ok(());
        }
        let f = self.lookup(&p, true)?;
        let Some(meta) = f.meta else { return Err(status::FNF) };
        if !(meta.is_file() || meta.is_dir()) || (meta.is_file() && meta.len() > u32::MAX as u64) {
            return Err(status::WKF);
        }
        let info = entry_of(String::new(), &meta, p.ro, self.fixed_date);
        if mode == 0 {
            if info.dir {
                return Err(status::IOD);
            }
            let k = self.free_handle()?;
            let file = File::open(&f.path).map_err(|e| host_status(&e))?;
            self.handles[k] = Some(Handle::Read(Arc::new(file)));
            r.handle(k as u32 + 1);
        }
        r.len(info.len);
        r.mtime(info.mtime);
        r.flags(info.dir as u32 | ro_bit);
        Ok(())
    }

    fn open_write(
        &mut self,
        p: &Place,
        if_exists: u32,
        if_none_error: bool,
        r: &mut Reply,
    ) -> Result<(), u32> {
        if p.key == Key::Bare && p.rel.len() > 1 {
            return Err(status::DNF);
        }
        if p.ro {
            return Err(status::ATF);
        }
        if p.is_mount_root() {
            return Err(status::IOD);
        }
        let f = self.lookup(p, true)?;
        let mut held = 0;
        match &f.meta {
            Some(m) if m.is_dir() => return Err(status::IOD),
            Some(m) if !m.is_file() => return Err(status::WKF),
            Some(_) if if_exists == 1 => return Err(status::FAE),
            Some(m) if if_exists == 2 => {
                if m.len() > u32::MAX as u64 {
                    return Err(status::WKF);
                }
                held = m.len();
            }
            Some(_) => {}
            None if if_none_error => return Err(status::FNF),
            None => {}
        }
        let k = self.free_handle()?;
        let dir = f.path.parent().map(Path::to_path_buf).unwrap_or(f.parent);
        let path = dir.join(format!(
            "{TEMP_PREFIX}{}-{}",
            std::process::id(),
            TEMPS.fetch_add(1, Ordering::Relaxed)
        ));
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| host_status(&e))?;
        let temp = Temp { path, file };
        if let Some(m) = &f.meta {
            // The file keeps its permissions across the write.
            let _ = fs::set_permissions(&temp.path, m.permissions());
            if if_exists == 2 {
                let mut from = File::open(&f.path).map_err(|e| host_status(&e))?;
                let mut to = &temp.file;
                held = io::copy(&mut from, &mut to).map_err(|e| host_status(&e))?;
            }
        }
        self.handles[k] = Some(Handle::Write {
            temp: Arc::new(temp),
            target: f.path,
            noreplace: if_exists == 1,
            held,
        });
        r.handle(k as u32 + 1);
        r.len(held);
        Ok(())
    }

    /// CLOSE: a read handle closed; a write's temporary file renamed onto
    /// its name, its date set first if asked, or removed on abort.
    fn close(&mut self, h: u32, flags: u32, date: u32, r: &mut Reply) -> Result<(), u32> {
        self.handle(h)?;
        let handle = self.handles[h as usize - 1].take().expect("checked");
        let abort = flags & 1 != 0;
        match handle {
            Handle::Read(file) => {
                if !abort {
                    let m = file.metadata().map_err(|e| host_status(&e))?;
                    r.len(m.len());
                    r.mtime(self.fixed_date.unwrap_or_else(|| mtime_of(&m)));
                }
            }
            Handle::Write { temp, target, noreplace, .. } => {
                if abort {
                    let _ = fs::remove_file(&temp.path);
                    return Ok(());
                }
                let landed = (|| {
                    if flags & 2 != 0 {
                        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(date as u64);
                        temp.file.set_modified(t)?;
                    }
                    if noreplace {
                        rename_noreplace(&temp.path, &target)
                    } else {
                        fs::rename(&temp.path, &target)
                    }
                })();
                if let Err(e) = landed {
                    let _ = fs::remove_file(&temp.path);
                    return Err(host_status(&e));
                }
                let m = fs::metadata(&target).map_err(|e| host_status(&e))?;
                r.len(m.len());
                r.mtime(self.fixed_date.unwrap_or_else(|| mtime_of(&m)));
            }
        }
        Ok(())
    }

    /// The entries of the directory `comps` names: the mounts and the
    /// default folder's entries at `/`.
    fn list(&self, comps: &[String]) -> Result<Vec<Entry>, u32> {
        if comps.is_empty() {
            let mut out = match &self.mounts.default {
                Some(m) => list_folder(&m.path, &m.path, m.ro, self.fixed_date)
                    .map_err(|e| host_status(&e))?,
                None => Vec::new(),
            };
            out.retain(|e| !self.mounts.named.contains_key(&e.name));
            for (n, m) in &self.mounts.named {
                let meta = fs::metadata(&m.path).map_err(|e| host_status(&e))?;
                out.push(entry_of(n.clone(), &meta, m.ro, self.fixed_date));
            }
            out.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
            return Ok(out);
        }
        let p = self.place(comps);
        let f = self.lookup(&p, true)?;
        match f.meta {
            None => Err(status::DNF),
            Some(m) if m.is_dir() => {
                list_folder(&f.path, p.root.as_deref().expect("a folder"), p.ro, self.fixed_date)
                    .map_err(|e| host_status(&e))
            }
            Some(_) => Err(status::WKF),
        }
    }

    /// DELETE: a file, or an empty directory.
    fn delete(&mut self, name: &[u8]) -> Result<(), u32> {
        let comps = parse_name(name)?;
        let p = self.place(&comps);
        if p.key == Key::Bare && p.rel.len() > 1 {
            return Err(status::DNF);
        }
        if p.ro {
            return Err(status::ATF);
        }
        if p.is_mount_root() {
            return Err(status::ACC);
        }
        let f = self.lookup(&p, false)?;
        let meta = f.meta.ok_or(status::FNF)?;
        if meta.is_dir() { fs::remove_dir(&f.path) } else { fs::remove_file(&f.path) }
            .map_err(|e| host_status(&e))
    }

    /// RENAME: never over an existing name, never across mounts.
    fn rename(&mut self, from: &[u8], to: &[u8]) -> Result<(), u32> {
        let (fc, tc) = (parse_name(from)?, parse_name(to)?);
        let (fp, tp) = (self.place(&fc), self.place(&tc));
        for p in [&fp, &tp] {
            if p.key == Key::Bare && p.rel.len() > 1 {
                return Err(status::DNF);
            }
        }
        if fp.ro || tp.ro {
            return Err(status::ATF);
        }
        if fp.is_mount_root() || tp.is_mount_root() {
            return Err(status::ACC);
        }
        if fp.key != tp.key {
            return Err(status::RAD);
        }
        let f = self.lookup(&fp, false)?;
        if f.meta.is_none() {
            return Err(status::FNF);
        }
        let t = self.lookup(&tp, false)?;
        if t.meta.is_some() {
            return Err(status::REF);
        }
        rename_noreplace(&f.path, &t.path).map_err(|e| match e.kind() {
            io::ErrorKind::AlreadyExists | io::ErrorKind::DirectoryNotEmpty => status::REF,
            _ => host_status(&e),
        })
    }

    /// CREATE-DIRECTORY: one level.
    fn create_directory(&mut self, name: &[u8]) -> Result<(), u32> {
        let comps = parse_name(name)?;
        let p = self.place(&comps);
        if p.key == Key::Bare && p.rel.len() > 1 {
            return Err(status::DNF);
        }
        if p.ro {
            return Err(status::ATF);
        }
        let f = self.lookup(&p, true)?;
        match f.meta {
            Some(m) if m.is_dir() => Err(status::DAE),
            Some(_) => Err(status::FAE),
            None => fs::create_dir(&f.path).map_err(|e| host_status(&e)),
        }
    }

    pub fn save(&self, w: &mut crate::checkpoint::Writer) {
        for b in [self.enabled, self.interrupt_enable, self.refused, self.index_fault] {
            w.bool(b);
        }
        for v in [self.cmd_base, self.cmd_log2, self.resp_base, self.resp_log2] {
            w.u32(v);
        }
        for v in [self.cmd_prod, self.cmd_cons, self.resp_cons] {
            w.u16(v);
        }
        w.opt(self.head_due, |w, d| w.u64(d));
    }

    /// Back from a checkpoint: the registers and the rings' state, with no
    /// handle open, which is the only way one is taken. The mounts stay the
    /// flags'.
    pub fn load(&mut self, r: &mut crate::checkpoint::Reader) -> io::Result<()> {
        self.disable();
        self.enabled = r.bool()?;
        self.interrupt_enable = r.bool()?;
        self.refused = r.bool()?;
        self.index_fault = r.bool()?;
        self.cmd_base = r.u32()?;
        self.cmd_log2 = r.u32()?;
        self.resp_base = r.u32()?;
        self.resp_log2 = r.u32()?;
        self.cmd_prod = r.u16()?;
        self.cmd_cons = r.u16()?;
        self.resp_cons = r.u16()?;
        self.head_due = r.opt(|r| r.u64())?;
        if self.cmd_log2 > 8 || self.resp_log2 > 8 {
            return Err(crate::checkpoint::bad("a file device ring of more than 256 entries"));
        }
        Ok(())
    }
}

/// A DIRECTORY record: word 0 the name's length `<7:0>`, the record's words
/// `<15:8>`, `<16>` a directory, `<17>` read-only, `<18>` 2^32 bytes or more;
/// 1 the length (0 for a directory, FFFFFFFF when too large); 2 the mtime;
/// then the name's bytes, zero-padded.
fn record(e: &Entry) -> Vec<u32> {
    let n = e.name.len();
    let words = 3 + n.div_ceil(4);
    let too_large = e.len > u32::MAX as u64;
    let mut out = vec![
        n as u32
            | (words as u32) << 8
            | (e.dir as u32) << 16
            | (e.ro as u32) << 17
            | (too_large as u32) << 18,
        e.len.min(u32::MAX as u64) as u32,
        e.mtime,
    ];
    out.resize(words, 0);
    put_bytes(&mut out[3..], 0, e.name.as_bytes(), 0);
    out
}
