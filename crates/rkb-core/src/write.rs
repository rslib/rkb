use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::time::Duration;

use jiff::civil::Date;
use serde_norway::{Mapping, Value};
use sha2::{Digest, Sha256};

use crate::body;
use crate::config::{self, KbConfig, TopicNote};
use crate::error::{Error, Result, io};
use crate::git;
use crate::kb::{FileKind, NoteRole, Snapshot, classify, note_chain, note_role};
use crate::lesson::{self, Frontmatter, Lesson, LessonType, Status, VerifiedHow};
use crate::lint::{self, LintEnv, Severity};
use crate::lock;
use crate::matching::Place;
use crate::request::{self, Action, Choice, Decision, Request};

pub const DIFF_LINES: usize = 80;
pub const SIMILAR_TITLE: f64 = 0.6;

/// What a write needs from the caller.
pub struct Ctx<'a> {
    pub root: &'a Path,
    pub env: &'a LintEnv,
    /// `$XDG_STATE_HOME/rkb`; requests live in its `requests/` folder.
    pub state: &'a Path,
    pub today: Date,
    /// Where the command runs; lets `add` offer to record a new project's remote.
    pub place: Option<&'a Place>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    pub kind: &'static str,
    pub id: String,
    pub path: String,
    pub title: String,
    pub commit: String,
    pub diff: String,
    pub notes: Vec<Note>,
}

/// Something a write did or found that the user should hear about, without stopping the write.
#[derive(Debug, Clone, PartialEq)]
pub enum Note {
    /// A scope or topic folder the write created.
    NewFolder(String),
    /// A lesson this write archived.
    Archived(String),
    /// Another file whose links a move or rename rewrote.
    LinksUpdated(String),
    /// The result of a check that led to this write.
    Checked(String),
    /// A lesson this write brought back from the archive.
    Unarchived(String),
    /// A project folder note that records the current repository's remote.
    ProjectNote { path: String, remote: String },
    /// An existing lesson in the same topic that looks like the new one.
    Similar { id: String, title: String, path: String },
}

impl Note {
    pub fn message(&self) -> String {
        match self {
            Note::NewFolder(f) => format!("created the folder {f}"),
            Note::Archived(p) => format!("archived {p}; search hides it now"),
            Note::LinksUpdated(p) => format!("updated links in {p}"),
            Note::Checked(m) => m.clone(),
            Note::Unarchived(p) => format!("{p} is active again and back in search"),
            Note::ProjectNote { path, remote } => format!("wrote {path} with the remote {remote}"),
            Note::Similar { id, title, path } => format!(
                "looks like {id} \"{title}\" ({path}); merge them with `rkb show {id}` and `rkb edit {id} --base <hash>` if they say the same"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Imported {
    pub added: Vec<Written>,
    /// Sources skipped as duplicates.
    pub skipped: Vec<String>,
    /// Sources not added, with the reason; the first one stopped the import.
    pub not_added: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Written(Written),
    Unchanged {
        id: String,
        path: String,
    },
    NeedsUser(Request),
    Cancelled,
    /// The result of a confirmed import.
    Imported(Imported),
    /// A finished action that wrote no lesson, such as breaking a lock.
    Info(String),
}

/// 10 random lowercase hex characters that no lesson in `taken` has.
pub fn new_id(taken: &HashSet<&str>) -> String {
    loop {
        let mut b = [0u8; 5];
        getrandom::fill(&mut b).expect("system random source");
        let id: String = b.iter().map(|x| format!("{x:02x}")).collect();
        // An all-digit id such as 1311911595 reads as a YAML number, so it would be written quoted.
        if !taken.contains(id.as_str()) && plain_yaml_string(&id) {
            return id;
        }
    }
}

fn plain_yaml_string(s: &str) -> bool {
    serde_norway::to_string(s).is_ok_and(|out| out.trim_end() == s)
}

/// The first 12 hex characters of the SHA-256 of a file, as `rkb show` prints it.
pub fn content_hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().take(6).map(|x| format!("{x:02x}")).collect()
}

/// Lowercase words of a title: runs of letters and digits.
fn words(title: &str) -> impl Iterator<Item = String> + '_ {
    title.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase)
}

/// The file name for a title: lowercase words joined by `-`, cut at a word boundary to 60 characters.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for w in words(title) {
        if !out.is_empty() && out.len() + 1 + w.len() > 60 {
            break;
        }
        if !out.is_empty() {
            out.push('-');
        }
        out.push_str(&w);
    }
    if out.is_empty() { "lesson".into() } else { out.chars().take(60).collect() }
}

pub fn jaccard(a: &str, b: &str) -> f64 {
    let a: BTreeSet<String> = words(a).collect();
    let b: BTreeSet<String> = words(b).collect();
    let union = a.union(&b).count();
    if union == 0 { 0.0 } else { a.intersection(&b).count() as f64 / union as f64 }
}

/// Up to three of `names`, closest to `name` first.
pub fn closest<'a>(name: &str, names: &'a [String]) -> Vec<&'a str> {
    let mut ranked: Vec<(f64, &str)> =
        names.iter().map(|n| (crate::text::edit_distance(name, n) as f64 / name.len().max(n.len()).max(1) as f64, n.as_str())).collect();
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(b.1)));
    ranked.into_iter().take(3).map(|(_, n)| n).collect()
}

/// The scope folder that `when` implies: one project, else one system, else `general`.
pub fn scope_of(when: &Mapping) -> String {
    let one = |k: &str| when.get(k).and_then(Value::as_str).map(str::to_string);
    match (one("project"), one("system")) {
        (Some(p), _) => format!("projects/{p}"),
        (None, Some(s)) => format!("systems/{s}"),
        _ => "general".into(),
    }
}

/// A lesson skeleton with the headings its type requires.
pub fn template(kind: LessonType) -> String {
    let name = serde_norway::to_string(&kind).unwrap_or_default();
    let mut out =
        format!("---\ntype: {}\nverified_how: ran\n# when:\n#   project: <name>\n# tags:\n#   - <word>\n---\n\n# <Title>\n", name.trim());
    for h in kind.required_headings() {
        out.push_str(&format!("\n## {h}\n\n"));
    }
    out
}

fn title_of(body_text: &str) -> Option<String> {
    body::scan(body_text).h1.into_iter().next().map(|(_, t)| t)
}

fn note_labels(text: &str) -> BTreeMap<String, String> {
    let Some(fm) = config::note_frontmatter(text) else { return BTreeMap::new() };
    let Ok(Value::Mapping(m)) = serde_norway::from_str::<Value>(fm) else { return BTreeMap::new() };
    let Some(Value::Mapping(labels)) = m.get("labels") else { return BTreeMap::new() };
    labels.iter().filter_map(|(k, v)| Some((k.as_str()?.to_string(), v.as_str()?.to_string()))).collect()
}

fn effective(snap: &Snapshot, path: &str, own: &Mapping) -> BTreeMap<String, String> {
    let notes: Vec<BTreeMap<String, String>> =
        note_chain(path).iter().filter_map(|n| snap.files.get(n)).map(|d| note_labels(&String::from_utf8_lossy(d))).collect();
    let refs: Vec<&BTreeMap<String, String>> = notes.iter().collect();
    config::effective_labels(own, &refs)
}

/// The first label that becomes looser: an earlier value in that key's list in `kb.toml`.
fn looser(kb: &KbConfig, old: &BTreeMap<String, String>, new: &BTreeMap<String, String>) -> Option<(String, String)> {
    for (key, value) in new {
        let (Some(order), Some(before)) = (kb.labels.get(key), old.get(key)) else { continue };
        let pos = |v: &str| order.iter().position(|o| o == v);
        if let (Some(n), Some(o)) = (pos(value), pos(before))
            && n < o
        {
            return Some((key.clone(), value.clone()));
        }
    }
    None
}

fn refused(msg: impl Into<String>) -> Error {
    Error::Refused(msg.into())
}

fn ask(action: &Action, approved: &[Decision], question: String, mut choices: Vec<Choice>) -> Result<Outcome> {
    choices.push(Choice { text: "cancel".into(), decision: None });
    let req =
        Request { id: request::new_id(), created: request::now(), action: action.clone(), approved: approved.to_vec(), question, choices };
    Ok(Outcome::NeedsUser(req))
}

pub(crate) struct Prepared {
    kind: &'static str,
    id: String,
    pub(crate) path: String,
    pub(crate) title: String,
    text: String,
    /// Other new files committed with the lesson, such as a created folder note.
    extra: Vec<(String, String)>,
    pub(crate) notes: Vec<Note>,
    /// A commit message other than `<kind>(<folder>): <title> [<id>]`, as for a folder archive.
    message: Option<String>,
}

/// Runs one write: prepare, check each decision point, validate, write, commit.
/// A check whose decision is not in `approved` stores a request and returns `NeedsUser`.
pub fn apply(ctx: &Ctx, action: &Action, approved: &[Decision]) -> Result<Outcome> {
    let kb: KbConfig = std::fs::read_to_string(ctx.root.join("kb.toml")).ok().and_then(|t| config::parse(&t).ok()).unwrap_or_default();
    let _lock = kb_lock(ctx.root, &kb)?;
    let snap = Snapshot::from_dir(ctx.root)?;
    let prepared = match action {
        Action::Add { text, topic } => add(ctx, &kb, &snap, action, approved, text, topic)?,
        Action::Edit { id, text, base } => edit(&kb, &snap, action, approved, id, text, base)?,
        Action::Flag { id, reason } => flag(&snap, id, reason)?,
        Action::Supersede { id, by, reason } => supersede(&snap, action, approved, id, by, reason)?,
        Action::Archive { target, reason } => archive(ctx, &snap, action, approved, target, reason)?,
        Action::Unarchive { id } => unarchive(&snap, id)?,
        Action::Move { id, folder } => return saved(ctx, relocate(ctx, snap, action, approved, id, Some(folder), None)?),
        Action::Rename { id, slug } => return saved(ctx, relocate(ctx, snap, action, approved, id, None, Some(slug))?),
        Action::BreakLock { .. } | Action::Install { .. } | Action::Import { .. } | Action::Approve { .. } | Action::ApproveFact { .. } => {
            unreachable!("confirm handles these itself")
        }
    };
    match prepared {
        Ok(p) => finish(ctx, snap, p),
        Err(outcome) => saved(ctx, outcome),
    }
}

/// Saves the request of a `NeedsUser` outcome, so `rkb confirm` can find it.
fn saved(ctx: &Ctx, o: Outcome) -> Result<Outcome> {
    if let Outcome::NeedsUser(req) = &o {
        request::save(&ctx.state.join("requests"), req)?;
    }
    Ok(o)
}

/// Takes the knowledge base's write lock with `lock.stale_s` and `lock.wait_s` from `kb.toml`.
pub fn kb_lock(root: &Path, kb: &KbConfig) -> Result<lock::LockGuard> {
    let setting = |key: &str, default: u64| {
        kb.lock.as_ref().and_then(|t| t.get(key)).and_then(|v| v.as_integer()).map_or(default, |v| v.max(0) as u64)
    };
    let git_dir = String::from_utf8_lossy(&git::run(root, &["rev-parse", "--git-dir"])?).trim().to_string();
    lock::acquire(&root.join(git_dir).join("rkb.lock"), setting("stale_s", 600), Duration::from_secs(setting("wait_s", 10)))
}

/// Runs the stored action of a request with the chosen decision added.
pub fn confirm(ctx: &Ctx, req: &Request, choice: &str) -> Result<Outcome> {
    let Some(c) = req.choices.iter().find(|c| c.text == choice) else {
        return Err(Error::BadChoice { choice: choice.to_string(), options: req.options() });
    };
    request::remove(&ctx.state.join("requests"), &req.id);
    let Some(decision) = &c.decision else { return Ok(Outcome::Cancelled) };
    if let Action::ApproveFact { key, sha256, system } = &req.action {
        return crate::verify::approve_fact(ctx, key, sha256, system);
    }
    if let Action::Approve { id, script, sha256, system, then_verify } = &req.action {
        return crate::verify::approve(ctx, id, *script, sha256, system, *then_verify);
    }
    if let Action::Install { harnesses, uninstall } = &req.action {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
        let steps = crate::install::plan(&home, &crate::paths::config_dir(), harnesses, *uninstall);
        let changes = crate::install::apply(&steps, crate::install::SKILL)?;
        let lines: Vec<String> =
            changes.iter().map(|c| format!("{:<9} {}", format!("{:?}", c.effect).to_lowercase(), c.path.display())).collect();
        return Ok(Outcome::Info(lines.join("\n")));
    }
    if let Action::BreakLock { path, holder } = &req.action {
        return Ok(Outcome::Info(if crate::doctor::break_lock(path, holder) {
            format!("Removed the lock {path}")
        } else {
            format!("The lock {path} changed or is gone since the request; nothing was removed")
        }));
    }
    if let Action::Import { items } = &req.action {
        return crate::import::write(ctx, items, *decision == Decision::SkipDuplicates);
    }
    let mut approved = req.approved.clone();
    approved.push(decision.clone());
    apply(ctx, &req.action, &approved)
}

fn find<'a>(lessons: &'a [Lesson], id: &str) -> Result<&'a Lesson> {
    lessons.iter().find(|l| l.frontmatter.id == id).ok_or_else(|| Error::NotFound(id.to_string()))
}

pub(crate) type Step = std::result::Result<Prepared, Outcome>;

pub(crate) fn add(
    ctx: &Ctx,
    kb: &KbConfig,
    snap: &Snapshot,
    action: &Action,
    approved: &[Decision],
    text: &str,
    topic: &str,
) -> Result<Step> {
    let (fm_text, body_text, _) = lesson::split(text).map_err(|e| refused(e.message))?;
    let mut map: Mapping = if fm_text.trim().is_empty() {
        Mapping::new()
    } else {
        serde_norway::from_str(fm_text).map_err(|e| refused(format!("frontmatter: {e}")))?
    };
    for key in ["id", "schema", "status", "superseded_by", "stale_reason"] {
        if map.contains_key(key) {
            return Err(refused(format!("remove `{key}` from the frontmatter; rkb sets it")));
        }
    }
    if map.get("verified_how").and_then(Value::as_str) == Some("checked") {
        return Err(refused("only `rkb verify` sets `verified_how: checked`"));
    }
    let title = title_of(body_text).ok_or_else(|| refused("the body needs a `# Title` line"))?;

    let (lessons, _) = snap.lessons();
    let taken: HashSet<&str> = lessons.iter().map(|l| l.frontmatter.id.as_str()).collect();
    let id = new_id(&taken);
    map.insert("schema".into(), 1.into());
    map.insert("id".into(), id.clone().into());
    map.insert("status".into(), "active".into());
    if !map.contains_key("verified") {
        map.insert("verified".into(), ctx.today.to_string().into());
    }
    let fm: Frontmatter = serde_norway::from_value(Value::Mapping(map)).map_err(|e| refused(format!("frontmatter: {e}")))?;

    let scope = scope_of(&fm.when);
    let folder = approved
        .iter()
        .find_map(|d| match d {
            Decision::UseTopic { path } => Some(path.clone()),
            _ => None,
        })
        .unwrap_or_else(|| format!("{scope}/{}", resolve_topic(snap, &scope, topic)));

    let project_note = approved.iter().find_map(|d| match d {
        Decision::CreateProject { path, remotes, root_commit } if *path == folder => Some((remotes.clone(), root_commit.clone())),
        _ => None,
    });
    let exists = ctx.root.join(&folder).is_dir() || snap.files.keys().any(|k| k.starts_with(&format!("{folder}/")));
    let decided = approved.iter().any(|d| matches!(d, Decision::CreateFolder { path } | Decision::UseTopic { path } if *path == folder));
    let mut notes = vec![];
    let mut project_note = project_note;
    if !exists && project_note.is_none() {
        let name = folder.rsplit('/').next().unwrap();
        let topics = topics_in(ctx.root, &scope);
        let limit = (name.chars().count() / 4).max(1);
        let close: Vec<&String> = topics.iter().filter(|t| *t != name && crate::text::edit_distance(name, t) <= limit).collect();
        if !close.is_empty() && !decided {
            let question = format!(
                "Topic folder {folder} does not exist, and its name is close to an existing topic. Create it, or use the existing one?"
            );
            let mut choices =
                vec![Choice { text: format!("create {folder}"), decision: Some(Decision::CreateFolder { path: folder.clone() }) }];
            for t in closest(name, &topics) {
                let path = format!("{scope}/{t}");
                choices.push(Choice { text: format!("use {path}"), decision: Some(Decision::UseTopic { path }) });
            }
            return ask(action, approved, question, choices).map(Err);
        }
        if let Some(repo) = new_project_repo(ctx, &scope)
            && let Some(first) = repo.remotes.first()
            && first.rsplit('/').next() == scope.strip_prefix("projects/")
        {
            let root_commit = match repo.root_commits.as_slice() {
                [one] => Some(one.clone()),
                _ => None,
            };
            project_note = Some((repo.remotes.clone(), root_commit));
        }
    }
    if !exists {
        notes.push(Note::NewFolder(folder.clone()));
    }

    let base_slug = slug(&title);
    let siblings: Vec<&Lesson> = lessons.iter().filter(|l| l.path.rsplit_once('/').map(|p| p.0) == Some(folder.as_str())).collect();
    for l in siblings {
        let stem = l.path.rsplit('/').next().unwrap().trim_end_matches(".md");
        let other = title_of(&l.body).unwrap_or_default();
        if stem == base_slug || jaccard(&other, &title) >= SIMILAR_TITLE {
            notes.push(Note::Similar { id: l.frontmatter.id.clone(), title: other, path: l.path.clone() });
        }
    }
    let mut path = format!("{folder}/{base_slug}.md");
    let mut n = 2;
    while snap.files.contains_key(&path) || ctx.root.join(&path).exists() {
        path = format!("{folder}/{base_slug}-{n}.md");
        n += 1;
    }

    if let Some(step) = check_labels(kb, snap, action, approved, &path, &Mapping::new(), &fm.labels)? {
        return Ok(Err(step));
    }
    let mut extra = vec![];
    if let Some((remotes, root_commit)) = project_note {
        let note = format!("{scope}/README.md");
        if !ctx.root.join(&note).exists() {
            extra.push((note.clone(), project_note_text(&scope, &remotes, root_commit.as_deref())));
            notes.push(Note::ProjectNote { path: note, remote: remotes.first().cloned().unwrap_or_default() });
        }
    }
    Ok(Ok(Prepared { kind: "add", id, path, title, text: lesson::write(&fm, body_text), extra, notes, message: None }))
}

/// The current repository, when a new project folder is being made from inside an unmatched checkout.
fn new_project_repo<'a>(ctx: &Ctx<'a>, scope: &str) -> Option<&'a crate::matching::Repo> {
    let place = ctx.place?;
    (scope.starts_with("projects/") && !ctx.root.join(scope).exists() && place.project.is_none()).then_some(())?;
    place.repo.as_ref()
}

fn project_note_text(scope: &str, remotes: &[String], root_commit: Option<&str>) -> String {
    let mut m = Mapping::new();
    m.insert("remotes".into(), Value::Sequence(remotes.iter().map(|r| r.clone().into()).collect()));
    if let Some(c) = root_commit {
        m.insert("root_commit".into(), c.into());
    }
    let name = scope.rsplit('/').next().unwrap_or(scope);
    format!("---\n{}---\n\n# {name}\n", crate::yaml::block(&Value::Mapping(m)))
}

fn resolve_topic(snap: &Snapshot, scope: &str, topic: &str) -> String {
    for (path, data) in snap.of_kind(FileKind::FolderNote) {
        if note_role(path) != NoteRole::Topic || !path.starts_with(&format!("{scope}/")) {
            continue;
        }
        let note: TopicNote = config::parse_note(&String::from_utf8_lossy(data)).unwrap_or_default();
        if note.aliases.iter().any(|a| a == topic) {
            return path.rsplit('/').nth(1).unwrap().to_string();
        }
    }
    topic.to_string()
}

fn topics_in(root: &Path, scope: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root.join(scope)) else { return vec![] };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.') && !n.ends_with(".assets"))
        .collect();
    names.sort();
    names
}

fn check_labels(
    kb: &KbConfig,
    snap: &Snapshot,
    action: &Action,
    approved: &[Decision],
    path: &str,
    old: &Mapping,
    new: &Mapping,
) -> Result<Option<Outcome>> {
    let before = effective(snap, path, old);
    let after = effective(snap, path, new);
    match looser(kb, &before, &after) {
        Some((key, value)) if !approved.contains(&Decision::Loosen { key: key.clone(), value: value.clone() }) => {
            let question = format!(
                "This write makes label {key} looser, from `{}` to `{value}`. Allow it?",
                before.get(&key).cloned().unwrap_or_default()
            );
            let choices = vec![Choice { text: format!("loosen {key} to {value}"), decision: Some(Decision::Loosen { key, value }) }];
            ask(action, approved, question, choices).map(Some)
        }
        _ => Ok(None),
    }
}

fn edit(kb: &KbConfig, snap: &Snapshot, action: &Action, approved: &[Decision], id: &str, text: &str, base: &str) -> Result<Step> {
    let (lessons, _) = snap.lessons();
    let old = find(&lessons, id)?;
    let current = content_hash(&snap.files[&old.path]);
    if current != base {
        return Err(Error::Conflict(current));
    }
    let mut new = lesson::parse(&old.path, text).map_err(|e| refused(e.message))?;
    let (o, n) = (&old.frontmatter, &mut new.frontmatter);
    if n.id != o.id {
        return Err(refused("`rkb edit` cannot change `id`"));
    }
    if n.schema != o.schema {
        return Err(refused("`rkb edit` cannot change `schema`; `rkb migrate` does that"));
    }
    if n.verified_how == VerifiedHow::Checked && o.verified_how != VerifiedHow::Checked {
        return Err(refused("only `rkb verify` sets `verified_how: checked`"));
    }
    let revive = o.status == Status::Stale
        && body::section_text(&old.body, "Evidence") != body::section_text(&new.body, "Evidence")
        && matches!(n.status, Status::Stale | Status::Active)
        && n.superseded_by == o.superseded_by;
    if revive {
        n.status = Status::Active;
        n.stale_reason = None;
    } else if n.status != o.status || n.superseded_by != o.superseded_by || n.stale_reason != o.stale_reason {
        return Err(refused(
            "`rkb edit` does not change `status`, `superseded_by` or `stale_reason`; use `rkb flag` (or change `Evidence` to revive a stale lesson)",
        ));
    }
    let out = lesson::write(&new.frontmatter, &new.body);
    if out.as_bytes() == snap.files[&old.path].as_slice() {
        return Ok(Err(Outcome::Unchanged { id: id.to_string(), path: old.path.clone() }));
    }
    if let Some(step) = check_labels(kb, snap, action, approved, &old.path, &o.labels, &new.frontmatter.labels)? {
        return Ok(Err(step));
    }
    let title = title_of(&new.body).unwrap_or_default();
    Ok(Ok(Prepared {
        kind: "edit",
        id: id.to_string(),
        path: old.path.clone(),
        title,
        text: out,
        extra: vec![],
        notes: vec![],
        message: None,
    }))
}

fn flag(snap: &Snapshot, id: &str, reason: &str) -> Result<Step> {
    let (lessons, _) = snap.lessons();
    let old = find(&lessons, id)?;
    if reason.trim().is_empty() {
        return Err(refused("give a reason with --reason"));
    }
    if !old.frontmatter.status.is_current() {
        return Err(refused(format!("lesson {id} is {}; only an active or stale lesson can be flagged", old.frontmatter.status.as_str())));
    }
    let mut fm = old.frontmatter.clone();
    fm.status = Status::Stale;
    fm.stale_reason = Some(reason.trim().to_string());
    let out = lesson::write(&fm, &old.body);
    if out.as_bytes() == snap.files[&old.path].as_slice() {
        return Ok(Err(Outcome::Unchanged { id: id.to_string(), path: old.path.clone() }));
    }
    let title = title_of(&old.body).unwrap_or_default();
    Ok(Ok(Prepared {
        kind: "flag",
        id: id.to_string(),
        path: old.path.clone(),
        title,
        text: out,
        extra: vec![],
        notes: vec![],
        message: None,
    }))
}

/// `body` with a `## <heading>` section appended.
fn with_section(body: &str, heading: &str, text: &str) -> String {
    format!("{}\n\n## {heading}\n{}\n", body.trim_end(), text.trim())
}

/// `body` without its `## <heading>` section, which ends at the next `## ` heading or the end.
fn without_section(body: &str, heading: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let Some(start) = lines.iter().position(|l| l.trim_end() == format!("## {heading}")) else { return body.to_string() };
    let end = lines[start + 1..].iter().position(|l| l.starts_with("## ")).map_or(lines.len(), |i| start + 1 + i);
    let kept: Vec<&str> = lines[..start].iter().chain(&lines[end..]).copied().collect();
    format!("{}\n", kept.join("\n").trim_end())
}

/// The path of `to` relative to the folder of `from`; both are knowledge-base paths.
fn relative(from: &str, to: &str) -> String {
    let from_dir: Vec<&str> = from.split('/').collect::<Vec<_>>()[..from.split('/').count() - 1].to_vec();
    let to_parts: Vec<&str> = to.split('/').collect();
    let common = from_dir.iter().zip(&to_parts).take_while(|(a, b)| a == b).count();
    let mut out = "../".repeat(from_dir.len() - common);
    out.push_str(&to_parts[common..].join("/"));
    out
}

fn current_or_refuse(l: &Lesson, what: &str) -> Result<()> {
    if l.frontmatter.status.is_current() {
        Ok(())
    } else {
        Err(refused(format!("lesson {} is {}; {what}", l.frontmatter.id, l.frontmatter.status.as_str())))
    }
}

fn supersede(snap: &Snapshot, action: &Action, approved: &[Decision], id: &str, by: &str, reason: &str) -> Result<Step> {
    let (lessons, _) = snap.lessons();
    let old = find(&lessons, id)?;
    let new = find(&lessons, by)?;
    if id == by {
        return Err(refused("a lesson cannot supersede itself; give another lesson with --by"));
    }
    if reason.trim().is_empty() {
        return Err(refused("give a reason with --reason"));
    }
    current_or_refuse(old, "only an active or stale lesson can be superseded")?;
    current_or_refuse(new, "the replacement must be active or stale")?;
    let old_title = title_of(&old.body).unwrap_or_default();
    let new_title = title_of(&new.body).unwrap_or_default();
    if !approved.contains(&Decision::Supersede) {
        let question = format!(
            "Supersede {id} \"{old_title}\" ({}) by {by} \"{new_title}\" ({})? Search will hide {id}.\nReason: {}",
            old.path,
            new.path,
            reason.trim()
        );
        let choices = vec![Choice { text: format!("supersede {id}"), decision: Some(Decision::Supersede) }];
        return ask(action, approved, question, choices).map(Err);
    }
    let mut fm = old.frontmatter.clone();
    fm.status = Status::Superseded;
    fm.superseded_by = Some(by.to_string());
    fm.stale_reason = None;
    let text = format!("{}\n\nReplaced by [{new_title}]({}).", reason.trim(), relative(&old.path, &new.path));
    let body = with_section(&old.body, "Why superseded", &text);
    Ok(Ok(Prepared {
        kind: "supersede",
        id: id.to_string(),
        path: old.path.clone(),
        title: old_title,
        text: lesson::write(&fm, &body),
        extra: vec![],
        notes: vec![],
        message: None,
    }))
}

fn archive(ctx: &Ctx, snap: &Snapshot, action: &Action, approved: &[Decision], target: &str, reason: &str) -> Result<Step> {
    let (lessons, _) = snap.lessons();
    if reason.trim().is_empty() {
        return Err(refused("give a reason with --reason"));
    }
    let folder = target.trim_end_matches('/');
    let chosen: Vec<&Lesson> = match find(&lessons, target) {
        Ok(l) => {
            current_or_refuse(l, "only an active or stale lesson can be archived")?;
            vec![l]
        }
        Err(_) if !folder.is_empty() && !folder.split('/').any(|p| p == ".." || p == ".") && ctx.root.join(folder).is_dir() => {
            let current: Vec<&Lesson> =
                lessons.iter().filter(|l| l.path.starts_with(&format!("{folder}/")) && l.frontmatter.status.is_current()).collect();
            if current.is_empty() {
                return Err(refused(format!("{folder} has no active or stale lesson to archive")));
            }
            current
        }
        Err(e) => return Err(e),
    };
    if !approved.contains(&Decision::Archive) {
        let mut question = format!("Archive {} lesson(s) under {target}? Search will hide them.\nReason: {}", chosen.len(), reason.trim());
        for l in &chosen {
            question.push_str(&format!("\n- {} {} ({})", l.frontmatter.id, title_of(&l.body).unwrap_or_default(), l.path));
        }
        let choices = vec![Choice { text: format!("archive {target}"), decision: Some(Decision::Archive) }];
        return ask(action, approved, question, choices).map(Err);
    }
    let texts: Vec<(String, String)> = chosen
        .iter()
        .map(|l| {
            let mut fm = l.frontmatter.clone();
            fm.status = Status::Archived;
            fm.stale_reason = None;
            (l.path.clone(), lesson::write(&fm, &with_section(&l.body, "Why archived", reason)))
        })
        .collect();
    let single = chosen.len() == 1 && chosen[0].frontmatter.id == target;
    let first = chosen[0];
    Ok(Ok(Prepared {
        kind: "archive",
        id: if single { first.frontmatter.id.clone() } else { String::new() },
        path: texts[0].0.clone(),
        title: title_of(&first.body).unwrap_or_default(),
        text: texts[0].1.clone(),
        extra: texts[1..].to_vec(),
        notes: if single { vec![] } else { chosen.iter().map(|l| Note::Archived(l.path.clone())).collect() },
        message: (!single).then(|| format!("archive({folder}): {} lessons", chosen.len())),
    }))
}

fn unarchive(snap: &Snapshot, id: &str) -> Result<Step> {
    let (lessons, _) = snap.lessons();
    let old = find(&lessons, id)?;
    if old.frontmatter.status != Status::Archived {
        return Err(refused(format!("lesson {id} is {}; only an archived lesson can be unarchived", old.frontmatter.status.as_str())));
    }
    let mut fm = old.frontmatter.clone();
    fm.status = Status::Active;
    Ok(Ok(Prepared {
        kind: "unarchive",
        id: id.to_string(),
        path: old.path.clone(),
        title: title_of(&old.body).unwrap_or_default(),
        text: lesson::write(&fm, &without_section(&old.body, "Why archived")),
        extra: vec![],
        notes: vec![Note::Unarchived(old.path.clone())],
        message: None,
    }))
}

/// `text` of a file at `from` that moves to `to`, with every relative link rewritten so it resolves to the same file,
/// where `moved` maps old paths to new ones. Links in code and links with a scheme stay as they are.
pub(crate) fn rewrite_links(text: &str, from: &str, to: &str, moved: &BTreeMap<String, String>) -> String {
    let b = body::scan(text);
    let mut lines: Vec<String> = text.split_inclusive('\n').map(String::from).collect();
    for link in &b.links {
        let Some(Some(old_target)) = lint::resolve(from, &link.target) else { continue };
        let new_target = moved.get(&old_target).cloned().unwrap_or_else(|| old_target.clone());
        if from == to && new_target == old_target {
            continue;
        }
        let tail = link.target.find(['#', '?']).map_or("", |i| &link.target[i..]);
        let new = format!("{}{tail}", relative(to, &new_target));
        let Some(line) = lines.get_mut(link.line - 1) else { continue };
        for (a, b) in [
            (format!("]({})", link.target), format!("]({new})")),
            (format!("](<{}>", link.target), format!("](<{new}>")),
            (format!("]({} ", link.target), format!("]({new} ")),
        ] {
            if line.contains(&a) {
                *line = line.replacen(&a, &b, 1);
                break;
            }
        }
    }
    lines.concat()
}

fn valid_slug(slug: &str) -> bool {
    !slug.is_empty() && slug.split('-').all(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()))
}

/// Moves a lesson to `folder` or renames it to `slug`, with its assets, and rewrites every link to them in one commit.
fn relocate(
    ctx: &Ctx,
    snap: Snapshot,
    action: &Action,
    approved: &[Decision],
    id: &str,
    folder: Option<&str>,
    slug: Option<&str>,
) -> Result<Outcome> {
    let (lessons, _) = snap.lessons();
    let old = find(&lessons, id)?;
    let (old_dir, old_file) = old.path.rsplit_once('/').expect("lessons live in folders");
    let old_stem = old_file.trim_end_matches(".md");
    let new_dir = folder.map(|f| f.trim_end_matches('/')).unwrap_or(old_dir);
    let new_stem = slug.unwrap_or(old_stem);
    if new_dir.is_empty() || new_dir.split('/').any(|p| p.is_empty() || p == ".." || p == "." || p.starts_with('.')) {
        return Err(refused(format!("`{new_dir}` is not a folder of the knowledge base")));
    }
    if !valid_slug(new_stem) {
        return Err(refused(format!("`{new_stem}` is not a valid slug; use lowercase letters, digits and `-`")));
    }
    if new_dir == old_dir && new_stem == old_stem {
        return Err(refused(format!("{} is already there", old.path)));
    }
    let new_path = format!("{new_dir}/{new_stem}.md");
    if snap.files.contains_key(&new_path) || ctx.root.join(&new_path).exists() {
        return Err(refused(format!("{new_path} already exists")));
    }

    let mut notes = vec![];
    let folder_exists = ctx.root.join(new_dir).is_dir();
    if !folder_exists {
        let decided = approved.iter().any(|d| matches!(d, Decision::CreateFolder { path } if path == new_dir));
        if let Some(use_topic) = approved.iter().find_map(|d| match d {
            Decision::UseTopic { path } => Some(path.clone()),
            _ => None,
        }) {
            return relocate(ctx, snap, action, &[], id, Some(&use_topic), None);
        }
        let (scope, name) = new_dir.rsplit_once('/').unwrap_or(("", new_dir));
        let topics = topics_in(ctx.root, scope);
        let limit = (name.chars().count() / 4).max(1);
        if !decided && topics.iter().any(|t| t != name && crate::text::edit_distance(name, t) <= limit) {
            let question = format!(
                "Topic folder {new_dir} does not exist, and its name is close to an existing topic. Create it, or use the existing one?"
            );
            let mut choices =
                vec![Choice { text: format!("create {new_dir}"), decision: Some(Decision::CreateFolder { path: new_dir.to_string() }) }];
            for t in closest(name, &topics) {
                let path = format!("{scope}/{t}");
                choices.push(Choice { text: format!("use {path}"), decision: Some(Decision::UseTopic { path }) });
            }
            return ask(action, approved, question, choices);
        }
        notes.push(Note::NewFolder(new_dir.to_string()));
    }

    let old_assets = format!("{old_dir}/{old_stem}.assets/");
    let mut moved: BTreeMap<String, String> = BTreeMap::new();
    moved.insert(old.path.clone(), new_path.clone());
    for path in snap.files.keys().filter(|p| p.starts_with(&old_assets)) {
        moved.insert(path.clone(), format!("{new_dir}/{new_stem}.assets/{}", &path[old_assets.len()..]));
    }

    let mut next = Snapshot { files: snap.files.clone() };
    let mut changed: Vec<(String, String)> = vec![];
    for (path, data) in &snap.files {
        if moved.contains_key(path) || !matches!(classify(path), FileKind::Lesson | FileKind::FolderNote) {
            continue;
        }
        let text = String::from_utf8_lossy(data);
        let new_text = rewrite_links(&text, path, path, &moved);
        if new_text != text {
            next.files.insert(path.clone(), new_text.clone().into_bytes());
            changed.push((path.clone(), new_text));
            notes.push(Note::LinksUpdated(path.clone()));
        }
    }
    let lesson_text = rewrite_links(&String::from_utf8_lossy(&snap.files[&old.path]), &old.path, &new_path, &moved);
    for (from, to) in &moved {
        let data = next.files.remove(from).unwrap_or_default();
        next.files.insert(to.clone(), if from == &old.path { lesson_text.clone().into_bytes() } else { data });
    }
    let focus: BTreeSet<String> = moved.values().cloned().chain(changed.iter().map(|(p, _)| p.clone())).collect();
    let blocking: Vec<_> = lint::lint(&next, ctx.env, Some(&focus))
        .into_iter()
        .filter(|f| f.severity == Severity::Error && (focus.contains(&f.path) || f.path.starts_with(&format!("{new_dir}/"))))
        .collect();
    if !blocking.is_empty() {
        return Err(Error::Invalid(blocking));
    }

    let write_file = |rel: &str, text: &str| -> Result<()> {
        let abs = ctx.root.join(rel);
        let dir = abs.parent().unwrap();
        std::fs::create_dir_all(dir).map_err(io(dir))?;
        let tmp = dir.join(format!(".{}.rkb-tmp", abs.file_name().unwrap().to_string_lossy()));
        std::fs::write(&tmp, text).map_err(io(&tmp))?;
        std::fs::rename(&tmp, &abs).map_err(io(&abs))
    };
    for (path, text) in &changed {
        write_file(path, text)?;
    }
    write_file(&new_path, &lesson_text)?;
    for (from, to) in moved.iter().filter(|(f, _)| **f != old.path) {
        let (src, dst) = (ctx.root.join(from), ctx.root.join(to));
        std::fs::create_dir_all(dst.parent().unwrap()).map_err(io(dst.parent().unwrap()))?;
        std::fs::rename(&src, &dst).map_err(io(&src))?;
    }
    std::fs::remove_file(ctx.root.join(&old.path)).map_err(io(ctx.root.join(&old.path)))?;
    let _ = std::fs::remove_dir(ctx.root.join(&old_assets));

    let kind = if folder.is_some() { "move" } else { "rename" };
    let title = title_of(&old.body).unwrap_or_default();
    let message = format!("{kind}({new_dir}): {title} [{id}]");
    let paths: Vec<&str> = moved.keys().chain(moved.values()).chain(changed.iter().map(|(p, _)| p)).map(String::as_str).collect();
    let mut add_args = vec!["add", "-A", "--"];
    add_args.extend(&paths);
    git::run(ctx.root, &add_args)?;
    git::commit_paths(ctx.root, &message, &paths)?;
    let commit = String::from_utf8_lossy(&git::run(ctx.root, &["rev-parse", "--short", "HEAD"])?).trim().to_string();
    let diff = String::from_utf8_lossy(&git::run(ctx.root, &["show", "-M", "--format=", &commit])?).into_owned();
    Ok(Outcome::Written(Written { kind, id: id.to_string(), path: new_path, title, commit, diff, notes }))
}

/// Applies a check result to the lesson as it is now, under the lock, when its `Check` is still the script that ran.
/// Returns `None` when there is nothing to change.
pub(crate) fn apply_check(
    ctx: &Ctx,
    id: &str,
    sha256: &str,
    decide: impl FnOnce(&Frontmatter) -> Option<(Frontmatter, String)>,
) -> Result<Option<Outcome>> {
    let kb: KbConfig = std::fs::read_to_string(ctx.root.join("kb.toml")).ok().and_then(|t| config::parse(&t).ok()).unwrap_or_default();
    let _lock = kb_lock(ctx.root, &kb)?;
    let snap = Snapshot::from_dir(ctx.root)?;
    let (lessons, _) = snap.lessons();
    let l = find(&lessons, id)?;
    if crate::script::of(l, crate::script::Kind::Check).map(|t| crate::script::hash(&t)).as_deref() != Some(sha256) {
        return Ok(Some(Outcome::Info(format!("the Check of {id} changed while it ran; nothing was written"))));
    }
    let Some((fm, message)) = decide(&l.frontmatter) else { return Ok(None) };
    let p = Prepared {
        kind: "verify",
        id: id.to_string(),
        path: l.path.clone(),
        title: title_of(&l.body).unwrap_or_default(),
        text: lesson::write(&fm, &l.body),
        extra: vec![],
        notes: vec![Note::Checked(message)],
        message: None,
    };
    finish(ctx, snap, p).map(Some)
}

/// Puts the prepared files into `snap` and returns the lint errors that concern them.
pub(crate) fn stage(snap: &mut Snapshot, env: &LintEnv, p: &Prepared) -> Vec<lint::Finding> {
    snap.files.insert(p.path.clone(), p.text.clone().into_bytes());
    for (path, text) in &p.extra {
        snap.files.insert(path.clone(), text.clone().into_bytes());
    }
    let written: Vec<&String> = std::iter::once(&p.path).chain(p.extra.iter().map(|(path, _)| path)).collect();
    let focus: BTreeSet<String> = written.iter().map(|s| s.to_string()).collect();
    lint::lint(snap, env, Some(&focus))
        .into_iter()
        .filter(|f| f.severity == Severity::Error && written.iter().any(|w| **w == f.path || w.starts_with(&format!("{}/", f.path))))
        .collect()
}

pub(crate) fn finish(ctx: &Ctx, mut snap: Snapshot, p: Prepared) -> Result<Outcome> {
    let blocking = stage(&mut snap, ctx.env, &p);
    if !blocking.is_empty() {
        return Err(Error::Invalid(blocking));
    }
    let written: Vec<&String> = std::iter::once(&p.path).chain(p.extra.iter().map(|(path, _)| path)).collect();

    for (path, text) in p.extra.iter().chain(std::iter::once(&(p.path.clone(), p.text.clone()))) {
        let abs = ctx.root.join(path);
        let dir = abs.parent().unwrap();
        std::fs::create_dir_all(dir).map_err(io(dir))?;
        let tmp = dir.join(format!(".{}.rkb-tmp", abs.file_name().unwrap().to_string_lossy()));
        std::fs::write(&tmp, text).map_err(io(&tmp))?;
        std::fs::rename(&tmp, &abs).map_err(io(&abs))?;
    }

    let folder = p.path.rsplit_once('/').map_or("", |(d, _)| d);
    let message = p.message.clone().unwrap_or_else(|| format!("{}({folder}): {} [{}]", p.kind, p.title, p.id));
    let paths: Vec<&str> = written.iter().map(|s| s.as_str()).collect();
    let mut add_args = vec!["add", "--"];
    add_args.extend(&paths);
    git::run(ctx.root, &add_args)?;
    git::commit_paths(ctx.root, &message, &paths)?;
    let commit = String::from_utf8_lossy(&git::run(ctx.root, &["rev-parse", "--short", "HEAD"])?).trim().to_string();
    let diff = String::from_utf8_lossy(&git::run(ctx.root, &["show", "--format=", &commit])?).into_owned();
    Ok(Outcome::Written(Written { kind: p.kind, id: p.id, path: p.path, title: p.title, commit, diff, notes: p.notes }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_follow_a_move() {
        let moved: BTreeMap<String, String> =
            [("general/cpp/a.md", "general/build/a.md"), ("general/cpp/a.assets/p.svg", "general/build/a.assets/p.svg")]
                .into_iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect();
        let inbound = "see [a](../cpp/a.md#fix), `[x](../cpp/a.md)` and [w](https://example.org/a.md)\n";
        assert_eq!(
            rewrite_links(inbound, "general/git/b.md", "general/git/b.md", &moved),
            "see [a](../build/a.md#fix), `[x](../cpp/a.md)` and [w](https://example.org/a.md)\n"
        );
        let own = "[b](../git/b.md) ![p](a.assets/p.svg)\n";
        assert_eq!(rewrite_links(own, "general/cpp/a.md", "general/build/a.md", &moved), own, "same depth, same relative links");
        let deep: BTreeMap<String, String> = [("general/cpp/a.md".to_string(), "projects/x/cmake/a.md".to_string())].into_iter().collect();
        assert_eq!(
            rewrite_links("[b](../git/b.md)\n", "general/cpp/a.md", "projects/x/cmake/a.md", &deep),
            "[b](../../../general/git/b.md)\n"
        );
        let renamed: BTreeMap<String, String> =
            [("general/cpp/a.md", "general/cpp/c.md"), ("general/cpp/a.assets/p.svg", "general/cpp/c.assets/p.svg")]
                .into_iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect();
        assert_eq!(rewrite_links("![p](a.assets/p.svg)\n", "general/cpp/a.md", "general/cpp/c.md", &renamed), "![p](c.assets/p.svg)\n");
        assert_eq!(rewrite_links("[untouched](other.md)\n", "general/git/b.md", "general/git/b.md", &moved), "[untouched](other.md)\n");
    }

    #[test]
    fn sections_and_relative_links() {
        let body = "\n# T\n\n## Fix\nDo it.\n";
        let with = with_section(body, "Why archived", "Retired.");
        assert_eq!(with, "\n# T\n\n## Fix\nDo it.\n\n## Why archived\nRetired.\n");
        assert_eq!(without_section(&with, "Why archived"), "\n# T\n\n## Fix\nDo it.\n");
        let middle = "\n# T\n\n## Why archived\nx\n\n## Fix\nDo it.\n";
        assert_eq!(without_section(middle, "Why archived"), "\n# T\n\n## Fix\nDo it.\n");
        assert_eq!(relative("general/cpp/a.md", "general/cpp/b.md"), "b.md");
        assert_eq!(relative("general/cpp/a.md", "general/git/b.md"), "../git/b.md");
        assert_eq!(relative("projects/x/cmake/a.md", "general/cmake/b.md"), "../../../general/cmake/b.md");
    }

    #[test]
    fn ids_are_plain_yaml_strings() {
        assert!(!plain_yaml_string("1311911595") && !plain_yaml_string("0000000001"));
        assert!(plain_yaml_string("7f3a9c2b41") && plain_yaml_string("12e4567890"));
        let taken = HashSet::new();
        assert!((0..2000).all(|_| plain_yaml_string(&new_id(&taken))));
    }

    #[test]
    fn ids_and_hash() {
        let id = new_id(&HashSet::new());
        assert_eq!(id.len(), 10);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(content_hash(b"abc"), "ba7816bf8f01");
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("CMake cannot find HDF5 unless HDF5_ROOT is set"), "cmake-cannot-find-hdf5-unless-hdf5-root-is-set");
        let long = slug(&"word ".repeat(30));
        assert!(long.len() <= 60 && !long.ends_with('-') && long.ends_with("word"));
        assert_eq!(slug("!!!"), "lesson");
    }

    #[test]
    fn title_similarity() {
        assert!(jaccard("Avoid std::regex in hot loops", "Avoid std regex in hot loops too") >= SIMILAR_TITLE);
        assert!(jaccard("Avoid std::regex in hot loops", "Link libstdc++ statically on old clusters") < SIMILAR_TITLE);
    }

    #[test]
    fn scopes_and_closest_topics() {
        let when = |y: &str| -> Mapping { serde_norway::from_str(y).unwrap() };
        assert_eq!(scope_of(&when("project: dftracer\nsystem: tuolumne")), "projects/dftracer");
        assert_eq!(scope_of(&when("system: tuolumne")), "systems/tuolumne");
        assert_eq!(scope_of(&when("project: [a, b]")), "general");
        assert_eq!(scope_of(&Mapping::new()), "general");
        let names = vec!["cmake".to_string(), "cpp".into(), "git".into(), "lustre".into()];
        assert_eq!(closest("cmak", &names), ["cmake", "cpp", "git"]);
    }

    #[test]
    fn label_order() {
        let kb: KbConfig = config::parse(config::DEFAULT_KB_TOML).unwrap();
        let m = |v: &str| BTreeMap::from([("sensitivity".to_string(), v.to_string())]);
        assert_eq!(looser(&kb, &m("internal"), &m("public")), Some(("sensitivity".into(), "public".into())));
        assert_eq!(looser(&kb, &m("internal"), &m("confidential")), None);
        assert_eq!(looser(&kb, &m("public"), &m("public")), None);
    }

    #[test]
    fn templates_have_required_headings() {
        let t = template(LessonType::Recipe);
        assert!(t.contains("type: recipe") && t.contains("## When to use") && t.contains("## Steps") && t.contains("## Evidence"));
        assert!(lesson::split(&t).is_ok());
    }

    #[test]
    fn section_text_is_compared() {
        let b = "# T\n\n## Evidence\nA\n\n## Check\nB\n";
        assert_eq!(body::section_text(b, "Evidence").unwrap(), "A\n");
        assert_eq!(body::section_text(b, "Check").unwrap(), "B");
    }
}
