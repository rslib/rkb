use std::path::Path;
use std::process::{Command, Stdio};

use serde::Serialize;

use crate::config::{self, ProjectNote};
use crate::error::Result;
use crate::git;
use crate::init::PRE_COMMIT_HOOK;
use crate::kb;
use crate::lint::{self, LintEnv, Severity};
use crate::matching::{Place, Rule};
use crate::request::{self, Action, Choice, Decision, Request};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub level: Level,
    pub detail: String,
    pub fix: Option<String>,
}

fn check(name: &'static str, level: Level, detail: impl Into<String>, fix: Option<String>) -> Check {
    Check { name, level, detail: detail.into(), fix }
}

fn first_line(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").trim().to_string())
}

fn on_path(program: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(program)).find(|p| p.is_file()).map(|p| p.display().to_string())
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// A shell command that writes the pre-commit hook to `path`.
pub fn hook_fix(path: &Path) -> String {
    let lines: Vec<String> = PRE_COMMIT_HOOK.lines().map(shell_quote).collect();
    let p = shell_quote(&path.display().to_string());
    format!("printf '%s\\n' {} > {p} && chmod +x {p}", lines.join(" "))
}

pub fn lock_path(root: &Path) -> Option<std::path::PathBuf> {
    let git_dir = String::from_utf8_lossy(&git::run(root, &["rev-parse", "--git-dir"]).ok()?).trim().to_string();
    Some(root.join(git_dir).join("rkb.lock"))
}

/// Runs every check. Changes nothing.
pub fn run(root: &Path, place: &Place, state_dir: &Path, config_dir: &Path, env: &LintEnv) -> Vec<Check> {
    let mut out = vec![];
    out.push(match first_line("git", &["--version"]) {
        Some(v) => check("git", Level::Ok, v, None),
        None => check("git", Level::Fail, "git does not run", Some("install git".into())),
    });
    out.push(match first_line("bash", &["--version"]) {
        Some(v) => check("bash", Level::Ok, v, None),
        None => check("bash", Level::Fail, "bash does not run", Some("install bash; Check and Probe scripts need it".into())),
    });
    out.push(match on_path("rkb") {
        Some(p) => check("rkb on PATH", Level::Ok, p, None),
        None => check(
            "rkb on PATH",
            Level::Fail,
            "the pre-commit hook runs `rkb`, and it is not on PATH",
            Some("run `cargo install --path crates/rkb` in the rkb repository, or add its folder to PATH".into()),
        ),
    });

    let kb_ok = kb::open(root).is_ok();
    out.push(if kb_ok {
        check("knowledge base", Level::Ok, root.display().to_string(), None)
    } else {
        check(
            "knowledge base",
            Level::Fail,
            format!("{} has no kb.toml or no git repository", root.display()),
            Some("run `rkb init`, or set RKB_HOME to your knowledge base".into()),
        )
    });
    if kb_ok {
        kb_checks(root, place, state_dir, env, &mut out);
    }
    out.push(trust_check(config_dir));
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    out.extend(skill_checks(&home));
    out.extend(hook_checks(&home, state_dir));
    out
}

/// For each harness with rkb's hooks installed: whether they match this rkb, and when the harness last called one.
pub fn hook_checks(home: &Path, state_dir: &Path) -> Vec<Check> {
    use crate::install::Harness;
    let mut out = vec![];
    for h in Harness::ALL {
        let name = match h {
            Harness::Claude => "hooks: claude",
            Harness::Pi => "hooks: pi",
            Harness::Omp => "hooks: omp",
        };
        let session = match h {
            Harness::Claude => "Claude Code",
            Harness::Pi => "pi",
            Harness::Omp => "omp",
        };
        if h == Harness::Claude && crate::install::hooks_installed(home) {
            out.push(check(
                "hooks: claude",
                Level::Warn,
                "rkb hook entries from an earlier rkb are still in ~/.claude/settings.json",
                Some("run `rkb install claude` in a terminal; the plugin carries the hooks now".into()),
            ));
        }
        match crate::install::extension_status(h, home) {
            None if crate::install::installed_plugin_version(home).is_none() => continue,
            Some((false, _)) => continue,
            Some((true, false)) => {
                out.push(check(
                    name,
                    Level::Warn,
                    "extension differs from this rkb",
                    Some(format!("run `rkb install {}` in a terminal", h.name())),
                ));
                continue;
            }
            _ => {}
        }
        out.push(match crate::hooks::last_beat(state_dir, h.trust_name()) {
            Some(s) => check(name, Level::Ok, format!("last called {}", age(s)), None),
            None => check(
                name,
                Level::Warn,
                "installed but never called",
                Some(format!("start a new {session} session; hooks load when a session starts")),
            ),
        });
    }
    out
}

fn age(seconds: u64) -> String {
    match seconds {
        s if s < 60 => format!("{s}s ago"),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86400 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86400),
    }
}

/// One check per detected harness: is the rkb skill installed and equal to this binary's.
pub fn skill_checks(home: &Path) -> Vec<Check> {
    use crate::install::Harness;
    Harness::ALL
        .into_iter()
        .filter(|h| h.detected(home))
        .map(|h| {
            let name = match h {
                Harness::Claude => "plugin: claude",
                Harness::Pi => "skill: pi",
                Harness::Omp => "skill: omp",
            };
            let fix = Some(format!("run `rkb install {}` in a terminal", h.name()));
            match crate::install::status(h, home) {
                (true, true) => check(name, Level::Ok, "installed and current", None),
                (true, false) => check(name, Level::Warn, "installed, but differs from this rkb", fix),
                (false, _) => check(name, Level::Warn, "not installed", fix),
            }
        })
        .collect()
}

fn kb_checks(root: &Path, place: &Place, state_dir: &Path, env: &LintEnv, out: &mut Vec<Check>) {
    let hook = git::run(root, &["rev-parse", "--git-path", "hooks/pre-commit"])
        .map(|o| root.join(String::from_utf8_lossy(&o).trim()))
        .unwrap_or_else(|_| root.join(".git/hooks/pre-commit"));
    let installed = std::fs::read_to_string(&hook).is_ok_and(|t| t.contains("rkb lint --staged"));
    out.push(if installed {
        check("pre-commit hook", Level::Ok, hook.display().to_string(), None)
    } else {
        check("pre-commit hook", Level::Fail, format!("{} does not run `rkb lint --staged`", hook.display()), Some(hook_fix(&hook)))
    });

    out.push(match git::check_identity(root) {
        Ok(()) => check("git identity", Level::Ok, "user.name and user.email are set", None),
        Err(_) => check(
            "git identity",
            Level::Fail,
            "git has no user.name or user.email in its config",
            Some("run `git config --global user.name \"Your Name\"` and `git config --global user.email you@example.org`".into()),
        ),
    });

    let deny = git::run(root, &["config", "--get", "receive.denyCurrentBranch"]).map(|o| String::from_utf8_lossy(&o).trim().to_string());
    out.push(match deny.as_deref() {
        Ok("updateInstead") => check("push to this clone", Level::Ok, "receive.denyCurrentBranch is updateInstead", None),
        _ => check(
            "push to this clone",
            Level::Warn,
            "a push from another machine into this clone would be refused",
            Some(format!("git -C {} config receive.denyCurrentBranch updateInstead", shell_quote(&root.display().to_string()))),
        ),
    });

    match lint::lint_tree(root, env) {
        Ok(findings) => {
            let errors = findings.iter().filter(|f| f.severity == Severity::Error).count();
            let warnings = findings.len() - errors;
            let level = if errors > 0 {
                Level::Fail
            } else if warnings > 0 {
                Level::Warn
            } else {
                Level::Ok
            };
            let fix = (level != Level::Ok).then(|| "run `rkb lint` and fix what it reports".to_string());
            out.push(check("lint", level, format!("{errors} errors, {warnings} warnings"), fix));
        }
        Err(e) => out.push(check("lint", Level::Fail, e.to_string(), Some("run `rkb lint`".into()))),
    }

    if let Some(c) = site_workflow(root) {
        out.push(c);
    }
    if let Some(c) = site_approvals(root, state_dir) {
        out.push(c);
    }

    let changes = git::run(root, &["status", "--porcelain"]).map(|o| String::from_utf8_lossy(&o).lines().count()).unwrap_or(0);
    out.push(if changes == 0 {
        check("uncommitted changes", Level::Ok, "none", None)
    } else {
        check(
            "uncommitted changes",
            Level::Warn,
            format!("{changes} files changed outside rkb"),
            Some(format!("review them with `git -C {} status`, then commit them (the hook lints them) or restore them", root.display())),
        )
    });

    let pending = request::pending(&state_dir.join("requests"));
    out.push(if pending == 0 {
        check("pending requests", Level::Ok, "none", None)
    } else {
        check(
            "pending requests",
            Level::Warn,
            format!("{pending} waiting for the user"),
            Some("answer them with `rkb confirm`, or let them expire after an hour".into()),
        )
    });

    if let Some(lock) = lock_path(root) {
        out.push(match std::fs::read_to_string(&lock) {
            Ok(holder) => check(
                "write lock",
                Level::Warn,
                format!("held by {}", holder.trim()),
                Some("if no rkb command is running, run `rkb doctor --break-lock`".into()),
            ),
            Err(_) => check("write lock", Level::Ok, "free", None),
        });
    }

    let project = match &place.project {
        Some(p) => format!("project {} ({})", p.name, p.rule.describe()),
        None => "no project".into(),
    };
    let system = match &place.system {
        Some(s) => format!("system {} ({})", s.name, s.rule.describe()),
        None => "no system".into(),
    };
    out.push(check("place", Level::Ok, format!("{project}, {system}"), None));
    if let Some(c) = root_commit_missing(root, place) {
        out.push(c);
    }
}

/// Lessons the site would publish but the publish record has not seen, with this shell's passwords.
/// No check without `[sinks.web]`; a knowledge base the site cannot build is `rkb site build`'s to report.
fn site_approvals(root: &Path, state_dir: &Path) -> Option<Check> {
    let s = crate::site::collect(root, |v| std::env::var(v).is_ok_and(|x| !x.is_empty())).ok()?;
    let (clear, enc) = crate::site::pending(&s, &crate::site::published(root, state_dir));
    if clear.is_empty() && enc.is_empty() {
        return Some(check("site approvals", Level::Ok, "every lesson the site publishes is recorded", None));
    }
    let title = |id: &str| {
        ["lessons", "protected"]
            .iter()
            .flat_map(|k| s.index[*k].as_array().into_iter().flatten())
            .find(|l| l["id"] == id)
            .and_then(|l| l["title"].as_str())
            .unwrap_or_default()
            .to_string()
    };
    let names: Vec<String> = clear.iter().chain(&enc).map(|i| format!("{i} {}", title(i))).collect();
    Some(check(
        "site approvals",
        Level::Warn,
        format!("{} lessons wait for a first-publication yes, so CI leaves them out: {}", names.len(), names.join("; ")),
        Some("run `rkb site build`, answer `publish`, and push the knowledge base".into()),
    ))
}

/// The rkb version `.github/workflows/site.yml` downloads, against the running one.
fn site_workflow(root: &Path) -> Option<Check> {
    const PATH: &str = ".github/workflows/site.yml";
    let text = std::fs::read_to_string(root.join(PATH)).ok()?;
    let pinned = text.lines().find_map(|l| l.trim().strip_prefix("RKB_VERSION:"))?.trim().trim_matches('"').to_string();
    let ours = env!("CARGO_PKG_VERSION");
    Some(if pinned == ours {
        check("site workflow", Level::Ok, format!("{PATH} uses rkb {ours}"), None)
    } else {
        check(
            "site workflow",
            Level::Warn,
            format!("{PATH} downloads rkb {pinned}, but this is rkb {ours}"),
            Some(format!("set `RKB_VERSION: \"{ours}\"` in {PATH} once rkb {ours} is released, or delete the file and run `rkb site ci`")),
        )
    })
}

/// A project matched by remote whose note lacks the repository's single root commit.
fn root_commit_missing(root: &Path, place: &Place) -> Option<Check> {
    let p = place.project.as_ref().filter(|p| matches!(p.rule, Rule::Remote(_)))?;
    let [commit] = place.repo.as_ref()?.root_commits.as_slice() else { return None };
    let note = root.join("projects").join(&p.name).join("README.md");
    let parsed: ProjectNote = config::parse_note(&std::fs::read_to_string(&note).ok()?).ok()?;
    parsed.root_commit.is_none().then(|| {
        check(
            "project root commit",
            Level::Warn,
            format!("projects/{}/README.md has no root_commit, so a fork or a moved remote will not match", p.name),
            Some(format!("add `root_commit: {commit}` to the frontmatter of projects/{}/README.md", p.name)),
        )
    })
}

fn trust_check(config_dir: &Path) -> Check {
    let path = config_dir.join("trust.toml");
    let harness = request::trusted_harness(config_dir);
    match std::fs::read_to_string(&path) {
        Err(_) => check("trust.toml", Level::Ok, "none; `rkb confirm` asks on the terminal", None),
        Ok(text) => match toml::from_str::<toml::Table>(&text) {
            Err(e) => check(
                "trust.toml",
                Level::Fail,
                format!("{}: {}", path.display(), e.message()),
                Some(format!("fix the syntax of {}", path.display())),
            ),
            Ok(_) => match harness {
                Some(h) => check("trust.toml", Level::Ok, format!("{h} is trusted; `rkb confirm` relies on its permission prompt"), None),
                None => check("trust.toml", Level::Ok, "no trusted harness is running; `rkb confirm` asks on the terminal", None),
            },
        },
    }
}

/// A request to remove the write lock, or `None` when there is no lock.
pub fn break_lock_request(root: &Path, state_dir: &Path) -> Result<Option<Request>> {
    let Some(lock) = lock_path(root) else { return Ok(None) };
    let Ok(holder) = std::fs::read_to_string(&lock) else { return Ok(None) };
    let req = Request {
        id: request::new_id(),
        created: request::now(),
        action: Action::BreakLock { path: lock.display().to_string(), holder: holder.clone() },
        approved: vec![],
        question: format!(
            "The knowledge base write lock is held by `{}`. Remove it? Only do this when no rkb command is running.",
            holder.trim()
        ),
        choices: vec![
            Choice { text: "break lock".into(), decision: Some(Decision::BreakLock) },
            Choice { text: "cancel".into(), decision: None },
        ],
    };
    request::save(&state_dir.join("requests"), &req)?;
    Ok(Some(req))
}

/// Removes the lock at `path` only when it still holds `holder`. Returns whether it did.
pub fn break_lock(path: &str, holder: &str) -> bool {
    std::fs::read_to_string(path).is_ok_and(|now| now == holder) && std::fs::remove_file(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_check_uses_heartbeat() {
        let dir = tempfile::tempdir().unwrap();
        let (home, state) = (dir.path().join("home"), dir.path().join("state"));
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        assert!(hook_checks(&home, &state).is_empty());
        let omp = [crate::install::Harness::Omp];
        crate::install::apply(&crate::install::plan(&home, &dir.path().join("cfg"), &omp, false), "S").unwrap();
        // The plugin as Claude Code records it; the unit tests never run a real `claude`.
        std::fs::create_dir_all(home.join(".claude/plugins")).unwrap();
        std::fs::write(home.join(".claude/plugins/installed_plugins.json"), r#"{"version":2,"plugins":{"rkb@rkb":[{"version":"x"}]}}"#)
            .unwrap();
        let checks = hook_checks(&home, &state);
        let summary: Vec<(&str, Level, &str)> = checks.iter().map(|c| (c.name, c.level, c.detail.as_str())).collect();
        assert_eq!(
            summary,
            [("hooks: claude", Level::Warn, "installed but never called"), ("hooks: omp", Level::Warn, "installed but never called")]
        );
        crate::hooks::beat(&state, "omp").unwrap();
        let checks = hook_checks(&home, &state);
        assert_eq!((checks[0].level, checks[1].level), (Level::Warn, Level::Ok));
        std::fs::write(home.join(".omp/agent/extensions/rkb.ts"), "// Generated by rkb install; old\n").unwrap();
        let omp = hook_checks(&home, &state).pop().unwrap();
        assert_eq!((omp.level, omp.fix.as_deref()), (Level::Warn, Some("run `rkb install omp` in a terminal")));
    }

    #[test]
    fn skill_checks_per_detected_harness() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::create_dir_all(home.join(".pi/agent/skills/rkb")).unwrap();
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(home.join(".pi/agent/skills/rkb/SKILL.md"), "old text").unwrap();
        let c = skill_checks(home);
        let got: Vec<(&str, Level)> = c.iter().map(|c| (c.name, c.level)).collect();
        assert_eq!(got, [("plugin: claude", Level::Warn), ("skill: pi", Level::Warn)]);
        assert_eq!(c[1].detail, "installed, but differs from this rkb");
        assert_eq!(c[1].fix.as_deref(), Some("run `rkb install pi` in a terminal"));
        std::fs::write(home.join(".pi/agent/skills/rkb/SKILL.md"), crate::install::SKILL).unwrap();
        assert_eq!(skill_checks(home)[1].level, Level::Ok);
    }
}
