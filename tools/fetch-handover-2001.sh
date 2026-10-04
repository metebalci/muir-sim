#!/bin/sh
# SPDX-FileCopyrightText: 2026 Mete Balci
# SPDX-License-Identifier: AGPL-3.0-or-later
# Fetch muir-sys's newest hand-over of System 2001, the 40-bit QUUX's system,
# for revision 13's tests until System 2001 is released (contract G2 §11.2).
#
# System 2001, microcode 2001 and PROM 2001 are not released. muir-sys
# publishes each hand-over of them as a GitHub pre-release of
# https://github.com/metebalci/muir-sys whose tag never moves; this script is
# pinned to one, `handover-2001-81b3973`, by its tag and every file's SHA-256, and
# a newer hand-over is taken by a commit that changes them. It goes when
# release-2001 exists, and a fetch of that release takes its place.
#
# The pre-release's files, all kept in ref/band-2001-81b3973/, which is in
# .gitignore:
#
#   SHA256SUMS                     the pre-release's own sums
#   README                         what the files are, and the hand-over's
#                                  note: the commit it was built from and
#                                  how it was checked
#   handover-2001-81b3973-sys.tar.gz   the sources the band was built from,
#                                      with the QFASLs and `sys/ubin/`; it
#                                      unpacks to release-2001-81b3973/
#   handover-2001-81b3973-disk.vhd.gz  the disk, a dynamic VHD holding a
#                                      GPT, decompressed beside it to
#                                      handover-2001-81b3973-disk.vhd
#   handover-2001-81b3973-promh.mcr    PROM 2001; `quux`'s own revision-13
#                                      PROM, data/quux-promh.mcr, is the
#                                      same
#
# and from the sources' `sys/ubin/`, copied beside them, the microcode's and
# the PROM's files the tests read: ucadr.mcr, ucadr.sym, ucadr.tbl,
# ucadr.locs, promh.mcr, promh.sym, promh.tbl and promh.locs, the PROM held
# to the pre-release's own. The tests copy the disk before a machine writes
# it, so the one here stays the pre-release's.
#
# Every file is checked against its sum, whether just downloaded or already
# in place, and the disk again as it is decompressed. HANDOVER_2001_BASE
# names another place to download from. Before anything is decompressed or
# copied, the sources must unpack to one `release-2NNN-*/` directory, a
# QUUX system's number, and the disk must be a dynamic VHD holding a GPT
# (the same check as tools/fetch-system-for-quux.sh's) laid out as System
# 2001's release disk is: 853,359 blocks of 1024 bytes, a PAGE partition
# of 655,360 blocks (128MW), microcode 2001 in the current MCR1,
# "MCR1 UCADR 2001", and the band in the current LOD1, "LOD1 System 2001".
#
# Re-running this is safe: whatever is already in place and checks is left
# alone, and nothing else in ref/ is touched.
#
# usage: tools/fetch-handover-2001.sh
# then:  cargo test --test system_2001

set -eu
muir=$(cd "$(dirname "$0")/.." && pwd)
tag=handover-2001-81b3973
base=${HANDOVER_2001_BASE:-https://github.com/metebalci/muir-sys/releases/download/$tag}
dir=$muir/ref/band-2001-81b3973
rdir=ref/band-2001-81b3973
sources=$tag-sys.tar.gz
disk=$tag-disk.vhd
prom=$tag-promh.mcr
assets="SHA256SUMS README $sources $disk.gz $prom"
ubin="ucadr.mcr ucadr.sym ucadr.tbl ucadr.locs promh.mcr promh.sym promh.tbl promh.locs"

# The SHA-256 of each file, as the pre-release's SHA256SUMS gives them.
sum_of() {
    case $1 in
    SHA256SUMS) echo 157535c07244e08195398735383bf417741bccbe350484be2aa917f923d1cfa8 ;;
    README) echo 2511954bfa89272e2b6d2d959bf70c7308724863ef47e5816947a053fa2de0cc ;;
    handover-2001-81b3973-sys.tar.gz) echo a11d3527f53450cccdaf30b41d37cc69529d686109eb061d1acd19133fee52ae ;;
    handover-2001-81b3973-disk.vhd.gz) echo 6607f93a20854e52ab823eb145cd68de99d9566f2fab77dda45da9b130b2e71e ;;
    handover-2001-81b3973-disk.vhd) echo 9034de8b512f660c4e9506b7e5a6c70500b8f988bb91fc67bde3d588dc65f872 ;;
    handover-2001-81b3973-promh.mcr) echo 5917bae4a5e21806acd40fa9af7bc89a307e45f49cc974ee880c9e4ba002b9ca ;;
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

# The unsigned little-endian number of $3 bytes at offset $2 of file $1.
le() {
    n=0 m=1
    for b in $(od -An -v -tu1 -j "$2" -N "$3" "$1"); do
        n=$((n + b * m))
        m=$((m * 256))
    done
    echo "$n"
}

# Whether file $1 is a dynamic VHD whose disk holds a GPT, saying why not
# on stderr: tools/fetch-system-for-quux.sh's check, which cites Microsoft's
# VHD specification and UEFI for each field.
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

# The offset in the dynamic VHD $1 of its disk's 512-byte sector $2, or
# nothing if the block holding it is not allocated: the block table entry
# is the sector where the block's sector bitmap begins, and the sector's
# data follows the bitmap (the fields vhd_holds_gpt reads).
vhd_sector() {
    foot=$(($(wc -c < "$1") - 512))
    header=$(be "$1" $((foot + 16)) 8)
    table=$(be "$1" $((header + 16)) 8)
    block=$(be "$1" $((header + 32)) 4)
    entry=$(be "$1" $((table + $2 * 512 / block * 4)) 4)
    if [ "$entry" != 4294967295 ]; then
        echo $((entry * 512 + (block / 512 + 4095) / 4096 * 512 + $2 * 512 % block))
    fi
}

# Whether the dynamic VHD $1, which holds a GPT, is laid out as System
# 2001's release disk, saying why not on stderr: the disk's size, the VHD
# footer's current size at 48, is 853,359 blocks of 1024 bytes; the GPT's
# PAGE partition is 655,360 blocks, 128MW; and the partitions whose
# attribute bit 48 is set, the current ones, are "MCR1 UCADR 2001" and
# "LOD1 System 2001". The GPT's fields are UEFI's (2.10, 5.3.2 and 5.3.3),
# little-endian: the header at sector 1 gives the entry array's sector at
# 72, the entry count at 80 and the entry size at 84; an entry has its
# type GUID at 0, its first and last sector at 32 and 40, its attributes at
# 48 and its UTF-16LE name, ASCII here, at 56.
release_disk_layout() {
    name=${1#"$muir"/}
    foot=$(($(wc -c < "$1") - 512))
    blocks=$(($(be "$1" $((foot + 48)) 8) / 1024))
    if [ "$blocks" != 853359 ]; then
        echo "$name's disk is $blocks blocks, not System 2001's 853,359" >&2
        return 1
    fi
    gpt=$(vhd_sector "$1" 1)
    array=$(le "$1" $((gpt + 72)) 8)
    count=$(le "$1" $((gpt + 80)) 4)
    size=$(le "$1" $((gpt + 84)) 4)
    page= current=
    k=0
    while [ "$k" -lt "$count" ]; do
        at=$((array * 512 + k * size))
        k=$((k + 1))
        sector=$(vhd_sector "$1" $((at / 512)))
        [ -n "$sector" ] || continue
        e=$((sector + at % 512))
        [ "$(od -An -v -tx1 -N 16 -j "$e" "$1" | tr -d ' 0\n')" ] || continue
        part=$(bytes_at "$1" $((e + 56)) 72 | tr -d '\000')
        first=$(le "$1" $((e + 32)) 8)
        last=$(le "$1" $((e + 40)) 8)
        if [ "$part" = PAGE ]; then
            page=$(((last + 1 - first) / 2))
        fi
        if [ $(($(be "$1" $((e + 54)) 1) & 1)) = 1 ]; then
            current="$current${current:+, }$part"
        fi
    done
    if [ "$page" != 655360 ]; then
        echo "$name's PAGE partition is ${page:-no} blocks, not 655,360 (128MW)" >&2
        return 1
    fi
    if [ "$current" != "MCR1 UCADR 2001, LOD1 System 2001" ]; then
        echo "$name's current partitions are $current," >&2
        echo "not MCR1 UCADR 2001 and LOD1 System 2001" >&2
        return 1
    fi
}

mkdir -p "$dir"

for f in $assets; do
    if [ -f "$dir/$f" ]; then
        echo "have $rdir/$f"
    else
        curl -fL -o "$dir/$f.part" "$base/$f"
        mv "$dir/$f.part" "$dir/$f"
    fi
    if [ "$(sha256 "$dir/$f")" != "$(sum_of "$f")" ]; then
        echo "$rdir/$f is not the file the pre-release published:" >&2
        echo "its SHA-256 is not $(sum_of "$f"); move it away and run this again" >&2
        exit 1
    fi
    echo "     $rdir/$f checks"
done

# The number: every member of the sources under one `release-2NNN-*/`.
top=$(tar -tzf "$dir/$sources" | sed 's|/.*||' | sort -u)
case $top in
release-2[0-9][0-9][0-9]-*) echo "     $rdir/$sources unpacks to $top/, a QUUX system's" ;;
*)
    echo "$rdir/$sources unpacks to $(echo "$top" | tr '\n' ' ')" >&2
    echo "and not to one release-2NNN-*/: it is not a system for QUUX" >&2
    exit 1
    ;;
esac

if [ -f "$dir/$disk" ] && [ "$(sha256 "$dir/$disk")" = "$(sum_of "$disk")" ]; then
    echo "have $rdir/$disk, which checks"
else
    gunzip -c "$dir/$disk.gz" > "$dir/$disk.part"
    if [ "$(sha256 "$dir/$disk.part")" != "$(sum_of "$disk")" ]; then
        rm -f "$dir/$disk.part"
        echo "$rdir/$disk.gz does not decompress to the disk the pre-release published" >&2
        exit 1
    fi
    if ! vhd_holds_gpt "$dir/$disk.part"; then
        rm -f "$dir/$disk.part"
        echo "$rdir/$disk is not QUUX's disk" >&2
        exit 1
    fi
    if ! release_disk_layout "$dir/$disk.part"; then
        rm -f "$dir/$disk.part"
        echo "$rdir/$disk is not laid out as System 2001's release disk" >&2
        exit 1
    fi
    mv "$dir/$disk.part" "$dir/$disk"
    echo "     $rdir/$disk checks, a dynamic VHD holding a GPT, 853,359 blocks,"
    echo "     PAGE 655,360 blocks, the band in the current LOD1"
fi

# The microcode's and the PROM's files, from the sources, replaced each
# time from the checked tarball.
tmp=$dir/.ubin.part
rm -rf "$tmp"
mkdir -p "$tmp"
members=
for f in $ubin; do
    members="$members $top/sys/ubin/$f"
done
# shellcheck disable=SC2086
tar xzf "$dir/$sources" -C "$tmp" $members
for f in $ubin; do
    mv "$tmp/$top/sys/ubin/$f" "$dir/$f"
done
rm -rf "$tmp"
if [ "$(sha256 "$dir/promh.mcr")" != "$(sum_of "$prom")" ]; then
    echo "$rdir/$sources's sys/ubin/promh.mcr is not the pre-release's PROM" >&2
    exit 1
fi
echo "     $rdir/ holds the sources' sys/ubin/ files, the PROM the pre-release's"

echo "done: $tag in $rdir/; its tests run with"
echo "  cargo test --test system_2001"
