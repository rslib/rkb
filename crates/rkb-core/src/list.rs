use std::collections::BTreeMap;
use std::path::Path;

use jiff::civil::Date;
use serde_norway::Value;

use crate::body;
use crate::error::{Error, Result};
use crate::kb::{SCOPES, Snapshot, topic_of};
use crate::lesson::{self, Lesson, LessonType, Status, VerifiedHow};

#[derive(Debug, Clone, PartialEq)]
pub struct TopicCount {
    pub folder: String,
    pub name: String,
    pub lessons: usize,
    pub stale: usize,
    /// Archived or superseded.
    pub retired: usize,
}

/// The topics of `general`, or of one project or system.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    /// The project or system name; `None` for `general`.
    pub name: Option<String>,
    pub folder: String,
    pub topics: Vec<TopicCount>,
}

impl Group {
    pub fn lessons(&self) -> usize {
        self.topics.iter().map(|t| t.lessons).sum()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scope {
    pub name: String,
    pub groups: Vec<Group>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Overview {
    pub total: usize,
    pub stale: usize,
    pub retired: usize,
    /// Lesson files that do not parse or sit at the wrong depth.
    pub unparsed: usize,
    pub scopes: Vec<Scope>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: String,
    pub kind: LessonType,
    pub status: Status,
    pub title: String,
    pub tags: Vec<String>,
    /// `when` entries as `(key, value)`, without the folder's own project or system.
    pub conditions: Vec<(String, String)>,
    pub checked: bool,
    pub verified: Date,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TagGroup {
    /// `None` for lessons without tags.
    pub tag: Option<String>,
    pub lessons: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TopicView {
    pub folder: String,
    /// The H1 of the folder note, when there is one.
    pub title: Option<String>,
    pub groups: Vec<TagGroup>,
    /// Archived and superseded lessons.
    pub retired: Vec<Entry>,
    pub unparsed: usize,
}

impl TopicView {
    pub fn count(&self) -> usize {
        self.groups.iter().map(|g| g.lessons.len()).sum::<usize>() + self.retired.len()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Listing {
    Overview(Overview),
    Topic(TopicView),
}

/// Builds the view for `arg`: nothing or a scope folder gives an overview, a topic folder its lessons.
pub fn list(root: &Path, arg: Option<&str>) -> Result<Listing> {
    let arg = arg.map(|a| a.trim_start_matches("./").trim_end_matches('/')).filter(|a| !a.is_empty());
    let snap = Snapshot::from_dir(root)?;
    let Some(arg) = arg else { return Ok(Listing::Overview(overview(&snap, ""))) };
    let parts: Vec<&str> = arg.split('/').collect();
    let known_scope = SCOPES.contains(&parts[0]);
    if !known_scope || !root.join(arg).is_dir() {
        return Err(Error::NoFolder(arg.to_string()));
    }
    let is_topic = match parts[0] {
        "general" => parts.len() == 2,
        _ => parts.len() == 3,
    };
    if is_topic {
        Ok(Listing::Topic(topic(&snap, arg)))
    } else if parts.len() <= 2 {
        Ok(Listing::Overview(overview(&snap, arg)))
    } else {
        Err(Error::NoFolder(arg.to_string()))
    }
}

fn in_folder(path: &str, folder: &str) -> bool {
    folder.is_empty() || path.starts_with(&format!("{folder}/"))
}

fn overview(snap: &Snapshot, within: &str) -> Overview {
    let (lessons, bad) = snap.lessons();
    let mut ov =
        Overview { total: 0, stale: 0, retired: 0, unparsed: bad.iter().filter(|(p, _)| in_folder(p, within)).count(), scopes: vec![] };
    let mut counts: BTreeMap<String, TopicCount> = BTreeMap::new();
    for l in lessons.iter().filter(|l| in_folder(&l.path, within)) {
        let Some(name) = topic_of(&l.path) else {
            ov.unparsed += 1;
            continue;
        };
        let folder = l.path.rsplit_once('/').unwrap().0.to_string();
        let c = counts.entry(folder.clone()).or_insert(TopicCount { folder, name: name.to_string(), lessons: 0, stale: 0, retired: 0 });
        c.lessons += 1;
        ov.total += 1;
        match l.frontmatter.status {
            Status::Stale => {
                c.stale += 1;
                ov.stale += 1;
            }
            Status::Archived | Status::Superseded => {
                c.retired += 1;
                ov.retired += 1;
            }
            Status::Active => {}
        }
    }
    for scope in ["general", "projects", "systems"] {
        let mut groups: BTreeMap<String, Group> = BTreeMap::new();
        for c in counts.values().filter(|c| c.folder.starts_with(&format!("{scope}/"))) {
            let parts: Vec<&str> = c.folder.split('/').collect();
            let (name, folder) = match scope {
                "general" => (None, "general".to_string()),
                _ => (Some(parts[1].to_string()), format!("{scope}/{}", parts[1])),
            };
            groups.entry(folder.clone()).or_insert(Group { name, folder, topics: vec![] }).topics.push(c.clone());
        }
        if !groups.is_empty() {
            ov.scopes.push(Scope { name: scope.to_string(), groups: groups.into_values().collect() });
        }
    }
    ov
}

fn note_title(snap: &Snapshot, folder: &str) -> Option<String> {
    let text = String::from_utf8_lossy(snap.files.get(&format!("{folder}/README.md"))?).into_owned();
    let body_text = match lesson::split(&text) {
        Ok((_, b, _)) => b.to_string(),
        Err(_) => text,
    };
    body::scan(&body_text).h1.into_iter().next().map(|(_, t)| t)
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Sequence(items) => items.iter().map(value_text).collect::<Vec<_>>().join("|"),
        other => serde_norway::to_string(other).unwrap_or_default().trim().to_string(),
    }
}

fn entry(l: &Lesson, folder: &str) -> Entry {
    let parts: Vec<&str> = folder.split('/').collect();
    let own = match parts[0] {
        "projects" => Some(("project", parts[1])),
        "systems" => Some(("system", parts[1])),
        _ => None,
    };
    let fm = &l.frontmatter;
    let conditions = fm
        .when
        .iter()
        .filter_map(|(k, v)| {
            let k = k.as_str()?;
            let v = value_text(v);
            (own != Some((k, v.as_str()))).then(|| (k.to_string(), v))
        })
        .collect();
    Entry {
        id: fm.id.clone(),
        kind: fm.kind,
        status: fm.status,
        title: body::scan(&l.body).h1.into_iter().next().map_or_else(|| l.path.clone(), |(_, t)| t),
        tags: fm.tags.clone(),
        conditions,
        checked: fm.verified_how == VerifiedHow::Checked,
        verified: fm.verified,
    }
}

fn by_title(a: &Entry, b: &Entry) -> std::cmp::Ordering {
    a.title.to_lowercase().cmp(&b.title.to_lowercase()).then(a.title.cmp(&b.title))
}

fn topic(snap: &Snapshot, folder: &str) -> TopicView {
    let (lessons, bad) = snap.lessons();
    let mut groups: BTreeMap<Option<String>, Vec<Entry>> = BTreeMap::new();
    let mut retired = vec![];
    for l in lessons.iter().filter(|l| l.path.rsplit_once('/').map(|p| p.0) == Some(folder)) {
        let e = entry(l, folder);
        if l.frontmatter.status.is_current() {
            groups.entry(e.tags.first().cloned()).or_default().push(e);
        } else {
            retired.push(e);
        }
    }
    let mut groups: Vec<TagGroup> = groups
        .into_iter()
        .map(|(tag, mut lessons)| {
            lessons.sort_by(by_title);
            TagGroup { tag, lessons }
        })
        .collect();
    // `None` sorts first in a BTreeMap; untagged lessons go last.
    groups.sort_by_key(|g| g.tag.is_none());
    retired.sort_by(by_title);
    TopicView {
        folder: folder.to_string(),
        title: note_title(snap, folder),
        groups,
        retired,
        unparsed: bad.iter().filter(|(p, _)| p.rsplit_once('/').map(|x| x.0) == Some(folder)).count(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/kb")
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn lesson(id: &str, title: &str, extra: &str) -> String {
        format!(
            "---\nschema: 1\nid: {id}\ntype: fact\nstatus: active\n{extra}verified: 2026-09-25\nverified_how: ran\n---\n\n# {title}\n\n## Statement\nX.\n\n## Evidence\nY.\n"
        )
    }

    #[test]
    fn arguments() {
        let root = fixture();
        assert!(matches!(list(&root, None).unwrap(), Listing::Overview(_)));
        assert!(matches!(list(&root, Some("general/")).unwrap(), Listing::Overview(_)));
        assert!(matches!(list(&root, Some("projects/dftracer")).unwrap(), Listing::Overview(_)));
        assert!(matches!(list(&root, Some("./general/cpp/")).unwrap(), Listing::Topic(_)));
        assert!(matches!(list(&root, Some("projects/dftracer/cmake")).unwrap(), Listing::Topic(_)));
        assert!(matches!(list(&root, Some("general/nope")), Err(Error::NoFolder(f)) if f == "general/nope"));
        assert!(matches!(list(&root, Some("elsewhere")), Err(Error::NoFolder(_))));
    }

    #[test]
    fn overview_of_fixture() {
        let Listing::Overview(ov) = list(&fixture(), None).unwrap() else { panic!() };
        assert_eq!(ov.total, 5);
        let names: Vec<&str> = ov.scopes.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["general", "projects", "systems"]);
        let general: Vec<(&str, usize)> = ov.scopes[0].groups[0].topics.iter().map(|t| (t.name.as_str(), t.lessons)).collect();
        assert_eq!(general, [("cpp", 1), ("git", 1)]);
        assert_eq!(ov.scopes[1].groups[0].name.as_deref(), Some("dftracer"));
        assert_eq!(ov.scopes[1].groups[0].lessons(), 2);

        let Listing::Overview(one) = list(&fixture(), Some("projects/dftracer")).unwrap() else { panic!() };
        assert_eq!(one.total, 2);
        assert_eq!(one.scopes.len(), 1);
    }

    #[test]
    fn topic_grouping_retired_and_conditions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "general/cpp/README.md", "---\naliases:\n  - c++\n---\n\n# C and C++\n");
        write(root, "general/cpp/b.md", &lesson("0000000001", "Zeta regex", "tags:\n  - regex\n"));
        write(root, "general/cpp/a.md", &lesson("0000000002", "alpha regex", "tags:\n  - regex\n"));
        write(root, "general/cpp/c.md", &lesson("0000000003", "Fast path", "tags:\n  - performance\n"));
        write(root, "general/cpp/d.md", &lesson("0000000004", "No tags here", ""));
        write(
            root,
            "general/cpp/e.md",
            &lesson("0000000005", "Old way", "tags:\n  - regex\n").replace("status: active", "status: archived"),
        );
        let Listing::Topic(v) = list(root, Some("general/cpp")).unwrap() else { panic!() };
        assert_eq!(v.title.as_deref(), Some("C and C++"));
        assert_eq!(v.count(), 5);
        let order: Vec<(Option<&str>, Vec<&str>)> =
            v.groups.iter().map(|g| (g.tag.as_deref(), g.lessons.iter().map(|e| e.title.as_str()).collect())).collect();
        assert_eq!(
            order,
            [(Some("performance"), vec!["Fast path"]), (Some("regex"), vec!["alpha regex", "Zeta regex"]), (None, vec!["No tags here"])]
        );
        assert_eq!(v.retired.iter().map(|e| (e.title.as_str(), e.status)).collect::<Vec<_>>(), [("Old way", Status::Archived)]);

        let Listing::Topic(p) = list(&fixture(), Some("projects/dftracer/cmake")).unwrap() else { panic!() };
        let e = &p.groups[0].lessons[0];
        assert_eq!(e.conditions, [("system".to_string(), "tuolumne".to_string()), ("hdf5".into(), "1.12:1.14.2".into())]);

        write(
            root,
            "projects/dftracer/cmake/x.md",
            &lesson("0000000009", "X", "when:\n  project: dftracer\n  gpu:\n    - a100\n    - mi300a\n"),
        );
        let Listing::Topic(p) = list(root, Some("projects/dftracer/cmake")).unwrap() else { panic!() };
        assert_eq!(p.groups[0].lessons[0].conditions, [("gpu".to_string(), "a100|mi300a".to_string())]);
    }
}
