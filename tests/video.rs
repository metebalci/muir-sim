// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The video controller, QUUX's display: a monochrome 1280 by 1024 frame
//! buffer by default, one bit a pixel, 40 words a line, 40,960 words from
//! `1760000000`, the frame buffer window (contract G2 §4.1), which reads a
//! word as a fixnum, tag `005`, and drops the tag of a word written. Its
//! mode is word 210 of the register page (`1777777610`, contract Q13),
//! which keeps black-on-white, bit 2, and nothing else; words 211-217 are
//! reserved; and it raises no interrupt, the clock being the processor's
//! tick. The CADR's control registers at `17377760` are nothing there on
//! QUUX of 2MW, past its main memory. QUUX only.

use muir::engine::Engine;
use muir::isa::Insn;
use muir::isa::asm::{ALU, MD, SETM, SRC_MD, START_READ, START_WRITE, a_dest, filler, m_src};
use muir::machine::{Geometry, Machine, UNBOXED_TAG, WINDOW_13, bus_error};
use muir::micro::Micro;
use muir::rtl::Rtl;
use muir::terminal::Frame;
use muir::tv::{self, Board};

mod support;

/// The register page, and the video controller's mode on it.
const PAGE: u32 = muir::machine::REGISTER_PAGE_13;
const MODE: u32 = PAGE + 0o210;

fn quux_with_video() -> Machine {
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
    m.tv.set_board(Board::Video);
    m
}

/// **The buffer is 40,960 words from `1760000000`**: its first and last
/// words keep what is written, read back as fixnums, and the word after
/// the last is nobody's, a read of it failing with the Xbus NXM bit, on
/// both engines.
#[test]
fn the_buffer_is_40960_words() {
    // (physical address, value): written through MD, read back into A.
    let last = WINDOW_13 + tv::VIDEO_WORDS - 1;
    // Page 1 onto the buffer's first word's frame, 2 onto its last word's,
    // 3 onto the word after it's.
    let cases = [(WINDOW_13, 0o1234567u32), (last, 0o7654321), (last + 1, 0)];
    let mut prom = Vec::new();
    for (k, &(va, _)) in cases.iter().enumerate() {
        let k = k as u64;
        prom.push(Insn::new(ALU | SETM | m_src(10 + k) | MD));
        prom.push(Insn::new(ALU | SETM | m_src(k + 1) | START_WRITE));
        prom.extend([filler(); 12]);
        prom.push(Insn::new(ALU | SETM | m_src(k + 1) | START_READ));
        prom.extend([filler(); 12]);
        prom.push(Insn::new(ALU | SETM | SRC_MD | a_dest(0o200 + k)));
        let _ = va;
    }
    let make = |prom: &[Insn]| {
        let mut m = quux_with_video();
        let mut words = vec![filler(); 512];
        words[..prom.len()].copy_from_slice(prom);
        m.load_prom(&words);
        support::prom_program_in_ram(&mut m);
        for (k, &(phys, v)) in cases.iter().enumerate() {
            m.mmem[1 + k] = support::quux_map(&mut m, 1 + k as u32, phys).into();
            m.mmem[10 + k] = u64::from(v);
        }
        m
    };
    // Each case is 28 instructions: the first two alone, then all three.
    for (cases_run, nxm) in [(2, false), (3, true)] {
        let mut e = Micro::new(make(&prom[..28 * cases_run]));
        e.boot();
        let mut r = Rtl::new(make(&prom[..28 * cases_run]));
        r.boot();
        for _ in 0..2000 {
            e.step().unwrap();
            r.step().unwrap();
        }
        for (name, m) in [("micro", e.machine()), ("rtl", r.machine())] {
            assert_eq!(m.amem[0o200], UNBOXED_TAG | 0o1234567, "{name}: the first word");
            assert_eq!(m.amem[0o201], UNBOXED_TAG | 0o7654321, "{name}: the last word");
            assert_eq!(m.tv.read_buffer(tv::VIDEO_WORDS - 1), 0o7654321, "{name}: in the buffer");
            let got = m.bus_error & bus_error::XBUS_NXM != 0;
            assert_eq!(got, nxm, "{name}: NXM after {cases_run} cases");
        }
    }
}

/// **Pixel `x` of line `y` is bit `x mod 32` of word `40 y + x / 32`**, the
/// low bit leftmost as on the CADR's TV, and the frame the terminal draws
/// is 1280 by 1024.
#[test]
fn a_pixel_is_where_the_cadr_would_put_it_at_40_words_a_line() {
    let mut m = quux_with_video();
    let (x, y) = (1279usize, 1023usize);
    m.bus_write(WINDOW_13 + (y * 40 + x / 32) as u32, 1 << (x % 32));
    assert!(m.tv.pixel(x, y));
    assert!(!m.tv.pixel(x - 1, y));
    let f = Frame::of(&m.tv);
    assert_eq!((f.width, f.height, f.words_per_line), (1280, 1024, 40));
    assert_eq!(f.visible(), tv::VIDEO_WORDS as usize);
    assert!(f.shows_white(x, y), "a one shows white while black-on-white is off");
    m.bus_write(MODE, tv::mode::BOW.into());
    assert!(!Frame::of(&m.tv).shows_white(x, y), "and black with it on");
}

/// **Word 210 keeps black-on-white alone, 211-217 are reserved, and nothing
/// interrupts**, the interrupt enable written or not, over a second of the
/// machine's time. The reserved words read 0, take writes to no effect and
/// answer; the CADR's eight control registers at `17377760` are nothing
/// there on a QUUX of 2MW, past its main memory, each access failing with
/// the Xbus NXM bit.
#[test]
fn it_keeps_black_on_white_and_never_interrupts() {
    let mut m = quux_with_video();
    m.bus_write(MODE, 0o377);
    assert_eq!(m.bus_read(MODE), tv::mode::BOW.into());
    for w in 1..8 {
        m.bus_write(MODE + w, 0o177777);
        assert_eq!(m.bus_read(MODE + w), 0, "word {:o}", 0o210 + w);
    }
    assert_eq!(m.bus_error, 0, "210-217 answer");
    assert_eq!(m.bus_read(MODE), tv::mode::BOW.into(), "the reserved words' writes went nowhere");
    for r in 0..8 {
        m.bus_error = 0;
        assert_eq!(m.bus_read(tv::CONTROL + r), 0, "register {r} read");
        assert_eq!(m.bus_error, bus_error::XBUS_NXM, "register {r} read");
        m.bus_error = 0;
        m.bus_write(tv::CONTROL + r, 0);
        assert_eq!(m.bus_error, bus_error::XBUS_NXM, "register {r} written");
    }
    assert_eq!(m.bus_read(MODE), tv::mode::BOW.into(), "the old address wrote nothing");
    m.bus_write(MODE, (tv::mode::INTERRUPT_ENABLE | tv::mode::BOW).into());
    for ms in 0..1000u64 {
        m.ns = ms * 1_000_000;
        assert!(!m.xbus_interrupt(), "an interrupt at {ms} ms");
        assert_eq!(m.interrupt_sources(), 0, "word 100 at {ms} ms");
    }
}

/// **The feature page describes the main screen** in three words: 11 is
/// the width in 31:16 and the height in 15:0, 12 the bits a pixel in 31:16
/// and the words a line in 15:0, and 13 the buffer's first physical address,
/// the window's, for the video controller or the CADR's TV when QUUX has
/// that board.
/// Word 14 is the clocks' (`tests/quux.rs`), 15 the optional devices
/// (`tests/quux_rtc.rs`), and 16 the number of interval timers, 3.
#[test]
fn the_feature_page_describes_the_main_screen() {
    let words =
        |m: &mut Machine| [0o11, 0o12, 0o13, 0o16].map(|w| support::low(m.bus_read(PAGE + w)));
    let packed = |hi: u32, lo: u32| hi << 16 | lo;
    assert_eq!(words(&mut quux_with_video()), [packed(1280, 1024), packed(1, 40), WINDOW_13, 3]);
    let mut m = Machine::new();
    m.geometry = Geometry::QUUX;
    assert_eq!(words(&mut m), [packed(768, 963), packed(1, 24), WINDOW_13, 3]);
}

/// **QUUX's decode takes the whole buffer as memory, and the mode as the
/// page's**: `rtl` takes a cycle to the buffer's last word as main
/// memory's, one past it as nothing there, word 210 as a register, and the
/// CADR's control registers as nothing there. What the word is and whether
/// NXM is set are `Machine::bus_read`'s, which the tests above hold; this is
/// the decode that makes the cycle's timing.
#[test]
fn the_decode_answers_the_whole_buffer() {
    use muir::busint::{Responder, decode_quux_13};
    let last = WINDOW_13 + tv::VIDEO_WORDS - 1;
    let m = quux_with_video();
    let words = m.tv.buffer_words();
    assert_eq!(words, tv::VIDEO_WORDS);
    assert_eq!(m.tv.control_registers(), 0, "none of the CADR's");
    let main = 1 << 20;
    assert_eq!(decode_quux_13(last, main, words), Responder::Memory(0));
    assert_eq!(decode_quux_13(last + 1, main, words), Responder::NoXbus);
    assert_eq!(decode_quux_13(last, main, tv::BUFFER_WORDS), Responder::NoXbus);
    assert_eq!(decode_quux_13(MODE, main, words), Responder::Device);
    for r in 0..8 {
        assert_eq!(decode_quux_13(tv::CONTROL + r, main, words), Responder::NoXbus, "register {r}");
    }
}

/// **The frame buffer may fill its window**: at most 4,193,280 words,
/// `1760000000`-`1777775777`. With a buffer that long QUUX's decode takes
/// `1777775777` as the buffer's, `1777776000` below the register page as
/// nothing and `1777777400` as the page's; the words bound refuses one word
/// more. A buffer of 261,888 words, set past the size QUUX supports, ends
/// where the machine reads and writes its last word.
#[test]
fn the_buffer_may_fill_its_window() {
    use muir::busint::{Responder, decode_quux_13};
    assert_eq!(tv::VIDEO_MAX_WORDS, 4_193_280);
    let main = 1 << 20;
    let words = tv::VIDEO_MAX_WORDS;
    assert_eq!(decode_quux_13(0o1777775777, main, words), Responder::Memory(0));
    assert_eq!(decode_quux_13(0o1777776000, main, words), Responder::NoXbus);
    assert_eq!(decode_quux_13(PAGE, main, words), Responder::Device);
    assert_eq!(decode_quux_13(PAGE | 0o377, main, words), Responder::Device);
    assert!(tv::check_video_words(4_193_280).is_ok());
    let e = tv::check_video_words(4_193_281).unwrap_err();
    assert!(e.contains("4193280"), "{e}");
    // 8192 by 1023 at one bit is 256 words a line and 261,888 words: past
    // the size QUUX supports, so set here and not through the flag.
    let mut m = quux_with_video();
    m.tv.set_video_size(8192, 1023);
    assert_eq!(m.tv.buffer_words(), 261_888);
    let last = WINDOW_13 + 261_887;
    m.bus_write(last, 0o707070);
    assert_eq!(m.bus_read(last), UNBOXED_TAG | 0o707070, "the buffer's last word");
    assert_eq!(m.tv.read_buffer(261_887), 0o707070);
    assert_eq!(m.bus_read(PAGE), Geometry::QUUX.machine_id.unwrap().into(), "the page's first");
    assert_eq!(m.bus_error, 0);
}

/// **The video controller can be another size, `--video-size`**, and the
/// feature page, the frame, the buffer's end and a checkpoint all follow
/// it: 1920 by 1080, the largest, is 60 words a line and 64,800 words.
#[test]
fn another_size_is_followed_everywhere() {
    let mut m = quux_with_video();
    m.tv.set_video_size(1920, 1080);
    assert_eq!(m.tv.screen(), (1920, 1080, 60));
    assert_eq!(m.tv.buffer_words(), 64_800);
    assert_eq!(m.bus_read(PAGE + 0o11), 1920 << 16 | 1080);
    assert_eq!(m.bus_read(PAGE + 0o12), 1 << 16 | 60);
    let f = Frame::of(&m.tv);
    assert_eq!((f.width, f.height, f.words_per_line), (1920, 1080, 60));
    let last = WINDOW_13 + 64_800 - 1;
    m.bus_write(last, 5);
    assert_eq!(m.bus_read(last), UNBOXED_TAG | 5);
    assert_eq!(m.bus_error & bus_error::XBUS_NXM, 0);
    m.bus_read(last + 1);
    assert_ne!(m.bus_error & bus_error::XBUS_NXM, 0, "one past the end");

    use muir::checkpoint::{Reader, Writer};
    let mut w = Writer::new();
    m.tv.save(&mut w);
    let body = w.finish();
    let mut back = tv::Tv::default();
    back.load(&mut Reader::new(&body)).unwrap();
    assert_eq!((back.board(), back.screen()), (Board::Video, (1920, 1080, 60)));
    assert_eq!(back.read_buffer(64_800 - 1), 5);
    assert_eq!(Board::Video.name(), "video");
}

/// **A size is refused unless a line is whole words and it is at most 1920
/// by 1080**, the largest the video controller supports,
/// which also keeps the buffer below the color TV's strap.
#[test]
fn a_size_is_checked() {
    use tv::check_video_size as check;
    assert!(check(1024, 768, false).is_ok());
    assert!(check(1280, 1024, false).is_ok());
    assert!(check(1280, 1024, true).is_ok());
    assert!(check(1920, 1080, false).is_ok());
    assert!(check(1920, 1080, true).is_ok());
    assert!(check(2560, 1440, false).is_err(), "past 1920 by 1080");
    assert!(check(1952, 1080, false).is_err(), "wider than 1920");
    assert!(check(1920, 1088, false).is_err(), "taller than 1080");
    assert!(check(1921, 1080, false).is_err(), "not whole words");
    assert!(check(3840, 2160, false).is_err(), "259,200 words");
    assert!(check(0, 1080, false).is_err());
}
