// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The TIME and UPTIME services, AIM-628 §5.8 and the Lisp Machine
//! manual's Chaosnet chapter.
//!
//! "An RFC to contact name TIME evokes an ANS containing the number of
//! seconds since midnight Greenwich Mean Time, Jan 1, 1900 as a 32-bit
//! number in four 8-bit bytes, least-significant byte first. Some
//! computers --- Lisp machines, for example --- which don't have hardware
//! calendar-clocks use this protocol to find out the date and time when
//! they first come up." The Lisp Machine's `HOST-TIME` in
//! `sys/network/chaos/chuse.lisp` asks each of its time-server hosts in
//! turn and `DECODE-CANONICAL-TIME-PACKET` reads the two words back, low
//! word first.
//!
//! UPTIME "is similar to the TIME protocol, except that the contact name
//! is UPTIME, and the time returned is actually an interval (in seconds)
//! describing how long the host has been up."

use super::server::{Response, Service};

/// Seconds from 1 January 1900 to 1 January 1970: the universal time of
/// the Unix epoch.
pub const UNIX_EPOCH_UNIVERSAL: u64 = 2_208_988_800;

/// The universal time the tests fix the Chaosnet server's clock at, so that two
/// runs of a boot against the Chaosnet server do the same work and can be
/// compared: 1 September 2026, 00:00:00 UT.  `muir` itself answers with
/// the machine's clock.
pub const TEST_UNIVERSAL: u32 = 3_997_209_600;

/// Where the time comes from: the machine's clock, or a fixed value for
/// a test.
pub enum Clock {
    System,
    Fixed(u32),
}

pub struct Time {
    clock: Clock,
}

impl Time {
    pub fn new() -> Time {
        Time { clock: Clock::System }
    }

    /// A server whose answer is always this universal time.
    pub fn fixed(universal: u32) -> Time {
        Time { clock: Clock::Fixed(universal) }
    }

    /// The universal time now: seconds since 1900, as the Lisp Machine
    /// counts it.
    pub fn universal(&self) -> u32 {
        match self.clock {
            Clock::Fixed(t) => t,
            Clock::System => {
                let unix = if neutral() {
                    // The time-neutral harness's, fixed, for runs that repeat.
                    FIXED_UNIX
                } else {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0)
                };
                // Four bytes on the wire: the count wraps on 7 February
                // 2036, as the band's own universal time does.
                (unix + UNIX_EPOCH_UNIVERSAL) as u32
            }
        }
    }
}

impl Default for Time {
    fn default() -> Self {
        Time::new()
    }
}

impl Service for Time {
    fn contact(&self) -> &str {
        "TIME"
    }
    fn request(&mut self, _now: u64, _args: &str, _from: (u16, u16)) -> Response {
        Response::Answer(self.universal().to_le_bytes().to_vec())
    }
}

/// UPTIME: seconds since the host came up, by the ether's clock.
pub struct Uptime {
    since: u64,
}

impl Uptime {
    pub fn new(since: u64) -> Uptime {
        Uptime { since }
    }
}

impl Service for Uptime {
    fn contact(&self) -> &str {
        "UPTIME"
    }
    fn request(&mut self, now: u64, _args: &str, _from: (u16, u16)) -> Response {
        let secs = (now.saturating_sub(self.since) / 1_000_000_000) as u32;
        Response::Answer(secs.to_le_bytes().to_vec())
    }
}

/// The host's date under the time-neutral harness, 1 September 2026
/// (`tests/support/neutral.rs`).
pub const FIXED_UNIX: u64 = 1_788_220_800;

/// Whether the time-neutral harness is on: `MUIR_TIME_NEUTRAL=1`.
pub fn neutral() -> bool {
    std::env::var("MUIR_TIME_NEUTRAL").is_ok_and(|v| v == "1")
}
