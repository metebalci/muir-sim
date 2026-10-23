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

mod support;

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
