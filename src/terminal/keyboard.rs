// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The keyboard on the I/O board's cable: MIT's "new keyboard" of November
//! 1980, the one with the 24-bit shift register.
//!
//! Three things are modeled and all are MIT's own. **The character on the
//! cable**, from the "Protocol documentation" section at the end of
//! `sys/io1/ukbd.lisp` in the System 100 release --- the 8748 firmware of
//! the keyboard itself: 24 bits, low-order first, a start bit that is low,
//! true-high data on a cable that is high when idle, source ID `001` in
//! bits 18-16 and ones in 23-19, and the up-down, all-keys-up and boot
//! codes laid out below. **The key positions**, from `KBD-MAKE-NEW-TABLE`
//! in `lmio/kbd.123` of the System 46 sources --- the table System 100
//! ships only as qfasl in the band --- cross-checked against the ten
//! positions `ukbd.lisp` names in passing, every one of which agrees:
//! mode lock 3, super 5 and 65, alt lock 15, control 20 and 26, rubout 23,
//! shift 24 and 25, greek 44 and 35, meta 45. `tests/keyboard.rs` holds
//! the table to those ten. **The boot sequence**, from the same
//! firmware's `check-boot` and `bootflag`: Controls and Metas held with
//! Rubout or Return send the boot word after the key-down, and no key-up
//! goes until the next key-down (`Keyboard::check_boot`); which Controls
//! and Metas it needs is the one setting in it, [`BootKeys`]. This is
//! **not** the Knight keyboard, which is source ID `111` with a different
//! word and is not modeled.
//!
//! **The keyboard sends positions and the machine does the shifting.**
//! `ukbd.lisp`: "All key-encoding, including hacking of shifts, will be
//! done in software in the central machine, not in the keyboard. Note that
//! both pressing and releasing a key send a code, therefore the central
//! machine knows the status of all keys." A VNC viewer sends the opposite
//! --- X11 keysyms with the shift already applied, `A` and not `shift, a`
//! --- so [`Keyboard::key`] has to find the position and plane that make
//! that character and hold or release the shift keys to match.
//!
//! **What a keysym means is the one piece of invention here, and it is
//! the user's to change.** No keyboard anyone has resembles this one: it
//! has 31 named keys and 11 shifting keys, and a host keyboard has no key
//! called Greek, Top or Hand Left. So which host key stands for which is
//! a choice rather than a fact, and it is [`Mapping`] --- data, read from
//! `default.keys` beside this file, and from the user's own on top of it.
//! Everything under it is MIT's.
//!
//! What reaches the I/O board is the same 24-bit word by either route:
//! [`Keyboard::deliver`] presses it into the behavioral board, and
//! `crate::terminal::cable` clocks it down the wire into the netlist one.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Source ID of the new keyboard, bits 18-16 of the word.
pub const SOURCE: u32 = 0o1;

/// The word with the reserved ones and the source in place and the
/// sixteen bits of information still to come: bits 23-19 "Reserved, must
/// be 1's", 18-16 the source.
const FRAME: u32 = 0o37 << 19 | SOURCE << 16;

/// Bit 8 of an up-down code: "1=key up, 0=key down".
pub const UP: u32 = 1 << 8;

/// Bit 15: "1 (indicates not an up-down code)", the all-keys-up word.
pub const ALL_KEYS_UP: u32 = 1 << 15;

/// A key going down or coming up, by position.
pub fn up_down(position: u8, up: bool) -> u32 {
    FRAME | if up { UP } else { 0 } | (position as u32 & 0o177)
}

/// The all-keys-up word: bit 15 set and one bit for each shifting key
/// still down, in the order `ukbd.lisp` gives --- shift 0, greek 1, top
/// 2, caps lock 3, control 4, meta 5, super 6, hyper 7, alt lock 8, mode
/// lock 9, repeat 10.
pub fn all_keys_up(shifts: u16) -> u32 {
    FRAME | ALL_KEYS_UP | (shifts as u32 & 0o3777)
}

/// The boot codes: "15-10 1, 9-6 0, 5-0 46 (octal) if cold, 62 (octal)
/// if warm". The I/O board decodes these itself, `ukbd.lisp` says ---
/// "bits 10-13 = 1, bits 6-9 = 0, and bit 16 = 1" --- and pulls `-BOOT*`.
pub fn boot(cold: bool) -> u32 {
    FRAME | 0o77 << 10 | if cold { 0o46 } else { 0o62 }
}

/// The two keys the boot sequence ends on, by position: Rubout for a cold
/// boot, Return for a warm one. `ukbd.lisp`'s `check-boot` gives them as
/// "rubout 23, return 136", and [`TABLE`] has them at the same positions;
/// `tests/keyboard.rs` holds the two to each other.
pub const RUBOUT: u8 = 0o23;
pub const RETURN: u8 = 0o136;

/// **The keys the boot sequence needs**, `--keyboard-boot`: how many
/// Controls and how many Metas have to be held with Rubout or Return.
///
/// The CADR keyboard's own sequence is both Controls and both Metas ---
/// `check-boot` in `ukbd.lisp` tests all four --- and a host keyboard
/// rarely has two of each free to map, so this is a setting, spelled as
/// the keys to hold: `ctrl,meta`, the default, is either Control and
/// either Meta, as Ctrl-Alt-Del is pressed; `ctrl,ctrl,meta,meta` is the
/// keyboard's own. `ctrl` is MIT's Control key and `meta` its Meta.
/// Rubout and Return are never in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootKeys {
    controls: u8,
    metas: u8,
}

impl Default for BootKeys {
    /// `ctrl,meta`.
    fn default() -> BootKeys {
        BootKeys { controls: 1, metas: 1 }
    }
}

impl BootKeys {
    /// The four spellings, which are the only four settings there are.
    pub const SPELLINGS: [&str; 4] =
        ["ctrl,meta", "ctrl,ctrl,meta", "ctrl,meta,meta", "ctrl,ctrl,meta,meta"];

    /// Reads a spelling: `ctrl` and `meta`, comma-separated, and nothing
    /// else, counted --- one of a word is either key of its pair, two is
    /// both --- in any order. Anything else is refused with the four
    /// named.
    pub fn parse(s: &str) -> Result<BootKeys, String> {
        let refused = || {
            let four: Vec<String> = Self::SPELLINGS.iter().map(|one| format!("`{one}`")).collect();
            format!(
                "{s:?} is not the keys the boot sequence needs: {}, {}, {} or {}",
                four[0], four[1], four[2], four[3]
            )
        };
        let (mut controls, mut metas) = (0u8, 0u8);
        for word in s.split(',') {
            match word.trim().to_ascii_lowercase().as_str() {
                "ctrl" => controls += 1,
                "meta" => metas += 1,
                _ => return Err(refused()),
            }
        }
        if !(1..=2).contains(&controls) || !(1..=2).contains(&metas) {
            return Err(refused());
        }
        Ok(BootKeys { controls, metas })
    }

    /// How many Controls have to be held: one is either, two is both.
    pub fn controls(&self) -> usize {
        self.controls as usize
    }

    /// How many Metas, likewise.
    pub fn metas(&self) -> usize {
        self.metas as usize
    }
}

/// The spelling [`BootKeys::parse`] reads, Controls first.
impl std::fmt::Display for BootKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let words: Vec<&str> = std::iter::repeat_n("ctrl", self.controls())
            .chain(std::iter::repeat_n("meta", self.metas()))
            .collect();
        write!(f, "{}", words.join(","))
    }
}

/// The shifting keys, as the bit each holds in the all-keys-up word and
/// in `KBD-SHIFTS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shift {
    Shift = 0,
    Greek = 1,
    Top = 2,
    CapsLock = 3,
    Control = 4,
    Meta = 5,
    Super = 6,
    Hyper = 7,
    AltLock = 8,
    ModeLock = 9,
    Repeat = 10,
}

/// MIT's own name for a shifting key, as the keyboard mapping writes it.
pub fn shift_name(s: Shift) -> &'static str {
    match s {
        Shift::Shift => "Shift",
        Shift::Greek => "Greek",
        Shift::Top => "Top",
        Shift::CapsLock => "Caps Lock",
        Shift::Control => "Control",
        Shift::Meta => "Meta",
        Shift::Super => "Super",
        Shift::Hyper => "Hyper",
        Shift::AltLock => "Alt Lock",
        Shift::ModeLock => "Mode Lock",
        Shift::Repeat => "Repeat",
    }
}

/// What a position on the keyboard is, in MIT's table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A character key: what it gives unshifted and shifted, planes 0 and
    /// 1 of the table. The other planes --- caps lock, top, greek --- are
    /// not needed to find a position for an ASCII keysym and are left out.
    Char(u8, u8),
    /// A key with a name and no ASCII character: Return, Rubout, Help.
    Named(&'static str),
    /// A shifting key.
    Shift(Shift),
    /// A position with no key, or one MIT's table leaves undefined.
    None,
}

/// MIT's table, by position in octal, transcribed from `KBD-MAKE-NEW-TABLE`
/// with its own comment on each line. A character entry is what the key
/// gives unshifted and with shift; where MIT's file has a character outside
/// ASCII in one of those two planes --- `plus-minus` at 21 --- the position
/// is [`Key::None`] here, because no keysym reaches it.
pub const TABLE: [Key; 128] = {
    use Key::*;
    let mut t = [None; 128];
    t[0o001] = Named("Roman II");
    t[0o002] = Named("Roman IV");
    t[0o003] = Shift(self::Shift::ModeLock);
    t[0o005] = Shift(self::Shift::Super); // Left super
    t[0o011] = Char(b'4', b'$');
    t[0o012] = Char(b'r', b'R');
    t[0o013] = Char(b'f', b'F');
    t[0o014] = Char(b'v', b'V');
    t[0o015] = Shift(self::Shift::AltLock);
    t[0o017] = Named("Hand Right");
    t[0o020] = Shift(self::Shift::Control); // Left control
    t[0o022] = Named("Tab");
    t[0o023] = Named("Rubout");
    t[0o024] = Shift(self::Shift::Shift); // Left Shift
    t[0o025] = Shift(self::Shift::Shift); // Right Shift
    t[0o026] = Shift(self::Shift::Control); // Right control
    t[0o030] = Named("Hold Output");
    t[0o031] = Char(b'8', b'*');
    t[0o032] = Char(b'i', b'I');
    t[0o033] = Char(b'k', b'K');
    t[0o034] = Char(b',', b'<');
    t[0o035] = Shift(self::Shift::Greek); // Right Greek
    t[0o036] = Named("Line");
    t[0o037] = Char(b'\\', b'|');
    t[0o040] = Named("Terminal");
    t[0o042] = Named("Network");
    t[0o044] = Shift(self::Shift::Greek); // Left Greek
    t[0o045] = Shift(self::Shift::Meta); // Left Meta
    t[0o046] = Named("Status");
    t[0o047] = Named("Resume");
    t[0o050] = Named("Clear Screen");
    t[0o051] = Char(b'6', b'^');
    t[0o052] = Char(b'y', b'Y');
    t[0o053] = Char(b'h', b'H');
    t[0o054] = Char(b'n', b'N');
    t[0o061] = Char(b'2', b'@');
    t[0o062] = Char(b'w', b'W');
    t[0o063] = Char(b's', b'S');
    t[0o064] = Char(b'x', b'X');
    t[0o065] = Shift(self::Shift::Super); // Right Super
    t[0o067] = Named("Abort");
    t[0o071] = Char(b'9', b'(');
    t[0o072] = Char(b'o', b'O');
    t[0o073] = Char(b'l', b'L');
    t[0o074] = Char(b'.', b'>');
    t[0o077] = Char(b'`', b'~');
    t[0o100] = Named("Macro");
    t[0o101] = Named("Roman I");
    t[0o102] = Named("Roman III");
    t[0o104] = Shift(self::Shift::Top); // Left Top
    t[0o106] = Named("Up Thumb");
    t[0o107] = Named("Call");
    t[0o110] = Named("Clear Input");
    t[0o111] = Char(b'5', b'%');
    t[0o112] = Char(b't', b'T');
    t[0o113] = Char(b'g', b'G');
    t[0o114] = Char(b'b', b'B');
    t[0o115] = Shift(self::Shift::Repeat);
    t[0o116] = Named("Help");
    t[0o117] = Named("Hand Left");
    t[0o120] = Named("Quote");
    t[0o121] = Char(b'1', b'!');
    t[0o122] = Char(b'q', b'Q');
    t[0o123] = Char(b'a', b'A');
    t[0o124] = Char(b'z', b'Z');
    t[0o125] = Shift(self::Shift::CapsLock);
    t[0o126] = Char(b'=', b'+');
    t[0o131] = Char(b'-', b'_');
    t[0o132] = Char(b'(', b'[');
    t[0o133] = Char(b'\'', b'"');
    t[0o134] = Char(b' ', b' ');
    t[0o136] = Named("Return");
    t[0o137] = Char(b')', b']');
    t[0o141] = Named("System");
    t[0o143] = Named("Alt Mode");
    t[0o145] = Shift(self::Shift::Hyper); // Left Hyper
    t[0o146] = Char(b'}', b'}'); // shifted is undefined in MIT's table
    t[0o151] = Char(b'7', b'&');
    t[0o152] = Char(b'u', b'U');
    t[0o153] = Char(b'j', b'J');
    t[0o154] = Char(b'm', b'M');
    t[0o155] = Shift(self::Shift::Top); // Right Top
    t[0o156] = Named("End");
    t[0o157] = Named("Delete");
    t[0o160] = Named("Overstrike");
    t[0o161] = Char(b'3', b'#');
    t[0o162] = Char(b'e', b'E');
    t[0o163] = Char(b'd', b'D');
    t[0o164] = Char(b'c', b'C');
    t[0o165] = Shift(self::Shift::Meta); // Right Meta
    t[0o166] = Char(b'{', b'{'); // shifted is undefined in MIT's table
    t[0o167] = Named("Break");
    t[0o170] = Named("Stop Output");
    t[0o171] = Char(b'0', b')');
    t[0o172] = Char(b'p', b'P');
    t[0o173] = Char(b';', b':');
    t[0o174] = Char(b'/', b'?');
    t[0o175] = Shift(self::Shift::Hyper); // Right Hyper
    t[0o176] = Named("Down Thumb");
    t
};

/// The position of a named key, if it has one.
pub fn named(name: &str) -> Option<u8> {
    TABLE.iter().position(|k| matches!(k, Key::Named(n) if *n == name)).map(|p| p as u8)
}

/// The positions of a shifting key: left and right where there are two.
pub fn shifting(s: Shift) -> Vec<u8> {
    (0..128u8).filter(|&p| TABLE[p as usize] == Key::Shift(s)).collect()
}

/// X11 keysyms, from `X11/keysymdef.h`, which is what RFC 6143 section
/// 7.5.4 says a `KeyEvent` carries. Only the ones this keyboard has a key
/// for.
pub mod keysym {
    pub const BACKSPACE: u32 = 0xff08;
    pub const TAB: u32 = 0xff09;
    pub const LINEFEED: u32 = 0xff0a;
    pub const RETURN: u32 = 0xff0d;
    pub const PAUSE: u32 = 0xff13;
    pub const ESCAPE: u32 = 0xff1b;
    pub const DELETE: u32 = 0xffff;
    pub const END: u32 = 0xff57;
    pub const CANCEL: u32 = 0xff69;
    pub const HELP: u32 = 0xff6a;
    pub const BREAK: u32 = 0xff6b;
    pub const KP_ENTER: u32 = 0xff8d;
    pub const F1: u32 = 0xffbe;
    pub const F12: u32 = 0xffc9;
    pub const SHIFT_L: u32 = 0xffe1;
    pub const SHIFT_R: u32 = 0xffe2;
    pub const CONTROL_L: u32 = 0xffe3;
    pub const CONTROL_R: u32 = 0xffe4;
    pub const CAPS_LOCK: u32 = 0xffe5;
    pub const META_L: u32 = 0xffe7;
    pub const META_R: u32 = 0xffe8;
    pub const ALT_L: u32 = 0xffe9;
    pub const ALT_R: u32 = 0xffea;
    pub const SUPER_L: u32 = 0xffeb;
    pub const SUPER_R: u32 = 0xffec;
    pub const HYPER_L: u32 = 0xffed;
    pub const HYPER_R: u32 = 0xffee;
}

/// The keys a viewer's function keys stand for, F1 to F12. The CADR has
/// no F keys; these are the keys on its top row that a PC keyboard has no
/// name for, and the order is ours. Nothing in MIT's sources says this.
pub const FUNCTION_KEYS: [&str; 12] = [
    "Terminal",
    "System",
    "Network",
    "Status",
    "Resume",
    "Abort",
    "Call",
    "Help",
    "Clear Input",
    "Clear Screen",
    "Break",
    "Quote",
];

/// The built-in mapping, read once: what [`positions`] and [`modifier`]
/// answer for, and what a run starts from before any file is read.
static BUILT_IN: LazyLock<Mapping> = LazyLock::new(Mapping::built_in);

/// The position a keysym is on in the **built-in** mapping, and whether
/// it wants the shift plane.  [`Mapping::positions`] is the same question
/// asked of the mapping a run is actually using.
pub fn positions(keysym: u32) -> Vec<(u8, bool)> {
    BUILT_IN.positions(keysym)
}

/// The shifting key a modifier keysym is in the built-in mapping, and
/// which of its two positions: left or right.
pub fn modifier(keysym: u32) -> Option<(Shift, usize)> {
    BUILT_IN.modifier(keysym)
}

// --- The keyboard mapping ---------------------------------------------------

/// muir's built-in keyboard mapping, in the form a user writes.
///
/// This is the whole of the mapping: [`Mapping::default`] is this text
/// read by the same parser a file goes through, so anything the default
/// says a file may say, and a file may replace any line of it.
pub const DEFAULT_MAPPING: &str = include_str!("default.keys");

/// What a viewer's keysyms mean on the Lisp Machine keyboard.
///
/// The key positions and the word on the cable are MIT's; this is not.
/// A viewer sends X11 keysyms from whatever keyboard the person has, and
/// the CADR has 31 named keys and 11 shifting keys that no such keyboard
/// has a key for, so which host key stands for which is a choice, and it
/// is the user's to make rather than muir's to settle.
///
/// Two kinds of binding:
///
/// - a **key**, one host keysym standing for one key;
/// - a **prefix**, a host keysym that sends nothing on its own and gives
///   the keysym after it a meaning of its own. There are more keys on
///   this keyboard than a host has spare, and a prefix is how the rest
///   are reached.
///
/// A printable ASCII keysym that no binding names is looked for on MIT's
/// table by the character it is, which is where the letters and digits
/// come from; nothing needs to bind those.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    /// A keysym on its own: the position it presses, and whether the
    /// character it wants is on the shifted plane.
    key: BTreeMap<u32, (u8, bool)>,
    /// A prefix keysym and the keysym after it.
    after: BTreeMap<(u32, u32), (u8, bool)>,
    /// Which file this was read from, for the run to say.
    source: Option<PathBuf>,
}

/// The built-in mapping: what a run uses when no file says otherwise.
impl Default for Mapping {
    fn default() -> Mapping {
        BUILT_IN.clone()
    }
}

impl Mapping {
    /// Nothing bound at all, which only [`Mapping::parse`] starts from.
    fn empty() -> Mapping {
        Mapping { key: BTreeMap::new(), after: BTreeMap::new(), source: None }
    }

    /// The built-in mapping alone.
    pub fn built_in() -> Mapping {
        Mapping::parse(DEFAULT_MAPPING).expect("muir's built-in keyboard mapping parses")
    }

    /// A mapping from `text` and nothing else --- what the built-in one
    /// is read by, and what a test uses to hold one binding on its own.
    pub fn parse(text: &str) -> Result<Mapping, String> {
        let mut m = Mapping::empty();
        m.read_into(text)?;
        Ok(m)
    }

    /// The built-in mapping with `text` over it: a line of the file
    /// replaces the binding for that keysym, and every keysym the file
    /// says nothing about keeps the one it had.
    pub fn read(text: &str) -> Result<Mapping, String> {
        let mut m = Mapping::built_in();
        m.read_into(text)?;
        Ok(m)
    }

    /// The mapping this run uses: the built-in one, with the file over it
    /// if there is one.  The file's path is kept so the run can say which
    /// it read.
    pub fn from_file(path: &Path) -> Result<Mapping, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut m = Mapping::read(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        m.source = Some(path.to_path_buf());
        Ok(m)
    }

    /// The file this mapping was read from, if it was read from one.
    pub fn source(&self) -> Option<&Path> {
        self.source.as_deref()
    }

    fn read_into(&mut self, text: &str) -> Result<(), String> {
        for (n, line) in text.lines().enumerate() {
            let at = |e: String| format!("line {}: {e}", n + 1);
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            let rest = rest.trim();
            match word {
                "key" => {
                    let (sym, key) = two(rest).map_err(&at)?;
                    let sym = keysym_of(sym).map_err(&at)?;
                    let bound = key_of(key).map_err(&at)?;
                    self.key.insert(sym, bound);
                }
                "prefix" => {
                    let (first, rest) = two(rest).map_err(&at)?;
                    let (second, key) = two(rest).map_err(&at)?;
                    let first = keysym_of(first).map_err(&at)?;
                    let second = keysym_of(second).map_err(&at)?;
                    let bound = key_of(key).map_err(&at)?;
                    self.after.insert((first, second), bound);
                }
                other => {
                    return Err(at(format!("{other} is not `key` or `prefix`")));
                }
            }
        }
        // A keysym is a key or a prefix, never both: the first press
        // would have to be two things at once.
        for (sym, _) in self.after.keys() {
            if self.key.contains_key(sym) {
                return Err(format!(
                    "{} is bound as a key and used as a prefix",
                    keysym_name(*sym)
                ));
            }
        }
        Ok(())
    }

    /// The position a keysym is on, and whether it wants the shift plane,
    /// as [`Keyboard::key`] asks.
    ///
    /// A binding first; failing that, a printable ASCII keysym is looked
    /// for on plane 0 and plane 1 of every character key of MIT's table,
    /// where a key may give it on either --- `(` being unshifted at 132
    /// and shifted at 71 --- and both are returned so that the one
    /// fitting the shift the viewer holds can be picked.
    pub fn positions(&self, keysym: u32) -> Vec<(u8, bool)> {
        if let Some(&bound) = self.key.get(&keysym) {
            return vec![bound];
        }
        character_positions(keysym)
    }

    /// The shifting key a keysym is, and which of its positions: 0 for
    /// the left of a pair, 1 for the right.
    pub fn modifier(&self, keysym: u32) -> Option<(Shift, usize)> {
        let &(position, _) = self.key.get(&keysym)?;
        let Key::Shift(s) = TABLE[position as usize] else { return None };
        let side = shifting(s).iter().position(|&p| p == position).unwrap_or(0);
        Some((s, side))
    }

    /// Whether a keysym sends nothing on its own and gives the keysym
    /// after it a meaning.
    pub fn is_prefix(&self, keysym: u32) -> bool {
        self.after.keys().any(|&(first, _)| first == keysym)
    }

    /// What `keysym` means after `prefix`.
    fn after_prefix(&self, prefix: u32, keysym: u32) -> Option<(u8, bool)> {
        self.after.get(&(prefix, keysym)).copied()
    }

    /// Every position any binding reaches: what can be typed at all.
    pub fn reachable(&self) -> BTreeSet<u8> {
        self.key.values().chain(self.after.values()).map(|&(p, _)| p).collect()
    }

    /// The mapping as a file `--keyboard-mapping` reads: every binding a
    /// line, in the format [`Mapping::parse`] takes.
    ///
    /// **The dump of a mapping parses back to that mapping**, so a run's
    /// mapping written out, fed back in unedited, changes nothing --- the
    /// property `tests/keyboard_mapping.rs` holds it to. That is what
    /// makes the output a starting point to edit rather than a report
    /// about the bindings.
    ///
    /// A key is written by its name where the name reads back as the same
    /// position and plane, and as `position <octal>` --- with `shifted`
    /// after it for the shifted plane --- where it does not.
    /// Two bindings need that. A character names the **first** position of
    /// MIT's table that gives it, and a character on two keys has one
    /// that is not first: `(` is shifted at 71 and unshifted at 132, a
    /// bare `(` reads back as 71 on the shifted plane, so the one at 132
    /// is written out. And a position MIT's table leaves unnamed --- 0 is
    /// the first --- has no other name at all.
    pub fn dump(&self) -> String {
        let key = key_written;
        let mut s = String::new();
        s.push_str("# muir's keyboard mapping, as --keyboard-mapping reads it:\n");
        s.push_str("# `key <keysym> <key>`, and `prefix <keysym> <keysym> <key>` for a\n");
        s.push_str("# key reached by pressing one and then another. A keysym is one\n");
        s.push_str("# word and the key is the rest of the line, which is why `Alt Mode`\n");
        s.push_str("# and `Left Control` need no quoting. A file goes over this rather\n");
        s.push_str("# than replacing it, so an edited copy of this file says the same\n");
        s.push_str("# thing with the edit in it.\n");
        for (sym, &(p, shifted)) in &self.key {
            s.push_str(&format!("key {} {}\n", keysym_name(*sym), key(p, shifted)));
        }
        for (&(first, second), &(p, shifted)) in &self.after {
            let (first, second) = (keysym_name(first), keysym_name(second));
            s.push_str(&format!("prefix {first} {second} {}\n", key(p, shifted)));
        }
        s
    }

    /// The mapping in force, a line a binding, for a user who cannot type
    /// a key and wants to know what would.
    pub fn show(&self) -> String {
        let mut s = String::new();
        for (sym, &(p, shifted)) in &self.key {
            s.push_str(&format!("  {:<18} {}\n", keysym_name(*sym), key_name(p, shifted)));
        }
        for (&(first, second), &(p, shifted)) in &self.after {
            s.push_str(&format!(
                "  {:<18} {}\n",
                format!("{} {}", keysym_name(first), keysym_name(second)),
                key_name(p, shifted)
            ));
        }
        s
    }
}

/// **A key as a mapping file writes it**, which is its name where
/// [`key_of`] reads that name back as the same position *and* the same
/// plane, and `position <octal>` --- with `shifted` after it --- where it
/// does not.
///
/// [`Mapping::dump`] writes its bindings with this and so does
/// `--keyboard-mapping-trace`, from here rather than each its own way.
/// That is the point of the trace: what it says a keysym became is what a
/// `key` line has to say to bind it there, so a traced line can be pasted
/// into a mapping file without translating anything. Two spellings for one
/// key would defeat it.
fn key_written(p: u8, shifted: bool) -> String {
    let name = key_name(p, shifted);
    match key_of(&name) {
        Ok(back) if back == (p, shifted) => name,
        _ => position_name(p, shifted),
    }
}

/// A printable ASCII keysym on MIT's table, by the character it is.
fn character_positions(keysym: u32) -> Vec<(u8, bool)> {
    if !(0x20..=0x7e).contains(&keysym) {
        return Vec::new();
    }
    let c = keysym as u8;
    let mut out = Vec::new();
    for (p, k) in TABLE.iter().enumerate() {
        if let Key::Char(plain, shifted) = k {
            if *plain == c {
                out.push((p as u8, false));
            }
            if *shifted == c && shifted != plain {
                out.push((p as u8, true));
            }
        }
    }
    out
}

/// A line's first word and the rest of it, both wanted.
fn two(rest: &str) -> Result<(&str, &str), String> {
    match rest.split_once(char::is_whitespace) {
        Some((a, b)) if !b.trim().is_empty() => Ok((a, b.trim())),
        _ => Err(format!("wants a keysym and what it means, not {rest:?}")),
    }
}

/// A keysym as the mapping writes it: an X11 name, a single printable
/// character, or a number in decimal or `0x` hexadecimal.
fn keysym_of(word: &str) -> Result<u32, String> {
    if let Some(&(_, sym)) = KEYSYM_NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(word)) {
        return Ok(sym);
    }
    let mut chars = word.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && (' '..='~').contains(&c)
    {
        return Ok(c as u32);
    }
    if let Some(hex) = word.strip_prefix("0x") {
        return u32::from_str_radix(hex, 16).map_err(|_| format!("{word} is no keysym"));
    }
    word.parse::<u32>().map_err(|_| format!("{word} is no keysym"))
}

/// A key as the mapping writes it: one of MIT's names for a named key, a
/// shifting key by name with `Left` or `Right` before it where there are
/// two, or the character a character key gives.
fn key_of(word: &str) -> Result<(u8, bool), String> {
    if let Some(p) =
        TABLE.iter().position(|k| matches!(k, Key::Named(n) if n.eq_ignore_ascii_case(word)))
    {
        return Ok((p as u8, false));
    }
    if let Some(rest) = strip_word(word, "position") {
        return position_of(rest);
    }
    let (side, name) = match word.split_once(char::is_whitespace) {
        Some((first, rest)) if first.eq_ignore_ascii_case("left") => (0, rest.trim()),
        Some((first, rest)) if first.eq_ignore_ascii_case("right") => (1, rest.trim()),
        _ => (0, word),
    };
    if let Some(s) = SHIFTS.iter().find(|s| shift_name(**s).eq_ignore_ascii_case(name)) {
        let at = shifting(*s);
        let p = at.get(side).or(at.first()).copied();
        return p.map(|p| (p, false)).ok_or_else(|| format!("{name} is on no position"));
    }
    let mut chars = word.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        let found = character_positions(c as u32);
        if let Some(&bound) = found.first() {
            return Ok(bound);
        }
    }
    Err(format!("{word} is no key of this keyboard"))
}

/// `word` with `first` taken off the front, if that is its first word.
fn strip_word<'a>(word: &'a str, first: &str) -> Option<&'a str> {
    let (a, rest) = word.split_once(char::is_whitespace)?;
    a.eq_ignore_ascii_case(first).then(|| rest.trim())
}

/// A position written exactly: its number on MIT's table in octal, and
/// `shifted` after it for a binding that wants the shifted plane of a
/// character key.
///
/// This is what [`Mapping::dump`] falls back to, and the only spelling
/// that names every binding: a character names the first position that
/// gives it, which is not always the one bound, and a position MIT's
/// table leaves unnamed has no other name at all.
fn position_of(rest: &str) -> Result<(u8, bool), String> {
    let (number, shifted) = match rest.split_once(char::is_whitespace) {
        Some((n, s)) if s.trim().eq_ignore_ascii_case("shifted") => (n, true),
        Some(_) => return Err(format!("position {rest}: `shifted` or nothing after the number")),
        None => (rest, false),
    };
    let p = u8::from_str_radix(number, 8)
        .map_err(|_| format!("position {number}: the number is in octal"))?;
    if usize::from(p) >= TABLE.len() {
        return Err(format!("position {number}: the table is {} positions", TABLE.len()));
    }
    Ok((p, shifted))
}

/// A position as [`position_of`] reads it back.
fn position_name(position: u8, shifted: bool) -> String {
    let plane = if shifted { " shifted" } else { "" };
    format!("position {position:o}{plane}")
}

/// What to call a position, the way the mapping writes it.
fn key_name(position: u8, shifted: bool) -> String {
    match TABLE[position as usize] {
        Key::Named(n) => n.to_string(),
        Key::Shift(s) => {
            let at = shifting(s);
            let side = at.iter().position(|&p| p == position).unwrap_or(0);
            if at.len() > 1 {
                format!("{} {}", if side == 0 { "Left" } else { "Right" }, shift_name(s))
            } else {
                shift_name(s).to_string()
            }
        }
        Key::Char(plain, up) => (if shifted { up } else { plain } as char).to_string(),
        Key::None => format!("position {position:o}"),
    }
}

/// What to call a keysym: its X11 name, the character it is, or its
/// number.
fn keysym_name(keysym: u32) -> String {
    if let Some(&(name, _)) = KEYSYM_NAMES.iter().find(|(_, s)| *s == keysym) {
        return name.to_string();
    }
    match char::from_u32(keysym) {
        Some(c) if (' '..='~').contains(&c) => c.to_string(),
        _ => format!("{keysym:#x}"),
    }
}

/// The eleven shifting keys, for reading a name back.
const SHIFTS: [Shift; 11] = [
    Shift::Shift,
    Shift::Greek,
    Shift::Top,
    Shift::CapsLock,
    Shift::Control,
    Shift::Meta,
    Shift::Super,
    Shift::Hyper,
    Shift::AltLock,
    Shift::ModeLock,
    Shift::Repeat,
];

/// The X11 keysym names the mapping understands, from `X11/keysymdef.h`:
/// the keys a host keyboard has that this one might want, and the
/// function keys and arrows a mapping is likely to reach for.  A keysym
/// with no name here is still written as a number.
const KEYSYM_NAMES: &[(&str, u32)] = &[
    // **The space, and it has to be here.** A keysym is written as one
    // word, and `0x20` is the one keysym whose single-character spelling
    // is whitespace: without a name it dumps as a bare space and the line
    // it is on cannot be read back. X11 calls it `space`, and naming it
    // is what makes "a keysym never contains whitespace" true of every
    // keysym rather than of the ones anybody had tried --- the other two
    // spellings, an X11 name and a number, cannot contain any.
    ("space", 0x20),
    ("BackSpace", 0xff08),
    ("Tab", 0xff09),
    ("Linefeed", 0xff0a),
    ("Return", 0xff0d),
    ("Pause", 0xff13),
    ("Scroll_Lock", 0xff14),
    ("Escape", 0xff1b),
    ("Home", 0xff50),
    ("Left", 0xff51),
    ("Up", 0xff52),
    ("Right", 0xff53),
    ("Down", 0xff54),
    ("Prior", 0xff55),
    ("Next", 0xff56),
    ("End", 0xff57),
    ("Begin", 0xff58),
    ("Print", 0xff61),
    ("Insert", 0xff63),
    ("Menu", 0xff67),
    ("Cancel", 0xff69),
    ("Help", 0xff6a),
    ("Break", 0xff6b),
    ("Mode_switch", 0xff7e),
    ("Num_Lock", 0xff7f),
    ("KP_Enter", 0xff8d),
    ("F1", 0xffbe),
    ("F2", 0xffbf),
    ("F3", 0xffc0),
    ("F4", 0xffc1),
    ("F5", 0xffc2),
    ("F6", 0xffc3),
    ("F7", 0xffc4),
    ("F8", 0xffc5),
    ("F9", 0xffc6),
    ("F10", 0xffc7),
    ("F11", 0xffc8),
    ("F12", 0xffc9),
    ("F13", 0xffca),
    ("F14", 0xffcb),
    ("F15", 0xffcc),
    ("Shift_L", 0xffe1),
    ("Shift_R", 0xffe2),
    ("Control_L", 0xffe3),
    ("Control_R", 0xffe4),
    ("Caps_Lock", 0xffe5),
    ("Meta_L", 0xffe7),
    ("Meta_R", 0xffe8),
    ("Alt_L", 0xffe9),
    ("Alt_R", 0xffea),
    ("Super_L", 0xffeb),
    ("Super_R", 0xffec),
    ("Hyper_L", 0xffed),
    ("Hyper_R", 0xffee),
    ("ISO_Level3_Shift", 0xfe03),
    ("Delete", 0xffff),
];

/// How many words wait to go down the cable while the software is not
/// reading the keyboard.  The keyboard's own firmware has a shift register
/// and the `DONE` that says it has been sent, and no queue --- `ukbd.lisp`
/// --- so any queue is the terminal's, and one that grew for as long as
/// the software left `764100` unread would be a leak rather than a
/// feature.  256 words is 128 keystrokes.  Beyond it a press is refused
/// whole and leaves nothing down ([`Keyboard::key`]); a release always
/// goes, because the machine tracks every key from the stream ---
/// `ukbd.lisp`: "both pressing and releasing a key send a code, therefore
/// the central machine knows the status of all keys" --- and a key-down it
/// has read whose key-up never follows is a key held for the rest of the
/// run.  So the queue holds at most this many words of presses, a few more
/// where a shift is worked around a key, and one up-code for each key
/// down.  [`super::cable::OnCable`] holds as many again on the wire.
pub const BACKLOG: usize = 256;

/// The keyboard: the keys a viewer is holding, and the words waiting to go
/// down the cable.
///
/// The keyboard's own firmware sends one character and waits for the I/O
/// board's `DONE` before the next --- `ukbd.lisp`: "DONE is 1 if the shift
/// register has been sent off to the host computer" --- so words queue
/// here and go one at a time as the board takes them, [`BACKLOG`] of them
/// at the most.
#[derive(Default)]
pub struct Keyboard {
    /// Words to send, oldest first.
    queue: VecDeque<u32>,
    /// Positions the viewer has down, so that a key up is sent for each
    /// and a shift the viewer holds is not sent twice.
    down: Vec<u8>,
    /// Which keysym holds which of those positions, and on which plane:
    /// a key's release, and its repeats, go to the position its press
    /// went to, whatever shift is held by then. `(` and `)` are each on
    /// two keys, and the shift held at release would otherwise choose the
    /// other one and leave the pressed key down. **Part of `down`, not a
    /// piece of [`Keyboard::resolve`]'s state**: every entry's position is
    /// down, and [`Keyboard::release`] drops the entries of the position
    /// it lets go.
    held: Vec<(u32, u8, bool)>,
    /// What a viewer's keysyms mean here.
    map: Mapping,
    /// A prefix keysym pressed and not yet answered: the next keysym is
    /// looked up behind it rather than on its own.
    prefix: Option<u32>,
    /// Shifting keys held for the one key that follows them, which is
    /// what a prefix naming a shifting key does.
    latched: Vec<u8>,
    /// Keysyms whose next release is to be dropped: a key the terminal
    /// has already sent whole, tapped rather than held.
    tapped: Vec<u32>,
    /// `--keyboard-mapping-trace`: say what every keysym arrived as and
    /// what it became.
    trace: bool,
    /// Keystrokes the queue had no room for, over the run: what
    /// [`Keyboard::refused`] hands back.
    refused: usize,
    /// The keys the boot sequence needs, `--keyboard-boot`.
    boot_keys: BootKeys,
    /// The firmware's `bootflag`: the boot word has gone, and no key-up
    /// goes until the next key-down. **Not a fourth piece of
    /// [`Keyboard::resolve`]'s state**: `resolve` never reads it. It is
    /// the firmware's, under the mapping, read and written in the two
    /// places a word is queued --- [`Keyboard::queue_down`] and
    /// [`Keyboard::queue_up`] --- which is where the firmware keeps it.
    hold_back: bool,
    /// What the firmware's part did with the last key, for the trace.
    firmware: Option<Firmware>,
}

/// **What a keysym became**, which is what `--keyboard-mapping-trace`
/// prints and the only thing that says which half of a key's journey is
/// wrong. A viewer chooses the keysym it sends for a physical key --- RFC
/// 6143 leaves that to it --- so muir is the only authority on which
/// keysym arrived, and the mapping is the only authority on what it meant.
///
/// A key is named as [`key_written`] names it, which is the spelling
/// [`Mapping::dump`] uses, so a traced line says what a `key` line would
/// have to say.
enum Went {
    /// The mapping has nothing for it. The answer to "why does this key do
    /// nothing".
    Unbound,
    /// Held as a prefix: nothing goes down the cable until the keysym after
    /// it. **Saying so is the point** --- a prefix's press produces no key
    /// by design, and printing nothing for it would look exactly like
    /// [`Went::Unbound`].
    HeldAsPrefix,
    /// The prefix pressed again, which is the way out of a sequence begun
    /// by mistake.
    PrefixLetGo,
    /// Looked up behind a standing prefix, and what was there.
    Behind(u32, Option<(u8, bool)>),
    /// Sent to a key: its position, the plane wanted, and whether the
    /// terminal had to work the shift around it rather than the viewer's
    /// own shift already giving that plane.
    Sent { p: u8, shifted: bool, tapped: bool },
    /// Found on a key and refused there: the queue was full, [`BACKLOG`]
    /// words the machine has not read, and the press was refused whole.
    /// **Said, and not folded into [`Went::Sent`]**: a keystroke refused
    /// here is a character that does not type, and a trace that called
    /// it sent would be asserting the opposite of what happened to the
    /// one person reading it for exactly this.
    Refused { p: u8, shifted: bool },
    /// Nothing went down the cable, and why. Every one of these is by
    /// design rather than a mapping that is short of a line.
    Nothing(&'static str),
}

impl std::fmt::Display for Went {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Went::Unbound => write!(f, "no binding"),
            Went::HeldAsPrefix => {
                write!(f, "held as a prefix; the keysym after it is looked up behind it")
            }
            Went::PrefixLetGo => write!(f, "the prefix is let go, and nothing is sent"),
            Went::Behind(first, None) => {
                write!(f, "behind {}: no binding", keysym_name(first))
            }
            Went::Behind(first, Some((p, shifted))) => {
                write!(f, "behind {}: {}", keysym_name(first), key_written(p, shifted))
            }
            Went::Sent { p, shifted, tapped: false } => write!(f, "{}", key_written(p, shifted)),
            Went::Sent { p, shifted, tapped: true } => {
                write!(f, "{}, tapped with the shift worked around it", key_written(p, shifted))
            }
            Went::Refused { p, shifted } => write!(
                f,
                "{} refused: the queue is full, {BACKLOG} words the machine has not read",
                key_written(p, shifted)
            ),
            Went::Nothing(why) => write!(f, "nothing: {why}"),
        }
    }
}

/// **What the keyboard's own firmware did with a key, over and above
/// the mapping**: `check-boot` and `bootflag` in `ukbd.lisp`, which act
/// on the words after the mapping has chosen them. Said on the trace's
/// line after [`Went`], because a key-up held back did nothing by design
/// and a line calling it sent would be asserting the opposite.
enum Firmware {
    /// The boot sequence was complete after this key-down, and the boot
    /// word went after its own: cold with Rubout, warm with Return.
    Boot { cold: bool },
    /// The key-up was held back: `bootflag` is set, and no key-up goes
    /// until the next key-down, so that the machine reads the boot word
    /// before anything else.
    HeldBack,
}

impl std::fmt::Display for Firmware {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Firmware::Boot { cold } => write!(
                f,
                ", and the boot sequence is complete: the {} boot word goes after it",
                if cold { "cold" } else { "warm" }
            ),
            Firmware::HeldBack => write!(
                f,
                " held back: no key-up goes until the next key-down, so that the machine \
                 reads the boot word first"
            ),
        }
    }
}

impl Keyboard {
    /// A keyboard on the built-in mapping.
    pub fn new() -> Keyboard {
        Keyboard::with_mapping(BUILT_IN.clone())
    }

    /// A keyboard on `map`.
    pub fn with_mapping(map: Mapping) -> Keyboard {
        Keyboard { map, ..Keyboard::default() }
    }

    /// The mapping this keyboard is using.
    pub fn mapping(&self) -> &Mapping {
        &self.map
    }

    /// `--keyboard-mapping-trace`: print every keysym as it arrives and
    /// what it became, **on stderr**, as every trace in muir does ---
    /// `--chaos-trace` writes there too --- and because this one runs
    /// alongside the machine where the prompt owns stdout.
    pub fn traced(&mut self, on: bool) {
        self.trace = on;
    }

    /// The keys the boot sequence needs.
    pub fn boot_keys(&self) -> BootKeys {
        self.boot_keys
    }

    /// `--keyboard-boot`: the keys the boot sequence needs from now on.
    pub fn set_boot_keys(&mut self, keys: BootKeys) {
        self.boot_keys = keys;
    }

    /// Whether a shifting key is down, at either of its positions.
    fn holding(&self, s: Shift) -> bool {
        shifting(s).iter().any(|p| self.down.contains(p))
    }

    /// A key-down's code onto the queue. Every one clears the firmware's
    /// `bootflag` --- `check-boot`'s `not-boot` path does, after every
    /// key-down --- and [`Keyboard::check_boot`] sets it again from
    /// [`Keyboard::press`] when the key completes the sequence.
    fn queue_down(&mut self, position: u8) {
        self.queue.push_back(up_down(position, false));
        self.hold_back = false;
    }

    /// A key-up's code onto the queue, unless the firmware's `bootflag`
    /// holds it back: `ukbd.lisp`, "If booting, don't send key-up codes".
    fn queue_up(&mut self, position: u8) {
        if self.hold_back {
            self.firmware = Some(Firmware::HeldBack);
            return;
        }
        self.queue.push_back(up_down(position, true));
    }

    /// **The firmware's `check-boot`**, run after every key-down that is
    /// held: with the Controls and Metas the setting asks for down,
    /// Rubout down sends the cold boot word and Return down the warm
    /// one, Rubout tested first as the firmware tests it; then `bootflag`
    /// is set, and no key-up goes until the next key-down --- "This gives
    /// the machine time to load microcode and read the character to see
    /// whether it is a warm or cold boot, before sending any other
    /// characters, such as up-codes."
    ///
    /// Held keys only: a key tapped rather than held --- behind a prefix,
    /// or with the shift worked around it --- is not down here and does
    /// not complete the sequence. The firmware compares whole bytes of
    /// its bit map, so on the keyboard itself another key down in the
    /// same byte as one of the four --- a Shift, at 24 or 25 beside the
    /// Controls --- defeats the sequence; here only the keys named count.
    fn check_boot(&mut self) {
        let held = |s: Shift| shifting(s).iter().filter(|p| self.down.contains(p)).count();
        if held(Shift::Control) < self.boot_keys.controls()
            || held(Shift::Meta) < self.boot_keys.metas()
        {
            return;
        }
        let cold = if self.down.contains(&RUBOUT) {
            true
        } else if self.down.contains(&RETURN) {
            false
        } else {
            return;
        };
        self.queue.push_back(boot(cold));
        self.hold_back = true;
        self.firmware = Some(Firmware::Boot { cold });
    }

    /// `position` down, if it is up and the queue has room.  A press the
    /// queue has no room for is refused whole, and the key stays up here
    /// too, so that no release is owed for it.
    ///
    /// Whether the key is down for the machine after this: so, too, for a
    /// key the viewer already had down, since the machine has that press
    /// or will; not so only for the press the queue refused.
    fn press(&mut self, position: u8) -> bool {
        if self.down.contains(&position) {
            return true;
        }
        if self.queue.len() >= BACKLOG {
            return false;
        }
        self.down.push(position);
        self.queue_down(position);
        self.check_boot();
        true
    }

    /// `position` up, if it is down.  Always queued, unless the boot
    /// sequence holds it back: the machine has read the key going down,
    /// or will.
    fn release(&mut self, position: u8) {
        if let Some(k) = self.down.iter().position(|&p| p == position) {
            self.down.remove(k);
            self.held.retain(|&(_, p, _)| p != position);
            self.queue_up(position);
        }
    }

    /// The key at `position` pressed and released at once, with the
    /// Shift key worked around it when the plane it wants is not the one
    /// the viewer is holding.
    ///
    /// The machine sees shift, key, and shift back, which is what a
    /// typist would have done. Refused whole beyond the backlog, as a
    /// plain press is, so that it leaves nothing down; whether it went.
    fn tap(&mut self, position: u8, wants_shift: bool) -> bool {
        if self.queue.len() >= BACKLOG {
            return false;
        }
        let shift = shifting(Shift::Shift)[0];
        let holding = self.holding(Shift::Shift);
        if wants_shift && !holding {
            self.queue_down(shift);
            self.queue_down(position);
            self.queue_up(position);
            self.queue_up(shift);
        } else if !wants_shift && holding {
            // Every shift the viewer holds comes up around the key.
            let held: Vec<u8> =
                shifting(Shift::Shift).into_iter().filter(|q| self.down.contains(q)).collect();
            for &q in &held {
                self.queue_up(q);
            }
            self.queue_down(position);
            self.queue_up(position);
            for &q in &held {
                self.queue_down(q);
            }
        } else {
            self.queue_down(position);
            self.queue_up(position);
        }
        true
    }

    /// A key the mapping named behind a prefix, or under a latched
    /// shifting key: a shifting key is held for the one key that follows
    /// it, anything else is tapped. Whether it went: a shifting key the
    /// queue refused is not latched either, there being nothing down to
    /// hold for the key after it.
    fn behind_prefix(&mut self, position: u8, wants_shift: bool) -> bool {
        if let Key::Shift(_) = TABLE[position as usize] {
            let went = self.press(position);
            if went {
                self.latched.push(position);
            }
            went
        } else {
            self.tap(position, wants_shift)
        }
    }

    /// A key from the viewer, by X11 keysym, going down or coming up.
    ///
    /// A modifier is pressed or released at its own position and nothing
    /// more. A character is found on the keyboard by
    /// [`Mapping::positions`] and sent as the position whose plane
    /// matches the shift the viewer is holding; when no position does ---
    /// `!` with no shift held, or `(` with it held and only the unshifted
    /// key free --- the shift is pressed or released around the key, so
    /// that the machine, which decodes from the stream of positions, sees
    /// the character the viewer typed.
    ///
    /// A prefix sends nothing of its own and the keysym after it is
    /// looked up behind it. A prefix naming a shifting key holds it for
    /// the one key that follows; anything reached behind a prefix or held
    /// under one is tapped rather than held, and its own release is
    /// dropped, the terminal having sent the key whole already.
    ///
    /// This whole function is the one place muir's keyboard invents
    /// anything; everything under it is MIT's.
    ///
    /// **Three pieces of state reached through branches is a state machine
    /// written as conditionals**: `prefix`, `latched` and `tapped`, each
    /// documented where it is declared. It is tested and it reads --- but a
    /// fourth would be the one that is hard to reason about, so if this
    /// grows again, make the machine explicit, a table of state and keysym
    /// to action, rather than adding another branch.
    ///
    /// **The boot sequence is not that fourth.** It is the firmware's
    /// work rather than the mapping's --- `check-boot` after a key-down,
    /// `bootflag` at a key-up --- and it sits under `resolve` in the two
    /// places a word is queued, `queue_down` and `queue_up`, where
    /// `resolve`'s branches never see it.
    pub fn key(&mut self, keysym: u32, down: bool) {
        if self.trace {
            self.key_traced(keysym, down);
            return;
        }
        let went = self.resolve(keysym, down);
        self.firmware = None;
        self.count(&went);
    }

    /// A refusal counted, and **the first of a run said without the
    /// trace**: a run that has lost typing is one thing to say, and a
    /// keystroke lost in silence is a character that does not type with
    /// nothing to tell it from a key that has no binding. Under the trace
    /// every refused keystroke is a line of its own already, so this says
    /// nothing there.
    fn count(&mut self, went: &Went) {
        if let Went::Refused { .. } = went {
            self.refused += 1;
            if self.refused == 1 && !self.trace {
                eprintln!(
                    "keyboard: the queue was full and a keystroke was refused; \
                     --keyboard-mapping-trace says each one"
                );
            }
        }
    }

    /// Keystrokes the queue had no room for, over the run. Each was a
    /// character that did not type, and each was refused whole, leaving
    /// nothing down: [`BACKLOG`] says why the queue is bounded.
    pub fn refused(&self) -> usize {
        self.refused
    }

    /// [`Keyboard::key`], and the line `--keyboard-mapping-trace` writes
    /// for it: the keysym by name **and** number --- the name is what goes
    /// in a mapping file, the number is what to write when there is none
    /// --- whether it went down or up, what it became, and what the
    /// firmware's part then did with it, when it did anything
    /// (`Firmware`).
    ///
    /// Returned as well as printed so that `tests/keyboard_mapping.rs` can
    /// hold the wording, and the key's spelling, without capturing a
    /// stream. The key is acted on either way: this is `key` with the line
    /// handed back.
    pub fn key_traced(&mut self, keysym: u32, down: bool) -> String {
        let went = self.resolve(keysym, down);
        let firmware = self.firmware.take();
        self.count(&went);
        let line = format!(
            "keysym {keysym:#x} {} {}, {went}{}",
            keysym_name(keysym),
            if down { "down" } else { "up" },
            firmware.map(|f| f.to_string()).unwrap_or_default()
        );
        if self.trace {
            eprintln!("{line}");
        }
        line
    }

    /// [`Keyboard::key`]'s work, and what it did, which is the trace's to
    /// print.
    fn resolve(&mut self, keysym: u32, down: bool) -> Went {
        // A key the terminal has already sent whole: its release is not
        // owed to the machine.
        if !down && let Some(i) = self.tapped.iter().position(|&s| s == keysym) {
            self.tapped.remove(i);
            return Went::Nothing("its key was tapped and has gone already");
        }
        // A prefix standing: this keysym is looked up behind it.
        if let Some(first) = self.prefix {
            if self.map.is_prefix(keysym) {
                // The prefix's own release, or the prefix again, which
                // is the way out of a sequence begun by mistake.
                if down {
                    self.prefix = None;
                    return Went::PrefixLetGo;
                }
                return Went::Nothing("a prefix acts on its press");
            }
            if !down {
                return Went::Nothing("the prefix stands until a key is pressed behind it");
            }
            self.prefix = None;
            self.tapped.push(keysym);
            let behind = self.map.after_prefix(first, keysym);
            if let Some((p, wants)) = behind
                && !self.behind_prefix(p, wants)
            {
                return Went::Refused { p, shifted: wants };
            }
            return Went::Behind(first, behind);
        }
        if self.map.is_prefix(keysym) {
            if down {
                self.prefix = Some(keysym);
                return Went::HeldAsPrefix;
            }
            return Went::Nothing("a prefix acts on its press");
        }
        if let Some((s, side)) = self.map.modifier(keysym) {
            let at = shifting(s);
            let Some(p) = at.get(side).or(at.first()).copied() else {
                return Went::Nothing("the shifting key it names is on no position");
            };
            if down && !self.press(p) {
                return Went::Refused { p, shifted: false };
            }
            if !down {
                self.release(p);
            }
            return Went::Sent { p, shifted: false, tapped: false };
        }
        // A key held: its repeats and its release go to the position its
        // press went to, not to the one the shift now held would choose.
        if let Some(&(_, p, shifted)) = self.held.iter().find(|&&(s, ..)| s == keysym) {
            if !down {
                self.release(p);
            }
            return Went::Sent { p, shifted, tapped: false };
        }
        let found = self.map.positions(keysym);
        if found.is_empty() {
            return Went::Unbound;
        }
        let shifted = self.holding(Shift::Shift);
        // Under a latched shifting key the key is tapped inside it and
        // the latch let go after, so that the shifting key held is held
        // for this key and no other.
        if !self.latched.is_empty() {
            if !down {
                return Went::Nothing("a latched shifting key holds for the press alone");
            }
            let (p, wants) =
                found.iter().find(|&&(_, w)| w == shifted).copied().unwrap_or(found[0]);
            self.tapped.push(keysym);
            let went = self.tap(p, wants);
            for q in std::mem::take(&mut self.latched) {
                self.release(q);
            }
            if !went {
                return Went::Refused { p, shifted: wants };
            }
            return Went::Sent { p, shifted: wants, tapped: true };
        }
        // The position whose plane the viewer's own shift already gives.
        if let Some(&(p, _)) = found.iter().find(|&&(_, wants)| wants == shifted) {
            if down {
                if !self.press(p) {
                    return Went::Refused { p, shifted };
                }
                self.held.push((keysym, p, shifted));
            } else {
                self.release(p);
            }
            return Went::Sent { p, shifted, tapped: false };
        }
        // Otherwise the shift is worked around the key.
        let (p, wants) = found[0];
        if !down {
            self.release(p);
            return Went::Sent { p, shifted: wants, tapped: false };
        }
        if !self.tap(p, wants) {
            return Went::Refused { p, shifted: wants };
        }
        Went::Sent { p, shifted: wants, tapped: true }
    }

    /// Words waiting to go down the cable.
    pub fn pending(&self) -> usize {
        self.queue.len()
    }

    /// The next word, taken.
    pub fn take(&mut self) -> Option<u32> {
        self.queue.pop_front()
    }

    /// The next word, looked at.
    pub fn peek(&self) -> Option<u32> {
        self.queue.front().copied()
    }

    /// Hands the next word to the keyboard's device, if it can take one:
    /// the behavioral I/O board once it has taken the last (its `KBD
    /// READY` is the keyboard's `DONE` the other way up), or QUUX's FIFO
    /// while it has room.
    pub fn deliver(&mut self, board: &mut impl crate::quux_input::KeyboardMouse) -> bool {
        if !board.takes_key() {
            return false;
        }
        match self.queue.pop_front() {
            Some(word) => {
                board.press(word);
                true
            }
            None => false,
        }
    }
}
