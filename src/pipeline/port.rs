// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **QUUX revision 15's memory side**, as `rtl`'s pipeline times it and
//! carries its words (contract G3 revision 15, §6, A15b.5 and A15b.6; its
//! MP2b rulings Q5-Q11 and F1).
//!
//! On revision 15 the port carries the words: main memory as the port sees
//! it lags the processor, a posted write's word reaching it at the write's
//! response, and the cache holds its lines' words, a write hit updating
//! them (MP2b ruling F1). Revisions 13 and 14 keep [`crate::memory_port`],
//! whose cache holds tags and whose words come from the machine at once.
//!
//! - **Posted writes.** A write start makes an entry in a queue of
//!   [`QUEUE`] (A15b.5): the 29-bit bus address and the 40-bit word. The
//!   port issues entries in order, one each occupancy interval, and frees
//!   an entry when the port accepts its write; the accepted write's line
//!   then stays on the in-flight list, at most [`IN_FLIGHT`], until its
//!   response, when the word lands in main memory. A write is not issued
//!   while the list is full.
//! - **The read rule.** A read that goes to main memory --- a cache miss's
//!   fill, a walk's or a write-back's read miss --- whose line has a write
//!   queued or in flight waits until every such write has its response.
//!   Every other read goes ahead of the queue. There is no forward from the
//!   queue.
//! - **The cache**: lines of [`LINE_WORDS`] words, two ways, write-through,
//!   allocated on a read miss only, one lookup at a time: a read miss holds
//!   it until its fill lands, and a write whose line has a fill in flight
//!   waits for the fill. Its size is the run's (`--cache`); a hit lands two
//!   clocks after the grant at every size (A15b.6, MP2b ruling Q8).
//! - **Timing**, in clocks of the period: a line fill [`PortTiming`]'s
//!   `read_ns`, the whole fill; a write's response `write_ns` after its
//!   accept; accepts `occupancy_ns` apart, each counted from the last
//!   (MP2b rulings Q2, Q5-Q7). Reads and writes are separate channels.
//! - **The file device's sweep**: a completion clears every valid bit in
//!   cache words / 512 clocks, and every lookup waits for it (A15b.6, MP2b
//!   ruling Q10).
//!
//! A seeded memory model for the checks, [`LateModel`], answers each write
//! 1 to 50 clocks late and lets a read pass a write accepted before it
//! (A15b's checks of posted writes).

use std::collections::VecDeque;

use crate::clock::TimeBase;
use crate::machine::{Machine, Word};

/// The posted writes' queue's entries (A15b.5).
pub const QUEUE: usize = 8;

/// The in-flight list's lines (A15b.5).
pub const IN_FLIGHT: usize = 16;

/// Words a line (G1 §4.1; MP2b ruling Q7): 40 bytes in packed storage.
pub const LINE_WORDS: u32 = 8;

/// The line a bus address is in, `key<28:3>` (A15b.5).
pub fn line_of(bus: u32) -> u32 {
    bus >> 3
}

/// **Main memory's timing on revision 15's port** (A15b.5): a line fill,
/// a write's response, and the occupancy, the interval between two
/// accepted writes, each in ns as M8c measured them at 100 MHz and turned
/// to clocks at the period (MP2b ruling Q5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortTiming {
    /// The whole line's fill, from the request to its last data: no beats
    /// term (MP2b ruling Q7).
    pub read_ns: u64,
    /// A write, from its accept to its response.
    pub write_ns: u64,
    /// The interval between two accepted writes.
    pub occupancy_ns: u64,
}

impl PortTiming {
    /// The Kria KR260's, `kria` (MP2b ruling Q6): M8c-board's idle 64-byte
    /// random line fill, 247 ns; AW to B, 130; the idle sequential single
    /// write's interval, 10, which the probe bounds.
    pub const KRIA: PortTiming = PortTiming { read_ns: 247, write_ns: 130, occupancy_ns: 10 };

    /// The Arty Z7-20's on revision 15, `arty`: M8c-board's idle figures
    /// in the same columns, 290, 120 and 16.
    pub const ARTY: PortTiming = PortTiming { read_ns: 290, write_ns: 120, occupancy_ns: 16 };

    /// The DE25-Nano's on revision 15, `de25`: today's model's whole line,
    /// 380 ns and three beats of 10; its write, 290; and the model's 40 ns
    /// occupancy. **Unverified**: the board is not measured (M8c's DE25
    /// half).
    pub const DE25: PortTiming = PortTiming { read_ns: 410, write_ns: 290, occupancy_ns: 40 };

    /// The preset `--memory-timing` names on revision 15.
    pub fn preset(name: &str) -> Option<PortTiming> {
        match name {
            "kria" => Some(PortTiming::KRIA),
            "arty" => Some(PortTiming::ARTY),
            "de25" => Some(PortTiming::DE25),
            _ => None,
        }
    }

    /// `<r>,<w>,<o>` in ns, each above 0, or a preset's name.
    pub fn parse(text: &str) -> Option<PortTiming> {
        if let Some(t) = Self::preset(text) {
            return Some(t);
        }
        let mut n = text.split(',').map(|v| v.parse::<u64>().ok().filter(|&v| v > 0));
        let t = PortTiming { read_ns: n.next()??, write_ns: n.next()??, occupancy_ns: n.next()?? };
        n.next().is_none().then_some(t)
    }

    /// The three in clocks of the period `time` gives, each rounded up on
    /// its own (MP2b ruling Q2): a fill, a write's response, an occupancy.
    pub fn clocks(self, time: TimeBase) -> Clocks {
        let c = |ns: u64| time.clocks(time.ns(ns)).max(1);
        Clocks { read: c(self.read_ns), write: c(self.write_ns), occupancy: c(self.occupancy_ns) }
    }
}

/// [`PortTiming`] in clocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clocks {
    pub read: u64,
    pub write: u64,
    pub occupancy: u64,
}

/// **The seeded memory model** for A15b's checks of posted writes: every
/// write answered 1 to 50 clocks after the time its accept and the write's
/// own time give, drawn from `seed`, and a read served at its fill's end
/// whatever writes were accepted before it, which is a read passing a
/// write. A test aid; a run has none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LateModel {
    pub seed: u64,
    /// The greatest lateness, in clocks (A15b: 50).
    pub most: u64,
    /// The planted error responses still to come: the write whose response
    /// is the next counts in register-page word 225 (A15b.1).
    pub errors: u32,
}

impl LateModel {
    fn next(&mut self) -> u64 {
        // xorshift64*: a fixed sequence per seed.
        let mut x = self.seed.max(1);
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.seed = x;
        (x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 33) % self.most.max(1) + 1
    }
}

/// One line of the cache: its line number and its words.
#[derive(Clone, Debug)]
struct Line {
    line: u32,
    words: [Word; LINE_WORDS as usize],
}

/// **The cache, holding its lines' words** (A15b.6): `words` in lines of
/// [`LINE_WORDS`], two ways a set, the least recently used of a set's two
/// replaced on a fill.
#[derive(Clone, Debug)]
pub struct Cache15 {
    words: u32,
    /// Two ways a set, `2 * set + way`.
    ways: Vec<Option<Box<Line>>>,
    /// The way of each set used last.
    mru: Vec<u8>,
    pub hits: u64,
    pub misses: u64,
}

impl Cache15 {
    /// A cache of `words`, a power of two of at least two lines.
    pub fn new(words: u32) -> Cache15 {
        assert!(
            words.is_power_of_two() && words >= 2 * LINE_WORDS,
            "a cache of {words} words: a power of two, two lines at least"
        );
        let sets = (words / LINE_WORDS / 2) as usize;
        Cache15 { words, ways: vec![None; 2 * sets], mru: vec![0; sets], hits: 0, misses: 0 }
    }

    pub fn words(&self) -> u32 {
        self.words
    }

    /// Sets: cache words / (8 words a line × 2 ways) (MP2b ruling Q7).
    pub fn sets(&self) -> u32 {
        self.words / LINE_WORDS / 2
    }

    fn set_of(&self, line: u32) -> usize {
        (line % self.sets()) as usize
    }

    fn way_of(&self, line: u32) -> Option<usize> {
        let s = self.set_of(line);
        (0..2).map(|w| 2 * s + w).find(|&k| self.ways[k].as_ref().is_some_and(|l| l.line == line))
    }

    /// Whether the cache holds the word at bus address `bus`.
    pub fn holds(&self, bus: u32) -> bool {
        self.way_of(line_of(bus)).is_some()
    }

    /// The word at `bus`, if held, and the set marked used.
    fn read(&mut self, bus: u32) -> Option<Word> {
        let k = self.way_of(line_of(bus))?;
        self.mru[k / 2] = (k % 2) as u8;
        Some(self.ways[k].as_ref()?.words[(bus % LINE_WORDS) as usize])
    }

    /// A write hit updates its line; a miss allocates nothing.
    fn write(&mut self, bus: u32, word: Word) {
        if let Some(k) = self.way_of(line_of(bus))
            && let Some(l) = self.ways[k].as_mut()
        {
            l.words[(bus % LINE_WORDS) as usize] = word;
        }
    }

    /// A fill installs `line`'s words in its set's least recently used way.
    fn install(&mut self, line: u32, words: [Word; LINE_WORDS as usize]) {
        let s = self.set_of(line);
        let k = match self.way_of(line) {
            Some(k) => k,
            None => 2 * s + (1 - self.mru[s] as usize),
        };
        self.ways[k] = Some(Box::new(Line { line, words }));
        self.mru[s] = (k % 2) as u8;
    }

    /// The set the word at `bus` maps to cleared, both ways: block-disk's
    /// written word's (coherence rule 2).
    pub fn clear_set(&mut self, bus: u32) {
        let s = self.set_of(line_of(bus));
        self.ways[2 * s] = None;
        self.ways[2 * s + 1] = None;
    }

    /// Every line invalid: the file device's completion, swept.
    pub fn invalidate(&mut self) {
        self.ways.iter_mut().for_each(|w| *w = None);
    }

    /// The lines held, `(index, line)`, for a checkpoint: their words are
    /// main memory's whenever the port is drained.
    fn held(&self) -> Vec<(u32, u32)> {
        self.ways
            .iter()
            .enumerate()
            .filter_map(|(k, w)| w.as_ref().map(|l| (k as u32, l.line)))
            .collect()
    }
}

/// A write the queue holds: its bus address and its word, the word `None`
/// until the microcycle after the start has fixed it (A15b.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Queued {
    bus: u32,
    word: Option<Word>,
    /// The start's own write, rather than a write-back's: what the
    /// processor's acknowledgment waits on.
    tag: u64,
}

/// An accepted write until its response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InFlight {
    bus: u32,
    /// The word, until it lands; `None` once block-disk's START has landed
    /// it early (MP2b ruling Q11).
    word: Option<Word>,
    respond_at: u64,
    error: bool,
}

/// What a fill is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reader {
    /// The processor's read, whose word goes to `MD`.
    Processor,
    /// A table read of a walk or a write-back, whose word the reader takes.
    Table,
}

/// A fill: the line, the word asked for, and when it was issued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fill {
    bus: u32,
    reader: Reader,
    /// When the fill was issued, once the read rule let it go.
    issued: Option<u64>,
    /// When it was asked for.
    asked: u64,
}

/// A read's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The word, at this clock.
    At(u64, Word),
    /// A fill is on its way: [`Port::filled`] has its word.
    Filling,
    /// The cache is busy, with a fill or the sweep: asked again next clock.
    Busy,
}

/// The port's meters, for the profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PortMeters {
    /// Reads that waited on the read rule, and the clocks they waited.
    pub read_rule_reads: u64,
    pub read_rule_clocks: u64,
    /// Clocks a write start waited for a full queue.
    pub queue_full_clocks: u64,
    /// Writes accepted, and fills.
    pub writes: u64,
    pub fills: u64,
    /// The file device's sweeps, and clocks lookups waited on one.
    pub sweeps: u64,
    pub sweep_clocks: u64,
}

/// **Revision 15's port**: the cache, the queue, the in-flight list and
/// the one fill.
#[derive(Clone, Debug)]
pub struct Port {
    timing: PortTiming,
    clocks: Clocks,
    pub cache: Cache15,
    queue: VecDeque<Queued>,
    inflight: VecDeque<InFlight>,
    /// The earliest clock the next write may be accepted at: the last
    /// accept's plus the occupancy.
    next_accept: u64,
    fill: Option<Fill>,
    /// The word the last fill read, and when, for its reader.
    filled: Option<(u64, Word)>,
    /// Until when the file device's sweep runs.
    sweep_until: u64,
    /// The test aid, when a test fits it.
    pub model: Option<LateModel>,
    /// The read rule kept, the default; a test's mutation turns it to
    /// waiting only for the line's writes to be issued.
    pub read_rule_waits_for_responses: bool,
    /// A write whose line has a fill in flight waits for the fill, the
    /// default; a test's mutation lets it go ahead.
    pub write_waits_for_fill: bool,
    pub meters: PortMeters,
    /// Errors answered, for register-page word 225, taken by the machine.
    errors: u32,
    /// Tags for the queue's entries.
    next_tag: u64,
    /// The line the last fill installed: a test aid, for a sweep planted
    /// to miss its set.
    pub last_filled: Option<u32>,
}

impl Port {
    /// A port of `timing` at the period `time` gives, with a cache of
    /// `cache_words`.
    pub fn new(timing: PortTiming, time: TimeBase, cache_words: u32) -> Port {
        Port {
            timing,
            clocks: timing.clocks(time),
            cache: Cache15::new(cache_words),
            queue: VecDeque::new(),
            inflight: VecDeque::new(),
            next_accept: 0,
            fill: None,
            filled: None,
            sweep_until: 0,
            model: None,
            read_rule_waits_for_responses: true,
            write_waits_for_fill: true,
            meters: PortMeters::default(),
            errors: 0,
            next_tag: 1,
            last_filled: None,
        }
    }

    pub fn timing(&self) -> PortTiming {
        self.timing
    }

    pub fn clocks(&self) -> Clocks {
        self.clocks
    }

    /// The queue and the in-flight list are empty: every write answered.
    pub fn empty(&self) -> bool {
        self.queue.is_empty() && self.inflight.is_empty()
    }

    /// Nothing at all in flight: drained, no fill, no sweep after `now`.
    pub fn idle(&self, now: u64) -> bool {
        self.empty() && self.fill.is_none() && self.sweep_until <= now
    }

    /// Entries queued, and lines in flight.
    pub fn depths(&self) -> (usize, usize) {
        (self.queue.len(), self.inflight.len())
    }

    /// The word at the physical address `phys` of main memory as every
    /// write the processor has made leaves it: main memory's word, or the
    /// newest write to it queued or in flight. What D dispatches on, as the
    /// stream's fetch of it will read it (the read rule keeps the fetch
    /// behind those writes).
    pub fn coherent(&self, m: &Machine, phys: u32) -> Word {
        let queued = self.queue.iter().rev().find(|q| q.bus == phys).and_then(|q| q.word);
        let inflight = || self.inflight.iter().rev().find(|f| f.bus == phys).and_then(|f| f.word);
        queued.or_else(inflight).unwrap_or_else(|| peek(m, phys))
    }

    /// The errors answered since the last call, for word 225.
    pub fn take_errors(&mut self) -> u32 {
        std::mem::take(&mut self.errors)
    }

    /// Whether `line` has a write queued or in flight.
    fn line_written(&self, line: u32) -> bool {
        self.queue.iter().any(|q| line_of(q.bus) == line)
            || self.inflight.iter().any(|f| f.word.is_some() && line_of(f.bus) == line)
    }

    /// Whether `line` has a write queued: the issue the read rule's broken
    /// form waits for alone.
    fn line_queued(&self, line: u32) -> bool {
        self.queue.iter().any(|q| line_of(q.bus) == line)
    }

    /// **The clock `now` begins**: the writes answered by now land in main
    /// memory, in order, one write is accepted if it may be, and a fill
    /// issued or ended. Called once a clock, before the stages.
    pub fn tick(&mut self, m: &mut Machine, now: u64) {
        while let Some(f) = self.inflight.front().copied()
            && f.respond_at <= now
        {
            self.inflight.pop_front();
            if let Some(word) = f.word {
                store(m, f.bus, word);
            }
            if f.error {
                self.errors += 1;
            }
        }
        if now >= self.next_accept
            && self.inflight.len() < IN_FLIGHT
            && let Some(q) = self.queue.front().copied()
            && let Some(word) = q.word
        {
            self.queue.pop_front();
            let late = self.model.as_mut().map_or(0, LateModel::next);
            let error = self.model.as_mut().is_some_and(|mm| {
                let e = mm.errors > 0;
                mm.errors = mm.errors.saturating_sub(1);
                e
            });
            self.inflight.push_back(InFlight {
                bus: q.bus,
                word: Some(word),
                respond_at: now + self.clocks.write + late,
                error,
            });
            self.next_accept = now + self.clocks.occupancy;
            self.meters.writes += 1;
        }
        if let Some(mut f) = self.fill {
            let line = line_of(f.bus);
            if f.issued.is_none() {
                let wait = if self.read_rule_waits_for_responses {
                    self.line_written(line)
                } else {
                    self.line_queued(line)
                };
                if wait {
                    self.meters.read_rule_clocks += 1;
                } else {
                    f.issued = Some(now);
                    if now > f.asked {
                        self.meters.read_rule_reads += 1;
                    }
                }
                self.fill = Some(f);
            }
            if let Some(at) = f.issued
                && now >= at + self.clocks.read
            {
                // The line's words as main memory holds them now: a write
                // whose response has not come is not in them.
                let base = line * LINE_WORDS;
                let mut words = [0; LINE_WORDS as usize];
                for (k, w) in words.iter_mut().enumerate() {
                    *w = load(m, base + k as u32);
                }
                self.cache.install(line, words);
                self.last_filled = Some(line);
                self.filled = Some((now, words[(f.bus % LINE_WORDS) as usize]));
                self.fill = None;
                self.meters.fills += 1;
            }
        }
    }

    /// Whether a lookup may be made at `now`: no fill in flight, no sweep.
    pub fn may_look(&mut self, now: u64) -> bool {
        if self.sweep_until > now {
            self.meters.sweep_clocks += 1;
            return false;
        }
        self.fill.is_none()
    }

    /// **A read of the cacheable bus address `bus`** granted at `now`: a
    /// hit's word two clocks after the grant (A15b.6); a miss's fill, the
    /// read rule first; or busy, the cache serving another lookup.
    pub fn read(&mut self, now: u64, bus: u32, reader: Reader) -> Answer {
        if !self.may_look(now) {
            return Answer::Busy;
        }
        if let Some(w) = self.cache.read(bus) {
            self.cache.hits += 1;
            return Answer::At(now + 2, w);
        }
        self.cache.misses += 1;
        // The tag is compared in the clock after the grant; the fill is
        // asked for then.
        self.fill = Some(Fill { bus, reader, issued: None, asked: now + 1 });
        self.filled = None;
        Answer::Filling
    }

    /// The word the last fill read, once it has: when, and the word.
    pub fn filled(&mut self) -> Option<(u64, Word)> {
        if self.fill.is_some() { None } else { self.filled.take() }
    }

    /// Whether a fill is in flight for `reader`.
    pub fn filling(&self, reader: Reader) -> bool {
        self.fill.is_some_and(|f| f.reader == reader)
    }

    /// **A write start to `bus`** granted at `now`: `None` when it must
    /// wait --- its line's fill in flight, or a full queue --- and
    /// otherwise the entry's tag. The entry is queued now and takes its
    /// word from [`Port::word`]; the start is acknowledged two clocks after
    /// the grant (A15b.5).
    pub fn write(&mut self, now: u64, bus: u32) -> Option<u64> {
        let behind_fill =
            self.fill.is_some_and(|f| line_of(f.bus) == line_of(bus)) && self.write_waits_for_fill;
        if behind_fill || self.sweep_until > now {
            return None;
        }
        if self.queue.len() >= QUEUE {
            self.meters.queue_full_clocks += 1;
            return None;
        }
        let tag = self.next_tag;
        self.next_tag += 1;
        self.queue.push_back(Queued { bus, word: None, tag });
        Some(tag)
    }

    /// A write-back's write, posted ahead of its reference's (A14.6): its
    /// word known. Waits, as [`Port::write`], when the queue is full.
    pub fn post(&mut self, bus: u32, word: Word) -> bool {
        if self.queue.len() >= QUEUE {
            self.meters.queue_full_clocks += 1;
            return false;
        }
        self.cache.write(bus, word);
        let tag = self.next_tag;
        self.next_tag += 1;
        self.queue.push_back(Queued { bus, word: Some(word), tag });
        true
    }

    /// The word of the queued write `tag`, fixed: the cache's line, if it
    /// holds the word, takes it with it.
    pub fn word(&mut self, tag: u64, word: Word) {
        if let Some(q) = self.queue.iter_mut().find(|q| q.tag == tag) {
            q.word = Some(word);
            let bus = q.bus;
            self.cache.write(bus, word);
        }
    }

    /// **Block-disk's START** (MP2b ruling Q11): every write queued or in
    /// flight lands in main memory now, no clock passing; their entries run
    /// on without their words.
    pub fn land_all(&mut self, m: &mut Machine) {
        for q in &mut self.queue {
            if let Some(w) = q.word.take() {
                store(m, q.bus, w);
            }
        }
        // A queued entry whose word has landed is accepted and answered as
        // ever; it carries nothing more.
        self.queue.retain(|q| q.word.is_some());
        for f in &mut self.inflight {
            if let Some(w) = f.word.take() {
                store(m, f.bus, w);
            }
        }
    }

    /// The file device's completion at `now`: the sweep (MP2b ruling Q10).
    /// `missing`, a test aid, leaves the set of that line as it is.
    pub fn sweep(&mut self, now: u64, missing: Option<u32>) {
        let keep = missing.map(|line| {
            let s = self.cache.set_of(line);
            (s, [self.cache.ways[2 * s].take(), self.cache.ways[2 * s + 1].take()])
        });
        self.cache.invalidate();
        if let Some((s, [a, b])) = keep {
            self.cache.ways[2 * s] = a;
            self.cache.ways[2 * s + 1] = b;
        }
        self.sweep_until = now + (self.cache.words() / 512).max(1) as u64;
        self.meters.sweeps += 1;
    }

    /// Everything dropped: a power-on or `-RESET` of the port. The writes
    /// in flight land.
    pub fn drain_now(&mut self, m: &mut Machine) {
        self.land_all(m);
        self.queue.clear();
        self.inflight.clear();
        self.fill = None;
        self.filled = None;
    }

    /// The port into a checkpoint, taken drained: the timing, and the
    /// cache's lines (their words are main memory's).
    pub fn save(&self, w: &mut crate::checkpoint::Writer) {
        w.u64(self.timing.read_ns);
        w.u64(self.timing.write_ns);
        w.u64(self.timing.occupancy_ns);
        w.u32(self.cache.words());
        let held = self.cache.held();
        w.u32(held.len() as u32);
        for (k, line) in held {
            w.u32(k);
            w.u32(line);
        }
        w.bytes(&self.cache.mru);
    }

    /// Back from a checkpoint: refused when it was taken at another timing
    /// or cache (MP2b ruling Q16); the lines' words read again from main
    /// memory.
    pub fn load(&mut self, m: &Machine, r: &mut crate::checkpoint::Reader) -> std::io::Result<()> {
        let timing = PortTiming { read_ns: r.u64()?, write_ns: r.u64()?, occupancy_ns: r.u64()? };
        if timing != self.timing {
            return Err(crate::checkpoint::bad(format!(
                "a memory timing of {},{},{}, and this run's is {},{},{}: --memory-timing {},{},{}",
                timing.read_ns,
                timing.write_ns,
                timing.occupancy_ns,
                self.timing.read_ns,
                self.timing.write_ns,
                self.timing.occupancy_ns,
                timing.read_ns,
                timing.write_ns,
                timing.occupancy_ns
            )));
        }
        let words = r.u32()?;
        if words != self.cache.words() {
            return Err(crate::checkpoint::bad(format!(
                "a cache of {words} words, and this run's is {}: --cache {words}",
                self.cache.words()
            )));
        }
        let n = r.u32()?;
        self.cache.invalidate();
        for _ in 0..n {
            let (k, line) = (r.u32()? as usize, r.u32()?);
            if k >= self.cache.ways.len() {
                return Err(crate::checkpoint::bad(format!("a cache way {k}")));
            }
            let base = line * LINE_WORDS;
            let mut lw = [0; LINE_WORDS as usize];
            for (i, w) in lw.iter_mut().enumerate() {
                *w = peek(m, base + i as u32);
            }
            self.cache.ways[k] = Some(Box::new(Line { line, words: lw }));
        }
        let mru = r.bytes()?;
        if mru.len() != self.cache.mru.len() {
            return Err(crate::checkpoint::bad("a cache's recency of another size"));
        }
        self.cache.mru = mru;
        self.queue.clear();
        self.inflight.clear();
        self.fill = None;
        self.filled = None;
        self.sweep_until = 0;
        Ok(())
    }
}

/// The word main memory or the frame buffer holds at the bus address
/// `bus`, as a line fill reads it: main memory's word, the frame buffer's
/// field with a fixnum's tag (G1 §4.2), 0 where nothing is.
fn load(m: &mut Machine, bus: u32) -> Word {
    peek(m, bus)
}

/// [`load`] with no side effect.
pub fn peek(m: &Machine, bus: u32) -> Word {
    if bus & crate::tlb::DEVICE != 0 {
        let off = bus & !crate::tlb::DEVICE;
        if off < m.tv.buffer_words() {
            crate::machine::UNBOXED_TAG | Word::from(m.tv.read_buffer(off))
        } else {
            0
        }
    } else {
        m.main.get(bus as usize).copied().unwrap_or(0)
    }
}

/// A posted write's word landing at the bus address `bus`: main memory
/// takes the whole word, the frame buffer the field (G1 §4.2).
fn store(m: &mut Machine, bus: u32, word: Word) {
    m.bus_write(bus, word);
}
