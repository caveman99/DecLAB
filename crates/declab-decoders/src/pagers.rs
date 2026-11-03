// made by CasualArclamp
//! Pagers: POCSAG at 512/1200/2400 bit/s and FLEX at 1600/2, 3200/2, 3200/4
//! and 6400/4, all listened for at once on one channel. The protocols are
//! DecPagers' (`crates/declab-pagers`); this file wraps them.
//!
//! Scopes: "Symbols" is the soft symbols of the decoder with sync (FLEX
//! first), oldest at the left: two bands for 2-level, four for 4-level.
//! "Eye" is the matched-filter output across two symbols. Every page goes
//! to the log; the latest are on the card.

use std::collections::VecDeque;

use declab_api::{
    Block, Card, Command, Decoder, DecoderFactory, DecoderInfo, Family, InputSpec, Iq, Lock, Sink,
};
use declab_pagers::pocsag::Charset;
use declab_pagers::{Page, Protocol, Receiver, ReceiverConfig};

/// The channel rate: ≥ 15 samples a symbol at 3200 baud, and a 20 kHz
/// channel (±4.8 kHz deviation at 3200 baud is ~16 kHz by Carson) fits.
const RATE: f64 = 50_000.0;
/// Pages kept on the card.
const RECENT: usize = 10;
/// Points sent to each scope a report.
const SYMBOL_POINTS: usize = 600;
const EYE_POINTS: usize = 3000;
/// The card says "locked" while pages came this recently.
const LOCKED_FOR_S: f64 = 10.0;

pub struct Factory;

impl DecoderFactory for Factory {
    fn info(&self) -> DecoderInfo {
        DecoderInfo {
            id: "pagers",
            name: "Pagers",
            family: Family::Data,
            standard: "ITU-R M.584-2 (POCSAG), FLEX",
            summary: "POCSAG 512/1200/2400 and FLEX 1600–6400, every message type",
        }
    }

    fn input(&self, _source_rate: f64) -> InputSpec {
        InputSpec {
            sample_rate: Some(RATE),
            bandwidth_hz: 20_000.0,
            block_len: 5_000,
        }
    }

    fn create(&self) -> Box<dyn Decoder> {
        Box::new(Pagers::new(Settings::default()))
    }

    fn default_scope(&self) -> &'static str {
        "Symbols"
    }

    #[cfg(feature = "ui")]
    fn panel(&self) -> Option<Box<dyn declab_api::Panel>> {
        Some(Box::new(panel::PagersPanel {
            s: Settings::default(),
        }))
    }
}

/// What the panel can change, as [`Command::Set`] keys and values.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Settings {
    /// POCSAG 512, 1200, 2400.
    pocsag: [bool; 3],
    flex: bool,
    /// FLEX date/time and system words as log lines.
    system: bool,
    charset: Charset,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            pocsag: [true; 3],
            flex: true,
            system: false,
            charset: Charset::Auto,
        }
    }
}

const POCSAG_KEYS: [(&str, u32); 3] = [
    ("pocsag512", 512),
    ("pocsag1200", 1200),
    ("pocsag2400", 2400),
];

fn charset_name(c: Charset) -> &'static str {
    match c {
        Charset::Auto => "auto",
        Charset::Alpha => "alpha",
        Charset::Numeric => "numeric",
    }
}

impl Settings {
    fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let on = || match value {
            "on" => Ok(true),
            "off" => Ok(false),
            v => Err(format!("{key}: {v:?} is not on or off")),
        };
        if let Some(i) = POCSAG_KEYS.iter().position(|k| k.0 == key) {
            self.pocsag[i] = on()?;
            return Ok(());
        }
        match key {
            "flex" => self.flex = on()?,
            "system" => self.system = on()?,
            "charset" => {
                self.charset = match value {
                    "auto" => Charset::Auto,
                    "alpha" => Charset::Alpha,
                    "numeric" => Charset::Numeric,
                    v => return Err(format!("charset: {v:?} is not auto, alpha or numeric")),
                }
            }
            k => return Err(format!("pagers: no setting {k:?}")),
        }
        Ok(())
    }

    fn config(&self) -> ReceiverConfig {
        ReceiverConfig {
            pocsag: POCSAG_KEYS
                .iter()
                .zip(self.pocsag)
                .filter(|(_, on)| *on)
                .map(|(k, _)| k.1)
                .collect(),
            flex: self.flex,
            system_words: self.system,
            charset: self.charset,
        }
    }
}

struct Pagers {
    settings: Settings,
    rx: Receiver,
    pages: Vec<Page>,
    /// The latest pages, newest first, as card lines.
    recent: VecDeque<(String, String)>,
    count_pocsag: u64,
    count_flex: u64,
    fixed_bits: u64,
    bad_words: u64,
    /// Stream time now and at the last page, seconds.
    now_s: f64,
    last_page_s: Option<f64>,
    since_report: usize,
    /// The scopes' rolling windows: the latest soft symbols and eye points.
    sym_hist: VecDeque<f32>,
    eye_hist: VecDeque<(f32, f32)>,
}

impl Pagers {
    fn new(settings: Settings) -> Self {
        let mut rx = Receiver::new(RATE, &settings.config());
        rx.enable_tap(true);
        Pagers {
            settings,
            rx,
            pages: Vec::new(),
            recent: VecDeque::new(),
            count_pocsag: 0,
            count_flex: 0,
            fixed_bits: 0,
            bad_words: 0,
            now_s: 0.0,
            last_page_s: None,
            since_report: 0,
            sym_hist: VecDeque::with_capacity(SYMBOL_POINTS + 512),
            eye_hist: VecDeque::with_capacity(EYE_POINTS + 8192),
        }
    }

    fn take_pages(&mut self, out: &mut Sink) {
        for p in self.pages.drain(..) {
            match p.protocol {
                Protocol::Pocsag => self.count_pocsag += 1,
                Protocol::Flex => self.count_flex += 1,
            }
            self.fixed_bits += u64::from(p.corrected_bits);
            self.bad_words += u64::from(p.bad_words);
            self.last_page_s = Some(self.now_s);
            let label = format!("{} {}", p.mode(), p.capcode);
            let mut value = p.kind.tag().to_string();
            if !p.text.is_empty() {
                value = format!("{value}  {}", p.text);
            }
            self.recent.push_front((label, value));
            self.recent.truncate(RECENT);
            // The page's own time is the receiver's; the log has its own.
            let line = p.to_string();
            out.log(
                line.trim_start()
                    .split_once(' ')
                    .map_or(line.as_str(), |(_, rest)| rest)
                    .to_string(),
            );
        }
    }

    fn report(&mut self, block: &Block<'_>, out: &mut Sink) {
        let synced = self.rx.synced();
        out.lock = match (&synced, self.last_page_s) {
            (Some(_), Some(t)) if self.now_s - t < LOCKED_FOR_S => Lock::Locked,
            (Some(_), _) => Lock::Synced,
            _ => Lock::Searching,
        };

        if let Some(tap) = self.rx.tap_mut() {
            // Rust note: `drain(..)` empties the tap's vectors as it moves
            // their points across, keeping their capacity for next time.
            self.sym_hist.extend(tap.symbols.drain(..));
            self.eye_hist.extend(tap.eye.drain(..));
        }
        let over = self.sym_hist.len().saturating_sub(SYMBOL_POINTS);
        self.sym_hist.drain(..over);
        let over = self.eye_hist.len().saturating_sub(EYE_POINTS);
        self.eye_hist.drain(..over);
        // Symbols in time order across the plot, level up the side.
        let n = self.sym_hist.len().max(2) as f32 - 1.0;
        let pts: Vec<Iq> = self
            .sym_hist
            .iter()
            .enumerate()
            .map(|(i, &v)| Iq::new(2.0 * i as f32 / n - 1.0, v.clamp(-1.5, 1.5)))
            .collect();
        out.scopes.push(("Symbols", pts));
        let pts: Vec<Iq> = self
            .eye_hist
            .iter()
            .map(|&(x, y)| Iq::new(x - 1.0, y.clamp(-1.5, 1.5)))
            .collect();
        out.scopes.push(("Eye", pts));

        // The channel IQ, scaled to the carrier.
        let iq = block.iq;
        if !iq.is_empty() {
            let power = iq.iter().map(|s| s.norm_sqr()).sum::<f32>() / iq.len() as f32;
            let k = 0.5 / power.max(1e-12).sqrt();
            out.constellation
                .extend(iq.iter().step_by((iq.len() / 256).max(1)).map(|s| s * k));
        }

        let (frames, words, bad) = self.rx.stats();
        out.measure("Pages", (self.count_pocsag + self.count_flex) as f64, "");
        out.measure("FLEX frames", frames as f64, "");
        let modes: Vec<String> = POCSAG_KEYS
            .iter()
            .zip(self.settings.pocsag)
            .filter(|(_, on)| *on)
            .map(|(k, _)| k.1.to_string())
            .collect();
        let listening = match (modes.is_empty(), self.settings.flex) {
            (false, true) => format!("POCSAG {} · FLEX", modes.join("/")),
            (false, false) => format!("POCSAG {}", modes.join("/")),
            (true, true) => "FLEX".to_string(),
            (true, false) => "nothing".to_string(),
        };
        let pair = |k: &str, v: String| (k.to_string(), v);
        out.card = Some(Card {
            title: format!("Pagers · {listening}"),
            headline: synced.unwrap_or_else(|| "searching".into()),
            gauge: None,
            details: vec![
                pair("POCSAG pages", self.count_pocsag.to_string()),
                pair("FLEX pages", self.count_flex.to_string()),
                pair("FLEX frames", frames.to_string()),
                pair("POCSAG words", format!("{words} ({bad} bad)")),
                pair("BCH bits fixed", self.fixed_bits.to_string()),
                pair("bad words in pages", self.bad_words.to_string()),
            ],
            text: self.recent.iter().cloned().collect(),
        });
    }
}

impl Decoder for Pagers {
    fn process(&mut self, block: &Block<'_>, out: &mut Sink) {
        self.rx.process_iq(block.iq, &mut self.pages);
        self.now_s = (block.first_sample + block.iq.len() as u64) as f64 / block.tuning.sample_rate;
        self.take_pages(out);
        self.since_report += block.iq.len();
        if self.since_report as f64 >= block.tuning.sample_rate / 10.0 {
            self.since_report = 0;
            self.report(block, out);
        }
    }

    fn command(&mut self, cmd: Command, out: &mut Sink) {
        if let Command::Set { key, value } = cmd {
            let mut s = self.settings;
            match s.set(&key, &value) {
                Ok(()) if s != self.settings => {
                    // A new receiver with the new modes; the counts carry on.
                    self.settings = s;
                    let mut rx = Receiver::new(RATE, &s.config());
                    rx.enable_tap(true);
                    self.rx = rx;
                    self.sym_hist.clear();
                    self.eye_hist.clear();
                }
                Ok(()) => {}
                Err(e) => out.log(e),
            }
        }
    }

    fn reset(&mut self) {
        let mut rx = Receiver::new(RATE, &self.settings.config());
        rx.enable_tap(true);
        self.rx = rx;
        self.sym_hist.clear();
        self.eye_hist.clear();
    }
}

/// The settings panel (see `declab_api::Panel`).
#[cfg(feature = "ui")]
mod panel {
    use declab_api::{Command, Panel};
    use declab_pagers::pocsag::Charset;
    use egui::Ui;

    use super::{POCSAG_KEYS, Settings, charset_name};

    pub struct PagersPanel {
        /// What has been sent, so the controls show it.
        pub s: Settings,
    }

    fn onoff(b: bool) -> String {
        if b { "on" } else { "off" }.into()
    }

    impl Panel for PagersPanel {
        fn settings(&self) -> Vec<Command> {
            let mut v: Vec<Command> = POCSAG_KEYS
                .iter()
                .zip(self.s.pocsag)
                .map(|(k, on)| Command::Set {
                    key: k.0.into(),
                    value: onoff(on),
                })
                .collect();
            v.push(Command::Set {
                key: "flex".into(),
                value: onoff(self.s.flex),
            });
            v.push(Command::Set {
                key: "system".into(),
                value: onoff(self.s.system),
            });
            v.push(Command::Set {
                key: "charset".into(),
                value: charset_name(self.s.charset).into(),
            });
            v
        }

        fn restore(&mut self, key: &str, value: &str) {
            let _ = self.s.set(key, value);
        }

        fn ui(&mut self, ui: &mut Ui, commands: &mut Vec<Command>) {
            egui::Grid::new("pagers_panel")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    ui.label("POCSAG");
                    ui.horizontal(|ui| {
                        for (i, (key, baud)) in POCSAG_KEYS.iter().enumerate() {
                            if ui
                                .checkbox(&mut self.s.pocsag[i], baud.to_string())
                                .changed()
                            {
                                commands.push(Command::Set {
                                    key: (*key).into(),
                                    value: onoff(self.s.pocsag[i]),
                                });
                            }
                        }
                    });
                    ui.end_row();
                    ui.label("FLEX");
                    if ui
                        .checkbox(&mut self.s.flex, "1600 / 3200 / 6400")
                        .changed()
                    {
                        commands.push(Command::Set {
                            key: "flex".into(),
                            value: onoff(self.s.flex),
                        });
                    }
                    ui.end_row();
                    ui.label("FLEX system words");
                    if ui
                        .checkbox(&mut self.s.system, "date, time, SSID in the log")
                        .changed()
                    {
                        commands.push(Command::Set {
                            key: "system".into(),
                            value: onoff(self.s.system),
                        });
                    }
                    ui.end_row();
                    ui.label("POCSAG text");
                    egui::ComboBox::from_id_salt("pagers_charset")
                        .selected_text(charset_name(self.s.charset))
                        .show_ui(ui, |ui| {
                            for c in [Charset::Auto, Charset::Alpha, Charset::Numeric] {
                                if ui
                                    .selectable_label(self.s.charset == c, charset_name(c))
                                    .clicked()
                                {
                                    self.s.charset = c;
                                    commands.push(Command::Set {
                                        key: "charset".into(),
                                        value: charset_name(c).into(),
                                    });
                                }
                            }
                        });
                    ui.end_row();
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use declab_api::Tuning;
    use declab_pagers::flex::{self, FlexBody, FlexPage};
    use declab_pagers::modulate::SignalBuilder;
    use declab_pagers::pocsag::{Body, PocsagMessage};

    #[test]
    fn decodes_both_protocols_and_fills_the_scopes() {
        let mut sig = SignalBuilder::new(RATE);
        sig.silence(0.2);
        sig.pocsag(
            1200,
            &[PocsagMessage {
                capcode: 123_456,
                function: 3,
                body: Body::Alpha("hello DecLAB".into()),
            }],
        );
        let pages = [FlexPage {
            capcode: 4242,
            body: FlexBody::Alpha("FLEX in DecLAB".into()),
        }];
        for frame in 0..2 {
            sig.flex(&flex::encode_frame(3200, 4, 0, frame, &pages).unwrap());
        }
        let iq = sig.to_iq(800.0, Some(20.0), 1.0 / 9600.0, 3);

        let mut d = Factory.create();
        d.command(
            Command::Set {
                key: "system".into(),
                value: "on".into(),
            },
            &mut Sink::default(),
        );
        let mut log = Vec::new();
        let mut symbols_seen = false;
        let tuning = Tuning {
            center_hz: 0.0,
            sample_rate: RATE,
            generation: 0,
        };
        for (n, chunk) in iq.chunks(5_000).enumerate() {
            let block = Block {
                iq: chunk,
                tuning,
                first_sample: n as u64 * 5_000,
                dropped_before: 0,
                live: false,
            };
            let mut sink = Sink::default();
            d.process(&block, &mut sink);
            log.append(&mut sink.log);
            if let Some((_, pts)) = sink.scopes.iter().find(|(n, _)| *n == "Symbols")
                && pts.len() == SYMBOL_POINTS
                && sink.lock != Lock::Searching
            {
                symbols_seen = true;
                assert!(sink.card.is_some());
            }
        }
        assert!(
            log.iter()
                .any(|l| l.contains("POCSAG1200") && l.ends_with("hello DecLAB")),
            "{log:#?}"
        );
        assert!(
            log.iter()
                .any(|l| l.contains("FLEX6400/4") && l.ends_with("FLEX in DecLAB")),
            "{log:#?}"
        );
        assert!(symbols_seen);
    }
}
