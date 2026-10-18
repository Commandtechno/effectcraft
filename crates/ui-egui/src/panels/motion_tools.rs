//! Window ▸ Motion Tools: a one-panel motion-design toolkit (our own design, in the spirit of the
//! motion panels After Effects animators install). Top to bottom:
//!
//! - the active comp (name, duration, frame rate, size);
//! - the **ease curve** graph (value or speed view; drag the two handles in the value view) and the
//!   **anchor grid** (3 × 3 points of the selected layers' content bounds: `motion.anchor`);
//! - **Ease out / Overshoot / Ease in** sliders for the curve;
//! - the **tools**: Null, Build, Dynamics, Delay, Falloff, Clone, Spin, Stare (`motion.*`). Click a
//!   tool to apply it (right-click to only show its options); the current tool's options follow;
//! - a **time strip** of the comp (work area, playhead, selected keyframes; click or drag to set the
//!   time);
//! - the menu, the **ease presets** and Read (the curve of the selected keys) / Apply (the curve to
//!   the selected keys: `motion.ease`).
//!
//! Settings live in [`crate::state::MotionToolsState`] (`ui.set {"motionTools": {…}}`).
//! Automation ids: `motionTools.graph`, `motionTools.graph.out`, `motionTools.graph.in`,
//! `motionTools.view.value|speed`, `motionTools.anchor.<top|middle|bottom><Left|Center|Right>`,
//! `motionTools.easeOut|overshoot|easeIn` (sliders) and `….value` (their numbers),
//! `motionTools.tool.<id>`, `motionTools.opt.<name>`, `motionTools.timeStrip`, `motionTools.menu`,
//! `motionTools.preset.<id>`, `motionTools.read`, `motionTools.apply`.

use effectcraft_engine::commands::motion::{Curve, PRESETS, curve_progress};
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::{Value, json};

use crate::i18n::tr;
use crate::state::MotionToolsState;
use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

/// (id, label, tooltip).
const TOOLS: &[(&str, &str, &str)] = &[
    ("null", "NULL", "Null: parent the selected layers to a new null at their centre"),
    ("build", "BUILD", "Build: key a fade, scale, slide or rotation at the layers' In / Out points, eased with the curve"),
    ("dynamics", "DYNAMICS", "Dynamics: overshoot (elastic) or bounce after the selected keyframes"),
    ("delay", "DELAY", "Delay: stagger the selected keyframes (or layers) by frames per layer"),
    ("falloff", "FALLOFF", "Falloff: a null whose distance scales the selected layers' Scale, Opacity or Rotation"),
    ("clone", "CLONE", "Clone: duplicate the selected layers with a position and time offset per copy"),
    ("spin", "SPIN", "Spin: rotate the selected layers continuously"),
    ("stare", "STARE", "Stare: point the selected layers' Rotation at a target layer"),
];

fn curve(st: &MotionToolsState) -> Curve {
    Curve {
        out_influence: (st.out_influence / 100.0).clamp(0.001, 1.0),
        out_speed: st.out_speed,
        in_influence: (st.in_influence / 100.0).clamp(0.001, 1.0),
        in_speed: st.in_speed,
    }
}

fn set_curve(st: &mut MotionToolsState, c: Curve) {
    st.out_influence = c.out_influence * 100.0;
    st.out_speed = c.out_speed;
    st.in_influence = c.in_influence * 100.0;
    st.in_speed = c.in_speed;
}

fn ease_params(st: &MotionToolsState) -> Value {
    json!({"outInfluence": st.out_influence, "outSpeed": st.out_speed, "inInfluence": st.in_influence, "inSpeed": st.in_speed})
}

/// The command and parameters a tool runs with the current options.
fn tool_call(st: &MotionToolsState, tool: &str) -> Option<(&'static str, Value)> {
    Some(match tool {
        "null" => ("motion.null", json!({})),
        "build" => {
            let mut p = ease_params(st);
            p["kind"] = json!(st.build_kind);
            p["mode"] = json!(st.build_mode);
            p["frames"] = json!(st.build_frames);
            p["from"] = json!(st.build_from);
            ("motion.build", p)
        }
        "dynamics" => ("motion.dynamics", json!({"mode": st.dyn_mode, "amplitude": st.dyn_amplitude, "frequency": st.dyn_frequency, "decay": st.dyn_decay})),
        "delay" => ("motion.delay", json!({"frames": st.delay_frames, "order": st.delay_order, "what": st.delay_what})),
        "falloff" => {
            let props: Vec<&str> = st.falloff_props.split('+').collect();
            ("motion.falloff", json!({"props": props, "radius": st.falloff_radius, "feather": st.falloff_feather, "invert": st.falloff_invert}))
        }
        "clone" => ("motion.clone", json!({"count": st.clone_count.round().max(1.0) as u64, "offset": st.clone_offset, "frames": st.clone_frames})),
        "spin" => ("motion.spin", json!({"speed": st.spin_speed})),
        "stare" => ("motion.stare", json!({"target": st.stare_target, "offset": st.stare_offset})),
        _ => return None,
    })
}

/// A short report of what a command did, for the status bar.
fn summary(label: &str, r: &Value) -> String {
    let n = |k: &str| r.get(k).and_then(Value::as_u64);
    let what = if let Some(x) = n("segments") {
        format!("{x} segment(s) eased")
    } else if let Some(x) = n("keys") {
        format!("{x} keyframe(s) added")
    } else if let Some(x) = n("builds") {
        format!("{x} propert(ies) keyed")
    } else if let Some(x) = n("properties") {
        format!("{x} propert(ies) linked")
    } else if let Some(a) = r.get("layers").and_then(Value::as_array) {
        format!("{} layer(s)", a.len())
    } else if let Some(x) = n("layers") {
        format!("{x} layer(s)")
    } else {
        "done".into()
    };
    format!("{label}: {what}")
}

fn run(app: &mut EffectcraftApp, ctx: &egui::Context, label: &str, id: &str, params: Value) -> Option<Value> {
    match crate::menus::invoke(app, ctx, id, params) {
        Ok(r) => {
            app.ui.status = summary(label, &r);
            Some(r)
        }
        Err(e) => {
            app.ui.status = format!("{label}: {e}");
            None
        }
    }
}

// ---------------------------------------------------------------- drawing helpers

/// The original line icons of the tools, drawn in `r`.
fn tool_icon(p: &egui::Painter, r: Rect, id: &str, c: Color32) {
    let s = Stroke::new(1.4, c);
    let at = |x: f32, y: f32| pos2(r.min.x + x * r.width(), r.min.y + y * r.height());
    match id {
        "null" => {
            // A square drawn as its four corners.
            for (x, y, dx, dy) in [(0.15, 0.15, 1.0, 1.0), (0.85, 0.15, -1.0, 1.0), (0.15, 0.85, 1.0, -1.0), (0.85, 0.85, -1.0, -1.0)] {
                p.line_segment([at(x, y), at(x + 0.22 * dx, y)], s);
                p.line_segment([at(x, y), at(x, y + 0.22 * dy)], s);
            }
        }
        "build" => {
            for (i, h) in [0.3, 0.55, 0.8].iter().enumerate() {
                let x = 0.18 + i as f32 * 0.25;
                p.rect_stroke(Rect::from_min_max(at(x, 0.9 - h), at(x + 0.16, 0.9)), 1.0, s, StrokeKind::Middle);
            }
        }
        "dynamics" => {
            let pts: Vec<Pos2> = (0..=32)
                .map(|i| {
                    let x = i as f32 / 32.0;
                    at(0.08 + 0.84 * x, 0.5 - 0.38 * (x * 4.0 * std::f32::consts::TAU / 2.0).sin() * (1.0 - x))
                })
                .collect();
            p.line(pts, s);
        }
        "delay" => {
            for i in 0..3 {
                let (x, y) = (0.12 + i as f32 * 0.2, 0.25 + i as f32 * 0.25);
                p.line_segment([at(x, y), at(x + 0.45, y)], Stroke::new(2.0, c));
            }
        }
        "falloff" => {
            for (k, a) in [(0.12, 255u8), (0.24, 170), (0.36, 90)] {
                p.circle_stroke(r.center(), k * r.width(), Stroke::new(1.4, Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)));
            }
        }
        "clone" => {
            p.rect_stroke(Rect::from_min_max(at(0.12, 0.12), at(0.58, 0.58)), 1.0, s, StrokeKind::Middle);
            p.rect_stroke(Rect::from_min_max(at(0.42, 0.42), at(0.88, 0.88)), 1.0, s, StrokeKind::Middle);
        }
        "spin" => {
            let pts: Vec<Pos2> = (0..=24)
                .map(|i| {
                    let a = -0.5 + i as f32 / 24.0 * 4.6;
                    pos2(r.center().x + a.cos() * r.width() * 0.32, r.center().y + a.sin() * r.height() * 0.32)
                })
                .collect();
            if let Some(&end) = pts.last() {
                let a: f32 = 4.1;
                let tip = [vec2(a.cos(), a.sin()) * 4.0, vec2((a + 2.2).cos(), (a + 2.2).sin()) * 4.0];
                p.line_segment([end, end + tip[0]], s);
                p.line_segment([end, end + tip[1]], s);
            }
            p.line(pts, s);
        }
        "stare" => {
            let eye = |top: bool| -> Vec<Pos2> {
                (0..=16)
                    .map(|i| {
                        let x = i as f32 / 16.0;
                        let y = (x * std::f32::consts::PI).sin() * 0.28;
                        at(0.1 + 0.8 * x, if top { 0.5 - y } else { 0.5 + y })
                    })
                    .collect()
            };
            p.line(eye(true), s);
            p.line(eye(false), s);
            p.circle_filled(r.center(), r.width() * 0.1, c);
        }
        _ => {}
    }
}

/// A preset's curve thumbnail.
fn mini_curve(p: &egui::Painter, r: Rect, c: Curve, col: Color32) {
    let (lo, hi) = (-0.35f32, 1.35f32);
    let map = |x: f64, v: f64| pos2(r.min.x + 3.0 + x as f32 * (r.width() - 6.0), r.max.y - 3.0 - ((v as f32 - lo) / (hi - lo)) * (r.height() - 6.0));
    let pts: Vec<Pos2> = (0..=20).map(|i| i as f64 / 20.0).map(|x| map(x, curve_progress(c, x))).collect();
    p.line(pts, Stroke::new(1.5, col));
}

// ---------------------------------------------------------------- sections

/// The graph's value range (room for overshoot).
const VLO: f64 = -0.35;
const VHI: f64 = 1.35;

/// The curve graph. Returns true when a drag finished (for auto-apply).
fn graph(app: &mut EffectcraftApp, ui: &mut egui::Ui, r: Rect, st: &mut MotionToolsState) -> bool {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(r.expand(6.0));
    p.rect_filled(r, 3.0, t.field_bg);
    let inner = r.shrink(10.0);
    let to_screen = |x: f64, v: f64| pos2(inner.min.x + x as f32 * inner.width(), inner.max.y - ((v - VLO) / (VHI - VLO)) as f32 * inner.height());
    let from_screen =
        |q: Pos2| (((q.x - inner.min.x) / inner.width().max(1.0)) as f64, VLO + ((inner.max.y - q.y) / inner.height().max(1.0)) as f64 * (VHI - VLO));
    let c = curve(st);
    // 0 and 1 levels.
    for v in [0.0, 1.0] {
        p.line_segment([to_screen(0.0, v), to_screen(1.0, v)], Stroke::new(1.0, t.separator));
    }
    let accent = t.accent;
    app.auto.add("motionTools.graph", r, if st.speed_graph { "Speed graph" } else { "Value graph" });
    if st.speed_graph {
        // Speed: the slope of the value curve, scaled to the graph's height.
        let h = 1e-3;
        let sp: Vec<f64> = (0..=96)
            .map(|i| i as f64 / 96.0)
            .map(|x| (curve_progress(c, (x + h).min(1.0)) - curve_progress(c, (x - h).max(0.0))) / ((x + h).min(1.0) - (x - h).max(0.0)))
            .collect();
        let max = sp.iter().fold(1e-9f64, |m, v| m.max(v.abs()));
        let pts: Vec<Pos2> = sp.iter().enumerate().map(|(i, v)| to_screen(i as f64 / 96.0, v / max)).collect();
        p.line(pts, Stroke::new(2.5, accent));
        return false;
    }
    p.line_segment([to_screen(0.0, 0.0), to_screen(1.0, 1.0)], Stroke::new(1.0, t.text_faint.gamma_multiply(0.4)));
    let pts: Vec<Pos2> = (0..=96).map(|i| i as f64 / 96.0).map(|x| to_screen(x, curve_progress(c, x))).collect();
    p.line(pts, Stroke::new(2.5, accent));
    // Handles: P1 = (oi, oi × os), P2 = (1 − ii, 1 − ii × is).
    let h1 = to_screen(c.out_influence, c.out_influence * c.out_speed);
    let h2 = to_screen(1.0 - c.in_influence, 1.0 - c.in_influence * c.in_speed);
    let mut done = false;
    for (which, h, anchor) in [("out", h1, to_screen(0.0, 0.0)), ("in", h2, to_screen(1.0, 1.0))] {
        p.line_segment([anchor, h], Stroke::new(1.0, t.text_dim));
        let hit = Rect::from_center_size(h, vec2(16.0, 16.0));
        let id = egui::Id::new(("motionTools.graph", which));
        let resp = ui.interact(hit, id, Sense::drag());
        app.auto.add(&format!("motionTools.graph.{which}"), hit, if which == "out" { "Out handle" } else { "In handle" });
        let hot = resp.hovered() || resp.dragged();
        p.circle_filled(h, if hot { 6.0 } else { 5.0 }, if hot { t.text } else { t.panel_bg });
        p.circle_stroke(h, if hot { 6.0 } else { 5.0 }, Stroke::new(1.5, accent));
        if resp.dragged()
            && let Some(q) = resp.interact_pointer_pos()
        {
            let (x, v) = from_screen(q);
            let v = v.clamp(VLO, VHI);
            if which == "out" {
                let x = x.clamp(0.001, 1.0);
                st.out_influence = x * 100.0;
                st.out_speed = (v / x).clamp(-20.0, 20.0);
            } else {
                let i = (1.0 - x).clamp(0.001, 1.0);
                st.in_influence = i * 100.0;
                st.in_speed = ((1.0 - v) / i).clamp(-20.0, 20.0);
            }
        }
        done |= resp.drag_stopped();
    }
    done
}

const ANCHOR_ROWS: [&str; 3] = ["top", "middle", "bottom"];
const ANCHOR_COLS: [&str; 3] = ["Left", "Center", "Right"];

/// The 3 × 3 anchor grid: returns the clicked point (fractions).
fn anchor_grid(app: &mut EffectcraftApp, ui: &mut egui::Ui, r: Rect) -> Option<(f64, f64)> {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(r.expand(4.0));
    let mut out = None;
    let has = !app.session.state.selected_layers.is_empty();
    for (j, row) in ANCHOR_ROWS.iter().enumerate() {
        for (i, col) in ANCHOR_COLS.iter().enumerate() {
            let c = pos2(r.min.x + r.width() * (i as f32 + 0.5) / 3.0, r.min.y + r.height() * (j as f32 + 0.5) / 3.0);
            let hit = Rect::from_center_size(c, vec2(22.0, 22.0));
            let resp = ui.interact(hit, egui::Id::new(("motionTools.anchor", i, j)), Sense::click());
            let name = format!("{row}{col}");
            app.auto.add(&format!("motionTools.anchor.{name}"), hit, &format!("Anchor {row} {}", col.to_lowercase()));
            let col_c = if !has {
                t.text_faint.gamma_multiply(0.5)
            } else if resp.hovered() {
                t.accent
            } else {
                t.text_dim
            };
            p.circle_filled(c, if resp.hovered() && has { 6.0 } else { 5.0 }, col_c);
            if resp.clicked() {
                out = Some((i as f64 / 2.0, j as f64 / 2.0));
            }
            resp.on_hover_text(tr("Move the anchor point here (the layer stays put)"));
        }
    }
    out
}

/// One slider row: a track with a handle and the value as a number. Returns (new value, released).
#[allow(clippy::too_many_arguments)]
fn slider(app: &mut EffectcraftApp, ui: &mut egui::Ui, x0: f32, x1: f32, y: f32, auto: &str, label: &str, v: f64, range: (f64, f64)) -> (f64, bool) {
    let t = app.tokens;
    let p = ui.painter().clone();
    p.text(pos2(x0, y + 9.0), Align2::LEFT_CENTER, tr(label), Tokens::ui(11.0), t.text_dim);
    let (tx0, tx1) = (x0 + 70.0, (x1 - 52.0).max(x0 + 90.0));
    let track = Rect::from_min_max(pos2(tx0, y + 2.0), pos2(tx1, y + 16.0));
    let resp = ui.interact(track, egui::Id::new(("motionTools.slider", auto)), Sense::click_and_drag());
    app.auto.add(auto, track, label);
    let frac = ((v - range.0) / (range.1 - range.0)).clamp(0.0, 1.0) as f32;
    let cy = track.center().y;
    p.line_segment([pos2(tx0, cy), pos2(tx1, cy)], Stroke::new(2.0, t.separator));
    p.line_segment([pos2(tx0, cy), pos2(tx0 + (tx1 - tx0) * frac, cy)], Stroke::new(2.0, t.accent));
    let hx = tx0 + (tx1 - tx0) * frac;
    let hr = Rect::from_center_size(pos2(hx, cy), vec2(9.0, 11.0));
    p.rect_filled(hr, 1.5, if resp.hovered() || resp.dragged() { t.text } else { t.text_dim });
    let mut nv = v;
    if (resp.dragged() || resp.clicked())
        && let Some(q) = resp.interact_pointer_pos()
    {
        let f = ((q.x - tx0) / (tx1 - tx0).max(1.0)).clamp(0.0, 1.0) as f64;
        nv = (range.0 + f * (range.1 - range.0)).round();
    }
    let (nr, typed, _) = widgets::hot_number_at(ui, pos2(tx1 + 10.0, y + 1.0), egui::Id::new(("motionTools.n", auto)), v, 0.5, range, 0, "", &t);
    app.auto.add(&format!("{auto}.value"), nr, label);
    if let Some(x) = typed {
        nv = x.clamp(range.0, range.1);
    }
    (nv, resp.drag_stopped() || resp.clicked() || typed.is_some())
}

/// Lays option controls out left to right, wrapping.
struct Flow {
    x0: f32,
    x1: f32,
    x: f32,
    y: f32,
}

impl Flow {
    fn place(&mut self, w: f32) -> Pos2 {
        if self.x + w > self.x1 && self.x > self.x0 {
            self.x = self.x0;
            self.y += 26.0;
        }
        let at = pos2(self.x, self.y);
        self.x += w + 12.0;
        at
    }
}

/// A labelled dropdown option; returns the chosen value.
fn opt_choice(app: &mut EffectcraftApp, ui: &mut egui::Ui, f: &mut Flow, name: &str, label: &str, items: &[(&str, &str)], cur: &str) -> String {
    let t = app.tokens;
    let lw = if label.is_empty() { 0.0 } else { ui.painter().layout_no_wrap(tr(label).to_string(), Tokens::ui(11.0), t.text_dim).size().x + 6.0 };
    let at = f.place(lw + 92.0);
    if lw > 0.0 {
        ui.painter().text(pos2(at.x, at.y + 10.0), Align2::LEFT_CENTER, tr(label), Tokens::ui(11.0), t.text_dim);
    }
    let r = Rect::from_min_size(pos2(at.x + lw, at.y), vec2(92.0, 20.0));
    let i = items.iter().position(|x| x.0 == cur).unwrap_or(0);
    let text = items.get(i).map(|x| tr(x.1)).unwrap_or_default();
    let id = egui::Id::new(("motionTools.opt", name));
    if widgets::dropdown(ui, r, text, &t, id).clicked() {
        widgets::open_popup(ui, id);
    }
    app.auto.add(&format!("motionTools.opt.{name}"), r, text);
    let opts: Vec<String> = items.iter().map(|x| tr(x.1).to_string()).collect();
    match widgets::popup_menu(ui, id, r.left_bottom(), &opts, Some(i)) {
        Some(j) => items.get(j).map(|x| x.0.to_string()).unwrap_or_else(|| cur.to_string()),
        None => cur.to_string(),
    }
}

/// A labelled number option.
#[allow(clippy::too_many_arguments)]
fn opt_number(app: &mut EffectcraftApp, ui: &mut egui::Ui, f: &mut Flow, name: &str, label: &str, v: f64, range: (f64, f64), dec: usize, suffix: &str) -> f64 {
    let t = app.tokens;
    let lw = ui.painter().layout_no_wrap(tr(label).to_string(), Tokens::ui(11.0), t.text_dim).size().x + 6.0;
    let at = f.place(lw + 56.0);
    ui.painter().text(pos2(at.x, at.y + 10.0), Align2::LEFT_CENTER, tr(label), Tokens::ui(11.0), t.text_dim);
    let (r, nv, _) = widgets::hot_number_at(ui, pos2(at.x + lw, at.y + 2.0), egui::Id::new(("motionTools.opt", name)), v, 0.5, range, dec, suffix, &t);
    app.auto.add(&format!("motionTools.opt.{name}"), r, label);
    nv.unwrap_or(v)
}

fn opt_check(app: &mut EffectcraftApp, ui: &mut egui::Ui, f: &mut Flow, name: &str, label: &str, on: bool) -> bool {
    let t = app.tokens;
    let lw = ui.painter().layout_no_wrap(tr(label).to_string(), Tokens::ui(11.0), t.text).size().x;
    let at = f.place(lw + 24.0);
    let r = Rect::from_min_size(pos2(at.x, at.y + 2.0), vec2(16.0, 16.0));
    let clicked = widgets::checkbox(ui, r, on, &t, egui::Id::new(("motionTools.opt", name))).clicked();
    ui.painter().text(pos2(r.max.x + 6.0, r.center().y), Align2::LEFT_CENTER, tr(label), Tokens::ui(11.0), t.text);
    app.auto.add(&format!("motionTools.opt.{name}"), r, label);
    if clicked { !on } else { on }
}

/// The current tool's options; returns the flow's bottom.
fn options(app: &mut EffectcraftApp, ui: &mut egui::Ui, x0: f32, x1: f32, y: f32, st: &mut MotionToolsState) -> f32 {
    let t = app.tokens;
    let mut f = Flow { x0, x1, x: x0, y };
    match st.tool.as_str() {
        "null" => {
            ui.painter().text(
                pos2(x0, y + 10.0),
                Align2::LEFT_CENTER,
                tr("Parents the selected layers to a new null at their centre."),
                Tokens::ui(11.0),
                t.text_faint,
            );
        }
        "build" => {
            st.build_kind =
                opt_choice(app, ui, &mut f, "buildKind", "", &[("fade", "Fade"), ("scale", "Scale"), ("slide", "Slide"), ("rotate", "Rotate")], &st.build_kind);
            st.build_mode = opt_choice(app, ui, &mut f, "buildMode", "", &[("in", "In"), ("out", "Out"), ("both", "In and Out")], &st.build_mode);
            st.build_frames = opt_number(app, ui, &mut f, "buildFrames", "Frames", st.build_frames, (1.0, 600.0), 0, "");
            if st.build_kind == "slide" {
                st.build_from = opt_choice(
                    app,
                    ui,
                    &mut f,
                    "buildFrom",
                    "From",
                    &[("bottom", "Bottom"), ("top", "Top"), ("left", "Left"), ("right", "Right")],
                    &st.build_from,
                );
            }
        }
        "dynamics" => {
            st.dyn_mode = opt_choice(app, ui, &mut f, "dynMode", "", &[("elastic", "Elastic"), ("bounce", "Bounce")], &st.dyn_mode);
            st.dyn_amplitude = opt_number(app, ui, &mut f, "dynAmplitude", "Amount", st.dyn_amplitude, (0.0, 500.0), 0, " %");
            st.dyn_frequency = opt_number(app, ui, &mut f, "dynFrequency", "Frequency", st.dyn_frequency, (0.05, 60.0), 1, "/s");
            st.dyn_decay = opt_number(app, ui, &mut f, "dynDecay", "Decay", st.dyn_decay, (0.0, 95.0), 0, " %");
        }
        "delay" => {
            st.delay_frames = opt_number(app, ui, &mut f, "delayFrames", "Frames", st.delay_frames, (-1000.0, 1000.0), 1, "");
            st.delay_order = opt_choice(app, ui, &mut f, "delayOrder", "", &[("selection", "Selection order"), ("reverse", "Reverse order")], &st.delay_order);
            st.delay_what =
                opt_choice(app, ui, &mut f, "delayWhat", "", &[("auto", "Keys or layers"), ("keys", "Keyframes"), ("layers", "Layers")], &st.delay_what);
        }
        "falloff" => {
            st.falloff_props = opt_choice(
                app,
                ui,
                &mut f,
                "falloffProps",
                "",
                &[("scale", "Scale"), ("opacity", "Opacity"), ("rotation", "Rotation"), ("scale+opacity", "Scale + Opacity")],
                &st.falloff_props,
            );
            st.falloff_radius = opt_number(app, ui, &mut f, "falloffRadius", "Radius", st.falloff_radius, (0.0, 100_000.0), 0, "");
            st.falloff_feather = opt_number(app, ui, &mut f, "falloffFeather", "Feather", st.falloff_feather, (0.0, 100_000.0), 0, "");
            st.falloff_invert = opt_check(app, ui, &mut f, "falloffInvert", "Invert", st.falloff_invert);
        }
        "clone" => {
            st.clone_count = opt_number(app, ui, &mut f, "cloneCount", "Copies", st.clone_count, (1.0, 200.0), 0, "");
            st.clone_offset[0] = opt_number(app, ui, &mut f, "cloneX", "X", st.clone_offset[0], (-100_000.0, 100_000.0), 0, "");
            st.clone_offset[1] = opt_number(app, ui, &mut f, "cloneY", "Y", st.clone_offset[1], (-100_000.0, 100_000.0), 0, "");
            st.clone_frames = opt_number(app, ui, &mut f, "cloneFrames", "Frames", st.clone_frames, (-1000.0, 1000.0), 0, "");
        }
        "spin" => {
            st.spin_speed = opt_number(app, ui, &mut f, "spinSpeed", "Speed", st.spin_speed, (-100_000.0, 100_000.0), 0, "°/s");
        }
        "stare" => {
            let layers: Vec<(String, String)> =
                app.session.active_comp().map(|c| c.layers.iter().map(|l| (l.id.0.to_string(), l.name.clone())).collect()).unwrap_or_default();
            let mut items: Vec<(&str, &str)> = vec![("", "Choose target…")];
            items.extend(layers.iter().map(|(i, n)| (i.as_str(), n.as_str())));
            let cur = st.stare_target.map(|x| x.to_string()).unwrap_or_default();
            st.stare_target = opt_choice(app, ui, &mut f, "stareTarget", "Target", &items, &cur).parse().ok();
            st.stare_offset = opt_number(app, ui, &mut f, "stareOffset", "Offset", st.stare_offset, (-360.0, 360.0), 0, "°");
        }
        _ => {}
    }
    f.y + 22.0
}

/// The comp's time strip: work area, playhead and the selected keyframes. Returns a time to go to.
fn time_strip(app: &mut EffectcraftApp, ui: &mut egui::Ui, r: Rect) -> Option<f64> {
    let t = app.tokens;
    let c = app.session.active_comp()?;
    let dur = c.duration.seconds().max(1e-6);
    let x_of = |s: f64| r.min.x + ((s / dur).clamp(0.0, 1.0) as f32) * r.width();
    let p = ui.painter().with_clip_rect(r.expand(2.0));
    p.rect_filled(r, 2.0, t.field_bg);
    let (wa0, wa1) = (c.work_area.0.seconds(), c.work_area.1.seconds());
    p.rect_filled(Rect::from_min_max(pos2(x_of(wa0), r.min.y + 4.0), pos2(x_of(wa1), r.max.y - 4.0)), 1.0, t.accent.gamma_multiply(0.45));
    for k in &app.session.state.selected_keys {
        let Some(l) = c.layer(k.layer) else { continue };
        let x = x_of(l.comp_time(k.time).seconds());
        let cy = r.center().y;
        p.add(egui::Shape::convex_polygon(vec![pos2(x, cy - 4.0), pos2(x + 4.0, cy), pos2(x, cy + 4.0), pos2(x - 4.0, cy)], t.keyframe_selected, Stroke::NONE));
    }
    let now = x_of(app.session.time().seconds());
    p.line_segment([pos2(now, r.min.y), pos2(now, r.max.y)], Stroke::new(1.5, t.cti));
    let resp = ui.interact(r, egui::Id::new("motionTools.timeStrip"), Sense::click_and_drag());
    app.auto.add("motionTools.timeStrip", r, "Time strip");
    if (resp.dragged() || resp.clicked())
        && let Some(q) = resp.interact_pointer_pos()
    {
        return Some((((q.x - r.min.x) / r.width().max(1.0)).clamp(0.0, 1.0) as f64) * dur);
    }
    None
}

// ---------------------------------------------------------------- the panel

/// Window ▸ Motion Tools: the panel's contents scroll vertically when it is shorter than them.
pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let key = egui::Id::new("motionTools.scrollY");
    let mut scroll: f32 = ui.data(|d| d.get_temp(key)).unwrap_or(0.0);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    let content = rect.with_max_x(rect.max.x - 8.0);
    let bottom = contents(app, &mut child, content, rect.min.y + 8.0 - scroll);
    let max = (bottom + 8.0 - scroll - rect.max.y).max(0.0);
    if ui.rect_contains_pointer(rect) && !widgets::any_menu_open(ui.ctx()) {
        scroll -= ui.input(|i| i.smooth_scroll_delta.y);
    }
    scroll = scroll.clamp(0.0, max);
    if max > 0.0 {
        let track = Rect::from_min_max(pos2(rect.max.x - 7.0, rect.min.y + 2.0), pos2(rect.max.x - 2.0, rect.max.y - 2.0));
        scroll = widgets::scroll_bar(ui, &mut app.auto, "motionTools.scroll", track, scroll, max, &t);
    }
    ui.data_mut(|d| d.insert_temp(key, scroll));
}

/// The panel's contents from `top` (scrolled); returns their bottom.
fn contents(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect, top: f32) -> f32 {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let mut st = app.ui.motion_tools.clone();
    let p = ui.painter().with_clip_rect(rect);
    // A compact column, however wide the panel is.
    let x0 = rect.min.x + 10.0;
    let x1 = (rect.max.x - 10.0).min(x0 + 420.0);
    let w = (x1 - x0).max(120.0);
    let mut y = top;
    // Commands to run after drawing: (status label, id, params).
    let mut acts: Vec<(String, &'static str, Value)> = vec![];
    let mut apply_curve = false;

    // The active comp.
    match app.session.active_comp_id().and_then(|id| Some((app.session.project.item(id)?.name.clone(), app.session.project.comp(id)?.clone()))) {
        Some((name, c)) => {
            p.text(pos2(x0, y + 7.0), Align2::LEFT_CENTER, &name, Tokens::medium(13.0), t.text);
            let info = format!("{}   {:.2} fps   {}×{}", crate::panels::timecode(&app.session, &c, c.duration), c.frame_rate.as_f64(), c.width, c.height);
            p.text(pos2(x0, y + 23.0), Align2::LEFT_CENTER, info, Tokens::ui(11.0), t.text_faint);
        }
        None => {
            p.text(pos2(x0, y + 7.0), Align2::LEFT_CENTER, tr("No composition"), Tokens::medium(13.0), t.text_dim);
        }
    }
    let nk = app.session.state.selected_keys.len();
    let nl = app.session.state.selected_layers.len();
    y += 38.0;
    p.text(pos2(x1, y + 9.0), Align2::RIGHT_CENTER, format!("{nl} layer(s) · {nk} key(s)"), Tokens::ui(11.0), t.text_faint);

    // Graph view toggle.
    for (i, (id, label)) in [("value", "Value"), ("speed", "Speed")].into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(x0 + i as f32 * 52.0, y), vec2(50.0, 18.0));
        let on = st.speed_graph == (id == "speed");
        let resp = ui.interact(r, egui::Id::new(("motionTools.view", id)), Sense::click());
        ui.painter().rect_filled(
            r,
            3.0,
            if on {
                t.pressed
            } else if resp.hovered() {
                t.hover
            } else {
                Color32::TRANSPARENT
            },
        );
        ui.painter().text(r.center(), Align2::CENTER_CENTER, tr(label), Tokens::ui(11.0), if on { t.text } else { t.text_dim });
        app.auto.add(&format!("motionTools.view.{id}"), r, label);
        if resp.clicked() {
            st.speed_graph = id == "speed";
        }
    }
    y += 24.0;

    // The graph and the anchor grid beside it.
    let gw = (w * 0.56).clamp(100.0, 240.0);
    let g = Rect::from_min_size(pos2(x0, y), vec2(gw, (gw * 0.8).clamp(80.0, 180.0)));
    apply_curve |= graph(app, ui, g, &mut st) && st.auto_apply;
    let side = (x1 - g.max.x - 16.0).min(g.height()).max(48.0);
    let grid = Rect::from_center_size(pos2((g.max.x + x1) / 2.0, g.center().y), vec2(side, side));
    if let Some((fx, fy)) = anchor_grid(app, ui, grid) {
        acts.push(("Anchor".into(), "motion.anchor", json!({"x": fx, "y": fy})));
    }
    y = g.max.y + 12.0;

    // Ease sliders.
    let (v, done) = slider(app, ui, x0, x1, y, "motionTools.easeOut", "Ease out", st.out_influence, (1.0, 100.0));
    st.out_influence = v;
    apply_curve |= done && st.auto_apply;
    y += 24.0;
    let over = (-st.in_speed).max(0.0) * 50.0;
    let (v, done) = slider(app, ui, x0, x1, y, "motionTools.overshoot", "Overshoot", over, (0.0, 100.0));
    if (v - over).abs() > 1e-9 {
        st.in_speed = -v / 50.0;
    }
    apply_curve |= done && st.auto_apply;
    y += 24.0;
    let (v, done) = slider(app, ui, x0, x1, y, "motionTools.easeIn", "Ease in", st.in_influence, (1.0, 100.0));
    st.in_influence = v;
    apply_curve |= done && st.auto_apply;
    y += 30.0;

    // Tools.
    let (cw, ch) = (54.0f32, 46.0f32);
    let per_row = ((w + 2.0) / cw).floor().max(1.0) as usize;
    for (i, (id, label, tip)) in TOOLS.iter().enumerate() {
        let (cx, cy) = ((i % per_row) as f32, (i / per_row) as f32);
        let r = Rect::from_min_size(pos2(x0 + cx * cw, y + cy * ch), vec2(cw - 2.0, ch - 2.0));
        let resp = ui.interact(r, egui::Id::new(("motionTools.tool", *id)), Sense::click());
        let cur = st.tool == *id;
        if cur || resp.hovered() {
            ui.painter().rect_filled(r, 3.0, if cur { t.pressed } else { t.hover });
        }
        let col = if cur { t.icon_active } else { t.icon };
        tool_icon(ui.painter(), Rect::from_center_size(pos2(r.center().x, r.min.y + 15.0), vec2(22.0, 22.0)), id, col);
        ui.painter().text(pos2(r.center().x, r.max.y - 8.0), Align2::CENTER_CENTER, tr(label), Tokens::medium(9.5), if cur { t.text } else { t.text_dim });
        app.auto.add(&format!("motionTools.tool.{id}"), r, label);
        if resp.secondary_clicked() {
            st.tool = id.to_string();
        } else if resp.clicked() {
            st.tool = id.to_string();
            if let Some((cmd, params)) = tool_call(&st, id) {
                acts.push((label.to_string(), cmd, params));
            }
        }
        resp.on_hover_text(tr(tip));
    }
    y += ch * TOOLS.len().div_ceil(per_row) as f32 + 6.0;
    ui.painter().line_segment([pos2(x0, y - 3.0), pos2(x1, y - 3.0)], Stroke::new(1.0, t.separator));
    y = options(app, ui, x0, x1, y + 4.0, &mut st) + 8.0;

    // Time strip.
    if let Some(s) = time_strip(app, ui, Rect::from_min_size(pos2(x0, y), vec2(w, 18.0))) {
        acts.push(("Time".into(), "time.set", json!({"time": s})));
    }
    y += 28.0;

    // Menu, presets, Read and Apply.
    let menu_r = Rect::from_min_size(pos2(x0, y), vec2(22.0, 22.0));
    let menu_id = egui::Id::new("motionTools.menu");
    let mresp = ui.interact(menu_r, menu_id, Sense::click());
    for k in 0..3 {
        let yy = menu_r.min.y + 7.0 + k as f32 * 4.0;
        ui.painter()
            .line_segment([pos2(menu_r.min.x + 5.0, yy), pos2(menu_r.max.x - 5.0, yy)], Stroke::new(1.3, if mresp.hovered() { t.text } else { t.icon }));
    }
    app.auto.add("motionTools.menu", menu_r, "Menu");
    if mresp.clicked() {
        widgets::open_popup(ui, menu_id);
    }
    let menu = [widgets::check_label(st.auto_apply, tr("Auto-Apply Curve")), tr("Read Curve from Keyframes").to_string(), tr("Reset Curve").to_string()];
    match widgets::popup_menu(ui, menu_id, menu_r.left_bottom(), &menu, None) {
        Some(0) => st.auto_apply = !st.auto_apply,
        Some(1) => acts.push(("Read".into(), "motion.readEase", json!({}))),
        Some(2) => set_curve(&mut st, Curve::default()),
        _ => {}
    }
    let btn_w = 26.0;
    let right = x1 - 2.0 * btn_w - 4.0;
    let (mut px, mut py) = (menu_r.max.x + 8.0, y);
    let cur = curve(&st);
    for (id, label, c) in PRESETS {
        if px + 24.0 > right {
            px = menu_r.max.x + 8.0;
            py += 26.0;
        }
        let r = Rect::from_min_size(pos2(px, py), vec2(22.0, 22.0));
        let resp = ui.interact(r, egui::Id::new(("motionTools.preset", *id)), Sense::click());
        let same = (cur.out_influence - c.out_influence).abs() < 1e-6
            && (cur.in_influence - c.in_influence).abs() < 1e-6
            && (cur.out_speed - c.out_speed).abs() < 1e-6
            && (cur.in_speed - c.in_speed).abs() < 1e-6;
        ui.painter().rect_filled(
            r,
            3.0,
            if same {
                t.accent.gamma_multiply(0.35)
            } else if resp.hovered() {
                t.hover
            } else {
                t.field_bg
            },
        );
        ui.painter().rect_stroke(r, 3.0, Stroke::new(1.0, if same { t.accent } else { t.field_border }), StrokeKind::Inside);
        mini_curve(ui.painter(), r, *c, if same { t.text } else { t.accent });
        app.auto.add(&format!("motionTools.preset.{id}"), r, label);
        if resp.clicked() {
            set_curve(&mut st, *c);
            apply_curve |= nk > 0;
        }
        resp.on_hover_text(tr(label));
        px += 24.0;
    }
    for (k, (id, label, tip)) in
        [("read", "Read", "Read the curve of the selected keyframes"), ("apply", "Apply", "Apply the curve to the selected keyframes")].into_iter().enumerate()
    {
        let r = Rect::from_min_size(pos2(x1 - (2 - k) as f32 * btn_w, y), vec2(btn_w - 2.0, 22.0));
        let resp = ui.interact(r, egui::Id::new(("motionTools", id)), Sense::click());
        let enabled = nk > 0;
        ui.painter().rect_filled(
            r,
            3.0,
            if id == "apply" && enabled {
                if resp.hovered() { t.accent_hover } else { t.accent }
            } else if resp.hovered() {
                t.hover
            } else {
                t.field_bg
            },
        );
        let col = if !enabled {
            t.text_faint
        } else if id == "apply" {
            Color32::WHITE
        } else {
            t.text
        };
        let c = r.center();
        if id == "read" {
            // An upward arrow: take the curve from the keys.
            ui.painter().line_segment([pos2(c.x, c.y + 6.0), pos2(c.x, c.y - 6.0)], Stroke::new(1.5, col));
            ui.painter().line(vec![pos2(c.x - 5.0, c.y - 1.0), pos2(c.x, c.y - 6.0), pos2(c.x + 5.0, c.y - 1.0)], Stroke::new(1.5, col));
        } else {
            // A lightning bolt: apply.
            let pts = vec![
                pos2(c.x + 2.0, c.y - 8.0),
                pos2(c.x - 4.0, c.y + 1.0),
                pos2(c.x + 1.0, c.y + 1.0),
                pos2(c.x - 2.0, c.y + 8.0),
                pos2(c.x + 4.0, c.y - 1.0),
                pos2(c.x - 1.0, c.y - 1.0),
            ];
            ui.painter().add(egui::Shape::closed_line(pts, Stroke::new(1.4, col)));
        }
        app.auto.add(&format!("motionTools.{id}"), r, label);
        if resp.clicked() {
            if id == "read" {
                acts.push(("Read".into(), "motion.readEase", json!({})));
            } else {
                apply_curve = true;
            }
        }
        resp.on_hover_text(tr(tip));
    }

    if apply_curve {
        let mut params = ease_params(&st);
        params["merge"] = json!("motionTools.ease");
        acts.push(("Ease".into(), "motion.ease", params));
    }
    app.ui.motion_tools = st;
    for (label, id, params) in acts {
        let r = run(app, &ctx, &label, id, params);
        if id == "motion.readEase"
            && let Some(c) = r.and_then(|r| effectcraft_engine::commands::motion::curve_p(&r, id).ok())
        {
            set_curve(&mut app.ui.motion_tools, c);
            app.ui.status = tr("Read the curve of the selected keyframes").to_string();
        }
    }
    py + 30.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_command() {
        let st = MotionToolsState::default();
        for (id, _, _) in TOOLS {
            let (cmd, _) = tool_call(&st, id).unwrap();
            assert!(effectcraft_engine::commands::find(cmd).is_some(), "{cmd}");
        }
        assert!(tool_call(&st, "nope").is_none());
    }

    #[test]
    fn curve_state_round_trips() {
        let mut st = MotionToolsState::default();
        let c = Curve { out_influence: 0.25, out_speed: 0.5, in_influence: 0.75, in_speed: -0.2 };
        set_curve(&mut st, c);
        let back = curve(&st);
        assert!((back.out_influence - 0.25).abs() < 1e-12 && (back.in_speed + 0.2).abs() < 1e-12);
    }
}
