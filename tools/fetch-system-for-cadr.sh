#!/bin/sh
# SPDX-FileCopyrightText: 2026 Mete Balci
# SPDX-License-Identifier: AGPL-3.0-or-later
# Fetch the CADR's current muir-sys release, System 1003.
#
# System 1003 continues System 100, 1001 and 1002 on the CADR, on the
# CADR's microcode 1001, microcode 1000 with more fixes and the changes for
# up to 60 memory boards, 3840K, where muir-sim's `cadr` gives 32 by
# default (`--main-memory-boards 60`); the release's notes,
# `docs/release-1003.md` in its sources, list them. It is the GitHub release
# `release-1003` of
# https://github.com/metebalci/muir-sys, made on
# that repository's `cadr` branch, and like System 100 it is not part of
# this repository: `vendor/` is fetched material and is in .gitignore.
# System 100, fetched by `fetch-system-100-for-cadr.sh`, stays the CADR's
# reference; this is the system muir-sys evolves for it.
#
# The release's files, and where this puts them:
#
#   SHA256SUMS                the release's own sums, kept beside the rest
#   release-1003-sys.tar.gz   the sources: the muir-sys repository at the
#                             release's tag, with `sys/ubin/` assembled,
#                             unpacked to vendor/system-1003/
#   release-1003-pack.img.gz  the disk pack, decompressed to
#                             vendor/run/release-1003-pack.img
#
# The license is in the sources: the GNU Affero General Public License,
# version 3 or later, muir's own, with `NOTICE` giving their provenance.
#
# The release is pinned: its tag and every file's SHA-256 are below, and a
# newer release is taken by a commit that changes them, never by following
# GitHub's latest release. Every file is checked against its sum, whether
# just downloaded or already in place, and the pack again as it is
# decompressed. SYSTEM_FOR_CADR_BASE names another place to download from.
#
# The number and the format say the machine, and both are checked before
# anything is unpacked: the sources must unpack to one `release-1NNN/`
# directory, a CADR system's number, and the pack must begin with `LABL`,
# the CADR's pack label. QUUX's releases, numbered from 2000 and on a VHD,
# are refused.
#
# The band in the pack was built for its site's Chaosnet: the machine
# LISPM-1 at 177201, and its file, time and host table server OZ at 177200,
# which serves the sources' `sys` and `site` and a writable `lispm` home,
# with FILE dates in UTC. The release's notes give the ozd and `cadr`
# commands.
#
# Re-running this is safe: whatever is already in place is left alone, and
# nothing else in vendor/ is touched.
#
# usage: tools/fetch-system-for-cadr.sh

set -eu
muir=$(cd "$(dirname "$0")/.." && pwd)
tag=release-1003
base=${SYSTEM_FOR_CADR_BASE:-https://github.com/metebalci/muir-sys/releases/download/$tag}
rel=$muir/vendor/system-${tag#release-}
vrel=vendor/system-${tag#release-}
run=$muir/vendor/run
sources=$tag-sys.tar.gz
pack=$tag-pack.img

# The SHA-256 of each file, as the release's SHA256SUMS and notes give them.
sum_of() {
    case $1 in
    SHA256SUMS) echo a28cefd16354ebd735f50edbb5f3dfd4c30e6ed46fcdcb42a3ecd7e5444c44b0 ;;
    release-1003-sys.tar.gz) echo 83fc38c0236fdb372099641ea2d93e2dd3a9ef905420d8ff1307b3369266b85e ;;
    release-1003-pack.img.gz) echo cf3632a7a2ab0522043994ee004e02b1766ee93b4ba287170566d149de1aed78 ;;
    release-1003-pack.img) echo 7e8ab2aac763f1e03a4cc2b08b31b43707565f75604c8683e14628d2a00e6bc1 ;;
    esac
}

# The SHA-256 of a file, with whichever tool the system has.
sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

mkdir -p "$rel" "$run"

for f in SHA256SUMS "$sources" "$pack.gz"; do
    if [ -f "$rel/$f" ]; then
        echo "have $vrel/$f"
    else
        curl -fL -o "$rel/$f.part" "$base/$f"
        mv "$rel/$f.part" "$rel/$f"
    fi
    if [ "$(sha256 "$rel/$f")" != "$(sum_of "$f")" ]; then
        echo "$vrel/$f is not the file the release published:" >&2
        echo "its SHA-256 is not $(sum_of "$f"); move it away and run this again" >&2
        exit 1
    fi
    echo "     $vrel/$f checks"
done

refused=

# The number: every member of the sources under one `release-1NNN/`.
top=$(tar -tzf "$rel/$sources" | sed 's|/.*||' | sort -u)
case $top in
release-1[0-9][0-9][0-9]) echo "     $vrel/$sources unpacks to $top/, a CADR system's" ;;
*)
    echo "$vrel/$sources unpacks to $(echo "$top" | tr '\n' ' ')" >&2
    echo "and not to one release-1NNN/: it is not a system for the CADR" >&2
    refused=1
    ;;
esac

# The pack is checked as it comes out and not afterwards: a machine writes
# to the pack it runs on, so the image in place is only the release's until
# the first run.
if [ -f "$run/$pack" ]; then
    echo "have vendor/run/$pack"
else
    gunzip -c "$rel/$pack.gz" > "$run/$pack.part"
    if [ "$(sha256 "$run/$pack.part")" != "$(sum_of "$pack")" ]; then
        rm -f "$run/$pack.part"
        echo "$vrel/$pack.gz does not decompress to the pack the release published" >&2
        exit 1
    fi
    echo "     vendor/run/$pack checks"
    # The format: a CADR pack begins with its label.
    if [ "$(head -c 4 "$run/$pack.part")" = LABL ]; then
        echo "     vendor/run/$pack begins with LABL, a CADR pack"
    else
        echo "vendor/run/$pack does not begin with LABL: it is not a CADR pack" >&2
        refused=1
    fi
    if [ -n "$refused" ]; then
        rm -f "$run/$pack.part"
    else
        mv "$run/$pack.part" "$run/$pack"
    fi
fi

if [ -n "$refused" ]; then
    echo "refused: nothing unpacked" >&2
    exit 1
fi

# The tarball's one top directory is left out, so the sources sit beside
# the tarball as System 100's do: vendor/system-1NNN/sys.
if [ -d "$rel/sys" ]; then
    echo "have $vrel/sys"
else
    tar xzf "$rel/$sources" -C "$rel" --strip-components=1
    echo "     $vrel/ holds the sources"
fi

echo "done: $tag; its notes at https://github.com/metebalci/muir-sys/releases/tag/$tag say how to run it"
