use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::body;
use crate::lesson::Lesson;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Check,
    Probe,
}

impl Kind {
    pub fn heading(self) -> &'static str {
        match self {
            Kind::Check => "Check",
            Kind::Probe => "Probe",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Check => "check",
            Kind::Probe => "probe",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        [Kind::Check, Kind::Probe].into_iter().find(|k| k.name() == name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Result {
    Pass,
    Fail,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    pub result: Result,
    /// The exit code, when the script exited on its own.
    pub code: Option<i32>,
}

/// The text of the one fenced `sh` or `bash` block in the lesson's `Check` or `Probe` section, if it has exactly one.
pub fn of(lesson: &Lesson, kind: Kind) -> Option<String> {
    let section = body::section_text(&lesson.body, kind.heading())?;
    let mut blocks: Vec<(String, Vec<&str>)> = vec![];
    let mut open: Option<(String, usize)> = None;
    for line in section.lines() {
        let trimmed = line.trim_start();
        let ticks = trimmed.chars().take_while(|c| *c == '`').count();
        match &mut open {
            None if ticks >= 3 => {
                let lang = trimmed[ticks..].split_whitespace().next().unwrap_or("").to_string();
                blocks.push((lang, vec![]));
                open = Some((String::new(), ticks));
            }
            Some((_, n)) if ticks >= *n && trimmed[ticks..].trim().is_empty() => open = None,
            Some(_) => blocks.last_mut().unwrap().1.push(line),
            None => {}
        }
    }
    match blocks.as_slice() {
        [(lang, lines)] if (lang == "sh" || lang == "bash") && open.is_none() => Some(format!("{}\n", lines.join("\n"))),
        _ => None,
    }
}

/// Lowercase hex SHA-256 of the script text; the key of an approval.
pub fn hash(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Runs the script with `bash -l` in a new temporary folder and maps its exit: 0 pass, 1 fail, anything else unknown.
/// On a timeout the whole process group is killed.
pub fn run(text: &str, project_root: Option<&Path>, timeout: Duration) -> Run {
    let unknown = Run { result: Result::Unknown, code: None };
    let mut id = [0u8; 6];
    if getrandom::fill(&mut id).is_err() {
        return unknown;
    }
    let dir = std::env::temp_dir().join(format!("rkb-{}", id.iter().map(|b| format!("{b:02x}")).collect::<String>()));
    if std::fs::create_dir(&dir).is_err() {
        return unknown;
    }
    let out = run_in(&dir, text, project_root, timeout);
    let _ = std::fs::remove_dir_all(&dir);
    out.unwrap_or(unknown)
}

fn run_in(dir: &Path, text: &str, project_root: Option<&Path>, timeout: Duration) -> Option<Run> {
    use std::os::unix::process::CommandExt;
    let file = dir.join("script.sh");
    // Options go in the file, not on the command line, so `-e` never applies to the login profile.
    std::fs::File::create(&file).ok()?.write_all(format!("set -eo pipefail\n{text}").as_bytes()).ok()?;
    let mut cmd = Command::new("bash");
    cmd.arg("-l").arg(&file).current_dir(dir).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
    match project_root {
        Some(p) => cmd.env("PROJECT_ROOT", p),
        None => cmd.env_remove("PROJECT_ROOT"),
    };
    let mut child = cmd.spawn().ok()?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().ok()? {
            let code = status.code();
            let result = match code {
                Some(0) => Result::Pass,
                Some(1) => Result::Fail,
                _ => Result::Unknown,
            };
            return Some(Run { result, code });
        }
        if start.elapsed() >= timeout {
            let _ = Command::new("kill").arg("-KILL").arg(format!("-{}", child.id())).stderr(Stdio::null()).status();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lesson(body: &str) -> Lesson {
        let text =
            format!("---\nschema: 1\nid: 0a1b2c3d4e\ntype: fact\nstatus: active\nverified: 2026-09-25\nverified_how: ran\n---\n{body}");
        crate::lesson::parse("general/x/a.md", &text).unwrap()
    }

    #[test]
    fn extracts_one_shell_block() {
        let l = lesson("\n# T\n\n## Check\nRun this:\n```bash\nh5cc -showconfig\ntest -f x\n```\n\n## Evidence\ny\n");
        assert_eq!(of(&l, Kind::Check).as_deref(), Some("h5cc -showconfig\ntest -f x\n"));
        assert_eq!(of(&l, Kind::Probe), None);
        let two = lesson("\n# T\n\n## Check\n```sh\na\n```\n```sh\nb\n```\n");
        assert_eq!(of(&two, Kind::Check), None, "two blocks");
        let py = lesson("\n# T\n\n## Check\n```python\nprint(1)\n```\n");
        assert_eq!(of(&py, Kind::Check), None, "not a shell block");
        assert_eq!(hash("x\n"), hash("x\n"));
        assert_ne!(hash("x\n"), hash("y\n"));
    }

    #[test]
    fn exit_codes_and_project_root() {
        let t = Duration::from_secs(20);
        assert_eq!(run("exit 0\n", None, t), Run { result: Result::Pass, code: Some(0) });
        assert_eq!(run("exit 1\n", None, t), Run { result: Result::Fail, code: Some(1) });
        assert_eq!(run("exit 2\n", None, t), Run { result: Result::Unknown, code: Some(2) });
        assert_eq!(run("false | true\n", None, t).result, Result::Fail, "pipefail is on");
        assert_eq!(run("test \"$PROJECT_ROOT\" = /somewhere\n", Some(Path::new("/somewhere")), t).result, Result::Pass);
        assert_eq!(run("test -f script.sh\n", None, t).result, Result::Pass, "runs in its own folder");
    }

    #[test]
    fn timeout_kills_the_group() {
        let marker = format!("rkb-timeout-{}", std::process::id());
        let started = Instant::now();
        let r = run(&format!("sleep 30 # {marker}\n"), None, Duration::from_millis(500));
        assert_eq!(r.result, Result::Unknown);
        assert!(started.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(200));
        let ps = Command::new("pgrep").arg("-f").arg(&marker).output().unwrap();
        assert!(ps.stdout.is_empty(), "no process left: {}", String::from_utf8_lossy(&ps.stdout));
    }
}
