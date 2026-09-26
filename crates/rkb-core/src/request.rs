use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result, io};

pub const EXPIRY_S: u64 = 3600;

/// A write that can be run again after the user decides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    Add {
        text: String,
        topic: String,
    },
    Edit {
        id: String,
        text: String,
        base: String,
    },
    Flag {
        id: String,
        reason: String,
    },
    Supersede {
        id: String,
        by: String,
        reason: String,
    },
    Archive {
        target: String,
        reason: String,
    },
    Unarchive {
        id: String,
    },
    Move {
        id: String,
        folder: String,
    },
    Rename {
        id: String,
        slug: String,
    },
    /// Approve a lesson's script on this system; with `then_verify`, run the check right after.
    Approve {
        id: String,
        script: crate::script::Kind,
        sha256: String,
        system: String,
        then_verify: bool,
    },
    BreakLock {
        path: String,
        holder: String,
    },
    Install {
        harnesses: Vec<crate::install::Harness>,
        uninstall: bool,
    },
    Import {
        items: Vec<ImportItem>,
    },
}

/// One lesson of an import batch, with the decisions its report showed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportItem {
    /// The file, relative to the import directory.
    pub source: String,
    pub topic: String,
    pub text: String,
    pub title: String,
    /// Where the report said the lesson goes.
    pub path: String,
    pub approved: Vec<Decision>,
    pub new_folder: Option<String>,
    /// Ids of the lessons it resembles; empty when it is not a duplicate.
    pub similar: Vec<String>,
    /// `key=value` of a label that becomes looser.
    pub looser: Option<String>,
}

/// A decision the user confirmed. Each one lets the write pipeline pass one check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Decision {
    CreateFolder { path: String },
    UseTopic { path: String },
    AddAnyway,
    Loosen { key: String, value: String },
    CreateProject { path: String, remotes: Vec<String>, root_commit: Option<String> },
    BreakLock,
    Install,
    ImportAll,
    SkipDuplicates,
    Supersede,
    Archive,
    Approve,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    pub text: String,
    /// `None` cancels the request.
    pub decision: Option<Decision>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub created: u64,
    pub action: Action,
    pub approved: Vec<Decision>,
    pub question: String,
    pub choices: Vec<Choice>,
}

impl Request {
    pub fn options(&self) -> Vec<String> {
        self.choices.iter().map(|c| c.text.clone()).collect()
    }
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The harness this process runs under, when `trust.toml` in `config` marks it as gated.
/// Claude Code sets `CLAUDECODE=1`; rkb's pi and omp extension sets `RKB_HARNESS`.
pub fn trusted_harness(config: &Path) -> Option<&'static str> {
    trusted_harness_with(config, &|k| std::env::var(k).ok())
}

fn trusted_harness_with(config: &Path, env: &dyn Fn(&str) -> Option<String>) -> Option<&'static str> {
    let name = if env("CLAUDECODE").as_deref() == Some("1") {
        "claude-code"
    } else {
        match env("RKB_HARNESS").as_deref() {
            Some("pi") => "pi",
            Some("omp") => "omp",
            _ => return None,
        }
    };
    let text = std::fs::read_to_string(config.join("trust.toml")).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    let gated = table.get("harness")?.get(name)?.get("gated")?.as_bool()?;
    gated.then_some(name)
}

pub fn new_id() -> String {
    let mut b = [0u8; 3];
    getrandom::fill(&mut b).expect("system random source");
    format!("r-{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
}

fn file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

/// Stores a request readable only by the user, and removes expired ones.
pub fn save(dir: &Path, req: &Request) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(io(dir))?;
    remove_expired(dir);
    let path = file(dir, &req.id);
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path).map_err(io(&path))?;
    f.write_all(serde_json::to_string(req).expect("request serializes").as_bytes()).map_err(io(&path))
}

pub fn load(dir: &Path, id: &str) -> Result<Request> {
    remove_expired(dir);
    let path = file(dir, id);
    let text = std::fs::read_to_string(&path).map_err(|_| Error::Expired(id.to_string()))?;
    let req: Request = serde_json::from_str(&text).map_err(|_| Error::Expired(id.to_string()))?;
    Ok(req)
}

/// The number of stored requests that have not expired.
pub fn pending(dir: &Path) -> usize {
    remove_expired(dir);
    std::fs::read_dir(dir).map_or(0, |e| e.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "json")).count())
}

pub fn remove(dir: &Path, id: &str) {
    let _ = std::fs::remove_file(file(dir, id));
}

fn remove_expired(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let old = std::fs::read_to_string(e.path())
            .ok()
            .and_then(|t| serde_json::from_str::<Request>(&t).ok())
            .is_none_or(|r| now().saturating_sub(r.created) > EXPIRY_S);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn harness_detection_needs_its_own_trust_entry() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("trust.toml"), "[harness.pi]\ngated = true\n").unwrap();
        let env =
            |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        assert_eq!(trusted_harness_with(dir.path(), &env(&[("RKB_HARNESS", "pi")])), Some("pi"));
        assert_eq!(trusted_harness_with(dir.path(), &env(&[("RKB_HARNESS", "omp")])), None);
        assert_eq!(
            trusted_harness_with(dir.path(), &env(&[("RKB_HARNESS", "pi"), ("CLAUDECODE", "1")])),
            None,
            "Claude Code wins and is not gated here"
        );
        assert_eq!(trusted_harness_with(dir.path(), &env(&[])), None);
    }

    fn req(created: u64) -> Request {
        Request {
            id: new_id(),
            created,
            action: Action::Flag { id: "0a1b2c3d4e".into(), reason: "x".into() },
            approved: vec![Decision::AddAnyway],
            question: "q?".into(),
            choices: vec![Choice { text: "add anyway".into(), decision: Some(Decision::AddAnyway) }],
        }
    }

    #[test]
    fn store_load_and_mode() {
        let dir = tempfile::tempdir().unwrap();
        let r = req(now());
        assert!(r.id.starts_with("r-") && r.id.len() == 8);
        save(dir.path(), &r).unwrap();
        assert_eq!(load(dir.path(), &r.id).unwrap(), r);
        let mode = std::fs::metadata(dir.path().join(format!("{}.json", r.id))).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        remove(dir.path(), &r.id);
        assert!(matches!(load(dir.path(), &r.id), Err(Error::Expired(_))));
    }

    #[test]
    fn expired_requests_are_gone() {
        let dir = tempfile::tempdir().unwrap();
        let r = req(now() - EXPIRY_S - 1);
        save(dir.path(), &r).unwrap();
        assert!(matches!(load(dir.path(), &r.id), Err(Error::Expired(_))));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
