// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Where a band's microcycles go, workload by workload.
//!
//! Boots the CADR's release, System 1003 (`tools/fetch-system-for-cadr.sh`),
//! on the CADR, and on QUUX the band `MUIR_BAND` names, System 2001's,
//! with the test harness's Chaosnet server at OZ,
//! logs in, defines a set of workloads at the listener and runs them one at
//! a time, counting every control-store address the engine executes. Each workload ends by writing a marker
//! file through the FILE service, which is how the run knows it is over:
//! nothing here reads the screen.
//!
//!     cargo run --release --example profile -- [micro|rtl] [cadr|quux|quux-4k|quux-16k] [workload ...]
//!
//! The machine is the CADR unless `quux` is named: QUUX, revision 13, as
//! the `quux` executable runs it.
//! `quux-4k` and `quux-16k` are QUUX with a PDL buffer of 4K or 16K words.
//! `quux-14` and `quux-15` are revisions 14 and 15, their band
//! `MUIR_BAND`'s and their PROM `MUIR_PROM`'s file, as `--prom` takes it;
//! revision 15 has none built in. On them `rtl` is the pipeline
//! (`muir::pipeline`), revision 14 as a measurement aid: its clock is
//! `MUIR_MICROCYCLE_NS`, in ns, 5 to 40 in steps of 0.5, the Kria's 8.5 by
//! default, on `micro` too; `MUIR_MEMORY_NS` is the port's
//! `<read>,<write>,<occupancy>` in ns or `kria`, `arty` or `de25`, `kria`
//! by default, `MUIR_CACHE` its cache's words, and `MUIR_SYNC_TICKS` is
//! refused.
//!
//! `MUIR_TIME_NEUTRAL=1` is the time-neutral harness (MP2b ruling Q12,
//! `tests/support/neutral.rs`), for `micro` and the pipeline: both run on
//! neutral time, `Machine::cycles` times the period; the harness's keys,
//! screen checks and marker polls are on an absolute schedule of
//! `Machine::cycles`; the host's dates and the RTC are fixed; and every
//! 65,536th macroinstruction boundary, a step of LC, prints a `digest`
//! line, the pipeline halted after that word and drained, with one of the
//! whole state at each workload's end. Two runs' `digest` lines compare as
//! they are.
//!
//! On QUUX, `MUIR_H8A` fills the MACRO DISPATCH MEMORY with the generic
//! handlers, `OPDTB`'s entry for each index's opcode, and enables the
//! MACRO-DISPATCH register with `QMLP`, `A-LOCALP` and `M-AP`, once the
//! microcode has reached its main loop, as a microcode that writes them
//! would; the macroinstructions counted then include those a fused return
//! dispatched. `MUIR_H8A=operand` gives the operand bit to the entries of
//! the opcodes whose `<8:0>` is a register and delta
//! (`tests/support/macro_dispatch.rs`, `fill_generic`).
//! `MUIR_H8A=microcode` fills nothing: the microcode fills the memory and
//! writes the register itself, and the checkers watch from its first
//! main-loop return with the register enabled. Either way the run is
//! watched by that file's checkers, each workload says what they counted,
//! and the whole run's counts close the output. On `rtl`, QUUX has its
//! cache-only prefetch (`muir::memory_port`, contract H8a §3.5), looking
//! for the next word in the fetched word's page; each workload then says
//! what it took, and how often a fused return used it. For measurement,
//! `MUIR_PREFETCH=line` fits it with the line's reach alone, and
//! `MUIR_PREFETCH=off` takes it out; `page` is QUUX's own.
//! `MUIR_MAIN_MEMORY_SIZE=<n>MW` gives QUUX `n` megawords of main memory,
//! as `--main-memory-size` does and written as it takes it; 2MW if not
//! given. QUUX boots its built-in PROM, PROM 2001.
//! `MUIR_RTC=<s>`
//! counts QUUX's real-time clock from second `s` of the Unix epoch in the
//! machine's own time, as `--rtc` does, in place of the host's clock, so
//! that the band's clock is the same in two runs.
//! With no workloads named, all of them run, in the order below. For each,
//! it prints the microcycles, the macroinstructions --- executions of
//! `QMLP+2`, the dispatch on `M-INST-OP` (`uc-macrocode.lisp`) --- and their
//! ratio; the time, where the microinstructions went by the category and
//! source file of the nearest `I-MEM` label in the symbol table, the
//! hottest labels, and the microcode's own meters out of A memory. On `rtl`
//! it adds the memory cycles. The workloads' time together closes the
//! output.
//!
//! **A microcycle count is not the time.** On `rtl` a wait or a hang for
//! the memory advances the clock and runs no microcycle, so the time line
//! gives the microcycles and their own time, the time stalled on memory,
//! and the time in all, which is the two added, side by side
//! (`tests/support/profile.rs`, held by `tests/profile_time.rs`). On `micro`
//! the memory's time is a fixed charge a memory cycle, not a stall, and the
//! line says so. The category and label shares are of the microinstructions
//! executed, which leave out the inhibited microcycles and the memory's
//! time both; every percentage says what it is of.
//!
//! Every count includes the typing: the listener reads the form a
//! character at a time. `nil`, the empty form, measures that overhead on
//! its own.
//!
//! The microcode is whatever the pack's current microload is; `MUIR_UCODE`
//! names a directory holding another microcode's `ucadr.mcr`, `.tbl` and
//! `.sym`, which is loaded into the run's copy of the pack and served as the
//! error table and read as the symbols. On the CADR it is MIT's, and
//! `diskpack load` puts it into MCR2 and makes MCR2 current. On QUUX it is
//! QUUX's, in partition order, and is written as it is over the current
//! microcode partition (GPT attribute bit 48) of the copy: the machine
//! never writes the GPT and muir does not edit it (contract Q8), so which
//! partition is current stays the disk's.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use muir::diskpack::{Command, Pack};
use muir::engine::Engine;
use muir::isa::{Insn, Op};
use muir::machine::Halt;
use muir::micro::Micro;
use muir::pipeline::Pipeline;
use muir::rtl::Rtl;
use muir::sym::{Space, Symbols};
use muir::terminal::keyboard::{Keyboard, keysym};

// The Chaosnet server and the typing are the test harness's: a CADR has no
// file server in it, and this program is a test run by hand.
#[path = "../tests/support/mod.rs"]
mod support;

use support::neutral::{Digests, Neutral};
use support::profile::Span;

/// LISPM-1 and OZ, as the release's `site/hosts.text` gives them.
const CHAOS: (u16, u16) = (0o177201, 0o177200);

/// The workloads: a name, and the form that runs it. Each is typed as
/// `(progn <form> (w-done "<name>"))`.
const WORKLOADS: &[(&str, &str)] = &[
    ("nil", "nil"),
    (
        "compile",
        "(mapc #'compile '(w-ack w-fib w-cons w-muldiv w-float w-array w-sort w-bignum w-intern))",
    ),
    ("calls-ack", "(w-ack 2 300)"),
    ("calls-fib", "(w-fib 24)"),
    ("cons", "(w-cons)"),
    ("arith-muldiv", "(w-muldiv)"),
    ("float", "(w-float)"),
    ("array", "(w-array)"),
    ("sort", "(w-sort)"),
    ("bignum", "(w-bignum)"),
    ("intern", "(w-intern)"),
    ("print-scroll", "(w-print)"),
    ("compile-again", "(mapc #'compile '(w-ack w-fib w-cons w-muldiv))"),
    ("bitblt", "(w-bitblt)"),
    // Not run unless named: the cost of switching stack groups, from the
    // listener's own shallow stack and from 500 frames deep, where every
    // switch has the PDL buffer's resident words to write out.
    ("switch-shallow", "(w-switch 0)"),
    ("switch-deep", "(w-switch 500)"),
];

/// The workloads run when none is named: all but the switching ones.
const DEFAULT_WORKLOADS: usize = 14;

/// The definitions, typed once after login. `w-done` writes the marker.
const DEFINITIONS: &[&str] = &[
    "(defun w-done (name) (with-open-file (s (string-append \"OZ: //lispm//\" name \".done\") :direction :output) (print name s)))",
    "(defun w-ack (m n) (cond ((zerop m) (1+ n)) ((zerop n) (w-ack (1- m) 1)) (t (w-ack (1- m) (w-ack m (1- n))))))",
    "(defun w-fib (n) (if (< n 2) n (+ (w-fib (1- n)) (w-fib (- n 2)))))",
    "(defun w-cons () (dotimes (i 10000) (make-list 200)))",
    "(defun w-muldiv () (let ((s 0)) (dotimes (i 150000) (setq s (remainder (+ s (* i 7)) 1000003))) s))",
    "(defun w-float () (let ((x 1.0)) (dotimes (i 50000) (setq x (+ (* x 1.0001) 0.5))) x))",
    "(defun w-array () (let ((a (make-array 1000))) (dotimes (i 186) (dotimes (j 1000) (aset j a j) (aref a j)))))",
    "(defun w-sort () (let ((l nil)) (dotimes (i 3000) (push (random 100000) l)) (sort l #'<)))",
    "(defun w-bignum () (dotimes (i 21) (print (expt 3 300))))",
    "(defun w-intern () (dotimes (i 1500) (intern (format nil \"W-SYM-~D\" i))))",
    "(defun w-print () (dotimes (i 1000) (print i)))",
    // BITBLT between two 512x256 one-bit arrays and within one, over a few
    // sizes and alignments, with the ALU functions the window system uses;
    // the last two overlap, as a scroll does, up and down.
    "(defun w-bitblt () (let ((a (make-pixel-array 512 256 ':type 'art-1b)) (b (make-pixel-array 512 256 ':type 'art-1b))) (dotimes (i 40) (bitblt tv:alu-seta 480 200 a 0 0 b 0 0) (bitblt tv:alu-xor 480 200 a 3 5 b 17 1) (bitblt tv:alu-ior 100 50 a 1 0 b 40 9) (bitblt tv:alu-seta 32 32 a 0 0 b 33 2) (bitblt tv:alu-seta 480 200 b 0 8 b 0 0) (bitblt tv:alu-xor 480 200 b 0 0 b 5 8))))",
    "(defun w-switch (n) (if (zerop n) (progn (dotimes (i 2000) (process-allow-schedule)) 0) (1+ (w-switch (1- n)))))",
];

/// The microcode's meters, by their `A-MEM` names in the symbol table.
const METERS: &[&str] = &[
    "A-FIRST-LEVEL-MAP-RELOADS",
    "A-SECOND-LEVEL-MAP-RELOADS",
    "A-META-BITS-MAP-RELOADS",
    "A-PDL-BUFFER-READ-FAULTS",
    "A-PDL-BUFFER-WRITE-FAULTS",
    "A-FRESH-PAGE-COUNT",
    "A-DISK-PAGE-READ-COUNT",
    "A-DISK-PAGE-WRITE-COUNT",
];

/// What the two engines can say that the harness needs beyond [`Engine`].
trait Profiled: Engine {
    /// The fused return's checkers, where the engine runs under them.
    fn checker(&self) -> Option<&support::macro_dispatch::Checker> {
        None
    }
    fn executed_pc(&self) -> Option<u16>;
    /// The microcycles and the time so far.
    fn span(&self) -> Span;
    /// Nanoseconds stalled on the bus, memory cycles, the machine's
    /// nanoseconds, and the memory cache's hits and misses, where the
    /// engine keeps them.
    fn bus(&self) -> Option<[u64; 5]>;
    /// `Some(wrong_word)` when the last microcycle started a macroinstruction
    /// fetch, where the engine says.
    fn fetch_started(&self) -> Option<bool> {
        None
    }
    /// QUUX's prefetch's counts, where it is fitted.
    fn prefetch_counts(&self) -> Option<muir::memory_port::PrefetchCounts> {
        None
    }
    /// A workload's end: under the time-neutral harness, its whole digest.
    fn workload_end(&mut self, _name: &str) {}
    /// Lines of the engine's own for the whole run's account.
    fn run_lines(&self) -> Vec<String> {
        Vec::new()
    }
}

impl Profiled for Micro {
    fn executed_pc(&self) -> Option<u16> {
        self.executed()
    }
    fn span(&self) -> Span {
        Span::of_micro(self)
    }
    fn bus(&self) -> Option<[u64; 5]> {
        None
    }
}

impl<E: Profiled + support::macro_dispatch::Executes> Profiled
    for support::macro_dispatch::Checked<E>
{
    fn checker(&self) -> Option<&support::macro_dispatch::Checker> {
        Some(&self.checker)
    }
    fn executed_pc(&self) -> Option<u16> {
        self.engine.executed_pc()
    }
    fn span(&self) -> Span {
        self.engine.span()
    }
    fn bus(&self) -> Option<[u64; 5]> {
        self.engine.bus()
    }
    fn fetch_started(&self) -> Option<bool> {
        Profiled::fetch_started(&self.engine)
    }
    fn prefetch_counts(&self) -> Option<muir::memory_port::PrefetchCounts> {
        self.engine.prefetch_counts()
    }
    fn workload_end(&mut self, name: &str) {
        self.engine.workload_end(name)
    }
    fn run_lines(&self) -> Vec<String> {
        self.engine.run_lines()
    }
}

/// Revision 15's `rtl`, its pipeline: no stall of its own in the bus's
/// sense; the port's writes and fills are its memory cycles.
impl Profiled for Pipeline {
    fn executed_pc(&self) -> Option<u16> {
        self.executed()
    }
    fn span(&self) -> Span {
        Span::of_pipeline(self)
    }
    fn bus(&self) -> Option<[u64; 5]> {
        let c = &self.port.cache;
        let cycles = self.port.meters.writes + self.port.meters.fills;
        Some([0, cycles, Span::of_pipeline(self).ns, c.hits, c.misses])
    }
    fn run_lines(&self) -> Vec<String> {
        let c = &self.meters.consecutive_starts;
        let kinds = ["read", "write", "fetch"];
        let pairs: Vec<String> = (0..3)
            .flat_map(|a| (0..3).map(move |b| (a, b)))
            .map(|(a, b)| format!("{}->{} {}", kinds[a], kinds[b], c[a][b]))
            .collect();
        let w = &self.port.meters;
        vec![
            format!("starts right after a start: {}", pairs.join(", ")),
            format!(
                "write channel: {} writes accepted, {} of two beats; {} accepts delayed by a second beat, {} clocks",
                w.writes, w.two_beat_writes, w.beat_delays, w.beat_delay_clocks
            ),
        ]
    }
}

/// An engine under the time-neutral harness: its digests printed as it
/// runs (`tests/support/neutral.rs`).
impl<E: Profiled + Neutral + support::macro_dispatch::Executes> Profiled for Digests<E> {
    fn checker(&self) -> Option<&support::macro_dispatch::Checker> {
        self.engine.checker()
    }
    fn executed_pc(&self) -> Option<u16> {
        self.engine.executed_pc()
    }
    fn span(&self) -> Span {
        self.engine.span()
    }
    fn bus(&self) -> Option<[u64; 5]> {
        self.engine.bus()
    }
    fn fetch_started(&self) -> Option<bool> {
        Profiled::fetch_started(&self.engine)
    }
    fn prefetch_counts(&self) -> Option<muir::memory_port::PrefetchCounts> {
        self.engine.prefetch_counts()
    }
    fn workload_end(&mut self, name: &str) {
        self.end_of(name);
    }
    fn run_lines(&self) -> Vec<String> {
        self.engine.run_lines()
    }
}

impl Profiled for Rtl {
    fn executed_pc(&self) -> Option<u16> {
        self.executed()
    }
    fn span(&self) -> Span {
        Span::of_rtl(self)
    }
    fn bus(&self) -> Option<[u64; 5]> {
        let (h, m) = self.cache().map_or((0, 0), |c| (c.hits, c.misses));
        Some([self.stalled_ns(), self.bus_cycles(), self.ns(), h, m])
    }
    fn fetch_started(&self) -> Option<bool> {
        Rtl::fetch_started(self)
    }
    fn prefetch_counts(&self) -> Option<muir::memory_port::PrefetchCounts> {
        self.prefetch().and(Rtl::prefetch_counts(self))
    }
}

/// The source file of every label, from the `uc-*.lisp` files: a label is a
/// token at the start of a line inside a file's list.
fn label_files(ucadr: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(ucadr).expect("sys/ucadr") {
        let path = entry.unwrap().path();
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        if !stem.starts_with("uc-") || path.extension().is_none_or(|e| e != "lisp") {
            continue;
        }
        let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
        for line in text.lines() {
            let Some(c) = line.chars().next() else { continue };
            if c.is_whitespace() || c == '(' || c == ';' || c == ')' {
                continue;
            }
            let label: String =
                line.chars().take_while(|c| !c.is_whitespace() && *c != ';').collect();
            files.entry(label).or_insert_with(|| stem.clone());
        }
    }
    files
}

/// The category the 17 September 2026 profile used for a label in a file:
/// named families first, then the file.
fn category(label: &str, file: &str) -> String {
    let starts = |ps: &[&str]| ps.iter().any(|p| label.starts_with(p));
    if label.starts_with("QMLP") {
        "dispatch".into()
    } else if starts(&["P-R-", "P-B-"]) || label.contains("PDL-BUFFER") {
        "pdl-buffer".into()
    } else if starts(&["CZRR", "PHTDEL", "FINDCORE", "AGER", "SWAP", "PAGE-IN"]) {
        "paging".into()
    } else if starts(&["MPY", "DIV", "DIVIDE-ONCE", "BMPY", "BIDIV", "BFXMPY", "FDIV"]) {
        "mpy/div".into()
    } else {
        match file {
            "uc-page-fault" => "map".into(),
            "uc-macrocode" => "macro-ops".into(),
            "uc-call-return" => "call-return".into(),
            "uc-fctns" => "fctns".into(),
            "uc-array" => "array".into(),
            "uc-arith" => "arith".into(),
            "uc-storage-allocation" => "alloc".into(),
            "uc-transporter" => "transporter".into(),
            "uc-tv" => "tv".into(),
            other => other.trim_start_matches("uc-").into(),
        }
    }
}

/// The TLB's counts a phase keeps ([`Phase::tlb`]).
const TLB_COUNTS: usize = 32;

struct Phase {
    /// The microcycles and the time, the memory's and in all.
    span: Span,
    /// QUUX's prefetch's counts over the workload, where it is fitted.
    prefetch: Option<muir::memory_port::PrefetchCounts>,
    /// What the fused return's checkers counted, where they ran.
    checked: Option<support::macro_dispatch::Counts>,
    /// Revision 14's TLB over the workload: walks, sweeps, on `rtl` the time
    /// held for them, write-backs, those carrying accessed, modified and
    /// ephemeral-reference, the guard's refusals, and the PDL buffer
    /// redirect's references inside and outside the buffer (contract G3
    /// revision 14, §11.1); then the model's counts of directly written
    /// entries a fill replaced, by port A then B, each by the replaced
    /// entry's status 0-7, and of microcycles in which both ports missed at
    /// one index; then, on `rtl`, walks that waited for the processor's
    /// cycle in flight, by port A then B, the time they waited, by port,
    /// and port-B walks made while the microcycle waits for `MD`.
    tlb: Option<[u64; TLB_COUNTS]>,
    /// Fused returns: macroinstructions dispatched without `QMLP+2`.
    fused: u64,
    hist: Vec<u64>,
    /// Nanoseconds stalled in the step that executed each address.
    stall_hist: Vec<u64>,
    meters: Vec<u32>,
    bus: Option<[u64; 5]>,
    /// Macroinstruction fetches started: `[at QMLP, elsewhere]` by
    /// `[next in sequence, wrong word]`.
    fetches: [[u64; 2]; 2],
    /// Macroinstructions by `(halfword, handler, microcycles from its
    /// dispatch to the next)`, and how often each halfword's opcode named,
    /// through D-MEM, the handler that ran: `[the one taken, the other]`.
    /// `LC` has already moved past the instruction when the dispatch has
    /// run, so the halfword taken is the one `LC<1>` does not select; the
    /// check is what says so.
    ops: HashMap<(u16, u16, u64), u64>,
    decode_agrees: [u64; 2],
    hazards: Hazards,
}

/// What a deeper microinstruction pipeline would meet, counted over the
/// executed stream (`MUIR_HAZARDS`): how control leaves the sequence, and
/// how often an instruction reads the A or M word one of the two before it
/// wrote.
#[derive(Default, Clone)]
struct Hazards {
    executed: u64,
    /// Inhibited cycles: a jump's `N`, or a dispatch's.
    nopped: u64,
    /// Transfers (the next executed address is not this one + 1), by what
    /// caused them: `[jump always, jump on a condition, dispatch, POPJ,
    /// other]`, and `[through the delay slot, the slot inhibited]`.
    transfers: [[u64; 2]; 5],
    /// Jumps on a condition executed, taken or not.
    conditional: u64,
    /// Reads of the word the previous instruction wrote, and of the one
    /// the instruction before that wrote (and not the previous).
    reads_d1: u64,
    reads_d2: u64,
}

/// The A memory words `i` writes, as a range, and the M word: an ALU or
/// BYTE destination in A memory writes that word; a functional one writes M
/// memory and the A word it shadows.
fn writes(i: Insn) -> Option<(u16, bool)> {
    let dest = match i.op() {
        Op::Alu => i.alu().dest,
        Op::Byte => i.byte().dest,
        _ => return None,
    };
    Some(if dest.is_a_mem() { (dest.a_addr(), false) } else { (u16::from(dest.m_addr()), true) })
}

/// Whether `i` reads A word `a` (M words are A words 0-37).
fn reads(i: Insn, w: (u16, bool)) -> bool {
    let a_read = i.op() != Op::Dispatch && i.a_src() == w.0;
    let m_read = !i.m_src_functional() && w.0 < 32 && u16::from(i.m_src()) == w.0;
    a_read || m_read
}

fn meters(e: &impl Engine, syms: &Symbols) -> Vec<u32> {
    METERS
        .iter()
        .map(|m| {
            syms.address(Space::AMem, m).map(|a| e.machine().amem[a as usize] as u32).unwrap_or(0)
        })
        .collect()
}

/// The screen, folded to a number: changed when it has.
fn screen_hash(e: &impl Engine) -> u64 {
    e.machine()
        .tv
        .buffer()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, &w| (h ^ w as u64).wrapping_mul(0x100_0000_01b3))
}

/// Types `text` a key at a time, each taken off the keyboard by the
/// microcode and then echoed by Lisp before the next, as `tests/cc_harness`
/// does: the microcode's keyboard buffer holds 64 characters and Lisp
/// empties it only when its process runs, so a line typed faster wraps it
/// and arrives garbled. A key not echoed within twenty million microcycles
/// goes on anyway. Every microcycle is run by `step`.
fn type_echoed<E: Engine>(e: &mut E, k: &mut Keyboard, text: &str, step: &mut impl FnMut(&mut E)) {
    for ch in text.chars() {
        let before = screen_hash(e);
        let sym = if ch == '\n' { keysym::RETURN } else { ch as u32 };
        let shifted = ch.is_ascii_uppercase() || "~!@#$%^&*()_+{}|:\"<>?".contains(ch);
        if shifted {
            k.key(keysym::SHIFT_L, true);
        }
        k.key(sym, true);
        k.key(sym, false);
        if shifted {
            k.key(keysym::SHIFT_L, false);
        }
        let mut waited = 0u64;
        // QUUX's keyboard is on the register page (contract Q3); the
        // CADR's on the I/O board.
        let on_quux = e.machine().geometry.machine_id.is_some();
        let waiting = |e: &E| {
            let m = e.machine();
            if on_quux { m.quux_input.key_waiting() } else { m.ioboard.keyboard_ready() }
        };
        while k.pending() > 0 || waiting(e) {
            if on_quux {
                k.deliver(&mut e.machine_mut().quux_input);
            } else {
                k.deliver(&mut e.machine_mut().ioboard);
            }
            // Under the time-neutral harness, at the same microcycles on
            // every engine.
            support::neutral::run_for(e, 1_000, &mut *step);
            waited += 1_000;
            assert!(waited < 50_000_000, "the machine never read the keyboard");
        }
        let mut echoed = 0u64;
        while screen_hash(e) == before && echoed < 20_000_000 {
            support::neutral::run_for(e, 50_000, &mut *step);
            echoed += 50_000;
        }
    }
}

/// Types `form` and runs until `marker` appears, counting what executes.
fn run<E: Profiled>(
    e: &mut E,
    k: &mut Keyboard,
    form: &str,
    marker: &Path,
    syms: &Symbols,
) -> Phase {
    let before = meters(e, syms);
    let bus0 = e.bus();
    let mut hist = vec![0u64; 1 << 14];
    let mut stall_hist = vec![0u64; 1 << 14];
    let span0 = e.span();
    let prefetch0 = e.prefetch_counts();
    let fused0 = e.machine().macro_dispatch.fused;
    let tlb_counts = |m: &muir::machine::Machine| {
        let t = &m.tlb;
        let [a, b, c] = t.written_bits;
        let [r_in, r_out] = t.redirects;
        let mut v = [0; TLB_COUNTS];
        v[..10].copy_from_slice(&[
            t.walks,
            t.sweeps,
            t.held_ns,
            t.write_backs,
            a,
            b,
            c,
            t.refusals,
            r_in,
            r_out,
        ]);
        v[10..26].copy_from_slice(t.evicted.as_flattened());
        v[26] = t.double_misses;
        v[27..29].copy_from_slice(&t.walks_waited);
        v[29..31].copy_from_slice(&t.walks_waited_ns);
        v[31] = t.walks_waiting_md;
        v
    };
    let tlb0 = tlb_counts(e.machine());
    let checked0 = e.checker().map(|c| c.counts.clone());
    let mut fetches = [[0u64; 2]; 2];
    let qmlp = syms.address(Space::IMem, "QMLP");
    let mut ops: HashMap<(u16, u16, u64), u64> = HashMap::new();
    let mut decode_agrees = [0u64; 2];
    // The last dispatch on `M-INST-OP`: its microcycle, both halfwords,
    // `LC`, and the handler once it runs.
    let mut last: Option<(u64, u32, u32, Option<u16>)> = None;
    let mut since_dispatch = 0u32;
    let ops_wanted = std::env::var_os("MUIR_OPS").is_some();
    let hazards_wanted = std::env::var_os("MUIR_HAZARDS").is_some();
    let mut hz = Hazards::default();
    // The last two executed `(PC, instruction)`, and whether a cycle was
    // inhibited since the last.
    let mut back: [Option<(u16, Insn)>; 2] = [None, None];
    let mut nop_since = false;
    // Typing steps the engine too, so count through it.
    let mut step = |e: &mut E| {
        let s0 = e.bus().map_or(0, |b| b[0]);
        if let Err(Halt::UnknownDest { pc, dest }) = e.step() {
            panic!("halted at {pc:o} on destination {dest:o}");
        }
        if let Some(pc) = e.executed_pc() {
            hist[pc as usize] += 1;
            stall_hist[pc as usize] += e.bus().map_or(0, |b| b[0]) - s0;
        }
        if hazards_wanted {
            match e.executed_pc() {
                None => {
                    hz.nopped += 1;
                    nop_since = true;
                }
                Some(pc) => {
                    // The workloads run with the boot PROM long disabled.
                    let i = e.machine().imem[pc as usize];
                    hz.executed += 1;
                    if i.op() == Op::Jump && !(i.jump().internal_cond && i.jump().cond == 7) {
                        hz.conditional += 1;
                    }
                    if let Some((ppc, _)) = back[0]
                        && pc != ppc.wrapping_add(1)
                    {
                        // The jump is the one before its delay slot, or,
                        // with the slot inhibited, the last executed.
                        let cause = if nop_since { back[0] } else { back[1] };
                        let kind = match cause.map(|(_, c)| c) {
                            Some(c) if c.popj() => 3,
                            Some(c) if c.op() == Op::Jump => {
                                let j = c.jump();
                                if j.internal_cond && j.cond == 7 { 0 } else { 1 }
                            }
                            Some(c) if c.op() == Op::Dispatch => 2,
                            _ => 4,
                        };
                        hz.transfers[kind][nop_since as usize] += 1;
                    }
                    let w1 = back[0].and_then(|(_, p)| writes(p));
                    let w2 = back[1].and_then(|(_, p)| writes(p));
                    if w1.is_some_and(|w| reads(i, w)) {
                        hz.reads_d1 += 1;
                    } else if w2.is_some_and(|w| reads(i, w)) {
                        hz.reads_d2 += 1;
                    }
                    back = [Some((pc, i)), back[0]];
                    nop_since = false;
                }
            }
        }
        if ops_wanted && let (Some(pc), Some(q)) = (e.executed_pc(), qmlp) {
            since_dispatch += 1;
            if u32::from(pc) == q + 2 {
                let now = e.machine().cycles;
                if let Some((then, word, lc, Some(handler))) = last {
                    let half = if lc & 2 != 0 { word & 0xffff } else { word >> 16 } as u16;
                    *ops.entry((half, handler, now - then)).or_default() += 1;
                }
                last = Some((now, e.machine().mmem[0o31] as u32, e.lc(), None));
                since_dispatch = 0;
            } else if since_dispatch == 2
                && let Some((_, word, lc, h @ None)) = last.as_mut()
            {
                *h = Some(pc);
                let (lo, hi) = (*word & 0xffff, *word >> 16);
                let chosen = if *lc & 2 != 0 { [lo, hi] } else { [hi, lo] };
                for (i, half) in chosen.into_iter().enumerate() {
                    let op = (half >> 9) & 0o37;
                    if e.machine().dmem[(0o2300 + op) as usize] & 0o37777 == u32::from(pc) {
                        decode_agrees[i] += 1;
                    }
                }
            }
        }
        if let Some(wrong) = e.fetch_started() {
            let at_qmlp = e.executed_pc().map(u32::from) == qmlp;
            fetches[!at_qmlp as usize][wrong as usize] += 1;
        }
    };
    // No Return: the listener runs a form when its last parenthesis is in,
    // and a Return after it would wait in the buffer as typeahead.
    type_echoed(e, k, form, &mut step);
    let mut ran = 0u64;
    while !marker.exists() {
        support::neutral::run_for(e, 100_000, &mut step);
        ran += 100_000;
        if ran >= 2_000_000_000 {
            // What the listener says is the only account of why.
            // Outside the run's scratch directory, which goes with the panic.
            let shot = std::env::temp_dir().join(format!(
                "muir-profile-{}.gif",
                marker.file_stem().unwrap().to_string_lossy()
            ));
            let mut rec = muir::capture::Recorder::new(false);
            rec.sample(&e.machine().tv, 0, 0);
            std::fs::write(&shot, rec.gif()).unwrap();
            panic!("{} never came; the screen is {}", marker.display(), shot.display());
        }
    }
    let after = meters(e, syms);
    let bus = match (bus0, e.bus()) {
        (Some(a), Some(b)) => Some(std::array::from_fn(|i| b[i] - a[i])),
        _ => None,
    };
    Phase {
        span: e.span().since(span0),
        prefetch: e.prefetch_counts().zip(prefetch0).map(|(a, b)| prefetch_since(a, b)),
        checked: e.checker().zip(checked0.as_ref()).map(|(c, c0)| c.counts.since(c0)),
        fused: e.machine().macro_dispatch.fused - fused0,
        tlb: e.machine().geometry.paged().then(|| {
            let now = tlb_counts(e.machine());
            std::array::from_fn(|k| now[k] - tlb0[k])
        }),
        hist,
        stall_hist,
        meters: after.iter().zip(&before).map(|(a, b)| a.wrapping_sub(*b)).collect(),
        bus,
        fetches,
        ops,
        decode_agrees,
        hazards: hz,
    }
}

/// The prefetch's counts after `b` was taken.
fn prefetch_since(
    a: muir::memory_port::PrefetchCounts,
    b: muir::memory_port::PrefetchCounts,
) -> muir::memory_port::PrefetchCounts {
    muir::memory_port::PrefetchCounts {
        fetches: a.fetches - b.fetches,
        same_line: a.same_line - b.same_line,
        next_line: a.next_line - b.next_line,
        page_end: a.page_end - b.page_end,
        not_held: a.not_held - b.not_held,
        dropped: std::array::from_fn(|k| a.dropped[k] - b.dropped[k]),
        used: a.used - b.used,
        refused: std::array::from_fn(|k| a.refused[k] - b.refused[k]),
    }
}

/// The prefetch's counts in a line.
fn prefetch_line(c: &muir::memory_port::PrefetchCounts) -> String {
    let taken = c.same_line + c.next_line;
    format!(
        "prefetch: {} fetches answered, {taken} next words taken ({:.1}% of the fetches answered: {} in the line, {} in the next), {} past the page, {} not held; dropped by LC {}, store {}, transfer {}, map {}, reset {}; used by {} fused returns; refused for condition 6 {}, a store starting {}",
        c.fetches,
        100.0 * taken as f64 / c.fetches.max(1) as f64,
        c.same_line,
        c.next_line,
        c.page_end,
        c.not_held,
        c.dropped[0],
        c.dropped[1],
        c.dropped[2],
        c.dropped[3],
        c.dropped[4],
        c.used,
        c.refused[0],
        c.refused[1],
    )
}

/// The returns the checkers counted as made by a handler a fused return
/// ran, all of them and those of a specialised handler, one `OPDTB` names
/// for no opcode, the most frequent of those by label; and of those, the
/// ones whose returning microinstruction is the specialised handler's own,
/// under its label.
fn handler_returns_line(
    c: &support::macro_dispatch::Counts,
    generic: &std::collections::BTreeSet<u16>,
    label: impl Fn(u16) -> String,
) -> String {
    let returns = &c.handler_returns;
    let base = |l: String| l.rsplit_once('+').map_or(l.clone(), |(b, _)| b.to_string());
    let special_labels: std::collections::BTreeSet<String> =
        returns.keys().filter(|h| !generic.contains(h)).map(|&h| base(label(h))).collect();
    let own: u64 = c
        .handler_return_sites
        .iter()
        .filter(|(at, _)| special_labels.contains(&base(label(**at))))
        .map(|(_, n)| n)
        .sum();
    let all: u64 = returns.values().sum();
    let mut special: Vec<(u64, u16)> =
        returns.iter().filter(|(h, _)| !generic.contains(h)).map(|(&h, &n)| (n, h)).collect();
    special.sort_by_key(|&(n, _)| std::cmp::Reverse(n));
    let total: u64 = special.iter().map(|(n, _)| n).sum();
    let top: Vec<String> =
        special.iter().take(8).map(|&(n, h)| format!("{} {n}", label(h))).collect();
    format!(
        "fused returns made by a handler a fused return ran: {all}, by a specialised one {total} ({}), by the specialised handler's own microinstruction {own}",
        top.join(", ")
    )
}

fn report(
    name: &str,
    p: &Phase,
    syms: &Symbols,
    files: &BTreeMap<String, String>,
    qmlp: u32,
    generic: &std::collections::BTreeSet<u16>,
) {
    let executed: u64 = p.hist.iter().sum();
    let macros = p.hist[(qmlp + 2) as usize] + p.fused;
    println!("== {name}");
    println!(
        "   {} microcycles, {} microinstructions executed, {} macroinstructions, {:.1} microcycles each",
        p.span.microcycles,
        executed,
        macros,
        p.span.microcycles as f64 / macros.max(1) as f64
    );
    println!("   {}", p.span.line());
    if p.fused > 0 {
        println!(
            "   fused returns {} ({:.1}% of macroinstructions)",
            p.fused,
            100.0 * p.fused as f64 / macros as f64
        );
    }
    if let Some(c) = &p.checked {
        let label = |pc: u16| {
            syms.nearest(Space::IMem, pc as u32)
                .map(|(l, off)| format!("{l}+{off:o}"))
                .unwrap_or_else(|| "?".into())
        };
        println!("   checkers: {}, problems {}", c.report(label), c.problems());
        println!("   {}", handler_returns_line(c, generic, label));
    }
    if let Some(t) = p.tlb {
        let [walks, sweeps, held, wbs, a, m, e, refused, r_in, r_out] = t[..10].try_into().unwrap();
        let evicted = |port: usize| {
            let by = &t[10 + 8 * port..18 + 8 * port];
            let statuses: Vec<String> =
                (0..8).filter(|&s| by[s] != 0).map(|s| format!("status {s} {}", by[s])).collect();
            let sum: u64 = by.iter().sum();
            if statuses.is_empty() {
                sum.to_string()
            } else {
                format!("{sum} ({})", statuses.join(", "))
            }
        };
        println!(
            "   tlb: {walks} walks, {sweeps} sweeps, {held} ns held for them and the redirect; {wbs} write-backs, accessed {a}, modified {m}, ephemeral-reference {e}, {refused} refused; redirected {r_in} inside the PDL buffer, {r_out} outside; direct writes evicted by a fill, port A {}, port B {}; {} microcycles with both ports missing at one index",
            evicted(0),
            evicted(1),
            t[26]
        );
        println!(
            "   tlb walks behind a cycle in flight: port A {} ({} ns), port B {} ({} ns); port-B walks while the microcycle waits for MD {}",
            t[27], t[29], t[28], t[30], t[31]
        );
    }
    if let Some(c) = &p.prefetch {
        println!("   {}", prefetch_line(c));
    }
    if let Some([_, bus, _, hits, misses]) = p.bus {
        println!("   {bus} memory cycles");
        if hits + misses > 0 {
            println!("   cache {hits} hits, {misses} misses");
        }
    }
    let h = &p.hazards;
    if h.executed > 0 {
        let pct = |n: u64| 100.0 * n as f64 / h.executed as f64;
        let t: Vec<String> = ["always", "conditional", "dispatch", "popj", "other"]
            .iter()
            .zip(h.transfers)
            .map(|(k, [slot, nop])| format!("{k} {:.2}%+{:.2}%", pct(slot), pct(nop)))
            .collect();
        println!(
            "   hazards, each a % of the {} microinstructions executed: {:.2}% inhibited; transfers (slot+inhibited) {}; conditional jumps {:.2}%; reads of the last write {:.2}%, of the one before {:.2}%",
            h.executed,
            pct(h.nopped),
            t.join(", "),
            pct(h.conditional),
            pct(h.reads_d1),
            pct(h.reads_d2)
        );
    }
    let [[q_seq, q_wrong], [o_seq, o_wrong]] = p.fetches;
    if q_seq + q_wrong + o_seq + o_wrong > 0 {
        println!(
            "   fetches: at QMLP {q_seq} next in sequence, {q_wrong} wrong word; elsewhere {o_seq}, {o_wrong}"
        );
    }
    let mut by_cat: BTreeMap<String, u64> = BTreeMap::new();
    let mut by_label: BTreeMap<String, u64> = BTreeMap::new();
    for (pc, &n) in p.hist.iter().enumerate() {
        if n == 0 {
            continue;
        }
        let label = syms
            .nearest(Space::IMem, pc as u32)
            .map(|(l, _)| l.to_string())
            .unwrap_or_else(|| "?".into());
        let file = files.get(&label).map(String::as_str).unwrap_or("?");
        *by_cat.entry(category(&label, file)).or_default() += n;
        *by_label.entry(label).or_default() += n;
    }
    let pct = |n: u64| 100.0 * n as f64 / executed.max(1) as f64;
    let mut cats: Vec<_> = by_cat.into_iter().collect();
    cats.sort_by_key(|c| std::cmp::Reverse(c.1));
    let line: Vec<String> = cats.iter().map(|(c, n)| format!("{c} {:.1}", pct(*n))).collect();
    println!("   by category, % of the microinstructions executed: {}", line.join(", "));
    let mut labels: Vec<_> = by_label.into_iter().collect();
    labels.sort_by_key(|l| std::cmp::Reverse(l.1));
    let top: Vec<String> =
        labels.iter().take(6).map(|(l, n)| format!("{l} {:.1}", pct(*n))).collect();
    println!("   hottest, % of the microinstructions executed: {}", top.join(", "));
    let m: Vec<String> = METERS.iter().zip(&p.meters).map(|(n, v)| format!("{n} {v}")).collect();
    println!("   {}", m.join(", "));
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let engine = if args.first().is_some_and(|a| a == "micro" || a == "rtl") {
        args.remove(0)
    } else {
        "micro".into()
    };
    use muir::machine::Geometry;
    let geometry = match args.first().map(String::as_str) {
        Some("cadr") => Some(Geometry::CADR),
        Some("quux") => Some(Geometry::QUUX),
        Some("quux-4k") => Some(Geometry { pdl_bits: 12, ..Geometry::QUUX }),
        Some("quux-16k") => Some(Geometry { pdl_bits: 14, ..Geometry::QUUX }),
        Some("quux-14") => Some(Geometry::QUUX_14),
        Some("quux-15") => Some(Geometry::QUUX_15),
        _ => None,
    };
    let geometry = match geometry {
        Some(g) => {
            args.remove(0);
            g
        }
        None => Geometry::CADR,
    };
    let wanted: Vec<&(&str, &str)> = if args.is_empty() {
        WORKLOADS[..DEFAULT_WORKLOADS].iter().collect()
    } else {
        args.iter()
            .map(|a| {
                WORKLOADS.iter().find(|(n, _)| n == a).unwrap_or_else(|| panic!("no workload {a}"))
            })
            .collect()
    };
    let neutral = support::neutral::on();
    // Revision 15's clock, on both engines: `MUIR_MICROCYCLE_NS`, in ns, 5
    // to 40 in steps of 0.5; the Kria's 8.5 by default.
    let period = std::env::var("MUIR_MICROCYCLE_NS").ok().map(|v| {
        assert!(geometry.extended(), "MUIR_MICROCYCLE_NS={v} is QUUX revision 15's: quux-15");
        muir::clock::parse_microcycle_ns(&v).unwrap_or_else(|| {
            panic!("MUIR_MICROCYCLE_NS={v}: the period in ns, 5 to 40 in steps of 0.5")
        })
    });
    println!("{}", configuration(&engine, geometry, period, neutral));
    match engine.as_str() {
        "rtl" if geometry.paged() => {
            // Revisions 14 and 15 on `rtl` are the pipeline, 14 as a
            // measurement aid; its clock is the period, never `sync`'s.
            if let Ok(v) = std::env::var("MUIR_SYNC_TICKS") {
                panic!(
                    "MUIR_SYNC_TICKS={v} is refused on the pipeline, whose clock is MUIR_MICROCYCLE_NS's"
                );
            }
            // `MUIR_MEMORY_NS=<read>,<write>,<occupancy>`, or `kria`, `arty`
            // or `de25`: the port's timing; `MUIR_CACHE`, its cache's words.
            let timing = std::env::var("MUIR_MEMORY_NS").ok().map_or(muir::pipeline::PortTiming::KRIA, |v| {
                muir::pipeline::PortTiming::parse(&v).unwrap_or_else(|| {
                    panic!("MUIR_MEMORY_NS={v}: <read>,<write>,<occupancy> in ns, or kria, arty or de25")
                })
            });
            let cache = std::env::var("MUIR_CACHE").ok().map_or(muir::pipeline::CACHE_WORDS, |v| {
                v.parse().unwrap_or_else(|_| panic!("MUIR_CACHE={v}: the cache's words"))
            });
            let pipeline = move |m: muir::machine::Machine| {
                let mut e = Pipeline::new(m);
                let p = period.unwrap_or(e.period());
                e.configure(p, timing, cache);
                e
            };
            if neutral {
                profile(
                    move |m| {
                        let mut e = pipeline(m);
                        e.time_neutral();
                        neutral_digests(e)
                    },
                    geometry,
                    &wanted,
                )
            } else {
                profile(pipeline, geometry, &wanted)
            }
        }
        "rtl" => {
            // `MUIR_SYNC_TICKS` runs QUUX's synchronous microcycle of that
            // many ticks, and `MUIR_CACHE` fits a memory cache of that many
            // words (H1a, H2).
            let ticks = std::env::var("MUIR_SYNC_TICKS").ok().and_then(|v| v.parse().ok());
            let cache = std::env::var("MUIR_CACHE")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(muir::cache::CacheConfig::with_words);
            // `MUIR_MEMORY_NS=<read>,<write>` times main memory as QUUX's
            // own, in place of the CADR's boards.
            let memory = std::env::var("MUIR_MEMORY_NS").ok().and_then(|v| {
                match v.as_str() {
                    "arty" => return Some(muir::cache::MemoryTiming::ARTY_Z7_20),
                    "de25" => return Some(muir::cache::MemoryTiming::DE25_NANO),
                    _ => {}
                }
                let (r, w) = v.split_once(',')?;
                Some(muir::cache::MemoryTiming {
                    read_ns: r.parse().ok()?,
                    write_ns: w.parse().ok()?,
                })
            });
            // `MUIR_PREFETCH`: QUUX's cache-only prefetch with another
            // reach, or none, as the module doc says.
            let prefetch = std::env::var("MUIR_PREFETCH").ok().map(|v| {
                use muir::memory_port::Reach;
                match v.as_str() {
                    "line" => Some(Reach::Line),
                    "page" => Some(Reach::Page),
                    "off" => None,
                    _ => panic!("MUIR_PREFETCH={v}: line, page or off"),
                }
            });
            profile(
                move |m| {
                    let on_quux = m.geometry.machine_id.is_some();
                    let mut e = Rtl::new(m);
                    if let Some(cycle_ticks) = ticks {
                        e.set_timing_model(muir::clock::TimingModel::Sync {
                            cycle_ticks,
                            ilong_ticks: 0,
                        });
                    }
                    e.set_cache(cache);
                    e.set_memory_timing(memory);
                    if let (true, Some(prefetch)) = (on_quux, prefetch) {
                        e.set_prefetch(prefetch);
                    }
                    e
                },
                geometry,
                &wanted,
            )
        }
        _ => {
            let micro = move |m: muir::machine::Machine| {
                let revision_14 = m.geometry.paged() && !m.geometry.extended();
                let mut e = Micro::new(m);
                if let Some(p) = period {
                    e.period = p;
                } else if revision_14 && neutral {
                    // Revision 14's neutral time at the pipeline's period for
                    // it, so that the two compare.
                    e.period = muir::pipeline::PERIOD_14;
                }
                e
            };
            if neutral {
                profile(
                    move |m| {
                        let mut e = micro(m);
                        e.time_neutral();
                        neutral_digests(e)
                    },
                    geometry,
                    &wanted,
                )
            } else {
                profile(micro, geometry, &wanted)
            }
        }
    }
}

/// The run's configuration in a line, at its head: the engine and the
/// machine, and every setting that changes what runs (MP2b rulings 2, F3).
fn configuration(
    engine: &str,
    geometry: muir::machine::Geometry,
    period: Option<u64>,
    neutral: bool,
) -> String {
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| "unset".into());
    let machine = match geometry.revision() {
        _ if geometry == muir::machine::Geometry::CADR => "the CADR".to_string(),
        Some(r) => {
            format!("QUUX revision {r}, a PDL buffer of {} words", geometry.pdl_mask() as u32 + 1)
        }
        None => "QUUX".to_string(),
    };
    let pipeline = engine == "rtl" && geometry.paged();
    let clock = if pipeline || (geometry.paged() && neutral) {
        let p = period.unwrap_or(if geometry.extended() {
            muir::clock::PERIOD_15
        } else {
            muir::pipeline::PERIOD_14
        });
        if geometry.extended() {
            format!("{} ns", muir::clock::microcycle_ns_text(p))
        } else {
            format!("{p} ns")
        }
    } else {
        format!("the engine's own (MUIR_SYNC_TICKS {})", env("MUIR_SYNC_TICKS"))
    };
    let prefetch = if pipeline {
        "none: the pipeline's fused returns use no prefetched word, as with MUIR_PREFETCH=off"
            .to_string()
    } else if engine == "rtl" && geometry != muir::machine::Geometry::CADR {
        format!("MUIR_PREFETCH {}, page when unset", env("MUIR_PREFETCH"))
    } else {
        "none".to_string()
    };
    format!(
        "configuration: engine {engine}; machine {machine}; band {}; PROM {}; microcode {}; main memory {}; period {clock}; memory timing {}; cache {}; prefetch {prefetch}; MUIR_H8A {}; time-neutral {neutral}; RTC {}",
        env("MUIR_BAND"),
        std::env::var("MUIR_PROM").unwrap_or_else(|_| "built in".into()),
        std::env::var("MUIR_UCODE").unwrap_or_else(|_| "the pack's".into()),
        std::env::var("MUIR_MAIN_MEMORY_SIZE").unwrap_or_else(|_| "2MW".into()),
        std::env::var("MUIR_MEMORY_NS").unwrap_or_else(|_| if pipeline {
            "kria".into()
        } else {
            "default".into()
        }),
        std::env::var("MUIR_CACHE").unwrap_or_else(|_| "default".into()),
        env("MUIR_H8A"),
        std::env::var("MUIR_RTC").unwrap_or_else(|_| if neutral {
            "counted from the fixed date".into()
        } else {
            "the host's".into()
        }),
    )
}

/// `e` under the time-neutral harness: its digests at every 65,536th
/// boundary and at each workload's end, printed as it runs.
fn neutral_digests<E: Neutral>(e: E) -> Digests<E> {
    let mut d = Digests::new(e, support::neutral::EVERY);
    d.print = true;
    d
}

fn profile<E: Profiled + support::macro_dispatch::Executes>(
    make: impl Fn(muir::machine::Machine) -> E,
    geometry: muir::machine::Geometry,
    wanted: &[&(&str, &str)],
) {
    let on_quux = geometry != muir::machine::Geometry::CADR;
    let dir = support::scratch("profile");
    // QUUX runs muir-sys's systems: `MUIR_BAND`, a directory holding a
    // band's GPT disk (a `.vhd`, or a raw `.img`) and the tree it was built
    // from, which unpacks to one `release-*` directory, such as System
    // 2001's release disk and its sources. The CADR runs the CADR's release.
    let quux_band = std::env::var_os("MUIR_BAND").map(PathBuf::from);
    let (pack, sources) = if on_quux && quux_band.is_none() {
        eprintln!("skipped: QUUX's band is MUIR_BAND's, a directory with its disk and its tree");
        return;
    } else if let Some(band) = quux_band.filter(|_| on_quux) {
        let file = |suffixes: &[&str]| {
            std::fs::read_dir(&band)
                .unwrap_or_else(|e| panic!("{}: {e}", band.display()))
                .map(|e| e.unwrap().path())
                .find(|p| suffixes.iter().any(|s| p.to_string_lossy().ends_with(s)))
                .unwrap_or_else(|| panic!("{}: no *{}", band.display(), suffixes.join(" or *")))
        };
        let untar = std::process::Command::new("tar")
            .arg("xzf")
            .arg(file(&[".tar.gz"]))
            .arg("-C")
            .arg(dir.path())
            .status()
            .unwrap();
        assert!(untar.success(), "the band's tree unpacks");
        let release = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("release-")))
            .expect("the band's tree unpacks to a release-* directory");
        (file(&[".vhd", ".img"]), release)
    } else {
        let (Some(pack), Some(sources)) =
            (support::vendor(&["run", "release-1003-pack.img"]), support::vendor(&["system-1003"]))
        else {
            return;
        };
        (pack, sources)
    };
    // The copy keeps the disk's extension, `.vhd` or `.img`; muir tells a
    // VHD by its footer either way.
    let copy = dir.join("pack").with_extension(pack.extension().unwrap_or_default());
    std::fs::copy(&pack, &copy).unwrap();
    let root = dir.join("root");
    let home = root.join("lispm");
    std::fs::create_dir_all(&home).unwrap();
    std::os::unix::fs::symlink(sources.join("site"), root.join("site")).unwrap();

    // Another microcode, when asked for: into MCR2, current, and its table
    // and symbols the ones served and read.
    let ucode = std::env::var_os("MUIR_UCODE").map(PathBuf::from);
    let sym_file = match &ucode {
        Some(u) => {
            if on_quux {
                // QUUX's `.mcr` is in partition order (contract Q8): the
                // file's bytes go into the current microcode partition as
                // they are, as `dd` writes them, through muir's disk layer,
                // which writes a VHD as well as a raw disk.
                let mcr = std::fs::read(u.join("ucadr.mcr")).unwrap();
                let mut d = muir::disk_image::Disk::open_rw(&copy).unwrap();
                let part = support::gpt_partitions(&mut d)
                    .into_iter()
                    .find(|p| p.current && p.name.starts_with("MCR"))
                    .expect("a current microcode partition");
                assert!(mcr.len() <= part.blocks as usize * 1024, "the microcode fits");
                for (k, block) in mcr.chunks(1024).enumerate() {
                    let mut words = [0u32; 256];
                    for (w, b) in words.iter_mut().zip(block.chunks(4)) {
                        let mut le = [0u8; 4];
                        le[..b.len()].copy_from_slice(b);
                        *w = u32::from_le_bytes(le);
                    }
                    assert!(d.write_block(part.first + k as u32, &words), "block {k} written");
                }
            } else {
                let (mut p, _) = Pack::open(&copy);
                p.run(Command::Load { partition: "MCR2".into(), file: Some(u.join("ucadr.mcr")) })
                    .unwrap();
                p.run(Command::Microload("MCR2".into())).unwrap();
            }
            let sys = root.join("sys");
            std::fs::create_dir_all(sys.join("ubin")).unwrap();
            for entry in std::fs::read_dir(sources.join("sys")).unwrap() {
                let entry = entry.unwrap();
                if entry.file_name() != "ubin" {
                    std::os::unix::fs::symlink(entry.path(), sys.join(entry.file_name())).unwrap();
                }
            }
            for entry in std::fs::read_dir(sources.join("sys/ubin")).unwrap() {
                let entry = entry.unwrap();
                if entry.file_name() != "ucadr.tbl" && entry.file_name() != "ucadr.sym" {
                    std::os::unix::fs::symlink(
                        entry.path(),
                        sys.join("ubin").join(entry.file_name()),
                    )
                    .unwrap();
                }
            }
            std::fs::copy(u.join("ucadr.tbl"), sys.join("ubin/ucadr.tbl")).unwrap();
            u.join("ucadr.sym")
        }
        None => {
            std::os::unix::fs::symlink(sources.join("sys"), root.join("sys")).unwrap();
            sources.join("sys/ubin/ucadr.sym")
        }
    };
    let syms = muir::sym::parse(&std::fs::read_to_string(&sym_file).unwrap()).unwrap();
    let qmlp = syms.address(Space::IMem, "QMLP").expect("QMLP in the symbol table");
    let files = label_files(&sources.join("sys/ucadr"));

    let mut m = if on_quux {
        // QUUX's own PROM at 36000, the pack on block-disk, the video controller, and
        // the file device serving the root as HOST's `/` and its `sys` and
        // `site` as `/sys` and `/site`, where the band's `SYS:` is.
        // `MUIR_MAIN_MEMORY_SIZE=<n>MW`: main memory in whole megawords, as
        // `--main-memory-size` takes it, within QUUX's range; 2MW if not
        // given.
        let boards =
            std::env::var("MUIR_MAIN_MEMORY_SIZE").map_or(muir::machine::MAIN_WORDS >> 16, |v| {
                let n = v
                    .strip_suffix("MW")
                    .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or_else(|| {
                        panic!(
                            "MUIR_MAIN_MEMORY_SIZE={v}: main memory is given in megawords, \
                             with the unit MW, such as 32MW"
                        )
                    });
                let range = geometry.main_memory_mw();
                assert!(
                    range.contains(&n),
                    "MUIR_MAIN_MEMORY_SIZE={v}: QUUX's main memory is {}MW to {}MW",
                    range.start(),
                    range.end()
                );
                n << 4
            });
        let mut m = muir::machine::Machine::with_geometry(geometry, boards);
        // QUUX's own PROM, PROM 2001, or `MUIR_PROM`'s file, as `--prom`
        // takes it: revision 15 has no PROM built in.
        let prom = match std::env::var_os("MUIR_PROM").map(PathBuf::from) {
            Some(file) => {
                let bytes =
                    std::fs::read(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
                muir::prom::parse_quux_mcr(&bytes, geometry)
                    .unwrap_or_else(|e| panic!("MUIR_PROM={}: {e}", file.display()))
            }
            None if geometry.extended() => {
                panic!(
                    "QUUX revision 15 has no built-in boot PROM: MUIR_PROM=<file>, a revision-15 .mcr"
                )
            }
            None => muir::prom::quux_boot_prom(),
        };
        m.load_prom(&prom);
        let mut d = muir::block_disk::BlockDisk::new(muir::block_disk::BLOCK_NS);
        d.attach(muir::disk_image::Disk::open_rw(&copy).unwrap());
        m.block_disk = Some(d);
        m.tv.set_board(muir::tv::Board::Video);
        m.file_device.mounts.add(&root.display().to_string()).unwrap();
        for part in ["sys", "site"] {
            m.file_device.mounts.add(&format!("{part}={}", root.join(part).display())).unwrap();
        }
        m
    } else {
        support::machine_with_pack(&copy)
    };
    m.geometry = geometry;
    if let Some(s) = std::env::var("MUIR_RTC").ok().and_then(|v| v.parse().ok()) {
        m.rtc = muir::machine::Rtc::Counted { start: s, base_ns: 0 };
    } else if support::neutral::on() {
        // The time-neutral harness counts the RTC, never the host's.
        m.rtc =
            muir::machine::Rtc::Counted { start: support::neutral::FIXED_UNIX as u32, base_ns: 0 };
    }
    let mut e = make(m);
    e.boot();
    // `MUIR_H8A`: the MACRO DISPATCH MEMORY filled from the microcode's
    // own opcode table and enabled with its main loop and bases, once it is
    // loaded, and the run watched by the checkers.
    match std::env::var("MUIR_H8A") {
        // The microcode fills the memory itself: the checkers watch from
        // its first main-loop return with the register enabled, after
        // `RESET-MACHINE` has written the bases.
        Ok(how) if on_quux && how == "microcode" => {
            while e.machine().macro_dispatch.register & muir::machine::macro_dispatch::ENABLE == 0
                || e.machine().opc != qmlp as u16
                || e.pc() >= muir::machine::QUUX_PROM_BASE
            {
                e.step().expect("halted before the main loop");
            }
            eprintln!(
                "microcode's fill enabled, register {:o}",
                e.machine().macro_dispatch.register
            );
            measure(
                support::macro_dispatch::Checked::new(e),
                &syms,
                &files,
                qmlp,
                root,
                home,
                wanted,
            )
        }
        Ok(how) if on_quux => {
            let at = |space, name| {
                syms.address(space, name).unwrap_or_else(|| panic!("{name} in the symbol table"))
            };
            let opdtb = at(Space::DMem, "OPDTB");
            let (localp, ap) = (at(Space::AMem, "A-LOCALP"), at(Space::MMem, "M-AP"));
            // Not while the PROM loads the microcode: on `rtl` a
            // control-store write's second microcycle stands at the address
            // written, and the PC goes back into the PROM after it.
            while e.machine().opc != qmlp as u16 || e.pc() >= muir::machine::QUUX_PROM_BASE {
                e.step().expect("halted before the main loop");
            }
            support::macro_dispatch::fill_generic(
                e.machine_mut(),
                qmlp as u16,
                opdtb as u16,
                localp as u16,
                ap as u8,
                how == "operand",
            );
            measure(
                support::macro_dispatch::Checked::new(e),
                &syms,
                &files,
                qmlp,
                root,
                home,
                wanted,
            )
        }
        _ => measure(e, &syms, &files, qmlp, root, home, wanted),
    }
}

/// The run from the boot on: the listener, the definitions, and the
/// workloads, each reported.
fn measure<E: Profiled>(
    mut e: E,
    syms: &Symbols,
    files: &BTreeMap<String, String>,
    qmlp: u32,
    root: PathBuf,
    home: PathBuf,
    wanted: &[&(&str, &str)],
) {
    // Neutral time runs the machine's waits at the period, many more
    // microcycles than its time otherwise gives them.
    let limit = if support::neutral::on() { 4_000_000_000 } else { 400_000_000 };
    let ran = support::boot_to_the_prompt_within(&mut e, CHAOS, root.clone(), limit);
    eprintln!("listener after {ran} microcycles");
    let mut k = Keyboard::new();
    let mut plain = |e: &mut E| {
        e.step().expect("halted");
    };
    type_echoed(&mut e, &mut k, "(login \"LISPM\" \"OZ\" t)", &mut plain);
    // No **MORE** break when the printing workloads fill the screen: it
    // waits for a key that nobody types.
    type_echoed(&mut e, &mut k, "(send terminal-io :set-more-p nil)", &mut plain);
    for d in DEFINITIONS {
        type_echoed(&mut e, &mut k, d, &mut plain);
    }
    // Compiled before any workload runs, so that each is measured compiled
    // whichever are asked for; `compile` and `compile-again` compile them
    // again.
    type_echoed(
        &mut e,
        &mut k,
        "(mapc #'compile '(w-done w-ack w-fib w-cons w-muldiv w-float w-array w-sort w-bignum w-intern w-print w-bitblt w-switch))",
        &mut plain,
    );
    let ready = home.join("ready.done");
    let p = run(&mut e, &mut k, "(w-done \"ready\")", &ready, syms);
    eprintln!("defined and logged in, {} microcycles", p.span.microcycles);
    // The generic handlers: `OPDTB`'s thirty-two entries' addresses.
    let generic: std::collections::BTreeSet<u16> = syms
        .address(Space::DMem, "OPDTB")
        .map(|o| (0..32).map(|k| (e.machine().dmem[(o + k) as usize] & 0o37777) as u16).collect())
        .unwrap_or_default();

    // A marker of its own for every run, so that a workload named twice is
    // run twice.
    let mut total: Option<Span> = None;
    for (n, (name, form)) in wanted.iter().enumerate() {
        let marker = home.join(format!("{name}-{n}.done"));
        let p =
            run(&mut e, &mut k, &format!("(progn {form} (w-done \"{name}-{n}\"))"), &marker, syms);
        report(name, &p, syms, files, qmlp, &generic);
        e.workload_end(&format!("{name}-{n}"));
        total = Some(total.map_or(p.span, |t| t.plus(p.span)));
        // `MUIR_PC_DUMP=<dir>`: every executed address's count and the
        // nanoseconds stalled at it, one file a workload.
        // `MUIR_OPS=<dir>`: the macroinstructions, one file a workload.
        if let Some(dir) = std::env::var_os("MUIR_OPS") {
            let mut out = format!(
                "# decode agrees: the halfword taken {}, the other {}\n",
                p.decode_agrees[0], p.decode_agrees[1]
            );
            let mut rows: Vec<_> = p.ops.iter().collect();
            rows.sort();
            for ((half, handler, cycles), n) in rows {
                out.push_str(&format!("{half:o} {handler:o} {cycles} {n}\n"));
            }
            std::fs::write(PathBuf::from(dir).join(format!("{name}.txt")), out).unwrap();
        }
        if let Some(dir) = std::env::var_os("MUIR_PC_DUMP") {
            let mut out = String::new();
            for (pc, &n) in p.hist.iter().enumerate().filter(|(_, n)| **n > 0) {
                out.push_str(&format!("{pc:o} {n} {}\n", p.stall_hist[pc]));
            }
            std::fs::write(PathBuf::from(dir).join(format!("{name}.txt")), out).unwrap();
        }
    }
    // The workloads' time together, each with its typing.
    if let Some(t) = total {
        println!("== the {} workloads together", wanted.len());
        println!("   {}", t.line());
    }
    // The whole run's counts, from the fill on: the boot, the login and
    // the definitions too.
    if e.checker().is_some() || e.prefetch_counts().is_some() {
        println!("== the whole run");
    }
    if let Some(c) = e.checker() {
        let label = |pc: u16| {
            syms.nearest(Space::IMem, pc as u32)
                .map(|(l, off)| format!("{l}+{off:o}"))
                .unwrap_or_else(|| "?".into())
        };
        println!("   checkers: {}, problems {}", c.counts.report(label), c.counts.problems());
        println!("   {}", handler_returns_line(&c.counts, &generic, label));
    }
    if let Some(c) = e.prefetch_counts() {
        println!("   {}", prefetch_line(&c));
    }
    for line in e.run_lines() {
        println!("== the whole run: {line}");
    }
}
