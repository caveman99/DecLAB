//! Meshtastic over the air: the 16-byte header the firmware puts before
//! each packet, AES-CTR with the channel's key, the channel hash, the modem
//! presets; and the decrypted `Data` picked apart with the official
//! protobufs (the `meshtastic` crate).
//!
//! The authority is the firmware (meshtastic/firmware, GPL-3.0):
//! `src/mesh/RadioInterface.h` (`PacketHeader`, flag masks),
//! `src/mesh/CryptoEngine.cpp` (`initNonce`, `encryptAESCtr`),
//! `src/mesh/Channels.cpp` (`xorHash`, `hashFor`, short PSK expansion),
//! `src/mesh/Channels.h` (`defaultpsk`), `src/mesh/MeshRadio.h`
//! (`modemPresetToParams`), `src/mesh/RadioLibInterface.h` (sync word).
//!
//! Only channel packets with a known key are decrypted: the default key and
//! its short-PSK siblings (`AQ==` and up), and unencrypted channels. Direct
//! messages encrypted to a node's public key (PKI) stay encrypted.

use aes::cipher::{BlockEncrypt, KeyInit};
use meshtastic::Message;
use meshtastic::protobufs::{self, PortNum};

/// The LoRa sync word Meshtastic sets (`RadioLibInterface.h`).
pub const SYNC_WORD: u8 = 0x2B;
/// `PacketHeader`'s size.
pub const HEADER_LEN: usize = 16;
/// The `to` of a broadcast.
pub const BROADCAST: u32 = 0xFFFF_FFFF;

/// The default channel key (`Channels.h`, `defaultpsk`): what the 1-byte
/// PSK `AQ==` (index 1) expands to.
pub const DEFAULT_PSK: [u8; 16] = [
    0xd4, 0xf1, 0xbb, 0x3a, 0x20, 0x29, 0x07, 0x59, 0xf0, 0xbc, 0xff, 0xab, 0xcf, 0x4e, 0x69, 0x01,
];

/// The key a 1-byte PSK stands for (`Channels::getKey`): the default key
/// with its last byte raised by `index − 1`.
pub fn short_psk(index: u8) -> [u8; 16] {
    let mut k = DEFAULT_PSK;
    k[15] = k[15].wrapping_add(index.wrapping_sub(1));
    k
}

/// Short PSK indices tried on each packet (1 is the default key).
const SHORT_PSKS: std::ops::RangeInclusive<u8> = 1..=10;

/// The header before every packet, little-endian (`PacketHeader`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Header {
    pub to: u32,
    pub from: u32,
    pub id: u32,
    /// Hops left; `hop_start` is what the sender started with.
    pub hop_limit: u8,
    pub want_ack: bool,
    pub via_mqtt: bool,
    pub hop_start: u8,
    /// The channel hash (0 for a PKI direct message).
    pub channel: u8,
    /// Last byte of the next hop's and of the relaying node's number.
    pub next_hop: u8,
    pub relay_node: u8,
}

impl Header {
    pub fn parse(b: &[u8]) -> Option<Header> {
        let u32_at = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        (b.len() >= HEADER_LEN).then(|| Header {
            to: u32_at(0),
            from: u32_at(4),
            id: u32_at(8),
            hop_limit: b[12] & 0x07,
            want_ack: b[12] & 0x08 != 0,
            via_mqtt: b[12] & 0x10 != 0,
            hop_start: b[12] >> 5,
            channel: b[13],
            next_hop: b[14],
            relay_node: b[15],
        })
    }

    pub fn bytes(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[0..4].copy_from_slice(&self.to.to_le_bytes());
        b[4..8].copy_from_slice(&self.from.to_le_bytes());
        b[8..12].copy_from_slice(&self.id.to_le_bytes());
        b[12] = self.hop_limit & 0x07
            | u8::from(self.want_ack) << 3
            | u8::from(self.via_mqtt) << 4
            | (self.hop_start & 0x07) << 5;
        b[13] = self.channel;
        b[14] = self.next_hop;
        b[15] = self.relay_node;
        b
    }
}

/// `!a1b2c3d4`, as the apps write a node number.
pub fn node_id(num: u32) -> String {
    if num == BROADCAST {
        "^all".into()
    } else {
        format!("!{num:08x}")
    }
}

fn xor_hash(b: &[u8]) -> u8 {
    b.iter().fold(0, |h, &x| h ^ x)
}

/// The channel hash in the header (`Channels::hashFor`).
pub fn channel_hash(name: &str, key: &[u8]) -> u8 {
    xor_hash(name.as_bytes()) ^ xor_hash(key)
}

/// AES-CTR as the firmware runs it (`CryptoEngine`): the nonce is the
/// packet id (64-bit LE) and the sender (32-bit LE), the last 4 bytes a
/// big-endian block counter from 0. Encrypts and decrypts alike. Keys of 16
/// or 32 bytes; anything else leaves `data` alone.
pub fn crypt(key: &[u8], from: u32, id: u32, data: &mut [u8]) {
    let mut nonce = [0u8; 16];
    nonce[..8].copy_from_slice(&u64::from(id).to_le_bytes());
    nonce[8..12].copy_from_slice(&from.to_le_bytes());
    // Rust note: a closure over whichever cipher the key size picks, so the
    // counter loop is written once.
    let mut run = |encrypt: &dyn Fn(&mut aes::Block)| {
        for (i, chunk) in data.chunks_mut(16).enumerate() {
            let mut block = aes::Block::from(nonce);
            block[12..].copy_from_slice(&(i as u32).to_be_bytes());
            encrypt(&mut block);
            for (d, k) in chunk.iter_mut().zip(block.iter()) {
                *d ^= k;
            }
        }
    };
    match key.len() {
        16 => {
            let c = aes::Aes128::new_from_slice(key).expect("16-byte key");
            run(&|b| c.encrypt_block(b));
        }
        32 => {
            let c = aes::Aes256::new_from_slice(key).expect("32-byte key");
            run(&|b| c.encrypt_block(b));
        }
        _ => {}
    }
}

/// A modem preset (`modemPresetToParams`, display names from
/// `DisplayFormatters`): its bandwidth below 1 GHz and on 2.4 GHz (kHz),
/// spreading factor and coding rate 4/cr.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Preset {
    pub name: &'static str,
    pub bw_khz: f64,
    pub bw_wide_khz: f64,
    pub sf: u8,
    pub cr: u8,
}

const fn preset(name: &'static str, bw_khz: f64, bw_wide_khz: f64, sf: u8, cr: u8) -> Preset {
    Preset {
        name,
        bw_khz,
        bw_wide_khz,
        sf,
        cr,
    }
}

pub const PRESETS: [Preset; 16] = [
    preset("LongFast", 250.0, 812.5, 11, 5),
    preset("LongSlow", 125.0, 406.25, 12, 8),
    preset("LongMod", 125.0, 406.25, 11, 8),
    preset("LongTurbo", 500.0, 1625.0, 11, 8),
    preset("MediumFast", 250.0, 812.5, 9, 5),
    preset("MediumSlow", 250.0, 812.5, 10, 5),
    preset("MediumTurbo", 500.0, 1625.0, 9, 5),
    preset("ShortFast", 250.0, 812.5, 7, 5),
    preset("ShortSlow", 250.0, 812.5, 8, 5),
    preset("ShortTurbo", 500.0, 1625.0, 7, 5),
    preset("LiteFast", 125.0, 125.0, 9, 5),
    preset("LiteSlow", 125.0, 125.0, 10, 5),
    preset("NarrowFast", 62.5, 62.5, 7, 6),
    preset("NarrowSlow", 62.5, 62.5, 8, 6),
    preset("TinyFast", 15.625, 15.625, 7, 5),
    preset("TinySlow", 15.625, 15.625, 8, 6),
];

/// The preset a frame's modulation matches, if any (`cr`: the LoRa
/// header's 1..=4, i.e. 4/(4 + cr)).
pub fn preset_for(sf: u8, bw_hz: f64, cr: u8) -> Option<&'static Preset> {
    let close = |khz: f64| (khz * 1e3 - bw_hz).abs() < 1.0;
    PRESETS
        .iter()
        .find(|p| p.sf == sf && p.cr == cr + 4 && (close(p.bw_khz) || close(p.bw_wide_khz)))
}

/// How a packet's payload was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// An unencrypted channel.
    None,
    /// A 1-byte PSK (1: the default key, `AQ==`).
    Short(u8),
}

/// A Meshtastic packet off the air.
#[derive(Debug, Clone, PartialEq)]
pub struct Packet {
    pub header: Header,
    /// The channel's name, when its hash is a preset's with the key found.
    pub channel: Option<&'static str>,
    pub key: Option<Key>,
    /// The decrypted `Data`, when a key fitted.
    pub data: Option<protobufs::Data>,
    /// The payload after the header: decrypted when a key fitted.
    pub payload: Vec<u8>,
}

/// Plausibly a `Data` message: nanopb writes `portnum` first, and the rest
/// must parse.
fn as_data(b: &[u8]) -> Option<protobufs::Data> {
    if b.first() != Some(&0x08) {
        return None;
    }
    let d = protobufs::Data::decode(b).ok()?;
    PortNum::try_from(d.portnum).ok().map(|_| d)
}

/// A LoRa payload as a Meshtastic packet: the header, and the `Data` if
/// one of the known keys opens it.
pub fn decode(frame: &[u8]) -> Option<Packet> {
    let header = Header::parse(frame)?;
    let body = &frame[HEADER_LEN..];
    let mut found = None;
    if header.channel != 0 || header.to == BROADCAST {
        if let Some(d) = as_data(body) {
            found = Some((Key::None, d, body.to_vec()));
        }
        for i in SHORT_PSKS {
            if found.is_some() {
                break;
            }
            let mut b = body.to_vec();
            crypt(&short_psk(i), header.from, header.id, &mut b);
            found = as_data(&b).map(|d| (Key::Short(i), d, b));
        }
    }
    let Some((key, data, payload)) = found else {
        return Some(Packet {
            header,
            channel: None,
            key: None,
            data: None,
            payload: body.to_vec(),
        });
    };
    let k = match key {
        Key::Short(i) => short_psk(i).to_vec(),
        Key::None => Vec::new(),
    };
    let channel = PRESETS
        .iter()
        .map(|p| p.name)
        .find(|n| channel_hash(n, &k) == header.channel);
    Some(Packet {
        header,
        channel,
        key: Some(key),
        data: Some(data),
        payload,
    })
}

/// A packet on a channel, encrypted with `key` (empty: none): what a node
/// transmits.
pub fn encode(header: &Header, data: &protobufs::Data, key: &[u8]) -> Vec<u8> {
    let mut body = data.encode_to_vec();
    crypt(key, header.from, header.id, &mut body);
    let mut b = header.bytes().to_vec();
    b.extend(body);
    b
}

/// The port's name as the protobufs have it ("TEXT_MESSAGE_APP").
pub fn port_name(port: i32) -> String {
    PortNum::try_from(port).map_or_else(|_| format!("port {port}"), |p| p.as_str_name().to_string())
}

/// A position report (`Position`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Fix {
    pub lat: f64,
    pub lon: f64,
    pub altitude_m: Option<f64>,
    /// Seconds since 1970, as the node's clock had it.
    pub time: Option<u32>,
    pub sats: u32,
}

/// What a `Data` says, for the log and the card: a short summary and,
/// where it has one, a position or a node's names.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Summary {
    pub text: String,
    pub fix: Option<Fix>,
    /// A node's own description (`User`): short name, long name.
    pub names: Option<(String, String)>,
    /// A text message.
    pub message: bool,
}

pub fn summarise(d: &protobufs::Data) -> Summary {
    let p = &d.payload[..];
    let mut s = Summary::default();
    match PortNum::try_from(d.portnum) {
        Ok(PortNum::TextMessageApp) => {
            s.text = String::from_utf8_lossy(p).into_owned();
            s.message = true;
        }
        Ok(PortNum::PositionApp) => {
            if let Ok(pos) = protobufs::Position::decode(p) {
                if let (Some(lat), Some(lon)) = (pos.latitude_i, pos.longitude_i) {
                    let f = Fix {
                        lat: f64::from(lat) * 1e-7,
                        lon: f64::from(lon) * 1e-7,
                        altitude_m: pos.altitude.map(f64::from),
                        time: (pos.time != 0).then_some(pos.time),
                        sats: pos.sats_in_view,
                    };
                    s.text = format!("{:.5}, {:.5}", f.lat, f.lon);
                    if let Some(a) = f.altitude_m {
                        s.text += &format!(", {a:.0} m");
                    }
                    s.fix = Some(f);
                } else {
                    s.text = "no fix".into();
                }
            }
        }
        Ok(PortNum::NodeinfoApp) => {
            if let Ok(u) = protobufs::User::decode(p) {
                let hw = protobufs::HardwareModel::try_from(u.hw_model).map_or_else(
                    |_| format!("hw {}", u.hw_model),
                    |h| h.as_str_name().to_string(),
                );
                s.text = format!("{} \"{}\" {}", u.short_name, u.long_name, hw);
                s.names = Some((u.short_name, u.long_name));
            }
        }
        Ok(PortNum::TelemetryApp) => {
            if let Ok(t) = protobufs::Telemetry::decode(p) {
                use protobufs::telemetry::Variant;
                s.text = match t.variant {
                    Some(Variant::DeviceMetrics(m)) => {
                        let mut v = Vec::new();
                        if let Some(b) = m.battery_level {
                            v.push(format!("battery {b} %"));
                        }
                        if let Some(x) = m.voltage {
                            v.push(format!("{x:.2} V"));
                        }
                        if let Some(x) = m.channel_utilization {
                            v.push(format!("channel {x:.1} %"));
                        }
                        if let Some(x) = m.air_util_tx {
                            v.push(format!("air TX {x:.1} %"));
                        }
                        format!("device: {}", v.join(", "))
                    }
                    Some(Variant::EnvironmentMetrics(m)) => {
                        let mut v = Vec::new();
                        if let Some(x) = m.temperature {
                            v.push(format!("{x:.1} °C"));
                        }
                        if let Some(x) = m.relative_humidity {
                            v.push(format!("{x:.0} % RH"));
                        }
                        if let Some(x) = m.barometric_pressure {
                            v.push(format!("{x:.1} hPa"));
                        }
                        format!("environment: {}", v.join(", "))
                    }
                    Some(_) => "other metrics".into(),
                    None => String::new(),
                };
            }
        }
        Ok(PortNum::TracerouteApp) => {
            if let Ok(r) = protobufs::RouteDiscovery::decode(p) {
                let hops: Vec<String> = r.route.iter().map(|&n| node_id(n)).collect();
                s.text = format!("route [{}]", hops.join(" "));
            }
        }
        Ok(PortNum::NeighborinfoApp) => {
            if let Ok(n) = protobufs::NeighborInfo::decode(p) {
                s.text = format!("{} neighbours", n.neighbors.len());
            }
        }
        Ok(PortNum::RoutingApp) => {
            if let Ok(r) = protobufs::Routing::decode(p) {
                s.text = match r.variant {
                    Some(protobufs::routing::Variant::ErrorReason(e)) => {
                        protobufs::routing::Error::try_from(e)
                            .map_or_else(|_| format!("error {e}"), |e| e.as_str_name().to_string())
                    }
                    Some(_) => "route".into(),
                    None => String::new(),
                };
            }
        }
        _ => s.text = format!("{} bytes", p.len()),
    }
    if d.request_id != 0 {
        s.text += &format!(" (reply to {:08x})", d.request_id);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default primary channel: LongFast with the default key hashes to
    /// 8 (the number the apps show for it).
    #[test]
    fn longfast_hashes_to_8() {
        assert_eq!(channel_hash("LongFast", &DEFAULT_PSK), 8);
    }

    #[test]
    fn a_text_message_round_trips_through_the_default_key() {
        let header = Header {
            to: BROADCAST,
            from: 0x1234_5678,
            id: 0xdead_beef,
            hop_limit: 3,
            hop_start: 3,
            channel: channel_hash("MediumFast", &DEFAULT_PSK),
            ..Header::default()
        };
        let data = protobufs::Data {
            portnum: PortNum::TextMessageApp as i32,
            payload: b"hello mesh".to_vec(),
            ..Default::default()
        };
        let frame = encode(&header, &data, &DEFAULT_PSK);
        assert_ne!(&frame[HEADER_LEN..], &data.encode_to_vec()[..], "encrypted");
        let p = decode(&frame).expect("a packet");
        assert_eq!(p.header, header);
        assert_eq!(p.key, Some(Key::Short(1)));
        assert_eq!(p.channel, Some("MediumFast"));
        assert_eq!(p.data.as_ref(), Some(&data));
        assert!(summarise(&data).message);
        // A key it does not know leaves it encrypted.
        let other = encode(&header, &data, &[7u8; 16]);
        assert_eq!(decode(&other).unwrap().data, None);
    }
}
