// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The CADR clock generator.
//!
//! Pages CLOCK1 and CLOCK2 are the one part of the board that cannot be
//! levelized: they contain four analog delay lines and three cross-coupled
//! NAND pairs, and the two remaining combinational cycles in the whole design
//! are both here. So the clock gets its own model behind a trait, and the rest
//! of the machine never has to know about nanoseconds.
//!
//! [`Behavioral`] is the fast one, driven by the tap table below. A
//! structural model built from the delay lines and SR latches can be added
//! later behind the same trait, to check this one rather than replace it.
//!
//! Everything here is read off the netlist. `CLOCKD` is *not* modeled: it is
//! pure distribution, 74S37 buffers and 74S04A inverters fanning `CLK1..CLK5`
//! out to `CLK1A`, `CLK2A..C`, `CLK3A..G` and `CLK4A..F`, and it stays
//! structural. So do the 7428 buffers on CLOCK2.

/// What the generator needs from the rest of the board.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Inputs {
    /// `MACHRUN` from OLORD1. With this low the clock runs but `-CLK0` is
    /// held off, so the machine stops without the generator stopping.
    pub machrun: bool,
    /// `-HANG` from VCTL1, active low: a memory cycle is stalling the machine.
    pub hang: bool,
    /// `-ILONG` from FLAG, active low. Stretches the read phase by 40 ns.
    pub ilong: bool,
    /// `SSPEED1`, `SSPEED0` from OLORD1, as a two-bit value.
    pub speed: Speed,
    pub reset: bool,
}

/// The mode register's two speed bits, `ir.bits` naming.
///
/// A microcycle at each, with no `ILONG`: **Fast 135, Normal 145, Slow 160,
/// ExtraSlow 220** nanoseconds.  The names are MIT's and the spacing is not
/// even --- `Slow` is fifteen nanoseconds off `Normal` and `ExtraSlow` is a
/// long way below all three --- because they name delay-line taps and not
/// intervals: the 74S151 at CLOCK1 1D08 selects `-TPR75`, `-TPR85`,
/// `-TPR100` and `-TPR160`, and each cycle is its tap plus the sixty
/// nanoseconds of restart after it.  [`Speed::read_phase_ns`] has the tap
/// table in full, `ILONG` included.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Speed {
    ExtraSlow = 0,
    Slow = 1,
    #[default]
    Normal = 2,
    Fast = 3,
}

/// The three master signals CLOCK2 produces. Everything else on the clock
/// pages is buffering of these.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Outputs {
    /// The read phase. `-CLK0` is `TPCLK AND MACHRUN`, and `CLK1..CLK5`
    /// follow it.
    pub tpclk: bool,
    /// The write pulse, buffered into `-WP1..-WP4`.
    pub tpwp: bool,
    /// The tri-state enable, buffered into `-TSE1..-TSE4`.
    pub tptse: bool,
    /// The control store write pulse, `-WP5`.
    pub tpwpiram: bool,
}

pub trait Clock {
    /// Advances to the next transition of any output, returning how many
    /// nanoseconds passed.
    fn advance(&mut self, inputs: Inputs) -> (u32, Outputs);
    /// When the next transition is due, or `None` while `-HANG` or `RESET`
    /// holds the generator at `-TPR0`.
    fn next_at(&self, inputs: Inputs) -> Option<u64>;
    /// Time passes to `until` with no transition. Held, the generator is
    /// stopped and its next transition moves with it; not held, the
    /// transition is simply not due yet.
    fn pass(&mut self, until: u64, held: bool);
    /// Nanoseconds since reset.
    fn time_ns(&self) -> u64;
    /// Where in the cycle we are, in nanoseconds from `-TPR0`.
    fn phase_ns(&self) -> u32;
}

impl Speed {
    /// The whole microcycle: the read phase plus the sixty nanoseconds the
    /// restart takes.
    ///
    /// The cycle restarts at `-TPDONE`, which is `-TPW60`, so the period is
    /// [`Speed::read_phase_ns`] plus `RESTART_AFTER_READ_NS`. Engines that
    /// have no nets to settle want this and not the event stream: it is the
    /// only thing the clock tells them that they cannot work out themselves.
    pub fn cycle_ns(self, ilong: bool) -> u32 {
        self.read_phase_ns(ilong) + RESTART_AFTER_READ_NS
    }

    /// The delay-line tap that ends the read phase.
    ///
    /// Straight off the 74S151 at CLOCK1 1D08, which selects on
    /// `{SSPEED1, SSPEED0, -ILONG}`:
    ///
    /// | select | tap | |
    /// |---|---|---|
    /// | 111 | `-TPR75` | fast |
    /// | 110 | `-TPR115` | fast, ILONG |
    /// | 101 | `-TPR85` | normal |
    /// | 100 | `-TPR125` | normal, ILONG |
    /// | 011 | `-TPR100` | slow |
    /// | 010 | `-TPR140` | slow, ILONG |
    /// | 001 | `-TPR160` | extra slow |
    /// | 000 | `-TPR160` | extra slow, ILONG |
    ///
    /// ILONG adds exactly 40 ns except at extra slow, where the tap is
    /// already the longest the chain provides.
    pub fn read_phase_ns(self, ilong: bool) -> u32 {
        match (self, ilong) {
            (Speed::Fast, false) => 75,
            (Speed::Fast, true) => 115,
            (Speed::Normal, false) => 85,
            (Speed::Normal, true) => 125,
            (Speed::Slow, false) => 100,
            (Speed::Slow, true) => 140,
            (Speed::ExtraSlow, _) => 160,
        }
    }
}

/// Whose time `rtl` keeps: the CADR's own, or the 10 ns grid muir-fpga's
/// fabric runs on. `--timing-model` chooses; `chip` and `micro` keep the
/// board's time whatever it says.
///
/// **The grid's rules are muir-fpga's**, stated so that references taken
/// from `rtl` come out as its fabric runs. A delay that something starts
/// is rounded up on its own, from its own start, to the next tick
/// ([`TimingModel::triggered`]). A clock that runs freely from power-on
/// keeps its phase exactly, and each edge is acted on at the first tick
/// at or after it ([`TimingModel::free_running`]), which is rounding that
/// never drifts. The microcycle is the read phase's tap rounded up, the
/// `ILONG` taps as one sum, and the restart after it rounded up on its own
/// ([`TimingModel::cycle_ns`]). `tests/timing_model.rs` holds muir to
/// those rules. **Unverified:** that the fabric keeps them, which its
/// source on the 10 ns grid, not yet published, or a reference
/// regenerated from `rtl` under `fpga` and passed by the fabric, would
/// settle.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TimingModel {
    /// The board's own nanoseconds.
    #[default]
    Cadr,
    /// muir-fpga's grid: every instant on a [`GRID_NS`] tick.
    Fpga,
    /// QUUX's synchronous microcycle, H1a: the grid, with a microcycle of
    /// `cycle_ticks` ticks and `ilong_ticks` more for an `ILONG`
    /// instruction, in place of the CADR's delay-line taps. Every register
    /// is still clocked at the one edge, the late writes one edge later,
    /// and the bus keeps its own time on the grid, so only the length of a
    /// microcycle changes. The ticks are a board's: the fit that proves its
    /// longest path settles in them. QUUX's, having no speed bits; `rtl`'s.
    Sync { cycle_ticks: u8, ilong_ticks: u8 },
}

/// `sync`'s microcycle when `--sync-cycle-ticks` does not say: 4 ticks,
/// 40 ns, the DE25-Nano's. The Arty Z7-20 runs revision 13 at 5: its
/// two-level map through the memory path's decode misses 4 ticks there by
/// about a nanosecond (muir-fpga's fit at its commit `ccc3d12`).
pub const SYNC_CYCLE_TICKS: u8 = 4;

/// The tick of muir-fpga's grid.
pub const GRID_NS: u64 = 10;

impl TimingModel {
    /// The name `--timing-model` takes.
    pub fn name(self) -> &'static str {
        match self {
            TimingModel::Cadr => "cadr",
            TimingModel::Fpga => "fpga",
            TimingModel::Sync { .. } => "sync",
        }
    }

    /// The model `--timing-model` names, if it names one.
    pub fn parse(word: &str) -> Option<TimingModel> {
        match word {
            "cadr" => Some(TimingModel::Cadr),
            "fpga" => Some(TimingModel::Fpga),
            "sync" => Some(TimingModel::Sync { cycle_ticks: SYNC_CYCLE_TICKS, ilong_ticks: 0 }),
            _ => None,
        }
    }

    /// A delay `ns` long that something starts: on the grid, the next
    /// tick at or past it, counted from its own start.
    pub fn triggered(self, ns: u64) -> u64 {
        self.tick(ns)
    }

    /// An edge of a clock that runs freely from power-on, whose exact
    /// instant is `at`: on the grid, the first tick at or after it. The
    /// phase is kept, so the next edge is reckoned from the clock's own
    /// period and not from this.
    pub fn free_running(self, at: u64) -> u64 {
        self.tick(at)
    }

    fn tick(self, ns: u64) -> u64 {
        match self {
            TimingModel::Cadr => ns,
            TimingModel::Fpga | TimingModel::Sync { .. } => ns.div_ceil(GRID_NS) * GRID_NS,
        }
    }

    /// The whole microcycle: [`Speed::cycle_ns`] on the board; on the grid,
    /// the tap rounded up and the restart rounded up after it.
    pub fn cycle_ns(self, speed: Speed, ilong: bool) -> u32 {
        if let TimingModel::Sync { cycle_ticks, ilong_ticks } = self {
            let ticks = cycle_ticks as u32 + if ilong { ilong_ticks as u32 } else { 0 };
            return ticks * GRID_NS as u32;
        }
        let read = self.triggered(speed.read_phase_ns(ilong) as u64);
        let restart = self.triggered(RESTART_AFTER_READ_NS as u64);
        (read + restart) as u32
    }
}

/// `TPTSE` is *cleared* at `-TPR5` and *set* at `-TPR25`, so it is asserted
/// for almost the whole cycle and drops for twenty nanoseconds at the start
/// of each one.
///
/// This was the other way round here, and it was wrong. The pair at CLOCK2
/// 1C06 and 1C07 is a NAND latch:
///
/// ```text
/// TPTSE  = NAND(-TPTSE, -TPR25, '-CLOCK RESET B')
/// -TPTSE = NAND(-TPR5, TPTSE)
/// ```
///
/// A low on `-TPR25` forces `TPTSE` high; a low on `-TPR5` forces `-TPTSE`
/// high and so `TPTSE` low. Set and reset, not on and off in the order the
/// tap numbers suggest.
///
/// It matters because `-TSE1..4` are `NOT TPTSE`, and they enable every
/// tri-state driver on the M and A buses. Asserted for only twenty
/// nanoseconds, the buses float for the rest of the cycle, an undriven TTL
/// input reads high, and every jump condition read off the shifter comes out
/// true.
const TSE_OFF_NS: u32 = 5;
const TSE_ON_NS: u32 = 25;

/// `TPWP` is set at `-TPW30` and cleared at `-TPW70`, both measured from
/// `-TPREND`: the TD50 at 1C12 gives `-TPW10..-TPW50`, and the TD25 at 1C14
/// hangs off `-TPW50` to give `-TPW55..-TPW75`. Read off the NAND latch at
/// CLOCK2 1C06/1C07, which `-TPW30` sets and `-TPW70` clears.
///
/// **The pulse is cut short at the cycle boundary, ten nanoseconds early.**
/// `-TPDONE` is `-TPW60`, so the next `-TPR0` asserts while the write pulse
/// is still on and the two overlap in the real machine. What keeps that from
/// writing the wrong word is propagation delay: `-TPR0` reaches the
/// scratchpad address multiplexers and the `DEST` register through six
/// stages --- `TPCLK`, `-TPCLK`, `-CLK0`, `CLK3`, `CLK3D`, then the part
/// itself --- while `-TPW70` reaches `-AWPA` through five. The pulse closes
/// before the clock edge arrives, so the write lands at `WADR` with `DESTD`
/// still holding the previous instruction's destination.
///
/// This engine has no gate delays, so a pulse that outlives the boundary
/// does two wrong things at once. The address multiplexers at ACTL 3A06 and
/// its fellows switch back to `IR<41:32>`, and the word is written at the
/// *next* instruction's A source address. And `DESTD` rises at the same
/// instant, so a new destination meets the old pulse and `-AWPA` glitches
/// low for a second, spurious write.
///
/// Ending the pulse at the boundary restores the order the hardware has, and
/// changes nothing else: no signal on the board is derived from the pulse's
/// width.
const WP_ON_NS: u32 = 30;
const WP_OFF_NS: u32 = 70;

/// `TPWPIRAM` comes from the latch at 1C13, set by `-TPREND` and cleared by
/// `-TPW45`.
const WPIRAM_OFF_NS: u32 = 45;

/// How long after `-TPREND` the next `-TPR0` asserts.
///
/// The restart runs through `CYCLECOMPLETED`, off the cross-coupled NAND pair
/// at CLOCK1 1C08/1C09: `-TPDONE` sets it, `-TPR40` clears it again 40 ns into
/// the next read phase, which is what makes the `-TPR0` pulse 40 ns wide.
///
/// `-TPDONE` is `-TPW60`. In `data/CADR.netlist` they are two names for one
/// net --- `-TPDONE` occurs once as an input with no driver, `-TPW60` once as
/// an output with no consumer --- so `src/netlist.rs` joins the two by an
/// explicit alias.
///
/// Note this is *before* `-TPW70`, so the write pulse of one cycle overlaps
/// the start of the next by 10 ns.
const RESTART_AFTER_READ_NS: u32 = 60;

/// The `-TPR0` pulse is forty nanoseconds wide; every read tap is that pulse
/// delayed.
pub const TPR_PULSE_NS: u32 = 40;

/// `-TPR60` is the one read tap something outside the generator uses:
/// `SPEEDCLK` is it inverted, off the 7428 at CLOCK2 1C01, and it clocks the
/// speed synchronizer at OLORD1 1A01 sixty nanoseconds into every generator
/// cycle, waits included. It is an event of its own so that it happens at
/// every speed: derived from whichever event fell inside its window, it
/// happened at normal speed and never at extra slow, and the machine could
/// not leave extra slow.
const TPR60_NS: u32 = 60;

/// When the tap that ends the read phase is chosen: after `SPEEDCLK` at 60
/// has clocked the synchronizer and the board has settled, before the
/// earliest tap at 75. The 74S151 at CLOCK1 1D08 is a multiplexer, so what
/// counts is what `SSPEED1, SSPEED0` and `-ILONG` stand at when the pulse
/// reaches it, and by 65 that is settled.
const SELECT_NS: u32 = 65;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ev {
    CycleStart,
    TseOn,
    TseOff,
    ReadEnd,
    WpIramOff,
    WpOn,
    WpOff,
    TprOn,
    TprOff,
    Select,
}

impl Ev {
    fn from_byte(b: u8) -> Option<Ev> {
        Some(match b {
            0 => Ev::CycleStart,
            1 => Ev::TseOn,
            2 => Ev::TseOff,
            3 => Ev::ReadEnd,
            4 => Ev::WpIramOff,
            5 => Ev::WpOn,
            6 => Ev::WpOff,
            7 => Ev::TprOn,
            8 => Ev::TprOff,
            9 => Ev::Select,
            _ => return None,
        })
    }
}

/// A small event scheduler --- the clock is the one place that needs one.
/// Everything else in the `chip` engine is levelized.
pub struct Behavioral {
    time: u64,
    cycle_start: u64,
    read_ns: u32,
    out: Outputs,
    pending: Vec<(u64, Ev)>,
}

impl Behavioral {
    pub fn new() -> Self {
        Behavioral {
            time: 0,
            cycle_start: 0,
            read_ns: 0,
            out: Outputs::default(),
            pending: vec![(0, Ev::CycleStart)],
        }
    }

    /// The cycle period at the current speed: read phase plus 60 ns.
    pub fn cycle_ns(&self) -> u32 {
        self.read_ns + RESTART_AFTER_READ_NS
    }

    fn schedule(&mut self, at: u64, ev: Ev) {
        let i = self.pending.partition_point(|&(t, _)| t <= at);
        self.pending.insert(i, (at, ev));
    }

    fn start_cycle(&mut self) {
        let t0 = self.time;
        self.cycle_start = t0;
        self.schedule(t0 + TSE_OFF_NS as u64, Ev::TseOff);
        self.schedule(t0 + TSE_ON_NS as u64, Ev::TseOn);
        self.schedule(t0 + TPR60_NS as u64, Ev::TprOn);
        self.schedule(t0 + SELECT_NS as u64, Ev::Select);
        self.schedule(t0 + (TPR60_NS + TPR_PULSE_NS) as u64, Ev::TprOff);
    }

    /// The read phase's length, and everything that hangs off its end.
    ///
    /// Chosen at [`SELECT_NS`] and not at `-TPR0`. `-ILONG` is `NAND(IR45,
    /// -NOPA)` off the `IR` the cycle's edge loads, and `SSPEED` shifts on
    /// `SPEEDCLK` at 60; sampled at `-TPR0` both were the previous cycle's,
    /// which is a microcycle wrong on every `ILONG` and a cycle late on a
    /// speed change.
    fn pick_tap(&mut self, inputs: Inputs) {
        self.read_ns = inputs.speed.read_phase_ns(inputs.ilong);
        let t0 = self.cycle_start;
        let r = self.read_ns as u64;
        self.schedule(t0 + r, Ev::ReadEnd);
        self.schedule(t0 + r + WPIRAM_OFF_NS as u64, Ev::WpIramOff);
        self.schedule(t0 + r + WP_ON_NS as u64, Ev::WpOn);
        // Scheduled before the restart, and at the same nanosecond as it, so
        // the pulse ends in its own step and the parts see it close before
        // the clock edge. See `WP_OFF_NS`.
        self.schedule(t0 + r + WP_OFF_NS.min(RESTART_AFTER_READ_NS) as u64, Ev::WpOff);
        self.schedule(t0 + r + RESTART_AFTER_READ_NS as u64, Ev::CycleStart);
    }
}

impl Behavioral {
    /// Writes the generator's state, so a run can be picked up again
    /// mid-cycle. See [`crate::chip::Chip::save`].
    ///
    /// The pending events have to go with it: this is an event scheduler,
    /// and where it is in the cycle is exactly what they say.
    pub fn save(&self, w: &mut impl std::io::Write) -> std::io::Result<()> {
        w.write_all(b"CADRCLK1")?;
        w.write_all(&self.time.to_le_bytes())?;
        w.write_all(&self.cycle_start.to_le_bytes())?;
        w.write_all(&self.read_ns.to_le_bytes())?;
        for up in [self.out.tpclk, self.out.tpwp, self.out.tptse, self.out.tpwpiram] {
            w.write_all(&[up as u8])?;
        }
        w.write_all(&(self.pending.len() as u32).to_le_bytes())?;
        for &(at, ev) in &self.pending {
            w.write_all(&at.to_le_bytes())?;
            w.write_all(&[ev as u8])?;
        }
        Ok(())
    }

    /// Reads back what [`Behavioral::save`] wrote.
    pub fn load(r: &mut impl std::io::Read) -> std::io::Result<Behavioral> {
        let bad = |what: &str| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("checkpoint: {what}"))
        };
        let mut magic = [0u8; 8];
        r.read_exact(&mut magic)?;
        if &magic != b"CADRCLK1" {
            return Err(bad("not a saved clock"));
        }
        let mut word = [0u8; 8];
        r.read_exact(&mut word)?;
        let time = u64::from_le_bytes(word);
        r.read_exact(&mut word)?;
        let cycle_start = u64::from_le_bytes(word);
        let mut half = [0u8; 4];
        r.read_exact(&mut half)?;
        let read_ns = u32::from_le_bytes(half);
        let mut ups = [0u8; 4];
        r.read_exact(&mut ups)?;
        let out = Outputs {
            tpclk: ups[0] != 0,
            tpwp: ups[1] != 0,
            tptse: ups[2] != 0,
            tpwpiram: ups[3] != 0,
        };
        r.read_exact(&mut half)?;
        let n = u32::from_le_bytes(half) as usize;
        // Not reserved up front: the count is the file's, and a corrupt one
        // would reserve gigabytes, where a clock has a cycle's worth of
        // events pending, under a dozen.  A count past what the file holds
        // fails on the first event that is not there.
        let mut pending = Vec::new();
        for _ in 0..n {
            r.read_exact(&mut word)?;
            let mut ev = [0u8; 1];
            r.read_exact(&mut ev)?;
            let ev = Ev::from_byte(ev[0]).ok_or_else(|| bad("unknown clock event"))?;
            pending.push((u64::from_le_bytes(word), ev));
        }
        Ok(Behavioral { time, cycle_start, read_ns, out, pending })
    }
}

impl Default for Behavioral {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for Behavioral {
    fn advance(&mut self, inputs: Inputs) -> (u32, Outputs) {
        if inputs.reset {
            self.out = Outputs::default();
            self.pending.clear();
            self.pending.push((self.time, Ev::CycleStart));
            return (0, self.out);
        }
        let Some(&(at, ev)) = self.pending.first() else { return (0, self.out) };
        // -HANG holds the next cycle off at -TPR0. That is how a memory wait
        // stretches a microcycle without disturbing the phase relationships.
        if inputs.hang && ev == Ev::CycleStart {
            return (0, self.out);
        }
        self.pending.remove(0);
        let dt = (at - self.time) as u32;
        self.time = at;
        match ev {
            Ev::CycleStart => {
                self.out.tpclk = true;
                self.start_cycle();
            }
            Ev::TseOn => self.out.tptse = true,
            Ev::TseOff => self.out.tptse = false,
            // `-TPR60` is put on the board from the phase, so these only
            // need to be transitions the board sees.
            Ev::TprOn | Ev::TprOff => {}
            Ev::Select => {
                // A clock loaded from a checkpoint written before the tap
                // moved here has the rest of its cycle scheduled already.
                if !self.pending.iter().any(|&(_, e)| e == Ev::ReadEnd) {
                    self.pick_tap(inputs);
                }
            }
            Ev::ReadEnd => {
                self.out.tpclk = false;
                self.out.tpwpiram = true;
            }
            Ev::WpIramOff => self.out.tpwpiram = false,
            Ev::WpOn => self.out.tpwp = true,
            Ev::WpOff => self.out.tpwp = false,
        }
        (dt, self.out)
    }

    fn next_at(&self, inputs: Inputs) -> Option<u64> {
        // `RESET` holds the ring cleared: no transition until it lifts, and
        // then a cycle starts from `-TPR0` --- [`Behavioral::advance`]
        // under reset leaves exactly that pending.  The debug cable's reset
        // bit holds it for as long as the debugger likes.
        if inputs.reset {
            return None;
        }
        let &(at, ev) = self.pending.first()?;
        if inputs.hang && ev == Ev::CycleStart { None } else { Some(at) }
    }

    fn pass(&mut self, until: u64, held: bool) {
        debug_assert!(until >= self.time, "time does not run backwards");
        if held {
            let dt = until - self.time;
            for e in &mut self.pending {
                e.0 += dt;
            }
        } else {
            debug_assert!(
                self.pending.first().is_none_or(|&(at, _)| until <= at),
                "passed a transition without taking it"
            );
        }
        self.time = until;
    }

    fn time_ns(&self) -> u64 {
        self.time
    }

    fn phase_ns(&self) -> u32 {
        (self.time - self.cycle_start) as u32
    }
}
