//! Laya on the GPU through candle and Metal, in fp16.
//! Weights are split at load so no step runs on a strided view, and attention uses candle's fused kernel.

use anyhow::Result;
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;

use crate::model::Cfg;

/// Layer norm through candle's fused kernel; a missing bias is zeros.
struct Norm {
    w: Tensor,
    b: Tensor,
    eps: f32,
}

impl Norm {
    fn load(vb: VarBuilder, d: usize, eps: f64, bias: bool) -> Result<Self> {
        let w = vb.get(d, "weight")?;
        let b = if bias { vb.get(d, "bias")? } else { w.zeros_like()? };
        Ok(Norm { w, b, eps: eps as f32 })
    }

    fn f(&self, x: &Tensor) -> Result<Tensor> {
        Ok(candle_nn::ops::layer_norm(x, &self.w, &self.b, self.eps)?)
    }
}

/// A linear layer applied to a 2D input as one matrix product.
struct Lin {
    w: Tensor,
    b: Option<Tensor>,
}

impl Lin {
    fn f(&self, x: &Tensor) -> Result<Tensor> {
        let y = x.matmul(&self.w.t()?)?;
        Ok(match &self.b {
            Some(b) => y.broadcast_add(b)?,
            None => y,
        })
    }
}

/// Rows `[i*n, (i+1)*n)` of a weight (and bias) as its own contiguous layer, so no output is ever a strided view.
fn part(w: &Tensor, b: Option<&Tensor>, i: usize, n: usize) -> Result<Lin> {
    Ok(Lin { w: w.narrow(0, i * n, n)?.contiguous()?, b: b.map(|b| b.narrow(0, i * n, n)?.contiguous()).transpose()? })
}

fn lin(vb: VarBuilder, i: usize, o: usize, bias: bool) -> Result<Lin> {
    Ok(Lin { w: vb.get((o, i), "weight")?, b: if bias { Some(vb.get(o, "bias")?) } else { None } })
}

struct Rope {
    cos: Tensor,
    sin: Tensor,
}

impl Rope {
    fn new(dim: usize, theta: f64, max: usize, dtype: DType, dev: &Device) -> Result<Self> {
        let inv: Vec<f32> = (0..dim).step_by(2).map(|i| 1.0 / theta.powf(i as f64 / dim as f64) as f32).collect();
        let n = inv.len();
        let t = Tensor::arange(0u32, max as u32, dev)?.to_dtype(DType::F32)?.reshape((max, 1))?;
        let f = t.matmul(&Tensor::from_vec(inv, (1, n), dev)?)?;
        Ok(Rope { cos: f.cos()?.to_dtype(dtype)?, sin: f.sin()?.to_dtype(dtype)? })
    }
}

struct Attn {
    q: Lin,
    k: Lin,
    v: Lin,
    o: Lin,
}

struct EncLayer {
    attn_norm: Option<Norm>,
    attn: Attn,
    mlp_norm: Norm,
    mlp_in: Lin,
    mlp_gate: Lin,
    mlp_out: Lin,
    local: bool,
}

struct HeadLayer {
    norm1: Norm,
    attn: Attn,
    norm2: Norm,
    lin1: Lin,
    lin2: Lin,
}

pub struct GpuLaya {
    pub cfg: Cfg,
    emb: Tensor,
    emb_norm: Norm,
    layers: Vec<EncLayer>,
    final_norm: Norm,
    global_rope: Rope,
    local_rope: Rope,
    type_noul: Tensor,
    head: Vec<HeadLayer>,
    scorer_norm: Norm,
    scorer1: Lin,
    scorer2: Lin,
    pub dtype: DType,
    pub device: Device,
}

impl GpuLaya {
    pub fn load(dir: &std::path::Path, dtype: DType, device: &Device) -> Result<Self> {
        let cfg = Cfg::load(dir)?;
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[dir.join("model.safetensors")], dtype, device)? };
        let (d, e) = (cfg.hidden, cfg.eps);
        let enc = vb.pp("encoder");
        let inter = enc.pp("layers.0.mlp.Wo").get_unchecked("weight")?.dim(1)?;
        let mut layers = vec![];
        for i in 0..cfg.layers {
            let l = enc.pp(format!("layers.{i}"));
            let wqkv = l.get((3 * d, d), "attn.Wqkv.weight")?;
            let wi = l.get((2 * inter, d), "mlp.Wi.weight")?;
            layers.push(EncLayer {
                attn_norm: if i == 0 { None } else { Some(Norm::load(l.pp("attn_norm"), d, e, false)?) },
                attn: Attn {
                    q: part(&wqkv, None, 0, d)?,
                    k: part(&wqkv, None, 1, d)?,
                    v: part(&wqkv, None, 2, d)?,
                    o: lin(l.pp("attn.Wo"), d, d, false)?,
                },
                mlp_norm: Norm::load(l.pp("mlp_norm"), d, e, false)?,
                mlp_in: part(&wi, None, 0, inter)?,
                mlp_gate: part(&wi, None, 1, inter)?,
                mlp_out: lin(l.pp("mlp.Wo"), inter, d, false)?,
                local: i % cfg.global_every != 0,
            });
        }
        let mut head = vec![];
        for i in 0..cfg.head_layers {
            let h = vb.pp(format!("head.layers.{i}"));
            let (w, b) = (h.get((3 * d, d), "self_attn.in_proj_weight")?, h.get(3 * d, "self_attn.in_proj_bias")?);
            head.push(HeadLayer {
                norm1: Norm::load(h.pp("norm1"), d, 1e-5, true)?,
                attn: Attn {
                    q: part(&w, Some(&b), 0, d)?,
                    k: part(&w, Some(&b), 1, d)?,
                    v: part(&w, Some(&b), 2, d)?,
                    o: lin(h.pp("self_attn.out_proj"), d, d, true)?,
                },
                norm2: Norm::load(h.pp("norm2"), d, 1e-5, true)?,
                lin1: lin(h.pp("linear1"), d, 4 * d, true)?,
                lin2: lin(h.pp("linear2"), 4 * d, d, true)?,
            });
        }
        let head_dim = d / cfg.heads;
        Ok(GpuLaya {
            emb: enc.pp("embeddings.tok_embeddings").get_unchecked("weight")?,
            emb_norm: Norm::load(enc.pp("embeddings.norm"), d, e, false)?,
            layers,
            final_norm: Norm::load(enc.pp("final_norm"), d, e, false)?,
            global_rope: Rope::new(head_dim, cfg.global_theta, cfg.max_len, dtype, device)?,
            local_rope: Rope::new(head_dim, cfg.local_theta, cfg.max_len, dtype, device)?,
            // qtype 2 is `noul`.
            type_noul: vb.get((3, d), "type_emb.weight")?.get(2)?,
            head,
            scorer_norm: Norm::load(vb.pp("scorer.0"), d, 1e-5, true)?,
            scorer1: lin(vb.pp("scorer.1"), d, d, true)?,
            scorer2: lin(vb.pp("scorer.3"), d, 1, true)?,
            cfg,
            dtype,
            device: device.clone(),
        })
    }

    /// Attention over `x` of shape (b*s, d). `mask` is (b, heads, s, s) on Metal, broadcastable elsewhere.
    fn attention(&self, x: &Tensor, a: &Attn, b: usize, s: usize, rope: Option<&Rope>, mask: &Tensor) -> Result<Tensor> {
        let d = self.cfg.hidden;
        let heads = self.cfg.heads;
        let dh = d / heads;
        let split = |l: &Lin| -> Result<Tensor> { Ok(l.f(x)?.reshape((b, s, heads, dh))?) };
        let (mut q, mut k, v) = (split(&a.q)?, split(&a.k)?, split(&a.v)?);
        if let Some(r) = rope {
            let (cos, sin) = (r.cos.narrow(0, 0, s)?, r.sin.narrow(0, 0, s)?);
            q = candle_nn::rotary_emb::rope_thd(&q, &cos, &sin)?;
            k = candle_nn::rotary_emb::rope_thd(&k, &cos, &sin)?;
        }
        let scale = (dh as f64).powf(-0.5);
        let (q, k, v) = (q.transpose(1, 2)?, k.transpose(1, 2)?, v.transpose(1, 2)?);
        // One fused kernel reads the transposed views directly: scores, mask, softmax, values.
        let o = candle_nn::ops::sdpa(&q, &k, &v, Some(mask), false, scale as f32, 1.0)?;
        a.o.f(&o.transpose(1, 2)?.contiguous()?.reshape((b * s, d))?)
    }

    /// Raw logits `[false, true]` for each sequence, with its two marker positions.
    pub fn forward(&self, ids: &[Vec<u32>], markers: &[[usize; 2]]) -> Result<Vec<[f32; 2]>> {
        let (b, heads) = (ids.len(), self.cfg.heads);
        let s = ids.iter().map(Vec::len).max().unwrap_or(0);
        let mut flat = Vec::with_capacity(b * s);
        let mut pad = Vec::with_capacity(b * s);
        for seq in ids {
            flat.extend(seq.iter().copied().chain(std::iter::repeat_n(0, s - seq.len())));
            pad.extend((0..s).map(|j| if j < seq.len() { 0f32 } else { -1e4 }));
        }
        let dev = &self.device;
        // Additive masks: padding keys everywhere; also keys farther than half the window for local layers.
        let half = self.cfg.local_window / 2;
        let win: Vec<f32> = (0..s).flat_map(|i| (0..s).map(move |j| if i.abs_diff(j) > half { -1e4 } else { 0.0 })).collect();
        let pad_mask = Tensor::from_vec(pad, (b, 1, 1, s), dev)?.to_dtype(self.dtype)?;
        let local_mask = pad_mask.broadcast_add(&Tensor::from_vec(win, (1, 1, s, s), dev)?.to_dtype(self.dtype)?)?;
        // The fused attention kernel wants the mask in its full shape.
        let (pad_mask, local_mask) =
            (pad_mask.broadcast_as((b, heads, s, s))?.contiguous()?, local_mask.broadcast_as((b, heads, s, s))?.contiguous()?);

        let input = Tensor::from_vec(flat, b * s, dev)?;
        let mut h = self.emb_norm.f(&self.emb.index_select(&input, 0)?)?;
        for l in &self.layers {
            let x = match &l.attn_norm {
                Some(n) => n.f(&h)?,
                None => h.clone(),
            };
            let (rope, mask) = if l.local { (&self.local_rope, &local_mask) } else { (&self.global_rope, &pad_mask) };
            h = (&h + self.attention(&x, &l.attn, b, s, Some(rope), mask)?)?;
            let n = l.mlp_norm.f(&h)?;
            let g = (l.mlp_in.f(&n)?.gelu_erf()? * l.mlp_gate.f(&n)?)?;
            h = (&h + l.mlp_out.f(&g)?)?;
        }
        h = self.final_norm.f(&h)?.broadcast_add(&self.type_noul)?;
        for l in &self.head {
            h = (&h + self.attention(&l.norm1.f(&h)?, &l.attn, b, s, None, &pad_mask)?)?;
            h = (&h + l.lin2.f(&l.lin1.f(&l.norm2.f(&h)?)?.relu()?)?)?;
        }
        let idx: Vec<u32> = markers.iter().enumerate().flat_map(|(i, m)| m.iter().map(move |p| (i * s + p) as u32)).collect();
        let picked = h.index_select(&Tensor::from_vec(idx, b * 2, dev)?, 0)?;
        let logits = self.scorer2.f(&self.scorer1.f(&self.scorer_norm.f(&picked)?)?.gelu_erf()?)?;
        let v = logits.to_dtype(DType::F32)?.reshape((b, 2))?.to_vec2::<f32>()?;
        Ok(v.into_iter().map(|r| [r[0], r[1]]).collect())
    }

    /// Scores many sequences: sorted by length and run in groups of `group`, so little work goes to padding.
    /// Returns logits in the input order.
    pub fn score(&self, ids: &[Vec<u32>], markers: &[[usize; 2]], group: usize) -> Result<Vec<[f32; 2]>> {
        let mut order: Vec<usize> = (0..ids.len()).collect();
        order.sort_by_key(|&i| ids[i].len());
        let mut out = vec![[0f32; 2]; ids.len()];
        for part in order.chunks(group.max(1)) {
            let seqs: Vec<Vec<u32>> = part.iter().map(|&i| ids[i].clone()).collect();
            let ms: Vec<[usize; 2]> = part.iter().map(|&i| markers[i]).collect();
            for (&i, l) in part.iter().zip(self.forward(&seqs, &ms)?) {
                out[i] = l;
            }
        }
        Ok(out)
    }
}
