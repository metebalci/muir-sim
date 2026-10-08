// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A simulator for the MIT CADR Lisp Machine.
//!
//! Targets System 100 (microcode 323), the restored last MIT release, which
//! is the one release muir fetches and tests.
//!
//! The netlists in `data/` are extracted from MIT's own drawings in `mit/`
//! and held to MIT's own wire lists; `data/README.md` and `mit/README.md` say
//! how, and each fact in the code names the drawing page it was read from.

pub mod band;
pub mod benchmark;
pub mod block_disk;
pub mod buses;
pub mod busint;
pub mod cable;
pub mod cache;
pub mod capture;
pub mod chaos;
pub mod checkpoint;
pub mod chip;
pub mod cli;
pub mod clock;
pub mod dcmicro;
pub mod disk_controller;
pub mod disk_image;
pub mod disk_unit;
pub mod diskpack;
pub mod dm;
pub mod engine;
pub mod fabric;
pub mod file_device;
pub mod ioboard;
pub mod isa;
pub mod lashup;
pub mod machine;
pub mod mcr;
pub mod memory_port;
pub mod micro;
pub mod muldiv;
pub mod netlist;
pub mod part;
pub mod pipeline;
pub mod prom;
pub mod prompt;
pub mod quux_input;
pub mod rtl;
pub mod serial;
pub mod spy;
pub mod sym;
pub mod terminal;
pub mod tlb;
pub mod trident;
pub mod ttl;
pub mod tv;
pub mod unibus;
pub mod wirelist;
pub mod xbus;

/// Trims a running record to its newest entries once it holds `cap`
/// entries: the records kept for a test to read back --- packets seen,
/// tags received --- must not grow for the life of an interactive run.
/// Half goes at a time, so the trimming is rare and the newest half is
/// always there.
pub fn keep_recent<T>(record: &mut Vec<T>, cap: usize) {
    if record.len() >= cap {
        record.drain(..cap / 2);
    }
}
