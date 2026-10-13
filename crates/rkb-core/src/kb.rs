use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config;
use crate::error::{Error, Result, io};
use crate::git;
use crate::lesson::{self, Lesson, ParseError};

pub const SCOPES: [&str; 3] = ["general", "systems", "projects"];

/// The knowledge base root: `$RKB_HOME`, or `~/Personal/kb` when it is not set.
/// `$RKB_TEST_HOME` wins over `$RKB_HOME` and is meant for tests that run under a user's shell profile.
pub fn home() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let var = |k| std::env::var(k).ok().filter(|p| !p.is_empty());
    match var("RKB_TEST_HOME").or_else(|| var("RKB_HOME")) {
        Some(p) if p == "~" => home,
        Some(p) => match p.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(p),
        },
        None => home.join("Personal/kb"),
    }
}

/// Whether neither `RKB_HOME` nor `HOME` is set. Then the knowledge base path is relative, so rkb stops
/// before it creates any file or folder, rather than write into the working directory. XDG variables
/// alone do not help, since they place the state but not the knowledge base.
pub fn no_home() -> bool {
    let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    !["RKB_TEST_HOME", "RKB_HOME", "HOME"].iter().any(|k| set(k))
}

/// Fails with `NotAKb` unless `root` has `kb.toml` and a git repository.
pub fn open(root: &Path) -> Result<()> {
    if root.join("kb.toml").is_file() && root.join(".git").exists() { Ok(()) } else { Err(Error::NotAKb(root.to_path_buf())) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    KbConfig,
    FolderNote,
    Lesson,
    Asset,
    Other,
}

pub fn classify(path: &str) -> FileKind {
    if path == "kb.toml" {
        return FileKind::KbConfig;
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 2 || !SCOPES.contains(&parts[0]) {
        return FileKind::Other;
    }
    if parts[..parts.len() - 1].iter().any(|p| p.ends_with(".assets")) {
        return FileKind::Asset;
    }
    match parts[parts.len() - 1] {
        "README.md" => FileKind::FolderNote,
        n if n.ends_with(".md") => FileKind::Lesson,
        _ => FileKind::Other,
    }
}

/// What a folder note describes, from where it sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteRole {
    /// `general/README.md`, `projects/README.md`, `systems/README.md`
    Scope,
    Project,
    System,
    Topic,
    /// Deeper than any folder a lesson can be in.
    Misplaced,
}

pub fn note_role(path: &str) -> NoteRole {
    let parts: Vec<&str> = path.split('/').collect();
    match (parts[0], parts.len()) {
        (_, 2) => NoteRole::Scope,
        ("projects", 3) => NoteRole::Project,
        ("systems", 3) => NoteRole::System,
        ("general", 3) | ("projects" | "systems", 4) => NoteRole::Topic,
        _ => NoteRole::Misplaced,
    }
}

/// Number of path parts a lesson has: `general/<topic>/<file>` or `<scope>/<name>/<topic>/<file>`.
pub fn lesson_depth(scope: &str) -> usize {
    if scope == "general" { 3 } else { 4 }
}

/// The topic folder of a lesson at the right depth.
pub fn topic_of(path: &str) -> Option<&str> {
    let parts: Vec<&str> = path.split('/').collect();
    (parts.len() == lesson_depth(parts[0])).then(|| parts[parts.len() - 2])
}

/// Folder notes from the lesson's folder up to its scope root, nearest first.
pub fn note_chain(path: &str) -> Vec<String> {
    let parts: Vec<&str> = path.split('/').collect();
    (1..parts.len()).rev().map(|i| format!("{}/README.md", parts[..i].join("/"))).collect()
}

fn relevant(path: &str) -> bool {
    path.split('/').all(|p| !p.starts_with('.')) && classify(path) != FileKind::Other
}

/// The files lint and index read, from the working tree or from the git index.
/// Paths of the relevant working-tree files, without reading them.
pub fn paths(root: &Path) -> Result<Vec<String>> {
    let mut out = vec![];
    let walk = ignore::WalkBuilder::new(root).parents(false).git_global(false).require_git(false).build();
    for entry in walk {
        let entry = entry.map_err(|e| Error::Io { path: root.to_path_buf(), source: std::io::Error::other(e) })?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else { continue };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if relevant(&rel) {
            out.push(rel);
        }
    }
    Ok(out)
}

#[derive(Debug, Default)]
pub struct Snapshot {
    pub files: BTreeMap<String, Vec<u8>>,
}

impl Snapshot {
    /// Reads the working tree, skipping hidden and git-ignored files.
    pub fn from_dir(root: &Path) -> Result<Self> {
        Self::from_dir_where(root, |_| true)
    }

    /// Reads only the working-tree files for which `keep(path)` is true.
    pub fn from_dir_where(root: &Path, keep: impl Fn(&str) -> bool) -> Result<Self> {
        let mut files = BTreeMap::new();
        for rel in paths(root)? {
            if keep(&rel) {
                let abs = root.join(&rel);
                let data = std::fs::read(&abs).map_err(io(&abs))?;
                files.insert(rel, data);
            }
        }
        Ok(Snapshot { files })
    }

    /// Reads the staged content of every relevant file in the git index.
    pub fn from_index(root: &Path) -> Result<Self> {
        let listed = git::run(root, &["ls-files", "-z"])?;
        let paths: Vec<String> = listed
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .filter(|p| relevant(p))
            .collect();
        let specs: Vec<String> = paths.iter().map(|p| format!(":{p}")).collect();
        let blobs = git::read_blobs(root, &specs)?;
        let files = paths.into_iter().zip(blobs).filter_map(|(p, b)| Some((p, b?))).collect();
        Ok(Snapshot { files })
    }

    pub fn of_kind(&self, kind: FileKind) -> impl Iterator<Item = (&String, &Vec<u8>)> {
        self.files.iter().filter(move |(p, _)| classify(p) == kind)
    }

    /// Every lesson that parses, and the parse error of every one that does not.
    pub fn lessons(&self) -> (Vec<Lesson>, Vec<(String, ParseError)>) {
        let mut ok = vec![];
        let mut bad = vec![];
        for (path, data) in self.of_kind(FileKind::Lesson) {
            let text = String::from_utf8_lossy(data);
            match lesson::parse(path, &text) {
                Ok(l) => ok.push(l),
                Err(e) => bad.push((path.clone(), e)),
            }
        }
        (ok, bad)
    }
}

/// The main knowledge base plus the extra read-only ones of the machine config.
#[derive(Debug, Clone)]
pub struct Roots {
    pub main: PathBuf,
    /// The extras whose folder is a readable knowledge base, in config order.
    pub extras: Vec<config::Extra>,
    /// One `(name, reason)` per configured extra that could not be read.
    pub skipped: Vec<(String, String)>,
}

impl Roots {
    /// Reads the machine config. A bad `[[extra]]` entry is an error with a fix; a missing or
    /// unreadable folder is skipped, because one broken share must not stop a search.
    pub fn load(main: &Path, config_dir: &Path) -> Result<Roots> {
        let fix = "fix the [[extra]] tables in the machine config.toml: each needs a unique name of 1 to 32 characters from a-z, 0-9 and - that is not `main`, and an absolute path";
        let configured = config::extras(config_dir).map_err(|reason| Error::NotReady { reason, fix: fix.to_string() })?;
        let mut extras = vec![];
        let mut skipped = vec![];
        for extra in configured {
            match std::fs::read_dir(&extra.path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    skipped.push((extra.name.clone(), format!("{} does not exist", extra.path.display())))
                }
                Err(e) => skipped.push((extra.name.clone(), format!("{} cannot be read: {e}", extra.path.display()))),
                Ok(_) if !extra.path.join("kb.toml").is_file() => {
                    skipped.push((extra.name.clone(), format!("{} has no kb.toml", extra.path.display())))
                }
                // A snapshot reads the git index, so a folder that is not a clone would fail every search.
                Ok(_) if !extra.path.join(".git").exists() => {
                    skipped.push((extra.name.clone(), format!("{} is not a git clone", extra.path.display())))
                }
                Ok(_) => extras.push(extra),
            }
        }
        Ok(Roots { main: main.to_path_buf(), extras, skipped })
    }

    /// The root of the knowledge base `kb` names, or the main one.
    pub fn root_of(&self, kb: Option<&str>) -> &Path {
        kb.and_then(|name| self.extras.iter().find(|e| e.name == name)).map_or(&self.main, |e| e.path.as_path())
    }

    /// The lesson `id` names, in the main or an extra knowledge base, with the name of that base.
    pub fn find(&self, id: &str) -> Result<(Option<String>, Lesson, String)> {
        let (kb, plain) = split_id(id, &self.extras);
        let (lesson, text) = find(self.root_of(kb.as_deref()), plain)?;
        Ok((kb, lesson, text))
    }

    /// A snapshot of every readable knowledge base, main first.
    pub fn snapshots(&self) -> Result<Vec<(Option<String>, Snapshot)>> {
        let mut out = vec![(None, Snapshot::from_dir(&self.main)?)];
        for extra in &self.extras {
            out.push((Some(extra.name.clone()), Snapshot::from_dir(&extra.path)?));
        }
        Ok(out)
    }
}

/// The `name` of an extra id such as `shared:2b81d05e77`, and the id without it. `None` and the
/// whole id for a main id, or for a prefix no extra uses.
pub fn split_id<'a>(id: &'a str, extras: &[config::Extra]) -> (Option<String>, &'a str) {
    match id.split_once(':') {
        Some((name, rest)) if extras.iter().any(|e| e.name == name) => (Some(name.to_string()), rest),
        _ => (None, id),
    }
}

/// `id` without an extra prefix, for looking a lesson up in its own knowledge base.
pub fn unqualified(id: &str) -> &str {
    id.split_once(':').map_or(id, |(_, rest)| rest)
}

/// The effective labels of the lesson at `path` in the knowledge base `kb`, from the snapshot of
/// that same knowledge base, so a folder note of one base never labels a lesson of another.
pub fn labels(snapshots: &[(Option<String>, Snapshot)], kb: Option<&str>, path: &str) -> std::collections::BTreeMap<String, String> {
    snapshots.iter().find(|(name, _)| name.as_deref() == kb).map(|(_, snap)| crate::write::effective_at(snap, path)).unwrap_or_default()
}

/// The lesson with `id` in the working tree and its full file text.
pub fn find(root: &Path, id: &str) -> Result<(Lesson, String)> {
    let snap = Snapshot::from_dir(root)?;
    let (lessons, _) = snap.lessons();
    let lesson = lessons.into_iter().find(|l| l.frontmatter.id == id).ok_or_else(|| Error::NotFound(id.to_string()))?;
    let text = String::from_utf8_lossy(&snap.files[&lesson.path]).into_owned();
    Ok((lesson, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_paths() {
        assert_eq!(classify("kb.toml"), FileKind::KbConfig);
        assert_eq!(classify("general/cpp/a.md"), FileKind::Lesson);
        assert_eq!(classify("general/cpp/README.md"), FileKind::FolderNote);
        assert_eq!(classify("notes.md"), FileKind::Other);
        assert_eq!(classify("README.md"), FileKind::Other);
        assert_eq!(classify("projects/x/project.toml"), FileKind::Other);
        assert_eq!(classify("projects/x/a.assets/p.png"), FileKind::Asset);
        assert_eq!(classify("projects/x/a.assets/n.md"), FileKind::Asset);
    }

    #[test]
    fn layout_helpers() {
        assert_eq!(note_role("general/README.md"), NoteRole::Scope);
        assert_eq!(note_role("projects/x/README.md"), NoteRole::Project);
        assert_eq!(note_role("systems/t/README.md"), NoteRole::System);
        assert_eq!(note_role("general/cpp/README.md"), NoteRole::Topic);
        assert_eq!(note_role("projects/x/cmake/README.md"), NoteRole::Topic);
        assert_eq!(note_role("general/cpp/regex/README.md"), NoteRole::Misplaced);
        assert_eq!(topic_of("general/cpp/a.md"), Some("cpp"));
        assert_eq!(topic_of("projects/x/cmake/a.md"), Some("cmake"));
        assert_eq!(topic_of("projects/x/a.md"), None);
        assert_eq!(topic_of("general/a.md"), None);
        assert_eq!(note_chain("projects/x/cmake/a.md"), ["projects/x/cmake/README.md", "projects/x/README.md", "projects/README.md"]);
    }

    #[test]
    fn walk_skips_top_level_hidden_and_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for (p, t) in [
            ("kb.toml", ""),
            (".gitignore", "ignored.md\n"),
            ("notes.md", "x"),
            ("general/cpp/README.md", "x"),
            ("general/cpp/a.md", "x"),
            ("general/cpp/ignored.md", "x"),
            ("general/.gitkeep", ""),
        ] {
            std::fs::create_dir_all(root.join(p).parent().unwrap()).unwrap();
            std::fs::write(root.join(p), t).unwrap();
        }
        let snap = Snapshot::from_dir(root).unwrap();
        let paths: Vec<_> = snap.files.keys().map(String::as_str).collect();
        assert_eq!(paths, ["general/cpp/README.md", "general/cpp/a.md", "kb.toml"]);
    }
}
