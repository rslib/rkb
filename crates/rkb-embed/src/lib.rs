//! `nomic-ai/modernbert-embed-base` (Apache-2.0) as a local text embedder, on candle: Metal on macOS, else the CPU.

pub mod files;

use std::path::Path;

use anyhow::{Result, anyhow};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::modernbert::{Config, ModernBert};
use tokenizers::Tokenizer;

pub const QUERY_PREFIX: &str = "search_query: ";
pub const DOCUMENT_PREFIX: &str = "search_document: ";
pub const DIM: usize = 768;
const MAX_TOKENS: usize = 512;

pub struct Embedder {
    model: ModernBert,
    tok: Tokenizer,
    device: Device,
    /// Why Metal was not used, when it was wanted.
    pub gpu_skipped: Option<String>,
}

impl Embedder {
    /// Loads the model from `dir` after checking its files. `cpu` skips Metal; otherwise Metal is
    /// used when it loads and the CPU when it does not.
    pub fn open(dir: &Path, cpu: bool) -> Result<Embedder> {
        files::check(dir)?;
        let mut tok = Tokenizer::from_file(dir.join("tokenizer.json")).map_err(|e| anyhow!("tokenizer.json: {e}"))?;
        tok.with_truncation(Some(tokenizers::TruncationParams { max_length: MAX_TOKENS, ..Default::default() }))
            .map_err(|e| anyhow!("{e}"))?;
        tok.with_padding(None);
        let cfg: Config = serde_json::from_slice(&std::fs::read(dir.join("config.json"))?)?;
        let mut gpu_skipped = None;
        if !cpu {
            if cfg!(target_os = "macos") {
                match Device::new_metal(0).map_err(anyhow::Error::from).and_then(|d| load(dir, &cfg, &d).map(|m| (m, d))) {
                    Ok((model, device)) => return Ok(Embedder { model, tok, device, gpu_skipped }),
                    Err(e) => gpu_skipped = Some(format!("gpu unavailable: {e}")),
                }
            } else {
                gpu_skipped = Some("this build has no GPU engine".into());
            }
        }
        let device = Device::Cpu;
        Ok(Embedder { model: load(dir, &cfg, &device)?, tok, device, gpu_skipped })
    }

    /// `gpu` or `cpu`.
    pub fn device(&self) -> &'static str {
        if self.device.is_cpu() { "cpu" } else { "gpu" }
    }

    /// One L2-normalised vector per text (mean over the tokens of the last hidden state), each text cut to 512 tokens.
    /// The caller adds the query or document prefix.
    pub fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        texts.iter().map(|t| self.embed_one(t)).collect()
    }

    fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let enc = self.tok.encode(text, true).map_err(|e| anyhow!("{e}"))?;
        let n = enc.get_ids().len();
        let ids = Tensor::new(enc.get_ids(), &self.device)?.unsqueeze(0)?;
        let mask = Tensor::ones((1, n), DType::U32, &self.device)?;
        let hidden = self.model.forward(&ids, &mask)?;
        let mean = hidden.mean(1)?.squeeze(0)?.to_dtype(DType::F32)?;
        let norm = mean.sqr()?.sum_all()?.sqrt()?.to_scalar::<f32>()?.max(1e-12);
        Ok(mean.to_vec1::<f32>()?.into_iter().map(|x| x / norm).collect())
    }
}

fn load(dir: &Path, cfg: &Config, device: &Device) -> Result<ModernBert> {
    // The checkpoint names its tensors without candle's `model.` prefix.
    let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[dir.join("model.safetensors")], DType::F32, device)? }
        .rename_f(|n: &str| n.strip_prefix("model.").unwrap_or(n).to_string());
    Ok(ModernBert::load(vb, cfg)?)
}
