use std::io::Read;
use std::path::{Path, PathBuf};

use rkb_core::conditions::{Facts, Verdict};
use rkb_core::hooks::{self, DEFAULT_MIN_COVERAGE, MAX_NUDGES};
use rkb_core::matching::Hints;
use rkb_core::search::{Mode, Options};
use rkb_core::{config, kb, lock, paths, request, search, state};
use serde_json::{Value, json};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Runs one Claude Code hook event. Never fails: errors go to `hook-errors.log` and nothing is printed.
pub fn run(event: &str, harness: &str) {
    let state = paths::state_dir();
    let reply = if hooks::HARNESSES.contains(&harness) {
        let _ = hooks::beat(&state, harness);
        read_payload().and_then(|p| handle(event, &p, &state))
    } else {
        Err(format!("unknown harness `{harness}`; use one of {}", hooks::HARNESSES.join(", ")).into())
    };
    match reply {
        Ok(Some(text)) => println!("{text}"),
        Ok(None) => {}
        Err(e) => {
            let line = json!({ "time": request::now(), "event": event, "error": e.to_string() });
            let _ = lock::append_line(&state.join("hook-errors.log"), &line.to_string());
        }
    }
}

fn read_payload() -> Result<Value> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let v: Value = serde_json::from_str(&input)?;
    if !v.is_object() {
        return Err("the hook payload is not a JSON object".into());
    }
    Ok(v)
}

fn handle(event: &str, p: &Value, state: &Path) -> Result<Option<String>> {
    let session = p["session_id"].as_str().unwrap_or("");
    match event {
        "session-start" => session_start(p, state),
        "pre-tool" => pre_tool(p, state),
        "tool-ok" => tool_ok(p, state, session),
        "tool-failed" => tool_failed(p, state, session),
        "prompt" => prompt(p, state, session),
        "stop" => stop(p, state, session),
        _ => Err(format!("unknown hook event `{event}`").into()),
    }
}

fn cwd(p: &Value) -> PathBuf {
    p["cwd"].as_str().map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

fn bash_command(p: &Value) -> Option<&str> {
    (p["tool_name"] == "Bash").then(|| p["tool_input"]["command"].as_str()).flatten()
}

fn hooks_config(root: &Path) -> toml::Table {
    std::fs::read_to_string(root.join("kb.toml"))
        .ok()
        .and_then(|t| config::parse::<config::KbConfig>(&t).ok())
        .and_then(|c| c.hooks)
        .unwrap_or_default()
}

fn reply(event: &str, key: &str, value: Value) -> String {
    let mut out = json!({ "hookEventName": event });
    out[key] = value;
    json!({ "hookSpecificOutput": out }).to_string()
}

fn session_start(p: &Value, state: &Path) -> Result<Option<String>> {
    hooks::cleanup(state);
    let root = kb::home();
    kb::open(&root)?;
    let place = state::locate(&root, &cwd(p), &Hints::default(), state)?;
    let s = state::build(&root, place, state)?;
    Ok(Some(crate::session::context(&s).human))
}

fn pre_tool(p: &Value, state: &Path) -> Result<Option<String>> {
    let Some(command) = bash_command(p) else { return Ok(None) };
    if !command.contains("rkb") {
        return Ok(None);
    }
    let Some((id, choice)) = hooks::confirm_call(command) else { return Ok(None) };
    let question = id.and_then(|id| request::load(&state.join("requests"), &id).ok()).map(|r| r.question);
    let reason = match (question, choice) {
        (Some(q), Some(c)) => format!("rkb confirm: {q} -- choice: {c}"),
        (Some(q), None) => format!("rkb confirm: {q}"),
        (None, Some(c)) => format!("rkb confirm needs your approval -- choice: {c}"),
        (None, None) => "rkb confirm needs your approval".into(),
    };
    let mut out = json!({ "hookEventName": "PreToolUse", "permissionDecision": "ask" });
    out["permissionDecisionReason"] = json!(reason);
    Ok(Some(json!({ "hookSpecificOutput": out }).to_string()))
}

fn tool_ok(p: &Value, state: &Path, session: &str) -> Result<Option<String>> {
    let Some(program) = bash_command(p).and_then(hooks::program) else { return Ok(None) };
    let last = hooks::read(state, session)
        .into_iter()
        .rev()
        .find(|r| r["program"] == program.as_str() && (r["kind"] == "failed" || r["kind"] == "fixed"));
    if last.is_some_and(|r| r["kind"] == "failed") {
        hooks::append(state, session, &json!({ "kind": "fixed", "program": program }))?;
    }
    Ok(None)
}

fn tool_failed(p: &Value, state: &Path, session: &str) -> Result<Option<String>> {
    if p["is_interrupt"] == true {
        return Ok(None);
    }
    let Some(command) = bash_command(p) else { return Ok(None) };
    let Some(program) = hooks::program(command) else { return Ok(None) };
    hooks::append(state, session, &json!({ "kind": "failed", "program": program }))?;

    let root = kb::home();
    kb::open(&root)?;
    let error = p["error"].as_str().unwrap_or("");
    let query = hooks::error_query(command, error);
    let place = state::locate(&root, &cwd(p), &Hints::default(), state)?;
    let facts = Facts::gather(&root, &place, &[]);
    let opts = Options { all: false, every_status: false, limit: 1 };
    let Some(hit) = search::search(&root, &place, &facts, &Mode::Ranked(query.clone()), &opts)?.hits.into_iter().next() else {
        return Ok(None);
    };
    if hit.applies == Verdict::No {
        return Ok(None);
    }
    if hooks::read(state, session).iter().any(|r| r["kind"] == "injected" && r["id"] == hit.id.as_str()) {
        return Ok(None);
    }
    let min = hooks_config(&root).get("min_coverage").and_then(toml::Value::as_float).unwrap_or(DEFAULT_MIN_COVERAGE);
    let (_, text) = kb::find(&root, &hit.id)?;
    let coverage = hooks::coverage(error, &text);
    if coverage < min {
        return Ok(None);
    }
    hooks::append(state, session, &json!({ "kind": "injected", "id": hit.id }))?;
    let log = json!({ "time": request::now(), "session": session, "id": hit.id, "coverage": coverage });
    lock::append_line(&state.join("injections.jsonl"), &log.to_string())?;
    let summary = if hit.summary.is_empty() { String::new() } else { format!(" - {}", hit.summary) };
    let context = format!(
        "rkb: lesson {} may explain this failure: {}{summary}\nRead it with `rkb show {}` and check that it applies before acting on it.",
        hit.id, hit.title, hit.id
    );
    Ok(Some(reply("PostToolUseFailure", "additionalContext", json!(context))))
}

fn prompt(p: &Value, state: &Path, session: &str) -> Result<Option<String>> {
    let (correction, remember) = hooks::prompt_signals(p["prompt"].as_str().unwrap_or(""));
    if correction {
        hooks::append(state, session, &json!({ "kind": "correction" }))?;
    }
    if remember {
        hooks::append(state, session, &json!({ "kind": "remember" }))?;
    }
    Ok(None)
}

fn stop(p: &Value, state: &Path, session: &str) -> Result<Option<String>> {
    if p["stop_hook_active"] == true {
        return Ok(None);
    }
    let root = kb::home();
    if hooks_config(&root).get("stop_nudge").and_then(toml::Value::as_bool) != Some(true) {
        return Ok(None);
    }
    let records = hooks::read(state, session);
    if records.iter().filter(|r| r["kind"] == "nudged").count() >= MAX_NUDGES {
        return Ok(None);
    }
    let from = records.iter().rev().find_map(|r| (r["kind"] == "offered").then(|| r["upto"].as_u64()).flatten()).unwrap_or(0);
    let new: Vec<&Value> = records
        .iter()
        .skip(from as usize)
        .filter(|r| r["kind"] == "fixed" || r["kind"] == "correction" || r["kind"] == "remember")
        .collect();
    if new.is_empty() {
        return Ok(None);
    }
    let count = |k: &str| new.iter().filter(|r| r["kind"] == k).count();
    let mut programs: Vec<&str> = new.iter().filter_map(|r| r["program"].as_str()).collect();
    programs.sort_unstable();
    programs.dedup();
    let mut parts = vec![];
    if !programs.is_empty() {
        parts.push(format!("{} fixed failure(s) ({})", count("fixed"), programs.join(", ")));
    }
    if count("correction") > 0 {
        parts.push(format!("{} correction(s) from the user", count("correction")));
    }
    if count("remember") > 0 {
        parts.push(format!("{} request(s) to remember something", count("remember")));
    }
    let context = format!(
        "rkb: this session had {}. If one taught something durable, record it with `rkb add`; otherwise ignore this note.",
        parts.join(", ")
    );
    hooks::append(state, session, &json!({ "kind": "offered", "upto": records.len() }))?;
    hooks::append(state, session, &json!({ "kind": "nudged" }))?;
    Ok(Some(reply("Stop", "additionalContext", json!(context))))
}
