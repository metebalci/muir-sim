// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! H2: QUUX's memory cache, as `rtl` times it ([`muir::cache`]).
//!
//! A unified, write-through cache of main memory by physical address. A
//! read hit is acknowledged after the cache's hit time with no bus cycle; a
//! miss is the board's cycle and fills its line; writes go to the board and
//! allocate nothing; a disk transfer invalidates it all. It holds tags only:
//! the machine's words come from main memory as always, so a program leaves
//! the same state with the cache as without, sooner.

use muir::cache::{Cache, CacheConfig};
use muir::clock::TimingModel;
use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{
    ADD, ALU, ALWAYS, AND, CARRY_IN, JUMP, M_PLUS_C, MD, SETM, SRC_MD, START_READ, START_WRITE,
    a_dest, a_src, filler, m_dest, m_src, target,
};
use muir::machine::{Geometry, Machine};
use muir::rtl::Rtl;

mod support;

/// The least cache QUUX's port fits, which [`reader`] gets nothing from:
/// two lines of 8 words in one set, and no write buffer. QUUX has no run
/// without its cache (contract Q6), and its line is 8 words (contract G2
/// §3), so this is the measure against.
const NONE: CacheConfig =
    CacheConfig { words: 16, line_words: 8, ways: 2, hit_ns: 20, write_buffer: false };

/// **Lines, sets and replacement**: a line of 4 fills on its first word's
/// miss and hits on the other three; a 2-way set keeps the two most
/// recently used lines; invalidation drops everything.
#[test]
fn a_line_fills_and_the_least_recently_used_goes() {
    let mut c = Cache::new(CacheConfig {
        words: 64,
        line_words: 4,
        ways: 2,
        hit_ns: 20,
        write_buffer: false,
    });
    // Eight sets of two lines of four: words 0, 32 and 64 are three lines
    // of set 0.
    assert!(!c.read(0));
    assert!(c.read(1) && c.read(2) && c.read(3), "the rest of the line");
    assert!(!c.read(32), "a second line in the set");
    assert!(c.read(0), "the first still there");
    assert!(!c.read(64), "a third evicts the least recently used, 32");
    assert!(c.holds(0) && c.holds(64) && !c.holds(32));
    assert_eq!((c.hits, c.misses), (4, 3));
    c.invalidate();
    assert!(!c.holds(0) && !c.holds(64));
    assert!(CacheConfig { words: 48, ..c.config }.check().is_err(), "not a power of two");
}

/// A loop that reads main memory a line of 8 apart from 1000, round the
/// eight lines 1000-1077, adding each word into A 3 and stepping the
/// address in M 1: [`NONE`]'s two lines miss every read, and a cache that
/// holds the eight hits every one after the first round.
fn reader() -> Machine {
    let prom = [
        Insn::new(ALU | SETM | m_src(1) | START_READ),
        filler(),
        filler(),
        Insn::new(ALU | ADD | SRC_MD | a_src(3) | a_dest(3)),
        Insn::new(ALU | ADD | m_src(1) | a_src(5) | m_dest(1)),
        Insn::new(ALU | AND | m_src(1) | a_src(6) | m_dest(1)),
        Insn::new(JUMP | target(0) | ALWAYS),
    ];
    let mut m = Machine::new();
    let mut words = vec![filler(); 512];
    words[..prom.len()].copy_from_slice(&prom);
    m.load_prom(&words);
    m.geometry = Geometry::QUUX;
    support::prom_program_in_ram(&mut m);
    // Level-2 entries 0 to 7: virtual pages onto physical pages 0 to 7,
    // readable and writable.
    for p in 0..8u32 {
        m.l2_map[p as usize] = (1 << 27) | (1 << 26) | p;
    }
    for (k, w) in m.main[..2048].iter_mut().enumerate() {
        *w = u64::from((k as u32).wrapping_mul(2_654_435_761));
    }
    m.mmem[1] = 0o1000;
    m.amem[5] = 8;
    m.amem[6] = 0o1077;
    m
}

/// `reader` on `rtl` for `steps` microcycles, with `cache` fitted, or
/// [`NONE`].
fn run(cache: Option<CacheConfig>, model: TimingModel, steps: usize) -> Rtl {
    let cache = cache.or(Some(NONE));
    let mut e = Rtl::new(reader());
    e.set_timing_model(model);
    e.set_cache(cache);
    e.boot();
    for _ in 0..steps {
        e.step().unwrap();
    }
    e
}

/// **The cache changes when, not what**: after the same microcycles the
/// machine holds the same words with it as with [`NONE`], under `sync` of
/// four ticks and of three, and it got there sooner, every read after the
/// first round of eight hitting.
#[test]
fn the_cache_changes_when_and_not_what() {
    let steps = 3_000;
    for model in [
        TimingModel::Sync { cycle_ticks: 4, ilong_ticks: 0 },
        TimingModel::Sync { cycle_ticks: 3, ilong_ticks: 0 },
    ] {
        let without = run(None, model, steps);
        let with = run(Some(CacheConfig::with_words(1024)), model, steps);
        let state = |e: &Rtl| (e.machine().amem[3], e.machine().mmem[1], e.pc());
        assert_eq!(state(&with), state(&without), "{model:?}: the same words");
        assert!(with.ns() < without.ns(), "{model:?}: {} ns against {}", with.ns(), without.ns());
        let c = with.cache().unwrap();
        let reads = c.hits + c.misses;
        assert!(reads > 50, "{model:?}: {reads} reads");
        assert_eq!(c.hits, reads - 8, "{model:?}: {} hits in {reads}", c.hits);
        eprintln!("{model:?}: {} ns with the cache, {} without", with.ns(), without.ns());
    }
}

/// **A disk transfer invalidates the cache**: the flag the disk controller's
/// write raises makes the next cycle's lookup miss where it would have hit.
#[test]
fn a_disk_transfer_invalidates_the_cache() {
    let mut e = run(
        Some(CacheConfig::with_words(1024)),
        TimingModel::Sync { cycle_ticks: 4, ilong_ticks: 0 },
        200,
    );
    let before = e.cache().unwrap().clone();
    assert!(before.holds(0o1000), "the loop's first line is held");
    e.machine_mut().dma_written = true;
    // Back to the first word: point M 1 at it and let the loop read it.
    e.machine_mut().mmem[1] = 0o1000;
    for _ in 0..12 {
        e.step().unwrap();
    }
    let after = e.cache().unwrap();
    assert!(!e.machine().dma_written, "the engine took the flag");
    assert!(after.misses > before.misses, "the held line missed");
}

/// **A checkpoint keeps the cache**: its shape, its lines and its counts,
/// and a run resumed from it goes on as the one saved.
#[test]
fn a_checkpoint_keeps_the_cache() {
    use muir::checkpoint::{Reader, Writer};
    let mut e = run(
        Some(CacheConfig::with_words(256)),
        TimingModel::Sync { cycle_ticks: 4, ilong_ticks: 0 },
        500,
    );
    let mut w = Writer::new();
    e.save(&mut w);
    let body = w.finish();
    let mut back = Rtl::new(reader());
    back.load(&mut Reader::for_word_bits(&body, 40)).unwrap();
    for _ in 0..500 {
        e.step().unwrap();
        back.step().unwrap();
    }
    let (a, b) = (e.cache().unwrap(), back.cache().unwrap());
    assert_eq!((a.config, a.hits, a.misses), (b.config, b.hits, b.misses));
    assert_eq!(e.ns(), back.ns());
}

/// A loop that writes a counting word to main memory at M 1, reads it back
/// at once into A 3, and steps the address every other time round, so that
/// each word is written twice and read back after each.
fn writer() -> Machine {
    let mut m = reader();
    let prom = [
        Insn::new(ALU | M_PLUS_C | CARRY_IN | m_src(2) | m_dest(2)),
        Insn::new(ALU | SETM | m_src(2) | MD),
        Insn::new(ALU | SETM | m_src(1) | START_WRITE),
        filler(),
        Insn::new(ALU | SETM | m_src(1) | START_READ),
        filler(),
        filler(),
        Insn::new(ALU | ADD | SRC_MD | a_src(3) | a_dest(3)),
        Insn::new(ALU | SETM | SRC_MD | a_dest(4)),
        Insn::new(ALU | M_PLUS_C | CARRY_IN | m_src(1) | m_dest(1)),
        Insn::new(JUMP | target(0) | ALWAYS),
    ];
    let mut words = vec![filler(); 512];
    words[..prom.len()].copy_from_slice(&prom);
    m.load_prom(&words);
    support::prom_program_in_ram(&mut m);
    m
}

/// **A word written through the buffer is read back as written**, by a
/// read straight after it, hit or miss: after the same microcycles the
/// machine holds the same words with the cache and its write buffer as
/// without, the last word read back being the last written, and sooner.
#[test]
fn a_written_word_is_read_back_through_the_buffer() {
    for model in [
        TimingModel::Sync { cycle_ticks: 4, ilong_ticks: 0 },
        TimingModel::Sync { cycle_ticks: 3, ilong_ticks: 0 },
    ] {
        let go = |cache: Option<CacheConfig>| {
            let mut e = Rtl::new(writer());
            e.set_timing_model(model);
            e.set_cache(cache.or(Some(NONE)));
            e.boot();
            for _ in 0..4_000 {
                e.step().unwrap();
            }
            e
        };
        let (without, with) = (go(None), go(Some(CacheConfig::with_words(1024))));
        let state = |e: &Rtl| {
            let m = e.machine();
            (m.amem[3], m.amem[4], m.mmem[1], m.mmem[2], m.main[0o1000..0o1100].to_vec())
        };
        assert_eq!(state(&with), state(&without), "{model:?}: the same words");
        assert!(with.ns() < without.ns(), "{model:?}: {} against {}", with.ns(), without.ns());
        eprintln!("{model:?}: {} ns with the buffer, {} without", with.ns(), without.ns());
    }
}

/// **QUUX's memory timing changes when, not what**: with main memory at
/// other figures, the read loop and the write-and-read-back loop leave the
/// same words, with its cache and with [`NONE`], and a slower read makes
/// the same run longer.
#[test]
fn quux_memory_timing_changes_when_and_not_what() {
    use muir::cache::MemoryTiming;
    let sync = TimingModel::Sync { cycle_ticks: 4, ilong_ticks: 0 };
    let go = |make: fn() -> Machine, memory: MemoryTiming, cache: CacheConfig| {
        let mut e = Rtl::new(make());
        e.set_timing_model(sync);
        e.set_memory_timing(Some(memory));
        e.set_cache(Some(cache));
        e.boot();
        for _ in 0..4_000 {
            e.step().unwrap();
        }
        e
    };
    let state = |e: &Rtl| {
        let m = e.machine();
        (m.amem[3], m.amem[4], m.mmem[1], m.mmem[2], m.main[0o1000..0o1100].to_vec())
    };
    let fast = MemoryTiming { read_ns: 200, write_ns: 150 };
    let slow = MemoryTiming { read_ns: 300, write_ns: 150 };
    for make in [reader as fn() -> Machine, writer] {
        let nominal = go(make, MemoryTiming::NOMINAL, NONE);
        for cache in [NONE, CacheConfig::with_words(1024)] {
            let quux = go(make, fast, cache);
            assert_eq!(state(&quux), state(&nominal), "the same words, cache {cache:?}");
        }
        let (f, s) = (go(make, fast, NONE), go(make, slow, NONE));
        assert!(s.ns() > f.ns(), "a slower read is a longer run: {} against {}", s.ns(), f.ns());
    }
}
