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
    json!({
        "id": i.id,
        "kind": i.meta.kind.as_str(),
        "priority": i.meta.priority(),
        "age": age(now.saturating_sub(i.meta.time)),
        "lines": i.body.lines().count(),
        "preview": i.preview(),
    })
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
        let head = format!("{} {} {} {} lines", i.meta.kind.as_str(), m.sep, age(now.saturating_sub(i.meta.time)), i.body.lines().count());
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

pub fn show(env: &Env, id: &str) -> Result<Output, CliError> {
    let item = distill::get(&env.state, id)?;
    let mut data = row(&item, request::now());
    data["meta"] = serde_json::to_value(&item.meta).unwrap_or(Value::Null);
    data["body"] = json!(item.body);
    Ok(Output { data, human: item.text(), exit: 0, raw: false })
}

pub fn done(env: &Env, ids: &[String]) -> Result<Output, CliError> {
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

/// `rkb job finish`: the answer to one part of a job, read from stdin, or `failed` when every model failed.
pub fn finish(env: &Env, id: &str, part: usize, model: Option<&str>, failed: Option<&str>) -> Result<Output, CliError> {
    use rkb_core::job::{self, Missing};
    let stored = job::load(&env.state, id).map_err(|m| match m {
        Missing::Expired => CliError::new(ErrorCode::Expired, format!("job {id} is older than one hour"), "prepare the job again"),
        Missing::NotFound => CliError::new(ErrorCode::NotFound, format!("no job {id}"), "prepare the job again"),
    })?;
    let job::Job::Observe(mut run) = stored.job;
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
