// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's real-time clock (contract Q9, revision 9): word 103 of the
//! register page, `17777503`, the time in whole seconds since 1970-01-01
//! UTC, unsigned 32 bits, read only. Live by default, the host's clock at
//! each read; `--rtc <s>` ([`Rtc::Counted`]) starts it at second `s` and
//! counts the machine's own time from there, holding at 2^32-1. The CADR
//! has no such word.

use std::time::{SystemTime, UNIX_EPOCH};

use muir::machine::{Geometry, Machine, Rtc, bus_error};

mod support;

const PAGE: u32 = muir::machine::REGISTER_PAGE_13;
const RTC: u32 = PAGE + 0o103;
const SECOND: u64 = 1_000_000_000;

fn quux() -> Machine {
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
    m
}

fn host_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

/// **`--rtc <s>` reads `s` at power-on and counts the machine's time**: a
/// second more for each 10^9 ns of the machine's clock, whatever the host's
/// clock does, and a partial second not yet.
#[test]
fn a_fixed_start_counts_machine_time() {
    let s = 1_700_000_000;
    let mut m = quux();
    m.rtc = Rtc::Counted { start: s, base_ns: m.ns };
    assert_eq!(m.bus_read(RTC), s.into(), "at power-on");
    m.ns = SECOND - 1;
    assert_eq!(m.bus_read(RTC), s.into(), "a nanosecond short of a second");
    m.ns = SECOND;
    assert_eq!(m.bus_read(RTC), (s + 1).into(), "a second on");
    m.ns = 3600 * SECOND + 5;
    assert_eq!(m.bus_read(RTC), (s + 3600).into(), "an hour on");
    // The base is the machine's clock when the RTC was set.
    m.rtc = Rtc::Counted { start: s, base_ns: 3600 * SECOND };
    assert_eq!(m.bus_read(RTC), s.into(), "from its own base");
    assert_eq!(m.bus_error & bus_error::XBUS_NXM, 0, "nothing timed out");
}

/// **It holds at 2^32-1 and never wraps**: 0 would read as no RTC at all,
/// an empty word also reading 0.
#[test]
fn it_holds_at_the_last_second() {
    let mut m = quux();
    m.rtc = Rtc::Counted { start: u32::MAX - 1, base_ns: 0 };
    assert_eq!(m.bus_read(RTC), (u32::MAX - 1).into());
    m.ns = SECOND;
    assert_eq!(m.bus_read(RTC), u32::MAX.into(), "the last second");
    m.ns = 1000 * SECOND;
    assert_eq!(m.bus_read(RTC), u32::MAX.into(), "held, not wrapped");
    m.rtc = Rtc::Counted { start: u32::MAX, base_ns: 0 };
    m.ns = u64::MAX;
    assert_eq!(m.bus_read(RTC), u32::MAX.into(), "at the machine clock's own end");
}

/// **Live, the default, is the host's clock at each read**: within a second
/// or two of `SystemTime::now()`, and not moved by the machine's time.
#[test]
fn live_is_the_host_s_clock() {
    let mut m = quux();
    assert_eq!(m.rtc, Rtc::Host, "the default");
    let before = host_now();
    let got = m.bus_read(RTC);
    let after = host_now();
    assert!(before <= got && got <= after, "{before} <= {got} <= {after}");
    m.ns = 100_000 * SECOND;
    let got = m.bus_read(RTC);
    assert!(got.abs_diff(host_now()) <= 2, "{got}: the machine's time does not move it");
}

/// **A write goes nowhere**, as a write of any read-only word on the page
/// does: the word reads the same, nothing else changes, nothing times out.
#[test]
fn a_write_changes_nothing() {
    for rtc in [Rtc::Counted { start: 1_000_000, base_ns: 0 }, Rtc::Host] {
        let mut m = quux();
        m.rtc = rtc;
        m.ns = 7 * SECOND;
        let before = m.bus_read(RTC);
        for v in [0u32, 1, !0, 0o1234567] {
            m.bus_write(RTC, v.into());
            let after = m.bus_read(RTC);
            if let Rtc::Counted { .. } = rtc {
                assert_eq!(after, before, "{rtc:?}: written {v:o}");
            } else {
                assert!(after.abs_diff(host_now()) <= 2, "{rtc:?}: written {v:o}");
            }
        }
        assert_eq!(m.rtc, rtc, "the setting is not the machine's to change");
        assert!(!m.mode.errstop);
        assert_eq!(m.bus_error & bus_error::XBUS_NXM, 0);
    }
}

/// **The CADR has no word 103**: nothing answers on the page there, and the
/// read times out as any read of an empty Unibus address does.
#[test]
fn the_cadr_has_no_rtc() {
    let mut m = Machine::new();
    m.rtc = Rtc::Counted { start: 1_700_000_000, base_ns: 0 };
    assert_eq!(m.bus_read(RTC), 0);
    assert_eq!(m.bus_error, bus_error::UNIBUS_NXM, "the read timed out");
}

/// **Feature word 15 says the RTC is there**, `<0>`, beside the file
/// device, `<1>` (`tests/quux_file_device.rs`); the CADR has no page.
#[test]
fn feature_word_15_announces_it() {
    assert_eq!(Geometry::QUUX.feature_word(PAGE + 0o15), Some(3));
    assert_eq!(Geometry::CADR.feature_word(PAGE + 0o15), None);
    let mut m = quux();
    assert_eq!(m.bus_read(PAGE + 0o15), 3);
    let id = Geometry::QUUX.machine_id.unwrap();
    assert!((id >> 4) & 0o7777 >= 9, "revision 9 or later");
}

/// **A checkpoint keeps the count**: a machine resumed from one reads the
/// second the saved one did, and counts on from it; a live one stays live.
#[test]
fn a_checkpoint_keeps_the_count() {
    use muir::checkpoint::{Reader, Writer};
    for (rtc, ns) in [
        (Rtc::Counted { start: 1_700_000_000, base_ns: 0 }, 5 * SECOND + 3),
        (Rtc::Counted { start: 1_700_000_000, base_ns: 2 * SECOND }, 5 * SECOND + 3),
        (Rtc::Host, SECOND),
    ] {
        let mut m = quux();
        m.rtc = rtc;
        m.ns = ns;
        let mut w = Writer::new();
        m.save(&mut w);
        let body = w.finish();
        let mut back = Machine::new();
        back.geometry = Geometry::QUUX;
        back.load(&mut Reader::for_word_bits(&body, 40)).unwrap();
        assert_eq!(back.rtc, rtc);
        assert_eq!(back.geometry, Geometry::QUUX);
        if let Rtc::Counted { .. } = rtc {
            assert_eq!(back.bus_read(RTC), m.bus_read(RTC), "{rtc:?}: the same second");
            back.ns += SECOND;
            m.ns += SECOND;
            assert_eq!(back.bus_read(RTC), m.bus_read(RTC), "{rtc:?}: and on");
        }
    }
}

/// **Both engines read it through the bus, and count the machine's own
/// time**: a program reads word 103, runs about 4 us of machine time on,
/// and reads it again. The machine's clock starts 2 us short of its first
/// second, so the first read gives the start and the second one more; the
/// test measures that the engine's clock did cross the second between.
#[test]
fn both_engines_count_machine_seconds() {
    use muir::engine::Engine;
    use muir::isa::Insn;
    use muir::isa::asm::{ALU, SETM, SRC_MD, START_READ, a_dest, filler, m_src};
    use muir::micro::Micro;
    use muir::rtl::Rtl;
    let s = 1_700_000_000;
    let mut prom = vec![Insn::new(ALU | SETM | m_src(1) | START_READ)];
    prom.extend([filler(); 12]);
    prom.push(Insn::new(ALU | SETM | SRC_MD | a_dest(0o200)));
    prom.extend([filler(); 100]);
    prom.push(Insn::new(ALU | SETM | m_src(1) | START_READ));
    prom.extend([filler(); 12]);
    prom.push(Insn::new(ALU | SETM | SRC_MD | a_dest(0o201)));
    let power_on = SECOND - 2_000;
    let machine = || {
        let mut m = quux();
        let mut words = vec![filler(); 1024];
        words[..prom.len()].copy_from_slice(&prom);
        m.load_prom(&words);
        m.mmem[1] = support::quux_map(&mut m, 1, PAGE | 0o103).into();
        m.rtc = Rtc::Counted { start: s, base_ns: 0 };
        m
    };
    let mut m = machine();
    m.ns = power_on;
    let mut e = Micro::new(m);
    e.boot();
    let mut r = Rtl::new(machine());
    r.set_clock(power_on);
    r.boot();
    let steps = prom.len() + 20;
    for _ in 0..steps {
        e.step().unwrap();
        r.step().unwrap();
    }
    for (name, m) in [("micro", e.machine()), ("rtl", r.machine())] {
        assert_eq!(m.bus_error & bus_error::XBUS_NXM, 0, "{name}: no timeout");
        assert_eq!(m.amem[0o200], s.into(), "{name}: the start, before the second");
        assert!(m.ns >= SECOND, "{name}: the machine's clock crossed its second: {} ns", m.ns);
        assert!(m.ns < 2 * SECOND, "{name}: and no more: {} ns", m.ns);
        assert_eq!(m.amem[0o201], (s + 1).into(), "{name}: a second on, at {} ns", m.ns);
    }
}
