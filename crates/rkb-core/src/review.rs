use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_norway::Value;

use crate::conditions::{Range, Version};
use crate::config::{self, KbConfig, ProjectNote, SystemNote};
use crate::error::{Error, Result};
use crate::git;
use crate::kb::{FileKind, NoteRole, Snapshot, note_role};
use crate::lesson::Status;
use crate::{matching, usage};

pub const DEFAULT_STALE_DAYS: i64 = 90;
pub const UNUSED_DAYS: i64 = 180;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonKind {
    Retired,
    Unsupported,
    LongStale,
    MissingCommit,
    FailedRepeatedly,
    NeverHelped,
    Unused,
    OftenIrrelevant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reason {
    pub kind: ReasonKind,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub id: String,
    pub title: String,
    pub path: String,
    pub status: Status,
    pub reasons: Vec<Reason>,
    pub last_worked: Option<String>,
    pub last_failed: Option<String>,
}

fn retired_scopes(snap: &Snapshot) -> Vec<String> {
    let mut out = vec![];
    for (path, data) in snap.of_kind(FileKind::FolderNote) {
        let text = String::from_utf8_lossy(data);
        let retired = match note_role(path) {
            NoteRole::Project => config::parse_note::<ProjectNote>(&text).is_ok_and(|n| n.retired),
            NoteRole::System => config::parse_note::<SystemNote>(&text).is_ok_and(|n| n.retired),
            _ => false,
        };
        if retired && let Some((folder, _)) = path.rsplit_once('/') {
            out.push(folder.to_string());
        }
    }
    out
}

fn oldest_supported(kb: &KbConfig) -> Vec<(String, Version)> {
    kb.facts
        .iter()
        .flatten()
        .filter_map(|(key, v)| {
            let oldest = v.get("oldest_supported")?.as_str()?;
            Some((key.clone(), Version::parse(oldest)?))
        })
        .collect()
}

fn strings(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Sequence(items) => items.iter().filter_map(|i| i.as_str().map(str::to_string)).collect(),
        _ => vec![],
    }
}

/// Days since the lesson was last flagged, or since its file last changed when no flag commit exists.
fn stale_days(root: &Path, path: &str, today: jiff::civil::Date) -> Option<i64> {
    let date = |args: &[&str]| git::run(root, args).ok().map(|o| String::from_utf8_lossy(&o).trim().to_string()).filter(|d| !d.is_empty());
    let day =
        date(&["log", "-1", "--format=%cs", "--grep=^flag(", "--", path]).or_else(|| date(&["log", "-1", "--format=%cs", "--", path]))?;
    let since: jiff::civil::Date = day.parse().ok()?;
    today.since(since).ok().map(|s| s.get_days() as i64)
}

/// The first recorded checkout of `project` that exists and is a full clone.
fn full_checkout(state: &Path, project: &str) -> Option<PathBuf> {
    matching::checkouts(state, project).into_iter().find(|p| {
        p.is_dir() && git::run(p, &["rev-parse", "--is-shallow-repository"]).is_ok_and(|o| String::from_utf8_lossy(&o).trim() == "false")
    })
}

/// Current lessons under `folder` (or the whole knowledge base) that have at least one review reason. Reads only.
/// What this machine's sessions show: how often signals came up, and whether injected lessons helped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Signals {
    pub sessions: usize,
    pub with_failed: usize,
    pub with_fixed: usize,
    pub with_correction: usize,
    pub with_remember: usize,
    pub injected: usize,
    pub injected_then_helped: usize,
    pub extracts: usize,
    /// Failure signatures seen in 2 or more sessions: the same mistake made again.
    pub repeated: usize,
    /// Of those, how many had a lesson injected in the session that repeated it.
    pub repeated_with_lesson: usize,
    /// Per injection path (`tool-failed`, `recall`, `other`), from the use records of these sessions.
    pub paths: BTreeMap<String, PathSignals>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PathSignals {
    pub injected: usize,
    pub helped: usize,
    pub irrelevant: usize,
}

/// Reads this machine's session logs and use records. Changes nothing.
pub fn signals(root: &Path, state: &Path) -> Signals {
    let records = usage::records(root);
    let mut out = Signals::default();
    for path in ["tool-failed", "recall"] {
        out.paths.insert(path.into(), PathSignals::default());
    }
    for e in std::fs::read_dir(state.join("sessions")).into_iter().flatten().flatten() {
        let Some(session) = e.path().file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
        let in_session: Vec<&usage::Record> = records.iter().filter(|u| u.session.as_deref() == Some(session.as_str())).collect();
        for r in in_session.iter().filter(|u| u.event == "injected") {
            let prefix = r.reason.as_deref().and_then(|x| x.split(',').next()).unwrap_or_default();
            let path = if matches!(prefix, "tool-failed" | "recall") { prefix } else { "other" };
            let followed = |event: &str| in_session.iter().any(|u| u.id == r.id && u.event == event);
            let p = out.paths.entry(path.into()).or_default();
            p.injected += 1;
            p.helped += usize::from(followed("worked") || followed("inferred"));
            p.irrelevant += usize::from(followed("irrelevant"));
        }
        let text = std::fs::read_to_string(e.path()).unwrap_or_default();
        let lines: Vec<serde_json::Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let has = |k: &str| lines.iter().any(|r| r["kind"] == k);
        out.sessions += 1;
        out.with_failed += usize::from(has("failed"));
        out.with_fixed += usize::from(has("fixed"));
        out.with_correction += usize::from(has("correction"));
        out.with_remember += usize::from(has("remember"));
        out.extracts += lines.iter().filter(|r| r["kind"] == "extracted").count();
        for r in lines.iter().filter(|r| r["kind"] == "injected") {
            let Some(id) = r["id"].as_str() else { continue };
            out.injected += 1;
            let inferred = lines.iter().any(|x| x["kind"] == "inferred" && x["id"] == id);
            let worked = records.iter().any(|u| u.id == id && u.event == "worked" && u.session.as_deref() == Some(session.as_str()));
            out.injected_then_helped += usize::from(inferred || worked);
        }
    }
    // Sessions in the order each signature showed up in them.
    let mut seen: HashMap<String, Vec<String>> = HashMap::new();
    for r in std::fs::read_to_string(state.join("signatures.jsonl")).unwrap_or_default().lines() {
        let Ok(r) = serde_json::from_str::<serde_json::Value>(r) else { continue };
        let (Some(sig), Some(session)) = (r["sig"].as_str(), r["session"].as_str()) else { continue };
        let sessions = seen.entry(sig.to_string()).or_default();
        if !sessions.iter().any(|s| s == session) {
            sessions.push(session.to_string());
        }
    }
    for sessions in seen.values().filter(|s| s.len() >= 2) {
        out.repeated += 1;
        let log = std::fs::read_to_string(state.join("sessions").join(format!("{}.jsonl", sessions[1]))).unwrap_or_default();
        out.repeated_with_lesson +=
            usize::from(log.lines().any(|l| serde_json::from_str::<serde_json::Value>(l).is_ok_and(|r| r["kind"] == "injected")));
    }
    out
}

pub fn review(root: &Path, state: &Path, folder: Option<&str>) -> Result<Vec<Candidate>> {
    let folder = folder.map(|f| f.trim_end_matches('/'));
    let usage = usage::counts(root);
    let records = usage::records(root);
    if let Some(f) = folder
        && (f.is_empty() || !root.join(f).is_dir())
    {
        return Err(Error::NoFolder(f.to_string()));
    }
    let kb: KbConfig = std::fs::read_to_string(root.join("kb.toml")).ok().and_then(|t| config::parse(&t).ok()).unwrap_or_default();
    let stale_limit = kb.review.as_ref().and_then(|t| t.get("stale_days")).and_then(|v| v.as_integer()).unwrap_or(DEFAULT_STALE_DAYS);
    let snap = Snapshot::from_dir(root)?;
    let retired = retired_scopes(&snap);
    let oldest = oldest_supported(&kb);
    let today = jiff::Zoned::now().date();
    let unused_cutoff = today.saturating_sub(jiff::Span::new().days(UNUSED_DAYS));
    let unused_since = unused_cutoff.to_string();
    let mut checkout_cache: HashMap<String, Option<PathBuf>> = HashMap::new();

    let (mut lessons, _) = snap.lessons();
    lessons.sort_by(|a, b| a.path.cmp(&b.path));
    let mut out = vec![];
    for l in lessons.iter().filter(|l| l.frontmatter.status.is_current()) {
        if folder.is_some_and(|f| !l.path.starts_with(&format!("{f}/"))) {
            continue;
        }
        let mut reasons = vec![];
        for r in retired.iter().filter(|r| l.path.starts_with(&format!("{r}/"))) {
            reasons.push(Reason { kind: ReasonKind::Retired, detail: r.clone() });
        }
        for (key, v) in &oldest {
            let Some(value) = l.frontmatter.when.get(key.as_str()) else { continue };
            let ranges = strings(value);
            let parsed: Vec<Range> = ranges.iter().filter_map(|r| Range::parse(r).ok()).collect();
            if !parsed.is_empty() && parsed.len() == ranges.len() && parsed.iter().all(|r| r.older_than(v)) {
                let oldest_text = kb
                    .facts
                    .as_ref()
                    .and_then(|f| f.get(key))
                    .and_then(|f| f.get("oldest_supported"))
                    .and_then(|o| o.as_str())
                    .unwrap_or("");
                reasons.push(Reason { kind: ReasonKind::Unsupported, detail: format!("{key} {} < {oldest_text}", ranges.join(", ")) });
            }
        }
        if l.frontmatter.status == Status::Stale
            && let Some(days) = stale_days(root, &l.path, today)
            && days > stale_limit
        {
            reasons.push(Reason { kind: ReasonKind::LongStale, detail: format!("stale {days} days") });
        }
        if let Some(project) = l.path.strip_prefix("projects/").and_then(|p| p.split('/').next()) {
            for key in ["since", "until"] {
                let Some(commit) = l.frontmatter.when.get(key).and_then(Value::as_str) else { continue };
                let checkout = checkout_cache.entry(project.to_string()).or_insert_with(|| full_checkout(state, project));
                if let Some(dir) = checkout
                    && git::run(dir, &["cat-file", "-e", &format!("{commit}^{{commit}}")]).is_err()
                {
                    reasons.push(Reason { kind: ReasonKind::MissingCommit, detail: format!("{key} {commit}") });
                }
            }
        }
        let c = usage.get(&l.frontmatter.id).cloned().unwrap_or_default();
        let verified = l.frontmatter.verified.to_string();
        let failed_since =
            records.iter().filter(|r| r.id == l.frontmatter.id && r.event == "failed" && r.time.as_str() >= verified.as_str()).count();
        if failed_since >= 2 {
            reasons.push(Reason { kind: ReasonKind::FailedRepeatedly, detail: format!("failed {failed_since} times since {verified}") });
        }
        if c.injected >= 5 && c.worked == 0 && c.inferred == 0 {
            reasons
                .push(Reason { kind: ReasonKind::NeverHelped, detail: format!("injected {} times, never reported to help", c.injected) });
        }
        if c.irrelevant >= 3 && c.irrelevant >= c.worked + c.inferred {
            reasons.push(Reason {
                kind: ReasonKind::OftenIrrelevant,
                detail: format!("irrelevant {}, helped {}", c.irrelevant, c.worked + c.inferred),
            });
        }
        if l.frontmatter.verified < unused_cutoff
            && !records.iter().any(|r| r.id == l.frontmatter.id && r.time.as_str() >= unused_since.as_str())
        {
            reasons.push(Reason { kind: ReasonKind::Unused, detail: format!("no use recorded since {unused_since}, verified {verified}") });
        }
        if reasons.is_empty() {
            continue;
        }
        let (last_worked, last_failed) = (c.last_worked, c.last_failed);
        out.push(Candidate {
            id: l.frontmatter.id.clone(),
            title: crate::body::scan(&l.body).h1.into_iter().next().map(|(_, t)| t).unwrap_or_default(),
            path: l.path.clone(),
            status: l.frontmatter.status,
            reasons,
            last_worked,
            last_failed,
        });
    }
    Ok(out)
}

/// One rkb commit in the knowledge base, for `rkb changes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub commit: String,
    pub date: String,
    pub kind: String,
    /// The lesson id; empty for a folder write such as an archive of a folder.
    pub id: String,
    pub folder: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The option `RKB_AUTO_CONFIRM` answered for this write.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_confirmed: Option<String>,
}

/// The reason a flag, supersede or archive recorded in the lesson it wrote.
fn reason_in(kind: &str, path: &str, text: &str) -> Option<String> {
    let l = crate::lesson::parse(path, text).ok()?;
    let first = |s: String| s.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string);
    match kind {
        "flag" => l.frontmatter.stale_reason,
        "supersede" => crate::body::section_text(&l.body, "Why superseded").and_then(first),
        "archive" => crate::body::section_text(&l.body, "Why archived").and_then(first),
        _ => None,
    }
}

/// rkb commits since `since` (a date git understands), newest first, without usage commits. Reads only.
pub fn changes(root: &Path, since: &str) -> Result<Vec<Change>> {
    let out = git::run(
        root,
        &[
            "log",
            &format!("--since={since}"),
            "--format=%x1e%h%x1f%cs%x1f%s%x1f%(trailers:key=Auto-confirmed,valueonly,separator=%x20)%x1f",
            "--name-only",
            "--no-renames",
        ],
    )?;
    let subject = regex::Regex::new(r"^([a-z]+)\(([^)]*)\): (.*?)(?: \[([0-9a-f]{10})\])?$").expect("valid regex");
    let mut rows = vec![];
    for entry in String::from_utf8_lossy(&out).split('\x1e').filter(|e| !e.trim().is_empty()) {
        let mut head = entry.splitn(5, '\x1f');
        let (Some(commit), Some(date), Some(s), Some(auto), Some(names)) =
            (head.next(), head.next(), head.next(), head.next(), head.next())
        else {
            continue;
        };
        let commit = commit.trim();
        let auto = Some(auto.trim().to_string()).filter(|a| !a.is_empty());
        let Some(m) = subject.captures(s) else { continue };
        let kind = m[1].to_string();
        let first_lesson = names.lines().map(str::trim).find(|p| p.ends_with(".md") && !p.ends_with("README.md")).map(str::to_string);
        rows.push((
            commit.to_string(),
            date.to_string(),
            kind,
            m[2].to_string(),
            m[3].to_string(),
            m.get(4).map(|i| i.as_str().to_string()),
            first_lesson,
            auto,
        ));
    }
    let wanted: Vec<String> = rows
        .iter()
        .filter(|r| matches!(r.2.as_str(), "flag" | "supersede" | "archive"))
        .filter_map(|r| r.6.as_ref().map(|p| format!("{}:{p}", r.0)))
        .collect();
    let mut blobs = git::read_blobs(root, &wanted)?.into_iter();
    Ok(rows
        .into_iter()
        .map(|(commit, date, kind, folder, title, id, path, auto_confirmed)| {
            let reason = match (&path, matches!(kind.as_str(), "flag" | "supersede" | "archive")) {
                (Some(p), true) => blobs.next().flatten().and_then(|b| reason_in(&kind, p, &String::from_utf8_lossy(&b))),
                _ => None,
            };
            Change { commit, date, kind, id: id.unwrap_or_default(), folder, title, reason, auto_confirmed }
        })
        .collect())
}

/// How many things `/rkb:curate` would look at: review candidates, likely duplicate pairs and lessons with
/// quality warnings. Counted at most once a day and kept in `curate.json` in the state folder, because
/// review reads git history.
pub fn curate_count(root: &Path, state: &Path) -> Result<usize> {
    let today = jiff::Zoned::now().date().to_string();
    let path = state.join("curate.json");
    let cached: Option<serde_json::Value> = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok());
    if let Some(c) = cached.filter(|c| c["date"] == today.as_str())
        && let Some(n) = c["count"].as_u64()
    {
        return Ok(n as usize);
    }
    let candidates = review(root, state, None)?.len();
    let pairs = crate::graph::Graph::load(root)?.dupes(crate::graph::min_similarity(root)).len();
    let snap = Snapshot::from_dir(root)?;
    let mut weak: Vec<String> = crate::lint::lint(&snap, &crate::lint::LintEnv::default(), None)
        .into_iter()
        .filter(|f| f.rule.starts_with("quality/"))
        .map(|f| f.path)
        .collect();
    weak.sort();
    weak.dedup();
    let n = candidates + pairs + weak.len();
    let _ = std::fs::write(&path, serde_json::json!({ "date": today, "count": n }).to_string());
    Ok(n)
}

/// The session-start line asking the agent to curate, when 5 or more things wait.
pub fn curate_nudge(root: &Path, state: &Path, command: &str) -> Option<String> {
    let n = curate_count(root, state).ok()?;
    (n >= 5).then(|| {
        format!(
            "rkb curate: {n} lessons or pairs may need a merge, a sharper text or archiving. At a natural pause in the work, run {command}, without waiting for the user."
        )
    })
}
