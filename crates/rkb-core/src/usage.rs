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
    /// `worked`, `failed`, `irrelevant`, `injected`, `inferred` or `restored`. Records written before the knowledge-base store
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
    /// Offered for a problem it did not fit. Harmful, so it lowers the ranking factor a little.
    pub irrelevant: u32,
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
    /// The helpful count: each `worked` is 1, each `inferred` is 0.5. An inference is weaker
    /// evidence than a report, so it counts for less.
    pub fn helpful(&self) -> f64 {
        f64::from(self.worked) + 0.5 * f64::from(self.inferred)
    }

    /// The harmful count: each `failed` and each `irrelevant` is 1. Both say the lesson did not
    /// help where it was offered.
    pub fn harmful(&self) -> f64 {
        f64::from(self.failed) + f64::from(self.irrelevant)
    }

    /// The ranking factor: a little up for lessons that helped, a little down for ones that harmed,
    /// never outside 0.9–1.1, so usage reorders close results but never hides a lesson.
    pub fn factor(&self) -> f64 {
        let up = 0.05 * (1.0 + self.helpful()).ln();
        let down = 0.1 * self.harmful().min(3.0) / 3.0;
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
        add(out.entry(r.id.clone()).or_default(), &r);
    }
    out
}

/// Adds one record to a lesson's counts.
fn add(c: &mut Counts, r: &Record) {
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
        "irrelevant" => c.irrelevant += 1,
        // A restoration carries no signal of its own, but it is a use event for the dates.
        "restored" => {}
        _ => return,
    }
    c.last_any = max(c.last_any.take(), &date);
}

/// The counts for one lesson from the records after its last `restored` record. A restoration starts
/// the counts again, so a lesson that was restored is not demoted for its old bad reports.
pub fn counts_since_restore(root: &Path, id: &str) -> Counts {
    let records = records(root);
    let last = records.iter().filter(|r| r.id == id && r.event == "restored").map(|r| r.time.as_str()).max();
    let mut out = Counts::default();
    for r in records.iter().filter(|r| r.id == id) {
        if last.is_some_and(|t| r.time.as_str() <= t) {
            continue;
        }
        add(&mut out, r);
    }
    out
}

/// How many `irrelevant` reports a lesson has since its last `worked` report, after its last
/// restoration. A restoration restarts the count like it restarts the tier counts.
pub fn irrelevant_since_worked(root: &Path, id: &str) -> u32 {
    let all = records(root);
    let mut mine: Vec<&Record> = all.iter().filter(|r| r.id == id).collect();
    mine.sort_by(|a, b| a.time.as_str().cmp(b.time.as_str()));
    let mut out = 0;
    for r in mine {
        match r.event.as_str() {
            "worked" | "restored" => out = 0,
            "irrelevant" => out += 1,
            _ => {}
        }
    }
    out
}

/// The cleaned failure query of the last replay in `session` that pushed `id`, if any.
/// The hook writes the replay log; `rkb used --worked` reads it back to learn the query.
pub fn replay_query(state: &Path, session: &str, id: &str) -> Option<String> {
    std::fs::read_to_string(state.join("replays.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["session"] == session && v["injected"] == id)
        .filter_map(|v| v["query"].as_str().map(String::from))
        .next_back()
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
        record(kb.path(), cfg.path(), state.path(), &now("b", "irrelevant", None, Some("about pi".into()))).unwrap();
        let other = kb.path().join(DIR).join("other-0000.jsonl");
        std::fs::write(&other, format!("{}\nnot json\n", serde_json::to_string(&now("a", "worked", None, None)).unwrap())).unwrap();

        let c = &counts(kb.path())["a"];
        assert_eq!((c.worked, c.failed, c.injected, c.inferred), (2, 1, 2, 0), "old state files moved in, other machines summed");
        let b = &counts(kb.path())["b"];
        assert_eq!((b.irrelevant, b.worked, b.last_any.is_some()), (1, 0, true));
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
        // `irrelevant` is harmful too, so it lowers the factor like `failed` does.
        let worked = Counts { worked: 2, ..Default::default() };
        assert!(Counts { irrelevant: 7, ..worked.clone() }.factor() < worked.factor());
        assert!((Counts { irrelevant: 7, ..worked.clone() }.factor() - 0.955).abs() < 0.005);
        let one = Counts { irrelevant: 1, ..worked }.factor();
        assert!((0.9..=1.1).contains(&one) && one > 0.9, "{one}");
    }

    #[test]
    fn helpful_and_harmful_counts() {
        let c = Counts { worked: 2, inferred: 2, failed: 1, irrelevant: 1, ..Default::default() };
        assert!((c.helpful() - 3.0).abs() < 1e-9);
        assert!((c.harmful() - 2.0).abs() < 1e-9);
        assert_eq!(Counts::default().helpful(), 0.0);
    }

    #[test]
    fn a_restoration_restarts_the_counts() {
        let kb = tempfile::tempdir().unwrap();
        let dir = kb.path().join(DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let rec = |event: &str, time: &str| Record { time: time.into(), id: "a".into(), event: event.into(), session: None, reason: None };
        let lines: String = [
            rec("worked", "2026-10-01T00:00:00Z"),
            rec("irrelevant", "2026-10-02T00:00:00Z"),
            rec("irrelevant", "2026-10-03T00:00:00Z"),
            rec("restored", "2026-10-04T00:00:00Z"),
            rec("worked", "2026-10-05T00:00:00Z"),
        ]
        .iter()
        .map(|r| serde_json::to_string(r).unwrap() + "\n")
        .collect();
        std::fs::write(dir.join("m-0000.jsonl"), lines).unwrap();
        let all = &counts(kb.path())["a"];
        assert_eq!((all.worked, all.irrelevant), (2, 2));
        let since = counts_since_restore(kb.path(), "a");
        assert_eq!((since.worked, since.irrelevant), (1, 0), "only the record after the restoration counts");
        assert_eq!(counts_since_restore(kb.path(), "b"), Counts::default(), "a lesson with no record is empty");
    }
}
