//! Automatic distill and curate: whether a run is due, and a claim that lets one session start it.
//! Off unless `[distill] auto = true` in this machine's `config.toml`; rkb decides, the harness only
//! starts the agent.

use std::path::{Path, PathBuf};

use crate::distill::{self, Verdict};
use crate::error::{Result, io};

/// Hours between automatic distill runs, unless `[distill] every_hours` says otherwise.
const DISTILL_EVERY_HOURS: u64 = 6;
/// Hours between automatic curate runs.
const CURATE_EVERY_HOURS: u64 = 24;
/// Curate candidates an automatic curate needs.
const CURATE_MIN: usize = 5;
/// A run that holds its lock longer than this is taken to have died.
const LOCK_STALE_SECS: u64 = 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Distill,
    Curate,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "distill" => Some(Kind::Distill),
            "curate" => Some(Kind::Curate),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Distill => "distill",
            Kind::Curate => "curate",
        }
    }
}

/// Whether an automatic run is due, and why or why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    pub due: bool,
    pub reason: String,
}

fn no(reason: impl Into<String>) -> Gate {
    Gate { due: false, reason: reason.into() }
}

/// `[distill] auto` and `every_hours`.
fn settings(config_dir: &Path) -> (bool, u64) {
    let table = crate::config::machine(config_dir).ok().and_then(|(_, t)| t.get("distill").and_then(|v| v.as_table()).cloned());
    let auto = table.as_ref().and_then(|t| t.get("auto")).and_then(|v| v.as_bool()).unwrap_or(false);
    let every =
        table.as_ref().and_then(|t| t.get("every_hours")).and_then(|v| v.as_integer()).map_or(DISTILL_EVERY_HOURS, |h| h.max(1) as u64);
    (auto, every)
}

/// Whether `[distill] auto = true`: the Claude Code mod then starts distill and curate itself.
pub fn enabled(config_dir: &Path) -> bool {
    settings(config_dir).0
}

fn lock_path(state: &Path, kind: Kind) -> PathBuf {
    state.join(format!("auto-{}.lock", kind.as_str()))
}

fn last_path(state: &Path) -> PathBuf {
    state.join("auto.json")
}

/// When the last automatic run of `kind` started, in seconds since the epoch.
pub fn last_run(state: &Path, kind: Kind) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(last_path(state)).ok()?).ok()?;
    v[kind.as_str()].as_u64()
}

fn held(state: &Path, kind: Kind, now: u64) -> bool {
    std::fs::read_to_string(lock_path(state, kind))
        .ok()
        .and_then(|t| t.trim().parse::<u64>().ok())
        .is_some_and(|t| now.saturating_sub(t) < LOCK_STALE_SECS)
}

/// Kept candidates without an outcome: how many, and whether one is in an item of priority 3.
fn kept(state: &Path) -> (usize, bool) {
    let mut n = 0;
    let mut high = false;
    for item in distill::list(state) {
        let open =
            item.meta.candidates.iter().filter(|c| matches!(c.verdict, Verdict::Keep | Verdict::Known) && c.outcome.is_none()).count();
        n += open;
        high |= open > 0 && item.meta.priority() == 3;
    }
    (n, high)
}

/// Whether an automatic run of `kind` is due now; `curate_count` is what `rkb review` and friends found.
pub fn gate(state: &Path, config_dir: &Path, kind: Kind, curate_count: usize, now: u64) -> Gate {
    let (auto, every) = settings(config_dir);
    if !auto {
        return no("off: set `auto = true` under [distill] in config.toml");
    }
    if held(state, kind, now) {
        return no(format!("an automatic {} runs already", kind.as_str()));
    }
    let gap = match kind {
        Kind::Distill => every,
        Kind::Curate => CURATE_EVERY_HOURS,
    } * 3600;
    if let Some(last) = last_run(state, kind)
        && now.saturating_sub(last) < gap
    {
        return no(format!("the last automatic {} ran {} h ago, under {} h", kind.as_str(), now.saturating_sub(last) / 3600, gap / 3600));
    }
    match kind {
        Kind::Distill => {
            let (n, high) = kept(state);
            let batch = distill::batch(config_dir);
            if high || n >= batch {
                Gate { due: true, reason: format!("{n} kept candidates wait") }
            } else {
                no(format!("{n} kept candidates, none of high priority, under a batch of {batch}"))
            }
        }
        Kind::Curate if curate_count >= CURATE_MIN => Gate { due: true, reason: format!("{curate_count} curate candidates wait") },
        Kind::Curate => no(format!("{curate_count} curate candidates, under {CURATE_MIN}")),
    }
}

/// Claims the run of `kind` when it is due and records its start, so no other session starts one
/// within the gap. The lock makes the check and the record one step across processes; it is dropped
/// again before this returns. Returns the gate either way.
pub fn claim(state: &Path, config_dir: &Path, kind: Kind, curate_count: usize, now: u64) -> Result<Gate> {
    let g = gate(state, config_dir, kind, curate_count, now);
    if !g.due {
        return Ok(g);
    }
    std::fs::create_dir_all(state).map_err(io(state))?;
    let lock = lock_path(state, kind);
    // A stale lock from a run that died is replaced; a fresh one means another session won.
    let _ = std::fs::remove_file(&lock).ok().filter(|_| !held(state, kind, now));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock) {
        Ok(mut f) => std::io::Write::write_all(&mut f, now.to_string().as_bytes()).map_err(io(&lock))?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(no(format!("an automatic {} runs already", kind.as_str()))),
        Err(e) => return Err(io(&lock)(e)),
    }
    let mut last: serde_json::Value =
        std::fs::read_to_string(last_path(state)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| serde_json::json!({}));
    last[kind.as_str()] = now.into();
    let tmp = state.join(".auto.json.tmp");
    std::fs::write(&tmp, last.to_string()).map_err(io(&tmp))?;
    std::fs::rename(&tmp, last_path(state)).map_err(io(&tmp))?;
    release(state, kind);
    Ok(g)
}

/// Drops the claim lock of `kind`.
fn release(state: &Path, kind: Kind) {
    let _ = std::fs::remove_file(lock_path(state, kind));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::distill::{Candidate, Meta};

    fn kept_item(state: &Path, priority: u8) {
        let meta = Meta {
            kind: distill::Kind::Note,
            time: 1,
            harness: None,
            session: None,
            cwd: None,
            signals: vec![],
            priority: Some(priority),
            source: None,
            candidates: vec![Candidate {
                index: 0,
                line: None,
                verdict: Verdict::Keep,
                reason: "jev".into(),
                score: Some(0.9),
                lesson: None,
                kind: None,
                outcome: None,
            }],
        };
        distill::add(state, meta, "a note").unwrap();
    }

    #[test]
    fn off_gated_claimed_and_released() {
        let dir = tempfile::tempdir().unwrap();
        let (state, config) = (dir.path().join("state"), dir.path().join("config"));
        std::fs::create_dir_all(&config).unwrap();
        kept_item(&state, 3);
        let now = 1_800_000_000;
        assert!(gate(&state, &config, Kind::Distill, 0, now).reason.starts_with("off"), "off by default");
        std::fs::write(config.join("config.toml"), "[distill]\nauto = true\n").unwrap();
        assert!(gate(&state, &config, Kind::Distill, 0, now).due, "a high-priority kept candidate is enough");
        assert!(!gate(&state, &config, Kind::Curate, 4, now).due);
        assert!(gate(&state, &config, Kind::Curate, 5, now).due);
        std::fs::write(lock_path(&state, Kind::Distill), now.to_string()).unwrap();
        let busy = claim(&state, &config, Kind::Distill, 0, now).unwrap();
        assert!(!busy.due && busy.reason.contains("runs already"), "a claim in progress: {busy:?}");
        std::fs::remove_file(lock_path(&state, Kind::Distill)).unwrap();
        assert!(claim(&state, &config, Kind::Distill, 0, now).unwrap().due);
        assert!(!lock_path(&state, Kind::Distill).exists(), "the claim drops its lock");
        let again = claim(&state, &config, Kind::Distill, 0, now + 60).unwrap();
        assert!(!again.due && again.reason.contains("under 6 h"), "the gap blocks a second run: {again:?}");
        let gated = gate(&state, &config, Kind::Distill, 0, now + 2 * 3600);
        assert!(!gated.due && gated.reason.contains("ran 2 h ago, under 6 h"), "{gated:?}");
        assert!(gate(&state, &config, Kind::Distill, 0, now + 7 * 3600).due, "due again after the gap");
        std::fs::write(lock_path(&state, Kind::Curate), (now - 2 * 3600).to_string()).unwrap();
        assert!(claim(&state, &config, Kind::Curate, 5, now).unwrap().due, "a stale lock is replaced");
    }
}
