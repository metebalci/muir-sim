// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A packet as the hardware carries it and as the software lays it out.
//!
//! AIM-628 §2.2: "a sequence of up to 4032 data bits, plus 48 bits of
//! header information used by the hardware", the hardware header being
//! "three 16-bit words, called destination, source, and check". §3.5: the
//! software header is eight 16-bit words --- operation, count, destination
//! address and index, source address and index, packet number,
//! acknowledgement --- followed by the data, up to 488 bytes. §7 says how
//! the interface takes it: the eight header words, the data words, then
//! the cable destination as the last word written, the hardware adding
//! the source and the check word itself.

/// The most data a packet carries, AIM-628 §3.5: "the maximum value is
/// 488".
pub const MAX_DATA: usize = 488;

/// Packet opcodes, AIM-628 chapter 4, as `sys/network/chaos/chsncp.lisp`
/// numbers them.
pub mod op {
    pub const RFC: u8 = 0o1;
    pub const OPN: u8 = 0o2;
    pub const CLS: u8 = 0o3;
    pub const FWD: u8 = 0o4;
    pub const ANS: u8 = 0o5;
    pub const SNS: u8 = 0o6;
    pub const STS: u8 = 0o7;
    pub const RUT: u8 = 0o10;
    pub const LOS: u8 = 0o11;
    pub const LSN: u8 = 0o12;
    pub const MNT: u8 = 0o13;
    pub const EOF: u8 = 0o14;
    pub const UNC: u8 = 0o15;
    pub const BRD: u8 = 0o16;
    /// "Opcodes 200 through 277 (octal) are controlled packets with user
    /// data in 8-bit bytes"; 200 is the default.
    pub const DAT: u8 = 0o200;
    /// "Opcodes 300 through 377 ... 16-bit bytes"; 300 is the default.
    pub const DWD: u8 = 0o300;
    /// Whether an opcode carries user data.
    pub fn is_data(op: u8) -> bool {
        op >= DAT
    }
    /// Whether packets of this opcode are controlled --- numbered,
    /// acknowledged and retransmitted, §3.8.
    pub fn is_controlled(op: u8) -> bool {
        matches!(op, RFC | OPN | EOF) || is_data(op)
    }
}

/// An opcode's name, for a trace: the mnemonic of [`op`], or `DAT`/`DWD`
/// with the opcode in octal for the data ranges, or `op<n>` for one
/// AIM-628 does not name.
pub fn op_name(op: u8) -> String {
    match op {
        op::RFC => "RFC".into(),
        op::OPN => "OPN".into(),
        op::CLS => "CLS".into(),
        op::FWD => "FWD".into(),
        op::ANS => "ANS".into(),
        op::SNS => "SNS".into(),
        op::STS => "STS".into(),
        op::RUT => "RUT".into(),
        op::LOS => "LOS".into(),
        op::LSN => "LSN".into(),
        op::MNT => "MNT".into(),
        op::EOF => "EOF".into(),
        op::UNC => "UNC".into(),
        op::BRD => "BRD".into(),
        o if o >= op::DWD => format!("DWD{:o}", o),
        o if o >= op::DAT => format!("DAT{:o}", o),
        o => format!("op{o:o}"),
    }
}

/// The software header and data, AIM-628 §3.5.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    /// "The most-significant 8 bits of this word are the Opcode."
    pub opcode: u8,
    /// The forwarding count, the top four bits of the count word.
    pub forward: u8,
    pub dest: u16,
    pub dest_index: u16,
    pub source: u16,
    pub source_index: u16,
    pub number: u16,
    pub ack: u16,
    /// The data, "8-bit bytes"; the byte count is its length.
    pub data: Vec<u8>,
}

impl Packet {
    /// The eight header words and the data words, low byte of each pair
    /// first, AIM-628 §3.6: "The first 8-bit byte in a 16-bit word is the
    /// one in the arithmetically least-significant position." An odd last
    /// byte sits in the low half with "a garbage padding byte in its high
    /// half", §7; the padding here is zero.
    pub fn words(&self) -> Vec<u16> {
        let mut w = vec![
            (self.opcode as u16) << 8,
            (self.forward as u16) << 12 | (self.data.len() as u16 & 0o7777),
            self.dest,
            self.dest_index,
            self.source,
            self.source_index,
            self.number,
            self.ack,
        ];
        for pair in self.data.chunks(2) {
            w.push(pair[0] as u16 | (pair.get(1).copied().unwrap_or(0) as u16) << 8);
        }
        w
    }

    /// What the software writes into the transmit buffer: the words and
    /// then the cable destination, AIM-628 §7.
    pub fn to_buffer(&self, cable_dest: u16) -> Vec<u16> {
        let mut w = self.words();
        w.push(cable_dest);
        w
    }

    /// The packet back out of a buffer as the software reads it: the
    /// header, the data, and the cable destination last.
    pub fn from_buffer(buffer: &[u16]) -> Result<(Packet, u16), String> {
        if buffer.len() < 9 {
            return Err(format!(
                "{} words is too short for a header and a destination",
                buffer.len()
            ));
        }
        let count = (buffer[1] & 0o7777) as usize;
        let data_words = count.div_ceil(2);
        if buffer.len() != 8 + data_words + 1 {
            return Err(format!(
                "a byte count of {count} wants {} words, not {}",
                8 + data_words + 1,
                buffer.len()
            ));
        }
        let mut data = Vec::with_capacity(count);
        for w in &buffer[8..8 + data_words] {
            data.push(*w as u8);
            data.push((*w >> 8) as u8);
        }
        data.truncate(count);
        Ok((
            Packet {
                opcode: (buffer[0] >> 8) as u8,
                forward: (buffer[1] >> 12) as u8,
                dest: buffer[2],
                dest_index: buffer[3],
                source: buffer[4],
                source_index: buffer[5],
                number: buffer[6],
                ack: buffer[7],
                data,
            },
            buffer[8 + data_words],
        ))
    }
}

/// The check word the interface puts on a packet: the Fairchild 9401 at
/// LMTBUF C09 dividing by CRC-16, `x^16 + x^15 + x^2 + 1`, its select
/// pins grounded, from a cleared register, over the buffer's words in the
/// order written --- header, data, cable destination, then the source the
/// hardware adds --- each word most-significant bit first, which is the
/// order the two 74165s at B12 and B13 shift a word out.
///
/// That is not read off a document; it is the one arrangement that
/// reproduces the word the netlist board itself produced. Looped back,
/// the board gave `135771` for the packet `a_packet_loops_back_through_the_board`
/// writes, and of the bit orders, seeds and polynomials the 9401 offers,
/// only this one gives it. `the_check_word_is_the_boards` in
/// `tests/chaos.rs` holds it there, and the loopback test asserts it live.
pub fn check_word(words: &[u16]) -> u16 {
    check_on(0, words)
}

/// [`check_word`] carried on from `check`, the check of the words before
/// `words`: `check_on(check_word(a), b)` is `check_word` of `a` then `b`.
/// The interface's checker runs this way over a buffer a word at a time,
/// as it is filled and as it is read out.
pub fn check_on(check: u16, words: &[u16]) -> u16 {
    let mut r = check as u32; // stage k in bit k
    for &w in words {
        for k in (0..16).rev() {
            let d = (w >> k) & 1 != 0;
            let fb = d ^ (r >> 15 & 1 != 0);
            let mut next = (r << 1) & 0xffff;
            if fb {
                next ^= 1 | 1 << 2 | 1 << 15;
            }
            r = next;
        }
    }
    r as u16
}

/// The bits a packet occupies on the cable, from the buffer's words and
/// the source address, AIM-628 §2.5: "Packets are transmitted over the
/// ether in reverse bit-order, for hardware convenience. The three
/// header words, which to the software appear to be at the end of the
/// packet, are transmitted first, in the order check, source, destination.
/// The data words, in reverse order, follow. Words are transmitted
/// least-significant bit first. ... At the end of the packet, an extra
/// zero bit is appended to bring the ether to the low state."
///
/// So the transmit buffer --- the words as written, then the source, then
/// the check word, each most-significant bit first --- is played out
/// backwards, bit by bit, and a zero follows.
pub fn frame(buffer: &[u16], source: u16) -> Vec<bool> {
    let mut all: Vec<u16> = buffer.to_vec();
    all.push(source);
    all.push(check_word(&all));
    line(&all)
}

/// The bits `words` occupy on the cable as a transmit buffer holding them
/// plays them out: backwards, bit by bit, and a zero. [`frame`] is this of
/// the buffer, the source and the check word; a buffer written past its
/// end holds other words, which go out the same way.
pub fn line(words: &[u16]) -> Vec<bool> {
    let all = words;
    let mut bits: Vec<bool> = Vec::with_capacity(all.len() * 16 + 1);
    for &w in all {
        for k in (0..16).rev() {
            bits.push((w >> k) & 1 != 0);
        }
    }
    bits.reverse();
    bits.push(false);
    bits
}

/// The source and destination words of a frame from the bits that have
/// gone by on the cable, for a receiver that must decide before the run
/// ends. AIM-628 §2.5, memo pages 5 and 6: the three hardware words "are
/// transmitted first, in the order check, source, destination", and
/// "Words are transmitted least-significant bit first" --- so the first
/// sixteen cells carry the check word, the next sixteen the source and
/// the next sixteen the destination, each least-significant bit first.
/// `None` until the destination word is whole, 48 cells in.
pub fn header_on_the_wire(bits: &[bool]) -> Option<(u16, u16)> {
    let word = |k: usize| (0..16).map(|i| (bits[k * 16 + i] as u16) << i).sum();
    (bits.len() >= 48).then(|| (word(1), word(2)))
}

/// A packet taken off the cable: [`frame`] undone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Framed {
    /// The buffer as the software would read it back: the words as
    /// written, cable destination last.
    pub buffer: Vec<u16>,
    /// The source address the sending interface put in.
    pub source: u16,
    /// The check word as received, and whether it is the right one.
    pub check: u16,
    pub check_ok: bool,
}

/// The words back out of the cable's bits. The trailing zero, if the
/// decoder delivered it, is dropped; the interface "strips it off".
/// A run of bits that is not a whole packet is an error here;
/// [`unframe_any`] takes it as the receiver does.
pub fn unframe(bits: &[bool]) -> Result<Framed, String> {
    let mut bits = bits.to_vec();
    if bits.len() % 16 == 1 && !bits[bits.len() - 1] {
        bits.pop();
    }
    if !bits.len().is_multiple_of(16) || bits.len() < 3 * 16 {
        return Err(format!("{} bits is not a packet", bits.len()));
    }
    Ok(received(&bits).framed)
}

/// A run of bits off the cable, as a receiver takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Received {
    /// The words as the software reads them back.
    pub framed: Framed,
    /// How many bits came, the trailing zero stripped: the bit count.
    pub bits: usize,
}

/// The words back out of any run of bits the decoder ended, a collision's
/// wreckage included, and how many bits there were: the receiver takes
/// whatever comes once the destination has matched, and the software
/// judges it by the check word and the bit count --- AIM-628 §5.1's
/// meters count "incoming packets with CRC errors" and those "rejected
/// for a length that is not a multiple of 16 bits". The trailing zero is
/// stripped. The buffer is a bit at a time, the 2147 at LMRBUF 0C04 on
/// `RBCT<11:0>`, and the software reads it sixteen bits at a time down
/// from the word boundary above the last bit stored, so a partial word is
/// the last bits received, read with zeros above them: the netlist board
/// reads wreckage back that way in `tests/chaos_netlist.rs` and
/// `tests/chaos_rtl.rs`, and does so over an earlier frame that left ones
/// in the buffer there (`reading_on_goes_round_the_buffer_on_both_alike`).
/// Fewer than three whole words, or a count that is not a whole number of
/// words, cannot check good.
pub fn unframe_any(bits: &[bool]) -> Received {
    let mut bits = bits.to_vec();
    if bits.last() == Some(&false) {
        bits.pop();
    }
    received(&bits)
}

/// The words of `bits`, the trailing zero already stripped, as the
/// software reads them back.
fn received(bits: &[bool]) -> Received {
    let mut bits = bits.to_vec();
    let count = bits.len();
    let n = count.div_ceil(16);
    bits.resize(n * 16, false);
    bits.reverse();
    let words: Vec<u16> =
        bits.chunks(16).map(|c| c.iter().fold(0u16, |w, &b| w << 1 | b as u16)).collect();
    let whole = count.is_multiple_of(16) && n >= 3;
    let (buffer, source, check) = match n {
        0 => (Vec::new(), 0, 0),
        1 => (Vec::new(), 0, words[0]),
        _ => (words[..n - 2].to_vec(), words[n - 2], words[n - 1]),
    };
    let mut over = buffer.clone();
    over.push(source);
    let check_ok = whole && check_word(&over) == check;
    Received { framed: Framed { buffer, source, check, check_ok }, bits: count }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The addresses read off the wire are the ones [`frame`] put
    /// there.** [`header_on_the_wire`] takes them from the first 48 cells
    /// while the frame is still going by, and must agree with what
    /// [`unframe_any`] reads out of the whole run.
    #[test]
    fn the_header_on_the_wire_is_the_frames_own_addresses() {
        for (source, dest) in [(0o3050u16, 0o3060u16), (0o177001, 0), (0o4401, 0o4403)] {
            let buffer = vec![0o1000, 4, dest, 0, source, 0o21, 1, 0, 0x4954, 0x454d, dest];
            let bits = frame(&buffer, source);
            assert_eq!(
                header_on_the_wire(&bits[..48]),
                Some((source, dest)),
                "the source and destination 48 cells in"
            );
            assert_eq!(header_on_the_wire(&bits[..47]), None, "and not before they are whole");
            let r = unframe_any(&bits);
            assert_eq!((r.framed.source, r.framed.buffer.last().copied()), (source, Some(dest)));
        }
    }
}
