//! Laya's forward pass on the CPU in plain f32 buffers: candle only loads the weights.
//! Follows the kernel style of ai-tools `rait-vision/src/kernels.rs`: one matrix product per
//! layer with a fused epilogue, and attention per (sequence, head) in parallel. Sequences are
//! concatenated with no padding, so no work goes to pad tokens.

use anyhow::Result;
use rayon::prelude::*;

use crate::model::Cfg;

/// exp(x) for x <= 0 as a polynomial the compiler vectorizes; from ai-tools `fast_exp`
/// (Cephes `exp2f` range reduction and coefficients, public domain, https://www.netlib.org/cephes/).
#[inline(always)]
fn fast_exp(x: f32) -> f32 {
    let x = x.max(-87.0);
    let t = x * std::f32::consts::LOG2_E;
    let i = t.round();
    let f = t - i;
    let p = 1.0
        + f * (6.931_472e-1 + f * (2.402_264_8e-1 + f * (5.550_332_5e-2 + f * (9.618_437e-3 + f * (1.339_887_4e-3 + f * 1.535_336_2e-4)))));
    f32::from_bits(((i as i32 + 127) << 23) as u32) * p
}

/// GELU in its exact `erf` form, with the Abramowitz-Stegun 7.1.26 `erf` (max abs error 1.5e-7).
#[inline(always)]
fn gelu(x: f32) -> f32 {
    let z = x * std::f32::consts::FRAC_1_SQRT_2;
    let a = z.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * a);
    let poly = t * (0.254_829_6 + t * (-0.284_496_74 + t * (1.421_413_7 + t * (-1.453_152_1 + t * 1.061_405_4))));
    let erf = (1.0 - poly * fast_exp(-a * a)).copysign(z);
    0.5 * x * (1.0 + erf)
}

/// C (m x n, row stride ldc) = A (m x k, row stride lda) * B^T, with B stored (n x k), row stride ldb.
#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
unsafe fn sgemm_bt(m: usize, n: usize, k: usize, a: *const f32, lda: usize, b: *const f32, ldb: usize, c: *mut f32, ldc: usize) {
    #[link(name = "Accelerate", kind = "framework")]
    unsafe extern "C" {
        fn cblas_sgemm(
            order: i32,
            ta: i32,
            tb: i32,
            m: i32,
            n: i32,
            k: i32,
            alpha: f32,
            a: *const f32,
            lda: i32,
            b: *const f32,
            ldb: i32,
            beta: f32,
            c: *mut f32,
            ldc: i32,
        );
    }
    unsafe { cblas_sgemm(101, 111, 112, m as i32, n as i32, k as i32, 1.0, a, lda as i32, b, ldb as i32, 0.0, c, ldc as i32) }
}

/// C (m x n) = A (m x k) * B (k x n), all row-major with the given row strides.
#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
unsafe fn sgemm_nn(m: usize, n: usize, k: usize, a: *const f32, lda: usize, b: *const f32, ldb: usize, c: *mut f32, ldc: usize) {
    #[link(name = "Accelerate", kind = "framework")]
    unsafe extern "C" {
        fn cblas_sgemm(
            order: i32,
            ta: i32,
            tb: i32,
            m: i32,
            n: i32,
            k: i32,
            alpha: f32,
            a: *const f32,
            lda: i32,
            b: *const f32,
            ldb: i32,
            beta: f32,
            c: *mut f32,
            ldc: i32,
        );
    }
    unsafe { cblas_sgemm(101, 111, 111, m as i32, n as i32, k as i32, 1.0, a, lda as i32, b, ldb as i32, 0.0, c, ldc as i32) }
}

#[cfg(not(target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
unsafe fn gemm_strided(
    m: usize,
    n: usize,
    k: usize,
    a: *const f32,
    lda: usize,
    b: *const f32,
    rhs_cs: isize,
    rhs_rs: isize,
    c: *mut f32,
    ldc: usize,
) {
    unsafe {
        // A large product threads itself; small ones (attention per head) run inside a rayon task already.
        let par = if m * n * k >= 1 << 22 { gemm::Parallelism::Rayon(0) } else { gemm::Parallelism::None };
        gemm::gemm(m, n, k, c, 1, ldc as isize, false, a, 1, lda as isize, b, rhs_cs, rhs_rs, 0.0, 1.0, false, false, false, par)
    }
}

#[cfg(not(target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
unsafe fn sgemm_bt(m: usize, n: usize, k: usize, a: *const f32, lda: usize, b: *const f32, ldb: usize, c: *mut f32, ldc: usize) {
    unsafe { gemm_strided(m, n, k, a, lda, b, ldb as isize, 1, c, ldc) }
}

#[cfg(not(target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
unsafe fn sgemm_nn(m: usize, n: usize, k: usize, a: *const f32, lda: usize, b: *const f32, ldb: usize, c: *mut f32, ldc: usize) {
    unsafe { gemm_strided(m, n, k, a, lda, b, 1, ldb as isize, c, ldc) }
}

/// A raw output pointer shared by tasks that write disjoint parts.
#[derive(Clone, Copy)]
struct Out(*mut f32);
unsafe impl Send for Out {}
unsafe impl Sync for Out {}
impl Out {
    unsafe fn at(self, offset: usize) -> *mut f32 {
        unsafe { self.0.add(offset) }
    }
}

/// Rows per parallel block for dense-layer epilogues.
const ROW_BLOCK: usize = 64;

/// y = x W^T (+ b), weights kept as stored (out x in).
struct Dense {
    w: Vec<f32>,
    b: Option<Vec<f32>>,
    i: usize,
    o: usize,
}

impl Dense {
    fn from(w: Vec<f32>, b: Option<Vec<f32>>, i: usize, o: usize) -> Self {
        Dense { w, b, i, o }
    }

    fn load(ws: &Weights, name: &str, i: usize, o: usize, bias: bool) -> Result<Self> {
        let w = ws.get(&format!("{name}.weight"))?;
        anyhow::ensure!(w.len() == i * o, "{name}: expected {o}x{i}");
        let b = if bias { Some(ws.get(&format!("{name}.bias"))?) } else { None };
        Ok(Dense::from(w, b, i, o))
    }

    /// Runs the product in row blocks in parallel; `epi(row index, row)` finishes each output row in cache.
    fn run(&self, x: &[f32], rows: usize, epi: impl Fn(usize, &mut [f32]) + Sync) -> Vec<f32> {
        let (i, o) = (self.i, self.o);
        let mut y = vec![0f32; rows * o];
        let finish = |r0: usize, out: &mut [f32]| {
            for (r, row) in out.chunks_mut(o).enumerate() {
                if let Some(b) = &self.b {
                    row.iter_mut().zip(b).for_each(|(v, b)| *v += b);
                }
                epi(r0 + r, row);
            }
        };
        // One call for all rows: Accelerate and gemm split a large product across cores themselves, and on Apple
        // chips parallel small calls fight over the shared matrix unit.
        unsafe { sgemm_bt(rows, o, i, x.as_ptr(), i, self.w.as_ptr(), i, y.as_mut_ptr(), o) };
        y.par_chunks_mut(ROW_BLOCK * o).enumerate().for_each(|(blk, out)| finish(blk * ROW_BLOCK, out));
        y
    }
}

/// The weights file, memory-mapped; each tensor is converted to f32 once, in parallel, when asked for.
struct Weights {
    _map: memmap2::Mmap,
    tensors: safetensors::SafeTensors<'static>,
}

impl Weights {
    fn open(path: &std::path::Path) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        let map = unsafe { memmap2::Mmap::map(&file)? };
        // The tensors borrow the map, which lives as long as `self`.
        let bytes: &'static [u8] = unsafe { std::slice::from_raw_parts(map.as_ptr(), map.len()) };
        Ok(Weights { tensors: safetensors::SafeTensors::deserialize(bytes)?, _map: map })
    }

    fn get(&self, name: &str) -> Result<Vec<f32>> {
        let t = self.tensors.tensor(name)?;
        let data = t.data();
        Ok(match t.dtype() {
            safetensors::Dtype::F16 => {
                let mut out = vec![0f32; data.len() / 2];
                out.par_chunks_mut(8192).zip(data.par_chunks(16384)).for_each(|(o, b)| {
                    for (o, b) in o.iter_mut().zip(b.chunks_exact(2)) {
                        *o = half::f16::from_le_bytes([b[0], b[1]]).to_f32();
                    }
                });
                out
            }
            safetensors::Dtype::F32 => data.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect(),
            other => anyhow::bail!("{name}: unsupported dtype {other:?}"),
        })
    }
}

struct Norm {
    w: Vec<f32>,
    b: Option<Vec<f32>>,
    eps: f32,
}

impl Norm {
    fn load(ws: &Weights, name: &str, d: usize, eps: f64, bias: bool) -> Result<Self> {
        let w = ws.get(&format!("{name}.weight"))?;
        anyhow::ensure!(w.len() == d, "{name}: expected {d}");
        Ok(Norm { w, b: if bias { Some(ws.get(&format!("{name}.bias"))?) } else { None }, eps: eps as f32 })
    }

    fn run(&self, x: &[f32], d: usize) -> Vec<f32> {
        let mut y = vec![0f32; x.len()];
        y.par_chunks_mut(d * 16).zip(x.par_chunks(d * 16)).for_each(|(out, rows)| {
            for (o, row) in out.chunks_mut(d).zip(rows.chunks(d)) {
                let mean = row.iter().sum::<f32>() / d as f32;
                let var = row.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / d as f32;
                let inv = 1.0 / (var + self.eps).sqrt();
                match &self.b {
                    Some(b) => o.iter_mut().zip(row).zip(&self.w).zip(b).for_each(|(((o, &v), &g), &b)| *o = (v - mean) * inv * g + b),
                    None => o.iter_mut().zip(row).zip(&self.w).for_each(|((o, &v), &g)| *o = (v - mean) * inv * g),
                }
            }
        });
        y
    }
}

struct Layer {
    attn_norm: Option<Norm>,
    qkv: Dense,
    wo: Dense,
    mlp_norm: Norm,
    wi: Dense,
    mlp_wo: Dense,
    local: bool,
}

struct HeadLayer {
    norm1: Norm,
    qkv: Dense,
    out: Dense,
    norm2: Norm,
    lin1: Dense,
    lin2: Dense,
}

pub struct CpuLaya {
    pub cfg: Cfg,
    emb: Vec<f32>,
    emb_norm: Norm,
    layers: Vec<Layer>,
    final_norm: Norm,
    type_noul: Vec<f32>,
    head: Vec<HeadLayer>,
    scorer_norm: Norm,
    scorer1: Dense,
    scorer2: Dense,
    inter: usize,
    /// cos and sin, (max_len, head_dim / 2), for global and local layers.
    rope: [(Vec<f32>, Vec<f32>); 2],
}

fn rope_table(dim: usize, theta: f64, max: usize) -> (Vec<f32>, Vec<f32>) {
    let half = dim / 2;
    let mut cos = vec![0f32; max * half];
    let mut sin = vec![0f32; max * half];
    for p in 0..max {
        for i in 0..half {
            let inv = 1.0 / theta.powf((2 * i) as f64 / dim as f64);
            let a = p as f64 * inv;
            cos[p * half + i] = a.cos() as f32;
            sin[p * half + i] = a.sin() as f32;
        }
    }
    (cos, sin)
}

impl CpuLaya {
    pub fn load(dir: &std::path::Path) -> Result<Self> {
        let cfg = Cfg::load(dir)?;
        let ws = Weights::open(&dir.join("model.safetensors"))?;
        let (d, e) = (cfg.hidden, cfg.eps);
        let inter = ws.tensors.tensor("encoder.layers.0.mlp.Wo.weight")?.shape()[1];
        let layers = (0..cfg.layers)
            .map(|i| -> Result<Layer> {
                let l = format!("encoder.layers.{i}");
                Ok(Layer {
                    attn_norm: if i == 0 { None } else { Some(Norm::load(&ws, &format!("{l}.attn_norm"), d, e, false)?) },
                    qkv: Dense::load(&ws, &format!("{l}.attn.Wqkv"), d, 3 * d, false)?,
                    wo: Dense::load(&ws, &format!("{l}.attn.Wo"), d, d, false)?,
                    mlp_norm: Norm::load(&ws, &format!("{l}.mlp_norm"), d, e, false)?,
                    wi: Dense::load(&ws, &format!("{l}.mlp.Wi"), d, 2 * inter, false)?,
                    mlp_wo: Dense::load(&ws, &format!("{l}.mlp.Wo"), inter, d, false)?,
                    local: i % cfg.global_every != 0,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let head = (0..cfg.head_layers)
            .map(|i| -> Result<HeadLayer> {
                let h = format!("head.layers.{i}");
                Ok(HeadLayer {
                    norm1: Norm::load(&ws, &format!("{h}.norm1"), d, 1e-5, true)?,
                    qkv: Dense::from(
                        ws.get(&format!("{h}.self_attn.in_proj_weight"))?,
                        Some(ws.get(&format!("{h}.self_attn.in_proj_bias"))?),
                        d,
                        3 * d,
                    ),
                    out: Dense::load(&ws, &format!("{h}.self_attn.out_proj"), d, d, true)?,
                    norm2: Norm::load(&ws, &format!("{h}.norm2"), d, 1e-5, true)?,
                    lin1: Dense::load(&ws, &format!("{h}.linear1"), d, 4 * d, true)?,
                    lin2: Dense::load(&ws, &format!("{h}.linear2"), 4 * d, d, true)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let dh = d / cfg.heads;
        let rope = [rope_table(dh, cfg.global_theta, cfg.max_len), rope_table(dh, cfg.local_theta, cfg.max_len)];
        let type_emb = ws.get("type_emb.weight")?;
        Ok(CpuLaya {
            emb: ws.get("encoder.embeddings.tok_embeddings.weight")?,
            emb_norm: Norm::load(&ws, "encoder.embeddings.norm", d, e, false)?,
            layers,
            final_norm: Norm::load(&ws, "encoder.final_norm", d, e, false)?,
            // qtype 2 is `noul`.
            type_noul: type_emb[2 * d..3 * d].to_vec(),
            head,
            scorer_norm: Norm::load(&ws, "scorer.0", d, 1e-5, true)?,
            scorer1: Dense::load(&ws, "scorer.1", d, d, true)?,
            scorer2: Dense::load(&ws, "scorer.3", d, 1, true)?,
            inter,
            rope,
            cfg,
        })
    }

    /// Self-attention over `qkv` rows (T x 3d), one task per (sequence, head); `window` limits |i - j| when set.
    fn attention(&self, qkv: &mut [f32], seqs: &[(usize, usize)], rope: Option<&(Vec<f32>, Vec<f32>)>, window: Option<usize>) -> Vec<f32> {
        let d = self.cfg.hidden;
        let heads = self.cfg.heads;
        let dh = d / heads;
        let half = dh / 2;
        let total = qkv.len() / (3 * d);
        if let Some((cos, sin)) = rope {
            // Rotate q and k in place; a token's position is its index in its own sequence.
            let pos: Vec<usize> = seqs.iter().flat_map(|&(_, n)| 0..n).collect();
            qkv.par_chunks_mut(3 * d).zip(pos.par_iter()).for_each(|(row, &p)| {
                let (c, s) = (&cos[p * half..(p + 1) * half], &sin[p * half..(p + 1) * half]);
                for part in 0..2 {
                    for h in 0..heads {
                        let x = &mut row[part * d + h * dh..part * d + (h + 1) * dh];
                        for i in 0..half {
                            let (a, b) = (x[i], x[i + half]);
                            x[i] = a * c[i] - b * s[i];
                            x[i + half] = b * c[i] + a * s[i];
                        }
                    }
                }
            });
        }
        let mut out = vec![0f32; total * d];
        let o = Out(out.as_mut_ptr());
        let q_ptr = Out(qkv.as_mut_ptr());
        let scale = (dh as f32).powf(-0.5);
        let tasks: Vec<(usize, usize, usize)> = seqs.iter().flat_map(|&(start, n)| (0..heads).map(move |h| (start, n, h))).collect();
        tasks.par_iter().for_each(|&(start, n, h)| {
            let mut scores = vec![0f32; n * n];
            unsafe {
                let base = q_ptr.at(start * 3 * d);
                let (q, k, v) = (base.add(h * dh), base.add(d + h * dh), base.add(2 * d + h * dh));
                sgemm_bt(n, n, dh, q, 3 * d, k, 3 * d, scores.as_mut_ptr(), n);
                for (i, row) in scores.chunks_mut(n).enumerate() {
                    let (lo, hi) = match window {
                        Some(w) => (i.saturating_sub(w), (i + w + 1).min(n)),
                        None => (0, n),
                    };
                    let mut max = f32::NEG_INFINITY;
                    for v in &row[lo..hi] {
                        max = max.max(*v * scale);
                    }
                    let mut sum = 0.0;
                    for (j, v) in row.iter_mut().enumerate() {
                        *v = if j < lo || j >= hi { 0.0 } else { fast_exp(*v * scale - max) };
                        sum += *v;
                    }
                    let inv = 1.0 / sum;
                    row.iter_mut().for_each(|v| *v *= inv);
                }
                sgemm_nn(n, dh, n, scores.as_ptr(), n, v, 3 * d, o.at(start * d + h * dh), d);
            }
        });
        out
    }

    /// Raw logits `[false, true]` for each sequence and its two marker positions.
    pub fn forward(&self, ids: &[Vec<u32>], markers: &[[usize; 2]]) -> Vec<[f32; 2]> {
        let d = self.cfg.hidden;
        let mut seqs = vec![];
        let mut total = 0;
        for s in ids {
            seqs.push((total, s.len()));
            total += s.len();
        }
        let flat: Vec<u32> = ids.iter().flatten().copied().collect();
        let mut x = vec![0f32; total * d];
        x.par_chunks_mut(d).zip(flat.par_iter()).for_each(|(row, &t)| row.copy_from_slice(&self.emb[t as usize * d..(t as usize + 1) * d]));
        let mut h = self.emb_norm.run(&x, d);
        let half_window = self.cfg.local_window / 2;
        for l in &self.layers {
            let n = match &l.attn_norm {
                Some(n) => n.run(&h, d),
                None => h.clone(),
            };
            let mut qkv = l.qkv.run(&n, total, |_, _| {});
            let rope = if l.local { &self.rope[1] } else { &self.rope[0] };
            let att = self.attention(&mut qkv, &seqs, Some(rope), l.local.then_some(half_window));
            h = l.wo.run(&att, total, |r, row| row.iter_mut().zip(&h[r * d..(r + 1) * d]).for_each(|(v, s)| *v += s));
            let n = l.mlp_norm.run(&h, d);
            // GeGLU in the epilogue: gelu(first half) * second half, written into the first half of the row.
            let inter = self.inter;
            let m = l.wi.run(&n, total, |_, row| {
                let (a, b) = row.split_at_mut(inter);
                a.iter_mut().zip(b.iter()).for_each(|(a, b)| *a = gelu(*a) * b);
            });
            let g: Vec<f32> = m.par_chunks(2 * inter).flat_map_iter(|row| row[..inter].iter().copied()).collect();
            h = l.mlp_wo.run(&g, total, |r, row| row.iter_mut().zip(&h[r * d..(r + 1) * d]).for_each(|(v, s)| *v += s));
        }
        let mut h = self.final_norm.run(&h, d);
        h.par_chunks_mut(d).for_each(|row| row.iter_mut().zip(&self.type_noul).for_each(|(v, t)| *v += t));
        for l in &self.head {
            let n = l.norm1.run(&h, d);
            let mut qkv = l.qkv.run(&n, total, |_, _| {});
            let att = self.attention(&mut qkv, &seqs, None, None);
            h = l.out.run(&att, total, |r, row| row.iter_mut().zip(&h[r * d..(r + 1) * d]).for_each(|(v, s)| *v += s));
            let n = l.norm2.run(&h, d);
            let f = l.lin1.run(&n, total, |_, row| row.iter_mut().for_each(|v| *v = v.max(0.0)));
            h = l.lin2.run(&f, total, |r, row| row.iter_mut().zip(&h[r * d..(r + 1) * d]).for_each(|(v, s)| *v += s));
        }
        let hr = &h;
        let picked: Vec<f32> = seqs
            .iter()
            .zip(markers)
            .flat_map(|(&(start, _), m)| m.iter().flat_map(move |&p| hr[(start + p) * d..(start + p + 1) * d].iter().copied()))
            .collect();
        let rows = picked.len() / d;
        let s = self.scorer_norm.run(&picked, d);
        let s = self.scorer1.run(&s, rows, |_, row| row.iter_mut().for_each(|v| *v = gelu(*v)));
        let logit = self.scorer2.run(&s, rows, |_, _| {});
        logit.chunks(2).map(|c| [c[0], c[1]]).collect()
    }
}
