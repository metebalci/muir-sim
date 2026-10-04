# `tools/`

Scripts that make the committed fixtures in `data/`, from MIT's files in
`mit/` or, for QUUX's disks, with standard tools, five that fetch what is not committed, and one that checks the
first lot still make what is committed. None of them runs as part of the
build --- CI runs `check-netlists.sh` on every push, which is what says the
fixtures and the drawings have not drifted apart. `data/README.md` says
which test holds each output to its source.

**The netlist scripts want a C compiler**, because each builds `soap4/` as it
runs. Nothing else here does, and neither does muir: the fixtures these make
are committed, so building, testing and running the simulator never reaches
for one. `xcode-select --install` on macOS, `build-essential` on Debian and
Ubuntu, `gcc` on Fedora --- and only when a netlist has to be made again.

| Script | Makes |
|---|---|
| `fetch-system-for-quux.sh` | `vendor/system-2000/`, `vendor/run/release-2000-disk.vhd` and `vendor/run/release-2000-root/`: QUUX's current release, System 2000, from the `release-2000` release of metebalci/muir-sys, pinned by tag and every file's SHA-256, with the machine's host folder made from copies of its `sys/` and `site/` and an empty `home/lispm/` |
| `fetch-handover-2001.sh` | `ref/band-2001-81b3973/`: muir-sys's release hand-over of System 2001, unreleased, from the pre-release `handover-2001-81b3973` of metebalci/muir-sys, pinned by tag and every file's SHA-256, its disk decompressed beside it and checked to be laid out as System 2001's release disk (853,359 blocks, a 128MW `PAGE`, the band in the current `LOD1`) and its sources' `sys/ubin/` microcode and PROM files copied out, for revision 13's tests until System 2001 is released |
| `fetch-dev-system-for-quux.sh` | `vendor/dev-system-for-quux/`, `vendor/run/dev-system-for-quux-disk.vhd` and `vendor/run/dev-system-for-quux-root/`: QUUX's system in development, muir-sys's rolling release `dev-system-for-quux`, checked against its own `SHA256SUMS` and replaced whole when the build changes. Never for tests |
| `fetch-system-100-for-cadr.sh` | `vendor/`: MIT's System 100 release --- the CADR's reference --- and a directory for muir to serve it from. Nothing in `vendor/` is committed |
| `fetch-system-for-cadr.sh` | `vendor/system-1002/` and `vendor/run/release-1002-pack.img`: the CADR's current muir-sys release, System 1002, from the `release-1002` release of metebalci/muir-sys, pinned by tag and every file's SHA-256 |
| `cadr-netlist.sh`, `busint-netlist.sh`, `cadrm-netlist.sh`, `cadrio-netlist.sh`, `cadrdc-netlist.sh`, `simpletv-netlist.sh`, `lispmtv-netlist.sh`, `dm-netlist.sh` | `data/<BOARD>.netlist`, one board each: MIT's drawings read with `soap4`, then reconciled with MIT's wire list by `examples/reconcile.rs` |
| `check-netlists.sh` | nothing. It runs all eight of those and says whether each committed netlist is still what its script makes, putting the committed files back afterwards. CI runs it on every push |
| `newdsk-proms.sh` | `data/newdsk-d0?.prom`, the disk controller's control store, assembled from `mit/cadrdc/newdsk.31` by `examples/dcmicro.rs` |
| `trident-tables.sh` | `data/trident-connectors.txt` and `data/trident-bus.txt`, by `examples/trident-tables.rs` |
| `quux-disk-fixtures.sh` | `data/quux-disk*`, QUUX's disk raw, as a fixed and a dynamic VHD, and grown: with sgdisk, dd, qemu-img and qemu-io, not muir. `QEMU_IMG` and `QEMU_IO` name the qemu tools when they are not on the path |

## `soap4/`

The reader for MIT's drawings, which are SUDS binaries. It is C, built by the
netlist scripts as they run, and it is the one part of this repository that
is not this project's own work throughout:

- `soap4.c` was written by Mete Balci in 2026 for `ams/cadr4`, following Brad
  Parker's `soap.c` of October 2004, whose header it keeps. It came here from
  `ams/cadr4`, which is under the GNU Affero General Public License, version
  3 or later.
- `unpack4.c` is `ams/cadr4`'s version of John Wilson's `unpack.c`, written
  in 1993 and separated from his `DUMP.C` in 1998, which reads ITS files
  stored in Alan Bawden's evacuated format; its header keeps John Wilson's.
  It came here the same way, under the AGPL through `ams/cadr4`.

**Neither original carries a license**, and that was checked at the source
rather than assumed from the copies here. Both are published in
`lisper/cpus-cadr`, Brad Parker's own repository: `suds/soap.c` and
`suds/unpack.c` there give authorship and a description in their headers and
state no terms, and the repository holds no license file. So for whatever
survives of the two originals in these files there is no grant to point at
--- not a permissive one, and not a refusal either. The terms were never
written down.

`ams/cadr4` released its versions under the AGPL and this repository carries
them on. That covers the work done in each --- the rewrite for cadr4, and
the changes listed in `soap4.c`'s own header --- and it cannot make a grant
for what came before it, which is why the copyright lines at the top of both
files name the original authors beside this project's. Anyone redistributing
these two files should know the position is unsettled rather than settled in
their favor. It is the one open licensing question in this repository.

Everything else in this directory is this project's, under the repository's
license.
