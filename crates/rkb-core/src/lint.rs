use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use serde_norway::Value;

use crate::body::{self, Body};
use crate::config::{self, KbConfig, ProjectNote, SystemNote, TopicNote};
use crate::error::Result;
use crate::git;
use crate::kb::{FileKind, NoteRole, SCOPES, Snapshot, classify, lesson_depth, note_role, topic_of};
use crate::leak::LeakScanner;
use crate::lesson::{self, Lesson, Status, VerifiedHow};
use crate::yaml;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Finding {
    pub path: String,
    pub line: Option<usize>,
    pub severity: Severity,
    pub rule: &'static str,
    pub message: String,
}

/// Values from the environment that the leak scan treats as personal.
#[derive(Debug, Default, Clone)]
pub struct LintEnv {
    pub user: Option<String>,
    pub home: Option<String>,
}

impl LintEnv {
    pub fn from_process() -> Self {
        LintEnv { user: std::env::var("USER").ok(), home: std::env::var("HOME").ok() }
    }
}

pub const MAX_IMAGE_BYTES: usize = 1024 * 1024;
pub const MAX_LESSON_LINES: usize = 150;
const IMAGE_EXTS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "svg"];

static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{10}$").unwrap());
static FOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9]+(-[a-z0-9]+)*$").unwrap());

struct Out(Vec<Finding>);

impl Out {
    fn push(&mut self, path: &str, line: Option<usize>, severity: Severity, rule: &'static str, message: String) {
        self.0.push(Finding { path: path.to_string(), line, severity, rule, message });
    }
    fn error(&mut self, path: &str, line: Option<usize>, rule: &'static str, message: String) {
        self.push(path, line, Severity::Error, rule, message);
    }
    fn warning(&mut self, path: &str, line: Option<usize>, rule: &'static str, message: String) {
        self.push(path, line, Severity::Warning, rule, message);
    }
}

/// Lints the working tree.
pub fn lint_tree(root: &Path, env: &LintEnv) -> Result<Vec<Finding>> {
    Ok(lint(&Snapshot::from_dir(root)?, env, None))
}

/// Lints the git index: per-file rules for staged files, global rules for the whole staged tree.
pub fn lint_staged(root: &Path, env: &LintEnv) -> Result<Vec<Finding>> {
    let snap = Snapshot::from_index(root)?;
    let staged: BTreeSet<String> = if git::has_head(root) {
        git::run(root, &["diff", "--cached", "--name-only", "-z", "--diff-filter=ACMR"])?
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect()
    } else {
        snap.files.keys().cloned().collect()
    };
    let mut findings = lint(&snap, env, Some(&staged));

    let staged_lessons: Vec<&String> = staged.iter().filter(|p| classify(p) == FileKind::Lesson && snap.files.contains_key(*p)).collect();
    let specs: Vec<String> = staged_lessons.iter().map(|p| format!("HEAD:{p}")).collect();
    let heads = if git::has_head(root) { git::read_blobs(root, &specs)? } else { vec![None; specs.len()] };
    for (path, head) in staged_lessons.into_iter().zip(heads) {
        let checked = |data: &[u8]| {
            lesson::parse(path, &String::from_utf8_lossy(data)).is_ok_and(|l| l.frontmatter.verified_how == VerifiedHow::Checked)
        };
        if checked(&snap.files[path]) && !head.as_deref().is_some_and(checked) {
            findings.push(Finding {
                path: path.clone(),
                line: None,
                severity: Severity::Error,
                rule: "format/checked-by-hand",
                message: "only `rkb verify` may set `verified_how: checked`".into(),
            });
        }
    }
    findings.sort();
    Ok(findings)
}

/// Rewrites flow-style frontmatter in block style in the working tree and returns the changed paths.
/// Files whose frontmatter does not parse are left for `rkb lint` to report.
pub fn fix_flow_style(root: &Path) -> Result<Vec<String>> {
    let snap = Snapshot::from_dir(root)?;
    let mut fixed = vec![];
    for (path, data) in &snap.files {
        if !matches!(classify(path), FileKind::Lesson | FileKind::FolderNote) {
            continue;
        }
        let text = String::from_utf8_lossy(data);
        let flow = config::note_frontmatter(&text).is_some_and(|fm| !yaml::flow_lines(fm).is_empty());
        let Some(new) = flow.then(|| yaml::fix_frontmatter(&text)).flatten() else { continue };
        let abs = root.join(path);
        let tmp = abs.with_file_name(format!(".{}.rkb-tmp", abs.file_name().unwrap().to_string_lossy()));
        std::fs::write(&tmp, new).map_err(crate::error::io(&tmp))?;
        std::fs::rename(&tmp, &abs).map_err(crate::error::io(&abs))?;
        fixed.push(path.clone());
    }
    Ok(fixed)
}

/// Runs every rule. With `focus`, per-file rules run only on those paths.
pub fn lint(snap: &Snapshot, env: &LintEnv, focus: Option<&BTreeSet<String>>) -> Vec<Finding> {
    let mut out = Out(vec![]);
    let in_focus = |p: &str| focus.is_none_or(|f| f.contains(p));

    // Without a valid kb.toml the allowed labels are unknown, so label checks are skipped.
    let kb: Option<KbConfig> = match snap.files.get("kb.toml") {
        None => {
            out.error("kb.toml", None, "config/missing", "the knowledge base has no kb.toml".into());
            None
        }
        Some(data) => config::parse(&String::from_utf8_lossy(data)).map_err(|e| out.error("kb.toml", None, "config/invalid", e)).ok(),
    };
    // The file itself usually sits in site/, which lint does not read; `rkb site build` reports it missing.
    if let Some(img) = kb.as_ref().and_then(|k| k.site.image.as_deref()) {
        let ext = img.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        if !["png", "jpg", "jpeg"].contains(&ext.as_str()) {
            out.error("kb.toml", None, "config/site-image", format!("[site] image `{img}` must be a png or jpg file"));
        }
    }
    if let Some(a) = kb.as_ref().and_then(|k| k.site.analytics.as_deref())
        && a != "cloudflare"
    {
        out.error("kb.toml", None, "config/site-analytics", format!("[site] analytics `{a}` is not known; the only value is `cloudflare`"));
    }
    let leak = kb.as_ref().map(|k| &k.leak).cloned().unwrap_or_default();
    let scanner = LeakScanner::new(&leak, env.user.as_deref(), env.home.as_deref());

    let mut aliases: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut linked: BTreeSet<String> = BTreeSet::new();
    for (path, data) in snap.of_kind(FileKind::FolderNote) {
        let text = String::from_utf8_lossy(data);
        let parsed = match note_role(path) {
            NoteRole::Project => config::parse_note::<ProjectNote>(&text).map(|n| (n.labels, vec![])),
            NoteRole::System => config::parse_note::<SystemNote>(&text).map(|n| (n.labels, vec![])),
            NoteRole::Topic => config::parse_note::<TopicNote>(&text).map(|n| (n.labels, n.aliases)),
            NoteRole::Scope => config::parse_note::<TopicNote>(&text).map(|n| (n.labels, vec![])),
            NoteRole::Misplaced => {
                out.error(path, None, "layout/depth", "no lesson folder is this deep, so this note describes nothing".into());
                continue;
            }
        };
        match parsed {
            Ok((labels, names)) => {
                if in_focus(path) {
                    for (k, v) in &labels {
                        check_label(&mut out, kb.as_ref(), path, None, k, v);
                    }
                }
                let topic = path.rsplit('/').nth(1).unwrap().to_string();
                for alias in names {
                    aliases.entry(alias).or_default().push((topic.clone(), path.clone()));
                }
            }
            Err(e) if in_focus(path) => out.error(path, None, "config/invalid", format!("frontmatter: {e}")),
            Err(_) => {}
        }
        check_links(&mut out, snap, path, 0, &body::scan(&text), &mut linked);
    }

    let (lessons, bad) = snap.lessons();
    for (path, e) in bad {
        if in_focus(&path) {
            out.error(&path, e.line, "format/parse", e.message);
        }
    }

    let mut ids: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for l in &lessons {
        ids.entry(&l.frontmatter.id).or_default().push(&l.path);
        let b = body::scan(&l.body);
        check_links(&mut out, snap, &l.path, l.body_line - 1, &b, &mut linked);
        if in_focus(&l.path) {
            lint_lesson(&mut out, kb.as_ref(), l, &b);
            lint_scope(&mut out, l);
        }
    }

    for (path, data) in &snap.files {
        let kind = classify(path);
        let scan_text = matches!(kind, FileKind::Lesson | FileKind::FolderNote) || (kind == FileKind::Asset && path.ends_with(".svg"));
        if matches!(kind, FileKind::Lesson | FileKind::FolderNote) && in_focus(path) {
            let text = String::from_utf8_lossy(data);
            if let Some(fm) = config::note_frontmatter(&text) {
                for line in yaml::flow_lines(fm) {
                    out.warning(
                        path,
                        Some(line + 1),
                        "format/flow-style",
                        "write frontmatter in block style; `rkb lint --fix` rewrites it".into(),
                    );
                }
            }
        }
        if scan_text && in_focus(path) {
            for m in scanner.scan(&String::from_utf8_lossy(data)) {
                out.error(path, Some(m.line), m.kind.rule(), format!("`{}` looks like personal or site data", m.text));
            }
        }
        if kind == FileKind::Asset {
            lint_asset(&mut out, path, data.len(), linked.contains(path), in_focus(path));
        }
    }

    for (id, paths) in &ids {
        if paths.len() > 1 {
            for p in paths {
                let others: Vec<&str> = paths.iter().copied().filter(|o| o != p).collect();
                out.error(p, None, "format/duplicate-id", format!("id {id} is also used by {}", others.join(", ")));
            }
        }
    }

    lint_near_duplicates(&mut out, kb.as_ref(), &lessons, &in_focus);
    lint_folders(&mut out, snap, &aliases);
    let mut findings = out.0;
    findings.sort();
    findings
}

/// Warns once per pair, on the path that sorts first; with a focus, only pairs that touch it.
fn lint_near_duplicates(out: &mut Out, kb: Option<&KbConfig>, lessons: &[Lesson], in_focus: &dyn Fn(&str) -> bool) {
    let set: Vec<&Lesson> = lessons.iter().filter(|l| crate::graph::compared(l)).collect();
    let from: Vec<usize> = (0..set.len()).filter(|&i| in_focus(&set[i].path)).collect();
    if from.is_empty() {
        return;
    }
    for (a, b, s) in crate::graph::Vectors::new(&set).pairs(&from, crate::graph::min_from(kb)) {
        let (first, other) = if set[a].path < set[b].path { (set[a], set[b]) } else { (set[b], set[a]) };
        out.warning(
            &first.path,
            None,
            "content/near-duplicate",
            format!("similarity {s:.2} with {} ({}); see `rkb dupes`", other.frontmatter.id, other.path),
        );
    }
}

fn check_links(out: &mut Out, snap: &Snapshot, path: &str, offset: usize, b: &Body, linked: &mut BTreeSet<String>) {
    for link in &b.links {
        match resolve(path, &link.target) {
            Some(Some(target)) if exists(snap, &target) => {
                linked.insert(target);
            }
            Some(_) => out.error(path, Some(offset + link.line), "links/broken", format!("link target `{}` does not exist", link.target)),
            None => {}
        }
    }
}

/// A single `when.project` or `when.system` value, which decides the lesson's scope folder.
fn single(l: &Lesson, key: &str) -> Option<String> {
    l.frontmatter.when.get(key).and_then(Value::as_str).map(str::to_string)
}

fn lint_scope(out: &mut Out, l: &Lesson) {
    let parts: Vec<&str> = l.path.split('/').collect();
    if parts.len() != lesson_depth(parts[0]) {
        let expected = match parts[0] {
            "general" => "general/<topic>/<file>.md",
            "projects" => "projects/<project>/<topic>/<file>.md",
            _ => "systems/<system>/<topic>/<file>.md",
        };
        out.error(&l.path, None, "layout/depth", format!("a lesson here must be at {expected}"));
    }
    let (project, system) = (single(l, "project"), single(l, "system"));
    let folder = parts.get(1).copied().unwrap_or("");
    let wrong = match parts[0] {
        "projects" => project.filter(|p| p != folder).map(|p| format!("when.project is `{p}`, so move it to projects/{p}/")),
        "systems" => system.filter(|s| s != folder).map(|s| format!("when.system is `{s}`, so move it to systems/{s}/")),
        _ => project
            .map(|p| format!("when.project is `{p}`, so move it to projects/{p}/"))
            .or_else(|| system.map(|s| format!("when.system is `{s}`, so move it to systems/{s}/"))),
    };
    if let Some(msg) = wrong {
        out.error(&l.path, None, "layout/scope", msg);
    }
}

fn exists(snap: &Snapshot, path: &str) -> bool {
    snap.files.contains_key(path) || snap.files.range(format!("{path}/")..).next().is_some_and(|(k, _)| k.starts_with(&format!("{path}/")))
}

/// Resolves a link target against the lesson's folder. `None` for links lint does not check,
/// `Some(None)` for a relative link that leaves the knowledge base.
pub(crate) fn resolve(from: &str, target: &str) -> Option<Option<String>> {
    if target.contains("://") || target.starts_with("mailto:") || target.starts_with('#') || target.starts_with('/') {
        return None;
    }
    let target = target.split(['#', '?']).next().unwrap_or("").replace("%20", " ");
    if target.is_empty() {
        return None;
    }
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Some(None);
                }
            }
            s => parts.push(s),
        }
    }
    Some(Some(parts.join("/")))
}

fn check_label(out: &mut Out, kb: Option<&KbConfig>, path: &str, line: Option<usize>, key: &str, value: &str) {
    let Some(kb) = kb else { return };
    match kb.labels.get(key) {
        None => out.error(path, line, "labels/unknown", format!("label `{key}` is not defined in kb.toml")),
        Some(allowed) if !allowed.iter().any(|a| a == value) => {
            out.error(path, line, "labels/unknown", format!("label {key} = `{value}` is not allowed; allowed: {}", allowed.join(", ")))
        }
        Some(_) => {}
    }
}

fn lint_lesson(out: &mut Out, kb: Option<&KbConfig>, l: &Lesson, b: &Body) {
    let fm = &l.frontmatter;
    let p = l.path.as_str();
    let at = |line: usize| Some(l.body_line - 1 + line);

    if fm.schema != 1 {
        out.error(p, None, "format/schema", format!("schema {} is not supported; expected 1", fm.schema));
    }
    if !ID.is_match(&fm.id) {
        out.error(p, None, "format/id", format!("id `{}` must be 10 lowercase hex characters", fm.id));
    }
    match (fm.status, &fm.superseded_by) {
        (Status::Superseded, None) => out.error(p, None, "format/superseded_by", "a superseded lesson needs `superseded_by`".into()),
        (Status::Superseded, Some(id)) if !ID.is_match(id) => {
            out.error(p, None, "format/superseded_by", format!("superseded_by `{id}` is not a lesson id"))
        }
        (s, Some(_)) if s != Status::Superseded => {
            out.error(p, None, "format/superseded_by", "only a superseded lesson has `superseded_by`".into())
        }
        _ => {}
    }
    match (fm.status, &fm.stale_reason) {
        (Status::Stale, None) => out.error(p, None, "format/stale_reason", "a stale lesson needs `stale_reason`".into()),
        (Status::Stale, Some(r)) if r.trim().is_empty() => out.error(p, None, "format/stale_reason", "`stale_reason` is empty".into()),
        (s, Some(_)) if s != Status::Stale => out.error(p, None, "format/stale_reason", "only a stale lesson has `stale_reason`".into()),
        _ => {}
    }
    for problem in crate::conditions::check(&fm.when) {
        out.error(p, None, "when/invalid", problem);
    }
    for tag in &fm.tags {
        if tag.is_empty() || tag.chars().any(|c| c.is_uppercase() || c.is_whitespace()) {
            out.error(p, None, "format/tags", format!("tag `{tag}` must be lowercase with no spaces"));
        }
    }
    for (k, v) in &fm.labels {
        match (k, v) {
            (Value::String(k), Value::String(v)) => check_label(out, kb, p, None, k, v),
            _ => out.error(p, None, "labels/unknown", "label keys and values must be strings".into()),
        }
    }

    match (b.first_line, b.h1.as_slice()) {
        (Some(first), [(line, _)]) if first == *line => {}
        (_, []) => out.error(p, at(1), "body/title", "the body must start with one `# Title` heading".into()),
        (_, [(line, _)]) => out.error(p, at(*line), "body/title", "the `# Title` heading must be the first line".into()),
        (_, [_, (line, _), ..]) => out.error(p, at(*line), "body/title", "the body has more than one H1 heading".into()),
    }

    let required = fm.kind.required_headings();
    let mut positions = vec![];
    for name in required.iter().copied().chain(fm.status.required_heading()) {
        let found: Vec<_> = b.sections.iter().filter(|s| s.heading == name).collect();
        match found.as_slice() {
            [] => out.error(p, None, "body/headings", format!("missing required heading `## {name}`")),
            [s] => {
                if !s.has_content {
                    out.error(p, at(s.line), "body/empty-section", format!("section `{name}` is empty"));
                }
                if required.contains(&name) {
                    positions.push((name, s.line));
                }
            }
            [_, dup, ..] => out.error(p, at(dup.line), "body/headings", format!("heading `## {name}` appears twice")),
        }
    }
    for pair in positions.windows(2) {
        let ((a, la), (bn, lb)) = (pair[0], pair[1]);
        if lb < la {
            out.error(p, at(lb), "body/heading-order", format!("`{bn}` comes before `{a}`; required order: {}", required.join(", ")));
        }
    }

    for f in &b.fences {
        if f.lang.is_empty() {
            out.error(p, at(f.line), "body/fence-lang", "a fenced code block needs a language tag".into());
        }
    }
    for line in &b.indented_code {
        out.error(p, at(*line), "body/indented-code", "use a fenced code block, not an indented one".into());
    }
    for s in b.sections.iter().filter(|s| s.heading == "Check" || s.heading == "Probe") {
        let ok = matches!(s.fences.as_slice(), [i] if matches!(b.fences[*i].lang.as_str(), "sh" | "bash"));
        if !ok {
            out.error(p, at(s.line), "body/script", format!("`{}` must hold exactly one fenced `sh` or `bash` block", s.heading));
        }
    }

    let lines = (l.body_line - 1) + l.body.lines().count();
    if lines > MAX_LESSON_LINES {
        out.warning(p, None, "size/lesson-lines", format!("{lines} lines; a lesson over {MAX_LESSON_LINES} lines is often two lessons"));
    }
}

fn lint_asset(out: &mut Out, path: &str, size: usize, linked: bool, focus: bool) {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    if focus && !IMAGE_EXTS.contains(&ext.as_str()) {
        out.error(path, None, "assets/type", format!("only images are allowed in .assets: {}", IMAGE_EXTS.join(", ")));
    }
    if focus && size > MAX_IMAGE_BYTES {
        out.error(path, None, "assets/size", format!("{size} bytes; the limit is 1 MB"));
    }
    if !linked {
        out.error(path, None, "assets/unlinked", "no lesson links to this file".into());
    }
}

fn lint_folders(out: &mut Out, snap: &Snapshot, aliases: &BTreeMap<String, Vec<(String, String)>>) {
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    let mut topics: BTreeSet<&str> = BTreeSet::new();
    for path in snap.files.keys() {
        if !SCOPES.iter().any(|s| path.starts_with(&format!("{s}/"))) {
            continue;
        }
        let parts: Vec<&str> = path.split('/').collect();
        for i in 2..parts.len() {
            if parts[i - 1].ends_with(".assets") {
                break;
            }
            dirs.insert(parts[..i].join("/"));
        }
        if classify(path) == FileKind::Lesson
            && let Some(t) = topic_of(path)
        {
            topics.insert(t);
        }
    }
    for dir in &dirs {
        let name = dir.rsplit('/').next().unwrap();
        if !name.ends_with(".assets") && !FOLDER.is_match(name) {
            out.error(dir, None, "layout/folder-name", format!("folder `{name}` must be lowercase letters, digits and hyphens"));
        }
    }
    for (alias, claims) in aliases {
        let mut owners: Vec<&str> = claims.iter().map(|(t, _)| t.as_str()).collect();
        owners.sort();
        owners.dedup();
        if owners.len() > 1 {
            for (_, note) in claims {
                out.error(note, None, "layout/alias", format!("alias `{alias}` is claimed by topics {}", owners.join(", ")));
            }
        }
        if topics.contains(alias.as_str()) {
            for dir in dirs.iter().filter(|d| d.rsplit('/').next() == Some(alias.as_str())) {
                out.error(
                    dir,
                    None,
                    "layout/alias",
                    format!("`{alias}` is an alias of topic `{}`; move these lessons to `{}`", owners[0], owners[0]),
                );
            }
        }
    }
}
