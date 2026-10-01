//! The backends the rerank chain can open in this build.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rkb_core::config::{self, KbConfig};
use rkb_core::doctor::{Check, Level};
use rkb_core::kb::Snapshot;
use rkb_core::leak::LeakScanner;
use rkb_core::rerank::{self, Opener, Ranked, Reranker, Settings};
use rkb_core::search::{self, Hit};

pub fn opener() -> Opener {
    Arc::new(|name: &str| match name {
        JEV => jev(),
        BERT => bert(),
        other => Err(format!("unknown backend `{other}`; this build knows jev, bm25-bert and bm25")),
    })
}

const BERT: &str = "bm25-bert";

#[cfg(feature = "bert")]
fn bert() -> Result<Box<dyn Reranker>, String> {
    crate::embedding::open()
}

#[cfg(not(feature = "bert"))]
fn bert() -> Result<Box<dyn Reranker>, String> {
    Err("not in this build".into())
}

/// The doctor checks for the `bm25-bert` backend, when the chain has it: the model files and the vector cache.
pub fn bert_checks(root: &Path, settings: &Settings) -> Vec<Check> {
    if !settings.chain.iter().any(|b| b == BERT) {
        return vec![];
    }
    #[cfg(feature = "bert")]
    return crate::embedding::checks(root);
    #[cfg(not(feature = "bert"))]
    {
        let _ = root;
        vec![Check {
            name: BERT,
            level: Level::Warn,
            detail: "not in this build".into(),
            fix: Some("install an rkb built with the default features".into()),
        }]
    }
}

const JEV_URL: &str = "https://api.typesafe.ai/v1/systemone";
const JEV_QUESTION: &str = "Is this lesson about the same problem as the user describes?";

/// The Jev API (TypeSafe System One) as a reranker: one `noul` question per candidate.
struct JevBackend {
    key: String,
    url: String,
    model: String,
    /// One request with every candidate numbered in the state; else one request per candidate.
    batch: bool,
}

fn jev() -> Result<Box<dyn Reranker>, String> {
    let key = jev_key()?;
    let root = rkb_core::kb::home();
    let cfg = std::fs::read_to_string(root.join("kb.toml"))
        .ok()
        .and_then(|t| config::parse::<KbConfig>(&t).ok())
        .and_then(|c| c.jev)
        .unwrap_or_default();
    Ok(Box::new(JevBackend {
        key,
        url: std::env::var("RKB_JEV_URL").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| JEV_URL.into()),
        model: cfg.get("model").and_then(|v| v.as_str()).unwrap_or("jev-latest").to_string(),
        batch: cfg.get("batch").and_then(|v| v.as_bool()).unwrap_or(true),
    }))
}

/// Where the Jev key comes from, for `rkb doctor`, without the key itself.
pub fn jev_key_source() -> Result<&'static str, String> {
    jev_key_from().map(|(_, s)| s)
}

fn jev_key() -> Result<String, String> {
    jev_key_from().map(|(k, _)| k)
}

/// `RKB_JEV_API_KEY`, then `[jev] api_key` in `config.toml` (only with mode 0600), then `[jev] api_key_cmd`.
/// No error or reason ever holds the key.
fn jev_key_from() -> Result<(String, &'static str), String> {
    if let Some(k) = std::env::var("RKB_JEV_API_KEY").ok().filter(|k| !k.trim().is_empty()) {
        return Ok((k.trim().to_string(), "RKB_JEV_API_KEY"));
    }
    let (path, table) = rkb_core::config::machine(&rkb_core::paths::config_dir())?;
    let not_configured = || format!("not configured: set RKB_JEV_API_KEY or [jev] api_key in {}", path.display());
    let Some(jev) = table.get("jev").and_then(|v| v.as_table()) else { return Err(not_configured()) };
    if let Some(k) = jev.get("api_key").and_then(|v| v.as_str()) {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).map(|m| m.permissions().mode()).unwrap_or(0o777);
        if mode & 0o077 != 0 {
            return Err(format!(
                "{} can be read by others, so its api_key is not used; run `chmod 600 {}`",
                path.display(),
                path.display()
            ));
        }
        return Ok((k.trim().to_string(), "config.toml"));
    }
    if let Some(cmd) = jev.get("api_key_cmd").and_then(|v| v.as_str()) {
        return key_from_command(cmd).map(|k| (k, "api_key_cmd"));
    }
    Err(not_configured())
}

/// The first line of `cmd`'s output, when it exits 0 within 10 s. Its stderr is dropped, so a
/// secret tool's message never reaches rkb's output.
fn key_from_command(cmd: &str) -> Result<String, String> {
    use std::io::Read;
    let mut child = std::process::Command::new("sh")
        .args(["-c", cmd])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("[jev] api_key_cmd did not start: {e}"))?;
    let mut out = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() < Duration::from_secs(10) => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("[jev] api_key_cmd did not finish within 10 s".into());
            }
        }
    };
    let text = reader.join().unwrap_or_default();
    let key = text.lines().next().unwrap_or("").trim().to_string();
    if !status.success() || key.is_empty() {
        return Err(format!("[jev] api_key_cmd exited with {} and printed no key", status.code().unwrap_or(-1)));
    }
    Ok(key)
}

impl JevBackend {
    /// One request; the `noul` of each question key, in `keys` order.
    fn ask(&self, state: String, keys: &[String], instructions: &[String]) -> Result<Vec<f32>, String> {
        let questions: serde_json::Map<String, serde_json::Value> =
            keys.iter().zip(instructions).map(|(k, i)| (k.clone(), serde_json::json!({ "type": "noul", "instructions": i }))).collect();
        let body = serde_json::json!({ "model": self.model, "state": state, "questions": questions }).to_string();
        // The chain's own time limit ends the search sooner; this only stops a stuck socket.
        let agent: ureq::Agent =
            ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(60))).build().into();
        let mut resp = agent
            .post(&self.url)
            .header("Authorization", &format!("Bearer {}", self.key))
            .header("Content-Type", "application/json")
            .send(body.as_str())
            .map_err(|e| format!("request failed: {e}"))?;
        let status = resp.status().as_u16();
        if status != 200 {
            return Err(format!("HTTP {status}"));
        }
        let text = resp.body_mut().read_to_string().map_err(|e| format!("reading the answer failed: {e}"))?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|_| "the answer is not JSON".to_string())?;
        keys.iter()
            .map(|k| match v["answers"][k]["noul"].as_f64() {
                Some(p) if (0.0..=1.0).contains(&p) => Ok(p as f32),
                Some(p) => Err(format!("answer {k} is {p}, outside 0 to 1")),
                None => Err(format!("the answer has no noul for {k}")),
            })
            .collect()
    }
}

impl Reranker for JevBackend {
    fn score(&self, query: &str, items: &[String]) -> Result<Vec<f32>, String> {
        if self.batch {
            let mut state = format!("problem: {query}\n\nlessons:");
            for (n, it) in items.iter().enumerate() {
                state.push_str(&format!("\n\n[{n}]\n{it}"));
            }
            let keys: Vec<String> = (0..items.len()).map(|n| format!("lesson_{n}")).collect();
            let asks: Vec<String> =
                (0..items.len()).map(|n| format!("Is lesson [{n}] about the same problem as the user describes?")).collect();
            return self.ask(state, &keys, &asks);
        }
        let (key, ask) = (&["relevant".to_string()], &[JEV_QUESTION.to_string()]);
        std::thread::scope(|s| {
            let handles: Vec<_> =
                items.iter().map(|it| s.spawn(move || self.ask(format!("problem: {query}\nlesson:\n{it}"), key, ask))).collect();
            handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("a request thread panicked".into())).map(|v| v[0])).collect()
        })
    }
}

const JEV: &str = "jev";

/// Which candidates Jev may see: those `[sinks.jev]` allows and the leak scan passes. An error names
/// why Jev may see nothing: no `[sinks.jev]`, the leak scan matches the query (which holds a hook's
/// error lines), or no candidate is allowed.
fn jev_allowed(root: &Path, query: &str, hits: &[Hit], items: &[String]) -> Result<Vec<bool>, String> {
    let kb: KbConfig = std::fs::read_to_string(root.join("kb.toml")).ok().and_then(|t| config::parse(&t).ok()).unwrap_or_default();
    let Some(sink) = kb.sinks.get(JEV) else { return Err("kb.toml has no [sinks.jev]".into()) };
    let Ok(snap) = Snapshot::from_dir(root) else { return Err("the lessons' labels could not be read".into()) };
    let env = rkb_core::lint::LintEnv::from_process();
    let scanner = LeakScanner::new(&kb.leak, env.user.as_deref(), env.home.as_deref());
    if !scanner.scan(query).is_empty() {
        return Err("the leak scan matched the query".into());
    }
    let allowed: Vec<bool> = hits
        .iter()
        .zip(items)
        .map(|(h, t)| config::sink_allows(&sink.allow, &rkb_core::write::effective_at(&snap, &h.path)) && scanner.scan(t).is_empty())
        .collect();
    if !allowed.contains(&true) && !hits.is_empty() {
        return Err("no candidate is allowed by sinks.jev or passes the leak scan".into());
    }
    Ok(allowed)
}

/// The rerank chain with this build's backends. The items are built from `hits` here, so the Jev guard sees the lessons they come from.
pub fn run(
    root: &Path,
    settings: &Settings,
    only: Option<&str>,
    query: &str,
    hits: &[Hit],
    timeout: Duration,
) -> rkb_core::error::Result<Ranked> {
    let items = search::rerank_items(root, hits);
    let wants_jev = only.map_or_else(|| settings.chain.iter().any(|b| b == JEV), |o| o == JEV);
    let mut blocked = BTreeMap::new();
    if wants_jev {
        match jev_allowed(root, query, hits, &items) {
            Err(reason) => {
                blocked.insert(JEV.to_string(), reason);
            }
            // Behind another backend Jev must not run first, so it sees all candidates or none.
            Ok(allowed) if allowed.contains(&false) && only != Some(JEV) && settings.chain.first().is_none_or(|b| b != JEV) => {
                blocked.insert(JEV.to_string(), "a candidate is not allowed by sinks.jev".into());
            }
            // Jev rates the lessons it may see; the others keep their BM25 place with no relevance.
            Ok(allowed) if allowed.contains(&false) => {
                let seen: Vec<String> = items.iter().zip(&allowed).filter(|(_, a)| **a).map(|(t, _)| t.clone()).collect();
                let r = rerank::run(settings, Some(JEV), &opener(), query, &seen, timeout, &BTreeMap::new())?;
                if r.backend == JEV {
                    let mut rated = r.scores.clone().unwrap_or_default().into_iter();
                    let scores = allowed.iter().map(|a| if *a { rated.next().flatten() } else { None }).collect();
                    return Ok(Ranked { scores: Some(scores), ..r });
                }
                let reason = r.skipped.into_iter().find(|(b, _)| b == JEV).map_or_else(|| "did not answer".into(), |(_, r)| r);
                blocked.insert(JEV.to_string(), reason);
            }
            Ok(_) => {}
        }
    }
    rerank::run(settings, only, &opener(), query, &items, timeout, &blocked)
}

/// The doctor checks for the `jev` backend, when the chain has it: where the key comes from (never the
/// key), and a warning when `[sinks.jev]` lets more than public lessons out. Never calls the API.
pub fn jev_checks(root: &Path, settings: &Settings) -> Vec<Check> {
    if !settings.chain.iter().any(|b| b == JEV) {
        return vec![];
    }
    let check = |level, detail: String, fix: Option<String>| Check { name: "jev", level, detail, fix };
    let mut out = vec![match jev_key_source() {
        Ok(source) => check(Level::Ok, format!("key from {source}"), None),
        Err(reason) => check(
            Level::Warn,
            reason,
            Some("put `[jev] api_key = \"…\"` in ~/.config/rkb/config.toml with mode 600 (or api_key_cmd), or set RKB_JEV_API_KEY".into()),
        ),
    }];
    let kb: KbConfig = std::fs::read_to_string(root.join("kb.toml")).ok().and_then(|t| config::parse(&t).ok()).unwrap_or_default();
    let wider: Vec<String> = kb
        .sinks
        .get(JEV)
        .and_then(|s| s.allow.get("sensitivity"))
        .into_iter()
        .flatten()
        .filter(|v| v.as_str() != "public")
        .cloned()
        .collect();
    if !wider.is_empty() {
        out.push(check(
            Level::Warn,
            format!("[sinks.jev] lets {} lessons go to Jev, whose data retention is not confirmed", wider.join(", ")),
            Some("set `[sinks.jev] allow = { sensitivity = [\"public\"] }` in kb.toml until Jev's retention is confirmed".into()),
        ));
    }
    out
}
