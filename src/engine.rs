// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! One interface, several implementations at different fidelity levels.
//!
//! Every engine agrees on the microcycle as the unit of progress and on
//! [`Machine`] as the state.  They differ in how much of the hardware they
//! compute on the way.

use crate::machine::{Halt, Machine};

pub trait Engine {
    /// The boot button: `-BOOT` presets `RUN`, and `-RESET` with it clears
    /// the console's registers, `PROMDISABLE` among them, so the machine
    /// starts from the boot PROM at the bottom of the control store.  It is
    /// what starts a CADR --- the machine comes up with its clock stopped
    /// --- and muir presses it for the engine at the start of a run unless
    /// `--no-auto-boot` says not to, where the prompt's `boot` presses it
    /// instead.
    fn boot(&mut self);

    /// **The keyboard's boot sequence**: `-BOOT1`, the boot line the
    /// keyboard reaches by way of the I/O board and the Unibus, pressed as
    /// the button presses `-BOOT2` --- on the board the two meet at the
    /// 74S02 at OLORD2 1A07 that makes `-BOOT`, and the processor cannot
    /// tell them apart.  The behavioral I/O board latches its decode of
    /// the boot word ([`crate::ioboard::IoBoard::take_boot`]); this takes
    /// it and presses, and says whether it did.  It leaves the word in the
    /// board's register with `KBD READY` up, as `boot` touches no board:
    /// microcode 323 reads it at `(LOC 6)` to choose cold from warm.  On
    /// `chip` the wire runs from the netlist board's `-BOOT*` instead
    /// ([`crate::cable::FarEnd::join`]).
    fn keyboard_boot(&mut self) -> bool {
        let m = self.machine_mut();
        // QUUX's keyboard is on the register page (contract Q3).
        let boot = m.ioboard.take_boot() || m.quux_input.take_boot();
        if boot {
            self.boot();
            true
        } else {
            false
        }
    }

    /// Runs one microcycle.
    fn step(&mut self) -> Result<(), Halt>;

    /// How long the machine's own microcycle is, what a run's speed is
    /// held against: the CADR's normal 145 ns, or QUUX's `sync` ticks.
    fn nominal_cycle_ns(&self) -> u64 {
        crate::ioboard::CYCLE_NS
    }

    /// Where the next microinstruction will be fetched from.
    fn pc(&self) -> u16;

    /// Whether [`Engine::pc`] holds a control-store write's address rather
    /// than where the program is. On the board the microcycle after
    /// `WRITE-I-MEM` writes the control store at the address the PC has
    /// moved to (`-IWEA`, `NAND(WP5A, IWRITEDA)` at ICTL 1B13), takes its
    /// instruction from `IWR` rather than from the PC, nopped and returning
    /// with a `POPJ` (`-POPJ = AND(-IPOPJ, -IWRITED)` at CONTRL 3D21;
    /// `IWRITED` among the terms of `N` at the 74S64 3E25), so the program
    /// never runs there. `rtl` keeps that microcycle; `micro` writes within
    /// `WRITE-I-MEM`'s own and never moves the PC to the address.
    fn pc_is_a_write(&self) -> bool {
        false
    }

    /// The location counter, [`crate::machine::LC_COUNTER`]: the byte
    /// address the next macroinstruction comes from, `LC<25:2>` being the
    /// word `VMA` takes at a fetch.
    ///
    /// **Where an engine keeps it is the engine's business, and this is
    /// how anything holding two of them to the same machine reaches it.**
    /// `micro` keeps it in [`Machine`], carrying `NEEDFETCH` above it in
    /// the same word; `rtl` keeps the counters in a field of their own
    /// beside the rest of its pipeline, with the byte-mode flags on page
    /// FLAG. Mirroring one into the other's storage would be two places
    /// holding one register with nothing keeping them in step, so the
    /// register is asked for instead and the mask is what makes the
    /// answers comparable.
    fn lc(&self) -> u32 {
        self.lc_wide() as u32
    }

    /// The location counter whole, [`crate::machine::Geometry::lc_counter`]'s
    /// bits: `LC<33:0>` on revision 14 (A14.11), whose `<33:32>`
    /// [`Engine::lc`] leaves out.
    fn lc_wide(&self) -> u64 {
        self.machine().lc & self.machine().geometry.lc_counter()
    }

    /// What the cpu drives onto `SPY<15:0>` while `-DBREAD` is low with
    /// `EADR<3:0>` at `eadr`: one of the sixteen diagnostic registers,
    /// [`crate::spy::IR_LOW`] to [`crate::spy::STAT_HIGH`], as the console
    /// reads them.  Register 3 has no read select and reads as the open
    /// bus, [`crate::spy::OPEN_READ`].
    ///
    /// The values are the machine's as it stands between two microcycles,
    /// which is what a halted machine shows a console and what a read in
    /// flight sees at the microcycle boundary the interface answers in.
    fn spy_read(&self, eadr: u8) -> u16;

    /// Loads one of the console's registers, as the trailing edge of
    /// `-DBWRITE` does.  The registers are [`Machine`]'s, so this is
    /// [`Machine::spy_write`] for every engine; it is here so that a console
    /// has one interface to hold.
    fn spy_write(&mut self, eadr: u8, v: u16) {
        self.machine_mut().spy_write(eadr, v);
    }

    fn machine(&self) -> &Machine;
    fn machine_mut(&mut self) -> &mut Machine;

    /// The engine and its machine into a checkpoint, [`crate::checkpoint`]:
    /// the machine first, then the engine's own state.
    fn save(&self, w: &mut crate::checkpoint::Writer);

    /// Back from a checkpoint, into an engine built and booted as the flags
    /// say: the pack under it and the Chaosnet on it stay as they are.
    fn load(&mut self, r: &mut crate::checkpoint::Reader) -> std::io::Result<()>;

    /// **The time-neutral harness's action point** (MP2b rulings 1,
    /// Q12(b)): stand still after microcycle `mc`, nothing of a later
    /// microcycle begun, so that what the harness does there --- a key, a
    /// look at the screen, a poll for a file --- lands between the two
    /// microcycles, as on a single-edge machine. An engine that runs one
    /// microcycle a step stands there already; the pipeline halts after the
    /// word and drains (`Pipeline::boundary_at`).
    fn stop_after_microcycle(&mut self, _mc: u64) {}

    /// Whether the engine stands where [`Engine::stop_after_microcycle`]
    /// asked, every write of the microcycles before taken.
    fn stands_between_microcycles(&self) -> bool {
        true
    }

    /// At the harness's action point, what a halt would settle: `micro`
    /// sends out a write still waiting (MP2b rulings 2, Q1).
    fn settle_for_harness(&mut self) {}

    /// Runs until it halts or `limit` microcycles have passed.  Returns the
    /// number run.
    fn run(&mut self, limit: u64) -> (u64, Option<Halt>) {
        for n in 0..limit {
            if let Err(h) = self.step() {
                return (n, Some(h));
            }
        }
        (limit, None)
    }
}
