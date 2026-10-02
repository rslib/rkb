//! The backends the rerank chain can open in this build.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rkb_core::config::{self, KbConfig};
use rkb_core::doctor::{Check, Level};
use rkb_core::dupes::{self, Verdict};
use rkb_core::kb::Snapshot;
use rkb_core::leak::LeakScanner;
use rkb_core::lesson::Lesson;
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
    Ok(Box::new(jev_client()?))
}

fn jev_client() -> Result<JevBackend, String> {
    let key = jev_key()?;
    let root = rkb_core::kb::home();
    let cfg = std::fs::read_to_string(root.join("kb.toml"))
        .ok()
        .and_then(|t| config::parse::<KbConfig>(&t).ok())
        .and_then(|c| c.jev)
        .unwrap_or_default();
    Ok(JevBackend {
        key,
        url: std::env::var("RKB_JEV_URL").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| JEV_URL.into()),
        model: cfg.get("model").and_then(|v| v.as_str()).unwrap_or("jev-latest").to_string(),
        batch: cfg.get("batch").and_then(|v| v.as_bool()).unwrap_or(true),
    })
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
        let v = self.post(&state, questions)?;
        keys.iter()
            .map(|k| match v["answers"][k]["noul"].as_f64() {
                Some(p) if (0.0..=1.0).contains(&p) => Ok(p as f32),
                Some(p) => Err(format!("answer {k} is {p}, outside 0 to 1")),
                None => Err(format!("the answer has no noul for {k}")),
            })
            .collect()
    }

    /// One request with any questions; the answer as JSON.
    fn post(&self, state: &str, questions: serde_json::Map<String, serde_json::Value>) -> Result<serde_json::Value, String> {
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
        serde_json::from_str(&text).map_err(|_| "the answer is not JSON".to_string())
    }
}

/// The lesson types the triage choice offers, with what each means; `not_a_lesson` is the rest.
const KINDS: [(&str, &str); 6] = [
    ("pitfall", "an error or trap and the fix that worked"),
    ("recipe", "steps that get something done"),
    ("fact", "something verified about a tool, an API or an environment"),
    ("decision", "a design choice with its reason"),
    ("preference", "how the user wants work done"),
    ("not_a_lesson", "chatter, task status, a plan, or something that will change within weeks"),
];

/// Why an inbox item of `project` (or `general`) may not go to Jev: no `[sinks.jev]`, or labels it does
/// not allow. Every path that sends inbox text to Jev, the novelty search included, asks this first.
pub fn item_jev_guard(root: &Path, project: Option<&str>) -> Result<(), String> {
    let kb: KbConfig = std::fs::read_to_string(root.join("kb.toml")).ok().and_then(|t| config::parse(&t).ok()).unwrap_or_default();
    let Some(sink) = kb.sinks.get(JEV) else { return Err("kb.toml has no [sinks.jev]".into()) };
    let Ok(snap) = Snapshot::from_dir(root) else { return Err("the labels could not be read".into()) };
    // The item's labels are those a lesson of its place would get.
    let folder = project.map_or_else(|| "general/inbox.md".to_string(), |p| format!("projects/{p}/inbox.md"));
    let labels = rkb_core::write::effective_at(&snap, &folder);
    if config::sink_allows(&sink.allow, &labels) {
        return Ok(());
    }
    let shown: Vec<String> = labels.values().cloned().collect();
    Err(format!("label {}", shown.join(", ")))
}

/// Asks Jev, in one request, the worth questions about the candidates of one inbox item. The first
/// element is the backend's name; a candidate the guard keeps from Jev gets the reason instead.
pub fn jev_worth(root: &Path, project: Option<&str>, texts: &[&str]) -> Option<(String, Vec<Result<rkb_core::triage::Worth, String>>)> {
    if !Settings::load(root).chain.iter().any(|b| b == JEV) {
        return None;
    }
    let every = |why: String| Some((JEV.to_string(), texts.iter().map(|_| Err(why.clone())).collect()));
    let client = match jev_client() {
        Ok(c) => c,
        Err(why) => return every(why),
    };
    if let Err(why) = item_jev_guard(root, project) {
        return every(why);
    }
    let kb: KbConfig = std::fs::read_to_string(root.join("kb.toml")).ok().and_then(|t| config::parse(&t).ok()).unwrap_or_default();
    let env = rkb_core::lint::LintEnv::from_process();
    let scanner = LeakScanner::new(&kb.leak, env.user.as_deref(), env.home.as_deref());
    let sent: Vec<usize> = (0..texts.len()).filter(|&i| scanner.scan(texts[i]).is_empty()).collect();
    let mut state = String::from("Notes from coding sessions, each a possible entry in a knowledge base of lessons learned:");
    let mut questions = serde_json::Map::new();
    let criteria: serde_json::Map<String, serde_json::Value> = KINDS.iter().map(|(k, m)| (k.to_string(), serde_json::json!(m))).collect();
    for (n, &i) in sent.iter().enumerate() {
        state.push_str(&format!("\n\n[{n}]\n{}", texts[i].chars().take(4000).collect::<String>()));
        for (key, ask) in [
            ("durable", "Would note [N] still help in a different session months from now?"),
            ("general", "Does note [N] hold beyond the one file or task it came from?"),
            ("mistake", "Does note [N] record a mistake and the fix that worked?"),
            ("preference", "Does note [N] state how the user wants work done?"),
        ] {
            questions.insert(format!("{key}_{n}"), serde_json::json!({ "type": "noul", "instructions": ask.replace('N', &n.to_string()) }));
        }
        questions.insert(
            format!("kind_{n}"),
            serde_json::json!({ "type": "choice", "instructions": format!("What kind of lesson is note [{n}]?"), "criteria": criteria }),
        );
    }
    let answer = if sent.is_empty() { Ok(serde_json::Value::Null) } else { client.post(&state, questions) };
    let worth = |n: usize, v: &serde_json::Value| -> Result<rkb_core::triage::Worth, String> {
        let noul = |key: &str| {
            v["answers"][format!("{key}_{n}")]["noul"].as_f64().filter(|p| (0.0..=1.0).contains(p)).ok_or(format!("no {key} answer"))
        };
        let probs = &v["answers"][format!("kind_{n}")]["probabilities"];
        let kind = v["answers"][format!("kind_{n}")]["choice"]
            .as_str()
            .map(|c| (c.to_string(), probs["not_a_lesson"].as_f64().unwrap_or(if c == "not_a_lesson" { 1.0 } else { 0.0 })));
        Ok(rkb_core::triage::Worth {
            durable: noul("durable")?,
            general: noul("general")?,
            mistake: noul("mistake")?,
            preference: noul("preference")?,
            kind,
        })
    };
    let answers = (0..texts.len())
        .map(|i| match (&answer, sent.iter().position(|&s| s == i)) {
            (_, None) => Err("leak scan".to_string()),
            (Err(why), _) => Err(why.clone()),
            (Ok(v), Some(n)) => worth(n, v),
        })
        .collect();
    Some((JEV.to_string(), answers))
}

const RELATIONS: [(&str, &str); 4] = [
    ("same", "One lesson can replace the other; they teach the same thing"),
    ("extends", "One adds steps or cases to the other on the same topic"),
    ("conflicts", "They disagree about what to do"),
    ("different", "They teach different things even if they share words"),
];

/// Asks Jev how each pair of lessons relates, 10 pairs to a request. `None` when Jev is not in the
/// chain; a pair either lesson of which the guard keeps from Jev gets the reason instead.
pub fn jev_dupes(root: &Path, pairs: &[(&Lesson, &Lesson)]) -> Option<Vec<Result<Verdict, String>>> {
    if !Settings::load(root).chain.iter().any(|b| b == JEV) {
        return None;
    }
    let every = |why: String| Some(pairs.iter().map(|_| Err(why.clone())).collect());
    let client = match jev_client() {
        Ok(c) => c,
        Err(why) => return every(why),
    };
    let kb: KbConfig = std::fs::read_to_string(root.join("kb.toml")).ok().and_then(|t| config::parse(&t).ok()).unwrap_or_default();
    let Some(sink) = kb.sinks.get(JEV) else { return every("kb.toml has no [sinks.jev]".into()) };
    let Ok(snap) = Snapshot::from_dir(root) else { return every("the labels could not be read".into()) };
    let env = rkb_core::lint::LintEnv::from_process();
    let scanner = LeakScanner::new(&kb.leak, env.user.as_deref(), env.home.as_deref());
    let guard = |l: &Lesson| {
        let labels = rkb_core::write::effective_at(&snap, &l.path);
        if !config::sink_allows(&sink.allow, &labels) {
            return Err(format!("label {}", labels.values().cloned().collect::<Vec<_>>().join(", ")));
        }
        if !scanner.scan(&dupes::text(l)).is_empty() {
            return Err("leak scan".to_string());
        }
        Ok(())
    };
    let mut out: Vec<Result<Verdict, String>> = pairs.iter().map(|&(a, b)| guard(a).and(guard(b)).map(|_| Verdict::default())).collect();
    let sent: Vec<usize> = (0..pairs.len()).filter(|&i| out[i].is_ok()).collect();
    let criteria: serde_json::Map<String, serde_json::Value> =
        RELATIONS.iter().map(|(k, m)| (k.to_string(), serde_json::json!(m))).collect();
    for batch in sent.chunks(10) {
        let mut state = String::from("Pairs of lessons from a knowledge base:");
        let mut questions = serde_json::Map::new();
        for (n, &i) in batch.iter().enumerate() {
            let cut = |l: &Lesson| dupes::text(l).chars().take(2000).collect::<String>();
            state.push_str(&format!("\n\n[{n}]\nA: {}\nB: {}", cut(pairs[i].0), cut(pairs[i].1)));
            questions.insert(
                format!("pair_{n}"),
                serde_json::json!({ "type": "choice", "instructions": format!("How do lesson A and lesson B of pair [{n}] relate?"), "criteria": criteria }),
            );
        }
        let answer = client.post(&state, questions);
        for (n, &i) in batch.iter().enumerate() {
            out[i] = answer.as_ref().map_err(|e| e.clone()).and_then(|v| {
                let a = &v["answers"][format!("pair_{n}")];
                let choice = a["choice"].as_str().ok_or("no answer")?;
                let prob = |k: &str| a["probabilities"][k].as_f64().unwrap_or(if k == choice { 1.0 } else { 0.0 });
                Ok(Verdict {
                    hash: dupes::hash(pairs[i].0, pairs[i].1),
                    verdict: choice.to_string(),
                    p: prob(choice),
                    different: prob("different"),
                })
            });
        }
    }
    Some(out)
}

impl Reranker for JevBackend {
    fn score(&self, query: &str, items: &[String], _: &[Vec<String>]) -> Result<Vec<f32>, String> {
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
    let queries = search::rerank_queries(root, hits);
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
                let r = rerank::run(settings, Some(JEV), &opener(), query, &seen, &[], timeout, &BTreeMap::new())?;
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
    rerank::run(settings, only, &opener(), query, &items, &queries, timeout, &blocked)
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
