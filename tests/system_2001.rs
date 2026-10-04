// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! System 2001 on QUUX revision 13: the first 40-bit band (contract G2).
//!
//! It is muir-sys's hand-over of System 2001, microcode 2001 and PROM 2001,
//! none of them released, published as the pre-release
//! `handover-2001-81b3973` and fetched by `tools/fetch-handover-2001.sh`
//! into the gitignored `ref/band-2001-81b3973`: System 2001's release disk,
//! a GPT disk of 853,359 blocks as a dynamic VHD, which QUUX boots as it is,
//! with microcode 2001 in its current `MCR1`, "MCR1 UCADR 2001", the band,
//! "LOD1 System 2001", in its current `LOD1`, and a `PAGE` partition of
//! 128MW; the microcode's and the PROM's files, the PROM being muir's
//! built-in `data/quux-promh.mcr` byte for byte (`tests/quux_prom.rs`); and
//! the sources the band was built from, which unpack to
//! `release-2001-81b3973/`. Without it the tests skip and say so.
//!
//! The band boots on both engines at 2MW of main memory, which G2 §3
//! allows tests, and at revision 13's 32MW; it restores its own band
//! (`%disk-restore`), ticks on timer 0, draws on the video controller at
//! the size it is given, writes through the file device, and runs H8a's
//! fused return under the checkers of `support::macro_dispatch`. The
//! guards of G2 §2.8 that muir-sim can show are here too: each revision's
//! PROM stops on the other revision's disk.

use std::path::{Path, PathBuf};

use muir::engine::Engine;
use muir::machine::{Geometry, Machine};
use muir::micro::Micro;
use muir::rtl::Rtl;
use muir::tv::Board;

mod support;

/// LISPM-1 and OZ, as the tree's `site/hosts.text` gives them.
const CHAOS: (u16, u16) = (0o177201, 0o177200);

/// muir-sys's hand-over of System 2001, as `tools/fetch-handover-2001.sh`
/// leaves it.
const BAND_2001: &str = "ref/band-2001-81b3973";

/// The hand-over's disk, decompressed by the fetch.
const DISK: &str = "handover-2001-81b3973-disk.vhd";

/// The hand-over's sources.
const SOURCES: &str = "handover-2001-81b3973-sys.tar.gz";

/// The directory the hand-over's tree unpacks to.
const TREE: &str = "release-2001-81b3973";

/// Main memory for most runs, in 64K-word boards: 2MW (G2 §3: "tests
/// may run at 2 M words").
const BOARDS: usize = 32;

/// The size the band is booted at.
const BAND_SIZE: (usize, usize) = (1280, 1024);

/// The hand-over's directory, or `None` with the skip line.
fn hand_over() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(BAND_2001);
    if !dir.join(DISK).exists() || !dir.join(SOURCES).exists() {
        eprintln!(
            "skipped: {} is not present; tools/fetch-handover-2001.sh fetches it",
            dir.join(DISK).display()
        );
        return None;
    }
    Some(dir)
}

/// A copy of the hand-over's disk, which the machine writes, and a file
/// root in a scratch directory named `name`: the tree unpacked beside it,
/// and `root/` with its `sys` and `site` and an empty `lispm` and
/// `home/lispm`. `None` with the skip line without the hand-over.
fn band_2001(name: &str) -> Option<(support::Scratch, PathBuf, PathBuf)> {
    let from = hand_over()?;
    let dir = support::scratch(name);
    let pack = dir.join("pack.vhd");
    std::fs::copy(from.join(DISK), &pack).unwrap();
    let untar = std::process::Command::new("tar")
        .arg("xzf")
        .arg(from.join(SOURCES))
        .arg("-C")
        .arg(dir.path())
        .status()
        .unwrap();
    assert!(untar.success(), "the tree unpacks");
    let root = dir.join("root");
    std::fs::create_dir_all(root.join("lispm")).unwrap();
    std::fs::create_dir_all(root.join("home/lispm")).unwrap();
    for part in ["sys", "site"] {
        std::os::unix::fs::symlink(dir.join(TREE).join(part), root.join(part)).unwrap();
    }
    Some((dir, pack, root))
}

/// Revision 13 with `boards` of main memory and PROM 2001, the disk on
/// block-disk, the video controller at `w` by `h`, and the file device
/// serving `root` as HOST's `/` and its `sys` and `site` as `/sys` and
/// `/site`, where the band's `SYS:` is.
fn quux_13(pack: &Path, root: &Path, boards: usize, (w, h): (usize, usize)) -> Machine {
    use muir::block_disk::{BLOCK_NS, BlockDisk};
    let mut m = Machine::with_geometry(Geometry::QUUX_13, boards);
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
    let boards = Geometry::QUUX_13.default_memory_boards();
    assert_eq!(boards << 16, 32 << 20, "32MW");
    boots_on_both_engines(boards);
}

/// The hand-over's microcode's symbols, its `ucadr.sym`.
fn symbols() -> muir::sym::Symbols {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(BAND_2001).join("ucadr.sym");
    muir::sym::parse(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The address of `name` in `space` of the hand-over's microcode.
fn ucadr(name: &str, space: muir::sym::Space) -> u16 {
    symbols().address(space, name).unwrap_or_else(|| panic!("{name} in ucadr.sym")) as u16
}

/// **The band is System 2001 on microcode 2001**, as the disk says: its
/// current `MCR1` is named "MCR1 UCADR 2001" and holds the hand-over's
/// `ucadr.mcr`, whose `A-VERSION` is 2001, with zeros after it, and its
/// current `LOD1` is named "LOD1 System 2001", the only current band. The
/// microcode's A memory is the 40-bit section and its dispatch memory 4,096
/// entries (contract G2 appendix A1.12, A1.4). Its `PAGE` partition is
/// 655,360 blocks, 128MW of virtual memory at 5 blocks a 1024-word
/// page, packed storage's 5 bytes a word. No boot.
#[test]
fn band_2001_is_system_2001_on_microcode_2001() {
    let Some((_dir, pack, _root)) = band_2001("system-2001-names") else { return };
    let bytes = std::fs::read(hand_over().unwrap().join("ucadr.mcr")).unwrap();
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
/// disk**, as PROM 2000 does on revision 12 (`tests/system_2000_timers.rs`,
/// M9), at revision 13's register page: from power-on, its writes show word
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

/// **System 2001 boots with the fused return** (contract H8a §6 items 4-6,
/// as `tests/system_2000.rs` holds System 2000 to them), on `rtl`: microcode
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

/// `ERROR-BAD-ADDRESS`, where PROM 2000 and PROM 2001 stop on a section
/// address out of range: 36024 in both `promh.sym`s.
const ERROR_BAD_ADDRESS: u16 = 0o36024;

/// **Revision 12's disk on revision 13 stops at PROM 2001's named halt**
/// (contract G2 §2.8): System 2000's release disk, whose microcode 2000
/// carries A memory as section 4, booted on revision 13 with PROM 2001
/// stops at `ERROR-A-MEM-SECTION-32-BITS`, on both engines, before any
/// microcode runs. The control: the same PROM with its jump to that halt
/// taken out does not stop there, so the test fails without the guard.
#[test]
fn revision_12_s_disk_on_revision_13_stops_at_prom_2001_s_halt() {
    let Some((_dir, pack, root)) = support::quux_release_band("system-2001-guard-12-on-13") else {
        return;
    };
    let m = quux(&pack, &root);
    both_stop_at(&m, ERROR_A_MEM_SECTION_32_BITS, 50_000_000);
    let mut control = m;
    control
        .load_prom(&without_jumps_to(&muir::prom::quux_boot_prom(), ERROR_A_MEM_SECTION_32_BITS));
    let r = stops_at(Micro::new, control, ERROR_A_MEM_SECTION_32_BITS, 50_000_000);
    eprintln!("control: {:?}", r.map_err(|pc| format!("at {pc:o}")));
    assert!(r.is_err(), "the control stopped there too");
}

/// **System 2001's disk on revision 12 stops at PROM 2000's
/// `ERROR-BAD-ADDRESS`** (contract G2 §2.8): PROM 2000 loads microcode
/// 2001's dispatch memory section, of 4,096 entries, until the entry at
/// 2,048, whose address has a bit in `<20:11>`, and halts there
/// (`PROCESS-D-MEM-SECTION`, the PROM's `promh.text:682-685`), before the
/// A-memory section, on both engines. The control: PROM 2000 with its
/// jumps to that halt taken out does not stop there, so the test fails
/// without the guard.
#[test]
fn system_2001_s_disk_on_revision_12_stops_at_error_bad_address() {
    use muir::block_disk::{BLOCK_NS, BlockDisk};
    let Some((_dir, pack, _root)) = band_2001("system-2001-guard-13-on-12") else { return };
    let mut m = Machine::with_geometry(Geometry::QUUX, 32);
    m.load_prom(&muir::prom::quux_12_boot_prom());
    let mut d = BlockDisk::new(BLOCK_NS);
    d.attach(muir::disk_image::Disk::open_rw(&pack).unwrap());
    m.block_disk = Some(d);
    m.tv.set_board(Board::Video);
    both_stop_at(&m, ERROR_BAD_ADDRESS, 50_000_000);
    let mut control = m;
    control.load_prom(&without_jumps_to(&muir::prom::quux_12_boot_prom(), ERROR_BAD_ADDRESS));
    let r = stops_at(Micro::new, control, ERROR_BAD_ADDRESS, 50_000_000);
    eprintln!("control: {:?}", r.map_err(|pc| format!("at {pc:o}")));
    assert!(r.is_err(), "the control stopped there too");
}
