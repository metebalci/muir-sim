# QUUX

QUUX is the CADR evolved: the same processor, buses and boards, changed where
a change pays for itself. In muir it is the `quux` executable, and `cadr`
is the CADR as MIT built it; muir-fpga and muir-sys choose it with
`--machine quux`. Back to [the manual](manual.md).

This page says where QUUX differs from the CADR. Everything it does not
mention is the CADR's.

QUUX's software is numbered in the 2000s and the CADR's in the 1000s: QUUX
runs muir-sys's System 2000 on microcode 2000 and boots on PROM 2000.
muir-sys's earlier builds for QUUX were numbered 1000 (the PROM and the
microcode) and 1002 (the system); where this page records what one of them
did, it names it by that number and calls it QUUX's.

## What each difference reaches

A change to the hardware reaches further than the board: the boot PROM,
the microcode, the Lisp system in the band, and the tools that read a
machine's state can all depend on what changed. For each of QUUX's
differences, what it needed:

| Difference | Boot PROM | Microcode | Lisp system | Tools |
|---|---|---|---|---|
| Six-bit level-1 map entry | nothing: MIT's PROM boots it; PROM 2000 also clears QUUX's 64 blocks | 2000: the six-bit read, the two-deposit write, invalid block 77, the reverse first-level map moved to system communication area 640-737 and the swap-out CCWs to 440-457 | nothing: System 1001 runs unchanged | CC's remote debugger (`CADR-DEBUGGER`) still assumes the CADR's map |
| MACHINE-ID in functional source 16 | nothing | 2000 reads it at boot and halts at `MACHINE-NOT-QUUX-11` on anything but QUUX from revision 11 | `PROCESSOR-TYPE-CODE` is 4 | nothing |
| The feature page | nothing | nothing: field widths are fixed when the microcode is assembled | System 2000 reads it: the PDL buffer's length, word 3, at every boot (muir-sys `sys/sys2/proces.lisp:268-274`); the video controller's size and buffer address, words 11 to 13 (`sys/sys/ltop.lisp:118-125`); and whether the file device and the real-time clock are there, word 15 (`sys/io/fdev.lisp:155`, `sys/io1/time.lisp:463`) | do not read it yet |
| `MUL` and `DIV` in one instruction | nothing | 2000 uses them in `MPY`, `DIV` and `BIDIV`'s quotient; the 31-step loops still step; `MULTIPLY` and `DIVIDE` named in `cadsym` | nothing | nothing |
| The clocks: the microsecond clock in the processor, and the interval timers on the register page, timer 0 the tick | writes reset devices and timer 0's period, 16,667 µs (revision 10) | 2000 writes timer 0's period at `RESET-MACHINE`, turns the tick on at `BEG06` and clears it in `INTR-TICK` through word 110, and turns off timer 1 or 2 if one interrupts | nothing: it reads the microsecond clock, and no timer | nothing |
| Reset devices, word 104 of the register page | writes it before it reads the disk (revision 10) | 2000 writes it at `RESET-MACHINE`, at every start of the microcode, a `%DISK-RESTORE`'s too, in place of `PROG.UNIBUS.RESET` | nothing | nothing |
| Block-disk | muir-sys's PROM 2000 for block-disk, which no longer boots the CADR controller | 2000 for block-disk: the disk routines by block number, no cylinder, head or sector | System 2000: the partitions, the band and the disk routines by block number | block-disk is `quux`'s only disk; `--disk-controller`, the CADR's controller, is `cadr`'s |
| The memory cache (`--cache`) | nothing | nothing | nothing | `--cache`; the profile harness's `MUIR_CACHE` |
| No delay lines: `sync`, always | nothing | nothing | nothing | `--sync-cycle-ticks`; `--timing-model` is `cadr`'s |
| No hung microcycle; the old word in a RAM's write cycle | nothing | nothing: microcode 324 and QUUX's 1000 never do either, counted (below) | nothing | nothing |
| No speed bits | nothing | the mode register write at boot need not set them | nothing | nothing |
| The video controller, the display | nothing | 2000: the run light in the video controller's buffer, no TV vertical flag | System 2000 sizes the main screen from the feature page | the terminal, screenshots and captures show whichever screen is fitted |
| The real-time clock | nothing | nothing | System 2000 sets the time from it at boot, ahead of the network, when word 15 `<0>` of the feature page says it is there, and its wall clock reads it from then on (muir-sys `sys/io1/time.lisp:461-471`, `:493`, `:691-696`) | `--rtc` |
| The file device | nothing | 2000 waits at `RESET-MACHINE` for it to be quiet, word 161 `<1>`, after reset devices | System 2000's `SYS:` is on it, HOST's `/sys` and `/site` (`site/sys.translations`) | `--file-root` |
| The fused return: the MACRO-DISPATCH register and the MACRO DISPATCH MEMORY, destinations 5 to 7 (revision 12) | nothing | 2000 writes destinations 5 to 7 only in `RESET-MACHINE`'s fill of the MACRO DISPATCH MEMORY (`tests/unused_codes.rs`) | nothing | the profile harness's `MUIR_H8A` fills and enables it |

## What each change measured

Microcycles counted by the profile harness (`examples/profile.rs`) over its
workloads on System 1001's band, each change against the machine without
it. These are microcycles, not time: on `rtl` a microcycle count leaves out
the time stalled on memory, and on `micro` the fixed charge a memory cycle.
A share of a workload is of its microinstructions executed, as the harness
prints it:

| Change | Measured |
|---|---|
| Six-bit level-1 map (QUUX on its microcode 1000 against the CADR on 323 rebuilt, 324) | 11.3% fewer microcycles on `micro` and 11.1% on `rtl` over thirteen workloads; `intern` 31% fewer, `print-scroll` 27%, `compile` 21%, the idle listener 20 to 30% |
| 16K-word PDL buffer (against QUUX's 1K) | 3.8% fewer microcycles on `micro` and 4.3% on `rtl` over thirteen workloads; deep recursion 29% fewer, its PDL buffer's share falling from 26% of its microinstructions executed to 0.9% |
| `MUL` and `DIV` (microcode using them against the same without) | 20% fewer microcycles a macroinstruction on the multiply-and-divide workload (25.0 to 20.0), 7% on float, 1.5% on bignum; multiply and divide's share of the first workload's microinstructions executed falling from 25.6% to 5.4% |

The time these save on a machine with the synchronous microcycle and the
cache is in [the memory cache](#the-memory-cache).

## The map

**A frame is a page-sized slot of physical memory, and a page is a page of
virtual memory.** "Frame" (page frame) is what MIT's CADR code calls a
"physical page": the `PHYSICAL-PAGE-DATA` region has a word for each one
(muir-sys `sys/sys/qmisc.lisp:32`), and a map entry's "physical page (frame)
number" is its `vma-phys-page-addr-part` (`sys/ucadr/uc-page-fault.lisp:165`).
"Page" alone is virtual. MIT's symbols keep their names. The frame buffer is
the video controller's memory, not a frame in this sense.

**A level-1 entry is six bits, not five.** The level-1 map names, for each
8K-word region of virtual memory, a block of 32 level-2 entries. On the CADR
the entry is five bits, so there are 32 blocks, and the microcode keeps the
last one, 37 octal, as the invalid block: at most 31 regions are mapped at
once (`ADVANCE-SECOND-LEVEL-MAP-REUSE-POINTER` in System 100's
`sys/ucadr/uc-page-fault.lisp`). QUUX's six bits give 64 blocks, 2,048
level-2 entries, and 63 regions.

**The sixth bit travels on the two bits the CADR leaves spare.**

| | CADR | QUUX |
|---|---|---|
| Read back, `MAP(MD)` | entry in `<28:24>`, bit 29 always 0 --- VMEMDR 1A01 drives it from `HI12` through a 74S240 | entry in `<29:24>` |
| Written, a store to `WRITE-MAP` with `VMA<26>` | entry from `VMA<31:27>` (`mit/cadr/ir.bits`) | bits 4:0 from `VMA<31:27>`, bit 5 from `VMA<24>` |
| Level-2 entry chosen | `{entry, VMA<12:8>}`, 1,024 entries | the same, 2,048 entries |

Bits 31 and 30 of `MAP(MD)` are the write and read faults on both, and bit
24 of `VMA` reaches no map write on the CADR. A store that writes both levels
at once addresses level 2 with the level-1 bits zero on both machines
(`Machine::write_map` has why).

`tests/quux.rs` holds the six-bit entry written through `VMA<24>` and read
back in `MAP(MD)<29:24>`, and a translation through a block above 37, on
`micro` and `rtl`; the same store on the CADR keeps five bits and reads bit
29 as 0.

## How software tells the two apart

**QUUX answers who it is in functional source 16, its MACHINE-ID**, one
microinstruction,
no bus cycle:

| Bits | QUUX | CADR |
|---|---|---|
| 31:16 | signature `0x5155` | nothing drives the M bus: all ones |
| 15:4 | hardware revision: 12 --- 1 the six-bit map, 2 the 16K PDL buffer, 3 the multiply and divide, 4 the tick, 5 the clocks, 6 the register page and the PROM at 36000, 7 the memory port, 8 the device registers, 9 the real-time clock and the file device, 10 the interval timers and reset devices, 11 the register page at `17777400` with block-disk and the video controller on it, word 100 in its final order, 12 the fused return | |
| 3:0 | processor type: 4 | |

Source 16 is one MIT left unassigned: the 74S138 on page SOURCE that
decodes it has that output unconnected, and microcode 323 does not read
it; microcode 2000 does, at boot (above). `IR<30>` is in no source decode, so source 36 is the
same. Source 17 reads all ones on both: open on the CADR, and on QUUX
unassigned since revision 10 (below). A machine is QUUX only if bits
31:16 hold the signature; the revision says which QUUX, each one containing
the last up to revision 9. Revision 10 does not contain revision 9: Q1's
interval timer (destination 4, source 17) and the reset `PROG.UNIBUS.RESET`
gave on QUUX are gone, the interval timers and reset devices taking their
places (below). Revision 11 does not contain revision 10: the register page
is at `17777400` and not `17377000`, block-disk's registers and the video
controller's mode are on it, word 100's bits are in another order, and the
network decodes its five registers alone ([the register
page](#the-register-page)); nothing answers at revision 10's addresses.
Revision 12 contains revision 11: it adds functional destinations 5 to 7
and feature word 17 ([the fused return](#the-fused-return)), which below
it write only M and read 0, and it runs microcode 2000 as revision 11 does.

**Software for revision 10 on a revision-11 machine** stops, measured on
`micro` and `rtl` with PROM 2000, microcode 2000 and System 2000 as muir-sys
built them for revision 10. Their PROM, with revision 11's System 2000
disk, waits at `DISK-AWAIT-PACK` (36600) for ever, reading block-disk's
status at `17377774`, where nothing answers, with word 101 `<0>` set: still
there after 50 M microcycles. Their microcode, loaded from their disk by
revision 11's PROM, passes its own check, which asks for revision 10 or
more, and halts at `FILE-DEVICE-NOT-QUIET` (26525 in its `ucadr.sym`; the
PC reads 26526), word 101 `<0>` set, 2.07 s after power-on (30,053,391
microcycles) on `micro` and 2.06 s (42,045,940) on `rtl`. On a
revision-11 machine that halt means the microcode is not revision 11's: the
file device is on the page and answers, but not at the address that
microcode reads.

`tests/quux.rs` holds the word on both engines and the CADR's all ones;
`the_unassigned_sources_read_all_ones_on_the_board` in `tests/output_bus.rs`
holds the CADR's on the netlist.

**Unverified:** that a real CADR reads all ones there. The M bus has only
tri-state drivers and no pull-ups, so for an unassigned source it floats, and
TTL reading an open input as high is what the netlist model does and what
the parts usually do, not what a datasheet promises. The 16-bit signature is
what makes that safe: a floating bus would pass for QUUX once in 65,536 at
worst. A CADR reading source 16 would settle it.

## The PDL buffer

**QUUX's PDL buffer is 16K words**, its pointer and index 14 bits where the
CADR's are 10 (revision 2). They read back whole in functional sources 2 and
3, whose upper bits read 0 on the CADR. `quux_s_pdl_buffer_is_4k_or_16k` in
`tests/quux.rs` holds a push past word 1777 landing above it and the wrap at
the buffer's own size. It needs QUUX's boot PROM (below): MIT's stops copying
A memory in on the index wrapping at 2000 words. Microcode for QUUX has to
know the size.

## The feature page

**QUUX lists its sizes in words 0-77 of its register page**, physical
`17777400` to `17777477` ([the register page](#the-register-page), page
37777). They are read-only and read like any device register, through the
map:

| Word | QUUX, revision 12 |
|---|---|
| 0 | the MACHINE-ID, as source 16 gives it |
| 1 | level-1 entry: 6 bits |
| 2 | level-2 map: 2,048 entries |
| 3 | PDL buffer: 16,384 words |
| 4 | control store: 16,384 words |
| 5 | A memory: 1,024 words |
| 6 | dispatch memory: 2,048 words |
| 7 | multiply and divide: 3, bit 0 `MUL` and bit 1 `DIV` |
| 10 | the tick, timer 0: 1 |
| 11 | the main screen: width in 31:16, height in 15:0 |
| 12 | the main screen: bits a pixel in 31:16, words a line in 15:0 |
| 13 | the main screen: its buffer's first physical address |
| 14 | the microsecond clock: 1 |
| 15 | the optional devices, a bit each: 3, bit 0 the real-time clock and bit 1 the file device; a later optional device takes the next bit |
| 16 | the number of interval timers: 3 |
| 17 | the MACRO DISPATCH MEMORY's entries: 1,024 |
| 20-24 | revision 13: the board name, 4 characters a word in 31:0; 0 below revision 13 |
| 25-77 | 0 |

Word 15 reads 0 below revision 9, as every unused word does, so software
decides by it whether the real-time clock and the file device are there;
word 16 reads 0 below revision 10, and so says whether the interval timers
and reset devices are; word 17 reads 0 below revision 12, and so says
whether the fused return is there. Word 14 named Q1's interval timer too up to revision
9, which revision 10 drops.

**The board name**, words 20-24 on revision 13, names what runs the
machine: up to 20 characters of printable ASCII, `040`-`176`, the codes the
Lisp Machine's character set shares with ASCII. Character `i` is in word
`20 + i / 4`, bits `8 (i mod 4) + 7` to `8 (i mod 4)`, the first character in
`<7:0>`; the first zero byte ends the name and every byte after it is zero,
so a 20-character name has none, and word 20 `<7:0>` 0 is no name. It is
read-only, printed and never branched on: a program that needs a property
reads the word that states it. muir-sim's is `muir-sim` on both engines.
`Machine::set_board_name` sets another, so that a fabric under test and
muir-sim read the same page, and refuses a name over 20 characters or with
a byte outside `040`-`176`; it is not a flag, since a name says what runs,
and a checkpoint does not keep it. `the_board_name_is_muir_sim_on_both_engines`,
`set_board_name_fills_words_20_to_24` and
`set_board_name_refuses_what_does_not_fit` in `tests/revision_13.rs` hold it,
the first two on both engines.

Words 11 to 13 describe whichever display is fitted: the video controller's 1280 by 1024,
one bit a pixel, 40 words a line at `17000000`, or, on a QUUX run with a CADR
board, that board's 768 by 963, one bit, 24 words a line at the same address.

On the CADR the page is the last of its Unibus window, Unibus
`777000`-`777776`, where nothing answers: a read there times out and sets
the Unibus NXM bit, `766044` `<3>`, as a read of any empty Unibus address
does. Software reads source 16 first and the page only on QUUX, and so never
waits for the timeout. `quux_lists_its_sizes_in_its_feature_page` in
`tests/quux.rs` holds the page on QUUX and the timeout on the CADR, on both
engines.

## Multiply and divide

**QUUX multiplies in one instruction and divides in one**, ALU functions 42
and 43 (revision 3). The CADR takes a step per bit: `MUS`, `DVS1`, `DVS` and
`DVREM` are ALU functions 40, 51, 41 and 45 (`mit/cadr/ir.bits`, ALU
FUNCTIONS), and microcode 323's `MPY` runs 32 multiply steps, its `DIV` a
first step, 31 steps, a last step and the remainder correction (System 1001's
`sys/ucadr/uc-arith.lisp`). On the CADR, 42 and 43 are no multiply or divide:
the 74S139 at SOURCE 3D04 that makes `-MUL` and `-DIV` from `IR<4:3>` has
outputs 2 and 3 unconnected, so they are the 74S181 functions their bits
select.

| | `MUL`, 42 | `DIV`, 43 |
|---|---|---|
| Is | 32 `MULTIPLY-STEP`s | `DIVIDE-FIRST-STEP`, then 31 `DIVIDE-STEP`s |
| M source | the high word to add into, usually 0 | the high dividend |
| A source | the multiplicand | the divisor |
| `Q` before | the multiplier | the low dividend |
| Output bus | the product's high word | the partial remainder |
| `Q` after | the product's low word | the quotient; `Q<31>` the first step's bit, set on overflow or a zero divisor |
| Time | one ordinary microcycle, once its operands are ready | ten microcycles in all: nine held once its operands are ready, then its own; 400 ns at four ticks |

Each step's M operand is the output bus of the step before, as when the
microcode writes the step's result back to the same M location. Both
instructions decode on `IR<8>` and `IR<4:3>` only, as the '139 does, and
drive the output bus and load `Q` whatever the output selector `IR<13:12>` and
the Q control `IR<1:0>` say. `DIVIDE-LAST-STEP` and `DVREM` stay the CADR's
separate instructions.

**A `DIV` takes ten microcycles, its own and nine held after its operands
are ready**, QUUX's definition as ruled on 25 September 2026. A register is
ready at once, so a `DIV` of a register closes 400 ns after it entered `IR`
at four ticks; muir-fpga's fabric closes it on the same tick
(`cadr_microcycle.sv` line 2192, its trace `quux_muldiv.quux.k4.golden`
row `c4`). An
`MD` operand first waits for its read to land, through the MD interlock any
instruction reading `MD` has; the count starts in the microcycle after that
wait ends. A `MUL` of `MD` waits the same and then takes its one
microcycle. muir-fpga's measurement is what the rule settles: since
contract Q6 a main-memory read releases at its acknowledgement, so a `DIV`
of `MD` can run in the microcycle after the word lands, and the fabric's
divider needs its operand 17 ticks before the `DIV` ends. Its program
`quux_divmd` puts a `DIV` of `MD` there; counted from the `DIV`'s entry into
`IR`, the hold runs during the wait and the new word is divided at once,
where the fabric divided the old `MD` --- at microcycle 81, an output bus of
`a850e26d` against the fabric's `11465777`. Counted from the end of the
wait, both divide the new word.

The hold is a `-WAIT` term of QUUX's own: the divider is busy while a `DIV`
stands in `IR`, not nopped, and `muldiv::DIV_CYCLES`, nine, generator cycles
have not passed since the edge that loaded `IR` or, if the MD interlock held
it, since the end of that hold. The master clock runs on, so the memory port
carries on. Nine held microcycles of QUUX's 40 ns at four ticks is 360 ns,
which covers the divider's 330 (32 quotient bits and a load at 10 ns each), and muir-fpga
finds it fits with 15 ticks to spare. The count is in microcycles, not
nanoseconds: at another `--sync-cycle-ticks` muir still holds nine, ten in
all, and a
contract that changes the microcycle's length recounts it. The time does
not depend on the operands. A halt during the hold stops the machine with
the `DIV` still in `IR`. The hold does not stop a single step, as `-WAIT`
does not; by then the divider is done.

**Unverified:** whether the fabric starts the count after `-WAIT`'s other
terms too --- a memory destination written while a cycle is busy, a fetch
--- which hold the instruction and not its operands. muir counts through
them, since the rule counts from when the operands are ready; a `DIV`
with a memory destination behind a busy cycle, run on both, would settle
it.

`tests/muldiv.rs` holds each against the step sequence on the CADR, for many
operands and every output selector and Q control, on `micro` and `rtl`, with
the step sequence itself held to the netlist; the CADR's 42 and 43 to the
netlist; the hold's length on both engines, from a register's `DIV`'s start
and from the end of a miss's wait for a `DIV` of `MD` (the `DIV` ends nine
microcycles after a plain copy of `MD` in its place, 21 after the read's
start on `rtl` where the copy ends at 12), and at three ticks; a `MUL` of
`MD` ending with the copy; the CADR's 43 held for nothing; a halted and
single-stepped `DIV`; and a checkpoint taken during one.

## The clocks

**QUUX has clocks of its own**: a microsecond clock in the processor
(revision 5), and three interval timers on the register page, timer 0, 1
and 2 (revision 10, contract Q11), timer 0 being the tick, the machine's
60-cycle clock. The CADR has no clock in the processor. Its clock is the
display board's vertical interrupt: microcode 323's `INTRX0`
(`sys/ucadr/uc-interrupt.lisp`) reads the TV's mode register, clears its
vertical flag, and runs the "roughly-60-cycle clock" handler --- the mouse,
the disk's idle time, the Chaosnet's transmit-abort wakeup, and the
sequence-break counter the scheduler runs on. Its microsecond clock and
interval timer are on the I/O board, on the Unibus (`764120`-`764124`).

**The interval timers** are identical, each a block of two words:

| Word | Read | Write |
|---|---|---|
| 110, 112, 114: timer 0, 1, 2's control and status | `<0>` on, `<1>` its flag, `<2>` its mode (0 periodic, 1 one-shot), `<8>` its interrupt enable; the rest 0 | `<0>` on; a write with `<1>` set clears the flag; `<2>` the mode, taken only by a write that turns the timer on; `<8>` the interrupt enable, taken by every write; the rest ignored |
| 111, 113, 115: timer 0, 1, 2's period | the period in µs, `<23:0>`, as last written; the rest 0 | `<23:0>` the period, 1 to 16,777,215 µs; the rest ignored |

- A write that turns a timer on starts a period from the write and takes
  its mode from it; one that leaves it on starts nothing and changes no
  mode; one that turns it off takes its flag down, and its mode reads as it
  was until the next turn-on. A write of one timer's words touches that
  timer alone.
- A period written while the timer is on starts a period from the write,
  and so takes the flag down: a rise up and not yet taken is lost. Written
  while it is off, it sets the period only. A timer on at period 0 never
  rises.
- **Periodic**: the flag rises a period after the start and every period
  after, on the start's grid, whether or not it was cleared between; while
  it is up, further rises merge into it, and a clear takes it down until the
  next.
- **One-shot**: the flag rises once, a period after the start. The timer
  stays on with nothing to count, as a timer on at period 0, until a period
  write or an off-then-on starts it again. A clear starts nothing, and under
  a raised flag nothing more is counted: a one-shot that has risen and been
  cleared reads as one armed and not yet risen, on with its flag down.
- **The flag rises whatever the interrupt enable says**, so a timer can be
  polled through its word. Under `<8>`, it is the timer's bit of word 100
  --- `<0>` timer 0, `<1>` timer 1, `<2>` timer 2 --- and is ORed into the
  interrupt pending that jump conditions 5 and 6 test: a level, down when
  the flag is cleared, the timer turned off or `<8>` cleared.
- **Reset**: power-on, `-RESET`, `-BOOT` and reset devices (below) each put
  every timer off, its flag down, periodic, interrupt enable 0 and period
  0. No timer has a period of its own: timer 0's 60 Hz, 16,667 µs, is
  written by the boot PROM since revision 10 (contract Q11). The microsecond
  clock moves on through every reset.
- **Instants.** A timer word is read or written at the instant the
  register's cycle is taken, the edge the memory port takes it at (`rtl`:
  the edge that ends the microcycle after the memory start), and a period
  starts from that edge; a write is in `SINTR` from the next edge on, the
  one that acknowledges the cycle, and not at the edge that takes it. A read
  gives the flags as they stood at that edge, a rise at the edge itself
  counting as before it.

A flag that rises while a microcycle waits for `MD` is up for the jump
after it: `SINTR` is registered at the edge that ends the waiting
microcycle, with the flags as they stand then (the case muir-fpga measured
on Q1's timers).

**Functional destinations 3 and 4 write only M, and functional source 17
reads all ones**, on QUUX since revision 10 as on the CADR: Q1's tick control,
its interval timer's period and their status, which they were up to
revision 9, are gone, and every timer is reached through the register page
alone. A write of destination 3 changes no timer, also at an edge that
takes a register write, and takes no flag out of any `SINTR`. So
microcode that turns its tick on through destination 3, as QUUX's System
1002 dev11's did, has no tick since revision 10.

**The microsecond clock** is functional source 15: the microseconds since
power-on, 32 bits, wrapping, one read giving the whole word.

On the CADR, destinations 3 to 7 have no output on the 74S138 that decodes
them and write only M, and sources 15 and 17 have none either and read all
ones. Microcode 323 writes destinations 3 to 7 and reads sources 15 and 17
nowhere, by a scan of every control-store word, and running shows the same
of what the OA registers make at run time: `tests/unused_codes.rs` reads
every executed microinstruction as it stood in `IR` through a boot to the
listener. System 1002 on the CADR's microcode 1000, MIT's 323 with three
of MIT's fixes, on the CADR, runs none. System 2000 on
microcode 2000 runs them at four addresses through its boot and a moment
after on revision 11, each a control-store word that carries them, and
each a read of source 15, the microsecond clock: in `RESET-MACHINE`, the
start of its wait for the file device, at `READ-MICROSECOND-CLOCK`, in
`XUSLDB`, and at 20346. It writes no destination 3 to 7 and reads no
source 17, and timer 0 is on at the end, turned on through the register
page (`system_2000_uses_the_clocks_codes_only_where_its_microcode_does`).

`tests/interval_timers.rs` holds, for each timer, the periodic grid to the
nanosecond with late clears and a period written under a raised flag, the
one-shot's one rise, the mode taken at turn-on, the independence of the
three, every reset on both engines, the interrupt under `<8>` and not
without it on both engines and inside a wait for `MD`, the layout,
destination 4 and source 17 on QUUX, a checkpoint resumed to the same
rises, destination 3 changing no timer on either engine, and on `rtl` the
shared edge: destination 3 changing nothing beside a register write, nor
its `SINTR`, nor a read's flags, which are as they stood at its edge.
`tests/tick.rs` holds the tick as timer 0 at the PROM's period, turned on
through word 110 and not through destination 3, the microsecond clock
against each engine's time (under `sync` too) and across its wrap, and the
CADR's all ones.

## QUUX drops the delay lines

**A QUUX microcycle is a fixed number of 10 ns ticks, always**: `sync`,
`--sync-cycle-ticks` of them, 4, the DE25-Nano's, unless a board's fit
says otherwise: the Arty Z7-20 runs revision 12 at 4 and revision 13
at 5. The CADR's clock is a string of delay-line phases: the read phase ends at the
tap the mode register's `SPEED1` and `SPEED0` choose (`mit/cadr/ir.bits`:
"00 Extra slow, 01 Slow, 10 Normal, 11 Fast"), and a 60 ns restart follows,
145 ns at normal speed, which muir-fpga's fabric replays as 15 ticks. QUUX
has none of it: no taps, no speed bits (a write of the mode register's bits
1 and 0 goes nowhere; its other bits are unchanged), and no timing but
`sync`. `--timing-model` is `cadr`'s alone, and an engine
made for a QUUX machine starts on four ticks: `rtl` refuses another timing
on it, and `micro`'s clock counts the same ticks.

The ticks are a board's: the number its fit proves its longest path settles
in. **Four ticks, 40 ns, is met on the Arty Z7-20**: muir-fpga's QUUX at
four ticks, with contracts Q1-Q5 and block-disk, its commit `2535395`
(against muir at `bcb6242`), has a worst setup slack of +0.461 ns and no
failing path, in 11,974 LUTs; an earlier fit of
the same design measured the longest chains as MD through both
map levels and the M bus to the control-store address, 28.0 ns of 40, and
the multiplier, 24.1 ns of the 30 a path after the scratchpads gets. Both
boards run revision 12 at four ticks: three, 30 ns, is out of the
DE25-Nano's reach by the divider alone, a `DIV` of `MD` needing its word 17 ticks before its
hold ends. The DE25-Nano at four ticks meets them at every corner, a worst
setup slack of +0.180 ns at `2535395`, its thinnest path the microsecond
clock's count under a constraint a tick tighter than the microcycle's (an
earlier fit's longest chain was MD through the maps to the next address at
16.1 ns); the level-1 map's MLAB write-to-read, which Quartus does not time,
is **unverified** by timing analysis, the rest of that path having 30 ns and
measuring about 16.

**Revision 13 on the Arty Z7-20 runs at five ticks**, 50 ns: its map, two
levels through the memory path's decode, misses four ticks on that part by
about a nanosecond and meets five with a setup slack of +0.153 ns
(muir-fpga's commit `ccc3d12`). The tick stays 10 ns, so the clocks that
count ticks keep true time. The DE25-Nano runs revision 13 at four. muir's
default stays 4 for both revisions; `--sync-cycle-ticks 5` is the Arty's
revision 13.

What the microcode sees does not change, only the time: every register is
clocked at the one edge and the late writes land one edge later, as the
single-edge contract below has it; the bus keeps its own time on the grid;
a held cycle is one microcycle long; and a `DIV` is ten microcycles, its
own and nine held, at any length. An `ILONG` instruction takes `ilong_ticks`
more, 0 unless the library says otherwise.

`tests/sync_timing.rs` holds the microcycle's length, the default and the
refusal, `ILONG`'s ticks, the grid, the divider's hold and a checkpoint;
`quux_has_no_speed_bits` in `tests/quux.rs` the speed bits;
`quux_runs_on_sync_alone` in `tests/cli.rs` the flags; and
`system_2000_runs_at_its_ticks` in `tests/system_2000.rs` System 2000 at
four ticks and at three.

## The memory port and the device registers

**QUUX's main memory and its frame buffer are on the processor's own
port**, the memory bus (contract Q6, revision 7; the frame buffer from Q7,
revision 8), and **its devices are reached by their registers alone**
(contract Q7, revision 8): there is no Xbus and no bus in its place. A
cycle to main memory or the frame buffer goes through the cache to the
memory controller; one to a device register goes to the processor's
register decode; one to any other address fails at once. QUUX has no bus
interface: the Unibus is gone (Q5) and the processor is the only
requester. Devices move bulk data to and from memory themselves ---
block-disk's transfers, the display reading its buffer --- and carry
control and small data in their registers. The CADR keeps its Xbus, Unibus
and bus interface. The boot PROM is not in the address space: it is in the
control store (Q2).

| | |
|---|---|
| Main memory | 380 ns a line fill and 290 ns a write (`MemoryTiming::NOMINAL`, the DE25-Nano's, the slower board's), one operation at a time, no setup, deskew or refresh; a floor, a board slower on an access waiting. `--memory-timing <read>,<write>`, `arty` or `de25` sets others, on `rtl` |
| The frame buffer | `17000000` up to the video controller's buffer's end, which the address map lets reach `17777377`, 261,888 words: on the memory bus with main memory, cached. The software reads it back (`BITBLT` combines with the destination, scrolling copies), and the display only reads it, which a write-through cache keeps current |
| The cache | always fitted: 4K words (`--cache <words>` another size) |
| A device register | a word of the register page, `17777400`-`17777777`, the feature page, block-disk's and the video controller's among them: never cached, taken at the edge and answered a microcycle on, two microcycles in all |
| Nothing there | past main memory's or the frame buffer's end, and everything else from `17000000` up below the register page --- `17377000`-`17377777` and the old Unibus window among it: fails at once, in the microcycle, reading 0 and setting word 101's NXM bit, with no timeout. A write there does not read back, which the microcode's memory-size probe, `MEM-SIZE-LOOP` in `uc-cold-disk.lisp`, relies on; nothing in QUUX's System 1002 or System 2000 depends on how long a failed access takes: `MEM-SIZE-LOOP` is the one probe, and it tests the value read back, not the time (muir-sys, read) |
| Block-disk | its words move at START, and the cache is invalidated; a transfer reads the processor's writes made before START and, after DONE, no read hits a word from before it |

The bus interface's registers all have homes on QUUX already: the
diagnostic registers are the host's (Q5), `ERRSTOP` is word 102, the
interrupt control word 100 and the error status word 101 (Q2); the Unibus
map is gone (Q5).

`tests/quux_memory_port.rs` holds the port in place of the bus interface,
a miss's line fill against a hit, a register never cached, nothing past
main memory's end reading back on both engines, the disk's write never hit
stale, and a checkpoint; `tests/quux_device_registers.rs` a register a
microcycle longer than nothing, every empty range failing at once on both
engines, the frame buffer through the cache, and a register write held
behind a register read keeping `MD` no longer than a main memory write;
`quux_s_memory_port_and_its_timing` in `tests/cli.rs` the flag and the
start's report.

## The memory cache

**QUUX's memory cache** (H2) is unified and write-through, in front of main
memory and the frame buffer, by physical address after the map. Device registers are not cached. In muir it is `rtl`'s, and
it holds tags only: `rtl` takes a word from main memory as a cycle ends,
and a write-through cache never holds a word memory does not, so the cache
changes when a cycle is answered and never what it reads.

| | |
|---|---|
| A read that hits | acknowledged `hit_ns` after the request (20 ns, two ticks of the grid, met on the Arty in muir-fpga's fit; the DE25's to follow), with no bus cycle and none of the bus's setup, deskew or release |
| A read that misses | main memory's line fill; it fills the line, the set's least recently used line going |
| A write | allocates nothing. With the write buffer it is acknowledged after `hit_ns`, or when the buffer's last write is done, and main memory runs it behind the processor; the word is memory's from the acknowledgement, as a read after it finds it |
| Shape | lines of 4 words, 2-way, `<words>` in all, a power of two |
| Coherence | a disk transfer writes main memory behind the processor, and invalidates the whole cache before the next cycle |

Behind the cache is main memory on the port. muir-fpga measured its two
boards, a 1,000,000 word array loop on System 1001 for 300 s:

| | Read, average | Write | In muir |
|---|---|---|---|
| Arty Z7-20 | 20.68 ticks of 10 ns (19 to 88) | 12 | read 220 ns, write 120 ns |
| DE25-Nano | 36.25 (33 to 228) | 29 (28 to 58) | read 380 ns, write 290 ns |

rounded up to the tick, with a tick more on a read for a line fill of four
words, two 64-bit beats: the machine has only ever made single-word
accesses, so a fill's time is **unverified**.

Measured with main memory as the CADR's memory boards on the Xbus, and as
the boards' own timings where the table says so, with the profile harness (`examples/profile.rs`, `MUIR_CACHE` and
`MUIR_SYNC_TICKS`) on `rtl`, System 1001's band on QUUX's microcode 1000
for revision 4, over ten workloads (compile, two call-heavy, cons, the
multiply-and-divide, float, array, sort, bignum, intern), in the machine's
time:

| | Total | |
|---|---|---|
| the CADR's delay lines, 145 ns, the baseline | 72.7 s | 1.00 |
| the same, cache of 4K words | 58.6 s | 1.24 |
| `sync` of 4 ticks, no cache | 33.7 s | 2.15 |
| `sync`, cache of 1K words | 19.6 s | 3.70 |
| `sync`, cache of 4K words, no write buffer | 18.9 s | 3.84 |
| `sync`, cache of 16K words, no write buffer | 18.6 s | 3.90 |
| `sync`, cache of 4K words and the write buffer | 17.7 s | 4.11 |
| `sync`, the Arty's memory, no cache | 21.9 s | 3.32 |
| `sync`, the Arty's memory, cache of 4K words and the buffer | 15.5 s | 4.70 |
| `sync`, the DE25's memory, no cache | 26.4 s | 2.75 |
| `sync`, the DE25's memory, cache of 4K words and the buffer | 16.2 s | 4.48 |

Read hits run from 75 to 99 per cent of reads at 1K words and from 81 to 99
at 4K; past 4K words little more is gained. The cons workload is written
far more than read --- over four in five of its memory cycles are writes
--- and the write buffer is most of what the cache does for it.

`tests/cache.rs` holds the lines and the replacement, a read loop and a
write-and-read-back loop leaving the same words with the cache as with one
that saves nothing, and sooner, the invalidation, and a checkpoint.

## Block-disk

**QUUX's disk is block-disk**, and nothing else: a `quux` run has it,
and `--disk-controller`, the CADR's controller, is `cadr`'s alone
(`quux_s_disk_is_block_disk_only` in `tests/cli.rs`). It is the CADR
disk controller's programming interface with the drive's geometry taken
out. Blocks are numbered from the start of the pack, and each is 256 words,
a page.

| | CADR controller | block-disk |
|---|---|---|
| Registers | `17377774`-`17377777`: status and command, command list pointer, disk address, START | the same four, words 200-203 of the register page, `17777600`-`17777603` |
| Command list | one word a block, `<23:8>` the page's physical address, `<0>` More | the same |
| Disk address | cylinder `<27:16>`, head `<15:8>`, sector `<7:0>`, unit `<30:28>` | the block number, `<27:0>`; one pack, unit 0 |
| Commands | read, read compare, write, read all, write all, seek, at ease, recalibrate, offset clear, reset | read, 0, and write, 11; any other stops by error |
| Status | not active, attention, errors of the drive, the ECC and the transfer | `<0>` not active, `<3>` interrupt request, `<9>` no pack, `<13>` stopped by error, `<17>` past the end of the pack, `<20>` NXM |
| After a transfer | the disk address at the last block moved, or the one that failed | the same |
| Interrupt | done, command `<11>`; attention, `<10>` | done, command `<11>` |
| Time | seeks and rotation, when timed | 100 us a block moved, **unverified**: an estimate until muir-fpga measures its disk path |

**Block-disk's contract is the outcome, not the time**: the status bits, the
disk address left at the last block moved or at the one that failed, the
command list pointer and memory. How long a transfer stays active, whether
it succeeds or fails, is the implementation's, and software waits for
not-active before it reads the status. muir's model does as follows, and a
fabric that walks its list through a host stays active until the walk ends,
a failure too.

The words move inside the store to START, and the controller stays busy for
a block's time each; a transfer writes main memory behind the processor, so
the memory cache is invalidated. The disk is a file in a standard format,
[below](#the-disk-file). `tests/block_disk.rs` holds the read, the write,
the end of the disk, the NXM, a command it does not do, the registers on
QUUX's register page with nothing at the CADR's, and a checkpoint.

muir-sys's PROM 2000, microcode 2000 and System 2000 address it by
block: System 2000's band boots on it on `micro`, `rtl` and under `sync`,
at 1024 by 768, 1280 by 1024 and 1920 by 1080 (`tests/system_2000.rs`). Lisp checks the disk address after a transfer
against the last block it expected, and reads the command list pointer
and the disk address after one; it does not read the fourth register,
where the CADR's controller gave the ECC.

## The disk file

**QUUX's disk is a raw image, a fixed VHD or a dynamic VHD, of any size**
(contract Q8); the CADR's pack stays MIT's, a raw Trident image exactly a
T-300's size, and a file of any other size is refused on it as before
(`the_cadr_refuses_quux_s_disks_as_before` in `tests/quux_disk.rs`).
Block-disk's block `n` is the file's 512-byte sectors `2n` and `2n + 1`,
and the disk's size in blocks is the file's size, or a VHD's current size,
over 1,024; a last half block is not reachable. A VHDX, a differencing VHD
and a VHD whose footer or dynamic header does not check are refused, saying
which (`what_muir_does_not_read_is_refused`).

| | raw | fixed VHD | dynamic VHD |
|---|---|---|---|
| The file | the disk's bytes | the disk's bytes, then a 512-byte footer | a copy of the footer, a dynamic header, the block allocation table, 2 MiB blocks each after a 512-byte sector bitmap, the footer |
| Told by | no `conectix` footer | the footer at the end, disk type 2 | the footer at the end, disk type 3; or, with the one at the end lost, its copy at 0 |
| The disk's size | the file's | the footer's current size, offset 48 | the same |
| A write | in place | in place, the footer untouched | in place in an allocated block; an unallocated one is allocated where the footer was, its bitmap all ones, then the footer after it, then the table entry |

**The format is the footer's, not the name's**: a fixed VHD called `.img`
is still a fixed VHD (`the_format_is_the_footer_s_not_the_name`), and
qemu-img reports a fixed VHD as raw unless told `-f vpc`. The start says
which of the three a QUUX run's disk is, and its size in blocks
(`quux_s_disk_is_raw_or_a_vhd_of_any_size` in `tests/cli.rs`).

`data/quux-disk*` are an 8 MiB disk in all three formats and a dynamic VHD
after three writes into unallocated blocks, with its raw twin, made by
qemu-img and qemu-io (`tools/quux-disk-fixtures.sh`). Every block of each
reads through muir as its raw twin (`each_fixture_reads_as_its_raw_twin`);
muir's own writes of the same bytes into the dynamic VHD make qemu-io's file
byte for byte, the footer checking and its copy at 0 unchanged
(`a_write_grows_a_dynamic_vhd_as_qemu_does`); and where qemu-img is installed
it compares a dynamic VHD muir grew equal to the raw file given the same
writes (`qemu_reads_what_muir_grew`, skipped without it). A disk opened
`ro` keeps its writes for the run and a checkpoint, the file untouched
(`opened_read_only_nothing_reaches_the_file`); a checkpoint carries the
disk's size in blocks and refuses a disk of another
(`a_checkpoint_keeps_the_disk`).

**The partition table is a GPT.** A partition's type is one of QUUX's own
type GUIDs, whose first 32-bit words all differ:

| Partition | Type GUID |
|---|---|
| microcode, `MCRn` | `9e318cf5-a95b-4b3b-b2ad-9ae306b0e2da` |
| band, `LODn` | `a3b30470-c5d4-41c1-87a8-d26590424cb8` |
| `PAGE` | `4652bea5-06af-4bd9-b2bb-3541370151c8` |
| `FILE` | `7afa9532-75de-409f-8dc8-fef9763511d5` |
| retired: once `TEMP`; not to be used | `445976f2-34e4-4583-b750-75d28a080cba` |

- **The name** is the partition's four-character Lisp name, a space and a
  comment of up to 31 characters, `MCR1 UCADR 2000`: 36 characters, which
  is what a GPT name holds.
- **The current microcode and the current band** carry attribute bit 48,
  the first of the bits a GPT leaves to the partition type.
- **Whole blocks**: a partition's first LBA is even and its last odd, so it
  starts and ends on a block.
- **At most 8 GiB**, 2^23 blocks, because Lisp's fixnums hold block numbers
  below 2^23. Block-disk's address, `<27:0>`, reaches further, and muir
  opens a larger file; the limit is the software's.
- **No pack name and no pack comment**: a GPT has neither.
- **There is no TEMP partition**, and the fixtures have none. MIT's PROM
  saves page 0 to block 1 of its pack, which on a GPT disk is the
  partition table; QUUX's saves nothing and writes no block of the disk
  ([below](#its-boot-prom-in-its-own-addresses)). No fixture partition
  carries the retired type GUID (`the_fixtures_gpt_is_q8_s`).

`the_fixtures_gpt_is_q8_s` reads the fixtures' GPT through muir's disk
layer and holds them to all of this. muir reads no partition of QUUX's
disk itself: block-disk moves blocks, and the partitions are the machine's
software's to find. `diskpack`, MIT's label editor, is the CADR's; given a
QUUX disk it says what the file is and that its partitions are made with
sgdisk, and writes nothing (`quux_s_disk_is_named_and_left_alone` in
`tests/diskpack.rs`). The boot PROMs, `data/quux-promh-2000.mcr` and
revision 13's `data/quux-promh.mcr`, the microcode and the bands read the
GPT; MIT's label in block 0 is the CADR's.

**System 2000's band is on a GPT disk in a dynamic VHD**: QUUX's
release, muir-sys's `release-2000`, which `tools/fetch-system-for-quux.sh`
fetches, a T-300's 263,245 blocks with the current `MCR1` at block 17,
"MCR1 UCADR 2000", holding microcode 2000 and the current `LOD1`, "LOD1
System 2000", the band, no FILE and no TEMP
(`band_2000_is_system_2000_on_microcode_2000`). QUUX boots the VHD as it
is, a copy of it, the disk being written; the tests find the release in
the gitignored `vendor/` and skip without it. Its `SYS:` is on the file
device, which the tests serve the release's sources through. It reaches
its listener, drawn at the screen's own words a line, in 156.5 M
microcycles on `micro` and 165.5 M on `rtl` at 1280 by 1024, with 2000 in A
memory's `A-VERSION` (`system_2000_runs_on_the_video_controller`, measured to the half
million). It restores its own band: `(si:disk-restore 1)`, answered
`yes`, reads 18,632 blocks of `LOD1` in 24 M microcycles and is back at
the listener 131.5 M later, on `micro`
(`system_2000_restores_its_band_to_the_listener`). That
test holds, at every microcycle in `DISK-AWAIT-READY`, the disk registers'
virtual address `77777600` to their physical `17777600`, and block-disk to
moving on every million microcycles; and after it the MACRO-DISPATCH
register enabled again by the restored microcode, and returns fused
again. The cold boot's `COLD-FAKE-L2-MAP`
maps the disk registers and the run light; when the two take one level-2
slot, the disk registers' virtual address reaches another word, and the
restore waits in `DISK-AWAIT-READY` for ever. On a microcode with that
collision, QUUX's System 1002 dev9's, the test fails at the first check,
the address reaching `17117774`, and without it at the second (measured).

How to make a disk with standard tools is in
[the manual](manual.md#quuxs-disk).

## The single-edge contract

**What a single-edge core must keep**, for H1b: in which microcycle each
resource's new value is seen, counted from the microcycle whose
instruction produced it (cycle *n*). It is `rtl`'s read phase, write
phase and edge, which `tests/cosim.rs`, `tests/chip.rs` and
`tests/dispatch_write_order.rs` hold to the netlist; an FPGA core that
keeps every row, and stalls where a row needs it, runs the microcode
unchanged.

| Resource | Written | Seen by |
|---|---|---|
| `IR` | at the edge ending *n*, from the I bus: the control store at `PC`, the boot PROM, the debug IR, or `IWR` in the cycle after `WRITE-I-MEM` | the instruction executed in *n*+1. The instruction at a jump's target runs in *n*+2; the one after the jump runs in *n*+1 unless `N` inhibits it |
| `PC`, `LC`, `Q`, `VMA`, `MD` (from the processor), `INTERRUPT-CONTROL`, the PDL pointer and index, the SPC pointer, the flags | at the edge ending *n* | *n*+1 |
| A memory, M memory | in *n*+1's write pulse, from `WADR` and `L` registered at the edge ending *n* | *n*+1, through the pass-around (ACTL 3B21/3B27, MCTL 4B18: a source address equal to the pending `WADR` reads `L`); the memory itself from *n*+2 |
| PDL buffer | in *n*+1's write pulse, at the pointer or the index as it stands then: only the choice between them is registered (`PWIDX`), not the address | *n*+2: no pass-around, so *n*+1 reads the word as it was (`pdl_read_right_after_a_push_on_the_board`, `tests/cosim.rs`) |
| SPC stack, a push | in *n*+1's write pulse, at the pointer the edge ending *n* moved to | the next-address path in *n*+1 (`SPCWPASS` puts the word on the `SPC` bus); an M-source read of the stack in *n*+1 reads the RAM's old word at the new pointer; *n*+2 reads the new |
| Map, a `WRITE-MAP` store | in *n*+1's write pulse, both levels, addressed by `MAPI` then (`VMA` while `MEMSTART`, else `MD`) | `MAP(MD)` and a dispatch on map bits read the old word in *n*+1 (QUUX's definition; below) and the new from *n*+2. A memory cycle an instruction in *n*+1 starts is translated at the edge ending *n*+2, through the new word: `PHYS-MEM-READ` stores the map and starts a read in the next instruction |
| Dispatch memory, a dispatch write | in *n*'s own write pulse, at its own `DADR` | the dispatch in *n* reads the old word (QUUX's definition); from *n*+1 the new |
| Control store, `WRITE-I-MEM` | in *n*+1's write pulse, at the `PC` it moved to | *n*+1's `IR` takes the word from `IWR` directly, not from the RAM; later fetches the RAM |
| OA registers, `IMOD` | at the edge ending *n* | or'd into `IR` as it loads at that same edge: the instruction executed in *n*+1 |
| `MD`, from memory | at `-LOADMD`, when the bus says | on the CADR, a microcycle that reads `MD` before `READ IN PROGRESS` falls is held by `-HANG`; on QUUX it waits (below) |

Holds and write pulses:

- A cycle held by `-WAIT` fires no write pulse: `TPWP` is `NOR(latch,
  -MACHRUNA)` at CLOCK2 1C10. The pending writes wait for the cycle that
  runs.
- On the CADR, a cycle held by `-HANG` fires its write pulse, which takes
  its address and data as the cycle's time ends: `MD` as the bus has left
  it then. Only the next cycle's start is held (`-TPR0`, CLOCK1 1C08).
- **QUUX has no hung microcycle** (`Geometry::hangs`): a microcycle that
  reads `MD` while a read is in flight waits as for `-WAIT`, whole
  microcycles with no write pulse, and runs once, whole, when the word is
  in `MD`. Unlike `-WAIT`, a single step does not pass it. Its writes
  therefore take their addresses from the word read: a dispatch write
  addressed by `MD`, and a map store's write pending into it, land where
  the word read says, where the CADR's land at the `MD` from before
  (`on_quux_a_dispatch_write_addressed_by_md_waits_for_the_word_read`,
  `on_quux_a_map_write_pending_into_the_wait_lands_at_the_word_read`,
  `on_quux_the_wait_for_md_is_whole_microcycles` in
  `tests/dispatch_write_order.rs`).
- A memory cycle goes out at the edge ending the microcycle after its
  start, and a write carries `MD` as it stands then: an `MD` loaded in
  that microcycle is the word written, on the CADR as on QUUX and on
  `rtl` and `micro` alike; one loaded later waits on `MBUSY.SYNC` for the
  cycle to end
  (`the_engines_write_the_md_of_the_microcycle_after_the_start` in
  `tests/chip.rs`, `a_write_carries_the_md_of_the_microcycle_after_its_start`
  in `tests/quux_memory_port.rs`).
- **QUUX holds a memory start in the microcycle right after a start**, a
  `-WAIT` term of its own, `MEMSTART AND MEMOP`, until the first cycle has
  gone out and ended; both then land as written, a first write with the
  `MD` from before the held microcycle, an instruction fetch held the
  same way (`a_start_right_after_a_start_waits_for_it` in
  `tests/quux_device_registers.rs`,
  `a_start_held_behind_a_write_loads_md_after_the_write` and
  `a_fetch_right_after_a_write_waits_for_it` in
  `tests/quux_memory_port.rs`). A register write held behind a register
  read does not put the read's word off: `MD` has it when the read's own
  `READ IN PROGRESS` falls, 140 ns after its acknowledgement, and the
  write's acknowledgement moves nothing, so a microcycle reading `MD`
  after one filler takes 80 ns, two microcycles, as with a write of main
  memory there
  (`a_register_write_right_after_a_register_read_holds_md_no_longer` in
  `tests/quux_device_registers.rs`). On the CADR nothing holds it: one cycle
  goes out for the two starts, with the second start's direction, page
  and `VMA<7:0>`, and the first is lost, measured on `chip` and held on
  `rtl` and `micro`
  (`on_the_board_a_start_right_after_a_start_loses_the_first`,
  `a_start_right_after_a_start_goes_out_as_the_second` and
  `a_fetch_right_after_a_write_loses_the_write` in `tests/chip.rs`).
  **Unverified** that muir-fpga's fabric holds it.
- **QUUX's definition**: a RAM read in the cycle its own write pulse fires
  --- the dispatch word a dispatch writes, the map word right after a map
  store --- gives the word from before the write
  (`Geometry::old_word_while_written`), as an FPGA's block RAM gives it.
  On the CADR the RAM's output floats while written and the answer is a
  race; muir's CADR engines take `chip`'s answer, the new word, which rests
  on its clock cutting the pulse at the edge
  (`on_the_cadr_rtl_and_micro_take_the_word_chip_does` and the two map
  tests beside it).
- **Nothing MIT's or muir-sys's microcode runs does either.** Counted on
  `rtl` over a boot of System 1001 and the profile harness's thirteen
  workloads, on QUUX (its microcode 1000) and on the CADR (System 1001's own
  microload, 324, which is 323 rebuilt): no dispatch write
  with `POPJ`, no map read in the microcycle a map store's write lands, and
  no hung microcycle whose pulse writes the dispatch memory or the map.
  The same counters fire on the test programs built to do each.

## The video controller

**QUUX's display is the video controller**, "video" for short, a monochrome
frame buffer: 1280 by 1024 unless `--video-size` gives another size, one
bit a pixel. The frame buffer is its memory. It is the frame buffer and one
word of the register page, and nothing else: no sync program, no color map,
and no interrupt, the machine's clock being the tick, timer 0 of the
interval timers. It is `quux`'s only display, and `--tv-board`, the choice
between the CADR's two boards, is `cadr`'s alone. MIT's "TV" names the
CADR's boards.

| | |
|---|---|
| Buffer | 40,960 words, physical `17000000`-`17117777`: 40 words a line, 1,024 lines |
| Pixel | pixel `x` of line `y` is bit `x mod 32` of word `40 y + x / 32` (at the default size), the low bit leftmost, as on the CADR's TV |
| Mode, word 210 of the register page, `17777610` | bit 2, black-on-white, reads back; every other bit reads 0 and a write of it is dropped |
| Words 211-217 | reserved: read 0, writes ignored |
| The CADR's control registers, `17377760`-`17377767` | nothing there: an access fails at once and sets word 101 `<0>` |
| Interrupt | none |

**Its size is muir's to choose**, `--video-size <width>x<height>`: the width
a multiple of 32, and at most **1920 by 1080**, the largest QUUX supports
(`a_size_is_checked` in `tests/video.rs`). That is 64,800 words, below the
color TV's buffer at `17200000`. The address map lets the buffer reach up to
below the register page, `17777377`: 261,888 words
(`the_buffer_reaches_up_to_the_page`), which no word states to the
software. The feature page's words 11 to 13 give the size to the software.
The table above is the default size.

**The boards' sizes differ**, each fixed in its bitstream: muir-fpga's Arty
Z7-20 and DE25-Nano are 1280 by 1024, its Kria KR260 1920 by 1080. A band
reads the size from words 11 to 13 at boot, so one band runs on each. At
1920 by 1080 the buffer is 60 words a line, 64,800 words; on revision 13 the
window is `1760000000`-`1760176437`, and `1760176440` is nothing and sets
word 101 `<0>` (`full_hd_s_window_ends_at_64800_words` in
`tests/revision_13.rs`, on both engines).

1280 bits a line is 40 whole words, which `BITBLT` needs of a screen array's
first dimension (`BITBLT-DECODE-ARRAY` in `sys/ucadr/uc-tv.lisp`). The buffer
starts where the CADR's does, so the band's `IO-SPACE-VIRTUAL-ADDRESS`
reaches it unchanged, and ends below the color TV's strap at `17200000`.

System 1001 runs on it but draws its screen wrong: `shwarm.lisp` makes the
main screen 768 by 963 at 24 words a line, and the video controller scans
40, so each of its lines is spread over parts of several. On QUUX's
microcode 1000 with the tick, for revision 4, the band reaches its listener
on `micro` in 136 M microcycles, as on the CADR's board, measured by reading
the rows the listener draws in at 24 words a line; its writes of the sync
program's registers fail and leave the NXM bit set, and nothing stops over
it. System 2000 sizes the main screen from the feature page's words 11 to 13
at every boot, and draws it right: with no sync program and no speed
bits, it reaches its listener with its herald,
listener and who line drawn at the screen's own words a line and at no
other width, at 1280 by 1024 on both engines
(`system_2000_runs_on_the_video_controller` in `tests/system_2000.rs`) and
at 1024 by 768 and 1920 by 1080 on `micro`
(`system_2000_sizes_its_screen_at_boot`).
**Unverified**: the sizes between, which no test boots.

`tests/video.rs` holds the buffer's first and last words and the NXM past
it on both engines, QUUX's decode of the whole buffer and of the largest
one, the pixel order and the terminal's frame, word 210 and the reserved
words after it, the CADR's registers answering nothing, the absence of an
interrupt over a second, and the feature page's three words.

## Its boot PROM, in its own addresses

**QUUX's boot PROM has control store addresses of its own**, 36000-37777,
1K words, read only and never overlaid (revision 6, contract Q2). Reset
starts the PC at 36000; the microcode lives in 0-35777, which is RAM from
the start, and there is no PROM-disable bit: the PROM loads the microcode
and jumps to 6. A reboot is a jump to 36000. The CADR keeps MIT's overlay:
its PROM covers 0-1777, the first 1K words, until `PROMDISABLE` in the
mode register, written at Unibus `766012`, lets the RAM show through.

The PROM is muir-sys's PROM 2000 for block-disk and a GPT
(`data/quux-promh-2000.mcr`; revision 13's, PROM 2001, is in
[Revision 13](#revision-13-the-40-bit-word)), MIT's `promh.text` changed so that a PDL buffer
of any width boots, QUUX's 64 level-2 blocks are cleared, the disk is read
by block number, nothing is saved, the microcode is found through the
GPT, the devices are reset through the register page and timer 0 given
its period (revision 10), and for revision 11 the register page is mapped
at physical page 37777 with block-disk's registers at its words 200-203
(muir-sys's `sys/ucadr/promh.text` as committed in muir-sys `62c4503`),
assembled at 36000. It sets error stop through the register page, not `766012`, and
halts at `ERROR-MICROCODE-TOO-BIG` if a microcode reaches 36000. The
control store stays 16K words: jump targets are `IR<25:12>`, dispatch
words carry 14 address bits, and `SPC<14>` is the macroinstruction-return
flag, so 32K waits for a new microinstruction format.

**It finds the microcode through the GPT**: the first microcode partition
in the entry array carrying attribute bit 48 (muir-sys). Its own halts are
`ERROR-NO-GPT` at 36642, no GPT (or an entry array whose LBA does not fit
in 32 bits, within the 8 GiB limit, muir-sys says);
`ERROR-NO-CURRENT-MICR` at 36644, no current microcode partition; and
`ERROR-ODD-MICR-START` at 36646, one whose first LBA is odd (the
hand-over's error table `promh.tbl` and symbols `promh.sym`). On a pack
with MIT's `LABL` label and no GPT --- a T-300 label with microcode 323 in
`MCR1`, which the PROM before it booted --- it reads block 0 into page 3,
nothing else, and halts at `ERROR-NO-GPT` after 629,625 microcycles on
`micro` and 660,870 on `rtl`, having written nothing
(`quux_s_prom_reads_a_gpt_not_mit_s_label`). **Unverified**: the other two
halts, which no test here reaches.

**It saves nothing and writes no block of the disk** (contract Q8). MIT's
PROM saves main memory's page 0 to block 1 before it loads anything
(`SAVE-A-PAGE`, `mit/sys/ucadr/promh.text`), and on a GPT disk block 1 is
the partition table's entry array. QUUX's reads every block into its buffer
at physical page 3, words 1400-1777, and loads the microcode's main-memory
section --- four blocks, pages 3-6, the microcode symbol area --- last,
over the buffer. Two halts are for that: `ERROR-TWO-MAIN-MEM-SECTIONS` at
36040, a second main-memory section with blocks, and
`ERROR-BUFFER-NOT-LOADED` at 36042, a section that does not cover the
buffer. 36000 is `JUMP GO`, and `GO` is at 36043; `DISK-AWAIT-PACK`, the
first disk routine, is at 36600, and the code ends at 36646 (`promh.locs`,
`I-MEM 36647`). `tests/quux_prom_saves_nothing.rs` boots it
on both engines until the microcode's location 6 runs, on
`data/quux-disk.img` with MIT's microcode 323 in its `MCR1` and on System
2000's VHD with microcode 2000, and counts: no block written; the only
stores are to word 777, the command list word, one a block read; every
block read goes into pages 3-6; the disk file is byte for byte as it was;
and pages 3-6 hold the main-memory section's four blocks. On System
2000's disk it reads 115 blocks and reaches 6 after 1,020,937 microcycles
on `micro` and 1,136,687 on `rtl`.

**Its file, like QUUX's microcode's, is in partition order** (contract
Q8): MIT's `.mcr` with the two 16-bit halves of every 32-bit word swapped,
so that each word is stored low byte first, as it lies in a microcode
partition and as block-disk reads it, and a whole number of 1024-byte
blocks, so that `dd` writes a microcode file into its partition with no
conversion. muir-sys's `sys/sys/qwmcr.lisp` writes it.
Swapped back, the PROM's file has MIT's `promh.mcr`'s four sections, its
dispatch and A memory word for word MIT's, only the program QUUX's
(`quux_s_prom_is_mit_s_promh_changed`); and System 2000's `MCR1` holds
the hand-over's `ucadr.mcr` block for block, as `dd` put it there
(`quux_s_prom_saves_nothing_on_system_2000_s_disk`). The CADR's `.mcr`
stays MIT's, and so does `diskpack`, which is the CADR's.

`tests/quux_prom.rs` holds the start at 36000, the PROM read only, the RAM
below live with no disable, the CADR's overlay, and the file read from
36000 in partition order, and the built-in file byte for byte the
PROM 2000 of QUUX's release, both its `release-2000-promh.mcr` and the
`sys/ubin/promh.mcr` of its sources, whose symbols and error table say
version 2000 (`the_built_in_quux_prom_is_the_release_s`, where the release
is present); `tests/system_2000.rs` boots System 2000 on it.
`--prom` on `quux` takes a file in partition order assembled at 36000, and
refuses one in MIT's order or assembled at 0.

**For revision 11 it maps the register page at physical page 37777**
(contract Q13): virtual page 2, which it uses for word 102 and for
block-disk, names physical page 37777, the page at `17777400`; block-disk's
registers are at virtual 1200, the page's words 200-203; and virtual page
1 is not mapped. It differs from revision 10's PROM in five of its 1024
words, two of them no-ops in the places of the two map writes it does not
make, so every address in it is revision 10's PROM's. Run to
`DISK-AWAIT-PACK` on both engines, the built-in PROM has written those
three (`quux_s_prom_maps_the_register_page_at_37777` in
`tests/quux_prom.rs`).

QUUX runs only muir-sys's latest band, System 2000. The PROMs assembled at
0, and System 1001 on QUUX, are retired with it.

**Since revision 10 it resets the devices and gives timer 0 its period**
(contract Q11). It pulses no `PROG.UNIBUS.RESET`, which resets nothing on
QUUX. After it maps the register page and writes error stop, and before
its first disk command, it writes word 104 with 1, reset devices, so that
a reboot starts the microcode with no timer on, the file device disabled
and block-disk idle; and then word 111 with 16,667, timer 0's period, since
no timer resets to a period. It turns no timer on. From power-on on `micro` the two writes come at microcycles 628,973 and
628,981, and at location 6 timer 0 is off at period 16,667 with its
interrupt enable 0 and timers 1 and 2 in their reset state
(`m9_the_prom_resets_the_devices_and_writes_timer_0_s_period`). On it
System 2000 reaches its listener with word 110 reading 401, timer 0 on,
periodic, under its interrupt enable, and word 111 16,667, and runs
`INTR-TICK` 600 times in 10 s after it; logged in, it writes `(3 2000
"QUUX" "Experimental System 2000, microcode 2000")`, its microcode's
version, the machine and the herald's line, and a `(time)` that moved 64
sixtieths over a sleep of 60; a mouse move of -40, -30 then changes the
band's `A-MOUSE-X` and `A-MOUSE-Y` by -40 and -27 within 100 ms, recorded
and not held (`m10_the_band_ticks_and_says_it_is_system_2000`). A reboot, a jump
to 36000 without `-RESET`, with timers 1 and 2 turned on and up under
their interrupt enables while the machine is halted (the band turns off a
timer 1 or 2 that interrupts, `INTR-TIMER-1-STRAY`) and the file device
enabled with three READs of 64 KiB and a CREATE-DIRECTORY queued, reaches
the listener after 153,696,634 microcycles with `INTR` run 1,873 times,
against 154,225,602 and 1,872 for the same reboot with neither, with the
timers off and the device disabled at location 6 and no queued command run
(`m11_a_reboot_resets_the_timers_and_the_file_device`). All on `micro`, in
`tests/system_2000_timers.rs`.

On revision 10, with revision 10's System 2000, the PROM before Q11,
dev11's, which writes neither word, failed that criterion: the same reboot
reached location 6 with timers 1 and 2 still on and the device still
enabled, and no queued command ran. From power-on on that PROM the band
reached its listener with word 110 reading 401 and word 111 16,667, the
period its microcode writes itself at `RESET-MACHINE`
(`uc-cold-disk.lisp`), and `INTR-TICK` ran 600 times in 10 s. That PROM
maps revision 10's register page and cannot run on revision 11, so these
two measurements are this record alone, with the tests that made them,
`m11_fails_on_the_prom_before_q11` and
`m12_the_prom_before_q11_on_revision_10`, in the repository's history.

## The register page

**QUUX's device registers are on one page of 256 words**, the register
page, physical `17777400`-`17777777`, the last page of the physical space
(revision 11, contracts Q2 and Q13). It stays there if the physical space
grows, and the frame buffer may grow up to below it. The microcode reaches
word *w* at virtual `77777400` + *w*, and Lisp's `%xbus-read` at the offset
`777400` + *w*. Each word answers in two microcycles ([the memory
port](#the-memory-port-and-the-device-registers)).

| Words | Device | Words and bits |
|---|---|---|
| 000-077 | the feature page | read only ([above](#the-feature-page)); 17-77 read 0 |
| 100-107 | the page's own words | 100: interrupt status, read only (below). 101: error status: `<0>` NXM is the only bit QUUX sets (`<3>` and `<5>`, the CADR's Unibus bits, read 0); a write of any value clears it. 102: mode, `<0>` error stop, read and written, which the host can set too. 103: the real-time clock, read only (below). 104: `RESET-DEVICES`, written, reads 0 (below). 105-107 reserved |
| 110-117 | the interval timers | 110-115 ([the clocks](#the-clocks)); 116-117 reserved |
| 120-137 | the keyboard and the mouse | 120-123 (below); 121 gives the key word in `<23:0>`, and `<31:24>` read 0. 124-137 reserved |
| 140-157 | the network, the Chaosnet interface | 140: the CSR, read and written. 141: my address when read, the write buffer when written. 142: the read buffer, read only; a read advances it. 143: the bit count, read only. 145: START, read only; a read starts a transmission. Writes of 142, 143 and 145 are ignored. 144, 146, 147 and 150-157 reserved |
| 160-177 | the file device | 160-171 (below); 172-177 reserved |
| 200-207 | block-disk | 200: status when read, command when written. 201: the last memory address when read, the command list pointer when written. 202: the disk address, read and written. 203: START, written; reads 0. The bits are [block-disk's](#block-disk), the done interrupt's enable command `<11>`. 204-207 reserved |
| 210-217 | the video controller | 210: mode, `<2>` black-on-white reads back, the other bits read 0 and a write of them is dropped. 211-217 reserved |
| 220-377 | none | reserved |

**Reserved** means a read gives 0 and a write changes nothing. Everything
else from `17000000` up below the page and past the frame buffer is
**nothing there**: an access fails at once, reads 0 and sets word 101 `<0>`.
That includes `17377000`-`17377777`, where the CADR's display and disk
registers are, and the rest of the old Unibus window
([No Unibus](#no-unibus)). On the CADR the page is inside the Unibus
window, Unibus `777000`-`777776`, where nothing answers: an access sets the
Unibus NXM bit, `766044` `<3>`.

**Word 100** says who interrupted, a bit each:

| Bit | Source | Up while | Cleared at the source by |
|---|---|---|---|
| `<0>` | timer 0, the tick | its flag and 110 `<8>` | a write with `<1>` set, turning the timer off, `<8>` off, a period write, `RESET-DEVICES` |
| `<1>` | timer 1 | its flag and 112 `<8>` | as timer 0 |
| `<2>` | timer 2 | its flag and 114 `<8>` | as timer 0 |
| `<3>` | block-disk | not active and command `<11>` | a command write with `<11>` clear, START, `RESET-DEVICES` |
| `<4>` | the keyboard | a key word waiting and 120 `<8>` | reading 121 until the FIFO is empty; `<8>` off |
| `<5>` | the mouse | 123 `<0>` and 123 `<8>` | reading 122; `<8>` off |
| `<6>` | the network | the Chaosnet interface's request, as the CADR's CSR gives it | as on the CADR, through 140-145; `RESET-DEVICES` |
| `<7>` | the file device | 170 ≠ 171 and 160 `<8>` | writing 171 up to 170; `<8>` off; a disable; `RESET-DEVICES` |
| `<31:8>` | reserved | never | --- |

Each bit is a level, and their OR is the interrupt pending that jump
conditions 5 and 6 test. So one write turns any of them off: 112 or 114
written 0, 200 written 0 with the disk idle, 123 written 0, or 160 written
back with `<8>` clear and `<0>` kept, which leaves the file device enabled.
That is what a handler does for a bit it does not serve.

**The page's convention** is for devices made for it: the enable in `<0>`
and the interrupt enable in `<8>` of the control word. Devices moved onto it
keep their own bits: block-disk the CADR disk controller's programming
interface, the network the Chaosnet interface's. 160 `<8>` is taken only by
a write that enables the file device or leaves it enabled.

**How the flags clear**: a write of any value clears 101, 120 `<1>`, and 161
`<2>` and `<3>` (the last two through any write of 160); a write with the bit
set clears 110, 112 and 114 `<1>`; a read clears or consumes --- 121 pops,
122 clears 123 `<0>`, 142 advances, 145 starts a transmission.

`tests/quux_registers.rs` holds every word of the page in one table, on the
machine and through both engines' map: its class and its value at power-on;
a write of all ones to every reserved or read-only word leaving the
machine's state byte for byte as it was; the words a read or a write of has
an effect listed apart, each with the test that holds it; word 100's eight
sources, each alone reading its own bit, and each turned off by one write;
and every address of `17377000`-`17377777`, `17400000` and `17777377` failing
at once on both engines.

**`RESET-DEVICES`** (revision 10, contract Q11): a write of word 104 with
`<0>` set resets every device, at the instant the write is taken. What the
file device had due by then runs first, as for a write of its word 160; then

| Device | What `RESET-DEVICES` does |
|---|---|
| The interval timers | every timer off, flag down, periodic, interrupt enable 0, period 0 |
| The file device | disabled, status `<2>` and `<3>` cleared (below) |
| Block-disk | command 0, its done interrupt's enable with it, and its errors cleared; not active at once, a transfer in flight ending there |
| The network | the Chaosnet interface reset, its CSR's writable bits cleared |
| The video controller | nothing to show: it has no vertical flag and no interrupt |
| The keyboard and mouse | nothing: the FIFO, the counts and both interrupt enables are kept, since a warm boot's key word is read by the microcode's location 6 after the PROM, which writes word 104, has run |
| The real-time clock, the microsecond clock, words 101 and 102 | nothing |

Block-disk, the network, the video controller and the file device are reset
as `PROG.UNIBUS.RESET` resets the CADR's boards (`Machine::bus_reset`).
**On QUUX `INTERRUPT-CONTROL<28>`, `PROG.UNIBUS.RESET`, drives nothing**:
the bit is written and read back through `LOCATION-COUNTER` as on the CADR,
and resets no device; the CADR's still resets its boards. Nothing is held
off `SINTR` at a write of word 104: its effect is in the `SINTR` of the edge
after the one that takes it, as any register write's is. Anything may write
the word; the boot PROM writes it before it reads the disk.

`tests/quux_reset_devices.rs` holds word 104 reading 0, a write with `<0>`
clear changing nothing in the machine's state, a write of 1 leaving every
device as `PROG.UNIBUS.RESET` does from the same state and every timer
reset with the keyboard and mouse kept, what the file device had due
running first, a destination 3 write at the same edge or the next turning
no timer on, `SINTR` at the write's edge and at the next, and `<28>`
resetting nothing on QUUX on both engines.

## The real-time clock

**QUUX keeps the real time in word 103 of the register page** (contract Q9,
revision 9): whole seconds since 1970-01-01 00:00 UTC, Unix time, as an
unsigned 32-bit number, which lasts to 2106. There are no fractions: the
microsecond clock, functional source 15, counts finer time from power-on.
Lisp's universal time counts from 1900, so it is the word plus 2,208,988,800,
a sum a 32-bit count from 1900 could not hold past 2036.

| | |
|---|---|
| Read | the seconds, `<31:0>` |
| Written | nothing: a write goes nowhere and the word reads on unchanged, as a write of any read-only word on the page does. The host keeps the time; the machine never sets it, and the time zone is not the clock's |
| The CADR | nothing answers on the page: a read times out and sets the NXM bit |

**It is live**: it gives the host's time, kept current by the host, as a
real clock keeps real time on its own crystal whatever the processor does.
muir reads the host's clock (`SystemTime`) at every read of the word, so it
never drifts from the host, however fast or slow the engine runs.

**`--rtc <s>` fixes it for runs that repeat**: the clock reads second `s`
at power-on and counts the machine's own time from there, a second for each
10^9 ns of the engine's clock, whatever the host's clock does. It holds at
2^32-1 and never wraps to 0, which would read as no clock at all; a start
past 2^32-1 is refused. `--rtc host` is the default. The start says which:
`rtc: the host's clock` or `rtc: from <s>, counting machine time`. A
checkpoint carries the setting, the start and the machine time it counts
from, so a resumed run reads the second the run that wrote it would have;
a resume under another `--rtc` is refused by the flag's name.

`tests/quux_rtc.rs` holds the live word against the host's clock, the start
and the count, the hold at 2^32-1, a write changing nothing, the CADR's
timeout, feature word 15, a checkpoint, and both engines reading a second
go by in their own time; `the_rtc_is_quux_s` in `tests/cli.rs` the flag,
its refusals and the start's report; `a_resume_has_the_checkpoint_s_rtc` in
`tests/muir_checkpoint.rs` the resume.

## The file device

**QUUX reads and writes files on its host through a file device** (contract
Q9, revision 9): folders of the host served to the machine under one
pathname host, `HOST`, with commands and responses in two rings in main
memory and the bytes moved by DMA. The registers carry control and the
rings' indexes; nothing polls memory for an index. `src/file_device.rs` is
muir's device.

| Word | | |
|---|---|---|
| 160 | control, read and written | `<0>` enable, `<8>` interrupt enable |
| 161 | status, read only | `<0>` enabled, `<1>` quiet, `<2>` configuration refused, `<3>` index fault, `<8>` a response waiting, `<23:16>` handles open |
| 162 | command ring base | `<23:0>` a physical word address, `<1:0>` 0 |
| 163 | command ring size | `<3:0>` the log2 of its entries, 0 to 8 |
| 164 | command producer, the processor's | `<15:0>` |
| 165 | command consumer, the device's, read only | `<15:0>` |
| 166 | response ring base | as 162 |
| 167 | response ring size | as 163 |
| 170 | response producer, the device's, read only | `<15:0>` |
| 171 | response consumer, the processor's | `<15:0>` |

**Configuration.** 162, 163, 166 and 167 are written while the device is
disabled and ignored while it is enabled. The enable, 160 `<0>` from 0 to 1,
checks them: a base off a 4-word line, a size over 8, or a ring reaching
past main memory is refused, status `<2>`, and the device stays disabled.
While disabled the four indexes read 0 and writes of 164 and 171 go nowhere;
the enable starts them at 0. A write of 160 clears `<2>` and `<3>`.

**Indexes.** Each counts entries, 16 bits, free-running; an entry's slot is
the index mod the ring's size. The processor writes a command into slot
`164 mod size` and then 164 with one more (or n more). A write of 164
claiming more commands than the ring holds, or fewer than are waiting, and a
write of 171 past 170, are ignored and set status `<3>`. The device answers
each command with one response, in command order, so response i answers
command i, and 165 and 170 always read the same. It takes the next command
only while the response ring has room, 170 - 171 below its size.

**Entries** are 8 words, command and response alike.

| Word | Command | Response |
|---|---|---|
| 0 | `<15:0>` tag, `<23:16>` opcode, `<31:24>` flags | `<15:0>` tag, `<23:16>` status, `<31:24>` opcode |
| 1 | handle (READ, WRITE, CLOSE) | count: bytes written to B (READ, DIRECTORY, COMPLETE), or taken from A (WRITE) |
| 2 | buffer A's address, `<23:0>` | handle (OPEN read or write) |
| 3 | buffer A's length in bytes | the file's length in bytes (OPEN, CLOSE) |
| 4 | buffer B's address | mtime, Unix seconds (OPEN, CLOSE) |
| 5 | buffer B's length | flags: `<0>` a directory, `<1>` on a read-only mount, `<2>` COMPLETE: an entry is exactly the completion, `<3>` and it is a directory |
| 6 | READ, WRITE: the offset in the file; DIRECTORY: the cookie | DIRECTORY: the next cookie, 0 at the end; COMPLETE: the matches |
| 7 | CLOSE: the date to set, Unix seconds | 0 |

A failed command's response is word 0 alone. A buffer starts on a 4-word
line, holds at most 65,536 bytes, and lies in main memory; byte k is bits
`8(k mod 4)+7:8(k mod 4)` of word k/4. A READ of n bytes writes the first
`ceil(n/4)` words of B, the bytes past n 0, and no other word.

**Commands.**

| Op | | In | Out |
|---|---|---|---|
| 1 | OPEN | A the name; flags `<1:0>` 0 read, 1 write, 2 probe; `<3:2>` if it exists (write): 0 supersede, 1 error, 2 append; `<4>` if it does not (write): 0 create, 1 error | handle (none for a probe), length, mtime, flags `<0>` `<1>` |
| 2 | READ | handle; B where, its length the bytes wanted; offset | count, the wanted or what is left |
| 3 | WRITE | handle; A the data; offset | count |
| 4 | CLOSE | handle; flags `<0>` abort, `<1>` set the date from word 7 | length and mtime as closed; 0 after an abort |
| 5 | DIRECTORY | A a directory, `/` the root; B at least 272 bytes; cookie, 0 to start | count, next cookie |
| 6 | COMPLETE | A `<directory>/<prefix>`; B | count (the completion in B), matches, flags `<2>` `<3>` |
| 7 | DELETE | A a file or an empty directory | |
| 8 | RENAME | A the old name, B the new | |
| 9 | CREATE-DIRECTORY | A, one level | |
| 10 | LOG | A a line of at most 1,024 bytes | |

- **The device moves bytes and never interprets them.** There is no
  character mode or byte size.
- **OPEN read** keeps the host file open, so a rename or delete on the host
  does not disturb the handle. There are 64 handles, numbered 1 to 64.
- **OPEN write** writes a temporary file in the target's folder, named
  `.quux-write-` and more, which DIRECTORY and COMPLETE never show. CLOSE
  renames it onto the name, so the file appears or changes whole; until
  then the name is as it was. Supersede starts it empty, which is also what
  the Lisp side sends for `:OVERWRITE` and `:TRUNCATE`: "starting at the
  beginning, and set the file's length to the length of the newly written
  data" (MIT's `sys/man/files.text`, lines 282-288). Append starts it as a
  copy of the file, and the reply's length is the copy's. Error answers FAE
  at OPEN for a name that exists, and at CLOSE for one that appeared
  meanwhile, discarding the write. A file's permissions carry over.
- **READ and WRITE are positional**: a READ past the end is short, 0 at the
  end, FOR past it; a WRITE leaving a hole, or ending past 2^32 - 1, is FOR.
- **CLOSE** with `<1>` sets the file's modification time before the rename;
  its reply gives the length and mtime the host then has, which the Lisp
  side takes as the creation date (a host may keep times to 2 s). With
  `<0>` the temporary file is removed and the name left as it was.
- **DIRECTORY** gives records, packed from B's start, whole words each:
  word 0 `<7:0>` the name's length, `<15:8>` the record's words (3 and the
  name's), `<16>` a directory, `<17>` on a read-only mount, `<18>` 2^32 bytes
  or more; word 1 the length (0 for a directory, FFFFFFFF too large); word 2
  the mtime; then the name. They are sorted bytewise; dot files are listed;
  `.` and `..`, the temporary files, a symlink that leaves its mount, a name
  the rules below refuse, and anything neither file nor directory are not.
  The cookie is the index of the next entry, and each call lists afresh.
  A missing directory is DNF and a file WKF.
- **COMPLETE** matches the entries DIRECTORY would list whose names begin
  with the prefix, case kept; B gets their longest common beginning, word 6
  their number, and `<2>` (with `<3>` for a directory) says one is exactly
  that. No match is count 0 and matches 0.
- **RENAME never overwrites** (REF), atomically on Linux
  (`renameat2`'s `RENAME_NOREPLACE`), and never across mounts (RAD).
- **CREATE-DIRECTORY** makes one level: a missing parent is DNF.
- **LOG** writes `log: ` and the line on muir's standard error, a byte
  outside 040-176 as a backslash and three octal digits.

**Names** are absolute paths of bytes, at most 1,024, a single trailing `/`
allowed; each component 1 to 255 bytes in 040-176 other than `/`, and not
`.` or `..`. Anything else is IPS, and so is a name the host's file system
refuses (EINVAL). Case is exact: a name that matches only with case ignored
is not found. muir checks the spelling against the folder when the host
finds a name's case-flipped twin as the same file, which a case-folding file
system does; **unverified** on one, muir's tests running on Linux's. A
symlink is followed when it resolves inside its mount's folder; one that
leaves it, or loops, is ACC.

**Statuses.**

| Code | | When |
|---|---|---|
| 0 | | done |
| 1 | FNF | the last component does not exist |
| 2 | DNF | a directory on the way does not exist or is a file; an unmounted name; DIRECTORY of a missing directory |
| 3 | FAE | OPEN write with if-exists error; CREATE-DIRECTORY on a file |
| 4 | REF | RENAME onto an existing name |
| 5 | ACC | the host refuses (EACCES, EPERM); a symlink leaving its mount, or a loop; a mount's own root deleted or renamed |
| 6 | ATF | a write under a read-only mount (and EROFS) |
| 7 | DAE | CREATE-DIRECTORY of an existing directory |
| 8 | DNE | DELETE of a directory not empty |
| 9 | NMR | the host is full (ENOSPC, EDQUOT) |
| 10 | IOD | OPEN read or write of a directory |
| 11 | WKF | neither file nor directory; a file of 2^32 bytes or more; DIRECTORY of a file |
| 12 | IPS | a name against the rules, or EINVAL |
| 13 | NER | all 64 handles open |
| 14 | UOP | an opcode not among the ten |
| 15 | DAT | the host's I/O error, and any error not named here |
| 16 | FOR | READ past the end; WRITE leaving a hole or past 2^32 - 1 |
| 17 | RAD | RENAME across mounts |
| 64 | | bad handle: not open, or the wrong kind |
| 65 | | bad buffer: off a line, over 65,536 bytes, or past main memory |
| 66 | | bad argument: flags out of range, a DIRECTORY buffer under 272 bytes, a LOG line over 1,024 bytes, a completion longer than B |

**Mounts** (`--file-root`). `--file-root <folder>[,ro]` is HOST's `/`, a
folder holding `sys/`, `site/` and `home/<user>/`, so that one path serves
`HOST:/sys/...`, `HOST:/site/...` and `HOST:/home/lispm/...`. `--file-root
<name>=<folder>[,ro]`, once for each name, is the top-level directory
`<name>`, over the default folder's entry of that name. A value is a named
mount when the text before its first `=` is a valid component. DIRECTORY of
`/` lists the top-level directories, each with its mount's read-only bit,
and every OPEN's reply carries it. A top-level name that is neither mounted
nor in the default folder is FNF itself and DNF below. With no default
folder `/` holds the mounts alone and is read-only (a name created there is
ATF); with no `--file-root` at all it is empty. A read-only mount answers
every OPEN write, DELETE, RENAME and CREATE-DIRECTORY with ATF, and a mount's
own root cannot be deleted or renamed (ACC). The start lists them. The CADR
refuses the flag.

**Disable is the reset.** 160 `<0>` from 1 to 0 drops the commands not yet
run, with no response and no memory written; closes every handle, a write
discarded and its temporary file removed; and sets the indexes and the
interrupt enable to 0. Quiet, 161 `<1>`, is up at once: muir's device is
never in the middle of a copy, and a command it ran stands, host effect and
all. The bases and sizes stay, and may be written before the next enable.
**Reset devices disables it too** (word 104, revision 10), clearing status
`<2>` and `<3>` with it; `PROG.UNIBUS.RESET` reaches it no more. Power-on
is disabled.

**Word 100 `<7>`** is up while the interrupt enable is on and a response is
waiting, 170 ≠ 171: a level, cleared by writing 171 up to 170 or by the
enable going off. Status `<8>` is the same ungated.

**Time.** A command completes at its due time: 20 us, and 100 us a KiB of
the two buffer lengths its entry names (a length over 65,536 counted as
65,536), rounded up to the ns, after the latest of its producer write, the
previous command's completion, and a response slot coming free. Both
constants are **unverified**: 20 us an estimate of the boards' round trip,
and 100 us a KiB block-disk's time for a block, itself an estimate, until
muir-fpga measures its path. At the due time, at the edge before the
processor's next microcycle and on both engines, the device reads the entry
and buffer A (and B for RENAME), does the host's operation, writes buffer B
and the response, and moves 165 and 170; main memory having been written
behind the processor, the whole memory cache is invalidated before the
processor's next memory cycle, as after a disk transfer. Nothing completes within the producer write,
and nothing in memory changes before 170 moves. Given the same host files a
run is the same.

**Coherence.** The device takes a new 164 only once the processor's write
buffer is empty: on `rtl` the command's time counts from when main memory
has done the last write the buffer took. `micro` has no write buffer.

**A checkpoint is refused** while a handle is open or a command is queued,
saying which: a host file and a command's host effect are outside the
machine. Otherwise it carries the registers and the rings' indexes; the
mounts are the flags'.

`tests/quux_file_device.rs` holds it: a scripted driver writing commands into
the rings against a scratch folder --- each register, the refused enable and
the index fault, the due time to the nanosecond and nothing before it, the
order, the wrap at 1, 2 and 256 entries and across 2^16, a full response
ring, the interrupt, the disable and reset devices, `<28>` leaving it as it
was, every command and
its statuses, the write landing whole with its temporary file never listed,
the date, the mounts, a read-only mount unchanged, names, symlinks, LOG, the
checkpoint's refusal and its round trip --- and both engines running a
program that writes the command and its producer index and reads the
answer, `rtl`'s cache invalidated and its write buffer waited for.
`the_file_root_is_quux_s` in `tests/cli.rs` holds the flag. **Not
produced by a test:** NMR, DAT, and a WRITE past 2^32 - 1.

## The keyboard and the mouse

**QUUX's keyboard and mouse are on the register page** (contract Q3),
without the CADR's keyboard timing or the mouse's quadrature lines: the
CADR's I/O board takes its keyboard's words off a serial line at the
keyboard's clock and counts its mouse's lines on `KB CLK^`.

| Word | |
|---|---|
| 120 | keyboard status: `<0>` a key word is waiting, `<1>` the FIFO overflowed (a write clears it), `<8>` the interrupt enable |
| 121 | a read takes the oldest key word, the word `764100`/`764102` give together on the CADR, in `<23:0>`, `<31:24>` reading 0; 0 when none is waiting |
| 122 | the mouse: `<11:0>` the X count, `<27:16>` the Y count, twelve bits each and wrapping as the CADR's counters do; `<14:12>` the buttons, as the CADR's Y register has them. A read clears 123's `<0>` |
| 123 | mouse status: `<0>` moved or a button changed since 122 was read, `<8>` the interrupt enable |

The FIFO holds 64 key words, the size of the microcode's own keyboard
buffer; a word past that is dropped and `<1>` set. A host adds its motion to
the counts, and `TRACK-MOUSE`, which takes the difference from last time and
sign-extends from bit 11, needs no change but where it reads. There is no
beeper: the CADR's `764110` is a toggle on the speaker line the microcode
flips once a half-wavelength, and QUUX leaves it out. muir's terminal
delivers to it on QUUX as it delivers to the I/O board on the CADR, and the
keyboard's boot word still boots. `tests/quux_input.rs` holds the FIFO's
order and its 64, the overflow, the counts and buttons, the interrupts, the
terminal's delivery, the boot word, a checkpoint and both engines' reads.

## No Unibus

**QUUX has no Unibus** (contract Q5). Every address of the CADR's Unibus
window, physical page 37000 and up, answers nothing on QUUX but its last
page, `17777400`-`17777777`, which is [the register
page](#the-register-page): a read or a write fails as any empty address does,
at once since Q7, the NXM bit set in word 101, and changes nothing. With it go, on QUUX, the I/O board (its keyboard,
mouse, clocks and Chaosnet interface now QUUX's own, contracts Q1-Q4; its
serial port and general-purpose register dropped), the bus interface's
Unibus side (the adapter, the Unibus map, its buffers, WRITE-THROUGH, the
interrupt control and error status at `766040`-`766044`, the Unibus
interrupt), the diagnostic registers as the machine's own (the spy stays
the host's port: muir's prompt, the FPGA's AXI face), and the debug cable,
a Unibus master: `--debug-cable-listen`, `--debug-cable-connect` and
`--debug-in-process` are `cadr`'s alone, and a `quux` run has no DBGIN
connector. A QUUX debug design of its own is a later contract. The CADR
keeps all of it: CC and the two-machine lashup are its acceptance test.

muir-sys's microcode for Q5 makes no Unibus access over a boot and a while
at the listener, counted on `micro`. `tests/quux_no_unibus.rs` holds the
window's failures on the machine and through both engines' bus, its last
page answering as the register page, the Unibus interrupt not reaching
QUUX, and the CADR's Unibus unchanged;
`quux_has_no_debug_cable` in `tests/cli.rs` the cable.

## The network

**QUUX's Chaosnet interface is on the register page** (contract Q4), off the
I/O board: its registers keep their order and their bits, word 140 + k being
the CADR's Unibus `764140` + 2k --- 140 the CSR, 141 my address (read) and
the write buffer (written), 142 the read buffer, 143 the bit count, 145
START --- sixteen bits each in the bottom of the word, and its interrupt is
word 100's `<6>`. Only these five are decoded: 144, 146 and 147 are
reserved, where the CADR's board answers the CSR at `764150` and the bit
count at `764156`, and writes of 142, 143 and 145 are ignored, where the
board takes one of `764152` as the write buffer's. muir's cable is the same:
CHUDP to `ozd`, `--chaos-address`, `--chaos-udp-peer`. An Ethernet interface
behind the same device is a later contract. `tests/quux_network.rs` holds
each decoded word to its Unibus register, the reserved words and the ignored
writes, and a STATUS request sent and answered through the page.

## The fused return

**QUUX can run the next macroinstruction's handler straight from a
return to the main loop** (revision 12, contract H8a). Microcode 2000's
main loop, `QMLP` in `uc-macrocode.lisp:9-13`, is four microinstructions:
a call on condition 6, `M-INST-BUFFER <- MD`, `(DISPATCH-XCT-NEXT M-INST-OP
OPDTB)`, and the push of `A-MAIN-DISPATCH` back as that dispatch's
XCT-NEXT. A return that pops `A-MAIN-DISPATCH` --- `<14>` and `QMLP` ---
when no instruction fetch is needed goes to `QMLP+2`, the stream
hardware's `SPCMUNG` stepping it over the first two (`uc-macrocode.lisp:6-7`):
the dispatch and the push are two microcycles every such macroinstruction
spends there.

Functional destinations 5 to 7, which the CADR's low group leaves without
a decoder output, are QUUX's from revision 12; below it, and on the CADR,
they write only M. Like every functional destination they write M as
well:

| Destination | Writes |
|---|---|
| 5 | the MACRO-DISPATCH register: `<13:0>` the main loop's address, `<23:14>` the A-memory address of `A-LOCALP` and `<28:24>` the M-memory address of `M-AP`, the operand address's bases, `<31>` the enable; `<30:29>` reserved, written 0 and kept 0 |
| 6 | the MACRO DISPATCH MEMORY's index, `<9:0>` |
| 7 | the entry at the index, `<17:0>`: D-MEM's word, `<13:0>` the handler's address, `<14>` N, `<15>` P, `<16>` R, and `<17>` the operand bit |

The MACRO DISPATCH MEMORY has 1,024 entries, feature word 17, indexed by
the halfword's `<15:6>`: its destination, opcode and register together.

**A fused return** is a return --- a POPJ, a dispatch whose entry has R,
or a jump with R (one switch in the code, `JUMP_RETURNS_FUSE` in
`src/machine.rs`, takes jumps out on both engines) --- that pops a word with `<14>` set and `<13:0>` the
register's, with the register enabled and no fetch needed, so that today
it would go to `QMLP+2`. When the entry for the next halfword has R and P
clear, it goes to the entry's address instead, two microcycles sooner; the
popped word stays on the stack, as the push would have put it back, unless
the entry's N is set, which would have nopped the push. The next halfword
is the one the main loop's dispatch would take: M 31 rotated as
`IR<11:10>` = 3 rotates it, by the location counter as the pop's `NEXT
INSTR` steps it in the microcycle after. The step, and a fetch it starts,
are the stream's as ever. Condition 6 is not tested, as the main loop does
not test it on this path.

It is today's return, `QMLP` or `QMLP+2`, whenever a fetch is needed and
the prefetch (below) does not hold the word, the entry has R or P, the word is another main loop's (`DMLP`'s), the
register is disabled, or the popping microinstruction pushes, pops by
functional source 14, writes M 31 or INTERRUPT-CONTROL, or steps the
location counter itself (a `NEXT INSTR` the microcycle before, or a
dispatch's `IR<24>`).

**The prefetch** (`src/memory_port.rs`, contract H8a §3.5). A return that
needs a fetch fuses too when the next word is already in hand. When a
macroinstruction fetch's read is answered from main memory at physical
word *p*, the memory port takes the word at *p* + 1 into a one-word
buffer, with its virtual and physical addresses, if it is in the line the
fetch has just read or filled: the cache puts that line's four words out
of its RAMs together, so taking one needs no memory cycle, no map lookup,
no arbitration and no second read of the cache, and it cannot fault. A
word in the next line is not taken, nor one in the next page. A return
that needs a fetch fuses on the buffered word when that is the next word
in sequence (`LC<25:2>`), condition 6 is false, and no store started in
the microcycle before; with condition 6 true it runs `QMLP` as today,
since the main loop tests condition 6 on this path. Such a return saves
four microcycles, `QMLP` to `QMLP+3`, and the stream's fetch of the word
starts as ever. M 31 is a register beside M memory, which reads of A and
M address 31 take: it is loaded with the buffered word at the end of the
microcycle after the return, as the operand address is, so that
microcycle reads the old word, as on the path the return skips, and the
handler the new one. The buffer is dropped by a write of the location
counter, a store to its word, a transfer by block-disk or the file
device, a map write, and -RESET. A checkpoint keeps the buffered word, a
fetch the port is yet to answer, and M 31's word armed.

Only `rtl` has the prefetch: it looks in the cache, and `micro` has none.
On revision 12 `micro` fuses only the returns that need no fetch, and
`rtl` those and the ones its prefetch holds the word for, so the two
engines take different microcycles; they leave the same state wherever
the microcode keeps the rule below, and the tests that compare them count
each engine's fused returns.

Over the profile harness's twelve workloads (`examples/profile.rs` on
`rtl`, System 2000 with microcode 2000 writing the register and the
entries itself, `MUIR_H8A=microcode`, a 4K-word cache and the Arty
Z7-20's memory timing), the fused return with the prefetch takes 7.02%
fewer microcycles, and 7.58% less time, than without it. The prefetch
takes the next word at 73.5% of the fetches answered from main memory,
and a fused return uses it at 28.4% of the macroinstructions.
`MUIR_PREFETCH=page` fits it with a page's reach, which looks in the next
line too when the cache holds it and needs a second read port of the
cache: 9.51% fewer microcycles and 10.27% less time on the same
workloads.
`MUIR_PREFETCH=off` takes it out.

**The operand address.** When the entry has the operand bit and the
halfword's register, `<8:6>`, is LOCAL (5) or ARG (6) (`QADCM1`,
`uc-macrocode.lisp:129-137`), PDL-INDEX is loaded at the end of the
microcycle after the return with `A-LOCALP` + delta or `M-AP` + 1 +
delta, as `QADLOC1` and `QADARG1` compute them
(`uc-macrocode.lisp:237-245`): delta is the halfword's `<5:0>`, and the
sum is masked to PDL-INDEX's bits. The handler's first microinstruction
finds its operand at `C-PDL-BUFFER-INDEX`. The bases are copies the
machine keeps, fourteen bits each: an A write at the register's
`<23:14>` writes the copy of `A-LOCALP`, and an M write at its `<28:24>`
the copy of `M-AP`, with their write pulse, and nothing else writes them.
The register's write does not load them from A and M memory, so the
microcode writes `A-LOCALP` and `M-AP` after destination 5. -RESET leaves
the copies and drops an armed operand address; a checkpoint keeps both
copies and an armed operand address, and its restore reads nothing from
A or M memory.

**The rule for the microcycle after a fused return.** It runs as it would
have before the main loop's dispatch, which has been decided by then, so
it must not write the location counter, M 31, INTERRUPT-CONTROL (whose
byte mode chooses the halfword) or destinations 5 to 7; and, where the
entry has the operand bit, PDL-INDEX, `A-LOCALP`, `M-AP`, or the PDL
buffer by PDL-INDEX (destination 12). A PDL buffer write lands in the
next microcycle's write phase at PDL-INDEX as it stands then (`PWIDX`),
which is after the operand address has been loaded, so it would go to the
next instruction's operand; both engines do that. Nor may the handler's
first microinstruction read the PDL buffer at the address the microcycle
after the return writes, at the pointer (destinations 10 and 11) or by
PDL-INDEX: the buffer has no pass-around (above), so the read finds the
word before that write, where today's path has the main loop's dispatch
and push in between. The machine does not check these rules;
`tests/support/macro_dispatch.rs` does, over a run.

**Microcode 2000 fills the memory and keeps the rule itself.** At every
start of the microcode, a cold or warm boot through the PROM and a
`%DISK-RESTORE`, and before its first main-loop return, `RESET-MACHINE`
reads feature word 17 and, where it is not 0, writes every entry with
`OPDTB`'s entry for its opcode, the operand bit clear; then the register
with `QMLP`, `A-LOCALP`'s and `M-AP`'s addresses and the enable; then
`A-LOCALP` and `M-AP` with their own values, which loads the base copies;
and then, over the generic entries, 36 entries for handlers of its own
(`uc-cold-disk.lisp:65-171` in muir-sys's release 2000). Destinations 5
to 7 are written there and nowhere else. A specialised handler runs only
from a fused return, so only where the entry's conditions hold; otherwise
the main loop's dispatch runs the generic one, and the two give the same
results:

| Macroinstructions | Handlers (`uc-macrocode.lisp`) | Indexes | Operand bit | Run in the profile below |
|---|---|---|---|---:|
| MOVE to the PDL of a local or an argument | `QIMOVE-PDL-OPERAND` | 425, 426 | yes | 2,489,177 |
| POP and MOVEM into a local or an argument | `QIPOP-OPERAND`, `QIMVM-OPERAND` | 1735, 1736; 1535, 1536 | yes | 706,462; 107,758 |
| BR, BR-NIL and BR-NOT-NIL, by the offset's sign | `QIBRN-BR-POS` and its five fellows | 140-147, 340-347, 540-547 | no | 1,848,904 |
| SETE-1+, + and < of a local or an argument, on fixnums | `QISP1-OPERAND`, `QIADD-OPERAND`, `QILSP-OPERAND` | 1525, 1526; 315, 316; 525, 526 | yes | 399,531; 35,339; 24,509 |

Over the twelve workloads below, 5.61 M of the 15.95 M macroinstructions
ran one, 35.2%.

No `POPJ-AFTER-NEXT` of it stores in the PDL at PDL-INDEX, or writes
PDL-INDEX, `A-LOCALP`, M 31 or the location counter, in the
microinstruction after it: at 29 returns the write is made before the
return, among them `QSTLOC` and `QSTARG`, the stores into a local or an
argument (`uc-macrocode.lisp:355-377`), and at 24 of those a no-op
follows the return, one microcycle more. And no return that can pop the
main loop's word has a call or a jump in the microinstruction after it,
where the micro stack would move and the next handler would not run.
With every entry the generic handler and the operand bit on every
halfword whose `<8:0>` is a register and a delta, the band reaches its
listener too, the checkers finding nothing.

**What it saves.** Over the profile harness's twelve workloads on `rtl`
(four ticks, a 4K-word cache, the Arty Z7-20's memory timing, the
real-time clock counted from a fixed second, `cons` after an untimed
first `cons`), System 2000 on revision 12 against the same band on
revision 11. Time is the microcycles' 40 ns each and the time stalled on
the memory, which a microcycle count leaves out:

| Workload | Revision 11 microcycles | Revision 12 microcycles | Fewer | Revision 11 time | Revision 12 time | Less |
|---|---:|---:|---:|---:|---:|---:|
| compile | 22,557,000 | 20,806,000 | 7.8% | 977.6 ms | 902.1 ms | 7.7% |
| calls-ack | 45,054,000 | 39,104,000 | 13.2% | 1885.9 ms | 1625.4 ms | 13.8% |
| calls-fib | 36,898,000 | 33,648,000 | 8.8% | 1556.6 ms | 1410.6 ms | 9.4% |
| cons | 47,934,000 | 47,334,000 | 1.3% | 1958.9 ms | 1932.8 ms | 1.3% |
| arith-muldiv | 36,204,000 | 27,304,000 | 24.6% | 1571.9 ms | 1195.3 ms | 24.0% |
| float | 19,538,000 | 16,890,000 | 13.6% | 827.2 ms | 716.4 ms | 13.4% |
| array | 38,988,000 | 27,588,000 | 29.2% | 1647.9 ms | 1167.0 ms | 29.2% |
| sort | 87,984,000 | 77,084,000 | 12.4% | 3675.5 ms | 3203.6 ms | 12.8% |
| bignum | 15,844,000 | 13,994,000 | 11.7% | 670.8 ms | 590.9 ms | 11.9% |
| intern | 49,194,000 | 43,144,000 | 12.3% | 2098.3 ms | 1843.6 ms | 12.1% |
| print-scroll | 39,004,000 | 35,606,000 | 8.7% | 1667.8 ms | 1517.2 ms | 9.0% |
| compile-again | 13,538,000 | 12,088,000 | 10.7% | 585.6 ms | 522.4 ms | 10.8% |
| **all twelve** | 452,737,000 | 394,590,000 | 12.8% | 19124.1 ms | 16627.4 ms | 13.1% |

At the DE25-Nano's memory timing, all twelve take 12.7% fewer
microcycles and 12.4% less time. The memory's stalls are 5.3% of
revision 11's time at the Arty's timing and 9.0% at the DE25's.

The parts, each as a share of revision 11's microcycles and time at the
Arty's timing, measured by taking them out one at a time: the fused
return to the generic handlers alone (every entry `OPDTB`'s, filled by
the harness, `MUIR_H8A=generic`) without the prefetch, 3.00% and 2.75%;
the specialised handlers on top, without the prefetch, 3.26% and 3.17%;
and the prefetch on top of both, 6.58% and 7.13%. The prefetch and the
handlers gain from each other: the prefetch with the generic handlers
alone saves 3.90% and 4.75%, and the handlers with the prefetch 5.94% and
5.55%. The 24 no-ops the rule costs run 2,639,638 microcycles on revision
11, 0.58% of its microcycles, and 1,427,815 on revision 12, 0.36%;
`QSTLOC`'s is 64%
of those microcycles on revision 11, and it falls on revision 12 because the
specialised handlers make their stores in their own return. Two runs of
the same configuration give the same counts, microcycle for microcycle.

-RESET clears the enable and nothing else, and every control-store write
clears it too, wherever it lands: the entries name control-store
addresses, and a new microcode must not run on those another left, whether
the PROM loaded it or a `%DISK-RESTORE` (which does not pass through the
PROM). Reset devices, word 104, does not touch it. A checkpoint keeps the
register, the index, the entries and the base copies.

`tests/macro_dispatch.rs` holds, on both engines, a main loop made as
`QMLP` is: the destinations' decode, on revision 12 and not on 11 or the
CADR; the same state with the MACRO DISPATCH MEMORY holding the generic
handlers as without it, two microcycles fewer for each fused return that
needs no fetch and, on `rtl`, four fewer for each on the fetch path,
returns by a POPJ, a dispatch with R and, as the switch says, a jump with
R fused and N honoured; the entry taken by the whole `<15:6>`; each case
above that is not fused running today's path microcycle for microcycle;
the operand address for LOCAL and ARG at the handler's first
microinstruction and not before, masked, from bases written by the
popping microinstruction itself, and nothing loaded without the operand
bit or for another register; the base copies not loaded by destination 5
and written by the writes of `A-LOCALP` and `M-AP` alone; a PDL buffer
write by PDL-INDEX in the microcycle after landing at the operand
address; the rule checker's decode, each forbidden write found in a run,
and a handler's first PDL read at the address the microcycle after the
return writes found; the generic fill's operand bit only on halfwords
whose `<8:0>` is a register and a delta (of ND4, `PUSH-CDR-IF-CAR-EQUAL`
and `PUSH-CDR-STORE-CAR-IF-CONS`, and not `PUSH-NUMBER`, whose `<8:0>` is
an immediate); the enable cleared by -RESET and by a control-store write,
the base copies kept; and the checkpoint. On `rtl` it holds the prefetch:
revision 12 fitting it with the line's reach and revision 11 and the CADR
not at all; a return on the fetch path fused on the buffered word, with
the same state and no memory cycle of its own, and M 31 main memory's
word after it; the microcycle after reading the old M 31; condition 6
refusing the word; a store, a map write, a transfer and a write of the
location counter dropping it, and a word that should have been dropped
found by the checkers; a page's reach taking the next line's word and
never the next page's; -RESET dropping it; and `rtl` saved at every
microcycle of a run and loaded into another running on to the same end.
`tests/system_2000.rs` holds microcode 2000 as it was before it wrote the
register reaching its listener on revision 12 in the same microcycles and
nanoseconds, with the same memories, as on revision 11, on both engines,
and, booted with the register enabled and every entry poisoned to `ILLOP`
before the PROM loads it, nothing fused. It holds System 2000 on its
microcode booting on both engines with the register written as above and
36 specialised entries, returns fused, operand addresses loaded, the
rules kept after every fused return, the main loop's handler run next,
the operand address right, the base copies equal to their memory after
every microcycle and, on `rtl`, M 31 main memory's word after every
return fused on the prefetched word; the same with the generic fill and
the operand bit; the same band booted with the register enabled and
every entry poisoned, every entry written again and returns fused; and,
after a `%DISK-RESTORE`, the register enabled again and returns fused
again. `tests/unused_codes.rs` holds that it writes destinations 5 to 7
only between `RESET-MACHINE-MACRO-DISPATCH-FILL` and
`RESET-MACHINE-MACRO-DISPATCH-DONE`.

## Its microcode

**Microcode 2000 is QUUX's**, muir-sys's change to MIT's 323. At boot, at
`RESET-MACHINE`, it reads functional source 16, and without QUUX's
signature and a revision of 11 or more it halts at `MACHINE-NOT-QUUX-11`
(`uc-cold-disk.lisp`): a CADR reads all ones there. On QUUX the type is
the word's bits 3:0, 4; it reads the six-bit entry at `MAP(MD)<29:24>`,
writes it in two deposits, `VMA<31:27>` and `VMA<24>`, and keeps block 77
as the invalid one. It keeps the reverse first-level map in system
communication area words 640 to 737 and the swap-out CCWs at 440 to 457
(`uc-parameters.lisp`). It checks at boot that block 0's level-1 entry
reads back as the invalid entry the MACHINE-ID promised, and stops at
`MAP-WIDTH-MISMATCH` if it does not. `A-VERSION`, A memory's word 40, is
2000 (`band_2000_is_system_2000_on_microcode_2000` and
`system_2000_runs_on_the_video_controller` in `tests/system_2000.rs`).

QUUX's microcode 1000, muir-sys's first change to MIT's 323, made from
System 1001's sources, ran on both machines: without the signature it took
the CADR's map, processor type 1, five-bit level-1 entries, invalid block
37. System 1001's band reached its listener on it on QUUX on both engines
with version 1000 and type 4. `tests/system_1002.rs` holds that build on
the CADR, with its files in the gitignored `ref/ucode-1000`: the CADR's
System 1002, the band as released, reaches its listener on it on both
engines with version 1000 and type 1 and no level-1 entry above block 37.
The band reads the error table for the running version at boot, `SYS:
UBIN; UCADR TBL 1000`, so the served `sys/ubin/ucadr.tbl` must be that
microcode's. It is not the CADR's own microcode 1000, muir-sys's change to
323 for the CADR, which System 1002 was built on and on which
`tests/system_1002.rs` boots it too.

## What the microcode had to do differently

Microcode 323 knows the CADR's map, five-bit level-1 entries and 31
regions. Using QUUX's other 32 takes microcode written for QUUX. From its sources (System 1001's
`sys/ucadr/`):

- `MAP-FIRST-LEVEL-MAP` and `MAP-WRITE-FIRST-LEVEL-MAP` are five-bit fields
  (`uc-page-fault.lisp`), and the write field cannot simply widen, `VMA<32>`
  not existing: the sixth bit is a second deposit, at `VMA<24>`.
- 37 octal is "no block" in three comparisons (`uc-page-fault.lisp`) and
  block 37 the invalid block, zeroed at boot (`uc-cold-disk.lisp`); on QUUX
  both are 77.
- The reverse first-level map has one word per block in the system
  communication area, words 440 to 477, and the keyboard buffer's header
  follows at 500: 64 blocks' worth does not fit there and has to move.

**Unverified:** that nothing else in the band assumes 32 blocks. What is
known is by reading the sources, and a band run on QUUX's own microcode would
settle it.

## Revision 13: the 40-bit word

muir has QUUX revision 13 beside revision 12: `Geometry::QUUX_13` in the
library, which boots System 2001 (below). `quux` runs it when the
environment variable `MUIR_QUUX_REVISION` is `13`; unset or `12` is
revision 12, and any other value is refused at the start. The switch is not
a documented flag: `--help` and the manual do not name it, since it goes
when revision 12 is retired (contract G2 §8.1). `cadr` does not read it
(`tests/quux_revision.rs`). The start says which revision runs, in its
`machine:` line, and on `rtl` the cache's 8-word lines. Its word is 40
bits: the tag `<39:32>` over the field `<31:0>`. What follows is what muir's revision 13 does and which tests hold
it; everything it does not mention is revision 12's.

**The processor** (`tests/revision_13.rs`, on `micro` and `rtl`):

| | |
|---|---|
| Words | A, M, the PDL buffer, `Q`, `VMA`, `MD` and main memory carry 40 bits (`tests/word_width.rs`); numeric sources read `<39:32>` as 0, an unassigned one all 40 bits as ones; MACHINE-ID says revision 13 |
| The ALU | logical functions on 40 bits; arithmetic, the shifts, `MUL` and `DIV` on `<31:0>`, the result's `<39:32>` M's |
| Fields | BYTE: rotate `IR<5:0>`, length − 1 `IR<11:6>`, LC byte mode by `IR<24>` on an LDB alone, and no misc function; JUMP and DISPATCH: `IR<47>` the rotate's bit 5; DISPATCH: address `IR<23:12>`; ALU output select 0: rotate `IR<5:0>`, length − 1 `IR<9:6>`, no LC byte mode |
| Rotator and masker | a ring of 40, a rotate taken mod 40; a byte that does not fit in bits 0-39 gives an empty mask, the A source showing through |
| LC byte mode | the rotate plus 0 or 24 for halfwords 0 and 1 (`LC<1>` 1 and 0), and 0, 32, 24, 16 for bytes 0 to 3 (`LC<1:0>` 1, 2, 3, 0), mod 40 |
| Conditions | in condition mode `IR<4:0>`: M < A and M ≤ A on the fields, signed; M = A on all 40 bits; 10 the fixnum overflow flag, which every executed ALU word loads, 1 for an arithmetic function whose 33-bit result has bit 32 unlike bit 31; 11 M < A on the fields, unsigned; the rest as `IR<2:0>` |
| Location counter | `LC<29:0>`, a fetch taking `LC<29:2>`; read with NEED-FETCH in `<39>` and INTERRUPT-CONTROL's flags in `<37:34>`, which a write of destination 2 takes from there |
| Dispatch memory | 4,096 entries |
| The map | level 1 8,192 seven-bit entries by `VA<27:15>`, level 2 4,096 28-bit entries by the level-1 entry and `VA<14:10>`; 1024-word pages, the word `{L2<17:0>, VA<9:0>}`; `<27:26>` the access bits and `<23:22>` the dispatch's map bits; an address with `<31:28>` set reads block 177, and a map write there writes nothing; `MAP(MD)` has level 1 in `<38:32>`, the fault bits in `<31:30>` and level 2 in `<27:0>`, and the map write's word in `VMA` the same, `<29>` writing level 1 and `<28>` level 2, both writing level 1 alone |
| The fused return | the halfword's index is M 31 rotated by 34, 40 − 6, under LC byte mode's addend; the operand's delta is `<39:34>` |

**The physical space** is 28 bits (`the_physical_space_is_28_bits` and
`the_frame_buffer_window_holds_the_field`, `tests/revision_13.rs`, on both
engines; `tests/revision_13_memory.rs`):

| Physical | What |
|---|---|
| `0` up to main memory's end | main memory, whole 40-bit words; revision 12's `17000000` and `17777400` are main memory when there is that much of it, and nothing when there is not |
| `1760000000` up to the video controller's buffer's end | the frame buffer window: a write stores `<31:0>` and drops the tag, a read gives the field with tag `005` |
| `1777777400`-`1777777777` | the register page, revision 12's offsets: feature word 0 MACHINE-ID, 1 the level-1 entry's 7 bits, 2 4,096 level-2 entries, 6 4,096 dispatch-memory entries, 13 `1760000000`, 20-24 the board name |
| anything else | nothing: reads 0 and sets word 101's NXM bit. Revision 12's Unibus window and its diagnostic registers are not there |

**The memory cache** has lines of 8 words, the lines of packed storage,
2-way, 4K words; `--cache`'s sizes keep the 8-word line. A fill takes the
board's time for today's 2-beat line and a tick for each further beat,
3 in main memory (40 bytes, 5 beats) and 2 in the window (32 bytes, 4):
**unverified**, until a fill is measured at 40 bits. The 4 KiB boundary a
line crosses 4 times in 512, which the boards' ports issue as two bursts,
costs nothing more here.

**The prefetch** takes the page's reach, the next word in the fetched
word's 1024-word page when the cache holds it, over 28-bit virtual
addresses; line reach stays selectable (`Rtl::set_prefetch`). Page reach
was measured at 8-word lines on the profile's workloads at 1.67% less time
on the Arty's timing and 1.13% on the DE25-Nano's.

**Block-disk** (`tests/revision_13_memory.rs`) moves a 1024-word page an
entry, `<27:10>` the page and `<0>` More, `<9:1>` ignored; command `<12>`
chooses the transfer:

| | Blocks a page | On the disk |
|---|---|---|
| Packed, `<12>` 0 | 5 | the page's 5,120 bytes as main memory holds them: word w at bytes 5w to 5w + 4, `<7:0>` first, the tag last |
| 4-byte, `<12>` 1 | 4 | `<31:0>` of word w at bytes 4w to 4w + 3, low first; a read writes tag `005`, a write drops the tag |

The command list pointer and word 201 are 28 bits; the disk address is
left at the last block moved, or at the one that failed, and word 201 at
the last word moved; a page outside main memory, the window's included,
stops the transfer with NXM `<20>`. A page is read whole before memory is
written, so a transfer that runs past the disk's end leaves the page it
stopped in as it was. The GPT fixture read by a 4-byte transfer and taken
4 bytes a word is the file's bytes.

**The file device** takes rings and buffers at 28-bit addresses on an
8-word line, a ring on a 4-word line being refused, and writes every word
it fills, a buffer's or a response's, as a fixnum, tag `005`; it reads
`<31:0>` of what it reads.

**The checkpoint** of a 40-bit machine is its own version, 50, which says
the width: every word 5 bytes, `<7:0>` first, so that main memory in it is
packed storage byte for byte; the larger dispatch memory and map, the
overflow flag, and the rest. A revision-13 machine refuses a checkpoint of
32-bit words, revision 12's among them. A 32-bit machine's checkpoint is
version 49 as before. `quux` refuses a checkpoint of the other revision,
each way, at the start, naming the revision that wrote it and the switch
that resumes it (`tests/quux_revision.rs`).

**The `.mcr`** at 40 bits has a dispatch memory section of `10000`
entries and A memory as section 5, each location two 32-bit words,
`<31:0>` and then `<39:32>` in `<7:0>`; the parser reads both, in
partition order as in MIT's.

**Its boot PROM** is PROM 2001, `data/quux-promh.mcr`, which `quux`
loads on revision 13; revision 12 keeps PROM 2000,
`data/quux-promh-2000.mcr`, and each revision's `--prom` is held to its
own (`tests/quux_revision.rs`). PROM 2001 is muir-sys's `promh.text`
for revision 13, assembled at `36000` and in partition order as PROM
2000: it maps the register page at `1777777400`, reads the GPT and the
microcode partition by 4-byte transfers, 4 blocks a page, loads a
dispatch memory of 4,096 entries and A memory as section 5, and stops at
`ERROR-A-MEM-SECTION-32-BITS`, 36661, on a microcode partition whose A
memory is section 4 (`tests/quux_prom.rs`, `data/README.md`). The guards
of contract G2 §2.8 that a boot shows, each on both engines with a control
that takes the halt's jumps out of the PROM and does not stop there
(`tests/system_2001.rs`): System 2000's disk on revision 13 stops at
`ERROR-A-MEM-SECTION-32-BITS` before any microcode runs, and System 2001's
disk on revision 12 stops at PROM 2000's `ERROR-BAD-ADDRESS`, 36024, when
the dispatch memory section reaches entry 2,048
(`PROCESS-D-MEM-SECTION`).

**System 2001**, muir-sys's first 40-bit band, boots on revision 13
(`tests/system_2001.rs`, which skips without muir-sys's hand-over, the
pre-release `handover-2001-81b3973` that `tools/fetch-handover-2001.sh`
fetches into the gitignored `ref/band-2001-81b3973`): System 2001's
release disk, a GPT disk of 853,359 blocks in a dynamic VHD, with
microcode 2001 in its current `MCR1`, the band in its current `LOD1` and
a `PAGE` partition of 655,360 blocks, 128MW. At 2MW of main
memory it reaches its listener on both engines, after about 165 million
microcycles on `micro` and 177 million on `rtl`, and at 32MW after
about 168 million and 179 million; it draws its listener at the video
controller's words a line at 1280 by 1024 and 1024 by 768, and at 1920 by
1080, 60 words a line, on both engines (`system_2001_sizes_its_screen_at_boot`);
`(si:disk-restore 1)` reads `LOD1` back through block-disk and boots it
to the listener again; the
PROM writes words 104 and 111 before the disk, and at the listener timer
0 is on, periodic, under its interrupt enable, with `INTR-TICK` 600 times
in 10 s of the machine's time; through the file device it writes its
herald, whose Machine Type line names the board from feature words 20-24,
"QUUX on muir-sim", and whose memory line says "2048K physical memory,
131072K virtual memory", 2MW and 128MW; it reads a form and writes what
it is, `(3 2001 "QUUX" "Experimental System 2001, microcode 2001")`, and
`most-positive-fixnum`, 2147483647; and microcode
2001 fills the MACRO DISPATCH MEMORY with 36 specialised entries and
boots with returns fused under the checkers of
`tests/support/macro_dispatch.rs`, with its own fill and with the
generic one, finding nothing, and fills a stale memory again.

**Main memory** is 32MW by default, the boards' (G2 §3), and
`--main-memory-size` gives it in whole megawords, 1MW to 64MW (G1 §3.2), where
revision 12 takes 1MW to 3MW, 2MW by default (`tests/quux_main_memory.rs`).
At 32MW muir holds 256 MiB for it, 8 bytes a word. A
checkpoint's body is 168,204,521 bytes, main memory 5 bytes a word; the
file packs runs of zeros, and is 248 bytes of an empty memory and
167,772,410 of one full of other words, against 8,388,854 for revision
12's 2MW full. Writing that full checkpoint on `micro` peaked at
757 MB of host memory, against 44 MB for revision 12's.

## Not modeled

`chip` is the CADR's boards, netlist for netlist, and QUUX has none:
`--chip` is `cadr`'s alone.
