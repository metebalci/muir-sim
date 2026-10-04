// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **QUUX's main memory is an amount, `--main-memory-size <n>MW`**: a whole
//! number of megawords with the unit written, and nothing else --- no KW,
//! no fractions, no bare M, which could be read as megabytes. Revision 13
//! takes 1MW to 64MW, 32MW by default; revision 12 takes 1MW to 3MW, 2MW
//! by default. `quux` refuses `--main-memory-boards` and `--main-memory`,
//! naming `--main-memory-size`; the boards and the board's netlist or
//! model are the CADR's, and `cadr` keeps them.
//! Everything that says how much memory a QUUX has says it in MW. The
//! boot PROM alone is run, so nothing here needs `vendor/`.

mod support;

use muir::machine::{Geometry, Machine, bus_error};
use support::{Run, cadr, quux, scratch, text};

const SWITCH: &str = "MUIR_QUUX_REVISION";

/// What a refused form of the amount is told.
const UNIT: &str = "main memory is given in megawords, with the unit MW, such as 32MW";

fn refused(out: &std::process::Output, says: &str) {
    let t = text(out);
    assert_eq!(out.status.code(), Some(2), "not refused at the start:\n{t}");
    assert!(t.contains(says), "the refusal says {says:?}:\n{t}");
}

/// `quux` on revision `rev`.
fn quux_at(rev: &str) -> std::process::Command {
    let mut c = quux();
    c.env(SWITCH, rev);
    c
}

/// The machine a checkpoint holds, loaded as the run left it.
fn checkpointed(path: &std::path::Path) -> Machine {
    let c = muir::checkpoint::read(path).unwrap();
    let g = Machine::checkpointed_geometry_at(&c.body, c.word_bits).unwrap();
    let mut m = Machine::with_geometry(g, c.memory_boards);
    m.block_disk = Some(muir::block_disk::BlockDisk::new(muir::block_disk::BLOCK_NS));
    m.load(&mut c.reader()).unwrap();
    m
}

/// Whether the bus answers a read of `phys` from main memory, or sets
/// NXM: the decode, as the machine's own memory-size probe meets it.
fn answers(m: &mut Machine, phys: u32) -> bool {
    m.bus_error = 0;
    m.bus_read(phys);
    m.bus_error & bus_error::XBUS_NXM == 0
}

/// A run with `--main-memory-size <n>MW`, on both engines, with the machine the
/// checkpoint holds: the start says the amount, the machine has `n` M
/// words, and the bus answers the last of them and not the word after.
fn runs_with(rev: &str, n: usize) {
    let dir = scratch(&format!("main-memory-{rev}-{n}"));
    for engine in ["--micro", "--rtl"] {
        let chk = dir.join(format!("{}.chk", &engine[2..]));
        let amount = format!("{n}MW");
        let out = quux_at(rev)
            .args([engine, "--main-memory-size", &amount, "--stop-after", "10", "--checkpoint"])
            .arg(&chk)
            .run();
        let t = text(&out);
        assert!(out.status.success(), "{rev} {engine} {amount}: {t}");
        assert!(t.contains(&format!("memory: {n}MW\n")), "{rev} {engine}: the start:\n{t}");
        let mut m = checkpointed(&chk);
        assert_eq!(m.main.len(), n << 20, "{rev} {engine}: {n}MW");
        let end = (n << 20) as u32;
        assert!(answers(&mut m, end - 1), "{rev} {engine}: the last word of {amount}");
        assert!(!answers(&mut m, end), "{rev} {engine}: the word past {amount}");
    }
}

/// **1MW, 32MW and 64MW on revision 13** each reach the machine.
#[test]
fn revision_13_runs_with_each_amount() {
    for n in [1, 32, 64] {
        runs_with("13", n);
    }
}

/// **1MW and 3MW on revision 12**, its least and its most.
#[test]
fn revision_12_runs_with_each_amount() {
    for n in [1, 3] {
        runs_with("12", n);
    }
}

/// **The default is 32MW on revision 13 and 2MW on revision 12**, said at
/// the start and built.
#[test]
fn the_default_by_revision() {
    let dir = scratch("main-memory-default");
    for (rev, n) in [("13", 32), ("12", 2)] {
        let chk = dir.join(format!("{rev}.chk"));
        let out =
            quux_at(rev).args(["--micro", "--stop-after", "1", "--checkpoint"]).arg(&chk).run();
        let t = text(&out);
        assert!(out.status.success(), "{rev}: {t}");
        assert!(t.contains(&format!("memory: {n}MW\n")), "{rev}: the start:\n{t}");
        let m = checkpointed(&chk);
        assert_eq!(m.main.len(), n << 20, "{rev}: {n}MW by default");
        assert_eq!(m.geometry.wide(), rev == "13", "{rev}: the revision built");
    }
    // Without the switch it is revision 12.
    let out = quux().args(["--micro", "--stop-after", "1"]).run();
    assert!(text(&out).contains("memory: 2MW\n"), "{}", text(&out));
}

/// **Any other way of writing the amount is refused**, saying how it is
/// written: a bare M, a bare number, the unit in lower case (every word a
/// flag takes is case-sensitive), a fraction, kilowords, megabytes, no
/// number, a sign, a space, and the flag with nothing after it.
#[test]
fn another_form_is_refused() {
    let forms =
        ["32M", "32", "32mw", "32Mw", "1.5MW", "32KW", "32MB", "MW", "", "+32MW", "-1MW", "32 MW"];
    for rev in ["13", "12"] {
        for form in forms {
            let out = quux_at(rev)
                .args(["--micro", "--main-memory-size", form, "--stop-after", "1"])
                .run();
            refused(&out, &format!("--main-memory-size {form}: {UNIT}"));
        }
        let out = quux_at(rev).args(["--micro", "--main-memory-size"]).run();
        refused(&out, &format!("--main-memory-size: {UNIT}"));
    }
}

/// **An amount outside the revision's range is refused, saying the
/// range**: 1MW to 64MW on revision 13, 1MW to 3MW on revision 12.
#[test]
fn an_amount_out_of_range_is_refused() {
    for (rev, form, range) in [
        ("13", "0MW", "1MW to 64MW"),
        ("13", "65MW", "1MW to 64MW"),
        ("13", "1024MW", "1MW to 64MW"),
        ("12", "0MW", "1MW to 3MW"),
        ("12", "4MW", "1MW to 3MW"),
        ("12", "32MW", "1MW to 3MW"),
    ] {
        let out =
            quux_at(rev).args(["--micro", "--main-memory-size", form, "--stop-after", "1"]).run();
        refused(
            &out,
            &format!("--main-memory-size {form}: revision {rev}'s main memory is {range}"),
        );
    }
}

/// **`quux` refuses `--main-memory-boards` and `--main-memory`**, on both
/// revisions, naming `--main-memory-size`; there is no alias.
#[test]
fn quux_refuses_main_memory_boards() {
    for rev in ["13", "12"] {
        for n in ["32", "512"] {
            let out = quux_at(rev)
                .args(["--micro", "--main-memory-boards", n, "--stop-after", "1"])
                .run();
            refused(&out, "--main-memory-boards is cadr's, not quux's");
            refused(&out, "--main-memory-size 32MW");
        }
        // The CADR's `--main-memory`, netlist or model, is no flag of
        // quux's either, with or without an amount after it.
        for word in ["32MW", "model"] {
            let out =
                quux_at(rev).args(["--micro", "--main-memory", word, "--stop-after", "1"]).run();
            refused(&out, "--main-memory is cadr's, not quux's");
            refused(&out, "--main-memory-size 32MW");
        }
    }
    let out = quux().arg("--help").run();
    let t = text(&out);
    assert!(!t.contains("--main-memory-boards"), "{t}");
    assert!(t.contains("--main-memory-size <n>MW"), "{t}");
    assert!(!t.contains("--main-memory "), "{t}");
}

/// **`cadr` keeps `--main-memory-boards` as it was**, and its
/// `--main-memory` is still netlist or model: an amount is no word of it.
#[test]
fn cadr_keeps_main_memory_boards() {
    let out = cadr().args(["--micro", "--main-memory-boards", "4", "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("memory: 4 boards, 256 KW"), "{t}");
    let out = cadr().args(["--micro", "--main-memory-boards", "60", "--stop-after", "1"]).run();
    assert!(text(&out).contains("memory: 60 boards, 3840 KW"), "{}", text(&out));
    let out = cadr().args(["--micro", "--stop-after", "1"]).run();
    assert!(text(&out).contains("memory: 32 boards, 2 MW"), "{}", text(&out));
    let out = cadr().args(["--micro", "--main-memory-boards", "61", "--stop-after", "1"]).run();
    refused(&out, "--main-memory-boards wants a count from 1 to 60");
    let out = cadr().args(["--micro", "--main-memory", "32MW", "--stop-after", "1"]).run();
    refused(&out, "--main-memory wants netlist or model");
    let out = cadr().arg("--help").run();
    assert!(text(&out).contains("--main-memory-boards <n>"), "{}", text(&out));
}

/// **A resume says the checkpoint's memory in MW**, and an amount that
/// does not agree with it is refused in MW.
#[test]
fn a_resume_says_its_memory_in_mw() {
    let dir = scratch("main-memory-resume");
    for engine in ["--micro", "--rtl"] {
        let chk = dir.join(format!("{}.chk", &engine[2..]));
        let out = quux_at("13")
            .args([engine, "--main-memory-size", "4MW", "--stop-after", "100", "--checkpoint"])
            .arg(&chk)
            .run();
        assert!(out.status.success(), "{engine}: {}", text(&out));
        let out = quux_at("13").args([engine, "--stop-after", "10", "--resume"]).arg(&chk).run();
        let t = text(&out);
        assert!(out.status.success(), "{engine}: {t}");
        assert!(t.contains("with 4MW of main memory"), "{engine}: the start:\n{t}");
        assert!(t.contains("100 microcycles") && t.contains(", 4MW of main memory"), "{t}");
        assert!(!t.contains("boards"), "{engine}: no boards on QUUX:\n{t}");
        let out = quux_at("13")
            .args([engine, "--main-memory-size", "8MW", "--stop-after", "10", "--resume"])
            .arg(&chk)
            .run();
        refused(&out, "the checkpoint has 4MW of main memory, --main-memory-size 8MW");
    }
    // The machine's own refusal, loading one checkpoint onto another
    // amount, says it in MW too.
    let c = muir::checkpoint::read(&dir.join("micro.chk")).unwrap();
    let mut m = Machine::with_geometry(Geometry::QUUX_13, 128);
    let err = m.load(&mut c.reader()).unwrap_err().to_string();
    assert!(err.contains("4MW of main memory") && err.contains("this machine has 8MW"), "{err}");
    assert!(!err.contains("boards"), "{err}");
}
