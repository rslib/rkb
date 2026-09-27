//! The backends the rerank chain can open in this build.

use std::sync::Arc;

use rkb_core::doctor::{Check, Level};
use rkb_core::rerank::{Opener, Reranker, Settings};

#[cfg(feature = "laya")]
struct LayaBackend(Arc<rkb_rerank::Laya>);

#[cfg(feature = "laya")]
impl Reranker for LayaBackend {
    fn device(&self) -> Option<String> {
        Some(self.0.device().to_string())
    }

    fn score(&self, query: &str, items: &[String]) -> Result<Vec<f32>, String> {
        self.0.score(query, items).map_err(|e| format!("{e:#}"))
    }
}

/// The model loaded by an earlier call in this process, so `rkb eval` loads it once.
#[cfg(feature = "laya")]
static LOADED: std::sync::Mutex<Option<Arc<rkb_rerank::Laya>>> = std::sync::Mutex::new(None);

#[cfg(feature = "laya")]
fn laya(settings: &Settings) -> Result<Box<dyn Reranker>, String> {
    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(l) = loaded.as_ref() {
        return Ok(Box::new(LayaBackend(l.clone())));
    }
    let cpu = settings.device.as_deref() == Some("cpu") || std::env::var("RKB_LAYA_DEVICE").is_ok_and(|v| v == "cpu");
    let pref = if cpu { rkb_rerank::Pref::Cpu } else { rkb_rerank::Pref::Auto };
    let l = Arc::new(rkb_rerank::Laya::open(&rkb_rerank::files::default_dir(), pref).map_err(|e| format!("{e:#}"))?);
    *loaded = Some(l.clone());
    Ok(Box::new(LayaBackend(l)))
}

#[cfg(not(feature = "laya"))]
fn laya(_: &Settings) -> Result<Box<dyn Reranker>, String> {
    Err("not in this build".into())
}

pub fn opener(settings: &Settings) -> Opener {
    let settings = settings.clone();
    Arc::new(move |name: &str| match name {
        "laya" => laya(&settings),
        other => Err(format!("unknown backend `{other}`; this build knows laya and bm25")),
    })
}

/// The doctor check for the `laya` backend, when the rerank chain has it.
pub fn model_check(settings: &Settings) -> Option<Check> {
    if !settings.chain.iter().any(|b| b == "laya") {
        return None;
    }
    let check = |level, detail: String, fix: Option<&str>| Some(Check { name: "model", level, detail, fix: fix.map(String::from) });
    #[cfg(not(feature = "laya"))]
    return check(
        Level::Warn,
        "the chain has laya, and this build does not; search uses the next backend".into(),
        Some("install an rkb built with the laya feature"),
    );
    #[cfg(feature = "laya")]
    {
        let dir = rkb_rerank::files::default_dir();
        if let Err(e) = rkb_rerank::files::check(&dir) {
            return check(Level::Warn, format!("laya: {e:#}"), Some("rkb models fetch"));
        }
        let cpu = settings.device.as_deref() == Some("cpu") || std::env::var("RKB_LAYA_DEVICE").is_ok_and(|v| v == "cpu");
        if cfg!(target_os = "macos") && !cpu && !rkb_rerank::warmed(&dir) {
            return check(
                Level::Warn,
                format!("laya files ok in {}; runs on the cpu until this rkb compiles its GPU kernels", dir.display()),
                Some("rkb models warm"),
            );
        }
        let device = if cfg!(target_os = "macos") && !cpu { "gpu" } else { "cpu" };
        check(Level::Ok, format!("laya files ok in {}; device {device}", dir.display()), None)
    }
}

/// Compiles the GPU kernels for this binary when Laya's files are there and it has not done so yet,
/// because a new binary (after `cargo install`) starts cold. Returns a line for the install report.
#[cfg(feature = "laya")]
pub fn warm_after_install() -> Option<String> {
    let dir = rkb_rerank::files::default_dir();
    if rkb_rerank::files::check(&dir).is_err() || rkb_rerank::warmed(&dir) {
        return None;
    }
    match rkb_rerank::warm(&dir) {
        Ok(Some(t)) => Some(format!("GPU kernels compiled for this rkb in {:.1} s", t.as_secs_f64())),
        Ok(None) => None,
        Err(e) => Some(format!("GPU kernels not compiled ({e:#}); `rkb models warm` tries again")),
    }
}

#[cfg(not(feature = "laya"))]
pub fn warm_after_install() -> Option<String> {
    None
}
