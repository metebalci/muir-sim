// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's memory port (contract Q6, revision 7) and register decode
//! (contract Q7, revision 8), in place of the CADR's bus interface
//! ([`crate::busint`]), as `rtl` times them.
//!
//! The processor's cycle goes one of three ways, by its physical address:
//!
//! - **The memory bus**: main memory, and the frame buffer (Q7), through
//!   the cache ([`crate::cache`], always fitted on QUUX) to the memory
//!   controller. A read hit is answered after the cache's hit time; a miss
//!   fills its line in [`MemoryTiming::read_ns`] and a write takes
//!   [`MemoryTiming::write_ns`], one operation at a time, the write buffer
//!   acknowledging a write after the hit time and running it behind the
//!   processor. No memory boards, no setup or deskew, no refresh. The
//!   nominal timing is a floor: a board slower on an access waits, muir
//!   answers at it.
//! - **A device register**, never cached: a word of the register page at
//!   `1777777400`, the video controller's and block-disk's among them
//!   (contract Q13). There is no bus: the register decode takes the cycle
//!   at the edge and answers it a microcycle on, a register access taking
//!   two microcycles in all.
//! - **Nothing**, past main memory's end below the frame buffer window,
//!   or past the frame buffer's end in it: a decode miss, failing at once,
//!   with the NXM bit. No timeout.
//!
//! There is nothing to arbitrate: the processor is the only requester in
//! muir. Block-disk moves its words at START, which its contract allows,
//! and invalidates the cache ([`crate::machine::Machine::dma_written`]).
//!
//! The words themselves come from [`crate::machine::Machine`], as with the
//! bus interface; the port says only when.
//!
//! **The cache-only prefetch** (contract H8a §3.5, with page reach, G2
//! §9): when a macroinstruction fetch's read is answered from main memory
//! at physical word `p`, the word at `p + 1` is taken into a one-word
//! buffer with its virtual and physical addresses, if it is in the line
//! the fetch has just read or filled, whose words the fabric's cache puts
//! out together, or in the next line when the cache holds it, never past
//! the page ([`Reach::Page`], a second cache read port). Nothing else
//! happens: no memory cycle, no map lookup and no arbitration, so it
//! cannot fault. The lines are 8 words, the pages 1024 and the virtual
//! addresses 28 bits ([`LINE_WORDS`], [`PAGE_WORDS`]). [`Reach::Line`],
//! the line alone, is a measurement's option. The buffer is dropped by a write of the location counter, a store
//! to its word, a transfer by block-disk or the file device, a map write,
//! and -RESET; a checkpoint keeps it, with a fetch's address the port is
//! yet to answer. What uses it is the fused return
//! (`crate::machine::macro_dispatch`), in `rtl` alone: `micro` has no
//! cache, and no prefetch.

use crate::busint::{Ack, Responder};
use crate::cache::{Cache, CacheConfig, MemoryTiming};
use crate::clock::TimingModel;

/// How far the prefetch looks for the next word (contract H8a §3.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// Only in the line the fetch read: the next word when the fetched one
    /// is not its line's last. The fabric's cache has that line's words out
    /// of its RAMs already, so this needs no second read of them. Not
    /// QUUX's: a measurement's, selectable by [`MemoryPort::set_prefetch`].
    Line,
    /// Anywhere in the fetched word's page that the cache holds: the next
    /// line's word is a lookup of its own, a second read port. QUUX's
    /// (contract G2 §9; measured at 8-word lines, 1.67% of time on the
    /// Arty's timing and 1.13% on the DE25's).
    Page,
}

/// The word the prefetch holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefetched {
    /// Its virtual word address, `VMA<27:0>` as the stream's fetch of it
    /// would give it: `LC<29:2>`.
    pub vaddr: u32,
    /// Its physical word address.
    pub phys: u32,
    /// The word, as main memory held it when it was taken.
    pub word: crate::machine::Word,
}

/// What dropped the buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drop {
    /// A write of the location counter.
    LcWrite,
    /// A store to its word.
    Store,
    /// A transfer by block-disk or the file device.
    Dma,
    /// A map write.
    MapWrite,
    /// -RESET.
    Reset,
}

/// The prefetch's counts, for the profile and the tests; not in a
/// checkpoint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrefetchCounts {
    /// Fetches answered from main memory.
    pub fetches: u64,
    /// Words taken: in the fetch's own line, and in the next line.
    pub same_line: u64,
    pub next_line: u64,
    /// Next words not taken: past the page's end, or not in the cache (for
    /// [`Reach::Line`], every next word in another line).
    pub page_end: u64,
    pub not_held: u64,
    /// Buffers dropped, by [`Drop`]'s kinds in order, a buffer counted
    /// once: by what dropped it first.
    pub dropped: [u64; 5],
    /// Fused returns that took the buffered word.
    pub used: u64,
    /// Returns that would have fused on the buffered word but for a
    /// condition of the fetch path: condition 6 true, and a store started
    /// in the microcycle before, whose compare with the word's address is
    /// yet to be made.
    pub refused: [u64; 2],
}

/// Where the port's cycle is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    /// Requested, to be taken at the next edge.
    Requested,
    /// Taken: answered at `answered`, acknowledged at `ack`.
    Granted {
        ack: u64,
        answered: u64,
        timed_out: bool,
    },
    /// Acknowledged, until the processor lets the request go.
    Acked {
        at: u64,
        answered: u64,
        timed_out: bool,
    },
}

#[derive(Clone, Debug)]
pub struct MemoryPort {
    state: State,
    write: bool,
    /// The physical address of the cycle, for the cache.
    addr: u32,
    /// The cycle is main memory's: no Xbus time between the answer and the
    /// data paths.
    memory: bool,
    cache: Cache,
    timing: MemoryTiming,
    /// When main memory is free for its next operation.
    memory_free_at: u64,
    /// When the write buffer is free again.
    buffer_free_at: u64,
    /// Revision 14: when the port has done a reference's write-back, which
    /// precedes its cycle (A14.6). Kept in a checkpoint of revision 14.
    write_back_until: u64,
    /// Whose clock the timeout's oscillator is measured on: the engine's,
    /// [`MemoryPort::keep_timing_model`]. Not in a checkpoint.
    model: TimingModel,
    /// The prefetch, if fitted, its word, the cycle's virtual word address
    /// when it is a macroinstruction fetch, and its counts. A checkpoint
    /// keeps the word and the address; the reach is the engine's, as the
    /// timing model is, and the counts are the profile's.
    prefetch: Option<Reach>,
    prefetched: Option<Prefetched>,
    fetch_vaddr: Option<u32>,
    pub prefetch_counts: PrefetchCounts,
    /// Revision 14's port ([`MemoryPort::for_geometry`]): 32-bit virtual
    /// addresses.
    paged: bool,
}

/// Words a cache line (contract G1 §4.1, G2 §3): a line of packed
/// storage, which [`MemoryPort::set_cache`] keeps whatever size is asked
/// for.
pub const LINE_WORDS: u32 = 8;

/// Words a page, which the prefetch never looks past.
pub const PAGE_WORDS: u32 = 1024;

/// The virtual word address's bits: 28.
const VADDR_MASK: u32 = 0x0fff_ffff;

/// Revision 14's virtual word address's bits: 32 (G3 revision 14,
/// A14.11). Its frame buffer is in the device window, its bus addresses'
/// `<28>` set (A14.1).
const VADDR_MASK_14: u32 = 0xffff_ffff;

/// The cache QUUX's port fits for `config`: its 8-word line, whatever
/// line `config` asks for.
pub fn fitted(config: CacheConfig) -> CacheConfig {
    CacheConfig { line_words: LINE_WORDS, ..config }
}

impl Default for MemoryPort {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryPort {
    /// QUUX's port: the cache at its shape with 8-word lines, main memory
    /// at the nominal timing, and the prefetch with page reach.
    pub fn new() -> MemoryPort {
        Self::with_paged(false)
    }

    /// The port a machine of `geometry` has: [`new`]'s, over 32-bit
    /// virtual addresses on revision 14 ([`crate::machine::Geometry::paged`]).
    ///
    /// [`new`]: MemoryPort::new
    pub fn for_geometry(geometry: &crate::machine::Geometry) -> MemoryPort {
        Self::with_paged(geometry.paged())
    }

    fn with_paged(paged: bool) -> MemoryPort {
        MemoryPort {
            state: State::Idle,
            write: false,
            addr: 0,
            memory: false,
            cache: Cache::new(fitted(CacheConfig::QUUX)),
            timing: MemoryTiming::NOMINAL,
            memory_free_at: 0,
            buffer_free_at: 0,
            write_back_until: 0,
            model: TimingModel::Sync { cycle_ticks: 4, ilong_ticks: 0 },
            prefetch: Some(Reach::Page),
            prefetched: None,
            fetch_vaddr: None,
            prefetch_counts: PrefetchCounts::default(),
            paged,
        }
    }

    /// The virtual word address's bits: 28, and 32 on revision 14.
    fn vaddr_mask(&self) -> u32 {
        if self.paged { VADDR_MASK_14 } else { VADDR_MASK }
    }

    /// The cache-only prefetch fitted with another reach, or taken out, for
    /// a measurement. A word held is dropped.
    pub fn set_prefetch(&mut self, prefetch: Option<Reach>) {
        self.prefetch = prefetch;
        self.prefetched = None;
    }

    pub fn prefetch(&self) -> Option<Reach> {
        self.prefetch
    }

    /// The word the prefetch holds, if it holds one.
    pub fn prefetched(&self) -> Option<Prefetched> {
        self.prefetched
    }

    /// The cycle just requested is the stream's macroinstruction fetch of
    /// virtual word `vaddr`.
    pub fn mark_fetch(&mut self, vaddr: u32) {
        self.fetch_vaddr = Some(vaddr & self.vaddr_mask());
    }

    /// The buffer dropped, if it holds a word, and counted by `why`.
    pub fn drop_prefetched(&mut self, why: Drop) {
        if self.prefetched.take().is_some() {
            self.prefetch_counts.dropped[why as usize] += 1;
        }
    }

    /// The prefetch's part of a read answered: when the cycle was a
    /// macroinstruction fetch from main memory, of `main`'s words, the next
    /// word is taken if the cache holds it in the same page, and the
    /// buffer is emptied otherwise. Called once the port has acknowledged
    /// the read, whose line the cache then holds.
    pub fn read_answered(&mut self, main: &[crate::machine::Word]) {
        let Some(vaddr) = self.fetch_vaddr.take() else { return };
        let Some(reach) = self.prefetch else { return };
        if !self.memory || self.write {
            return;
        }
        self.prefetched = None;
        let c = &mut self.prefetch_counts;
        c.fetches += 1;
        let next = self.addr + 1;
        let line = self.cache.config.line_words;
        if next.is_multiple_of(PAGE_WORDS) || next as usize >= main.len() {
            c.page_end += 1;
            return;
        }
        let same_line = !next.is_multiple_of(line);
        if !same_line && (reach == Reach::Line || !self.cache.holds(next)) {
            c.not_held += 1;
            return;
        }
        if same_line {
            c.same_line += 1;
        } else {
            c.next_line += 1;
        }
        self.prefetched = Some(Prefetched {
            vaddr: (vaddr + 1) & self.vaddr_mask(),
            phys: next,
            word: main[next as usize],
        });
    }

    pub fn cache(&self) -> &Cache {
        &self.cache
    }

    /// Another shape of cache, before the machine runs: `--cache`. The
    /// line stays 8 words, a line of packed storage (G1 §4.1).
    pub fn set_cache(&mut self, config: CacheConfig) {
        self.cache = Cache::new(fitted(config));
    }

    pub fn memory_timing(&self) -> MemoryTiming {
        self.timing
    }

    /// Other figures for main memory, before the machine runs:
    /// `--memory-timing`.
    pub fn set_memory_timing(&mut self, timing: MemoryTiming) {
        self.timing = timing;
    }

    /// Drops everything the cache holds: main memory was written by
    /// something else, the disk.
    pub fn invalidate_cache(&mut self) {
        self.cache.invalidate();
        self.drop_prefetched(Drop::Dma);
    }

    /// When the write buffer is empty: main memory has done the last write
    /// it acknowledged early. Before the first write, 0.
    pub fn write_buffer_empty_at(&self) -> u64 {
        self.buffer_free_at
    }

    pub fn keep_timing_model(&mut self, model: TimingModel) {
        self.model = model;
    }

    pub fn timing_model(&self) -> TimingModel {
        self.model
    }

    /// The processor asks for a cycle at physical address `phys`.
    pub fn request_at(&mut self, write: bool, phys: u32) {
        debug_assert_eq!(self.state, State::Idle, "a cycle is already running");
        self.state = State::Requested;
        self.write = write;
        self.addr = phys;
        self.memory = false;
        self.fetch_vaddr = None;
        if write && self.prefetched.is_some_and(|p| p.phys == phys) {
            self.drop_prefetched(Drop::Store);
        }
    }

    /// An edge of the processor's clock: a cycle requested is taken here.
    pub fn mclk_edge(&mut self, now: u64, responder: Responder) {
        if self.state != State::Requested {
            return;
        }
        self.memory = matches!(responder, Responder::Memory(_));
        let (at, timed_out) = if self.memory {
            (self.memory_cycle(now), false)
        } else if responder == Responder::Device {
            // A device register (contract Q7): taken at this edge, and
            // answered a microcycle on, with no setup or deskew.
            let cycle = self.model.cycle_ns(crate::clock::Speed::Normal, false) as u64;
            self.state = State::Granted { ack: now + cycle, answered: now, timed_out: false };
            return;
        } else {
            // Nothing there: a decode miss, failing at once.
            (now, true)
        };
        self.state = State::Granted { ack: at, answered: at, timed_out };
    }

    /// Main memory's answer to the cycle taken at `now`: a hit after the
    /// hit time; a miss or a write when main memory has done it, a buffered
    /// write after the hit time or when the buffer is free.
    fn memory_cycle(&mut self, now: u64) -> u64 {
        // Revision 14: after the reference's write-back (A14.6).
        let now = now.max(self.write_back_until);
        let hit_ns = self.cache.config.hit_ns;
        if !self.write && self.cache.read(self.addr) {
            return now + hit_ns;
        }
        let start = now.max(self.memory_free_at);
        let done = start + if self.write { self.timing.write_ns } else { self.fill_ns() };
        self.memory_free_at = done;
        if self.write && self.cache.config.write_buffer {
            let at = (now + hit_ns).max(self.buffer_free_at);
            self.buffer_free_at = done;
            at
        } else {
            done
        }
    }

    /// A line fill's time: the board's [`MemoryTiming::read_ns`], which is
    /// two 64-bit beats', and a tick more for each further beat --- 40
    /// bytes, 5 beats, in main memory, and 32 bytes, 4, in the frame buffer
    /// window (G1 §4.1-§4.2). A line that
    /// crosses a 4 KiB boundary, which the fabric issues as two bursts (4
    /// in every 512), is taken as one: **unverified**, until muir-fpga
    /// measures a fill at 40 bits (D5).
    fn fill_ns(&self) -> u64 {
        self.fill_ns_at(self.addr)
    }

    /// [`MemoryPort::fill_ns`] of the line at `addr`: on revision 14 the
    /// frame buffer's bus addresses, `<28>` set, lie above the window's
    /// revision-13 base too.
    fn fill_ns_at(&self, addr: u32) -> u64 {
        const TICK: u64 = 10;
        let beats = if addr >= crate::machine::WINDOW_13 { 2 } else { 3 };
        self.timing.read_ns + beats * TICK
    }

    /// **One read of revision 14's walk** (A14.6) at main memory's word
    /// `phys`, from `now`: through the cache, a hit answered after its hit
    /// time and a miss filling its line when main memory is free, as a
    /// processor's read is. When it is done.
    pub fn walk_read(&mut self, now: u64, phys: u32) -> u64 {
        if self.cache.read(phys) {
            return now + self.cache.config.hit_ns;
        }
        let done = now.max(self.memory_free_at) + self.fill_ns_at(phys);
        self.memory_free_at = done;
        done
    }

    /// **The write of revision 14's write-back** (A14.6) at main memory's
    /// word `phys`, from `now`: through the write buffer, as every write is,
    /// answered after the hit time or when the buffer is free, main memory
    /// doing it behind. When it is answered.
    pub fn walk_write(&mut self, now: u64, phys: u32) -> u64 {
        let _ = phys;
        let done = now.max(self.memory_free_at) + self.timing.write_ns;
        self.memory_free_at = done;
        if self.cache.config.write_buffer {
            let at = (now + self.cache.config.hit_ns).max(self.buffer_free_at);
            self.buffer_free_at = done;
            at
        } else {
            done
        }
    }

    /// The reference's cycle, requested next, is taken no sooner than `at`,
    /// when its write-back is done (A14.6).
    pub fn write_back_until(&mut self, at: u64) {
        self.write_back_until = self.write_back_until.max(at);
    }

    /// Advances to `now` and reports the acknowledgement if it has come.
    pub fn poll(&mut self, now: u64, responder: Responder) -> Option<Ack> {
        if let State::Granted { ack, answered, timed_out } = self.state
            && now >= ack
        {
            self.state = State::Acked { at: ack, answered, timed_out };
        }
        match self.state {
            State::Acked { at, answered, timed_out } => Some(Ack {
                timed_out,
                responder,
                at,
                loadmd_at: at,
                answered_at: answered,
                cached: self.memory,
            }),
            _ => None,
        }
    }

    pub fn granted(&self) -> bool {
        matches!(self.state, State::Granted { .. } | State::Acked { .. })
    }

    pub fn busy(&self) -> bool {
        self.state != State::Idle
    }

    pub fn ack_at(&self) -> Option<u64> {
        match self.state {
            State::Granted { ack, .. } => Some(ack),
            State::Acked { at, .. } => Some(at),
            _ => None,
        }
    }

    pub fn answered_at(&self) -> Option<u64> {
        match self.state {
            State::Granted { answered, .. } | State::Acked { answered, .. } => Some(answered),
            _ => None,
        }
    }

    /// The processor lets the request go.
    pub fn finish(&mut self) {
        self.state = State::Idle;
    }

    pub fn save(&self, w: &mut crate::checkpoint::Writer) {
        let MemoryPort {
            state,
            write,
            addr,
            memory,
            cache,
            timing,
            memory_free_at,
            buffer_free_at,
            write_back_until,
            model: _,
            // The prefetch's reach is the engine's setting; its counts are
            // the profile's.
            prefetch: _,
            prefetched,
            fetch_vaddr,
            prefetch_counts: _,
            // The revision is the machine's.
            paged: _,
        } = self;
        match *state {
            State::Idle => w.u8(0),
            State::Requested => w.u8(1),
            State::Granted { ack, answered, timed_out } => {
                w.u8(2);
                w.u64(ack);
                w.u64(answered);
                w.bool(timed_out);
            }
            State::Acked { at, answered, timed_out } => {
                w.u8(3);
                w.u64(at);
                w.u64(answered);
                w.bool(timed_out);
            }
        }
        w.bool(*write);
        w.u32(*addr);
        w.bool(*memory);
        cache.save(w);
        w.u64(timing.read_ns);
        w.u64(timing.write_ns);
        w.u64(*memory_free_at);
        w.u64(*buffer_free_at);
        w.opt(*prefetched, |w, p| {
            w.u32(p.vaddr);
            w.u32(p.phys);
            w.word(p.word);
        });
        w.opt(*fetch_vaddr, |w, v| w.u32(v));
        if self.paged {
            w.u64(*write_back_until);
        }
    }

    pub fn load(&mut self, r: &mut crate::checkpoint::Reader) -> std::io::Result<()> {
        self.state = match r.u8()? {
            0 => State::Idle,
            1 => State::Requested,
            k @ (2 | 3) => {
                let (at, answered, timed_out) = (r.u64()?, r.u64()?, r.bool()?);
                if k == 2 {
                    State::Granted { ack: at, answered, timed_out }
                } else {
                    State::Acked { at, answered, timed_out }
                }
            }
            k => return Err(crate::checkpoint::bad(format!("memory port state {k}"))),
        };
        self.write = r.bool()?;
        self.addr = r.u32()?;
        self.memory = r.bool()?;
        self.cache = Cache::load(r)?;
        self.timing = MemoryTiming { read_ns: r.u64()?, write_ns: r.u64()? };
        self.memory_free_at = r.u64()?;
        self.buffer_free_at = r.u64()?;
        // The word the prefetch held, and a fetch it is to look past; kept
        // only where the engine has the prefetch fitted.
        let mask = self.vaddr_mask();
        let prefetched =
            r.opt(|r| Ok(Prefetched { vaddr: r.u32()? & mask, phys: r.u32()?, word: r.word()? }))?;
        let fetch_vaddr = r.opt(|r| Ok(r.u32()? & mask))?;
        if self.prefetch.is_some() {
            self.prefetched = prefetched;
            self.fetch_vaddr = fetch_vaddr;
        }
        if self.paged {
            self.write_back_until = r.u64()?;
        }
        Ok(())
    }
}
