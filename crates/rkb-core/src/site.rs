//! What `rkb site` publishes: the lessons the `web` sink allows in the clear, the lessons the
//! `web-protected` sink allows behind the site password, the links that would point at an
//! unpublished lesson, and the `site.json` index the rs-web template reads.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Value, json};

use crate::config::{self, KbConfig};
use crate::error::{Error, Result};
use crate::graph::{self, Graph};
use crate::kb::{self, Snapshot};
use crate::lesson::{Lesson, Status};

pub const SINK: &str = "web";
pub const PROTECTED_SINK: &str = "web-protected";
/// Related lessons listed per lesson in `site.json`.
const RELATED: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Public,
    Protected,
}

/// What a protected lesson must never show in plain text in the built site.
pub struct Secret {
    pub id: String,
    pub title: String,
    pub tags: Vec<String>,
    pub path: String,
}

/// The lessons to publish, as unchanged files, and the index that describes them.
pub struct Site {
    /// `(path in the staging folder, file bytes)`: `lessons/<kb path>` or `protected/<kb path>`.
    pub files: Vec<(String, Vec<u8>)>,
    /// Public lesson ids, in path order.
    pub ids: Vec<String>,
    /// Protected lesson ids, in path order.
    pub protected: Vec<String>,
    pub secrets: Vec<Secret>,
    /// Lessons `web-protected` allows that were left out because protection is off.
    pub left_out: usize,
    pub index: Value,
}

/// Whether `labels` pass every key of the sink's `allow`. An empty `allow` passes nothing.
fn allowed(allow: &BTreeMap<String, Vec<String>>, labels: &BTreeMap<String, String>) -> bool {
    !allow.is_empty() && allow.iter().all(|(k, vals)| labels.get(k).is_some_and(|v| vals.contains(v)))
}

/// The page of a lesson. The output check relies on public pages under `lessons/<id>/` and
/// protected pages under `protected/<id>/`.
pub fn url(kind: Kind, id: &str) -> String {
    match kind {
        Kind::Public => format!("/lessons/{id}/"),
        Kind::Protected => format!("/protected/{id}/"),
    }
}

fn folder(kind: Kind) -> &'static str {
    match kind {
        Kind::Public => "lessons",
        Kind::Protected => "protected",
    }
}

/// `protect` says whether a usable site password is set; without it no lesson is protected.
pub fn collect(root: &Path, protect: bool) -> Result<Site> {
    let snap = Snapshot::from_dir(root)?;
    let kb: KbConfig = snap
        .files
        .get("kb.toml")
        .map(|d| config::parse(&String::from_utf8_lossy(d)))
        .transpose()
        .map_err(|e| Error::NotReady { reason: format!("kb.toml: {e}"), fix: "run `rkb lint` and fix kb.toml".into() })?
        .unwrap_or_default();
    let Some(sink) = kb.sinks.get(SINK) else {
        return Err(Error::NotReady {
            reason: format!("kb.toml has no [sinks.{SINK}], so no lesson may be published"),
            fix: format!("add `[sinks.{SINK}]` with `allow = {{ sensitivity = [\"public\"] }}` to kb.toml"),
        });
    };
    let empty = BTreeMap::new();
    let protected_allow = kb.sinks.get(PROTECTED_SINK).map_or(&empty, |s| &s.allow);
    let (mut lessons, _) = snap.lessons();
    lessons.sort_by(|a, b| a.path.cmp(&b.path));
    let mut left_out = 0;
    let kinds: Vec<Option<Kind>> = lessons
        .iter()
        .map(|l| {
            if !matches!(l.frontmatter.status, Status::Active | Status::Stale) {
                return None;
            }
            let labels = crate::write::effective(&snap, &l.path, &l.frontmatter.labels);
            if allowed(&sink.allow, &labels) {
                Some(Kind::Public)
            } else if allowed(protected_allow, &labels) {
                if protect {
                    Some(Kind::Protected)
                } else {
                    left_out += 1;
                    None
                }
            } else {
                None
            }
        })
        .collect();
    let g = Graph::new(lessons);
    let leaks: Vec<String> = g
        .links
        .iter()
        .filter(|&&(a, b)| kinds[a].is_some() && kinds[b].is_none())
        .map(|&(a, b)| format!("{} -> {}", g.lessons[a].path, g.lessons[b].path))
        .collect();
    if !leaks.is_empty() {
        return Err(Error::NotReady {
            reason: format!("published lessons link to lessons that are not published: {}", leaks.join(", ")),
            fix: format!(
                "label each linked lesson so [sinks.{SINK}] allows it, or [sinks.{PROTECTED_SINK}] with SITE_PASSWORD set, or remove the link"
            ),
        });
    }
    let index = index(&g, &kinds);
    let mut site = Site { files: vec![], ids: vec![], protected: vec![], secrets: vec![], left_out, index };
    for (l, kind) in g.lessons.iter().zip(&kinds) {
        let Some(kind) = *kind else { continue };
        site.files.push((format!("{}/{}", folder(kind), l.path), snap.files[&l.path].clone()));
        let id = l.frontmatter.id.clone();
        match kind {
            Kind::Public => site.ids.push(id),
            Kind::Protected => {
                site.secrets.push(Secret {
                    id: id.clone(),
                    title: graph::title(l),
                    tags: l.frontmatter.tags.clone(),
                    path: l.path.clone(),
                });
                site.protected.push(id);
            }
        }
    }
    Ok(site)
}

fn row(g: &Graph, kinds: &[Option<Kind>], by_path: &BTreeMap<&str, usize>, i: usize, related_to: Option<Kind>) -> Value {
    let l: &Lesson = &g.lessons[i];
    let kind = kinds[i].expect("rows are for staged lessons");
    let id = |j: usize| g.lessons[j].frontmatter.id.clone();
    let fm = &l.frontmatter;
    let parts: Vec<&str> = l.path.split('/').collect();
    let links: BTreeSet<String> = g.links.iter().filter(|&&(a, b)| a == i && kinds[b].is_some()).map(|&(_, b)| id(b)).collect();
    let related: Vec<String> = g
        .related(i)
        .into_iter()
        .filter(|r| kinds[r.lesson].is_some() && related_to.is_none_or(|k| kinds[r.lesson] == Some(k)))
        .take(RELATED)
        .map(|r| id(r.lesson))
        .collect();
    let hrefs: BTreeMap<String, String> = crate::body::scan(&l.body)
        .links
        .iter()
        .filter(|k| !k.image)
        .filter_map(|k| {
            let target = crate::lint::resolve(&l.path, &k.target)??;
            let j = *by_path.get(target.as_str())?;
            kinds[j].map(|kj| (k.target.clone(), url(kj, &id(j))))
        })
        .collect();
    let mut row = json!({
        "id": fm.id,
        "path": format!("{}/{}", folder(kind), l.path),
        "url": url(kind, &fm.id),
        "title": graph::title(l),
        "type": fm.kind,
        "status": fm.status.as_str(),
        "scope": parts[0],
        "topic": kb::topic_of(&l.path).unwrap_or_default(),
        "tags": fm.tags,
        "when": fm.when,
        "verified": fm.verified.to_string(),
        "links": links,
        "related": related,
        "hrefs": hrefs,
    });
    for (scope, key) in [("projects", "project"), ("systems", "system")] {
        if parts[0] == scope {
            row[key] = json!(parts[1]);
        }
    }
    row
}

fn index(g: &Graph, kinds: &[Option<Kind>]) -> Value {
    let by_path: BTreeMap<&str, usize> = g.lessons.iter().enumerate().map(|(j, l)| (l.path.as_str(), j)).collect();
    let mut groups: BTreeMap<&str, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    let (mut public, mut protected) = (vec![], vec![]);
    for i in 0..g.lessons.len() {
        match kinds[i] {
            Some(Kind::Public) => {
                let r = row(g, kinds, &by_path, i, Some(Kind::Public));
                let id = r["id"].as_str().unwrap_or_default().to_string();
                groups.entry("topics").or_default().entry(r["topic"].as_str().unwrap_or_default().into()).or_default().push(id.clone());
                for (key, group) in [("project", "projects"), ("system", "systems")] {
                    if let Some(name) = r[key].as_str() {
                        groups.entry(group).or_default().entry(name.into()).or_default().push(id.clone());
                    }
                }
                for t in g.lessons[i].frontmatter.tags.iter() {
                    groups.entry("tags").or_default().entry(t.clone()).or_default().push(id.clone());
                }
                public.push(r);
            }
            Some(Kind::Protected) => protected.push(row(g, kinds, &by_path, i, None)),
            None => {}
        }
    }
    let group = |k: &str| json!(groups.get(k).cloned().unwrap_or_default());
    json!({
        "lessons": public,
        "protected": protected,
        "topics": group("topics"),
        "projects": group("projects"),
        "systems": group("systems"),
        "tags": group("tags"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lesson(id: &str, title: &str, labels: &str, body: &str) -> String {
        let labels = if labels.is_empty() { String::new() } else { format!("labels:\n  sensitivity: {labels}\n") };
        format!(
            "---\nschema: 1\nid: {id}\ntype: fact\nstatus: active\nverified: 2026-09-01\nverified_how: ran\ntags:\n  - t{id}\n  - git\n{labels}---\n\n# {title}\n\n## Statement\n{body}\n\n## Evidence\nSeen.\n"
        )
    }

    fn kb(files: &[(&str, String)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("kb.toml"), config::DEFAULT_KB_TOML).unwrap();
        for (p, text) in files {
            let path = dir.path().join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    #[test]
    fn only_public_lessons_and_only_their_ids() {
        let dir = kb(&[
            ("general/git/a.md", lesson("0000000001", "Public one", "public", "See [b](../../projects/p/io/b.md).")),
            ("projects/p/io/b.md", lesson("0000000002", "Public by folder", "", "Plain.")),
            ("projects/p/README.md", "---\nlabels:\n  sensitivity: public\n---\n# p\n".into()),
            ("general/git/c.md", lesson("0000000003", "No label", "", "Public one shares words with this.")),
        ]);
        let s = collect(dir.path(), false).unwrap();
        assert_eq!(s.ids, ["0000000001", "0000000002"]);
        assert_eq!((s.protected.len(), s.left_out), (0, 1), "the unlabeled lesson waits for a password");
        assert_eq!(s.files[0].0, "lessons/general/git/a.md");
        let text = s.index.to_string();
        assert!(!text.contains("0000000003") && !text.contains("No label"), "{text}");
        let a = &s.index["lessons"][0];
        assert_eq!((a["title"].as_str(), a["type"].as_str(), a["topic"].as_str()), (Some("Public one"), Some("fact"), Some("git")));
        assert_eq!(a["links"], json!(["0000000002"]));
        assert_eq!(a["hrefs"], json!({ "../../projects/p/io/b.md": "/lessons/0000000002/" }));
        assert_eq!(a["url"], "/lessons/0000000001/");
        assert_eq!(s.index["lessons"][1]["project"], "p");
        assert_eq!(s.index["projects"]["p"], json!(["0000000002"]));
    }

    #[test]
    fn internal_lessons_are_protected_and_stay_apart() {
        let dir = kb(&[
            ("general/git/a.md", lesson("0000000001", "Public one", "public", "See [c](c.md). Shared words about rebase.")),
            ("general/git/c.md", lesson("0000000003", "Secret one", "", "Shared words about rebase.")),
            ("general/git/d.md", lesson("0000000004", "Confidential one", "confidential", "Shared words about rebase.")),
        ]);
        let s = collect(dir.path(), true).unwrap();
        assert_eq!((s.ids.as_slice(), s.protected.as_slice()), (&["0000000001".to_string()][..], &["0000000003".to_string()][..]));
        assert_eq!(s.files[1].0, "protected/general/git/c.md");
        let a = &s.index["lessons"][0];
        assert_eq!(a["related"], json!([]), "public related lists leave protected lessons out");
        assert_eq!(a["hrefs"], json!({ "c.md": "/protected/0000000003/" }));
        assert_eq!(s.index["tags"]["git"], json!(["0000000001"]));
        assert_eq!(s.index["topics"]["git"], json!(["0000000001"]));
        let p = &s.index["protected"][0];
        assert_eq!((p["url"].as_str(), p["title"].as_str()), (Some("/protected/0000000003/"), Some("Secret one")));
        assert_eq!(p["related"], json!(["0000000001"]));
        assert!(!s.index.to_string().contains("Confidential one"));
        assert_eq!((s.secrets[0].title.as_str(), s.secrets[0].path.as_str()), ("Secret one", "general/git/c.md"));
    }

    #[test]
    fn a_link_to_an_unpublished_lesson_fails() {
        let dir = kb(&[
            ("general/git/a.md", lesson("0000000001", "Public", "public", "See [c](c.md).")),
            ("general/git/c.md", lesson("0000000003", "Private", "confidential", "x")),
        ]);
        let Err(Error::NotReady { reason, .. }) = collect(dir.path(), true) else { panic!("expected a refusal") };
        assert!(reason.contains("general/git/a.md -> general/git/c.md"), "{reason}");
        std::fs::write(dir.path().join("general/git/c.md"), lesson("0000000003", "Private", "internal", "x")).unwrap();
        assert!(collect(dir.path(), false).is_err(), "without a password an internal target is not published");
        assert!(collect(dir.path(), true).is_ok(), "with one it is protected");
    }

    #[test]
    fn empty_allow_and_missing_sink_publish_nothing() {
        assert!(!allowed(&BTreeMap::new(), &BTreeMap::from([("sensitivity".into(), "public".into())])));
        let dir = kb(&[]);
        std::fs::write(dir.path().join("kb.toml"), "[labels]\nsensitivity = [\"public\"]\n").unwrap();
        assert!(matches!(collect(dir.path(), true), Err(Error::NotReady { .. })));
    }
}
