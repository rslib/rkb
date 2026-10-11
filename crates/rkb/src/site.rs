//! `rkb publish`: the lessons the `web` sink allows, the knowledge base's rs-web template and the
//! site index, written as the input rs-web builds from.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rkb_core::leak::LeakScanner;
use rkb_core::request::{self, Action, Choice, Decision, Request};
use rkb_core::{kb, site};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::output::{CliError, ErrorCode, Output, paint};
use crate::writes::{self, Env};

/// Staged files by path in the output folder.
type Staged = BTreeMap<String, Vec<u8>>;

fn io_error(what: impl std::fmt::Display, fix: &str) -> CliError {
    CliError::new(ErrorCode::Io, what.to_string(), fix)
}

fn kb_config(env: &Env) -> rkb_core::config::KbConfig {
    std::fs::read_to_string(env.root.join("kb.toml")).ok().and_then(|t| rkb_core::config::parse(&t).ok()).unwrap_or_default()
}

/// Adds `clear` ids and `encrypted` ids with their groups to `site/published.json` and commits it
/// when it changed; a lesson now public no longer counts as encrypted only.
fn record(env: &Env, clear: &[String], encrypted: &[(String, String)]) -> Result<(), CliError> {
    let _lock = rkb_core::write::kb_lock(&env.root, &kb_config(env))?;
    let path = env.root.join(site::RECORD);
    let on_disk = site::read_published(&path);
    let mut p = site::published(&env.root, &env.state);
    p.clear.extend(clear.iter().cloned());
    p.encrypted.extend(encrypted.iter().cloned());
    p.encrypted.retain(|i, _| !p.clear.contains(i) || encrypted.iter().any(|(e, _)| e == i));
    if p == on_disk {
        return Ok(());
    }
    let added =
        p.clear.difference(&on_disk.clear).count() + p.encrypted.iter().filter(|(i, g)| on_disk.encrypted.get(*i) != Some(*g)).count();
    let fix = "check that the knowledge base is writable";
    let text = serde_json::to_string_pretty(&json!({ "clear": p.clear, "encrypted": p.encrypted })).expect("the record serializes") + "\n";
    std::fs::create_dir_all(env.root.join("site")).map_err(|e| io_error(e, fix))?;
    let tmp = env.root.join(format!("site/.published.json.{}", std::process::id()));
    std::fs::write(&tmp, text).map_err(|e| io_error(format!("{}: {e}", tmp.display()), fix))?;
    std::fs::rename(&tmp, &path).map_err(|e| io_error(format!("{}: {e}", path.display()), fix))?;
    rkb_core::git::run(&env.root, &["add", "--", site::RECORD])?;
    if !rkb_core::git::run(&env.root, &["status", "--porcelain", "--", site::RECORD])?.is_empty() {
        rkb_core::git::commit_paths(&env.root, &format!("site: publish {added} lessons"), &[site::RECORD])?;
    }
    Ok(())
}

/// The output folder's files: the knowledge base's template, the allowed lessons and their images,
/// and `site.json`.
fn stage_files(env: &Env, s: &site::Site) -> Result<Staged, CliError> {
    let fix = "check that the knowledge base is readable";
    let mut files = Staged::new();
    let custom = env.root.join("site");
    for p in walk(&custom) {
        let rel = p.strip_prefix(&custom).expect("walk stays under the folder").to_string_lossy().into_owned();
        files.insert(rel, std::fs::read(&p).map_err(|e| io_error(e, fix))?);
    }
    for (path, data) in &s.files {
        let data = staged(path, data)?;
        // The Lua cannot read binary files, so a protected image also goes in as text to embed.
        if path.starts_with("protected/") && path.contains(".assets/") {
            files.insert(format!("{path}.b64"), rkb_core::image::base64(&data).into_bytes());
        }
        files.insert(path.clone(), data);
    }
    let mut index = s.index.clone();
    // `[site] image`: staged next to the template's static files, its metadata removed.
    if let Some(img) = kb_config(env).site.image {
        let ext = img.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        let fix = "set [site] image in kb.toml to a png or jpg file in the knowledge base (`rkb lint` checks it)";
        let data = std::fs::read(env.root.join(&img)).map_err(|e| io_error(format!("[site] image {img}: {e}"), fix))?;
        let data = rkb_core::image::strip(&ext, &data)
            .map_err(|e| CliError::new(ErrorCode::Refused, format!("[site] image {img}: {e}; nothing was written"), fix))?;
        let name = format!("og-site.{ext}");
        files.insert(format!("static/{name}"), data);
        index["site"]["image"] = json!(name);
    }
    // Stylesheets and scripts also go under a content hash, so they can be cached for good. Vendored
    // `.min.js` files keep their plain path: site.js loads the Argon2 build by name.
    let mut assets = serde_json::Map::new();
    for (rel, data) in &files {
        let Some(name) = rel.strip_prefix("static/").filter(|n| !n.contains('/')) else { continue };
        if (name.ends_with(".css") || name.ends_with(".js")) && !name.ends_with(".min.js") {
            let hash: String = Sha256::digest(data).iter().take(6).map(|b| format!("{b:02x}")).collect();
            assets.insert(name.replace('.', "_"), json!(format!("/static/v/{hash}/{name}")));
        }
    }
    index["assets"] = Value::Object(assets);
    files.insert("site.json".into(), serde_json::to_string_pretty(&index).expect("the index serializes").into_bytes());
    Ok(files)
}

/// Brings `dir` in line with `files`, writing only what differs and removing what is gone, and keeps
/// rs-web's output folder. Returns whether anything changed.
fn stage(dir: &Path, files: &Staged) -> Result<bool, CliError> {
    let fix = "check the disk space and that the output folder is writable";
    let mut changed = false;
    for (rel, data) in files {
        let p = dir.join(rel);
        if std::fs::read(&p).is_ok_and(|old| old == *data) {
            continue;
        }
        std::fs::create_dir_all(p.parent().expect("staged files are in a folder")).map_err(|e| io_error(e, fix))?;
        std::fs::write(&p, data).map_err(|e| io_error(format!("{}: {e}", p.display()), fix))?;
        changed = true;
    }
    for p in walk(dir) {
        let rel = p.strip_prefix(dir).expect("walk stays under the folder").to_string_lossy().into_owned();
        if !rel.starts_with("dist/") && !files.contains_key(&rel) {
            let _ = std::fs::remove_file(&p);
            changed = true;
        }
    }
    Ok(changed)
}

/// A staged file as it may be published: a PNG, JPEG or WebP image without its metadata.
fn staged(path: &str, data: &[u8]) -> Result<Vec<u8>, CliError> {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    if !path.contains(".assets/") || !["png", "jpg", "jpeg", "webp"].contains(&ext.as_str()) {
        return Ok(data.to_vec());
    }
    let kb_path = path.split_once('/').map_or(path, |(_, p)| p);
    rkb_core::image::strip(&ext, data).map_err(|e| {
        CliError::new(
            ErrorCode::Refused,
            format!("{kb_path}: {e}; its metadata cannot be removed, so it is not published"),
            "open the image and save it again as a plain PNG, JPEG or WebP; nothing was written",
        )
    })
}

/// Output text files the checks read.
const TEXT: [&str; 8] = ["html", "xml", "json", "js", "css", "txt", "md", "svg"];

/// Vendored files ship as they are, so a minified token must not look like a finding.
fn vendored(rel: &str) -> bool {
    rel.ends_with(".min.js") || rel.starts_with("static/fonts/")
}

/// Removes the values of the encrypted-content attributes and of the `ciphertext`, `salt` and `nonce`
/// keys of an encrypted JSON file, so random base64 cannot look like a finding. A minified page may drop
/// the quotes around a value.
fn without_ciphertext(text: &str) -> String {
    const KEYS: [&str; 6] = ["data-encrypted=", "data-salt=", "data-nonce=", "\"ciphertext\":", "\"salt\":", "\"nonce\":"];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = KEYS.iter().filter_map(|a| rest.find(a).map(|i| i + a.len())).min() {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = match rest.strip_prefix('"') {
            Some(r) => r.find('"').map_or(rest.len(), |j| j + 2),
            None => rest.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(rest.len()),
        };
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// A protected lesson as the checks must never see it in plain text: its title, its knowledge-base
/// path and the words of its metadata.
struct Protected<'a> {
    id: &'a str,
    /// The lesson's knowledge-base path with `.md`, for a page that sits at it.
    path: &'a str,
    /// The same path without `.md`, which its staged images share.
    slug: &'a str,
    words: Vec<&'a str>,
}

/// The tags and queries of every lesson of one list, public or protected: a word a public lesson also
/// has is public knowledge, so it must not be a finding.
fn words_of(index: &Value, key: &str) -> Vec<String> {
    index[key]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|l| l["tags"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(str::to_owned)))
        .chain(
            index[key]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|l| l["queries"].as_array().into_iter().flatten().filter_map(|q| q.as_str().map(str::to_owned))),
        )
        .collect()
}

/// Leak findings in the built text files, lesson pages that belong to no published lesson, and
/// protected titles, tags and paths in plain text. A finding names a protected lesson by id only.
fn check_output(env: &Env, dir: &Path, dist: &Path, index: &Value) -> Vec<String> {
    let kb = kb_config(env);
    let scanner = LeakScanner::new(&kb.leak, env.lint.user.as_deref(), env.lint.home.as_deref());
    let mut public: Vec<String> = walk(&dir.join("lessons")).into_iter().filter_map(|p| std::fs::read_to_string(&p).ok()).collect();
    public.extend(words_of(index, "lessons"));
    public.extend(words_of(index, "protected"));
    let protected: Vec<Protected<'_>> = index["protected"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|l| {
            let path = l["path"].as_str().unwrap_or_default().strip_prefix("protected/").unwrap_or_default();
            let mut words: Vec<&str> = vec![l["title"].as_str().unwrap_or_default(), path];
            words.extend(l["tags"].as_array().into_iter().flatten().filter_map(|t| t.as_str()).filter(|t| t.chars().count() >= 4));
            words.extend(l["queries"].as_array().into_iter().flatten().filter_map(|q| q.as_str()).filter(|q| q.chars().count() >= 4));
            words.retain(|w| !public.iter().any(|text| text.contains(w)));
            Protected { id: l["id"].as_str().unwrap_or_default(), path, slug: path.strip_suffix(".md").unwrap_or(path), words }
        })
        .collect();
    let hidden: Vec<(String, Vec<u8>)> = walk(&dir.join("protected"))
        .into_iter()
        .filter_map(|p| {
            let rel = p.strip_prefix(dir).ok()?.to_string_lossy().into_owned();
            Some((rel, std::fs::read(&p).ok()?))
        })
        .collect();
    let public_ids: Vec<&str> = index["lessons"].as_array().into_iter().flatten().filter_map(|l| l["id"].as_str()).collect();
    let protected_ids: Vec<&str> = protected.iter().map(|p| p.id).collect();
    let mut out = vec![];
    for p in walk(dist) {
        let rel = p.strip_prefix(dist).expect("walk stays under the folder").to_string_lossy().into_owned();
        if p.extension().and_then(|e| e.to_str()).is_some_and(|e| TEXT.contains(&e))
            && let Ok(text) = std::fs::read_to_string(&p)
        {
            // The vendored Argon2 build and font files ship as is; minified tokens can look like job ids.
            if !vendored(&rel) {
                let text = without_ciphertext(&text);
                for m in scanner.scan(&text) {
                    out.push(format!("{rel}: {} `{}`", m.kind.rule(), m.text));
                }
                for x in &protected {
                    if x.words.iter().any(|w| text.contains(w)) {
                        out.push(format!("{rel}: shows protected lesson `{}` in plain text", x.id));
                    }
                }
            }
        }
        if let Ok(bytes) = std::fs::read(&p)
            && let Some((img, _)) = hidden.iter().find(|(_, d)| !d.is_empty() && *d == bytes)
        {
            let id = protected.iter().find(|x| img.contains(x.slug)).map_or("?", |x| x.id);
            out.push(format!("{rel}: an image of protected lesson `{id}` as a plain file"));
        }
        if let Some(x) = protected.iter().find(|x| rel.starts_with(&format!("{}/", x.path.strip_suffix(".md").unwrap_or(x.path)))) {
            out.push(format!("{rel}: a page at the path of protected lesson `{}`", x.id));
        }
        for (prefix, ids, what) in [("lessons/", &public_ids, "public"), ("protected/", &protected_ids, "protected")] {
            if let Some(id) = rel.strip_prefix(prefix).and_then(|r| r.split('/').next()).filter(|id| !id.contains('.'))
                && !ids.contains(&id)
            {
                out.push(format!("{rel}: a {what} lesson page for `{id}`, which is not published that way"));
            }
        }
    }
    out.dedup();
    out
}

/// The staged files for a template the checks read: `site.json` of the folder, by path.
fn index_of(dir: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(dir.join("site.json")).ok()?).ok()
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() { todo.push(p) } else { out.push(p) }
        }
    }
    out.sort();
    out
}

/// Minimum length of `SITE_PASSWORD` and each group password. Pages can be guessed at offline, so a
/// password must be long.
pub const MIN_PASSWORD: usize = 16;

/// Ids the user agreed to publish for the first time, in the clear and encrypted.
#[derive(Default)]
pub struct Approved {
    pub clear: Vec<String>,
    pub encrypted: Vec<String>,
}

/// Refuses a `SITE_PASSWORD` or `SITE_PASSWORD_<GROUP>` that is set but too short.
fn check_passwords() -> Result<(), CliError> {
    for (name, value) in std::env::vars() {
        let n = value.chars().count();
        if (name == "SITE_PASSWORD" || name.starts_with("SITE_PASSWORD_")) && n > 0 && n < MIN_PASSWORD {
            return Err(CliError::new(
                ErrorCode::Refused,
                format!("{name} has {n} characters; protected pages need at least {MIN_PASSWORD}"),
                format!(
                    "use a long random password, such as the output of `openssl rand -base64 24`, or unset {name} to leave its lessons out"
                ),
            ));
        }
    }
    Ok(())
}

fn password_set(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| !v.is_empty())
}

fn listed(index: &Value, key: &str, ids: &[String]) -> Vec<String> {
    index[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|l| ids.iter().any(|i| l["id"] == *i))
        .map(|l| format!("  {} {}", l["id"].as_str().unwrap_or(""), l["title"].as_str().unwrap_or("")))
        .collect()
}

/// `rkb publish <dir>`: the knowledge base's template and the lessons the publish gate allows, as the
/// input rs-web builds from, without running rs-web or touching an earlier rs-web output. `approved`
/// holds the ids the user just agreed to publish. With `no_ask`, a first publication leaves the
/// lessons out instead of asking, and the knowledge base is not written.
pub fn run(env: &Env, dir: String, approved: &Approved, no_ask: bool) -> Result<Output, CliError> {
    kb::open(&env.root)?;
    check_passwords()?;
    let mut s = site::collect(&env.root, password_set)?;
    if !env.root.join("site").join("config.lua").is_file() {
        return Err(CliError::new(
            ErrorCode::NotFound,
            "the knowledge base has no rs-web template, so nothing can be published",
            format!("add site/config.lua with the templates/ and static/ it reads, then run `rkb publish {dir}` again"),
        ));
    }
    // Staging removes every file it did not write, so it only works in an empty folder or its own.
    if !Path::new(&dir).join("site.json").is_file() && std::fs::read_dir(&dir).is_ok_and(|mut d| d.next().is_some()) {
        return Err(CliError::new(
            ErrorCode::Refused,
            format!("{dir} has files but no site.json, so it is not a folder `rkb publish` wrote"),
            "give `rkb publish` an empty or new folder; nothing was written",
        ));
    }
    let before = site::published(&env.root, &env.state);
    let (mut new_clear, mut new_enc) = site::pending(&s, &before);
    new_clear.retain(|i| !approved.clear.contains(i));
    new_enc.retain(|i| !approved.encrypted.contains(i));
    let mut held_note = None;
    if no_ask && (!new_clear.is_empty() || !new_enc.is_empty()) {
        let fix = "run `rkb publish` on your machine, answer `publish`, and push the knowledge base";
        if !env.root.join(site::RECORD).is_file() {
            return Err(CliError::new(
                ErrorCode::Refused,
                format!(
                    "{} lessons wait for a first-publication yes, and the knowledge base has no {}",
                    new_clear.len() + new_enc.len(),
                    site::RECORD
                ),
                format!("{fix}; nothing was written"),
            ));
        }
        let wait: std::collections::BTreeSet<String> = new_clear.iter().chain(&new_enc).cloned().collect();
        let titles = s.index.clone();
        s = site::collect_holding(&env.root, password_set, &wait)?;
        let names: Vec<String> = s
            .held
            .iter()
            .map(|i| {
                let row = ["lessons", "protected"].iter().flat_map(|k| titles[*k].as_array().into_iter().flatten()).find(|l| l["id"] == *i);
                let title = row.and_then(|l| l["title"].as_str()).unwrap_or_default();
                if wait.contains(i) { format!("{i} {title}") } else { format!("{i} {title} (links to one)") }
            })
            .collect();
        if s.ids.is_empty() && s.protected.is_empty() {
            return Err(CliError::new(
                ErrorCode::Refused,
                format!("every lesson waits for a first-publication yes, so nothing would be published: {}", names.join("; ")),
                format!("{fix}; nothing was written"),
            ));
        }
        held_note = Some(format!("{} lessons were left out until the user says yes: {}; {fix}", names.len(), names.join("; ")));
    } else if !new_clear.is_empty() || !new_enc.is_empty() {
        let n = new_clear.len() + new_enc.len();
        let mut list = String::new();
        if !new_clear.is_empty() {
            list.push_str(&format!("\nIn the clear:\n{}", listed(&s.index, "lessons", &new_clear).join("\n")));
        }
        let mut by_env: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for i in &new_enc {
            by_env.entry(site::password_env(s.groups.get(i).cloned().flatten().as_deref())).or_default().push(i.clone());
        }
        for (var, ids) in &by_env {
            list.push_str(&format!("\nEncrypted with {var}:\n{}", listed(&s.index, "protected", ids).join("\n")));
        }
        let question = format!("Publish {n} lessons for the first time?{list}");
        let req = Request {
            id: request::new_id(),
            created: request::now(),
            action: Action::Publish { ids: new_clear, encrypted: new_enc, out: dir },
            approved: vec![],
            question,
            choices: vec![
                Choice { text: "publish".into(), decision: Some(Decision::Publish) },
                Choice { text: "cancel".into(), decision: None },
            ],
            session: None,
        };
        request::save(&env.state.join("requests"), &req)?;
        return writes::needs_user_output(env, &req);
    }
    let gone: Vec<String> = before
        .clear
        .iter()
        .filter(|i| !s.ids.contains(i))
        .chain(
            before
                .encrypted
                .iter()
                .filter(|(i, g)| !s.ids.contains(i) && (!s.protected.contains(i) || **g != site::group_name(&s, i)))
                .map(|(i, _)| i),
        )
        .cloned()
        .collect();
    let dir = PathBuf::from(dir);
    stage(&dir, &stage_files(env, &s)?)?;
    let mut notes: Vec<String> = vec![];
    if !gone.is_empty() {
        notes.push(format!(
            "{} lessons published before are no longer published the same way ({}); the old web copy, feeds and caches may still hold them",
            gone.len(),
            gone.join(", ")
        ));
    }
    for (var, n) in &s.left_out {
        let what = if var == "SITE_PASSWORD" { "internal lessons" } else { "lessons" };
        notes.push(format!("{n} {what} were left out; set {var} ({MIN_PASSWORD}+ characters) to publish them encrypted"));
    }
    for (folder, n) in &s.defaulted {
        notes.push(format!(
            "{n} lesson(s) in {folder} are internal by default: neither they nor a folder above them set sensitivity. If they should be public, run `rkb label {folder} sensitivity=public`"
        ));
    }
    notes.extend(held_note);
    // Under GitHub Actions the notes also show as warnings on the run.
    if std::env::var("GITHUB_ACTIONS").is_ok_and(|v| v == "true") {
        for n in &notes {
            eprintln!("::warning title=rkb publish::{}", n.replace('%', "%25").replace('\r', "%0D").replace('\n', "%0A"));
        }
    }
    let count = s.ids.len() + s.protected.len();
    if !no_ask {
        let enc: Vec<(String, String)> = s.protected.iter().map(|i| (i.clone(), site::group_name(&s, i))).collect();
        record(env, &s.ids, &enc)?;
    }
    let mut human =
        format!("{} {count} lessons ({} protected) to {}", paint(env.colored, "32", "published"), s.protected.len(), dir.display());
    for n in &notes {
        human.push_str(&format!("\n{} {n}", paint(env.colored, "33", "note:")));
    }
    let data = json!({
        "status": "published",
        "out": dir.display().to_string(),
        "lessons": count,
        "protected": s.protected.len(),
        "notes": notes,
        "help": [format!("Run `rs-web build` in {} for the site, then `rkb publish --check {}`", dir.display(), dir.display())],
    });
    Ok(Output { data, human, exit: 0, raw: false })
}

/// `rkb publish --check <dir>`: the leak scan, the page checks and the protected-text checks on the
/// rs-web output in `<dir>/dist`, against the site index publish left in the folder.
pub fn check(env: &Env, dir: &str) -> Result<Output, CliError> {
    kb::open(&env.root)?;
    let dir = PathBuf::from(dir);
    let dist = dir.join("dist");
    if !dist.is_dir() {
        return Err(CliError::new(
            ErrorCode::NotFound,
            format!("{} has no rs-web output", dist.display()),
            format!("run `rs-web build` in {} first, then `rkb publish --check {}`", dir.display(), dir.display()),
        ));
    }
    let Some(index) = index_of(&dir) else {
        return Err(CliError::new(
            ErrorCode::NotFound,
            format!("{} has no site.json", dir.join("site.json").display()),
            format!("run `rkb publish {}` first, then `rs-web build` in {}", dir.display(), dir.display()),
        ));
    };
    let findings = check_output(env, &dir, &dist, &index);
    let files = walk(&dist).len();
    if !findings.is_empty() {
        return Err(CliError::new(
            ErrorCode::Refused,
            format!("the built site failed its checks:\n{}", findings.join("\n")),
            format!("fix the lesson or the template, then run `rs-web build` in {} again", dir.display()),
        ));
    }
    let human = format!("{} 0 findings in {} files under {}", paint(env.colored, "32", "checked"), files, dist.display());
    let data = json!({ "status": "ok", "findings": 0, "files": files, "notes": [] });
    Ok(Output { data, human, exit: 0, raw: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ciphertext_is_skipped_with_or_without_quotes() {
        assert_eq!(
            without_ciphertext(r#"<div data-encrypted="QUJD" data-nonce=Tm9u>x</div>"#),
            r#"<div data-encrypted= data-nonce=>x</div>"#
        );
        assert_eq!(without_ciphertext("a data-salt=abc"), "a data-salt=");
        assert_eq!(without_ciphertext(r#"{"ciphertext":"QUJD","salt":"c2FsdA==","nonce":"bm9u"}"#), r#"{"ciphertext":,"salt":,"nonce":}"#);
    }

    #[test]
    fn stage_writes_only_what_differs_and_keeps_the_rs_web_output() {
        let dir = tempfile::tempdir().unwrap();
        let files = Staged::from([("site.json".to_string(), b"{}".to_vec())]);
        assert!(stage(dir.path(), &files).unwrap());
        assert_eq!(std::fs::read_to_string(dir.path().join("site.json")).unwrap(), "{}");
        assert!(!stage(dir.path(), &files).unwrap(), "the second stage changes nothing");
        std::fs::create_dir_all(dir.path().join("dist")).unwrap();
        std::fs::write(dir.path().join("dist/index.html"), "built").unwrap();
        std::fs::write(dir.path().join("gone.txt"), "x").unwrap();
        assert!(stage(dir.path(), &files).unwrap(), "a file that is no longer staged is removed");
        assert!(!dir.path().join("gone.txt").exists());
        assert_eq!(std::fs::read_to_string(dir.path().join("dist/index.html")).unwrap(), "built");
    }

    #[test]
    fn walk_reads_every_file_under_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("a/b/c.txt"), "x").unwrap();
        std::fs::write(dir.path().join("d.txt"), "y").unwrap();
        let found: Vec<String> =
            walk(dir.path()).iter().map(|p| p.strip_prefix(dir.path()).unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(found, ["a/b/c.txt", "d.txt"]);
    }
}
