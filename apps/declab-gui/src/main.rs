// made by CasualArclamp
//! DecLAB's window: one receiver, any decoder.
//!
//! The top bar holds the frequency, the source and its gains, the level and
//! the decoder button. Under it the band (spectrum over waterfall): left
//! click tunes the decoder's channel, middle-drag moves the band, a middle
//! click opens the decoder wheel, scroll zooms. Below, the decoder's card,
//! its scope and readings, its own settings panel and its services; the
//! status bar last. Laid out from the agreed mockups (docs/DESIGN.md §1).
//!
//! ```text
//! declab-gui [CAPTURE] [--rate HZ] [--freq HZ] [--radio airspy|hackrf|rtlsdr]
//!            [--decoder ID] [--chan HZ] [--zoom Z] [--wheel]
//!            [--screenshot OUT.png] [--after SECS]
//! ```
//!
//! A screenshot run opens, runs `--after` seconds (default 4), saves the
//! window and closes; it neither reads nor writes the preferences.

mod band;
mod data_tabs;
mod epg_view;
mod external;
mod freq_display;
mod gain_opt;
mod panels;
mod player;
mod prefs;
mod redbutton;
mod services;
mod settings;
#[cfg(test)]
mod sim;
mod ts_viewer;
mod vfo;
mod tv;
mod waterfall;
mod wheel;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use declab_api::{DecoderFactory, Family, Panel};
use declab_engine::meter::CpuGauge;
use declab_engine::{DecoderView, Host, HostOptions, MAX_VFOS, Stats};
use declab_io::{IqFileReader, IqSource, format_from_path, name_hint};
use eframe::egui::{self, Color32, RichText};
use waterfall::{History, RingImage};
use wheel::{Entry, Outcome, Wheel, family_colour};

/// Smallest view span, Hz: zooming stops here.
const MIN_SPAN_HZ: f64 = 20e3;
/// The spectrum's FFT size: at least this, and at most this (or the GPU's
/// widest texture, the waterfall being one), about a bin a pixel across the
/// view between (finer as it zooms in).
const FFT_MIN: usize = 4096;
const FFT_MAX: usize = 16384;
/// The zoom holds this long before the FFT size follows it, s (a scroll
/// through several zoom steps asks once).
const FFT_SETTLE_S: f64 = 0.25;
/// Frames a second while a source runs: decoders report 60 times a second.
const FPS: f64 = 60.0;
/// Retunes while the band is being dragged, at most this often.
const RETUNE_EVERY: Duration = Duration::from_millis(90);

#[derive(Default)]
struct Options {
    file: Option<PathBuf>,
    rate: Option<f64>,
    freq: Option<f64>,
    radio: Option<SourceKind>,
    decoder: Option<String>,
    chan: Option<f64>,
    zoom: Option<f64>,
    wheel: bool,
    /// `--scope NAME`: show this decoder scope (for screenshots).
    scope: Option<String>,
    /// `--tab heard|hex|lrrp|ip`: Digital voice's tab to open on.
    tab: data_tabs::Tab,
    /// `--play SID`: play this service once its streams are known.
    play: Option<u16>,
    /// `--window ts|epg`: open with that window up.
    window: Option<String>,
    /// `--settings`: open with the Settings window up (for screenshots).
    settings: bool,
    /// `--optimise`: run the Airspy gain optimiser once the radio is up (it
    /// changes the gains, and the result is kept in the preferences).
    optimise: bool,
    /// `--bias-tee`: power the antenna port from the start (it is never
    /// saved, so it is off at every start unless this is given).
    bias_tee: bool,
    /// `--vfo ID@FREQ` (repeatable): more VFOs, besides the one `--decoder`
    /// and `--chan` give.
    vfos: Vec<(String, f64)>,
    /// `--rail-closed`: start with the VFO list collapsed.
    rail_closed: bool,
    screenshot: Option<PathBuf>,
    after: f64,
}

/// "98.8M", "2048k" or plain Hz.
fn parse_hz(v: &str) -> Result<f64, String> {
    let (num, scale) = match v.chars().last() {
        Some('k' | 'K') => (&v[..v.len() - 1], 1e3),
        Some('M' | 'm') => (&v[..v.len() - 1], 1e6),
        Some('G' | 'g') => (&v[..v.len() - 1], 1e9),
        _ => (v, 1.0),
    };
    num.parse::<f64>()
        .map(|n| n * scale)
        .map_err(|_| format!("{v:?} is not a frequency"))
}

impl Options {
    fn from_args() -> Result<Options, String> {
        let mut o = Options {
            after: 4.0,
            ..Options::default()
        };
        let mut args = std::env::args().skip(1);
        while let Some(a) = args.next() {
            let mut value = |what: &str| args.next().ok_or(format!("{a} needs {what}"));
            match a.as_str() {
                "--rate" => o.rate = Some(parse_hz(&value("a rate")?)?),
                "--freq" => o.freq = Some(parse_hz(&value("a frequency")?)?),
                "--chan" => o.chan = Some(parse_hz(&value("a frequency")?)?),
                "--zoom" => {
                    o.zoom = Some(value("a factor")?.parse().map_err(|_| "--zoom: a number")?)
                }
                "--decoder" => o.decoder = Some(value("a decoder id")?),
                "--radio" => {
                    o.radio = Some(match value("airspy, hackrf or rtlsdr")?.as_str() {
                        "airspy" => SourceKind::Airspy,
                        "hackrf" => SourceKind::HackRf,
                        "rtlsdr" | "rtl" => SourceKind::RtlSdr,
                        r => return Err(format!("unknown radio {r:?}")),
                    })
                }
                "--wheel" => o.wheel = true,
                "--scope" => o.scope = Some(value("a scope name")?),
                "--tab" => {
                    o.tab = match value("heard, hex, lrrp or ip")?.as_str() {
                        "heard" => data_tabs::Tab::Heard,
                        "hex" => data_tabs::Tab::Hex,
                        "lrrp" => data_tabs::Tab::Lrrp,
                        "ip" => data_tabs::Tab::Ip,
                        t => return Err(format!("--tab: no tab {t:?} (heard, hex, lrrp, ip)")),
                    }
                }
                "--settings" => o.settings = true,
                "--optimise" | "--optimize" => o.optimise = true,
                "--vfo" => {
                    let v = value("ID@FREQ")?;
                    let (id, hz) = v.split_once('@').ok_or("--vfo: ID@FREQ, like wfm@94.5M")?;
                    o.vfos.push((id.to_string(), parse_hz(hz)?));
                }
                "--rail-closed" => o.rail_closed = true,
                "--bias-tee" => o.bias_tee = true,
                "--play" => {
                    o.play = Some(
                        value("a service id")?
                            .parse()
                            .map_err(|_| "--play: a service id")?,
                    )
                }
                "--window" => o.window = Some(value("ts or epg")?),
                "--screenshot" => o.screenshot = Some(value("a .png path")?.into()),
                "--after" => o.after = value("seconds")?.parse().map_err(|_| "--after: seconds")?,
                "-h" | "--help" => {
                    return Err("declab-gui [CAPTURE] [--rate HZ] [--freq HZ] [--radio airspy|hackrf|rtlsdr] \
                                [--decoder ID] [--chan HZ] [--zoom Z] [--wheel] [--screenshot OUT.png] [--after SECS]"
                        .into());
                }
                f if !f.starts_with("--") => o.file = Some(f.into()),
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(o)
    }
}

/// libairspy's gain tables: the (LNA, mixer, VGA) indices at step `n` 0–21 of
/// the linearity or sensitivity knob.
#[cfg(feature = "airspy")]
fn air_preset(linear: bool, n: u8) -> (u8, u8, u8) {
    use declab_io::AirspyGains;
    let g = if linear { AirspyGains::linearity(n) } else { AirspyGains::sensitivity(n) };
    (g.lna, g.mixer, g.vga)
}

#[cfg(not(feature = "airspy"))]
fn air_preset(_: bool, _: u8) -> (u8, u8, u8) {
    (0, 0, 0)
}

/// The window's icon, from the PNG built in (drawn by `examples/make_icon.rs`).
/// Rust note: `include_bytes!` puts the file in the exe at compile time.
fn window_icon() -> egui::IconData {
    let rgba = image::load_from_memory(include_bytes!("../assets/icon-256.png"))
        .map(|i| i.to_rgba8())
        .unwrap_or_default();
    egui::IconData {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    }
}

fn main() -> eframe::Result {
    let opts = match Options::from_args() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DecLAB")
            .with_inner_size([1440.0, 940.0])
            .with_min_inner_size([960.0, 640.0])
            .with_icon(window_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "DecLAB",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_theme(egui::Theme::Dark);
            // One monospace face throughout, as in the other Dec apps.
            cc.egui_ctx.style_mut_of(egui::Theme::Dark, |s| {
                s.override_text_style = Some(egui::TextStyle::Monospace);
            });
            Ok(Box::new(App::new(opts)))
        }),
    )
}

/// One of the decoder's own waterfalls (`declab_api::WaterfallRow`).
struct MiniState {
    name: &'static str,
    hist: History,
    ring: RingImage,
    low_hz: f32,
    span_hz: f32,
    marks: Vec<(f32, &'static str)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SourceKind {
    Airspy,
    HackRf,
    RtlSdr,
    File,
}

impl SourceKind {
    fn label(self) -> &'static str {
        match self {
            SourceKind::Airspy => "Airspy R2",
            SourceKind::HackRf => "HackRF One",
            SourceKind::RtlSdr => "RTL-SDR",
            SourceKind::File => "IQ file",
        }
    }
    fn key(self) -> &'static str {
        match self {
            SourceKind::Airspy => "airspy",
            SourceKind::HackRf => "hackrf",
            SourceKind::RtlSdr => "rtlsdr",
            SourceKind::File => "file",
        }
    }
}

/// Live controls of the open radio (gains, retune from the GUI thread).
enum Control {
    None,
    #[cfg(feature = "hackrf")]
    HackRf(declab_io::HackRfControl),
    #[cfg(feature = "airspy")]
    Airspy(declab_io::AirspyControl),
    #[cfg(feature = "rtlsdr")]
    RtlSdr(declab_io::RtlSdrControl),
}

struct App {
    opts: Options,
    /// What is to be opened.
    kind: SourceKind,
    file: Option<PathBuf>,
    file_rate: f64,
    /// The radio's centre (what it is tuned to, or a file's label).
    center_hz: f64,
    hack_rate: f64,
    hack_amp: bool,
    hack_lna: u16,
    hack_vga: u16,
    /// Power the antenna port of a live radio (an LNA or active antenna).
    /// Never saved: off at every start of the program, since DC on the wrong
    /// antenna can damage it.
    bias_tee: bool,
    air_lna: u8,
    air_mix: u8,
    air_vga: u8,
    /// The Airspy's one gain knob: libairspy's linearity (true) or
    /// sensitivity table, and the step 0–21 on it. The three stages above
    /// follow it; the optimiser may leave them between steps.
    air_linear: bool,
    air_step: u8,
    /// The RTL-SDR's tuner AGC, its manual gain (tenths of a dB, a step of
    /// the R820T's table) and the dongle's clock error in ppm.
    rtl_agc: bool,
    rtl_db10: u16,
    rtl_ppm: f64,
    /// The Airspy gain optimiser while it runs, and what the last run found.
    opt: Option<gain_opt::Optimiser>,
    opt_note: Option<String>,

    host: Option<Host>,
    control: Control,
    live: bool,
    source_rate: f64,
    error: Option<String>,

    /// The VFOs, in the host's order; `cur` is the selected one, whose live
    /// values are the fields below (`dec`, `chan_hz`, `bandwidth_hz`, `scope`,
    /// `selected_service`, `view`). See `vfo.rs`.
    slots: Vec<vfo::Slot>,
    cur: usize,
    /// The VFO list down the left is open (else a strip of numbered discs).
    rail_open: bool,
    /// Index into `declab_decoders::ALL`.
    dec: usize,
    /// Every decoder's settings panel (None: it has none), by index into
    /// `declab_decoders::ALL`; the Settings window shows them.
    panels: Vec<Option<Box<dyn Panel>>>,
    settings_open: bool,
    /// What follows a decoder's transport stream: services, player, guide.
    tv: tv::Tv,
    /// Digital voice's tabs over the right-hand box: heard, hex, LRRP, IP.
    data_tabs: data_tabs::DataTabs,
    /// The settings window's page: None for General, else a decoder.
    settings_page: Option<usize>,
    chan_hz: f64,
    bandwidth_hz: f64,
    selected_service: Option<u32>,
    volume: f32,
    /// The top bar's mute: the volume is kept, the sound is off.
    muted: bool,
    /// The scope shown: "" for the channel IQ, else a decoder scope's name.
    scope: String,

    /// The view: its centre, and how far it is zoomed into the source's span.
    view_hz: f64,
    zoom: f64,
    /// The spectrum's FFT size in use, the one the view asks for, and since
    /// when it has asked (see `follow_zoom`).
    fft: usize,
    fft_want: usize,
    fft_since: Instant,
    dragging: bool,
    last_retune: Instant,
    wheel: Option<Wheel>,

    history: History,
    ring: RingImage,
    /// The decoder's own waterfalls, by name.
    minis: Vec<MiniState>,
    /// The other VFOs' own waterfalls, by VFO number, kept while another
    /// is selected (switching back finds them as they were, not empty);
    /// and which VFO `minis` belongs to.
    slot_minis: std::collections::HashMap<usize, Vec<MiniState>>,
    minis_owner: Option<usize>,
    /// The large one's height, physical pixels (its rows).
    big_rows: usize,
    smooth: Vec<f32>,
    row_center: f64,
    row_rate: f64,
    wf_rows: usize,

    stats: Stats,
    view: DecoderView,
    cpu: CpuGauge,

    started: Instant,
    /// Frames drawn, and the pacing (see `pace`).
    frames: u64,
    last_frame: Option<Instant>,
    delay_ms: f64,
    shot_sent: bool,
    /// For a screenshot run: process CPU seconds and the time at 3 s, so the
    /// report averages the run after start-up.
    cpu_mark: Option<(f64, Instant)>,
    cpu_avg: Option<f32>,
    /// Time in each part of the frame (screenshot runs report it): poll,
    /// bottom panel, band, windows, top bar and status.
    frame_cost: [Duration; 5],
    saving: bool,
}

impl App {
    fn new(opts: Options) -> App {
        // A screenshot run must come out the same each time: no preferences.
        let saving = opts.screenshot.is_none();
        let p = if saving { prefs::load() } else { Vec::new() };
        let kind = opts
            .radio
            .or(opts.file.as_ref().map(|_| SourceKind::File))
            .unwrap_or(match prefs::get(&p, "source") {
                Some("hackrf") => SourceKind::HackRf,
                Some("file") => SourceKind::File,
                Some("rtlsdr") => SourceKind::RtlSdr,
                _ => SourceKind::Airspy,
            });
        let file = opts
            .file
            .clone()
            .or_else(|| prefs::get(&p, "file").map(PathBuf::from));
        let dec_id = opts
            .decoder
            .clone()
            .or_else(|| prefs::get(&p, "decoder").map(String::from));
        let dec = dec_id
            .and_then(|id| declab_decoders::ALL.iter().position(|f| f.info().id == id))
            .unwrap_or(0);
        let center_hz = opts.freq.or(prefs::num(&p, "centre")).unwrap_or(98.0e6);
        let chan_hz = opts.chan.or(prefs::num(&p, "chan")).unwrap_or(center_hz);
        let mut app = App {
            kind,
            file,
            file_rate: opts.rate.unwrap_or(10e6),
            center_hz,
            hack_rate: opts
                .rate
                .filter(|_| kind == SourceKind::HackRf)
                .or(prefs::num(&p, "hackrf_rate"))
                .unwrap_or(10e6),
            hack_amp: prefs::get(&p, "hackrf_amp") == Some("1"),
            hack_lna: prefs::num(&p, "hackrf_lna").unwrap_or(24),
            hack_vga: prefs::num(&p, "hackrf_vga").unwrap_or(20),
            bias_tee: opts.bias_tee,
            air_lna: prefs::num(&p, "airspy_lna").unwrap_or(12),
            air_mix: prefs::num(&p, "airspy_mix").unwrap_or(10),
            air_vga: prefs::num(&p, "airspy_vga").unwrap_or(8),
            air_linear: prefs::get(&p, "airspy_mode") == Some("linearity"),
            air_step: prefs::num(&p, "airspy_step").unwrap_or(10),
            rtl_agc: prefs::get(&p, "rtlsdr_agc") == Some("1"),
            rtl_db10: prefs::num(&p, "rtlsdr_gain").unwrap_or(declab_io::RtlGains::default().db10),
            rtl_ppm: prefs::num(&p, "rtlsdr_ppm").unwrap_or(0.0),
            opt: None,
            opt_note: None,
            host: None,
            control: Control::None,
            live: false,
            source_rate: 0.0,
            error: None,
            slots: Vec::new(),
            cur: 0,
            rail_open: prefs::get(&p, "vfo_rail") != Some("0") && !opts.rail_closed,
            dec,
            panels: declab_decoders::ALL
                .iter()
                .map(|f| {
                    let mut panel = f.panel();
                    // Settings kept from earlier runs: "set.<id>.<key>".
                    if let Some(pn) = &mut panel {
                        let pre = format!("set.{}.", f.info().id);
                        for (k, v) in &p {
                            if let Some(key) = k.strip_prefix(&pre) {
                                pn.restore(key, v);
                            }
                        }
                    }
                    panel
                })
                .collect(),
            settings_open: opts.settings,
            tv: {
                let mut t = tv::Tv::new(opts.window.as_deref());
                // Screenshot (test) runs are silent.
                if opts.screenshot.is_some() {
                    t.mute();
                }
                t.play_when_ready = opts.play;
                t
            },
            data_tabs: data_tabs::DataTabs::opening_on(opts.tab),
            settings_page: None,
            // (`--settings` opens on the running decoder's page: set in `new`.)
            chan_hz,
            bandwidth_hz: 0.0,
            selected_service: None,
            volume: prefs::num(&p, "volume").unwrap_or(0.8),
            muted: false,
            scope: opts.scope.clone().unwrap_or_else(|| declab_decoders::ALL[dec].default_scope().to_string()),
            view_hz: center_hz,
            zoom: opts.zoom.or(prefs::num(&p, "zoom")).unwrap_or(1.0),
            fft: FFT_MIN,
            fft_want: FFT_MIN,
            fft_since: Instant::now(),
            dragging: false,
            last_retune: Instant::now(),
            wheel: None,
            history: History::default(),
            ring: RingImage::default(),
            minis: Vec::new(),
            slot_minis: std::collections::HashMap::new(),
            minis_owner: None,
            big_rows: 240,
            smooth: Vec::new(),
            row_center: 0.0,
            row_rate: 0.0,
            wf_rows: 0,
            stats: Stats::default(),
            view: DecoderView::default(),
            cpu: CpuGauge::default(),
            started: Instant::now(),
            frames: 0,
            last_frame: None,
            delay_ms: 1000.0 / FPS,
            shot_sent: false,
            cpu_mark: None,
            cpu_avg: None,
            frame_cost: [Duration::ZERO; 5],
            saving,
            opts,
        };
        app.choose_decoder(dec);
        let first = app.slot_from_app(1);
        app.slots.push(first);
        for (id, hz) in app.opts.vfos.clone() {
            match declab_decoders::ALL.iter().position(|f| f.info().id == id) {
                Some(d) => {
                    app.push_slot(d, hz, true);
                }
                None => eprintln!("--vfo: no decoder {id:?}"),
            }
        }
        if app.settings_open {
            app.settings_page = Some(app.dec);
        }
        // A file or radio named on the command line starts at once.
        if app.opts.file.is_some() || app.opts.radio.is_some() {
            app.start();
        }
        app
    }

    // ---- VFOs ----------------------------------------------------------

    /// A slot holding the selected VFO's live values.
    fn slot_from_app(&self, n: usize) -> vfo::Slot {
        vfo::Slot {
            n,
            colour: vfo::COLOURS[(n - 1) % vfo::COLOURS.len()],
            vid: None,
            muted: false,
            dec: self.dec,
            chan_hz: self.chan_hz,
            bandwidth_hz: self.bandwidth_hz,
            scope: self.scope.clone(),
            selected_service: self.selected_service,
            view: DecoderView::default(),
        }
    }

    /// Write the selected VFO's live values into its slot (before another
    /// is swapped in, or the host started).
    fn stash(&mut self) {
        let Some(s) = self.slots.get_mut(self.cur) else { return };
        s.dec = self.dec;
        s.chan_hz = self.chan_hz;
        s.bandwidth_hz = self.bandwidth_hz;
        s.scope = self.scope.clone();
        s.selected_service = self.selected_service;
        s.view = self.view.clone();
    }

    /// Make slot `i` the selected VFO: its values become the live ones and
    /// the host's single-decoder calls follow it.
    fn load_slot(&mut self, i: usize) {
        let s = &self.slots[i];
        self.cur = i;
        self.dec = s.dec;
        self.chan_hz = s.chan_hz;
        self.bandwidth_hz = s.bandwidth_hz;
        self.scope = s.scope.clone();
        self.selected_service = s.selected_service;
        self.view = s.view.clone();
        // The old VFO's waterfalls are kept for when it is selected again,
        // and this one's come back as they were. (Its stream is not this
        // one's.)
        let n = s.n;
        match self.minis_owner.replace(n) {
            Some(old) if old == n => {}
            Some(old) => {
                let mine = self.slot_minis.remove(&n).unwrap_or_default();
                self.slot_minis.insert(old, std::mem::replace(&mut self.minis, mine));
            }
            None => self.minis = self.slot_minis.remove(&n).unwrap_or_default(),
        }
        self.tv.reset();
        if let (Some(h), Some(v)) = (&mut self.host, self.slots[i].vid) {
            h.set_focus(v);
        }
    }

    fn select_slot(&mut self, i: usize) {
        if i == self.cur || i >= self.slots.len() {
            return;
        }
        self.stash();
        self.load_slot(i);
        self.save_prefs();
    }

    /// A new slot for decoder `dec` at `hz` (on the running host too), muted
    /// if asked. Returns its index.
    fn push_slot(&mut self, dec: usize, hz: f64, muted: bool) -> usize {
        let f = declab_decoders::ALL[dec];
        let bw = f.input(self.rate()).bandwidth_hz;
        let n = (1..=MAX_VFOS).find(|k| !self.slots.iter().any(|s| s.n == *k)).unwrap_or(1);
        let vid = self.host.as_mut().and_then(|h| h.add_vfo(f, hz, bw));
        if let (Some(h), Some(v)) = (&self.host, vid) {
            if muted {
                h.set_vfo_gain(v, 0.0);
            }
            if let Some(Some(p)) = self.panels.get(dec) {
                for c in p.settings() {
                    h.vfo_command(v, c);
                }
            }
        }
        self.slots.push(vfo::Slot {
            n,
            colour: vfo::COLOURS[(n - 1) % vfo::COLOURS.len()],
            vid,
            muted,
            dec,
            chan_hz: hz,
            bandwidth_hz: bw,
            scope: f.default_scope().to_string(),
            selected_service: None,
            view: DecoderView::default(),
        });
        self.slots.len() - 1
    }

    /// Somewhere in the radio's span clear of the other VFOs.
    fn free_spot(&self) -> f64 {
        let rate = self.rate();
        let clear = |hz: f64| self.slots.iter().all(|s| (s.chan_hz - hz).abs() > s.bandwidth_hz.max(50e3) * 1.5);
        [0.0, 0.15, -0.15, 0.3, -0.3, 0.05, -0.05, 0.4, -0.4]
            .iter()
            .map(|k| self.center_hz + k * rate)
            .find(|hz| clear(*hz))
            .unwrap_or(self.center_hz)
    }

    /// Add a VFO running the selected one's decoder, muted, and select it.
    fn add_vfo(&mut self, hz: Option<f64>) {
        if self.slots.len() >= MAX_VFOS {
            return;
        }
        self.stash();
        let snap = if self.factory().info().id == "wfm" { 100e3 } else { 1e3 };
        let hz = hz.map_or_else(|| self.free_spot(), |h| (h / snap).round() * snap);
        let i = self.push_slot(self.dec, hz, true);
        self.load_slot(i);
        self.save_prefs();
    }

    /// Remove VFO `i` (the last one stays).
    fn remove_slot(&mut self, i: usize) {
        if self.slots.len() <= 1 || i >= self.slots.len() {
            return;
        }
        if let (Some(h), Some(v)) = (&mut self.host, self.slots[i].vid) {
            h.remove_vfo(v);
        }
        let gone = self.slots[i].n;
        self.slots.remove(i);
        if i == self.cur {
            // Its waterfalls go with it.
            self.minis.clear();
            self.minis_owner = None;
            // Its values are gone: the neighbour's become the live ones.
            self.load_slot(i.min(self.slots.len() - 1));
        } else if i < self.cur {
            self.cur -= 1;
        }
        self.slot_minis.remove(&gone);
        self.save_prefs();
    }

    fn toggle_mute(&mut self, i: usize) {
        let Some(s) = self.slots.get_mut(i) else { return };
        s.muted = !s.muted;
        if let (Some(h), Some(v)) = (&self.host, s.vid) {
            h.set_vfo_gain(v, if s.muted { 0.0 } else { 1.0 });
        }
    }

    fn factory(&self) -> &'static dyn DecoderFactory {
        declab_decoders::ALL[self.dec]
    }

    /// Where a radio starts: the remembered centre if the radio can tune
    /// there and the channel lies in its span, else the channel itself,
    /// within the radio's range. (A file's centre, 0 Hz for a simulated
    /// one, stays remembered after switching to a radio: handed over as it
    /// was, the radio refused to start.)
    #[allow(dead_code)] // unused when no radio is built in
    fn live_centre(&mut self, lo_hz: f64, hi_hz: f64, rate: f64) -> u64 {
        let tunable = (lo_hz..=hi_hz).contains(&self.center_hz);
        if !tunable || (self.chan_hz - self.center_hz).abs() > rate / 2.0 {
            self.center_hz = self.chan_hz.clamp(lo_hz, hi_hz).round();
        }
        self.center_hz as u64
    }

    /// Finer FFT bins as the view zooms in: about one a pixel across the
    /// view, `px` wide, once the zoom has held still for a moment. The
    /// waterfall keeps its rows (rescaled) when the size changes.
    fn follow_zoom(&mut self, px: f64, max_texture: usize) {
        // The largest power of two the GPU can take as a texture's width.
        let limit = FFT_MAX.min(max_texture).max(FFT_MIN);
        let cap = if limit.is_power_of_two() { limit } else { limit.next_power_of_two() / 2 };
        let want = ((self.zoom * px).ceil() as usize).next_power_of_two().clamp(FFT_MIN, cap);
        if want != self.fft_want {
            self.fft_want = want;
            self.fft_since = Instant::now();
        }
        if want != self.fft
            && self.fft_since.elapsed().as_secs_f64() >= FFT_SETTLE_S
            && let Some(h) = &self.host
        {
            h.set_fft(want);
            self.fft = want;
        }
    }

    /// The rate the source runs at, or would.
    fn rate(&self) -> f64 {
        if self.source_rate > 0.0 {
            return self.source_rate;
        }
        match self.kind {
            SourceKind::Airspy => 10e6,
            SourceKind::HackRf => self.hack_rate,
            SourceKind::RtlSdr => declab_io::rtlsdr_proto::SAMPLE_RATE_HZ as f64,
            SourceKind::File => self.file_rate,
        }
    }

    fn span(&self) -> f64 {
        (self.rate() / self.zoom).max(MIN_SPAN_HZ)
    }

    fn open_source(&mut self) -> Result<Box<dyn IqSource>, String> {
        self.control = Control::None;
        match self.kind {
            SourceKind::File => {
                let path = self.file.clone().ok_or("open a capture first")?;
                let hint = name_hint(&path);
                let fmt = format_from_path(&path)
                    .ok_or("unknown sample format (.cs8 .cs16 .cf32 .cu8 .wav)")?;
                let rate = self
                    .opts
                    .rate
                    .or(hint.sample_rate)
                    .unwrap_or(self.file_rate);
                self.file_rate = rate;
                let mut r =
                    IqFileReader::open(&path, fmt, rate, 131_072).map_err(|e| e.to_string())?;
                if let Some(c) = self.opts.freq.or(hint.center_hz) {
                    r = r.with_center_freq(c);
                }
                Ok(Box::new(r))
            }
            #[cfg(feature = "hackrf")]
            SourceKind::HackRf => {
                use declab_io::{HackRfGains, HackRfSettings, HackRfSource};
                // The HackRF One tunes 1 MHz to 6 GHz.
                let centre = self.live_centre(1e6, 6e9, self.hack_rate);
                let s = HackRfSettings {
                    center_hz: centre,
                    sample_rate: self.hack_rate as u32,
                    gains: HackRfGains {
                        amp: self.hack_amp,
                        lna_db: self.hack_lna,
                        vga_db: self.hack_vga,
                    },
                    bias_tee: self.bias_tee,
                };
                let src = HackRfSource::open(s).map_err(|e| e.to_string())?;
                self.control = Control::HackRf(src.control());
                Ok(Box::new(src))
            }
            #[cfg(feature = "airspy")]
            SourceKind::Airspy => {
                use declab_io::{AirspyGains, AirspySettings, AirspySource};
                use declab_io::airspy::proto::{FREQ_MAX_HZ, FREQ_MIN_HZ};
                let centre = self.live_centre(FREQ_MIN_HZ as f64, FREQ_MAX_HZ as f64, 10e6);
                let s = AirspySettings {
                    center_hz: centre,
                    gains: AirspyGains {
                        lna: self.air_lna,
                        mixer: self.air_mix,
                        vga: self.air_vga,
                    }
                    .clamped(),
                    bias_tee: self.bias_tee,
                    ..AirspySettings::default()
                };
                let src = AirspySource::open(s).map_err(|e| e.to_string())?;
                self.control = Control::Airspy(src.control());
                Ok(Box::new(src))
            }
            #[cfg(feature = "rtlsdr")]
            SourceKind::RtlSdr => {
                use declab_io::{RtlSdrSource, RtlSettings};
                use declab_io::rtlsdr_proto::{FREQ_MAX_HZ, FREQ_MIN_HZ, SAMPLE_RATE_HZ};
                let centre =
                    self.live_centre(FREQ_MIN_HZ as f64, FREQ_MAX_HZ as f64, SAMPLE_RATE_HZ as f64);
                let s = RtlSettings {
                    center_hz: centre,
                    gains: self.rtl_gains(),
                    ppm: self.rtl_ppm,
                    bias_tee: self.bias_tee,
                    ..RtlSettings::default()
                };
                let src = RtlSdrSource::open(s).map_err(|e| e.to_string())?;
                self.control = Control::RtlSdr(src.control());
                Ok(Box::new(src))
            }
            #[allow(unreachable_patterns)]
            k => Err(format!("{} support was not built in", k.label())),
        }
    }

    fn start(&mut self) {
        self.stop();
        match self.open_source() {
            Ok(src) => {
                self.live = src.is_live();
                self.source_rate = src.sample_rate();
                if !self.live {
                    self.center_hz = src.center_freq();
                    if (self.chan_hz - self.center_hz).abs() > self.source_rate / 2.0 {
                        self.chan_hz = self.center_hz;
                    }
                }
                self.view_hz = self.center_hz;
                self.clamp_view();
                let opts = HostOptions {
                    fft: self.fft,
                    rows_per_s: 30.0,
                    segments_per_row: 12,
                    ..HostOptions::default()
                };
                let mut host = Host::start(src, opts);
                // Screenshot (test) runs are silent.
                host.set_volume(self.effective_volume());
                // Every VFO, in order (the first is the one that may retune
                // the radio), each with its decoder's settings.
                self.stash();
                for s in &mut self.slots {
                    s.vid = host.add_vfo(declab_decoders::ALL[s.dec], s.chan_hz, s.bandwidth_hz);
                    let Some(v) = s.vid else { continue };
                    if s.muted {
                        host.set_vfo_gain(v, 0.0);
                    }
                    if let Some(Some(p)) = self.panels.get(s.dec) {
                        for c in p.settings() {
                            host.vfo_command(v, c);
                        }
                    }
                }
                if let Some(v) = self.slots.get(self.cur).and_then(|s| s.vid) {
                    host.set_focus(v);
                }
                self.host = Some(host);
                self.history.reset();
                self.smooth.clear();
                self.row_rate = 0.0;
                self.error = None;
                self.save_prefs();
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn stop(&mut self) {
        self.opt = None;
        self.tv.reset();
        self.host = None;
        for s in &mut self.slots {
            s.vid = None;
        }
        self.control = Control::None;
        self.source_rate = 0.0;
    }

    /// Run decoder `i` (on the running host too).
    fn choose_decoder(&mut self, i: usize) {
        let changed = self.dec != i.min(declab_decoders::ALL.len() - 1);
        self.dec = i.min(declab_decoders::ALL.len() - 1);
        let f = self.factory();
        self.bandwidth_hz = f.input(self.rate()).bandwidth_hz;
        // A new decoder opens on its own scope (`--scope` still wins at start).
        if changed && !f.default_scope().is_empty() {
            self.scope = f.default_scope().to_string();
        }
        self.selected_service = None;
        self.view = DecoderView::default();
        self.minis.clear();
        self.tv.reset();
        if let Some(h) = &mut self.host {
            // The new decoder takes the selected VFO's place (and its id).
            h.set_decoder(Some(f));
            h.set_channel_at(self.chan_hz, self.bandwidth_hz);
            let vid = h.focused_id();
            if let Some(s) = self.slots.get_mut(self.cur) {
                s.vid = vid;
                if let (true, Some(v)) = (s.muted, vid) {
                    h.set_vfo_gain(v, 0.0);
                }
            }
        }
        self.send_settings();
        self.save_prefs();
    }

    /// The volume the sound card gets: none when muted, and none in a
    /// screenshot (test) run.
    fn effective_volume(&self) -> f32 {
        if self.muted || !self.saving { 0.0 } else { self.volume }
    }

    /// The decoder makes its own sound (the analog modes, digital radio): the
    /// top bar shows the volume. TV sound has the player window's slider.
    fn makes_sound(&self) -> bool {
        matches!(self.factory().info().family, Family::Analog | Family::Radio | Family::Voice)
    }

    /// The volume slider and mute button.
    fn volume_ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        let mute_text = if self.muted { "Unmute" } else { "Mute" };
        if ui.selectable_label(self.muted, mute_text).clicked() {
            self.muted = !self.muted;
            changed = true;
        }
        ui.spacing_mut().slider_width = 120.0;
        let r = ui.add_enabled(!self.muted, egui::Slider::new(&mut self.volume, 0.0..=1.0).show_value(false));
        if r.changed() {
            changed = true;
        }
        ui.label(RichText::new(format!("{:.0} %", self.volume * 100.0)).size(12.0).color(Color32::GRAY));
        if changed {
            if let Some(h) = &self.host {
                h.set_volume(self.effective_volume());
            }
            // Saved when the drag ends, not at every step of it.
            if !r.dragged() {
                self.save_prefs();
            }
        }
        if r.drag_stopped() {
            self.save_prefs();
        }
    }

    /// The running decoder starts as its settings panel shows.
    fn send_settings(&self) {
        if let (Some(h), Some(Some(p))) = (&self.host, self.panels.get(self.dec)) {
            for c in p.settings() {
                h.command(c);
            }
        }
    }

    /// Keep decoder `i`'s settings for the next run.
    fn save_settings(&self, i: usize) {
        if !self.saving {
            return;
        }
        let Some(Some(p)) = self.panels.get(i) else {
            return;
        };
        let id = declab_decoders::ALL[i].info().id;
        let pairs: Vec<(String, String)> = p
            .settings()
            .into_iter()
            .filter_map(|c| match c {
                declab_api::Command::Set { key, value } => Some((format!("set.{id}.{key}"), value)),
                _ => None,
            })
            .collect();
        let refs: Vec<(&str, String)> =
            pairs.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        prefs::set_many(&refs);
    }

    fn tune_channel(&mut self, hz: f64) {
        // Broadcast FM sits on a 100 kHz raster; anything else to 1 kHz.
        let snap = if self.factory().info().id == "wfm" {
            100e3
        } else {
            1e3
        };
        self.tune_exact((hz / snap).round() * snap);
    }

    /// Tune the channel to exactly `hz` (a TETRA carrier sits 6.25 or
    /// 12.5 kHz off the 25 kHz raster: rounding to 1 kHz would cost it
    /// 500 Hz).
    fn tune_exact(&mut self, hz: f64) {
        self.chan_hz = hz;
        // Off the edge of a radio's span: retune to it.
        if self.live
            && (self.chan_hz - self.center_hz).abs() > self.rate() / 2.0 - self.bandwidth_hz
        {
            self.retune(self.chan_hz);
            self.view_hz = self.chan_hz;
        }
        if let Some(h) = &self.host {
            h.set_channel_at(self.chan_hz, self.bandwidth_hz);
        }
        self.save_prefs();
    }

    fn retune(&mut self, hz: f64) {
        self.center_hz = hz.round();
        if let Some(h) = &self.host {
            h.retune(self.center_hz);
        }
        self.last_retune = Instant::now();
    }

    /// Keep the view's centre on what the source covers: a file's band can
    /// be dragged until its edge reaches the middle of the view; a radio's
    /// view is the radio's span, which a drag retunes.
    fn clamp_view(&mut self) {
        let half = if self.live {
            (self.rate() - self.span()).max(0.0) / 2.0
        } else {
            self.rate() / 2.0
        };
        self.view_hz = self
            .view_hz
            .clamp(self.center_hz - half, self.center_hz + half);
    }

    /// The antenna-port power switch, beside the gain menu of a live radio.
    fn bias_tee_ui(&mut self, ui: &mut egui::Ui) {
        let on = self.bias_tee;
        let text = RichText::new("Bias tee").color(if on { Color32::from_rgb(255, 176, 40) } else { Color32::GRAY });
        let r = ui.selectable_label(on, text).on_hover_text(
            "Power the antenna port (3.3-5 V) for an LNA or active antenna.\n\
             Off at every start of DecLAB: leave it off for a plain or\n\
             DC-grounded antenna, which it can damage.",
        );
        if r.clicked() {
            self.bias_tee = !on;
            self.apply_bias_tee();
        }
    }

    /// Send the bias tee setting to the open radio.
    fn apply_bias_tee(&mut self) {
        let on = self.bias_tee;
        let r = match &self.control {
            #[cfg(feature = "hackrf")]
            Control::HackRf(c) => c.set_bias_tee(on),
            #[cfg(feature = "airspy")]
            Control::Airspy(c) => c.set_bias_tee(on),
            #[cfg(feature = "rtlsdr")]
            Control::RtlSdr(c) => c.set_bias_tee(on),
            _ => Ok(()),
        };
        if let Err(e) = r {
            self.error = Some(e.to_string());
            self.bias_tee = false;
        }
    }

    fn apply_gains(&mut self) {
        match &self.control {
            #[cfg(feature = "hackrf")]
            Control::HackRf(c) => {
                let g = declab_io::HackRfGains {
                    amp: self.hack_amp,
                    lna_db: self.hack_lna,
                    vga_db: self.hack_vga,
                };
                if let Err(e) = c.set_gains(g.snapped()) {
                    self.error = Some(e.to_string());
                }
            }
            #[cfg(feature = "airspy")]
            Control::Airspy(c) => {
                let g = declab_io::AirspyGains {
                    lna: self.air_lna,
                    mixer: self.air_mix,
                    vga: self.air_vga,
                };
                if let Err(e) = c.set_gains(g.clamped()) {
                    self.error = Some(e.to_string());
                }
            }
            #[cfg(feature = "rtlsdr")]
            Control::RtlSdr(c) => {
                let r = c.set_gains(self.rtl_gains()).and_then(|()| c.set_ppm(self.rtl_ppm));
                if let Err(e) = r {
                    self.error = Some(e.to_string());
                }
            }
            _ => {}
        }
        self.save_prefs();
    }

    fn rtl_gains(&self) -> declab_io::RtlGains {
        declab_io::RtlGains { agc: self.rtl_agc, db10: declab_io::RtlGains::nearest_step(self.rtl_db10) }
    }

    /// Set the radio's gains to what the optimiser says.
    fn set_opt_gains(&mut self, g: gain_opt::Gains) {
        match g {
            gain_opt::Gains::Air(g) => (self.air_lna, self.air_mix, self.air_vga) = (g.lna, g.mixer, g.vga),
            gain_opt::Gains::Hack(g) => (self.hack_amp, self.hack_lna, self.hack_vga) = (g.amp, g.lna, g.vga),
        }
        self.apply_gains();
    }

    /// Start the gain optimiser on the signal being received (see
    /// `gain_opt.rs`): DecDVB-T's search for the HackRF; for the Airspy, the
    /// sensitivity ladder in steps of two rungs, then each stage stepped
    /// about the best.
    fn start_optimise(&mut self) {
        if self.opt.is_some() {
            return;
        }
        match &self.control {
            #[cfg(feature = "hackrf")]
            Control::HackRf(_) => {
                let original = gain_opt::HackGains { amp: self.hack_amp, lna: self.hack_lna, vga: self.hack_vga };
                self.opt = Some(gain_opt::Optimiser::hackrf(original));
                self.opt_note = None;
            }
            #[cfg(feature = "airspy")]
            Control::Airspy(_) => {
                let as_gains = |a: declab_io::AirspyGains| gain_opt::AirGains { lna: a.lna, mixer: a.mixer, vga: a.vga };
                let mut ladder: Vec<gain_opt::AirGains> = (0..=20u8)
                    .step_by(2)
                    .chain(std::iter::once(21))
                    .map(|n| as_gains(declab_io::AirspyGains::sensitivity(n)))
                    .collect();
                ladder.dedup();
                let original = gain_opt::AirGains { lna: self.air_lna, mixer: self.air_mix, vga: self.air_vga };
                self.opt = Some(gain_opt::Optimiser::airspy(original, ladder));
                self.opt_note = None;
            }
            _ => {}
        }
    }

    /// The optimiser's part of a gain menu: its button (or progress and
    /// Cancel) and what the last run found. Returns (start, cancel).
    fn optimiser_menu(&self, ui: &mut egui::Ui) -> (bool, bool) {
        let (mut start, mut cancel) = (false, false);
        ui.separator();
        if let Some(o) = &self.opt {
            ui.label(format!("optimising… {} settings tried", o.done()));
            cancel = ui.button("Cancel").clicked();
        } else {
            let can = self.host.is_some() && self.live;
            start = ui
                .add_enabled(can, egui::Button::new("Optimise gain"))
                .on_hover_text("Tries gain settings on the tuned channel and keeps the one with the best MER (or C/N)")
                .clicked();
            if let Some(n) = &self.opt_note {
                ui.label(RichText::new(n).size(12.0).color(Color32::GRAY));
            }
        }
        (start, cancel)
    }

    /// Stop the optimiser and put the gains back as they were.
    fn cancel_optimise(&mut self) {
        if let Some(o) = self.opt.take() {
            self.set_opt_gains(o.original);
            self.opt_note = Some("cancelled: the old gains are back".into());
        }
    }

    /// Feed the optimiser what the radio and decoder say this frame.
    fn step_optimiser(&mut self) {
        let ready = self.live && self.stats.samples > 0;
        if self.opts.optimise
            && self.opt.is_none()
            && ready
            && self.started.elapsed() > Duration::from_secs(3)
        {
            self.opts.optimise = false;
            self.start_optimise();
        }
        let Some(mut o) = self.opt.take() else { return };
        if !ready {
            self.opt = Some(o);
            return;
        }
        let mer = if self.view.lock == declab_api::Lock::Searching {
            None
        } else {
            self.view
                .measurements
                .iter()
                .find(|m| m.name == "MER")
                .map(|m| m.value as f32)
        };
        let cn = gain_opt::channel_cn(
            &self.smooth,
            self.row_center,
            self.row_rate,
            self.chan_hz,
            self.bandwidth_hz,
        );
        let r = gain_opt::Reading {
            now: Instant::now(),
            mer,
            cn,
            rms_dbfs: self.stats.rms_dbfs,
            clip_percent: self.stats.clip_percent,
        };
        let tried = o.done();
        let action = o.tick(&r);
        if o.done() > tried
            && let Some(p) = o.points.last()
        {
            eprintln!(
                "optimise gain: {:<28} MER {}  C/N {}  {:.1} dBFS{}",
                p.gains.to_string(),
                p.mer.map_or("  -  ".into(), |m| format!("{m:5.1}")),
                p.cn.map_or("  -  ".into(), |c| format!("{c:5.1}")),
                p.rms_dbfs,
                if p.spoiled { "  (clipping / too hot)" } else { "" }
            );
        }
        match action {
            gain_opt::Action::Wait => self.opt = Some(o),
            gain_opt::Action::Apply(g) => {
                self.set_opt_gains(g);
                self.opt = Some(o);
            }
            gain_opt::Action::Finish(found) => {
                let note = match (found, o.best()) {
                    (Some(g), Some(p)) => {
                        self.set_opt_gains(g);
                        let figure = o.figure(&p).map_or(String::new(), |f| {
                            format!("{} {f:.1} dB at ", if o.uses_mer() { "MER" } else { "C/N" })
                        });
                        format!(
                            "{g}: {figure}{:.0} dBFS, {} settings tried",
                            p.rms_dbfs,
                            o.done()
                        )
                    }
                    _ => {
                        self.set_opt_gains(o.original);
                        "no usable signal in the channel (C/N under 3 dB, or clipping at every setting): the old gains are back".into()
                    }
                };
                eprintln!("optimise gain: {note}");
                self.opt_note = Some(note);
            }
        }
    }

    fn save_prefs(&self) {
        if !self.saving {
            return;
        }
        prefs::set_many(&[
            ("source", self.kind.key().into()),
            (
                "file",
                self.file
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            ),
            ("decoder", self.factory().info().id.into()),
            ("centre", format!("{:.0}", self.center_hz)),
            ("chan", format!("{:.0}", self.chan_hz)),
            ("zoom", format!("{:.3}", self.zoom)),
            ("vfo_rail", if self.rail_open { "1" } else { "0" }.into()),
            ("volume", format!("{:.2}", self.volume)),
            ("hackrf_rate", format!("{:.0}", self.hack_rate)),
            ("hackrf_amp", if self.hack_amp { "1" } else { "0" }.into()),
            ("hackrf_lna", self.hack_lna.to_string()),
            ("hackrf_vga", self.hack_vga.to_string()),
            ("airspy_lna", self.air_lna.to_string()),
            ("airspy_mix", self.air_mix.to_string()),
            ("airspy_vga", self.air_vga.to_string()),
            ("airspy_mode", if self.air_linear { "linearity" } else { "sensitivity" }.into()),
            ("airspy_step", self.air_step.to_string()),
            ("rtlsdr_agc", if self.rtl_agc { "1" } else { "0" }.into()),
            ("rtlsdr_gain", self.rtl_db10.to_string()),
            ("rtlsdr_ppm", format!("{:.1}", self.rtl_ppm)),
        ]);
    }

    /// Take what the host has made since the last frame.
    /// A name for what is received (recordings are named after it).
    fn stream_name(&self) -> String {
        format!("{}_{:.3}MHz", self.factory().info().id, self.chan_hz / 1e6)
    }

    fn poll(&mut self) {
        let name = self.stream_name();
        let Some(h) = &self.host else { return };
        self.tv.poll(h, &name);
        for (name, row) in h.take_waterfall_rows() {
            // These rows are the selected VFO's.
            if self.minis_owner.is_none() {
                self.minis_owner = self.slots.get(self.cur).map(|s| s.n);
            }
            let i = match self.minis.iter().position(|m| m.name == name) {
                Some(i) => i,
                None => {
                    // Its rows are set each frame to its height in pixels.
                    let hist = History::default();
                    self.minis.push(MiniState {
                        name,
                        hist,
                        ring: RingImage::default(),
                        low_hz: 0.0,
                        span_hz: 0.0,
                        marks: Vec::new(),
                    });
                    self.minis.len() - 1
                }
            };
            let m = &mut self.minis[i];
            m.low_hz = row.low_hz;
            m.span_hz = row.span_hz;
            m.marks = row.marks;
            m.hist.push(row.db);
        }
        for row in h.take_rows() {
            if row.sample_rate != self.row_rate || row.center_hz != self.row_center {
                self.smooth.clear();
            }
            self.row_center = row.center_hz;
            self.row_rate = row.sample_rate;
            if self.smooth.len() != row.db.len() {
                self.smooth = row.db.clone();
            } else {
                for (s, &d) in self.smooth.iter_mut().zip(&row.db) {
                    *s += 0.3 * (d - *s);
                }
            }
            self.history.push_at(row.db, row.center_hz, row.sample_rate);
        }
        self.stats = h.stats();
        self.view = h.view();
        // The other VFOs: a look every 15 frames for the rail. What their
        // panels and waterfalls sent is dropped: those belong to the selected
        // one, and the host keeps only a few seconds anyway.
        let look = self.frames.is_multiple_of(15);
        for (i, s) in self.slots.iter_mut().enumerate() {
            let Some(v) = s.vid.filter(|_| i != self.cur) else { continue };
            drop(h.take_vfo_custom(v));
            drop(h.take_vfo_waterfall_rows(v));
            if look && let Some(view) = h.vfo_view(v) {
                s.view = view;
            }
        }
        let msgs = h.take_custom();
        if let Some(Some(p)) = self.panels.get_mut(self.dec) {
            for m in &msgs {
                p.receive(m.as_ref());
            }
        }
        if !self.live && self.stats.center_hz != 0.0 {
            self.center_hz = self.stats.center_hz;
        }
    }

    fn entries(&self) -> Vec<Entry> {
        let rate = self.rate();
        declab_decoders::ALL
            .iter()
            .map(|f| {
                let i = f.info();
                let spec = f.input(rate);
                let r = spec.sample_rate.map_or("source rate".to_string(), |r| {
                    if r >= 1e6 {
                        format!("{:.3} MS/s", r / 1e6)
                    } else {
                        format!("{:.0} kS/s", r / 1e3)
                    }
                });
                let bw = if spec.bandwidth_hz >= 1e6 {
                    format!("{:.1} MHz", spec.bandwidth_hz / 1e6)
                } else {
                    format!("{:.1} kHz", spec.bandwidth_hz / 1e3)
                };
                Entry {
                    id: i.id,
                    label: i.name,
                    family: i.family,
                    input: format!("{r} · {bw}"),
                    summary: i.summary,
                }
            })
            .collect()
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let colour = family_colour(self.factory().info().family);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let mut hz = self.chan_hz;
            if freq_display::show(ui, &mut hz, 0.0, 6e9, 40.0) {
                self.tune_channel(hz);
            }
            ui.add_space(14.0);
            ui.vertical(|ui| {
                ui.horizontal_wrapped(|ui| {
                    let running = self.host.is_some();
                    ui.label(RichText::new("source").color(Color32::GRAY));
                    let before = self.kind;
                    egui::ComboBox::from_id_salt("source").selected_text(self.kind.label()).show_ui(ui, |ui| {
                        for k in [SourceKind::Airspy, SourceKind::HackRf, SourceKind::RtlSdr, SourceKind::File] {
                            ui.selectable_value(&mut self.kind, k, k.label());
                        }
                    });
                    if self.kind != before {
                        self.stop();
                        self.save_prefs();
                    }
                    match self.kind {
                        SourceKind::File => {
                            let name = self
                                .file
                                .as_ref()
                                .and_then(|p| p.file_name())
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "no file".into());
                            ui.label(RichText::new(name).color(Color32::WHITE));
                            if ui.button("Open…").clicked()
                                && let Some(p) = rfd::FileDialog::new()
                                    .add_filter("IQ capture", &["cs8", "cs16", "cf32", "cu8", "wav", "iq"])
                                    .pick_file()
                            {
                                self.file = Some(p);
                                self.opts.rate = None;
                                self.opts.freq = None;
                                self.start();
                            }
                        }
                        SourceKind::HackRf => {
                            ui.label(RichText::new("rate").color(Color32::GRAY));
                            let before = self.hack_rate;
                            egui::ComboBox::from_id_salt("hackrf_rate")
                                .selected_text(format!("{:.2} MS/s", self.hack_rate / 1e6))
                                .show_ui(ui, |ui| {
                                    for r in [2e6, 4e6, 8e6, 10e6, 12.5e6, 16e6, 20e6] {
                                        ui.selectable_value(&mut self.hack_rate, r, format!("{:.2} MS/s", r / 1e6));
                                    }
                                });
                            if self.hack_rate != before && running {
                                self.start();
                            }
                            let (mut start_opt, mut cancel_opt) = (false, false);
                            ui.menu_button(
                                format!("{}LNA {} · VGA {}", if self.hack_amp { "AMP · " } else { "" }, self.hack_lna, self.hack_vga),
                                |ui| {
                                    // The optimiser is moving the gains: not the sliders too.
                                    let mut ch = false;
                                    ui.add_enabled_ui(self.opt.is_none(), |ui| {
                                        ch |= ui.checkbox(&mut self.hack_amp, "RF amp (+14 dB)").changed();
                                        ch |= ui.add(egui::Slider::new(&mut self.hack_lna, 0..=40).step_by(8.0).text("LNA dB")).changed();
                                        ch |= ui.add(egui::Slider::new(&mut self.hack_vga, 0..=62).step_by(2.0).text("VGA dB")).changed();
                                    });
                                    if ch {
                                        self.apply_gains();
                                    }
                                    (start_opt, cancel_opt) = self.optimiser_menu(ui);
                                },
                            );
                            if start_opt {
                                self.start_optimise();
                            }
                            if cancel_opt {
                                self.cancel_optimise();
                            }
                        }
                        SourceKind::Airspy => {
                            ui.label(RichText::new("rate").color(Color32::GRAY));
                            ui.label(RichText::new("10.00 MS/s").color(Color32::WHITE));
                            let (mut start_opt, mut cancel_opt) = (false, false);
                            let mode = if self.air_linear { "linearity" } else { "sensitivity" };
                            let stages = (self.air_lna, self.air_mix, self.air_vga);
                            // The optimiser may have left the stages between two steps.
                            let title = if stages == air_preset(self.air_linear, self.air_step) {
                                format!("gain {} · {mode}", self.air_step)
                            } else {
                                format!("LNA {} · MIX {} · VGA {}", stages.0, stages.1, stages.2)
                            };
                            ui.menu_button(title, |ui| {
                                // The optimiser is moving the gains: not the slider too.
                                let busy = self.opt.is_some();
                                let mut ch = false;
                                ui.add_enabled_ui(!busy, |ui| {
                                    ui.horizontal(|ui| {
                                        ch |= ui.selectable_value(&mut self.air_linear, false, "Sensitivity").changed();
                                        ch |= ui.selectable_value(&mut self.air_linear, true, "Linearity").changed();
                                    });
                                    ch |= ui.add(egui::Slider::new(&mut self.air_step, 0..=21).text("gain")).changed();
                                    ui.label(
                                        RichText::new(format!("LNA {} · mixer {} · VGA {}", stages.0, stages.1, stages.2))
                                            .color(Color32::GRAY),
                                    );
                                });
                                if ch {
                                    (self.air_lna, self.air_mix, self.air_vga) = air_preset(self.air_linear, self.air_step);
                                    self.apply_gains();
                                }
                                (start_opt, cancel_opt) = self.optimiser_menu(ui);
                            });
                            if start_opt {
                                self.start_optimise();
                            }
                            if cancel_opt {
                                self.cancel_optimise();
                            }
                        }
                        SourceKind::RtlSdr => {
                            ui.label(RichText::new("rate").color(Color32::GRAY));
                            ui.label(RichText::new("2.048 MS/s").color(Color32::WHITE));
                            let title = if self.rtl_agc {
                                "gain AGC".to_string()
                            } else {
                                format!("gain {:.1} dB", f64::from(self.rtl_gains().db10) / 10.0)
                            };
                            ui.menu_button(title, |ui| {
                                let mut ch = ui.checkbox(&mut self.rtl_agc, "Tuner AGC").changed();
                                // The slider walks the R820T's own steps, by index.
                                let table = &declab_io::rtlsdr_proto::GAINS_DB10;
                                let mut i = table.iter().position(|&g| g == self.rtl_gains().db10).unwrap_or(0);
                                ui.add_enabled_ui(!self.rtl_agc, |ui| {
                                    let r = ui.add(
                                        egui::Slider::new(&mut i, 0..=table.len() - 1)
                                            .custom_formatter(|v, _| format!("{:.1}", f64::from(table[v as usize]) / 10.0))
                                            .text("dB"),
                                    );
                                    if r.changed() {
                                        self.rtl_db10 = table[i];
                                        ch = true;
                                    }
                                });
                                ch |= ui
                                    .add(egui::DragValue::new(&mut self.rtl_ppm).range(-200.0..=200.0).speed(0.1).suffix(" ppm"))
                                    .changed();
                                if ch {
                                    self.apply_gains();
                                }
                            });
                        }
                    }
                    if self.kind != SourceKind::File {
                        self.bias_tee_ui(ui);
                    }
                    ui.label(RichText::new("centre").color(Color32::GRAY));
                    ui.label(RichText::new(format!("{:.3} MHz", self.center_hz / 1e6)).color(Color32::WHITE));
                    let rec = self.stats.recording.as_ref().map(|(_, b)| *b);
                    if running {
                        let label = match rec {
                            Some(b) => RichText::new(format!("● {:.0} MB", b as f64 / 1e6)).color(panels::RED),
                            None => RichText::new("● Record").color(panels::RED),
                        };
                        if ui.button(label).clicked()
                            && let Some(h) = &self.host
                        {
                            if rec.is_some() {
                                h.record(None);
                            } else {
                                let name = format!(
                                    "declab_{:.3}MHz_{}Msps.cs8",
                                    self.center_hz / 1e6,
                                    panels::value(self.rate() / 1e6)
                                );
                                if let Some(p) = rfd::FileDialog::new().set_file_name(name).save_file() {
                                    h.record(Some(p));
                                }
                            }
                        }
                        if ui.button("■ Stop").clicked() {
                            self.stop();
                        }
                    } else if ui.button(RichText::new("▶ Start").color(panels::GREEN)).clicked() {
                        self.start();
                    }
                });
                ui.horizontal(|ui| {
                    panels::level_bar(ui, self.stats.rms_dbfs, self.host.is_some() && self.stats.samples > 0);
                    let f = self.factory().info();
                    let b = egui::Button::new(RichText::new(format!("●  {}  ▾ decoder", f.name)).color(colour))
                        .stroke(egui::Stroke::new(1.0, colour));
                    if ui.add(b).clicked() {
                        let c = ui.ctx().content_rect().center();
                        self.wheel = Some(Wheel::new(c, self.dec, ui.ctx().cumulative_frame_nr()));
                    }
                    ui.label(RichText::new("or middle-click the band").size(12.0).color(Color32::GRAY));
                    ui.add_space(12.0);
                    if ui.button("Settings").clicked() {
                        self.settings_open = !self.settings_open;
                        self.settings_page = self.settings_page.or(Some(self.dec));
                    }
                    if self.makes_sound() {
                        ui.add_space(16.0);
                        ui.label(RichText::new("volume").color(Color32::GRAY));
                        self.volume_ui(ui);
                    }
                });
            });
        });
        ui.add_space(4.0);
    }

    fn handle_band(&mut self, actions: Vec<band::Action>, ctx: &egui::Context) {
        for a in actions {
            match a {
                band::Action::Tune(hz) => self.tune_channel(hz),
                band::Action::Select(i) => self.select_slot(i),
                band::Action::Add(hz) => self.add_vfo(Some(hz)),
                band::Action::Bandwidth(bw) => {
                    self.bandwidth_hz = bw.clamp(500.0, self.rate());
                    if let Some(h) = &self.host {
                        h.set_channel_at(self.chan_hz, self.bandwidth_hz);
                    }
                }
                band::Action::MoveChannel(hz) => {
                    self.chan_hz = (hz / 100.0).round() * 100.0;
                    if let Some(h) = &self.host {
                        h.set_channel_at(self.chan_hz, self.bandwidth_hz);
                    }
                }
                band::Action::ChannelDragEnd => {
                    // Settle on the raster as a click would.
                    let hz = self.chan_hz;
                    self.tune_channel(hz);
                }
                band::Action::Pan(d) => {
                    self.dragging = true;
                    self.view_hz += d;
                    if self.live {
                        // The radio follows the view, a retune at a time.
                        if self.last_retune.elapsed() >= RETUNE_EVERY {
                            let target = self.view_hz;
                            self.retune(target);
                        }
                    } else {
                        self.clamp_view();
                    }
                }
                band::Action::DragEnd => {
                    self.dragging = false;
                    if self.live {
                        let target = self.view_hz;
                        self.retune(target);
                        self.clamp_view();
                    }
                    self.save_prefs();
                }
                band::Action::OpenWheel(p) => {
                    self.wheel = Some(Wheel::new(p, self.dec, ctx.cumulative_frame_nr()));
                }
                band::Action::Zoom {
                    factor,
                    anchor_hz,
                    frac,
                } => {
                    let max_zoom = (self.rate() / MIN_SPAN_HZ).max(1.0);
                    self.zoom = (self.zoom * factor).clamp(1.0, max_zoom);
                    let span = self.span();
                    self.view_hz = anchor_hz - (frac - 0.5) * span;
                    self.clamp_view();
                }
            }
        }
    }

    /// The wait to ask egui for before the next frame. egui's repaint timer
    /// does not keep the interval it is given (a 16 ms request drew a few
    /// hundred frames a second here, a core and more), so the wait is steered
    /// by the gaps between frames actually seen, to hold `FPS`. DecDVB-T's
    /// way (same author).
    fn pace(&mut self) -> Duration {
        // DECLAB_FPS: another rate, for measuring what a frame costs.
        let fps = std::env::var("DECLAB_FPS")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(FPS);
        let target = 1000.0 / fps;
        let now = Instant::now();
        if let Some(prev) = self.last_frame.replace(now) {
            let gap = now.duration_since(prev).as_secs_f64() * 1e3;
            // A long gap is a pause (input, nothing running), not a measurement.
            if gap < 4.0 * target {
                self.delay_ms += 0.5 * (target - gap);
            }
        }
        self.delay_ms = self.delay_ms.clamp(1.0, 4.0 * target);
        Duration::from_secs_f64(self.delay_ms / 1e3)
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.opts.screenshot.clone() else {
            return;
        };
        if self.cpu_mark.is_none() && self.started.elapsed().as_secs_f64() >= 3.0 {
            self.cpu_mark = Some((declab_engine::meter::process_cpu_seconds(), Instant::now()));
        }
        if !self.shot_sent && self.started.elapsed().as_secs_f64() >= self.opts.after {
            if let Some((c0, t0)) = self.cpu_mark {
                let dt = t0.elapsed().as_secs_f64().max(1e-3);
                self.cpu_avg =
                    Some((100.0 * (declab_engine::meter::process_cpu_seconds() - c0) / dt) as f32);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            self.shot_sent = true;
        }
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        let Some(image) = image else { return };
        let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
        let (w, h) = (image.size[0] as u32, image.size[1] as u32);
        match image::save_buffer(&path, &rgba, w, h, image::ColorType::Rgba8) {
            Ok(()) => eprintln!(
                "screenshot saved to {} ({:.0} fps, app CPU {:.0} % average from 3 s, decoder {:.0} %, {}); per frame, ms: poll {:.2}, bottom {:.2}, band {:.2}, windows {:.2}, top {:.2}",
                path.display(),
                self.frames as f64 / self.started.elapsed().as_secs_f64(),
                self.cpu_avg.unwrap_or(self.cpu.percent),
                self.view.cpu_percent,
                self.stats.describe,
                self.frame_cost[0].as_secs_f64() * 1e3 / self.frames.max(1) as f64,
                self.frame_cost[1].as_secs_f64() * 1e3 / self.frames.max(1) as f64,
                self.frame_cost[2].as_secs_f64() * 1e3 / self.frames.max(1) as f64,
                self.frame_cost[3].as_secs_f64() * 1e3 / self.frames.max(1) as f64,
                self.frame_cost[4].as_secs_f64() * 1e3 / self.frames.max(1) as f64
            ),
            Err(e) => eprintln!("cannot save screenshot {}: {e}", path.display()),
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frames += 1;
        let mut mark = Instant::now();
        let mut lap = |i: usize, costs: &mut [Duration; 5]| {
            costs[i] += mark.elapsed();
            mark = Instant::now();
        };
        self.poll();
        self.step_optimiser();
        lap(0, &mut self.frame_cost);
        self.cpu.poll();
        if self.opts.wheel
            && self.wheel.is_none()
            && self.started.elapsed() > Duration::from_millis(300)
        {
            self.opts.wheel = false;
            let c = ctx.content_rect().center() - egui::vec2(0.0, 120.0);
            self.wheel = Some(Wheel::new(c, self.dec, ctx.cumulative_frame_nr()));
        }

        let info = self.factory().info();
        let colour = family_colour(info.family);
        let running = self.host.as_ref().is_some_and(|h| h.running());

        let rows: Vec<vfo::Row> = self
            .slots
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let (dec, chan, view) = if i == self.cur {
                    (self.dec, self.chan_hz, &self.view)
                } else {
                    (s.dec, s.chan_hz, &s.view)
                };
                vfo::Row {
                    n: s.n,
                    colour: s.colour,
                    decoder: declab_decoders::ALL[dec].info().name,
                    freq_hz: chan,
                    status: vfo::status_of(view, running),
                    lock: view.lock,
                    cpu: view.cpu_percent,
                    level_db: vfo::level_of(view),
                    muted: s.muted,
                    primary: i == 0,
                    running,
                }
            })
            .collect();
        let rail_was = self.rail_open;
        let acts = vfo::rail(ui, &rows, self.cur, &mut self.rail_open, MAX_VFOS);
        if rail_was != self.rail_open {
            self.save_prefs();
        }
        for a in acts {
            match a {
                vfo::Act::Select(i) => self.select_slot(i),
                vfo::Act::ToggleMute(i) => self.toggle_mute(i),
                vfo::Act::Remove(i) => self.remove_slot(i),
                vfo::Act::Add => self.add_vfo(None),
            }
        }
        egui::Panel::top("top")
            .frame(
                egui::Frame::side_top_panel(&ctx.global_style())
                    .fill(Color32::from_rgb(25, 25, 25)),
            )
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| {
            panels::status(
                ui,
                &self.stats,
                &self.view,
                running,
                self.cpu.percent,
                self.error.as_deref(),
            );
        });
        lap(4, &mut self.frame_cost);
        let skip = std::env::var("DECLAB_SKIP").unwrap_or_default();
        egui::Panel::bottom("decoder")
            .resizable(false)
            .exact_size(330.0)
            .show(ui, |ui| {
                if skip.contains("bottom") {
                    return;
                }
                ui.add_space(6.0);
                panels::card(ui, info.name, colour, &self.view, running);
                ui.add_space(6.0);
                // The scope and the card on the left; the decoder's large
                // waterfall (WFM: its multiplex) takes the rest. Settings are in
                // the Settings window.
                let w = ui.available_width();
                let h = ui.available_height();
                let gap = ui.spacing().item_spacing.x;
                let widths = [w * 0.5 - gap, w * 0.5 - gap];
                // The decoder's waterfalls, a row to a physical pixel (the small
                // one's height is fixed, the large one's the room it reported),
                // their textures brought up to date.
                let ppp = ctx.pixels_per_point();
                let scopes: Vec<&str> = self.view.scopes.iter().map(|(n, _)| *n).collect();
                let big_rows = self.big_rows;
                let minis: Vec<panels::Mini> = self
                    .minis
                    .iter_mut()
                    .filter_map(|m| {
                        let small = scopes.contains(&m.name);
                        m.hist.set_capacity(if small {
                            (panels::MINI_H * ppp).round() as usize
                        } else {
                            big_rows
                        });
                        m.ring.update(&ctx, &m.hist).map(|(tex, uv)| panels::Mini {
                            rows: m.hist.capacity(),
                            name: m.name,
                            tex,
                            uv,
                            low_hz: m.low_hz,
                            span_hz: m.span_hz,
                            marks: m.marks.clone(),
                        })
                    })
                    .collect();
                let stream_name = self.stream_name();
                let has_big = minis
                    .iter()
                    .any(|m| !self.view.scopes.iter().any(|(n, _)| *n == m.name));
                let mut picked = None;
                ui.horizontal_top(|ui| {
                    let chan = self.chan_hz;
                    ui.allocate_ui_with_layout(
                        egui::vec2(widths[0], h),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            panels::boxed(ui, |ui| {
                                let pick = panels::scope_and_card(
                                    ui,
                                    &self.view,
                                    colour,
                                    &mut self.scope,
                                    info.name,
                                    chan,
                                    &minis,
                                    self.selected_service,
                                    // The services go under the card when the right
                                    // column holds a waterfall; else they have it.
                                    has_big,
                                );
                                if let Some(id) = pick {
                                    picked = Some(panels::Pick::Service(id));
                                }
                            })
                        },
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(widths[1], h),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            panels::boxed(ui, |ui| {
                                // A transport stream's services (DVB), else a decoder's
                                // large waterfall (WFM: its multiplex), else the
                                // services on their own.
                                // Digital voice: talkgroups and radios, or its
                                // data as hex, LRRP locations or IP.
                                let voice = info.family == declab_api::Family::Voice;
                                if voice || info.family == declab_api::Family::Data {
                                    match self.data_tabs.bar(ui, &self.view, voice) {
                                        data_tabs::Tab::Heard => {
                                            if let Some(p) =
                                                panels::services(ui, &self.view, self.selected_service)
                                            {
                                                picked = Some(p);
                                            }
                                        }
                                        data_tabs::Tab::Hex => self.data_tabs.hex(ui, &self.view),
                                        data_tabs::Tab::Lrrp => self.data_tabs.lrrp(ui, &self.view),
                                        data_tabs::Tab::Ip => {
                                            if let Some(host) = &self.host {
                                                let (n, b) = (self.view.ip_packets, self.view.ip_bytes);
                                                self.tv.ip_ui(ui, host, &stream_name, n, b);
                                                ui.add_space(4.0);
                                            }
                                            self.data_tabs.ip(ui, &self.view);
                                        }
                                    }
                                    return;
                                }
                                if self.tv.active()
                                    && let Some(host) = &self.host
                                {
                                    self.tv.services_ui(ui, host, &stream_name);
                                    return;
                                }
                                // IP datagrams (GSE) have no services: a count and a PCAP.
                                if self.view.ip_packets > 0
                                    && let Some(host) = &self.host
                                {
                                    let (n, b) = (self.view.ip_packets, self.view.ip_bytes);
                                    self.tv.ip_ui(ui, host, &stream_name, n, b);
                                    return;
                                }
                                match minis
                                    .iter()
                                    .find(|m| !self.view.scopes.iter().any(|(n, _)| *n == m.name))
                                {
                                    Some(m) => self.big_rows = panels::big_waterfall(ui, m).max(64),
                                    None => {
                                        if let Some(p) =
                                            panels::services(ui, &self.view, self.selected_service)
                                        {
                                            picked = Some(p);
                                        }
                                    }
                                }
                            })
                        },
                    );
                });
                match picked {
                    Some(panels::Pick::Service(id)) => {
                        self.selected_service = Some(id);
                        if let Some(h) = &self.host {
                            h.command(declab_api::Command::SelectService(id));
                        }
                    }
                    Some(panels::Pick::Tune(hz)) => self.tune_exact(hz),
                    None => {}
                }
            });

        // The Settings window.
        if self.settings_open {
            let mut open = true;
            let mut page = self.settings_page;
            let ch = settings::show(
                &ctx,
                &mut open,
                &mut page,
                settings::View {
                    decoders: declab_decoders::ALL,
                    panels: &mut self.panels,
                    current: self.dec,
                    bandwidth_hz: &mut self.bandwidth_hz,
                    volume: &mut self.volume,
                },
            );
            self.settings_open = open;
            self.settings_page = page;
            let mut touched = Vec::new();
            for (i, c) in ch.commands {
                // Only the running decoder hears them; the panel keeps them
                // for the others, which start with them.
                if i == self.dec
                    && let Some(h) = &self.host
                {
                    h.command(c);
                }
                touched.push(i);
            }
            touched.dedup();
            for i in touched {
                self.save_settings(i);
            }
            if let Some(h) = &self.host {
                if ch.bandwidth {
                    h.set_channel_at(self.chan_hz, self.bandwidth_hz);
                }
                if ch.volume {
                    h.set_volume(self.effective_volume());
                }
            }
            if ch.volume {
                self.save_prefs();
            }
        }

        lap(1, &mut self.frame_cost);
        let mut actions = Vec::new();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                self.wf_rows =
                    band::waterfall_rows(ui.available_height(), ctx.pixels_per_point()).max(64);
                self.history.set_capacity(self.wf_rows);
                let px = f64::from(ui.available_width() * ctx.pixels_per_point());
                self.follow_zoom(px, ctx.input(|i| i.max_texture_side));
                let tex = self.ring.update(&ctx, &self.history);
                let sel = self.slots.get(self.cur);
                let label = format!("{} {} {:.4}", sel.map_or(1, |s| s.n), info.name, self.chan_hz / 1e6);
                let chan_colour = sel.map_or(colour, |s| s.colour);
                let idle = match (&self.host, &self.error) {
                    (_, Some(e)) => e.clone(),
                    (None, _) => "press Start, or open an IQ file".to_string(),
                    (Some(_), _) => "waiting for samples…".to_string(),
                };
                let inp = band::BandInput {
                    spectrum_db: &self.smooth,
                    row_center_hz: self.row_center,
                    row_rate: self.row_rate,
                    wf_center_hz: self.history.anchor().map_or(self.row_center, |a| a.0),
                    wf_rate: self.history.anchor().map_or(self.row_rate, |a| a.1),
                    view_center_hz: self.view_hz,
                    view_span_hz: self.span(),
                    levels: self.history.levels(),
                    waterfall: tex,
                    channel: Some((self.chan_hz, self.bandwidth_hz, chan_colour, label)),
                    others: self
                        .slots
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != self.cur)
                        .map(|(i, s)| band::Other {
                            slot: i,
                            hz: s.chan_hz,
                            bandwidth_hz: s.bandwidth_hz,
                            colour: s.colour,
                            label: format!("{} {}", s.n, declab_decoders::ALL[s.dec].info().name),
                        })
                        .collect(),
                    moving: self
                        .dragging
                        .then(|| format!("moving the band · centre {:.3} MHz", self.view_hz / 1e6)),
                    inert: self.wheel.is_some(),
                    idle_text: &idle,
                };
                if !skip.contains("band") {
                    actions = band::show(ui, &inp);
                }
            });
        self.handle_band(actions, &ctx);

        let entries = if self.wheel.is_some() {
            self.entries()
        } else {
            Vec::new()
        };
        if let Some(w) = &mut self.wheel {
            match w.show(&ctx, &entries) {
                Some(Outcome::Pick(i)) => {
                    self.wheel = None;
                    if i != self.dec {
                        self.choose_decoder(i);
                    }
                }
                Some(Outcome::Cancel) => self.wheel = None,
                None => {}
            }
        }

        lap(2, &mut self.frame_cost);
        // The player, guide and TS windows.
        let name = self.stream_name();
        if let Some(h) = &self.host {
            self.tv.windows(&ctx, h, &name);
        }

        lap(3, &mut self.frame_cost);
        self.handle_screenshot(&ctx);
        // Stopped, the window only redraws on input (it costs a fifth of a
        // core to draw it 30 times a second for nothing).
        // 60 frames a second while running: decoders report 60 times a second.
        if self.host.is_some()
            || self.opts.screenshot.is_some()
            || self.tv.player.wants_fps().is_some()
        {
            let wait = self.pace();
            ctx.request_repaint_after(wait);
        }
    }
}
