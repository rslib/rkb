use std::io::{IsTerminal, Read};
use std::path::Path;

use rkb_core::distill::{self, Item, Kind, Meta};
use rkb_core::request;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::list::{cut, marks, width};
use crate::output::{CliError, ErrorCode, Output, paint};
use crate::writes::Env;

pub fn age(secs: u64) -> String {
    match secs {
        s if s < 3600 => format!("{} min", s / 60),
        s if s < 2 * 86400 => format!("{} h", s / 3600),
        s => format!("{} d", s / 86400),
    }
}

fn row(i: &Item, now: u64) -> Value {
    let mut r = json!({
        "id": i.id,
        "kind": i.meta.kind.as_str(),
        "priority": i.meta.priority(),
        "age": age(now.saturating_sub(i.meta.time)),
        "lines": i.body.lines().count(),
        "preview": i.preview(),
    });
    if !i.meta.candidates.is_empty() {
        r["verdicts"] = counts_json(rkb_core::triage::Counts::of(&i.meta.candidates));
    }
    r
}

/// `2 keep, 1 drop`: the verdicts an item has, or nothing before triage.
fn verdicts(i: &Item) -> String {
    let c = rkb_core::triage::Counts::of(&i.meta.candidates);
    [(c.keep, "keep"), (c.known, "known"), (c.unsure, "unsure"), (c.drop, "drop")]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, w)| format!("{n} {w}"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn note(env: &Env, words: Vec<String>, priority: Option<u8>) -> Result<Output, CliError> {
    let mut text = words.join(" ");
    if text.trim().is_empty() && !std::io::stdin().is_terminal() {
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| CliError::new(ErrorCode::Usage, format!("cannot read stdin: {e}"), "pass the note as an argument"))?;
    }
    if text.trim().is_empty() {
        return Err(CliError::new(ErrorCode::Usage, "the note is empty", "run `rkb note \"<what you found>\"`"));
    }
    let meta = Meta {
        kind: Kind::Note,
        time: request::now(),
        harness: None,
        session: rkb_core::usage::env_session(),
        cwd: None,
        signals: vec![],
        signatures: vec![],
        priority: priority.map(|p| p.clamp(1, 3)),
        source: None,
        candidates: vec![],
    };
    // Read before the add, so a bad [proposals] ntfy refuses the note instead of failing after it.
    let topic = ntfy_topic()?;
    let item = distill::add(&env.state, meta, &text)?;
    let mut data = json!({ "id": item.id, "help": ["Run `rkb inbox` to list the inbox"] });
    let mut human =
        format!("noted {} in the inbox; `rkb promote` turns an item into a lesson once its signature repeats in 2 sessions", item.id);
    if let Some(url) = topic {
        match ntfy_post(env, &url, &item.id, &item.body) {
            NtfyOutcome::Posted => {}
            NtfyOutcome::Skipped(why) => {
                data["posted"] = why.clone().into();
                human.push_str(&format!("\n{why}"));
            }
            NtfyOutcome::Failed(why) => {
                let line = format!("the ntfy post failed: {why}");
                data["posted"] = line.clone().into();
                human.push_str(&format!("\n{line}"));
            }
        }
    }
    Ok(Output { data, human, exit: 0, raw: false })
}

/// `rkb import --claude-memory [<dir>] [--file <f>]`: Claude Code memory as inbox notes, never lessons.
/// `memory` is `None` without `--claude-memory`, `Some("")` for every memory folder, or one folder.
pub fn import_claude(env: &Env, memory: Option<&str>, files: &[std::path::PathBuf]) -> Result<Output, CliError> {
    let dirs = match memory {
        None => vec![],
        Some("") => distill::claude_memory_dirs(),
        Some(d) => vec![std::path::PathBuf::from(d)],
    };
    let notes = distill::claude_notes(&dirs, files);
    let (saved, skipped) = distill::import_notes(&env.state, &notes)?;
    let human = format!("saved {saved} notes to the inbox; skipped {skipped} imported before. Distill turns them into lessons.");
    Ok(Output {
        data: json!({ "saved": saved, "skipped": skipped, "help": ["Write lessons from the notes with `rkb add`; `rkb promote` turns repeated items into lessons"] }),
        human,
        exit: 0,
        raw: false,
    })
}

pub fn list(env: &Env) -> Result<Output, CliError> {
    let now = request::now();
    let expired = distill::enforce_cap(&env.state, &rkb_core::paths::config_dir());
    let items = distill::list(&env.state);
    let (c, m, w) = (env.colored, marks(), width());
    let mut human = format!("{} {} {} items\n", paint(c, "1", "inbox"), m.sep, items.len());
    for i in &items {
        let mut head =
            format!("{} {} {} {} lines", i.meta.kind.as_str(), m.sep, age(now.saturating_sub(i.meta.time)), i.body.lines().count());
        if !i.meta.candidates.is_empty() {
            head.push_str(&format!(" {} {}", m.sep, verdicts(i)));
        }
        human.push_str(&format!(
            "\n  {} {}\n    {}\n",
            paint(c, "36", &i.id),
            paint(c, "2", &head),
            cut(&i.preview(), w.saturating_sub(4), m)
        ));
    }
    if expired > 0 {
        human.push_str(&format!("\n{expired} oldest items dropped over the inbox cap\n"));
    }
    let help = if items.is_empty() {
        "The inbox is empty"
    } else {
        "Run `rkb promote`, or `rkb inbox show <id>`, write lessons with `rkb add`, then `rkb inbox done <id>`"
    };
    let rows: Vec<Value> = items.iter().map(|i| row(i, now)).collect();
    Ok(Output { data: json!({ "items": rows, "expired": expired, "help": [help] }), human, exit: 0, raw: false })
}

fn counts_json(c: rkb_core::triage::Counts) -> Value {
    json!({ "keep": c.keep, "known": c.known, "unsure": c.unsure, "drop": c.drop })
}

/// The best lesson for `text` at the item's place, rated by the rerank chain; `None` when BM25 ranked.
fn novelty(env: &Env, item: &Item, text: &str) -> Option<(String, f32)> {
    use rkb_core::search::{self, Mode, Options};
    let here = item.meta.cwd.as_deref().map(std::path::PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let place = rkb_core::state::locate(&env.root, &here, &rkb_core::matching::Hints::default(), &env.state).ok()?;
    let facts = rkb_core::conditions::Facts::gather(&env.root, &place, &[]);
    let mut settings = rkb_core::rerank::Settings::load(&env.root);
    // The search sends the note as its query, so a note Jev may not see searches without Jev.
    let project = place.project.as_ref().map(|m| m.name.clone());
    if crate::rerankers::item_jev_guard(&env.root, project.as_deref()).is_err() {
        settings.chain.retain(|b| b != "jev");
    }
    // `every_status` so a tombstone matches too: the same claim is not added again.
    let opts = Options { all: false, every_status: true, limit: settings.top.max(1), probes: search::ProbeMode::Cached, tier: None };
    let query: String = text.chars().take(1000).collect();
    let mut found = search::search(&env.root, &place, &facts, &Mode::Ranked(query.clone()), &opts).ok()?;
    let timeout = std::time::Duration::from_millis(settings.timeout_ms);
    let ranked = crate::rerankers::run(&env.root, &settings, None, &query, &found.hits, timeout).ok()?;
    search::apply_relevance(&mut found.hits, ranked.scores.as_ref()?, ranked.order.as_deref());
    found.hits.iter().filter_map(|h| Some((h.id.clone(), h.relevance?))).max_by(|a, b| a.1.total_cmp(&b.1))
}

/// `rkb inbox triage`: candidates and verdicts for the items that have none yet; with `ask`, then the
/// unsure ones through `harness`'s CLI and its approved triage models.
pub fn triage(env: &Env, ask: Option<&str>) -> Result<Output, CliError> {
    use rkb_core::triage::{self, Counts, Stages, Thresholds};
    distill::enforce_cap(&env.state, &rkb_core::paths::config_dir());
    let has_kb = rkb_core::kb::open(&env.root).is_ok();
    let novel = |item: &Item, text: &str| novelty(env, item, text);
    let score = |item: &Item, texts: &[&str]| {
        let here = item.meta.cwd.as_deref().map(std::path::PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let place = rkb_core::state::locate(&env.root, &here, &rkb_core::matching::Hints::default(), &env.state).ok();
        let project = place.as_ref().and_then(|p| p.project.as_ref()).map(|m| m.name.clone());
        crate::rerankers::jev_worth(&env.root, project.as_deref(), texts)
    };
    let stages = Stages {
        thresholds: Thresholds::load(&rkb_core::paths::config_dir()),
        novelty: has_kb.then_some(&novel as triage::Novelty),
        scorer: has_kb.then_some(&score as triage::Scorer),
    };
    let done = triage::triage(&env.state, &stages)?;
    let mut total = Counts::default();
    let rows: Vec<Value> = done
        .iter()
        .map(|(item, c)| {
            total.add(*c);
            json!({ "id": item.id, "kind": item.meta.kind.as_str(), "counts": counts_json(*c) })
        })
        .collect();
    let human = if done.is_empty() {
        "every inbox item is triaged already".to_string()
    } else {
        format!("triaged {} items: {} keep, {} known, {} unsure, {} drop", done.len(), total.keep, total.known, total.unsure, total.drop)
    };
    let help = "Run `rkb promote` for repeated items, or `rkb inbox show <id>` for the verdicts of one item";
    let mut data = json!({ "items": rows, "total": counts_json(total), "help": [help] });
    let mut human = human;
    if let Some(harness) = ask
        && let Some(asked) = ask_with_cli(env, harness)?
    {
        human.push_str(&format!("\n{}", asked.human));
        data["asked"] = asked.data;
    }
    Ok(Output { data, human, exit: 0, raw: false })
}

pub fn show(env: &Env, id: &str) -> Result<Output, CliError> {
    let item = distill::get(&env.state, id)?;
    let mut data = row(&item, request::now());
    data["meta"] = serde_json::to_value(&item.meta).unwrap_or(Value::Null);
    data["body"] = json!(item.body);
    let mut human = item.text();
    if !item.meta.candidates.is_empty() {
        human.push_str("\nverdicts:\n");
        for c in &item.meta.candidates {
            let what = rkb_core::triage::text(&item, c).chars().take(70).collect::<String>();
            let score = c.score.map(|s| format!(" {s:.2}")).unwrap_or_default();
            human.push_str(&format!("  {} {} ({}{score}) {what}\n", c.index, c.verdict.as_str(), c.reason));
        }
    }
    Ok(Output { data, human: human.trim_end().to_string(), exit: 0, raw: false })
}

pub fn done(env: &Env, ids: &[String], outcomes: &[String]) -> Result<Output, CliError> {
    if !outcomes.is_empty() {
        let [id] = ids else {
            return Err(CliError::new(
                ErrorCode::Usage,
                "--outcome takes one item id",
                "run `rkb inbox done <id> --outcome ...` once per item",
            ));
        };
        let parsed: Vec<(usize, String)> = outcomes
            .iter()
            .map(|o| {
                let (n, op) = o.split_once('=').ok_or(())?;
                Ok((n.trim().parse::<usize>().map_err(|_| ())?, op.trim().to_string()))
            })
            .collect::<Result<_, ()>>()
            .map_err(|()| {
                CliError::new(ErrorCode::Usage, "an outcome is `<index>=<op>[:<lesson id>]`", "for example `--outcome 0=add:c04e11a9f3`")
            })?;
        let removed = rkb_core::triage::record_outcomes(&env.state, id, &parsed)
            .map_err(|m| CliError::new(ErrorCode::Usage, m, "run `rkb inbox show <id>` for the candidates"))?;
        let human = if removed { format!("{id}: done") } else { format!("{id}: recorded; candidates without an outcome remain") };
        return Ok(Output { data: json!({ "items": [{ "id": id, "removed": removed }] }), human, exit: 0, raw: false });
    }
    let mut human = String::new();
    let rows: Vec<Value> = ids
        .iter()
        .map(|id| {
            let removed = distill::remove(&env.state, id);
            human.push_str(&format!("{id}: {}\n", if removed { "done" } else { "no such inbox item" }));
            json!({ "id": id, "removed": removed })
        })
        .collect();
    let exit = u8::from(rows.iter().any(|r| r["removed"] == false));
    Ok(Output { data: json!({ "items": rows }), human: human.trim_end().to_string(), exit, raw: false })
}

/// The observer settings for `harness` when its chain is set and approved, and the lesson titles of the
/// session's place.
fn read_answer() -> Result<String, CliError> {
    let mut out = String::new();
    std::io::stdin()
        .take(1 << 20)
        .read_to_string(&mut out)
        .map_err(|e| CliError::new(ErrorCode::Usage, format!("cannot read stdin: {e}"), "pipe the model's answer on stdin"))?;
    Ok(out)
}

fn triaged(c: rkb_core::triage::Counts, via: &str) -> Output {
    Output {
        data: json!({ "outcome": "triaged", "counts": counts_json(c), "via": via }),
        human: format!("{via} decided: {} keep, {} drop, {} still unsure", c.keep, c.drop, c.unsure),
        exit: 0,
        raw: false,
    }
}

fn no_job(reason: &str) -> Output {
    Output { data: json!({ "job": null, "skipped": reason }), human: format!("no job: {reason}"), exit: 0, raw: false }
}

fn finish_triage(
    env: &Env,
    id: &str,
    part: usize,
    model: Option<&str>,
    failed: Option<&str>,
    ask: &rkb_core::triage::Ask,
) -> Result<Output, CliError> {
    use rkb_core::job;
    if part != 0 {
        return Err(CliError::new(ErrorCode::Usage, format!("job {id} has one part, 0"), "pass --part 0"));
    }
    job::remove(&env.state, id);
    if let Some(reason) = failed {
        return Err(CliError::new(
            ErrorCode::Refused,
            format!("every triage model failed: {reason}"),
            "the candidates stay unsure; see `rkb doctor`",
        ));
    }
    let entries: Vec<&str> = ask.models.iter().map(|m| m.0.as_str()).collect();
    let entry = model.filter(|m| entries.contains(m)).ok_or_else(|| {
        CliError::new(ErrorCode::Usage, "--model must name the chain entry that answered", format!("pass one of: {}", entries.join(", ")))
    })?;
    let counts = rkb_core::triage::finish_ask(&env.state, ask, entry, &read_answer()?)?;
    Ok(triaged(counts, entry))
}

/// `rkb job prepare triage`: the prompt for the unsure candidates, or why there is none.
pub fn prepare_triage(env: &Env, harness: &str, model: Option<&str>) -> Result<Output, CliError> {
    use rkb_core::{job, observer, triage};
    let config = rkb_core::paths::config_dir();
    let cfg = match observer::load(&config) {
        Ok(c) => c,
        Err(e) => return Ok(no_job(&e)),
    };
    if !observer::approved(&config, &cfg) {
        return Ok(no_job("the observer and triage models are not approved here; run `rkb approve observer`"));
    }
    let Some(ask) = triage::prepare_ask(&env.state, &cfg, harness, model) else {
        return Ok(no_job(&format!("no unsure candidate, or no [triage] chain for {harness}")));
    };
    let stored = job::Stored { id: job::new_id(), created: request::now(), job: job::Job::Triage(ask.clone()) };
    job::save(&env.state, &stored)?;
    let models: Vec<Value> = ask.models.iter().map(|(entry, model)| json!({ "entry": entry, "model": model })).collect();
    let human = format!("job {}: {} unsure candidates; ask the models in order: {}", stored.id, ask.entries.len(), entries_of(&ask));
    let data = json!({
        "job": stored.id,
        "kind": "triage",
        "models": models,
        "part": 0,
        "parts": 1,
        "prompt": ask.prompt,
        "help": [format!("Send the first answer to `rkb job finish {} --part 0 --model <entry>` on stdin", stored.id)],
    });
    Ok(Output { data, human, exit: 0, raw: false })
}

fn entries_of(ask: &rkb_core::triage::Ask) -> String {
    ask.models.iter().map(|m| m.0.as_str()).collect::<Vec<_>>().join(", ")
}

/// `rkb inbox triage --ask`: the unsure candidates through the harness's own CLI, as the observer runs.
fn ask_with_cli(env: &Env, harness: &str) -> Result<Option<Output>, CliError> {
    use rkb_core::{observer, triage};
    let config = rkb_core::paths::config_dir();
    let Ok(cfg) = observer::load(&config) else { return Ok(None) };
    if !observer::approved(&config, &cfg) {
        return Ok(None);
    }
    let Some(ask) = triage::prepare_ask(&env.state, &cfg, harness, None) else { return Ok(None) };
    let timeout = std::time::Duration::from_secs(cfg.timeout_secs);
    let mut failures = vec![];
    for (entry, model) in &ask.models {
        let (program, args) = observer::command(harness, model.as_deref());
        match observer::run(&program, &args, &ask.prompt, timeout, &env.state.join("observer"))? {
            observer::Ran::Ok { text, .. } => return Ok(Some(triaged(triage::finish_ask(&env.state, &ask, entry, &text)?, entry))),
            observer::Ran::Failed(r) => failures.push(format!("{entry}: {r}")),
            observer::Ran::Timeout => failures.push(format!("{entry}: timeout")),
        }
    }
    Err(CliError::new(
        ErrorCode::Refused,
        format!("every triage model failed: {}", failures.join("; ")),
        "the candidates stay unsure; see `rkb doctor`",
    ))
}

/// `rkb job finish`: the answer to one part of a job, read from stdin, or `failed` when every model failed.
pub fn finish(env: &Env, id: &str, part: usize, model: Option<&str>, failed: Option<&str>) -> Result<Output, CliError> {
    use rkb_core::job::{self, Missing};
    let stored = job::load(&env.state, id).map_err(|m| match m {
        Missing::Expired => CliError::new(ErrorCode::Expired, format!("job {id} is older than one hour"), "prepare the job again"),
        Missing::NotFound => CliError::new(ErrorCode::NotFound, format!("no job {id}"), "prepare the job again"),
    })?;
    match stored.job {
        job::Job::Triage(ask) => finish_triage(env, id, part, model, failed, &ask),
    }
}

/// The `[proposals] ntfy` topic URL from the machine `config.toml`, or `None` when unset. A bad value
/// is an error with a fix.
fn ntfy_topic() -> Result<Option<String>, CliError> {
    rkb_core::config::proposals_ntfy(&rkb_core::paths::config_dir())
        .map_err(|m| CliError::new(ErrorCode::Refused, m, "fix [proposals] ntfy in the machine config.toml"))
}

/// What a note's ntfy post decided, so `note` can say the right thing: a leak-skipped note stays local
/// and is not a transport failure.
enum NtfyOutcome {
    Posted,
    /// The note was not posted on purpose (the leak scan flagged it); the string says why.
    Skipped(String),
    /// The post failed; the string names the reason. The note stays in the inbox.
    Failed(String),
}

/// Post a new note for inbox item `id` to the ntfy topic `url`. The body is at most 4096 bytes (the
/// text cut on a char boundary with `"cut": true`), a note the KB leak scan flags is never posted, the
/// `RKB_NTFY_TOKEN` (when set) goes as `Authorization: Bearer` and is never written down, and a failed
/// post still leaves the note in the local inbox.
fn ntfy_post(env: &Env, url: &str, id: &str, text: &str) -> NtfyOutcome {
    let kb_text = std::fs::read_to_string(env.root.join("kb.toml")).unwrap_or_default();
    let kb = rkb_core::config::parse::<rkb_core::config::KbConfig>(&kb_text).unwrap_or_default();
    let lint = rkb_core::lint::LintEnv::from_process();
    let scanner = rkb_core::leak::LeakScanner::new(&kb.leak, lint.user.as_deref(), lint.home.as_deref());
    let flags = scanner.scan(text);
    if !flags.is_empty() {
        let mut kinds: Vec<&str> = vec![];
        for f in &flags {
            if !kinds.contains(&f.kind.rule()) {
                kinds.push(f.kind.rule());
            }
        }
        return NtfyOutcome::Skipped(format!("the note was not posted to ntfy: it contains a {} leak", kinds.join(" and ")));
    }
    let agent = rkb_core::usage::agent().unwrap_or_else(|| "unknown".to_string());
    let time = request::now();
    let mut text = text.to_string();
    let mut body = post_body(&agent, id, time, &text, false);
    if body.len() > 4096 {
        loop {
            let n = text.chars().count();
            if n == 0 {
                break;
            }
            text = text.chars().take(n - (n / 4).max(1)).collect();
            body = post_body(&agent, id, time, &text, true);
            if body.len() <= 4096 {
                break;
            }
        }
    }
    let token = ntfy_token();
    let client: ureq::Agent =
        ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(std::time::Duration::from_secs(3))).build().into();
    let mut req = client.post(url).header("Content-Type", "application/json");
    if let Some(t) = &token {
        req = req.header("Authorization", &format!("Bearer {t}"));
    }
    let mut resp = match req.send(body.as_str()) {
        Ok(r) => r,
        Err(e) => return NtfyOutcome::Failed(format!("{e}")),
    };
    let status = resp.status().as_u16();
    if status != 200 {
        return NtfyOutcome::Failed(format!("HTTP {status}"));
    }
    if let Err(e) = resp.body_mut().read_to_string() {
        return NtfyOutcome::Failed(format!("reading ntfy's reply failed: {e}"));
    }
    NtfyOutcome::Posted
}

/// `RKB_NTFY_TOKEN`, unless it is empty or still the `${RKB_NTFY_TOKEN}` reference a launcher such
/// as Hermes leaves in place when its own secrets lack the token.
fn ntfy_token() -> Option<String> {
    std::env::var("RKB_NTFY_TOKEN").ok().filter(|v| !v.is_empty() && !v.starts_with("${"))
}

/// The JSON post body for a note; `cut` says the text was shortened to fit 4096 bytes.
fn post_body(agent: &str, id: &str, time: u64, text: &str, cut: bool) -> String {
    let mut m = serde_json::Map::new();
    m.insert("v".into(), 1.into());
    m.insert("agent".into(), agent.into());
    m.insert("id".into(), id.into());
    m.insert("time".into(), time.into());
    m.insert("text".into(), text.into());
    if cut {
        m.insert("cut".into(), true.into());
    }
    serde_json::Value::Object(m).to_string()
}

/// `rkb inbox pull <url>`: read the messages ntfy cached since the last pull (`<url>/json?poll=1&since=`)
/// and add each as one inbox item sourced `ntfy:<agent>`, once. The last message id and the ids already
/// seen live per topic in the state dir (keyed by a hash of the url, never the token), so a re-pull or a
/// since reset cannot add twice. Messages are untrusted: anything that is not the JSON body above is
/// skipped and counted, the text is capped at 4096 bytes with control characters (except newlines)
/// stripped, and an agent name must match `[a-z0-9_-]{1,32}` or it becomes `unknown`. Nothing pulled is
/// promoted, approved or run automatically.
pub fn pull(env: &Env, url: &str) -> Result<Output, CliError> {
    if !rkb_core::config::valid_ntfy_url(url) {
        return Err(CliError::new(
            ErrorCode::Usage,
            format!("`{url}` is not an http(s) topic URL"),
            "pass the same topic URL that [proposals] ntfy sets, such as https://ntfy.sh/mytopic",
        ));
    }
    let (key, mut cursor) = ntfy_cursor(&env.state, url);
    let since = if cursor.last_id.is_empty() { "all" } else { cursor.last_id.as_str() };
    let req_url = format!("{}/json?poll=1&since={since}", url.trim_end_matches('/'));
    let client: ureq::Agent =
        ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(std::time::Duration::from_secs(10))).build().into();
    let mut req = client.get(&req_url);
    if let Some(t) = ntfy_token() {
        req = req.header("Authorization", &format!("Bearer {t}"));
    }
    let mut resp = req.call().map_err(|e| {
        CliError::new(ErrorCode::Io, format!("ntfy pull failed: {e}"), "is the topic reachable? run `rkb inbox pull <url>` when it is")
    })?;
    let status = resp.status().as_u16();
    if status != 200 {
        return Err(CliError::new(ErrorCode::Io, format!("ntfy pull failed: HTTP {status}"), "check the topic URL and RKB_NTFY_TOKEN"));
    }
    let text = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| CliError::new(ErrorCode::Io, format!("ntfy pull failed: {e}"), "is the reply readable?"))?;
    let mut added = 0usize;
    let mut skipped = 0usize;
    // ntfy answers one JSON event per line, oldest first; only `message` events carry a post.
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(ev) = serde_json::from_str::<Value>(line) else {
            skipped += 1;
            continue;
        };
        if ev["event"].as_str().is_some_and(|e| e != "message") {
            continue;
        }
        // The id goes back into the next request's query, so it must be a plain token.
        let Some(id) = ev["id"].as_str().filter(|i| (1..=64).contains(&i.len()) && i.bytes().all(|b| b.is_ascii_alphanumeric())) else {
            skipped += 1;
            continue;
        };
        cursor.last_id = id.to_string();
        if cursor.seen.iter().any(|s| s == id) {
            continue;
        }
        cursor.seen.push(id.to_string());
        let Some(parsed) = ev["message"]
            .as_str()
            .and_then(|m| serde_json::from_str::<Value>(m).ok())
            .filter(|p| p["v"].as_u64() == Some(1) && p["text"].is_string())
        else {
            skipped += 1;
            continue;
        };
        let agent = parsed["agent"].as_str().map(sanitize_agent).unwrap_or_else(|| "unknown".to_string());
        let text = cap_text(parsed["text"].as_str().unwrap_or(""));
        if text.trim().is_empty() {
            skipped += 1;
            continue;
        }
        let meta = Meta {
            kind: Kind::Note,
            time: parsed["time"].as_u64().unwrap_or_else(request::now),
            harness: None,
            session: None,
            cwd: None,
            signals: vec![],
            signatures: vec![],
            priority: None,
            source: Some(format!("ntfy:{agent}")),
            candidates: vec![],
        };
        distill::add(&env.state, meta, &text)?;
        added += 1;
    }
    let keep = cursor.seen.len().saturating_sub(SEEN_KEEP);
    cursor.seen.drain(..keep);
    save_cursor(&env.state, &key, &cursor)?;
    ok_pull(url, added, skipped)
}

/// The message ids a cursor remembers. ntfy caches 12 hours of messages by default, far fewer than this.
const SEEN_KEEP: usize = 2000;

fn ok_pull(url: &str, added: usize, skipped: usize) -> Result<Output, CliError> {
    let mut msg = format!("added {added} proposals to the inbox");
    if skipped > 0 {
        msg.push_str(&format!(" ({skipped} messages skipped)"));
    }
    Ok(Output {
        data: json!({ "added": added, "skipped": skipped, "url": url, "help": ["Run `rkb inbox` and `rkb promote` to triage them"] }),
        human: msg,
        exit: 0,
        raw: false,
    })
}

fn sanitize_agent(agent: &str) -> String {
    let valid =
        (1..=32).contains(&agent.len()) && agent.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if valid { agent.to_string() } else { "unknown".to_string() }
}

/// Strip control characters except newlines and cap the text at 4096 bytes on a char boundary.
fn cap_text(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars().filter(|c| !c.is_control() || *c == '\n') {
        if out.len() + ch.len_utf8() > 4096 {
            break;
        }
        out.push(ch);
    }
    out
}

#[derive(Serialize, Deserialize, Default)]
struct NtfyCursor {
    url: String,
    last_id: String,
    seen: Vec<String>,
}

/// The per-topic pull cursor from `state/ntfy-<hash>.json`, the hash of the url. `seen` is separate
/// from `last_id` so a pull after a since reset (or a re-pull) never adds a message twice.
fn ntfy_cursor(state: &Path, url: &str) -> (String, NtfyCursor) {
    let key: String = Sha256::digest(url.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect();
    let path = state.join(format!("ntfy-{key}.json"));
    let mut cursor: NtfyCursor = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    cursor.url = url.to_string();
    (key, cursor)
}

fn save_cursor(state: &Path, key: &str, cursor: &NtfyCursor) -> Result<(), CliError> {
    let fail = |e: std::io::Error| CliError::new(ErrorCode::Io, format!("cannot save the ntfy cursor: {e}"), "check the state dir");
    std::fs::create_dir_all(state).map_err(fail)?;
    let path = state.join(format!("ntfy-{key}.json"));
    let text = serde_json::to_string(cursor).unwrap_or_default();
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &text).map_err(fail)?;
    std::fs::rename(&tmp, &path).map_err(fail)
}
