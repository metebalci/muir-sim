// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Xbus and the Unibus, answered by [`Machine`]: the far end of the bus
//! interface board, which with the board and the cables is
//! [`crate::cable::FarEnd`], the far end of the processor board.
//!
//! The bus interface is the master of both buses here, and what it does on
//! them is MIT's: "You treat XBUSRQ as MSYN and XBUSACK as SSYN, Unibus-style
//! and completely asynchronous" (`busint.erface`), so both sides are the same
//! handshake. A request comes with the address and, for a write, the data;
//! this puts the data on the bus for a read, acknowledges, and holds both
//! until the request drops. "On a read, the leading edge of XBUSACK is what
//! clocks in the data", so the data goes on before the acknowledge. A device
//! that is not there does not answer, and the board's own timeout ends the
//! cycle.
//!
//! Every bus line is active low and open collector, pulled up by
//! [`crate::cable::busint_board`]; a one is put on a line by pulling it low,
//! a zero by letting go, which leaves the terminator's pull-up in place.
//! On the Unibus this drives the interface's nets. On the Xbus it asserts
//! onto [`crate::xbus::Xbus`], which is what carries the wires between the
//! interface and the memory boards, and main memory is theirs: this
//! answers the disk controller and the display and keeps its copy of
//! `main` current for the disk's DMA. With no boards on the backplane it
//! answers memory too, as it did before there were any. The board's own
//! Unibus block --- `766000` to `766176`, the diagnostic registers, the
//! interrupt and error status and the Unibus map,
//! [`crate::busint::register`] --- is answered on the board and not here.

use crate::busint::{self, Responder};
use crate::chip::Chip;
use crate::ioboard;
use crate::machine::Machine;
use crate::netlist::{NetId, Netlist};
use crate::part::Level;

/// The device's side of a Unibus interrupt, as the behavioral I/O board
/// runs it against the netlist bus interface --- the netlist I/O board
/// runs its own. The PDP-11 Unibus handshake, which the CADR's interface
/// takes as the PDP-11 would: the device asserts its bus request; the
/// arbiter grants; the device answers `SACK` and drops the request; with
/// the bus free it asserts `BBSY` and `INTR` with its vector on the data
/// lines and lets `SACK` go; the processor answers `SSYN`, and the device
/// takes everything back. On UBINTC, `INTR IN` is `LOCAL ENABLE` and the
/// received `INTR`, two clocks of the 74LS74s at 0D13 make `INTR SSYN`,
/// which is the `SSYN` sent back, presets `UB INT` at 0D15 while `ENABLE
/// UB INTS` is up, and clocks the vector off `UDI` into the 74LS374 at
/// 0D17.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Intr {
    /// No cycle. `served` says the board's present request has had its
    /// cycle, and waits for the request to drop before another: the
    /// request is a level, `KBD READY` until the register is read, and the
    /// interface's `INT STOPS GRANTS` would otherwise be the only thing
    /// between one cycle and the next.
    Idle { served: bool },
    /// `-UB BR5` is down; waiting for the grant on `UB BG5 IN`.
    Requesting,
    /// Granted: `-UB SACK` is down, for the vector the board was requesting
    /// with; waiting for the arbiter to take the grant back, and then for
    /// the bus to be free.
    Granted { vector: u16 },
    /// `-UB BBSY` and `-UB INTR` are down with the vector on the data
    /// lines; waiting for `-UB SSYN`.
    Interrupting,
    /// Everything let go; waiting for `-UB SSYN` to rise.
    Done,
}

impl Intr {
    const IDLE: Intr = Intr::Idle { served: false };
}

pub struct Buses {
    pub machine: Machine,
    /// Memory boards are on the backplane, and main memory is theirs.
    pub memory_boards: bool,
    /// With no boards on the backplane, whether a memory cycle is answered
    /// when a board's clock would answer it, through the same
    /// [`busint::MemoryBoard`] twins `rtl` runs, one a board --- `chip`
    /// with `--main-memory model` --- or at once, as a device that answers
    /// in no time of its own, which is what the interface harness in
    /// `tests/busint_netlist.rs` measures the interface against.
    pub memory_twins: bool,
    /// The twins, when [`Buses::memory_twins`]: clocked from `-XBUS SYNC`,
    /// reset over `-XBUS INIT`, asked at `-XBUS RQ` and told when it
    /// lifts, as the boards are.
    pub memory: Vec<busint::MemoryBoard>,
    xsync: NetId,
    xinit: NetId,
    xsync_was: bool,
    xinit_was: bool,
    /// A memory cycle seen and not yet answered: when the twin will, the
    /// address, whether it is a write, and the board.
    xbus_pending: Option<(u64, u32, bool, u8)>,
    /// Whether the I/O board is a netlist on the Unibus, answering its own
    /// registers; else they are answered here from the machine.
    pub io_board: bool,
    /// Whether the display is a netlist on the backplane, answering its
    /// frame buffer and mode register itself. Writes are mirrored into
    /// the machine's model either way, so `machine.tv` is the
    /// picture whichever board drew it.
    pub tv_board: bool,
    /// The same for the color TV, the second display board: whether it is
    /// a netlist on the backplane rather than the model answering
    /// `17200000` and `17377750`. Only ever true where
    /// `machine.color_tv` is fitted, the model beside the board being
    /// where the color picture is read off, and a machine with no second
    /// screen at all has neither.
    pub color_tv_board: bool,
    /// Whether the disk controller is a netlist on the backplane. Then its
    /// registers are its own and the machine's model is not consulted:
    /// the netlist reads and writes the pack through the drive on its
    /// cable, [`crate::xbus::Xbus::plug`], as a bus master of its own.
    pub disk_board: bool,
    /// What this holds low on the Xbus, indexed by the interface's net.
    /// A vector and not a map: the backplane asks about every wire at
    /// every exchange.
    asserting: Vec<Option<Level>>,
    xrq: NetId,
    xack: NetId,
    xwr: NetId,
    xintr: NetId,
    xignpar: NetId,
    /// `-XADDR0..21`, bit 0 first.
    xaddr: Vec<NetId>,
    /// `-XBUS0..31`, bit 0 first.
    xdata: Vec<NetId>,
    msyn: NetId,
    ssyn: NetId,
    c1: NetId,
    /// The Unibus interrupt wires, for the behavioral I/O board's
    /// interrupt cycle: its request, the interface's grant, and the
    /// device's `SACK`, `BBSY` and `INTR`.
    br5: NetId,
    bg5: NetId,
    sack: NetId,
    bbsy: NetId,
    intr: NetId,
    /// Where the behavioral I/O board's interrupt cycle is.
    interrupting: Intr,
    /// `-UB ADR0..17`, bit 0 first.
    ubaddr: Vec<NetId>,
    /// `-UBD0..15`, bit 0 first.
    ubdata: Vec<NetId>,
    /// An Xbus cycle this has answered, and whether it drove the data.
    xbus_answered: Option<bool>,
    unibus_answered: Option<bool>,
    /// A word still on the Unibus after `-UB MSYN` dropped. The board loads
    /// `MD` on the edge that drops `MSYN`, and a slave that took its data
    /// away in the same instant would beat the strobe --- on the real bus
    /// the negation has to travel to the slave first. So the word is held
    /// for one more step, and let go at the next.
    unibus_holding: bool,
    /// `-UB INIT`, whose release starts the I/O board's microsecond clock.
    /// The I/O board's answer times, the same twin `rtl` runs.
    io_timing: busint::IoBoardTiming,
    /// A Unibus cycle seen and not yet answered: when the I/O board will,
    /// and when `-MSYN` was seen, which is when its counter is read.
    unibus_pending: Option<(u64, u64)>,
}

impl Buses {
    pub fn new(n: &Netlist, machine: Machine) -> Buses {
        let net = |name: &str| {
            n.by_name_id(name)
                .or_else(|| n.by_name_id(&format!("'{name}'")))
                .unwrap_or_else(|| panic!("no net {name}"))
        };
        let bus =
            |prefix: &str, bits: u32| (0..bits).map(|b| net(&format!("{prefix}{b}"))).collect();
        let boards = machine.main.len() >> 16;
        Buses {
            machine,
            memory_boards: false,
            memory_twins: false,
            memory: vec![busint::MemoryBoard::default(); boards],
            xsync: net("-XBUS SYNC"),
            xinit: net("-XBUS INIT"),
            xsync_was: false,
            xinit_was: false,
            xbus_pending: None,
            io_board: false,
            tv_board: false,
            color_tv_board: false,
            disk_board: false,
            asserting: vec![None; n.nets.len()],
            xrq: net("-XBUS RQ"),
            xack: net("-XBUS ACK"),
            xwr: net("-XBUS WR"),
            xintr: net("-XBUS INTR"),
            xignpar: net("-XBUS IGNPAR"),
            xaddr: bus("-XADDR", 22),
            xdata: bus("-XBUS", 32),
            msyn: net("-UB MSYN"),
            ssyn: net("-UB SSYN"),
            c1: net("-UB C1"),
            br5: net("-UB BR5"),
            bg5: net("UB BG5 IN"),
            sack: net("-UB SACK"),
            bbsy: net("-UB BBSY"),
            intr: net("-UB INTR"),
            interrupting: Intr::IDLE,
            ubaddr: bus("-UB ADR", 18),
            ubdata: bus("-UBD", 16),
            xbus_answered: None,
            unibus_answered: None,
            unibus_holding: false,
            io_timing: busint::IoBoardTiming::default(),
            unibus_pending: None,
        }
    }

    /// Whether an Xbus address is a netlist display board's: its frame
    /// buffer or its control registers.
    ///
    /// Either strap, and only a board that is on the backplane: the main
    /// screen's board under `--tv netlist` and the second display board's
    /// under `--color-tv netlist`, each answering its own two blocks.
    /// Where a display is the model instead, its addresses fall through to
    /// the machine behind the buses.
    fn is_display(&self, phys: u32) -> bool {
        (self.tv_board && crate::tv::NORMAL_TV.answers(phys))
            || (self.color_tv_board && crate::tv::COLOR_TV.answers(phys))
    }

    /// Whether a device board on the backplane answers this address, so
    /// that the model must not.
    fn device_board_answers(&self, phys: u32) -> bool {
        self.is_display(phys)
            || (self.disk_board && crate::disk_controller::register(phys).is_some())
    }

    /// The board is asserting an active-low line: its own drivers hold it
    /// low, whatever this has put on it.
    fn asserted(c: &Chip, net: NetId) -> bool {
        c.board_level(net) == (Level::Low, true)
    }

    /// A word off active-low lines, as the board is driving them.
    fn word(c: &Chip, lines: &[NetId]) -> u32 {
        !(c.read(lines) as u32) & ((1u64 << lines.len()) - 1) as u32
    }

    fn put(c: &mut Chip, lines: &[NetId], word: u32) {
        for (b, &net) in lines.iter().enumerate() {
            if (word >> b) & 1 != 0 {
                c.drive(net, Level::Low);
            } else {
                c.pull_up(net);
            }
        }
    }

    fn take_back(c: &mut Chip, lines: &[NetId]) {
        for &net in lines {
            c.pull_up(net);
        }
    }

    /// When the I/O board, answered from here, will put `-UB SSYN` up: an
    /// event the far end steps to.
    pub fn next_event(&self) -> Option<u64> {
        [self.unibus_pending.map(|(at, _)| at), self.xbus_pending.map(|(at, ..)| at)]
            .into_iter()
            .flatten()
            .min()
    }

    /// What this holds an Xbus wire at, if it holds it at all.
    pub fn asserting(&self, net: NetId) -> Option<Level> {
        self.asserting[net as usize]
    }

    fn assert_word(&mut self, lines: &[NetId], word: u32) {
        for (b, &net) in lines.iter().enumerate() {
            self.asserting[net as usize] = ((word >> b) & 1 != 0).then_some(Level::Low);
        }
    }

    fn release(&mut self, lines: &[NetId]) {
        for &net in lines {
            self.asserting[net as usize] = None;
        }
    }

    /// Puts a word on the data lines, and takes it off: the lines are
    /// lent out rather than cloned, since this is every read answered.
    fn assert_data(&mut self, word: u32) {
        let xdata = std::mem::take(&mut self.xdata);
        self.assert_word(&xdata, word);
        self.xdata = xdata;
    }

    fn release_data(&mut self) {
        let xdata = std::mem::take(&mut self.xdata);
        self.release(&xdata);
        self.xdata = xdata;
    }

    /// Answers the board at `now`, and says whether anything on the buses
    /// changed.
    pub fn tick(&mut self, c: &mut Chip, now: u64) -> bool {
        let mut changed = false;

        // The memory twins' clock, off the interface's net: the rising edge
        // of `-XBUS SYNC` is the microcycle's instant, where `rtl` clocks
        // its twins (measured: it rises at 12,980, 13,200, 13,420 ns in the
        // boot, `rtl`'s microcycles 56, 57, 58, and falls 60 ns before
        // each).
        if self.memory_twins {
            let sync = Self::asserted(c, self.xsync);
            if !sync && self.xsync_was {
                for b in &mut self.memory {
                    b.xbus_clock(now);
                }
            }
            self.xsync_was = sync;
        }
        // `-XBUS INIT`, off the interface's net: the twins are held by it,
        // and the model boards behind this end reset as it is asserted, as
        // the netlist boards on the backplane do --- each what its own
        // reset pin clears, [`Machine::bus_reset`].
        let init = Self::asserted(c, self.xinit);
        if init != self.xinit_was {
            if self.memory_twins {
                for b in &mut self.memory {
                    b.unibus_reset(now, init);
                }
            }
            if init {
                self.machine.ns = now;
                self.machine.bus_reset();
            }
            self.xinit_was = init;
        }

        // The Xbus.
        let rq = Self::asserted(c, self.xrq);
        match (rq, self.xbus_answered) {
            (true, None) => {
                let phys = Self::word(c, &self.xaddr);
                let write = Self::asserted(c, self.xwr);
                let responder = busint::decode_with(
                    phys,
                    self.machine.main.len(),
                    self.machine.color_tv.is_some(),
                );
                if matches!(responder, Responder::Memory(_)) && self.memory_boards {
                    // The boards answer. A write is mirrored into `main`,
                    // which is what the disk controller's DMA reads.
                    if write {
                        self.machine.main[phys as usize] = Self::word(c, &self.xdata).into();
                    }
                    self.xbus_answered = Some(false);
                } else if let Responder::Memory(k) = responder
                    && self.memory_twins
                {
                    // Answered when the board's clock says: the twin takes
                    // the request at its next edge after it and answers
                    // eleven stages on, refresh first when both wait.
                    let (at, ..) = *self.xbus_pending.get_or_insert_with(|| {
                        (self.memory[k as usize].request(now), phys, write, k)
                    });
                    if now >= at {
                        self.machine.ns = now;
                        if write {
                            self.machine.bus_write(phys, Self::word(c, &self.xdata).into());
                        } else {
                            // The CADR's Xbus carries 32 bits.
                            let data = self.machine.bus_read(phys) as u32;
                            self.assert_data(data);
                        }
                        self.asserting[self.xignpar as usize] = Some(Level::Low);
                        self.asserting[self.xack as usize] = Some(Level::Low);
                        self.xbus_answered = Some(!write);
                        self.xbus_pending = None;
                        changed = true;
                    }
                } else if responder == Responder::Device && self.device_board_answers(phys) {
                    // A device board on the backplane answers. A write to a
                    // display is mirrored into the model, so that the
                    // screen can be read off it --- the main screen into
                    // `machine.tv` and the color one into
                    // `machine.color_tv`, which `Machine::bus_write` picks
                    // by the address.
                    if write && self.is_display(phys) {
                        self.machine.ns = now;
                        self.machine.bus_write(phys, Self::word(c, &self.xdata).into());
                    }
                    self.xbus_answered = Some(false);
                } else if matches!(responder, Responder::Memory(_) | Responder::Device) {
                    self.machine.ns = now;
                    if write {
                        self.machine.bus_write(phys, Self::word(c, &self.xdata).into());
                    } else {
                        // The CADR's Xbus carries 32 bits.
                        let data = self.machine.bus_read(phys) as u32;
                        self.assert_data(data);
                    }
                    // "Should it not be convenient for the device to produce
                    // parity ... the device may assert -XBUS.IGNPAR": these
                    // devices hold none.
                    self.asserting[self.xignpar as usize] = Some(Level::Low);
                    self.asserting[self.xack as usize] = Some(Level::Low);
                    self.xbus_answered = Some(!write);
                    changed = true;
                }
            }
            (false, Some(drove)) => {
                self.asserting[self.xack as usize] = None;
                self.asserting[self.xignpar as usize] = None;
                if drove {
                    self.release_data();
                }
                self.xbus_answered = None;
                changed = true;
                // The request lifted: the twin's idle waits for it.
                if self.memory_twins {
                    let phys = Self::word(c, &self.xaddr);
                    if let Responder::Memory(k) = busint::decode(phys, self.machine.main.len()) {
                        self.memory[k as usize].released(now);
                    }
                }
            }
            (false, None) => self.xbus_pending = None,
            _ => {}
        }

        // The Unibus, with the board as master.
        if self.unibus_holding {
            Self::take_back(c, &self.ubdata);
            self.unibus_holding = false;
            changed = true;
        }
        let msyn = Self::asserted(c, self.msyn);
        match (msyn, self.unibus_answered) {
            (true, None) => {
                let u = Self::word(c, &self.ubaddr);
                let phys = busint::unibus_physical(u);
                let write = Self::asserted(c, self.c1);
                // The I/O board's registers, answered when the board would
                // answer them: `-MSYN` seen now, `-SSYN` at the twin's time.
                if let Some(r) = ioboard::answers(u, write)
                    && busint::register(u).is_none()
                    && !self.io_board
                {
                    let (at, msyn_at) = *self
                        .unibus_pending
                        .get_or_insert_with(|| (self.io_timing.answer_cycle(r, write, now), now));
                    if now >= at {
                        // The counter's low half is the count at `-MSYN`.
                        self.machine.ns = if r == ioboard::USEC_LOW { msyn_at } else { now };
                        if write {
                            self.machine.bus_write(phys, Self::word(c, &self.ubdata).into());
                        } else {
                            let data = self.machine.bus_read(phys) as u32 & 0xffff;
                            Self::put(c, &self.ubdata, data);
                        }
                        c.drive(self.ssyn, Level::Low);
                        self.unibus_answered = Some(!write);
                        self.unibus_pending = None;
                        changed = true;
                    }
                }
            }
            (false, Some(drove)) => {
                c.pull_up(self.ssyn);
                self.unibus_holding = drove;
                self.unibus_answered = None;
                changed = true;
            }
            (false, None) => self.unibus_pending = None,
            _ => {}
        }

        // The Unibus, with the behavioral I/O board as an interrupting
        // device. Not when the netlist board is on the backplane: that
        // board pulls `-BR*` and puts its vector on the bus itself.
        if !self.io_board {
            changed |= self.interrupt_cycle(c, now);
        }

        // The Xbus interrupt line: the disk controller's request, and the
        // behavioral displays' vertical interrupts --- a netlist display
        // board drives the wire itself, so the model beside it must not,
        // and that goes for the second display board as for the first.
        // The I/O board's is a Unibus one, and goes by the cycle above or
        // by the netlist board's own.
        self.machine.disk.advance(now);
        let vertical = (!self.tv_board && self.machine.tv.interrupt(now))
            || (!self.color_tv_board
                && self.machine.color_tv.as_ref().is_some_and(|tv| tv.interrupt(now)));
        let want = if self.machine.disk.interrupt() || vertical { Some(Level::Low) } else { None };
        if self.asserting(self.xintr) != want {
            self.asserting[self.xintr as usize] = want;
            changed = true;
        }
        changed
    }

    /// One step of the behavioral I/O board's interrupt cycle
    /// ([`Intr`]); returns whether a wire moved.
    fn interrupt_cycle(&mut self, c: &mut Chip, now: u64) -> bool {
        let request = self.machine.ioboard.interrupt_request(now);
        let next = match self.interrupting {
            Intr::Idle { served } => match request {
                Some(_) if !served => {
                    c.drive(self.br5, Level::Low);
                    Intr::Requesting
                }
                Some(_) => return false,
                None => {
                    if served {
                        Intr::IDLE
                    } else {
                        return false;
                    }
                }
            },
            Intr::Requesting => match request {
                None => {
                    // Read before it was granted: nothing to say after all.
                    c.pull_up(self.br5);
                    Intr::IDLE
                }
                Some(vector) if !Self::asserted(c, self.bg5) => {
                    // Granted: the 74S38 at UBINTC 0F13 has let the grant
                    // line go. Take it with `SACK`, and drop the request.
                    c.drive(self.sack, Level::Low);
                    c.pull_up(self.br5);
                    Intr::Granted { vector }
                }
                Some(_) => return false,
            },
            Intr::Granted { vector } => {
                // `SACK` is held until the arbiter has seen it and taken
                // the grant back --- the 74S38 pulling the line low again
                // --- and the bus is the device's once nobody holds `BBSY`
                // or `SSYN`. Then the interrupt goes out with the vector,
                // and `SACK` is let go.
                //
                // The vector is the one the grant was taken for. A request
                // that has dropped since --- the processor's own read of
                // the keyboard register clearing `KBD READY` while the bus
                // was busy --- has been granted all the same, and the
                // cycle goes through with the board's vector rather than
                // with none. The board keeps a copy: the 74S175 at IOBINT
                // 0F14 takes `KBD/MOUSE.IREQ`, `SER.IREQ`, `CHAOS.IREQ` and
                // `CLOCK.IREQ` on its `D` pins and clocks them on `BG.IN`,
                // the grant arriving, and it is `V2` and `V3` off that
                // latch --- through the 74LS00s at 0E12 and the 74S260 at
                // 0E14 --- that the 74S38s at 0F16 and 0F15 put on `-D2*`
                // and `-D3*` under `MASTER B`, beside the `-D4*`, `-D5*`
                // and `-D7*` those gates hold up for every vector, which is
                // `ioboard::KBD_VECTOR` as the base. The latch is
                // cleared only by `-CLR IREQ`, the 74S08 at 0E10 taking
                // `-INTR ACK` and `-RESET`, so the live request lines are
                // not looked at again between the grant and the vector.
                if !Self::asserted(c, self.bg5)
                    || Self::asserted(c, self.bbsy)
                    || Self::asserted(c, self.ssyn)
                {
                    return false;
                }
                let vector = vector as u32;
                c.drive(self.bbsy, Level::Low);
                Self::put(c, &self.ubdata, vector);
                c.drive(self.intr, Level::Low);
                c.pull_up(self.sack);
                Intr::Interrupting
            }
            Intr::Interrupting => {
                // `INTR SSYN`, the interface's acknowledgement, on `-UB
                // SSYN`: the vector has been taken. Everything back.
                if !Self::asserted(c, self.ssyn) {
                    return false;
                }
                c.pull_up(self.intr);
                Self::take_back(c, &self.ubdata);
                c.pull_up(self.bbsy);
                Intr::Done
            }
            Intr::Done => {
                if Self::asserted(c, self.ssyn) {
                    return false;
                }
                Intr::Idle { served: true }
            }
        };
        self.interrupting = next;
        true
    }
}

// --- Checkpoints ------------------------------------------------------------

impl Intr {
    fn save(self, w: &mut crate::checkpoint::Writer) {
        match self {
            Intr::Idle { served } => {
                w.u8(0);
                w.bool(served);
            }
            Intr::Requesting => w.u8(1),
            Intr::Granted { vector } => {
                w.u8(2);
                w.u16(vector);
            }
            Intr::Interrupting => w.u8(3),
            Intr::Done => w.u8(4),
        }
    }

    fn load(r: &mut crate::checkpoint::Reader) -> std::io::Result<Intr> {
        Ok(match r.u8()? {
            0 => Intr::Idle { served: r.bool()? },
            1 => Intr::Requesting,
            2 => Intr::Granted { vector: r.u16()? },
            3 => Intr::Interrupting,
            4 => Intr::Done,
            v => return Err(crate::checkpoint::bad(format!("{v} for the interrupt cycle"))),
        })
    }
}

impl Buses {
    /// What is behind the buses into a checkpoint: the machine's model,
    /// the memory twins, and the cycle in flight on either bus.  The
    /// boards are not here --- they are netlists and save themselves,
    /// [`crate::cable::FarEnd::save`] --- and neither are the nets, which
    /// are the netlist's and come back with it.
    ///
    /// Which boards are behind the buses and which are netlists is written
    /// first and held to at the load, because it says what the rest means:
    /// a checkpoint whose I/O board answered here would put its registers
    /// nowhere on a machine whose board is a netlist.
    pub fn save(&self, w: &mut crate::checkpoint::Writer) {
        let Buses {
            machine,
            memory_boards,
            memory_twins,
            memory,
            xsync: _,
            xinit: _,
            xsync_was,
            xinit_was,
            xbus_pending,
            io_board,
            tv_board,
            color_tv_board,
            disk_board,
            asserting,
            xrq: _,
            xack: _,
            xwr: _,
            xintr: _,
            xignpar: _,
            xaddr: _,
            xdata: _,
            msyn: _,
            ssyn: _,
            c1: _,
            br5: _,
            bg5: _,
            sack: _,
            bbsy: _,
            intr: _,
            interrupting,
            ubaddr: _,
            ubdata: _,
            xbus_answered,
            unibus_answered,
            unibus_holding,
            // The I/O board's answer times hold nothing: the board's clock
            // runs from power-on and every time it gives is worked out
            // from the instant it is asked.
            io_timing: _,
            unibus_pending,
        } = self;
        w.bool(*memory_boards);
        w.bool(*memory_twins);
        w.bool(*io_board);
        w.bool(*tv_board);
        w.bool(*color_tv_board);
        w.bool(*disk_board);
        machine.save(w);
        w.u64(memory.len() as u64);
        for m in memory {
            m.save(w);
        }
        w.bool(*xsync_was);
        w.bool(*xinit_was);
        w.opt(*xbus_pending, |w, (at, addr, write, board)| {
            w.u64(at);
            w.u32(addr);
            w.bool(write);
            w.u8(board);
        });
        w.u64(asserting.len() as u64);
        for &l in asserting {
            w.opt(l, crate::checkpoint::Writer::level);
        }
        interrupting.save(w);
        w.opt(*xbus_answered, crate::checkpoint::Writer::bool);
        w.opt(*unibus_answered, crate::checkpoint::Writer::bool);
        w.bool(*unibus_holding);
        w.opt(*unibus_pending, |w, (at, msyn)| {
            w.u64(at);
            w.u64(msyn);
        });
    }

    /// Back from a checkpoint, onto buses built as the flags say: the same
    /// boards behind them, and the same machine under it --- the pack the
    /// drive holds and the Chaosnet on the cable are the flags' again, as
    /// they are on the other two engines.
    pub fn load(&mut self, r: &mut crate::checkpoint::Reader) -> std::io::Result<()> {
        let bad = crate::checkpoint::bad;
        for (what, is) in [
            ("main memory on the backplane", &mut self.memory_boards),
            ("the memory twins", &mut self.memory_twins),
            ("an I/O board netlist", &mut self.io_board),
            ("a display netlist", &mut self.tv_board),
            ("a color TV netlist", &mut self.color_tv_board),
            ("a disk controller netlist", &mut self.disk_board),
        ] {
            let was = r.bool()?;
            if was != *is {
                return Err(bad(format!(
                    "{what}: {} in the checkpoint, {} here",
                    if was { "yes" } else { "no" },
                    if *is { "yes" } else { "no" }
                )));
            }
        }
        self.machine.load(r)?;
        let n = r.u64()? as usize;
        if n != self.memory.len() {
            return Err(bad(format!(
                "{n} memory twins, and these buses have {}",
                self.memory.len()
            )));
        }
        for m in self.memory.iter_mut() {
            *m = busint::MemoryBoard::load(r)?;
        }
        self.xsync_was = r.bool()?;
        self.xinit_was = r.bool()?;
        self.xbus_pending = r.opt(|r| Ok((r.u64()?, r.u32()?, r.bool()?, r.u8()?)))?;
        let n = r.u64()? as usize;
        if n != self.asserting.len() {
            return Err(bad(format!(
                "{n} nets held on the Xbus, and this interface has {}",
                self.asserting.len()
            )));
        }
        for a in self.asserting.iter_mut() {
            *a = r.opt(crate::checkpoint::Reader::level)?;
        }
        self.interrupting = Intr::load(r)?;
        self.xbus_answered = r.opt(crate::checkpoint::Reader::bool)?;
        self.unibus_answered = r.opt(crate::checkpoint::Reader::bool)?;
        self.unibus_holding = r.bool()?;
        self.unibus_pending = r.opt(|r| Ok((r.u64()?, r.u64()?)))?;
        Ok(())
    }
}
