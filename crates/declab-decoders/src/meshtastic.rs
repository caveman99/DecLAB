//! Meshtastic: LoRa frames of any spreading factor on one channel (the SF
//! found by trying them all, the coding rate read from each frame's
//! header), and Meshtastic's packets in them, decrypted where the channel
//! uses the default key or one of its short-PSK siblings. The LoRa receiver
//! and the Meshtastic layer are `crates/declab-lora`; this file wraps them.
//!
//! Two decoders, as the host's channel is fixed per decoder: below 1 GHz
//! (1 MS/s, a 500 kHz channel: LoRa bandwidths up to 500 kHz) and at
//! 2.4 GHz (SX128x: 3.25 MS/s, 1625 kHz). The LoRa bandwidth is a setting
//! (`bandwidth`); the channel is low-passed and decimated to twice it here.
//!
//! Out: every frame to the log and as a data message (header and decrypted
//! `Data`, or the frame as it came); positions as locations; channels and
//! nodes heard; the card has the modulation, the latest SNR against what
//! the SF needs, the counts and the latest packets.

use std::collections::{HashMap, VecDeque};

use declab_api::{
    Block, Card, Command, DataPacket, Decoder, DecoderFactory, DecoderInfo, Family, Gauge,
    InputSpec, Iq, Location, Lock, Sink,
};
use declab_dsp::ddc::{Stage, lowpass};
use declab_lora::meshtastic::{self as mesh, Key, Packet};
use declab_lora::rx::{Frame, OS, Receiver};

use crate::heard::HeardList;

/// Packets kept on the card.
const RECENT: usize = 10;
/// The card says "locked" while frames came this recently, s.
const LOCKED_FOR_S: f64 = 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    /// 433, 868, 915 MHz and the like (SX126x, SX127x, LR11x0).
    SubGhz,
    /// 2.4 GHz (SX128x; Meshtastic's LORA_24 region).
    Wide,
}

impl Band {
    fn rate(self) -> f64 {
        match self {
            Band::SubGhz => 1e6,
            Band::Wide => 3.25e6,
        }
    }

    fn channel_hz(self) -> f64 {
        match self {
            Band::SubGhz => 500e3,
            Band::Wide => 1625e3,
        }
    }

    /// The LoRa bandwidths the panel offers (Meshtastic's presets use these;
    /// `MeshRadio.h`, `modemPresetToParams`).
    fn bandwidths(self) -> &'static [f64] {
        match self {
            Band::SubGhz => &[15_625.0, 62_500.0, 125e3, 250e3, 500e3],
            Band::Wide => &[203_125.0, 406_250.0, 812_500.0, 1_625e3],
        }
    }

    /// LongFast's.
    fn default_bw(self) -> f64 {
        match self {
            Band::SubGhz => 250e3,
            Band::Wide => 812_500.0,
        }
    }

    /// Upchirps before the sync word (`RadioInterface.h`).
    fn preamble(self) -> f64 {
        match self {
            Band::SubGhz => 16.0,
            Band::Wide => 12.0,
        }
    }
}

pub struct Factory(pub Band);

impl DecoderFactory for Factory {
    fn info(&self) -> DecoderInfo {
        match self.0 {
            Band::SubGhz => DecoderInfo {
                id: "meshtastic",
                name: "Meshtastic",
                family: Family::Data,
                standard: "LoRa (Semtech), Meshtastic firmware",
                summary: "Meshtastic packets below 1 GHz: any SF 7–12, coding rate from the header, default-key channels decrypted",
            },
            Band::Wide => DecoderInfo {
                id: "meshtastic-2g4",
                name: "Meshtastic 2.4 GHz",
                family: Family::Data,
                standard: "LoRa (Semtech SX128x), Meshtastic firmware",
                summary: "Meshtastic packets at 2.4 GHz (LORA_24): any SF 7–12, coding rate from the header",
            },
        }
    }

    fn input(&self, _source_rate: f64) -> InputSpec {
        InputSpec {
            sample_rate: Some(self.0.rate()),
            bandwidth_hz: self.0.channel_hz(),
            block_len: 65_536,
        }
    }

    fn create(&self) -> Box<dyn Decoder> {
        Box::new(Mesh::new(self.0, self.0.default_bw()))
    }

    #[cfg(feature = "ui")]
    fn panel(&self) -> Option<Box<dyn declab_api::Panel>> {
        Some(Box::new(panel::MeshPanel {
            band: self.0,
            bw: self.0.default_bw(),
        }))
    }
}

/// "250 kHz", "812.5 kHz", "1.625 MHz".
fn bw_label(bw: f64) -> String {
    if bw >= 1e6 {
        format!("{} MHz", bw / 1e6)
    } else {
        format!("{} kHz", bw / 1e3)
    }
}

/// A bandwidth from the command line or the panel: "250k", "812.5k",
/// "1.625M", "250000".
fn parse_bw(v: &str) -> Option<f64> {
    let v = v.trim().trim_end_matches("Hz").trim_end_matches("hz");
    let (num, mult) = match v.chars().last()? {
        'k' | 'K' => (&v[..v.len() - 1], 1e3),
        'M' | 'm' => (&v[..v.len() - 1], 1e6),
        _ => (v, 1.0),
    };
    num.trim().parse::<f64>().ok().map(|x| x * mult)
}

/// SNR each spreading factor demodulates at (Semtech SX1276 datasheet,
/// table 13): −7.5 dB at SF7, 2.5 dB lower each step.
fn snr_floor(sf: u8) -> f32 {
    -7.5 - 2.5 * f32::from(sf - 7)
}

/// The decimation to twice `bw` from `rate`, if a whole number.
fn decimation(rate: f64, bw: f64) -> Option<usize> {
    let d = rate / (OS as f64 * bw);
    (d >= 1.0 - 1e-9 && (d - d.round()).abs() < 1e-6).then_some(d.round() as usize)
}

/// The channel filter: flat to 0.58 BW (LoRa's ±0.5 BW and an 8 % carrier
/// offset), stopped by 0.75 BW, so little noise folds into the receiver's
/// one sample a chip.
fn channel_filter(rate: f64, bw: f64, decim: usize) -> Option<Stage> {
    if decim == 1 {
        return None; // the host's channel filter is the LoRa band
    }
    let (pass, stop) = (0.58 * bw, 0.75 * bw);
    let taps = (5.5 * rate / (stop - pass)).ceil() as usize;
    Some(Stage::new(lowpass((pass + stop) / 2.0 / rate, taps), decim))
}

struct Mesh {
    band: Band,
    bw: f64,
    filter: Option<Stage>,
    rx: Receiver,
    chan: Vec<Iq>,
    frames: Vec<Frame>,
    generation: u32,
    /// Stream time of the receiver's first sample, s.
    rx_t0: Option<f64>,
    now_s: f64,
    last_frame_s: Option<f64>,
    last: Option<Frame>,
    /// Nodes' short and long names, from their NODEINFO.
    names: HashMap<u32, (String, String)>,
    heard: HeardList,
    recent: VecDeque<(String, String)>,
    count_ok: u64,
    count_crc: u64,
    count_decrypted: u64,
    count_encrypted: u64,
    count_other: u64,
    since_report: f64,
}

impl Mesh {
    fn new(band: Band, bw: f64) -> Self {
        let decim = decimation(band.rate(), bw).expect("a bandwidth the rate divides");
        Mesh {
            band,
            bw,
            filter: channel_filter(band.rate(), bw, decim),
            rx: Receiver::new(bw),
            chan: Vec::new(),
            frames: Vec::new(),
            generation: 0,
            rx_t0: None,
            now_s: 0.0,
            last_frame_s: None,
            last: None,
            names: HashMap::new(),
            heard: HeardList::for_data(),
            recent: VecDeque::new(),
            count_ok: 0,
            count_crc: 0,
            count_decrypted: 0,
            count_encrypted: 0,
            count_other: 0,
            since_report: f64::INFINITY,
        }
    }

    /// A new receiver (a retune, a new bandwidth); the counts carry on.
    fn restart(&mut self) {
        let decim = decimation(self.band.rate(), self.bw).expect("checked");
        self.filter = channel_filter(self.band.rate(), self.bw, decim);
        self.rx = Receiver::new(self.bw);
        self.rx_t0 = None;
    }

    fn node(&self, num: u32) -> String {
        match self.names.get(&num) {
            Some((short, _)) if !short.is_empty() => format!("{} {short}", mesh::node_id(num)),
            _ => mesh::node_id(num),
        }
    }

    fn take(&mut self, f: Frame, out: &mut Sink) {
        let at_s = self.rx_t0.unwrap_or(0.0) + f.at as f64 / (OS as f64 * self.bw);
        let n = f64::from(1u32 << f.sf);
        let airtime = (self.band.preamble() + 4.25 + f.symbols as f64) * n / self.bw;
        let preset = mesh::preset_for(f.sf, self.bw, f.header.cr);
        let modem = format!("SF{} 4/{}", f.sf, 4 + f.header.cr);
        let radio = format!(
            "{modem} · SNR {:.1} dB · {:+.1} kHz",
            f.snr_db,
            f.cfo_hz / 1e3
        );
        self.last_frame_s = Some(at_s);

        let packet = (f.crc_ok != Some(false) && f.sync_word == mesh::SYNC_WORD)
            .then(|| mesh::decode(&f.payload))
            .flatten();
        let Some(p) = packet else {
            // A CRC error, another network's sync word, or too short to be
            // Meshtastic.
            let why = if f.crc_ok == Some(false) {
                self.count_crc += 1;
                "CRC error".to_string()
            } else {
                self.count_other += 1;
                format!("sync word 0x{:02x}, not Meshtastic", f.sync_word)
            };
            out.log(format!(
                "LoRa {modem}, {} bytes: {why} ({radio})",
                f.payload.len()
            ));
            out.packets.push(DataPacket {
                at_s,
                channel: modem.clone(),
                kind: format!("LoRa frame, {why}"),
                crc_ok: false,
                bytes: f.payload.clone(),
                ..Default::default()
            });
            self.last = Some(f);
            return;
        };
        self.count_ok += 1;
        let h = p.header;
        let channel = match (p.channel, p.key) {
            (Some(name), _) => name.to_string(),
            (None, _) if h.channel == 0 && h.to != mesh::BROADCAST => "direct (PKI)".into(),
            _ => format!("ch 0x{:02x}", h.channel),
        };
        let from = self.node(h.from);
        let to = self.node(h.to);
        let (kind, text) = match &p.data {
            Some(d) => {
                self.count_decrypted += 1;
                let s = mesh::summarise(d);
                if let Some((short, long)) = &s.names {
                    self.names.insert(h.from, (short.clone(), long.clone()));
                }
                if let Some(fix) = s.fix {
                    out.locations.push(Location {
                        at_s,
                        channel: channel.clone(),
                        radio: mesh::node_id(h.from),
                        message: "position".into(),
                        lat: Some(fix.lat),
                        lon: Some(fix.lon),
                        altitude_m: fix.altitude_m,
                        note: format!("{} satellites", fix.sats),
                        ..Default::default()
                    });
                    self.heard.position(at_s, &mesh::node_id(h.from));
                }
                (mesh::port_name(d.portnum), s.text)
            }
            None => {
                self.count_encrypted += 1;
                (
                    "encrypted".to_string(),
                    format!("{} bytes", p.payload.len()),
                )
            }
        };
        let key = match p.key {
            Some(Key::None) => " (no key)",
            Some(Key::Short(1)) | None => "",
            Some(Key::Short(_)) => " (short PSK)",
        };
        let hops = format!(
            "hop {}/{}",
            h.hop_start.saturating_sub(h.hop_limit),
            h.hop_start
        );
        out.log(format!(
            "{channel}{key} {from} → {to} {kind}: {text} ({hops}, {radio})"
        ));
        self.recent.push_front((
            format!("{from} → {to}"),
            format!("{}: {text}", short_port(&kind)),
        ));
        self.recent.truncate(RECENT);
        let encrypted = p.data.is_none();
        let node = mesh::node_id(h.from);
        self.heard.talk(
            at_s,
            &channel,
            preset.map_or(modem.as_str(), |p| p.name),
            Some(&node),
            encrypted,
            airtime,
        );
        self.heard.end(&channel);
        out.packets.push(DataPacket {
            at_s,
            channel,
            from,
            to,
            kind: if encrypted {
                kind
            } else {
                format!("{kind}, decrypted")
            },
            crc_ok: true,
            bytes: packet_bytes(&p),
        });
        self.last = Some(f);
    }

    fn report(&mut self, out: &mut Sink) {
        let recent = self
            .last_frame_s
            .is_some_and(|t| self.now_s - t < LOCKED_FOR_S);
        out.lock = if self.rx.busy().is_some() {
            Lock::Synced
        } else if recent {
            Lock::Locked
        } else {
            Lock::Searching
        };
        let st = self.rx.stats();
        let pair = |k: &str, v: String| (k.to_string(), v);
        let (title, headline, gauge) = match &self.last {
            Some(f) => {
                let preset = mesh::preset_for(f.sf, self.bw, f.header.cr);
                out.measure("SNR", f64::from(f.snr_db), "dB");
                (
                    // Short: the card's title bar also holds the frequency.
                    format!("Meshtastic · SF{} · 4/{}", f.sf, 4 + f.header.cr),
                    preset.map_or_else(
                        || format!("SF{} {}", f.sf, bw_label(self.bw)),
                        |p| p.name.to_string(),
                    ),
                    Some(Gauge {
                        label: "SNR, last frame".into(),
                        value_db: f.snr_db,
                        need_db: Some(snr_floor(f.sf)),
                        need_label: format!("SF{}", f.sf),
                        range: (-25.0, 20.0),
                    }),
                )
            }
            None => (
                format!("Meshtastic · {}", bw_label(self.bw)),
                "listening".to_string(),
                None,
            ),
        };
        out.measure("Packets", self.count_ok as f64, "");
        let mut details = vec![
            pair("bandwidth", bw_label(self.bw)),
            pair("SF", "7–12, found".into()),
            pair("packets", self.count_ok.to_string()),
            pair("decrypted", self.count_decrypted.to_string()),
            pair("encrypted", self.count_encrypted.to_string()),
            pair("nodes named", self.names.len().to_string()),
            pair("CRC errors", self.count_crc.to_string()),
            pair("headers bad", st.header_errors.to_string()),
            pair("other LoRa", self.count_other.to_string()),
            pair("preambles", st.preambles.to_string()),
        ];
        if let Some(f) = &self.last {
            details.push(pair(
                "carrier offset",
                format!("{:+.2} kHz", f.cfo_hz / 1e3),
            ));
            details.push(pair("level", format!("{:.1} dBFS", f.level_db)));
        }
        out.card = Some(Card {
            title,
            headline,
            gauge,
            details,
            text: self.recent.iter().cloned().collect(),
        });
        self.heard.report(self.now_s, out);
    }
}

/// "TEXT" for "TEXT_MESSAGE_APP", as the card has room for.
fn short_port(kind: &str) -> &str {
    kind.split('_').next().unwrap_or(kind)
}

/// What the hex dump shows: the header, then the payload (decrypted when
/// a key fitted).
fn packet_bytes(p: &Packet) -> Vec<u8> {
    let mut b = p.header.bytes().to_vec();
    b.extend_from_slice(&p.payload);
    b
}

impl Decoder for Mesh {
    fn process(&mut self, block: &Block<'_>, out: &mut Sink) {
        if block.tuning.generation != self.generation {
            self.generation = block.tuning.generation;
            self.restart();
        }
        let rate = block.tuning.sample_rate;
        self.rx_t0.get_or_insert(block.first_sample as f64 / rate);
        self.rx.set_carrier(block.tuning.center_hz);
        match &mut self.filter {
            Some(f) => {
                self.chan.clear();
                f.process(block.iq, &mut self.chan);
                self.rx.process(&self.chan, &mut self.frames);
            }
            None => self.rx.process(block.iq, &mut self.frames),
        }
        self.now_s = (block.first_sample + block.iq.len() as u64) as f64 / rate;
        let got = !self.frames.is_empty();
        // Rust note: `take` empties the vector into a local, so `self.take`
        // can borrow `self` while going through them.
        for f in std::mem::take(&mut self.frames) {
            self.take(f, out);
        }
        self.since_report += block.iq.len() as f64 / rate;
        if got || self.since_report >= 0.25 {
            self.since_report = 0.0;
            self.report(out);
        }
    }

    fn command(&mut self, cmd: Command, out: &mut Sink) {
        if let Command::Set { key, value } = cmd {
            match key.as_str() {
                "bandwidth" => match parse_bw(&value).filter(|&bw| decimation(self.band.rate(), bw).is_some()) {
                    Some(bw) if bw != self.bw => {
                        self.bw = bw;
                        self.restart();
                        self.last = None;
                    }
                    Some(_) => {}
                    None => out.log(format!(
                        "meshtastic: bandwidth {value:?} does not divide {} into twice itself; one of {}",
                        bw_label(self.band.rate()),
                        self.band.bandwidths().iter().map(|&b| bw_label(b)).collect::<Vec<_>>().join(", ")
                    )),
                },
                k => out.log(format!("meshtastic: no setting {k:?}")),
            }
        }
    }

    fn reset(&mut self) {
        self.restart();
    }
}

/// The settings panel (see `declab_api::Panel`).
#[cfg(feature = "ui")]
mod panel {
    use declab_api::{Command, Panel};
    use egui::{Color32, RichText, Ui};

    use super::{Band, bw_label, parse_bw};

    pub struct MeshPanel {
        pub band: Band,
        /// What has been sent, so the control shows it.
        pub bw: f64,
    }

    impl Panel for MeshPanel {
        fn settings(&self) -> Vec<Command> {
            vec![Command::Set {
                key: "bandwidth".into(),
                value: format!("{}", self.bw),
            }]
        }

        fn restore(&mut self, key: &str, value: &str) {
            if key == "bandwidth"
                && let Some(bw) = parse_bw(value).filter(|b| self.band.bandwidths().contains(b))
            {
                self.bw = bw;
            }
        }

        fn ui(&mut self, ui: &mut Ui, commands: &mut Vec<Command>) {
            egui::Grid::new("meshtastic_panel")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    ui.label("LoRa bandwidth");
                    egui::ComboBox::from_id_salt("meshtastic_bw")
                        .selected_text(bw_label(self.bw))
                        .show_ui(ui, |ui| {
                            for &bw in self.band.bandwidths() {
                                if ui.selectable_label(self.bw == bw, bw_label(bw)).clicked()
                                    && self.bw != bw
                                {
                                    self.bw = bw;
                                    commands.push(Command::Set {
                                        key: "bandwidth".into(),
                                        value: format!("{bw}"),
                                    });
                                }
                            }
                        });
                    ui.end_row();
                });
            ui.label(
                RichText::new(
                    "The spreading factor (7–12) is found from the signal and the coding rate read from each \
                     frame. Channels on the default key (AQ==) or its short-PSK siblings are decrypted.",
                )
                .size(11.0)
                .color(Color32::GRAY),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use declab_api::Tuning;
    use declab_lora::Message;
    use declab_lora::meshtastic::{DEFAULT_PSK, Header, PRESETS, channel_hash};
    use declab_lora::protobufs::{self, PortNum};
    use declab_lora::tx::{self, Noise, TxConfig};

    fn packet(
        from: u32,
        id: u32,
        port: PortNum,
        payload: Vec<u8>,
        key: &[u8],
        chan: &str,
    ) -> Vec<u8> {
        let h = Header {
            to: mesh::BROADCAST,
            from,
            id,
            hop_limit: 3,
            hop_start: 3,
            channel: channel_hash(chan, key),
            ..Header::default()
        };
        let d = protobufs::Data {
            portnum: port as i32,
            payload,
            ..Default::default()
        };
        mesh::encode(&h, &d, key)
    }

    /// LongFast traffic, a MediumFast packet on the same channel (another
    /// SF), and one on a key the decoder does not know, at 1 MS/s with an
    /// offset and noise, through the plug-in.
    fn traffic(rate: f64, snr_db: f64) -> Vec<Iq> {
        let user = protobufs::User {
            id: "!a1b2c3d4".into(),
            long_name: "DecLAB test node".into(),
            short_name: "DLAB".into(),
            ..Default::default()
        };
        let pos = protobufs::Position {
            latitude_i: Some(515_000_000),
            longitude_i: Some(-1_000_000),
            altitude: Some(42),
            ..Default::default()
        };
        let frames: Vec<(TxConfig, Vec<u8>)> = vec![
            (
                TxConfig::meshtastic(11, 250e3, 1),
                packet(
                    0xa1b2_c3d4,
                    1,
                    PortNum::NodeinfoApp,
                    user.encode_to_vec(),
                    &DEFAULT_PSK,
                    "LongFast",
                ),
            ),
            (
                TxConfig::meshtastic(11, 250e3, 1),
                packet(
                    0xa1b2_c3d4,
                    2,
                    PortNum::TextMessageApp,
                    b"hello from DecLAB".to_vec(),
                    &DEFAULT_PSK,
                    "LongFast",
                ),
            ),
            (
                TxConfig::meshtastic(9, 250e3, 1),
                packet(
                    0x0bad_cafe,
                    3,
                    PortNum::PositionApp,
                    pos.encode_to_vec(),
                    &DEFAULT_PSK,
                    "MediumFast",
                ),
            ),
            (
                TxConfig::meshtastic(11, 250e3, 1),
                packet(
                    0x1234_5678,
                    4,
                    PortNum::TextMessageApp,
                    b"secret".to_vec(),
                    &[9u8; 16],
                    "Private",
                ),
            ),
        ];
        let mut x = vec![Iq::new(0.0, 0.0); 30_011];
        for (cfg, p) in &frames {
            x.extend(tx::modulate(p, cfg, rate, 4_200.0, 0.0));
            x.extend(vec![Iq::new(0.0, 0.0); 200_000]);
        }
        Noise::new(7).add(&mut x, snr_db, 250e3, rate);
        x
    }

    fn run(x: &[Iq], rate: f64) -> (Vec<String>, Vec<DataPacket>, Vec<Location>) {
        let mut d = Factory(Band::SubGhz).create();
        let (mut log, mut packets, mut locs) = (Vec::new(), Vec::new(), Vec::new());
        for (k, chunk) in x.chunks(65_536).enumerate() {
            let block = Block {
                iq: chunk,
                tuning: Tuning {
                    center_hz: 0.0,
                    sample_rate: rate,
                    generation: 0,
                },
                first_sample: (k * 65_536) as u64,
                dropped_before: 0,
                live: false,
            };
            let mut sink = Sink::default();
            d.process(&block, &mut sink);
            log.append(&mut sink.log);
            packets.append(&mut sink.packets);
            locs.append(&mut sink.locations);
        }
        (log, packets, locs)
    }

    #[test]
    fn decodes_and_decrypts_longfast_and_another_sf() {
        let x = traffic(1e6, 0.0);
        let (log, packets, locs) = run(&x, 1e6);
        assert_eq!(packets.len(), 4, "{log:#?}");
        assert!(
            log.iter().any(|l| l
                .contains("LongFast !a1b2c3d4 DLAB → ^all TEXT_MESSAGE_APP: hello from DecLAB")),
            "{log:#?}"
        );
        assert!(
            log.iter()
                .any(|l| l.starts_with("LongFast !a1b2c3d4 → ^all NODEINFO_APP: DLAB")),
            "{log:#?}"
        );
        assert!(
            log.iter()
                .any(|l| l.starts_with("MediumFast !0badcafe → ^all POSITION_APP")),
            "{log:#?}"
        );
        assert!(log.iter().any(|l| l.contains("encrypted: ")), "{log:#?}");
        assert_eq!(locs.len(), 1);
        assert!(
            (locs[0].lat.unwrap() - 51.5).abs() < 1e-6 && (locs[0].lon.unwrap() + 0.1).abs() < 1e-6
        );
        assert!(PRESETS.iter().any(|p| p.name == "LongFast"));
    }

    #[test]
    fn the_bandwidth_is_a_setting() {
        let mut d = Factory(Band::SubGhz).create();
        let mut sink = Sink::default();
        d.command(
            Command::Set {
                key: "bandwidth".into(),
                value: "125k".into(),
            },
            &mut sink,
        );
        assert!(sink.log.is_empty(), "{:?}", sink.log);
        d.command(
            Command::Set {
                key: "bandwidth".into(),
                value: "300k".into(),
            },
            &mut sink,
        );
        assert_eq!(sink.log.len(), 1);
        assert_eq!(parse_bw("812.5k"), Some(812_500.0));
        assert_eq!(decimation(3.25e6, 812_500.0), Some(2));
        assert_eq!(decimation(1e6, 15_625.0), Some(32));
    }

    /// Packets decoded against SNR (in the LoRa bandwidth) for each SF at
    /// 250 kHz through the whole plug-in, and the CPU it takes per second
    /// of signal. `cargo test -p declab-decoders --release mesh_sweep -- --ignored --nocapture`
    #[test]
    #[ignore = "a sweep, minutes"]
    fn mesh_sweep() {
        let rate = 1e6;
        let trials = 20;
        for sf in 7..=12u8 {
            let cfg = TxConfig::meshtastic(sf, 250e3, 1);
            let mut line = format!("SF{sf:2} (floor {:5.1} dB):", snr_floor(sf));
            let mut cpu = 0.0;
            let mut secs = 0.0;
            for step in 0..8 {
                let snr = f64::from(snr_floor(sf)) + 6.0 - 1.5 * f64::from(step);
                let mut ok = 0;
                for t in 0..trials {
                    let p = packet(
                        0xa1b2_c3d4,
                        t,
                        PortNum::TextMessageApp,
                        vec![b'x'; 24],
                        &DEFAULT_PSK,
                        "LongFast",
                    );
                    let mut x = vec![Iq::new(0.0, 0.0); 20_000 + t as usize * 37];
                    x.extend(tx::modulate(&p, &cfg, rate, 3_000.0, 0.0));
                    x.extend(vec![Iq::new(0.0, 0.0); 20_000 + (OS << sf) * 4]);
                    Noise::new(u64::from(t) + 100 * u64::from(sf)).add(&mut x, snr, 250e3, rate);
                    let t0 = std::time::Instant::now();
                    let (_, packets, _) = run(&x, rate);
                    cpu += t0.elapsed().as_secs_f64();
                    secs += x.len() as f64 / rate;
                    ok += packets
                        .iter()
                        .filter(|p| p.crc_ok && p.kind.contains("decrypted"))
                        .count();
                }
                line += &format!(" {snr:6.1} dB {ok:2}/{trials}");
            }
            println!("{line}  · CPU {:.1} % of a core", 100.0 * cpu / secs);
        }
    }
}
