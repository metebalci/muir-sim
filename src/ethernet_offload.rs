// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **QUUX's Ethernet device: what it does to a frame's bytes** (the Ethernet
//! contract, section 2.4). The device understands IPv4. On transmit it
//! fills in the IPv4 header checksum and the TCP, UDP and ICMP checksums,
//! whatever QUUX left in their fields; on receive it drops a frame with a
//! wrong checksum or a malformed IPv4 header before the frame takes a
//! buffer. It takes two types, IPv4 (0x0800) and ARP (0x0806), and refuses
//! or drops every other: **the type filter**. An ARP frame passes both ways
//! untouched.
//!
//! This module holds no device state: [`transmit`] and [`receive`] look at
//! one frame. A frame is a whole Ethernet frame as the device's rings hold
//! it, from the destination address to the end of the data: 14 to 1514
//! bytes, with no FCS. A tap carries frames so, and the 1514 bytes are 14 of
//! header and a 1500-byte MTU. Byte `k` here is byte `k` of that frame.
//!
//! **Definitions**, for a frame of `n` bytes:
//! - the type is bytes 12-13, big-endian;
//! - an IPv4 frame is **well-formed** when `n >= 34`; byte 14 holds version
//!   4 and an IHL of 5 or more, giving `H = 4 * IHL` bytes of header,
//!   options included; and the total length `TL` (bytes 16-17) has
//!   `H <= TL` and `14 + TL <= n` (RFC 791 section 3.1);
//! - bytes from `14 + TL` on are Ethernet padding, never summed or changed;
//! - the payload is bytes `14 + H` to `14 + TL - 1`, `L = TL - H` bytes, of
//!   protocol `P` (byte 23);
//! - a **fragment** has MF (byte 20 `<5>`) set or a fragment offset (bytes
//!   20-21 `<12:0>`) other than 0 (RFC 791 section 3.1);
//! - a **checked protocol** is TCP (6), UDP (17) or ICMP (1) in a datagram
//!   that is not a fragment; it is well-formed only if TCP has `L >= 20`,
//!   UDP `L >= 8` and its length field (payload bytes 4-5) equal to `L`, and
//!   ICMP `L >= 8`;
//! - a checksum is the complement of the ones'-complement sum of big-endian
//!   16-bit words, an odd last byte padded with 0 (RFC 1071 section 3); for
//!   TCP and UDP the sum also covers the pseudo-header: bytes 26-33 (the two
//!   addresses), a zero byte, `P`, and `L` as 16 bits (RFC 9293 section
//!   3.1, RFC 768).
//!
//! **Transmit** refuses a frame of another type or one under 14 bytes, and
//! an IPv4 frame that is not well-formed; a refused frame is left as it was.
//! For a well-formed IPv4 frame it fills in bytes 24-25 over the `H` header
//! bytes, and for a checked protocol the checksum at payload bytes 16-17
//! (TCP), 6-7 (UDP) or 2-3 (ICMP), each field taken as 0. A UDP checksum
//! computed as 0 is sent as 0xFFFF (RFC 768); TCP, ICMP and the header keep
//! a computed 0. A fragment, or another protocol, gets the header checksum
//! only.
//!
//! **Receive** drops, counted by the type filter ([`RxCounter::OtherType`]),
//! a frame under 14 bytes or of another type; and, counted by the IPv4
//! checks ([`RxCounter::Ipv4Check`]), an IPv4 frame that is not
//! well-formed, whose header does not sum to 0xFFFF over its `H` bytes, or
//! whose checked protocol's payload (with the pseudo-header for TCP and
//! UDP) does not. A UDP checksum field of 0 means "no checksum": that
//! payload is not checked (RFC 768; RFC 1122 section 4.1.3.4).
//!
//! `tests/ethernet_vectors.rs` holds both directions to the checksum
//! vectors, whose results come from frames Linux made and from a checker
//! written apart from this module.

/// The EtherType of IPv4.
pub const TYPE_IPV4: u16 = 0x0800;
/// The EtherType of ARP.
pub const TYPE_ARP: u16 = 0x0806;

const TCP: u8 = 6;
const UDP: u8 = 17;
const ICMP: u8 = 1;

/// Why an IPv4 frame is not well-formed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Malformed {
    /// Fewer than 34 bytes.
    Short,
    /// Byte 14's version is not 4.
    Version,
    /// Byte 14's IHL is under 5.
    Ihl,
    /// The total length is under the header's length `H`.
    TotalUnderHeader,
    /// `14 + TL` is past the frame's end.
    TotalPastFrame,
    /// A TCP payload under 20 bytes.
    TcpShort,
    /// A UDP payload under 8 bytes.
    UdpShort,
    /// A UDP length field other than the payload's length `L`.
    UdpLength,
    /// An ICMP payload under 8 bytes.
    IcmpShort,
}

impl Malformed {
    /// What `--ethernet-trace` prints.
    pub fn reason(&self) -> &'static str {
        match self {
            Malformed::Short => "IPv4 frame under 34 bytes",
            Malformed::Version => "IPv4 version not 4",
            Malformed::Ihl => "IPv4 IHL under 5",
            Malformed::TotalUnderHeader => "IPv4 total length under the header's",
            Malformed::TotalPastFrame => "IPv4 total length past the frame",
            Malformed::TcpShort => "TCP payload under 20 bytes",
            Malformed::UdpShort => "UDP payload under 8 bytes",
            Malformed::UdpLength => "UDP length not the payload's",
            Malformed::IcmpShort => "ICMP payload under 8 bytes",
        }
    }
}

/// Why [`transmit`] refuses a frame. Every refusal is counted in status
/// `<31:24>`, with the device's other refusals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxRefusal {
    /// Under 14 bytes: no type. The type filter.
    NoType,
    /// A type other than IPv4 and ARP. The type filter.
    OtherType(u16),
    /// An IPv4 frame that is not well-formed.
    Malformed(Malformed),
}

impl TxRefusal {
    /// What `--ethernet-trace` prints.
    pub fn reason(&self) -> &'static str {
        match self {
            TxRefusal::NoType => "frame under 14 bytes",
            TxRefusal::OtherType(_) => "type not IPv4 or ARP",
            TxRefusal::Malformed(m) => m.reason(),
        }
    }
}

/// Why [`receive`] drops a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxDrop {
    /// Under 14 bytes: no type. The type filter.
    NoType,
    /// A type other than IPv4 and ARP. The type filter.
    OtherType(u16),
    /// An IPv4 frame that is not well-formed.
    Malformed(Malformed),
    /// The IPv4 header does not sum to 0xFFFF.
    HeaderChecksum,
    /// The TCP payload and pseudo-header do not sum to 0xFFFF.
    TcpChecksum,
    /// The UDP payload and pseudo-header do not sum to 0xFFFF, and the
    /// field is not 0.
    UdpChecksum,
    /// The ICMP payload does not sum to 0xFFFF.
    IcmpChecksum,
}

/// The counter of status word 241 a dropped frame advances.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxCounter {
    /// `<7:4>`: dropped by the type filter, mod 16.
    OtherType,
    /// `<15:9>`: dropped by the IPv4 checks, mod 128.
    Ipv4Check,
}

impl RxDrop {
    /// Which of 241's counters counts this drop.
    pub fn counter(&self) -> RxCounter {
        match self {
            RxDrop::NoType | RxDrop::OtherType(_) => RxCounter::OtherType,
            _ => RxCounter::Ipv4Check,
        }
    }

    /// What `--ethernet-trace` prints.
    pub fn reason(&self) -> &'static str {
        match self {
            RxDrop::NoType => "frame under 14 bytes",
            RxDrop::OtherType(_) => "type not IPv4 or ARP",
            RxDrop::Malformed(m) => m.reason(),
            RxDrop::HeaderChecksum => "IPv4 header checksum bad",
            RxDrop::TcpChecksum => "TCP checksum bad",
            RxDrop::UdpChecksum => "UDP checksum bad",
            RxDrop::IcmpChecksum => "ICMP checksum bad",
        }
    }
}

/// The ones'-complement sum of `data` as big-endian 16-bit words, an odd
/// last byte padded with 0, folded by end-around carry; not complemented
/// (RFC 1071 section 3).
pub fn ones_sum(data: &[u8]) -> u16 {
    fold(add(0, data))
}

/// Adds `data`'s words to a running sum. Every piece but the last is of
/// even length, so no word straddles two pieces.
fn add(mut sum: u64, data: &[u8]) -> u64 {
    let (words, rest) = data.as_chunks::<2>();
    for w in words {
        sum += u64::from(u16::from_be_bytes(*w));
    }
    if let [last] = rest {
        sum += u64::from(*last) << 8;
    }
    sum
}

fn fold(mut sum: u64) -> u16 {
    while sum > 0xFFFF {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    sum as u16
}

fn be16(f: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([f[at], f[at + 1]])
}

/// A well-formed IPv4 frame's parts.
struct Datagram {
    /// The header's length `H`, options included.
    h: usize,
    /// The payload's protocol `P`.
    p: u8,
    /// The payload: frame bytes `start..end`.
    start: usize,
    end: usize,
    /// For a checked protocol, the checksum field's offset in the payload.
    field: Option<usize>,
}

impl Datagram {
    /// The pseudo-header's sum for TCP and UDP, 0 for ICMP.
    fn pseudo(&self, f: &[u8]) -> u64 {
        match self.p {
            TCP | UDP => {
                let len = (self.end - self.start) as u16;
                add(add(0, &f[26..34]), &[0, self.p]) + u64::from(len)
            }
            _ => 0,
        }
    }

    /// The protocol's sum over the payload and pseudo-header, the field as
    /// it stands.
    fn sum(&self, f: &[u8]) -> u16 {
        fold(add(self.pseudo(f), &f[self.start..self.end]))
    }
}

/// A frame as the type filter and well-formedness see it.
enum Kind {
    NoType,
    OtherType(u16),
    Arp,
    Ipv4(Result<Datagram, Malformed>),
}

fn kind(f: &[u8]) -> Kind {
    if f.len() < 14 {
        return Kind::NoType;
    }
    match be16(f, 12) {
        TYPE_ARP => Kind::Arp,
        TYPE_IPV4 => Kind::Ipv4(datagram(f)),
        t => Kind::OtherType(t),
    }
}

fn datagram(f: &[u8]) -> Result<Datagram, Malformed> {
    let n = f.len();
    if n < 34 {
        return Err(Malformed::Short);
    }
    if f[14] >> 4 != 4 {
        return Err(Malformed::Version);
    }
    let ihl = usize::from(f[14] & 0x0F);
    if ihl < 5 {
        return Err(Malformed::Ihl);
    }
    let h = 4 * ihl;
    let tl = usize::from(be16(f, 16));
    if tl < h {
        return Err(Malformed::TotalUnderHeader);
    }
    if 14 + tl > n {
        return Err(Malformed::TotalPastFrame);
    }
    let p = f[23];
    let (start, end) = (14 + h, 14 + tl);
    let l = end - start;
    let mf = f[20] & 0x20 != 0;
    let offset = be16(f, 20) & 0x1FFF;
    let fragment = mf || offset != 0;
    let field = match p {
        _ if fragment => None,
        TCP if l < 20 => return Err(Malformed::TcpShort),
        TCP => Some(16),
        UDP if l < 8 => return Err(Malformed::UdpShort),
        UDP if usize::from(be16(f, start + 4)) != l => return Err(Malformed::UdpLength),
        UDP => Some(6),
        ICMP if l < 8 => return Err(Malformed::IcmpShort),
        ICMP => Some(2),
        _ => None,
    };
    Ok(Datagram { h, p, start, end, field })
}

/// Fill the IPv4 header checksum and the TCP/UDP/ICMP checksum in place, or refuse.
pub fn transmit(frame: &mut [u8]) -> Result<(), TxRefusal> {
    let d = match kind(frame) {
        Kind::NoType => return Err(TxRefusal::NoType),
        Kind::OtherType(t) => return Err(TxRefusal::OtherType(t)),
        Kind::Arp => return Ok(()),
        Kind::Ipv4(Err(m)) => return Err(TxRefusal::Malformed(m)),
        Kind::Ipv4(Ok(d)) => d,
    };
    frame[24..26].fill(0);
    let header = !ones_sum(&frame[14..14 + d.h]);
    frame[24..26].copy_from_slice(&header.to_be_bytes());
    if let Some(field) = d.field {
        let at = d.start + field;
        frame[at..at + 2].fill(0);
        let mut c = !d.sum(frame);
        if d.p == UDP && c == 0 {
            c = 0xFFFF;
        }
        frame[at..at + 2].copy_from_slice(&c.to_be_bytes());
    }
    Ok(())
}

/// Ok if the received Ethernet frame may take a buffer, else why it is dropped.
pub fn receive(frame: &[u8]) -> Result<(), RxDrop> {
    let d = match kind(frame) {
        Kind::NoType => return Err(RxDrop::NoType),
        Kind::OtherType(t) => return Err(RxDrop::OtherType(t)),
        Kind::Arp => return Ok(()),
        Kind::Ipv4(Err(m)) => return Err(RxDrop::Malformed(m)),
        Kind::Ipv4(Ok(d)) => d,
    };
    if ones_sum(&frame[14..14 + d.h]) != 0xFFFF {
        return Err(RxDrop::HeaderChecksum);
    }
    let Some(field) = d.field else { return Ok(()) };
    if d.p == UDP && be16(frame, d.start + field) == 0 {
        return Ok(());
    }
    if d.sum(frame) != 0xFFFF {
        return Err(match d.p {
            TCP => RxDrop::TcpChecksum,
            UDP => RxDrop::UdpChecksum,
            _ => RxDrop::IcmpChecksum,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each drop names its own reason, and the type filter's drops alone
    /// count in `<7:4>`.
    #[test]
    fn reasons_and_counters() {
        let all = [
            RxDrop::NoType,
            RxDrop::OtherType(0x86DD),
            RxDrop::Malformed(Malformed::Short),
            RxDrop::Malformed(Malformed::Version),
            RxDrop::Malformed(Malformed::Ihl),
            RxDrop::Malformed(Malformed::TotalUnderHeader),
            RxDrop::Malformed(Malformed::TotalPastFrame),
            RxDrop::Malformed(Malformed::TcpShort),
            RxDrop::Malformed(Malformed::UdpShort),
            RxDrop::Malformed(Malformed::UdpLength),
            RxDrop::Malformed(Malformed::IcmpShort),
            RxDrop::HeaderChecksum,
            RxDrop::TcpChecksum,
            RxDrop::UdpChecksum,
            RxDrop::IcmpChecksum,
        ];
        let reasons: std::collections::HashSet<_> = all.iter().map(|d| d.reason()).collect();
        assert_eq!(reasons.len(), all.len());
        for d in all {
            let by_type = matches!(d, RxDrop::NoType | RxDrop::OtherType(_));
            assert_eq!(d.counter() == RxCounter::OtherType, by_type, "{d:?}");
        }
        assert_eq!(TxRefusal::NoType.reason(), RxDrop::NoType.reason());
        assert_eq!(
            TxRefusal::Malformed(Malformed::UdpLength).reason(),
            "UDP length not the payload's"
        );
    }
}
