//! The backends the rerank chain can open in this build.

#[cfg(feature = "laya")]
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use rkb_core::doctor::{Check, Level};
use rkb_core::rerank::{self, Opener, Ranked, Reranker, Settings};

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
    let pref = if cpu_forced(settings) { rkb_rerank::Pref::Cpu } else { rkb_rerank::Pref::Auto };
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

#[cfg(feature = "laya")]
fn cpu_forced(settings: &Settings) -> bool {
    settings.device.as_deref() == Some("cpu") || std::env::var("RKB_LAYA_DEVICE").is_ok_and(|v| v == "cpu")
}

/// This process has Metal's shared cache files open, and a child inherits them: a warm-up that holds them
/// lost its compile 4 of 4 times on 2026-09-29, and kept it 2 of 2 with them closed. std cannot close them, so sh does.
#[cfg(feature = "laya")]
const CLOSE_INHERITED_THEN_WARM: &str =
    r#"for fd in $(ls /dev/fd); do [ "$fd" -gt 2 ] && eval "exec $fd>&-"; done 2>/dev/null; exec "$0" models warm --background"#;

/// The rerank chain with this build's backends. A laya GPU call that ran out of time was most likely
/// compiling kernels the Metal cache lost; that compile dies with this process, so rkb drops the
/// confirmed warm-up (later calls use the CPU) and finishes the compile in one background warm-up.
pub fn run(settings: &Settings, only: Option<&str>, query: &str, items: &[String], timeout: Duration) -> rkb_core::error::Result<Ranked> {
    let ranked = rerank::run(settings, only, &opener(settings), query, items, timeout)?;
    #[cfg(feature = "laya")]
    if cfg!(target_os = "macos") && ranked.skipped.iter().any(|(b, r)| b == "laya" && r.starts_with("timeout")) && !cpu_forced(settings) {
        let dir = rkb_rerank::files::default_dir();
        if rkb_rerank::warmed(&dir) {
            rkb_rerank::forget_warm(&dir);
            rkb_rerank::start_background(&dir, || {
                use std::os::unix::process::CommandExt;
                std::process::Command::new("/bin/sh")
                    .args(["-c", CLOSE_INHERITED_THEN_WARM])
                    .arg(std::env::current_exe()?)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .process_group(0)
                    .spawn()
                    .map(drop)
            });
        }
    }
    Ok(ranked)
}

/// Times one GPU call in a fresh process (`rkb models probe`). The probe has no time limit and never
/// starts a background warm-up.
#[cfg(feature = "laya")]
pub fn probe_child() -> Result<Duration, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let o = std::process::Command::new(exe)
        .args(["models", "probe", "--format", "json"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).map_err(|_| String::from_utf8_lossy(&o.stdout).trim().to_string())?;
    match v["ms"].as_u64() {
        Some(ms) if o.status.success() => Ok(Duration::from_millis(ms)),
        _ => Err(v["error"]["message"].as_str().unwrap_or("the GPU probe failed").to_string()),
    }
}

/// The doctor check for the `laya` backend, when the rerank chain has it. On macOS it times a real GPU
/// call in a fresh process instead of trusting the warm-up marker.
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
        if !cfg!(target_os = "macos") || cpu_forced(settings) {
            return check(Level::Ok, format!("laya files ok in {}; device cpu", dir.display()), None);
        }
        let files = format!("laya files ok in {}", dir.display());
        match probe_child() {
            Err(e) => check(Level::Warn, format!("{files}; the GPU probe failed ({e}), so searches use the cpu"), None),
            Ok(t) if t > rkb_rerank::WARM_LIMIT => check(
                Level::Warn,
                format!("{files}; a GPU call took {:.1} s (the kernels were cold; compiled now)", t.as_secs_f64()),
                Some("rkb models warm"),
            ),
            Ok(t) if !rkb_rerank::warmed(&dir) => check(
                Level::Warn,
                format!("{files}; a GPU call took {:.1} s, but no warm-up is confirmed, so searches use the cpu", t.as_secs_f64()),
                Some("rkb models warm"),
            ),
            Ok(t) => check(Level::Ok, format!("{files}; device gpu, a GPU call took {:.1} s", t.as_secs_f64()), None),
        }
    }
}

/// The report line for a checked warm-up.
#[cfg(feature = "laya")]
pub fn warm_line(w: &rkb_rerank::Warmed) -> String {
    if w.confirmed {
        format!("GPU kernels compiled in {:.1} s; a fresh process scored in {:.1} s", w.compile.as_secs_f64(), w.probe.as_secs_f64())
    } else {
        format!(
            "warning: the GPU is still cold after {} compiles (a fresh process took {:.1} s); another Metal process may have run at the same time. Searches use the cpu; run `rkb models warm` again",
            w.compiles,
            w.probe.as_secs_f64()
        )
    }
}

/// Compiles and checks the GPU kernels when Laya's files are there and no warm-up is confirmed yet,
/// because a new rkb version starts cold. Returns a line for the install report.
#[cfg(feature = "laya")]
pub fn warm_after_install() -> Option<String> {
    let dir = rkb_rerank::files::default_dir();
    if rkb_rerank::files::check(&dir).is_err() || rkb_rerank::warmed(&dir) {
        return None;
    }
    match rkb_rerank::warm(&dir, probe_child) {
        Ok(Some(w)) => Some(warm_line(&w)),
        Ok(None) => None,
        Err(e) => Some(format!("GPU kernels not compiled ({e:#}); `rkb models warm` tries again")),
    }
}

#[cfg(not(feature = "laya"))]
pub fn warm_after_install() -> Option<String> {
    None
}
