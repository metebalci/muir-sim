// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A memory cache, H2: a unified, write-through cache of main memory in
//! front of the CADR's bus, as `rtl` times it, a measurement's. QUUX's own
//! cache is its pipeline's port's (`crate::pipeline::port`).
//!
//! It holds tags and nothing else. `rtl` takes a read's word from main
//! memory when the cycle ends, and a write-through cache never holds a word
//! main memory does not, so what the machine reads is right by
//! construction; the cache decides only how soon a read is acknowledged.
//! The fabric's cache holds the data too, and its contract is this one's
//! timing plus that coherence.
//!
//! - **Physical addresses**, after the map, and **main memory only**: Xbus
//!   I/O space and the Unibus are not cached, their reads having side
//!   effects or being a device's.
//! - **Reads**: a hit is acknowledged [`CacheConfig::hit_ns`] after the
//!   request, with no bus cycle; a miss is the memory board's cycle, and
//!   fills the line the word is in.
//! - **Writes** go through to the board and allocate nothing; a line that
//!   holds the word keeps it. With the write buffer, a write is
//!   acknowledged after the hit time and the board runs it behind the
//!   processor; a write finding the buffer still full waits for the board
//!   to empty it. The word is main memory's from the acknowledgement, as a
//!   read of it after, hit or miss, would find it through the buffer.
//! - **Coherence**: anything else that writes main memory --- the disk's
//!   transfers --- invalidates the cache, all of it ([`Cache::invalidate`]).
//!
//! Replacement is least recently used within a set.

/// QUUX's main memory as the FPGA has it, in place of the CADR's memory
/// boards: a read, which is a line fill behind the cache, and a write each
/// answered a fixed time after the processor's request, off the Xbus and
/// so with none of its setup and deskew, one at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryTiming {
    pub read_ns: u64,
    pub write_ns: u64,
}

impl MemoryTiming {
    /// The Arty Z7-20's, from muir-fpga's measurement on the board
    /// (`S_AXI_HP0`, System 1001 running a 1,000,000-word array loop for
    /// 300 s, 138 M reads and 6.1 M writes): a single read, AR to the last
    /// R, 20.68 ticks of 10 ns on average (19 at least, 88 at most, 95.8 per
    /// cent within 19 to 25); a write, AW to B, 12. Rounded up to the tick:
    /// 210 and 120 ns. A read here is a line fill of four words, two 64-bit
    /// beats where the measured reads were one: **unverified**, taken as one
    /// tick more, 220 ns, until a fill is measured.
    pub const ARTY_Z7_20: MemoryTiming = MemoryTiming { read_ns: 220, write_ns: 120 };

    /// The DE25-Nano's, measured the same way (`F2SDRAM`, 130 M reads, 5.9 M
    /// writes, 471 passes), at the machine's ask, the bridge and a tick for
    /// its share: a single read 36.25 ticks on average (33 at least, 228 at
    /// most), a write 29 (28 to 58); rounded up, 370 and 290 ns. The line
    /// fill is taken as one tick more, 380 ns, **unverified** likewise.
    pub const DE25_NANO: MemoryTiming = MemoryTiming { read_ns: 380, write_ns: 290 };
}

/// A cache's shape and speed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheConfig {
    /// Words it holds, a power of two.
    pub words: u32,
    /// Words a line, a power of two: what a miss fills.
    pub line_words: u32,
    /// Lines a set, a power of two dividing the lines.
    pub ways: u32,
    /// From the processor's request to the acknowledgement of a hit.
    pub hit_ns: u64,
    /// A write buffer of one word: a write is acknowledged without waiting
    /// for the board unless the last one is still in it.
    pub write_buffer: bool,
}

impl CacheConfig {
    /// `words` in lines of 4, 2-way, a hit in 20 ns --- two ticks of the
    /// fabric's grid, one for the RAM and one for the tag's compare --- and
    /// a write buffer. The 20 ns is met on the Arty Z7-20 in muir-fpga's
    /// routed fit of QUUX with contract Q6, a worst setup slack of +0.136 ns
    /// in 12,236 LUTs and 71 block RAMs: the hit's paths, the cache RAM to
    /// `MD` and the lookup to the port, each meet one 10 ns tick, with
    /// +1.495 and +0.926 ns to spare. The DE25-Nano's fit is to follow.
    pub const fn with_words(words: u32) -> CacheConfig {
        CacheConfig { words, line_words: 4, ways: 2, hit_ns: 20, write_buffer: true }
    }

    /// Whether the shape is one: powers of two, a line no bigger than the
    /// cache, and the ways dividing the lines.
    pub fn check(self) -> Result<(), String> {
        let p2 = |n: u32| n.is_power_of_two();
        if !p2(self.words) || !p2(self.line_words) || !p2(self.ways) {
            return Err(format!("{self:?}: words, line and ways must be powers of two"));
        }
        if self.line_words * self.ways > self.words {
            return Err(format!("{self:?}: fewer words than one set's lines"));
        }
        Ok(())
    }

    fn sets(self) -> u32 {
        self.words / self.line_words / self.ways
    }
}

/// The cache's tags and counts.
#[derive(Clone, Debug)]
pub struct Cache {
    pub config: CacheConfig,
    /// A set's ways, most recently used first: the line numbers they hold.
    sets: Vec<Vec<u32>>,
    pub hits: u64,
    pub misses: u64,
}

impl Cache {
    pub fn new(config: CacheConfig) -> Cache {
        Cache { config, sets: vec![Vec::new(); config.sets() as usize], hits: 0, misses: 0 }
    }

    fn set_of(&self, line: u32) -> usize {
        (line % self.config.sets()) as usize
    }

    /// A read of physical word `phys`: whether it hit. A miss fills its
    /// line, evicting the set's least recently used.
    pub fn read(&mut self, phys: u32) -> bool {
        let line = phys / self.config.line_words;
        let ways = self.config.ways as usize;
        let set = self.set_of(line);
        let s = &mut self.sets[set];
        if let Some(k) = s.iter().position(|&l| l == line) {
            s.remove(k);
            s.insert(0, line);
            self.hits += 1;
            true
        } else {
            s.insert(0, line);
            s.truncate(ways);
            self.misses += 1;
            false
        }
    }

    /// Whether `phys` is held, without touching the order.
    pub fn holds(&self, phys: u32) -> bool {
        let line = phys / self.config.line_words;
        self.sets[self.set_of(line)].contains(&line)
    }

    /// Everything dropped: something other than the processor has written
    /// main memory.
    pub fn invalidate(&mut self) {
        for s in &mut self.sets {
            s.clear();
        }
    }

    pub fn save(&self, w: &mut crate::checkpoint::Writer) {
        let c = self.config;
        for v in [c.words, c.line_words, c.ways] {
            w.u32(v);
        }
        w.u64(c.hit_ns);
        w.bool(c.write_buffer);
        w.u64(self.hits);
        w.u64(self.misses);
        for s in &self.sets {
            w.u32(s.len() as u32);
            for &l in s {
                w.u32(l);
            }
        }
    }

    pub fn load(r: &mut crate::checkpoint::Reader) -> std::io::Result<Cache> {
        let config = CacheConfig {
            words: r.u32()?,
            line_words: r.u32()?,
            ways: r.u32()?,
            hit_ns: r.u64()?,
            write_buffer: r.bool()?,
        };
        config.check().map_err(crate::checkpoint::bad)?;
        let mut c = Cache::new(config);
        c.hits = r.u64()?;
        c.misses = r.u64()?;
        for s in &mut c.sets {
            let n = r.u32()?;
            if n > config.ways {
                return Err(crate::checkpoint::bad(format!("a set of {n} lines in {config:?}")));
            }
            for _ in 0..n {
                s.push(r.u32()?);
            }
        }
        Ok(c)
    }
}
