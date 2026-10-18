//! Window ▸ Motion Tools commands (`motion.*`).

use effectcraft_keyframe::{Interp, evaluate};
use effectcraft_project::LayerId;
use effectcraft_time::Tick;
use serde_json::json;

use crate::Session;
use crate::tests_timeline::{animate, prop, setup};

fn select(s: &mut Session, layer: u64, path: &str) {
    s.execute("prop.select", json!({"layer": layer, "path": path})).unwrap();
}

fn layer(s: &Session, id: u64) -> effectcraft_project::Layer {
    s.active_comp().unwrap().layer(LayerId(id)).unwrap().clone()
}

fn solid(s: &mut Session, name: &str) -> u64 {
    s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 100, "height": 100, "name": name})).unwrap()["layer"].as_u64().unwrap()
}

#[test]
fn ease_applies_the_curve_and_reads_it_back() {
    let (mut s, l) = setup();
    animate(&mut s, l, "transform/opacity", &[(0.0, json!(0)), (1.0, json!(100)), (2.0, json!(50))]);
    select(&mut s, l, "transform/opacity");
    let r = s.execute("motion.ease", json!({"outInfluence": 70, "outSpeed": 0, "inInfluence": 20, "inSpeed": 0.5})).unwrap();
    assert_eq!(r["segments"], 2);
    let o = prop(&s, l, "transform/opacity");
    assert!(o.keys.iter().all(|k| k.in_interp == Interp::Bezier || k.time == Tick::ZERO));
    let c = s.execute("motion.readEase", json!({})).unwrap();
    assert!((c["outInfluence"].as_f64().unwrap() - 70.0).abs() < 1e-6, "{c}");
    assert!((c["inSpeed"].as_f64().unwrap() - 0.5).abs() < 1e-6, "{c}");
    // The second segment goes down: the same normalised curve.
    let mid = evaluate(&o.keys, Tick::from_seconds_f64(1.5), false).unwrap().as_f64();
    let expect = 100.0 - 50.0 * crate::commands::motion::curve_progress(crate::commands::motion::curve_p(&c, "t").unwrap(), 0.5);
    assert!((mid - expect).abs() < 1e-6, "{mid} vs {expect}");
    // One undo step.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(prop(&s, l, "transform/opacity").keys.iter().all(|k| k.out_interp == Interp::Linear));
}

#[test]
fn ease_on_position_uses_the_path_length() {
    let (mut s, l) = setup();
    animate(&mut s, l, "transform/position", &[(0.0, json!([0, 0])), (1.0, json!([300, 400]))]);
    select(&mut s, l, "transform/position");
    s.execute("motion.ease", json!({"preset": "linear"})).unwrap();
    let p = prop(&s, l, "transform/position");
    // Linear speed along a 500 px path in 1 s.
    assert!((p.keys[0].out_ease[0].speed - 500.0).abs() < 1e-6, "{:?}", p.keys[0].out_ease);
    let v = evaluate(&p.keys, Tick::from_seconds_f64(0.5), true).unwrap().as_vec2();
    assert!((v[0] - 150.0).abs() < 0.5 && (v[1] - 200.0).abs() < 0.5, "{v:?}");
}

#[test]
fn ease_needs_keys_and_rejects_bad_presets() {
    let (mut s, l) = setup();
    assert!(s.execute("motion.ease", json!({})).is_err());
    animate(&mut s, l, "transform/opacity", &[(0.0, json!(0)), (1.0, json!(100))]);
    select(&mut s, l, "transform/opacity");
    assert!(s.execute("motion.ease", json!({"preset": "wobble"})).is_err());
    assert!(s.execute("motion.ease", json!({"preset": "back"})).is_ok());
}

#[test]
fn anchor_grid_moves_the_anchor_but_not_the_layer() {
    let (mut s, l) = setup();
    let before = layer(&s, l).props.prop("transform/position").unwrap().value.as_vec3();
    s.execute("motion.anchor", json!({"layers": [l], "x": 0, "y": 1})).unwrap();
    let ly = layer(&s, l);
    let a = ly.props.prop("transform/anchor").unwrap().value.as_vec3();
    let p = ly.props.prop("transform/position").unwrap().value.as_vec3();
    assert_eq!([a[0], a[1]], [0.0, 100.0]);
    assert_eq!([p[0] - before[0], p[1] - before[1]], [-50.0, 50.0]);
    assert!(s.execute("motion.anchor", json!({"layers": [l], "x": 2, "y": 0})).is_err());
}

#[test]
fn null_parents_the_selection_at_its_centre() {
    let (mut s, a) = setup();
    let b = solid(&mut s, "B");
    s.execute("prop.set", json!({"layer": a, "path": "transform/position", "value": [100, 100]})).unwrap();
    s.execute("prop.set", json!({"layer": b, "path": "transform/position", "value": [300, 200]})).unwrap();
    let undo = s.history.undo.len();
    let r = s.execute("motion.null", json!({"layers": [a, b]})).unwrap();
    let n = r["layer"].as_u64().unwrap();
    let np = layer(&s, n).props.prop("transform/position").unwrap().value.as_vec3();
    assert_eq!([np[0], np[1]], [200.0, 150.0]);
    assert_eq!(layer(&s, a).parent, Some(LayerId(n)));
    assert_eq!(layer(&s, b).parent, Some(LayerId(n)));
    // Parenting compensates: B still sits at (300, 200) in the comp (in the null's space, whose
    // anchor is its own centre, 50, 50).
    let na = layer(&s, n).props.prop("transform/anchor").unwrap().value.as_vec3();
    let bp = layer(&s, b).props.prop("transform/position").unwrap().value.as_vec3();
    assert_eq!([bp[0] - na[0], bp[1] - na[1]], [100.0, 50.0]);
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    assert_eq!(s.history.undo.last().unwrap().0, "Null to Selection");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.active_comp().unwrap().layer(LayerId(n)).is_none());
    assert_eq!(layer(&s, b).parent, None);
}

#[test]
fn build_in_and_out_keys_the_span() {
    let (mut s, l) = setup();
    s.execute("motion.build", json!({"layers": [l], "kind": "fade", "mode": "both", "frames": 15})).unwrap();
    let o = prop(&s, l, "transform/opacity");
    let v: Vec<f64> = o.keys.iter().map(|k| k.value.as_f64()).collect();
    assert_eq!(v, vec![0.0, 100.0, 100.0, 0.0]);
    let t: Vec<f64> = o.keys.iter().map(|k| (k.time.seconds() * 30.0).round()).collect();
    assert_eq!(t, vec![0.0, 15.0, 105.0, 120.0]);
    assert!(o.keys[0].out_interp == Interp::Bezier && o.keys[1].in_interp == Interp::Bezier);
    // Slide from the left by 200 px.
    s.execute("motion.build", json!({"layers": [l], "kind": "slide", "from": "left", "distance": 200, "frames": 10})).unwrap();
    let p = prop(&s, l, "transform/position");
    assert_eq!(p.keys.len(), 2);
    assert!((p.keys[1].value.as_vec3()[0] - p.keys[0].value.as_vec3()[0] - 200.0).abs() < 1e-9);
    assert!(s.execute("motion.build", json!({"layers": [l], "kind": "explode"})).is_err());
    assert!(s.execute("motion.build", json!({"layers": [l], "frames": 0})).is_err());
}

#[test]
fn dynamics_add_swings_after_the_move() {
    let (mut s, l) = setup();
    animate(&mut s, l, "transform/rotation", &[(0.0, json!(0)), (1.0, json!(90))]);
    select(&mut s, l, "transform/rotation");
    let r = s.execute("motion.dynamics", json!({"mode": "elastic", "amplitude": 20, "decay": 50, "count": 3})).unwrap();
    assert_eq!(r["keys"], 4);
    let k = prop(&s, l, "transform/rotation").keys;
    let v: Vec<f64> = k.iter().map(|k| k.value.as_f64()).collect();
    assert_eq!(v, vec![0.0, 90.0, 108.0, 81.0, 94.5, 90.0]);
    s.execute("edit.undo", json!({})).unwrap();
    select(&mut s, l, "transform/rotation");
    s.execute("motion.dynamics", json!({"mode": "bounce", "amplitude": 50, "count": 2})).unwrap();
    let k = prop(&s, l, "transform/rotation").keys;
    let v: Vec<f64> = k.iter().map(|k| k.value.as_f64()).collect();
    assert_eq!(v, vec![0.0, 90.0, 45.0, 90.0, 67.5, 90.0]);
    // Rotation never goes past the end value while bouncing.
    for f in 30..=90 {
        let x = evaluate(&k, Tick::from_seconds_f64(f as f64 / 30.0), false).unwrap().as_f64();
        assert!(x <= 90.0 + 1e-6, "frame {f}: {x}");
    }
    assert!(s.execute("motion.dynamics", json!({"mode": "jelly"})).is_err());
}

#[test]
fn delay_staggers_keys_then_layers() {
    let (mut s, a) = setup();
    let b = solid(&mut s, "B");
    let c = solid(&mut s, "C");
    for l in [a, b, c] {
        animate(&mut s, l, "transform/opacity", &[(0.0, json!(0)), (1.0, json!(100))]);
    }
    s.execute("layer.select", json!({"layers": [a, b, c]})).unwrap();
    for (i, l) in [a, b, c].into_iter().enumerate() {
        s.execute("prop.select", json!({"layer": l, "path": "transform/opacity", "add": i > 0})).unwrap();
    }
    assert_eq!(s.state.selected_keys.len(), 6);
    s.execute("motion.delay", json!({"frames": 3})).unwrap();
    let first = |l: u64| (prop(&s, l, "transform/opacity").keys[0].time.seconds() * 30.0).round();
    assert_eq!([first(a), first(b), first(c)], [0.0, 3.0, 6.0]);
    assert_eq!(s.state.selected_keys.len(), 6, "the moved keys stay selected");
    // Layers, in reverse.
    s.execute("motion.delay", json!({"frames": 10, "what": "layers", "order": "reverse", "layers": [a, b, c]})).unwrap();
    let start = |l: u64| (layer(&s, l).start_time.seconds() * 30.0).round();
    assert_eq!([start(a), start(b), start(c)], [20.0, 10.0, 0.0]);
    assert!(s.execute("motion.delay", json!({"layers": [a]})).is_err());
}

#[test]
fn clone_offsets_each_copy() {
    let (mut s, l) = setup();
    let p0 = layer(&s, l).props.prop("transform/position").unwrap().value.as_vec3();
    let n = s.active_comp().unwrap().layers.len();
    let r = s.execute("motion.clone", json!({"layers": [l], "count": 2, "offset": [40, 10], "frames": 5})).unwrap();
    let ids: Vec<u64> = r["layers"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    assert_eq!(ids.len(), 2);
    assert_eq!(s.active_comp().unwrap().layers.len(), n + 2);
    let p2 = layer(&s, ids[1]).props.prop("transform/position").unwrap().value.as_vec3();
    assert_eq!([p2[0] - p0[0], p2[1] - p0[1]], [80.0, 20.0]);
    assert_eq!((layer(&s, ids[1]).start_time.seconds() * 30.0).round(), 10.0);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), n);
}

#[test]
fn rigs_add_expressions_and_sliders() {
    let (mut s, a) = setup();
    let b = solid(&mut s, "Target");
    s.execute("motion.spin", json!({"layers": [a], "speed": 45})).unwrap();
    let la = layer(&s, a);
    assert!(la.props.prop("transform/rotation").unwrap().expr.as_ref().unwrap().text.contains("Spin Speed"));
    assert_eq!(la.effects().unwrap().groups().filter(|g| g.name == "Spin Speed").count(), 1);
    s.execute("motion.stare", json!({"layers": [a, b], "target": b})).unwrap();
    assert!(layer(&s, a).props.prop("transform/rotation").unwrap().expr.as_ref().unwrap().text.contains("\"Target\""));
    assert!(layer(&s, b).props.prop("transform/rotation").unwrap().expr.is_none(), "the target doesn't stare at itself");
    let r = s.execute("motion.falloff", json!({"layers": [a, b], "props": ["scale", "opacity"], "radius": 50})).unwrap();
    assert_eq!(r["properties"], 4);
    let f = layer(&s, r["layer"].as_u64().unwrap());
    assert_eq!(f.name, "Falloff");
    assert_eq!(f.effects().unwrap().groups().count(), 2);
    assert!(s.execute("motion.falloff", json!({"layers": [a], "props": ["position"]})).is_err());
    assert!(s.execute("motion.stare", json!({"layers": [a]})).is_err());
}
