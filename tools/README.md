# `tools/`

Scripts that make the committed fixtures in `data/`, from MIT's files in
`mit/` or, for QUUX's disks, with standard tools, four that fetch what is not committed, one that checks the
first lot still make what is committed, one that checks QUUX's Ethernet
checksum vectors, and `quux-taps`, which installs the
taps QUUX's Ethernet link runs over (below). None of them runs as part of the
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
| `fetch-system-for-quux.sh` | `vendor/system-2001/`, `vendor/run/release-2001-disk.vhd` and `vendor/run/release-2001-root/`: QUUX's current release, System 2001, from the `release-2001` release of metebalci/muir-sys, pinned by tag and every file's SHA-256, its disk checked to be laid out as System 2001's release disk (853,359 blocks, a 128MW `PAGE`, the band in the current `LOD1`) and its sources' `sys/ubin/promh.mcr` to be the release's PROM, with the machine's host folder made from copies of its `sys/` and `site/` and an empty `home/lispm/` |
| `fetch-dev-system-for-quux.sh` | `vendor/dev-system-for-quux/`, `vendor/run/dev-system-for-quux-disk.vhd` and `vendor/run/dev-system-for-quux-root/`: QUUX's system in development, muir-sys's rolling release `dev-system-for-quux`, checked against its own `SHA256SUMS` and replaced whole when the build changes. Never for tests |
| `fetch-system-100-for-cadr.sh` | `vendor/`: MIT's System 100 release --- the CADR's reference --- and a directory for muir to serve it from. Nothing in `vendor/` is committed |
| `fetch-system-for-cadr.sh` | `vendor/system-1003/` and `vendor/run/release-1003-pack.img`: the CADR's current muir-sys release, System 1003, from the `release-1003` release of metebalci/muir-sys, pinned by tag and every file's SHA-256 |
| `cadr-netlist.sh`, `busint-netlist.sh`, `cadrm-netlist.sh`, `cadrio-netlist.sh`, `cadrdc-netlist.sh`, `simpletv-netlist.sh`, `lispmtv-netlist.sh`, `dm-netlist.sh` | `data/<BOARD>.netlist`, one board each: MIT's drawings read with `soap4`, then reconciled with MIT's wire list by `examples/reconcile.rs` |
| `check-netlists.sh` | nothing. It runs all eight of those and says whether each committed netlist is still what its script makes, putting the committed files back afterwards. CI runs it on every push |
| `newdsk-proms.sh` | `data/newdsk-d0?.prom`, the disk controller's control store, assembled from `mit/cadrdc/newdsk.31` by `examples/dcmicro.rs` |
| `trident-tables.sh` | `data/trident-connectors.txt` and `data/trident-bus.txt`, by `examples/trident-tables.rs` |
| `quux-disk-fixtures.sh` | `data/quux-disk*`, QUUX's disk raw, as a fixed and a dynamic VHD, and grown: with sgdisk, dd, qemu-img and qemu-io, not muir. `QEMU_IMG` and `QEMU_IO` name the qemu tools when they are not on the path |
| `ethernet-vectors.py` | `tests/ethernet_vectors.txt`, QUUX's Ethernet checksum and type vectors: `generate LINUX` makes it, the named edge cases plus the frames Linux made, read from `LINUX`. `check tests/ethernet_vectors.txt` works every line out again and exits non-zero on a disagreement. Python's standard library only, written from RFC 791, 768, 792, 9293 and 1071, sharing no code with muir; `tests/ethernet_vectors.rs` runs every vector through the device's rules |
| `quux-taps` | nothing in the tree. Run once as root, it installs a pool of taps for QUUX's Ethernet link on the computer it runs on; see below |

## `quux-taps`

A tap is the host's end of a machine's Ethernet link. `quux-taps` makes a
pool of them, `quux0` to `quux<N-1>`, owned by one user, and installs them
so they are made again at every boot. It runs once, as root; after that the
user opens a tap with no privilege at all. A process takes a tap by name; a
tap already held refuses a second open, so the next one is tried, and a tap
is free again when its holder exits.

    sudo tools/quux-taps                # quux0 and quux1, for the user running sudo
    sudo tools/quux-taps --count 1      # one tap: enough for one machine
    sudo tools/quux-taps --count 4      # a machine, and checks beside it
    tools/quux-taps --count 4 --dry-run # what it would do; changes nothing
    sudo tools/quux-taps --uninstall    # remove all of it

One tap is enough for one machine, and two to four leave room for a check
running beside it; `--count` takes 1 to 64. `--user U` names another
owner. Without root it refuses to run, except with `--dry-run`, and it
refuses an argument it cannot use before it does anything.

Each tap has its own address. The host side of `quuxN` is
169.254.1NN.1 and the machine behind it is 169.254.1NN.2, with NN the
tap's index N in two digits: quux1's are 169.254.101.1 and .2. quux0's
are muir's defaults, so one machine needs no address flags. For a
second machine, the Ethernet frame mover prints the two flags to give
`quux` for the tap it took. Before installing, check that `ip route`
shows nothing else in 169.254.100 to 169.254.163.

Every tap's host side has the MAC address `02:00:00:00:00:02` and an MTU
of 1,500. On each tap, before it comes up, IPv6 is turned off
(`net.ipv6.conf.quuxN.disable_ipv6=1`), so Linux sends no IPv6 on the link
at all. `arp_ignore` is set to 2: Linux answers an ARP request only when
its target is this tap's own address and its sender is on this tap's /30.
`rp_filter` is set to 1, strict: each tap carries a prefix of its own, so
the route back to a packet's sender names the tap it arrived on. For both,
Linux uses the larger of `conf.all` and the tap's own value, so a host
whose `conf.all.rp_filter` is 2 still checks loosely.

**What it changes on the system:**

- `/usr/local/sbin/quux-taps`, a copy of the script;
- `/etc/systemd/system/quux-taps.service`, a oneshot unit, enabled and
  started. At boot it runs `quux-taps --up`, which makes the taps, waits for
  udev to settle, and then sets each one up as above. At shutdown, and when
  stopped, it runs `quux-taps --down`, which deletes them.

Nothing else: no firewall rule, no `sysctl.d` file, no network manager's
configuration. `--uninstall` stops and disables the unit, deletes any tap
left, and removes both files. Installing again replaces the pool: the old
unit is stopped first, and the new one makes the pool afresh.

The pool is a systemd unit rather than systemd-networkd's `.netdev` and
`.network` files because the unit works whichever program manages the
rest of the network, NetworkManager or networkd, and does the same thing
at boot as by hand. It needs systemd and the `ip` command of iproute2.

`tests/quux_taps.rs` holds the dry run's commands and the unit, line for
line, for one tap, two, 32 and 64, and the refusals.

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
