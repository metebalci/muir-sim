// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `--checkpoint` and `--resume` under `muir` itself: a run writes its
//! state at its stop, another starts from it, and the refusals.  The boot
//! PROM alone is run, so nothing here needs `vendor/`.

mod support;

use support::{Run, cadr, quux_on_16, scratch, text};

/// `--checkpoint` writes the machine at the stop, `--resume` starts there:
/// the microcycle count carries on, the window counts from the resume.
#[test]
fn a_run_checkpoints_at_its_stop_and_another_resumes_from_it() {
    let dir = scratch("checkpoint");
    let chk = dir.join("a.chk");
    let out = cadr().args(["--micro", "--stop-after", "3000", "--checkpoint"]).arg(&chk).run();
    let t = text(&out);
    assert!(out.status.success(), "the first run failed:\n{t}");
    assert!(
        t.contains(&format!("checkpoint: {} at 3000 microcycles", chk.display())),
        "the checkpoint reported:\n{t}"
    );
    assert!(chk.exists());
    let out = cadr().args(["--micro", "--stop-after", "1000", "--resume"]).arg(&chk).run();
    let t = text(&out);
    assert!(out.status.success(), "the resumed run failed:\n{t}");
    assert!(
        t.contains(&format!("resumed: {} at 3000 microcycles", chk.display())),
        "the resume reported:\n{t}"
    );
    assert!(t.contains("ran out at 1000"), "the window counts from the resume:\n{t}");
    let out = cadr().args(["--rtl", "--stop-after", "1", "--resume"]).arg(&chk).run();
    assert!(!out.status.success());
    assert!(text(&out).contains("a micro checkpoint"), "{}", text(&out));
}

/// A resume builds the machine with as much memory as the checkpoint's
/// had, and `--main-memory-boards` may only agree.
#[test]
fn a_resume_has_the_checkpoint_s_memory() {
    let dir = scratch("checkpoint-memory");
    let chk = dir.join("four.chk");
    let out = cadr()
        .args(["--micro", "--main-memory-boards", "4", "--stop-after", "100", "--checkpoint"])
        .arg(&chk)
        .run();
    assert!(out.status.success(), "{}", text(&out));
    let out = cadr().args(["--micro", "--stop-after", "10", "--resume"]).arg(&chk).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("100 microcycles") && t.contains("4 memory boards"), "{t}");
    let out = cadr()
        .args(["--micro", "--main-memory-boards", "8", "--stop-after", "10", "--resume"])
        .arg(&chk)
        .run();
    assert!(!out.status.success());
    assert!(text(&out).contains("4 memory boards"), "{}", text(&out));
}

/// **A checkpoint is one machine**: the lashup has two and writes none.
/// Run from a scratch directory, where the file named would land were the
/// run to go ahead.
#[test]
fn a_checkpoint_is_one_machine() {
    let dir = scratch("checkpoint-refused");
    let out = cadr()
        .args(["--rtl", "--debug-in-process", "--stop-after", "1", "--resume", "x.chk"])
        .current_dir(&dir)
        .run();
    assert!(!out.status.success());
    assert!(text(&out).contains("not the lashup"), "{}", text(&out));
    assert!(!dir.join("x.chk").exists(), "the run wrote no file");
}

/// **A `chip` checkpoint carries the netlist controller's drives.** They
/// are on the controller's own cable rather than in the machine's model,
/// so before this the pairing was refused outright; now the two runs
/// together are the run that was never stopped, as they are with the model
/// controller.
///
/// The pack is made here by `diskpack`, so nothing in this test needs
/// `vendor/`: a fresh T-300 label on a sparse file, which the boot PROM
/// finds no microload on and does not need to. What matters is that there
/// is a drive on the cable at all --- a spindle turning, an arm, and a
/// controller talking to it --- for the checkpoint to have something to
/// carry.
#[test]
fn chip_checkpoints_the_netlist_controllers_drives() {
    let dir = scratch("checkpoint-drives");
    let pack = dir.join("t300.img");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_diskpack"))
        .arg(&pack)
        .arg("initialize")
        .run();
    assert!(out.status.success(), "the pack was not made:\n{}", text(&out));
    let chk = dir.join("drives.chk");
    let chip = [
        "--chip",
        "--main-memory-boards",
        "4",
        "--disk-controller",
        "netlist",
        "--disk-pack",
        pack.to_str().unwrap(),
    ];
    let out = cadr().args(chip).args(["--stop-after", "300", "--checkpoint"]).arg(&chk).run();
    let t = text(&out);
    assert!(out.status.success(), "the first run failed:\n{t}");
    assert!(chk.exists(), "the checkpoint was written:\n{t}");
    let out = cadr().args(chip).args(["--stop-after", "200", "--resume"]).arg(&chk).run();
    let resumed = text(&out);
    assert!(out.status.success(), "the resumed run failed:\n{resumed}");
    let out = cadr().args(chip).args(["--stop-after", "500"]).run();
    let straight = text(&out);
    assert!(out.status.success(), "the straight run failed:\n{straight}");
    let stop = |t: &str| {
        t.lines()
            .find_map(|l| l.trim().strip_prefix("ran out at ").map(|r| r.to_string()))
            .unwrap_or_else(|| panic!("no stop line in:\n{t}"))
    };
    assert_eq!(
        stop(&resumed).split_once("; ").map(|(_, at)| at),
        stop(&straight).split_once("; ").map(|(_, at)| at),
        "the resumed run is where the straight one is"
    );
}

/// **`chip` checkpoints and resumes like the others**, and the two runs
/// together are the run that was never stopped: the same PC at the same
/// microcycle as a straight run of both windows.  The boot PROM alone, so
/// nothing here needs `vendor/`, and four memory boards rather than
/// thirty-two to keep the boards to build down.  Then the two refusals a
/// checkpoint's own header cannot make: another engine's, and the display
/// board the backplane was built with.
#[test]
fn chip_checkpoints_at_its_stop_and_another_resumes_from_it() {
    let dir = scratch("checkpoint-chip");
    let chk = dir.join("chip.chk");
    let chip = ["--chip", "--main-memory-boards", "4"];
    let out = cadr().args(chip).args(["--stop-after", "300", "--checkpoint"]).arg(&chk).run();
    let t = text(&out);
    assert!(out.status.success(), "the first run failed:\n{t}");
    assert!(
        t.contains(&format!("checkpoint: {} at 300 microcycles", chk.display())),
        "the checkpoint reported at the microcycle it was taken at:\n{t}"
    );
    let out = cadr().args(chip).args(["--stop-after", "200", "--resume"]).arg(&chk).run();
    let resumed = text(&out);
    assert!(out.status.success(), "the resumed run failed:\n{resumed}");
    assert!(
        resumed.contains(&format!("resumed: {} at 300 microcycles", chk.display())),
        "the resume reported:\n{resumed}"
    );
    assert!(resumed.contains("ran out at 200"), "the window counts from the resume:\n{resumed}");
    // The same place a run that was never stopped is at.
    let out = cadr().args(chip).args(["--stop-after", "500"]).run();
    let straight = text(&out);
    assert!(out.status.success(), "the straight run failed:\n{straight}");
    let pc = |t: &str| {
        t.lines()
            .find_map(|l| l.trim().strip_prefix("ran out at ").map(|r| r.to_string()))
            .unwrap_or_else(|| panic!("no stop line in:\n{t}"))
    };
    assert_eq!(
        pc(&resumed).split_once("; ").map(|(_, at)| at),
        pc(&straight).split_once("; ").map(|(_, at)| at),
        "the resumed run is where the straight one is"
    );
    // A checkpoint of one machine does not load onto another engine, and
    // the engine it names is what it says.
    let out = cadr().args(["--rtl", "--stop-after", "1", "--resume"]).arg(&chk).run();
    assert!(!out.status.success());
    assert!(text(&out).contains("a chip checkpoint"), "{}", text(&out));
    let out = cadr()
        .args(chip)
        .args(["--tv-board", "lispm-tv", "--stop-after", "1", "--resume"])
        .arg(&chk)
        .run();
    assert!(!out.status.success());
    assert!(text(&out).contains("--tv-board"), "{}", text(&out));
}

/// **An `rtl` checkpoint carries its timing model**, and a resume under the
/// other is refused by the flag's name: every instant in it is on the time
/// it was run on. Resumed under its own, it runs on.
#[test]
fn a_resume_has_the_checkpoint_s_timing_model() {
    let dir = scratch("checkpoint-timing-model");
    let chk = dir.join("fpga.chk");
    let out = cadr()
        .args(["--rtl", "--timing-model", "fpga", "--stop-after", "100", "--checkpoint"])
        .arg(&chk)
        .run();
    let t = text(&out);
    assert!(out.status.success(), "the first run failed:\n{t}");
    assert!(chk.exists(), "the checkpoint was written:\n{t}");

    let out = cadr()
        .args(["--rtl", "--timing-model", "fpga", "--stop-after", "10", "--resume"])
        .arg(&chk)
        .run();
    let t = text(&out);
    assert!(out.status.success(), "the resumed run failed:\n{t}");

    let out = cadr().args(["--rtl", "--stop-after", "10", "--resume"]).arg(&chk).run();
    let t = text(&out);
    assert_eq!(out.status.code(), Some(2), "not a usage error:\n{t}");
    assert!(t.contains("written under --timing-model fpga, and this run is under cadr"), "{t}");
}

/// **A checkpoint carries its machine, and the executable is the
/// machine**: `cadr` refuses a checkpoint `quux` wrote and `quux` one
/// `cadr` wrote, each naming the executable that resumes it, since the map
/// in it is that machine's. Resumed by its own, it runs on, on both
/// engines. The refusal comes before the engine's: a QUUX checkpoint on
/// `cadr --chip` is QUUX's before it is `rtl`'s, and a `chip` checkpoint is
/// the CADR's, QUUX having no netlist.
#[test]
fn a_resume_has_the_checkpoint_s_machine() {
    let dir = scratch("checkpoint-machine");
    let prom = support::idle_prom_16(&dir);
    let start = || {
        let mut c = quux_on_16();
        c.arg("--prom").arg(&prom);
        c
    };
    let refused = |out: std::process::Output, says: &str| {
        let t = text(&out);
        assert_eq!(out.status.code(), Some(2), "not a usage error:\n{t}");
        assert!(t.contains(says), "the refusal says {says:?}:\n{t}");
    };
    for engine in ["--micro", "--rtl"] {
        let chk = dir.join(format!("quux-{}.chk", &engine[2..]));
        let out = start().args([engine, "--stop-after", "100", "--checkpoint"]).arg(&chk).run();
        assert!(out.status.success(), "{engine}: the first run failed:\n{}", text(&out));
        let out = quux_on_16().args([engine, "--stop-after", "10", "--resume"]).arg(&chk).run();
        assert!(out.status.success(), "{engine}: the resumed run failed:\n{}", text(&out));
        let path = chk.display();
        let says = format!("--resume {path} is quux's, not cadr's: quux --resume {path}");
        for cadr_engine in ["--micro", "--rtl", "--chip"] {
            let out = cadr().args([cadr_engine, "--stop-after", "10", "--resume"]).arg(&chk).run();
            refused(out, &says);
        }

        let chk = dir.join(format!("cadr-{}.chk", &engine[2..]));
        let out = cadr().args([engine, "--stop-after", "100", "--checkpoint"]).arg(&chk).run();
        assert!(out.status.success(), "{engine}: the CADR's run failed:\n{}", text(&out));
        let path = chk.display();
        let says = format!("--resume {path} is cadr's, not quux's: cadr --resume {path}");
        for quux_engine in ["--micro", "--rtl"] {
            let out =
                quux_on_16().args([quux_engine, "--stop-after", "10", "--resume"]).arg(&chk).run();
            refused(out, &says);
        }
    }
    // A `chip` checkpoint is the CADR's boards.
    let chk = dir.join("cadr-chip.chk");
    let models = ["--main-memory", "model", "--io-board", "model", "--tv", "model"];
    let out = cadr()
        .args(["--chip", "--main-memory-boards", "1"])
        .args(models)
        .args(["--stop-after", "20", "--checkpoint"])
        .arg(&chk)
        .run();
    assert!(out.status.success(), "the chip run failed:\n{}", text(&out));
    let path = chk.display();
    let out = quux_on_16().args(["--rtl", "--stop-after", "10", "--resume"]).arg(&chk).run();
    refused(out, &format!("--resume {path} is cadr's, not quux's: cadr --resume {path}"));
}

/// **A checkpoint carries the RTC's setting** (contract Q9): a run under
/// `--rtc <s>` resumes counting from the same start and the same base, and
/// a resume whose `--rtc` says otherwise --- another second, or the host's
/// clock --- is refused by the flag's name, as the machine is. A live
/// checkpoint resumes live.
#[test]
fn a_resume_has_the_checkpoint_s_rtc() {
    let dir = scratch("checkpoint-rtc");
    let prom = support::idle_prom_16(&dir);
    let start = || {
        let mut c = quux_on_16();
        c.arg("--prom").arg(&prom);
        c
    };
    for engine in ["--micro", "--rtl"] {
        let chk = dir.join(format!("{}.chk", &engine[2..]));
        let out = start()
            .args([engine])
            .args(["--rtc", "1700000000", "--stop-after", "100", "--checkpoint"])
            .arg(&chk)
            .run();
        assert!(out.status.success(), "{engine}: the first run failed:\n{}", text(&out));
        let out = quux_on_16()
            .args([engine])
            .args(["--rtc", "1700000000", "--stop-after", "10", "--resume"])
            .arg(&chk)
            .run();
        assert!(out.status.success(), "{engine}: the resumed run failed:\n{}", text(&out));
        for other in [&["--rtc", "1700000001"][..], &["--rtc", "host"], &[]] {
            let out = quux_on_16()
                .args([engine])
                .args(other)
                .args(["--stop-after", "10", "--resume"])
                .arg(&chk)
                .run();
            let t = text(&out);
            assert_eq!(out.status.code(), Some(2), "{engine} {other:?}: not a usage error:\n{t}");
            assert!(
                t.contains("an RTC from 1700000000, and --rtc here is"),
                "{engine} {other:?}: {t}"
            );
        }
        let out =
            start().args([engine]).args(["--stop-after", "100", "--checkpoint"]).arg(&chk).run();
        assert!(out.status.success(), "{engine}: the live run failed:\n{}", text(&out));
        let out =
            quux_on_16().args([engine]).args(["--stop-after", "10", "--resume"]).arg(&chk).run();
        assert!(out.status.success(), "{engine}: live resumes live:\n{}", text(&out));
        let out = quux_on_16()
            .args([engine])
            .args(["--rtc", "5", "--stop-after", "10", "--resume"])
            .arg(&chk)
            .run();
        let t = text(&out);
        assert_eq!(out.status.code(), Some(2), "{engine}: not a usage error:\n{t}");
        assert!(t.contains("the host's clock, and --rtc here is 5"), "{engine}: {t}");
    }
}

/// **An engine's checkpoint carries the display board too**, and a resume
/// onto the other board is refused by the flag's name, as a `chip`
/// checkpoint's header refuses one.  The board is not in the header here
/// --- it is in the body, what `Tv::save` writes --- so the refusal comes
/// after the file has been read rather than before, and says the same
/// thing.
#[test]
fn a_resume_has_the_checkpoint_s_display_board() {
    let dir = scratch("checkpoint-tv-board");
    let chk = dir.join("lispm.chk");
    let out = cadr()
        .args(["--micro", "--tv-board", "lispm-tv", "--stop-after", "100", "--checkpoint"])
        .arg(&chk)
        .run();
    let t = text(&out);
    assert!(out.status.success(), "the first run failed:\n{t}");
    assert!(chk.exists(), "the checkpoint was written:\n{t}");

    // The board it was written with, and the run resumes.
    let out = cadr()
        .args(["--micro", "--tv-board", "lispm-tv", "--stop-after", "10", "--resume"])
        .arg(&chk)
        .run();
    let t = text(&out);
    assert!(out.status.success(), "the resumed run failed:\n{t}");
    assert!(t.contains("100 microcycles"), "{t}");
    assert!(t.contains("tv: model lispm-tv"), "the resumed run has the board:\n{t}");

    // Without the flag it is the SIMPLE TV, which is another machine.
    let out = cadr().args(["--micro", "--stop-after", "10", "--resume"]).arg(&chk).run();
    let t = text(&out);
    assert!(!out.status.success(), "a board apart resumed anyway:\n{t}");
    assert!(t.contains("--tv-board"), "the refusal names the flag:\n{t}");
    assert!(t.contains("lispm-tv"), "and the board the checkpoint has:\n{t}");
}

/// **A checkpoint carries the color TV too**, and a resume onto a machine
/// `--color-tv` disagrees with is refused by the flag's name, as
/// `--tv-board` is.  The second display board is the backplane's: a
/// machine with a color screen is not the machine without one, and the
/// probe `COLOR-EXISTS-P` makes answers differently on each.
#[test]
fn a_resume_has_the_checkpoint_s_color_tv() {
    let dir = scratch("checkpoint-color-tv");
    let chk = dir.join("color.chk");
    let out = cadr()
        .args(["--micro", "--color-tv", "--stop-after", "100", "--checkpoint"])
        .arg(&chk)
        .run();
    let t = text(&out);
    assert!(out.status.success(), "the first run failed:\n{t}");
    assert!(chk.exists(), "the checkpoint was written:\n{t}");

    // The board it was written with, and the run resumes.
    let out =
        cadr().args(["--micro", "--color-tv", "--stop-after", "10", "--resume"]).arg(&chk).run();
    let t = text(&out);
    assert!(out.status.success(), "the resumed run failed:\n{t}");
    assert!(t.contains("100 microcycles"), "{t}");
    assert!(t.contains("color tv: model lispm-tv"), "the resumed run has the board:\n{t}");

    // Without the flag there is one screen, which is another machine.
    let out = cadr().args(["--micro", "--stop-after", "10", "--resume"]).arg(&chk).run();
    let t = text(&out);
    assert!(!out.status.success(), "a machine without the board resumed anyway:\n{t}");
    assert!(t.contains("--color-tv"), "the refusal names the flag:\n{t}");
    assert!(t.contains("with the color tv"), "and says what the checkpoint has:\n{t}");
}

/// **A `chip` checkpoint carries the second display board**, and a resume
/// onto a machine whose `--color-tv` is not the checkpoint's is refused by
/// the flag's name --- none against one, and the netlist board against the
/// model.
///
/// It is refused before anything is read, as `--tv-board` is: the color
/// board is one more device board in the file, and the device boards go in
/// one after another with no count, so a checkpoint with one cannot be
/// read at all by a machine built without it. Hence the word at the front
/// of the file, which [`muir::cable::write_checkpoint`] writes and
/// `read_checkpoint` gives back.
///
/// The boards apart from the two displays are models, and four memory
/// boards rather than thirty-two, so that three runs of this are quick.
#[test]
fn a_chip_resume_has_the_checkpoint_s_color_tv() {
    let dir = scratch("checkpoint-chip-color-tv");
    let chk = dir.join("color.chk");
    let chip = [
        "--chip",
        "--main-memory-boards",
        "4",
        "--main-memory",
        "model",
        "--io-board",
        "model",
        "--disk-controller",
        "model",
    ];
    let out = cadr()
        .args(chip)
        .args(["--color-tv", "--stop-after", "300", "--checkpoint"])
        .arg(&chk)
        .run();
    let t = text(&out);
    assert!(out.status.success(), "the first run failed:\n{t}");
    assert!(t.contains("color TV netlist"), "the board was on the backplane:\n{t}");
    assert!(chk.exists(), "the checkpoint was written:\n{t}");

    // The same machine resumes, and goes on from where it was.
    let out =
        cadr().args(chip).args(["--color-tv", "--stop-after", "50", "--resume"]).arg(&chk).run();
    let t = text(&out);
    assert!(out.status.success(), "the resumed run failed:\n{t}");
    assert!(t.contains("at 300 microcycles"), "the resume reported:\n{t}");
    assert!(t.contains("color TV netlist"), "with the board it was taken with:\n{t}");

    // Without the flag there is one screen, and with `model` the second
    // board is another board: both refused by the flag's name.
    for args in [&["--stop-after", "50"][..], &["--color-tv", "model", "--stop-after", "50"][..]] {
        let out = cadr().args(chip).args(args).args(["--resume"]).arg(&chk).run();
        let t = text(&out);
        assert!(!out.status.success(), "{args:?}: a board apart resumed anyway:\n{t}");
        assert!(t.contains("--color-tv"), "{args:?}: the refusal names the flag:\n{t}");
        assert!(t.contains("netlist"), "{args:?}: and says what the checkpoint has:\n{t}");
    }
}
