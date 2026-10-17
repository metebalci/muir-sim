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
h = ip("169.254.101.1")
mv = m.Mover(tap.fileno(), "quux1", m.quux_side("quux1", h, mask(30)), m.flags_line("quux1", h, mask(30)), err)
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

/// A host's kernel as the mover reads it, standing in for `/proc/net/route`,
/// `/proc/sys`, `ip neigh show proxy`, the taps' addresses and their
/// flags: quux0 a LAN tap for 192.0.2.50/24 on `eth0`, whose router is
/// 192.0.2.1, set up as `quux-taps` sets it (QUUX's Ethernet contract,
/// §4.3), and quux1 a host-only tap.
const KERNEL: &str = r#"
def rt(iface, dest, mask_bits, gateway="0.0.0.0", flags=1, metric=0):
    def h(t):
        return "%08X" % struct.unpack("=I", socket.inet_aton(t))[0]
    return f"{iface}\t{h(dest)}\t{h(gateway)}\t{flags:04X}\t0\t0\t{metric}\t{h(m.text(mask(mask_bits)) if mask_bits else '0.0.0.0')}\t0\t0\t0\n"

HEADER = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n"
ROUTES = (HEADER + rt("eth0", "0.0.0.0", 0, "192.0.2.1", 3, 100) + rt("eth0", "192.0.2.0", 24, metric=100)
          + rt("quux0", "192.0.2.50", 32, flags=5) + rt("eth0", "198.51.100.0", 24, "192.0.2.1", 3))
SYSCTLS = {
    "net/ipv4/conf/all/forwarding": "0", "net/ipv4/conf/default/forwarding": "0",
    "net/ipv4/conf/all/proxy_arp": "0", "net/ipv4/conf/eth0/proxy_arp": "0",
    "net/ipv4/conf/eth0/forwarding": "1", "net/ipv4/neigh/eth0/proxy_delay": "0",
    "net/ipv4/conf/quux0/forwarding": "1", "net/ipv4/conf/quux0/proxy_arp": "1",
    "net/ipv4/conf/quux0/arp_ignore": "0", "net/ipv4/conf/quux0/arp_filter": "1",
    "net/ipv4/conf/quux0/rp_filter": "1", "net/ipv4/neigh/quux0/proxy_delay": "0",
    "net/ipv6/conf/quux0/disable_ipv6": "1",
}

class Fake(m.Kernel):
    def __init__(self, routes=ROUTES, sysctls=None, proxy="192.0.2.50 dev eth0 proxy \n",
                 addresses=None, up=("quux0", "quux1")):
        self.r, self.p, self.u = routes, proxy, set(up)
        self.s = dict(SYSCTLS if sysctls is None else sysctls)
        self.a = {"quux1": (ip("169.254.101.1"), mask(30))} if addresses is None else addresses
    def route_text(self):
        return self.r
    def sysctl(self, path):
        return self.s.get(path)
    def proxy_text(self):
        return self.p
    def ipv4(self, name):
        return self.a.get(name)
    def is_up(self, name):
        return name in self.u
    def exists(self, name):
        return name in ("quux0", "quux1", "eth0")
    def is_tap(self, name):
        return name.startswith("quux")

def refused(kernel, *says, name="quux0"):
    try:
        got = m.tap_line(name, kernel)
    except m.Refused as e:
        for s in says:
            assert s in str(e), (s, str(e))
        return str(e)
    raise AssertionError(f"taken: {got}")
"#;

/// **On a LAN tap the line is read from the kernel** (§3, "A LAN tap";
/// amendment 3, §5.2): QUUX's address from quux0's /32, the prefix from
/// the LAN interface's connected route, the LAN interface named by the
/// proxy entry for that address, and the gateway from the default route on
/// it; a host-only tap's line is as before. **Fails** a mover that prints
/// the pool's /30 rule for a LAN tap, takes the prefix from anywhere but
/// L's connected route, or a gateway from another interface.
#[test]
fn a_lan_tap_s_line_is_read_from_the_kernel() {
    passes(&format!(
        "{KERNEL}\n{}",
        r#"
assert m.tap_line("quux0", Fake()) == (ip("192.0.2.50"), "quux0 --ip-address 192.0.2.50/24 --ip-gateway 192.0.2.1"), m.tap_line("quux0", Fake())
q, line = m.tap_line("quux1", Fake())
assert line == "quux1 --ip-address 169.254.101.2/30 --ip-gateway 169.254.101.1", line
assert m.text(q) == "169.254.101.2"
# The lowest metric of two default routes on L; a default route on another
# interface is not L's.
two = ROUTES + rt("eth0", "0.0.0.0", 0, "192.0.2.254", 3, 50) + rt("eth1", "0.0.0.0", 0, "192.0.2.7", 3, 1)
assert m.tap_line("quux0", Fake(routes=two))[1].endswith("--ip-gateway 192.0.2.254")
# A /25 inside L's /24 that is not connected (through a gateway) is not
# the prefix.
via = ROUTES + rt("eth0", "192.0.2.0", 25, "192.0.2.1", 3)
assert m.tap_line("quux0", Fake(routes=via))[1] == "quux0 --ip-address 192.0.2.50/24 --ip-gateway 192.0.2.1"
"#
    ));
}

/// **A LAN tap's gateway must pass muir's refusals, or the mover refuses
/// and names `--ip-gateway`** (amendment 3, §5.2): no default route on L,
/// one through a gateway off QUUX's subnet, at QUUX's own address, or at
/// the subnet's broadcast address; and with no connected route for QUUX's
/// address on L there is no prefix. **Fails** a line printed with a
/// gateway muir would refuse.
#[test]
fn a_lan_tap_s_gateway_off_its_subnet_is_refused_by_name() {
    passes(&format!(
        "{KERNEL}\n{}",
        r#"
no_default = HEADER + rt("eth0", "192.0.2.0", 24) + rt("quux0", "192.0.2.50", 32, flags=5)
refused(Fake(routes=no_default), "--ip-gateway", "eth0")
other_iface = no_default + rt("eth1", "0.0.0.0", 0, "192.0.2.1", 3)
refused(Fake(routes=other_iface), "--ip-gateway")
for g in ["198.51.100.1", "192.0.2.50", "192.0.2.255", "192.0.2.0"]:
    refused(Fake(routes=no_default + rt("eth0", "0.0.0.0", 0, g, 3)), "--ip-gateway", g)
no_prefix = HEADER + rt("eth0", "0.0.0.0", 0, "192.0.2.1", 3) + rt("quux0", "192.0.2.50", 32, flags=5)
refused(Fake(routes=no_prefix), "eth0", "connected route")
"#
    ));
}

/// **Before it takes a LAN tap the mover reads its settings back, and
/// refuses with the fix when one is gone** (amendment 3, §5.2): each of
/// §4.3's settings on quux0 and on L, `conf.all` and `conf.default`
/// forwarding (a later global `ip_forward`), `proxy_arp` on all or on L,
/// the proxy entry, quux0's /32 and quux0 being up; each refusal names the
/// setting and `sudo systemctl restart quux-taps.service`, and no tap is
/// opened. **Fails** a mover that takes a LAN tap whose host side a network
/// manager or a global write has undone.
#[test]
fn a_lan_tap_whose_settings_are_gone_is_refused_with_the_fix() {
    passes(&format!(
        "{KERNEL}\n{}",
        r#"
FIX = "sudo systemctl restart quux-taps.service"
assert m.FIX == FIX
wrong = {"0": "1", "1": "0"}
for path, value in SYSCTLS.items():
    s = dict(SYSCTLS)
    s[path] = wrong[value]
    refused(Fake(sysctls=s), FIX, path.replace("/", "."))
    s = dict(SYSCTLS)
    del s[path]
    if path != "net/ipv6/conf/quux0/disable_ipv6":
        refused(Fake(sysctls=s), FIX, path.replace("/", "."))
# A kernel without IPv6 has nothing to turn off.
s = dict(SYSCTLS); del s["net/ipv6/conf/quux0/disable_ipv6"]
assert m.tap_line("quux0", Fake(sysctls=s))[0] == ip("192.0.2.50")
s = dict(SYSCTLS); s["net/ipv4/neigh/eth0/proxy_delay"] = "80"
refused(Fake(sysctls=s), FIX, "proxy_delay")
refused(Fake(proxy=""), FIX, "proxy entry")
refused(Fake(proxy="192.0.2.51 dev eth0 proxy \n"), FIX, "proxy entry")
refused(Fake(proxy="192.0.2.50 dev eth0 proxy \n192.0.2.50 dev eth1 proxy \n"), FIX, "proxy entry", "eth1")
refused(Fake(up=("quux1",)), FIX, "down")
no_route = HEADER + rt("eth0", "0.0.0.0", 0, "192.0.2.1", 3) + rt("eth0", "192.0.2.0", 24)
refused(Fake(routes=no_route), FIX, "/32")
two = ROUTES + rt("quux0", "198.51.100.7", 32, flags=5)
refused(Fake(routes=two), FIX, "/32")
# Taking: the refusal comes before any tap is opened, and is the run's.
def no_open(name):
    raise AssertionError(f"{name} opened")
m.open_tap = no_open
try:
    m.take(["quux0", "quux1"], False, Fake(proxy=""))
    raise AssertionError("taken")
except m.Refused as e:
    assert FIX in str(e) and e.status == 1, str(e)
"#
    ));
}

/// **The warning on a LAN tap names its own flags** (§3): a band sending
/// from another address than quux0's LAN address is told the line's
/// flags. **Fails** a warning with the pool's rule for a LAN tap.
#[test]
fn a_lan_tap_s_warning_names_its_flags() {
    passes(&format!(
        "{KERNEL}\n{}",
        r#"
tap, far = socket.socketpair(socket.AF_UNIX, socket.SOCK_DGRAM)
err = io.StringIO()
q, line = m.tap_line("quux0", Fake())
mv = m.Mover(tap.fileno(), "quux0", q, line, err)
mv.to_tap(arp("169.254.100.2"), False)
w = err.getvalue()
assert "--ip-address 192.0.2.50/24 --ip-gateway 192.0.2.1" in w and "169.254.100.2" in w, w
err.truncate(0); err.seek(0)
assert not mv.to_tap(arp("192.0.2.50"), False) and err.getvalue() == ""
"#
    ));
}
