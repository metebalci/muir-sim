// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The keyboard: the word on the cable as `ukbd.lisp` lays it out, the key
//! positions as `KBD-MAKE-NEW-TABLE` has them, and the shifting a viewer's
//! keysyms need to become positions.

use muir::ioboard::{self, IoBoard, csr};
use muir::terminal::keyboard::{
    self, BootKeys, Key, Keyboard, RETURN, RUBOUT, Shift, boot, keysym, named, shifting, up_down,
};

mod support;
use support::release;

/// **The word is `ukbd.lisp`'s.** Bits 23-19 ones, 18-16 the source `001`,
/// 15 clear for an up-down code, 8 the direction, 6-0 the position; the
/// all-keys-up word has 15 set and the shifts below; a boot has 15-10 set
/// and 46 or 62 in 5-0.
#[test]
fn the_word_is_laid_out_as_ukbd_says() {
    let down = keyboard::up_down(0o123, false);
    assert_eq!(down >> 19, 0o37, "bits 23-19 are ones");
    assert_eq!(down >> 16 & 0o7, 0o1, "source ID 001, the new keyboard");
    assert_eq!(down >> 15 & 1, 0, "an up-down code");
    assert_eq!(down >> 8 & 1, 0, "down");
    assert_eq!(down & 0o177, 0o123, "the position");
    assert_eq!(keyboard::up_down(0o123, true) >> 8 & 1, 1, "up");
    assert_eq!(down >> 9 & 0o77, 0, "12-9 and 13, 14 reserved zero");

    let up = keyboard::all_keys_up(1 << Shift::Control as u16 | 1 << Shift::Shift as u16);
    assert_eq!(up >> 15 & 1, 1, "not an up-down code");
    assert_eq!(up & 0o3777, 0o21, "control and shift still down");

    assert_eq!(keyboard::boot(true) & 0o77, 0o46, "cold");
    assert_eq!(keyboard::boot(false) & 0o77, 0o62, "warm");
    assert_eq!(keyboard::boot(true) >> 10 & 0o77, 0o77, "15-10 ones");
    assert_eq!(keyboard::boot(true) >> 6 & 0o17, 0, "9-6 zero");
    assert_eq!(keyboard::boot(true) >> 16 & 1, 1, "bit 16, which the I/O board also checks");
}

/// **The table agrees with `ukbd.lisp` at every position it names.** The
/// table is from the System 46 sources; the firmware is System 100's own,
/// and names positions in passing: "the bit numbers of the keys which are
/// specially known about for shifting purposes", eleven shifts with one or
/// two positions each, and "rubout is 23 and return is 136". Those are read
/// out of `sys/io1/ukbd.lisp` here and compared with the table; without the
/// release the test says it was skipped. The rest of the table's positions
/// have no second route in the repository.
#[test]
fn mits_firmware_names_the_shift_positions_and_the_table_has_them() {
    let Some(ukbd) = release("io1/ukbd.lisp") else { return };
    // The comment block: `;  <shift>\t<position>` or `;  <shift>\t<a> / <b>`,
    // octal, from the line after the sentence that opens it to the blank
    // line that closes it.
    let opening = ukbd.find("known about for shifting purposes:").expect("the comment block");
    let block = &ukbd[opening..];
    let block = &block[block.find('\n').unwrap() + 1..];
    let block = &block[..block.find("\n\n").expect("the blank line that closes it")];
    let shift = |name: &str| match name {
        "mode lock" => Shift::ModeLock,
        "caps lock" => Shift::CapsLock,
        "alt lock" => Shift::AltLock,
        "repeat" => Shift::Repeat,
        "top" => Shift::Top,
        "greek" => Shift::Greek,
        "shift" => Shift::Shift,
        "hyper" => Shift::Hyper,
        "super" => Shift::Super,
        "meta" => Shift::Meta,
        "control" => Shift::Control,
        other => panic!("the firmware names a shift the table has not got: {other}"),
    };
    let mut shifts = 0;
    for line in block.lines() {
        let line = line.strip_prefix(';').expect("a comment line").trim();
        let (name, positions) = line.split_once('\t').expect("a shift and its positions");
        let mut positions: Vec<u8> =
            positions.split('/').map(|p| u8::from_str_radix(p.trim(), 8).unwrap()).collect();
        positions.sort_unstable();
        assert_eq!(shifting(shift(name.trim())), positions, "{name}");
        shifts += 1;
    }
    assert_eq!(shifts, 11, "eleven shifts");

    // "For booting, we know that rubout is 23 and return is 136".
    let booting = ukbd
        .lines()
        .find(|l| l.contains("rubout is") && l.contains("return is"))
        .expect("the booting sentence");
    let after = |words: &str| -> u8 {
        let rest = &booting[booting.find(words).unwrap() + words.len()..];
        let digits: String = rest.trim_start().chars().take_while(|c| c.is_digit(8)).collect();
        u8::from_str_radix(&digits, 8).unwrap()
    };
    assert_eq!(named("Rubout"), Some(after("rubout is")), "rubout");
    assert_eq!(named("Return"), Some(after("return is")), "return");
}

/// **A keysym lands on the key that gives it.** Letters on their key,
/// shifted characters on the shifted plane of theirs, `(` on both the key
/// that has it unshifted and the one that has it over `9`.
#[test]
fn a_keysym_is_found_on_the_keyboard() {
    assert_eq!(keyboard::positions('a' as u32), [(0o123, false)]);
    assert_eq!(keyboard::positions('A' as u32), [(0o123, true)]);
    assert_eq!(keyboard::positions('!' as u32), [(0o121, true)], "shift 1");
    assert_eq!(
        keyboard::positions('(' as u32),
        [(0o71, true), (0o132, false)],
        "9 shifted, or its own key"
    );
    assert_eq!(keyboard::positions(' ' as u32), [(0o134, false)]);
    assert_eq!(keyboard::positions(keysym::RETURN), [(0o136, false)]);
    assert_eq!(keyboard::positions(keysym::BACKSPACE), [(0o23, false)], "rubout");
    assert_eq!(keyboard::positions(keysym::ESCAPE), [(0o143, false)], "alt mode");
    assert_eq!(keyboard::positions(keysym::F1), [(0o40, false)], "terminal");
    assert!(keyboard::positions(0xff50).is_empty(), "the CADR has no Home key");
    assert_eq!(keyboard::modifier(keysym::ALT_L), Some((Shift::Meta, 0)), "Alt is Meta");
    assert!(matches!(keyboard::TABLE[0o21], Key::None), "plus-minus is not ASCII");
}

/// The keys the machine would decode from a stream of words, as
/// `KBD-CONVERT-NEW` in `lmio/kbd.123` does it over planes 0 and 1:
/// shifts tracked from up-down codes, a character taken on a key-down from
/// the plane the shift selects.
fn decode(words: &[u32]) -> String {
    let mut shift = false;
    let mut out = String::new();
    for &w in words {
        assert_eq!(w >> 16, 0o371, "every word is the new keyboard's");
        let up = w & keyboard::UP != 0;
        let position = (w & 0o177) as usize;
        match keyboard::TABLE[position] {
            Key::Shift(Shift::Shift) => shift = !up,
            Key::Char(plain, shifted) if !up => {
                out.push(if shift { shifted } else { plain } as char)
            }
            Key::Named(n) if !up => out.push_str(&format!("<{n}>")),
            _ => {}
        }
    }
    out
}

/// **What a viewer types is what the machine decodes.** Each keysym a
/// viewer sends, with the shifts a viewer sends, comes out of MIT's own
/// decoding as the character typed --- including the ones where the
/// viewer's shift and the keyboard's disagree.
#[test]
fn what_the_viewer_types_is_what_the_machine_reads() {
    let mut k = Keyboard::new();
    let mut words = Vec::new();
    let mut type_key = |k: &mut Keyboard, sym: u32| {
        k.key(sym, true);
        k.key(sym, false);
        while let Some(w) = k.take() {
            words.push(w);
        }
    };
    // a, then shift held by the viewer and A, then shift released.
    type_key(&mut k, 'a' as u32);
    k.key(keysym::SHIFT_L, true);
    type_key(&mut k, 'A' as u32);
    // `(` with the viewer's shift held: the key over 9, no unshifting.
    type_key(&mut k, '(' as u32);
    k.key(keysym::SHIFT_L, false);
    // `!` with no shift held: shift is pressed around it.
    type_key(&mut k, '!' as u32);
    // `(` with no shift held: its own key, plane 0.
    type_key(&mut k, '(' as u32);
    // A shifted character on a key whose plane 0 the viewer wants while
    // holding shift: `9` with shift down has to let shift go.
    k.key(keysym::SHIFT_R, true);
    type_key(&mut k, '9' as u32);
    k.key(keysym::SHIFT_R, false);
    type_key(&mut k, keysym::RETURN);
    while let Some(w) = k.take() {
        words.push(w);
    }
    assert_eq!(decode(&words), "aA(!(9<Return>");
    // And every key that went down came up.
    let downs = words.iter().filter(|&&w| w & keyboard::UP == 0).count();
    let ups = words.iter().filter(|&&w| w & keyboard::UP != 0).count();
    let octal: Vec<String> = words.iter().map(|w| format!("{w:o}")).collect();
    assert_eq!(downs, ups, "every key up: {octal:?}");
}

/// **The word reaches the behavioral I/O board and reads back.** `KBD
/// READY` rises, the low half reads at `764100` and the high at `764102`
/// with the floating byte above it, and the next word waits until the
/// board has been read --- the keyboard's `DONE`.
#[test]
fn a_key_is_delivered_to_the_io_board_and_read_back() {
    let mut k = Keyboard::new();
    let mut b = IoBoard::default();
    k.key('z' as u32, true);
    k.key('z' as u32, false);
    assert_eq!(k.pending(), 2);
    assert!(k.deliver(&mut b), "the first word goes in");
    assert!(b.keyboard_ready());
    assert!(!k.deliver(&mut b), "the second waits: the board has not been read");

    let word = keyboard::up_down(0o124, false);
    let high = b.read(ioboard::KBD_HIGH, 0);
    assert_eq!(high & 0xff, (word >> 16) as u16, "bits 23-16 at 764102");
    assert_eq!(high & csr::FLOATING, csr::FLOATING, "and the floating byte above");
    assert!(b.keyboard_ready(), "the high half leaves ready: the word is not taken yet");
    assert!(!k.deliver(&mut b), "so the second still waits");
    let low = b.read(ioboard::KBD_LOW, 0);
    assert_eq!(low, word as u16, "bits 15-0 at 764100");
    assert!(!b.keyboard_ready(), "reading it cleared ready");
    assert!(k.deliver(&mut b), "and the key-up goes in");
    assert_eq!(b.read(ioboard::KBD_LOW, 0), keyboard::up_down(0o124, true) as u16);
}

/// **The board requests an interrupt only when told to, and the machine
/// takes it as vector 260.** `KBD READY` alone is not a request: the CSR's
/// `KBD INT ENABLE` has to be set, and the bus interface's `ENABLE UB
/// INTS` has to be set for the interface to take it; then `766040` reads
/// `UB INT` with the vector in bits 2-9, and a write of zero to `766042`
/// after the data has been read leaves nothing pending.
#[test]
fn the_keyboard_interrupt_is_taken_with_its_vector() {
    use muir::busint::interrupt_status::{ENABLE_UB_INTS, UB_INT};
    use muir::machine::Machine;
    // A Unibus address as the processor reaches it: the inverse of
    // `busint::unibus_address`, Unibus I/O space at physical 17400000 with
    // the 16-bit word address halved into it.
    let phys = |unibus: u32| 0o17400000 | (unibus >> 1);
    let mut m = Machine::new();
    m.ioboard.press(keyboard::up_down(0o123, false));
    assert!(m.ioboard.interrupt_request(m.ns).is_none(), "ready, but the enable is clear");
    m.ioboard.write(ioboard::CSR, csr::KBD_INT_ENABLE, 0);
    assert_eq!(m.ioboard.interrupt_request(m.ns), Some(ioboard::KBD_VECTOR));
    assert!(!m.interrupt(), "the interface has not been enabled");

    // What the microcode writes at the end of the cold boot.
    m.bus_write(phys(0o766040), 0o6000);
    assert!(m.interrupt(), "taken");
    let status = m.bus_read(phys(0o766040)) as u16;
    assert_ne!(status & UB_INT, 0, "UB INT reads set");
    assert_eq!(status & 0o1774, 0o260, "the vector, in place, in bits 2-9");
    assert_ne!(status & ENABLE_UB_INTS, 0);

    // The interrupt routine reads the data, high half first.
    let _ = m.bus_read(phys(ioboard::KBD_HIGH));
    let _ = m.bus_read(phys(ioboard::KBD_LOW));
    assert!(m.ioboard.interrupt_request(m.ns).is_none(), "the request drops with KBD READY");
    m.bus_write(phys(0o766042), 0);
    assert!(!m.interrupt(), "dismissed");
    assert_eq!(m.bus_read(phys(0o766040)) as u16 & UB_INT, 0);
}

/// **The boot sequence's keys are the ones the firmware tests.**
/// `check-boot` in `sys/io1/ukbd.lisp` names the positions it looks at,
/// in octal --- "meta 45 / 165, control 20 / 26, rubout 23, return 136"
/// --- and the keyboard model finds them on MIT's table through
/// `shifting` and `named`, which is the half of this that needs no
/// release. The firmware's own numbers are read out of the file and
/// compared; without the release that half says it was skipped.
#[test]
fn the_boot_sequences_keys_are_the_ones_the_firmware_tests() {
    assert_eq!(shifting(Shift::Control), [0o20, 0o26]);
    assert_eq!(shifting(Shift::Meta), [0o45, 0o165]);
    assert_eq!(named("Rubout"), Some(RUBOUT));
    assert_eq!(named("Return"), Some(RETURN));
    assert_eq!((RUBOUT, RETURN), (0o23, 0o136));
    let Some(ukbd) = release("io1/ukbd.lisp") else { return };
    // The comment above `check-boot` --- "Is request to boot machine if
    // both controls and both metas are held down" --- then `;  meta\t\t45
    // / 165` and three lines like it, up to the one about the locking
    // keys. The shift block near the top of the file names meta too, so
    // the sentence is found first.
    let sentence = ukbd.find("Is request to boot machine").expect("check-boot's own sentence");
    let block = &ukbd[sentence..];
    let start = block.find(";  meta\t").expect("the block names meta first");
    let mut found = std::collections::BTreeMap::new();
    for line in block[start..].lines().take_while(|l| !l.contains("locking keys")) {
        let line = line.strip_prefix(';').expect("a comment line").trim();
        let (name, positions) = line.split_once('\t').expect("a key and its positions");
        let positions: Vec<u8> =
            positions.split('/').map(|p| u8::from_str_radix(p.trim(), 8).unwrap()).collect();
        found.insert(name.trim().to_string(), positions);
    }
    assert_eq!(found.len(), 4, "meta, control, rubout and return: {found:?}");
    assert_eq!(found["meta"], shifting(Shift::Meta));
    assert_eq!(found["control"], shifting(Shift::Control));
    assert_eq!(found["rubout"], [RUBOUT]);
    assert_eq!(found["return"], [RETURN]);
}

/// The words a keyboard holds, taken.
fn words(k: &mut Keyboard) -> Vec<u32> {
    let mut out = Vec::new();
    while let Some(w) = k.take() {
        out.push(w);
    }
    out
}

/// **The boot sequence sends the boot word after the key-down that
/// completes it, then holds every key-up back until the next key-down.**
/// `ukbd.lisp`: `check-boot` runs after every key-down's code has gone,
/// sends the cold boot code with Rubout down and the warm one with
/// Return, and sets `bootflag`, under which no key-up code goes --- "This
/// gives the machine time to load microcode and read the character to see
/// whether it is a warm or cold boot, before sending any other
/// characters, such as up-codes" --- until the next key-down clears it.
/// The keys released meanwhile are up on the keyboard all the same. With
/// the default keys: one Control and one Meta.
#[test]
fn the_boot_sequence_sends_the_boot_word_and_holds_the_key_ups_back() {
    let mut k = Keyboard::new();
    assert_eq!(k.boot_keys(), BootKeys::default(), "ctrl,meta unless told otherwise");
    k.key(keysym::CONTROL_L, true);
    k.key(keysym::ALT_L, true);
    k.key(keysym::BACKSPACE, true);
    assert_eq!(
        words(&mut k),
        [up_down(0o20, false), up_down(0o45, false), up_down(RUBOUT, false), boot(true)],
        "Control, Meta and Rubout down, and the cold boot word after them"
    );
    k.key(keysym::BACKSPACE, false);
    k.key(keysym::ALT_L, false);
    k.key(keysym::CONTROL_L, false);
    assert_eq!(k.pending(), 0, "no key-up goes");
    // The next key-down ends it; it and its own key-up go.
    k.key('a' as u32, true);
    k.key('a' as u32, false);
    assert_eq!(words(&mut k), [up_down(0o123, false), up_down(0o123, true)]);
    // Return warm-boots, with either Control and either Meta.
    k.key(keysym::CONTROL_R, true);
    k.key(keysym::ALT_R, true);
    k.key(keysym::RETURN, true);
    assert_eq!(
        words(&mut k),
        [up_down(0o26, false), up_down(0o165, false), up_down(RETURN, false), boot(false)]
    );
    // Rubout added with Return still down: the firmware tests Rubout
    // first, so cold. A key pressed again while down is no key-down.
    k.key(keysym::BACKSPACE, true);
    assert_eq!(words(&mut k), [up_down(RUBOUT, false), boot(true)]);
    k.key(keysym::BACKSPACE, true);
    assert_eq!(k.pending(), 0);
}

/// **The keys the boot sequence needs are a setting**, `--keyboard-boot`,
/// spelled as the Controls and Metas to hold: one of a word is either key
/// of its pair, two is both, the order does not matter, and anything else
/// is refused with the four spellings named. Rubout and Return are not in
/// it. Each setting holds out for its keys, and a key-up before the
/// sequence is complete goes as any key-up does.
#[test]
fn the_keys_the_boot_sequence_needs_are_a_setting() {
    for (spelling, controls, metas) in [
        ("ctrl,meta", 1, 1),
        ("ctrl,ctrl,meta", 2, 1),
        ("ctrl,meta,meta", 1, 2),
        ("ctrl,ctrl,meta,meta", 2, 2),
    ] {
        let keys = BootKeys::parse(spelling).unwrap_or_else(|e| panic!("{spelling}: {e}"));
        assert_eq!((keys.controls(), keys.metas()), (controls, metas), "{spelling}");
        assert_eq!(keys.to_string(), spelling, "and spelled back the same");
    }
    assert_eq!(
        BootKeys::SPELLINGS,
        ["ctrl,meta", "ctrl,ctrl,meta", "ctrl,meta,meta", "ctrl,ctrl,meta,meta"]
    );
    assert_eq!(BootKeys::parse("meta,ctrl").unwrap(), BootKeys::parse("ctrl,meta").unwrap());
    assert_eq!(BootKeys::parse(" meta , ctrl,ctrl ").unwrap().to_string(), "ctrl,ctrl,meta");
    for bad in [
        "",
        "ctrl",
        "meta",
        "ctrl,ctrl",
        "ctrl,ctrl,ctrl,meta",
        "ctrl,meta,rubout",
        "control,meta",
        "ctrl,,meta",
    ] {
        let e = BootKeys::parse(bad).expect_err(bad);
        for one in BootKeys::SPELLINGS {
            assert!(e.contains(one), "{bad:?}: the refusal names {one}:\n{e}");
        }
    }
    // Both Controls wanted: one is not enough, and the key-ups go.
    let mut k = Keyboard::new();
    k.set_boot_keys(BootKeys::parse("ctrl,ctrl,meta").unwrap());
    k.key(keysym::CONTROL_L, true);
    k.key(keysym::ALT_L, true);
    k.key(keysym::BACKSPACE, true);
    assert_eq!(words(&mut k), [up_down(0o20, false), up_down(0o45, false), up_down(RUBOUT, false)]);
    k.key(keysym::BACKSPACE, false);
    assert_eq!(words(&mut k), [up_down(RUBOUT, true)], "not held back: no boot word went");
    // The other Control completes it, whichever key comes last.
    k.key(keysym::BACKSPACE, true);
    k.key(keysym::CONTROL_R, true);
    assert_eq!(words(&mut k), [up_down(RUBOUT, false), up_down(0o26, false), boot(true)]);
    // Both Metas wanted likewise.
    let mut k = Keyboard::new();
    k.set_boot_keys(BootKeys::parse("ctrl,meta,meta").unwrap());
    k.key(keysym::CONTROL_L, true);
    k.key(keysym::ALT_L, true);
    k.key(keysym::RETURN, true);
    assert_eq!(k.pending(), 3, "one Meta: no boot word");
    k.key(keysym::ALT_R, true);
    assert_eq!(words(&mut k).last(), Some(&boot(false)));
}

/// One key's words, down then up.
fn stroke(p: u8) -> [u32; 2] {
    [up_down(p, false), up_down(p, true)]
}

/// **A key's release goes to the position its press went to**, not to
/// the one the shift held at release would choose. `(` and `)` are the
/// two keysyms on two positions: `(` shifted on the `9` key and plain on
/// its own, `)` shifted on the `0` key and plain on its own. Typed with
/// the viewer's shift, in either order of letting go, the key that went
/// down comes up; and the next press of that key --- the same character
/// again, or the digit that shares it --- reaches the machine whole.
/// Every step is checked and every mismatch said, so that a character
/// lost after the first is seen too.
fn release_goes_where_press_went(sym: char, digit: char, on_digit: u8, shift_first: bool) {
    let shift = shifting(Shift::Shift)[0];
    let (sym, digit) = (sym as u32, digit as u32);
    let mut wrong = Vec::new();
    let mut check = |step: &str, got: Vec<u32>, want: &[u32]| {
        if got != want {
            let o = |w: &[u32]| w.iter().map(|w| format!("{w:o}")).collect::<Vec<_>>();
            wrong.push(format!("{step}: got {:?}, want {:?}", o(&got), o(want)));
        }
    };
    let mut want = vec![up_down(shift, false), up_down(on_digit, false)];
    if shift_first {
        want.extend([up_down(shift, true), up_down(on_digit, true)]);
    } else {
        want.extend([up_down(on_digit, true), up_down(shift, true)]);
    }
    // The character typed with the viewer's shift, let go in this order.
    let typed = || {
        let mut k = Keyboard::new();
        k.key(keysym::SHIFT_L, true);
        k.key(sym, true);
        if shift_first {
            k.key(keysym::SHIFT_L, false);
            k.key(sym, false);
        } else {
            k.key(sym, false);
            k.key(keysym::SHIFT_L, false);
        }
        k
    };
    let mut k = typed();
    check("typed with shift", words(&mut k), &want);
    // Then the same character again, with the viewer's shift.
    k.key(keysym::SHIFT_L, true);
    k.key(sym, true);
    k.key(sym, false);
    k.key(keysym::SHIFT_L, false);
    let again = [
        up_down(shift, false),
        up_down(on_digit, false),
        up_down(on_digit, true),
        up_down(shift, true),
    ];
    check("the same again", words(&mut k), &again);
    // Or the digit that shares its key.
    let mut k = typed();
    words(&mut k);
    k.key(digit, true);
    k.key(digit, false);
    check("the digit after it", words(&mut k), &stroke(on_digit));
    // Nothing is left down: a stray release finds nothing to let go.
    k.key(sym, false);
    k.key(digit, false);
    check("nothing left down", words(&mut k), &[]);
    let order = if shift_first { "shift let go first" } else { "key let go first" };
    assert!(wrong.is_empty(), "{sym:#x}, {order}:\n{}", wrong.join("\n"));
}

#[test]
fn open_paren_released_before_its_shift() {
    release_goes_where_press_went('(', '9', 0o71, false);
}

#[test]
fn open_paren_released_after_its_shift() {
    release_goes_where_press_went('(', '9', 0o71, true);
}

#[test]
fn close_paren_released_before_its_shift() {
    release_goes_where_press_went(')', '0', 0o171, false);
}

#[test]
fn close_paren_released_after_its_shift() {
    release_goes_where_press_went(')', '0', 0o171, true);
}

/// **A key held and repeated stays on the position it went down on**,
/// whatever the shift does meanwhile: `(` pressed with shift, the shift
/// let go, and the viewer's repeat of `(` is the key over `9` still down,
/// not a second key; and a viewer that names the key by its unshifted
/// keysym on release, `9` for the `(` it pressed, lets the same key go,
/// and leaves nothing behind to catch the next `(`.
#[test]
fn a_held_key_repeats_and_is_let_go_where_it_went_down() {
    let shift = shifting(Shift::Shift)[0];
    let mut k = Keyboard::new();
    k.key(keysym::SHIFT_L, true);
    k.key('(' as u32, true);
    k.key(keysym::SHIFT_L, false);
    k.key('(' as u32, true);
    k.key('(' as u32, false);
    assert_eq!(
        words(&mut k),
        [up_down(shift, false), up_down(0o71, false), up_down(shift, true), up_down(0o71, true)]
    );
    k.key(keysym::SHIFT_L, true);
    k.key('(' as u32, true);
    k.key(keysym::SHIFT_L, false);
    k.key('9' as u32, false);
    assert_eq!(
        words(&mut k),
        [up_down(shift, false), up_down(0o71, false), up_down(shift, true), up_down(0o71, true)]
    );
    k.key('(' as u32, false);
    assert_eq!(k.pending(), 0, "nothing is left to let go");
    // And `(` typed next without a shift is its own key, a stroke.
    k.key('(' as u32, true);
    k.key('(' as u32, false);
    assert_eq!(words(&mut k), stroke(0o132));
}

/// **Without a shift, nothing changes**: `(` and `)` go to their own
/// keys and the digits to theirs, a stroke each.
#[test]
fn unshifted_parentheses_and_digits_are_one_stroke_each() {
    let mut k = Keyboard::new();
    for (sym, p) in [('(', 0o132u8), ('9', 0o71), (')', 0o137), ('0', 0o171), ('(', 0o132)] {
        k.key(sym as u32, true);
        k.key(sym as u32, false);
        assert_eq!(words(&mut k), stroke(p), "{sym}");
    }
}
