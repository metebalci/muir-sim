// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **`quux` runs one QUUX**, hardware revision 16, and nothing chooses
//! another: `MUIR_QUUX_REVISION`, which earlier commits read, is taken set
//! to 16 and refused set to anything else, the refusal naming the tag of
//! muir-sim's commit that runs the earlier revisions; a checkpoint of
//! revision 13, 14 or 15 is refused naming its revision and the tag; and
//! `cadr` reads neither. The boot PROM alone is run, so nothing here needs
//! `vendor/`.

mod support;

use muir::machine::{Geometry, Machine};
use support::{Run, cadr, quux, text};

const SWITCH: &str = "MUIR_QUUX_REVISION";

/// The tag the refusals name.
const TAG: &str = "muir-sim's tag quux-revisions-13-15";

/// **`--cache` takes no size smaller than a set of the cache's lines**:
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

/// **`MUIR_QUUX_REVISION=16` is taken, and says nothing**: the run's start
/// is the run's with the variable unset, line for line. **Fails** a `quux`
/// that refuses 16, or says something of it.
#[test]
fn muir_quux_revision_16_is_taken() {
    let run = |v: Option<&str>| {
        let mut c = quux();
        if let Some(v) = v {
            c.env(SWITCH, v);
        }
        let out = c.args(["--micro", "--stop-after", "1"]).run();
        let t = text(&out);
        assert!(out.status.success(), "{v:?}: {t}");
        // The run's speed and the terminal's port differ from run to run.
        let same = |l: &&str| !l.starts_with("micro ") && !l.starts_with("terminal:");
        t.lines().filter(same).collect::<Vec<_>>().join("\n")
    };
    assert_eq!(run(Some("16")), run(None));
}

/// **Any other `MUIR_QUUX_REVISION` is refused, naming the tag** that runs
/// the earlier revisions, before anything runs: exit status 2. **Fails** a
/// value taken, or a refusal that does not name the tag.
#[test]
fn any_other_muir_quux_revision_is_refused_naming_the_tag() {
    for v in ["13", "14", "15", "17", "16 ", "x", ""] {
        let out = quux().env(SWITCH, v).args(["--micro", "--stop-after", "1"]).run();
        let t = text(&out);
        assert_eq!(out.status.code(), Some(2), "{v:?}: {t}");
        assert!(t.contains(&format!("{SWITCH}={v:?}")) && t.contains(TAG), "{v:?}: {t}");
        assert!(!t.contains("engine:"), "{v:?}: refused before the run: {t}");
    }
}

/// A checkpoint as QUUX hardware revision `revision` wrote it, 13, 14 or
/// 15: its file's version, 55 for 13 and 14 and 56 for 15, and in its body
/// the level-1 entry's byte the revision wrote, 7 bits on 13, none on 14,
/// and on 15 its revision with `<7>` set (A15b.13).
fn earlier_checkpoint(dir: &support::Scratch, revision: u32) -> std::path::PathBuf {
    let l1_bits = match revision {
        13 => 7,
        14 => 0,
        _ => u32::from(muir::machine::REVISION_15_MARK),
    };
    let mut m = Machine::new();
    m.geometry = Geometry { machine_id: None, l1_bits, ..Geometry::QUUX };
    let mut w = muir::checkpoint::Writer::new();
    m.save(&mut w);
    let version = if revision == 15 { muir::checkpoint::VERSION_15 } else { 55 };
    let path = dir.join(format!("revision-{revision}.chk"));
    muir::checkpoint::write_version(&path, "micro", 32, version, &w.finish()).unwrap();
    path
}

/// **A checkpoint of revision 13, 14 or 15 is refused, naming its revision
/// and the tag that resumes it**, on `quux` and on `cadr`, before anything
/// is built: exit status 2. The control: a checkpoint `quux` wrote resumes.
/// **Fails** a refusal that names another revision, or not the tag, and a
/// checkpoint of an earlier revision read as this one's.
#[test]
fn an_earlier_revision_s_checkpoint_is_refused_naming_it_and_the_tag() {
    let dir = support::scratch("earlier-revisions");
    for revision in [13, 14, 15] {
        let chk = earlier_checkpoint(&dir, revision);
        let body = muir::checkpoint::read(&chk).unwrap();
        let e = Machine::checkpointed_geometry_at(&body.body, body.word_bits).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::Unsupported, "{revision}: {e}");
        for c in [quux(), cadr()] {
            let mut c = c;
            let out = c.args(["--micro", "--stop-after", "1", "--resume"]).arg(&chk).run();
            let t = text(&out);
            assert_eq!(out.status.code(), Some(2), "{revision}: {t}");
            assert!(
                t.contains(&format!(
                    "a checkpoint of QUUX revision {revision}, which this build does not run: it runs the CADR and QUUX; it resumes on {TAG}"
                )),
                "{revision}: {t}"
            );
        }
    }
    let own = dir.join("16.chk");
    let out = quux().args(["--micro", "--stop-after", "10", "--checkpoint"]).arg(&own).run();
    assert!(out.status.success(), "{}", text(&out));
    let out = quux().args(["--micro", "--stop-after", "1", "--resume"]).arg(&own).run();
    assert!(out.status.success(), "its own resumes: {}", text(&out));
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

/// **`cadr` does not read `MUIR_QUUX_REVISION`**: the CADR is the CADR
/// whatever it says.
#[test]
fn cadr_does_not_read_it() {
    for v in ["13", "16"] {
        let out = cadr().env(SWITCH, v).args(["--micro", "--stop-after", "1"]).run();
        let t = text(&out);
        assert!(out.status.success(), "{v}: {t}");
        assert!(t.contains("memory: 32 boards, 2 MW"), "{v}: {t}");
        assert!(!t.contains("revision"), "{v}: {t}");
    }
}

/// **`quux --help` names no revision**: its flags are QUUX's, not a
/// revision's. **Fails** a help entry that says one.
#[test]
fn quux_s_help_names_no_revision() {
    let out = quux().arg("--help").run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(!t.to_lowercase().contains("revision"), "{t}");
}
