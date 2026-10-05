# The Chaosnet board

Part of [the muir manual](manual.md).

The CADR's Chaosnet interface is not a board of its own. It shares the I/O
board with the keyboard, the mouse, the clocks and the serial port, and MIT
drew the two halves separately: thirteen `LM*` pages for the network and
fourteen for the rest, all twenty-seven extracted into `data/CADRIO.netlist`
and held to one wire list, `mit/cadrio/iob.wlr`, because they are one board.
[Where the netlists come from](netlists.md) is the chain that puts them
there; [the machine it models](machine.md) is where the board sits on the
Unibus.

The authority for what the interface does is **AI Memo 628, *Chaosnet*,
David A. Moon, 1981** --- section 2 for the cable and the hardware
protocols, section 7 for the programming documentation --- and the board
itself, where the two disagree. Which one was followed is said each time.

Every claim below names the file it was read from or the test that holds it.
`tests/chaos_netlist.rs` runs MIT's board through its registers under a
Unibus master; `tests/chaos_rtl.rs` runs the same register sequences on the
board and on muir's behavioral interface and holds the two to each other,
word for word and instant for instant; `tests/ether.rs` holds the cable as a
medium; `tests/chudp.rs` holds the datagram; `tests/chaos.rs` holds the
check word, the framing and the servers on the model side; and
`tests/micro_chaos.rs` boots a band over the Chaosnet and reads a file
across it.

## The interface the software sees

AIM-628 §7, memo page 38: "The standard interrupt-vector address for the
Chaosnet interface is 270. The standard interrupt priority level is 5. The
standard Unibus address is 764140." The section describes "the Unibus
version of the Chaosnet interface, which attaches to pdp11s and Lisp
Machines", and the I/O board's `LM*` pages are that interface.

Six registers at five addresses --- `764142` is two, one for reading and one
for writing:

| Address | Register | Read or write |
|---|---|---|
| `764140` | Command/Status Register | read/write |
| `764142` | My Address | read |
| `764142` | Write Buffer | write |
| `764144` | Read Buffer | read only |
| `764146` | Bit Count | read only |
| `764152` | Start Transmission | read only |

**The board ignores `A3` except where a register needs it.** Its decode
gives the Chaosnet the group on `A<6:4>` = 6, and within that group the CSR
answers at `764140` and `764150` alike, a write to `764142` or `764152`
reaches the transmit buffer, and Bit Count answers at `764146` and `764156`.
`A3` tells apart exactly two things: a **read** of `764142` is My Address
and a read of `764152` is Start Transmission, and a read of `764154` is
answered by nothing at all.
`tests/cadrio_netlist.rs::the_board_and_the_model_decode_the_block_alike`
walks every even address of the board's block, read and written, on the
netlist board and on the model, and holds the two to the same answer at the
same nanosecond.

**The CSR's bits**, as AIM-628 §7 lists them by mask, memo pages 38 and 39.
"All read/write bits are initialized to zero on power-up."

| Mask | Bit | Kind |
|---|---|---|
| `1` | Timer Interrupt Enable | read/write |
| `2` | Loop Back | read/write |
| `4` | Spy | read/write |
| `10` | Clear Receiver | write only |
| `20` | Receive Interrupt Enable | read/write |
| `40` | Transmit Interrupt Enable | read/write |
| `100` | Transmit Abort | read only |
| `200` | Transmit Done | read only |
| `400` | Clear Transmitter | write only |
| `17000` | Lost Count | read only |
| `20000` | Reset | write only |
| `40000` | CRC Error | read only |
| `100000` | Receive Done | read only |

Loop Back: "If this bit is 1, the cable and transceiver are not used and the
interface is looped back to itself. This is for maintenance." Spy: "the
interface will receive all packets regardless of their destination."
Transmit Abort: "the last transmission was aborted, by a collision or
because the receiver was busy." Transmit Done: "set to 1 when a transmission
is completed or aborted, and cleared to 0 when a word is written into the
outgoing packet buffer." CRC Error is "only valid at two times: when the
incoming packet buffer contains a fresh packet, and when the packet has been
completely read out of the packet buffer" --- between them the board's check
register is mid-packet and the bit reads 1 at every poll, which
`tests/chaos_netlist.rs::crc_error_while_a_frame_comes_in` measures and
`tests/chaos_rtl.rs` holds the model to.

**Timer Interrupt Enable reaches no gate on this board.** AIM-628 has it
"for the interval timer present in some versions of the interface", and this
is not one of them. In `mit/cadrio/iob.wlr` the signal `TIMER.IEN` has
exactly two pins --- pin 12 of the 74LS174 at LMUCON `0B20`, which the CSR
write loads, and pin 17 of the 74LS244 at LMDATP `0D16`, which reads it back
--- and the netlist gives the net the same two. The 74S51 at LMUCON `0E05`
that makes the interrupt has both of its AND pairs taken, `RDONE` with
`RIEN` and `TDONE` with `TIEN`, and a '51 has no third input. So the bit is
stored, readable, and connected to nothing, which is what MIT's own
`chatst.lisp` says of it: "This bit doesnt seem to do anything."
`tests/chaos_netlist.rs::the_timer_interrupt_enable_reaches_no_gate` holds
the net to those two pins in the netlist and in MIT's wire list at once, and
holds the 74S51's ten used pins.

**The address is sixteen switches.** Two eight-switch bodies at LMMYNM
`0D10` and `0D12`: `0D12` carries `MY#0` on pin 16 up to `MY#7` on pin 9,
`0D10` carries `MY#8` up to `MY#15` the same way, and each switch grounds
its bit against a pull-up pack, so **a bit of the address that is one is an
open switch**. `tests/chaos_netlist.rs::the_address_switches_read_back` sets
them for `3050`, reads `764142`, and gets `3050` back, then does it again at
`3060` so the answer is not a coincidence of one bit pattern.

**Both buffers are one 2147 static RAM, 4K by 1.** The outgoing buffer is
the 2147 at LMTBUF `0C10`, addressed a bit at a time by `TBCT<11:0>` off the
three 25LS193s at `0C11`-`0C13`; the incoming buffer is its twin at LMRBUF
`0C04` on `RBCT<11:0>`. Four thousand and ninety-six bits is **256 sixteen-bit
words**, and a word written past that has nowhere to go.

**Bit Count** is "the number of bits in the incoming packet buffer, minus
one. After the whole packet has been read out, it will contain 7777 (a
12-bit minus-one)." Because the count is bits and not words, a receiver that
took wreckage reads back a partial word at the top: the count comes down by
sixteen for each whole word and by only the bits there are for that last
one. `tests/chaos_rtl.rs::wreckage_addressed_to_the_board_lands_on_both_alike`
reads every word of such a buffer off the netlist board and off the model
and holds the count after each.

**A transmission is started by reading a register.** AIM-628 §7: the eight
header words are written into the outgoing buffer, "then exactly the number
of 16-bit data words implied by the byte count in the header", and "the last
16-bit word to be written is the cable address of the destination of the
packet, or 0 to broadcast it". Then `764152` is read. "This method for
starting transmission may seem strange, but it makes it easier for the
hardware to get the source address into the packet" --- and the value read
is the interface's own address, which is the very thing the hardware is
appending. The hardware adds the source and the check word itself; the
software never writes either.

## The cable

AIM-628 §2.1, memo page 3: the ether is "a coaxial cable, of the semi-rigid
1/2 inch low-loss type used for cable TV, with 75-ohm termination at both
ends", a ten-meter flat cable to each transceiver, about a kilometer at
most, and "probably a few dozen" nodes.

§2.3, memo page 4: the transceiver "receives a differential digital signal
from the computer interface and impresses it onto the cable as a level of
about 8 volts for a 1, or 0 volts (open circuit) for a 0 ... When the cable
is idle it is held at 0 volts by the terminations." It "returns a
differential signal to the interface. In addition, it detects interference
(another transceiver transmitting at the same time as this one) and informs
the interface." So the cable's level is the OR of everything driving it,
every transceiver hears the whole cable including its own transmission, and
interference is a fact the hardware reports rather than one it infers.

On the board the transceiver itself is off the card: LMLNDR `0A03` is a
placeholder carrying its four wires. What the board drives is the 26LS31
line driver at LMLNDR `0A02` --- `mit/cadrio/iob.wlr` puts the active-low
`-TTL.D.OUT` on its input at `A02-01`, its true output on `TRANS.DATA-` and
its complement on `TRANS.DATA+`, so sending a one puts the plus above the
minus. The receive pair comes back through the 26LS33 at LMLNDR `0A01`, and
the inverting 74S158 at LMLNDR `0E02` turns it into `TTL.D.IN`, the line
level.

**The coding is Upright Biphase NRZI**, §2.5, memo page 5:

> Each bit cell, which is approximately 250 nanoseconds long, begins with a
> transition in state, from high to low or from low to high. This transition
> marks the beginning of a bit cell and provides self-clocking. 3/4 of the
> way through the bit cell, the state of the cable is sampled; high
> represents a 1 and low represents a 0. If the bit being represented is the
> same as the previous bit, there will be one transition at the beginning of
> the bit cell and a second in the middle of the bit cell.

The information bit-rate is 4 million bits a second. "If the ether remains
low for more than about two bit cells, it is considered to be not-busy. This
condition marks the end of a packet." And: "If the ether remains high for
about two bit cells, this is an 'abort signal'."

**Where the memo says 3/4 and the board says something else, the board is
followed.** Three quarters of a 250 ns cell is 187 ns; the board's own delay
chain puts the sample at 175. `GENCLK` is preset by the cell's opening edge
and runs through the TD100NC at LMDETC `0B04`, whose `SDLYD` feeds the
TD100NC at `0B11`, whose `SDLYD2` feeds the TD25NC at `0A11` --- 100 + 60 +
10 ns of strapped taps, which MIT's own comments in `mit/cadrio/iob.eco`
give as those three in the consolidation of 18 February 1981. `0A11` forks:
`LOCKOUT END` at its 10 ns tap clocks the lockout flip-flop, the 74S74 at
LMDETC `0E08`, and `SAMPLE` at its 15 ns tap is where the level is read.
They are siblings, both 160 ns into the chain, and neither is downstream of
the other. So the lockout ends at 170 ns --- between the mid-cell transition
at 125 and the next cell at 250, which is what it is for ---  and the sample
is taken at 175. `tests/chaos.rs::the_lockout_ends_between_the_mid_cell_transition_and_the_next_cell`
holds that ordering.

**The frame's word order is backwards**, §2.5, memo page 6:

> Packets are transmitted over the ether in reverse bit-order, for hardware
> convenience. The three header words, which to the software appear to be at
> the end of the packet, are transmitted first, in the order check, source,
> destination. The data words, in reverse order, follow. Words are
> transmitted least-significant bit first. ... At the end of the packet, an
> extra zero bit is appended to bring the ether to the low state so that an
> extra spurious clock-transition will not be generated when it goes idle.
> This bit is stripped off by the interface and is never seen by software.

So the transmit buffer as written, then the source, then the check word,
each most-significant bit first, is played out backwards bit by bit with a
zero on the end. `tests/chaos.rs::a_frame_survives_the_wire` puts a packet
through the coding and back and holds that the first sixteen bits on the
cable are the check word.

**A packet is "a sequence of up to 4032 data bits, plus 48 bits of header
information used by the hardware"** (§2.2, memo page 3), the hardware header
being "three 16-bit words, called destination, source, and check". The
software protocol takes 128 of those data bits as its own header (§2.2,
memo page 4), which leaves 3,904 bits --- the 488 bytes §3.5 gives as the
most a packet carries.

## The check word

The check word is the third of the hardware's three header words, and the
chain behind it is worth setting out in full: part of it is read off a data
sheet, part is settled by the board where the sheet is ambiguous, and part
was recovered by experiment because nothing states it anywhere. Which is
which is said here, since that is this page's rule.

**The polynomial is read, not guessed.** The check word is generated by the
Fairchild 9401 at LMTBUF `0C09`. That part divides by one of eight
polynomials chosen by a three-bit code on its select pins, and the sheet's
Table 1 lists all eight --- `src/part.rs` carries the table as `CRC9401`,
cited to the sheet. **All three select pins are grounded on the transmit generator**,
which makes its code 0 and its polynomial **CRC-16,
`x^16 + x^15 + x^2 + 1`**. Both sources agree that they are grounded:
`data/CADRIO.netlist` has pins 3, 5 and 8 of `0C09` on `GND`, and
`mit/cadrio/iob.wlr` has the same three, naming each as a select as it goes.

**The board carries a second 9401 and it is not wired alike.** The receive
generator, at LMRBUF `0C07`, takes `RACT` on pin 3 with pins 5 and 8
grounded, `-CRC.PRE` on pin 2, `RRESET` on pin 4 and `CRCERR` off pin 13.
So its code is 0 while the receiver is idle and something else for as long
as the receiver is active.

**Which pin is which select, the board settles.** The sheet's connection
diagram and its logic symbol disagree over pins 3, 5 and 11. Pin 11 stops
being in doubt as soon as the board is looked at: `0C09` takes it from pin 9
of the 74165 at LMTBUF `0B12`, which is that shift register's `QH`, and a
serial output can only be feeding the 9401's data input --- and MIT's wire
list names that pin `D` outright. That leaves 3, 5 and 8 as the three
selects. The two sources then **disagree about the order**: MIT's wire list
names pin 3 `S2`, pin 5 `S1` and pin 8 `S0`, where `src/part.rs` has 3 as
`S0` and 8 as `S2`. **It changes nothing for the transmit generator**, whose three
pins are grounded, so its code is 0 read from either end. **It decides the
receive generator**, where `RACT` sits on the pin in dispute: during
reception the code is 1 under `src/part.rs`'s naming and 4 under the wire
list's, and those are two different polynomials. **The board has now answered**, by what its
receiver does with a frame it has just sent. The wire list is the board
as built and outranks the sheet's own diagram; where its pin names and the
board's behavior disagree, that is a finding to state and not a preference
to exercise.

**The bit order is the shift registers'.** The two 74165s at LMTBUF `0B12`
and `0B13` shift each sixteen-bit word out most-significant bit first, and
that is the order the 9401 divides it in.

**The rest is not written down anywhere, and was recovered from the
board.** Neither the sheet nor the drawings fix the seed the register starts
from, the direction it shifts, or whether the source word the hardware
appends falls inside the division or outside it. So they were not read ---
they were found. Of the 9401's eight polynomials, the two bit orders and the
two seeds, **exactly one arrangement reproduces the word the netlist board
itself produced** when a packet was looped back through it: CRC-16, from a
cleared register, over the buffer's words in the order written --- header,
data, cable destination, then the source the hardware adds --- each word
most-significant bit first. For the packet
`tests/chaos_netlist.rs::a_packet_loops_back_through_the_board` writes, the
word the board produced is **`135771`** octal.
`tests/chaos.rs::the_check_word_is_the_boards` pins that word against
`check_word`'s own arithmetic, the loopback test reads it back off the board
live, and `tests/chaos.rs::a_frame_survives_the_wire` holds that it is the
first word on the cable, which is AIM-628's order.

**What none of that has met is a real 9401**; see [Unverified](#unverified)
below.

## Whose turn it is

Chaosnet has no central arbiter. AIM-628 §2.6, memo page 6: "when a network
node has a message to transmit, its interface seizes the ether and transmits
a packet." Two avoidance mechanisms sit on top of that. The first is carrier
sense: "an interface will never initiate transmission unless the ether is
seen to be not busy, i.e. it has been in the low state for some time." The
second is the turn:

> The basic idea is that each interface is assigned a time-slot, or *turn*,
> according to its address. It may only initiate transmission during its
> turn. ... Each interface contains a time-slot counter which counts while
> the ether is not busy, keeping track of whose turn it is. Each packet
> synchronizes the counters in all of the interfaces by setting them from
> the source address of that packet.

**The board computes the address difference bit-serially as the source word
goes by.** Page LMMYNM subtracts in the 74S287 at LMMYNM `0D01` --- `MATCH
SO FAR` the difference bit, `RS2` the borrow --- and the two 74LS164s at
LMTURN `0B18` and `0B19` shift the difference bits in as they are made, so
that at `SRC STB`, when the word is over, they hold bits 14 down to 3 of the
difference. The counter takes bits 14 to 7 of it as its low byte, bit 14 at
the bottom: the difference arrives bit-reversed. The turn comes that many
counts and one after the cable goes idle.
`tests/chaos_rtl.rs::the_turn_timer_loads_the_address_difference_bit_reversed`
runs a board at four different addresses, reads the loaded value off the
nets at each `-LOAD.MY.TURN`, and counts the clocks to `MY.TURN^` rising.

The counter itself is three 74LS193s at LMTURN `0A17`, `0A18` and `0A19`,
twelve bits, of which only the low byte can reach bit 7 --- `MY.TURN^` ---
so only the low byte decides a turn. It is clocked by `MY.TURN CLK^`, which
is the 74S112 at LMTURN `0D14` toggling on every terminal count of the
74LS161 divider at LMTURN `0A16` while the cable is idle, and held set while
the cable is busy. The divider reloads 14 from its own terminal count on
`FCLK/2^`, giving a count every 500 ns, and the toggle halves that: **one
slot is 1,000 ns and a whole round is 256 of them.** A frame ready goes
468 ns after the count that carries the low byte from 0 to 255, the 74S74 at
LMMODU `0B15` clocking `TSTART` from `TSREMPTY AND CW AND -CBLBSY`.

**The board takes the first turn of the round after its own frame, where
AIM-628 describes the last.** The memo's picture is a virtual token, §2.6,
memo page 7:

> When an interface transmits, the token stops moving and remains at that
> interface until the end of the packet, whereupon it continues down the
> cable, passing every other interface, giving them each a chance to
> transmit before letting the first interface transmit a second packet.

The board as wired cannot do that. It hears its own transmission as it hears
any frame --- it must, for its transceiver to find a collision --- and loads
the difference between that source word and its own address, which is
**zero**. So `MY.TURN^` rises on the first count after the cable goes idle,
not the last. Measured at four addresses by the test named above: for the
board's own packet the load is 0 and the turn comes at count 1; for another
station's it is `turn_byte` and the turn comes that many counts and one
later. **The board is followed, and this is a discrepancy that the board
wins.**

What keeps the memo's picture true on a real machine is that no host can
reload the transmitter inside one microsecond. Transmit Done comes 500 ns
*before* the frame's nominal end, and from there the software must write the
packet back a word at a time down the Unibus and read `START`, and the
hardware must then shift the source and check words in behind it. So the
first turn is always missed and the frame goes a whole round later.
`tests/chaos_netlist.rs::two_packets_back_to_back_wait_a_whole_round`
measures **257 slots** between two packets the board sends back to back with
microcode 323 driving it --- the one turn its software missed, and the 256
of a round.

muir holds its own model stations to the same bound rather than letting them
answer at the first slot: `Node::station` says which nodes stand for a
station with an interface and a host behind it, and such a node is charged
`refill()` --- `REFILL_WORD_NS` a word, plus the hardware's own shift-in ---
before its next turn counts.
`tests/ether.rs::a_stations_next_frame_waits_for_its_host_to_refill` holds
two short frames from one station 257 slots apart and two full packets 513,
and `::a_stations_refill_holds_up_only_its_own_next_frame` holds that this
delays nobody else. The counter also **runs free**: a turn missed while the
cable stays idle is gone, and the next is a whole round on, which
`::a_station_after_a_long_idle_goes_on_the_turn_timers_cadence` holds by
asking a station twice in one round and getting the same instant both times.

## The abort signal, in both its uses

AIM-628 §2.5, memo page 5: "Abort signals are used for two purposes."

**Interference.** "If the transceiver detects a collision (two nodes trying
to transmit at the same time), each transmitting interface ceases to
transmit and sends an abort signal (four bit cells long), which tells all
receivers to ignore the aborted packet and ensures that the other
transmitter also aborts." On the board, `COLLISION` is `TBUSY` with
`INTERFERENCE` at the open-collector 74S02 at LMMODU `0B08`, and the `ABORT`
flip-flop, the 74S112 at LMMODU `0A09`, takes it on `-FCLK^`. So a
transmitter looks for interference once per period of the board's 8 MHz
clock --- **interference that is not there at an edge is missed, and the
transmitter goes on until an edge finds it.** The same `0B08` carries a second NOR, `-TTL.D.OUT` on its pin 4 from
`ABORT` and `D OUT` on pins 5 and 6, so `ABORT` setting forces that output
low and the line driver holds the cable **high** for four cells: the abort
signal itself, rather than the driver merely going off.
`tests/chaos_netlist.rs::the_board_aborts_its_transmission_on_interference`
watches `INTERFERENCE IN`, `COLLISION`, `ABORT`, `TBUSY`, `TABORTED` and
`TDONE` in turn and holds `ABORT` within a few cells of the first
interference, with the CSR then reading Transmit Done and Transmit Abort
together. `tests/ether.rs::interference_is_two_transceivers_driving_high_at_once`
holds the other half: the board driving high while another transmitter is
*low*, between cells, is not interference.

**A busy receiver's flow control.** The same page: "When a receiving
interface determines that an incoming packet is addressed to it, but its
receive buffer already contains a packet, it sends an abort signal which
causes the transmitter to stop. This serves the dual purpose of immediately
informing the transmitter that its message did not get through, and
preventing the ether from being tied up while a long packet is transmitted
which the receiver cannot receive."

The board does it in four gates. `RACT`, the second half of the 74S74 at
LMRCLK `0C06`, has `-RDONE` on its D input, `START^` on its clock, its
preset tied high, and its clear from the 74S02 at LMRCLK `0B08` NORing
`RRESET` with `-CBLBSY`. **So the receiver decides once, at `START^`, and
keeps that decision for the whole frame**: 60 ns after the frame's first
edge it takes Receive Done as it stands, and if the buffer was full then,
`RACT` never comes up and nothing of the frame is stored --- even if the
software clears the receiver a moment later, before the destination word has
gone by. `tests/ether.rs::the_receiver_decides_at_start_and_keeps_it_for_the_frame`
holds all three cases, and
`tests/chaos_rtl.rs::a_clear_receiver_inside_a_frame_does_not_let_it_in_on_both_alike`
watches `START^` on the netlist board to confirm the model samples where the
board does.

The destination word then matches all the same, and the 74S10 at LMMYNM
`0D02` makes `-LOST.ONE` from `MATCH SO FAR`, `ITS.ME` and `-RACT`. That
wire has exactly two pins in `data/CADRIO.netlist`: the gate's own output,
and pin 4 of the 74S112 at LMMODU `0A09`, which `mit/cadrio/iob.wlr` names
`-SET1` --- the preset of the very same `ABORT` flip-flop the collision
detector uses. **It does not step Lost Count**; that counter is clocked from
`ITS.ME`, as below. The 26LS31 at
LMLNDR `0A02` then holds the cable high for four bit cells.

**The instant is twelve microseconds in.** The three hardware words go
first, "in the order check, source, destination", which is 48 cells, and the
driver comes on in the cell after them.
`tests/chaos_netlist.rs::the_busy_receiver_aborts_a_frame_addressed_to_it`
measures the second frame's first edge, then `DEST MATCH` at 12,060 ns, the
line driver on at 12,260, and Lost Count stepping at 12,310 --- and holds
the driver on for four cells, the packet already in the buffer untouched,
and the sender's frame ending as wreckage on the cable.

**The sender is told, and reads Transmit Abort.** Two interfaces on one
cable measure it both ways round.
`tests/chaos_two_boards.rs::a_busy_netlist_board_stops_the_sending_interface_which_reads_transmit_abort`
makes the netlist board the busy receiver: its driver comes on 12,250 ns
into the frame and the sending interface reads Transmit Abort 375 ns after
that, which is three edges of its 8 MHz clock, since a transmitter finds
interference only at an edge and only while its own driver is high.
`tests/chaos_two_boards.rs::a_busy_model_receiver_stops_the_netlist_board_which_reads_transmit_abort`
turns it round: the interface's abort signal at 12,260 ns, the board's
transceiver finding interference 240 ns later at the first cell where its
own driver is high, and `TABORTED` one clock edge after that, 365 ns in
all. Each receiver counted one frame in Lost Count and kept the packet it
already held. One bit answers for both uses of the signal because §2.6,
memo page 8, says the transmitter cannot tell them apart: "the transmitter
does not distinguish receiver-busy aborts from real collisions."

**Only what is specifically addressed is aborted.** AIM-628, memo page 6:
"Note that a receiver whose packet buffer is full will only generate an
abort signal if the packet was specifically addressed to it." That is
exactly the `MATCH SO FAR` term on the 74S10 at LMMYNM `0D02`: `MATCH SO
FAR` is the bit-by-bit comparison alone, where `DEST MATCH` is "mine, or
zero, or spying". So a broadcast to a full buffer is counted and not
aborted, anything at all under Spy is counted and not aborted, and another
station's packet is neither.
`tests/chaos_netlist.rs::the_busy_receiver_aborts_only_what_is_addressed_to_it`
measures all three on the board and
`tests/chaos_rtl.rs::a_full_buffer_aborts_only_what_is_addressed_to_it_on_both_alike`
holds the model to the same three.

**Lost Count counts what would have been received; it does not count what
was aborted.** AIM-628 §7: four bits holding "a count of the number of
packets which would have been received if the incoming packet buffer had not
been busy. Setting Clear Receiver resets the lost count to 0." The two sets
are not the same: a broadcast arriving at a full buffer is counted and not
aborted, and another station's packet is neither counted nor aborted. On the
board the counter is the 74LS161 at LMMYNM `0F04`, on `LSTCNT0`-`LSTCNT3`,
clocked by `ITS.ME` falling with `-RACT` up and cleared by Clear Receiver.
**A '161 wraps**: eighteen frames for a board whose buffer was taken by the
first leave seventeen counted and the field reading 1.
`tests/chaos_netlist.rs::the_lost_count_wraps_at_sixteen` measures that on
the board and `tests/chaos_rtl.rs::the_lost_count_wraps_at_sixteen_on_both_alike`
holds the model to wrapping with it.

## The measured constants

Every number muir keeps about this interface is either MIT's own or was
measured on the netlist board, and each says which.

| Constant | Nanoseconds | What it is | Where the number comes from |
|---|---|---|---|
| `CELL_NS` | 250 | one bit cell | AIM-628 §2.5, "approximately 250 nanoseconds"; the board's 4 MHz `FCLK/2^` |
| `SAMPLE_NS` | 175 | where in the cell the level is read | the board's delay chain, against §2.5's "3/4 of the way"; the board is followed |
| `LOCKOUT_NS` | 170 | after which an edge is the next cell's, not the mid-cell one | the same chain: 100 + 60 + 10 ns of taps, `mit/cadrio/iob.eco` |
| `IDLE_NS` | 625 | quiet this long and the cable is not busy | §2.5, "more than about two bit cells" |
| `SLOT_NS` | 1000 | one count of `MY.TURN CLK^` | netlist board: `MY.TURN^` toggles every 128,000 ns |
| `ROUND_NS` | 256000 | one whole round of the turn timer | bit 7 of the 74LS193s: 256 counts of a slot |
| `ABORT_NS` | 125 | how often a transmitter looks for interference | one period of the board's 8 MHz `FCLK^` |
| `ABORT_HOLD_NS` | 1000 | how long an aborting driver holds the cable high | netlist board, and §2.5's "four bit cells" |
| `BUSY_ABORT_NS` | 12260 | frame's first edge to the busy receiver's abort signal | netlist board: `DEST MATCH` 12,060, driver on 12,260, Lost Count 12,310 |
| `RACT_NS` | 60 | frame's first edge to `START^`, where the receiver decides | netlist board |
| `REFILL_WORD_NS` | 1995 | what one word costs a station's host to write | microcode 323's `CHAOS-XMT-2`, fitted over 9 to 253 words |
| `TURN_TC_NS` | 500 | the divider's terminal count | the 74LS161 at LMTURN `0A16` reloading 14 |
| `TURN_FIRST_TC_NS` | 4250 | power-on to the first terminal count | netlist board |
| `TURN_LOAD_NS` | 8250 | frame's first edge to `-LOAD.MY.TURN` | netlist board: `SRC STB`, 33 bit cells in |
| `TURN_START_NS` | 468 | the count that raises bit 7 to the frame's first edge | netlist board |
| `CBLBSY_OFF_NS` | 375 | frame's nominal end --- its first edge plus a cell for each of its bits, the appended zero included --- to `-CBLBSY` lifting | netlist board; `RDONE` rises with it |
| `TDONE_BEFORE_END_NS` | 500 | how far before that nominal end Transmit Done comes | netlist board |
| `TSR_READY_NS` | 4150 | the read of `START` answered, `-SSYN`, to `TSREMPTY` | netlist board, at every phase of its clocks; the answer itself takes 350 to 2,100 ns |
| `TDONE_AFTER_ABORT_NS` | 125 | `ABORT` setting to Transmit Done | netlist board |

## What each engine has

**On `chip` the interface is MIT's board.** `data/CADRIO.netlist` runs as
gates, and the cable reaches it through the transceiver's four wires and
nothing else: `TRANS.DATA+`, the line driver's true output, going out, and
the receive pair and the interference pair coming back, each driven from the
modeled ether's level at the nanosecond. `--io-board model` puts the
behavioral interface there instead.

**On `micro` and `rtl` it is the behavioral interface**, the registers of
AIM-628 §7 over the same ether, with a turn timer that takes terminal counts
one at a time exactly as the board's counter does. The Chaosnet belongs to
the machine rather than to the engine, so the registers answer any engine
through the same path; what the interface wants from an engine is a clock,
and `micro` has one.

**The two are held to each other, not to anybody's opinion of them.**
`tests/chaos_rtl.rs` runs every register sequence on both at once --- the
netlist board under a Unibus master and the behavioral interface under the
same reads and writes, stepped to one clock --- and asserts that every CSR
read, every word read back and every bit count agree, and that the packet
lands at the same poll on both. That covers the loopback, a request for the
time answered by a server, a collision, wreckage addressed to the board,
what a reset interface does before any Clear Receiver, the busy receiver's
abort, Lost Count's wrap, and a Clear Receiver written inside a frame.
`tests/micro_chaos.rs` then holds `micro` and `rtl` to having the same
conversation with a file and time host across a whole boot: the band asks
for the time, gets it, logs in, and probes a file over the `FILE` service.

**The cable is modeled as a medium, not drawn as gates.** It carries a
level, which is the OR of everything driving it; it frames, codes and
decodes; it keeps the turn-taking; and it reports interference when two
transceivers drive high at once. On its model side sit any number of
stations, each of which sees every frame and may offer one to send.

## The stations on the model side

A station is asked for a frame when the cable is free and it is its turn,
and the ether adds the source and the check word. A frame it is given back
--- aborted, by interference or by a busy receiver --- may be offered again,
which is what an interface's driver does on Transmit Abort.

**The CHUDP link is one such station, standing in for a real sender.** Its
address on the cable is the address of the peer whose frame it is carrying,
so a packet from 3040 goes onto the cable with 3040 in the hardware source,
exactly as that host's own interface would have put it there, and every
board's turn timer loads from it accordingly. It waits its turn like anything
else --- a node that pushed a frame onto the cable the moment a datagram
landed would look to the board like a transceiver that had failed.
`tests/chudp.rs::a_peers_packet_reaches_the_cable_at_its_turn` holds both
halves of that.

**What stands in for the far host's driver is the CADR's own**, because it
is the one driver this project can read: muir has no model of an ITS or a
`cbridge` host. So the refill above is microcode 323's `CHAOS-XMT-2` loop,
and the retry bound is microcode 323's `CHAOS-NUMBER-TRANSMIT-RETRIES`,
which `vendor/system-100-0/sys/ucadr/uc-chaos.lisp` assigns as 3 over MIT's
own comment "Send once and retry twice if aborted". `CHAOS-XMT-INTR` counts
it down on every Transmit Abort and at zero falls into `CHAOS-XMT-DONE`,
which takes the packet off the transmit list; the packet is then the
transport's to retransmit, as it is for anything else the cable loses. That
fixes the order of magnitude, which is what the spacing of a burst turns on.

**The tests put a server of their own on the cable instead**, because a test
cannot want a daemon running beside it. It answers STATUS, TIME, UPTIME and
FILE, it lives in `tests/support/`, and it is no part of the simulator ---
**a CADR had no file or time server inside it**, and neither has muir. A run
reaches a real one over the link.

## Chaosnet over UDP

CHUDP puts one Chaosnet packet in one UDP datagram behind a four-byte
header, and it is what `cbridge`, `usim`, `klh10` and the live Chaosnet
hosts speak to each other. It is ordinary UDP, so it needs no privileges and
crosses a NAT, which IP protocol 16 --- the assigned number for Chaosnet ---
does not.

```text
offset  width  field
     0      1  version, 1
     1      1  function, 1: "here is a Chaos packet", the only one defined
     2      2  two argument bytes, sent as zero and not read
---- the Chaos packet, AIM-628 §3.5, in 16-bit words ----
     4     16  the eight software header words
    20      n  data, the byte count rounded up to a whole word
---- the trailer ----
20 + n      2  destination
22 + n      2  source
24 + n      2  checksum
```

**Every 16-bit word goes most significant byte first** --- the header's
words, the data's and the trailer's. The data bytes are packed into those
words as AIM-628 §3.6 says, the first byte of a pair in the word's least
significant half, so a pair appears swapped on the wire and `STATUS` goes
out as `TSTASU`. An odd byte count is padded to a whole word, the zero
landing in the high half, which is the byte that goes first.

**The trailer's first two words are the cable's** --- the address on this
subnet the frame is for, which is the next hop and not necessarily the
packet's own destination, and the sender's own address. **Its third is the
Internet checksum**, the one's complement of the one's complement sum of
every word before it, and **not** the CADR's CRC-16: no CADR ever sent a UDP
datagram, and the check word the machine's own hardware makes is the
cable's. A frame arriving has that checksum verified and is then given the
CRC-16 the interface will want when it comes off the modeled cable.

That framing was read from a running `cbridge` rather than from a
specification: frames in this shape were accepted and forwarded, frames with
their words least significant byte first were refused as "bogus", and frames
in this byte order carrying the CADR's CRC-16 in the trailer were refused
with "Bad checksum", the value printed being exactly the one's complement
sum. `tests/chudp.rs::the_frame_is_these_bytes` pins a datagram muir writes
byte for byte, and `::cbridges_own_answer_reads_back` pins one `cbridge`
itself wrote and holds that muir writes it again identically. The protocol
page at `chaosnet.net/protocol` §2.3 names the same Internet checksum and is
**wrong about the byte order**: it says `cbridge` sends words least
significant byte first, which the running `cbridge` does not.

**muir is a leaf, not a router.** A datagram whose hardware destination is
neither an address on this machine's cable nor the broadcast address is
dropped rather than forwarded; AIM-628 chapter 6's routing is a bridge's
job. A frame goes out over UDP only when a station of this process put it on
the cable, so what arrives from one peer is never sent on to another, and a
broadcast goes to the named peers and never to the route of last resort.
`tests/chudp.rs` holds each of those.

## Unverified

**Whether a real interface reads Transmit Abort from a busy receiver.**
Both sides of it are measured here, in [the abort
signal](#the-abort-signal-in-both-its-uses): a netlist board stopped by a
behavioral interface, and a behavioral interface stopped by the netlist
board, each sender reading the bit. But both parties are this program.
Two netlist boards on one ether is not buildable in this harness either,
an ether carrying one behavioral board and one transceiver, so what is
held is the model against itself with the board's own gates on one side of
each measurement. **What would settle it:** the interface on a real cable
or in fabric, one made busy and the other made to send to it, with the
sender's CSR read.

**A real 9401's pin names.** The transmit generator's three
select pins are grounded, so its code is 0 whichever naming is used, but the
receive generator takes `RACT` on the pin the data sheet's two diagrams
disagree about, and there the naming decides the polynomial. The board
requires pin 3 to be the low bit of the code: with it, the receiver divides
by the reciprocal of the transmitter's polynomial, which is what a frame
sent in reverse bit order needs and what the sheet itself calls CRC-16
reverse; with the wire list's naming it divides by an unrelated degree-eight
polynomial and cannot check a frame the board has just sent, which
`the_receive_generator_divides_by_the_reciprocal_polynomial` in
`tests/chaos_netlist.rs` measures both ways. So MIT's wire list and this
board's behavior cannot both be right about the names. **The conflict is not
about wiring**: both sources agree exactly which net reaches which pin
number, and the wire list's function column comes from a CAD body library
that inherits the same contradictory sheet. **What would settle it:** a data
sheet whose connection diagram and logic symbol agree, or the part in hand.

**The check word's seed and shift direction have never met real
hardware.** They were fixed by matching the netlist board's own output, as
[the check word](#the-check-word) says, which is a simulator agreeing with
itself, and a pair of wrong guesses that cancelled would look exactly like a
right one. What the receive generator adds is a constraint rather than a
proof: the two dividers must be reciprocals for a frame to check, and they
are. **What would settle it:** a real 9401 on a real board, over a frame
whose words are published, or an MIT file that states the arrangement.

**The appended source word is covered by the divider, and that one is not
fitted.** The interface appends the source itself, and driving the netlist
board's own two 9401s over one frame while changing only that word changes
the check word: `3050` gives `135771`, `3060` gives `135651`, `177100`
gives `25206`, `177101` gives `125203`. Were the appended word outside the
division, all four would be equal. So its inclusion is held by the wiring
and the part rather than by the constant that was fitted to them.

**Whether the receive buffer reads as zeros above the last bit of a short
run.** A receiver that took wreckage has a partial word at the top of its
buffer, and the software reads sixteen bits down from the word boundary
above the last bit stored --- so what comes back with it is whatever the RAM
held there. Both the netlist board and the model read zeros in the tests,
but every such test sends the wreckage first. **What would settle it:** a
longer packet received first, then a short run of bits, with the top word
read back.

**What `usim` and `klh10` write as CHUDP.** Only `cbridge` was watched, and
it is the authority here because a muir on a real Chaosnet reaches one
through `cbridge`. **What would settle it:** watching one of them, or one
interoperation.

**Whether MIT set subnet 376 aside.** muir's default address, `177001`, is
on subnet 376 --- 254 decimal, the subnet being the address's high byte ---
so that a run started without `--chaos-address` cannot answer where a real
Chaosnet put somebody. That such a subnet is set aside for private use is
the Global Chaosnet's own convention and is **not** in any MIT file here;
no MIT document in this tree reserves a subnet, and no host table in this
tree names a host on subnet 376, so no band here calls it. **What would
settle it:** a copy of MIT's network-wide host table, rather than the
release's trimmed one.

**What a System 100 band does with an address on that subnet is measured,
not unverified: it never reaches a listener.** The band's routing table is
96 entries --- `(DEFCONST ROUTING-TABLE-SIZE 96.)` at
`vendor/system-100-0/sys/network/chaos/chsncp.lisp:253`, where muir-sys's
System 1003 has `256.` at
`vendor/system-1003/sys/network/chaos/chsncp.lisp:259`, as System 1002
has --- and
`RESET-ROUTING-TABLE` writes `(AREF ROUTING-TABLE MY-SUBNET)`, and the cost
and type tables likewise, with no bounds test (System 100's file, lines 311
to 317);
the send path does test a subnet against the table's length (lines 1934 and
1937). So a subnet of 96 or more is past the end of the table the
reset writes. Booted on `micro` off the System 100 pack with no
`--chaos-address`, the band stops at cold boot in the error handler:

    >>TRAP 4302 (SUBSCRIPT-OOB M-Q M-S (NIL XAREF-RECHECK-INDEX) M-A)
    The subscript 254 for #<ART-16B-96 10730160> was out of range in SYSTEM:SET-AR-1.
    While in the function
    CHAOS::RESET-ROUTING-TABLE
     ← CHAOS::INITIALIZE-NCP-COLD
     ← SYSTEM:EVAL1

The array prints its own size, `ART-16B-96`, so the machine gives the
constant back without the file being read. Under it the handler offers "Use
different subscripts" and "Abort the CHAOS-NCP initialization", and
unattended nothing takes either: the screen was the same 150 million
microcycles on. **The table's length decides this, not the number 376**: at
`57401`, subnet 95, the same band goes past the reset and cold-boots, and at
`60001`, subnet 96, it traps with the subscript 96. A file and time host on
the cable makes no difference --- the band at `177001` traps with one at
3060 as without. **The same band at `3050` boots to its listener**, where
`chaos:my-subnet` reads 6, `(array-length chaos:routing-table)` and the cost
and type tables all read 96, and `(aref chaos:routing-table
chaos:my-subnet)` reads 1576, which is 3050 octal: its own address, which is
what the reset writes there.

**The memo's round and the board's round.** AIM-628 §2.6, memo page 8,
gives "a typical value for the token's round-trip time" as 64 microseconds.
The board as wired makes a round 256 slots of 1,000 ns, which is 256
microseconds --- four times that, and exactly what 256 slots of a quarter
microsecond would have been. Whether the memo is describing a different slot
length, a different network, or this board before a change is not settled
here; the board is followed, and its slot and round are measured above.
**What would settle it:** an MIT drawing or note giving the interface's
intended slot length, or a revision of the LMTURN divider.
