use std::io::{IsTerminal, Read};

use rkb_core::distill::{self, Item, Kind, Meta};
use rkb_core::request;
use serde_json::{Value, json};

use crate::list::{cut, marks, width};
use crate::output::{CliError, ErrorCode, Output, paint};
use crate::writes::Env;

pub fn age(secs: u64) -> String {
    match secs {
        s if s < 3600 => format!("{} min", s / 60),
        s if s < 2 * 86400 => format!("{} h", s / 3600),
        s => format!("{} d", s / 86400),
    }
}

fn row(i: &Item, now: u64) -> Value {
    json!({
        "id": i.id,
        "kind": i.meta.kind.as_str(),
        "priority": i.meta.priority(),
        "age": age(now.saturating_sub(i.meta.time)),
        "lines": i.body.lines().count(),
        "preview": i.preview(),
    })
}

pub fn note(env: &Env, words: Vec<String>, priority: Option<u8>) -> Result<Output, CliError> {
    let mut text = words.join(" ");
    if text.trim().is_empty() && !std::io::stdin().is_terminal() {
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| CliError::new(ErrorCode::Usage, format!("cannot read stdin: {e}"), "pass the note as an argument"))?;
    }
    if text.trim().is_empty() {
        return Err(CliError::new(ErrorCode::Usage, "the note is empty", "run `rkb note \"<what you found>\"`"));
    }
    let meta = Meta {
        kind: Kind::Note,
        time: request::now(),
        harness: None,
        session: rkb_core::usage::env_session(),
        cwd: None,
        signals: vec![],
        priority: priority.map(|p| p.clamp(1, 3)),
        source: None,
    };
    let item = distill::add(&env.state, meta, &text)?;
    let human = format!("noted {} in the inbox; `/rkb-distill` turns inbox items into lessons", item.id);
    Ok(Output { data: json!({ "id": item.id, "help": ["Run `rkb inbox` to list the inbox"] }), human, exit: 0, raw: false })
}

pub fn list(env: &Env) -> Result<Output, CliError> {
    let now = request::now();
    let expired = distill::expire(&env.state, now);
    let items = distill::list(&env.state);
    let (c, m, w) = (env.colored, marks(), width());
    let mut human = format!("{} {} {} items\n", paint(c, "1", "inbox"), m.sep, items.len());
    for i in &items {
        let head = format!("{} {} {} {} lines", i.meta.kind.as_str(), m.sep, age(now.saturating_sub(i.meta.time)), i.body.lines().count());
        human.push_str(&format!(
            "\n  {} {}\n    {}\n",
            paint(c, "36", &i.id),
            paint(c, "2", &head),
            cut(&i.preview(), w.saturating_sub(4), m)
        ));
    }
    if expired > 0 {
        human.push_str(&format!("\n{expired} items older than {} days expired\n", distill::EXPIRY_DAYS));
    }
    let help = if items.is_empty() {
        "The inbox is empty"
    } else {
        "Run `/rkb-distill`, or `rkb inbox show <id>`, write lessons with `rkb add`, then `rkb inbox done <id>`"
    };
    let rows: Vec<Value> = items.iter().map(|i| row(i, now)).collect();
    Ok(Output { data: json!({ "items": rows, "expired": expired, "help": [help] }), human, exit: 0, raw: false })
}

pub fn show(env: &Env, id: &str) -> Result<Output, CliError> {
    let item = distill::get(&env.state, id)?;
    let mut data = row(&item, request::now());
    data["meta"] = serde_json::to_value(&item.meta).unwrap_or(Value::Null);
    data["body"] = json!(item.body);
    Ok(Output { data, human: item.text(), exit: 0, raw: false })
}

pub fn done(env: &Env, ids: &[String]) -> Result<Output, CliError> {
    let mut human = String::new();
    let rows: Vec<Value> = ids
        .iter()
        .map(|id| {
            let removed = distill::remove(&env.state, id);
            human.push_str(&format!("{id}: {}\n", if removed { "done" } else { "no such inbox item" }));
            json!({ "id": id, "removed": removed })
        })
        .collect();
    let exit = u8::from(rows.iter().any(|r| r["removed"] == false));
    Ok(Output { data: json!({ "items": rows }), human: human.trim_end().to_string(), exit, raw: false })
}
