use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use globset::Glob;

use crate::config::{self, ProjectNote, SystemNote};
use crate::error::{Result, io};
use crate::git;
use crate::kb::{FileKind, NoteRole, Snapshot, note_role};

/// Why a project or system matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    Flag,
    Env(&'static str),
    Remote(String),
    RootCommit(String),
    MatchEnv(String),
    Hostname(String),
    /// A checkout path recorded on this machine, for a project with no git remote or no git at all.
    Checkout(String),
}

impl Rule {
    /// One word, for `rkb context`.
    pub fn short(&self) -> &'static str {
        match self {
            Rule::Flag => "flag",
            Rule::Env(_) => "env",
            Rule::Remote(_) => "remote",
            Rule::RootCommit(_) => "root commit",
            Rule::MatchEnv(_) => "env",
            Rule::Hostname(_) => "hostname",
            Rule::Checkout(_) => "checkout",
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Rule::Flag => "--project or --system flag".into(),
            Rule::Env(var) => format!("{var} is set"),
            Rule::Remote(r) => format!("remote {r}"),
            Rule::RootCommit(c) => format!("root commit {}", &c[..c.len().min(12)]),
            Rule::MatchEnv(k) => format!("environment {k}"),
            Rule::Hostname(g) => format!("hostname {g}"),
            Rule::Checkout(p) => format!("checkout {p} on this machine"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matched {
    pub name: String,
    pub rule: Rule,
}

/// The git repository around the current directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub top: PathBuf,
    /// Normalized, `origin` first.
    pub remotes: Vec<String>,
    pub root_commits: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Place {
    pub project: Option<Matched>,
    pub system: Option<Matched>,
    pub repo: Option<Repo>,
    /// The project root around the current directory: see `project_root`.
    pub dir: Option<PathBuf>,
}

/// The nearest folder at or above `cwd` that holds a version-control folder of any kind, else `cwd`.
/// Projects kept in hg, jj, svn, darcs, fossil or no version control are found this way too.
pub fn project_root(cwd: &Path) -> PathBuf {
    const MARKERS: [&str; 6] = [".git", ".hg", ".jj", ".svn", "_darcs", ".fossil"];
    cwd.ancestors().find(|d| MARKERS.iter().any(|m| d.join(m).exists())).unwrap_or(cwd).to_path_buf()
}

/// Names given with `--project` and `--system`.
#[derive(Debug, Clone, Default)]
pub struct Hints {
    pub project: Option<String>,
    pub system: Option<String>,
}

/// The `hostname` that `ssh -G` reports for `host`, without prompting.
pub fn ssh_hostname(host: &str) -> Option<String> {
    let out = Command::new("ssh").args(["-G", "-o", "BatchMode=yes", host]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| l.strip_prefix("hostname ").map(|h| h.trim().to_string()))
}

fn clean_path(p: &str) -> String {
    let p = p.trim_matches('/');
    p.strip_suffix(".git").unwrap_or(p).trim_end_matches('/').to_string()
}

/// Normalizes a remote to `host/path`. `resolve` maps an ssh host alias to its real host name.
pub fn normalize(url: &str, resolve: &mut dyn FnMut(&str) -> Option<String>) -> String {
    let url = url.trim();
    let (host, path, ssh) = if let Some((scheme, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = authority.rsplit('@').next().unwrap_or(authority);
        let host = host.split(':').next().unwrap_or(host);
        (host.to_string(), path.to_string(), matches!(scheme, "ssh" | "git+ssh" | "ssh+git"))
    } else if url.starts_with('/') || url.starts_with('.') {
        return clean_path(url);
    } else if let Some((before, path)) = url.split_once(':').filter(|(b, _)| !b.contains('/')) {
        let host = before.rsplit('@').next().unwrap_or(before);
        (host.to_string(), path.to_string(), true)
    } else {
        let (host, path) = url.split_once('/').unwrap_or((url, ""));
        (host.to_string(), path.to_string(), false)
    };
    let host = if ssh { resolve(&host).unwrap_or(host) } else { host };
    format!("{}/{}", host.to_lowercase(), clean_path(&path))
}

/// The repository around `cwd`, or `None` outside git.
pub fn repo(cwd: &Path) -> Option<Repo> {
    let top = String::from_utf8_lossy(&git::run(cwd, &["rev-parse", "--show-toplevel"]).ok()?).trim().to_string();
    let top = PathBuf::from(top);
    let names = String::from_utf8_lossy(&git::run(&top, &["remote"]).unwrap_or_default()).into_owned();
    let mut names: Vec<&str> = names.lines().collect();
    names.sort_by_key(|n| *n != "origin");
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    let mut resolve = |h: &str| cache.entry(h.to_string()).or_insert_with(|| ssh_hostname(h)).clone();
    let remotes = names
        .iter()
        .filter_map(|n| git::run(&top, &["remote", "get-url", n]).ok())
        .map(|u| normalize(&String::from_utf8_lossy(&u), &mut resolve))
        .collect();
    let root_commits = String::from_utf8_lossy(&git::run(&top, &["rev-list", "--max-parents=0", "HEAD"]).unwrap_or_default())
        .lines()
        .map(str::to_string)
        .collect();
    Some(Repo { top, remotes, root_commits })
}

fn notes<T: serde::de::DeserializeOwned + Default>(snap: &Snapshot, role: NoteRole) -> Vec<(String, T)> {
    let mut out: Vec<(String, T)> = snap
        .of_kind(FileKind::FolderNote)
        .filter(|(p, _)| note_role(p) == role)
        .filter_map(|(p, d)| {
            let name = p.split('/').nth(1)?.to_string();
            Some((name, config::parse_note::<T>(&String::from_utf8_lossy(d)).ok()?))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn named(root: &Path, scope: &str, flag: Option<&str>, env: Option<String>, var: &'static str) -> Option<Matched> {
    let (name, rule) = match (flag, env) {
        (Some(f), _) => (f.to_string(), Rule::Flag),
        (None, Some(e)) if !e.is_empty() => (e, Rule::Env(var)),
        _ => return None,
    };
    root.join(scope).join(&name).is_dir().then_some(Matched { name, rule })
}

/// Finds the current project and system. `env` reads an environment variable; `host` is the host name.
/// Where rkb runs: the git repository, the project root (`project_root`), and the checkouts recorded on
/// this machine as `(project, path)`.
pub struct Here<'a> {
    pub repo: Option<Repo>,
    pub dir: Option<&'a Path>,
    pub checkouts: &'a [(String, PathBuf)],
}

pub fn current(root: &Path, snap: &Snapshot, here: Here, hints: &Hints, env: &dyn Fn(&str) -> Option<String>, host: &str) -> Place {
    let Here { repo, dir, checkouts } = here;
    let projects: Vec<(String, ProjectNote)> = notes(snap, NoteRole::Project);
    let mut none = |_: &str| None;
    let project = named(root, "projects", hints.project.as_deref(), env("RKB_PROJECT"), "RKB_PROJECT").or_else(|| {
        let repo = repo.as_ref()?;
        for remote in &repo.remotes {
            let owners: Vec<&String> = projects
                .iter()
                .filter(|(_, n)| n.remotes.iter().any(|r| normalize(r, &mut none) == *remote))
                .map(|(name, _)| name)
                .collect();
            if let [one] = owners.as_slice() {
                return Some(Matched { name: (*one).clone(), rule: Rule::Remote(remote.clone()) });
            }
        }
        let [root_commit] = repo.root_commits.as_slice() else { return None };
        let owners: Vec<&String> =
            projects.iter().filter(|(_, n)| n.root_commit.as_deref() == Some(root_commit)).map(|(name, _)| name).collect();
        match owners.as_slice() {
            [one] => Some(Matched { name: (*one).clone(), rule: Rule::RootCommit(root_commit.clone()) }),
            _ => None,
        }
    });
    let project = project.or_else(|| {
        // The deepest recorded checkout that holds the current folder, when exactly one project recorded it.
        let dir = dir?;
        let inside: Vec<&(String, PathBuf)> =
            checkouts.iter().filter(|(name, path)| dir.starts_with(path) && root.join("projects").join(name).is_dir()).collect();
        let deepest = inside.iter().map(|(_, p)| p.components().count()).max()?;
        let owners: Vec<&&(String, PathBuf)> = inside.iter().filter(|(_, p)| p.components().count() == deepest).collect();
        match owners.as_slice() {
            [(name, path)] => Some(Matched { name: name.clone(), rule: Rule::Checkout(path.display().to_string()) }),
            _ => None,
        }
    });

    let systems: Vec<(String, SystemNote)> = notes(snap, NoteRole::System);
    let short = host.split('.').next().unwrap_or(host);
    let system = named(root, "systems", hints.system.as_deref(), env("RKB_SYSTEM"), "RKB_SYSTEM")
        .or_else(|| {
            systems.iter().find_map(|(name, n)| {
                let all = !n.match_env.is_empty() && n.match_env.iter().all(|(k, v)| env(k).as_deref() == Some(v));
                all.then(|| Matched { name: name.clone(), rule: Rule::MatchEnv(n.match_env.keys().next().unwrap().clone()) })
            })
        })
        .or_else(|| {
            systems.iter().find_map(|(name, n)| {
                n.hostname.iter().find_map(|g| {
                    let m = Glob::new(g).ok()?.compile_matcher();
                    (m.is_match(host) || m.is_match(short)).then(|| Matched { name: name.clone(), rule: Rule::Hostname(g.clone()) })
                })
            })
        });
    Place { project, system, repo, dir: dir.map(Path::to_path_buf) }
}

/// Every recorded checkout on this machine, as `(project, path)`.
pub fn all_checkouts(state: &Path) -> Vec<(String, PathBuf)> {
    let table: toml::Table =
        std::fs::read_to_string(state.join("checkouts.toml")).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default();
    table
        .iter()
        .flat_map(|(name, v)| v.as_array().into_iter().flatten().filter_map(|p| p.as_str()).map(|p| (name.clone(), PathBuf::from(p))))
        .collect()
}

/// Adds the repository's top-level path to `project` in `checkouts.toml` once.
/// Returns whether the file changed.
/// The recorded checkout paths of `project` on this machine, in the order they were recorded.
pub fn checkouts(state: &Path, project: &str) -> Vec<std::path::PathBuf> {
    let table: toml::Table =
        std::fs::read_to_string(state.join("checkouts.toml")).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default();
    table.get(project).and_then(|v| v.as_array()).into_iter().flatten().filter_map(|v| v.as_str()).map(std::path::PathBuf::from).collect()
}

pub fn record_checkout(state: &Path, project: &str, top: &Path) -> Result<bool> {
    let path = state.join("checkouts.toml");
    let mut table: toml::Table = std::fs::read_to_string(&path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default();
    let top = top.display().to_string();
    let list = table.entry(project).or_insert_with(|| toml::Value::Array(vec![]));
    let Some(list) = list.as_array_mut() else { return Ok(false) };
    if list.iter().any(|v| v.as_str() == Some(top.as_str())) {
        return Ok(false);
    }
    list.push(top.into());
    std::fs::create_dir_all(state).map_err(io(state))?;
    let tmp = state.join(".checkouts.toml.tmp");
    std::fs::write(&tmp, toml::to_string(&table).expect("table serializes")).map_err(io(&tmp))?;
    std::fs::rename(&tmp, &path).map_err(io(&path))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkouts_are_read_back() {
        let dir = tempfile::tempdir().unwrap();
        record_checkout(dir.path(), "dftracer", Path::new("/a")).unwrap();
        record_checkout(dir.path(), "dftracer", Path::new("/b")).unwrap();
        assert_eq!(checkouts(dir.path(), "dftracer"), [std::path::PathBuf::from("/a"), std::path::PathBuf::from("/b")]);
        assert!(checkouts(dir.path(), "other").is_empty());
    }

    fn norm(url: &str) -> String {
        normalize(url, &mut |h: &str| (h == "gh").then(|| "github.com".to_string()))
    }

    #[test]
    fn normalizes_remote_forms() {
        for url in [
            "git@github.com:llnl/dftracer.git",
            "https://github.com/llnl/dftracer/",
            "ssh://git@GitHub.com/llnl/dftracer.git",
            "https://user@github.com:443/llnl/dftracer",
            "gh:llnl/dftracer",
            "github.com/llnl/dftracer",
            "GitHub.com/llnl/dftracer.git",
        ] {
            assert_eq!(norm(url), "github.com/llnl/dftracer", "{url}");
        }
        assert_eq!(norm("/srv/git/kb.git"), "srv/git/kb");
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=T", "-c", "user.email=t@example.org"])
            .args(args)
            .stdout(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "{args:?}");
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn repo_with(remote: Option<&str>) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "root"]);
        if let Some(r) = remote {
            git(dir.path(), &["remote", "add", "origin", r]);
        }
        let root = String::from_utf8(
            Command::new("git").arg("-C").arg(dir.path()).args(["rev-list", "--max-parents=0", "HEAD"]).output().unwrap().stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        (dir, root)
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn project_by_remote_root_commit_and_env() {
        let kb = tempfile::tempdir().unwrap();
        let (proj, root_commit) = repo_with(Some("https://github.com/llnl/dftracer.git"));
        write(kb.path(), "projects/dftracer/README.md", "---\nremotes:\n  - github.com/llnl/dftracer\n---\n# d\n");
        write(kb.path(), "projects/other/README.md", "# other\n");
        let snap = Snapshot::from_dir(kb.path()).unwrap();
        let place =
            current(kb.path(), &snap, Here { repo: repo(proj.path()), dir: None, checkouts: &[] }, &Hints::default(), &no_env, "laptop");
        assert_eq!(place.project, Some(Matched { name: "dftracer".into(), rule: Rule::Remote("github.com/llnl/dftracer".into()) }));
        assert_eq!(place.repo.as_ref().unwrap().root_commits, std::slice::from_ref(&root_commit));

        let env = |k: &str| (k == "RKB_PROJECT").then(|| "other".to_string());
        let place =
            current(kb.path(), &snap, Here { repo: repo(proj.path()), dir: None, checkouts: &[] }, &Hints::default(), &env, "laptop");
        assert_eq!(place.project.unwrap().rule, Rule::Env("RKB_PROJECT"));

        let (fork, fork_root) = repo_with(None);
        for p in ["a", "b"] {
            write(kb.path(), &format!("projects/{p}/README.md"), &format!("---\nroot_commit: {fork_root}\n---\n"));
        }
        let snap = Snapshot::from_dir(kb.path()).unwrap();
        assert_eq!(
            current(kb.path(), &snap, Here { repo: repo(fork.path()), dir: None, checkouts: &[] }, &Hints::default(), &no_env, "h").project,
            None
        );
        std::fs::remove_dir_all(kb.path().join("projects/b")).unwrap();
        let snap = Snapshot::from_dir(kb.path()).unwrap();
        let p = current(kb.path(), &snap, Here { repo: repo(fork.path()), dir: None, checkouts: &[] }, &Hints::default(), &no_env, "h")
            .project
            .unwrap();
        assert_eq!((p.name.as_str(), p.rule.short()), ("a", "root commit"));

        let outside = tempfile::tempdir().unwrap();
        assert!(repo(outside.path()).is_none());
    }

    #[test]
    fn system_by_env_and_hostname() {
        let kb = tempfile::tempdir().unwrap();
        write(
            kb.path(),
            "systems/tuolumne/README.md",
            "---\nhostname:\n  - \"tuolumne*\"\n  - \"tuo[0-9]*\"\nmatch_env:\n  LCSCHEDCLUSTER: tuolumne\n---\n",
        );
        let snap = Snapshot::from_dir(kb.path()).unwrap();
        let env = |k: &str| (k == "LCSCHEDCLUSTER").then(|| "tuolumne".to_string());
        let s =
            current(kb.path(), &snap, Here { repo: None, dir: None, checkouts: &[] }, &Hints::default(), &env, "laptop").system.unwrap();
        assert_eq!(s.rule, Rule::MatchEnv("LCSCHEDCLUSTER".into()));
        let s =
            current(kb.path(), &snap, Here { repo: None, dir: None, checkouts: &[] }, &Hints::default(), &no_env, "tuo42").system.unwrap();
        assert_eq!(s.rule, Rule::Hostname("tuo[0-9]*".into()));
        let s =
            current(kb.path(), &snap, Here { repo: None, dir: None, checkouts: &[] }, &Hints::default(), &no_env, "tuolumne1011.llnl.gov")
                .system
                .unwrap();
        assert_eq!(s.name, "tuolumne");
        assert!(
            current(kb.path(), &snap, Here { repo: None, dir: None, checkouts: &[] }, &Hints::default(), &no_env, "laptop")
                .system
                .is_none()
        );
        let hints = Hints { project: None, system: Some("tuolumne".into()) };
        assert_eq!(
            current(kb.path(), &snap, Here { repo: None, dir: None, checkouts: &[] }, &hints, &no_env, "laptop").system.unwrap().rule,
            Rule::Flag
        );
    }

    #[test]
    fn checkout_recorded_once() {
        let state = tempfile::tempdir().unwrap();
        assert!(record_checkout(state.path(), "dftracer", Path::new("/work/dftracer")).unwrap());
        let before = std::fs::metadata(state.path().join("checkouts.toml")).unwrap().modified().unwrap();
        assert!(!record_checkout(state.path(), "dftracer", Path::new("/work/dftracer")).unwrap());
        assert_eq!(std::fs::metadata(state.path().join("checkouts.toml")).unwrap().modified().unwrap(), before);
        let text = std::fs::read_to_string(state.path().join("checkouts.toml")).unwrap();
        assert_eq!(text.matches("/work/dftracer").count(), 1);
    }

    #[test]
    fn project_by_recorded_checkout() {
        let kb = tempfile::tempdir().unwrap();
        for p in ["projects/tool/README.md", "projects/sub/README.md", "projects/twin/README.md"] {
            write(kb.path(), p, "# x\n");
        }
        let snap = Snapshot::from_dir(kb.path()).unwrap();
        let recorded = vec![
            ("tool".to_string(), PathBuf::from("/work/tool")),
            ("sub".to_string(), PathBuf::from("/work/tool/vendor/sub")),
            ("gone".to_string(), PathBuf::from("/work/gone")),
        ];
        let at = |dir: &str, rec: &[(String, PathBuf)]| {
            current(kb.path(), &snap, Here { repo: None, dir: Some(Path::new(dir)), checkouts: rec }, &Hints::default(), &no_env, "h")
                .project
        };
        let p = at("/work/tool/src", &recorded).unwrap();
        assert_eq!((p.name.as_str(), p.rule.short()), ("tool", "checkout"));
        assert_eq!(at("/work/tool/vendor/sub/x", &recorded).unwrap().name, "sub", "the deepest checkout wins");
        assert_eq!(at("/work/gone", &recorded), None, "a checkout of a project with no folder does not count");
        assert_eq!(at("/elsewhere", &recorded), None);
        let shared = vec![("tool".to_string(), PathBuf::from("/w")), ("twin".to_string(), PathBuf::from("/w"))];
        assert_eq!(at("/w/a", &shared), None, "two projects at one path match neither");
    }

    #[test]
    fn project_root_finds_any_version_control() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("proj");
        std::fs::create_dir_all(root.join(".hg")).unwrap();
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        assert_eq!(project_root(&root.join("src/deep")), root);
        let plain = d.path().join("plain/src");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(project_root(&plain), plain, "no version control: the folder itself");
    }
}
