//! The `bm25-bert` backend: BM25 candidates reranked by their cosine to the query under a local embedding model.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rkb_core::doctor::{Check, Level};
use rkb_core::rerank::Reranker;
use rkb_core::{lock, paths, search};
use rkb_embed::{DIM, DOCUMENT_PREFIX, Embedder, QUERY_PREFIX, files};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::output::{CliError, ErrorCode, Output, paint};

pub const NAME: &str = "bm25-bert";

/// Measured offline on 184 real failures: a cosine plus this share of the BM25 place breaks near
/// ties toward BM25's order. It sorts only; the relevance a hook reads is the cosine alone.
const PRIOR: f32 = 0.05;

/// A search embeds at most this many missing lesson texts and example queries itself; more means a bulk change (a sync, a new
/// KB), which a background `rkb models warm` fills so the hook's time limit is not spent on it.
const INLINE_MISSES: usize = 3;

pub trait Embed {
    fn embed(&self, text: &str) -> Result<Vec<f32>, String>;
}

impl Embed for Embedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        Embedder::embed(self, &[text]).map(|mut v| v.remove(0)).map_err(|e| format!("{e:#}"))
    }
}

/// Lesson vectors on this machine, keyed by the SHA-256 of the exact text embedded; one file per model revision.
struct Cache {
    path: PathBuf,
    vectors: HashMap<String, Vec<f32>>,
}

fn cache_path() -> PathBuf {
    paths::cache_dir().join(format!("embeddings-{}.txt", &files::REVISION[..8]))
}

fn key(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn document(item: &str) -> String {
    format!("{DOCUMENT_PREFIX}{item}")
}

fn query(text: &str) -> String {
    format!("{QUERY_PREFIX}{text}")
}

/// Every text a search scores: each lesson text, then each lesson's example queries.
fn texts(items: &[String], queries: &[Vec<String>]) -> Vec<String> {
    items.iter().map(|i| document(i)).chain(queries.iter().flatten().map(|q| query(q))).collect()
}

impl Cache {
    /// Lines are `key hex-of-768-f32`; a torn or unreadable line is skipped.
    fn open(path: PathBuf) -> Cache {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let vectors = text.lines().filter_map(|l| l.split_once(' ')).filter_map(|(k, h)| Some((k.to_string(), decode(h)?))).collect();
        Cache { path, vectors }
    }

    fn add(&mut self, key: String, v: Vec<f32>) {
        let line = format!("{key} {}", v.iter().flat_map(|x| x.to_le_bytes()).map(|b| format!("{b:02x}")).collect::<String>());
        // A failed write only costs the next search this embedding.
        let _ = lock::append_line(&self.path, &line);
        self.vectors.insert(key, v);
    }
}

fn decode(hex: &str) -> Option<Vec<f32>> {
    if hex.len() != DIM * 8 || !hex.is_ascii() {
        return None;
    }
    (0..DIM)
        .map(|i| {
            let mut b = [0u8; 4];
            for (j, byte) in b.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&hex[i * 8 + j * 2..i * 8 + j * 2 + 2], 16).ok()?;
            }
            Some(f32::from_le_bytes(b))
        })
        .collect()
}

struct Bert<E> {
    embedder: E,
    cache: PathBuf,
    /// Starts the background warm-up.
    warm: fn(),
}

fn warm_lock() -> PathBuf {
    paths::cache_dir().join("embeddings-warm.lock")
}

/// Runs `rkb models warm` detached, one at a time: the lock file is created exclusively and `models warm`
/// removes it; a lock older than ten minutes belongs to a warm-up that died.
fn spawn_warm() {
    let lock = warm_lock();
    let stale = std::fs::metadata(&lock)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|e| e > Duration::from_secs(600));
    if stale {
        let _ = std::fs::remove_file(&lock);
    }
    let created =
        std::fs::create_dir_all(paths::cache_dir()).is_ok() && std::fs::OpenOptions::new().write(true).create_new(true).open(&lock).is_ok();
    if !created {
        return;
    }
    let spawned = std::env::current_exe().and_then(|exe| {
        Command::new(exe).args(["models", "warm"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
    });
    if spawned.is_err() {
        let _ = std::fs::remove_file(&lock);
    }
}

/// Embeds what the cache lacks, appending each vector as it is made so a search cut short by its timeout still leaves progress.
fn fill(embedder: &impl Embed, cache: &mut Cache, texts: &[String]) -> Result<(), String> {
    for text in texts {
        let k = key(text);
        if !cache.vectors.contains_key(&k) {
            cache.add(k, embedder.embed(text)?);
        }
    }
    Ok(())
}

impl<E: Embed + Send> Reranker for Bert<E> {
    /// The cosine of the query to each item or to any of its example queries, whichever is higher, cut to [0, 1].
    fn score(&self, query_text: &str, items: &[String], queries: &[Vec<String>]) -> Result<Vec<f32>, String> {
        let mut cache = Cache::open(self.cache.clone());
        let wanted = texts(items, queries);
        let missing = wanted.iter().filter(|t| !cache.vectors.contains_key(&key(t))).count();
        if missing > INLINE_MISSES {
            (self.warm)();
            return Err(format!(
                "{missing} of {} lesson texts and example queries are not embedded yet; embedding them in the background",
                wanted.len()
            ));
        }
        let q = self.embedder.embed(&query(query_text))?;
        fill(&self.embedder, &mut cache, &wanted)?;
        let cosine = |text: String| q.iter().zip(&cache.vectors[&key(&text)]).map(|(a, b)| a * b).sum::<f32>();
        Ok(items
            .iter()
            .enumerate()
            .map(|(i, it)| {
                let examples = queries.get(i).into_iter().flatten().map(|e| cosine(query(e)));
                examples.fold(cosine(document(it)), f32::max).clamp(0.0, 1.0)
            })
            .collect())
    }

    fn order(&self, relevance: &[f32]) -> Option<Vec<f32>> {
        Some(relevance.iter().enumerate().map(|(p, c)| c + PRIOR / (2.0 + p as f32)).collect())
    }
}

pub fn open() -> Result<Box<dyn Reranker>, String> {
    // A search embeds one query, which the CPU does as fast as Metal, without Metal's shader compile on a cold start.
    let cpu = std::env::var("RKB_EMBED_DEVICE").map_or(true, |v| v != "gpu");
    let embedder = Embedder::open(&files::default_dir(), cpu).map_err(|e| format!("{e:#}"))?;
    Ok(Box::new(Bert { embedder, cache: cache_path(), warm: spawn_warm }))
}

/// `rkb doctor`: the model files and the cache of lesson vectors.
pub fn checks(root: &Path) -> Vec<Check> {
    let check = |level, detail: String, fix: Option<&str>| vec![Check { name: NAME, level, detail, fix: fix.map(String::from) }];
    if let Err(e) = files::check(&files::default_dir()) {
        return check(Level::Warn, format!("{e:#}"), Some("rkb models fetch"));
    }
    let Ok((items, queries)) = search::active_rerank_items(root) else { return vec![] };
    let cache = Cache::open(cache_path());
    let wanted = texts(&items, &queries);
    let missing = wanted.iter().filter(|t| !cache.vectors.contains_key(&key(t))).count();
    if missing > 0 {
        let detail = format!(
            "{missing} of {} lesson texts and example queries are not embedded yet, so searches embed them in the background",
            wanted.len()
        );
        return check(Level::Warn, detail, Some("rkb models warm"));
    }
    check(Level::Ok, format!("model present; {} lessons and {} example queries embedded", items.len(), wanted.len() - items.len()), None)
}

fn io_error(what: &str, e: impl std::fmt::Display, fix: &str) -> CliError {
    CliError::new(ErrorCode::Io, format!("{what}: {e}"), fix)
}

pub fn models_fetch(colored: bool) -> Result<Output, CliError> {
    let dir = files::default_dir();
    let base = std::env::var("RKB_MODELS_URL").unwrap_or_else(|_| "https://huggingface.co".into());
    let f = files::fetch(&dir, &base).map_err(|e| {
        io_error(
            "downloading the model failed",
            format!("{e:#}"),
            "check the network, the disk space and the TLS proxy certificates, then run `rkb models fetch` again",
        )
    })?;
    let mut human = String::new();
    for name in &f.downloaded {
        human.push_str(&format!("{} {name}\n", paint(colored, "32", "downloaded")));
    }
    for name in &f.present {
        human.push_str(&format!("present    {name}\n"));
    }
    human.push_str(&format!("{:.0} MB downloaded into {}; every file matches its pinned hash", f.bytes as f64 / 1e6, dir.display()));
    let data = json!({
        "dir": dir.display().to_string(),
        "revision": files::REVISION,
        "downloaded": f.downloaded,
        "present": f.present,
        "bytes": f.bytes,
        "help": ["Run `rkb models warm` to embed the lessons, and add \"bm25-bert\" to `chain` under [rerank] in kb.toml"],
    });
    Ok(Output { data, human, exit: 0, raw: false })
}

pub fn models_warm(root: &Path) -> Result<Output, CliError> {
    let out = warm(root);
    let _ = std::fs::remove_file(warm_lock());
    out
}

fn warm(root: &Path) -> Result<Output, CliError> {
    let start = Instant::now();
    let embedder = Embedder::open(&files::default_dir(), std::env::var("RKB_EMBED_DEVICE").is_ok_and(|v| v == "cpu"))
        .map_err(|e| io_error("the model did not load", format!("{e:#}"), "run `rkb models fetch`"))?;
    let (items, queries) = search::active_rerank_items(root)?;
    let wanted = texts(&items, &queries);
    let mut cache = Cache::open(cache_path());
    let before = cache.vectors.len();
    fill(&embedder, &mut cache, &wanted).map_err(|e| io_error("embedding failed", e, "run `rkb models warm` again"))?;
    let (device, secs) = (embedder.device(), start.elapsed().as_secs_f64());
    let added = cache.vectors.len() - before;
    let human = format!(
        "{added} lesson texts and example queries embedded, {} already in the cache ({device}, {secs:.1} s)",
        wanted.len().saturating_sub(added)
    );
    let data = json!({ "lessons": items.len(), "queries": wanted.len() - items.len(), "embedded": added, "device": device, "seconds": (secs * 10.0).round() / 10.0 });
    Ok(Output { data, human, exit: 0, raw: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rkb_core::rerank::{self, Opener, Settings};
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::sync::Arc;
    use std::time::Duration;

    /// A word-count vector: texts that share words are close.
    struct Words;

    impl Embed for Words {
        fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
            let mut v = vec![0.0; DIM];
            for w in text.split_whitespace().filter(|w| !w.ends_with(':')) {
                v[w.bytes().map(usize::from).sum::<usize>() % DIM] += 1.0;
            }
            let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
            Ok(v.into_iter().map(|x| x / n).collect())
        }
    }

    fn bert(dir: &Path) -> Bert<Words> {
        Bert { embedder: Words, cache: dir.join("cache.txt"), warm: || {} }
    }

    fn items(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|t| t.to_string()).collect()
    }

    #[test]
    fn relevance_is_the_pure_cosine_and_order_adds_the_bm25_prior() {
        let dir = tempfile::tempdir().unwrap();
        let b = bert(dir.path());
        let it = items(&["linker vtable undefined", "slurm binding cpu", "linker vtable undefined"]);
        let rel = b.score("undefined vtable linker", &it, &[]).unwrap();
        assert!((rel[0] - 1.0).abs() < 1e-5 && rel[1] == 0.0 && rel[0] == rel[2], "{rel:?}");
        let order = b.order(&rel).unwrap();
        for (p, (o, r)) in order.iter().zip(&rel).enumerate() {
            assert!((o - (r + 0.05 / (2.0 + p as f32))).abs() < 1e-6, "{p}: {o} {r}");
        }
        assert!(order[0] > order[2], "equal cosines keep BM25 order");
        let close = [0.50, 0.505];
        assert_eq!(b.order(&close).map(|o| o[0] > o[1]), Some(true), "a 0.005 lead does not beat the prior of the first place");
        let clear = [0.50, 0.60];
        assert_eq!(b.order(&clear).map(|o| o[0] > o[1]), Some(false));
    }

    #[test]
    fn the_chain_sorts_by_blend_and_reports_cosine() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache.txt");
        let opener: Opener =
            Arc::new(move |_: &str| Ok(Box::new(Bert { embedder: Words, cache: cache.clone(), warm: || {} }) as Box<dyn Reranker>));
        let it = items(&["slurm binding cpu", "linker vtable undefined", "linker vtable"]);
        let r = rerank::run(&Settings::default(), Some(NAME), &opener, "linker vtable", &it, &[], Duration::from_secs(5), &BTreeMap::new())
            .unwrap();
        let scores: Vec<f32> = r.scores.unwrap().into_iter().map(Option::unwrap).collect();
        assert!(scores[0] == 0.0 && scores[2] > scores[1] && scores[2] > 0.99, "{scores:?}");
        let order = r.order.unwrap();
        assert_eq!(order.len(), 3);
        assert!((order[1] - (scores[1] + 0.05 / 3.0)).abs() < 1e-6, "the sort key is not the reported relevance");
    }

    #[test]
    fn the_cache_is_keyed_by_the_exact_text_and_survives_a_torn_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.txt");
        let mut c = Cache::open(path.clone());
        fill(&Words, &mut c, &texts(&items(&["a b", "c d"]), &[])).unwrap();
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"deadbeef 12\ntorn").unwrap_or_default();
        let again = Cache::open(path);
        assert_eq!(again.vectors.len(), 2);
        assert_eq!(again.vectors[&key("search_document: a b")], Words.embed("search_document: a b").unwrap());
        assert!(!again.vectors.contains_key(&key("a b")), "the prefix is part of the key");
    }

    #[test]
    fn a_warm_cache_embeds_only_the_query() {
        struct Counting(std::sync::atomic::AtomicUsize);
        impl Embed for &Counting {
            fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Words.embed(text)
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let n = Counting(Default::default());
        let b = Bert { embedder: &n, cache: dir.path().join("c.txt"), warm: || {} };
        let it = items(&["a b", "c d", "e f"]);
        b.score("a", &it, &[]).unwrap();
        assert_eq!(n.0.load(std::sync::atomic::Ordering::SeqCst), 4, "query plus three lessons when cold");
        b.score("c", &it, &[]).unwrap();
        assert_eq!(n.0.load(std::sync::atomic::Ordering::SeqCst), 5, "only the query when warm");
    }

    #[test]
    fn a_failing_embedder_is_a_skip_reason() {
        struct Broken;
        impl Embed for Broken {
            fn embed(&self, _: &str) -> Result<Vec<f32>, String> {
                Err("out of memory".into())
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let b = Bert { embedder: Broken, cache: dir.path().join("c.txt"), warm: || {} };
        assert_eq!(b.score("q", &items(&["a"]), &[]).unwrap_err(), "out of memory");
    }

    #[test]
    fn a_bulk_miss_starts_the_background_warm_up_and_skips_the_backend() {
        static STARTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = tempfile::tempdir().unwrap();
        let b = Bert {
            embedder: Words,
            cache: dir.path().join("c.txt"),
            warm: || {
                STARTED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            },
        };
        let err = b.score("q", &items(&["a", "b", "c", "d"]), &[]).unwrap_err();
        assert!(err.starts_with("4 of 4 lesson texts and example queries are not embedded yet"), "{err}");
        assert_eq!(STARTED.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(!dir.path().join("c.txt").exists(), "nothing was embedded inline");
        b.score("q", &items(&["a", "b", "c"]), &[]).unwrap();
        assert_eq!(STARTED.load(std::sync::atomic::Ordering::SeqCst), 1, "three misses are embedded inline");
    }

    #[test]
    fn an_example_query_can_beat_the_lesson_text() {
        let dir = tempfile::tempdir().unwrap();
        let b = bert(dir.path());
        let it = items(&["slurm binding cpu", "linker vtable undefined"]);
        let none: Vec<Vec<String>> = vec![vec![], vec![]];
        let plain = b.score("segfault at exit", &it, &none).unwrap();
        assert_eq!(plain, [0.0, 0.0]);
        let with = vec![vec!["segfault at exit".to_string(), "never matches".to_string()], vec![]];
        let rel = b.score("segfault at exit", &it, &with).unwrap();
        assert!((rel[0] - 1.0).abs() < 1e-5 && rel[1] == 0.0, "{rel:?}");
        let text_wins = vec![vec!["unrelated words".to_string()], vec![]];
        assert!((b.score("slurm binding cpu", &it, &text_wins).unwrap()[0] - 1.0).abs() < 1e-5, "the text still counts");
    }

    #[test]
    fn queries_are_cached_under_the_query_prefix_and_count_as_misses() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("c.txt");
        let b = Bert { embedder: Words, cache: cache.clone(), warm: || {} };
        let it = items(&["a b"]);
        b.score("q", &it, &[vec!["x y".into()]]).unwrap();
        let c = Cache::open(cache);
        assert!(c.vectors.contains_key(&key("search_query: x y")) && c.vectors.contains_key(&key("search_document: a b")));
        assert_eq!(texts(&it, &[vec!["x y".into(), "z".into()]]).len(), 3);
        let cold = bert(&dir.path().join("cold"));
        let err = cold.score("q", &it, &[vec!["1".into(), "2".into(), "3".into()]]).unwrap_err();
        assert!(err.starts_with("4 of 4 lesson texts"), "a lesson and its three queries are four misses: {err}");
    }
}
