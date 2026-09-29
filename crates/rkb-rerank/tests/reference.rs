//! Laya's relevance against the official Python implementation, on the synthetic search fixture.
//! Needs the model files: `RKB_TEST_LAYA_DIR=<dir> cargo test -p rkb-rerank --release`.

use rkb_rerank::{Laya, Pref};

fn check(pref: Pref, tolerance: f32) {
    let Some(dir) = std::env::var_os("RKB_TEST_LAYA_DIR") else { return };
    let dir = std::path::Path::new(&dir);
    if pref == Pref::Auto {
        rkb_rerank::warm(dir, || Ok(std::time::Duration::ZERO)).unwrap();
    }
    let model = Laya::open(dir, pref).unwrap();
    let want_device = if pref == Pref::Auto && cfg!(target_os = "macos") { "gpu" } else { "cpu" };
    assert_eq!(model.device(), want_device, "{:?}", model.gpu_skipped);
    let reference: serde_json::Value = serde_json::from_str(include_str!("../../../tests/fixtures/laya/ref.json")).unwrap();
    let t = reference["temperature_noul"].as_f64().unwrap() as f32;
    let mut worst = 0f32;
    for p in reference["pairs"].as_array().unwrap() {
        let state = p["state"].as_str().unwrap();
        let item = state.split_once("\nlesson:\n").unwrap().1.to_string();
        let got = model.score(p["query"].as_str().unwrap(), &[item]).unwrap()[0];
        let l: Vec<f32> = p["logits"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect();
        let (a, b) = (l[0] / t, l[1] / t);
        let want = 1.0 / (1.0 + (a - b).exp());
        worst = worst.max((got - want).abs());
    }
    assert!(worst <= tolerance, "{} engine: max difference {worst} > {tolerance}", model.device());
}

#[test]
fn cpu_engine_matches_python() {
    check(Pref::Cpu, 0.0005);
}

#[test]
fn default_engine_matches_python() {
    check(Pref::Auto, 0.005);
}
