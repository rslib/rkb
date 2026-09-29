//! Laya (`convaiinnovations/laya`, Apache-2.0) as a reranker: how well a lesson fits a problem.
//! Two engines give the same answers: the GPU one (candle on Metal, macOS) and the CPU one (plain f32 kernels).

mod cpu;
pub mod files;
#[cfg(target_os = "macos")]
mod gpu;
mod model;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;

/// The question Laya answers for each lesson.
pub const QUESTION: &str = "Is this lesson about the same problem as the user describes?";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pref {
    /// The GPU when the build has it and one is available, else the CPU.
    Auto,
    Cpu,
}

#[allow(clippy::large_enum_variant, reason = "one engine per process")]
enum Engine {
    Cpu(cpu::CpuLaya),
    #[cfg(target_os = "macos")]
    Gpu(gpu::GpuLaya),
}

pub struct Laya {
    engine: Engine,
    tok: model::Tok,
    /// Why the GPU was not used, when it was wanted.
    pub gpu_skipped: Option<String>,
}

impl Laya {
    /// Loads the model from `dir` after checking its files; picks the device as `pref` says.
    pub fn open(dir: &Path, pref: Pref) -> Result<Laya> {
        files::check(dir)?;
        let tok = model::Tok::load(dir)?;
        let mut gpu_skipped = None;
        #[cfg(target_os = "macos")]
        if pref == Pref::Auto {
            if !warmed(dir) {
                // The first GPU run of a binary compiles its Metal kernels (about 10 s), which no search should pay.
                gpu_skipped = Some("no confirmed GPU warm-up on this machine for this rkb; run `rkb models warm`".into());
            } else {
                let gpu = candle_core::Device::new_metal(0)
                    .map_err(anyhow::Error::from)
                    .and_then(|dev| gpu::GpuLaya::load(dir, candle_core::DType::F16, &dev));
                match gpu {
                    Ok(g) => return Ok(Laya { engine: Engine::Gpu(g), tok, gpu_skipped }),
                    Err(e) => gpu_skipped = Some(format!("gpu unavailable: {e}")),
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        if pref == Pref::Auto {
            gpu_skipped = Some("this build has no GPU engine".into());
        }
        Ok(Laya { engine: Engine::Cpu(cpu::CpuLaya::load(dir)?), tok, gpu_skipped })
    }

    /// `gpu` or `cpu`.
    pub fn device(&self) -> &'static str {
        match self.engine {
            Engine::Cpu(_) => "cpu",
            #[cfg(target_os = "macos")]
            Engine::Gpu(_) => "gpu",
        }
    }

    fn cfg(&self) -> &model::Cfg {
        match &self.engine {
            Engine::Cpu(m) => &m.cfg,
            #[cfg(target_os = "macos")]
            Engine::Gpu(m) => &m.cfg,
        }
    }

    /// Relevance in [0, 1] of each item to the query, in the items' order.
    pub fn score(&self, query: &str, items: &[String]) -> Result<Vec<f32>> {
        let cfg = self.cfg();
        let (ids, markers): (Vec<_>, Vec<_>) =
            items.iter().map(|it| self.tok.noul_sequence(QUESTION, &format!("problem: {query}\nlesson:\n{it}"), cfg)).unzip();
        let logits = match &self.engine {
            Engine::Cpu(m) => m.forward(&ids, &markers),
            #[cfg(target_os = "macos")]
            Engine::Gpu(m) => m.score(&ids, &markers, 4)?,
        };
        Ok(logits.into_iter().map(|l| model::p_true(cfg, l)).collect())
    }
}

/// The version of candle's Metal kernel source. macOS keeps compiled kernels in one cache shared by
/// every command-line tool, keyed by source, so a warm-up holds for any binary with the same kernels.
/// Bump it with the candle dependency.
const KERNELS: &str = "candle-0.11";

/// A GPU call at or under this counts as warm: a cold one compiles for about 10 s.
pub const WARM_LIMIT: Duration = Duration::from_millis(1500);

fn warm_dir(dir: &Path) -> PathBuf {
    dir.join(".gpu-warm")
}

/// The marker of a confirmed warm-up for this rkb version and kernel source.
fn warm_marker(dir: &Path) -> PathBuf {
    warm_dir(dir).join(format!("{}+{KERNELS}", env!("CARGO_PKG_VERSION")))
}

/// Whether a warm-up was confirmed on this machine, so a GPU run is fast.
pub fn warmed(dir: &Path) -> bool {
    warm_marker(dir).exists()
}

/// Drops the confirmed warm-up, so later calls use the CPU engine until a warm-up confirms the GPU again.
pub fn forget_warm(dir: &Path) {
    let _ = std::fs::remove_file(warm_marker(dir));
}

fn mark_warm(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(warm_dir(dir))?;
    std::fs::write(warm_marker(dir), "")?;
    Ok(())
}

/// Loads the GPU engine in a fresh Metal device and scores one short item with no time limit; a cold
/// kernel cache compiles here. Returns how long it took, or `None` when this build has no GPU engine.
pub fn gpu_once(dir: &Path) -> Result<Option<Duration>> {
    #[cfg(target_os = "macos")]
    {
        files::check(dir)?;
        let start = std::time::Instant::now();
        let dev = candle_core::Device::new_metal(0)?;
        let model = gpu::GpuLaya::load(dir, candle_core::DType::F16, &dev)?;
        let tok = model::Tok::load(dir)?;
        let (ids, markers) = tok.noul_sequence(QUESTION, "problem: warm up\nlesson:\n# Warm up", &model.cfg);
        model.score(&[ids], &[markers], 4)?;
        Ok(Some(start.elapsed()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = dir;
        Ok(None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Warmed {
    /// The first compile.
    pub compile: Duration,
    /// The last probe of a fresh process.
    pub probe: Duration,
    pub compiles: u32,
    /// A fresh process then scored within `WARM_LIMIT`.
    pub confirmed: bool,
}

/// Compiles, then asks a fresh process: another Metal process running at the same time can make the
/// shared cache lose the compile. When the probe is still cold, compiles once more and probes again.
fn confirm(mut compile: impl FnMut() -> Result<Duration>, mut probe: impl FnMut() -> Result<Duration, String>) -> Result<Warmed> {
    let mut probe = || probe().map_err(anyhow::Error::msg);
    let first = compile()?;
    let mut w = Warmed { compile: first, probe: probe()?, compiles: 1, confirmed: false };
    if w.probe > WARM_LIMIT {
        compile()?;
        w.compiles = 2;
        w.probe = probe()?;
    }
    w.confirmed = w.probe <= WARM_LIMIT;
    Ok(w)
}

/// Compiles the GPU kernels and records the warm-up when `probe` (a fresh process's GPU call) confirms it.
/// `None` when this build has no GPU engine.
pub fn warm(dir: &Path, probe: impl FnMut() -> Result<Duration, String>) -> Result<Option<Warmed>> {
    if cfg!(not(target_os = "macos")) {
        return Ok(None);
    }
    let w = confirm(|| gpu_once(dir).map(Option::unwrap_or_default), probe)?;
    if w.confirmed {
        mark_warm(dir)?;
    }
    Ok(Some(w))
}

/// The lock of the one background warm-up; older than this counts as left behind by a dead run.
const STALE_LOCK: Duration = Duration::from_secs(120);

pub fn warming_lock(dir: &Path) -> PathBuf {
    warm_dir(dir).join("warming.lock")
}

/// Starts one background warm-up with `spawn`, unless one holds the lock. Returns whether it started.
/// The warm-up removes the lock when it ends; `create_new` works on NFS where flock may not.
pub fn start_background(dir: &Path, spawn: impl FnOnce() -> std::io::Result<()>) -> bool {
    let lock = warming_lock(dir);
    if std::fs::create_dir_all(warm_dir(dir)).is_err() {
        return false;
    }
    let stale = std::fs::metadata(&lock).and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|a| a > STALE_LOCK));
    if stale {
        let _ = std::fs::remove_file(&lock);
    }
    if std::fs::OpenOptions::new().write(true).create_new(true).open(&lock).is_err() {
        return false;
    }
    if spawn().is_err() {
        let _ = std::fs::remove_file(&lock);
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_does_not_depend_on_the_binary() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!warmed(dir.path()));
        mark_warm(dir.path()).unwrap();
        assert!(warmed(dir.path()), "any copy of this rkb version shares the kernel cache");
        let name = warm_marker(dir.path()).file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(name, format!("{}+{KERNELS}", env!("CARGO_PKG_VERSION")));
        forget_warm(dir.path());
        assert!(!warmed(dir.path()));
    }

    #[test]
    fn confirm_retries_once() {
        let ms = Duration::from_millis;
        let run = |probes: Vec<u64>| {
            let (mut compiles, mut p) = (0, probes.into_iter());
            let w = confirm(
                || {
                    compiles += 1;
                    Ok(ms(9000))
                },
                || Ok(ms(p.next().unwrap())),
            )
            .unwrap();
            (w.compiles, w.confirmed, compiles)
        };
        assert_eq!(run(vec![300]), (1, true, 1));
        assert_eq!(run(vec![9800, 250]), (2, true, 2), "lost to another process, then kept");
        assert_eq!(run(vec![9800, 9500]), (2, false, 2));
        assert_eq!(run(vec![1500]), (1, true, 1));
    }

    #[test]
    fn one_background_warm_up() {
        let dir = tempfile::tempdir().unwrap();
        let mut spawned = 0;
        assert!(start_background(dir.path(), || {
            spawned += 1;
            Ok(())
        }));
        assert!(!start_background(dir.path(), || {
            spawned += 1;
            Ok(())
        }));
        assert_eq!(spawned, 1, "the lock holds while the first runs");
        let old = std::time::SystemTime::now() - Duration::from_secs(300);
        std::fs::File::options().write(true).open(warming_lock(dir.path())).unwrap().set_modified(old).unwrap();
        assert!(start_background(dir.path(), || Ok(())), "a stale lock is taken over");
        std::fs::remove_file(warming_lock(dir.path())).unwrap();
        assert!(!start_background(dir.path(), || Err(std::io::Error::other("no exe"))));
        assert!(!warming_lock(dir.path()).exists(), "a failed spawn leaves no lock");
    }
}
