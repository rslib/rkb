//! Use records: whether a lesson worked, failed, was injected by a hook, or was followed by a fix.
//! They live in the knowledge base, one append-only file per machine under `.rkb/usage/`, so every
//! machine sees every other machine's records after `rkb sync`, and two machines never edit one file.
//! The folder is hidden, so lint, search and the site never read it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::lock;

pub const DIR: &str = ".rkb/usage";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub time: String,
    pub id: String,
    /// `worked`, `failed`, `injected` or `inferred`. Records written before the knowledge-base store
    /// called it `result`.
    #[serde(alias = "result")]
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Counts of each event for one lesson over every machine, with the date (`YYYY-MM-DD`) of the last one.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Counts {
    pub worked: u32,
    pub failed: u32,
    pub injected: u32,
    pub inferred: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_worked: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_failed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_injected: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_any: Option<String>,
}

impl Counts {
    /// The ranking factor: a little up for lessons that worked, a little down for ones that failed,
    /// never outside 0.9–1.1, so usage reorders close results but never hides a lesson.
    pub fn factor(&self) -> f64 {
        let up = 0.05 * (1.0 + f64::from(self.worked) + 0.5 * f64::from(self.inferred)).ln();
        let down = 0.1 * f64::from(self.failed.min(3)) / 3.0;
        (1.0 + up - down).clamp(0.9, 1.1)
    }
}

/// This machine's id: the slugified host name plus 4 random hex, made once and kept in the config
/// folder, so login nodes that share a host name pattern still get their own file.
pub fn machine(config: &Path) -> String {
    let path = config.join("machine");
    if let Some(id) = std::fs::read_to_string(&path).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        return id;
    }
    let host: String = lock::hostname().chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let host = host.trim_matches('-').split('.').next().unwrap_or("host").to_string();
    let mut b = [0u8; 2];
    getrandom::fill(&mut b).expect("system random source");
    let id = format!("{}-{:02x}{:02x}", if host.is_empty() { "host" } else { &host }, b[0], b[1]);
    let _ = std::fs::create_dir_all(config);
    // The first process to create the file wins; a parallel one reads the winner's id.
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut f) => {
            use std::io::Write;
            let _ = writeln!(f, "{id}");
            id
        }
        Err(_) => {
            for _ in 0..50 {
                if let Some(won) = std::fs::read_to_string(&path).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
                    return won;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            id
        }
    }
}

pub fn file(root: &Path, config: &Path) -> PathBuf {
    root.join(DIR).join(format!("{}.jsonl", machine(config)))
}

/// The session a CLI call runs in, from the harness's environment.
pub fn env_session() -> Option<String> {
    ["CLAUDE_CODE_SESSION_ID", "PI_SESSION_ID", "RKB_SESSION"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
}

/// Appends one record to this machine's file. The first append moves the records of the per-machine
/// state files an earlier rkb wrote into it.
pub fn record(root: &Path, config: &Path, state: &Path, r: &Record) -> Result<()> {
    let path = file(root, config);
    let ignore = root.join(".rkb/.gitignore");
    if !ignore.exists() {
        std::fs::create_dir_all(root.join(".rkb")).map_err(crate::error::io(root))?;
        std::fs::write(&ignore, "*.lock\n").map_err(crate::error::io(&ignore))?;
    }
    migrate(state, &path)?;
    lock::append_line(&path, &serde_json::to_string(r).expect("record serializes"))
}

fn migrate(state: &Path, to: &Path) -> Result<()> {
    for (name, event) in [("usage.jsonl", None), ("injections.jsonl", Some("injected"))] {
        let old = state.join(name);
        let Ok(text) = std::fs::read_to_string(&old) else { continue };
        for line in text.lines() {
            let Ok(mut v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            if let Some(e) = event {
                v["event"] = e.into();
            }
            if let Ok(r) = serde_json::from_value::<Record>(v) {
                lock::append_line(to, &serde_json::to_string(&r).expect("record serializes"))?;
            }
        }
        let _ = std::fs::rename(&old, old.with_extension("jsonl.migrated"));
    }
    Ok(())
}

/// Every record of every machine, oldest file order kept per machine. Malformed lines are skipped.
pub fn records(root: &Path) -> Vec<Record> {
    let mut out = vec![];
    let mut files: Vec<PathBuf> = std::fs::read_dir(root.join(DIR))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    files.sort();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        out.extend(text.lines().filter_map(|l| serde_json::from_str::<Record>(l).ok()));
    }
    out
}

pub fn counts(root: &Path) -> BTreeMap<String, Counts> {
    let mut out: BTreeMap<String, Counts> = BTreeMap::new();
    for r in records(root) {
        let c = out.entry(r.id.clone()).or_default();
        let date = r.time.get(..10).unwrap_or(&r.time).to_string();
        match r.event.as_str() {
            "worked" => {
                c.worked += 1;
                c.last_worked = max(c.last_worked.take(), &date);
            }
            "failed" => {
                c.failed += 1;
                c.last_failed = max(c.last_failed.take(), &date);
            }
            "injected" => {
                c.injected += 1;
                c.last_injected = max(c.last_injected.take(), &date);
            }
            "inferred" => c.inferred += 1,
            _ => continue,
        }
        c.last_any = max(c.last_any.take(), &date);
    }
    out
}

fn max(a: Option<String>, b: &str) -> Option<String> {
    Some(a.filter(|a| a.as_str() >= b).unwrap_or_else(|| b.to_string()))
}

/// The session of this machine's last injection of `id` within 6 hours, to link a later report to it.
pub fn recent_injection(root: &Path, config: &Path, id: &str) -> Option<String> {
    let text = std::fs::read_to_string(file(root, config)).ok()?;
    let cutoff = jiff::Timestamp::now().checked_sub(jiff::SignedDuration::from_hours(6)).ok()?;
    text.lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<Record>(l).ok())
        .find(|r| r.id == id && r.event == "injected" && r.time.parse::<jiff::Timestamp>().is_ok_and(|t| t >= cutoff))
        .and_then(|r| r.session)
}

/// Stages this machine's new use records, so the next commit carries them. Returns whether any were new.
pub fn stage(root: &Path) -> Result<bool> {
    if crate::git::run(root, &["status", "--porcelain", "--", ".rkb"])?.is_empty() {
        return Ok(false);
    }
    crate::git::run(root, &["add", "--", ".rkb"])?;
    Ok(true)
}

/// Commits staged use records on their own, as `rkb sync` does before it pulls.
pub fn commit(root: &Path) -> Result<bool> {
    if !stage(root)? {
        return Ok(false);
    }
    crate::git::commit_paths(root, "usage: record lesson use", &[".rkb"])?;
    Ok(true)
}

/// A record stamped now.
pub fn now(id: &str, event: &str, session: Option<String>, reason: Option<String>) -> Record {
    Record { time: jiff::Timestamp::now().to_string(), id: id.to_string(), event: event.to_string(), session, reason }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_counts_and_machines() {
        let kb = tempfile::tempdir().unwrap();
        let (cfg, state) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(state.path().join("usage.jsonl"), "{\"time\":\"2026-09-01T00:00:00Z\",\"id\":\"a\",\"result\":\"worked\"}\n")
            .unwrap();
        std::fs::write(
            state.path().join("injections.jsonl"),
            "{\"time\":\"2026-09-02T00:00:00Z\",\"session\":\"s0\",\"id\":\"a\",\"ranked_by\":\"bm25\"}\n",
        )
        .unwrap();
        record(kb.path(), cfg.path(), state.path(), &now("a", "injected", Some("s1".into()), None)).unwrap();
        record(kb.path(), cfg.path(), state.path(), &now("a", "failed", None, Some("broke".into()))).unwrap();
        let other = kb.path().join(DIR).join("other-0000.jsonl");
        std::fs::write(&other, format!("{}\nnot json\n", serde_json::to_string(&now("a", "worked", None, None)).unwrap())).unwrap();

        let c = &counts(kb.path())["a"];
        assert_eq!((c.worked, c.failed, c.injected, c.inferred), (2, 1, 2, 0), "old state files moved in, other machines summed");
        assert!(state.path().join("usage.jsonl.migrated").exists() && !state.path().join("usage.jsonl").exists());
        assert_eq!(std::fs::read_to_string(kb.path().join(".rkb/.gitignore")).unwrap(), "*.lock\n");
        assert_eq!(recent_injection(kb.path(), cfg.path(), "a").as_deref(), Some("s1"));
        let m = machine(cfg.path());
        assert_eq!(m, machine(cfg.path()), "the id is kept");
        assert!(file(kb.path(), cfg.path()).ends_with(format!("{m}.jsonl")));
    }

    #[test]
    fn factor_stays_in_bounds() {
        let base = Counts::default().factor();
        assert!((base - 1.0).abs() < 1e-9);
        assert!(Counts { worked: 3, ..Default::default() }.factor() > 1.0);
        assert!((Counts { worked: 10_000, ..Default::default() }.factor() - 1.1).abs() < 1e-9);
        assert!((Counts { failed: 9, ..Default::default() }.factor() - 0.9).abs() < 1e-9);
    }
}
