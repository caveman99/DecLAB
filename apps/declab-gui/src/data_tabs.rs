// made by CasualArclamp
//! Digital voice's and the data decoders' tabs, over the right-hand box:
//! Heard (talkgroups and radios, or channels and nodes), Hex (every data
//! message, hex with ASCII beside it), LRRP or Positions (location messages
//! as a table) and IP (the datagrams, PCAP recording, one picked apart).
//!
//! The hex dump can run to thousands of lines: it is laid out once when a
//! message arrives and drawn with `show_rows`, which lays out only the rows
//! on screen.

use declab_api::DataPacket;
use declab_engine::DecoderView;
use eframe::egui::{self, Color32, FontId, RichText, Ui, vec2};

use crate::panels::{AMBER, GREEN, RED};

const GREY: Color32 = Color32::from_gray(154);
const TEXT: Color32 = Color32::from_gray(225);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Heard,
    Hex,
    Lrrp,
    Ip,
}

#[derive(Default)]
pub struct DataTabs {
    pub tab: Tab,
    /// The hex dump's lines, and what they were made from (count of
    /// messages ever, the newest one's time).
    hex: Vec<(Color32, String)>,
    hex_for: (u64, u64),
    /// The datagram picked in the IP tab (its number); None follows the
    /// newest.
    ip_pick: Option<u64>,
    /// Digital voice (DMR, TETRA…) rather than a data decoder: the tabs'
    /// names and hints.
    voice: bool,
}

/// `bytes` as hex-dump rows: offset, 16 bytes in two groups of 8, ASCII
/// (dots for anything unprintable).
pub fn hex_rows(bytes: &[u8]) -> impl Iterator<Item = String> + '_ {
    bytes.chunks(16).enumerate().map(|(i, row)| {
        let mut s = format!("{:04x}  ", i * 16);
        for k in 0..16 {
            match row.get(k) {
                Some(b) => s += &format!("{b:02x} "),
                None => s += "   ",
            }
            if k == 7 {
                s.push(' ');
            }
        }
        s.push(' ');
        // Rust note: `char::from(u8)` is Latin-1, so only 0x20..0x7e are
        // printed as themselves.
        s.extend(row.iter().map(|&b| {
            if (0x20..0x7f).contains(&b) {
                char::from(b)
            } else {
                '.'
            }
        }));
        s
    })
}

fn packet_head(p: &DataPacket) -> String {
    format!(
        "{:8.1} s  {}  {} → {}  {}  {} bytes{}",
        p.at_s,
        p.channel,
        p.from,
        p.to,
        p.kind,
        p.bytes.len(),
        if p.crc_ok { "" } else { "  CRC failed" }
    )
}

/// UDP/TCP ports the radios' data services use, and a few common ones.
fn port_name(port: u16) -> Option<&'static str> {
    Some(match port {
        // MOTOTRBO's: location, registration, text messages, telemetry.
        4001 => "LRRP",
        4005 => "ARS",
        4007 => "TMS",
        4008 => "telemetry",
        53 => "DNS",
        67 | 68 => "DHCP",
        123 => "NTP",
        161 | 162 => "SNMP",
        _ => return None,
    })
}

fn proto_name(p: u8) -> String {
    match p {
        1 => "ICMP".into(),
        6 => "TCP".into(),
        17 => "UDP".into(),
        _ => format!("proto {p}"),
    }
}

/// An IPv4 datagram's outline: source, destination, protocol, ports (for
/// UDP and TCP), the payload's offset.
struct Ip4 {
    src: String,
    dst: String,
    proto: u8,
    ports: Option<(u16, u16)>,
    header_len: usize,
}

fn ipv4(d: &[u8]) -> Option<Ip4> {
    if d.len() < 20 || d[0] >> 4 != 4 {
        return None;
    }
    let ihl = usize::from(d[0] & 15) * 4;
    let a = |b: &[u8]| format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]);
    let ports = (matches!(d[9], 6 | 17) && d.len() >= ihl + 4).then(|| {
        (
            u16::from_be_bytes([d[ihl], d[ihl + 1]]),
            u16::from_be_bytes([d[ihl + 2], d[ihl + 3]]),
        )
    });
    Some(Ip4 {
        src: a(&d[12..16]),
        dst: a(&d[16..20]),
        proto: d[9],
        ports,
        header_len: ihl,
    })
}

fn checksum(h: &[u8]) -> u16 {
    let mut sum: u32 = h
        .chunks(2)
        .map(|w| u32::from(w[0]) << 8 | u32::from(*w.get(1).unwrap_or(&0)))
        .sum();
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

impl DataTabs {
    pub fn opening_on(tab: Tab) -> Self {
        DataTabs {
            tab,
            ..Default::default()
        }
    }

    /// The tab bar, with counts. Returns the tab to draw.
    pub fn bar(&mut self, ui: &mut Ui, view: &DecoderView, voice: bool) -> Tab {
        self.voice = voice;
        ui.horizontal(|ui| {
            let fixes = view.locations.iter().filter(|l| l.lat.is_some()).count();
            for (t, name) in [
                (Tab::Heard, "Heard".to_string()),
                (Tab::Hex, format!("Hex {}", view.packets_total)),
                (
                    Tab::Lrrp,
                    format!(
                        "{} {}",
                        if voice { "LRRP" } else { "Positions" },
                        if view.locations_total > 0 {
                            fixes.to_string()
                        } else {
                            "0".into()
                        }
                    ),
                ),
                (Tab::Ip, format!("IP {}", view.ip_packets)),
            ] {
                let text = RichText::new(name).color(if self.tab == t { AMBER } else { GREY });
                if ui.selectable_label(self.tab == t, text).clicked() {
                    self.tab = t;
                }
            }
        });
        ui.separator();
        self.tab
    }

    /// Every data message kept, oldest first, each a heading and its hex.
    pub fn hex(&mut self, ui: &mut Ui, view: &DecoderView) {
        if view.packets.is_empty() {
            ui.label(RichText::new("no data messages yet").color(GREY));
            ui.label(
                RichText::new(if self.voice {
                    "DMR packet data (LRRP, text, IP) and TETRA SDS appear here, in hex and ASCII."
                } else {
                    "Each message appears here, in hex and ASCII."
                })
                .color(GREY)
                .size(12.0),
            );
            return;
        }
        let key = (
            view.packets_total,
            view.packets.back().map_or(0, |p| p.at_s.to_bits()),
        );
        if key != self.hex_for {
            self.hex_for = key;
            self.hex.clear();
            for p in &view.packets {
                self.hex
                    .push((if p.crc_ok { AMBER } else { RED }, packet_head(p)));
                self.hex.extend(hex_rows(&p.bytes).map(|r| (TEXT, r)));
                self.hex.push((TEXT, String::new()));
            }
        }
        let font = FontId::monospace(12.0);
        let row_h = ui.fonts_mut(|f| f.row_height(&font));
        egui::ScrollArea::both()
            .id_salt("hex_dump")
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show_rows(ui, row_h, self.hex.len(), |ui, rows| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (c, line) in &self.hex[rows] {
                    ui.add(
                        egui::Label::new(RichText::new(line).font(font.clone()).color(*c)).extend(),
                    );
                }
            });
    }

    /// The location messages, newest first. A click on a fix copies it.
    pub fn lrrp(&mut self, ui: &mut Ui, view: &DecoderView) {
        let fixes: Vec<_> = view.locations.iter().filter(|l| l.lat.is_some()).collect();
        let mut radios: Vec<&str> = fixes.iter().map(|l| l.radio.as_str()).collect();
        radios.sort_unstable();
        radios.dedup();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Locations").color(AMBER));
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "{} messages · {} fixes from {} radios",
                    view.locations_total,
                    fixes.len(),
                    radios.len()
                ))
                .color(GREY)
                .size(12.0),
            );
        });
        if view.locations.is_empty() {
            ui.label(
                RichText::new(if self.voice {
                    "no LRRP yet (Motorola location, UDP port 4001)"
                } else {
                    "no positions yet"
                })
                .color(GREY),
            );
            return;
        }
        let opt = |v: Option<f64>, f: &dyn Fn(f64) -> String| v.map_or("–".to_string(), f);
        egui::ScrollArea::both()
            .id_salt("lrrp")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("lrrp_table")
                    .striped(true)
                    .spacing(vec2(12.0, 2.0))
                    .show(ui, |ui| {
                        for h in [
                            "time", "channel", "radio", "message", "lat", "lon", "alt", "±",
                            "speed", "heading", "fix at", "",
                        ] {
                            ui.label(RichText::new(h).color(GREY));
                        }
                        ui.end_row();
                        for l in view.locations.iter().rev() {
                            let fix = l.lat.is_some();
                            let c = if fix { Color32::from_gray(230) } else { GREY };
                            let cell = |ui: &mut Ui, s: String| ui.label(RichText::new(s).color(c));
                            cell(ui, format!("{:.1} s", l.at_s));
                            cell(ui, l.channel.clone());
                            ui.label(RichText::new(&l.radio).color(if fix { GREEN } else { GREY }));
                            cell(ui, l.message.clone());
                            if let (Some(lat), Some(lon)) = (l.lat, l.lon) {
                                let pos = format!("{lat:.5}, {lon:.5}");
                                for v in [lat, lon] {
                                    let r = ui
                                        .add(
                                            egui::Label::new(
                                                RichText::new(format!("{v:.5}")).color(c),
                                            )
                                            .sense(egui::Sense::click()),
                                        )
                                        .on_hover_text("click to copy the position");
                                    if r.clicked() {
                                        ui.ctx().copy_text(pos.clone());
                                    }
                                }
                            } else {
                                cell(ui, "–".into());
                                cell(ui, "–".into());
                            }
                            cell(ui, opt(l.altitude_m, &|a| format!("{a:.0} m")));
                            cell(ui, opt(l.radius_m, &|r| format!("{r:.0} m")));
                            cell(ui, opt(l.speed, &|s| format!("{s:.1}")));
                            cell(ui, opt(l.heading_deg, &|h| format!("{h:.0}°")));
                            cell(ui, l.time.clone().unwrap_or_else(|| "–".into()));
                            cell(ui, l.note.clone());
                            ui.end_row();
                        }
                    });
            });
    }

    /// The datagrams, newest first; the picked one's headers and payload.
    /// (The count and the PCAP recording are drawn above, by the caller.)
    pub fn ip(&mut self, ui: &mut Ui, view: &DecoderView) {
        if view.datagrams.is_empty() {
            ui.label(RichText::new("no IP datagrams yet").color(GREY));
            return;
        }
        // A pick that has scrolled out of the list goes back to following.
        if self
            .ip_pick
            .is_some_and(|n| !view.datagrams.iter().any(|(k, _)| *k == n))
        {
            self.ip_pick = None;
        }
        let newest = view.datagrams.back().map(|(n, _)| *n);
        let shown = self.ip_pick.or(newest);
        let list_h = (ui.available_height() * 0.45).max(60.0);
        egui::ScrollArea::vertical()
            .id_salt("ip_list")
            .max_height(list_h)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("ip_table")
                    .striped(true)
                    .spacing(vec2(12.0, 2.0))
                    .show(ui, |ui| {
                        for h in ["#", "source", "destination", "protocol", "bytes", "service"] {
                            ui.label(RichText::new(h).color(GREY));
                        }
                        ui.end_row();
                        for (n, d) in view.datagrams.iter().rev() {
                            let picked = shown == Some(*n);
                            let tip = if Some(*n) == newest {
                                "the newest: followed"
                            } else {
                                "show this one"
                            };
                            if ui
                                .selectable_label(picked, n.to_string())
                                .on_hover_text(tip)
                                .clicked()
                            {
                                // Clicking the newest goes back to following.
                                self.ip_pick = (Some(*n) != newest).then_some(*n);
                            }
                            match ipv4(d) {
                                Some(h) => {
                                    let port = |a: &str, p: Option<u16>| {
                                        p.map_or(a.to_string(), |p| format!("{a}:{p}"))
                                    };
                                    ui.label(port(&h.src, h.ports.map(|p| p.0)));
                                    ui.label(port(&h.dst, h.ports.map(|p| p.1)));
                                    ui.label(proto_name(h.proto));
                                    ui.label(d.len().to_string());
                                    let svc =
                                        h.ports.and_then(|(s, t)| port_name(t).or(port_name(s)));
                                    ui.label(RichText::new(svc.unwrap_or("")).color(GREEN));
                                }
                                None => {
                                    ui.label(
                                        RichText::new(if d.first().is_some_and(|b| b >> 4 == 6) {
                                            "IPv6"
                                        } else {
                                            "?"
                                        })
                                        .color(GREY),
                                    );
                                    ui.label("");
                                    ui.label("");
                                    ui.label(d.len().to_string());
                                    ui.label("");
                                }
                            }
                            ui.end_row();
                        }
                    });
            });
        ui.separator();
        let Some((n, d)) = view.datagrams.iter().find(|(k, _)| Some(*k) == shown) else {
            return;
        };
        let font = FontId::monospace(12.0);
        egui::ScrollArea::both()
            .id_salt("ip_detail")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                let line = |ui: &mut Ui, s: String, c: Color32| {
                    ui.add(egui::Label::new(RichText::new(s).font(font.clone()).color(c)).extend());
                };
                line(ui, format!("datagram {n}, {} bytes", d.len()), AMBER);
                if let Some(h) = ipv4(d) {
                    let ok = checksum(&d[..h.header_len.min(d.len())]) == 0;
                    line(
                        ui,
                        format!(
                            "IPv4 {} → {}  {}  TTL {}  ID {:#06x}  header {} bytes, checksum {}",
                            h.src,
                            h.dst,
                            proto_name(h.proto),
                            d[8],
                            u16::from_be_bytes([d[4], d[5]]),
                            h.header_len,
                            if ok { "ok" } else { "BAD" }
                        ),
                        if ok { TEXT } else { RED },
                    );
                    let mut payload = h.header_len;
                    if h.proto == 17 && d.len() >= h.header_len + 8 {
                        let u = &d[h.header_len..];
                        let (s, t) = h.ports.unwrap_or_default();
                        let svc = port_name(t)
                            .or(port_name(s))
                            .map_or(String::new(), |n| format!("  ({n})"));
                        line(
                            ui,
                            format!(
                                "UDP {s} → {t}  length {}  checksum {:#06x}{svc}",
                                u16::from_be_bytes([u[4], u[5]]),
                                u16::from_be_bytes([u[6], u[7]])
                            ),
                            TEXT,
                        );
                        payload += 8;
                    }
                    line(
                        ui,
                        format!("payload, {} bytes:", d.len().saturating_sub(payload)),
                        GREY,
                    );
                    for r in hex_rows(&d[payload.min(d.len())..]) {
                        line(ui, r, TEXT);
                    }
                } else {
                    for r in hex_rows(d) {
                        line(ui, r, TEXT);
                    }
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hex_row_has_offset_hex_and_ascii() {
        let rows: Vec<String> = hex_rows(b"Hello, DMR!\x00\x01\x7f\xffXYZ").collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            "0000  48 65 6c 6c 6f 2c 20 44  4d 52 21 00 01 7f ff 58  Hello, DMR!....X"
        );
        assert_eq!(rows[1], format!("0010  59 5a {}  YZ", " ".repeat(3 * 14)));
    }
}
