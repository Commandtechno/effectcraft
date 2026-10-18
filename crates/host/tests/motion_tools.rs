//! Window ▸ Motion Tools rigs evaluated by the expression engine: Spin, Stare and Falloff write
//! expressions, so check what they compute, not just that they exist.

use serde_json::{Value, json};

fn evaluated(s: &mut effectcraft_engine::Session, layer: u64, path: &str, time: f64) -> Value {
    let r = s.execute("prop.get", json!({"layer": layer, "path": path, "time": time})).unwrap();
    assert!(r.get("expressionError").is_none(), "{path}: {r}");
    r["evaluated"].clone()
}

fn solid(s: &mut effectcraft_engine::Session, name: &str, at: [f64; 2]) -> u64 {
    let l = s.execute("layer.newSolid", json!({"color": "#ffffff", "width": 20, "height": 20, "name": name})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": l, "path": "transform/position", "value": at})).unwrap();
    l
}

#[test]
fn spin_stare_and_falloff_evaluate() {
    let mut s = effectcraft_host::session();
    s.execute("comp.new", json!({"name": "M", "width": 400, "height": 400, "frameRate": 30, "duration": 4})).unwrap();
    let a = solid(&mut s, "A", [100.0, 100.0]);
    let b = solid(&mut s, "B", [200.0, 200.0]);
    let far = solid(&mut s, "Far", [390.0, 390.0]);

    // Spin: 45 °/s from the Spin Speed slider.
    s.execute("motion.spin", json!({"layers": [far], "speed": 45})).unwrap();
    assert!((evaluated(&mut s, far, "transform/rotation", 2.0).as_f64().unwrap() - 90.0).abs() < 1e-6);

    // Stare: A looks at B, down-right at 45°.
    s.execute("motion.stare", json!({"layers": [a], "target": b})).unwrap();
    assert!((evaluated(&mut s, a, "transform/rotation", 0.0).as_f64().unwrap() - 45.0).abs() < 1e-6);

    // Falloff at the comp centre (200, 200), radius 50, feather 100: B (distance 0) keeps its
    // scale; A (141 px) is part way; Far (269 px) is at zero.
    let r = s.execute("motion.falloff", json!({"layers": [b, a, far], "props": ["scale"], "radius": 50, "feather": 100})).unwrap();
    assert_eq!(r["properties"], 3);
    let sc = |s: &mut effectcraft_engine::Session, l| evaluated(s, l, "transform/scale", 0.0)[0].as_f64().unwrap();
    assert!((sc(&mut s, b) - 100.0).abs() < 1e-6);
    let d = (100.0f64 * 100.0 * 2.0).sqrt();
    assert!((sc(&mut s, a) - 100.0 * (1.0 - (d - 50.0) / 100.0)).abs() < 1e-6, "{}", sc(&mut s, a));
    assert!(sc(&mut s, far).abs() < 1e-6);
}
