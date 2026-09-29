use std::path::Path;

use serde::Serialize;

use crate::config::{self, KbConfig};
use crate::error::{Error, Result};
use crate::git;
use crate::lint::{self, Finding, LintEnv, Severity};
use crate::write::kb_lock;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Commit {
    pub hash: String,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Synced {
    pub remote: String,
    pub branch: String,
    pub pulled: Vec<Commit>,
    pub pushed: Vec<Commit>,
    /// Lint findings after the pull. With any error, nothing was pushed.
    pub findings: Vec<Finding>,
}

impl Synced {
    pub fn blocked(&self) -> bool {
        self.findings.iter().any(|f| f.severity == Severity::Error)
    }
}

fn text(root: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8_lossy(&git::run(root, args)?).trim().to_string())
}

fn commits(root: &Path, range: &str) -> Result<Vec<Commit>> {
    Ok(text(root, &["log", "--format=%h%x09%s", range])?
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(h, s)| Commit { hash: h.into(), subject: s.into() })
        .collect())
}

fn not_ready(reason: impl Into<String>, fix: impl Into<String>) -> Error {
    Error::NotReady { reason: reason.into(), fix: fix.into() }
}

/// The other side of a sync.
pub enum Other<'a> {
    /// A git remote; `None` means the branch's upstream remote, else `origin`.
    Remote(Option<&'a str>),
    /// A bundle file that both machines read and write.
    Bundle(&'a Path),
}

/// Writes `branch` to `file` through a temporary file in the same folder, so a reader never sees half a bundle.
fn write_bundle(root: &Path, file: &Path, branch: &str) -> Result<()> {
    let dir = file.parent().unwrap_or(Path::new("."));
    let tmp = dir.join(format!(".{}.rkb-tmp", file.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
    git::run(root, &["bundle", "create", "-q", &tmp.display().to_string(), branch])?;
    std::fs::rename(&tmp, file).map_err(crate::error::io(file))
}

/// Pulls with rebase from `other`, lints the result, and pushes only when lint has no errors.
/// Holds the write lock throughout. Never resolves a conflict, resets or forces a push.
pub fn sync(root: &Path, other: Other, env: &LintEnv) -> Result<Synced> {
    let kb: KbConfig = std::fs::read_to_string(root.join("kb.toml")).ok().and_then(|t| config::parse(&t).ok()).unwrap_or_default();
    let _lock = kb_lock(root, &kb)?;
    let kb_path = root.display().to_string();

    // A rebase detaches HEAD, so this check comes before the branch check.
    let git_dir = root.join(text(root, &["rev-parse", "--git-dir"])?);
    if ["rebase-merge", "rebase-apply", "MERGE_HEAD"].iter().any(|p| git_dir.join(p).exists()) {
        return Err(not_ready(
            "a rebase or merge is in progress",
            format!(
                "finish it with `git -C {kb_path} rebase --continue`, or undo it with `git -C {kb_path} rebase --abort`, then run `rkb sync` again"
            ),
        ));
    }
    let branch = text(root, &["symbolic-ref", "--short", "-q", "HEAD"]).map_err(|_| {
        not_ready("HEAD is detached, so there is no branch to sync", format!("check out your branch: git -C {kb_path} switch <branch>"))
    })?;
    crate::usage::commit(root)?;
    let status = git::run(root, &["status", "--porcelain"])?;
    let dirty: Vec<String> = String::from_utf8_lossy(&status).lines().map(|l| l.get(3..).unwrap_or(l).to_string()).collect();
    if !dirty.is_empty() {
        let shown: Vec<&str> = dirty.iter().take(10).map(String::as_str).collect();
        let more = if dirty.len() > 10 { format!(" and {} more", dirty.len() - 10) } else { String::new() };
        return Err(not_ready(
            format!("uncommitted changes: {}{more}", shown.join(", ")),
            format!("see them with `git -C {kb_path} status`; commit or discard them, then run `rkb sync` again"),
        ));
    }

    let (source, bundle) = match other {
        Other::Remote(remote) => {
            let remotes: Vec<String> = text(root, &["remote"])?.lines().map(str::to_string).collect();
            let remote = match remote {
                Some(r) => r.to_string(),
                None => text(root, &["config", &format!("branch.{branch}.remote")]).unwrap_or_else(|_| "origin".into()),
            };
            if !remotes.contains(&remote) {
                return Err(Error::NoRemote { name: remote, remotes });
            }
            (remote, None)
        }
        Other::Bundle(file) => {
            let file = std::path::absolute(file).map_err(crate::error::io(file))?;
            (file.display().to_string(), Some(file))
        }
    };

    // The other side's tip of this branch, or None when it has no such branch yet.
    let head_ref = format!("refs/heads/{branch}");
    let tip = match &bundle {
        Some(file) if !file.exists() => None,
        Some(file) => {
            if git::run(root, &["bundle", "verify", "-q", &source]).is_err() {
                return Err(not_ready(
                    format!("{} is not a git bundle this knowledge base can read", file.display()),
                    "check the file; `rkb sync --bundle` writes one when the file does not exist yet",
                ));
            }
            let listed = text(root, &["ls-remote", &source, &head_ref])?;
            let Some(tip) = listed.split_whitespace().next().map(str::to_string) else {
                return Err(not_ready(
                    format!("{} has no branch {branch}", file.display()),
                    format!("check out the branch the bundle holds, or write a new bundle from {branch}"),
                ));
            };
            Some(tip)
        }
        None => text(root, &["ls-remote", "--heads", &source, &head_ref])?.split_whitespace().next().map(str::to_string),
    };

    let before = text(root, &["rev-parse", "HEAD"])?;
    let pulled = match &tip {
        None => vec![],
        Some(_) => {
            git::run(root, &["fetch", "-q", &source, &branch])?;
            if git::run(root, &["merge-base", "HEAD", "FETCH_HEAD"]).is_err() {
                return Err(not_ready(
                    format!("{source} shares no history with this knowledge base"),
                    "check that the remote or the bundle belongs to this knowledge base; nothing was changed",
                ));
            }
            if let Err(e) = git::run(root, &["rebase", "-q", "FETCH_HEAD"]) {
                let files: Vec<String> = text(root, &["diff", "--name-only", "--diff-filter=U"])?.lines().map(str::to_string).collect();
                return Err(if files.is_empty() { e } else { Error::RebaseConflict { root: root.to_path_buf(), files } });
            }
            commits(root, &format!("{before}..FETCH_HEAD"))?
        }
    };
    let pushed_range = tip.as_ref().map_or_else(|| "HEAD".to_string(), |t| format!("{t}..HEAD"));

    let findings = lint::lint_tree(root, env)?;
    let mut out = Synced { remote: source, branch, pulled, pushed: vec![], findings };
    if out.blocked() {
        return Ok(out);
    }
    let pushed = commits(root, &pushed_range)?;
    if !pushed.is_empty() {
        match &bundle {
            Some(file) => write_bundle(root, file, &out.branch)?,
            None => {
                let mut args = vec!["push", "-q"];
                if tip.is_none() {
                    args.push("-u");
                }
                args.extend([out.remote.as_str(), out.branch.as_str()]);
                git::run(root, &args)?;
            }
        }
    }
    out.pushed = pushed;
    Ok(out)
}
