use rkb_core::conditions::{Facts, Verdict};
use rkb_core::eval;
use rkb_core::lesson::Status;
use rkb_core::rerank::{BM25, Ranked, Settings};
use rkb_core::search::{self, Found, Mode, Options, RANKED_BY, Results};
use serde_json::{Value, json};

use crate::list::{cut, kind_name, len, marker, marks, width};
use crate::output::{CliError, ErrorCode, Output, paint};
use crate::writes::Env;

const SUMMARY_LIMIT: usize = 160;

fn applies_color(v: Verdict) -> &'static str {
    match v {
        Verdict::Yes => "32",
        Verdict::No => "31",
        Verdict::Unknown => "33",
    }
}

/// The color of a tier in the result line: proven is good, archive is a tombstone.
fn tier_color(t: rkb_core::tier::Tier) -> &'static str {
    match t {
        rkb_core::tier::Tier::Proven => "32",
        rkb_core::tier::Tier::Candidate => "2",
        rkb_core::tier::Tier::Archive => "33",
    }
}

fn hidden_text(r: &Results) -> Option<String> {
    (r.hidden > 0).then(|| {
        let keys: Vec<String> = r.hidden_by.keys().cloned().collect();
        format!("hidden: {} (applies=no: {}); --all shows them", r.hidden, keys.join(", "))
    })
}

/// Whether the chain would try a model at all, so a BM25-only search skips building rerank texts.
fn wants_model(settings: &Settings, only: Option<&str>) -> bool {
    if std::env::var("RKB_NO_MODEL").is_ok_and(|v| v == "1") {
        return false;
    }
    match only {
        Some(b) => b != BM25,
        None => settings.chain.first().is_some_and(|b| b != BM25),
    }
}

/// Reranks the first `rerank.top` hits with the chain and cuts the list to `limit`.
pub fn rerank_hits(
    env: &Env,
    query: &str,
    r: &mut Results,
    limit: usize,
    only: Option<&str>,
    timeout_ms: Option<u64>,
) -> Result<Ranked, CliError> {
    let settings = Settings::load(&env.root);
    let n = if wants_model(&settings, only) { settings.top.min(r.hits.len()) } else { 0 };
    let timeout = std::time::Duration::from_millis(timeout_ms.unwrap_or(settings.timeout_ms));
    let ranked = crate::rerankers::run(&env.root, &settings, only, query, &r.hits[..n], timeout)?;
    if let Some(scores) = &ranked.scores {
        search::apply_relevance(&mut r.hits, scores, ranked.order.as_deref());
    }
    r.hits.truncate(limit);
    Ok(ranked)
}

pub fn search(env: &Env, facts: &Facts, mode: Mode, opts: Options, only: Option<&str>) -> Result<Output, CliError> {
    let place = env.place.clone().unwrap_or_default();
    let roots = rkb_core::kb::Roots::load(&env.root, &rkb_core::paths::config_dir())?;
    let ranked = matches!(mode, Mode::Ranked(_));
    let top = if ranked { Settings::load(&env.root).top } else { 0 };
    let limit = opts.limit;
    let mut r = search::search_with(&env.root, &roots.extras, &place, facts, &mode, &Options { limit: limit.max(top), ..opts })?;
    let ranking = match &mode {
        Mode::Ranked(q) => Some(rerank_hits(env, q, &mut r, limit, only, None)?),
        _ => None,
    };
    let ranked_by = ranking.as_ref().map(Ranked::describe).unwrap_or_else(|| RANKED_BY.to_string());
    let (label, query) = match &mode {
        Mode::Ranked(q) => ("search", q.clone()),
        Mode::Literal(t) => ("literal", t.clone()),
        Mode::Regex(p) => ("regex", p.clone()),
    };
    let m = marks();
    let w = width();
    let c = env.colored;

    let mut human = format!(
        "{} {} {} {}",
        paint(c, "1", &format!("{label} \"{query}\"")),
        m.sep,
        if r.hits.len() == 1 { "1 result".to_string() } else { format!("{} results", r.hits.len()) },
        if ranked { format!("{} reranker: {ranked_by}", m.sep) } else { String::new() }
    )
    .trim_end()
    .to_string();
    human.push('\n');
    for h in &r.hits {
        let mut right = vec![];
        right.push(paint(c, tier_color(h.tier), h.tier.as_str()));
        if let Some(d) = h.expired {
            right.push(paint(c, "33", &format!("expired {d}")));
        } else if h.status != Status::Active {
            right.push(paint(c, "33", h.status.as_str()));
        }
        if let Some(rel) = h.relevance {
            right.push(paint(c, "36", &format!("{rel:.2}")));
        }
        right.push(paint(c, applies_color(h.applies), h.applies.as_str()));
        let right_plain: usize = right.iter().map(|s| len(&strip(s)) + 2).sum();
        let (mk, color) = marker(h.kind, m);
        let title = cut(&h.title, w.saturating_sub(4 + right_plain).max(20), m);
        let pad = w.saturating_sub(4 + len(&title) + right_plain).min(w);
        human.push_str(&format!("\n  {} {title}{}  {}\n", paint(c, color, mk), " ".repeat(pad), right.join("  ")));
        human.push_str(&format!("    {}\n", paint(c, "2", &format!("{} {} {}", h.id, m.sep, h.path))));
        let detail = match &h.line {
            Some((n, line)) => format!("L{n}: {line}"),
            None => h.summary.clone(),
        };
        if !detail.is_empty() {
            human.push_str(&format!("    {}\n", paint(c, "2", &cut(&detail, w.saturating_sub(4), m))));
        }
    }
    if r.hits.is_empty() && r.hidden > 0 {
        human.push_str("\nNo lesson that applies here matches.\n");
    } else if r.hits.is_empty() {
        human.push_str("\nNo lessons match. Try `rkb find` for names, `--all` for every folder, or `rkb list` to browse.\n");
    }
    if let Some(h) = hidden_text(&r) {
        human.push_str(&format!("\n{}\n", paint(c, "2", &h)));
    }

    let usage = rkb_core::usage::counts(&env.root);
    let rows: Vec<Value> = r
        .hits
        .iter()
        .map(|h| {
            let u = usage.get(&h.id).cloned().unwrap_or_default();
            let mut row = json!({
                "id": h.id,
                "type": kind_name(h.kind),
                "title": h.title,
                "path": h.path,
                "status": h.status.as_str(),
                "tier": h.tier.as_str(),
                "applies": h.applies.as_str(),
                "worked": u.worked,
                "failed": u.failed,
                "injected": u.injected,
                "irrelevant": u.irrelevant,
            });
            if let Some(kb) = &h.kb {
                row["kb"] = json!(kb);
            }
            if let Some(d) = h.expired {
                row["expired"] = json!(d.to_string());
            }
            if let Some(rel) = h.relevance {
                row["relevance"] = json!(crate::output::score2(rel));
            }
            match &h.line {
                Some((n, line)) => row["line"] = json!(format!("{n}: {}", crate::output::cut(line, SUMMARY_LIMIT, "rkb show <id>"))),
                None => row["summary"] = json!(crate::output::cut(&h.summary, SUMMARY_LIMIT, "rkb show <id>")),
            }
            row
        })
        .collect();
    let inbox = crate::inbox::search_hits(&env.state, &query);
    if !inbox.is_empty() {
        human.push_str(&format!("\n{}\n", paint(c, "1", "Inbox, not lessons yet:")));
        for (id, line, at) in &inbox {
            human.push_str(&format!("  {id}  L{at}: {}\n", cut(line, w.saturating_sub(22), m)));
        }
    }
    let mut data = json!({ "query": query, "mode": label, "results": rows });
    if !inbox.is_empty() {
        data["inbox"] = json!(inbox.iter().map(|(id, line, at)| json!({ "id": id, "line": line, "at": at })).collect::<Vec<_>>());
    }
    if ranked {
        data["ranked_by"] = json!(ranked_by);
    }
    if r.hidden > 0 {
        data["hidden"] = json!(r.hidden);
        data["hidden_by"] = json!(r.hidden_by);
    }
    let help: Vec<String> =
        if r.hits.is_empty() {
            vec![
                "Run `rkb find <words>` to look up a lesson by name".into(),
                "Add --all to search every folder and show hidden results".into(),
                "Run `rkb list` to browse the topics".into(),
            ]
        } else {
            vec!["Run `rkb show <id>` to read a lesson; check `applies` before you act on it".into()]
        }
        .into_iter()
        .chain((!inbox.is_empty()).then(|| {
            "Run `rkb show <id> --line <at>` for an inbox item; if it fixes the problem, run `rkb used <id> --worked`".to_string()
        }))
        // One line per extra knowledge base the search had to skip, so a down share is visible.
        .chain(roots.skipped.iter().map(|(name, why)| format!("the extra knowledge base `{name}` was skipped: {why}; run `rkb doctor`")))
        .collect();
    data["help"] = json!(help);
    Ok(Output { data, human, exit: 0, raw: false })
}

/// Removes ANSI color codes, to measure the visible width of a painted string.
fn strip(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            for x in chars.by_ref() {
                if x == 'm' {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

pub fn find(env: &Env, query: &str, limit: usize) -> Result<Output, CliError> {
    let found: Vec<Found> = search::find(&env.root, query, limit)?;
    let m = marks();
    let w = width();
    let c = env.colored;
    let mut human = format!("{} {} {}\n", paint(c, "1", &format!("find \"{query}\"")), m.sep, found.len());
    for f in &found {
        let (mk, color) = marker(f.kind, m);
        human.push_str(&format!(
            "\n  {} {}\n    {}\n",
            paint(c, color, mk),
            cut(&f.title, w.saturating_sub(4), m),
            paint(c, "2", &format!("{} {} {}", f.id, m.sep, f.path))
        ));
    }
    if found.is_empty() {
        human.push_str("\nNo lesson name matches. Try `rkb search` with words from the problem.\n");
    }
    let rows: Vec<Value> =
        found.iter().map(|f| json!({ "id": f.id, "type": kind_name(f.kind), "title": f.title, "path": f.path })).collect();
    let help = if found.is_empty() { "Run `rkb search <words>` to search the lesson text" } else { "Run `rkb show <id>` to read a lesson" };
    Ok(Output { data: json!({ "query": query, "results": rows, "help": [help] }), human, exit: 0, raw: false })
}

/// `hooks.recall_min_relevance` and `hooks.recall_min_margin` from kb.toml, else the defaults.
fn recall_thresholds(root: &std::path::Path, backend: &str) -> (f64, f64) {
    let hooks = std::fs::read_to_string(root.join("kb.toml"))
        .ok()
        .and_then(|t| rkb_core::config::parse::<rkb_core::config::KbConfig>(&t).ok())
        .and_then(|c| c.hooks)
        .unwrap_or_default();
    let get = |k: &str, d: f64| hooks.get(k).and_then(|v| v.as_float()).unwrap_or(d);
    let (min, margin) = rkb_core::hooks::recall_defaults(backend);
    (get("recall_min_relevance", min), get("recall_min_margin", margin))
}

pub fn eval(
    env: &Env,
    file: Option<String>,
    self_check: bool,
    min_recall: Option<f64>,
    backend: &str,
    sweep: bool,
) -> Result<Output, CliError> {
    let queries = if self_check {
        eval::self_queries(&env.root)?
    } else {
        let path = file.map(std::path::PathBuf::from).unwrap_or_else(|| env.root.join("eval/queries.toml"));
        let text = std::fs::read_to_string(&path).map_err(|e| {
            CliError::new(
                ErrorCode::NotFound,
                format!("cannot read {}: {e}", path.display()),
                "create it with [[query]] tables (text, expect), pass --queries <file>, or run `rkb eval --self`",
            )
        })?;
        eval::parse(&text).map_err(|e| {
            CliError::new(
                ErrorCode::Usage,
                format!("{}: {e}", path.display()),
                "each [[query]] has text, expect and optionally mode, with, project, system",
            )
        })?
    };
    let mut settings = Settings::load(&env.root);
    settings.strict = true;
    let mut ranked_by = BM25.to_string();
    let depth = if backend == BM25 { 0 } else { settings.top };
    let report = eval::run(&env.root, &queries, depth, &mut |q, r| {
        if backend == BM25 {
            return Ok(());
        }
        // Eval is not interactive, so a slow backend still counts; only a skip fails it.
        let n = settings.top.min(r.hits.len());
        let ranked = crate::rerankers::run(&env.root, &settings, Some(backend), q, &r.hits[..n], std::time::Duration::from_secs(120))?;
        if ranked.backend != backend {
            return Err(rkb_core::Error::Refused(format!(
                "rerank backend {backend} did not run ({}); unset RKB_NO_MODEL",
                ranked.describe()
            )));
        }
        if let Some(scores) = &ranked.scores {
            search::apply_relevance(&mut r.hits, scores, ranked.order.as_deref());
        }
        ranked_by = ranked.describe();
        Ok(())
    })?;
    let pass = min_recall.is_none_or(|m| report.recall_at_5 + 1e-9 >= m);
    let (min, margin) = recall_thresholds(&env.root, backend);
    let c = env.colored;
    let mut human = String::new();
    let scores = |r: &eval::Row| -> String {
        r.top.iter().filter_map(|t| t.1).map(|s| format!("{:.2}", crate::output::score2(s))).collect::<Vec<_>>().join(" ")
    };
    for r in &report.rows {
        let rank = if !r.answerable() {
            paint(c, "2", " n")
        } else {
            let rank = r.rank.map_or_else(|| "-".to_string(), |k| k.to_string());
            if r.rank.is_some_and(|k| k <= 5) { paint(c, "32", &format!("{rank:>2}")) } else { paint(c, "31", &format!("{rank:>2}")) }
        };
        let verdict = eval::recall(r, min, margin).map(|v| {
            let color = match v {
                eval::Recall::Right => "32",
                eval::Recall::Wrong => "31",
                eval::Recall::Silent => "2",
            };
            format!("  {} {}", paint(c, color, &format!("{:<6}", v.as_str())), paint(c, "2", &scores(r)))
        });
        human.push_str(&format!("{rank}  {}{}\n", r.text, verdict.unwrap_or_default()));
    }
    let answerable = report.rows.iter().filter(|r| r.answerable()).count();
    human.push_str(&format!(
        "\n{} queries ({} with no answer)  recall@5 {:.2}  mrr {:.2}  ranked by: {ranked_by}",
        report.rows.len(),
        report.rows.len() - answerable,
        report.recall_at_5,
        report.mrr
    ));
    if let Some(m) = min_recall {
        human.push_str(&format!("  ({} the minimum {m:.2})", if pass { "meets" } else { "BELOW" }));
    }
    let missed: Vec<&str> =
        report.rows.iter().filter(|r| r.answerable() && !r.rank.is_some_and(|k| k <= 5)).map(|r| r.expect[0].as_str()).collect();
    if !missed.is_empty() {
        human.push_str(&format!("\nnot in the first 5: {}", missed.join(" ")));
    }
    let totals = eval::tally(&report.rows, min, margin);
    match totals {
        Some([right, wrong, silent]) => {
            human.push_str(&format!("\nrecall at {min:.2}, margin {margin:.2}: {right} right, {wrong} wrong, {silent} silent"))
        }
        None => human.push_str("\nrecall does not apply: no model ranked the results"),
    }
    let mut grid = vec![];
    if sweep && totals.is_some() {
        human.push_str("\n\n min  margin  right  wrong  silent");
        for m in eval::SWEEP_MIN {
            for g in eval::SWEEP_MARGIN {
                let [right, wrong, silent] = eval::tally(&report.rows, m, g).expect("a model ranked the results");
                human.push_str(&format!("\n{m:.2}   {g:.2}   {right:>5}  {wrong:>5}  {silent:>6}"));
                grid.push(json!({ "min": m, "margin": g, "right": right, "wrong": wrong, "silent": silent }));
            }
        }
    }
    let rows: Vec<Value> = report
        .rows
        .iter()
        .map(|r| {
            let mut row = json!({ "query": r.text, "answerable": r.answerable(), "rank": r.rank.unwrap_or(0) });
            if let Some(v) = eval::recall(r, min, margin) {
                row["recall"] = json!(v.as_str());
                row["top"] =
                    json!(r.top.iter().map(|(id, s)| json!({ "id": id, "relevance": s.map(crate::output::score2) })).collect::<Vec<_>>());
            }
            row
        })
        .collect();
    let mut data = json!({
        "queries": report.rows.len(),
        "answerable": answerable,
        "recall_at_5": (report.recall_at_5 * 1000.0).round() / 1000.0,
        "mrr": (report.mrr * 1000.0).round() / 1000.0,
        "ranked_by": ranked_by,
        "rows": rows,
        "missed": missed,
        "help": ["rank 0 means not in the first 10; add a missed search to the query file to keep it tested"],
    });
    if let Some([right, wrong, silent]) = totals {
        data["recall"] = json!({ "min_relevance": min, "margin": margin, "right": right, "wrong": wrong, "silent": silent });
    }
    if sweep && !grid.is_empty() {
        data["recall_sweep"] = json!(grid);
    }
    Ok(Output { data, human, exit: u8::from(!pass), raw: false })
}

/// The knowledge base's committed revision, so a reviewed set pins what it was read against.
fn kb_revision(root: &std::path::Path) -> Result<String, CliError> {
    let out = rkb_core::git::run(root, &["rev-parse", "HEAD"]).map_err(|e| {
        CliError::new(ErrorCode::Refused, format!("cannot verify KB revision: {e}"), "run against a committed knowledge base")
    })?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// `rkb eval --draft-reviewed <file>`: one reviewed-set entry for each logged injection with a
/// later use report. The file must not exist; the user reviews the draft before `--reviewed` reads it.
pub fn draft_reviewed(env: &Env, path: &str) -> Result<Output, CliError> {
    use rkb_core::{hooks, usage};
    if std::path::Path::new(path).exists() {
        return Err(CliError::new(ErrorCode::Refused, format!("{path} exists; nothing was written"), "pass a new file"));
    }
    let replays: Vec<eval::DraftReplay> = hooks::replays(&env.state, 90)
        .into_iter()
        .map(|r| eval::DraftReplay {
            session: r.session,
            query: r.query,
            injected: r.injected,
            project: r.project,
            system: r.system,
            time: r.time,
        })
        .collect();
    let reports: Vec<eval::DraftReport> = usage::records(&env.root)
        .into_iter()
        .map(|r| eval::DraftReport { id: r.id, event: r.event, session: r.session, time: r.time })
        .collect();
    let text = eval::draft_reviewed(&replays, &reports, &kb_revision(&env.root)?)
        .map_err(|e| CliError::new(ErrorCode::Refused, e.to_string(), "check the replay log and the use records"))?;
    let events = text.matches("[[event]]").count();
    std::fs::write(path, &text).map_err(|e| CliError::new(ErrorCode::Io, format!("cannot write {path}: {e}"), "check the path"))?;
    let human = format!(
        "drafted {events} reviewed events from {} replays into {path}; review it, then run `rkb eval --reviewed {path}`",
        replays.len()
    );
    Ok(Output { data: serde_json::json!({ "file": path, "events": events, "replays": replays.len() }), human, exit: 0, raw: false })
}

/// `rkb eval --reviewed <file> --first-stage`: for each path, how many answerable events
/// (at least one expected lesson) have none of those lessons in the BM25 candidate set the
/// reranker receives. It makes no model call, loads no reranker and writes nothing.
pub fn first_stage(env: &Env, path: &str) -> Result<Output, CliError> {
    use rkb_core::eval::EventPath;
    use rkb_core::hooks;
    use rkb_core::matching::{Matched, Place, Rule};
    let text = std::fs::read_to_string(path)
        .map_err(|e| CliError::new(ErrorCode::NotFound, format!("cannot read {path}: {e}"), "pass a readable reviewed manifest"))?;
    let set = eval::parse_reviewed(&text, &env.root)
        .map_err(|e| CliError::new(ErrorCode::Usage, e.to_string(), "fix the reviewed manifest and retry"))?;
    let current_revision = kb_revision(&env.root)?;
    if current_revision != set.kb_revision {
        return Err(CliError::new(
            ErrorCode::Refused,
            format!("reviewed manifest drift: expected KB revision {}, found {current_revision}", set.kb_revision),
            "refresh or select a manifest pinned to this KB revision",
        ));
    }
    let settings = Settings::load(&env.root);
    // The candidate set is the exact BM25 list `reviewed` passes to the reranker: the same
    // query, place, facts and limit, without the rerank step.
    let mut failure_rows = Vec::with_capacity(set.events.len());
    let mut prompt_rows = Vec::with_capacity(set.events.len());
    let mut paraphrase_rows = Vec::with_capacity(set.events.len());
    for event in &set.events {
        let query = match event.path {
            EventPath::Failure => event
                .error
                .as_deref()
                .zip(event.command.as_deref())
                .map_or_else(|| event.text.clone(), |(error, command)| hooks::error_query(command, error)),
            EventPath::Prompt => hooks::typed_text(&event.text),
        };
        let matched = |name: &Option<String>| name.as_ref().map(|n| Matched { name: n.clone(), rule: Rule::Flag });
        let place = Place { project: matched(&event.project), system: matched(&event.system), repo: None, dir: None };
        let facts = Facts::gather(&env.root, &place, &[]);
        let found = search::search(
            &env.root,
            &place,
            &facts,
            &Mode::Ranked(query),
            &Options {
                all: false,
                every_status: false,
                limit: settings.top.max(eval::DEPTH),
                probes: search::ProbeMode::Cached,
                tier: None,
            },
        )?;
        let answerable = !event.expect.is_empty();
        let hit = answerable && found.hits.iter().any(|h| event.expect.contains(&h.id));
        match event.path {
            EventPath::Failure => failure_rows.push((answerable, hit)),
            EventPath::Prompt => prompt_rows.push((answerable, hit)),
        }
        if event.paraphrase {
            paraphrase_rows.push((answerable, hit));
        }
    }
    let failure = eval::first_stage(failure_rows);
    let prompt = eval::first_stage(prompt_rows);
    let paraphrase = eval::first_stage(paraphrase_rows);
    let round = |v: Option<f64>| v.map(|x| (x * 1000.0).round() / 1000.0);
    let line = |name: &str, s: eval::FirstStage| match s.rate() {
        Some(r) => format!("\n{name} {} of {} ({r:.2})", s.misses, s.answerable),
        None => format!("\n{name} {} of {}", s.misses, s.answerable),
    };
    let human = format!(
        "first-stage misses (BM25 candidates):{}{}{}",
        line("failure", failure),
        line("prompt", prompt),
        line("paraphrase", paraphrase)
    );
    let group = |s: eval::FirstStage| json!({ "answerable": s.answerable, "misses": s.misses, "rate": round(s.rate()) });
    let data = json!({
        "paths": { "failure": group(failure), "prompt": group(prompt) },
        "paraphrase": group(paraphrase),
        "help": ["a miss has no expected lesson in the BM25 candidates; 5% or more says hybrid search is worth it"],
    });
    Ok(Output { data, human, exit: 0, raw: false })
}

pub fn reviewed(env: &Env, path: &str, backend: Option<&str>, sweep: bool, pick: Option<f64>) -> Result<Output, CliError> {
    use rkb_core::eval::{EventPath, ReviewedDecision, ReviewedObservation};
    use rkb_core::hooks;
    use rkb_core::matching::{Matched, Place, Rule};
    if let Some(p) = pick
        && !(0.0 < p && p <= 1.0)
    {
        return Err(CliError::new(ErrorCode::Usage, format!("--pick {p} is not within (0, 1]"), "pass a precision such as --pick 0.95"));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| CliError::new(ErrorCode::NotFound, format!("cannot read {path}: {e}"), "pass a readable reviewed manifest"))?;
    let set = eval::parse_reviewed(&text, &env.root)
        .map_err(|e| CliError::new(ErrorCode::Usage, e.to_string(), "fix the reviewed manifest and retry"))?;
    let current_revision = kb_revision(&env.root)?;
    if current_revision != set.kb_revision {
        return Err(CliError::new(
            ErrorCode::Refused,
            format!("reviewed manifest drift: expected KB revision {}, found {current_revision}", set.kb_revision),
            "refresh or select a manifest pinned to this KB revision",
        ));
    }
    let settings = Settings::load(&env.root);
    let requested = backend.unwrap_or("chain").to_string();
    let mut actual_backends = std::collections::BTreeSet::new();
    let mut skipped_backends: Vec<(String, String)> = Vec::new();
    let mut observations = Vec::with_capacity(set.events.len());
    let cfg = crate::hook::hooks_config(&env.root);
    for event in &set.events {
        let unavailable_failure = event.path == EventPath::Failure && (event.error.is_none() || event.command.is_none());
        let query = match event.path {
            EventPath::Failure => event
                .error
                .as_deref()
                .zip(event.command.as_deref())
                .map_or_else(|| event.text.clone(), |(error, command)| hooks::error_query(command, error)),
            EventPath::Prompt => hooks::typed_text(&event.text),
        };
        let skipped_prompt =
            event.path == EventPath::Prompt && (query.trim_start().starts_with('/') || query.split_whitespace().count() < 3);
        if skipped_prompt {
            observations.push(ReviewedObservation {
                path: event.path,
                expect: event.expect.clone(),
                top: vec![],
                scores: vec![],
                decision: ReviewedDecision::Silent,
                backend: None,
                ranked_available: false,
                curve: vec![],
            });
            continue;
        }
        let matched = |name: &Option<String>| name.as_ref().map(|n| Matched { name: n.clone(), rule: Rule::Flag });
        let place = Place { project: matched(&event.project), system: matched(&event.system), repo: None, dir: None };
        let facts = Facts::gather(&env.root, &place, &[]);
        let mut found = search::search(
            &env.root,
            &place,
            &facts,
            &Mode::Ranked(query.clone()),
            &Options {
                all: false,
                every_status: false,
                limit: settings.top.max(eval::DEPTH),
                probes: search::ProbeMode::Cached,
                tier: None,
            },
        )?;
        let ranked = crate::rerankers::run(&env.root, &settings, backend, &query, &found.hits, std::time::Duration::from_secs(120))?;
        skipped_backends.extend(ranked.skipped.iter().cloned());
        if let Some(requested_backend) = backend.filter(|name| *name != BM25)
            && ranked.backend != requested_backend
        {
            return Err(CliError::new(
                ErrorCode::Refused,
                format!("requested backend {requested_backend} fell back to {}", ranked.describe()),
                "run with the requested backend available",
            ));
        }
        actual_backends.insert(ranked.backend.clone());
        if let Some(scores) = &ranked.scores {
            search::apply_relevance(&mut found.hits, scores, ranked.order.as_deref());
        }
        let ranked_available = true;
        let top: Vec<String> = found.hits.iter().take(eval::DEPTH).map(|h| h.id.clone()).collect();
        let scores: Vec<Option<f32>> = found.hits.iter().take(eval::DEPTH).map(|h| h.relevance).collect();
        let active_backend = ranked.backend.clone();
        let (min, margin) = recall_thresholds(&env.root, &active_backend);
        let decision_at = |min: f64, margin: f64| {
            if unavailable_failure {
                return ReviewedDecision::Unavailable;
            }
            if event.path == EventPath::Failure {
                let mut strength = hooks::Strength::from_config(&cfg, &active_backend);
                strength.min_relevance = min;
                strength.min_margin = margin;
                let chosen = event.error.as_deref().and_then(|error| {
                    hooks::would_inject(
                        &found.hits,
                        |id| rkb_core::kb::find(&env.root, id).map(|(_, t)| t).unwrap_or_default(),
                        error,
                        &strength,
                    )
                });
                // The hook pushes a proven lesson, or one candidate as its explore push.
                if chosen.is_some_and(|h| hooks::may_push(h, false).is_some()) {
                    ReviewedDecision::Injected
                } else {
                    ReviewedDecision::Silent
                }
            } else if ranked.scores.is_some() {
                let score = |h: Option<&rkb_core::search::Hit>| h.and_then(|h| h.relevance).map_or(0.0, f64::from);
                let inject = hooks::clear_winner(score(found.hits.first()), score(found.hits.get(1)), min, margin)
                    && found.hits.first().is_some_and(|h| h.applies != Verdict::No && h.status == Status::Active);
                if inject { ReviewedDecision::Injected } else { ReviewedDecision::Silent }
            } else {
                ReviewedDecision::Silent
            }
        };
        let decision = decision_at(min, margin);
        let mut curve = vec![(min, margin, decision)];
        if (sweep || pick.is_some()) && ranked.scores.is_some() {
            curve.extend(
                eval::SWEEP_MIN
                    .into_iter()
                    .flat_map(|min| eval::SWEEP_MARGIN.into_iter().map(move |margin| (min, margin, decision_at(min, margin)))),
            );
        }
        observations.push(ReviewedObservation {
            path: event.path,
            expect: event.expect.clone(),
            top,
            scores,
            decision,
            backend: Some(active_backend),
            ranked_available,
            curve,
        });
    }

    let metric_json = |m: &eval::Metric| json!({ "numerator": m.numerator, "denominator": m.denominator, "value": m.value.map(|v| (v * 1000.0).round() / 1000.0) });
    let metrics_json = |m: &eval::ReviewedMetrics| {
        json!({
            "samples": m.samples,
            "eligible": m.eligible,
            "unknown": m.unknown,
            "skipped": m.skipped,
            "answerable": m.answerable,
            "no_answer": m.no_answer,
            "correct": m.correct,
            "wrong": m.wrong,
            "silent_positive": m.silent_positive,
            "silent_negative": m.silent_negative,
            "unavailable": m.unavailable,
            "recall_at_1": metric_json(&m.recall_at_1),
            "recall_at_5": metric_json(&m.recall_at_5),
            "precision": metric_json(&m.precision),
            "false_injection_rate": metric_json(&m.false_injection),
            "injection_rate": metric_json(&m.injection_rate),
            "risk": metric_json(&m.risk),
            "coverage": metric_json(&m.coverage),
        })
    };
    let path_metrics = |path: EventPath| {
        let rows: Vec<_> = observations.iter().filter(|r| r.path == path).cloned().collect();
        metrics_json(&eval::reviewed_metrics(&rows))
    };
    let all = eval::reviewed_metrics(&observations);
    let mut backend_metrics = serde_json::Map::new();
    let mut backend_human = String::new();
    let mut backend_names: Vec<Option<String>> = actual_backends.iter().cloned().map(Some).collect();
    if observations.iter().any(|row| row.backend.is_none()) {
        backend_names.push(None);
    }
    for backend_name in backend_names {
        let rows: Vec<_> = observations.iter().filter(|row| row.backend == backend_name).cloned().collect();
        let name = backend_name.as_deref().unwrap_or("skipped").to_string();
        let failure: Vec<_> = rows.iter().filter(|row| row.path == EventPath::Failure).cloned().collect();
        let prompt: Vec<_> = rows.iter().filter(|row| row.path == EventPath::Prompt).cloned().collect();
        let f = eval::reviewed_metrics(&failure);
        let p = eval::reviewed_metrics(&prompt);
        backend_metrics.insert(name.clone(), json!({ "failure": metrics_json(&f), "prompt": metrics_json(&p) }));
        backend_human.push_str(&format!("\n{name}: failure {} correct {} wrong {} silent+ {} silent- {} unavailable {}; prompt {} correct {} wrong {} silent+ {} silent- {} unavailable {}",  f.samples, f.correct, f.wrong, f.silent_positive, f.silent_negative, f.unavailable, p.samples, p.correct, p.wrong, p.silent_positive, p.silent_negative, p.unavailable));
    }
    let mut curves = Vec::new();
    let mut curve_specs: Vec<(String, EventPath, f64, f64)> = Vec::new();
    for row in &observations {
        if row.backend.as_deref().is_some_and(|name| name != BM25) {
            for (min, margin, _) in &row.curve {
                if !curve_specs.iter().any(|(backend_name, path_kind, a, b)| {
                    backend_name == row.backend.as_ref().unwrap()
                        && *path_kind == row.path
                        && a.to_bits() == min.to_bits()
                        && b.to_bits() == margin.to_bits()
                }) {
                    curve_specs.push((row.backend.clone().unwrap(), row.path, *min, *margin));
                }
            }
        }
    }
    for (backend_name, path_kind, min, margin) in curve_specs {
        let rows: Vec<_> = observations
            .iter()
            .filter(|r| r.backend.as_deref() == Some(backend_name.as_str()) && r.path == path_kind)
            .map(|r| {
                let mut row = r.clone();
                row.decision = r
                    .curve
                    .iter()
                    .find(|(a, b, _)| a.to_bits() == min.to_bits() && b.to_bits() == margin.to_bits())
                    .map_or(ReviewedDecision::Unavailable, |(_, _, decision)| *decision);
                row
            })
            .collect();
        let m = eval::reviewed_metrics(&rows);
        let literal_gate = path_kind == EventPath::Failure && hooks::Strength::from_config(&cfg, &backend_name).literal_gate;
        curves.push(json!({ "backend": backend_name, "path": match path_kind { EventPath::Failure => "failure", EventPath::Prompt => "prompt" }, "decision_rule": if literal_gate { "literal-gate" } else { "relevance-threshold" }, "min_relevance": min, "margin": margin, "injections": m.coverage.numerator, "events": m.coverage.denominator, "wrong": m.risk.numerator, "risk": metric_json(&m.risk), "coverage": metric_json(&m.coverage) }));
    }
    let mut skipped_counts = std::collections::BTreeMap::<(String, String), usize>::new();
    for skipped in skipped_backends {
        *skipped_counts.entry(skipped).or_default() += 1;
    }
    let skipped_report: Vec<_> = skipped_counts
        .into_iter()
        .map(|((backend_name, reason), count)| json!({ "backend": backend_name, "reason": reason, "count": count }))
        .collect();
    let mut data = json!({
        "manifest": { "kb_revision": set.kb_revision, "source_fingerprint": set.source_fingerprint },
        "backend": { "requested": requested, "actual": actual_backends, "fallback": backend.is_none() && !skipped_report.is_empty(), "skipped": skipped_report },
        "paths": { "failure": path_metrics(EventPath::Failure), "prompt": path_metrics(EventPath::Prompt) },
        "by_backend": backend_metrics,
        "metrics": metrics_json(&all),
        "bm25_decisions": actual_backends.contains(BM25).then(|| {
            json!({ "backend": BM25, "failure": metrics_json(&eval::reviewed_metrics_for(&observations, Some(BM25), Some(EventPath::Failure))), "prompt": metrics_json(&eval::reviewed_metrics_for(&observations, Some(BM25), Some(EventPath::Prompt))) })
        }),
        "model_risk_coverage": curves,
        "help": ["reviewed evaluation reads the manifest and cached probes only"],
    });
    let mut pick_note = String::new();
    if let Some(p) = pick {
        let mut picks_data = serde_json::Map::new();
        let mut pick_human = String::new();
        for backend_name in &actual_backends {
            let mut tables = vec![];
            let mut paths_data = serde_json::Map::new();
            for path_kind in [EventPath::Failure, EventPath::Prompt] {
                let name = match path_kind {
                    EventPath::Failure => "failure",
                    EventPath::Prompt => "prompt",
                };
                let rows: Vec<_> = observations
                    .iter()
                    .filter(|r| r.backend.as_deref() == Some(backend_name.as_str()) && r.path == path_kind)
                    .cloned()
                    .collect();
                match eval::pick(&rows, p) {
                    Some((min, margin)) => {
                        tables.push(format!("{name} = {{ min = {min:?}, margin = {margin:?} }}"));
                        paths_data.insert(name.to_string(), json!({ "min": min, "margin": margin }));
                    }
                    None if rows.is_empty() => {
                        pick_human.push_str(&format!("\n{name} {backend_name}: no eligible events"));
                        paths_data.insert(name.to_string(), Value::Null);
                    }
                    None => {
                        pick_human.push_str(&format!("\n{name} {backend_name}: no point reaches precision {p}"));
                        paths_data.insert(name.to_string(), Value::Null);
                    }
                }
            }
            if !tables.is_empty() {
                pick_human.push_str(&format!("\n[hooks.thresholds.{backend_name}]\n{}", tables.join("\n")));
            }
            picks_data.insert(backend_name.clone(), Value::Object(paths_data));
        }
        data["picks"] = Value::Object(picks_data);
        pick_note = format!("\npick {p}:{pick_human}");
    }
    let ratio = |m: &eval::Metric| {
        m.value.map_or_else(|| format!("{}/{} (-)", m.numerator, m.denominator), |v| format!("{}/{} ({v:.2})", m.numerator, m.denominator))
    };
    let mut human = format!(
        "reviewed evaluation: {} events, KB revision {}\nsource fingerprint: {}\nbackends: {:?}\nfailure: recall@1 {} recall@5 {} precision {} false-injection {} injection {}\nprompt: recall@1 {} recall@5 {} precision {} false-injection {} injection {}",
        all.samples,
        set.kb_revision,
        set.source_fingerprint,
        actual_backends,
        ratio(
            &eval::reviewed_metrics(&observations.iter().filter(|r| r.path == EventPath::Failure).cloned().collect::<Vec<_>>()).recall_at_1
        ),
        ratio(
            &eval::reviewed_metrics(&observations.iter().filter(|r| r.path == EventPath::Failure).cloned().collect::<Vec<_>>()).recall_at_5
        ),
        ratio(
            &eval::reviewed_metrics(&observations.iter().filter(|r| r.path == EventPath::Failure).cloned().collect::<Vec<_>>()).precision
        ),
        ratio(
            &eval::reviewed_metrics(&observations.iter().filter(|r| r.path == EventPath::Failure).cloned().collect::<Vec<_>>())
                .false_injection
        ),
        ratio(
            &eval::reviewed_metrics(&observations.iter().filter(|r| r.path == EventPath::Failure).cloned().collect::<Vec<_>>())
                .injection_rate
        ),
        ratio(
            &eval::reviewed_metrics(&observations.iter().filter(|r| r.path == EventPath::Prompt).cloned().collect::<Vec<_>>()).recall_at_1
        ),
        ratio(
            &eval::reviewed_metrics(&observations.iter().filter(|r| r.path == EventPath::Prompt).cloned().collect::<Vec<_>>()).recall_at_5
        ),
        ratio(&eval::reviewed_metrics(&observations.iter().filter(|r| r.path == EventPath::Prompt).cloned().collect::<Vec<_>>()).precision),
        ratio(
            &eval::reviewed_metrics(&observations.iter().filter(|r| r.path == EventPath::Prompt).cloned().collect::<Vec<_>>())
                .false_injection
        ),
        ratio(
            &eval::reviewed_metrics(&observations.iter().filter(|r| r.path == EventPath::Prompt).cloned().collect::<Vec<_>>())
                .injection_rate
        )
    );
    let counts = |m: &eval::ReviewedMetrics| {
        format!(
            "samples {} eligible {} unknown {} skipped {} correct {} wrong {} silent+ {} silent- {} unavailable {}",
            m.samples, m.eligible, m.unknown, m.skipped, m.correct, m.wrong, m.silent_positive, m.silent_negative, m.unavailable
        )
    };
    let failure_rows: Vec<_> = observations.iter().filter(|r| r.path == EventPath::Failure).cloned().collect();
    let prompt_rows: Vec<_> = observations.iter().filter(|r| r.path == EventPath::Prompt).cloned().collect();
    human.push_str(&format!(
        "\ncounts: failure {}\nprompt {}{}",
        counts(&eval::reviewed_metrics(&failure_rows)),
        counts(&eval::reviewed_metrics(&prompt_rows)),
        backend_human
    ));
    let unavailable_failure =
        observations.iter().filter(|r| r.path == EventPath::Failure && r.decision == ReviewedDecision::Unavailable).count();
    let unavailable_prompt =
        observations.iter().filter(|r| r.path == EventPath::Prompt && r.decision == ReviewedDecision::Unavailable).count();
    human.push_str(&format!(
        "\nunavailable: failure {unavailable_failure}, prompt {unavailable_prompt}; risk {} coverage {}; curve points {} (see JSON/TOON model_risk_coverage)",
        ratio(&all.risk),
        ratio(&all.coverage),
        data["model_risk_coverage"].as_array().map_or(0, Vec::len)
    ));
    human.push_str(&pick_note);
    Ok(Output { data, human, exit: 0, raw: false })
}

/// `rkb eval --replay`: reruns this machine's logged failure queries with the failure hook's own rule.
/// Ground truth is a heuristic: a lesson written in a session (in `written.jsonl`, or an older lesson's `meta.source.session`) most likely
/// fixes one of that session's failures.
pub fn replay(env: &Env, days: u64, min_recall: Option<f64>, backend: Option<&str>) -> Result<Output, CliError> {
    use rkb_core::hooks;
    use rkb_core::matching::{Matched, Place, Rule};
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    let log = hooks::replays(&env.state, days);
    if log.is_empty() {
        let human = format!("nothing to replay: no failure logged in the last {days} days");
        return Ok(Output { data: json!({ "failures": 0, "help": [human] }), human, exit: 0, raw: false });
    }
    let snap = rkb_core::kb::Snapshot::from_dir(&env.root)?;
    let (lessons, _) = snap.lessons();
    let by_id: HashMap<&str, &rkb_core::lesson::Lesson> = lessons.iter().map(|l| (l.frontmatter.id.as_str(), l)).collect();
    let mut written: HashMap<String, BTreeSet<String>> = HashMap::new();
    let legacy = lessons.iter().filter_map(|l| {
        let session = l.frontmatter.meta.get("source").and_then(|s| s.get("session")).and_then(|s| s.as_str())?;
        Some((session.to_string(), l.frontmatter.id.clone()))
    });
    for (session, id) in hooks::written(&env.state).into_iter().chain(legacy) {
        let Some(mut now) = by_id.get(id.as_str()).copied() else { continue };
        for _ in 0..10 {
            match (now.frontmatter.status, now.frontmatter.superseded_by.as_deref().and_then(|n| by_id.get(n))) {
                (Status::Superseded, Some(next)) => now = next,
                _ => break,
            }
        }
        if now.frontmatter.status.is_current() {
            written.entry(session).or_default().insert(now.frontmatter.id.clone());
        }
    }
    let irrelevant: BTreeSet<(String, String)> = rkb_core::usage::records(&env.root)
        .into_iter()
        .filter(|r| r.event == "irrelevant")
        .filter_map(|r| Some((r.session?, r.id)))
        .collect();

    let settings = Settings::load(&env.root);
    let cfg = crate::hook::hooks_config(&env.root);
    let hook_ms = cfg.get("hook_timeout_ms").and_then(toml::Value::as_integer).map_or(hooks::DEFAULT_HOOK_TIMEOUT_MS, |v| v.max(0) as u64);
    let timeout = std::time::Duration::from_millis(if backend.is_some() { 120_000 } else { hook_ms });
    let mut ranked_by = BM25.to_string();
    let (mut found, mut would, mut would_irrelevant) = (BTreeSet::new(), 0, 0);
    let mut uncovered: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for r in &log {
        let matched = |name: &Option<String>| name.as_ref().map(|n| Matched { name: n.clone(), rule: Rule::Flag });
        let place = Place { project: matched(&r.project), system: matched(&r.system), repo: None, dir: None };
        let facts = Facts::gather(&env.root, &place, &[]);
        let opts = Options { all: false, every_status: false, limit: settings.top.max(1), probes: search::ProbeMode::Cached, tier: None };
        let mut res = search::search(&env.root, &place, &facts, &Mode::Ranked(r.query.clone()), &opts)?;
        let ranked = crate::rerankers::run(&env.root, &settings, backend, &r.query, &res.hits, timeout)?;
        if let Some(scores) = &ranked.scores {
            search::apply_relevance(&mut res.hits, scores, ranked.order.as_deref());
        }
        ranked_by = ranked.describe();
        let expect = written.get(&r.session);
        if expect.is_some_and(|e| res.hits.iter().take(5).any(|h| e.contains(&h.id))) {
            found.insert(r.session.clone());
        }
        let text_of = |id: &str| rkb_core::kb::find(&env.root, id).map(|(_, t)| t).unwrap_or_default();
        // The hook pushes a proven lesson, or one candidate as its explore push.
        let chosen = hooks::would_inject(&res.hits, text_of, &r.query, &hooks::Strength::from_config(&cfg, &ranked.backend))
            .filter(|h| hooks::may_push(h, false).is_some());
        match chosen {
            Some(h) => {
                would += 1;
                would_irrelevant += usize::from(irrelevant.contains(&(r.session.clone(), h.id.clone())));
            }
            None if expect.is_none() => {
                let program = r.query.split_whitespace().next().unwrap_or("?").to_string();
                let e = uncovered.entry(program).or_insert((0, r.query.clone()));
                e.0 += 1;
            }
            None => {}
        }
    }
    let with_lesson: BTreeSet<&String> = log.iter().map(|r| &r.session).filter(|s| written.contains_key(*s)).collect();
    let recall = (!with_lesson.is_empty()).then(|| found.len() as f64 / with_lesson.len() as f64);
    let pass = match (min_recall, recall) {
        (Some(m), Some(r)) => r + 1e-9 >= m,
        _ => true,
    };
    let sessions: BTreeSet<&String> = log.iter().map(|r| &r.session).collect();
    let mut uncovered: Vec<(String, usize, String)> = uncovered.into_iter().map(|(p, (n, q))| (p, n, q)).collect();
    uncovered.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let c = env.colored;
    let mut human =
        format!("{}\n", paint(c, "1", &format!("Replay of {} failures from {} sessions, last {days} days", log.len(), sessions.len())));
    match recall {
        Some(x) => human.push_str(&format!(
            "  recall@5 {x:.2}: {} of {} sessions that wrote a lesson find it in the first 5",
            found.len(),
            with_lesson.len()
        )),
        None => human.push_str("  recall@5 -: no logged session wrote a lesson yet"),
    }
    if let (Some(m), Some(_)) = (min_recall, recall) {
        human.push_str(&format!("  ({} the minimum {m:.2})", if pass { "meets" } else { "BELOW" }));
    }
    human.push_str(&format!(
        "\n  the failure hook would now add a lesson for {would} of {} failures; {would_irrelevant} of those were marked irrelevant in that session",
        log.len()
    ));
    if !uncovered.is_empty() {
        human.push_str("\n  uncovered (no strong lesson, and the session wrote none):");
        for (p, n, q) in &uncovered {
            human.push_str(&format!("\n    {p:<12} {n:>3}  {}", paint(c, "2", &crate::output::cut(q, 80, ""))));
        }
    }
    human.push_str(&format!("\n  ranked by: {ranked_by}"));
    human.push_str("\n  a session counts as found when a lesson written in it ranks in the first 5 for one of its failures");
    let data = json!({
        "failures": log.len(),
        "sessions": sessions.len(),
        "sessions_with_lesson": with_lesson.len(),
        "found": found.len(),
        "recall_at_5": recall.map(|x| (x * 1000.0).round() / 1000.0),
        "would_inject": would,
        "irrelevant": would_irrelevant,
        "uncovered": uncovered.iter().map(|(p, n, q)| json!({ "program": p, "count": n, "example": q })).collect::<Vec<_>>(),
        "ranked_by": ranked_by,
        "help": ["a session counts as found when a lesson written in it ranks in the first 5 for one of its failures"],
    });
    Ok(Output { data, human, exit: u8::from(!pass), raw: false })
}
