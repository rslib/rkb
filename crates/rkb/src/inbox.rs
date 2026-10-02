use std::io::{IsTerminal, Read};

use rkb_core::distill::{self, Item, Kind, Meta};
use rkb_core::request;
use serde_json::{Value, json};

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
        priority: priority.map(|p| p.clamp(1, 3)),
        source: None,
        candidates: vec![],
    };
    let item = distill::add(&env.state, meta, &text)?;
    let human = format!("noted {} in the inbox; `/rkb-distill` turns inbox items into lessons", item.id);
    Ok(Output { data: json!({ "id": item.id, "help": ["Run `rkb inbox` to list the inbox"] }), human, exit: 0, raw: false })
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
        data: json!({ "saved": saved, "skipped": skipped, "help": ["Run the distill command to turn the notes into lessons"] }),
        human,
        exit: 0,
        raw: false,
    })
}

pub fn list(env: &Env) -> Result<Output, CliError> {
    let now = request::now();
    let expired = distill::expire(&env.state, now);
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
        human.push_str(&format!("\n{expired} items older than {} days expired\n", distill::EXPIRY_DAYS));
    }
    let help = if items.is_empty() {
        "The inbox is empty"
    } else {
        "Run `/rkb-distill`, or `rkb inbox show <id>`, write lessons with `rkb add`, then `rkb inbox done <id>`"
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
    let opts = Options { all: false, every_status: false, limit: settings.top.max(1), probes: search::ProbeMode::Cached };
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
    distill::expire(&env.state, request::now());
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
    let help = "Run `/rkb-distill` for the kept candidates, or `rkb inbox show <id>` for the verdicts of one item";
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
fn observer_setup(env: &Env, harness: &str, cwd: Option<&str>) -> Result<(rkb_core::config::ObserverConfig, Vec<String>), CliError> {
    use rkb_core::observer;
    let config = rkb_core::paths::config_dir();
    let cfg = observer::load(&config).map_err(|e| CliError::new(ErrorCode::Usage, e, "fix [observer] in config.toml"))?;
    if cfg.chain(harness).is_empty() {
        return Err(CliError::new(
            ErrorCode::Usage,
            format!("no observer model chain for {harness}"),
            format!("add `{harness} = [\"<model>\", \"session\"]` under [observer] in ~/.config/rkb/config.toml"),
        ));
    }
    if !observer::approved(&config, &cfg) {
        return Err(CliError::new(ErrorCode::Usage, "the observer is not approved here", "run `rkb approve observer`"));
    }
    rkb_core::kb::open(&env.root)?;
    let here = cwd.map(std::path::PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let place = rkb_core::state::locate(&env.root, &here, &rkb_core::matching::Hints::default(), &env.state)?;
    let name = |m: &Option<rkb_core::matching::Matched>| m.as_ref().map(|m| m.name.clone());
    let titles = observer::titles(&env.root, name(&place.project).as_deref(), name(&place.system).as_deref())?;
    Ok((cfg, titles))
}

fn observed(o: rkb_core::observer::Observed) -> Result<Output, CliError> {
    use rkb_core::observer::Observed;
    let (human, data) = match o {
        Observed::Added { item, dropped, via } => {
            let n = item.body.lines().count();
            (
                format!("observed {n} notes into inbox item {} via {via}; {dropped} other lines dropped", item.id),
                json!({ "outcome": "added", "id": item.id, "notes": n, "dropped": dropped, "via": via }),
            )
        }
        Observed::NoNotes { dropped, via } => (
            format!("no durable notes via {via}; {dropped} other lines dropped"),
            json!({ "outcome": "none", "id": null, "notes": 0, "dropped": dropped, "via": via }),
        ),
        Observed::Skipped(r) => (format!("skipped: {r}"), json!({ "outcome": "skipped", "id": null, "skipped": r })),
        Observed::Failed(r) => {
            return Err(CliError::new(
                ErrorCode::Refused,
                format!("every observer model failed: {r}"),
                "check the [observer] models; see `rkb doctor`",
            ));
        }
    };
    Ok(Output { data, human, exit: 0, raw: false })
}

/// `rkb observe`: runs the harness's approved model chain over the new part of one session.
pub fn observe(
    env: &Env,
    session: &str,
    transcript: &std::path::Path,
    harness: &str,
    cwd: Option<&str>,
    model: Option<&str>,
) -> Result<Output, CliError> {
    let (cfg, titles) = observer_setup(env, harness, cwd)?;
    let job = rkb_core::observer::Job { session, transcript, harness, cwd, model, titles };
    observed(rkb_core::observer::observe(&env.state, &cfg, &job)?)
}

fn next_prompt(env: &Env, id: &str, run: &rkb_core::observer::Run) -> Output {
    let models: Vec<Value> = run.models.iter().map(|(entry, model)| json!({ "entry": entry, "model": model })).collect();
    let human = format!(
        "job {id}: part {} of {}; ask the models in order: {}",
        run.next + 1,
        run.parts.len(),
        run.models.iter().map(|m| m.0.as_str()).collect::<Vec<_>>().join(", ")
    );
    let data = json!({
        "job": id,
        "kind": "observe",
        "models": models,
        "part": run.next,
        "parts": run.parts.len(),
        "prompt": run.prompt(&env.state),
        "help": [format!("Send the first answer to `rkb job finish {id} --part {} --model <entry>` on stdin", run.next)],
    });
    Output { data, human, exit: 0, raw: false }
}

fn no_job(reason: &str) -> Output {
    Output { data: json!({ "job": null, "skipped": reason }), human: format!("no job: {reason}"), exit: 0, raw: false }
}

/// `rkb job prepare observe`: the first prompt of an observer run for another runner, or why there is none.
pub fn prepare_observe(
    env: &Env,
    session: &str,
    transcript: &std::path::Path,
    harness: &str,
    cwd: Option<&str>,
    model: Option<&str>,
) -> Result<Output, CliError> {
    use rkb_core::{job, observer};
    let (cfg, titles) = match observer_setup(env, harness, cwd) {
        Ok(s) => s,
        Err(e) if e.code == ErrorCode::Usage => return Ok(no_job(&e.message)),
        Err(e) => return Err(e),
    };
    let spec = observer::Job { session, transcript, harness, cwd, model, titles };
    let run = match observer::prepare(&env.state, &cfg, &spec)? {
        observer::Prepared::Skipped(r) => return Ok(no_job(&r)),
        observer::Prepared::Ready(r) => *r,
    };
    std::fs::create_dir_all(&env.state)
        .map_err(|e| CliError::new(ErrorCode::Io, format!("cannot create {}: {e}", env.state.display()), "check the state folder"))?;
    let busy = |run: &observer::Run| {
        run.busy(&env.state);
        Ok(no_job("another observer run is active"))
    };
    let _guard = match rkb_core::lock::acquire(&observer::run_lock(&env.state), 6 * 3600, std::time::Duration::ZERO) {
        Ok(g) => g,
        Err(rkb_core::error::Error::Locked(_)) => return busy(&run),
        Err(e) => return Err(e.into()),
    };
    if job::open(&env.state).iter().any(|j| matches!(j.job, job::Job::Observe(_))) {
        return busy(&run);
    }
    let stored = job::Stored { id: job::new_id(), created: request::now(), job: job::Job::Observe(run.clone()) };
    job::save(&env.state, &stored)?;
    Ok(next_prompt(env, &stored.id, &run))
}

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
            observer::Ran::Ok(out) => return Ok(Some(triaged(triage::finish_ask(&env.state, &ask, entry, &out)?, entry))),
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
    let mut run = match stored.job {
        job::Job::Observe(run) => run,
        job::Job::Triage(ask) => return finish_triage(env, id, part, model, failed, &ask),
    };
    if part != run.next {
        return Err(CliError::new(
            ErrorCode::Usage,
            format!("job {id} waits for part {}, not {part}", run.next),
            format!("pass --part {}", run.next),
        ));
    }
    if let Some(reason) = failed {
        job::remove(&env.state, id);
        return observed(run.fail(&env.state, reason.to_string()));
    }
    let entries: Vec<&str> = run.models.iter().map(|m| m.0.as_str()).collect();
    let entry = model.filter(|m| entries.contains(m)).ok_or_else(|| {
        CliError::new(ErrorCode::Usage, "--model must name the chain entry that answered", format!("pass one of: {}", entries.join(", ")))
    })?;
    let mut out = String::new();
    std::io::stdin()
        .take(1 << 20)
        .read_to_string(&mut out)
        .map_err(|e| CliError::new(ErrorCode::Usage, format!("cannot read stdin: {e}"), "pipe the model's answer on stdin"))?;
    match run.answer(&env.state, entry, &[], &out)? {
        Some(o) => {
            job::remove(&env.state, id);
            observed(o)
        }
        None => {
            job::save(&env.state, &job::Stored { job: job::Job::Observe(run.clone()), ..stored })?;
            Ok(next_prompt(env, id, &run))
        }
    }
}
