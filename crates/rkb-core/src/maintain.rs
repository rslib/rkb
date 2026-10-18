//! One upkeep pass: the lock `rkb maintain` and `rkb promote` share, the daily model cap, the
//! transcript expiry, the fade of unused automatic lessons and the record of the last pass.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::lesson::{LessonType, today_utc};
use crate::request::{self, Action};
use crate::write::Ctx;
use crate::{distill, kb, promote, usage, write};

/// The lock file `rkb maintain` and `rkb promote` share. One pass runs at a time.
pub const LOCK: &str = "pass.lock";

/// The lock path inside the per-machine state folder.
pub fn lock_path(state: &Path) -> PathBuf {
    state.join(LOCK)
}

/// What a second pass prints and the reason it runs no step.
pub const BUSY: &str = "a pass is already running";

/// Model calls one UTC day allows, unless `[maintain] max_calls_per_day` says otherwise.
pub const DEFAULT_MAX_CALLS_PER_DAY: u64 = 3;

/// `[maintain] max_calls_per_day`, from 0 to 100.
pub fn max_calls_per_day(config_dir: &Path) -> u64 {
    machine(config_dir, "maintain", "max_calls_per_day").map_or(DEFAULT_MAX_CALLS_PER_DAY, |n| n.clamp(0, 100) as u64)
}

/// Days an automatic lesson may go unused before the pass archives it.
pub const DEFAULT_UNUSED_DAYS: u64 = 30;

/// `[maintain] unused_days`, from 1 to 3650.
pub fn unused_days(config_dir: &Path) -> u64 {
    machine(config_dir, "maintain", "unused_days").map_or(DEFAULT_UNUSED_DAYS, |n| n.clamp(1, 3650) as u64)
}

/// The reason the fade archives a lesson, as the spec words it.
pub const UNUSED_REASON: &str = "unused for 30 days after promotion";

fn machine(config_dir: &Path, section: &str, key: &str) -> Option<i64> {
    crate::config::machine(config_dir).ok().and_then(|(_, t)| t.get(section)?.get(key)?.as_integer())
}

/// What the last pass did, for `rkb doctor` and the pass summary.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Last {
    /// Seconds since the Unix epoch.
    pub time: u64,
    pub pulled: usize,
    pub triaged: usize,
    /// Unsure candidates the triage models were asked about.
    #[serde(default)]
    pub asked: usize,
    pub expired: usize,
    pub faded: usize,
    pub promoted: usize,
    /// Why the proposals pull failed, such as no network after a wake; the other steps still ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_error: Option<String>,
    /// Why the pass made no model call, such as a missing approved chain or a failed harness program.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip: Option<String>,
}

/// The per-machine pass state: the UTC day the model calls are counted in, the calls made that day
/// and the last pass. It lives in the state folder, never in the knowledge base.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// `YYYY-MM-DD` in UTC.
    pub date: String,
    pub calls: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<Last>,
}

fn file(state: &Path) -> PathBuf {
    state.join("maintain.json")
}

/// The state, with the calls reset when the stored day is not today.
pub fn load(state: &Path) -> State {
    let mut s: State = std::fs::read_to_string(file(state)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let today = today_utc().to_string();
    if s.date != today {
        s.date = today;
        s.calls = 0;
    }
    s
}

/// Writes the state through a temporary file and a rename, so a reader never sees half of it.
pub fn save(state: &Path, s: &State) -> Result<()> {
    std::fs::create_dir_all(state).map_err(crate::error::io(state))?;
    let path = file(state);
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string(s).expect("maintain state serializes");
    std::fs::write(&tmp, &text).map_err(crate::error::io(&tmp))?;
    std::fs::rename(&tmp, &path).map_err(crate::error::io(&path))
}

/// How many model calls today still allows.
pub fn remaining(state: &Path, max: u64) -> u64 {
    max.saturating_sub(load(state).calls)
}

/// Records one model call and says whether it was allowed. A call that then fails is counted too.
pub fn take_call(state: &Path, max: u64) -> bool {
    let mut s = load(state);
    if s.calls >= max {
        return false;
    }
    s.calls += 1;
    save(state, &s).is_ok()
}

/// The last pass, when one ran.
pub fn last(state: &Path) -> Option<Last> {
    load(state).last
}

/// Records the pass that just ended.
pub fn record(state: &Path, last: Last) {
    let mut s = load(state);
    s.last = Some(last);
    let _ = save(state, &s);
}

/// Removes each transcript item the rule triage marked `drop` or that is older than
/// `[inbox] transcript_days`, not eligible for promotion and not reported as worked. Never a note.
/// Returns how many it removed.
pub fn expire(state: &Path, config_dir: &Path) -> usize {
    let days = distill::transcript_days(config_dir);
    let worked = distill::worked(state);
    let now = request::now();
    let mut removed = 0;
    for item in distill::list(state) {
        if item.meta.kind != distill::Kind::Transcript {
            continue;
        }
        let dropped = crate::triage::dropped(&item);
        let old = now.saturating_sub(item.meta.time) > days.saturating_mul(86400);
        let eligible = worked.contains(&item.id)
            || item.meta.signatures.iter().any(|sig| distill::eligible(state, sig) && !distill::is_done(state, sig));
        if (dropped || old) && !eligible && distill::remove(state, &item.id) {
            removed += 1;
        }
    }
    removed
}

/// Archives each active lesson that promotion wrote more than `[maintain] unused_days` ago, that is
/// not a preference or a decision and that has no `worked` report. Returns how many it archived.
pub fn fade(ctx: &Ctx, config_dir: &Path) -> Result<usize> {
    let days = unused_days(config_dir);
    let now = request::now();
    let counts = usage::counts(ctx.root);
    let snap = kb::Snapshot::from_dir(ctx.root)?;
    let (lessons, _) = snap.lessons();
    let mut archived = 0;
    for (id, time) in promote::promoted(ctx.state) {
        if now.saturating_sub(time) < days.saturating_mul(86400) {
            continue;
        }
        let Some(lesson) = lessons.iter().find(|l| l.frontmatter.id == id) else { continue };
        if !lesson.frontmatter.status.is_current() {
            continue;
        }
        if matches!(lesson.frontmatter.kind, LessonType::Preference | LessonType::Decision) {
            continue;
        }
        if counts.get(&id).is_some_and(|c| c.worked > 0) {
            continue;
        }
        write::apply(ctx, &Action::Archive { target: id.clone(), reason: UNUSED_REASON.into() }, &[])?;
        archived += 1;
    }
    Ok(archived)
}
