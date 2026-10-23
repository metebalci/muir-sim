// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **A halted machine runs on at `continue`, without a reset.** A board's
//! checkpoint is taken with the machine halted, its `RUN` clear; the
//! prompt's `continue`, and `--continue` at the start, set `RUN` as a
//! console does and the machine runs on from where it stood, while `boot`
//! still presses the button. The boot PROM alone is run, so nothing here
//! needs `vendor/`.

mod support;

use std::io::Write;
use std::path::Path;
use std::process::Stdio;

use muir::block_disk::BlockDisk;
use muir::checkpoint::{Reader, Writer};
use muir::engine::Engine;
use muir::machine::{Geometry, Machine};
use muir::micro::Micro;
use muir::pipeline::Pipeline;
use muir::rtl::Rtl;
use muir::spy;
use support::{Run, cadr, scratch, text};

/// The executable for `exe`, `cadr` or `quux` on revision 16.
fn executable(exe: &str) -> std::process::Command {
    match exe {
        "quux" => support::quux_on_16(),
        _ => support::executable(exe),
    }
}

/// [`executable`] for a run from the PROM: on QUUX a PROM of its own whose
/// loop keeps the PC moving, as revision 16 has no PROM built in.
fn starting(exe: &str, dir: &Path) -> std::process::Command {
    let mut c = executable(exe);
    if exe == "quux" {
        c.arg("--prom").arg(support::loop_prom_16(dir));
    }
    c
}

/// Where a checkpoint's machine stands: its PC, its microcycles, and
/// whether `RUN` is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stands {
    pc: u16,
    cycles: u64,
    run: bool,
}

fn stands<E: Engine>(e: &E) -> Stands {
    Stands { pc: e.pc(), cycles: e.machine().cycles, run: e.machine().clock_control.run }
}

/// What [`halted_copy`] does to an engine, whichever it is.
trait Halting {
    fn halt(&mut self) -> Stands;
}

impl<E: Engine> Halting for E {
    /// The console stops the machine as a board's checkpoint tool does,
    /// `RUN` cleared through the clock control register (CC's
    /// `CC-STOP-MACH`, `SPY-WRITE SPY-CLK 0`), and the master clock run on
    /// a few edges so that `SRUN` has followed it, the pipeline drained.
    fn halt(&mut self) -> Stands {
        self.spy_write(spy::CLK, 0);
        for _ in 0..8 {
            self.step().unwrap();
        }
        self.settle_for_checkpoint().unwrap();
        let f = spy::Flag1::of(self.spy_read(spy::FLAG_1));
        assert!(!f.srun, "SRUN has followed RUN down");
        stands(self)
    }
}

/// A machine for the checkpoint `c` to load onto, as `--resume` builds
/// one: its geometry, its memory, and on QUUX the block disk.
fn machine_for(c: &muir::checkpoint::Checkpoint) -> Machine {
    let geometry = Machine::checkpointed_geometry_at(&c.body, c.word_bits).unwrap();
    let mut m = Machine::with_geometry(geometry, c.memory_boards);
    if geometry != Geometry::CADR {
        m.block_disk = Some(BlockDisk::new(muir::block_disk::BLOCK_NS));
    }
    m
}

/// Reads the checkpoint at `from`, halts its machine and writes it to
/// `to`: a checkpoint as a board takes one. Says where the machine stood.
fn halted_copy(from: &Path, to: &Path) -> Stands {
    let c = muir::checkpoint::read(from).unwrap();
    let m = machine_for(&c);
    let mut w = Writer::new();
    let at = match c.engine.as_str() {
        "micro" => {
            let mut e = Micro::new(m);
            e.load(&mut c.reader()).unwrap();
            let at = e.halt();
            e.save(&mut w);
            at
        }
        "rtl" if m.geometry.paged() => {
            let mut e = Pipeline::new(m);
            e.load(&mut c.reader()).unwrap();
            let at = e.halt();
            e.save(&mut w);
            at
        }
        "rtl" => {
            let mut e = Rtl::new(m);
            e.load(&mut c.reader()).unwrap();
            let at = e.halt();
            e.save(&mut w);
            at
        }
        other => panic!("{other} is no engine this test halts"),
    };
    assert!(!at.run, "the copy is halted");
    muir::checkpoint::write_version(to, &c.engine, c.memory_boards, c.version, &w.finish())
        .unwrap();
    at
}

/// Where the machine in the checkpoint at `path` stands.
fn stands_in(path: &Path) -> Stands {
    let c = muir::checkpoint::read(path).unwrap();
    let m = machine_for(&c);
    let mut r: Reader<'_> = c.reader();
    match c.engine.as_str() {
        "micro" => {
            let mut e = Micro::new(m);
            e.load(&mut r).unwrap();
            stands(&e)
        }
        "rtl" if m.geometry.paged() => {
            let mut e = Pipeline::new(m);
            e.load(&mut r).unwrap();
            stands(&e)
        }
        "rtl" => {
            let mut e = Rtl::new(m);
            e.load(&mut r).unwrap();
            stands(&e)
        }
        other => panic!("{other} is no engine this test reads"),
    }
}

/// Whether `exe`'s `engine` is the pipeline: QUUX's `rtl`.
fn pipeline(exe: &str, engine: &str) -> bool {
    exe == "quux" && engine == "--rtl"
}

/// Every machine and engine that resumes a `micro` or `rtl` checkpoint.
const RUNS: [(&str, &str); 4] =
    [("cadr", "--micro"), ("cadr", "--rtl"), ("quux", "--micro"), ("quux", "--rtl")];

/// A run of 3000 microcycles from the PROM, checkpointed and halted: the
/// halted checkpoint's path and where its machine stands.
fn a_halted_checkpoint(dir: &Path, exe: &str, engine: &str) -> (std::path::PathBuf, Stands) {
    let ran = dir.join(format!("{exe}{engine}-ran.chk"));
    let out =
        starting(exe, dir).args([engine, "--stop-after", "3000", "--checkpoint"]).arg(&ran).run();
    assert!(out.status.success(), "{exe} {engine}:\n{}", text(&out));
    let halted = dir.join(format!("{exe}{engine}-halted.chk"));
    let at = halted_copy(&ran, &halted);
    (halted, at)
}

/// **`continue` runs a halted checkpoint on from where it stood**: `RUN`
/// is set, the microcycles carry on from the checkpoint's count rather
/// than standing still, the PC moves, and nothing pressed the button ---
/// the run says it set `RUN`, not that it booted.
#[test]
fn continue_runs_a_halted_checkpoint_on() {
    let dir = scratch("continue");
    for (exe, engine) in RUNS {
        let (halted, at) = a_halted_checkpoint(&dir, exe, engine);
        let after = dir.join(format!("{exe}{engine}-after.chk"));
        let mut child = executable(exe)
            .args([engine, "--stop-after", "2000000", "--checkpoint"])
            .arg(&after)
            .arg("--resume")
            .arg(&halted)
            .stdin(Stdio::piped())
            .start();
        let mut stdin = child.stdin();
        write!(stdin, "continue\nstep 1000\n").unwrap();
        drop(stdin);
        let out = child.wait();
        let t = text(&out);
        assert!(out.status.success(), "{exe} {engine}:\n{t}");
        assert!(t.contains("continue: RUN set"), "{exe} {engine}: continue says so:\n{t}");
        assert!(!t.contains("the machine is halted"), "{exe} {engine}: nothing refused:\n{t}");
        let now = stands_in(&after);
        assert!(now.run, "{exe} {engine}: RUN is set: {now:?}");
        // All but the first master clock edge of the step, which takes
        // `RUN` into `SRUN` and runs no microcycle; the pipeline counts
        // its own, a drained checkpoint's microcycle among them.
        let want = if pipeline(exe, engine) { at.cycles + 1001 } else { at.cycles + 999 };
        assert_eq!(
            now.cycles, want,
            "{exe} {engine}: the microcycles carry on from {at:?}: {now:?}\n{t}"
        );
        assert_ne!(now.pc, at.pc, "{exe} {engine}: the PC moved from {at:?}: {now:?}");
    }
}

/// **`--continue` does at the start what the prompt's `continue` does**,
/// with no prompt: a halted checkpoint runs at once.
#[test]
fn the_continue_flag_runs_a_halted_checkpoint_at_once() {
    let dir = scratch("continue-flag");
    for (exe, engine) in RUNS {
        let (halted, at) = a_halted_checkpoint(&dir, exe, engine);
        let after = dir.join(format!("{exe}{engine}-after.chk"));
        let out = executable(exe)
            .args([engine, "--stop-after", "2000", "--checkpoint"])
            .arg(&after)
            .arg("--resume")
            .arg(&halted)
            .arg("--continue")
            .run();
        let t = text(&out);
        assert!(out.status.success(), "{exe} {engine}:\n{t}");
        assert!(t.contains("continue: RUN set"), "{exe} {engine}: the start says so:\n{t}");
        let now = stands_in(&after);
        assert!(now.run, "{exe} {engine}: {now:?}");
        // Every one but the edge that takes `RUN` into `SRUN`; on the
        // pipeline its own count, as above.
        let want = if pipeline(exe, engine) { at.cycles + 2001 } else { at.cycles + 1999 };
        assert_eq!(now.cycles, want, "{exe} {engine}: from {at:?}: {now:?}\n{t}");
        assert_ne!(now.pc, at.pc, "{exe} {engine}: the PC moved from {at:?}: {now:?}");
        // And it is where the machine that was never halted is at the same
        // microcycle: the halt and the continue cost master clock edges
        // and nothing else, no reset among them.
        let ran = dir.join(format!("{exe}{engine}-ran.chk"));
        let straight = dir.join(format!("{exe}{engine}-straight.chk"));
        let out = executable(exe)
            .args([engine, "--stop-after", "2000", "--checkpoint"])
            .arg(&straight)
            .arg("--resume")
            .arg(&ran)
            .run();
        assert!(out.status.success(), "{exe} {engine}:\n{}", text(&out));
        let never = stands_in(&straight);
        assert_eq!(never.cycles, now.cycles, "{exe} {engine}: {never:?} {now:?}");
        assert_eq!(never.pc, now.pc, "{exe} {engine}: {never:?} {now:?}");
    }
}

/// **`--continue` wants a checkpoint**: on a cold start there is nothing
/// halted for it to run on, and the button does that.
#[test]
fn the_continue_flag_wants_resume() {
    let out = cadr().args(["--micro", "--stop-after", "10", "--continue"]).run();
    assert!(!out.status.success());
    assert!(text(&out).contains("--continue wants --resume"), "{}", text(&out));
}

/// **`--continue` on a checkpoint whose machine is running does nothing**
/// but say so, and the run goes on as it would have.
#[test]
fn the_continue_flag_on_a_running_checkpoint_is_harmless() {
    let dir = scratch("continue-running");
    let ran = dir.join("ran.chk");
    let out = cadr().args(["--micro", "--stop-after", "3000", "--checkpoint"]).arg(&ran).run();
    assert!(out.status.success(), "{}", text(&out));
    let out = cadr()
        .args(["--micro", "--stop-after", "1000", "--resume"])
        .arg(&ran)
        .arg("--continue")
        .run();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("continue: the machine is running already"), "{t}");
    assert!(t.contains("ran out at 1000"), "{t}");
}

/// **`boot` still presses the button** on a halted checkpoint: the
/// machine starts again from the PROM's first word, as a CADR does.
#[test]
fn boot_on_a_halted_checkpoint_still_resets() {
    let dir = scratch("continue-boot");
    let (halted, _) = a_halted_checkpoint(&dir, "cadr", "--micro");
    let mut child = cadr()
        .args(["--micro", "--stop-after", "2000000", "--resume"])
        .arg(&halted)
        .stdin(Stdio::piped())
        .start();
    let mut stdin = child.stdin();
    write!(stdin, "boot\nhold\n").unwrap();
    drop(stdin);
    let out = child.wait();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    let pcs: Vec<&str> = t.lines().filter(|l| l.starts_with("PC ")).collect();
    assert!(pcs.first().is_some_and(|l| l.starts_with("PC 0 in the PROM")), "{t}");
    assert!(!t.contains("continue:"), "{t}");
}

/// **`continue` on a running machine does nothing and says so**: the run
/// is not held, and it goes on to its stop.
#[test]
fn continue_on_a_running_machine_is_harmless() {
    let mut child =
        cadr().args(["--micro", "--stop-after", "2000000"]).stdin(Stdio::piped()).start();
    let mut stdin = child.stdin();
    writeln!(stdin, "continue").unwrap();
    drop(stdin);
    let out = child.wait();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("continue: the machine is running already"), "{t}");
    assert!(t.contains("ran out at 2000000"), "it ran on to its stop:\n{t}");
}

/// **On `chip` too**: a netlist machine left halted by `--no-auto-boot`,
/// checkpointed so, resumed and continued, runs --- its PC moves --- where
/// before `continue` took the hold off a machine whose clock never reached
/// the datapath.
#[test]
fn continue_runs_a_halted_chip_checkpoint_on() {
    let dir = scratch("continue-chip");
    let chk = dir.join("halted.chk");
    let mut child = cadr()
        .args(["--chip", "--no-auto-boot", "--stop-after", "100"])
        .stdin(Stdio::piped())
        .start();
    let mut stdin = child.stdin();
    write!(stdin, "checkpoint {}\nquit\n", chk.display()).unwrap();
    drop(stdin);
    let out = child.wait();
    assert!(out.status.success(), "{}", text(&out));
    assert!(chk.exists(), "{}", text(&out));
    let mut child = cadr()
        .args(["--chip", "--stop-after", "100000", "--resume"])
        .arg(&chk)
        .stdin(Stdio::piped())
        .start();
    let mut stdin = child.stdin();
    write!(stdin, "hold\ncontinue\nstep 40\n").unwrap();
    drop(stdin);
    let out = child.wait();
    let t = text(&out);
    assert!(out.status.success(), "{t}");
    assert!(t.contains("continue: RUN set"), "{t}");
    let pcs: Vec<&str> = t.lines().filter(|l| l.starts_with("PC ")).collect();
    assert!(pcs.len() >= 2, "hold and step each say where the machine is:\n{t}");
    let pc = |l: &str| l.split(';').next().unwrap().to_string();
    assert_ne!(pc(pcs[0]), pc(pcs[pcs.len() - 1]), "the PC moved:\n{t}");
}
