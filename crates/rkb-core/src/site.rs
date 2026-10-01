//! What `rkb site` publishes: the lessons the `web` sink allows in the clear, the lessons the
//! `web-protected` sink allows behind the site password, the links that would point at an
//! unpublished lesson, and the `site.json` index the rs-web template reads.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Value, json};

use crate::config::{self, KbConfig, SiteConfig};
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
    pub queries: Vec<String>,
    pub path: String,
}

/// The lessons to publish, as unchanged files, and the index that describes them.
pub struct Site {
    /// `(path in the staging folder, file bytes)`: `lessons/<kb path>` or `protected/<kb path>`, for
    /// lessons and the images they link from their own `.assets/` folder.
    pub files: Vec<(String, Vec<u8>)>,
    /// Public lesson ids, in path order.
    pub ids: Vec<String>,
    /// Protected lesson ids, in path order.
    pub protected: Vec<String>,
    /// The password group of each protected id; the site password has none.
    pub groups: BTreeMap<String, Option<String>>,
    pub secrets: Vec<Secret>,
    /// Lessons left out because the password they need is not set, by its variable.
    pub left_out: BTreeMap<String, usize>,
    /// Lessons held back: the held ids given to `collect_holding` and the lessons that link to them.
    pub held: Vec<String>,
    /// Lessons left out or encrypted only because no label set their sensitivity, by project, system
    /// or scope folder, so the build can name `rkb label <folder>`.
    pub defaulted: BTreeMap<String, usize>,
    pub index: Value,
}

/// The folder a label for `path` belongs on: `projects/<p>`, `systems/<s>`, or the scope, such as `general`.
fn label_folder(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        ["projects" | "systems", name, _, ..] => format!("{}/{name}", parts[0]),
        _ => parts[0].to_string(),
    }
}

/// The record of first publications, in the knowledge base so every machine and CI share it.
pub const RECORD: &str = "site/published.json";

/// Ids published in the clear, and ids published encrypted with their password group (`""` for the
/// site password). An older file's `ids` are in the clear, and its `encrypted` list is the site password.
#[derive(Default, PartialEq)]
pub struct Published {
    pub clear: BTreeSet<String>,
    pub encrypted: BTreeMap<String, String>,
}

pub fn read_published(path: &Path) -> Published {
    let v: Value = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let set = |k: &str| -> BTreeSet<String> {
        v[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
    };
    let mut clear = set("clear");
    clear.extend(set("ids"));
    let mut encrypted: BTreeMap<String, String> = set("encrypted").into_iter().map(|i| (i, String::new())).collect();
    if let Some(m) = v["encrypted"].as_object() {
        encrypted.extend(m.iter().map(|(k, g)| (k.clone(), g.as_str().unwrap_or_default().to_string())));
    }
    Published { clear, encrypted }
}

/// The knowledge base's record, with the per-machine record of an earlier rkb merged in.
pub fn published(root: &Path, state: &Path) -> Published {
    let mut p = read_published(&root.join(RECORD));
    let old = read_published(&state.join("site-published.json"));
    p.clear.extend(old.clear);
    for (i, g) in old.encrypted {
        p.encrypted.entry(i).or_insert(g);
    }
    p
}

/// The password group of a protected lesson; `""` for the site password.
pub fn group_name(s: &Site, id: &str) -> String {
    s.groups.get(id).cloned().flatten().unwrap_or_default()
}

/// Lessons `s` would publish that the record `p` has not seen in that form: new in the clear, and
/// new encrypted or encrypted with another password.
pub fn pending(s: &Site, p: &Published) -> (Vec<String>, Vec<String>) {
    let clear = s.ids.iter().filter(|i| !p.clear.contains(*i)).cloned().collect();
    let enc = s.protected.iter().filter(|i| p.encrypted.get(*i) != Some(&group_name(s, i))).cloned().collect();
    (clear, enc)
}

/// The label that names a lesson's password group.
pub const PASSWORD_LABEL: &str = "password";

/// The environment variable of a password: `SITE_PASSWORD`, or for group `team-a`,
/// `SITE_PASSWORD_TEAM_A`.
pub fn password_env(group: Option<&str>) -> String {
    match group {
        None => "SITE_PASSWORD".into(),
        Some(g) => {
            let name: String = g.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect();
            format!("SITE_PASSWORD_{name}")
        }
    }
}

/// Whether `labels` pass every key of the sink's `allow`. An empty `allow` passes nothing.
/// The page of a lesson: a public lesson at its knowledge-base path without `.md`, a protected one
/// at `/protected/<id>/`, since its path would show its topic and title. `/lessons/<id>/` is kept as
/// a redirect to a public lesson's page, so links survive `rkb move` and `rkb rename`.
pub fn url(kind: Kind, id: &str, path: &str) -> String {
    match kind {
        Kind::Public => format!("/{}/", path.strip_suffix(".md").unwrap_or(path)),
        Kind::Protected => format!("/protected/{id}/"),
    }
}

/// `(link target, kb path)` of each image a lesson links from its own `<slug>.assets/` folder.
fn images(l: &Lesson, files: &BTreeMap<String, Vec<u8>>) -> Vec<(String, String)> {
    let own = format!("{}.assets/", l.path.strip_suffix(".md").unwrap_or(&l.path));
    let mut out: Vec<(String, String)> = crate::body::scan(&l.body)
        .links
        .into_iter()
        .filter(|k| k.image)
        .filter_map(|k| {
            let target = crate::lint::resolve(&l.path, &k.target)??;
            (target.starts_with(&own) && files.contains_key(&target)).then_some((k.target, target))
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

fn folder(kind: Kind) -> &'static str {
    match kind {
        Kind::Public => "lessons",
        Kind::Protected => "protected",
    }
}

/// `usable` says whether the password in an environment variable is set; a lesson whose password
/// is not set is left out. A `password` label protects a lesson that either sink allows.
pub fn collect(root: &Path, usable: impl Fn(&str) -> bool) -> Result<Site> {
    collect_holding(root, usable, &BTreeSet::new())
}

/// `collect` without the lessons in `held`, nor any staged lesson that links to one of them, so a
/// lesson waiting for the user's yes and its referrers wait together instead of failing the build.
pub fn collect_holding(root: &Path, usable: impl Fn(&str) -> bool, held: &BTreeSet<String>) -> Result<Site> {
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
    let mut left_out: BTreeMap<String, usize> = BTreeMap::new();
    let mut defaulted: BTreeMap<String, usize> = BTreeMap::new();
    let mut passwords: Vec<Option<String>> = vec![];
    let mut kinds: Vec<Option<Kind>> = lessons
        .iter()
        .map(|l| {
            let labels = crate::write::effective(&snap, &l.path, &l.frontmatter.labels);
            let group = labels.get(PASSWORD_LABEL).cloned();
            let (public, protectable) = (config::sink_allows(&sink.allow, &labels), config::sink_allows(protected_allow, &labels));
            passwords.push(group.clone());
            if !matches!(l.frontmatter.status, Status::Active | Status::Stale) {
                return None;
            }
            if public && group.is_none() {
                return Some(Kind::Public);
            }
            if !crate::write::labeled(&snap, &l.path, &l.frontmatter.labels, "sensitivity") {
                *defaulted.entry(label_folder(&l.path)).or_default() += 1;
            }
            if !public && !protectable {
                return None;
            }
            let env = password_env(group.as_deref());
            if usable(&env) {
                Some(Kind::Protected)
            } else {
                *left_out.entry(env).or_default() += 1;
                None
            }
        })
        .collect();
    let g = Graph::new(lessons);
    let mut dropped: BTreeSet<usize> =
        (0..g.lessons.len()).filter(|&i| kinds[i].is_some() && held.contains(&g.lessons[i].frontmatter.id)).collect();
    loop {
        for &i in &dropped {
            kinds[i] = None;
        }
        let more: BTreeSet<usize> = g.links.iter().filter(|&&(a, b)| kinds[a].is_some() && dropped.contains(&b)).map(|&(a, _)| a).collect();
        if more.is_empty() {
            break;
        }
        dropped.extend(more);
    }
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
                "label each linked lesson so [sinks.{SINK}] allows it, or [sinks.{PROTECTED_SINK}] with its password set, or remove the link"
            ),
        });
    }
    let index = index(&g, &kinds, &passwords, &kb.site, &snap.files);
    let held: Vec<String> = dropped.iter().map(|&i| g.lessons[i].frontmatter.id.clone()).collect();
    let mut site =
        Site { files: vec![], ids: vec![], protected: vec![], groups: BTreeMap::new(), secrets: vec![], left_out, held, defaulted, index };
    for ((l, kind), group) in g.lessons.iter().zip(&kinds).zip(&passwords) {
        let Some(kind) = *kind else { continue };
        site.files.push((format!("{}/{}", folder(kind), l.path), snap.files[&l.path].clone()));
        for (_, path) in images(l, &snap.files) {
            let staged = (format!("{}/{path}", folder(kind)), snap.files[&path].clone());
            if !site.files.contains(&staged) {
                site.files.push(staged);
            }
        }
        let id = l.frontmatter.id.clone();
        match kind {
            Kind::Public => site.ids.push(id),
            Kind::Protected => {
                site.secrets.push(Secret {
                    id: id.clone(),
                    title: graph::title(l),
                    tags: l.frontmatter.tags.clone(),
                    queries: l.frontmatter.queries.clone(),
                    path: l.path.clone(),
                });
                site.groups.insert(id.clone(), group.clone());
                site.protected.push(id);
            }
        }
    }
    Ok(site)
}

/// A public row names public lessons only; a protected row also names lessons with its own password.
fn row(
    g: &Graph,
    kinds: &[Option<Kind>],
    passwords: &[Option<String>],
    by_path: &BTreeMap<&str, usize>,
    files: &BTreeMap<String, Vec<u8>>,
    i: usize,
) -> Value {
    let l: &Lesson = &g.lessons[i];
    let kind = kinds[i].expect("rows are for staged lessons");
    let shown = |j: usize| match kinds[j] {
        Some(Kind::Public) => true,
        Some(Kind::Protected) => kind == Kind::Protected && passwords[j] == passwords[i],
        None => false,
    };
    let id = |j: usize| g.lessons[j].frontmatter.id.clone();
    let fm = &l.frontmatter;
    let parts: Vec<&str> = l.path.split('/').collect();
    let links: BTreeSet<String> = g.links.iter().filter(|&&(a, b)| a == i && kinds[b].is_some()).map(|&(_, b)| id(b)).collect();
    let backlinks: BTreeSet<String> = g.links.iter().filter(|&&(a, b)| b == i && shown(a)).map(|&(a, _)| id(a)).collect();
    let related: Vec<String> = g.related(i).into_iter().filter(|r| shown(r.lesson)).take(RELATED).map(|r| id(r.lesson)).collect();
    let hrefs: BTreeMap<String, String> = crate::body::scan(&l.body)
        .links
        .iter()
        .filter(|k| !k.image)
        .filter_map(|k| {
            let target = crate::lint::resolve(&l.path, &k.target)??;
            let j = *by_path.get(target.as_str())?;
            kinds[j].map(|kj| (k.target.clone(), url(kj, &id(j), &g.lessons[j].path)))
        })
        .collect();
    let mut row = json!({
        "id": fm.id,
        "path": format!("{}/{}", folder(kind), l.path),
        "url": url(kind, &fm.id, &l.path),
        "title": graph::title(l),
        "type": fm.kind,
        "status": fm.status.as_str(),
        "scope": parts[0],
        "topic": kb::topic_of(&l.path).unwrap_or_default(),
        "tags": fm.tags,
        "queries": fm.queries,
        "when": fm.when,
        "verified": fm.verified.to_string(),
        "verified_how": fm.verified_how,
        "links": links,
        "backlinks": backlinks,
        "related": related,
        "hrefs": hrefs,
        "images": images(l, files).into_iter().map(|(t, p)| (t, format!("{}/{p}", folder(kind)))).collect::<BTreeMap<_, _>>(),
    });
    if let Some(r) = &fm.stale_reason {
        row["stale_reason"] = json!(r);
    }
    for (scope, key) in [("projects", "project"), ("systems", "system")] {
        if parts[0] == scope {
            row[key] = json!(parts[1]);
        }
    }
    if kind == Kind::Protected {
        row["password"] = json!(passwords[i]);
        row["password_env"] = json!(password_env(passwords[i].as_deref()));
    }
    row
}

fn index(g: &Graph, kinds: &[Option<Kind>], passwords: &[Option<String>], site: &SiteConfig, files: &BTreeMap<String, Vec<u8>>) -> Value {
    let by_path: BTreeMap<&str, usize> = g.lessons.iter().enumerate().map(|(j, l)| (l.path.as_str(), j)).collect();
    let mut groups: BTreeMap<&str, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    let (mut public, mut protected) = (vec![], vec![]);
    for i in 0..g.lessons.len() {
        match kinds[i] {
            Some(Kind::Public) => {
                let r = row(g, kinds, passwords, &by_path, files, i);
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
            Some(Kind::Protected) => protected.push(row(g, kinds, passwords, &by_path, files, i)),
            None => {}
        }
    }
    let group = |k: &str| json!(groups.get(k).cloned().unwrap_or_default());
    let setting = |v: &Option<String>, default: &str| v.clone().unwrap_or_else(|| default.into());
    json!({
        "site": {
            "title": setting(&site.title, "Lessons learned"),
            "description": setting(&site.description, ""),
            "base_url": setting(&site.base_url, ""),
            "author": setting(&site.author, ""),
            "analytics": setting(&site.analytics, ""),
        },
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

    /// `lesson` with extra label lines, such as `"  password: team-a\n"`.
    fn labeled(id: &str, title: &str, labels: &str, body: &str) -> String {
        lesson(id, title, "", body).replacen("\n---\n", &format!("\nlabels:\n{labels}---\n"), 1)
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
        let s = collect(dir.path(), |_: &str| false).unwrap();
        assert_eq!(s.ids, ["0000000001", "0000000002"]);
        assert_eq!((s.protected.len(), s.left_out.get("SITE_PASSWORD")), (0, Some(&1)), "the unlabeled lesson waits for a password");
        assert_eq!(s.files[0].0, "lessons/general/git/a.md");
        let text = s.index.to_string();
        assert!(!text.contains("0000000003") && !text.contains("No label"), "{text}");
        let a = &s.index["lessons"][0];
        assert_eq!((a["title"].as_str(), a["type"].as_str(), a["topic"].as_str()), (Some("Public one"), Some("fact"), Some("git")));
        assert_eq!(a["links"], json!(["0000000002"]));
        assert_eq!(a["hrefs"], json!({ "../../projects/p/io/b.md": "/projects/p/io/b/" }));
        assert_eq!(a["url"], "/general/git/a/");
        assert_eq!(s.index["lessons"][1]["project"], "p");
        assert_eq!(s.index["projects"]["p"], json!(["0000000002"]));
    }

    #[test]
    fn linked_images_and_stale_reasons_are_staged() {
        let stale = lesson("0000000002", "Stale one", "public", "Old.")
            .replace("status: active", "status: stale\nstale_reason: newer cmake changed this");
        let dir = kb(&[
            ("general/git/a.md", lesson("0000000001", "Public one", "public", "![p](a.assets/p.png) ![o](b.assets/o.png)")),
            ("general/git/a.assets/p.png", "png".into()),
            ("general/git/a.assets/unlinked.png", "x".into()),
            ("general/git/b.md", stale),
            ("general/git/b.assets/o.png", "other".into()),
            ("general/git/c.md", lesson("0000000003", "Secret one", "", "![s](c.assets/s.svg)")),
            ("general/git/c.assets/s.svg", "<svg/>".into()),
        ]);
        let s = collect(dir.path(), |_: &str| true).unwrap();
        let staged: Vec<&str> = s.files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            staged,
            [
                "lessons/general/git/a.md",
                "lessons/general/git/a.assets/p.png",
                "lessons/general/git/b.md",
                "protected/general/git/c.md",
                "protected/general/git/c.assets/s.svg"
            ]
        );
        let a = &s.index["lessons"][0];
        assert_eq!(a["images"], json!({ "a.assets/p.png": "lessons/general/git/a.assets/p.png" }), "only its own folder");
        assert!(a.get("stale_reason").is_none());
        assert_eq!(s.index["lessons"][1]["stale_reason"], "newer cmake changed this");
        assert_eq!(s.index["protected"][0]["images"], json!({ "c.assets/s.svg": "protected/general/git/c.assets/s.svg" }));
    }

    #[test]
    fn held_lessons_take_their_referrers_along() {
        let dir = kb(&[
            ("general/git/a.md", lesson("0000000001", "Links to b", "public", "See [b](b.md).")),
            ("general/git/b.md", lesson("0000000002", "Links to c", "public", "See [c](c.md).")),
            ("general/git/c.md", lesson("0000000003", "New one", "public", "Plain.")),
            ("general/git/d.md", lesson("0000000004", "Unrelated", "public", "Plain.")),
        ]);
        let held: BTreeSet<String> = ["0000000003".to_string()].into();
        let s = collect_holding(dir.path(), |_: &str| false, &held).unwrap();
        assert_eq!(s.ids, ["0000000004"]);
        assert_eq!(s.held, ["0000000001", "0000000002", "0000000003"]);
        assert!(!s.index.to_string().contains("New one") && !s.index.to_string().contains("Links to b"));
        assert!(collect(dir.path(), |_: &str| false).unwrap().held.is_empty());
    }

    #[test]
    fn internal_lessons_are_protected_and_stay_apart() {
        let dir = kb(&[
            ("general/git/a.md", lesson("0000000001", "Public one", "public", "See [c](c.md). Shared words about rebase.")),
            ("general/git/c.md", lesson("0000000003", "Secret one", "", "Shared words about rebase.")),
            ("general/git/d.md", lesson("0000000004", "Confidential one", "confidential", "Shared words about rebase.")),
        ]);
        let s = collect(dir.path(), |_: &str| true).unwrap();
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
    fn backlinks_never_reveal_a_protected_source_to_a_public_page() {
        let dir = kb(&[
            ("general/git/a.md", lesson("0000000001", "Public a", "public", "See [b](b.md).")),
            ("general/git/b.md", lesson("0000000002", "Public b", "public", "Plain.")),
            ("general/git/p.md", lesson("0000000003", "Protected p", "", "See [b](b.md) and [a](a.md).")),
        ]);
        let s = collect(dir.path(), |_: &str| true).unwrap();
        let (a, b, p) = (&s.index["lessons"][0], &s.index["lessons"][1], &s.index["protected"][0]);
        assert_eq!(b["backlinks"], json!(["0000000001"]));
        assert_eq!(a["backlinks"], json!([]));
        assert_eq!(a["verified_how"], "ran");
        assert_eq!(p["backlinks"], json!([]));
        std::fs::write(dir.path().join("general/git/a.md"), lesson("0000000001", "Public a", "public", "See [p](p.md).")).unwrap();
        let s = collect(dir.path(), |_: &str| true).unwrap();
        assert_eq!(s.index["protected"][0]["backlinks"], json!(["0000000001"]));
    }

    #[test]
    fn site_settings_come_from_kb_toml() {
        let dir = kb(&[]);
        assert_eq!(collect(dir.path(), |_: &str| false).unwrap().index["site"]["title"], "Lessons learned");
        let toml = format!("{}\n[site]\ntitle = \"Field notes\"\n", config::DEFAULT_KB_TOML);
        std::fs::write(dir.path().join("kb.toml"), toml).unwrap();
        let site = &collect(dir.path(), |_: &str| false).unwrap().index["site"];
        assert_eq!((site["title"].as_str(), site["base_url"].as_str()), (Some("Field notes"), Some("")));
    }

    #[test]
    fn a_link_to_an_unpublished_lesson_fails() {
        let dir = kb(&[
            ("general/git/a.md", lesson("0000000001", "Public", "public", "See [c](c.md).")),
            ("general/git/c.md", lesson("0000000003", "Private", "confidential", "x")),
        ]);
        let Err(Error::NotReady { reason, .. }) = collect(dir.path(), |_: &str| true) else { panic!("expected a refusal") };
        assert!(reason.contains("general/git/a.md -> general/git/c.md"), "{reason}");
        std::fs::write(dir.path().join("general/git/c.md"), lesson("0000000003", "Private", "internal", "x")).unwrap();
        assert!(collect(dir.path(), |_: &str| false).is_err(), "without a password an internal target is not published");
        assert!(collect(dir.path(), |_: &str| true).is_ok(), "with one it is protected");
    }

    #[test]
    fn empty_allow_and_missing_sink_publish_nothing() {
        assert!(!config::sink_allows(&BTreeMap::new(), &BTreeMap::from([("sensitivity".into(), "public".into())])));
        let dir = kb(&[]);
        std::fs::write(dir.path().join("kb.toml"), "[labels]\nsensitivity = [\"public\"]\n").unwrap();
        assert!(matches!(collect(dir.path(), |_: &str| true), Err(Error::NotReady { .. })));
    }

    #[test]
    fn password_env_names() {
        assert_eq!(password_env(None), "SITE_PASSWORD");
        assert_eq!(password_env(Some("team-a")), "SITE_PASSWORD_TEAM_A");
        assert_eq!(password_env(Some("x.y z")), "SITE_PASSWORD_X_Y_Z");
    }

    #[test]
    fn folder_and_lesson_passwords() {
        let dir = kb(&[
            ("projects/p/README.md", "---\nlabels:\n  sensitivity: public\n  password: team-a\n---\n# p\n".into()),
            ("projects/p/io/a.md", lesson("0000000001", "Team one", "", "Plain words about rebase.")),
            ("projects/p/io/b.md", labeled("0000000002", "Solo one", "  password: solo\n", "Plain words about rebase.")),
            ("general/git/c.md", lesson("0000000003", "Site one", "", "Plain words about rebase. See [a](../../projects/p/io/a.md).")),
            ("general/git/d.md", lesson("0000000004", "Public one", "public", "Plain words about rebase.")),
        ]);
        std::fs::write(
            dir.path().join("kb.toml"),
            config::DEFAULT_KB_TOML.replace("[labels]\n", "[labels]\npassword = [\"team-a\", \"solo\"]\n"),
        )
        .unwrap();
        let s = collect(dir.path(), |_: &str| true).unwrap();
        assert_eq!(s.ids, ["0000000004"], "a password label protects even a public folder");
        assert_eq!(s.groups["0000000001"].as_deref(), Some("team-a"));
        assert_eq!(s.groups["0000000002"].as_deref(), Some("solo"), "the lesson's label beats the folder's");
        assert_eq!(s.groups["0000000003"], None);
        let prot = s.index["protected"].as_array().unwrap();
        let row = |id: &str| prot.iter().find(|r| r["id"] == id).unwrap().clone();
        assert_eq!(row("0000000001")["password_env"], "SITE_PASSWORD_TEAM_A");
        assert_eq!(row("0000000003")["password_env"], "SITE_PASSWORD");
        assert_eq!(row("0000000001")["related"], json!(["0000000004"]), "related lessons never cross passwords");
        assert_eq!(row("0000000001")["backlinks"], json!([]), "a site-password lesson linking in is not named");
        assert_eq!(row("0000000003")["related"], json!(["0000000004"]));

        let left = collect(dir.path(), |env: &str| env != "SITE_PASSWORD_TEAM_A");
        assert!(matches!(left, Err(Error::NotReady { .. })), "the site lesson links to a lesson that is left out");
        std::fs::write(dir.path().join("general/git/c.md"), lesson("0000000003", "Site one", "", "Plain.")).unwrap();
        let s = collect(dir.path(), |env: &str| env != "SITE_PASSWORD_TEAM_A").unwrap();
        assert_eq!(s.left_out.get("SITE_PASSWORD_TEAM_A"), Some(&1));
        assert!(!s.protected.contains(&"0000000001".to_string()) && s.protected.contains(&"0000000002".to_string()));
    }
}
