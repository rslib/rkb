use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, io};
use crate::matching::Place;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub sha256: String,
    pub system: String,
    /// What the script belongs to, for the reader only; it plays no part in the match.
    #[serde(default)]
    pub lesson: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub date: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    approval: Vec<Approval>,
}

pub fn path(config_dir: &Path) -> PathBuf {
    config_dir.join("approvals.toml")
}

/// The system an approval is for here: the matched system, or `host:<hostname>` when none matches.
pub fn system_key(place: &Place) -> String {
    match &place.system {
        Some(s) => s.name.clone(),
        None => format!("host:{}", crate::lock::hostname()),
    }
}

fn read(config_dir: &Path) -> File {
    std::fs::read_to_string(path(config_dir)).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
}

pub fn is_approved(config_dir: &Path, sha256: &str, system: &str) -> bool {
    read(config_dir).approval.iter().any(|a| a.sha256 == sha256 && a.system == system)
}

/// Records an approval; the file stays readable only by the user. Returns false when it was already there.
pub fn add(config_dir: &Path, approval: Approval) -> Result<bool> {
    let mut file = read(config_dir);
    if file.approval.iter().any(|a| a.sha256 == approval.sha256 && a.system == approval.system) {
        return Ok(false);
    }
    file.approval.push(approval);
    std::fs::create_dir_all(config_dir).map_err(io(config_dir))?;
    let target = path(config_dir);
    let tmp = config_dir.join(".approvals.toml.tmp");
    std::fs::write(&tmp, toml::to_string(&file).expect("approvals serialize")).map_err(io(&tmp))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).map_err(io(&tmp))?;
    std::fs::rename(&tmp, &target).map_err(io(&target))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approval(sha: &str, system: &str) -> Approval {
        Approval { sha256: sha.into(), system: system.into(), lesson: "0a1b2c3d4e".into(), kind: "check".into(), date: "2026-09-26".into() }
    }

    #[test]
    fn approve_changed_text_and_other_system() {
        let dir = tempfile::tempdir().unwrap();
        let sha = crate::script::hash("exit 0\n");
        assert!(!is_approved(dir.path(), &sha, "tuolumne"));
        assert!(add(dir.path(), approval(&sha, "tuolumne")).unwrap());
        assert!(!add(dir.path(), approval(&sha, "tuolumne")).unwrap(), "already there");
        assert!(is_approved(dir.path(), &sha, "tuolumne"));
        assert!(!is_approved(dir.path(), &crate::script::hash("exit 0 \n"), "tuolumne"), "changed text");
        assert!(!is_approved(dir.path(), &sha, "host:laptop"), "another system");
        let mode = std::fs::metadata(path(dir.path())).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(system_key(&Place::default()), format!("host:{}", crate::lock::hostname()));
    }
}
