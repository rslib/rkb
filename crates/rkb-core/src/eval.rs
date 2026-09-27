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
    /// 1-based rank of the first expected id, `None` when not in the first `DEPTH` results.
    pub rank: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub rows: Vec<Row>,
    pub recall_at_5: f64,
    pub mrr: f64,
}

pub fn parse(text: &str) -> std::result::Result<Vec<Query>, String> {
    toml::from_str::<File>(text).map(|f| f.query).map_err(|e| e.message().to_string())
}

/// Recall at 5 and mean reciprocal rank over `rows`.
pub fn score(rows: &[Row]) -> (f64, f64) {
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
        let place = Place { project: matched(&q.project), system: matched(&q.system), repo: None };
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
        rows.push(Row { text: q.text.clone(), rank });
    }
    let (recall_at_5, mrr) = score(&rows);
    Ok(Report { rows, recall_at_5, mrr })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores() {
        let rows = [Some(1), Some(3), None].map(|rank| Row { text: String::new(), rank });
        let (recall, mrr) = score(&rows);
        assert!((recall - 2.0 / 3.0).abs() < 1e-9);
        assert!((mrr - (1.0 + 1.0 / 3.0) / 3.0).abs() < 1e-9);
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
