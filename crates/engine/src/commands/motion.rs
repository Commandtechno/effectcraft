//! Window ▸ Motion Tools: our own motion-design toolkit panel (in the spirit of the third-party
//! motion panels After Effects animators install), built from these commands so agents get the
//! same results as the panel:
//!
//! * `motion.ease` applies a normalised ease curve (the value graph of a segment from (0, 0) to
//!   (1, 1) as a cubic Bezier) to the selected keyframes; `motion.readEase` reads it back.
//! * `motion.anchor` moves the Anchor Point to one of nine points of the layer's content bounds
//!   without moving the layer (the panel's 3 × 3 grid).
//! * `motion.null` creates a null at the centre of the selected layers and parents them to it.
//! * `motion.build` keys a build-in and/or build-out (fade, scale, slide, rotate) at the layers'
//!   In and Out points, eased with the curve.
//! * `motion.dynamics` adds overshoot (elastic) or bounce keys after the selected keys.
//! * `motion.delay` staggers the selected keys (or layers) by a number of frames per layer.
//! * `motion.falloff` makes a Falloff null whose distance scales the selected layers' Scale,
//!   Opacity or Rotation (expressions with Radius / Feather sliders).
//! * `motion.clone` duplicates layers with a position and time offset per copy.
//! * `motion.spin` rotates layers continuously (an expression with a Spin Speed slider).
//! * `motion.stare` points layers' Rotation at another layer (an expression).
//!
//! Each command is one undo step.

use effectcraft_keyframe::{Ease, Interp, Keyframe, Value as KV, side_ease, spatial_segment_length};
use effectcraft_project::build::Ids;
use effectcraft_project::{Expression, ItemId, LayerId, PropGroup, Property, Uid};
use effectcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, f_p, has_comp, has_keys, has_layers, layer_mut, layers_p, resolve_layer, str_p};
use crate::{EngineError, KeyRef, Result, Session, cmd, query};

// ---------------------------------------------------------------- the ease curve

/// A normalised ease: the value graph of one segment as a cubic Bezier from (0, 0) to (1, 1)
/// with handles at (`out_influence`, `out_influence × out_speed`) and
/// (1 − `in_influence`, 1 − `in_influence × in_speed`). Influences are fractions (0.001–1);
/// speeds are relative to the segment's average speed (1 = linear, 0 = eased to a stop).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Curve {
    pub out_influence: f64,
    pub out_speed: f64,
    pub in_influence: f64,
    pub in_speed: f64,
}

impl Default for Curve {
    fn default() -> Self {
        Curve { out_influence: 1.0 / 3.0, out_speed: 0.0, in_influence: 1.0 / 3.0, in_speed: 0.0 }
    }
}

/// The ease presets of the panel's preset strip: (id, label, curve).
pub const PRESETS: &[(&str, &str, Curve)] = &[
    ("linear", "Linear", Curve { out_influence: 1.0 / 3.0, out_speed: 1.0, in_influence: 1.0 / 3.0, in_speed: 1.0 }),
    ("easeIn", "Ease In (accelerate)", Curve { out_influence: 0.5, out_speed: 0.0, in_influence: 1.0 / 3.0, in_speed: 1.5 }),
    ("easeOut", "Ease Out (decelerate)", Curve { out_influence: 1.0 / 3.0, out_speed: 1.5, in_influence: 0.5, in_speed: 0.0 }),
    ("easy", "Easy Ease", Curve { out_influence: 1.0 / 3.0, out_speed: 0.0, in_influence: 1.0 / 3.0, in_speed: 0.0 }),
    ("smooth", "Smooth", Curve { out_influence: 0.6, out_speed: 0.0, in_influence: 0.6, in_speed: 0.0 }),
    ("strong", "Strong", Curve { out_influence: 0.85, out_speed: 0.0, in_influence: 0.85, in_speed: 0.0 }),
    ("snap", "Snap (fast out, long settle)", Curve { out_influence: 0.1, out_speed: 0.0, in_influence: 0.9, in_speed: 0.0 }),
    ("anticipate", "Anticipate (slow start, sharp stop)", Curve { out_influence: 0.9, out_speed: 0.0, in_influence: 0.1, in_speed: 0.0 }),
    ("back", "Back (overshoot)", Curve { out_influence: 0.3, out_speed: 0.0, in_influence: 0.45, in_speed: -0.9 }),
];

/// A curve from `{outInfluence %, outSpeed, inInfluence %, inSpeed}` or `{preset}`, defaulting to
/// Easy Ease (the panel sends all four numbers).
pub fn curve_p(p: &Value, cmd: &str) -> Result<Curve> {
    let mut c = match str_p(p, "preset") {
        Some(id) => PRESETS
            .iter()
            .find(|(i, l, _)| *i == id || *l == id)
            .map(|x| x.2)
            .ok_or_else(|| bad(cmd, format!("unknown preset `{id}` ({})", PRESETS.iter().map(|x| x.0).collect::<Vec<_>>().join("|"))))?,
        None => Curve::default(),
    };
    let num = |k: &str| -> Result<Option<f64>> {
        match f_p(p, k) {
            Some(v) if v.is_finite() => Ok(Some(v)),
            Some(_) => Err(bad(cmd, format!("`{k}` must be a finite number"))),
            None => Ok(None),
        }
    };
    if let Some(v) = num("outInfluence")? {
        c.out_influence = v / 100.0;
    }
    if let Some(v) = num("inInfluence")? {
        c.in_influence = v / 100.0;
    }
    if let Some(v) = num("outSpeed")? {
        c.out_speed = v;
    }
    if let Some(v) = num("inSpeed")? {
        c.in_speed = v;
    }
    c.out_influence = c.out_influence.clamp(0.001, 1.0);
    c.in_influence = c.in_influence.clamp(0.001, 1.0);
    // Wild speeds make wild curves but no harm; keep them in a range the graph can show.
    c.out_speed = c.out_speed.clamp(-20.0, 20.0);
    c.in_speed = c.in_speed.clamp(-20.0, 20.0);
    Ok(c)
}

fn curve_json(c: Curve) -> Value {
    json!({"outInfluence": c.out_influence * 100.0, "outSpeed": c.out_speed, "inInfluence": c.in_influence * 100.0, "inSpeed": c.in_speed})
}

/// Progress (0..1, may overshoot) of a curve at time fraction `x` — what the panel's graph draws.
pub fn curve_progress(c: Curve, x: f64) -> f64 {
    effectcraft_keyframe::ease_progress(c.out_speed, c.out_influence, c.in_speed, c.in_influence, x.clamp(0.0, 1.0))
}

/// How a segment's value change is measured per dimension: spatial values by path length, values
/// without numeric components (paths, gradients) as progress 0 → 1, others per dimension.
fn segment_scale(keys: &[Keyframe], i: usize, spatial: bool) -> Vec<f64> {
    let (Some(a), Some(b)) = (keys.get(i), keys.get(i + 1)) else { return vec![] };
    let n = a.value.dims().max(1);
    if spatial && matches!(a.value, KV::Vec2(_) | KV::Vec3(_)) {
        return vec![spatial_segment_length(keys, i); n];
    }
    let (ca, cb) = (a.value.components(), b.value.components());
    if ca.is_empty() || ca.len() != cb.len() {
        return vec![1.0; n];
    }
    ca.iter().zip(&cb).map(|(x, y)| y - x).collect()
}

fn seg_secs(keys: &[Keyframe], i: usize) -> f64 {
    match (keys.get(i), keys.get(i + 1)) {
        (Some(a), Some(b)) => (b.time.seconds() - a.time.seconds()).max(1e-9),
        _ => 1.0,
    }
}

/// Set the out side of key `i` (segment `i → i+1`) or the in side of key `i + 1` to normalised
/// speed `n` and influence `inf`.
fn set_side(keys: &mut [Keyframe], i: usize, out: bool, n: f64, inf: f64, spatial: bool) {
    let scale = segment_scale(keys, i, spatial);
    let dur = seg_secs(keys, i);
    let Some(k) = keys.get_mut(if out { i } else { i + 1 }) else { return };
    let eases: Vec<Ease> = scale.iter().map(|d| Ease { speed: n * d / dur, influence: inf.clamp(0.001, 1.0) }).collect();
    if out {
        k.out_interp = Interp::Bezier;
        k.out_ease = eases;
    } else {
        k.in_interp = Interp::Bezier;
        k.in_ease = eases;
    }
    k.auto_bezier = false;
    k.continuous = false;
}

/// Apply curve `c` to segment `i → i + 1`.
pub fn ease_segment(keys: &mut [Keyframe], i: usize, c: Curve, spatial: bool) {
    if i + 1 >= keys.len() {
        return;
    }
    set_side(keys, i, true, c.out_speed, c.out_influence, spatial);
    set_side(keys, i, false, c.in_speed, c.in_influence, spatial);
}

/// The curve segment `i → i + 1` presents (its dimension with the largest change).
pub fn read_segment(keys: &[Keyframe], i: usize, spatial: bool) -> Option<Curve> {
    let scale = segment_scale(keys, i, spatial);
    let d = (0..scale.len()).max_by(|a, b| scale.get(*a).map_or(0.0, |v| v.abs()).total_cmp(&scale.get(*b).map_or(0.0, |v| v.abs())))?;
    let dv = *scale.get(d)?;
    let dur = seg_secs(keys, i);
    let norm = |e: Ease| if dv.abs() > 1e-12 { e.speed * dur / dv } else { 0.0 };
    let o = side_ease(keys, i, d, spatial, true)?;
    let n = side_ease(keys, i + 1, d, spatial, false)?;
    Some(Curve { out_influence: o.influence, out_speed: norm(o), in_influence: n.influence, in_speed: norm(n) })
}

/// Selected keys grouped by property, as stored indices (sorted).
fn selected_indices(s: &Session, cid: ItemId) -> Vec<(LayerId, Uid, Vec<usize>)> {
    let mut g: std::collections::BTreeMap<(LayerId, Uid), Vec<usize>> = Default::default();
    let Some(comp) = s.project.comp(cid) else { return vec![] };
    for k in &s.state.selected_keys {
        let Some(pr) = comp.layer(k.layer).and_then(|l| l.props.find(k.prop)) else { continue };
        if let Some(i) = effectcraft_keyframe::key_at(&pr.keys, k.time) {
            g.entry((k.layer, k.prop)).or_default().push(i);
        }
    }
    g.into_iter()
        .map(|((l, u), mut v)| {
            v.sort_unstable();
            v.dedup();
            (l, u, v)
        })
        .collect()
}

/// The segments a selection covers: between consecutive selected keys, or both sides of a lone
/// selected key.
fn segments(sel: &[usize], n_keys: usize) -> Vec<usize> {
    match sel {
        [] => vec![],
        [i] => [i.checked_sub(1), (i + 1 < n_keys).then_some(*i)].into_iter().flatten().collect(),
        _ => sel.windows(2).filter_map(|w| (w[1] == w[0] + 1).then_some(w[0])).collect(),
    }
}

fn ease(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "motion.ease";
    let c = curve_p(p, C)?;
    let cid = s.active_comp_id().ok_or(EngineError::NoComp)?;
    let sel = selected_indices(s, cid);
    if sel.is_empty() {
        return Err(bad(C, "select keyframes to ease"));
    }
    let n = s.edit("Apply Ease Curve", super::merge_p(p), |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let mut n = 0;
        for (lid, uid, idx) in &sel {
            let Some(pr) = comp.layer_mut(*lid).and_then(|l| l.props.find_mut(*uid)) else { continue };
            if !pr.value.interpolates() {
                continue;
            }
            let spatial = pr.spatial;
            for i in segments(idx, pr.keys.len()) {
                ease_segment(&mut pr.keys, i, c, spatial);
                n += 1;
            }
        }
        Ok(n)
    })?;
    if n == 0 {
        return Err(bad(C, "select two neighbouring keyframes (or one key with a neighbour)"));
    }
    Ok(json!({"segments": n}))
}

fn read_ease(s: &mut Session, _: &Value) -> Result<Value> {
    let cid = s.active_comp_id().ok_or(EngineError::NoComp)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    for (lid, uid, idx) in selected_indices(s, cid) {
        let Some(pr) = comp.layer(lid).and_then(|l| l.props.find(uid)) else { continue };
        for i in segments(&idx, pr.keys.len()) {
            if let Some(c) = read_segment(&pr.keys, i, pr.spatial) {
                return Ok(curve_json(c));
            }
        }
    }
    Err(bad("motion.readEase", "select two neighbouring keyframes with an interpolated segment"))
}

// ---------------------------------------------------------------- compound edits

/// Run `f` (which may execute other commands) as one undo step named `label`; on failure
/// everything is rolled back. The inner commands leave no journal entries of their own: the
/// outer command is journaled.
fn compound(s: &mut Session, label: &str, f: impl FnOnce(&mut Session) -> Result<Value>) -> Result<Value> {
    let (project, history, state) = (s.project.clone(), s.history.clone(), s.state.clone());
    let journal = s.journal.len();
    let r = f(s);
    s.journal.truncate(journal);
    match r {
        Ok(v) => {
            super::batch::fold(s, &project, label);
            Ok(v)
        }
        Err(e) => {
            s.project = project;
            s.history = history;
            s.state = state;
            s.sanitize_state();
            s.bump();
            Err(e)
        }
    }
}

/// The comp-space position of each layer's anchor point (2D layers by their transform chain).
fn comp_positions(s: &Session, cid: ItemId, ids: &[LayerId]) -> Vec<(LayerId, [f64; 2])> {
    let Some(comp) = s.project.comp(cid) else { return vec![] };
    let ectx =
        effectcraft_render::EvalCtx { project: &s.project, comp_id: cid, comp, time: s.time(), expr: s.expr.as_deref(), footage: Some(s.footage.as_ref()) };
    ids.iter()
        .filter_map(|id| {
            let l = comp.layer(*id)?;
            let lt = l.layer_time(s.time());
            let tr = l.transform()?;
            if l.is_3d() {
                let p = tr.get("position").map(|p| p.value_at(lt).as_vec3())?;
                return Some((*id, [p[0], p[1]]));
            }
            let a = tr.get("anchor").map(|p| p.value_at(lt).as_vec3()).unwrap_or([0.0; 3]);
            let m = ectx.layer_to_comp(l).0;
            let q = m.apply(effectcraft_geom::vec2(a[0], a[1]));
            Some((*id, [q.x, q.y]))
        })
        .collect()
}

/// Set a layer's Position (now, or its static value) to comp point `at`, keeping Z.
fn place(pg: &mut PropGroup, lt: Tick, at: [f64; 2]) {
    if pg.get("positionX").is_some() {
        for (m, v) in [("positionX", at[0]), ("positionY", at[1])] {
            if let Some(pr) = pg.get_mut(m) {
                pr.set_value_at(lt, KV::Scalar(v));
            }
        }
    } else if let Some(pr) = pg.get_mut("position") {
        let mut c = pr.value_at(lt).components();
        c.resize(c.len().max(2), 0.0);
        c[0] = at[0];
        c[1] = at[1];
        let v = pr.value.with_components(&c);
        pr.set_value_at(lt, v);
    }
}

fn new_null_at(s: &mut Session, cid: ItemId, name: &str, at: [f64; 2]) -> Result<LayerId> {
    let unique = s.project.comp(cid).ok_or(EngineError::NoComp)?.unique_layer_name(name);
    let r = s.execute("layer.newNull", json!({"comp": cid.0, "name": unique}))?;
    let id = LayerId(r["layer"].as_u64().ok_or_else(|| bad("motion.null", "the null was not created"))?);
    let t = s.time();
    s.edit("Position", None, |proj, _| {
        let l = layer_mut(proj, cid, id)?;
        let lt = l.layer_time(t);
        if let Some(tr) = l.transform_mut() {
            place(tr, lt, at);
        }
        Ok(())
    })?;
    Ok(id)
}

fn null(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    if ids.is_empty() {
        return Err(bad("motion.null", "select the layers to parent to a new null"));
    }
    let pos = comp_positions(s, cid, &ids);
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let center = if pos.is_empty() {
        [comp.width as f64 / 2.0, comp.height as f64 / 2.0]
    } else {
        let n = pos.len() as f64;
        [pos.iter().map(|x| x.1[0]).sum::<f64>() / n, pos.iter().map(|x| x.1[1]).sum::<f64>() / n]
    };
    let name = str_p(p, "name").unwrap_or("Control").to_string();
    // Parent only the layers whose current parent isn't also being parented (keeps hierarchies).
    let top: Vec<u64> = ids.iter().filter(|id| comp.layer(**id).is_some_and(|l| l.parent.is_none_or(|q| !ids.contains(&q)))).map(|l| l.0).collect();
    compound(s, "Null to Selection", |s| {
        let nl = new_null_at(s, cid, &name, center)?;
        s.execute("layer.setParent", json!({"comp": cid.0, "layers": top, "parent": nl.0}))?;
        s.state.selected_layers = vec![nl];
        Ok(json!({"layer": nl.0, "position": center, "children": top}))
    })
}

// ---------------------------------------------------------------- anchor grid

fn anchor(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "motion.anchor";
    let fx = f_p(p, "x").unwrap_or(0.5);
    let fy = f_p(p, "y").unwrap_or(0.5);
    if !(0.0..=1.0).contains(&fx) || !(0.0..=1.0).contains(&fy) {
        return Err(bad(c, "`x` and `y` are fractions of the content bounds, 0..1 (0 left/top, 0.5 centre, 1 right/bottom)"));
    }
    super::layer_menu::anchor_to(s, p, fx, fy, "Move Anchor Point", c)
}

// ---------------------------------------------------------------- build in / out

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BuildKind {
    Fade,
    Scale,
    Slide,
    Rotate,
}

fn frames_tick(fr: FrameRate, frames: f64) -> Tick {
    Tick::from_seconds_f64(frames / fr.as_f64().max(1e-6))
}

/// The changed ("away") value for a build: the state the layer builds in from / out to.
fn away(kind: BuildKind, prop: &str, v: &KV, slide: [f64; 2], rotate: f64) -> Option<KV> {
    let c = v.components();
    let out = match (kind, prop) {
        (BuildKind::Fade, "opacity") => vec![0.0],
        (BuildKind::Scale, "scale") => c.iter().map(|_| 0.0).collect(),
        (BuildKind::Slide, "position") => c.iter().enumerate().map(|(i, x)| x + slide.get(i).copied().unwrap_or(0.0)).collect(),
        (BuildKind::Slide, "positionX") => vec![c.first().copied().unwrap_or(0.0) + slide[0]],
        (BuildKind::Slide, "positionY") => vec![c.first().copied().unwrap_or(0.0) + slide[1]],
        (BuildKind::Rotate, "rotation") => vec![c.first().copied().unwrap_or(0.0) + rotate],
        _ => return None,
    };
    Some(v.with_components(&out))
}

/// Key `prop` from `from` at `t0` to `to` at `t1` (layer times), eased with `c`.
fn key_pair(pr: &mut Property, t0: Tick, from: KV, t1: Tick, to: KV, c: Curve) {
    effectcraft_keyframe::set_key(&mut pr.keys, Keyframe::new(t0, from));
    effectcraft_keyframe::set_key(&mut pr.keys, Keyframe::new(t1, to));
    if let Some(a) = effectcraft_keyframe::key_at(&pr.keys, t0)
        && effectcraft_keyframe::key_at(&pr.keys, t1) == Some(a + 1)
    {
        let spatial = pr.spatial;
        ease_segment(&mut pr.keys, a, c, spatial);
    }
}

fn build(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "motion.build";
    let kind = match str_p(p, "kind").unwrap_or("fade") {
        "fade" => BuildKind::Fade,
        "scale" => BuildKind::Scale,
        "slide" => BuildKind::Slide,
        "rotate" => BuildKind::Rotate,
        x => return Err(bad(C, format!("kind: fade|scale|slide|rotate, got `{x}`"))),
    };
    let (b_in, b_out) = match str_p(p, "mode").unwrap_or("in") {
        "in" => (true, false),
        "out" => (false, true),
        "both" => (true, true),
        x => return Err(bad(C, format!("mode: in|out|both, got `{x}`"))),
    };
    let frames = f_p(p, "frames").unwrap_or(12.0);
    if !(frames.is_finite() && frames >= 1.0) {
        return Err(bad(C, "`frames` (the build's length) must be at least 1"));
    }
    let dist = f_p(p, "distance").filter(|d| d.is_finite()).unwrap_or(300.0);
    let slide = match str_p(p, "from").unwrap_or("bottom") {
        "left" => [-dist, 0.0],
        "right" => [dist, 0.0],
        "top" => [0.0, -dist],
        "bottom" => [0.0, dist],
        x => return Err(bad(C, format!("from: left|right|top|bottom, got `{x}`"))),
    };
    let rotate = f_p(p, "angle").filter(|a| a.is_finite()).unwrap_or(-90.0);
    let c = curve_p(p, C)?;
    let (cid, ids) = layers_p(s, p)?;
    let ids = super::unlocked(s, cid, ids, C)?;
    if ids.is_empty() {
        return Err(bad(C, "select the layers to build"));
    }
    let fr = s.project.comp(cid).ok_or(EngineError::NoComp)?.frame_rate;
    let len = frames_tick(fr, frames);
    let label = match (b_in, b_out) {
        (true, true) => "Build In and Out",
        (true, false) => "Build In",
        _ => "Build Out",
    };
    let n = s.edit(label, None, |proj, _| {
        let mut n = 0;
        for lid in &ids {
            let l = layer_mut(proj, cid, *lid)?;
            let (lin, lout) = (l.in_point, l.out_point);
            let span = Tick(lout.0.saturating_sub(lin.0));
            // Both builds share the layer's span: each gets at most half.
            let d = if b_in && b_out { Tick(len.0.min(span.0 / 2)) } else { Tick(len.0.min(span.0)) };
            if d.0 <= 0 {
                continue;
            }
            let (lt_in, lt_in_end) = (l.layer_time(lin), l.layer_time(Tick(lin.0.saturating_add(d.0))));
            let (lt_out_start, lt_out) = (l.layer_time(Tick(lout.0.saturating_sub(d.0))), l.layer_time(lout));
            let Some(tr) = l.transform_mut() else { continue };
            let names: &[&str] = match kind {
                BuildKind::Fade => &["opacity"],
                BuildKind::Scale => &["scale"],
                BuildKind::Slide if tr.get("positionX").is_some() => &["positionX", "positionY"],
                BuildKind::Slide => &["position"],
                BuildKind::Rotate => &["rotation"],
            };
            for m in names {
                let Some(pr) = tr.get_mut(m) else { continue };
                if b_in {
                    let rest = pr.value_at(lt_in_end);
                    if let Some(a) = away(kind, m, &rest, slide, rotate) {
                        key_pair(pr, lt_in, a, lt_in_end, rest, c);
                        n += 1;
                    }
                }
                if b_out {
                    let rest = pr.value_at(lt_out_start);
                    // Out: the mirror move (slides leave the way they came in, rotations reverse).
                    let (sl, ro) = ([-slide[0], -slide[1]], -rotate);
                    if let Some(a) = away(kind, m, &rest, sl, ro) {
                        key_pair(pr, lt_out_start, rest, lt_out, a, c);
                        n += 1;
                    }
                }
            }
        }
        Ok(n)
    })?;
    Ok(json!({"builds": n}))
}

// ---------------------------------------------------------------- dynamics

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dynamics {
    /// Overshoot the end value and settle with a decaying oscillation.
    Elastic,
    /// Hit the end value and bounce back toward the start a few times.
    Bounce,
}

/// Dynamics settings.
#[derive(Clone, Copy, Debug)]
pub struct DynOpts {
    pub mode: Dynamics,
    /// First overshoot / bounce height as a fraction of the move.
    pub amplitude: f64,
    /// Oscillations per second (elastic) or bounces per second (first bounce).
    pub frequency: f64,
    /// Each swing's height relative to the previous one (0..1).
    pub decay: f64,
    /// Most swings added.
    pub max: usize,
}

/// Keys after key `i` (which ends a move from key `i − 1`): the new keys (comp frames snapped
/// by `fr` through the layer-time maps), stopping before `limit` (the next key's time).
fn dynamics_keys(keys: &[Keyframe], i: usize, o: &DynOpts, fr: FrameRate, to_comp: &dyn Fn(Tick) -> Tick, to_layer: &dyn Fn(Tick) -> Tick) -> Vec<Keyframe> {
    let (Some(prev), Some(end)) = (i.checked_sub(1).and_then(|j| keys.get(j)), keys.get(i)) else { return vec![] };
    let (a, b) = (prev.value.components(), end.value.components());
    if a.is_empty() || a.len() != b.len() {
        return vec![];
    }
    let delta: Vec<f64> = a.iter().zip(&b).map(|(x, y)| y - x).collect();
    if delta.iter().all(|d| d.abs() < 1e-9) {
        return vec![];
    }
    let limit = keys.get(i + 1).map(|k| k.time);
    let fps = fr.as_f64().max(1e-6);
    let f_end = fr.frame_at(to_comp(end.time));
    let at = |secs: f64| -> i64 { f_end.saturating_add((secs * fps).round() as i64) };
    let val = |k: f64| end.value.with_components(&b.iter().zip(&delta).map(|(v, d)| v + d * k).collect::<Vec<_>>());
    // (frame, value as a fraction of the move past the end, apex?)
    let mut plan: Vec<(i64, f64, bool)> = vec![];
    let half = 0.5 / o.frequency.max(0.01);
    match o.mode {
        Dynamics::Elastic => {
            let mut amp = o.amplitude;
            for k in 1..=o.max {
                if amp.abs() < 0.005 {
                    break;
                }
                let sign = if k % 2 == 1 { 1.0 } else { -1.0 };
                // Peaks are a quarter period after the pass, then every half period.
                plan.push((at(half * (k as f64 - 0.5)), sign * amp, true));
                amp *= o.decay;
            }
            if let Some(last) = plan.last().map(|x| x.0) {
                plan.push((last.saturating_add((half * 0.5 * fps).round().max(1.0) as i64), 0.0, false));
            }
        }
        Dynamics::Bounce => {
            let (mut amp, mut dur, mut t) = (o.amplitude, half * 2.0, 0.0);
            for _ in 0..o.max {
                if amp.abs() < 0.005 {
                    break;
                }
                plan.push((at(t + dur / 2.0), -amp, true));
                plan.push((at(t + dur), 0.0, false));
                t += dur;
                amp *= o.decay;
                // A bounce's duration goes with the square root of its height.
                dur *= o.decay.sqrt();
            }
        }
    }
    let mut out: Vec<Keyframe> = vec![];
    let mut last_f = f_end;
    for (f, k, apex) in plan {
        if f <= last_f {
            break;
        }
        let t = to_layer(fr.tick_of(f));
        if limit.is_some_and(|l| t >= l) {
            break;
        }
        last_f = f;
        let mut key = Keyframe::new(t, val(k));
        key.spatial_auto = true;
        if apex {
            key = key.eased();
        }
        out.push(key);
    }
    // Elastic ends at rest on the value: drop a trailing swing that the next key cut off.
    if o.mode == Dynamics::Elastic && out.last().is_some_and(|k| k.value != end.value) {
        while out.last().is_some_and(|k| k.value != end.value) {
            out.pop();
        }
    }
    out
}

fn dynamics(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "motion.dynamics";
    let mode = match str_p(p, "mode").unwrap_or("elastic") {
        "elastic" | "overshoot" => Dynamics::Elastic,
        "bounce" => Dynamics::Bounce,
        x => return Err(bad(C, format!("mode: elastic|bounce, got `{x}`"))),
    };
    let fin = |k: &str, d: f64| f_p(p, k).filter(|v| v.is_finite()).unwrap_or(d);
    let o = DynOpts {
        mode,
        amplitude: (fin("amplitude", 30.0) / 100.0).clamp(0.0, 5.0),
        frequency: fin("frequency", if mode == Dynamics::Bounce { 2.0 } else { 3.0 }).clamp(0.05, 60.0),
        decay: (fin("decay", 50.0) / 100.0).clamp(0.0, 0.95),
        max: p.get("count").and_then(Value::as_u64).unwrap_or(6).clamp(1, 32) as usize,
    };
    let cid = s.active_comp_id().ok_or(EngineError::NoComp)?;
    let sel = selected_indices(s, cid);
    let mut added = 0usize;
    s.edit(if mode == Dynamics::Bounce { "Bounce" } else { "Overshoot" }, None, |proj, st| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let fr = comp.frame_rate;
        for (lid, uid, idx) in &sel {
            let Some(l) = comp.layer_mut(*lid) else { continue };
            let lc = l.clone();
            let (to_comp, to_layer) = (|t: Tick| lc.comp_time(t), |t: Tick| lc.layer_time(t));
            let Some(pr) = l.props.find_mut(*uid) else { continue };
            if !pr.value.interpolates() {
                continue;
            }
            // Later keys first so earlier indices stay valid.
            for &i in idx.iter().rev() {
                let new = dynamics_keys(&pr.keys, i, &o, fr, &to_comp, &to_layer);
                if new.is_empty() {
                    continue;
                }
                // The key the move ends on is passed through, not stopped at.
                if let Some(k) = pr.keys.get_mut(i)
                    && mode == Dynamics::Elastic
                {
                    k.out_interp = Interp::Bezier;
                    k.in_interp = Interp::Bezier;
                    k.auto_bezier = true;
                    k.in_ease.clear();
                    k.out_ease.clear();
                }
                added += new.len();
                for k in new {
                    st.selected_keys.push(KeyRef { layer: *lid, prop: *uid, time: k.time });
                    effectcraft_keyframe::set_key(&mut pr.keys, k);
                }
                // Bounce contacts leave as a ball does: shaped like a parabola (speed 2 × the
                // average into the apex, which has none).
                if mode == Dynamics::Bounce {
                    let end = pr.keys.get(i).map(|k| k.time);
                    let spatial = pr.spatial;
                    let mut j = i;
                    while j + 1 < pr.keys.len() && pr.keys.get(j + 1).is_some_and(|k| Some(k.time) > end) {
                        let apex_next = pr.keys.get(j + 1).is_some_and(|k| k.in_interp == Interp::Bezier && k.in_ease.first().is_some_and(|e| e.speed == 0.0));
                        if apex_next {
                            set_side(&mut pr.keys, j, true, 2.0, 1.0 / 3.0, spatial);
                        } else {
                            set_side(&mut pr.keys, j, false, 2.0, 1.0 / 3.0, spatial);
                        }
                        j += 1;
                        if j > i + 2 * o.max {
                            break;
                        }
                    }
                }
            }
        }
        st.selected_keys.sort_by_key(|k| (k.layer, k.prop, k.time));
        st.selected_keys.dedup();
        Ok(())
    })?;
    if added == 0 {
        return Err(bad(C, "select keyframes that end a move (a key with a different key before it)"));
    }
    Ok(json!({"keys": added}))
}

// ---------------------------------------------------------------- delay

fn delay(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "motion.delay";
    let frames = f_p(p, "frames").filter(|v| v.is_finite()).unwrap_or(2.0).clamp(-10_000.0, 10_000.0);
    let reverse = str_p(p, "order") == Some("reverse");
    let what = str_p(p, "what").unwrap_or("auto");
    let cid = s.active_comp_id().ok_or(EngineError::NoComp)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let fr = comp.frame_rate;
    // Layers in selection order (then stacking order for layers with keys selected only).
    let mut order: Vec<LayerId> = s.state.selected_layers.clone();
    let use_keys = match what {
        "keys" => true,
        "layers" => false,
        "auto" => !s.state.selected_keys.is_empty(),
        x => return Err(bad(C, format!("what: auto|keys|layers, got `{x}`"))),
    };
    if use_keys {
        for l in &comp.layers {
            if s.state.selected_keys.iter().any(|k| k.layer == l.id) && !order.contains(&l.id) {
                order.push(l.id);
            }
        }
        order.retain(|l| s.state.selected_keys.iter().any(|k| k.layer == *l));
    }
    if let Some(ids) = p.get("layers").and_then(Value::as_array) {
        order = ids.iter().filter_map(|v| resolve_layer(comp, v)).collect();
    }
    if reverse {
        order.reverse();
    }
    if order.len() < 2 {
        return Err(bad(C, "select two or more layers (or keys on two or more layers) to stagger"));
    }
    let step = frames_tick(fr, frames);
    let order = super::unlocked(s, cid, order, C)?;
    s.edit("Delay", None, |proj, st| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let mut sel = vec![];
        for (n, lid) in order.iter().enumerate() {
            let shift = Tick(step.0.saturating_mul(n as i64));
            let Some(l) = comp.layer_mut(*lid) else { continue };
            if !use_keys {
                for t in [&mut l.start_time, &mut l.in_point, &mut l.out_point] {
                    *t = Tick(t.0.saturating_add(shift.0));
                }
                continue;
            }
            // Keys move in layer time (a stretched layer moves them by its own amount).
            let lshift = Tick(l.layer_time(Tick(l.start_time.0.saturating_add(shift.0))).0);
            let mine: Vec<&KeyRef> = st.selected_keys.iter().filter(|k| k.layer == *lid).collect();
            let mut by_prop: std::collections::BTreeMap<Uid, Vec<Tick>> = Default::default();
            for k in mine {
                by_prop.entry(k.prop).or_default().push(k.time);
            }
            for (uid, times) in by_prop {
                let Some(pr) = l.props.find_mut(uid) else { continue };
                let mut moved = vec![];
                pr.keys.retain(|k| {
                    if times.contains(&k.time) {
                        moved.push(k.clone());
                        false
                    } else {
                        true
                    }
                });
                for mut k in moved {
                    k.time = Tick(k.time.0.saturating_add(lshift.0));
                    sel.push(KeyRef { layer: *lid, prop: uid, time: k.time });
                    effectcraft_keyframe::set_key(&mut pr.keys, k);
                }
            }
        }
        if use_keys {
            st.selected_keys = sel;
        }
        Ok(())
    })?;
    Ok(json!({"layers": order.len(), "what": if use_keys { "keys" } else { "layers" }}))
}

// ---------------------------------------------------------------- expression rigs

fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

/// Add a Slider Control named `name` (value `v`) to `layer`'s effects.
fn add_slider(proj: &mut effectcraft_project::Project, cid: ItemId, lid: LayerId, name: &str, v: f64) -> Result<()> {
    let spec = effectcraft_effects::lookup("ec.control.slider").ok_or_else(|| bad("motion", "the Slider Control effect is missing"))?;
    let mut next = proj.next_id;
    let l = layer_mut(proj, cid, lid)?;
    let fx = l.props.sub_mut("effects").ok_or_else(|| bad("motion", "this layer type has no effects"))?;
    let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), name, [100.0, 100.0]);
    if let Some(pr) = g.get_mut("slider") {
        pr.value = KV::Scalar(v);
    }
    fx.children.push(g.into());
    proj.next_id = next;
    Ok(())
}

fn set_expression(proj: &mut effectcraft_project::Project, cid: ItemId, lid: LayerId, prop: &str, text: String) -> Result<bool> {
    let l = layer_mut(proj, cid, lid)?;
    let Some(pr) = l.transform_mut().and_then(|tr| tr.get_mut(prop)) else { return Ok(false) };
    pr.expr = Some(Expression { text, enabled: true });
    Ok(true)
}

/// This layer's position in comp space without reading its own transform (a property's
/// expression can't depend on the property itself).
const MY_POS: &str = "(hasParent ? parent.toComp(transform.position) : transform.position)";

fn spin(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "motion.spin";
    let speed = f_p(p, "speed").filter(|v| v.is_finite()).unwrap_or(90.0);
    let (cid, ids) = layers_p(s, p)?;
    let ids = super::unlocked(s, cid, ids, C)?;
    if ids.is_empty() {
        return Err(bad(C, "select the layers to spin"));
    }
    s.edit("Spin", None, |proj, _| {
        for lid in &ids {
            add_slider(proj, cid, *lid, "Spin Speed", speed)?;
            set_expression(proj, cid, *lid, "rotation", "value + time * effect(\"Spin Speed\")(\"Slider\")".into())?;
        }
        Ok(())
    })?;
    Ok(json!({"layers": ids.len()}))
}

fn stare(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "motion.stare";
    let (cid, ids) = layers_p(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let target = p.get("target").and_then(|v| resolve_layer(comp, v)).ok_or_else(|| bad(C, "missing `target`: the layer to look at"))?;
    let tname = comp.layer(target).map(|l| l.name.clone()).ok_or_else(|| bad(C, "no such target layer"))?;
    let offset = f_p(p, "offset").filter(|v| v.is_finite()).unwrap_or(0.0);
    let ids: Vec<LayerId> = ids.into_iter().filter(|l| *l != target).collect();
    let ids = super::unlocked(s, cid, ids, C)?;
    if ids.is_empty() {
        return Err(bad(C, "select the layers that should look at the target (not the target itself)"));
    }
    let text = format!(
        "var t = thisComp.layer({});\nvar a = {MY_POS};\nvar b = t.hasParent ? t.parent.toComp(t.transform.position) : t.transform.position;\nvar d = sub(b, a);\nradiansToDegrees(Math.atan2(d[1], d[0])) + ({offset})",
        quote(&tname)
    );
    s.edit("Stare", None, |proj, _| {
        for lid in &ids {
            set_expression(proj, cid, *lid, "rotation", text.clone())?;
        }
        Ok(())
    })?;
    Ok(json!({"layers": ids.len(), "target": target.0}))
}

fn falloff(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "motion.falloff";
    let props: Vec<String> = match p.get("props") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        Some(Value::String(x)) => vec![x.clone()],
        _ => vec!["scale".into()],
    };
    for pr in &props {
        if !["scale", "opacity", "rotation"].contains(&pr.as_str()) {
            return Err(bad(C, format!("props: scale|opacity|rotation, got `{pr}`")));
        }
    }
    let radius = f_p(p, "radius").filter(|v| v.is_finite()).unwrap_or(200.0).max(0.0);
    let feather = f_p(p, "feather").filter(|v| v.is_finite()).unwrap_or(200.0).max(0.0);
    let invert = b_p(p, "invert").unwrap_or(false);
    let (cid, ids) = layers_p(s, p)?;
    let ids = super::unlocked(s, cid, ids, C)?;
    if ids.is_empty() {
        return Err(bad(C, "select the layers the falloff affects"));
    }
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let center = [comp.width as f64 / 2.0, comp.height as f64 / 2.0];
    compound(s, "Falloff", |s| {
        let nl = new_null_at(s, cid, "Falloff", center)?;
        let name = s.project.comp(cid).and_then(|c| c.layer(nl)).map(|l| l.name.clone()).unwrap_or_default();
        let k = if invert { "k" } else { "1 - k" };
        let text = format!(
            "var f = thisComp.layer({});\nvar r = f.effect(\"Radius\")(\"Slider\");\nvar w = Math.max(f.effect(\"Feather\")(\"Slider\"), 0.001);\nvar d = length({MY_POS}, f.hasParent ? f.parent.toComp(f.transform.position) : f.transform.position);\nvar k = linear(d, r, r + w, 0, 1);\nmul(value, {k})",
            quote(&name)
        );
        let mut n = 0;
        s.edit("Falloff", None, |proj, _| {
            add_slider(proj, cid, nl, "Feather", feather)?;
            add_slider(proj, cid, nl, "Radius", radius)?;
            for lid in &ids {
                for m in &props {
                    if set_expression(proj, cid, *lid, m, text.clone())? {
                        n += 1;
                    }
                }
            }
            Ok(())
        })?;
        Ok(json!({"layer": nl.0, "properties": n}))
    })
}

fn clone(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "motion.clone";
    let count = p.get("count").and_then(Value::as_u64).unwrap_or(3).clamp(1, 200) as i64;
    let off = match p.get("offset").and_then(Value::as_array) {
        Some(a) => [a.first().and_then(Value::as_f64).unwrap_or(0.0), a.get(1).and_then(Value::as_f64).unwrap_or(0.0)],
        None => [50.0, 0.0],
    };
    if !off.iter().all(|v| v.is_finite()) {
        return Err(bad(C, "`offset` must be [x, y] numbers"));
    }
    let frames = f_p(p, "frames").filter(|v| v.is_finite()).unwrap_or(0.0).clamp(-10_000.0, 10_000.0);
    let (cid, ids) = layers_p(s, p)?;
    if ids.is_empty() {
        return Err(bad(C, "select the layers to clone"));
    }
    let fr = s.project.comp(cid).ok_or(EngineError::NoComp)?.frame_rate;
    let step = frames_tick(fr, frames);
    compound(s, "Clone", |s| {
        let mut made = vec![];
        for id in &ids {
            for k in 1..=count {
                let r = s.execute("edit.duplicate", json!({"comp": cid.0, "layers": [id.0]}))?;
                let Some(new) = r.as_array().and_then(|a| a.first()).and_then(Value::as_u64) else { continue };
                made.push((LayerId(new), k));
            }
        }
        s.edit("Offset Clones", None, |proj, st| {
            for (lid, k) in &made {
                let l = layer_mut(proj, cid, *lid)?;
                let shift = Tick(step.0.saturating_mul(*k));
                for t in [&mut l.start_time, &mut l.in_point, &mut l.out_point] {
                    *t = Tick(t.0.saturating_add(shift.0));
                }
                let d = [off[0] * *k as f64, off[1] * *k as f64];
                let Some(tr) = l.transform_mut() else { continue };
                let offset = |pr: &mut Property, dd: &[f64]| {
                    let f = |v: &KV| {
                        let c: Vec<f64> = v.components().iter().enumerate().map(|(i, x)| x + dd.get(i).copied().unwrap_or(0.0)).collect();
                        v.with_components(&c)
                    };
                    pr.value = f(&pr.value);
                    for key in &mut pr.keys {
                        key.value = f(&key.value);
                    }
                };
                if tr.get("positionX").is_some() {
                    if let Some(pr) = tr.get_mut("positionX") {
                        offset(pr, &[d[0]]);
                    }
                    if let Some(pr) = tr.get_mut("positionY") {
                        offset(pr, &[d[1]]);
                    }
                } else if let Some(pr) = tr.get_mut("position") {
                    offset(pr, &d);
                }
            }
            st.selected_layers = made.iter().map(|x| x.0).collect();
            Ok(())
        })?;
        Ok(json!({"layers": made.iter().map(|x| x.0.0).collect::<Vec<_>>()}))
    })
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "motion.ease",
            "Apply Ease Curve",
            [],
            None,
            "{preset?: linear|easeIn|easeOut|easy|smooth|strong|snap|anticipate|back, outInfluence? %, outSpeed? (1 = linear, 0 = stop), inInfluence? %, inSpeed?, merge?} → the selected keyframes' segments (between neighbouring selected keys, or both sides of a lone key) take the curve",
            has_keys,
            ease
        ),
        query!("motion.readEase", "Read Ease Curve", "{} → {outInfluence %, outSpeed, inInfluence %, inSpeed} of the first selected segment", read_ease),
        cmd!(
            "motion.anchor",
            "Move Anchor Point",
            [],
            None,
            "{layers?, x: 0|0.5|1, y: 0|0.5|1 (fractions of the content bounds: 0 left/top, 1 right/bottom)} → the layers stay where they are",
            has_layers,
            anchor
        ),
        cmd!("motion.null", "Null to Selection", [], None, "{layers?, name?} → a null at the layers' centre, parenting them", has_layers, null),
        cmd!(
            "motion.build",
            "Build In / Out",
            [],
            None,
            "{layers?, kind: fade|scale|slide|rotate, mode?: in|out|both, frames? (default 12), from?: left|right|top|bottom (slide), distance? px (300), angle? ° (-90), preset? | outInfluence?, outSpeed?, inInfluence?, inSpeed? (the ease)}",
            has_layers,
            build
        ),
        cmd!(
            "motion.dynamics",
            "Add Dynamics",
            [],
            None,
            "{mode?: elastic|bounce, amplitude? % of the move (30), frequency? per s, decay? % (50), count? (6)} → swings after each selected key that ends a move",
            has_keys,
            dynamics
        ),
        cmd!(
            "motion.delay",
            "Stagger (Delay)",
            [],
            None,
            "{frames? (2, per layer), order?: selection|reverse, what?: auto|keys|layers, layers? (order)} → the selected keys of each layer (or the layers) move by n × frames",
            has_comp,
            delay
        ),
        cmd!(
            "motion.falloff",
            "Falloff",
            [],
            None,
            "{layers?, props?: [scale|opacity|rotation], radius? (200), feather? (200), invert?} → a Falloff null; near it the properties keep their value, beyond radius + feather they reach 0",
            has_layers,
            falloff
        ),
        cmd!("motion.clone", "Clone", [], None, "{layers?, count? (3), offset? [x, y] per copy ([50, 0]), frames? per copy (0)}", has_layers, clone),
        cmd!("motion.spin", "Spin", [], None, "{layers?, speed? °/s (90)} → Rotation turns continuously (a Spin Speed slider on the layer)", has_layers, spin),
        cmd!("motion.stare", "Stare", [], None, "{layers?, target: layer, offset? °} → Rotation points at the target", has_layers, stare),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(t: f64, v: f64) -> Keyframe {
        Keyframe::new(Tick::from_seconds_f64(t), KV::Scalar(v))
    }

    #[test]
    fn ease_round_trips_through_read() {
        let mut keys = vec![k(0.0, 0.0), k(1.0, 100.0)];
        let c = Curve { out_influence: 0.6, out_speed: 0.2, in_influence: 0.25, in_speed: 0.0 };
        ease_segment(&mut keys, 0, c, false);
        let r = read_segment(&keys, 0, false).unwrap();
        assert!((r.out_influence - 0.6).abs() < 1e-9 && (r.out_speed - 0.2).abs() < 1e-9, "{r:?}");
        assert!((r.in_influence - 0.25).abs() < 1e-9 && r.in_speed.abs() < 1e-9, "{r:?}");
        // The keyframe curve follows the normalised curve.
        for x in [0.1, 0.3, 0.5, 0.8] {
            let v = effectcraft_keyframe::evaluate(&keys, Tick::from_seconds_f64(x), false).unwrap().as_f64();
            assert!((v / 100.0 - curve_progress(c, x)).abs() < 1e-6, "{x}: {v}");
        }
    }

    #[test]
    fn segments_of_a_selection() {
        assert_eq!(segments(&[1], 3), vec![0, 1]);
        assert_eq!(segments(&[0], 1), Vec::<usize>::new());
        assert_eq!(segments(&[0, 1, 3], 4), vec![0]);
        assert_eq!(segments(&[2, 3, 4], 5), vec![2, 3]);
    }

    #[test]
    fn presets_are_valid_curves() {
        for (id, _, c) in PRESETS {
            let p = curve_p(&json!({"preset": id}), "t").unwrap();
            assert_eq!(p, *c);
            assert!((curve_progress(p, 1.0) - 1.0).abs() < 1e-9 && curve_progress(p, 0.0).abs() < 1e-9, "{id}");
        }
        assert!(curve_p(&json!({"preset": "nope"}), "t").is_err());
        assert!(curve_p(&json!({"outSpeed": f64::NAN}), "t").is_ok(), "NaN is not JSON: serde makes it null");
    }

    #[test]
    fn elastic_overshoots_and_settles() {
        let keys = vec![k(0.0, 0.0), k(1.0, 100.0)];
        let fr = FrameRate::new(30, 1);
        let o = DynOpts { mode: Dynamics::Elastic, amplitude: 0.3, frequency: 3.0, decay: 0.5, max: 4 };
        let new = dynamics_keys(&keys, 1, &o, fr, &|t| t, &|t| t);
        let v: Vec<f64> = new.iter().map(|k| k.value.as_f64()).collect();
        assert_eq!(v.len(), 5, "{v:?}");
        assert!((v[0] - 130.0).abs() < 1e-9 && (v[1] - 85.0).abs() < 1e-9 && (v[4] - 100.0).abs() < 1e-9, "{v:?}");
        assert!(new.windows(2).all(|w| w[0].time < w[1].time));
        // A next key cuts the swings short but they still end at rest.
        let mut keys = keys;
        keys.push(k(1.2, 0.0));
        let new = dynamics_keys(&keys, 1, &o, fr, &|t| t, &|t| t);
        assert!(new.iter().all(|k| k.time < keys[2].time));
        assert!(new.last().is_none_or(|k| k.value.as_f64() == 100.0));
    }

    #[test]
    fn bounce_returns_to_the_end_value() {
        let keys = vec![k(0.0, 0.0), k(1.0, 100.0)];
        let o = DynOpts { mode: Dynamics::Bounce, amplitude: 0.4, frequency: 2.0, decay: 0.5, max: 3 };
        let new = dynamics_keys(&keys, 1, &o, FrameRate::new(30, 1), &|t| t, &|t| t);
        let v: Vec<f64> = new.iter().map(|k| k.value.as_f64()).collect();
        assert_eq!(v, vec![60.0, 100.0, 80.0, 100.0, 90.0, 100.0]);
    }

    #[test]
    fn dynamics_ignore_unusable_keys() {
        let o = DynOpts { mode: Dynamics::Elastic, amplitude: 0.3, frequency: 3.0, decay: 0.5, max: 4 };
        let fr = FrameRate::new(30, 1);
        assert!(dynamics_keys(&[k(0.0, 5.0)], 0, &o, fr, &|t| t, &|t| t).is_empty());
        assert!(dynamics_keys(&[k(0.0, 5.0), k(1.0, 5.0)], 1, &o, fr, &|t| t, &|t| t).is_empty());
        assert!(dynamics_keys(&[k(0.0, 5.0), k(1.0, 6.0)], 9, &o, fr, &|t| t, &|t| t).is_empty());
    }
}
