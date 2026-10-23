# `tools/`

Scripts that make the committed fixtures in `data/`, from MIT's files in
`mit/` or, for QUUX's disks, with standard tools, five that fetch what is not committed, one that checks the
first lot still make what is committed, one that checks QUUX's Ethernet
checksum vectors, `quux-taps`, which installs the
taps QUUX's Ethernet link runs over, and `ethernet-frame-mover`, which
connects muir to one of them (below). None of them runs as part of the
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
| `fetch-dev-system-for-quux.sh` | `vendor/dev-system-for-quux/`, `vendor/run/dev-system-for-quux-disk.vhd` and `vendor/run/dev-system-for-quux-root/`: QUUX's system in development, muir-sys's rolling release `dev-system-for-quux`, checked against its own `SHA256SUMS` and replaced whole when the build changes. Never for tests |
| `fetch-system-100-for-cadr.sh` | `vendor/`: MIT's System 100 release --- the CADR's reference --- and a directory for muir to serve it from. Nothing in `vendor/` is committed |
| `fetch-handover-2002.sh` | `ref/band-2002-2999694/`: muir-sys's hand-over of System 2002 on QUUX revision 16, the pre-release `handover-2002-2999694`, pinned by tag and every file's SHA-256; both its disks, the band's and the high band's, decompressed and checked to be laid out as QUUX's release disk (853,359 blocks, a 128MW `PAGE`, microcode 2002 in the current `MCR1` and the band in the current `LOD1`), and the sources' `sys/ubin/` files copied beside them, their `promh.mcr` held to the pre-release's PROM 2002. Until System 2002 is released; `tests/system_2002.rs` reads it |
| `fetch-system-for-cadr.sh` | `vendor/system-1003/` and `vendor/run/release-1003-pack.img`: the CADR's current muir-sys release, System 1003, from the `release-1003` release of metebalci/muir-sys, pinned by tag and every file's SHA-256 |
| `cadr-netlist.sh`, `busint-netlist.sh`, `cadrm-netlist.sh`, `cadrio-netlist.sh`, `cadrdc-netlist.sh`, `simpletv-netlist.sh`, `lispmtv-netlist.sh`, `dm-netlist.sh` | `data/<BOARD>.netlist`, one board each: MIT's drawings read with `soap4`, then reconciled with MIT's wire list by `examples/reconcile.rs` |
| `check-netlists.sh` | nothing. It runs all eight of those and says whether each committed netlist is still what its script makes, putting the committed files back afterwards. CI runs it on every push |
| `newdsk-proms.sh` | `data/newdsk-d0?.prom`, the disk controller's control store, assembled from `mit/cadrdc/newdsk.31` by `examples/dcmicro.rs` |
| `trident-tables.sh` | `data/trident-connectors.txt` and `data/trident-bus.txt`, by `examples/trident-tables.rs` |
| `quux-disk-fixtures.sh` | `data/quux-disk*`, QUUX's disk raw, as a fixed and a dynamic VHD, and grown: with sgdisk, dd, qemu-img and qemu-io, not muir. `QEMU_IMG` and `QEMU_IO` name the qemu tools when they are not on the path |
| `ethernet-vectors.py` | `tests/ethernet_vectors.txt`, QUUX's Ethernet checksum and type vectors: `generate LINUX` makes it, the named edge cases plus the frames Linux made, read from `LINUX`. `check tests/ethernet_vectors.txt` works every line out again and exits non-zero on a disagreement. Python's standard library only, written from RFC 791, 768, 792, 9293 and 1071, sharing no code with muir; `tests/ethernet_vectors.rs` runs every vector through the device's rules |
| `quux-taps` | nothing in the tree. Run once as root, it installs a pool of taps for QUUX's Ethernet link on the computer it runs on, and checks first how the host side behaves; see below |
| `ethernet-frame-mover` | nothing in the tree. It takes a tap of that pool and moves Ethernet frames between it and muir's `--ethernet-socket`, as an unprivileged user; see below. Python's standard library only |

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
are muir's defaults, so one machine on `quux0` needs no flags. For a
second machine, the Ethernet frame mover prints the two flags to give
`quux` for the tap it took. Before installing, check that `ip route`
shows nothing else in 169.254.100 to 169.254.163.

These taps are **host-only**: each is a link between the host and the
machine behind it, and nothing else. Every tap's host side has the MAC
address `02:00:00:00:00:02` and an MTU of 1,500. On each tap, before it
comes up, IPv6 is turned off (`net.ipv6.conf.quuxN.disable_ipv6=1`), so
Linux sends no IPv6 on the link at all. `arp_ignore` is set to 2: Linux
answers an ARP request only when its target is this tap's own address and
its sender is on this tap's /30. `rp_filter` is set to 1, strict: each
tap carries a prefix of its own, so the route back to a packet's sender
names the tap it arrived on. For both, Linux uses the larger of
`conf.all` and the tap's own value, so a host whose `conf.all.rp_filter`
is 2 still checks loosely. `proxy_arp` and `forwarding` are set to 0: a
host-only tap answers for nobody else and forwards nothing.

### QUUX on the LAN: `--ip-address`

    sudo tools/quux-taps --ip-address <a free LAN address>/<the LAN's prefix>

puts the machine behind `quux0` on the host's LAN, as a computer of its
own: for example `--ip-address 192.0.2.50/24` on a LAN of 192.0.2.0/24.
The address is `quux`'s flag of the same name, with its meaning, and
must be one nobody else on the LAN has: outside the router's DHCP pool,
or reserved there. `quux0` is then a **LAN tap**, and `quux1` up stay
host-only. The host forwards between its LAN and `quux0`, and answers ARP
on the LAN for the machine's address alone; the Ethernet frame mover then
prints the flags to give `quux`, the LAN address and the LAN's router.

The install finds the host's LAN interface: the one interface, not a tap
of the pool, with a route on the LAN holding that address at that prefix
(`ip -4 route` shows it). It refuses, before it changes anything, when
there is none or more than one, when the address is that interface's own,
when it is link-local (169.254.x.y, which is a host-only tap's), when its
subnet holds link-local addresses, and when `quux` would refuse it: a
prefix outside 1 to 30, 0.x.x.x, 127.x.x.x, from 224.0.0.0 up, and the
subnet's network and broadcast addresses.

On `quux0`, after the pool is up, and before any forwarding is on: the
link-local rule (below); then IPv6 off; `arp_ignore` 0, so the
machine reaches the host's own LAN address; `arp_filter` 1, so the host
answers the machine's ARP for its own addresses only on `quux0`;
`rp_filter` 1; `proxy_arp` 1, so the machine's ARP for any address on the
LAN is answered by the host; `forwarding` 1; the proxy delay 0, since
Linux would otherwise hold each first answer up to 0.8 s; **no address**;
`quux0` up; and a route for the machine's address alone through it. On
the LAN interface: `forwarding` 1 and the proxy delay 0, their old values
kept in `/run/quux-taps.lan`; then one proxy entry, for the machine's
address, and any older proxy entry there removed. Nothing else: never
the global `ip_forward`, which would also forward everything the pool's
taps and the host's other interfaces send, and never `proxy_arp` on the
LAN interface. The stop removes the proxy entry, puts the LAN interface's
two values back, removes the link-local rule and deletes the tap. If the LAN tap cannot be made, the
pool still works, `quux0` is down, and `systemctl status
quux-taps.service` says why.

The install refuses an address that is already in use on the LAN: one
of the host's gateways (the LAN's router), or one the host's neighbour
table shows another machine answering for. The host would otherwise
answer ARP for that address on the LAN and take its traffic.

**The link-local rule.** With a LAN tap, the host drops every packet
that arrives on its LAN interface, or on the LAN tap, for a link-local
destination (169.254.x.y). Two routing rules do it, of type blackhole, at
priority 1169. That is after the local table's rule at priority 0, so the
host's own link-local addresses still answer, and its own traffic is not
touched, since the rules name an input interface. Routers never forward
link-local addresses (RFC 3927, section 2.7). So nothing on the LAN, and
no machine on a LAN tap, reaches a host-only tap's machine through the
host. The rules need the kernel's policy routing
(`CONFIG_IP_MULTIPLE_TABLES`); without it, the LAN tap is refused and
undone. Every start first removes any rule at priority 1169, which
belongs to these two alone, and a start with no LAN tap adds none.

**What forwarding on the LAN interface still opens.** Linux decides
forwarding by the interface a packet arrives on, so with a LAN tap the
host forwards what arrives on its LAN interface for any other destination
to every route it has: back out of the LAN interface, and into the host's
other interfaces (a VPN, a container or virtual machine bridge).

So a LAN tap is for a host that stays on its LAN. The LAN interface is
fixed at install, and the host answers ARP for the machine's address on
whatever network that interface is on, so install again without
`--ip-address` before taking the host elsewhere. Turning forwarding on
also turns large receive offload off on the LAN interface, and the stop
does not turn it back on; on some network cards that change resets the
link once.

### The check: `--check`

    sudo tools/quux-taps --check

runs the host side in throwaway network namespaces that stand in for the
host, a LAN switch, two LAN machines, a machine behind a LAN tap and one
behind a host-only tap, with documentation addresses, and touches none of
the host's own interfaces. It sets the stand-in host up with the
script's own steps and prints seven lines, each a pass or a fail: the
settings; what the host answers ARP for, before and after; that each
answer comes within 50 ms; that the LAN and the machine reach each other
both ways, by ping and by TCP; that no LAN broadcast reaches the machine
and the machine's MAC address never reaches the LAN; that a host-only
tap stays host-only; and that the stop undoes it all. It exits non-zero on
any fail. Run from this tree, it writes its log to `notes/runs/`. Every
install runs it first and installs nothing when it fails, leaving the pool
already there as it was. It wants `python3` and `ping`.

**What it changes on the system:**

- `/usr/local/sbin/quux-taps`, a copy of the script;
- `/etc/systemd/system/quux-taps.service`, a oneshot unit, enabled and
  started. At boot it runs `quux-taps --up`, which makes the taps, waits for
  udev to settle, and then sets each one up as above. At shutdown, and when
  stopped, it runs `quux-taps --down`, which deletes them. With a LAN tap
  the unit is ordered after the LAN interface's device, and does not wait
  for its address;
- with a LAN tap, while the unit runs, `/run/quux-taps.lan`, and the
  settings above on the LAN interface.

Nothing else: no firewall rule, no `sysctl.d` file, no network manager's
configuration. `--uninstall` stops and disables the unit, deletes any tap
left, and removes both files. Installing again replaces the pool: the old
unit is stopped first, and the new one makes the pool afresh.

The pool is a systemd unit rather than systemd-networkd's `.netdev` and
`.network` files because the unit works whichever program manages the
rest of the network, NetworkManager or networkd, and does the same thing
at boot as by hand. It needs systemd and the `ip` command of iproute2.

`tests/quux_taps.rs` holds the dry run's commands and the unit, line for
line, for one tap, two, 32 and 64, and for a LAN tap with the pool behind
it, the stop's commands, the refusals, and how `--check` judges each of
its seven lines; the namespaces themselves need root, so the check is run
by hand.

## `ethernet-frame-mover`

The Ethernet frame mover puts a machine behind a tap: a tap of
`quux-taps`'s pool at one end, and at the other a UNIX stream socket that
muir's `--ethernet-socket` connects to, in its plain mode. muir connects
as a client, so the mover runs first:

    tools/ethernet-frame-mover quux.sock
    quux --ethernet-socket quux.sock

Once it holds a tap and listens at the socket, it prints one line: the
tap, and the two flags to give `quux` for it, for example

    quux1 --ip-address 169.254.101.2/30 --ip-gateway 169.254.101.1

It reads them from the kernel, from the tap's own address: that is the
host side and the machine's gateway, and the machine's address is the
/30's other host address. For `quux0` they are muir's defaults, so the
first machine needs no flags at all; a second mover takes the next free
tap, and its `quux` takes the flags the line names.

On a LAN tap (`quux-taps --ip-address`, above) the line names the LAN
address and the LAN's router, for example

    quux0 --ip-address 192.0.2.50/24 --ip-gateway 192.0.2.1

read from the kernel as well: the address from `quux0`'s route, the
prefix from the LAN interface's route, and the gateway from the default
route on the LAN interface, which must be on the machine's subnet, or the
mover refuses and names `--ip-gateway`. Before it takes a LAN tap, it
reads the host side's settings back, without privilege. When one is gone,
as when a network manager has reconfigured the LAN interface or something
has set the global `ip_forward`, it refuses, says which, and names the
fix: `sudo systemctl restart quux-taps.service`.

`--tap NAME[,NAME...]` names the taps to try, in order; without it the
mover tries the pool from `quux0` up. A tap another program holds is
passed for the next. A named tap that is taken, missing or another user's
is refused, saying which and why, and no tap outside the list is tried.
When every tap tried was taken it exits with status 75, so that a script
may wait and try again; another refusal is 1, and a usage error 2.

The mover moves every Ethernet frame, whatever it holds. When the machine
sends ARP or IPv4 from another address than its tap's, it warns once for
the connection and names the flags to use; the tap itself answers no ARP
request from an address off its /30 (`arp_ignore` 2), and muir refuses to
resume a checkpoint under other addresses than its own. One machine is
connected at a time. When it closes, the mover keeps its tap and waits for
the next; while none is connected, what the tap gives is dropped. ^C ends
it, and the kernel frees the tap.

`tests/ethernet_frame_mover.rs` holds the line, a LAN tap's line and its
refusals against a stand-in for the kernel's tables, the warning, the
framing both ways and the refusals that need no tap, against a socket pair
in place of the tap; it runs `python3`.

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
