// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The standard black-and-white display: a frame buffer and a mode register
//! on the Xbus.
//!
//! One bit per pixel, 768 across, 24 words to a line. MIT's own name for it
//! is the TV, and the window system asks for it by
//! `'(:VIDEO :BLACK-AND-WHITE :CONTROLLER :SIMPLE)`.
//!
//! Three sources, reaching us by three routes, and they agree:
//!
//! 1. **`sys/window/shwarm.lisp`** in the System 100 release --- the software
//!    we target, writing to this device. It gives the control address, the
//!    buffer length, the geometry and the one mode bit it uses.  **primary**
//! 2. **`cadrtv/lmtv.order`** --- MIT's own programming specification for the
//!    board, which numbers the eight control registers and the mode bits and
//!    says where the buffer is: "for the normal TV, x is 6", and the normal
//!    TV is the `:CONTROLLER :SIMPLE` one.  **primary**
//! 3. **The board itself**, `data/SIMPLETV.netlist` through
//!    `tools/simpletv-netlist.sh`: all 29 of MIT's SUDS pages, of which
//!    `nxbctl` is the mode register and the control decode. The netlist
//!    board runs --- `tests/simpletv_netlist.rs` brings it up on a bus and
//!    measures its clock --- and is on the machine's Xbus under `chip`;
//!    this model answers for it on `micro` and `rtl`, and on `chip` with
//!    `--tv model`, written from the programming interface as
//!    [`crate::disk_controller`] is.  **primary, for the mode register**
//!
//! **Two boards, one model.** `--tv-board` says which ([`Board`]), on
//! every engine: the SIMPLE TV above, and the LISPM TV that replaced it in
//! December 1980 --- `data/LISPMTV.netlist` through
//! `tools/lispmtv-netlist.sh`, the board `lmtv.order` is the specification
//! of. **They program alike but for one bit**, mode
//! bit 7 ([`mode::SYNC_PROM_ENABLE`]); register 4, the color map's write
//! port ([`Tv::color_map`]), is the same circuit on both, measured on each
//! board in `tests/simpletv_netlist.rs` and `tests/lispmtv_netlist.rs`.
//! Either board is strapped here as the normal TV, at `17000000` and
//! `17377760`.
//!
//! **A second board, the color TV.** `lmtv.order`: "For the normal TV, x
//! is 6.  For the color TV, x is 5", and the buffer "starts at 17200000".
//! That is [`COLOR_TV`], a LISPM TV strapped there and fitted by
//! `--color-tv`; the main screen stays at `17000000` whichever board
//! `--tv-board` named, because System 100 hardwires `MAIN-SCREEN` to one
//! bit a pixel there and defines `COLOR-SCREEN` at `17200000`
//! ([`Tv::color`]). Its picture is 576 by 454 at four bits a pixel
//! ([`Tv::pixel4`]) through the sixteen colors of the map
//! ([`Tv::rgb`]). On `chip` the board is a netlist on the backplane like
//! the others --- `data/LISPMTV.netlist` through
//! [`crate::netlist::parse_color_tv`], wrapped to [`COLOR_TV`] by
//! [`crate::xbus::straps`] --- and `--color-tv model` keeps this model
//! there instead. Either way the model is fitted: every write to the
//! netlist board is mirrored into it, so the color picture is read off
//! the same place whichever board drew it, exactly as the main screen's
//! is.
//!
//! **The sync program is run.** The board's timing is the program in its
//! sync RAM, or in its PROM until the software selects the RAM, and the
//! model runs that program as the board does ([`sync`]): the vertical flag
//! is preset where the program's `TVMA CLR` falls, and `VSYNC` and `HSYNC`
//! in the mode register are the program's own bits, which is what the
//! color software's `%XBUS-WRITE-SYNC` waits on. What the model does not
//! do is scan: no dot is fetched and no monitor is driven, so the picture
//! is the frame buffer as it stands, and a program that fetches part of it
//! or none shows the whole of it all the same.

pub mod sync;

use sync::Timeline;

/// First word of the frame buffer.  `MAIN-SCREEN-BUFFER-ADDRESS` is
/// `IO-SPACE-VIRTUAL-ADDRESS`, the base of Xbus I/O space, which
/// `bus-adaptor.c` decodes as physical `017000000`.
pub const BUFFER: u32 = 0o17000000;

/// `(DEFCONST MAIN-SCREEN-BUFFER-LENGTH #o100000)` --- 32,768 words, which is
/// more than the 23,112 the screen uses.
pub const BUFFER_WORDS: u32 = 0o100000;

/// `(DEFCONST MAIN-SCREEN-CONTROL-ADDRESS #o377760)`, as an Xbus I/O offset;
/// physical is that plus [`BUFFER`].  Eight words: the mode register, then
/// the sync program's data, pointer and enable, then three that answer
/// and do nothing.
pub const CONTROL: u32 = 0o17377760;

/// Words of the sync program RAM: the eight 2147s at NSYRAM 0A01-0B04,
/// 4K by 1 each, one per bit, addressed by the twelve bits of
/// [`SyncRam::pointer`]. `cpt.prom`'s 297 words and `SET-TV-SPEED`'s
/// program both fit many times over.
pub const SYNC_RAM_WORDS: usize = 4096;

/// How many control words the board answers on.  `lmtv.order` runs the
/// registers from `173777x0` to `173777x7`, the last three of which "respond
/// but don't do anything", and NXBCTL 0F13 is a 74S138 on `ADR0..2` whose
/// top three outputs go nowhere.  `bus-adaptor.c` decodes the same eight,
/// `017377760`-`017377767`.
pub const CONTROL_WORDS: u32 = 8;

/// `(DEFVAR MAIN-SCREEN-WIDTH (SELECT-PROCESSOR (:CADR 768.)))`
pub const WIDTH: usize = 768;

/// `(:CADR 963.)`, with MIT's own comment `;was 896. for CPT`.  The older
/// value is 896, which is the CPT monitor's.
pub const HEIGHT: usize = 963;

/// `(DEFVAR MAIN-SCREEN-LOCATIONS-PER-LINE (SELECT-PROCESSOR (:CADR 24.)))`
/// --- 24 words of 32 bits is the 768 pixels of a line, one bit each.
pub const WORDS_PER_LINE: usize = 24;

/// The video controller, QUUX's display, "video" for short: 1280 by 1024
/// unless `--video-size` says otherwise ([`check_video_size`]), one bit a
/// pixel. Not the CADR's: QUUX's own, its frame buffer its memory, and its
/// mode, word 210 of the register page, with black-on-white in it
/// (contract Q13); nothing else --- no sync program, no color map and no
/// interrupt, the machine's clock being the tick, timer 0 of the interval
/// timers (`machine::Timers`).
pub const VIDEO_WIDTH: usize = 1280;
/// Lines of the video controller's screen, at its default size.
pub const VIDEO_HEIGHT: usize = 1024;
/// 1280 pixels of one bit each are 40 words of 32, a whole number, which
/// `BITBLT` needs of an array's first dimension (`sys/ucadr/uc-tv.lisp`,
/// `BITBLT-DECODE-ARRAY`).
pub const VIDEO_WORDS_PER_LINE: usize = 40;
/// The video controller's buffer at its default size, 40,960 words. The
/// size is muir-fpga's Arty Z7-20's and DE25-Nano's; its Kria KR260's is
/// 1920 by 1080, [`VIDEO_MAX_SIZE`].
pub const VIDEO_WORDS: u32 = (VIDEO_HEIGHT * VIDEO_WORDS_PER_LINE) as u32;

/// The most the video controller's buffer can be: QUUX's frame buffer
/// window, `1760000000`-`1777775777` ([`crate::machine::WINDOW_13`]),
/// 4,193,280 words. The address map fixes it, and nothing states it to
/// software.
pub const VIDEO_MAX_WORDS: u32 = 0o1777776000 - crate::machine::WINDOW_13;

/// The largest video controller screen: QUUX supports up to 1920 by 1080
/// (a revisable limit). Its 64,800 words also stay below the color TV's strap.
pub const VIDEO_MAX_SIZE: (usize, usize) = (1920, 1080);

/// Whether the video controller can be `width` by `height`: a line a whole number of
/// words, which `BITBLT` needs of a screen array's first dimension
/// (`BITBLT-DECODE-ARRAY` in `sys/ucadr/uc-tv.lisp`); at most
/// [`VIDEO_MAX_SIZE`]; both at most 16 bits, as the feature page gives
/// them; the buffer inside [`VIDEO_MAX_WORDS`]; and, with the color TV
/// fitted, below its strap at `17200000`.
pub fn check_video_size(width: usize, height: usize, color_tv: bool) -> Result<(), String> {
    if width == 0 || height == 0 || !width.is_multiple_of(32) {
        return Err(format!("a width of {width} is not a whole number of 32-bit words"));
    }
    let (max_w, max_h) = VIDEO_MAX_SIZE;
    if width > max_w || height > max_h {
        return Err(format!("{width} by {height} is past {max_w} by {max_h}"));
    }
    if width > 0xffff || height > 0xffff {
        return Err(format!("{width} by {height} does not fit the feature page's 16-bit fields"));
    }
    let words = (width / 32 * height) as u64;
    check_video_words(words).map_err(|e| format!("{width} by {height} is {e}"))?;
    let color_words = (COLOR_TV.buffer - BUFFER) as u64;
    if color_tv && words > color_words {
        return Err(format!(
            "{width} by {height} is {words} words, over the color TV's buffer at 17200000"
        ));
    }
    Ok(())
}

/// The words bound of [`check_video_size`]: a buffer of `words` fits in
/// the frame buffer window, at most [`VIDEO_MAX_WORDS`].
pub fn check_video_words(words: u64) -> Result<(), String> {
    if words > VIDEO_MAX_WORDS as u64 {
        return Err(format!("{words} words, past the window's {VIDEO_MAX_WORDS}"));
    }
    Ok(())
}

/// Where a board is strapped on the Xbus: `lmtv.order`'s x, which the two
/// boards of a two-screen machine are wired to two values of.
///
/// > Note: For the normal TV, x is 6.  For the color TV, x is 5.
///
/// and, of the buffer, "The normal TV has x equal to 0, so the buffer
/// starts at 17000000.  The color TV has x equal to 2, and so the buffer
/// starts at 17200000." Two xs, because the registers and the buffer are
/// two decodes; one strap, because one board carries both.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Strap {
    /// First word of this board's 32K-word frame buffer.
    pub buffer: u32,
    /// First of its eight control words.
    pub control: u32,
}

/// The normal TV's strap, [`BUFFER`] and [`CONTROL`]: whichever board
/// `--tv-board` put there.
pub const NORMAL_TV: Strap = Strap { buffer: BUFFER, control: CONTROL };

/// The color TV's strap: the buffer at `17200000` and the registers at
/// `17377750`, `lmtv.order`'s x = 2 and x = 5. System 100 asks for exactly
/// these --- `COLOR:MAKE-SCREEN` in `sys/window/color.lisp` defines the
/// screen `:BUFFER -600000 :CONTROL-ADDRESS 377750`, and `COLOR-EXISTS-P`
/// probes `(LOGAND (TV:SCREEN-BUFFER SCREEN) 377777)`, which is `200000`
/// as an Xbus I/O offset.
pub const COLOR_TV: Strap = Strap { buffer: 0o17200000, control: 0o17377750 };

impl Strap {
    /// The word offset into this board's frame buffer a physical address
    /// names, if it is in it.
    pub fn buffer_offset(self, phys: u32) -> Option<u32> {
        let off = phys.wrapping_sub(self.buffer);
        (off < BUFFER_WORDS).then_some(off)
    }

    /// Which of this board's control registers a physical address names,
    /// if it is one.
    pub fn control_register(self, phys: u32) -> Option<u32> {
        let off = phys.wrapping_sub(self.control);
        (off < CONTROL_WORDS).then_some(off)
    }

    /// Whether a board at this strap answers the address at all.
    pub fn answers(self, phys: u32) -> bool {
        self.buffer_offset(phys).is_some() || self.control_register(phys).is_some()
    }
}

/// `(:WIDTH 576.)` of `COLOR:MAKE-SCREEN` --- the color picture's width in
/// pixels, which is `lmtv.order`'s "successive 4-bit pixels of the video
/// buffer at a 12 MHz rate" over the 36 video cycles of 64 bits a line
/// that `COLOR:SYNC` fetches (`tests/sync_program.rs`).
pub const COLOR_WIDTH: usize = 576;

/// `(:HEIGHT 454.)` of `COLOR:MAKE-SCREEN`, which is the 227 picture lines
/// of each of `COLOR:SYNC`'s two NTSC fields.
pub const COLOR_HEIGHT: usize = 454;

/// `(:BITS-PER-PIXEL 4)` of `COLOR:MAKE-SCREEN`: a pixel is a color, the
/// four-bit address into the map.
pub const COLOR_BITS_PER_PIXEL: usize = 4;

/// Words of the buffer a color line takes: 576 pixels of 4 bits is 2304
/// bits, 72 words of 32.  The screen array `COLOR:MAKE-SCREEN` displaces
/// onto the buffer is `ART-4B` and this is its row.
pub const COLOR_WORDS_PER_LINE: usize = COLOR_WIDTH * COLOR_BITS_PER_PIXEL / 32;

/// Which display board this is: `--tv-board`, on every engine.
///
/// **One model serves both boards.** `cadrtv/lmtv.order` is the LISPM
/// TV's own programming specification and it is what the SIMPLE TV model
/// was written from; both boards answer the same eight control words at
/// `17377760` and carry the same 32K-word buffer at `17000000`
/// (`tests/simpletv_netlist.rs` and `tests/lispmtv_netlist.rs` each bring
/// their board up on a bus and read them). **One bit of the interface
/// differs**, [`mode::SYNC_PROM_ENABLE`], mode bit 7, and that is what
/// this says.
///
/// Register 4, the color map's write port, is **not** a difference: the
/// SIMPLE TV carries the same page, `RAMCOL.DRW` in `lmtv.stf`'s own
/// words "SIMPLE TV / COLOR MAP", revised to `nracol` in May 1980, and
/// part for part it is the LISPM TV's `COLOR`. Both boards strobe it,
/// measured on each, so [`Tv::color_map`] is written on both.
///
/// The LISPM TV is strapped here as the normal TV, exactly as the SIMPLE
/// TV is: `lmtv.order`'s "for the normal TV, x is 6" and "the normal TV
/// has x equal to 0, so the buffer starts at 17000000". The color TV ---
/// MIT's own spelling for the second board, strapped to `17200000` and
/// `17377750` --- is not this flag.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Board {
    /// The black-and-white board System 100 drives, `data/SIMPLETV.netlist`.
    #[default]
    SimpleTv,
    /// The four- and eight-bit board that replaced it in December 1980,
    /// `data/LISPMTV.netlist`.
    LispmTv,
    /// The video controller, QUUX's display, [`VIDEO_WIDTH`] by
    /// [`VIDEO_HEIGHT`] unless [`Tv::set_video_size`] says otherwise: not
    /// the CADR's, and not on its backplane.
    Video,
}

impl Board {
    /// What `--tv-board` calls it, which is also what the start summary
    /// says and what a checkpoint carries.
    pub fn name(self) -> &'static str {
        match self {
            Board::SimpleTv => "simple-tv",
            Board::LispmTv => "lispm-tv",
            Board::Video => "video",
        }
    }
}

/// Colors the color map holds: `lmtv.order`'s "3-0 Color (i.e. address
/// into color map)" and "we only use a 16x8 subset of it", and
/// `WRITE-COLOR-MAP`'s `(LOGAND LOC 17)`.
pub const COLORS: usize = 16;

/// Channels the color map has: `lmtv.order`'s "7-6 Select which color
/// map (up to 4 channels)", of which three are wired --- the 74S139 at
/// 0E10, on the LISPM TV's COLOR page and the SIMPLE TV's NRACOL, decodes
/// them into `-LOAD COLOR 0`, `1` and `2` and leaves its fourth output
/// unconnected. `WRITE-COLOR-MAP` writes red on 0, green on 1 and blue
/// on 2.
pub const CHANNELS: usize = 3;

/// Mode register bits, read off the board's own "SIMPLE TV / MODE REGISTER"
/// drawing and named as `lmtv.order` names them. That is the word MIT's
/// window system uses in `'(:CONTROLLER :SIMPLE)`.
///
/// MIT drew this page twice. `synmod.drw` of 28 May 1979 has the register as
/// one **74S174**, a hex D flip-flop whose data pins are `XDI 3..0` alone,
/// read back through a **74S241** octal buffer on `XDO 7..0`. `nxbctl.drw`
/// of 17 May 1980, the newer sheet and the one in `data/SIMPLETV.netlist`,
/// does the same job with an **Am25LS2519** --- a quad register with a
/// second, three-state output set that puts the four bits straight back on
/// `XDO 3..0` --- and half a **74LS244** on `XDO 7..4`. Two parts, two
/// drawings, one interface: four bits latch and four are read through a
/// buffer. `tests/simpletv_netlist.rs` holds the constants below to the
/// netlist pin by pin.
///
/// `MODE<4>` is not in the register with the four below it: it is a flop
/// of its own, clocked by the same write --- [`mode::VERT`].
pub mod mode {
    /// `MODE<1:0>`, `CLOCK MODE 0` and `CLOCK MODE 1`: which dot clock the
    /// sync PROM is addressed with.
    pub const CLOCK: u32 = 0o3;
    /// `MODE<2>`, `MODE BOW`.  `BLACK-ON-WHITE` sets it --- "display one bits
    /// as black and zeros as white" --- with `(LOGIOR 4 ...)`, and
    /// `WHITE-ON-BLACK` clears it with `(LOGAND -5 ...)`, MIT commenting that
    /// as "1's comp of 4".
    pub const BOW: u32 = 0o4;
    /// `MODE<3>`, `MODE INTR ENB`: enables the vertical interrupt.
    pub const INTERRUPT_ENABLE: u32 = 0o10;

    /// `MODE<3:0>` --- the four data pins the register has, and the whole of
    /// what a write can change.
    pub const WRITABLE: u32 = 0o17;

    /// `MODE<4>`, `VERT FLAG`, "Causes Interrupt". Not in the 2519 with the
    /// four below it but a flop of its own, the 74LS74 at NXBCTL 0E14:
    /// **preset** by `-TVMA CLR`, the sync program's start of frame ---
    /// `lmtv.order`: "this is set by TVMA CLR, not by the start of Vertical
    /// Sync" --- and **clocked by `-LOAD MODE` with `XDI 4` as its data**,
    /// so a write of the register puts the written bit 4 into it. It reads
    /// back through the 74LS244 at 0F11. Microcode 323's `INTRX0` takes
    /// the interrupt by reading the register, testing this bit, and
    /// writing it back with the bit cleared. With [`INTERRUPT_ENABLE`] it
    /// is `SEND INTR` through the 74S08 at 0D10, onto `-XBUS.INTR`.
    pub const VERT: u32 = 0o20;
    /// `MODE<5>`, `VSYNC`.  Read only, "directly from sync generator".
    pub const VSYNC: u32 = 0o40;
    /// `MODE<6>`, `HSYNC`.  Read only.
    pub const HSYNC: u32 = 0o100;
    /// `MODE<7>`, `SYNC PROM ENB`.  Read only, and **the one bit of the
    /// interface the two boards differ in**: on the LISPM TV it reads the
    /// sync enable back, and on the SIMPLE TV it reads zero.
    ///
    /// On the LISPM TV the read buffer is the 74LS244 at XBCTL 0F11
    /// section A, and its bit 7 input, pin 8, is the net `-SYNC PROM ENB`
    /// --- pin 19 of the 74LS273 at TVINC 0A07, the Q of the D that `XDI7`
    /// feeds, which is register 3's bit 7, [`SyncRam::enabled`]. So the bit
    /// is one while the sync RAM is selected and zero while MIT's PROM is.
    /// On the SIMPLE TV the same pin is `GND`: ECO 2 of `cadrtv/lmtv.eco`,
    /// 18 June 1980, "new window system not initializing tv properly at
    /// original power-up; on old TV boards the check if TV is in PROM mode
    /// (extant only on new TV boards) reads an unused input", applied when
    /// `data/SIMPLETV.netlist` is built. `lmtv.order` --- the LISPM TV's
    /// own sheet --- calls everything from bit 7 up garbage all the same,
    /// so the bit is the board's and not the programming specification's.
    ///
    /// **No source in System 100 reads it, and the source that would is
    /// not in the release.** The release's only reads of the mode register
    /// are `shwarm.lisp`'s three read-modify-writes of [`BOW`] and
    /// `color.lisp`'s waits on [`VSYNC`]. `SI:SETUP-CPT`, which ECO 2 says
    /// does the checking, is called from `sys/sys/ltop.lisp` and
    /// `shwarm.lisp` and exported by `sys/cold/export.lisp` as
    /// `SYS: WINDOW; SHWARM`, and is defined in no file the release ships;
    /// nor are `SI:STOP-SYNC`, `SI:FILL-SYNC` and `SI:START-SYNC`, which
    /// `color.lisp` calls and the same file exports as `SYS: WINDOW;
    /// COLOR`. So what the software does with the bit is **the ECO's word
    /// and not a line of code we have**: it tells a new board from an old
    /// one. Both boards are modeled as the boards read, which is what
    /// either answer of that check would find.
    pub const SYNC_PROM_ENABLE: u32 = 0o200;

    /// `MODE<7:4>`, everything the read buffer sources from somewhere other
    /// than the 2519: [`VERT`], the flop above; [`VSYNC`] and [`HSYNC`],
    /// the sync program's own bits; and [`SYNC_PROM_ENABLE`], the sync
    /// enable on the LISPM TV and ground on the SIMPLE TV. None of the
    /// four can be written.
    pub const READ_ONLY: u32 = 0o360;
}

/// One frame of MIT's PROM program in clock mode 0, and so the period of
/// `TVMA CLR` and the vertical flag from power-on: 966 lines of 16.000 us,
/// measured on the netlist board in `tests/simpletv_netlist.rs` and
/// `tests/monitor.rs`, and what running `cpt.prom` here comes to
/// (`tests/sync_program.rs`). 64.7 Hz, which is what the microcode calls
/// "the roughly-60-cycle clock". The model's own timing is the program
/// running, [`sync`]; this is the nominal frame for whoever wants one, the
/// terminal's refresh among them.
pub const FRAME_NS: u64 = 15_456_000;

/// The word offset into the normal TV's frame buffer a physical address
/// names, if it is in it: [`NORMAL_TV`]'s.
pub fn buffer_offset(phys: u32) -> Option<u32> {
    NORMAL_TV.buffer_offset(phys)
}

/// Which of the normal TV's control registers a physical address names, if
/// it is one: [`NORMAL_TV`]'s.
pub fn control_register(phys: u32) -> Option<u32> {
    NORMAL_TV.control_register(phys)
}

/// The sync program RAM and its two registers, `lmtv.order`'s `173777x1`
/// to `x3`: "Sync Program [Sync Ptr] (read/write)", "Sync Ptr (write
/// only) Address for reading and writing the Sync Program RAM", and "Sync
/// Enable (write only, cleared by Xbus reset) ... Set it to 1 after you
/// have loaded the correct sync program" over the "Vertical Spacing".
///
/// The enable and the spacing are the 74LS273 at NTVINC 0A07, whose clear
/// (pin 1) is `-POWER RESET` and not the `-RESET` that `XBUS INIT IN`
/// makes; so where `lmtv.order` has the enable "cleared by Xbus reset",
/// the drawing has it cleared by the backplane's `-XBUS POWER RESET`, and
/// the drawing is followed: a bus reset leaves both standing
/// ([`Tv::xbus_init`]), and power-on --- [`SyncRam::default`] ---
/// clears them.
///
/// The enable is also what selects the RAM over the PROM at NSYRAM --- the
/// 2147s' chip select is `SYNC PROM ENB` and the 74S472's its complement
/// --- so the program the board runs is the RAM's while it is set and
/// MIT's `cpt.prom` while it is clear ([`SyncRam::program`]), and a read
/// of the data register with it clear is the PROM's word.
#[derive(Clone)]
pub struct SyncRam {
    words: Vec<u8>,
    /// The twelve-bit address the next data access goes to.
    pub pointer: u16,
    /// Bit 7 the sync enable, 6-0 the vertical spacing.
    pub enable: u8,
}

impl Default for SyncRam {
    fn default() -> Self {
        SyncRam { words: vec![0; SYNC_RAM_WORDS], pointer: 0, enable: 0 }
    }
}

impl SyncRam {
    /// The program as loaded.
    pub fn words(&self) -> &[u8] {
        &self.words
    }

    /// Whether the software has turned the sync outputs on and the RAM in:
    /// bit 7 of register 3.
    pub fn enabled(&self) -> bool {
        self.enable & 0o200 != 0
    }

    /// The program the sync generator fetches: the RAM's while the enable
    /// selects it, MIT's PROM's otherwise.
    pub fn program(&self) -> &[u8] {
        if self.enabled() { &self.words } else { sync::prom() }
    }
}

/// The frame buffer, the mode register, the vertical flag, the sync
/// program RAM with the program running, and the color map.
#[derive(Clone)]
pub struct Tv {
    /// Which of the two boards this is: what mode bit 7 reads.
    board: Board,
    /// The video controller's width and height, when that is the board.
    video_size: (usize, usize),
    /// Where on the Xbus it is strapped, and so which screen it is: the
    /// normal TV or the color TV.  The backplane's, not the software's:
    /// it does not change under a running machine.
    strap: Strap,
    buffer: Vec<u32>,
    mode: u32,
    /// Registers 1 to 3.
    pub sync: SyncRam,
    /// The color map as written, `[color][channel]`: [`Tv::color_map`].
    color_map: [[u8; CHANNELS]; COLORS],
    /// The vertical flag's flop as something other than the running
    /// program last set it: the bit a mode write clocked in, the zero
    /// `-XBUS INIT` cleared it to ([`Tv::xbus_init`]), or the value it
    /// carried over the program being run afresh ([`Tv::restart`]).
    flag_written: bool,
    /// When that was, in the machine's nanoseconds: the flag is that bit,
    /// or the `TVMA CLR` that has come since.
    written_at: u64,
    /// The first `-TVMA CLR` after `written_at` under the program running,
    /// in the machine's nanoseconds; the end of time while no program makes
    /// a frame. Kept so that [`Tv::vert_flag`], which every microcycle asks,
    /// is one comparison: [`Tv::flag_written_at`].
    next_clr: u64,
    /// The program running, laid out in time; `None` while the RAM is
    /// selected and holds no program that makes a frame, as it does before
    /// and part way through the software's loading of it.
    timeline: Option<Timeline>,
    /// When the program running started from its location 0, in the
    /// machine's nanoseconds: power-on, or the last change of program or
    /// clock mode ([`Tv::restart`]).
    origin: u64,
    /// The sync bits `(hsync, vsync)` the mode register was holding when
    /// the program running started, which is what it goes on reading
    /// until that program's first instruction lands: the 74LS175 that
    /// latches them, NSYREG 0D02, has its clear on a pull-up and the
    /// program's start reaches neither it nor its clock
    /// ([`sync::Timeline::sync_at_since_start`]).
    sync_held: (bool, bool),
}

impl Default for Tv {
    fn default() -> Self {
        let sync = SyncRam::default();
        let timeline = Timeline::of(sync.program(), 0);
        Tv {
            board: Board::default(),
            video_size: (VIDEO_WIDTH, VIDEO_HEIGHT),
            strap: NORMAL_TV,
            buffer: vec![0; BUFFER_WORDS as usize],
            mode: 0,
            sync,
            color_map: [[0; CHANNELS]; COLORS],
            flag_written: false,
            written_at: 0,
            next_clr: timeline.as_ref().map_or(u64::MAX, |t| t.next_tvma_clr_after(0)),
            timeline,
            origin: 0,
            // Power-on is the program started from a register holding
            // nothing: the 74LS175 at NSYREG 0D02 has no clear at all ---
            // pin 1 is the pull-up `HI` at XBADR 0F10 on the SIMPLE TV
            // and `HI5` at XBADR 0D04 on the LISPM TV --- so what it
            // holds there is whatever its flops came up in, and zero is
            // what the model starts them at.
            sync_held: (false, false),
        }
    }
}

impl Tv {
    /// **The color TV**: a LISPM TV strapped to [`COLOR_TV`], which is the
    /// second display board a CADR can carry and what `--color-tv` fits.
    ///
    /// A LISPM TV because that is the board the four-bit picture and its
    /// map belong to --- `lmtv.order` is its specification and MIT's own
    /// `lmtv4b` is the four-bit build of it --- and because the main board
    /// may be either: `--tv-board` is the normal TV's and not this one's.
    pub fn color() -> Tv {
        Tv { board: Board::LispmTv, strap: COLOR_TV, ..Tv::default() }
    }

    /// Which board this is.
    pub fn board(&self) -> Board {
        self.board
    }

    /// Where on the Xbus this board is strapped: [`NORMAL_TV`] or
    /// [`COLOR_TV`].
    pub fn strap(&self) -> Strap {
        self.strap
    }

    /// Puts the model on the other board. What `--tv-board` does, where
    /// each engine builds its machine; the board is the backplane's and
    /// does not change under a running machine.
    pub fn set_board(&mut self, board: Board) {
        self.board = board;
        self.buffer = vec![0; self.buffer_words() as usize];
        if board == Board::Video {
            // No sync program: nothing presets a vertical flag.
            self.timeline = None;
            self.next_clr = u64::MAX;
        } else {
            self.timeline = Timeline::of(self.sync.program(), self.mode & mode::CLOCK);
            let (flag, at) = (self.flag_written, self.written_at);
            self.flag_written_at(flag, at);
        }
    }

    /// The video controller's size, `--video-size`: [`check_video_size`] is the
    /// caller's. The buffer is made again, so this is for building a
    /// machine and not for a running one.
    pub fn set_video_size(&mut self, width: usize, height: usize) {
        self.video_size = (width, height);
        if self.board == Board::Video {
            self.buffer = vec![0; self.buffer_words() as usize];
        }
    }

    /// The main screen it shows: width, height and words a line. The
    /// CADR's two boards show what System 100 hardwires, [`WIDTH`] by
    /// [`HEIGHT`]; the video controller its size.
    pub fn screen(&self) -> (usize, usize, usize) {
        match self.board {
            Board::Video => {
                let (w, h) = self.video_size;
                (w, h, w / 32)
            }
            _ => (WIDTH, HEIGHT, WORDS_PER_LINE),
        }
    }

    /// Which of the eight control registers answer, a bit each: all of them
    /// on the CADR's boards; none on the video controller, whose mode is
    /// word 210 of QUUX's register page (contract Q13), reached as register
    /// 0 by [`Tv::read_control`] and [`Tv::write_control`].
    pub fn control_registers(&self) -> u8 {
        match self.board {
            Board::Video => 0,
            _ => 0xff,
        }
    }

    /// Words of its frame buffer: the CADR boards' 32K, or the video controller's screen.
    pub fn buffer_words(&self) -> u32 {
        match self.board {
            Board::Video => {
                let (_, h, wpl) = self.screen();
                (h * wpl) as u32
            }
            _ => BUFFER_WORDS,
        }
    }

    /// The word offset into this board's frame buffer a physical address
    /// names, if it is in it: the strap's start and the board's size.
    pub fn buffer_offset(&self, phys: u32) -> Option<u32> {
        let off = phys.wrapping_sub(self.strap.buffer);
        (off < self.buffer_words()).then_some(off)
    }

    /// The color map as the software has written it, `[color][channel]`:
    /// sixteen colors of three channels, red, green and blue. Written on
    /// either board, both having the circuit that strobes it.
    ///
    /// **These are the bytes written, not brightnesses.**
    /// `WRITE-COLOR-MAP` in `sys/window/color.lisp` writes `377 - value`
    /// --- "R, G and B (red, green and blue) are numbers from 0 to 377
    /// that together say how pixels containing LOC should appear", stored
    /// inverted --- and what the D-A makes of the byte is off this board
    /// and **unverified**: `lmtv.order` says only "the color map is a 64x9
    /// RAM for each channel, with a D-A on it", the RAMs and the D-As are
    /// past the paddle connections ECO 2 of `cadrtv/lmtv4b.eco` rewires
    /// ("these wires are from paddles to old or new Outs"), and no drawing
    /// of them reached us: `cadrtv/lmtv.book`, the board's own print list,
    /// is the 25 sheets and eight text files of the board itself and
    /// nothing of the paddles. What would settle it is a drawing or a
    /// parts list of them. Nothing is rendered from this map here.
    pub fn color_map(&self) -> &[[u8; CHANNELS]; COLORS] {
        &self.color_map
    }

    /// The program the board is running, as the model runs it: `None` while
    /// what is loaded makes no frame.
    pub fn timeline(&self) -> Option<&Timeline> {
        self.timeline.as_ref()
    }

    /// When the running program last started from location 0.
    pub fn origin(&self) -> u64 {
        self.origin
    }

    /// The program or the clock mode changed at `ns`: the program is run
    /// afresh from location 0 there. **Unverified** that the board starts
    /// over rather than fetching the new program from wherever its address
    /// counter stood: the 74LS569s at NSYADR are cleared only by `-SYNC ADR
    /// CLR`, and what settles it is the phase of `-TVMA CLR` on the netlist
    /// across a `SETUP-CPT`. Nothing in the software depends on the phase.
    ///
    /// **The vertical flag and the mode register's sync bits are carried
    /// over**, the start of the program reaching neither part: the flag is
    /// the 74LS74 at NXBCTL 0E14, whose preset is `-TVMA CLR`, clock
    /// `-LOAD MODE` with `XDI4` as data and clear `-RESET`; the sync bits
    /// are the 74LS175 at NSYREG 0D02, whose clear, pin 1, is a pull-up
    /// (`HI` at XBADR 0F10 on the SIMPLE TV, `HI5` at XBADR 0D04 on the
    /// LISPM TV) and whose clock is `-CLK`. So the flag keeps its value,
    /// and the register keeps its bits until the new program's first
    /// instruction lands.
    fn restart(&mut self, ns: u64) {
        let flag = self.vert_flag(ns);
        self.sync_held = self.sync_at(ns);
        self.timeline = Timeline::of(self.sync.program(), self.mode & mode::CLOCK);
        self.origin = ns;
        self.flag_written_at(flag, ns);
    }

    /// The flag's flop as of `ns`: `flag` in it, and the next `-TVMA CLR`
    /// to preset it looked up once, under the program running from
    /// `origin`. Every write of the flop comes through here, so that the
    /// instant is never stale.
    fn flag_written_at(&mut self, flag: bool, ns: u64) {
        self.flag_written = flag;
        self.written_at = ns;
        self.next_clr = match &self.timeline {
            Some(t) => {
                self.origin.saturating_add(t.next_tvma_clr_after(ns.saturating_sub(self.origin)))
            }
            None => u64::MAX,
        };
    }

    /// The sync bits the program has in the register at `ns`: `(hsync,
    /// vsync)`. Before the running program's first instruction has landed
    /// they are still the ones the program before it left in the register,
    /// which the model holds from the restart.
    pub fn sync_at(&self, ns: u64) -> (bool, bool) {
        match &self.timeline {
            Some(t) if ns >= self.origin => t.sync_at_since_start(ns - self.origin, self.sync_held),
            _ => (false, false),
        }
    }

    /// The whole frame buffer, for whatever draws it.
    pub fn buffer(&self) -> &[u32] {
        &self.buffer
    }

    pub fn mode(&self) -> u32 {
        self.mode
    }

    /// One bits are black when [`mode::BOW`] is set, and white otherwise.
    pub fn black_on_white(&self) -> bool {
        self.mode & mode::BOW != 0
    }

    /// Whether the pixel at `x`, `y` is lit, ignoring which way round the
    /// screen is showing them.
    pub fn pixel(&self, x: usize, y: usize) -> bool {
        let bit = y * self.screen().2 * 32 + x;
        self.buffer[bit / 32] >> (bit % 32) & 1 != 0
    }

    /// The four-bit pixel at `x`, `y` of the color picture: the color,
    /// which is an address into [`Tv::color_map`].
    ///
    /// `COLOR:MAKE-SCREEN` displaces an `ART-4B` array onto the buffer, so
    /// a pixel is an element of one: `sys/cold/qcom.lisp` gives `ART-4B`
    /// eight elements a word of four bits each, and
    /// `XCOLOR-TRANSFORM` in `sys/ucadr/uc-hacks.lisp` --- MIT's own
    /// microcode walking such an array over this very screen --- takes
    /// element `k` from bit `4 * (k mod 8)` of word `k / 8`
    /// (`((M-K) DPB M-J (BYTE-FIELD 3 2) A-ZERO) ;Rotation amount in
    /// bits`). The low nibble first, as `lmtv.order` has the low-order bit
    /// of a word sent to the TV first. Pixel `x` of line `y` is therefore
    /// nibble `x mod 8` of word `y * `[`COLOR_WORDS_PER_LINE`]` + x / 8`.
    pub fn pixel4(&self, x: usize, y: usize) -> u8 {
        let at = y * COLOR_WORDS_PER_LINE + x / 8;
        (self.buffer[at] >> (x % 8 * COLOR_BITS_PER_PIXEL)) as u8 & 0o17
    }

    /// What the monitor shows a pixel of color `color` as: the three
    /// guns, red, green and blue, from the map.
    ///
    /// **This is a property decision and not a fact about the D-A**, which
    /// is off the board and undocumented --- see [`Tv::color_map`]. The
    /// software's own model of the RAM is that it holds the complement:
    /// `WRITE-COLOR-MAP` stores `377 - value` and `READ-COLOR-MAP` hands
    /// back `377 - stored`, so the map that `R-G-B-COLOR-MAP` leaves for
    /// full red has zero in the red channel. So a stored byte is rendered
    /// as `255 - stored`, and the software's inversion is the only
    /// reference for it. **Unverified**: a drawing or a parts list of the
    /// paddles that carry the map RAMs and their D-As would settle what a
    /// stored byte really makes at the monitor.
    pub fn rgb(&self, color: usize) -> [u8; CHANNELS] {
        let mut out = [0; CHANNELS];
        for (gun, stored) in out.iter_mut().zip(self.color_map[color & (COLORS - 1)]) {
            *gun = u8::MAX - stored;
        }
        out
    }

    /// Whether the monitor shows the pixel at `x`, `y` white: a lit bit is
    /// white unless [`mode::BOW`] is set, and the other way round when it
    /// is.
    ///
    /// The one place that rule lives on this side, so that everything
    /// drawing this screen draws the same screen. `crate::terminal` states
    /// it again over its own frame, which may be the monitor's raster
    /// rather than this buffer, and `tests/terminal.rs` holds the two to
    /// each other pixel for pixel.
    pub fn shows_white(&self, x: usize, y: usize) -> bool {
        self.pixel(x, y) != self.black_on_white()
    }

    /// The screen as the monitor shows it, as a PNG: 768 by 963, one bit a
    /// pixel, a one white unless [`mode::BOW`] is set.
    ///
    /// The encoder is here rather than a crate: a 1-bit grayscale PNG is a
    /// header, the rows behind stored deflate blocks, and two checksums.
    pub fn png(&self) -> Vec<u8> {
        let (width, height, _) = self.screen();
        let mut raw = Vec::with_capacity(height * (width / 8 + 1));
        for y in 0..height {
            raw.push(0); // filter: none
            for x in (0..width).step_by(8) {
                let mut byte = 0u8;
                for b in 0..8 {
                    if self.shows_white(x + b, y) {
                        byte |= 0x80 >> b;
                    }
                }
                raw.push(byte);
            }
        }
        let mut z = vec![0x78, 0x01];
        let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
        for (i, b) in blocks.iter().enumerate() {
            z.push((i + 1 == blocks.len()) as u8);
            z.extend_from_slice(&(b.len() as u16).to_le_bytes());
            z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
            z.extend_from_slice(b);
        }
        z.extend_from_slice(&adler32(&raw).to_be_bytes());

        let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&(width as u32).to_be_bytes());
        ihdr.extend_from_slice(&(height as u32).to_be_bytes());
        ihdr.extend_from_slice(&[1, 0, 0, 0, 0]); // 1 bit, grayscale, deflate, none, no interlace
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", &z);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    /// How many pixels are lit.
    pub fn lit(&self) -> usize {
        let (_, height, words_per_line) = self.screen();
        self.buffer[..height * words_per_line].iter().map(|w| w.count_ones() as usize).sum()
    }

    pub fn read_buffer(&self, offset: u32) -> u32 {
        self.buffer[offset as usize]
    }

    pub fn write_buffer(&mut self, offset: u32, v: u32) {
        self.buffer[offset as usize] = v;
    }

    /// `VERT FLAG` at `ns`: what the last mode write put in the flop, or
    /// set, if a frame has started since --- `-TVMA CLR` presets it once
    /// every [`FRAME_NS`], the frames counted from power-on.
    pub fn vert_flag(&self, ns: u64) -> bool {
        self.board != Board::Video && (self.flag_written || ns >= self.next_clr)
    }

    /// `SEND INTR`: the vertical flag with [`mode::INTERRUPT_ENABLE`] up,
    /// which the board puts on `-XBUS.INTR` and the microcode takes at
    /// `INTRX0` as the 60-cycle clock.
    pub fn interrupt(&self, ns: u64) -> bool {
        self.mode & mode::INTERRUPT_ENABLE != 0 && self.vert_flag(ns)
    }

    /// The window system read-modify-writes this register, so what it writes
    /// has to read back; `INTRX0` reads it to find the vertical flag; and
    /// `SETUP-CPT` reads the sync program back through register 1.
    pub fn read_control(&self, register: u32, ns: u64) -> u32 {
        if self.board == Board::Video {
            // The mode, word 210: black-on-white and nothing else.
            return if register == 0 { self.mode & mode::BOW } else { 0 };
        }
        match register {
            0 => {
                let (hsync, vsync) = self.sync_at(ns);
                // Bit 7 is the sync enable read back on the LISPM TV, and
                // grounded on the SIMPLE TV: `mode::SYNC_PROM_ENABLE`.
                let prom_enable = self.board == Board::LispmTv && self.sync.enabled();
                self.mode
                    | if self.vert_flag(ns) { mode::VERT } else { 0 }
                    | if vsync { mode::VSYNC } else { 0 }
                    | if hsync { mode::HSYNC } else { 0 }
                    | if prom_enable { mode::SYNC_PROM_ENABLE } else { 0 }
            }
            // The sync program's word at the pointer, eight bits: the RAM's
            // while it is selected, the PROM's otherwise; `lmtv.order`
            // calls 31-8 garbage.
            1 => self.sync.program().get(self.sync.pointer as usize).copied().unwrap_or(0) as u32,
            // 2, 3 and 4 are write only --- `color.lisp` keeps
            // `HARDWARE-COLOR-MAP` in the band because "the hardware does
            // not allow reading back of the color map" --- and 5 to 7
            // "respond but don't do anything".
            _ => 0,
        }
    }

    /// The four pins of the 2519 land, bit 4 lands in the vertical flag's
    /// flop, the sync program's three registers take theirs, and register 4
    /// writes one byte of the color map; everything
    /// else the write carries has nowhere to be stored. A write
    /// that changes the program the generator runs --- the clock mode, the
    /// RAM's selection, or a word of the RAM while it is selected --- runs
    /// it afresh ([`Tv::restart`]).
    pub fn write_control(&mut self, register: u32, v: u32, ns: u64) {
        if self.board == Board::Video {
            // Only black-on-white is kept; the rest has nowhere to go.
            if register == 0 {
                self.mode = v & mode::BOW;
            }
            return;
        }
        match register {
            0 => {
                let clock_changed = (v ^ self.mode) & mode::CLOCK != 0;
                self.mode = v & mode::WRITABLE;
                self.flag_written_at(v & mode::VERT != 0, ns);
                if clock_changed {
                    self.restart(ns);
                }
            }
            1 => {
                self.sync.words[self.sync.pointer as usize] = v as u8;
                if self.sync.enabled() {
                    self.restart(ns);
                }
            }
            2 => self.sync.pointer = (v as u16) & (SYNC_RAM_WORDS as u16 - 1),
            3 => {
                let was = self.sync.enabled();
                self.sync.enable = v as u8;
                if self.sync.enabled() != was {
                    self.restart(ns);
                }
            }
            // The color register, `lmtv.order`'s "173777x4 Color (write
            // only), 15-8 Value to write into color map, 7-6 Select which
            // color map (up to 4 channels), 3-0 Color (i.e. address into
            // color map)". On the board, page COLOR: the 74LS244 at 0D13
            // puts `XDI0..7` on `COLOR 0..7` while `-LOAD COLOR` is low,
            // the 74S241 at 0E09 puts `XDI8..15` on `COLOR VALUE 0..7`,
            // and the 74S139 at 0E10 decodes `XDI6` and `XDI7` into
            // `-LOAD COLOR 0`, `1` and `2`, the three channels' write
            // strobes, its fourth output unconnected --- so a write naming
            // the fourth channel strobes nothing, which
            // `tests/lispmtv_netlist.rs` measures. The map RAMs are off
            // the board, so what is kept here is the byte written.
            //
            // **The SIMPLE TV has the same page**, `nracol`, titled
            // "SIMPLE TV / COLOR MAP" in `lmtv.stf` and part for part the
            // same circuit; the two differ only in the 74S257's select and
            // the 241's pull-up net, neither of which is this write.
            // `tests/simpletv_netlist.rs` measures that board strobing the
            // map with the same color and value, so the write is the same
            // here.
            //
            // **`XDI4` and `XDI5` leave the board too**, on `COLOR 4` and
            // `COLOR 5`, where a 64-entry map would take them as address;
            // `lmtv.order` gives the color four bits and `WRITE-COLOR-MAP`
            // writes `(LOGAND LOC 17)`, so MIT's own software never sets
            // them. What an off-board map does with them is
            // **unverified** --- see [`Tv::color_map`].
            4 => {
                let channel = (v >> 6) as usize & 3;
                if channel < CHANNELS {
                    self.color_map[v as usize & (COLORS - 1)][channel] = (v >> 8) as u8;
                }
            }
            // 5 to 7 "respond but don't do anything".
            _ => {}
        }
    }

    /// `-XBUS INIT` on the backplane at `ns`: `XBUS INIT IN` off the 26S10
    /// at XBDATA 0F15, inverted to `-RESET` by the 74S04 at NXBCTL 0F10,
    /// and `-RESET` clears one flop on this board --- the vertical flag's
    /// 74LS74 at NXBCTL 0E14, pin 13.  The mode register, the 25LS2519 at
    /// 0F12 (pin 19), and the sync enable and spacing, the 74LS273 at
    /// NTVINC 0A07 (pin 1), clear on `-POWER RESET` instead, the board's
    /// receiver of the backplane's `-XBUS POWER RESET`, a wire of its own
    /// that the interface drives from the cable's `-BUS.POWER.RESET`
    /// (OLORD2 1A06).  Power-on is the only processor event that raises it,
    /// so here only [`Tv::default`] clears them.  `-BUS.POWER.RESET`
    /// is the 74S37 at OLORD2 1A06 inverting `POWER RESET A`, and `POWER
    /// RESET A` is `-POWER RESET` inverted by the 74S02 at 1A11 (pin 8 on
    /// ground); `-POWER RESET` comes off the 74LS14 at 1A20 from the
    /// resistor-capacitor network at 1A19, which has nothing on it but VCC,
    /// ground and that Schmitt, so nothing the microcode does reaches it.
    /// `PROG.BUS.RESET`, the other input of the 74S02 at 1A07, is the
    /// programmed reset and goes the other way, to `-BUS.RESET`: it is
    /// bit 28 of the Interrupt Control register, the 25LS2519 at FLAG 3E08
    /// where `OB28` is `PROG.UNIBUS.RESET`, which AIM-528 describes as
    /// "Bit <28>, BUS-RESET, generates a RESET signal on the Unibus (BUS
    /// INIT L) and on the Xbus (XBUS.INIT L), and resets the bus interface,
    /// when it is written 1 and then 0.  The machine also resets the busses
    /// when it is powered up."  `-BUS.RESET` is the cable wire that becomes
    /// `-LM UNIBUS RESET` at DBGIN 0A14, and `-BUS.POWER.RESET` the one
    /// that becomes `-LM POWER RESET` at XA 0B13, which the 26S10 at XA
    /// 0F21 puts on `-XBUS POWER RESET`; no other part drives it.
    /// The flag is preset again by the next frame's `-TVMA CLR`.
    ///
    /// The sync program's phase is left alone. `lmtv.order` says "Control
    /// also gets to location 0 when the Xbus is reset", but the counters'
    /// clear, `-SYNC ADR CLR`, is the 74S10 at 0D01 on `-SYNC EOL`, `-SYNC
    /// NEW LINE` and a third input, the program's own end-of-loop logic;
    /// **unverified** whether that third input carries the reset, which
    /// what drives 0D03 pin 11 would settle.
    pub fn xbus_init(&mut self, ns: u64) {
        self.flag_written_at(false, ns);
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(!0u32, |c, &b| {
        (0..8).fold(c ^ b as u32, |c, _| if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 })
    })
}

fn adler32(bytes: &[u8]) -> u32 {
    let (a, b) = bytes.iter().fold((1u32, 0u32), |(a, b), &x| {
        let a = (a + x as u32) % 65521;
        (a, (b + a) % 65521)
    });
    b << 16 | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

// --- Checkpoints ------------------------------------------------------------

impl SyncRam {
    pub fn save(&self, w: &mut crate::checkpoint::Writer) {
        let SyncRam { words, pointer, enable } = self;
        w.bytes(words);
        w.u16(*pointer);
        w.u8(*enable);
    }

    pub fn load(&mut self, r: &mut crate::checkpoint::Reader) -> std::io::Result<()> {
        r.bytes_into(&mut self.words)?;
        // The pointer indexes `words`, and a write to register 2 keeps the
        // twelve bits that address the 2147s, so a wider one is a corrupt
        // checkpoint: refused here, not indexed with at the next access.
        let pointer = r.u16()?;
        if pointer as usize >= SYNC_RAM_WORDS {
            return Err(crate::checkpoint::bad(format!(
                "sync pointer {pointer:o}, past the {SYNC_RAM_WORDS} words its twelve bits address"
            )));
        }
        self.pointer = pointer;
        self.enable = r.u8()?;
        Ok(())
    }
}

impl Tv {
    /// The display into a checkpoint: which board it is, the frame buffer,
    /// the mode, the sync RAM, the color map and the vertical flag.
    pub fn save(&self, w: &mut crate::checkpoint::Writer) {
        // The strap is the backplane's and not the board's state: which
        // screen this is, is where the checkpoint keeps it --- the
        // machine's `tv` or its `color_tv` --- and `--color-tv` is refused
        // against the checkpoint by name, as `--tv-board` is.
        let Tv {
            board,
            video_size,
            strap: _,
            buffer,
            mode,
            sync,
            color_map,
            flag_written,
            written_at,
            // Looked up again at the load, from the program and the write.
            next_clr: _,
            timeline: _,
            origin,
            sync_held,
        } = self;
        w.u8(match board {
            Board::SimpleTv => 0,
            Board::LispmTv => 1,
            Board::Video => 2,
        });
        w.u16(video_size.0 as u16);
        w.u16(video_size.1 as u16);
        w.u32s(buffer);
        w.u32(*mode);
        sync.save(w);
        for color in color_map {
            for channel in color {
                w.u8(*channel);
            }
        }
        w.bool(*flag_written);
        w.u64(*written_at);
        w.u64(*origin);
        w.bool(sync_held.0);
        w.bool(sync_held.1);
    }

    /// The timeline is not in the checkpoint: it is the program and the
    /// clock mode run, and is run again here.
    pub fn load(&mut self, r: &mut crate::checkpoint::Reader) -> std::io::Result<()> {
        // The board the machine was built with, so that a resume onto the
        // other one is refused rather than run: `--tv-board`.
        let board = match r.u8()? {
            0 => Board::SimpleTv,
            1 => Board::LispmTv,
            2 => Board::Video,
            other => return Err(crate::checkpoint::bad(format!("display board {other}"))),
        };
        let size = (r.u16()? as usize, r.u16()? as usize);
        if board != self.board || size != self.video_size {
            self.video_size = size;
            self.set_board(board);
        }
        r.u32s_into(&mut self.buffer)?;
        self.mode = r.u32()?;
        self.sync.load(r)?;
        for color in &mut self.color_map {
            for channel in color {
                *channel = r.u8()?;
            }
        }
        self.flag_written = r.bool()?;
        self.written_at = r.u64()?;
        self.origin = r.u64()?;
        self.sync_held = (r.bool()?, r.bool()?);
        self.timeline = Timeline::of(self.sync.program(), self.mode & mode::CLOCK);
        // The next preset is not in the checkpoint: it is the program and the
        // write's instant looked up, and is looked up again here.
        let (flag, at) = (self.flag_written, self.written_at);
        self.flag_written_at(flag, at);
        Ok(())
    }
}
