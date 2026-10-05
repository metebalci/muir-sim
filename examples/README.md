# `examples/`

Development tools, not parts of the simulator. Nothing in `src/` or `tests/`
depends on any of them, and `cargo test` runs none of them.

Each file's own header is the usage; this is the map. They come in three
kinds.

## They write committed files

These are the generators behind the scripts in `tools/`. Run one after the
material it reads changes, and a test will tell you when you have not.

| | Writes | Run by |
|---|---|---|
| `dcmicro` | `data/newdsk-d03.prom`, `-d04`, `-d05` --- the disk controller's control store, assembled from MIT's source as `MICRO` and `newdsk.trans` did between them | `tools/newdsk-proms.sh` |
| `reconcile` | a netlist reconciled with MIT's wire list for the same board: a wire the drawings label twice becomes one net under MIT's name, and the drawing errors in its `MOVES` table are corrected | every `tools/*-netlist.sh` but `cadrm-netlist.sh` |
| `trident-tables` | `data/trident-connectors.txt` and `data/trident-bus.txt`, the disk cable seam read off the netlist and `mit/cadrdc/dc.wlr` | `tools/trident-tables.sh` |

## They show what a machine is doing

| | |
|---|---|
| `screen` | boots off the pack and writes the frame buffer as a PNG, at an interval and at the end. The quickest way to see whether a change broke the display |
| `band` | what is on a pack --- the partition table, a band's system communication area, or any run of words from a virtual address. Octal, the way MIT's sources write it |
| `cc` | the acceptance test by hand: two `rtl` machines on the debug cable, CC loaded into one from the Chaosnet server, debugging the other. `tests/cc_lashup.rs` is the same thing as a test |
| `coverage` | how much of microcode 323 the `chip`-against-`rtl` comparison has actually covered, counted as distinct control-store words rather than microcycles |

## They measure

`benchmark` runs the two programs in `src/benchmark.rs` on all three engines
and prints a rate for each of the six runs. `datapath` is every one of the
ALU's sixteen logic functions and four of its arithmetic ones, the output bus
shifted both ways, the Q register loaded, shifted and read back, and the byte
hardware's three functions --- thirty-three operations, each taking the last
one's answer. `control` is calls three deep and back, a call that
returns on the POPJ bit, a dispatch through dispatch memory the program wrote
itself, and branches taken and not taken. Every run is five seconds unless a
different number is given, so the engines are compared over the same clock
rather than over the same work.

Use it rather than timing a boot when comparing builds: every time round the
loop is the same as the last, so the window does not matter, and every run is
held to the count the program left in `VMA` --- the microcycles it ran divided
by what one time round the loop costs --- because a machine that has stalled
toggles fewer nets and looks *faster*.

Neither program touches the disk, and the rate it prints against the
machine's own 145 ns microcycle does not account for one. The controller
behind the bus here is the behavioral model, as it is on `micro` and `rtl`
and on `cadr --chip --disk-controller model`: a seek takes no time where the
hardware spent milliseconds running the microcode's polling loop, so a
program that seeks does better against a CADR than this rate implies.
`cadr --chip` comes up with MIT's controller on the backplane instead, and
there the drive takes its own time --- about 3% on a run that touches no
pack, and days on one that reads a band. See the note on `report` in
`benchmark.rs`.

`profile` is where a band's microcycles go. It boots the QUUX band that
`MUIR_BAND` names, or the CADR's muir-sys release on the CADR, with the test
harness's Chaosnet server, logs in, and types thirteen workloads at
the listener --- the empty form, compiling, two kinds of recursion, consing,
fixnum multiply and remainder, flonum arithmetic, `aset` and `aref`, sorting,
bignums, `intern`, printing, and compiling again --- each ending by writing a
marker file through the FILE service. For each it prints the microcycles and
macroinstructions; the time; where the microinstructions went by the
category and source file of the nearest microcode label, the hottest labels,
and the microcode's own meters; and on `rtl` the memory cycles. The
workloads' time together closes the output. `MUIR_UCODE` runs it on another
microcode. About a minute on `micro`.

A microcycle count is not the time. On `rtl` a wait or a hang for the memory
advances the clock and runs no microcycle, so the time line gives three
figures side by side: the microcycles and their own time, the time stalled on
memory, and the time in all, which is the two added. On `micro` the memory's
time is a fixed charge a memory cycle, not a stall, and the line says so. The
category and label shares are of the microinstructions executed, which leave
out inhibited microcycles and the memory's time; each percentage the harness
prints says what it is of.

## What needs fetching

`dcmicro`, `reconcile`, `trident-tables` and `benchmark` read only what is
committed. `screen`, `band`, `cc` and `coverage` need the System 100 release in `vendor/` --- `cc`
needs the system sources too, since the Chaosnet server serves them to the
machine as `SYS:`. `tools/fetch-system-100-for-cadr.sh` puts both in place.
`profile` needs a muir-sys band instead: on QUUX the directory
`MUIR_BAND` names, a band's disk and the tree it was built from, such as
a directory holding System 2001's release disk, decompressed, and its
`release-2001-sys.tar.gz`, both from `tools/fetch-system-for-quux.sh`; on
the CADR its release, from `tools/fetch-system-for-cadr.sh`.
