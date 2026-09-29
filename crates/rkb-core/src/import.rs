use std::collections::BTreeSet;
use std::path::Path;

use crate::config::{self, KbConfig};
use crate::error::{Error, Result, io};
use crate::kb::Snapshot;
use crate::request::{self, Action, Choice, Decision, ImportItem, Request};
use crate::write::{self, Ctx, Imported, Note, Outcome};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid {
    pub source: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant, reason = "one per import")]
pub enum Plan {
    /// Nothing was written and no request was made.
    Invalid(Vec<Invalid>),
    /// The saved request with the report as its question.
    Ready(Request),
}

fn kb_config(root: &Path) -> KbConfig {
    std::fs::read_to_string(root.join("kb.toml")).ok().and_then(|t| config::parse(&t).ok()).unwrap_or_default()
}

struct Source {
    source: String,
    topic: String,
    text: String,
}

/// Every `<topic>/<name>.md`, sorted by source, and the files that do not fit.
fn sources(dir: &Path) -> Result<(Vec<Source>, Vec<Invalid>)> {
    let mut found = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(io(&d))?.flatten() {
            let path = e.path();
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "md") {
                found.push(path);
            }
        }
    }
    if found.is_empty() {
        return Err(Error::NothingToImport(dir.to_path_buf()));
    }
    found.sort();
    let (mut ok, mut bad) = (vec![], vec![]);
    for path in found {
        let rel = path.strip_prefix(dir).unwrap_or(&path).to_string_lossy().into_owned();
        let parts: Vec<&str> = rel.split('/').collect();
        let reason = match parts.as_slice() {
            [_, "README.md"] => Some("folder notes are not imported; put lessons at <topic>/<name>.md".to_string()),
            [topic, _] => match std::fs::read(&path).map(String::from_utf8) {
                Ok(Ok(text)) => {
                    ok.push(Source { source: rel.clone(), topic: topic.to_string(), text });
                    None
                }
                Ok(Err(_)) => Some("the file is not UTF-8".into()),
                Err(e) => Some(e.to_string()),
            },
            _ => Some("put each lesson at <topic>/<name>.md inside the import folder".into()),
        };
        if let Some(r) = reason {
            bad.push(Invalid { source: rel, reasons: vec![r] });
        }
    }
    Ok((ok, bad))
}

/// Checks every lesson in `dir` as `rkb add` would, and returns the report as a saved request.
/// Writes nothing to the knowledge base.
pub fn plan(ctx: &Ctx, dir: &Path) -> Result<Plan> {
    let kb = kb_config(ctx.root);
    let (files, mut invalid) = sources(dir)?;
    let mut sim = Snapshot::from_dir(ctx.root)?;
    let mut announced: BTreeSet<String> = BTreeSet::new();
    let mut items = vec![];
    for Source { source, topic, text } in files {
        let action = Action::Add { text: text.clone(), topic: topic.clone(), assets: vec![], inbox: None };
        let mut item = ImportItem {
            source: source.clone(),
            topic: topic.clone(),
            text: text.clone(),
            title: String::new(),
            path: String::new(),
            approved: vec![],
            new_folder: None,
            similar: vec![],
            looser: None,
        };
        // Each retry approves one more decision of two kinds, so the loop ends.
        let prepared = loop {
            match write::add(ctx, &kb, &sim, &action, &item.approved, &text, &topic, &[], None) {
                Err(Error::Refused(m)) => break Err(vec![m]),
                Err(e) => return Err(e),
                Ok(Ok(p)) => break Ok(p),
                Ok(Err(Outcome::NeedsUser(req))) => {
                    // Only a likely typo in the topic name or a looser label still asks.
                    let pick = req
                        .choices
                        .iter()
                        .filter_map(|c| c.decision.clone())
                        .find(|d| matches!(d, Decision::CreateFolder { .. } | Decision::Loosen { .. }) && !item.approved.contains(d));
                    match &pick {
                        Some(Decision::CreateFolder { .. }) => {}
                        Some(Decision::Loosen { key, value }) => item.looser = Some(format!("{key}={value}")),
                        _ => break Err(vec![format!("needs a decision an import cannot make: {}", req.question)]),
                    }
                    item.approved.push(pick.unwrap());
                }
                Ok(Err(other)) => break Err(vec![format!("unexpected result: {other:?}")]),
            }
        };
        match prepared {
            Err(reasons) => invalid.push(Invalid { source, reasons }),
            Ok(p) => {
                let errors = write::stage(&mut sim, ctx.env, &p);
                if errors.is_empty() {
                    for n in &p.notes {
                        match n {
                            Note::NewFolder(f) if announced.insert(f.clone()) => item.new_folder = Some(f.clone()),
                            Note::Similar { id, .. } => item.similar.push(id.clone()),
                            _ => {}
                        }
                    }
                    item.path = p.path.clone();
                    item.title = p.title.clone();
                    items.push(item);
                } else {
                    sim.files.remove(&p.path);
                    invalid.push(Invalid { source, reasons: errors.iter().map(|f| format!("{}: {}", f.rule, f.message)).collect() });
                }
            }
        }
    }
    if !invalid.is_empty() {
        invalid.sort_by(|a, b| a.source.cmp(&b.source));
        return Ok(Plan::Invalid(invalid));
    }

    let mut question = format!("Import {} lessons from {}?", items.len(), dir.display());
    for i in &items {
        question.push_str(&format!("\n+ {}  \"{}\"  (from {})", i.path, i.title, i.source));
        if let Some(f) = &i.new_folder {
            question.push_str(&format!("  [new topic folder {f}]"));
        }
        if !i.similar.is_empty() {
            question.push_str(&format!("  [looks like {}]", i.similar.join(", ")));
        }
        if let Some(l) = &i.looser {
            question.push_str(&format!("  [loosens {l}]"));
        }
    }
    let mut choices = vec![Choice { text: "import all".into(), decision: Some(Decision::ImportAll) }];
    if items.iter().any(|i| !i.similar.is_empty()) {
        choices.push(Choice { text: "import without duplicates".into(), decision: Some(Decision::SkipDuplicates) });
    }
    choices.push(Choice { text: "cancel".into(), decision: None });
    let req = Request {
        id: request::new_id(),
        created: request::now(),
        action: Action::Import { items },
        approved: vec![],
        question,
        choices,
        session: None,
    };
    request::save(&ctx.state.join("requests"), &req)?;
    Ok(Plan::Ready(req))
}

/// Adds the reported lessons in order, one commit each, with only the decisions the report showed.
/// Stops at the first lesson that needs anything else, keeping the ones already added.
pub fn write(ctx: &Ctx, items: &[ImportItem], skip_duplicates: bool) -> Result<Outcome> {
    let kb = kb_config(ctx.root);
    let _lock = write::kb_lock(ctx.root, &kb)?;
    let mut out = Imported::default();
    let mut stopped = false;
    for item in items {
        if skip_duplicates && !item.similar.is_empty() {
            out.skipped.push(item.source.clone());
            continue;
        }
        if stopped {
            out.not_added.push((item.source.clone(), "not tried: the import stopped before it".into()));
            continue;
        }
        let snap = Snapshot::from_dir(ctx.root)?;
        let action = Action::Add { text: item.text.clone(), topic: item.topic.clone(), assets: vec![], inbox: None };
        let reason = match write::add(ctx, &kb, &snap, &action, &item.approved, &item.text, &item.topic, &[], None) {
            Ok(Ok(p)) => match write::finish(ctx, snap, p) {
                Ok(Outcome::Written(w)) => {
                    out.added.push(w);
                    continue;
                }
                Ok(other) => format!("unexpected result: {other:?}"),
                Err(e) => e.to_string(),
            },
            Ok(Err(Outcome::NeedsUser(req))) => format!(
                "the knowledge base changed since the report, and it now needs a new decision: {}",
                req.question.lines().next().unwrap_or_default()
            ),
            Ok(Err(other)) => format!("unexpected result: {other:?}"),
            Err(e) => e.to_string(),
        };
        out.not_added.push((item.source.clone(), reason));
        stopped = true;
    }
    Ok(Outcome::Imported(out))
}
