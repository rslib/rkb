use rkb_core::conditions::{Facts, Verdict};
use rkb_core::eval;
use rkb_core::lesson::Status;
use rkb_core::rerank::{self, BM25, Ranked, Settings};
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
    let items = search::rerank_items(&env.root, &r.hits[..n]);
    let timeout = std::time::Duration::from_millis(timeout_ms.unwrap_or(settings.timeout_ms));
    let ranked = rerank::run(&settings, only, &crate::rerankers::opener(&settings), query, &items, timeout)?;
    if let Some(scores) = &ranked.scores {
        search::apply_relevance(&mut r.hits, scores);
    }
    r.hits.truncate(limit);
    Ok(ranked)
}

pub fn search(env: &Env, facts: &Facts, mode: Mode, opts: Options, only: Option<&str>) -> Result<Output, CliError> {
    let place = env.place.clone().unwrap_or_default();
    let ranked = matches!(mode, Mode::Ranked(_));
    let top = if ranked { Settings::load(&env.root).top } else { 0 };
    let limit = opts.limit;
    let mut r = search::search(&env.root, &place, facts, &mode, &Options { limit: limit.max(top), ..opts })?;
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
        if h.status != Status::Active {
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
                "applies": h.applies.as_str(),
                "worked": u.worked,
                "failed": u.failed,
                "injected": u.injected,
                "irrelevant": u.irrelevant,
            });
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
    let mut data = json!({ "query": query, "mode": label, "results": rows });
    if ranked {
        data["ranked_by"] = json!(ranked_by);
    }
    if r.hidden > 0 {
        data["hidden"] = json!(r.hidden);
        data["hidden_by"] = json!(r.hidden_by);
    }
    let help: Vec<String> = if r.hits.is_empty() {
        vec![
            "Run `rkb find <words>` to look up a lesson by name".into(),
            "Add --all to search every folder and show hidden results".into(),
            "Run `rkb list` to browse the topics".into(),
        ]
    } else {
        vec!["Run `rkb show <id>` to read a lesson; check `applies` before you act on it".into()]
    };
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
fn recall_thresholds(root: &std::path::Path) -> (f64, f64) {
    let hooks = std::fs::read_to_string(root.join("kb.toml"))
        .ok()
        .and_then(|t| rkb_core::config::parse::<rkb_core::config::KbConfig>(&t).ok())
        .and_then(|c| c.hooks)
        .unwrap_or_default();
    let get = |k: &str, d: f64| hooks.get(k).and_then(|v| v.as_float()).unwrap_or(d);
    (get("recall_min_relevance", rkb_core::hooks::RECALL_MIN_RELEVANCE), get("recall_min_margin", rkb_core::hooks::RECALL_MIN_MARGIN))
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
    let opener = crate::rerankers::opener(&settings);
    let mut ranked_by = BM25.to_string();
    let depth = if backend == BM25 { 0 } else { settings.top };
    let report = eval::run(&env.root, &queries, depth, &mut |q, r| {
        if backend == BM25 {
            return Ok(());
        }
        let items = search::rerank_items(&env.root, &r.hits[..settings.top.min(r.hits.len())]);
        // Eval is not interactive, so a slow backend still counts; only a skip fails it.
        let ranked = rerank::run(&settings, Some(backend), &opener, q, &items, std::time::Duration::from_secs(120))?;
        if ranked.backend != backend {
            return Err(rkb_core::Error::Refused(format!(
                "rerank backend {backend} did not run ({}); unset RKB_NO_MODEL, or run `rkb models fetch`",
                ranked.describe()
            )));
        }
        if let Some(scores) = &ranked.scores {
            search::apply_relevance(&mut r.hits, scores);
        }
        ranked_by = ranked.describe();
        Ok(())
    })?;
    let pass = min_recall.is_none_or(|m| report.recall_at_5 + 1e-9 >= m);
    let (min, margin) = recall_thresholds(&env.root);
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
