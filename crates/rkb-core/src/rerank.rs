//! The rerank chain: try each backend in order, fall back to BM25, and say why a backend was skipped.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::config::{self, KbConfig};
use crate::error::{Error, Result};

/// A model that scores how well each item fits a query.
pub trait Reranker: Send {
    /// Where it runs, such as `gpu` or `cpu`.
    fn device(&self) -> Option<String>;
    /// Relevance of each item in [0, 1], in the items' order.
    fn score(&self, query: &str, items: &[String]) -> std::result::Result<Vec<f32>, String>;
}

/// Opens a backend by name, or says why it cannot run.
pub type Opener = Arc<dyn Fn(&str) -> std::result::Result<Box<dyn Reranker>, String> + Send + Sync>;

pub const BM25: &str = "bm25";

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub chain: Vec<String>,
    pub strict: bool,
    pub timeout_ms: u64,
    /// How many BM25 candidates a model scores.
    pub top: usize,
    /// `cpu` forces the CPU engine; anything else lets the backend choose.
    pub device: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { chain: vec![BM25.into()], strict: false, timeout_ms: 2000, top: 20, device: None }
    }
}

impl Settings {
    /// `[rerank]` from `kb.toml`; a missing key keeps its default.
    pub fn load(root: &Path) -> Self {
        let kb: KbConfig = std::fs::read_to_string(root.join("kb.toml")).ok().and_then(|t| config::parse(&t).ok()).unwrap_or_default();
        let mut s = Settings::default();
        let Some(t) = kb.rerank else { return s };
        if let Some(c) = t.get("chain").and_then(|v| v.as_array()) {
            s.chain = c.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
        }
        if let Some(v) = t.get("strict").and_then(|v| v.as_bool()) {
            s.strict = v;
        }
        if let Some(v) = t.get("timeout_ms").and_then(|v| v.as_integer()) {
            s.timeout_ms = v.max(1) as u64;
        }
        if let Some(v) = t.get("top").and_then(|v| v.as_integer()) {
            s.top = v.max(1) as usize;
        }
        s.device = t.get("device").and_then(|v| v.as_str()).map(str::to_string);
        s
    }
}

/// Which backend ranked the results and what the others said.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    pub backend: String,
    pub device: Option<String>,
    /// One relevance per item from a model; `None` when BM25 ranked them.
    pub scores: Option<Vec<f32>>,
    /// Backends tried before, with the reason each was skipped.
    pub skipped: Vec<(String, String)>,
}

impl Ranked {
    fn bm25(skipped: Vec<(String, String)>) -> Self {
        Ranked { backend: BM25.into(), device: None, scores: None, skipped }
    }

    /// `laya (gpu)`, or `bm25 (laya: timeout after 2000 ms)`.
    pub fn describe(&self) -> String {
        let mut out = match &self.device {
            Some(d) => format!("{} ({d})", self.backend),
            None => self.backend.clone(),
        };
        if !self.skipped.is_empty() {
            let why: Vec<String> = self.skipped.iter().map(|(b, r)| format!("{b}: {r}")).collect();
            out.push_str(&format!(" ({})", why.join("; ")));
        }
        out
    }
}

/// Runs the chain: `only` replaces it for one call; `RKB_NO_MODEL=1` forces BM25 over everything.
/// Each model backend is opened and asked on a worker thread, so `timeout` covers loading too.
/// A backend in `blocked` is skipped with its reason before it is opened, such as Jev when a
/// candidate may not leave the machine.
pub fn run(
    settings: &Settings,
    only: Option<&str>,
    opener: &Opener,
    query: &str,
    items: &[String],
    timeout: Duration,
    blocked: &BTreeMap<String, String>,
) -> Result<Ranked> {
    if std::env::var("RKB_NO_MODEL").is_ok_and(|v| v == "1") {
        return Ok(Ranked::bm25(vec![]));
    }
    let chain: Vec<String> = match only {
        Some(b) => vec![b.to_string()],
        None => settings.chain.clone(),
    };
    let mut skipped = vec![];
    for name in chain {
        if name == BM25 || items.is_empty() {
            return Ok(Ranked::bm25(skipped));
        }
        let reason = match blocked.get(&name) {
            Some(reason) => reason.clone(),
            None => match ask(&name, opener, query, items, timeout) {
                Ok((scores, device)) => return Ok(Ranked { backend: name, device, scores: Some(scores), skipped }),
                Err(reason) => reason,
            },
        };
        if settings.strict {
            return Err(Error::Refused(format!(
                "rerank backend {name} did not answer: {reason}; `strict = true` stops instead of falling back"
            )));
        }
        skipped.push((name, reason));
    }
    Ok(Ranked::bm25(skipped))
}

fn ask(
    name: &str,
    opener: &Opener,
    query: &str,
    items: &[String],
    timeout: Duration,
) -> std::result::Result<(Vec<f32>, Option<String>), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let (opener, name_owned, query, items) = (opener.clone(), name.to_string(), query.to_string(), items.to_vec());
    // The worker is never joined: a model cannot be interrupted, and the process ends with the command.
    std::thread::spawn(move || {
        let answer = opener(&name_owned).and_then(|r| {
            let scores = r.score(&query, &items)?;
            if scores.len() != items.len() {
                return Err(format!("returned {} scores for {} items", scores.len(), items.len()));
            }
            Ok((scores, r.device()))
        });
        let _ = tx.send(answer);
    });
    match rx.recv_timeout(timeout) {
        Ok(answer) => answer,
        Err(_) => Err(format!("timeout after {} ms", timeout.as_millis())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        delay: Duration,
        fail: bool,
    }

    impl Reranker for Fake {
        fn device(&self) -> Option<String> {
            Some("cpu".into())
        }
        fn score(&self, _: &str, items: &[String]) -> std::result::Result<Vec<f32>, String> {
            std::thread::sleep(self.delay);
            if self.fail { Err("model crashed".into()) } else { Ok((0..items.len()).map(|i| i as f32 / 10.0).collect()) }
        }
    }

    fn opener() -> Opener {
        Arc::new(|name: &str| -> std::result::Result<Box<dyn Reranker>, String> {
            match name {
                "fast" => Ok(Box::new(Fake { delay: Duration::ZERO, fail: false })),
                "slow" => Ok(Box::new(Fake { delay: Duration::from_secs(5), fail: false })),
                "broken" => Ok(Box::new(Fake { delay: Duration::ZERO, fail: true })),
                _ => Err("not configured".into()),
            }
        })
    }

    fn settings(chain: &[&str], strict: bool) -> Settings {
        Settings { chain: chain.iter().map(|s| s.to_string()).collect(), strict, ..Settings::default() }
    }

    fn items() -> Vec<String> {
        vec!["a".into(), "b".into(), "c".into()]
    }

    #[test]
    fn first_that_answers_wins() {
        let r = run(
            &settings(&["missing", "broken", "fast", BM25], false),
            None,
            &opener(),
            "q",
            &items(),
            Duration::from_secs(2),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(r.backend, "fast");
        assert_eq!(r.scores.as_deref(), Some(&[0.0, 0.1, 0.2][..]));
        assert_eq!(r.describe(), "fast (cpu) (missing: not configured; broken: model crashed)");
    }

    #[test]
    fn a_blocked_backend_is_skipped_unopened() {
        let blocked = BTreeMap::from([("fast".to_string(), "a candidate is not allowed by sinks.fast".to_string())]);
        let r = run(&settings(&["fast", BM25], false), None, &opener(), "q", &items(), Duration::from_secs(1), &blocked).unwrap();
        assert_eq!(r.describe(), "bm25 (fast: a candidate is not allowed by sinks.fast)");
        let e = run(&settings(&["fast"], true), None, &opener(), "q", &items(), Duration::from_secs(1), &blocked).unwrap_err();
        assert!(e.to_string().contains("not allowed by sinks.fast"), "{e}");
    }

    #[test]
    fn fallback_timeout_and_strict() {
        let r =
            run(&settings(&["slow", BM25], false), None, &opener(), "q", &items(), Duration::from_millis(100), &BTreeMap::new()).unwrap();
        assert_eq!(r.describe(), "bm25 (slow: timeout after 100 ms)");
        let r = run(&settings(&["missing"], false), None, &opener(), "q", &items(), Duration::from_secs(1), &BTreeMap::new()).unwrap();
        assert_eq!((r.backend.as_str(), r.scores.is_none()), (BM25, true), "an exhausted chain falls back to BM25");
        let e = run(&settings(&["missing"], true), None, &opener(), "q", &items(), Duration::from_secs(1), &BTreeMap::new())
            .unwrap_err()
            .to_string();
        assert!(e.contains("missing") && e.contains("not configured") && e.contains("strict"), "{e}");
        let r = run(&settings(&[BM25], true), Some("fast"), &opener(), "q", &items(), Duration::from_secs(1), &BTreeMap::new()).unwrap();
        assert_eq!(r.backend, "fast", "--rerank replaces the chain");
    }

    #[test]
    fn settings_from_kb_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("kb.toml"), "[rerank]\nchain = [\"laya\", \"bm25\"]\nstrict = true\ntop = 15\ndevice = \"cpu\"\n")
            .unwrap();
        let s = Settings::load(dir.path());
        assert_eq!(
            (s.chain.clone(), s.strict, s.timeout_ms, s.top, s.device.as_deref()),
            (vec!["laya".to_string(), BM25.to_string()], true, 2000, 15, Some("cpu"))
        );
        assert_eq!(Settings::load(&dir.path().join("none")), Settings::default());
    }
}
