// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QUUX's keyboard and mouse (contract Q3), on the register page at
//! `1777777400`: word 120 the keyboard's status, 121 its data, 122 the mouse,
//! 123 the mouse's status.
//!
//! The CADR's I/O board takes its keyboard's words off a serial line at the
//! keyboard's own clock and its mouse's quadrature lines on `KB CLK^`
//! (`crate::ioboard`). QUUX has neither: its keyboard is a FIFO of
//! [`FIFO_WORDS`] of the words the CADR's keyboard gives, the same 32-bit
//! words `764100`/`764102` read together, and its mouse the CADR's twelve-bit
//! counts, to which the host adds its motion. What a host hands it --- muir's
//! terminal, or the Linux side of an FPGA board --- goes in whole.
//!
//! | Word | |
//! |---|---|
//! | 120 | `<0>` a key word is waiting, `<1>` the FIFO overflowed (a write clears it), `<8>` the keyboard's interrupt enable (written) |
//! | 121 | a read takes the oldest key word, in `<23:0>`, `<31:24>` reading 0; 0 when none is waiting |
//! | 122 | `<11:0>` the X count, `<27:16>` the Y count, `<14:12>` the buttons as the CADR's Y register has them; a read clears 123's `<0>` |
//! | 123 | `<0>` the mouse moved or a button changed since 122 was read, `<8>` the mouse's interrupt enable (written) |
//!
//! There is no beeper: the CADR's `764110` is a toggle on the speaker line
//! that the microcode flips once a half-wavelength, and QUUX leaves it out.

use std::collections::VecDeque;

/// How many key words the FIFO holds: the size of the microcode's own
/// keyboard buffer, so a burst fits even when the interrupt is taken late.
pub const FIFO_WORDS: usize = 64;

/// The register page's words, by their offset from its base.
pub const KBD_STATUS: u32 = 0o120;
pub const KBD_DATA: u32 = 0o121;
pub const MOUSE: u32 = 0o122;
pub const MOUSE_STATUS: u32 = 0o123;

/// An interrupt enable, `<8>` of 120 and of 123.
pub const ENABLE: u32 = 1 << 8;

/// A keyboard and mouse a host delivers to: the CADR's I/O board, or
/// QUUX's. What muir's terminal hands on, it hands through this.
pub trait KeyboardMouse {
    /// Whether a key word can be taken now: on the CADR, the board's one
    /// register is empty; on QUUX, the FIFO has room.
    fn takes_key(&self) -> bool;
    /// A key word came in.
    fn press(&mut self, word: u32);
    /// The mouse moved: `dx` counts to the right and `dy` down.
    fn mouse_move(&mut self, dx: i32, dy: i32);
    /// The buttons as the mouse now holds them, as the software's mask.
    fn mouse_buttons(&mut self, mask: u8);
    /// The buttons as the device last took them.
    fn mouse_buttons_held(&self) -> u8;
}

/// QUUX's keyboard and mouse.
#[derive(Clone, Debug, Default)]
pub struct QuuxInput {
    fifo: VecDeque<u32>,
    overflowed: bool,
    kbd_enable: bool,
    x: u16,
    y: u16,
    buttons: u8,
    mouse_changed: bool,
    mouse_enable: bool,
    /// The keyboard's boot word came in and the engine has not taken it
    /// ([`QuuxInput::take_boot`]); as on the I/O board, not in a checkpoint.
    boot: bool,
}

impl QuuxInput {
    pub fn new() -> QuuxInput {
        QuuxInput::default()
    }

    /// A read of the register page's word `word`, if it is one of these.
    pub fn read(&mut self, word: u32) -> Option<u32> {
        Some(match word {
            KBD_STATUS => {
                !self.fifo.is_empty() as u32
                    | (self.overflowed as u32) << 1
                    | if self.kbd_enable { ENABLE } else { 0 }
            }
            // The key word in `<23:0>`, as the CADR's keyboard gives it;
            // `<31:24>` read 0 (contract Q13).
            KBD_DATA => self.fifo.pop_front().unwrap_or(0) & 0xff_ffff,
            MOUSE => {
                self.mouse_changed = false;
                (self.y as u32) << 16 | (self.buttons as u32 & 7) << 12 | self.x as u32
            }
            MOUSE_STATUS => self.mouse_changed as u32 | if self.mouse_enable { ENABLE } else { 0 },
            _ => return None,
        })
    }

    /// A write of the register page's word `word`: whether it is one of
    /// these.
    pub fn write(&mut self, word: u32, v: u32) -> bool {
        match word {
            KBD_STATUS => {
                self.overflowed = false;
                self.kbd_enable = v & ENABLE != 0;
            }
            MOUSE_STATUS => self.mouse_enable = v & ENABLE != 0,
            KBD_DATA | MOUSE => {}
            _ => return false,
        }
        true
    }

    /// The interrupt status's bits: `<4>` the keyboard, `<5>` the mouse,
    /// each under its enable (contract Q13).
    pub fn interrupts(&self) -> u32 {
        ((self.kbd_enable && !self.fifo.is_empty()) as u32) << 4
            | ((self.mouse_enable && self.mouse_changed) as u32) << 5
    }

    /// Whether a key word is waiting to be read, word 120's `<0>`.
    pub fn key_waiting(&self) -> bool {
        !self.fifo.is_empty()
    }

    /// Whether the keyboard's boot word has come in since this was last
    /// asked, handed out once, as [`crate::ioboard::IoBoard::take_boot`].
    pub fn take_boot(&mut self) -> bool {
        std::mem::take(&mut self.boot)
    }

    pub fn save(&self, w: &mut crate::checkpoint::Writer) {
        w.u32(self.fifo.len() as u32);
        for &k in &self.fifo {
            w.u32(k);
        }
        w.bool(self.overflowed);
        w.bool(self.kbd_enable);
        w.u16(self.x);
        w.u16(self.y);
        w.u8(self.buttons);
        w.bool(self.mouse_changed);
        w.bool(self.mouse_enable);
    }

    pub fn load(&mut self, r: &mut crate::checkpoint::Reader) -> std::io::Result<()> {
        let n = r.u32()? as usize;
        if n > FIFO_WORDS {
            return Err(crate::checkpoint::bad(format!("a keyboard FIFO of {n} words")));
        }
        self.fifo = (0..n).map(|_| r.u32()).collect::<std::io::Result<_>>()?;
        self.overflowed = r.bool()?;
        self.kbd_enable = r.bool()?;
        self.x = r.u16()?;
        self.y = r.u16()?;
        self.buttons = r.u8()?;
        self.mouse_changed = r.bool()?;
        self.mouse_enable = r.bool()?;
        Ok(())
    }
}

impl KeyboardMouse for QuuxInput {
    fn takes_key(&self) -> bool {
        self.fifo.len() < FIFO_WORDS
    }

    fn press(&mut self, word: u32) {
        if crate::ioboard::boot_word(word) {
            self.boot = true;
        }
        if self.fifo.len() < FIFO_WORDS {
            self.fifo.push_back(word);
        } else {
            self.overflowed = true;
        }
    }

    fn mouse_move(&mut self, dx: i32, dy: i32) {
        if dx != 0 || dy != 0 {
            self.x = (self.x as i32 + dx).rem_euclid(4096) as u16;
            self.y = (self.y as i32 + dy).rem_euclid(4096) as u16;
            self.mouse_changed = true;
        }
    }

    fn mouse_buttons(&mut self, mask: u8) {
        let mask = mask & 7;
        if mask != self.buttons {
            self.buttons = mask;
            self.mouse_changed = true;
        }
    }

    fn mouse_buttons_held(&self) -> u8 {
        self.buttons
    }
}
