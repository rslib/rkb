//! Similarity between lessons and the graph of how lessons connect: links, supersedes, similar words, shared tags.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use crate::error::Result;
use crate::kb::{FileKind, Snapshot, classify};
use crate::lesson::{Lesson, Status};
use crate::search::parallel;
use crate::text::each_token;

/// Pairs at or above this are listed by `rkb dupes`, warned about by lint and named by `rkb add`.
pub const DEFAULT_MIN_SIMILARITY: f64 = 0.4;
/// Below this, a similarity is not worth listing as an edge.
pub const RELATED_MIN: f64 = 0.2;
/// Terms kept per lesson. A duplicate pair shares its rarest terms, which weigh the most.
const TOP_TERMS: usize = 64;
const TITLE_WEIGHT: f64 = 3.0;
const TAG_WEIGHT: f64 = 2.5;

pub fn min_similarity(root: &Path) -> f64 {
    let kb = std::fs::read_to_string(root.join("kb.toml")).ok().and_then(|t| crate::config::parse::<crate::config::KbConfig>(&t).ok());
    min_from(kb.as_ref())
}

pub fn min_from(kb: Option<&crate::config::KbConfig>) -> f64 {
    kb.and_then(|c| c.dupes.as_ref())
        .and_then(|t| t.get("min_similarity").and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))))
        .unwrap_or(DEFAULT_MIN_SIMILARITY)
}

/// Whether a lesson takes part in similarity.
pub fn compared(l: &Lesson) -> bool {
    matches!(l.frontmatter.status, Status::Active | Status::Stale)
}

pub fn title(l: &Lesson) -> String {
    crate::body::scan(&l.body).h1.into_iter().next().map(|(_, t)| t).unwrap_or_else(|| l.path.clone())
}

/// Unit-length TF-IDF vectors of a set of lessons, as sorted `(term, weight)` lists.
pub struct Vectors {
    vecs: Vec<Vec<(u32, f64)>>,
}

impl Vectors {
    pub fn new(lessons: &[&Lesson]) -> Vectors {
        let counts: Vec<HashMap<String, f64>> = parallel(lessons, |l| {
            let mut tf: HashMap<String, f64> = HashMap::new();
            let mut add = |text: &str, w: f64| {
                each_token(text, &mut |t| match tf.get_mut(t) {
                    Some(x) => *x += w,
                    None => {
                        tf.insert(t.to_string(), w);
                    }
                })
            };
            add(&title(l), TITLE_WEIGHT);
            add(&l.frontmatter.tags.join(" "), TAG_WEIGHT);
            add(&l.body, 1.0);
            tf
        });
        let mut ids: HashMap<&str, u32> = HashMap::new();
        let mut df: Vec<f64> = vec![];
        for c in &counts {
            for t in c.keys() {
                let n = ids.len() as u32;
                let i = *ids.entry(t).or_insert(n);
                if i as usize == df.len() {
                    df.push(0.0);
                }
                df[i as usize] += 1.0;
            }
        }
        let n = counts.len() as f64;
        let vecs = counts
            .iter()
            .map(|c| {
                let mut v: Vec<(u32, f64)> = c
                    .iter()
                    .map(|(t, tf)| {
                        let i = ids[t.as_str()];
                        (i, (1.0 + tf.ln()) * ((1.0 + n) / (1.0 + df[i as usize])).ln())
                    })
                    .filter(|(_, w)| *w > 0.0)
                    .collect();
                v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                v.truncate(TOP_TERMS);
                let norm = v.iter().map(|(_, w)| w * w).sum::<f64>().sqrt();
                v.iter_mut().for_each(|(_, w)| *w /= norm.max(f64::MIN_POSITIVE));
                v.sort_by_key(|(t, _)| *t);
                v
            })
            .collect();
        Vectors { vecs }
    }

    pub fn similarity(&self, a: usize, b: usize) -> f64 {
        let (x, y) = (&self.vecs[a], &self.vecs[b]);
        let (mut i, mut j, mut s) = (0, 0, 0.0);
        while i < x.len() && j < y.len() {
            match x[i].0.cmp(&y[j].0) {
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
                std::cmp::Ordering::Equal => {
                    s += x[i].1 * y[j].1;
                    i += 1;
                    j += 1;
                }
            }
        }
        s.min(1.0)
    }

    /// Pairs `(a, b, similarity)` with `a` in `from`, `a != b`, at or above `min`, most similar first.
    /// A pair with both ends in `from` is given once, with `a < b`.
    pub fn pairs(&self, from: &[usize], min: f64) -> Vec<(usize, usize, f64)> {
        let mut postings: HashMap<u32, Vec<(usize, f64)>> = HashMap::new();
        for (d, v) in self.vecs.iter().enumerate() {
            for &(t, w) in v {
                postings.entry(t).or_default().push((d, w));
            }
        }
        let mut in_from = vec![false; self.vecs.len()];
        from.iter().for_each(|&a| in_from[a] = true);
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(from.len().max(1));
        let chunks: Vec<&[usize]> = from.chunks(from.len().div_ceil(threads).max(1)).collect();
        let mut out: Vec<(usize, usize, f64)> = parallel(&chunks, |chunk| {
            let mut acc = vec![0.0f64; self.vecs.len()];
            let mut touched = vec![];
            let mut found = vec![];
            for &a in *chunk {
                for (t, wa) in &self.vecs[a] {
                    for &(b, wb) in &postings[t] {
                        if b == a || (in_from[b] && b < a) {
                            continue;
                        }
                        if acc[b] == 0.0 {
                            touched.push(b);
                        }
                        acc[b] += wa * wb;
                    }
                }
                for b in touched.drain(..) {
                    if acc[b] >= min {
                        found.push((a, b, acc[b].min(1.0)));
                    }
                    acc[b] = 0.0;
                }
            }
            found
        })
        .into_iter()
        .flatten()
        .collect();
        out.sort_by(|x, y| y.2.total_cmp(&x.2).then((x.0, x.1).cmp(&(y.0, y.1))));
        out
    }
}

/// The lessons of the knowledge base at `root`, sorted by path.
pub fn lessons(root: &Path) -> Result<Vec<Lesson>> {
    let snap = Snapshot::from_dir_where(root, |p| classify(p) == FileKind::Lesson)?;
    let (mut lessons, _) = snap.lessons();
    lessons.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(lessons)
}

/// One kind of connection between two lessons.
#[derive(Debug, Clone, PartialEq)]
pub enum Edge {
    Link,
    Supersedes,
    Similar(f64),
    /// Shared tags and `when` keys.
    Shares(Vec<String>),
}

impl Edge {
    pub fn name(&self) -> &'static str {
        match self {
            Edge::Link => "link",
            Edge::Supersedes => "supersedes",
            Edge::Similar(_) => "similar",
            Edge::Shares(_) => "shares",
        }
    }
}

/// Every lesson and the directed `link` and `supersedes` edges and the `similar` pairs at or above `RELATED_MIN`.
pub struct Graph {
    pub lessons: Vec<Lesson>,
    /// `(from, to)`: `from` has a Markdown link to `to`.
    pub links: Vec<(usize, usize)>,
    /// `(old, new)`: `old` is superseded by `new`.
    pub supersedes: Vec<(usize, usize)>,
    pub similar: Vec<(usize, usize, f64)>,
}

pub struct Related {
    pub lesson: usize,
    pub edges: Vec<Edge>,
}

impl Graph {
    pub fn load(root: &Path) -> Result<Graph> {
        Ok(Graph::new(lessons(root)?))
    }

    pub fn new(lessons: Vec<Lesson>) -> Graph {
        let by_path: HashMap<&str, usize> = lessons.iter().enumerate().map(|(i, l)| (l.path.as_str(), i)).collect();
        let by_id: HashMap<&str, usize> = lessons.iter().enumerate().map(|(i, l)| (l.frontmatter.id.as_str(), i)).collect();
        let mut links = BTreeSet::new();
        for (i, l) in lessons.iter().enumerate() {
            for link in crate::body::scan(&l.body).links.iter().filter(|k| !k.image) {
                if let Some(Some(target)) = crate::lint::resolve(&l.path, &link.target)
                    && let Some(&j) = by_path.get(target.as_str())
                    && j != i
                {
                    links.insert((i, j));
                }
            }
        }
        let supersedes = lessons
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.frontmatter.superseded_by.as_deref().and_then(|id| by_id.get(id)).map(|&j| (i, j)))
            .collect();
        let set: Vec<usize> = (0..lessons.len()).filter(|&i| compared(&lessons[i])).collect();
        let refs: Vec<&Lesson> = set.iter().map(|&i| &lessons[i]).collect();
        let every: Vec<usize> = (0..set.len()).collect();
        let similar = Vectors::new(&refs).pairs(&every, RELATED_MIN).into_iter().map(|(a, b, s)| (set[a], set[b], s)).collect();
        Graph { lessons, links: links.into_iter().collect(), supersedes, similar }
    }

    pub fn find(&self, id: &str) -> Option<usize> {
        self.lessons.iter().position(|l| l.frontmatter.id == id)
    }

    /// Pairs at or above `min`, most similar first.
    pub fn dupes(&self, min: f64) -> Vec<(usize, usize, f64)> {
        self.similar.iter().copied().filter(|p| p.2 >= min).collect()
    }

    /// Lessons connected to `i`: links and supersedes first, then by similarity, then by what they share.
    pub fn related(&self, i: usize) -> Vec<Related> {
        let mut edges: BTreeMap<usize, Vec<Edge>> = BTreeMap::new();
        let mut add = |j: usize, e: Edge| {
            let list = edges.entry(j).or_default();
            if !list.contains(&e) {
                list.push(e);
            }
        };
        for &(a, b) in &self.links {
            if a == i || b == i {
                add(a + b - i, Edge::Link);
            }
        }
        for &(a, b) in &self.supersedes {
            if a == i || b == i {
                add(a + b - i, Edge::Supersedes);
            }
        }
        for &(a, b, s) in &self.similar {
            if a == i || b == i {
                add(a + b - i, Edge::Similar(s));
            }
        }
        let mine = keys(&self.lessons[i]);
        for (j, l) in self.lessons.iter().enumerate() {
            if j != i && compared(l) {
                let shared: Vec<String> = keys(l).intersection(&mine).cloned().collect();
                if !shared.is_empty() {
                    add(j, Edge::Shares(shared));
                }
            }
        }
        let mut out: Vec<Related> = edges.into_iter().map(|(lesson, edges)| Related { lesson, edges }).collect();
        let rank = |r: &Related| {
            let strong = r.edges.iter().any(|e| matches!(e, Edge::Link | Edge::Supersedes));
            let sim = r.edges.iter().find_map(|e| if let Edge::Similar(s) = e { Some(*s) } else { None }).unwrap_or(0.0);
            let shared = r.edges.iter().find_map(|e| if let Edge::Shares(k) = e { Some(k.len()) } else { None }).unwrap_or(0);
            (strong, sim, shared)
        };
        out.sort_by(|a, b| {
            let (x, y) = (rank(a), rank(b));
            y.0.cmp(&x.0).then(y.1.total_cmp(&x.1)).then(y.2.cmp(&x.2)).then(self.lessons[a.lesson].path.cmp(&self.lessons[b.lesson].path))
        });
        out
    }

    /// Groups of two or more lessons joined by links, supersedes or similarity at or above `min`, largest first.
    pub fn clusters(&self, min: f64) -> Vec<Vec<usize>> {
        let mut parent: Vec<usize> = (0..self.lessons.len()).collect();
        fn root(p: &mut [usize], mut x: usize) -> usize {
            while p[x] != x {
                p[x] = p[p[x]];
                x = p[x];
            }
            x
        }
        let joins = self.links.iter().chain(&self.supersedes).copied().chain(self.dupes(min).into_iter().map(|(a, b, _)| (a, b)));
        for (a, b) in joins {
            let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
            parent[ra.max(rb)] = ra.min(rb);
        }
        let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for i in 0..self.lessons.len() {
            let r = root(&mut parent, i);
            groups.entry(r).or_default().push(i);
        }
        let mut out: Vec<Vec<usize>> = groups.into_values().filter(|g| g.len() > 1).collect();
        out.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
        out
    }

    /// Active and stale lessons with no link, supersedes or similar edge.
    pub fn orphans(&self) -> Vec<usize> {
        let mut touched = vec![false; self.lessons.len()];
        for (a, b) in self.links.iter().chain(&self.supersedes).copied().chain(self.similar.iter().map(|p| (p.0, p.1))) {
            touched[a] = true;
            touched[b] = true;
        }
        (0..self.lessons.len()).filter(|&i| !touched[i] && compared(&self.lessons[i])).collect()
    }
}

/// Tags as `tag:x` and `when` keys as `when:x`, so the two never collide.
fn keys(l: &Lesson) -> BTreeSet<String> {
    let tags = l.frontmatter.tags.iter().map(|t| format!("tag:{t}"));
    let when = l.frontmatter.when.keys().filter_map(|k| k.as_str()).map(|k| format!("when:{k}"));
    tags.chain(when).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<Lesson> {
        let dir = tempfile::tempdir().unwrap();
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
        copy(&base.join("search/kb"), dir.path());
        copy(&base.join("graph/kb"), dir.path());
        lessons(dir.path()).unwrap()
    }

    fn copy(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).unwrap();
        for e in std::fs::read_dir(src).unwrap() {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() {
                copy(&e.path(), &dst.join(e.file_name()));
            } else {
                std::fs::copy(e.path(), dst.join(e.file_name())).unwrap();
            }
        }
    }

    fn sim(all: &[Lesson], a: &str, b: &str) -> f64 {
        let set: Vec<&Lesson> = all.iter().filter(|l| compared(l)).collect();
        let at = |id: &str| set.iter().position(|l| l.frontmatter.id == id).unwrap();
        let v = Vectors::new(&set);
        let s = v.similarity(at(a), at(b));
        assert!((s - v.similarity(at(b), at(a))).abs() < 1e-12, "symmetric");
        s
    }

    #[test]
    fn duplicate_pair_is_close_and_other_lessons_are_not() {
        let all = fixture();
        assert!(sim(&all, "1a00000001", "1b00000001") >= DEFAULT_MIN_SIMILARITY);
        assert!(sim(&all, "1a00000003", "1a00000005") < DEFAULT_MIN_SIMILARITY);
        let set: Vec<&Lesson> = all.iter().filter(|l| compared(l)).collect();
        let v = Vectors::new(&set);
        let every: Vec<usize> = (0..set.len()).collect();
        let pairs = v.pairs(&every, 0.0);
        let ids = |p: &(usize, usize, f64)| (set[p.0].frontmatter.id.as_str(), set[p.1].frontmatter.id.as_str());
        assert_eq!(ids(&pairs[0]), ("1b00000001", "1a00000001"), "the duplicate pair is the closest");
        for p in &pairs[1..] {
            assert!(p.2 < DEFAULT_MIN_SIMILARITY - 0.1, "{:?} {} leaves no margin", ids(p), p.2);
        }
        assert!(pairs[0].2 > DEFAULT_MIN_SIMILARITY + 0.1, "{} leaves no margin", pairs[0].2);
        assert_eq!(pairs, v.pairs(&every, 0.0), "deterministic");
    }

    #[test]
    fn pairs_from_a_subset_match_all_pairs() {
        let all = fixture();
        let set: Vec<&Lesson> = all.iter().filter(|l| compared(l)).collect();
        let v = Vectors::new(&set);
        let every: Vec<usize> = (0..set.len()).collect();
        let whole = v.pairs(&every, RELATED_MIN);
        let one = v.pairs(&[3], RELATED_MIN);
        assert_eq!(one.len(), whole.iter().filter(|p| p.0 == 3 || p.1 == 3).count());
    }

    #[test]
    fn edges_clusters_and_orphans() {
        let g = Graph::new(fixture());
        let at = |id: &str| g.find(id).unwrap();
        let names = |i: usize| -> Vec<(String, Vec<&'static str>)> {
            g.related(i).iter().map(|r| (g.lessons[r.lesson].frontmatter.id.clone(), r.edges.iter().map(Edge::name).collect())).collect()
        };
        let hub = names(at("1b00000002"));
        assert_eq!(hub[0].1[0], "link");
        assert_eq!(hub[1].1[0], "link");
        assert!(names(at("1a00000013")).iter().any(|(id, e)| id == "1b00000002" && e.contains(&"link")));
        assert!(names(at("1a00000007")).iter().any(|(id, e)| id == "1b00000003" && e == &["supersedes"]));
        let clusters: Vec<Vec<&str>> =
            g.clusters(DEFAULT_MIN_SIMILARITY).iter().map(|c| c.iter().map(|&i| g.lessons[i].frontmatter.id.as_str()).collect()).collect();
        assert!(
            clusters.contains(&vec!["1b00000001", "1a00000001"])
                || clusters.iter().any(|c| c.contains(&"1b00000001") && c.contains(&"1a00000001")),
            "{clusters:?}"
        );
        assert!(g.orphans().contains(&at("1b00000004")));
        assert!(!g.orphans().contains(&at("1b00000002")));
    }
}
