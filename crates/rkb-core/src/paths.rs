use std::ffi::OsString;
use std::path::PathBuf;

fn xdg(value: Option<OsString>, home: Option<OsString>, fallback: &str) -> PathBuf {
    match value.filter(|v| !v.is_empty()) {
        Some(v) => PathBuf::from(v).join("rkb"),
        None => home.map(PathBuf::from).unwrap_or_default().join(fallback).join("rkb"),
    }
}

/// `$HOME`, or with no `HOME` the folder `.git/rkb-home` in the knowledge base that `RKB_HOME` names:
/// inside `.git` it is never synced, and it keeps state out of the working directory of a container.
pub fn home_base() -> Option<OsString> {
    let set = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    set("HOME").or_else(|| set("RKB_TEST_HOME").or_else(|| set("RKB_HOME")).map(|_| crate::kb::home().join(".git/rkb-home").into()))
}

/// Per-machine state: requests, usage records. Never synced.
pub fn state_dir() -> PathBuf {
    xdg(std::env::var_os("XDG_STATE_HOME"), home_base(), ".local/state")
}

/// Deletable caches such as fact results. Never synced.
pub fn cache_dir() -> PathBuf {
    xdg(std::env::var_os("XDG_CACHE_HOME"), home_base(), ".cache")
}

/// Per-machine config such as `trust.toml`. Never synced.
pub fn config_dir() -> PathBuf {
    xdg(std::env::var_os("XDG_CONFIG_HOME"), home_base(), ".config")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_and_defaults() {
        let home = || Some(OsString::from("/h"));
        assert_eq!(xdg(Some("/s".into()), home(), ".local/state"), PathBuf::from("/s/rkb"));
        assert_eq!(xdg(None, home(), ".config"), PathBuf::from("/h/.config/rkb"));
        assert_eq!(xdg(Some("".into()), home(), ".local/state"), PathBuf::from("/h/.local/state/rkb"));
        assert_eq!(xdg(None, Some("/kb/.git/rkb-home".into()), ".cache"), PathBuf::from("/kb/.git/rkb-home/.cache/rkb"));
    }
}
