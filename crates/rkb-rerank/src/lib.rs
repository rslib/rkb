//! Laya (`convaiinnovations/laya`, Apache-2.0) as a reranker: how well a lesson fits a problem.
//! Two engines give the same answers: the GPU one (candle on Metal, macOS) and the CPU one (plain f32 kernels).

mod cpu;
pub mod files;
#[cfg(target_os = "macos")]
mod gpu;
mod model;

use std::path::Path;

use anyhow::Result;

/// The question Laya answers for each lesson.
pub const QUESTION: &str = "Does this lesson help with this problem?";

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
                gpu_skipped = Some("gpu kernels not compiled for this rkb yet; run `rkb models warm`".into());
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

/// Identifies the running binary; Metal's compiled-kernel cache belongs to one binary.
fn binary_key() -> Option<String> {
    use sha2::{Digest, Sha256};
    let exe = std::env::current_exe().ok()?;
    let m = std::fs::metadata(&exe).ok()?;
    let mtime = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    let key = format!("{}|{}|{mtime}", exe.display(), m.len());
    Some(Sha256::digest(key.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect())
}

fn warm_marker(dir: &Path) -> Option<std::path::PathBuf> {
    Some(dir.join(".gpu-warm").join(binary_key()?))
}

/// Whether this binary has compiled its GPU kernels, so a GPU run is fast.
pub fn warmed(dir: &Path) -> bool {
    warm_marker(dir).is_some_and(|m| m.exists())
}

/// Compiles the GPU kernels for this binary by scoring one item with no time limit, then records it.
/// Returns how long it took, or `None` when this build or machine has no GPU engine.
pub fn warm(dir: &Path) -> Result<Option<std::time::Duration>> {
    #[cfg(target_os = "macos")]
    {
        files::check(dir)?;
        let start = std::time::Instant::now();
        let dev = candle_core::Device::new_metal(0)?;
        let model = gpu::GpuLaya::load(dir, candle_core::DType::F16, &dev)?;
        let tok = model::Tok::load(dir)?;
        let (ids, markers) = tok.noul_sequence(QUESTION, "problem: warm up\nlesson:\n# Warm up", &model.cfg);
        model.score(&[ids], &[markers], 4)?;
        if let Some(m) = warm_marker(dir) {
            std::fs::create_dir_all(m.parent().unwrap())?;
            std::fs::write(&m, "")?;
        }
        Ok(Some(start.elapsed()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = dir;
        Ok(None)
    }
}
