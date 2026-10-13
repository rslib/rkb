use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::Deserialize;

use crate::conditions::Facts;
use crate::error::{Error, Result};
use crate::kb::{FileKind, Snapshot, classify};
use crate::matching::{Matched, Place, Rule};
use crate::search::{Mode, Options, Results, search};

pub const DEPTH: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewedDecision {
    Injected,
    Silent,
    Unavailable,
}

#[derive(Debug, Clone)]
pub struct ReviewedObservation {
    pub path: EventPath,
    pub expect: Vec<String>,
    pub top: Vec<String>,
    pub scores: Vec<Option<f32>>,
    pub decision: ReviewedDecision,
    pub backend: Option<String>,
    pub ranked_available: bool,
    pub curve: Vec<(f64, f64, ReviewedDecision)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Metric {
    pub numerator: usize,
    pub denominator: usize,
    pub value: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReviewedMetrics {
    pub samples: usize,
    pub eligible: usize,
    pub unknown: usize,
    pub skipped: usize,
    pub answerable: usize,
    pub no_answer: usize,
    pub correct: usize,
    pub wrong: usize,
    pub silent_positive: usize,
    pub silent_negative: usize,
    pub unavailable: usize,
    pub recall_at_1: Metric,
    pub recall_at_5: Metric,
    pub precision: Metric,
    pub false_injection: Metric,
    pub injection_rate: Metric,
    pub risk: Metric,
    pub coverage: Metric,
}

fn metric(numerator: usize, denominator: usize) -> Metric {
    Metric { numerator, denominator, value: if denominator == 0 { None } else { Some(numerator as f64 / denominator as f64) } }
}

pub fn reviewed_metrics(rows: &[ReviewedObservation]) -> ReviewedMetrics {
    let skipped = rows.iter().filter(|r| !r.ranked_available).count();
    let unavailable = rows.iter().filter(|r| r.ranked_available && r.decision == ReviewedDecision::Unavailable).count();
    let unknown = unavailable;
    let eligible = rows.len().saturating_sub(skipped + unknown);
    let answerable = rows.iter().filter(|r| !r.expect.is_empty() && r.ranked_available).count();
    let no_answer =
        rows.iter().filter(|r| r.expect.is_empty() && r.ranked_available && r.decision != ReviewedDecision::Unavailable).count();
    let at = |n: usize| {
        metric(
            rows.iter()
                .filter(|r| !r.expect.is_empty() && r.ranked_available && r.top.iter().take(n).any(|id| r.expect.contains(id)))
                .count(),
            answerable,
        )
    };
    let injections = rows.iter().filter(|r| r.ranked_available && r.decision == ReviewedDecision::Injected).count();
    let correct = rows
        .iter()
        .filter(|r| {
            r.ranked_available
                && r.decision == ReviewedDecision::Injected
                && !r.expect.is_empty()
                && r.top.first().is_some_and(|id| r.expect.contains(id))
        })
        .count();
    let wrong = injections.saturating_sub(correct);
    let silent_positive =
        rows.iter().filter(|r| r.ranked_available && r.decision == ReviewedDecision::Silent && !r.expect.is_empty()).count();
    let silent_negative =
        rows.iter().filter(|r| r.ranked_available && r.decision == ReviewedDecision::Silent && r.expect.is_empty()).count();
    let false_injection =
        rows.iter().filter(|r| r.ranked_available && r.decision == ReviewedDecision::Injected && r.expect.is_empty()).count();
    ReviewedMetrics {
        samples: rows.len(),
        eligible,
        unknown,
        skipped,
        answerable,
        no_answer,
        correct,
        wrong,
        silent_positive,
        silent_negative,
        unavailable,
        recall_at_1: at(1),
        recall_at_5: at(5),
        precision: metric(correct, injections),
        false_injection: metric(false_injection, no_answer),
        injection_rate: metric(injections, eligible),
        risk: metric(wrong, injections),
        coverage: metric(injections, eligible),
    }
}

/// Computes reviewed metrics for one backend and, optionally, one event path.
pub fn reviewed_metrics_for(rows: &[ReviewedObservation], backend: Option<&str>, path: Option<EventPath>) -> ReviewedMetrics {
    let selected: Vec<_> = rows
        .iter()
        .filter(|row| backend.is_none_or(|name| row.backend.as_deref() == Some(name)))
        .filter(|row| path.is_none_or(|kind| row.path == kind))
        .cloned()
        .collect();
    reviewed_metrics(&selected)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventPath {
    Failure,
    Prompt,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedEvent {
    pub key: String,
    pub path: EventPath,
    pub text: String,
    pub error: Option<String>,
    pub command: Option<String>,
    pub project: Option<String>,
    pub system: Option<String>,
    pub expect: Vec<String>,
    #[serde(default)]
    pub near_miss: Vec<String>,
    pub source: String,
    pub rationale: String,
    pub worked: Option<String>,
    /// A paraphrase of a lesson symptom in the asker's own words; counted also in its path.
    #[serde(default)]
    pub paraphrase: bool,
}

/// First-stage misses over one group: answerable events and those with no expected lesson
/// in the BM25 candidate set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirstStage {
    pub answerable: usize,
    pub misses: usize,
}

impl FirstStage {
    pub fn rate(self) -> Option<f64> {
        if self.answerable == 0 { None } else { Some(self.misses as f64 / self.answerable as f64) }
    }
}

/// Counts first-stage misses in `rows`: `(answerable, hit)` per event. Only answerable events
/// count; a miss has no expected lesson in the candidate set.
pub fn first_stage(rows: impl IntoIterator<Item = (bool, bool)>) -> FirstStage {
    let mut out = FirstStage { answerable: 0, misses: 0 };
    for (answerable, hit) in rows {
        if answerable {
            out.answerable += 1;
            out.misses += usize::from(!hit);
        }
    }
    out
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewedFile {
    version: u32,
    kb_revision: String,
    source_fingerprint: String,
    event: Vec<ReviewedEvent>,
}

#[derive(Debug, Clone)]
pub struct ReviewedSet {
    pub kb_revision: String,
    pub source_fingerprint: String,
    pub events: Vec<ReviewedEvent>,
}

pub fn parse_reviewed(text: &str, root: &Path) -> Result<ReviewedSet> {
    let file: ReviewedFile = toml::from_str(text).map_err(|e| Error::Refused(format!("invalid reviewed set: {e}")))?;
    if file.version != 1 || file.kb_revision.trim().is_empty() || file.source_fingerprint.trim().is_empty() {
        return Err(Error::Refused("reviewed set needs version = 1, kb_revision and source_fingerprint".into()));
    }
    let snap = Snapshot::from_dir_where(root, |p| classify(p) == FileKind::Lesson)?;
    let (lessons, _) = snap.lessons();
    let active: HashSet<&str> =
        lessons.iter().filter(|l| l.frontmatter.status == crate::lesson::Status::Active).map(|l| l.frontmatter.id.as_str()).collect();
    let mut keys = HashSet::new();
    for event in &file.event {
        if event.key.trim().is_empty() || !keys.insert(event.key.as_str()) {
            return Err(Error::Refused(format!("empty or duplicate event key: {}", event.key)));
        }
        if event.text.trim().is_empty() || event.source.trim().is_empty() || event.rationale.trim().is_empty() {
            return Err(Error::Refused(format!("event {} needs text, source and rationale", event.key)));
        }
        if event.path == EventPath::Prompt && (event.error.is_some() || event.command.is_some()) {
            return Err(Error::Refused(format!("event {}: prompt cannot have error", event.key)));
        }
        if event.path == EventPath::Failure && event.error.is_some() != event.command.is_some() {
            return Err(Error::Refused(format!("event {}: error and command must occur together", event.key)));
        }
        if event.expect.is_empty() && event.worked.is_some()
            || !event.expect.is_empty() && event.worked.as_ref().is_none_or(|s| s.trim().is_empty())
        {
            return Err(Error::Refused(format!("event {}: positives need a worked reference; negatives cannot have one", event.key)));
        }
        for id in event.expect.iter().chain(&event.near_miss) {
            if !active.contains(id.as_str()) {
                return Err(Error::Refused(format!("event {} names unknown or inactive lesson {id}", event.key)));
            }
        }
        if event.near_miss.iter().any(|id| event.expect.contains(id)) {
            return Err(Error::Refused(format!("event {}: a near miss cannot be expected", event.key)));
        }
    }
    Ok(ReviewedSet { kb_revision: file.kb_revision, source_fingerprint: file.source_fingerprint, events: file.event })
}

/// One logged injection for a reviewed-set draft: the failure hook's replay row.
#[derive(Debug, Clone)]
pub struct DraftReplay {
    pub session: String,
    pub query: String,
    pub injected: Option<String>,
    pub project: Option<String>,
    pub system: Option<String>,
    pub time: u64,
}

/// One use report for a draft: the knowledge base's usage record.
#[derive(Debug, Clone)]
pub struct DraftReport {
    pub id: String,
    pub event: String,
    pub session: Option<String>,
    pub time: String,
}

/// Whether a usage record's report came after the replay it answers.
fn later(report: &str, replay: u64) -> bool {
    use std::str::FromStr;
    jiff::Timestamp::from_str(report).map(|t| t.as_second() > replay as i64).unwrap_or(false)
}

/// A reviewed-set draft from the replay log and the use reports: one entry for each logged
/// injection with a later `worked` or `irrelevant` report, labelled from that report and marked
/// `auto`. A `worked` report names the lesson; `irrelevant` means no answer.
pub fn draft_reviewed(replays: &[DraftReplay], reports: &[DraftReport], kb_revision: &str) -> Result<String> {
    use sha2::Digest;
    let mut events = Vec::new();
    for r in replays {
        let Some(id) = r.injected.as_deref() else { continue };
        let Some(rep) = reports.iter().find(|p| {
            p.id == id
                && p.session.as_deref() == Some(r.session.as_str())
                && matches!(p.event.as_str(), "worked" | "irrelevant")
                && later(&p.time, r.time)
        }) else {
            continue;
        };
        let mut e = toml::map::Map::new();
        e.insert("key".into(), toml::Value::String(format!("replay-{}", events.len() + 1)));
        e.insert("path".into(), toml::Value::String("failure".into()));
        e.insert("text".into(), toml::Value::String(r.query.clone()));
        // The logged query starts with the program, so `--reviewed` rebuilds the same failure query.
        let (command, error) = r.query.split_once(' ').unwrap_or((&r.query, ""));
        e.insert("command".into(), toml::Value::String(command.to_string()));
        e.insert("error".into(), toml::Value::String(error.to_string()));
        if let Some(p) = &r.project {
            e.insert("project".into(), toml::Value::String(p.clone()));
        }
        if let Some(s) = &r.system {
            e.insert("system".into(), toml::Value::String(s.clone()));
        }
        if rep.event == "worked" {
            e.insert("expect".into(), toml::Value::Array(vec![toml::Value::String(id.to_string())]));
            e.insert("worked".into(), toml::Value::String(format!("{} in session {}", rep.event, r.session)));
        } else {
            e.insert("expect".into(), toml::Value::Array(vec![]));
        }
        e.insert("source".into(), toml::Value::String(format!("replay {}", r.session)));
        e.insert("rationale".into(), toml::Value::String("auto".into()));
        events.push(toml::Value::Table(e));
    }
    let mut doc = toml::map::Map::new();
    doc.insert("version".into(), toml::Value::Integer(1));
    doc.insert("kb_revision".into(), toml::Value::String(kb_revision.to_string()));
    let input: String = replays.iter().map(|r| format!("{} {}\n", r.session, r.query)).collect();
    let print: String = sha2::Sha256::digest(input.as_bytes()).iter().take(6).map(|b| format!("{b:02x}")).collect();
    doc.insert("source_fingerprint".into(), toml::Value::String(print));
    doc.insert("event".into(), toml::Value::Array(events));
    toml::to_string(&toml::Value::Table(doc)).map_err(|e| Error::Refused(format!("cannot write a reviewed set: {e}")))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub text: String,
    pub expect: Vec<String>,
    #[serde(default)]
    pub mode: QueryMode,
    #[serde(default)]
    pub with: BTreeMap<String, String>,
    pub project: Option<String>,
    pub system: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueryMode {
    #[default]
    Search,
    Literal,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    query: Vec<Query>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub text: String,
    /// The lessons that answer the query; empty when none does.
    pub expect: Vec<String>,
    /// 1-based rank of the first expected id, `None` when not in the first `DEPTH` results.
    pub rank: Option<usize>,
    /// The first two results with their model relevance, when a model ranked them.
    pub top: Vec<(String, Option<f32>)>,
}

impl Row {
    pub fn answerable(&self) -> bool {
        !self.expect.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub rows: Vec<Row>,
    pub recall_at_5: f64,
    pub mrr: f64,
}

/// What the recall rule of `rkb hook prompt` would do with a query's results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recall {
    /// It adds a lesson the query expects.
    Right,
    /// It adds a lesson the query does not expect, or any lesson for a query with no answer.
    Wrong,
    Silent,
}

impl Recall {
    pub fn as_str(self) -> &'static str {
        match self {
            Recall::Right => "right",
            Recall::Wrong => "wrong",
            Recall::Silent => "silent",
        }
    }
}

/// The recall verdict for `row` at `min` and `margin`; `None` when no model ranked it.
pub fn recall(row: &Row, min: f64, margin: f64) -> Option<Recall> {
    let score = |i: usize| row.top.get(i).and_then(|t| t.1).map(f64::from);
    let top = score(0)?;
    let next = score(1).unwrap_or(0.0);
    if !crate::hooks::clear_winner(top, next, min, margin) {
        return Some(Recall::Silent);
    }
    Some(if row.expect.contains(&row.top[0].0) { Recall::Right } else { Recall::Wrong })
}

/// Counts of `right`, `wrong` and `silent` over the rows a model ranked (literal queries are never
/// reranked, so they have no verdict); `None` when no row has one.
pub fn tally(rows: &[Row], min: f64, margin: f64) -> Option<[usize; 3]> {
    let verdicts: Vec<Recall> = rows.iter().filter_map(|r| recall(r, min, margin)).collect();
    if verdicts.is_empty() {
        return None;
    }
    let count = |v: Recall| verdicts.iter().filter(|x| **x == v).count();
    Some([count(Recall::Right), count(Recall::Wrong), count(Recall::Silent)])
}

/// The grid of `--recall-sweep`: minimum relevance and margin.
pub const SWEEP_MIN: [f64; 6] = [0.70, 0.75, 0.80, 0.85, 0.90, 0.95];
pub const SWEEP_MARGIN: [f64; 4] = [0.0, 0.05, 0.10, 0.15];

/// The sweep point with the most coverage at `precision`, if any. A row without that point in its
/// curve does not decide there. Ties keep the lowest threshold, then the lowest margin.
pub fn pick(rows: &[ReviewedObservation], precision: f64) -> Option<(f64, f64)> {
    let mut best: Option<(f64, f64, f64)> = None;
    for min in SWEEP_MIN {
        for margin in SWEEP_MARGIN {
            let decided: Vec<ReviewedObservation> = rows
                .iter()
                .map(|r| {
                    let mut r = r.clone();
                    r.decision = r
                        .curve
                        .iter()
                        .find(|(a, b, _)| a.to_bits() == min.to_bits() && b.to_bits() == margin.to_bits())
                        .map_or(ReviewedDecision::Unavailable, |(_, _, decision)| *decision);
                    r
                })
                .collect();
            let m = reviewed_metrics(&decided);
            let (Some(p), Some(c)) = (m.precision.value, m.coverage.value) else { continue };
            if p >= precision && best.is_none_or(|(_, _, b)| c > b) {
                best = Some((min, margin, c));
            }
        }
    }
    best.map(|(min, margin, _)| (min, margin))
}

pub fn parse(text: &str) -> std::result::Result<Vec<Query>, String> {
    toml::from_str::<File>(text).map(|f| f.query).map_err(|e| e.message().to_string())
}

/// For `rkb eval --self`: one query per current lesson with a Symptom or Statement section, the first line of it,
/// expecting that lesson, from the lesson's own project or system.
pub fn self_queries(root: &Path) -> Result<Vec<Query>> {
    let snap = Snapshot::from_dir_where(root, |p| classify(p) == FileKind::Lesson)?;
    let (mut lessons, _) = snap.lessons();
    lessons.sort_by(|a, b| a.path.cmp(&b.path));
    let scope = |prefix: &str, path: &str| path.strip_prefix(prefix).and_then(|p| p.split('/').next()).map(str::to_string);
    Ok(lessons
        .iter()
        .filter(|l| l.frontmatter.status.is_current())
        .filter_map(|l| {
            let section = ["Symptom", "Statement"].iter().find_map(|h| crate::body::section_text(&l.body, h))?;
            let text = section.lines().map(str::trim).find(|t| !t.is_empty() && !t.starts_with("```"))?.to_string();
            Some(Query {
                text,
                expect: vec![l.frontmatter.id.clone()],
                mode: QueryMode::Search,
                with: BTreeMap::new(),
                project: scope("projects/", &l.path),
                system: scope("systems/", &l.path),
            })
        })
        .collect())
}

/// Recall at 5 and mean reciprocal rank over the rows that have an answer.
pub fn score(rows: &[Row]) -> (f64, f64) {
    let rows: Vec<&Row> = rows.iter().filter(|r| r.answerable()).collect();
    let n = rows.len().max(1) as f64;
    let recall = rows.iter().filter(|r| r.rank.is_some_and(|k| k <= 5)).count() as f64 / n;
    let mrr = rows.iter().map(|r| r.rank.map_or(0.0, |k| 1.0 / k as f64)).sum::<f64>() / n;
    (recall, mrr)
}

/// Runs every query against the knowledge base at `root`. `rerank` reorders the results of a ranked
/// query, which are searched `depth` deep and then cut to the first 10.
pub fn run(root: &Path, queries: &[Query], depth: usize, rerank: &mut dyn FnMut(&str, &mut Results) -> Result<()>) -> Result<Report> {
    let snap = Snapshot::from_dir_where(root, |p| classify(p) == FileKind::Lesson)?;
    let (lessons, _) = snap.lessons();
    let ids: HashSet<&str> = lessons.iter().map(|l| l.frontmatter.id.as_str()).collect();
    for q in queries {
        if let Some(bad) = q.expect.iter().find(|id| !ids.contains(id.as_str())) {
            return Err(Error::Refused(format!("query \"{}\" expects {bad}, and no lesson has that id", q.text)));
        }
    }
    let mut rows = vec![];
    for q in queries {
        let matched = |name: &Option<String>| name.as_ref().map(|n| Matched { name: n.clone(), rule: Rule::Flag });
        let place = Place { project: matched(&q.project), system: matched(&q.system), repo: None, dir: None };
        let with: Vec<(String, String)> = q.with.clone().into_iter().collect();
        let facts = Facts::gather(root, &place, &with);
        let mode = match q.mode {
            QueryMode::Search => Mode::Ranked(q.text.clone()),
            QueryMode::Literal => Mode::Literal(q.text.clone()),
        };
        let mut res = search(
            root,
            &place,
            &facts,
            &mode,
            &Options { all: false, every_status: false, limit: DEPTH.max(depth), probes: crate::search::ProbeMode::Off, tier: None },
        )?;
        if let Mode::Ranked(text) = &mode {
            rerank(text, &mut res)?;
        }
        res.hits.truncate(DEPTH);
        let rank = res.hits.iter().position(|h| q.expect.contains(&h.id)).map(|i| i + 1);
        let top = res.hits.iter().take(2).map(|h| (h.id.clone(), h.relevance)).collect();
        rows.push(Row { text: q.text.clone(), expect: q.expect.clone(), rank, top });
    }
    let (recall_at_5, mrr) = score(&rows);
    Ok(Report { rows, recall_at_5, mrr })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(expect: &[&str], rank: Option<usize>, top: &[(&str, Option<f32>)]) -> Row {
        Row {
            text: String::new(),
            expect: expect.iter().map(|s| s.to_string()).collect(),
            rank,
            top: top.iter().map(|(i, r)| (i.to_string(), *r)).collect(),
        }
    }

    #[test]
    fn scores() {
        let mut rows: Vec<Row> = [Some(1), Some(3), None].into_iter().map(|rank| row(&["a"], rank, &[])).collect();
        rows.push(row(&[], None, &[]));
        let (recall, mrr) = score(&rows);
        assert!((recall - 2.0 / 3.0).abs() < 1e-9, "a query with no answer does not count");
        assert!((mrr - (1.0 + 1.0 / 3.0) / 3.0).abs() < 1e-9);
    }

    #[test]
    fn pick_meets_the_precision() {
        fn judged(top: &str, expect: &[&str], f: impl Fn(f64, f64) -> ReviewedDecision + Copy) -> ReviewedObservation {
            ReviewedObservation {
                path: EventPath::Failure,
                expect: expect.iter().map(|s| s.to_string()).collect(),
                top: vec![top.to_string()],
                scores: vec![],
                decision: ReviewedDecision::Silent,
                backend: Some("jev".into()),
                ranked_available: true,
                curve: SWEEP_MIN
                    .into_iter()
                    .flat_map(|min| SWEEP_MARGIN.into_iter().map(move |margin| (min, margin, f(min, margin))))
                    .collect(),
            }
        }
        let rows = vec![
            judged("a", &["a"], |min, _| if min <= 0.9 { ReviewedDecision::Injected } else { ReviewedDecision::Silent }),
            judged("a", &["a"], |min, _| if min <= 0.8 { ReviewedDecision::Injected } else { ReviewedDecision::Silent }),
            judged(
                "a",
                &["b"],
                |min, margin| {
                    if min <= 0.85 && margin <= 0.0 { ReviewedDecision::Injected } else { ReviewedDecision::Silent }
                },
            ),
            judged("a", &["b"], |min, _| if min <= 0.75 { ReviewedDecision::Injected } else { ReviewedDecision::Silent }),
        ];
        assert_eq!(pick(&rows, 0.95), Some((0.8, 0.05)));
        assert_eq!(pick(&rows, 0.5), Some((0.7, 0.0)), "lower precision keeps the widest net");
        let wrong = vec![judged("a", &["b"], |_, _| ReviewedDecision::Injected)];
        assert_eq!(pick(&wrong, 0.95), None, "a wrong injection never reaches the precision");
        assert_eq!(pick(&[], 0.95), None);
    }

    #[test]
    fn recall_counts() {
        let rows = [
            row(&["a"], Some(1), &[("a", Some(0.90)), ("b", Some(0.40))]),
            row(&[], None, &[("c", Some(0.88)), ("d", Some(0.30))]),
            row(&["e"], Some(2), &[("f", Some(0.60)), ("e", Some(0.50))]),
        ];
        assert_eq!(tally(&rows, 0.85, 0.05), Some([1, 1, 1]));
        assert_eq!(recall(&rows[0], 0.95, 0.05), Some(Recall::Silent));
        assert_eq!(tally(&[row(&["a"], Some(1), &[("a", None)])], 0.85, 0.05), None, "BM25 has no relevance");
        let mut with_literal = rows.to_vec();
        with_literal.push(row(&["a"], Some(1), &[("a", None)]));
        assert_eq!(tally(&with_literal, 0.85, 0.05), Some([1, 1, 1]), "a literal query has no verdict and is skipped");
    }

    #[test]
    fn reviewed_validation() {
        let root = tempfile::tempdir().unwrap();
        let head = "version = 1\nkb_revision = 'commit'\nsource_fingerprint = 'hash'\n";
        let row = "\n[[event]]\nkey = 'one'\npath = 'failure'\ntext = 'cmake error'\nsource = 'local replay line 1'\nrationale = 'checked no active lesson answers this'\nexpect = []\n";
        assert_eq!(parse_reviewed(&(head.to_owned() + row), root.path()).unwrap().events.len(), 1);
        assert!(parse_reviewed(&(head.to_owned() + &row.replace("source = 'local replay line 1'\n", "")), root.path()).is_err());
        assert!(parse_reviewed(&(head.to_owned() + row + row), root.path()).unwrap_err().to_string().contains("duplicate"));
        let unknown = row.replace("expect = []", "expect = ['0000000000']\nworked = 'usage line 1'");
        assert!(parse_reviewed(&(head.to_owned() + &unknown), root.path()).unwrap_err().to_string().contains("unknown"));
        let unproved = row.replace("expect = []", "expect = ['0000000000']");
        assert!(parse_reviewed(&(head.to_owned() + &unproved), root.path()).unwrap_err().to_string().contains("worked"));
    }

    #[test]
    fn reviewed_metrics_keep_denominators_and_zero_rates_undefined() {
        let rows = vec![
            ReviewedObservation {
                path: EventPath::Failure,
                expect: vec!["a".into()],
                top: vec!["a".into()],
                scores: vec![Some(0.9)],
                decision: ReviewedDecision::Injected,
                backend: Some("model".into()),
                ranked_available: true,
                curve: vec![],
            },
            ReviewedObservation {
                path: EventPath::Prompt,
                expect: vec!["a".into()],
                top: vec!["b".into()],
                scores: vec![Some(0.9)],
                decision: ReviewedDecision::Injected,
                backend: Some("model".into()),
                ranked_available: true,
                curve: vec![],
            },
            ReviewedObservation {
                path: EventPath::Prompt,
                expect: vec![],
                top: vec!["b".into()],
                scores: vec![Some(0.9)],
                decision: ReviewedDecision::Injected,
                backend: Some("model".into()),
                ranked_available: true,
                curve: vec![],
            },
            ReviewedObservation {
                path: EventPath::Prompt,
                expect: vec![],
                top: vec![],
                scores: vec![],
                decision: ReviewedDecision::Silent,
                backend: None,
                ranked_available: true,
                curve: vec![],
            },
            ReviewedObservation {
                path: EventPath::Failure,
                expect: vec!["a".into()],
                top: vec![],
                scores: vec![],
                decision: ReviewedDecision::Unavailable,
                backend: None,
                ranked_available: true,
                curve: vec![],
            },
        ];
        let m = reviewed_metrics(&rows);
        assert_eq!((m.samples, m.eligible, m.unknown, m.skipped), (5, 4, 1, 0));
        assert_eq!((m.answerable, m.no_answer), (3, 2));
        assert_eq!((m.correct, m.wrong, m.silent_positive, m.silent_negative, m.unavailable), (1, 2, 0, 1, 1));
        assert_eq!((m.precision.numerator, m.precision.denominator), (1, 3));
        assert_eq!((m.false_injection.numerator, m.false_injection.denominator), (1, 2));
        assert_eq!((m.injection_rate.numerator, m.injection_rate.denominator), (3, 4));
        assert_eq!(m.recall_at_1.value, Some(1.0 / 3.0));
        assert_eq!(metric(0, 0).value, None);
    }

    #[test]
    fn reviewed_synthetic_positive_negative_near_miss_and_missing_context() {
        let rows = vec![
            ReviewedObservation {
                path: EventPath::Failure,
                expect: vec!["answer".into()],
                top: vec!["answer".into()],
                scores: vec![Some(0.9)],
                decision: ReviewedDecision::Injected,
                backend: Some("jev".into()),
                ranked_available: true,
                curve: vec![(0.55, 0.08, ReviewedDecision::Injected)],
            },
            ReviewedObservation {
                path: EventPath::Prompt,
                expect: vec![],
                top: vec!["near-miss".into()],
                scores: vec![Some(0.9)],
                decision: ReviewedDecision::Injected,
                backend: Some("jev".into()),
                ranked_available: true,
                curve: vec![(0.55, 0.08, ReviewedDecision::Injected)],
            },
            ReviewedObservation {
                path: EventPath::Prompt,
                expect: vec![],
                top: vec![],
                scores: vec![],
                decision: ReviewedDecision::Silent,
                backend: None,
                ranked_available: true,
                curve: vec![],
            },
            ReviewedObservation {
                path: EventPath::Failure,
                expect: vec!["answer".into()],
                top: vec!["answer".into()],
                scores: vec![Some(0.9)],
                decision: ReviewedDecision::Unavailable,
                backend: Some("jev".into()),
                ranked_available: true,
                curve: vec![(0.55, 0.08, ReviewedDecision::Unavailable)],
            },
        ];
        let m = reviewed_metrics(&rows);
        assert_eq!((m.recall_at_1.numerator, m.recall_at_1.denominator), (2, 2));
        assert_eq!((m.false_injection.numerator, m.false_injection.denominator), (1, 2));
        assert_eq!((m.injection_rate.numerator, m.injection_rate.denominator), (2, 3));
        assert_eq!((m.eligible, m.unknown, m.skipped), (3, 1, 0));
        assert_eq!(rows[3].curve[0].2, ReviewedDecision::Unavailable);
    }

    #[test]
    fn mixed_backend_metrics_stay_separate() {
        let rows = vec![
            ReviewedObservation {
                path: EventPath::Failure,
                expect: vec!["answer".into()],
                top: vec!["answer".into()],
                scores: vec![],
                decision: ReviewedDecision::Injected,
                backend: Some("bm25".into()),
                ranked_available: true,
                curve: vec![],
            },
            ReviewedObservation {
                path: EventPath::Failure,
                expect: vec!["answer".into()],
                top: vec!["wrong".into()],
                scores: vec![],
                decision: ReviewedDecision::Injected,
                backend: Some("jev".into()),
                ranked_available: true,
                curve: vec![],
            },
        ];
        let bm25 = reviewed_metrics_for(&rows, Some("bm25"), None);
        assert_eq!((bm25.samples, bm25.correct, bm25.wrong), (1, 1, 0));
        assert_eq!(reviewed_metrics_for(&rows, Some("jev"), None).wrong, 1);
    }

    #[test]
    fn skipped_rows_are_not_eligible_but_unavailable_ranked_rows_keep_recall() {
        let rows = vec![
            ReviewedObservation {
                path: EventPath::Prompt,
                expect: vec!["answer".into()],
                top: vec!["answer".into()],
                scores: vec![],
                decision: ReviewedDecision::Silent,
                backend: None,
                ranked_available: false,
                curve: vec![],
            },
            ReviewedObservation {
                path: EventPath::Failure,
                expect: vec!["answer".into()],
                top: vec!["answer".into()],
                scores: vec![],
                decision: ReviewedDecision::Unavailable,
                backend: Some("jev".into()),
                ranked_available: true,
                curve: vec![],
            },
        ];
        let m = reviewed_metrics(&rows);
        assert_eq!((m.eligible, m.unknown, m.skipped), (0, 1, 1));
        assert_eq!((m.recall_at_1.numerator, m.recall_at_1.denominator), (1, 1));
        assert_eq!(m.injection_rate.value, None);
        assert_eq!(m.false_injection.value, None);
    }

    #[test]
    fn file_format() {
        let q =
            parse("[[query]]\ntext = \"a\"\nexpect = [\"0000000001\"]\nmode = \"literal\"\nwith = { hdf5 = \"1.14.3\" }\nsystem = \"t\"\n")
                .unwrap();
        assert_eq!((q[0].mode, q[0].with["hdf5"].as_str(), q[0].system.as_deref()), (QueryMode::Literal, "1.14.3", Some("t")));
        assert!(parse("[[query]]\ntext = \"a\"\nexpect = []\nexpected = 1\n").unwrap_err().contains("expected"));
    }
}
