//! Window ▸ Motion Tools driven like an agent would: by automation id over the control channel.
//! `MOTION_TOOLS_SNAPSHOT=path.png` also saves a capture of the maximised panel for review.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::control::{self, ControlRequest, Outcome};
use effectcraft_ui_egui::dock::PanelKind;
use egui_kittest::Harness;
use serde_json::{Value, json};

/// One control-channel request, then its synthetic input fed in one event per frame (kittest
/// doesn't call `raw_input_hook`).
fn send(h: &mut Harness<'_, EffectcraftApp>, method: &str, params: Value) {
    let ctx = h.ctx.clone();
    let (req, _rx) = ControlRequest::new(method, params);
    if let Outcome::Done(v) = control::handle(h.state_mut(), &ctx, &req) {
        assert!(v["ok"] != false, "{method}: {v}");
    }
    for e in h.state_mut().take_synthetic_input() {
        h.event(e);
        h.step();
    }
    h.run_steps(3);
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    assert!(h.state().auto.find(id).is_some(), "no automation id `{id}`");
    send(h, "ui.click", json!({"id": id}));
}

#[test]
fn motion_tools_panel_applies_presets_anchor_and_tools() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "M", "width": 640, "height": 360, "frameRate": 30, "duration": 4})).unwrap();
    let l = s.execute("layer.newSolid", json!({"color": "#4080ff", "width": 100, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    for (t, v) in [(0.0, 0.0), (1.0, 100.0)] {
        s.execute("prop.addKey", json!({"layer": l, "path": "transform/opacity", "time": t, "value": v})).unwrap();
    }
    s.execute("prop.select", json!({"layer": l, "path": "transform/opacity"})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(900.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::MotionTools);
    h.state_mut().ui.maximized = Some(PanelKind::MotionTools);
    h.run_steps(4);
    if let Ok(path) = std::env::var("MOTION_TOOLS_SNAPSHOT") {
        h.render().unwrap().save(path).unwrap();
    }

    // A preset applies to the selected keys.
    click(&mut h, "motionTools.preset.snap");
    let read = h.state_mut().session.execute("motion.readEase", json!({})).unwrap();
    assert!((read["outInfluence"].as_f64().unwrap() - 10.0).abs() < 1e-6, "{read}");
    assert!((h.state().ui.motion_tools.in_influence - 90.0).abs() < 1e-6);

    // Change the curve over `ui.set`, then Apply.
    send(&mut h, "ui.set", json!({"motionTools": {"out_influence": 50.0, "in_influence": 25.0}}));
    click(&mut h, "motionTools.apply");
    let read = h.state_mut().session.execute("motion.readEase", json!({})).unwrap();
    assert!((read["inInfluence"].as_f64().unwrap() - 25.0).abs() < 1e-6, "{read}");

    // The anchor grid's top-left point.
    h.state_mut().session.execute("layer.select", json!({"layers": [l]})).unwrap();
    h.run_steps(2);
    click(&mut h, "motionTools.anchor.topLeft");
    let a = h.state().session.execute_query_anchor(l);
    assert_eq!(a, [0.0, 0.0]);

    // A tool click applies it with its options; a right-click only shows them.
    send(&mut h, "ui.set", json!({"motionTools": {"spin_speed": 30.0}}));
    click(&mut h, "motionTools.tool.spin");
    assert_eq!(h.state().ui.motion_tools.tool, "spin");
    let comp = h.state().session.active_comp().unwrap();
    let rot = comp.layer(effectcraft_engine::project::LayerId(l)).unwrap().props.prop("transform/rotation").unwrap();
    assert!(rot.expr.as_ref().is_some_and(|e| e.text.contains("Spin Speed")));
    assert!(h.state().auto.find("motionTools.opt.spinSpeed").is_some(), "the Spin options show");
}

trait AnchorQuery {
    fn execute_query_anchor(&self, layer: u64) -> [f64; 2];
}

impl AnchorQuery for Session {
    fn execute_query_anchor(&self, layer: u64) -> [f64; 2] {
        let l = self.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(layer)).unwrap();
        let a = l.props.prop("transform/anchor").unwrap().value.as_vec3();
        [a[0], a[1]]
    }
}
