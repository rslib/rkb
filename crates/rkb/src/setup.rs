use std::io::IsTerminal;
use std::path::PathBuf;

use rkb_core::install::{self, Effect, Harness};
use rkb_core::paths;
use rkb_core::request::{self, Action, Choice, Decision, Request};
use serde_json::json;

use crate::output::{CliError, ErrorCode, Output, paint};
use crate::writes::{Env, needs_user_output};

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

/// `path` with the home folder shown as `~`.
fn tilde(path: &std::path::Path) -> String {
    let h = home();
    match path.strip_prefix(&h) {
        Ok(rest) if !h.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

fn harnesses(names: &[String], home: &std::path::Path) -> Result<Vec<Harness>, CliError> {
    if names.is_empty() {
        let found: Vec<Harness> = Harness::ALL.into_iter().filter(|h| h.detected(home)).collect();
        if found.is_empty() {
            return Err(CliError::new(
                ErrorCode::NotFound,
                "no supported harness found (~/.claude, ~/.pi or ~/.omp)",
                "install Claude Code, pi or omp, or name one: rkb install claude",
            ));
        }
        return Ok(found);
    }
    names
        .iter()
        .map(|n| {
            Harness::parse(n).ok_or_else(|| {
                let known: Vec<&str> = Harness::ALL.iter().map(|h| h.name()).collect();
                CliError::new(ErrorCode::Usage, format!("unknown harness `{n}`"), format!("use one of: {}", known.join(", ")))
            })
        })
        .collect()
}

fn list(env: &Env) -> Output {
    let home = home();
    let mut human = String::new();
    let rows: Vec<serde_json::Value> = Harness::ALL
        .iter()
        .map(|h| {
            let detected = h.detected(&home);
            let (installed, current) = install::status(*h, &home);
            let state = match (detected, installed, current) {
                (false, _, _) => "not detected",
                (true, false, _) => "not installed",
                (true, true, false) => "outdated",
                (true, true, true) => "current",
            };
            let color = match state {
                "current" => "32",
                "outdated" | "not installed" => "33",
                _ => "2",
            };
            human.push_str(&format!(
                "{:<7} {}  {}\n",
                h.name(),
                paint(env.colored, color, &format!("{state:<13}")),
                tilde(&h.skill_dir(&home).join("SKILL.md"))
            ));
            let mut row = json!({ "harness": h.name(), "detected": detected, "installed": installed, "current": current });
            if let (Some((ext_installed, ext_current)), Some(file)) = (install::extension_status(*h, &home), h.extension_file(&home)) {
                let state = match (detected, ext_installed, ext_current) {
                    (false, _, _) => "not detected",
                    (true, false, _) => "not installed",
                    (true, true, false) => "outdated",
                    (true, true, true) => "current",
                };
                let color = if state == "current" {
                    "32"
                } else if detected {
                    "33"
                } else {
                    "2"
                };
                human.push_str(&format!("{:<7} {}  {}\n", "", paint(env.colored, color, &format!("{state:<13}")), tilde(&file)));
                row["extension"] = json!({ "installed": ext_installed, "current": ext_current });
            }
            row
        })
        .collect();
    let gate = match request::trusted_harness(&paths::config_dir()) {
        Some(h) => format!("{h} asks the user before `rkb confirm`"),
        None => "terminal (no trusted harness in this session)".into(),
    };
    human.push_str(&format!("\nconfirm gate: {gate}"));
    Output {
        data: json!({ "harnesses": rows, "help": ["Run `rkb install` to install or update the skill in every detected harness"] }),
        human,
        exit: 0,
        raw: false,
    }
}

pub fn install(env: &Env, names: Vec<String>, list_only: bool, uninstall: bool) -> Result<Output, CliError> {
    if list_only {
        return Ok(list(env));
    }
    let home = home();
    let hs = harnesses(&names, &home)?;
    let steps = install::plan(&home, &paths::config_dir(), &hs, uninstall);

    let person = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    if !person {
        let files: Vec<String> = steps.iter().map(|s| format!("  {}", tilde(&s.file()))).collect();
        let verb = if uninstall { "Uninstall rkb from" } else { "Install rkb into" };
        let names: Vec<&str> = hs.iter().map(|h| h.name()).collect();
        let req = Request {
            id: request::new_id(),
            created: request::now(),
            action: Action::Install { harnesses: hs.clone(), uninstall },
            approved: vec![],
            question: format!(
                "{verb} {}? This changes these files, and it lets the harness's own permission prompt stand in for the terminal check of `rkb confirm`:\n{}",
                names.join(", "),
                files.join("\n")
            ),
            choices: vec![
                Choice { text: if uninstall { "uninstall".into() } else { "install".into() }, decision: Some(Decision::Install) },
                Choice { text: "cancel".into(), decision: None },
            ],
        };
        request::save(&env.state.join("requests"), &req)?;
        return Ok(needs_user_output(env, &req));
    }

    let changes = install::apply(&steps, install::SKILL)?;
    let mut human = String::new();
    for c in &changes {
        let (word, color) = match c.effect {
            Effect::Written => ("written", "32"),
            Effect::Removed => ("removed", "33"),
            Effect::Unchanged => ("unchanged", "2"),
        };
        human.push_str(&format!("{} {}\n", paint(env.colored, color, &format!("{word:<9}")), tilde(&c.path)));
    }
    if changes.iter().all(|c| c.effect == Effect::Unchanged) {
        human.push_str("Nothing changed; everything was already in place.\n");
    }
    let rows: Vec<serde_json::Value> = changes.iter().map(|c| json!({ "path": tilde(&c.path), "effect": c.effect })).collect();
    Ok(Output { data: json!({ "status": "done", "changes": rows }), human, exit: 0, raw: false })
}
