// made by CasualArclamp
//! The parts of the window below the band: the decoder's card, its scope
//! and readings, its own settings panel, its services and text, and the
//! status bar.

use declab_api::{Iq, Lock};
use declab_engine::{DecoderView, Stats};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, RichText, Sense, Stroke, Ui, pos2, vec2,
};

pub const GREEN: Color32 = Color32::from_rgb(110, 230, 130);
pub const AMBER: Color32 = Color32::from_rgb(255, 176, 40);
pub const RED: Color32 = Color32::from_rgb(255, 138, 128);
const GREY: Color32 = Color32::from_gray(154);
const BOX: Color32 = Color32::from_rgb(24, 24, 24);
/// The scope's side, points.
const SCOPE: f32 = 150.0;

/// Format a reading: few digits for big numbers, more for small ones.
pub fn value(v: f64) -> String {
    let a = v.abs();
    if a >= 1000.0 || (a > 0.0 && a < 0.01) {
        if a >= 1e6 || a < 0.01 { format!("{v:.2e}") } else { format!("{v:.0}") }
    } else if a >= 100.0 || v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

/// The card: the decoder's name, its lock and a one-line summary.
pub fn card(ui: &mut Ui, name: &str, colour: Color32, view: &DecoderView, running: bool) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(name).strong().color(colour));
        let (state, c) = match (running, view.lock) {
            (false, _) => ("stopped", GREY),
            (true, Lock::Locked) => ("locked", GREEN),
            (true, Lock::Synced) => ("synced", AMBER),
            (true, Lock::Searching) => ("searching", RED),
        };
        ui.label(RichText::new(state).color(c));
        let line: Vec<String> = view
            .measurements
            .iter()
            .take(4)
            .map(|m| format!("{} {}{}{}", m.name.to_lowercase(), value(m.value), if m.unit.is_empty() { "" } else { " " }, m.unit))
            .collect();
        if running && !line.is_empty() {
            ui.label(RichText::new(line.join(" · ")).color(GREEN));
        }
        if let Some(s) = view.services.first().filter(|s| !s.name.is_empty()) {
            ui.label(RichText::new(format!("· {}", s.name)).color(Color32::WHITE));
        }
    });
}

/// A titled box filling its share of the row.
pub fn boxed(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::NONE
        .fill(BOX)
        .stroke(Stroke::new(1.0, Color32::from_gray(42)))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.set_min_height(ui.available_height() - 22.0);
            ui.set_min_width(ui.available_width());
            add(ui)
        });
}

/// A decoder's small waterfall, ready to draw: its texture and the texture
/// coordinates that put the newest row on top.
pub struct Mini {
    pub name: &'static str,
    /// Rows the texture holds: drawn one to a physical pixel.
    pub rows: usize,
    pub tex: egui::TextureId,
    pub uv: egui::Rect,
    pub low_hz: f32,
    pub span_hz: f32,
    pub marks: Vec<(f32, &'static str)>,
}

/// The height of a small waterfall under the scope, points.
pub const MINI_H: f32 = 64.0;

/// The constellation scope (with its choice of scopes above it) and the
/// decoder's card beside it, in the style of DecDVB-T's and DecSAT's cards.
///
/// `which` names the scope shown: "" for the channel IQ, else one of the
/// decoder's own (`DecoderView::scopes`).
// Everything the box shows comes in as arguments; a struct would only rename them.
#[allow(clippy::too_many_arguments)]
pub fn scope_and_card(
    ui: &mut Ui,
    view: &DecoderView,
    colour: Color32,
    which: &mut String,
    name: &str,
    freq_hz: f64,
    minis: &[Mini],
    selected: Option<u32>,
    services_here: bool,
) -> Option<u32> {
    let mut pick = None;
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                let mut pick = |ui: &mut Ui, key: &str, label: &str| {
                    let on = which.as_str() == key;
                    let text = RichText::new(label).size(11.0).color(if on { Color32::WHITE } else { GREY });
                    if ui.selectable_label(on, text).clicked() {
                        *which = key.to_string();
                    }
                };
                pick(ui, "", "channel IQ");
                for (n, _) in &view.scopes {
                    pick(ui, n, n);
                }
            });
            let pts = view.scopes.iter().find(|(n, _)| *n == which.as_str()).map_or(&view.constellation, |(_, p)| p);
            scope(ui, pts, colour, SCOPE);
            // The waterfall of the scope shown, if the decoder makes one
            // (RDS: the subcarrier at baseband, its two sidebands about a
            // null at 0 Hz).
            if let Some(m) = minis.iter().find(|m| m.name == which.as_str()) {
                mini_waterfall(ui, m);
            }
        });
        ui.add_space(10.0);
        ui.vertical(|ui| {
            let fallback;
            let card = match &view.card {
                Some(c) => c,
                None => {
                    fallback = card_from_measurements(name, view);
                    &fallback
                }
            };
            egui::ScrollArea::vertical().id_salt("card").max_height(ui.available_height()).show(ui, |ui| {
                draw_card(ui, card, colour, freq_hz, view.cpu_percent);
                // The services under the card, a row each; the decoder's text
                // too when its card does not carry it.
                if services_here && !view.services.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new("services").size(11.0).color(GREY));
                    for s in &view.services {
                        let label = if s.name.is_empty() { format!("[{}]", s.id) } else { s.name.clone() };
                        let on = selected == Some(s.id) || (selected.is_none() && view.services.len() == 1);
                        let text = RichText::new(format!("{label}   {}", s.kind)).size(13.0);
                        if ui.selectable_label(on, text).clicked() {
                            pick = Some(s.id);
                        }
                    }
                }
                if services_here && view.card.as_ref().is_none_or(|c| c.text.is_empty()) {
                    for t in view.text.values() {
                        ui.add_space(4.0);
                        ui.add(egui::Label::new(RichText::new(t).size(13.0).color(Color32::WHITE)).wrap());
                    }
                }
            });
        });
    });
    pick
}

/// A card for a decoder that sends none: its measurements as details.
fn card_from_measurements(name: &str, view: &DecoderView) -> declab_api::Card {
    declab_api::Card {
        title: name.to_string(),
        headline: match view.lock {
            Lock::Locked => "locked",
            Lock::Synced => "synced",
            Lock::Searching => "searching",
        }
        .to_string(),
        gauge: None,
        details: view
            .measurements
            .iter()
            .map(|m| (m.name.to_string(), format!("{} {}", value(m.value), m.unit)))
            .collect(),
        text: Vec::new(),
    }
}

/// A margin's colour: red below 0 dB, amber within 1 dB, else green (as in
/// DecDVB-T's card).
fn health(margin: f32) -> Color32 {
    if margin < 0.0 {
        Color32::from_rgb(230, 110, 110)
    } else if margin < 1.0 {
        Color32::from_rgb(240, 200, 80)
    } else {
        Color32::from_rgb(110, 220, 110)
    }
}

fn draw_card(ui: &mut Ui, c: &declab_api::Card, colour: Color32, freq_hz: f64, cpu: f32) {
    let strong = Color32::WHITE;
    // The title bar: what is received, and where.
    let bar = egui::Frame::new()
        .fill(colour.gamma_multiply(0.15))
        .corner_radius(CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 })
        .inner_margin(egui::Margin { left: 8, right: 8, top: 3, bottom: 3 })
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(&c.title).size(14.0).strong().color(colour));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new("MHz").size(11.0).color(GREY));
                    ui.label(RichText::new(format!("{:.3}", freq_hz / 1e6)).size(16.0).color(strong));
                });
            });
        })
        .response;
    ui.painter().hline(bar.rect.x_range(), bar.rect.bottom(), Stroke::new(2.0, colour));
    ui.add_space(4.0);

    // The headline and the margin beside it.
    let margin = c.gauge.as_ref().and_then(|g| g.need_db.map(|n| ((g.value_db - n) * 10.0).round() / 10.0 + 0.0));
    ui.horizontal(|ui| {
        let empty = c.headline == "–" || c.headline.is_empty();
        let text = if empty { "–" } else { &c.headline };
        // A long headline ("TS1 TG 2350 ← 2341234", a last call with its
        // age) shrinks to leave room for the margin, then is cut.
        let room = ui.available_width() - if margin.is_some() { 90.0 } else { 0.0 };
        // Rust note: `WidgetText::into_galley` lays the label's own text out
        // (its font, unwrapped), so its width is what the label will take.
        let wide = egui::WidgetText::from(RichText::new(text).size(24.0))
            .into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body)
            .size()
            .x;
        let size = (24.0 * room / wide.max(1.0)).clamp(15.0, 24.0);
        ui.add_sized(
            vec2(room.max(0.0), 30.0),
            egui::Label::new(RichText::new(text).size(size).color(if empty { GREY } else { colour })).truncate(),
        );
        if let Some(m) = margin {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("{m:+5.1} dB")).size(17.0).color(health(m)));
            });
        }
    });

    // The gauge, ticked where the signal needs to be.
    if let Some(g) = &c.gauge {
        let (lo, hi) = g.range;
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
        let x_of = |db: f32| rect.left() + rect.width() * ((db - lo) / (hi - lo)).clamp(0.0, 1.0);
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(2), Color32::from_rgb(10, 10, 10));
        let fill = margin.map_or(Color32::from_rgb(100, 170, 255).gamma_multiply(0.8), health);
        p.rect_filled(egui::Rect::from_min_max(rect.min, pos2(x_of(g.value_db), rect.max.y)), CornerRadius::same(2), fill);
        if let Some(n) = g.need_db {
            let x = x_of(n);
            p.rect_filled(
                egui::Rect::from_min_max(pos2(x - 1.0, rect.top() - 3.0), pos2(x + 1.0, rect.bottom() + 3.0)),
                CornerRadius::ZERO,
                strong,
            );
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} {:.1} dB", g.label, g.value_db)).size(12.0).color(GREY));
            if let Some(n) = g.need_db {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{} {n:.1} dB", g.need_label)).size(12.0).color(GREY));
                });
            }
        });
    }

    // The details, two pairs a row, the decoder's CPU last.
    ui.add_space(4.0);
    let mut details: Vec<(String, String)> = c.details.clone();
    details.push(("CPU".into(), format!("{cpu:.0} %")));
    egui::Grid::new("card_details").num_columns(4).spacing([10.0, 3.0]).show(ui, |ui| {
        for pair in details.chunks(2) {
            for (k, v) in pair {
                ui.label(RichText::new(k).size(12.0).color(GREY));
                ui.label(RichText::new(v).size(12.0).color(strong));
            }
            ui.end_row();
        }
    });

    // The text, each line under its label.
    for (k, v) in &c.text {
        ui.add_space(4.0);
        ui.label(RichText::new(k).size(11.0).color(GREY));
        ui.add(egui::Label::new(RichText::new(v).size(13.0).color(strong)).wrap());
    }
}

/// A decoder waterfall's image with its marks (lines, labels along the top).
///
/// Drawn as the main waterfall is (DecDVB-T's way): its rows one to a
/// physical pixel, on a whole pixel. Spreading 96 rows over 64 points left
/// each row two thirds of a pixel, and the scroll shimmered as rows slid
/// across pixel boundaries.
fn waterfall_image(ui: &mut Ui, m: &Mini, width: f32, labels: bool) -> egui::Rect {
    let ppp = ui.ctx().pixels_per_point();
    let h = m.rows as f32 / ppp;
    let (got, _) = ui.allocate_exact_size(vec2(width, h + 1.0 / ppp), Sense::hover());
    let top = (got.top() * ppp).round() / ppp;
    let rect = egui::Rect::from_min_size(pos2(got.left(), top), vec2(width, h));
    let p = ui.painter_at(rect);
    p.image(m.tex, rect, m.uv, Color32::WHITE);
    let x_of = |hz: f32| rect.left() + (hz - m.low_hz) / m.span_hz.max(1.0) * rect.width();
    for (hz, label) in &m.marks {
        let x = x_of(*hz);
        if !rect.x_range().contains(x) {
            continue;
        }
        p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, Color32::from_white_alpha(45)));
        if labels && !label.is_empty() {
            p.text(pos2(x + 3.0, rect.top() + 2.0), Align2::LEFT_TOP, *label, FontId::monospace(10.0), Color32::from_gray(220));
        }
    }
    rect
}

fn mini_waterfall(ui: &mut Ui, m: &Mini) {
    ui.add_space(4.0);
    waterfall_image(ui, m, SCOPE, false);
    let (lo, hi) = (m.low_hz / 1e3, (m.low_hz + m.span_hz) / 1e3);
    ui.horizontal(|ui| {
        ui.set_width(SCOPE);
        ui.label(RichText::new(format!("{lo:+.0}")).size(10.0).color(GREY));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(format!("{hi:+.0} kHz")).size(10.0).color(GREY));
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add_space((SCOPE / 2.0 - 60.0).max(0.0));
                ui.label(RichText::new(format!("{} carrier", m.name)).size(10.0).color(GREY));
            });
        });
    });
}

/// A decoder's large waterfall (WFM's multiplex), filling its box. Returns
/// the rows (physical pixels) it has room for, for the next frame.
pub fn big_waterfall(ui: &mut Ui, m: &Mini) -> usize {
    ui.horizontal(|ui| {
        ui.label(RichText::new(m.name).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new("baseband, after the FM demodulator").size(11.0).color(GREY));
        });
    });
    let w = ui.available_width();
    let room = ((ui.available_height() - 26.0).max(40.0) * ui.ctx().pixels_per_point()).floor() as usize;
    let rect = waterfall_image(ui, m, w, true);
    // A kHz scale: every 10 kHz, labelled every 20.
    let p = ui.painter();
    let x_of = |hz: f32| rect.left() + (hz - m.low_hz) / m.span_hz.max(1.0) * rect.width();
    let mut k = (m.low_hz / 10e3).ceil() * 10e3;
    while k <= m.low_hz + m.span_hz {
        let x = x_of(k);
        p.line_segment([pos2(x, rect.bottom()), pos2(x, rect.bottom() + 4.0)], Stroke::new(1.0, GREY));
        // Every other tick labelled, clear of "kHz" at the right.
        if (k / 10e3).round() as i64 % 2 == 0 && x < rect.right() - 36.0 {
            p.text(pos2(x, rect.bottom() + 5.0), Align2::CENTER_TOP, format!("{:.0}", k / 1e3), FontId::monospace(10.0), GREY);
        }
        k += 10e3;
    }
    p.text(pos2(rect.right(), rect.bottom() + 5.0), Align2::RIGHT_TOP, "kHz", FontId::monospace(10.0), GREY);
    room
}

/// IQ points, scaled so the outer points fill the plot, drawn as
/// fine points: an image a physical pixel to a dot, brighter where points
/// pile up (so a dense cloud shows its shape, not a blob). As DecDVB-T's
/// scopes do, the image is one texture updated in place; painting thousands
/// of circles was both coarse and dear.
fn scope(ui: &mut Ui, pts: &[Iq], colour: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let ppp = ui.ctx().pixels_per_point();
    let n = ((size * ppp).round() as usize).max(16);
    let bg = Color32::from_rgb(13, 13, 13);
    let mut counts = vec![0u16; n * n];
    if !pts.is_empty() {
        // The scale: the 98th percentile of the larger of |I| and |Q| to 92 %
        // of the half-width, so a square constellation reaches the edges
        // (the RMS at 45 % had left a wide empty margin); stray points past
        // it fall off. Smoothed over frames so it does not pump.
        let mut ext: Vec<f32> = pts.iter().map(|z| z.re.abs().max(z.im.abs())).collect();
        let at = ((ext.len() - 1) as f32 * 0.98) as usize;
        let (_, v, _) = ext.select_nth_unstable_by(at, |a, b| a.total_cmp(b));
        let target = (0.92 * n as f32 / 2.0) / v.max(1e-9);
        let sid = ui.id().with("scope_scale");
        let k = ui.data_mut(|d| {
            let s = d.get_temp_mut_or(sid, target);
            *s += 0.2 * (target - *s);
            *s
        });
        let c = n as f32 / 2.0;
        for z in pts {
            let (x, y) = (c + z.re * k, c - z.im * k);
            if x >= 0.0 && y >= 0.0 && (x as usize) < n && (y as usize) < n {
                let i = y as usize * n + x as usize;
                counts[i] = counts[i].saturating_add(1);
            }
        }
    }
    // A point is clearly seen; each more on the same pixel brightens it.
    let [r, g, b, _] = colour.to_array();
    let grid = Color32::from_gray(42);
    let mid = n / 2;
    let pixels: Vec<Color32> = counts
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            if c == 0 {
                let (x, y) = (i % n, i / n);
                return if x == mid || y == mid { grid } else { bg };
            }
            let a = 1.0 - (-0.55 * f32::from(c)).exp();
            let mix = |fg: u8, bgc: u8| (f32::from(bgc) + (f32::from(fg) - f32::from(bgc)) * (0.35 + 0.65 * a)) as u8;
            Color32::from_rgb(mix(r, 13), mix(g, 13), mix(b, 13))
        })
        .collect();
    let image = egui::ColorImage::new([n, n], pixels);
    let id = ui.id().with("scope_texture");
    let tex = ui.data_mut(|d| d.get_temp::<egui::TextureHandle>(id));
    let tex = match tex {
        Some(mut t) => {
            t.set(image, egui::TextureOptions::NEAREST);
            t
        }
        None => {
            let t = ui.ctx().load_texture("scope", image, egui::TextureOptions::NEAREST);
            ui.data_mut(|d| d.insert_temp(id, t.clone()));
            t
        }
    };
    ui.painter_at(rect).image(tex.id(), rect, egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
}

/// Services (click one to decode it) and the decoder's text.
/// The service a text belongs to when none is picked: the first listed.
fn shown_service(view: &DecoderView, selected: Option<u32>) -> Option<u32> {
    selected.or(view.services.first().map(|s| s.id))
}

/// What a click in the services panel asks for.
pub enum Pick {
    Service(u32),
    /// Tune the channel here (a nearby cell's carrier), Hz.
    Tune(f64),
}

pub fn services(ui: &mut Ui, view: &DecoderView, selected: Option<u32>) -> Option<Pick> {
    let mut pick = None;
    // Everything scrolls: a multiplex has a dozen services or more, and
    // the box must not grow out of the panel.
    egui::ScrollArea::vertical().id_salt("services").max_height(ui.available_height()).show(ui, |ui| {
        let shown = shown_service(view, selected);
        // The picked (or first) service's text first: its now/next.
        if let Some(t) = shown.and_then(|id| view.text.get(&id)).or(view.text.values().next()) {
            ui.add(egui::Label::new(RichText::new(t).color(Color32::WHITE)).wrap());
            ui.add_space(6.0);
        }
        if let Some(h) = &view.heard {
            pick = heard(ui, h).map(Pick::Tune);
        } else {
            ui.label(RichText::new("Services").strong());
            if view.services.is_empty() {
                ui.label(RichText::new("none announced").color(GREY));
            }
        }
        for s in &view.services {
            let name = if s.name.is_empty() { format!("[{}]", s.id) } else { s.name.clone() };
            let text = format!("{name:<24} {}", s.kind);
            if ui.selectable_label(shown == Some(s.id), RichText::new(text)).clicked() {
                pick = Some(Pick::Service(s.id));
            }
        }
        if let Some(last) = view.log.back() {
            ui.add_space(6.0);
            ui.label(RichText::new(format!("log: {last}")).size(11.0).color(GREY));
        }
    });
    pick
}

/// Seconds as m:ss (h:mm:ss past an hour).
fn clock(s: f64) -> String {
    let s = s.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// How a heard table is sorted: by column `col`, the column's natural way
/// (names A–Z, counts and times largest first, last heard most recent
/// first) or, clicked again, the other way. Kept in egui's memory per table.
#[derive(Clone, Copy, PartialEq)]
struct SortBy {
    col: usize,
    reversed: bool,
}

/// A table's header row of clickable column names (the sorted one marked
/// ▼/▲); returns how to sort. `last` is the default column.
fn sort_header(ui: &mut Ui, table: &str, cols: &[(&str, bool)], last: usize) -> SortBy {
    let id = ui.id().with(("heard_sort", table));
    let mut by = ui.data(|d| d.get_temp::<SortBy>(id)).unwrap_or(SortBy { col: last, reversed: false });
    for (i, &(name, right)) in cols.iter().enumerate() {
        let mark = match (by.col == i, by.reversed) {
            (true, false) => " ▼",
            (true, true) => " ▲",
            _ => "",
        };
        let text = RichText::new(format!("{name}{mark}")).color(if by.col == i { AMBER } else { GREY });
        let label = egui::Label::new(text).sense(Sense::click());
        let r = if right {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.add(label)).inner
        } else {
            ui.add(label)
        };
        if r.on_hover_text("sort").clicked() {
            by = if by.col == i { SortBy { col: i, reversed: !by.reversed } } else { SortBy { col: i, reversed: false } };
        }
    }
    ui.data_mut(|d| d.insert_temp(id, by));
    ui.end_row();
    by
}

/// "TG 2350" as ("TG ", 2350), so TG 9 sorts before TG 2350.
fn natural(s: &str) -> (String, u64) {
    let i = s.find(|c: char| c.is_ascii_digit()).unwrap_or(s.len());
    let n = s[i..].chars().take_while(char::is_ascii_digit).collect::<String>();
    (s[..i].to_string(), n.parse().unwrap_or(0))
}

/// Digital voice: the talkgroups and radios heard, two tables (mockup A),
/// and between them the nearby cells the network announces. The one on
/// the air is green, encrypted ones grey with a lock. Returns a nearby
/// cell's frequency when it is clicked.
fn heard(ui: &mut Ui, h: &declab_api::Heard) -> Option<f64> {
    let mut tune = None;
    let ago = |last: f64, live: bool| if live { "now".to_string() } else { clock(h.now_s - last) };
    let head = |ui: &mut Ui, title: &str, note: String| {
        // Beside the title, not right-aligned: in the scroll area the right
        // edge is the content's, which clipped it.
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).color(AMBER));
            ui.add_space(8.0);
            ui.label(RichText::new(note).color(GREY).size(12.0));
        });
    };
    // A table's header row, then its rows; numbers right-aligned.
    let cell = |ui: &mut Ui, text: String, colour: Color32, right: bool| {
        let t = RichText::new(text).color(colour);
        if right {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(t));
        } else {
            ui.label(t);
        }
    };
    let colour = |live: bool, enc: bool| {
        if live {
            GREEN
        } else if enc {
            GREY
        } else {
            Color32::from_gray(230)
        }
    };
    let lock = |enc: bool| if enc { " 🔒" } else { "" };
    let dot = |live: bool| if live { "● " } else { "" };

    // A data network's lists are channels and nodes, its calls packets.
    let groups = if h.data { "Channels" } else { "Talkgroups" };
    head(ui, groups, format!("{} heard in {}", h.groups.len(), clock(h.now_s - h.since_s)));
    if h.groups.is_empty() {
        ui.label(RichText::new("none yet").color(GREY));
    } else {
        egui::Grid::new("heard_groups").striped(false).spacing(vec2(14.0, 2.0)).show(ui, |ui| {
            let cols = if h.data {
                [("channel", false), ("modem", false), ("packets", true), ("nodes", true), ("air time", true), ("last", true)]
            } else {
                [("TG", false), ("slot", false), ("calls", true), ("radios", true), ("air time", true), ("last", true)]
            };
            let by = sort_header(ui, "groups", &cols, 5);
            let mut rows: Vec<&declab_api::HeardGroup> = h.groups.iter().collect();
            // Rust note: `sort_by` is stable, so equal keys keep the
            // decoder's order (most recent first).
            rows.sort_by(|a, b| {
                let o = match by.col {
                    0 => natural(&a.name).cmp(&natural(&b.name)),
                    1 => a.slot.cmp(&b.slot),
                    2 => b.calls.cmp(&a.calls),
                    3 => b.radios.cmp(&a.radios),
                    4 => b.air_s.total_cmp(&a.air_s),
                    _ => b.last_s.total_cmp(&a.last_s),
                };
                if by.reversed { o.reverse() } else { o }
            });
            for g in rows {
                let c = colour(g.live, g.encrypted);
                cell(ui, format!("{}{}{}", dot(g.live), g.name, lock(g.encrypted)), c, false);
                cell(ui, g.slot.clone(), c, false);
                cell(ui, g.calls.to_string(), c, true);
                cell(ui, g.radios.to_string(), c, true);
                cell(ui, clock(g.air_s), c, true);
                cell(ui, ago(g.last_s, g.live), c, true);
                ui.end_row();
            }
        });
    }
    if !h.cells.is_empty() {
        ui.add_space(8.0);
        head(ui, "Nearby cells", "click a frequency to tune there".into());
        egui::Grid::new("heard_cells").spacing(vec2(14.0, 2.0)).show(ui, |ui| {
            for c in &h.cells {
                cell(ui, c.name.clone(), Color32::from_gray(230), false);
                let f = RichText::new(format!("{:.4} MHz", c.freq_hz / 1e6)).color(AMBER);
                if ui.link(f).on_hover_text("tune here").clicked() {
                    tune = Some(c.freq_hz);
                }
                cell(ui, c.detail.clone(), GREY, false);
                ui.end_row();
            }
        });
    }
    ui.add_space(8.0);
    head(ui, if h.data { "Nodes" } else { "Radios" }, format!("{} heard", h.radios.len()));
    if h.radios.is_empty() {
        ui.label(RichText::new("none yet").color(GREY));
    } else {
        egui::Grid::new("heard_radios").striped(false).spacing(vec2(14.0, 2.0)).show(ui, |ui| {
            let cols = if h.data {
                [("node", false), ("channel", false), ("packets", true), ("air time", true), ("position", false), ("last", true)]
            } else {
                [("radio", false), ("last TG", false), ("calls", true), ("talk time", true), ("position", false), ("last", true)]
            };
            let by = sort_header(ui, "radios", &cols, 5);
            let mut rows: Vec<&declab_api::HeardRadio> = h.radios.iter().collect();
            rows.sort_by(|a, b| {
                let o = match by.col {
                    0 => natural(&a.id).cmp(&natural(&b.id)),
                    1 => natural(&a.group).cmp(&natural(&b.group)),
                    2 => b.calls.cmp(&a.calls),
                    3 => b.talk_s.total_cmp(&a.talk_s),
                    // With a fix first, the most recent first.
                    4 => b.position_s.unwrap_or(f64::NEG_INFINITY).total_cmp(&a.position_s.unwrap_or(f64::NEG_INFINITY)),
                    _ => b.last_s.total_cmp(&a.last_s),
                };
                if by.reversed { o.reverse() } else { o }
            });
            for r in rows {
                let c = colour(r.live, r.encrypted);
                cell(ui, format!("{}{}", dot(r.live), r.id), c, false);
                cell(ui, format!("{}{}", r.group, lock(r.encrypted)), c, false);
                cell(ui, r.calls.to_string(), c, true);
                cell(ui, if r.calls > 0 { clock(r.talk_s) } else { String::new() }, c, true);
                let pos = r.position_s.map_or(String::new(), |p| format!("fix {} ago", clock(h.now_s - p)));
                cell(ui, pos, Color32::from_rgb(138, 154, 168), false);
                cell(ui, ago(r.last_s, r.live), c, true);
                ui.end_row();
            }
        });
    }
    tune
}

/// The status bar.
pub fn status(ui: &mut Ui, stats: &Stats, view: &DecoderView, running: bool, app_cpu: f32, error: Option<&str>) {
    let kv = |ui: &mut Ui, k: &str, v: String| {
        ui.label(RichText::new(k).color(GREY).size(12.0));
        ui.label(RichText::new(v).size(12.0));
        ui.add_space(14.0);
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        let state = if let Some(e) = error {
            format!("error: {e}")
        } else if running {
            "running".into()
        } else if stats.ended {
            "ended".into()
        } else {
            "stopped".into()
        };
        kv(ui, "state", state);
        if !stats.describe.is_empty() {
            kv(ui, "source", stats.describe.clone());
        }
        let ch = view.channel;
        if ch.out_rate > 0.0 {
            let rs = ch.resample.map(|(u, d)| format!(" ×{u}/{d}")).unwrap_or_default();
            kv(ui, "channel", format!("{:.0} kS/s (÷{}{rs})", ch.out_rate / 1e3, ch.decimation));
        }
        if stats.handoff_dropped > 0 {
            kv(ui, "dropped", format!("{} blocks", stats.handoff_dropped));
        }
        if stats.clip_percent > 0.1 {
            ui.label(RichText::new(format!("CLIPPING {:.1} %", stats.clip_percent)).color(RED).size(12.0));
            ui.add_space(14.0);
        }
        if let Some(a) = &view.audio {
            kv(ui, "sound", a.clone());
        }
        let p = stats.stages.percent;
        kv(
            ui,
            "CPU",
            format!(
                "app {app_cpu:.0} % · source {:.0} · front end {:.0} · spectrum {:.0} · decoder {:.0}",
                p[0] + p[1],
                p[2],
                p[3],
                view.cpu_percent
            ),
        );
    });
}

/// A level meter bar like the other Dec apps'.
pub fn level_bar(ui: &mut Ui, dbfs: f32, live: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(180.0, 20.0), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(10), Color32::from_rgb(38, 38, 38));
    if live {
        let f = ((dbfs + 60.0) / 60.0).clamp(0.0, 1.0);
        let fill = egui::Rect::from_min_size(rect.min, vec2(rect.width() * f, rect.height()));
        p.rect_filled(fill, CornerRadius::same(10), Color32::from_rgb(11, 93, 122));
        p.text(
            rect.left_center() + vec2(10.0, 0.0),
            Align2::LEFT_CENTER,
            format!("{dbfs:.1} dBFS"),
            FontId::monospace(12.0),
            Color32::WHITE,
        );
    }
}
