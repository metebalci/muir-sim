// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `tools/ethernet-frame-mover`, held to what it says it does, without a
//! tap (QUUX's Ethernet contract, §3, "Where the pieces live").
//!
//! The mover is Python with the standard library. These tests load it as
//! a module in `python3` and drive its parts: the line it prints for a tap,
//! from the tap's own address; the Ethernet frames it moves both ways,
//! with a datagram socket pair in place of the tap, whose reads, like a
//! tap's, give one Ethernet frame each; the warning, once for a
//! connection, when the machine sends from another address than its
//! tap's; and the refusals that need no tap. Taking a real tap, and a tap
//! another program holds, need the pool `quux-taps` installs, which no CI
//! runner has; they are checked by hand on a host that has it.
//!
//! A runner without `python3` skips them, saying so.

use std::path::PathBuf;
use std::process::{Command, Output};

fn mover() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tools/ethernet-frame-mover")
}

/// What every check starts with: the mover loaded as the module `m`, and
/// the helpers the checks share.
const PRELUDE: &str = r#"
import importlib.machinery, importlib.util, io, os, socket, struct, sys, threading
loader = importlib.machinery.SourceFileLoader("mover", sys.argv[1])
spec = importlib.util.spec_from_loader("mover", loader)
m = importlib.util.module_from_spec(spec)
loader.exec_module(m)

def ip(t):
    return int.from_bytes(socket.inet_aton(t), "big")

def mask(n):
    return (0xFFFFFFFF << (32 - n)) & 0xFFFFFFFF

def arp(sender):
    f = bytes([0xff] * 6) + bytes([2, 0, 0, 0, 0, 1]) + b"\x08\x06"
    f += struct.pack("!HHBBH", 1, 0x0800, 6, 4, 1) + bytes([2, 0, 0, 0, 0, 1])
    f += socket.inet_aton(sender) + bytes(6) + socket.inet_aton("169.254.101.1")
    return f + bytes(60 - len(f))

def ipv4(sender):
    f = bytes([2, 0, 0, 0, 0, 2]) + bytes([2, 0, 0, 0, 0, 1]) + b"\x08\x00"
    f += bytes([0x45, 0]) + struct.pack("!H", 28) + bytes(8)
    f += socket.inet_aton(sender) + socket.inet_aton("169.254.101.1") + bytes(8)
    return f + bytes(60 - len(f))

def record(f):
    return struct.pack("!I", len(f)) + f

def read_record(s):
    def exactly(n):
        b = b""
        while len(b) < n:
            got = s.recv(n - len(b))
            assert got, "the connection closed"
            b += got
        return b
    n = struct.unpack("!I", exactly(4))[0]
    return exactly(n)
"#;

/// `check`, after the prelude, in `python3 -I -B`, which writes no
/// bytecode beside the mover, with the mover's path as its argument;
/// `None` when there is no `python3`.
fn python(check: &str) -> Option<Output> {
    let code = format!("{PRELUDE}\n{check}\nprint('checked')\n");
    match Command::new("python3").args(["-I", "-B", "-c"]).arg(code).arg(mover()).output() {
        Ok(out) => Some(out),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipped: no python3 to run tools/ethernet-frame-mover");
            None
        }
        Err(e) => panic!("python3: {e}"),
    }
}

fn passes(check: &str) {
    let Some(out) = python(check) else { return };
    let (o, e) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && o.contains("checked"), "stdout:\n{o}\nstderr:\n{e}");
}

/// The mover run as a program with `args`; `None` without `python3`.
fn run(args: &[&str]) -> Option<Output> {
    match Command::new("python3").args(["-I", "-B"]).arg(mover()).args(args).output() {
        Ok(out) => Some(out),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipped: no python3 to run tools/ethernet-frame-mover");
            None
        }
        Err(e) => panic!("python3: {e}"),
    }
}

/// **The line is the tap and quux's two flags, from the tap's own
/// address** (§3, "The mover's flag and line"): QUUX's address is the
/// /30's other host address and the gateway the tap's own, for every tap
/// of the pool as `quux-taps` gives them, and for a tap whose host side is
/// the /30's second host address; a tap that is not a /30, or
/// whose address is the /30's network or broadcast address, is refused.
/// **Fails** a mover that prints quux0's line for another tap, or keeps a
/// second copy of the rule.
#[test]
fn the_line_names_the_tap_and_quux_s_flags_from_its_own_address() {
    passes(
        r#"
cases = [
    ("quux0", "169.254.100.1", "quux0 --ip-address 169.254.100.2/30 --ip-gateway 169.254.100.1"),
    ("quux1", "169.254.101.1", "quux1 --ip-address 169.254.101.2/30 --ip-gateway 169.254.101.1"),
    ("other", "192.0.2.6", "other --ip-address 192.0.2.5/30 --ip-gateway 192.0.2.6"),
]
# Every tap of the pool, its host side as quux-taps gives it.
for k in range(64):
    host, quux = f"169.254.{100 + k}.1", f"169.254.{100 + k}.2"
    cases.append((f"quux{k}", host, f"quux{k} --ip-address {quux}/30 --ip-gateway {host}"))
for name, host, want in cases:
    got = m.flags_line(name, ip(host), mask(30))
    assert got == want, (got, want)
for host, n, why in [("192.0.2.6", 29, "is not a /30"), ("192.0.2.4", 30, "not one of its"), ("192.0.2.7", 30, "not one of its")]:
    try:
        m.flags_line("other", ip(host), mask(n))
        raise AssertionError(f"{host}/{n} taken")
    except m.Refused as e:
        assert why in str(e), str(e)
"#,
    );
}

/// **Ethernet frames move both ways in plain mode's records, and the
/// machine sending from another address is warned of once for each
/// connection, its Ethernet frames moved all the same** (§3): records the
/// machine writes, a byte at a time and with one of length 0 among them,
/// reach the tap whole, one Ethernet frame a read; what the tap gives
/// reaches the machine as records; ARP and IPv4 from 169.254.0.2 behind
/// quux1 give one warning naming both flags, and ARP from quux1's own
/// QUUX address none; a second connection is warned again. **Fails** a
/// warning given twice, never, or for the right address, or a frame
/// dropped.
#[test]
fn frames_move_both_ways_and_a_foreign_sender_is_warned_of_once() {
    passes(
        r#"
tap, far = socket.socketpair(socket.AF_UNIX, socket.SOCK_DGRAM)
err = io.StringIO()
mv = m.Mover(tap.fileno(), "quux1", ip("169.254.101.1"), mask(30), err)
assert mv.line == "quux1 --ip-address 169.254.101.2/30 --ip-gateway 169.254.101.1", mv.line
far.settimeout(10)

def connection(sends):
    ours, theirs = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    theirs.settimeout(10)
    t = threading.Thread(target=mv.pump, args=(ours,), daemon=True)
    t.start()
    data = b"".join(record(f) for f in sends)
    for k in range(len(data)):
        theirs.sendall(data[k:k + 1])
    got = [far.recv(65536) for f in sends if f]
    far.send(arp("169.254.101.1"))
    far.send(ipv4("169.254.101.1")[:42])
    back = [read_record(theirs), read_record(theirs)]
    theirs.close()
    t.join(10)
    assert not t.is_alive(), "the pump never saw the close"
    ours.close()
    return got, back

sends = [arp("169.254.0.2"), b"", ipv4("169.254.0.2"), arp("169.254.101.2"), ipv4("169.254.0.2")]
got, back = connection(sends)
assert got == [f for f in sends if f], "every Ethernet frame reaches the tap whole"
assert back == [arp("169.254.101.1"), ipv4("169.254.101.1")[:42]], "the tap's reach the machine"
said = err.getvalue().splitlines()
assert len(said) == 1, said
w = said[0]
for part in ["warning", "169.254.0.2", "quux1", "169.254.101.2", "--ip-address 169.254.101.2/30 --ip-gateway 169.254.101.1"]:
    assert part in w, (part, w)
got, _ = connection([arp("169.254.101.2"), ipv4("169.254.101.2")])
assert len(got) == 2
assert len(err.getvalue().splitlines()) == 1, "none for the right address"
connection([ipv4("169.254.0.2")])
assert len(err.getvalue().splitlines()) == 2, "a second connection warned again"
assert m.sender(arp("0.0.0.0")) == 0
assert m.sender(bytes(14) + b"\x86\xdd") is None
"#,
    );
}

/// **What needs no tap is refused before anything is taken** (§3): a tap
/// that does not exist, or a network interface that is not a tap, named
/// with `--tap`, is refused with that reason for each, status 1; a usage
/// error is status 2: no socket path, two, `--tap` with no names or an
/// empty one, or a flag the mover does not have. **Fails** a named tap
/// passed over in silence, or a refusal that does not say why.
#[test]
fn a_named_tap_it_cannot_take_is_refused_with_the_reason() {
    let dir = std::env::temp_dir().join(format!("muir-mover-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("m.sock");
    let s = sock.to_str().unwrap();
    let Some(out) = run(&["--tap", "nosuchtap0,lo", s]) else { return };
    let e = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(1), "{e}");
    for why in ["the taps named", "nosuchtap0: no such tap", "lo: not a tap"] {
        assert!(e.contains(why), "{why:?}: {e}");
    }
    assert!(out.stdout.is_empty(), "no line without a tap");
    assert!(!sock.exists(), "nothing listens without a tap");
    for args in [
        &[][..],
        &["a.sock", "b.sock"],
        &["--tap"],
        &["--tap", "quux0,", s],
        &["--tap", "", s],
        &["--address", "192.0.2.1", s],
    ] {
        let out = run(args).unwrap();
        let e = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(2), "{args:?}: {e}");
        assert!(e.contains("usage: ethernet-frame-mover"), "{args:?}: {e}");
    }
    let out = run(&["--help"]).unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("--tap NAME[,NAME...]"));
    std::fs::remove_dir_all(&dir).ok();
}
