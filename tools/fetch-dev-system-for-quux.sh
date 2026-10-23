#!/bin/sh
# SPDX-FileCopyrightText: 2026 Mete Balci
# SPDX-License-Identifier: AGPL-3.0-or-later
# Fetch QUUX's system in development, muir-sys's rolling release.
#
# `dev-system-for-quux` is a prerelease of https://github.com/metebalci/muir-sys
# whose files are replaced with each new build of muir-sys's `main`; its tag
# never moves, and its README names the build. It is for trying the newest
# system, never for tests or CI, whose bytes must not move:
# `fetch-handover-2002.sh` fetches a pinned hand-over for those.
#
# The release's files, and where this puts them:
#
#   SHA256SUMS                          every file below, the README
#                                       included, and the disk uncompressed
#   README                              the build: commit, date, sums
#   dev-system-for-quux-sys.tar.gz      the sources, unpacked to
#                                       vendor/dev-system-for-quux/
#   dev-system-for-quux-disk.vhd.gz     QUUX's disk, decompressed to
#                                       vendor/run/dev-system-for-quux-disk.vhd
#   dev-system-for-quux-promh.mcr       the boot PROM (`quux --prom`)
#
# and it keeps the machine's host folder, its file root,
# vendor/run/dev-system-for-quux-root/: copies of the sources' `sys/` and
# `site/`, replaced with each new build, and `home/lispm/`, the login's
# home, made empty once and kept.
#
# Nothing is pinned here, so the files are checked against each other: the
# release's SHA256SUMS is downloaded first, then every file into a staging
# directory, then SHA256SUMS again. The two must be equal and every file
# must match its line, so the disk, the sources, the PROM and the README
# all come from one build; a replacement under way shows as a mismatch. On
# a failure this waits 60 seconds and tries once more. The number and the
# format are checked before anything is unpacked: the sources must
# unpack to `dev-system-for-quux/`, and the disk must be a dynamic VHD
# holding a GPT. Only then is the old build replaced, by renaming the
# staging directory into place, and the old disk is removed before that and
# the new one moved in last, so a run cut short leaves no disk from one
# build beside sources from another.
#
# The disk is checked as it is decompressed and not afterwards: a machine
# writes to the disk it runs on. So when the release's SHA256SUMS is the one
# kept from the last run and the disk is in place, nothing is replaced, and
# the machine keeps its disk. DEV_SYSTEM_FOR_QUUX_BASE names another place
# to download from.
#
# usage: tools/fetch-dev-system-for-quux.sh
# then:  quux --prom vendor/dev-system-for-quux/dev-system-for-quux-promh.mcr \
#             --disk-pack vendor/run/dev-system-for-quux-disk.vhd \
#             --file-root vendor/run/dev-system-for-quux-root

set -eu
muir=$(cd "$(dirname "$0")/.." && pwd)
tag=dev-system-for-quux
base=${DEV_SYSTEM_FOR_QUUX_BASE:-https://github.com/metebalci/muir-sys/releases/download/$tag}
rel=$muir/vendor/$tag
run=$muir/vendor/run
stage=$muir/vendor/$tag.staging
sources=$tag-sys.tar.gz
disk=$tag-disk.vhd
prom=$tag-promh.mcr
root=$run/$tag-root
assets="README $sources $disk.gz $prom"
wait=60

# The SHA-256 of a file, with whichever tool the system has.
sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

# The sum SHA256SUMS file $1 gives name $2, or nothing. A line is the sum,
# two spaces, and the name (GNU format, text mode).
sum_in() {
    awk -v n="$2" '$2 == n { print $1; exit }' "$1"
}

# The unsigned big-endian number of $3 bytes at offset $2 of file $1.
be() {
    n=0
    for b in $(od -An -v -tu1 -j "$2" -N "$3" "$1"); do
        n=$((n * 256 + b))
    done
    echo "$n"
}

# The $3 bytes at offset $2 of file $1, as text.
bytes_at() {
    dd if="$1" bs=1 skip="$2" count="$3" 2>/dev/null
}

# Whether file $1 is a dynamic VHD whose disk holds a GPT, saying why not
# on stderr. Microsoft's VHD specification (October 2006), every field
# big-endian: the footer is the file's last 512 bytes, cookie `conectix` at
# 0, the dynamic header's offset at 16, the disk type at 60 (3, dynamic);
# the dynamic header has cookie `cxsparse` at 0, the block table's offset at
# 16 and the block size at 32; a table entry is the sector where a block's
# sector bitmap begins, the block's data after the bitmap, or FFFFFFFF for a
# block not allocated, which reads as zeros. The GPT's header is the disk's
# sector 1 (UEFI 2.10, 5.3.1), in block 0.
vhd_holds_gpt() {
    name=${1#"$muir"/}
    size=$(wc -c < "$1")
    if [ "$size" -lt 1536 ]; then
        echo "$name is $size bytes, too short for a VHD" >&2
        return 1
    fi
    foot=$((size - 512))
    if [ "$(bytes_at "$1" "$foot" 8)" != conectix ]; then
        echo "$name has no VHD footer (no conectix in its last 512 bytes): not a VHD" >&2
        return 1
    fi
    if [ "$(be "$1" $((foot + 60)) 4)" != 3 ]; then
        echo "$name is a VHD of type $(be "$1" $((foot + 60)) 4), not a dynamic one (3)" >&2
        return 1
    fi
    header=$(be "$1" $((foot + 16)) 8)
    if [ "$(bytes_at "$1" "$header" 8)" != cxsparse ]; then
        echo "$name has no dynamic header (cxsparse) at $header" >&2
        return 1
    fi
    table=$(be "$1" $((header + 16)) 8)
    block=$(be "$1" $((header + 32)) 4)
    first=$(be "$1" "$table" 4)
    if [ "$first" = 4294967295 ]; then
        echo "$name: its disk's first block is not allocated, so sector 1 is zero: no GPT" >&2
        return 1
    fi
    bitmap=$(((block / 512 + 4095) / 4096 * 512))
    at=$((first * 512 + bitmap + 512))
    if [ "$(bytes_at "$1" "$at" 8)" != "EFI PART" ]; then
        echo "$name is a VHD whose sector 1 is not a GPT header (no EFI PART): no GPT" >&2
        return 1
    fi
}

# The transfer: SHA256SUMS, every file, SHA256SUMS again, into the staging
# directory; whether the two SHA256SUMS agree and every file matches.
transfer() {
    rm -rf "$stage"
    mkdir -p "$stage"
    curl -fsSL -o "$stage/SHA256SUMS" "$base/SHA256SUMS" || return 1
    for f in $assets; do
        curl -fL -o "$stage/$f" "$base/$f" || return 1
    done
    curl -fsSL -o "$stage/SHA256SUMS.again" "$base/SHA256SUMS" || return 1
    if ! cmp -s "$stage/SHA256SUMS" "$stage/SHA256SUMS.again"; then
        echo "SHA256SUMS changed during the download: the release was being replaced" >&2
        return 1
    fi
    rm "$stage/SHA256SUMS.again"
    ok=0
    # Every file the sums name, and every file this takes, and every one
    # present; the uncompressed disk is no file of the release and is
    # checked as it is decompressed.
    names=$(awk '{ print $2 }' "$stage/SHA256SUMS")
    for f in $assets $disk; do
        if [ -z "$(sum_in "$stage/SHA256SUMS" "$f")" ]; then
            echo "SHA256SUMS has no line for $f" >&2
            ok=1
        fi
    done
    for f in $names; do
        [ "$f" = "$disk" ] && continue
        if [ ! -f "$stage/$f" ]; then
            echo "SHA256SUMS names $f, which is not here" >&2
            ok=1
        elif [ "$(sha256 "$stage/$f")" != "$(sum_in "$stage/SHA256SUMS" "$f")" ]; then
            echo "$f does not match its line in SHA256SUMS" >&2
            ok=1
        else
            echo "     $f checks"
        fi
    done
    return $ok
}

# Its build, as its README gives it.
build_of() {
    grep -E '^ +(commit|built) ' "$1" | sed 's/^ */  /'
}

mkdir -p "$run"

# The same build as the last run's, with the disk in place: left alone.
if [ -f "$rel/SHA256SUMS" ] && [ -f "$run/$disk" ] && [ -d "$root/sys" ]; then
    if curl -fsSL -o "$run/$tag.SHA256SUMS.part" "$base/SHA256SUMS" &&
        cmp -s "$run/$tag.SHA256SUMS.part" "$rel/SHA256SUMS"; then
        rm -f "$run/$tag.SHA256SUMS.part"
        echo "the build is unchanged since the last run; nothing replaced:"
        build_of "$rel/README"
        exit 0
    fi
    rm -f "$run/$tag.SHA256SUMS.part"
fi

if ! transfer; then
    echo "waiting $wait s and trying once more" >&2
    sleep "$wait"
    if ! transfer; then
        rm -rf "$stage"
        echo "refused: the release's files do not agree with its SHA256SUMS" >&2
        exit 1
    fi
fi

refused=

# The number: every member of the sources under `dev-system-for-quux/`.
top=$(tar -tzf "$stage/$sources" | sed 's|/.*||' | sort -u)
if [ "$top" = "$tag" ]; then
    echo "     $sources unpacks to $top/"
else
    echo "$sources unpacks to $(echo "$top" | tr '\n' ' ')" >&2
    echo "and not to $tag/: it is not the system for QUUX in development" >&2
    refused=1
fi

# The disk, checked as it comes out: its sum, then its format.
gunzip -c "$stage/$disk.gz" > "$stage/$disk"
if [ "$(sha256 "$stage/$disk")" != "$(sum_in "$stage/SHA256SUMS" "$disk")" ]; then
    echo "$disk.gz does not decompress to the disk SHA256SUMS names" >&2
    refused=1
else
    echo "     $disk checks"
    if vhd_holds_gpt "$stage/$disk"; then
        echo "     $disk is a dynamic VHD holding a GPT, QUUX's disk"
    else
        echo "$disk is not QUUX's disk" >&2
        refused=1
    fi
fi

if [ -n "$refused" ]; then
    rm -rf "$stage"
    echo "refused: nothing replaced" >&2
    exit 1
fi

tar xzf "$stage/$sources" -C "$stage" --strip-components=1

if [ -f "$rel/README" ]; then
    echo "replacing the last run's build:"
    build_of "$rel/README"
    echo "with:"
else
    echo "the build:"
fi
build_of "$stage/README"

# The old disk goes first and the new one comes in last, so no disk from
# one build ever sits beside the sources of another.
rm -f "$run/$disk"
rm -rf "$rel.old"
if [ -d "$rel" ]; then
    mv "$rel" "$rel.old"
fi
mv "$stage" "$rel"
rm -rf "$rel.old"
mkdir -p "$root/home/lispm"
rm -rf "$root/sys" "$root/site"
cp -Rp "$rel/sys" "$rel/site" "$root/"
mv "$rel/$disk" "$run/$disk"

echo "done: vendor/$tag/, vendor/run/$disk, and vendor/run/$tag-root with"
echo "this build's sys/ and site/ and the home kept; run it with"
echo "  quux --prom vendor/$tag/$prom --disk-pack vendor/run/$disk --file-root vendor/run/$tag-root"
