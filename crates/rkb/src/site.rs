//! `rkb site`: build, preview and customize an rs-web site of the lessons the `web` sink allows.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use rkb_core::leak::LeakScanner;
use rkb_core::request::{self, Action, Choice, Decision, Request};
use rkb_core::{kb, paths, site};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::output::{CliError, ErrorCode, Output, paint};
use crate::writes::{self, Env};

/// The rs-web release this rkb downloads and its template is tested with. An older rs-web is refused.
pub const RS_WEB_VERSION: &str = "0.4.3";
const RELEASES: &str = "https://github.com/rslib/web/releases/download";
/// `(os, arch, asset stem, SHA-256 of the .tar.gz)`.
const ASSETS: [(&str, &str, &str, &str); 3] = [
    ("macos", "aarch64", "rs-web-darwin-aarch64", "df3f0145233cc77e36a23b8396462f5eb8d47aaa6250c975e8f2d1df56025343"),
    ("linux", "aarch64", "rs-web-linux-aarch64", "a06038f1801d726a8d36c8cf80d417101c78d5bd418348078d30304ce700a87b"),
    ("linux", "x86_64", "rs-web-linux-x86_64", "da5f75d0b5165329138f84743076ed28f735999124610fb9b2f83d9484c6db3f"),
];

const TEMPLATE: [(&str, &[u8]); 17] = [
    ("config.lua", include_bytes!("../site/config.lua")),
    ("templates/base.html", include_bytes!("../site/templates/base.html")),
    ("templates/home.html", include_bytes!("../site/templates/home.html")),
    ("templates/lesson.html", include_bytes!("../site/templates/lesson.html")),
    ("templates/list.html", include_bytes!("../site/templates/list.html")),
    ("templates/groups.html", include_bytes!("../site/templates/groups.html")),
    ("templates/protected.html", include_bytes!("../site/templates/protected.html")),
    ("templates/protected-index.html", include_bytes!("../site/templates/protected-index.html")),
    ("templates/unlock.html", include_bytes!("../site/templates/unlock.html")),
    ("templates/redirect.html", include_bytes!("../site/templates/redirect.html")),
    ("static/site.css", include_bytes!("../site/static/site.css")),
    ("static/site.js", include_bytes!("../site/static/site.js")),
    // hash-wasm 4.12.0, MIT (https://www.npmjs.com/package/hash-wasm): Argon2id in the browser.
    ("static/argon2.umd.min.js", include_bytes!("../site/static/argon2.umd.min.js")),
    ("static/hash-wasm.LICENSE", include_bytes!("../site/static/hash-wasm.LICENSE")),
    // Geist and Geist Mono from the geist 1.7.2 npm package, SIL OFL 1.1.
    ("static/fonts/Geist-Variable.woff2", include_bytes!("../site/static/fonts/Geist-Variable.woff2")),
    ("static/fonts/GeistMono-Variable.woff2", include_bytes!("../site/static/fonts/GeistMono-Variable.woff2")),
    ("static/fonts/OFL.txt", include_bytes!("../site/static/fonts/OFL.txt")),
];

const TEXT: [&str; 7] = ["html", "xml", "json", "js", "css", "txt", "md"];

fn io_error(what: impl std::fmt::Display, fix: &str) -> CliError {
    CliError::new(ErrorCode::Io, what.to_string(), fix)
}

fn site_dir() -> PathBuf {
    paths::cache_dir().join("site")
}

fn install_dir() -> PathBuf {
    paths::data_dir().join("bin")
}

/// `rs-web 0.4.3` -> `(0, 4, 3)`.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let v = text.split_whitespace().map(|w| w.trim_start_matches('v')).find(|w| w.starts_with(|c: char| c.is_ascii_digit()))?;
    let mut it = v.split('.').map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next()??, it.next().flatten().unwrap_or(0)))
}

fn version_of(bin: &Path) -> Option<(u64, u64, u64)> {
    let out = Command::new(bin).arg("--version").output().ok()?;
    out.status.success().then(|| parse_version(&String::from_utf8_lossy(&out.stdout))).flatten()
}

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|p| p.is_file())
}

/// The rs-web to run, and a note when rkb had to download it.
fn rs_web() -> Result<(PathBuf, Option<String>), CliError> {
    let fix_update = format!("install rs-web {RS_WEB_VERSION} or later (`cargo install rs-web`), or set RKB_RS_WEB to one");
    let (bin, note) = match std::env::var_os("RKB_RS_WEB").filter(|v| !v.is_empty()) {
        Some(p) => (PathBuf::from(p), None),
        None => match on_path("rs-web").or_else(|| Some(install_dir().join("rs-web")).filter(|p| p.is_file())) {
            Some(p) => (p, None),
            None => {
                let base = std::env::var("RKB_RS_WEB_URL").unwrap_or_else(|_| RELEASES.into());
                let (stem, sha) = asset().ok_or_else(|| {
                    CliError::new(
                        ErrorCode::NotFound,
                        format!(
                            "rs-web is not installed, and rs-web {RS_WEB_VERSION} has no release for {} {}",
                            std::env::consts::OS,
                            std::env::consts::ARCH
                        ),
                        &fix_update,
                    )
                })?;
                let url = format!("{}/v{RS_WEB_VERSION}/{stem}-v{RS_WEB_VERSION}.tar.gz", base.trim_end_matches('/'));
                let bin = download(&url, sha, &install_dir())?;
                let note = format!("downloaded rs-web {RS_WEB_VERSION} to {}", bin.display());
                (bin, Some(note))
            }
        },
    };
    match version_of(&bin) {
        Some(v) if v >= parse_version(RS_WEB_VERSION).expect("pinned version parses") => Ok((bin, note)),
        Some((a, b, c)) => Err(CliError::new(
            ErrorCode::Refused,
            format!("{} is rs-web {a}.{b}.{c}; rkb's site template needs {RS_WEB_VERSION} or later", bin.display()),
            &fix_update,
        )),
        None => {
            Err(CliError::new(ErrorCode::NotFound, format!("{} does not run as rs-web (`--version` failed)", bin.display()), &fix_update))
        }
    }
}

fn asset() -> Option<(&'static str, &'static str)> {
    ASSETS.iter().find(|(os, arch, ..)| *os == std::env::consts::OS && *arch == std::env::consts::ARCH).map(|(.., stem, sha)| (*stem, *sha))
}

/// Downloads the archive at `url`, checks it against `sha`, and installs the `rs-web` in it to `dir`.
fn download(url: &str, sha: &str, dir: &Path) -> Result<PathBuf, CliError> {
    let fix = "check the network and try again, or install rs-web yourself and put it on PATH";
    std::fs::create_dir_all(dir).map_err(|e| io_error(format!("{}: {e}", dir.display()), fix))?;
    let work = dir.join(format!(".rs-web-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| io_error(e, fix))?;
    let result = (|| {
        let mut resp = ureq::get(url).call().map_err(|e| io_error(format!("{url}: {e}"), fix))?;
        let mut body = vec![];
        resp.body_mut().with_config().limit(200 << 20).reader().read_to_end(&mut body).map_err(|e| io_error(format!("{url}: {e}"), fix))?;
        let got: String = Sha256::digest(&body).iter().map(|b| format!("{b:02x}")).collect();
        if got != sha {
            return Err(CliError::new(
                ErrorCode::Refused,
                format!("{url} does not match its pinned hash: expected {sha}, got {got}; nothing was installed"),
                fix,
            ));
        }
        let archive = work.join("rs-web.tar.gz");
        std::fs::write(&archive, &body).map_err(|e| io_error(e, fix))?;
        let tar =
            Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&work).status().map_err(|e| io_error(format!("tar: {e}"), fix))?;
        if !tar.success() {
            return Err(io_error(format!("tar could not unpack {url}"), fix));
        }
        let found = walk(&work)
            .into_iter()
            .find(|p| p.file_name().is_some_and(|n| n == "rs-web"))
            .ok_or_else(|| io_error(format!("{url} holds no rs-web"), fix))?;
        let target = dir.join("rs-web");
        std::fs::rename(&found, &target).map_err(|e| io_error(e, fix))?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).map_err(|e| io_error(e, fix))?;
        Ok(target)
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
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

/// The record of first publications, in the knowledge base so every machine and CI share it.
const RECORD: &str = "site/published.json";

/// Ids published in the clear, and ids published encrypted with their password group (`""` for the
/// site password). An older file's `ids` are in the clear, and its `encrypted` list is the site password.
#[derive(Default, PartialEq)]
struct Published {
    clear: BTreeSet<String>,
    encrypted: BTreeMap<String, String>,
}

fn read_published(path: &Path) -> Published {
    let v: Value = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let set = |k: &str| -> BTreeSet<String> {
        v[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
    };
    let mut clear = set("clear");
    clear.extend(set("ids"));
    let mut encrypted: BTreeMap<String, String> = set("encrypted").into_iter().map(|i| (i, String::new())).collect();
    if let Some(m) = v["encrypted"].as_object() {
        encrypted.extend(m.iter().map(|(k, g)| (k.clone(), g.as_str().unwrap_or_default().to_string())));
    }
    Published { clear, encrypted }
}

/// The knowledge base's record, with the per-machine record of an earlier rkb merged in.
fn published(env: &Env) -> Published {
    let mut p = read_published(&env.root.join(RECORD));
    let old = read_published(&env.state.join("site-published.json"));
    p.clear.extend(old.clear);
    for (i, g) in old.encrypted {
        p.encrypted.entry(i).or_insert(g);
    }
    p
}

fn group_name(s: &site::Site, id: &str) -> String {
    s.groups.get(id).cloned().flatten().unwrap_or_default()
}

fn kb_config(env: &Env) -> rkb_core::config::KbConfig {
    std::fs::read_to_string(env.root.join("kb.toml")).ok().and_then(|t| rkb_core::config::parse(&t).ok()).unwrap_or_default()
}

/// Adds `clear` ids and `encrypted` ids with their groups to `site/published.json` and commits it
/// when it changed; a lesson now public no longer counts as encrypted only.
fn record(env: &Env, clear: &[String], encrypted: &[(String, String)]) -> Result<(), CliError> {
    let _lock = rkb_core::write::kb_lock(&env.root, &kb_config(env))?;
    let path = env.root.join(RECORD);
    let on_disk = read_published(&path);
    let mut p = published(env);
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
    rkb_core::git::run(&env.root, &["add", "--", RECORD])?;
    if !rkb_core::git::run(&env.root, &["status", "--porcelain", "--", RECORD])?.is_empty() {
        rkb_core::git::commit_paths(&env.root, &format!("site: publish {added} lessons"), &[RECORD])?;
    }
    Ok(())
}

/// Writes the staging folder: the template, the allowed lessons and `site.json`.
fn stage(env: &Env, s: &site::Site, dir: &Path) -> Result<(), CliError> {
    let fix = "check the disk space and that the cache folder is writable";
    let _ = std::fs::remove_dir_all(dir);
    let put = |rel: &str, data: &[u8]| -> Result<(), CliError> {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().expect("staged files are in a folder")).map_err(|e| io_error(e, fix))?;
        std::fs::write(&p, data).map_err(|e| io_error(format!("{}: {e}", p.display()), fix))
    };
    let custom = env.root.join("site");
    if custom.join("config.lua").is_file() {
        for p in walk(&custom) {
            let rel = p.strip_prefix(&custom).expect("walk stays under the folder").to_string_lossy().into_owned();
            put(&rel, &std::fs::read(&p).map_err(|e| io_error(e, fix))?)?;
        }
    } else {
        for (rel, data) in TEMPLATE {
            put(rel, data)?;
        }
    }
    for (path, data) in &s.files {
        put(path, data)?;
    }
    put("site.json", serde_json::to_string_pretty(&s.index).expect("the index serializes").as_bytes())
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

/// Leak findings in the built text files, lesson pages that belong to no published lesson, and
/// protected titles, tags and paths in plain text. A finding names a protected lesson by id only.
fn check(env: &Env, dist: &Path, s: &site::Site) -> Vec<String> {
    let kb = kb_config(env);
    let scanner = LeakScanner::new(&kb.leak, env.lint.user.as_deref(), env.lint.home.as_deref());
    // Text that public lessons show anyway cannot give a protected lesson away.
    let public: String =
        s.files.iter().filter(|(p, _)| p.starts_with("lessons/")).map(|(_, d)| String::from_utf8_lossy(d).into_owned()).collect();
    let secrets: Vec<(&str, Vec<&str>)> = s
        .secrets
        .iter()
        .map(|x| {
            let mut words: Vec<&str> = vec![x.title.as_str(), x.path.as_str()];
            words.extend(x.tags.iter().map(String::as_str).filter(|t| t.chars().count() >= 4));
            (x.id.as_str(), words.into_iter().filter(|w| !public.contains(w)).collect())
        })
        .collect();
    let mut out = vec![];
    for p in walk(dist) {
        let rel = p.strip_prefix(dist).expect("walk stays under the folder").to_string_lossy().into_owned();
        if p.extension().and_then(|e| e.to_str()).is_some_and(|e| TEXT.contains(&e))
            && let Ok(text) = std::fs::read_to_string(&p)
        {
            // The vendored Argon2 build and font files ship as is; minified tokens can look like job ids.
            if TEMPLATE.iter().any(|(rel, body)| vendored(rel) && *body == text.as_bytes()) {
                continue;
            }
            let text = without_ciphertext(&text);
            for m in scanner.scan(&text) {
                out.push(format!("{rel}: {} `{}`", m.kind.rule(), m.text));
            }
            for (id, words) in &secrets {
                if words.iter().any(|w| text.contains(w)) {
                    out.push(format!("{rel}: shows protected lesson `{id}` in plain text"));
                }
            }
        }
        if let Some(x) = s.secrets.iter().find(|x| rel.starts_with(&format!("{}/", x.path.strip_suffix(".md").unwrap_or(&x.path)))) {
            out.push(format!("{rel}: a page at the path of protected lesson `{}`", x.id));
        }
        for (prefix, ids, what) in [("lessons/", &s.ids, "public"), ("protected/", &s.protected, "protected")] {
            if let Some(id) = rel.strip_prefix(prefix).and_then(|r| r.split('/').next()).filter(|id| !id.contains('.'))
                && !ids.iter().any(|i| i == id)
            {
                out.push(format!("{rel}: a {what} lesson page for `{id}`, which is not published that way"));
            }
        }
    }
    out.dedup();
    out
}

fn vendored(rel: &str) -> bool {
    rel.ends_with(".min.js") || rel.starts_with("static/fonts/")
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    for p in walk(from) {
        let target = to.join(p.strip_prefix(from).expect("walk stays under the folder"));
        std::fs::create_dir_all(target.parent().expect("copied files are in a folder"))?;
        std::fs::copy(&p, &target)?;
    }
    Ok(())
}

/// Replaces `out` with `dist`, through a sibling folder so a failed copy leaves `out` as it was.
fn replace(dist: &Path, out: &Path) -> std::io::Result<()> {
    let parent = out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "site".into());
    let new = parent.join(format!(".{name}.new"));
    let _ = std::fs::remove_dir_all(&new);
    std::fs::create_dir_all(&new)?;
    copy_tree(dist, &new)?;
    if out.exists() {
        std::fs::remove_dir_all(out)?;
    }
    std::fs::rename(&new, out)
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

/// `rkb site build` and `rkb site serve`. `approved` holds the ids the user just agreed to publish.
/// With `no_ask`, a first publication fails instead of asking, and the knowledge base is not written.
pub fn run(env: &Env, out: Option<String>, serve: Option<u16>, approved: &Approved, no_ask: bool) -> Result<Output, CliError> {
    kb::open(&env.root)?;
    check_passwords()?;
    let s = site::collect(&env.root, password_set)?;
    let before = published(env);
    let new_clear: Vec<String> = s.ids.iter().filter(|i| !before.clear.contains(*i) && !approved.clear.contains(i)).cloned().collect();
    let new_enc: Vec<String> = s
        .protected
        .iter()
        .filter(|i| before.encrypted.get(*i) != Some(&group_name(&s, i)) && !approved.encrypted.contains(i))
        .cloned()
        .collect();
    if !new_clear.is_empty() || !new_enc.is_empty() {
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
        if no_ask {
            return Err(CliError::new(
                ErrorCode::Refused,
                format!("{n} lessons would be published for the first time, which needs the user's yes:{list}"),
                "run `rkb site build` on your machine, answer `publish`, and push the knowledge base; nothing was written",
            ));
        }
        let question = format!("Publish {n} lessons for the first time?{list}");
        let req = Request {
            id: request::new_id(),
            created: request::now(),
            action: Action::Publish { ids: new_clear, encrypted: new_enc, out, serve },
            approved: vec![],
            question,
            choices: vec![
                Choice { text: "publish".into(), decision: Some(Decision::Publish) },
                Choice { text: "cancel".into(), decision: None },
            ],
        };
        request::save(&env.state.join("requests"), &req)?;
        return Ok(writes::needs_user_output(env, &req));
    }
    let gone: Vec<String> = before
        .clear
        .iter()
        .filter(|i| !s.ids.contains(i))
        .chain(
            before
                .encrypted
                .iter()
                .filter(|(i, g)| !s.ids.contains(i) && (!s.protected.contains(i) || **g != group_name(&s, i)))
                .map(|(i, _)| i),
        )
        .cloned()
        .collect();
    let (bin, note) = rs_web()?;
    let dir = site_dir().join("stage");
    stage(env, &s, &dir)?;
    let mut notes: Vec<String> = note.into_iter().collect();
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
    let count = s.ids.len() + s.protected.len();
    if let Some(port) = serve {
        let enc: Vec<(String, String)> = approved.encrypted.iter().map(|i| (i.clone(), group_name(&s, i))).collect();
        record(env, &approved.clear, &enc)?;
        for n in &notes {
            eprintln!("note: {n}");
        }
        eprintln!("serving {count} lessons ({} protected) from {} on port {port}; stop with Ctrl-C", s.protected.len(), dir.display());
        let status = Command::new(&bin)
            .args(["serve", "--port", &port.to_string()])
            .current_dir(&dir)
            .status()
            .map_err(|e| io_error(format!("{}: {e}", bin.display()), "check that rs-web runs"))?;
        return Ok(Output {
            data: json!({ "status": "stopped", "lessons": count }),
            human: String::new(),
            exit: u8::from(!status.success()),
            raw: false,
        });
    }
    let dist = dir.join("dist");
    let run = Command::new(&bin)
        .arg("build")
        .current_dir(&dir)
        .output()
        .map_err(|e| io_error(format!("{}: {e}", bin.display()), "check that rs-web runs"))?;
    if !run.status.success() {
        let text = format!("{}{}", String::from_utf8_lossy(&run.stderr), String::from_utf8_lossy(&run.stdout));
        let tail: Vec<&str> = text.lines().rev().take(15).collect::<Vec<_>>().into_iter().rev().collect();
        return Err(CliError::new(
            ErrorCode::Refused,
            format!("rs-web build failed:\n{}", tail.join("\n")),
            format!("fix the template (run `rs-web build` in {} to see the whole output); nothing was written", dir.display()),
        ));
    }
    let findings = check(env, &dist, &s);
    if !findings.is_empty() {
        return Err(CliError::new(
            ErrorCode::Refused,
            format!("the built site failed its checks:\n{}", findings.join("\n")),
            "fix the lesson or the template; nothing was written",
        ));
    }
    let out = out.map(PathBuf::from).unwrap_or_else(|| site_dir().join("dist"));
    replace(&dist, &out).map_err(|e| io_error(format!("{}: {e}", out.display()), "check that the output folder is writable"))?;
    if !no_ask {
        let enc: Vec<(String, String)> = s.protected.iter().map(|i| (i.clone(), group_name(&s, i))).collect();
        record(env, &s.ids, &enc)?;
    }
    let mut human = format!("{} {count} lessons ({} protected) to {}", paint(env.colored, "32", "built"), s.protected.len(), out.display());
    for n in &notes {
        human.push_str(&format!("\n{} {n}", paint(env.colored, "33", "note:")));
    }
    let data = json!({
        "status": "built",
        "out": out.display().to_string(),
        "lessons": count,
        "protected": s.protected.len(),
        "rs_web": bin.display().to_string(),
        "notes": notes,
        "help": [format!("Open {}/index.html, or run `rkb site serve` for a live preview", out.display())],
    });
    Ok(Output { data, human, exit: 0, raw: false })
}

/// `rkb site init`: the built-in template into `$RKB_HOME/site/`, keeping files that exist.
pub fn init(env: &Env) -> Result<Output, CliError> {
    kb::open(&env.root)?;
    let _lock = rkb_core::write::kb_lock(&env.root, &kb_config(env))?;
    let fix = "check that the knowledge base is writable";
    let (mut written, mut skipped) = (vec![], vec![]);
    for (rel, text) in TEMPLATE {
        let path = format!("site/{rel}");
        let p = env.root.join(&path);
        if p.exists() {
            skipped.push(path);
            continue;
        }
        std::fs::create_dir_all(p.parent().expect("template files are in a folder")).map_err(|e| io_error(e, fix))?;
        let mut f = std::fs::File::create(&p).map_err(|e| io_error(format!("{}: {e}", p.display()), fix))?;
        f.write_all(text).map_err(|e| io_error(e, fix))?;
        written.push(path);
    }
    if !written.is_empty() {
        let refs: Vec<&str> = written.iter().map(String::as_str).collect();
        let mut add = vec!["add", "--"];
        add.extend(&refs);
        rkb_core::git::run(&env.root, &add)?;
        let mut status = vec!["status", "--porcelain", "--"];
        status.extend(&refs);
        // A file removed and written again is unchanged, and git would refuse an empty commit.
        if !rkb_core::git::run(&env.root, &status)?.is_empty() {
            rkb_core::git::commit_paths(&env.root, "site: add the rs-web template", &refs)?;
        }
    }
    let mut human = String::new();
    for p in &written {
        human.push_str(&format!("{} {p}\n", paint(env.colored, "32", "wrote  ")));
    }
    for p in &skipped {
        human.push_str(&format!("{} {p} (exists)\n", paint(env.colored, "2", "skipped")));
    }
    human.push_str("`rkb site build` now uses site/ as the template");
    let data = json!({
        "written": written,
        "skipped": skipped,
        "help": ["Edit site/config.lua and site/templates/, then run `rkb site build`"],
    });
    Ok(Output { data, human, exit: 0, raw: false })
}

/// `rkb site ci`: `.github/workflows/site.yml` into the knowledge base, kept when it exists.
pub fn ci(env: &Env) -> Result<Output, CliError> {
    kb::open(&env.root)?;
    let kbc = kb_config(env);
    let _lock = rkb_core::write::kb_lock(&env.root, &kbc)?;
    const PATH: &str = ".github/workflows/site.yml";
    let branch = String::from_utf8_lossy(&rkb_core::git::run(&env.root, &["rev-parse", "--abbrev-ref", "HEAD"])?).trim().to_string();
    let mut secrets = vec!["SITE_PASSWORD".to_string()];
    secrets.extend(kbc.labels.get("password").into_iter().flatten().map(|g| site::password_env(Some(g))));
    secrets.extend(["CLOUDFLARE_API_TOKEN".into(), "CLOUDFLARE_ACCOUNT_ID".into()]);
    let variables = ["CLOUDFLARE_PROJECT_NAME"];
    let p = env.root.join(PATH);
    let written = !p.exists();
    if written {
        let env_lines: Vec<String> =
            secrets.iter().filter(|s| s.starts_with("SITE_PASSWORD")).map(|s| format!("          {s}: ${{{{ secrets.{s} }}}}")).collect();
        let text = include_str!("../site/ci.yml")
            .replace("@BRANCH@", &branch)
            .replace("@VERSION@", env!("CARGO_PKG_VERSION"))
            .replace("@PASSWORDS@", &env_lines.join("\n"));
        let fix = "check that the knowledge base is writable";
        std::fs::create_dir_all(p.parent().expect("the workflow is in a folder")).map_err(|e| io_error(e, fix))?;
        let mut f = std::fs::File::create_new(&p).map_err(|e| io_error(format!("{}: {e}", p.display()), fix))?;
        f.write_all(text.as_bytes()).map_err(|e| io_error(e, fix))?;
        rkb_core::git::run(&env.root, &["add", "--", PATH])?;
        if !rkb_core::git::run(&env.root, &["status", "--porcelain", "--", PATH])?.is_empty() {
            rkb_core::git::commit_paths(&env.root, "site: add the CI workflow", &[PATH])?;
        }
    }
    let mut notes = vec![];
    if kbc.site.base_url.as_deref().unwrap_or_default().is_empty() {
        notes.push(
            "[site] base_url in kb.toml is empty; set it to the site's address for absolute links, the sitemap and link previews"
                .to_string(),
        );
    }
    let mut human = if written {
        format!("{} {PATH} (rkb {}, branch {branch})", paint(env.colored, "32", "wrote  "), env!("CARGO_PKG_VERSION"))
    } else {
        format!("{} {PATH} (exists)", paint(env.colored, "2", "kept   "))
    };
    human.push_str(&format!("\nset these repository secrets: {}", secrets.join(", ")));
    human.push_str(&format!("\nset this repository variable: {}", variables.join(", ")));
    for n in &notes {
        human.push_str(&format!("\n{} {n}", paint(env.colored, "33", "note:")));
    }
    let data = json!({
        "status": if written { "written" } else { "kept" },
        "path": PATH,
        "branch": branch,
        "rkb": env!("CARGO_PKG_VERSION"),
        "secrets": secrets,
        "variables": variables,
        "notes": notes,
        "help": ["Set the secrets and the variable in the repository settings, then push the knowledge base"],
    });
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
    fn versions() {
        assert_eq!(parse_version("rs-web 0.4.3\n"), Some((0, 4, 3)));
        assert_eq!(parse_version("rs-web v1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("nothing"), None);
        assert!(parse_version("rs-web 0.10.0") > parse_version(RS_WEB_VERSION));
    }

    /// Serves `body` once over HTTP on a local port and returns the URL.
    fn serve_once(body: Vec<u8>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let head = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
            s.write_all(head.as_bytes()).unwrap();
            s.write_all(&body).unwrap();
        });
        format!("http://127.0.0.1:{port}/rs-web.tar.gz")
    }

    fn archive() -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("rs-web-x");
        std::fs::create_dir(&inner).unwrap();
        std::fs::write(inner.join("rs-web"), "#!/bin/sh\necho 'rs-web 0.4.3'\n").unwrap();
        let tgz = dir.path().join("a.tar.gz");
        assert!(Command::new("tar").arg("-czf").arg(&tgz).arg("-C").arg(dir.path()).arg("rs-web-x").status().unwrap().success());
        std::fs::read(tgz).unwrap()
    }

    #[test]
    fn download_checks_the_hash_before_installing() {
        let body = archive();
        let sha: String = Sha256::digest(&body).iter().map(|b| format!("{b:02x}")).collect();
        let dir = tempfile::tempdir().unwrap();

        let e = download(&serve_once(body.clone()), &"0".repeat(64), dir.path()).unwrap_err();
        assert!(e.message.contains("does not match its pinned hash") && e.message.contains(&sha), "{}", e.message);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "nothing installed");

        let bin = download(&serve_once(body), &sha, dir.path()).unwrap();
        assert_eq!(bin, dir.path().join("rs-web"));
        assert_eq!(version_of(&bin), Some((0, 4, 3)));
    }
}
