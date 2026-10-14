// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The checksum vectors (the Ethernet contract, section 2.4 and section 9):
//! every case of `tests/ethernet_vectors.txt` run through the device's
//! [`transmit`] and [`receive`]. The file's header gives its format; its
//! results come from frames Linux made and from `tools/ethernet-vectors.py`,
//! never from this crate.

use muir::ethernet_offload::{RxCounter, ones_sum, receive, transmit};

const VECTORS: &str = include_str!("ethernet_vectors.txt");

struct Case<'a> {
    line: usize,
    name: &'a str,
    direction: &'a str,
    frame: Vec<u8>,
    result: &'a str,
}

fn hex(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2), "odd hex length");
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex")).collect()
}

fn cases() -> Vec<Case<'static>> {
    let mut out = Vec::new();
    for (i, line) in VECTORS.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut f = line.splitn(4, ' ');
        let (name, direction, frame, result) = (
            f.next().unwrap(),
            f.next().expect("direction"),
            f.next().expect("frame"),
            f.next().expect("result"),
        );
        out.push(Case { line: i + 1, name, direction, frame: hex(frame), result });
    }
    out
}

/// What the device does with one case, in the file's words.
fn run(c: &Case) -> String {
    match c.direction {
        "tx" => {
            let mut f = c.frame.clone();
            match transmit(&mut f) {
                Ok(()) => f.iter().map(|b| format!("{b:02x}")).collect(),
                Err(r) => {
                    assert_eq!(f, c.frame, "{}: a refused frame was changed", c.name);
                    assert!(!r.reason().is_empty());
                    "refused".into()
                }
            }
        }
        "rx" => match receive(&c.frame) {
            Ok(()) => "delivered".into(),
            Err(d) => {
                assert!(!d.reason().is_empty());
                match d.counter() {
                    RxCounter::OtherType => "dropped by type".into(),
                    RxCounter::Ipv4Check => "dropped by the IPv4 checks".into(),
                }
            }
        },
        d => panic!("line {}: direction {d}", c.line),
    }
}

#[test]
fn every_vector_passes() {
    let cases = cases();
    let mut failed = Vec::new();
    for c in &cases {
        let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(c)))
            .unwrap_or_else(|_| "a panic".into());
        if got != c.result {
            let at =
                got.bytes().zip(c.result.bytes()).position(|(a, b)| a != b).unwrap_or(0) / 2 * 2;
            let (e, g) = (&c.result[at.min(c.result.len())..], &got[at.min(got.len())..]);
            failed.push(format!(
                "line {} {} {}: from byte {}, expected {:.24}, got {:.24}",
                c.line,
                c.name,
                c.direction,
                at / 2,
                e,
                g
            ));
        }
    }
    for f in &failed {
        eprintln!("{f}");
    }
    eprintln!("{} vectors, {} failed", cases.len(), failed.len());
    assert!(failed.is_empty(), "{} of {} vectors failed", failed.len(), cases.len());
}

/// The file is what its header says: unique names, known results, and the
/// kinds of case section 9 asks for all present.
#[test]
fn the_file_is_well_formed() {
    let cases = cases();
    let mut seen = std::collections::HashSet::new();
    for c in &cases {
        assert!(
            seen.insert((c.name, c.direction)),
            "line {}: {} {} twice",
            c.line,
            c.name,
            c.direction
        );
        assert!(c.frame.len() <= 1514, "line {}: longer than 1514 bytes", c.line);
        match c.direction {
            "rx" => assert!(
                ["delivered", "dropped by type", "dropped by the IPv4 checks"].contains(&c.result),
                "line {}: {}",
                c.line,
                c.result
            ),
            "tx" => assert!(
                c.result == "refused" || hex(c.result).len() == c.frame.len(),
                "line {}",
                c.line
            ),
            d => panic!("line {}: direction {d}", c.line),
        }
    }
    let count = |f: &dyn Fn(&Case) -> bool| cases.iter().filter(|c| f(c)).count();
    let linux = |c: &Case| c.name.starts_with("linux-");
    assert!(count(&|c| linux(c) && c.direction == "rx") >= 20);
    assert!(count(&|c| linux(c) && c.direction == "tx") >= 20);
    assert!(count(&|c| linux(c) && c.result != "delivered" && c.direction == "rx") == 0);
    for result in ["delivered", "dropped by type", "dropped by the IPv4 checks"] {
        assert!(count(&|c| c.result == result) >= 5, "{result}");
    }
    assert!(count(&|c| c.result == "refused") >= 10);
    assert!(seen.len() >= 150);
}

/// No frame, cut anywhere, makes either direction panic.
#[test]
fn every_prefix_is_handled() {
    for c in cases() {
        for k in 0..=c.frame.len() {
            let _ = receive(&c.frame[..k]);
            let mut f = c.frame[..k].to_vec();
            let _ = transmit(&mut f);
        }
    }
}

/// RFC 1071 section 3's worked example: the sum of 00 01 f2 03 f4 f5 f6 f7
/// is ddf2.
#[test]
fn rfc_1071_example() {
    assert_eq!(ones_sum(&[0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7]), 0xddf2);
    assert_eq!(ones_sum(&[0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6]), 0xdcfb);
}
