use std::ffi::OsString;
use std::path::PathBuf;

fn xdg(value: Option<OsString>, home: Option<OsString>, fallback: &str) -> PathBuf {
    match value.filter(|v| !v.is_empty()) {
        Some(v) => PathBuf::from(v).join("rkb"),
        None => home.map(PathBuf::from).unwrap_or_default().join(fallback).join("rkb"),
    }
}

/// Per-machine state: requests, usage records. Never synced.
pub fn state_dir() -> PathBuf {
    xdg(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME"), ".local/state")
}

/// Per-machine config such as `trust.toml`. Never synced.
pub fn config_dir() -> PathBuf {
    xdg(std::env::var_os("XDG_CONFIG_HOME"), std::env::var_os("HOME"), ".config")
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
    }
}
