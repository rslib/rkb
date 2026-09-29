use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use regex::Regex;
use serde_norway::Value;

use crate::conditions::{Facts, Verdict, evaluate};
use crate::error::{Error, Result};
use crate::kb::{FileKind, Snapshot, classify};
use crate::lesson::{Lesson, LessonType, Status};
use crate::matching::Place;
use crate::text::{each_token, edit_distance, is_version, summary, tokens};

pub const RANKED_BY: &str = "bm25";

const K1: f64 = 1.2;
const B: f64 = 0.75;
const EXPANDED_WEIGHT: f64 = 0.5;
/// Parts of a query token whose whole form is known, such as `use` in `H5_USE_18_API`.
const PART_WEIGHT: f64 = 0.5;
/// Results below this share of the best score are noise from common words and are dropped.
const MIN_RELATIVE_SCORE: f64 = 0.3;

#[derive(Debug, Clone)]
pub enum Mode {
    Ranked(String),
    Literal(String),
    Regex(String),
}

#[derive(Debug, Clone)]
pub struct Options {
    /// Search every folder and show results with `applies: no`.
    pub all: bool,
    /// Show superseded and archived lessons too.
    pub every_status: bool,
    pub limit: usize,
    /// Whether approved `Probe` scripts of the top results may run, or only their cached results count.
    pub probes: ProbeMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProbeMode {
    #[default]
    Off,
    /// Use cached probe results only, as hooks do.
    Cached,
    /// Run approved probes that have no fresh cached result, within `probe.budget_s`.
    Run,
}

pub const PROBE_TOP: usize = 10;
pub const DEFAULT_PROBE_BUDGET_S: i64 = 5;

/// Probe results by lesson id for `lessons`, from the cache and, with `ProbeMode::Run`, by running them in parallel.
fn probe_results(root: &Path, place: &Place, lessons: &[&Lesson], mode: ProbeMode) -> HashMap<String, crate::script::Result> {
    use crate::script::{self, Kind};
    let mut out = HashMap::new();
    if mode == ProbeMode::Off {
        return out;
    }
    let system = crate::approval::system_key(place);
    let config_dir = crate::paths::config_dir();
    let ttl = crate::facts::ttl(root);
    let mut to_run = vec![];
    for l in lessons {
        let Some(text) = script::of(l, Kind::Probe) else { continue };
        let sha = script::hash(&text);
        if !crate::approval::is_approved(&config_dir, &sha, &system) {
            continue;
        }
        let key = crate::facts::cache_key(&sha, &system);
        match crate::facts::get(&key, ttl) {
            Some(Some(v)) if v == "pass" => {
                out.insert(l.frontmatter.id.clone(), script::Result::Pass);
            }
            Some(Some(v)) if v == "fail" => {
                out.insert(l.frontmatter.id.clone(), script::Result::Fail);
            }
            Some(_) => {}
            None if mode == ProbeMode::Run => to_run.push((l, text, key)),
            None => {}
        }
    }
    if to_run.is_empty() {
        return out;
    }
    let kb: crate::config::KbConfig =
        std::fs::read_to_string(root.join("kb.toml")).ok().and_then(|t| crate::config::parse(&t).ok()).unwrap_or_default();
    let budget = kb.probe.as_ref().and_then(|t| t.get("budget_s")).and_then(|v| v.as_integer()).unwrap_or(DEFAULT_PROBE_BUDGET_S);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(budget.max(1) as u64);
    let state = crate::paths::state_dir();
    let ran: Vec<(String, script::Run)> = std::thread::scope(|s| {
        let handles: Vec<_> = to_run
            .iter()
            .map(|(l, text, key)| {
                let state = &state;
                s.spawn(move || {
                    let project_root = l
                        .path
                        .strip_prefix("projects/")
                        .and_then(|p| p.split('/').next())
                        .and_then(|p| crate::matching::checkouts(state, p).into_iter().find(|c| c.is_dir()));
                    let left = deadline.saturating_duration_since(std::time::Instant::now());
                    let run = script::run(text, project_root.as_deref(), left);
                    // A run cut off by the budget says nothing about the lesson, so it is not cached.
                    if run.code.is_some() {
                        let value = match run.result {
                            script::Result::Pass => Some("pass"),
                            script::Result::Fail => Some("fail"),
                            script::Result::Unknown => None,
                        };
                        crate::facts::put(key, value);
                    }
                    (l.frontmatter.id.clone(), run)
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    for (id, run) in ran {
        if run.result != script::Result::Unknown {
            out.insert(id, run.result);
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub id: String,
    pub kind: LessonType,
    pub title: String,
    pub path: String,
    pub status: Status,
    pub applies: Verdict,
    pub summary: String,
    pub score: f64,
    /// For literal and regex search: the first matching line and its number.
    pub line: Option<(usize, String)>,
    /// How well the lesson fits the query, when a rerank model scored it.
    pub relevance: Option<f32>,
    /// The `verified` date (`YYYY-MM-DD`) and how (`ran`, `read`, `told` or `checked`).
    pub verified: String,
    pub verified_how: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Results {
    pub hits: Vec<Hit>,
    /// Results hidden because `applies` is `no`, and the keys that said `no`.
    pub hidden: usize,
    pub hidden_by: BTreeMap<String, usize>,
}

/// The folders searched from `place`, or `None` for every folder.
fn scope(place: &Place, all: bool) -> Option<Vec<String>> {
    if all {
        return None;
    }
    let mut out = vec!["general/".to_string()];
    if let Some(p) = &place.project {
        out.push(format!("projects/{}/", p.name));
    }
    if let Some(s) = &place.system {
        out.push(format!("systems/{}/", s.name));
    }
    Some(out)
}

fn boost(path: &str, place: &Place) -> f64 {
    if place.project.as_ref().is_some_and(|p| path.starts_with(&format!("projects/{}/", p.name))) {
        1.3
    } else if place.system.as_ref().is_some_and(|s| path.starts_with(&format!("systems/{}/", s.name))) {
        1.15
    } else {
        1.0
    }
}

/// Title and H2 headings by their line prefix. Faster than a full `body::scan`, which also finds links;
/// a `#` line inside a code block can slip in, which only nudges ranking.
fn title_and_headings(l: &Lesson) -> (String, String) {
    let mut title = None;
    let mut headings = String::new();
    for line in l.body.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            headings.push_str(h);
            headings.push(' ');
        } else if let Some(t) = line.strip_prefix("# ").filter(|_| title.is_none()) {
            title = Some(t.trim().to_string());
        }
    }
    (title.unwrap_or_else(|| l.path.clone()), headings)
}

fn title(l: &Lesson) -> String {
    title_and_headings(l).0
}

fn hit(l: &Lesson, score: f64, line: Option<(usize, String)>) -> Hit {
    let fm = &l.frontmatter;
    Hit {
        id: fm.id.clone(),
        kind: fm.kind,
        title: title(l),
        path: l.path.clone(),
        status: fm.status,
        applies: Verdict::Unknown,
        summary: summary(fm.kind, &l.body).unwrap_or_default(),
        score,
        line,
        relevance: None,
        verified: fm.verified.to_string(),
        verified_how: serde_json::to_value(fm.verified_how).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default(),
    }
}

/// The text a reranker reads for each hit: the title, the type's first section, `Cause`, and `Fix` or `Steps`.
/// Only these, so a long lesson does not cost more to score.
pub fn rerank_items(root: &Path, hits: &[Hit]) -> Vec<String> {
    hits.iter()
        .map(|h| {
            let body = std::fs::read_to_string(root.join(&h.path))
                .ok()
                .and_then(|t| crate::lesson::split(&t).ok().map(|(_, b, _)| b.to_string()))
                .unwrap_or_default();
            let mut out = format!("# {}\n", h.title);
            for heading in [crate::text::lead_heading(h.kind), "Cause", "Fix", "Steps"] {
                if let Some(text) = crate::body::section_text(&body, heading) {
                    out.push_str(&format!("\n## {heading}\n{}\n", text.trim()));
                }
            }
            out
        })
        .collect()
}

/// Gives the first `scores.len()` hits their relevance and sorts them by it; the rest keep their BM25 order.
pub fn apply_relevance(hits: &mut [Hit], scores: &[f32]) {
    let n = scores.len().min(hits.len());
    for (h, s) in hits.iter_mut().zip(scores) {
        h.relevance = Some(*s);
    }
    hits[..n].sort_by(|a, b| b.relevance.unwrap_or(0.0).total_cmp(&a.relevance.unwrap_or(0.0)));
}

fn when_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Sequence(items) => items.iter().map(when_text).collect::<Vec<_>>().join(" "),
        other => serde_norway::to_string(other).unwrap_or_default(),
    }
}

/// The weighted text fields of a lesson: title, tags, `when`, headings, body.
fn fields<'a>(l: &'a Lesson, title: &str) -> [(f64, std::borrow::Cow<'a, str>); 5] {
    let (_, headings) = title_and_headings(l);
    let when: String =
        l.frontmatter.when.iter().map(|(k, v)| format!("{} {}", k.as_str().unwrap_or(""), when_text(v))).collect::<Vec<_>>().join(" ");
    [
        (3.0, title.to_string().into()),
        (2.5, l.frontmatter.tags.join(" ").into()),
        (1.5, when.into()),
        (1.5, headings.into()),
        (1.0, l.body.as_str().into()),
    ]
}

/// Weighted counts of `terms` in one lesson, and its length in tokens.
struct Counts {
    tf: Vec<f64>,
    len: f64,
}

fn count(l: &Lesson, title: &str, terms: &HashMap<String, usize>) -> Counts {
    let mut tf = vec![0.0; terms.len()];
    let mut len = 0.0;
    for (w, text) in fields(l, title) {
        each_token(&text, &mut |t| {
            len += 1.0;
            if let Some(&i) = terms.get(t) {
                tf[i] += w;
            }
        });
    }
    Counts { tf, len }
}

/// Runs `f` over `items` on all cores, keeping the order.
pub(crate) fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(items.len().max(1));
    let chunk = items.len().div_ceil(threads).max(1);
    std::thread::scope(|s| {
        let handles: Vec<_> = items.chunks(chunk).map(|c| s.spawn(|| c.iter().map(&f).collect::<Vec<R>>())).collect();
        handles.into_iter().flat_map(|h| h.join().expect("search thread")).collect()
    })
}

/// Query pieces: each whitespace-separated token as its whole term and its parts.
fn pieces(query: &str) -> Vec<Vec<String>> {
    query.split_whitespace().map(tokens).filter(|t| !t.is_empty()).collect()
}

/// BM25 scores, times the place boost and each lesson's usage factor (0.9–1.1).
fn rank(
    lessons: &[&Lesson],
    place: &Place,
    query: &str,
    usage: &std::collections::BTreeMap<String, crate::usage::Counts>,
) -> Vec<(usize, f64)> {
    let titles: Vec<String> = lessons.iter().map(|l| title(l)).collect();
    let pieces = pieces(query);
    let mut index: HashMap<String, usize> = HashMap::new();
    for t in pieces.iter().flatten() {
        let n = index.len();
        index.entry(t.clone()).or_insert(n);
    }
    let pairs: Vec<(&&Lesson, &String)> = lessons.iter().zip(&titles).collect();
    let mut counts: Vec<Counts> = parallel(&pairs, |(l, t)| count(l, t, &index));
    let df = |counts: &[Counts], i: usize| counts.iter().filter(|c| c.tf[i] > 0.0).count();

    // Weights: 1 for a whole term, lower for parts of a known whole and for typo expansions.
    let mut weights: Vec<(usize, f64)> = vec![];
    let mut missing = vec![];
    for piece in &pieces {
        let whole_known = df(&counts, index[&piece[0]]) > 0;
        for (j, t) in piece.iter().enumerate() {
            let i = index[t];
            if weights.iter().any(|(k, _)| *k == i) {
                continue;
            }
            if df(&counts, i) > 0 || is_version(t) {
                weights.push((i, if j > 0 && whole_known { PART_WEIGHT } else { 1.0 }));
            } else {
                missing.push(t.clone());
            }
        }
    }
    if !missing.is_empty() {
        let vocab: HashSet<String> = parallel(&pairs, |(l, t)| {
            let mut v = HashSet::new();
            for (_, text) in fields(l, t) {
                each_token(&text, &mut |x| {
                    if !v.contains(x) {
                        v.insert(x.to_string());
                    }
                });
            }
            v
        })
        .into_iter()
        .flatten()
        .collect();
        let mut added = vec![];
        for t in missing {
            let max = match t.chars().count() {
                0..=3 => continue,
                4..=7 => 1,
                _ => 2,
            };
            let mut close: Vec<(usize, &String)> = vocab.iter().map(|v| (edit_distance(&t, v), v)).filter(|(d, _)| *d <= max).collect();
            close.sort();
            // Two typos can expand to the same word; each counted term must appear once.
            for (_, v) in close.into_iter().take(3) {
                if !index.contains_key(v) && !added.contains(v) {
                    added.push(v.clone());
                }
            }
        }
        if !added.is_empty() {
            let extra: HashMap<String, usize> = added.iter().enumerate().map(|(i, t)| (t.clone(), i)).collect();
            let more = parallel(&pairs, |(l, t)| count(l, t, &extra));
            let base = index.len();
            for (c, m) in counts.iter_mut().zip(more) {
                c.tf.extend(m.tf);
            }
            for (i, _) in added.iter().enumerate() {
                weights.push((base + i, EXPANDED_WEIGHT));
            }
        }
    }

    let n = counts.len() as f64;
    let avg = (counts.iter().map(|c| c.len).sum::<f64>() / n.max(1.0)).max(1.0);
    let dfs: Vec<f64> = weights.iter().map(|(i, _)| df(&counts, *i) as f64).collect();
    let mut scored: Vec<(usize, f64)> = counts
        .iter()
        .enumerate()
        .filter_map(|(d, c)| {
            let s: f64 = weights
                .iter()
                .zip(&dfs)
                .filter(|((i, _), _)| c.tf[*i] > 0.0)
                .map(|((i, w), dfv)| {
                    let tf = c.tf[*i];
                    let idf = (1.0 + (n - dfv + 0.5) / (dfv + 0.5)).ln();
                    w * idf * tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * c.len / avg))
                })
                .sum();
            let used = usage.get(&lessons[d].frontmatter.id).map_or(1.0, crate::usage::Counts::factor);
            (s > 0.0).then(|| (d, s * boost(&lessons[d].path, place) * used))
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| titles[a.0].cmp(&titles[b.0])));
    let best = scored.first().map_or(0.0, |s| s.1);
    scored.retain(|s| s.1 >= best * MIN_RELATIVE_SCORE);
    scored
}

fn scope_order(path: &str, place: &Place) -> u8 {
    match boost(path, place) {
        b if b > 1.2 => 0,
        b if b > 1.0 => 1,
        _ => 2,
    }
}

/// Runs a search from `place` with `facts` for the `when` conditions.
pub fn search(root: &Path, place: &Place, facts: &Facts, mode: &Mode, opts: &Options) -> Result<Results> {
    let folders = scope(place, opts.all);
    let in_scope = |p: &str| folders.as_ref().is_none_or(|f| f.iter().any(|x| p.starts_with(x.as_str())));
    let paths: Vec<String> = crate::kb::paths(root)?.into_iter().filter(|p| classify(p) == FileKind::Lesson && in_scope(p)).collect();
    let texts: Vec<(&String, String)> = paths.iter().filter_map(|p| std::fs::read_to_string(root.join(p)).ok().map(|t| (p, t))).collect();
    let loaded: Vec<(Lesson, String)> =
        parallel(&texts, |(p, text)| crate::lesson::parse(p, text).ok().map(|l| (l, text.clone()))).into_iter().flatten().collect();
    let raw: HashMap<&str, &str> = loaded.iter().map(|(l, t)| (l.path.as_str(), t.as_str())).collect();
    let lessons: Vec<&Lesson> = loaded.iter().map(|(l, _)| l).filter(|l| opts.every_status || l.frontmatter.status.is_current()).collect();

    let ordered: Vec<Hit> = match mode {
        Mode::Ranked(q) => {
            rank(&lessons, place, q, &crate::usage::counts(root)).into_iter().map(|(i, s)| hit(lessons[i], s, None)).collect()
        }
        Mode::Literal(_) | Mode::Regex(_) => {
            let re = match mode {
                Mode::Literal(t) => Regex::new(&regex::escape(t)),
                Mode::Regex(p) => Regex::new(p),
                Mode::Ranked(_) => unreachable!(),
            }
            .map_err(|e| Error::BadPattern(e.to_string()))?;
            let mut found: Vec<Hit> = lessons
                .iter()
                .filter_map(|l| {
                    let text = raw[l.path.as_str()];
                    let (n, line) = text.lines().enumerate().find(|(_, line)| re.is_match(line))?;
                    Some(hit(l, 1.0, Some((n + 1, line.trim().to_string()))))
                })
                .collect();
            found.sort_by(|a, b| scope_order(&a.path, place).cmp(&scope_order(&b.path, place)).then(a.title.cmp(&b.title)));
            found
        }
    };

    let by_id: HashMap<&str, &&Lesson> = lessons.iter().map(|l| (l.frontmatter.id.as_str(), l)).collect();
    let evaluated: Vec<(Hit, crate::conditions::Applies)> = ordered
        .into_iter()
        .map(|h| {
            let a = evaluate(&by_id[h.id.as_str()].frontmatter.when, facts);
            (h, a)
        })
        .collect();
    let top: Vec<&Lesson> =
        evaluated.iter().filter(|(_, a)| a.result != Verdict::No).take(PROBE_TOP).map(|(h, _)| *by_id[h.id.as_str()]).collect();
    let probes = probe_results(root, place, &top, opts.probes);

    let mut out = Results::default();
    for (mut h, applies) in evaluated {
        if out.hits.len() >= opts.limit {
            break;
        }
        let mut no_keys: Vec<String> = applies.keys.iter().filter(|k| k.result == Verdict::No).map(|k| k.key.clone()).collect();
        h.applies = match probes.get(&h.id) {
            Some(crate::script::Result::Pass) => Verdict::Yes,
            Some(crate::script::Result::Fail) => {
                no_keys = vec!["probe".to_string()];
                Verdict::No
            }
            _ => applies.result,
        };
        if h.applies == Verdict::No && !opts.all {
            out.hidden += 1;
            for k in no_keys {
                *out.hidden_by.entry(k).or_default() += 1;
            }
            continue;
        }
        out.hits.push(h);
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub id: String,
    pub kind: LessonType,
    pub title: String,
    pub path: String,
    pub score: f64,
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// How well `query` names `candidate`: 0 for no match, higher is better.
fn name_score(query: &str, candidate: &str) -> f64 {
    let (q, c) = (query.trim().to_lowercase(), candidate.to_lowercase());
    if q.is_empty() {
        return 0.0;
    }
    if c.contains(&q) {
        return 100.0 - (c.len() as f64 / 100.0).min(1.0);
    }
    let (qw, cw) = (words(&q), words(&c));
    if qw.iter().all(|w| cw.iter().any(|x| x.starts_with(w.as_str()))) {
        return 80.0;
    }
    let mut cost = 0;
    for w in &qw {
        let allowed = if w.chars().count() <= 4 { 1 } else { 2 };
        let best = cw
            .iter()
            .map(|x| {
                let prefix: String = x.chars().take(w.chars().count()).collect();
                if x.starts_with(w.as_str()) { 0 } else { edit_distance(w, x).min(edit_distance(w, &prefix)) }
            })
            .min()
            .unwrap_or(usize::MAX);
        if best > allowed {
            return 0.0;
        }
        cost += best;
    }
    60.0 - 5.0 * cost as f64
}

/// Lessons whose title, slug, path, tags or id match `query` roughly, best first. Every folder and status.
pub fn find(root: &Path, query: &str, limit: usize) -> Result<Vec<Found>> {
    let snap = Snapshot::from_dir_where(root, |p| classify(p) == FileKind::Lesson)?;
    let (lessons, _) = snap.lessons();
    let mut found: Vec<Found> = lessons
        .iter()
        .filter_map(|l| {
            let t = title(l);
            let slug = l.path.rsplit('/').next().unwrap_or("").trim_end_matches(".md").to_string();
            let mut names = vec![t.clone(), slug, l.path.clone(), l.frontmatter.id.clone()];
            names.extend(l.frontmatter.tags.iter().cloned());
            let score = names.iter().map(|n| name_score(query, n)).fold(0.0, f64::max);
            (score > 0.0).then(|| Found { id: l.frontmatter.id.clone(), kind: l.frontmatter.kind, title: t, path: l.path.clone(), score })
        })
        .collect();
    found.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.title.len().cmp(&b.title.len())));
    found.truncate(limit);
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matching::{Matched, Rule};

    fn write(root: &Path, rel: &str, id: &str, title: &str, extra: &str, body: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            p,
            format!("---\nschema: 1\nid: {id}\ntype: fact\nstatus: active\n{extra}verified: 2026-09-25\nverified_how: ran\n---\n\n# {title}\n\n## Statement\n{body}\n\n## Evidence\nSeen.\n"),
        )
        .unwrap();
    }

    fn opts() -> Options {
        Options { all: false, every_status: false, limit: 10, probes: ProbeMode::Off }
    }

    fn ids(r: &Results) -> Vec<&str> {
        r.hits.iter().map(|h| h.id.as_str()).collect()
    }

    fn place(project: Option<&str>) -> Place {
        Place { project: project.map(|p| Matched { name: p.into(), rule: Rule::Flag }), system: None, repo: None, dir: None }
    }

    #[test]
    fn usage_breaks_a_tie() {
        let kb = tempfile::tempdir().unwrap();
        let r = kb.path();
        write(r, "general/a/x.md", "0000000001", "Striping large files", "", "Words.");
        write(r, "general/a/y.md", "0000000002", "Striping large files", "", "Words.");
        let facts = Facts { values: Default::default(), ancestry: None };
        let q = Mode::Ranked("striping".into());
        let before = ids(&search(r, &place(None), &facts, &q, &opts()).unwrap()).iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let other = if before[0] == "0000000001" { "0000000002" } else { "0000000001" };
        let dir = r.join(crate::usage::DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let lines: String =
            (0..3).map(|_| serde_json::to_string(&crate::usage::now(other, "worked", None, None)).unwrap() + "\n").collect();
        std::fs::write(dir.join("m-0000.jsonl"), lines).unwrap();
        let after = search(r, &place(None), &facts, &q, &opts()).unwrap();
        assert_eq!(ids(&after)[0], other, "the lesson that worked three times ranks first");
        assert_eq!(after.hits.len(), 2, "nothing is hidden");
    }

    #[test]
    fn ranking_scope_status_and_applies() {
        let kb = tempfile::tempdir().unwrap();
        let r = kb.path();
        write(r, "general/a/t.md", "0000000001", "Striping large files", "", "Some words.");
        write(r, "general/a/b.md", "0000000002", "Unrelated title", "", "One mention of striping here.");
        write(r, "projects/p/a/c.md", "0000000003", "Striping in the project", "", "Words.");
        write(r, "projects/other/a/d.md", "0000000004", "Striping elsewhere", "", "Words.");
        write(r, "general/a/e.md", "0000000005", "Old striping note", "", "Words.");
        let old = std::fs::read_to_string(r.join("general/a/e.md")).unwrap().replace("status: active", "status: archived");
        std::fs::write(r.join("general/a/e.md"), old + "\n## Why archived\nGone.\n").unwrap();
        write(r, "general/a/f.md", "0000000006", "Striping for old hdf5", "when:\n  hdf5: \":1.12\"\n", "Words.");

        let facts = Facts { values: [("hdf5".to_string(), "1.14.3".to_string())].into(), ancestry: None };
        let q = Mode::Ranked("striping".into());
        let res = search(r, &place(Some("p")), &facts, &q, &opts()).unwrap();
        assert_eq!(ids(&res)[0], "0000000003", "project first: {:?}", ids(&res));
        assert!(ids(&res).iter().position(|i| *i == "0000000001") < ids(&res).iter().position(|i| *i == "0000000002"));
        assert!(!ids(&res).contains(&"0000000004") && !ids(&res).contains(&"0000000005"));
        assert_eq!((res.hidden, res.hidden_by.get("hdf5").copied()), (1, Some(1)));

        let all = search(r, &place(Some("p")), &facts, &q, &Options { all: true, ..opts() }).unwrap();
        assert!(ids(&all).contains(&"0000000004") && ids(&all).contains(&"0000000006"));
        let every = search(r, &place(Some("p")), &facts, &q, &Options { every_status: true, ..opts() }).unwrap();
        assert!(ids(&every).contains(&"0000000005"));

        let typo = search(r, &place(None), &facts, &Mode::Ranked("stripng".into()), &opts()).unwrap();
        assert!(ids(&typo).contains(&"0000000001"));
        // Two typos of one word, and a typo of a word the query already has, used to count a term twice.
        let twice = search(r, &place(None), &facts, &Mode::Ranked("stripng strping striping".into()), &opts()).unwrap();
        assert!(ids(&twice).contains(&"0000000001"));
        assert!(search(r, &place(None), &facts, &Mode::Ranked("1.13".into()), &opts()).unwrap().hits.is_empty());
    }

    #[test]
    fn literal_regex_and_find() {
        let kb = tempfile::tempdir().unwrap();
        let r = kb.path();
        write(
            r,
            "general/cmake/a.md",
            "0000000001",
            "CMake cannot find HDF5 unless HDF5_ROOT is set",
            "",
            "Configure says Could NOT find HDF5 (missing: HDF5_LIBRARIES).",
        );
        write(r, "general/git/b.md", "0000000002", "Shallow clones break git describe", "tags:\n  - git\n", "No names found.");
        let f = Facts::default();
        let lit = search(r, &place(None), &f, &Mode::Literal("Could NOT find HDF5 (".into()), &opts()).unwrap();
        assert_eq!(ids(&lit), ["0000000001"]);
        assert!(lit.hits[0].line.as_ref().unwrap().1.contains("Could NOT find HDF5"));
        let re = search(r, &place(None), &f, &Mode::Regex(r"HDF5_\w+".into()), &opts()).unwrap();
        assert_eq!(ids(&re), ["0000000001"]);
        assert!(matches!(search(r, &place(None), &f, &Mode::Regex("(".into()), &opts()), Err(Error::BadPattern(_))));

        assert_eq!(find(r, "hdf5 rot", 10).unwrap()[0].id, "0000000001");
        assert_eq!(find(r, "shallwo clone", 10).unwrap()[0].id, "0000000002");
        assert_eq!(find(r, "0000000002", 10).unwrap()[0].id, "0000000002");
        assert!(find(r, "zzzz qqqq", 10).unwrap().is_empty());
    }
}
