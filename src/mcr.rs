// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reading MCR microcode files.
//!
//! The format is the one MIT's microassembler writes and the boot PROM
//! reads, and both are in the System 100 release.  `sys/sys/qwmcr.lisp`
//! writes it: `WRITE-MCR-FILE` puts out the control store as section 1,
//! the dispatch memory as 2, one section 3 for the microcode symbol area and
//! the A memory as 4 (`WRITE-I-MEM I-MEM 1`, `WRITE-D-MEM D-MEM 2`,
//! `WRITE-MICRO-CODE-SYMBOL-AREA-PART-1`, `WRITE-A-MEM A-MEM 4`), each
//! section opening with `OUT32` of the code, a start address and a size.
//! `sys/ucadr/promh.text` reads it, `PROCESS-SECTION`: "Each section starts
//! with three words: The section type, the initial address, and the number
//! of locations ... Section codes are: 1 = I-MEM, 2 = D-MEM, 3 = MAIN-MEM,
//! 4 = A-M-MEM", and its `PROCESS-A-MEM-SECTION` runs into `DONE-LOADING`,
//! so the A-memory section is the last one read.  What follows it in the
//! file is the symbol area, padded to a page boundary
//! (`WRITE-MICRO-CODE-SYMBOL-AREA-PART-2`): [`Mcr::trailing_bytes`].
//!
//! Every word is 32 bits put out by `OUT32` as two 16-bit halves, the high
//! half first --- "Note non-standard order of 16-bit bytes" --- each half a
//! PDP-11 word, low byte first: `Reader::u32_pdp`.
//!
//! The main-memory section's size field is not a count of words in the
//! file; the note under [`parse`] says what it is.
//!
//! **QUUX's `.mcr` is in partition order** (contract Q8): the same sections
//! with the two 16-bit halves of every 32-bit word swapped, so that each
//! word is stored little-endian, as it lies in a microcode partition and as
//! block-disk reads it, and a whole number of 1024-byte blocks, so that
//! `dd` writes the file into a partition with no conversion. muir-sys's
//! writer, `sys/sys/qwmcr.lisp` at its commit `7749360`, makes it for both
//! the microcode and the boot PROM. [`parse_partition_order`] reads it;
//! [`swap_halves`] turns either order into the other. The CADR's `.mcr`
//! stays MIT's.
//!
//! **QUUX revision 13's `.mcr`** (contract G2 appendix A1.4, A1.12) has a
//! dispatch memory section of `10000` entries and A memory as section 5,
//! 40-bit words, each location two 32-bit words: `<31:0>`, then `<39:32>`
//! in `<7:0>`.
//!
//! **QUUX revision 14's `.mcr`** (contract G3 revision 14, appendix A14.13)
//! opens with section 6, microcode and boot PROM alike: code 6, start 0,
//! count 1, then one 32-bit word, the hardware revision, 14.
//! [`Mcr::check_revision`] holds a file to the machine it is loaded on.
//!
//! **QUUX revision 15's `.mcr`** (contract G3 revision 15, appendix
//! A15b.7) is self-describing: little-endian 32-bit words, a file header
//! of two words, the format word [`FORMAT_WORD`] and the number of
//! sections, then each section an 8-word header --- the type, the number of
//! items, the actual width in bits, the storage width in bits (a multiple of
//! 32), the start address, and three parameters, 0 where unused --- and
//! exactly items × storage width bits, an item least significant word first
//! with zeros from its actual width up; then zeros to a whole number of
//! 1,024-byte blocks. The types, in this order, 6 first and 4 last:
//! 6 the hardware revision, 1 the control store at 64 bits, 2 the dispatch
//! memory at 18, 3 the microcode symbol area at 40, 4 A memory at 40.
//! [`parse_15`] reads it and refuses everything A15b.7 lists.

use crate::isa::Insn;

/// The release's own microcode as a file: `sys/ubin/ucadr.mcr`, which is
/// microcode 323, the version this project targets.
///
/// Byte for byte what the release ships, committed in `mit/` beside the boot
/// PROM's own file so that what a pack made here loads is the target's own
/// microcode and not a copy of it from somewhere else.  `tests/mcr.rs` holds
/// the committed copy to the release's, the way `tests/prom.rs` does for the
/// PROM: two builds of a microcode version can both parse and both run, and
/// only the release's bytes settle which one this is.
///
/// It is 12,449 control store words --- `0o30241`, the count `tests/boot.rs`
/// finds in the `MCR1` partition of the pack after the boot PROM has loaded
/// it.
pub const UCADR_323: &[u8] = include_bytes!("../mit/sys/ubin/ucadr.mcr");

#[derive(Clone, Debug, Default)]
pub struct Mcr {
    /// Bytes after the last section header's data.  Not zero in practice: the
    /// A-memory section is the last one and the file runs on past it.
    pub trailing_bytes: usize,
    pub imem_start: u32,
    /// Control store words, in address order from `imem_start`.
    pub imem: Vec<Insn>,
    pub dmem_start: u32,
    /// Dispatch memory: 18 bits used --- parity in 17, R/P/N in 16:14,
    /// address in 13:0 (`ir.bits`).  `WRITE-D-MEM` puts each word out as
    /// two halves, the high one carrying bit 16 in its bit 0 and odd parity
    /// in its bit 1, the low one bits 15-0.
    ///
    /// 2,048 entries, `4000`; QUUX revision 13's 4,096, `10000` (contract G2
    /// appendix A1.4).
    pub dmem: Vec<u32>,
    pub amem_start: u32,
    /// A memory: section 4's 32-bit words, or revision 13's section 5,
    /// 40-bit ones ([`Mcr::amem_wide`]).
    pub amem: Vec<crate::machine::Word>,
    /// Whether A memory came as section 5 (contract G2 appendix A1.12):
    /// each location two 32-bit words, `<31:0>` and then `<39:32>` in
    /// `<7:0>` with `<31:8>` zero.
    pub amem_wide: bool,
    /// The main-memory section, where the file has one: its relative disk
    /// block and its number of blocks ([`parse`] says which field is which).
    pub main_memory: Option<(u32, u32)>,
    /// Section 6's word, the hardware revision the file is for, where the
    /// file has one (contract G3 revision 14, appendix A14.13).
    pub hardware_revision: Option<u32>,
    /// The format number under the format word, for revision 15's format
    /// ([`parse_15`]); `None` for MIT's sections.
    pub format: Option<u32>,
    /// Revision 15's section 3, the microcode symbol area (A15b.7): the
    /// physical address of its first word, and its 40-bit words.
    pub symbol_area: Option<(u32, Vec<crate::machine::Word>)>,
}

/// **Revision 15's format word** (A15b.7): MACHINE-ID's signature `0x5155`
/// in `<31:16>` over the format number 1 in `<15:0>`. Every earlier loader
/// reads it as an unknown section type.
pub const FORMAT_WORD: u32 = 0x5155_0001;

/// Revision 15's `.mcr` is a whole number of these (A15b.7).
pub const BLOCK_BYTES: usize = 1024;

/// What a revision-15 `.mcr` holds, which bounds its control store
/// section (A15b.7): microcode below the PROM's 36000, or the boot PROM's
/// own words, from 36000, at most 2000.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Holds {
    Microcode,
    Prom,
}

/// Revision 15's section types in their order (A15b.7), each with the
/// machine's actual width for it in bits: 6 the hardware revision, 1 the
/// control store, 2 the dispatch memory, 3 the symbol area, 4 A memory.
const SECTIONS_15: [(u32, u32); 5] = [(6, 32), (1, 64), (2, 18), (3, 40), (4, 40)];

/// Whether `bytes` opens with revision 15's format word, little-endian.
pub fn has_format_word(bytes: &[u8]) -> bool {
    bytes.get(..4) == Some(&FORMAT_WORD.to_le_bytes()[..])
}

/// **Reads a revision-15 `.mcr`** (A15b.7), refusing: a length that is
/// not a whole number of 1,024-byte blocks; a format word other than
/// [`FORMAT_WORD`]; a first section other than type 6, or one of another
/// shape than one 32-bit item at 0; an unknown type (5, 0, and every type
/// above 6), a type twice, or the order broken; an actual width other than
/// the machine's for the type, or a storage width not a multiple of 32 or
/// below the actual width; a non-zero unused header word or padding bit; a
/// start plus items past the memory, or for [`Holds::Microcode`] a control
/// store section at or past the PROM's 36000, for [`Holds::Prom`] one that
/// does not start there or runs past its 2000 words; sections that run
/// past the file's end; and a non-zero word after the last section. The
/// hardware revision is read and not judged: [`Mcr::check_revision`] holds
/// it to the machine.
pub fn parse_15(bytes: &[u8], holds: Holds) -> Result<Mcr, String> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(BLOCK_BYTES) {
        return Err(format!(
            "{} bytes, not a whole number of {BLOCK_BYTES}-byte blocks",
            bytes.len()
        ));
    }
    let words: Vec<u32> = bytes.as_chunks::<4>().0.iter().map(|w| u32::from_le_bytes(*w)).collect();
    if words[0] != FORMAT_WORD {
        return Err(format!("format word {:08x}, not {FORMAT_WORD:08x}", words[0]));
    }
    let count = words[1] as usize;
    let mut at = 2usize;
    let mut mcr = Mcr { format: Some(FORMAT_WORD & 0xffff), ..Mcr::default() };
    let mut rank = None;
    for k in 0..count {
        let header = words
            .get(at..at + 8)
            .ok_or_else(|| format!("section {k}'s header runs past the file's end"))?;
        let (kind, items, actual, storage, start) =
            (header[0], header[1], header[2], header[3], header[4]);
        let here = at * 4;
        let Some(r) = SECTIONS_15.iter().position(|&(t, _)| t == kind) else {
            return Err(format!("section {k} at offset {here}: unknown type {kind}"));
        };
        if k == 0 && kind != 6 {
            return Err(format!(
                "the first section is type {kind}: type 6, the hardware revision, is first"
            ));
        }
        if rank.is_some_and(|last| r <= last) {
            return Err(format!(
                "section {k} at offset {here}: type {kind} after type {}, twice or out of the order 6, 1, 2, 3, 4",
                SECTIONS_15[rank.unwrap_or(0)].0
            ));
        }
        rank = Some(r);
        let width = SECTIONS_15[r].1;
        if actual != width {
            return Err(format!(
                "section type {kind}: actual width {actual} bits, and the machine's is {width}"
            ));
        }
        if storage == 0 || !storage.is_multiple_of(32) || storage < actual {
            return Err(format!(
                "section type {kind}: storage width {storage} bits, not a multiple of 32 at least {actual}"
            ));
        }
        if let Some(p) = header[5..8].iter().position(|&w| w != 0) {
            return Err(format!(
                "section type {kind}: header word {} is {:o}, unused and not zero",
                5 + p,
                header[5 + p]
            ));
        }
        let end = u64::from(start) + u64::from(items);
        let past = |size: u64, what: &str| {
            (end > size).then(|| {
                format!(
                    "section type {kind} runs from {start:o} for {items:o} items, past {what}'s {size:o}"
                )
            })
        };
        let prom = crate::machine::QUUX_PROM_BASE as u64;
        let refused = match (kind, holds) {
            (6, _) if start != 0 || items != 1 => Some(format!(
                "section 6 starts at {start:o} for {items:o} items: it is one item, the hardware revision, at 0"
            )),
            (1, Holds::Microcode) => past(prom, "the PROM"),
            (1, Holds::Prom) if u64::from(start) != prom => Some(format!(
                "the control store section starts at {start:o}: the PROM's own file starts at {prom:o}"
            )),
            (1, Holds::Prom) => past(prom + crate::machine::PROM_WORDS as u64, "the PROM"),
            (2, _) => past(crate::machine::DMEM_WORDS as u64, "the dispatch memory"),
            (3, _) => past(1 << 32, "the physical space"),
            (4, _) => past(1024, "A memory"),
            _ => None,
        };
        if let Some(e) = refused {
            return Err(e);
        }
        at += 8;
        let per = (storage / 32) as usize;
        let mut item = |n: usize| -> Result<u64, String> {
            let w = words.get(at..at + per).ok_or_else(|| {
                format!("section type {kind}'s item {n:o} runs past the file's end")
            })?;
            at += per;
            let low =
                w.iter().take(2).enumerate().fold(0u64, |v, (i, &x)| v | u64::from(x) << (32 * i));
            let mask = if actual >= 64 { u64::MAX } else { (1u64 << actual) - 1 };
            if low & !mask != 0 || w.iter().skip(2).any(|&x| x != 0) {
                return Err(format!(
                    "section type {kind}'s item {n:o}: a padding bit above bit {} set",
                    actual - 1
                ));
            }
            Ok(low)
        };
        let n = items as usize;
        match kind {
            6 => mcr.hardware_revision = Some(item(0)? as u32),
            1 => {
                mcr.imem_start = start;
                mcr.imem = (0..n).map(|i| item(i).map(Insn::extended)).collect::<Result<_, _>>()?;
            }
            2 => {
                mcr.dmem_start = start;
                mcr.dmem = (0..n).map(|i| item(i).map(|v| v as u32)).collect::<Result<_, _>>()?;
            }
            3 => mcr.symbol_area = Some((start, (0..n).map(&mut item).collect::<Result<_, _>>()?)),
            _ => {
                mcr.amem_start = start;
                mcr.amem_wide = true;
                mcr.amem = (0..n).map(&mut item).collect::<Result<_, _>>()?;
            }
        }
    }
    if let Some(p) = words[at..].iter().position(|&w| w != 0) {
        return Err(format!(
            "a non-zero word at offset {} after the last of {count} sections",
            (at + p) * 4
        ));
    }
    Ok(mcr)
}

impl Mcr {
    /// The version this microcode says it is: A memory's word 40,
    /// `A-VERSION`, a fixnum whose value is the "VERSION NUMBER FROM SECOND
    /// FILE NAME OF SOURCE" (`mit/sys/ucadr/uc-parameters.lisp`, where
    /// "A-VERSION MUST BE FIRST"); System 100's `sys/cold/qcom.lisp` names
    /// it `%MICROCODE-VERSION-NUMBER`, first of the locations "IN ORDER OF
    /// CONTENTS OF A-MEMORY STARTING AT 40".  The value is the fixnum's
    /// 24-bit pointer field.  `None` if the file's A memory does not reach
    /// word 40.
    pub fn version(&self) -> Option<u32> {
        let i = 0o40usize.checked_sub(self.amem_start as usize)?;
        self.amem.get(i).map(|&w| w as u32 & 0o77777777)
    }

    /// Whether the file is for a machine of `geometry`'s revision, by its
    /// section 6 (contract G3 revision 14, appendix A14.13). A file with
    /// section 6 is revision 14's: below 14, and on the CADR, no boot PROM
    /// reads section 6 (PROM 2001 halts at ERROR-BAD-SECTION-TYPE), and on
    /// revision 14 its word must be 14. A `microcode` file without one is
    /// revision 13's, which revision 14's PROM 2002 refuses; a boot PROM
    /// without one is not refused here.
    pub fn check_revision(
        &self,
        geometry: crate::machine::Geometry,
        microcode: bool,
    ) -> Result<(), String> {
        // Revision 15's format and MIT's sections are each one machine's
        // (A15b.1): the format named with the revision.
        match (self.format, geometry.extended()) {
            (Some(_), false) => {
                return Err(format!(
                    "a revision-15 .mcr, format word {FORMAT_WORD:08x}, for hardware revision {}, and this is {}, which reads MIT's sections",
                    self.hardware_revision.unwrap_or(0),
                    match geometry.revision() {
                        Some(r) => format!("revision {r}"),
                        None => "the CADR".to_string(),
                    }
                ));
            }
            (None, true) => {
                return Err(format!(
                    "MIT's sections, a file for {}, and this is revision 15, which reads the format word {FORMAT_WORD:08x}'s",
                    match self.hardware_revision {
                        Some(r) => format!("hardware revision {r}"),
                        None => "revision 13 or below".to_string(),
                    }
                ));
            }
            _ => {}
        }
        match (self.hardware_revision, geometry.revision()) {
            (Some(r), None) => Err(format!(
                "section 6 says hardware revision {r}, and the CADR reads no section 6"
            )),
            (Some(r), Some(ours)) if ours < 14 => Err(format!(
                "section 6 says hardware revision {r}, and this is revision {ours}, \
                 which reads no section 6"
            )),
            (Some(r), Some(ours)) if r != ours => {
                Err(format!("section 6 says hardware revision {r}, and this is revision {ours}"))
            }
            (None, Some(ours)) if ours >= 14 && microcode => Err(format!(
                "no section 6 first: microcode for revision 13 or below, \
                 and revision {ours} loads microcode whose section 6 says {ours}"
            )),
            _ => Ok(()),
        }
    }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).ok_or("length overflow")?;
        let s = self.b.get(self.at..end).ok_or_else(|| {
            format!("short read of {n} at offset {}, file is {} bytes", self.at, self.b.len())
        })?;
        self.at = end;
        Ok(s)
    }

    /// 32 bits in PDP-11 word order: the two 16-bit halves are swapped, each
    /// stored little-endian.
    fn u32_pdp(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok((b[1] as u32) << 24 | (b[0] as u32) << 16 | (b[3] as u32) << 8 | b[2] as u32)
    }

    fn u16_le(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok((b[1] as u16) << 8 | b[0] as u16)
    }

    /// One control store word: four 16-bit little-endian halves, most
    /// significant first --- `WRITE-I-MEM` puts out `(LDB 6020)`, `4020`,
    /// `2020` and `0020` of each word, "A high", "A low", "M high", "M low".
    /// The top 16 bits are unused; a microinstruction is 48 bits.
    fn insn(&mut self) -> Result<Insn, String> {
        let w1 = self.u16_le()? as u64;
        let w2 = self.u16_le()? as u64;
        let w3 = self.u16_le()? as u64;
        let w4 = self.u16_le()? as u64;
        if w1 != 0 {
            return Err(format!("control store word has bits above 48 set: {w1:#x}"));
        }
        Ok(Insn::new(w2 << 32 | w3 << 16 | w4))
    }
}

/// Parses a whole MCR file.
///
/// The section headers form a sequential stream.  A main-memory section is
/// four words and no data: `WRITE-MICRO-CODE-SYMBOL-AREA-PART-1` puts out
/// the code 3, the number of blocks, the relative disk block and the
/// physical memory address --- the second and third of which arrive here as
/// `start` and `size` --- and `WRITE-MCR-FILE` opens a file built on a base
/// version with the same shape, `3, 0, 0, BASE-VERSION-NUMBER`.  The
/// section's data is on the disk, not in the file, so one more word is
/// consumed and nothing else, which is what the boot PROM's
/// `PROCESS-MAIN-MEM-SECTION` does before it reads the blocks off the pack.
///
/// Sections do not consume the file exactly; [`Mcr::trailing_bytes`] reports
/// what is left.
pub fn parse(bytes: &[u8]) -> Result<Mcr, String> {
    let mut r = Reader { b: bytes, at: 0 };
    let mut mcr = Mcr::default();
    loop {
        let code = r.u32_pdp()?;
        let start = r.u32_pdp()?;
        let size = r.u32_pdp()? as usize;
        match code {
            // The boot PROM's `PROCESS-I-MEM-SECTION` takes `(BYTE-FIELD 18.
            // 14.)` of every address and stops at `ERROR-BAD-ADDRESS` if it
            // is not zero: the control store is 16K words.
            1 => {
                if start as usize + size > 0o40000 {
                    return Err(format!(
                        "the control store section runs from {start:o} for {size:o} words, \
                         past the control store's 40000"
                    ));
                }
                mcr.imem_start = start;
                mcr.imem = (0..size).map(|_| r.insn()).collect::<Result<_, _>>()?;
            }
            // 2,048 entries, and revision 13's 4,096 (A1.4).
            2 => {
                if size != 0o4000 && size != 0o10000 {
                    return Err(format!(
                        "dispatch memory is {size:o} words, expected 4000 or 10000"
                    ));
                }
                mcr.dmem_start = start;
                mcr.dmem = (0..size).map(|_| r.u32_pdp()).collect::<Result<_, _>>()?;
            }
            // Main memory: the physical address word, and `size` --- the
            // relative disk block --- not a count of anything in the file.
            3 => {
                r.u32_pdp()?;
                mcr.main_memory = Some((size as u32, start));
            }
            // `PROCESS-A-MEM-SECTION` checks `(BYTE-FIELD 22. 10.)` of the
            // start alone, and then pushes every word through the PDL buffer
            // from there: a start past A memory's 2000 words stops the PROM,
            // and a long section is the PROM's to load.
            4 => {
                if start >= 0o2000 {
                    return Err(format!("the A memory section starts at {start:o}, past A memory"));
                }
                mcr.amem_start = start;
                mcr.amem =
                    (0..size).map(|_| r.u32_pdp().map(u64::from)).collect::<Result<_, _>>()?;
                break;
            }
            // Revision 13's A memory at 40 bits, the last section as 4 is
            // (A1.12).
            5 => {
                if start >= 0o2000 {
                    return Err(format!("the A memory section starts at {start:o}, past A memory"));
                }
                mcr.amem_start = start;
                mcr.amem_wide = true;
                for k in 0..size {
                    let (low, high) = (r.u32_pdp()?, r.u32_pdp()?);
                    if high >> 8 != 0 {
                        return Err(format!(
                            "A memory {:o}'s high word is {high:o}, more than the tag",
                            start as usize + k
                        ));
                    }
                    mcr.amem.push(u64::from(high) << 32 | u64::from(low));
                }
                break;
            }
            // Revision 14's hardware revision, one word from 0, and the
            // file's first section (A14.13).
            6 => {
                let at = r.at - 12;
                if at != 0 {
                    return Err(format!(
                        "section 6, the hardware revision, at offset {at}: \
                         it is the file's first section"
                    ));
                }
                if start != 0 || size != 1 {
                    return Err(format!(
                        "section 6 starts at {start:o} for {size:o} words: \
                         it is one word, the hardware revision, at 0"
                    ));
                }
                mcr.hardware_revision = Some(r.u32_pdp()?);
            }
            _ => return Err(format!("unknown section code {code:o} at offset {}", r.at - 12)),
        }
    }
    mcr.trailing_bytes = bytes.len() - r.at;
    Ok(mcr)
}

/// A QUUX microcode file, in partition order or revision 15's format
/// ([`parse_quux`]), refused unless it is for a machine of `geometry`'s
/// revision ([`Mcr::check_revision`]).
pub fn parse_quux_microcode(
    bytes: &[u8],
    geometry: crate::machine::Geometry,
) -> Result<Mcr, String> {
    parse_for(bytes, geometry, Holds::Microcode)
}

/// A QUUX `.mcr` of either format, as its first word says: revision 15's
/// ([`parse_15`]) when it opens with the format word, MIT's sections in
/// partition order ([`parse_partition_order`]) otherwise. Which machine it
/// is for is [`Mcr::check_revision`]'s to judge.
pub fn parse_quux(bytes: &[u8], holds: Holds) -> Result<Mcr, String> {
    if has_format_word(bytes) { parse_15(bytes, holds) } else { parse_partition_order(bytes) }
}

/// A QUUX `.mcr` for a machine of `geometry`, in the format its revision
/// reads, with [`Mcr::check_revision`]'s judgement: on revision 15 a file
/// that is not MIT's sections is read as revision 15's, so a damaged format
/// word is refused as that; a file of MIT's sections is refused by its
/// revision, as a revision-15 file is below revision 15.
pub fn parse_for(
    bytes: &[u8],
    geometry: crate::machine::Geometry,
    holds: Holds,
) -> Result<Mcr, String> {
    let mcr = match parse_quux(bytes, holds) {
        Err(e) if geometry.extended() && !has_format_word(bytes) => {
            return Err(parse_15(bytes, holds).err().unwrap_or(e));
        }
        r => r?,
    };
    mcr.check_revision(geometry, holds == Holds::Microcode)?;
    Ok(mcr)
}

/// The file with the two 16-bit halves of every 32-bit word swapped: MIT's
/// order into QUUX's partition order, or back, the swap being its own
/// inverse. A file that is not whole words is refused.
pub fn swap_halves(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if !bytes.len().is_multiple_of(4) {
        return Err(format!("{} bytes, not a whole number of 32-bit words", bytes.len()));
    }
    Ok(bytes.as_chunks::<4>().0.iter().flat_map(|w| [w[2], w[3], w[0], w[1]]).collect())
}

/// Parses an MCR file in QUUX's partition order ([module docs](self)).
///
/// A file that reads in MIT's order and not in partition order is refused
/// saying so: it is the CADR's kind of file, or QUUX's from before Q8.
pub fn parse_partition_order(bytes: &[u8]) -> Result<Mcr, String> {
    let swapped = swap_halves(bytes)?;
    parse(&swapped).map_err(|e| {
        if parse(bytes).is_ok() {
            "the file is in MIT's order; QUUX's .mcr is in partition order, \
             the two 16-bit halves of every word swapped"
                .to_string()
        } else {
            e
        }
    })
}
