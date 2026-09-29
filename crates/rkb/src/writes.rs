use std::io::{BufRead, BufReader, IsTerminal, Read, Write};
use std::path::PathBuf;

use rkb_core::lesson::LessonType;
use rkb_core::lint::LintEnv;
use rkb_core::matching::Place;
use rkb_core::request::{self, Action, Request};
use rkb_core::write::{self, Ctx, Outcome, content_hash};
use rkb_core::{git, kb, paths, usage};
use serde_json::{Value, json};

use crate::output::{CliError, ErrorCode, Output, paint};

pub const TYPES: [&str; 5] = ["pitfall", "recipe", "fact", "decision", "preference"];

pub struct Env {
    pub root: PathBuf,
    pub lint: LintEnv,
    pub state: PathBuf,
    pub colored: bool,
    pub place: Option<Place>,
}

impl Env {
    pub(crate) fn ctx(&self) -> Ctx<'_> {
        Ctx { root: &self.root, env: &self.lint, state: &self.state, today: jiff::Zoned::now().date(), place: self.place.as_ref() }
    }
}

fn usage_error(message: &str, fix: &str) -> CliError {
    CliError::new(ErrorCode::Usage, message, fix)
}

fn stdin_is_terminal() -> bool {
    std::io::stdin().is_terminal()
}

fn read_stdin() -> Result<String, CliError> {
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|e| usage_error(&format!("cannot read stdin: {e}"), "pipe the lesson file on stdin"))?;
    Ok(text)
}

/// Opens `$VISUAL`, `$EDITOR` or `vi` on `initial` and returns the saved text.
fn editor(initial: &str) -> Result<String, CliError> {
    let path = std::env::temp_dir().join(format!("rkb-{}-{}.md", std::process::id(), request::now()));
    std::fs::write(&path, initial).map_err(|e| usage_error(&format!("cannot write {}: {e}", path.display()), "check TMPDIR"))?;
    let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| "vi".into());
    let status = std::process::Command::new("sh").arg("-c").arg(format!("{editor} \"$1\"")).arg("sh").arg(&path).status();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::remove_file(&path);
    match status {
        Ok(s) if s.success() => Ok(text),
        _ => Err(usage_error(&format!("the editor `{editor}` failed"), "set VISUAL or EDITOR to a working editor")),
    }
}

fn nothing(message: &str) -> Output {
    Output { data: json!({ "status": "unchanged", "message": message }), human: message.to_string(), exit: 0, raw: false }
}

fn quote(choice: &str) -> String {
    format!("\"{}\"", choice.replace('"', "\\\""))
}

pub(crate) fn needs_user_output(env: &Env, req: &Request) -> Output {
    needs_user(env, req)
}

pub fn outcome(env: &Env, o: Outcome) -> Output {
    match o {
        Outcome::Written(w) => {
            let lines: Vec<&str> = w.diff.lines().collect();
            let show = format!("git -C {} show {}", env.root.display(), w.commit);
            let diff = if lines.len() > write::DIFF_LINES {
                format!("{}\n... [{} lines; run `{show}` for all]", lines[..write::DIFF_LINES].join("\n"), lines.len())
            } else {
                w.diff.trim_end().to_string()
            };
            let mut human = format!("{} {} [{}] in commit {}\n\n{diff}", paint(env.colored, "32", w.kind), w.path, w.id, w.commit);
            for n in &w.notes {
                human.push_str(&format!("\n{} {}", paint(env.colored, "33", "note:"), n.message()));
            }
            let data = json!({
                "status": "written",
                "action": w.kind,
                "id": w.id,
                "path": w.path,
                "title": w.title,
                "commit": w.commit,
                "diff": diff.lines().collect::<Vec<_>>(),
                "notes": w.notes.iter().map(|n| n.message()).collect::<Vec<_>>(),
                "help": [format!("Run `rkb show {}` to read the lesson", w.id)],
            });
            Output { data, human, exit: 0, raw: false }
        }
        Outcome::Unchanged { id, path } => {
            let mut out = nothing(&format!("No change to {path}; nothing was written"));
            out.data["id"] = json!(id);
            out.data["path"] = json!(path);
            out
        }
        Outcome::Cancelled => {
            Output { data: json!({ "status": "cancelled" }), human: "Cancelled; nothing was written".into(), exit: 0, raw: false }
        }
        Outcome::NeedsUser(req) => needs_user(env, &req),
        Outcome::Imported(i) => imported(env, &i),
        Outcome::Info(message) => Output { data: json!({ "status": "done", "message": message }), human: message, exit: 0, raw: false },
    }
}

fn imported(env: &Env, i: &write::Imported) -> Output {
    let c = env.colored;
    let mut human = String::new();
    for w in &i.added {
        human.push_str(&format!("{} {} [{}] in commit {}\n", paint(c, "32", "added  "), w.path, w.id, w.commit));
    }
    for s in &i.skipped {
        human.push_str(&format!("{} {s} (duplicate)\n", paint(c, "2", "skipped")));
    }
    for (s, reason) in &i.not_added {
        human.push_str(&format!("{} {s}: {reason}\n", paint(c, "31", "not added")));
    }
    let stopped = !i.not_added.is_empty();
    human.push_str(&format!("{} added, {} skipped, {} not added", i.added.len(), i.skipped.len(), i.not_added.len()));
    let mut help = vec!["Run `rkb list` to see the new lessons".to_string()];
    if stopped {
        help.insert(0, "Fix the first lesson that was not added, then run `rkb import` again with only the lessons not added".into());
    }
    let data = json!({
        "status": if stopped { "stopped" } else { "done" },
        "added": i.added.iter().map(|w| json!({ "id": w.id, "path": w.path, "commit": w.commit })).collect::<Vec<_>>(),
        "skipped": i.skipped,
        "not_added": i.not_added.iter().map(|(s, r)| json!({ "source": s, "reason": r })).collect::<Vec<_>>(),
        "help": help,
    });
    Output { data, human, exit: u8::from(stopped), raw: false }
}

/// `rkb import <dir>`: an invalid batch lists the problems; a valid one becomes one request with the report.
pub fn import(env: &Env, dir: &std::path::Path) -> Result<Output, CliError> {
    kb::open(&env.root)?;
    match rkb_core::import::plan(&env.ctx(), dir)? {
        rkb_core::import::Plan::Ready(req) => Ok(needs_user(env, &req)),
        rkb_core::import::Plan::Invalid(bad) => {
            let mut human = String::new();
            for b in &bad {
                human.push_str(&format!("{} {}\n", paint(env.colored, "31", "invalid"), b.source));
                for r in &b.reasons {
                    human.push_str(&format!("  {r}\n"));
                }
            }
            human.push_str(&format!("{} invalid; nothing was written. Fix the files, then run `rkb import` again", bad.len()));
            let data = json!({
                "status": "invalid",
                "invalid": bad.iter().map(|b| json!({ "source": b.source, "reasons": b.reasons })).collect::<Vec<_>>(),
                "help": ["Fix each file, then run `rkb import` again; nothing was written"],
            });
            Ok(Output { data, human, exit: 1, raw: false })
        }
    }
}

fn needs_user(env: &Env, req: &Request) -> Output {
    let options = req.options();
    let next = format!("rkb confirm {} --choice {}", req.id, quote(&options[0]));
    let mut human = format!("{}\n{}\n", paint(env.colored, "33;1", "Needs your decision:"), req.question);
    human.push_str(&format!("Options: {}\n", options.iter().map(|o| quote(o)).collect::<Vec<_>>().join(", ")));
    human.push_str(&format!("Answer with: rkb confirm {} --choice \"<option>\"", req.id));
    let data = json!({
        "status": "needs_user",
        "request": req.id,
        "question": req.question,
        "options": options,
        "next": next,
        "help": [
            "Show the question and the options to the user and wait for their answer",
            "Then run `rkb confirm` with the option they chose; nothing was written yet",
        ],
    });
    Output { data, human, exit: 3, raw: false }
}

pub fn add(
    env: &Env,
    topic: Option<String>,
    kind: Option<String>,
    template: bool,
    given: Option<String>,
    assets: Vec<String>,
) -> Result<Output, CliError> {
    let kind: Option<LessonType> = kind.map(|k| LessonType::from_name(&k).expect("clap checks the type"));
    if template {
        let kind =
            kind.ok_or_else(|| usage_error("--template needs --type", "add --type pitfall (or recipe, fact, decision, preference)"))?;
        let text = write::template(kind);
        return Ok(Output { data: json!({ "template": text }), human: text, exit: 0, raw: true });
    }
    kb::open(&env.root)?;
    let topic = topic.ok_or_else(|| usage_error("rkb add needs --topic", "add --topic <topic>, such as --topic cmake"))?;
    let text = if let Some(text) = given {
        text
    } else if stdin_is_terminal() {
        let kind =
            kind.ok_or_else(|| usage_error("no lesson on stdin", "pipe a lesson file on stdin, or pass --type to write one in $EDITOR"))?;
        let skeleton = write::template(kind);
        let text = editor(&skeleton)?;
        if text.trim().is_empty() || text == skeleton {
            return Ok(nothing("The lesson was not changed; nothing was written"));
        }
        text
    } else {
        read_stdin()?
    };
    let o = write::apply(&env.ctx(), &Action::Add { text, topic, assets: absolute(assets) }, &[])?;
    Ok(outcome(env, o))
}

/// `--asset` paths made absolute, so a confirm from another folder finds them.
fn absolute(assets: Vec<String>) -> Vec<String> {
    assets.into_iter().map(|a| std::path::absolute(&a).map(|p| p.display().to_string()).unwrap_or(a)).collect()
}

pub fn edit(env: &Env, id: String, base: Option<String>, assets: Vec<String>, given: Option<String>) -> Result<Output, CliError> {
    kb::open(&env.root)?;
    let (text, base) = if let Some(text) = given {
        let base = base.ok_or_else(|| usage_error("rkb_edit needs `base`", &format!("run rkb_show {id} and pass its hash as base")))?;
        (text, base)
    } else if stdin_is_terminal() {
        let (_, current) = kb::find(&env.root, &id)?;
        let text = editor(&current)?;
        if text == current && assets.is_empty() {
            return Ok(nothing("The lesson was not changed; nothing was written"));
        }
        (text, content_hash(current.as_bytes()))
    } else {
        let base = base.ok_or_else(|| {
            usage_error("rkb edit needs --base when the lesson comes on stdin", &format!("run `rkb show {id}` and pass its hash as --base"))
        })?;
        (read_stdin()?, base)
    };
    let o = write::apply(&env.ctx(), &Action::Edit { id, text, base, assets: absolute(assets) }, &[])?;
    Ok(outcome(env, o))
}

pub fn flag(env: &Env, id: String, reason: String) -> Result<Output, CliError> {
    kb::open(&env.root)?;
    let o = write::apply(&env.ctx(), &Action::Flag { id, reason }, &[])?;
    Ok(outcome(env, o))
}

pub fn lifecycle(env: &Env, action: Action) -> Result<Output, CliError> {
    kb::open(&env.root)?;
    Ok(outcome(env, write::apply(&env.ctx(), &action, &[])?))
}

pub fn used(env: &Env, id: String, failed: bool, reason: Option<String>, session: Option<String>) -> Result<Output, CliError> {
    kb::open(&env.root)?;
    kb::find(&env.root, &id)?;
    let result = if failed { "failed" } else { "worked" };
    if failed && reason.as_deref().is_none_or(|r| r.trim().is_empty()) {
        return Err(usage_error("a failed use needs a reason", &format!("used {id} --failed --reason \"what went wrong\"")));
    }
    let config = paths::config_dir();
    let session = session.or_else(usage::env_session).or_else(|| usage::recent_injection(&env.root, &config, &id));
    usage::record(&env.root, &config, &env.state, &usage::now(&id, result, session.clone(), reason.clone()))?;
    let mut data = json!({ "status": "recorded", "id": id, "result": result, "session": session });
    let mut human = format!("Recorded: {id} {result}");
    if failed {
        let reason = reason.unwrap_or_default();
        let flagged = outcome(env, write::apply(&env.ctx(), &Action::Flag { id, reason }, &[])?);
        human.push_str(&format!("\n{}", flagged.human));
        data["flag"] = flagged.data;
    }
    Ok(Output { data, human, exit: 0, raw: false })
}

pub fn log(env: &Env, id: String) -> Result<Output, CliError> {
    kb::open(&env.root)?;
    let (lesson, _) = kb::find(&env.root, &id)?;
    let out = git::run(&env.root, &["log", "--follow", "--format=%h%x09%as%x09%s", "--", &lesson.path])?;
    let rows: Vec<Value> = String::from_utf8_lossy(&out)
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(3, '\t');
            Some(json!({ "commit": p.next()?, "date": p.next()?, "subject": p.next()? }))
        })
        .collect();
    let human = rows
        .iter()
        .map(|r| format!("{}  {}  {}", r["commit"].as_str().unwrap(), r["date"].as_str().unwrap(), r["subject"].as_str().unwrap()))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Output { data: json!({ "id": id, "path": lesson.path, "commits": rows }), human, exit: 0, raw: false })
}

/// Asks the person at the terminal to type `yes`. `RKB_TTY` replaces `/dev/tty` in tests.
/// Set when the person answered the question at the terminal, so confirm does not ask again.
static ANSWERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The option a person typed: its number or its text. Anything else is `None`.
fn pick(options: &[String], answer: &str) -> Option<String> {
    let a = answer.trim();
    a.parse::<usize>().ok().and_then(|n| options.get(n.wrapping_sub(1))).or_else(|| options.iter().find(|o| o.as_str() == a)).cloned()
}

/// For a person at a terminal, asks a `needs_user` question right away and returns the option they chose.
/// Agents (Claude Code, rkb's pi/omp extension), pipes and machine formats get the question as output instead.
pub fn ask_now(format: crate::output::Format, out: &Output) -> Option<String> {
    let agent = std::env::var("CLAUDECODE").is_ok_and(|v| v == "1") || std::env::var_os("RKB_HARNESS").is_some();
    if out.data["status"] != "needs_user"
        || format != crate::output::Format::Human
        || agent
        || !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
    {
        return None;
    }
    let options: Vec<String> = out.data["options"].as_array()?.iter().filter_map(|o| o.as_str().map(String::from)).collect();
    let mut prompt = format!("{}\n", out.data["question"].as_str()?);
    for (i, o) in options.iter().enumerate() {
        prompt.push_str(&format!("  {}. {o}\n", i + 1));
    }
    let cancel = options.iter().find(|o| *o == "cancel").cloned();
    let mut stdout = std::io::stdout();
    for _ in 0..3 {
        let _ = write!(stdout, "{prompt}Choose 1-{}{}: ", options.len(), if cancel.is_some() { " (Enter cancels)" } else { "" });
        let _ = stdout.flush();
        let mut answer = String::new();
        if std::io::stdin().read_line(&mut answer).unwrap_or(0) == 0 {
            return cancel;
        }
        if answer.trim().is_empty() && cancel.is_some() {
            return cancel;
        }
        if let Some(choice) = pick(&options, &answer) {
            ANSWERED.store(true, std::sync::atomic::Ordering::Relaxed);
            return Some(choice);
        }
        prompt = format!("`{}` is not an option.\n", answer.trim());
    }
    cancel
}

fn ask_terminal(req: &Request, choice: &str) -> Result<(), CliError> {
    let tty = std::env::var("RKB_TTY").unwrap_or_else(|_| "/dev/tty".into());
    let command = format!("rkb confirm {} --choice {}", req.id, quote(choice));
    let open = std::fs::File::open(&tty).and_then(|r| std::fs::OpenOptions::new().append(true).open(&tty).map(|w| (r, w)));
    let Ok((r, mut w)) = open else {
        return Err(CliError::new(
            ErrorCode::NeedsTerminal,
            "rkb confirm needs a person at a terminal, and there is none",
            format!("ask the user to run `{command}` in a separate terminal window; Claude Code's `!` prefix has no terminal"),
        ));
    };
    let _ = write!(w, "{}\nChoice: {choice}\nType yes to confirm: ", req.question);
    let _ = w.flush();
    let mut answer = String::new();
    let _ = BufReader::new(r).read_line(&mut answer);
    if answer.trim() == "yes" {
        Ok(())
    } else {
        Err(CliError::new(ErrorCode::Refused, "the user did not confirm", format!("run `{command}` again to confirm")))
    }
}

pub fn confirm(env: &Env, id: String, choice: String) -> Result<Output, CliError> {
    let req = request::load(&env.state.join("requests"), &id)?;
    if !matches!(req.action, request::Action::Install { .. }) {
        kb::open(&env.root)?;
    }
    if !req.options().contains(&choice) {
        return Err(rkb_core::Error::BadChoice { choice, options: req.options() }.into());
    }
    let answered = ANSWERED.load(std::sync::atomic::Ordering::Relaxed);
    if choice != "cancel" && !answered && request::trusted_harness(&paths::config_dir()).is_none() {
        ask_terminal(&req, &choice)?;
    }
    if let request::Action::Publish { ids, encrypted, out, serve } = &req.action {
        request::remove(&env.state.join("requests"), &req.id);
        if choice == "cancel" {
            return Ok(outcome(env, Outcome::Cancelled));
        }
        return crate::site::run(
            env,
            out.clone(),
            *serve,
            &crate::site::Approved { clear: ids.clone(), encrypted: encrypted.clone() },
            false,
        );
    }
    let o = write::confirm(&env.ctx(), &req, &choice)?;
    Ok(outcome(env, o))
}

#[cfg(test)]
mod tests {
    use super::pick;

    #[test]
    fn pick_takes_a_number_or_the_option() {
        let o = vec!["publish".to_string(), "cancel".to_string()];
        assert_eq!(pick(&o, "1\n").as_deref(), Some("publish"));
        assert_eq!(pick(&o, " cancel ").as_deref(), Some("cancel"));
        assert_eq!(pick(&o, "0"), None);
        assert_eq!(pick(&o, "3"), None);
        assert_eq!(pick(&o, "yes"), None);
    }
}
