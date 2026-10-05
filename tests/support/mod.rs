// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared by the test binaries, each taking what it needs: where the tests
//! find MIT's files and the fetched release, the checks every board's
//! netlist gets, MIT's own census and wire list read back, the I/O board's
//! inputs at rest, the **Chaosnet server** the boots are given, and the
//! boot to the listener.
//!
//! The Chaosnet server is here rather than in `src/` because a CADR has no
//! file or time server inside it: `muir` reaches a host on the network
//! over CHUDP, and `cargo test` cannot want a daemon running beside it.
//! [`server`], [`mod@file`], [`mod@time`] and [`status`] are that server,
//! and [`ChaosServer`] is how a test puts one on a machine's cable.

#![allow(dead_code)]

pub mod file;
pub mod macro_dispatch;
pub mod profile;
pub mod server;
pub mod status;
pub mod time;
pub mod unused_codes;

/// `<31:0>` of a word of a 32-bit machine, which has nothing above bit 31:
/// a word with more is a fault to report, not a value to cut.
pub fn low(w: muir::machine::Word) -> u32 {
    u32::try_from(w).expect("a 32-bit machine's word with bits above 31")
}

/// QUUX's virtual page `vpage`, below 32 and so in level-1 block 0, mapped
/// readable and writable to the 1024-word frame that holds physical word
/// `phys` (contract G2 §2.6, appendix A1.7): the frame in its level-2
/// entry, `<17:0>`, and the access bits `<27:26>`. Returns the virtual
/// address of `phys`.
pub fn quux_map(m: &mut muir::machine::Machine, vpage: u32, phys: u32) -> u32 {
    m.l2_map[vpage as usize] = 1 << 27 | 1 << 26 | phys >> 10;
    vpage << 10 | phys & 0o1777
}

pub use server::ChaosServer;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use muir::engine::Engine;
use muir::netlist::Netlist;
use muir::part::{self, Drive, Level};
use muir::unibus::{IDLE_CHAOSNET, IDLE_KEYBOARD};
use muir::wirelist;

/// A file under `mit/`. Everything there is committed and always present,
/// so a missing one is a broken checkout and fails; it is never a skip.
pub fn mit(parts: &[&str]) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("mit");
    p.extend(parts);
    assert!(p.exists(), "{} is committed with the repository and should be there", p.display());
    p
}

/// A file under `mit/` as text. MIT wrote ASCII; a byte the tapes carry
/// outside it is read lossily rather than refused.
pub fn mit_text(parts: &[&str]) -> String {
    String::from_utf8_lossy(&std::fs::read(mit(parts)).unwrap()).into_owned()
}

/// The pack the machine tests boot: **System 100**, what this project
/// targets, put there by `tools/fetch-system-100-for-cadr.sh`. It is also the
/// fixture the label and band tests are written against --- their
/// partition table, pack name and band comments are its.
pub fn pack_100() -> Option<PathBuf> {
    vendor(&["run", "disk-sys-100-0.img"])
}

/// The Chaosnet numbers the **System 100** band holds: this machine
/// `MIT-LISPM-1` at 3050, and `MIT-OZ`, its file and time host, at 3060.
/// Read off the release's own `sys/site/hosts.text`, and enforced against
/// it by `tests/chaos.rs::the_bands_own_hosts_are_named_and_are_not_the_defaults`.
///
/// A run has to say them: [`muir::chaos::Config`] defaults to 177001, on
/// the private subnet 376, which is no band's address on purpose, and it
/// carries no server's address at all. A band reached at the wrong pair
/// gets neither the time nor its files and stops in the cold-load debugger
/// to ask for the date, so every test that boots this band over the
/// Chaosnet passes these.
pub const CHAOS_100: (u16, u16) = (0o3050, 0o3060);

/// The directory the Chaosnet server's FILE service serves as its `/`,
/// `vendor/run/file-root`, or `None` with the skip line.
///
/// A directory of its own rather than `vendor/` or a release, because the
/// service writes, renames and deletes under its root and fetched material
/// should not be in reach of a running machine by accident. The band asks
/// its file host under a name of its own --- System 100's translates
/// `SYS: SYS2; FOO LISP` to `//TREE//SYS2//FOO LISP` --- so
///
/// ```text
/// mkdir -p vendor/run/file-root
/// ln -s ../../system-100-0/sys vendor/run/file-root/tree
/// ```
///
/// makes `SYS:` resolve, and the release's fetch script makes that link.
pub fn file_root() -> Option<PathBuf> {
    vendor(&["run", "file-root"])
}

/// QUUX's release, the directory its sources unpack to.
pub const QUUX_RELEASE: &str = "release-2001";

/// A file of QUUX's release, under `vendor/system-2001/` where
/// `tools/fetch-system-for-quux.sh` puts the release's files and unpacks
/// its sources, or `None` with the skip line.
pub fn quux_release(parts: &[&str]) -> Option<PathBuf> {
    let mut p = vec!["system-2001"];
    p.extend(parts);
    let found = vendor(&p);
    if found.is_none() {
        eprintln!("skipped: tools/fetch-system-for-quux.sh fetches it");
    }
    found
}

/// QUUX's release in a scratch directory, or `None` with the skip line:
/// its disk decompressed from the release's own `release-2001-disk.vhd.gz`
/// to `pack.vhd`, which the machine writes; its sources unpacked from
/// `release-2001-sys.tar.gz` beside it, to `release-2001/`; and a file root,
/// `root/`, with the sources' `sys` and `site` and an empty `lispm` and
/// `home/lispm`. The disk is not `vendor/run/release-2001-disk.vhd`, the
/// one the fetch script leaves for running `quux` by hand, which a machine
/// has written to once it has run.
pub fn quux_release_band(name: &str) -> Option<(Scratch, PathBuf, PathBuf)> {
    let disk = quux_release(&[&format!("{QUUX_RELEASE}-disk.vhd.gz")])?;
    let sources = quux_release(&[&format!("{QUUX_RELEASE}-sys.tar.gz")])?;
    let dir = scratch(name);
    let pack = dir.join("pack.vhd");
    let out = std::process::Command::new("gzip").arg("-dc").arg(&disk).output().expect("gzip");
    assert!(out.status.success(), "{} decompresses", disk.display());
    std::fs::write(&pack, out.stdout).unwrap();
    let untar = std::process::Command::new("tar")
        .arg("xzf")
        .arg(&sources)
        .arg("-C")
        .arg(dir.path())
        .status()
        .unwrap();
    assert!(untar.success(), "{} unpacks", sources.display());
    let root = dir.join("root");
    std::fs::create_dir_all(root.join("lispm")).unwrap();
    std::fs::create_dir_all(root.join("home/lispm")).unwrap();
    for part in ["sys", "site"] {
        std::os::unix::fs::symlink(dir.join(QUUX_RELEASE).join(part), root.join(part)).unwrap();
    }
    Some((dir, pack, root))
}

/// A file under `vendor/`, where the fetch script puts the release, or
/// `None` with a line saying so: a test that needs it skips without it,
/// and says that it did.
pub fn vendor(parts: &[&str]) -> Option<PathBuf> {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vendor");
    p.extend(parts);
    if p.exists() {
        Some(p)
    } else {
        eprintln!("skipped: {} is not present", p.display());
        None
    }
}

/// A source file of the System 100 release, `vendor/system-100-0/sys/<file>`,
/// as text, or `None` with the skip line.
///
/// **The name says which release**, because a fact read out of one
/// release's sources is not a fact about another's: `window/shwarm.lisp`,
/// for one, carries the display geometry as `DEFCONST`s here and does not
/// in LM-3's later System 304.
pub fn release(file: &str) -> Option<String> {
    let p = vendor(&["system-100-0", "sys", file])?;
    Some(String::from_utf8_lossy(&std::fs::read(p).unwrap()).into_owned())
}

/// A file of the System 100 release, `vendor/system-100-0/sys/<parts>`, as
/// a path: for `SYS: UBIN;`, where the readers want bytes rather than text.
pub fn release_100_file(parts: &[&str]) -> Option<PathBuf> {
    let mut p = vec!["system-100-0", "sys"];
    p.extend_from_slice(parts);
    vendor(&p)
}

/// A supply, a pull-up, an unconnected pin or a spare: a net no signal is
/// on, which the driver checks leave out.
pub fn is_power(name: &str) -> bool {
    // Trimmed once, and used for every test: a name MIT wrote with a space
    // in it reaches the netlist quoted, so `'HI 1-14'` starts with an
    // apostrophe and not with `HI`. Matching the untrimmed name let that
    // one through as an ordinary wire --- the same shape of miss as
    // `- 11CLRTDN`, where a character nobody looks at decided the answer.
    let name = name.trim_matches('\'');
    matches!(name, "GND" | "VCC" | "NC" | "+5" | "-5")
        || name.starts_with("HI")
        || name.starts_with("NC#")
        || name.starts_with("PULLUP")
}

/// **Two totem-pole outputs on one net is an electrical fault**, so a
/// wrongly claimed output pin in `src/part.rs` surfaces here. It is the
/// check that caught a `74S472` entry claiming an output on its ground pin.
pub fn no_net_has_two_push_pull_drivers(n: &Netlist) {
    let mut drivers: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for p in &n.parts {
        let Some(po) = part::pinout(&p.kind) else { continue };
        for &(pin, net) in &p.pins {
            if po.drive_of(pin) == Some(Drive::Totem) && !is_power(n.net(net)) {
                drivers
                    .entry(net)
                    .or_default()
                    .push(format!("{} {} ({}) p{pin}", p.page, p.reference, p.kind));
            }
        }
    }
    let conflicts: Vec<_> = drivers.iter().filter(|(_, v)| v.len() > 1).collect();
    for (net, who) in conflicts.iter().take(12) {
        eprintln!("net {} driven by {}: {:?}", n.net(**net), who.len(), who);
    }
    eprintln!(
        "{} nets driven by a totem-pole output; {} conflicts",
        drivers.len(),
        conflicts.len()
    );
    assert!(conflicts.is_empty(), "{} nets have two push-pull drivers", conflicts.len());
}

/// One write of the display board's color register, `lmtv.order`'s
/// `173777x4`, watched all the way through: which of `-LOAD COLOR 0`, `1`
/// and `2` went low during the cycle, and what `COLOR 0..7` and
/// `COLOR VALUE 0..7` read while one was.
///
/// `strap` is which board this is, `x` being 6 on the normal TV and 5 on
/// the color TV: [`muir::xbus::straps`]. The register is the same circuit
/// at either address.
///
/// Both display boards have the page --- `COLOR` on the LISPM TV and
/// `NRACOL`, titled "COLOR MAP", on the SIMPLE TV --- so both are measured
/// with this, and `tests/tv.rs` holds the model to what they do.
///
/// It is [`muir::xbus::XbusMaster::cycle`] taken apart, because the strobe
/// is one 16 MHz period wide and is gone by the time the cycle ends: the
/// bus is watched every 5 ns from the request to the end of the settle, as
/// `cycle` itself watches for the acknowledgement.
pub fn color_write(
    b: &mut muir::xbus::XbusMaster,
    strap: muir::tv::Strap,
    word: u32,
) -> (Vec<usize>, Option<(u64, u64)>) {
    use muir::xbus::XbusMaster;

    let strobes: Vec<u32> = (0..3).map(|k| b.net(&format!("-LOAD COLOR {k}"))).collect();
    let color: Vec<u32> = (0..8).map(|k| b.net(&format!("COLOR {k}"))).collect();
    let value: Vec<u32> = (0..8).map(|k| b.net(&format!("COLOR VALUE {k}"))).collect();

    let (mut low, mut sampled) = (Vec::new(), None);
    let watch = |b: &XbusMaster, low: &mut Vec<usize>, sampled: &mut Option<(u64, u64)>| {
        for (k, &s) in strobes.iter().enumerate() {
            if b.chip.net(s) == Level::Low {
                if !low.contains(&k) {
                    low.push(k);
                }
                *sampled = Some((b.chip.read(&color), b.chip.read(&value)));
            }
        }
    };

    let t0 = b.now;
    b.request(strap.control + 4, Some(word));
    while !b.acked() {
        assert!(b.now < t0 + 40_000, "the board never acknowledged");
        b.run(b.now + 5);
        watch(b, &mut low, &mut sampled);
    }
    let until = b.now + XbusMaster::RELEASE_NS;
    while b.now < until {
        b.run(b.now + 5);
        watch(b, &mut low, &mut sampled);
    }
    b.release();
    let until = b.now + 600;
    while b.now < until {
        b.run(b.now + 5);
        watch(b, &mut low, &mut sampled);
    }
    (low, sampled)
}

/// What `src/part.rs` makes of the kinds a board uses, each kind once and
/// sorted: the kinds with no pinout, the kinds with a pinout and no
/// behavior, and the kinds whose behavior computes nothing --- no gate
/// and no update.
pub struct Kinds {
    pub unknown: Vec<String>,
    pub silent: Vec<String>,
    pub empty: Vec<String>,
}

pub fn kinds(n: &Netlist) -> Kinds {
    let (mut unknown, mut silent, mut empty) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for p in &n.parts {
        if part::pinout(&p.kind).is_none() {
            unknown.insert(p.kind.clone());
        } else if let Some(b) = part::behavior(&p.kind) {
            if b.gates.is_empty() && b.update.is_none() {
                empty.insert(p.kind.clone());
            }
        } else {
            silent.insert(p.kind.clone());
        }
    }
    Kinds {
        unknown: unknown.into_iter().collect(),
        silent: silent.into_iter().collect(),
        empty: empty.into_iter().collect(),
    }
}

/// The control-store board's pages, by MIT's own list of them:
/// `cadr/framl.txt`, the frame list of `icmem.book`. `CADR.netlist` holds
/// two boards and they share designators --- `1A01` is a 74S240 on the
/// processor's VMEMDR and a 74S174 on the control store's OLORD1 --- so
/// counting locations means counting them a board at a time. The list
/// names the control-store RAM pages `RAM00`..`RAM33` where the drawings
/// are `iram00.drw`, and the netlist took the file names.
pub fn control_store_pages() -> BTreeSet<String> {
    let text = std::fs::read_to_string(mit(&["cadr", "framl.txt"])).unwrap();
    text.split_whitespace()
        .map(|p| if p.starts_with("RAM") { format!("I{p}") } else { p.to_string() })
        .collect()
}

/// **A body in MIT's stuffing list.** One row of a `*.stf`, which is a
/// board as the stockroom saw it: six columns --- part number, DIP type,
/// `CARD LOC`, body, file and position --- one row per body, and a further
/// row carrying only body, file and position for each of that body's gates
/// drawn elsewhere.
pub struct Body {
    /// The board location as MIT prints it: `B05`, or `1A01` on the
    /// processor pair, whose designators carry a leading digit. A netlist
    /// writes the one-section boards' designators with a leading `0` that
    /// MIT's zero-suppressed column drops.
    pub location: String,
    /// **What tells two bodies at one location apart.** MIT writes the
    /// second body at a location `B05@03` where the first is `B05`, and
    /// this is that number, 0 for a body written without one.
    ///
    /// What the number itself means is **unverified**: no MIT file found so
    /// far defines it, and nothing here needs it to. The SUDS documentation
    /// of the wire-list format would settle it. All that is used is
    /// that bodies sharing a location carry different ones, which
    /// `parts_mounted.rs` asserts over every body in every list it reads.
    /// It is not read as a pin offset or a device number, and must not be
    /// until something says so.
    pub at: u16,
    /// One entry per gate, in the order the file lists them: the `BODY`
    /// column and the page it is drawn on. A quad NAND has four, and a
    /// chip MIT drew across two sheets names both pages.
    ///
    /// **The gates of one body need not share a body name.** `dm.stf`'s
    /// 74LS02 at D13 is `LS02L` three times on DMSEQ and `OLS02L` once on
    /// DMSEL, one chip drawn under the name of the gate.
    pub gates: Vec<(String, String)>,
}

impl Body {
    /// The `BODY` column of the row that placed it, which is what a netlist
    /// carries as a part's `kind`: `OLS14L`, `74LS569`, `CAP1`.
    pub fn kind(&self) -> &str {
        &self.gates[0].0
    }

    /// The pages the body is drawn on, each once.
    pub fn pages(&self) -> BTreeSet<&str> {
        self.gates.iter().map(|(_, page)| page.as_str()).collect()
    }
}

/// MIT's stuffing list for a board, `mit/<parts>`, in the order it prints.
///
/// The row shape is exact, so this asserts it rather than passing over what
/// it cannot read: from the location on, a row carries four fields, and a
/// row with no location carries three. That holds for every row of the
/// eight lists the tests read.
pub fn stuffing_list(parts: &[&str]) -> Vec<Body> {
    let text = mit_text(parts);
    let mut out: Vec<Body> = Vec::new();
    let mut in_table = false;
    // Some of these lists are stored with CR line endings and some with LF.
    for line in text.split(['\n', '\r']) {
        // A form feed starts a printed page, which repeats the title and
        // the column headings; the table resumes at the next heading.
        if line.contains('\u{c}') {
            in_table = false;
        }
        if line.contains("PART NUMBER") && line.contains("DIPTYPE") {
            in_table = true;
            continue;
        }
        if !in_table {
            continue;
        }
        let f: Vec<&str> = line.split('\t').map(str::trim).filter(|c| !c.is_empty()).collect();
        if f.is_empty() {
            continue;
        }
        match f.iter().position(|c| board_location(c).is_some()) {
            Some(k) => {
                let (location, at) = board_location(f[k]).unwrap();
                assert_eq!(f.len() - k, 4, "{parts:?}: {line:?} is not a body row");
                let gates = vec![(f[k + 1].to_string(), f[k + 2].to_string())];
                out.push(Body { location, at, gates });
            }
            None => {
                assert_eq!(f.len(), 3, "{parts:?}: {line:?} is not a gate row");
                let body = out.last_mut().expect("a gate row before the body it continues");
                body.gates.push((f[0].to_string(), f[1].to_string()));
            }
        }
    }
    out
}

/// A `CARD LOC` entry: the board location, and the number that tells two
/// bodies at one location apart. `B05( )` is `("B05", 0)` and `B05@03( )`
/// is `("B05", 3)`. The parentheses hold the variable settings the column
/// heading names, which are empty on every board here.
fn board_location(field: &str) -> Option<(String, u16)> {
    let (name, settings) = field.split_once('(')?;
    if !settings.ends_with(')') {
        return None;
    }
    let (name, at) = match name.split_once('@') {
        Some((name, at)) => (name, at.parse().ok()?),
        None => (name, 0),
    };
    // A letter and two digits, `B05`, after an optional section digit,
    // which the processor pair's designators carry and no other board's do.
    let b = name.as_bytes();
    let row = b.len().checked_sub(3)?;
    (row <= 1
        && b[row].is_ascii_uppercase()
        && b[row + 1..].iter().all(u8::is_ascii_digit)
        && b[..row].iter().all(u8::is_ascii_digit))
    .then(|| (name.to_string(), at))
}

/// MIT's census of a board by body, a `.wls` file: under "DIPTYPE  BODY
/// NAME  # SECTION  TOTAL DIPS  #SPARE SECTIONS" a type starts at column 0
/// and its further bodies are indented, and the count taken is each body's
/// `# SECTION` --- how many of that body the drawings place, which is what
/// a netlist's parts count too. `dip_census` below takes the packages
/// instead. The file's own header lines and its closing note are passed
/// over.
pub fn body_census(text: &str) -> BTreeMap<String, usize> {
    let mut census: BTreeMap<String, usize> = BTreeMap::new();
    let mut started = false;
    for line in text.lines() {
        if line.starts_with("DIPTYPE\tBODY NAME") {
            started = true;
            continue;
        }
        if !started || line.starts_with("NUMBER IN PARENS") {
            if started {
                break;
            }
            continue;
        }
        if line.starts_with("LISP") || line.starts_with("FILNAM") {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').filter(|f| !f.trim().is_empty()).collect();
        if fields.is_empty() || fields[0].trim() == "WITH LOCATIONS" {
            continue;
        }
        let (body, count) = if line.starts_with('\t') {
            if fields.len() < 2 {
                continue;
            }
            (fields[0].trim(), fields[1].trim())
        } else {
            if fields.len() < 3 {
                continue;
            }
            (fields[1].trim(), fields[2].trim())
        };
        let Ok(k) = count.parse::<usize>() else { continue };
        *census.entry(body.to_string()).or_default() += k;
    }
    census
}

/// MIT's wire list for a board, `mit/<parts>`, read against the netlist's
/// pages.
pub fn wire_list(n: &Netlist, parts: &[&str]) -> Vec<wirelist::Signal> {
    wirelist::parse(&mit_text(parts), &n.pages)
}

/// What a comparison against the wire list found, printed: how much of the
/// list landed on the board, the wires split over nets, and the nets that
/// hold several wires.
pub fn report(signals: &[wirelist::Signal], r: &wirelist::Report) {
    eprintln!(
        "{} signals, {} pins placed, {} on parts the netlist has not got",
        signals.len(),
        r.placed,
        r.unplaced
    );
    for (names, nets) in &r.split {
        eprintln!("  wire {names:?} is nets {nets:?}");
    }
    for (net, names) in &r.merged {
        eprintln!("  net {net} is wires {names:?}");
    }
}

/// The I/O board's inputs from off the Unibus as the far end holds them at
/// rest: an idle Chaosnet cable and a keyboard with nothing typed.
pub fn quiet() -> Vec<(&'static str, Level)> {
    IDLE_CHAOSNET.iter().chain(IDLE_KEYBOARD).copied().collect()
}

/// Boots a machine over the Chaosnet server at `chaos` --- this machine's
/// address and the server's --- serving `file_root`, to the listener: the
/// boot is at its prompt once the rows the `;Reading` line is printed in
/// hold more than 400 lit pixels, and a moment more for the prompt to
/// settle. Returns the microcycles it took.
///
/// The pair is the caller's because it is the band's and not muir's:
/// [`CHAOS_100`] for the System 100 pack, and a band reached at any other
/// pair --- the default address included --- boots but stops to ask for
/// the date and reaches no files. The server is
/// the harness's, [`ChaosServer`]: `muir` has none, and a run of it names
/// an external host with `--chaos-udp-peer` instead.
///
/// Any engine: the Chaosnet is the machine's and not the engine's, and
/// every engine keeps the machine's clock, which is what the interface
/// runs on.
pub fn boot_to_the_prompt<E: Engine>(e: &mut E, chaos: (u16, u16), file_root: PathBuf) -> u64 {
    boot_to_the_prompt_within(e, chaos, file_root, 100_000_000)
}

/// [`boot_to_the_prompt`], giving up after `limit` microcycles rather than
/// a hundred million: a band that loads its error table as it boots, which
/// one on a microcode it was not saved with does, takes longer.
pub fn boot_to_the_prompt_within<E: Engine>(
    e: &mut E,
    chaos: (u16, u16),
    file_root: PathBuf,
    limit: u64,
) -> u64 {
    let m = e.machine_mut();
    m.chaos.address = chaos.0;
    ChaosServer::new(chaos.1).serving(file_root).at_time(time::TEST_UNIVERSAL).plug(m, 0);
    wait_for_the_prompt_within(e, limit)
}

/// The wait itself, for a machine whose Chaosnet is already plugged: a
/// test that wants the ether's log on has to turn it on after the plug,
/// so it does that and comes here.
pub fn wait_for_the_prompt<E: Engine>(e: &mut E) -> u64 {
    wait_for_the_prompt_within(e, 100_000_000)
}

/// [`wait_for_the_prompt`], giving up after `limit` microcycles.
pub fn wait_for_the_prompt_within<E: Engine>(e: &mut E, limit: u64) -> u64 {
    // The `;Reading at top level` line, which is the prompt appearing.
    // Where it lands depends on how tall the herald above it is --- five
    // lines on System 100's band --- so the rows watched are a band the
    // line falls in rather than one row of it, and the herald itself is
    // above all of them.
    let reading = |e: &E| {
        let tv = &e.machine().tv;
        (84..130usize)
            .flat_map(|y| (0..muir::tv::WIDTH).map(move |x| (x, y)))
            .filter(|&(x, y)| tv.pixel(x, y))
            .count()
            > 400
    };
    let mut ran = 0u64;
    while !reading(e) {
        for _ in 0..500_000 {
            e.step().expect("the boot halted");
        }
        ran += 500_000;
        assert!(ran < limit, "the listener never began reading");
    }
    // And a moment for the prompt to settle.
    for _ in 0..2_000_000 {
        e.step().expect("the boot halted");
    }
    ran + 2_000_000
}

/// A frame of a GIF as the two-color recordings are read back: the
/// rectangle and a byte a pixel.  What the recorder writes is read back
/// here rather than trusted, by the recorder's own tests and by the CC
/// diagnostics'.
pub type Frame = ((usize, usize, usize, usize), Vec<u8>);

pub fn decode_gif(g: &[u8]) -> Vec<Frame> {
    decode_gif_fully(g).1.into_iter().map(|f| (f.rect, f.pixels)).collect()
}

/// One frame of a GIF, read back whole: where it lands on the canvas, the
/// color table in force for it, whether that table was the frame's own,
/// and a byte a pixel.
pub struct GifFrame {
    pub rect: (usize, usize, usize, usize),
    /// The frame's local color table if it carries one, the file's global
    /// table otherwise --- which is what a viewer resolves its pixels
    /// through either way.
    pub colors: Vec<[u8; 3]>,
    /// Whether that table was the frame's own.
    pub local: bool,
    pub pixels: Vec<u8>,
}

/// A GIF read back with its color tables: the canvas size, and every
/// frame with the table a viewer shows it through.  The color recording
/// is checked with this --- its map is the file's colors, and a frame
/// taken through a changed map carries a table of its own --- and
/// [`decode_gif`] is this without the colors.
pub fn decode_gif_fully(g: &[u8]) -> ((usize, usize), Vec<GifFrame>) {
    assert_eq!(&g[..6], b"GIF89a");
    let u = |k: usize| u16::from_le_bytes([g[k], g[k + 1]]) as usize;
    let canvas = (u(6), u(8));
    // The packed field of a descriptor: bit 7 says a color table follows
    // and bits 2-0 give its size, 2^(n+1) entries of three bytes.
    let table = |at: usize, packed: u8| -> (Vec<[u8; 3]>, usize) {
        if packed & 0x80 == 0 {
            return (Vec::new(), at);
        }
        let n = 1 << ((packed & 7) + 1);
        let entries = g[at..at + 3 * n].chunks(3).map(|c| [c[0], c[1], c[2]]).collect();
        (entries, at + 3 * n)
    };
    let (global, mut i) = table(13, g[10]);
    let mut frames = Vec::new();
    while i < g.len() {
        match g[i] {
            0x21 => {
                i += 2;
                while g[i] != 0 {
                    i += g[i] as usize + 1;
                }
                i += 1;
            }
            0x2c => {
                let (l, t, w, h) = (u(i + 1), u(i + 3), u(i + 5), u(i + 7));
                let packed = g[i + 9];
                let (local, after) = table(i + 10, packed);
                i = after;
                let min = g[i] as u32;
                i += 1;
                let mut data = Vec::new();
                while g[i] != 0 {
                    let n = g[i] as usize;
                    data.extend_from_slice(&g[i + 1..i + 1 + n]);
                    i += n + 1;
                }
                i += 1;
                frames.push(GifFrame {
                    rect: (l, t, w, h),
                    local: !local.is_empty(),
                    colors: if local.is_empty() { global.clone() } else { local },
                    pixels: lzw_decode(&data, min, w * h),
                });
            }
            0x3b => break,
            b => panic!("unexpected block {b:#x} at {i}"),
        }
    }
    (canvas, frames)
}

fn lzw_decode(data: &[u8], min: u32, n: usize) -> Vec<u8> {
    let clear = 1u32 << min;
    let end = clear + 1;
    let mut table: Vec<Vec<u8>> = Vec::new();
    let reset = |table: &mut Vec<Vec<u8>>| {
        table.clear();
        for k in 0..clear {
            table.push(vec![k as u8]);
        }
        table.push(Vec::new());
        table.push(Vec::new());
    };
    reset(&mut table);
    let mut width = min + 1;
    let (mut acc, mut nbits, mut pos) = (0u32, 0u32, 0usize);
    let mut out = Vec::new();
    let mut prev: Option<Vec<u8>> = None;
    loop {
        while nbits < width && pos < data.len() {
            acc |= (data[pos] as u32) << nbits;
            nbits += 8;
            pos += 1;
        }
        if nbits < width {
            break;
        }
        let code = acc & ((1 << width) - 1);
        acc >>= width;
        nbits -= width;
        if code == clear {
            reset(&mut table);
            width = min + 1;
            prev = None;
            continue;
        }
        if code == end {
            break;
        }
        let entry = if (code as usize) < table.len() {
            table[code as usize].clone()
        } else {
            let p = prev.as_ref().expect("a code past the table with nothing before it");
            let mut e = p.clone();
            e.push(p[0]);
            e
        };
        out.extend_from_slice(&entry);
        if let Some(p) = prev {
            let mut e = p;
            e.push(entry[0]);
            table.push(e);
            if table.len() == (1 << width) && width < 12 {
                width += 1;
            }
        }
        prev = Some(entry);
    }
    assert_eq!(out.len(), n, "pixels decoded");
    out
}

/// An MCR file holding these control store words and nothing else: the
/// control store section, then the empty A-memory section that ends a
/// file, in the layout `muir::mcr` reads. Written out here so that a
/// boot PROM this repository has no file of --- one word too long, one
/// carrying a bit the chips cannot hold, one that jumps somewhere else
/// --- can still be handed to the reader, or to `cadr --prom`.
pub fn mcr(words: &[u64]) -> Vec<u8> {
    /// 32 bits in PDP-11 word order, as `mcr`'s reader takes them.
    fn u32_pdp(b: &mut Vec<u8>, v: u32) {
        b.extend_from_slice(&[(v >> 16) as u8, (v >> 24) as u8, v as u8, (v >> 8) as u8]);
    }
    let mut b = Vec::new();
    // Section 1, the control store, from address 0.
    u32_pdp(&mut b, 1);
    u32_pdp(&mut b, 0);
    u32_pdp(&mut b, words.len() as u32);
    for w in words {
        // Four 16-bit little-endian halves, most significant first; the
        // top one is zero, a microinstruction being 48 bits.
        for shift in [48, 32, 16, 0] {
            b.push((w >> shift) as u8);
            b.push((w >> (shift + 8)) as u8);
        }
    }
    // Section 4, A memory, empty: it is the one that ends the file.
    u32_pdp(&mut b, 4);
    u32_pdp(&mut b, 0);
    u32_pdp(&mut b, 0);
    b
}

// --- The boards as netlists, a machine with the pack, and typing at it -----

use std::path::Path;

use muir::cable::FarEnd;
use muir::chip::Chip;
use muir::clock::Behavioral;
use muir::disk_unit::{Geometry, Unit};
use muir::machine::Machine;
use muir::terminal::keyboard::{Keyboard, keysym};

/// The I/O board, `data/CADRIO.netlist`, as the model has it:
/// `netlist::parse`, which joins the two ends of every series resistor into
/// one net. Every pin the board was moved on against its drawings is
/// already in the file, so `parse` and `parse_wired` differ in the
/// resistors alone.
pub fn cadrio() -> Netlist {
    muir::netlist::parse(include_str!("../../data/CADRIO.netlist")).unwrap()
}

/// The five boards `cadr --chip` boots, each parsed from `data/`: the
/// processor, the bus interface, memory, the I/O board and the display.
pub struct Netlists {
    pub cpu: Netlist,
    pub busint: Netlist,
    pub cadrm: Netlist,
    pub cadrio: Netlist,
    pub simple_tv: Netlist,
}

pub fn netlists() -> Netlists {
    let parse = |s: &str| muir::netlist::parse(s).unwrap();
    Netlists {
        cpu: parse(include_str!("../../data/CADR.netlist")),
        busint: parse(include_str!("../../data/BUSINT.netlist")),
        cadrm: parse(include_str!("../../data/CADRM.netlist")),
        cadrio: cadrio(),
        simple_tv: parse(include_str!("../../data/SIMPLETV.netlist")),
    }
}

/// The board as `cadr --chip` runs it, `benchmark::chip` over
/// [`netlists`]: the processor chip, its clock and the far end of its
/// cables.
pub fn chip(n: &Netlists) -> (Chip, Behavioral, FarEnd) {
    muir::benchmark::chip(&n.cpu, &n.busint, &n.cadrm, &n.cadrio, &n.simple_tv)
}

/// A machine with the boot PROM in place and the System 100 pack, `pack`,
/// as a T-300 on unit 0: what every boot off the pack starts from, on
/// whichever engine.
pub fn machine_with_pack(pack: &Path) -> Machine {
    machine_with_pack_at(pack, muir::machine::MAIN_WORDS >> 16)
}

/// [`machine_with_pack`] with `boards` 64K-word boards of main memory, as
/// `--main-memory-boards` gives them.
pub fn machine_with_pack_at(pack: &Path, boards: usize) -> Machine {
    let mut m = Machine::with_memory_boards(boards);
    m.load_prom(&muir::prom::boot_prom());
    m.disk.attach(0, Unit::open(pack, Geometry::T300).expect("the System 100 pack"));
    m
}

/// A partition of QUUX's GPT (contract Q8), in block-disk's 1024-byte
/// blocks: `first` its first block, `blocks` how many, `name` the GPT name
/// (the four-character Lisp name, then a space and a comment), `current`
/// its attribute bit 48.
#[derive(Clone, Debug)]
pub struct GptPartition {
    pub name: String,
    pub first: u32,
    pub blocks: u32,
    pub current: bool,
}

/// The primary GPT's partitions, read through muir's disk layer. The
/// fields are UEFI's: the header's `EFI PART` at LBA 1 with the entry
/// array's LBA at 72, entry count at 80 and entry size at 84; an entry's
/// type GUID at 0, first and last LBA at 32 and 40, attributes at 48 and
/// UTF-16LE name at 56. Q8 makes every partition whole blocks, first LBA
/// even and last odd, which this asserts.
pub fn gpt_partitions(d: &mut muir::disk_image::Disk) -> Vec<GptPartition> {
    let le = |b: &[u8], at: usize, n: usize| {
        b[at..at + n].iter().rev().fold(0u64, |v, &x| v << 8 | x as u64)
    };
    let bytes = |d: &mut muir::disk_image::Disk, n: u32| -> Vec<u8> {
        d.read_block(n).expect("a block of the disk").iter().flat_map(|w| w.to_le_bytes()).collect()
    };
    let header = bytes(d, 0)[512..].to_vec();
    assert_eq!(&header[..8], b"EFI PART", "the GPT header at LBA 1");
    let (array, count, size) = (le(&header, 72, 8), le(&header, 80, 4), le(&header, 84, 4));
    let mut table = Vec::new();
    for n in (array / 2) as u32..=((array * 512 + count * size - 1) / 1024) as u32 {
        table.extend(bytes(d, n));
    }
    let table = &table[(array as usize % 2) * 512..];
    (0..count as usize)
        .map(|k| &table[k * size as usize..(k + 1) * size as usize])
        .filter(|e| e[..16].iter().any(|&b| b != 0))
        .map(|e| {
            let (first, last) = (le(e, 32, 8), le(e, 40, 8));
            assert_eq!((first % 2, last % 2), (0, 1), "whole blocks");
            let name: Vec<u16> = e[56..128]
                .chunks(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|&u| u != 0)
                .collect();
            GptPartition {
                name: String::from_utf16(&name).unwrap(),
                first: (first / 2) as u32,
                blocks: ((last + 1 - first) / 2) as u32,
                current: le(e, 48, 8) >> 48 & 1 == 1,
            }
        })
        .collect()
}

/// The GPT partition whose Lisp name is `lisp`, the first four characters
/// of its GPT name.
pub fn gpt_partition(d: &mut muir::disk_image::Disk, lisp: &str) -> GptPartition {
    gpt_partitions(d)
        .into_iter()
        .find(|p| p.name.get(..4) == Some(lisp))
        .unwrap_or_else(|| panic!("no {lisp} in the GPT"))
}

/// MIT's microcode 323, `mit/sys/ubin/ucadr.mcr`, in partition order and
/// whole blocks: what `dd` writes into a QUUX microcode partition. The
/// PROM does not care whose microcode it loads, only about its sections.
pub fn ucadr_323_partition_order() -> Vec<u8> {
    let mut mcr = muir::mcr::swap_halves(muir::mcr::UCADR_323).unwrap();
    mcr.resize(mcr.len().div_ceil(1024) * 1024, 0);
    mcr
}

/// MIT's microcode 323 in the shapes PROM 2001 loads (contract G2 appendix
/// A1.4, A1.12), in partition order and whole blocks: its control store
/// section as it is; its dispatch memory section of 4,096 entries, MIT's
/// 2,048 then 2,048 of 0; its main-memory section's header with the
/// relative block its four blocks move to and their physical address
/// `6000`, page 3 at 1024-word pages, as microcode 2001's own section has
/// it, where MIT's says `1400`, page 3 at 256; A memory as section 5, each
/// word `<31:0>` then a high word of 0; and the four blocks after the
/// sections, at that block. Only the shapes are revision 13's: the program
/// is MIT's 32-bit one, which the PROM loads and jumps to as it would any.
pub fn ucadr_323_at_40_partition_order() -> Vec<u8> {
    let mit = muir::mcr::UCADR_323;
    let word = |at: usize| {
        let b = &mit[at..at + 4];
        (b[1] as u32) << 24 | (b[0] as u32) << 16 | (b[3] as u32) << 8 | b[2] as u32
    };
    let put = |out: &mut Vec<u8>, v: u32| {
        out.extend_from_slice(&((v >> 16) as u16).to_le_bytes());
        out.extend_from_slice(&(v as u16).to_le_bytes());
    };
    let mut out = Vec::new();
    let mut at = 0;
    let mut main_memory = None;
    loop {
        let (code, start, size) = (word(at), word(at + 4), word(at + 8) as usize);
        match code {
            1 => {
                let end = at + 12 + 8 * size;
                out.extend_from_slice(&mit[at..end]);
                at = end;
            }
            2 => {
                assert_eq!(size, 0o4000, "MIT's dispatch memory");
                for v in [2, start, 0o10000] {
                    put(&mut out, v);
                }
                out.extend_from_slice(&mit[at + 12..at + 12 + 4 * size]);
                out.resize(out.len() + 4 * 0o4000, 0);
                at += 12 + 4 * size;
            }
            3 => {
                // Blocks, relative block, physical address: written below,
                // once the relative block the data moves to is known.
                main_memory = Some((out.len(), start, size, word(at + 12)));
                out.resize(out.len() + 16, 0);
                at += 16;
            }
            4 => {
                for v in [5, start, size as u32] {
                    put(&mut out, v);
                }
                for k in 0..size {
                    put(&mut out, word(at + 12 + 4 * k));
                    put(&mut out, 0);
                }
                break;
            }
            _ => panic!("section {code} in MIT's microcode"),
        }
    }
    let (header, blocks, relative, physical) = main_memory.expect("a main-memory section");
    assert_eq!(physical, 3 << 8, "MIT's page 3");
    let physical = 3 << 10;
    out.resize(out.len().next_multiple_of(1024), 0);
    let moved = out.len() / 1024;
    let mut h = Vec::new();
    for v in [3, blocks, moved as u32, physical] {
        put(&mut h, v);
    }
    out[header..header + 16].copy_from_slice(&h);
    let from = relative * 1024;
    out.extend_from_slice(&mit[from..from + blocks as usize * 1024]);
    let mut mcr = muir::mcr::swap_halves(&out).unwrap();
    mcr.resize(mcr.len().next_multiple_of(1024), 0);
    mcr
}

/// A GPT disk QUUX's PROM boots, made from committed files:
/// `data/quux-disk.img`, made by sgdisk with `MCR1` current, with `mcr`
/// written at `MCR1`'s first block as `dd` writes it, as `pack.img` in
/// `dir`. Returns the disk and `MCR1`.
pub fn quux_gpt_disk(dir: &Path, mcr: &[u8]) -> (PathBuf, GptPartition) {
    let pack = dir.join("pack.img");
    std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/data/quux-disk.img"), &pack).unwrap();
    let mcr1 = gpt_partition(&mut muir::disk_image::Disk::open(&pack).unwrap(), "MCR1");
    assert!(mcr1.current, "MCR1 is the current microcode");
    assert!(mcr.len() <= mcr1.blocks as usize * 1024, "the .mcr fits MCR1");
    let mut bytes = std::fs::read(&pack).unwrap();
    let at = mcr1.first as usize * 1024;
    bytes[at..at + mcr.len()].copy_from_slice(mcr);
    std::fs::write(&pack, &bytes).unwrap();
    (pack, mcr1)
}

/// Lit pixels in rows `rows` of the screen.
pub fn lit_rows<E: Engine>(e: &E, rows: std::ops::Range<usize>) -> usize {
    let tv = &e.machine().tv;
    rows.flat_map(|y| (0..muir::tv::WIDTH).map(move |x| (x, y)))
        .filter(|&(x, y)| tv.pixel(x, y))
        .count()
}

/// Types `text` at the machine as a viewer does: each character a key
/// down and up, Shift held around an upper-case letter or a shifted
/// symbol, and `\n` the Return key. The board takes one word at a time
/// and the microcode reads it within a few thousand microcycles, so the
/// machine runs between words; a word it never reads fails the test
/// instead of running for ever.
pub fn type_at<E: Engine>(e: &mut E, k: &mut Keyboard, text: &str) {
    for ch in text.chars() {
        let sym = if ch == '\n' { keysym::RETURN } else { ch as u32 };
        let shifted = ch.is_ascii_uppercase() || "~!@#$%^&*()_+{}|:\"<>?".contains(ch);
        if shifted {
            k.key(keysym::SHIFT_L, true);
        }
        k.key(sym, true);
        k.key(sym, false);
        if shifted {
            k.key(keysym::SHIFT_L, false);
        }
        let mut waited = 0u64;
        // QUUX's keyboard is on the register page (contract Q3), the
        // CADR's on the I/O board.
        let on_quux = e.machine().geometry.machine_id.is_some();
        let waiting = |e: &E| {
            let m = e.machine();
            if on_quux { m.quux_input.key_waiting() } else { m.ioboard.keyboard_ready() }
        };
        while k.pending() > 0 || waiting(e) {
            if on_quux {
                k.deliver(&mut e.machine_mut().quux_input);
            } else {
                k.deliver(&mut e.machine_mut().ioboard);
            }
            for _ in 0..1_000 {
                e.step().expect("halted while typing");
            }
            waited += 1_000;
            assert!(waited < 50_000_000, "the machine never read the keyboard");
        }
    }
}

// --- cadr and quux as children of the tests: the command, what they write, ---
// --- a deadline on it, and a directory of the test's own --------------------

/// `cadr` itself, the binary Cargo built for these tests, with stdin closed
/// and no flags from the developer's own `~/.cadrrc`: `MUIR_RC` names an
/// empty file, so the run is the flags the test gives and nothing else ---
/// and no debug cable connector at the host's shared port, which the tests
/// running beside each other would fight over and a real debugger on the
/// host could find: `--no-debug-cable-listen`.  A test that wants the
/// connector gives `--debug-cable-listen 127.0.0.1:0` after it, the last
/// of the two winning; [`cadr_default`] is the run as a person gets it.
/// And as fast as the host goes, `--no-pace`, where a person's `rtl` or
/// `chip` would be paced; a test that wants pacing gives `--pace` after it.  A
/// test that types at the prompt opens stdin as a pipe instead.
pub fn cadr() -> std::process::Command {
    let mut c = cadr_default();
    c.arg("--no-debug-cable-listen");
    c.arg("--no-pace");
    c
}

/// `cadr` as a person runs it: [`cadr`] without the flag that leaves the
/// debug cable connector empty, for the tests of that default.
pub fn cadr_default() -> std::process::Command {
    // At run time, so that an example that takes this module in (as
    // `examples/profile.rs` does) builds: Cargo gives the binary's path to
    // test targets alone.
    built(option_env!("CARGO_BIN_EXE_cadr"), "cadr")
}

/// `quux` itself, as [`cadr`] is `cadr`: stdin closed, no file of flags,
/// and as fast as the host goes, `--no-pace`. QUUX has no debug cable, so
/// there is no connector to leave empty.
pub fn quux() -> std::process::Command {
    let mut c = quux_default();
    c.arg("--no-pace");
    c
}

/// `quux` as a person runs it: [`quux`] without `--no-pace`.
pub fn quux_default() -> std::process::Command {
    built(option_env!("CARGO_BIN_EXE_quux"), "quux")
}

/// [`cadr`] or [`quux`] by the executable's name, for the tests that ask
/// both the same thing.
pub fn executable(name: &str) -> std::process::Command {
    match name {
        "cadr" => cadr(),
        "quux" => quux(),
        _ => panic!("no executable {name}: cadr or quux"),
    }
}

/// The binary at `path`, with stdin closed and `MUIR_RC` naming an empty
/// file.
fn built(path: Option<&str>, name: &str) -> std::process::Command {
    let Some(path) = path else { panic!("only a test binary is told where {name} is") };
    let mut c = std::process::Command::new(path);
    c.stdin(std::process::Stdio::null());
    c.env("MUIR_RC", "/dev/null");
    c
}

/// The address a debuggee's DBGIN listens at, said in the setup on its
/// stderr once it is bound.  The debuggee is given `--debug-cable-listen
/// 127.0.0.1:0` and the host picks the port, so there is none to guess at;
/// and the debugger is started once the address has been said, so there is
/// none to lose in between either.
pub fn listening(debuggee: &Child) -> String {
    const SAID: &str = "debug cable: DBGIN listening at ";
    debuggee.stderr().wait_until(|t| t.contains(SAID), "the debuggee said where it listens");
    let t = debuggee.stderr().so_far();
    t.lines().find_map(|l| l.trim().strip_prefix(SAID)).unwrap().to_string()
}

/// What a run wrote, stdout then stderr, as text.
pub fn text(out: &std::process::Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

/// How long a child of the tests is given to end, or to write a line that
/// is waited for, before it is taken to have hung: killed, and the test
/// failed with what it wrote.  The longest run under it is a few seconds of
/// `micro`; the allowance is for a loaded machine, not for the run.
pub const DEADLINE: std::time::Duration = std::time::Duration::from_secs(300);

/// A directory of the test's own, removed when the guard is dropped --- on
/// the way out of a failing test as much as a passing one.  It derefs to
/// its path.
pub struct Scratch(PathBuf);

/// `muir-<name>-<pid>` under the system's temporary directory, made here.
pub fn scratch(name: &str) -> Scratch {
    Scratch::at(std::env::temp_dir().join(format!("muir-{name}-{}", std::process::id())))
}

impl Scratch {
    /// The directory at `path`, made here and removed with the guard.
    pub fn at(path: PathBuf) -> Scratch {
        std::fs::create_dir_all(&path).unwrap();
        Scratch(path)
    }

    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl std::ops::Deref for Scratch {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl AsRef<std::path::Path> for Scratch {
    fn as_ref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One of a child's pipes, read by a thread of its own as the child writes
/// it, so that the pipe never fills and what has come so far can be looked
/// at, and waited for, while the child runs.
pub struct Gathered {
    bytes: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl Gathered {
    pub fn from(mut pipe: impl std::io::Read + Send + 'static) -> Gathered {
        let bytes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = bytes.clone();
        let reader = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        });
        Gathered { bytes, reader: Some(reader) }
    }

    /// What has come so far, as text.
    pub fn so_far(&self) -> String {
        String::from_utf8_lossy(&self.bytes.lock().unwrap()).into_owned()
    }

    /// Whether the writer has closed the pipe: nothing more will come.
    pub fn closed(&self) -> bool {
        self.reader.as_ref().is_none_or(|r| r.is_finished())
    }

    /// Waits until `what` holds of what has come, and says so; or until
    /// `within` is up, or the pipe is closed with `what` still not so, and
    /// says not.
    pub fn wait_for(&self, what: impl Fn(&str) -> bool, within: std::time::Duration) -> bool {
        let until = std::time::Instant::now() + within;
        loop {
            // The reader appends the last bytes before it finishes, so a
            // pipe seen closed is looked at once more.
            let closed = self.closed();
            if what(&self.so_far()) {
                return true;
            }
            if closed || std::time::Instant::now() >= until {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// [`Gathered::wait_for`] under [`DEADLINE`], failing the test with
    /// `why` and what came if `what` never holds.
    pub fn wait_until(&self, what: impl Fn(&str) -> bool, why: &str) {
        assert!(
            self.wait_for(what, DEADLINE),
            "{why}: not within {} seconds{}; what came:\n{}",
            DEADLINE.as_secs(),
            if self.closed() { ", and the pipe closed" } else { "" },
            self.so_far()
        );
    }

    /// The whole of what the pipe carried, once the writer has closed it.
    fn all(mut self) -> Vec<u8> {
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
        std::mem::take(&mut *self.bytes.lock().unwrap())
    }
}

/// A child of the tests --- `muir`, or `script(1)` with `muir` under it ---
/// with its stdout and stderr gathered as they are written, killed when
/// dropped so that a failing test leaves no machine running, and waited
/// for under [`DEADLINE`].
pub struct Child {
    child: std::process::Child,
    stdout: Option<Gathered>,
    stderr: Option<Gathered>,
}

impl Child {
    /// Spawns `c` with stdout and stderr piped and gathered; stdin is as
    /// `c` has it.
    pub fn spawn(c: &mut std::process::Command) -> std::io::Result<Child> {
        let mut child =
            c.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn()?;
        let stdout = Gathered::from(child.stdout.take().unwrap());
        let stderr = Gathered::from(child.stderr.take().unwrap());
        Ok(Child { child, stdout: Some(stdout), stderr: Some(stderr) })
    }

    /// The child's stdin, which the command opened as a pipe; taken once.
    pub fn stdin(&mut self) -> std::process::ChildStdin {
        self.child.stdin.take().expect("stdin opened as a pipe, and not taken before")
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// `kill -INT`, what ^C sends: to this child alone, by its pid.
    pub fn interrupt(&self) {
        let pid = self.id().to_string();
        let status = std::process::Command::new("kill").args(["-INT", &pid]).status().unwrap();
        assert!(status.success(), "kill -INT {pid}");
    }

    /// What the child has written on stdout so far.
    pub fn stdout(&self) -> &Gathered {
        self.stdout.as_ref().unwrap()
    }

    /// What the child has written on stderr so far.
    pub fn stderr(&self) -> &Gathered {
        self.stderr.as_ref().unwrap()
    }

    /// Waits for the child to end, within [`DEADLINE`], and gives what it
    /// wrote and how it ended, as `Command::output` does.  One still going
    /// at the deadline is killed, and the test fails with what it wrote.
    pub fn wait(mut self) -> std::process::Output {
        let until = std::time::Instant::now() + DEADLINE;
        let status = loop {
            match self.child.try_wait().unwrap() {
                Some(status) => break status,
                None if std::time::Instant::now() >= until => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    panic!(
                        "the child had not ended {} seconds on, and was killed; it wrote:\n{}{}",
                        DEADLINE.as_secs(),
                        self.stdout().so_far(),
                        self.stderr().so_far()
                    );
                }
                None => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        };
        let stdout = self.stdout.take().unwrap().all();
        let stderr = self.stderr.take().unwrap().all();
        std::process::Output { status, stdout, stderr }
    }

    /// Ends the child: killed, and reaped, as dropping it does.
    pub fn kill(self) {
        drop(self);
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The two ways a test runs a command: to its end, or as a [`Child`] to be
/// typed at, watched and waited for.
pub trait Run {
    /// Runs it to its end within [`DEADLINE`]: `Command::output`, with the
    /// deadline on it.
    fn run(&mut self) -> std::process::Output;

    /// Starts it as a [`Child`].  The binary is always there, so failing to
    /// start it is a failure.
    fn start(&mut self) -> Child;
}

impl Run for std::process::Command {
    fn run(&mut self) -> std::process::Output {
        self.start().wait()
    }

    fn start(&mut self) -> Child {
        Child::spawn(self).unwrap_or_else(|e| panic!("{self:?}: {e}"))
    }
}

// --- the structural checks every board's netlist gets, and MIT's census ------
// --- by part type -----------------------------------------------------------

/// The nets no part on the board drives, sorted by name: what arrives from
/// off the board, or an input a pinout wrongly calls one. Supplies, spares
/// and the unnamed nets are left out.
pub fn undriven_nets(n: &Netlist) -> Vec<&str> {
    let mut driven = BTreeSet::new();
    for p in &n.parts {
        let Some(po) = part::pinout(&p.kind) else { continue };
        for &(pin, net) in &p.pins {
            if po.drives(pin) {
                driven.insert(net);
            }
        }
    }
    let mut undriven: Vec<&str> = (0..n.nets.len() as u32)
        .filter(|id| !driven.contains(id))
        .map(|id| n.net(id))
        .filter(|name| !is_power(name) && !name.starts_with('@'))
        .collect();
    undriven.sort_unstable();
    undriven
}

/// Prints [`undriven_nets`], for the reports of what plugs into a board.
pub fn report_undriven_nets(n: &Netlist) {
    let undriven = undriven_nets(n);
    eprintln!("{} nets with no driver on this board:", undriven.len());
    eprintln!("{undriven:?}");
}

/// **A pinout must not claim an output on its own supply pin**, or past the
/// end of its package. The two-drivers check cannot catch this on a
/// tri-state or open-collector part, those being exempt from it, which is
/// how a `74S472` entry claiming an output on pin 10, its ground, survived
/// for a while. Every kind the board uses is checked once. A kind declared
/// with no package --- a resistor pack, or a part whose ground is not the
/// middle pin --- has no standard supply pins to check, which is what
/// `Pinout::package` being zero means.
pub fn no_pinout_drives_its_own_supply_pin(n: &Netlist) {
    let kinds: BTreeSet<&str> = n.parts.iter().map(|p| p.kind.as_str()).collect();
    for kind in kinds {
        let Some(po) = part::pinout(kind) else { continue };
        if po.package == 0 {
            continue;
        }
        for &pin in po.outputs {
            assert_ne!(pin, po.gnd(), "{kind} claims an output on pin {pin}, its ground");
            assert_ne!(pin, po.vcc(), "{kind} claims an output on pin {pin}, its supply");
            assert!(
                pin <= po.package,
                "{kind} claims an output on pin {pin} of a DIP{}",
                po.package
            );
        }
    }
}

/// **No part uses its own ground or supply pin.** A pin the netlist puts on
/// a part's ground or supply is a wrong pinout or a wrong package size.
/// Parts declared with no package are passed over as above. Returns how
/// many parts were checked.
pub fn no_part_touches_its_own_supply_pins(n: &Netlist) -> usize {
    let mut checked = 0;
    for p in &n.parts {
        let Some(po) = part::pinout(&p.kind) else { continue };
        if po.package == 0 {
            continue;
        }
        checked += 1;
        for &(pin, _) in &p.pins {
            assert_ne!(
                pin,
                po.gnd(),
                "{} {} ({}) uses pin {pin}, ground on a DIP{}",
                p.page,
                p.reference,
                p.kind,
                po.package
            );
            assert_ne!(
                pin,
                po.vcc(),
                "{} {} ({}) uses pin {pin}, the supply on a DIP{}",
                p.page,
                p.reference,
                p.kind,
                po.package
            );
        }
    }
    checked
}

/// MIT's DIP census by part type: the same `.wls` file [`body_census`] reads
/// for sections, read for devices. Under each type's bodies comes a `WITH
/// LOCATIONS` line whose numbers are the sections the sheets use of the
/// type, the packages they take and the sections left spare; the second is
/// the count of devices, and it is the type's rather than a body's --- in
/// `cadr1/busint.wls` the type `74LS74` is three bodies and twelve sections
/// in six packages. Type names are cut to seven characters there, so
/// `SIP220/330-8` reads `SIP220/`.
pub fn dip_census(text: &str) -> BTreeMap<String, usize> {
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    let mut kind = String::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split('\t').map(str::trim).filter(|c| !c.is_empty()).collect();
        let Some(&first) = cols.first() else { continue };
        if !line.starts_with('\t') {
            kind = first.to_string();
        } else if first == "WITH LOCATIONS" {
            let n: Vec<usize> = cols[1..].iter().filter_map(|c| c.parse().ok()).collect();
            if let Some(&dips) = n.get(1) {
                *out.entry(kind.clone()).or_default() += dips;
            }
        }
    }
    out
}

// --- what is mounted on a board ---------------------------------------------
//
// Here rather than in `tests/parts_mounted.rs`, which is the account of the
// three numbers a netlist can be counted by, because `tests/documents.rs`
// holds the documents that quote this one to it. Two copies of a definition
// that nothing reconciles is the thing that file exists to stop.

/// Bypass capacitors, resistor and terminator packs, busbars, pull-up
/// networks, and the bodies a sheet carries with no device in them. MIT's
/// parts lists put these at sub-positions of a location --- `C08@01` beside
/// the chip at `C08` --- which is the shape of the thing: they are not
/// parts of the machine, and nothing simulates them.
///
/// The names are MIT's own body names, which is what the netlist's `kind`
/// and the stuffing lists' `BODY` column both carry, and one body has
/// several of them across the files: a bypass capacitor is `.1UFCAP` on the
/// memory board's sheets, `CAP1` on the multiplexor's, and `BYPASS` in
/// every stuffing list's capacitor page.
pub fn is_passive(kind: &str) -> bool {
    let k = kind.to_ascii_uppercase();
    k.contains("SIP")           // resistor packs: SIP180/390-8, DUAL-SIP
        || k.contains("DUMMY")  // a body on the sheet, no device in it
        || k.contains("SERRES")
        || k.starts_with("CAP")
        || k.ends_with("UFCAP")
        || k.starts_with("RES")
        || k.starts_with("DUAL-SI") // DUAL-SIP, cut to seven in a census
        || k.starts_with("898-") // a resistor network by its Bourns number
        || matches!(k.as_str(), "BUSBAR" | "BYPASS" | "PULLUP" | "TRITERM")
}

/// The board locations carrying a device, one part apiece. A location holds
/// one: where a designator appears twice it is a chip and the pack or
/// capacitor beside it, never two chips, which is what MIT's parts lists
/// say too and what [`mits_parts_lists_agree`] checks.
pub fn parts_on(n: &Netlist, on_board: impl Fn(&str) -> bool) -> BTreeSet<String> {
    let mut mounted: BTreeMap<&str, bool> = BTreeMap::new();
    for p in n.parts.iter().filter(|p| on_board(&p.page)) {
        *mounted.entry(p.reference.as_str()).or_insert(false) |= !is_passive(&p.kind);
    }
    mounted.into_iter().filter(|&(_, live)| live).map(|(r, _)| r.to_string()).collect()
}

pub fn parts(n: &Netlist) -> BTreeSet<String> {
    parts_on(n, |_| true)
}

/// A test program loaded as a boot PROM at 0, made to run on QUUX, whose
/// PROM is at 36000 (contract Q2): the program goes into the control
/// store's RAM at 0, where its jumps expect it, and the PROM is one jump
/// there, the slot after it inhibited. Nothing on the CADR, which runs the
/// PROM at 0 as it is.
pub fn prom_program_in_ram(m: &mut Machine) {
    use muir::isa::Insn;
    use muir::isa::asm::{ALWAYS, JUMP, N, target};
    let Some(base) = m.geometry.prom_base else { return };
    let words: Vec<Insn> = m.prom.clone();
    for (k, w) in words.into_iter().enumerate().take(base as usize) {
        m.imem[k] = w;
    }
    m.load_prom(&[Insn::new(JUMP | target(0) | ALWAYS | N)]);
    for k in 1..m.prom.len() {
        m.prom[k] = Insn::new(0);
    }
}
