// made by CasualArclamp
//! The decoder wheel: a radial picker, like a game's weapon wheel.
//!
//! A middle click on the band (or the decoder button) opens it. One slice
//! per decoder, grouped by family with a coloured band outside; each slice
//! has a small drawn icon and a short label. Point at a slice or scroll to
//! step round it, then click (left or middle) to choose. The centre shows
//! what would be chosen; clicking it, clicking outside, a right click or Esc
//! closes the wheel and keeps the decoder.

use std::f32::consts::TAU;

use declab_api::Family;
use eframe::egui::{
    self, Align2, Color32, FontId, Id, Key, Mesh, Order, PointerButton, Pos2, Rect, Sense,
    Shape, Stroke, pos2, vec2,
};

/// Radii, points: slices from `R0` to `R1`, the family band beyond.
const R0: f32 = 76.0;
const R1: f32 = 174.0;
const BAND: (f32, f32) = (184.0, 190.0);
const LABEL_R: f32 = 204.0;
/// How far out a click still counts as on the wheel.
const REACH: f32 = 236.0;

pub fn family_colour(f: Family) -> Color32 {
    match f {
        Family::Terrestrial => Color32::from_rgb(255, 176, 40),
        Family::Satellite => Color32::from_rgb(95, 176, 255),
        Family::Radio => Color32::from_rgb(110, 230, 130),
        Family::Analog => Color32::from_rgb(210, 155, 255),
        Family::Voice => Color32::from_rgb(255, 120, 110),
        Family::Data => Color32::from_rgb(80, 220, 210),
        Family::Other => Color32::from_rgb(176, 182, 188),
    }
}

/// One slice.
pub struct Entry {
    pub id: &'static str,
    pub label: &'static str,
    pub family: Family,
    /// "384 kS/s · 200 kHz": what the decoder asks the host for.
    pub input: String,
    pub summary: &'static str,
}

pub enum Outcome {
    Pick(usize),
    Cancel,
}

pub struct Wheel {
    centre: Pos2,
    /// The slice pointed at or stepped to; `None`: the centre.
    hover: Option<usize>,
    current: usize,
    /// The frame it opened in: the click that opened it is not a choice.
    opened_frame: u64,
    /// Where the pointer was last frame: only a pointer that moves changes
    /// the slice, so one resting in the centre does not undo a scroll.
    last_pointer: Option<Pos2>,
}

impl Wheel {
    pub fn new(centre: Pos2, current: usize, frame: u64) -> Wheel {
        Wheel { centre, hover: None, current, opened_frame: frame, last_pointer: None }
    }

    pub fn show(&mut self, ctx: &egui::Context, entries: &[Entry]) -> Option<Outcome> {
        let n = entries.len().max(1);
        let screen = ctx.content_rect();
        // Kept on screen.
        let m = R1 + 30.0;
        self.centre = pos2(
            self.centre.x.clamp(screen.left() + m, (screen.right() - m).max(screen.left() + m)),
            self.centre.y.clamp(screen.top() + m, (screen.bottom() - m).max(screen.top() + m)),
        );
        let c = self.centre;
        let mut outcome = None;

        // A layer over everything that takes every click, so nothing under
        // the wheel reacts while it is open.
        egui::Area::new(Id::new("decoder_wheel"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let resp = ui.allocate_rect(screen, Sense::click());
                let painter = ui.painter();
                painter.rect_filled(screen, 0.0, Color32::from_black_alpha(110));

                // Pointer and scroll pick the slice.
                let pointer = ctx.pointer_hover_pos();
                let moved = pointer.is_some() && self.last_pointer.is_some_and(|q| Some(q) != pointer);
                self.last_pointer = pointer.or(self.last_pointer);
                if let Some(p) = pointer.filter(|_| moved) {
                    let d = p - c;
                    let dist = d.length();
                    if dist < R0 {
                        self.hover = None;
                    } else if dist < REACH {
                        self.hover = Some(slice_at(d, n));
                    }
                }
                let notches: i64 = ctx.input(|i| {
                    i.raw
                        .events
                        .iter()
                        .map(|e| match e {
                            egui::Event::MouseWheel { delta, .. } if delta.y < 0.0 => 1,
                            egui::Event::MouseWheel { delta, .. } if delta.y > 0.0 => -1,
                            _ => 0,
                        })
                        .sum()
                });
                if notches != 0 {
                    let base = self.hover.unwrap_or(self.current) as i64;
                    self.hover = Some((base + notches).rem_euclid(n as i64) as usize);
                }

                draw(painter, c, entries, self.hover, self.current);

                let fresh = ctx.cumulative_frame_nr() <= self.opened_frame + 1;
                if ctx.input(|i| i.key_pressed(Key::Escape)) || resp.clicked_by(PointerButton::Secondary) {
                    outcome = Some(Outcome::Cancel);
                } else if !fresh && (resp.clicked_by(PointerButton::Primary) || resp.clicked_by(PointerButton::Middle)) {
                    let far = resp.interact_pointer_pos().is_some_and(|p| (p - c).length() > REACH);
                    outcome = Some(match self.hover {
                        Some(i) if !far => Outcome::Pick(i),
                        _ => Outcome::Cancel,
                    });
                } else if let Some(Key::Enter) = ctx.input(|i| i.key_pressed(Key::Enter).then_some(Key::Enter)) {
                    outcome = Some(match self.hover {
                        Some(i) => Outcome::Pick(i),
                        None => Outcome::Cancel,
                    });
                }
            });
        outcome
    }
}

/// A slice's label: the decoder's name if it is short, else its first word.
fn short(name: &str) -> &str {
    if name.len() <= 7 { name } else { name.split([' ', '/']).next().unwrap_or(name) }
}

/// The slice under offset `d` from the centre (slice 0 starts at the top,
/// going clockwise).
fn slice_at(d: egui::Vec2, n: usize) -> usize {
    let a = (d.y.atan2(d.x) + TAU / 4.0).rem_euclid(TAU);
    ((a / (TAU / n as f32)) as usize).min(n - 1)
}

/// The point at radius `r`, angle `a` (radians clockwise from the top).
fn at(c: Pos2, r: f32, a: f32) -> Pos2 {
    pos2(c.x + r * a.sin(), c.y - r * a.cos())
}

/// An annular sector as a mesh (egui's polygons must be convex; this is not).
fn sector(c: Pos2, r0: f32, r1: f32, a0: f32, a1: f32, fill: Color32) -> Shape {
    let steps = (((a1 - a0) / 0.03).ceil() as usize).max(2);
    let mut mesh = Mesh::default();
    for k in 0..=steps {
        let a = a0 + (a1 - a0) * k as f32 / steps as f32;
        mesh.colored_vertex(at(c, r0, a), fill);
        mesh.colored_vertex(at(c, r1, a), fill);
        if k > 0 {
            let i = (2 * k) as u32;
            mesh.add_triangle(i - 2, i - 1, i);
            mesh.add_triangle(i - 1, i + 1, i);
        }
    }
    Shape::mesh(mesh)
}

/// The outline of an annular sector.
fn outline(c: Pos2, r0: f32, r1: f32, a0: f32, a1: f32, stroke: Stroke) -> Shape {
    let steps = (((a1 - a0) / 0.03).ceil() as usize).max(2);
    let mut pts: Vec<Pos2> = (0..=steps).map(|k| at(c, r1, a0 + (a1 - a0) * k as f32 / steps as f32)).collect();
    pts.extend((0..=steps).rev().map(|k| at(c, r0, a0 + (a1 - a0) * k as f32 / steps as f32)));
    Shape::closed_line(pts, stroke)
}

fn draw(painter: &egui::Painter, c: Pos2, entries: &[Entry], hover: Option<usize>, current: usize) {
    let n = entries.len().max(1);
    let per = TAU / n as f32;
    let gap = 0.02;
    painter.circle_filled(c, BAND.1 + 4.0, Color32::from_rgba_unmultiplied(20, 20, 20, 240));

    for (i, e) in entries.iter().enumerate() {
        let col = family_colour(e.family);
        let on = hover == Some(i);
        let (a0, a1) = (i as f32 * per + gap / 2.0, (i + 1) as f32 * per - gap / 2.0);
        let (r0, r1) = if on { (R0 + 3.0, R1 + 5.0) } else { (R0, R1) };
        let fill = if on { col } else { col.gamma_multiply(0.19) };
        painter.add(sector(c, r0, r1, a0, a1, fill));
        let stroke = if i == current { Stroke::new(2.5, col) } else { Stroke::new(1.0, Color32::from_gray(46)) };
        painter.add(outline(c, r0, r1, a0, a1, stroke));

        let mid = (a0 + a1) / 2.0;
        let ink = if on { Color32::from_rgb(20, 20, 20) } else { Color32::from_gray(236) };
        icon(painter, e.id, at(c, (r0 + r1) / 2.0 - 10.0, mid), 13.0, ink);
        painter.text(
            at(c, (r0 + r1) / 2.0 + 24.0, mid),
            Align2::CENTER_CENTER,
            short(e.label),
            FontId::monospace(12.0),
            ink,
        );
    }

    // A band outside for each run of one family, with its name.
    let mut start = 0;
    for i in 1..=entries.len() {
        if i == entries.len() || entries[i].family != entries[start].family {
            let fam = entries[start].family;
            let col = family_colour(fam);
            let (a0, a1) = (start as f32 * per + gap / 2.0, i as f32 * per - gap / 2.0);
            painter.add(sector(c, BAND.0, BAND.1, a0, a1, col));
            let mid = (a0 + a1) / 2.0;
            let p = at(c, LABEL_R, mid);
            let align = if p.x > c.x + 20.0 {
                Align2::LEFT_CENTER
            } else if p.x < c.x - 20.0 {
                Align2::RIGHT_CENTER
            } else {
                Align2::CENTER_CENTER
            };
            painter.text(p, align, fam.label(), FontId::monospace(13.0), col);
            start = i;
        }
    }

    // The centre: what a click would choose, or how to leave.
    let shown = &entries[hover.unwrap_or(current).min(entries.len() - 1)];
    let col = family_colour(shown.family);
    let ring = if hover.is_some() { col } else { Color32::from_gray(70) };
    painter.circle(c, R0 - 4.0, Color32::from_rgb(28, 28, 28), Stroke::new(2.0, ring));
    let name_size = if shown.label.len() > 8 { 13.0 } else { 17.0 };
    let lines: [(String, f32, Color32, f32); 3] = if hover.is_some() {
        [
            (shown.family.label().to_string(), -26.0, Color32::from_gray(160), 11.0),
            (shown.label.to_string(), -4.0, col, name_size),
            (shown.input.clone(), 20.0, Color32::from_gray(200), 9.5),
        ]
    } else {
        [
            ("click to keep".into(), -26.0, Color32::from_gray(160), 11.0),
            (shown.label.to_string(), -4.0, col, name_size),
            ("or point · scroll".into(), 20.0, Color32::from_gray(200), 10.0),
        ]
    };
    for (text, dy, colour, size) in lines {
        painter.text(c + vec2(0.0, dy), Align2::CENTER_CENTER, text, FontId::monospace(size), colour);
    }
    // What it does, under the wheel where there is room for a sentence.
    let g = painter.layout_no_wrap(shown.summary.to_string(), FontId::monospace(12.0), Color32::from_gray(225));
    let r = Rect::from_center_size(c + vec2(0.0, BAND.1 + 34.0), g.size() + vec2(16.0, 8.0));
    painter.rect_filled(r, 4.0, Color32::from_rgba_unmultiplied(20, 20, 20, 235));
    painter.galley(r.min + vec2(8.0, 4.0), g, Color32::from_gray(225));
}

/// A small line drawing for a decoder, about `s` points from its middle to
/// its edge, in `ink`.
pub fn icon(painter: &egui::Painter, id: &str, p: Pos2, s: f32, ink: Color32) {
    let st = Stroke::new(1.6, ink);
    let wave = |f: &dyn Fn(f32) -> f32| -> Shape {
        let pts: Vec<Pos2> = (0..=40)
            .map(|k| {
                let t = k as f32 / 40.0;
                pos2(p.x - s + 2.0 * s * t, p.y - f(t) * s * 0.7)
            })
            .collect();
        Shape::line(pts, st)
    };
    // A satellite dish: a bowl opening up and right, its feed and its foot.
    let dish = |painter: &egui::Painter| {
        let c = p + vec2(-s * 0.15, s * 0.1);
        let pts: Vec<Pos2> = (0..=20)
            .map(|k| {
                let a = -0.15 * TAU + 0.5 * TAU * k as f32 / 20.0 + TAU * 0.125;
                at(c, s * 0.75, a)
            })
            .collect();
        painter.add(Shape::line(pts, st));
        painter.line_segment([c, c + vec2(s * 0.45, -s * 0.45)], st);
        painter.circle_filled(c + vec2(s * 0.5, -s * 0.5), 2.2, ink);
        painter.line_segment([c + vec2(0.0, s * 0.5), c + vec2(0.0, s * 0.9)], st);
        painter.line_segment([c + vec2(-s * 0.35, s * 0.9), c + vec2(s * 0.35, s * 0.9)], st);
    };
    match id {
        "dvbt" | "dvbt2" => {
            // A television with its stand and an aerial.
            let r = Rect::from_center_size(p + vec2(0.0, s * 0.1), vec2(s * 1.8, s * 1.2));
            painter.rect_stroke(r, 2.0, st, egui::StrokeKind::Middle);
            painter.line_segment([r.center_bottom(), r.center_bottom() + vec2(0.0, s * 0.3)], st);
            painter.line_segment([r.center_top(), r.center_top() + vec2(-s * 0.45, -s * 0.5)], st);
            painter.line_segment([r.center_top(), r.center_top() + vec2(s * 0.45, -s * 0.5)], st);
        }
        "dvbs" | "s2ts" => dish(painter),
        "sat" => {
            // A dish with a feed horn and the packets of its data.
            dish(painter);
            for k in 0..3 {
                painter.circle_filled(p + vec2(s * 0.95, s * (0.1 + 0.35 * k as f32)), 1.8, ink);
            }
        }
        "s2ip" => {
            dish(painter);
            for k in 0..3 {
                painter.circle_filled(p + vec2(s * 0.95, s * (0.1 + 0.35 * k as f32)), 1.8, ink);
            }
        }
        "rcs2" => {
            dish(painter);
            let a = p + vec2(s * 0.9, s * 0.8);
            painter.line_segment([a, a + vec2(0.0, -s * 1.2)], st);
            painter.line_segment([a + vec2(0.0, -s * 1.2), a + vec2(-s * 0.25, -s * 0.9)], st);
            painter.line_segment([a + vec2(0.0, -s * 1.2), a + vec2(s * 0.25, -s * 0.9)], st);
        }
        "dab" => {
            // A radio set: speaker grille and a display.
            let r = Rect::from_center_size(p, vec2(s * 1.9, s * 1.3));
            painter.rect_stroke(r, 3.0, st, egui::StrokeKind::Middle);
            painter.circle_stroke(r.left_center() + vec2(s * 0.5, 0.0), s * 0.35, st);
            painter.rect_stroke(
                Rect::from_min_size(r.center() + vec2(s * 0.05, -s * 0.35), vec2(s * 0.7, s * 0.35)),
                1.0,
                st,
                egui::StrokeKind::Middle,
            );
            painter.line_segment([r.left_top() + vec2(s * 0.3, 0.0), r.left_top() + vec2(s * 0.9, -s * 0.5)], st);
        }
        "wfm" => {
            // Frequency modulation: the cycles bunch up and spread out.
            painter.add(wave(&|t| (TAU * (3.0 * t + 0.6 * (TAU * t).sin())).sin()));
        }
        "nfm" => {
            painter.add(wave(&|t| 0.6 * (TAU * (3.0 * t + 0.3 * (TAU * t).sin())).sin()));
        }
        "am" => {
            // A carrier under an envelope.
            painter.add(wave(&|t| (0.55 + 0.4 * (TAU * t).sin()) * (TAU * 7.0 * t).sin()));
        }
        "usb" | "lsb" | "dsb" => {
            // The spectrum about the carrier: one sideband, or both.
            let base = p.y + s * 0.7;
            painter.line_segment([pos2(p.x - s, base), pos2(p.x + s, base)], st);
            painter.line_segment([pos2(p.x, base + 2.0), pos2(p.x, p.y - s * 0.9)], Stroke::new(1.0, ink));
            let side = |dir: f32| {
                vec![
                    pos2(p.x + dir * s * 0.12, base),
                    pos2(p.x + dir * s * 0.12, p.y - s * 0.55),
                    pos2(p.x + dir * s * 0.9, p.y + s * 0.15),
                    pos2(p.x + dir * s * 0.9, base),
                ]
            };
            if id != "lsb" {
                painter.add(Shape::line(side(1.0), st));
            }
            if id != "usb" {
                painter.add(Shape::line(side(-1.0), st));
            }
        }
        "cw" => {
            // Dot, dash, dot.
            painter.circle_filled(p + vec2(-s * 0.8, 0.0), 2.6, ink);
            painter.rect_filled(Rect::from_center_size(p, vec2(s * 0.9, 5.0)), 1.5, ink);
            painter.circle_filled(p + vec2(s * 0.8, 0.0), 2.6, ink);
        }
        "voice" | "dmr" | "nxdn" | "nxdn96" | "p25" | "tetra" => {
            // A handheld two-way radio: body, aerial, speaker slots.
            let r = Rect::from_center_size(p + vec2(0.0, s * 0.2), vec2(s * 0.9, s * 1.4));
            painter.rect_stroke(r, 2.5, st, egui::StrokeKind::Middle);
            painter.line_segment([r.left_top() + vec2(s * 0.2, 0.0), r.left_top() + vec2(s * 0.2, -s * 0.7)], st);
            for k in 0..3 {
                let y = r.top() + s * (0.3 + 0.22 * k as f32);
                painter.line_segment([pos2(r.left() + s * 0.2, y), pos2(r.right() - s * 0.2, y)], Stroke::new(1.0, ink));
            }
        }
        "meshtastic" | "meshtastic-2g4" => {
            // Three nodes of a mesh, each linked to the others.
            let nodes = [p + vec2(-s * 0.8, s * 0.55), p + vec2(s * 0.8, s * 0.55), p + vec2(0.0, -s * 0.75)];
            for (a, b) in [(0, 1), (1, 2), (2, 0)] {
                painter.line_segment([nodes[a], nodes[b]], Stroke::new(1.0, ink));
            }
            for n in nodes {
                painter.circle_filled(n, 3.0, ink);
            }
        }
        "level" => {
            // A bar meter.
            for (k, h) in [0.5f32, 0.9, 1.4].iter().enumerate() {
                let x = p.x - s * 0.7 + k as f32 * s * 0.7;
                painter.rect_filled(
                    Rect::from_min_max(pos2(x - 3.0, p.y + s * 0.7 - h * s), pos2(x + 3.0, p.y + s * 0.7)),
                    1.0,
                    ink,
                );
            }
        }
        _ => {
            painter.circle_stroke(p, s * 0.7, st);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{Sim, button, moved, scroll};

    fn entries() -> Vec<Entry> {
        ["wfm", "nfm", "am", "usb", "lsb", "dsb", "cw", "level"]
            .iter()
            .map(|id| Entry { id, label: id, family: Family::Analog, input: String::new(), summary: "" })
            .collect()
    }

    /// Open a wheel at (500, 400) on decoder `current`, run frames with these
    /// events and return the first outcome.
    fn run(current: usize, frames: Vec<Vec<egui::Event>>) -> Option<Outcome> {
        let mut sim = Sim::new(1000.0, 800.0);
        let e = entries();
        let mut w = Wheel::new(pos2(500.0, 400.0), current, sim.ctx.cumulative_frame_nr());
        let mut out = None;
        let mut all = vec![vec![], vec![]];
        all.extend(frames);
        for events in all {
            sim.frame(events, |ui| {
                if out.is_none() {
                    out = w.show(ui.ctx(), &e);
                }
            });
        }
        out
    }

    fn click(x: f32, y: f32, b: PointerButton) -> Vec<Vec<egui::Event>> {
        vec![vec![button(x, y, b, true)], vec![button(x, y, b, false)]]
    }

    #[test]
    fn pointing_at_a_slice_and_clicking_picks_it() {
        // Eight slices: straight right of the centre is slice 2.
        let mut f = vec![vec![moved(500.0, 400.0)], vec![moved(620.0, 400.0)]];
        f.extend(click(620.0, 400.0, PointerButton::Primary));
        assert!(matches!(run(0, f), Some(Outcome::Pick(2))));
    }

    #[test]
    fn middle_click_picks_too() {
        let mut f = vec![vec![moved(500.0, 400.0)], vec![moved(500.0, 520.0)]];
        f.extend(click(500.0, 520.0, PointerButton::Middle));
        assert!(matches!(run(0, f), Some(Outcome::Pick(4))));
    }

    #[test]
    fn scrolling_steps_from_the_current_decoder_and_a_click_takes_it() {
        // The pointer rests in the centre; two notches down from decoder 3.
        let mut f = vec![vec![moved(500.0, 400.0)], vec![scroll(false)], vec![scroll(false)]];
        f.extend(click(500.0, 400.0, PointerButton::Primary));
        assert!(matches!(run(3, f), Some(Outcome::Pick(5))));
    }

    #[test]
    fn the_centre_outside_or_a_right_click_cancels() {
        let mut f = vec![vec![moved(500.0, 400.0)], vec![moved(505.0, 402.0)]];
        f.extend(click(505.0, 402.0, PointerButton::Primary));
        assert!(matches!(run(1, f), Some(Outcome::Cancel)));
        let mut f = vec![vec![moved(500.0, 400.0)], vec![moved(900.0, 400.0)]];
        f.extend(click(900.0, 400.0, PointerButton::Primary));
        assert!(matches!(run(1, f), Some(Outcome::Cancel)));
        let mut f = vec![vec![moved(620.0, 400.0)], vec![moved(621.0, 400.0)]];
        f.extend(click(621.0, 400.0, PointerButton::Secondary));
        assert!(matches!(run(1, f), Some(Outcome::Cancel)));
    }

    #[test]
    fn slices_go_clockwise_from_the_top() {
        // Straight up is slice 0, straight right a quarter of the way round.
        assert_eq!(slice_at(vec2(0.1, -100.0), 8), 0);
        assert_eq!(slice_at(vec2(100.0, 0.1), 8), 2);
        assert_eq!(slice_at(vec2(0.0, 100.0), 8), 4);
        assert_eq!(slice_at(vec2(-100.0, 0.1), 8), 5);
        assert_eq!(slice_at(vec2(-100.0, -0.1), 8), 6);
        assert_eq!(slice_at(vec2(-0.1, -100.0), 8), 7);
    }
}
