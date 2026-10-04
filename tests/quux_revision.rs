// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **Which revision `quux` runs** (contract G2 §8.1): revision 12, the
//! released machine, unless `MUIR_QUUX_REVISION` says 13. The switch is
//! not a documented flag, since it goes when revision 12 is retired. It is
//! read once, where the machine is built, so the start's lines, main
//! memory's default and limits, and a resume's refusal all see the
//! revision the machine is. `cadr` does not read it. The boot PROM alone
//! is run, so nothing here needs `vendor/`.

mod support;

use muir::machine::{Geometry, Machine};
use support::{Run, cadr, quux, scratch, text};

const SWITCH: &str = "MUIR_QUUX_REVISION";

/// The machine a checkpoint was written of, and how many 64K-word boards
/// of main memory it had: read from the file, so it is the machine the
/// run built, not what its start said.
fn checkpointed(path: &std::path::Path) -> (Geometry, u32, usize) {
    let c = muir::checkpoint::read(path).unwrap();
    let g = Machine::checkpointed_geometry_at(&c.body, c.word_bits).unwrap();
    (g, c.word_bits, c.memory_boards)
}

/// The boot PROM a checkpoint carries: the one the run loaded.
fn checkpointed_prom(path: &std::path::Path) -> Vec<muir::isa::Insn> {
    let c = muir::checkpoint::read(path).unwrap();
    let g = Machine::checkpointed_geometry_at(&c.body, c.word_bits).unwrap();
    let mut m = Machine::with_geometry(g, c.memory_boards);
    m.block_disk = Some(muir::block_disk::BlockDisk::new(muir::block_disk::BLOCK_NS));
    m.load(&mut c.reader()).unwrap();
    m.prom.clone()
}

/// **Each revision's `--prom` is held to its own built-in PROM**: PROM
/// 2001's file on revision 13, and PROM 2000's on revision 12, are each
/// said to be QUUX's own word for word, and each on the other revision
/// is not.
#[test]
fn prom_files_are_held_to_their_revision_s_prom() {
    let file = |f: &str| format!("{}/data/{f}", env!("CARGO_MANIFEST_DIR"));
    for (rev, own, other) in [
        ("13", "quux-promh.mcr", "quux-promh-2000.mcr"),
        ("12", "quux-promh-2000.mcr", "quux-promh.mcr"),
    ] {
        for (f, same) in [(own, true), (other, false)] {
            let out = quux()
                .env(SWITCH, rev)
                .args(["--micro", "--prom", &file(f), "--stop-after", "1"])
                .run();
            let t = text(&out);
            assert!(out.status.success(), "{rev} {f}: {t}");
            assert_eq!(t.contains("QUUX's own word for word"), same, "{rev} {f}: {t}");
        }
    }
}

fn refused(out: &std::process::Output, says: &str) {
    let t = text(out);
    assert_eq!(out.status.code(), Some(2), "not refused at the start:\n{t}");
    assert!(t.contains(says), "the refusal says {says:?}:\n{t}");
}

/// **`MUIR_QUUX_REVISION=13` runs revision 13**: a 40-bit word, its
/// MACHINE-ID saying 13, 32MW of main memory (G2 §3), PROM 2001
/// built in, and on `rtl` the memory cache's 8-word lines. The checkpoint is the machine the run
/// built; the start says the same.
#[test]
fn the_switch_runs_revision_13() {
    let dir = scratch("revision-13");
    for engine in ["--micro", "--rtl"] {
        let chk = dir.join(format!("{}.chk", &engine[2..]));
        let out = quux()
            .env(SWITCH, "13")
            .args([engine, "--stop-after", "10", "--checkpoint"])
            .arg(&chk)
            .run();
        let t = text(&out);
        assert!(out.status.success(), "{engine}: the run failed:\n{t}");
        let (g, bits, boards) = checkpointed(&chk);
        assert_eq!(g, Geometry::QUUX_13, "{engine}: the machine built");
        assert_eq!(g.machine_id.map(|id| id >> 4 & 0o7777), Some(13), "{engine}: MACHINE-ID");
        assert_eq!(bits, 40, "{engine}: the word");
        assert_eq!(boards << 16, 32 << 20, "{engine}: 32MW of main memory");
        assert!(t.contains("memory: 32MW\n"), "{engine}: {t}");
        assert!(t.contains("machine: quux, revision 13: "), "{engine}: {t}");
        assert!(!t.contains("machine: quux, revision 12"), "{engine}: {t}");
        assert!(t.contains("QUUX's data/quux-promh.mcr, version 2001"), "{engine}: {t}");
        assert_eq!(checkpointed_prom(&chk), muir::prom::quux_boot_prom(), "{engine}: PROM 2001");
        if engine == "--rtl" {
            assert!(t.contains("cache: 4096 words, lines of 8, 2-way"), "{t}");
        }
    }
    // `--cache`'s sizes keep revision 13's line.
    let out =
        quux().env(SWITCH, "13").args(["--rtl", "--cache", "8192", "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("cache: 8192 words, lines of 8, 2-way"), "{t}");
}

/// **Unset, or 12, is revision 12**, as it was: a 32-bit word, 2MW,
/// PROM 2000 built in, 4-word lines.
#[test]
fn unset_is_revision_12() {
    let dir = scratch("revision-12");
    for set in [None, Some("12")] {
        for engine in ["--micro", "--rtl"] {
            let chk = dir.join(format!("{}-{}.chk", &engine[2..], set.unwrap_or("unset")));
            let mut c = quux();
            if let Some(v) = set {
                c.env(SWITCH, v);
            }
            let out = c.args([engine, "--stop-after", "10", "--checkpoint"]).arg(&chk).run();
            let t = text(&out);
            assert!(out.status.success(), "{set:?} {engine}: the run failed:\n{t}");
            let (g, bits, boards) = checkpointed(&chk);
            assert_eq!(g, Geometry::QUUX, "{set:?} {engine}");
            assert_eq!((bits, boards), (32, 32), "{set:?} {engine}");
            assert!(t.contains("memory: 2MW\n"), "{set:?} {engine}: {t}");
            assert!(t.contains("machine: quux, revision 12: "), "{set:?} {engine}: {t}");
            assert!(
                t.contains("QUUX's data/quux-promh-2000.mcr, version 2000"),
                "{set:?} {engine}: {t}"
            );
            assert_eq!(
                checkpointed_prom(&chk),
                muir::prom::quux_12_boot_prom(),
                "{set:?} {engine}: PROM 2000"
            );
            if engine == "--rtl" {
                assert!(t.contains("cache: 4096 words, lines of 4, 2-way"), "{t}");
            }
        }
    }
}

/// **Any other value is refused at the start**, naming the two.
#[test]
fn another_revision_is_refused() {
    for v in ["14", "11", "", "13 ", "thirteen"] {
        let out = quux().env(SWITCH, v).args(["--micro", "--stop-after", "1"]).run();
        refused(&out, &format!("quux: {SWITCH}={v:?}: revision 12 or 13"));
    }
}

/// **`cadr` does not read it**: the CADR is the CADR whatever it says.
#[test]
fn cadr_does_not_read_it() {
    for v in ["13", "14"] {
        let out = cadr().env(SWITCH, v).args(["--micro", "--stop-after", "1"]).run();
        let t = text(&out);
        assert!(out.status.success(), "{v}: {t}");
        assert!(t.contains("memory: 32 boards, 2 MW"), "{v}: {t}");
        assert!(!t.contains("revision"), "{v}: {t}");
    }
}

/// **Each revision refuses the other's checkpoint**, on both engines,
/// saying which revision wrote it and how to resume it; its own resumes.
#[test]
fn each_revision_refuses_the_other_s_checkpoint() {
    let dir = scratch("revision-checkpoint");
    for engine in ["--micro", "--rtl"] {
        let c12 = dir.join(format!("12-{}.chk", &engine[2..]));
        let c13 = dir.join(format!("13-{}.chk", &engine[2..]));
        let out = quux().args([engine, "--stop-after", "100", "--checkpoint"]).arg(&c12).run();
        assert!(out.status.success(), "{engine}: {}", text(&out));
        let out = quux()
            .env(SWITCH, "13")
            .args([engine, "--stop-after", "100", "--checkpoint"])
            .arg(&c13)
            .run();
        assert!(out.status.success(), "{engine}: {}", text(&out));

        let out = quux()
            .env(SWITCH, "13")
            .args([engine, "--stop-after", "10", "--resume"])
            .arg(&c12)
            .run();
        let p = c12.display();
        refused(
            &out,
            &format!(
                "--resume {p} is revision 12's, and this is revision 13: {SWITCH}=12 quux --resume {p}"
            ),
        );
        let out = quux().args([engine, "--stop-after", "10", "--resume"]).arg(&c13).run();
        let p = c13.display();
        refused(
            &out,
            &format!(
                "--resume {p} is revision 13's, and this is revision 12: {SWITCH}=13 quux --resume {p}"
            ),
        );

        let out = quux().args([engine, "--stop-after", "10", "--resume"]).arg(&c12).run();
        assert!(out.status.success(), "{engine}: 12 resumes 12: {}", text(&out));
        let out = quux()
            .env(SWITCH, "13")
            .args([engine, "--stop-after", "10", "--resume"])
            .arg(&c13)
            .run();
        assert!(out.status.success(), "{engine}: 13 resumes 13: {}", text(&out));
    }
}

/// **`--cache` takes no size smaller than a set of the revision's lines**:
/// on revision 13, whose lines are 8 words, 2-way, 8 words is refused at the
/// start, as a shape of no sets, and 16 is taken; revision 12, whose lines
/// are 4 words, takes 8.
#[test]
fn the_cache_is_at_least_a_set_of_the_revision_s_lines() {
    let out = quux().env(SWITCH, "13").args(["--rtl", "--cache", "8", "--stop-after", "1"]).run();
    refused(&out, "--cache: ");
    refused(&out, "fewer words than one set's lines");
    let out = quux().env(SWITCH, "13").args(["--rtl", "--cache", "16", "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("cache: 16 words, lines of 8, 2-way"), "{t}");
    let out = quux().args(["--rtl", "--cache", "8", "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("cache: 8 words, lines of 4, 2-way"), "{t}");
}
