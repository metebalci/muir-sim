#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Mete Balci
# SPDX-License-Identifier: AGPL-3.0-or-later
"""The checker for QUUX's Ethernet checksum vectors (the Ethernet contract,
section 2.4): Python's standard library only, written from RFC 791 (IPv4),
768 (UDP), 792 (ICMP), 9293 (TCP) and RFC 1071 section 3 (the sum), and
sharing no code with muir's device.

    ethernet-vectors.py generate LINUX > tests/ethernet_vectors.txt
    ethernet-vectors.py check tests/ethernet_vectors.txt

`generate` builds the named edge cases, works out each one's expected result
with the rules below, and checks that against what the case was built to
show; it adds the frames Linux made, read from LINUX: lines of a name
starting `linux-` and the frame in hex (an `rx` line of a vectors file
serves too, so the committed file regenerates itself). `check` works out
every line of a vectors file again and says whether each agrees. Either
exits non-zero on a disagreement.

The file's format is in its own header, which `generate` writes.
"""
import struct
import sys

# ---- the sum (RFC 1071 section 3) ----


def ones_sum(data):
    """The ones'-complement sum of big-endian 16-bit words, an odd last byte
    padded with 0, folded by end-around carry. Not complemented."""
    total = 0
    for i in range(0, len(data) - 1, 2):
        total += data[i] * 256 + data[i + 1]
    if len(data) % 2:
        total += data[-1] * 256
    while total > 0xFFFF:
        total = (total >> 16) + (total & 0xFFFF)
    return total


def raw_sum(data):
    """The words' plain sum, not folded: only for building cases."""
    assert len(data) % 2 == 0
    return sum(data[i] * 256 + data[i + 1] for i in range(0, len(data), 2))


def checksum(data):
    return 0xFFFF - ones_sum(data)


# RFC 1071 section 3's worked example: the sum of 00 01 f2 03 f4 f5 f6 f7
# is ddf2, and its checksum 220d.
assert ones_sum(bytes.fromhex("0001f203f4f5f6f7")) == 0xDDF2
assert checksum(bytes.fromhex("0001f203f4f5f6f7")) == 0x220D

# ---- the rules ----

ETH_IPV4, ETH_ARP = 0x0800, 0x0806
TCP, UDP, ICMP = 6, 17, 1
FIELD = {TCP: 16, UDP: 6, ICMP: 2}
DELIVERED = "delivered"
DROP_TYPE = "dropped by type"
DROP_IPV4 = "dropped by the IPv4 checks"
REFUSED = "refused"


def be16(b, i):
    return b[i] * 256 + b[i + 1]


class Shape:
    """An IPv4 Ethernet frame's parts, or why it is not well-formed."""

    def __init__(self, f):
        self.bad = None
        n = len(f)
        if n < 34:
            self.bad = "shorter than 34 bytes"
            return
        version, ihl = f[14] >> 4, f[14] & 15
        if version != 4:
            self.bad = "version not 4"
            return
        if ihl < 5:
            self.bad = "IHL under 5"
            return
        self.h = 4 * ihl
        self.tl = be16(f, 16)
        if self.tl < self.h:
            self.bad = "total length under the header"
            return
        if 14 + self.tl > n:
            self.bad = "total length past the frame"
            return
        self.p = f[23]
        self.l = self.tl - self.h
        self.start = 14 + self.h
        self.end = 14 + self.tl
        mf = (f[20] >> 5) & 1
        offset = be16(f, 20) & 0x1FFF
        self.fragment = mf == 1 or offset != 0
        self.checked = not self.fragment and self.p in FIELD
        if self.checked:
            if self.p == TCP and self.l < 20:
                self.bad = "TCP under 20 bytes"
            elif self.p == UDP and self.l < 8:
                self.bad = "UDP under 8 bytes"
            elif self.p == UDP and be16(f, self.start + 4) != self.l:
                self.bad = "UDP length not the payload's"
            elif self.p == ICMP and self.l < 8:
                self.bad = "ICMP under 8 bytes"

    def pseudo(self, f):
        if self.p == ICMP:
            return b""
        return bytes(f[26:34]) + bytes([0, self.p]) + struct.pack("!H", self.l)


def etype(f):
    return be16(f, 12) if len(f) >= 14 else None


def receive(f):
    t = etype(f)
    if t not in (ETH_IPV4, ETH_ARP):
        return DROP_TYPE
    if t == ETH_ARP:
        return DELIVERED
    s = Shape(f)
    if s.bad:
        return DROP_IPV4
    if ones_sum(f[14 : 14 + s.h]) != 0xFFFF:
        return DROP_IPV4
    if s.checked:
        if s.p == UDP and be16(f, s.start + 6) == 0:
            return DELIVERED
        if ones_sum(s.pseudo(f) + bytes(f[s.start : s.end])) != 0xFFFF:
            return DROP_IPV4
    return DELIVERED


def transmit(f):
    """The frame sent, or None for refused."""
    t = etype(f)
    if t not in (ETH_IPV4, ETH_ARP):
        return None
    f = bytearray(f)
    if t == ETH_ARP:
        return bytes(f)
    s = Shape(f)
    if s.bad:
        return None
    f[24:26] = b"\0\0"
    f[24:26] = struct.pack("!H", checksum(f[14 : 14 + s.h]))
    if s.checked:
        at = s.start + FIELD[s.p]
        f[at : at + 2] = b"\0\0"
        c = checksum(s.pseudo(f) + bytes(f[s.start : s.end]))
        if s.p == UDP and c == 0:
            c = 0xFFFF
        f[at : at + 2] = struct.pack("!H", c)
    return bytes(f)


def expected(direction, f):
    if direction == "rx":
        return receive(f)
    out = transmit(f)
    return REFUSED if out is None else out.hex()


def zero_fields(f):
    """The frame with the fields transmit fills set to 0: QUUX's frame as it
    may hand it over."""
    f = bytearray(f)
    if etype(f) != ETH_IPV4:
        return bytes(f)
    s = Shape(f)
    if s.bad:
        return bytes(f)
    f[24:26] = b"\0\0"
    if s.checked:
        at = s.start + FIELD[s.p]
        f[at : at + 2] = b"\0\0"
    return bytes(f)


def junk_fields(f):
    f = bytearray(zero_fields(f))
    s = Shape(f)
    f[24:26] = b"\xde\xad"
    if s.checked:
        at = s.start + FIELD[s.p]
        f[at : at + 2] = b"\xbe\xef"
    return bytes(f)


# ---- building frames ----

MAC_A = bytes.fromhex("020000000001")
MAC_B = bytes.fromhex("020000000002")
IP_A = bytes([192, 0, 2, 1])
IP_B = bytes([192, 0, 2, 2])


def eth(t, payload, dst=MAC_A, src=MAC_B):
    return dst + src + struct.pack("!H", t) + payload


def ip_header(proto, payload_len, options=b"", ident=0x1234, frag=0x4000, ttl=64, src=IP_B, dst=IP_A, version=4, ihl=None, tl=None):
    assert len(options) % 4 == 0
    if ihl is None:
        ihl = 5 + len(options) // 4
    if tl is None:
        tl = 20 + len(options) + payload_len
    h = bytearray(struct.pack("!BBHHHBBH4s4s", (version << 4) | ihl, 0, tl, ident, frag, ttl, proto, 0, src, dst) + options)
    h[10:12] = struct.pack("!H", checksum(h))
    return bytes(h)


def l4(proto, seg, src=IP_B, dst=IP_A):
    """Fill seg's checksum the way RFC 768/792/9293 say."""
    seg = bytearray(seg)
    at = FIELD[proto]
    seg[at : at + 2] = b"\0\0"
    ph = b"" if proto == ICMP else src + dst + bytes([0, proto]) + struct.pack("!H", len(seg))
    c = checksum(ph + seg)
    if proto == UDP and c == 0:
        c = 0xFFFF
    seg[at : at + 2] = struct.pack("!H", c)
    return bytes(seg)


def tcp_seg(data=b"", flags=0x18, opts=b"", sport=1024, dport=23, seq=0x01020304, ack=0x0A0B0C0D):
    off = (20 + len(opts)) // 4
    return l4(TCP, struct.pack("!HHIIBBHHH", sport, dport, seq, ack, off << 4, flags, 8192, 0, 0) + opts + data)


def udp_seg(data=b"", sport=1025, dport=9, ulen=None):
    if ulen is None:
        ulen = 8 + len(data)
    return l4(UDP, struct.pack("!HHHH", sport, dport, ulen, 0) + data)


def icmp_seg(data=b"", t=8, ident=0x51, seq=1):
    return l4(ICMP, struct.pack("!BBHHH", t, 0, 0, ident, seq) + data)


def ipv4(proto, seg, pad=b"", **kw):
    return eth(ETH_IPV4, ip_header(proto, len(seg), **kw) + seg + pad)


def poke(f, at, b):
    f = bytearray(f)
    f[at : at + len(b)] = b
    return bytes(f)


def flip(f, at, mask=1):
    f = bytearray(f)
    f[at] ^= mask
    return bytes(f)


def swap(f, at):
    f = bytearray(f)
    f[at], f[at + 1] = f[at + 1], f[at]
    return bytes(f)


def refill_header(f):
    f = bytearray(f)
    h = 4 * (f[14] & 15)
    f[24:26] = b"\0\0"
    f[24:26] = struct.pack("!H", checksum(f[14 : 14 + h]))
    return bytes(f)


def zero_word(seg, at, extra_ph):
    """seg with the 16-bit word at `at` chosen so the sum of extra_ph and seg,
    its checksum field taken as 0, folds to 0xFFFF: its checksum computes to
    0."""
    seg = bytearray(seg)
    seg[at : at + 2] = b"\0\0"
    s = ones_sum(extra_ph + bytes(seg))
    seg[at : at + 2] = struct.pack("!H", 0xFFFF - s)
    assert ones_sum(extra_ph + bytes(seg)) == 0xFFFF
    return bytes(seg)


def ph(proto, length, src=IP_B, dst=IP_A):
    return src + dst + bytes([0, proto]) + struct.pack("!H", length)


def raw_l4(proto, seg):
    """seg with its checksum field 0, so the caller sets the field."""
    seg = bytearray(seg)
    seg[FIELD[proto] : FIELD[proto] + 2] = b"\0\0"
    return bytes(seg)


DATA_EVEN = bytes(range(32, 32 + 40))
DATA_ODD = b"an odd payload: 21 b."
assert len(DATA_ODD) % 2 == 1

SYN_OPTS = bytes.fromhex("020405b4") + bytes.fromhex("0402") + bytes.fromhex("080a0001e24000000000") + bytes.fromhex("01030307")
assert len(SYN_OPTS) == 20

RR9 = bytes([7, 39, 4]) + bytes(36) + b"\0"  # record route, 9 slots, then EOL
NOPS = bytes([1, 1, 1, 0])  # three NOPs and EOL


def cases():
    """(name, direction, frame, intended result): for tx the intent is
    REFUSED, or the frame sent, or a function taking the sent frame and
    saying whether it shows what the case is for."""
    out = []

    def both(name, f, rx, tx):
        out.append((name, "rx", f, rx))
        out.append((name, "tx", f, tx))

    def same(sent, f):
        return sent == f

    # -- ARP passes untouched, and is not taken as IPv4 --
    arp = struct.pack("!HHBBH6s4s6s4s", 1, ETH_IPV4, 6, 4, 1, MAC_B, IP_B, b"\0" * 6, IP_A)
    arp_padded = eth(ETH_ARP, arp + bytes([0xA5]) * 18, dst=b"\xff" * 6)
    both("arp-request-padded", arp_padded, DELIVERED, arp_padded)
    both("arp-header-only", eth(ETH_ARP, b""), DELIVERED, eth(ETH_ARP, b""))

    # -- the type filter --
    udp_frame = ipv4(UDP, udp_seg(DATA_EVEN))
    ipv6 = bytes.fromhex("6000000000083a40") + bytes.fromhex("20010db8000000000000000000000002") + bytes.fromhex("20010db8000000000000000000000001") + bytes.fromhex("8000000000010001")
    both("type-ipv6", eth(0x86DD, ipv6), DROP_TYPE, REFUSED)
    both("type-chaos-0804", eth(0x0804, bytes(32)), DROP_TYPE, REFUSED)
    both("type-vlan-8100", eth(0x8100, bytes.fromhex("00010800") + udp_frame[14:]), DROP_TYPE, REFUSED)
    both("type-lldp-88cc", eth(0x88CC, bytes(46)), DROP_TYPE, REFUSED)
    both("type-0000", eth(0x0000, bytes(46)), DROP_TYPE, REFUSED)
    both("type-ipv4-low-byte-first", poke(udp_frame, 12, b"\x00\x08"), DROP_TYPE, REFUSED)
    both("type-arp-low-byte-first", poke(arp_padded, 12, b"\x06\x08"), DROP_TYPE, REFUSED)
    both("short-13", udp_frame[:13], DROP_TYPE, REFUSED)
    both("short-12", udp_frame[:12], DROP_TYPE, REFUSED)

    # -- well-formed --
    both("ipv4-n14", udp_frame[:14], DROP_IPV4, REFUSED)
    both("ipv4-n33", udp_frame[:33], DROP_IPV4, REFUSED)
    v6 = refill_header(poke(udp_frame, 14, b"\x65"))
    both("ipv4-version-6", v6, DROP_IPV4, REFUSED)
    ihl4 = refill_header(poke(udp_frame, 14, b"\x44"))
    # IHL 4: the header checksum is right over 20 bytes, which an IHL taken
    # as at least 5 would accept.
    ihl4 = poke(ihl4, 24, b"\0\0")
    ihl4 = poke(ihl4, 24, struct.pack("!H", checksum(ihl4[14:34])))
    both("ipv4-ihl-4", ihl4, DROP_IPV4, REFUSED)
    # IHL 4 again, of a protocol with no checksum, its first 16 bytes
    # summing to 0xFFFF: a header taken as 16 bytes would pass.
    ihl4 = poke(eth(ETH_IPV4, ip_header(253, 8, ihl=4) + bytes(range(8))), 24, b"\0\0")
    ihl4 = poke(ihl4, 24, struct.pack("!H", checksum(ihl4[14:30])))
    both("ipv4-ihl-4-other-protocol", ihl4, DROP_IPV4, REFUSED)
    # H > n - 14: IHL 15, a 40-byte datagram.
    big_h = eth(ETH_IPV4, ip_header(253, 20, ihl=15, tl=40) + bytes(20))
    both("ipv4-header-past-frame", big_h, DROP_IPV4, REFUSED)
    seg6 = udp_seg(DATA_EVEN)
    both("ipv4-total-under-header", ipv4(UDP, seg6, options=NOPS, tl=23), DROP_IPV4, REFUSED)
    f = ipv4(UDP, udp_seg(DATA_EVEN))
    both("ipv4-total-past-frame", ipv4(UDP, udp_seg(DATA_EVEN), tl=len(f) - 14 + 1), DROP_IPV4, REFUSED)
    f = ipv4(253, bytes(range(10)))
    both("ipv4-total-past-frame-other-protocol", ipv4(253, bytes(range(10)), tl=len(f) - 14 + 1), DROP_IPV4, REFUSED)
    hdr_only = ipv4(253, b"")
    both("ipv4-header-only", hdr_only, DELIVERED, hdr_only)

    # -- a checked protocol's lengths --
    tcp19 = tcp_seg()[:19]
    both("tcp-l19", ipv4(TCP, tcp19), DROP_IPV4, REFUSED)
    tcp20 = ipv4(TCP, tcp_seg(flags=0x10))
    both("tcp-l20", tcp20, DELIVERED, tcp20)
    both("udp-l7", ipv4(UDP, udp_seg()[:7]), DROP_IPV4, REFUSED)
    # L 7 with a UDP length of 7, the field's one byte inside 0 and the
    # padding byte after it 0: a field taken as 0 would pass it.
    both("udp-l7-length-7", ipv4(UDP, struct.pack("!HHH", 1025, 9, 7) + b"\0", pad=bytes(20)), DROP_IPV4, REFUSED)
    udp8 = ipv4(UDP, udp_seg())
    both("udp-l8", udp8, DELIVERED, udp8)
    # UDP length one short and one long, the checksum right over L bytes.
    both("udp-length-short", ipv4(UDP, udp_seg(DATA_EVEN, ulen=8 + len(DATA_EVEN) - 1)), DROP_IPV4, REFUSED)
    both("udp-length-long", ipv4(UDP, udp_seg(DATA_EVEN, ulen=8 + len(DATA_EVEN) + 1)), DROP_IPV4, REFUSED)
    both("icmp-l7", ipv4(ICMP, icmp_seg()[:7]), DROP_IPV4, REFUSED)
    # The short ones again, each checksum right over its L bytes.
    both("tcp-l19-summed", ipv4(TCP, l4(TCP, tcp_seg()[:19])), DROP_IPV4, REFUSED)
    both("icmp-l7-summed", ipv4(ICMP, l4(ICMP, icmp_seg()[:7])), DROP_IPV4, REFUSED)
    icmp8 = ipv4(ICMP, icmp_seg())
    both("icmp-l8", icmp8, DELIVERED, icmp8)

    # -- the checksums filled in and checked --
    for name, seg, proto in [
        ("tcp-even", tcp_seg(DATA_EVEN), TCP),
        ("tcp-odd", tcp_seg(DATA_ODD), TCP),
        ("tcp-syn-options", tcp_seg(flags=0x02, opts=SYN_OPTS), TCP),
        ("udp-even", udp_seg(DATA_EVEN), UDP),
        ("udp-odd", udp_seg(DATA_ODD), UDP),
        ("udp-all-ones", udp_seg(b"\xff" * 64), UDP),
        ("icmp-echo", icmp_seg(bytes(range(56))), ICMP),
        ("icmp-odd", icmp_seg(DATA_ODD), ICMP),
    ]:
        good = ipv4(proto, seg)
        out.append((name, "rx", good, DELIVERED))
        out.append((name + "-zero-fields", "tx", zero_fields(good), good))
        out.append((name + "-junk-fields", "tx", junk_fields(good), good))
    for name, proto, seg, options in [
        ("tcp-ihl6", TCP, tcp_seg(DATA_EVEN), NOPS),
        ("udp-ihl15", UDP, udp_seg(DATA_ODD), RR9),
        ("icmp-ihl15", ICMP, icmp_seg(DATA_EVEN), RR9),
    ]:
        good = ipv4(proto, seg, options=options)
        out.append((name, "rx", good, DELIVERED))
        out.append((name + "-zero-fields", "tx", zero_fields(good), good))
    # The options are in the header's sum: one changed option byte.
    good = ipv4(UDP, udp_seg(DATA_ODD), options=RR9)
    out.append(("udp-ihl15-option-changed", "rx", flip(good, 14 + 20 + 10), DROP_IPV4))
    good = ipv4(TCP, tcp_seg(DATA_EVEN), options=NOPS)
    out.append(("tcp-ihl6-option-changed", "rx", flip(good, 14 + 21), DROP_IPV4))

    # A computed 0: UDP's goes out as 0xFFFF; TCP's, ICMP's and the
    # header's stay 0.
    useg = zero_word(raw_l4(UDP, struct.pack("!HHHH", 1025, 9, 8 + 12, 0) + b"zero-sum..xx"), 18, ph(UDP, 20))
    u0 = ipv4(UDP, useg)  # field 0
    assert be16(u0, 14 + 20 + 6) == 0
    u_ffff = poke(u0, 40, b"\xff\xff")
    out.append(("udp-computes-zero", "tx", u0, u_ffff))
    out.append(("udp-computes-zero-ffff", "rx", u_ffff, DELIVERED))
    tseg = zero_word(raw_l4(TCP, struct.pack("!HHIIBBHHH", 1024, 23, 1, 2, 5 << 4, 0x18, 8192, 0, 0) + b"zero-sum..xx"), 30, ph(TCP, 32))
    t0 = ipv4(TCP, tseg)
    assert be16(t0, 14 + 20 + 16) == 0
    out.append(("tcp-computes-zero", "tx", poke(t0, 50, b"\x12\x34"), t0))
    out.append(("tcp-computes-zero", "rx", t0, DELIVERED))
    iseg = zero_word(raw_l4(ICMP, struct.pack("!BBHHH", 8, 0, 0, 0x51, 1) + b"zero-sum..xx"), 18, b"")
    i0 = ipv4(ICMP, iseg)
    out.append(("icmp-computes-zero", "tx", poke(i0, 36, b"\x12\x34"), i0))
    out.append(("icmp-computes-zero", "rx", i0, DELIVERED))
    # The header: pick the identification so the header's checksum is 0.
    hdr = bytearray(ip_header(253, 8, ident=0))
    hdr[10:12] = b"\0\0"
    hdr[4:6] = struct.pack("!H", 0xFFFF - ones_sum(bytes(hdr)))
    assert checksum(bytes(hdr)) == 0
    h0 = eth(ETH_IPV4, bytes(hdr) + bytes(range(8)))
    out.append(("header-computes-zero", "tx", poke(h0, 24, b"\x55\xaa"), h0))
    out.append(("header-computes-zero", "rx", h0, DELIVERED))

    # A received UDP 0: no checksum, delivered; the rest still applies.
    u = ipv4(UDP, udp_seg(DATA_EVEN))
    u_none = poke(u, 40, b"\0\0")
    out.append(("udp-no-checksum", "rx", u_none, DELIVERED))
    out.append(("udp-no-checksum-data-changed", "rx", flip(u_none, 50), DELIVERED))
    out.append(("udp-no-checksum-header-bad", "rx", flip(u_none, 25), DROP_IPV4))
    out.append(("udp-no-checksum-length-short", "rx", poke(u_none, 38, struct.pack("!H", 8 + len(DATA_EVEN) - 1)), DROP_IPV4))
    out.append(("udp-no-checksum", "tx", u_none, u))

    # Each checksum wrong by one bit, and swapped byte for byte.
    for name, f, at in [
        ("header", ipv4(TCP, tcp_seg(DATA_EVEN)), 24),
        ("tcp", ipv4(TCP, tcp_seg(DATA_EVEN)), 50),
        ("udp", ipv4(UDP, udp_seg(DATA_EVEN)), 40),
        ("icmp", ipv4(ICMP, icmp_seg(bytes(range(56)))), 36),
    ]:
        assert f[at] != f[at + 1]
        out.append((name + "-checksum-bit", "rx", flip(f, at + 1), DROP_IPV4))
        out.append((name + "-checksum-swapped", "rx", swap(f, at), DROP_IPV4))
    out.append(("tcp-data-bit", "rx", flip(ipv4(TCP, tcp_seg(DATA_EVEN)), 60, 0x80), DROP_IPV4))
    # The pseudo-header: a source address changed and the header's checksum
    # made right again; only TCP's and UDP's sums see it.
    for name, f in [
        ("tcp", ipv4(TCP, tcp_seg(DATA_EVEN))),
        ("udp", ipv4(UDP, udp_seg(DATA_EVEN))),
    ]:
        out.append((name + "-pseudo-address", "rx", refill_header(flip(f, 29)), DROP_IPV4))
    f = ipv4(ICMP, icmp_seg(bytes(range(56))))
    out.append(("icmp-no-pseudo-header", "rx", refill_header(flip(f, 29)), DELIVERED))

    # -- padding past TL: never summed, never changed --
    pad = bytes([0x5A, 0xC3, 0x0F, 0xF0, 0x99, 0x66])
    for name, proto, seg in [
        ("tcp-odd-padded", TCP, tcp_seg(DATA_ODD)),
        ("udp-odd-padded", UDP, udp_seg(DATA_ODD)),
        ("icmp-odd-padded", ICMP, icmp_seg(DATA_ODD)),
        ("udp-short-padded", UDP, udp_seg(b"ab")),
    ]:
        good = ipv4(proto, seg, pad=pad + bytes(max(0, 60 - 14 - 20 - len(seg) - len(pad))))
        out.append((name, "rx", good, DELIVERED))
        out.append((name, "tx", zero_fields(good), good))
    hp = ipv4(253, bytes(3), pad=pad)
    both("header-only-padded", poke(hp, 24, b"\0\0"), DROP_IPV4, hp)

    # -- fragments and other protocols: the header only --
    junk_tcp = bytes(range(0x40, 0x40 + 30))
    for name, proto, payload, frag in [
        ("fragment-first-udp", UDP, b"\x04\x01\x00\x09\x00\x99\xbe\xef" + bytes(24), 0x2000),
        ("fragment-first-short-udp", UDP, b"\x04\x01\x00", 0x2000),
        ("fragment-later-tcp", TCP, junk_tcp[:8], 185),
        ("fragment-offset-1-tcp", TCP, junk_tcp, 1),
        ("fragment-last-icmp", ICMP, junk_tcp[:5], 0x0002),
        ("proto-47", 47, junk_tcp, 0x4000),
        ("proto-255-odd", 255, junk_tcp[:21], 0x0000),
    ]:
        good = eth(ETH_IPV4, ip_header(proto, len(payload), frag=frag) + payload)
        out.append((name, "rx", good, DELIVERED))
        out.append((name, "tx", poke(good, 24, b"\0\0"), good))
    # DF is not a fragment: a DF datagram's UDP checksum is still checked.
    df = ipv4(UDP, udp_seg(DATA_EVEN), frag=0x4000)
    out.append(("df-udp-checksum-bad", "rx", flip(df, 41), DROP_IPV4))
    out.append(("df-udp", "tx", zero_fields(df), df))
    nodf = ipv4(UDP, udp_seg(DATA_EVEN), frag=0x0000)
    out.append(("no-flags-udp-checksum-bad", "rx", flip(nodf, 41), DROP_IPV4))

    # A field of 0 is "no checksum" for UDP only.
    t = ipv4(TCP, tcp_seg(DATA_EVEN))
    out.append(("tcp-field-zero", "rx", poke(t, 50, b"\0\0"), DROP_IPV4))
    i = ipv4(ICMP, icmp_seg(bytes(range(56))))
    out.append(("icmp-field-zero", "rx", poke(i, 36, b"\0\0"), DROP_IPV4))

    # A sum that carries twice: its 16 low bits 0xFFFF before the first
    # fold, so the first fold carries out again.
    for name, proto, seg, at in [
        ("udp-double-carry", UDP, raw_l4(UDP, struct.pack("!HHHH", 1025, 9, 8 + 40, 0) + b"\xff" * 38 + b"\0\0"), 46),
        ("tcp-double-carry", TCP, raw_l4(TCP, struct.pack("!HHIIBBHHH", 1024, 23, 1, 2, 5 << 4, 0x18, 8192, 0, 0) + b"\xff" * 38 + b"\0\0"), 58),
        ("icmp-double-carry", ICMP, raw_l4(ICMP, struct.pack("!BBHHH", 8, 0, 0, 0x51, 1) + b"\xff" * 38 + b"\0\0"), 46),
    ]:
        extra = b"" if proto == ICMP else ph(proto, len(seg))
        seg = bytearray(seg)
        r = raw_sum(extra + bytes(seg))
        seg[at : at + 2] = struct.pack("!H", (0xFFFF - r) % 0x10000)
        assert raw_sum(extra + bytes(seg)) % 0x10000 == 0xFFFF and raw_sum(extra + bytes(seg)) > 0xFFFF
        f0 = FIELD[proto]
        seg[f0 : f0 + 2] = struct.pack("!H", checksum(extra + bytes(seg)))
        good = ipv4(proto, bytes(seg))
        out.append((name, "tx", zero_fields(good), good))
        out.append((name, "rx", good, DELIVERED))
    hdr = bytearray(ip_header(253, 8, ident=0))
    hdr[10:12] = b"\0\0"
    r = raw_sum(bytes(hdr))
    hdr[4:6] = struct.pack("!H", (0xFFFF - r) % 0x10000)
    assert raw_sum(bytes(hdr)) % 0x10000 == 0xFFFF and raw_sum(bytes(hdr)) > 0xFFFF
    hdr[10:12] = struct.pack("!H", checksum(bytes(hdr)))
    good = eth(ETH_IPV4, bytes(hdr) + bytes(range(8)))
    out.append(("header-double-carry", "tx", poke(good, 24, b"\0\0"), good))
    out.append(("header-double-carry", "rx", good, DELIVERED))
    return out


HEADER = """\
# QUUX's Ethernet checksum vectors: the Ethernet contract's section 2.4,
# for muir's device and the boards' program alike.
#
# One case a line, four fields separated by single spaces:
#   NAME DIRECTION FRAME RESULT
# NAME is unique with its DIRECTION. DIRECTION is `rx` or `tx`. FRAME is a
# whole Ethernet frame in lower-case hex, from the destination address to
# the end of the data, no FCS. For
# `tx`, RESULT is the frame sent, in hex, or `refused`. For `rx`, it is
# `delivered`, `dropped by type` or `dropped by the IPv4 checks`, the rest
# of the line. Lines starting with # and empty lines are comments.
#
# Cases named `linux-*` are frames Linux made (Linux 7.0, on a
# tap opened IFF_TAP | IFF_NO_PI): the `rx` line is the frame as Linux
# sent it, the `tx` line the same frame with the fields the device fills
# set to 0, or junk, and Linux's frame as the result. Every other case is
# made, and every result worked out, by tools/ethernet-vectors.py, Python's
# standard library written from RFC 791, 768, 792, 9293 and 1071, which
# shares no code with muir.
"""


def linux_frames(path):
    frames = []
    for line in open(path):
        line = line.rstrip("\n")
        if not line or line.startswith("#"):
            continue
        parts = line.split(" ")
        if not parts[0].startswith("linux-"):
            continue
        if len(parts) == 2:
            frames.append((parts[0], bytes.fromhex(parts[1])))
        elif parts[1] == "rx":
            frames.append((parts[0], bytes.fromhex(parts[2])))
    return frames


def generate(linux_path):
    lines = [HEADER]
    bad = 0
    seen = set()

    def emit(name, d, f, res):
        assert (name, d) not in seen, name
        seen.add((name, d))
        lines.append("%s %s %s %s" % (name, d, f.hex(), res))

    lines.append("# -- frames Linux made --")
    for name, f in linux_frames(linux_path):
        if receive(f) != DELIVERED:
            print("Linux frame not delivered:", name, receive(f), file=sys.stderr)
            bad += 1
        emit(name, "rx", f, DELIVERED)
        s = Shape(f) if etype(f) == ETH_IPV4 else None
        no_sum = s is not None and s.checked and s.p == UDP and be16(f, s.start + 6) == 0
        sent = transmit(zero_fields(f))
        if not no_sum and sent != f:
            print("Linux frame not refilled to itself:", name, file=sys.stderr)
            bad += 1
        emit(name, "tx", zero_fields(f), sent.hex())
        if s is not None and not no_sum and transmit(junk_fields(f)) != f:
            bad += 1
            print("Linux frame with junk not refilled:", name, file=sys.stderr)
        if s is not None and not no_sum and name.endswith(("-even", "-odd", "-options", "-ihl15", "-ihl6")):
            emit(name + "-junk", "tx", junk_fields(f), f.hex())
    lines.append("")
    lines.append("# -- made by the checker --")
    for name, d, f, intent in cases():
        res = expected(d, f)
        if d == "tx" and intent != REFUSED:
            ok = res != REFUSED and bytes.fromhex(res) == intent
        else:
            ok = res == intent
        if not ok:
            print("case %s %s: worked out %s, built for %s" % (name, d, res[:60], (intent.hex() if isinstance(intent, bytes) else intent)[:60]), file=sys.stderr)
            bad += 1
        emit(name, d, f, res)
    print("\n".join(lines))
    return bad


def check(path):
    bad = n = 0
    for line in open(path):
        line = line.rstrip("\n")
        if not line or line.startswith("#"):
            continue
        name, d, frame, res = line.split(" ", 3)
        n += 1
        got = expected(d, bytes.fromhex(frame))
        if got != res:
            print("%s %s: the file says %s, the checker %s" % (name, d, res[:60], got[:60]))
            bad += 1
        if name.startswith("linux-") and d == "rx" and res != DELIVERED:
            print("%s: a frame Linux made is not delivered" % name)
            bad += 1
    print("%d cases, %d disagree" % (n, bad))
    return bad


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "generate":
        sys.exit(1 if generate(sys.argv[2]) else 0)
    if len(sys.argv) == 3 and sys.argv[1] == "check":
        sys.exit(1 if check(sys.argv[2]) else 0)
    sys.exit(__doc__)
