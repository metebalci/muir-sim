// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **A scan of what a band runs of the codes the CADR leaves unassigned**
//! and QUUX took for its clocks and its fused return: functional
//! destinations 3 to 7 and functional sources 15 and 17, at the same
//! positions in a 40-bit microinstruction as in a 48-bit CADR one
//! (`IR<23:19>`, `IR<30:26>`).
//!
//! A scan of the control store finds which microinstructions carry them;
//! an instruction the OA registers modify as it loads (`IMOD`) is made at
//! run time and no scan sees it. So a band is booted on `rtl` and every
//! executed microinstruction is read as it stood in `IR`, the OA
//! substitution done: each that writes destinations 3 to 7 or reads source
//! 17 has to be a control-store word that already did. Used by
//! `tests/unused_codes.rs` and `tests/system_2001.rs`.

use std::path::PathBuf;

use muir::engine::Engine;
use muir::machine::Machine;
use muir::rtl::Rtl;

pub fn octal(pcs: &[u16]) -> String {
    pcs.iter().map(|p| format!("{p:o}")).collect::<Vec<_>>().join(" ")
}

/// Whether `ir` writes functional destinations 3 to 7, or reads functional
/// source 15 or 17: an ALU or BYTE instruction with `IR<25>` clear and
/// `IR<23:19>` 3 to 7, or any class with `IR<31>` set and `IR<30:26>` 17.
pub fn uses_the_codes(ir: u64) -> bool {
    let class = ir >> 43 & 3;
    let dest =
        (class == 0 || class == 3) && ir >> 25 & 1 == 0 && (3..=7).contains(&(ir >> 19 & 0o37));
    // `IR<30>` is in no source decode, so 35 and 37 are 15 and 17 again.
    let src = ir >> 31 & 1 == 1 && matches!(ir >> 26 & 0o17, 0o15 | 0o17);
    dest || src
}

/// What a run found: the addresses whose executed word used the codes;
/// those among them whose control-store word did not; and, of what ran,
/// every value written to destination 3, the destination 4 writes, the
/// destination 5 to 7 writes and the source 17 reads, as `(address,
/// value)`, the value being the output bus the console reads for the
/// instruction in `IR` before it executes; and whether timer 0 was on at
/// the end.
#[derive(Default)]
pub struct Found {
    pub used: Vec<u16>,
    pub made: Vec<u16>,
    pub dest_3: Vec<(u16, u32)>,
    pub dest_4: Vec<(u16, u32)>,
    pub dest_5_to_7: Vec<(u16, u32)>,
    pub source_17: Vec<u16>,
    pub timer_0_on: bool,
}

/// Which of destinations 3, 4, and 5 to 7 `ir` writes, or source 17
/// reads.
pub fn codes(ir: u64) -> (bool, bool, bool, bool) {
    let class = ir >> 43 & 3;
    let d = (class == 0 || class == 3) && ir >> 25 & 1 == 0;
    let dest = ir >> 19 & 0o37;
    let src = ir >> 31 & 1 == 1 && ir >> 26 & 0o17 == 0o17;
    (d && dest == 3, d && dest == 4, d && (5..=7).contains(&dest), src)
}

/// Boots `m` to its listener on `rtl`, checking every executed
/// microinstruction ([`Found`]). It asserts the listener came, so that a
/// boot stuck early is not a pass.
pub fn run(m: Machine, chaos: (u16, u16), root: PathBuf) -> Found {
    let mut e = Rtl::new(m);
    e.boot();
    let m = e.machine_mut();
    m.chaos.address = chaos.0;
    super::ChaosServer::new(chaos.1).serving(root).at_time(super::time::TEST_UNIVERSAL).plug(m, 0);
    let mut found = Found::default();
    let (used, made) = (&mut found.used, &mut found.made);
    // Up to the listener, checked every million microcycles, and two
    // million more.
    let mut until = 300_000_000u64;
    for n in 0..300_000_000u64 {
        if n == until {
            break;
        }
        if n % 1_000_000 == 0 && until == 300_000_000 && super::lit_rows(&e, 84..130) > 400 {
            until = n + 2_000_000;
        }
        let ir = e.ir();
        let ob = uses_the_codes(ir).then(|| {
            use muir::spy::{OB_HIGH, OB_LOW};
            (e.spy_read(OB_HIGH) as u32) << 16 | e.spy_read(OB_LOW) as u32
        });
        e.step().unwrap();
        if let Some(pc) = e.executed()
            && uses_the_codes(ir)
        {
            let ob = ob.unwrap();
            match codes(ir) {
                (true, _, _, _) => found.dest_3.push((pc, ob)),
                (_, true, _, _) => found.dest_4.push((pc, ob)),
                (_, _, true, _) => found.dest_5_to_7.push((pc, ob)),
                _ => {}
            }
            if codes(ir).3 {
                found.source_17.push(pc);
            }
            if !used.contains(&pc) {
                used.push(pc);
            }
            let stored = e.machine().imem[pc as usize].raw();
            if !uses_the_codes(stored) && !made.contains(&pc) {
                made.push(pc);
            }
        }
    }
    assert!(super::lit_rows(&e, 84..130) > 400, "the listener never came");
    found.timer_0_on = e.machine().timers.timer[0].on;
    found
}
