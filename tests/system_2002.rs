// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! System 2002 on QUUX revision 16, from muir-sys's hand-over.
//!
//! System 2002 is not released. muir-sys's pre-release
//! `handover-2002-2999694`, fetched by `tools/fetch-handover-2002.sh` into
//! the gitignored `ref/band-2002-2999694/` and pinned by every file's
//! SHA-256, holds two disks --- QUUX's release disk with the band, and the
//! same disk with the high band, whose working storage lies across 2^31 ---
//! each with microcode 2002 in its current `MCR1`, "MCR1 UCADR 2002", and
//! the band in its current `LOD1`, "LOD1 System 2002"; PROM 2002, which
//! `quux --prom` takes, revision 16 having no boot PROM built in; and the
//! sources the band was built from, which unpack to
//! `release-2002-2999694/`, with the microcode's and the PROM's files in
//! their `sys/ubin/`. Without it the tests skip and say so.
//!
//! Each disk is run as the hand-over says to run it: `quux` with
//! `MUIR_QUUX_REVISION=16`, PROM 2002 given with `--prom`, the disk, and
//! the sources as the file device's root, the band's `FILES`; and
//! `--ethernet-socket`, behind which the test is the host. On both
//! engines --- `micro`, and `rtl`, which on revision 16 is its four-stage
//! pipeline --- the band boots to its listener: its TELNET server answers
//! a connection to port 23 at the machine's default address and sends the
//! herald, System 2002 on microcode 2002 on QUUX, and "Telnet server
//! here". The pre-release's PROM has its published digest and is the
//! sources' `sys/ubin/promh.mcr`, byte for byte.

use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

mod support;

use support::Run;

/// The hand-over, by its tag, and where the fetch script puts it.
const TAG: &str = "handover-2002-2999694";
const DIR: &str = "ref/band-2002-2999694";

/// PROM 2002's SHA-256, as the pre-release's `SHA256SUMS` gives it for
/// `handover-2002-2999694-promh.mcr`.
const PROM_2002_SHA256: &str = "3d93319a8e851f5f14a2fcd66cd1ed21abb33049c355edf8cc893c791fb4b124";

/// A file of the hand-over, or `None` with the skip line, written whole
/// in one call so that no other test's output lands inside it: CI looks
/// for the hand-over's directory or fetch script in it.
fn handover(file: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DIR).join(file);
    if p.exists() {
        Some(p)
    } else {
        let line = format!(
            "skipped: {DIR}/{file} is not present; tools/fetch-handover-2002.sh fetches it\n"
        );
        eprint!("{line}");
        None
    }
}

/// The SHA-256 of `path`, by the system's `sha256sum`.
fn sha256(path: &Path) -> String {
    let out = std::process::Command::new("sha256sum").arg(path).output().expect("sha256sum");
    assert!(out.status.success(), "sha256sum {}", path.display());
    String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap().to_string()
}

/// A copy of the hand-over's disk `which` (`disk` or `high`), which the
/// machine writes, and a file root in a scratch directory named `name`:
/// `root/`, the sources unpacked, their `sys` and `site` with an empty
/// `home/lispm`, as `quux --file-root` serves it. `None` with the skip
/// line.
fn band(which: &str, name: &str) -> Option<(support::Scratch, PathBuf, PathBuf)> {
    let disk = handover(&format!("{TAG}-{which}.vhd"))?;
    let sources = handover(&format!("{TAG}-sys.tar.gz"))?;
    let dir = support::scratch(name);
    let pack = dir.join("pack.vhd");
    std::fs::copy(&disk, &pack).unwrap();
    let untar = std::process::Command::new("tar")
        .arg("xzf")
        .arg(&sources)
        .arg("-C")
        .arg(dir.path())
        .status()
        .unwrap();
    assert!(untar.success(), "{} unpacks", sources.display());
    // The sources themselves are the root: the file device refuses a
    // symlink that leaves its mount.
    let root = dir.join("root");
    std::fs::rename(dir.join("release-2002-2999694"), &root).unwrap();
    std::fs::create_dir_all(root.join("home/lispm")).unwrap();
    Some((dir, pack, root))
}

// --- the host side of the link: ARP and a TCP client of one connection ---

/// The host's end of the link: a MAC address of its own, and the
/// machine's default gateway, 169.254.100.1, the tap's host side.
const HOST_MAC: [u8; 6] = [0x02, 0, 0, 0, 0, 0x02];
const HOST_IP: [u8; 4] = [169, 254, 100, 1];
/// The machine's default addresses (`muir::ethernet_device::Host`).
const QUUX_MAC: [u8; 6] = [0x02, 0, 0, 0, 0, 0x01];
const QUUX_IP: [u8; 4] = [169, 254, 100, 2];
const TELNET: u16 = 23;
const OUR_PORT: u16 = 40023;
const OUR_ISN: u32 = 1000;

/// An Ethernet frame from the host to the machine of `ethertype`.
fn ether(ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(14 + payload.len());
    f.extend_from_slice(&QUUX_MAC);
    f.extend_from_slice(&HOST_MAC);
    f.extend_from_slice(&ethertype.to_be_bytes());
    f.extend_from_slice(payload);
    f
}

/// The host's ARP reply to the machine's request for `HOST_IP` (RFC 826:
/// Ethernet, IPv4, op 2).
fn arp_reply() -> Vec<u8> {
    let mut a = vec![0, 1, 8, 0, 6, 4, 0, 2];
    a.extend_from_slice(&HOST_MAC);
    a.extend_from_slice(&HOST_IP);
    a.extend_from_slice(&QUUX_MAC);
    a.extend_from_slice(&QUUX_IP);
    ether(0x0806, &a)
}

/// Whether `f` is an ARP request (op 1) from the machine for `HOST_IP`.
fn asks_for_the_host(f: &[u8]) -> bool {
    f.len() >= 42 && f[12..14] == [8, 6] && f[20..22] == [0, 1] && f[38..42] == HOST_IP
}

/// TCP's flags (RFC 9293, 3.1).
const FIN: u8 = 0x01;
const SYN: u8 = 0x02;
const ACK: u8 = 0x10;

/// A TCP segment from the host's port to the machine's TELNET port, its
/// checksums filled by the rules the machine's own transmit applies
/// (`muir::ethernet_offload::transmit`).
fn segment(seq: u32, ack: u32, flags: u8) -> Vec<u8> {
    let total = 20 + 20;
    let mut p = vec![0x45, 0, 0, total, 0, 0, 0x40, 0, 64, 6, 0, 0];
    p.extend_from_slice(&HOST_IP);
    p.extend_from_slice(&QUUX_IP);
    p.extend_from_slice(&OUR_PORT.to_be_bytes());
    p.extend_from_slice(&TELNET.to_be_bytes());
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(&ack.to_be_bytes());
    p.extend_from_slice(&[5 << 4, flags, 0xff, 0xff, 0, 0, 0, 0]);
    let mut f = ether(0x0800, &p);
    muir::ethernet_offload::transmit(&mut f).expect("a well-formed segment");
    f
}

/// A TCP segment the machine sent from its TELNET port to the host's.
struct Segment {
    seq: u32,
    ack: u32,
    flags: u8,
    data: Vec<u8>,
}

fn from_telnet(f: &[u8]) -> Option<Segment> {
    if f.len() < 54 || f[12..14] != [8, 0] || f[23] != 6 || f[26..30] != QUUX_IP {
        return None;
    }
    let h = 14 + (f[14] as usize & 0xf) * 4;
    let total = 14 + u16::from_be_bytes([f[16], f[17]]) as usize;
    let t = f.get(h..total.min(f.len()))?;
    let port = |k: usize| u16::from_be_bytes([t[k], t[k + 1]]);
    if t.len() < 20 || port(0) != TELNET || port(2) != OUR_PORT {
        return None;
    }
    let word = |k: usize| u32::from_be_bytes([t[k], t[k + 1], t[k + 2], t[k + 3]]);
    let off = (t[12] >> 4) as usize * 4;
    Some(Segment { seq: word(4), ack: word(8), flags: t[13], data: t.get(off..)?.to_vec() })
}

/// `--ethernet-socket`'s plain mode (`muir::ethernet_socket`): each
/// Ethernet frame a record, its length in 32 bits big-endian and its
/// bytes.
fn put(s: &mut UnixStream, f: &[u8]) {
    s.write_all(&(f.len() as u32).to_be_bytes()).unwrap();
    s.write_all(f).unwrap();
}

/// The whole records in `buf`, taken from it.
fn records(buf: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    while buf.len() >= 4 {
        let n = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        if buf.len() < 4 + n {
            break;
        }
        out.push(buf[4..4 + n].to_vec());
        buf.drain(..4 + n);
    }
    out
}

// --- the boot ---

/// The microcycles a run may take: about five times the 190 million or so
/// the disk's band takes on `micro` to its TELNET server.
const LIMIT: &str = "1000000000";

/// What the TELNET server said, once it said "Telnet server here".
const HERE: &str = "Telnet server here";

/// Boots the hand-over's disk `which` on `engine` as the hand-over says to
/// run it, and connects to its TELNET server: what the session sent, up
/// to and with "Telnet server here".
fn boots(which: &str, engine: &str) -> Option<String> {
    let prom = handover(&format!("{TAG}-promh.mcr"))?;
    let name = format!("system-2002-{which}-{engine}");
    let (dir, pack, root) = band(which, &name)?;
    let sock = dir.join("ether.sock");
    let gif = dir.join("screen.gif");
    let listener = UnixListener::bind(&sock).unwrap();
    listener.set_nonblocking(true).unwrap();
    let t = Instant::now();
    let child = support::quux()
        .env("MUIR_QUUX_REVISION", "16")
        .arg(format!("--{engine}"))
        .args(["--terminal", "127.0.0.1:0", "--stop-after", LIMIT, "--tv-capture-no-time"])
        .arg("--tv-capture")
        .arg(&gif)
        .arg("--prom")
        .arg(&prom)
        .arg("--disk-pack")
        .arg(&pack)
        .arg("--file-root")
        .arg(&root)
        .arg("--ethernet-socket")
        .arg(&sock)
        .start();
    let failed = |why: &str| -> ! {
        let kept = std::env::temp_dir().join(format!("muir-{name}.gif"));
        let _ = std::fs::copy(&gif, &kept);
        panic!("{name}: {why}; the screen is {}:\n{}", kept.display(), child.stderr().so_far())
    };
    let mut s = loop {
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                if child.stderr().closed() {
                    failed("quux ended without connecting to the socket");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("{name}: accept: {e}"),
        }
    };
    s.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    let (mut buf, mut chunk) = (Vec::new(), [0u8; 4096]);
    let mut session = Vec::new();
    let mut their_next: Option<u32> = None;
    let mut syn_at: Option<Instant> = None;
    while !String::from_utf8_lossy(&session).contains(HERE) {
        // A SYN, and again every five seconds until it is answered.
        if their_next.is_none() && syn_at.is_none_or(|at| at.elapsed() > Duration::from_secs(5)) {
            syn_at = Some(Instant::now());
            put(&mut s, &segment(OUR_ISN, 0, SYN));
        }
        match s.read(&mut chunk) {
            Ok(0) => failed("the socket closed before the TELNET server said it was here"),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                if child.stderr().closed() {
                    failed("the run ended before the TELNET server said it was here");
                }
                continue;
            }
            Err(e) => failed(&format!("the socket: {e}")),
        }
        for f in records(&mut buf) {
            if asks_for_the_host(&f) {
                put(&mut s, &arp_reply());
                continue;
            }
            let Some(g) = from_telnet(&f) else { continue };
            if g.flags & (SYN | ACK) == SYN | ACK && g.ack == OUR_ISN + 1 {
                their_next = Some(g.seq.wrapping_add(1));
            } else if let Some(next) = their_next {
                if !g.data.is_empty() && g.seq == next {
                    session.extend_from_slice(&g.data);
                    their_next = Some(next.wrapping_add(g.data.len() as u32));
                }
                if g.flags & FIN != 0 {
                    failed(&format!(
                        "the TELNET server closed the connection after {:?}",
                        String::from_utf8_lossy(&session)
                    ));
                }
            }
            if let Some(next) = their_next {
                put(&mut s, &segment(OUR_ISN + 1, next, ACK));
            }
        }
    }
    eprintln!("{name}: the TELNET server here after {:.1} s", t.elapsed().as_secs_f64());
    child.kill();
    Some(String::from_utf8_lossy(&session).into_owned())
}

/// The herald the session sends, its spacing as one space: System 2002,
/// microcode 2002, QUUX on muir-sim, the band's `FILES`.
fn heralds_system_2002(which: &str, engine: &str) {
    let Some(session) = boots(which, engine) else { return };
    let said = session.split_whitespace().collect::<Vec<_>>().join(" ");
    for line in [
        "Experimental System 2002",
        "Microcode 2002",
        "Machine Type QUUX on muir-sim",
        "with associated machine FILES",
        HERE,
    ] {
        assert!(said.contains(line), "{which} on {engine}: no {line:?} in {session:?}");
    }
}

#[test]
fn system_2002_boots_to_its_telnet_server_on_micro() {
    heralds_system_2002("disk", "micro");
}

#[test]
fn system_2002_boots_to_its_telnet_server_on_rtl() {
    heralds_system_2002("disk", "rtl");
}

#[test]
fn the_high_band_boots_to_its_telnet_server_on_micro() {
    heralds_system_2002("high", "micro");
}

#[test]
fn the_high_band_boots_to_its_telnet_server_on_rtl() {
    heralds_system_2002("high", "rtl");
}

/// **PROM 2002 is the hand-over's**: the pre-release's `promh.mcr` has the
/// digest its `SHA256SUMS` gives, and the sources' `sys/ubin/promh.mcr`,
/// which the fetch script copies out of them, is the same file byte for
/// byte; and it reads as a revision-16 boot PROM.
#[test]
fn prom_2002_is_the_hand_over_s() {
    let Some(prom) = handover(&format!("{TAG}-promh.mcr")) else { return };
    let Some(ubin) = handover("promh.mcr") else { return };
    assert_eq!(sha256(&prom), PROM_2002_SHA256, "{TAG}-promh.mcr");
    assert_eq!(sha256(&ubin), PROM_2002_SHA256, "the sources' sys/ubin/promh.mcr");
    muir::prom::parse_quux_mcr(&std::fs::read(&prom).unwrap(), muir::machine::Geometry::QUUX_16)
        .expect("PROM 2002 reads on revision 16");
}
