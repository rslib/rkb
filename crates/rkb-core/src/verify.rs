use std::time::Duration;

use serde_json::json;

use crate::approval::{self, Approval};
use crate::conditions::{Facts, Verdict, evaluate};
use crate::config::{self, KbConfig};
use crate::error::{Error, Result};
use crate::kb::Snapshot;
use crate::lesson::{Frontmatter, Lesson, Status, VerifiedHow};
use crate::matching::{self, Place};
use crate::request::{self, Action, Choice, Decision, Request};
use crate::script::{self, Kind};
use crate::write::{Ctx, Outcome, apply_check};

pub const DEFAULT_TIMEOUT_S: u64 = 60;
pub const DEFAULT_REFRESH_DAYS: i64 = 30;

/// What happened to one lesson's check, for summaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Passed,
    Failed,
    Unknown,
    Skipped,
}

fn setting(root: &std::path::Path, key: &str, default: i64) -> i64 {
    let kb: KbConfig = std::fs::read_to_string(root.join("kb.toml")).ok().and_then(|t| config::parse(&t).ok()).unwrap_or_default();
    kb.verify.as_ref().and_then(|t| t.get(key)).and_then(|v| v.as_integer()).unwrap_or(default)
}

fn place(ctx: &Ctx) -> Place {
    ctx.place.cloned().unwrap_or_default()
}

fn title(l: &Lesson) -> String {
    crate::body::scan(&l.body).h1.into_iter().next().map(|(_, t)| t).unwrap_or_default()
}

fn load<'a>(lessons: &'a [Lesson], id: &str) -> Result<&'a Lesson> {
    lessons.iter().find(|l| l.frontmatter.id == id).ok_or_else(|| Error::NotFound(id.to_string()))
}

/// The request to approve a lesson's script here, or `None` when that exact script is already approved.
pub fn approve_request(ctx: &Ctx, id: &str, kind: Kind, then_verify: bool) -> Result<Option<Request>> {
    let snap = Snapshot::from_dir(ctx.root)?;
    let (lessons, _) = snap.lessons();
    let l = load(&lessons, id)?;
    let text = script::of(l, kind).ok_or_else(|| {
        Error::Refused(format!("lesson {id} has no {} script: one fenced sh or bash block under `## {}`", kind.name(), kind.heading()))
    })?;
    let sha = script::hash(&text);
    let system = approval::system_key(&place(ctx));
    if approval::is_approved(&crate::paths::config_dir(), &sha, &system) {
        return Ok(None);
    }
    let verb = if then_verify { "Approve and run" } else { "Approve" };
    let question = format!(
        "{verb} the {} script of {id} \"{}\" on {system}? rkb runs it with `bash -l` in a temporary folder:\n```bash\n{text}```\nsha256 {sha}",
        kind.name(),
        title(l)
    );
    let req = Request {
        id: request::new_id(),
        created: request::now(),
        action: Action::Approve { id: id.to_string(), script: kind, sha256: sha, system, then_verify },
        approved: vec![],
        question,
        choices: vec![
            Choice { text: format!("approve {id} {}", kind.name()), decision: Some(Decision::Approve) },
            Choice { text: "cancel".into(), decision: None },
        ],
    };
    request::save(&ctx.state.join("requests"), &req)?;
    Ok(Some(req))
}

/// Records a confirmed approval, when the script is still the one the request showed; then runs the check if asked.
pub fn approve(ctx: &Ctx, id: &str, kind: Kind, sha256: &str, system: &str, then_verify: bool) -> Result<Outcome> {
    let snap = Snapshot::from_dir(ctx.root)?;
    let (lessons, _) = snap.lessons();
    let l = load(&lessons, id)?;
    if script::of(l, kind).map(|t| script::hash(&t)).as_deref() != Some(sha256) {
        return Err(Error::Refused(format!(
            "the {} script of {id} changed since the request; run `rkb approve {id} {}` again",
            kind.name(),
            kind.name()
        )));
    }
    let date = ctx.today.to_string();
    approval::add(
        &crate::paths::config_dir(),
        Approval { sha256: sha256.into(), system: system.into(), lesson: id.into(), kind: kind.name().into(), date },
    )?;
    if then_verify {
        return one(ctx, id, false).map(|(_, o)| o);
    }
    Ok(Outcome::Info(format!("Approved the {} script of {id} on {system}", kind.name())))
}

/// The new frontmatter and a message for a check result, or `None` when nothing changes.
fn decide(
    fm: &Frontmatter,
    result: script::Result,
    code: Option<i32>,
    system: &str,
    auto: bool,
    today: jiff::civil::Date,
    refresh: i64,
) -> Option<(Frontmatter, String)> {
    let mut new = fm.clone();
    let code_text = code.map_or("no code".to_string(), |c| format!("exit {c}"));
    let message = match result {
        script::Result::Fail if fm.status == Status::Active => {
            new.status = Status::Stale;
            new.stale_reason = Some(format!("check failed on {system} ({code_text})"));
            format!("check failed on {system} ({code_text}); the lesson is stale now")
        }
        script::Result::Pass => {
            let reactivate = fm.status == Status::Stale && !auto;
            if reactivate {
                new.status = Status::Active;
                new.stale_reason = None;
            }
            let old = today.since(fm.verified).map(|s| s.get_days() as i64).unwrap_or(0) > refresh;
            if !(reactivate || old) {
                return None;
            }
            new.verified = today;
            new.verified_how = VerifiedHow::Checked;
            if reactivate { format!("check passed on {system}; the lesson is active again") } else { format!("check passed on {system}") }
        }
        _ => return None,
    };
    Some((new, message))
}

/// Runs one lesson's `Check`. `auto` never asks and never reactivates a stale lesson.
pub fn one(ctx: &Ctx, id: &str, auto: bool) -> Result<(CheckStatus, Outcome)> {
    let snap = Snapshot::from_dir(ctx.root)?;
    let (lessons, _) = snap.lessons();
    let l = load(&lessons, id)?;
    if !l.frontmatter.status.is_current() {
        return Err(Error::Refused(format!("lesson {id} is {}; only an active or stale lesson is checked", l.frontmatter.status.as_str())));
    }
    let text = script::of(l, Kind::Check)
        .ok_or_else(|| Error::Refused(format!("lesson {id} has no Check script: one fenced sh or bash block under `## Check`")))?;
    let here = place(ctx);
    let applies = evaluate(&l.frontmatter.when, &Facts::gather(ctx.root, &here, &[]));
    if applies.result == Verdict::No {
        return Ok((CheckStatus::Skipped, Outcome::Info(format!("Skipped {id}: it does not apply here ({})", applies.summary()))));
    }
    let sha = script::hash(&text);
    let system = approval::system_key(&here);
    if !approval::is_approved(&crate::paths::config_dir(), &sha, &system) {
        if auto {
            return Ok((CheckStatus::Skipped, Outcome::Info(format!("Skipped {id}: its Check is not approved on {system}"))));
        }
        let req = approve_request(ctx, id, Kind::Check, true)?.expect("not approved");
        return Ok((CheckStatus::Skipped, Outcome::NeedsUser(req)));
    }
    let project_root = l
        .path
        .strip_prefix("projects/")
        .and_then(|p| p.split('/').next())
        .and_then(|p| matching::checkouts(ctx.state, p).into_iter().find(|c| c.is_dir()));
    let timeout = Duration::from_secs(setting(ctx.root, "timeout_s", DEFAULT_TIMEOUT_S as i64).max(1) as u64);
    let run = script::run(&text, project_root.as_deref(), timeout);
    let record = json!({ "id": id, "system": system, "date": ctx.today.to_string(), "result": run.result, "code": run.code });
    crate::lock::append_line(&ctx.state.join("checks.jsonl"), &record.to_string())?;

    let status = match run.result {
        script::Result::Pass => CheckStatus::Passed,
        script::Result::Fail => CheckStatus::Failed,
        script::Result::Unknown => CheckStatus::Unknown,
    };
    let refresh = setting(ctx.root, "refresh_days", DEFAULT_REFRESH_DAYS);
    let today = ctx.today;
    let written = apply_check(ctx, id, &sha, |fm| decide(fm, run.result, run.code, &system, auto, today, refresh))?;
    let result = serde_json::to_value(run.result).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    Ok((status, written.unwrap_or_else(|| Outcome::Info(format!("Check of {id} {result} on {system}; nothing to change")))))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub passed: Vec<String>,
    pub failed: Vec<String>,
    pub unknown: Vec<String>,
    pub skipped: Vec<String>,
}

/// Runs every approved `Check` of current lessons that can apply here. Never asks, never reactivates.
pub fn auto(ctx: &Ctx) -> Result<Summary> {
    let snap = Snapshot::from_dir(ctx.root)?;
    let (mut lessons, _) = snap.lessons();
    lessons.sort_by(|a, b| a.path.cmp(&b.path));
    let mut s = Summary::default();
    for l in lessons.iter().filter(|l| l.frontmatter.status.is_current() && script::of(l, Kind::Check).is_some()) {
        let id = l.frontmatter.id.clone();
        match one(ctx, &id, true) {
            Ok((CheckStatus::Passed, _)) => s.passed.push(id),
            Ok((CheckStatus::Failed, _)) => s.failed.push(id),
            Ok((CheckStatus::Unknown, _)) => s.unknown.push(id),
            Ok((CheckStatus::Skipped, _)) | Err(_) => s.skipped.push(id),
        }
    }
    Ok(s)
}
