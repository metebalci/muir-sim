#!/bin/sh
# SPDX-FileCopyrightText: 2026 Mete Balci
# SPDX-License-Identifier: AGPL-3.0-or-later
# Fetch QUUX's current release, System 2001.
#
# QUUX runs muir-sys's system and no other: System 2001, on microcode 2001
# and boot PROM 2001, runs only on QUUX hardware revision 13, the 40-bit
# word. It is the GitHub release `release-2001` of
# https://github.com/metebalci/muir-sys, and like System 100 it is not part
# of this repository: `vendor/` is fetched material and is in .gitignore.
#
# The release's files, and where this puts them:
#
#   SHA256SUMS                the release's own sums, kept beside the rest
#   release-2001-sys.tar.gz   the sources: the muir-sys repository at the
#                             release's tag, with `sys/ubin/` assembled,
#                             unpacked to vendor/system-2001/
#   release-2001-disk.vhd.gz  QUUX's disk, a dynamic VHD holding a GPT,
#                             decompressed to vendor/run/release-2001-disk.vhd
#   release-2001-promh.mcr    boot PROM 2001, kept in vendor/system-2001/;
#                             `quux`'s own PROM, data/quux-promh.mcr, is the
#                             same, byte for byte
#
# and it makes the machine's host folder, its file root,
# vendor/run/release-2001-root/: copies of the sources' `sys/` and `site/`
# and an empty `home/lispm/`, the login's home: a login needs none, but a
# file written to the home needs it to exist. The machine writes, renames
# and deletes under its root, so the root is a directory of its own and the
# checked sources are never in its reach; they are copies and not links
# because the file device refuses a link that leaves its mount. The root is
# made once and left alone afterwards, whatever is in it.
#
# The tests (`tests/system_2001.rs`, `tests/quux_prom.rs`) read the files in
# vendor/system-2001/ and make their own copies of the disk and the tree,
# so vendor/run/ is only for running `quux` by hand.
#
# The license is in the sources: the GNU Affero General Public License,
# version 3 or later, muir's own, with `NOTICE` giving their provenance.
#
# The release is pinned: its tag and every file's SHA-256 are below, and a
# newer release is taken by a commit that changes them, never by following
# GitHub's latest release. Every file is checked against its sum, whether
# just downloaded or already in place, and the disk again as it is
# decompressed. SYSTEM_FOR_QUUX_BASE names another place to download from.
#
# The number and the format say the machine, and both are checked before
# anything is unpacked: the sources must unpack to one `release-2NNN/`
# directory, a QUUX system's number, and the disk must be a dynamic VHD
# (`conectix` in its last 512 bytes) holding a GPT (`EFI PART` at the start
# of its sector 1, found through the VHD's block table), laid out as System
# 2001's release disk is: 853,359 blocks of 1024 bytes, a PAGE partition of
# 655,360 blocks (128MW), microcode 2001 in the current MCR1, "MCR1 UCADR
# 2001", and the band in the current LOD1, "LOD1 System 2001". The CADR's
# releases, numbered from 1000 and on a LABL pack, are refused. And the
# sources' `sys/ubin/promh.mcr` must be the release's PROM.
#
# Re-running this is safe: whatever is already in place is left alone, and
# nothing else in vendor/ is touched.
#
# usage: tools/fetch-system-for-quux.sh
# then:  quux --disk-pack vendor/run/release-2001-disk.vhd \
#             --file-root vendor/run/release-2001-root

set -eu
muir=$(cd "$(dirname "$0")/.." && pwd)
tag=release-2001
base=${SYSTEM_FOR_QUUX_BASE:-https://github.com/metebalci/muir-sys/releases/download/$tag}
rel=$muir/vendor/system-${tag#release-}
vrel=vendor/system-${tag#release-}
run=$muir/vendor/run
sources=$tag-sys.tar.gz
disk=$tag-disk.vhd
prom=$tag-promh.mcr
root=$run/$tag-root
assets="SHA256SUMS $sources $disk.gz $prom"

# The SHA-256 of each file, as the release's SHA256SUMS and notes give them.
sum_of() {
    case $1 in
    SHA256SUMS) echo b3b2d9c9a2d1151f8f643fc5a6b098c99076cf29babead0c75f3fc2966dab9b5 ;;
    release-2001-sys.tar.gz) echo 71006884aec9c5a82ec107cd3ade85ef0d5ea187f863eeea9d058cd23997e00c ;;
    release-2001-disk.vhd.gz) echo 6607f93a20854e52ab823eb145cd68de99d9566f2fab77dda45da9b130b2e71e ;;
    release-2001-disk.vhd) echo 9034de8b512f660c4e9506b7e5a6c70500b8f988bb91fc67bde3d588dc65f872 ;;
    release-2001-promh.mcr) echo 5917bae4a5e21806acd40fa9af7bc89a307e45f49cc974ee880c9e4ba002b9ca ;;
    esac
}

# The SHA-256 of a file, or of standard input with no argument, with
# whichever tool the system has.
sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$@" | cut -d' ' -f1
    else
        shasum -a 256 "$@" | cut -d' ' -f1
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
    # The sector bitmap: a bit per sector, rounded up to whole sectors.
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

mkdir -p "$rel" "$run"

for f in $assets; do
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

# The number: every member of the sources under one `release-2NNN/`.
top=$(tar -tzf "$rel/$sources" | sed 's|/.*||' | sort -u)
case $top in
release-2[0-9][0-9][0-9]) echo "     $vrel/$sources unpacks to $top/, a QUUX system's" ;;
*)
    echo "$vrel/$sources unpacks to $(echo "$top" | tr '\n' ' ')" >&2
    echo "and not to one release-2NNN/: it is not a system for QUUX" >&2
    refused=1
    ;;
esac

# The PROM: the sources' `sys/ubin/promh.mcr` is the release's.
if [ -z "$refused" ]; then
    if [ "$(tar -xOzf "$rel/$sources" "$top/sys/ubin/promh.mcr" | sha256)" = "$(sum_of "$prom")" ]; then
        echo "     $vrel/$sources holds the release's PROM as sys/ubin/promh.mcr"
    else
        echo "$vrel/$sources's sys/ubin/promh.mcr is not the release's PROM, $prom" >&2
        refused=1
    fi
fi

# The disk is checked as it comes out and not afterwards: a machine writes
# to the disk it runs on, so the file in place is only the release's until
# the first run.
if [ -f "$run/$disk" ]; then
    echo "have vendor/run/$disk"
else
    gunzip -c "$rel/$disk.gz" > "$run/$disk.part"
    if [ "$(sha256 "$run/$disk.part")" != "$(sum_of "$disk")" ]; then
        rm -f "$run/$disk.part"
        echo "$vrel/$disk.gz does not decompress to the disk the release published" >&2
        exit 1
    fi
    echo "     vendor/run/$disk checks"
    # The format: QUUX's disk is a dynamic VHD holding a GPT.
    if ! vhd_holds_gpt "$run/$disk.part"; then
        echo "vendor/run/$disk is not QUUX's disk" >&2
        refused=1
    elif ! release_disk_layout "$run/$disk.part"; then
        echo "vendor/run/$disk is not laid out as System 2001's release disk" >&2
        refused=1
    else
        echo "     vendor/run/$disk is a dynamic VHD holding a GPT, QUUX's disk:"
        echo "     853,359 blocks, PAGE 655,360 blocks, the band in the current LOD1"
    fi
    if [ -n "$refused" ]; then
        rm -f "$run/$disk.part"
    else
        mv "$run/$disk.part" "$run/$disk"
    fi
fi

if [ -n "$refused" ]; then
    echo "refused: nothing unpacked" >&2
    exit 1
fi

# The tarball's one top directory is left out, so the sources sit beside
# the tarball as System 100's do: vendor/system-2NNN/sys.
if [ -d "$rel/sys" ]; then
    echo "have $vrel/sys"
else
    tar xzf "$rel/$sources" -C "$rel" --strip-components=1
    echo "     $vrel/ holds the sources"
fi

# The file root, made once from copies, in a directory of its own that is
# renamed into place only when whole.
if [ -e "$root" ]; then
    echo "have vendor/run/$tag-root"
else
    rm -rf "$root.part"
    mkdir -p "$root.part/home/lispm"
    cp -Rp "$rel/sys" "$rel/site" "$root.part/"
    mv "$root.part" "$root"
    echo "made vendor/run/$tag-root: copies of sys/ and site/, and an empty home/lispm/"
fi

echo "done: $tag; run it with"
echo "  quux --disk-pack vendor/run/$disk --file-root vendor/run/$tag-root"
