// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The command line of muir's two executables, `cadr` and `quux`: one
//! engine at a time from the boot PROM, a pack on the disk controller's
//! cable, and microcycles per second against the machine's own rate.
//! **The executable is the machine**: `cadr` is MIT's CADR as built and
//! `quux` the CADR evolved, and each takes only the flags that mean
//! something on its own machine. [`run`] is both, given the machine;
//! `src/bin/cadr.rs` and `src/bin/quux.rs` are each one call of it.
//!
//! The reference is the machine's own microcycle, read off the delay-line
//! taps: 145 ns at normal speed, so 6.9 M microcycles/s.
//!
//! `cadr --help` and `quux --help` list the flags each takes: its usage
//! and the entries of `HELP` that are its own.
//!
//! `cargo build --release` leaves them at `target/release/cadr` and
//! `target/release/quux`, and `cargo install --path .` puts both on the
//! path. The programs in `examples/` stay `cargo run --example` programs.
//!
//! The engine flags are mutually exclusive and `--rtl` is the default,
//! which is the engine for ordinary use: the models throughout, at about
//! twice the machine's own rate. `chip` is the reference, and it runs
//! every board as a netlist. Each board flag takes `model` instead ---
//! `rtl`'s twin of that board, the same timing, no gates --- which is how
//! one board is taken out of the picture while something else is under
//! investigation, rather than a machine to run for its own sake.
//! On `cadr`, `--main-memory` is main memory's board and
//! `--main-memory-boards` is how many 64K-word boards the machine has ---
//! main memory on every engine, the boards on the Xbus on `chip` --- 32 by
//! default for the two million words. QUUX has no memory boards: on
//! `quux`, `--main-memory-size` is how much main memory, in whole
//! megawords with the unit written, `--main-memory-size 32MW`.
//! `--io-board` is the I/O board and `--tv` the display.
//! `--tv-board` is which display, **on every engine**: the SIMPLE TV, the
//! black-and-white board System 100 drives, or the LISPM TV that replaced
//! it in December 1980. One model serves either board --- MIT's own
//! `cadrtv/lmtv.order` is the LISPM TV's specification and both boards
//! program by it --- so the flag chooses the netlist `chip` builds the
//! backplane with and the board every engine's model answers as, and the
//! start says which it is. `--color-tv` fits a **second** display board,
//! the color TV: a LISPM TV strapped to the other addresses the same sheet
//! gives, the buffer at `17200000` and the registers at `17377750`, "for
//! the color TV, x is 5". The main screen stays at `17000000` whichever
//! board `--tv-board` named, the release hardwiring `MAIN-SCREEN` to one
//! bit a pixel there. It is off by default, and a machine without it
//! answers those addresses with an NXM, which is how `COLOR-EXISTS-P`
//! finds out there is no color screen; with it, the cold boot's
//! `COLOR:SETUP` loads the NTSC sync program, starts it in clock mode 3
//! with vertical spacing 36, and writes the color map. The picture is 576
//! by 454 at four bits a pixel through that map. The flag takes a word of
//! its own, `--color-tv [netlist|model]`, as `--tv` does: the netlist is
//! `chip`'s, a second LISPM TV on the backplane wrapped to the color
//! addresses, and the model is every engine's. The bare flag is the
//! netlist on `chip` and the model elsewhere, and `--color-tv netlist` off
//! `chip` is refused by the engine's name. `--disk-controller`
//! is the disk controller, whose netlist runs its own microcode with the
//! pack on its cable as a drive and takes the drive's time over every
//! block, milliseconds where the model takes none. **A run that touches no
//! pack pays about 3% for that, and a run that reads one pays days**:
//! System 100 booted through the netlist controller on 12 September 2026
//! in 2 days 14 hours and 301 million microcycles, 51 hours of it the cold
//! boot's copy of all 21,342 pages of the band. The controller's sequencer
//! waits on the drive's clocks and on its own delay lines, so it cannot be
//! untimed the way the model is, and `--disk-controller model` is how a
//! `chip` run that does not care about the disk is made quick.
//! `--main-memory model` takes the disk controller down with it, the
//! netlist controller being a second master on the Xbus that the model
//! memory does not answer; asking for that memory and
//! `--disk-controller netlist` together is refused, being a backplane that
//! cannot be built. Either way the start says which controller the run
//! has. The other engines run the models always.
//! `--disk-pack` takes a pack image --- the System 100 release's
//! `disk-sys-100-0.img` --- and there is no default: no flag is a drive
//! with no pack in it. The image is the pack's blocks end to end, 256 words of 32 bits
//! each, in the geometry's order. It is opened read-write and written as a
//! drive writes its pack. After the image, in any order, come the
//! drive's unit --- 0 unless a DISK MULTIPLEXOR is fitted with
//! `--disk-multiplexor`, the netlist controller having one port of its
//! own ---; `ro`, the file opened read-only behind a drive that is
//! writable all the same, a written block staying in the run and a
//! checkpoint, as `quux`'s `ro` does; and `wp`, the drive's read-only
//! switch, `STATUS<7>`, a write then faulting as MIT says it does and
//! nothing reaching the file. With
//! no pack at all the engines run the same PROM waiting on a drive that
//! never answers, which measures the wait loop.
//!
//! Every run serves a terminal: the display, the keyboard and the mouse
//! over RFB, RFC 6143, so that any VNC viewer can work the machine, which
//! has no other way to be worked. It is at VNC's display :0 on the loopback,
//! `vnc://127.0.0.1:5900`, and the start says where it is; a display
//! already taken --- a second muir on the host, which is what the lashup
//! over TCP is --- moves it up to the first free one. `--terminal` says
//! where instead: a port, and an address before it to listen anywhere but
//! the loopback --- `--terminal 5900` for this machine only, `--terminal
//! 0.0.0.0:5900` to let another one in, which is worth meaning, because
//! RFB's `None` security is the only type offered and a viewer needs no
//! password. A port that is named is bound as it stands and the run stops
//! if it cannot be, rather than serving a viewer somewhere it was not told
//! to look. What it shows is the frame buffer, which
//! is the screen on every engine; the monitor on the netlist board's video
//! cable is a separate thing and not built. It keeps serving the last
//! screen for as long as a viewer is looking at it once the run has
//! stopped. In the lashup the other machine is served a terminal too, the
//! display above this machine's, and `--debuggee-terminal` puts that
//! elsewhere. With `--color-tv` the color screen is served as well, at
//! the display above the last one bound or where `--color-terminal` says,
//! and it is **pixels only**: the machine has one keyboard and one mouse,
//! both on the I/O board, and they stay with the terminal that serves the
//! main screen, so what a viewer types or points at the color screen is
//! dropped. `--tv-capture` records the main screen and
//! `--color-tv-capture` the color one, each to a file of its own.
//!
//! **Every `rtl` and `chip` run listens for a debugger too**, since the
//! bus interface's DBGIN is on every machine: it takes the Unibus as
//! master when a debugger drives its cable, nothing in the machine enables
//! it, and the microcode neither knows nor can refuse. The connector is
//! at 127.0.0.1:7661, or the port above it when that one is taken by
//! another muir, and the start says where; a debugger connecting to it ---
//! `--debug-cable-connect` in another muir, or any program speaking the
//! cable's frames --- is plugged in between two microcycles, and the two
//! run in step from that instant until the debugger is done or goes away,
//! when the machine runs on its own again and listens again. A debugger
//! reads and writes the whole Unibus and stops the clock, so the connector
//! stays on the loopback unless `--debug-cable-listen` names an address.
//! `--no-debug-cable-listen` leaves it empty, as do the flags that want a
//! machine on its own --- `--checkpoint`, `--tv-capture`,
//! `--color-tv-capture`, and `--watch` on `chip` --- which the start says.
//! `micro` has no timing model and no end of the cable, and says so. The
//! debugger's own end, `--debug-cable-connect`,
//! runs the machine inside the cable it plugged into a debuggee, and its
//! DBGIN is not listened at.
//!
//! **What a viewer types waits for the machine's next look, and a queue
//! that fills loses the oldest keystroke** ---
//! [`crate::terminal::INPUT_BACKLOG`] events of it --- so that a machine
//! that has stopped reading its keyboard cannot grow a queue for the
//! length of the run. **A run says what went**: once, from the first
//! keystroke it loses, and every time the count changes under
//! `--keyboard-mapping-trace`, which is the flag for a key that will not
//! type. A keystroke lost in silence is a character that does not type
//! with nothing to tell it from a mapping that has no binding for the key.
//! Below the terminal the keyboard holds a queue of its own,
//! [`crate::terminal::keyboard::BACKLOG`] words the machine takes one at a
//! time, and a keystroke it has no room for is refused whole and said the
//! same way: once unasked, and as itself under the trace, which used to
//! call it sent. The pointer's queue loses its oldest too and nothing is
//! said: a viewer sends where the pointer is rather than how far it moved,
//! so the newest is the one that matters and that one is always kept.
//!
//! **A band wants a file and time host, and muir is not one**: a CADR had
//! no such server in it, and neither has this. The host is another program
//! on the network --- `ozd`, `https://github.com/metebalci/ozd`, is one
//! that boots a band. Two flags reach it, because they are two things on
//! the board: `--chaos-address` is this machine's own sixteen address
//! switches, which are set whether or not anything is plugged in, and
//! `--chaos-udp` is the cable, Chaosnet over UDP at port 42042 unless it
//! says otherwise. **Without the cable muir sends nothing**, as a machine
//! with none talks to nobody however its switches read. Every host
//! `--chaos-udp-peer` names is then a station on that cable, taking its
//! turn on it. Which numbers a run wants are its band's:
//! `--chaos-address 3050 --chaos-udp` with
//! `--chaos-udp-peer 3060@<where the host is>` for the System 100 pack.
//! Every engine has a Chaosnet.
//! muir stays a leaf: a frame goes out only when this machine put it on
//! the cable, so one peer's is never carried on to another, and a
//! `cbridge` beside it is what routes.
//! `--chaos-udp-default-peer` is where that bridge is: a frame whose
//! destination no `--chaos-udp-peer` named goes there rather than
//! nowhere, which is the route of last resort and the whole of muir's
//! routing. Naming the bridge as a peer would not do it --- a peer entry
//! places one address --- and a broadcast is not handed to it. A machine
//! that reaches no host --- no `--chaos-address`, or one at a number its
//! band does not call --- stops in the debugger at the initialization
//! that wants a host: `Super-B` there, then the date and time it asks
//! for and `y`, finish it.
//!
//! The machine's other way out is the serial port at J9, the 2651 at
//! IOBSER 0A12, and `--serial <endpoint>` is where it is reached: a TCP
//! port, or address:port, attached to with `nc` or `telnet`. A connection
//! is the device on the null-modem cable plugging in --- `DSR`, `DCD` and
//! `CTS` asserted, which is what the chip needs before it will transmit or
//! receive at all --- and hanging up drops them; one device at a time. The
//! rate and the frame are the machine's, whatever it programmed into the
//! chip, and nothing at this end sets or checks them, so a far end that
//! assumes another rate reads garbage as it would on a real line. The port
//! is off unless the flag is given: nothing needs it to work the machine,
//! and on `chip` a port the machine has opened counts the baud-rate
//! crystal and the I/O board stops idling. It is one machine's, so it is
//! refused with the lashup.
//!
//! Separately, and on every engine: the band's cold boot leaves the
//! display's vertical interrupt off, so the mouse is not tracked until
//! `(si:setup-cpt)` is typed at the listener. That is the band's own ---
//! `LISP-REINITIALIZE` guards its `SETUP-CPT` block with `(UNLESS (NOT
//! CALLED-BY-USER) ...)`, which the cold boot's `(LISP-REINITIALIZE NIL)`
//! does not satisfy --- so it is wanted just as much on a boot that
//! reached the listener with no trouble. `tests/vertical.rs` holds that:
//! it boots with a Chaosnet, gets to the prompt, and finds the interrupt
//! still off.
//!
//! The prompt is muir's own line on stdin while a machine runs on its
//! own, or at either end of the debug cable to another program or to
//! the fabric: `boot`, `hold`, `continue`, `step`, `pc`, `reg`, `amem`,
//! `mmem`, `dmem`, `pdl`, `spc`, `screenshot`, `startcapture`,
//! `endcapture`, `info`, `checkpoint`, `quit` and `help`,
//! [`crate::prompt`], read from a pipe or from a terminal muir is in the
//! foreground of, and acted on between two microcycles. An end of the
//! cable refuses `checkpoint` and the capture commands, saying why, as
//! the command line refuses `--checkpoint` and `--tv-capture` there; the
//! lashup in one process alone has no prompt, and its start says so.
//! `muir: ` is written while the machine is held, to a terminal and not
//! to a pipe; a line typed while it runs is acted on all the same. ^C
//! holds the machine at the prompt; ^C while held, or with no prompt to
//! go on from, ends the run as `quit` does. `--no-auto-boot` leaves the
//! boot button unpressed, as a CADR is when the power comes on, and
//! starts the run held for the prompt's `boot` to press it; a hold
//! nothing can run on --- stdin having ended --- ends the run rather
//! than standing there. `continue` on a halted machine, its `RUN`
//! clear, sets `RUN` as a console does and runs it on from where it
//! stands, with no reset: that is how a checkpoint taken halted, as a
//! board takes one, runs on, and `--continue` does it at the start.
//!
//! A machine that stops itself is held at the prompt and says so, rather
//! than being run on through: `HALT-CONS` under `ERRSTOP` --- what System
//! 100's `(si:%halt)` runs --- and the statistics counter under `STATHENB`
//! both drop `MACHRUN` with `RUN` still set, and no microcycle runs from
//! there. Nothing about stepping says so, the screen simply stops, so the
//! run loop reads it off `FLAG-1` where a console would. `boot` presses
//! the button that starts it again. `chip` holds at the prompt the same
//! way, reading the nets the spy registers are buffered from, since it is
//! not an `Engine` and has no registers to read.
//!
//! A run goes on until a stop, a halt or ^C. `--stop-after` ends it after
//! that many microcycles, `--stop-at` when the PC reaches an address with
//! the boot PROM disabled, `--stop-at-prom` with it enabled --- the CADR's
//! PROM and control store share their low addresses, so a PC alone names
//! two places. QUUX's PROM is never disabled and has addresses of its own,
//! 36000-37777: there `--stop-at` is a PC outside them, `--stop-at-prom` a
//! PC inside them, given as the control-store address. A PC holding a
//! control-store write's address stops neither. Addresses are octal, as
//! MIT writes them, and whichever stop comes first wins. The rate reported
//! at the end is over the whole run, and past the boot the work is
//! cheaper, so a longer run reports a higher one.
//!
//! The boot PROM is MIT's own `mit/sys/ubin/promh.mcr`, so nothing here
//! needs `vendor/`. `--prom` runs another one instead, out of an MCR
//! microcode file as MIT's own is; the start says how the file stands to
//! MIT's own, because recovered copies of the boot PROM are not all the
//! same program. A checkpoint carries the 512 words it ran, so `--resume`
//! brings its own and the two flags are refused together.
//!
//! `--checkpoint` and `--resume` work on all three engines. On `micro`
//! and `rtl` a checkpoint is [`Machine`] and the engine's own state; on
//! `chip` there are no arrays to write, so it is the boards --- every
//! net, every part's cells, every oscillator, one-shot and delay-line
//! transition in flight on the processor, the bus interface, the memory
//! boards, the I/O board and the display --- with the machine behind the
//! buses and what each end of each bus is driving onto the others. It is
//! taken at the first microcycle from the stop with no bus cycle in
//! flight, which is the only kind of instant it does not describe, and
//! those microcycles are counted and said. A netlist disk
//! controller's drives are on its own cable rather than in the machine,
//! and the multiplexor between them on its connector rather than the
//! backplane; both are in a checkpoint too, each where it stood.

use std::io::IsTerminal;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::cable::{Boards, DebugIn, FarEnd};
use crate::capture::{ColorRecorder, Recorder, local_time, wall_clock};
use crate::checkpoint::Checkpoint;
use crate::chip::Chip;
use crate::clock::{Behavioral, Clock, TimingModel};
use crate::disk_unit::{Geometry, Unit};
use crate::engine::Engine;
use crate::isa::Insn;
use crate::lashup::{CableEnd, Connector, FreeRunning, Lashup, Plug, Remote, Turn};
use crate::machine::{Machine, megawords};
use crate::micro::Micro;
use crate::netlist;
use crate::part::Level;
use crate::prompt::{Command, Memory, NetName};
use crate::rtl::Rtl;
use crate::serial::Endpoint;
use crate::terminal::glass_tty::{Glass, GlassTty};
use crate::terminal::keyboard::{BootKeys, Keyboard, Mapping};
use crate::terminal::mouse::Mouse;
use crate::terminal::{Frame, Terminal};
// Which display board `--tv-board` puts on the backplane, on every engine.
// The model's own type, since the board a machine has is the machine's and
// not this program's: it names the netlist `chip` builds the backplane
// with and the board every engine's model answers as, and a checkpoint
// carries it so that a resume onto the other one is refused by the flag's
// name.
use crate::tv::Board as TvBoard;

/// How often the terminal is given a turn: about thirty times a second.
/// The machine's own raster is 64.7 Hz, so a viewer sees every other frame
/// at best, and the cost does not show in any engine's rate.
///
/// **The terminal has no thread of its own on purpose.** It was measured
/// rather than assumed, release build, one viewer reading as fast as it
/// can, 768 by 963, the screen scrambled so no run of equal bits flatters
/// the encoder: a poll costs 0.7 us with nobody connected, 100 to 200 us
/// on an idle screen, 0.4 ms for a hundred rows changed and 3.0 ms for the
/// whole screen. At thirty polls a second that is about 0.5% of wall time
/// idle, 1.2% scrolling, and 9% only if the whole screen repaints every
/// poll --- which [`terminal::Terminal::FULL_UPDATE_INTERVAL`] already caps
/// at one whole screen a raster frame. The share is the same on every
/// engine, the interval being wall clock rather than microcycles.
///
/// **1.2% does not buy a lock.** The frame buffer is written by the
/// processor, by the disk controller's DMA and by the netlist display's
/// mirror, and none of them has to know a viewer exists; a second thread
/// reading the screen would put a synchronization point into a part of the
/// machine that has none.
///
/// What would change the answer: a screen that really does repaint whole
/// at 30 Hz for long stretches; a terminal doing more per poll than
/// encoding raw rectangles, such as a compressed encoding or several
/// viewers wanting different pixel formats; this interval dropping to the
/// machine's own 64.7 Hz; or a profile of a real session putting the
/// terminal higher than these figures predict. **Those are one machine on
/// one day: acting on them means measuring again, not quoting them.**
const TERMINAL_INTERVAL: Duration = Duration::from_millis(33);

/// Microcycles between glances at the computer's clock to see whether
/// [`TERMINAL_INTERVAL`] has gone by. `Instant::now` is not free and
/// `micro` runs 66 M microcycles a second.
const TERMINAL_CHECK: u64 = 4_096;

/// How often the serial endpoint is given a turn, when `--serial` has
/// opened one: as often as the terminal.
///
/// A poll is a system call or two and a run reaches a check far more often
/// than a serial line has anything to say --- `micro` sixteen thousand
/// times a second. A character typed at the endpoint waits at most this
/// long to reach the port, which is about one character's own time at 300
/// baud, the rate MIT's `sys/io1/serial.lisp` defaults to.
const SERIAL_INTERVAL: Duration = TERMINAL_INTERVAL;

/// The longest a paced run waits at once: half the terminal's own
/// interval, so that a keystroke never waits on the pacing longer than it
/// already waits on the poll.  A run further ahead than this waits again
/// at the next check rather than in one long sleep.
const PACE_SLEEP: Duration = Duration::from_millis(16);

/// The least a paced run waits for; a lead shorter than this is carried to
/// the next check instead.  A host rounds a sleep up rather than down, and
/// the shorter the sleep the larger that rounding is as a share of it ---
/// a quarter of a millisecond asked for came back as four tenths where
/// this was measured --- so a run of very short waits pays the rounding on
/// every one of them and wakes the core thousands of times a second to do
/// it, which is the opposite of what `--pace` is for.
const PACE_FLOOR: Duration = Duration::from_millis(1);

/// **The machine's own speed, when `--pace` asks for it**: how long a run
/// that has got ahead of the hardware waits before its next microcycle.
///
/// It is here, beside the intervals the run loops already keep, because
/// nothing inside the machine knows what a wall clock is.  An engine keeps
/// the machine's own nanoseconds, [`Machine::ns`], and the only place
/// those meet the host's clock is the loop that also polls the terminal.
/// Keeping the arithmetic in one small thing with no clock of its own is
/// what lets the rule be tested without one: every instant below is passed
/// in.
///
/// **The rule.** The machine's nanoseconds since the anchor, against the
/// wall clock's since the same anchor.  Ahead, the difference is waited
/// for, [`PACE_SLEEP`] at most; behind, the anchor moves to now.  So a run
/// that loses time --- a heavy microcycle, a loaded host, `chip`, which is
/// far slower than the hardware and never waits at all --- comes back to
/// the machine's speed without sprinting past it to make the loss up.  A
/// run that stalled and then ran at nine times speed would be worse than
/// one that is simply late.
struct Pace {
    /// The wall clock where the machine's own clock was last set against
    /// it.
    from: Instant,
    /// The machine's nanoseconds there.
    ns: u64,
}

impl Pace {
    /// The anchor the run begins at.
    fn new(now: Instant, ns: u64) -> Pace {
        Pace { from: now, ns }
    }

    /// The anchor moved to here, for time the machine spent standing
    /// still: held at the prompt, or stepped by a debugger on the cable.
    /// What went by while it stood is not time it owes.
    fn anchor(&mut self, now: Instant, ns: u64) {
        self.from = now;
        self.ns = ns;
    }

    /// How long to wait now, if at all.  Nothing when the run is level
    /// with the machine or behind it, and nothing when it is ahead by less
    /// than [`PACE_FLOOR`]; behind, the anchor moves to `now`, so that the
    /// time lost is not a debt to be run off afterwards.
    fn owed(&mut self, now: Instant, ns: u64) -> Option<Duration> {
        let machine = Duration::from_nanos(ns.saturating_sub(self.ns));
        let wall = now.saturating_duration_since(self.from);
        let Some(ahead) = machine.checked_sub(wall) else {
            self.anchor(now, ns);
            return None;
        };
        (ahead >= PACE_FLOOR).then(|| ahead.min(PACE_SLEEP))
    }
}

/// The port a terminal is served at unless `--terminal` says another:
/// VNC's display :0, RFB's convention.
const TERMINAL_PORT: u16 = 5900;

/// How many displays up from where a terminal was asked for a free one is
/// looked for when the port was not named: VNC's :0 to :99, which is 5900
/// to 5999 from the default port.
const TERMINAL_DISPLAYS: u16 = 100;

/// The port a glass TTY is served at unless `--glass-tty` says another.
///
/// **This is muir's own number and no convention.** Telnet's port is 23
/// and a server on it needs privilege this has no business asking for,
/// so the protocol's own number is not available; 10023 is that number
/// with room in front of it, picked here and written down as a choice
/// rather than a fact.
const GLASS_TTY_PORT: u16 = 10023;

/// Where a glass TTY is served, and whether what is typed at it reaches
/// the machine: `--glass-tty [<endpoint>][,ro]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GlassAt {
    addr: SocketAddr,
    port_named: bool,
    read_only: bool,
}

/// `--glass-tty`'s argument: an endpoint, `ro`, either, both in either
/// order, or nothing at all.
///
/// The endpoint is [`endpoint`]'s --- nothing, a port, an address, or
/// address:port, on the loopback unless an address says otherwise --- and
/// `ro` is spelled as `--disk-pack` and `--file-root` spell it, because it
/// means the same thing: look and do not touch.
fn glass_spec(arg: Option<&str>) -> Result<GlassAt, String> {
    let mut endpoint_spec = None;
    let mut read_only = None;
    for part in arg.unwrap_or_default().split(',').filter(|p| !p.is_empty()) {
        if part == "ro" || part == "rw" {
            if read_only.replace(part == "ro").is_some() {
                return Err("ro or rw twice".into());
            }
        } else if endpoint_spec.replace(part).is_some() {
            return Err("the endpoint twice".into());
        }
    }
    let addr = endpoint(endpoint_spec, GLASS_TTY_PORT)
        .ok_or("wants nothing, a port, an address or address:port, and ro")?;
    Ok(GlassAt {
        addr,
        port_named: names_a_port(endpoint_spec),
        read_only: read_only.unwrap_or(false),
    })
}

/// Where a terminal is served, and how hard: the endpoint; whether the
/// port in it was named, since a named port is bound as it stands and an
/// unnamed one is only where the search for a free display starts; and
/// whether the flag was given at all, since a terminal that was asked for
/// and cannot be served stops the run, and one nobody asked for leaves the
/// run without a terminal and says so.
#[derive(Clone, Copy)]
struct TerminalAt {
    addr: SocketAddr,
    port_named: bool,
    asked: bool,
}

impl TerminalAt {
    /// The terminal nobody asked for: VNC's display :0 on the loopback,
    /// which is where an unauthenticated server belongs unless someone
    /// says otherwise in as many words.
    fn default_display() -> TerminalAt {
        TerminalAt {
            addr: SocketAddr::from((Ipv4Addr::LOCALHOST, TERMINAL_PORT)),
            port_named: false,
            asked: false,
        }
    }
}

/// The terminal `at` says: bound where it says when the port was named,
/// and otherwise at the first free display from there up, over
/// [`TERMINAL_DISPLAYS`] of them --- a second muir on one host, which is
/// what the lashup over TCP is, then has a display of its own rather than
/// the first one's port. `Err` is why there is no terminal, which the
/// caller makes fatal or not.
/// The glass TTYs `at` asks for, bound.
///
/// A named port is bound as it stands and a run that cannot have it stops:
/// somebody told a person where to attach. An unnamed one moves up until
/// it finds a free port, so that `--glass-tty --glass-tty` is two of them
/// rather than a collision, and so that a second muir on one host has its
/// own.
fn bind_glass(at: &[GlassAt]) -> Result<Glass, String> {
    let mut ttys = Vec::new();
    for want in at {
        let mut addr = want.addr;
        let last = want.addr.port().saturating_add(TERMINAL_DISPLAYS - 1);
        let tty = loop {
            match GlassTty::bind(addr) {
                Ok(t) => break t,
                Err(e) if want.port_named => return Err(format!("--glass-tty {addr}: {e}")),
                Err(e) if e.kind() != std::io::ErrorKind::AddrInUse => {
                    return Err(format!("--glass-tty {addr}: {e}"));
                }
                Err(_) if addr.port() < last => addr.set_port(addr.port() + 1),
                Err(e) => {
                    return Err(format!(
                        "--glass-tty {}: no free port in {}-{}: {e}",
                        addr.ip(),
                        want.addr.port(),
                        last
                    ));
                }
            }
        };
        let mut tty = tty;
        tty.trace = true;
        tty.read_only = want.read_only;
        ttys.push(tty);
    }
    Ok(Glass::new(ttys))
}

fn bind_terminal(at: TerminalAt) -> Result<Terminal, String> {
    let last = at.addr.port().saturating_add(TERMINAL_DISPLAYS - 1);
    let mut addr = at.addr;
    loop {
        match Terminal::bind(addr) {
            Ok(mut t) => {
                t.trace = true;
                return Ok(t);
            }
            Err(e) if at.port_named => return Err(format!("{addr}: {e}")),
            Err(e) if e.kind() != std::io::ErrorKind::AddrInUse => {
                return Err(format!("{addr}: {e}"));
            }
            Err(_) if addr.port() < last => addr.set_port(addr.port() + 1),
            Err(e) => {
                return Err(format!(
                    "{}: no free display in {}-{}: {e}",
                    addr.ip(),
                    at.addr.port(),
                    last
                ));
            }
        }
    }
}

/// The port the debug cable meets at: 7661 for DBGOUT's Unibus address
/// 766100. IANA leaves 7649-7662 unassigned (its registry, read 6 Sep 2026)
/// and it is below the ranges macOS and Linux hand out to clients.
const DEBUG_CABLE_PORT: u16 = 7661;

/// How many ports from [`DEBUG_CABLE_PORT`] up a connector nobody placed
/// may take, the port being taken --- by a second muir on the host, which
/// is what the lashup over TCP is: 7661 and 7662, the two IANA leaves
/// unassigned there.
const DEBUG_CABLE_PORTS: u16 = 2;

/// Where DBGIN's connector listens: the endpoint, whether its port was
/// named --- bound as it stands, then, as `--terminal`'s is --- and whether
/// it was asked for at all, which decides whether a port that cannot be
/// bound stops the run or leaves it without a connector.
#[derive(Clone, Copy)]
struct CableAt {
    addr: SocketAddr,
    port_named: bool,
    asked: bool,
}

impl CableAt {
    /// The connector nobody placed: 7661 on the loopback, where a debugger
    /// that reads and writes the whole Unibus and stops the clock belongs
    /// unless someone says otherwise in as many words.
    fn default_port() -> CableAt {
        CableAt {
            addr: SocketAddr::from((Ipv4Addr::LOCALHOST, DEBUG_CABLE_PORT)),
            port_named: false,
            asked: false,
        }
    }
}

/// The listener for DBGIN's connector at `at`, or why there is none: as
/// [`bind_terminal`], a port that was named is bound as it stands, and one
/// that was not moves up while the port is taken, [`DEBUG_CABLE_PORTS`] of
/// them.
fn bind_cable(at: CableAt) -> Result<std::net::TcpListener, String> {
    let last = at.addr.port().saturating_add(DEBUG_CABLE_PORTS - 1);
    let mut addr = at.addr;
    loop {
        match std::net::TcpListener::bind(addr) {
            Ok(l) => return Ok(l),
            Err(e) if at.port_named => return Err(format!("{addr}: {e}")),
            Err(e) if e.kind() != std::io::ErrorKind::AddrInUse => {
                return Err(format!("{addr}: {e}"));
            }
            Err(_) if addr.port() < last => addr.set_port(addr.port() + 1),
            Err(_) => return Err(format!("{} and {last} are both taken", at.addr.port())),
        }
    }
}

/// Where `--debug-cable-connect` puts the debuggee: at an endpoint on the
/// network, which is another program speaking the cable's frames, or
/// behind a window of memory-mapped registers, which is a CADR in FPGA
/// fabric on the board muir is running on ([`crate::fabric`]).
///
/// One flag rather than two, because it is one concept --- this machine is
/// the debugger and here is the debuggee --- and the argument says which
/// it is: `0x` is unambiguous against a port, a host name and a host with
/// a port, so nothing has to be remembered about which flag takes which. A
/// host genuinely named `0x…` is not supported.
#[derive(Clone, Copy)]
enum Connect {
    Endpoint(SocketAddr),
    Window(u64),
}

/// Whether a debug cable flag's argument names the fabric's register
/// window rather than an endpoint: `0x` or `0X`.
fn names_a_window(spec: Option<&str>) -> bool {
    spec.is_some_and(|v| v.starts_with("0x") || v.starts_with("0X"))
}

/// The physical address the fabric's register window is at, out of a
/// `--debug-cable-connect 0x…`: hexadecimal after the prefix, and a
/// multiple of four, the window being 32-bit registers and every access to
/// it one 32-bit load or store. Page alignment is not asked for, which
/// would be a needless restriction. There is no default: where the window
/// sits is a property of the bitstream and muir holds no opinion about it.
fn window_address(flag: &str, spec: &str) -> u64 {
    let want = format!(
        "{flag} {spec}: the fabric's register window is 0x and a physical address in hexadecimal, \
         a multiple of 4"
    );
    let at = u64::from_str_radix(&spec[2..], 16).unwrap_or_else(|_| usage(&want));
    if !at.is_multiple_of(4) {
        usage(&want);
    }
    at
}

/// A pack flag's argument: the image, and after commas in any order the
/// drive's unit, `ro` for a file that is never written, and `wp`, the
/// drive's read-only switch.
#[derive(Debug, PartialEq, Eq, Clone)]
struct Pack {
    path: PathBuf,
    unit: usize,
    /// The file is opened read-only: a written block stays in the run and
    /// goes into a checkpoint. `ro`, and `wp` too, whose switch lets no
    /// write reach the drive at all.
    read_only: bool,
    /// The drive's read-only switch, `STATUS<7>`: `wp`, the CADR's alone.
    write_protect: bool,
}

/// `<image>[,<unit>][,ro][,wp]`, the parts after the image in any order:
/// unit 0, read-write and the switch off unless said, `rw` allowed for
/// saying so. `wp` with `rw` is refused: the switch keeps every write from
/// the file.
fn pack_spec(arg: &str) -> Result<Pack, String> {
    let mut parts = arg.split(',');
    let path = match parts.next() {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => return Err("wants an image".into()),
    };
    let (mut unit, mut read_only, mut write_protect) = (None, None, false);
    for part in parts {
        if part == "ro" || part == "rw" {
            if read_only.replace(part == "ro").is_some() {
                return Err("ro or rw twice".into());
            }
        } else if part == "wp" {
            if std::mem::replace(&mut write_protect, true) {
                return Err("wp twice".into());
            }
        } else if !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()) {
            let u = part.parse().ok().filter(|&u| u < crate::disk_controller::UNITS);
            let u = u.ok_or_else(|| format!("unit {part} is not one of 0 to 7"))?;
            if unit.replace(u).is_some() {
                return Err("the unit twice".into());
            }
        } else {
            return Err(format!("{part:?} is neither a unit, ro nor wp"));
        }
    }
    if write_protect && read_only == Some(false) {
        return Err("wp with rw: the switch keeps every write from the file".into());
    }
    Ok(Pack {
        path,
        unit: unit.unwrap_or(0),
        read_only: write_protect || read_only.unwrap_or(false),
        write_protect,
    })
}

/// `--chaos-udp-peer`'s argument, `<address>@<host>[:<port>]`: a
/// Chaosnet host and where it lives. The address is octal or
/// `subnet:host`, as `--chaos-address` takes it; the host is a name or an
/// address, and the port may be left off for the protocol's own.
///
/// **The name is resolved here**, once, before a machine is built, so
/// that a name with no address is a refusal at the start rather than a
/// peer that is never reached. A name that moves afterwards is not
/// followed; naming the address instead is what covers that.
fn peer_spec(arg: &str) -> Result<(u16, SocketAddr), String> {
    let (address, lives) = arg.split_once('@').ok_or("wants <address>@<host>:<port>")?;
    let a = crate::chaos::parse_address(address)
        .ok_or_else(|| format!("{address} is not an address in octal or subnet:host"))?;
    let at =
        resolved(lives).ok_or_else(|| format!("{lives} has no address this host can reach"))?;
    Ok((a, at))
}

/// A `<host>[:<port>]` as the one endpoint it names, the port left off
/// taking CHUDP's own. The lookup happens here and not again.
fn resolved(lives: &str) -> Option<SocketAddr> {
    let first = |s: String| s.to_socket_addrs().ok().and_then(|mut a| a.next());
    first(lives.to_string()).or_else(|| first(format!("{lives}:{}", crate::chaos::udp::PORT)))
}

/// `--chaos-udp-default-peer`'s argument, `<host>[:<port>]`: where a
/// directed frame goes whose destination no `--chaos-udp-peer` named.
///
/// **It takes no Chaosnet address**, which is what tells it from
/// `--chaos-udp-peer <address>@<host>`. It is not a host at an address;
/// it is where what is not named goes, and the CHUDP frame carries the
/// real destination in its trailer for the bridge there to
/// route on. So it is an endpoint as the other endpoint flags take one
/// --- a bare port on the loopback, an address at the protocol's own
/// port, or address:port --- or a name, resolved here as a peer's is.
fn default_peer_spec(arg: &str) -> Option<SocketAddr> {
    endpoint(Some(arg), crate::chaos::udp::PORT).or_else(|| resolved(arg))
}

/// A pack flag's argument parsed, or the usage. Which units a run can
/// fill is the controller's business and not the flag's: see
/// `--disk-multiplexor`.
fn pack_flag(flag: &str, arg: Option<String>) -> Pack {
    let arg = arg.unwrap_or_else(|| usage(&format!("{flag} wants <image>[,<unit>][,ro][,wp]")));
    pack_spec(&arg).unwrap_or_else(|e| usage(&format!("{flag} {arg}: {e}")))
}

/// An endpoint from a flag's argument, against a default: nothing is the
/// default, a port is that port at the default's address, an address is
/// the default's port there, and address:port is itself. A bare port is
/// on the loopback address, which is where an unauthenticated server
/// belongs unless someone says otherwise in as many words.
fn endpoint_at(spec: Option<&str>, default: SocketAddr) -> Option<SocketAddr> {
    let Some(v) = spec else {
        return Some(default);
    };
    if let Ok(a) = v.parse::<SocketAddr>() {
        return Some(a);
    }
    if let Ok(p) = v.parse::<u16>() {
        return Some(SocketAddr::new(default.ip(), p));
    }
    v.parse::<IpAddr>().ok().map(|ip| SocketAddr::new(ip, default.port()))
}

/// A second screen's display against the one below it: the display above
/// it unless the flag names another. A free display is looked for by
/// moving the port up, so this against the endpoint that was asked for and
/// this against the one that was bound differ only in the port each
/// already carries.
fn display_above(spec: Option<&str>, base: SocketAddr, flag: &str, what: &str) -> SocketAddr {
    let above = base
        .port()
        .checked_add(1)
        .unwrap_or_else(|| usage(&format!("--terminal: no port above this one for {what}")));
    endpoint_at(spec, SocketAddr::new(base.ip(), above)).unwrap_or_else(|| {
        usage(&format!("{flag} wants nothing, a port, an address or address:port"))
    })
}

/// The other machine's display against this machine's: the display above
/// it unless `--debuggee-terminal` says another.
fn debuggee_endpoint(spec: Option<&str>, base: SocketAddr) -> SocketAddr {
    display_above(spec, base, "--debuggee-terminal", "the other machine's display")
}

/// The color TV's screen against the display below it: the one above it
/// unless `--color-terminal` says another.
fn color_endpoint(spec: Option<&str>, base: SocketAddr) -> SocketAddr {
    display_above(spec, base, "--color-terminal", "the color TV's screen")
}

/// Whether a flag's endpoint names a port: nothing and a bare address do
/// not, a bare port and address:port do.
fn names_a_port(spec: Option<&str>) -> bool {
    spec.is_some_and(|v| v.parse::<SocketAddr>().is_ok() || v.parse::<u16>().is_ok())
}

/// [`endpoint_at`] the loopback at `port`.
fn endpoint(spec: Option<&str>, port: u16) -> Option<SocketAddr> {
    endpoint_at(spec, SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
}

/// `--serial`'s endpoint: a port, on the loopback, or an address and a
/// port.
///
/// **The port has to be named.** The other endpoints have a default to
/// fall back on --- VNC's display :0, the debug cable's 7661 for DBGOUT's
/// Unibus address --- and this one has none: the serial port is off unless
/// the flag is given, so a number here would be muir's own invention and
/// not something a viewer or a convention already knows. A bare address is
/// refused rather than bound where nobody was told to attach.
fn serial_endpoint(spec: &str) -> Option<SocketAddr> {
    if let Ok(a) = spec.parse::<SocketAddr>() {
        return Some(a);
    }
    spec.parse::<u16>().ok().map(|p| SocketAddr::from((Ipv4Addr::LOCALHOST, p)))
}

/// What the start says a terminal is: where a viewer connects to it, or
/// why there is none.
fn terminal_line(terminal: &Option<Terminal>, why: &Option<String>, at: SocketAddr) -> String {
    match (terminal, why) {
        (Some(t), _) => format!("vnc://{} --- RFB, no password", t.addr().unwrap_or(at)),
        (None, Some(why)) => format!("none --- {why}"),
        (None, None) => "none".to_string(),
    }
}

/// Serves each screen as it was left, while anyone is still looking at
/// any of them and until ^C.
///
/// **^C ends this by returning, not by ending the process.** The run's
/// handler is still on `SIGINT` --- [`catch_interrupts`] leaves it there
/// for the rest of the process --- so a ^C here is counted as one during
/// the run is, and [`interrupted`] against `seen`, the run's own count of
/// the ones it has acted on, is what stops the serving. The run function
/// then returns as it does from any other stop, and what it holds is
/// dropped on the way out: the cable's socket, the fabric's window.
/// Before this the count was read by nobody once the run had stopped, so
/// the line below was false on every run function but [`time_engine`],
/// which put `SIGINT`'s default back first --- and that ended the process
/// with nothing dropped, as `kill -KILL`, the way out on the other five,
/// does. Issue 103 met it on the fabric's run.
fn serve_last_screens(screens: &mut [(&mut Terminal, &crate::tv::Tv)], seen: &mut u32) {
    let looking =
        |screens: &[(&mut Terminal, &crate::tv::Tv)]| screens.iter().any(|(t, _)| t.viewers() > 0);
    // A ^C since the run last looked at the count --- while the checkpoint
    // was written, say --- is the stop it asked for, and is not to be
    // asked for twice.
    if !looking(screens) || interrupted(seen) {
        return;
    }
    eprintln!("terminal: serving the last screen while a viewer is on it; ^C to stop");
    while looking(screens) && !interrupted(seen) {
        for (terminal, tv) in screens.iter_mut() {
            terminal.poll(Frame::of(tv));
        }
        std::thread::sleep(TERMINAL_INTERVAL);
    }
}

/// One turn of a machine's terminal: the screen out and the keys and the
/// pointer in when it is time to poll, whatever the keyboard and mouse
/// hold delivered to the I/O board as it takes them, and the boot
/// sequence's word, once the board has decoded it, pressing the engine's
/// boot.
fn attend<E: Engine>(
    terminal: Option<&mut Terminal>,
    color: Option<&mut Terminal>,
    glass: Option<&mut Glass>,
    poll: bool,
    e: &mut E,
    keyboard: &mut Keyboard,
    mouse: &mut Mouse,
) {
    let m = e.machine_mut();
    // The color screen, when the board is fitted: the picture out and
    // nothing in.
    if poll
        && let Some(term) = color
        && let Some(tv) = m.color_tv.as_ref()
    {
        term.poll(Frame::of(tv));
    }
    if poll && let Some(term) = terminal {
        term.poll(Frame::of(&m.tv));
        for (keysym, down) in term.take_keys() {
            keyboard.key(keysym, down);
        }
        for (buttons, x, y) in term.take_pointers() {
            mouse.pointer(buttons, x, y);
        }
        if m.ioboard.take_beep() {
            term.ring();
        }
    }
    // The glass TTYs: the screen as text out, and what was typed in,
    // which goes to the same keyboard a viewer's keys do --- the machine
    // has one, on the I/O board, and everything that types shares it.
    if poll && let Some(glass) = glass {
        for (keysym, down) in glass.poll(Frame::of(&m.tv)) {
            keyboard.key(keysym, down);
        }
    }
    deliver_input(m, keyboard, mouse);
    e.keyboard_boot();
}

/// What the viewer typed and moved, to the machine's keyboard and mouse:
/// the I/O board's on the CADR, the register page's on QUUX (contract Q3).
fn deliver_input(
    m: &mut crate::machine::Machine,
    keyboard: &mut crate::terminal::keyboard::Keyboard,
    mouse: &mut crate::terminal::mouse::Mouse,
) {
    use crate::quux_input::KeyboardMouse;
    fn to(
        board: &mut impl KeyboardMouse,
        keyboard: &mut crate::terminal::keyboard::Keyboard,
        mouse: &mut crate::terminal::mouse::Mouse,
    ) {
        if keyboard.pending() > 0 {
            keyboard.deliver(board);
        }
        if mouse.pending(board.mouse_buttons_held()) {
            mouse.deliver(board);
        }
    }
    if m.geometry.machine_id.is_some() {
        to(&mut m.quux_input, keyboard, mouse);
    } else {
        to(&mut m.ioboard, keyboard, mouse);
    }
}

/// The netlists `chip` builds the CADR's boards from: the text of the
/// eight `data/*.netlist` files, one field each.
///
/// **The executable embeds them and hands them to [`run`], not the
/// library**, so that the library builds without them. `tools/*-netlist.sh`
/// make those files with `examples/reconcile.rs`, which links the library,
/// and `tools/check-netlists.sh` deletes the committed ones before it runs
/// the scripts: a library that embedded them could not be built to make
/// them. `cadr` passes them; `quux` passes none, `--chip` being `cadr`'s.
pub struct Netlists {
    /// `data/CADR.netlist`: the processor, both sections.
    pub cadr: &'static str,
    /// `data/BUSINT.netlist`: the bus interface.
    pub busint: &'static str,
    /// `data/CADRM.netlist`: a main memory board.
    pub cadrm: &'static str,
    /// `data/CADRIO.netlist`: the I/O board.
    pub cadrio: &'static str,
    /// `data/SIMPLETV.netlist`: the SIMPLE TV.
    pub simpletv: &'static str,
    /// `data/LISPMTV.netlist`: the LISPM TV, main screen or color.
    pub lispmtv: &'static str,
    /// `data/CADRDC.netlist`: the disk controller.
    pub cadrdc: &'static str,
    /// `data/DM.netlist`: the DISK MULTIPLEXOR.
    pub dm: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Which {
    Micro,
    Rtl,
    Chip,
}

/// What the ratio does **not** include: the disk.
///
/// This is microcycles against microcycles. With the disk controller as a
/// behavioral model --- always on `micro` and `rtl`, and on `chip` when
/// it is given `--disk-controller model` --- a transfer completes inside
/// the store to `START` and a seek takes no time. The machine spent
/// milliseconds on a seek and spent them running the microcode's polling
/// loop, so a 55 ms seek is about 380,000 microcycles the hardware executes
/// and muir does not. A program that seeks therefore finishes further ahead
/// of the hardware than this says, by an amount that depends on the program.
///
/// On the netlist disk controller it inverts: the drive takes its own time,
/// the polling loop runs through every gate on the board, and a seeking
/// program comes out slower than this rather than faster.
fn report(name: &str, cycles: u64, secs: f64, cycle_ns: u64) {
    let rate = cycles as f64 / secs;
    let ratio = rate / (1e9 / cycle_ns as f64);
    // Far below real time the reciprocal is the readable number:
    // "hardware/2800" says what "0.00x" does not. Near it, the ratio does.
    let against = if ratio >= 0.1 {
        format!("{ratio:.2}x hardware")
    } else {
        format!("hardware/{:.0}", 1.0 / ratio)
    };
    println!(
        "{name:6} {cycles:>9} microcycles  {secs:7.3} s  {rate:>11.0} microcycles/s  {against:>16}"
    );
}

/// Whose a flag is: both executables', or one machine's alone. **The
/// executable is the machine**, so a flag that means nothing on it is no
/// flag of it: `cadr` refuses QUUX's by name and `quux` the CADR's, each
/// saying which executable takes it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Whose {
    /// A flag of both, meaning the same thing on each.
    Both,
    /// The CADR's alone: `chip` and its boards, the debug cable, the
    /// CADR's timing, display boards, disk controller and color TV, and
    /// the Unibus's serial port.
    Cadr,
    /// QUUX's alone: its cache, main memory's port, clock, display size,
    /// real-time clock and file device.
    Quux,
}

/// The flags that are one machine's alone, and whose. A flag not here is
/// both executables', or neither's; `--machine` is neither's, the
/// executable being the machine.
const OWN_FLAGS: &[(&str, Whose)] = &[
    ("--chip", Whose::Cadr),
    ("--color-terminal", Whose::Cadr),
    ("--color-tv", Whose::Cadr),
    ("--color-tv-capture", Whose::Cadr),
    ("--debug-cable-connect", Whose::Cadr),
    ("--debug-cable-listen", Whose::Cadr),
    ("--debug-in-process", Whose::Cadr),
    ("--debuggee-chaos-address", Whose::Cadr),
    ("--debuggee-disk-pack", Whose::Cadr),
    ("--debuggee-terminal", Whose::Cadr),
    ("--disk-controller", Whose::Cadr),
    ("--disk-multiplexor", Whose::Cadr),
    ("--io-board", Whose::Cadr),
    ("--main-memory", Whose::Cadr),
    ("--main-memory-boards", Whose::Cadr),
    ("--no-debug-cable-listen", Whose::Cadr),
    ("--serial", Whose::Cadr),
    ("--timing-model", Whose::Cadr),
    ("--tv", Whose::Cadr),
    ("--tv-board", Whose::Cadr),
    ("--watch", Whose::Cadr),
    ("--cache", Whose::Quux),
    ("--file-root", Whose::Quux),
    ("--main-memory-size", Whose::Quux),
    ("--memory-timing", Whose::Quux),
    ("--microcycle-ns", Whose::Quux),
    ("--rtc", Whose::Quux),
    ("--sync-cycle-ticks", Whose::Quux),
    ("--tlb", Whose::Quux),
    ("--video-size", Whose::Quux),
];

/// `cadr`'s usage: its flags, in the order `--help` lists them.
const USAGE_CADR: &str = "usage: cadr [--micro|--rtl|--chip] [--chaos-address <address>]
            [--chaos-trace] [--chaos-udp [<endpoint>]]
            [--chaos-udp-default-peer <host>[:<port>]]
            [--chaos-udp-peer <address>@<host>:<port>] [--checkpoint <file>]
            [--color-terminal [<endpoint>]] [--color-tv [netlist|model]]
            [--color-tv-capture <gif>] [-c|--config <file>] [--continue]
            [--debug-cable-connect [<endpoint>|0x<address>]]
            [--debug-cable-listen [<endpoint>]] [--debug-in-process]
            [--debuggee-chaos-address <address>]
            [--debuggee-disk-pack <image>[,<unit>][,ro][,wp]]
            [--debuggee-terminal [<endpoint>]]
            [--disk-controller netlist|model] [--disk-multiplexor]
            [--disk-pack <image>[,<unit>][,ro][,wp]]
            [--glass-tty [<endpoint>][,ro]]
            [--io-board netlist|model] [--keyboard-boot <keys>]
            [--keyboard-mapping <file>] [--keyboard-mapping-dump]
            [--keyboard-mapping-trace] [--main-memory netlist|model]
            [--main-memory-boards <n>] [--no-auto-boot]
            [--no-debug-cable-listen] [--no-pace] [--pace]
            [--prom <file>] [--resume <file>] [--serial <endpoint>]
            [--stop-after <microcycles>] [--stop-at <pc>]
            [--stop-at-prom <pc>] [--terminal [<endpoint>]]
            [--timing-model cadr|fpga]
            [--tv netlist|model] [--tv-board simple-tv|lispm-tv]
            [--tv-capture <gif>] [--tv-capture-no-time]
            [--watch <from>[-<to>]:<net>,<net>,...] [-h|--help]
            [-V|--version]";

/// `quux`'s usage: its flags, in the order `--help` lists them.
const USAGE_QUUX: &str = "usage: quux [--micro|--rtl] [--cache <words>] [--chaos-address <address>]
            [--chaos-trace] [--chaos-udp [<endpoint>]]
            [--chaos-udp-default-peer <host>[:<port>]]
            [--chaos-udp-peer <address>@<host>:<port>] [--checkpoint <file>]
            [-c|--config <file>] [--continue] [--disk-pack <image>[,ro]]
            [--file-root [<name>=]<folder>[,ro]]
            [--glass-tty [<endpoint>][,ro]] [--keyboard-boot <keys>]
            [--keyboard-mapping <file>] [--keyboard-mapping-dump]
            [--keyboard-mapping-trace] [--main-memory-size <n>MW]
            [--memory-timing <r>,<w>[,<o>]] [--microcycle-ns <ns>]
            [--no-auto-boot] [--no-pace]
            [--pace] [--prom <file>] [--resume <file>]
            [--rtc <unix-seconds>|host] [--stop-after <microcycles>]
            [--stop-at <pc>] [--stop-at-prom <pc>]
            [--sync-cycle-ticks <k>] [--terminal [<endpoint>]] [--tlb <entries>]
            [--tv-capture <gif>] [--tv-capture-no-time]
            [--video-size <w>x<h>] [-h|--help] [-V|--version]";

/// What `-h` and `--help` print after the usage: each flag the executable
/// takes, in the order the usage lists them --- the entries of the other
/// machine's left out, and a flag both take given its own entry for each
/// where the two machines differ. `{exe}` is the executable's name.
const HELP: &[(Whose, &str)] = &[
    (
        Whose::Cadr,
        "  --micro | --rtl | --chip     the engine: microinstruction, register
                               transfer, or chip level. [default: --rtl]",
    ),
    (
        Whose::Quux,
        "  --micro | --rtl              the engine: microinstruction or register
                               transfer. [default: --rtl]",
    ),
    (
        Whose::Both,
        "  --chaos-address <address>    this machine's Chaosnet address: the sixteen
                               address switches on the I/O board, the bits
                               in octal, 3050, or subnet:host with each in
                               octal, 6:50 --- the same number, subnet in
                               the high byte. The switches are set whether
                               or not anything is plugged in: --chaos-udp
                               is the cable. Which
                               address to give is the band's own: 3050 on
                               System 100, whose file and time host is 3060
                               --- not muir, but another program on the
                               network, named with --chaos-udp-peer.
                               [default: 177001, on subnet 376 and no
                               band's]",
    ),
    (
        Whose::Both,
        "  --chaos-trace                every Chaosnet packet and frame on the cable,
                               to stderr. [default: off]",
    ),
    (
        Whose::Both,
        "  --chaos-udp [<endpoint>]     the Chaosnet cable, plugged in: Chaosnet over
                               UDP, which cbridge, usim, klh10 and ozd
                               speak. **Without this muir sends nothing**,
                               whatever --chaos-address has set the switches
                               to, as a machine with no cable talks to
                               nobody. Nothing, a port, an address or
                               address:port; a bare port is on the loopback,
                               so reaching another host means naming an
                               address to listen on. muir is a leaf: a
                               frame goes out only when this machine put
                               it on the cable, so one peer's is never
                               carried on to another. [default: off, the cable
                               unplugged; 127.0.0.1:42042, the protocol's
                               own port, when the flag is given bare]",
    ),
    (
        Whose::Both,
        "  --chaos-udp-default-peer <host>[:<port>]
                               where a frame goes whose destination no
                               --chaos-udp-peer named: the route of last
                               resort, which is what lets a cbridge beside
                               muir carry the traffic on to the wider
                               Chaosnet. Naming that bridge as a peer does
                               not do it --- a peer entry places one
                               address --- so this takes an endpoint and no
                               Chaosnet address, the frame carrying the
                               destination in its trailer for the bridge to
                               route on: a port, an address or address:port,
                               a name resolved once here, and 42042 if no
                               port is given. A broadcast is not sent here,
                               going to the named peers alone, who are
                               stations on this machine's cable. It needs
                               the cable, --chaos-udp. [default: off, and a
                               frame no peer entry names is dropped]",
    ),
    (
        Whose::Both,
        "  --chaos-udp-peer <address>@<host>:<port>
                               a Chaosnet host reached over UDP and where it
                               lives: 3060@127.0.0.1:42043, the address in
                               octal or subnet:host and the host a name or
                               an address, resolved once here. The port may
                               be left off for 42042. Once per peer, and the
                               address may not be this machine's own. It
                               needs the cable, --chaos-udp.",
    ),
    (
        Whose::Both,
        "  --checkpoint <file>          write the machine's whole state to <file>
                               when the run stops, for --resume to start
                               from. On chip it is the boards themselves,
                               taken at the first microcycle from the stop
                               with no bus cycle in flight. The prompt's
                               checkpoint writes one as the run goes, and
                               the run goes on.",
    ),
    (
        Whose::Cadr,
        "  --color-terminal [<endpoint>]
                               where the color TV's screen is served, as
                               --terminal is the main screen's: a port, an
                               address or address:port. Pixels only --- the
                               machine has one keyboard and one mouse, on
                               the I/O board, and they stay with the main
                               screen --- so what a viewer types or points
                               at here is dropped. It needs --color-tv.
                               [default: the display above the main
                               screen's]",
    ),
    (
        Whose::Cadr,
        "  --color-tv [netlist|model]   fit the color TV, the second display board:
                               a LISPM TV strapped to 17200000 with its
                               registers at 17377750, which is MIT's own
                               \"for the color TV, x is 5\". Off by default,
                               and a machine without it answers those
                               addresses with an NXM, which is how System
                               100 finds out it has no color screen. With
                               it the band's cold boot starts the board:
                               the NTSC sync program, clock mode 3, and the
                               color map. The picture is 576 x 454 at four
                               bits a pixel through sixteen colors, on
                               --color-terminal. netlist puts the board
                               itself on chip's backplane, a second LISPM
                               TV wrapped to the color addresses, and is
                               chip's alone; model is the behavioral
                               board and is taken on every engine. Without
                               a word it is the netlist on chip and the
                               model elsewhere.
                               [default: off; with the flag and no word,
                               netlist on chip and model elsewhere]",
    ),
    (
        Whose::Cadr,
        "  --color-tv-capture <gif>     record the color TV's screen to <gif> as the
                               run goes, as --tv-capture records the main
                               screen: a file of its own, 576 x 454, four
                               bits a pixel through the color map, which
                               is the GIF's own colors and is written
                               again whenever the machine changes it. There
                               is no default path; one must be given. It
                               needs --color-tv. Not over the debug cable,
                               where the two machines are two clocks.",
    ),
    (
        Whose::Both,
        "  -c, --config <file>          the file of flags to read before the command
                               line, which must be there. Without it {exe}
                               reads .{exe}rc in the directory it was run
                               from, or failing that .{exe}rc in the home
                               directory --- the first of the three there,
                               not all of them. MUIR_RC names a file in
                               place of the two that are looked for.",
    ),
    (
        Whose::Both,
        "  --continue                   with --resume: set RUN at the start, as the
                               prompt's continue does, so that a checkpoint
                               taken with the machine halted runs on from
                               where it stood, with no reset. On a running
                               checkpoint it does nothing and says so.",
    ),
    (
        Whose::Cadr,
        "  --debug-cable-connect [<endpoint>|0x<address>]
                               rtl: this machine is the debugger: its DBGOUT
                               connects to a debuggee listening at the
                               endpoint, a port, an address or address:port.
                               Either end may be another program that speaks
                               the cable's frames. An argument beginning 0x
                               is no endpoint but a physical address, where
                               the muir-fpga project's CADR presents its
                               DBGIN as a register window, reached through
                               /dev/mem. It is that project's window and no
                               other: one that does not say so is refused,
                               and there is no default for where it sits.
                               Either way this machine has the prompt, as
                               one alone has, less checkpoint and the
                               capture commands. [default: 127.0.0.1:7661]",
    ),
    (
        Whose::Cadr,
        "  --debug-cable-listen [<endpoint>]
                               rtl, chip: where this machine's DBGIN
                               listens for a debugger's cable over TCP: a
                               port, an address or address:port. Every rtl
                               and chip run listens, as every machine's
                               DBGIN is there; this moves the connector, or
                               puts it back after --no-debug-cable-listen,
                               and a port named here is bound as it stands.
                               The machine runs on its own until a debugger
                               connects, in step with it while one is on
                               the cable, and on its own again when the
                               debugger is done or goes away. It has the
                               prompt throughout, less checkpoint and the
                               capture commands while a debugger is on. On
                               chip the cable meets the board's own
                               connector, run an event at a time while a
                               debugger is on. [default: 127.0.0.1:7661, or
                               the port above it when that one is taken]",
    ),
    (
        Whose::Cadr,
        "  --debug-in-process           rtl: the two-machine lashup in one process. A
                               second machine runs beside this one with both
                               debug cables between them, each machine's
                               DBGOUT to the other's DBGIN, so that CC here
                               debugs the other, or the other this one. The
                               other boots the same PROM with no pack unless
                               one is named, and its console is CC's alone.
                               The stops are this machine's, and
                               --stop-after counts its microcycles. Both
                               machines get a terminal, the other's one port
                               above; neither has the prompt.",
    ),
    (
        Whose::Cadr,
        "  --debuggee-chaos-address <address>
                               rtl: the other machine's Chaosnet address, as
                               --chaos-address is this machine's. The other
                               machine has a cable of its own with no link
                               on it, so the address is all it has.
                               [default: the same address as this machine's,
                               the two cables never meeting]",
    ),
    (
        Whose::Cadr,
        "  --debuggee-disk-pack <image>[,<unit>][,ro][,wp]
                               rtl: the other machine's pack, as
                               --disk-pack.",
    ),
    (
        Whose::Cadr,
        "  --debuggee-terminal [<endpoint>]
                               rtl: the other machine's terminal, as
                               --terminal is this machine's, somewhere other
                               than the display above this one's: a port, an
                               address or address:port. [default: the
                               display above this machine's, 127.0.0.1:5901
                               when it is at :0]",
    ),
    (
        Whose::Cadr,
        "  --disk-controller netlist|model
                               chip: the disk controller. The netlist takes
                               the drive's real milliseconds over every block:
                               a run that touches no pack pays about 3% for
                               that, and System 100's boot through it took two
                               and a half days. model is how a run that does
                               not care about the disk is made quick, and is
                               what --main-memory model leaves. [default:
                               netlist]",
    ),
    (
        Whose::Cadr,
        "  --disk-multiplexor           chip: a DISK MULTIPLEXOR on the netlist
                               controller's cable, which is what gives it
                               eight drive ports instead of one. Without it
                               the one port is unit 0, so a second
                               --disk-pack, or one past unit 0, is refused.
                               It needs --disk-controller netlist; the model
                               controller wants no board. [default: off,
                               with one pack in unit 0; the start says when
                               it is fitted]",
    ),
    (
        Whose::Cadr,
        "  --disk-pack <image>[,<unit>][,ro][,wp]
                               the pack in a drive: its blocks end to end. The
                               image is opened read-write, as a drive writes
                               its pack. After the image, in any order: the
                               unit; ro, the image opened read-only and the
                               drive writable all the same, so a written
                               block stays in the run and reaches a
                               checkpoint rather than the file; and wp, the
                               drive's read-only switch --- the status word
                               says so, a write faults and nothing reaches
                               the file, and MIT's boot PROM halts at
                               ERROR-DISK-ERROR. Once for each pack, one to a
                               unit, up to the eight the controller
                               addresses. [default: unit 0; no pack unless
                               one is named, which is a drive with no pack in
                               it and a boot that waits on it for ever]",
    ),
    (
        Whose::Quux,
        "  --disk-pack <image>[,ro]     block-disk's one disk, unit 0: raw, a fixed VHD
                               or a dynamic VHD, of any size, and the start
                               says which. block-disk has the same registers
                               and command list as the CADR's controller, with
                               blocks by number, read and write only. The
                               image is opened read-write, as a drive writes
                               its pack; with ro it is opened read-only and
                               the disk is writable all the same, so a
                               written block stays in the run and reaches a
                               checkpoint rather than the file. [default: no
                               disk unless one is named, and a boot that waits
                               on it for ever]",
    ),
    (
        Whose::Quux,
        "  --file-root [<name>=]<folder>[,ro]
                               QUUX: a host folder its file device serves.
                               A folder alone is HOST's /, holding sys/,
                               site/ and home/<user>/; <name>=<folder> is
                               the top-level directory <name>, over the
                               default folder's entry of that name. ro
                               refuses every write under it. Repeatable, a
                               name once and one default folder; with no
                               default folder / holds the mounts alone and
                               is read-only. The start lists them.
                               [default: none; / is empty]",
    ),
    (
        Whose::Cadr,
        "  --glass-tty [<endpoint>][,ro]
                               a glass TTY: the screen as text over
                               telnet, and what is typed there back into
                               the keyboard. **Not a device.** --serial is
                               a real 2651 the band's own software must
                               drive; this is neither, and nothing in the
                               band knows it is there. It reads the frame
                               buffer through the machine's own character
                               font and puts what is typed on the I/O
                               board's keyboard cable, so it works where
                               the band's software cannot help: on a cold
                               machine with no drivers, in the boot PROM,
                               in PRAID and in the MIT diagnostics. A
                               cell that is not a character of that font
                               reads as `?`, so under the window system
                               much of the screen will not read at all.
                               Nothing, a port, an address or
                               address:port, and `ro` for a client that
                               may watch and not type, in either order.
                               Once for each; a port not named moves up
                               until it finds one free, so the flag twice
                               is two of them. Any telnet client will do:
                               `telnet 127.0.0.1 10023`. [default: off,
                               and no glass TTY at all; 127.0.0.1:10023
                               when the flag is given bare, which is
                               muir's own number and no convention ---
                               telnet's own port is 23 and a server on it
                               needs privilege this has no business
                               asking for]",
    ),
    (
        Whose::Quux,
        "  --glass-tty [<endpoint>][,ro]
                               a glass TTY: the screen as text over telnet,
                               and what is typed there back into the
                               keyboard. **Not a device**: nothing in the
                               band knows it is there. It reads the frame
                               buffer through the machine's own character
                               font and puts what is typed on the keyboard.
                               A cell that is not a character of that font
                               reads as `?`, so under the window system
                               much of the screen will not read at all.
                               Nothing, a port, an address or address:port,
                               and `ro` for a client that may watch and not
                               type, in either order. Once for each; a port
                               not named moves up until it finds one free,
                               so the flag twice is two of them. Any telnet
                               client will do: `telnet 127.0.0.1 10023`.
                               [default: off, and no glass TTY at all;
                               127.0.0.1:10023 when the flag is given bare,
                               which is muir's own number and no convention
                               --- telnet's own port is 23 and a server on
                               it needs privilege this has no business
                               asking for]",
    ),
    (Whose::Cadr, "  --io-board netlist|model     chip: the I/O board. [default: netlist]"),
    (
        Whose::Both,
        "  --keyboard-boot <keys>       the keys the boot sequence needs: held
                               with Rubout they cold-boot the machine,
                               with Return they warm-boot it, from the
                               keyboard, as on a CADR. ctrl is MIT's
                               Control key and meta its Meta; one of a
                               word is either key of its pair, two is
                               both. ctrl,meta is either Control and
                               either Meta, as Ctrl-Alt-Del is pressed;
                               ctrl,ctrl,meta both Controls and either
                               Meta; ctrl,meta,meta either Control and
                               both Metas; ctrl,ctrl,meta,meta both of
                               each, the CADR keyboard's own sequence.
                               [default: ctrl,meta]",
    ),
    (
        Whose::Both,
        "  --keyboard-mapping <file>    what a viewer's keysyms mean on the Lisp
                               Machine keyboard: `key <keysym> <key>` a
                               line, and `prefix <keysym> <keysym> <key>`
                               for a key reached by pressing one and then
                               another. It goes over the built-in mapping
                               rather than replacing it, and the prompt's
                               `keys` prints what is in force. MUIR_KEYS
                               names a file in place of the two looked for.
                               [default: .muirkeys, looked for where .{exe}rc
                               is; without one the built-in mapping stands]",
    ),
    (
        Whose::Both,
        "  --keyboard-mapping-dump      write the mapping this run would use to
                               stdout, in the format --keyboard-mapping
                               reads, and stop before a machine is built.
                               Fed back in unedited it changes nothing, so
                               it is a copy to edit rather than a report:
                               `{exe} --keyboard-mapping-dump > my.keys`,
                               edit it, `{exe} --keyboard-mapping my.keys`.",
    ),
    (
        Whose::Both,
        "  --keyboard-mapping-trace     every keysym a viewer sends and what it
                               became, on stderr, alongside the run: the
                               keysym by name and number, whether it went
                               down or up, and the key it became, spelled as
                               --keyboard-mapping-dump spells it so that the
                               line can be pasted into a mapping file, or
                               refused, when the machine has not read the
                               keystrokes before it. And how many key events
                               the terminal's input queue had no room for,
                               every time that count changes: a keystroke
                               lost there never became a keysym line at all.
                               A run says the first of either without this
                               flag. [default: off]",
    ),
    (
        Whose::Cadr,
        "  --main-memory netlist|model  chip: main memory as MIT's board or as rtl's
                               model of it. model takes the disk controller
                               down with it, the netlist controller being a
                               second master the model memory does not
                               answer; asking for it and --disk-controller
                               netlist together is refused. [default:
                               netlist]",
    ),
    (
        Whose::Cadr,
        "  --main-memory-boards <n>     how many 64K-word boards, 1 to 60: main
                               memory on every engine, and on chip the
                               boards on the backplane. [default: 32, the
                               two million words]",
    ),
    (
        Whose::Quux,
        "  --main-memory-size <n>MW     how much main memory, in whole megawords with
                               the unit written: 1MW to 64MW. No other unit
                               and no fraction. [default: 32MW]",
    ),
    (
        Whose::Both,
        "  --no-auto-boot               leave the boot button unpressed, as a CADR is
                               when the power comes on: RUN clear and
                               nothing running. The run starts held at the
                               prompt, and boot there presses the button;
                               continue sets RUN without it, and step says
                               it does neither. [default: muir presses the
                               button for you]",
    ),
    (
        Whose::Cadr,
        "  --no-debug-cable-listen      rtl, chip: no connector for a debugger's
                               cable; the machine cannot be debugged from
                               another. Of this and --debug-cable-listen
                               the last given wins. --checkpoint,
                               --tv-capture and chip's --watch leave the
                               connector empty by themselves, wanting a
                               machine on its own, and the start says so.
                               [default: the connector is there]",
    ),
    (
        Whose::Both,
        "  --no-pace                    run as fast as the host will take it, rather
                               than at the machine's own speed. Of this and
                               --pace the last given wins. [default: rtl and
                               chip are paced, micro is not]",
    ),
    (
        Whose::Both,
        "  --pace                       run at the machine's own speed rather than
                               as fast as the host will take it: the
                               machine's own nanoseconds are the target, and
                               a run ahead of them waits. Unpaced, micro
                               is about nine times a CADR and rtl about
                               twice, so much of a paced run of either is
                               spent waiting rather than computing, and the
                               core it was pinning is left idle for that
                               share of it. One speed, the machine's own:
                               there is no factor. A run that falls behind
                               --- a loaded host, a heavy microcycle ---
                               does not then sprint to make it up, and time
                               held at the prompt is not made up either. A
                               wait the host rounds up is not taken back
                               either, so a paced run keeps the machine's
                               speed or falls a little under it, never over.
                               On chip, which is far slower than the
                               machine, it never waits at all. Refused on an
                               end of the debug cable, where the two
                               machines pace each other, and not taken there
                               by default. [default: on for rtl and chip,
                               off for micro]",
    ),
    (
        Whose::Cadr,
        "  --prom <file>                the boot PROM to run, an MCR microcode file
                               as MIT's own sys/ubin/promh.mcr is: at most
                               the 512 words the machine fetches before it
                               turns the PROM off, assembled at address 0,
                               with the statistics bit IR<46> nowhere set.
                               Anything else is refused rather than run. The
                               start says how the file stands to MIT's own.
                               [default: MIT's own, built in --- System
                               100's sys/ubin/promh.mcr, version 9]",
    ),
    (
        Whose::Quux,
        "  --prom <file>                the boot PROM to run, an MCR microcode file as
                               QUUX's own data/quux-promh.mcr is: in partition
                               order, MIT's with the two 16-bit halves of
                               every 32-bit word swapped, and assembled at
                               36000, where QUUX's PROM sits, with the
                               statistics bit IR<46> nowhere set. A file in
                               MIT's order is refused saying so, and so is one
                               assembled at 0. The start says how the file
                               stands to QUUX's own. [default: QUUX's own,
                               built in --- data/quux-promh.mcr, version
                               2001]",
    ),
    (
        Whose::Quux,
        "  --resume <file>              start from a checkpoint instead of cold: the
                               engine that wrote it, the same pack under it,
                               the Chaosnet plugged in afresh, and as much
                               main memory as it had, which
                               --main-memory-size may not gainsay. The
                               button is not pressed, and the stops count
                               from here. A checkpoint taken halted resumes
                               halted: continue at the prompt, or
                               --continue, runs it on.",
    ),
    (
        Whose::Cadr,
        "  --resume <file>              start from a checkpoint instead of cold: the
                               engine that wrote it, the same pack under it,
                               the Chaosnet plugged in afresh, and as many
                               memory boards as it had, which
                               --main-memory-boards may not gainsay. On chip
                               the boards on the backplane have to be the
                               checkpoint's too. The button is not pressed,
                               and the stops count from here. A checkpoint
                               taken halted resumes halted: continue at the
                               prompt, or --continue, runs it on.",
    ),
    (
        Whose::Quux,
        "  --rtc <unix-seconds>|host    QUUX: its real-time clock, register page word
                               103. host reads the host's clock at each
                               read; a second, 0 to 4294967295, starts it
                               there at power-on and counts the machine's
                               own time from it, holding at 4294967295, so
                               that runs repeat. A checkpoint carries it,
                               and a resume under another is refused.
                               [default: host]",
    ),
    (
        Whose::Cadr,
        "  --serial <endpoint>          where the serial port at J9 is reached: a TCP
                               port, or address:port. Attach with `nc <host>
                               <port>` or telnet. A connection is the device
                               on the null-modem cable plugging in: it
                               asserts DSR, DCD and CTS, and hanging up
                               drops them. One device at a time, and a
                               second connection is closed as it arrives.
                               The rate and the frame are the machine's to
                               program into the 2651 --- MIT's driver
                               defaults to 300 baud, seven data bits and
                               even parity --- and nothing here sets them.
                               One machine's port, so it is refused with
                               --debug-in-process and the cable flags.
                               [default: off, and J9 empty]",
    ),
    (
        Whose::Both,
        "  --stop-after <microcycles>   how many to run, then stop. [default: none;
                               the run goes on until a --stop-at, a halt or
                               ^C]",
    ),
    (
        Whose::Cadr,
        "  --stop-at <pc>               stop when the PC reaches this address with the
                               boot PROM disabled: in microcode loaded into
                               the control store. Octal, as MIT writes it.",
    ),
    (
        Whose::Quux,
        "  --stop-at <pc>               stop when the PC reaches this address in
                               microcode loaded into the control store,
                               outside the boot PROM's 36000-37777. Octal, as
                               MIT writes it.",
    ),
    (
        Whose::Cadr,
        "  --stop-at-prom <pc>          the same with the PROM enabled: an address in
                               the boot PROM, below 1000. With --stop-after,
                               whichever comes first.",
    ),
    (
        Whose::Quux,
        "  --stop-at-prom <pc>          the same in the boot PROM, which is never
                               disabled: an address in 36000-37777, the
                               control-store address the PC holds. With
                               --stop-after, whichever comes first.",
    ),
    (
        Whose::Both,
        "  --terminal [<endpoint>]      where the display, keyboard and mouse are
                               served over RFB, RFC 6143, for any VNC viewer
                               to connect to: a port, an address or
                               address:port. Every run serves one, asked for
                               or not; this says where instead, and the
                               start says where it went. A named port is
                               bound as it stands, and the run stops if it
                               cannot be; an unnamed one is where the first
                               free display is looked for. An address other
                               than the loopback lets another machine in,
                               RFB's None security being the only type
                               offered. [default: 127.0.0.1:5900, VNC's
                               display :0, or the first free display above
                               it]",
    ),
    (
        Whose::Cadr,
        "  --timing-model cadr|fpga     rtl: whose time the processor keeps: the CADR's
                               own, or the 10 ns grid muir-fpga's fabric runs
                               on, where a delay rounds up to the next tick
                               and a free-running clock's edge is taken at the
                               first tick at or after it. [default: cadr]",
    ),
    (
        Whose::Quux,
        "  --cache <words>              rtl, QUUX: its memory cache of <words>, 16
                               or more, in lines of 8, 2-way, a hit in 20 ns:
                               write-through, by physical address, main
                               memory and the frame buffer. [default: 4096]",
    ),
    (
        Whose::Quux,
        "  --tlb <entries>              QUUX revision 14: its TLB of <entries>, a power
                               of two from 1024 to 32768, direct-mapped.
                               [default: 4096]",
    ),
    (
        Whose::Quux,
        "  --memory-timing <r>,<w>[,<o>]
                               rtl, QUUX: main memory's line fill and write
                               in ns, or arty or de25, the boards' own; on
                               revision 15 the occupancy between two
                               writes too, <r>,<w>,<o>, or kria, arty or
                               de25. [default: 380,290; revision 15: kria,
                               247,130,10]",
    ),
    (
        Whose::Quux,
        "  --microcycle-ns <ns>         QUUX revision 15: the clock's period, the
                               microcycle, 5 to 40 in steps of 0.5, on both
                               engines. [default: 8.5, the Kria KR260's]",
    ),
    (
        Whose::Quux,
        "  --sync-cycle-ticks <k>       sync: a microcycle's 10 ns ticks, the
                               board's: its fit proves its longest path
                               settles in them. [default: 4]",
    ),
    (Whose::Cadr, "  --tv netlist|model           chip: the display. [default: netlist]"),
    (
        Whose::Cadr,
        "  --tv-board simple-tv|lispm-tv
                               which display board, on every engine: the
                               SIMPLE TV that System 100 drives, or the LISPM
                               TV that replaced it in 1980. The two program
                               alike but for mode bit 7, which reads the sync
                               enable back on the LISPM TV and zero on the
                               SIMPLE TV. [default: simple-tv]",
    ),
    (
        Whose::Quux,
        "  --video-size <w>x<h>         the video controller's size, QUUX's display:
                               1280 by 1024 unless this says otherwise, one
                               bit a pixel, no interrupt. The width a multiple
                               of 32, at most 1920 by 1080. The feature page
                               gives it to the software. [default: 1280x1024]",
    ),
    (
        Whose::Cadr,
        "  --tv-capture <gif>           record the display to <gif> as the run goes,
                               an animated GIF timed by the machine's own
                               clock so that it plays at the machine's
                               speed. There is no default path; one must be
                               given. In the lashup it is both machines on
                               one canvas, the debugger at the left and the
                               debuggee at the right; not over the debug
                               cable, where they are two clocks. The main
                               screen: the color TV's is --color-tv-capture's.",
    ),
    (
        Whose::Quux,
        "  --tv-capture <gif>           record the display to <gif> as the run goes, an
                               animated GIF timed by the machine's own clock
                               so that it plays at the machine's speed. There
                               is no default path; one must be given.",
    ),
    (
        Whose::Cadr,
        "  --tv-capture-no-time         leave the clocks off the recordings, the
                               color screen's as much as the main
                               screen's: there is one flag for the two. By
                               default a line below the screen, hiding none
                               of it, shows the machine's simulated time at
                               the left and the local time of day at the
                               right, each hh:mm:ss.",
    ),
    (
        Whose::Quux,
        "  --tv-capture-no-time         leave the clocks off the recording. By default
                               a line below the screen, hiding none of it,
                               shows the machine's simulated time at the left
                               and the local time of day at the right, each
                               hh:mm:ss.",
    ),
    (
        Whose::Cadr,
        "  --watch <from>[-<to>]:<net>,<net>,...
                               chip: record the named nets over microcycles
                               <from> to <to>, or from <from> to the end of
                               the run with no <to>, counted as --stop-after
                               counts them. Each net as the prompt's `net`
                               names one --- `disk:NEW CCW`, `PC/14` for a
                               bus --- and the nets comma separated. The
                               boards are sampled at every instant they move
                               and not once a microcycle, so a pulse shorter
                               than one is seen. One line on stderr,
                               prefixed `watch:`, with the time in
                               nanoseconds, the microcycle and every value,
                               as the range begins and then at every change.
                               The prompt's watch records the next n
                               microcycles the same way, without a restart.
                               [default: off]",
    ),
    (Whose::Both, "  -h, --help                   this."),
    (
        Whose::Both,
        "  -V, --version                what this build calls itself: the version,
                               the commit it was built from --- with -dirty
                               after it if the tree had uncommitted work ---
                               and whether it was built with optimizations
                               off. Every run says it in its first line too.",
    ),
];

/// What this build calls itself: the crate's version, the commit it was
/// built from, and whether it was built with optimizations off ---
/// `muir 0.1.0-8a69eea-release`. `--version` prints it, and every run says
/// it in its first line, so a report of a run says which muir made it.
///
/// **A tree with uncommitted work in it says `-dirty` after the commit**,
/// because the commit alone would name something that was never built.
/// The commit is stamped in at build time by `build.rs`, which asks git
/// once there; nothing here runs git, and a built muir does not need it.
/// Built where there is no repository --- a source archive, a machine
/// with no git --- there is no commit to name and the version is the
/// crate's and the build's alone.
fn version() -> String {
    let build = if cfg!(debug_assertions) { "dev" } else { "release" };
    match option_env!("MUIR_GIT") {
        Some(commit) => format!("muir {}-{commit}-{build}", env!("CARGO_PKG_VERSION")),
        None => format!("muir {}-{build}", env!("CARGO_PKG_VERSION")),
    }
}

/// A run that cannot start, for a reason that is not the command line's
/// shape: a pack that will not open, a port already taken. The flags are no
/// help, so they are not printed.
fn fail(msg: &str) -> ! {
    eprintln!("{}: {msg}", executable());
    std::process::exit(1);
}

fn usage(msg: &str) -> ! {
    let exe = executable();
    eprintln!("{exe}: {msg}");
    eprintln!("{}", if exe == "quux" { USAGE_QUUX } else { USAGE_CADR });
    eprintln!("{exe} --help says more");
    std::process::exit(2);
}

/// The executable this run is, `cadr` or `quux`, which [`run`] settles
/// once from the machine it is given: what the refusals and the usage are
/// said by, as a program's messages are said by its name.
static EXECUTABLE: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// [`EXECUTABLE`], or the crate's name before [`run`] has settled it.
fn executable() -> &'static str {
    EXECUTABLE.get().copied().unwrap_or("muir")
}

/// The executable that is `machine`: `cadr` for the CADR, `quux` for QUUX.
fn executable_of(machine: crate::machine::Geometry) -> &'static str {
    if machine == crate::machine::Geometry::CADR { "cadr" } else { "quux" }
}

/// Why `flag` is no flag of the executable `exe`, when it is the other
/// machine's or `--machine`, which chose between them: the one refusal,
/// said of the command line and of a file of flags alike.
fn not_this_executables(flag: &str, exe: &str) -> Option<String> {
    match (whose(flag), exe) {
        // QUUX's main memory is an amount, not boards, and the flag that
        // gave it in boards went without an alias; `--main-memory` is the
        // CADR's board, netlist or model.
        (Whose::Cadr, "quux") if flag == "--main-memory-boards" || flag == "--main-memory" => {
            Some(format!(
                "{flag} is cadr's, not quux's: quux's main memory is an amount, \
                 --main-memory-size <n>MW, such as --main-memory-size 32MW"
            ))
        }
        (Whose::Quux, "cadr") => Some(format!("{flag} is quux's, not cadr's")),
        (Whose::Cadr, "quux") => Some(format!("{flag} is cadr's, not quux's")),
        _ if flag == "--machine" => Some(format!(
            "--machine is not a flag of {exe}: the executable is the machine, cadr or quux"
        )),
        // The video controller's size before contract Q13 named its board
        // MONO TV.
        _ if flag == "--mono-tv-size" => Some(format!(
            "--mono-tv-size is not a flag of {exe}: the video controller's size is --video-size"
        )),
        _ => None,
    }
}

/// Whose `flag` is, when it is one machine's alone: [`OWN_FLAGS`].
fn whose(flag: &str) -> Whose {
    OWN_FLAGS.iter().find(|(f, _)| *f == flag).map_or(Whose::Both, |&(_, w)| w)
}

/// **A checkpoint this build cannot read is a file to make again, not a
/// mistyped flag**, so it does not get [`usage`]'s sixty lines.
///
/// Two things go stale and both land here. The format is versioned and the
/// version moves --- 10 to 15 on 8 September 2026, three of them in one
/// afternoon --- and a board's [`Chip::fingerprint`] moves under a resume
/// whenever `data/CADR.netlist` is regenerated, which is
/// "saved from a different board or a different build". Neither is the
/// reader's mistake and neither is fixed by reading the flag list; what
/// fixes both is running the machine again to the same place with
/// `--checkpoint`, which is what wrote the file in the first place.
fn stale_checkpoint(path: &Path, err: &dyn std::fmt::Display, ran: Option<u64>) -> ! {
    eprintln!("{}: --resume {}: {err}", executable(), path.display());
    eprintln!("  the file is from another build, not a broken one, and making it again is");
    eprintln!("  the fix: run the machine to the same place with --checkpoint, as this was");
    match ran {
        Some(n) => eprintln!("  written --- `--stop-after {n} --checkpoint {}`", path.display()),
        None => eprintln!("  written --- `--stop-after <microcycles> --checkpoint <file>`"),
    }
    std::process::exit(2);
}

/// Prints `-h` and `--help`'s answer for the executable `exe`: its usage,
/// then the [`HELP`] entries that are its own or both executables'.
fn help(exe: &'static str) -> ! {
    let (usage, own, title) = if exe == "quux" {
        (USAGE_QUUX, Whose::Quux, "A simulator of QUUX, the MIT CADR Lisp Machine evolved.")
    } else {
        (USAGE_CADR, Whose::Cadr, "A simulator of the MIT CADR Lisp Machine.")
    };
    println!("{usage}\n\n{title}\n");
    for (whose, entry) in HELP {
        if *whose == Whose::Both || *whose == own {
            println!("{}", entry.replace("{exe}", exe));
        }
    }
    std::process::exit(0);
}

/// The packs a run gets: the ones `--disk-pack` names, and nothing at all
/// otherwise. There is no default, because which pack a drive holds is not
/// something to guess: no flag is a drive with no pack in it, which the
/// boot waits on for ever. The path, the unit, whether the file is opened
/// read-only and the drive's read-only switch, in the order the flags came.
fn pack_choice(packs: &[Pack]) -> Vec<(PathBuf, usize, bool, bool)> {
    packs.iter().map(|p| (p.path.clone(), p.unit, p.read_only, p.write_protect)).collect()
}

/// What a file of flags is called where the executable `exe` looks for
/// one: `.cadrrc` for `cadr` and `.quuxrc` for `quux`, each executable
/// reading its own and never the other's, the flags in it being its
/// machine's.
fn rc_name(exe: &str) -> String {
    format!(".{exe}rc")
}

/// The keyboard mapping this run's terminal uses, settled once from the
/// flags and read by every place that makes a [`Keyboard`].
///
/// One run has one mapping --- it is what a viewer's keysyms mean, and a
/// run serves one viewer's keyboard --- so it is here rather than
/// threaded through the five timing loops and the prompt, none of which
/// would do anything with it but pass it on.
static KEYS_IN_FORCE: std::sync::OnceLock<Mapping> = std::sync::OnceLock::new();

/// Whether `--keyboard-mapping-trace` was given, for every keyboard this
/// run builds: the lashup builds two, and a trace of one machine's keys
/// and not the other's would say less than it appears to.
static KEYS_TRACED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `--keyboard-boot`: the keys the boot sequence needs, for every keyboard
/// this run builds, the lashup's second included.
static BOOT_KEYS: std::sync::OnceLock<BootKeys> = std::sync::OnceLock::new();

/// The keyboard mapping in force, as the prompt's `keys` prints it: what
/// each of a viewer's keysyms means, and where the mapping came from.
fn keys_in_force() -> String {
    let m = KEYS_IN_FORCE.get().cloned().unwrap_or_default();
    let from = match m.source() {
        Some(p) => format!("{}, over the built-in mapping", shown(p)),
        None => "the built-in mapping".to_string(),
    };
    format!("keyboard mapping: {from}\n{}", m.show())
}

/// A keyboard on the mapping this run settled on, needing the keys the
/// run's boot sequence needs.
fn a_keyboard() -> Keyboard {
    let mut k = Keyboard::with_mapping(KEYS_IN_FORCE.get().cloned().unwrap_or_default());
    k.traced(KEYS_TRACED.load(std::sync::atomic::Ordering::Relaxed));
    k.set_boot_keys(BOOT_KEYS.get().copied().unwrap_or_default());
    k
}

/// The keyboard mapping a run reads, beside its flags: `.muirkeys` in the
/// directory muir was run from, else `.muirkeys` in the home directory.
const KEYS: &str = ".muirkeys";

/// The keyboard mapping file this run reads, and whether it was asked for
/// by name.
///
/// `--keyboard-mapping` if it is given, else `MUIR_KEYS`, else [`KEYS`] in the
/// directory muir was run from, else [`KEYS`] in the home directory ---
/// the same order and the same rule as [`config_path`], the first of them
/// there and not all of them. A file asked for by name must be there; the
/// ones looked for need not be, and most runs have none, which leaves the
/// built-in mapping standing.
fn keyboard_path(named: Option<&Path>) -> Option<(PathBuf, bool)> {
    if let Some(p) = named {
        return Some((p.to_path_buf(), true));
    }
    if let Some(from_env) = std::env::var_os("MUIR_KEYS") {
        return Some((PathBuf::from(from_env), false));
    }
    let here = PathBuf::from(KEYS);
    if here.exists() {
        return Some((here, false));
    }
    let home = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(KEYS))?;
    home.exists().then_some((home, false))
}

/// Which board of the netlist machine a net is on: where the prompt's
/// `net` and `--watch` find its chip, [`chip_on`], once a name has been
/// resolved.  The boards' names, and the order they are searched in, are
/// [`boards_named`]'s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum On {
    Cpu,
    Busint,
    /// The first memory board on the backplane.
    Memory,
    /// A device on the Xbus, by its place in `Xbus::devices`.
    Device(usize),
    Io,
}

/// A board by the name `net` and `--watch` take, where it is, its chip and
/// its netlist.
type Named<'a> = (&'static str, On, &'a Chip, &'a netlist::Netlist);

/// The boards of a netlist machine that carry nets, by the names the
/// prompt's `net` and `--watch` take, in the order a name is searched:
/// the processor, the interface, a memory board, then the devices.  The
/// devices are built display first, then the disk controller, which is
/// `Xbus::new`'s own order; the I/O board is on the Unibus, last.
fn boards_named<'a>(
    cpu: &'a Chip,
    far: &'a FarEnd,
    boards: &Boards<'a>,
    (cpu_n, bus_n, mem_n): &'a (netlist::Netlist, netlist::Netlist, netlist::Netlist),
) -> Vec<Named<'a>> {
    let mut on: Vec<Named<'a>> =
        vec![("cpu", On::Cpu, cpu, cpu_n), ("busint", On::Busint, &far.board, bus_n)];
    if let Some(m) = far.xbus.boards.first() {
        on.push(("memory", On::Memory, m, mem_n));
    }
    let devices = far.xbus.devices.iter().enumerate();
    for ((what, n), (k, chip)) in [("tv", boards.tv), ("disk", boards.disk)]
        .into_iter()
        .filter_map(|(w, n)| n.map(|n| (w, n)))
        .zip(devices)
    {
        on.push((what, On::Device(k), chip, n));
    }
    if let (Some(u), Some(n)) = (far.unibus.as_ref(), boards.io) {
        on.push(("io", On::Io, &u.board, n));
    }
    on
}

/// The chip a resolved net is on, as the machine stands now.  The boards
/// are where [`boards_named`] found them: a resolved `On` names a board
/// the machine has.
fn chip_on<'a>(cpu: &'a Chip, far: &'a FarEnd, on: On) -> &'a Chip {
    match on {
        On::Cpu => cpu,
        On::Busint => &far.board,
        On::Memory => &far.xbus.boards[0],
        On::Device(k) => &far.xbus.devices[k],
        On::Io => &far.unibus.as_ref().expect("resolved on the I/O board, so it is there").board,
    }
}

/// A name resolved: the board that carries it, the nets --- one, or a
/// bus's from bit 0 up --- and any other boards that carry the name too.
struct Found {
    board: &'static str,
    on: On,
    nets: Vec<netlist::NetId>,
    also: Vec<&'static str>,
}

/// A name as `net` takes one, found on whichever board carries it, or why
/// it was not.
///
/// **A name is unique only within a board.** `-XBUS RQ` is on nearly all of
/// them and `TRIDENT.READY/` on one, so the boards are searched in the
/// order [`boards_named`] gives them and the answer says which one carried
/// it. Where more than one does, the others are named too, so that a
/// reading is never quietly the wrong board's.
///
/// A bus is `NAME/width`, `NAME0` up, which is how `MUIR_WATCH` writes one.
fn resolve_net(on: &[Named], want: &NetName) -> Result<Found, String> {
    let NetName { board: want_board, name, width } = want;
    if let Some(board) = want_board
        && !on.iter().any(|&(b, ..)| b == board)
    {
        let names: Vec<&str> = on.iter().map(|&(b, ..)| b).collect();
        return Err(format!("no board called {board} here; this run has {}", names.join(", ")));
    }
    let named = |n: &netlist::Netlist, what: &str| {
        n.by_name_id(what).or_else(|| n.by_name_id(&format!("'{what}'")))
    };
    let asked = |b: &str| want_board.as_deref().is_none_or(|w| w == b);
    // A bus is carried by `NAME0` and there may be no net called `NAME` at
    // all, so what decides which board carries it is the first bit.
    let first = match width {
        None => name.clone(),
        Some(_) => format!("{name}0"),
    };
    let carries: Vec<&Named> =
        on.iter().filter(|&&(b, _, _, n)| asked(b) && named(n, &first).is_some()).collect();
    let Some(&&(board, at, chip, n)) = carries.first() else {
        let looked: Vec<&str> = on.iter().map(|&(b, ..)| b).filter(|b| asked(b)).collect();
        return Err(format!("no net {first} on {}", looked.join(", ")));
    };
    let nets = match width {
        None => vec![named(n, name).expect("just found")],
        Some(bits) => {
            if let Some(b) = (0..*bits).find(|b| named(n, &format!("{name}{b}")).is_none()) {
                return Err(format!("{board} has no {name}{b} --- a bus is {name}0 up"));
            }
            chip.bus_nets(n, name, *bits)
        }
    };
    let also = carries[1..].iter().map(|&&(b, ..)| b).collect();
    Ok(Found { board, on: at, nets, also })
}

/// A net or a bus read off whichever board carries the name: the prompt's
/// `net` on `chip`, which is [`resolve_net`] and a reading.
///
/// A bus's value is read the way a TTL input reads it, an undriven net as
/// a one, and the count of undriven bits is said beside it, because half
/// the datapath is tri-state and undriven for part of every cycle.
fn say_net(on: &[Named], want: &NetName) -> String {
    use std::fmt::Write;
    let Found { board, on: at, nets, also } = match resolve_net(on, want) {
        Ok(found) => found,
        Err(what) => return format!("prompt: {what}\n"),
    };
    let chip = on.iter().find(|&&(_, o, ..)| o == at).map(|&(_, _, c, _)| c).expect("named");
    let name = &want.name;
    let mut out = String::new();
    match want.width {
        None => writeln!(out, "{name} on {board}: {:?}", chip.net(nets[0])).unwrap(),
        Some(bits) => {
            let word = chip.read(&nets);
            let undriven = nets.iter().filter(|&&id| chip.net(id) == Level::Z).count();
            write!(out, "{name}/{bits} on {board}: {word:o} octal, {word:#x}").unwrap();
            match undriven {
                0 => writeln!(out, ", every bit driven").unwrap(),
                k => writeln!(out, ", {k} of {bits} bits undriven and read as ones").unwrap(),
            }
        }
    }
    if !also.is_empty() {
        writeln!(out, "  ({name} is also on {})", also.join(", ")).unwrap();
    }
    out
}

/// `--watch`'s argument, parsed: the first microcycle, the last or `None`
/// for the end of the run, and the nets, still by name.
type WatchSpec = (u64, Option<u64>, Vec<NetName>);

/// `<from>[-<to>]:<net>,<net>,...`: the microcycles, counted as
/// `--stop-after` counts them, and the nets as `net` names them.
///
/// The range is before the first colon and has none of its own, so the
/// colon that ends it is the first one, and a board prefix or a colon in
/// a name --- `cpu:LM UB: GRANTED` --- is the nets' to parse.
fn watch_spec(arg: &str) -> Result<WatchSpec, String> {
    let Some((range, list)) = arg.split_once(':') else {
        return Err("wants <from>[-<to>]:<net>,<net>,...".to_string());
    };
    let (from, to) = match range.split_once('-') {
        Some((f, "")) => (f, None),
        Some((f, t)) => (f, Some(t)),
        None => (range, Some(range)),
    };
    let from: u64 = from.parse().map_err(|_| format!("{from:?} is not a microcycle"))?;
    let to = match to {
        Some(t) => Some(t.parse::<u64>().map_err(|_| format!("{t:?} is not a microcycle"))?),
        None => None,
    };
    if to.is_some_and(|t| t < from) {
        return Err(format!("the range ends at {} before it begins at {from}", to.unwrap()));
    }
    let nets = crate::prompt::parse_net_names(list, "--watch")?;
    Ok((from, to, nets))
}

/// What one watched net read as the last time a line was printed: a net
/// as `net` prints it, a bus as a word when every bit is driven.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reading {
    Net(Level),
    Bus(Option<u64>),
}

/// One net or bus being recorded: what to call it, where it is, its nets.
struct Watched {
    label: String,
    on: On,
    nets: Vec<netlist::NetId>,
    bus: bool,
}

/// `--watch` and the prompt's `watch`: the named nets recorded over a range
/// of microcycles, one line on stderr at every change.
///
/// **The record is taken at the chip's own step, which is an event and
/// not a fixed interval.** [`FarEnd::tick_with`] moves every board to the
/// next instant anything on any of them is due --- the processor clock's
/// next transition, or a delay-line tap, an oscillator edge or a one-shot
/// on some board, whichever comes first --- and the gates are zero-delay,
/// so between two of those instants no net moves.  Sampled after each,
/// the record has every level a net settled at, however brief: `CCW CLK`
/// on the disk controller is two taps of a delay line 50 ns apart, and
/// once-a-microcycle sampling, 145 to 220 ns, would step over it.  What
/// it does not have is a level a net took and left inside one instant ---
/// a glitch of no width --- which is not a level the model has either.
///
/// The microcycle is the run's own count, as `--stop-after` and the
/// prompt's `pc` count it: a resume counts from the checkpoint.  The one
/// stretch of a run the record does not cover is the walk to a quiet
/// microcycle a checkpoint makes, [`chip_to_quiet`], which ticks the
/// boards itself, up to a thousand microcycles: those are counted and
/// said, and not sampled.
struct Watch {
    from: u64,
    /// `None` runs to the end of the run.
    to: Option<u64>,
    nets: Vec<Watched>,
    /// What was last printed for each, `None` before the first line.
    last: Vec<Option<Reading>>,
}

impl Watch {
    /// The nets resolved against the boards, or which one was not.
    fn new(from: u64, to: Option<u64>, nets: &[NetName], on: &[Named]) -> Result<Watch, String> {
        let nets = nets
            .iter()
            .map(|want| {
                let found = resolve_net(on, want)?;
                Ok(Watched {
                    label: want.to_string(),
                    on: found.on,
                    nets: found.nets,
                    bus: want.width.is_some(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let last = vec![None; nets.len()];
        Ok(Watch { from, to, nets, last })
    }

    /// What is recorded, comma separated, for the line that says so.
    fn labels(&self) -> String {
        self.nets.iter().map(|w| w.label.as_str()).collect::<Vec<_>>().join(", ")
    }

    /// One sample, after one step of the boards: a line if anything
    /// watched has changed since the last one, or at the first sample of
    /// the range, so that a reader has the values it began at.  `false`
    /// once the range is behind, when there is nothing more to sample.
    ///
    /// Before the range this is one comparison, and the run pays nothing
    /// else for a watch still to come.
    fn sample(&mut self, cycle: u64, now: u64, cpu: &Chip, far: &FarEnd) -> bool {
        if cycle < self.from {
            return true;
        }
        if self.to.is_some_and(|to| cycle > to) {
            return false;
        }
        let mut changed = false;
        for (w, last) in self.nets.iter().zip(self.last.iter_mut()) {
            let chip = chip_on(cpu, far, w.on);
            let reading = if w.bus {
                Reading::Bus(chip.read_driven(&w.nets))
            } else {
                Reading::Net(chip.net(w.nets[0]))
            };
            if *last != Some(reading) {
                *last = Some(reading);
                changed = true;
            }
        }
        if changed {
            use std::fmt::Write;
            let mut line = format!("watch: {now} ns, microcycle {cycle}:");
            for (w, last) in self.nets.iter().zip(&self.last) {
                match last.expect("every net has been read") {
                    Reading::Net(l) => write!(line, " {}={l:?}", w.label).unwrap(),
                    Reading::Bus(Some(v)) => write!(line, " {}={v:o}", w.label).unwrap(),
                    Reading::Bus(None) => write!(line, " {}=Z", w.label).unwrap(),
                }
            }
            eprintln!("{line}");
        }
        true
    }
}

/// The mapping this run's terminal uses: the built-in one, with whatever
/// [`keyboard_path`] found over it.  A file that cannot be read or that
/// says something muir does not understand stops the run rather than
/// leaving the user with a keyboard that is quietly not the one they
/// wrote.
fn keyboard_mapping(named: Option<&Path>) -> (Mapping, String) {
    let Some((path, _)) = keyboard_path(named) else {
        return (
            Mapping::default(),
            format!("built in; no {KEYS} found (--keyboard-mapping <file>)"),
        );
    };
    match Mapping::from_file(&path) {
        Ok(m) => {
            let line = format!("{}, over the built-in one", shown(&path));
            (m, line)
        }
        Err(e) => usage(&format!("--keyboard-mapping {e}")),
    }
}

/// The file of flags this run reads, and whether it was asked for by name.
///
/// `--config` if it is given, else [`rc_name`] in the directory the
/// executable was run from, else [`rc_name`] in the user's home directory: **the first of those
/// there, not all of them**, so a file in the directory is the whole of
/// the run's flags and not an addition to the home one.  `MUIR_RC` stands
/// in for the two that are looked for, which is how a test gives a run a
/// file of its own.
///
/// A file asked for by name must be there; the ones looked for need not
/// be, and most runs have none.
fn config_path(typed: &[String], exe: &str) -> Option<(PathBuf, bool)> {
    let mut typed = typed.iter();
    while let Some(word) = typed.next() {
        if word == "-c" || word == "--config" {
            let Some(path) = typed.next() else { usage("--config wants a file of flags") };
            return Some((PathBuf::from(path), true));
        }
    }
    if let Some(named) = std::env::var_os("MUIR_RC") {
        return Some((PathBuf::from(named), false));
    }
    let here = PathBuf::from(rc_name(exe));
    if here.exists() {
        return Some((here, false));
    }
    std::env::var_os("HOME").map(|home| (PathBuf::from(home).join(rc_name(exe)), false))
}

/// The flags in the file, [`config_path`], and which file that was.
///
/// A line is a flag and, after a space, whatever it takes, which is the
/// rest of the line --- so a path with a space in it is one word, as it is
/// in a shell's quotes.  A line whose first word is blank, or begins with
/// `#`, is a comment.
///
/// A flag the command line gives too is left out, and so is the engine
/// when the command line names one: the command line is what the person
/// at the keyboard means this time, and `--micro`, `--rtl` and `--chip`
/// are exclusive of each other, not last-wins.  A file cannot name
/// another; that is the command line's to say.
fn rc_flags(typed: &[String], exe: &str) -> (Vec<String>, Option<PathBuf>) {
    const ENGINES: [&str; 3] = ["--micro", "--rtl", "--chip"];
    let Some((path, named)) = config_path(typed, exe) else { return (Vec::new(), None) };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if named => usage(&format!("--config {}: {e}", path.display())),
        // One that is not there is one a run has none of.
        Err(_) => return (Vec::new(), None),
    };
    let engine_typed = typed.iter().any(|a| ENGINES.contains(&a.as_str()));
    let mut flags = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (flag, value) = match line.split_once(char::is_whitespace) {
            Some((flag, value)) => (flag, value.trim()),
            None => (line, ""),
        };
        if flag == "-c" || flag == "--config" {
            usage(&format!("{}: a file of flags cannot name another", shown(&path)));
        }
        // The file is this executable's own, so a flag of the other
        // machine's in it is the file's to mend: refused as the command
        // line refuses it, with the file named.
        if let Some(why) = not_this_executables(flag, exe) {
            usage(&format!("{}: {why}", shown(&path)));
        }
        if typed.iter().any(|a| a == flag) || (engine_typed && ENGINES.contains(&flag)) {
            continue;
        }
        flags.push(flag.to_string());
        if !value.is_empty() {
            flags.push(value.to_string());
        }
    }
    (flags, Some(path))
}

/// A path as the setup writes it: what it is under the directory muir was
/// run from when it is under it, and the whole path otherwise.  The pack
/// and the file of flags are looked for under that directory, and their
/// whole paths are long and say nothing a reader does not know.
fn shown(path: &Path) -> String {
    let Ok(here) = std::env::current_dir() else {
        return path.display().to_string();
    };
    match path.strip_prefix(&here) {
        Ok(p) if p.as_os_str().is_empty() => ".".to_string(),
        Ok(p) => p.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

/// QUUX's main memory from `--main-memory-size`'s word, in 64K-word boards
/// as the machine is built: a whole number of megawords with the unit
/// written, `32MW`, in [`Geometry::main_memory_mw`]'s range for the
/// revision. Nothing else is taken --- no KW, no fraction, and never a
/// bare M, which could be read as megabytes --- and the word is
/// case-sensitive, as every flag's word is.
///
/// [`Geometry::main_memory_mw`]: crate::machine::Geometry::main_memory_mw
fn main_memory_amount(
    word: Option<&str>,
    geometry: crate::machine::Geometry,
) -> Result<usize, String> {
    let unit = "main memory is given in megawords, with the unit MW, such as 32MW";
    let Some(word) = word else { return Err(format!("--main-memory-size: {unit}")) };
    let n = word
        .strip_suffix("MW")
        .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|n| n.parse::<usize>().ok())
        .ok_or_else(|| format!("--main-memory-size {word}: {unit}"))?;
    let range = geometry.main_memory_mw();
    if !range.contains(&n) {
        return Err(format!(
            "--main-memory-size {word}: revision {}'s main memory is {}MW to {}MW",
            geometry.revision().unwrap_or(0),
            range.start(),
            range.end()
        ));
    }
    Ok(n << 4)
}

/// Main memory's size for `boards` of 64K words, in KW, or in MW when it
/// comes to whole ones.
fn memory_size(boards: usize) -> String {
    let kw = boards * 64;
    if kw.is_multiple_of(1024) { format!("{} MW", kw / 1024) } else { format!("{kw} KW") }
}

/// Attaches each pack, [`pack_choice`], to its unit.
fn attach(m: &mut Machine, packs: &[Pack]) {
    for (p, unit, read_only, write_protect) in pack_choice(packs) {
        if m.block_disk.is_some() && unit != 0 {
            fail(&format!(
                "{}: block-disk has one pack, unit 0, and this is unit {unit}",
                p.display()
            ));
        }
        // QUUX's disk is a file of any size, raw or a VHD (contract Q8),
        // opened as the CADR's is: read-write, or read-only with `ro`, a
        // written block then kept for the run and a checkpoint.
        if let Some(d) = m.block_disk.as_mut() {
            let opened = if read_only {
                crate::disk_image::Disk::open(&p)
            } else {
                crate::disk_image::Disk::open_rw(&p)
            };
            match opened {
                Ok(disk) => d.attach(disk),
                Err(e) => fail(&e.to_string()),
            }
            continue;
        }
        // A drive writes its pack, so the image is opened read-write and a
        // written block goes into the file. With `ro` the file is opened
        // read-only and the drive is a writable one all the same: a
        // written block stays in memory for the run and goes into a
        // checkpoint instead. `wp` is the drive's own read-only switch,
        // `STATUS<7>`, and a write faults as MIT says it does, so nothing
        // is written at all.
        let opened = if read_only {
            Unit::open(&p, Geometry::T300)
        } else {
            Unit::open_rw(&p, Geometry::T300)
        };
        let mut u = match opened {
            Ok(u) => u,
            Err(e) => fail(&format!("{}: {e}", p.display())),
        };
        u.read_only = write_protect;
        m.disk.attach(unit, u);
    }
}

/// **What `--color-tv` asked for**: no second display board, the
/// behavioral one, or the LISPM TV netlist wrapped to the color
/// addresses on `chip`'s backplane.
///
/// The bare flag is the netlist on `chip` and the model on the other two
/// engines, as every board on `chip` is a netlist unless a flag says
/// otherwise; `--color-tv netlist` is `chip`'s alone and is refused by the
/// engine's name elsewhere, there being no backplane to put a board on.
/// The model is fitted behind the buses whichever it is, so the color
/// picture is read off `machine.color_tv` either way, exactly as the main
/// screen's is off `machine.tv` beside its netlist board.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ColorTv {
    /// One screen. `17200000` and `17377750` answer with an NXM, which is
    /// how `COLOR-EXISTS-P` finds out which machine this is.
    Off,
    /// The model, [`crate::machine::Machine::fit_color_tv`].
    Model,
    /// The netlist on the backplane, `data/LISPMTV.netlist` through
    /// [`netlist::parse_color_tv`].
    Netlist,
}

impl ColorTv {
    /// Whether a second display board is fitted at all.
    fn fitted(self) -> bool {
        self != ColorTv::Off
    }

    /// The word the start line, the checkpoint and the refusals carry.
    fn name(self) -> &'static str {
        match self {
            ColorTv::Off => "none",
            ColorTv::Model => "model",
            ColorTv::Netlist => "netlist",
        }
    }
}

fn machine(
    prom: &[Insn],
    packs: &[Pack],
    memory_boards: usize,
    (tv_board, video_size): (TvBoard, (usize, usize)),
    color_tv: ColorTv,
    (geometry, block_disk, rtc, file_roots): (
        crate::machine::Geometry,
        bool,
        crate::machine::Rtc,
        &crate::file_device::Mounts,
    ),
) -> Machine {
    let mut m = Machine::with_geometry(geometry, memory_boards);
    m.file_device.mounts = file_roots.clone();
    // Counted from the machine's clock at power-on, which is now.
    m.rtc = match rtc {
        crate::machine::Rtc::Counted { start, .. } => {
            crate::machine::Rtc::Counted { start, base_ns: m.ns }
        }
        live => live,
    };
    if block_disk {
        m.block_disk = Some(crate::block_disk::BlockDisk::new(crate::block_disk::BLOCK_NS));
    }
    m.load_prom(prom);
    m.tv.set_video_size(video_size.0, video_size.1);
    m.tv.set_board(tv_board);
    if color_tv.fitted() {
        m.fit_color_tv();
    }
    attach(&mut m, packs);
    m
}

/// The boot PROM this run loads: MIT's own, built in, or QUUX's on
/// `quux`, unless `--prom` names an MCR microcode file of one's own.
///
/// A file muir cannot read stops the run before it starts. It is the
/// program the machine is about to execute, so there is nothing to fall
/// back on: 512 zero words are not a boot PROM.
fn boot_prom(file: Option<&Path>, geometry: crate::machine::Geometry, resuming: bool) -> Vec<Insn> {
    let Some(path) = file else {
        // PROM 2001 is MIT's sections, which revision 15 does not read
        // (A15b.1), and no PROM of revision 15's is built in; a resume
        // takes the checkpoint's.
        if geometry.extended() && resuming {
            return vec![Insn::extended(0); crate::machine::PROM_WORDS];
        }
        if geometry.extended() {
            usage("QUUX revision 15 has no built-in boot PROM: --prom <file>, a revision-15 .mcr");
        }
        return if geometry == crate::machine::Geometry::CADR {
            crate::prom::boot_prom()
        } else {
            crate::prom::quux_boot_prom()
        };
    };
    let bytes =
        std::fs::read(path).unwrap_or_else(|e| usage(&format!("--prom {}: {e}", shown(path))));
    // QUUX's PROM is assembled at 36000, its own addresses (contract Q2).
    let parsed = if geometry == crate::machine::Geometry::CADR {
        crate::prom::parse_mcr(&bytes)
    } else {
        crate::prom::parse_quux_mcr(&bytes, geometry)
    };
    parsed.unwrap_or_else(|e| usage(&format!("--prom {}: {e}", shown(path))))
}

/// What the setup says the boot PROM is: which file, and how it stands to
/// MIT's own.
///
/// Recovered copies of the boot PROM are not all the same program --- two
/// builds of "version 9" exist that differ in 214 of their 454 words, and
/// nothing about a copy announces which it is --- so a run on a file of
/// one's own says how the file stands to MIT's: word for word, or how
/// many words apart.
fn prom_shown(file: Option<&Path>, prom: &[Insn], geometry: crate::machine::Geometry) -> String {
    let Some(path) = file else {
        if geometry.extended() {
            return "the checkpoint's".to_string();
        }
        return if geometry == crate::machine::Geometry::CADR {
            "built in, System 100's own sys/ubin/promh.mcr, version 9".to_string()
        } else {
            "built in, QUUX's data/quux-promh.mcr, version 2001, at 36000".to_string()
        };
    };
    // Revision 15's PROM has no built-in one to be measured against.
    if geometry.extended() {
        return format!("{}, a revision-15 .mcr", shown(path));
    }
    let (theirs, whose) = if geometry == crate::machine::Geometry::CADR {
        (crate::prom::boot_prom(), "MIT's own")
    } else {
        (crate::prom::quux_boot_prom(), "QUUX's own")
    };
    match prom.iter().zip(&theirs).filter(|(a, b)| a != b).count() {
        0 => format!("{}, {whose} word for word", shown(path)),
        n => format!("{}, {n} of its {} words differing from {whose}", shown(path), theirs.len()),
    }
}

/// Where a run stops: after so many microcycles, or when the PC reaches
/// an address --- one for the boot PROM enabled, one for it disabled, the
/// two sharing their low addresses --- whichever comes first.
#[derive(Clone, Copy)]
struct Stop {
    after: u64,
    at: Option<u16>,
    at_prom: Option<u16>,
}

impl Stop {
    /// Whether the PC, with the PROM as it is, is where the run stops. A PC
    /// that holds a control-store write's address is not where the program
    /// is ([`Engine::pc_is_a_write`]) and stops nothing.
    fn reached(&self, (pc, prom_enabled, a_write): (u16, bool, bool)) -> bool {
        !a_write && if prom_enabled { self.at_prom == Some(pc) } else { self.at == Some(pc) }
    }

    /// Says how the run ended, after the rate.
    fn conclude(
        &self,
        ran: u64,
        (pc, prom_enabled, a_write): (u16, bool, bool),
        halt: Option<crate::machine::Halt>,
    ) {
        let prom = if prom_enabled { " in the PROM" } else { "" };
        if self.reached((pc, prom_enabled, a_write)) {
            println!("       stopped at PC {pc:o}{prom} after {ran}");
        } else if let Some(h) = halt {
            println!("       stopped after {ran}: {h:?}");
        } else if self.at.is_some() || self.at_prom.is_some() {
            println!("       stop not reached in {ran}; PC {pc:o}{prom}");
        } else {
            // A run that simply reaches its cycle count still has to say
            // where it got to. A nine-hour `--stop-after` through the
            // boot's microcode load reported nothing but a rate, and
            // whether it had reached microcode 323 at all could not be
            // told from its output.
            println!("       ran out at {ran}; PC {pc:o}{prom}");
        }
    }
}

/// Where an engine's PC is, for [`Stop`]: the PC, whether it is the
/// PROM's ([`crate::machine::Machine::in_prom`]), and whether it is a
/// control-store write's ([`Engine::pc_is_a_write`]).
fn stop_pc<E: Engine>(e: &E) -> (u16, bool, bool) {
    (e.pc(), e.machine().in_prom(e.pc()), e.pc_is_a_write())
}

/// The lashup in one process: this machine, the debugger, with the
/// terminal and the stops, and the debuggee stepped beside it as the cable
/// allows --- [`Lashup::step`] moves whichever may move, so a microcycle
/// counted here is one of the debugger's.
#[allow(clippy::too_many_arguments)]
fn time_lashup(
    mut lashup: Lashup,
    stop: Stop,
    terminal: Option<&mut Terminal>,
    debuggee_terminal: Option<&mut Terminal>,
    mut glass: Option<&mut Glass>,
    color: Option<&mut Terminal>,
    capture: Option<(PathBuf, bool)>,
    color_capture: Option<(PathBuf, bool)>,
) {
    let t = Instant::now();
    let mut halt = None;
    let (mut terminal, mut debuggee_terminal) = (terminal, debuggee_terminal);
    // The color TV is the debugger's board: `--color-tv` fits one machine,
    // the one this process is running as its own.
    let mut color = color;
    // Both screens on one canvas: the debugger's at the left and the
    // debuggee's at the right, timed by the debugger's clock, which the
    // lashup holds the debuggee's to within a generator cycle.
    let mut capture = capture.map(|(path, time)| (path, Recorder::pair(time)));
    // The color screen is this machine's alone: the debuggee has no
    // second display board, `--color-tv` fitting the machine this process
    // runs as its own.
    let mut color_capture = color_capture.map(|(path, time)| (path, ColorRecorder::new(time)));
    let (mut keyboard, mut mouse) = (a_keyboard(), Mouse::new());
    let (mut b_keyboard, mut b_mouse) = (a_keyboard(), Mouse::new());
    let mut last_poll = Instant::now();
    let mut ran = 0;
    let (mut a_cycles, mut b_cycles) = (0u64, 0u64);
    catch_interrupts();
    let mut interrupts_seen = 0;
    loop {
        let e = &lashup.debugger;
        if ran >= stop.after || stop.reached(stop_pc(e)) {
            break;
        }
        if interrupted(&mut interrupts_seen) {
            break;
        }
        let before = lashup.steps;
        if let Err(h) = lashup.step() {
            halt = Some(h);
            break;
        }
        if lashup.steps.0 == before.0 {
            continue;
        }
        ran += 1;
        a_cycles = lashup.debugger.machine().cycles;
        b_cycles = lashup.debuggee.machine().cycles;
        if ran % TERMINAL_CHECK == 0 {
            if let Some((_, rec)) = capture.as_mut() {
                rec.sample_pair(
                    &lashup.debugger.machine().tv,
                    &lashup.debuggee.machine().tv,
                    lashup.debugger.ns(),
                    wall_clock(),
                );
            }
            if let Some((_, rec)) = color_capture.as_mut()
                && let Some(tv) = lashup.debugger.machine().color_tv.as_ref()
            {
                rec.sample(tv, lashup.debugger.ns(), wall_clock());
            }
            let poll = last_poll.elapsed() >= TERMINAL_INTERVAL;
            let e = &mut lashup.debugger;
            attend(
                terminal.as_deref_mut(),
                color.as_deref_mut(),
                glass.as_deref_mut(),
                poll,
                e,
                &mut keyboard,
                &mut mouse,
            );
            let e = &mut lashup.debuggee;
            attend(
                debuggee_terminal.as_deref_mut(),
                None,
                None,
                poll,
                e,
                &mut b_keyboard,
                &mut b_mouse,
            );
            if poll {
                last_poll = Instant::now();
            }
        }
    }
    report("rtl, debugger", ran, t.elapsed().as_secs_f64(), crate::ioboard::CYCLE_NS);
    let e = &lashup.debugger;
    stop.conclude(ran, stop_pc(e), halt);
    println!(
        "       debuggee: {b_cycles} microcycles to {} ns, PC {:o}{}; debugger {a_cycles} to {} ns; \
         {} debug cycles on the cable",
        lashup.debuggee.ns(),
        lashup.debuggee.pc(),
        if lashup.debuggee.machine().in_prom(lashup.debuggee.pc()) { " in the PROM" } else { "" },
        lashup.debugger.ns(),
        lashup.debugger.debug_cycles()
    );
    if let Some((path, rec)) = capture.as_mut() {
        rec.sample_pair(
            &lashup.debugger.machine().tv,
            &lashup.debuggee.machine().tv,
            lashup.debugger.ns(),
            wall_clock(),
        );
        write_capture(path, rec);
    }
    if let Some((path, rec)) = color_capture.as_mut() {
        if let Some(tv) = lashup.debugger.machine().color_tv.as_ref() {
            rec.sample(tv, lashup.debugger.ns(), wall_clock());
        }
        write_color_capture(path, rec);
    }
    let mut screens = Vec::new();
    if let Some(term) = terminal {
        screens.push((term, &lashup.debugger.machine().tv));
    }
    if let Some(term) = debuggee_terminal {
        screens.push((term, &lashup.debuggee.machine().tv));
    }
    if let (Some(term), Some(tv)) = (color, lashup.debugger.machine().color_tv.as_ref()) {
        screens.push((term, tv));
    }
    serve_last_screens(&mut screens, &mut interrupts_seen);
}

/// What a run loop steps: a machine alone ([`Alone`]), one with DBGIN's
/// connector at it ([`Connector`]), or the debugger's end of the cable over
/// TCP ([`Remote`]).  One loop, [`time_engine`], for the three: what
/// differs is the step --- a microcycle, or a wait for the other end's
/// promise --- and what is on the cable.
trait Stepper {
    type E: Engine;
    fn engine(&self) -> &Self::E;
    fn engine_mut(&mut self) -> &mut Self::E;
    /// One turn: whether a microcycle ran.  None ran when the other end of
    /// the cable had not promised one yet and its next message was waited
    /// for instead, or when the cable went.
    fn step(&mut self) -> Result<bool, crate::lashup::Error>;
    /// Between microcycles: the connector's listener looked at, and what
    /// it found said.
    fn attend(&mut self) {}
    /// A debugger is on this machine's DBGIN, or this machine is on a
    /// debuggee's: the prompt's `checkpoint` and the capture commands are
    /// refused --- the cable is in the bus interface's state, and the two
    /// machines are two clocks.
    fn on_cable(&self) -> bool {
        false
    }
    /// Whether a machine that stops itself is held here, as a machine
    /// alone is.  Not with a debugger on its DBGIN: a halted debuggee is
    /// what CC reads through the cable, and a held one would answer
    /// nothing.
    fn holds_on_self_halt(&self) -> bool {
        true
    }
    /// Debug cycles over the cable, if there was ever one.
    fn debug_cycles(&self) -> Option<u64> {
        None
    }
    /// The run's end: the other end of the cable told, and waited for.
    fn finish(&mut self) -> Result<(), crate::lashup::Error> {
        Ok(())
    }
}

/// A machine with no end of the cable: `micro`, which has no timing model.
struct Alone<E: Engine>(E);

impl<E: Engine> Stepper for Alone<E> {
    type E = E;
    fn engine(&self) -> &E {
        &self.0
    }
    fn engine_mut(&mut self) -> &mut E {
        &mut self.0
    }
    fn step(&mut self) -> Result<bool, crate::lashup::Error> {
        self.0.step()?;
        Ok(true)
    }
}

/// A machine with DBGIN's connector at it: on its own until a debugger
/// connects, in step with the debugger while one is on the cable, on its
/// own again when the cable goes, and each of those said on stderr.
///
/// **A hold here is felt at the other end.** Each end steps only as far
/// as the other has promised and waits for a message otherwise
/// ([`Remote`]), so a held debuggee holds the debugger at its next
/// request, whose acknowledgement it then waits for --- the other process
/// sits in [`Remote::step`]'s wait, its terminal and its own prompt
/// unattended, until this end runs on.  A slow debuggee does the same to
/// its debugger, the netlist one above all, and the cable is built for
/// it: `continue` puts both back as they were.
impl<E: Engine + CableEnd> Stepper for Connector<E> {
    type E = E;
    fn engine(&self) -> &E {
        self.machine()
    }
    fn engine_mut(&mut self) -> &mut E {
        self.machine_mut()
    }
    fn step(&mut self) -> Result<bool, crate::lashup::Error> {
        if self.debugger().is_none() {
            self.machine_mut().step()?;
            return Ok(true);
        }
        match self.step_cabled()? {
            Turn::Stepped => Ok(true),
            Turn::Waited => Ok(false),
            Turn::Unplugged(why) => {
                say_unplugged(self.addr(), &why);
                Ok(false)
            }
        }
    }
    fn attend(&mut self) {
        say_plugged(Connector::attend(self));
    }
    fn on_cable(&self) -> bool {
        self.debugger().is_some()
    }
    fn holds_on_self_halt(&self) -> bool {
        self.debugger().is_none()
    }
    fn debug_cycles(&self) -> Option<u64> {
        (self.connections() > 0).then(|| Connector::debug_cycles(self))
    }
    fn finish(&mut self) -> Result<(), crate::lashup::Error> {
        Connector::finish(self)
    }
}

/// The debugger's end of the cable over TCP: this machine's DBGOUT on the
/// stream to a debuggee in another program, the two run in step.  A hold
/// here is felt at the other end as a debuggee's is, above: a held
/// debugger holds the debuggee once it has run to the debugger's last
/// promise.  A debuggee that stops itself is this machine's to notice,
/// and this machine stopping itself is held here as a machine alone is.
impl<E: Engine + CableEnd> Stepper for Remote<E> {
    type E = E;
    fn engine(&self) -> &E {
        &self.machine
    }
    fn engine_mut(&mut self) -> &mut E {
        &mut self.machine
    }
    fn step(&mut self) -> Result<bool, crate::lashup::Error> {
        Remote::step(self)
    }
    fn on_cable(&self) -> bool {
        true
    }
    fn debug_cycles(&self) -> Option<u64> {
        Some(Remote::debug_cycles(self))
    }
    fn finish(&mut self) -> Result<(), crate::lashup::Error> {
        Remote::finish(self)
    }
}

/// What the connector found at its listener, said: a debugger plugged in,
/// or one refused while another is on.
fn say_plugged(plug: Option<Plug>) {
    match plug {
        Some(Plug::Connected(from)) => {
            eprintln!("debug cable: the debugger connected from {from}");
        }
        Some(Plug::Refused(from)) => {
            eprintln!(
                "debug cable: a second debugger from {from} refused: one cable per connector"
            );
        }
        None => {}
    }
}

/// The cable gone, said, with where the connector listens again.
fn say_unplugged(addr: Option<SocketAddr>, why: &str) {
    match addr {
        Some(a) => eprintln!("debug cable: {why}; DBGIN listening at {a}"),
        None => eprintln!("debug cable: {why}"),
    }
}

/// The debugger's end of the cable to a debuggee in FPGA fabric: this
/// machine stepped and the window polled beside it by
/// [`FreeRunning::step`], with the terminal, the stops and the prompt as
/// for a machine alone. Nothing is promised either way and there is
/// nothing at the far end to agree with about stopping: the run ends
/// where this machine's own stops say, or at the prompt's `quit`, with
/// any request left at the window lifted as the window goes.
///
/// **A hold here leaves the window as it stands.** The debuggee runs on
/// its own crystal and is not held with the debugger.  A request standing
/// at the window when the hold comes on stands through it, and holds
/// `-DB NEED UB` down on the debuggee and its Unibus with it, until the
/// adapter's watchdog lifts it ([`crate::fabric::FAULT_WATCHDOG`]); the
/// debugger's own timeout on that cycle is in its clock, which the hold
/// stops, so the cycle is costed when the debugger runs on, as one the
/// adapter dropped is.
fn time_fabric(
    mut run: FreeRunning<crate::fabric::Fabric<crate::fabric::Mapped>>,
    stop: Stop,
    terminal: Option<&mut Terminal>,
    color: Option<&mut Terminal>,
    mut glass: Option<&mut Glass>,
    setup: &str,
) {
    let t = Instant::now();
    let mut halt = None;
    let mut terminal = terminal;
    let mut color = color;
    let (mut keyboard, mut mouse) = (a_keyboard(), Mouse::new());
    let mut last_poll = Instant::now();
    let mut ran = 0;
    let mut hold = Hold::open(false);
    catch_interrupts();
    while !hold.quit && ran < stop.after && !stop.reached(stop_pc(&run.debugger)) {
        if hold.on {
            std::thread::sleep(TERMINAL_INTERVAL / 4);
        } else {
            match run.step() {
                Ok(()) => {
                    ran += 1;
                    hold.stepped(&run.debugger, ran);
                }
                Err(crate::lashup::Error::Halt(h)) => {
                    halt = Some(h);
                    break;
                }
                Err(e) => {
                    eprintln!("{}: the debug cable: {e}", executable());
                    break;
                }
            }
            // A window that stops answering as the adapter is the end of
            // the run: what it gives after that is not data. An adapter
            // that has lost muir's request is said and not stopped on ---
            // the cycle is one the debugger times out, and the next begins
            // again.
            if let Some(fault) = run.debuggee.fault() {
                eprintln!("{}: the fabric's window: {}", executable(), fault.what);
                if fault.fatal {
                    break;
                }
            }
        }
        if !hold.check(ran) {
            continue;
        }
        let poll = last_poll.elapsed() >= TERMINAL_INTERVAL;
        attend(
            terminal.as_deref_mut(),
            color.as_deref_mut(),
            glass.as_deref_mut(),
            poll,
            &mut run.debugger,
            &mut keyboard,
            &mut mouse,
        );
        if poll {
            last_poll = Instant::now();
        }
        hold.self_halt(&run.debugger, ran);
        hold.interrupts(&run.debugger, ran);
        hold.lines(&mut run.debugger, ran, setup, &mut Writes::CableEnd);
    }
    hold.done();
    report("rtl, debugger", ran, t.elapsed().as_secs_f64(), crate::ioboard::CYCLE_NS);
    hold.conclude(&run.debugger, &stop, ran, halt);
    match run.debuggee.taken() {
        Some((count, faults)) => println!(
            "       {} debug cycles on the cable; the window took {count} requests, faults {faults:#x}",
            run.debugger.debug_cycles()
        ),
        None => println!(
            "       {} debug cycles on the cable; the window no longer answers as the adapter",
            run.debugger.debug_cycles()
        ),
    }
    if !hold.quit {
        let m = run.debugger.machine();
        let mut screens: Vec<(&mut Terminal, &crate::tv::Tv)> = Vec::new();
        if let Some(term) = terminal {
            screens.push((term, &m.tv));
        }
        if let (Some(term), Some(tv)) = (color, m.color_tv.as_ref()) {
            screens.push((term, tv));
        }
        serve_last_screens(&mut screens, &mut hold.interrupts_seen);
    }
}

/// What a run is to do besides run the machine: where it stops, what it
/// records as it goes, what it writes when it is over, what it says it is
/// when `info` asks, and whether it starts held.
struct Run<'a> {
    stop: Stop,
    capture: Option<(PathBuf, bool)>,
    /// The color screen's own recording, when `--color-tv-capture` asked
    /// for one: its file, and whether the clocks go below it, which is
    /// the main screen's recording's flag too.  A second recorder beside
    /// the main screen's rather than a second canvas, the two screens
    /// being two pictures of different shapes.
    color_capture: Option<(PathBuf, bool)>,
    checkpoint: Option<PathBuf>,
    setup: &'a str,
    /// The color TV's screen, when `--color-tv` fitted the board: a second
    /// RFB server, pixels only.  It rides here rather than beside the
    /// terminal because it is a second screen of the same machine and
    /// every run loop that serves one serves the other.
    color: Option<&'a mut Terminal>,
    /// The run starts held, with the machine as `--no-auto-boot` left it:
    /// the button unpressed, and nothing to run until the prompt's `boot`.
    hold: bool,
    /// `--pace`: the run is held to the machine's own speed, waiting when
    /// it is ahead of it ([`Pace`]).
    pace: bool,
    /// Whether a recording carries the clocks below the screen, which
    /// `--tv-capture-no-time` turns off: the prompt's `startcapture` makes
    /// its recorder with it.
    clocks: bool,
}

/// The prompt's hold on a run: no microcycle runs while it is on, `step`
/// takes it off for so many microcycles, and ^C and `quit` end the run
/// through it.  One in each run loop that has the prompt --- a machine on
/// its own or at either end of the debug cable over TCP, [`time_engine`],
/// and the debugger of a machine in fabric, [`time_fabric`] --- so that a
/// line and a ^C mean the same on each.  `chip`'s loop keeps its own, its
/// machine being nets and not an [`Engine`].
struct Hold {
    prompt: Option<Prompt>,
    /// The hold is on: no microcycle runs, and the terminal and the prompt
    /// are attended at the terminal's pace.  `--no-auto-boot` starts the
    /// run with it on, the machine halted and its button unpressed, before
    /// the first microcycle.
    on: bool,
    /// A `step` in flight: so many microcycles still to run before the
    /// hold comes back on.  No further line is read until they have run,
    /// so that lines act in the order they were typed.
    stepping: Option<u64>,
    /// The run is over: `quit`, ^C while held, or a hold nothing can run
    /// on.  A quit is the run's own end, so what it was to write gets
    /// written, and the last screen is not served after one.
    quit: bool,
    /// The ^Cs acted on so far, against [`INTERRUPTS`].
    interrupts_seen: u32,
}

/// What a run has behind the prompt's `checkpoint`, `startcapture` and
/// `endcapture`.
enum Writes<'a> {
    /// A machine on its own: the checkpoint is written as `name`'s, and
    /// the capture commands record the display, with the clocks below it
    /// unless `--tv-capture-no-time`.
    Alone { name: &'a str, capture: &'a mut Option<(PathBuf, Recorder)>, clocks: bool },
    /// An end of the debug cable writes neither, and says why: the cable
    /// is in the bus interface's state ([`crate::busint::Busint`] saves
    /// whether one is attached), so its checkpoint is none `--resume`,
    /// which takes a machine on its own, can start from; and over the
    /// cable the two machines are two clocks, which is why `--tv-capture`
    /// is refused there.
    CableEnd,
}

impl Hold {
    /// The prompt opened on stdin, if it can be, and the hold on or off
    /// as the run starts.
    fn open(on: bool) -> Hold {
        Hold { prompt: Prompt::open(), on, stepping: None, quit: false, interrupts_seen: 0 }
    }

    /// Whether this turn of the loop attends the terminal and the prompt:
    /// every turn while held, and every [`TERMINAL_CHECK`] microcycles
    /// otherwise.
    fn check(&self, ran: u64) -> bool {
        self.on || ran.is_multiple_of(TERMINAL_CHECK)
    }

    /// A microcycle ran: one fewer of a step in flight, and the hold back
    /// on when the last has, saying where the machine is.
    fn stepped<E: Engine>(&mut self, e: &E, ran: u64) {
        if let Some(left) = self.stepping.as_mut() {
            *left -= 1;
            if *left == 0 {
                self.stepping = None;
                self.on = true;
                say_pc(e, ran);
            }
        }
    }

    /// The machine stopping itself --- `(si:%halt)`, or the statistics
    /// counter --- looks like nothing at all from `step`, which goes on
    /// returning `Ok` and running no microcycle.  So it is read off
    /// `FLAG-1` here, [`machrun_low`], and held on, once, rather than spun
    /// on: the screen has stopped, and without this the run says nothing
    /// about why.
    fn self_halt<E: Engine>(&mut self, e: &E, ran: u64) {
        if !self.on
            && let Some(why) = machrun_low(e)
        {
            self.on = true;
            self.stepping = None;
            say_machrun_low(why);
            say_pc(e, ran);
        }
    }

    /// ^C: with a prompt to go on from, the first holds the machine there
    /// and one more while held quits; with none, one quits.
    fn interrupts<E: Engine>(&mut self, e: &E, ran: u64) {
        let seen = INTERRUPTS.load(Ordering::SeqCst);
        while self.interrupts_seen < seen {
            self.interrupts_seen += 1;
            let at_prompt = self.prompt.as_ref().is_some_and(|p| !p.ended());
            if self.on || !at_prompt {
                self.quit = true;
            } else {
                self.on = true;
                self.stepping = None;
                if let Some(prompt) = self.prompt.as_ref() {
                    prompt.past_interrupt();
                }
                println!("held at ^C; continue runs on, ^C again quits");
                say_pc(e, ran);
            }
        }
    }

    /// The lines typed since the last turn, each acted on in order, and
    /// `muir: ` shown when the machine is held and nothing is in flight.
    /// Not while a step is: the microcycles it asked for run first, and
    /// the prompt comes back with where they left the machine.
    fn lines<E: Engine>(&mut self, e: &mut E, ran: u64, setup: &str, writes: &mut Writes<'_>) {
        if self.stepping.is_some() {
            return;
        }
        let Some(prompt) = self.prompt.as_ref() else { return };
        // Read before the lines are: the reader thread sets it after the
        // last line it will ever send, so a hold left standing when this
        // was already true is one nothing can run on.
        let ending = prompt.ended();
        while let Some(line) = prompt.line() {
            match crate::prompt::parse(&line) {
                Ok(None) => {}
                Ok(Some(Command::Boot)) => {
                    // The button starts the machine: it presets RUN, and a
                    // finger on it is all a CADR is given.  So the hold
                    // comes off with it.
                    e.boot();
                    say_pc(e, ran);
                    self.on = false;
                }
                Ok(Some(Command::Hold)) => {
                    self.on = true;
                    say_pc(e, ran);
                }
                Ok(Some(Command::Continue)) => {
                    if halted(e) {
                        take_off(e);
                        self.on = false;
                    } else if let Some(why) = machrun_low(e) {
                        say_machrun_low(why);
                    } else if self.on {
                        self.on = false;
                    } else {
                        say_running_already();
                    }
                }
                Ok(Some(Command::Step(n))) => {
                    if halted(e) {
                        say_halted();
                    } else if let Some(why) = machrun_low(e) {
                        say_machrun_low(why);
                    } else {
                        self.on = false;
                        self.stepping = Some(n);
                        break;
                    }
                }
                Ok(Some(Command::Pc)) => say_pc(e, ran),
                Ok(Some(Command::Registers)) => print!("{}", say_registers(e)),
                Ok(Some(Command::Dump { memory, from, words })) => {
                    match say_memory(e.machine(), memory, from, words) {
                        Ok(dump) => print!("{dump}"),
                        Err(what) => println!("prompt: {what}"),
                    }
                }
                // Main memory is an array here and a physical address is
                // an index into it; on `chip` it is the memory boards'
                // cells and the same address picks the board.
                Ok(Some(Command::Mem { from, words })) => {
                    let m = e.machine();
                    // Each word whole, at the machine's width.
                    let read = |a: usize| m.main.get(a).copied();
                    let bits = m.geometry.word_bits;
                    match crate::prompt::main_dump_wide(from, words, m.main.len(), read, bits) {
                        Ok(dump) => print!("{dump}"),
                        Err(what) => println!("prompt: {what}"),
                    }
                }
                Ok(Some(Command::Net(_) | Command::Watch { .. })) => {
                    println!("prompt: nets are the chip engine's --- this machine is");
                    println!("        registers and memories and has no wires to read;");
                    println!("        `reg` gives the registers and `pc` the PC");
                }
                Ok(Some(Command::Info)) => print!("{setup}"),
                Ok(Some(Command::Keys)) => print!("{}", keys_in_force()),
                Ok(Some(Command::Screenshot(path))) => {
                    let path = path.unwrap_or_else(|| timestamped("png"));
                    write_screenshot(&path, &e.machine().tv);
                }
                Ok(Some(Command::StartCapture(path))) => match writes {
                    Writes::Alone { capture, clocks, .. } => match capture.as_ref() {
                        Some((going, _)) => println!(
                            "capture: one is going already, to {}; endcapture closes it",
                            going.display()
                        ),
                        None => {
                            let path = path.unwrap_or_else(|| timestamped("gif"));
                            println!(
                                "capture: recording the display to {}{}; endcapture writes it, and so does the stop",
                                path.display(),
                                if *clocks { "" } else { ", no clocks" }
                            );
                            **capture = Some((path, Recorder::new(*clocks)));
                        }
                    },
                    Writes::CableEnd => println!("capture: {NO_CAPTURE_OVER_THE_CABLE}"),
                },
                Ok(Some(Command::EndCapture)) => match writes {
                    Writes::Alone { capture, .. } => match capture.take() {
                        Some((path, mut rec)) => {
                            rec.sample(&e.machine().tv, e.machine().ns, wall_clock());
                            write_capture(&path, &rec);
                        }
                        None => println!("capture: none is going; startcapture begins one"),
                    },
                    Writes::CableEnd => println!("capture: {NO_CAPTURE_OVER_THE_CABLE}"),
                },
                Ok(Some(Command::Checkpoint(path))) => match writes {
                    Writes::Alone { name, .. } => {
                        write_checkpoint(name, e, &path.unwrap_or_else(|| timestamped("chk")));
                    }
                    Writes::CableEnd => println!("checkpoint: {NO_CHECKPOINT_OVER_THE_CABLE}"),
                },
                Ok(Some(Command::Quit)) => {
                    self.quit = true;
                    break;
                }
                Ok(Some(Command::Help)) => print!("{}", crate::prompt::HELP),
                Err(what) => println!("prompt: {what}"),
            }
        }
        // A hold with no one left to type `continue` is a run that would
        // never end: stdin has ended, and ^C is the only thing that could
        // still reach it.  The run ends here instead, as a quit does, with
        // what it was to write written.
        if self.on && ending && !self.quit {
            println!("held, and stdin has ended: there is nothing to run the machine on");
            self.quit = true;
        }
        // The prompt is the held machine's: it is there while muir is
        // waiting to be told what to do next, and not while the machine is
        // running --- a line typed then is acted on all the same, there is
        // just nothing waiting for it.
        if self.on && !self.quit && self.stepping.is_none() {
            prompt.show();
        }
    }

    /// No further command will be typed: the line a `muir: ` is on is
    /// ended, so the run's last words start on one of their own.
    fn done(&self) {
        if let Some(prompt) = self.prompt.as_ref() {
            prompt.done();
        }
    }

    /// The run's last word: that it was quit at the prompt, or how the
    /// stop came.
    fn conclude<E: Engine>(
        &self,
        e: &E,
        stop: &Stop,
        ran: u64,
        halt: Option<crate::machine::Halt>,
    ) {
        if self.quit {
            let prom = if e.machine().in_prom(e.pc()) { " in the PROM" } else { "" };
            println!("       quit at PC {:o}{prom} after {ran}", e.pc());
        } else {
            stop.conclude(ran, stop_pc(e), halt);
        }
    }
}

/// Why an end of the debug cable writes no checkpoint: [`Writes::CableEnd`].
const NO_CHECKPOINT_OVER_THE_CABLE: &str = "none on an end of the debug cable: the cable is in \
     the bus interface's state, and --resume takes a machine on its own";

/// Why an end of the debug cable records no capture: [`Writes::CableEnd`].
const NO_CAPTURE_OVER_THE_CABLE: &str = "none on an end of the debug cable, where the two \
     machines are two clocks; --tv-capture is refused there for the same reason";

/// Runs a machine muir holds: alone, with DBGIN's connector at it, or at
/// the debugger's end of the cable ([`Stepper`]), with the terminal, the
/// serial port, the stops and the prompt.  At the end the other end of the
/// cable, if there is one, is told and the two agree to stop.
fn time_engine<S: Stepper>(
    name: &str,
    mut s: S,
    terminal: Option<&mut Terminal>,
    mut glass: Option<&mut Glass>,
    serial: Option<&mut Endpoint>,
    run: Run,
) {
    let Run {
        stop,
        capture,
        color_capture,
        checkpoint,
        setup,
        hold: held,
        pace: paced,
        clocks,
        color,
    } = run;
    let t = Instant::now();
    let mut ran = 0;
    let mut halt = None;
    let mut terminal = terminal;
    let mut color = color;
    let mut serial = serial;
    let mut keyboard = a_keyboard();
    let mut mouse = Mouse::new();
    let mut last_poll = Instant::now();
    let mut last_serial = Instant::now();
    // `--pace`: the machine's own clock against the host's, anchored where
    // the run begins.
    let mut pace = paced.then(|| Pace::new(Instant::now(), s.engine().machine().ns));
    let mut capture = capture.map(|(path, time)| (path, Recorder::new(time)));
    let mut color_capture = color_capture.map(|(path, time)| (path, ColorRecorder::new(time)));
    let mut hold = Hold::open(held);
    catch_interrupts();
    while !hold.quit && ran < stop.after && !stop.reached(stop_pc(s.engine())) {
        if hold.on {
            std::thread::sleep(TERMINAL_INTERVAL / 4);
        } else {
            match s.step() {
                Ok(true) => {
                    ran += 1;
                    hold.stepped(s.engine(), ran);
                }
                // The other end's message was waited for, or the cable
                // went: no microcycle ran, so nothing below is due.
                Ok(false) => continue,
                Err(crate::lashup::Error::Halt(h)) => {
                    halt = Some(h);
                    break;
                }
                Err(e) => {
                    eprintln!("{}: the debug cable: {e}", executable());
                    break;
                }
            }
        }
        if !hold.check(ran) {
            continue;
        }
        // The connector's listener, held or not: a debugger may come to a
        // machine standing at the prompt.
        s.attend();
        if !hold.on && (capture.is_some() || color_capture.is_some()) {
            // One reading of the wall clock for the two recordings: they
            // are two files of one run, and a frame of each is one
            // instant.
            let now = wall_clock();
            let m = s.engine().machine();
            if let Some((_, rec)) = capture.as_mut() {
                rec.sample(&m.tv, m.ns, now);
            }
            // The color screen, when the board is fitted.
            if let Some((_, rec)) = color_capture.as_mut()
                && let Some(tv) = m.color_tv.as_ref()
            {
                rec.sample(tv, m.ns, now);
            }
        }
        if last_poll.elapsed() >= TERMINAL_INTERVAL {
            if let Some(term) = terminal.as_deref_mut() {
                let e = s.engine_mut();
                term.poll(Frame::of(&e.machine().tv));
                for (keysym, down) in term.take_keys() {
                    keyboard.key(keysym, down);
                }
                for (buttons, x, y) in term.take_pointers() {
                    mouse.pointer(buttons, x, y);
                }
                if e.machine_mut().ioboard.take_beep() {
                    term.ring();
                }
            }
            // The glass TTYs: the screen as text out, and what was typed
            // in to the one keyboard a machine has.
            if let Some(glass) = glass.as_deref_mut() {
                let keys = glass.poll(Frame::of(&s.engine().machine().tv));
                for (keysym, down) in keys {
                    keyboard.key(keysym, down);
                }
            }
            // The color screen, when the board is fitted: the picture
            // out and nothing in.
            if let Some(term) = color.as_deref_mut()
                && let Some(tv) = s.engine().machine().color_tv.as_ref()
            {
                term.poll(Frame::of(tv));
            }
            last_poll = Instant::now();
        }
        // The keyboard hands the board one word at a time, as the board
        // takes them; a glance every check is far more often than the
        // machine reads it. The mouse's counts go in whole. The boot
        // sequence's word, once the board has decoded it, presses the boot.
        {
            let e = s.engine_mut();
            deliver_input(e.machine_mut(), &mut keyboard, &mut mouse);
            e.keyboard_boot();
        }
        // The serial port's endpoint, when `--serial` opened one: what the
        // port has finished sending goes to the socket, and what was typed
        // at it goes on the cable. The port takes its own frame time over
        // each character either way, so a burst read in one turn still
        // arrives one frame at a time.
        if let Some(port) = serial.as_deref_mut()
            && last_serial.elapsed() >= SERIAL_INTERVAL
        {
            let m = s.engine_mut().machine_mut();
            let now = m.ns;
            port.poll_cable(&mut m.ioboard.serial.cable, now);
            last_serial = Instant::now();
        }
        if s.holds_on_self_halt() {
            hold.self_halt(s.engine(), ran);
        }
        hold.interrupts(s.engine(), ran);
        let mut writes = if s.on_cable() {
            Writes::CableEnd
        } else {
            Writes::Alone { name, capture: &mut capture, clocks }
        };
        hold.lines(s.engine_mut(), ran, setup, &mut writes);
        // `--pace`: the wait that keeps the run at the machine's own
        // speed, taken at the end of the turn so that the terminal, the
        // port and the prompt have had theirs first.  Held at the prompt,
        // or stepped by a debugger on the cable, there is nothing to pace
        // and the anchor moves instead.
        if let Some(pace) = pace.as_mut() {
            let ns = s.engine().machine().ns;
            if hold.on || s.on_cable() {
                pace.anchor(Instant::now(), ns);
            } else if let Some(wait) = pace.owed(Instant::now(), ns) {
                std::thread::sleep(wait);
            }
        }
    }
    hold.done();
    // One last turn, so that what the port sent between the final poll and
    // the stop reaches whoever is attached before the socket closes.
    if let Some(port) = serial {
        let m = s.engine_mut().machine_mut();
        let now = m.ns;
        port.poll_cable(&mut m.ioboard.serial.cable, now);
    }
    report(name, ran, t.elapsed().as_secs_f64(), s.engine().nominal_cycle_ns());
    hold.conclude(s.engine(), &stop, ran, halt);
    if let Some(n) = s.debug_cycles() {
        println!("       {n} debug cycles on the cable");
    }
    if let Err(e) = s.finish() {
        eprintln!("{}: the debug cable at the end: {e}", executable());
    }
    if let Some((path, rec)) = capture.as_mut() {
        let m = s.engine().machine();
        rec.sample(&m.tv, m.ns, wall_clock());
        write_capture(path, rec);
    }
    if let Some((path, rec)) = color_capture.as_mut() {
        let m = s.engine().machine();
        if let Some(tv) = m.color_tv.as_ref() {
            rec.sample(tv, m.ns, wall_clock());
        }
        write_color_capture(path, rec);
    }
    if let Some(path) = &checkpoint {
        write_checkpoint(name, s.engine(), path);
    }
    if !hold.quit {
        let m = s.engine().machine();
        let mut screens: Vec<(&mut Terminal, &crate::tv::Tv)> = Vec::new();
        if let Some(term) = terminal {
            screens.push((term, &m.tv));
        }
        if let (Some(term), Some(tv)) = (color, m.color_tv.as_ref()) {
            screens.push((term, tv));
        }
        serve_last_screens(&mut screens, &mut hold.interrupts_seen);
    }
}

/// How many ^Cs have come since the run began: `SIGINT`'s handler counts
/// them, and the run loop acts on each between two microcycles.
static INTERRUPTS: AtomicU32 = AtomicU32::new(0);

extern "C" fn on_interrupt(_signal: std::ffi::c_int) {
    INTERRUPTS.fetch_add(1, Ordering::SeqCst);
}

/// **How many `SIGUSR1`s have come**: `kill -USR1` on a run asks it where
/// it is, and the run answers between two microcycles and carries on.
///
/// A long `chip` run had no prompt when this was added --- the process
/// had a terminal and nothing else --- so the only way to know where one
/// was was to infer it from what it had touched. Issue 86 has a run whose
/// state was read from pack mtimes, then from lit pixels, then from a
/// block-by-block comparison, two of the three retracted, over six hours,
/// with `pc` unanswered throughout.
static DUMPS: AtomicU32 = AtomicU32::new(0);

extern "C" fn on_dump(_signal: std::ffi::c_int) {
    DUMPS.fetch_add(1, Ordering::SeqCst);
}

/// `SIGINT`, 2 on every Unix; `SIGUSR1`, 30 on macOS and the BSDs and 10
/// on Linux.
const SIGINT: std::ffi::c_int = 2;
#[cfg(target_os = "linux")]
const SIGUSR1: std::ffi::c_int = 10;
#[cfg(not(target_os = "linux"))]
const SIGUSR1: std::ffi::c_int = 30;

// POSIX `signal`, declared here as `localtime_r` is: the handler is a
// function's address. The C libraries this builds against, macOS's and
// glibc, keep a handler installed after a signal, so one call serves the
// run.
unsafe extern "C" {
    fn signal(sig: std::ffi::c_int, handler: usize) -> usize;
}

/// Takes ^C for the rest of the process: counted, not fatal. The run loop
/// acts on the count between two microcycles and [`serve_last_screens`]
/// after the stop, and nothing after that waits on anything, so the
/// default is never put back: a ^C that ended the process would skip
/// every `Drop`.
fn catch_interrupts() {
    // SAFETY: installing a handler that does nothing but an atomic add,
    // which is safe to do in a signal handler.
    let handler: extern "C" fn(std::ffi::c_int) = on_interrupt;
    unsafe { signal(SIGINT, handler as *const () as usize) };
    // `SIGUSR1` is taken by every engine and acted on by `chip`, which
    // had no prompt when the signal was added and keeps it now that it
    // has one. Every engine, because the default action for it is to kill
    // the process: a signal sent to the wrong run of a pair would
    // otherwise end a run that had been going for hours.
    let dump: extern "C" fn(std::ffi::c_int) = on_dump;
    unsafe { signal(SIGUSR1, dump as *const () as usize) };
}

/// This run's own process id, for the banner line that says how to ask it
/// where it is.
fn pid() -> u32 {
    std::process::id()
}

/// Whether `SIGUSR1` has come since this was last asked, as
/// [`interrupted`] is for ^C.
fn asked_where(seen: &mut u32) -> bool {
    let now = DUMPS.load(Ordering::SeqCst);
    let asked = now > *seen;
    *seen = now;
    asked
}

/// Whether ^C has been pressed since this was last asked.
///
/// A run that has no prompt to hold the machine from --- the lashup in
/// one process --- ends on the first one, so that what the run was to
/// write is written: the recording, and the checkpoint.
/// A run with the prompt wants more than this, a first ^C holding the
/// machine and a second quitting, which is [`Hold::interrupts`]; `chip`'s
/// loop does the same inline.
fn interrupted(seen: &mut u32) -> bool {
    let now = INTERRUPTS.load(Ordering::SeqCst);
    let asked = now > *seen;
    *seen = now;
    asked
}

/// The prompt's answer to `pc`, and to `hold` and `step`: where the
/// machine is.
fn say_pc<E: Engine>(e: &E, ran: u64) {
    let m = e.machine();
    let prom = if m.in_prom(e.pc()) { " in the PROM" } else { "" };
    println!("PC {:o}{prom}; {} microcycles, {} ns; {ran} this run", e.pc(), m.cycles, m.ns);
}

/// Whether the machine is halted: `RUN` is clear, so the clock does not
/// reach the datapath and no microcycle can run.  It is what a CADR is
/// when the power comes on, and what `--no-auto-boot` leaves it as.
fn halted<E: Engine>(e: &E) -> bool {
    !e.machine().clock_control.run
}

/// Why `step` does nothing for a halted machine: `continue` sets `RUN`
/// and runs it on from where it stands, and the button starts it afresh.
fn say_halted() {
    println!(
        "the machine is halted, its RUN clear: continue sets RUN and runs it on from where it stands, boot presses the button that starts it"
    );
}

/// **`RUN` set as a console sets it, with no reset**: the prompt's
/// `continue` and `--continue` on a halted machine, which is how a board's
/// checkpoint comes, taken with `RUN` cleared.  CC's `CC-START-MACH` ends
/// `(SPY-WRITE SPY-CLK 1) ;TAKE OFF` (System 100's `sys/cc/lcadrd.lisp`):
/// the clock control register written with `RUN` alone up, `STEP`,
/// `NOP11`, `IDEBUG` and `LDSTAT` down.  `SRUN` follows it at the next
/// master clock edge, and the machine runs from its PC, its pipeline and
/// its memories as they stand; nothing of the console's registers is
/// reset and the PROM is not put back, which is what `boot` does.
fn take_off<E: Engine>(e: &mut E) {
    e.spy_write(crate::spy::CLK, 1);
    let prom = if e.machine().in_prom(e.pc()) { " in the PROM" } else { "" };
    say_took_off(e.pc(), prom);
}

/// [`take_off`] on a netlist machine: `RUN`, the 74S74 at OLORD1 1A14,
/// set where `-LDCLK` would clock `SPY0` up into it, and the board settled
/// on it.  The 74S175 at 1A09 that the same write strobe loads with
/// `STEP`, `NOP11`, `IDEBUG` and `LDSTAT` is left as it stands, there
/// being no write here to clock it; a halted checkpoint has them down.
fn take_off_chip(c: &mut Chip, run: netlist::NetId, pc_nets: &[netlist::NetId], prom: bool) {
    c.set_state_bit("OLORD1", "1A14", 0, true);
    debug_assert_eq!(c.net(run), Level::High, "RUN is bit 0 of the 74S74 at 1A14, pin 5");
    say_took_off(c.read(pc_nets) as u16, if prom { " in the PROM" } else { "" });
}

/// What [`take_off`] says, on every engine.
fn say_took_off(pc: u16, prom: &str) {
    println!("continue: RUN set; the machine runs on from PC {pc:o}{prom}, with no reset");
}

/// What `continue` says to a machine that is running and not held.
fn say_running_already() {
    println!("continue: the machine is running already");
}

/// Where a netlist machine is, for the prompt.  [`say_pc`]'s counterpart:
/// `Chip` is not an [`Engine`] and keeps no microcycle count of its own ---
/// `time_chip` counts them by the clock phase wrapping --- so this says the
/// PC and the run's count and leaves the rest out.
fn say_pc_chip(
    c: &Chip,
    pc_nets: &[netlist::NetId],
    ir_nets: &[netlist::NetId],
    ran: u64,
    prom_enabled: bool,
) {
    let prom = if prom_enabled { " in the PROM" } else { "" };
    println!("PC {:o}{prom}; {ran} microcycles this run", c.read(pc_nets) as u16);
    // **The instruction, which on a halt is the one that halted.** `IR` is
    // held while the machine is stopped, and `HALT-CONS` is `IR<11:10>`,
    // so a machine that stopped itself says here what stopped it --- which
    // the PC does not, `ILLOP` being a `POPJ` whose PC is the address it
    // popped rather than the trap.
    let ir = c.read(ir_nets);
    println!("IR {ir:#014x}; misc function {}", (ir >> 10) & 3);
}

/// **The machine has stopped itself with `RUN` still set**, and why, or
/// `None` if it is running.  `MACHRUN` is low because `ERR` is up under
/// `ERRSTOP` --- which is what `HALT-CONS` does, and so what System 100's
/// `(si:%halt)` does --- or because the statistics counter ran out under
/// `STATHENB`.  A `WAIT` is neither: the machine comes out of a bus wait
/// by itself.
///
/// This is read from `FLAG-1`, where a console reads it, because nothing
/// in [`Engine::step`] says it has happened: a stopped machine's `step`
/// goes on returning `Ok`, advancing the master clock and running no
/// microcycle, so a run loop that watched only the return would spin here
/// for as long as it was left to.  `tests/halt.rs` holds both engines to
/// that.
fn machrun_low<E: Engine>(e: &E) -> Option<&'static str> {
    let f = crate::spy::Flag1::of(e.spy_read(crate::spy::FLAG_1));
    if !f.srun {
        // A cleared RUN is the other halt, and `halted` is its name.
        return None;
    }
    if f.err && e.machine().mode.errstop {
        return Some("ERR is up under ERRSTOP, which is what HALT-CONS does: (si:%halt)");
    }
    if f.stathalt {
        return Some("the statistics counter ran out under STATHENB");
    }
    None
}

/// What a machine that stopped itself says, once, as it drops to the
/// prompt.  `boot` is the way on: the button presets `RUN` and resets the
/// console's registers, `ERRSTOP` among them, which is what a CADR's
/// operator does here too.
fn say_machrun_low(why: &str) {
    println!("the machine stopped itself: {why}");
    println!("it will not run on by itself; boot presses the button that starts it again");
}

/// The prompt's answer to `reg`: the machine's registers, in hex and as
/// characters.  The names are the hardware's: PC, OPC, Q, VMA, MD, LC,
/// SPCPTR, PDLPTR and PDLIDX are all names the boards' own nets carry, and
/// PDLPTR, SPC, VMA, MD, Q and LC are pages of MIT's drawing set besides.
///
/// OPC here is [`Machine::opc`], the PC of the microinstruction that just
/// executed, which every engine keeps.  The eight-deep OPCS shift register
/// the board holds behind it is `rtl`'s own, and a console reads it a word
/// at a time through the OPC control register.
fn say_registers<E: Engine>(e: &E) -> String {
    let m = e.machine();
    // The words whole, at the machine's width; the rest as they are kept.
    crate::prompt::registers_wide(
        &[
            ("PC", e.pc().into()),
            ("OPC", m.opc.into()),
            ("Q", m.q),
            ("VMA", m.vma),
            ("MD", m.md),
            ("LC", m.lc),
            ("SPCPTR", m.spcptr.into()),
            ("PDLPTR", m.pdl_pointer.into()),
            ("PDLIDX", m.pdl_index.into()),
            ("DISPATCH CONSTANT", m.dispatch_constant.into()),
            ("INTERRUPT CONTROL", m.interrupt_control.into()),
        ],
        m.geometry.word_bits,
    )
}

/// The prompt's answer to `amem`, `mmem`, `dmem`, `pdl` and `spc`: so many
/// words from an address, or the rest of the memory, or why there are
/// none there.
/// [`say_memory`] for `chip`, where a memory is the RAM chips' cells
/// rather than an array: `rams` is in [`Memory`]'s own order, built once
/// when the machine was.
fn say_chip_memory(
    c: &Chip,
    rams: &[crate::chip::Ram],
    memory: Memory,
    from: usize,
    words: Option<usize>,
) -> Result<String, String> {
    let ram = &rams[memory as usize];
    if from >= ram.len() {
        return Err(format!(
            "{} is {:o} words, and {from:o} is past its end",
            memory.name(),
            ram.len()
        ));
    }
    let to = match words {
        Some(n) => from.saturating_add(n).min(ram.len()),
        None => ram.len(),
    };
    let all: Vec<u32> = (from..to).map(|a| ram.word(c, a)).collect();
    Ok(crate::prompt::dump(&all, from))
}

fn say_memory(
    m: &Machine,
    memory: Memory,
    from: usize,
    words: Option<usize>,
) -> Result<String, String> {
    // A, M and the PDL buffer whole, at the machine's width; the dispatch
    // memory as many entries as the machine has.
    let wide = |w: &[u32]| w.iter().map(|&w| w.into()).collect::<Vec<u64>>();
    let (all, bits): (Vec<u64>, u32) = match memory {
        Memory::Amem => (m.amem.to_vec(), m.geometry.word_bits),
        Memory::Mmem => (m.mmem.to_vec(), m.geometry.word_bits),
        Memory::Dmem => (wide(&m.dmem[..m.geometry.dmem_words()]), 32),
        Memory::Pdl => (m.pdl.to_vec(), m.geometry.word_bits),
        Memory::Spc => (wide(&m.spc), 32),
    };
    if from >= all.len() {
        return Err(format!(
            "{} is {:o} words, and {from:o} is past its end",
            memory.name(),
            all.len()
        ));
    }
    // A count past the end is the rest of the memory, however far past:
    // the largest count the prompt reads, added to the address, would
    // otherwise overflow.
    let to = match words {
        Some(n) => from.saturating_add(n).min(all.len()),
        None => all.len(),
    };
    Ok(crate::prompt::dump_wide(&all[from..to], from, bits))
}

/// The prompt: a line on stdin is a command to muir itself,
/// [`crate::prompt`], read on a thread of its own and acted on between two
/// microcycles, where the terminal is attended.
struct Prompt {
    lines: std::sync::mpsc::Receiver<String>,
    /// Stdin has ended: no line will come, and there is no one to type
    /// `continue` at a hold.
    ended: std::sync::Arc<AtomicBool>,
    /// Someone is typing at a terminal, rather than a script feeding a
    /// pipe: the `muir: ` at a hold is for them, and so is the newline past
    /// what the terminal echoes of a ^C.
    terminal: bool,
    /// A `muir: ` is on the screen with nothing written past it yet.
    showing: std::cell::Cell<bool>,
}

impl Prompt {
    /// Opens the prompt on stdin when it can be read: a pipe or a file, or
    /// a terminal that muir is in the foreground of. A terminal muir is in
    /// the background of is left alone, since a read from it would stop the
    /// process, and there is no prompt.
    fn open() -> Option<Prompt> {
        use std::io::BufRead;
        if !Prompt::possible() {
            return None;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ended = std::sync::Arc::new(AtomicBool::new(false));
        let over = ended.clone();
        std::thread::Builder::new()
            .name("prompt".into())
            .spawn(move || {
                for line in std::io::stdin().lock().lines() {
                    let Ok(line) = line else { break };
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                over.store(true, Ordering::SeqCst);
            })
            .ok()?;
        let terminal = std::io::stdin().is_terminal();
        Some(Prompt { lines: rx, ended, terminal, showing: std::cell::Cell::new(false) })
    }

    /// A prompt a test types at in place of stdin: what goes down the
    /// sender comes up as lines, and stdin is never taken to have ended.
    #[cfg(test)]
    fn piped() -> (std::sync::mpsc::Sender<String>, Prompt) {
        let (tx, rx) = std::sync::mpsc::channel();
        let ended = std::sync::Arc::new(AtomicBool::new(false));
        (tx, Prompt { lines: rx, ended, terminal: false, showing: std::cell::Cell::new(false) })
    }

    /// Whether stdin can be read for a prompt: anything but a terminal
    /// muir is in the background of.
    fn possible() -> bool {
        !(std::io::stdin().is_terminal() && !stdin_is_foreground())
    }

    /// The next line typed, if one is waiting.
    fn line(&self) -> Option<String> {
        let line = self.lines.try_recv().ok();
        if line.is_some() {
            // The terminal echoed the Enter that ended it, so what muir
            // writes next starts on a line of its own and the `muir: ` the
            // line was typed after is behind.
            self.showing.set(false);
        }
        line
    }

    /// Writes `muir: `, muir's own line, while the machine is held and one
    /// is not already on the screen.  Only to a terminal: a pipe or a file
    /// gets muir's answers alone, as a script wants them.
    fn show(&self) {
        use std::io::Write;
        if self.showing.get() || !self.terminal {
            return;
        }
        print!("{}", crate::prompt::PROMPT);
        let _ = std::io::stdout().flush();
        self.showing.set(true);
    }

    /// Past what the terminal echoed of a ^C: it came after a `muir: `,
    /// with no Enter to end the line.
    fn past_interrupt(&self) {
        self.showing.set(false);
        if self.terminal {
            println!();
        }
    }

    /// No further command will be typed: the line a `muir: ` is on is
    /// ended, so the run's last words start on one of their own.
    fn done(&self) {
        if self.showing.replace(false) {
            println!();
        }
    }

    /// Whether stdin has ended, so that no line will come.
    fn ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }
}

/// Whether this process is in its terminal's foreground process group, so
/// that reading the terminal does not stop it with `SIGTTIN`: POSIX's
/// `tcgetpgrp` on stdin against `getpgrp`, declared here as `localtime_r`
/// is; both take and give a `pid_t`, an `int`.
fn stdin_is_foreground() -> bool {
    use std::ffi::c_int;
    unsafe extern "C" {
        fn tcgetpgrp(fd: c_int) -> c_int;
        fn getpgrp() -> c_int;
    }
    // SAFETY: two calls that take no pointers, on file descriptor 0.
    unsafe { tcgetpgrp(0) == getpgrp() }
}

/// Writes the engine and its machine to `path` and says how big it came.
fn write_checkpoint<E: Engine>(name: &str, e: &E, path: &Path) {
    // QUUX's file device with a handle open or a command queued holds state
    // on the host that no checkpoint carries (contract Q9).
    if let Some(why) = e.machine().checkpoint_refusal() {
        eprintln!("checkpoint: {} not written: {why}", path.display());
        return;
    }
    let mut w = crate::checkpoint::Writer::new();
    e.save(&mut w);
    let boards = e.machine().memory_boards();
    let version = crate::checkpoint::version_for(&e.machine().geometry);
    match crate::checkpoint::write_version(path, name, boards, version, &w.finish()) {
        Ok(n) => eprintln!(
            "checkpoint: {} at {} microcycles, {n} bytes",
            path.display(),
            e.machine().cycles
        ),
        Err(err) => eprintln!("checkpoint: could not write {}: {err}", path.display()),
    }
}

/// `muir-yyyymmdd-hhmmss.chk` in the current directory, by the wall clock.
fn timestamped(extension: &str) -> PathBuf {
    let t = local_time();
    PathBuf::from(format!(
        "muir-{:04}{:02}{:02}-{:02}{:02}{:02}.{extension}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    ))
}

/// The screen as it stands, as a PNG: [`Tv::png`], which is the
/// frame buffer as the monitor shows it.
fn write_screenshot(path: &Path, tv: &crate::tv::Tv) {
    match std::fs::write(path, tv.png()) {
        Ok(()) => eprintln!(
            "screenshot: {}, {} bytes",
            path.display(),
            std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
        ),
        Err(e) => eprintln!("screenshot: could not write {}: {e}", path.display()),
    }
}

/// Writes a netlist machine to `path` and says how big it came, or why
/// it did not: [`crate::cable::write_checkpoint`], which is the one format
/// the cosim harness writes too.  Taken where [`FarEnd::quiet`] says it
/// may be, which is what [`chip_to_quiet`] runs on to.
fn write_chip_checkpoint(
    path: &Path,
    cpu: &Chip,
    clk: &Behavioral,
    far: &FarEnd,
    tv_board: TvBoard,
    color_tv: ColorTv,
    ran: u64,
) {
    match crate::cable::write_checkpoint(path, ran, tv_board.name(), color_tv.name(), cpu, clk, far)
    {
        Ok(n) => eprintln!("checkpoint: {} at {ran} microcycles, {n} bytes", path.display()),
        Err(err) => eprintln!("checkpoint: could not write {}: {err}", path.display()),
    }
}

/// Loads a `chip` checkpoint onto a netlist machine built as the flags say
/// and not booted, and says which microcycle it resumed at; or says why
/// not and exits.  The cables are joined after it, so that each board
/// holds what the others drive onto it.
fn resume_chip(
    cpu: &mut Chip,
    clk: &mut Behavioral,
    far: &mut FarEnd,
    tv_board: TvBoard,
    color_tv: ColorTv,
    (path, c): &(PathBuf, Checkpoint),
) -> u64 {
    let refuse = |err: std::io::Error| -> ! { stale_checkpoint(path, &err, None) };
    let mut it = crate::cable::read_checkpoint(c).unwrap_or_else(|e| refuse(e));
    if it.tv_board != tv_board.name() {
        usage(&format!(
            "--resume {}: a {} checkpoint, and --tv-board is {}",
            path.display(),
            it.tv_board,
            tv_board.name()
        ));
    }
    // **The color TV before anything is read.** A second display board is
    // one more board in the file --- the device boards go in one after
    // another and nothing counts them --- so a checkpoint whose color
    // board is not this run's cannot be read at all, and is refused here
    // by the flag's name rather than a hundred kilobytes later as a board
    // that does not fit.
    if it.color_tv != color_tv.name() {
        usage(&format!(
            "--resume {}: a checkpoint whose color tv is {}, and --color-tv here is {}",
            path.display(),
            it.color_tv,
            color_tv.name()
        ));
    }
    let ran = it.ran;
    it.processor(cpu)
        .and_then(|()| {
            *clk = it.clock()?;
            it.far_end(far)
        })
        .unwrap_or_else(|e| refuse(e));
    far.join(cpu, clk.time_ns());
    eprintln!(
        "resumed: {} at {ran} microcycles, {} ns, {} memory boards",
        path.display(),
        clk.time_ns(),
        far.buses.machine.memory_boards()
    );
    ran
}

/// A checkpoint whose real-time clock was counted from another start, or
/// was live where `--rtc` gives a start, or the reverse, is refused by the
/// flag's name: a resume under its own `--rtc` reads the second the run
/// that wrote it would have. The base is the checkpoint's, the machine's
/// clock at its power-on, and a resume's clock carries on from it.
fn refuse_rtc(path: &Path, had: crate::machine::Rtc, asked: crate::machine::Rtc) {
    use crate::machine::Rtc;
    let same = match (had, asked) {
        (Rtc::Host, Rtc::Host) => true,
        (Rtc::Counted { start: a, .. }, Rtc::Counted { start: b, .. }) => a == b,
        _ => false,
    };
    if !same {
        usage(&format!(
            "--resume {}: a checkpoint with {}, and --rtc here is {}",
            path.display(),
            match had {
                Rtc::Host => "the host's clock".to_string(),
                Rtc::Counted { start, .. } => format!("an RTC from {start}"),
            },
            match asked {
                Rtc::Host => "host".to_string(),
                Rtc::Counted { start, .. } => start.to_string(),
            }
        ));
    }
}

/// **A resume onto a machine `--color-tv` disagrees with is refused by the
/// flag's name**, as `--tv-board` is: the second display board is the
/// backplane's, and a checkpoint of a machine with one is not a
/// description of a machine without.
///
/// This is `micro`'s and `rtl`'s, where the board is the model or nothing
/// and the fact is in the machine the body carries. A `chip` checkpoint
/// carries which kind of color board it was taken with in its header,
/// where [`resume_chip`] settles it before anything is read, because a
/// netlist board is one more board in the file.
fn refuse_color_tv(path: &Path, had: bool, asked: bool) {
    if had == asked {
        return;
    }
    usage(&format!(
        "--resume {}: a checkpoint of a machine {} the color tv, and --color-tv was {} given",
        path.display(),
        if had { "with" } else { "without" },
        if asked { "" } else { "not" }
    ));
}

/// Runs on to the first point a netlist machine may be checkpointed at,
/// and says how many microcycles that took, or what the machine was doing
/// instead if it did not come.
///
/// **Not every microcycle boundary is one.** A checkpoint carries no bus
/// cycle in flight --- [`FarEnd::quiet`] is what says so --- and no memory
/// request from the processor, whose answer would be owed to a cycle the
/// checkpoint does not describe.  Between cycles both are true, and a
/// machine reaches such a point within a few microcycles: the longest
/// anything holds them is the bus timeout, about twelve microseconds,
/// which is eighty microcycles.  The bound is well past that, and a
/// machine that never comes quiet is told about rather than checkpointed
/// wrong.
///
/// **A transition on its way down a delay line used to be asked about
/// here too, and is not any more.** The format had no field for one until
/// version 21, so a board could not be saved with one in flight; it has
/// one now, and `tests/checkpoint.rs` holds a machine loaded from a
/// checkpoint taken with taps in flight to being the machine that was
/// never stopped.  That was the half of this that a busy machine could
/// not get past: issue 89, where a run held after two and a half hours
/// could not be banked.
fn chip_to_quiet(m: &mut DebugIn, memrq: netlist::NetId) -> Result<u64, &'static str> {
    let mut why = match chip_busy_with(&m.cpu, &m.far, memrq) {
        None => return Ok(0),
        Some(why) => why,
    };
    let mut ran = 0;
    while ran < 1000 {
        if m.tick() {
            ran += 1;
            match chip_busy_with(&m.cpu, &m.far, memrq) {
                None => return Ok(ran),
                // The last boundary's, so a run that never came quiet can
                // say what the machine was doing at one rather than what
                // it happens to be doing between two.
                Some(w) => why = w,
            }
        }
    }
    Err(why)
}

/// What is holding a netlist machine off a checkpoint at this instant, or
/// `None` if nothing is: what [`chip_to_quiet`] runs on until, and what it
/// says the machine was doing instead when it never came.
///
/// The two are told apart because they mean different things to whoever
/// asked: a bus cycle is the boards', and a machine doing nothing else
/// but bus cycles may never be between them, while a memory request is
/// the microcode's and goes as soon as it is answered.
fn chip_busy_with(cpu: &Chip, far: &FarEnd, memrq: netlist::NetId) -> Option<&'static str> {
    if !far.quiet() {
        Some("a bus cycle in flight")
    } else if cpu.net(memrq) == Level::High {
        Some("a memory request up")
    } else {
        None
    }
}

/// **A checkpoint the other executable wrote is refused naming that
/// executable**: the executable is the machine, and the map in a
/// checkpoint is its machine's. A `chip` checkpoint is the CADR's boards,
/// QUUX having no netlist; an engine's says its machine in its body,
/// [`crate::machine::Machine::checkpointed_geometry`]. Settled before
/// anything is built, and before the engine's own refusal, which would
/// say something less to the point. A body that cannot be read that far
/// is left to the resume, which says why.
fn refuse_other_executable((path, c): &(PathBuf, Checkpoint), exe: &str) {
    let saved = if c.engine == "chip" {
        Some(crate::machine::Geometry::CADR)
    } else {
        crate::machine::Machine::checkpointed_geometry_at(&c.body, c.word_bits).ok()
    };
    if let Some(saved) = saved
        && executable_of(saved) != exe
    {
        let (theirs, p) = (executable_of(saved), path.display());
        usage(&format!("--resume {p} is {theirs}'s, not {exe}'s: {theirs} --resume {p}"));
    }
}

/// **Which revision `quux` runs** (contract G3 revision 14): `geometry`
/// as it is, revision 13, unless `MUIR_QUUX_REVISION` says 14 or 15. Unset
/// or `13` is revision 13, the released machine; any other value is refused
/// at the start, naming the three. `cadr` does not read it. The switch is
/// not a documented flag: a bitstream, which muir-fpga builds for one
/// revision, could not carry it.
fn quux_revision(geometry: crate::machine::Geometry) -> crate::machine::Geometry {
    use crate::machine::Geometry;
    if geometry != Geometry::QUUX {
        return geometry;
    }
    match std::env::var_os("MUIR_QUUX_REVISION") {
        None => geometry,
        Some(v) if v == "13" => geometry,
        Some(v) if v == "14" => Geometry::QUUX_14,
        Some(v) if v == "15" => Geometry::QUUX_15,
        Some(v) => {
            eprintln!(
                "{}: MUIR_QUUX_REVISION={:?}: revision 13, 14 or 15",
                executable(),
                v.to_string_lossy()
            );
            std::process::exit(2);
        }
    }
}

/// **A checkpoint of the other revision is refused**, naming the revision
/// that wrote it and how to resume it: a revision-14 checkpoint's
/// addresses, TLB and location counter are revision 14's, and the reverse
/// (A14.13). Settled, like [`refuse_other_executable`], before anything
/// is built.
fn refuse_other_revision((path, c): &(PathBuf, Checkpoint), geometry: crate::machine::Geometry) {
    if c.engine == "chip" {
        return;
    }
    let Ok(saved) = crate::machine::Machine::checkpointed_geometry_at(&c.body, c.word_bits) else {
        return;
    };
    // A retired revision, 11 or 12, is refused by what it is when the
    // checkpoint is read.
    if let (Some(theirs @ (13..=15)), Some(ours)) = (saved.revision(), geometry.revision())
        && theirs != ours
    {
        let p = path.display();
        usage(&format!(
            "--resume {p} is revision {theirs}'s, and this is revision {ours}: MUIR_QUUX_REVISION={theirs} quux --resume {p}"
        ));
    }
}

/// A checkpoint of this executable's machine with another geometry --- a
/// QUUX whose PDL buffer is not this one's --- is refused rather than
/// loaded: the map and the PDL in it are that machine's.
/// [`refuse_other_executable`] has refused the other executable's already.
fn refuse_machine(
    (path, _): &(PathBuf, Checkpoint),
    saved: crate::machine::Geometry,
    flag: crate::machine::Geometry,
) {
    if executable_of(saved) != executable_of(flag) {
        let (theirs, exe, p) = (executable_of(saved), executable_of(flag), path.display());
        usage(&format!("--resume {p} is {theirs}'s, not {exe}'s: {theirs} --resume {p}"));
    }
    if saved != flag {
        usage(&format!(
            "--resume {}: a checkpoint of a {} with a {}-bit PDL buffer, and this is {}",
            path.display(),
            executable_of(saved),
            saved.pdl_bits,
            executable_of(flag)
        ));
    }
}

/// A checkpoint `rtl` wrote on one timing model resumed under another is
/// refused by the flag's name: every instant in it is on the time it was
/// run on.
fn refuse_timing_model((path, _): &(PathBuf, Checkpoint), saved: TimingModel, flag: TimingModel) {
    let said = |m: TimingModel| match m {
        TimingModel::Sync { cycle_ticks, .. } => format!("sync of {cycle_ticks} ticks"),
        _ => m.name().to_string(),
    };
    if saved != flag {
        usage(&format!(
            "--resume {}: written under --timing-model {}, and this run is under {}",
            path.display(),
            said(saved),
            said(flag)
        ));
    }
}

/// Loads the checkpoint read from `path` into `e`, built and booted as the
/// flags say, or says why not and exits.
///
/// **The display board is refused after the read, not before it.** A
/// `chip` checkpoint carries the board in its own header, so
/// [`resume_chip`] can refuse one before it builds anything; an engine's
/// carries it in the body, where `Tv::save` writes it, and the machine is
/// built before the file is opened. So the file is read, and a board apart
/// from the flag's ends the run then --- by the flag's name, as the other
/// refusal does.
fn resume_engine<E: Engine>(
    name: &str,
    e: &mut E,
    (tv_board, video_size): (TvBoard, (usize, usize)),
    color_tv: ColorTv,
    (geometry, rtc): (crate::machine::Geometry, crate::machine::Rtc),
    resume: &(PathBuf, Checkpoint),
    go_on: bool,
) {
    let (path, c) = resume;
    if c.engine != name {
        usage(&format!(
            "--resume {}: a {} checkpoint, and this is {name}",
            path.display(),
            c.engine
        ));
    }
    let mut r = c.reader();
    // The machine first: a board refused on another machine's checkpoint
    // is only the machine's default board, and a disk the same (QUUX's is
    // block-disk). The geometry is read before either, so a load that
    // fails on one has it already.
    let loaded = e.load(&mut r).and_then(|()| r.done());
    refuse_machine(resume, e.machine().geometry, geometry);
    loaded.unwrap_or_else(|err| stale_checkpoint(path, &err, None));
    if e.machine().tv.board() != tv_board {
        usage(&format!(
            "--resume {}: a {} checkpoint, and --tv-board is {}",
            path.display(),
            e.machine().tv.board().name(),
            tv_board.name()
        ));
    }
    let (w, h, _) = e.machine().tv.screen();
    if tv_board == TvBoard::Video && (w, h) != video_size {
        usage(&format!(
            "--resume {}: a video controller of {w}x{h}, and --video-size is {}x{}",
            path.display(),
            video_size.0,
            video_size.1
        ));
    }
    refuse_color_tv(path, e.machine().color_tv.is_some(), color_tv.fitted());
    refuse_rtc(path, e.machine().rtc, rtc);
    let m = e.machine();
    let memory = if m.geometry.revision().is_some() {
        format!("{} of main memory", megawords(m.main.len()))
    } else {
        format!("{} memory boards", m.memory_boards())
    };
    eprintln!("resumed: {} at {} microcycles, {} ns, {memory}", path.display(), m.cycles, m.ns);
    // `--continue`: the prompt's `continue`, before the first microcycle.
    if go_on {
        if halted(e) { take_off(e) } else { say_running_already() }
    }
}

/// Writes a recording of the main screen to `path` and says how big it
/// came.
fn write_capture(path: &Path, rec: &Recorder) {
    wrote_capture("capture", "the display", path, rec.frames(), rec.gif());
}

/// The same for a recording of the color screen, which is a file of its
/// own and says which screen it is.
fn write_color_capture(path: &Path, rec: &ColorRecorder) {
    wrote_capture("color capture", "the color screen", path, rec.frames(), rec.gif());
}

fn wrote_capture(what: &str, screen: &str, path: &Path, frames: usize, gif: Vec<u8>) {
    match std::fs::write(path, gif) {
        Ok(()) => eprintln!(
            "{what}: {frames} frames of {screen} at {}, {} bytes",
            path.display(),
            std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
        ),
        Err(e) => eprintln!("{what}: could not write {}: {e}", path.display()),
    }
}

/// A netlist machine as `--chip` runs it: the processor with the boot PROM
/// loaded and the button pressed, its clock, the far end with the boards
/// asked for, and the interface netlist the far end's board was built from;
/// run to the first microcycle whose PC is not zero.
struct ChipMachine {
    cpu: Chip,
    clk: Behavioral,
    far: FarEnd,
    bus: netlist::Netlist,
    pc_nets: Vec<netlist::NetId>,
    /// `IR<47:0>`, the instruction register.  It holds the instruction the
    /// machine last executed, and on a halt that is the one that halted it
    /// --- `HALT-CONS` is `IR<11:10>` --- which is why the prompt prints it
    /// beside the PC here and does not on the other engines, where it is
    /// not what a halt leaves behind.
    ir_nets: Vec<netlist::NetId>,
    /// The board's five readable memories, in [`Memory`]'s order, each a
    /// map from the RAM chips' cells to a word.  Built once: `Ram::new`
    /// walks every instance, and the prompt would otherwise do it per
    /// command.
    rams: Vec<crate::chip::Ram>,
    promdisable: netlist::NetId,
    /// `-BOOT1`, the keyboard's boot line, for the boot sequence under
    /// `--io-board model`: with no netlist board to drive it, the model
    /// board's decode presses it here as the button presses `-BOOT2`.
    boot1: netlist::NetId,
    /// `SRUN`, `-ERRHALT` and `-STATHALT`: three of the six inputs of the
    /// 9S42 at OLORD1 1A15 that makes `MACHRUN`, which the drawing has as
    /// `MACHRUN = (SSTEP AND -SSDONE) OR (SRUN AND -ERRHALT AND -WAIT AND
    /// -STATHALT)`.  What [`machrun_low`] reads off `FLAG-1` on the other
    /// two engines is read off these nets here, and `-WAIT` is left out of
    /// it for the same reason: a machine waiting on the bus comes out of
    /// it by itself.
    srun: netlist::NetId,
    errhalt: netlist::NetId,
    stathalt: netlist::NetId,
    /// `RUN`, the 74S74 at OLORD1 1A14 that `-LDCLK` clocks `SPY0` into
    /// and `-BOOT` presets: clear, the machine is halted, and the prompt's
    /// `continue` sets it ([`take_off_chip`]).
    run_net: netlist::NetId,
    /// `-BOOT2`, the light panel's boot button --- the MBCPIN drawing marks
    /// connector 1AJ2 "TO LIGHT PANEL" --- for the prompt's `boot` to press
    /// again. `-BOOT1` is the other input, the Unibus boot line the keyboard
    /// comes in on through the bus interface's `-LM BOOT`, and nothing
    /// presses it here: `docs/keyboard-boot.md`.
    boot: netlist::NetId,
}

#[allow(clippy::too_many_arguments)]
fn chip_machine(
    nets: &Netlists,
    image: &[u64],
    packs: &[Pack],
    boards: Boards,
    memory_boards: usize,
    chaos: crate::chaos::Config,
    tv_board: TvBoard,
    color_tv: ColorTv,
    auto_boot: bool,
) -> ChipMachine {
    let n = netlist::parse(nets.cadr).unwrap();
    let mut c = Chip::new(&n);
    c.power_on();
    c.load_prom(&n, image);
    c.settle();
    let mut clk = Behavioral::new();
    let mut machine = Machine::with_memory_boards(memory_boards);
    attach(&mut machine, packs);
    // The board `--tv model` answers as: the far end's machine is the
    // model display on a netlist machine, and it is the board the run
    // named whether or not there is a netlist of it on the backplane.
    machine.tv.set_board(tv_board);
    // The color TV's model, which is fitted whichever board is on the
    // backplane: with `--color-tv model` it answers `17200000` and
    // `17377750` as `--tv model` answers the main screen's addresses, and
    // with `--color-tv netlist` the netlist board answers them and every
    // write is mirrored in here, so that the color picture is read off
    // the same place either way.
    if color_tv.fitted() {
        machine.fit_color_tv();
    }
    machine.chaos = chaos;
    let bus_n = netlist::parse(nets.busint).unwrap();
    let mem_n = netlist::parse(nets.cadrm).unwrap();
    let mut far = FarEnd::new(&n, &bus_n, &mem_n, boards, 0, machine);
    far.join(&mut c, clk.time_ns());

    // The button, then the few start-up microcycles before the PC moves.
    // `--no-auto-boot` leaves it unpressed, as a CADR is when the power
    // comes on, for the prompt's `boot` to press.
    let boot = n.by_name_id("-BOOT2").unwrap();
    if auto_boot {
        press_boot(&mut c, &mut clk, boot);
    }
    let pc_nets = c.bus_nets(&n, "PC", 14);
    let ir_nets = c.bus_nets(&n, "IR", 48);
    // In `Memory`'s order, so the prompt indexes by the command's own enum.
    let rams: Vec<crate::chip::Ram> = ["A", "M", "DISPATCH", "PDL", "SPC"]
        .iter()
        .map(|name| {
            let m = crate::chip::MEMS.iter().find(|m| m.name == *name).expect("a memory by name");
            crate::chip::Ram::new(&c, &n, m)
        })
        .collect();
    // The mode register's bit, as `Machine::mode` has it on the other
    // engines.
    let promdisable = n.by_name_id("PROMDISABLE").unwrap();
    let boot1 = n.by_name_id("-BOOT1").unwrap();
    let srun = n.by_name_id("SRUN").unwrap();
    let run_net = n.by_name_id("RUN").unwrap();
    let errhalt = n.by_name_id("-ERRHALT").unwrap();
    let stathalt = n.by_name_id("-STATHALT").unwrap();
    let mut skipped = 0;
    while auto_boot && c.read(&pc_nets) == 0 && skipped < 40 {
        c.microcycle(&mut clk);
        skipped += 1;
    }
    ChipMachine {
        cpu: c,
        clk,
        far,
        bus: bus_n,
        pc_nets,
        ir_nets,
        rams,
        promdisable,
        boot1,
        srun,
        errhalt,
        stathalt,
        run_net,
        boot,
    }
}

/// **The boot button on a netlist machine**: `-BOOT2` held down, the
/// board settled with it down, and twenty master clock cycles before it
/// comes back up.  It is all that starts a CADR, so the prompt's `boot`
/// presses this and nothing else, as it does on the other two engines.
/// `-BOOT1`, the Unibus boot line, and `PROG.BOOT` from the debug cable
/// reach the same 74S02 at OLORD2 1A07 that makes `-BOOT`; the board
/// cannot tell which was pressed.  Under `--io-board model` the keyboard's
/// boot sequence presses `-BOOT1` through this same hold, there being no
/// netlist board to pulse it; with the netlist board the far end carries
/// its `-BOOT*` to `-BOOT1` itself.
fn press_boot(c: &mut Chip, clk: &mut Behavioral, boot: netlist::NetId) {
    c.set_net(boot, Level::Low);
    c.settle();
    for _ in 0..20 {
        c.tick(clk);
    }
    c.set_net(boot, Level::High);
}

/// One turn of the terminal for a netlist machine: the screen out and the
/// keys and the pointer in when it is time to poll, and both delivered as
/// the I/O board takes them.  Whether the model I/O board, under
/// `--io-board model`, decoded the boot sequence's word this turn, for the
/// caller to press `-BOOT1`; the netlist board drives its own `-BOOT*`,
/// which the far end carries, and this says nothing for it.
fn attend_chip(
    far: &mut FarEnd,
    terminal: Option<&mut Terminal>,
    color: Option<&mut Terminal>,
    glass: Option<&mut Glass>,
    poll: bool,
    keyboard: &mut Keyboard,
    mouse: &mut Mouse,
) -> bool {
    // The color screen, when the board is fitted: the picture out and
    // nothing in.
    if poll
        && let Some(term) = color
        && let Some(tv) = far.buses.machine.color_tv.as_ref()
    {
        term.poll(Frame::of(tv));
    }
    if poll && let Some(term) = terminal {
        term.poll(Frame::of(&far.buses.machine.tv));
        for (keysym, down) in term.take_keys() {
            keyboard.key(keysym, down);
        }
        for (buttons, x, y) in term.take_pointers() {
            mouse.pointer(buttons, x, y);
        }
        // The behavioral board under `--io-board model`. The netlist
        // board's speaker is `AUDIO+`/`AUDIO-` out of the 75118 at IOBXCV
        // 0F30 and nothing is plugged into that pair, so a beep on it is
        // heard by nobody.
        if far.buses.machine.ioboard.take_beep() {
            term.ring();
        }
    }
    // The glass TTYs, as in [`attend`]: the screen as text out, and what
    // was typed in to the one keyboard.
    if poll && let Some(glass) = glass {
        for (keysym, down) in glass.poll(Frame::of(&far.buses.machine.tv)) {
            keyboard.key(keysym, down);
        }
    }
    match far.unibus.as_mut().and_then(|u| u.mouse()) {
        // The netlist I/O board takes its motion as quadrature stepped
        // down the lines, and its buttons as levels.
        Some(cable) => {
            let (dx, dy) = mouse.take_motion();
            if dx != 0 || dy != 0 || cable.buttons() != mouse.buttons() {
                cable.send(dx, dy, mouse.buttons());
            }
        }
        None => {
            let board = &mut far.buses.machine.ioboard;
            if mouse.pending(board.mouse_buttons_held()) {
                mouse.deliver(board);
            }
        }
    }
    if keyboard.pending() > 0 {
        match far.unibus.as_mut().and_then(|u| u.keyboard()) {
            // The netlist I/O board takes its keys down the cable, one
            // word at a time as the cable frees.
            Some(cable) => {
                if let Some(word) = keyboard.peek()
                    && cable.send(word)
                {
                    keyboard.take();
                }
            }
            // The behavioral board under `--io-board model` takes the
            // word, and `Buses` runs its interrupt cycle against the
            // netlist bus interface --- request, grant, `SACK`, `INTR`
            // with vector 260 --- as the netlist board would run its own.
            None => {
                keyboard.deliver(&mut far.buses.machine.ioboard);
            }
        }
    }
    far.unibus.is_none() && far.buses.machine.ioboard.take_boot()
}

/// One turn of the serial endpoint for a netlist machine.
///
/// Two far ends, and which one is on J9 is `--io-board`'s: the netlist
/// board's, a bit at a time on the EIA wires, or the behavioral port's
/// cable under `--io-board model`. The model board is advanced by its own
/// register accesses here and by nothing else, so its time is the
/// machine's last access rather than the clock's; MIT's driver polls the
/// status register, so a character leaves within a poll of being written.
fn attend_serial_chip(far: &mut FarEnd, end: &mut Endpoint) {
    match far.unibus.as_mut().and_then(|u| u.serial()) {
        Some(cable) => end.poll_on_cable(cable),
        None => {
            let now = far.buses.machine.ns;
            end.poll_cable(&mut far.buses.machine.ioboard.serial.cable, now);
        }
    }
}

/// Runs a netlist machine: what the command line has to say about one,
/// the memory board count among them, and [`Run`] for the rest.  `cable`
/// is the listener for DBGIN's connector, if the run has one: the board's
/// own connector answers a debugger that connects, an event at a time
/// while one is on ([`DebugIn`] in a [`Connector`]), and the machine is
/// `--chip` alone before and after, a transition at a time.
#[allow(clippy::too_many_arguments)]
fn time_chip(
    nets: &Netlists,
    cable: Option<std::net::TcpListener>,
    image: &[u64],
    packs: &[Pack],
    boards: Boards,
    memory_boards: usize,
    chaos: crate::chaos::Config,
    terminal: Option<&mut Terminal>,
    mut glass: Option<&mut Glass>,
    serial: Option<&mut Endpoint>,
    run: Run,
    (resume, go_on): (Option<(PathBuf, Checkpoint)>, bool),
    tv_board: TvBoard,
    color_tv: ColorTv,
    watch: Option<WatchSpec>,
) {
    let Run {
        stop,
        capture,
        color_capture,
        checkpoint,
        setup,
        hold,
        pace: paced,
        clocks,
        mut color,
    } = run;
    let ChipMachine {
        mut cpu,
        mut clk,
        mut far,
        bus,
        pc_nets,
        ir_nets,
        rams,
        promdisable,
        boot1,
        srun,
        errhalt,
        stathalt,
        run_net,
        boot,
        // A resume brings the board up but does not press the button: what
        // the button and the power-on set is what the checkpoint replaces.
    } = chip_machine(
        nets,
        image,
        packs,
        boards,
        memory_boards,
        chaos,
        tv_board,
        color_tv,
        !hold && resume.is_none(),
    );
    // One microcycle is however many clock transitions it takes for the phase
    // to wrap, not a fixed number of them.
    let t = Instant::now();
    // Where the checkpoint left the machine, which the microcycles this
    // run makes are counted from; `ran` is this run's own, as it is on the
    // other two engines, so that `--stop-after` is a window on the run and
    // not on the machine's whole life.
    let resumed_at = match &resume {
        Some(p) => resume_chip(&mut cpu, &mut clk, &mut far, tv_board, color_tv, p),
        None => 0,
    };
    // `--continue`: the prompt's `continue`, before the first microcycle.
    if go_on {
        if cpu.net(run_net) == Level::High {
            say_running_already();
        } else {
            let prom = cpu.net(promdisable) != Level::High;
            take_off_chip(&mut cpu, run_net, &pc_nets, prom);
        }
    }
    let mut serial = serial;
    // The far end of the null-modem cable goes on the netlist board's J9
    // only when `--serial` opened an endpoint: without one the board pays
    // nothing for a port nobody is at. After the resume, which brings the
    // board back as the checkpoint left it and carries no far end.
    if serial.is_some()
        && let Some(u) = far.unibus.as_mut()
    {
        u.plug_serial(clk.time_ns());
    }
    let mut ran = 0;
    // `MEMRQ`, for the quiet point a checkpoint is taken at.
    let memrq = netlist::parse(nets.cadr).unwrap().by_name_id("MEMRQ").unwrap();
    // The machine behind DBGIN's connector: the board's own, with the
    // processor, its clock and the far end, whether or not a debugger ever
    // comes.  With nobody on the cable the loop below ticks it a
    // transition at a time, [`DebugIn::tick`], as `--chip` always has.
    let mut end = Connector::new(DebugIn::new(&bus, cpu, clk, far), cable)
        .unwrap_or_else(|e| fail(&format!("the debug cable's listener: {e}")));
    let prom_enabled = |c: &Chip| c.net(promdisable) != Level::High;
    // As [`machrun_low`] is on the other two engines, off the nets rather
    // than off `FLAG-1`: `Chip` is not an `Engine` and has no spy registers
    // to read, but it has the nets those registers are buffered from.
    let stopped_itself = |c: &Chip| {
        if c.net(srun) != Level::High {
            return None;
        }
        if c.net(errhalt) == Level::Low {
            return Some("ERR is up under ERRSTOP, which is what HALT-CONS does: (si:%halt)");
        }
        if c.net(stathalt) == Level::Low {
            return Some("the statistics counter ran out under STATHENB");
        }
        None
    };
    let mut terminal = terminal;
    let (mut keyboard, mut mouse) = (a_keyboard(), Mouse::new());
    let mut last_poll = Instant::now();
    let mut capture = capture.map(|(path, time)| (path, Recorder::new(time)));
    let mut color_capture = color_capture.map(|(path, time)| (path, ColorRecorder::new(time)));
    let mut last_check = Instant::now();
    // `--pace`, taken here and never acted on: `chip` is some thousands of
    // times slower than the hardware, so the run is never ahead of the
    // machine's clock for [`Pace`] to hold it back.  It is wired up all
    // the same, so that the flag means one thing on every engine and it is
    // the rule that decides, not the engine.
    let mut pace = paced.then(|| Pace::new(Instant::now(), end.machine().clk.time_ns()));
    let prompt = Prompt::open();
    // The same hold the other two engines have: nothing is ticked while it
    // is on, so the netlist stands where it stopped and can be looked at.
    // `chip` is slow enough that this matters --- a run that has spent an
    // hour getting somewhere should not have to be started again to be
    // asked where it is.
    let mut held = hold;
    let mut stepping: Option<u64> = None;
    let mut quit = false;
    catch_interrupts();
    // The processor's, the interface's and a memory board's netlists, for
    // the prompt's `net` and `watch`: parsed on the first one asked for,
    // since most runs ask for none.
    let mut net_netlists: Option<(netlist::Netlist, netlist::Netlist, netlist::Netlist)> = None;
    let parse_netlists = || {
        (
            netlist::parse(nets.cadr).unwrap(),
            netlist::parse(nets.busint).unwrap(),
            netlist::parse(nets.cadrm).unwrap(),
        )
    };
    // `--watch`, resolved now that the boards are there: a name no board
    // carries stops the run before it starts, as the prompt's `net` would
    // answer it, rather than recording nothing for hours.
    let mut watch = watch.map(|(from, to, nets)| {
        let m = end.machine();
        let named =
            boards_named(&m.cpu, &m.far, &boards, net_netlists.get_or_insert_with(parse_netlists));
        Watch::new(from, to, &nets, &named).unwrap_or_else(|what| fail(&format!("--watch: {what}")))
    });
    let mut interrupts_seen = 0;
    let mut asks_seen = 0;
    while !quit && ran < stop.after && {
        let c = &end.machine().cpu;
        // `IWRITED` is not read here, so a PC holding a control-store
        // write's address is taken for the program's, as `rtl`'s is not
        // (`Engine::pc_is_a_write`). The board's PC nets do pass the
        // write's address: `--chip --stop-at-prom 400` on MIT's PROM stops
        // 412,629 microcycles in, while the PROM clears the control store,
        // where `rtl` and `micro` run on (measured).
        !stop.reached((c.read(&pc_nets) as u16, prom_enabled(c), false))
    } {
        // Microcycles this turn: one at most on its own, where a turn is a
        // transition of the boards; as many as the debugger's promise
        // allowed with one on the cable, where a turn is a quantum of the
        // boards or a wait for the debugger's next message.
        let mut delta = 0;
        if held {
            std::thread::sleep(TERMINAL_INTERVAL / 4);
        } else if end.debugger().is_some() {
            let before = end.machine().microcycles;
            match end.step_cabled() {
                Ok(Turn::Unplugged(why)) => say_unplugged(end.addr(), &why),
                Ok(Turn::Stepped | Turn::Waited) => {}
                Err(h) => {
                    eprintln!("{}: the debug cable: the machine halted: {h:?}", executable());
                    break;
                }
            }
            delta = end.machine().microcycles - before;
        } else {
            let m = end.machine_mut();
            if m.tick() {
                delta = 1;
            }
            // The record, at every step: [`Watch`] says why a step and
            // not a microcycle. A run with no watch pays one test here,
            // and one with a range behind it drops the watch and pays the
            // same.  A run with a watch has no connector, so this is the
            // only path a watch is on.
            if let Some(w) = watch.as_mut()
                && !w.sample(ran + delta, m.clk.time_ns(), &m.cpu, &m.far)
            {
                watch = None;
            }
        }
        let wrapped = delta > 0;
        if wrapped {
            ran += delta;
            if let Some(left) = stepping.as_mut() {
                *left = left.saturating_sub(delta);
                if *left == 0 {
                    stepping = None;
                    held = true;
                    let m = end.machine();
                    say_pc_chip(&m.cpu, &pc_nets, &ir_nets, ran, prom_enabled(&m.cpu));
                }
            }
        }
        // The capture keeps its own cadence, in microcycles.
        if wrapped && ran % TERMINAL_CHECK == 0 {
            let m = end.machine();
            if let Some((_, rec)) = capture.as_mut() {
                rec.sample(&m.far.buses.machine.tv, m.clk.time_ns(), wall_clock());
            }
            if let Some((_, rec)) = color_capture.as_mut()
                && let Some(tv) = m.far.buses.machine.color_tv.as_ref()
            {
                rec.sample(tv, m.clk.time_ns(), wall_clock());
            }
        }
        // Everything else goes by the wall clock, not by a microcycle
        // count. `chip` runs about 1,800 microcycles a second, so
        // `TERMINAL_CHECK` of them is two and a half seconds --- too long
        // to wait on a typed line, and a run shorter than that never
        // reaches a check at all. `Instant::now` once a microcycle costs
        // nothing at this rate, which is the only reason the other engines
        // count microcycles instead.
        let check = held || (wrapped && last_check.elapsed() >= TERMINAL_INTERVAL);
        if !check {
            continue;
        }
        last_check = Instant::now();
        // The connector's listener, held or not: a debugger may come to a
        // machine standing at the prompt.  While one is on the cable the
        // machine's stops are the debugger's to notice, which is what CC
        // is for, and `checkpoint` and the capture commands are refused as
        // they are at the `rtl` end of a cable.
        say_plugged(end.attend());
        let on_cable = end.debugger().is_some();
        let m = end.machine_mut();
        // **`kill -USR1` asks a run where it is**, and it answers here,
        // between two microcycles, and goes on. It does not hold the
        // machine: a reader that stopped the run would be no use for the
        // long timing runs this exists for, and this costs one atomic
        // load against the `Instant::now` above it, which the comment
        // there already calls free at this rate. Issue 86.
        if asked_where(&mut asks_seen) {
            say_pc_chip(&m.cpu, &pc_nets, &ir_nets, ran, prom_enabled(&m.cpu));
        }
        let poll = last_poll.elapsed() >= TERMINAL_INTERVAL;
        if poll || !held {
            if attend_chip(
                &mut m.far,
                terminal.as_deref_mut(),
                color.as_deref_mut(),
                glass.as_deref_mut(),
                poll,
                &mut keyboard,
                &mut mouse,
            ) {
                press_boot(&mut m.cpu, &mut m.clk, boot1);
            }
            if poll {
                last_poll = Instant::now();
            }
        }
        // The serial port's endpoint, when `--serial` opened one: a check
        // is already the terminal's cadence here, which is as often as a
        // serial line needs.
        if let Some(port) = serial.as_deref_mut() {
            attend_serial_chip(&mut m.far, port);
        }
        // The machine stopping itself, held on once rather than spun on,
        // exactly as `time_engine` does it off `FLAG-1`.
        if !held
            && !on_cable
            && let Some(why) = stopped_itself(&m.cpu)
        {
            held = true;
            stepping = None;
            say_machrun_low(why);
            say_pc_chip(&m.cpu, &pc_nets, &ir_nets, ran, prom_enabled(&m.cpu));
        }
        // ^C: the first holds the machine at the prompt, one more while
        // held quits; with no prompt to go on from, one quits.
        let seen = INTERRUPTS.load(Ordering::SeqCst);
        while interrupts_seen < seen {
            interrupts_seen += 1;
            let at_prompt = prompt.as_ref().is_some_and(|p| !p.ended());
            if held || !at_prompt {
                quit = true;
            } else {
                held = true;
                stepping = None;
                if let Some(prompt) = prompt.as_ref() {
                    prompt.past_interrupt();
                }
                println!("held at ^C; continue runs on, ^C again quits");
                say_pc_chip(&m.cpu, &pc_nets, &ir_nets, ran, prom_enabled(&m.cpu));
            }
        }
        if stepping.is_none()
            && let Some(prompt) = prompt.as_ref()
        {
            let ending = prompt.ended();
            while let Some(line) = prompt.line() {
                match crate::prompt::parse(&line) {
                    Ok(None) => {}
                    Ok(Some(Command::Boot)) => {
                        press_boot(&mut m.cpu, &mut m.clk, boot);
                        say_pc_chip(&m.cpu, &pc_nets, &ir_nets, ran, prom_enabled(&m.cpu));
                        held = false;
                    }
                    Ok(Some(Command::Hold)) => {
                        held = true;
                        say_pc_chip(&m.cpu, &pc_nets, &ir_nets, ran, prom_enabled(&m.cpu));
                    }
                    Ok(Some(Command::Continue)) => {
                        if m.cpu.net(run_net) != Level::High {
                            let prom = prom_enabled(&m.cpu);
                            take_off_chip(&mut m.cpu, run_net, &pc_nets, prom);
                            held = false;
                        } else if let Some(why) = stopped_itself(&m.cpu) {
                            say_machrun_low(why);
                        } else if held {
                            held = false;
                        } else {
                            say_running_already();
                        }
                    }
                    Ok(Some(Command::Step(n))) => match stopped_itself(&m.cpu) {
                        Some(why) => say_machrun_low(why),
                        None => {
                            held = false;
                            stepping = Some(n);
                            break;
                        }
                    },
                    Ok(Some(Command::Pc)) => {
                        say_pc_chip(&m.cpu, &pc_nets, &ir_nets, ran, prom_enabled(&m.cpu))
                    }
                    Ok(Some(Command::Info)) => print!("{setup}"),
                    Ok(Some(Command::Keys)) => print!("{}", keys_in_force()),
                    Ok(Some(Command::Screenshot(path))) => {
                        let path = path.unwrap_or_else(|| timestamped("png"));
                        write_screenshot(&path, &m.far.buses.machine.tv);
                    }
                    Ok(Some(Command::StartCapture(_))) if on_cable => {
                        println!("capture: {NO_CAPTURE_OVER_THE_CABLE}")
                    }
                    Ok(Some(Command::StartCapture(path))) => match capture.as_ref() {
                        Some((going, _)) => println!(
                            "capture: one is going already, to {}; endcapture closes it",
                            going.display()
                        ),
                        None => {
                            let path = path.unwrap_or_else(|| timestamped("gif"));
                            println!(
                                "capture: recording the display to {}{}; endcapture writes it, and so does the stop",
                                path.display(),
                                if clocks { "" } else { ", no clocks" }
                            );
                            capture = Some((path, Recorder::new(clocks)));
                        }
                    },
                    Ok(Some(Command::EndCapture)) if on_cable => {
                        println!("capture: {NO_CAPTURE_OVER_THE_CABLE}")
                    }
                    Ok(Some(Command::EndCapture)) => match capture.take() {
                        Some((path, mut rec)) => {
                            rec.sample(&m.far.buses.machine.tv, m.clk.time_ns(), wall_clock());
                            write_capture(&path, &rec);
                        }
                        None => println!("capture: none is going; startcapture begins one"),
                    },
                    // The scratchpads live in the RAM chips' own cells here
                    // rather than in arrays, so a dump walks those cells:
                    // `crate::chip::Ram`, which is also what
                    // `chip_and_rtl_hold_the_same_memories` holds to `rtl`,
                    // so this prints the same words that comparison checks.
                    Ok(Some(Command::Dump { memory, from, words })) => {
                        match say_chip_memory(&m.cpu, &rams, memory, from, words) {
                            Ok(dump) => print!("{dump}"),
                            Err(what) => println!("prompt: {what}"),
                        }
                    }
                    // Main memory is the boards on the backplane here, so a
                    // word is a bit off each of the 32 DRAMs of one bank of
                    // one board: `FarEnd::main_word`, which reads the cells
                    // and runs no bus cycle, so this is answered while the
                    // machine runs as `net` is.
                    Ok(Some(Command::Mem { from, words })) => {
                        let read = |a: usize| m.far.main_word(a as u32);
                        match crate::prompt::main_dump(from, words, m.far.main_words(), read) {
                            Ok(dump) => print!("{dump}"),
                            Err(what) => println!("prompt: {what}"),
                        }
                    }
                    // A register is a net bundle rather than a memory and
                    // wants naming one at a time; the memories and `IR`,
                    // which is what a halt leaves behind, are here.
                    Ok(Some(Command::Registers)) => {
                        println!("prompt: not on chip yet --- a register here is the nets of the");
                        println!(
                            "        parts driving it, not a word to read off; `pc` gives the"
                        );
                        println!(
                            "        PC and IR, and amem, mmem, dmem, pdl and spc the memories"
                        );
                    }
                    Ok(Some(Command::Checkpoint(_))) if on_cable => {
                        println!("checkpoint: {NO_CHECKPOINT_OVER_THE_CABLE}")
                    }
                    Ok(Some(Command::Checkpoint(path))) => {
                        let path = path.unwrap_or_else(|| timestamped("chk"));
                        match chip_to_quiet(m, memrq) {
                            Ok(on) => {
                                ran += on;
                                write_chip_checkpoint(
                                    &path,
                                    &m.cpu,
                                    &m.clk,
                                    &m.far,
                                    tv_board,
                                    color_tv,
                                    resumed_at + ran,
                                );
                            }
                            Err(why) => println!(
                                "checkpoint: the machine has {why} and has not come quiet in \
                                 a thousand microcycles; nothing written"
                            ),
                        }
                    }
                    Ok(Some(Command::Net(want))) => {
                        // The netlists are parsed the first time one is
                        // asked for and kept: a run that never asks pays
                        // nothing, and one that asks twice parses once.
                        let netlists = net_netlists.get_or_insert_with(parse_netlists);
                        let on = boards_named(&m.cpu, &m.far, &boards, netlists);
                        print!("{}", say_net(&on, &want));
                    }
                    // The next `cycles` microcycles from here: the one in
                    // progress, or the one about to start if held at a
                    // boundary, and the rest. A watch already going, or
                    // one `--watch` set for later, is replaced.
                    Ok(Some(Command::Watch { cycles, nets })) => {
                        let netlists = net_netlists.get_or_insert_with(parse_netlists);
                        let on = boards_named(&m.cpu, &m.far, &boards, netlists);
                        let to = ran + cycles - 1;
                        match Watch::new(ran, Some(to), &nets, &on) {
                            // Not `watch: ...`, which is the record's own
                            // prefix and what a reader greps for.
                            Ok(w) => {
                                println!(
                                    "recording {} over microcycles {ran} to {to}; the record is on \
                                     stderr, each line prefixed watch:",
                                    w.labels()
                                );
                                watch = Some(w);
                            }
                            Err(what) => println!("prompt: {what}"),
                        }
                    }
                    Ok(Some(Command::Quit)) => {
                        quit = true;
                        break;
                    }
                    Ok(Some(Command::Help)) => print!("{}", crate::prompt::HELP),
                    Err(what) => println!("prompt: {what}"),
                }
            }
            if held && ending && !quit {
                println!("held, and stdin has ended: there is nothing to run the machine on");
                quit = true;
            }
            if held && !quit && stepping.is_none() {
                prompt.show();
            }
        }
        // `--pace`, as on the other two engines: wait when the run is
        // ahead of the machine's clock, which on `chip` it never is.
        if let Some(pace) = pace.as_mut() {
            let ns = m.clk.time_ns();
            if held || on_cable {
                pace.anchor(Instant::now(), ns);
            } else if let Some(wait) = pace.owed(Instant::now(), ns) {
                std::thread::sleep(wait);
            }
        }
    }
    if let Some(prompt) = prompt.as_ref() {
        prompt.done();
    }
    report("chip", ran, t.elapsed().as_secs_f64(), crate::ioboard::CYCLE_NS);
    {
        let c = &end.machine().cpu;
        if quit {
            let prom = if prom_enabled(c) { " in the PROM" } else { "" };
            println!("       quit at PC {:o}{prom} after {ran}", c.read(&pc_nets) as u16);
        } else {
            stop.conclude(ran, (c.read(&pc_nets) as u16, prom_enabled(c), false), None);
        }
    }
    // A debugger on the cable is told the run is over and waited for, as
    // at the `rtl` end; one that was and went is only counted.
    if end.connections() > 0 {
        println!("       {} debug cycles on the cable", end.debug_cycles());
    }
    if let Err(e) = end.finish() {
        eprintln!("{}: the debug cable at the end: {e}", executable());
    }
    let m = end.machine_mut();
    if let Some((path, rec)) = capture.as_mut() {
        rec.sample(&m.far.buses.machine.tv, m.clk.time_ns(), wall_clock());
        write_capture(path, rec);
    }
    if let Some((path, rec)) = color_capture.as_mut() {
        if let Some(tv) = m.far.buses.machine.color_tv.as_ref() {
            rec.sample(tv, m.clk.time_ns(), wall_clock());
        }
        write_color_capture(path, rec);
    }
    // The checkpoint last, and at the first quiet microcycle from here:
    // the stop falls where it falls, and a machine part way through a bus
    // cycle is not a machine a checkpoint describes.  Those microcycles
    // are the run's like any other, so they are counted and said.
    if let Some(path) = &checkpoint {
        match chip_to_quiet(m, memrq) {
            Ok(on) => {
                if on > 0 {
                    eprintln!("checkpoint: {on} microcycles on to a quiet one");
                }
                write_chip_checkpoint(
                    path,
                    &m.cpu,
                    &m.clk,
                    &m.far,
                    tv_board,
                    color_tv,
                    resumed_at + ran + on,
                );
            }
            Err(why) => eprintln!(
                "checkpoint: {} not written: the machine has {why} and has not come quiet in \
                 a thousand microcycles",
                path.display()
            ),
        }
    }
    {
        let machine = &m.far.buses.machine;
        let mut screens: Vec<(&mut Terminal, &crate::tv::Tv)> = Vec::new();
        if let Some(term) = terminal {
            screens.push((term, &machine.tv));
        }
        if let (Some(term), Some(tv)) = (color, machine.color_tv.as_ref()) {
            screens.push((term, tv));
        }
        serve_last_screens(&mut screens, &mut interrupts_seen);
    }
}

/// Runs `cadr` or `quux`, whichever executable is `geometry`'s machine ---
/// [`crate::machine::Geometry::CADR`] or [`crate::machine::Geometry::QUUX`]
/// --- on the command line it was given. A flag of the other machine's is
/// refused by name, saying which executable takes it, and so is a
/// checkpoint the other one wrote. `netlists` are the boards `--chip`
/// builds, which only `cadr` takes and so only `cadr` passes.
///
/// `quux` is revision 13 unless `MUIR_QUUX_REVISION` says 14
/// ([`quux_revision`]), read here once, so that everything after it ---
/// the start's lines, a resume's refusal, the machine --- is the
/// revision's.
pub fn run(geometry: crate::machine::Geometry, netlists: Option<&Netlists>) {
    let exe = executable_of(geometry);
    let _ = EXECUTABLE.set(exe);
    let geometry = quux_revision(geometry);
    let mut which: Option<Which> = None;
    let mut packs: Vec<Pack> = Vec::new();
    // The glass TTYs asked for, in the order the flags came. Bound after
    // the flags are read, as the terminal is.
    let mut glass_at: Vec<GlassAt> = Vec::new();
    let mut chaos = crate::chaos::Config::default();
    // The CHUDP link: where it listens, the peers named for it, and
    // where a frame goes that none of them names. The socket is bound
    // after the flags are read, so that what is refused is refused before
    // anything is bound.
    let mut udp_at: Option<SocketAddr> = None;
    let mut udp_peers: Vec<(u16, SocketAddr)> = Vec::new();
    let mut udp_default_peer: Option<SocketAddr> = None;
    let mut cycles: Option<u64> = None;
    let mut auto_boot = true;
    // `--pace` and `--no-pace`: the run held to the machine's own speed, or
    // left to go as fast as the host will take it; of the two the last given
    // wins, and neither given is the engine's own default, below.
    let mut pace: Option<bool> = None;
    let mut checkpoint: Option<PathBuf> = None;
    let mut prom_file: Option<PathBuf> = None;
    let mut keyboard_file: Option<PathBuf> = None;
    let mut keyboard_dump = false;
    let mut keyboard_trace = false;
    let mut boot_keys = BootKeys::default();
    let mut resume: Option<PathBuf> = None;
    let mut go_on = false;
    let mut stop_at: Option<u16> = None;
    let mut stop_at_prom: Option<u16> = None;
    let mut boards: usize = geometry.default_memory_boards();
    let mut boards_given = false;
    let mut main_memory_model = false;
    let mut io = true;
    let mut tv = true;
    // `None` until `--tv-board` names one: the machine's own display, the
    // SIMPLE TV on the CADR and the video controller on QUUX.
    let mut tv_board: Option<TvBoard> = None;
    let mut video_size: Option<(usize, usize)> = None;
    let mut timing_model = TimingModel::Cadr;
    let mut timing_given = false;
    let mut sync_cycle_ticks: Option<u8> = None;
    let mut cache: Option<crate::cache::CacheConfig> = None;
    // Revision 14's TLB, `--tlb` (A14.4).
    let mut tlb: Option<usize> = None;
    let mut memory_timing: Option<crate::cache::MemoryTiming> = None;
    // Revision 15's: the period, `--microcycle-ns`, in units of 0.5 ns,
    // and the port's timing with its occupancy (MP2b rulings Q6, Q15, Q17).
    let mut microcycle: Option<u64> = None;
    let mut port_timing: Option<crate::pipeline::PortTiming> = None;
    // QUUX's real-time clock: live unless `--rtc` gives a second to count
    // from.
    let mut rtc = crate::machine::Rtc::Host;
    // QUUX's file device's folders (contract Q9).
    let mut file_roots = crate::file_device::Mounts::default();
    // The color TV, the second display board: off unless `--color-tv`
    // fits it, because a CADR has one screen unless somebody plugged a
    // second board in, and `COLOR-EXISTS-P` is System 100 asking which
    // kind of machine this is. The word after the flag is which board,
    // and without one it is the netlist on `chip` and the model
    // elsewhere; which engine this is is not known until the flags have
    // all been read, so what is kept here is what was asked for.
    let mut color_tv = false;
    let mut color_tv_netlist: Option<bool> = None;
    // Absent, or present with or without an endpoint.
    let mut color_terminal: Option<Option<String>> = None;
    // **The disk controller is a netlist like every other board**, since
    // 12 September 2026, when a boot through it was run to the end ---
    // two and a half days of it, issue 40.  `disk_given` is whether a run
    // said which it wanted, which decides what the model memory does to
    // it below.
    let mut disk_controller = true;
    let mut disk_given = false;
    // The DISK MULTIPLEXOR on the netlist controller's cable, which is
    // what gives it eight drive ports instead of one.
    let mut use_multiplexor = false;
    // A terminal is served whether or not it is asked for: the display,
    // the keyboard and the mouse are the only way the machine is worked.
    // `--serial` opens the other way out, and nothing on that port works
    // the machine.
    let mut listen = TerminalAt::default_display();
    let mut debuggee = false;
    let mut debuggee_pack: Option<Pack> = None;
    // The other machine's Chaosnet, which is its own: its own cable, with
    // nothing else on it. Unset, its address is the debugger's over
    // again, the two cables never meeting.
    let mut debuggee_address: Option<u16> = None;
    // Absent, or present with or without an endpoint.
    let mut debuggee_terminal: Option<Option<String>> = None;
    // DBGIN's connector is there unless the run says not: where it is,
    // and whether `--debug-cable-listen` placed it; `cable_off` is
    // `--no-debug-cable-listen`, and of the two the last given wins.
    let mut cable_listen = CableAt::default_port();
    let mut cable_off = false;
    let mut cable_connect: Option<Connect> = None;
    // The serial port's endpoint: nothing unless `--serial` names one.
    let mut serial_at: Option<SocketAddr> = None;
    let mut capture_tv: Option<PathBuf> = None;
    // The color screen's own recording, which takes the board and shares
    // the one clocks flag with the main screen's.
    let mut capture_color_tv: Option<PathBuf> = None;
    let mut capture_tv_time = true;
    // `--watch`: the range and the nets, resolved against the boards once
    // the machine is built.
    let mut watch: Option<WatchSpec> = None;

    // The flags in the executable's own file, `~/.cadrrc` or `~/.quuxrc`,
    // come first, so that a flag on the command line, which is read after,
    // has the last word.  [`rc_flags`] drops the ones the command line
    // gives too, for the few that may not be given twice.
    let typed: Vec<String> = std::env::args().skip(1).collect();
    // **Asked what this build is, muir answers before it reads anything
    // else.**  Neither of these runs a machine, so nothing a file of
    // flags configures applies to either, and a file outlives the flags
    // it holds: one still holding a spelling muir has since stopped
    // taking is rightly refused for a run, and would otherwise leave a
    // person with no way to ask which muir they have.  The loop below
    // answers them again, for a file that names one, which is harmless
    // and reaches nothing this has not.
    for a in &typed {
        if a == "-h" || a == "--help" {
            help(exe);
        }
        if a == "-V" || a == "--version" {
            println!("{}", version());
            std::process::exit(0);
        }
    }
    // **Which muir, and which file of flags, before anything is parsed.**
    // A report of a refused run wants those two facts most of all, and a
    // file the person at the keyboard was not asked about is the
    // commonest reason a run will not start: one written when a flag was
    // spelled differently outlives the spelling.  So both are said here,
    // and every refusal below follows them.  The rest of the setup waits
    // until there is a machine to describe.
    let (from_file, rc) = rc_flags(&typed, exe);
    let head = {
        let mut head = format!("{} started\n", version());
        if let Some(path) = &rc
            && !from_file.is_empty()
        {
            head.push_str(&format!("flags: {}, from {}\n", from_file.join(" "), shown(path)));
        }
        eprint!("{head}");
        head
    };
    let words: Vec<String> = from_file.iter().chain(&typed).cloned().collect();
    // Kept to say afterwards which flags the chosen engine has no use for.
    let given = words.clone();
    let mut args = words.into_iter().peekable();
    while let Some(a) = args.next() {
        if a == "-h" || a == "--help" {
            help(exe);
        }
        if a == "-V" || a == "--version" {
            println!("{}", version());
            std::process::exit(0);
        }
        // **The executable is the machine**: a flag of the other one's is
        // refused by name before anything it takes is read, saying which
        // executable takes it, and `--machine`, which chose between them,
        // is a flag of neither.
        if let Some(why) = not_this_executables(&a, exe) {
            usage(&why);
        }
        let engine = match a.as_str() {
            "--micro" => Some(Which::Micro),
            "--rtl" => Some(Which::Rtl),
            "--chip" => Some(Which::Chip),
            _ => None,
        };
        match (engine, a.as_str()) {
            (Some(e), _) => match which {
                Some(had) if had != e => usage("--micro, --rtl and --chip are exclusive"),
                _ => which = Some(e),
            },
            (None, "--disk-pack") => {
                let p = pack_flag("--disk-pack", args.next());
                if p.write_protect && exe == "quux" {
                    usage(
                        "--disk-pack: wp is the CADR's Trident's read-only switch, and block-disk has none",
                    );
                }
                if packs.iter().any(|q: &Pack| q.unit == p.unit) {
                    usage(&format!("--disk-pack: unit {} twice; one pack a drive", p.unit));
                }
                packs.push(p);
            }
            (None, "--disk-multiplexor") => use_multiplexor = true,
            (None, "--chaos-address") => {
                // One address: this machine's sixteen switches, in octal
                // or subnet:host. The file and time host's is not muir's
                // to hold --- it is a peer over CHUDP --- so a comma is
                // the old spelling and is refused by name.
                let want = "--chaos-address wants one address in octal or subnet:host; the file and time host is not muir's, and --chaos-udp-peer is where it lives";
                let arg = args.next().unwrap_or_else(|| usage(want));
                match crate::chaos::parse_address(&arg) {
                    Some(a) => chaos.address = a,
                    None => usage(want),
                }
            }
            (None, "--chaos-trace") => chaos.trace = true,
            // The flags of the Chaosnet server muir used to carry. muir
            // has no file or time server in it any more --- a CADR had
            // none --- so there is nothing for them to configure, and
            // they say where the host went rather than reading as an
            // unknown argument.
            (
                None,
                gone @ ("--chaos-file-root" | "--chaos-file-peers" | "--debuggee-chaos-file-root"),
            ) => usage(&format!(
                "{gone} is gone: muir has no file server. A band's file and time host is another program on the network --- ozd, https://github.com/metebalci/ozd --- named with --chaos-udp-peer"
            )),
            (None, "--chaos-udp") => {
                // The endpoint is optional: the next word is it unless it is a flag.
                let spec = args.next_if(|v| !v.starts_with('-'));
                match endpoint(spec.as_deref(), crate::chaos::udp::PORT) {
                    Some(a) => udp_at = Some(a),
                    None => usage("--chaos-udp wants nothing, a port, an address or address:port"),
                }
            }
            (None, "--chaos-udp-default-peer") => {
                let want = "wants <host>[:<port>]: a port, an address, address:port, or a name this host can reach --- and no Chaosnet address, which is what --chaos-udp-peer takes";
                let arg = args
                    .next()
                    .unwrap_or_else(|| usage(&format!("--chaos-udp-default-peer {want}")));
                match default_peer_spec(&arg) {
                    Some(at) => udp_default_peer = Some(at),
                    None => usage(&format!("--chaos-udp-default-peer {arg}: {want}")),
                }
            }
            (None, "--chaos-udp-peer") => {
                let want = "--chaos-udp-peer wants <address>@<host>:<port>, the address in octal or subnet:host";
                let arg = args.next().unwrap_or_else(|| usage(want));
                match peer_spec(&arg) {
                    Ok(p) => udp_peers.push(p),
                    Err(e) => usage(&format!("--chaos-udp-peer {arg}: {e}")),
                }
            }
            // QUUX's main memory is an amount, [`main_memory_amount`].
            (None, "--main-memory-size") => {
                match main_memory_amount(args.next().as_deref(), geometry) {
                    Ok(b) => {
                        boards = b;
                        boards_given = true;
                    }
                    Err(e) => usage(&e),
                }
            }
            (None, "--main-memory") => match args.next().as_deref() {
                Some("netlist") => main_memory_model = false,
                Some("model") => main_memory_model = true,
                _ => usage("--main-memory wants netlist or model"),
            },
            // Main memory on every engine, and the backplane on chip: from
            // one board to the sixty the Xbus I/O space leaves room for
            // (`busint::MAX_MEMORY_BOARDS`). Zero is no memory; the model
            // memory on chip is `--main-memory model`. The CADR's alone:
            // QUUX's main memory is `--main-memory-size <n>MW`.
            (None, "--main-memory-boards") => match args.next().and_then(|v| v.parse().ok()) {
                Some(b) if (1..=geometry.max_memory_boards()).contains(&b) => {
                    boards = b;
                    boards_given = true;
                }
                _ => usage(&format!(
                    "--main-memory-boards wants a count from 1 to {}",
                    geometry.max_memory_boards()
                )),
            },
            (None, "--tv") => match args.next().as_deref() {
                Some("netlist") => tv = true,
                Some("model") => tv = false,
                _ => usage("--tv wants netlist or model"),
            },
            // The CADR's delay lines or muir-fpga's grid under them;
            // QUUX's `sync` is `quux`'s own, and its ticks the flag's.
            (None, "--timing-model") => match args.next().as_deref() {
                Some("sync") => usage("--timing-model wants cadr or fpga: sync is quux's timing"),
                model => match model.and_then(TimingModel::parse) {
                    Some(model) => {
                        timing_model = model;
                        timing_given = true;
                    }
                    None => usage("--timing-model wants cadr or fpga"),
                },
            },
            (None, "--cache") => match args.next().as_deref().and_then(|v| v.parse::<u32>().ok()) {
                Some(words) => {
                    // As the memory port fits it, its line 8 words.
                    let c = crate::cache::CacheConfig::with_words(words);
                    if let Err(e) = crate::memory_port::fitted(c).check() {
                        usage(&format!("--cache: {e}"));
                    }
                    cache = Some(c);
                }
                None => usage("--cache wants its size in words, a power of two"),
            },
            (None, "--tlb") => match args.next().as_deref().and_then(|v| v.parse::<usize>().ok()) {
                Some(n)
                    if n.is_power_of_two()
                        && (crate::tlb::MIN_ENTRIES..=crate::tlb::MAX_ENTRIES).contains(&n) =>
                {
                    tlb = Some(n)
                }
                _ => usage("--tlb wants its entries, a power of two from 1024 to 32768"),
            },
            (None, "--memory-timing") if geometry.extended() => {
                // Revision 15's port: three numbers, the occupancy third, or
                // a board's preset (MP2b rulings Q6, Q17).
                let t = args.next();
                port_timing = t.as_deref().and_then(crate::pipeline::PortTiming::parse);
                if port_timing.is_none() {
                    usage(
                        "--memory-timing wants <read>,<write>,<occupancy> in ns, or kria, arty or de25, on QUUX revision 15",
                    );
                }
            }
            (None, "--memory-timing") => {
                let t = args.next();
                if t.as_deref() == Some("kria")
                    || t.as_deref().is_some_and(|v| v.split(',').count() == 3)
                {
                    usage(&format!(
                        "--memory-timing {} is QUUX revision 15's, with its occupancy: MUIR_QUUX_REVISION=15",
                        t.as_deref().unwrap_or_default()
                    ));
                }
                memory_timing = match t.as_deref() {
                    Some("arty") => Some(crate::cache::MemoryTiming::ARTY_Z7_20),
                    Some("de25") => Some(crate::cache::MemoryTiming::DE25_NANO),
                    Some(v) => v.split_once(',').and_then(|(r, w)| {
                        Some(crate::cache::MemoryTiming {
                            read_ns: r.parse().ok().filter(|&n| n > 0)?,
                            write_ns: w.parse().ok().filter(|&n| n > 0)?,
                        })
                    }),
                    None => None,
                };
                if memory_timing.is_none() {
                    usage("--memory-timing wants <read>,<write> in ns, arty or de25");
                }
            }
            (None, "--rtc") => {
                const WANTS: &str = "--rtc wants a Unix second, 0 to 4294967295, or host";
                rtc = match args.next().as_deref() {
                    Some("host") => crate::machine::Rtc::Host,
                    Some(v) => match v.parse::<u64>() {
                        Ok(start) => match u32::try_from(start) {
                            Ok(start) => crate::machine::Rtc::Counted { start, base_ns: 0 },
                            Err(_) => usage(&format!(
                                "--rtc {start} is past 4294967295, 2^32-1, the last second the RTC's 32 bits hold"
                            )),
                        },
                        Err(_) => usage(WANTS),
                    },
                    None => usage(WANTS),
                };
            }
            (None, "--microcycle-ns") => {
                if !geometry.extended() {
                    usage("--microcycle-ns is QUUX revision 15's: MUIR_QUUX_REVISION=15");
                }
                microcycle = args.next().as_deref().and_then(crate::clock::parse_microcycle_ns);
                if microcycle.is_none() {
                    usage("--microcycle-ns wants the period in ns, 5 to 40 in steps of 0.5");
                }
            }
            (None, "--sync-cycle-ticks") if geometry.extended() => {
                usage(
                    "--sync-cycle-ticks is refused on QUUX revision 15, whose clock is --microcycle-ns's",
                );
            }
            (None, "--sync-cycle-ticks") => {
                match args.next().as_deref().and_then(|v| v.parse::<u8>().ok()).filter(|&k| k > 0) {
                    Some(k) => sync_cycle_ticks = Some(k),
                    None => usage("--sync-cycle-ticks wants a count of 10 ns ticks, 1 to 255"),
                }
            }
            // The CADR's two boards; the video controller is QUUX's
            // display, always.
            (None, "--tv-board") => match args.next().as_deref() {
                Some("simple-tv") => tv_board = Some(TvBoard::SimpleTv),
                Some("lispm-tv") => tv_board = Some(TvBoard::LispmTv),
                Some("video") => {
                    usage("--tv-board wants simple-tv or lispm-tv: video is quux's display")
                }
                _ => usage("--tv-board wants simple-tv or lispm-tv"),
            },
            (None, "--video-size") => {
                let v = args.next().unwrap_or_default();
                let size =
                    v.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
                match size {
                    Some(size) => video_size = Some(size),
                    None => usage("--video-size wants <width>x<height>, such as 1280x1024"),
                }
            }
            (None, "--color-tv") => {
                color_tv = true;
                // The word is optional, as `--color-terminal`'s endpoint
                // is: the next word is it unless it is a flag.
                match args.next_if(|v| !v.starts_with('-')).as_deref() {
                    Some("netlist") => color_tv_netlist = Some(true),
                    Some("model") => color_tv_netlist = Some(false),
                    None => {}
                    Some(_) => usage("--color-tv wants netlist or model, or nothing"),
                }
            }
            (None, "--color-tv-capture") => match args.next() {
                Some(path) => capture_color_tv = Some(PathBuf::from(path)),
                None => usage("--color-tv-capture wants a file for the GIF"),
            },
            (None, "--color-terminal") => {
                // The endpoint is optional: the next word is it unless it is a flag.
                color_terminal = Some(args.next_if(|v| !v.starts_with('-')));
            }
            // MIT's controller, as its netlist or its model; block-disk is
            // QUUX's disk, and `quux`'s only one.
            (None, "--disk-controller") => {
                disk_given = true;
                match args.next().as_deref() {
                    Some("netlist") => disk_controller = true,
                    Some("model") => disk_controller = false,
                    Some("block-disk") => {
                        usage("--disk-controller wants netlist or model: block-disk is quux's disk")
                    }
                    _ => usage("--disk-controller wants netlist or model"),
                }
            }
            (None, "--file-root") => {
                const WANTS: &str = "--file-root wants <folder>[,ro] or <name>=<folder>[,ro]";
                let v = args.next().unwrap_or_else(|| usage(WANTS));
                if let Err(e) = file_roots.add(&v) {
                    usage(&format!("--file-root {v}: {e}"));
                }
            }
            (None, "--io-board") => match args.next().as_deref() {
                Some("netlist") => io = true,
                Some("model") => io = false,
                _ => usage("--io-board wants netlist or model"),
            },
            (None, "--terminal") => {
                // The endpoint is optional: the next word is it unless it is a flag.
                let spec = args.next_if(|v| !v.starts_with('-'));
                match endpoint(spec.as_deref(), TERMINAL_PORT) {
                    Some(addr) => {
                        listen = TerminalAt {
                            addr,
                            port_named: names_a_port(spec.as_deref()),
                            asked: true,
                        };
                    }
                    None => usage("--terminal wants nothing, a port, an address or address:port"),
                }
            }
            (None, "--glass-tty") => {
                // The argument is optional: the next word is it unless it
                // is a flag.
                let spec = args.next_if(|v| !v.starts_with('-'));
                match glass_spec(spec.as_deref()) {
                    Ok(g) => glass_at.push(g),
                    Err(e) => usage(&format!("--glass-tty: {e}")),
                }
            }
            (None, "--debug-in-process") => debuggee = true,
            (None, "--debuggee-chaos-address") => {
                let want = "--debuggee-chaos-address wants one address in octal or subnet:host";
                let arg = args.next().unwrap_or_else(|| usage(want));
                match crate::chaos::parse_address(&arg) {
                    Some(a) => debuggee_address = Some(a),
                    None => usage(want),
                }
            }
            (None, "--debuggee-disk-pack") => {
                debuggee_pack = Some(pack_flag("--debuggee-disk-pack", args.next()));
            }
            (None, "--debuggee-terminal") => {
                debuggee_terminal = Some(args.next_if(|v| !v.starts_with('-')));
            }
            (None, "--debug-cable-listen") => {
                // The endpoint is optional: the next word is it unless it is a flag.
                let spec = args.next_if(|v| !v.starts_with('-'));
                // The window is the other flag's. What is to be built in
                // fabric is the debuggee's DBGIN end, so the fabric is
                // always the debuggee and muir always the debugger; there
                // is no listening at a window and none is proposed.
                if names_a_window(spec.as_deref()) {
                    usage(
                        "--debug-cable-listen takes an endpoint and not a window: the fabric is \
                         the debuggee and muir the debugger, so the window is \
                         --debug-cable-connect's",
                    );
                }
                match endpoint(spec.as_deref(), DEBUG_CABLE_PORT) {
                    Some(addr) => {
                        cable_listen = CableAt {
                            addr,
                            port_named: names_a_port(spec.as_deref()),
                            asked: true,
                        };
                        cable_off = false;
                    }
                    None => usage(
                        "--debug-cable-listen wants nothing, a port, an address or address:port",
                    ),
                }
            }
            (None, "--no-debug-cable-listen") => {
                cable_off = true;
                cable_listen = CableAt::default_port();
            }
            (None, "--debug-cable-connect") => {
                // The endpoint is optional: the next word is it unless it is a flag.
                let spec = args.next_if(|v| !v.starts_with('-'));
                if names_a_window(spec.as_deref()) {
                    let at = window_address("--debug-cable-connect", spec.as_deref().unwrap());
                    cable_connect = Some(Connect::Window(at));
                } else {
                    match endpoint(spec.as_deref(), DEBUG_CABLE_PORT) {
                        Some(a) => cable_connect = Some(Connect::Endpoint(a)),
                        None => usage(
                            "--debug-cable-connect wants nothing, a port, an address, \
                             address:port or 0x<address>",
                        ),
                    }
                }
            }
            (None, "--tv-capture") => match args.next() {
                Some(path) => capture_tv = Some(PathBuf::from(path)),
                None => usage("--tv-capture wants a file for the GIF"),
            },
            (None, "--tv-capture-no-time") => capture_tv_time = false,
            (None, "--watch") => {
                const WANT: &str = "--watch wants <from>[-<to>]:<net>,<net>,...: the microcycles to record over, and the nets as `net` names them";
                let arg = args.next().unwrap_or_else(|| usage(WANT));
                match watch_spec(&arg) {
                    Ok(w) => watch = Some(w),
                    Err(e) => usage(&format!("--watch {arg}: {e}")),
                }
            }
            (None, "--checkpoint") => match args.next() {
                Some(path) => checkpoint = Some(PathBuf::from(path)),
                None => usage("--checkpoint wants a file to write"),
            },
            // Read before the loop, by `rc_flags`, since the file it names is
            // where the loop's first words come from.
            (None, "-c" | "--config") => {
                args.next();
            }
            (None, "--keyboard-boot") => match args.next() {
                Some(keys) => match BootKeys::parse(&keys) {
                    Ok(k) => boot_keys = k,
                    Err(e) => usage(&format!("--keyboard-boot {e}")),
                },
                None => usage(&format!(
                    "--keyboard-boot wants the keys the boot sequence needs: {}",
                    BootKeys::SPELLINGS.join(", ")
                )),
            },
            (None, "--keyboard-mapping") => match args.next() {
                Some(path) => keyboard_file = Some(PathBuf::from(path)),
                None => usage("--keyboard-mapping wants a file of key bindings"),
            },
            (None, "--keyboard-mapping-dump") => keyboard_dump = true,
            (None, "--keyboard-mapping-trace") => keyboard_trace = true,
            (None, "--no-auto-boot") => auto_boot = false,
            (None, "--pace") => pace = Some(true),
            (None, "--no-pace") => pace = Some(false),
            (None, "--prom") => match args.next() {
                Some(path) => prom_file = Some(PathBuf::from(path)),
                None => usage("--prom wants an MCR microcode file"),
            },
            (None, "--resume") => match args.next() {
                Some(path) => resume = Some(PathBuf::from(path)),
                None => usage("--resume wants a checkpoint to start from"),
            },
            (None, "--continue") => go_on = true,
            (None, "--serial") => {
                const WANT: &str = "--serial wants a port or address:port: the endpoint the serial port at J9 is reached at, which has no default";
                let arg = args.next().unwrap_or_else(|| usage(WANT));
                match serial_endpoint(&arg) {
                    Some(a) => serial_at = Some(a),
                    None => usage(&format!("--serial {arg}: {WANT}")),
                }
            }
            (None, "--stop-after") => match args.next().and_then(|v| v.parse().ok()) {
                Some(n) => cycles = Some(n),
                None => usage("--stop-after wants a count of microcycles"),
            },
            (None, "--stop-at") => {
                match args.next().and_then(|v| u16::from_str_radix(&v, 8).ok()) {
                    Some(pc) if pc < 1 << 14 => stop_at = Some(pc),
                    _ => usage("--stop-at wants a PC in octal, below 40000"),
                }
            }
            (None, "--stop-at-prom") => {
                match args.next().and_then(|v| u16::from_str_radix(&v, 8).ok()) {
                    // Which PCs are the PROM's is the machine's, known
                    // once every flag is read.
                    Some(pc) if pc < 1 << 14 => stop_at_prom = Some(pc),
                    _ => usage(
                        "--stop-at-prom wants a PC in octal: below 1000 on the CADR, \
                         36000-37777 on QUUX",
                    ),
                }
            }
            (None, v) => usage(&format!("{v} is not a flag of {exe}")),
        }
    }

    let which = which.unwrap_or(Which::Rtl);
    // A stop where the PC can never be is refused rather than run out.
    // The CADR's PROM lies over control store 0-777 until `PROMDISABLE`
    // (the second bank of `PROM_WORDS` is empty), so a PC names a place
    // in each; QUUX's has addresses of its own, never left to the RAM
    // (contract Q2), and a stop is given as the control-store address the
    // PC holds, the same as on the CADR, whose PROM is at 0.
    match geometry.prom_base {
        None => {
            if stop_at_prom.is_some_and(|pc| pc >= 0o1000) {
                usage("--stop-at-prom wants a PC in octal, below 1000");
            }
        }
        Some(base) => {
            if let Some(pc) = stop_at_prom.filter(|&pc| pc < base) {
                usage(&format!(
                    "--stop-at-prom {pc:o} is not in QUUX's boot PROM, {base:o}-37777: \
                     the PROM's PC is its control-store address"
                ));
            }
            if let Some(pc) = stop_at.filter(|&pc| pc >= base) {
                usage(&format!(
                    "--stop-at {pc:o} is in QUUX's boot PROM, {base:o}-37777: --stop-at-prom \
                     stops there"
                ));
            }
        }
    }
    // The display is the machine's: the SIMPLE TV unless `--tv-board`
    // names the LISPM TV on the CADR, and the video controller on QUUX, always --- the
    // flag being `cadr`'s alone, refused on `quux` with the others.
    let tv_board = tv_board.unwrap_or(if geometry == crate::machine::Geometry::CADR {
        TvBoard::SimpleTv
    } else {
        TvBoard::Video
    });
    // The grid is muir-fpga's, and it is `rtl`'s references its fabric is
    // held to; `micro` and `chip` keep the board's time.
    // QUUX drops the delay lines: its timing is `sync`, always, of the
    // ticks `--sync-cycle-ticks` gives, both flags being their own
    // machine's alone.
    if geometry != crate::machine::Geometry::CADR {
        let cycle_ticks = sync_cycle_ticks.unwrap_or(crate::clock::SYNC_CYCLE_TICKS);
        timing_model = TimingModel::Sync { cycle_ticks, ilong_ticks: 0 };
    }
    // QUUX's disk is block-disk and nothing else: muir-sys's PROM,
    // microcode and band for QUUX address the disk by block. The CADR's
    // is MIT's controller, `--disk-controller` choosing its netlist or its
    // model, which is `cadr`'s alone.
    let block_disk = geometry != crate::machine::Geometry::CADR;
    // QUUX's main memory is on its own port and `rtl` times it.
    if (memory_timing.is_some() || port_timing.is_some()) && which != Which::Rtl {
        usage(&format!(
            "--memory-timing is rtl's, and this run is {}",
            if which == Which::Micro { "micro" } else { "chip" }
        ));
    }
    // The TLB is revision 14's.
    if tlb.is_some() && !geometry.paged() {
        usage("--tlb is QUUX revision 14's: MUIR_QUUX_REVISION=14");
    }
    // The memory cache is QUUX's, and `rtl` is what times it.
    if cache.is_some() && which != Which::Rtl {
        usage(&format!(
            "--cache is rtl's, and this run is {}",
            if which == Which::Micro { "micro" } else { "chip" }
        ));
    }
    if timing_given && timing_model != TimingModel::Cadr && which != Which::Rtl {
        usage(&format!(
            "--timing-model {} is rtl's, and this run is {}",
            timing_model.name(),
            if which == Which::Micro { "micro" } else { "chip" }
        ));
    }
    // **Which color TV, now that the engine is known.** `--color-tv
    // netlist` is a board on `chip`'s backplane and there is no backplane
    // to put one on elsewhere, so it is refused by the engine's name as
    // `--tv netlist` would be; `--color-tv model` is every engine's, and
    // the bare flag is the netlist on `chip` and the model on the other
    // two, as every board on `chip` is a netlist unless a flag says
    // otherwise.
    let color_tv = match (color_tv, which) {
        (false, _) => ColorTv::Off,
        (true, Which::Chip) if color_tv_netlist.unwrap_or(true) => ColorTv::Netlist,
        (true, Which::Chip) => ColorTv::Model,
        (true, engine) => {
            if color_tv_netlist == Some(true) {
                usage(&format!(
                    "--color-tv netlist is chip's: the board is on the backplane, and this run                      is {}",
                    match engine {
                        Which::Micro => "micro",
                        _ => "rtl",
                    }
                ));
            }
            ColorTv::Model
        }
    };
    // The video controller's size is the board's, and checked against the
    // color TV's strap.
    if video_size.is_some() && tv_board != TvBoard::Video {
        usage(&format!(
            "--video-size is the video controller's, and this run's board is {}",
            tv_board.name()
        ));
    }
    let video_size = video_size.unwrap_or((crate::tv::VIDEO_WIDTH, crate::tv::VIDEO_HEIGHT));
    if let Err(e) = crate::tv::check_video_size(video_size.0, video_size.1, color_tv.fitted()) {
        usage(&format!("--video-size: {e}"));
    }
    // The lashups asked for in as many words; the connector nobody placed
    // is not one, being there on every rtl and chip run.
    let cabled = debuggee as u8 + cable_listen.asked as u8 + cable_connect.is_some() as u8;
    if cabled > 1 {
        usage(
            "one of --debug-in-process, --debug-cable-listen and --debug-cable-connect: one lashup at a time",
        );
    }
    if cabled == 1 {
        match which {
            Which::Micro => usage("micro has no timing model, and no end of the debug cable"),
            Which::Chip if !cable_listen.asked => {
                usage("on chip the debug cable is the board's DBGIN only: --debug-cable-listen")
            }
            _ => {}
        }
    }
    // A window muir cannot map is refused by name, and before anything is
    // built: the mapping is `/dev/mem`, which only Linux has, and the
    // debugger has to be a muir running on the board's own processor.
    if let Some(Connect::Window(at)) = cable_connect
        && let Some(why) = crate::fabric::unmappable()
    {
        usage(&format!("--debug-cable-connect {at:#x}: {why}"));
    }
    // The debuggee on a cable is stepped by the debugger's events, in
    // `Remote::step`, and nothing there samples between them; refused
    // rather than quietly recording nothing.  The connector nobody placed
    // is left empty by `--watch` instead, below.
    if which == Which::Chip && watch.is_some() && cable_listen.asked {
        usage("--watch is the run's own loop, which a debuggee on a cable does not have");
    }
    if debuggee_address.is_some() && !debuggee {
        usage(
            "--debuggee-chaos-address is the other machine's Chaosnet: it needs --debug-in-process",
        );
    }
    if debuggee_pack.is_some() && !debuggee {
        usage("--debuggee-disk-pack is the other machine's pack: it needs --debug-in-process");
    }
    // The serial port is one machine's. In the lashup there are two, and
    // the run loops that step them through the debug cable reach neither
    // machine's J9, so an endpoint here would be opened for one of them
    // without saying which. Refused rather than quietly the debugger's.
    // An end of the cable over TCP is one machine in this process, and
    // the port is its.
    if serial_at.is_some() && debuggee {
        usage("--serial is one machine's serial port, and the lashup in one process runs two");
    }
    // The two flags that describe a CHUDP link describe one that has to
    // be there: without a link nothing is listening and the cable carries
    // this machine alone.
    for (flag, given_it) in [
        ("--chaos-udp-peer", !udp_peers.is_empty()),
        ("--chaos-udp-default-peer", udp_default_peer.is_some()),
    ] {
        if given_it && udp_at.is_none() {
            usage(&format!(
                "{flag} is part of the CHUDP link: it needs --chaos-udp, which is the cable"
            ));
        }
    }
    // A peer is somebody else, and one endpoint an address: this
    // machine's own address is not a host over the network, and an
    // address given twice is two answers to where one host lives.
    for (k, &(a, _)) in udp_peers.iter().enumerate() {
        if a == chaos.address {
            usage(&format!("--chaos-udp-peer {a:o}: that is this machine's own address"));
        }
        if udp_peers[..k].iter().any(|&(b, _)| b == a) {
            usage(&format!("--chaos-udp-peer: {a:o} twice; one endpoint an address"));
        }
    }
    // The other machine's display: it takes another machine, and it is not
    // this machine's endpoint. Both are settled here, before anything is
    // bound, so that what is refused is refused whatever the host has
    // going on at the port.
    if let Some(spec) = debuggee_terminal.as_ref() {
        if !debuggee {
            usage(
                "--debuggee-terminal is the other machine's display: it needs --debug-in-process",
            );
        }
        // Port 0 is the host's choice, and two of them are never the same
        // port.
        if debuggee_endpoint(spec.as_deref(), listen.addr) == listen.addr && listen.addr.port() != 0
        {
            usage("--debuggee-terminal: the same endpoint as --terminal");
        }
    }
    // The color screen's display: it takes the board, and it is not
    // another screen's endpoint.  Settled here, before anything is bound.
    if let Some(spec) = color_terminal.as_ref() {
        if !color_tv.fitted() {
            usage("--color-terminal is the color TV's screen: it needs --color-tv");
        }
        // Port 0 is the host's choice, and two of them are never the same
        // port.
        let at = color_endpoint(spec.as_deref(), listen.addr);
        if listen.addr.port() != 0 {
            if at == listen.addr {
                usage("--color-terminal: the same endpoint as --terminal");
            }
            if let Some(other) = debuggee_terminal.as_ref()
                && at == debuggee_endpoint(other.as_deref(), listen.addr)
            {
                usage("--color-terminal: the same endpoint as --debuggee-terminal");
            }
        }
    }
    // The color screen's recording takes the board, as its terminal does.
    if capture_color_tv.is_some() && !color_tv.fitted() {
        usage("--color-tv-capture records the color TV's screen: it needs --color-tv");
    }
    for (flag, given) in
        [("--tv-capture", capture_tv.is_some()), ("--color-tv-capture", capture_color_tv.is_some())]
    {
        if given && (cable_listen.asked || cable_connect.is_some()) {
            usage(&format!(
                "{flag} records a machine on its own or the lashup in one process, not an end of the debug cable to another program or to the fabric",
            ));
        }
    }
    // The prompt's `startcapture` makes a recorder too, so the flag means
    // something on the engines that have one even with no --tv-capture.
    if capture_tv.is_none()
        && capture_color_tv.is_none()
        && !capture_tv_time
        && (which == Which::Chip || cabled == 1)
    {
        usage("--tv-capture-no-time only means anything with --tv-capture");
    }
    let capture = capture_tv.map(|path| (path, capture_tv_time));
    let color_capture = capture_color_tv.map(|path| (path, capture_tv_time));
    if !auto_boot {
        // A machine held with its connector listening is one machine on
        // its own, and a debugger may come to it standing: the two
        // machines powered on together.
        if debuggee || cable_connect.is_some() {
            usage("--no-auto-boot is one machine on its own, not the lashup");
        }
        if !Prompt::possible() {
            usage(
                "--no-auto-boot has nothing to press the button: stdin is a terminal muir is in the background of",
            );
        }
    }
    // A checkpoint holds the whole machine, the PROM's 512 words with it,
    // so resuming one loads the PROM it ran and `--prom` would be a second
    // answer the checkpoint quietly overruled.
    if prom_file.is_some() && resume.is_some() {
        usage("--prom and --resume: the checkpoint carries the PROM it ran");
    }
    // A checkpoint is of a machine on its own: a debugger's cycle in the
    // bus interface is nothing `--resume` can start from.  The connector
    // nobody placed is left empty by `--checkpoint` instead, below.  A
    // resume is before the run, and only the lashup in one process, which
    // builds two machines, has no one machine to resume.
    if checkpoint.is_some() && cabled == 1 {
        usage("--checkpoint is one machine on its own, not the lashup");
    }
    // Pacing is one machine's clock against the host's.  Two machines on a
    // cable already pace each other through the cable's own clock --- each
    // runs as far as the other has promised and then waits for it --- so
    // an end that slept on top of that would only hold the other up, and
    // neither end would be at the machine's speed for it.
    if pace == Some(true) && cabled == 1 {
        usage(
            "--pace is one machine at its own speed, not the lashup: the cable paces the two machines, and an end that slept would only hold the other up",
        );
    }
    // Neither given, `rtl` and `chip` are paced and `micro` is not.  The
    // machine's own nanoseconds are what the band's clock counts, so a run
    // that got ahead of them keeps the band's time ahead of the day: `rtl` is
    // the engine for working the machine, `chip` is slower than the machine
    // and never waits, and `micro` is the fast engine.  A lashup takes no
    // pace nobody asked for, for the reason above.
    let pace = pace.unwrap_or(which != Which::Micro && cabled == 0);
    if resume.is_some() && debuggee {
        usage("--resume is one machine on its own, not the lashup in one process, which runs two");
    }
    // `--continue` is the prompt's `continue` at the start: it runs on a
    // checkpoint's machine, and a cold start has the button for that.
    if go_on && resume.is_none() {
        usage(
            "--continue wants --resume: it runs a checkpoint's halted machine on, and a cold start has the button",
        );
    }
    // A checkpoint is read before the machine is built, so that the machine
    // can be built with as much memory as the checkpoint's had.
    let resume = resume.map(|path| {
        let c = crate::checkpoint::read(&path)
            .unwrap_or_else(|err| stale_checkpoint(&path, &err, None));
        (path, c)
    });
    if let Some(r) = &resume {
        refuse_other_executable(r, exe);
        refuse_other_revision(r, geometry);
    }
    if let Some((path, c)) = &resume {
        if boards_given && boards != c.memory_boards {
            usage(&if exe == "quux" {
                format!(
                    "--resume {}: the checkpoint has {} of main memory, --main-memory-size {}",
                    path.display(),
                    megawords(c.memory_boards << 16),
                    megawords(boards << 16)
                )
            } else {
                format!(
                    "--resume {}: the checkpoint has {} memory boards, --main-memory-boards {boards}",
                    path.display(),
                    c.memory_boards
                )
            });
        }
        boards = c.memory_boards;
    }
    // `--keyboard-mapping-dump` is the mapping and nothing else: it writes
    // the file a run would read and stops, before a terminal is bound or a
    // machine is built, so that stdout carries the mapping alone. The
    // start banner is on stderr, so nothing else has to be held back.
    if keyboard_dump {
        let (mapping, _) = keyboard_mapping(keyboard_file.as_deref());
        print!("{}", mapping.dump());
        std::process::exit(0);
    }
    // The behavioral memory answers the interface's cycles and no other
    // master's; the netlist controller's DMA needs memory boards.
    // **The model memory takes the disk down with it unless the netlist
    // controller was asked for.**  A run that chose the model memory did
    // not ask for a netlist disk and should not be refused for the
    // default's sake; one that asked for both asked for something the
    // backplane cannot do, and is told so.  Either way the start says
    // which controller the run has, so neither is silent.
    if main_memory_model && disk_controller {
        if disk_given {
            usage(
                "--disk-controller netlist needs --main-memory netlist: the model memory does not answer a second master",
            );
        }
        disk_controller = false;
    }
    // The DISK MULTIPLEXOR hangs off the netlist controller's edge
    // connector, so there has to be one for it to hang off: the `chip`
    // engine's, and not its model. The model controller wants no such
    // board on any engine --- it is behavioral and has had eight units
    // all along, `disk_controller::UNITS` --- and `micro` and `rtl` have
    // no netlist board of any kind. The engine is named here rather than
    // left to the controller's default, which is netlist on `chip` and
    // means nothing on the other two.
    if use_multiplexor && !(which == Which::Chip && disk_controller) {
        usage(
            "--disk-multiplexor is a board on the netlist controller's cable: it needs chip, with --disk-controller netlist",
        );
    }
    // Without it the netlist controller has one drive port --- and not
    // because the unit number is forced to 0. `UNIT<2:0>` reach one
    // 74LS244's inputs at DCDA B17 and nothing else: `cadrdc/dc.wlr` gives
    // a direction per pin and there is no `TO` on any of the three, so the
    // board cannot drive them. The one-board jumpers `EP2:ER2`, `ER2:ES2`
    // and `ES2:ET1` ground them to stop them floating, and unit 0 is the
    // consequence. The multiplexor is what supplies the driver: its
    // 74LS175 at 0F05 latches `XBI<30:28>` and reports the unit back on
    // those three posts.
    //
    // So a second drive, or a drive past unit 0, wants a multiplexor ---
    // and is refused until it is asked for. muir could fit one by
    // implication, and used to; a board that appears because of how a
    // pack was spelled is a board the machine has without anybody
    // choosing it, and which machine is being simulated is the user's to
    // say. The model controller is refused nothing: it wants no board for
    // its eight units.
    // The engine is named for the same reason the multiplexor's refusal
    // names it: the controller's default is netlist on `chip` and means
    // nothing on `micro` and `rtl`, whose behavioral controller has
    // addressed eight units all along.
    if which == Which::Chip
        && disk_controller
        && !use_multiplexor
        && (packs.len() > 1 || packs.iter().any(|p| p.unit != 0))
    {
        usage(
            "the netlist disk controller has one drive port, unit 0: a second --disk-pack, or one past unit 0, wants --disk-multiplexor",
        );
    }
    // The run goes on until a stop, a halt or ^C unless a window was asked for.
    let window = cycles.unwrap_or(u64::MAX);
    let stop = Stop { after: window, at: stop_at, at_prom: stop_at_prom };
    let packs: &[Pack] = &packs;
    // The backplane's netlist boards; the model memory, `main`, is `boards`
    // long on every engine.
    let netlist_boards = if main_memory_model { 0 } else { boards };

    // A terminal that was asked for and cannot be served stops the run;
    // the display nobody asked for, when there is no free one or the host
    // will not have a listener at all, leaves the run without a terminal
    // and the start says why.
    let (mut terminal, no_terminal) = match bind_terminal(listen) {
        Ok(t) => (Some(t), None),
        Err(e) if listen.asked => usage(&format!("--terminal {e}")),
        Err(e) => (None, Some(e)),
    };
    // The glass TTYs, which are asked for or not there at all: an empty
    // one costs a run nothing, so there is always a `Glass` rather than a
    // `None` threaded through every loop.  A glass TTY that cannot be
    // served always stops the run, unlike the display nobody asked for:
    // this one nobody gets by default, so it is there because somebody
    // said so.
    let mut glass = match bind_glass(&glass_at) {
        Ok(g) => g,
        Err(e) => usage(&e),
    };
    // DBGIN's connector: a listener for a debugger's cable on every rtl
    // and chip run, as the bus interface's DBGIN is on every machine ---
    // unless the run said not, or wants a machine on its own: micro has no
    // end of the cable; --checkpoint, --tv-capture and chip's --watch are
    // refused beside --debug-cable-listen above and leave the connector
    // nobody placed empty here.  The lashup in one process wires both
    // machines' connectors to each other, and the debugger's end runs its
    // machine inside the cable it plugged into the debuggee: neither
    // listens, and their own lines say what they are.  Bound here, before
    // the machine is built, so that the start can say where.
    let no_cable: Option<String> = if !geometry.unibus {
        Some("QUUX has no Unibus, and the cable is a Unibus master".to_string())
    } else if cable_off {
        Some("--no-debug-cable-listen".to_string())
    } else if which == Which::Micro {
        Some("micro has no timing model, and no end of the debug cable".to_string())
    } else if checkpoint.is_some() {
        Some("--checkpoint writes a machine on its own".to_string())
    } else if capture.is_some() {
        Some("--tv-capture records a machine on its own".to_string())
    } else if color_capture.is_some() {
        Some("--color-tv-capture records a machine on its own".to_string())
    } else if which == Which::Chip && watch.is_some() {
        Some("--watch is the run's own loop".to_string())
    } else {
        None
    };
    let (cable, no_cable) = if debuggee || cable_connect.is_some() {
        (None, None)
    } else {
        match no_cable {
            Some(why) => (None, Some(why)),
            None => match bind_cable(cable_listen) {
                Ok(l) => (Some(l), None),
                Err(e) if cable_listen.asked => usage(&format!("--debug-cable-listen {e}")),
                Err(e) => (None, Some(e)),
            },
        }
    };
    // The other machine's display: the one above this machine's, or where
    // the flag says. In the lashup both machines are served, neither
    // being workable without a terminal.
    let debuggee_listen = debuggee.then(|| {
        let base = terminal.as_ref().and_then(|t| t.addr().ok()).unwrap_or(listen.addr);
        let spec = debuggee_terminal.clone().flatten();
        TerminalAt {
            addr: debuggee_endpoint(spec.as_deref(), base),
            port_named: names_a_port(spec.as_deref()),
            asked: debuggee_terminal.is_some(),
        }
    });
    let (mut debuggee_terminal, no_debuggee_terminal) = match debuggee_listen {
        None => (None, None),
        Some(at) => match bind_terminal(at) {
            Ok(t) => (Some(t), None),
            Err(e) if at.asked => usage(&format!("--debuggee-terminal {e}")),
            Err(e) => (None, Some(e)),
        },
    };
    // The color screen's display: the one above the last display bound,
    // so a lashup with a color board has three and none of them collide,
    // or where the flag says.
    let color_listen = color_tv.fitted().then(|| {
        let bound = |t: &Option<Terminal>| t.as_ref().and_then(|t| t.addr().ok());
        let base = bound(&debuggee_terminal).or_else(|| bound(&terminal)).unwrap_or(listen.addr);
        let spec = color_terminal.clone().flatten();
        TerminalAt {
            addr: color_endpoint(spec.as_deref(), base),
            port_named: names_a_port(spec.as_deref()),
            asked: color_terminal.is_some(),
        }
    });
    let (mut color_screen, no_color_screen) = match color_listen {
        None => (None, None),
        Some(at) => match bind_terminal(at) {
            Ok(mut t) => {
                // Pixels only: the machine's one keyboard and one mouse
                // are on the I/O board and stay with the main screen.
                t.pixels_only = true;
                (Some(t), None)
            }
            Err(e) if at.asked => usage(&format!("--color-terminal {e}")),
            Err(e) => (None, Some(e)),
        },
    };
    // The serial port's endpoint, if one was asked for. Its port is always
    // named, so it is bound as it stands and the run stops if it cannot
    // be: it is where someone is being told to attach, and serving that
    // somewhere else would be worse than not serving it.
    let mut serial = serial_at.map(|addr| match Endpoint::bind(addr) {
        Ok(mut end) => {
            end.trace = true;
            end
        }
        Err(e) => usage(&format!("--serial {addr}: {e}")),
    });
    // The CHUDP link, bound here so that a port that cannot be had stops
    // the run rather than leaving a machine that quietly reaches nobody.
    chaos.udp = udp_at.map(|at| {
        crate::chaos::udp::Link::bind(at, udp_peers, udp_default_peer)
            .unwrap_or_else(|e| usage(&format!("--chaos-udp {at}: {e}")))
    });
    // The other machine's Chaosnet: a cable of its own, since muir's cable
    // carries one machine.  Its address is the debugger's over again
    // unless told otherwise --- the two cables never meet, so there is
    // nothing for them to collide with.  The other machine has no CHUDP
    // link: one socket belongs to one cable, and there is no flag that
    // gives the other machine one.
    let debuggee_chaos = crate::chaos::Config {
        address: debuggee_address.unwrap_or(chaos.address),
        udp: None,
        ..chaos.clone()
    };

    // The boot PROM, before the setup: the setup says which one it is.
    let prom = boot_prom(prom_file.as_deref(), geometry, resume.is_some());
    // What a viewer's keysyms mean on the Lisp Machine keyboard, which is
    // the one part of it that is muir's own and so the user's to change.
    let (keyboard_map, keyboard_said) = keyboard_mapping(keyboard_file.as_deref());
    let _ = KEYS_IN_FORCE.set(keyboard_map);
    KEYS_TRACED.store(keyboard_trace, std::sync::atomic::Ordering::Relaxed);
    let _ = BOOT_KEYS.set(boot_keys);
    // The trace is the flag for a key that will not type, and a key the
    // terminal's input queue lost is one of the answers: under it the
    // count is said every time it changes, and without it the first loss
    // of the run alone. Both terminals, as the trace itself is both
    // machines' keyboards.
    for t in [terminal.as_mut(), debuggee_terminal.as_mut()].into_iter().flatten() {
        t.trace_lost_keys = keyboard_trace;
    }

    // What this run is: said once here, and again by the prompt's `info`.
    let setup = {
        use std::fmt::Write;
        let mut s = String::new();
        let engine = match which {
            Which::Micro => "micro",
            Which::Rtl => "rtl",
            Which::Chip => "chip",
        };
        // A flag the chosen engine has no use for is not an error --- one
        // file of flags serves runs of every engine --- but a run that
        // quietly ignored it would look as though it had obeyed.
        for (flag, engines, has) in [
            ("--disk-controller", "chip", which == Which::Chip || block_disk),
            ("--io-board", "chip", which == Which::Chip),
            ("--main-memory", "chip", which == Which::Chip),
            ("--tv", "chip", which == Which::Chip),
            ("--watch", "chip", which == Which::Chip),
        ] {
            if !has && given.iter().any(|w| w == flag) {
                writeln!(s, "warning: {flag} is {engines}, and this run is {engine}: ignored")
                    .unwrap();
            }
        }
        writeln!(s, "engine: {engine}").unwrap();
        writeln!(s, "prom: {}", prom_shown(prom_file.as_deref(), &prom, geometry)).unwrap();
        let memory_kind = match which {
            Which::Chip if main_memory_model => ", model",
            Which::Chip => ", netlist boards on the Xbus",
            _ => "",
        };
        if exe == "quux" {
            writeln!(s, "memory: {}", megawords(boards << 16)).unwrap();
        } else {
            writeln!(s, "memory: {boards} boards, {}{memory_kind}", memory_size(boards)).unwrap();
        }
        if which == Which::Chip {
            let kind = |netlist: bool| if netlist { "netlist" } else { "model" };
            let tv_kind = tv_board.name();
            writeln!(
                s,
                "boards: I/O board {}, TV {} {tv_kind}, disk controller {}{}{}",
                kind(io),
                kind(tv),
                kind(disk_controller),
                if use_multiplexor { " with a multiplexor, eight drive ports" } else { "" },
                // The second display board, where there is one: which
                // kind of it is on the backplane, beside the others.
                match color_tv {
                    ColorTv::Off => String::new(),
                    kind => format!(", color TV {}", kind.name()),
                }
            )
            .unwrap();
        } else {
            // The board `--tv-board` chose, which the other engines run as
            // the model of; `chip` says it in the line above, beside
            // whether the board itself or its model is on the backplane.
            if tv_board == TvBoard::Video {
                let (w, h) = video_size;
                writeln!(s, "tv: model video, {w}x{h}").unwrap();
            } else {
                writeln!(s, "tv: model {}", tv_board.name()).unwrap();
            }
        }
        if block_disk {
            writeln!(
                s,
                "disk: block-disk, blocks by number, {} us a block",
                crate::block_disk::BLOCK_NS / 1000
            )
            .unwrap();
        }
        // QUUX's main memory is behind its own port, through its cache
        // (contract Q6): `rtl` times both.
        let quux_rtl = geometry.machine_id.is_some() && which == Which::Rtl;
        let rtl_15 = quux_rtl && geometry.extended();
        if rtl_15 {
            // Revision 15's port, in ns and in clocks of the period (MP2b
            // ruling Q17).
            let t = port_timing.unwrap_or(crate::pipeline::PortTiming::KRIA);
            let period = microcycle.unwrap_or(crate::clock::PERIOD_15);
            let c = t.clocks(crate::clock::TimeBase::half_ns(period));
            writeln!(
                s,
                "memory port: a line fill in {} ns, {} clocks; a write answered in {} ns, {}; writes accepted {} ns apart, {}; a queue of {} and {} in flight",
                t.read_ns,
                c.read,
                t.write_ns,
                c.write,
                t.occupancy_ns,
                c.occupancy,
                crate::pipeline::port::QUEUE,
                crate::pipeline::port::IN_FLIGHT
            )
            .unwrap();
            let words = cache.map_or(crate::pipeline::CACHE_WORDS, |c| c.words);
            writeln!(
                s,
                "cache: {words} words, lines of {}, 2-way, a hit in two clocks",
                crate::pipeline::port::LINE_WORDS
            )
            .unwrap();
        } else if quux_rtl {
            let t = memory_timing.unwrap_or(crate::cache::MemoryTiming::NOMINAL);
            writeln!(s, "memory port: a line fill in {} ns, a write in {}", t.read_ns, t.write_ns)
                .unwrap();
        }
        // The cache the memory port fits: QUUX keeps its 8-word line
        // whatever `--cache` asks for.
        if let Some(c) = cache
            .or(quux_rtl.then_some(crate::cache::CacheConfig::QUUX))
            .filter(|_| !rtl_15 && !geometry.extended())
        {
            let c = if quux_rtl { crate::memory_port::fitted(c) } else { c };
            writeln!(
                s,
                "cache: {} words, lines of {}, {}-way, a hit in {} ns",
                c.words, c.line_words, c.ways, c.hit_ns
            )
            .unwrap();
        }
        if geometry.revision() == Some(15) {
            writeln!(
                s,
                "machine: quux, revision 15: revision 14 with a 64-bit microinstruction, MIT's 48 bits and an extension, the OA registers read through a word's OA select in place of IMOD, and its own .mcr; rtl is its four-stage pipeline"
            )
            .unwrap();
        }
        if geometry.revision() == Some(14) {
            writeln!(
                s,
                "machine: quux, revision 14: revision 13 with 32-bit virtual addresses, the device window at 34000000000 and the physical memory window at 36000000000, a page table walked by hardware behind a direct-mapped TLB of {} entries, a 34-bit location counter, and the register page at 35777777400",
                tlb.unwrap_or(crate::tlb::DEFAULT_ENTRIES)
            )
            .unwrap();
        }
        if geometry.revision() == Some(13) {
            writeln!(
                s,
                "machine: quux, revision 13: a 40-bit word, the tag <39:32> over the field <31:0>; a map of two levels over 1024-word pages, 28-bit virtual and physical addresses; a dispatch memory of 4,096 entries; a 16K-word PDL buffer, MUL and DIV in one instruction each, a microsecond clock in the processor, its boot PROM at control store 36000, main memory and the frame buffer on its own port, the memory cache's lines of 8 words, its devices reached by their registers, a real-time clock, a file device, three interval timers and reset devices, the frame buffer window at 1760000000 and the register page at 1777777400 with block-disk and the video controller on it, and the fused return"
            )
            .unwrap();
        }
        if geometry.rtc {
            match rtc {
                crate::machine::Rtc::Host => writeln!(s, "rtc: the host's clock").unwrap(),
                crate::machine::Rtc::Counted { start, .. } => {
                    writeln!(s, "rtc: from {start}, counting machine time").unwrap()
                }
            }
        }
        if geometry.file_device {
            writeln!(
                s,
                "file device: {} us a command and {} us a KiB",
                crate::file_device::COMMAND_NS / 1000,
                crate::file_device::KIB_NS / 1000
            )
            .unwrap();
            for line in file_roots.describe() {
                writeln!(s, "file device: {line}").unwrap();
            }
        }
        let chosen = pack_choice(packs);
        if chosen.is_empty() {
            writeln!(s, "pack: none; the boot waits on a drive that never answers").unwrap();
        }
        for (p, unit, ro, wp) in chosen {
            // QUUX's disk says what its footer made it and its size, the
            // two things that are no longer a T-300's (contract Q8).
            let kind = match block_disk.then(|| crate::disk_image::probe(&p)) {
                Some(Ok((f, bytes))) => {
                    format!(
                        ", {}, {} blocks",
                        f.name(),
                        bytes / crate::disk_image::BLOCK_BYTES as u64
                    )
                }
                _ => String::new(),
            };
            writeln!(
                s,
                "pack: {} in unit {unit}{kind}{}",
                shown(&p),
                if wp {
                    ", the read-only switch on: a write faults, and nothing reaches the file"
                } else if ro {
                    ", opened read-only: a written block stays in the run, and nothing reaches the file"
                } else {
                    ", written as the machine writes it"
                }
            )
            .unwrap();
        }
        // **The switches, then the cable.**  They are two things on the
        // board and two lines here: a machine has its address set whether
        // or not anything is plugged into it, and one with no cable talks
        // to nobody however its switches read.
        writeln!(s, "chaosnet: {:o}", chaos.address).unwrap();
        match &chaos.udp {
            None => {
                writeln!(s, "chaosnet over udp: disabled; --chaos-udp is the cable").unwrap();
            }
            Some(link) => {
                let peers = match link.peers.as_slice() {
                    // With a default peer there is a way to a host all
                    // the same, so the line does not say there is none.
                    [] if link.default_peer.is_some() => "no peer named".to_string(),
                    [] => "no peer named, so no file or time host".to_string(),
                    p => p
                        .iter()
                        .map(|(a, e)| format!("{a:o} at {e}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                };
                let rest = match link.default_peer {
                    Some(at) => format!(", anything else to {at}"),
                    None => String::new(),
                };
                writeln!(s, "chaosnet over udp: {}, {peers}{rest}", link.at).unwrap();
            }
        }
        writeln!(s, "terminal: {}", terminal_line(&terminal, &no_terminal, listen.addr)).unwrap();
        // The glass TTYs, when any were asked for: where each is and
        // whether it takes typing.  Nobody gets one by default, so a run
        // without them says nothing rather than saying there are none.
        if !glass.is_empty() {
            let each: Vec<String> = glass
                .addrs()
                .iter()
                .map(|(at, ro)| format!("telnet://{at}{}", if *ro { " read-only" } else { "" }))
                .collect();
            writeln!(
                s,
                "glass tty: {}; the screen as text through {}",
                each.join(", "),
                glass.font_in_force()
            )
            .unwrap();
        }
        // The second display board and where its screen is served. Both
        // lines, because they are two things: a board on the backplane,
        // and a monitor on it.
        if color_tv.fitted() {
            writeln!(
                s,
                "color tv: {} lispm-tv at 17200000, 576 x 454 four-bit{}",
                color_tv.name(),
                match color_tv {
                    ColorTv::Netlist => "; the board answers and the model holds the picture",
                    _ => "",
                }
            )
            .unwrap();
            let at = color_listen.map_or(listen.addr, |a| a.addr);
            writeln!(
                s,
                "color terminal: {}; pixels only, no keyboard or mouse",
                terminal_line(&color_screen, &no_color_screen, at)
            )
            .unwrap();
        }
        writeln!(s, "keyboard: {keyboard_said}; boot sequence {boot_keys} with Rubout or Return")
            .unwrap();
        if let Some(end) = &serial {
            let at = end.addr().unwrap_or_else(|_| serial_at.expect("the endpoint was asked for"));
            writeln!(s, "serial: tcp://{at} --- the device on the null-modem cable at J9").unwrap();
        }
        if debuggee {
            let pack = match debuggee_pack.as_ref() {
                Some(p) => format!("pack {}", shown(&p.path)),
                None => "no pack, so its boot waits on the drive".to_string(),
            };
            writeln!(s, "debuggee: in this process, {pack}").unwrap();
            writeln!(
                s,
                "debuggee chaosnet: {:o}, alone on a cable of its own",
                debuggee_chaos.address
            )
            .unwrap();
            if let Some(at) = debuggee_listen {
                writeln!(
                    s,
                    "debuggee terminal: {}",
                    terminal_line(&debuggee_terminal, &no_debuggee_terminal, at.addr)
                )
                .unwrap();
            }
        } else if let Some(Connect::Endpoint(a)) = cable_connect {
            writeln!(
                s,
                "debug cable: this machine the debugger, DBGOUT connecting to {a}; its DBGIN not \
                 listening"
            )
            .unwrap();
        } else if let Some(Connect::Window(a)) = cable_connect {
            writeln!(
                s,
                "debug cable: this machine the debugger, DBGOUT at the fabric's window at {a:#x}; \
                 its DBGIN not listening"
            )
            .unwrap();
        } else if let Some(l) = &cable {
            let at = l.local_addr().map_or(cable_listen.addr.to_string(), |a| a.to_string());
            writeln!(s, "debug cable: DBGIN listening at {at}").unwrap();
        } else if let Some(why) = &no_cable {
            writeln!(s, "debug cable: none --- {why}").unwrap();
        }
        let clocks = if capture_tv_time { "" } else { ", no clocks" };
        if let Some((p, _)) = &capture {
            writeln!(s, "capture: {}{clocks}", p.display()).unwrap();
        }
        if let Some((p, _)) = &color_capture {
            writeln!(s, "color capture: {}{clocks}", p.display()).unwrap();
        }
        if let Some(p) = &checkpoint {
            writeln!(s, "checkpoint: {} at the stop", p.display()).unwrap();
        }
        if let Some((p, c)) = &resume {
            let memory = if exe == "quux" {
                format!("{} of main memory", megawords(c.memory_boards << 16))
            } else {
                format!("{} boards", c.memory_boards)
            };
            writeln!(s, "resume: {}, {} with {memory}", p.display(), c.engine).unwrap();
        }
        let mut stops = Vec::new();
        if let Some(n) = cycles {
            stops.push(format!("after {n} microcycles"));
        }
        if let Some(pc) = stop_at {
            stops.push(format!("at PC {pc:o}"));
        }
        if let Some(pc) = stop_at_prom {
            stops.push(format!("at PC {pc:o} in the PROM"));
        }
        // What a microcycle takes: the board's delay lines on the CADR,
        // muir-fpga's grid under them, or QUUX's `sync`, which has none.
        if geometry.extended() {
            let period = microcycle.unwrap_or(crate::clock::PERIOD_15);
            writeln!(
                s,
                "timing: revision 15's clock, {} ns a microcycle ({period} units of 0.5 ns)",
                crate::clock::microcycle_ns_text(period)
            )
            .unwrap();
        } else if which != Which::Chip {
            let per = timing_model.cycle_ns(crate::clock::Speed::Normal, false);
            let timing = match timing_model {
                TimingModel::Cadr => {
                    format!("cadr, the board's delay lines, {per} ns a microcycle at normal speed")
                }
                TimingModel::Fpga => format!(
                    "fpga, the board's delay lines on muir-fpga's 10 ns grid, {per} ns a microcycle at normal speed"
                ),
                TimingModel::Sync { cycle_ticks, .. } => {
                    format!("sync, {cycle_ticks} ticks of 10 ns, {per} ns a microcycle")
                }
            };
            writeln!(s, "timing: {timing}").unwrap();
        }
        if stops.is_empty() {
            writeln!(s, "stop: none; a halt or ^C").unwrap();
        } else {
            writeln!(s, "stop: {}", stops.join(", ")).unwrap();
        }
        // `--pace`: what the run is held to, and on `chip` that the flag
        // will not bite --- the engine being far slower than the machine,
        // the run is never ahead of its clock.
        if pace {
            writeln!(
                s,
                "pace: {}",
                if which == Which::Chip {
                    "the machine's own speed; chip is far slower than that, so nothing waits"
                        .to_string()
                } else {
                    format!(
                        "the machine's own speed, {} ns a microcycle; the run waits when it is ahead",
                        timing_model.cycle_ns(crate::clock::Speed::Normal, false)
                    )
                }
            )
            .unwrap();
        }
        // `chip` is the engine whose runs go for hours, and had no prompt
        // when this was added; the person watching one needs to be told
        // this exists or it does not pay. Issue 86.
        if which == Which::Chip {
            writeln!(s, "where: kill -USR1 {} prints the PC and IR, and the run goes on", pid())
                .unwrap();
        }
        if !auto_boot {
            writeln!(
                s,
                "start: held, and the boot button not pressed; boot at the prompt presses it"
            )
            .unwrap();
        }
        // The prompt: every run that is one machine muir holds has it ---
        // an engine alone, `chip` included, with or without a debugger on
        // its connector, and the `rtl` machine at the debugger's end of
        // the cable.  The lashup in one process has none, and says so
        // rather than nothing.
        let no_prompt = if debuggee {
            Some("the lashup in one process runs two machines and takes no commands for either")
        } else if !Prompt::possible() {
            Some("stdin is a terminal muir is in the background of")
        } else {
            None
        };
        match no_prompt {
            None => writeln!(s, "^C holds the machine at the prompt; help lists muir's commands"),
            Some(why) => writeln!(s, "prompt: none; {why}"),
        }
        .unwrap();
        s
    };
    // **Armed before it is announced.**  The line below says `kill -USR1`
    // asks a run where it is, and the default action for that signal is
    // to kill the process, so a signal sent on the strength of the line
    // must find a handler already installed.  Each engine's loop calls
    // this again, which costs nothing: the same handler, installed twice.
    catch_interrupts();
    eprint!("{setup}");
    let setup = format!("{head}{setup}");

    match which {
        Which::Micro => {
            // The Chaosnet, as under rtl and chip: the interface is the
            // I/O board's and the board is the machine's, so it is the
            // same three lines whatever engine runs it.  What it wants
            // from an engine is a clock, and this one has the machine's
            // periods; `tests/micro_chaos.rs` holds the two engines to
            // the same conversation with the server.
            let mut m = machine(
                &prom,
                packs,
                boards,
                (tv_board, video_size),
                color_tv,
                (geometry, block_disk, rtc, &file_roots),
            );
            if let Some(n) = tlb {
                m.set_tlb_entries(n);
            }
            m.chaos = chaos.clone();
            m.plug_chaos(0);
            let mut e = Micro::new(m);
            // QUUX's microcycle is `sync`'s ticks, on this engine's clock too;
            // revision 15's is its period (MP2b ruling Q15).
            if geometry.extended() {
                e.period = microcycle.unwrap_or(crate::clock::PERIOD_15);
            } else if let TimingModel::Sync { cycle_ticks, .. } = timing_model {
                e.sync_cycle_ns = cycle_ticks as u64 * crate::clock::GRID_NS;
            }
            if auto_boot {
                e.boot();
            }
            if let Some(p) = &resume {
                resume_engine(
                    "micro",
                    &mut e,
                    (tv_board, video_size),
                    color_tv,
                    (geometry, rtc),
                    p,
                    go_on,
                );
            }
            let run = Run {
                stop,
                capture,
                color_capture,
                checkpoint,
                setup: &setup,
                hold: !auto_boot,
                pace,
                clocks: capture_tv_time,
                color: color_screen.as_mut(),
            };
            time_engine(
                "micro",
                Alone(e),
                terminal.as_mut(),
                Some(&mut glass),
                serial.as_mut(),
                run,
            );
        }
        Which::Rtl if geometry.extended() => {
            // Revision 15's `rtl` is its pipeline (contract G3 revision 15,
            // §12.1): the Kria's period, memory timing and cache unless the
            // flags say otherwise (MP2b ruling Q14).
            let mut m = machine(
                &prom,
                packs,
                boards,
                (tv_board, video_size),
                color_tv,
                (geometry, block_disk, rtc, &file_roots),
            );
            if let Some(n) = tlb {
                m.set_tlb_entries(n);
            }
            m.chaos = chaos.clone();
            m.plug_chaos(0);
            let mut e = crate::pipeline::Pipeline::new(m);
            e.configure(
                microcycle.unwrap_or(crate::clock::PERIOD_15),
                port_timing.unwrap_or(crate::pipeline::PortTiming::KRIA),
                cache.map_or(crate::pipeline::CACHE_WORDS, |c| c.words),
            );
            if auto_boot {
                e.boot();
            }
            if let Some(p) = &resume {
                resume_engine(
                    "rtl",
                    &mut e,
                    (tv_board, video_size),
                    color_tv,
                    (geometry, rtc),
                    p,
                    go_on,
                );
            }
            let run = Run {
                stop,
                capture,
                color_capture,
                checkpoint,
                setup: &setup,
                hold: !auto_boot,
                pace,
                clocks: capture_tv_time,
                color: color_screen.as_mut(),
            };
            time_engine("rtl", Alone(e), terminal.as_mut(), Some(&mut glass), serial.as_mut(), run);
        }
        Which::Rtl => {
            let mut m = machine(
                &prom,
                packs,
                boards,
                (tv_board, video_size),
                color_tv,
                (geometry, block_disk, rtc, &file_roots),
            );
            if let Some(n) = tlb {
                m.set_tlb_entries(n);
            }
            // The Chaosnet, as under chip: the interface on the I/O board
            // and, if a link was bound, the network on its cable.
            m.chaos = chaos.clone();
            m.plug_chaos(0);
            let mut e = Rtl::new(m);
            e.set_timing_model(timing_model);
            if cache.is_some() {
                e.set_cache(cache);
            }
            if memory_timing.is_some() {
                e.set_memory_timing(memory_timing);
            }
            if auto_boot {
                e.boot();
            }
            if debuggee {
                // The other machine's pack is only the one named: CC's
                // debuggee usually has none --- CC loads it over the cable.
                let mut mb = Machine::with_memory_boards(boards);
                mb.geometry = geometry;
                if block_disk {
                    mb.block_disk =
                        Some(crate::block_disk::BlockDisk::new(crate::block_disk::BLOCK_NS));
                }
                mb.load_prom(&prom);
                mb.tv.set_video_size(video_size.0, video_size.1);
                mb.tv.set_board(tv_board);
                if let Some(p) = debuggee_pack.as_ref() {
                    attach(&mut mb, std::slice::from_ref(p));
                }
                // Its own Chaosnet, on a cable of its own: the two
                // machines cannot hear each other over it, and the only
                // wire between them is the debug cable.
                mb.chaos = debuggee_chaos.clone();
                mb.plug_chaos(0);
                let mut b = Rtl::new(mb);
                b.set_timing_model(timing_model);
                if cache.is_some() {
                    b.set_cache(cache);
                }
                b.boot();
                time_lashup(
                    Lashup::new(e, b),
                    stop,
                    terminal.as_mut(),
                    debuggee_terminal.as_mut(),
                    Some(&mut glass),
                    color_screen.as_mut(),
                    capture,
                    color_capture,
                );
            } else if let Some(Connect::Endpoint(addr)) = cable_connect {
                // The debuggee may still be starting: try for five seconds.
                let mut tries = 0;
                let stream = loop {
                    match std::net::TcpStream::connect(addr) {
                        Ok(s) => break s,
                        Err(err)
                            if tries < 50
                                && err.kind() == std::io::ErrorKind::ConnectionRefused =>
                        {
                            tries += 1;
                            std::thread::sleep(Duration::from_millis(100));
                        }
                        Err(err) => usage(&format!("--debug-cable-connect {addr}: {err}")),
                    }
                };
                eprintln!("debug cable: DBGOUT connected to the debuggee at {addr}");
                let reader = stream.try_clone().expect("a second handle on the cable");
                if let Some(p) = &resume {
                    resume_engine(
                        "rtl",
                        &mut e,
                        (tv_board, video_size),
                        color_tv,
                        (geometry, rtc),
                        p,
                        go_on,
                    );
                    refuse_timing_model(p, e.timing_model(), timing_model);
                }
                let run = Run {
                    stop,
                    capture: None,
                    color_capture: None,
                    checkpoint: None,
                    setup: &setup,
                    hold: !auto_boot,
                    pace,
                    clocks: capture_tv_time,
                    color: color_screen.as_mut(),
                };
                time_engine(
                    "rtl, debugger",
                    Remote::debugger(e, reader, stream),
                    terminal.as_mut(),
                    Some(&mut glass),
                    serial.as_mut(),
                    run,
                );
            } else if let Some(Connect::Window(at)) = cable_connect {
                // The identity is read before anything is stored, and a
                // window that is not the adapter ends the run here: there
                // is no falling back to the network and no retrying.
                let window = crate::fabric::open(at).unwrap_or_else(|why| {
                    eprintln!("{}: --debug-cable-connect {at:#x}: {why}", executable());
                    std::process::exit(1);
                });
                eprintln!("debug cable: DBGOUT at the fabric's window at {at:#x}");
                time_fabric(
                    FreeRunning::new(e, window),
                    stop,
                    terminal.as_mut(),
                    color_screen.as_mut(),
                    Some(&mut glass),
                    &setup,
                );
            } else {
                if let Some(p) = &resume {
                    resume_engine(
                        "rtl",
                        &mut e,
                        (tv_board, video_size),
                        color_tv,
                        (geometry, rtc),
                        p,
                        go_on,
                    );
                    refuse_timing_model(p, e.timing_model(), timing_model);
                }
                let run = Run {
                    stop,
                    capture,
                    color_capture,
                    checkpoint,
                    setup: &setup,
                    hold: !auto_boot,
                    pace,
                    clocks: capture_tv_time,
                    color: color_screen.as_mut(),
                };
                // The machine with DBGIN's connector at it, listening or
                // not: a debugger that connects is plugged in between two
                // microcycles.
                let end = Connector::new(e, cable)
                    .unwrap_or_else(|e| fail(&format!("the debug cable's listener: {e}")));
                time_engine("rtl", end, terminal.as_mut(), Some(&mut glass), serial.as_mut(), run);
            }
        }
        Which::Chip => {
            let image: Vec<u64> = prom.iter().copied().map(crate::prom::programming).collect();
            // `--chip` is `cadr`'s alone and refused above on `quux`, which
            // has no netlists to pass.
            let nets = netlists.expect("chip on an executable with no netlists");
            let io_n = io.then(|| netlist::parse(nets.cadrio).unwrap());
            let tv_n = tv.then(|| {
                netlist::parse(match tv_board {
                    TvBoard::SimpleTv => nets.simpletv,
                    TvBoard::LispmTv => nets.lispmtv,
                    // Refused with `chip` above: QUUX has no netlist.
                    TvBoard::Video => unreachable!("the video controller on chip"),
                })
                .unwrap()
            });
            // With a multiplexor on the controller's cable the six
            // one-board jumpers come off, those nets being the
            // multiplexor's to drive.
            let disk_n = disk_controller.then(|| {
                if use_multiplexor {
                    netlist::parse_with_multiplexor(nets.cadrdc).unwrap()
                } else {
                    netlist::parse(nets.cadrdc).unwrap()
                }
            });
            let dm_n = use_multiplexor.then(|| netlist::parse(nets.dm).unwrap());
            // The second display board, when `--color-tv` asked for the
            // netlist: the LISPM TV, which is the board `lmtv.order`
            // specifies and the only one there is a color strap for,
            // whatever `--tv-board` put at the main screen's addresses.
            let color_tv_n = (color_tv == ColorTv::Netlist)
                .then(|| netlist::parse_color_tv(nets.lispmtv).unwrap());
            let on_the_buses = Boards {
                memory: netlist_boards,
                io: io_n.as_ref(),
                tv: tv_n.as_ref(),
                color_tv: color_tv_n.as_ref(),
                disk: disk_n.as_ref(),
                multiplexor: dm_n.as_ref(),
            };
            let run = Run {
                stop,
                capture,
                color_capture,
                checkpoint,
                setup: &setup,
                hold: !auto_boot,
                pace,
                clocks: capture_tv_time,
                color: color_screen.as_mut(),
            };
            time_chip(
                nets,
                cable,
                &image,
                packs,
                on_the_buses,
                boards,
                chaos,
                terminal.as_mut(),
                Some(&mut glass),
                serial.as_mut(),
                run,
                (resume, go_on),
                tv_board,
                color_tv,
                watch,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A machine that stopped itself is seen in `FLAG-1`, never in
    /// `step`.** `HALT-CONS` under `ERRSTOP` --- misc function 1, which is
    /// what System 100's `(si:%halt)` runs --- leaves `RUN` set and `ERR`
    /// up, and every `step` after it returns `Ok` having run no
    /// microcycle. So the run loop cannot learn this from stepping, and
    /// [`machrun_low`] is what it reads instead.
    ///
    /// `tests/halt.rs` holds both engines to the halt itself; this holds
    /// the run loop's reading of it.
    #[test]
    fn a_machine_that_stopped_itself_reads_low_on_machrun() {
        // `HALT-CONS`, `1_10.` in `sys/sys/cadsym.lisp`: `IR<11:10>` = 1.
        const HALT_CONS: u64 = 1 << 10;
        let halting = || {
            let mut m = Machine::new();
            let mut prom = vec![crate::isa::asm::filler(); 512];
            prom[5] = Insn::new(crate::isa::asm::filler().raw() | HALT_CONS);
            m.load_prom(&prom);
            m
        };

        // Running, with nothing to report.
        let mut e = Rtl::new(halting());
        e.boot();
        assert_eq!(machrun_low(&e), None, "just booted and running");

        // `ERRSTOP` is set after the boot, which resets the console's
        // registers. Forty microcycles is well past the halt at 5.
        e.machine_mut().mode.errstop = true;
        for _ in 0..40 {
            e.step().expect("no halt this engine raises");
        }
        let why = machrun_low(&e).expect("stopped by HALT-CONS under ERRSTOP");
        assert!(why.contains("ERRSTOP"), "and says why: {why}");

        // Without `ERRSTOP` the same program runs straight through it, so
        // there is nothing for the run loop to hold on.
        let mut e = Rtl::new(halting());
        e.boot();
        for _ in 0..40 {
            e.step().expect("no halt this engine raises");
        }
        assert_eq!(machrun_low(&e), None, "HALT-CONS without ERRSTOP runs on");

        // And the same on `micro`, which the run loop treats alike.
        let mut e = Micro::new(halting());
        e.boot();
        e.machine_mut().mode.errstop = true;
        for _ in 0..40 {
            e.step().expect("no halt this engine raises");
        }
        assert!(machrun_low(&e).is_some(), "micro stops the same way");
    }

    /// **The prompt's hold on the fabric's debugger**, at the unit level:
    /// a run against fabric wants `/dev/mem` on Linux and cannot be
    /// spawned here, so this is the [`Hold`] `time_fabric` runs, on the
    /// machine it runs it on --- [`FreeRunning`] over a
    /// [`crate::fabric::Fabric`], on the array window --- fed lines as
    /// stdin would feed them.  `hold` holds; `step` runs exactly so many
    /// and holds again, with no line read until they have run;
    /// `checkpoint` is refused and writes nothing; and ^C holds, and one
    /// more while held quits.  What this does not hold is `time_fabric`'s
    /// own loop calling these at its checks, which mirrors
    /// `time_remote`'s, and that one `tests/muir_prompt.rs` holds end to
    /// end over TCP.
    #[test]
    fn the_prompt_holds_the_fabrics_debugger() {
        let mut m = Machine::new();
        m.load_prom(&crate::prom::boot_prom());
        let window = crate::fabric::Fabric::open(crate::fabric::Words::new()).unwrap();
        let mut run = FreeRunning::new(Rtl::new(m), window);
        run.debugger.boot();
        let (typed, prompt) = Prompt::piped();
        let mut hold = Hold {
            prompt: Some(prompt),
            on: false,
            stepping: None,
            quit: false,
            interrupts_seen: INTERRUPTS.load(Ordering::SeqCst),
        };
        let chk = std::env::temp_dir().join(format!("muir-fabric-hold-{}.chk", std::process::id()));
        let mut ran = 0;

        typed.send("hold".into()).unwrap();
        hold.lines(&mut run.debugger, ran, "", &mut Writes::CableEnd);
        assert!(hold.on, "hold holds");

        // `step 3`, and a line behind it that is not read until the three
        // have run: the loop's turns, as `time_fabric` takes them.
        typed.send("step 3".into()).unwrap();
        typed.send(format!("checkpoint {}", chk.display())).unwrap();
        hold.lines(&mut run.debugger, ran, "", &mut Writes::CableEnd);
        assert!(!hold.on && hold.stepping == Some(3), "step takes the hold off for three");
        while !hold.on {
            run.step().unwrap();
            ran += 1;
            hold.stepped(&run.debugger, ran);
            hold.lines(&mut run.debugger, ran, "", &mut Writes::CableEnd);
        }
        assert_eq!(ran, 3, "exactly three, then the hold is back on");
        assert!(hold.stepping.is_none() && !hold.quit);
        assert!(!chk.exists(), "checkpoint is refused on an end of the cable: nothing written");

        // ^C while running holds; ^C while held quits.
        typed.send("continue".into()).unwrap();
        hold.lines(&mut run.debugger, ran, "", &mut Writes::CableEnd);
        assert!(!hold.on, "continue runs on");
        INTERRUPTS.fetch_add(1, Ordering::SeqCst);
        hold.interrupts(&run.debugger, ran);
        assert!(hold.on && !hold.quit, "the first ^C holds");
        INTERRUPTS.fetch_add(1, Ordering::SeqCst);
        hold.interrupts(&run.debugger, ran);
        assert!(hold.quit, "and one more while held quits");
        drop(typed);
    }

    /// **The wall clock reads the C library's `struct tm` where the hours,
    /// minutes and seconds are.** It is a time of day, and it stands from
    /// UTC's by a time zone's offset: a whole number of quarter hours,
    /// within fourteen hours either way.
    #[test]
    fn the_wall_clock_is_the_local_time_of_day() {
        let utc =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
                % 86_400;
        let local = wall_clock() / 1_000_000_000;
        assert!(local < 86_400, "a time of day: {local}");
        let offset = (local as i64 - utc as i64).rem_euclid(86_400);
        let offset = if offset > 43_200 { offset - 86_400 } else { offset };
        assert!(offset.abs() <= 14 * 3600, "within a zone's reach: {offset} s");
        assert_eq!(offset.rem_euclid(900), 0, "a whole number of quarter hours: {offset} s");
    }

    #[test]
    fn a_pack_is_an_image_then_its_unit_ro_and_wp_in_any_order() {
        let pack = |unit, read_only, write_protect| Pack {
            path: PathBuf::from("a.img"),
            unit,
            read_only,
            write_protect,
        };
        assert_eq!(pack_spec("a.img"), Ok(pack(0, false, false)));
        assert_eq!(pack_spec("a.img,ro"), Ok(pack(0, true, false)));
        assert_eq!(pack_spec("a.img,rw"), Ok(pack(0, false, false)));
        assert_eq!(pack_spec("a.img,3"), Ok(pack(3, false, false)));
        assert_eq!(pack_spec("a.img,3,ro"), Ok(pack(3, true, false)));
        assert_eq!(pack_spec("a.img,ro,3"), Ok(pack(3, true, false)));
        // The switch lets nothing reach the file, which is opened read-only.
        assert_eq!(pack_spec("a.img,wp"), Ok(pack(0, true, true)));
        assert_eq!(pack_spec("a.img,wp,ro"), Ok(pack(0, true, true)));
        assert_eq!(pack_spec("a.img,wp,2"), Ok(pack(2, true, true)));
        assert!(pack_spec("a.img,wp,rw").is_err());
        assert!(pack_spec("a.img,wp,wp").is_err());
        assert!(pack_spec("").is_err());
        assert!(pack_spec(",ro").is_err());
        assert!(pack_spec("a.img,ro,ro").is_err());
        assert!(pack_spec("a.img,ro,rw").is_err());
        assert!(pack_spec("a.img,1,2").is_err());
        assert!(pack_spec("a.img,8").is_err());
        assert!(pack_spec("a.img,x").is_err());
        assert!(pack_spec("a.img,").is_err());
    }

    /// **A named port is bound as it stands and an unnamed one is where
    /// the search for a free display starts**, so which is which is what
    /// tells a refusal from a display one up.
    #[test]
    fn an_endpoint_names_a_port_or_leaves_it_to_the_default() {
        assert!(!names_a_port(None));
        assert!(!names_a_port(Some("0.0.0.0")));
        assert!(!names_a_port(Some("::1")));
        assert!(names_a_port(Some("5901")));
        assert!(names_a_port(Some("0")));
        assert!(names_a_port(Some("127.0.0.1:5901")));
        assert!(names_a_port(Some("[::1]:5900")));
    }

    /// **A display that is taken moves an unnamed port up and refuses a
    /// named one.** The port taken here is one the host picked and is
    /// held for the whole test, so nothing else on the machine is in the
    /// way of it.
    #[test]
    fn a_taken_display_moves_up_unless_its_port_was_named() {
        let held = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = held.local_addr().unwrap();
        if addr.port() == u16::MAX {
            return;
        }
        let named = TerminalAt { addr, port_named: true, asked: true };
        assert!(bind_terminal(named).is_err(), "a named port that is taken is not moved");
        let unnamed = TerminalAt { addr, port_named: false, asked: false };
        match bind_terminal(unnamed) {
            // The next display up, or one above that if the host had it
            // taken too.
            Ok(t) => assert!(t.addr().unwrap().port() > addr.port(), "the next free display"),
            Err(e) => panic!("no free display above {addr}: {e}"),
        }
    }

    #[test]
    fn an_endpoint_is_nothing_a_port_an_address_or_both() {
        let lo = |p| SocketAddr::from((Ipv4Addr::LOCALHOST, p));
        assert_eq!(endpoint(None, DEBUG_CABLE_PORT), Some(lo(7661)));
        assert_eq!(endpoint(None, TERMINAL_PORT), Some(lo(5900)));
        assert_eq!(endpoint(Some("5901"), TERMINAL_PORT), Some(lo(5901)));
        assert_eq!(endpoint(Some("0.0.0.0"), TERMINAL_PORT), "0.0.0.0:5900".parse().ok());
        assert_eq!(endpoint(Some("10.0.0.2:7000"), DEBUG_CABLE_PORT), "10.0.0.2:7000".parse().ok());
        assert_eq!(endpoint(Some("::1"), TERMINAL_PORT), "[::1]:5900".parse().ok());
        assert_eq!(endpoint(Some("70000"), TERMINAL_PORT), None);
        assert_eq!(endpoint(Some("nowhere"), TERMINAL_PORT), None);
        assert_eq!(endpoint(Some("nowhere:5900"), TERMINAL_PORT), None);
    }

    /// **`--serial`'s endpoint names its port or is refused.** The other
    /// endpoint flags have a default port to fall back on and this one has
    /// none, so a bare address --- which for them means "there, on the
    /// usual port" --- is not an endpoint here at all.
    #[test]
    fn the_serial_endpoint_is_a_port_or_an_address_and_a_port() {
        let lo = |p| SocketAddr::from((Ipv4Addr::LOCALHOST, p));
        assert_eq!(serial_endpoint("5962"), Some(lo(5962)));
        assert_eq!(serial_endpoint("0"), Some(lo(0)));
        assert_eq!(serial_endpoint("0.0.0.0:5962"), "0.0.0.0:5962".parse().ok());
        assert_eq!(serial_endpoint("[::1]:5962"), "[::1]:5962".parse().ok());
        assert_eq!(serial_endpoint("127.0.0.1"), None, "no port");
        assert_eq!(serial_endpoint("::1"), None, "no port");
        assert_eq!(serial_endpoint(""), None);
        assert_eq!(serial_endpoint("70000"), None);
        assert_eq!(serial_endpoint("nowhere"), None);
        assert_eq!(serial_endpoint("nowhere:5962"), None);
    }

    #[test]
    fn an_endpoint_against_a_default_keeps_what_is_not_said() {
        let base: SocketAddr = "10.0.0.2:5900".parse().unwrap();
        assert_eq!(endpoint_at(None, base), Some(base));
        assert_eq!(endpoint_at(Some("5901"), base), "10.0.0.2:5901".parse().ok());
        assert_eq!(endpoint_at(Some("0.0.0.0"), base), "0.0.0.0:5900".parse().ok());
        assert_eq!(endpoint_at(Some("127.0.0.1:7"), base), "127.0.0.1:7".parse().ok());
        assert_eq!(endpoint_at(Some("x"), base), None);
    }

    /// **A paced run waits when it is ahead of the machine's clock, and at
    /// no other time.** The arithmetic is [`Pace`]'s and the instants are
    /// passed in, so the rule is held here without a wall clock: what a run
    /// does with the answer --- sleep for it --- is the one line in each run
    /// loop that this does not reach.
    ///
    /// The cap is the responsiveness of the run: the terminal, the keyboard
    /// and the prompt are polled on the same loop, so a lead is waited off
    /// in frames rather than in one long sleep.
    #[test]
    fn the_pace_waits_only_when_the_run_is_ahead_of_the_machine() {
        assert!(PACE_SLEEP <= TERMINAL_INTERVAL, "a wait never outlasts a terminal poll");

        let t = Instant::now();
        let mut pace = Pace::new(t, 0);

        // 20 ms of the machine's time in 5 ms of the host's: 15 ms ahead,
        // and 15 ms is what it waits.
        assert_eq!(
            pace.owed(t + Duration::from_millis(5), 20_000_000),
            Some(Duration::from_millis(15)),
            "ahead by 15 ms"
        );

        // Exactly level: the two clocks agree, so there is nothing to wait
        // for.
        assert_eq!(pace.owed(t + Duration::from_millis(20), 20_000_000), None, "level");

        // Ahead by less than the floor: carried to the next check rather
        // than slept away in a wait the host would round up.
        assert_eq!(
            pace.owed(t + Duration::from_millis(20), 20_500_000),
            None,
            "half a millisecond ahead is carried"
        );

        // Further ahead than one wait: capped, and the rest is waited for
        // at the checks after this one.
        assert_eq!(
            pace.owed(t + Duration::from_millis(10), 60_000_000),
            Some(PACE_SLEEP),
            "50 ms ahead waits one frame of it"
        );
    }

    /// **A paced run that has fallen behind never runs the debt off.** It
    /// starts again from where it is: the anchor moves to now, and the
    /// next lead is measured from there. A run that stalled and then went
    /// at nine times speed to make the stall up would be worse than one
    /// that is simply late.
    ///
    /// Three ways to fall behind, one rule for all of them: a host that
    /// took longer over the microcycles than the machine would have, an
    /// engine slower than the hardware, which `chip` always is, and a run
    /// standing at the prompt with nothing running at all.
    #[test]
    fn the_pace_never_runs_off_a_debt() {
        let t = Instant::now();

        // A second of the host's time for 100 ms of the machine's: behind,
        // so nothing is waited for...
        let mut pace = Pace::new(t, 0);
        assert_eq!(pace.owed(t + Duration::from_secs(1), 100_000_000), None, "behind waits not");
        // ...and the 900 ms are gone with the anchor. From here the run is
        // paced as one that had never stalled: 20 ms of machine time in 5
        // ms of the host's is 15 ms ahead, not 15 ms less a debt.
        assert_eq!(
            pace.owed(t + Duration::from_millis(1005), 120_000_000),
            Some(Duration::from_millis(15)),
            "and the debt is not carried into the next wait"
        );

        // A long stall --- a minute of the host's clock and not one
        // microcycle --- is the same: nothing to wait for, nothing to make
        // up afterwards.
        let mut pace = Pace::new(t, 500_000_000);
        let stalled = t + Duration::from_secs(60);
        assert_eq!(pace.owed(stalled, 500_000_000), None, "a minute of stall waits not");
        assert_eq!(
            pace.owed(stalled + Duration::from_millis(2), 512_000_000),
            Some(Duration::from_millis(10)),
            "and the minute is not run off"
        );

        // Held at the prompt, where the run loops move the anchor rather
        // than measuring against it: ten seconds of somebody typing is not
        // ten seconds the machine owes when `continue` runs it on.
        let mut pace = Pace::new(t, 0);
        let typed = t + Duration::from_secs(10);
        pace.anchor(typed, 0);
        assert_eq!(
            pace.owed(typed + Duration::from_millis(1), 10_000_000),
            Some(Duration::from_millis(9)),
            "the hold is not a debt"
        );
    }
}
