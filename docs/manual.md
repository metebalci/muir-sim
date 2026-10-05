# muir manual

What muir does when you run it, every flag it takes, and what it is doing
underneath. The site, with the pictures and the short way in, is at
[muir.metebalci.com/simulator/](https://muir.metebalci.com/simulator/).

This file is the program itself. Beside it:

- [Making a pack](diskpack.md) --- `diskpack`, the third binary: a pack of
  one's own, its partitions, and bands loaded and dumped
- [The machine it models](machine.md) --- the boards, the two buses, and
  what each one reaches outside muir
- [How the engines work](engines.md) --- `micro`, `rtl` and `chip`, why the
  level matters, and what each engine models
- [Where the netlists come from](netlists.md) --- MIT's own files, how a
  board becomes a netlist, and what each board is checked against
- [The Chaosnet board](chaosnet.md) --- the interface that shares the I/O
  board, the cable it talks on, and what muir does at each engine
- [Sources and attribution](sources.md) --- what this is built on, how it
  was written, the license, and the name

## Installing

muir needs a Rust toolchain and nothing else: the version is pinned in
`rust-toolchain.toml`, there are no crate dependencies, and everything the
build and the tests read is committed, so `cargo test` passes on a fresh
clone with nothing fetched.

```text
git clone https://github.com/metebalci/muir-sim
cd muir-sim
cargo build --release
tools/fetch-system-for-quux.sh
tools/fetch-system-100-for-cadr.sh
```

The machines boot from a disk or a pack, which is not part of the
repository. Four scripts fetch them into `vendor/`, where the tests look,
and check every file against its SHA-256 sum:

| Script | Fetches | Puts |
|---|---|---|
| `tools/fetch-system-for-quux.sh` | QUUX's current release of [muir-sys](https://github.com/metebalci/muir-sys), `release-2001`: System 2001 | `vendor/system-2001/`, the disk `vendor/run/release-2001-disk.vhd`, and the machine's host folder `vendor/run/release-2001-root/` |
| `tools/fetch-dev-system-for-quux.sh` | QUUX's system in development, muir-sys's rolling release `dev-system-for-quux`, replaced with each build | `vendor/dev-system-for-quux/`, `vendor/run/dev-system-for-quux-disk.vhd`, `vendor/run/dev-system-for-quux-root/` |
| `tools/fetch-system-100-for-cadr.sh` | MIT's System 100, the CADR's reference | `vendor/system-100-0/`, the pack `vendor/run/disk-sys-100-0.img`, and `vendor/run/file-root/` |
| `tools/fetch-system-for-cadr.sh` | the CADR's current muir-sys release, `release-1003`: System 1003 | `vendor/system-1003/`, the pack `vendor/run/release-1003-pack.img` |

The stable scripts are pinned: each holds its release's tag and the SHA-256
of every file it takes, so the bytes the tests were written against stay
the bytes, and a newer release comes with a commit that changes them. The
rolling release is not pinned, so its script checks its files against the
release's own `SHA256SUMS`, downloaded before them and again after, and
replaces what it fetched last time only when all of them agree; it is for
trying the newest system, never for the tests. Each script also checks
that what it fetched is for its machine before it unpacks anything: QUUX's
sources unpack to `release-2NNN/` or `dev-system-for-quux/` and its disk is
a VHD holding a GPT; the CADR's unpack to `release-1NNN/` and its pack
begins with `LABL`.

The machine writes to its disk and under its host folder, so both are
copies: the scripts keep what they downloaded beside them. The stable
scripts leave a disk or a folder already in place alone; the rolling
release's replaces its disk, and its folder's `sys/` and `site/`, only with
a new build, and keeps the home. QUUX's host folder holds copies of the
release's `sys/` and `site/`, where the band's `SYS:` is, and an empty
`home/lispm/`, the login's home.

System 100 comes from muir-sim's own GitHub release
[`system-100-0`](https://github.com/metebalci/muir-sim/releases/tag/system-100-0):
the pack byte for byte as [upstream](https://tumbleweed.nu/lm-3/) publishes
it, and the script says exactly what it fetched and from where. Everything
the four fetch is under the AGPL, muir-sim's own license. Without them,
every test needing a disk or a pack skips and says so. Windows is untested;
the scripts are POSIX shell, so use WSL.

A CADR band also wants its file and time host, which is not muir ---
[the Chaosnet](#the-chaosnet) says why --- and
[ozd](https://github.com/metebalci/ozd) is built and run beside it, as its
own README says. QUUX needs none: its file device serves its host folder.

## Running it

muir builds to two executables that are machines: [`quux`](#quux), the
CADR evolved, and `cadr`, MIT's CADR as built. **The executable is the
machine**: each takes only the flags that mean something on its own.

QUUX boots its release's disk, with its host folder as the file device's
root:

```text
target/release/quux --disk-pack vendor/run/release-2001-disk.vhd \
    --file-root vendor/run/release-2001-root
```

Run `cadr` with no flags and it starts an `rtl` machine, presses the boot
button, and runs the boot PROM with no pack in the drive, which the boot
waits on for ever. A pack is named: `--disk-pack
vendor/run/disk-sys-100-0.img`, where `tools/fetch-system-100-for-cadr.sh`
puts the System 100 pack, and with it `--chaos-address 3050`, which is that
band's own address, `--chaos-udp`, which is the cable, and
`--chaos-udp-peer 3060@<where ozd is>`, which is where that band looks for
its files and the date. ozd has the protocol's own port, 42042, so a
machine on the same computer takes another.

```text
target/release/cadr
```

One engine at a time: `--micro`, `--rtl` or `--chip`, and `--rtl` is what
you get without one. A run goes on until a stop, a halt or ^C.

Every run says what it is before it starts a microcycle, one line a thing,
on stderr. Nothing here is hidden state: if a run behaves oddly, the reason
is usually in this block.

```text
$ cadr --disk-pack vendor/run/disk-sys-100-0.img --chaos-address 3050 \
       --chaos-udp 42043 --chaos-udp-peer 3060@127.0.0.1:42042 --stop-after 2000000
engine: rtl
memory: 32 boards, 2 MW
pack: vendor/run/disk-sys-100-0.img in unit 0
chaosnet: 3050
chaosnet over udp: 127.0.0.1:42043, 3060 at 127.0.0.1:42042
terminal: vnc://127.0.0.1:5900 --- RFB, no password
debug cable: DBGIN listening at 127.0.0.1:7661
stop: after 2000000 microcycles
^C holds the machine at the prompt; help lists muir's commands
```

Paths under the directory muir was run from are written relative to it, so
the block stays readable wherever the tree lives.

At the end it reports what it did: microcycles, wall time, the rate, and
that rate against the machine's own microcycle, the CADR's 145 ns or
QUUX's `sync` ticks --- `2.42x hardware` is faster than the machine,
`hardware/3827` is that many times slower.

**That ratio is microcycles against microcycles, and the disk is outside
it.** With the disk controller as a behavioral model --- what `micro` and
`rtl` always have, and what `chip` runs on `--disk-controller model` --- its
transfers complete inside the store to `START` and its seeks take no time.
The machine it stands for spent milliseconds on a seek and spent them
running the microcode's polling loop, so a 55 ms seek on real hardware is
about 380,000 microcycles that muir never executes. A program that seeks
therefore finishes further ahead of the hardware than the ratio says, by an
amount that depends on the program and is not quantified here.

**On `chip`, where the controller is MIT's board by default, it inverts:**
the drive takes its own time, the polling loop is executed through every
gate on the board, and a program that waits on the disk is slower than the
ratio rather than faster. **Measured, on the System 100 pack: about 4.9
times slower.** The boot PROM alone reaches `PROM-DISABLE` in 1.40-1.425
million microcycles with the behavioral controller and in 6.90-6.925 million
with the netlist one --- the same work, and nearly five times the
microcycles, because the extra ones are the PROM sitting in `DISK-WAIT`
while the drive turns. In wall clock that was about fifteen minutes against
108.

**A run that touches no pack pays almost nothing for the netlist controller,
and a run that boots one pays days.** Measured over 2,000 microcycles that
touch no pack, 0.97 s against 0.94 s, about 3%: the board is on the
backplane, and nothing asks it to move a head. Booting is the other end of
it --- System 100 came up through this controller on 12 September 2026 in 2
days 14 hours and 301 million microcycles, held between about 1,200 and
2,000 microcycles a second throughout, of which 51 hours was the cold boot's
copy of all 21,342 pages of the band at about 330 pages an hour. That is not
the engine being slow at gates; it is the controller's sequencer waiting on
the drive's clocks and on its own delay lines, which is what the board does
and cannot be untimed the way the model can. `--disk-controller model` is
how a `chip` run that does not care about the disk is made quick.

```text
rtl      2000000 microcycles    0.194 s     10328518 microcycles/s    1.50x hardware
       ran out at 2000000; PC 27501
```

Addresses in the flags and in what muir prints are octal, as MIT writes
them.

### `quux`

`quux` is QUUX, the CADR evolved, revision 13 by default. Its word is 40
bits, the tag `<39:32>` over the field `<31:0>`; its map is two levels over
1024-word pages, with 28-bit virtual and physical addresses; its PDL buffer
is 16K words to the CADR's 1K; ALU functions 42 and 43 multiply and divide
in one instruction each, where the CADR takes a step per bit; and it has
clocks of its own, a microsecond clock in the processor and three interval
timers on its register page, timer 0 the 60 Hz tick, where the CADR's clock
is the display's vertical interrupt and the others are on the I/O board.
The rest of it is the CADR's. It boots from its own PROM and needs
microcode that knows it. [QUUX](quux.md) has the whole of the difference.

With `MUIR_QUUX_REVISION=14` in its environment `quux` runs revision 14
instead, a page table walked by hardware behind a TLB ([revision
14](quux.md#revision-14-a-page-table-behind-a-tlb)). Unset or `13`, it
runs revision 13; any other value is refused at the start. `cadr` does not
read it.

It runs on `micro` and `rtl`: `chip` is the CADR's boards as MIT drew them.
A checkpoint carries its machine, and the other executable refuses it,
naming the one that resumes it --- `cadr: --resume q.chk is quux's, not
cadr's: quux --resume q.chk`. The start block says `machine: quux`.

## The command line

Every flag `cadr` and `quux` take, in the order their `--help` lists them:
the engine first, then the rest by name. Most are both executables', with
the same meaning on each. **A flag that is one machine's alone says whose**,
`cadr` only or `quux` only, and the other executable refuses it by name,
saying which takes it --- `quux: --chip is cadr's, not quux's` --- as it
refuses a flag of neither: `cadr: --foo is not a flag of cadr`. Each
`--help` lists its own flags and no other's. A flag that only means
something on some engines says which.

### `--micro | --rtl | --chip`

The engine: microinstruction, register transfer, or chip level. [How the
engines work](engines.md) says what each computes. `--chip` is `cadr` only:
it is the CADR's boards as MIT drew them, and QUUX has no netlist.

Default: `--rtl`.

### `--chaos-address <address>`

This machine's Chaosnet address: the sixteen address switches on the I/O
board, the bits in octal, `3050`, or subnet:host with each in octal, `6:50`
--- the same number, subnet in the high byte.

**The switches are set whether or not anything is plugged in.** This is the
address alone: `--chaos-udp` is the cable, and a machine without it sends
nothing however its switches read.

Which address a run wants is its band's own, out of its host table: System
100's puts this machine at `3050` and its file and time host at `3060`. muir
is not that host --- a CADR had no file or time server in it --- so the host
is another program on the network, named with `--chaos-udp-peer`. A host answering anywhere else is one the
band never calls, and the machine then boots but stops to ask for the date
and reaches no files.

Default: `177001`, on subnet 376 --- the Chaosnet's private, non-routable
range --- and so no band's. muir models the CADR and not one distribution of
it, and a default out of one band's host table would be the wrong default
for every other; a default in the private range cannot collide with an
address a real Chaosnet allocated. A run that wants its band to reach a host
names the band's address: `--chaos-address 3050` for the System 100 pack.

### `--chaos-trace`

Every Chaosnet packet and frame on the cable, to stderr. What to reach for
when a lashup goes quiet: it shows whether the machine is still talking.

Default: off.

### `--chaos-udp [<endpoint>]`

Where the Chaosnet cable is on the network: **Chaosnet over UDP**, the
encapsulation `cbridge`, `usim`, `klh10`, `ozd` and the live Chaosnet hosts
speak. Ordinary UDP, so it needs no privileges and it crosses a NAT.

Nothing, a port, an address or address:port. A bare port is on the loopback,
which is where a server nobody authenticates belongs, so reaching another
host means naming an address to listen on. Two muirs on one host are two
peers on the loopback and want no other transport between them.

The frame is `cbridge`'s: one Chaos packet behind a four-byte header, every
16-bit word most significant byte first, and a trailer of three words ---
the address on this subnet the frame is for, the sender's own, and an
Internet checksum over everything before it. A frame whose checksum is wrong
is dropped, as `cbridge` drops it; `--chaos-trace` says so. The check word
the CADR's own hardware makes is the modeled cable's and is not what the
datagram carries.

muir is a leaf and not a router: a frame goes out over UDP only when a
station of this machine put it on the cable, so what arrives from one peer
is never carried on to another. A `cbridge` beside muir is what routes, and
`--chaos-udp-default-peer` is how a frame reaches it.

Default: `127.0.0.1:42042`, the protocol's own port, when `--chaos-address`
is given; off with neither flag.

### `--chaos-udp-default-peer <host>[:<port>]`

Where a frame goes whose destination no `--chaos-udp-peer` named: the route
of last resort, and what lets a `cbridge` beside muir carry the traffic on
to the wider Chaosnet. Naming that bridge as a peer does not do it ---
`--chaos-udp-peer 3040@bridge` says that 3040 lives there, and a frame for
any other address is still dropped.

**An endpoint and no Chaosnet address**, which is what tells it from
`--chaos-udp-peer`: the CHUDP frame carries the real destination in its
trailer and the bridge routes on that. A port, an address or
address:port, a name resolved once at the start, and `42042` if no port is
given.

**A broadcast does not go here.** It goes to the named peers alone, who are
stations on this machine's own cable; handing a broadcast to a bridge would
invite it into a wider network than it was meant for. Decided, and not an
oversight. muir stays a leaf either way: a frame goes out only when a
station of this process put it on the cable, so what arrives from one peer
is never sent to another.

It needs the cable, `--chaos-udp`. Default: off, and a frame no peer entry
names is dropped.

### `--chaos-udp-peer <address>@<host>:<port>`

A Chaosnet host reached over UDP and where it lives: `3060@127.0.0.1:42042`,
the address in octal or subnet:host and the host a name or an address,
resolved once at the start. The port may be left off for `42042`. This is
how a run names its band's file and time host.

Once per peer, and the address may not be this machine's own. It needs the
cable, `--chaos-udp`.

### `--checkpoint <file>`

Write the machine's whole state to the file when the run stops, for
`--resume` to start from: the engine, the processor's memories and
registers, main memory, the display, the I/O board, and the drive with every
block written to its pack.

On `chip` it is the boards themselves --- every net, every cell and every
timer of the processor, the bus interface, the memory, the I/O board and the
display --- taken at the first microcycle from the stop with no bus cycle in
flight, with the drives on a netlist controller's cable and the multiplexor
between them where each stood.

The prompt's `checkpoint`, and Home in the terminal, write one as the run
goes, as `muir-yyyymmdd-hhmmss.chk` in the current directory, and the run
goes on.

On QUUX none is written while the [file device](quux.md#the-file-device)
has a handle open or a command queued: its host files are outside the
machine. muir says so, and why, and the run goes on.

### `--color-terminal [<endpoint>]`

`cadr` only.

Where the color TV's screen is served, as `--terminal` is the main screen's:
a port, an address or address:port.

**Pixels only.** The machine has one keyboard and one mouse, both on the I/O
board, and they stay with the terminal that serves the main screen, so what
a viewer types or points at this one is dropped. `--tv-capture` records the
main screen and `--color-tv-capture` this one.

It needs `--color-tv`. Default: the display above the last one bound, which
in a run with one machine is the main screen's.

### `--color-tv [netlist|model]`

`cadr` only.

Fit **the color TV**, the second display board: a LISPM TV strapped to the
other addresses MIT's own `cadrtv/lmtv.order` gives --- the frame buffer at
`17200000` and the registers at `17377750`, "for the color TV, x is 5". The
main screen stays at `17000000` whichever board `--tv-board` named, System
100 hardwiring `MAIN-SCREEN` to one bit a pixel there.

**Off by default, and that is how the software finds out.** A machine
without the board answers those addresses with an NXM, which is what
`COLOR-EXISTS-P` reads when it writes into the color buffer with the error
stop off. With the board there the cold boot sets it up: `COLOR:SETUP` loads
the NTSC sync program, starts it in clock mode 3 with vertical spacing 36,
and writes the sixteen colors of the map.

The picture is 576 by 454 at four bits a pixel through that map, served on
`--color-terminal`. `netlist` puts the board itself on `chip`'s backplane
--- a second LISPM TV, wrapped to the color addresses --- and is `chip`'s
alone; `model` is the behavioral board and every engine takes it. Without a
word it is the netlist on `chip` and the model elsewhere. Either way the
model is fitted behind the buses and every write to the board is mirrored
into it, so the color picture is read off the same place whichever board
drew it.

Default: off, and the machine has one screen; with the flag and no word, the
netlist on `chip` and the model elsewhere.

### `--color-tv-capture <gif>`

`cadr` only.

Record the color TV's screen to the file as the run goes, as `--tv-capture`
records the main screen and to a file of its own: 576 by 454, each frame the
rectangle that changed since the last, timed by the machine's own clock.
There is no default path; one must be given.

**The picture is the buffer through the map**, so a frame's pixels are the
four-bit colors and the GIF's own color table is the sixteen the map holds.
The machine may rewrite the map as it runs, and a frame taken through a
changed map carries a table of its own and is the whole picture: a GIF
resolves each frame's pixels as it lays them down, so a new map recolors
everything already on the canvas.

It needs `--color-tv`. In the lashup in one process it is this machine's
color screen, the debuggee having no second board. Not over the debug cable,
where the two machines are two clocks. The clocks below it are
`--tv-capture-no-time`'s, one flag for both recordings.

### `-c, --config <file>`

The file of flags to read before the command line, which must be there.
Without it `cadr` reads `.cadrrc` and `quux` `.quuxrc`, in the directory it
was run from, or failing that in the home directory. [Flags in a
file](#flags-in-a-file) has the rest.

### `--continue`

With [`--resume`](#--resume-file), and refused without it: the prompt's
[`continue`](#continue-c) at the start, before the first microcycle. A
checkpoint taken with the machine halted, its `RUN` clear --- as a board's
checkpoint is taken --- runs on at once from where it stood, with no
reset and no prompt needed. On a checkpoint whose machine was running it
does nothing, and says so. On all three engines and both machines.

### `--debug-cable-connect [<endpoint>|0x<address>]`

`cadr` only.

The CADR's: refused on QUUX, which has no Unibus and so no debug cable.

**rtl:** this machine is the debugger: its DBGOUT connects to a debuggee
listening at the endpoint, a port, an address or address:port. Either end
may be another program that speaks the cable's frames. This machine has [the
prompt](#the-prompt), as one alone has, less `checkpoint` and the capture
commands.

An argument beginning `0x` is no endpoint but the physical address of the
window of registers a CADR in FPGA fabric presents its DBGIN at, which muir
reaches through `/dev/mem` with ordinary loads and stores. The debugger is
then muir running on the board's own processor under Linux; there is no
remote form of it. The two machines run free of each other rather than in
step, since fabric runs on its own crystal and cannot be asked to wait, so
no timing measured across that cable means anything. Where the window sits
is a property of the bitstream, so there is no default address, and muir
refuses a window that does not identify itself as the cable's rather than
store anything into it. `--debug-cable-listen` takes no window: what is
built in fabric is the debuggee's end.

Default: `127.0.0.1:7661`.

### `--debug-cable-listen [<endpoint>]`

`cadr` only.

The CADR's: refused on QUUX, which has no Unibus and so no debug cable.

**rtl, chip:** where this machine's DBGIN listens for a debugger's cable
over TCP: a port, an address or address:port. **Every `rtl` and `chip` run
listens**, asked for or not, since the bus interface's DBGIN is on every
machine; this moves the connector, or puts it back after
`--no-debug-cable-listen`, and a port named here is bound as it stands.

The machine runs on its own until a debugger connects, in step with it while
one is on the cable, and on its own again when the debugger is done or goes
away, listening again. It has [the prompt](#the-prompt) throughout, less
`checkpoint` and the capture commands while a debugger is on. On `chip` the
cable meets the board's own connector, run an event at a time.

A debugger reads and writes the whole Unibus and stops the clock, so the
connector stays on the loopback unless an address is named. Default:
`127.0.0.1:7661`, or the port above it when that one is taken by another
muir.

### `--debug-in-process`

`cadr` only.

The CADR's: refused on QUUX, which has no Unibus and so no debug cable.

**rtl:** the two-machine lashup in one process. A second machine runs beside
this one with both debug cables between them, each machine's DBGOUT to the
other's DBGIN, so that CC here debugs the other --- or the other this one
--- as MIT ran two CADRs.

The other boots the same PROM with no pack unless one is named, and its
console is CC's alone. The window and the stops are this machine's; both
machines get a terminal, the other's at the display above this one's.
Neither machine has [the prompt](#the-prompt).

### `--debuggee-chaos-address <address>`

`cadr` only.

**rtl:** the other machine's Chaosnet address, as `--chaos-address` is this
machine's. The other machine has a cable of its own, since muir's cable
carries one machine: the two cannot hear each other over it, and the only
wire between them is the debug cable. It has no link of its own either, so
the address is all it has.

Default: the same address as this machine's, which collides with nothing,
the two cables never meeting.

### `--debuggee-disk-pack <image>[,<unit>][,ro][,wp]`

`cadr` only.

**rtl:** the other machine's pack, as `--disk-pack`.

### `--debuggee-terminal [<endpoint>]`

`cadr` only.

**rtl:** the other machine's terminal --- its display, keyboard and mouse
over RFB, as `--terminal` is this machine's. In the lashup both machines are
served a terminal, the other machine's at the display above this one's; this
puts it elsewhere.

Default: the display above this machine's, `127.0.0.1:5901` when it is at
:0.

### `--disk-controller netlist|model`

`cadr` only.

**chip:** the disk controller, as MIT's board or as a model of it. QUUX's
disk is block-disk, always, and `quux` has no such flag
([`--disk-pack`](#--disk-pack-imageunitrowp)).

**The netlist runs the drive's real milliseconds.** Its sequencer waits on
the drive's clocks and on its own delay lines, so the block it is reading
takes as long as a block takes, and the microcode waits in `DISK-WAIT`
through every gate on the board while it does. **A run that touches no pack
pays about 3% for that** --- 0.97 s against 0.94 s over 2,000 microcycles
--- and **a run that boots a pack pays days**: System 100 came up this way
on 12 September 2026 after 2 days 14 hours and 301 million microcycles, 51
hours of it the cold boot's copy of all 21,342 pages of the band.

**`model` is how a `chip` run that does not care about the disk is made
quick**, a transfer completing inside the store to `START` and a seek taking
no time, as on `micro` and `rtl`. It is also what `--main-memory model`
leaves: the netlist controller is a second master on the Xbus and the model
memory answers the interface's own cycles alone, so that memory takes this
board down with it rather than refusing the run. Asking for both the model
memory and `--disk-controller netlist` *is* refused --- that is a backplane
that cannot be built. Either way the start says which controller the run
has.

Default: `netlist`, as every other board on the backplane is. `chip` means
the machine at gate level throughout; a board quietly running its model was
the thing that did not match the name.

### `--disk-multiplexor`

`cadr` only.

**chip:** a DISK MULTIPLEXOR on the netlist controller's cable, which is
what gives it eight drive ports instead of one. It hangs off that board, so
it wants the netlist controller --- which a `chip` run has unless it asked
for `--disk-controller model`, and asking for both is refused.

**Not because the unit number is forced to 0.** `UNIT<2:0>` reach one
74LS244's inputs on the controller and nothing else --- `cadrdc/dc.wlr`
gives a direction per pin and none of the three has a `TO` --- so the board
has no driver for them at all, and the six one-board jumpers ground them to
stop three inputs floating. Unit 0 is the consequence. The multiplexor is
what supplies the driver, so a second `--disk-pack`, or one past unit 0, is
refused without it: the board it needs is a board somebody chose to have,
and muir names the flag rather than fitting one nobody asked for.

The model controller is refused nothing, because it wants no board: it is
behavioral and has addressed eight units all along. And this flag takes no
`netlist`-or-`model`, as the other board flags do --- MIT drew one
multiplexor and there is nothing to model it against.

Default: off, with one pack in unit 0 and the jumpers on. The start says
when it is fitted.

### `--disk-pack <image>[,<unit>][,ro][,wp]`

The pack in a drive: its blocks end to end.

After the image, in any order: the unit, `ro`, and on `cadr` `wp`.

Without `ro` the image is opened read-write and a written block goes into
the file, as a drive writes its pack; `rw` says so. With `ro` the file is
opened read-only and the drive is a writable one to the machine all the
same: a written block stays in the run and reads back, so it reaches a
checkpoint but never the image. `ro` means the same on `cadr` and `quux`.

`wp` is the Trident's read-only switch, `cadr`'s alone: the status word
says so, `STATUS<7>`, and a write faults, so nothing reaches the image
either, and it is opened read-only. MIT's boot PROM takes `STATUS<7>` for a
disk error and halts at `ERROR-DISK-ERROR`, so a pack in a drive with the
switch on does not boot. `wp` with `rw` is refused. Block-disk has no such
switch, and `quux` refuses `wp`.

The flag can come more than once, one pack to a unit, up to the eight the
controller addresses. On the netlist disk controller a second pack, or one
past unit 0, wants `--disk-multiplexor`.

On `quux` the pack is [block-disk](quux.md)'s one disk: the same registers
and command list as the CADR's controller with blocks by number, read and
write only, one pack at unit 0, and 100 us a block. It is a file of any
size, raw, a fixed VHD or a dynamic VHD, told apart by the VHD footer; the
start says which and how many blocks. [QUUX's disk](#quuxs-disk) says how
to make one. The CADR's pack is MIT's raw Trident image, exactly a T-300's size.

Default: unit 0; no pack unless one is named, which is a drive with no pack
in it and a boot that waits on it for ever.

### `--file-root [<name>=]<folder>[,ro]`

`quux` only.

**QUUX:** a host folder its [file device](quux.md#the-file-device) serves
under the pathname host `HOST`. A folder alone is HOST's `/`, holding
`sys/`, `site/` and `home/<user>/`; `<name>=<folder>` is the top-level
directory `<name>`, over the default folder's entry of that name. A value is
a named mount when the text before its first `=` is a name's component.
`,ro` refuses every write under it, answering ATF. The flag can come more
than once, each name once and one default folder; the folder must be one.
With no default folder `/` holds the mounts alone and is read-only. The
start lists them, with the device's time.

Default: none; `/` is empty.

### `--io-board netlist|model`

`cadr` only.

**chip:** the I/O board.

Default: `netlist`.

### `--keyboard-boot <keys>`

The keys the boot sequence needs. On a CADR, holding both Control keys and
both Meta keys and pressing Rubout cold-boots the machine, and with Return
instead warm-boots it: the keyboard's own firmware sees the keys down, sends
a boot word after the key-down, and the I/O board decodes it and pulls the
processor's boot line, as the button does. muir does the same from the
terminal's keyboard, on every engine.

A host keyboard rarely has two Controls and two Metas free to map, so which
of them the sequence needs is this setting. `ctrl` is MIT's Control key and
`meta` its Meta; one of a word is either key of its pair, two is both, and
the order does not matter. `ctrl,meta` is either Control and either Meta, as
Ctrl-Alt-Del is pressed; `ctrl,ctrl,meta` both Controls and either Meta;
`ctrl,meta,meta` either Control and both Metas; `ctrl,ctrl,meta,meta` both
of each, the CADR keyboard's own sequence. Rubout and Return are never in
it, and anything else is refused with the four named.

After the boot word the keyboard sends no key-up until the next key-down, as
the firmware does, so that the machine reads the boot word before anything
else; `--keyboard-mapping-trace` says so on each line it holds back. The
word stays in the I/O board's register for the microcode, which reads it as
it starts to choose cold from warm.

Default: `ctrl,meta`.

### `--keyboard-mapping <file>`

What a viewer's keysyms mean on the Lisp Machine keyboard: `key <keysym>
<key>` a line, and `prefix <keysym> <keysym> <key>` for a key reached by
pressing one and then another, there being more keys on this keyboard than a
host has spare.

It goes over muir's built-in mapping rather than replacing it, so a file
naming one key leaves the rest as they were, and the prompt's `keys` prints
what is in force.

The keyboard itself is MIT's and is not a choice --- its key positions are
MIT's own tables --- and the mapping is what this names.

Default: `.muirkeys` in the directory muir was run from, else `.muirkeys` in
the home directory, as `.cadrrc` and `.quuxrc` are looked for; `MUIR_KEYS` in the
environment names a file in place of the two. Without one the built-in
mapping stands.

### `--keyboard-mapping-dump`

Write the mapping this run would use to stdout, in the format
`--keyboard-mapping` reads, and stop --- before a terminal is bound or a
machine is built, so stdout carries the mapping and nothing else. Fed back
in unedited it changes nothing, so it is a copy to edit rather than a
report.

```text
cadr --keyboard-mapping-dump > my.keys
# edit my.keys
cadr --keyboard-mapping my.keys
```

### `--keyboard-mapping-trace`

Every keysym a viewer sends and what it became, on stderr, alongside the
run. The other half of the same job: the dump says what a keysym means here,
this says which keysym arrived, and a key that will not type needs both. A
viewer chooses which X11 keysym to send for a physical key, so muir is the
only authority on what it received --- `xev` reports what the host's X
server thinks, which is not the same thing and differs most on the
modifiers.

Each line names the keysym by name and number, whether it went down or up,
and the key it became --- spelled as `--keyboard-mapping-dump` spells it, so
the line can be pasted into a mapping file --- or `no binding` where the
mapping has nothing for it. A keysym held as a prefix says so rather than
printing nothing, a prefix's press producing no key by design.

**And what the terminal lost.** What a viewer types waits for the machine's
next look, 256 events of it, and beyond that the oldest keystroke goes whole
rather than the queue growing for the length of the run. A keystroke that
goes there never becomes a keysym line at all, so a character that failed to
type looks exactly like a key with no binding --- which is why the count is
said here as well:

```text
terminal: the input queue was full: 20 key events lost, 37 in this run
```

The count and not one line an event, a queue that fills under load losing
hundreds of them. **A run says the first loss with or without this flag**,
one line, naming the flag for the rest; under the flag it says each time the
count changes.

**And what the keyboard refused.** Below the terminal the keyboard holds a
queue of its own, 256 words the machine takes one at a time, and a keystroke
it has no room for is refused whole, leaving nothing down. A shifted
character is the most exposed, a tap needing three words where a letter
needs one, which is how `(` and `)` go missing on a machine slow to read its
keyboard. The key's own line says so --- `( refused: the queue is full, 256
words the machine has not read` --- and a run says the first refusal
without the flag.

The pointer's queue loses its oldest the same way and nothing is printed for
it. A viewer sends where the pointer is rather than how far it moved and the
mouse hands the machine the difference from the position it last saw, so the
newest event is the one that matters and that one is always kept.

Default: off --- and the first loss is said anyway.

### `--main-memory netlist|model`

`cadr` only. `quux`'s main memory is an amount,
[`--main-memory-size`](#--main-memory-size-nmw).

**chip:** main memory as MIT's board or as `rtl`'s model of it.

**`model` takes the disk controller down with it.** The netlist controller
masters the Xbus for its own transfers and the model memory answers the
interface's cycles alone, so a run that chose the model memory gets the
model controller too; a run that named `--disk-controller netlist` beside it
is refused instead, that pair being a backplane that cannot be built.

Default: `netlist`.

### `--main-memory-boards <n>`

`cadr` only: how many 64K-word boards, 1 to 60: main memory on every
engine, and on `chip` the boards on the backplane. `quux` refuses it,
naming [`--main-memory-size`](#--main-memory-size-nmw).

Default: 32, the two million words.

### `--main-memory-size <n>MW`

`quux` only: how much main memory, in whole megawords with the unit
written, `--main-memory-size 32MW`. QUUX has no memory boards, so its
memory is an amount, 1MW to 64MW.

Nothing else is taken: no other unit (`32KW`, `32MB`), no fraction
(`1.5MW`), no bare number, and never a bare M (`32M`), which could be read
as megabytes. The unit is case-sensitive, as every flag's word is, so
`32mw` is refused too. Each is refused saying how the amount is written:

```text
quux: --main-memory-size 32M: main memory is given in megawords, with the unit MW, such as 32MW
```

and an amount outside the range is refused naming the range.
The start says the amount, `memory: 32MW`, and so do a resume and the
prompt's `info`.

Default: 32MW.

### `--no-auto-boot`

Leave the boot button unpressed, as a CADR is when the power comes on: `RUN`
is clear, the machine is halted, and nothing runs. On any of the three
engines.

The run starts held at the prompt, so that the machine can be looked at as
it came up; `boot` there presses the button, and the machine runs from that.
`continue` sets `RUN` without the button, and the machine runs from the
state the power left it in, with no boot trap; `step` does neither and says
so.

Default: muir presses the button for you.

### `--no-debug-cable-listen`

`cadr` only.

**rtl, chip:** no connector for a debugger's cable, so that this machine
cannot be debugged from another. Of this and `--debug-cable-listen` the last
given wins.

`--checkpoint`, `--tv-capture`, `--color-tv-capture` and `chip`'s `--watch`
leave the connector empty by themselves, wanting a machine on its own, and
the start says so.

Default: the connector is there, at `127.0.0.1:7661`.

### `--no-pace`

Run as fast as the host will take it, rather than at the machine's own speed:
the one engine that does that by default is `micro`. Of this and `--pace`,
the last given wins.

Default: `rtl` and `chip` are paced, `micro` is not.

### `--pace`

Run at the machine's own speed rather than as fast as the host will take it:
the machine's own nanoseconds are the target, and a run that is ahead of
them waits until they catch up.

**It is what keeps the band's clock right.** A CADR has no clock chip: the
band asks its time host for the time once, when it boots, and counts the I/O
board's microsecond clock from there (`INITIALIZE-TIMEBASE` and
`UPDATE-TIMEBASE` in `sys/io1/time.lisp`). That clock counts the machine's own
nanoseconds, so a run that gets ahead of them keeps the band's time ahead of
the day by as much.

Unpaced, an engine runs as fast as it can. `micro` is about nine times a
CADR and `rtl` about twice, so much of a paced run of either is spent
waiting rather than computing --- which is the other half of what this is
for: the core the run was pinning is left idle for that share of it, and the
host can power it down between waits.

**One speed, the machine's own: there is no factor.** A wait is never longer
than half the terminal's own interval, so a keystroke waits no longer on the
pacing than it already waits on the poll; a run further ahead than that
waits again at the next check instead of in one long sleep.

**A wait the host rounds up is time the run does not take back.** A sleep is
asked for and the host decides when it is over, which is a little late
rather than a little early, and the pacing does not claw that back any more
than it claws back a stall. So a paced run keeps the machine's speed or
falls a little under it, and never goes over it.

**A run that falls behind does not sprint to catch up.** Behind --- a loaded
host, a heavy microcycle, an engine slower than the hardware --- the pacing
starts again from where the run is rather than making the loss up
afterwards: a run that stalled and then ran at nine times speed would be
worse than one that is simply late. Time held at the prompt is the same:
what went by while nothing ran is not a debt, and the run goes on from
`continue` at the machine's speed.

On `chip` it never waits: that engine is some thousands of times slower than
the machine, so the run is never ahead of its clock.
It is refused on an end of the debug cable --- `--debug-in-process`,
`--debug-cable-listen`, `--debug-cable-connect` --- where the two machines
already pace each other through the cable's own clock, and an end that slept
on top of that would only hold the other up. A debugger that connects to a
paced machine's own connector takes the pacing off while it is on the cable,
for the same reason, and the machine is paced again when it goes. Given
neither flag, a run on an end of the cable is not paced.

Default: on for `rtl` and `chip`, off for `micro`.

### `--prom <file>`

The boot PROM to run, an MCR microcode file as MIT's own
`sys/ubin/promh.mcr` is: the 1024 words the machine fetches before it
turns the PROM off. A program longer than that, or assembled somewhere other than
address 0, or setting the statistics bit `IR<46>`, which a burned word has
nowhere to hold, is refused rather than run.

The start says how the file stands to MIT's own, word for word or how many
words apart, and that matters: **recovered copies of the boot PROM are not
all the same program.** Two builds of "version 9" exist that differ in 214
of their 454 words, and nothing about a copy announces which it is.

Default: MIT's own, built in --- System 100's `sys/ubin/promh.mcr`, version
9.

On `quux` the file is QUUX's own kind: an MCR file in **partition
order**, MIT's with the two 16-bit halves of every 32-bit word swapped, as
muir-sys's builder writes QUUX's PROM and microcode, with the program
assembled at `36000`, where QUUX's PROM sits. A file in MIT's order is
refused saying so, and so is one assembled at 0. Default: QUUX's own,
built in, `data/quux-promh.mcr`, PROM 2001 ([QUUX](quux.md#its-boot-prom-in-its-own-addresses)).

### `--resume <file>`

Start from a checkpoint instead of cold: the engine that wrote it, the same
pack under it, the Chaosnet plugged in afresh, and as much main memory as it
had, which `--main-memory-boards` on `cadr`, and `--main-memory-size` on
`quux`, may not gainsay. On `chip` the boards on the backplane have to be the
checkpoint's too.

The boot button is not pressed: what it would set is what the checkpoint
replaces. The stops count from here.

A checkpoint carries `RUN` as it stood. One taken with the machine halted
--- a board's checkpoint is --- resumes halted, and the prompt's
[`continue`](#continue-c), or [`--continue`](#--continue) at the start,
sets `RUN` and runs it on from where it stood; `boot` starts it afresh from
the PROM.

### `--rtc <unix-seconds>|host`

`quux` only.

**QUUX:** its [real-time clock](quux.md#the-real-time-clock), register page
word 103. `host` reads the host's clock at each read. A second, 0 to
4294967295, starts the clock there at power-on and counts the machine's own
time from it, holding at 4294967295, so that a run repeats; a larger one is
refused. The start says which. A checkpoint carries it, and a resume under
another `--rtc` is refused.

Default: `host`.

### `--serial <endpoint>`

`cadr` only.

Where the serial port at J9 --- the Signetics 2651 at IOBSER 0A12 --- is
reached: a TCP port, or address:port. Attach with `nc <host> <port>` or with
`telnet`. So a machine started with `--serial 5952` is reached by `nc
127.0.0.1 5952`, and muir names the endpoint in its startup lines and
reports each connection and hangup as they happen. A connection is the
device on the null-modem cable plugging in, which asserts DSR, DCD and CTS
--- the sheet has the chip "conditioned to transmit data when the -CTS input
is low" and "to receive data when the -DCD input is low" --- and hanging up
drops them. One device at a time, and a second connection is closed as it
arrives.

The rate and the frame are whatever the machine has programmed into the
chip, and nothing at this end sets or checks them; MIT's own driver defaults
to 300 baud, seven data bits and even parity. This is a TCP endpoint rather
than a pseudo-terminal for that reason: a pty would look like a serial
device, so a rate would be set on it that the port never sees.

It is one machine's port, so it is refused with the lashup flags.

Default: off, and J9 empty. Having one costs something: on `chip`, a port
the machine has opened counts the baud-rate crystal and the I/O board stops
idling.

### `--stop-after <microcycles>`

How many to run, then stop.

Default: none; the run goes on until a `--stop-at`, a halt or ^C.

### `--stop-at <pc>`

Stop when the PC reaches this address with the boot PROM disabled: in
microcode loaded into the control store. Octal, as MIT writes it. On
QUUX, whose PROM is never disabled, that is a PC outside the PROM's
`36000`-`37777`, and an address inside them is refused.

### `--stop-at-prom <pc>`

The same with the PROM enabled: an address in the boot PROM. On the CADR
it is below `1000`: the PROM and the control store share their low
addresses, so a PC alone names two places. On QUUX the PROM has addresses
of its own, and `<pc>` is the control-store address the PC holds there,
as on the CADR: `36043`, not `43`. An address outside `36000`-`37777` is
refused.

Neither stops where the PC holds a control-store write's address: the
microcycle after `WRITE-I-MEM` writes the control store there and runs
nothing from it, and the program goes on where it was. `rtl` passes
through those addresses while a PROM clears the control store.

With `--stop-after`, whichever comes first.

### `--terminal [<endpoint>]`

Where the display, keyboard and mouse are served over RFB, RFC 6143, for any
VNC viewer to connect to: a port, an address or address:port. Every run
serves a terminal, asked for or not --- the machine has no other way to be
worked --- and this says where instead. A port named here is bound as it
stands and the run stops if it cannot be; an unnamed one is where the first
free display is looked for. An address other than the loopback lets another
machine in --- RFB's None security is the only type offered, so a viewer
needs no password. [The terminal](#the-terminal) has the rest.

Default: `127.0.0.1:5900`, VNC's display :0, or the first free display above
it.

### `--timing-model cadr|fpga`

`cadr` only.

**rtl:** whose time the processor and its boards keep. `cadr` is the board's
own nanoseconds. `fpga` is the 10 ns grid muir-fpga's fabric runs on, so that
references taken from `rtl` come out as that fabric runs: a delay something
starts ends at the first tick at or past it, counted from its start, and a
clock that runs freely from power-on keeps its exact phase, each edge taken at
the first tick at or after it. A microcycle at normal speed is 150 ns there
rather than 145. [What each engine models](engines.md#what-each-engine-models)
says which delays and clocks those are.

QUUX drops the delay lines, and its one timing is `sync`: the same grid,
with every microcycle [`--sync-cycle-ticks`](#--sync-cycle-ticks-k) ticks
long in place of the CADR's delay-line taps. Registers are still clocked at
the one edge, and the bus keeps its own time, so only the length of a
microcycle changes. A `quux` run has it without asking, and `sync` is no
word of this flag. [QUUX](quux.md) has it.

`fpga` is refused on `micro` and `chip`, which keep the board's time. A
checkpoint carries the timing, and a resume under another is refused.

Default: `cadr`.

### `--cache <words>`

`quux` only.

**rtl, QUUX:** the size of QUUX's memory cache, `<words>`, a power of two:
[QUUX's cache](quux.md), always fitted, unified and write-through, by
physical address, main memory and the frame buffer and no device
register, in lines of 8 words, 2-way, a hit in
20 ns, with a one-word write buffer; at least 16 words, a set of two
lines. It changes when the machine's reads and writes are answered and
never what they read. The start says it and a checkpoint carries it.
Refused on `micro`.

Default: 4096.

### `--tlb <entries>`

`quux` only, revision 14.

**micro, rtl, QUUX revision 14:** the size of its TLB, `<entries>`, a power
of two from 1024 to 32768: [revision 14](quux.md), direct-mapped, its
contents modelled on both engines; on `rtl` a sweep takes a tick of 10 ns
an entry. The start says it. Refused below revision 14.

Default: 4096.

### `--memory-timing <read>,<write>`

`quux` only.

**rtl, QUUX:** main memory's timing on [QUUX's memory port](quux.md): a line
fill and a write, in ns, or `arty` or `de25` for the two boards' own
figures. The start says it and a checkpoint carries it. Refused on
`micro`.

Default: 380,290, the slower board's.

### `--sync-cycle-ticks <k>`

`quux` only.

**rtl and micro:** a microcycle's length in 10 ns ticks, 1 to 255, QUUX's
`sync` timing. It is a board's: the number its fit proves its longest path
settles in.

Default: 4, the DE25-Nano's. The Arty Z7-20 runs QUUX at 5.

### `--tv netlist|model`

`cadr` only.

**chip:** the display.

Default: `netlist`.

### `--tv-board simple-tv|lispm-tv`

`cadr` only.

Which display board, **on every engine**: the SIMPLE TV that System 100
drives, or the LISPM TV that replaced it in December 1980. One model serves
either --- MIT's own `cadrtv/lmtv.order` is the LISPM TV's programming
specification and both boards answer it --- so this chooses the netlist
`chip` builds the backplane with and the board the model answers as on
`micro`, on `rtl` and under `--tv model`. The start says which the run has,
and a checkpoint carries it.

The two program alike but for mode bit 7, which reads the sync enable back
on the LISPM TV and zero on the SIMPLE TV, where an ECO grounds it.

QUUX's display is [the video controller](quux.md#the-video-controller),
always, and `quux` has no such flag; `video` is refused as a word of it.

Default: `simple-tv`.

### `--video-size <width>x<height>`

`quux` only.

The size of the video controller, QUUX's display: 1280 by 1024 unless this
says otherwise, one bit a pixel, with no sync program and no interrupt. The
width a multiple of 32, and at most 1920 by 1080, the largest QUUX
supports.
The start says it, the feature page gives it to the software, and a
checkpoint carries it; a resume at another size is refused.
`--mono-tv-size` is refused by both executables, the refusal naming
`--video-size`.

Default: `1280x1024`, the size of muir-fpga's Arty Z7-20 and DE25-Nano;
its Kria KR260 is `1920x1080`.

### `--tv-capture <gif>`

Record the display to the file as the run goes, an animated GIF: each frame
the rectangle that changed since the last, over the one before it, two
colors and LZW, timed by the machine's own clock so it plays at the
machine's speed. It stays small while the screen stays still. There is no
default path; one must be given.

On `cadr`, in the lashup it is both machines on one canvas, the debugger's screen at
the left and the debuggee's at the right with a rule between them, so that a
frame is one instant on both: the two machines are one clock there, which
two files could not keep. Not over the debug cable, where they are two.

The main screen: the color TV's is `cadr`'s `--color-tv-capture`.

### `--tv-capture-no-time`

Leave the clocks off the recordings, `cadr`'s color screen's as much as the
main screen's: there is one flag for the two. By default a line below the screen,
hiding no part of the display, shows the machine's simulated time at the
left and the wall clock, the local time of day, at the right, each hh:mm:ss;
this drops that line.

### `--watch <from>[-<to>]:<net>,<net>,...`

`cadr` only.

**chip:** record the named nets over microcycles `from` to `to`, or from
`from` to the end of the run when there is no `to`, counted as
`--stop-after` counts them --- from the start of this run, so a resumed
checkpoint counts from the checkpoint. Each net is named as the prompt's
[`net`](#net-boardnamewidth) names one, `disk:NEW CCW` or `PC/14` for a bus, and the
nets are comma separated. A name that does not resolve stops the run before
it starts, with the boards it was looked for on.

**The boards are sampled at every instant they move, not once a
microcycle.** A step of the netlist machine is to the next event on any
board --- the processor clock's next transition, or a delay-line tap, an
oscillator edge or a one-shot on some board, whichever comes first --- and
the gates have no delay, so nothing moves between two of them. Sampled after
each, the record has every level a net settled at, however brief: `CCW CLK`
on the disk controller is two taps of a delay line 50 ns apart, and a sample
once a microcycle, 145 to 220 ns, would step over it.

One line on stderr, prefixed `watch:` so that it can be grepped out from
under the run, with the time in nanoseconds, the microcycle and every
watched value --- a net as `net` prints it, a bus in octal, or `Z` while any
bit of it is undriven --- once as the range begins and then at every change,
and nothing while nothing changes. Outside the range the run pays nothing.

```text
$ cadr --chip --disk-controller netlist --disk-pack vendor/run/disk-sys-100-0.img,ro \
    --watch '530000-:disk:NEW CCW,disk:CCW CLK,disk:-CHAN.MASTER,disk:-LAST CCW,disk:XBAO/22' \
    --stop-after 700000 2>&1 | grep '^watch:'
watch: 116600440 ns, microcycle 530000: disk:NEW CCW=High disk:CCW CLK=Low disk:-CHAN.MASTER=High disk:-LAST CCW=Low disk:XBAO/22=0
watch: 118613110 ns, microcycle 539144: disk:NEW CCW=High disk:CCW CLK=Low disk:-CHAN.MASTER=High disk:-LAST CCW=Low disk:XBAO/22=777
```

The first line is what the range began at; the second is the boot PROM's
command list pointer, `777`, arriving in the controller's address register
as it sets up its first read. A range with a beginning and no end is the one
to give a long run: nothing is printed until the range, and nothing after
the last change.

The prompt's [`watch`](#watch-n-netnet) records the next *n* microcycles the
same way, so a machine that has been running for hours can be told to
without a restart. Not on a debuggee at the end of a debug cable, whose
steps are the debugger's.

### `-V, --version`

What this build calls itself, on stdout, the same from `cadr` and `quux`,
both being one muir: the name, the version, the commit
it was built from --- with `-dirty` after it where the tree had uncommitted
work, since the commit alone would name something that was never built ---
and whether it was built with optimizations off: `muir
0.1.0-8a69eea-release`. The commit is stamped in at build time, so a built
muir never runs git; built where there is no repository, a source archive or
a machine without git, there is no commit to name and the version is the
crate's and the build's alone. Every run says the same line first, so a
report of a run says which muir made it.

### `-h, --help`

The usage, then every flag with its default: the executable's own flags,
and no other's.

## Flags in a file

Flags that every run should have go in a file, one to a line: the flag, then
after a space whatever it takes, which is the rest of the line --- so a path
with a space in it needs no quotes. A line that is blank or begins with `#`
is a comment.

```text
# what every run of mine wants
--rtl
--terminal
--disk-pack /Users/me/lispm packs/system-100.img
```

**Each executable has its own file**: `.cadrrc` for `cadr` and `.quuxrc`
for `quux`, and neither reads the other's; `.muirrc` is read by neither.
The file is the one `--config` names, or the executable's own in the
directory it was run from, or its own in the home directory: **the first of
the three there, not all of them**, so a file beside the work is the whole
of a run's flags rather than an addition to the home one. A file named with
`--config` must be there; the two that are looked for need not be, and most
runs have neither. `MUIR_RC` in the environment names a file in place of
the two, for either executable, which is how muir's own tests keep a
developer's files out of their runs.

A flag of the other machine's in the file is refused as it is on the
command line, with the file named: `cadr: /Users/me/.cadrrc: --cache is
quux's, not cadr's`.

It is read before the command line, so **the command line wins**: a flag
given there replaces the one in the file, and an engine named there replaces
the file's --- `--micro`, `--rtl` and `--chip` are exclusive of one another
rather than last-wins, so the file's is dropped rather than refused. A file
cannot name another with `--config`; which file to read is the command
line's to say.

What came from the file is the first thing a run says, so that the lines
under it are never a mystery.

```text
flags: --rtl --terminal, from /Users/me/.cadrrc
engine: rtl
```

## The prompt

While a machine runs, on any of the three engines, a line on stdin is a
command to *muir* rather than to the machine --- the machine's own keyboard
is [the terminal's](#the-terminal). It is read from a pipe, or from a
terminal muir is in the foreground of, and acted on between two microcycles.
Every machine muir holds alone has it, `chip` included, with or without a
debugger on its connector, and so do both ends of the debug cable over TCP
and the debugger of a CADR in fabric --- less `checkpoint` and the capture
commands, which say why they are refused there. The lashup in one process
has none, and the start says so.

`muir: ` is written while the machine is held, which is when muir is waiting
to be told what to do next; it is not written down a pipe, where the answers
alone are wanted. A line typed while the machine is running is acted on all
the same --- there is just no prompt in front of it, because muir is not
waiting.

^C holds the machine at the prompt, and says where it stopped. A second ^C
while held, or a ^C with no prompt to go on from, ends the run as `quit`
does, with the recording and the checkpoint written. A hold nothing can run
on --- stdin has ended, so no `continue` will ever come --- ends the run the
same way rather than standing there.

A machine that stops *itself* is held here too, and says why. `(si:%halt)`
in the band runs `HALT-CONS`, which under `ERRSTOP` drops `MACHRUN` with
`RUN` still set; the statistics counter under `STATHENB` does the same. From
there no microcycle runs, and nothing about stepping says so --- the screen
simply stops --- so muir reads it off `FLAG-1`, where a console reads it,
and holds rather than spinning. `continue` and `step` say so as well; `boot`
presses the button that starts it again. `chip` holds the same way, off the
nets those registers are buffered from, it being a netlist and not an engine
with registers to read.

### `boot`

The boot button, which is what starts a CADR and all that starts one: it
presets `RUN`, clears the console's registers so the PROM is back over the
bottom of the control store, and the machine runs from address 0. muir
presses it at the start of a run unless
[`--no-auto-boot`](#--no-auto-boot) says not to.

### `hold`

No microcycle runs until `continue` or `step`. What ^C does.

### `continue, c`

Run on, from a hold. On a halted machine, its `RUN` clear, it sets `RUN` as
a console does --- CC's `CC-START-MACH` ends `(SPY-WRITE SPY-CLK 1)`, "TAKE
OFF", in System 100's `sys/cc/lcadrd.lisp` --- and the machine runs on from
its PC, its pipeline and its memories as they stand: no reset, the PROM not
put back, the console's other registers untouched. `SRUN` follows `RUN` at
the next master clock edge, which runs no microcycle. That is how a
checkpoint taken halted, as a board takes one, is run on; `boot` is the
button and starts the machine afresh. On a machine that is running and not
held it does nothing, and says so.

On `rtl` and `micro` it is the clock control register's write, so `STEP`,
`NOP11`, `IDEBUG` and `LDSTAT` go down with it; on `chip` it sets the 74S74
at OLORD1 1A14 that holds `RUN` and leaves the 74S175 at 1A09 that holds
the other four as it stands.

### `step [n]`

Run n microcycles, one without n, then hold and say where the machine is.

### `pc`

Where the machine is: the PC, whether the PROM is enabled, its microcycles
and its clock.

### `reg`

Every register in hex and as characters --- PC, OPC, Q, VMA, MD, LC, SPCPTR,
PDLPTR, PDLIDX, the dispatch constant and the interrupt control. The names
are the boards' own nets'.

### `amem, mmem, dmem, pdl, spc [from [n]]`

Dump one of the machine's memories: the A and M memories, the dispatch
memory, the PDL buffer, the SPC stack. All of it, or n words from an
address; both octal, as MIT writes them.

Four words to a line --- the address, the words in hex, and the four
characters each word holds, the first in the low byte, in the Lisp Machine's
own character set. A line the same as the one above it is a `*`.

### `mem <address> [n]`

Main memory, in the same shape: the word at an address, or n words from
there, both octal. The address is not optional and a count left out is one
word, main memory being two million of them where the largest of the
memories above is 2048.

**The address is physical** --- the board and the cell on it --- and is
never translated through the map, so a program's own address is not one. An
address above the 22 bits the Xbus carries is refused as no physical address
at all; one inside them with no board behind it is told how many words this
machine has.

The machine need not be held. On `chip` main memory is the memory boards'
own cells and a word is a bit off each of the 32 4116s of one bank of one
board: reading them drives nothing, advances no clock and runs no bus cycle,
so a run is not disturbed by the asking.

### `screenshot, ss [file]`

The screen as it stands, as a PNG, to the file or to
`muir-yyyymmdd-hhmmss.png` in the current directory.

### `startcapture, sc [file]`

Record the display from here on, as a GIF, to the file or to
`muir-yyyymmdd-hhmmss.gif`. It is written when `endcapture` closes it, and
by the stop otherwise. Not at an end of the debug cable, as `--tv-capture`
is not: over the cable the two machines are two clocks.

### `endcapture, ec`

Write the recording that is going and stop recording, with the machine left
running. A `startcapture` after it begins another.

### `checkpoint [file]`

The machine's whole state to the file, or to `muir-yyyymmdd-hhmmss.chk` in
the current directory, for `--resume` to start from. Not at an end of the
debug cable, as `--checkpoint` is not: the cable is in the bus interface's
state, and `--resume` takes a machine on its own.

### `info, i`

What this run is, as the start said it.

### `net [board:]name[/width]`

**chip:** what a net is doing at this instant, by the name the drawings give
it, spaces and all --- `net TRIDENT.READY/`, `net -XBUS RQ` --- or a bus of
that many bits from `name0` up, `net PC/14`. A name is unique only within a
board, so the boards are searched in the order the machine is built, `cpu`,
`busint`, `memory`, `tv`, `disk`, `io`, and the answer says which one
carried it and which others do; a board before a colon asks that one. The
machine need not be held.

### `watch <n> <net>,<net>,...`

**chip:** record the nets, each named as `net` names one and comma
separated, over the next *n* microcycles from here: what
[`--watch`](#--watch-from-tonetnet) does from the command line, in the same lines
on stderr, for a machine already running. A second `watch` replaces the
first.

### `quit, q`

End the run, as a stop does: the recording and the checkpoint are written.

### `help, h, ?`

The commands, with what each does.

So a machine can be looked at before it has done anything, and started by
hand:

```text
$ cadr --micro --no-auto-boot
engine: micro
memory: 32 boards, 2 MW
pack: vendor/run/disk-sys-100-0.img in unit 0
terminal: vnc://127.0.0.1:5900 --- RFB, no password
stop: none; a halt or ^C
start: held, and the boot button not pressed; boot at the prompt presses it
^C holds the machine at the prompt; help lists muir's commands
muir: reg
PC                 00000000  ....
OPC                00000000  ....
Q                  00000000  ....
... and the rest of the registers ...
muir: continue
the machine is halted, its RUN clear: boot presses the button that starts it
muir: boot
PC 0 in the PROM; 0 microcycles, 0 ns; 0 this run
```

## The terminal

The **terminal** is the far end of the machine: pixels out, keys and the
mouse in. Every run serves one over RFB --- the VNC protocol, RFC 6143 ---
so any VNC viewer is a CADR terminal, and the machine has no other way to be
worked; `--serial` opens the serial port at J9, and nothing on that port
works the machine. Only None security is offered, so a viewer needs no
password; keep it on the loopback unless you mean to let another machine in.

```text
cadr                       # then point a viewer at 127.0.0.1:5900
cadr --terminal 5901       # or wherever you say
```

The display is VNC's :0 unless `--terminal` says another, and the start says
where it is. A display already taken --- a second muir on the host, which is
what the lashup over TCP is --- moves it up to the first free one; a port
you named yourself is bound as it stands, and the run stops rather than
putting a viewer somewhere it was not told to look.

What it shows is the frame buffer, which is the screen on every engine. The
keyboard is the CADR's own, with its key positions taken from MIT's tables,
and the mouse is the machine's quadrature mouse; Home writes a checkpoint as
the run goes. Once the run has stopped, muir keeps serving the last screen
for as long as a viewer is looking at it, or until ^C.

With `--color-tv` the color screen is served as well, at the display above
the last one bound or wherever `--color-terminal` says. **Pixels only:** the
machine has one keyboard and one mouse, both on the I/O board, and they stay
with the terminal that serves the main screen, so what a viewer types or
points at the color one is dropped.

On an engine with no [Chaosnet](#the-chaosnet) the boot stops in the
debugger at the initialization that wants a host. Super-B there, then the
date and time it asks for and y, finish it.

Separately, and on every engine: the band's cold boot leaves the display's
vertical interrupt off, so **the mouse is not tracked until `(si:setup-cpt)`
is typed at the listener**. That one is the band's own, not a Chaosnet
matter --- `LISP-REINITIALIZE` in this release guards its `SETUP-CPT` block
with `(UNLESS (NOT CALLED-BY-USER) ...)`, which the cold boot's
`(LISP-REINITIALIZE NIL)` does not satisfy, where the trunk source has
`(UNLESS CALLED-BY-USER ...)` --- so it is wanted just as much on a boot
that reached the listener with no trouble at all.

## What a run can write

**Checkpoints.** `--checkpoint <file>` writes the machine's whole state when
the run stops and `--resume <file>` starts from it instead of booting: the
engine that wrote it, with the same pack under it. The prompt's `checkpoint`
and Home in the terminal write one as the run goes. It works on all three
engines; on `chip` what goes in the file is the boards themselves, net by
net and cell by cell. A pack given `ro` is only ever read, a block the
machine writes being kept in memory and going into the checkpoint instead,
so that image stays as fetched and a checkpoint over it can be resumed from
any number of times.

**Screenshots.** The prompt's `screenshot` writes the screen as a PNG: 768
by 963, one bit a pixel, encoded by muir itself rather than by a library.

**Recordings.** `--tv-capture <gif>` records the display for the whole run,
and the prompt's `startcapture` and `endcapture` begin and end one as it
goes. Each frame is the rectangle that changed since the last, over the one
before it, two colors and LZW, timed by the machine's own clock so it plays
at the machine's speed --- and it stays small while the screen stays still.
A line below the screen carries the machine's time and the wall clock unless
`--tv-capture-no-time` drops it. `--color-tv-capture` records the color
screen the same way, to a file of its own, the picture being 576 by 454
through the sixteen colors of the map.

Without a file, all three write `muir-yyyymmdd-hhmmss` with the right
extension in the current directory.

## QUUX's disk

QUUX's disk is made with standard tools and no muir-specific one: a raw
file, a GPT written by `sgdisk`, the partitions' contents written by `dd`,
and, if a VHD is wanted, `qemu-img convert`. What the disk holds is in
[QUUX](quux.md#the-disk-file): the type GUIDs, the names, the current bit.
A 1 GiB disk:

```
qemu-img create -f raw quux.img 1G          # or: truncate -s 1G quux.img
sgdisk \
  -n 1:0:+256K -t 1:9e318cf5-a95b-4b3b-b2ad-9ae306b0e2da -c 1:"MCR1 UCADR 2001" -A 1:set:48 \
  -n 2:0:+100M -t 2:a3b30470-c5d4-41c1-87a8-d26590424cb8 -c 2:"LOD1 System 2001" -A 2:set:48 \
  -n 3:0:+256M -t 3:4652bea5-06af-4bd9-b2bb-3541370151c8 -c 3:"PAGE" \
  -n 4:0:+600M -t 4:7afa9532-75de-409f-8dc8-fef9763511d5 -c 4:"FILE" \
  quux.img
sgdisk -i 2 quux.img                        # First sector: 4096
dd if=band.lod of=quux.img bs=512 seek=4096 conv=notrunc
qemu-img convert -f raw -O vpc -o subformat=dynamic,force_size=on quux.img quux.vhd
quux --disk-pack quux.vhd
```

- **A partition is whole blocks**: 1,024 bytes, two sectors, its first
  sector even and its last odd. sgdisk's default alignment starts each on
  a 2,048-sector boundary, and a size in K or M ends it on an odd sector.
  **An end given as 0 or as `-1M`, "to the end", ends on whatever sector
  the free space ends on**: on this disk that was sector 2,095,070, even,
  half a block short.
- **The current microcode and band carry bit 48**, `-A <n>:set:48`; sgdisk
  shows it as "Undefined bit #48". The comment after the four-character
  name is up to 31 characters.
- **There is no TEMP partition.** QUUX's disk has none; the boot PROM
  saves nothing to the disk ([QUUX](quux.md#the-disk-file)).
- **At most 8 GiB**, 2^23 blocks: Lisp's block numbers are fixnums.
- **`dd` writes a band file, or a microcode file written in partition
  order, as it is**, at the partition's first sector (`sgdisk -i`), with
  `conv=notrunc` so the rest of the disk stays.
- The raw file is itself a disk muir takes; `-o subformat=fixed` makes a
  fixed VHD, the raw bytes and a 512-byte footer, which muir-fpga needs no
  translation for.

The traps, each measured with qemu-img 10.2.1 and sgdisk 1.0.10:

- **`force_size=on`, always.** Without it qemu rounds the disk up to a
  cylinder, head and sector geometry: the 1 GiB disk above becomes
  1,073,995,776 bytes, and the backup GPT sgdisk wrote at the raw file's
  last sector is no longer at the disk's end, which `sgdisk -v` reports.
- **Never run sgdisk on a fixed VHD file.** It takes the file's last sector
  for the disk's and writes its backup header over the VHD footer; qemu-img
  then calls the file "invalid VPC image" and muir opens it as raw. Change
  the table on the raw file and convert again, or on a loop device of the
  data alone.
- **`losetup -P` needs root.** A raw file or a fixed VHD's data on a loop
  device (`losetup -P --sizelimit <size without the footer>`) would be
  partitioned and written through `/dev/loopNpM`; **unverified**, not run
  here for want of root. A dynamic VHD is not its disk's bytes in order and
  cannot go on a loop device at all. qemu-io writes into a dynamic VHD
  without root:
  `qemu-io -f vpc -c "write -s band.lod <byte offset> <length>" quux.vhd`.
- **qemu-img reports a fixed VHD as raw** unless given `-f vpc`: the footer
  is at the end, where it does not probe. muir tells the formats by the
  footer and says which it found.

**Only the GPT PROM may meet a GPT disk.** QUUX's built-in PROM,
`data/quux-promh.mcr`, PROM 2001, is a GPT PROM: it takes the first
microcode partition carrying bit 48 and writes no block of the disk, and on
a disk with no GPT it halts at `ERROR-NO-GPT`, 36653
([QUUX](quux.md#its-boot-prom-in-its-own-addresses)). A PROM given with
`--prom` that does not read the GPT, or that saves page 0 to block 1
before it loads anything, as MIT's does, writes over sectors 2 and 3, the
start of the primary GPT's entry array, and breaks the primary table
(muir-sys, measured). System 2001's release disk is a disk made this way,
as a dynamic VHD ([QUUX](quux.md#the-disk-file)).

## The Chaosnet

The machine's Chaosnet interface is real --- it is half the I/O board, and
on `chip` it is that board's netlist. What is on the other end of the cable
is **not in muir**. A CADR had no file or time server inside it: it called
its **associated machine**, the host the boot banner names --- "with
associated machine OZ" --- for its files and for the date. So muir carries a
machine and nothing else, and a band that wants a host wants one on the
network.

**[ozd](https://github.com/metebalci/ozd) is that host.** It is the OZ
daemon: one server for however many Lisp machines are on the cable, which is
the commoner case than one, and it answers the four contact names a band
asks for.

| Contact | What the host answers |
|---|---|
| STATUS | The host's name and its subnet meters. AIM-628 §5.1 requires every node to answer it, and it is how a machine decides another is up: `HOST-UP-P` asks for nothing else, and `(hostat)` prints what comes back. |
| TIME | The universal time in four bytes, least significant first --- seconds since midnight GMT, 1 January 1900. |
| UPTIME | Seconds since the host came up. |
| FILE | The file protocol the band loads `SYS:` over --- a control connection and data connections beside it, over a directory the host serves as its `/`. |

`FILE` is what makes the acceptance test possible: CC is not in the band, so
`sys/cc/*.qfasl` has to come over the network, the way it would have on a
real machine.

**How a run reaches it.** `--chaos-address` sets the sixteen address
switches on the I/O board and nothing else; `--chaos-udp` is the cable,
which puts those switches on a network as **Chaosnet over UDP**, at the
protocol's own port unless it says where. Without the cable muir sends
nothing, as a machine with none talks to nobody. Every host
`--chaos-udp-peer` names is then a station on the same modeled cable, taking
its turn on it like any other, and a packet from one goes on with that
host's own address in the hardware source. The numbers are the band's own:
System 100's `sys/site/hosts.text` puts `MIT-LISPM-1` at `3050` and `MIT-OZ`
at `3060`, so that band is run with `--chaos-address 3050 --chaos-udp
--chaos-udp-peer 3060@<where ozd is>`. muir stays a leaf: a frame goes out
only when this machine put it on the cable, so one host's is never carried
on to another, and a `cbridge` beside it is what routes.
`--chaos-udp-default-peer` is where that bridge is: a frame whose
destination no `--chaos-udp-peer` named goes there rather than nowhere,
which is how a run reaches the wider Chaosnet. A broadcast is not handed to
it, the named peers being stations on this machine's own cable.

muir's own tests do not want a daemon running beside them, so they put a
Chaosnet server of their own on the modeled cable, in process. It lives in
`tests/support/` and is no part of the simulator.

## Two machines

MIT debugged a CADR from another CADR, over a **debug cable**: 21 wires from
one machine's DBGOUT connector to the other's DBGIN, through which the
console program **CC** halts the other machine, reads its registers and
drives its bus. muir runs that lashup, and it is the project's acceptance
test.

Each machine has a Chaosnet of its own: muir's cable carries one machine, so
the other gets a cable of its own. The two cannot hear each other over it
--- the only wire between them is the debug cable --- and the other machine
has no CHUDP link either, one socket belonging to one cable, so it reaches
no file or time host and its own boot stops in the cold-load debugger. That
is where CC finds it, and it is what the acceptance test wants.

`--debug-in-process` puts both machines in one process with both cables
between them. Over TCP the two machines are in two processes, and there
**the debuggee needs no flag at all: every `rtl` and `chip` run listens for
a debugger**, since the bus interface's DBGIN is on every machine --- it
takes the Unibus as master when a debugger drives its cable, and nothing in
the machine enables it. The connector is at `127.0.0.1:7661`, or the port
above it when another muir has that one, and the start says where. The
machine runs on its own until a debugger connects, in step with it while one
is on the cable, and on its own again when the debugger is done or goes
away, listening again; a second debugger while one is on is refused.
`--debug-cable-connect` in another `cadr` is the debugger, meeting it at
`127.0.0.1:7661` unless told otherwise. The listener may be a `--chip`
machine, and then the netlist board's own DBGIN answers the debugger, an
event at a time, at the netlist's pace.

`--debug-cable-listen` moves the connector, and `--no-debug-cable-listen`
leaves it empty, which a machine that must not be reachable from outside
wants --- a debugger reads and writes the whole Unibus and stops the clock,
which is why the connector stays on the loopback unless an address is named.
`--checkpoint`, `--tv-capture`, `--color-tv-capture` and `--watch` want a
machine on its own and leave it empty too, saying so. **The cable's clock
starts at the connection**, in one process and over TCP alike, each end
translating instants by its own origin, so a debugger that arrives late is
not a machine that has been waiting for it.

Over TCP each machine has [the prompt](#the-prompt), and a hold at one end
is felt at the other: each end steps only as far as the other has promised
and waits otherwise, so a held debugger holds the debuggee once it has run
to the debugger's last promise, and a held debuggee holds the debugger at
its next request --- the other process sits in its wait, terminal and prompt
unattended, until `continue`. A debuggee that stops itself is not held for
it, since a halted debuggee is what CC reads through the cable.

There is a third transport, where the debuggee is not a program at all:
`--debug-cable-connect 0x<address>` is the CADR of the
[muir-fpga](https://github.com/metebalci/muir-fpga) project, on the
FPGA of a Zynq board with Linux on the Arm cores beside it,
which is where muir runs to be its debugger. The cable's 21 wires are a
window of memory-mapped registers muir reaches with ordinary loads and
stores, and it is that project's window and no other: muir refuses one that
does not identify itself rather than store into it. The first two transports
keep the two machines' simulated clocks in step by exchanging promises;
fabric runs on its own crystal in real time, so nothing is promised, only
the debugger is stepped, and the debugger's own 11.05 us timeout is what
ends a cycle nothing answers. The window's layout is proposed and nothing
has been built to it yet. The debugger has [the prompt](#the-prompt); a hold
leaves the window as it stands, so a request standing at it when the hold
comes on holds the debuggee's Unibus until the adapter's watchdog lifts it,
and the debugger costs that cycle when it runs on.

### The recording

The acceptance test as it ran is at [muir.metebalci.com/simulator/lashup.html](https://muir.metebalci.com/simulator/lashup.html), the recording alone at the size the machines drew it.

This is the recording itself, at the size the machines drew it, which is the
size to read the screens at. Two machines on one canvas: 768 by 963 each, with
a two-pixel rule between them, and the line below the screens carrying the
machine's own time at the left and the wall clock at the right. The frames are
timed by the machine's clock, so it plays at the machine's speed.

The debugger is at the left. It boots the band, logs in, and loads the console
program CC from the Chaosnet, file by file --- CC is not in the band, so it
has to come over the network, the way it would have on a real machine. It then
seizes the machine at the right through the debug cable and runs MIT's own
test of it.

The machine at the right is where the acceptance test is won or lost. It boots
a band of its own --- it has a Chaosnet of its own to take the time from, and
comes up to a Lisp listener the same way the left one does --- and then it
goes still the moment CC halts it, and never moves again. That freeze is the
signal: from then on nothing of that machine's own software is running, and
every register and bus cycle the diagnostic reads is being driven through the
cable by the other machine.
