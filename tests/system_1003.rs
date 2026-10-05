// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! System 1003, the CADR's current muir-sys release, boots on muir.
//!
//! System 1003 continues System 100, 1001 and 1002 on the CADR's microcode
//! 1001, microcode 1000 with more fixes and the changes for up to 60 memory
//! boards. The pack and the sources are muir-sys's `release-1003`, fetched
//! by `tools/fetch-system-for-cadr.sh`; without them these tests say they
//! were skipped. The band's site puts the machine LISPM-1 at 177201 and its
//! file and time host OZ at 177200 (`site/hosts.text` in the sources), and
//! its `SYS:` translations send `SYS: SITE;` to `/site/` and the rest to
//! `/sys/` on OZ (`site/sys.translations`), so the harness's server is given
//! a root with those two names in it and a writable `lispm` beside them.

use std::path::{Path, PathBuf};

use muir::engine::Engine;
use muir::micro::Micro;
use muir::rtl::Rtl;

mod support;
use support::{boot_to_the_prompt_within, machine_with_pack};

/// LISPM-1 and OZ, as `site/hosts.text` gives them.
const CHAOS_1003: (u16, u16) = (0o177201, 0o177200);

/// A copy of the release's pack, which a run writes to, and a file root
/// serving the release's `sys` and `site`, in a scratch directory; or
/// `None` with the skip line.
fn release_1003(name: &str) -> Option<(support::Scratch, PathBuf, PathBuf)> {
    let pack = support::vendor(&["run", "release-1003-pack.img"])?;
    let sources = support::vendor(&["system-1003"])?;
    let dir = support::scratch(name);
    let copy = dir.join("pack.img");
    std::fs::copy(&pack, &copy).unwrap();
    let root = dir.join("root");
    std::fs::create_dir_all(root.join("lispm")).unwrap();
    std::os::unix::fs::symlink(sources.join("sys"), root.join("sys")).unwrap();
    std::os::unix::fs::symlink(sources.join("site"), root.join("site")).unwrap();
    Some((dir, copy, root))
}

/// The same, with the served `sys/ubin/ucadr.tbl` replaced by `table`: the
/// band asks for its running microcode's error table as `SYS: UBIN; UCADR
/// TBL <version>`, and its translations send that to `/sys/ubin/ucadr.tbl`
/// with no version in the name, so a served tree holds one microcode's
/// table.
fn serving_table(root: &std::path::Path, table: &std::path::Path) {
    let sources = support::vendor(&["system-1003"]).unwrap();
    std::fs::remove_file(root.join("sys")).unwrap();
    std::fs::create_dir_all(root.join("sys/ubin")).unwrap();
    for entry in std::fs::read_dir(sources.join("sys")).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() != "ubin" {
            std::os::unix::fs::symlink(entry.path(), root.join("sys").join(entry.file_name()))
                .unwrap();
        }
    }
    for entry in std::fs::read_dir(sources.join("sys/ubin")).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() != "ucadr.tbl" {
            std::os::unix::fs::symlink(entry.path(), root.join("sys/ubin").join(entry.file_name()))
                .unwrap();
        }
    }
    std::fs::copy(table, root.join("sys/ubin/ucadr.tbl")).unwrap();
}

/// A microcode rebuilt from the release's sources by muir-sys and handed
/// over into the gitignored `ref/`, with its README saying from what; or
/// `None` with the skip line.
fn rebuilt_microcode(dir: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ref").join(dir);
    if p.join("ucadr.mcr").exists() {
        Some(p)
    } else {
        eprintln!("skipped: {} is not present", p.display());
        None
    }
}

/// `%MICROCODE-VERSION-NUMBER`, A memory's word 40 (`mcr::Mcr::version`
/// has where that is from), as the running machine holds it.
fn microcode_version(e: &impl Engine) -> u32 {
    support::low(e.machine().amem[0o40]) & 0o77777777
}

/// **System 1003 reaches its listener on `micro`, on microcode 1001.** It
/// takes longer than the hundred million microcycles of
/// `support::boot_to_the_prompt`: 168.5 million on `micro` and 168 million
/// on `rtl` (measured to the half million).
#[test]
fn system_1003_reaches_the_listener_on_micro() {
    let Some((_dir, pack, root)) = release_1003("system-1003-micro") else { return };
    let mut e = Micro::new(machine_with_pack(&pack));
    e.boot();
    let ran = boot_to_the_prompt_within(&mut e, CHAOS_1003, root, 400_000_000);
    eprintln!("listener after {ran} microcycles");
    assert_eq!(microcode_version(&e), 1001);
}

/// **And on `rtl`.**
#[test]
fn system_1003_reaches_the_listener_on_rtl() {
    let Some((_dir, pack, root)) = release_1003("system-1003-rtl") else { return };
    let mut e = Rtl::new(machine_with_pack(&pack));
    e.boot();
    let ran = boot_to_the_prompt_within(&mut e, CHAOS_1003, root, 400_000_000);
    eprintln!("listener after {ran} microcycles");
    assert_eq!(microcode_version(&e), 1001);
}

/// The release's microcode renumbered `version` and changed in nothing
/// else, in `dir`: its `ucadr.mcr` with A memory's word 40, `A-VERSION`,
/// the fixnum the microcode's version is, rewritten in the file's own word
/// order, and its `ucadr.tbl` with `MICROCODE-ERROR-TABLE-VERSION-NUMBER`
/// rewritten. That is what assembling the same sources under another
/// number gives: System 1003's notes, `docs/release-1003.md` in the
/// release's sources, find microcode 1000 and 1001 assembled from one
/// source to differ in one byte of `ucadr.mcr` and one of `ucadr.tbl`.
fn renumbered_microcode(dir: &Path, version: u32) -> PathBuf {
    let ubin = support::vendor(&["system-1003", "sys", "ubin"]).unwrap();
    let mut mcr = std::fs::read(ubin.join("ucadr.mcr")).unwrap();
    let parsed = muir::mcr::parse(&mcr).unwrap();
    assert_eq!(parsed.version(), Some(1001), "the release's microcode");
    // A memory is the file's last section, 4 bytes a word: word 40 is
    // counted back from where the section ends. A word is its two 16-bit
    // halves, the high half first, each low byte first.
    let end = mcr.len() - parsed.trailing_bytes;
    let at = end - 4 * (parsed.amem.len() - (0o40 - parsed.amem_start as usize));
    let fixnum = (parsed.amem[0o40 - parsed.amem_start as usize] as u32 & !0o77777777) | version;
    mcr[at..at + 4].copy_from_slice(&[
        (fixnum >> 16) as u8,
        (fixnum >> 24) as u8,
        fixnum as u8,
        (fixnum >> 8) as u8,
    ]);
    let renumbered = muir::mcr::parse(&mcr).unwrap();
    assert_eq!(renumbered.version(), Some(version), "renumbered");
    assert_eq!(renumbered.imem, parsed.imem, "the same control store");
    assert_eq!(renumbered.dmem, parsed.dmem, "the same dispatch memory");
    let tbl = std::fs::read_to_string(ubin.join("ucadr.tbl")).unwrap();
    let (was, is) = (
        "(SETQ MICROCODE-ERROR-TABLE-VERSION-NUMBER 1751)",
        format!("(SETQ MICROCODE-ERROR-TABLE-VERSION-NUMBER {version:o})"),
    );
    assert_eq!(tbl.matches(was).count(), 1, "the release's table says 1001, 1751 octal");
    let out = dir.join("ucode");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("ucadr.mcr"), mcr).unwrap();
    std::fs::write(out.join("ucadr.tbl"), tbl.replace(was, &is)).unwrap();
    out
}

/// **System 1003 runs on its microcode renumbered, with no rebuilt band.**
/// The release's microcode 1001 renumbered 1002
/// ([`renumbered_microcode`]), loaded into MCR2 and made current with
/// `diskpack`, which writes the partition's comment as MIT's
/// `LOAD-MCR-FILE` does; the band's error table served for that version.
/// The band reaches its listener on `micro` and `rtl` with 1002 in A
/// memory.
#[test]
fn system_1003_runs_on_its_microcode_renumbered() {
    use muir::diskpack::{Command, Pack};
    let want = 1002;
    for engine in ["micro", "rtl"] {
        let Some((dir, pack, root)) = release_1003(&format!("system-1003-renumbered-{engine}"))
        else {
            return;
        };
        let ucode = renumbered_microcode(dir.path(), want);
        serving_table(&root, &ucode.join("ucadr.tbl"));
        let (mut p, _) = Pack::open(&pack);
        p.run(Command::Load { partition: "MCR2".to_string(), file: Some(ucode.join("ucadr.mcr")) })
            .unwrap();
        p.run(Command::Microload("MCR2".to_string())).unwrap();
        let label = muir::band::Label::open(&pack).unwrap();
        assert_eq!(label.microload_partition, "MCR2");
        assert_eq!(label.partition("MCR2").unwrap().comment, format!("UCADR {want}"));
        let m = machine_with_pack(&pack);
        let ran = match engine {
            "micro" => {
                let mut e = Micro::new(m);
                e.boot();
                let ran = boot_to_the_prompt_within(&mut e, CHAOS_1003, root, 400_000_000);
                assert_eq!(microcode_version(&e), want, "micro");
                ran
            }
            _ => {
                let mut e = Rtl::new(m);
                e.boot();
                let ran = boot_to_the_prompt_within(&mut e, CHAOS_1003, root, 400_000_000);
                assert_eq!(microcode_version(&e), want, "rtl");
                ran
            }
        };
        eprintln!("{engine}: listener on microcode {want} after {ran} microcycles");
    }
}

/// A copy of the release's pack with the microcode in `ucode` loaded into
/// MCR2 and made current, and the served tree holding its error table.
fn with_microcode(pack: &std::path::Path, root: &std::path::Path, ucode: &std::path::Path) {
    use muir::diskpack::{Command, Pack};
    serving_table(root, &ucode.join("ucadr.tbl"));
    let (mut p, _) = Pack::open(pack);
    p.run(Command::Load { partition: "MCR2".to_string(), file: Some(ucode.join("ucadr.mcr")) })
        .unwrap();
    p.run(Command::Microload("MCR2".to_string())).unwrap();
}

/// **QUUX's microcode 1000 stops on a CADR**: the build for revision 4
/// (`ref/ucode-1000-quux4`), from before QUUX's numbers moved to the 2000s,
/// and not the CADR's own microcode 1000. It reads the MACHINE-ID at
/// boot and, without QUUX's signature and a revision of 4 or more, halts at
/// `MACHINE-NOT-QUUX-4`, so that the PC shows 26621. On a CADR booted by
/// MIT's PROM it gets there and stays, on both engines.
#[test]
fn quux_s_microcode_1000_halts_on_a_cadr() {
    let Some(ucode) = rebuilt_microcode("ucode-1000-quux4") else { return };
    // The PROM passes through PC 26621 while it writes the control store
    // there, so it is the PC staying there that is the halt.
    fn stops<E: Engine>(mut e: E, name: &str) {
        e.boot();
        let mut still = 0;
        for _ in 0..50_000_000u64 {
            e.step().unwrap();
            still = if e.pc() == 0o26621 { still + 1 } else { 0 };
            if still == 10_000 {
                return;
            }
        }
        panic!("{name}: never halted at MACHINE-NOT-QUUX-4; PC {:o}", e.pc());
    }
    for engine in ["micro", "rtl"] {
        let Some((_d, pack, root)) = release_1003(&format!("system-1003-quux4-on-cadr-{engine}"))
        else {
            return;
        };
        with_microcode(&pack, &root, &ucode);
        let m = machine_with_pack(&pack);
        match engine {
            "micro" => stops(Micro::new(m), engine),
            _ => stops(Rtl::new(m), engine),
        }
    }
}

/// Types `text` a character at a time, 3 ms of the machine's time after
/// each, as `tests/system_2001.rs` does: typed with no pause, a form of
/// this length reached the listener with two characters swapped.
fn type_slow(e: &mut Micro, k: &mut muir::terminal::keyboard::Keyboard, text: &str) {
    for ch in text.chars() {
        support::type_at(e, k, &ch.to_string());
        let t = e.machine().ns;
        while e.machine().ns < t + 3_000_000 {
            e.step().expect("the machine halted");
        }
    }
}

/// The band's herald as it prints it at `boards` of main memory on `micro`:
/// booted to its listener, logged in, and `si:print-herald` written to
/// `OZ://lispm//herald`, the harness's file server's writable `lispm`. Each
/// line with its runs of spaces made one. `None` with the skip line.
fn herald_at(boards: usize) -> Option<Vec<String>> {
    use muir::terminal::keyboard::Keyboard;
    let (_dir, pack, root) = release_1003(&format!("system-1003-herald-{boards}"))?;
    let mut e = Micro::new(support::machine_with_pack_at(&pack, boards));
    e.boot();
    let ran = boot_to_the_prompt_within(&mut e, CHAOS_1003, root.clone(), 400_000_000);
    eprintln!("{boards} boards: listener after {ran} microcycles");
    let mut k = Keyboard::new();
    type_slow(&mut e, &mut k, "(login 'lispm)\n");
    for _ in 0..30_000_000 {
        e.step().expect("the machine halted");
    }
    type_slow(
        &mut e,
        &mut k,
        "(with-open-file (s \"OZ://lispm//herald\" ':direction ':output) (si:print-herald s))",
    );
    let file = root.join("lispm/herald");
    for _ in 0..400 {
        for _ in 0..1_000_000 {
            e.step().expect("the machine halted");
        }
        if let Ok(t) = std::fs::read_to_string(&file)
            && t.contains("Microcode")
        {
            // A moment more for the file to be closed whole.
            for _ in 0..20_000_000 {
                e.step().expect("the machine halted");
            }
            let t = std::fs::read_to_string(&file).unwrap();
            eprintln!("{boards} boards: its herald:\n{t}");
            return Some(
                t.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect(),
            );
        }
    }
    let png = std::env::temp_dir().join(format!("muir-system-1003-herald-{boards}.png"));
    std::fs::write(&png, e.machine().tv.png()).unwrap();
    panic!(
        "{boards} boards: no herald written to {}; the screen is {}",
        file.display(),
        png.display()
    );
}

/// **System 1003's herald says how to give it all the memory it can use**:
/// with fewer boards than its page tables serve it prints "This system can
/// use up to 3840K of physical memory:" and "use --main-memory-boards 60 in
/// muir-sim or muir-fpga." under the memory line, and at 60 boards, 3840K,
/// it does not (`PRINT-HERALD`, `io/disk.lisp` in the release's sources).
/// At `cadr`'s default 32 boards and at 60, on `micro`; the herald names
/// System 1003 and microcode 1001 at both.
#[test]
fn system_1003_s_herald_says_how_to_reach_60_boards() {
    const HINT: [&str; 2] = [
        "This system can use up to 3840K of physical memory:",
        "use --main-memory-boards 60 in muir-sim or muir-fpga.",
    ];
    for (boards, memory, hinted) in [(32, "2048K", true), (60, "3840K", false)] {
        let Some(herald) = herald_at(boards) else { return };
        let at = herald
            .iter()
            .position(|l| l.contains("physical memory,"))
            .unwrap_or_else(|| panic!("{boards} boards: no memory line in {herald:?}"));
        assert!(
            herald[at].starts_with(&format!("{memory} physical memory")),
            "{boards} boards: {}",
            herald[at]
        );
        if hinted {
            assert_eq!(herald[at + 1..at + 3], HINT, "{boards} boards: the hint");
        } else {
            assert!(
                !herald.iter().any(|l| HINT.contains(&l.as_str())),
                "{boards} boards: {herald:?}"
            );
        }
        assert!(herald.iter().any(|l| l == "Experimental System 1003"), "{boards}: {herald:?}");
        assert!(herald.iter().any(|l| l == "Microcode 1001"), "{boards}: {herald:?}");
    }
}

/// The SHA-256 of `path`, by `sha256sum`.
fn sha256(path: &std::path::Path) -> String {
    let out = std::process::Command::new("sha256sum").arg(path).output().expect("sha256sum");
    assert!(out.status.success(), "sha256sum {}", path.display());
    String::from_utf8(out.stdout).unwrap().split_whitespace().next().unwrap().to_string()
}

/// **A pack opened `ro` is a writable drive to the machine, and its file
/// never changes.** `cadr --disk-pack <pack>,ro` boots the release's pack
/// past MIT's boot PROM --- which writes block 1 on every boot
/// (`SAVE-A-PAGE`, `mit/sys/ucadr/promh.text`) and takes the status word's
/// read-only bit, `STATUS<7>`, for a disk error --- and runs as the same
/// pack opened read-write does, microcycle for microcycle: both end at the
/// same PC. The read-write copy's file has changed, so the run wrote; the
/// read-only copy's has not.
#[test]
fn a_read_only_pack_boots_as_a_writable_one_and_its_file_never_changes() {
    use support::{Run, cadr, text};
    let Some(pack) = support::vendor(&["run", "release-1003-pack.img"]) else { return };
    let dir = support::scratch("system-1003-ro");
    let before = sha256(&pack);
    let mut ends = Vec::new();
    for spelled in ["ro", "rw"] {
        let copy = dir.join(format!("{spelled}.img"));
        std::fs::copy(&pack, &copy).unwrap();
        let out = cadr()
            .args(["--micro", "--disk-pack", &format!("{},{spelled}", copy.display())])
            .args(["--stop-after", "20000000"])
            .run();
        let t = text(&out);
        assert!(out.status.success(), "{spelled}:\n{t}");
        let end = t
            .lines()
            .map(str::trim)
            .find(|l| l.starts_with("ran out at ") || l.starts_with("quit at "))
            .unwrap_or_else(|| panic!("{spelled}: no end said:\n{t}"))
            .to_string();
        assert!(
            end.starts_with("ran out at 20000000; PC ") && !end.contains("in the PROM"),
            "{spelled}: past the PROM and running, not halted:\n{t}"
        );
        let after = sha256(&copy);
        match spelled {
            "ro" => assert_eq!(after, before, "the read-only pack's file changed"),
            _ => assert_ne!(after, before, "the read-write run wrote nothing"),
        }
        std::fs::remove_file(&copy).unwrap();
        ends.push(end);
    }
    assert_eq!(ends[0], ends[1], "ro and rw end at the same place");
}
