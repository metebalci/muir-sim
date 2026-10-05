// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The boot PROM, and the shape it is burned in.
//!
//! The engines boot MIT's own microcode file, `mit/sys/ubin/promh.mcr`,
//! out of `SYS: UBIN;` --- the release's own `sys/ubin/promh.mcr`, taken
//! from it unchanged, as `mit/README.md` records, and the same file in
//! System 100 and System 304.  [`boot_prom`]
//! is that file's words in the 1K the board decodes as PROM
//! ([`PROM_WORDS`]).  [`parse_mcr`] reads a PROM of one's own out of the
//! same kind of file, which is what `cadr --prom` hands it.
//!
//! What the PROM *chips* hold is not a control store word.  A bank of six
//! 74S472s, 512 by 8 each, holds 48 bits a word and the microinstruction is
//! 48 bits, but they are not the same 48.  Page PROM0 of `data/CADR.netlist`
//! shows it on the top chip of the bank, 1B19, whose eight data pins carry
//! `I40` to `I45`, `I47` and `I48` --- `I46` is on no chip:
//!
//! | bit | holds |
//! |---|---|
//! | 47 | odd parity over bits 46:0 |
//! | 46 | the microinstruction's `IR<47>` |
//! | 45:0 | the microinstruction's `IR<45:0>` |
//!
//! `IR<46>`, the statistics bit, is not stored at all --- and no word of the
//! boot PROM sets it, so nothing is lost.  [`boot_prom_image`] does that
//! conversion, and it is what `Chip::load_prom` takes, because the chips hold
//! what was burned.
//!
//! Not every recovered copy of "version 9" is the same program, so which
//! copy is loaded matters: the engines run the release's own, and
//! `tests/prom.rs` holds `mit/sys/ubin/promh.mcr` to it byte for byte.

use crate::isa::Insn;

/// The PROM holds 1K words, as the board decodes it; version 9 uses the
/// first 0o706 of them.
pub use crate::machine::PROM_WORDS;

/// What one PROM chip holds: 512 words, the 74S472 being the largest part
/// MIT's listings are burned into.
pub const CHIP_WORDS: usize = 512;

/// MIT's own microcode file for the boot PROM, version 9, out of `SYS: UBIN;`.
///
/// Byte for byte what the releases ship as `sys/ubin/promh.mcr` --- System
/// 100 and System 304 carry the same file --- so what the engines boot is
/// the release's own and not a copy of it.
const PROMH_9MCR: &[u8] = include_bytes!("../mit/sys/ubin/promh.mcr");

/// The boot PROM's microinstructions, from MIT's own file, [`PROM_WORDS`]
/// of them.
///
/// Version 9 defines `0o706`; the rest of the PROM is unburned and reads as
/// zero.
pub fn boot_prom() -> Vec<Insn> {
    parse_mcr(PROMH_9MCR).expect("mit/sys/ubin/promh.mcr")
}

/// QUUX's boot PROM, version 2001, at control store 36000 (contracts Q2,
/// G2 §4.4): muir-sys's `promh.text`, block-disk only, saving nothing and
/// finding the microcode through the disk's GPT (contract Q8), in
/// partition order; the register page at `1777777400` and block-disk's
/// registers at its words 200-203 (contract Q13); the GPT and the
/// microcode partition read by 4-byte transfers, 4 blocks a page; the
/// 40-bit A-memory section loaded and a partition without it refused at
/// `ERROR-A-MEM-SECTION-32-BITS`; and its buffer and the microcode's
/// main-memory section on 1024-word pages. `data/README.md` has where it
/// came from; `tests/quux_prom.rs` and `tests/system_2001.rs` hold it.
const QUUX_PROMH: &[u8] = include_bytes!("../data/quux-promh.mcr");

/// QUUX's boot PROM's microinstructions, [`PROM_WORDS`] of them, from
/// 36000 up: PROM 2001.
pub fn quux_boot_prom() -> Vec<Insn> {
    parse_quux_mcr(QUUX_PROMH, crate::machine::Geometry::QUUX).expect("data/quux-promh.mcr")
}

/// A QUUX boot PROM out of an MCR file in QUUX's partition order
/// (contract Q8, [`crate::mcr::parse_partition_order`]): [`PROM_WORDS`]
/// words for control store [`crate::machine::QUUX_PROM_BASE`] up. The
/// assembler writes the control store section from 0, so the words are
/// taken from the PROM's base, and a file with anything assembled below
/// it, or past the top of the control store, is refused; so is a word
/// setting `IR<46>`, as for the CADR's, a file in MIT's order, and a file
/// whose section 6 is not `geometry`'s revision
/// ([`crate::mcr::Mcr::check_revision`]).
pub fn parse_quux_mcr(
    bytes: &[u8],
    geometry: crate::machine::Geometry,
) -> Result<Vec<Insn>, String> {
    let base = crate::machine::QUUX_PROM_BASE as usize;
    let mcr = crate::mcr::parse_partition_order(bytes)
        .map_err(|e| format!("not QUUX's MCR microcode file, as promh.mcr is: {e}"))?;
    mcr.check_revision(geometry, false)?;
    if mcr.imem_start != 0 {
        return Err(format!("the control store section starts at {:o}, not 0", mcr.imem_start));
    }
    if mcr.imem.len() > base + PROM_WORDS {
        return Err(format!(
            "{:o} words: QUUX's PROM ends at {:o}",
            mcr.imem.len(),
            base + PROM_WORDS - 1
        ));
    }
    if let Some(at) = mcr.imem.iter().take(base).position(|w| w.raw() != 0) {
        return Err(format!("a word at {at:o}: QUUX's PROM is assembled at {base:o}"));
    }
    if mcr.imem.len() <= base {
        return Err(format!("nothing at {base:o}: QUUX's PROM starts there"));
    }
    let mut v: Vec<Insn> = mcr.imem[base..].to_vec();
    if let Some(at) = v.iter().position(|w| w.raw() >> 46 & 1 != 0) {
        return Err(format!(
            "word {:o} sets the statistics bit IR<46>, which a burned word has nowhere to hold",
            base + at
        ));
    }
    v.resize(PROM_WORDS, Insn::new(0));
    Ok(v)
}

/// A boot PROM of one's own, out of an MCR microcode file: what `muir
/// --prom` reads, and the same reader [`boot_prom`] comes through, so
/// naming MIT's own file is the default run and not a second path to it.
///
/// The [`PROM_WORDS`] words, padded with the unburned zeros past what the
/// file defines. What is refused is what could not be burned and run:
///
/// - a file with no control store section, which holds no program;
/// - a program longer than the chips, which would be cut off at the end;
/// - one assembled somewhere other than address 0, word 0 being the reset
///   entry point the machine fetches;
/// - a word setting the statistics bit `IR<46>`, which the image has
///   nowhere to hold --- bit 47 is parity and bit 46 is `IR<47>`, see
///   [`programming`]. `chip` runs what the 74S472s hold and would drop
///   it, `micro` and `rtl` fetch the microinstruction and would keep it,
///   and one file running as two programs is no use to engines that are
///   compared against each other.
pub fn parse_mcr(bytes: &[u8]) -> Result<Vec<Insn>, String> {
    // The reader's own complaint is about a section header, which says
    // little to someone who has handed muir the wrong file altogether.
    let mcr = crate::mcr::parse(bytes)
        .map_err(|e| format!("not an MCR microcode file, as promh.mcr is: {e}"))?;
    mcr.check_revision(crate::machine::Geometry::CADR, false)?;
    if mcr.imem.is_empty() {
        return Err("no control store section: the file holds no program".to_string());
    }
    if mcr.imem.len() > PROM_WORDS {
        return Err(format!("{} words: the PROM holds {PROM_WORDS}", mcr.imem.len()));
    }
    if mcr.imem_start != 0 {
        return Err(format!(
            "the control store section starts at {:o}: the boot PROM is fetched from 0",
            mcr.imem_start
        ));
    }
    for (addr, insn) in mcr.imem.iter().enumerate() {
        if insn.raw() >> 46 & 1 != 0 {
            return Err(format!(
                "word {addr:o} sets the statistics bit IR<46>, which a burned word has nowhere to hold"
            ));
        }
    }
    let mut v = mcr.imem;
    v.resize(PROM_WORDS, Insn::new(0));
    Ok(v)
}

/// The same, as the programming image the 74S472s hold.
///
/// This is what `Chip::load_prom` takes, because the chips hold what was
/// burned and not the microinstruction.
pub fn boot_prom_image() -> Vec<u64> {
    boot_prom().into_iter().map(programming).collect()
}

/// The microinstruction a burned word holds: bit 46 back to `IR<47>`, bits
/// 45:0 as they are, and the statistics bit `IR<46>` zero, the image having
/// nowhere to keep it.
pub fn unprogramming(word: u64) -> Insn {
    let low = word & ((1 << 46) - 1);
    let ir47 = (word >> 46) & 1;
    Insn::new(ir47 << 47 | low)
}

/// A microinstruction as it would be burned: `IR<47>` into bit 46, odd
/// parity over bits 46:0 into bit 47, and the statistics bit dropped.  The
/// inverse of [`unprogramming`] for every word the PROM can hold, which is
/// every word without `IR<46>`; `tests/prom.rs` round-trips the boot PROM
/// through both.  For putting a program of one's own on the `chip` board,
/// whose PROM parts take the image and not the instruction.
pub fn programming(insn: Insn) -> u64 {
    let raw = insn.raw();
    let low = (raw & ((1 << 46) - 1)) | ((raw >> 47) & 1) << 46;
    let parity = 1 - (low.count_ones() & 1) as u64;
    parity << 47 | low
}

/// MIT's own PROM listings, as `cadr1/reqtim.prom` and `cadr1/uprior.prom`
/// are written: a tape label or title on the first line, `;` comments, and
/// then one word a line as `address data` in octal, with a comment after,
/// to an `END`. Returns
/// the contents by address, sized to the last one written; a word the file
/// leaves out is zero.
///
/// An address at or past [`CHIP_WORDS`] is refused.  The listings are
/// burned into 74S288s, 74S287s and 74S472s, the largest of them 512
/// words, so no address goes past `0o777`; one that does is a corrupt file
/// and not a bigger part, and would otherwise be made room for.
pub fn parse_mit(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for (lineno, line) in text.lines().enumerate().skip(1) {
        let line = line.split(';').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        // `cadr1/reqtim.prom` ends in `END`; the Chaosnet's `lmmodu.prom`
        // and `lmmynm.prom` in `end`.
        if line.eq_ignore_ascii_case("END") {
            break;
        }
        let mut f = line.split_whitespace();
        let (Some(a), Some(d)) = (f.next(), f.next()) else {
            return Err(format!("line {}: {line:?}", lineno + 1));
        };
        let address =
            usize::from_str_radix(a, 8).map_err(|e| format!("line {}: {e}", lineno + 1))?;
        let data = u32::from_str_radix(d, 8).map_err(|e| format!("line {}: {e}", lineno + 1))?;
        if data > 0xff {
            return Err(format!("line {}: {data:o} is wider than eight bits", lineno + 1));
        }
        if address >= CHIP_WORDS {
            return Err(format!(
                "line {}: address {address:o}, past the {CHIP_WORDS} words of a 74S472",
                lineno + 1
            ));
        }
        if out.len() <= address {
            out.resize(address + 1, 0);
        }
        out[address] = data as u8;
    }
    Ok(out)
}
