// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **`quux` runs revision 13** (contract G2): a 40-bit word, its MACHINE-ID
//! saying 13, 32MW of main memory by default (G2 §3), PROM 2001 built in,
//! and on `rtl` the memory cache's 8-word lines; and a checkpoint of QUUX
//! revision 12, the 32-bit machine before it, is refused by its version,
//! which says so (G2 §11.1). `MUIR_QUUX_REVISION=14` runs revision 14
//! (contract G3 revision 14); the switch is not a documented flag, read
//! once where the machine is built, and `cadr` does not read it. The boot
//! PROM alone is run, so nothing here needs `vendor/`.

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

/// **PROM 2001's file is QUUX's own word for word** as `--prom` says it.
#[test]
fn prom_2001_s_file_is_quux_s_own() {
    let file = format!("{}/data/quux-promh.mcr", env!("CARGO_MANIFEST_DIR"));
    let out = quux().args(["--micro", "--prom", &file, "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("QUUX's own word for word"), "{t}");
}

/// **`quux` runs revision 13**: a 40-bit word, its MACHINE-ID saying 13,
/// 32MW of main memory (G2 §3), PROM 2001 built in, and on `rtl` the
/// memory cache's 8-word lines. The checkpoint is the machine the run
/// built; the start says the same.
#[test]
fn quux_runs_revision_13() {
    let dir = scratch("revision-13");
    for engine in ["--micro", "--rtl"] {
        let chk = dir.join(format!("{}.chk", &engine[2..]));
        let out = quux().args([engine, "--stop-after", "10", "--checkpoint"]).arg(&chk).run();
        let t = text(&out);
        assert!(out.status.success(), "{engine}: the run failed:\n{t}");
        let (g, bits, boards) = checkpointed(&chk);
        assert_eq!(g, Geometry::QUUX, "{engine}: the machine built");
        assert_eq!(g.machine_id.map(|id| id >> 4 & 0o7777), Some(13), "{engine}: MACHINE-ID");
        assert_eq!(bits, 40, "{engine}: the word");
        assert_eq!(boards << 16, 32 << 20, "{engine}: 32MW of main memory");
        assert!(t.contains("memory: 32MW\n"), "{engine}: {t}");
        assert!(t.contains("machine: quux, revision 13: "), "{engine}: {t}");
        assert!(t.contains("QUUX's data/quux-promh.mcr, version 2001"), "{engine}: {t}");
        assert_eq!(checkpointed_prom(&chk), muir::prom::quux_boot_prom(), "{engine}: PROM 2001");
        if engine == "--rtl" {
            assert!(t.contains("cache: 4096 words, lines of 8, 2-way"), "{t}");
        }
        let out = quux().args([engine, "--stop-after", "10", "--resume"]).arg(&chk).run();
        assert!(out.status.success(), "{engine}: its checkpoint resumes: {}", text(&out));
    }
    // `--cache`'s sizes keep the 8-word line.
    let out = quux().args(["--rtl", "--cache", "8192", "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("cache: 8192 words, lines of 8, 2-way"), "{t}");
}

/// **`--cache` takes no size smaller than a set of revision 13's lines**:
/// its lines are 8 words, 2-way, so 8 words is refused at the start, as a
/// shape of no sets, and 16 is taken.
#[test]
fn the_cache_is_at_least_a_set_of_the_revision_s_lines() {
    let out = quux().args(["--rtl", "--cache", "8", "--stop-after", "1"]).run();
    let t = text(&out);
    assert_eq!(out.status.code(), Some(2), "{t}");
    assert!(t.contains("--cache: ") && t.contains("fewer words than one set's lines"), "{t}");
    let out = quux().args(["--rtl", "--cache", "16", "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("cache: 16 words, lines of 8, 2-way"), "{t}");
}

/// **`cadr` says no revision**: the CADR is the CADR.
#[test]
fn cadr_says_no_revision() {
    let out = cadr().args(["--micro", "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("memory: 32 boards, 2 MW"), "{t}");
    assert!(!t.contains("revision"), "{t}");
}

/// QUUX revision 12, the retired 32-bit machine (G2 §11.1), and 11, 12
/// without the fused return: what a checkpoint of either records of its
/// machine, the fields [`Machine::save`] writes of the geometry.
fn retired(revision: u32) -> Geometry {
    Geometry {
        word_bits: 32,
        l1_bits: 6,
        machine_id: Some((0x5155 << 16) | (revision << 4) | 4),
        macro_dispatch: revision >= 12,
        ..Geometry::QUUX
    }
}

/// **A checkpoint of QUUX revision 12 is refused by the machine it
/// records**, on both executables and both engines, naming the revision
/// and where it resumes. It is format version 49, the CADR's, which this
/// build keeps so that the CADR's checkpoints, a board's among them, still
/// resume; the body says which machine wrote it: a 6-bit level-1 map
/// entry, multiply and divide, the tick and the fused return, where the
/// CADR has a 5-bit entry and none of the three. Revision 11, without the
/// fused return, is refused as 11. The body is the machine's alone, as
/// [`Machine::save`] writes it: both engines' bodies begin with it, and
/// nothing past its geometry is read.
#[test]
fn a_revision_12_checkpoint_is_refused_by_its_machine() {
    assert_eq!(muir::checkpoint::VERSION, 49);
    let dir = scratch("revision-12-checkpoint");
    for revision in [12, 11] {
        let mut w = muir::checkpoint::Writer::new();
        Machine::with_geometry(retired(revision), 32).save(&mut w);
        let body = w.finish();
        let said =
            format!("a checkpoint of QUUX revision {revision}, which this build no longer runs");
        let e = Machine::checkpointed_geometry(&body).unwrap_err().to_string();
        assert!(e.contains(&said), "{e}");
        assert!(e.contains("resumes only on an earlier muir-sim"), "{e}");
        let mut quux_13 = Machine::with_geometry(Geometry::QUUX, 512);
        let e = quux_13.load(&mut muir::checkpoint::Reader::new(&body)).unwrap_err().to_string();
        assert!(e.contains(&said), "revision 13 says whose it is, not only its word: {e}");
        for engine in ["micro", "rtl"] {
            let old = dir.join(format!("revision-{revision}-{engine}.chk"));
            muir::checkpoint::write(&old, engine, 32, 32, &body).unwrap();
            for resumer in ["quux", "cadr"] {
                let out = support::executable(resumer)
                    .args([&format!("--{engine}"), "--stop-after", "10", "--resume"])
                    .arg(&old)
                    .run();
                let t = text(&out);
                assert_eq!(out.status.code(), Some(2), "{resumer} {engine}: not refused:\n{t}");
                assert!(t.contains(&said), "{resumer} {engine}: the refusal says whose:\n{t}");
            }
        }
    }
}

fn refused(out: &std::process::Output, says: &str) {
    let t = text(out);
    assert_eq!(out.status.code(), Some(2), "not refused at the start:\n{t}");
    assert!(t.contains(says), "the refusal says {says:?}:\n{t}");
}

/// **`MUIR_QUUX_REVISION=13` is revision 13**, as unset is.
#[test]
fn the_switch_at_13_runs_revision_13() {
    let dir = scratch("revision-13-switch");
    for engine in ["--micro", "--rtl"] {
        let chk = dir.join(format!("{}.chk", &engine[2..]));
        let out = quux()
            .env(SWITCH, "13")
            .args([engine, "--stop-after", "10", "--checkpoint"])
            .arg(&chk)
            .run();
        let t = text(&out);
        assert!(out.status.success(), "{engine}: the run failed:\n{t}");
        assert_eq!(checkpointed(&chk).0, Geometry::QUUX, "{engine}: the machine built");
        assert!(t.contains("machine: quux, revision 13: "), "{engine}: {t}");
    }
}

/// **Any other value is refused at the start**, naming the two: revision 12 is
/// retired with System 2000 and refused as any other value.
#[test]
fn another_revision_is_refused() {
    for v in ["12", "16", "11", "", "13 ", "thirteen"] {
        let out = quux().env(SWITCH, v).args(["--micro", "--stop-after", "1"]).run();
        refused(&out, &format!("quux: {SWITCH}={v:?}: revision 13, 14 or 15"));
    }
}

/// **`MUIR_QUUX_REVISION=14` runs revision 14** (contract G3 revision 14,
/// A14): its MACHINE-ID saying 14, a 40-bit word, 32MW, the start naming
/// it and its TLB, `--tlb` taking another size; `--tlb` below revision 14,
/// and a size that is not a power of two from 1,024 to 32,768, refused.
#[test]
fn the_switch_runs_revision_14() {
    let dir = scratch("revision-14");
    for engine in ["--micro", "--rtl"] {
        let chk = dir.join(format!("{}.chk", &engine[2..]));
        let out = quux()
            .env(SWITCH, "14")
            .args([engine, "--stop-after", "10", "--checkpoint"])
            .arg(&chk)
            .run();
        let t = text(&out);
        assert!(out.status.success(), "{engine}: the run failed:\n{t}");
        let (g, bits, boards) = checkpointed(&chk);
        assert_eq!(g, Geometry::QUUX_14, "{engine}: the machine built");
        assert_eq!(g.revision(), Some(14), "{engine}: MACHINE-ID");
        assert_eq!(bits, 40, "{engine}: the word");
        assert_eq!(boards << 16, 32 << 20, "{engine}: 32MW of main memory");
        assert!(t.contains("machine: quux, revision 14: "), "{engine}: {t}");
        assert!(t.contains("TLB of 4096 entries"), "{engine}: {t}");
        let out =
            quux().env(SWITCH, "14").args([engine, "--tlb", "16384", "--stop-after", "1"]).run();
        let t = text(&out);
        assert!(out.status.success(), "{engine}: {t}");
        assert!(t.contains("TLB of 16384 entries"), "{engine}: {t}");
    }
    let out =
        quux().env(SWITCH, "13").args(["--micro", "--tlb", "4096", "--stop-after", "1"]).run();
    refused(&out, "--tlb is QUUX revision 14's");
    for n in ["512", "65536", "3000", "many"] {
        let out = quux().env(SWITCH, "14").args(["--micro", "--tlb", n, "--stop-after", "1"]).run();
        refused(&out, "--tlb wants its entries, a power of two from 1024 to 32768");
    }
}

/// **Revisions 13 and 14 refuse each other's checkpoints** (A14.13,
/// decisions.md:255), on both engines, by the revision each records; each
/// resumes its own.
#[test]
fn revisions_13_and_14_refuse_each_other_s_checkpoints() {
    let dir = scratch("revision-14-checkpoint");
    for engine in ["--micro", "--rtl"] {
        let chk = |r: &str| dir.join(format!("{r}-{}.chk", &engine[2..]));
        for r in ["13", "14"] {
            let out = quux()
                .env(SWITCH, r)
                .args([engine, "--stop-after", "100", "--checkpoint"])
                .arg(chk(r))
                .run();
            assert!(out.status.success(), "{engine} {r}: {}", text(&out));
        }
        for (ours, theirs) in [("13", "14"), ("14", "13")] {
            let c = chk(theirs);
            let out = quux()
                .env(SWITCH, ours)
                .args([engine, "--stop-after", "10", "--resume"])
                .arg(&c)
                .run();
            let p = c.display();
            refused(
                &out,
                &format!(
                    "--resume {p} is revision {theirs}'s, and this is revision {ours}: {SWITCH}={theirs} quux --resume {p}"
                ),
            );
            let out = quux()
                .env(SWITCH, ours)
                .args([engine, "--stop-after", "10", "--resume"])
                .arg(chk(ours))
                .run();
            assert!(out.status.success(), "{engine}: {ours} resumes {ours}: {}", text(&out));
        }
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
