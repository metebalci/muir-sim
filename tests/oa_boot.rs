// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! An OA-register write does not survive a boot. `IMOD<25:0>` and
//! `IMOD<47:26>`, functional destinations 16 and 17, OR the word written
//! into the next instruction as `IR` loads it (page IREG); if a boot comes
//! in the next microcycle, the instruction so modified is the one the
//! boot's trap nops, and the PROM's first word runs as it was burned.
//! `rtl` has this from the board. `micro` held the modification pending and
//! ORed it into the first instruction it executed, which after a boot is
//! the PROM's first word: the keyboard's warm boot then jumped somewhere
//! else. Both engines are held here to the board's answer, through the
//! boot button and through the console's `PROG.BOOT`, on the CADR. QUUX
//! has no IMOD: its OA registers are read only through a word's OA select,
//! and `-RESET` clears them (`reset_clears_the_oa_registers` in
//! `tests/revision_15.rs`).

use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{ALU, ALWAYS, JUMP, N, SETA, a_src, filler, target};
use muir::machine::Machine;
use muir::micro::Micro;
use muir::rtl::Rtl;
use muir::spy;

/// The word written to `OA-REG-LOW`: into a JUMP it ORs `5763` into the
/// target, `IR<25:12>`, and touches nothing else.
const OA: u32 = 0o57630000;

/// `((OA-REG-LOW) SETA A-MEM-5)`: functional destination 16 in
/// `IR<23:19>`, the M address `IR<18:14>` the unused 37, as
/// `asm::START_READ` has it.
fn oa_write() -> Insn {
    Insn::new(ALU | SETA | a_src(5) | (0o16 << 19) | (0o37 << 14))
}

/// The CADR, whose PROM jumps from its first word to its word 10 under
/// `N`, and whose word 10 jumps to `at`, 20. At `at` the OA write; after
/// it, a jump to itself. The PROM overlays the bottom of the control store
/// until `PROMDISABLE`, so the program stays in the PROM. Returns the
/// machine and `at`.
fn machine() -> (Machine, u16) {
    let mut m = Machine::new();
    let base = m.reset_pc();
    let mut prom = vec![filler(); 1024];
    prom[0] = Insn::new(JUMP | target(base as u64 + 0o10) | ALWAYS | N);
    let at = 0o20;
    prom[0o10] = Insn::new(JUMP | target(at as u64) | ALWAYS | N);
    prom[at as usize] = oa_write();
    prom[at as usize + 1] = Insn::new(JUMP | target(at as u64 + 1) | ALWAYS | N);
    m.load_prom(&prom);
    m.amem[5] = u64::from(OA);
    (m, at)
}

/// How the second boot comes.
#[derive(Clone, Copy, Debug)]
enum Boot {
    /// The button, or the keyboard's boot, which presses the same line.
    Button,
    /// The console's `PROG.BOOT`, the mode register's boot bit.
    Console,
}

/// Boots `e`, runs it until the OA write has executed, boots it again
/// the way `how` says, and returns the first `n` addresses executed from
/// the PROM's first word on. Counted from there because the console's
/// pulse is taken at a master clock edge, and on `rtl` the instruction in
/// `IR` --- the modified one --- runs before the trap does; the button's
/// trap nops it.
fn executed_after_boot<E: Engine>(
    e: &mut E,
    executed: fn(&E) -> Option<u16>,
    at: u16,
    how: Boot,
    n: usize,
) -> Vec<u16> {
    let base = e.machine().reset_pc();
    e.boot();
    let mut ran = false;
    for _ in 0..50 {
        e.step().unwrap();
        if executed(e) == Some(at) {
            ran = true;
            break;
        }
    }
    assert!(ran, "the OA write at {at:o} executed");
    match how {
        Boot::Button => e.boot(),
        Boot::Console => e.spy_write(spy::MODE, spy::MODE_BOOT),
    }
    let mut after = Vec::new();
    for _ in 0..50 {
        e.step().unwrap();
        if let Some(pc) = executed(e)
            && (pc == base || !after.is_empty())
        {
            after.push(pc);
        }
        if after.len() == n {
            break;
        }
    }
    after
}

/// **The PROM's first word runs as burned after a boot that follows an OA
/// write**, on both engines and both boot lines: its jump goes to the
/// PROM's word 10, and then to the OA write again.
#[test]
fn an_oa_write_does_not_survive_a_boot() {
    let mut wrong = Vec::new();
    {
        for how in [Boot::Button, Boot::Console] {
            let (m, at) = machine();
            let base = m.reset_pc();
            let want = vec![base, base + 0o10, at];

            let mut r = Rtl::new(m.clone());
            let got_rtl = executed_after_boot(&mut r, Rtl::executed, at, how, 3);
            let mut e = Micro::new(m);
            let got_micro = executed_after_boot(&mut e, Micro::executed, at, how, 3);

            let show = |v: &[u16]| v.iter().map(|a| format!("{a:o}")).collect::<Vec<_>>();
            for (engine, got) in [("rtl", got_rtl), ("micro", got_micro)] {
                if got != want {
                    wrong.push(format!(
                        "{how:?} {engine}: {:?}, not {:?}",
                        show(&got),
                        show(&want)
                    ));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// **The test is aimed**: at the moment of the second boot `rtl`'s `IR`
/// holds the instruction after the OA write with the OA word ORed in, so
/// the modification is pending when the boot comes.
#[test]
fn the_boot_comes_with_the_modification_in_ir() {
    {
        let (m, at) = machine();
        let mut r = Rtl::new(m);
        r.boot();
        for _ in 0..50 {
            r.step().unwrap();
            if r.executed() == Some(at) {
                break;
            }
        }
        assert_eq!(r.executed(), Some(at));
        assert_eq!(r.ir() & OA as u64, OA as u64, "IR modified: {:o}", r.ir());
    }
}

/// Runs `e` from its boot until the OA write has executed, nops one
/// microcycle with the console's `NOP11`, and returns the next `n`
/// addresses executed.
fn executed_after_nop11<E: Engine>(
    e: &mut E,
    executed: fn(&E) -> Option<u16>,
    at: u16,
    n: usize,
) -> Vec<u16> {
    e.boot();
    for _ in 0..50 {
        e.step().unwrap();
        if executed(e) == Some(at) {
            break;
        }
    }
    assert_eq!(executed(e), Some(at), "the OA write at {at:o} executed");
    e.machine_mut().clock_control.nop11 = true;
    e.step().unwrap();
    assert_eq!(executed(e), None, "NOP11 nopped the microcycle");
    e.machine_mut().clock_control.nop11 = false;
    let mut after = Vec::new();
    for _ in 0..50 {
        e.step().unwrap();
        after.extend(executed(e));
        if after.len() == n {
            break;
        }
    }
    after
}

/// **An OA write dies with the instruction it modified when the console's
/// `NOP11` nops that instruction**: the next one runs as written. Here the
/// modified instruction is the jump to itself after the OA write; nopped,
/// it does not jump, and the word after it --- a jump to the PROM's word
/// 10 under `N` --- goes there, not to the target the OA word would have
/// made of it.
#[test]
fn an_oa_write_dies_with_an_instruction_nop11_nops() {
    let mut wrong = Vec::new();
    {
        let (mut m, at) = machine();
        let base = m.reset_pc();
        let next = Insn::new(JUMP | target(base as u64 + 0o10) | ALWAYS | N);
        let mut prom = m.prom.clone();
        prom[at as usize + 2] = next;
        m.load_prom(&prom);
        let want = vec![at + 2, base + 0o10, at];

        let mut r = Rtl::new(m.clone());
        let got_rtl = executed_after_nop11(&mut r, Rtl::executed, at, 3);
        let mut e = Micro::new(m);
        let got_micro = executed_after_nop11(&mut e, Micro::executed, at, 3);

        let show = |v: &[u16]| v.iter().map(|a| format!("{a:o}")).collect::<Vec<_>>();
        for (engine, got) in [("rtl", got_rtl), ("micro", got_micro)] {
            if got != want {
                wrong.push(format!("{engine}: {:?}, not {:?}", show(&got), show(&want)));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
