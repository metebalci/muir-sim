// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The command lines of `cadr` and `quux`: what each refuses, and that it
//! says why rather than starting a machine it cannot build; which flags are
//! whose, the executable being the machine; the boot PROM a flag puts in
//! the machine; and the flags each reads from a file before it.

mod support;

use std::io::Write;

use std::path::{Path, PathBuf};

use support::{Run, Scratch, cadr, cadr_default, executable, quux, scratch, text};

/// A usage error from `exe`, `cadr` or `quux`: exit status 2, and the
/// flag named on stderr.
fn refused(exe: &str, args: &[&str], flag: &str) {
    let out = executable(exe).args(args).run();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{exe} {args:?}: not a usage error:\n{err}");
    // The refusal, not the first line: the version and any file of flags
    // are written before anything is parsed, so they come before it.
    let prefix = format!("{exe}: ");
    assert!(
        err.lines().find(|l| l.starts_with(&prefix)).is_some_and(|l| l.contains(flag)),
        "{exe} {args:?}: the refusal names {flag}:\n{err}"
    );
}

/// A usage error whose message says something in particular, for the
/// refusals that name the engine or the other flag rather than the one
/// that was given.
fn refused_saying(exe: &str, args: &[&str], says: &str) {
    let out = executable(exe).args(args).run();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{exe} {args:?}: not a usage error:\n{err}");
    assert!(err.contains(says), "{exe} {args:?}: the refusal says {says:?}:\n{err}");
}

/// **The memory board count is one to sixty, on every engine.** The
/// Xbus I/O space begins where the sixty-first board would, so sixty is
/// the backplane's most; zero is no memory at all --- the model memory on
/// `chip` is `--main-memory model`. The count sizes main memory on
/// `micro` and `rtl` as it does the backplane on `chip`.
#[test]
fn the_memory_board_count_is_one_to_sixty() {
    for n in ["0", "61", "65"] {
        refused(
            "cadr",
            &["--chip", "--main-memory-boards", n, "--stop-after", "1"],
            "--main-memory-boards",
        );
    }
    for (engine, n) in [("--micro", "4"), ("--rtl", "60"), ("--micro", "1")] {
        let out = cadr().args([engine, "--main-memory-boards", n, "--stop-after", "1"]).run();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{engine} with {n} boards:\n{err}");
    }
}

/// **`chip` means netlist for every board, the disk controller included,
/// and `--main-memory model` takes the disk down with it rather than
/// being refused.**
///
/// The netlist controller is a second master on the Xbus and the model
/// memory answers only the interface's own cycles, so the two cannot be
/// combined --- and that was a refusal while the disk defaulted to its
/// model. With the disk defaulting to its netlist the refusal would fire
/// on a run that asked for nothing of the sort, so it now fires only when
/// the netlist controller was asked for by name. A run that chose the
/// model memory gets the model disk, and the start says so rather than
/// leaving it to be guessed at.
#[test]
fn the_disk_controller_is_netlist_unless_the_memory_is_the_model() {
    let start = |args: &[&str]| {
        let out = cadr().args(args).run();
        let t = text(&out);
        assert!(out.status.success(), "{args:?}:\n{t}");
        t
    };
    // Every board a netlist, with nothing asked for.
    let t = start(&["--chip", "--main-memory-boards", "4", "--stop-after", "1"]);
    assert!(t.contains("disk controller netlist"), "the disk is a netlist too:\n{t}");

    // The model memory takes the disk with it, and is not refused.
    let t = start(&[
        "--chip",
        "--main-memory",
        "model",
        "--main-memory-boards",
        "4",
        "--stop-after",
        "1",
    ]);
    assert!(t.contains("disk controller model"), "the model memory takes the disk:\n{t}");

    // Asked for by name against the model memory, it is refused.
    refused(
        "cadr",
        &["--chip", "--main-memory", "model", "--disk-controller", "netlist", "--stop-after", "1"],
        "--disk-controller netlist",
    );

    // And the model asked for by name is the model, memory or no.
    let t = start(&[
        "--chip",
        "--disk-controller",
        "model",
        "--main-memory-boards",
        "4",
        "--stop-after",
        "1",
    ]);
    assert!(t.contains("disk controller model"), "asked for, and given:\n{t}");
}

/// **`--debug-cable-connect` takes either a debuggee on the network or a
/// debuggee in FPGA fabric, and `0x` is which.** One flag rather than two
/// for one concept --- this machine is the debugger and here is the
/// debuggee --- and the argument is self-describing where it is used: `0x`
/// is unambiguous against a port, a host name and a host with a port.
///
/// The window is `rtl`'s alone, as the endpoint already is: `micro` has no
/// timing model and on `chip` the debug cable is the board's own DBGIN.
/// It is `--debug-cable-connect`'s alone too, since what is to be built in
/// fabric is the debuggee's DBGIN end, so there is no listening at a
/// window. It must be a multiple of four, the window being 32-bit
/// registers, and it has no default: where the window sits is a property
/// of the bitstream.
///
/// **The last case is the platform.** The mapping is `/dev/mem`, which
/// only Linux has; muir is developed on macOS, where the flag is refused
/// by name rather than failing to build. Either way the run stops and the
/// refusal names the flag.
#[test]
fn a_window_address_is_rtls_and_the_debuggers_and_a_multiple_of_four() {
    for engine in ["--micro", "--chip"] {
        refused_saying(
            "cadr",
            &[engine, "--debug-cable-connect", "0x80000080", "--stop-after", "1"],
            if engine == "--micro" { "no timing model" } else { "the board's DBGIN only" },
        );
    }
    refused(
        "cadr",
        &["--rtl", "--debug-cable-listen", "0x80000080", "--stop-after", "1"],
        "--debug-cable-listen",
    );
    refused_saying(
        "cadr",
        &["--rtl", "--debug-cable-listen", "--debug-cable-connect", "0x80000080"],
        "one lashup at a time",
    );
    for bad in ["0x80000082", "0x80000081", "0xnothex", "0x"] {
        refused(
            "cadr",
            &["--rtl", "--debug-cable-connect", bad, "--stop-after", "1"],
            "--debug-cable-connect",
        );
    }
    // An argument that is not 0x is still an endpoint, and a bad one is
    // refused as an endpoint.
    refused(
        "cadr",
        &["--rtl", "--debug-cable-connect", "300000", "--stop-after", "1"],
        "--debug-cable-connect",
    );
    // And a window this build cannot map at all.
    let out =
        cadr().args(["--rtl", "--debug-cable-connect", "0x80000080", "--stop-after", "1"]).run();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "a window muir cannot reach is no run:\n{err}");
    assert!(
        err.lines()
            .find(|l| l.starts_with("cadr: "))
            .is_some_and(|l| l.contains("--debug-cable-connect")),
        "the refusal names the flag:\n{err}"
    );
    if !cfg!(target_os = "linux") {
        assert_eq!(out.status.code(), Some(2), "refused by name, not attempted:\n{err}");
        assert!(err.contains("/dev/mem, which only Linux has"), "and says why:\n{err}");
    }
}

/// **A debuggee's pack needs a debuggee.** Like `--debuggee-terminal`,
/// the flag is the other machine's, and without the lashup there is no
/// other machine to give it to.
#[test]
fn a_debuggee_pack_needs_the_lashup() {
    refused(
        "cadr",
        &["--rtl", "--debuggee-disk-pack", "nothing.img", "--stop-after", "1"],
        "--debuggee-disk-pack",
    );
}

/// **The serial port is one machine's, and the lashup in one process runs
/// two.** The endpoint would be the debugger's alone, and nothing in the
/// loop that steps two machines through the debug cable reaches the other
/// machine's port, so the flag is refused there rather than opened for one
/// of them without saying which.  A machine with its DBGIN listening is
/// one machine, and the port is its.
#[test]
fn the_serial_port_is_not_the_lashups() {
    refused(
        "cadr",
        &["--rtl", "--debug-in-process", "--serial", "0", "--stop-after", "1"],
        "--serial",
    );
    let t = text(
        &cadr()
            .args(["--rtl", "--serial", "0", "--debug-cable-listen", "127.0.0.1:0"])
            .args(["--stop-after", "1"])
            .run(),
    );
    assert!(t.contains("serial: tcp://"), "the serial port beside the connector:\n{t}");
    assert!(t.contains("debug cable: DBGIN listening at 127.0.0.1:"), "and the connector:\n{t}");
}

/// **DBGIN listens by default.** The bus interface's DBGIN is always there
/// --- nothing in the machine enables it, and the microcode neither knows
/// nor can refuse --- so every `rtl` and `chip` run has the connector at
/// 127.0.0.1:7661, or the port above it when that is taken, and says where;
/// with both taken the run says so and goes on without.  Either way the run
/// does not wait for a debugger.  `--no-debug-cable-listen` leaves the
/// connector empty, `--debug-cable-listen` moves it, and of the two the
/// last given wins, so a file of flags can say one and the command line
/// the other.  `micro` has no timing model and no end of the cable, and
/// says so rather than nothing.
#[test]
fn dbgin_listens_by_default() {
    let t = text(&cadr_default().args(["--rtl", "--stop-after", "1"]).run());
    assert!(t.contains("ran out at 1"), "the run did not wait for a debugger:\n{t}");
    let line = t.lines().find(|l| l.starts_with("debug cable: ")).expect("a debug cable line");
    assert!(
        line.starts_with("debug cable: DBGIN listening at 127.0.0.1:766")
            || line.starts_with("debug cable: none --- 7661 and 7662 are both taken"),
        "the default connector, or why not:\n{t}"
    );
    let t =
        text(&cadr_default().args(["--rtl", "--no-debug-cable-listen", "--stop-after", "1"]).run());
    assert!(
        t.contains("debug cable: none --- --no-debug-cable-listen"),
        "left empty, and said:\n{t}"
    );
    let t = text(
        &cadr_default()
            .args(["--rtl", "--no-debug-cable-listen", "--debug-cable-listen", "127.0.0.1:0"])
            .args(["--stop-after", "1"])
            .run(),
    );
    assert!(t.contains("debug cable: DBGIN listening at 127.0.0.1:"), "the last wins:\n{t}");
    let t = text(
        &cadr_default()
            .args(["--rtl", "--debug-cable-listen", "127.0.0.1:0", "--no-debug-cable-listen"])
            .args(["--stop-after", "1"])
            .run(),
    );
    assert!(t.contains("debug cable: none --- --no-debug-cable-listen"), "either way:\n{t}");
    let t = text(&cadr_default().args(["--micro", "--stop-after", "1"]).run());
    assert!(
        t.contains("debug cable: none --- micro has no timing model"),
        "micro says why it has none:\n{t}"
    );
    refused_saying(
        "cadr",
        &["--micro", "--debug-cable-listen", "--stop-after", "1"],
        "no timing model",
    );
}

/// **What wants a machine on its own leaves the connector empty, and says
/// which flag did.** `--checkpoint` writes a machine on its own --- a
/// debugger's cycle in the bus interface is nothing `--resume` can start
/// from --- `--tv-capture` records one clock, and `--watch` on `chip` is
/// the run's own loop, which a debuggee stepped by the debugger's events
/// does not have.  Asked for beside one of them in as many words, the
/// connector is refused as before.
#[test]
fn flags_that_want_a_machine_on_its_own_leave_the_connector_empty() {
    let dir = scratch("cable-alone");
    let chk = dir.join("alone.chk");
    let gif = dir.join("alone.gif");
    for (flag, path) in [("--checkpoint", &chk), ("--tv-capture", &gif)] {
        let path = path.to_str().unwrap();
        let t = text(&cadr_default().args(["--rtl", flag, path, "--stop-after", "1"]).run());
        assert!(t.contains(&format!("debug cable: none --- {flag}")), "{flag}: said:\n{t}");
        refused(
            "cadr",
            &["--rtl", flag, path, "--debug-cable-listen", "127.0.0.1:0", "--stop-after", "1"],
            flag,
        );
    }
    let t =
        text(&cadr_default().args(["--chip", "--watch", "0-1:PC/14", "--stop-after", "1"]).run());
    assert!(t.contains("debug cable: none --- --watch"), "--watch: said:\n{t}");
    refused(
        "cadr",
        &[
            "--chip",
            "--watch",
            "0-1:PC/14",
            "--debug-cable-listen",
            "127.0.0.1:0",
            "--stop-after",
            "1",
        ],
        "--watch",
    );
}

/// **A held start goes with the connector**: `--no-auto-boot` is a machine
/// on its own with its button unpressed, and its DBGIN is there like any
/// machine's --- a debugger may connect to a machine standing at the
/// prompt, which is the two machines powered on together.  The lashup in
/// one process and the debugger's end still refuse it.
#[test]
fn a_held_start_goes_with_the_connector() {
    let out = cadr().args(["--rtl", "--no-auto-boot", "--debug-cable-listen", "127.0.0.1:0"]).run();
    let t = text(&out);
    assert!(out.status.success(), "held, stdin ended, and the run ended:\n{t}");
    assert!(t.contains("start: held"), "held:\n{t}");
    assert!(t.contains("debug cable: DBGIN listening at 127.0.0.1:"), "and listening:\n{t}");
    refused("cadr", &["--rtl", "--debug-in-process", "--no-auto-boot"], "--no-auto-boot");
}

/// **The netlist machine runs with its connector listening**, to its stop,
/// with the prompt: nothing waits for a debugger, and the run is `--chip`
/// alone until one connects.
#[test]
fn the_netlist_machine_runs_with_its_connector_listening() {
    let t = text(
        &cadr().args(["--chip", "--debug-cable-listen", "127.0.0.1:0", "--stop-after", "1"]).run(),
    );
    assert!(t.contains("debug cable: DBGIN listening at 127.0.0.1:"), "listening:\n{t}");
    assert!(t.contains("ran out at 1"), "and ran to its stop:\n{t}");
    assert!(t.contains("^C holds the machine at the prompt"), "with the prompt:\n{t}");
}

/// **`--watch` is refused for the shape of its argument**, and the first
/// line says which flag and what was wrong: no colon between the range
/// and the nets, a range that is no number or ends before it begins, a
/// bus of no bits, and no nets at all.
#[test]
fn a_watch_that_is_not_a_range_and_nets_is_refused() {
    for spec in ["PC/14", "x-3:PC/14", "9-3:PC/14", "3:", "3:PC/0", "3:PC/14,", "-:PC/14"] {
        refused("cadr", &["--chip", "--watch", spec, "--stop-after", "1"], "--watch");
    }
    refused("cadr", &["--chip", "--stop-after", "1", "--watch"], "--watch");
}

/// **A net `--watch` names has to be on a board this run has**, and the
/// refusal says which boards those are, as the prompt's `net` does: a net
/// on no board, a `disk:` net with the model controller in the machine ---
/// which has no board and so no nets --- and a bus missing its top bit.
/// This is not the command line's shape, so it is the one-line refusal
/// and exit status 1, not the usage.
#[test]
fn a_watch_on_a_net_the_machine_has_not_got_is_refused() {
    for (spec, says) in [
        ("0-1:NOSUCH", "no net NOSUCH on cpu, busint, memory, tv, disk, io"),
        // A board this run has not got: the disk controller as its model
        // has no nets, so `chip` with that has no `disk` to name.
        ("0-1:cpu:PC/15", "cpu has no PC14 --- a bus is PC0 up"),
        ("0-1:disk:NOSUCH", "no net NOSUCH on disk"),
    ] {
        let out = cadr().args(["--chip", "--watch", spec, "--stop-after", "1"]).run();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            out.status.code(),
            Some(1),
            "{spec}: not refused as a run that cannot start:\n{err}"
        );
        assert!(err.contains(says), "{spec}: the refusal says {says:?}:\n{err}");
        assert!(!err.contains("usage:"), "{spec}: and the usage is no help here:\n{err}");
    }

    // `disk` is a board this run has because the controller is a netlist.
    // Asked for against its model there is no such board, and the refusal
    // names the boards there are.
    let out = cadr()
        .args(["--chip", "--disk-controller", "model", "--watch", "0-1:disk:NEW CCW"])
        .args(["--stop-after", "1"])
        .run();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("no board called disk here; this run has cpu, busint, memory, tv, io"),
        "the model controller is no board:\n{err}"
    );
}

/// **On the other two engines `--watch` is ignored, and the start says
/// so**, as it says of every flag that is chip's alone: they have
/// registers and memories and no nets to record.
#[test]
fn a_watch_on_the_other_engines_is_said_to_be_ignored() {
    for engine in ["--micro", "--rtl"] {
        let out = cadr().args([engine, "--watch", "0-1:PC/14", "--stop-after", "1"]).run();
        let t = text(&out);
        assert!(out.status.success(), "{engine}: not refused:\n{t}");
        assert!(t.contains("warning: --watch is chip, and this run is"), "{engine}: said:\n{t}");
        assert!(!t.contains("watch: "), "{engine}: and nothing was recorded:\n{t}");
    }
}

/// **`--tv-board` names the display board on every engine**, not `chip`
/// alone: the same model answers for either board on `micro` and `rtl`
/// and under `--tv model`, so the flag is obeyed rather than warned
/// about, and the start says which board the run has.
#[test]
fn the_display_board_is_named_on_every_engine() {
    for engine in ["--micro", "--rtl"] {
        let out = cadr().args([engine, "--tv-board", "lispm-tv", "--stop-after", "1"]).run();
        let t = text(&out);
        assert!(out.status.success(), "{engine}:\n{t}");
        assert!(t.contains("tv: model lispm-tv"), "{engine}: the start says the board:\n{t}");
        assert!(!t.contains("warning: --tv-board"), "{engine}: and does not ignore it:\n{t}");
    }
    // The board a run has with nothing said is the SIMPLE TV, System 100's
    // own.
    let out = cadr().args(["--rtl", "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("tv: model simple-tv"), "the default board:\n{t}");

    // `chip` says it in its boards line, where it always has, and the
    // model on `chip` takes the flag as the netlist does.
    let out = cadr()
        .args(["--chip", "--main-memory-boards", "4", "--tv", "model"])
        .args(["--tv-board", "lispm-tv", "--stop-after", "1"])
        .run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("TV model lispm-tv"), "chip with the model board:\n{t}");
}

/// **The video controller is QUUX's, and QUUX's only**: it is QUUX's
/// display, always, and the start says so; `--tv-board` is the CADR's
/// choice between its two boards and not a flag of `quux`, and `video` is
/// refused as a word of it on `cadr`. Its size is `--video-size`; the
/// old `--mono-tv-size` is refused by both executables with a message
/// naming the new flag (contract Q13).
#[test]
fn the_video_controller_is_quux_s() {
    refused_saying("cadr", &["--rtl", "--tv-board", "video"], "video is quux's");
    for engine in ["--micro", "--rtl"] {
        let out = quux().args([engine, "--stop-after", "1"]).run();
        let t = text(&out);
        assert!(out.status.success(), "{engine}:\n{t}");
        assert!(t.contains("tv: model video"), "{engine}: the start says the board:\n{t}");
    }
    for board in ["simple-tv", "lispm-tv", "video"] {
        refused_saying("quux", &["--rtl", "--tv-board", board], "--tv-board is cadr's, not quux's");
    }
    // Its size is a flag of its own, and says so.
    let out = quux().args(["--rtl", "--video-size", "1920x1080", "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("tv: model video, 1920x1080"), "the start says the size:\n{t}");
    refused_saying(
        "quux",
        &["--video-size", "2560x1440"],
        "--video-size: 2560 by 1440 is past 1920 by 1080",
    );
    refused_saying("quux", &["--video-size", "1921x1080"], "--video-size: a width of 1921");
    refused_saying("quux", &["--video-size", "wide"], "--video-size wants");
    refused_saying("cadr", &["--video-size", "1920x1080"], "--video-size is quux's, not cadr's");
    for exe in ["quux", "cadr"] {
        refused_saying(
            exe,
            &["--mono-tv-size", "1920x1080"],
            &format!(
                "--mono-tv-size is not a flag of {exe}: the video controller's size is --video-size"
            ),
        );
    }
}

/// **QUUX drops the delay lines: its timing is `sync`, always.** A QUUX run
/// is on `sync` of four 10 ns ticks without asking, on `rtl` and `micro`
/// alike, and the start says so, the pace too; `--sync-cycle-ticks` sets
/// the ticks, and is `quux`'s alone; `--timing-model` is the CADR's choice
/// between its delay lines and muir-fpga's grid, and not a flag of `quux`;
/// `sync` is no word of it on `cadr`, which keeps its delay lines.
#[test]
fn quux_runs_on_sync_alone() {
    for engine in ["--rtl", "--micro"] {
        let out = quux().args([engine, "--pace", "--stop-after", "1"]).run();
        let t = text(&out);
        assert!(out.status.success(), "{engine}: {t}");
        assert!(t.contains("timing: sync, 4 ticks of 10 ns, 40 ns a microcycle"), "{engine}:\n{t}");
        assert!(t.contains("40 ns a microcycle; the run waits"), "{engine}: the pace:\n{t}");
    }
    let out = quux().args(["--rtl", "--sync-cycle-ticks", "3", "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("timing: sync, 3 ticks of 10 ns, 30 ns a microcycle"), "{t}");
    for model in ["sync", "cadr", "fpga"] {
        refused_saying(
            "quux",
            &["--rtl", "--timing-model", model],
            "--timing-model is cadr's, not quux's",
        );
    }
    refused_saying("cadr", &["--rtl", "--timing-model", "sync"], "sync is quux's");
    refused_saying(
        "cadr",
        &["--rtl", "--sync-cycle-ticks", "3"],
        "--sync-cycle-ticks is quux's, not cadr's",
    );
    refused_saying("quux", &["--rtl", "--sync-cycle-ticks", "0"], "--sync-cycle-ticks wants");
    let out = cadr().args(["--rtl", "--pace", "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(t.contains("timing: cadr, the board's delay lines"), "the CADR's:\n{t}");
    assert!(t.contains("145 ns a microcycle; the run waits"), "the CADR's pace:\n{t}");
}

/// **`--cache` is QUUX's and `rtl`'s**: it runs there and the start says
/// the cache, its lines 8 words (contract G2 §3), and it is refused on
/// `cadr`, on `micro`, at a size that is not a power of two, and at one
/// smaller than a set of two 8-word lines, 16 words being the least.
#[test]
fn the_cache_is_quux_s_and_rtl_s() {
    for words in ["4096", "16"] {
        let out = quux().args(["--rtl", "--cache", words, "--stop-after", "1"]).run();
        let t = text(&out);
        assert!(out.status.success(), "{t}");
        let says = format!("cache: {words} words, lines of 8, 2-way");
        assert!(t.contains(&says), "the start says it:\n{t}");
    }
    refused_saying("cadr", &["--rtl", "--cache", "4096"], "--cache is quux's, not cadr's");
    refused_saying("quux", &["--micro", "--cache", "4096"], "--cache is rtl's");
    refused_saying("quux", &["--rtl", "--cache", "3000"], "--cache:");
    refused_saying("quux", &["--rtl", "--cache", "8"], "fewer words than one set's lines");
}

/// **QUUX's main memory is on its own port** (contract Q6): the start
/// says the port's timing and the cache, fitted without `--cache`;
/// `--memory-timing` sets other figures there, and is refused on `cadr`,
/// on `micro`, and with a figure that is not one.
#[test]
fn quux_s_memory_port_and_its_timing() {
    let out = quux().args(["--rtl", "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("memory port: a line fill in 380 ns, a write in 290"), "{t}");
    assert!(t.contains("cache: 4096 words, lines of 8, 2-way"), "always fitted:\n{t}");
    let out = quux().args(["--rtl", "--memory-timing", "arty", "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(t.contains("memory port: a line fill in 220 ns, a write in 120"), "{t}");
    refused_saying(
        "cadr",
        &["--rtl", "--memory-timing", "300,200"],
        "--memory-timing is quux's, not cadr's",
    );
    refused_saying("quux", &["--micro", "--memory-timing", "300,200"], "--memory-timing is rtl's");
    refused_saying("quux", &["--rtl", "--memory-timing", "fast"], "--memory-timing");
}

/// **`--rtc` is QUUX's** (contract Q9): the real-time clock is the host's
/// by default, and `--rtc <s>` starts it at Unix second `s` and counts the
/// machine's time from there; the start says which. A second past 2^32-1,
/// which the 32-bit word cannot hold, is refused, and so is anything that
/// is not a second, and the flag on `cadr`, the CADR having no RTC.
#[test]
fn the_rtc_is_quux_s() {
    for engine in ["--micro", "--rtl"] {
        let out = quux().args([engine, "--stop-after", "1"]).run();
        let t = text(&out);
        assert!(out.status.success(), "{engine}: {t}");
        assert!(t.contains("rtc: the host's clock"), "{engine}: the start says it:\n{t}");
        for (flag, said) in [
            ("1700000000", "rtc: from 1700000000, counting machine time"),
            ("4294967295", "rtc: from 4294967295, counting machine time"),
            ("host", "rtc: the host's clock"),
        ] {
            let out = quux().args([engine, "--rtc", flag, "--stop-after", "1"]).run();
            let t = text(&out);
            assert!(out.status.success(), "{engine} --rtc {flag}: {t}");
            assert!(t.contains(said), "{engine} --rtc {flag}: the start says it:\n{t}");
        }
    }
    let out = cadr().args(["--micro", "--stop-after", "1"]).run();
    assert!(!text(&out).contains("rtc:"), "the CADR has none:\n{}", text(&out));
    // Each with a stop, so that one wrongly taken ends rather than runs on.
    let run = ["--micro", "--stop-after", "1"];
    refused_saying("quux", &[&run[..], &["--rtc", "4294967296"]].concat(), "--rtc 4294967296");
    refused_saying("quux", &[&run[..], &["--rtc", "4294967296"]].concat(), "past 4294967295");
    refused("quux", &[&run[..], &["--rtc", "-1"]].concat(), "--rtc");
    refused("quux", &[&run[..], &["--rtc", "soon"]].concat(), "--rtc");
    refused("quux", &[&run[..], &["--rtc"]].concat(), "--rtc");
    refused_saying("cadr", &[&run[..], &["--rtc", "5"]].concat(), "--rtc is quux's, not cadr's");
}

/// **`--file-root` is QUUX's** (contract Q9): the start lists the file
/// device's folders --- HOST's `/` from a folder alone, a named mount from
/// `<name>=<folder>`, each read-write or `,ro` --- and its time; with none
/// `/` is empty. A name mounted twice, a second default folder, a folder
/// that is not there, a missing value and the flag on `cadr` are refused.
#[test]
fn the_file_root_is_quux_s() {
    let dir = scratch("file-root");
    let folder = dir.join("root");
    std::fs::create_dir_all(folder.join("sys")).unwrap();
    let f = folder.display().to_string();
    let sys = format!("{}/sys", f);
    let cases: [(Vec<String>, Vec<String>); 4] = [
        (
            vec![],
            vec![
                "a real-time clock, a file device, three interval timers and reset devices".into(),
                "file device: 20 us a command and 100 us a KiB".into(),
                "file device: / is empty and read-only: nothing is mounted".into(),
            ],
        ),
        (vec!["--file-root".into(), f.clone()], vec![format!("file device: / is {f}, read-write")]),
        (
            vec!["--file-root".into(), format!("sys={sys},ro")],
            vec![
                "file device: / holds the mounts alone, read-only".into(),
                format!("file device: /sys is {sys}, read-only"),
            ],
        ),
        (
            vec![
                "--file-root".into(),
                format!("{f},ro"),
                "--file-root".into(),
                format!("scratch={sys}"),
            ],
            vec![
                format!("file device: / is {f}, read-only"),
                format!("file device: /scratch is {sys}, read-write"),
            ],
        ),
    ];
    for engine in ["--micro", "--rtl"] {
        for (flags, said) in &cases {
            let out = quux().args([engine, "--stop-after", "1"]).args(flags).run();
            let t = text(&out);
            assert!(out.status.success(), "{engine} {flags:?}: {t}");
            for line in said {
                assert!(
                    t.contains(line.as_str()),
                    "{engine} {flags:?}: the start says {line:?}:\n{t}"
                );
            }
        }
    }
    let out = cadr().args(["--micro", "--stop-after", "1"]).run();
    assert!(!text(&out).contains("file device"), "the CADR has none:\n{}", text(&out));
    let run = ["--micro", "--stop-after", "1"];
    let twice = format!("sys={f}");
    refused_saying(
        "quux",
        &[&run[..], &["--file-root", &twice, "--file-root", &twice]].concat(),
        "sys is mounted twice",
    );
    refused_saying(
        "quux",
        &[&run[..], &["--file-root", &f, "--file-root", &sys]].concat(),
        "one default",
    );
    let none = format!("{f}/none");
    refused("quux", &[&run[..], &["--file-root", &none]].concat(), "--file-root");
    refused("quux", &[&run[..], &["--file-root"]].concat(), "--file-root");
    refused_saying(
        "cadr",
        &[&run[..], &["--file-root", &f]].concat(),
        "--file-root is quux's, not cadr's",
    );
}

/// **Block-disk is QUUX's disk, and the only one**: a `quux` run has it
/// without asking and the start says it, on `micro` and `rtl`;
/// `--disk-controller` is the CADR's choice between MIT's controller's
/// netlist and its model, and not a flag of `quux`; `block-disk` is no word
/// of it on `cadr`.
#[test]
fn quux_s_disk_is_block_disk_only() {
    for engine in ["--micro", "--rtl"] {
        let out = quux().args([engine, "--stop-after", "1"]).run();
        let t = text(&out);
        assert!(out.status.success(), "{engine}: {t}");
        assert!(t.contains("disk: block-disk"), "{engine}: the start says it:\n{t}");
        for word in ["block-disk", "netlist", "model"] {
            refused_saying(
                "quux",
                &[engine, "--disk-controller", word],
                "--disk-controller is cadr's, not quux's",
            );
        }
    }
    refused_saying("cadr", &["--rtl", "--disk-controller", "block-disk"], "block-disk is quux's");
}

/// **QUUX's `--prom` is a PROM assembled at 36000, in partition order**
/// (contracts Q2 and Q8): its own file is taken and said to be the built-in
/// word for word, and MIT's, in MIT's order, is refused saying so.
#[test]
fn quux_s_prom_is_assembled_at_36000() {
    let own = concat!(env!("CARGO_MANIFEST_DIR"), "/data/quux-promh.mcr");
    let out = quux().args(["--rtl", "--prom", own, "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("QUUX's own word for word"), "the start says it:\n{t}");
    let mits = concat!(env!("CARGO_MANIFEST_DIR"), "/mit/sys/ubin/promh.mcr");
    refused_saying("quux", &["--rtl", "--prom", mits], "the file is in MIT's order");
}

/// A disk QUUX's PROM boots: `data/quux-disk.img`, the GPT disk sgdisk
/// made, with MIT's microcode 323, `mit/sys/ubin/ucadr.mcr`, in revision
/// 13's shapes and partition order at its current `MCR1`'s first block, as
/// `dd` writes it (`support::ucadr_323_at_40_partition_order`). The PROM
/// finds the microcode through the GPT and does not care whose it is.
fn quux_pack(dir: &Path) -> PathBuf {
    support::quux_gpt_disk(dir, &support::ucadr_323_at_40_partition_order()).0
}

/// The PC a run ended at and after how many microcycles, from its last
/// word: `stopped at PC <pc>[ in the PROM] after <n>` or `ran out at <n>;
/// PC <pc>[ in the PROM]`.
fn ended_at(t: &str) -> (u16, bool, u64) {
    let line = t
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("stopped at PC") || l.starts_with("ran out at"))
        .unwrap_or_else(|| panic!("no end said:\n{t}"));
    let in_prom = line.contains(" in the PROM");
    let words: Vec<&str> = line.split([' ', ';']).filter(|w| !w.is_empty()).collect();
    let after = |w: &str| words[words.iter().position(|x| *x == w).unwrap() + 1];
    let pc = u16::from_str_radix(after("PC"), 8).unwrap();
    let n = if line.starts_with("stopped") { after("after") } else { after("at") };
    (pc, in_prom, n.parse().unwrap())
}

/// **`--stop-at` and `--stop-at-prom` mean on QUUX what they mean on the
/// CADR**: the manual's "Stop when the PC reaches this address with the
/// boot PROM disabled: in microcode loaded into the control store", and
/// "the same with the PROM enabled: an address in the boot PROM". The
/// CADR's PROM lies over control store 0 until `PROMDISABLE`; QUUX's has
/// addresses of its own, 36000-37777, and is never disabled (contract Q2).
/// So on QUUX `--stop-at` is the microcode's PC outside those, and
/// `--stop-at-prom` a PC inside them, given as the control-store address
/// the PC holds, 36043 and not 43, as on the CADR, whose PROM is at 0.
///
/// A stop at the microcode's 6 is where the PROM jumps when it is done:
/// the next microcycle is the microcode's too. `rtl` passes 6 earlier,
/// while the PROM clears the control store, a write taking its address
/// through the PC and the PROM running on after it; that is not the
/// microcode reaching 6.
#[test]
fn quux_s_stops_are_the_microcode_s_and_the_prom_s() {
    // Inside the PROM, and outside it, each refused where it cannot fire.
    for pc in ["6", "43", "35777"] {
        refused_saying(
            "quux",
            &["--stop-at-prom", pc],
            &format!("--stop-at-prom {pc} is not in QUUX's boot PROM, 36000-37777"),
        );
    }
    refused_saying(
        "quux",
        &["--stop-at", "36043"],
        "--stop-at 36043 is in QUUX's boot PROM, 36000-37777: --stop-at-prom",
    );
    refused_saying(
        "cadr",
        &["--stop-at-prom", "36043"],
        "--stop-at-prom wants a PC in octal, below 1000",
    );

    let dir = scratch("quux-stops");
    let pack = quux_pack(&dir);
    for engine in ["--micro", "--rtl"] {
        // `36000` is `JUMP GO`, and `GO` is at `36043`.
        let out = quux().args([engine, "--stop-after", "100", "--stop-at-prom", "36043"]).run();
        let t = text(&out);
        assert!(out.status.success(), "{engine}:\n{t}");
        assert!(t.contains("stopped at PC 36043 in the PROM after "), "{engine}:\n{t}");

        let copy = dir.join(format!("{engine}.img"));
        std::fs::copy(&pack, &copy).unwrap();
        let run = |stops: &[&str]| {
            let out =
                quux().args([engine, "--disk-pack", copy.to_str().unwrap()]).args(stops).run();
            let t = text(&out);
            assert!(out.status.success(), "{engine}:\n{t}");
            (ended_at(&t), t)
        };
        let ((pc, in_prom, n), t) = run(&["--stop-after", "5000000", "--stop-at", "6"]);
        assert_eq!((pc, in_prom), (6, false), "{engine}: stopped at the microcode's 6:\n{t}");
        assert!(t.contains(&format!("stopped at PC 6 after {n}\n")), "{engine}:\n{t}");
        eprintln!("{engine}: at the microcode's 6 after {n} microcycles");
        // One microcycle on, the PC is still the microcode's: the PROM
        // is done.
        std::fs::copy(&pack, &copy).unwrap();
        let ((next, in_prom, m), t) = run(&["--stop-after", &(n + 1).to_string()]);
        assert_eq!(m, n + 1, "{engine}:\n{t}");
        assert!(next < 0o36000 && !in_prom, "{engine}: after 6, PC {next:o}:\n{t}");
    }

    // The same on the CADR: MIT's PROM never runs its 400, but `rtl`'s PC
    // passes 400 while the PROM clears the control store, 412,631
    // microcycles in (measured), and that stops nothing.
    let out = cadr().args(["--rtl", "--stop-after", "500000", "--stop-at-prom", "400"]).run();
    let t = text(&out);
    assert!(t.contains("stop not reached in 500000"), "{t}");
}

/// **QUUX has no debug cable** (contract Q5): the cable is a Unibus master
/// and QUUX has no Unibus, so a `quux` run has no DBGIN connector and says
/// so, and the cable's flags and the lashup are not flags of `quux`.
#[test]
fn quux_has_no_debug_cable() {
    let out = quux().args(["--rtl", "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("debug cable: none --- QUUX has no Unibus"), "{t}");
    assert!(!t.contains("DBGIN listening"), "{t}");
    for flags in [
        &["--debug-cable-listen"][..],
        &["--debug-cable-connect", "127.0.0.1:1"][..],
        &["--debug-in-process"][..],
        &["--no-debug-cable-listen"][..],
    ] {
        let mut args = vec!["--rtl"];
        args.extend_from_slice(flags);
        refused_saying("quux", &args, &format!("{} is cadr's, not quux's", flags[0]));
    }
}

/// **QUUX's disk is a file of any size, raw or a VHD** (contract Q8): the
/// 8 MiB fixtures, none of them a T-300's size, each start a QUUX run and
/// the start says what the footer made each, and its size in blocks.
#[test]
fn quux_s_disk_is_raw_or_a_vhd_of_any_size() {
    for (file, said) in [
        ("quux-disk.img", "raw, 8192 blocks"),
        ("quux-disk-fixed.vhd", "a fixed VHD, 8192 blocks"),
        ("quux-disk-dynamic.vhd", "a dynamic VHD, 8192 blocks"),
    ] {
        let path = format!("{}/data/{file}", env!("CARGO_MANIFEST_DIR"));
        let out = quux()
            .args(["--micro", "--disk-pack", &format!("{path},ro")])
            .args(["--stop-after", "1"])
            .run();
        let t = text(&out);
        assert!(out.status.success(), "{file}: {t}");
        assert!(
            t.contains(&format!("{file} in unit 0, {said}")),
            "{file}: the start says it:\n{t}"
        );
    }
}

/// **`--timing-model` is `cadr` or `fpga`, and `fpga` is `rtl`'s.** The
/// grid is what muir-fpga's fabric runs on, and it is `rtl`'s references
/// that fabric is held to; `chip` and `micro` keep the board's time, so a
/// run that asks for the grid on either is refused by the engine's name.
#[test]
fn the_timing_model_is_cadr_or_fpga_and_fpga_is_rtls() {
    refused("cadr", &["--timing-model", "fast"], "--timing-model");
    refused("cadr", &["--timing-model"], "--timing-model");
    for engine in ["--micro", "--chip"] {
        refused_saying("cadr", &[engine, "--timing-model", "fpga"], "--timing-model fpga is rtl's");
    }
}

/// **The executable is the machine, and `quux` has no netlist.** `cadr`
/// is MIT's machine as built and `quux` the CADR evolved, whose map
/// differs; each says which it is. `--machine` is a flag of neither, and
/// `--chip` is `cadr`'s alone, being the CADR's boards as MIT drew them.
#[test]
fn the_executable_is_the_machine_and_quux_has_no_netlist() {
    for exe in ["cadr", "quux"] {
        for words in [&["--machine", "quux"][..], &["--machine", "cadr"], &["--machine"]] {
            refused_saying(exe, words, &format!("--machine is not a flag of {exe}"));
        }
    }
    refused_saying("quux", &["--chip"], "--chip is cadr's, not quux's");
    for engine in ["--micro", "--rtl"] {
        let out = quux().args([engine, "--stop-after", "1"]).run();
        let t = text(&out);
        assert!(out.status.success(), "{engine}: {t}");
        assert!(t.contains("machine: quux"), "{engine} says which machine:\n{t}");
        assert!(
            t.contains("QUUX's data/quux-promh.mcr, version 2001"),
            "{engine}: QUUX's PROM:\n{t}"
        );
        let out = cadr().args([engine, "--stop-after", "1"]).run();
        let t = text(&out);
        assert!(out.status.success(), "{engine}: {t}");
        assert!(!t.contains("machine: quux"), "{engine}: the CADR is not QUUX:\n{t}");
        assert!(t.contains("System 100's own sys/ubin/promh.mcr"), "{engine}: MIT's PROM:\n{t}");
    }
}

/// The flags each executable takes, each once, as its `--help` lists them.
/// Shared first, then each machine's own. **This list is the contract**:
/// a flag added to either executable is added here, and the tests below
/// hold each executable's refusals and help to it.
const SHARED: &[&str] = &[
    "--micro",
    "--rtl",
    "--chaos-address",
    "--chaos-trace",
    "--chaos-udp",
    "--chaos-udp-default-peer",
    "--chaos-udp-peer",
    "--checkpoint",
    "--config",
    "--continue",
    "--disk-pack",
    "--glass-tty",
    "--keyboard-boot",
    "--keyboard-mapping",
    "--keyboard-mapping-dump",
    "--keyboard-mapping-trace",
    "--no-auto-boot",
    "--no-pace",
    "--pace",
    "--prom",
    "--resume",
    "--stop-after",
    "--stop-at",
    "--stop-at-prom",
    "--terminal",
    "--tv-capture",
    "--tv-capture-no-time",
    "--help",
    "--version",
];
const CADR_ONLY: &[&str] = &[
    "--chip",
    "--color-terminal",
    "--color-tv",
    "--color-tv-capture",
    "--debug-cable-connect",
    "--debug-cable-listen",
    "--debug-in-process",
    "--debuggee-chaos-address",
    "--debuggee-disk-pack",
    "--debuggee-terminal",
    "--disk-controller",
    "--disk-multiplexor",
    "--io-board",
    "--main-memory",
    "--main-memory-boards",
    "--no-debug-cable-listen",
    "--serial",
    "--timing-model",
    "--tv",
    "--tv-board",
    "--watch",
];
const QUUX_ONLY: &[&str] = &[
    "--cache",
    "--file-root",
    "--main-memory-size",
    "--memory-timing",
    "--microcycle-ns",
    "--rtc",
    "--sync-cycle-ticks",
    "--tlb",
    "--video-size",
];

/// **A flag of the other executable is refused by name, saying whose it
/// is**, before anything it takes is read: `--cache is quux's, not
/// cadr's`. A flag of neither is not a flag of this one. Each is a usage
/// error, exit status 2, and none of them starts a machine.
#[test]
fn a_flag_of_the_other_executable_says_whose_it_is() {
    for (exe, other, theirs) in [("cadr", "quux", QUUX_ONLY), ("quux", "cadr", CADR_ONLY)] {
        for flag in theirs {
            refused_saying(
                exe,
                &[flag, "--stop-after", "1"],
                &format!("{flag} is {other}'s, not {exe}'s"),
            );
        }
        refused_saying(exe, &["--no-such-flag"], &format!("--no-such-flag is not a flag of {exe}"));
        refused_saying(exe, &["stray"], &format!("stray is not a flag of {exe}"));
    }
}

/// **`--help` lists the executable's own flags and no other's**: every
/// flag it takes has an entry, and no word of it that looks like a flag is
/// a flag the executable refuses. The usage line starts with the
/// executable's name.
#[test]
fn the_help_lists_only_the_executable_s_own_flags() {
    for (exe, own, theirs) in [("cadr", CADR_ONLY, QUUX_ONLY), ("quux", QUUX_ONLY, CADR_ONLY)] {
        let out = executable(exe).arg("--help").run();
        let t = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(out.status.success(), "{exe} --help:\n{}", text(&out));
        assert!(t.starts_with(&format!("usage: {exe} ")), "{exe}: the usage first:\n{t}");
        let named: std::collections::BTreeSet<String> = t
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .filter(|w| w.starts_with("--") && w.len() > 2 && !w.starts_with("---"))
            .map(str::to_string)
            .collect();
        for flag in SHARED.iter().chain(own) {
            let entry = t.lines().any(|l| {
                let l = l.trim_start();
                l.starts_with(&format!("{flag} "))
                    || l.starts_with(&format!("{flag}\n"))
                    || l == *flag
                    || l.contains(&format!(", {flag}"))
                    || l.contains(&format!("| {flag}"))
            });
            assert!(entry, "{exe} --help has no entry for {flag}:\n{t}");
        }
        for flag in &named {
            assert!(
                SHARED.contains(&flag.as_str()) || own.contains(&flag.as_str()),
                "{exe} --help names {flag}, which is not a flag of {exe}:\n{t}"
            );
            assert!(!theirs.contains(&flag.as_str()), "{exe} --help names {flag}:\n{t}");
        }
        assert!(!named.contains("--machine"), "{exe} --help names --machine:\n{t}");
    }
}

/// **`--color-tv` takes a word, and the bare flag is the netlist on `chip`
/// and the model everywhere else.** The second display board is a board
/// like the others: a netlist on `chip`'s backplane unless a flag says
/// otherwise, and a model on the two engines that have no backplane to
/// put one on. `--color-tv netlist` there is refused by the engine's name,
/// as `--tv netlist` would be.
///
/// The chip runs keep every other board a model, there being nothing here
/// about them and a netlist backplane costing a second a run to build.
#[test]
fn the_color_tv_is_a_netlist_on_chip_and_a_model_elsewhere() {
    let models = ["--main-memory", "model", "--io-board", "model", "--disk-controller", "model"];

    // The bare flag on `chip`: the board on the backplane.
    let out = cadr().args(["--chip", "--color-tv"]).args(models).args(["--stop-after", "1"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("color TV netlist"), "the boards line says which board:\n{t}");
    assert!(t.contains("color tv: netlist lispm-tv at 17200000"), "and where it is:\n{t}");

    // And `model` on `chip`, which is what the board was before there was
    // a netlist of it.
    let out = cadr()
        .args(["--chip", "--color-tv", "model"])
        .args(models)
        .args(["--stop-after", "1"])
        .run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("color TV model"), "the boards line says which board:\n{t}");
    assert!(!t.contains("color TV netlist"), "and only one of them:\n{t}");

    // `netlist` on an engine with no backplane is refused by the engine's
    // name, and `model` is taken there.
    for engine in ["--micro", "--rtl"] {
        refused_saying(
            "cadr",
            &[engine, "--color-tv", "netlist", "--stop-after", "1"],
            "--color-tv",
        );
        let out = cadr().args([engine, "--color-tv", "model", "--stop-after", "1"]).run();
        let t = text(&out);
        assert!(out.status.success(), "{engine} with the model board:\n{t}");
        assert!(t.contains("color tv: model lispm-tv"), "{engine}:\n{t}");
    }

    // A word that is neither is refused by the flag's name, and the flag
    // with nothing after it is still the bare flag.
    refused("cadr", &["--rtl", "--color-tv", "both", "--stop-after", "1"], "--color-tv");
    let out = cadr().args(["--rtl", "--color-tv", "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("color tv: model lispm-tv"), "the bare flag off chip is the model:\n{t}");
}

/// A `.cadrrc` of its own for one test, in a directory of its own: the
/// directory, and the file's path under it.
fn cadrrc(name: &str, text: &str) -> (Scratch, PathBuf) {
    let dir = scratch(&format!("rc-{name}"));
    let path = dir.join(".cadrrc");
    std::fs::write(&path, text).unwrap();
    (dir, path)
}

/// **The flags in the file are the run's**, comments and blank lines
/// **The version comes first, then the file of flags, then everything
/// else --- and a run that is refused has said both before it says why.**
///
/// A report of a run says which muir made it, and a report of a *refused*
/// run wants the same two facts most of all: which muir, and which file
/// of flags it read, since a file it was not asked about is the commonest
/// reason a run will not start.  So both are written before anything is
/// parsed, and an error follows them rather than standing alone.
#[test]
fn the_version_and_the_file_are_said_before_anything_is_parsed() {
    let (_dir, rc) = cadrrc("flags", "--chaos-address 3050,3060\n");
    let out = cadr().env("MUIR_RC", &rc).args(["--stop-after", "1"]).run();
    let t = text(&out);
    assert_eq!(out.status.code(), Some(2), "the run is refused:\n{t}");
    let lines: Vec<&str> = t.lines().collect();
    assert!(lines[0].starts_with("muir 0.1.0"), "the version is the first line:\n{t}");
    assert!(lines[1].contains(&rc.display().to_string()), "the file of flags is the second:\n{t}");
    let why = lines.iter().position(|l| l.starts_with("cadr: ")).expect("a refusal");
    assert!(why > 1, "and the refusal comes after both:\n{t}");

    // A run that starts says them once and not twice.
    let out = cadr().args(["--micro", "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert_eq!(t.matches("muir 0.1.0").count(), 1, "the version once:\n{t}");
    assert!(!t.contains("flags:"), "and no file, this run having none:\n{t}");

    // And `info` says it again, being what the run is: the two lines are
    // written before there is a machine to describe, and the prompt has
    // them all the same.
    let mut child =
        cadr().args(["--micro", "--no-auto-boot"]).stdin(std::process::Stdio::piped()).start();
    let mut stdin = child.stdin();
    write!(stdin, "info\nquit\n").expect("muir took the lines");
    drop(stdin);
    let out = child.wait();
    let t = text(&out);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("muir 0.1.0"),
        "info says which muir:\n{t}"
    );
}

/// **`--version` and `--help` are answered before the file of flags is
/// read at all**, because neither runs a machine and so nothing a file
/// configures applies to either.
///
/// A file outlives the flags it holds. `--chaos-address 3050,3060` was
/// the spelling until the Chaosnet server left muir, and a file still
/// holding it is refused --- rightly, for a run. But `cadr --version`
/// asks what this build is, and a build that cannot say so because of a
/// file it was not asked to use leaves a person with no way to report
/// which muir they have.
#[test]
fn the_version_and_the_help_are_answered_whatever_the_file_holds() {
    let (_dir, rc) = cadrrc("flags", "--chaos-address 3050,3060\n");
    // The file really is refused for a run, or this test proves nothing.
    let out = cadr().env("MUIR_RC", &rc).args(["--stop-after", "1"]).run();
    let t = text(&out);
    assert_eq!(out.status.code(), Some(2), "the file is refused for a run:\n{t}");
    assert!(t.contains("--chaos-address"), "by name:\n{t}");

    for flag in ["--version", "-V"] {
        let out = cadr().env("MUIR_RC", &rc).arg(flag).run();
        let t = text(&out);
        assert!(out.status.success(), "{flag} is answered all the same:\n{t}");
        assert!(t.starts_with("muir "), "{flag} says what this build is:\n{t}");
        assert!(!t.contains("usage:"), "{flag} is no run:\n{t}");
    }
    for flag in ["--help", "-h"] {
        let out = cadr().env("MUIR_RC", &rc).arg(flag).run();
        let t = text(&out);
        assert!(out.status.success(), "{flag} is answered all the same:\n{t}");
        assert!(t.contains("usage: cadr"), "{flag} prints the usage:\n{t}");
    }
}

/// apart, and whatever a flag takes is the rest of the line, spaces and
/// all. The start says which flags came from the file, and where from.
#[test]
fn the_flags_in_the_file_are_the_runs() {
    // A directory with a space in its name, for the rest of the line.
    let dir = scratch("rc root");
    let gif = dir.join("a recording.gif");
    let (_rc_dir, rc) = cadrrc(
        "flags",
        &format!(
            "# what every run of mine wants\n\n--rtl\n--stop-after 10\n--tv-capture {}\n",
            gif.display()
        ),
    );
    let out = cadr().env("MUIR_RC", &rc).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("engine: rtl"), "the engine came from the file:\n{t}");
    assert!(t.contains("stop: after 10 microcycles"), "and the stop:\n{t}");
    assert!(
        t.contains(&format!("capture: {}", gif.display())),
        "the rest of the line is one word, spaces and all:\n{t}"
    );
    assert!(t.contains(&format!("from {}", rc.display())), "the start says where from:\n{t}");
    assert!(!t.contains("# what every run"), "the comment is not a flag:\n{t}");
}

/// **What the command line gives replaces what the file gives**, the
/// engine among it: `--micro`, `--rtl` and `--chip` are exclusive of one
/// another, so the file's is dropped rather than refused.
#[test]
fn the_command_line_replaces_the_file() {
    let (_dir, rc) = cadrrc("replaced", "--rtl\n--stop-after 10\n");
    let out = cadr().env("MUIR_RC", &rc).args(["--micro", "--stop-after", "3"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("engine: micro"), "the command line's engine:\n{t}");
    assert!(t.contains("stop: after 3 microcycles"), "and its stop:\n{t}");
    assert!(!t.contains("flags:"), "nothing was left to come from the file:\n{t}");
}

/// **A file that is not there is no error**: muir runs as if there were
/// none, which is what most runs have.
#[test]
fn no_file_is_no_error() {
    let out =
        cadr().env("MUIR_RC", "/no/such/.cadrrc").args(["--micro", "--stop-after", "3"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(!t.contains("flags:"), "{t}");
}

/// **`--config` names the file**, `-c` for short, and one named that is
/// not there is a usage error: a file asked for by name is one the run is
/// meant to have.
#[test]
fn config_names_the_file() {
    let (_dir, rc) = cadrrc("named", "--micro\n--stop-after 7\n");
    for flag in ["--config", "-c"] {
        let out = cadr().args([flag]).arg(&rc).run();
        let t = text(&out);
        assert!(out.status.success(), "{flag}:\n{t}");
        assert!(t.contains("stop: after 7 microcycles"), "{flag}:\n{t}");
        assert!(t.contains(&format!("from {}", rc.display())), "{flag}:\n{t}");
    }
    refused("cadr", &["--config", "/no/such/.cadrrc"], "--config");
    refused("cadr", &["--config"], "--config");
}

/// **The directory muir was run from comes before the home directory, and
/// `--config` before both** --- the first of the three there, not all of
/// them.
#[test]
fn the_run_directory_comes_before_the_home_one() {
    let (home, _) = cadrrc("home", "--micro\n--stop-after 11\n");
    let (here, _) = cadrrc("here", "--micro\n--stop-after 22\n");
    let (_named_dir, named) = cadrrc("named-first", "--micro\n--stop-after 33\n");
    let run = |args: &[&Path]| {
        let mut c = cadr();
        c.env_remove("MUIR_RC").env("HOME", home.path()).current_dir(here.path());
        for a in args {
            c.arg("--config").arg(a);
        }
        text(&c.run())
    };
    let t = run(&[]);
    assert!(t.contains("stop: after 22 microcycles"), "the directory muir was run from:\n{t}");
    let t = run(&[&named]);
    assert!(t.contains("stop: after 33 microcycles"), "--config before either:\n{t}");
    // With nothing in the run directory, the home one is what is left.
    let empty = scratch("rc-empty");
    let mut c = cadr();
    c.env_remove("MUIR_RC").env("HOME", home.path()).current_dir(&empty);
    let t = text(&c.run());
    assert!(t.contains("stop: after 11 microcycles"), "the home directory:\n{t}");
}

/// **Each executable reads its own file of flags**: `cadr` `.cadrrc` and
/// `quux` `.quuxrc`, in the directory it was run from or else the home
/// directory, and never the other's, nor `.muirrc`, which neither reads.
#[test]
fn each_executable_reads_its_own_file() {
    let home = scratch("rc-own-home");
    let here = scratch("rc-own-here");
    std::fs::write(home.join(".cadrrc"), "--micro\n--stop-after 11\n").unwrap();
    std::fs::write(home.join(".quuxrc"), "--micro\n--stop-after 22\n").unwrap();
    std::fs::write(home.join(".muirrc"), "--micro\n--stop-after 33\n").unwrap();
    // The other's file in the run directory is not this one's.
    std::fs::write(here.join(".quuxrc"), "--micro\n--stop-after 44\n").unwrap();
    let run = |exe: &str| {
        let mut c = executable(exe);
        c.env_remove("MUIR_RC").env("HOME", home.path()).current_dir(here.path());
        let out = c.run();
        let t = text(&out);
        assert!(out.status.success(), "{exe}:\n{t}");
        t
    };
    let t = run("cadr");
    assert!(t.contains("stop: after 11 microcycles"), "cadr reads ~/.cadrrc:\n{t}");
    assert!(t.contains(".cadrrc"), "and says so:\n{t}");
    let t = run("quux");
    assert!(t.contains("stop: after 44 microcycles"), "quux reads ./.quuxrc first:\n{t}");
    std::fs::remove_file(here.join(".quuxrc")).unwrap();
    let t = run("quux");
    assert!(t.contains("stop: after 22 microcycles"), "then ~/.quuxrc:\n{t}");
    // `.muirrc` alone is no file of flags for either.
    std::fs::remove_file(home.join(".cadrrc")).unwrap();
    std::fs::remove_file(home.join(".quuxrc")).unwrap();
    for exe in ["cadr", "quux"] {
        let mut c = executable(exe);
        c.env_remove("MUIR_RC").env("HOME", home.path()).current_dir(here.path());
        let t = text(&c.args(["--micro", "--stop-after", "1"]).run());
        assert!(!t.contains("flags:"), "{exe} reads no .muirrc:\n{t}");
        assert!(!t.contains("after 33"), "{exe} reads no .muirrc:\n{t}");
    }
}

/// **A flag of the other machine's in a file of flags is refused as it is
/// on the command line, naming the file**: the file is the executable's
/// own, so a flag in it that the executable does not take is the file's
/// mistake, and the refusal says which file to mend.
#[test]
fn a_flag_of_the_other_machine_in_the_file_is_refused_naming_it() {
    let home = scratch("rc-other-home");
    let here = scratch("rc-other-here");
    for (exe, file, line, says) in [
        ("cadr", ".cadrrc", "--cache 4096", "--cache is quux's, not cadr's"),
        ("quux", ".quuxrc", "--chip", "--chip is cadr's, not quux's"),
        ("cadr", ".cadrrc", "--machine quux", "--machine is not a flag of cadr"),
    ] {
        let path = home.join(file);
        std::fs::write(&path, format!("--micro\n{line}\n")).unwrap();
        for by_env in [false, true] {
            let mut c = executable(exe);
            c.env("HOME", home.path()).current_dir(here.path());
            if by_env {
                c.env("MUIR_RC", &path);
            } else {
                c.env_remove("MUIR_RC");
            }
            let out = c.args(["--stop-after", "1"]).run();
            let err = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), Some(2), "{exe} {line}: a usage error:\n{err}");
            let want = format!("{exe}: {}: {says}", path.display());
            assert!(err.contains(&want), "{exe} {line}: {want:?}:\n{err}");
        }
        std::fs::remove_file(&path).unwrap();
    }
}

/// **A file of flags cannot name another.** Which file to read is the
/// command line's to say, and a file that could point at another could
/// point at itself.
#[test]
fn a_file_cannot_name_another() {
    let (_dir, rc) = cadrrc("recursive", "--micro\n--config /somewhere/else\n");
    let out = cadr().arg("--config").arg(&rc).run();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "a usage error:\n{err}");
    assert!(err.contains("cannot name another"), "{err}");
}

/// **A PROM muir cannot read stops the run before it starts.** The file
/// is the program the machine is about to execute, so a run on a file
/// that is missing, or that is not MIT's MCR, is no run at all: it is
/// refused with the flag named, not begun on 512 zero words.
#[test]
fn a_prom_that_cannot_be_read_is_refused() {
    refused("cadr", &["--rtl", "--prom", "nothing.mcr", "--stop-after", "1"], "--prom");
    refused("cadr", &["--rtl", "--prom", "README.md", "--stop-after", "1"], "--prom");
}

/// **A checkpoint carries the PROM it ran**, all 512 words of it, so
/// `--resume` and `--prom` would be two answers to the same question and
/// the checkpoint's would win silently. Refused instead.
#[test]
fn a_prom_and_a_checkpoint_to_resume_do_not_go_together() {
    refused(
        "cadr",
        &["--rtl", "--prom", "mit/sys/ubin/promh.mcr", "--resume", "nothing.chk"],
        "--prom",
    );
}

/// **The PROM the machine runs is the file `--prom` names**, on all three
/// engines. One file has to reach all three, and they take it by two
/// different routes: `micro` and `rtl` fetch the microinstruction, `chip`
/// fetches what the 74S472s hold, which is the programming image derived from
/// it.
///
/// The file is MIT's own PROM with word 0 --- the reset vector, a jump to
/// `GO` at 45 --- retargeted at 400. Word 0 is the machine's first fetch,
/// so `--stop-at-prom 400` is reached at once; MIT's own never goes near
/// 400 and runs the window out instead. Which of the two happened is what
/// the end of the run says.
#[test]
fn the_prom_the_machine_runs_is_the_file_named() {
    let mut words: Vec<u64> = muir::prom::boot_prom().iter().map(|i| i.raw()).collect();
    assert_eq!(muir::isa::Insn::new(words[0]).jump().target, 0o45, "the reset vector jumps to GO");
    // `IR<25:12>` is the jump's target, `muir::isa::Insn::jump`.
    words[0] = words[0] & !(0o37777 << 12) | 0o400 << 12;
    let dir = scratch("prom");
    let path = dir.join("retargeted.mcr");
    std::fs::write(&path, support::mcr(&words)).unwrap();

    // `chip` runs its boards as models: what is under test is the PROM
    // reaching the chips, not the rest of the backplane.
    let models = ["--main-memory", "model", "--io-board", "model", "--tv", "model"];
    for engine in ["--micro", "--rtl", "--chip"] {
        let mut c = cadr();
        c.args([engine, "--prom", path.to_str().unwrap()]);
        if engine == "--chip" {
            c.args(models);
        }
        let out = c.args(["--stop-after", "100", "--stop-at-prom", "400"]).run();
        let t = text(&out);
        assert!(out.status.success(), "{engine}:\n{t}");
        assert!(t.contains("stopped at PC 400 in the PROM"), "{engine} ran the named PROM:\n{t}");
    }

    // And MIT's own, which the flag is standing in for, does not.
    let out = cadr().args(["--micro", "--stop-after", "100", "--stop-at-prom", "400"]).run();
    let t = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(t.contains("stop not reached in 100"), "MIT's own does not go to 400:\n{t}");
}

/// **`--version` says what this build is**, `cadr`'s and `quux`'s alike,
/// both being one muir, and says it on stdout so a script can read it. The name, the crate's version, and whether it was
/// built with optimizations off --- a run says the same line first, so a
/// report of a run says which muir made it.
#[test]
fn version_says_what_this_build_is() {
    for exe in ["cadr", "quux"] {
        for flag in ["--version", "-V"] {
            let out = executable(exe).arg(flag).run();
            assert!(out.status.success(), "{exe}: {}", text(&out));
            let said = String::from_utf8_lossy(&out.stdout);
            let said = said.trim();
            let want = format!("muir {}-", env!("CARGO_PKG_VERSION"));
            assert!(said.starts_with(&want), "{exe} {flag}: {said:?}");
            assert!(
                said.ends_with("-dev") || said.ends_with("-release"),
                "{exe} {flag}: the build kind: {said:?}"
            );
        }
    }
}

/// **A drive the netlist controller has no port for is refused until the
/// multiplexor is asked for.**
///
/// Not because the unit number is forced to 0. `UNIT<2:0>` reach one
/// 74LS244's inputs on the controller and nothing else --- `cadrdc/dc.wlr`
/// gives a direction per pin and none of the three has a `TO` --- so the
/// board has no driver for them at all, and the six one-board jumpers
/// ground them to stop three inputs floating. Unit 0 is the consequence.
/// The DISK MULTIPLEXOR is what supplies the driver, so a second drive or
/// a drive past unit 0 wants one.
///
/// **muir does not fit it by implication**, though it could and once did:
/// a board that appears because of the way a pack was spelled is a board
/// the machine has without anybody choosing it, and which machine is
/// being simulated is the user's to say. So the run stops and names the
/// flag.
///
/// The model controller is refused nothing, because it wants no board: it
/// is behavioral and has addressed eight units all along,
/// `disk_controller::UNITS`.
#[test]
fn a_drive_past_unit_0_wants_the_multiplexor_named() {
    const FITTED: &str = "with a multiplexor";
    let start =
        |args: &[&str]| String::from_utf8_lossy(&cadr().args(args).run().stderr).into_owned();
    let netlist =
        ["--chip", "--main-memory", "netlist", "--disk-controller", "netlist", "--stop-after", "1"];
    let with = |extra: &[&'static str]| -> Vec<&'static str> {
        let mut args = netlist.to_vec();
        args.extend_from_slice(extra);
        args
    };
    // The image is never opened: the start says what the machine is
    // before it is built, and a missing pack stops the run after that.
    let one = start(&with(&["--disk-pack", "nothing.img"]));
    assert!(!one.contains(FITTED), "one drive in unit 0 wants no board:\n{one}");
    // A drive the one port cannot reach, refused by name.
    for extra in [
        &["--disk-pack", "nothing.img,3"][..],
        &["--disk-pack", "a.img", "--disk-pack", "b.img,1"][..],
    ] {
        let said = start(&with(extra));
        assert!(said.contains("--disk-multiplexor"), "{extra:?} names the flag:\n{said}");
        assert!(said.contains("usage:"), "{extra:?} is refused:\n{said}");
    }
    // And with the flag, fitted and said to be.
    for extra in [
        &["--disk-multiplexor", "--disk-pack", "nothing.img"][..],
        &["--disk-multiplexor", "--disk-pack", "nothing.img,3"][..],
        &["--disk-multiplexor", "--disk-pack", "a.img", "--disk-pack", "b.img,1"][..],
    ] {
        let said = start(&with(extra));
        assert!(said.contains(FITTED), "{extra:?} fits the board, and says so:\n{said}");
        assert!(!said.contains("usage:"), "{extra:?} is not refused:\n{said}");
    }
    // The model controller takes the units and fits nothing: on `micro`,
    // where it is the only controller there is, and on `chip` asked for
    // by name, where the start says which boards are on the buses and so
    // can be caught saying it fitted one.
    for engine in [&["--micro"][..], &["--chip", "--disk-controller", "model"][..]] {
        let mut args = engine.to_vec();
        args.extend(["--disk-pack", "a.img", "--disk-pack", "b.img,1", "--stop-after", "1"]);
        let said = start(&args);
        assert!(!said.contains(FITTED), "{engine:?}: the model controller wants no board:\n{said}");
        assert!(!said.contains("usage:"), "{engine:?}: and is not refused:\n{said}");
    }
}

/// **The multiplexor is a board, so it needs a board to hang off**, and
/// one pack a drive.
#[test]
fn the_multiplexor_is_the_netlist_controllers_board() {
    // On `chip` the controller is a netlist, so the board has one to hang
    // off and is fitted; asked for against the model controller, or on an
    // engine that has no netlist at all, it is refused.
    let out = cadr().args(["--chip", "--disk-multiplexor", "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(out.status.success(), "chip has a netlist controller to hang it off:\n{t}");
    assert!(t.contains("with a multiplexor"), "and it is fitted:\n{t}");
    refused(
        "cadr",
        &["--chip", "--disk-controller", "model", "--disk-multiplexor", "--stop-after", "1"],
        "--disk-multiplexor",
    );
    refused("cadr", &["--micro", "--disk-multiplexor", "--stop-after", "1"], "--disk-multiplexor");
    refused(
        "cadr",
        &["--micro", "--disk-pack", "a.img,1", "--disk-pack", "b.img,1", "--stop-after", "1"],
        "--disk-pack",
    );
}

/// Where the label `name` is in MIT's boot PROM, from its symbol table,
/// `mit/sys/ubin/promh.sym`: a line `<name> I-MEM <octal>`.
fn prom_label(name: &str) -> u16 {
    let sym =
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/mit/sys/ubin/promh.sym")).unwrap();
    let sym = String::from_utf8_lossy(&sym);
    let at = sym
        .lines()
        .find_map(|l| l.strip_prefix(name)?.trim().strip_prefix("I-MEM "))
        .unwrap_or_else(|| panic!("{name} is not in promh.sym"));
    u16::from_str_radix(at.trim(), 8).unwrap()
}

/// **`wp` is the Trident's read-only switch, and `ro` is not a switch.**
/// MIT's boot PROM takes `STATUS<7>`, the switch, for a disk error ---
/// `A-DISK-ERROR` has `<7:4>` in it, `mit/sys/ucadr/promh.text` --- and
/// halts at `ERROR-DISK-ERROR` on its first disk command, before it has
/// read a word. A pack opened `ro` is a writable drive to the machine, so
/// the same PROM gets through its disk commands, writes block 1 and reads
/// the label; a pack of zeros has none, and it halts at `ERROR-BAD-LABEL`
/// instead. The pack is a T-300's size and sparse, and nothing needs
/// `vendor/`.
#[test]
fn wp_is_the_drives_read_only_switch_and_ro_is_not() {
    let dir = scratch("wp");
    let pack = dir.join("zeros.img");
    // A T-300: 815 cylinders, 19 heads, 17 blocks a track, 1 KiB a block.
    const T300: u64 = 815 * 19 * 17 * 1024;
    std::fs::File::create(&pack).unwrap().set_len(T300).unwrap();
    for (spelled, halts_at, start) in [
        ("wp", "ERROR-DISK-ERROR", "the read-only switch on: a write faults"),
        ("ro", "ERROR-BAD-LABEL", "opened read-only: a written block stays in the run"),
    ] {
        let out = cadr()
            .args(["--micro", "--disk-pack", &format!("{},{spelled}", pack.display())])
            .args(["--stop-after", "5000000"])
            .run();
        let t = text(&out);
        assert!(t.contains(start), "{spelled}: the start says what it is:\n{t}");
        let pc = format!("PC {:o} in the PROM; ", prom_label(halts_at));
        assert!(t.contains("the machine stopped itself"), "{spelled}: halted:\n{t}");
        assert!(t.contains(&pc), "{spelled}: halted at {halts_at}, {pc}:\n{t}");
    }
    // QUUX's block-disk has no such switch, so `quux` has no `wp`.
    refused_saying(
        "quux",
        &["--micro", "--disk-pack", "a.img,wp"],
        "wp is the CADR's Trident's read-only switch, and block-disk has none",
    );
    refused_saying("cadr", &["--micro", "--disk-pack", "a.img,wp,rw"], "wp with rw");
}

/// **`kill -USR1` asks a running machine where it is, and it answers and
/// goes on.**
///
/// A long `chip` run has no prompt --- the process has a terminal and
/// nothing else --- so before this the only way to know where one was was
/// to infer it from what it had touched. Issue 86 has a run whose state
/// was read from pack mtimes, then from lit pixels, then from a
/// block-by-block comparison of the pack, two of the three retracted, over
/// six hours, and `pc` unanswered the whole time.
///
/// **The answer arrives while the machine runs**, which is the point: a
/// reader that held the run would be no use for the timing runs this
/// exists for. Measured over the same 20,000-microcycle window, signaled
/// three times against not at all: both end at PC 240 having run 20,000,
/// in 8.926 s against 8.903, which is the cost of the three lines printed.
/// So asking neither changes the answer nor slows the run.
///
/// The signal is taken by every engine and acted on by `chip` alone,
/// because the default action for `SIGUSR1` is to kill the process: one
/// sent to the wrong run of a pair would otherwise end a run that had been
/// going for hours.
#[test]
fn a_running_chip_says_where_it_is_when_asked() {
    // Long enough to still be running when the signal lands, short enough
    // that the test is a second or two: `chip` does about 2,200
    // microcycles a second.
    // **Signaled once the run says it is listening, not after a guess at
    // how long that takes.**  The default action for `SIGUSR1` is to kill
    // the process, so a signal sent before the handler is installed kills
    // the run --- and building a `chip` machine takes as long as it takes,
    // which on a loaded machine is longer than any sleep worth writing.
    // The run's own start line says when it is armed.
    let child =
        cadr().args(["--chip", "--main-memory-boards", "4", "--stop-after", "6000"]).start();
    child.stderr().wait_until(|t| t.contains("where: kill -USR1"), "muir says it is listening");
    let pid = child.id().to_string();
    let killed = std::process::Command::new("kill")
        .args(["-USR1", &pid])
        .status()
        .expect("kill did not run");
    assert!(killed.success(), "the signal was delivered");
    let out = child.wait();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let said = text
        .lines()
        .find(|l| l.starts_with("PC ") && l.contains("microcycles this run"))
        .unwrap_or_else(|| panic!("no answer to the signal in:\n{text}"));
    // It answered from inside the run rather than at the end of it.
    let ran: u64 = said
        .rsplit_once("; ")
        .and_then(|(_, tail)| tail.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no microcycle count in `{said}`"));
    assert!(0 < ran && ran < 6_000, "answered mid-run, at {ran} of 6000: `{said}`");
    assert!(text.contains("IR "), "and said what it was executing:\n{text}");
    assert!(text.contains("ran out at 6000"), "and ran to its stop after answering:\n{text}");
}

/// **The CHUDP flags describe a link that has to be there.** With neither
/// `--chaos-address` nor `--chaos-udp` nothing is listening, so a peer and
/// a route of last resort for the rest are each a statement about a link
/// that does not exist.
#[test]
fn the_chudp_flags_need_the_link() {
    refused(
        "cadr",
        &["--chaos-udp-peer", "3040@127.0.0.1:42043", "--stop-after", "1"],
        "--chaos-udp-peer",
    );
    refused(
        "cadr",
        &["--chaos-udp-default-peer", "127.0.0.1:42043", "--stop-after", "1"],
        "--chaos-udp-default-peer",
    );
}

/// **The default peer is an endpoint and no Chaosnet address.** It is
/// not a host at an address, which is what `--chaos-udp-peer` names; it
/// is where a frame goes whose destination no peer entry names, and the
/// CHUDP frame carries the real destination in its trailer for the
/// bridge there to route on. So it takes a port, an address, or
/// address:port, the port left off taking the protocol's own, and
/// `<address>@<host>` is refused rather than read as either.
#[test]
fn the_default_peer_is_an_endpoint_and_no_address() {
    let link = ["--chaos-udp", "127.0.0.1:0"];
    let out = cadr()
        .args(link)
        .args(["--chaos-udp-default-peer", "127.0.0.1"])
        .args(["--micro", "--stop-after", "1"])
        .run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("anything else to 127.0.0.1:42042"), "the protocol's own port: {t}");
    for bad in ["3060@127.0.0.1:42043", "127.0.0.1:not-a-port"] {
        let args =
            [link.as_slice(), &["--chaos-udp-default-peer", bad], &["--stop-after", "1"]].concat();
        refused("cadr", &args, "--chaos-udp-default-peer");
    }
}

/// **A peer is one endpoint, and not this machine's own address.** Two
/// endpoints for one address are two answers to where one host lives; the
/// address this machine answers at is not a host over the network. Only
/// this machine's: nothing else is on the cable, the file and time host
/// being a peer like any other.
#[test]
fn a_peer_is_one_endpoint_and_not_this_machines_own() {
    let link = ["--chaos-udp", "127.0.0.1:0"];
    let twice = [
        link.as_slice(),
        &["--chaos-udp-peer", "3040@127.0.0.1:42043"],
        &["--chaos-udp-peer", "3040@127.0.0.1:42044"],
        &["--stop-after", "1"],
    ]
    .concat();
    refused("cadr", &twice, "--chaos-udp-peer");
    // The cable's own with no --chaos-address: `chaos::Config`'s default,
    // 177001, which is no band's.
    let own =
        [link.as_slice(), &["--chaos-udp-peer", "177001@127.0.0.1:42043"], &["--stop-after", "1"]]
            .concat();
    refused("cadr", &own, "--chaos-udp-peer");
    // 177002 was the old server's address, and is now a peer like any
    // other: nothing on this cable answers there.
    let out = cadr()
        .args(link)
        .args(["--chaos-udp-peer", "177002@127.0.0.1:42043", "--micro", "--stop-after", "1"])
        .run();
    assert!(out.status.success(), "{}", text(&out));
    // And this machine's own address is a peer when this machine is
    // somewhere else.
    let out = cadr()
        .args(link)
        .args(["--chaos-address", "4401"])
        .args(["--chaos-udp-peer", "3050@127.0.0.1:42043", "--micro", "--stop-after", "1"])
        .run();
    assert!(out.status.success(), "{}", text(&out));
}

/// **A peer's spelling is `<address>@<host>[:<port>]`**, the address in
/// octal or `subnet:host`, and the port may be left off for the
/// protocol's own. A name with no address is refused at the start rather
/// than becoming a peer that is never reached.
#[test]
fn a_peer_is_an_address_and_where_it_lives() {
    let link = ["--chaos-udp", "127.0.0.1:0"];
    for bad in [
        // no address
        "127.0.0.1:42043",
        // nowhere to live
        "3040",
        // 9 is not an octal digit
        "99@127.0.0.1:42043",
        // a name with no address
        "3040@no-such-host.invalid",
        // nor is that a port
        "3040@127.0.0.1:not-a-port",
    ] {
        let args = [link.as_slice(), &["--chaos-udp-peer", bad], &["--stop-after", "1"]].concat();
        refused("cadr", &args, "--chaos-udp-peer");
    }
    // The port may be left off, and the address may be subnet:host.
    let out = cadr()
        .args(link)
        .args(["--chaos-udp-peer", "6:40@127.0.0.1"])
        .args(["--micro", "--stop-after", "1"])
        .run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("3040 at 127.0.0.1:42042"), "the protocol's own port: {t}");
}

/// **The start says the link is there and who is on it**, so that a run
/// that reaches nobody says so rather than looking as though it had.
#[test]
fn the_start_says_what_the_link_is() {
    let out = cadr()
        .args(["--micro", "--stop-after", "1", "--chaos-udp", "127.0.0.1:0"])
        .args(["--chaos-udp-peer", "3060@127.0.0.1:42043"])
        .args(["--chaos-udp-default-peer", "127.0.0.1:42044"])
        .run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    let line =
        t.lines().find(|l| l.starts_with("chaosnet over udp:")).unwrap_or_else(|| panic!("{t}"));
    assert!(line.contains("127.0.0.1:"), "where it listens: {line}");
    assert!(line.contains("3060 at 127.0.0.1:42043"), "and who is on it: {line}");
    assert!(line.contains("anything else to 127.0.0.1:42044"), "and where the rest go: {line}");
    // A link with no peer named is a run that reaches no file or time
    // host, and the line says so rather than leaving it to be found out
    // at the cold-load debugger.
    let out = cadr().args(["--micro", "--stop-after", "1", "--chaos-udp", "127.0.0.1:0"]).run();
    let t = text(&out);
    let line =
        t.lines().find(|l| l.starts_with("chaosnet over udp:")).unwrap_or_else(|| panic!("{t}"));
    assert!(line.contains("no peer named, so no file or time host"), "{line}");
    // And with no cable the line says the cable is not there, since a
    // missing line says nothing about which of the two is missing.
    let out = cadr().args(["--micro", "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(t.contains("chaosnet over udp: disabled"), "{t}");
}

/// **The flags of the Chaosnet server muir used to carry are refused, and
/// the refusal says where the host went.** muir has no file or time server
/// in it any more --- a CADR had none --- so `--chaos-file-root`,
/// `--chaos-file-peers` and `--debuggee-chaos-file-root` configure
/// nothing. A run whose `.cadrrc` still names one would otherwise boot
/// quietly to the cold-load debugger asking for the date, so it is stopped
/// with the answer in the first line: the host is another program on the
/// network, `ozd`, named with `--chaos-udp-peer`.
#[test]
fn the_file_server_flags_are_gone_and_say_where_the_host_went() {
    for flag in ["--chaos-file-root", "--chaos-file-peers", "--debuggee-chaos-file-root"] {
        let out = cadr().args([flag, ".", "--stop-after", "1"]).run();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{flag}: not a usage error:\n{err}");
        let first = err.lines().find(|l| l.starts_with("cadr: ")).unwrap_or("");
        assert!(first.contains(flag), "{flag}: the refusal names it:\n{err}");
        assert!(first.contains("--chaos-udp-peer"), "{flag}: and what replaced it:\n{err}");
        assert!(first.contains("ozd"), "{flag}: and which host that is:\n{err}");
    }
}

/// **`--chaos-address` takes one address.** The second half was the
/// Chaosnet server's, and there is no Chaosnet server in muir to give an
/// address to; a run that still writes the old pair is refused rather than
/// left to find out at the cold-load debugger.
#[test]
fn the_chaos_address_is_one_address() {
    for arg in ["3050,3060", "4401,4403", "3050,"] {
        refused("cadr", &["--chaos-address", arg, "--stop-after", "1"], "--chaos-address");
    }
    // And what is refused says where the host is named instead.
    let out = cadr().args(["--chaos-address", "3050,3060", "--stop-after", "1"]).run();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.lines()
            .find(|l| l.starts_with("cadr: "))
            .is_some_and(|l| l.contains("--chaos-udp-peer")),
        "{err}"
    );
    // One address is taken, in octal or subnet:host.
    for arg in ["3050", "6:50"] {
        let out = cadr()
            .args(["--micro", "--stop-after", "1", "--chaos-address", arg])
            .args(["--chaos-udp", "127.0.0.1:0"])
            .run();
        let t = text(&out);
        assert!(out.status.success(), "{arg}:\n{t}");
        assert!(t.contains("chaosnet: 3050"), "{arg}: the same number either way:\n{t}");
    }
}

/// **`--chaos-udp` is the cable, and without it muir sends nothing.**
///
/// The two are different things on the board and are different flags
/// here. `--chaos-address` is the sixteen address switches on the I/O
/// board, which a machine has set whether or not anything is plugged
/// into it; `--chaos-udp` is the cable, and a machine with no cable
/// talks to nobody however its switches are set. So an address alone
/// opens no socket, which also lets many runs go at once --- this file
/// starts dozens.
#[test]
fn the_cable_is_chaos_udp_and_an_address_alone_is_no_cable() {
    let line = |args: &[&str]| {
        let out = cadr().args(args).run();
        let t = text(&out);
        assert!(out.status.success(), "{args:?}:\n{t}");
        t
    };
    // The switches, and no cable.
    for args in [
        &["--micro", "--stop-after", "1"][..],
        &["--micro", "--stop-after", "1", "--chaos-address", "3050"][..],
    ] {
        let t = line(args);
        assert!(
            t.contains("chaosnet over udp: disabled"),
            "{args:?} binds nothing, and says so:\n{t}"
        );
        assert!(t.contains("--chaos-udp is the cable"), "{args:?} names the flag:\n{t}");
    }
    // The switches are still the switches.
    let t = line(&["--micro", "--stop-after", "1", "--chaos-address", "3050"]);
    assert!(t.contains("chaosnet: 3050"), "the address is the run's:\n{t}");

    // The cable, plugged in where it is told.
    let t = line(&["--micro", "--stop-after", "1", "--chaos-udp", "127.0.0.1:0"]);
    assert!(t.contains("chaosnet over udp: 127.0.0.1:"), "{t}");

    // And the flags that describe what is on the cable want the cable.
    for extra in [
        &["--chaos-udp-peer", "3060@127.0.0.1:42043"][..],
        &["--chaos-udp-default-peer", "127.0.0.1:42043"][..],
    ] {
        let mut args = vec!["--micro", "--stop-after", "1"];
        args.extend_from_slice(extra);
        refused("cadr", &args, "--chaos-udp");
    }
}

/// **`--keyboard-boot` takes the keys the boot sequence needs in four
/// spellings, says which in the setup, and refuses the rest by naming
/// the four.**
#[test]
fn keyboard_boot_takes_four_spellings_and_names_them_to_the_rest() {
    let out = cadr()
        .args(["--micro", "--keyboard-boot", "meta,meta,ctrl,ctrl", "--stop-after", "10"])
        .run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(
        t.contains("boot sequence ctrl,ctrl,meta,meta with Rubout or Return"),
        "the setup says what the run's boot sequence needs, spelled its own way:\n{t}"
    );
    let out = cadr().args(["--micro", "--stop-after", "10"]).run();
    let t = text(&out);
    assert!(t.contains("boot sequence ctrl,meta with Rubout or Return"), "the default:\n{t}");
    for bad in ["ctrl,alt,delete", "ctrl", "ctrl,ctrl,ctrl,meta", "control,meta"] {
        refused_saying("cadr", &["--keyboard-boot", bad], "ctrl,ctrl,meta,meta");
        refused("cadr", &["--keyboard-boot", bad], "--keyboard-boot");
    }
    refused("cadr", &["--keyboard-boot"], "--keyboard-boot");
}

/// A viewer of our own: RFC 6143's opening exchange as far as `ServerInit`,
/// whose first four bytes are the screen's width and height.  The idiom is
/// `tests/muir_lashup.rs`'s, which reads the lashup's two displays the same
/// way.
fn rfb_screen(addr: &str) -> (u16, u16) {
    use std::io::Read;
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut s = loop {
        match TcpStream::connect(addr) {
            Ok(s) => break s,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => panic!("{addr}: {e}"),
        }
    };
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut version = [0u8; 12];
    s.read_exact(&mut version).unwrap();
    assert_eq!(&version, b"RFB 003.008\n", "{addr}: the version offered");
    s.write_all(&version).unwrap();
    let mut types = [0u8; 2];
    s.read_exact(&mut types).unwrap();
    assert_eq!(types, [1, 1], "{addr}: one security type, None");
    s.write_all(&[1]).unwrap();
    let mut result = [0u8; 4];
    s.read_exact(&mut result).unwrap();
    assert_eq!(result, [0; 4], "{addr}: SecurityResult ok");
    s.write_all(&[1]).unwrap();
    let mut init = [0u8; 24];
    s.read_exact(&mut init).unwrap();
    (u16::from_be_bytes([init[0], init[1]]), u16::from_be_bytes([init[2], init[3]]))
}

/// **`--color-tv` fits the second display board on every engine**, and the
/// start says the board and where its screen is served.  Which board it is
/// on each engine is
/// [`the_color_tv_is_a_netlist_on_chip_and_a_model_elsewhere`]; here it is
/// that the flag is obeyed rather than warned away, and that a run that
/// did not ask has one screen.
#[test]
fn the_color_tv_is_fitted_on_every_engine() {
    for engine in ["--micro", "--rtl"] {
        let out = cadr().args([engine, "--color-tv", "--stop-after", "1"]).run();
        let t = text(&out);
        assert!(out.status.success(), "{engine}:\n{t}");
        assert!(t.contains("color tv: model lispm-tv at 17200000"), "{engine}: the board:\n{t}");
        assert!(t.contains("color terminal: vnc://"), "{engine}: and its screen:\n{t}");
        assert!(t.contains("pixels only"), "{engine}: which has no keyboard:\n{t}");
        assert!(!t.contains("warning: --color-tv"), "{engine}: and is not ignored:\n{t}");
    }
    let out = cadr()
        .args(["--chip", "--main-memory-boards", "4", "--tv", "model"])
        .args(["--color-tv", "--stop-after", "1"])
        .run();
    let t = text(&out);
    assert!(out.status.success(), "chip:\n{t}");
    assert!(t.contains("color tv: netlist lispm-tv"), "chip has the board itself:\n{t}");
    assert!(t.contains("color TV netlist"), "and says so beside the other boards:\n{t}");

    // Off unless it is asked for: a CADR has one screen unless somebody
    // plugged a second board in.
    let out = cadr().args(["--rtl", "--stop-after", "1"]).run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(!t.contains("color tv:"), "no second board unasked:\n{t}");
}

/// **The color screen is a second RFB server of 576 by 454**, at the
/// display above the main one or where `--color-terminal` says; and the
/// flag without the board is refused, as is the main screen's endpoint.
#[test]
fn the_color_terminal_serves_the_color_screen() {
    refused("cadr", &["--rtl", "--color-terminal", "--stop-after", "1"], "--color-terminal");

    // Both displays on ports the host picks, each said on stderr as it is
    // bound.  The run is killed once the color screen has answered.
    let run = cadr()
        .args(["--rtl", "--color-tv", "--terminal", "127.0.0.1:0"])
        .args(["--color-terminal", "127.0.0.1:0", "--stop-after", "400000000"])
        .start();
    let endpoint = |t: &str| {
        t.lines()
            .find_map(|l| l.trim().strip_prefix("color terminal: vnc://"))
            .map(|rest| rest.split([' ', ';']).next().unwrap().to_string())
    };
    run.stderr().wait_until(|t| endpoint(t).is_some(), "the color screen said where it is");
    let said = run.stderr().so_far();
    let at = endpoint(&said).unwrap();
    let main = said
        .lines()
        .find_map(|l| l.trim().strip_prefix("terminal: vnc://"))
        .map(|rest| rest.split(' ').next().unwrap().to_string())
        .unwrap();
    assert_ne!(at, main, "two screens, two ports:\n{said}");
    let screen = rfb_screen(&at);
    run.kill();
    assert_eq!(
        screen,
        (muir::tv::COLOR_WIDTH as u16, muir::tv::COLOR_HEIGHT as u16),
        "COLOR:MAKE-SCREEN's 576 by 454"
    );
}

/// **`--color-tv-capture` records the color screen, and it needs the
/// board.** Its own GIF, 576 by 454 with the clocks below it;
/// `--tv-capture-no-time` is both recordings' and leaves this one 454
/// high; and an end of the debug cable is refused it as it is refused
/// `--tv-capture`, the two machines being two clocks.
#[test]
fn the_color_tv_capture_records_the_color_screen() {
    let dir = scratch("color-capture");
    let gif = dir.join("color.gif");
    let path = gif.to_str().unwrap();
    // The board is what there is to record: the refusal names both flags.
    refused(
        "cadr",
        &["--rtl", "--color-tv-capture", path, "--stop-after", "1"],
        "--color-tv-capture",
    );
    refused_saying(
        "cadr",
        &["--rtl", "--color-tv-capture", path, "--stop-after", "1"],
        "it needs --color-tv",
    );

    let size = |out: &std::process::Output, gif: &Path| {
        let t = text(out);
        assert!(out.status.success(), "muir failed:\n{t}");
        let bytes = std::fs::read(gif)
            .unwrap_or_else(|e| panic!("the recording {}: {e}\n{t}", gif.display()));
        assert!(bytes.starts_with(b"GIF89a"), "the recording is a GIF:\n{t}");
        assert!(
            t.contains(&format!("color capture: {}", gif.display())),
            "the start says where it goes:\n{t}"
        );
        assert!(
            t.contains(&format!("frames of the color screen at {}", gif.display())),
            "and the stop says what it wrote:\n{t}"
        );
        (
            u16::from_le_bytes([bytes[6], bytes[7]]) as usize,
            u16::from_le_bytes([bytes[8], bytes[9]]) as usize,
        )
    };

    let out = cadr()
        .args(["--micro", "--color-tv", "--color-tv-capture", path])
        .args(["--stop-after", "200"])
        .run();
    assert_eq!(
        size(&out, &gif),
        (muir::tv::COLOR_WIDTH, muir::tv::COLOR_HEIGHT + muir::capture::TIME_H),
        "576 by 454 and the clock line below it"
    );

    // The one flag turns the clocks off, on this recording as on the main
    // screen's: there is no second one for it.
    let bare = dir.join("no-clocks.gif");
    let out = cadr()
        .args(["--micro", "--color-tv", "--color-tv-capture", bare.to_str().unwrap()])
        .args(["--tv-capture-no-time", "--stop-after", "200"])
        .run();
    assert_eq!(
        size(&out, &bare),
        (muir::tv::COLOR_WIDTH, muir::tv::COLOR_HEIGHT),
        "the picture alone"
    );

    // A machine on its own, as `--tv-capture` is: over the cable the two
    // machines are two clocks.
    let t = text(
        &cadr_default()
            .args(["--rtl", "--color-tv", "--color-tv-capture", path])
            .args(["--stop-after", "1"])
            .run(),
    );
    assert!(t.contains("debug cable: none --- --color-tv-capture"), "said:\n{t}");
    refused(
        "cadr",
        &[
            "--rtl",
            "--color-tv",
            "--color-tv-capture",
            path,
            "--debug-cable-listen",
            "127.0.0.1:0",
            "--stop-after",
            "1",
        ],
        "--color-tv-capture",
    );
}

/// **`--pace` is one machine at its own speed, and a lashup is refused
/// it.** Two machines on a cable already pace each other through the
/// cable's own clock: each runs as far as the other has promised and then
/// waits for it. An end that also slept on its own clock would hold the
/// other up at its next request, and neither end would be at the machine's
/// speed for it. All three shapes of cable run refuse the flag rather than
/// taking it and quietly doing something else with it.
#[test]
fn the_pace_is_one_machine_at_its_own_speed() {
    for args in [
        &["--rtl", "--pace", "--debug-in-process", "--stop-after", "1"][..],
        &["--rtl", "--pace", "--debug-cable-listen", "127.0.0.1:0", "--stop-after", "1"][..],
        &["--rtl", "--pace", "--debug-cable-connect", "127.0.0.1:65500", "--stop-after", "1"][..],
    ] {
        refused("cadr", args, "--pace");
    }
    // The machine on its own takes it, on the engines that can outrun the
    // hardware and on the one that cannot.
    for engine in ["--micro", "--rtl"] {
        let out = cadr().args([engine, "--pace", "--stop-after", "1"]).run();
        assert!(out.status.success(), "{engine} --pace:\n{}", text(&out));
    }
    // Not asked for, the pace `rtl` takes by default is left off a lashup
    // rather than refused: nobody gave the flag the refusal would be about.
    for args in [
        &["--rtl", "--debug-in-process", "--no-debug-cable-listen", "--stop-after", "1"][..],
        &["--rtl", "--debug-cable-listen", "127.0.0.1:0", "--stop-after", "1"][..],
    ] {
        let out = cadr_default().args(args).run();
        let t = text(&out);
        assert!(out.status.success(), "{args:?}:\n{t}");
        assert!(!t.contains("pace:"), "{args:?} is not paced:\n{t}");
    }
}
