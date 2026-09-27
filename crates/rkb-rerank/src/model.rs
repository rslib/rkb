//! Laya's configuration, tokenizer and input builder, shared by both engines.
//! Ported from the model's own `rl_common.py` (`build_sequence`); Laya is `convaiinnovations/laya`, Apache-2.0.

use anyhow::{Result, anyhow};

pub struct Cfg {
    pub hidden: usize,
    pub heads: usize,
    pub layers: usize,
    pub global_every: usize,
    pub local_window: usize,
    pub global_theta: f64,
    pub local_theta: f64,
    pub eps: f64,
    pub max_len: usize,
    pub head_max_len: usize,
    pub head_layers: usize,
    pub temperature_noul: f64,
}

impl Cfg {
    pub fn load(dir: &std::path::Path) -> Result<Self> {
        let enc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("encoder/config.json"))?)?;
        let rl: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("rl_agent_config.json"))?)?;
        let u = |v: &serde_json::Value, k: &str| v[k].as_u64().ok_or_else(|| anyhow!("missing {k}")).map(|x| x as usize);
        let rope = &enc["rope_parameters"];
        Ok(Cfg {
            hidden: u(&enc, "hidden_size")?,
            heads: u(&enc, "num_attention_heads")?,
            layers: u(&enc, "num_hidden_layers")?,
            global_every: u(&enc, "global_attn_every_n_layers")?,
            local_window: u(&enc, "local_attention")?,
            global_theta: rope["full_attention"]["rope_theta"].as_f64().unwrap_or(160000.0),
            local_theta: rope["sliding_attention"]["rope_theta"].as_f64().unwrap_or(10000.0),
            eps: enc["norm_eps"].as_f64().unwrap_or(1e-5),
            max_len: u(&rl, "max_len")?,
            head_max_len: u(&rl, "head_max_len")?,
            head_layers: u(&rl, "head_layers")?,
            temperature_noul: rl["temperature_by_options"]["noul:2"].as_f64().unwrap_or(1.0),
        })
    }
}

pub struct Tok {
    tok: tokenizers::Tokenizer,
    pub cls: u32,
    pub sep: u32,
    pub mask: u32,
}

impl Tok {
    pub fn load(dir: &std::path::Path) -> Result<Self> {
        let tok = tokenizers::Tokenizer::from_file(dir.join("tokenizer/tokenizer.json")).map_err(|e| anyhow!("{e}"))?;
        let names: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("tokenizer/tokenizer_config.json"))?)?;
        let id = |k: &str| {
            let t = names[k].as_str().ok_or_else(|| anyhow!("no {k} in tokenizer_config.json"))?;
            tok.token_to_id(t).ok_or_else(|| anyhow!("no token {t}"))
        };
        Ok(Tok { cls: id("cls_token")?, sep: id("sep_token")?, mask: id("mask_token")?, tok })
    }

    fn enc(&self, text: &str) -> Vec<u32> {
        self.tok.encode(text, false).map(|e| e.get_ids().to_vec()).unwrap_or_default()
    }

    /// `[CLS] noul question: <ins> [SEP] [MASK] false... [MASK] true... [SEP] <state> [SEP]`, and the two marker positions.
    pub fn noul_sequence(&self, ins: &str, state: &str, cfg: &Cfg) -> (Vec<u32>, [usize; 2]) {
        let opts = ["false: no, the statement does not hold", "true: yes, the statement holds"];
        let mask_text = self.tok.id_to_token(self.mask).unwrap_or_default();
        let head = self.enc(&format!("noul question: {}", ins.replace(&mask_text, " ")));
        let opt_ids: Vec<Vec<u32>> = opts
            .iter()
            .map(|o| {
                let mut v = vec![self.mask];
                v.extend(self.enc(&format!(" {}", o.replace(&mask_text, " "))).into_iter().take(48));
                v
            })
            .collect();
        let opt_budget = cfg.head_max_len as i64 - opt_ids.iter().map(|o| o.len() as i64).sum::<i64>();
        let head: Vec<u32> = head.into_iter().take(opt_budget.max(8) as usize).collect();
        let mut ids = vec![self.cls];
        ids.extend(head);
        ids.push(self.sep);
        let mut markers = [0usize; 2];
        for (i, o) in opt_ids.iter().enumerate() {
            markers[i] = ids.len();
            ids.extend(o);
        }
        ids.push(self.sep);
        let room = cfg.max_len.saturating_sub(ids.len() + 1);
        ids.extend(self.enc(&state.replace(&mask_text, " ")).into_iter().take(room));
        ids.push(self.sep);
        ids.truncate(cfg.max_len);
        (ids, markers)
    }
}

/// Probability that the answer is `true`, after the model's temperature for a two-option question.
pub fn p_true(cfg: &Cfg, logits: [f32; 2]) -> f32 {
    let t = cfg.temperature_noul as f32;
    let (a, b) = (logits[0] / t, logits[1] / t);
    let m = a.max(b);
    let (ea, eb) = ((a - m).exp(), (b - m).exp());
    eb / (ea + eb)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs the model files: `RKB_TEST_LAYA_DIR=<dir> cargo test -p rkb-rerank`.
    #[test]
    fn input_matches_the_python_builder() {
        let Some(dir) = std::env::var_os("RKB_TEST_LAYA_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        let (cfg, tok) = (Cfg::load(&dir).unwrap(), Tok::load(&dir).unwrap());
        let reference: serde_json::Value = serde_json::from_str(include_str!("../../../tests/fixtures/laya/ref.json")).unwrap();
        let ins = reference["question"]["ins"].as_str().unwrap();
        assert_eq!(ins, crate::QUESTION);
        for p in reference["pairs"].as_array().unwrap() {
            let (ids, markers) = tok.noul_sequence(ins, p["state"].as_str().unwrap(), &cfg);
            let want: Vec<u32> = p["ids"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u32).collect();
            let want_m: Vec<usize> = p["markers"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as usize).collect();
            assert_eq!((ids, markers.to_vec()), (want, want_m));
        }
    }
}
