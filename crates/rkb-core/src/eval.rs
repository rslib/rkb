use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::Deserialize;

use crate::conditions::Facts;
use crate::error::{Error, Result};
use crate::kb::{FileKind, Snapshot, classify};
use crate::matching::{Matched, Place, Rule};
use crate::search::{Mode, Options, Results, search};

pub const DEPTH: usize = 10;

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

pub fn parse(text: &str) -> std::result::Result<Vec<Query>, String> {
    toml::from_str::<File>(text).map(|f| f.query).map_err(|e| e.message().to_string())
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
            &Options { all: false, every_status: false, limit: DEPTH.max(depth), probes: crate::search::ProbeMode::Off },
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
    fn file_format() {
        let q =
            parse("[[query]]\ntext = \"a\"\nexpect = [\"0000000001\"]\nmode = \"literal\"\nwith = { hdf5 = \"1.14.3\" }\nsystem = \"t\"\n")
                .unwrap();
        assert_eq!((q[0].mode, q[0].with["hdf5"].as_str(), q[0].system.as_deref()), (QueryMode::Literal, "1.14.3", Some("t")));
        assert!(parse("[[query]]\ntext = \"a\"\nexpect = []\nexpected = 1\n").unwrap_err().contains("expected"));
    }
}
