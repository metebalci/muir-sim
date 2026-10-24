# QUUX

QUUX is the CADR evolved: the same processor, buses and boards, changed where
a change pays for itself. In muir it is the `quux` executable, and `cadr`
is the CADR as MIT built it; muir-fpga and muir-sys choose it with
`--machine quux`. Back to [the manual](manual.md).

This page says where QUUX, hardware revision 16, differs from the CADR.
Everything it does not mention is the CADR's. Its word is 40 bits, the tag
`<39:32>` over the field `<31:0>` (contract G2). `quux` runs this one
hardware revision and nothing chooses another ([the
manual](manual.md#quux)).

QUUX's software is numbered in the 2000s and the CADR's in the 1000s: QUUX
runs muir-sys's System 2002 on microcode 2002 and boots on PROM 2002.

## What each difference reaches

A change to the hardware reaches further than the board: the boot PROM,
the microcode, the Lisp system in the band, and the tools that read a
machine's state can all depend on what changed. For each of QUUX's
differences, what it needed (muir-sys's sources as handed over with System
2002):

| Difference | Boot PROM | Microcode | Lisp system | Tools |
|---|---|---|---|---|
| The 40-bit word: the tag over the field, the ALU on tags, 6-bit BYTE fields, a dispatch memory of 4,096 entries | PROM 2002 loads A memory as a 40-bit section and the dispatch memory as 4,096 entries, and halts at `ERROR-BAD-WIDTH` on a section of another width (`sys/ucadr/promh.text`) | 2002, assembled at 40 bits | System 2002 | muir's `.mcr` reader takes QUUX's format (`src/mcr.rs`) |
| The page table and the TLB: 32-bit virtual addresses, two windows | the PROM writes no map (`promh.text`) | 2002 sets the directory's first frame, register-page word 220, and the pointer-type register at `INITIAL-MAP` (`uc-cold-disk.lisp`) | System 2002 makes the addresses above 2^31 with `%POINTER-PLUS` (`sys/sys/ltop.lisp`) | muir models the TLB's contents (`src/tlb.rs`) |
| MACHINE-ID in functional source 16 | `GO` halts at `ERROR-NOT-REVISION-15` when the hardware revision is below 15 (`promh.text`) | 2002 reads it at `RESET-DEVICES` and halts at `MACHINE-NOT-QUUX-14` on anything but QUUX of hardware revision 14 or more (`uc-cold-disk.lisp`) | `PROCESSOR-TYPE-CODE` is 4, which the cold load checks (`sys/sys/ltop.lisp`) | nothing |
| The feature page | nothing | nothing: field widths are fixed when the microcode is assembled | System 2002 reads it: the PDL buffer's length, word 3, at every boot (`sys/sys2/proces.lisp`); the video controller's size and buffer address, words 11 to 13 (`sys/sys/ltop.lisp`); and whether the file device and the real-time clock are there, word 15 (`sys/io/fdev.lisp`, `sys/io1/time.lisp`) | do not read it yet |
| `MUL` and `DIV` in one instruction | nothing | 2002 uses them in `MPY`, `DIV` and `BIDIV`'s quotient (`uc-arith.lisp`) | nothing | nothing |
| The clocks: the microsecond clock in the processor, and the interval timers on the register page, timer 0 the tick | writes reset devices and timer 0's period, 16,667 µs | 2002 writes timer 0's period at `RESET-DEVICES`, at every start of the microcode, turns the tick on at `BEG06` (`uc-cold-disk.lisp`) and clears it in `INTR-TICK` through word 110, and turns off timer 1 or 2 if one interrupts (`INTR-TIMER-1-STRAY`, `uc-interrupt.lisp`) | nothing: it reads the microsecond clock, and no timer | nothing |
| Reset devices, word 104 of the register page | writes it before it reads the disk | 2002 writes it at `RESET-DEVICES`, at every start of the microcode, a `%DISK-RESTORE`'s too, in place of `PROG.UNIBUS.RESET` | nothing | nothing |
| Block-disk | PROM 2002 for block-disk, which does not boot the CADR controller | 2002 for block-disk: the disk routines by block number, no cylinder, head or sector | System 2002: the partitions, the band and the disk routines by block number | block-disk is `quux`'s only disk; `--disk-controller`, the CADR's controller, is `cadr`'s |
| The memory cache (`--cache`) | nothing | nothing | nothing | `--cache`; the profile harness's `MUIR_CACHE` |
| No delay lines: one clock of the period `--microcycle-ns` | nothing | nothing | nothing | `--microcycle-ns`; `--timing-model` is `cadr`'s |
| No hung microcycle; the old word in a RAM's write cycle | nothing | nothing (below) | nothing | nothing |
| No speed bits | nothing | the mode register write at boot need not set them | nothing | nothing |
| The video controller, the display | nothing | 2002: no TV vertical flag (`uc-interrupt.lisp`) | System 2002 sizes the main screen from the feature page | the terminal, screenshots and captures show whichever screen is fitted |
| The real-time clock | nothing | nothing | System 2002 sets the time from it at boot when word 15 `<0>` of the feature page says it is there (`sys/io1/time.lisp`) | `--rtc` |
| The file device | nothing | 2002 waits at `RESET-DEVICES` for it to be quiet, word 161 `<1>`, after reset devices, and halts at `FILE-DEVICE-NOT-QUIET` if it is not (`uc-cold-disk.lisp`) | System 2002's `SYS:` is on it, at `/sys` and `/site`, under the pathname host `FILES` (`site/sys.translations`) | `--file-root` |
| The Ethernet device | nothing | 2002 waits at `RESET-DEVICES` for it to be quiet, word 241 `<1>`, when feature word 15 `<2>` says it is there, and halts at `ETHERNET-DEVICE-NOT-QUIET` if it is not (`uc-cold-disk.lisp`) | System 2002's network (`sys/network/ip/ether.lisp`) | `--ethernet-socket`, `--ethernet-mac`, `--ip-address`, `--ip-gateway`, `--ethernet-trace` |
| The fused return: the MACRO-DISPATCH register and the MACRO DISPATCH MEMORY, destinations 5 to 7 | nothing | 2002 fills the MACRO DISPATCH MEMORY and writes the register at `RESET-DEVICES` (`RESET-MACHINE-MACRO-DISPATCH-FILL`, `uc-cold-disk.lisp`) | nothing | the profile harness's `MUIR_H8A` fills and enables it |

## How software tells the two apart

**QUUX answers who it is in functional source 16, its MACHINE-ID**, one
microinstruction, no bus cycle:

| Bits | QUUX | CADR |
|---|---|---|
| 31:16 | signature `0x5155` | nothing drives the M bus: all ones |
| 15:4 | hardware revision: 16 | |
| 3:0 | processor type: 4 | |

Source 16 is one MIT left unassigned: the 74S138 on page SOURCE that
decodes it has that output unconnected, and microcode 323 does not read
it; microcode 2002 does, at boot (above). `IR<30>` is in no source decode,
so source 36 is the same. Source 17 reads all ones on both: open on the
CADR, and on QUUX unassigned (below), all 40 bits. A machine is QUUX only if
bits 31:16 hold the signature.

`tests/quux.rs` holds the word on both engines and the CADR's all ones;
`the_unassigned_sources_read_all_ones_on_the_board` in `tests/output_bus.rs`
holds the CADR's on the netlist.

**Unverified:** that a real CADR reads all ones there. The M bus has only
tri-state drivers and no pull-ups, so for an unassigned source it floats, and
TTL reading an open input as high is what the netlist model does and what
the parts usually do, not what a datasheet promises. The 16-bit signature is
what makes that safe: a floating bus would pass for QUUX once in 65,536 at
worst. A CADR reading source 16 would settle it.

## The 40-bit word

**QUUX's word is 40 bits** (contract G2 §2, appendix A1): A, M, the PDL
buffer, `Q`, `VMA`, `MD` and main memory carry the tag `<39:32>` over the
field `<31:0>` (`tests/word_width.rs`). `tests/revision_13.rs` holds the
rest on `micro` and `rtl`:

| | |
|---|---|
| Sources | numeric sources read `<39:32>` as 0, an unassigned one all 40 bits as ones |
| The ALU | logical functions on 40 bits; arithmetic, the shifts, `MUL` and `DIV` on `<31:0>`, the result's `<39:32>` M's |
| Fields | BYTE: rotate `IR<5:0>`, length − 1 `IR<11:6>`, LC byte mode by `IR<24>` on an LDB alone, and no misc function; JUMP and DISPATCH: `IR<47>` the rotate's bit 5; DISPATCH: address `IR<23:12>`; ALU output select 0: rotate `IR<5:0>`, length − 1 `IR<9:6>`, no LC byte mode |
| Rotator and masker | a ring of 40, a rotate taken mod 40; a byte that does not fit in bits 0-39 gives an empty mask, the A source showing through |
| LC byte mode | the rotate plus 0 or 24 for halfwords 0 and 1 (`LC<1>` 1 and 0), and 0, 32, 24, 16 for bytes 0 to 3 (`LC<1:0>` 1, 2, 3, 0), mod 40 |
| Conditions | in condition mode `IR<4:0>`: M < A and M ≤ A on the fields, signed; M = A on all 40 bits; 10 the fixnum overflow flag, which every executed ALU word loads, 1 for an arithmetic function whose 33-bit result has bit 32 unlike bit 31; 11 M < A on the fields, unsigned; 12 M ≤ A on the fields, unsigned (`condition_12_is_m_at_most_a_unsigned` in `tests/revision_14.rs`); the rest as `IR<2:0>` |
| Location counter | `LC<33:0>`, a byte address, a fetch taking `LC<33:2>` ([the location counter](#the-page-table-and-the-tlb)); read with NEED-FETCH in `<39>` and INTERRUPT-CONTROL's flags in `<37:34>`, which a write of destination 2 takes from there: LC byte mode `<37>`, `PROG.UNIBUS.RESET` `<36>`, `INT.ENABLE` `<35>`, `SEQUENCE.BREAK` `<34>`, where the CADR has `<29:26>` |
| Dispatch memory | 4,096 entries |

## The PDL buffer

**QUUX's PDL buffer is 16K words**, its pointer and index 14 bits where the
CADR's are 10. They read back whole in functional sources 2 and 3, whose
upper bits read 0 on the CADR. `quux_s_pdl_buffer_is_4k_or_16k` in
`tests/quux.rs` holds a push past word 1777 landing above it and the wrap at
the buffer's own size. It needs QUUX's boot PROM (below): MIT's stops copying
A memory in on the index wrapping at 2000 words. Microcode for QUUX has to
know the size.

## The feature page

**QUUX lists its sizes in words 0-77 of its register page**, at
`35777777400` to `35777777477` in the device window ([the register
page](#the-register-page)). They are read-only and read like any device
register:

| Word | QUUX |
|---|---|
| 0 | the MACHINE-ID, as source 16 gives it: `0x51550104` |
| 1 | level-1 map entry: 0 bits, there being no map |
| 2 | the TLB: 4,096 entries |
| 3 | PDL buffer: 16,384 words |
| 4 | control store: 16,384 words |
| 5 | A memory: 1,024 words |
| 6 | dispatch memory: 4,096 words |
| 7 | multiply and divide: 3, bit 0 `MUL` and bit 1 `DIV` |
| 10 | the tick, timer 0: 1 |
| 11 | the main screen: width in 31:16, height in 15:0 |
| 12 | the main screen: bits a pixel in 31:16, words a line in 15:0 |
| 13 | the main screen: its buffer's first address, the device window's base, `34000000000` |
| 14 | the microsecond clock: 1 |
| 15 | the optional devices, a bit each: 7, bit 0 the real-time clock, bit 1 the file device and bit 2 the Ethernet device |
| 16 | the number of interval timers: 3 |
| 17 | the MACRO DISPATCH MEMORY's entries: 1,024 |
| 20-24 | the board name, 4 characters a word in 31:0 |
| 25 | the microcycle, the clock's period, in units of 0.5 ns: 20 at the default 10 ns, and what `--microcycle-ns` sets (`feature_words_0_and_25_say_revision_15_and_its_period`, `word_25_reads_the_period_on_rtl`) |
| 26-77 | 0 |

**The board name**, words 20-24, names what runs the
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

Words 11 to 13 describe whichever display is fitted: the video controller's
1280 by 1024, one bit a pixel, 40 words a line, or, on a QUUX run with a
CADR board, that board's 768 by 963, one bit, 24 words a line, both in the
device window.

On the CADR the same words of its last page, `17777400`, are in its Unibus
window, Unibus `777000`-`777776`, where nothing answers: a read there times
out and sets the Unibus NXM bit, `766044` `<3>`, as a read of any empty
Unibus address does. Software reads source 16 first and the page only on
QUUX, and so never waits for the timeout.
`quux_lists_its_sizes_in_its_feature_page` in `tests/quux.rs` holds the
page on QUUX and the timeout on the CADR, on both engines.

## Multiply and divide

**QUUX multiplies in one instruction and divides in one**, ALU functions 42
and 43, on the field, `<31:0>`. The CADR takes a step per bit: `MUS`, `DVS1`, `DVS` and
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
| Time | 5 clocks in EX, 4 of them held, once its operands are ready | 18 clocks in EX, 17 of them held, once its operands are ready |

Each step's M operand is the output bus of the step before, as when the
microcode writes the step's result back to the same M location. Both
instructions decode on `IR<8>` and `IR<4:3>` only, as the '139 does, and
drive the output bus and load `Q` whatever the output selector `IR<13:12>` and
the Q control `IR<1:0>` say. `DIVIDE-LAST-STEP` and `DVREM` stay the CADR's
separate instructions.

**A `DIV` stays in EX 18 clocks and a `MUL` 5**, at every period (contract
G3 revision 15, A15b.3; its MP2b ruling Q3), 17 and 4 of them held. A
register operand is ready at once. An `MD` operand first waits for its read
to land, through the MD interlock any instruction reading `MD` has, and the
count starts when that wait ends, so that a `DIV` of `MD` divides the word
that landed. `micro` holds its microcycle for the same number of clocks, 17
for a `DIV` and 4 for a `MUL`, counted from when the instruction enters
`IR` (`src/micro.rs:2818-2819`, `src/muldiv.rs:82-83`). The time does not
depend on the operands. The hold does not stop a single step.

`tests/muldiv.rs` holds each against the step sequence on the CADR, for many
operands and every output selector and Q control, on `micro` and `rtl`, with
the step sequence itself held to the netlist; the CADR's 42 and 43 to the
netlist; and the CADR's 43 held for nothing (`on_the_cadr_43_is_not_held`).
`div_stays_18_clocks_in_ex_and_mul_5` in `tests/revision_15_rtl.rs` holds the
pipeline's clocks at three periods, a `DIV` of `MD` counted from the landing,
with the results as `micro`'s.

## The clocks

**QUUX has clocks of its own**: a microsecond clock in the processor
(contract Q1), and three interval timers on the register page, timer 0, 1
and 2 (contract Q11), timer 0 being the tick, the machine's
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
  written by the boot PROM (contract Q11). The microsecond
  clock moves on through every reset.
- **Instants.** A timer word is read or written at the instant the
  register's cycle is taken, and a period starts from that instant. A read
  gives the flags as they stood then, a rise at that instant itself
  counting as before it
  (`m1_a_periodic_timer_rises_every_period_on_its_start_s_grid` in
  `tests/interval_timers.rs`).

**Functional destinations 3 and 4 write only M, and functional source 17
reads all ones**, on QUUX as on the CADR: Q1's tick control, its interval
timer's period and their status are not there, and every timer is reached
through the register page alone. A write of destination 3 changes no
timer. So microcode that turns its tick on through destination 3 has no
tick.

**The microsecond clock** is functional source 15: the microseconds since
power-on, 32 bits, wrapping, one read giving the whole word.

On the CADR, destinations 3 to 7 have no output on the 74S138 that decodes
them and write only M, and sources 15 and 17 have none either and read all
ones. Microcode 323 writes destinations 3 to 7 and reads sources 15 and 17
nowhere, by a scan of every control-store word, and running shows the same
of what the OA registers make at run time: `tests/unused_codes.rs` reads
every executed microinstruction as it stood in `IR` through a boot to the
listener. System 1003 on the CADR's microcode 1001, on the CADR, runs
none.

`tests/interval_timers.rs` holds, for each timer, the periodic grid to the
nanosecond with late clears and a period written under a raised flag, the
one-shot's one rise, the mode taken at turn-on, the independence of the
three, every reset on both engines, the interrupt under `<8>` and not
without it on both engines, the layout, destination 4 and source 17 on QUUX,
a checkpoint resumed to the same rises, and destination 3 changing no timer
on either engine. `tests/tick.rs` holds the tick as timer 0 at the PROM's
period, turned on through word 110 and not through destination 3, the
microsecond clock against each engine's time and across its wrap, and the
CADR's all ones.

## QUUX's clock

**A QUUX microcycle is one clock of the period `--microcycle-ns` gives**,
5 to 63.5 ns in steps of 0.5, 10 ns by default. The CADR's clock is a string
of delay-line phases: the read phase ends at the tap the mode register's
`SPEED1` and `SPEED0` choose (`mit/cadr/ir.bits`: "00 Extra slow, 01 Slow, 10
Normal, 11 Fast"), and a 60 ns restart follows, 145 ns at normal speed.
QUUX has none of it: no taps, no speed bits (a write of the mode register's
bits 1 and 0 goes nowhere; its other bits are unchanged), and no timing
model. `--timing-model` is `cadr`'s alone
(`the_timing_model_is_the_cadr_s` in `tests/cli.rs`).

**Time** is in units of 0.5 ns, `Machine::ns` counting them. The period is a
whole number of them, so that every period the flag admits is exact and
clock *k* ends at the instant *k* times the period; the Kria KR260's
revision 15 fit closes at 19 ns. A device's duration is turned to clocks
once, rounded up, and a timer keeps true time over a second at every period
(`a_command_s_time_is_rounded_to_clocks_once`,
`a_1_us_timer_rises_at_clock_1649_the_14th_time`,
`the_accumulators_keep_true_time_over_a_second`). The reports say the
machine's time and the period in ns, with ".5" where an instant is odd
(MP2b ruling Q1): the prompt's `pc` and `step` lines, the resume line, and
`--pace`'s line; and the run's report measures it against the hardware at
the period
(`the_prompt_s_pc_on_revision_15_prints_an_odd_instant_s_half`,
`a_resume_on_revision_15_prints_an_odd_instant_s_half`,
`pace_on_revision_15_says_its_period`,
`the_run_s_report_on_revision_15_is_against_the_period`). Feature word 25
says the period ([the feature page](#the-feature-page)).

`quux_has_no_speed_bits` in `tests/quux.rs` holds the speed bits, and
`--microcycle-ns` is held in `tests/revision_15.rs`: its range and
refusals, the period in the start, in `--pace`'s line and in a checkpoint,
which a resume under another period refuses.

## The memory port and the device registers

**QUUX's main memory and its frame buffer are on the processor's own
port**, the memory bus (contracts Q6 and Q7), and **its devices are reached
by their registers alone** (contract Q7): there is no Xbus and no bus in
its place. A cycle to main memory or the frame buffer goes through the
cache to the memory controller; one to a device register goes to the
processor's register decode; one to any other address fails at once. QUUX
has no bus interface: the Unibus is gone (Q5) and the processor is the
only requester. Devices move bulk data to and from memory themselves ---
block-disk's transfers, the display reading its buffer --- and carry
control and small data in their registers. The CADR keeps its Xbus, Unibus
and bus interface. The boot PROM is not in the address space: it is in the
control store (Q2).

A reference leaves the processor on a 29-bit bus address, the cache's key
([the page table and the TLB](#the-page-table-and-the-tlb) has how a virtual
address becomes one): `<28>` clear and the physical word address for main
memory, `<28>` set and the device window's offset `<27:0>` for the device
window.

| | |
|---|---|
| Main memory | from 0 up to its end: whole 40-bit words. A line of 8 words is 40 bytes, 5 beats of the 64-bit port. `--memory-timing <read>,<write>,<occupancy>`, or `kria`, `arty` or `de25`, gives a line fill, a write's response and the interval between two accepted writes: `kria`, the default, 344, 215 and 10 ns, `arty` 290, 120 and 16, `de25` 600, 485 and 40 (`PortTiming`, `src/pipeline/port.rs`). **Unverified**: `kria`'s occupancy, 10 ns, and `de25`'s, 40 ns |
| The frame buffer | from the device window's base, `34000000000`, the video controller's buffer, at most 4,193,280 words: on the memory bus with main memory, cached, 4 bytes a word. A write stores the field and drops the tag, and a read gives the field with tag `005`, a fixnum's (G1 §4.2). A line of it is read in 4 beats, and the port has one fill time for both, so a fill there takes the time of main memory's. The software reads it back (`BITBLT` combines with the destination, scrolling copies), and the display only reads it, which a write-through cache keeps current |
| The cache | always fitted: 64K words by default (`--cache <words>` another size), unified and write-through, 2-way, in lines of 8 words, by bus address. A hit lands two clocks after the grant and a miss when its line's fill does; a write allocates nothing. [`rtl` is the pipeline](#rtl-is-the-pipeline) has the port's queue and the cache's rules |
| A device register | a word of the register page, `35777777400`-`35777777777` in the device window, the feature page, block-disk's and the video controller's among them: never cached |
| Nothing there | past main memory's end below the device window, past the frame buffer's end in the window, the window's reserved slices, and the words below the register page: fails at once, in the microcycle, reading 0 and setting word 101's NXM bit, with no timeout. The CADR's addresses from `17000000` up are main memory's when there is that much of it, and nothing when there is not. A write to nothing does not read back, which the microcode's memory-size probe, `RESTORE-MEM-SIZE-LOOP` in `uc-cold-disk.lisp`, relies on: it tests the value read back, not the time |
| Block-disk | its words move at START, which first lands every posted write, so that a transfer reads every word written before it and a read's words are not overwritten by an older write (`block_disk_s_start_lands_the_posted_writes`); the transfer clears the cache's sets of the words it writes and no others (`a_block_disk_transfer_clears_the_sets_it_writes_and_no_more`) |

The bus interface's registers all have homes on QUUX already: the
diagnostic registers are the host's (Q5), `ERRSTOP` is word 102, the
interrupt control word 100 and the error status word 101 (Q2); the Unibus
map is gone (Q5).

`tests/quux_memory_port.rs` holds nothing past main memory's end reading
back on both engines; `tests/quux_device_registers.rs` every empty range
failing at once on both engines, a reserved register answering 0, and a
start right after a start waiting for it; `the_frame_buffer_window_holds_the_field`
and `a_store_to_a_word_spanning_two_beats_is_whole` in `tests/revision_13.rs`
the window and a packed word across two beats on both engines;
`quux_s_memory_port_and_its_timing` in `tests/cli.rs` the flag and the
start's report.

## Block-disk

**QUUX's disk is block-disk**, and nothing else: a `quux` run has it,
and `--disk-controller`, the CADR's controller, is `cadr`'s alone
(`quux_s_disk_is_block_disk_only` in `tests/cli.rs`). It is the CADR
disk controller's programming interface with the drive's geometry taken
out. Blocks are numbered from the start of the disk, 1,024 bytes each, and
a transfer moves 1024-word pages of 40-bit words (contract G2 §4.2,
appendix A1.11).

| | CADR controller | block-disk |
|---|---|---|
| Registers | `17377774`-`17377777`: status and command, command list pointer, disk address, START | the same four, words 200-203 of the register page, `35777777600`-`35777777603` |
| Command list | one word a block, `<23:8>` the page's physical address, `<0>` More | one word a page, `<27:10>` the page, `<0>` More, `<9:1>` and `<39:28>` ignored; the pointer 28 bits |
| Disk address | cylinder `<27:16>`, head `<15:8>`, sector `<7:0>`, unit `<30:28>` | the block number, `<27:0>`; one disk, unit 0 |
| Commands | read, read compare, write, read all, write all, seek, at ease, recalibrate, offset clear, reset | read, 0, and write, 11; any other stops by error. Command `<12>` chooses the transfer (below) |
| Status | not active, attention, errors of the drive, the ECC and the transfer | `<0>` not active, `<3>` interrupt request, `<9>` no disk, `<13>` stopped by error, `<17>` past the end of the disk, `<20>` NXM |
| After a transfer | the disk address at the last block moved, or the one that failed | the same; word 201 at the last word moved |
| Interrupt | done, command `<11>`; attention, `<10>` | done, command `<11>` |
| Time | seeks and rotation, when timed | 100 us a block moved, **unverified**: an estimate until muir-fpga measures its disk path |

| Transfer | Blocks a page | On the disk |
|---|---|---|
| Packed, `<12>` 0 | 5 | the page's 5,120 bytes as main memory holds them: word w at bytes 5w to 5w + 4, `<7:0>` first, the tag last (G1 §4.1) |
| 4-byte, `<12>` 1 | 4 | `<31:0>` of word w at bytes 4w to 4w + 3, low first; a read writes tag `005`, a write drops the tag |

A page outside main memory, the window's included, stops the transfer
with NXM `<20>`. A page is read whole before memory is written, so a
transfer that runs past the disk's end leaves the page it stopped in as it
was.

**Block-disk's contract is the outcome, not the time**: the status bits, the
disk address left at the last block moved or at the one that failed, the
command list pointer and memory. How long a transfer stays active, whether
it succeeds or fails, is the implementation's, and software waits for
not-active before it reads the status. muir's model does as follows, and a
fabric that walks its list through a host stays active until the walk ends,
a failure too.

The words move inside the store to START, and the controller stays busy for
a block's time each; a transfer writes main memory behind the processor, so
the cache clears the sets of the words it wrote. The disk is a file in a
standard format,
[below](#the-disk-file). `tests/block_disk.rs` holds the read, the write,
the end of the disk, the NXM, a command it does not do, the registers on
QUUX's register page with nothing at the CADR's, and a checkpoint;
`tests/revision_13_memory.rs` the packed and the 4-byte transfers against
the disk file, and the GPT fixture read by a 4-byte transfer and taken 4
bytes a word as the file's bytes.

PROM 2002, microcode 2002 and System 2002 address it by block.

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
  comment of up to 31 characters, `MCR1 UCADR 2002`: 36 characters, which
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
`tests/diskpack.rs`). The boot PROM, `data/quux-promh.mcr`, the microcode
and the bands read the GPT; MIT's label in block 0 is the CADR's.

**System 2002's band is on a GPT disk in a dynamic VHD**: its disk,
853,359 blocks, with microcode 2002 in its current `MCR1`, "MCR1 UCADR
2002", the band in its current `LOD1`, "LOD1 System 2002", and a `PAGE`
partition of 655,360 blocks, 128MW at 5 blocks a page
(`tools/fetch-handover-2002.sh` checks the layout of the hand-over's disks,
and `tests/system_2002.rs` boots them). QUUX boots the VHD as it is, a copy
of it, the disk being written. Its `SYS:` is on the file device, which the
tests serve the band's sources through.

How to make a disk with standard tools is in
[the manual](manual.md#quuxs-disk).

## The single-edge contract

**What a single-edge core must keep**, for H1b: in which microcycle each
resource's new value is seen, counted from the microcycle whose
instruction produced it (cycle *n*). It is the CADR's `rtl`'s read phase,
write phase and edge, which `tests/cosim.rs`, `tests/chip.rs` and
`tests/dispatch_write_order.rs` hold to the netlist; an FPGA core that
keeps every row, and stalls where a row needs it, runs the microcode
unchanged. `rtl` is the single-edge machine on the CADR alone: on QUUX it is
the pipeline ([below](#rtl-is-the-pipeline)), whose rows are `micro`'s.

| Resource | Written | Seen by |
|---|---|---|
| `IR` | at the edge ending *n*, from the I bus: the control store at `PC`, the boot PROM, the debug IR, or `IWR` in the cycle after `WRITE-I-MEM` | the instruction executed in *n*+1. The instruction at a jump's target runs in *n*+2; the one after the jump runs in *n*+1 unless `N` inhibits it |
| `PC`, `LC`, `Q`, `VMA`, `MD` (from the processor), `INTERRUPT-CONTROL`, the PDL pointer and index, the SPC pointer, the flags | at the edge ending *n* | *n*+1 |
| A memory, M memory | in *n*+1's write pulse, from `WADR` and `L` registered at the edge ending *n* | *n*+1, through the pass-around (ACTL 3B21/3B27, MCTL 4B18: a source address equal to the pending `WADR` reads `L`); the memory itself from *n*+2 |
| PDL buffer | in *n*+1's write pulse, at the pointer or the index as it stands then: only the choice between them is registered (`PWIDX`), not the address | *n*+2: no pass-around, so *n*+1 reads the word as it was (`pdl_read_right_after_a_push_on_the_board`, `tests/cosim.rs`) |
| SPC stack, a push | in *n*+1's write pulse, at the pointer the edge ending *n* moved to | the next-address path in *n*+1 (`SPCWPASS` puts the word on the `SPC` bus); an M-source read of the stack in *n*+1 reads the RAM's old word at the new pointer; *n*+2 reads the new |
| Map, a `WRITE-MAP` store | in *n*+1's write pulse, both levels, addressed by `MAPI` then (`VMA` while `MEMSTART`, else `MD`) | `MAP(MD)` and a dispatch on map bits read the old word in *n*+1 (QUUX's definition; below) and the new from *n*+2. A memory cycle an instruction in *n*+1 starts is translated at the edge ending *n*+2, through the new word: `PHYS-MEM-READ` stores the map and starts a read in the next instruction. QUUX's `WRITE-MAP` is the TLB's operation, [below](#the-page-table-and-the-tlb) |
| Dispatch memory, a dispatch write | in *n*'s own write pulse, at its own `DADR` | the dispatch in *n* reads the old word (QUUX's definition); from *n*+1 the new |
| Control store, `WRITE-I-MEM` | in *n*+1's write pulse, at the `PC` it moved to. N nops *n*+1's word; `IR` takes `IWR` at the edge ending *n*+1, and `IWRITED`'s N nops it | `IWRITED`'s POPJ returns to the address the write pushed, the word after it in execution order (the next in sequence, or the target of the transfer whose slot it fills), which is fetched from the RAM after the write: a write of that word runs the new word, on `rtl`, `micro` and the pipeline (`a_write_i_mem_of_the_word_after_it_runs_as_the_board_runs_it`, `a_write_i_mem_in_a_delay_slot_goes_on_at_the_jump_s_target`). That is MIT's form, `IR<9:0>` 1647; CC's form without N runs the word fetched during the write as it was, and costs one nopped microcycle where MIT's costs two (`a_write_i_mem_costs_its_nopped_microcycles_on_micro_as_on_the_board`) |
| OA registers, `IMOD` | at the edge ending *n* | or'd into `IR` as it loads at that same edge: the instruction executed in *n*+1. QUUX's OA registers are read only through a word's OA select ([below](#the-64-bit-microinstruction)) |
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
  in `MD`.
- A memory cycle goes out at the edge ending the microcycle after its
  start, and a write carries `MD` as it stands then: an `MD` loaded in
  that microcycle is the word written, on `rtl` and `micro` alike; one
  loaded later waits on `MBUSY.SYNC` for the cycle to end
  (`the_engines_write_the_md_of_the_microcycle_after_the_start` in
  `tests/chip.rs`). QUUX leaves this rule: its write carries `MD` as its
  start's own microcycle leaves it (below, the pipeline's rows).
- **QUUX holds a memory start in the microcycle right after a start**, a
  `-WAIT` term of its own, `MEMSTART AND MEMOP`, until the first cycle has
  gone out and ended; both then land as written, a first write with the
  `MD` from before the held microcycle, a first read's word in `MD` before
  the second's, the word right after the second start reading the first's
  (`row_a_read_start_right_after_a_read_start_both_land` in
  `tests/revision_15_rtl.rs`), an instruction fetch held the same way
  (`a_start_right_after_a_start_waits_for_it` in
  `tests/quux_device_registers.rs`). On the CADR nothing holds it: one
  cycle goes out for the two starts, with the second start's direction,
  page and `VMA<7:0>`, and the first is lost, measured on `chip` and held
  on `rtl` and `micro`
  (`on_the_board_a_start_right_after_a_start_loses_the_first`,
  `a_start_right_after_a_start_goes_out_as_the_second` and
  `a_fetch_right_after_a_write_loses_the_write` in `tests/chip.rs`).
  **Unverified** that muir-fpga's fabric holds it.
- **QUUX's definition**: a RAM read in the cycle its own write pulse fires
  --- the dispatch word a dispatch writes, the map word right after a map
  store --- gives the word from before the write
  (`Geometry::old_word_while_written`; `on_quux_rtl_and_micro_take_the_old_word`
  in `tests/dispatch_write_order.rs`), as an FPGA's block RAM gives it.
  On the CADR the RAM's output floats while written and the answer is a
  race; muir's CADR engines take `chip`'s answer, the new word, which rests
  on its clock cutting the pulse at the edge
  (`on_the_cadr_rtl_and_micro_take_the_word_chip_does` and the two map
  tests beside it).
- **Nothing MIT's microcode runs does either.** Counted on `rtl` over a
  boot of System 1001 and the profile harness's thirteen workloads on the
  CADR (System 1001's own microload, 324, which is 323 rebuilt): no
  dispatch write with `POPJ`, no map read in the microcycle a map store's
  write lands, and no hung microcycle whose pulse writes the dispatch
  memory or the map. The same counters fire on the test programs built to
  do each. **Unverified** for microcode 2002, which no count has run.

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
| Buffer | 40,960 words in the frame buffer window, `34000000000`-`34000117777` in the device window: 40 words a line, 1,024 lines, 4 bytes a word, a read giving tag `005` and a write dropping the tag |
| Pixel | pixel `x` of line `y` is bit `x mod 32` of word `40 y + x / 32` (at the default size), the low bit leftmost, as on the CADR's TV |
| Mode, word 210 of the register page, `35777777610` | bit 2, black-on-white, reads back; every other bit reads 0 and a write of it is dropped |
| Words 211-217 | reserved: read 0, writes ignored |
| The CADR's control registers, `17377760`-`17377767` | main memory's addresses when there is that much of it; on a QUUX of 2MW nothing there: an access fails at once and sets word 101 `<0>` |
| Interrupt | none |

**Its size is muir's to choose**, `--video-size <width>x<height>`: the width
a multiple of 32, and at most **1920 by 1080**, the largest QUUX supports
(`a_size_is_checked` in `tests/video.rs`). That is 64,800 words. The device
window lets the buffer reach 4,193,280 words, up to `34017775777`
(`the_buffer_may_fill_its_window`), which no word states to the software.
The feature page's words 11 to 13 give the size and the buffer's address
to the software. The table above is the default size.

**The boards' sizes differ**, each fixed in its bitstream: muir-fpga's Arty
Z7-20 and DE25-Nano are 1280 by 1024, its Kria KR260 1920 by 1080. A band
reads the size from words 11 to 13 at boot, so one band runs on each. At
1920 by 1080 the buffer is 60 words a line, 64,800 words; the window is
`34000000000`-`34000176437`, and `34000176440` is nothing and sets word 101
`<0>` (`full_hd_s_window_ends_at_64800_words` in `tests/revision_13.rs`, on
both engines).

1280 bits a line is 40 whole words, which `BITBLT` needs of a screen array's
first dimension (`BITBLT-DECODE-ARRAY` in `sys/ucadr/uc-tv.lisp`).

System 2002 reads the main screen's size from the feature page's words 11 to
13 (`video-width`, `video-height` and `video-words-per-line` in
`sys/sys/ltop.lisp`), and draws its listener at each size's own words a
line: booted at 1024 by 768 on `micro`, and at 1920 by 1080 on both engines,
as at the default (`system_2002_sizes_its_screen_at_boot` in
`tests/system_2002.rs`, where the hand-over is fetched).

`tests/video.rs` holds the buffer's first and last words and the NXM past
it on both engines, QUUX's decode of the whole buffer and of the largest
one, the pixel order and the terminal's frame, word 210 and the reserved
words after it, the CADR's registers answering nothing, the absence of an
interrupt over a second, and the feature page's three words.

## Its boot PROM, in its own addresses

**QUUX's boot PROM has control store addresses of its own**, 36000-37777,
1K words, read only and never overlaid (contract Q2). Reset starts the PC
at 36000; the microcode lives in 0-35777, which is RAM from the start, and
there is no PROM-disable bit: the PROM loads the microcode and jumps to 6.
A reboot is a jump to 36000. The CADR keeps MIT's overlay: its PROM covers
0-1777, the first 1K words, until `PROMDISABLE` in the mode register,
written at Unibus `766012`, lets the RAM show through.

The PROM is muir-sys's PROM 2002 for block-disk and a GPT
(`data/quux-promh.mcr`), MIT's `promh.text` changed so that the disk is
read by block number, nothing is saved, the microcode is found through the
GPT, the devices are reset through the register page and timer 0 given its
period, the register page is at `35777777400` with block-disk's registers at
its words 200-203, the GPT and the microcode partition are read by
block-disk's 4-byte transfers, 4 blocks a 1024-word page, and the
microcode, in QUUX's own `.mcr` format ([the 64-bit
microinstruction](#the-64-bit-microinstruction)), is loaded: its control
store of 64-bit words, its dispatch memory of 4,096 entries, its symbol
area and its A memory of 40-bit words (muir-sys's `sys/ucadr/promh.text` as
committed in muir-sys `2999694`, assembled at 36000; `data/README.md` has
where the file came from and its digest). `GO` is at 36043, the file's first
word at 36000 being `JUMP GO`. It sets error stop through the register page,
not `766012`, and halts at `ERROR-MICROCODE-TOO-BIG` if a microcode reaches
36000. The control store stays 16K words: jump targets are `IR<25:12>`,
dispatch words carry 14 address bits, and `SPC<14>` is the
macroinstruction-return flag, so 32K waits for a new microinstruction
format.

**It checks the machine first**: at `GO` it halts at `ERROR-NOT-REVISION-15`
when MACHINE-ID's hardware revision is below 15, by bit tests alone, before
it makes anything (`promh.text`).

**It finds the microcode through the GPT**: the first microcode partition
in the entry array carrying attribute bit 48 (muir-sys). Its own halts,
from the hand-over's error table `promh.tbl` and symbols `promh.sym`, are
`ERROR-NO-GPT` at 36776, no GPT (a header without the signature "EFI PART",
an entry array that does not start on an even sector below 2^32, or entries
that are not 128 bytes); `ERROR-NO-CURRENT-MICR` at 37000, no current
microcode partition; and `ERROR-ODD-MICR-START` at 37002, one whose first
LBA is odd. Reading the microcode in QUUX's format, it halts at
`ERROR-MCR-FORMAT` at 37006 on a first word that is not the format word,
`ERROR-MICROCODE-NOT-REVISION-15` at 37010 on a first section that is not
section 6 saying 15, `ERROR-BAD-SECTION-TYPE` at 36022 on a type other than
1-4 and 6, `ERROR-SECTION-ORDER` at 37012 on a type twice or out of order,
`ERROR-BAD-WIDTH` at 37014 on an actual width that is not the machine's or a
storage width that is not a multiple of 32 or is below it,
`ERROR-NONZERO-PADDING` at 37016 on a padding bit or an unused header word
that is not zero, `ERROR-BAD-ADDRESS` at 36024 on an item past its memory,
`ERROR-MICROCODE-TOO-BIG` at 36036 on a control store at or past 36000, and
`ERROR-BUFFER-NOT-LOADED` at 36042 on no symbol area or one other than
6000-7777 (`promh.text`). `tests/quux_prom_halts.rs` reaches each of these
halts on both engines, from a fault planted in a disk made here, and
`ERROR-NOT-REVISION-15` on a machine of revision 0, 13 and 14, with no block
of the disk written; the halts' addresses are held to the hand-over's
`promh.sym` where it is fetched. Every halt after the PROM sets error stop
stops the machine; `ERROR-NOT-REVISION-15` comes before it, so `ERR` goes up
and the clock runs on, the PC held at the halt by its jump to itself.

**It saves nothing and writes no block of the disk** (contract Q8). MIT's
PROM saves main memory's page 0 to block 1 before it loads anything
(`SAVE-A-PAGE`, `mit/sys/ucadr/promh.text`), and on a GPT disk block 1 is
the partition table's entry array. QUUX's reads every page into its buffer
at physical page 3 of 1024 words, words 6000-7777, and loads the microcode's
symbol area, which lies there, last, over the buffer (`promh.text`: "the
prom writes nothing to the disk"). `tests/quux_prom_saves_nothing.rs` holds
it on both engines, from the boot to the microcode's location 6, on a disk
made here: no block written, the disk file unchanged byte for byte, main
memory stored only in page 3 and word 2377, the command list word of its
transfers, and page 3 then holding the microcode's symbol area;
`tests/system_2002.rs` holds the same of the hand-over's disk and microcode
2002, where it is fetched.

**Its file is QUUX's own `.mcr`**, as muir-sys's builder writes it for the
PROM and the microcode alike: the format word `51550001`, section 6 first
saying 15, and the control store section from 36000, 64 bits a word, and a
whole number of 1024-byte blocks, so that `dd` writes a microcode file into
its partition with no conversion. The CADR's `.mcr` stays MIT's, and so
does `diskpack`, which is the CADR's.

`tests/quux_prom.rs` holds the start at 36000, the PROM read only, the RAM
below live with no disable, the CADR's overlay, and the built-in file byte
for byte the hand-over's PROM 2002, by its SHA-256, section 6 and address
(`the_built_in_prom_is_prom_2002`); `tests/system_2002.rs` holds
`data/quux-promh.mcr` byte for byte the hand-over's `promh.mcr` and the
sources' `sys/ubin/promh.mcr` (`prom_2002_is_the_hand_over_s`), and boots
System 2002 on it, where the hand-over is fetched. `tests/cli.rs` stops
`quux` at `GO` and boots a disk made here with a microcode of QUUX's format
to the microcode's location 6, on both engines
(`quux_s_stops_are_the_microcode_s_and_the_prom_s`). `--prom` on `quux`
takes a file in QUUX's own format whose control store starts at 36000, and
refuses MIT's (`quux_s_prom_is_assembled_at_36000`).

**It resets the devices and gives timer 0 its period** (contract Q11). It
pulses no `PROG.UNIBUS.RESET`, which resets nothing on QUUX. Before its
first disk command it writes word 104 with 1, reset devices, so that a
reboot starts the microcode with no timer on, the file device disabled and
block-disk idle; and then word 111 with 16,667, timer 0's period, since no
timer resets to a period. It turns no timer on. At location 6 timer 0 is
off at period 16,667 with its interrupt enable 0, and timers 1 and 2 are in
their reset state (`promh.text`).

## The register page

**QUUX's device registers are on one page of 256 words**, the register
page, `35777777400`-`35777777777`, the last page of the device window
(contracts Q2, Q13 and G2 §4.1; G3 revision 14, A14.1). Microcode 2002
reaches word *w* at virtual `35777777400` + *w*, virtual equal to physical
there (`QUUX-REGISTER-PAGE-VIRTUAL-ADDRESS` in `uc-cadr.lisp:65`), and
Lisp's `%xbus-read` at the offset `17777400` + *w* from
`IO-SPACE-VIRTUAL-ADDRESS`, `35760000000` (`FEATURE-PAGE-XBUS-ADDRESS` in
`sys/sys/ltop.lisp`, `sys/cold/qcom.lisp`). A register access is never
cached ([the memory port](#the-memory-port-and-the-device-registers)).

| Words | Device | Words and bits |
|---|---|---|
| 000-077 | the feature page | read only ([above](#the-feature-page)); 17-77 read 0 |
| 100-107 | the page's own words | 100: interrupt status, read only (below). 101: error status: `<0>` NXM is the only bit QUUX sets (`<3>` and `<5>`, the CADR's Unibus bits, read 0); a write of any value clears it. 102: mode, `<0>` error stop, read and written, which the host can set too. 103: the real-time clock, read only (below). 104: `RESET-DEVICES`, written, reads 0 (below). 105-107 reserved |
| 110-117 | the interval timers | 110-115 ([the clocks](#the-clocks)); 116-117 reserved |
| 120-137 | the keyboard and the mouse | 120-123 (below); 121 gives the key word in `<23:0>`, and `<31:24>` read 0. 124-137 reserved |
| 140-157 | the network | all reserved, and never reused: QUUX has no Chaosnet interface ([the network](#the-network)) |
| 160-177 | the file device | 160-171 (below); 172-177 reserved |
| 200-207 | block-disk | 200: status when read, command when written. 201: the last memory address when read, the command list pointer when written. 202: the disk address, read and written. 203: START, written; reads 0. The bits are [block-disk's](#block-disk), the done interrupt's enable command `<11>`. 204-207 reserved |
| 210-217 | the video controller | 210: mode, `<2>` black-on-white reads back, the other bits read 0 and a write of them is dropped. 211-217 reserved |
| 220-237 | the memory system | 220-227 ([the page table and the TLB](#the-page-table-and-the-tlb)); 230-237 reserved |
| 240-257 | the Ethernet device | 240-257 ([the Ethernet device](#the-ethernet-device)) |
| 260-377 | none | reserved |

**Reserved** means a read gives 0 and a write changes nothing. Everything past
main memory's end, past the frame buffer's end in the device window, and
between the buffer and the page is **nothing there**: an access fails at once,
reads 0 and sets word 101 `<0>`. The CADR's device addresses,
`17377000`-`17377777` where its display and disk registers are, and its Unibus
window, are main memory's when there is that much of it, and nothing when
there is not ([No Unibus](#no-unibus)). On the CADR the same words of its last
page, `17777400`, are inside the Unibus window, Unibus `777000`-`777776`,
where nothing answers: an access sets the Unibus NXM bit, `766044` `<3>`.

**Word 100** says who interrupted, a bit each:

| Bit | Source | Up while | Cleared at the source by |
|---|---|---|---|
| `<0>` | timer 0, the tick | its flag and 110 `<8>` | a write with `<1>` set, turning the timer off, `<8>` off, a period write, `RESET-DEVICES` |
| `<1>` | timer 1 | its flag and 112 `<8>` | as timer 0 |
| `<2>` | timer 2 | its flag and 114 `<8>` | as timer 0 |
| `<3>` | block-disk | not active and command `<11>` | a command write with `<11>` clear, START, `RESET-DEVICES` |
| `<4>` | the keyboard | a key word waiting and 120 `<8>` | reading 121 until the FIFO is empty; `<8>` off |
| `<5>` | the mouse | 123 `<0>` and 123 `<8>` | reading 122; `<8>` off |
| `<6>` | reserved: the Chaosnet interface's, which QUUX does not have | never | --- |
| `<7>` | the file device | 170 ≠ 171 and 160 `<8>` | writing 171 up to 170; `<8>` off; a disable; `RESET-DEVICES` |
| `<8>` | the Ethernet device | 251 ≠ 252 and 240 `<8>` | writing 252 up to 251; `<8>` off; a disable; `RESET-DEVICES` |
| `<31:9>` | reserved | never | --- |

Each bit is a level, and their OR is the interrupt pending that jump
conditions 5 and 6 test. So one write turns any of them off: 112 or 114
written 0, 200 written 0 with the disk idle, 123 written 0, or 160 written
back with `<8>` clear and `<0>` kept, which leaves the file device enabled.
That is what a handler does for a bit it does not serve.

**The page's convention** is for devices made for it: the enable in `<0>`
and the interrupt enable in `<8>` of the control word. Devices moved onto it
keep their own bits: block-disk the CADR disk controller's programming
interface. 160 `<8>` is taken only by
a write that enables the file device or leaves it enabled.

**How the flags clear**: a write of any value clears 101, 120 `<1>`, and 161
`<2>` and `<3>` (the last two through any write of 160); a write with the bit
set clears 110, 112 and 114 `<1>`; a read clears or consumes --- 121 pops,
122 clears 123 `<0>`.

`tests/quux_registers.rs` holds every word of the page in one table, on the
machine and through both engines' device window: its class and its value at power-on;
a write of all ones to every reserved or read-only word leaving the
machine's state byte for byte as it was; the words a read or a write of has
an effect listed apart, each with the test that holds it; word 100's
sources, each alone reading its own bit, and each turned off by one write;
and, on a QUUX of 2MW, every address of `17377000`-`17377777`, `17400000`
and `17777377` failing at once on both engines
(`the_old_addresses_answer_nothing`,
`both_engines_find_nothing_at_the_old_addresses`).

**`RESET-DEVICES`** (contract Q11): a write of word 104 with
`<0>` set resets every device, at the instant the write is taken. What the
file device had due by then runs first, as for a write of its word 160; then

| Device | What `RESET-DEVICES` does |
|---|---|
| The interval timers | every timer off, flag down, periodic, interrupt enable 0, period 0 |
| The file device | disabled, status `<2>` and `<3>` cleared (below) |
| Block-disk | command 0, its done interrupt's enable with it, and its errors cleared; not active at once, a transfer in flight ending there |
| The Ethernet device | disabled, status `<2>` and `<3>` cleared ([the Ethernet device](#the-ethernet-device)) |
| The video controller | nothing to show: it has no vertical flag and no interrupt |
| The keyboard and mouse | nothing: the FIFO, the counts and both interrupt enables are kept, since a warm boot's key word is read by the microcode's location 6 after the PROM, which writes word 104, has run |
| The real-time clock, the microsecond clock, words 101 and 102 | nothing |

Block-disk, the video controller and the file device are reset as
`PROG.UNIBUS.RESET` resets the CADR's boards (`Machine::bus_reset`).
**On QUUX `INTERRUPT-CONTROL`'s `PROG.UNIBUS.RESET`, `<36>`, drives
nothing**: the bit is written and read back through `LOCATION-COUNTER` as
the CADR's `<28>` is, and resets no device; the CADR's still resets its
boards. Nothing is held
off `SINTR` at a write of word 104: its effect is in the `SINTR` of the edge
after the one that takes it, as any register write's is. Anything may write
the word; the boot PROM writes it before it reads the disk.

`tests/quux_reset_devices.rs` holds word 104 reading 0, a write with `<0>`
clear changing nothing in the machine's state, a write of 1 leaving every
device as `PROG.UNIBUS.RESET` does from the same state and every timer
reset with the keyboard and mouse kept, what the file device had due
running first, a destination 3 write at the same edge or the next turning
no timer on, `SINTR` at the write's edge and at the next, and
`PROG.UNIBUS.RESET` resetting nothing on QUUX on both engines.

## The real-time clock

**QUUX keeps the real time in word 103 of the register page** (contract
Q9): whole seconds since 1970-01-01 00:00 UTC, Unix time, as an
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
Q9; G1 §4.4, G2 §4.3): folders of the host served to the machine under one
pathname host, `FILES`, with commands and responses in two rings in main
memory and the bytes moved by DMA. The registers carry control and the rings'
indexes; nothing polls memory for an index. `src/file_device.rs` is muir's
device.

| Word | | |
|---|---|---|
| 160 | control, read and written | `<0>` enable, `<8>` interrupt enable |
| 161 | status, read only | `<0>` enabled, `<1>` quiet, `<2>` configuration refused, `<3>` index fault, `<8>` a response waiting, `<23:16>` handles open |
| 162 | command ring base | `<27:0>` a physical word address, `<2:0>` 0 |
| 163 | command ring size | `<3:0>` the log2 of its entries, 0 to 8 |
| 164 | command producer, the processor's | `<15:0>` |
| 165 | command consumer, the device's, read only | `<15:0>` |
| 166 | response ring base | as 162 |
| 167 | response ring size | as 163 |
| 170 | response producer, the device's, read only | `<15:0>` |
| 171 | response consumer, the processor's | `<15:0>` |

**Configuration.** 162, 163, 166 and 167 are written while the device is
disabled and ignored while it is enabled. The enable, 160 `<0>` from 0 to 1,
checks them: a base off an 8-word line, a size over 8, or a ring reaching
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
| 2 | buffer A's address, `<27:0>` | handle (OPEN read or write) |
| 3 | buffer A's length in bytes | the file's length in bytes (OPEN, CLOSE) |
| 4 | buffer B's address | mtime, Unix seconds (OPEN, CLOSE) |
| 5 | buffer B's length | flags: `<0>` a directory, `<1>` on a read-only mount, `<2>` COMPLETE: an entry is exactly the completion, `<3>` and it is a directory |
| 6 | READ, WRITE: the offset in the file; DIRECTORY: the cookie | DIRECTORY: the next cookie, 0 at the end; COMPLETE: the matches |
| 7 | CLOSE: the date to set, Unix seconds | 0 |

A failed command's response is word 0 alone. A buffer starts on an 8-word
line, holds at most 65,536 bytes, and lies in main memory; byte k is bits
`8(k mod 4)+7:8(k mod 4)` of word k/4, the field holding 4 bytes. A READ of
n bytes writes the first `ceil(n/4)` words of B, the bytes past n 0, and no
other word. **Every word the device writes**, a buffer's or a response's,
is a fixnum, tag `005`; it reads `<31:0>` of what it reads, whatever the
tag.

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

**Mounts** (`--file-root`). `--file-root <folder>[,ro]` is the root `/`, a
folder holding `sys/`, `site/` and `home/<user>/`, so that one path serves
`/sys/...`, `/site/...` and `/home/lispm/...` under the band's pathname host,
`FILES`. `--file-root
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
**Reset devices disables it too** (word 104), clearing status `<2>` and
`<3>` with it; `PROG.UNIBUS.RESET` does not reach it. Power-on is
disabled.

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
behind the processor, the completion sweeps the cache, clearing every valid
bit in the cache's words / 512 clocks, and every lookup waits for the sweep
(`src/pipeline/port.rs`). Nothing completes within the producer write,
and nothing in memory changes before 170 moves. Given the same host files a
run is the same.

**Coherence.** The device takes a new 164 only once every earlier write is
answered: on `rtl` the command's time counts from when the port has answered
the last posted write, so that a command whose entry is still in the posted
writes is read whole
(`the_file_device_reads_its_entry_whole_and_the_sweep_shows_its_response` in
`tests/revision_15_rtl.rs`). `micro` posts no writes.

**A checkpoint is refused** while a handle is open or a command is queued,
saying which: a host file and a command's host effect are outside the
machine. Otherwise it carries the registers and the rings' indexes; the
mounts are the flags'.

`tests/quux_file_device.rs` holds it: a scripted driver writing commands
into the rings against a scratch folder --- each register, the refused
enable and the index fault, the due time to the nanosecond and nothing
before it, the order, the wrap at 1, 2 and 256 entries and across 2^16, a
full response ring, the interrupt, the disable and reset devices,
`PROG.UNIBUS.RESET` leaving it as it was, every command and its statuses,
the write landing whole with its temporary file never listed, the date, the
mounts, a read-only mount unchanged, names, symlinks, LOG, the checkpoint's
refusal and its round trip --- and both engines running a program that
writes the command and its producer index and reads the answer, `rtl`'s
cache swept and its posted writes waited for.
`the_file_root_is_quux_s` in `tests/cli.rs` holds the flag. **Not produced
by a test:** NMR, DAT, and a WRITE past 2^32 - 1.

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

**QUUX has no Unibus** (contract Q5). The CADR's Unibus window, its
physical pages 37000 and up, is no window on QUUX: in QUUX's 28-bit space
those are main memory's addresses when there is that much of it, and
nothing when there is not, where a read or a write fails as any empty
address does, at once, the NXM bit set in word 101, and changes nothing.
With it go, on QUUX, the I/O board (its keyboard, mouse and clocks now
QUUX's own, contracts Q1-Q3; its Chaosnet interface, serial port and
general-purpose register dropped), the bus interface's Unibus side (the
adapter, the Unibus map, its buffers, WRITE-THROUGH, the interrupt control
and error status at `766040`-`766044`, the Unibus interrupt), the
diagnostic registers as the machine's own (the spy stays the host's port:
muir's prompt, the FPGA's AXI face), and the debug cable, a Unibus master:
`--debug-cable-listen`, `--debug-cable-connect` and `--debug-in-process`
are `cadr`'s alone, and a `quux` run has no DBGIN connector. A QUUX debug
design of its own is a later contract. The CADR keeps all of it: CC and
the two-machine lashup are its acceptance test.

`tests/quux_no_unibus.rs` holds, on a QUUX of 2MW, the window's addresses
failing on the machine and through both engines' bus, the space's last
page answering as the register page and the CADR's last page not, the
Unibus interrupt not reaching QUUX, and the CADR's Unibus unchanged;
`quux_has_no_debug_cable` in `tests/cli.rs` the cable.

## The network

**QUUX has no Chaosnet interface** (QUUX's Ethernet contract, §8 R4): the
register page's words 140-157 are reserved, read 0, ignore writes and are
never reused; nothing sets word 100 `<6>`; the I/O board has no interface to
plug, and QUUX's checkpoint carries none. `quux` refuses the five Chaosnet
flags by name, and its start has no chaosnet line. The CADR's interface is on
its Unibus ([the Chaosnet](chaosnet.md)). `tests/quux_network.rs` holds
words 140-157 reserved (`words_140_to_157_are_reserved`) and word 100 `<6>`
never set (`nothing_sets_word_100_bit_6`), and `tests/ethernet_socket.rs` the
flags.

## The fused return

**QUUX can run the next macroinstruction's handler straight from a
return to the main loop** (contract H8a). Microcode 2002's main loop,
`QMLP` in `uc-macrocode.lisp:9-13`, is four microinstructions: a call on
condition 6, `M-INST-BUFFER <- MD`, `(DISPATCH-XCT-NEXT M-INST-OP OPDTB)`,
and the push of `A-MAIN-DISPATCH` back as that dispatch's XCT-NEXT. A
return that pops `A-MAIN-DISPATCH` --- `<14>` and `QMLP` --- when no
instruction fetch is needed goes to `QMLP+2`, the stream hardware's
`SPCMUNG` stepping it over the first two (`uc-macrocode.lisp:5-6`): the
dispatch and the push are two microcycles every such macroinstruction
spends there.

Functional destinations 5 to 7, which the CADR's low group leaves without
a decoder output, are QUUX's; on the CADR they write only M. Like every
functional destination they write M as well:

| Destination | Writes |
|---|---|
| 5 | the MACRO-DISPATCH register: `<13:0>` the main loop's address, `<23:14>` the A-memory address of `A-LOCALP` and `<28:24>` the M-memory address of `M-AP`, the operand address's bases, `<31>` the enable; `<30:29>` reserved, written 0 and kept 0 |
| 6 | the MACRO DISPATCH MEMORY's index, `<9:0>` |
| 7 | the entry at the index, `<17:0>`: D-MEM's word, `<13:0>` the handler's address, `<14>` N, `<15>` P, `<16>` R, and `<17>` the operand bit |

The MACRO DISPATCH MEMORY has 1,024 entries, feature word 17, indexed by
the halfword's `<15:6>`: its destination, opcode and register together.

**A fused return** is a return --- a POPJ, a dispatch whose entry has R,
or a jump with R (one switch in the code, `JUMP_RETURNS_FUSE` in
`src/machine.rs`, takes jumps out on both engines) --- that pops a word
with `<14>` set and `<13:0>` the register's, with the register enabled and
no fetch needed, so that today it would go to `QMLP+2`. When the entry for
the next halfword has R and P clear, it goes to the entry's address
instead, two microcycles sooner; the popped word stays on the stack, as
the push would have put it back, unless the entry's N is set, which would
have nopped the push. The next halfword is the one the main loop's
dispatch would take: its index is M 31 rotated by 34, 40 − 6, under LC
byte mode's addend (contract G2 appendix A1.2), by the location counter as
the pop's `NEXT INSTR` steps it in the microcycle after. The step, and a
fetch it starts, are the stream's as ever. Condition 6 is not tested, as
the main loop does not test it on this path.

It is today's return, `QMLP` or `QMLP+2`, whenever a fetch is needed (D,
[below](#the-64-bit-microinstruction), takes the fetched word), the entry
has R or P, the word is another main loop's (`DMLP`'s), the register is
disabled, or the popping microinstruction pushes, pops by functional source
14, writes M 31 or INTERRUPT-CONTROL, follows a `NEXT INSTR` the microcycle
before (`src/micro.rs:870`, `src/pipeline/exec.rs:251`), or is a dispatch
with `IR<24>` that pops by its entry's R (`advance`, `src/micro.rs:2401`,
`src/pipeline/exec.rs:709`). A dispatch with `IR<24>` whose entry has R and
P both set, so that its own POPJ pops, still fuses: `dispatch()` steps the
location counter first (`src/micro.rs:2384`, `src/pipeline/exec.rs:695`),
the return is decided after (`src/micro.rs:2942`, `src/pipeline/exec.rs:843`),
and the index is taken from the counter as the step leaves it, the halfword
`QMLP+2` would dispatch.

**The operand address.** When the entry has the operand bit and the
halfword's register, `<8:6>`, is LOCAL (5) or ARG (6) (`QADCM1`,
`uc-macrocode.lisp:138-147`), PDL-INDEX is loaded at the end of the
microcycle after the return with `A-LOCALP` + delta or `M-AP` + 1 +
delta, as `QADLOC1` and `QADARG1` compute them
(`uc-macrocode.lisp:249-257`): delta is the halfword's `<5:0>`, taken from
M 31 rotated, at `<39:34>`, and the sum is masked to PDL-INDEX's bits. The
handler's first microinstruction finds its operand at
`C-PDL-BUFFER-INDEX`. The bases are copies the machine keeps, fourteen
bits each: an A write at the register's `<23:14>` writes the copy of
`A-LOCALP`, and an M write at its `<28:24>` the copy of `M-AP`, with their
write pulse, and nothing else writes them. The register's write does not
load them from A and M memory, so the microcode writes `A-LOCALP` and
`M-AP` after destination 5. -RESET leaves the copies and drops an armed
operand address; a checkpoint keeps both copies and an armed operand
address, and its restore reads nothing from A or M memory.

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

**Microcode 2002 fills the memory itself.** At every start of the
microcode, a cold or warm boot through the PROM and a `%DISK-RESTORE`, and
before its first main-loop return, `RESET-DEVICES` reads feature word 17
and, where it is not 0, writes every entry with `OPDTB`'s entry for its
opcode, the operand bit clear; then the register with `QMLP`,
`A-LOCALP`'s and `M-AP`'s addresses, the enable and D's enable, `<30>`; then `A-LOCALP` and
`M-AP` with their own values, which loads the base copies; and then, over
the generic entries, 36 entries for handlers of its own
(`RESET-MACHINE-MACRO-DISPATCH-FILL` in `uc-cold-disk.lisp`). A
specialised handler runs only from a fused return, so only where the
entry's conditions hold; otherwise the main loop's dispatch runs the
generic one, and the two give the same results:

| Macroinstructions | Handlers (`uc-macrocode.lisp`) | Operand bit |
|---|---|---|
| MOVE to the PDL of a local or an argument | `QIMOVE-PDL-OPERAND` | yes |
| POP and MOVEM into a local or an argument | `QIPOP-OPERAND`, `QIMVM-OPERAND` | yes |
| BR, BR-NIL and BR-NOT-NIL, by the offset's sign | `QIBRN-BR-POS` and its five fellows | no |
| SETE-1+, + and < of a local or an argument, on fixnums | `QISP1-OPERAND`, `QIADD-OPERAND`, `QILSP-OPERAND` | yes |

Where a `POPJ-AFTER-NEXT`'s next microinstruction would make a write the
rule forbids, the write is made before the return, by the popping
microinstruction or the one before it with a no-op after the return, as
the stores into a local or an argument, `QSTLOC` and `QSTARG`, do
(`uc-macrocode.lisp:363-381`, the comment above `QSTLOC`).

-RESET clears the enable and nothing else, and every control-store write
clears it too, wherever it lands: the entries name control-store
addresses, and a new microcode must not run on those another left, whether
the PROM loaded it or a `%DISK-RESTORE` (which does not pass through the
PROM). Reset devices, word 104, does not touch it. A checkpoint keeps the
register, the index, the entries and the base copies.

`tests/macro_dispatch.rs` holds, on both engines, a main loop made as
`QMLP` is: the destinations' decode, on QUUX and not on the CADR; the same
state with the MACRO DISPATCH MEMORY holding the generic handlers as
without it, two microcycles fewer for each fused return that needs no
fetch, returns by a POPJ, a dispatch with R and, as the switch says, a jump
with R fused and N honoured; the entry taken by the whole `<15:6>`; each case above that is
not fused running today's path microcycle for microcycle; the operand
address for LOCAL and ARG at the handler's first microinstruction and not
before, masked, from bases written by the popping microinstruction itself,
and nothing loaded without the operand bit or for another register; the
base copies not loaded by destination 5 and written by the writes of
`A-LOCALP` and `M-AP` alone; a PDL buffer write by PDL-INDEX in the
microcycle after landing at the operand address; the rule checker's
decode, each forbidden write found in a run, and a handler's first PDL
read at the address the microcycle after the return writes found; the
generic fill's operand bit only on halfwords whose `<8:0>` is a register
and a delta (of ND4, `PUSH-CDR-IF-CAR-EQUAL` and
`PUSH-CDR-STORE-CAR-IF-CONS`, and not `PUSH-NUMBER`, whose `<8:0>` is an
immediate); the enable cleared by -RESET and by a control-store write, the
base copies kept; and the checkpoint.
`the_fused_return_takes_the_halfword_in_the_ring_of_40` in
`tests/revision_13.rs` holds the index and the operand's delta in the ring
of 40.

## Its microcode

**Microcode 2002 is QUUX's**, muir-sys's change to MIT's 323 for the 40-bit
word (contract G2 §6) and the 64-bit microinstruction. At boot, at
`RESET-DEVICES`, it reads functional source 16, and without QUUX's
signature and a hardware revision of 14 or more it halts at
`MACHINE-NOT-QUUX-14` (`uc-cold-disk.lisp`): a CADR reads all ones there. On
QUUX the type is the word's bits 3:0, 4. The band reaches its TELNET server
on it, and the herald says Microcode 2002 (`tests/system_2002.rs`).

## The checkpoint

**A checkpoint of QUUX is a 40-bit machine's, version 58**, which says its
revision in the byte where the CADR keeps its level-1 entry's width: `0o220`,
16 with `<7>` set, a value no width takes (`REVISION_16_MARK`,
`src/machine.rs`). Every word is 5 bytes, `<7:0>` first and the tag last, so
that main memory in it is packed storage byte for byte (contract G2 appendix
A1.13); the dispatch memory of 4,096 entries, the overflow flag, the location
counter's `<33:32>`, the memory system's words and the redirect's two copies
(`a_checkpoint_keeps_the_34_bit_counter_and_the_words`), the control store's
64 bits and the OA registers, word 225 and the OA select check's shadow, the
Ethernet device's state, 253-257 with it, and no Chaosnet interface on the I/O
board (`src/checkpoint.rs`). The TLB is not kept: a resume starts with it
swept. A checkpoint of the CADR is version 54, every word 4 bytes. QUUX
refuses a checkpoint of 32-bit words, the CADR's
(`a_32_bit_checkpoint_is_refused_on_revision_13` in
`tests/revision_13_memory.rs`), and `quux` refuses one `cadr` wrote at the
start, naming the executable; a CADR checkpoint keeps version 54's bytes and
resumes (`a_cadr_checkpoint_keeps_version_54_s_bytes` in
`tests/checkpoint.rs`).

The versions read are 54, the CADR's, and 58, QUUX's. A checkpoint of an
earlier QUUX hardware revision is refused as that revision's, by the machine
its body records, and not by its version alone: revisions 13 and 14 wrote
version 55 and revision 15 version 56, and `quux` and `cadr` both refuse
them, saying that this build does not run revision N and naming muir-sim's
tag `quux-revisions-13-15`, where it resumes
(`an_earlier_revision_s_checkpoint_is_refused_naming_it_and_the_tag` in
`tests/quux_revision.rs`). The 32-bit QUUX of revisions 11 and 12 wrote the
CADR's format version, and records its machine as every 32-bit checkpoint
does: a 6-bit level-1 map entry, multiply and divide, the tick and, on 12,
the fused return, where the CADR has a 5-bit entry and none of the three;
both executables refuse it by that record, saying it resumes only on an
earlier muir-sim (`src/machine.rs`, `Machine::load`). Versions 49, 50, 51
and 53, the formats of 54, 55, 56 and 57 before the Chaosnet
interface's pending state was kept after them (`docs/chaosnet.md`), are
refused by version, and so are 52, QUUX's before it had 253-257, and 57, its
version with the Chaosnet interface.

## System 2002

**System 2002**, muir-sys's band for QUUX, boots on it (`tests/system_2002.rs`,
which skips without muir-sys's hand-over, the pre-release
`handover-2002-2999694` that `tools/fetch-handover-2002.sh` fetches into the
gitignored `ref/band-2002-2999694`): microcode 2002 in the current `MCR1` and
the band in the current `LOD1` of a GPT disk in a dynamic VHD, on `quux`'s
built-in PROM 2002. Run as `quux` with the disk, the sources as the file
device's root and `--ethernet-socket`, both of the hand-over's disks, the
band's and the high band's, boot on both engines to the band's TELNET
server: a connection to port 23 at the default address is answered with the
herald, System 2002 on microcode 2002 on "QUUX on muir-sim" with the
associated machine `FILES`, and "Telnet server here". One of the runs of the
high band, on `micro`, gives the hand-over's PROM 2002 with `--prom`. PROM
2002 is the hand-over's `promh.mcr` and the sources' `sys/ubin/promh.mcr`,
byte for byte (`prom_2002_is_the_hand_over_s`).

## Main memory

**Main memory is 32MW by default**, the boards' (contract G2 §3), and
`--main-memory-size` gives it in whole megawords, 1MW to 64MW (G1 §3.2;
`tests/quux_main_memory.rs`). At 32MW muir holds 256 MiB for it, 8 bytes a
word. A checkpoint's body holds main memory 5 bytes a word, and the file
packs runs of zeros.

## The page table and the TLB

**A frame is a page-sized slot of physical memory, and a page is a page of
virtual memory.** "Frame" (page frame) is what MIT's CADR code calls a
"physical page": the `PHYSICAL-PAGE-DATA` region has a word for each one
(muir-sys `sys/sys/qmisc.lisp:40-42`), and a page entry's "physical page
(frame) number" is its `vma-phys-page-addr-part`
(`sys/ucadr/uc-page-fault.lisp:237`). "Page" alone is virtual. MIT's symbols
keep their names. The frame buffer is the video controller's memory, not a
frame in this sense.

**QUUX's virtual addresses are 32 bits** (contract G3 revision 14 and its
appendix A14): two untranslated windows, a page table in memory walked by
hardware behind a TLB in place of the CADR's two map levels, a 34-bit
location counter, and jump condition 12 ([the 40-bit word](#the-40-bit-word)).
What follows is what muir does and which tests hold it
(`tests/revision_14.rs`, every program on `micro` and `rtl`).

**The address space**, by `VA<31:28>`, the decode `VA<31:29>` = `111` for
the windows and `VA<28>` between them; `VA<39:32>` is never looked at:

| `VA<31:28>` | Virtual | What |
|---|---|---|
| `0000`-`1101` | `0`-`33777777777` | paged, through the TLB |
| `1110` | `34000000000`-`35777777777` | the device window |
| `1111` | `36000000000`-`37777777777` | the physical memory window: main memory at physical `VA<27:0>`, past its end nothing there |

In the device window, frame buffer 0 from its base holds the field, a write
dropping the tag and a read giving `005`; A memory's window,
`35700000000`-`35700001777`, faults on every reference; the register page
is at `35777777400` ([the register page](#the-register-page)); the rest is
reserved and nothing there, reading 0 and setting word 101 `<0>`
(`the_device_window_and_its_register_page`,
`nothing_there_past_main_memory_and_in_the_reserved_slices`,
`a_memory_s_window_faults_with_status_7`). A window address never consults
the TLB and never walks (`the_windows_never_touch_the_tlb`), and the
physical memory window aliases a translated frame coherently, setting no
bit in its entry (`the_physical_memory_window_aliases_a_translated_frame`).
A reference goes out on a 29-bit bus address, which is the cache's key:
`<28>` clear and the physical word for main memory, `<28>` set and
`VA<27:0>` for the device window.

**The page table.** Register-page word 220 holds the directory's first
frame, a multiple of 4; 0 is no directory. A miss on a paged address walks:
the directory entry at `{base<17:2>, VA<31:20>}`, present when its status
`<26:24>` is 4 and its frame `<17:0>` is in main memory, then the page entry
at `{frame, VA<19:10>}`. What it finds is loaded into the TLB unless it is
no entry: no directory, a missing directory entry, a page entry of status 0,
one of status 7 (A memory's window's, never in a table), or one in core
whose frame is past main memory's end. A not-in-core entry, status 1, is
loaded; the reference faults, and `MAP(MD)` then reads it with no further
walk. A no-entry reference faults, and the next one walks again. A walk
never faults and reads memory by physical address, a word past main memory
reading 0 (`no_entry_faults_and_loads_nothing`,
`a_walk_with_no_directory_reads_nothing`,
`a_not_in_core_entry_faults_and_map_md_reads_it_without_a_walk`). Faults
are the entry's access code, `<27>` read and `<26>` write.
**The PDL buffer redirect** (A14.7) takes a read or write start through
a TLB entry of status 5 whose access code faults it; one written directly
with access `11`, the microcode's fiddle, does not fault and goes to memory
(`an_access_11_status_5_entry_goes_to_memory`). With PP the PDL buffer
pointer, n = (PP − head + 1) AND 37777 and off = (`VA<31:0>` − base) mod
2^32, unsigned; when off ≤ n, the word one past PP admitted as PGF-R-PDL
admits it, the reference is inside the buffer: a read's word is the
buffer's at (head + off) AND 37777, and a write's word, `MD` as its start's
own microcycle leaves it, goes there, with no memory cycle, no write-back
and no setter. Otherwise it goes to memory as if its access
code were `11`, with its write-backs (`the_redirect_takes_the_buffer_up_to_the_word_past_pp`,
`the_redirect_across_2_31_words`). The base and the head are copies of A
memory's 430 and 431, A-PDL-BUFFER-VIRTUAL-ADDRESS and A-PDL-BUFFER-HEAD,
taken in A memory's write pulse, so a start in the microcycle after a write
of A 431 uses the new head
(`a_start_right_after_a_write_of_a_431_uses_the_new_head`). `MAP(MD)` of a
status-5 page reads the entry as it
is held. A port-B lookup that misses fills the one TLB too, so a `MAP(MD)`
or a pointer-typed map-bit dispatch on a page sharing the fiddle's index
evicts the fiddle, and the next write to the status-5 page walks, takes the
table's access `01` and is redirected into the buffer, while a dispatch on
a fixnum looks nothing up and leaves it
(`e1_a_port_b_fill_evicts_a_status_5_fiddle_and_the_write_is_redirected`).
At status 6, the MAR's, the same eviction makes the write fault, the
redirect serving status 5 alone
(`e2_a_port_b_fill_evicts_a_status_6_fiddle_and_the_write_faults`).

**The write-backs.** A reference to a paged address that does not fault
sets bits in its page's entry: accessed `<28>` by the first read, write
or fetch through an entry with it 0; modified `<29>` by the first write;
and ephemeral-reference `<19>` by a write when the enable, word 221 `<0>`,
is 1, the word written has its data type in the pointer-type register and
`<31:28>` = `1101`, and the entry has it 0 (A14.8). `MAP(MD)`'s and a
dispatch's walks set nothing, nor does a window's address. The bits are
ORed into the TLB entry at once, and into the table by one write-back a
reference, ahead of the reference's own cycle: the directory entry and the
page entry are read again, and the page entry written with the bits ORed
in, only if it is in core, status 2 to 6, with the frame of the TLB entry
the reference went through. Otherwise nothing is written and word 224
counts one. The write is an OR, so the table keeps its own bits where a TLB
entry written directly differs from it (`accessed_is_set_by_the_first_reference_that_does_not_fault`,
`modified_is_set_by_the_first_write_and_the_table_keeps_its_bits`,
`the_guard_refuses_a_write_through_a_stale_entry`,
`the_setter_marks_a_store_of_an_ephemeral_pointer`). On `micro` a read's
write-back is made at its start and a write's as it goes out, with the word
it writes. On `rtl`, the pipeline, a start's walk and its write-back are
holds of WB ahead of the port's grant (`src/pipeline/mod.rs`), and the
table reads they make go through the port and the cache like any read
(`src/pipeline/port.rs`).

**The TLB** is direct-mapped, 4,096 entries, on both engines; the start says
it. The index is `VA<21:10>` and the tag `VA<31:22>`, and its contents are
modelled: two pages that
share an index evict each other, and an entry changed in memory keeps
translating to the old frame until it is invalidated
(`two_pages_sharing_an_index_translate_to_their_own_frames`,
`pages_either_side_of_2_31_translate_to_their_own_frames`,
`a_stale_entry_is_kept_until_it_is_invalidated`). A store to `WRITE-MAP`
is an operation, landing in the microcycle after it: `VMA<33:32>` 1 a
direct write of `VMA<29:0>` at `MD`'s index and tag, 2 an invalidation at
`MD`'s index whatever its tag, 3 an empty; 0, the CADR's map-write word,
and any operation at a window address do nothing
(`a_direct_write_loads_and_an_empty_clears`). An empty, and `-RESET`, sweep
the TLB: at once on `micro`, and on `rtl` one entry a clock, a memory start
or a `MAP(MD)` or map-bit dispatch lookup waiting for the sweep's end
(`an_empty_sweeps_the_tlb_one_entry_a_clock` in `tests/revision_15_rtl.rs`).
A port looks an address up once a microcycle, and port B again when `MD`
changes while the microcycle waits, as when a dispatch on `MD` waits for a
read: a miss then walks, counted, filled and timed as any other
(`a_port_b_lookup_walks_for_the_md_that_lands_during_its_wait`). As a test
aid only, `Tlb::small_for_tests` builds a TLB of any power of two down to 1
entry, where every fill evicts
(`a_1_entry_tlb_runs_e1_s_control_and_evicts_on_every_fill`). The profile
harness's `tlb:` line counts, as the model's figures and not the
hardware's, the walks, the sweeps, the write-backs and the redirects, the
directly written entries a fill replaced before their invalidation, by port
and by the replaced entry's status, and the microcycles in which port A and
port B both missed at one index, on `micro` port A's miss at a start paired
with port B's in the next instruction
(`a_double_miss_at_one_index_and_a_port_a_eviction_are_counted`); a second
line counts the walks that waited for a cycle in flight, by port, with the
time they waited, and the port-B walks made while the microcycle waits for
`MD` (`examples/profile.rs`).

**`MAP(MD)`** reads `<39:32>` 0, the fault bits `<31:30>` of the last memory
cycle's entry as before, and `<29:0>` the entry for `MD`'s address, looked
up whatever `MD`'s type and walked for on a miss; a window's address reads
its fixed entry, `11` and `1460` with the frame `VA<27:10>` for the
physical memory window and 0 for the device window, `11` and `0760` for A
memory's window, and no entry `00` and `0060` (`map_md_reads_the_entry_and_the_fixed_entries`).
**A dispatch on map bits** looks up `MD`'s page only when `MD<37:32>`'s
bit is set in the pointer-type register, words 222 and 223; otherwise its
map bits read 1 and 1, with no lookup (`a_transport_on_a_fixnum_walks_nothing`).

**The location counter** is `LC<33:0>`, read with NEED-FETCH in `<39>` and
the four flags in `<37:34>`. Destination 1 writes `<31:0>` from the bus and
`<33:32>` from the word, except for an arithmetic ALU function through the
ALU or the left shift, which take them from LC's adder: M's `<33:32>`, plus
A's sign twice, plus the carry into bit 32, and under the left shift that
sum shifted. The stepper counts over all 34 bits, and a fetch takes
`LC<33:2>` (`lc_s_adder_carries_branches_across_2_30_and_2_31_words`,
`lc_rebuilt_from_a_relative_pc_and_an_fef_address`,
`qlenx_s_shifted_write_takes_the_sum_s_carry`,
`the_stepper_carries_and_a_fetch_reads_lc_33_2`).

**The register page** gains the memory system's words, 220-227: 220 the
directory base, 221 `<0>` the ephemeral-reference enable, 222 and 223 the
pointer-type register, types 0-31 and 32-63, all read and written; 224 a
count of the write-backs the guard refused, which a write clears; 225-227
reserved.
`-RESET` clears them, `RESET-DEVICES` does not
(`the_memory_system_words_read_back_and_set_the_directory`). The feature
page says 0 in word 1 (no level-1 map), the TLB's entries in word 2, and
the buffer's device-window address, `34000000000`, in word 13.

**The `.mcr`'s section 6** (A14.13) names the hardware revision of the
microinstruction: code 6, start 0, count 1, then one 32-bit word, and it is
the file's first section, in microcode and boot PROM alike. QUUX loads files
whose section 6 says 15; a file whose section 6 says another revision is
refused, saying "section 6 says hardware revision N, and QUUX loads files
whose section 6 says 15", and so is microcode without one. `quux --prom`
holds a boot PROM to the same rules, except that a PROM without section 6 is
not refused for it (`Mcr::check_revision` in `src/mcr.rs`). The CADR reads
no section 6 and refuses a file that has one.

## The 64-bit microinstruction

QUUX's microinstruction is 64 bits (contract G3 revision 15 and its appendix
A15b). It runs on `micro`, the specification, and on `rtl`, which on QUUX is
its four-stage pipeline ([below](#rtl-is-the-pipeline)). Its boot PROM, PROM
2002, is built in; a resume takes the checkpoint's. What holds it is
`tests/revision_15.rs` and, for `rtl`, `tests/revision_15_rtl.rs`, on
hand-built programs and files.

**The microinstruction is 64 bits**: MIT's 48 in `IR<47:0>`, meaning what
they mean on every other machine, and the extension in `IR<63:48>`, read by
the class `IR<44:43>`. On a JUMP `<48>` is the hint bit; on a DISPATCH
`<61:48>` the predicted target's address and `<62>`, `<63>` its P and R
inverted; on an ALU or BYTE word `<48>` says a PDL address field is
present, `<50:49>` its base (`M-AP`, `A-LOCALP`, the PDL pointer,
PDL-INDEX) and `<58:51>` its signed displacement. An all-zero extension is
the 48-bit word. The hint and the predicted target are predictions for a
pipeline and change no result; `micro` executes `IR<47:0>` alone
(`the_extension_changes_no_result`). The PDL address field says that the
word writes PDL-INDEX with its base, as the word finds it, plus the
displacement, AND 37777: `micro` forms that index before the word, `M-AP`
and `A-LOCALP` from the machine's copies of them ([the fused
return](#the-fused-return)), and after the word holds PDL-INDEX to it; a
mismatch halts the run at PDL-FIELD-MISMATCH, `Halt::PdlFieldMismatch`,
the word committed (`a_wrong_pdl_field_halts_at_pdl_field_mismatch`). The
pipeline forms it the same way in EX, a write of `M-AP` or `A-LOCALP` by
the word before, still in WB, included, as the word's own operands take it
(`a_pdl_field_reads_a_base_the_word_before_wrote`). The
control store and the PROM hold 64-bit words.

**The OA registers replace IMOD** (A15b.15). Destinations 16 and 17 load
OA-REG-LOW, 26 bits in `IR<25:0>`'s positions, and OA-REG-HIGH, 22 bits in
`IR<47:26>`'s, at the end of the word, writing M as every functional
destination does; each keeps its word until the next write, `-RESET`
clears both, and no source reads them. Nothing ORs them into the next word,
as IMOD does on the CADR: a word reads one only
through its OA select, SL `IR<60>` for OA-REG-LOW and SH `IR<61>` for
OA-REG-HIGH (`an_oa_write_leaves_a_word_without_a_select_alone`,
`a_select_takes_a_value_written_three_words_before`,
`reset_clears_the_oa_registers`). A select ORs its register into nine
fields (`each_of_the_nine_fields_is_taken_through_its_select`):

| Select | Class | Fields |
|---|---|---|
| SL | ALU | the A destination `<23:14>` if `IR<25>` is 1, the M destination `<18:14>` if 0; the ALU function `<6:3>` |
| SL | BYTE | the destination as ALU's; the rotate `<5:0>`, the length − 1 `<11:6>` |
| SL | JUMP | the address `<25:12>`: the target, or `WRITE-I-MEM`'s address |
| SL | DISPATCH, a dispatch-memory write only | the address `<23:12>` |
| SH | ALU, BYTE, JUMP | the A source `<41:32>`; the M source `<30:26>` when it is M memory, `IR<31>` 0 |

A dispatch that transfers takes no select: its `IR<61:48>` is the
predicted address, so `<60>` and `<61>` are that address's `<12>` and
`<13>`, and both engines read them so whatever the address
(`a_dispatch_predicting_an_address_above_7777_takes_no_select`).

A register bit that is set neither in the word nor in its class's fields
halts the run before the word commits, at OA-OUTSIDE-FIELDS,
`Halt::OaOutsideFields`, where the CADR would run a word outside its
fields; a bit already in the word does not
(`oa_outside_fields_fires_on_a_bit_outside_and_not_on_one_in_the_word`).

**The OA select check** keeps on `micro` a shadow of IMOD's pending flags:
a write of destination 16 or 17 sets its flag, the next microcycle spends
it, and a nopped one drops it, as IMOD's is dropped. The run halts when a
word selects a register the word executed before it did not write,
`Halt::OaSelectWithoutWrite`, and when a word does not select a register
the word before it wrote, `Halt::OaWriteWithoutSelect`. Counting executed
words, it honours N: a write in the slot of a taken transfer with N is
nopped and asks for no select, and a write in the slot of one not taken is
followed by the fall-through (`the_shadow_check_halts_on_each_breach`). It
sees computed targets as they run. It is on by default, `Micro`'s
`oa_select_check`, and a checkpoint keeps the shadow, so that a halt or a
resume between a write and its select runs on as the run would have
(`a_halt_and_a_checkpoint_between_a_write_and_its_select_resume`).

**D, the dispatch from the fetched word** (A15b.9; the D-timing ruling).
The MACRO-DISPATCH register keeps `<30>`, D's enable, with `<31>`. A return that [the fused return](#the-fused-return) would fuse but
for a needed fetch goes to the main loop, `QMLP`, as today, and where both
enables are set, the popped word is the main loop's and condition 6's
interrupt and sequence break are false, D's wait begins at the return. The
return commits as any return does, popping the stack; its delay slot runs
as on the single-edge machine, its end stepping LC and starting the
stream's fetch, and it sees the stack popped
(`d_s_slot_sees_the_stack_popped`). D decides on that fetch's own word:
rotated by LC as stepped, its halfword's entry, with no R and no P, makes
the next word the entry's handler, keeps the popped word unless the entry's
N is set, loads PDL-INDEX with the operand address and M 31 with the word,
all at once; an entry with R or P leaves `QMLP`. A fetch that faults, or
that reads anything but main memory, ends the wait there, to `QMLP`, whose
first word takes condition 6 (`a_faulting_fetch_ends_d_s_wait`). So a word
stored into the next macroinstruction word by the word before the return,
by the return or by its slot is the one D dispatches on, as `QMLP` would
(`d_decides_on_the_fetch_s_own_word`). A fetch started before the return
and still in flight, its word arriving during the wait, is not D's: D
waits on for its slot's fetch, so that the halfword is the one LC as
stepped names (`d_decides_on_its_slot_s_fetch_not_an_earlier_one`, the
main loop's sequence-break path). `micro` decides at the end of the
slot's microcycle, on the word its fetch reads. The pipeline's return
squashes the words behind its slot and holds CS, with no redirect, so the
slot runs on; D decides in the clock its word is in `MD` and CS loads the
handler at that clock's end, nothing of D's effects showing before
(`d_s_effects_wait_for_its_word`): a return taken by D to its handler's
commit is 7 clocks on a hit (`d_s_slot_does_not_wait_in_rd`). A halt
once the slot has entered EX drains through D's decision; one that squashes
the slot keeps D's wait and fetches the slot again. With `<30>` clear D is
held off, microcycle for microcycle
(`d_dispatches_a_return_that_needs_a_fetch`,
`d_s_enable_clear_is_d_held_off_microcycle_for_microcycle`,
`condition_6_a_faulting_fetch_and_p_go_to_the_main_loop`,
`m31_and_the_operand_address_after_d_are_a_fused_return_s`). A fused
return's operand address over a base the word before it writes clears RD's
mark on its index copy as the address loads
(`an_operand_address_over_a_base_the_word_before_writes`). Over a base the
popping microinstruction itself writes, the address is formed from the new
base, as on `micro`: RD counts that write as a base still being written,
so that RD's words wait for the index, and EX forms the address with the
write, then in WB, forwarded, as the PDL address field takes a base
(`a_handler_reads_its_operand_over_the_return_s_own_base_write`,
`the_pipeline_s_operand_address_needs_both_halves_of_the_forward`).

**CMD_PROD** is taken once every write before it is answered (A15b.5).
`micro` posts no writes: a write goes out by the end of the microcycle
after its start, and a start right after it waits for it, so a command's
entry written before the producer index is in main memory when the index
is taken (`cmd_prod_is_taken_after_every_earlier_write`). `rtl` posts its
writes, and holds the producer index's write until its port is empty.

**A register write's interrupt** is taken by the next word that tests it,
not the word right after the start. A write to a device register goes out
at the edge ending the microcycle after its start, as the single-edge
contract has every cycle go out, so that word's conditions see the
interrupt as it stood before the write; `micro` sends a register write out
then, sooner only when a start in that microcycle or a
halt sends it, and keeps its shortcut, the end of the start's own
microcycle, for main memory alone. The register takes the start's word
whenever it goes out (`a_register_write_carries_its_start_s_md`). A write of block-disk's command with
`<11>`, the disk idle, raises word 100 `<3>`: the page-fault-or-interrupt
check right after the start does not call, the one after it does; a write
that lowers the level is still seen up by the check right after it
(`a_register_write_s_interrupt_is_seen_by_the_check_after_the_next`,
`a_register_write_that_lowers_the_level_is_seen_by_the_check_after_the_next`).
Under neutral time the device sees the write at its start's instant
(`under_neutral_time_a_device_sees_a_write_at_its_start_s_instant`).

**`WRITE-I-MEM`** writes `IWR<63:32>` from `A<31:0>` and `IWR<31:0>` from
`M<31:0>`: a word with `<63:48>` set is written whole, and a tag left in
`A<39:32>` reaches no bit of it, where the CADR takes `A<15:0>`
(`write_i_mem_writes_64_bits_and_no_tag`).

**Register-page word 225** counts the
posted writes answered with an error; a write clears it, and so does
`-RESET`, as word 224. `micro` posts no writes, so it reads 0 there unless
a count is planted (`word_225_reads_the_errors_and_a_write_clears_it`);
`rtl` counts its port's error responses (`word_225_counts_error_responses`).

**Its `.mcr`** is self-describing (A15b.7): little-endian 32-bit words, the
format word `0x51550001` (MACHINE-ID's signature over the format number 1)
and the number of sections; each section an 8-word header --- the type,
the number of items, the actual width in bits, the storage width in bits,
the start address, and three parameters --- and then exactly its items, an
item least significant word first; then zeros to a whole number of
1,024-byte blocks. The types, in this order: 6 the hardware revision,
first, one 32-bit item at 0; 1 the control store, 64 bits; 2 the dispatch
memory, 18 bits, `<17>` odd parity; 3 the microcode symbol area, 40 bits
at the physical address of its first word; and 4 A memory, 40 bits, last.
`mcr::parse_15` reads it, and the PROM's own file, whose control store
section starts at 36000. It refuses a format word other than `0x51550001`;
a first section other than type 6, or of another shape; an unknown type
(0, 5 and every type above 6), a type twice, the order broken; an actual
width other than the machine's for the type; a storage width not a
multiple of 32 or below the actual width; a padding bit set in an item; a
non-zero parameter word, none being used; a section past its memory, a
microcode's control store at or past 36000, a PROM's not starting at 36000
or past its 2000 words; sections running past the file's end, a non-zero
word after the last section, and a length that is not a whole number of
blocks (`every_mcr_refusal_on_a_planted_file`, against
`a_revision_15_mcr_reads_whole`). **The format is one machine's**: a QUUX
file is refused on the CADR, which reads MIT's sections, and MIT's sections
on QUUX, the refusal naming the format word the file lacks
(`each_format_is_refused_on_the_other_machine`); a QUUX file whose section 6
says a revision other than 15 is refused
(`quux_loads_an_mcr_whose_section_6_says_15_and_refuses_others` in
`tests/quux_ethernet.rs`). The symbol area is refused when its start or extent passes the largest
main memory, 64MW, and `Mcr::check_main_memory` holds it to a machine's own
(`the_symbol_area_lies_in_main_memory`).

**The console** reads `IR<63:48>` at spy register 3, `SPY-IR-EXT`, after
`IR<15:0>`, `<31:16>` and `<47:32>` at 0 to 2, and write strobe 6, with
its alias 14, loads the debug IR's `<63:48>`, `-LDDBIRX`; a console writes
all four halves. On the
CADR register 3 reads open and strobe 6 loads nothing
(`spy_register_3_and_write_strobe_6_are_ir_s_extension_on_quux`).
The debug IR's word, all 64 bits, runs in place of the word at PC when a
step is made with `IDEBUG` up, on `micro` and on the pipeline alike
(`the_pipeline_runs_the_debug_ir_s_64_bits_as_micro_does`).

### `rtl` is the pipeline

On QUUX `rtl` is `muir::pipeline::Pipeline` (contract G3 revision 15, §5, §6,
§9, §12.1; A15b.2-A15b.6, A15b.13), four stages of one clock each. Time is
[QUUX's clock](#quuxs-clock). CS reads the control store and, at its end, the
word's A memory and PDL buffer operands; RD reads M, keeps speculative copies
of the micro stack's top and pointer, LC, the PDL pointer and index, and
chooses the next address; EX runs the microcycle as `micro` does and checks
RD's choices; WB writes A, M and the PDL buffer and grants the word's memory
starts. Programs at random, the main loop's paths, D and the fused return end
as on `micro` microcycle for microcycle (`random_programs_end_as_on_micro`,
`the_main_loop_s_paths_end_as_on_micro`,
`d_and_the_fused_return_end_as_on_micro`).

**Transfers.** Jumps, calls, POPJ and the fused return resolve in RD; a
conditional jump is predicted by its hint and a dispatch by its predicted
target, P and R, and EX checks the address and the prediction itself. A
wrong one squashes the words behind the delay slot and restores RD's copies
from the committed state: one bubble. The delay slot, when RD holds it, waits
there a clock and plans from the restored copies in the next, so that its
plan never follows EX's outcome in the clock EX decides it: the bubble moves
ahead of the slot and none is added, the word after the slot committing at
the same clock as when the slot plans at once
(`p2_the_slot_waits_a_clock_in_rd_after_a_wrong_prediction`). A restore of
RD's copies without a redirect, after a transfer whose delay slot N nops,
changes no value a word RD plans from in that clock: it changes the stack's
top after a dispatch call whose entry sets N, the pushed return being the
word after the dispatch, while RD's word then is the slot N nops; the
pipeline counts both, and `Pipeline::assert_restore` stops on the second
(`the_speculation_matrix_ends_as_on_micro`,
`random_programs_end_as_on_micro`). `Pipeline::bubbles`, `MUIR_BUBBLES=2` in
the profile, gives a wrong prediction A15b.14's fallback of two bubbles, the
redirect a clock later, so that the target, or the slot RD nopped on the
prediction, is fetched a clock later and the slot does not wait in RD;
the programs end alike (`two_bubbles_fetch_the_target_a_clock_later`). A
delay slot N inhibits is nopped and counts once in `Machine::cycles`
(`conditional_jumps_hinted_each_way_end_as_on_micro`,
`dispatches_predicted_each_way_end_as_on_micro`,
`the_speculation_matrix_ends_as_on_micro`,
`a_squash_restores_rd_s_lc_and_stack_copies`, `a_nopped_slot_counts_once`).

**Holds.** RD holds a return a clock when a word just before writes what it
reads (`each_case_of_the_guard_on_planted_pairs`); CS holds a word that
selects OA-REG-HIGH 2, 1 and 0 clocks behind a writer 1, 2 and 3 words
before (`the_oa_high_hold_is_2_1_0_clocks`); DIV takes 18 clocks in EX and
MUL 5 (`div_stays_18_clocks_in_ex_and_mul_5`); a start right after a map
write waits a clock and translates through the new entry
(`a_start_after_a_map_write_translates_through_the_new_entry`). A page
fault check after a start that faults is held a clock and squashes late,
two bubbles, the delay slot as N says
(`the_late_squash_after_a_faulting_start`,
`the_late_squash_costs_two_bubbles`). A check samples the interrupt in its
last clock in EX: raised at any clock around a check, held on `MD`, behind
a wrong prediction or after a faulting start, the run ends as `micro`'s
raised before the matching microcycle
(`an_interrupt_at_every_clock_around_a_check`). A squashed word makes no
start and no TLB lookup (`a_squashed_start_is_caught`,
`a_wrong_path_map_md_evicts_nothing`). The PDL buffer's word written by a
microcycle is read old by the next and forwarded to the two after, across
the buffer's wrap (`the_pdl_buffer_across_its_wrap`). A write redirected
into the buffer lands at its grant, as it lands on `micro` when its start's
microcycle ends, and the words behind the start read it, the next among them
(`a_write_redirected_into_the_pdl_buffer_carries_its_start_s_md`).

**The single-edge contract's rows** hold on the pipeline, each against
`micro` with its own fault planted (`row_*` in `tests/revision_15_rtl.rs`):
a PDL read through a pointer the word before wrote waits a clock for it; a
write by PDL-INDEX lands at its own word's index; an M read of the SPC
stack right after a push reads the old word and the next word the new,
while a POPJ right after a push returns to the word pushed; `MAP(MD)` right
after a map write reads the old entry; a dispatch right after a
dispatch-memory write takes the new entry, its prediction checked against
it; a word `WRITE-I-MEM` writes runs as written, the words behind it fetched
again from the word after it in execution order, the jump's target when it
fills a delay slot (`write_i_mem_goes_on_at_the_word_after_it_in_execution_order`);
under the OA select check `micro` halts on a `WRITE-I-MEM` outside MIT's
form, `IR<9:0>` 1647 without POPJ, or run as a delay slot, the WRITE-I-MEM
check (a proposed name;
`the_write_i_mem_check_halts_outside_mit_s_form_and_in_a_slot`). **A
write carries `MD` as its start's own microcycle leaves it**, whatever the
word after it loads, the single-edge rule's "the `MD` of the microcycle
after its start" left by QUUX (the MD-after-write ruling, contract
amendment 10): the pipeline's start takes its word at EX's commit and
carries it to the port's queue, the register's write or the PDL buffer, and
`micro` takes it at the end of that microcycle; a halt anywhere then writes
what the run without it writes (`row_a_write_carries_its_start_s_md`). The
word right after a read start reads `MD` as the start found it and waits
for nothing, a write of `MD` there giving way to the read's word, whether
or not that word starts a cycle, as the cycle goes out at the edge ending
that microcycle; a write it starts is held behind the read and carries the
read's word (rule (A) of the MD-after-read ruling;
`row_a_write_right_after_a_read_start_writes_the_read_s_word`); every
later word that uses `MD` waits for the word read
(`row_the_word_after_a_read_start_reads_the_old_md`). Under the OA select
check `micro` halts on a word that writes `MD` (destinations 30 to 33, 34 to
37 decoding as those) in the microcycle right after a memory start or an
instruction fetch's start, the MD-after-start check, which the
micro-assembler's refusal of the same words matches
(`the_md_after_start_check_halts_micro`). Its port-B
lookup, `MAP(MD)` or a map-bit dispatch, is of that `MD` too, however long
its walk takes and whenever the read's word lands, and a walk keeps the
address it began with to its fill: the TLB then holds what `micro`'s holds,
and no page index is read under another address's directory entry
(`row_port_b_right_after_a_read_start_walks_the_md_it_reads`,
`a_walk_keeps_the_address_it_began_with`). Port B uses its walk's fill a
clock after it lands, so that no table word reaches EX's operand in the
clock it is read (`p1_port_b_uses_its_walk_s_fill_a_clock_later`).

**The memory port** posts writes: a queue of 8 entries, freed when the port
accepts a write, and 16 accepted writes in flight until their responses;
main memory takes a write's word at its response. Writes are answered in
order, at most one a clock, as one AXI ID for every write answers them: a
run's are so already, and the test memory that answers a write 1 to 50
clocks late is held to it (`p3_writes_are_answered_in_order_one_a_clock`).
A write to a word of the frame buffer's window whose line the cache holds
leaves in the line what the window will hold, the field with tag 005, as a
fill reads it (`row_a_cached_frame_buffer_word_written_reads_back_as_the_window_holds_it`). A read whose line has a
write queued or in flight waits for every such response, and every other
read goes ahead (`the_read_rule_returns_the_word_written`,
`the_ninth_write_waits_for_the_first_s_acceptance`,
`the_seventeenth_write_is_not_issued`). A write is accepted when its address
and its first data beat are taken, and answered the write's time after
that. A packed word of main memory, five bytes from byte 5w, is two data
beats when it runs past its first 8-byte beat, at byte offsets 4 to 7: half
of all words, a word that crosses 4 KiB among them; a word of the frame
buffer's window is one. The write channel takes the next accept no sooner
than the occupancy or the last write's beats, in clocks, whichever is
later: a clock more behind a two-beat write at 10 ns with the `kria`
timing and at 20 ns with `arty`, where the occupancy is one clock, and
nothing at 8.5 ns or with `de25` at 15 ns. The profile counts the two-beat
writes and the accepts their second beats delayed, with the clocks
(`a_two_beat_write_holds_the_next_accept_by_its_second_beat`,
`a_two_beat_write_is_answered_from_its_accept`,
`consecutive_words_written_at_10_ns_wait_for_the_second_beats`). The cache, 64K words by default,
two ways of 8-word lines that hold the words, answers a hit two clocks
after the grant and a miss when its fill lands; a write behind its line's
fill waits for it (`a_hit_lands_two_clocks_after_its_grant_and_a_miss_its_fill_later`,
`a_write_behind_a_fill_lands_after_it`). `--memory-timing` takes
`<read>,<write>,<occupancy>` in ns or `kria`, `arty` or `de25`, `kria` by
default, each turned to clocks of the period rounded up. A fill is never
shorter than the port can do, at any period and any timing: a line of
main memory is 5 beats on the 64-bit port, the first a clock after the
address at the soonest, and the fill lands the clock after the last, so a
fill under 6 clocks is held to 6; 247 ns at 50 ns, 5 clocks, is 6, and
`arty`'s 290 ns at 50 ns, 6, is its own
(`a_fill_is_never_shorter_than_the_port_s_beats_and_its_landing`). Block-disk's START lands every posted write first, so a transfer
reads every word written before it and a read's words are not overwritten
by an older write (`block_disk_s_start_lands_the_posted_writes`); the
transfer clears the cache's sets of the words it writes and no others
(`a_block_disk_transfer_clears_the_sets_it_writes_and_no_more`). A register
access waits at WB while a register write before it is still to be taken,
so that a read right after a write reads the device as the write left it
(`a_register_read_right_after_a_register_write_reads_it_written`). A
register read takes its word and makes its change at the grant (121's key
word taken, 122's clear of 123 `<0>`), but an interrupt level the change
lowers reads as it stood until the clock after the word right after the
read's start leaves EX, as a register write is taken: that word's
conditions see the level as before the read, however late it reaches EX
--- behind a wrong prediction with two bubbles, or held in CS --- and the
next word that tests the interrupt sees it lowered. No register access is
granted in between, so word 100 and a read started right after see the
change. `micro` counts it at the end of the microcycle after the start, or
sooner at a start in that microcycle, a halt or a harness boundary; the
pipeline's halt counts it as it drains
(`a_register_read_s_side_effect_is_seen_from_the_clock_after_its_grant`,
`a_register_read_in_a_mispredicted_slot_holds_the_level_for_the_target`,
`a_register_read_holds_the_level_for_the_word_after_its_start_held_in_cs`,
`a_boundary_between_a_register_read_and_the_next_word_digests_alike`,
`a_register_read_right_after_a_read_of_121_finds_the_key_taken`). A halt
between a register access's start and the next instruction sends the access
out, a write taken and a read's change counted, so that instruction sees the
interrupt level as the access leaves it, as on the CADR
(`a_halt_between_a_register_access_s_start_and_the_next_instruction_sends_it_out`).
The file device takes CMD_PROD once every earlier write is answered, so a command
whose entry is still in the posted writes is read whole, and its
completion's sweep leaves no line of what it wrote in the cache
(`the_file_device_reads_its_entry_whole_and_the_sweep_shows_its_response`).

**The halt** completes the words in EX and WB, squashes CS and RD, and
waits for the port to empty. The state is then a single-edge machine's
between two microcycles: the last microcycle's PDL buffer write lands after
the next word's read, as `micro`'s does. A halt at every clock of a run,
and a checkpoint taken there, run on to the same end, the state at the halt
`micro`'s at the same `Machine::cycles`; so does a microcode single step,
one microcycle each (`a_halt_at_every_clock_resumes_to_the_same_end`,
`a_halt_mid_burst_loses_no_write`, `a_single_step_runs_one_microcycle`).
The exception is a halt between a register access's start and the next
instruction, which sends the access out, so that instruction sees it; both
engines agree at every halt point
(`a_halt_between_a_register_access_s_start_and_the_next_instruction_sends_it_out`).
The squashed words are fetched again in order, the first two at the
addresses committed words chose for them: a lone squashed word is followed
by the address the next fetch would have taken
(`a_halt_that_squashes_one_word_keeps_the_address_after_it`). A D that
waits for the stream's word keeps waiting over the halt, its delay slot,
which makes the fetch, fetched again ahead of the wait
(`a_halt_keeps_d_s_wait_for_the_stream_s_word`); on the main loop's
programs, D on and off, a halt at every clock ends as the run without it
(`a_halt_at_every_clock_of_the_main_loop_machines`).
A write's word is fixed as its start's microcycle ends, so a halt at any
clock writes the word the run without it writes, on `micro` halted after
any microcycle as well (`row_a_write_carries_its_start_s_md`,
`row_a_write_right_after_a_read_start_writes_the_read_s_word`). The checkpoint records the period, the
port's timing and the cache's size, and a resume at another is refused,
naming the flag that matches it
(`a_checkpoint_refuses_another_period_timing_or_cache`). A checkpoint
written from the command line, at `--stop-after`, `--stop-at`, a quit or
the prompt's `checkpoint`, is taken as the harness's action point is: the
pipeline halts after the last word counted, drains and keeps RUN, so that a
run held at the prompt goes on from there. Stopped where words are in the
stages and writes queued, in flight or a read on its way, it digests as
`micro`'s at the same microcycle, and each resumed and run on digests alike
again (`an_rtl_stop_on_revision_15_drains_before_its_checkpoint`,
`the_prompt_s_checkpoint_on_revision_15_s_rtl_drains_and_goes_on`).
`HALT-CONS` under `ERRSTOP` halts the pipeline after its own word, as
`micro` halts: the word in EX not yet run is squashed with RD's and CS's,
and the pipeline drains. RUN stays set and `ERR` up, so `FLAG-1` reads as
`micro`'s, and the run is held at the prompt with `micro`'s message, at
`micro`'s PC and microcycle; the run's count stands while the prompt
answers (`rtl_on_revision_16_reports_a_halt_cons_halt_as_micro_does` in
`tests/halt.rs`). A halted pipeline's PC is the fetch after the next word
to run, as committed words chose it, which is `micro`'s PC, and the spy
readout gives it (A15b.13): `IR` the next word to run, `PC` the next
fetch's address, both `micro`'s after a console's halt, a microcode single
step and `HALT-CONS` (`the_pipeline_s_readout_at_a_drained_halt_is_micro_s`).
Halted before the boot's nopped word has run, the readout's `IR` is the
PROM's first word where `micro`'s is the nopped one.

**The time-neutral harness** compares the pipeline with `micro` on a run
(`tests/support/neutral.rs`, `examples/profile.rs` under
`MUIR_TIME_NEUTRAL=1`). Both run on neutral time, `Machine::cycles` times
the period: the timers, the RTC, the interrupts' sampling and the devices
see it and nothing else, and the drain, the sweeps and every memory wait
take none; `micro`'s profile line then gives its time as neutral time with
no memory charged (`micro_under_neutral_time_charges_no_memory_time` in
`tests/profile_time.rs`). The harness acts on an absolute schedule of `Machine::cycles`
with the host's dates fixed, the file device's among them: every date it
reports is the harness's, so that a run's words do not depend on when its
files were made
(`under_the_time_neutral_harness_the_device_reports_the_harness_s_date` in
`tests/quux_file_device.rs`), and between two microcycles on both engines:
the pipeline halts after the schedule's microcycle and drains, its posted
writes landed, before the harness looks at the screen or presses a key, and
`micro` sends out a write still waiting
(`the_harness_acts_between_two_microcycles_on_both_engines`). With
`MUIR_D=off` the harness holds D off on both engines: a write of the
MACRO-DISPATCH register keeps its enable and never D's, `<30>`, so that no
return dispatches on a fetched word; with `MUIR_D=on` it sets D's enable
with the register's, for a microcode that does not
(`MacroDispatch::d`, `d_s_switch_keeps_or_clears_d_s_enable`); held off,
the main-loop machines whose microcode sets D's enable run as those whose
microcode leaves it clear (`d_held_off_runs_the_main_loop_machines_as_d_off`). The
fused returns that need no fetch are the same either way. At every 65,536th step of LC the pipeline
halts after the word that stepped it, the word in EX not yet run squashed
with RD's and CS's, and drains (`Pipeline::boundary_at`); a digest of
`Machine::cycles`, the step count, the registers, A, M, the PDL buffer, the
SPC stack, the OA registers and the MACRO-DISPATCH state is then `micro`'s
after that microcycle, and at a workload's end with main memory and the
dispatch memory. At a boundary `micro` sends out a write still waiting, as
its halt does, where the pipeline's drain takes it
(`a_boundary_between_a_register_write_and_the_next_word_digests_alike`).
On main-loop programs, D on and off, every boundary digests alike, and a
missing d3 bypass, a stack copy not restored, a squashed start and a nopped
slot counted twice each change a digest
(`the_harness_digests_the_pipeline_as_micro_at_every_boundary`). The
profile's log opens with the run's configuration: the engine, the machine,
the band, the PROM, the microcode, main memory, the period, the memory
timing, the cache, `MUIR_H8A`, D, whether time is neutral and the RTC; and,
on the pipeline, closes with the starts made right
after a start, by the two starts' kinds, and the write channel's writes,
two-beat writes and accepts delayed by a second beat.

### The profile

**The profile** (A15b.8) is the file the micro-assembler's profile pass
reads to set each conditional jump's hint, `IR<48>`, and each dispatch's
predicted entry, `IR<63:48>` (A15b.2). `examples/profile.rs` makes it on
the pipeline with `MUIR_PROFILE_OUT=<file>`, booting QUUX's band `MUIR_BAND`
names, its files on the file device under the pathname host `FILES`, with no
Chaosnet server: the pipeline records, from the
first workload's typing to the last workload's end, what every predicted
site did (`Pipeline::outcomes`), and `muir::pipeline::profile` writes one
record a site with the majority outcome. `MUIR_PROFILE_MICROCODE` names the
microcode for the header, its version, its `ucadr.mcr`'s SHA-256 and its
commit, as the caller knows them. A program whose branches
are known gives the lines it should, and a hint, an offset, a kind or a
count written wrong is caught
(`the_profile_of_a_program_whose_branches_are_known` in
`tests/revision_15_rtl.rs`).

**The sites.** A conditional jump that RD predicts: a JUMP whose condition
is not "always", that is not a WRITE-I-MEM (P and R), carries no POPJ
(`IR<42>`) and no SL (`IR<60>`); and a DISPATCH that is not a
dispatch-memory write (`IR<11:10>` = 2). A site no word executed has no
line.

**The format**, exactly, as the profile pass reads it. The file is ASCII
text, one record a line, its words separated by spaces. A line whose first
character is `;` is a comment. The header's lines come first; the pass
reads their words and ignores them:

```text
profile 1
microcode <version> <sha256 of ucadr.mcr>
muir-sim <the commit that made it>
workloads <the workloads' names>
```

`profile 1` is the format, version 1. Comments after them give the
microcode's commit, the machine and the coverage. Then one record a site,
in the order of the sites' addresses:

```text
jump KEY H EXECUTIONS
dispatch KEY KIND TARGET EXECUTIONS
```

- A `KEY` is `LABEL+OFFSET`: the nearest control store label at or below
  the address in the microcode's symbol table, `ucadr.sym`'s `I-MEM`
  names, the alphabetically first where several name that address; and
  the address's offset from it, in octal, always written, `+0` included,
  so that a label ending in `+` reads as one. The pass resolves a key as
  the label's location plus the offset.
- A jump's `H` is `1` when the jump transferred in more than half its
  executions, and `0` otherwise, a tie included.
- A dispatch's `KIND` is its majority entry's kind by P and R: `drop` (P
  and R), `jump` (neither), `call` (P) or `return` (R). `TARGET` is the
  entry's address as a key for a jump or a call, and `-` for a drop-through
  or a return. The majority is the entry, by kind and address, taken most
  often; a tie goes to the first in the order drop, jump, call, return and
  then by address.
- `EXECUTIONS` is the site's executions, in decimal. Every executed site
  has a record, a jump whose `H` is 0 and a dispatch whose `KIND` is `drop`
  among them, so that the pass's shares of the executions it applies mean
  what A15b.8 says.
- The pass sets H for `H` 1, and for a dispatch sets `<61:48>` to the
  target's address (0 for `drop` and `return`), `<62>` to P inverted and
  `<63>` to R inverted: `drop` 00, `jump` 11, `call` 01, `return` 10
  (A15b.2's table). A key it does not find, or finds on a word of another
  class, leaves the word's extension zero.

The coverage comments give, for each class, the sites in the control store
the run loaded and those executed, which the file keys, and the share of
their executions whose outcome is the one the record predicts.

## The Ethernet device

QUUX has an Ethernet device (QUUX's Ethernet contract, §2 and §10),
`muir::ethernet_device`: the register page's words 240-257, word 100 `<8>`
its interrupt, and feature word 15 `<2>` saying it is there. It has no
Chaosnet interface: its 140-157 are reserved and word 100 `<6>` is never set
([the network](#the-network)). A checkpoint of it carries the device's state
and 253-257 ([the checkpoint](#the-checkpoint)). What holds it is
`tests/quux_ethernet.rs` and, for the posted writes and the cache, the last
tests of `tests/revision_15_rtl.rs`.

**The registers**, words 240-257:

| Word | | |
|---|---|---|
| 240 | control, read and written | `<0>` enable, `<8>` interrupt enable |
| 241 | status, read only | `<0>` enabled, `<1>` quiet, `<2>` configuration refused, `<3>` index fault, `<7:4>` received Ethernet frames dropped by the type filter mod 16, `<8>` an Ethernet frame waiting (251 ≠ 252), `<15:9>` received Ethernet frames dropped by the IPv4 checks mod 128, `<23:16>` received Ethernet frames dropped mod 256, `<31:24>` transmit entries refused mod 256 |
| 242 | transmit ring base | `<27:0>` a physical word address on an 8-word line |
| 243 | transmit ring size | `<3:0>` the log2 of its entries, 0 to 8 |
| 244 | transmit producer, the processor's | `<15:0>` |
| 245 | transmit consumer, the device's, read only | `<15:0>` |
| 246 | receive ring base | as 242 |
| 247 | receive ring size | as 243 |
| 250 | receive producer, the processor's | `<15:0>`: buffers given to the device |
| 251 | receive consumer, the device's, read only | `<15:0>`: buffers filled |
| 252 | receive taken, the processor's | `<15:0>`: filled entries taken back |
| 253 | the MAC address, high, read only | `<15:0>`: bytes 0-1, byte 0 in `<15:8>` |
| 254 | the MAC address, low, read only | `<31:0>`: bytes 2-5, byte 2 in `<31:24>` |
| 255 | the IPv4 address, read only | `<31:0>`: a.b.c.d as `a<<24 \| b<<16 \| c<<8 \| d` |
| 256 | the netmask, read only | as 255 |
| 257 | the gateway, read only | as 255; always a gateway, by default the link's other end |

253-257 are the host's (`ethernet_device::Host`): `--ethernet-mac`,
`--ip-address` and `--ip-gateway`'s, by default `02:00:00:00:00:01` and
169.254.100.2/30 by 169.254.100.1, QUUX's side of the tap `quux0` and the
link's other end (QUUX's Ethernet contract, §3). There is always a
gateway: on the address's subnet, and never 0.0.0.0 or QUUX's own
address. No reset changes them, and a checkpoint carries them (below).
Writes of the read-only words are ignored
(`the_page_has_240_to_257`).

**Configuration** is the file device's. 242, 243, 246 and 247 are written
while disabled and ignored while enabled. The enable checks them: a base
off a line, a size over 8, or a ring past main memory is refused, status
`<2>`, and the device stays disabled
(`the_enable_refuses_a_ring_off_a_line_too_big_or_past_memory`). While
disabled the indexes read 0 and writes of 244, 250 and 252 go nowhere
(`the_configuration_words_read_back_and_the_indexes_go_nowhere_while_disabled`).
A write of 240 clears `<2>` and `<3>`; 240 `<8>` is taken only by a write
that enables the device or leaves it enabled.

**The rings.** Each index is 16 bits and runs free, and an entry's slot is
its index mod the ring's size (`the_rings_wrap_at_1_2_and_256_entries`,
`the_indexes_run_free_across_2_to_the_16`). An entry is 8 words. A transmit
entry's word 0 is the Ethernet frame's address, `<27:0>`, on a line, and
word 1 its length in bytes, 14 to 1514; its other words are 0. A receive
entry's word 0 is the buffer's address, on a line, and word 1 its length in
bytes, both the processor's; word 2 is the received Ethernet frame's
length, the device's. There is no response ring: a transmit is done when
245 passes it. An index write is ignored, and sets `<3>`, if it makes
244 − 245 more than the transmit ring's size or 250 − 252 more than the
receive ring's, takes 252 past 251, or moves an index backwards
(`an_index_write_against_the_rules_is_ignored_and_faults`).

**Transmit.** The device takes entries from 245 up to 244 in order, sends
each Ethernet frame and advances 245. An entry whose length is outside
14-1514, or whose Ethernet frame is off a line or past main memory, is not
sent: it is counted in `<31:24>`, and 245 passes it. The checksum and type
rules (contract §2.4, `muir::ethernet_offload`) fill in the checksums in
the copy the device sends, or refuse the Ethernet frame, which is counted
the same way (`transmit_refusals_are_counted_and_passed`). A transmit
writes no memory (`a_transmit_writes_no_memory`). With nothing behind the
device, a transmitted Ethernet frame goes nowhere and 245 still advances.

**Receive.** The device takes an Ethernet frame addressed to its MAC address
or to broadcast; any other is dropped and counted nowhere, and there is no
promiscuous bit
(`the_mac_address_filter_takes_its_own_address_and_broadcast`). Then the
rules' type filter, counted in `<7:4>`, and their IPv4 checks, counted in
`<15:9>`; a frame either drops takes no buffer
(`the_type_filter_and_the_ipv4_checks_count_and_take_no_buffer`). Then a
buffer: none given (250 at the entries filled), or one shorter than the Ethernet frame,
drops it, counted in `<23:16>`, and the buffer stays for the next
(`a_full_receive_ring_drops_and_counts`); so does an Ethernet frame longer
than 1,514 bytes, whatever its buffer's room
(`a_received_frame_over_1514_bytes_is_dropped_and_counted`). A buffer off a
line, or running past main memory, is taken as too short: nothing is
written and the entry stays unfilled, so later Ethernet frames drop too
(`a_buffer_off_a_line_or_past_memory_is_too_short`). A taken Ethernet frame is written
into the buffer of the next entry to fill as the file device writes a
buffer, byte k in `<8(k mod 4)+7:8(k mod 4)>` of word k/4, every word a
fixnum, the bytes past its end in the last word 0 and no word past it; its
length goes into the entry's word 2, and the entry is filled: the host's
side reads RX_CONS moved
(`a_received_frame_is_written_as_fixnums_with_its_tail_zero`). 251 follows
the entries filled once the cache is coherent, below. The driver
keeps a buffer within one 1,024-word page: the device cannot see pages.

**Coherence** follows the pipeline's rules. On the pipeline a write of 244
or 250 moves only once every earlier write is answered, as CMD_PROD's does
(`a_transmit_entry_in_the_posted_writes_is_read_whole`,
`a_receive_entry_in_the_posted_writes_is_taken_once_it_lands`). Before 251
moves past an entry, the cache holds nothing stale: by **line clearing**
(contract §2.5 (ii)), as muir-fpga's fabric does it, clock for clock. An
engine reads the entry at slot 251 out of main memory, through the port's
one fill, installing nothing: its buffer, its room and its length. With n
the length clamped to the room and to 1,514 bytes, it clears the set of
each line from the buffer's first word to its ceil(n/4)th, one a clock,
then the entry's line's, and only then moves 251 by one
(`line_clearing_clears_the_sets_line_by_line_before_251_moves`). The
engine takes an entry in the clock after it is filled; the port's transfer
channel is its two clocks later; it drops its line, and reads the entry in
R + 2 clocks, R the port's read latency in clocks, then its three words a
clock each. So with nothing else on the port, 251 moves at the edge of
clock T = R + 9 + L after the clock the entry was filled in, L the
buffer's lines cleared (0 when n is 0): a read of 251 granted in that
clock, which takes the register in the clock after, reads the new index. At the Kria's memory timing a 42-byte
Ethernet frame in a buffer on a line and a 1,514-byte one give T = 46 and
92 at 10 ns, 52 and 98 at 8.5 ns, and 31 and 77 at 18 ns
(`line_clearing_moves_251_at_r_plus_9_plus_l`). Status `<8>` and the
interrupt follow 251, and 252 may not pass it. Lookups never wait for the
clears, and nothing is swept (`line_clearing_never_holds_a_lookup`). The
engine's read of the entry waits for a fill of the processor's or a table
read's in flight, a processor's lookup in its clock or the clock before, a
table read asked for in its clock or looked up in the clock before, and
the file device's sweep; after 16 clocks of waiting it is let in, and the
lookups are held a clock as if busy. A clear waits for a clock no fill
installs in (`line_clearing_waits_for_a_processor_s_fill_and_its_install`).
Block-disk's transfer moves at START on muir and never holds the channel,
so it never delays the engine there. The halt's drain waits for the
engine. On `micro`, which has no cache, and under neutral time, 251 moves
with the entry filled, and under neutral time the sets are cleared at once
(`a_received_frame_is_seen_after_251_moves`,
`the_machine_s_251_follows_the_entries_filled`).

**The interrupt**, word 100 `<8>`, is up while 240 `<8>` is on and 251 ≠
252: a level, cleared by writing 252 up to 251, by `<8>` off, by a disable
and by `RESET-DEVICES`. Status `<8>` is the same, ungated
(`word_100_bit_8_is_a_frame_waiting_under_the_interrupt_enable`).

**Reset and quiet.** Disable is the reset: transmit entries not yet sent
are dropped, nothing more is received, the indexes and the interrupt
enable go to 0, and the bases, the sizes and 253-257 stay. Quiet, `<1>`,
is not enabled, and on muir it is up at once
(`disable_drops_what_waits_and_keeps_the_configuration`). `RESET-DEVICES`
disables it and clears `<2>` and `<3>`; `PROG.UNIBUS.RESET` does not reach
it, and at power-on it is disabled. The four counters run free from
power-on (`reset_devices_disables_it_and_clears_the_fault_bits`).

**Time.** A transmit entry is sent 20 µs after it is due: after the later of
the producer write that posts it and the previous entry's sending, a
refused entry's included (`a_refused_transmit_entry_takes_20_us`),
reckoned exactly in units of 0.5 ns and rounded up once to the first clock
at or after its end, as a file device command's time is. A received
Ethernet frame is delivered at the first clock at or after it arrives,
and never before the time the device has reached
(`time_is_on_the_grid`, `a_transmit_takes_20_us_at_every_period`). The
20 µs is **unverified**: the file device's estimate of the boards' round
trip, which the boards' own measurement settles.

**The host side** is what sits behind the device, `ethernet_device::Peer`:
a test attaches a scripted one in the process with
`Machine::attach_ethernet_peer` (`ethernet_device::Script`), and a run the
socket `--ethernet-socket` names (below). A copy of the machine has nothing
behind its device. Without the flag, a transmitted Ethernet frame goes
nowhere, 245 still advancing, and none arrives
(`without_a_socket_transmits_go_nowhere_and_245_advances` in
`tests/ethernet_socket.rs`). `--ethernet-mac` gives 253-254 and the filter's
address, `--ip-address` and `--ip-gateway` give 255-257, each checked
before the run
(`the_address_flags_are_words_255_to_257_and_default_to_quux0_s`,
`the_address_flags_values_are_checked_before_the_run`), and `--ethernet-trace` prints every Ethernet frame sent and
received, and why one is refused or dropped
(`the_trace_says_what_is_sent_received_refused_and_dropped`).

**A checkpoint** never refuses on the device's account: waiting transmit
entries are in main memory and their indexes are registers, so a resume
sends them, at the times an unbroken run would
(`a_checkpoint_with_transmits_waiting_resumes_and_sends_them`). It holds
251 as the entries filled: the pipeline's halt waits for line clearing, so
the two are one there, and a resume reads the cache's lines again from main
memory. It carries 253-257 (`revision_16_s_checkpoint_carries_253_to_257`), and a
resume under another `--ethernet-mac`, `--ip-address` or `--ip-gateway`,
given or by default, is refused naming each flag that differs, as one
under another `--rtc` is. A band's addresses are its tap's, so a
checkpoint resumes only on the tap it was taken on
(`a_resume_under_other_addresses_is_refused_by_the_flag_s_name`).

### The host side: `--ethernet-socket`

muir connects to a UNIX stream socket as a client (QUUX's Ethernet
contract, §3; `muir::ethernet_socket`). Every number on it is big-endian.

**Plain mode**, `--ethernet-socket <path>`: each Ethernet frame, either
way, is a record of its length in bytes, 32 bits, and its bytes. That is
QEMU's `-netdev stream` format. A record of length 0 is skipped. A received
Ethernet frame is delivered, or dropped, at the first edge after it arrives,
by the host's clock (`plain_framing_carries_ethernet_frames_both_ways`).
muir holds at most 1,024 that the machine has not taken, and drops what
arrives beyond them, saying so once until it takes one. What the device
sends is written by a thread of its own, in order, as the socket takes it:
muir holds at most 1,024 that the socket has not taken, and drops what is
sent beyond them, saying so once until the socket takes some, so a peer
that stops reading never stalls the machine. A dropped Ethernet frame is
counted nowhere in the device, either way
(`plain_mode_holds_1024_frames_for_a_peer_that_reads_nothing_and_runs_on`).
At the run's end, what muir still holds is written if the socket takes it
within a second, and what it does not take is said.

**Clocked mode**, `--ethernet-socket <path>,clocked`: every record carries
the machine's time.

| Field | Bytes | |
|---|---|---|
| length | 4 | the Ethernet frame's length in bytes, 0 to 65,535 |
| time | 8 | machine time in whole ns |
| Ethernet frame | length | |

A record of length 0 is a control record: from the peer, **run until** its
time; from muir, **reached** its time. Otherwise it is an Ethernet frame:
from the peer, to be delivered at its time; from muir, sent at its time.

- muir runs until the time the peer last named, 0 at the start. At the
  first edge at or after it --- the run's first edge, at the start --- it
  sends the Ethernet frames it has sent since its last "reached", each with
  the time it was sent, in order; then "reached" with that edge's time; then
  it reads records until a "run until", and runs on.
- An Ethernet frame from the peer is delivered at the first clock at or
  after its time and never before the time muir has reached; frames of one
  time are delivered in the order they came.
- A "run until" a time already reached is answered at once with "reached"
  and the same time.
- The machine's time is in units of 0.5 ns: a time muir
  sends is its own rounded down to the nanosecond, and one it reads is
  exact.

muir reads the socket only while it waits, so the band's time and every
delivery are independent of the host's scheduling, and a script rerun on
the same band gives the same records bit for bit
(`clocked_frames_arrive_at_their_times_and_muir_waits_at_each_reached`,
`clocked_mode_reports_each_transmit_with_its_time_before_reached`,
`two_runs_of_one_clocked_script_give_the_same_bytes`,
`quux_waits_in_clocked_mode_on_both_engines_and_repeats_itself`). The peer
reads what muir sends while it waits for "reached".

The wait wakes every 100 ms of the host's time, and at a signal, without a
"run until", so ^C reaches a run whose peer has gone silent: between two
microcycles the run loop looks at ^C, the prompt and its stops, as it does
every 4,096 microcycles of a run, and waits again, the microcycles and the
records being those of an unbroken wait. A wait that `rtl`'s clocks reach
inside a microcycle goes on until a ^C, which ends the microcycle without
the peer and is taken at once after it; a run held there and continued
waits for the peer again, and what the peer sent for the rest of that
microcycle is delivered at the next edge
(`a_c_ends_a_run_waiting_on_a_silent_clocked_peer_within_a_second`).

**In either mode** the device takes the socket at every edge, enabled or
not: an Ethernet frame arriving while it is disabled is discarded and
counted nowhere, so a band that never touches 240-257 holds nothing
(`a_disabled_device_discards_what_a_socket_sends_and_holds_no_more`). The
peer closing the socket, or a record longer than 65,535 bytes, ends it:
muir says so once and runs on as without the flag, and a clocked run no
longer waits (`the_peer_closing_leaves_the_device_as_without_a_socket`).
A socket muir cannot connect to stops the run before it starts. The setup
names the MAC address and the socket and its mode. A run's socket, MAC
address and trace are the run's, not a checkpoint's: a resume keeps them
(`a_resumed_device_sends_and_receives_through_the_socket_by_its_mac_address`).
The CADR refuses the three flags by name
(`the_flags_are_refused_on_the_cadr`).

## Not modeled

`chip` is the CADR's boards, netlist for netlist, and QUUX has none:
`--chip` is `cadr`'s alone.
