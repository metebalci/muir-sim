// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **The profile** (contract G3 revision 15, A15b.8): per predicted site,
//! the majority outcome over a run, written for the micro-assembler's
//! profile pass, which sets the hint and the predicted target from it.
//!
//! The pipeline records what each site did ([`super::Outcomes`]); this
//! writes one line a site, keyed by the nearest preceding control store
//! label and the offset from it, so that edits elsewhere do not move a key.
//! `docs/quux.md` gives the format exactly, and the pass reads it.

use std::collections::BTreeMap;

use super::{Kind, Outcomes};
use crate::isa::{Insn, Op};
use crate::sym::{Space, Symbols};

/// A site's key: the nearest control store label at or below `addr`, the
/// alphabetically first of the names at that address, and the offset from
/// it in octal, always written, as `LABEL+OFFSET`; `None` when no label is
/// at or below it.
pub fn key(syms: &Symbols, addr: u16) -> Option<String> {
    let (_, off) = syms.nearest(Space::IMem, addr as u32)?;
    let at = addr as u32 - off;
    let name = syms.at(Space::IMem, at).iter().min()?;
    Some(format!("{name}+{off:o}"))
}

/// The word `kind` is written as in a dispatch's line.
pub fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Drop => "drop",
        Kind::Jump => "jump",
        Kind::Call => "call",
        Kind::Return => "return",
    }
}

/// A conditional jump's majority: whether it transferred in more than half
/// its executions. A tie predicts no transfer, today's word.
pub fn hint(counts: [u64; 2]) -> bool {
    counts[1] > counts[0]
}

/// A dispatch's majority entry: its kind and address, with its count. A
/// tie goes to the first in the order drop, jump, call, return and then by
/// address, so a drop-through, today's word, wins one.
pub fn majority(entries: &BTreeMap<(Kind, u16), u64>) -> Option<((Kind, u16), u64)> {
    let mut best: Option<((Kind, u16), u64)> = None;
    for (&e, &n) in entries {
        if best.is_none_or(|(_, b)| n > b) {
            best = Some((e, n));
        }
    }
    best
}

/// **The site records**, in address order: for each conditional jump
/// `jump KEY H EXECUTIONS`, and for each dispatch
/// `dispatch KEY KIND TARGET EXECUTIONS`, the target a key for a jump or a
/// call and `-` for a drop-through or a return; executions in decimal. A
/// site with no label at or below it, or whose target has none, is left
/// out and returned apart.
pub fn lines(o: &Outcomes, syms: &Symbols) -> (Vec<String>, Vec<u16>) {
    let mut by_addr: BTreeMap<u16, String> = BTreeMap::new();
    let mut unkeyed = Vec::new();
    for (&pc, &counts) in &o.jumps {
        let Some(k) = key(syms, pc) else {
            unkeyed.push(pc);
            continue;
        };
        let line = format!("jump {k} {} {}", u8::from(hint(counts)), counts[0] + counts[1]);
        by_addr.insert(pc, line);
    }
    for (&pc, entries) in &o.dispatches {
        let (Some(k), Some(((kind, addr), _))) = (key(syms, pc), majority(entries)) else {
            unkeyed.push(pc);
            continue;
        };
        let target = match kind {
            Kind::Jump | Kind::Call => match key(syms, addr) {
                Some(t) => t,
                None => {
                    unkeyed.push(pc);
                    continue;
                }
            },
            Kind::Drop | Kind::Return => "-".to_string(),
        };
        let total: u64 = entries.values().sum();
        by_addr.insert(pc, format!("dispatch {k} {} {target} {total}", kind_name(kind)));
    }
    unkeyed.sort_unstable();
    (by_addr.into_values().collect(), unkeyed)
}

/// What the profile covers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Conditional jumps RD predicts in the control store, and of them
    /// those executed, which the profile keys.
    pub jump_sites: u64,
    pub jump_sites_executed: u64,
    /// Their executions, and of them those whose outcome is the line's.
    pub jump_executions: u64,
    pub jumps_predicted: u64,
    /// The same for the dispatches that transfer.
    pub dispatch_sites: u64,
    pub dispatch_sites_executed: u64,
    pub dispatch_executions: u64,
    pub dispatches_predicted: u64,
}

/// Whether `w` is a site the profile keys, by its class: a conditional
/// jump RD predicts (`Some(true)`), a dispatch that transfers
/// (`Some(false)`), or neither.
pub fn site(w: Insn) -> Option<bool> {
    let ir = w.raw();
    let field = |pos: u32, len: u32| (ir >> pos) & ((1 << len) - 1);
    match w.op() {
        Op::Jump => {
            let always =
                field(5, 1) == 1 && !matches!(field(0, 5), 0o10..=0o12) && field(0, 5) & 7 == 7;
            (!always && field(8, 2) != 3 && !w.popj() && !w.oa_low_select()).then_some(true)
        }
        Op::Dispatch => (field(10, 2) != 2).then_some(false),
        _ => None,
    }
}

/// **The coverage** of the outcomes `o` in the control store `imem`
/// (A15b.8): the sites there and those executed, and the share of their
/// executions whose outcome is the majority's.
pub fn coverage(o: &Outcomes, imem: &[Insn]) -> Coverage {
    let mut c = Coverage::default();
    for w in imem {
        match site(*w) {
            Some(true) => c.jump_sites += 1,
            Some(false) => c.dispatch_sites += 1,
            None => {}
        }
    }
    for &counts in o.jumps.values() {
        c.jump_sites_executed += 1;
        c.jump_executions += counts[0] + counts[1];
        c.jumps_predicted += counts[hint(counts) as usize];
    }
    for entries in o.dispatches.values() {
        c.dispatch_sites_executed += 1;
        c.dispatch_executions += entries.values().sum::<u64>();
        c.dispatches_predicted += majority(entries).map_or(0, |(_, n)| n);
    }
    c
}
