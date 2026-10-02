//! Model jobs: a model step that rkb prepares and another runner answers, such as the Claude Code mod
//! through its own model call. rkb keeps the prompt, the parsing and every write; the runner only asks
//! the model. A job is a file in the state folder between `rkb job prepare` and `rkb job finish`.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, io};
use crate::observer;
use crate::request;

/// A job that is not finished within an hour is dropped, and finishing it fails.
pub const EXPIRY_S: u64 = 3600;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Job {
    Observe(observer::Run),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stored {
    pub id: String,
    pub created: u64,
    #[serde(flatten)]
    pub job: Job,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Missing {
    Expired,
    NotFound,
}

fn dir(state: &Path) -> PathBuf {
    state.join("jobs")
}

fn file(state: &Path, id: &str) -> PathBuf {
    dir(state).join(format!("{id}.json"))
}

pub fn new_id() -> String {
    let mut b = [0u8; 4];
    getrandom::fill(&mut b).expect("system random source");
    format!("j-{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
}

/// Writes the job readable only by the user, replacing an earlier state of the same job.
pub fn save(state: &Path, job: &Stored) -> Result<()> {
    let d = dir(state);
    std::fs::create_dir_all(&d).map_err(io(&d))?;
    let path = file(state, &job.id);
    let tmp = d.join(format!(".{}.tmp", job.id));
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(io(&tmp))?;
    f.write_all(serde_json::to_string(job).expect("job serializes").as_bytes()).map_err(io(&tmp))?;
    std::fs::rename(&tmp, &path).map_err(io(&path))
}

pub fn load(state: &Path, id: &str) -> std::result::Result<Stored, Missing> {
    let valid = id.len() <= 16 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    let text = std::fs::read_to_string(file(state, id)).ok().filter(|_| valid).ok_or(Missing::NotFound)?;
    let job: Stored = serde_json::from_str(&text).map_err(|_| Missing::NotFound)?;
    if request::now().saturating_sub(job.created) > EXPIRY_S {
        remove(state, id);
        return Err(Missing::Expired);
    }
    Ok(job)
}

pub fn remove(state: &Path, id: &str) {
    let _ = std::fs::remove_file(file(state, id));
}

/// The jobs that have not expired; expired ones are removed.
pub fn open(state: &Path) -> Vec<Stored> {
    let mut out = vec![];
    for e in std::fs::read_dir(dir(state)).into_iter().flatten().flatten() {
        let job = std::fs::read_to_string(e.path()).ok().and_then(|t| serde_json::from_str::<Stored>(&t).ok());
        match job {
            Some(j) if request::now().saturating_sub(j.created) <= EXPIRY_S => out.push(j),
            _ if e.path().extension().is_some_and(|x| x == "json") => {
                let _ = std::fs::remove_file(e.path());
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> observer::Run {
        observer::Run {
            session: "s1".into(),
            harness: "claude-code".into(),
            cwd: None,
            models: vec![("sonnet".into(), Some("sonnet".into()))],
            titles: vec![],
            parts: vec!["part".into()],
            end: 10,
            next: 0,
            kept: vec![],
            dropped: 0,
            first_dropped: None,
            via: vec![],
        }
    }

    #[test]
    fn save_load_expire() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path();
        let job = Stored { id: new_id(), created: request::now(), job: Job::Observe(run()) };
        save(state, &job).unwrap();
        assert_eq!(load(state, &job.id), Ok(job.clone()));
        assert_eq!(open(state).len(), 1);
        assert_eq!(load(state, "j-00000000"), Err(Missing::NotFound));
        assert_eq!(load(state, "../../etc/passwd"), Err(Missing::NotFound));
        let old = Stored { id: new_id(), created: request::now() - EXPIRY_S - 1, ..job.clone() };
        save(state, &old).unwrap();
        assert_eq!(load(state, &old.id), Err(Missing::Expired));
        save(state, &old).unwrap();
        assert_eq!(open(state), vec![job]);
        assert!(!file(state, &old.id).exists(), "open drops expired jobs");
    }
}
