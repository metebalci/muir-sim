// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! System 2001 on QUUX revision 13: the first 40-bit band (contract G2).
//!
//! It is QUUX's release, muir-sys's `release-2001`, fetched by
//! `tools/fetch-system-for-quux.sh` into the gitignored
//! `vendor/system-2001/`: System 2001's release disk, a GPT disk of
//! 853,359 blocks as a dynamic VHD, which QUUX boots as it is, with
//! microcode 2001 in its current `MCR1`, "MCR1 UCADR 2001", the band,
//! "LOD1 System 2001", in its current `LOD1`, and a `PAGE` partition of
//! 128MW; PROM 2001, muir's built-in `data/quux-promh.mcr` byte for byte
//! (`tests/quux_prom.rs`); and the sources the band was built from, which
//! unpack to `release-2001/`, with the microcode's files in their
//! `sys/ubin/`. Without it the tests skip and say so.
//!
//! The band boots on both engines at 2MW of main memory, which G2 §3
//! allows tests, and at revision 13's 32MW; it restores its own band
//! (`%disk-restore`), ticks on timer 0, draws on the video controller at
//! the size it is given, writes through the file device, and runs H8a's
//! fused return under the checkers of `support::macro_dispatch`; a reboot
//! finds the timers and the file device reset, the band reaches its
//! listener at three ticks too, and it runs the clocks' codes only where
//! its microcode has them (`support::unused_codes`). The
//! guard of G2 §2.8 that muir-sim can show is here too: PROM 2001 stops
//! on a microcode partition of 32-bit words, on a disk made here.

use std::path::{Path, PathBuf};

use muir::engine::Engine;
use muir::machine::{Geometry, Machine};
use muir::micro::Micro;
use muir::rtl::Rtl;
use muir::tv::Board;

mod support;

/// LISPM-1 and OZ, as the tree's `site/hosts.text` gives them.
const CHAOS: (u16, u16) = (0o177201, 0o177200);

/// Main memory for most runs, in 64K-word boards: 2MW (G2 §3: "tests
/// may run at 2 M words").
const BOARDS: usize = 32;

/// The size the band is booted at.
const BAND_SIZE: (usize, usize) = (1280, 1024);

/// A copy of the release's disk, which the machine writes, and a file root
/// in a scratch directory named `name` (`support::quux_release_band`).
/// `None` with the skip line without the release.
fn band_2001(name: &str) -> Option<(support::Scratch, PathBuf, PathBuf)> {
    support::quux_release_band(name)
}

/// A file of the release's microcode, from its sources' `sys/ubin/`.
fn ubin(file: &str) -> PathBuf {
    support::quux_release(&["sys", "ubin", file]).expect("the release, as its disk was")
}

/// Revision 13 with `boards` of main memory and PROM 2001, the disk on
/// block-disk, the video controller at `w` by `h`, and the file device
/// serving `root` as HOST's `/` and its `sys` and `site` as `/sys` and
/// `/site`, where the band's `SYS:` is.
fn quux_13(pack: &Path, root: &Path, boards: usize, (w, h): (usize, usize)) -> Machine {
    use muir::block_disk::{BLOCK_NS, BlockDisk};
    let mut m = Machine::with_geometry(Geometry::QUUX, boards);
    m.load_prom(&muir::prom::quux_boot_prom());
    let mut d = BlockDisk::new(BLOCK_NS);
    d.attach(muir::disk_image::Disk::open_rw(pack).expect("the pack"));
    m.block_disk = Some(d);
    m.tv.set_board(Board::Video);
    m.tv.set_video_size(w, h);
    m.file_device.mounts.add(&root.display().to_string()).unwrap();
    for part in ["sys", "site"] {
        m.file_device.mounts.add(&format!("{part}={}", root.join(part).display())).unwrap();
    }
    m
}

fn quux(pack: &Path, root: &Path) -> Machine {
    quux_13(pack, root, BOARDS, BAND_SIZE)
}

/// `%MICROCODE-VERSION-NUMBER`, A memory's word 40, as the running machine
/// holds it: the fixnum's value, `<31:0>` of the 40-bit word.
fn microcode_version(e: &impl Engine) -> u32 {
    (e.machine().amem[0o40] & 0xffff_ffff) as u32
}

/// Whether the listener is framed at the screen's own size, and not at any
/// other width: its border lights the first and the last pixel of every
/// row through the middle half of the screen, read at the screen's words a
/// line, and read at any other it does not.
fn drawn_at_its_words_a_line(e: &impl Engine) -> bool {
    let (_, h, own) = e.machine().tv.screen();
    framed(e, own, h)
        && [24, 40, 60, 80].into_iter().filter(|&w| w != own).all(|w| !framed(e, w, h))
}

fn framed(e: &impl Engine, words_per_line: usize, h: usize) -> bool {
    let buf = e.machine().tv.buffer();
    let lit = |bit: usize| buf.get(bit / 32).is_some_and(|w| w >> (bit % 32) & 1 != 0);
    let width = words_per_line * 32;
    (h / 4..3 * h / 4).all(|y| lit(y * width) && lit(y * width + width - 1))
}

/// The screen as a GIF in the temporary directory, for a failure message.
fn shot(e: &impl Engine, name: &str) -> String {
    let path = std::env::temp_dir().join(format!("muir-system-2001-{name}.gif"));
    let mut rec = muir::capture::Recorder::new(false);
    rec.sample(&e.machine().tv, 0, 0);
    std::fs::write(&path, rec.gif()).unwrap();
    path.display().to_string()
}

/// Boots the band at `boards` of main memory on both engines to the
/// listener: microcode 2001 in A memory, and the listener framed at the
/// video controller's words a line and at no other width.
fn boots_on_both_engines(boards: usize) {
    for engine in ["micro", "rtl"] {
        let Some((_dir, pack, root)) = band_2001(&format!("system-2001-{engine}-{boards}")) else {
            return;
        };
        let m = quux_13(&pack, &root, boards, BAND_SIZE);
        let t = std::time::Instant::now();
        let (ran, drawn, version) = match engine {
            "micro" => {
                let mut e = Micro::new(m);
                e.boot();
                let ran = support::boot_to_the_prompt_within(&mut e, CHAOS, root, 600_000_000);
                (ran, drawn_at_its_words_a_line(&e), microcode_version(&e))
            }
            _ => {
                let mut e = Rtl::new(m);
                e.boot();
                let ran = support::boot_to_the_prompt_within(&mut e, CHAOS, root, 600_000_000);
                (ran, drawn_at_its_words_a_line(&e), microcode_version(&e))
            }
        };
        eprintln!(
            "{engine}, {}MW: listener after {ran} microcycles, microcode {version}, {:.1} s",
            boards >> 4,
            t.elapsed().as_secs_f64()
        );
        assert!(drawn, "{engine}: drawn at the screen's words a line");
        assert_eq!(version, 2001, "{engine}: the microcode's version");
    }
}

/// **System 2001 reaches its listener on revision 13**, on both engines,
/// at 2MW.
#[test]
fn system_2001_boots_on_both_engines() {
    boots_on_both_engines(BOARDS);
}

/// **System 2001 boots at revision 13's 32MW**, the boards' size and
/// `quux`'s default (G2 §3), on both engines, to the same listener.
#[test]
fn system_2001_boots_at_32_m_words() {
    let boards = Geometry::QUUX.default_memory_boards();
    assert_eq!(boards << 16, 32 << 20, "32MW");
    boots_on_both_engines(boards);
}

/// The release's microcode's symbols, its `ucadr.sym`.
fn symbols() -> muir::sym::Symbols {
    muir::sym::parse(&std::fs::read_to_string(ubin("ucadr.sym")).unwrap()).unwrap()
}

/// The address of `name` in `space` of the release's microcode.
fn ucadr(name: &str, space: muir::sym::Space) -> u16 {
    symbols().address(space, name).unwrap_or_else(|| panic!("{name} in ucadr.sym")) as u16
}

/// **The band is System 2001 on microcode 2001**, as the disk says: its
/// current `MCR1` is named "MCR1 UCADR 2001" and holds the release's
/// `ucadr.mcr`, whose `A-VERSION` is 2001, with zeros after it, and its
/// current `LOD1` is named "LOD1 System 2001", the only current band. The
/// microcode's A memory is the 40-bit section and its dispatch memory 4,096
/// entries (contract G2 appendix A1.12, A1.4). Its `PAGE` partition is
/// 655,360 blocks, 128MW of virtual memory at 5 blocks a 1024-word
/// page, packed storage's 5 bytes a word. No boot.
#[test]
fn band_2001_is_system_2001_on_microcode_2001() {
    let Some((_dir, pack, _root)) = band_2001("system-2001-names") else { return };
    let bytes = std::fs::read(ubin("ucadr.mcr")).unwrap();
    let mcr = muir::mcr::parse_partition_order(&bytes).unwrap();
    assert_eq!(mcr.version(), Some(2001), "ucadr.mcr's A-VERSION");
    assert!(mcr.amem_wide, "A memory as the 40-bit section");
    assert_eq!(mcr.dmem.len(), 4096, "4,096 dispatch entries");
    let mut d = muir::disk_image::Disk::open(&pack).unwrap();
    let parts = support::gpt_partitions(&mut d);
    let current = |lisp: &str| {
        parts
            .iter()
            .find(|p| p.current && p.name.starts_with(lisp))
            .unwrap_or_else(|| panic!("no current {lisp}: {parts:?}"))
    };
    assert_eq!(current("MCR").name, "MCR1 UCADR 2001");
    assert_eq!(current("LOD").name, "LOD1 System 2001");
    let lods = parts.iter().filter(|p| p.current && p.name.starts_with("LOD")).count();
    assert_eq!(lods, 1, "one current band: {parts:?}");
    let page = parts.iter().find(|p| p.name == "PAGE").expect("a PAGE partition");
    assert_eq!(page.blocks, 655_360, "PAGE: 128MW, 5 blocks a page");
    let mcr1 = current("MCR");
    for k in 0..mcr1.blocks as usize {
        let on_disk: Vec<u8> = d
            .read_block(mcr1.first + k as u32)
            .unwrap()
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        let want = bytes.get(k * 1024..(k + 1) * 1024);
        match want {
            Some(block) => assert!(on_disk == block, "MCR1's block {k} is ucadr.mcr's"),
            None => assert!(on_disk.iter().all(|&b| b == 0), "MCR1's block {k} is zero"),
        }
    }
}

/// **System 2001 sizes its screen at boot, on the video controller**: the
/// band, booted at other sizes than [`BAND_SIZE`], draws its listener at
/// each size's own words a line; the feature page's word 13 says where the
/// frame buffer is, revision 13's window at `1760000000` (G2 §4.3). 1920 by
/// 1080, full HD, the largest screen QUUX supports and the Kria KR260's, is
/// booted on both engines, to its listener at 60 words a line; 1024 by 768
/// on `micro`.
#[test]
fn system_2001_sizes_its_screen_at_boot() {
    for (engine, size) in [("micro", (1024, 768)), ("micro", (1920, 1080)), ("rtl", (1920, 1080))] {
        let name = format!("system-2001-{engine}-{}x{}", size.0, size.1);
        let Some((_dir, pack, root)) = band_2001(&name) else {
            return;
        };
        let m = quux_13(&pack, &root, BOARDS, size);
        let t = std::time::Instant::now();
        match engine {
            "micro" => sized_at_boot(Micro::new(m), root, size, &name),
            _ => sized_at_boot(Rtl::new(m), root, size, &name),
        }
        eprintln!("{engine}, {size:?}: {:.1} s", t.elapsed().as_secs_f64());
    }
}

/// Boots `e` to the listener and checks it drawn at `size`'s words a line,
/// and feature word 13.
fn sized_at_boot(mut e: impl Engine, root: PathBuf, size: (usize, usize), name: &str) {
    e.boot();
    let ran = support::boot_to_the_prompt_within(&mut e, CHAOS, root, 600_000_000);
    eprintln!("{name}: listener after {ran} microcycles");
    assert_eq!(e.machine().tv.screen().2, size.0 / 32, "{name}: the screen's words a line");
    assert!(
        drawn_at_its_words_a_line(&e),
        "{name}: drawn at the screen's words a line; the screen is {}",
        shot(&e, name)
    );
    let m = e.machine_mut();
    let word_13 = m.bus_read(muir::machine::REGISTER_PAGE_13 + 0o13);
    assert_eq!(word_13, 0o1760000000, "{name}: feature word 13");
}

/// **System 2001 restores its own band and comes back to the listener**:
/// booted, `(si:disk-restore 1)` answered `yes` reads LOD1 back in through
/// block-disk's packed transfers and boots it to the listener again, on
/// `micro`; the microcode restored writes the MACRO-DISPATCH register
/// again and returns fuse again.
#[test]
fn system_2001_restores_its_band_to_the_listener() {
    use muir::terminal::keyboard::Keyboard;
    let Some((_dir, pack, root)) = band_2001("system-2001-restore") else { return };
    let lod1 = support::gpt_partition(&mut muir::disk_image::Disk::open(&pack).unwrap(), "LOD1");
    let mut m = quux(&pack, &root);
    m.block_disk.as_mut().unwrap().log = Some(Vec::new());
    let mut e = Micro::new(m);
    e.boot();
    let ran = support::boot_to_the_prompt_within(&mut e, CHAOS, root, 600_000_000);
    eprintln!("listener after {ran} microcycles");
    let mut k = Keyboard::new();
    support::type_at(&mut e, &mut k, "(si:disk-restore 1)");
    // Time for the question, whether to reload LOD1, before its answer.
    for _ in 0..20_000_000 {
        e.step().unwrap();
    }
    support::type_at(&mut e, &mut k, "yes\n");
    let transfers =
        |e: &Micro| e.machine().block_disk.as_ref().unwrap().log.as_ref().unwrap().len();
    let from = transfers(&e);
    // The band read, until the screen goes dark for the boot.
    let mut n = 0u64;
    while support::lit_rows(&e, 84..130) > 400 {
        let before = transfers(&e);
        for _ in 0..1_000_000 {
            e.step().unwrap();
        }
        n += 1_000_000;
        assert!(
            transfers(&e) > before,
            "block-disk still after {n} microcycles; the screen is {}",
            shot(&e, "restore")
        );
        assert!(n < 200_000_000, "the screen never went dark for the boot");
    }
    let log = &e.machine().block_disk.as_ref().unwrap().log.as_ref().unwrap()[from..];
    let band_reads = log
        .iter()
        .filter(|t| !t.write && (lod1.first..lod1.first + lod1.blocks).contains(&t.block))
        .count();
    eprintln!("band read, {band_reads} transfers from LOD1, after {n} microcycles");
    assert!(band_reads > 0, "LOD1 read");
    let fused = e.machine().macro_dispatch.fused;
    let again = support::wait_for_the_prompt_within(&mut e, 600_000_000);
    eprintln!("listener again after {again} microcycles more");
    assert!(
        drawn_at_its_words_a_line(&e),
        "the listener again, at the screen's words a line; the screen is {}",
        shot(&e, "restored")
    );
    let d = &e.machine().macro_dispatch;
    assert_eq!(d.register >> 31, 1, "the register enabled again after the restore");
    assert!(d.fused > fused, "returns fused again after the restore");
    assert_eq!(microcode_version(&e), 2001);
}

/// Revision 13's register page, `1777777400` (G2 §4.1), and the words of
/// it the timers use (contract Q11's offsets, unchanged).
const PAGE: u32 = muir::machine::REGISTER_PAGE_13;
const fn control(k: usize) -> u32 {
    PAGE + 0o110 + 2 * k as u32
}
const fn period(k: usize) -> u32 {
    PAGE + 0o111 + 2 * k as u32
}

/// **PROM 2001 resets the devices and writes timer 0's period before the
/// disk** (contract Q11, M9), at revision 13's register page: from
/// power-on, its writes show word
/// 104 with `<0>` set, then word 111 with 16,667, and no other timer word,
/// before its first block-disk command (words 200-203); and at the
/// microcode's location 6 timer 0 is off with that period and timers 1 and
/// 2 are in their reset state.
#[test]
fn prom_2001_resets_the_devices_and_writes_timer_0_s_period() {
    use muir::machine::{IntervalTimer, Timers};
    let Some((_dir, pack, root)) = band_2001("system-2001-m9") else { return };
    let mut m = quux(&pack, &root);
    m.register_log = Some(Vec::new());
    let mut e = Micro::new(m);
    e.boot();
    let mut n = 0u64;
    while e.executed() != Some(6) {
        e.step().unwrap();
        n += 1;
        assert!(n < 50_000_000, "location 6 never ran");
    }
    eprintln!("location 6 after {n} microcycles on micro");
    let log = e.machine().register_log.clone().unwrap();
    let disk = log
        .iter()
        .position(|&(a, _, _)| (PAGE + 0o200..PAGE + 0o204).contains(&a))
        .expect("a block-disk command");
    let timers: Vec<_> = log[..disk]
        .iter()
        .filter(|&&(a, _, _)| a == PAGE + 0o104 || (control(0)..=period(2)).contains(&a))
        .map(|&(a, v, c)| (a - PAGE, v, c))
        .collect();
    eprintln!("before the first disk command: {timers:?}");
    assert_eq!(timers.len(), 2, "word 104 and word 111 alone: {timers:?}");
    assert_eq!((timers[0].0, timers[0].1 & 1), (0o104, 1), "reset devices first");
    assert_eq!((timers[1].0, timers[1].1), (0o111, Timers::TICK_PERIOD_US), "then the period");
    let t = e.machine().timers.timer;
    assert_eq!(
        t[0],
        IntervalTimer { period_us: Timers::TICK_PERIOD_US, ..IntervalTimer::RESET },
        "timer 0 at location 6"
    );
    assert_eq!([t[1], t[2]], [IntervalTimer::RESET; 2], "timers 1 and 2 at location 6");
}

fn run_ns(e: &mut Micro, ns: u64) {
    let t = e.machine().ns;
    while e.machine().ns < t + ns {
        e.step().unwrap();
    }
}

/// Types `text` a character at a time, 3 ms of simulated time after each.
fn type_slow(e: &mut Micro, k: &mut muir::terminal::keyboard::Keyboard, text: &str) {
    for ch in text.chars() {
        support::type_at(e, k, &ch.to_string());
        run_ns(e, 3_000_000);
    }
}

/// **System 2001 ticks on timer 0 and says what it is through the file
/// device**, on `micro`: at its listener word 110 reads 401, timer 0 on,
/// periodic, under its interrupt enable (its flag, `<1>`, masked), and word
/// 111 16,667; over 10 s of simulated time `INTR-TICK` executes 600 times,
/// give or take one. Then, logged in, it writes its herald,
/// `si:print-herald`, to `HOST://home//lispm//herald`, whose Machine Type
/// line names the board from feature words 20-24, "QUUX on muir-sim", and
/// whose memory line says 2MW of main memory and 128MW of virtual; it
/// reads a form from `HOST://home//lispm//in` and writes to
/// `HOST://home//lispm//versions`, through the file device, 3, its
/// microcode's version, its machine type, its herald's line,
/// `most-positive-fixnum` --- 2^31 - 1 on a 40-bit band (G1 §2.2) --- the
/// form it read, and the count `(time)` moved over a `process-sleep` of 60.
#[test]
fn system_2001_ticks_and_says_what_it_is_through_the_file_device() {
    use muir::machine::Timers;
    use muir::terminal::keyboard::Keyboard;
    let Some((_dir, pack, root)) = band_2001("system-2001-m10") else { return };
    std::fs::write(root.join("home/lispm/in"), "(7 \"eight\" 9.5)\n").unwrap();
    let tick = ucadr("INTR-TICK", muir::sym::Space::IMem);
    let mut e = Micro::new(quux(&pack, &root));
    e.boot();
    let ran = support::boot_to_the_prompt_within(&mut e, CHAOS, root.clone(), 600_000_000);
    eprintln!("listener after {ran} microcycles");
    let m = e.machine_mut();
    let (c0, p0) = (m.bus_read(control(0)), m.bus_read(period(0)));
    let t0 = e.machine().ns;
    let mut ticks = 0u32;
    while e.machine().ns < t0 + 10_000_000_000 {
        e.step().unwrap();
        ticks += (e.executed() == Some(tick)) as u32;
    }
    eprintln!("word 110 {c0:o}, word 111 {p0}; INTR-TICK ran {ticks} times in 10 s");
    assert_eq!(c0 & !2, 0o401, "word 110: timer 0 on, periodic, interrupt enable");
    assert_eq!(p0, Timers::TICK_PERIOD_US.into(), "word 111");
    assert!(ticks.abs_diff(600) <= 1, "INTR-TICK ran {ticks} times in 10 s");
    let mut k = Keyboard::new();
    type_slow(&mut e, &mut k, "(login \"LISPM\" \"HOST\" t)\n");
    run_ns(&mut e, 2_000_000_000);
    type_slow(
        &mut e,
        &mut k,
        "(progn (with-open-file (s \"HOST://home//lispm//herald\" :direction :output) \
         (si:print-herald s)) \
         (let ((t0 (time)) (in (with-open-file (s \"HOST://home//lispm//in\") (read s)))) \
         (process-sleep 60.) (with-open-file (s \"HOST://home//lispm//versions\" :direction \
         :output) (format s \"~S ~S ~S ~S END~%\" (list (+ 1 2) %microcode-version-number \
         (si:machine-type) (si:system-version-info)) most-positive-fixnum in \
         (time-difference (time) t0)))))\n",
    );
    let file = root.join("home/lispm/versions");
    let t = e.machine().ns;
    let mut said = None;
    while e.machine().ns < t + 60_000_000_000 {
        run_ns(&mut e, 50_000_000);
        if let Ok(s) = std::fs::read_to_string(&file)
            && s.contains("END")
        {
            said = Some(s);
            break;
        }
    }
    eprintln!("the band says {said:?}");
    let said = said.unwrap_or_else(|| panic!("nothing written; the screen is {}", shot(&e, "m10")));
    let (what, moved) = said.trim_end().strip_suffix(" END").unwrap().rsplit_once(' ').unwrap();
    assert_eq!(
        what,
        r#"(3 2001 "QUUX" "Experimental System 2001, microcode 2001") 2147483647 (7 "eight" 9.5)"#
    );
    let moved: u32 = moved.parse().unwrap();
    assert!((60..90).contains(&moved), "(time) moved {moved} over a second's sleep");
    let herald = std::fs::read_to_string(root.join("home/lispm/herald")).unwrap();
    eprintln!("its herald:\n{herald}");
    let line = |starts: &str| {
        herald
            .lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .find(|l| l.starts_with(starts))
            .unwrap_or_else(|| panic!("no line {starts:?} in the herald {herald:?}"))
    };
    assert_eq!(line("Machine Type"), "Machine Type QUUX on muir-sim", "the herald's machine");
    assert_eq!(line("Microcode"), "Microcode 2001", "the herald's microcode");
    assert_eq!(
        line("2048K"),
        "2048K physical memory, 131072K virtual memory.",
        "the herald's memory: 2MW main, 128MW virtual"
    );
}

/// Steps `e` until the microcode's main loop, `QMLP`, has run once with
/// the PC out of the PROM (on `rtl` a control-store write's second
/// microcycle stands at the address written), and the MACRO-DISPATCH
/// register enabled, which the band's microcode does at `RESET-MACHINE`.
fn to_the_main_loop(e: &mut impl Engine, qmlp: u16) {
    use muir::machine::macro_dispatch::ENABLE;
    for _ in 0..200_000_000 {
        if e.machine().opc == qmlp
            && e.pc() < muir::machine::QUUX_PROM_BASE
            && e.machine().macro_dispatch.register & ENABLE != 0
        {
            return;
        }
        e.step().unwrap();
    }
    panic!("QMLP never ran with the register enabled");
}

/// Who fills the MACRO DISPATCH MEMORY for [`boots_with_the_fused_return`].
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fill {
    /// The band's microcode, at `RESET-MACHINE`.
    Microcode,
    /// The test, once the microcode has filled it: every entry `OPDTB`'s
    /// generic handler, with the operand bit wherever `<8:0>` is a register
    /// and a delta (`fill_generic`).
    Generic,
}

/// The entries whose handler, `<13:0>`, is not `OPDTB`'s for their
/// opcode: the specialised ones.
fn specialised_entries(m: &Machine, opdtb: u16) -> usize {
    m.macro_dispatch
        .entries
        .iter()
        .enumerate()
        .filter(|&(k, &x)| x & 0o37777 != m.dmem[opdtb as usize + (k >> 3 & 0o37)] & 0o37777)
        .count()
}

/// Boots the band on `make`'s engine with the MACRO DISPATCH MEMORY as
/// `fill` says, under the checkers of `support::macro_dispatch` from the
/// first main-loop return with the register enabled, to the listener.
fn boots_with_the_fused_return<E: support::macro_dispatch::Executes>(
    engine: &str,
    make: impl Fn(Machine) -> E,
    fill: Fill,
) {
    use muir::sym::Space;
    use support::macro_dispatch::{Checked, fill_generic};
    let Some((_dir, pack, root)) = band_2001(&format!("system-2001-fused-{engine}-{fill:?}"))
    else {
        return;
    };
    let (qmlp, opdtb) = (ucadr("QMLP", Space::IMem), ucadr("OPDTB", Space::DMem));
    let (localp, ap) = (ucadr("A-LOCALP", Space::AMem), ucadr("M-AP", Space::MMem));
    let mut e = make(quux(&pack, &root));
    e.boot();
    to_the_main_loop(&mut e, qmlp);
    let word = muir::machine::macro_dispatch::word(qmlp, localp, ap as u8);
    assert_eq!(e.machine().macro_dispatch.register, word, "{engine}: the microcode's register");
    let specialised = specialised_entries(e.machine(), opdtb);
    if fill == Fill::Generic {
        fill_generic(e.machine_mut(), qmlp, opdtb, localp, ap as u8, true);
    }
    let mut e = Checked::new(e);
    let n = support::boot_to_the_prompt_within(&mut e, CHAOS, root, 600_000_000);
    let m = e.machine();
    let c = &e.checker.counts;
    eprintln!(
        "{engine}, {fill:?}: listener after {n} steps, {} fused returns; the microcode's fill \
         had {specialised} specialised entries",
        m.macro_dispatch.fused
    );
    eprintln!("{engine}, {fill:?}: {}", c.report(|pc| format!("{pc:o}")));
    assert!(drawn_at_its_words_a_line(&e), "{engine}, {fill:?}: the listener, fused");
    assert!(m.macro_dispatch.fused > 0, "{engine}, {fill:?}: returns fused");
    assert_eq!(c.fused, m.macro_dispatch.fused, "{engine}, {fill:?}: every one checked");
    assert!(c.operand_loads > 0, "{engine}, {fill:?}: operand addresses loaded");
    assert_eq!(c.problems(), 0, "{engine}, {fill:?}: the rule of §3.3 kept and every check met");
    assert_eq!(m.macro_dispatch.register, word, "{engine}, {fill:?}: still enabled");
    let now = specialised_entries(m, opdtb);
    match fill {
        Fill::Microcode => {
            assert!(specialised > 0, "{engine}: the microcode's specialised entries");
            assert_eq!(now, specialised, "{engine}: the entries as the microcode left them");
        }
        Fill::Generic => assert_eq!(now, 0, "{engine}: the generic entries kept"),
    }
    assert_eq!(microcode_version(&e), 2001);
}

/// **System 2001 boots with the fused return** (contract H8a §6 items
/// 4-6), on `rtl`: microcode
/// 2001 fills the MACRO DISPATCH MEMORY with its specialised handlers,
/// family 4's rewritten for the ALU on tags (G2 §2.7), enables the register
/// with its `QMLP`, `A-LOCALP` and `M-AP` before its first main-loop
/// return, and reaches the listener with returns fused and the register
/// still enabled, every fused return checked: the rule of §3.3, the
/// handler's first microinstruction reading no PDL word that microcycle
/// writes, the handler the main loop would have reached running next, the
/// operand address, the base copies, and M 31 after a return fused on the
/// prefetched word.
#[test]
fn system_2001_boots_with_the_fused_return_on_rtl() {
    boots_with_the_fused_return("rtl", Rtl::new, Fill::Microcode);
}

/// The same on `micro`.
#[test]
fn system_2001_boots_with_the_fused_return_on_micro() {
    boots_with_the_fused_return("micro", Micro::new, Fill::Microcode);
}

/// **System 2001 boots with the generic handlers and the operand bit on
/// every LOCAL and ARG entry** (contract H8a §6 item 5), on `rtl`, and the
/// checkers find nothing.
#[test]
fn system_2001_boots_with_the_generic_operand_fill_on_rtl() {
    boots_with_the_fused_return("rtl", Rtl::new, Fill::Generic);
}

/// The same on `micro`.
#[test]
fn system_2001_boots_with_the_generic_operand_fill_on_micro() {
    boots_with_the_fused_return("micro", Micro::new, Fill::Generic);
}

/// **Microcode 2001 fills a stale MACRO DISPATCH MEMORY again** (contract
/// H8a §6 item 3): with the register enabled for its main loop and every
/// entry poisoned to `ILLOP` before the boot, the PROM's load of the
/// microcode clears the enable, and the microcode writes every entry and
/// the register at `RESET-MACHINE`: the band reaches its listener with
/// returns fused, no entry left poisoned and the register its own.
#[test]
fn a_stale_macro_dispatch_memory_is_filled_again() {
    use muir::sym::Space;
    use support::macro_dispatch::set_register;
    let Some((_dir, pack, root)) = band_2001("system-2001-stale") else { return };
    let (qmlp, illop) = (ucadr("QMLP", Space::IMem), ucadr("ILLOP", Space::IMem));
    let (localp, ap) = (ucadr("A-LOCALP", Space::AMem), ucadr("M-AP", Space::MMem));
    let mut e = Micro::new(quux(&pack, &root));
    e.boot();
    e.machine_mut().macro_dispatch.entries.fill(illop as u32);
    set_register(e.machine_mut(), muir::machine::macro_dispatch::word(qmlp, 0, 0));
    let n = support::boot_to_the_prompt_within(&mut e, CHAOS, root, 600_000_000);
    eprintln!("listener after {n} steps");
    assert!(drawn_at_its_words_a_line(&e), "the listener");
    let d = &e.machine().macro_dispatch;
    assert!(d.fused > 0, "returns fused");
    assert_eq!(d.register, muir::machine::macro_dispatch::word(qmlp, localp, ap as u8));
    let poisoned = d.entries.iter().filter(|&&x| x == illop as u32).count();
    assert_eq!(poisoned, 0, "entries left poisoned");
}

/// Boots `m` on `make`'s engine and steps it until its PC has stayed at
/// `at` for 1,000 microcycles, a halt's loop, within `limit`: after how
/// many microcycles it got there, or `None` with where the PC was.
fn stops_at<E: Engine>(
    make: impl Fn(Machine) -> E,
    m: Machine,
    at: u16,
    limit: u64,
) -> Result<u64, u16> {
    let mut e = make(m);
    e.boot();
    let mut there = 0;
    for n in 0..limit {
        e.step().unwrap();
        there = if e.pc() == at { there + 1 } else { 0 };
        if there == 1000 {
            return Ok(n - 999);
        }
    }
    Err(e.pc())
}

/// [`stops_at`] on both engines, which have to agree.
fn both_stop_at(m: &Machine, at: u16, limit: u64) {
    for (engine, r) in [
        ("micro", stops_at(Micro::new, m.clone(), at, limit)),
        ("rtl", stops_at(Rtl::new, m.clone(), at, limit)),
    ] {
        match r {
            Ok(n) => eprintln!("{engine}: at {at:o} after {n} microcycles"),
            Err(pc) => panic!("{engine}: not at {at:o} after {limit} microcycles, at {pc:o}"),
        }
    }
}

/// The PROM `prom` with every jump to `to` that is not the halt's own
/// loop zeroed: the guard taken out, for a control.
fn without_jumps_to(prom: &[muir::isa::Insn], to: u16) -> Vec<muir::isa::Insn> {
    let base = muir::machine::QUUX_PROM_BASE;
    let mut p = prom.to_vec();
    let mut taken = 0;
    for (k, w) in p.iter_mut().enumerate() {
        let at = base + k as u16;
        if at != to && w.raw() >> 43 & 3 == 1 && w.jump().target == to {
            *w = muir::isa::Insn::new(0);
            taken += 1;
        }
    }
    assert!(taken > 0, "no jump to {to:o}");
    p
}

/// `ERROR-A-MEM-SECTION-32-BITS`, PROM 2001's halt for a microcode
/// partition whose A memory is section 4, 32 bits (`tests/quux_prom.rs`
/// holds it to the PROM's `promh.sym` and `promh.tbl`).
const ERROR_A_MEM_SECTION_32_BITS: u16 = 0o36661;

/// **A 32-bit microcode on revision 13 stops at PROM 2001's named halt**
/// (contract G2 §2.8): a GPT disk made here from committed files, whose
/// current `MCR1` holds MIT's microcode 323 in partition order, its A
/// memory section 4, 32 bits, as every 32-bit microcode has it, booted
/// on revision 13 with PROM 2001 stops at `ERROR-A-MEM-SECTION-32-BITS`, on
/// both engines, before any microcode runs. The control: the same PROM
/// with its jump to that halt taken out does not stop there, so the test
/// fails without the guard. It needs no release.
#[test]
fn a_32_bit_microcode_on_revision_13_stops_at_prom_2001_s_halt() {
    let dir = support::scratch("system-2001-guard-32-bit-microcode");
    let (pack, _) = support::quux_gpt_disk(&dir, &support::ucadr_323_partition_order());
    for part in ["sys", "site"] {
        std::fs::create_dir_all(dir.join(part)).unwrap();
    }
    let m = quux(&pack, &dir);
    both_stop_at(&m, ERROR_A_MEM_SECTION_32_BITS, 50_000_000);
    let mut control = m;
    control
        .load_prom(&without_jumps_to(&muir::prom::quux_boot_prom(), ERROR_A_MEM_SECTION_32_BITS));
    let r = stops_at(Micro::new, control, ERROR_A_MEM_SECTION_32_BITS, 50_000_000);
    eprintln!("control: {:?}", r.map_err(|pc| format!("at {pc:o}")));
    assert!(r.is_err(), "the control stopped there too");
}

/// **System 2001 uses the clocks' codes only where its microcode says so**,
/// through its boot to the listener and a moment after on revision 13
/// (`support::unused_codes`'s scan, on `rtl`): no instruction the OA
/// registers make writes destinations 3 to 7 or reads sources 15 or 17.
/// And what it writes: nothing to destination 3 or 4, its tick being timer
/// 0 on the register page, which is on at the end; destinations 5 to 7
/// only in `RESET-MACHINE`'s fill of the MACRO DISPATCH MEMORY, from
/// `RESET-MACHINE-MACRO-DISPATCH-FILL` up to
/// `RESET-MACHINE-MACRO-DISPATCH-DONE` in the release's `ucadr.sym`
/// (contract H8a); and it reads source 17 nowhere.
#[test]
fn system_2001_uses_the_clocks_codes_only_where_its_microcode_does() {
    use muir::sym::Space;
    use support::unused_codes::{octal, run};
    let Some((_dir, pack, root)) = band_2001("system-2001-unused-codes") else { return };
    let found = run(quux(&pack, &root), CHAOS, root);
    let (used, made) = (&found.used, &found.made);
    eprintln!("2001: the codes ran at {}", octal(used));
    assert!(!used.is_empty(), "the clocks' own sites ran");
    assert!(made.is_empty(), "made by the OA registers at {}", octal(made));
    assert!(found.dest_3.is_empty(), "destination 3 written: {:?}", found.dest_3);
    assert!(found.dest_4.is_empty(), "destination 4 written: {:?}", found.dest_4);
    let fill = ucadr("RESET-MACHINE-MACRO-DISPATCH-FILL", Space::IMem)
        ..ucadr("RESET-MACHINE-MACRO-DISPATCH-DONE", Space::IMem);
    let mut sites: Vec<u16> = found.dest_5_to_7.iter().map(|&(pc, _)| pc).collect();
    sites.sort();
    sites.dedup();
    eprintln!("2001: destinations 5 to 7 written at {}", octal(&sites));
    assert!(!sites.is_empty(), "destinations 5 to 7 never written");
    let outside: Vec<u16> = sites.iter().copied().filter(|pc| !fill.contains(pc)).collect();
    assert!(
        outside.is_empty(),
        "destinations 5 to 7 written outside the fill at {}",
        octal(&outside)
    );
    assert!(found.source_17.is_empty(), "source 17 read at {}", octal(&found.source_17));
    assert!(found.timer_0_on, "timer 0 on at the end");
}

/// **System 2001 runs at its ticks**: QUUX's microcycle is `sync`'s K
/// ticks of 10 ns. The same microcode and band reach the same listener at
/// four ticks and at three, on `rtl`, and the time to it is shorter at
/// three by less than the microcycles' ratio, the memory port keeping its
/// own time.
#[test]
fn system_2001_runs_at_its_ticks() {
    use muir::clock::TimingModel;
    let mut times = Vec::new();
    for ticks in [4, 3] {
        let model = TimingModel::Sync { cycle_ticks: ticks, ilong_ticks: 0 };
        let Some((_dir, pack, root)) = band_2001(&format!("system-2001-sync-{ticks}")) else {
            return;
        };
        let mut e = Rtl::new(quux(&pack, &root));
        e.set_timing_model(model);
        e.boot();
        let ran = support::boot_to_the_prompt_within(&mut e, CHAOS, root, 600_000_000);
        assert!(
            drawn_at_its_words_a_line(&e),
            "{ticks} ticks: at the screen's words a line; the screen is {}",
            shot(&e, &format!("sync-{ticks}"))
        );
        eprintln!("{ticks} ticks: listener after {ran} microcycles, {} ns", e.ns());
        times.push(e.ns());
    }
    let ratio = times[0] as f64 / times[1] as f64;
    assert!(ratio > 1.0 && ratio < 4.0 / 3.0, "{ratio:.3} times faster at three ticks");
}

/// Steps `e` until the listener has been drawn afresh --- the screen dark
/// first, then the listener --- within `limit` microcycles: how many it
/// took, or `None`, calling `each` on every microcycle.
fn to_a_new_listener(e: &mut Micro, limit: u64, mut each: impl FnMut(&Micro)) -> Option<u64> {
    let listening = |e: &Micro| support::lit_rows(e, 84..130) > 400;
    let mut dark = !listening(e);
    let mut n = 0u64;
    while n < limit {
        for _ in 0..100_000 {
            e.step().expect("halted");
            each(e);
        }
        n += 100_000;
        if !dark {
            dark = !listening(e);
        } else if listening(e) {
            return Some(n);
        }
    }
    None
}

/// The console's way to a PC without `-RESET`: halt, a `JUMP` through the
/// debug IR --- loaded with `NOP11`, run with `IDEBUG` --- and run again.
fn jump_to(e: &mut Micro, pc: u16) {
    use muir::isa::asm::{ALWAYS, JUMP, N, target};
    use muir::spy;
    let clock = |e: &mut Micro, clk: u16| {
        e.spy_write(spy::CLK, clk);
        e.step().unwrap();
        e.step().unwrap();
        e.spy_write(spy::CLK, 0);
        e.step().unwrap();
        e.step().unwrap();
    };
    e.spy_write(spy::CLK, 0);
    e.step().unwrap();
    e.step().unwrap();
    let insn = JUMP | target(pc as u64) | ALWAYS | N;
    e.spy_write(spy::IR_LOW, insn as u16);
    e.spy_write(spy::IR_MED, (insn >> 16) as u16);
    e.spy_write(spy::IR_HIGH, (insn >> 32) as u16);
    clock(e, 0o16);
    clock(e, 0o12);
    e.spy_write(spy::CLK, 1);
    for _ in 0..8 {
        e.step().unwrap();
        if e.executed() == Some(pc) {
            return;
        }
    }
    panic!("the jump to {pc:o} never ran");
}

/// Commands to queue on a machine, later.
type Queue = Box<dyn Fn(&mut Machine)>;

/// What a reboot measures.
#[derive(Debug)]
struct Reboot {
    /// Microcycles from 36000 to the listener, if it came within the limit.
    to_listener: Option<u64>,
    /// `INTR`'s executions over the same.
    intr: u64,
    /// At the microcode's location 6: words 112 and 114, and 161.
    at_6: [u32; 3],
    /// A queued command ran: a response written by location 6, or the
    /// folder the last one creates, by the end.
    ran: bool,
}

/// The file device's rings and buffers for a reboot, high in 2MW of main
/// memory.
const CMD_RING: u32 = 0o7000000;
const RESP_RING: u32 = 0o7000100;
const NAME: u32 = 0o7000200;
const BUF: u32 = 0o7100000;

/// **A reboot**: boot to the listener; if `loaded`, halt, turn timers 1
/// and 2 on under their interrupt enables (1 periodic at 1,000 us, 2
/// one-shot at 5,000 us), let 6 ms pass so that both are up, and leave the
/// file device enabled with an OPEN of `/big` done and three READs of 64
/// KiB each and a CREATE-DIRECTORY queued, the first due 6.4 ms after
/// PROM 2001's write of word 102, its step 4, just before its reset
/// devices. Then the PC to 36000 without `-RESET`, and on to the listener,
/// within `limit` microcycles. The timers are turned on with the machine
/// halted because microcode 2001 turns off a timer 1 or 2 that interrupts
/// (`INTR-TIMER-1-STRAY` and `INTR-TIMER-2-STRAY` in the release's
/// `ucadr.sym`), which a running band would do before the reboot.
fn reboot(pack: &Path, root: &Path, loaded: bool, limit: u64) -> Reboot {
    use muir::file_device::op;
    use muir::spy;
    let (intr, intr_at) = (ucadr("INTR", muir::sym::Space::IMem), 6u16);
    let mut e = Micro::new(quux(pack, root));
    e.boot();
    let ran = support::boot_to_the_prompt_within(&mut e, CHAOS, root.to_path_buf(), 600_000_000);
    eprintln!("listener after {ran} microcycles");
    let post = |m: &mut Machine, k: u32, words: [u32; 8]| {
        let slot = (CMD_RING + 8 * (k % 4)) as usize;
        for (m, w) in m.main[slot..slot + 8].iter_mut().zip(words) {
            *m = w.into();
        }
        m.bus_write(PAGE + 0o164, (k + 1).into());
    };
    let mut queue: Option<Queue> = None;
    if loaded {
        // Halted, so that the band touches neither the timers nor the
        // device's memory.
        e.spy_write(spy::CLK, 0);
        e.step().unwrap();
        e.step().unwrap();
        let m = e.machine_mut();
        m.bus_write(period(1), 1_000);
        m.bus_write(control(1), 0o401);
        m.bus_write(period(2), 5_000);
        m.bus_write(control(2), 0o405);
        let t = m.ns;
        while e.machine().ns < t + 6_000_000 {
            e.step().unwrap();
        }
        let m = e.machine_mut();
        assert_eq!(m.bus_read(control(1)) & 3, 3, "timer 1 on and up");
        assert_eq!(m.bus_read(control(2)) & 3, 3, "timer 2 on and up");
        // The rings, the device enabled, and a file opened.
        m.main[CMD_RING as usize..NAME as usize + 8].fill(0);
        m.bus_write(PAGE + 0o162, CMD_RING.into());
        m.bus_write(PAGE + 0o163, 2);
        m.bus_write(PAGE + 0o166, RESP_RING.into());
        m.bus_write(PAGE + 0o167, 2);
        m.bus_write(PAGE + 0o160, 1);
        m.main[NAME as usize] = u64::from(u32::from_le_bytes(*b"/big"));
        post(m, 0, [1 | op::OPEN << 16, 0, NAME, 4, 0, 0, 0, 0]);
        let t = m.ns;
        while e.machine().file_device.response_producer() == 0 {
            e.step().unwrap();
            assert!(e.machine().ns < t + 1_000_000, "the OPEN answered, the machine halted");
        }
        let m = e.machine_mut();
        let handle = support::low(m.main[RESP_RING as usize + 2]);
        assert_eq!(m.main[RESP_RING as usize] >> 16 & 0xff, 0, "the OPEN answered");
        m.bus_write(PAGE + 0o171, 1);
        // Queued at the PROM's step 4, below.
        queue = Some(Box::new(move |m: &mut Machine| {
            for k in 1..4 {
                post(m, k, [(1 + k) | (op::READ << 16), handle, 0, 0, BUF, 65_536, 0, 0]);
            }
            m.main[NAME as usize..NAME as usize + 2].copy_from_slice(&[
                u32::from_le_bytes(*b"/new").into(),
                u32::from_le_bytes(*b"dir\0").into(),
            ]);
            post(m, 4, [5 | op::CREATE_DIRECTORY << 16, 0, NAME, 7, 0, 0, 0, 0]);
        }));
    }
    e.machine_mut().main[RESP_RING as usize..RESP_RING as usize + 32].fill(0);
    e.machine_mut().register_log = Some(Vec::new());
    jump_to(&mut e, muir::machine::QUUX_PROM_BASE);
    if let Some(queue) = queue {
        let mut n = 0;
        while !e.machine().register_log.as_ref().unwrap().iter().any(|w| w.0 == PAGE + 0o102) {
            e.step().unwrap();
            n += 1;
            assert!(n < 5_000_000, "the PROM never wrote word 102");
        }
        eprintln!("the PROM's step 4 after {n} microcycles; the commands queued");
        queue(e.machine_mut());
    }
    let mut intrs = (e.executed() == Some(intr)) as u64;
    let to_6 = {
        let mut n = 0u64;
        loop {
            e.step().unwrap();
            n += 1;
            intrs += (e.executed() == Some(intr)) as u64;
            if e.executed() == Some(intr_at) {
                break;
            }
            assert!(n < 50_000_000, "location 6 never ran");
        }
        n
    };
    let m = e.machine_mut();
    let at_6 = [m.bus_read(control(1)), m.bus_read(control(2)), m.bus_read(PAGE + 0o161)]
        .map(support::low);
    let answered = m.main[RESP_RING as usize..RESP_RING as usize + 32].iter().any(|&w| w != 0);
    let to_listener = to_a_new_listener(&mut e, limit, |e| {
        intrs += (e.executed() == Some(intr)) as u64;
    })
    .map(|n| n + to_6);
    let created = root.join("newdir").exists();
    Reboot { to_listener, intr: intrs, at_6, ran: answered || created }
}

/// A reboot's pass criterion, against the baseline's figures.
fn reboot_verdict(run: &Reboot, base: &Reboot) -> Result<(), String> {
    let mut why = Vec::new();
    let (Some(b), Some(r)) = (base.to_listener, run.to_listener) else {
        return Err(format!("no listener: {run:?} against {base:?}"));
    };
    if run.intr as f64 > 1.1 * base.intr as f64 + 10.0 {
        why.push(format!("INTR ran {} times against the baseline's {}", run.intr, base.intr));
    }
    if r as f64 > 1.05 * b as f64 {
        why.push(format!("{r} microcycles to the listener against the baseline's {b}"));
    }
    if run.at_6[0] & 1 != 0 || run.at_6[1] & 1 != 0 {
        why.push(format!("timers 1 and 2 at location 6: {:o} {:o}", run.at_6[0], run.at_6[1]));
    }
    if run.at_6[2] & 1 != 0 || run.at_6[2] >> 16 & 0xff != 0 {
        why.push(format!("161 at location 6: {:x}", run.at_6[2]));
    }
    if run.ran {
        why.push("a queued command ran".into());
    }
    if why.is_empty() { Ok(()) } else { Err(why.join("; ")) }
}

/// **A reboot resets the timers and the file device** (contract Q11's
/// M11, and the Q9 amendment's R3), on revision 13 with PROM 2001 and
/// microcode 2001: timers 1 and 2 left on and up under their interrupt
/// enables, and the file device enabled with commands queued due after the
/// PROM's reset devices; a jump to 36000 without `-RESET` then reaches the
/// listener as a reboot with neither does --- `INTR` at most 1.1 times the
/// baseline's executions and 10, the microcycles at most 1.05 times ---
/// with timers 1 and 2 off and the device disabled, no handle open, at
/// location 6, and no queued command run.
#[test]
fn a_reboot_resets_the_timers_and_the_file_device() {
    let mut runs = Vec::new();
    for loaded in [false, true] {
        let Some((_dir, pack, root)) = band_2001(&format!("system-2001-reboot-{loaded}")) else {
            return;
        };
        std::fs::write(root.join("big"), vec![5u8; 300_000]).unwrap();
        let limit = match runs.first() {
            Some(Reboot { to_listener: Some(n), .. }) => 3 * n,
            Some(base) => panic!("the baseline reached no listener: {base:?}"),
            None => 600_000_000,
        };
        let run = reboot(&pack, &root, loaded, limit);
        eprintln!("{}: {run:?}", if loaded { "loaded" } else { "baseline" });
        runs.push(run);
    }
    reboot_verdict(&runs[1], &runs[0]).unwrap();
}
