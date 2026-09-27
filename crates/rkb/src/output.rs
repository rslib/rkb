use std::io::{IsTerminal, Write};

use clap::ValueEnum;
use rkb_core::Error;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Toon,
    Json,
    Human,
}

/// The format for this call: the flag, then `RKB_FORMAT`, then human. Agents pass `--toon`.
pub fn resolve(flag: Option<Format>) -> Result<Format, CliError> {
    if let Some(f) = flag {
        return Ok(f);
    }
    match std::env::var("RKB_FORMAT") {
        Ok(v) => Format::from_str(&v, true).map_err(|_| CliError {
            code: ErrorCode::BadFormat,
            message: format!("RKB_FORMAT={v} is not a format"),
            fix: "set RKB_FORMAT to toon, json or human, or unset it".into(),
            extra: None,
        }),
        Err(_) => Ok(Format::Human),
    }
}

/// `path` with the home folder shown as `~`.
pub fn tilde(path: &std::path::Path) -> String {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    match path.strip_prefix(&home) {
        Ok(rest) if !home.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

pub fn color() -> bool {
    color_for(std::io::stdout().is_terminal(), std::env::var_os("NO_COLOR").is_some())
}

fn color_for(terminal: bool, no_color: bool) -> bool {
    terminal && !no_color
}

pub fn paint(on: bool, code: &str, text: &str) -> String {
    if on { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_string() }
}

/// A command result: `data` for agents, `human` for people, and the exit code.
pub struct Output {
    pub data: Value,
    pub human: String,
    /// 0 success, 1 findings, 3 needs_user.
    pub exit: u8,
    /// Print `human` as is in TOON too, for text meant to be saved as a file.
    pub raw: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    NotAKb,
    Exists,
    GitIdentity,
    Git,
    Io,
    NotFound,
    BadFormat,
    Refused,
    InvalidLesson,
    Locked,
    Conflict,
    Expired,
    BadChoice,
    NeedsTerminal,
    Usage,
}

impl ErrorCode {
    fn as_str(self) -> &'static str {
        match self {
            ErrorCode::NotAKb => "not_a_kb",
            ErrorCode::Exists => "exists",
            ErrorCode::GitIdentity => "git_identity",
            ErrorCode::Git => "git",
            ErrorCode::Io => "io",
            ErrorCode::NotFound => "not_found",
            ErrorCode::BadFormat => "bad_format",
            ErrorCode::Refused => "refused",
            ErrorCode::InvalidLesson => "invalid_lesson",
            ErrorCode::Locked => "locked",
            ErrorCode::Conflict => "conflict",
            ErrorCode::Expired => "expired",
            ErrorCode::BadChoice => "bad_choice",
            ErrorCode::NeedsTerminal => "needs_terminal",
            ErrorCode::Usage => "usage",
        }
    }
}

#[derive(Debug)]
pub struct CliError {
    pub code: ErrorCode,
    pub message: String,
    pub fix: String,
    /// Extra fields for the error record, such as lint findings or valid options.
    pub extra: Option<Box<Value>>,
}

impl CliError {
    pub fn new(code: ErrorCode, message: impl Into<String>, fix: impl Into<String>) -> Self {
        CliError { code, message: message.into(), fix: fix.into(), extra: None }
    }
}

impl From<Error> for CliError {
    fn from(e: Error) -> Self {
        let mut extra = None;
        let (code, fix) = match &e {
            Error::NotAKb(_) => (ErrorCode::NotAKb, "run `rkb init` to create it, or set RKB_HOME to an existing knowledge base".into()),
            Error::Exists(_) => (ErrorCode::Exists, "set RKB_HOME to a new or empty directory".into()),
            Error::GitIdentity => (
                ErrorCode::GitIdentity,
                "run `git config --global user.name \"Your Name\"` and `git config --global user.email you@example.org`".into(),
            ),
            Error::Git { .. } => (ErrorCode::Git, "fix the git problem in the message, then run the command again".into()),
            Error::Io { .. } => (ErrorCode::Io, "check that the path exists and that you can read and write it".into()),
            Error::NotFound(_) => (ErrorCode::NotFound, "check the id; it is the `id:` field in the lesson's frontmatter".into()),
            Error::NoFolder(_) => (ErrorCode::NotFound, "run `rkb list` to see the folders".into()),
            Error::BadPattern(_) => (ErrorCode::Usage, "fix the regular expression, or use --literal for plain text".into()),
            Error::Refused(_) => (ErrorCode::Refused, "change the input as the message says, then run the command again".into()),
            Error::Invalid(findings) => {
                extra = Some(Box::new(json!({ "findings": findings })));
                (ErrorCode::InvalidLesson, "fix each finding in the lesson, then run the command again".into())
            }
            Error::Locked(_) => (ErrorCode::Locked, "wait for the other rkb command to finish, then run this one again".into()),
            Error::Conflict(hash) => (
                ErrorCode::Conflict,
                format!("run `rkb show <id>` to read the current lesson, merge your change, then edit with `--base {hash}`"),
            ),
            Error::RebaseConflict { root, files } => {
                extra = Some(Box::new(json!({ "files": files })));
                let kb = root.display();
                (
                    ErrorCode::Conflict,
                    format!(
                        "resolve the conflict in each file, then `git -C {kb} add <file>` and `git -C {kb} rebase --continue`, then `rkb sync` again; or undo with `git -C {kb} rebase --abort`"
                    ),
                )
            }
            Error::NotReady { fix, .. } => (ErrorCode::Refused, fix.clone()),
            Error::NoRemote { remotes, .. } => {
                extra = Some(Box::new(json!({ "remotes": remotes })));
                let fix = if remotes.is_empty() {
                    "add one with `git -C $RKB_HOME remote add origin <url>`".to_string()
                } else {
                    format!("use one of: {}", remotes.join(", "))
                };
                (ErrorCode::NotFound, fix)
            }
            Error::NothingToImport(_) => {
                (ErrorCode::NotFound, "write each lesson as <dir>/<topic>/<name>.md, then run `rkb import <dir>`".into())
            }
            Error::Expired(_) => (ErrorCode::Expired, "run the original command again to get a new request".into()),
            Error::BadChoice { options, .. } => {
                extra = Some(Box::new(json!({ "options": options })));
                (ErrorCode::BadChoice, "run `rkb confirm` again with one of the options".into())
            }
        };
        CliError { code, message: e.to_string(), fix, extra }
    }
}

fn expand(data: &Value) -> String {
    toon_format::encode_default(data).map(|t| expand_lists(&t)).unwrap_or_else(|e| format!("error: {e}"))
}

pub fn print(format: Format, out: &Output) {
    let text = match format {
        Format::Toon if out.raw => out.human.trim_end().to_string(),
        Format::Toon => expand(&out.data),
        Format::Json => out.data.to_string(),
        Format::Human => out.human.trim_end().to_string(),
    };
    // A closed pipe, as in `| head`, is not worth reporting.
    let _ = writeln!(std::io::stdout().lock(), "{text}");
}

pub fn print_error(format: Format, e: &CliError) {
    let mut record = json!({ "error": { "code": e.code.as_str(), "message": e.message, "fix": e.fix } });
    if let Some(Value::Object(extra)) = e.extra.as_deref() {
        for (k, v) in extra {
            record["error"][k] = v.clone();
        }
    }
    match format {
        Format::Toon => println!("{}", expand(&record)),
        Format::Json => println!("{record}"),
        Format::Human => {
            let on = std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
            eprintln!("{} {}", paint(on, "31;1", "error:"), e.message);
            if let Some(Value::Object(extra)) = e.extra.as_deref() {
                for f in extra.get("findings").and_then(Value::as_array).into_iter().flatten() {
                    let line = f["line"].as_u64().map(|l| format!(":{l}")).unwrap_or_default();
                    eprintln!(
                        "  {}{line}  {}  {}",
                        f["path"].as_str().unwrap_or(""),
                        f["rule"].as_str().unwrap_or(""),
                        f["message"].as_str().unwrap_or("")
                    );
                }
                for o in extra.get("options").and_then(Value::as_array).into_iter().flatten() {
                    eprintln!("  option: {}", o.as_str().unwrap_or(""));
                }
            }
            eprintln!("{} {}", paint(on, "2", "fix:"), e.fix);
        }
    }
}

/// Cuts `text` to `max` characters and says how to get the rest.
pub fn cut(text: &str, max: usize, rest: &str) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}... [{n} chars; run `{rest}` for all]")
}

// Adapted from ai-tools/crates/rait/src/render.rs.
/// Rewrites inline lists of plain values (`key[N]: a,b`) one item per line, as AXI tools print them.
fn expand_lists(toon: &str) -> String {
    let mut out = String::with_capacity(toon.len());
    for line in toon.lines() {
        match inline_list(line) {
            Some((head, indent, items)) => {
                out.push_str(head);
                out.push_str(":\n");
                for item in items {
                    out.push_str(&" ".repeat(indent));
                    out.push_str(&item);
                    out.push('\n');
                }
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out.pop();
    out
}

fn inline_list(line: &str) -> Option<(&str, usize, Vec<String>)> {
    let body = line.trim_start();
    let mut indent = line.len() - body.len();
    let key = match body.strip_prefix("- ") {
        Some(rest) => {
            indent += 2;
            rest
        }
        None => body,
    };
    let open = key.find('[')?;
    let close = open + key[open..].find("]: ")?;
    let count = &key[open + 1..close];
    if open == 0 || key[..open].starts_with('"') || !count.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let head_len = line.len() - key.len() + close + 1;
    Some((&line[..head_len], indent + 2, split_values(&key[close + 3..])))
}

fn split_values(values: &str) -> Vec<String> {
    let mut items = vec![];
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for c in values.chars() {
        match c {
            _ if escaped => {
                current.push(c);
                escaped = false;
            }
            '\\' if quoted => {
                current.push(c);
                escaped = true;
            }
            '"' => {
                current.push(c);
                quoted = !quoted;
            }
            ',' if !quoted => items.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    items.push(current);
    items.into_iter().map(|item| unquote(&item)).collect()
}

fn unquote(item: &str) -> String {
    let Some(inner) = item.strip_prefix('"').and_then(|i| i.strip_suffix('"')) else {
        return item.to_string();
    };
    if inner.is_empty() || inner.contains("\\n") || inner.trim() != inner {
        return item.to_string();
    }
    inner.replace("\\\"", "\"").replace("\\\\", "\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_inline_lists_one_item_per_line() {
        let toon = "count: 1\nhelp[2]: Run `a`,\"Run `b`, then c\"\nrows[1]{a,b}:\n  1,2\nempty[0]:";
        assert_eq!(expand_lists(toon), "count: 1\nhelp[2]:\n  Run `a`\n  Run `b`, then c\nrows[1]{a,b}:\n  1,2\nempty[0]:");
    }

    #[test]
    fn color_only_on_terminal_without_no_color() {
        assert!(color_for(true, false));
        assert!(!color_for(true, true));
        assert!(!color_for(false, false));
        assert_eq!(paint(false, "31", "x"), "x");
    }

    #[test]
    fn cut_says_how_to_get_the_rest() {
        assert_eq!(cut("abc", 5, "x"), "abc");
        assert_eq!(cut("abcdef", 3, "rkb lint --full"), "abc... [6 chars; run `rkb lint --full` for all]");
    }
}
