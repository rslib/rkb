//! Jev's verdicts on pairs of likely duplicate lessons, kept in `dupes.json` in the state folder.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::graph::{self, Graph};
use crate::lesson::Lesson;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Verdict {
    pub hash: String,
    pub verdict: String,
    pub p: f64,
    pub different: f64,
}

pub type Cache = BTreeMap<String, Verdict>;

/// What Jev reads of a lesson.
pub fn text(l: &Lesson) -> String {
    format!("{}\n{}", graph::title(l), l.body.trim())
}

pub fn key(a: &Lesson, b: &Lesson) -> String {
    let (x, y) = (&a.frontmatter.id, &b.frontmatter.id);
    if x <= y { format!("{x}:{y}") } else { format!("{y}:{x}") }
}

pub fn hash(a: &Lesson, b: &Lesson) -> String {
    let (x, y) = (text(a), text(b));
    let (x, y) = if a.frontmatter.id <= b.frontmatter.id { (x, y) } else { (y, x) };
    Sha256::digest(format!("{x}\0{y}")).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

pub fn load(state: &Path) -> Cache {
    std::fs::read_to_string(state.join("dupes.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save(state: &Path, cache: &Cache) -> std::io::Result<()> {
    std::fs::create_dir_all(state)?;
    let tmp = state.join(".dupes.json.tmp");
    std::fs::write(&tmp, serde_json::to_string(cache).unwrap_or_default())?;
    std::fs::rename(&tmp, state.join("dupes.json"))
}

/// The cached verdict of a pair, when both lessons still read as they did.
pub fn get<'a>(cache: &'a Cache, a: &Lesson, b: &Lesson) -> Option<&'a Verdict> {
    cache.get(&key(a, b)).filter(|v| v.hash == hash(a, b))
}

/// Pairs of `g` at or above `min` that Jev did not rate `different` at `keep` or more.
pub fn shown(g: &Graph, min: f64, cache: &Cache, keep: f64) -> Vec<(usize, usize, f64)> {
    let mut pairs = g.dupes(min);
    pairs.retain(|&(a, b, _)| get(cache, &g.lessons[a], &g.lessons[b]).is_none_or(|v| v.different < keep));
    pairs
}
