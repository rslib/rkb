use std::collections::HashMap;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonKind {
    Retired,
    Unsupported,
    LongStale,
    MissingCommit,
    FailedRepeatedly,
    NeverHelped,
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
}

/// Reads this machine's session logs and use records. Changes nothing.
pub fn signals(root: &Path, state: &Path) -> Signals {
    let records = usage::records(root);
    let mut out = Signals::default();
    for e in std::fs::read_dir(state.join("sessions")).into_iter().flatten().flatten() {
        let Some(session) = e.path().file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
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
