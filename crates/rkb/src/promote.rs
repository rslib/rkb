//! `rkb promote`: one model call per repeated inbox item, then the normal write path.

use std::time::Duration;

use rkb_core::conditions::Facts;
use rkb_core::observer::{self, Ran};
use rkb_core::promote;
use rkb_core::search::{self, Mode, Options};
use rkb_core::{distill, write};
use serde_json::json;

use crate::output::{CliError, ErrorCode, Output};
use crate::writes::{self, Env};

/// The 5 closest active lessons to `text`, as `id: title`.
fn closest(env: &Env, text: &str) -> Vec<String> {
    let place = env.place.clone().unwrap_or(rkb_core::matching::Place { project: None, system: None, repo: None, dir: None });
    let facts = Facts::gather(&env.root, &place, &[]);
    let options = Options { all: false, every_status: false, limit: 5, probes: search::ProbeMode::Cached };
    match search::search(&env.root, &place, &facts, &Mode::Ranked(text.to_string()), &options) {
        Ok(found) => found.hits.iter().take(5).map(|h| format!("{}: {}", h.id, h.title)).collect(),
        Err(_) => vec![],
    }
}

/// The `base` hash of the current lesson, so `rkb edit` accepts the promotion's text.
fn lesson_base(env: &Env, id: &str) -> Option<String> {
    let (_, text) = rkb_core::kb::find(&env.root, id).ok()?;
    Some(write::content_hash(text.as_bytes()))
}

/// The lesson file the write path needs. The model may return only the body, so rkb adds the
/// frontmatter the parser requires; a text that already has frontmatter is left alone.
fn lesson_text(answer: &promote::Answer) -> String {
    let kind_name = answer.json["type"].as_str().unwrap_or("pitfall");
    let kind = rkb_core::lesson::LessonType::from_name(kind_name).unwrap_or(rkb_core::lesson::LessonType::Pitfall);
    let type_name = match kind {
        rkb_core::lesson::LessonType::Pitfall => "pitfall",
        rkb_core::lesson::LessonType::Recipe => "recipe",
        rkb_core::lesson::LessonType::Fact => "fact",
        rkb_core::lesson::LessonType::Decision => "decision",
        rkb_core::lesson::LessonType::Preference => "preference",
    };
    let mut fm = format!("---\ntype: {type_name}\nverified_how: read\n");
    if let Some(queries) = answer.json["queries"].as_array().filter(|q| !q.is_empty()) {
        fm.push_str("queries:\n");
        for q in queries.iter().filter_map(|v| v.as_str()) {
            // Double-quoted so the text is stored exactly as the model wrote it.
            fm.push_str(&format!("  - \"{}\"\n", q.replace('\\', "\\\\").replace('"', "\\\"")));
        }
    }
    fm.push_str("---\n\n");
    // The model gives the sections, so rkb writes the headings the lint requires, in order.
    if answer.json["sections"].as_object().is_some_and(|m| !m.is_empty()) {
        let title = answer.json["title"].as_str().unwrap_or("Promoted lesson");
        fm.push_str(&format!("# {title}\n"));
        for heading in kind.required_headings() {
            let body = answer.json["sections"][heading].as_str().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("Not known yet.");
            fm.push_str(&format!("\n## {heading}\n\n{body}\n"));
        }
        return fm;
    }
    // A full file, or a body with its own H1, passes through.
    let text = answer.text().unwrap_or_default().trim().to_string();
    if text.starts_with("---") {
        return text;
    }
    if text.lines().any(|l| l.starts_with("# ")) {
        return format!("{fm}{text}");
    }
    let title = answer.json["title"].as_str().unwrap_or("Promoted lesson");
    format!("{fm}# {title}\n\n{text}")
}

/// Promotes up to `max` repeated items with one model call each. `max` defaults to
/// `[promote] max_per_pass` (3). One pass runs at a time, and one item stands for each signature.
pub fn run(env: &Env, max: Option<usize>, harness: &str) -> Result<Output, CliError> {
    let config = rkb_core::paths::config_dir();
    let Some(cfg) = observer::ready(&config, harness) else {
        let human = "no approved model chain; nothing promoted".to_string();
        return Ok(Output { data: json!({ "promoted": 0, "reason": "no approved model chain" }), human, exit: 0, raw: false });
    };
    // Two session ends can overlap, so one pass at a time.
    let Ok(_guard) = rkb_core::lock::acquire(&env.state.join("promote.lock"), 3600, Duration::ZERO) else {
        let human = "another promotion pass is running; nothing promoted".to_string();
        return Ok(Output { data: json!({ "promoted": 0, "reason": "another promotion pass is running" }), human, exit: 0, raw: false });
    };
    let items = promote::ready_items(&env.state);
    // One item per signature: the others are the same failure, and one lesson covers them all.
    let reps = promote::representatives(&items);
    let cap = max.unwrap_or_else(|| promote::max_per_pass(&config));
    let (mut promoted, mut left) = (0usize, 0usize);
    let mut results = vec![];
    for item in reps.iter().take(cap) {
        // The duplicates go before the model call, so a failed write leaves none behind.
        if !item.meta.signatures.is_empty() {
            distill::remove_signatures(&env.state, &item.meta.signatures, &item.id);
        }
        let lessons = closest(env, &item.body);
        let prompt = promote::prompt(item, &lessons);
        let model = cfg.chain(harness).first().cloned().unwrap_or_else(|| "session".into());
        let (program, args) = observer::command(harness, Some(&model));
        let scratch = env.state.join("promote");
        let ran = observer::run(&program, &args, &prompt, Duration::from_secs(cfg.timeout_secs), &scratch)
            .map_err(|e| CliError::new(ErrorCode::Refused, e.to_string(), "check the model chain and the harness command"))?;
        let (text, (input, output)) = match ran {
            Ran::Ok { text, usage } => (text, usage.unwrap_or((0, 0))),
            Ran::Failed(reason) => {
                promote::log_call(&env.state, &item.id, &model, 0, 0, "failed");
                distill::remove(&env.state, &item.id);
                results.push(json!({ "id": item.id, "action": "failed", "reason": reason }));
                continue;
            }
            Ran::Timeout => {
                promote::log_call(&env.state, &item.id, &model, 0, 0, "timeout");
                distill::remove(&env.state, &item.id);
                results.push(json!({ "id": item.id, "action": "timeout" }));
                continue;
            }
        };
        let answer = match promote::parse(&text) {
            Ok(answer) => answer,
            Err(e) => {
                promote::log_call(&env.state, &item.id, &model, input, output, "noop");
                distill::remove(&env.state, &item.id);
                results.push(json!({ "id": item.id, "action": "unparsed", "reason": e.to_string() }));
                continue;
            }
        };
        if !promote::quotes_match(&answer, item) {
            promote::log_call(&env.state, &item.id, &model, input, output, "noop");
            distill::remove(&env.state, &item.id);
            results.push(json!({ "id": item.id, "action": "noop", "reason": "the quote is not in the item", "quotes": answer.quotes() }));
            continue;
        }
        let written = match answer.action.as_str() {
            "add" => writes::add(
                env,
                answer.json["topic"].as_str().map(str::to_string),
                answer.json["type"].as_str().map(str::to_string),
                false,
                Some(lesson_text(&answer)),
                vec![],
                Some(item.id.clone()),
            ),
            "update" => {
                let id = answer.id().unwrap_or_default().to_string();
                let base = lesson_base(env, &id);
                writes::edit(env, id, base, vec![], Some(lesson_text(&answer)))
            }
            // The model read the lesson; no session used it, so the record is inferred, not worked.
            "used" => writes::used(
                env,
                answer.id().unwrap_or_default().to_string(),
                "inferred",
                Some("promoted from a repeated inbox item".into()),
                None,
            ),
            _ => Ok(Output { data: json!({ "status": "noop" }), human: "noop".into(), exit: 0, raw: false }),
        };
        let written = match written {
            Ok(written) => written,
            Err(e) => {
                // The item goes, or every session end would pay for the same failed write again.
                promote::log_call(&env.state, &item.id, &model, input, output, "failed");
                distill::remove(&env.state, &item.id);
                results.push(json!({ "id": item.id, "action": "failed", "reason": e.message }));
                continue;
            }
        };
        promote::log_call(&env.state, &item.id, &model, input, output, &answer.action);
        if written.exit == 3 {
            // The request holds the write, so the item goes and no pass pays for the same call again.
            for sig in &item.meta.signatures {
                distill::mark_done(&env.state, sig);
            }
            distill::remove(&env.state, &item.id);
            results.push(json!({ "id": item.id, "action": answer.action, "needs_user": true }));
            left += 1;
            break;
        }
        // A lesson now covers these signatures, so a third session does not promote them again.
        for sig in &item.meta.signatures {
            distill::mark_done(&env.state, sig);
        }
        distill::remove(&env.state, &item.id);
        promoted += 1;
        results.push(json!({ "id": item.id, "action": answer.action }));
    }
    let (accepted, total_tokens) = promote::cost(&env.state);
    let human = format!(
        "promoted {promoted} of {} repeated items ({left} left for the user); {accepted} accepted lessons cost {total_tokens} tokens",
        reps.len()
    );
    Ok(Output {
        data: json!({ "promoted": promoted, "left": left, "ready": reps.len(), "results": results, "accepted": accepted, "tokens": total_tokens }),
        human,
        exit: 0,
        raw: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rkb_core::promote::Answer;

    fn answer(json: serde_json::Value) -> Answer {
        Answer { action: json["action"].as_str().unwrap_or_default().to_string(), json }
    }

    #[test]
    fn a_lesson_keeps_its_queries_exactly_and_is_marked_read() {
        let a = answer(json!({
            "action": "add",
            "type": "pitfall",
            "title": "cmake cannot find HDF5",
            "queries": ["cmake error: could not find HDF5", "HDF5_ROOT: the fix"],
            "sections": {"Symptom": "it fails", "Cause": "the prefix", "Fix": "set HDF5_ROOT", "Evidence": "it worked"}
        }));
        let text = lesson_text(&a);
        assert!(text.contains("verified_how: read"), "a model only read the text: {text}");
        assert!(text.contains("- \"cmake error: could not find HDF5\""), "a colon must survive: {text}");
        assert!(text.contains("- \"HDF5_ROOT: the fix\""), "{text}");
        assert!(text.contains("# cmake cannot find HDF5\n"), "{text}");
        for heading in ["## Symptom", "## Cause", "## Fix", "## Evidence"] {
            assert!(text.contains(heading), "{heading} missing: {text}");
        }
    }

    #[test]
    fn an_empty_section_gets_a_placeholder_and_the_headings_stay_in_order() {
        let a = answer(json!({
            "action": "add",
            "type": "fact",
            "title": "T",
            "queries": [],
            "sections": {"Statement": "X."}
        }));
        let text = lesson_text(&a);
        assert!(text.contains("## Statement\n\nX."), "{text}");
        assert!(text.contains("## Evidence\n\nNot known yet."), "{text}");
        assert!(text.find("## Statement").unwrap() < text.find("## Evidence").unwrap(), "{text}");
    }
}
