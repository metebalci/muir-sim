// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A checkpoint: a machine's whole state in a file, for a run to be picked
//! up from where another stopped.
//!
//! The engine and everything it holds --- the processor's memories and
//! registers, main memory, the display, the I/O board, the drives with
//! their positions and every block written to their packs --- go through
//! a [`Writer`] as fixed-width little-endian fields, in the one order the
//! matching [`Reader`] takes them back.  Each type saves and loads its own
//! fields, destructured whole so that a field added later has to be
//! placed.  Not in a checkpoint: the pack's file, which is only ever read
//! and a resume opens again; the Chaosnet's cable and the server on it,
//! plugged in afresh; the terminal, which a viewer reconnects to.
//!
//! The file is a header --- the magic, the format's version, the engine's
//! name, and how many memory boards the machine had, so that a resume can
//! build one the same size before reading the rest --- and the body packed
//! as runs.  Most of a machine's two million
//! words are zero, so the body is written as pairs of counts, zeros then
//! literal bytes, each an LEB128 varint, a run of at least
//! `MIN_ZERO_RUN` zero bytes standing as its count and everything else
//! copied.  An empty memory costs almost nothing; a full one costs itself.

use std::io::{self, Error, ErrorKind};
use std::path::Path;

use crate::clock::Speed;
use crate::part::Level;

const MAGIC: &[u8; 16] = b"muir checkpoint\n";

/// Bumped whenever any type changes what it writes; a file from another
/// version is refused rather than read wrong. A 32-bit machine's, the
/// CADR's. QUUX revision 12, retired, wrote this version too; its
/// checkpoint is refused by the machine its body records
/// ([`crate::machine::Machine::load`]).
pub const VERSION: u32 = 49;

/// A 40-bit machine's, QUUX revision 13's (contract G2 appendix A1.13):
/// every word 5 bytes, `<7:0>` first and the tag last, so that main memory
/// is G1 §4.1's packed storage; the dispatch memory of 4,096 entries, the
/// map's 8,192 and 4,096, the overflow flag, and revision 13's location
/// counter and devices. The version says the width: a resume reads the
/// body at 40 bits.
pub const VERSION_40: u32 = 50;

/// QUUX revision 15's: a 40-bit machine's, as [`VERSION_40`], whose
/// pipeline's state has no pending OA flag, revision 14's two IMOD flags
/// left out (A15b.13; MP4 ruling Q3). A revision-15 file of
/// [`VERSION_40`], which has them, still loads.
pub const VERSION_15: u32 = 51;

/// The version a checkpoint of a machine of `geometry` is written at.
pub fn version_for(geometry: &crate::machine::Geometry) -> u32 {
    match geometry.word_bits {
        _ if geometry.extended() => VERSION_15,
        40 => VERSION_40,
        _ => VERSION,
    }
}

/// The shortest run of zero bytes worth a count of its own.
const MIN_ZERO_RUN: usize = 4;

/// The error a checkpoint that cannot be read gives.
pub fn bad(what: impl std::fmt::Display) -> Error {
    Error::new(ErrorKind::InvalidData, format!("checkpoint: {what}"))
}

/// A word of a 32-bit machine, as a checkpoint has always written it: four
/// bytes. A wider machine's words take [`Writer::set_word_bits`]'s bytes.
const WORD_BYTES_32: usize = 4;

/// How many bytes a word of `bits` takes: 4 for 32 bits, 5 for 40.
fn word_bytes(bits: u32) -> usize {
    bits.div_ceil(8) as usize
}

/// The fields of a checkpoint, in order, little-endian.
pub struct Writer {
    out: Vec<u8>,
    /// The bytes a word ([`crate::machine::Word`]) takes: four on a 32-bit
    /// machine, five on a 40-bit one. The machine sets it before its words
    /// ([`crate::machine::Machine::save`]), and the engine's words after
    /// it take the same.
    word_bytes: usize,
}

impl Default for Writer {
    fn default() -> Writer {
        Writer { out: Vec::new(), word_bytes: WORD_BYTES_32 }
    }
}

impl Writer {
    pub fn new() -> Writer {
        Writer::default()
    }

    /// Words from here on are `bits` wide: 32 or 40.
    pub fn set_word_bits(&mut self, bits: u32) {
        self.word_bytes = word_bytes(bits);
    }

    /// A word ([`crate::machine::Word`]), in as many bytes as the width
    /// takes. Nothing above the width is written: a 32-bit machine's word
    /// is four bytes, as it always was.
    pub fn word(&mut self, v: u64) {
        self.out.extend_from_slice(&v.to_le_bytes()[..self.word_bytes]);
    }

    /// A count, then the words. Main memory is most of a checkpoint, so a
    /// 32-bit machine's words take the four-byte road straight.
    pub fn words(&mut self, v: &[u64]) {
        self.u64(v.len() as u64);
        let n = self.word_bytes;
        self.out.reserve(n * v.len());
        if n == WORD_BYTES_32 {
            for &x in v {
                self.out.extend_from_slice(&(x as u32).to_le_bytes());
            }
        } else {
            for &x in v {
                self.out.extend_from_slice(&x.to_le_bytes()[..n]);
            }
        }
    }

    pub fn u8(&mut self, v: u8) {
        self.out.push(v);
    }

    pub fn u16(&mut self, v: u16) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    pub fn bool(&mut self, v: bool) {
        self.u8(v as u8);
    }

    pub fn speed(&mut self, s: Speed) {
        self.u8(s as u8);
    }

    /// A net's level, as [`crate::chip::Chip`] writes one: the four a wire
    /// can be in, low, high, floating and unknown.
    pub fn level(&mut self, l: Level) {
        self.u8(match l {
            Level::Low => 0,
            Level::High => 1,
            Level::Z => 2,
            Level::X => 3,
        });
    }

    /// A flag, then the value if there is one.
    pub fn opt<T>(&mut self, v: Option<T>, put: impl FnOnce(&mut Writer, T)) {
        self.bool(v.is_some());
        if let Some(x) = v {
            put(self, x);
        }
    }

    /// A count, then the items.
    pub fn bytes(&mut self, v: &[u8]) {
        self.u64(v.len() as u64);
        self.out.extend_from_slice(v);
    }

    pub fn u16s(&mut self, v: &[u16]) {
        self.u64(v.len() as u64);
        for &x in v {
            self.u16(x);
        }
    }

    pub fn u32s(&mut self, v: &[u32]) {
        self.u64(v.len() as u64);
        for &x in v {
            self.u32(x);
        }
    }

    pub fn u64s(&mut self, v: &[u64]) {
        self.u64(v.len() as u64);
        for &x in v {
            self.u64(x);
        }
    }

    /// The width words are written at, in bits of whole bytes.
    pub fn word_bits(&self) -> u32 {
        self.word_bytes as u32 * 8
    }

    pub fn finish(self) -> Vec<u8> {
        self.out
    }
}

/// The netlist boards write themselves as raw bytes through
/// [`std::io::Write`] --- [`crate::chip::Chip::save`] is every net and
/// every cell, and a field at a time would cost more than the board ---
/// and a `chip` checkpoint carries them beside the fielded types.  Their
/// bytes go in where they are written, self-delimiting as every other
/// field is: each carries its own counts.
impl io::Write for Writer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.out.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The fields back, in the order they were written; each read says when
/// the file ends early or holds a value no field can take.
pub struct Reader<'a> {
    data: &'a [u8],
    at: usize,
    /// The bytes a word takes, as [`Writer`] has it. The body does not say:
    /// whoever reads it knows the width, as a resume knows the memory
    /// boards from the header.
    word_bytes: usize,
    /// The file's version, when the body came from a file: what a type
    /// whose fields changed reads an older file by. `None` is this build's
    /// own.
    version: Option<u32>,
}

impl<'a> Reader<'a> {
    /// A body of a 32-bit machine's.
    pub fn new(data: &'a [u8]) -> Reader<'a> {
        Reader { data, at: 0, word_bytes: WORD_BYTES_32, version: None }
    }

    /// A body of a machine whose words are `bits` wide, 32 or 40.
    pub fn for_word_bits(data: &'a [u8], bits: u32) -> Reader<'a> {
        Reader { data, at: 0, word_bytes: word_bytes(bits), version: None }
    }

    /// The file's version the body is of, if it came from a file.
    pub fn version(&self) -> Option<u32> {
        self.version
    }

    /// The width this reader takes words at.
    pub fn word_bits(&self) -> u32 {
        self.word_bytes as u32 * 8
    }

    /// A word, [`Writer::word`].
    pub fn word(&mut self) -> io::Result<u64> {
        let mut b = [0u8; 8];
        b[..self.word_bytes].copy_from_slice(self.take(self.word_bytes)?);
        Ok(u64::from_le_bytes(b))
    }

    /// A count that has to be the slot's own size, then the words into it.
    pub fn words_into(&mut self, into: &mut [u64]) -> io::Result<()> {
        let n = self.count_for(into.len())?;
        let size = self.word_bytes;
        let bytes = self.take(n * size)?;
        if size == WORD_BYTES_32 {
            for (x, b) in into.iter_mut().zip(bytes.chunks_exact(size)) {
                *x = u32::from_le_bytes(b.try_into().unwrap()).into();
            }
        } else {
            for (x, b) in into.iter_mut().zip(bytes.chunks_exact(size)) {
                let mut w = [0u8; 8];
                w[..size].copy_from_slice(b);
                *x = u64::from_le_bytes(w);
            }
        }
        Ok(())
    }

    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.data.len());
        let Some(end) = end else {
            return Err(bad(format!("the file ends early, {n} bytes wanted at {}", self.at)));
        };
        let out = &self.data[self.at..end];
        self.at = end;
        Ok(out)
    }

    pub fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn bool(&mut self) -> io::Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            v => Err(bad(format!("{v} for a flag"))),
        }
    }

    pub fn speed(&mut self) -> io::Result<Speed> {
        Ok(match self.u8()? {
            0 => Speed::ExtraSlow,
            1 => Speed::Slow,
            2 => Speed::Normal,
            3 => Speed::Fast,
            v => return Err(bad(format!("{v} for a speed"))),
        })
    }

    pub fn level(&mut self) -> io::Result<Level> {
        Ok(match self.u8()? {
            0 => Level::Low,
            1 => Level::High,
            2 => Level::Z,
            3 => Level::X,
            v => return Err(bad(format!("{v} for a level"))),
        })
    }

    pub fn opt<T>(
        &mut self,
        get: impl FnOnce(&mut Reader<'a>) -> io::Result<T>,
    ) -> io::Result<Option<T>> {
        if self.bool()? { get(self).map(Some) } else { Ok(None) }
    }

    /// A count that has to be the slot's own size, then the items into it.
    fn count_for(&mut self, slot: usize) -> io::Result<usize> {
        let n = self.u64()? as usize;
        if n != slot {
            return Err(bad(format!("{n} items where {slot} belong")));
        }
        Ok(n)
    }

    pub fn bytes(&mut self) -> io::Result<Vec<u8>> {
        let n = self.u64()? as usize;
        Ok(self.take(n)?.to_vec())
    }

    pub fn bytes_into(&mut self, into: &mut [u8]) -> io::Result<()> {
        let n = self.count_for(into.len())?;
        into.copy_from_slice(self.take(n)?);
        Ok(())
    }

    pub fn u16s(&mut self) -> io::Result<Vec<u16>> {
        let n = self.u64()? as usize;
        (0..n).map(|_| self.u16()).collect()
    }

    pub fn u16s_into(&mut self, into: &mut [u16]) -> io::Result<()> {
        self.count_for(into.len())?;
        for x in into {
            *x = self.u16()?;
        }
        Ok(())
    }

    pub fn u32s(&mut self) -> io::Result<Vec<u32>> {
        let n = self.u64()? as usize;
        (0..n).map(|_| self.u32()).collect()
    }

    pub fn u32s_into(&mut self, into: &mut [u32]) -> io::Result<()> {
        self.count_for(into.len())?;
        for x in into {
            *x = self.u32()?;
        }
        Ok(())
    }

    pub fn u64s(&mut self) -> io::Result<Vec<u64>> {
        let n = self.u64()? as usize;
        (0..n).map(|_| self.u64()).collect()
    }

    pub fn u64s_into(&mut self, into: &mut [u64]) -> io::Result<()> {
        self.count_for(into.len())?;
        for x in into {
            *x = self.u64()?;
        }
        Ok(())
    }

    /// Nothing left: every field has been taken, and no more were written.
    pub fn done(&self) -> io::Result<()> {
        match self.data.len() - self.at {
            0 => Ok(()),
            n => Err(bad(format!("{n} bytes left over"))),
        }
    }
}

/// The other side of [`impl io::Write for Writer`](Writer#impl-Write-for-Writer):
/// the boards read their own bytes back.  A read past the end gives no
/// bytes, so [`std::io::Read::read_exact`] fails with
/// [`std::io::ErrorKind::UnexpectedEof`] --- which is how the far end
/// tells a checkpoint written before a board was stored from one that has
/// it.
impl io::Read for Reader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().min(self.data.len() - self.at);
        buf[..n].copy_from_slice(&self.data[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

// --- The file ---------------------------------------------------------------

/// A checkpoint read back: what its header says, and its body.
#[derive(Debug)]
pub struct Checkpoint {
    /// The format's version: [`VERSION`], [`VERSION_40`] or [`VERSION_15`].
    pub version: u32,
    /// The machine's word, as the version says: 32 or 40 bits.
    pub word_bits: u32,
    /// The engine that wrote it, `micro` or `rtl`.
    pub engine: String,
    /// How many 64K-word memory boards the machine had: on QUUX, which
    /// has no boards, main memory in 64K-word units, sixteen to each MW
    /// `--main-memory-size` gives.
    pub memory_boards: usize,
    pub body: Vec<u8>,
}

impl Checkpoint {
    /// A reader of the body at the machine's width.
    pub fn reader(&self) -> Reader<'_> {
        Reader { version: Some(self.version), ..Reader::for_word_bits(&self.body, self.word_bits) }
    }
}

/// Writes `body`, packed, under the header naming `engine`, the machine's
/// `memory_boards` and, by the version, its `word_bits`, and says how big
/// the file came.
pub fn write(
    path: &Path,
    engine: &str,
    memory_boards: usize,
    word_bits: u32,
    body: &[u8],
) -> io::Result<u64> {
    let version = match word_bits {
        32 => VERSION,
        40 => VERSION_40,
        _ => return Err(bad(format!("{word_bits}-bit words are no machine's"))),
    };
    write_version(path, engine, memory_boards, version, body)
}

/// [`write`] at the format's version `version`, which says the width.
pub fn write_version(
    path: &Path,
    engine: &str,
    memory_boards: usize,
    version: u32,
    body: &[u8],
) -> io::Result<u64> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&version.to_le_bytes());
    out.push(engine.len() as u8);
    out.extend_from_slice(engine.as_bytes());
    out.extend_from_slice(&(memory_boards as u32).to_le_bytes());
    out.extend_from_slice(&pack(body));
    std::fs::write(path, &out)?;
    Ok(out.len() as u64)
}

/// The checkpoint at `path`.
pub fn read(path: &Path) -> io::Result<Checkpoint> {
    let file = std::fs::read(path)?;
    let mut r = Reader::new(&file);
    if r.take(MAGIC.len()).ok() != Some(MAGIC.as_slice()) {
        return Err(bad("not a muir checkpoint"));
    }
    let version = r.u32()?;
    let word_bits = match version {
        VERSION => 32,
        VERSION_40 | VERSION_15 => 40,
        _ => {
            return Err(bad(format!(
                "format version {version}; this build reads {VERSION}, {VERSION_40} and {VERSION_15}"
            )));
        }
    };
    let n = r.u8()? as usize;
    let engine = std::str::from_utf8(r.take(n)?).map_err(|_| bad("the engine's name"))?;
    let memory_boards = r.u32()? as usize;
    let body = unpack(&file[r.at..])?;
    Ok(Checkpoint { version, word_bits, engine: engine.to_string(), memory_boards, body })
}

// --- Packing ----------------------------------------------------------------

fn varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn unvarint(data: &[u8], at: &mut usize) -> io::Result<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let Some(&byte) = data.get(*at) else {
            return Err(bad("a count runs off the end"));
        };
        *at += 1;
        v |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(bad("a count too long"))
}

/// `raw` as pairs of counts, zeros then literals, and the literal bytes.
pub fn pack(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let zeros = raw[i..].iter().take_while(|&&b| b == 0).count();
        i += zeros;
        let start = i;
        while i < raw.len() {
            if raw[i] != 0 {
                i += 1;
                continue;
            }
            let run = raw[i..].iter().take_while(|&&b| b == 0).count();
            if run >= MIN_ZERO_RUN {
                break;
            }
            i += run;
        }
        varint(&mut out, zeros as u64);
        varint(&mut out, (i - start) as u64);
        out.extend_from_slice(&raw[start..i]);
    }
    out
}

/// The most a body can be, which is what a run of zeros is held to before
/// room is made for it: the header does not say how long the body is, and
/// a corrupt count would otherwise ask for exabytes.  Four gigabytes is
/// past the largest machine the format describes --- sixty memory boards
/// are 15 MiB, and eight T300 packs with every block written since they
/// were loaded, the most the drives can carry, a little over 2 GB.
const MAX_BODY: u64 = 1 << 32;

/// The bytes [`pack`] was given.
pub fn unpack(packed: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < packed.len() {
        let zeros = unvarint(packed, &mut at)?;
        let literals = unvarint(packed, &mut at)? as usize;
        let Some(end) = at.checked_add(literals).filter(|&e| e <= packed.len()) else {
            return Err(bad("literals run off the end"));
        };
        if zeros > MAX_BODY.saturating_sub(out.len() as u64) {
            return Err(bad(format!(
                "a run of {zeros} zeros, past the {MAX_BODY} bytes a body can be"
            )));
        }
        out.resize(out.len() + zeros as usize, 0);
        out.extend_from_slice(&packed[at..end]);
        at = end;
    }
    Ok(out)
}
