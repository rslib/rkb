//! `rkb promote`: one model call per repeated inbox item, then the normal write path.

use std::time::Duration;

use rkb_core::conditions::Facts;
use rkb_core::observer::{self, Ran};
use rkb_core::promote;
use rkb_core::request::{self, Action};
use rkb_core::search::{self, Mode, Options};
use rkb_core::{distill, write};
use serde_json::json;

use crate::output::{CliError, ErrorCode, Output};
use crate::writes::{self, Env};

/// A lesson close to the item, with the status that marks a tombstone.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Closest {
    pub id: String,
    pub title: String,
    pub status: rkb_core::lesson::Status,
}

impl Closest {
    /// How the promotion prompt shows it: a tombstone is marked, so the model does not propose the
    /// same claim again.
    fn label(&self) -> String {
        if self.status.is_current() {
            format!("{}: {}", self.id, self.title)
        } else {
            format!("{}: {} ({} tombstone)", self.id, self.title, self.status.as_str())
        }
    }
}

/// The 5 closest lessons to `text`, tombstones included, as `id: title`.
fn closest(env: &Env, text: &str) -> Vec<Closest> {
    let place = env.place.clone().unwrap_or(rkb_core::matching::Place { project: None, system: None, repo: None, dir: None });
    let facts = Facts::gather(&env.root, &place, &[]);
    // `every_status` so an archived lesson is in the prompt and can block the same claim again.
    let options = Options { all: false, every_status: true, limit: 5, probes: search::ProbeMode::Cached, tier: None };
    match search::search(&env.root, &place, &facts, &Mode::Ranked(text.to_string()), &options) {
        Ok(found) => found.hits.iter().take(5).map(|h| Closest { id: h.id.clone(), title: h.title.clone(), status: h.status }).collect(),
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

/// Promotes up to `max` repeated items with one model call each, under the upkeep lock. `max` defaults
/// to `[promote] max_per_pass` (3). `rkb maintain` calls `run_locked` instead, because it holds that lock.
pub fn run(env: &Env, max: Option<usize>, harness: &str) -> Result<Output, CliError> {
    let Ok(_guard) = rkb_core::lock::acquire(&rkb_core::maintain::lock_path(&env.state), 3600, Duration::ZERO) else {
        return Ok(busy());
    };
    run_locked(env, max, harness, false, || true)
}

/// A second pass that found the upkeep lock held: exit 0, no step run.
fn busy() -> Output {
    let human = format!("{}; nothing promoted", rkb_core::maintain::BUSY);
    Output { data: json!({ "promoted": 0, "reason": rkb_core::maintain::BUSY }), human, exit: 0, raw: false }
}

/// Promotes up to `max` repeated items with one model call each. With `stop_on_failure` the model step
/// ends on the first failed or timed-out run and leaves its item for a later pass. `next_call` says
/// whether the daily cap still allows a call and counts it when it does.
pub fn run_locked<F: FnMut() -> bool>(
    env: &Env,
    max: Option<usize>,
    harness: &str,
    stop_on_failure: bool,
    mut next_call: F,
) -> Result<Output, CliError> {
    let config = rkb_core::paths::config_dir();
    let Some(cfg) = observer::ready(&config, harness) else {
        let human = "no approved model chain; nothing promoted".to_string();
        return Ok(Output { data: json!({ "promoted": 0, "reason": "no approved model chain" }), human, exit: 0, raw: false });
    };
    let items = promote::ready_items(&env.state);
    // One item per signature: the others are the same failure, and one lesson covers them all.
    let reps = promote::representatives(&items);
    let cap = max.unwrap_or_else(|| promote::max_per_pass(&config));
    let (mut promoted, mut left) = (0usize, 0usize);
    let mut skip: Option<String> = None;
    let mut results = vec![];
    for item in reps.iter().take(cap) {
        if !next_call() {
            skip = Some("the daily cap is reached".to_string());
            break;
        }
        // The duplicates go before the model call, so a failed write leaves none behind.
        if !item.meta.signatures.is_empty() {
            distill::remove_signatures(&env.state, &item.meta.signatures, &item.id);
        }
        let matches = closest(env, &item.body);
        let lessons: Vec<String> = matches.iter().map(Closest::label).collect();
        let prompt = promote::prompt(item, &lessons);
        let model = cfg.chain(harness).first().cloned().unwrap_or_else(|| "session".into());
        let (program, args) = observer::command(harness, Some(&model));
        let scratch = env.state.join("promote");
        let ran = observer::run(&program, &args, &prompt, Duration::from_secs(cfg.timeout_secs), &scratch)
            .map_err(|e| CliError::new(ErrorCode::Refused, e.to_string(), "check the model chain and the harness command"))?;
        let (text, (input, output)) = match ran {
            Ran::Ok { text, usage } => (text, usage.unwrap_or((0, 0))),
            Ran::Failed(reason) => {
                promote::log_call(&env.state, &item.id, &model, 0, 0, "failed", None);
                if stop_on_failure {
                    skip = Some(format!("the model program failed: {reason}"));
                    results.push(json!({ "id": item.id, "action": "failed", "reason": reason }));
                    break;
                }
                distill::remove(&env.state, &item.id);
                results.push(json!({ "id": item.id, "action": "failed", "reason": reason }));
                continue;
            }
            Ran::Timeout => {
                promote::log_call(&env.state, &item.id, &model, 0, 0, "timeout", None);
                if stop_on_failure {
                    skip = Some("the model program timed out".to_string());
                    results.push(json!({ "id": item.id, "action": "timeout" }));
                    break;
                }
                distill::remove(&env.state, &item.id);
                results.push(json!({ "id": item.id, "action": "timeout" }));
                continue;
            }
        };
        let answer = match promote::parse(&text) {
            Ok(answer) => answer,
            Err(e) => {
                promote::log_call(&env.state, &item.id, &model, input, output, "noop", None);
                distill::remove(&env.state, &item.id);
                results.push(json!({ "id": item.id, "action": "unparsed", "reason": e.to_string() }));
                continue;
            }
        };
        if !promote::quotes_match(&answer, item) {
            // The item stays in the inbox. One later pass may retry it; after a second miss its
            // signatures close, so a stuck item stops paying for a call and taking a slot each pass.
            if promote::quote_failures(&env.state, &item.id) >= 1 {
                for sig in &item.meta.signatures {
                    distill::mark_done(&env.state, sig);
                }
            }
            promote::log_call(&env.state, &item.id, &model, input, output, "noop", Some(promote::BAD_QUOTE));
            results.push(json!({ "id": item.id, "action": "noop", "reason": promote::BAD_QUOTE, "quotes": answer.quotes() }));
            continue;
        }
        if matches!(answer.action.as_str(), "add" | "update") && answer.one_off() {
            let reason = "the preference is a one-off request for this session only, not a standing rule";
            promote::log_call(&env.state, &item.id, &model, input, output, "noop", Some(reason));
            for sig in &item.meta.signatures {
                distill::mark_done(&env.state, sig);
            }
            distill::remove(&env.state, &item.id);
            results.push(json!({ "id": item.id, "action": "noop", "reason": reason }));
            continue;
        }
        if matches!(answer.action.as_str(), "add" | "update") && !answer.changes_behavior() {
            let reason = "the lesson would not change what an agent does in a different session";
            promote::log_call(&env.state, &item.id, &model, input, output, "noop", Some(reason));
            for sig in &item.meta.signatures {
                distill::mark_done(&env.state, sig);
            }
            distill::remove(&env.state, &item.id);
            results.push(json!({ "id": item.id, "action": "noop", "reason": reason }));
            continue;
        }
        // A lesson with a shell command or a URL waits for the user. Transcript text can carry
        // injected instructions, so it must never reach later sessions unseen.
        if matches!(answer.action.as_str(), "add" | "update") {
            let text = lesson_text(&answer);
            if write::holds_commands_or_urls(&text) {
                let held = if answer.action.as_str() == "update" {
                    let id = answer.id().unwrap_or_default().to_string();
                    lesson_base(env, &id).map(|base| Action::Edit { id, text, base, assets: vec![] })
                } else {
                    Some(Action::Add {
                        text,
                        topic: answer.json["topic"].as_str().unwrap_or("general").to_string(),
                        assets: vec![],
                        inbox: Some(item.id.clone()),
                        session: item.meta.session.clone(),
                        quote: provenance_quote(&answer),
                    })
                };
                if let Some(action) = held {
                    let req = request::Request {
                        id: request::new_id(),
                        created: request::now(),
                        action,
                        approved: vec![],
                        question: "The promoted lesson holds a shell command or a URL. Transcript text can carry injected instructions. Write it anyway?".into(),
                        choices: vec![
                            request::Choice { text: "write lesson".into(), decision: Some(request::Decision::AddAnyway) },
                            request::Choice { text: "cancel".into(), decision: None },
                        ],
                        session: rkb_core::usage::env_session(),
                    };
                    request::save(&env.state.join("requests"), &req)
                        .map_err(|e| CliError::new(ErrorCode::Refused, e.to_string(), "check the state directory"))?;
                    promote::log_call(&env.state, &item.id, &model, input, output, &answer.action, None);
                    for sig in &item.meta.signatures {
                        distill::mark_done(&env.state, sig);
                    }
                    distill::remove(&env.state, &item.id);
                    results.push(json!({ "id": item.id, "action": answer.action, "needs_user": true }));
                    left += 1;
                    break;
                }
            }
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
                item.meta.session.clone(),
                provenance_quote(&answer),
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
                promote::log_call(&env.state, &item.id, &model, input, output, "failed", Some(&e.message));
                results.push(json!({ "id": item.id, "action": "failed", "reason": e.message }));
                // A write that fails in the background job is often the job's environment, so the
                // pass keeps the item and stops. By hand the item goes, so no pass pays for it again.
                if stop_on_failure {
                    skip = Some(format!("the lesson write failed: {}", e.message));
                    break;
                }
                distill::remove(&env.state, &item.id);
                continue;
            }
        };
        let lesson = written.data["id"].as_str().map(str::to_string);
        promote::log_call(&env.state, &item.id, &model, input, output, &answer.action, None);
        if matches!(answer.action.as_str(), "add" | "update")
            && let Some(lesson) = &lesson
        {
            promote::log_lesson(&env.state, &item.id, lesson);
        }
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
    let human = match &skip {
        Some(reason) => format!("promoted {promoted} of {} repeated items; skipped the model step: {reason}", reps.len()),
        None => format!("promoted {promoted} of {} repeated items ({left} left for the user)", reps.len()),
    };
    let human = format!("{human}; {accepted} accepted lessons cost {total_tokens} tokens");
    let mut data = json!({ "promoted": promoted, "left": left, "ready": reps.len(), "results": results, "accepted": accepted, "tokens": total_tokens });
    if let Some(reason) = skip {
        data["skip"] = reason.into();
    }
    Ok(Output { data, human, exit: 0, raw: false })
}

/// The first checked quote, kept for `rkb show --why` only when it cannot carry a path, an email
/// or a home folder and fits a short line. Otherwise the leak scan would refuse the whole lesson.
fn provenance_quote(answer: &promote::Answer) -> Option<String> {
    answer.quotes().into_iter().next().filter(|q| q.chars().count() <= 200 && !q.contains(['/', '@', '~']))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rkb_core::promote::Answer;

    fn answer(json: serde_json::Value) -> Answer {
        Answer { action: json["action"].as_str().unwrap_or_default().to_string(), json }
    }

    #[test]
    fn a_quote_with_a_path_is_not_kept() {
        let q = |quote: &str| provenance_quote(&answer(json!({ "action": "add", "quotes": [quote] })));
        assert_eq!(q("could not find HDF5"), Some("could not find HDF5".into()));
        assert_eq!(q("cannot open /home/u/x.h5"), None);
        assert_eq!(q(&"x".repeat(201)), None);
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
    fn a_tombstone_reaches_the_prompt_labelled() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("kb");
        let write = |name: &str, id: &str, status: &str, title: &str| {
            let p = root.join(format!("general/a/{name}.md"));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            let why = if status == "archived" { "\n## Why archived\nRetired.\n" } else { "" };
            let text = format!(
                "---\nschema: 1\nid: {id}\ntype: fact\nstatus: {status}\nverified: 2026-09-25\nverified_how: ran\n---\n\n# {title}\n\n## Statement\nUndefined reference to vtable means a missing virtual definition.\n\n## Evidence\nSeen.{why}"
            );
            std::fs::write(p, text).unwrap();
        };
        write("live", "0000000001", "active", "A live note");
        write("gone", "0000000002", "archived", "Undefined reference to vtable means a missing virtual definition");
        let env = Env {
            root: root.clone(),
            lint: rkb_core::lint::LintEnv { user: None, home: None },
            state: dir.path().join("state"),
            colored: false,
            place: None,
        };
        // The closest lessons come from BM25, so a tombstone reaches the model through the prompt.
        let matches = closest(&env, "undefined reference to vtable");
        let gone = matches.iter().find(|c| c.id == "0000000002").expect("the tombstone is in the closest list");
        assert!(!gone.status.is_current());
        let lessons: Vec<String> = matches.iter().map(Closest::label).collect();
        let item = rkb_core::distill::Item {
            id: "0a1b2c3d4e".into(),
            meta: rkb_core::distill::Meta {
                kind: rkb_core::distill::Kind::Note,
                time: 1,
                harness: None,
                session: None,
                cwd: None,
                signals: vec![],
                signatures: vec![],
                priority: None,
                source: None,
                candidates: vec![],
            },
            body: "undefined reference to vtable for Widget".into(),
        };
        let p = promote::prompt(&item, &lessons);
        assert!(
            p.contains("0000000002: Undefined reference to vtable means a missing virtual definition (archived tombstone)"),
            "the prompt marks the tombstone: {p}"
        );
        assert!(p.contains("marked (archived tombstone) was retired"), "the prompt says what the mark means: {p}");
        let current = Closest { id: "0000000001".into(), title: "A live note".into(), status: rkb_core::lesson::Status::Active };
        assert_eq!(current.label(), "0000000001: A live note", "a live lesson carries no tombstone mark");
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
