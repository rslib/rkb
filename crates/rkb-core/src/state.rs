use std::path::Path;

use crate::error::Result;
use crate::git;
use crate::kb::{self, FileKind, Snapshot, classify};
use crate::matching::{self, Hints, Place};
use crate::request;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub place: Place,
    /// Lessons in the current project folder, when there is one.
    pub project_lessons: Option<usize>,
    pub system_lessons: Option<usize>,
    pub general_lessons: usize,
    pub pending: usize,
    pub uncommitted: usize,
}

/// Matches the current place, reading only folder notes, and records the checkout of a matched project.
pub fn locate(root: &Path, cwd: &Path, hints: &Hints, state_dir: &Path) -> Result<Place> {
    let notes = Snapshot::from_dir_where(root, |p| classify(p) == FileKind::FolderNote)?;
    let env = |k: &str| std::env::var(k).ok();
    let dir = matching::project_root(cwd);
    let checkouts = matching::all_checkouts(state_dir);
    let here = matching::Here { repo: matching::repo(cwd), dir: Some(&dir), checkouts: &checkouts };
    let place = matching::current(root, &notes, here, hints, &env, crate::lock::hostname());
    if let (Some(p), Some(r)) = (&place.project, &place.repo) {
        matching::record_checkout(state_dir, &p.name, &r.top)?;
    }
    Ok(place)
}

/// Counts from file paths only, so it stays fast on large knowledge bases.
pub fn build(root: &Path, place: Place, state_dir: &Path) -> Result<State> {
    let lessons: Vec<String> = kb::paths(root)?.into_iter().filter(|p| classify(p) == FileKind::Lesson).collect();
    let under = |folder: &str| lessons.iter().filter(|p| p.starts_with(&format!("{folder}/"))).count();
    let project_lessons = place.project.as_ref().map(|p| under(&format!("projects/{}", p.name)));
    let system_lessons = place.system.as_ref().map(|s| under(&format!("systems/{}", s.name)));
    let uncommitted = String::from_utf8_lossy(&git::run(root, &["status", "--porcelain"])?).lines().count();
    Ok(State {
        project_lessons,
        system_lessons,
        general_lessons: under("general"),
        pending: request::pending(&state_dir.join("requests")),
        uncommitted,
        place,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matching::{Matched, Rule};
    use crate::request::{Action, Request, save};

    #[test]
    fn counts_and_pending() {
        let kb = tempfile::tempdir().unwrap();
        let root = kb.path();
        assert!(std::process::Command::new("git").arg("-C").arg(root).args(["init", "-q"]).status().unwrap().success());
        for p in
            ["general/cpp/a.md", "general/git/b.md", "projects/d/cmake/c.md", "projects/d/io/e.md", "projects/x/t/f.md", "systems/t/l/g.md"]
        {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, "x").unwrap();
        }
        let state = tempfile::tempdir().unwrap();
        let req = |created| Request {
            id: crate::request::new_id(),
            created,
            action: Action::Flag { id: "a".into(), reason: "b".into() },
            approved: vec![],
            question: "q".into(),
            choices: vec![],
        };
        save(&state.path().join("requests"), &req(crate::request::now())).unwrap();
        save(&state.path().join("requests"), &req(0)).unwrap();
        let place = Place {
            project: Some(Matched { name: "d".into(), rule: Rule::Flag }),
            system: Some(Matched { name: "t".into(), rule: Rule::Flag }),
            repo: None,
            dir: None,
        };
        let s = build(root, place, state.path()).unwrap();
        assert_eq!((s.project_lessons, s.system_lessons, s.general_lessons), (Some(2), Some(1), 2));
        assert_eq!(s.pending, 1);
        assert_eq!(s.uncommitted, 3);
    }
}
