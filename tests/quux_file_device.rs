// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's file device (contract Q9, revision 9): host folders served to the
//! machine through two rings in main memory, a command ring the processor
//! fills and a response ring the device fills, with the registers at words
//! 160-171 of the register page and its interrupt at word 100 `<7>`
//! ([`muir::file_device`]).
//!
//! A scripted driver ([`Dev`]) writes commands into the rings in main
//! memory, writes the producer index, moves the machine's clock on, and
//! reads the responses back, against a scratch folder on the host. The
//! engines' own tests at the end run a program that does the same through
//! the bus, on `micro` and on `rtl`.

mod support;

use std::path::{Path, PathBuf};

use muir::file_device::{self, Mounts, op, status};
use muir::machine::{Geometry, Machine};

const PAGE: u32 = muir::machine::REGISTER_PAGE_13;
const CONTROL: u32 = PAGE + 0o160;
const STATUS: u32 = PAGE + 0o161;
const CMD_BASE: u32 = PAGE + 0o162;
const CMD_SIZE: u32 = PAGE + 0o163;
const CMD_PROD: u32 = PAGE + 0o164;
const CMD_CONS: u32 = PAGE + 0o165;
const RESP_BASE: u32 = PAGE + 0o166;
const RESP_SIZE: u32 = PAGE + 0o167;
const RESP_PROD: u32 = PAGE + 0o170;
const RESP_CONS: u32 = PAGE + 0o171;
const INTERRUPTS: u32 = PAGE + 0o100;

/// Where the driver keeps its rings and buffers, physical word addresses,
/// each on a line.
const CMD_RING: u32 = 0o100000;
const RESP_RING: u32 = 0o110000;
const BUF_A: u32 = 0o120000;
const BUF_B: u32 = 0o160000;

/// The time a command takes, as the device charges it: 20 us and 100 us a
/// KiB of the lengths its entry names, rounded up to the ns.
fn cost(a_len: u32, b_len: u32) -> u64 {
    20_000 + ((a_len as u64 + b_len as u64) * 100_000).div_ceil(1024)
}

// --- the scratch folder ---------------------------------------------------

/// A folder of the test's own under the system's temporary directory,
/// removed when dropped.
struct Folder(PathBuf);

impl Folder {
    fn new(name: &str) -> Folder {
        let p = std::env::temp_dir().join(format!("muir-fdev-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Folder(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
    fn file(&self, rel: &str, bytes: &[u8]) -> PathBuf {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        p
    }
    fn dir(&self, rel: &str) -> PathBuf {
        let p = self.0.join(rel);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Every name under `root` with its bytes and modification time: what a
/// test compares to say a folder was not changed.
fn manifest(root: &Path) -> Vec<(PathBuf, Option<Vec<u8>>, i64)> {
    use std::os::unix::fs::MetadataExt;
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let m = std::fs::symlink_metadata(&p).unwrap();
            if m.is_dir() {
                stack.push(p.clone());
                out.push((p, None, m.mtime()));
            } else if m.is_file() {
                out.push((p.clone(), Some(std::fs::read(&p).unwrap()), m.mtime()));
            } else {
                out.push((p, None, 0));
            }
        }
    }
    out.sort();
    out
}

fn set_mtime(p: &Path, secs: u64) {
    let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)).unwrap();
}

fn mtime(p: &Path) -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p).unwrap().mtime() as u32
}

fn mounts(specs: &[String]) -> Mounts {
    let mut m = Mounts::default();
    for s in specs {
        m.add(s).unwrap_or_else(|e| panic!("{s}: {e}"));
    }
    m
}

fn default_root(f: &Folder) -> Mounts {
    mounts(&[f.path().display().to_string()])
}

// --- the scripted driver --------------------------------------------------

/// A command entry, the fields the layout gives it.
#[derive(Clone, Copy, Default, Debug)]
struct Cmd {
    op: u32,
    flags: u32,
    handle: u32,
    a: (u32, u32),
    b: (u32, u32),
    w6: u32,
    w7: u32,
}

/// A response entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Resp([u32; 8]);

impl Resp {
    fn tag(self) -> u32 {
        self.0[0] & 0xffff
    }
    fn status(self) -> u32 {
        (self.0[0] >> 16) & 0xff
    }
    fn op(self) -> u32 {
        self.0[0] >> 24
    }
    fn count(self) -> u32 {
        self.0[1]
    }
    fn handle(self) -> u32 {
        self.0[2]
    }
    fn len(self) -> u32 {
        self.0[3]
    }
    fn mtime(self) -> u32 {
        self.0[4]
    }
    fn flags(self) -> u32 {
        self.0[5]
    }
    fn w6(self) -> u32 {
        self.0[6]
    }
    /// A failed command's response has only word 0.
    fn failed_alone(self) -> bool {
        self.0[1..].iter().all(|&w| w == 0)
    }
}

/// A machine with its file device enabled on rings of `2^cmd_log2` and
/// `2^resp_log2` entries, and the indexes the driver keeps.
struct Dev {
    m: Machine,
    prod: u16,
    cons: u16,
    cmd_log2: u32,
    resp_log2: u32,
    tag: u16,
}

fn put_bytes(m: &mut Machine, at: u32, bytes: &[u8]) {
    for (k, chunk) in bytes.chunks(4).enumerate() {
        let mut w = [0u8; 4];
        w[..chunk.len()].copy_from_slice(chunk);
        m.main[at as usize + k] = u64::from(u32::from_le_bytes(w));
    }
}

/// `<31:0>` of a word the device wrote, every one a fixnum, tag `005`
/// (contract G2 §4.3).
fn fixnum(w: muir::machine::Word) -> u32 {
    assert_eq!(w >> 32, 0o005, "{w:o}: a word the device writes is a fixnum");
    w as u32
}

fn get_bytes(m: &Machine, at: u32, n: usize) -> Vec<u8> {
    (0..n).map(|k| (m.main[at as usize + k / 4] >> (8 * (k % 4))) as u8).collect()
}

fn quux(mounts: Mounts) -> Machine {
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
    m.file_device.mounts = mounts;
    m
}

impl Dev {
    fn new(mounts: Mounts) -> Dev {
        Dev::with_rings(mounts, 2, 2)
    }

    fn with_rings(mounts: Mounts, cmd_log2: u32, resp_log2: u32) -> Dev {
        let mut m = quux(mounts);
        m.bus_write(CMD_BASE, CMD_RING.into());
        m.bus_write(CMD_SIZE, cmd_log2.into());
        m.bus_write(RESP_BASE, RESP_RING.into());
        m.bus_write(RESP_SIZE, resp_log2.into());
        m.bus_write(CONTROL, 1);
        assert_eq!(m.bus_read(STATUS) & 0b111, 1, "enabled, not quiet, not refused");
        Dev { m, prod: 0, cons: 0, cmd_log2, resp_log2, tag: 0o100 }
    }

    /// Writes `c` into the next command slot and the producer index after
    /// it; the tag it carries.
    fn post(&mut self, c: Cmd) -> u32 {
        self.tag = self.tag.wrapping_add(1);
        let slot = CMD_RING + 8 * (self.prod as u32 % (1 << self.cmd_log2));
        let words = [
            self.tag as u32 | (c.op & 0xff) << 16 | c.flags << 24,
            c.handle,
            c.a.0,
            c.a.1,
            c.b.0,
            c.b.1,
            c.w6,
            c.w7,
        ];
        for (m, w) in self.m.main[slot as usize..slot as usize + 8].iter_mut().zip(words) {
            *m = w.into();
        }
        self.prod = self.prod.wrapping_add(1);
        self.m.bus_write(CMD_PROD, (self.prod as u32).into());
        self.tag as u32
    }

    /// Moves the clock on until the response index passes the driver's,
    /// takes the response and hands its slot back.
    fn wait(&mut self) -> Resp {
        let start = self.m.ns;
        while self.m.bus_read(RESP_PROD) as u16 == self.cons {
            self.m.ns += 1_000;
            assert!(self.m.ns - start < 10_000_000_000, "no response in ten seconds");
        }
        let r = self.response(self.cons);
        self.cons = self.cons.wrapping_add(1);
        self.m.bus_write(RESP_CONS, (self.cons as u32).into());
        r
    }

    fn response(&self, index: u16) -> Resp {
        let slot = RESP_RING + 8 * (index as u32 % (1 << self.resp_log2));
        Resp(std::array::from_fn(|k| fixnum(self.m.main[slot as usize + k])))
    }

    fn run(&mut self, c: Cmd) -> Resp {
        let tag = self.post(c);
        let r = self.wait();
        assert_eq!(r.tag(), tag, "response {r:?} answers the command");
        assert_eq!(r.op(), c.op & 0xff, "and echoes its opcode");
        if r.status() != 0 {
            assert!(r.failed_alone(), "a failed response is word 0 alone: {r:?}");
        }
        r
    }

    /// Buffer A holding `bytes`.
    fn a(&mut self, bytes: &[u8]) -> (u32, u32) {
        put_bytes(&mut self.m, BUF_A, bytes);
        (BUF_A, bytes.len() as u32)
    }

    fn open(&mut self, name: &str, flags: u32) -> Resp {
        let a = self.a(name.as_bytes());
        self.run(Cmd { op: op::OPEN, flags, a, ..Default::default() })
    }
    fn probe(&mut self, name: &str) -> Resp {
        self.open(name, PROBE)
    }
    fn read(&mut self, handle: u32, offset: u32, wanted: u32) -> (Resp, Vec<u8>) {
        let r = self.run(Cmd {
            op: op::READ,
            handle,
            b: (BUF_B, wanted),
            w6: offset,
            ..Default::default()
        });
        let n = if r.status() == 0 { r.count() as usize } else { 0 };
        (r, get_bytes(&self.m, BUF_B, n))
    }
    fn write(&mut self, handle: u32, offset: u32, data: &[u8]) -> Resp {
        let a = self.a(data);
        self.run(Cmd { op: op::WRITE, handle, a, w6: offset, ..Default::default() })
    }
    fn close(&mut self, handle: u32, flags: u32, date: u32) -> Resp {
        self.run(Cmd { op: op::CLOSE, flags, handle, w7: date, ..Default::default() })
    }
    fn directory(&mut self, name: &str, cookie: u32, room: u32) -> (Resp, Vec<Rec>) {
        let a = self.a(name.as_bytes());
        let r = self.run(Cmd {
            op: op::DIRECTORY,
            a,
            b: (BUF_B, room),
            w6: cookie,
            ..Default::default()
        });
        let n = if r.status() == 0 { r.count() as usize / 4 } else { 0 };
        let words: Vec<u32> =
            self.m.main[BUF_B as usize..BUF_B as usize + n].iter().map(|&w| fixnum(w)).collect();
        (r, records(&words))
    }
    fn complete(&mut self, text: &str) -> (Resp, String) {
        let a = self.a(text.as_bytes());
        let r = self.run(Cmd { op: op::COMPLETE, a, b: (BUF_B, 256), ..Default::default() });
        let n = if r.status() == 0 { r.count() as usize } else { 0 };
        (r, String::from_utf8(get_bytes(&self.m, BUF_B, n)).unwrap())
    }
    fn simple(&mut self, opcode: u32, name: &str) -> Resp {
        let a = self.a(name.as_bytes());
        self.run(Cmd { op: opcode, a, ..Default::default() })
    }
    fn delete(&mut self, name: &str) -> Resp {
        self.simple(op::DELETE, name)
    }
    fn mkdir(&mut self, name: &str) -> Resp {
        self.simple(op::CREATE_DIRECTORY, name)
    }
    fn rename(&mut self, from: &str, to: &str) -> Resp {
        let a = self.a(from.as_bytes());
        put_bytes(&mut self.m, BUF_B, to.as_bytes());
        self.run(Cmd { op: op::RENAME, a, b: (BUF_B, to.len() as u32), ..Default::default() })
    }
    /// Opens `name` for writing, writes `data`, closes it: the close's
    /// response.
    fn put(&mut self, name: &str, data: &[u8]) -> Resp {
        let o = self.open(name, WRITE);
        assert_eq!(o.status(), 0, "open {name} for writing: {o:?}");
        assert_eq!(self.write(o.handle(), 0, data).status(), 0);
        self.close(o.handle(), 0, 0)
    }
    fn slurp(&mut self, name: &str) -> Vec<u8> {
        let o = self.open(name, READ);
        assert_eq!(o.status(), 0, "open {name}: {o:?}");
        let (r, bytes) = self.read(o.handle(), 0, 65536);
        assert_eq!(r.status(), 0);
        assert_eq!(self.close(o.handle(), 0, 0).status(), 0);
        bytes
    }
}

/// OPEN's flags, `<31:24>` of word 0 as the entry carries them: `<1:0>` the
/// mode, `<3:2>` if-exists, `<4>` if-does-not-exist.
const READ: u32 = 0;
const WRITE: u32 = 1;
const PROBE: u32 = 2;
const IF_EXISTS_ERROR: u32 = 1 << 2;
const IF_EXISTS_APPEND: u32 = 2 << 2;
const IF_DOES_NOT_EXIST_ERROR: u32 = 1 << 4;
/// CLOSE's: `<0>` abort, `<1>` set the date.
const ABORT: u32 = 1;
const SET_DATE: u32 = 2;

/// A directory record, as the device packs it into buffer B.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Rec {
    name: String,
    dir: bool,
    ro: bool,
    too_large: bool,
    len: u32,
    mtime: u32,
}

fn records(words: &[u32]) -> Vec<Rec> {
    let mut out = Vec::new();
    let mut k = 0;
    while k < words.len() {
        let w0 = words[k];
        let n = (w0 & 0xff) as usize;
        let len_words = ((w0 >> 8) & 0xff) as usize;
        assert_eq!(len_words, 3 + n.div_ceil(4), "the record's length in words");
        let bytes: Vec<u8> =
            (0..n).map(|i| (words[k + 3 + i / 4] >> (8 * (i % 4))) as u8).collect();
        out.push(Rec {
            name: String::from_utf8(bytes).unwrap(),
            dir: w0 >> 16 & 1 != 0,
            ro: w0 >> 17 & 1 != 0,
            too_large: w0 >> 18 & 1 != 0,
            len: words[k + 1],
            mtime: words[k + 2],
        });
        k += len_words;
    }
    out
}

fn names(recs: &[Rec]) -> Vec<&str> {
    recs.iter().map(|r| r.name.as_str()).collect()
}

// --- registers --------------------------------------------------------------

/// **Each register's access**: control and the configuration words read
/// back as written while disabled; the status says quiet while disabled;
/// the four indexes read 0 while disabled and their writes go nowhere; the
/// words around the device read 0. On the CADR nothing answers there.
#[test]
fn the_registers_read_and_write_as_the_layout_says() {
    let mut m = quux(Mounts::default());
    assert_eq!(m.bus_read(CONTROL), 0, "disabled at power-on");
    assert_eq!(m.bus_read(STATUS), 0b10, "quiet, nothing else");
    for (reg, v, back) in [
        (CMD_BASE, 0o1234560, 0o1234560),
        (CMD_BASE, 0xffff_fff0, 0xfff_fff0),
        (CMD_SIZE, 0o17, 0o17),
        (CMD_SIZE, 0o37, 0o17),
        (RESP_BASE, 0o7654320, 0o7654320),
        (RESP_SIZE, 3, 3),
    ] {
        m.bus_write(reg, v);
        assert_eq!(m.bus_read(reg), back, "{reg:o} written {v:o}");
    }
    for reg in [CMD_PROD, CMD_CONS, RESP_PROD, RESP_CONS] {
        m.bus_write(reg, 7);
        assert_eq!(m.bus_read(reg), 0, "{reg:o} reads 0 while disabled, and takes no write");
    }
    for w in [0o150, 0o157, 0o172, 0o177] {
        m.bus_write(PAGE + w, !0);
        assert_eq!(m.bus_read(PAGE + w), 0, "word {w:o}");
    }
    assert_eq!(m.bus_error, 0, "nothing timed out");
    // The CADR's own last page, `17777400`, is in its Unibus window.
    let mut cadr = Machine::new();
    assert_eq!(cadr.bus_read(0o17777400 + 0o160), 0);
    assert_eq!(cadr.bus_error, muir::machine::bus_error::UNIBUS_NXM, "the CADR has none");
}

/// **An enable checks the rings**: a base off a line, a size over 8 and a
/// ring past main memory are each refused, status `<2>`, the device staying
/// disabled; the next write of 160 clears the bit. While enabled, a write of
/// the configuration goes nowhere.
#[test]
fn an_enable_refuses_a_bad_ring_and_the_configuration_holds_while_enabled() {
    let words = Machine::new().main.len() as u32;
    for (what, base, size) in [
        ("a base off a line", CMD_RING + 2, 2),
        ("a size of 9", CMD_RING, 9),
        ("a ring past main memory", words - 8 * 3, 2),
    ] {
        let mut m = quux(Mounts::default());
        m.bus_write(CMD_BASE, base.into());
        m.bus_write(CMD_SIZE, size);
        m.bus_write(RESP_BASE, RESP_RING.into());
        m.bus_write(RESP_SIZE, 0);
        m.bus_write(CONTROL, 0x101);
        let s = m.bus_read(STATUS);
        assert_eq!(s & 0b111, 0b110, "{what}: refused and quiet, {s:b}");
        assert_eq!(m.bus_read(CONTROL), 0, "{what}: disabled, no interrupt enable");
        m.bus_write(CONTROL, 0);
        assert_eq!(m.bus_read(STATUS) & 0b100, 0, "{what}: the next write clears it");
    }
    let mut d = Dev::new(Mounts::default());
    for reg in [CMD_BASE, CMD_SIZE, RESP_BASE, RESP_SIZE] {
        let was = d.m.bus_read(reg);
        d.m.bus_write(reg, 0o40);
        assert_eq!(d.m.bus_read(reg), was, "{reg:o}: ignored while enabled");
    }
}

/// **An index fault**: a producer write claiming more commands than the
/// ring holds, and a response consumer write passing the response producer,
/// are ignored and set status `<3>`, which the next write of 160 clears.
#[test]
fn an_index_that_claims_too_much_is_a_fault_and_ignored() {
    let mut d = Dev::new(Mounts::default());
    d.m.bus_write(CMD_PROD, 5);
    assert_eq!(d.m.bus_read(CMD_PROD), 0, "5 commands in a ring of 4: ignored");
    assert_ne!(d.m.bus_read(STATUS) & 0b1000, 0, "the fault");
    d.m.bus_write(CONTROL, 1);
    assert_eq!(d.m.bus_read(STATUS) & 0b1000, 0, "cleared by a write of 160");
    d.m.bus_write(RESP_CONS, 1);
    assert_eq!(d.m.bus_read(RESP_CONS), 0, "past the response producer: ignored");
    assert_ne!(d.m.bus_read(STATUS) & 0b1000, 0, "the fault");
    d.m.bus_write(CMD_PROD, 4);
    assert_eq!(d.m.bus_read(CMD_PROD), 4, "4 in a ring of 4 is full, and allowed");
}

// --- rings, order and time ----------------------------------------------------

/// **Nothing completes within the producer write, and a response appears at
/// its due time and not a nanosecond before**: 20 us and 100 us a KiB of the
/// lengths the entry names, from when the producer write landed. The
/// command ring's consumer and the response ring's producer move together.
#[test]
fn a_response_comes_at_its_due_time_and_not_before() {
    let f = Folder::new("due");
    f.file("x", b"hello");
    let mut d = Dev::new(default_root(&f));
    d.m.ns = 1_000_000;
    let a = d.a(b"/x");
    let before = d.m.main.clone();
    d.post(Cmd { op: op::OPEN, flags: PROBE, a, b: (BUF_B, 1000), ..Default::default() });
    let due = 1_000_000 + cost(2, 1000);
    assert_eq!(d.m.bus_read(RESP_PROD), 0, "not within the producer write");
    assert_eq!(d.m.bus_read(CMD_CONS), 0);
    d.m.ns = due - 1;
    assert_eq!(d.m.bus_read(RESP_PROD), 0, "a nanosecond before");
    d.m.advance_file_device();
    assert_eq!(d.m.main[RESP_RING as usize..], before[RESP_RING as usize..], "memory unchanged");
    assert!(!d.m.dma_written);
    d.m.ns = due;
    assert_eq!(d.m.bus_read(RESP_PROD), 1, "at its due time");
    assert_eq!(d.m.bus_read(CMD_CONS), 1);
    assert!(d.m.dma_written, "the device wrote memory behind the processor");
    let r = d.response(0);
    assert_eq!((r.status(), r.len(), r.flags()), (0, 5, 0), "{r:?}");
}

/// **One at a time, in command order**: three commands posted together
/// complete each a command's time after the one before, answered in order
/// with their tags.
#[test]
fn commands_run_one_at_a_time_in_order() {
    let f = Folder::new("order");
    let mut d = Dev::new(default_root(&f));
    let mut tags = Vec::new();
    for n in [1u32, 100, 1024] {
        tags.push(d.tag.wrapping_add(1) as u32);
        d.tag = d.tag.wrapping_add(1);
        let slot = CMD_RING as usize + 8 * d.prod as usize;
        d.m.main[slot..slot + 8].copy_from_slice(&[
            (d.tag as u32 | op::LOG << 16).into(),
            0,
            BUF_A.into(),
            u64::from(n),
            0,
            0,
            0,
            0,
        ]);
        d.prod += 1;
    }
    d.m.bus_write(CMD_PROD, 3);
    let mut due = 0;
    for (k, n) in [1u32, 100, 1024].into_iter().enumerate() {
        due += cost(n, 0);
        d.m.ns = due - 1;
        assert_eq!(d.m.bus_read(RESP_PROD), (k as u32).into(), "command {k} not yet");
        d.m.ns = due;
        assert_eq!(d.m.bus_read(RESP_PROD), (k as u32 + 1).into(), "command {k} at {due}");
    }
    for (k, tag) in tags.into_iter().enumerate() {
        let r = d.response(k as u16);
        assert_eq!((r.tag(), r.status(), r.op()), (tag, 0, op::LOG));
    }
}

/// **The rings wrap**: at 1, 2 and 256 entries, and the free-running
/// 16-bit indexes across 2^16, every command answered once and in order.
#[test]
fn the_rings_wrap_at_their_size_and_across_2_to_the_16() {
    for (log2, n) in [(0u32, 5u32), (1, 7), (8, 600), (0, 65_540)] {
        let mut d = Dev::with_rings(Mounts::default(), log2, log2);
        d.a(b"/");
        for k in 0..n {
            let r = d.run(Cmd { op: op::OPEN, flags: PROBE, a: (BUF_A, 1), ..Default::default() });
            assert_eq!((r.status(), r.flags() & 1), (0, 1), "size {log2}, command {k}");
        }
        assert_eq!(d.m.bus_read(CMD_CONS), (n & 0xffff).into());
        assert_eq!(d.m.bus_read(RESP_PROD), (n & 0xffff).into());
        assert_eq!(d.m.bus_read(RESP_CONS), (n & 0xffff).into());
    }
}

/// **A full response ring holds commands back**: with one response slot
/// unread, the next command waits, and runs a command's time after the
/// processor frees the slot.
#[test]
fn a_full_response_ring_holds_the_next_command() {
    let mut d = Dev::with_rings(Mounts::default(), 2, 0);
    d.a(b"/");
    let c = Cmd { op: op::OPEN, flags: PROBE, a: (BUF_A, 1), ..Default::default() };
    d.post(c);
    d.post(c);
    d.m.ns = 10_000_000;
    assert_eq!(d.m.bus_read(RESP_PROD), 1, "one answered, the other held");
    assert_eq!(d.m.bus_read(CMD_CONS), 1);
    d.m.bus_write(RESP_CONS, 1);
    d.m.ns = 10_000_000 + cost(1, 0) - 1;
    assert_eq!(d.m.bus_read(RESP_PROD), 1, "not before its time from the freeing");
    d.m.ns += 1;
    assert_eq!(d.m.bus_read(RESP_PROD), 2);
}

/// **Word 100 `<7>` is a level under 160 `<8>`**: up while a response
/// waits and the enable is on, the processor's interrupt with it; cleared by
/// consuming the response, or by the enable going off. Status `<8>` shows the
/// same ungated. The level is known from the due time, before anything
/// looks at the device.
#[test]
fn the_interrupt_is_a_level_under_its_enable() {
    let mut d = Dev::new(Mounts::default());
    d.a(b"/");
    d.m.bus_write(CONTROL, 0x101);
    assert_eq!(d.m.bus_read(CONTROL), 0x101);
    d.post(Cmd { op: op::OPEN, flags: PROBE, a: (BUF_A, 1), ..Default::default() });
    let due = cost(1, 0);
    assert!(!d.m.interrupt_at(due - 1));
    assert!(d.m.interrupt_at(due), "known from the due time");
    d.m.ns = due;
    assert_eq!(d.m.interrupt_sources(), 1 << 7);
    assert_eq!(d.m.bus_read(INTERRUPTS), 1 << 7);
    assert_ne!(d.m.bus_read(STATUS) & 0x100, 0);
    d.m.bus_write(CONTROL, 1);
    assert_eq!(d.m.bus_read(INTERRUPTS), 0, "the enable off");
    assert!(!d.m.interrupt());
    assert_ne!(d.m.bus_read(STATUS) & 0x100, 0, "status <8> ungated");
    d.m.bus_write(CONTROL, 0x101);
    assert_eq!(d.m.bus_read(INTERRUPTS), 1 << 7, "a level: up again");
    d.m.bus_write(RESP_CONS, 1);
    assert_eq!(d.m.bus_read(INTERRUPTS), 0, "consumed");
    assert_eq!(d.m.bus_read(STATUS) & 0x100, 0);
}

// --- disable and reset ------------------------------------------------------------

/// **Disable is the reset**: a queued command is dropped with no response
/// and no memory written, every handle is closed and a write discarded, its
/// temporary file gone and the target unchanged, the indexes and the
/// interrupt enable 0, and quiet at once. Reset devices, a write of word 104
/// with 1 (contract Q11), does the same, and clears status `<2>` and `<3>`.
#[test]
fn a_disable_or_a_machine_reset_drops_everything() {
    for how in ["disable", "reset devices"] {
        let f = Folder::new(&format!("reset-{}", how.len()));
        f.file("keep", b"old");
        let mut d = Dev::new(default_root(&f));
        d.m.bus_write(CONTROL, 0x101);
        let r = d.open("/keep", READ);
        let w = d.open("/keep", WRITE);
        assert_eq!(d.write(w.handle(), 0, b"new").status(), 0);
        assert_eq!(d.m.bus_read(STATUS) >> 16 & 0xff, 2, "two handles");
        let temps = |f: &Folder| {
            std::fs::read_dir(f.path())
                .unwrap()
                .filter(|e| {
                    e.as_ref().unwrap().file_name().to_string_lossy().starts_with(".quux-write-")
                })
                .count()
        };
        assert_eq!(temps(&f), 1, "the write's temporary file");
        d.a(b"/");
        d.post(Cmd { op: op::OPEN, flags: PROBE, a: (BUF_A, 1), ..Default::default() });
        let before = d.m.main.clone();
        match how {
            "disable" => d.m.bus_write(CONTROL, 0),
            _ => d.m.bus_write(PAGE + 0o104, 1),
        }
        let s = d.m.bus_read(STATUS);
        assert_eq!(s & 0b11, 0b10, "{how}: disabled and quiet, {s:b}");
        assert_eq!(s >> 16, 0, "{how}: no handle open");
        assert_eq!(d.m.bus_read(CONTROL), 0, "{how}: the interrupt enable too");
        d.m.ns += 1_000_000_000;
        d.m.advance_file_device();
        assert_eq!(d.m.main, before, "{how}: nothing written");
        for reg in [CMD_PROD, CMD_CONS, RESP_PROD, RESP_CONS] {
            assert_eq!(d.m.bus_read(reg), 0, "{how}: {reg:o}");
        }
        assert_eq!(temps(&f), 0, "{how}: the temporary file removed");
        assert_eq!(std::fs::read(f.path().join("keep")).unwrap(), b"old", "{how}");
        // Enabled again, the handles are gone.
        d.m.bus_write(CONTROL, 1);
        (d.prod, d.cons) = (0, 0);
        let (rr, _) = d.read(r.handle(), 0, 10);
        assert_eq!(rr.status(), status::BAD_HANDLE, "{how}");
    }
}

/// **On QUUX `PROG.UNIBUS.RESET` does not reach the device** (contract
/// Q11): a program that raises and drops `INTERRUPT-CONTROL`'s
/// `PROG.UNIBUS.RESET`, `<36>` on QUUX (contract G2 appendix A1.6), leaves
/// it enabled, its handles open and its queue as it was, on both engines;
/// the queued command then completes.
#[test]
fn a_prog_unibus_reset_leaves_the_device_as_it_was() {
    use muir::engine::Engine;
    use muir::isa::Insn;
    use muir::isa::asm::{ALU, ALWAYS, JUMP, N, SETM, START_READ, filler, m_src, target};
    const INTERRUPT_CONTROL: u64 = (2 << 19) | (0o37 << 14);
    let f = Folder::new("unibus-reset");
    f.file("data", &[7u8; 70_000]);
    for engine in ["micro", "rtl"] {
        let mut d = Dev::new(default_root(&f));
        let o = d.open("/data", READ);
        assert_eq!(o.status(), 0);
        let mut words = vec![filler(); 1024];
        // <36> up and down, then a loop reading word 100, so that `rtl`'s
        // machine keeps time.
        words[..6].copy_from_slice(&[
            Insn::new(ALU | SETM | m_src(1) | INTERRUPT_CONTROL),
            Insn::new(ALU | SETM | m_src(2) | INTERRUPT_CONTROL),
            Insn::new(ALU | SETM | m_src(3) | START_READ),
            filler(),
            filler(),
            Insn::new(JUMP | target(2) | ALWAYS | N),
        ]);
        d.m.load_prom(&words);
        support::prom_program_in_ram(&mut d.m);
        d.m.mmem[1] = 1 << 36;
        d.m.mmem[3] = support::quux_map(&mut d.m, 1, PAGE | 0o100).into();
        let mut e: Box<dyn Engine> = match engine {
            "micro" => Box::new(muir::micro::Micro::new(d.m)),
            _ => {
                // Its clock where the scripted driver left the machine's.
                let ns = d.m.ns;
                let mut r = muir::rtl::Rtl::new(d.m);
                r.set_clock(ns);
                Box::new(r)
            }
        };
        e.boot();
        // A READ of 64 KiB queued, some 6 ms of the device's time.
        let m = e.machine_mut();
        let slot = CMD_RING as usize + 8 * d.prod as usize;
        m.main[slot..slot + 8].copy_from_slice(&[
            (0o77 | op::READ << 16).into(),
            o.handle().into(),
            0,
            0,
            u64::from(BUF_B),
            65_536,
            0,
            0,
        ]);
        let prod = d.prod as u32 + 1;
        m.bus_write(CMD_PROD, prod.into());
        let look = |m: &mut Machine| {
            [m.bus_read(CONTROL), m.bus_read(STATUS), m.bus_read(CMD_PROD), m.bus_read(CMD_CONS)]
        };
        let before = look(m);
        assert_eq!(before[1] & 1, 1, "{engine}: enabled");
        assert_eq!(before[1] >> 16 & 0xff, 1, "{engine}: one handle open");
        for _ in 0..10 {
            e.step().unwrap();
        }
        assert_eq!(look(e.machine_mut()), before, "{engine}: <28> reached the device");
        for _ in 0..1_000_000 {
            if e.machine().file_device.response_producer() as u32 == prod {
                break;
            }
            e.step().unwrap();
        }
        assert_eq!(
            e.machine().file_device.response_producer() as u32,
            prod,
            "{engine}: the queued READ completed"
        );
    }
}

// --- commands ---------------------------------------------------------------------

/// **Under the time-neutral harness the words the device writes are the
/// same in every run** (`support::neutral::fix_dates`): two runs over a
/// folder whose file and subfolder were made seconds apart, as a run's are
/// made when it starts, OPEN, CLOSE and list them, and main memory, which a
/// workload's end digest takes, is the same after both; every date the
/// device reports is the harness's, `FIXED_UNIX`.
#[test]
fn under_the_time_neutral_harness_the_device_reports_the_harness_s_date() {
    let run = |made: u64| {
        let f = Folder::new(&format!("neutral-{made}"));
        let p = f.file("sys/data.bin", b"twelve bytes");
        set_mtime(&p, made);
        let dir = std::fs::File::open(f.path().join("sys")).unwrap();
        dir.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(made + 1)).unwrap();
        let mut d = Dev::new(default_root(&f));
        support::neutral::fix_dates(&mut d.m);
        let o = d.open("/sys/data.bin", READ);
        assert_eq!(o.status(), 0);
        let c = d.close(o.handle(), 0, 0);
        let (r, recs) = d.directory("/", 0, 4096);
        assert_eq!(r.status(), 0);
        let dates = (o.mtime(), c.mtime(), recs.iter().map(|r| r.mtime).collect::<Vec<_>>());
        let pending = muir::machine::Pending { md: None, pdl: None, spc: None };
        (dates, support::neutral::digest(&d.m, pending, (0, 0), true))
    };
    let (a, b) = (run(1_791_491_843), run(1_791_491_940));
    let fixed = support::neutral::FIXED_UNIX as u32;
    assert_eq!(a.0, (fixed, fixed, vec![fixed]), "the harness's date, everywhere");
    assert_eq!(a, b, "two runs made 97 s apart");
}

/// **OPEN and READ**: the length and mtime; positional reads of any size,
/// short at the end, 0 at the end, FOR past it; a READ of n bytes writing
/// `ceil(n/4)` words with the bytes past n zero and no other word; CLOSE
/// of a read handle answering its length and date.
#[test]
fn open_and_read() {
    let f = Folder::new("read");
    let data: Vec<u8> = (0..3000u32).map(|k| (k * 7 + 3) as u8).collect();
    let p = f.file("sys/data.bin", &data);
    set_mtime(&p, 1_700_000_123);
    let mut d = Dev::new(default_root(&f));
    let o = d.open("/sys/data.bin", READ);
    assert_eq!((o.status(), o.len(), o.mtime(), o.flags()), (0, 3000, 1_700_000_123, 0));
    assert!((1..=64).contains(&o.handle()));
    for (off, want, got) in
        [(0, 10, 10), (5, 1024, 1024), (2990, 100, 10), (3000, 10, 0), (7, 0, 0)]
    {
        d.m.main[BUF_B as usize..BUF_B as usize + 300].fill(0xdead_beef);
        let (r, bytes) = d.read(o.handle(), off, want);
        assert_eq!((r.status(), r.count()), (0, got), "offset {off}, {want} wanted");
        assert_eq!(bytes, data[off as usize..(off + got) as usize]);
        let words = (got as usize).div_ceil(4);
        if got % 4 != 0 {
            let last = fixnum(d.m.main[BUF_B as usize + words - 1]);
            assert_eq!(last >> (8 * (got % 4)), 0, "the bytes past n are 0");
        }
        assert!(
            d.m.main[BUF_B as usize + words..BUF_B as usize + 300]
                .iter()
                .all(|&w| w == 0xdead_beef),
            "no other word written"
        );
    }
    let (r, _) = d.read(o.handle(), 3001, 1);
    assert_eq!(r.status(), status::FOR, "past the end");
    let c = d.close(o.handle(), 0, 0);
    assert_eq!((c.status(), c.len(), c.mtime()), (0, 3000, 1_700_000_123));
    let (r, _) = d.read(o.handle(), 0, 1);
    assert_eq!(r.status(), status::BAD_HANDLE, "closed");
}

/// **A write lands whole at CLOSE**: the target is unchanged until then,
/// and the temporary file is in its folder and never listed; CLOSE answers
/// the final length and date; supersede starts empty (how `:OVERWRITE` and
/// `:TRUNCATE` are sent); append starts from the old bytes; abort leaves the
/// old file and removes the temporary one.
#[test]
fn a_write_lands_whole_at_close() {
    let f = Folder::new("atomic");
    let target = f.file("home/lispm/f.text", b"the old contents, longer");
    let mut d = Dev::new(default_root(&f));
    let o = d.open("/home/lispm/f.text", WRITE);
    assert_eq!((o.status(), o.len(), o.mtime()), (0, 0, 0), "{o:?}");
    assert_eq!(d.write(o.handle(), 0, b"new").count(), 3);
    assert_eq!(d.write(o.handle(), 3, b" data").count(), 5);
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"the old contents, longer",
        "unchanged before CLOSE"
    );
    let hidden: Vec<String> = std::fs::read_dir(f.path().join("home/lispm"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".quux-write-"))
        .collect();
    assert_eq!(hidden.len(), 1, "the temporary file beside it");
    let (_, recs) = d.directory("/home/lispm", 0, 4096);
    assert_eq!(names(&recs), ["f.text"], "and never listed");
    let c = d.close(o.handle(), 0, 0);
    assert_eq!((c.status(), c.len()), (0, 8));
    assert_eq!(c.mtime(), mtime(&target));
    assert_eq!(std::fs::read(&target).unwrap(), b"new data", "supersede: the new bytes alone");
    assert!(!f.path().join("home/lispm").join(&hidden[0]).exists(), "the temporary file renamed");

    // Append: from the old bytes; the reply's length is theirs.
    let o = d.open("/home/lispm/f.text", WRITE | IF_EXISTS_APPEND);
    assert_eq!((o.status(), o.len()), (0, 8));
    d.write(o.handle(), 8, b"!");
    assert_eq!(d.close(o.handle(), 0, 0).len(), 9);
    assert_eq!(std::fs::read(&target).unwrap(), b"new data!");

    // Abort: the old file stands and the temporary file goes.
    let o = d.open("/home/lispm/f.text", WRITE);
    d.write(o.handle(), 0, b"lost");
    let c = d.close(o.handle(), ABORT, 0);
    assert_eq!((c.status(), c.len(), c.mtime()), (0, 0, 0));
    assert_eq!(std::fs::read(&target).unwrap(), b"new data!");
    assert_eq!(manifest(f.path()).len(), 3, "home, lispm and f.text: no temporary file left");

    // A new file, created.
    let c = d.put("/home/lispm/g", b"g");
    assert_eq!((c.status(), c.len()), (0, 1));
    assert_eq!(std::fs::read(f.path().join("home/lispm/g")).unwrap(), b"g");
}

/// **A stale temporary file is never listed**, one a killed run left: not
/// by DIRECTORY, not by COMPLETE.
#[test]
fn a_stale_temporary_file_is_never_listed() {
    let f = Folder::new("stale");
    f.file(".quux-write-999-1", b"stale");
    f.file(".dotfile", b"listed");
    let mut d = Dev::new(default_root(&f));
    let (_, recs) = d.directory("/", 0, 4096);
    assert_eq!(names(&recs), [".dotfile"]);
    let (r, text) = d.complete("/.q");
    assert_eq!((r.status(), r.count(), r.w6(), text.as_str()), (0, 0, 0, ""));
}

/// **The date set at CLOSE** (`<1>`), COPY-FILE's `:COPY-CREATION-DATE`:
/// the file's mtime is the second given, and the reply says so.
#[test]
fn close_sets_the_date() {
    let f = Folder::new("date");
    let mut d = Dev::new(default_root(&f));
    let o = d.open("/new", WRITE);
    d.write(o.handle(), 0, b"x");
    let c = d.close(o.handle(), SET_DATE, 1_234_567_890);
    assert_eq!((c.status(), c.mtime()), (0, 1_234_567_890));
    assert_eq!(mtime(&f.path().join("new")), 1_234_567_890);
    let p = d.probe("/new");
    assert_eq!(p.mtime(), 1_234_567_890);
}

/// **OPEN's refusals**: FNF, DNF, FAE (if-exists error, at OPEN and at
/// CLOSE when the name appeared meanwhile), FNF for if-does-not-exist
/// error, IOD for a directory, WKF for neither file nor directory, NER at
/// 64 handles, and bad argument for flags out of range.
#[test]
fn open_refuses_what_it_should() {
    let f = Folder::new("openerr");
    f.file("a/file", b"x");
    f.dir("a/dir");
    let mut d = Dev::new(default_root(&f));
    assert_eq!(d.open("/a/none", READ).status(), status::FNF);
    assert_eq!(d.probe("/a/none").status(), status::FNF);
    assert_eq!(d.open("/a/none/x", READ).status(), status::DNF);
    assert_eq!(d.open("/a/file/x", READ).status(), status::DNF, "a file on the way");
    assert_eq!(d.open("/b/x", WRITE).status(), status::DNF, "a missing parent; nothing created");
    assert!(!f.path().join("b").exists());
    assert_eq!(d.open("/a/dir", READ).status(), status::IOD);
    assert_eq!(d.open("/a/dir", WRITE).status(), status::IOD);
    assert_eq!(d.open("/a/file", WRITE | IF_EXISTS_ERROR).status(), status::FAE);
    assert_eq!(d.open("/a/new", WRITE | IF_DOES_NOT_EXIST_ERROR).status(), status::FNF);
    assert!(!f.path().join("a/new").exists());
    for flags in [3, 3 << 2 | WRITE, 1 << 5] {
        assert_eq!(d.open("/a/file", flags).status(), status::BAD_ARGUMENT, "flags {flags:b}");
    }
    // If-exists error at CLOSE: a file that appeared meanwhile wins.
    let o = d.open("/a/race", WRITE | IF_EXISTS_ERROR);
    assert_eq!(o.status(), 0);
    d.write(o.handle(), 0, b"mine");
    f.file("a/race", b"theirs");
    assert_eq!(d.close(o.handle(), 0, 0).status(), status::FAE);
    assert_eq!(std::fs::read(f.path().join("a/race")).unwrap(), b"theirs");
    // Neither file nor directory.
    let sock = f.path().join("a/sock");
    let _l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    assert_eq!(d.open("/a/sock", READ).status(), status::WKF);
    // 64 handles, then NER; a probe takes none.
    let mut open = Vec::new();
    for k in 0..64 {
        let o = d.open("/a/file", READ);
        assert_eq!(o.status(), 0, "handle {k}");
        open.push(o.handle());
    }
    open.sort();
    assert_eq!(open, (1..=64).collect::<Vec<_>>(), "numbered 1-64");
    assert_eq!(d.m.bus_read(STATUS) >> 16 & 0xff, 64);
    assert_eq!(d.open("/a/file", READ).status(), status::NER);
    assert_eq!(d.probe("/a/file").status(), 0);
}

/// **Handles and buffers**: a READ on a write handle or a WRITE on a read
/// one, a handle never opened, 0 and 65 are bad handles; a buffer off its
/// line, over 64 KiB or past main memory is a bad buffer; an opcode not in
/// the ten answers UOP; a WRITE leaving a hole answers FOR.
#[test]
fn handles_buffers_and_opcodes_are_checked() {
    let f = Folder::new("faults");
    f.file("r", b"read me");
    let mut d = Dev::new(default_root(&f));
    let r = d.open("/r", READ);
    let w = d.open("/w", WRITE);
    assert_eq!(d.read(w.handle(), 0, 4).0.status(), status::BAD_HANDLE);
    assert_eq!(d.write(r.handle(), 0, b"x").status(), status::BAD_HANDLE);
    for h in [0, 3, 65, 0xffff_ffff] {
        assert_eq!(d.read(h, 0, 4).0.status(), status::BAD_HANDLE, "handle {h}");
        assert_eq!(d.close(h, 0, 0).status(), status::BAD_HANDLE, "handle {h}");
    }
    let words = d.m.main.len() as u32;
    for (what, b) in [
        ("off a line", (BUF_B + 1, 4)),
        ("over 64 KiB", (BUF_B, 65537)),
        ("past main memory", (words - 4, 17)),
    ] {
        let x = d.run(Cmd { op: op::READ, handle: r.handle(), b, ..Default::default() });
        assert_eq!(x.status(), status::BAD_BUFFER, "{what}");
    }
    let x =
        d.run(Cmd { op: op::READ, handle: r.handle(), b: (BUF_B, 65536), ..Default::default() });
    assert_eq!((x.status(), x.count()), (0, 7), "64 KiB exactly is a buffer");
    let x = d.run(Cmd { op: op::OPEN, a: (BUF_A + 2, 2), ..Default::default() });
    assert_eq!(x.status(), status::BAD_BUFFER, "A off a line");
    for opcode in [0, 11, 0xff] {
        assert_eq!(
            d.run(Cmd { op: opcode, ..Default::default() }).status(),
            status::UOP,
            "{opcode}"
        );
    }
    assert_eq!(d.write(w.handle(), 1, b"hole").status(), status::FOR, "a hole");
    assert_eq!(d.write(w.handle(), 0, b"ok").status(), 0);
    assert_eq!(d.write(w.handle(), 2, b"ok").status(), 0, "at the end is no hole");
    assert_eq!(d.close(w.handle(), SET_DATE << 1, 0).status(), status::BAD_ARGUMENT, "CLOSE <2>");
}

/// **DIRECTORY**: records sorted bytewise with length, mtime and the
/// directory bit, dot files listed, continued by the cookie when B is full,
/// a file WKF, a missing directory DNF, B under 272 bytes a bad argument; the
/// folder unchanged by every read.
#[test]
fn directory_lists_sorted_and_continues() {
    let f = Folder::new("dir");
    let long = "L".repeat(255);
    for (n, bytes) in
        [("b", &b"bb"[..]), ("a", b"a"), ("B", b"BBB"), (".x", b""), (long.as_str(), b"l")]
    {
        let p = f.file(&format!("d/{n}"), bytes);
        set_mtime(&p, 1_000_000 + n.len() as u64);
    }
    f.dir("d/sub");
    let before = manifest(f.path());
    let mut d = Dev::new(default_root(&f));
    let (r, recs) = d.directory("/d", 0, 4096);
    assert_eq!((r.status(), r.w6()), (0, 0), "all of it, the next cookie 0");
    assert_eq!(names(&recs), [".x", "B", long.as_str(), "a", "b", "sub"]);
    assert_eq!(r.count() as usize, recs.iter().map(|r| 4 * (3 + r.name.len().div_ceil(4))).sum());
    let b = &recs[1];
    assert_eq!((b.dir, b.ro, b.len, b.mtime), (false, false, 3, 1_000_001));
    assert!(recs[5].dir && recs[5].len == 0);
    // Continued: 272 bytes hold the long name's record alone.
    let mut got = Vec::new();
    let mut cookie = 0;
    loop {
        let (r, recs) = d.directory("/d", cookie, 272);
        assert_eq!(r.status(), 0);
        assert!(!recs.is_empty());
        got.extend(recs);
        cookie = r.w6();
        if cookie == 0 {
            break;
        }
    }
    assert_eq!(names(&got), [".x", "B", long.as_str(), "a", "b", "sub"]);
    assert_eq!(d.directory("/d/a", 0, 4096).0.status(), status::WKF);
    assert_eq!(d.directory("/d/none", 0, 4096).0.status(), status::DNF);
    assert_eq!(d.directory("/d", 0, 271).0.status(), status::BAD_ARGUMENT);
    let o = d.open("/d/b", READ);
    d.read(o.handle(), 0, 100);
    d.close(o.handle(), 0, 0);
    d.probe("/d/B");
    d.complete("/d/");
    assert_eq!(manifest(f.path()), before, "reads change nothing");
}

/// **COMPLETE's three outcomes**: an entry exactly the completion (`<2>`,
/// and `<3>` a directory), a longer one only (matches, no `<2>`), and none;
/// case kept.
#[test]
fn complete_extends_the_prefix() {
    let f = Folder::new("complete");
    f.file("s/compile.lisp", b"");
    f.file("s/compiler.lisp", b"");
    f.file("s/Comp", b"");
    f.dir("s/zdir");
    let mut d = Dev::new(default_root(&f));
    let (r, t) = d.complete("/s/com");
    assert_eq!((r.status(), t.as_str(), r.w6(), r.flags()), (0, "compile", 2, 0), ":NEW");
    let (r, t) = d.complete("/s/compile.");
    assert_eq!((t.as_str(), r.w6(), r.flags()), ("compile.lisp", 1, 0b100), "exact: :OLD");
    let (r, t) = d.complete("/s/z");
    assert_eq!((t.as_str(), r.w6(), r.flags()), ("zdir", 1, 0b1100), "exact, a directory");
    let (r, t) = d.complete("/s/q");
    assert_eq!((r.status(), r.count(), r.w6(), t.as_str()), (0, 0, 0, ""), "no match");
    let (r, t) = d.complete("/s/C");
    assert_eq!((t.as_str(), r.w6()), ("Comp", 1), "case kept");
    let (r, _) = d.complete("/s/");
    assert_eq!((r.status(), r.w6()), (0, 4), "an empty prefix matches all");
    assert_eq!(d.complete("/none/x").0.status(), status::DNF);
}

/// **DELETE, RENAME and CREATE-DIRECTORY**: a file and an empty directory
/// deleted, a full one DNE; RENAME never overwriting (REF), onto a missing
/// folder DNF, of a missing name FNF; CREATE-DIRECTORY one level, DNF for a
/// missing parent, DAE for a directory and FAE for a file.
#[test]
fn delete_rename_and_create_directory() {
    let f = Folder::new("dre");
    f.file("x/a", b"a");
    f.file("x/b", b"b");
    f.file("x/full/in", b"");
    f.dir("x/empty");
    let mut d = Dev::new(default_root(&f));
    assert_eq!(d.delete("/x/a").status(), 0);
    assert!(!f.path().join("x/a").exists());
    assert_eq!(d.delete("/x/a").status(), status::FNF);
    assert_eq!(d.delete("/x/empty").status(), 0);
    assert_eq!(d.delete("/x/full").status(), status::DNE);
    assert_eq!(d.delete("/y/z").status(), status::DNF);
    assert_eq!(d.rename("/x/b", "/x/full").status(), status::REF, "never onto a directory");
    f.file("x/c", b"c");
    assert_eq!(d.rename("/x/b", "/x/c").status(), status::REF, "never onto a file");
    assert_eq!(std::fs::read(f.path().join("x/c")).unwrap(), b"c");
    assert_eq!(d.rename("/x/b", "/y/b").status(), status::DNF);
    assert_eq!(d.rename("/x/none", "/x/n2").status(), status::FNF);
    assert_eq!(d.rename("/x/b", "/x/full/b2").status(), 0);
    assert_eq!(std::fs::read(f.path().join("x/full/b2")).unwrap(), b"b");
    assert_eq!(d.rename("/x/full", "/x/moved").status(), 0, "a directory too");
    assert!(f.path().join("x/moved/b2").exists());
    assert_eq!(d.mkdir("/x/new").status(), 0);
    assert!(f.path().join("x/new").is_dir());
    assert_eq!(d.mkdir("/x/new").status(), status::DAE);
    assert_eq!(d.mkdir("/x/c").status(), status::FAE);
    assert_eq!(d.mkdir("/x/q/r").status(), status::DNF, "one level only");
    assert!(!f.path().join("x/q").exists());
}

/// **Names**: absolute, printable ASCII, components of 1-255 bytes, a
/// single trailing `/`, and neither `.` nor `..`; anything else IPS. Case is
/// exact: a name that matches only when case is ignored is not found.
#[test]
fn names_outside_the_rules_are_refused() {
    let f = Folder::new("names");
    f.file("sys/File", b"f");
    let mut d = Dev::new(default_root(&f));
    let long = format!("/{}", "a".repeat(256));
    for bad in [
        "",
        "sys/File",
        "/sys//File",
        "/sys/./File",
        "/sys/../sys/File",
        "/..",
        "/a\tb",
        "/a\u{7f}",
        "/\u{e9}",
        &long,
    ] {
        assert_eq!(d.probe(bad).status(), status::IPS, "{bad:?}");
        assert_eq!(d.open(bad, WRITE).status(), status::IPS, "{bad:?}");
        assert_eq!(d.mkdir(bad).status(), status::IPS, "{bad:?}");
    }
    let too_long = format!("/{}", "a/".repeat(512));
    assert_eq!(d.probe(&too_long).status(), status::IPS, "over 1,024 bytes");
    assert_eq!(d.probe("/sys/File").status(), 0);
    assert_eq!(d.probe("/sys/").status(), 0, "a trailing /");
    assert_eq!(d.probe("/sys/file").status(), status::FNF, "case is exact");
    assert_eq!(d.probe("/SYS/File").status(), status::DNF);
    assert_eq!(
        d.probe(&format!("/{}", "b".repeat(255))).status(),
        status::FNF,
        "255 bytes is a name"
    );
}

/// **The host's EINVAL is IPS**, a name the host's file system refuses;
/// the other errno mappings of the table.
#[test]
fn host_errors_map_to_the_table() {
    use std::io::{Error, ErrorKind};
    let st = |e: Error| file_device::host_status(&e);
    assert_eq!(st(Error::from_raw_os_error(22)), status::IPS, "EINVAL");
    assert_eq!(st(Error::from(ErrorKind::NotFound)), status::FNF);
    assert_eq!(st(Error::from_raw_os_error(13)), status::ACC, "EACCES");
    assert_eq!(st(Error::from_raw_os_error(1)), status::ACC, "EPERM");
    assert_eq!(st(Error::from_raw_os_error(30)), status::ATF, "EROFS");
    assert_eq!(st(Error::from_raw_os_error(28)), status::NMR, "ENOSPC");
    assert_eq!(st(Error::from_raw_os_error(5)), status::DAT, "EIO");
}

/// **Symlinks** are followed inside their mount's folder; one leaving it
/// answers ACC, and so does a loop; DIRECTORY leaves out the ones that leave.
#[test]
fn symlinks_stay_inside_their_mount() {
    let f = Folder::new("links");
    let outside = Folder::new("links-outside");
    outside.file("secret", b"s");
    f.file("real/file", b"inside");
    std::os::unix::fs::symlink("real/file", f.path().join("in")).unwrap();
    std::os::unix::fs::symlink("real", f.path().join("indir")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("secret"), f.path().join("out")).unwrap();
    std::os::unix::fs::symlink("loop2", f.path().join("loop1")).unwrap();
    std::os::unix::fs::symlink("loop1", f.path().join("loop2")).unwrap();
    let mut d = Dev::new(default_root(&f));
    assert_eq!(d.slurp("/in"), b"inside");
    assert_eq!(d.slurp("/indir/file"), b"inside");
    assert_eq!(d.open("/out", READ).status(), status::ACC);
    assert_eq!(d.open("/loop1", READ).status(), status::ACC);
    let (_, recs) = d.directory("/", 0, 4096);
    assert_eq!(names(&recs), ["in", "indir", "real"]);
}

// --- mounts ----------------------------------------------------------------------

/// **The default folder, a named mount over it and another name**: the
/// named one overrides the default's entry of that name; DIRECTORY of `/`
/// lists them with each one's read-only bit, and a probe says the same; an
/// unmounted name is DNF below and FNF itself; RENAME across mounts RAD; a
/// mount's root is not deleted or renamed (ACC).
#[test]
fn mounts_override_and_list_at_the_root() {
    let base = Folder::new("mount-base");
    base.file("sys/old", b"default's sys");
    base.file("site/s", b"site");
    base.dir("home/lispm");
    let sys = Folder::new("mount-sys");
    sys.file("new", b"named sys");
    let scratch = Folder::new("mount-scratch");
    let mut d = Dev::new(mounts(&[
        base.path().display().to_string(),
        format!("sys={},ro", sys.path().display()),
        format!("scratch={}", scratch.path().display()),
    ]));
    let (_, recs) = d.directory("/", 0, 4096);
    let got: Vec<(&str, bool, bool)> =
        recs.iter().map(|r| (r.name.as_str(), r.dir, r.ro)).collect();
    assert_eq!(
        got,
        [
            ("home", true, false),
            ("scratch", true, false),
            ("site", true, false),
            ("sys", true, true)
        ]
    );
    assert_eq!(d.slurp("/sys/new"), b"named sys", "the named mount wins");
    assert_eq!(d.open("/sys/old", READ).status(), status::FNF);
    let p = d.probe("/sys/");
    assert_eq!((p.status(), p.flags()), (0, 0b11), "a directory, read-only");
    assert_eq!(d.probe("/site/s").flags(), 0);
    assert_eq!(d.open("/sys/new", READ).flags(), 0b10, "every OPEN says read-only");
    assert_eq!(d.probe("/nothing").status(), status::FNF, "an unmounted name");
    assert_eq!(d.probe("/nothing/x").status(), status::DNF, "and under it");
    assert_eq!(d.put("/scratch/x", b"x").status(), 0);
    assert_eq!(d.rename("/scratch/x", "/site/x").status(), status::RAD);
    assert_eq!(d.rename("/site/s", "/scratch/s").status(), status::RAD);
    assert_eq!(d.delete("/scratch").status(), status::ACC, "a mount's own root");
    assert_eq!(d.rename("/scratch", "/scratch2").status(), status::ACC);
    assert_eq!(d.delete("/").status(), status::ACC);
    assert_eq!(d.mkdir("/scratch").status(), status::DAE);
}

/// **A read-only mount refuses every write with ATF** and is left as it
/// was: OPEN write, DELETE, RENAME, CREATE-DIRECTORY.
#[test]
fn a_read_only_mount_refuses_every_write() {
    let f = Folder::new("ro");
    f.file("sys/a", b"a");
    f.dir("sys/d");
    let before = manifest(f.path());
    for spec in
        [format!("{},ro", f.path().display()), format!("sys={},ro", f.path().join("sys").display())]
    {
        let mut d = Dev::new(mounts(std::slice::from_ref(&spec)));
        assert_eq!(d.open("/sys/a", WRITE).status(), status::ATF, "{spec}");
        assert_eq!(d.open("/sys/new", WRITE).status(), status::ATF, "{spec}");
        assert_eq!(d.open("/sys/a", WRITE | IF_EXISTS_APPEND).status(), status::ATF, "{spec}");
        assert_eq!(d.delete("/sys/a").status(), status::ATF, "{spec}");
        assert_eq!(d.delete("/sys/d").status(), status::ATF, "{spec}");
        assert_eq!(d.rename("/sys/a", "/sys/b").status(), status::ATF, "{spec}");
        assert_eq!(d.mkdir("/sys/e").status(), status::ATF, "{spec}");
        assert_eq!(d.slurp("/sys/a"), b"a", "{spec}: reads still work");
        assert_eq!(manifest(f.path()), before, "{spec}: unchanged");
    }
}

/// **With no default folder the root is read-only**, only the mounts: a
/// name there is ATF to create; with no `--file-root` at all it is empty.
#[test]
fn with_no_default_folder_the_root_is_the_mounts_and_read_only() {
    let sys = Folder::new("only-sys");
    let mut d = Dev::new(mounts(&[format!("sys={}", sys.path().display())]));
    let (_, recs) = d.directory("/", 0, 4096);
    assert_eq!(names(&recs), ["sys"]);
    assert_eq!(d.open("/x", WRITE).status(), status::ATF);
    assert_eq!(d.mkdir("/x").status(), status::ATF);
    assert_eq!(d.probe("/").flags(), 0b11, "/ is a directory, read-only");
    assert_eq!(d.put("/sys/y", b"y").status(), 0, "the mount itself is writable");
    let mut d = Dev::new(Mounts::default());
    let (r, recs) = d.directory("/", 0, 4096);
    assert_eq!((r.status(), recs.len()), (0, 0), "empty");
    assert_eq!(d.probe("/sys").status(), status::FNF);
    assert_eq!(d.probe("/sys/x").status(), status::DNF);
    assert_eq!(d.open("/sys/x", WRITE).status(), status::DNF);
}

/// **`--file-root`'s values**: a folder, or `<name>=<folder>`, each with
/// `,ro`; a name given twice, a second default folder, a folder that is not
/// there and a name that is no component are refused.
#[test]
fn mount_specifications_parse_and_refuse() {
    let f = Folder::new("specs");
    let p = f.path().display().to_string();
    let mut m = Mounts::default();
    m.add(&p).unwrap();
    m.add(&format!("sys={p},ro")).unwrap();
    assert!(m.add(&format!("{p},ro")).unwrap_err().contains("one default"));
    assert!(m.add(&format!("sys={p}")).unwrap_err().contains("sys"));
    assert!(m.add(&format!("x={p}/none")).is_err());
    assert!(Mounts::default().add(&format!("{p}/none")).is_err());
    let d = m.default.as_ref().unwrap();
    assert!(!d.ro);
    assert!(m.named["sys"].ro);
}

// --- LOG ------------------------------------------------------------------------

/// **LOG prints the line**: `log: ` and its bytes, those outside 040-176 as
/// a backslash and three octal digits; it answers OK; the device keeps the
/// lines when asked to.
#[test]
fn log_prints_the_line() {
    assert_eq!(file_device::log_line(b"report: qld-complete"), "log: report: qld-complete");
    assert_eq!(file_device::log_line(b"a\tb\\\x8d"), "log: a\\011b\\\\215");
    let mut d = Dev::new(Mounts::default());
    d.m.file_device.log = Some(Vec::new());
    let r = d.simple(op::LOG, "report: script-ends");
    assert_eq!(r.status(), 0);
    assert_eq!(d.m.file_device.log.as_ref().unwrap(), &[b"report: script-ends".to_vec()]);
    let a = d.a(&[b'x'; 1025]);
    assert_eq!(d.run(Cmd { op: op::LOG, a, ..Default::default() }).status(), status::BAD_ARGUMENT);
}

// --- checkpoints -------------------------------------------------------------------

/// **A checkpoint is refused while a handle is open or a command queued**,
/// saying which; an idle device's registers and indexes come back from one.
#[test]
fn a_checkpoint_waits_for_an_idle_device_and_keeps_its_registers() {
    use muir::checkpoint::{Reader, Writer};
    let f = Folder::new("chk");
    f.file("a", b"a");
    let mut d = Dev::with_rings(default_root(&f), 3, 1);
    assert_eq!(d.m.checkpoint_refusal(), None);
    let o = d.open("/a", READ);
    let why = d.m.checkpoint_refusal().expect("a handle is open");
    assert!(why.contains("1 handle open"), "{why}");
    d.close(o.handle(), 0, 0);
    d.a(b"/");
    d.post(Cmd { op: op::OPEN, flags: PROBE, a: (BUF_A, 1), ..Default::default() });
    let why = d.m.checkpoint_refusal().expect("a command is queued");
    assert!(why.contains("1 command queued"), "{why}");
    let _ = d.wait();
    assert_eq!(d.m.checkpoint_refusal(), None);
    d.m.bus_write(CONTROL, 0x101);
    let regs = |m: &mut Machine| (0o160..=0o171).map(|w| m.bus_read(PAGE + w)).collect::<Vec<_>>();
    let before = regs(&mut d.m);
    let mut w = Writer::new();
    d.m.save(&mut w);
    let body = w.finish();
    let mut back = quux(default_root(&f));
    back.load(&mut Reader::for_word_bits(&body, 40)).unwrap();
    assert_eq!(regs(&mut back), before);
    let mut e = Dev { m: back, prod: d.prod, cons: d.cons, cmd_log2: 3, resp_log2: 1, tag: 0o500 };
    assert_eq!(e.slurp("/a"), b"a", "and it runs on");
}

// --- both engines ---------------------------------------------------------------------

/// The engines' program: it writes word 0 of the command entry, then the
/// producer index, in the next memory cycle; then reads word 170, word 100
/// and the first word of buffer B round a loop.
mod engines {
    use super::*;
    use muir::engine::Engine;
    use muir::isa::Insn;
    use muir::isa::asm::{
        ALU, ALWAYS, JUMP, MD, SETM, SRC_MD, START_READ, START_WRITE, a_dest, filler, m_src, target,
    };
    use muir::micro::Micro;
    use muir::rtl::Rtl;

    const TAG: u32 = 0o4321;

    fn program() -> Vec<Insn> {
        let read = |m: u64, a: u64| {
            [
                Insn::new(ALU | SETM | m_src(m) | START_READ),
                filler(),
                filler(),
                Insn::new(ALU | SETM | SRC_MD | a_dest(a)),
            ]
        };
        let mut p = vec![
            Insn::new(ALU | SETM | m_src(4) | MD),
            Insn::new(ALU | SETM | m_src(3) | START_WRITE),
            // An `MD` loaded in the microcycle after a start is the word
            // written, on the CADR and QUUX alike: `tests/chip.rs`,
            // `the_engines_write_the_md_of_the_microcycle_after_the_start`.
            filler(),
            Insn::new(ALU | SETM | m_src(2) | MD),
            Insn::new(ALU | SETM | m_src(1) | START_WRITE),
        ];
        p.extend(read(5, 0o200));
        p.extend(read(6, 0o201));
        p.extend(read(7, 0o202));
        p.push(Insn::new(JUMP | target(5) | ALWAYS));
        p
    }

    /// A machine with a file open for reading at handle `h`, its rings set
    /// up, the READ of its first 8 bytes in the command slot but for word 0,
    /// which the program writes, and the interrupt enabled.
    fn machine(f: &Folder) -> (Machine, [u32; 2]) {
        let mut d = Dev::new(default_root(f));
        let o = d.open("/data", READ);
        assert_eq!(o.status(), 0);
        d.m.bus_write(CONTROL, 0x101);
        let slot = CMD_RING as usize + 8 * d.prod as usize;
        d.m.main[slot..slot + 8].copy_from_slice(&[
            0,
            o.handle().into(),
            0,
            0,
            BUF_B.into(),
            8,
            0,
            0,
        ]);
        d.m.main[BUF_B as usize] = 0o7070;
        let mut m = d.m;
        let mut words = vec![filler(); 512];
        let p = program();
        words[..p.len()].copy_from_slice(&p);
        m.load_prom(&words);
        support::prom_program_in_ram(&mut m);
        // Virtual pages of 1024 words: 1 the register page's frame, the
        // page at its word 1400; 2 the command ring; 3 buffer B.
        let rw = (1 << 27) | (1 << 26);
        m.l2_map[1] = rw | (PAGE >> 10);
        m.l2_map[2] = rw | (CMD_RING >> 10);
        m.l2_map[3] = rw | (BUF_B >> 10);
        let page = u64::from((1 << 10) | (PAGE & 0o1777));
        m.mmem[1] = page | 0o164;
        m.mmem[2] = u64::from(d.prod as u32 + 1);
        m.mmem[3] = u64::from((2 << 10) | (8 * d.prod as u32));
        m.mmem[4] = u64::from(TAG | op::READ << 16);
        m.mmem[5] = page | 0o170;
        m.mmem[6] = page | 0o100;
        m.mmem[7] = 3 << 10;
        let resp = RESP_RING + 8 * d.cons as u32;
        (m, [d.cons as u32 + 1, resp])
    }

    /// Steps `e` until the response index moves: the head's due time as the
    /// device took it, and the engine's clock after the step two before and
    /// after the step it moved in. `micro` runs the device at the edge that
    /// ends a microcycle and `rtl` as the next one begins, the same instant;
    /// so it moved in a step ending at or after the due time, and the step
    /// two before ended before it.
    fn run_to_response<E: Engine>(
        e: &mut E,
        answered: u32,
        clock: fn(&E) -> u64,
    ) -> (u64, u64, u64) {
        let mut due = None;
        let mut ends = [0u64; 2];
        for _ in 0..1_000_000 {
            e.step().unwrap();
            let m = e.machine();
            if due.is_none() {
                due = m.file_device.head_due();
            }
            if m.file_device.response_producer() as u32 == answered {
                return (due.expect("the command was taken"), ends[0], clock(e));
            }
            ends = [ends[1], clock(e)];
        }
        panic!("no response");
    }

    fn check<E: Engine>(name: &str, e: &mut E, answered: u32, resp: u32, clock: fn(&E) -> u64) {
        let (due, before, at) = run_to_response(e, answered, clock);
        assert!(before < due && due <= at, "{name}: due {due}, answered between {before} and {at}");
        let m = e.machine();
        let r = &m.main[resp as usize..resp as usize + 8];
        assert_eq!(
            fixnum(r[0]),
            TAG | op::READ << 24,
            "{name}: status 0, and the program's word 0 was read"
        );
        assert_eq!(fixnum(r[1]), 8, "{name}");
        for _ in 0..200 {
            e.step().unwrap();
        }
        let m = e.machine();
        assert_eq!(m.amem[0o200], answered.into(), "{name}: the program saw word 170 move");
        assert_eq!(m.amem[0o201], 1 << 7, "{name}: and word 100 <7>");
        assert_eq!(
            fixnum(m.amem[0o202]),
            u32::from_le_bytes(*b"0123"),
            "{name}: and the data in B, a fixnum"
        );
    }

    /// **Both engines drive the device**: the command a program writes
    /// the microcycle before its producer write is the one executed; it
    /// completes at its due time, a command's time after the write; the
    /// program sees the index, the interrupt and the data.
    #[test]
    fn both_engines_drive_the_device() {
        let f = Folder::new("engines");
        f.file("data", b"0123456789");
        let (m, [answered, resp]) = machine(&f);
        let mut e = Micro::new(m);
        e.boot();
        check("micro", &mut e, answered, resp, |e| e.machine().ns);
        let (m, [answered, resp]) = machine(&f);
        let mut r = Rtl::new(m);
        r.boot();
        check("rtl", &mut r, answered, resp, Rtl::ns);
    }

    /// **A device write never hits stale in the cache**: `rtl`'s loop reads
    /// B's first word, a hit each time once its line is filled, until the
    /// READ lands there; the next read of it misses, the whole cache having
    /// been invalidated before the response index moved.
    #[test]
    fn a_device_write_invalidates_the_cache() {
        let f = Folder::new("cache");
        f.file("data", b"0123456789");
        let (m, [answered, _]) = machine(&f);
        let mut r = Rtl::new(m);
        r.boot();
        let _ = run_to_response(&mut r, answered, Rtl::ns);
        let misses = r.cache().unwrap().misses;
        assert_eq!(misses, 1, "B's line filled once before the response");
        for _ in 0..200 {
            r.step().unwrap();
        }
        assert_eq!(r.cache().unwrap().misses, 2, "and missed once after it");
        assert_eq!(fixnum(r.machine().amem[0o202]), u32::from_le_bytes(*b"0123"));
    }

    /// **The producer index is taken once the write buffer is empty**: with
    /// main memory taking 25 us a write, the command entry's word 0 is
    /// still going in when the producer write lands, and the command's time
    /// counts from when it is in. (Longer than about 40 us and the loop's
    /// next read of memory waits past `rtl`'s own limit on a stall.)
    #[test]
    fn the_producer_waits_for_the_write_buffer() {
        use muir::cache::MemoryTiming;
        let f = Folder::new("drain");
        f.file("data", b"0123456789");
        for (write_ns, least, most) in
            [(25_000u64, 25_000 + cost(0, 8), 50_000), (290, cost(0, 8), 25_000)]
        {
            let (m, [answered, resp]) = machine(&f);
            let mut r = Rtl::new(m);
            r.set_memory_timing(Some(MemoryTiming { read_ns: 380, write_ns }));
            r.boot();
            let (due, _, _) = run_to_response(&mut r, answered, Rtl::ns);
            assert!((least..most).contains(&due), "{write_ns} ns a write: due {due}");
            assert_eq!(r.machine().main[resp as usize] & 0xffff, TAG.into());
        }
    }
}

/// **A file of 2^32 bytes or more** answers WKF to OPEN, read or append,
/// and DIRECTORY lists it with `<18>` and the length FFFFFFFF. A sparse
/// file, so that nothing is written.
#[test]
fn a_file_too_large_for_32_bits() {
    let f = Folder::new("large");
    let p = f.file("big", b"");
    std::fs::OpenOptions::new().write(true).open(&p).unwrap().set_len(1 << 32).unwrap();
    let mut d = Dev::new(default_root(&f));
    assert_eq!(d.open("/big", READ).status(), status::WKF);
    assert_eq!(d.probe("/big").status(), status::WKF);
    assert_eq!(d.open("/big", WRITE | IF_EXISTS_APPEND).status(), status::WKF);
    let (_, recs) = d.directory("/", 0, 4096);
    assert_eq!((recs[0].too_large, recs[0].len), (true, 0xffff_ffff));
}

/// **The host's refusal is ACC**: a file its owner may not read.
#[test]
fn the_host_s_refusal_is_acc() {
    use std::os::unix::fs::PermissionsExt;
    let f = Folder::new("eacces");
    let p = f.file("locked", b"x");
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&p).is_ok() {
        eprintln!("skipped: this user reads a file of mode 0, as root does");
        return;
    }
    let mut d = Dev::new(default_root(&f));
    assert_eq!(d.open("/locked", READ).status(), status::ACC);
    assert_eq!(d.probe("/locked").status(), 0, "a probe reads no bytes");
}
