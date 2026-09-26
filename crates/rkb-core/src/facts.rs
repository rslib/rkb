use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::approval;
use crate::config::{self, KbConfig};
use crate::matching::Place;
use crate::script;

pub const DEFAULT_TTL_HOURS: i64 = 24;
pub const DEFAULT_TIMEOUT_S: i64 = 10;

fn kb(root: &Path) -> KbConfig {
    std::fs::read_to_string(root.join("kb.toml")).ok().and_then(|t| config::parse(&t).ok()).unwrap_or_default()
}

fn setting(kb: &KbConfig, key: &str, default: i64) -> i64 {
    kb.facts.as_ref().and_then(|t| t.get(key)).and_then(|v| v.as_integer()).unwrap_or(default)
}

/// The script text of a fact command: the `cmd` string with one trailing newline, as it is hashed and run.
pub fn text(cmd: &str) -> String {
    format!("{}\n", cmd.trim_end())
}

/// `(key, script text)` of every `[facts.<key>]` table in `kb.toml` that has a `cmd`.
pub fn commands(root: &Path) -> Vec<(String, String)> {
    kb(root).facts.iter().flatten().filter_map(|(k, v)| Some((k.clone(), text(v.as_table()?.get("cmd")?.as_str()?)))).collect()
}

/// The cache entry name for a script on this system with the current `LOADEDMODULES` and `PATH`.
pub fn cache_key(sha256: &str, system: &str) -> String {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    script::hash(&format!("{sha256}|{system}|{}|{}", env("LOADEDMODULES"), env("PATH")))
}

#[derive(Serialize, Deserialize)]
struct Entry {
    value: Option<String>,
    time: u64,
}

fn entry_path(key: &str) -> PathBuf {
    crate::paths::cache_dir().join("facts").join(format!("{key}.json"))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A cached value younger than `ttl`: `Some(None)` for a cached unknown, `None` when missing, stale or corrupt.
pub fn get(key: &str, ttl: Duration) -> Option<Option<String>> {
    let e: Entry = serde_json::from_str(&std::fs::read_to_string(entry_path(key)).ok()?).ok()?;
    (now().saturating_sub(e.time) < ttl.as_secs()).then_some(e.value)
}

/// Stores a value through a temporary file and a rename. A cache is disposable, so a failure is ignored.
pub fn put(key: &str, value: Option<&str>) {
    let path = entry_path(key);
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = dir.join(format!(".{key}.tmp"));
    let body = serde_json::to_string(&Entry { value: value.map(str::to_string), time: now() }).unwrap_or_default();
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// The fact from a command's result: exit 0 and exactly one non-empty line, trimmed.
pub fn value_of(run: script::Run, output: &str) -> Option<String> {
    let lines: Vec<&str> = output.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    (run.result == script::Result::Pass && lines.len() == 1).then(|| lines[0].to_string())
}

pub fn ttl(root: &Path) -> Duration {
    Duration::from_secs(setting(&kb(root), "cache_ttl", DEFAULT_TTL_HOURS).max(0) as u64 * 3600)
}

/// Runs every approved fact command whose cache entry is missing or stale, and caches the result.
pub fn refresh(root: &Path, place: &Place) {
    let system = approval::system_key(place);
    let config_dir = crate::paths::config_dir();
    let (ttl, timeout) = (ttl(root), Duration::from_secs(setting(&kb(root), "timeout_s", DEFAULT_TIMEOUT_S).max(1) as u64));
    for (_, text) in commands(root) {
        let sha = script::hash(&text);
        let key = cache_key(&sha, &system);
        if !approval::is_approved(&config_dir, &sha, &system) || get(&key, ttl).is_some() {
            continue;
        }
        let (run, output) = script::run_capture(&text, None, timeout);
        put(&key, value_of(run, &output).as_deref());
    }
}

/// The fresh cached value of every approved fact command. Runs nothing.
pub fn cached(root: &Path, place: &Place) -> Vec<(String, String)> {
    let system = approval::system_key(place);
    let config_dir = crate::paths::config_dir();
    let ttl = ttl(root);
    commands(root)
        .into_iter()
        .filter_map(|(k, text)| {
            let sha = script::hash(&text);
            if !approval::is_approved(&config_dir, &sha, &system) {
                return None;
            }
            Some((k, get(&cache_key(&sha, &system), ttl)??))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(result: script::Result) -> script::Run {
        script::Run { result, code: None }
    }

    #[test]
    fn value_rule() {
        assert_eq!(value_of(run(script::Result::Pass), "1.14.3\n").as_deref(), Some("1.14.3"));
        assert_eq!(value_of(run(script::Result::Pass), "\n  1.14.3  \n\n").as_deref(), Some("1.14.3"));
        assert_eq!(value_of(run(script::Result::Pass), "1.14.3\n1.12\n"), None, "two lines");
        assert_eq!(value_of(run(script::Result::Pass), ""), None);
        assert_eq!(value_of(run(script::Result::Fail), "1.14.3\n"), None);
        assert_eq!(text("echo x  \n\n"), "echo x\n");
    }

    #[test]
    fn cache_key_parts_ttl_and_corrupt_entry() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: tests in this module are the only ones that read XDG_CACHE_HOME.
        unsafe { std::env::set_var("XDG_CACHE_HOME", dir.path()) };
        let a = cache_key("abc", "tuolumne");
        assert_ne!(a, cache_key("abd", "tuolumne"));
        assert_ne!(a, cache_key("abc", "host:laptop"));
        assert_eq!(get(&a, Duration::from_secs(60)), None);
        put(&a, Some("1.14.3"));
        assert_eq!(get(&a, Duration::from_secs(60)), Some(Some("1.14.3".into())));
        assert_eq!(get(&a, Duration::ZERO), None, "stale");
        put(&a, None);
        assert_eq!(get(&a, Duration::from_secs(60)), Some(None), "a cached unknown");
        std::fs::write(entry_path(&a), "{ not json").unwrap();
        assert_eq!(get(&a, Duration::from_secs(60)), None, "corrupt is missing");
    }
}
