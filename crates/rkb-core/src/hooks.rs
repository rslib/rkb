use std::collections::HashSet;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use regex::Regex;
use serde_json::Value;

use crate::error::{Result, io};
use crate::lock;
use crate::text::tokens;

/// Harness names `rkb hook --harness` accepts; the first is the default.
pub const HARNESSES: [&str; 3] = ["claude-code", "pi", "omp"];
pub const SESSION_DAYS: u64 = 14;
pub const MAX_QUERY_TERMS: usize = 40;
pub const DEFAULT_MIN_COVERAGE: f64 = 0.6;
pub const DEFAULT_MIN_RELEVANCE: f64 = 0.8;
pub const DEFAULT_HOOK_TIMEOUT_MS: u64 = 500;
pub const MAX_NUDGES: usize = 2;

fn sessions_dir(state: &Path) -> PathBuf {
    state.join("sessions")
}

/// The session file for a harness session id, with the id reduced to safe characters.
pub fn session_path(state: &Path, id: &str) -> PathBuf {
    let safe: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(100).collect();
    sessions_dir(state).join(format!("{}.jsonl", if safe.is_empty() { "unknown" } else { &safe }))
}

/// Appends one record; the file is created readable only by the user.
pub fn append(state: &Path, session: &str, record: &Value) -> Result<()> {
    let path = session_path(state, session);
    if !path.exists() {
        std::fs::create_dir_all(sessions_dir(state)).map_err(io(sessions_dir(state)))?;
        std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&path).map_err(io(&path))?;
    }
    lock::append_line(&path, &record.to_string())
}

pub fn read(state: &Path, session: &str) -> Vec<Value> {
    std::fs::read_to_string(session_path(state, session)).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Deletes session files not modified for `SESSION_DAYS` days.
pub fn cleanup(state: &Path) {
    let Ok(entries) = std::fs::read_dir(sessions_dir(state)) else { return };
    let limit = Duration::from_secs(SESSION_DAYS * 24 * 3600);
    for e in entries.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age > limit);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn heartbeat_path(state: &Path, harness: &str) -> PathBuf {
    state.join("heartbeat").join(harness)
}

pub fn beat(state: &Path, harness: &str) -> Result<()> {
    let path = heartbeat_path(state, harness);
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir).map_err(io(dir))?;
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let tmp = dir.join(".beat.tmp");
    std::fs::write(&tmp, now.to_string()).map_err(io(&tmp))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).map_err(io(&tmp))?;
    std::fs::rename(&tmp, &path).map_err(io(&path))
}

/// Seconds since the last hook call, or `None` when there was none.
pub fn last_beat(state: &Path, harness: &str) -> Option<u64> {
    let t: u64 = std::fs::read_to_string(heartbeat_path(state, harness)).ok()?.trim().parse().ok()?;
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    Some(now.saturating_sub(t))
}

/// The program a shell command runs: the first word that is not a `VAR=value` assignment, without its folder.
pub fn program(command: &str) -> Option<String> {
    let word = command.split_whitespace().find(|w| !w.contains('=') || w.starts_with('='))?;
    let word = word.trim_matches(|c| c == '(' || c == '\'' || c == '"');
    let name = word.rsplit('/').next().unwrap_or(word);
    (!name.is_empty()).then(|| name.to_string())
}

static ERROR_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)error|fatal|failed|not found|undefined|cannot|no such").unwrap());

/// The lines of an error that name the problem, or its last 3 lines when none do.
pub fn key_lines(error: &str) -> Vec<&str> {
    let lines: Vec<&str> = error.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("Exit code ")).collect();
    let mut key: Vec<&str> = lines.iter().copied().filter(|l| ERROR_LINE.is_match(l)).collect();
    if key.is_empty() {
        key = lines.iter().rev().take(3).rev().copied().collect();
    }
    key
}

/// A short search query from a failed command: its program and the key lines of the error text.
pub fn error_query(command: &str, error: &str) -> String {
    let mut out = program(command).unwrap_or_default();
    let mut count = tokens(&out).len();
    for l in key_lines(error) {
        for word in l.split_whitespace() {
            let n = tokens(word).len();
            if count + n > MAX_QUERY_TERMS {
                return out;
            }
            out.push(' ');
            out.push_str(word);
            count += n;
        }
    }
    out
}

/// The best share, over the key error lines, of a line's distinct terms that appear in `text`.
/// Plain numbers and hex addresses do not count; they never name the problem.
pub fn coverage(error: &str, text: &str) -> f64 {
    let t: HashSet<String> = tokens(text).into_iter().collect();
    key_lines(error).into_iter().map(|l| line_coverage(l, &t)).fold(0.0, f64::max)
}

fn line_coverage(line: &str, text: &HashSet<String>) -> f64 {
    let q: HashSet<String> = tokens(line).into_iter().filter(|w| !w.starts_with("0x") && !w.chars().all(|c| c.is_ascii_digit())).collect();
    if q.is_empty() {
        return 0.0;
    }
    q.iter().filter(|w| text.contains(*w)).count() as f64 / q.len() as f64
}

static CORRECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(no,\s*(use|it)\b|actually\b|that'?s wrong|that is wrong|don'?t\b|do not\b|stop doing\b|instead\b)").unwrap()
});
static REMEMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(remember|note that|keep in mind|don'?t forget)\b").unwrap());

/// Whether a prompt corrects the agent, and whether it asks to remember something.
/// A correction phrase counts only at the start of a sentence, and questions never count,
/// so "i don't see the line" and "do you remember?" are not signals.
pub fn prompt_signals(prompt: &str) -> (bool, bool) {
    let mut out = (false, false);
    let mut rest = prompt;
    while !rest.is_empty() {
        let end = rest.find(['.', '!', '?', '\n']).map_or(rest.len(), |i| i + 1);
        let (sentence, next) = rest.split_at(end);
        rest = next;
        if sentence.ends_with('?') {
            continue;
        }
        let sentence = sentence.trim();
        out.0 |= CORRECTION.is_match(sentence);
        out.1 |= REMEMBER.is_match(sentence);
    }
    out
}

static CONFIRM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(^|[\s;&|('"/])rkb\s+confirm\b"#).unwrap());
static REQUEST_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(r-[0-9a-f]{6})\b").unwrap());
static CHOICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"--choice(?:=|\s+)(?:"((?:[^"\\]|\\.)*)"|'([^']*)'|(\S+))"#).unwrap());

/// For a command that runs `rkb confirm`, the request id and choice when they can be read.
pub fn confirm_call(command: &str) -> Option<(Option<String>, Option<String>)> {
    if !CONFIRM.is_match(command) {
        return None;
    }
    let id = REQUEST_ID.captures(command).map(|c| c[1].to_string());
    let choice = CHOICE.captures(command).and_then(|c| c.get(1).or(c.get(2)).or(c.get(3)).map(|m| m.as_str().replace("\\\"", "\"")));
    Some((id, choice))
}

/// Whether a failed-command hook adds its top result: a model's relevance decides when a model
/// ranked it, and the coverage rule decides for BM25.
pub fn strong_enough(relevance: Option<f32>, coverage: impl FnOnce() -> f64, min_relevance: f64, min_coverage: f64) -> bool {
    match relevance {
        Some(r) => f64::from(r) >= min_relevance,
        None => coverage() >= min_coverage,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_files() {
        let dir = tempfile::tempdir().unwrap();
        let s = dir.path();
        assert!(session_path(s, "../../etc/passwd").ends_with("sessions/etcpasswd.jsonl"));
        assert!(session_path(s, "").ends_with("sessions/unknown.jsonl"));
        append(s, "abc", &serde_json::json!({"kind": "failed", "program": "cmake"})).unwrap();
        append(s, "abc", &serde_json::json!({"kind": "fixed", "program": "cmake"})).unwrap();
        assert_eq!(read(s, "abc").len(), 2);
        let mode = std::fs::metadata(session_path(s, "abc")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        append(s, "old", &serde_json::json!({"kind": "remember"})).unwrap();
        let old = std::fs::File::options().append(true).open(session_path(s, "old")).unwrap();
        old.set_modified(SystemTime::now() - Duration::from_secs((SESSION_DAYS + 1) * 24 * 3600)).unwrap();
        cleanup(s);
        assert!(session_path(s, "abc").exists());
        assert!(!session_path(s, "old").exists());
    }

    #[test]
    fn programs() {
        assert_eq!(program("cmake -B build").as_deref(), Some("cmake"));
        assert_eq!(program("/usr/bin/cmake ..").as_deref(), Some("cmake"));
        assert_eq!(program("CC=gcc make -j8").as_deref(), Some("make"));
        assert_eq!(program("   ").as_deref(), None);
    }

    #[test]
    fn query_from_error() {
        let q = error_query(
            "c++ main.o -o app",
            "Exit code 1\n/usr/bin/ld: main.o: in function `main':\nmain.cpp:(.text+0x1f): undefined reference to `vtable for Widget'\ncollect2: error: ld returned 1 exit status\n",
        );
        assert!(q.starts_with("c++ ") && q.contains("undefined reference") && q.contains("collect2"), "{q}");
        assert!(!q.contains("in function"), "{q}");
        let q = error_query("make", "Exit code 2\nline one\nline two\nline three\nline four\n");
        assert_eq!(q, "make line two line three line four");
        let long = format!("Exit code 1\nerror: {}", "word ".repeat(100));
        assert!(tokens(&error_query("x", &long)).len() <= MAX_QUERY_TERMS);
    }

    #[test]
    fn coverage_share() {
        let lesson = "Undefined reference to vtable: the link fails with undefined reference to `vtable for Widget'\n```cpp\n";
        let error = "Exit code 1\n/usr/bin/ld: main.o: in function `main':\nmain.cpp:(.text+0x1f): undefined reference to `vtable for Widget'\ncollect2: error: ld returned 1 exit status\n";
        let c = coverage(error, lesson);
        // Terms of the best line: main.cpp main cpp text undefined reference vtable widget.
        assert!((c - 5.0 / 8.0).abs() < 1e-9, "{c}");
        assert!((coverage("error: undefined zzz", "undefined error") - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(coverage("", "x"), 0.0);
    }

    #[test]
    fn prompts() {
        assert_eq!(prompt_signals("actually, use the release build and remember this"), (true, true));
        assert_eq!(prompt_signals("No, use cmake instead"), (true, false));
        assert_eq!(prompt_signals("please build the project"), (false, false));
        assert_eq!(prompt_signals("Don't forget the flags"), (true, true));
        assert_eq!(prompt_signals("i don't see any rkb: ... context line"), (false, false));
        assert_eq!(prompt_signals("do you remember the flag? why don't we use ninja?"), (false, false));
        assert_eq!(prompt_signals("ok. don't use make here"), (true, false));
        assert_eq!(prompt_signals("looks good\nactually wait, that's wrong"), (true, false));
    }

    #[test]
    fn confirm_calls() {
        assert_eq!(
            confirm_call(r#"sh -c 'rkb confirm r-4f2a9c --choice "create general/cmak"'"#),
            Some((Some("r-4f2a9c".into()), Some("create general/cmak".into())))
        );
        assert_eq!(
            confirm_call("/home/me/.cargo/bin/rkb confirm r-000001 --choice=install"),
            Some((Some("r-000001".into()), Some("install".into())))
        );
        assert_eq!(confirm_call("rkb confirm"), Some((None, None)));
        assert_eq!(confirm_call("ls -la"), None);
        assert_eq!(confirm_call("echo rkbconfirm"), None);
        assert_eq!(
            confirm_call("rkb show x && rkb  confirm r-abcdef --choice cancel"),
            Some((Some("r-abcdef".into()), Some("cancel".into())))
        );
    }

    #[test]
    fn model_relevance_overrides_coverage() {
        assert!(!strong_enough(Some(0.62), || 1.0, 0.8, 0.6));
        assert!(strong_enough(Some(0.91), || 0.0, 0.8, 0.6));
        assert!(strong_enough(None, || 0.7, 0.8, 0.6));
        assert!(!strong_enough(None, || 0.5, 0.8, 0.6));
    }
}
