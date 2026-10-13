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
        if let Some(c) = tier_check(root) {
            out.push(c);
        }
    }
    out.push(trust_check(config_dir));
    out.push(agent_sink_check(root, config_dir));
    out.push(extra_kbs_check(root, config_dir));
    out.push(observer_check(config_dir, state_dir));
    out.extend(triage_check(state_dir));
    out.push(schedule_check(state_dir));
    out.extend(recall_check(root, state_dir));
    for (session, g) in crate::write::graces(state_dir) {
        let until =
            jiff::Timestamp::from_second(g.until as i64).map(|t| t.to_zoned(jiff::tz::TimeZone::system()).strftime("%H:%M").to_string());
        out.push(check("burst grace", Level::Ok, format!("session {session} until {}", until.unwrap_or_default()), None));
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    out.extend(skill_checks(&home));
    out.extend(hook_checks(&home, state_dir));
    out.extend(permission_check(&home));
    out
}

/// The count of lessons in each memory tier, so the growth of the store and the size of the proven
/// set are visible. Reads only.
pub fn tier_check(root: &Path) -> Option<Check> {
    let snap = kb::Snapshot::from_dir(root).ok()?;
    let (lessons, _) = snap.lessons();
    let counts = crate::usage::counts(root);
    let mut n = [0usize; 3];
    for l in &lessons {
        match crate::tier::of(l.frontmatter.status, counts.get(&l.frontmatter.id)) {
            crate::tier::Tier::Candidate => n[0] += 1,
            crate::tier::Tier::Proven => n[1] += 1,
            crate::tier::Tier::Archive => n[2] += 1,
        }
    }
    Some(check("tiers", Level::Ok, format!("{} candidate, {} proven, {} archive", n[0], n[1], n[2]), None))
}

/// A warning when Claude Code's default permission mode never shows the `rkb confirm` question to a person.
pub fn permission_check(home: &Path) -> Option<Check> {
    crate::install::installed_plugin_version(home)?;
    let text = std::fs::read_to_string(home.join(".claude/settings.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let mode = v["permissions"]["defaultMode"].as_str().filter(|m| crate::hooks::mode_skips_prompts(Some(m)))?;
    Some(check(
        "claude permissions",
        Level::Warn,
        format!("Claude Code's default mode is {mode}, so the agent is denied `rkb confirm`"),
        Some("the user runs each `rkb confirm` the agent shows, in a separate terminal window or with Claude Code's `!` prefix".into()),
    ))
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
            // Hermes runs no rkb hooks, so it has no hook check.
            Harness::Hermes => continue,
        };
        let session = match h {
            Harness::Claude => "Claude Code",
            Harness::Pi => "pi",
            Harness::Omp => "omp",
            Harness::Hermes => continue,
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
        if h == Harness::Claude {
            match crate::hooks::callers(state_dir) {
                (0, _) => {}
                (n, 0) => out.push(check("hooks: claude mod", Level::Ok, format!("the mod handled {n} recent sessions"), None)),
                (n, both) => out.push(check(
                    "hooks: claude mod",
                    Level::Warn,
                    format!("{both} of {n} recent sessions ran rkb's hooks twice, through the mod and the command hooks"),
                    Some("the mod could not switch the command hooks off; report `claude --version`, then run `rkb install claude`".into()),
                )),
            }
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
                Harness::Hermes => "skill: hermes",
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

    if let Some(c) = site_approvals(root, state_dir) {
        out.push(c);
    }

    // Use records are committed with the next rkb write or `rkb sync`, so they are not "outside rkb".
    let changes = git::run(root, &["status", "--porcelain"])
        .map(|o| String::from_utf8_lossy(&o).lines().filter(|l| !l.get(3..).unwrap_or(l).starts_with(".rkb/")).count())
        .unwrap_or(0);
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
            Ok(holder) => {
                let parts: Vec<&str> = holder.split_whitespace().collect();
                let detail = match parts.as_slice() {
                    [host, pid, _] => format!("held by pid {pid} on host {host}"),
                    [pid, _] => format!("held by pid {pid}; an old lock file without a host"),
                    _ => format!("held by {}", holder.trim()),
                };
                check("write lock", Level::Warn, detail, Some("if no rkb command is running, run `rkb doctor --break-lock`".into()))
            }
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

/// Lessons `rkb publish` would publish but the record has not seen, with this shell's passwords.
/// No check without `[sinks.web]`; a knowledge base that cannot be published is `rkb publish`'s to report.
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
        Some("run `rkb publish <dir>`, answer `publish`, and push the knowledge base".into()),
    ))
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

/// How the hooks' rankings went in the last 7 days: a warning when more than a quarter fell back
/// from a backend, with the time the model rankings took, so the hook time limit can be tuned.
pub fn recall_check(root: &Path, state_dir: &Path) -> Option<Check> {
    let lines = crate::hooks::rankings(state_dir, 7);
    if lines.is_empty() {
        return None;
    }
    let skipped = |l: &serde_json::Value| l["skipped"].as_array().is_some_and(|a| !a.is_empty());
    let timed_out = |l: &serde_json::Value| {
        l["skipped"].as_array().is_some_and(|a| a.iter().any(|r| r.as_str().is_some_and(|r| r.contains(": timeout"))))
    };
    let total = lines.len();
    let fallbacks = lines.iter().filter(|l| skipped(l)).count();
    let timeouts = lines.iter().filter(|l| timed_out(l)).count();
    let mut ms: Vec<u64> = lines.iter().filter(|l| l["backend"] != crate::rerank::BM25).filter_map(|l| l["ms"].as_u64()).collect();
    ms.sort_unstable();
    let pct = |p: usize| ms.get((ms.len() * p / 100).min(ms.len().saturating_sub(1))).copied();
    let limit = std::fs::read_to_string(root.join("kb.toml"))
        .ok()
        .and_then(|t| config::parse::<config::KbConfig>(&t).ok())
        .and_then(|c| c.hooks)
        .and_then(|h| h.get("hook_timeout_ms").and_then(toml::Value::as_integer))
        .map_or(crate::hooks::DEFAULT_HOOK_TIMEOUT_MS, |v| v.max(0) as u64);
    let times = match (pct(50), pct(90)) {
        (Some(a), Some(b)) => format!("; model rankings took {a} ms (median), {b} ms (90th percentile)"),
        _ => String::new(),
    };
    let detail =
        format!("{total} hook searches in 7 days, {fallbacks} fell back from a backend ({timeouts} after the {limit} ms limit){times}");
    if total >= 10 && fallbacks * 4 > total {
        let fix = if timeouts * 2 >= fallbacks {
            let to = pct(90).map_or(limit * 2, |p| (p + 200).max(limit + 100));
            format!("set hook_timeout_ms = {to} under [hooks] in kb.toml")
        } else {
            format!("see the skipped reasons in {}", state_dir.join(crate::hooks::RANKINGS).display())
        };
        return Some(check("hook rankings", Level::Warn, detail, Some(fix)));
    }
    Some(check("hook rankings", Level::Ok, detail, None))
}

/// The observer: off without a command, a warning when it is not approved or its last 3 runs failed.
/// How triage did over the last 30 days, from `triage.jsonl`; `None` before the first verdict.
pub fn triage_check(state_dir: &Path) -> Option<Check> {
    let since = crate::request::now().saturating_sub(30 * 86400);
    let text = std::fs::read_to_string(state_dir.join(crate::triage::LOG)).ok()?;
    let mut verdicts = std::collections::BTreeMap::new();
    let mut outcomes = std::collections::BTreeMap::new();
    for v in text.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()) {
        if v["time"].as_u64().is_none_or(|t| t < since) {
            continue;
        }
        let key = (v["item"].as_str().unwrap_or_default().to_string(), v["index"].as_u64().unwrap_or_default());
        if let Some(verdict) = v["verdict"].as_str() {
            verdicts.insert(key, verdict.to_string());
        } else if let Some(op) = v["outcome"].as_str() {
            outcomes.insert(key, op.to_string());
        }
    }
    if verdicts.is_empty() {
        return None;
    }
    let total = verdicts.len();
    let unsure = verdicts.values().filter(|v| *v == "unsure").count();
    let kept: Vec<_> = verdicts.iter().filter(|(_, v)| *v == "keep").map(|(k, _)| k).collect();
    let became = kept.iter().filter(|k| outcomes.get(**k).is_some_and(|o| matches!(o.as_str(), "add" | "update" | "supersede"))).count();
    let detail = format!("{total} candidates in 30 days: {unsure} unsure, {} kept, {became} of them became lessons", kept.len());
    let fix = "move `keep` and `drop` under [triage] in config.toml closer together; `rkb inbox show <id>` shows the scores";
    if unsure * 10 > total * 3 {
        return Some(check("triage", Level::Warn, format!("{detail}; over 30% unsure"), Some(fix.into())));
    }
    if kept.len() >= 20 && became * 2 < kept.len() {
        let fix = "raise `keep` under [triage] in config.toml, since many kept candidates did not become lessons";
        return Some(check("triage", Level::Warn, format!("{detail}; under half of the kept became lessons"), Some(fix.into())));
    }
    Some(check("triage", Level::Ok, detail, None))
}

/// The agent sink in force: what agents may read and where the rule came from. A machine config
/// that fails closed is a failing check with a fix, so a blind server does not go unnoticed.
pub fn agent_sink_check(root: &Path, config_dir: &Path) -> Check {
    let fix = || {
        Some(
            "fix [sinks.agent] in the machine config.toml (it must be a table with an allow map of known label keys), or remove that table"
                .to_string(),
        )
    };
    match config::agent_sink(root, config_dir) {
        Err(e) => check("agent sink", Level::Fail, e, fix()),
        Ok((None, _)) => check("agent sink", Level::Ok, "no agent sink is set; every lesson passes to agents", None),
        Ok((Some(sink), source)) => {
            let allow = if sink.allow.is_empty() {
                "nothing (the allow list is empty)".to_string()
            } else {
                sink.allow.iter().map(|(k, v)| format!("{k}: {}", v.join(", "))).collect::<Vec<_>>().join("; ")
            };
            let from = match source {
                Some(config::AgentSinkSource::Machine) => "the machine config.toml",
                _ => "kb.toml",
            };
            check("agent sink", Level::Ok, format!("agents may read {allow} (from {from})"), None)
        }
    }
}

/// The extra read-only knowledge bases of the machine config: what a search reads, and what it had
/// to skip. A skipped folder is a warning, so a share that is down does not go unnoticed.
pub fn extra_kbs_check(root: &Path, config_dir: &Path) -> Check {
    match kb::Roots::load(root, config_dir) {
        Err(e) => {
            check("extra knowledge bases", Level::Fail, e.to_string(), Some("fix the [[extra]] tables in the machine config.toml".into()))
        }
        Ok(roots) => {
            let reading: Vec<String> = roots.extras.iter().map(|e| e.name.clone()).collect();
            let skipped: Vec<String> = roots.skipped.iter().map(|(name, why)| format!("{name} ({why})")).collect();
            let detail = match (reading.is_empty(), skipped.is_empty()) {
                (true, true) => "none are configured".to_string(),
                (false, true) => format!("reading {}", reading.join(", ")),
                (true, false) => format!("skipped: {}", skipped.join("; ")),
                (false, false) => format!("reading {}; skipped: {}", reading.join(", "), skipped.join("; ")),
            };
            let fix = (!skipped.is_empty()).then(|| "check the path in its [[extra]] table, or remove that table".to_string());
            check("extra knowledge bases", if skipped.is_empty() { Level::Ok } else { Level::Warn }, detail, fix)
        }
    }
}

pub fn observer_check(config_dir: &Path, _state_dir: &Path) -> Check {
    use crate::observer;
    let cfg = match observer::load(config_dir) {
        Ok(c) => c,
        Err(e) => return check("observer", Level::Warn, e, Some("fix [observer] in config.toml".into())),
    };
    if cfg.chains.is_empty() {
        return check("observer", Level::Ok, "off; no model chain under [observer]", None);
    }
    let chains = cfg.summary().replace('\n', "; ");
    if !observer::approved(config_dir, &cfg) {
        return check("observer", Level::Warn, format!("{chains}; not approved here"), Some("run `rkb approve observer`".into()));
    }
    check("observer", Level::Ok, format!("{chains}; approved here"), None)
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

/// The OS schedule and the last `rkb maintain` pass: whether the job is installed, when the pass ran,
/// its counts and the reason the model step was skipped. A warning when the job is installed and the
/// last pass is more than 36 hours old.
pub fn schedule_check(state_dir: &Path) -> Check {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    schedule_check_at(&home, state_dir)
}

fn schedule_check_at(home: &Path, state_dir: &Path) -> Check {
    use crate::schedule::{self, Os};
    let kind = if schedule::current_os() == Os::Macos { "launchd" } else { "systemd" };
    let times = schedule::HOURS.map(|h| format!("{h:02}:00")).join(" and ");
    let installed = std::fs::read_to_string(schedule::primary_path(home)).is_ok_and(|t| t.contains(schedule::MARK));
    let Some(last) = crate::maintain::last(state_dir) else {
        return if installed {
            check(
                "schedule",
                Level::Warn,
                format!("installed ({kind}, {times}); no pass has run yet"),
                Some("wait for the next run, or run `rkb maintain` once".into()),
            )
        } else {
            check("schedule", Level::Ok, "not installed; run `rkb schedule install`", None)
        };
    };
    let age = crate::request::now().saturating_sub(last.time);
    let at = jiff::Timestamp::from_second(last.time as i64)
        .map(|t| t.to_zoned(jiff::tz::TimeZone::UTC).strftime("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default();
    let counts = format!(
        "{} pulled, {} triaged, {} expired, {} faded, {} promoted",
        last.pulled, last.triaged, last.expired, last.faded, last.promoted
    );
    let pull = last.pull_error.as_ref().map(|r| format!("; the pull failed: {r}")).unwrap_or_default();
    let skip = last.skip.as_ref().map(|r| format!("; skipped the model step: {r}")).unwrap_or_default();
    let where_ = if installed { format!("installed ({kind}, {times})") } else { "not installed".to_string() };
    let detail = format!("{where_}; last pass {at} ({} h ago): {counts}{pull}{skip}", age / 3600);
    if installed && age > 36 * 3600 {
        let fix = if schedule::current_os() == Os::Macos {
            "run `rkb maintain`, or check the job with `launchctl print gui/$(id -u)/rkb.maintain`".to_string()
        } else {
            "run `rkb maintain`, or check the job with `systemctl --user status rkb-maintain.timer`".to_string()
        };
        return check("schedule", Level::Warn, detail, Some(fix));
    }
    check("schedule", Level::Ok, detail, None)
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
        session: None,
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
    fn tier_check_reports_the_three_counts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let write = |i: usize, status: &str| {
            let id = format!("{i:010x}");
            let p = root.join(format!("general/a/{i}.md"));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            let text = format!(
                "---\nschema: 1\nid: {id}\ntype: fact\nstatus: {status}\nverified: 2026-09-25\nverified_how: ran\n---\n\n# T\n\n## Statement\nX.\n\n## Evidence\nSeen.\n"
            );
            std::fs::write(p, text).unwrap();
        };
        for i in 0..600 {
            write(i, "active");
        }
        for i in 600..715 {
            write(i, "active");
        }
        for i in 715..730 {
            write(i, "archived");
        }
        let usage = root.join(crate::usage::DIR);
        std::fs::create_dir_all(&usage).unwrap();
        let lines: String = (600..715)
            .map(|i| serde_json::to_string(&crate::usage::now(&format!("{i:010x}"), "worked", None, None)).unwrap() + "\n")
            .collect();
        std::fs::write(usage.join("m-0000.jsonl"), lines).unwrap();
        let c = tier_check(root).unwrap();
        assert_eq!(c.name, "tiers");
        assert_eq!(c.detail, "600 candidate, 115 proven, 15 archive");
    }

    #[test]
    fn triage_check_warns_on_a_wide_band() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path();
        assert!(triage_check(state).is_none(), "no line before the first verdict");
        let now = crate::request::now();
        let line = |i: usize, v: &str| format!("{{\"time\":{now},\"item\":\"a\",\"index\":{i},\"verdict\":\"{v}\"}}\n");
        let mut log: String = (0..40).map(|i| line(i, "unsure")).collect();
        log.extend((40..100).map(|i| line(i, "drop")));
        std::fs::write(state.join(crate::triage::LOG), &log).unwrap();
        let c = triage_check(state).unwrap();
        assert_eq!(c.level, Level::Warn);
        assert!(c.detail.contains("40 unsure") && c.fix.as_deref().unwrap().contains("`keep` and `drop` under [triage]"), "{c:?}");
        let mut fine: String = (0..10).map(|i| line(i, "unsure")).collect();
        fine.extend((10..100).map(|i| line(i, "drop")));
        std::fs::write(state.join(crate::triage::LOG), &fine).unwrap();
        assert_eq!(triage_check(state).unwrap().level, Level::Ok);
    }

    #[test]
    fn schedule_check_names_the_last_pass_and_a_skip() {
        let dir = tempfile::tempdir().unwrap();
        let (home, state) = (dir.path().join("home"), dir.path().join("state"));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        let c = schedule_check_at(&home, &state);
        assert_eq!((c.level, c.detail.as_str()), (Level::Ok, "not installed; run `rkb schedule install`"));
        let path = crate::schedule::primary_path(&home);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("{}\n", crate::schedule::MARK)).unwrap();
        let now = crate::request::now();
        crate::maintain::record(
            &state,
            crate::maintain::Last {
                time: now - 40 * 3600,
                pulled: 2,
                triaged: 4,
                expired: 1,
                faded: 0,
                promoted: 1,
                pull_error: None,
                skip: Some("the model program failed: exit 1".into()),
            },
        );
        let c = schedule_check_at(&home, &state);
        assert_eq!(c.level, Level::Warn, "{c:?}");
        assert!(c.detail.contains("40 h ago") && c.detail.contains("2 pulled"), "{}", c.detail);
        assert!(c.detail.contains("the model program failed: exit 1"), "{}", c.detail);
        crate::maintain::record(&state, crate::maintain::Last { time: now, ..Default::default() });
        assert_eq!(schedule_check_at(&home, &state).level, Level::Ok, "a fresh pass is not a warning");
    }

    #[test]
    fn hook_check_warns_when_a_session_ran_twice() {
        let dir = tempfile::tempdir().unwrap();
        let (home, state) = (dir.path().join("home"), dir.path().join("state"));
        std::fs::create_dir_all(home.join(".claude/plugins")).unwrap();
        std::fs::write(home.join(".claude/plugins/installed_plugins.json"), r#"{"version":2,"plugins":{"rkb@rkb":[{"version":"x"}]}}"#)
            .unwrap();
        let mod_line = |state: &Path| hook_checks(&home, state).into_iter().find(|c| c.name == "hooks: claude mod");
        assert!(mod_line(&state).is_none(), "no line before the mod ran");
        crate::hooks::note_caller(&state, "s1", "mod").unwrap();
        crate::hooks::note_caller(&state, "s1", "mod").unwrap();
        crate::hooks::note_caller(&state, "p7", "command").unwrap();
        assert_eq!(std::fs::read_to_string(state.join("sessions/s1.callers")).unwrap(), "mod\n", "recorded once");
        assert!(crate::hooks::read(&state, "s1").is_empty(), "the session file keeps only signals");
        assert_eq!(mod_line(&state).map(|c| c.level), Some(Level::Ok));
        crate::hooks::note_caller(&state, "s1", "command").unwrap();
        let warn = mod_line(&state).unwrap();
        assert_eq!(warn.level, Level::Warn);
        assert!(warn.detail.starts_with("1 of 1 recent sessions ran rkb's hooks twice"), "{}", warn.detail);
    }

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

    #[test]
    fn observer_states() {
        let d = tempfile::tempdir().unwrap();
        let (config, state) = (d.path().join("config"), d.path().join("state"));
        let c = observer_check(&config, &state);
        assert_eq!(c.level, Level::Ok);
        assert!(c.detail.starts_with("off"));
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("config.toml"), "[observer]\nclaude-code = [\"sonnet\", \"haiku\"]\n").unwrap();
        let c = observer_check(&config, &state);
        assert_eq!((c.level, c.fix.as_deref()), (Level::Warn, Some("run `rkb approve observer`")));
        assert!(c.detail.starts_with("claude-code = sonnet, haiku"), "{}", c.detail);
        crate::approval::add(
            &config,
            crate::approval::Approval {
                sha256: crate::script::hash("claude-code = sonnet, haiku\ntriage claude-code = haiku"),
                system: crate::observer::system(),
                lesson: "observer".into(),
                kind: "observer".into(),
                date: "2026-09-29".into(),
            },
        )
        .unwrap();
        assert_eq!(observer_check(&config, &state).detail, "claude-code = sonnet, haiku; triage claude-code = haiku; approved here");
    }

    #[test]
    fn site_approvals_warns_until_the_user_says_yes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let state = dir.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        assert!(site_approvals(root, &state).is_none(), "no [sinks.web], no check");
        std::fs::write(root.join("kb.toml"), "[sinks.web]\nallow = { sensitivity = [\"public\"] }\n").unwrap();
        let lesson = |id: &str, labels: &str| {
            let labels = if labels.is_empty() { String::new() } else { format!("labels:\n{labels}\n") };
            let p = root.join(format!("general/a/{id}.md"));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(
                p,
                format!(
                    "---\nschema: 1\nid: {id}\ntype: fact\nstatus: active\nverified: 2026-09-25\nverified_how: ran\n{labels}---\n\n# T\n\n## Statement\nX.\n\n## Evidence\nSeen.\n"
                ),
            )
            .unwrap();
        };
        lesson("1a00000001", "");
        lesson("1a00000002", "  sensitivity: public\n");

        let c = site_approvals(root, &state).unwrap();
        assert_eq!(c.level, Level::Warn, "{c:?}");
        assert!(c.detail.contains("wait for a first-publication yes") && c.detail.contains("1a00000002"), "{}", c.detail);
        assert!(!c.detail.contains("1a00000001"), "an internal lesson is not waiting: {}", c.detail);
        assert_eq!(c.fix.as_deref(), Some("run `rkb publish <dir>`, answer `publish`, and push the knowledge base"));

        std::fs::create_dir_all(root.join("site")).unwrap();
        std::fs::write(root.join("site/published.json"), r#"{"clear":["1a00000002"],"encrypted":{}}"#).unwrap();
        let c = site_approvals(root, &state).unwrap();
        assert_eq!((c.name, c.level, c.detail.as_str()), ("site approvals", Level::Ok, "every lesson the site publishes is recorded"));
    }

    #[test]
    fn hook_rankings_warn_on_timeouts() {
        let d = tempfile::tempdir().unwrap();
        let (root, state) = (d.path().join("kb"), d.path().join("state"));
        std::fs::create_dir_all(&state).unwrap();
        assert_eq!(recall_check(&root, &state), None, "no log, no line");
        let now = crate::request::now();
        let line = |backend: &str, skipped: &str, ms: u64| {
            format!("{{\"time\":{now},\"hook\":\"recall\",\"backend\":\"{backend}\",\"skipped\":[{skipped}],\"ms\":{ms}}}\n")
        };
        let mut log = String::new();
        for i in 0..8 {
            log += &line("jev", "", 200 + i * 10);
        }
        log += &line("bm25", "\"jev: timeout after 500 ms\"", 501);
        let c = recall_check(&root, &state);
        assert!(c.is_none(), "the log file is not written yet");
        std::fs::write(state.join(crate::hooks::RANKINGS), &log).unwrap();
        let c = recall_check(&root, &state).unwrap();
        assert_eq!(c.level, Level::Ok, "{}", c.detail);
        for _ in 0..4 {
            log += &line("bm25", "\"jev: timeout after 500 ms\"", 501);
        }
        std::fs::write(state.join(crate::hooks::RANKINGS), &log).unwrap();
        let c = recall_check(&root, &state).unwrap();
        assert_eq!(c.level, Level::Warn);
        assert!(c.detail.starts_with("13 hook searches in 7 days, 5 fell back from a backend (5 after the 500 ms limit)"), "{}", c.detail);
        assert_eq!(c.fix.as_deref(), Some("set hook_timeout_ms = 600 under [hooks] in kb.toml"), "90th percentile 270 + 200, at least 600");
    }
}
