use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result, io};

/// A held lock. Dropping it removes the lock file.
#[derive(Debug)]
pub struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub fn hostname() -> &'static str {
    static HOST: OnceLock<String> = OnceLock::new();
    HOST.get_or_init(|| {
        Command::new("hostname")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "unknown-host".into())
    })
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn alive(pid: u32) -> bool {
    Command::new("kill").args(["-0", &pid.to_string()]).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// A lock is stale when its process on this host is gone or it is older than `stale_s`.
/// An unreadable lock (a writer died between create and write) is judged by its file age.
fn is_stale(path: &Path, stale_s: u64) -> bool {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let parts: Vec<&str> = text.split_whitespace().collect();
    match parts.as_slice() {
        [host, pid, time] => {
            let (Ok(pid), Ok(time)) = (pid.parse::<u32>(), time.parse::<u64>()) else { return false };
            (*host == hostname() && !alive(pid)) || now().saturating_sub(time) > stale_s
        }
        _ => std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age.as_secs() > stale_s),
    }
}

/// Takes the lock at `path`, created with `O_EXCL` so it also works on NFS.
/// Waits up to `wait`, then fails with `Locked` naming the holder.
pub fn acquire(path: &Path, stale_s: u64, wait: Duration) -> Result<LockGuard> {
    let deadline = Instant::now() + wait;
    loop {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut f) => {
                write!(f, "{} {} {}", hostname(), std::process::id(), now()).map_err(io(path))?;
                return Ok(LockGuard { path: path.to_path_buf() });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io(path)(e)),
        }
        if is_stale(path, stale_s) {
            // Rename first, so two waiters cannot both delete a lock that a third just created.
            let aside = path.with_extension(format!("stale-{}-{}", std::process::id(), now()));
            if std::fs::rename(path, &aside).is_ok() {
                let _ = std::fs::remove_file(aside);
            }
            continue;
        }
        if Instant::now() >= deadline {
            let holder = std::fs::read_to_string(path).unwrap_or_default();
            return Err(Error::Locked(format!("{} ({})", holder.trim(), path.display())));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Appends one line to a log under a short lock, because `O_APPEND` is not atomic on NFS.
pub fn append_line(path: &Path, line: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io(dir))?;
    }
    let _guard = acquire(&path.with_extension("lock"), 60, Duration::from_secs(5))?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(io(path))?;
    writeln!(f, "{line}").map_err(io(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_then_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("l.lock");
        let g = acquire(&p, 600, Duration::from_secs(1)).unwrap();
        let p2 = p.clone();
        let t = std::thread::spawn(move || acquire(&p2, 600, Duration::from_secs(5)).map(|_| ()));
        std::thread::sleep(Duration::from_millis(300));
        drop(g);
        t.join().unwrap().unwrap();
        assert!(!p.exists());
    }

    #[test]
    fn locked_error_names_holder() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("l.lock");
        let _g = acquire(&p, 600, Duration::from_secs(1)).unwrap();
        let e = acquire(&p, 600, Duration::from_millis(200)).unwrap_err();
        assert!(matches!(&e, Error::Locked(h) if h.contains(hostname())), "{e}");
    }

    #[test]
    fn dead_holder_and_old_foreign_holder_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("l.lock");
        std::fs::write(&p, format!("{} 999999 {}", hostname(), now())).unwrap();
        drop(acquire(&p, 600, Duration::from_millis(200)).unwrap());
        std::fs::write(&p, "otherhost 1 0").unwrap();
        drop(acquire(&p, 600, Duration::from_millis(200)).unwrap());
        std::fs::write(&p, format!("otherhost 1 {}", now())).unwrap();
        assert!(acquire(&p, 600, Duration::from_millis(200)).is_err());
    }

    #[test]
    fn parallel_appends_keep_lines_whole() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("usage.jsonl");
        let threads: Vec<_> = (0..20)
            .map(|i| {
                let p = p.clone();
                std::thread::spawn(move || append_line(&p, &format!("{{\"n\":{i},\"pad\":\"{}\"}}", "x".repeat(500))))
            })
            .collect();
        for t in threads {
            t.join().unwrap().unwrap();
        }
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text.lines().count(), 20);
        for l in text.lines() {
            serde_json::from_str::<serde_json::Value>(l).unwrap();
        }
    }
}
