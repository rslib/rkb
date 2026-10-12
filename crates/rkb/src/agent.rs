//! The agent tools: `rkb tool`, `rkb tools`, and the same runs for `rkb mcp` and the pi/omp extension.

use rkb_core::tools;
use serde_json::{Value, json};

use crate::output::{self, CliError, ErrorCode, Format, Output};
use crate::{Cmd, run};

/// How `rkb tool` prints a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// As the matching command prints with `--toon`.
    Toon,
    /// As the matching command prints with `--format json`.
    Json,
    /// `{"text": <TOON>, "json": <JSON>}` from one run, for a harness that needs both.
    Both,
}

/// Runs one tool and returns its TOON text and exit code.
pub fn call(name: &str, args: &Value) -> (String, u8) {
    call_as(name, args, Form::Toon)
}

/// Runs one tool once and returns its result in `form` and the exit code.
pub fn call_as(name: &str, args: &Value, form: Form) -> (String, u8) {
    let result = tools::check(name, args).map_err(|m| CliError::new(ErrorCode::Usage, m, fix(name))).and_then(|()| run_tool(name, args));
    let (toon, data, exit) = match result {
        Ok(out) => (output::render(Format::Toon, &out), out.data.clone(), out.exit),
        Err(e) => (output::render_error_toon(&e), output::error_record(&e), if e.code == ErrorCode::Usage { 2 } else { 1 }),
    };
    let text = match form {
        Form::Toon => toon,
        Form::Json => data.to_string(),
        Form::Both => json!({ "text": toon, "json": data }).to_string(),
    };
    (text, exit)
}

fn fix(name: &str) -> String {
    format!("run `rkb tools --toon` to see the arguments of {name}")
}

fn agent_sink_fix() -> String {
    "fix [sinks.agent] in the machine config.toml (it must be a table with an allow map of known label keys), or remove that table"
        .to_string()
}

/// The `[sinks.agent]` in force, or an error with a fix when the machine config fails closed.
fn resolve_sink() -> Result<Option<rkb_core::config::Sink>, CliError> {
    let (sink, _) = rkb_core::config::agent_sink(&rkb_core::kb::home(), &rkb_core::paths::config_dir())
        .map_err(|m| CliError::new(ErrorCode::Refused, m, agent_sink_fix()))?;
    Ok(sink)
}

fn snapshot(root: &std::path::Path) -> Result<rkb_core::kb::Snapshot, CliError> {
    rkb_core::kb::Snapshot::from_dir(root)
        .map_err(|e| CliError::new(ErrorCode::Refused, format!("the labels could not be read: {e}"), agent_sink_fix()))
}

fn allows(sink: &rkb_core::config::Sink, snap: &rkb_core::kb::Snapshot, path: &str) -> bool {
    rkb_core::config::sink_allows(&sink.allow, &rkb_core::write::effective_at(snap, path))
}

/// Agent search with the sink applied before the limit, so a limit of 5 still returns up to 5
/// allowed lessons. One search asks for the limit plus every lesson the sink hides, which is
/// enough to fill it, so the reranker runs once. Hidden lessons leave no id, title or count behind.
fn agent_search(sink: &rkb_core::config::Sink, args: &Value, limit: usize) -> Result<Output, CliError> {
    use rkb_core::matching::Hints;
    let root = rkb_core::kb::home();
    let snap = snapshot(&root)?;
    let hidden = snap.lessons().0.iter().filter(|l| !allows(sink, &snap, &l.path)).count();
    let cmd = Cmd::Search {
        query: Some(text(args, "query")),
        literal: None,
        regex: None,
        all: args["all"].as_bool().unwrap_or(false),
        status: None,
        tier: args["tier"].as_str().map(String::from),
        limit: limit + hidden,
        rerank: None,
        no_model: false,
    };
    let mut out = run(Some(cmd), Format::Toon, &Hints::default(), &[])?;
    let rows = out.data["results"].as_array().cloned().unwrap_or_default();
    let kept: Vec<Value> = rows.into_iter().filter(|r| r["path"].as_str().is_some_and(|p| allows(sink, &snap, p))).take(limit).collect();
    out.data["results"] = Value::Array(kept);
    // The count of lessons that do not apply here would count hidden ones too.
    if let Some(o) = out.data.as_object_mut() {
        o.remove("hidden");
        o.remove("hidden_by");
    }
    Ok(out)
}

/// Agent show with the sink applied: a hidden lesson reads exactly like a missing one, and an
/// allowed lesson comes back without the related lessons the sink hides.
fn agent_show(sink: &rkb_core::config::Sink, id: &str) -> Result<Output, CliError> {
    use rkb_core::matching::Hints;
    let refused = || {
        CliError::new(
            ErrorCode::Refused,
            format!("lesson {id} is not available to agents on this machine"),
            "search for another lesson; only the user can read this one",
        )
    };
    let root = rkb_core::kb::home();
    let snap = snapshot(&root)?;
    let (lesson, _) = rkb_core::kb::find(&root, id).map_err(|_| refused())?;
    if !allows(sink, &snap, &lesson.path) {
        return Err(refused());
    }
    let mut out = run(Some(Cmd::Show { id: id.to_string(), why: false }), Format::Toon, &Hints::default(), &[])?;
    if let Some(related) = out.data.get("related").and_then(|r| r.as_array()).cloned() {
        let kept: Vec<Value> = related.into_iter().filter(|r| r["path"].as_str().is_some_and(|p| allows(sink, &snap, p))).collect();
        out.data["related"] = Value::Array(kept);
    }
    Ok(out)
}

fn strings(args: &Value, key: &str) -> Vec<String> {
    args[key].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect()
}

fn text(args: &Value, key: &str) -> String {
    args[key].as_str().unwrap_or_default().to_string()
}

fn run_tool(name: &str, args: &Value) -> Result<Output, CliError> {
    let cmd = match name {
        "rkb_search" => {
            let limit = args["limit"].as_u64().map_or(10, |n| n as usize);
            if let Some(sink) = resolve_sink()? {
                return agent_search(&sink, args, limit);
            }
            Cmd::Search {
                query: Some(text(args, "query")),
                literal: None,
                regex: None,
                all: args["all"].as_bool().unwrap_or(false),
                status: None,
                tier: args["tier"].as_str().map(String::from),
                limit,
                rerank: None,
                no_model: false,
            }
        }
        "rkb_show" => {
            let id = text(args, "id");
            if let Some(sink) = resolve_sink()? {
                return agent_show(&sink, &id);
            }
            Cmd::Show { id, why: false }
        }
        "rkb_note" => Cmd::Note { words: vec![text(args, "text")], priority: args["priority"].as_u64().map(|p| p.clamp(1, 3) as u8) },
        "rkb_add" => {
            let (topic, lesson) = tools::lesson(args).map_err(|m| CliError::new(ErrorCode::Usage, m, fix(name)))?;
            Cmd::Add {
                topic: Some(topic),
                kind: None,
                template: false,
                text: Some(lesson),
                assets: strings(args, "assets"),
                from_inbox: None,
            }
        }
        "rkb_used" => Cmd::Used {
            id: text(args, "id"),
            worked: args["result"] == "worked",
            failed: args["result"] == "failed",
            irrelevant: args["result"] == "irrelevant",
            reason: args["reason"].as_str().map(String::from),
            session: None,
        },
        "rkb_flag" => Cmd::Flag { id: text(args, "id"), reason: text(args, "reason") },
        "rkb_edit" => Cmd::Edit {
            id: text(args, "id"),
            base: Some(text(args, "base")),
            text: Some(text(args, "text")),
            assets: strings(args, "assets"),
        },
        other => return Err(CliError::new(ErrorCode::Usage, format!("unknown tool `{other}`"), fix(other))),
    };
    run(Some(cmd), Format::Toon, &rkb_core::matching::Hints::default(), &[])
}

/// `rkb tools`: the tool definitions.
pub fn list() -> Output {
    let defs = tools::definitions();
    let human = defs
        .iter()
        .map(|d| format!("{}  {}", d["name"].as_str().unwrap_or(""), d["description"].as_str().unwrap_or("")))
        .collect::<Vec<_>>()
        .join("\n\n");
    Output { data: json!({ "tools": defs, "namespace": tools::namespace() }), human, exit: 0, raw: false }
}

/// `rkb tool <name>`: the arguments come as one JSON object on stdin.
pub fn from_stdin(name: &str, form: Form) -> (String, u8) {
    let usage = |e: CliError| match form {
        Form::Toon => output::render_error_toon(&e),
        Form::Json => output::error_record(&e).to_string(),
        Form::Both => json!({ "text": output::render_error_toon(&e), "json": output::error_record(&e) }).to_string(),
    };
    let mut input = String::new();
    if let Err(e) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input) {
        return (usage(CliError::new(ErrorCode::Usage, format!("cannot read stdin: {e}"), fix(name))), 2);
    }
    let args: Value = if input.trim().is_empty() {
        json!({})
    } else {
        match serde_json::from_str(&input) {
            Ok(v) => v,
            Err(e) => {
                return (usage(CliError::new(ErrorCode::Usage, format!("the arguments are not JSON: {e}"), fix(name))), 2);
            }
        }
    };
    call_as(name, &args, form)
}
