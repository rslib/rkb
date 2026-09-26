use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{Error, Result, io};

fn command(dir: &Path) -> Command {
    let mut c = Command::new("git");
    // An agent has no terminal, so a remote that needs a password must fail, not wait.
    c.arg("-C").arg(dir).env("GIT_TERMINAL_PROMPT", "0");
    c
}

/// Runs git in `dir` and returns stdout. A non-zero exit is an error with git's stderr.
pub fn run(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = command(dir).args(args).output().map_err(io("git"))?;
    if !out.status.success() {
        return Err(Error::Git { args: args.join(" "), stderr: String::from_utf8_lossy(&out.stderr).trim().to_string() });
    }
    Ok(out.stdout)
}

pub fn has_head(dir: &Path) -> bool {
    command(dir).args(["rev-parse", "--verify", "-q", "HEAD"]).stdout(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// Fails with `GitIdentity` unless user.name and user.email come from git config.
pub fn check_identity(dir: &Path) -> Result<()> {
    for var in ["GIT_AUTHOR_IDENT", "GIT_COMMITTER_IDENT"] {
        let ok = command(dir)
            .args(["-c", "user.useConfigOnly=true", "var", var])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(io("git"))?
            .success();
        if !ok {
            return Err(Error::GitIdentity);
        }
    }
    Ok(())
}

/// Commits the index. rkb lints in process before it commits, so the hook is skipped.
pub fn commit(dir: &Path, message: &str) -> Result<()> {
    run(dir, &["-c", "user.useConfigOnly=true", "commit", "--no-verify", "-q", "-m", message]).map(|_| ())
}

/// Commits only `paths`, even when other changes are staged.
pub fn commit_paths(dir: &Path, message: &str, paths: &[&str]) -> Result<()> {
    let mut args = vec!["-c", "user.useConfigOnly=true", "commit", "--no-verify", "-q", "-m", message, "--"];
    args.extend_from_slice(paths);
    run(dir, &args).map(|_| ())
}

/// Reads objects such as `:path` or `HEAD:path` in one `git cat-file --batch` call.
/// A missing object gives `None`.
pub fn read_blobs(dir: &Path, specs: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
    let mut child = command(dir).args(["cat-file", "--batch"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().map_err(io("git"))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let input: String = specs.iter().map(|s| format!("{s}\n")).collect();
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));

    let mut reader = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut out = Vec::with_capacity(specs.len());
    for _ in specs {
        let mut header = String::new();
        reader.read_line(&mut header).map_err(io("git cat-file"))?;
        if header.trim_end().ends_with(" missing") || header.trim_end().ends_with(" ambiguous") {
            out.push(None);
            continue;
        }
        let size: usize = header
            .split_whitespace()
            .nth(2)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Error::Git { args: "cat-file --batch".into(), stderr: format!("bad header: {header}") })?;
        let mut data = vec![0; size + 1];
        reader.read_exact(&mut data).map_err(io("git cat-file"))?;
        data.pop();
        out.push(Some(data));
    }
    writer.join().expect("writer thread").map_err(io("git cat-file"))?;
    child.wait().map_err(io("git cat-file"))?;
    Ok(out)
}
