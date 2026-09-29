//! The inbox of raw material for lessons: notes, and extracts of sessions that had a signal.
//! Items live in the per-machine state folder and are never searched, synced or committed.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result, io};

/// Items older than this are deleted without being distilled.
pub const EXPIRY_DAYS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Note,
    Transcript,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Note => "note",
            Kind::Transcript => "transcript",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub kind: Kind,
    /// Seconds since the Unix epoch.
    pub time: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The signals that caused a transcript extract, such as `fixed cmake`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<String>,
    /// 1 to 3, highest first in the inbox; an item without one counts as 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    /// Where an imported note came from, such as a Claude Code memory file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl Meta {
    pub fn priority(&self) -> u8 {
        self.priority.unwrap_or(1).clamp(1, 3)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub meta: Meta,
    pub body: String,
}

impl Item {
    pub fn text(&self) -> String {
        let fm = serde_norway::to_string(&self.meta).expect("inbox metadata serializes");
        format!("---\n{fm}---\n\n{}\n", self.body.trim_end())
    }

    pub fn preview(&self) -> String {
        self.body.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).unwrap_or("").to_string()
    }
}

pub fn dir(state: &Path) -> PathBuf {
    state.join("inbox")
}

fn file(state: &Path, id: &str) -> PathBuf {
    dir(state).join(format!("{id}.md"))
}

fn valid_id(id: &str) -> bool {
    id.len() == 10 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn parse(id: &str, text: &str) -> Option<Item> {
    let rest = text.strip_prefix("---\n")?;
    let (fm, body) = rest.split_once("\n---\n")?;
    let meta: Meta = serde_norway::from_str(fm).ok()?;
    Some(Item { id: id.to_string(), meta, body: body.trim().to_string() })
}

/// Adds an item. It is written to a temporary file and renamed, so a reader never sees half of it.
pub fn add(state: &Path, meta: Meta, body: &str) -> Result<Item> {
    let d = dir(state);
    std::fs::create_dir_all(&d).map_err(io(&d))?;
    let id = loop {
        let mut b = [0u8; 5];
        getrandom::fill(&mut b).expect("system random source");
        let id: String = b.iter().map(|x| format!("{x:02x}")).collect();
        if !file(state, &id).exists() {
            break id;
        }
    };
    let item = Item { id: id.clone(), meta, body: body.trim().to_string() };
    let tmp = d.join(format!(".{id}.tmp"));
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(io(&tmp))?;
    f.write_all(item.text().as_bytes()).map_err(io(&tmp))?;
    f.sync_all().map_err(io(&tmp))?;
    std::fs::rename(&tmp, file(state, &id)).map_err(io(&tmp))?;
    Ok(item)
}

/// Every item, oldest first. Files that do not parse are skipped.
pub fn list(state: &Path) -> Vec<Item> {
    let Ok(entries) = std::fs::read_dir(dir(state)) else { return vec![] };
    let mut items: Vec<Item> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let id = name.strip_suffix(".md").filter(|id| valid_id(id))?.to_string();
            parse(&id, &std::fs::read_to_string(e.path()).ok()?)
        })
        .collect();
    items.sort_by(|a, b| b.meta.priority().cmp(&a.meta.priority()).then(a.meta.time.cmp(&b.meta.time)).then(a.id.cmp(&b.id)));
    items
}

pub fn get(state: &Path, id: &str) -> Result<Item> {
    let missing = || Error::NotFound(format!("inbox item {id}"));
    if !valid_id(id) {
        return Err(missing());
    }
    let text = std::fs::read_to_string(file(state, id)).map_err(|_| missing())?;
    parse(id, &text).ok_or_else(missing)
}

/// Removes an item; `false` when there was none with that id.
pub fn remove(state: &Path, id: &str) -> bool {
    valid_id(id) && std::fs::remove_file(file(state, id)).is_ok()
}

/// Deletes items older than `EXPIRY_DAYS` at `now` and returns how many.
pub fn expire(state: &Path, now: u64) -> usize {
    let limit = now.saturating_sub(EXPIRY_DAYS * 24 * 3600);
    list(state).into_iter().filter(|i| i.meta.time < limit && remove(state, &i.id)).count()
}

/// Lines of one extract at most; the most recent part is kept.
pub const MAX_LINES: usize = 400;
/// Error lines kept per failed command.
const MAX_ERROR_LINES: usize = 20;

/// One thing that happened in a session, the same for every harness.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Prompt(String),
    Text(String),
    Command { command: String, ok: bool, output: String },
}

fn content_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => {
            blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n")
        }
        _ => String::new(),
    }
}

/// Steps from Claude Code transcript lines. Tool results are matched to their calls by id.
pub fn read_claude(lines: &[&str]) -> Vec<Step> {
    let mut steps = vec![];
    let mut calls: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for line in lines {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let content = &v["message"]["content"];
        match v["type"].as_str() {
            Some("user") if v["isMeta"] != true => {
                if let Some(text) = content.as_str() {
                    steps.push(Step::Prompt(text.to_string()));
                }
                for b in content.as_array().into_iter().flatten() {
                    match b["type"].as_str() {
                        Some("text") => steps.extend(b["text"].as_str().map(|t| Step::Prompt(t.to_string()))),
                        Some("tool_result") => {
                            if let Some(command) = b["tool_use_id"].as_str().and_then(|id| calls.remove(id)) {
                                steps.push(Step::Command { command, ok: b["is_error"] != true, output: content_text(&b["content"]) });
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("assistant") => {
                for b in content.as_array().into_iter().flatten() {
                    match b["type"].as_str() {
                        Some("text") => steps.extend(b["text"].as_str().map(|t| Step::Text(t.to_string()))),
                        Some("tool_use") if b["name"] == "Bash" => {
                            if let (Some(id), Some(cmd)) = (b["id"].as_str(), b["input"]["command"].as_str()) {
                                calls.insert(id.to_string(), cmd.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    steps
}

/// Steps from pi or omp session lines. A failed bash output ends with `Command exited with code N`.
pub fn read_pi(lines: &[&str]) -> Vec<Step> {
    let mut steps = vec![];
    let mut calls: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for line in lines {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v["type"] != "message" {
            continue;
        }
        let m = &v["message"];
        match m["role"].as_str() {
            Some("user") => steps.push(Step::Prompt(content_text(&m["content"]))),
            Some("assistant") => {
                for b in m["content"].as_array().into_iter().flatten() {
                    match b["type"].as_str() {
                        Some("text") => steps.extend(b["text"].as_str().map(|t| Step::Text(t.to_string()))),
                        Some("toolCall") if b["name"] == "bash" => {
                            if let (Some(id), Some(cmd)) = (b["id"].as_str(), b["arguments"]["command"].as_str()) {
                                calls.insert(id.to_string(), cmd.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("toolResult") => {
                if let Some(command) = m["toolCallId"].as_str().and_then(|id| calls.remove(id)) {
                    let text = content_text(&m["content"]);
                    let trimmed = text.trim_end();
                    let (body, code) = match trimmed.rsplit_once('\n') {
                        Some((body, last)) if last.starts_with("Command exited with code ") => {
                            (body.to_string(), last.trim_start_matches("Command exited with code ").trim().to_string())
                        }
                        _ => (trimmed.to_string(), String::new()),
                    };
                    let output = if code.is_empty() { body } else { format!("Exit code {code}\n{}", body.trim_end()) };
                    steps.push(Step::Command { command, ok: m["isError"] != true, output });
                }
            }
            _ => {}
        }
    }
    steps
}

/// The key lines of a failure: lines that name the problem and an indented line right after one.
fn error_lines(output: &str) -> Vec<String> {
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty() && !l.starts_with("Exit code ")).collect();
    let key: std::collections::HashSet<&str> = crate::hooks::key_lines(output).into_iter().collect();
    let mut out = vec![];
    for (i, l) in lines.iter().enumerate() {
        let after_key = i > 0 && key.contains(lines[i - 1].trim()) && l.starts_with([' ', '\t']);
        if key.contains(l.trim()) || after_key {
            out.push(l.trim().to_string());
        }
    }
    out.truncate(MAX_ERROR_LINES);
    out
}

fn exit_code(output: &str) -> Option<&str> {
    output.lines().next()?.strip_prefix("Exit code ").map(str::trim)
}

fn labeled(label: &str, text: &str) -> Vec<String> {
    let mut lines = text.trim().lines();
    let Some(first) = lines.next() else { return vec![] };
    std::iter::once(format!("[{label}] {first}")).chain(lines.map(|l| format!("  {l}"))).collect()
}

/// The extract of `steps`: prompts, agent text, failed commands with their key error lines, and the
/// command that then worked; each part labeled by its source. At most `MAX_LINES`, keeping the end.
pub fn extract(steps: &[Step]) -> String {
    let mut out: Vec<String> = vec![];
    let mut failed: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in steps {
        match s {
            Step::Prompt(t) => out.extend(labeled("user", t)),
            Step::Text(t) => out.extend(labeled("agent", t)),
            Step::Command { command, ok: false, output } => {
                let head = match exit_code(output) {
                    Some(code) => format!("command failed, exit {code}"),
                    None => "command failed".to_string(),
                };
                out.extend(labeled(&head, command));
                out.push("[tool output]".to_string());
                out.extend(error_lines(output).into_iter().map(|l| format!("  {l}")));
                failed.extend(crate::hooks::program(command));
            }
            Step::Command { command, ok: true, .. } => {
                if let Some(p) = crate::hooks::program(command)
                    && failed.remove(&p)
                {
                    out.extend(labeled("command worked", command));
                }
            }
        }
    }
    // Repeated identical output lines (a warning printed per file, say) collapse to one with a count.
    let mut runs: Vec<(String, usize)> = vec![];
    for line in out {
        match runs.last_mut() {
            Some((prev, n)) if line.starts_with("  ") && *prev == line => *n += 1,
            _ => runs.push((line, 1)),
        }
    }
    let mut out: Vec<String> = runs.into_iter().map(|(l, n)| if n > 1 { format!("{l} (×{n})") } else { l }).collect();
    if out.len() > MAX_LINES {
        let cut = out.len() - (MAX_LINES - 1);
        out.drain(..cut);
        out.insert(0, format!("[{cut} earlier lines cut]"));
    }
    out.join("\n")
}

/// The session-start line, only when the inbox holds at least 5 items or its oldest is over 7 days old.
pub fn nudge(state: &Path, now: u64, command: &str) -> Option<String> {
    let items = list(state);
    let oldest = items.iter().map(|i| i.meta.time).min()?;
    let days = now.saturating_sub(oldest) / 86400;
    let urgent = items.iter().filter(|i| i.meta.priority() == 3).count();
    (items.len() >= 5 || days > 7 || urgent > 0).then(|| {
        let high = if urgent > 0 { format!(" ({urgent} of high priority)") } else { String::new() };
        format!(
            "rkb inbox: {} items wait{high}, the oldest {days} days old. At a natural pause in the work, run {command} for up to 3 of them, without waiting for the user.",
            items.len()
        )
    })
}

/// Session-state signals that make a session worth an extract.
const CAPTURE_SIGNALS: [&str; 3] = ["fixed", "correction", "remember"];

/// Saves an extract of the part of `transcript` after the last extract of `session`, when the session
/// recorded a signal since then. Returns the new item, or `None` when there was no signal or nothing new.
pub fn capture(state: &Path, harness: &str, session: &str, transcript: &Path, cwd: Option<&str>) -> Result<Option<Item>> {
    use std::io::{Read, Seek, SeekFrom};
    expire(state, crate::request::now());
    let records = crate::hooks::read(state, session);
    let last = records.iter().rposition(|r| r["kind"] == "extracted");
    let since = &records[last.map_or(0, |i| i + 1)..];
    let signals: Vec<String> = since
        .iter()
        .filter(|r| CAPTURE_SIGNALS.iter().any(|k| r["kind"] == *k))
        .map(|r| match r["program"].as_str() {
            Some(p) => format!("{} {p}", r["kind"].as_str().unwrap_or("")),
            None => r["kind"].as_str().unwrap_or("").to_string(),
        })
        .collect();
    if signals.is_empty() {
        return Ok(None);
    }
    let mut f = std::fs::File::open(transcript).map_err(io(transcript))?;
    let len = f.metadata().map_err(io(transcript))?.len();
    let from = last.and_then(|i| records[i]["offset"].as_u64()).filter(|&o| o <= len).unwrap_or(0);
    f.seek(SeekFrom::Start(from)).map_err(io(transcript))?;
    let mut bytes = vec![];
    f.read_to_end(&mut bytes).map_err(io(transcript))?;
    let whole = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let text = String::from_utf8_lossy(&bytes[..whole]);
    let lines: Vec<&str> = text.lines().collect();
    let steps = if harness == "claude-code" { read_claude(&lines) } else { read_pi(&lines) };
    let body = extract(&steps);
    let item = if body.is_empty() {
        None
    } else {
        let meta = Meta {
            kind: Kind::Transcript,
            time: crate::request::now(),
            harness: Some(harness.to_string()),
            session: Some(session.to_string()),
            cwd: cwd.map(str::to_string),
            signals: signals.into_iter().collect::<std::collections::BTreeSet<_>>().into_iter().collect(),
            priority: Some(crate::hooks::score(since, &records).clamp(1, 3) as u8),
            source: None,
        };
        Some(add(state, meta, &body)?)
    };
    crate::hooks::append(state, session, &serde_json::json!({ "kind": "extracted", "offset": from + whole as u64 }))?;
    Ok(item)
}

/// Every Claude Code auto memory folder: `~/.claude/projects/*/memory/`.
pub fn claude_memory_dirs() -> Vec<std::path::PathBuf> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    let mut ds: Vec<_> = std::fs::read_dir(home.join(".claude/projects"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join("memory"))
        .filter(|p| p.is_dir())
        .collect();
    ds.sort();
    ds
}

/// Notes from Claude Code auto memory files (`<dir>/*.md` except the `MEMORY.md` index) and from files
/// such as `CLAUDE.md` (one note per `## ` section), as `(source, text)`.
pub fn claude_notes(dirs: &[std::path::PathBuf], files: &[std::path::PathBuf]) -> Vec<(String, String)> {
    let mut out = vec![];
    for d in dirs {
        let mut paths: Vec<_> = std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path()).collect();
        paths.sort();
        for p in paths.into_iter().filter(|p| p.extension().is_some_and(|x| x == "md") && p.file_name().is_some_and(|n| n != "MEMORY.md")) {
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            let (fm, body) = crate::lesson::split(&text).map(|(f, b, _)| (f, b)).unwrap_or(("", &text));
            let v: serde_norway::Value = serde_norway::from_str(fm).unwrap_or(serde_norway::Value::Null);
            let get = |k: &str| v.get(k).and_then(serde_norway::Value::as_str).map(str::to_string);
            let kind = get("type")
                .or_else(|| v.get("metadata").and_then(|m| m.get("type")).and_then(serde_norway::Value::as_str).map(str::to_string));
            let name = get("name").unwrap_or_else(|| p.file_stem().unwrap_or_default().to_string_lossy().into_owned());
            let mut note = format!("From Claude Code memory ({}): {name}", kind.as_deref().unwrap_or("note"));
            if let Some(d) = get("description") {
                note.push_str(&format!("\n{d}"));
            }
            note.push_str(&format!("\n\n{}", body.trim()));
            out.push((p.display().to_string(), note));
        }
    }
    for f in files {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let mut sections: Vec<String> = vec![String::new()];
        for line in text.lines() {
            if line.starts_with("## ") {
                sections.push(String::new());
            }
            let cur = sections.last_mut().expect("never empty");
            cur.push_str(line);
            cur.push('\n');
        }
        for sec in sections.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
            let heading = sec.lines().next().unwrap_or("").trim_start_matches('#').trim().to_string();
            out.push((format!("{}#{heading}", f.display()), format!("From {}: {heading}\n\n{sec}", f.display())));
        }
    }
    out
}

/// Saves each `(source, text)` as a priority-2 inbox note unless the same text was imported before.
/// Returns how many were saved and how many skipped.
pub fn import_notes(state: &Path, notes: &[(String, String)]) -> Result<(usize, usize)> {
    use sha2::Digest;
    let log = state.join("imported.jsonl");
    let seen: std::collections::HashSet<String> = std::fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v["hash"].as_str().map(String::from))
        .collect();
    let (mut saved, mut skipped) = (0, 0);
    for (source, text) in notes {
        let hash: String = sha2::Sha256::digest(text.as_bytes()).iter().take(12).map(|b| format!("{b:02x}")).collect();
        if seen.contains(&hash) {
            skipped += 1;
            continue;
        }
        let meta = Meta {
            kind: Kind::Note,
            time: crate::request::now(),
            harness: None,
            session: None,
            cwd: None,
            signals: vec![],
            priority: Some(2),
            source: Some(source.clone()),
        };
        add(state, meta, text)?;
        crate::lock::append_line(&log, &serde_json::json!({ "hash": hash, "source": source }).to_string())?;
        saved += 1;
    }
    Ok((saved, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_order_nudge_and_collapse() {
        let dir = tempfile::tempdir().unwrap();
        let old = add(dir.path(), note(100), "old low").unwrap();
        let high = add(dir.path(), Meta { priority: Some(3), ..note(200) }, "new high").unwrap();
        let ids: Vec<String> = list(dir.path()).into_iter().map(|i| i.id).collect();
        assert_eq!(ids, [high.id.clone(), old.id.clone()], "priority first, then oldest");
        let line = nudge(dir.path(), 300, "/rkb:distill").expect("a high-priority item is enough");
        assert!(line.contains("1 of high priority") && line.contains("without waiting for the user"), "{line}");

        let steps = vec![Step::Command {
            command: "cc a.c".into(),
            ok: false,
            output: "Exit code 1\nerror: x undefined\nerror: x undefined\nerror: x undefined\nerror: failed".into(),
        }];
        let text = extract(&steps);
        assert!(text.contains("error: x undefined (×3)") && text.matches("x undefined").count() == 1, "{text}");
    }

    fn note(time: u64) -> Meta {
        Meta { kind: Kind::Note, time, harness: None, session: None, cwd: None, signals: vec![], priority: None, source: None }
    }

    #[test]
    fn add_list_remove_and_expire() {
        let dir = tempfile::tempdir().unwrap();
        let s = dir.path();
        let b = add(s, note(200), "second").unwrap();
        let a = add(s, note(100), "first line\nmore").unwrap();
        let ids: Vec<String> = list(s).into_iter().map(|i| i.id).collect();
        assert_eq!(ids, [a.id.clone(), b.id.clone()], "oldest first");
        assert_eq!(get(s, &a.id).unwrap(), a);
        assert_eq!(a.preview(), "first line");
        assert!(!std::fs::read_dir(super::dir(s)).unwrap().flatten().any(|e| e.file_name().to_string_lossy().ends_with(".tmp")));

        assert!(remove(s, &a.id));
        assert!(!remove(s, &a.id));
        assert!(get(s, &a.id).is_err());
        assert!(get(s, "../../etc").is_err());

        let day = 24 * 3600;
        add(s, note(1_000 * day), "old").unwrap();
        let fresh = add(s, note(1_029 * day), "fresh").unwrap();
        assert_eq!(expire(s, 1_031 * day), 2, "the old item and the item at 200 s");
        assert_eq!(list(s).into_iter().map(|i| i.id).collect::<Vec<_>>(), [fresh.id]);
    }

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/transcripts").join(name)).unwrap()
    }

    #[test]
    fn both_formats_give_the_same_extract() {
        let claude = fixture("claude.jsonl");
        let pi = fixture("pi.jsonl");
        let a = extract(&read_claude(&claude.lines().collect::<Vec<_>>()));
        let b = extract(&read_pi(&pi.lines().collect::<Vec<_>>()));
        assert_eq!(a, b);
        assert_eq!(
            a,
            "[user] The configure step fails, can you fix it?
[command failed, exit 1] cmake -B build
[tool output]
  CMake Error at CMakeLists.txt:5 (find_package):
  Could NOT find ZLIB (missing: ZLIB_LIBRARY ZLIB_INCLUDE_DIR)
  -- Configuring incomplete, errors occurred!
[agent] ZLIB is installed under /opt/zlib. Point CMake at it with ZLIB_ROOT.
[command worked] cmake -B build -DZLIB_ROOT=/opt/zlib
[agent] Configured. ZLIB_ROOT tells find_package where to look."
        );
        assert!(!a.contains("must not appear"));
    }

    #[test]
    fn nudge_on_count_or_age() {
        let dir = tempfile::tempdir().unwrap();
        let s = dir.path();
        let day = 86400;
        assert_eq!(nudge(s, 100 * day, "/rkb-distill"), None);
        add(s, note(90 * day), "old").unwrap();
        assert!(nudge(s, 97 * day, "/rkb-distill").is_none(), "7 days is not yet old");
        assert!(nudge(s, 98 * day, "/rkb-distill").unwrap().contains("the oldest 8 days old"));
    }

    #[test]
    fn long_extract_keeps_the_end() {
        let steps: Vec<Step> = (0..500).map(|i| Step::Text(format!("line {i}"))).collect();
        let e = extract(&steps);
        let lines: Vec<&str> = e.lines().collect();
        assert_eq!(lines.len(), MAX_LINES);
        assert_eq!(lines[0], "[101 earlier lines cut]");
        assert_eq!(*lines.last().unwrap(), "[agent] line 499");
    }

    #[test]
    fn capture_needs_a_signal_and_takes_only_new_entries() {
        let dir = tempfile::tempdir().unwrap();
        let s = dir.path();
        let t = s.join("t.jsonl");
        let text = fixture("claude.jsonl");
        let (first, rest) = text.split_at(
            text.find("{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Configured")
                .unwrap(),
        );
        std::fs::write(&t, first).unwrap();
        assert_eq!(capture(s, "claude-code", "x", &t, None).unwrap(), None, "no signal, no item");

        crate::hooks::append(s, "x", &serde_json::json!({ "kind": "fixed", "program": "cmake" })).unwrap();
        let item = capture(s, "claude-code", "x", &t, Some("/work/demo")).unwrap().unwrap();
        assert_eq!(item.meta.signals, ["fixed cmake"]);
        assert!(item.body.contains("Could NOT find ZLIB") && !item.body.contains("Configured."));
        assert_eq!(capture(s, "claude-code", "x", &t, None).unwrap(), None, "nothing new");

        std::fs::write(&t, format!("{first}{rest}")).unwrap();
        crate::hooks::append(s, "x", &serde_json::json!({ "kind": "correction" })).unwrap();
        let later = capture(s, "claude-code", "x", &t, None).unwrap().unwrap();
        assert_eq!(later.body, "[agent] Configured. ZLIB_ROOT tells find_package where to look.");
        assert_eq!(list(s).len(), 2);
    }
}
