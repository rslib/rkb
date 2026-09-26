use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::lock;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub time: String,
    pub id: String,
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub fn log_path(state: &Path) -> PathBuf {
    state.join("usage.jsonl")
}

/// Appends one use record. `result` is `worked` or `failed`.
pub fn record(state: &Path, id: &str, result: &str, reason: Option<&str>) -> Result<()> {
    let r = Record {
        time: jiff::Timestamp::now().to_string(),
        id: id.to_string(),
        result: result.to_string(),
        reason: reason.map(str::to_string),
    };
    lock::append_line(&log_path(state), &serde_json::to_string(&r).expect("record serializes"))
}

/// The dates (`YYYY-MM-DD`) of the last `worked` and the last `failed` record for `id`.
pub fn last(state: &Path, id: &str) -> (Option<String>, Option<String>) {
    let text = std::fs::read_to_string(log_path(state)).unwrap_or_default();
    let mut worked = None;
    let mut failed = None;
    for r in text.lines().filter_map(|l| serde_json::from_str::<Record>(l).ok()).filter(|r| r.id == id) {
        let date = r.time.get(..10).unwrap_or(&r.time).to_string();
        match r.result.as_str() {
            "worked" => worked = Some(date),
            "failed" => failed = Some(date),
            _ => {}
        }
    }
    (worked, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_last() {
        let dir = tempfile::tempdir().unwrap();
        record(dir.path(), "a", "worked", None).unwrap();
        record(dir.path(), "b", "failed", Some("broke")).unwrap();
        let (w, f) = last(dir.path(), "a");
        assert_eq!(w.as_deref(), Some(&jiff::Timestamp::now().to_string()[..10]));
        assert_eq!(f, None);
        assert!(last(dir.path(), "b").1.is_some());
        assert_eq!(std::fs::read_to_string(log_path(dir.path())).unwrap().lines().count(), 2);
    }
}
