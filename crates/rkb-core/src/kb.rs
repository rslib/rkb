use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result, io};
use crate::git;
use crate::lesson::{self, Lesson, ParseError};

pub const SCOPES: [&str; 3] = ["general", "systems", "projects"];

/// The knowledge base root: `$RKB_HOME`, or `~/Personal/kb` when it is not set.
pub fn home() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    match std::env::var("RKB_HOME") {
        Ok(p) if p == "~" => home,
        Ok(p) => match p.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(p),
        },
        Err(_) => home.join("Personal/kb"),
    }
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
