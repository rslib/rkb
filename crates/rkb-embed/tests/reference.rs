//! The embeddings against sentence-transformers' own.
//! Needs the model files: `RKB_TEST_EMBED_DIR=<dir> cargo test -p rkb-embed --release`.

use rkb_embed::Embedder;

fn check(cpu: bool) {
    let Some(dir) = std::env::var_os("RKB_TEST_EMBED_DIR") else { return };
    let model = Embedder::open(std::path::Path::new(&dir), cpu).unwrap();
    let want = if cpu || !cfg!(target_os = "macos") { "cpu" } else { "gpu" };
    assert_eq!(model.device(), want, "{:?}", model.gpu_skipped);
    let reference: serde_json::Value = serde_json::from_str(include_str!("../../../tests/fixtures/modernbert/ref.json")).unwrap();
    for r in reference.as_array().unwrap() {
        let text = r["text"].as_str().unwrap();
        let theirs: Vec<f32> = r["vec"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect();
        let ours = model.embed(&[text]).unwrap().remove(0);
        let cosine: f32 = ours.iter().zip(&theirs).map(|(a, b)| a * b).sum();
        let norm = theirs.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(cosine / norm >= 0.999, "{} engine: cosine {} for {text:?}", model.device(), cosine / norm);
    }
}

#[test]
fn cpu_matches_python() {
    check(true);
}

#[test]
fn default_device_matches_python() {
    check(false);
}
