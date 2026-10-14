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
//!   named, with the same address, MAC address and MTU;
//! - IPv6 off on every tap before that tap comes up, and `rp_filter` 2;
//! - the defaults: two taps, 169.254.0.1/30;
//! - the refusals: without root, and for arguments it cannot use.
//!
//! The commands are run by `sh`, which every Unix CI runner has, and read
//! `id`; nothing else is wanted.

use std::path::PathBuf;
use std::process::{Command, Output};

const SBIN: &str = "/usr/local/sbin/quux-taps";
const UNIT: &str = "/etc/systemd/system/quux-taps.service";

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tools/quux-taps")
}

/// The script run with `args`, with no `SUDO_USER` to pick the owner.
fn quux_taps(args: &[&str]) -> Output {
    Command::new("sh").arg(script()).args(args).env_remove("SUDO_USER").output().expect("sh runs")
}

fn id(flag: &str) -> String {
    let o = Command::new("id").arg(flag).output().expect("id runs");
    String::from_utf8(o.stdout).expect("id prints text").trim().to_string()
}

/// The user running the tests, who owns the taps unless one is named.
fn me() -> String {
    id("-un")
}

/// A dry run's output, split into what it does now (`now`), the unit's
/// lines (`unit`) and what starting the unit runs (`up`).
struct DryRun {
    now: Vec<String>,
    unit: Vec<String>,
    up: Vec<String>,
}

fn dry_run(args: &[&str]) -> DryRun {
    let mut all = vec!["--dry-run"];
    all.extend_from_slice(args);
    let o = quux_taps(&all);
    assert!(o.status.success(), "quux-taps {all:?} failed: {}", String::from_utf8_lossy(&o.stderr));
    let text = String::from_utf8(o.stdout).expect("text");
    let mut d = DryRun { now: vec![], unit: vec![], up: vec![] };
    for line in text.lines() {
        if let Some(l) = line.strip_prefix("| ") {
            d.unit.push(l.to_string());
        } else if line == "|" {
            d.unit.push(String::new());
        } else if let Some(l) = line.strip_prefix(">   ") {
            d.up.push(l.to_string());
        } else {
            d.now.push(line.to_string());
        }
    }
    d
}

/// The unit, as it must read for `count` taps at `address` for `user`.
fn unit(count: usize, address: &str, user: &str) -> Vec<String> {
    let taps = if count == 1 { "quux0".to_string() } else { format!("quux0 to quux{}", count - 1) };
    vec![
        "# Written by quux-taps; `quux-taps --uninstall` removes it.".to_string(),
        "[Unit]".to_string(),
        format!("Description=QUUX tap pool: {taps}, owned by {user}"),
        String::new(),
        "[Service]".to_string(),
        "Type=oneshot".to_string(),
        "RemainAfterExit=yes".to_string(),
        format!("ExecStart={SBIN} --up --count {count} --address {address} --user {user}"),
        format!("ExecStop={SBIN} --down"),
        String::new(),
        "[Install]".to_string(),
        "WantedBy=multi-user.target".to_string(),
    ]
}

/// What starting the unit runs, for `count` taps at `address` for `user`:
/// every tap made, udev settled, then each tap set up and brought up.
fn up(count: usize, address: &str, user: &str) -> Vec<String> {
    let mut v: Vec<String> =
        (0..count).map(|k| format!("ip tuntap add dev quux{k} mode tap user {user}")).collect();
    v.push("udevadm settle".to_string());
    for k in 0..count {
        v.push(format!("sysctl -q -w net.ipv6.conf.quux{k}.disable_ipv6=1"));
        v.push(format!("sysctl -q -w net.ipv4.conf.quux{k}.rp_filter=2"));
        v.push(format!("ip link set dev quux{k} address 02:00:00:00:00:02 mtu 1500"));
        v.push(format!("ip address add {address} dev quux{k}"));
        v.push(format!("ip link set dev quux{k} up"));
    }
    v
}

/// What the install does now, around the unit: the old pool stopped, the
/// script copied, the unit written, loaded, enabled and started.
fn check_now(d: &DryRun) {
    let pos = |what: &str| {
        d.now
            .iter()
            .position(|l| l == what || l.ends_with(what))
            .unwrap_or_else(|| panic!("the dry run does not say `{what}`:\n{}", d.now.join("\n")))
    };
    let stop = pos("systemctl stop quux-taps.service");
    let copy = pos(&format!(" {SBIN}"));
    assert!(d.now[copy].starts_with("install -D -m 0755 "), "{}", d.now[copy]);
    let write = pos(&format!("# write {UNIT}, mode 0644:"));
    let reload = pos("systemctl daemon-reload");
    let enable = pos("systemctl enable --now quux-taps.service");
    assert!(stop < copy && copy < write && write < reload && reload < enable, "{:#?}", d.now);
}

/// Every tap's IPv6 is turned off, and its `rp_filter` set, before that
/// tap comes up: Linux starts IPv6 on a link when it comes up, so the
/// order is the point. Held for any count, apart from the exact lists.
fn check_order(up: &[String], count: usize) {
    for k in 0..count {
        let at = |what: String| {
            up.iter().position(|l| *l == what).unwrap_or_else(|| panic!("no `{what}`"))
        };
        let ipv6 = at(format!("sysctl -q -w net.ipv6.conf.quux{k}.disable_ipv6=1"));
        let rp = at(format!("sysctl -q -w net.ipv4.conf.quux{k}.rp_filter=2"));
        let made = at(format!("ip tuntap add dev quux{k} mode tap user {}", me()));
        let comes_up = at(format!("ip link set dev quux{k} up"));
        assert!(made < ipv6 && ipv6 < comes_up && rp < comes_up, "quux{k}: {up:#?}");
    }
    let made = up.iter().filter(|l| l.starts_with("ip tuntap add ")).count();
    assert_eq!(made, count, "taps made");
    assert!(
        !up.iter().any(|l| l.contains(&format!("quux{count} "))),
        "a tap past quux{}",
        count - 1
    );
}

#[test]
fn a_pool_of_one() {
    let me = me();
    let d = dry_run(&["--count", "1"]);
    assert_eq!(d.unit, unit(1, "169.254.0.1/30", &me));
    assert_eq!(d.up, up(1, "169.254.0.1/30", &me));
    check_order(&d.up, 1);
    check_now(&d);
}

#[test]
fn a_pool_of_thirty_two_on_the_build_hosts_address() {
    let me = me();
    let d = dry_run(&["--count", "32", "--address", "169.254.1.1/30", "--user", &me]);
    assert_eq!(d.unit, unit(32, "169.254.1.1/30", &me));
    assert_eq!(d.up, up(32, "169.254.1.1/30", &me));
    check_order(&d.up, 32);
    check_now(&d);
}

#[test]
fn the_defaults_are_two_taps_at_169_254_0_1() {
    let me = me();
    let d = dry_run(&[]);
    assert_eq!(d.unit, unit(2, "169.254.0.1/30", &me));
    assert_eq!(d.up, up(2, "169.254.0.1/30", &me));
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
    for args in [
        vec![],
        vec!["--count", "32", "--address", "169.254.1.1/30", "--user", &me],
        vec!["--uninstall"],
        vec!["--up", "--count", "1", "--user", &me],
        vec!["--down"],
    ] {
        let o = quux_taps(&args);
        assert_eq!(o.status.code(), Some(1), "{args:?}");
        assert!(o.stdout.is_empty(), "{args:?} printed {}", String::from_utf8_lossy(&o.stdout));
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains("must run as root"), "{args:?}: {err}");
    }
}

/// Arguments it cannot use are refused with status 2 and nothing done,
/// before the question of root arises.
#[test]
fn refuses_arguments_it_cannot_use() {
    for args in [
        ["--count", "0"],
        ["--count", "257"],
        ["--count", "08"],
        ["--count", "two"],
        ["--address", "169.254.0.1"],
        ["--address", "169.254.0/30"],
        ["--address", "169.254.0.256/30"],
        ["--address", "169.254.0.1/31"],
        ["--user", "no-such-user-for-quux-taps"],
        ["--bogus", "x"],
    ] {
        for dry in [true, false] {
            let mut a = args.to_vec();
            if dry {
                a.push("--dry-run");
            }
            let o = quux_taps(&a);
            assert_eq!(o.status.code(), Some(2), "{a:?}: {}", String::from_utf8_lossy(&o.stderr));
            assert!(o.stdout.is_empty(), "{a:?}");
        }
    }
}
