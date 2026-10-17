// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `tools/quux-taps`, held to what it says it does, without root.
//!
//! The script changes the system only as root, so a test cannot run it for
//! real. Its `--dry-run` prints every command it would run and the unit it
//! would write, and needs no root; these tests read that output. They hold:
//!
//! - the unit, line for line;
//! - the taps, `quux0` to `quux<N-1>` and no more, each owned by the user
//!   named, each with its own address, and all with the same MAC address
//!   and MTU;
//! - the addresses: the host side of `quuxN` is 169.254.(100+N).1/30, held
//!   as literals for quux0, quux1 and quux63, and for every other tap by
//!   [`host`], a second copy of the rule;
//! - a host-only tap's list (QUUX's Ethernet contract, §4.3): IPv6 off,
//!   `arp_ignore` 2, `rp_filter` 1, `proxy_arp` 0 and `forwarding` 0, the
//!   MAC address and MTU, the address, before that tap comes up;
//! - a LAN tap's list, `quux0` under `--ip-address` (§4.3, §4.4), literally
//!   and in its order, and the stop's, after the pool's own;
//! - the LAN interface as `quux-taps` finds it, from the routes `ip` gives,
//!   which these tests give through a stand-in `ip` on the path;
//! - the defaults: two taps, all host-only;
//! - the refusals: without root, for arguments it cannot use, `--address`
//!   among them, a link-local `--ip-address`, anything muir refuses, a mixed
//!   setup, no LAN interface, and QUUX's address being the LAN interface's
//!   own;
//! - `--check`'s judging of its seven lines (§4.4), on recorded results,
//!   since the namespaces it judges need root.
//!
//! The commands are run by `sh`, which every Unix CI runner has, and read
//! `id`; the check's judging wants `python3`, and skips without it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SBIN: &str = "/usr/local/sbin/quux-taps";
const UNIT: &str = "/etc/systemd/system/quux-taps.service";
const RUN_FILE: &str = "/run/quux-taps.lan";

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tools/quux-taps")
}

/// The script run with `args`, with no `SUDO_USER` to pick the owner, and
/// with `fake` (a stand-in `ip`'s directory) first on the path if given.
fn quux_taps_with(args: &[&str], fake: Option<&Path>) -> Output {
    let mut c = Command::new("sh");
    c.arg(script()).args(args).env_remove("SUDO_USER");
    if let Some(dir) = fake {
        let path = std::env::var("PATH").unwrap_or_default();
        c.env("PATH", format!("{}:{path}", dir.display()));
    }
    c.output().expect("sh runs")
}

fn quux_taps(args: &[&str]) -> Output {
    quux_taps_with(args, None)
}

fn id(flag: &str) -> String {
    let o = Command::new("id").arg(flag).output().expect("id runs");
    String::from_utf8(o.stdout).expect("id prints text").trim().to_string()
}

/// The user running the tests, who owns the taps unless one is named.
fn me() -> String {
    id("-un")
}

/// A host's network as `ip` reports it, for a stand-in `ip`: its routes
/// (`ip -4 -o route show table main`), its addresses (`ip -4 -o address
/// show`) and its proxy entries (`ip neigh show proxy`). Any other use of
/// `ip` fails the run, so a dry run that ran a command would be seen.
struct Network {
    dir: PathBuf,
}

impl Network {
    fn new(name: &str, routes: &str, addresses: &str, proxy: &str) -> Network {
        let dir =
            std::env::temp_dir().join(format!("muir-quux-taps-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("routes"), routes).unwrap();
        std::fs::write(dir.join("addresses"), addresses).unwrap();
        std::fs::write(dir.join("proxy"), proxy).unwrap();
        std::fs::write(dir.join("neigh"), "").unwrap();
        let d = dir.display();
        let ip = format!(
            "#!/bin/sh\n\
             case \"$*\" in\n\
             '-4 -o route show table main') cat '{d}/routes' ;;\n\
             '-4 -o address show') cat '{d}/addresses' ;;\n\
             'neigh show proxy') cat '{d}/proxy' ;;\n\
             'neigh show to '*) cat '{d}/neigh' ;;\n\
             *) echo \"stand-in ip: not a question: $*\" >&2; exit 3 ;;\n\
             esac\n"
        );
        let path = dir.join("ip");
        std::fs::write(&path, ip).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        Network { dir }
    }

    /// A LAN of 192.0.2.0/24 on `eth0`, where the host is 192.0.2.10 and
    /// the router 192.0.2.1, beside two pool taps of its own.
    fn lan(name: &str) -> Network {
        Network::new(
            name,
            "default via 192.0.2.1 dev eth0 proto dhcp src 192.0.2.10 metric 100 \n\
             192.0.2.0/24 dev eth0 proto kernel scope link src 192.0.2.10 metric 100 \n\
             198.51.100.0/24 via 192.0.2.1 dev eth0 \n",
            "1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever preferred_lft forever\n\
             2: eth0    inet 192.0.2.10/24 brd 192.0.2.255 scope global dynamic eth0\\       valid_lft 86000sec preferred_lft 86000sec\n",
            "",
        )
    }

    fn run(&self, args: &[&str]) -> Output {
        quux_taps_with(args, Some(&self.dir))
    }

    /// What `ip neigh show to Q dev L` answers, for every Q.
    fn neighbours(&self, text: &str) {
        std::fs::write(self.dir.join("neigh"), text).unwrap();
    }
}

impl Drop for Network {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// A dry run's output, split into what it does now (`now`), the unit's
/// lines (`unit`), what starting the unit runs (`up`) and what stopping it
/// runs (`stop`).
struct DryRun {
    now: Vec<String>,
    unit: Vec<String>,
    up: Vec<String>,
    stop: Vec<String>,
}

fn split(o: Output, what: &str) -> DryRun {
    assert!(o.status.success(), "quux-taps {what} failed: {}", String::from_utf8_lossy(&o.stderr));
    let text = String::from_utf8(o.stdout).expect("text");
    let mut d = DryRun { now: vec![], unit: vec![], up: vec![], stop: vec![] };
    for line in text.lines() {
        if let Some(l) = line.strip_prefix("| ") {
            d.unit.push(l.to_string());
        } else if line == "|" {
            d.unit.push(String::new());
        } else if let Some(l) = line.strip_prefix(">   ") {
            d.up.push(l.to_string());
        } else if let Some(l) = line.strip_prefix("<   ") {
            d.stop.push(l.to_string());
        } else {
            d.now.push(line.to_string());
        }
    }
    d
}

fn dry_run(args: &[&str]) -> DryRun {
    let mut all = vec!["--dry-run"];
    all.extend_from_slice(args);
    split(quux_taps(&all), &format!("{all:?}"))
}

fn dry_run_on(net: &Network, args: &[&str]) -> DryRun {
    let mut all = vec!["--dry-run"];
    all.extend_from_slice(args);
    split(net.run(&all), &format!("{all:?}"))
}

/// The host side of `quuxN`, address and prefix: 169.254.(100+N).1/30.
/// The script's rule, written again here so that the two are checked
/// against each other; `the_rule_by_its_literals` holds this copy.
fn host(n: usize) -> String {
    format!("169.254.{}.1/30", 100 + n)
}

/// The unit, as it must read for `count` taps for `user`, with quux0 a LAN
/// tap at `lan` = (QUUX's address and prefix, the LAN interface) if given.
fn unit(count: usize, user: &str, lan: Option<(&str, &str)>) -> Vec<String> {
    let taps = if count == 1 { "quux0".to_string() } else { format!("quux0 to quux{}", count - 1) };
    let mut v = vec![
        "# Written by quux-taps; `quux-taps --uninstall` removes it.".to_string(),
        "[Unit]".to_string(),
    ];
    let mut start = format!("ExecStart={SBIN} --up --count {count} --user {user}");
    match lan {
        None => v.push(format!("Description=QUUX tap pool: {taps}, owned by {user}")),
        Some((address, l)) => {
            v.push(format!(
                "Description=QUUX tap pool: {taps}, owned by {user}; quux0 a LAN tap on {l}"
            ));
            v.push(format!("Wants=sys-subsystem-net-devices-{l}.device"));
            v.push(format!("After=sys-subsystem-net-devices-{l}.device"));
            start += &format!(" --ip-address {address} --lan-interface {l}");
        }
    }
    v.extend([
        String::new(),
        "[Service]".to_string(),
        "Type=oneshot".to_string(),
        "RemainAfterExit=yes".to_string(),
        start,
        format!("ExecStop={SBIN} --down"),
        String::new(),
        "[Install]".to_string(),
        "WantedBy=multi-user.target".to_string(),
    ]);
    v
}

/// One host-only tap's settings, after it is made (§4.3, "A host-only
/// tap"): the board's `make_tap` list, with the same names.
fn host_only(k: usize) -> Vec<String> {
    vec![
        format!("sysctl -q -w net.ipv6.conf.quux{k}.disable_ipv6=1"),
        format!("sysctl -q -w net.ipv4.conf.quux{k}.arp_ignore=2"),
        format!("sysctl -q -w net.ipv4.conf.quux{k}.rp_filter=1"),
        format!("sysctl -q -w net.ipv4.conf.quux{k}.proxy_arp=0"),
        format!("sysctl -q -w net.ipv4.conf.quux{k}.forwarding=0"),
        format!("ip link set dev quux{k} down"),
        format!("ip link set dev quux{k} address 02:00:00:00:00:02 mtu 1500"),
        format!("ip address flush dev quux{k}"),
        format!("ip address add {} dev quux{k}", host(k)),
        format!("ip link set dev quux{k} up"),
    ]
}

/// What starting the unit runs for the host-only taps `first` to
/// `count - 1` for `user`: every tap made, udev settled, then each tap set
/// up and brought up.
fn pool(first: usize, count: usize, user: &str) -> Vec<String> {
    let mut v: Vec<String> =
        (first..count).map(|k| format!("ip tuntap add dev quux{k} mode tap user {user}")).collect();
    if first < count {
        v.push("udevadm settle".to_string());
    }
    for k in first..count {
        v.extend(host_only(k));
    }
    v
}

fn up(count: usize, user: &str) -> Vec<String> {
    pool(0, count, user)
}

/// **A LAN tap's list, literally** (§4.3, "A LAN tap, in this order"): T
/// made, IPv6 off and the six settings, T down with its MAC address, MTU
/// and no address, T up, the /32 route, L's two values kept, L's two
/// settings, and the one proxy entry last. Written out, not built from the
/// script's own words.
fn lan_list(user: &str, l: &str, run_file: &str) -> Vec<String> {
    [
        format!("ip tuntap add dev quux0 mode tap user {user}"),
        "udevadm settle".to_string(),
        "sysctl -q -w net.ipv6.conf.quux0.disable_ipv6=1".to_string(),
        "sysctl -q -w net.ipv4.conf.quux0.arp_ignore=0".to_string(),
        "sysctl -q -w net.ipv4.conf.quux0.arp_filter=1".to_string(),
        "sysctl -q -w net.ipv4.conf.quux0.rp_filter=1".to_string(),
        "sysctl -q -w net.ipv4.conf.quux0.proxy_arp=1".to_string(),
        "sysctl -q -w net.ipv4.conf.quux0.forwarding=1".to_string(),
        "sysctl -q -w net.ipv4.neigh.quux0.proxy_delay=0".to_string(),
        "ip link set dev quux0 down".to_string(),
        "ip link set dev quux0 address 02:00:00:00:00:02 mtu 1500".to_string(),
        "ip address flush dev quux0".to_string(),
        "ip link set dev quux0 up".to_string(),
        "ip route replace 192.0.2.50/32 dev quux0".to_string(),
        format!(
            "# write {run_file}: tap, lan, quux, and {l}'s forwarding and proxy_delay as they are"
        ),
        format!("sysctl -q -w net.ipv4.conf.{l}.forwarding=1"),
        format!("sysctl -q -w net.ipv4.neigh.{l}.proxy_delay=0"),
        format!("ip neigh add proxy 192.0.2.50 dev {l}"),
    ]
    .to_vec()
}

/// **The stop's list for a LAN tap** (§4.3, "The stop"): the proxy entry
/// removed, L's two values put back from the run file, the run file gone,
/// T down and deleted; then the pool's taps deleted.
fn lan_stop(l: &str, count: usize) -> Vec<String> {
    let mut v = vec![
        format!("ip neigh del proxy 192.0.2.50 dev {l}"),
        format!("sysctl -q -w net.ipv4.conf.{l}.forwarding=<from {RUN_FILE}>"),
        format!("sysctl -q -w net.ipv4.neigh.{l}.proxy_delay=<from {RUN_FILE}>"),
        format!("rm -f {RUN_FILE}"),
        "ip link set dev quux0 down".to_string(),
        "ip link delete dev quux0".to_string(),
    ];
    v.extend((1..count).map(|k| format!("ip link delete dev quux{k}")));
    v
}

/// What the install does now, around the unit: the check run, the old pool
/// stopped, the script copied, the unit written, loaded, enabled and
/// started, in that order.
fn check_now(d: &DryRun) {
    let pos = |what: &str| {
        d.now
            .iter()
            .position(|l| l == what || l.ends_with(what))
            .unwrap_or_else(|| panic!("the dry run does not say `{what}`:\n{}", d.now.join("\n")))
    };
    let check = pos("tools/quux-taps --check");
    assert!(d.now[check].starts_with("sh /"), "{}", d.now[check]);
    // Both files are made beside their places before the old pool stops,
    // and renamed into place after it, so that nothing a failure can leave
    // is half a file.
    let copy = pos(&format!(" {SBIN}.new"));
    assert!(d.now[copy].starts_with("install -D -m 0755 "), "{}", d.now[copy]);
    let write = pos(&format!("# write {UNIT}.new, mode 0644:"));
    let stop = pos("systemctl stop quux-taps.service");
    let place = pos(&format!("mv -f {SBIN}.new {SBIN}"));
    let place_unit = pos(&format!("mv -f {UNIT}.new {UNIT}"));
    let reload = pos("systemctl daemon-reload");
    let enable = pos("systemctl enable --now quux-taps.service");
    assert!(
        check < copy
            && copy < write
            && write < stop
            && stop < place
            && place < place_unit
            && place_unit < reload
            && reload < enable,
        "{:#?}",
        d.now
    );
}

/// Every tap's IPv6 is turned off, and its `arp_ignore` and `rp_filter`
/// set, before that tap comes up: Linux starts IPv6 on a link when it comes up, so the
/// order is the point. Held for any count, apart from the exact lists.
fn check_order(up: &[String], count: usize) {
    for k in 0..count {
        let at = |what: String| {
            up.iter().position(|l| *l == what).unwrap_or_else(|| panic!("no `{what}`"))
        };
        let ipv6 = at(format!("sysctl -q -w net.ipv6.conf.quux{k}.disable_ipv6=1"));
        let arp = at(format!("sysctl -q -w net.ipv4.conf.quux{k}.arp_ignore=2"));
        let rp = at(format!("sysctl -q -w net.ipv4.conf.quux{k}.rp_filter=1"));
        let made = at(format!("ip tuntap add dev quux{k} mode tap user {}", me()));
        let comes_up = at(format!("ip link set dev quux{k} up"));
        assert!(
            made < ipv6 && ipv6 < comes_up && arp < comes_up && rp < comes_up,
            "quux{k}: {up:#?}"
        );
    }
    let made = up.iter().filter(|l| l.starts_with("ip tuntap add ")).count();
    assert_eq!(made, count, "taps made");
    assert!(
        !up.iter().any(|l| l.contains(&format!("quux{count} "))),
        "a tap past quux{}",
        count - 1
    );
    // Each tap its own address: no two taps share one.
    let mut addresses: Vec<&str> = up
        .iter()
        .filter_map(|l| l.strip_prefix("ip address add "))
        .map(|l| l.split(' ').next().unwrap_or(""))
        .collect();
    assert_eq!(addresses.len(), count, "addresses given: {up:#?}");
    addresses.sort_unstable();
    addresses.dedup();
    assert_eq!(addresses.len(), count, "taps sharing an address: {up:#?}");
}

/// The line that gives `quuxN` its address, as the dry run prints it.
fn address_line(up: &[String], n: usize) -> &str {
    up.iter()
        .find(|l| l.starts_with("ip address add ") && l.ends_with(&format!(" dev quux{n}")))
        .unwrap_or_else(|| panic!("no address for quux{n}: {up:#?}"))
}

/// The helper's copy of the rule, held to the addresses written out.
#[test]
fn the_rule_by_its_literals() {
    assert_eq!(host(0), "169.254.100.1/30");
    assert_eq!(host(1), "169.254.101.1/30");
    assert_eq!(host(63), "169.254.163.1/30");
}

/// **A host-only tap's list, literally** (§4.3): the board's list for a
/// host-only card, `proxy_arp` 0 and `forwarding` 0 said explicitly, T down
/// and its addresses flushed before its own goes on. **Fails** a list
/// without the two zeros, or in another order.
#[test]
fn a_host_only_tap_s_list_literally() {
    let d = dry_run(&["--count", "1"]);
    assert_eq!(
        d.up,
        [
            format!("ip tuntap add dev quux0 mode tap user {}", me()),
            "udevadm settle".to_string(),
            "sysctl -q -w net.ipv6.conf.quux0.disable_ipv6=1".to_string(),
            "sysctl -q -w net.ipv4.conf.quux0.arp_ignore=2".to_string(),
            "sysctl -q -w net.ipv4.conf.quux0.rp_filter=1".to_string(),
            "sysctl -q -w net.ipv4.conf.quux0.proxy_arp=0".to_string(),
            "sysctl -q -w net.ipv4.conf.quux0.forwarding=0".to_string(),
            "ip link set dev quux0 down".to_string(),
            "ip link set dev quux0 address 02:00:00:00:00:02 mtu 1500".to_string(),
            "ip address flush dev quux0".to_string(),
            "ip address add 169.254.100.1/30 dev quux0".to_string(),
            "ip link set dev quux0 up".to_string(),
        ]
    );
    assert_eq!(d.stop, ["ip link delete dev quux0"]);
}

#[test]
fn a_pool_of_one() {
    let me = me();
    let d = dry_run(&["--count", "1"]);
    assert_eq!(d.unit, unit(1, &me, None));
    assert_eq!(d.up, up(1, &me));
    assert_eq!(address_line(&d.up, 0), "ip address add 169.254.100.1/30 dev quux0");
    check_order(&d.up, 1);
    check_now(&d);
}

#[test]
fn a_pool_of_thirty_two_as_the_build_host_has() {
    let me = me();
    let d = dry_run(&["--count", "32", "--user", &me]);
    assert_eq!(d.unit, unit(32, &me, None));
    assert_eq!(d.up, up(32, &me));
    let stop: Vec<String> = (0..32).map(|k| format!("ip link delete dev quux{k}")).collect();
    assert_eq!(d.stop, stop);
    check_order(&d.up, 32);
    check_now(&d);
}

#[test]
fn a_pool_of_sixty_four_the_most() {
    let me = me();
    let d = dry_run(&["--count", "64", "--user", &me]);
    assert_eq!(d.unit, unit(64, &me, None));
    assert_eq!(d.up, up(64, &me));
    check_order(&d.up, 64);
    check_now(&d);
    assert_eq!(address_line(&d.up, 0), "ip address add 169.254.100.1/30 dev quux0");
    assert_eq!(address_line(&d.up, 1), "ip address add 169.254.101.1/30 dev quux1");
    assert_eq!(address_line(&d.up, 63), "ip address add 169.254.163.1/30 dev quux63");
}

#[test]
fn the_defaults_are_two_taps() {
    let me = me();
    let d = dry_run(&[]);
    assert_eq!(d.unit, unit(2, &me, None));
    assert_eq!(d.up, up(2, &me));
    check_order(&d.up, 2);
    assert_eq!(address_line(&d.up, 0), "ip address add 169.254.100.1/30 dev quux0");
    assert_eq!(address_line(&d.up, 1), "ip address add 169.254.101.1/30 dev quux1");
}

/// **`--ip-address` makes quux0 a LAN tap, and quux1 up stay the pool's**
/// (§3, §4.3, §4.4): the LAN interface found from the routes, the one
/// interface with a connected route holding QUUX's address at its prefix;
/// the unit ordered after that interface's device, with it in `--up`'s
/// arguments; the pool first, host-only from quux1, and the LAN tap after
/// it, by §4.3's list; the stop the LAN tap's undo, then the pool; the
/// check before anything. **Fails** a LAN tap made host-only, an address on
/// quux0, the proxy entry before the route, a setting dropped or moved, a
/// stop that forgets L's values, or quux0 made before the pool.
#[test]
fn a_lan_tap_on_quux0_and_the_pool_behind_it() {
    let me = me();
    let net = Network::lan("lan");
    let d = dry_run_on(&net, &["--count", "32", "--user", &me, "--ip-address", "192.0.2.50/24"]);
    assert_eq!(d.unit, unit(32, &me, Some(("192.0.2.50/24", "eth0"))));
    let mut want = pool(1, 32, &me);
    want.extend(lan_list(&me, "eth0", RUN_FILE));
    assert_eq!(d.up, want);
    assert_eq!(d.stop, lan_stop("eth0", 32));
    check_now(&d);
    let starts = d
        .now
        .iter()
        .find(|l| l.starts_with("# starting the unit runs "))
        .expect("the dry run says what starting runs");
    assert_eq!(
        *starts,
        format!(
            "# starting the unit runs {SBIN} --up --count 32 --user {me} --ip-address 192.0.2.50/24 --lan-interface eth0:"
        )
    );
    assert!(!d.up.iter().any(|l| l.starts_with("ip address add") && l.ends_with("dev quux0")));
    // One LAN tap of one: no pool behind it.
    let d = dry_run_on(&net, &["--count", "1", "--ip-address", "192.0.2.50/24"]);
    assert_eq!(d.up, lan_list(&me, "eth0", RUN_FILE));
    assert_eq!(d.unit, unit(1, &me, Some(("192.0.2.50/24", "eth0"))));
}

/// **A proxy entry an earlier start left on L, for another address, goes
/// before the new one is added** (amendment 3, §3.3, M5). Its own address's
/// entry is not removed. **Fails** a start that leaves the old entry live.
#[test]
fn an_old_proxy_entry_on_the_lan_interface_goes() {
    let me = me();
    let lan = Network::lan("old");
    let net = Network::new(
        "old-entry",
        &std::fs::read_to_string(lan.dir.join("routes")).unwrap(),
        &std::fs::read_to_string(lan.dir.join("addresses")).unwrap(),
        "192.0.2.77 dev eth0 proxy \n192.0.2.50 dev eth0 proxy \n192.0.2.78 dev eth1 proxy \n",
    );
    let d = dry_run_on(&net, &["--count", "1", "--ip-address", "192.0.2.50/24"]);
    let mut want = lan_list(&me, "eth0", RUN_FILE);
    let add = want.len() - 1;
    want.insert(add, "ip neigh del proxy 192.0.2.77 dev eth0".to_string());
    assert_eq!(d.up, want);
}

/// **The check sets its stand-in host up with the same code as `--up`**
/// (§4.4): `--step`'s lists, with the namespace's names, are `--up`'s.
/// **Fails** a check whose host side is written a second time.
#[test]
fn the_check_s_host_side_is_up_s_own_code() {
    let me = me();
    let d = dry_run(&["--step", "lan-up", "quux0", "lan0", "192.0.2.50", "24", "/x/check.lan"]);
    assert_eq!(d.now, lan_list(&me, "lan0", "/x/check.lan"));
    let d = dry_run(&["--step", "host-only", "quux1", "1"]);
    assert_eq!(d.now, host_only(1));
    // The undo, from a run file as the start writes it.
    let dir = std::env::temp_dir().join(format!("muir-quux-taps-{}-step", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("check.lan");
    std::fs::write(&f, "tap quux0\nlan lan0\nquux 192.0.2.50\nforwarding 0\nproxy_delay 80\n")
        .unwrap();
    let fs = f.to_str().unwrap();
    let d = dry_run(&["--step", "lan-down", fs]);
    assert_eq!(
        d.now,
        [
            "ip neigh del proxy 192.0.2.50 dev lan0".to_string(),
            "sysctl -q -w net.ipv4.conf.lan0.forwarding=0".to_string(),
            "sysctl -q -w net.ipv4.neigh.lan0.proxy_delay=80".to_string(),
            format!("rm -f {fs}"),
            "ip link set dev quux0 down".to_string(),
            "ip link delete dev quux0".to_string(),
        ]
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn uninstall_removes_what_install_wrote() {
    let d = dry_run(&["--uninstall"]);
    let pos = |what: String| {
        d.now.iter().position(|l| *l == what).unwrap_or_else(|| panic!("no `{what}`: {:#?}", d.now))
    };
    let stop = pos("systemctl disable --now quux-taps.service".into());
    let unit = pos(format!("rm -f {UNIT}"));
    let reload = pos("systemctl daemon-reload".into());
    let sbin = pos(format!("rm -f {SBIN}"));
    assert!(stop < unit && unit < reload, "{:#?}", d.now);
    assert!(stop < sbin);
    assert!(d.unit.is_empty() && d.up.is_empty(), "uninstall writes nothing");
}

/// Without root and without `--dry-run`, every mode refuses, prints
/// nothing to stdout, and says why.
#[test]
fn refuses_without_root() {
    if id("-u") == "0" {
        eprintln!("skipped: the tests run as root, so the refusal cannot be seen");
        return;
    }
    let me = me();
    let net = Network::lan("root");
    for args in [
        vec![],
        vec!["--count", "32", "--user", &me],
        vec!["--count", "32", "--user", &me, "--ip-address", "192.0.2.50/24"],
        vec!["--uninstall"],
        vec!["--up", "--count", "1", "--user", &me],
        vec!["--down"],
        vec!["--check"],
    ] {
        let o = net.run(&args);
        assert_eq!(o.status.code(), Some(1), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        assert!(o.stdout.is_empty(), "{args:?} printed {}", String::from_utf8_lossy(&o.stdout));
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains("must run as root"), "{args:?}: {err}");
    }
}

/// `args` refused with status 2, nothing on stdout, with and without
/// `--dry-run`, before the question of root arises, on `net`, the stderr
/// containing every one of `says`.
fn refused(net: &Network, args: &[&str], says: &[&str]) {
    for dry in [true, false] {
        let mut a = args.to_vec();
        if dry {
            a.push("--dry-run");
        }
        let o = net.run(&a);
        let err = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(2), "{a:?}: {err}");
        assert!(o.stdout.is_empty(), "{a:?} printed {}", String::from_utf8_lossy(&o.stdout));
        for s in says {
            assert!(err.contains(s), "{a:?}: wants {s:?}: {err}");
        }
    }
}

/// Arguments it cannot use are refused with status 2 and nothing done,
/// before the question of root arises.
#[test]
fn refuses_arguments_it_cannot_use() {
    let net = Network::lan("args");
    for args in [
        &["--count", "0"][..],
        &["--count", "65"],
        &["--count", "08"],
        &["--count", "two"],
        &["--user", "no-such-user-for-quux-taps"],
        &["--bogus", "x"],
        &["--ip-address"],
        &["--lan-interface", "eth0"],
        &["--uninstall", "--ip-address", "192.0.2.50/24"],
        &["--check", "--ip-address", "192.0.2.50/24"],
        &["--up", "--ip-address", "192.0.2.50/24"],
    ] {
        refused(&net, args, &[]);
        if args[0] != "--bogus" {
            let err = String::from_utf8_lossy(&net.run(args).stderr).into_owned();
            assert!(!err.contains("unknown argument"), "{args:?}: {err}");
        }
    }
    let o = net.run(&["--check", "--dry-run"]);
    assert_eq!(o.status.code(), Some(2), "{}", String::from_utf8_lossy(&o.stderr));
}

/// `--address` is gone: given, with a value or without, it is refused with
/// status 2, nothing done, and a message that states the rule.
#[test]
fn refuses_address() {
    for args in [
        vec!["--address", "169.254.100.1/30"],
        vec!["--address"],
        vec!["--count", "1", "--address", "169.254.100.1/30", "--dry-run"],
        vec!["--up", "--address", "169.254.100.1/30"],
    ] {
        let o = quux_taps(&args);
        let err = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(2), "{args:?}: {err}");
        assert!(o.stdout.is_empty(), "{args:?}");
        assert!(err.contains("169.254.1NN.1/30") && err.contains("two digits"), "{args:?}: {err}");
    }
}

/// **A link-local `--ip-address` is refused** (§4.2): an address in
/// 169.254.x.y is a host-only tap's, and every pool tap is one already.
/// **Fails** a quux0 made a LAN tap for a link-local address.
#[test]
fn refuses_a_link_local_ip_address() {
    let net = Network::lan("link-local");
    for a in ["169.254.100.2/30", "169.254.1.2/30", "169.254.163.1/24"] {
        refused(&net, &["--ip-address", a], &["--ip-address", "link-local", "host-only"]);
    }
}

/// **What muir refuses, `quux-taps` refuses** (§3, "Flags"; muir-sim
/// `src/ethernet_socket.rs`, `parse_ip_address`): not A.B.C.D/N, a part
/// with a leading zero or past 255, a prefix outside 1 to 30, an address in
/// 0.x.x.x or 127.x.x.x or from 224.0.0.0 up, and the subnet's network or
/// broadcast address. **Fails** a LAN tap made for an address muir would
/// refuse to run with.
#[test]
fn refuses_what_muir_refuses() {
    let net = Network::lan("muir");
    let wants = "--ip-address wants A.B.C.D/N";
    let prefix = "a prefix length of";
    let not_a_host = "not a host's address";
    for (a, why) in [
        ("192.0.2.50", wants),
        ("192.0.2.50/", wants),
        ("192.0.2/24", wants),
        ("192.0.2.50.1/24", wants),
        ("192.0.02.50/24", wants),
        ("192.0.2.256/24", wants),
        ("192.0.2.50/024", wants),
        ("x/24", wants),
        ("192.0.2.50/0", prefix),
        ("192.0.2.50/31", prefix),
        ("192.0.2.50/32", prefix),
        ("0.1.2.3/8", "this network"),
        ("127.0.0.5/8", "loopback"),
        ("224.0.0.5/24", "multicast"),
        ("240.0.0.5/24", "reserved"),
        ("192.0.2.0/24", "the subnet's network address"),
        ("192.0.2.255/24", "the subnet's broadcast address"),
    ] {
        let says: &[&str] = if why == wants || why == prefix { &[why] } else { &[not_a_host, why] };
        refused(&net, &["--ip-address", a], says);
    }
}

/// **A mixed setup is refused** (§4.2): an address that is not link-local
/// whose subnet holds link-local addresses, so that QUUX's gateway or the
/// pool's own host sides could be on QUUX's subnet. **Fails** a LAN tap
/// whose subnet takes in the pool.
#[test]
fn refuses_a_mixed_setup() {
    let net = Network::lan("mixed");
    for a in ["169.0.0.5/8", "128.0.0.5/1", "168.0.0.5/6", "169.255.0.5/15"] {
        refused(&net, &["--ip-address", a], &["--ip-address", "link-local"]);
    }
}

/// **No LAN interface, and QUUX's address being its own, are refused at
/// install** (§4.4): no interface that is not a pool tap has a connected
/// route holding the address at its prefix (another prefix, a route through
/// a gateway, a pool tap's own route, nothing at all), or two have; or the
/// address is the LAN interface's own. **Fails** an install that guesses
/// an interface, takes a pool tap for the LAN, or holds the host's own
/// address for QUUX.
#[test]
fn refuses_without_one_lan_interface_or_at_its_own_address() {
    let lan = Network::lan("own");
    refused(&lan, &["--ip-address", "192.0.2.10/24"], &["192.0.2.10", "eth0", "own"]);
    refused(&lan, &["--ip-address", "192.0.2.50/25"], &["no interface", "192.0.2.50/25"]);
    refused(&lan, &["--ip-address", "198.51.100.5/24"], &["no interface"]);
    let pool_only = Network::new(
        "pool-only",
        "192.0.2.0/24 dev quux3 proto kernel scope link src 192.0.2.10 \n\
         192.0.2.0/24 dev lo scope link \n",
        "",
        "",
    );
    refused(&pool_only, &["--ip-address", "192.0.2.50/24"], &["no interface"]);
    let two = Network::new(
        "two",
        "192.0.2.0/24 dev eth0 proto kernel scope link src 192.0.2.10 \n\
         192.0.2.0/24 dev eth1 proto kernel scope link src 192.0.2.11 \n",
        "",
        "",
    );
    refused(&two, &["--ip-address", "192.0.2.50/24"], &["eth0", "eth1"]);
    let none = Network::new("none", "", "", "");
    refused(&none, &["--ip-address", "192.0.2.50/24"], &["no interface"]);
    // A pool tap's own connected route beside L's is passed over.
    let beside = Network::new(
        "beside",
        "192.0.2.0/24 dev quux3 proto kernel scope link src 192.0.2.10 \n\
         192.0.2.0/24 dev eth0 proto kernel scope link src 192.0.2.10 \n",
        "",
        "",
    );
    let d = dry_run_on(&beside, &["--count", "1", "--ip-address", "192.0.2.50/24"]);
    assert_eq!(d.up, lan_list(&me(), "eth0", RUN_FILE));
}

// --- the check's judging --------------------------------------------------

/// The check's program, as `--check` runs it: the text between its
/// heredoc's markers in the script.
fn check_program() -> String {
    let text = std::fs::read_to_string(script()).unwrap();
    let start = text.find("<<'CHECK_PROGRAM'\n").expect("the script holds the check's program");
    let body = &text[start + "<<'CHECK_PROGRAM'\n".len()..];
    let end = body.find("\nCHECK_PROGRAM\n").expect("the program's end marker");
    body[..end + 1].to_string()
}

/// `check`, after the check's program is loaded as the module `c`, in
/// `python3 -I -B`; skipped without python3.
fn judged(check: &str) {
    static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("muir-quux-taps-{}-judge-{call}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let prog = dir.join("check.py");
    std::fs::write(&prog, check_program()).unwrap();
    let code = format!(
        "import importlib.machinery, importlib.util, sys\n\
         loader = importlib.machinery.SourceFileLoader('check', sys.argv[1])\n\
         spec = importlib.util.spec_from_loader('check', loader)\n\
         c = importlib.util.module_from_spec(spec)\n\
         loader.exec_module(c)\n\
         {check}\nprint('checked')\n"
    );
    let out = match Command::new("python3").args(["-I", "-B", "-c"]).arg(code).arg(&prog).output() {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipped: no python3 to run the check's program");
            return;
        }
        Err(e) => panic!("python3: {e}"),
    };
    std::fs::remove_dir_all(&dir).ok();
    let (o, e) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && o.contains("checked"), "stdout:\n{o}\nstderr:\n{e}");
}

/// What every judging check starts from: the results a correct host side
/// gives, as the check records them, and a helper that says which lines a
/// change to them fails.
const RESULTS: &str = r#"
import copy
LAN_MAC, L2_MAC = "0a:00:00:00:00:10", "0a:00:00:00:00:30"

def good():
    settings = dict(c.EXPECTED)
    start = {k: "0" for k in c.UNDONE}
    start["net.ipv4.neigh.lan0.proxy_delay"] = "80"
    baseline = {t: [] for t in c.ASKED}
    baseline[c.Q2_GW] = [LAN_MAC]
    baseline[c.L2_IP] = [L2_MAC]
    baseline[c.ROUTER] = [L2_MAC]
    after = copy.deepcopy(baseline)
    after[c.Q_IP] = [LAN_MAC]
    return {
        "lan_mac": LAN_MAC, "l2_mac": L2_MAC,
        "start": start,
        "settings": {"sysctl": settings, "quux0 addresses": [],
                     "routes": ["192.0.2.50 dev quux0", "192.0.2.0/24 dev lan0"],
                     "proxy": ["192.0.2.50 dev lan0"]},
        "baseline": baseline, "after": after,
        "latency": [3.0] * 10 + [2.0] * 10,
        "reach": {"L1 to Q": [100, 100], "Q to L1": [100, 100], "Q to the router": [100, 100],
                  "Q to H": [100, 100], "H to Q": [100, 100], "H's source": "192.0.2.10",
                  "banner": c.BANNER, "echo": True},
        "q_frames": [{"type": 0x0806, "src": c.HOST_MAC, "dst": c.QUUX_MAC, "ip_dst": None},
                     {"type": 0x0800, "src": c.HOST_MAC, "dst": c.QUUX_MAC, "ip_dst": c.Q_IP}],
        "l1_frames": [{"type": 0x0800, "src": LAN_MAC, "dst": "0a:00:00:00:00:20", "ip_dst": c.L1_IP}],
        "host_only": 0,
        "stopped": {"sysctl": dict(start), "proxy": [], "arp": copy.deepcopy(baseline)},
    }

def failing(r):
    return [n for n, (name, why) in enumerate(c.judge(r), 1) if why]

assert failing(good()) == [], c.judge(good())
"#;

/// **The seven lines pass on a correct host side's results, and each
/// partial job of §4.4 fails the lines it names**: global forwarding fails
/// 1 and 6; `proxy_arp` on lan0 fails 1 and 2; no proxy entry fails 1 and
/// 2; no `proxy_arp` on quux0 fails 4; `arp_ignore` 2 left on quux0 fails
/// 4; the proxy delay left at 0.8 s fails 3; an address on quux0 fails 1
/// and 2; forwarding on quux1 fails 1 and 6; no undo fails 7. The results
/// are what the kernel would give for each fault (amendment 3, §5.4: I,
/// from Linux's code); the real namespaces are `--check`'s, as root.
/// **Fails** a judge that passes any of them.
#[test]
fn the_check_s_lines_judge_each_partial_job() {
    judged(&format!(
        "{RESULTS}\n{}",
        r#"
def case(change):
    r = good()
    change(r)
    return failing(r)

def global_forwarding(r):
    r["settings"]["sysctl"]["net.ipv4.conf.all.forwarding"] = "1"
    r["host_only"] = 5
assert case(global_forwarding) == [1, 6], case(global_forwarding)

def proxy_arp_on_lan0(r):
    r["settings"]["sysctl"]["net.ipv4.conf.lan0.proxy_arp"] = "1"
    r["after"][c.PLANTED] = [LAN_MAC]
    r["after"][c.Q2_IP] = [LAN_MAC]
assert case(proxy_arp_on_lan0) == [1, 2]

def no_proxy_entry(r):
    r["settings"]["proxy"] = []
    r["after"][c.Q_IP] = []
assert case(no_proxy_entry) == [1, 2]

def no_proxy_arp_on_quux0(r):
    r["reach"]["Q to L1"] = [100, 0]
    r["reach"]["Q to the router"] = [100, 0]
    r["latency"] = [3.0] * 10 + [None] * 10
assert 4 in case(no_proxy_arp_on_quux0)

def arp_ignore_2_on_quux0(r):
    r["reach"]["Q to H"] = [100, 0]
    r["reach"]["H to Q"] = [100, 0]
assert case(arp_ignore_2_on_quux0) == [4]

def slow_proxy(r):
    r["latency"] = [3.0, 412.0, 790.0] + [3.0] * 17
assert case(slow_proxy) == [3]

def address_on_quux0(r):
    r["settings"]["quux0 addresses"] = ["192.0.2.51/24"]
    r["after"][c.ASKED[2]] = [LAN_MAC]
assert case(address_on_quux0) == [1, 2]

def forwarding_on_quux1(r):
    r["settings"]["sysctl"]["net.ipv4.conf.quux1.forwarding"] = "1"
    r["host_only"] = 5
assert case(forwarding_on_quux1) == [1, 6]

def no_undo(r):
    r["stopped"] = {"sysctl": dict(r["settings"]["sysctl"]), "proxy": ["192.0.2.50 dev lan0"],
                    "arp": copy.deepcopy(r["after"])}
assert case(no_undo) == [7]

# The isolation line: a LAN broadcast, an IPv6 frame or ARP from the LAN
# reaching QUUX, or QUUX's MAC address reaching the LAN.
for frame in [{"type": 0x0806, "src": "0a:00:00:00:00:20", "dst": "ff:ff:ff:ff:ff:ff", "ip_dst": None},
              {"type": 0x0800, "src": c.HOST_MAC, "dst": "ff:ff:ff:ff:ff:ff", "ip_dst": "192.0.2.255"},
              {"type": 0x0800, "src": c.HOST_MAC, "dst": c.QUUX_MAC, "ip_dst": "255.255.255.255"},
              {"type": 0x86DD, "src": c.HOST_MAC, "dst": "33:33:00:00:00:01", "ip_dst": None},
              {"type": 0x0804, "src": "0a:00:00:00:00:20", "dst": "ff:ff:ff:ff:ff:ff", "ip_dst": None}]:
    assert case(lambda r: r["q_frames"].append(frame)) == [5], frame
assert case(lambda r: r["l1_frames"].append(
    {"type": 0x0800, "src": c.QUUX_MAC, "dst": "0a:00:00:00:00:20", "ip_dst": c.L1_IP})) == [5]

# L2's addresses answered by anyone else, or the host's own pool sides
# answered differently after: line 2.
assert case(lambda r: r["after"].__setitem__(c.ROUTER, [L2_MAC, LAN_MAC])) == [2]
assert case(lambda r: r["after"].__setitem__(c.Q2_GW, [])) == [2]
assert case(lambda r: r["baseline"].__setitem__(c.Q_IP, [LAN_MAC])) == [2, 7]

# The reach line wants all of it: the banner, the echo, H's source.
assert case(lambda r: r["reach"].__setitem__("echo", False)) == [4]
assert case(lambda r: r["reach"].__setitem__("H's source", "192.0.2.51")) == [4]
assert case(lambda r: r["reach"].__setitem__("L1 to Q", [100, 99])) == [4]

# A result the check could not get is a fail, not a pass.
r = good(); del r["latency"]
assert failing(r) == [3], c.judge(r)
r = good(); r["settings"] = None
assert failing(r) == [1], c.judge(r)
"#
    ));
}

/// **The check's report: seven lines, each a pass or a fail, in §4.4's
/// order and words, and the last line the verdict.** **Fails** a report
/// that hides a failed line or says pass when one failed.
#[test]
fn the_check_reports_seven_lines_and_a_verdict() {
    judged(&format!(
        "{RESULTS}\n{}",
        r#"
lines, ok = c.report(c.judge(good()))
assert ok and len(lines) == 8, lines
names = ["the settings", "the ARP difference", "latency", "reach", "isolation",
         "a host-only tap stays host-only", "the stop"]
for n, (line, name) in enumerate(zip(lines, names), 1):
    assert line == f"line {n}, {name}: pass", line
assert lines[7] == "quux-taps --check: passed", lines[7]
r = good(); r["latency"][4] = 51.0
lines, ok = c.report(c.judge(r))
assert not ok
assert lines[2].startswith("line 3, latency: FAIL: "), lines[2]
assert "51" in lines[2], lines[2]
assert lines[7] == "quux-taps --check: FAILED (line 3)", lines[7]
"#
    ));
}

/// **The check's frames and ARP are built and read as Linux sends them**:
/// an ARP request carries the asker's MAC and IPv4 addresses and the
/// target; a reply is read back for its sender's two; an Ethernet frame is
/// read for its type, its two MAC addresses and an IPv4 destination.
/// **Fails** an ask or a reading that swaps a field.
#[test]
fn the_check_s_frames_are_read_as_linux_sends_them() {
    judged(
        r#"
import socket, struct
req = c.arp_request("0a:00:00:00:00:20", "192.0.2.20", "192.0.2.50")
assert len(req) == 42
assert req[:6] == b"\xff" * 6 and req[6:12] == bytes.fromhex("0a0000000020")
assert req[12:14] == b"\x08\x06"
assert struct.unpack("!HHBBH", req[14:22]) == (1, 0x0800, 6, 4, 1)
assert req[22:28] == bytes.fromhex("0a0000000020") and req[28:32] == socket.inet_aton("192.0.2.20")
assert req[32:38] == bytes(6) and req[38:42] == socket.inet_aton("192.0.2.50")
reply = (bytes.fromhex("0a0000000020") + bytes.fromhex("0a0000000010") + b"\x08\x06"
         + struct.pack("!HHBBH", 1, 0x0800, 6, 4, 2) + bytes.fromhex("0a0000000010")
         + socket.inet_aton("192.0.2.50") + bytes.fromhex("0a0000000020") + socket.inet_aton("192.0.2.20"))
assert c.arp_reply(reply) == ("192.0.2.50", "0a:00:00:00:00:10"), c.arp_reply(reply)
assert c.arp_reply(req) is None
ipv4 = (bytes.fromhex("020000000001") + bytes.fromhex("020000000002") + b"\x08\x00"
        + bytes([0x45, 0]) + struct.pack("!H", 20) + bytes(8) + socket.inet_aton("192.0.2.20")
        + socket.inet_aton("192.0.2.50"))
f = c.frame(ipv4)
assert f == {"type": 0x0800, "dst": "02:00:00:00:00:01", "src": "02:00:00:00:00:02", "ip_dst": "192.0.2.50"}, f
assert c.frame(req)["ip_dst"] is None and c.frame(req)["type"] == 0x0806
assert c.ping_counts("100 packets transmitted, 97 received, 3% packet loss, time 1010ms") == [100, 97]
assert c.ping_counts("5 packets transmitted, 0 received, +5 errors, 100% packet loss") == [5, 0]
assert c.ping_counts("") is None
"#,
    );
}

/// **The check touches none of the host's own interfaces** (§4.4): every
/// command it runs is `ip netns add` or `ip netns del` for its own
/// namespaces, or runs inside one of them by `ip netns exec`; a command
/// for the host's own namespace is refused before it runs. Its stand-in
/// host is set up by `quux-taps --step`, inside the namespace. Recorded
/// with `subprocess.run` replaced, so nothing runs. **Fails** a check that
/// would make a link, an address or a setting on the host itself.
#[test]
fn the_check_touches_no_interface_of_the_host() {
    judged(
        r#"
import subprocess
ran = []
class Done:
    returncode, stdout, stderr = 0, "", ""
def record(cmd, **kw):
    ran.append(list(cmd))
    return Done()
subprocess.run = record
k = c.Check("/x/tools/quux-taps", "/x/work", None)
k.setup()
k.step("lan-up", "quux0", "lan0", c.Q_IP, "24", "/x/work/check.lan")
k.ping("l1", c.Q_IP)
k.made = list(k.ns.values())
k.close()
names = set(k.ns.values())
assert len(names) == 6 and all(n.startswith("qtc") for n in names), names
for cmd in ran:
    assert cmd[:2] == ["ip", "netns"], cmd
    assert cmd[2] in ("add", "del", "exec") and cmd[3] in names, cmd
assert ["ip", "netns", "exec", k.ns["h"], "sh", "/x/tools/quux-taps", "--step", "lan-up",
        "quux0", "lan0", c.Q_IP, "24", "/x/work/check.lan"] in ran
assert sum(1 for cmd in ran if cmd[2] == "add") == 6 and sum(1 for cmd in ran if cmd[2] == "del") == 6
try:
    k.sh("ip", "link", "add", "x", "type", "dummy")
    raise AssertionError("a command for the host's own namespace ran")
except AssertionError as e:
    assert "ran" not in str(e), e
"#,
    );
}

/// **The check's order is §4.4's**: the start's values read, the host-only
/// tap set up, the baseline asked, the LAN tap set up, its settings read,
/// the /32 planted, the asks again; the stop last, then its settings and
/// the asks once more. Recorded with the commands stood in for, so nothing
/// runs. **Fails** a baseline taken before the host-only tap or after the
/// LAN tap, or a stop before the lines it undoes.
#[test]
fn the_check_runs_its_steps_in_order() {
    judged(
        r#"
order = []
k = c.Check("/x/tools/quux-taps", "/x/work", None)
class P:
    returncode, stdout, stderr = 0, "192.0.2.50 dev quux0 src 192.0.2.10", ""
def sh(*argv, ns=None, check=True, timeout=0):
    if argv[0] == "sh":
        order.append("step " + argv[3])
    elif "route" in argv and "add" in argv:
        order.append("route " + argv[3])
    return P()
def role(ns, name, *args, timeout=0):
    order.append(name)
    if name == "settings":
        return {"sysctl": {}, "proxy": [], "routes": [], "quux0 addresses": []}
    return {"arp-latency": [], "echo": {"banner": c.BANNER, "echo": True}}.get(name, {})
k.sh, k.role = sh, role
k.setup = lambda: order.append("setup")
k.start = lambda ns, *a: order.append("start " + a[0]) or a[0]
k.finish = lambda p: order.append("finish " + p) or []
k.ping = lambda ns, to, count=100: [count, count]
k.mac = lambda ns, dev: "0a:00:00:00:00:10"
r = k.run()
assert not r["errors"], r["errors"]
want = ["setup", "settings", "step host-only", "arp-ask", "step lan-up", "settings",
        "route 198.51.100.7/32", "arp-ask", "arp-latency", "arp-latency"]
assert order[:len(want)] == want, order
tail = ["step lan-down", "settings", "arp-ask"]
assert order[-3:] == tail, order
assert order.index("route 169.254.101.2/32") < order.index("step lan-down")
assert order.index("finish sniff") < order.index("step lan-down")
"#,
    );
}

/// **QUUX's address may be nobody else's on the LAN** (§4.3, step 1;
/// amendment 3, M6): the LAN's router, as the default route's gateway or
/// any other route's, is refused, and so is an address the neighbour table
/// shows another machine answering for on L (reachable, stale, delayed,
/// probed or permanent); an entry that failed, or none, is not one. The
/// host would otherwise answer ARP on the LAN for the router or for that
/// machine, and take their traffic. **Fails** an install that puts a proxy
/// entry on for an address in use.
#[test]
fn refuses_an_address_the_lan_already_uses() {
    let lan = Network::lan("in-use");
    refused(&lan, &["--ip-address", "192.0.2.1/24"], &["192.0.2.1", "router"]);
    let via = Network::new(
        "via",
        "default via 192.0.2.1 dev eth0 \n\
         192.0.2.0/24 dev eth0 proto kernel scope link src 192.0.2.10 \n\
         203.0.113.0/24 via 192.0.2.9 dev eth0 \n",
        "",
        "",
    );
    refused(&via, &["--ip-address", "192.0.2.9/24"], &["192.0.2.9", "router"]);
    for state in ["REACHABLE", "STALE", "DELAY", "PROBE", "PERMANENT"] {
        lan.neighbours(&format!("192.0.2.60 lladdr 0a:00:00:00:00:60 {state} \n"));
        refused(&lan, &["--ip-address", "192.0.2.60/24"], &["192.0.2.60", "eth0", "answers"]);
    }
    for state in ["FAILED", "INCOMPLETE"] {
        lan.neighbours(&format!("192.0.2.60 {state} \n"));
        let d = dry_run_on(&lan, &["--count", "1", "--ip-address", "192.0.2.60/24"]);
        assert!(d.up.contains(&"ip neigh add proxy 192.0.2.60 dev eth0".to_string()), "{state}");
    }
}

/// **The unit names L's device as systemd does** (`systemd-escape --path
/// --suffix=device`): an interface named `br-lan` is the device
/// `sys-subsystem-net-devices-br\x2dlan.device`; unescaped, systemd reads
/// `br/lan` and waits its whole device timeout at every boot for a device
/// that never comes. **Fails** a unit that writes the name raw.
#[test]
fn the_unit_escapes_the_lan_interface_s_device_name() {
    let net = Network::new(
        "br-lan",
        "default via 192.0.2.1 dev br-lan \n\
         192.0.2.0/24 dev br-lan proto kernel scope link src 192.0.2.10 \n",
        "",
        "",
    );
    let d = dry_run_on(&net, &["--count", "2", "--ip-address", "192.0.2.50/24"]);
    for l in ["Wants=", "After="] {
        let line = format!("{l}sys-subsystem-net-devices-br\\x2dlan.device");
        assert!(d.unit.contains(&line), "{line}: {:#?}", d.unit);
    }
    assert!(d.unit.iter().any(|l| l.ends_with("--lan-interface br-lan")));
}

/// **The check's log is a new file** (`open(log, "x")`): run as root in a
/// tree its user can write, it never writes through a name already there,
/// a symbolic link planted at the log's path among them. **Fails** a check
/// that opens the log for writing whatever is there.
#[test]
fn the_check_s_log_is_never_an_existing_file() {
    judged(
        r#"
import os, tempfile
d = tempfile.mkdtemp()
target = os.path.join(d, "target")
open(target, "w").write("kept\n")
link = os.path.join(d, "check.log")
os.symlink(target, link)
try:
    c.Check("/x/tools/quux-taps", d, link)
    raise AssertionError("the log was opened through a link")
except FileExistsError:
    pass
assert open(target).read() == "kept\n"
k = c.Check("/x/tools/quux-taps", d, os.path.join(d, "new.log"))
k.log.close()
"#,
    );
}

/// **An interrupt or a signal still removes the check's namespaces**: a
/// SIGTERM or SIGHUP during the run ends it through `close()`, which
/// deletes every namespace it made, one `ip netns del` failing not keeping
/// the others; a SIGINT during `close()` does not cut it short. **Fails** a
/// check that leaves namespaces behind at a signal.
#[test]
fn a_signal_still_removes_the_check_s_namespaces() {
    judged(
        r#"
import os, signal, subprocess
for sig in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT):
    deleted = []
    class K(c.Check):
        def run(self):
            self.made = list(self.ns.values())
            os.kill(os.getpid(), sig)
            raise AssertionError("ran on after the signal")
        def sh(self, *argv, ns=None, check=True, timeout=0):
            if argv[:3] == ("ip", "netns", "del"):
                deleted.append(argv[3])
                if len(deleted) == 1:
                    raise subprocess.TimeoutExpired(argv, 1)
                if len(deleted) == 2:
                    os.kill(os.getpid(), signal.SIGINT)
    c.Check = K
    try:
        status = c.main(["--self", "/x/tools/quux-taps", "--work", "/x/work"])
    except SystemExit as e:
        status = e.code
    except KeyboardInterrupt:
        status = 130
    assert status not in (0, None), (sig, status)
    assert len(deleted) == 6, (sig, deleted)
    signal.signal(signal.SIGINT, signal.default_int_handler)
"#,
    );
}
