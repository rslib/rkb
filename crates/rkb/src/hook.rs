use std::io::Read;
use std::path::{Path, PathBuf};

use rkb_core::conditions::{Facts, Verdict};
use rkb_core::hooks::{self, DEFAULT_HOOK_TIMEOUT_MS, DEFAULT_RECORD_SCORE, MAX_NUDGES};
use rkb_core::matching::{Hints, Place};
use rkb_core::search::{Mode, Options};
use rkb_core::{config, kb, lock, paths, request, rerank, search, state, usage};
use serde_json::{Value, json};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Runs one Claude Code hook event. Never fails: errors go to `hook-errors.log` and nothing is printed.
/// `from_mod` is true when the Claude Code mod calls; a command hook of a session the mod handles does nothing.
pub fn run(event: &str, harness: &str, from_mod: bool) {
    if rkb_core::observer::active() {
        return;
    }
    let state = paths::state_dir();
    // A panic, also in a search thread, is logged like any hook error instead of printed, so it never
    // reaches the harness as a failed hook.
    let log = state.join("hook-errors.log");
    let event_name = event.to_string();
    std::panic::set_hook(Box::new(move |info| {
        let line = json!({ "time": request::now(), "event": event_name, "error": format!("panic: {info}") });
        let _ = lock::append_line(&log, &line.to_string());
    }));
    let reply = std::panic::catch_unwind(|| {
        if !hooks::HARNESSES.contains(&harness) {
            return Err(format!("unknown harness `{harness}`; use one of {}", hooks::HARNESSES.join(", ")).into());
        }
        let p = read_payload()?;
        // The confirm gate needs `permission_mode`, which only the command hook's input carries.
        if !from_mod && event != "pre-tool" && mod_handles(&p) {
            return Ok(None);
        }
        let _ = hooks::beat(&state, harness);
        if harness == "claude-code" && event != "pre-tool" {
            let _ = hooks::note_caller(&state, p["session_id"].as_str().unwrap_or(""), if from_mod { "mod" } else { "command" });
        }
        handle(event, &p, &state, harness)
    })
    .unwrap_or_else(|_| Ok(None));
    match reply {
        Ok(Some(text)) => println!("{text}"),
        Ok(None) => {}
        Err(e) => {
            let line = json!({ "time": request::now(), "event": event, "error": e.to_string() });
            let _ = lock::append_line(&state.join("hook-errors.log"), &line.to_string());
        }
    }
}

/// Whether the Claude Code mod handles this payload's session: it sets `RKB_MOD` to the session id, which
/// a pi or omp session started from a Claude Code shell inherits with another id.
fn mod_handles(p: &Value) -> bool {
    let session = p["session_id"].as_str().unwrap_or("");
    !session.is_empty() && std::env::var("RKB_MOD").is_ok_and(|m| m == session)
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

fn handle(event: &str, p: &Value, state: &Path, harness: &str) -> Result<Option<String>> {
    let session = p["session_id"].as_str().unwrap_or("");
    match event {
        "session-start" => session_start(p, state, harness),
        "pre-tool" => pre_tool(p, state),
        "tool-ok" => tool_ok(p, state, session),
        "tool-failed" => tool_failed(p, state, session),
        "prompt" => prompt(p, state, session),
        "stop" => stop(p, state, session, harness),
        "pre-compact" => capture(p, state, session, harness),
        "session-end" => {
            let captured = capture(p, state, session, harness);
            start_observer(p, session, harness)?;
            captured
        }
        _ => Err(format!("unknown hook event `{event}`").into()),
    }
}

fn cwd(p: &Value) -> PathBuf {
    p["cwd"].as_str().map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

fn bash_command(p: &Value) -> Option<&str> {
    (p["tool_name"] == "Bash").then(|| p["tool_input"]["command"].as_str()).flatten()
}

pub(crate) fn hooks_config(root: &Path) -> toml::Table {
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

fn session_start(p: &Value, state: &Path, harness: &str) -> Result<Option<String>> {
    hooks::cleanup(state);
    let root = kb::home();
    kb::open(&root)?;
    let place = state::locate(&root, &cwd(p), &Hints::default(), state)?;
    let s = state::build(&root, place, state)?;
    let mut text = crate::session::context(&s).human;
    // With automatic runs on, the Claude Code mod starts distill and curate, so the agent is not asked to.
    if harness == "claude-code" && rkb_core::auto::enabled(&paths::config_dir()) {
        return Ok(Some(text));
    }
    let distill = if harness == "claude-code" { "/rkb:distill" } else { "/rkb-distill" };
    if let Some(line) = rkb_core::distill::nudge(state, request::now(), distill, rkb_core::distill::batch(&paths::config_dir())) {
        text.push_str(&format!("\n{line}"));
    }
    let curate = if harness == "claude-code" { "/rkb:curate" } else { "/rkb-curate" };
    if let Some(line) = rkb_core::review::curate_nudge(&root, state, curate) {
        text.push_str(&format!("\n{line}"));
    }
    Ok(Some(text))
}

fn pre_tool(p: &Value, state: &Path) -> Result<Option<String>> {
    let Some(command) = bash_command(p) else { return Ok(None) };
    if !command.contains("rkb") {
        return Ok(None);
    }
    let Some((id, choice)) = hooks::confirm_call(command) else { return Ok(None) };
    let question = id.as_ref().and_then(|id| request::load(&state.join("requests"), id).ok()).map(|r| r.question);
    let mode = p["permission_mode"].as_str();
    if hooks::mode_skips_prompts(mode) {
        let reason = hooks::confirm_deny_reason(mode.unwrap_or_default(), id.as_deref(), question.as_deref(), choice.as_deref());
        let out = json!({ "hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": reason });
        return Ok(Some(json!({ "hookSpecificOutput": out }).to_string()));
    }
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
        // A lesson injected while this program kept failing, and the program now works: it probably helped.
        // The window starts after the program's previous fix, so retries after the hint still count.
        let records = hooks::read(state, session);
        let before_now = &records[..records.len().saturating_sub(1)];
        let from = before_now.iter().rposition(|r| r["kind"] == "fixed" && r["program"] == program.as_str()).map_or(0, |i| i + 1);
        let injected: Vec<String> = before_now[from..]
            .iter()
            .filter(|r| r["kind"] == "injected")
            .filter_map(|r| r["id"].as_str().map(String::from))
            .filter(|id| !records.iter().any(|r| r["kind"] == "inferred" && r["id"] == id.as_str()))
            .collect();
        if !injected.is_empty() {
            let root = kb::home();
            for id in injected {
                hooks::append(state, session, &json!({ "kind": "inferred", "id": id }))?;
                record_use(&root, state, &id, "inferred", session, Some(format!("{program} worked after it was injected")))?;
            }
        }
    }
    Ok(None)
}

/// `missing here: …` for the names a project lesson gives that this checkout lacks.
fn missing_note(place: &Place, path: &str, text: &str) -> Vec<String> {
    let missing = hooks::missing_here(place, path, text);
    if missing.is_empty() { vec![] } else { vec![format!("missing here: {}", missing.join(", "))] }
}

/// A search hit as an injected lesson block.
fn injected(h: &search::Hit, notes: Vec<String>) -> hooks::Injected<'_> {
    hooks::Injected {
        id: &h.id,
        title: &h.title,
        summary: &h.summary,
        verified: &h.verified,
        how: &h.verified_how,
        applies: h.applies.as_str(),
        notes,
    }
}

/// Appends a use record for a hook event to this machine's usage file in the knowledge base.
fn record_use(root: &Path, state: &Path, id: &str, event: &str, session: &str, reason: Option<String>) -> Result<()> {
    let session = (!session.is_empty()).then(|| session.to_string());
    Ok(usage::record(root, &paths::config_dir(), state, &usage::now(id, event, session, reason))?)
}

fn tool_failed(p: &Value, state: &Path, session: &str) -> Result<Option<String>> {
    if p["is_interrupt"] == true {
        return Ok(None);
    }
    let Some(command) = bash_command(p) else { return Ok(None) };
    let Some(program) = hooks::program(command) else { return Ok(None) };
    hooks::append(state, session, &json!({ "kind": "failed", "program": program }))?;
    let error = p["error"].as_str().unwrap_or("");
    // Signals for "worth a lesson": the same error in an earlier session, and failures no lesson covers.
    let once = |kind: &str| -> Result<()> {
        if !hooks::read(state, session).iter().any(|r| r["kind"] == kind && r["program"] == program.as_str()) {
            hooks::append(state, session, &json!({ "kind": kind, "program": program }))?;
        }
        Ok(())
    };
    if hooks::seen_before(state, &hooks::signature(&program, error), session) {
        once("repeat")?;
    }

    let root = kb::home();
    kb::open(&root)?;
    if hooks_config(&root).get("failure_recall").and_then(toml::Value::as_bool) == Some(false) {
        return Ok(None);
    }
    let query = hooks::error_query(command, error);
    let cfg = hooks_config(&root);
    let (hits, ranked, place) = ranked_search(&root, p, state, &cfg, &query, "failure")?;
    let chosen = hooks::would_inject(
        &hits,
        |id| kb::find(&root, id).map(|(_, t)| t).unwrap_or_default(),
        error,
        &hooks::Strength::from_config(&cfg, &ranked.backend),
    );
    let seen = chosen.is_some_and(|h| hooks::read(state, session).iter().any(|r| r["kind"] == "injected" && r["id"] == h.id.as_str()));
    let hit = chosen.filter(|_| !seen).cloned();
    if cfg.get("replay_log").and_then(toml::Value::as_bool) != Some(false) {
        let name = |m: &Option<rkb_core::matching::Matched>| m.as_ref().map(|m| m.name.clone());
        hooks::log_replay(
            state,
            &hooks::Replay {
                time: rkb_core::request::now(),
                session: session.to_string(),
                project: name(&place.project),
                system: name(&place.system),
                query: query.clone(),
                injected: hit.as_ref().map(|h| h.id.clone()),
            },
        )?;
    }
    let Some(hit) = hit else {
        if !seen {
            once("nohit")?;
        }
        return Ok(None);
    };
    let (_, text) = kb::find(&root, &hit.id)?;
    hooks::append(state, session, &json!({ "kind": "injected", "id": hit.id, "hook": "tool-failed", "time": rkb_core::request::now() }))?;
    record_use(&root, state, &hit.id, "injected", session, Some(format!("tool-failed, {}", ranked.describe())))?;
    let context = hooks::render(&[injected(&hit, missing_note(&place, &hit.path, &text))]);
    Ok(Some(reply("PostToolUseFailure", "additionalContext", json!(context))))
}

fn capture(p: &Value, state: &Path, session: &str, harness: &str) -> Result<Option<String>> {
    let transcript = p["transcript_path"].as_str().ok_or("the payload has no transcript_path")?;
    rkb_core::distill::capture(state, harness, session, Path::new(transcript), p["cwd"].as_str())?;
    Ok(None)
}

/// Starts `rkb observe` for the session in the background when the observer has an approved chain
/// for this harness, and returns at once.
fn start_observer(p: &Value, session: &str, harness: &str) -> Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let Some(transcript) = p["transcript_path"].as_str() else { return Ok(()) };
    if rkb_core::observer::ready(&paths::config_dir(), harness).is_none() {
        return Ok(());
    }
    let mut c = Command::new(std::env::current_exe()?);
    c.args(["observe", "--session", session, "--transcript", transcript, "--harness", harness]);
    if let Some(cwd) = p["cwd"].as_str() {
        c.args(["--cwd", cwd]);
    }
    if let Some(model) = p["model"].as_str().filter(|m| !m.is_empty()) {
        c.args(["--model", model]);
    }
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0).spawn()?;
    Ok(())
}

/// The hits for `query` from the current place, reranked by the chain within the hook time limit.
fn ranked_search(
    root: &Path,
    p: &Value,
    state: &Path,
    cfg: &toml::Table,
    query: &str,
    hook: &str,
) -> Result<(Vec<search::Hit>, rerank::Ranked, Place)> {
    let place = state::locate(root, &cwd(p), &Hints::default(), state)?;
    let facts = Facts::gather(root, &place, &[]);
    let settings = rerank::Settings::load(root);
    let opts = Options { all: false, every_status: false, limit: settings.top.max(1), probes: rkb_core::search::ProbeMode::Cached };
    let mut found = search::search(root, &place, &facts, &Mode::Ranked(query.to_string()), &opts)?;
    let timeout = cfg.get("hook_timeout_ms").and_then(toml::Value::as_integer).map_or(DEFAULT_HOOK_TIMEOUT_MS, |v| v.max(0) as u64);
    let start = std::time::Instant::now();
    let ranked = crate::rerankers::run(root, &settings, None, query, &found.hits, std::time::Duration::from_millis(timeout))?;
    if settings.chain.iter().any(|b| b != rerank::BM25) && !found.hits.is_empty() {
        hooks::log_ranking(state, hook, &ranked, start.elapsed().as_millis() as u64);
    }
    if let Some(scores) = &ranked.scores {
        search::apply_relevance(&mut found.hits, scores, ranked.order.as_deref());
    }
    Ok((found.hits, ranked, place))
}

fn prompt(p: &Value, state: &Path, session: &str) -> Result<Option<String>> {
    let text = hooks::typed_text(p["prompt"].as_str().unwrap_or(""));
    let text = text.as_str();
    let (correction, remember) = hooks::prompt_signals(text);
    if correction {
        hooks::append(state, session, &json!({ "kind": "correction" }))?;
    }
    if remember {
        hooks::append(state, session, &json!({ "kind": "remember" }))?;
    }
    recall(p, state, session, text)
}

/// Adds the top lesson when a model rated it high and clearly above the next one.
fn recall(p: &Value, state: &Path, session: &str, text: &str) -> Result<Option<String>> {
    if text.trim_start().starts_with('/') || text.split_whitespace().count() < 3 {
        return Ok(None);
    }
    let root = kb::home();
    kb::open(&root)?;
    let cfg = hooks_config(&root);
    if cfg.get("recall").and_then(toml::Value::as_bool) == Some(false) {
        return Ok(None);
    }
    let (hits, ranked, place) = ranked_search(&root, p, state, &cfg, text, "recall")?;
    let (min_default, margin_default) = hooks::recall_defaults(&ranked.backend);
    let min = cfg.get("recall_min_relevance").and_then(toml::Value::as_float).unwrap_or(min_default);
    let margin = cfg.get("recall_min_margin").and_then(toml::Value::as_float).unwrap_or(margin_default);
    // A BM25 score means different things for different queries, so without a model there is no threshold.
    if ranked.scores.is_none() {
        return Ok(None);
    }
    let score = |h: Option<&search::Hit>| h.and_then(|h| h.relevance).map_or(0.0, f64::from);
    let (top, next) = (score(hits.first()), score(hits.get(1)));
    let seen = hooks::read(state, session);
    let strong: Vec<search::Hit> = hits
        .into_iter()
        .take(1)
        .filter(|h| hooks::clear_winner(top, next, min, margin) && h.applies != Verdict::No && h.status == rkb_core::lesson::Status::Active)
        .filter(|h| !seen.iter().any(|r| r["kind"] == "injected" && r["id"] == h.id.as_str()))
        .collect();
    if strong.is_empty() {
        return Ok(None);
    }
    let mut blocks = vec![];
    for h in &strong {
        hooks::append(state, session, &json!({ "kind": "injected", "id": h.id, "hook": "recall", "time": rkb_core::request::now() }))?;
        record_use(&root, state, &h.id, "injected", session, Some(format!("recall, {}", ranked.describe())))?;
        let text = kb::find(&root, &h.id).map(|(_, t)| t).unwrap_or_default();
        blocks.push(injected(h, missing_note(&place, &h.path, &text)));
    }
    Ok(Some(reply("UserPromptSubmit", "additionalContext", json!(hooks::render(&blocks)))))
}

fn stop(p: &Value, state: &Path, session: &str, harness: &str) -> Result<Option<String>> {
    if p["stop_hook_active"] == true {
        return Ok(None);
    }
    let root = kb::home();
    let cfg = hooks_config(&root);
    if cfg.get("stop_nudge").and_then(toml::Value::as_bool) == Some(false) {
        return Ok(None);
    }
    let records = hooks::read(state, session);
    let from = records.iter().rev().find_map(|r| (r["kind"] == "offered").then(|| r["upto"].as_u64()).flatten()).unwrap_or(0);
    let new: Vec<Value> =
        records.iter().skip(from as usize).filter(|r| hooks::WORTH_KINDS.iter().any(|k| r["kind"] == *k)).cloned().collect();
    let score = hooks::score(&new, &records);
    if score == 0 {
        return Ok(None);
    }
    let threshold = cfg.get("record_score").and_then(toml::Value::as_integer).map_or(DEFAULT_RECORD_SCORE, |v| v.max(1) as u32);
    let asked = records.iter().any(|r| r["kind"] == "asked");
    let nudged = records.iter().filter(|r| r["kind"] == "nudged").count();
    // Only the user's own words reach the stop. In the agent A/B (scripts/agent-ab, 2026-09-29) even a
    // non-blocking note on task signals cost turns and added no passes; transcript capture and the
    // observer still take those sessions to the inbox.
    if !new.iter().any(|r| r["kind"] == "remember" || r["kind"] == "correction") {
        return Ok(None);
    }
    let strong = score >= threshold && !asked;
    if !strong && nudged >= MAX_NUDGES {
        return Ok(None);
    }
    let count = |k: &str| new.iter().filter(|r| r["kind"] == k).count();
    let mut programs: Vec<&str> = new.iter().filter(|r| r["kind"] == "fixed").filter_map(|r| r["program"].as_str()).collect();
    programs.sort_unstable();
    programs.dedup();
    let mut parts = vec![];
    if !programs.is_empty() {
        parts.push(format!("{} fixed failure(s) ({})", count("fixed"), programs.join(", ")));
    }
    for (kind, what) in [
        ("correction", "correction(s) from the user"),
        ("remember", "request(s) to remember something"),
        ("repeat", "error(s) seen in an earlier session"),
        ("nohit", "failure(s) no lesson covered"),
    ] {
        if count(kind) > 0 {
            parts.push(format!("{} {what}", count(kind)));
        }
    }
    let ask = "Search first (rkb_search); if a lesson covers it, report rkb_used or improve it with rkb_edit; otherwise record it with rkb_add (verified_how: ran when you saw the fix work), or rkb_note with a priority when you are not sure. Keep the real error text and the command that fixed it.";
    hooks::append(state, session, &json!({ "kind": "offered", "upto": records.len() }))?;
    if strong {
        hooks::append(state, session, &json!({ "kind": "asked" }))?;
        let reason = format!(
            "rkb: this session looks worth a lesson ({}). Before you stop, record what would help next time. {ask} If nothing here would help next time, say so and stop.",
            parts.join(", ")
        );
        // Every harness continues on a blocking reason; the pi and omp extensions map it to a continuation.
        let _ = harness;
        return Ok(Some(json!({ "decision": "block", "reason": reason }).to_string()));
    }
    hooks::append(state, session, &json!({ "kind": "nudged" }))?;
    let context = format!(
        "rkb: this session had {}. If one taught something durable, record it. {ask} Otherwise ignore this note.",
        parts.join(", ")
    );
    Ok(Some(reply("Stop", "additionalContext", json!(context))))
}
