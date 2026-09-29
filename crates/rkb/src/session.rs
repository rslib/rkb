use rkb_core::doctor::{self, Check, Level};
use rkb_core::matching::{Matched, Place};
use rkb_core::state::State;
use rkb_core::{kb, paths};
use serde_json::{Value, json};

use crate::output::{CliError, Output, paint};
use crate::writes::{self, Env};

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn matched(m: &Option<Matched>) -> Value {
    m.as_ref().map_or(Value::Null, |m| json!({ "name": m.name, "rule": m.rule.describe() }))
}

fn next_list(place: &Place) -> String {
    match &place.project {
        Some(p) => format!("rkb list projects/{}", p.name),
        None => "rkb list".into(),
    }
}

/// The live state for `rkb` with no command.
pub fn status(env: &Env, s: &State) -> Output {
    let c = env.colored;
    let bin = std::env::current_exe().map(|p| crate::output::tilde(&p)).unwrap_or_default();
    let mut human = format!(
        "{} {}\n{}\n{} {}\n\n",
        paint(c, "1", "rkb"),
        paint(c, "2", &format!("- {}", crate::ABOUT)),
        paint(c, "2", &format!("bin {bin}")),
        paint(c, "2", "kb "),
        crate::output::tilde(&env.root)
    );
    let row = |label: &str, m: &Option<Matched>, n: Option<usize>| match m {
        Some(m) => format!(
            "{label:<8} {:<14} {:>12}   {}\n",
            m.name,
            plural(n.unwrap_or(0), "lesson"),
            paint(c, "2", &format!("({})", m.rule.describe()))
        ),
        None => format!("{label:<8} {}\n", paint(c, "2", &format!("none matched here; --{label} <name> sets it"))),
    };
    human.push_str(&row("project", &s.place.project, s.project_lessons));
    human.push_str(&row("system", &s.place.system, s.system_lessons));
    human.push_str(&format!("{:<8} {:<14} {:>12}\n", "general", "", plural(s.general_lessons, "lesson")));
    let mut notes = vec![];
    if s.pending > 0 {
        notes.push(paint(c, "33", &plural(s.pending, "pending request")));
    }
    if s.uncommitted > 0 {
        notes.push(paint(c, "33", &format!("{} uncommitted", plural(s.uncommitted, "change"))));
    }
    if !notes.is_empty() {
        human.push_str(&format!("\n{}\n", notes.join("  ")));
    }
    let next = next_list(&s.place);
    human.push_str(&format!("\nNext: {next}"));

    let mut help = vec![format!("Run `{next}` to see the lessons here")];
    if s.pending > 0 {
        help.push("Answer pending requests with `rkb confirm`, after asking the user".into());
    }
    if s.uncommitted > 0 {
        help.push("Run `rkb doctor` to see the uncommitted changes".into());
    }
    let data = json!({
        "bin": bin,
        "description": crate::ABOUT,
        "kb": crate::output::tilde(&env.root),
        "project": matched(&s.place.project),
        "system": matched(&s.place.system),
        "lessons": { "project": s.project_lessons, "system": s.system_lessons, "general": s.general_lessons },
        "pending": s.pending,
        "uncommitted": s.uncommitted,
        "help": help,
    });
    Output { data, human, exit: 0, raw: false }
}

const SEARCH_FIRST: &str = "before a web search or a guess about a problem: rkb search \"<words>\" --toon";

/// At most 5 lines, for a SessionStart hook.
pub fn context(s: &State) -> Output {
    let part = |label: &str, m: &Option<Matched>| match m {
        Some(m) => format!("{label} {} ({})", m.name, m.rule.short()),
        None => format!("no {label}"),
    };
    let place = format!("{}, {}", part("project", &s.place.project), part("system", &s.place.system));
    let mut counts = vec![];
    if let Some(n) = s.project_lessons {
        counts.push(format!("{n} project"));
    }
    if let Some(n) = s.system_lessons {
        counts.push(format!("{n} system"));
    }
    counts.push(format!("{} general", s.general_lessons));
    let lessons = counts.join(", ");
    let next = next_list(&s.place);
    let pending = (s.pending > 0).then(|| format!("{}; ask the user before `rkb confirm`", plural(s.pending, "pending request")));

    // Every agent sees these lines, also when it never loads the skill, so they carry its two rules.
    let first = SEARCH_FIRST;
    let mut human = format!("rkb: {place}; lessons here: {lessons}\n");
    if let Some(p) = &pending {
        human.push_str(&format!("{p}\n"));
    }
    human.push_str(&format!("{first}\nmore: {next} --toon; load the rkb skill before you write a lesson"));
    let mut data = json!({ "rkb": place, "lessons": lessons, "first": first });
    if let Some(p) = pending {
        data["pending"] = json!(p);
    }
    data["next"] = json!(next);
    Output { data, human, exit: 0, raw: false }
}

pub fn doctor(env: &Env, place: &Place, break_lock: bool) -> Result<Output, CliError> {
    if break_lock {
        kb::open(&env.root)?;
        return Ok(match doctor::break_lock_request(&env.root, &env.state)? {
            Some(req) => writes::outcome(env, rkb_core::write::Outcome::NeedsUser(req)),
            None => writes::outcome(env, rkb_core::write::Outcome::Info("There is no write lock; nothing to break".into())),
        });
    }
    let mut checks = doctor::run(&env.root, place, &env.state, &paths::config_dir(), &env.lint);
    checks.extend(crate::rerankers::model_check(&rkb_core::rerank::Settings::load(&env.root)));
    let failed = checks.iter().filter(|c| c.level == Level::Fail).count();
    let warned = checks.iter().filter(|c| c.level == Level::Warn).count();
    Ok(Output {
        data: doctor_data(&checks, failed, warned),
        human: doctor_human(env.colored, &checks, failed, warned),
        exit: u8::from(failed > 0),
        raw: false,
    })
}

fn doctor_data(checks: &[Check], failed: usize, warned: usize) -> Value {
    let rows: Vec<Value> = checks
        .iter()
        .map(|c| json!({ "check": c.name, "status": c.level, "detail": c.detail, "fix": c.fix.clone().unwrap_or_default() }))
        .collect();
    let mut data = json!({ "failed": failed, "warnings": warned, "checks": rows });
    if failed + warned > 0 {
        data["help"] = json!(["Run the fix of each check that is not ok, then `rkb doctor` again"]);
    }
    data
}

fn doctor_human(c: bool, checks: &[Check], failed: usize, warned: usize) -> String {
    let width = checks.iter().map(|k| k.name.len()).max().unwrap_or(0);
    let mut out = String::new();
    for k in checks {
        let mark = match k.level {
            Level::Ok => paint(c, "32", "ok  "),
            Level::Warn => paint(c, "33", "warn"),
            Level::Fail => paint(c, "31", "FAIL"),
        };
        out.push_str(&format!("{mark}  {:width$}  {}\n", k.name, k.detail));
        if let Some(fix) = &k.fix {
            out.push_str(&format!("      {:width$}  {} {fix}\n", "", paint(c, "2", "fix:")));
        }
    }
    out.push_str(&format!("\n{failed} failed, {warned} warnings"));
    out
}

/// `rkb review`: one block per candidate with every reason and the use dates.
pub fn signals(s: &rkb_core::review::Signals, c: bool) -> Output {
    let share = |n: usize, of: usize| (n * 100).checked_div(of).map_or_else(|| "-".to_string(), |p| format!("{p}%"));
    let mut human = format!(
        "{}\n  {} sessions: {} with a failed command, {} with a fix, {} with a correction, {} with \"remember\"\n  {} lessons injected, {} then helped ({})\n  {} extracts saved to the inbox\n  {} mistakes repeated in a later session, {} of them with a lesson injected",
        paint(c, "1", "Signals on this machine"),
        s.sessions,
        s.with_failed,
        s.with_fixed,
        s.with_correction,
        s.with_remember,
        s.injected,
        s.injected_then_helped,
        share(s.injected_then_helped, s.injected),
        s.extracts,
        s.repeated,
        s.repeated_with_lesson
    );
    for (path, p) in &s.paths {
        human.push_str(&format!(
            "\n  {path}: {} injected, {} then helped ({}), {} marked irrelevant ({})",
            p.injected,
            p.helped,
            share(p.helped, p.injected),
            p.irrelevant,
            share(p.irrelevant, p.injected)
        ));
    }
    Output { data: json!({ "signals": s }), human, exit: 0, raw: false }
}

pub fn review(candidates: &[rkb_core::review::Candidate], c: bool) -> Output {
    if candidates.is_empty() {
        return Output {
            data: json!({ "candidates": [], "help": ["Nothing to review"] }),
            human: "Nothing to review: no current lesson has a review reason.".into(),
            exit: 0,
            raw: false,
        };
    }
    let mut human = String::new();
    for k in candidates {
        human.push_str(&format!("{}  {}\n  {}\n", paint(c, "1", &k.title), paint(c, "2", &k.id), paint(c, "2", &k.path)));
        for r in &k.reasons {
            let kind = serde_json::to_value(r.kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
            human.push_str(&format!("  {} {}\n", paint(c, "33", &format!("{kind:<14}")), r.detail));
        }
        let used = format!(
            "  last worked {}, last failed {}",
            k.last_worked.as_deref().unwrap_or("never"),
            k.last_failed.as_deref().unwrap_or("never")
        );
        human.push_str(&format!("{}\n\n", paint(c, "2", &used)));
    }
    human.push_str(&format!("{}\nArchive with: rkb archive <id> --reason \"<why>\"", plural(candidates.len(), "candidate")));
    let data = json!({
        "candidates": candidates,
        "help": ["For each candidate: fix it with `rkb edit`, merge it with `rkb supersede`, archive it with `rkb archive <id> --reason \"<why>\"`, or leave it; tell the user what you did"],
    });
    Output { data, human, exit: 0, raw: false }
}

/// `rkb changes`: one line per rkb commit, with the reason under it when there is one.
pub fn changes(rows: &[rkb_core::review::Change], since: &str, c: bool) -> Output {
    let mut human = String::new();
    for r in rows {
        let what = if r.id.is_empty() { r.folder.clone() } else { format!("{}  {}", r.id, r.folder) };
        human.push_str(&format!(
            "{}  {}  {}  {}\n",
            paint(c, "2", &r.date),
            paint(c, "33", &format!("{:<9}", r.kind)),
            r.title,
            paint(c, "2", &what)
        ));
        if let Some(reason) = &r.reason {
            human.push_str(&format!("    {}\n", paint(c, "2", reason)));
        }
    }
    if rows.is_empty() {
        human = format!("rkb wrote nothing since {since}.");
    } else {
        human.push_str(&format!("{} since {since}; see one with `rkb show <id>` or `git show <commit>`", plural(rows.len(), "change")));
    }
    Output { data: json!({ "since": since, "changes": rows }), human, exit: 0, raw: false }
}

/// `rkb verify --auto`: counts, then the ids of each group that is not empty.
pub fn verify_summary(s: &rkb_core::verify::Summary, c: bool) -> Output {
    let groups = [("passed", &s.passed, "32"), ("failed", &s.failed, "31"), ("unknown", &s.unknown, "33"), ("skipped", &s.skipped, "2")];
    let mut human = groups.iter().map(|(n, v, _)| format!("{} {n}", v.len())).collect::<Vec<_>>().join(", ");
    for (name, ids, color) in groups {
        if !ids.is_empty() {
            human.push_str(&format!("\n{} {}", paint(c, color, &format!("{name:<8}")), ids.join(" ")));
        }
    }
    let mut data = json!({ "passed": s.passed, "failed": s.failed, "unknown": s.unknown, "skipped": s.skipped });
    if !s.failed.is_empty() {
        data["help"] = json!(["Failed lessons are stale now; run `rkb show <id>` to see the reason"]);
    }
    Output { data, human, exit: 0, raw: false }
}
