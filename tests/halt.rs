// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `HALT-CONS`, misc function 1, and what the CADR does with it.
//!
//! MIT's assembler defines it as `1_10.` in `sys/sys/cadsym.lisp`, beside
//! `WRITE-DISPATCH-RAM` at `2_10.` and `INSTRUCTION-STREAM` at `3_10.`, so
//! `IR<11:10>` is the misc function on every instruction class, and
//! microcode 323 writes it at `ZERO`, `ILLOP` and `%HALT`. On the board,
//! page SOURCE decodes the field through the 74S139 at 3D05 into
//! `-FUNCT0..3`; `-FUNCT1` leaves the processor board on connector pin
//! 3CJ1-19 and is `-HALT` on the ICMEM board at 1CJ2-21 (`cadrwd/cadr4.wlr`
//! and `icmem3.wlr`), the D input of the 74S374 at OLORD2 1A05 whose Q is
//! `-HALTED`, one of the inputs of the 74S133 at 1A02 that makes `ERR`.
//! `ERR` shows in `FLAG-1` bit 10, and under `ERRSTOP` in the mode register
//! it drops `MACHRUN`: the machine halts after the instruction, and runs on
//! without `ERRSTOP`. `micro` and `rtl` are held to that here.

use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{JUMP, filler};
use muir::machine::Machine;
use muir::micro::Micro;
use muir::rtl::Rtl;
use muir::spy;

/// `HALT-CONS`, `1_10.`.
const HALT_CONS: u64 = 1 << 10;
/// `ERR` in `FLAG-1`, bit 10.
const ERR: u16 = 1 << 10;

/// A straight-line program whose PC climbs one a microcycle, with `insn`
/// carrying `HALT-CONS` at address 5.
fn program(insn: Insn) -> Machine {
    let mut m = Machine::new();
    let mut prom = vec![filler(); 512];
    prom[5] = Insn::new(insn.raw() | HALT_CONS);
    m.load_prom(&prom);
    m
}

/// Boots `e`, sets `ERRSTOP` as asked --- after the boot, which resets the
/// console's registers --- and runs forty microcycles.
fn run(e: &mut impl Engine, errstop: bool) {
    e.machine_mut().mode.errstop = errstop;
    for _ in 0..40 {
        e.step().expect("no halt this engine raises");
    }
}

/// **Under `ERRSTOP`, `HALT-CONS` halts the machine after the
/// instruction, on either engine and on either class.**
#[test]
fn halt_cons_halts_under_errstop() {
    for (what, insn) in
        [("an ALU instruction", filler()), ("a JUMP to itself", Insn::new(JUMP | 5 << 12))]
    {
        let mut r = Rtl::new(program(insn));
        r.boot();
        run(&mut r, true);
        assert!(r.pc() <= 7, "rtl, {what}: halted, PC {:o}", r.pc());
        assert_ne!(r.spy_read(spy::FLAG_1) & ERR, 0, "rtl, {what}: ERR up");
        assert!(r.halted_ns() > 0, "rtl, {what}: the cpu clock held off");

        let mut e = Micro::new(program(insn));
        e.boot();
        run(&mut e, true);
        assert!(e.pc() <= 7, "micro, {what}: halted, PC {:o}", e.pc());
        assert_ne!(e.spy_read(spy::FLAG_1) & ERR, 0, "micro, {what}: ERR up");
    }
}

/// **Without `ERRSTOP`, `HALT-CONS` does nothing**: `HALTED` sets, `ERR`
/// is up for the microcycle, and the machine runs on.
#[test]
fn halt_cons_runs_on_without_errstop() {
    let mut r = Rtl::new(program(filler()));
    r.boot();
    run(&mut r, false);
    assert!(r.pc() > 20, "rtl ran on, PC {:o}", r.pc());
    assert_eq!(r.spy_read(spy::FLAG_1) & ERR, 0, "and ERR is down again");

    let mut e = Micro::new(program(filler()));
    e.boot();
    run(&mut e, false);
    assert!(e.pc() > 20, "micro ran on, PC {:o}", e.pc());
    assert_eq!(e.spy_read(spy::FLAG_1) & ERR, 0, "and ERR is down again");
}

/// **A machine stopped by `HALT-CONS` under `ERRSTOP` stays stopped**, and
/// says so in `FLAG-1` where a console reads it: `SRUN` still set, so this
/// is not the halt a cleared `RUN` is, with `ERR` up and no `WAIT`.
///
/// This is what `(si:%halt)` does in System 100, and it is why the run loop
/// has to watch `FLAG-1` rather than step on: [`Engine::step`] goes on
/// returning `Ok` for as long as it is called, advancing the master clock
/// and running no microcycle, so nothing about the call says the machine
/// has stopped.
fn stays_halted<E: Engine>(what: &str, e: &mut E) {
    e.boot();
    run(e, true);
    let pc = e.pc();
    let f = spy::Flag1::of(e.spy_read(spy::FLAG_1));
    assert!(f.srun, "{what}: RUN is still set, so a cleared RUN is not what stopped it");
    assert!(f.err, "{what}: ERR is up");
    assert!(!f.wait, "{what}: and it is not a wait on the bus, which it would come out of");
    for _ in 0..10_000 {
        e.step().expect("no halt this engine raises");
    }
    assert_eq!(e.pc(), pc, "{what}: stepped 10,000 times more and never moved");
}

#[test]
fn an_errstop_halt_stays_halted_and_shows_in_flag_1() {
    stays_halted("rtl", &mut Rtl::new(program(filler())));
    stays_halted("micro", &mut Micro::new(program(filler())));
}

/// **`MACHRUN` is made where the run loop thinks it is**, which is what
/// the `chip` engine's reading of a self-halt rests on.
///
/// `Chip` is not an [`Engine`] and has no spy registers to read, so
/// `time_chip` goes to the nets instead: `SRUN`, `-ERRHALT` and
/// `-STATHALT` on the 9S42 at OLORD1 1A15, leaving out `-WAIT`, which is a
/// stall the machine comes out of by itself. That only works while those
/// are the names and the wiring, so this holds them --- a rename or a
/// rewire fails here rather than quietly making the engine blind to a
/// halt it can no longer see.
#[test]
fn machrun_is_made_from_the_nets_the_chip_engine_reads() {
    let n = muir::netlist::parse(include_str!("../data/CADR.netlist")).unwrap();
    // A quad gate is drawn as more than one group under the one
    // designator, so these are found by the net they carry and then held
    // to where they are, rather than found by position.
    let pin = |p: &muir::netlist::Part, want: u8| {
        p.pins.iter().find(|(x, _)| *x == want).map(|&(_, id)| n.net(id).to_string())
    };
    let carries =
        |p: &muir::netlist::Part, net: &str| p.pins.iter().any(|&(_, id)| n.net(id) == net);

    // `-ERRHALT` is `NAND(ERRSTOP, ERR)`: low exactly when an error is up
    // and `ERRSTOP` says to stop for one.
    let nand = n
        .parts
        .iter()
        .find(|p| p.kind == "74S00" && carries(p, "-ERRHALT"))
        .expect("the NAND that makes -ERRHALT");
    assert_eq!((nand.page.as_str(), nand.reference.as_str()), ("OLORD2", "1C09"));
    assert_eq!(pin(nand, 11).as_deref(), Some("-ERRHALT"), "its output");
    let mut ins = [pin(nand, 12), pin(nand, 13)];
    ins.sort();
    assert_eq!(ins, [Some("ERR".into()), Some("ERRSTOP".into())], "its two inputs");

    // And the 9S42 that makes `MACHRUN` from six.
    let gate = n
        .parts
        .iter()
        .find(|p| p.kind.starts_with("9S42") && pin(p, 7).as_deref() == Some("MACHRUN"))
        .expect("the gate that makes MACHRUN");
    assert_eq!((gate.page.as_str(), gate.reference.as_str()), ("OLORD1", "1A15"));
    let ins: Vec<String> = (1..=6).map(|k| pin(gate, k).expect("an input")).collect();
    assert_eq!(ins, ["SSTEP", "-SSDONE", "SRUN", "-ERRHALT", "-WAIT", "-STATHALT"]);
}

// --- QUUX revision 16 --------------------------------------------------------

mod support;

/// Where [`halting_prom_16`]'s `HALT-CONS` stands: the PROM's words start at
/// 36000 on revision 16, and the word is the 44th.
const HALT_AT_16: u64 = 0o36000 + 43;

/// **Revision 16's boot PROM that halts itself**: it builds the register
/// page's word 102, error stop (contract Q2), in M 1 a bit at a time, writes
/// it with `<0>` set, waits for the write, and runs `HALT-CONS` at
/// [`HALT_AT_16`]; the word after it jumps to itself. A revision-15 `.mcr`,
/// section 6 saying 15, as revision 16 loads.
fn halting_prom_16(dir: &std::path::Path) -> std::path::PathBuf {
    use muir::isa::asm::{ALU, ALWAYS, CARRY_IN, M_PLUS_M, MD, N, SETM, SETO, SETZ, START_WRITE};
    use muir::isa::asm::{m_dest, m_src, target};
    use muir::mcr;
    let errstop = u64::from(muir::tlb::REGISTER_PAGE | 0o102);
    let mut words = vec![ALU | SETZ | m_dest(1)];
    for b in (0..32).rev() {
        let carry = if errstop >> b & 1 != 0 { CARRY_IN } else { 0 };
        words.push(ALU | M_PLUS_M | carry | m_src(1) | m_dest(1));
    }
    words.push(ALU | SETO | MD);
    words.push(ALU | SETM | m_src(1) | START_WRITE);
    words.extend([filler().raw(); 8]);
    assert_eq!(0o36000 + words.len() as u64, HALT_AT_16);
    words.push(filler().raw() | HALT_CONS);
    words.push(JUMP | target(HALT_AT_16 + 1) | ALWAYS | N);
    words.push(filler().raw());
    let mut w = vec![mcr::FORMAT_WORD, 2];
    w.extend_from_slice(&[6, 1, 32, 32, 0, 0, 0, 0, 15]);
    w.extend_from_slice(&[1, words.len() as u32, 64, 64, 0o36000, 0, 0, 0]);
    for x in &words {
        w.extend_from_slice(&[*x as u32, (*x >> 32) as u32]);
    }
    let mut b: Vec<u8> = w.iter().flat_map(|x| x.to_le_bytes()).collect();
    b.resize(b.len().div_ceil(mcr::BLOCK_BYTES) * mcr::BLOCK_BYTES, 0);
    let path = dir.join("halting-16.mcr");
    std::fs::write(&path, b).unwrap();
    path
}

/// What one engine's run of [`halting_prom_16`] said: the halt's message
/// and the `PC` line after it, two answers of `pc` at the prompt taken half
/// a second apart, the quit's line, and how the run ended.
struct Halted16 {
    message: String,
    halt_pc: String,
    answers: [String; 2],
    quit: String,
    status: std::process::ExitStatus,
}

/// Runs `engine` on revision 16 with [`halting_prom_16`] and the prompt on
/// a pipe, waits for the halt's message, asks `pc` twice, and quits.
fn halted_16(engine: &str, prom: &std::path::Path) -> Halted16 {
    use std::io::Write;
    use support::{Run, quux, text};
    const SAID: &str = "the machine stopped itself: ";
    let mut child = quux()
        .env("MUIR_QUUX_REVISION", "16")
        .args([&format!("--{engine}"), "--main-memory-size", "2MW", "--stop-after", "2000000"])
        .args(["--terminal", "127.0.0.1:0", "--prom"])
        .arg(prom)
        .stdin(std::process::Stdio::piped())
        .start();
    let mut stdin = child.stdin();
    let came = child.stdout().wait_for(|t| t.contains(SAID), std::time::Duration::from_secs(60));
    let lines = |t: &str, k: usize| -> Vec<String> {
        t.lines().filter(|l| l.contains("this run")).map(str::to_string).skip(k).collect()
    };
    let mut answers = [String::new(), String::new()];
    if came {
        for (k, answer) in answers.iter_mut().enumerate() {
            if k == 1 {
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            writeln!(stdin, "pc").ok();
            // The halt's own `PC` line is the first; each answer one more.
            let want = k + 2;
            child
                .stdout()
                .wait_for(|t| lines(t, 0).len() >= want, std::time::Duration::from_secs(30));
            *answer = lines(&child.stdout().so_far(), k + 1).first().cloned().unwrap_or_default();
        }
    }
    writeln!(stdin, "q").ok();
    drop(stdin);
    let out = child.wait();
    let t = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(came, "{engine}: the halt was never said:\n{}", text(&out));
    let message = t.lines().find(|l| l.contains(SAID)).unwrap_or_default().to_string();
    let halt_pc = lines(&t, 0).first().cloned().unwrap_or_default();
    let quit = t.lines().find(|l| l.contains("quit at PC")).unwrap_or_default().trim().to_string();
    Halted16 { message, halt_pc, answers, quit, status: out.status }
}

/// `PC <octal>` and the machine's microcycles from a `pc` line, `PC 36055
/// in the PROM; 61 microcycles, 610 ns; 63 this run`: what two engines
/// agree on, the time and the run's turns being each engine's own.
fn pc_and_cycles(line: &str) -> (String, String) {
    let pc = line.split(';').next().unwrap_or_default().to_string();
    let cycles = line.split("; ").nth(1).and_then(|s| s.split(',').next()).unwrap_or_default();
    (pc, cycles.to_string())
}

/// The run's turns from a `pc` line: `63` of `...; 63 this run`.
fn this_run(line: &str) -> u64 {
    let n = line.rsplit("; ").next().and_then(|s| s.strip_suffix(" this run"));
    n.and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("no run's count in {line:?}"))
}

/// **On revision 16, `rtl` reports a `HALT-CONS` halt as `micro` does, and
/// stops there.** Under `ERRSTOP` the pipeline drains after the word and
/// stands with `RUN` still set and `ERR` up, which is the halt the run
/// loop reads off `FLAG-1` (contract G3 revision 15, A15b.13: the halt
/// drains; the CADR's `-HALTED` and `ERR`, above). Both engines say the
/// machine stopped itself and why, at the word after the halt and the same
/// microcycle; the run's count stands still while the prompt answers `pc`;
/// and `q` ends both alike. **Fails** an `rtl` that clears `RUN` at the
/// drain's end, which neither says the halt nor holds, and runs its count
/// on to `--stop-after`.
#[test]
fn rtl_on_revision_16_reports_a_halt_cons_halt_as_micro_does() {
    let dir = support::scratch("halt-cons-16");
    let prom = halting_prom_16(&dir);
    let micro = halted_16("micro", &prom);
    let rtl = halted_16("rtl", &prom);
    // The halt is after the word: the word after it is in `IR` and `PC` is
    // the one fetched next, two past the halt.
    let at = format!("PC {:o} in the PROM;", HALT_AT_16 + 2);
    for (engine, h) in [("micro", &micro), ("rtl", &rtl)] {
        assert!(h.message.contains("ERR is up under ERRSTOP"), "{engine}: {:?}", h.message);
        assert!(h.halt_pc.starts_with(&at), "{engine}: halted at {at}: {:?}", h.halt_pc);
        for a in &h.answers {
            assert!(a.starts_with(&at), "{engine}: the prompt answers pc: {:?}", h.answers);
        }
        let counts = [this_run(&h.halt_pc), this_run(&h.answers[0]), this_run(&h.answers[1])];
        assert!(counts.iter().all(|&c| c == counts[0]), "{engine}: the count runs on: {counts:?}");
        assert!(h.quit.starts_with("quit at PC"), "{engine}: quit: {:?}", h.quit);
    }
    assert_eq!(rtl.message, micro.message, "the same message");
    assert_eq!(pc_and_cycles(&rtl.halt_pc), pc_and_cycles(&micro.halt_pc), "the same halt");
    assert_eq!(rtl.quit, micro.quit, "the same quit");
    assert_eq!(rtl.status.code(), micro.status.code(), "the same exit status");
    assert!(micro.status.success(), "a quit ends the run well");
}
