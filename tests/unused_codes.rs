// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What runs of the codes the CADR leaves unassigned and QUUX took for its
//! clocks and its fused return: functional destinations 3 to 7 and
//! functional sources 15 and 17.
//!
//! The scan is `support::unused_codes`'s: a band booted on `rtl`, every
//! executed microinstruction read as it stood in `IR`, the OA substitution
//! done. System 2001's run is in `tests/system_2001.rs`.

use support::unused_codes::{octal, run};

mod support;

/// **System 1003 on the CADR's microcode 1001, microcode 1000 with more
/// fixes and the changes for 60 boards, never runs them at all**, the OA
/// registers' words included, through its boot to the listener. The
/// CADR's release, fetched by `tools/fetch-system-for-cadr.sh`.
#[test]
fn system_1003_on_1001_never_runs_the_codes() {
    let (Some(pack), Some(sources)) =
        (support::vendor(&["run", "release-1003-pack.img"]), support::vendor(&["system-1003"]))
    else {
        return;
    };
    let dir = support::scratch("unused-codes-1003");
    let copy = dir.join("pack.img");
    std::fs::copy(&pack, &copy).unwrap();
    let root = dir.join("root");
    std::fs::create_dir_all(root.join("lispm")).unwrap();
    std::os::unix::fs::symlink(sources.join("sys"), root.join("sys")).unwrap();
    std::os::unix::fs::symlink(sources.join("site"), root.join("site")).unwrap();
    let found = run(support::machine_with_pack(&copy), (0o177201, 0o177200), root);
    assert!(found.used.is_empty(), "the codes ran at {}", octal(&found.used));
}
