//! LoRa's coding chain, both ways: whitening, the explicit header, the
//! payload CRC, the Hamming codes, the diagonal interleaver and the Gray map,
//! between payload bytes and symbol values (FFT bins of the dechirped
//! symbols, carrier offset removed).
//!
//! LoRa has no public specification. The layout here is the one
//! reverse-engineered and verified against Semtech chips by Tapparel et al.
//! and implemented in gr-lora_sdr (EPFL TCL, GPL-3.0): J. Tapparel,
//! O. Afisiadis, P. Mayoraz, A. Balatsoukas-Stimming, A. Burg, "An
//! Open-Source LoRa Physical Layer Prototype on GNU Radio", IEEE SPAWC 2020.
//! Each function names the gr-lora_sdr block it follows.
//!
//! Order on the air: 5 header nibbles, the payload bytes whitened (low nibble
//! first), the CRC (4 nibbles, not whitened). The first `sf - 2` nibbles go
//! out at coding rate 4/8 in one reduced-rate interleaver block of 8
//! symbols; the rest at the frame's own rate in blocks of `4 + cr` symbols,
//! reduced rate too when low-data-rate optimisation (LDRO) is on.

/// The whitening sequence XORed onto the payload bytes (gr-lora_sdr
/// `tables.h`).
const WHITENING: [u8; 255] = [
    0xFF, 0xFE, 0xFC, 0xF8, 0xF0, 0xE1, 0xC2, 0x85, 0x0B, 0x17, 0x2F, 0x5E, 0xBC, 0x78, 0xF1, 0xE3,
    0xC6, 0x8D, 0x1A, 0x34, 0x68, 0xD0, 0xA0, 0x40, 0x80, 0x01, 0x02, 0x04, 0x08, 0x11, 0x23, 0x47,
    0x8E, 0x1C, 0x38, 0x71, 0xE2, 0xC4, 0x89, 0x12, 0x25, 0x4B, 0x97, 0x2E, 0x5C, 0xB8, 0x70, 0xE0,
    0xC0, 0x81, 0x03, 0x06, 0x0C, 0x19, 0x32, 0x64, 0xC9, 0x92, 0x24, 0x49, 0x93, 0x26, 0x4D, 0x9B,
    0x37, 0x6E, 0xDC, 0xB9, 0x72, 0xE4, 0xC8, 0x90, 0x20, 0x41, 0x82, 0x05, 0x0A, 0x15, 0x2B, 0x56,
    0xAD, 0x5B, 0xB6, 0x6D, 0xDA, 0xB5, 0x6B, 0xD6, 0xAC, 0x59, 0xB2, 0x65, 0xCB, 0x96, 0x2C, 0x58,
    0xB0, 0x61, 0xC3, 0x87, 0x0F, 0x1F, 0x3E, 0x7D, 0xFB, 0xF6, 0xED, 0xDB, 0xB7, 0x6F, 0xDE, 0xBD,
    0x7A, 0xF5, 0xEB, 0xD7, 0xAE, 0x5D, 0xBA, 0x74, 0xE8, 0xD1, 0xA2, 0x44, 0x88, 0x10, 0x21, 0x43,
    0x86, 0x0D, 0x1B, 0x36, 0x6C, 0xD8, 0xB1, 0x63, 0xC7, 0x8F, 0x1E, 0x3C, 0x79, 0xF3, 0xE7, 0xCE,
    0x9C, 0x39, 0x73, 0xE6, 0xCC, 0x98, 0x31, 0x62, 0xC5, 0x8B, 0x16, 0x2D, 0x5A, 0xB4, 0x69, 0xD2,
    0xA4, 0x48, 0x91, 0x22, 0x45, 0x8A, 0x14, 0x29, 0x52, 0xA5, 0x4A, 0x95, 0x2A, 0x54, 0xA9, 0x53,
    0xA7, 0x4E, 0x9D, 0x3B, 0x77, 0xEE, 0xDD, 0xBB, 0x76, 0xEC, 0xD9, 0xB3, 0x67, 0xCF, 0x9E, 0x3D,
    0x7B, 0xF7, 0xEF, 0xDF, 0xBF, 0x7E, 0xFD, 0xFA, 0xF4, 0xE9, 0xD3, 0xA6, 0x4C, 0x99, 0x33, 0x66,
    0xCD, 0x9A, 0x35, 0x6A, 0xD4, 0xA8, 0x51, 0xA3, 0x46, 0x8C, 0x18, 0x30, 0x60, 0xC1, 0x83, 0x07,
    0x0E, 0x1D, 0x3A, 0x75, 0xEA, 0xD5, 0xAA, 0x55, 0xAB, 0x57, 0xAF, 0x5F, 0xBE, 0x7C, 0xF9, 0xF2,
    0xE5, 0xCA, 0x94, 0x28, 0x50, 0xA1, 0x42, 0x84, 0x09, 0x13, 0x27, 0x4F, 0x9F, 0x3F, 0x7F,
];

/// Symbols in the header's interleaver block (4 data + 4 parity bits).
pub const HEADER_SYMBOLS: usize = 8;

/// What the explicit header says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Payload bytes (1..=255).
    pub len: usize,
    /// Coding rate 4/(4 + cr), cr 1..=4.
    pub cr: u8,
    /// A CRC-16 follows the payload.
    pub crc: bool,
}

/// The CRC the chips append (gr-lora_sdr `add_crc`): CRC-16/XMODEM (poly
/// 0x1021, init 0) over all but the last two bytes, XORed with those two.
pub fn payload_crc(p: &[u8]) -> u16 {
    let n = p.len().saturating_sub(2);
    let mut crc = 0u16;
    for &b in &p[..n] {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    let at = |i: usize| p.get(i).map_or(0, |&b| u16::from(b));
    crc ^ at(n + 1) ^ (at(n) << 8)
}

/// The header's 5-bit checksum over its first 12 bits (gr-lora_sdr
/// `header`). `b(n, k)`: bit k of nibble n.
fn header_check(n: [u8; 3]) -> u8 {
    let b = |i: usize, k: u8| (n[i] >> k) & 1;
    let c4 = b(0, 3) ^ b(0, 2) ^ b(0, 1) ^ b(0, 0);
    let c3 = b(0, 3) ^ b(1, 3) ^ b(1, 2) ^ b(1, 1) ^ b(2, 0);
    let c2 = b(0, 2) ^ b(1, 3) ^ b(1, 0) ^ b(2, 3) ^ b(2, 1);
    let c1 = b(0, 1) ^ b(1, 2) ^ b(1, 0) ^ b(2, 2) ^ b(2, 1) ^ b(2, 0);
    let c0 = b(0, 0) ^ b(1, 1) ^ b(2, 3) ^ b(2, 2) ^ b(2, 1) ^ b(2, 0);
    c4 << 4 | c3 << 3 | c2 << 2 | c1 << 1 | c0
}

fn header_nibbles(h: Header) -> [u8; 5] {
    let n = [
        (h.len >> 4) as u8,
        (h.len & 0x0f) as u8,
        h.cr << 1 | u8::from(h.crc),
    ];
    let c = header_check(n);
    [n[0], n[1], n[2], c >> 4, c & 0x0f]
}

/// The header from its 5 nibbles, if the checksum holds and it makes sense.
fn parse_header(n: &[u8]) -> Option<Header> {
    let head = [n[0], n[1], n[2]];
    let h = Header {
        len: usize::from(n[0]) << 4 | usize::from(n[1]),
        cr: n[2] >> 1,
        crc: n[2] & 1 != 0,
    };
    let ok = header_check(head) == ((n[3] & 1) << 4 | n[4]) && h.len > 0 && (1..=4).contains(&h.cr);
    ok.then_some(h)
}

/// Hamming-code one nibble to `4 + cr` bits (gr-lora_sdr `hamming_enc`):
/// the data bits LSB first at the top, then the parity bits. 4/5 has one
/// parity bit over all four; 4/6 and 4/7 keep the first two and three of
/// 4/8's four.
fn hamming_encode(nib: u8, cr: u8) -> u8 {
    let b = |k: u8| (nib >> k) & 1;
    let data = b(0) << 3 | b(1) << 2 | b(2) << 1 | b(3);
    if cr == 1 {
        return data << 1 | (b(0) ^ b(1) ^ b(2) ^ b(3));
    }
    let p0 = b(0) ^ b(1) ^ b(2);
    let p1 = b(1) ^ b(2) ^ b(3);
    let p2 = b(0) ^ b(1) ^ b(3);
    let p3 = b(0) ^ b(2) ^ b(3);
    (data << 4 | p0 << 3 | p1 << 2 | p2 << 1 | p3) >> (4 - cr)
}

/// The nibble of a received codeword (gr-lora_sdr `hamming_dec`, hard
/// decisions): 4/7 and 4/8 correct one bit (the nearest codeword, unique at
/// distance 1); 4/5 and 4/6 can only detect, so their data bits are taken as
/// they came. Returns the nibble and whether the codeword was exact.
fn hamming_decode(cw: u8, cr: u8) -> (u8, bool) {
    let nibble = |cw: u8| {
        let d = cw >> cr;
        (d >> 3 & 1) | (d >> 1 & 2) | (d << 1 & 4) | (d << 3 & 8)
    };
    let n = nibble(cw);
    if hamming_encode(n, cr) == cw {
        return (n, true);
    }
    if cr >= 3
        && let Some(n) = (0..16u8).find(|&n| (hamming_encode(n, cr) ^ cw).count_ones() == 1)
    {
        return (n, false);
    }
    (n, false)
}

/// Interleave one block (gr-lora_sdr `interleaver`): `cws` holds `sf_app`
/// codewords of `cw_len` bits; symbol `i`'s row bit `j` (MSB first) is bit
/// `i` (MSB first) of codeword `(i - j - 1) mod sf_app`.
fn interleave(cws: &[u8], cw_len: usize, rows: &mut Vec<u16>) {
    let sf_app = cws.len();
    for i in 0..cw_len {
        let mut row = 0u16;
        for j in 0..sf_app {
            let cw = cws[(i + 2 * sf_app - j - 1) % sf_app];
            let bit = (cw >> (cw_len - 1 - i)) & 1;
            row |= u16::from(bit) << (sf_app - 1 - j);
        }
        rows.push(row);
    }
}

/// The inverse of [`interleave`]: `cw_len` rows of `sf_app` bits to
/// `sf_app` codewords.
fn deinterleave(rows: &[u16], sf_app: usize, cws: &mut Vec<u8>) {
    let cw_len = rows.len();
    for k in 0..sf_app {
        let mut cw = 0u8;
        for (i, &row) in rows.iter().enumerate() {
            let j = (i + 2 * sf_app - k - 1) % sf_app;
            let bit = (row >> (sf_app - 1 - j)) & 1;
            cw |= (bit as u8) << (cw_len - 1 - i);
        }
        cws.push(cw);
    }
}

/// Bits a symbol carries: `sf - 2` in a reduced-rate block, else `sf`.
fn sf_app(sf: u8, reduced: bool) -> usize {
    usize::from(if reduced { sf - 2 } else { sf })
}

/// A row to its symbol value (gr-lora_sdr `gray_demap`, TX side): inverse
/// Gray, then one up. A reduced-rate row also moves up two bits, its two
/// low bits zero (the interleaver's parity bit makes them so).
fn row_to_bin(row: u16, sf: u8, reduced: bool) -> u16 {
    let mut x = row;
    x ^= x >> 1;
    x ^= x >> 2;
    x ^= x >> 4;
    x ^= x >> 8;
    if reduced {
        x <<= 2;
    }
    (x + 1) & ((1 << sf) - 1)
}

/// A symbol value back to its row (gr-lora_sdr `fft_demod` and
/// `gray_mapping`): one down, then Gray. A reduced-rate symbol is divided
/// by 4 with rounding, so a bin off by one still maps right (gr-lora_sdr
/// truncates; rounding is never worse).
fn bin_to_row(bin: u16, sf: u8, reduced: bool) -> u16 {
    let mask = (1u16 << sf) - 1;
    let mut v = bin.wrapping_sub(1) & mask;
    if reduced {
        v = (v.wrapping_add(2) & mask) >> 2;
    }
    v ^ (v >> 1)
}

/// Symbols in a frame after the preamble: the header block and the
/// payload's (the SX127x datasheet's formula, nibble by nibble).
pub fn frame_symbols(h: Header, sf: u8, ldro: bool) -> usize {
    let nibbles = 5 + 2 * h.len + if h.crc { 4 } else { 0 };
    let rest = nibbles.saturating_sub(usize::from(sf - 2));
    HEADER_SYMBOLS + rest.div_ceil(sf_app(sf, ldro)) * (4 + usize::from(h.cr))
}

/// Low-data-rate optimisation as the Semtech drivers (and RadioLib) switch
/// it on: symbols of 16 ms or longer.
pub fn ldro_rule(sf: u8, bw_hz: f64) -> bool {
    f64::from(1u32 << sf) / bw_hz >= 0.016
}

/// A payload's symbol values, header first.
pub fn encode(payload: &[u8], sf: u8, cr: u8, crc: bool, ldro: bool) -> Vec<u16> {
    assert!((7..=12).contains(&sf) && (1..=4).contains(&cr) && (1..=255).contains(&payload.len()));
    let h = Header {
        len: payload.len(),
        cr,
        crc,
    };
    let mut nibbles: Vec<u8> = header_nibbles(h).to_vec();
    for (&b, &w) in payload.iter().zip(&WHITENING) {
        nibbles.extend([(b ^ w) & 0x0f, (b ^ w) >> 4]);
    }
    if crc {
        let c = payload_crc(payload);
        nibbles.extend((0..4).map(|k| (c >> (4 * k) & 0x0f) as u8));
    }

    let mut bins = Vec::with_capacity(frame_symbols(h, sf, ldro));
    let mut rows = Vec::new();
    let mut put = |chunk: &[u8], cr: u8, reduced: bool| {
        let sf_app = sf_app(sf, reduced);
        let mut cws: Vec<u8> = chunk.iter().map(|&n| hamming_encode(n, cr)).collect();
        cws.resize(sf_app, 0);
        rows.clear();
        interleave(&cws, 4 + usize::from(cr), &mut rows);
        bins.extend(rows.iter().map(|&r| row_to_bin(r, sf, reduced)));
    };
    let head = usize::from(sf - 2).min(nibbles.len());
    put(&nibbles[..head], 4, true);
    for chunk in nibbles[head..].chunks(sf_app(sf, ldro)) {
        put(chunk, cr, ldro);
    }
    bins
}

/// The header from the first [`HEADER_SYMBOLS`] symbol values, with the
/// payload nibbles that share its block.
pub fn decode_header(bins: &[u16], sf: u8) -> Option<(Header, Vec<u8>)> {
    let rows: Vec<u16> = bins[..HEADER_SYMBOLS]
        .iter()
        .map(|&b| bin_to_row(b, sf, true))
        .collect();
    let mut cws = Vec::new();
    deinterleave(&rows, usize::from(sf - 2), &mut cws);
    let nibbles: Vec<u8> = cws.iter().map(|&c| hamming_decode(c, 4).0).collect();
    let h = parse_header(&nibbles)?;
    Some((h, nibbles[5..].to_vec()))
}

/// What came out of a payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    pub bytes: Vec<u8>,
    /// The CRC matched (None: the header said there is none).
    pub crc_ok: Option<bool>,
    /// Codewords the Hamming code had to correct or could not check.
    pub bad_codewords: usize,
}

/// The payload from all the frame's symbol values (header block included),
/// given its header.
pub fn decode_payload(bins: &[u16], sf: u8, h: Header, ldro: bool) -> Option<Payload> {
    let (_, mut nibbles) = decode_header(bins, sf)?;
    let need = 2 * h.len + if h.crc { 4 } else { 0 };
    let block = 4 + usize::from(h.cr);
    let sf_app = sf_app(sf, ldro);
    let mut bad = 0;
    let (mut rows, mut cws) = (Vec::new(), Vec::new());
    for chunk in bins[HEADER_SYMBOLS..].chunks_exact(block) {
        if nibbles.len() >= need {
            break;
        }
        rows.clear();
        rows.extend(chunk.iter().map(|&b| bin_to_row(b, sf, ldro)));
        cws.clear();
        deinterleave(&rows, sf_app, &mut cws);
        for &c in &cws {
            let (n, exact) = hamming_decode(c, h.cr);
            bad += usize::from(!exact);
            nibbles.push(n);
        }
    }
    if nibbles.len() < need {
        return None;
    }
    // Rust note: `as_chunks::<2>()` views the nibbles as `[u8; 2]` pairs
    // (`.0`; `.1` is an odd one left over, never here).
    let mut bytes: Vec<u8> = nibbles
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| p[0] | p[1] << 4)
        .collect();
    let crc_ok = h.crc.then(|| {
        let rx = u16::from(bytes[h.len]) | u16::from(bytes[h.len + 1]) << 8;
        bytes.truncate(h.len);
        for (b, &w) in bytes.iter_mut().zip(&WHITENING) {
            *b ^= w;
        }
        payload_crc(&bytes) == rx
    });
    bytes.truncate(h.len);
    if !h.crc {
        for (b, &w) in bytes.iter_mut().zip(&WHITENING) {
            *b ^= w;
        }
    }
    Some(Payload {
        bytes,
        crc_ok,
        bad_codewords: bad,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hamming_round_trips_and_corrects_one_bit_at_4_7_and_4_8() {
        for cr in 1..=4u8 {
            for n in 0..16u8 {
                let cw = hamming_encode(n, cr);
                assert_eq!(hamming_decode(cw, cr), (n, true));
                if cr >= 3 {
                    for k in 0..4 + cr {
                        assert_eq!(
                            hamming_decode(cw ^ 1 << k, cr),
                            (n, false),
                            "cr {cr} n {n} bit {k}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn interleaver_and_gray_invert() {
        for sf in 7..=12u8 {
            for reduced in [false, true] {
                let sf_app = sf_app(sf, reduced);
                let cws: Vec<u8> = (0..sf_app).map(|k| (k * 37 + 11) as u8).collect();
                let mut rows = Vec::new();
                interleave(&cws, 8, &mut rows);
                let bins: Vec<u16> = rows.iter().map(|&r| row_to_bin(r, sf, reduced)).collect();
                let back: Vec<u16> = bins.iter().map(|&b| bin_to_row(b, sf, reduced)).collect();
                assert_eq!(back, rows);
                let mut out = Vec::new();
                deinterleave(&rows, sf_app, &mut out);
                assert_eq!(out, cws);
                if reduced {
                    // Reduced-rate symbols sit at 4x + 1, so one bin off is no error.
                    for &b in &bins {
                        assert_eq!(b % 4, 1);
                        assert_eq!(bin_to_row(b + 1, sf, true), bin_to_row(b, sf, true));
                        assert_eq!(bin_to_row(b - 1, sf, true), bin_to_row(b, sf, true));
                    }
                }
            }
        }
    }

    #[test]
    fn frames_round_trip_with_the_symbol_count_of_the_datasheet() {
        for sf in 7..=12u8 {
            for cr in 1..=4u8 {
                for ldro in [false, true] {
                    for len in [1usize, 16, 37, 255] {
                        let payload: Vec<u8> =
                            (0..len).map(|k| (k * 29 + sf as usize) as u8).collect();
                        let bins = encode(&payload, sf, cr, true, ldro);
                        let h = Header { len, cr, crc: true };
                        assert_eq!(bins.len(), frame_symbols(h, sf, ldro));
                        // Semtech AN1200.13: 8 + max(ceil((8PL - 4SF + 28 + 16) / (4(SF - 2DE))) (CR + 4), 0).
                        let de = if ldro { 2 } else { 0 };
                        let num = 8 * len as i64 - 4 * i64::from(sf) + 28 + 16;
                        let want = 8
                            + (num.max(0) as usize).div_ceil(4 * (usize::from(sf) - de))
                                * (usize::from(cr) + 4);
                        assert_eq!(bins.len(), want, "sf {sf} cr {cr} ldro {ldro} len {len}");
                        let (got, _) = decode_header(&bins, sf).expect("header");
                        assert_eq!(got, h);
                        let p = decode_payload(&bins, sf, h, ldro).expect("payload");
                        assert_eq!((p.bytes, p.crc_ok), (payload, Some(true)));
                    }
                }
            }
        }
    }

    #[test]
    fn a_wrong_bin_is_corrected_at_4_8_and_caught_by_the_crc_at_4_5() {
        let payload = b"meshtastic over the air".to_vec();
        let mut bins = encode(&payload, 9, 4, true, false);
        bins[20] ^= 1 << 5;
        let (h, _) = decode_header(&bins, 9).unwrap();
        assert_eq!(
            decode_payload(&bins, 9, h, false).unwrap().crc_ok,
            Some(true)
        );
        let mut bins = encode(&payload, 9, 1, true, false);
        bins[20] ^= 1 << 5;
        let (h, _) = decode_header(&bins, 9).unwrap();
        assert_eq!(
            decode_payload(&bins, 9, h, false).unwrap().crc_ok,
            Some(false)
        );
    }
}
