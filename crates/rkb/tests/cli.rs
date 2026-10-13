use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("gitconfig"), "[user]\n\tname = Test\n\temail = test@example.org\n").unwrap();
        let fake = dir.path().join("fake/claude");
        std::fs::create_dir_all(fake.parent().unwrap()).unwrap();
        std::fs::write(&fake, include_str!("../../../tests/fixtures/fake-claude.sh")).unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        Env { dir }
    }

    fn kb(&self) -> PathBuf {
        self.dir.path().join("kb")
    }

    fn cmd(&self, program: &str) -> Command {
        let bin = PathBuf::from(env!("CARGO_BIN_EXE_rkb"));
        let path = format!("{}:{}", bin.parent().unwrap().display(), std::env::var("PATH").unwrap());
        let mut c = Command::new(program);
        // Never the developer's checkout: project detection would run git there.
        c.current_dir(self.dir.path());
        c.env("RKB_HOME", self.kb())
            .env("HOME", self.dir.path())
            .env("USER", "testuser")
            .env("GIT_CONFIG_GLOBAL", self.dir.path().join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("PATH", path)
            .env("XDG_STATE_HOME", self.dir.path().join("state"))
            .env("XDG_CONFIG_HOME", self.dir.path().join("config"))
            .env("XDG_DATA_HOME", self.dir.path().join("data"))
            .env("XDG_CACHE_HOME", self.dir.path().join("cache"))
            .env("RKB_CLAUDE", self.dir.path().join("fake/claude"))
            .env("FAKE_CLAUDE_HOME", self.dir.path())
            .env("RKB_TTY", self.dir.path().join("no-tty"))
            .env_remove("CLAUDECODE")
            .env_remove("HERMES_HOME")
            .env_remove("RKB_TEST_HOME")
            .env_remove("RKB_FORMAT")
            .env_remove("RKB_AGENT")
            .env_remove("EMAIL")
            .env_remove("GIT_AUTHOR_NAME")
            .env_remove("GIT_AUTHOR_EMAIL")
            .env_remove("GIT_COMMITTER_NAME")
            .env_remove("GIT_COMMITTER_EMAIL")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("PI_SESSION_ID")
            .env_remove("RKB_SESSION");
        c
    }

    fn rkb(&self, args: &[&str]) -> Output {
        self.cmd(env!("CARGO_BIN_EXE_rkb")).args(args).stdin(std::process::Stdio::null()).output().unwrap()
    }

    fn rkb_in(&self, args: &[&str], input: &str) -> Output {
        rkb_with(self.cmd(env!("CARGO_BIN_EXE_rkb")), args, input)
    }

    fn json(&self, args: &[&str], input: &str) -> (serde_json::Value, Option<i32>) {
        let mut all = args.to_vec();
        all.extend(["--format", "json"]);
        let o = self.rkb_in(&all, input);
        let v = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("not json: {}", stdout(&o)));
        (v, o.status.code())
    }

    fn trust_claude(&self) {
        self.write_abs(&self.dir.path().join("config/rkb/trust.toml"), "[harness.claude-code]\ngated = true\n");
    }

    fn write_abs(&self, p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn git(&self, args: &[&str]) -> Output {
        self.cmd("git").arg("-C").arg(self.kb()).args(args).output().unwrap()
    }

    fn init(&self) {
        let o = self.rkb(&["init"]);
        assert!(o.status.success(), "{}", stdout(&o));
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.kb().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn copy_fixture(&self) {
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/kb");
        copy_dir(&src, &self.kb());
    }
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let to = dst.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &to);
        } else {
            std::fs::copy(e.path(), to).unwrap();
        }
    }
}

fn rkb_with(mut c: Command, args: &[&str], input: &str) -> Output {
    use std::io::Write;
    let mut child = c
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // A command that exits without reading stdin closes the pipe; its output still decides the test.
    let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
    child.wait_with_output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr)
}

const FACT: &str = "---
schema: 1
id: 0a1b2c3d4e
type: fact
status: active
verified: 2026-09-25
verified_how: ran
tags:
  - demo
---

# A fact

## Statement
Something is true.

## Evidence
It was seen.
";

#[test]
fn init_creates_a_valid_knowledge_base() {
    let env = Env::new();
    env.init();
    let kb = env.kb();
    assert!(kb.join("kb.toml").is_file());
    assert!(!kb.join(".gitignore").exists());
    for d in ["general", "systems", "projects"] {
        assert!(kb.join(d).is_dir());
    }
    let hook = std::fs::read_to_string(kb.join(".git/hooks/pre-commit")).unwrap();
    assert!(hook.contains("rkb lint --staged"));
    assert_eq!(stdout(&env.git(&["rev-list", "--count", "HEAD"])).trim(), "1");
    assert_eq!(stdout(&env.git(&["config", "receive.denyCurrentBranch"])).trim(), "updateInstead");
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "");
    let lint = env.rkb(&["lint"]);
    assert!(lint.status.success(), "{}", stdout(&lint));
}

#[test]
fn init_refuses_a_non_empty_directory() {
    let env = Env::new();
    env.write("notes.txt", "x");
    let o = env.rkb(&["init", "--toon"]);
    assert!(!o.status.success());
    assert!(stdout(&o).contains("code: exists"), "{}", stdout(&o));
    assert!(!env.kb().join(".git").exists());
}

#[test]
fn init_needs_a_git_identity() {
    let env = Env::new();
    std::fs::write(env.dir.path().join("gitconfig"), "").unwrap();
    let o = env.rkb(&["init", "--toon"]);
    assert!(!o.status.success());
    let out = stdout(&o);
    assert!(out.contains("code: git_identity") && out.contains("git config --global user.name"), "{out}");
    assert!(!env.kb().exists());
}

#[test]
fn missing_knowledge_base_points_to_init() {
    let env = Env::new();
    let o = env.rkb(&["lint", "--toon"]);
    assert!(!o.status.success());
    let out = stdout(&o);
    assert!(out.contains("code: not_a_kb") && out.contains("rkb init"), "{out}");
}

#[test]
fn hook_blocks_a_bad_hand_commit_and_allows_a_good_one() {
    let env = Env::new();
    env.init();
    env.write("general/demo/bad.md", &FACT.replace("verified_how: ran", "verified_how: ran\nverifed: x"));
    env.git(&["add", "-A"]);
    let o = env.git(&["commit", "-q", "-m", "add: bad"]);
    assert!(!o.status.success());
    assert!(stdout(&o).contains("verifed"), "{}", stdout(&o));

    env.git(&["rm", "-q", "--cached", "general/demo/bad.md"]);
    std::fs::remove_file(env.kb().join("general/demo/bad.md")).unwrap();
    env.write("general/demo/good.md", FACT);
    env.git(&["add", "-A"]);
    let o = env.git(&["commit", "-q", "-m", "add: good"]);
    assert!(o.status.success(), "{}", stdout(&o));
}

#[test]
fn staged_lint_reads_the_index_only() {
    let env = Env::new();
    env.init();
    env.write("general/demo/a.md", FACT);
    env.git(&["add", "-A"]);
    env.write("general/demo/a.md", "not a lesson");
    let o = env.rkb(&["lint", "--staged"]);
    assert!(o.status.success(), "{}", stdout(&o));
    assert!(!env.rkb(&["lint"]).status.success());
}

#[test]
fn staged_lint_rejects_hand_written_checked() {
    let env = Env::new();
    env.init();
    env.write("general/demo/a.md", FACT);
    env.git(&["add", "-A"]);
    assert!(env.git(&["commit", "-q", "-m", "add: a"]).status.success());
    env.write("general/demo/a.md", &FACT.replace("verified_how: ran", "verified_how: checked"));
    env.git(&["add", "-A"]);
    let o = env.rkb(&["lint", "--staged"]);
    assert!(!o.status.success());
    assert!(stdout(&o).contains("format/checked-by-hand"), "{}", stdout(&o));
}

#[test]
fn output_formats() {
    let env = Env::new();
    env.init();
    let human = stdout(&env.rkb(&["lint"]));
    assert!(human.contains("0 errors, 0 warnings") && !human.contains('\x1b'), "piped output is human text without color: {human}");
    let toon = stdout(&env.rkb(&["lint", "--toon"]));
    assert!(toon.contains("errors: 0") && toon.contains("findings"), "{toon}");
    let o = env.rkb(&["lint", "--toon", "--format", "json"]);
    assert_eq!(o.status.code(), Some(2), "--toon and another --format conflict");
    let json: serde_json::Value = serde_json::from_slice(&env.rkb(&["lint", "--format", "json"]).stdout).unwrap();
    assert_eq!(json["findings"], serde_json::json!([]));
    let human = env.cmd(env!("CARGO_BIN_EXE_rkb")).env("RKB_FORMAT", "json").args(["lint", "--format", "human"]).output().unwrap();
    assert_eq!(stdout(&human).trim(), "0 errors, 0 warnings");
    let bad = env.cmd(env!("CARGO_BIN_EXE_rkb")).env("RKB_FORMAT", "yaml").args(["lint"]).output().unwrap();
    assert!(stdout(&bad).contains("RKB_FORMAT=yaml"));
}

#[test]
fn error_records_in_each_format() {
    let env = Env::new();
    env.init();
    let toon = env.rkb(&["show", "0000000000", "--toon"]);
    assert!(!toon.status.success());
    assert!(String::from_utf8_lossy(&toon.stdout).contains("code: not_found"));
    let json: serde_json::Value = serde_json::from_slice(&env.rkb(&["show", "0000000000", "--format", "json"]).stdout).unwrap();
    assert_eq!(json["error"]["code"], "not_found");
    assert!(!json["error"]["fix"].as_str().unwrap().is_empty());
    let human = env.rkb(&["show", "0000000000", "--format", "human"]);
    assert!(human.stdout.is_empty());
    let err = String::from_utf8_lossy(&human.stderr);
    assert!(err.contains("error:") && err.contains("fix:") && !err.contains('\x1b'), "{err}");
}

#[test]
fn lint_reports_findings_and_cuts_long_messages() {
    let env = Env::new();
    env.init();
    let long = format!("/Users/someone/{}", "x".repeat(300));
    env.write("general/demo/a.md", &FACT.replace("It was seen.", &format!("Built in {long}.")));
    let o = env.rkb(&["lint", "--format", "json"]);
    assert!(!o.status.success());
    let json: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(json["errors"], 1);
    let error = |v: &serde_json::Value| {
        v["findings"].as_array().unwrap().iter().find(|f| f["severity"] == "error").unwrap()["message"].as_str().unwrap().to_string()
    };
    let msg = error(&json);
    assert!(msg.contains("rkb lint --full") && msg.len() < 300, "{msg}");
    let full: serde_json::Value = serde_json::from_slice(&env.rkb(&["lint", "--full", "--format", "json"]).stdout).unwrap();
    assert!(error(&full).contains(&long));
}

#[test]
fn show_prints_the_whole_lesson() {
    let env = Env::new();
    env.init();
    let evidence = (1..=120).map(|i| format!("Line {i} of evidence.\n")).collect::<String>();
    let text = FACT.replace("It was seen.\n", &evidence);
    assert!(text.lines().count() >= 130);
    env.write("general/demo/a.md", &text);
    let json: serde_json::Value = serde_json::from_slice(&env.rkb(&["show", "0a1b2c3d4e", "--format", "json"]).stdout).unwrap();
    assert_eq!(json["path"], "general/demo/a.md");
    assert_eq!(json["title"], "A fact");
    assert_eq!(json["frontmatter"]["type"], "fact");
    assert!(json["body"].as_str().unwrap().contains("Line 120 of evidence."));
    let human = stdout(&env.rkb(&["show", "0a1b2c3d4e", "--format", "human"]));
    assert!(human.contains("verified_how: ran") && human.contains("Line 120 of evidence."));
    let toon = stdout(&env.rkb(&["show", "0a1b2c3d4e"]));
    assert!(toon.contains("Line 120 of evidence."));
}

#[test]
fn every_command_help_has_an_example() {
    let env = Env::new();
    for cmd in [
        "", "init", "lint", "show", "list", "context", "doctor", "search", "find", "eval", "install", "add", "edit", "flag", "used", "log",
        "confirm",
    ] {
        let cmd: Vec<&str> = if cmd.is_empty() { vec!["--help"] } else { vec![cmd, "--help"] };
        let o = env.rkb(&cmd);
        assert!(o.status.success());
        assert!(stdout(&o).contains("Example:\n  rkb") || stdout(&o).contains("Example:\n  RKB_HOME"), "{cmd:?}");
    }
}

#[test]
fn full_flow_on_the_fixture() {
    let env = Env::new();
    env.init();
    env.copy_fixture();
    let lint = env.rkb(&["lint"]);
    assert!(lint.status.success(), "{}", stdout(&lint));
    assert!(stdout(&env.rkb(&["show", "7f3a9c2b41", "--format", "human"])).contains("HDF5_ROOT"));
    env.git(&["add", "-A"]);
    let o = env.git(&["commit", "-q", "-m", "add: fixture"]);
    assert!(o.status.success(), "{}", stdout(&o));
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "");
}

#[test]
fn lint_fix_rewrites_flow_style() {
    let env = Env::new();
    env.init();
    let lesson = FACT.replace("tags:\n  - demo", "tags: [demo, io]") + "  trailing  \n";
    env.write("general/demo/a.md", &lesson);
    env.write("general/demo/README.md", "---\nlabels: {sensitivity: public}\n---\n\n# Demo\n");
    let before: serde_json::Value = serde_json::from_slice(&env.rkb(&["lint", "--format", "json"]).stdout).unwrap();
    let style = |v: &serde_json::Value| {
        v["findings"].as_array().unwrap().iter().filter(|f| !f["rule"].as_str().unwrap().starts_with("quality/")).count()
    };
    assert_eq!(style(&before), 2, "{before}");

    let o = env.rkb(&["lint", "--fix", "--format", "json"]);
    assert!(o.status.success(), "{}", stdout(&o));
    let json: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(json["fixed"], serde_json::json!(["general/demo/README.md", "general/demo/a.md"]));
    assert_eq!(style(&json), 0, "{json}");
    let a = std::fs::read_to_string(env.kb().join("general/demo/a.md")).unwrap();
    assert!(a.contains("tags:\n  - demo\n  - io\n"), "{a}");
    assert!(a.ends_with("It was seen.\n  trailing  \n"));
    let note = std::fs::read_to_string(env.kb().join("general/demo/README.md")).unwrap();
    assert_eq!(note, "---\nlabels:\n  sensitivity: public\n---\n\n# Demo\n");
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "?? general/demo/\n");

    let again: serde_json::Value = serde_json::from_slice(&env.rkb(&["lint", "--fix", "--format", "json"]).stdout).unwrap();
    assert_eq!(again["fixed"], serde_json::json!([]));
    assert_eq!(env.rkb(&["lint", "--fix", "--staged"]).status.code(), Some(2));
}

const PITFALL: &str = "---
type: pitfall
verified_how: ran
tags:
  - cmake
---

# CMake cannot find HDF5 unless HDF5_ROOT is set

## Symptom
Configure fails.

## Cause
The module has no CMake config file.

## Fix
Set HDF5_ROOT.

## Evidence
Configure passed with the flag.
";

fn lesson_with(title: &str) -> String {
    PITFALL.replace("CMake cannot find HDF5 unless HDF5_ROOT is set", title)
}

fn preference_with(title: &str, rule: &str) -> String {
    format!(
        "---\ntype: preference\nverified_how: told\ntags:\n  - demo\n---\n\n# {title}\n\n## Rule\n{rule}\n\n## Why\nThe user asked for it.\n\n## How to apply\nDo what the rule says.\n"
    )
}

#[test]
fn digest_ranks_and_caps() {
    let env = kb_with_topics();
    let a = add_ok(
        &env,
        "cmake",
        &preference_with("Always pass the module prefix explicitly", "Pass the module prefix explicitly on every configure."),
    );
    let b = add_ok(&env, "cmake", &preference_with("Prefer ninja over make for local builds", "Use ninja for every local build."));
    let c = add_ok(&env, "cpp", &preference_with("Avoid std regex in hot loops", "Never use std regex in a hot loop."));
    let (a, b, c) = (a["id"].as_str().unwrap().to_string(), b["id"].as_str().unwrap().to_string(), c["id"].as_str().unwrap().to_string());
    for _ in 0..5 {
        assert_eq!(env.json(&["used", &a, "--worked"], "").0["status"], "recorded");
    }
    for _ in 0..2 {
        assert_eq!(env.json(&["used", &c, "--worked"], "").0["status"], "recorded");
    }
    // Ranked by worked reports: 5, 2, 0.
    let (v, code) = env.json(&["digest"], "");
    assert_eq!(code, Some(0), "{v}");
    let ids: Vec<&str> = v["lines"].as_array().unwrap().iter().map(|l| l["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [&a, &c, &b], "{v}");
    let human = stdout(&env.rkb(&["digest"]));
    let (ia, ib, ic) = (human.find("module prefix").unwrap(), human.find("local build").unwrap(), human.find("std regex").unwrap());
    assert!(ia < ic && ic < ib, "ranked 5, 2, 0: {human}");
    // A pitfall, a stale preference and an archived preference stay out of the digest.
    let pitfall = add_ok(&env, "cmake", PITFALL)["id"].as_str().unwrap().to_string();
    assert_eq!(env.json(&["flag", &c, "--reason", "rule no longer holds"], "").0["status"], "written");
    assert!(current(&env, &c).0.contains("status: stale"));
    assert_eq!(env.json(&["archive", &b, "--reason", "no longer wanted"], "").0["status"], "written");
    let (v, code) = env.json(&["digest"], "");
    assert_eq!(code, Some(0), "{v}");
    let ids: Vec<&str> = v["lines"].as_array().unwrap().iter().map(|l| l["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [&a], "only the standing preference: {v}");
    let human = stdout(&env.rkb(&["digest"]));
    assert!(human.contains("module prefix"), "{human}");
    assert!(!human.contains(&pitfall) && !human.contains("local build") && !human.contains("std regex"), "{human}");

    for i in 0..100 {
        add_ok(
            &env,
            if i % 2 == 0 { "cmake" } else { "cpp" },
            &preference_with(
                &format!("Standing preference number {i} for daily work"),
                &format!("Follow standing preference number {i} in daily work."),
            ),
        );
    }
    let human = stdout(&env.rkb(&["digest"]));
    assert!(human.chars().count() <= 1200, "{} chars: {human}", human.chars().count());
    let (v, _) = env.json(&["digest"], "");
    assert!(v["lines"].as_array().unwrap().len() < 103, "the budget keeps lines out: {v}");
    assert!(human.lines().all(|l| l.ends_with("in daily work.") || l.ends_with("every configure.")), "whole lines only: {human}");
}

/// A knowledge base with committed topic folders `general/cmake` and `general/cpp` (aliases c++ and cxx).
fn kb_with_topics() -> Env {
    let env = Env::new();
    env.init();
    env.write("general/cmake/README.md", "# CMake\n");
    env.write("general/cpp/README.md", "---\naliases:\n  - c++\n  - cxx\n---\n\n# C and C++\n");
    env.git(&["add", "-A"]);
    assert!(env.git(&["commit", "-q", "-m", "add: topics"]).status.success());
    env
}

fn add_ok(env: &Env, topic: &str, text: &str) -> serde_json::Value {
    let (v, code) = env.json(&["add", "--topic", topic], text);
    assert_eq!(code, Some(0), "{v}");
    v
}

#[test]
fn add_stores_images_without_metadata() {
    let env = kb_with_topics();
    let img = env.dir.path().join("shots/layout.png");
    env.write_abs(&img, "");
    std::fs::write(&img, png_with_gps()).unwrap();
    let text = format!("{}\n![layout](layout.png)\n", lesson_with("Image lesson").trim_end());
    let before = commit_count(&env);
    let (v, code) = env.json(&["add", "--topic", "cmake", "--asset", img.to_str().unwrap()], &text);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(commit_count(&env), before + 1, "one commit");
    let path = v["path"].as_str().unwrap();
    let stem = path.trim_end_matches(".md");
    let stored = env.kb().join(format!("{stem}.assets/layout.png"));
    let png = std::fs::read(&stored).unwrap();
    assert!(png.starts_with(b"\x89PNG") && !String::from_utf8_lossy(&png).contains("GPS"), "the stored image holds no EXIF");
    let name = stem.rsplit('/').next().unwrap();
    assert!(front(&env, path).contains(&format!("![layout]({name}.assets/layout.png)")), "{}", front(&env, path));
    let files = stdout(&env.git(&["show", "--name-only", "--format=", "HEAD"]));
    assert!(files.contains(".assets/layout.png") && files.contains(".md"), "{files}");
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "");

    let id = v["id"].as_str().unwrap().to_string();
    let trace = env.dir.path().join("shots/trace.png");
    std::fs::write(&trace, png_with_gps()).unwrap();
    let (text, hash) = current(&env, &id);
    let (v, code) = env
        .json(&["edit", &id, "--base", &hash, "--asset", trace.to_str().unwrap()], &format!("{}\n![trace](trace.png)\n", text.trim_end()));
    assert_eq!((v["status"].as_str(), code), (Some("written"), Some(0)), "{v}");
    assert!(front(&env, path).contains(&format!("![trace]({name}.assets/trace.png)")));
    let t = std::fs::read(env.kb().join(format!("{stem}.assets/trace.png"))).unwrap();
    assert!(!String::from_utf8_lossy(&t).contains("GPS"), "the edit strips the metadata");
    let files = stdout(&env.git(&["show", "--name-only", "--format=", "HEAD"]));
    assert!(files.contains(".assets/trace.png") && files.contains(".md"), "one commit: {files}");

    let (text, hash) = current(&env, &id);
    let (v, _) = env.json(&["edit", &id, "--base", &hash, "--asset", trace.to_str().unwrap()], &text);
    assert_eq!(v["status"], "unchanged", "the same text and the same image write nothing: {v}");
    let replacement = env.dir.path().join("new/layout.png");
    env.write_abs(&replacement, "");
    let mut png = png_with_gps();
    png.splice(16..20, [0, 0, 0, 2]);
    std::fs::write(&replacement, png).unwrap();
    let old = std::fs::read(&stored).unwrap();
    let (v, code) = env.json(&["edit", &id, "--base", &hash, "--asset", replacement.to_str().unwrap()], &text);
    assert_eq!((v["status"].as_str(), code), (Some("written"), Some(0)), "same text, a replaced image: {v}");
    assert_ne!(std::fs::read(&stored).unwrap(), old, "the image was replaced");
    let before = commit_count(&env);

    let txt = env.dir.path().join("notes.txt");
    env.write_abs(&txt, "hi");
    let (v, code) = env.json(&["add", "--topic", "cmake", "--asset", txt.to_str().unwrap()], &lesson_with("Another lesson"));
    assert_eq!(code, Some(1), "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("notes.txt is not an image"), "{v}");
    assert_eq!(commit_count(&env), before, "nothing written");
}

fn head_subject(env: &Env) -> String {
    stdout(&env.git(&["log", "-1", "--format=%s"])).trim().to_string()
}

#[test]
fn add_fills_fields_commits_one_file() {
    let env = kb_with_topics();
    env.write("general/cmake/README.md", "# CMake\n\nChanged but not committed.\n");
    let v = add_ok(&env, "cmake", PITFALL);
    let id = v["id"].as_str().unwrap();
    let path = v["path"].as_str().unwrap();
    assert_eq!(path, "general/cmake/cmake-cannot-find-hdf5-unless-hdf5-root-is-set.md");
    let text = std::fs::read_to_string(env.kb().join(path)).unwrap();
    assert!(text.starts_with(&format!("---\nschema: 1\nid: {id}\ntype: pitfall\nstatus: active\nverified: ")), "{text}");
    assert_eq!(head_subject(&env), format!("add(general/cmake): CMake cannot find HDF5 unless HDF5_ROOT is set [{id}]"));
    let files = stdout(&env.git(&["show", "--name-only", "--format=", "HEAD"]));
    assert_eq!(files.trim(), path);
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])).trim(), "M general/cmake/README.md");
    assert!(v["diff"].as_array().unwrap().iter().any(|l| l == "+# CMake cannot find HDF5 unless HDF5_ROOT is set"));
}

#[test]
fn add_refuses_bad_input() {
    let env = kb_with_topics();
    let (v, code) = env.json(&["add", "--topic", "cmake"], &PITFALL.replace("type: pitfall", "id: 0000000000\ntype: pitfall"));
    assert_eq!((v["error"]["code"].as_str(), code), (Some("refused"), Some(1)));
    assert!(v["error"]["message"].as_str().unwrap().contains("`id`"));
    let (v, _) = env.json(&["add", "--topic", "cmake"], &PITFALL.replace("verified_how: ran", "verified_how: checked"));
    assert_eq!(v["error"]["code"], "refused");
    let (v, _) = env.json(&["add", "--topic", "cmake"], &PITFALL.replace("## Cause\nThe module has no CMake config file.\n\n", ""));
    assert_eq!(v["error"]["code"], "invalid_lesson");
    assert_eq!(v["error"]["findings"][0]["rule"], "body/headings");
    assert_eq!(stdout(&env.git(&["rev-list", "--count", "HEAD"])).trim(), "2");
}

/// A lesson labeled `public`: adding it loosens the default `internal`, one of the questions that remain.
fn public_pitfall() -> String {
    PITFALL.replacen("\n---\n", "\nlabels:\n  sensitivity: public\n---\n", 1)
}

const LOOSEN: &str = "loosen sensitivity to public";

#[test]
fn close_topic_is_a_note_and_confirm_paths() {
    let env = kb_with_topics();
    let (v, code) = env.json(&["add", "--topic", "cmak"], PITFALL);
    assert_eq!((v["status"].as_str(), code), (Some("written"), Some(0)), "no question for a close topic name: {v}");
    assert!(v["path"].as_str().unwrap().starts_with("general/cmak/"), "{v}");
    assert!(v["notes"].to_string().contains("close to general/cmake") && v["notes"].to_string().contains("rkb move"), "{v}");

    let o = env.rkb_in(&["add", "--topic", "cmake", "--toon"], &public_pitfall());
    assert_eq!(o.status.code(), Some(3));
    let out = stdout(&o);
    assert!(out.contains("status: needs_user") && out.contains(LOOSEN), "{out}");
    let (v, _) = env.json(&["add", "--topic", "cmake"], &public_pitfall());
    let req = v["request"].as_str().unwrap().to_string();
    assert_eq!(v["options"][0], LOOSEN);
    assert!(v["next"].as_str().unwrap().starts_with(&format!("rkb confirm {req} --choice")));

    let (bad, _) = env.json(&["confirm", &req, "--choice", "delete everything"], "");
    assert_eq!(bad["error"]["code"], "bad_choice");
    assert!(bad["error"]["options"].as_array().unwrap().iter().any(|o| o == LOOSEN));

    let (nt, _) = env.json(&["confirm", &req, "--choice", LOOSEN], "");
    assert_eq!(nt["error"]["code"], "needs_terminal");
    assert!(nt["error"]["fix"].as_str().unwrap().contains(&format!("rkb confirm {req} --choice \"{LOOSEN}\"")));

    let tty = env.dir.path().join("tty");
    std::fs::write(&tty, "yes\n").unwrap();
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_TTY", &tty);
    let o = rkb_with(c, &["confirm", &req, "--choice", LOOSEN, "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["status"], "written", "{v}");
    assert!(v["path"].as_str().unwrap().starts_with("general/cmake/"));
    assert!(std::fs::read_to_string(&tty).unwrap().contains("Type yes to confirm"));

    let (again, _) = env.json(&["confirm", &req, "--choice", LOOSEN], "");
    assert_eq!(again["error"]["code"], "expired");
}

#[test]
fn writes_record_provenance() {
    let env = kb_with_topics();
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("CLAUDECODE", "1").env("CLAUDE_CODE_SESSION_ID", "s1");
    let o = rkb_with(c, &["add", "--topic", "cmake", "--from-inbox", "0a1b2c3d4e", "--format", "json"], PITFALL);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["status"], "written", "{v}");
    let (id, path) = (v["id"].as_str().unwrap().to_string(), v["path"].as_str().unwrap().to_string());
    let text = front(&env, &path);
    let today = jiff::Zoned::now().date().to_string();
    for part in ["meta:", "source:", "harness: claude-code", &format!("date: {today}"), "inbox: 0a1b2c3d4e"] {
        assert!(text.contains(part), "{part} missing:\n{text}");
    }
    assert!(!text.contains("session"), "the session stays out of the lesson:\n{text}");
    let log = std::fs::read_to_string(env.dir.path().join("state/rkb/written.jsonl")).unwrap();
    let rec: Vec<serde_json::Value> = log.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!((rec.len(), rec[0]["id"].as_str(), rec[0]["session"].as_str()), (1, Some(id.as_str()), Some("s1")), "{log}");
    assert!(rec[0]["time"].as_u64().is_some());

    let (text, hash) = current(&env, &id);
    let (v, _) = env.json(&["edit", &id, "--base", &hash], &text);
    assert_eq!(v["status"], "unchanged", "no change, no edited_by: {v}");
    let (v, _) = env.json(&["edit", &id, "--base", &hash], &text.replace("Set HDF5_ROOT.", "Set HDF5_ROOT to the prefix."));
    assert_eq!(v["status"], "written", "{v}");
    let text = front(&env, &path);
    assert!(text.contains("edited_by:") && text.contains("source:") && text.contains("inbox: 0a1b2c3d4e"), "the source is kept:\n{text}");
    assert!(env.rkb(&["lint"]).status.success());
}

#[test]
fn import_claude_memory_into_the_inbox() {
    let env = kb_with_topics();
    let mem = env.dir.path().join("claude-memory");
    for (name, body) in [
        ("MEMORY.md", "- [a](a.md)\n".to_string()),
        (
            "a.md",
            "---\nname: prefer-ninja\ndescription: Builds use ninja\nmetadata:\n  type: feedback\n---\nUse `-G Ninja` for every build.\n"
                .to_string(),
        ),
        ("b.md", "---\nname: hdf5-root\ntype: project\n---\nSet HDF5_ROOT before configuring.\n".to_string()),
        ("c.md", "No frontmatter here, just a note.\n".to_string()),
    ] {
        env.write_abs(&mem.join(name), &body);
    }
    let claude = env.dir.path().join("CLAUDE.md");
    env.write_abs(&claude, "# Project\nIntro line.\n\n## Build\nRun make.\n\n## Test\nRun make test.\n");
    let before = commit_count(&env);
    let (v, code) = env.json(&["import", "--claude-memory", mem.to_str().unwrap(), "--file", claude.to_str().unwrap()], "");
    assert_eq!(code, Some(0), "{v}");
    assert_eq!((v["saved"].as_u64(), v["skipped"].as_u64()), (Some(6), Some(0)), "3 memory files, and 3 sections of CLAUDE.md: {v}");
    let (v, _) = env.json(&["import", "--claude-memory", mem.to_str().unwrap(), "--file", claude.to_str().unwrap()], "");
    assert_eq!((v["saved"].as_u64(), v["skipped"].as_u64()), (Some(0), Some(6)), "nothing twice: {v}");
    assert_eq!(commit_count(&env), before, "nothing reaches the knowledge base directly");
    let (v, _) = env.json(&["inbox"], "");
    let items = v["items"].as_array().unwrap_or_else(|| panic!("{v}"));
    assert_eq!(items.len(), 6);
    assert!(items.iter().all(|i| i["priority"] == 2), "{v}");
    let all = items.iter().map(|i| i["preview"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>().join("\n");
    assert!(all.contains("From Claude Code memory (feedback): prefer-ninja") && all.contains("(project): hdf5-root"), "{all}");
}

#[test]
fn tools_carry_images_and_search_shows_counts() {
    let env = search_kb();
    let img = env.dir.path().join("a.png");
    std::fs::write(&img, png_with_gps()).unwrap();
    let args = serde_json::json!({
        "type": "fact", "title": "Tool images land next to the lesson", "topic": "git",
        "statement": "It shows ![a](a.png).", "evidence": "Seen in the test.", "assets": [img.to_str().unwrap()],
    });
    let (out, code) = tool(&env, "rkb_add", &args);
    assert_eq!(code, Some(0), "{out}");
    let path = out.lines().find_map(|l| l.strip_prefix("path: ")).unwrap().trim_matches('"').to_string();
    let stem = path.trim_end_matches(".md");
    assert!(env.kb().join(format!("{stem}.assets/a.png")).is_file(), "{out}");

    let (out, _) = tool(&env, "rkb_used", &serde_json::json!({ "id": "1a00000012", "result": "worked" }));
    assert!(out.contains("recorded"), "{out}");
    let (v, _) = env.json(&["search", "undefined reference to vtable"], "");
    let row = v["results"].as_array().unwrap().iter().find(|r| r["id"] == "1a00000012").cloned().unwrap_or_else(|| panic!("{v}"));
    assert_eq!((row["worked"].as_u64(), row["failed"].as_u64()), (Some(1), Some(0)), "{row}");
}

#[test]
fn write_burst_asks_before_flooding() {
    let env = kb_with_topics();
    env.trust_claude();
    let toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap() + "\n[writes]\nburst = 2\n";
    env.write("kb.toml", &toml);
    add_ok(&env, "cmake", &lesson_with("First lesson of the burst"));
    add_ok(&env, "cmake", &lesson_with("Zeroth lesson of the burst"));
    let before = commit_count(&env);
    let add_as = |session: &str, title: &str| -> serde_json::Value {
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.env("RKB_SESSION", session);
        serde_json::from_slice(&rkb_with(c, &["add", "--topic", "cmake", "--format", "json"], &lesson_with(title)).stdout).unwrap()
    };
    let v = add_as("s1", "Second lesson of the burst");
    assert_eq!(v["status"].as_str(), Some("needs_user"), "3 commits in the last hour > 2: {v}");
    assert!(v["question"].as_str().unwrap().contains("may be looping"), "{v}");
    assert_eq!(commit_count(&env), before, "nothing written");
    let (bad, _) = env.json(&["flag", "0000000000", "--reason", "x"], "");
    assert_eq!(bad["error"]["code"], "not_found", "a write that would fail does not ask: {bad}");
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("CLAUDECODE", "1");
    let o = rkb_with(c, &["confirm", v["request"].as_str().unwrap(), "--choice", "continue", "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["status"], "written", "{v}");

    assert_eq!(add_as("s2", "Other session in the burst")["status"], "needs_user", "the window is s1's");
    for t in ["Grace window lesson one", "Grace window lesson two", "Grace window lesson three"] {
        assert_eq!(add_as("s1", t)["status"], "written", "one continue covers s1's next writes");
    }
    assert_eq!(add_as("s1", "Grace window lesson four")["status"], "needs_user", "3 commits since the continue > 2: a loop");
    let grace = env.dir.path().join("state/rkb/burst-grace.json");
    assert_eq!(std::fs::read_to_string(&grace).unwrap(), "{}", "the window closed");

    let head = stdout(&env.git(&["rev-parse", "HEAD"])).trim().to_string();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let window = |until: u64| serde_json::json!({ "s1": { "until": until, "head": head } }).to_string();
    std::fs::write(&grace, window(now - 60)).unwrap();
    assert_eq!(add_as("s1", "After the window ended")["status"], "needs_user", "an expired window asks");
    std::fs::write(&grace, window(now + 300)).unwrap();
    let (v, _) = env.json(&["doctor"], "");
    let line = v["checks"].as_array().unwrap().iter().find(|c| c["check"] == "burst grace").cloned().unwrap();
    assert!(line["detail"].as_str().unwrap().starts_with("session s1 until "), "{line}");
}

#[test]
fn burst_ignores_other_commits() {
    let env = kb_with_topics();
    let toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap() + "\n[writes]\nburst = 2\n";
    env.write("kb.toml", &toml);
    for i in 0..5 {
        env.write(&format!("site{i}.txt"), "x\n");
        env.git(&["add", "--", &format!("site{i}.txt")]);
        assert!(env.git(&["commit", "-q", "-m", &format!("chore: site {i}")]).status.success());
    }
    let v = add_ok(&env, "cmake", &lesson_with("After many other commits"));
    assert_eq!(v["status"], "written", "{v}");
}

#[test]
fn burst_default_is_200() {
    assert_eq!(rkb_core::write::DEFAULT_BURST, 200);
}

fn stuck_cmd(env: &Env, args: &[&str], input: &str) -> serde_json::Value {
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_SESSION", "s1");
    serde_json::from_slice(&rkb_with(c, &[args, &["--format", "json"]].concat(), input).stdout).unwrap()
}

#[test]
fn stuck_third_edit_and_ping_pong_ask() {
    let env = kb_with_topics();
    env.trust_claude();
    let v = stuck_cmd(&env, &["add", "--topic", "cmake"], &lesson_with("Stuck lesson"));
    assert_eq!(v["status"], "written", "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    let (a, _) = current(&env, &id);
    let b = a.replace("Set HDF5_ROOT.", "Set HDF5_ROOT to the prefix.");
    assert_ne!(a, b);
    let edit = |text: &str| {
        let (_, hash) = current(&env, &id);
        stuck_cmd(&env, &["edit", &id, "--base", &hash], text)
    };
    assert_eq!(edit(&b)["status"], "written", "second write of the lesson");
    let v = edit(&a);
    assert_eq!(v["status"], "needs_user", "third write of the lesson: {v}");
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("CLAUDECODE", "1");
    let o = rkb_with(c, &["confirm", v["request"].as_str().unwrap(), "--choice", "continue", "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["status"], "written", "continue lets it through: {v}");
    let (v, _) = env.json(&["edit", &id, "--base", &current(&env, &id).1], &b);
    assert_eq!(v["status"], "written", "no session, not checked: {v}");
}

#[test]
fn a_demote_that_needs_the_user_exits_three() {
    let env = kb_with_topics();
    env.trust_claude();
    let v = stuck_cmd(&env, &["add", "--topic", "cmake"], &lesson_with("Demote question"));
    assert_eq!(v["status"], "written", "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    let (a, _) = current(&env, &id);
    let b = a.replace("Set HDF5_ROOT.", "Set HDF5_ROOT to the prefix.");
    assert_eq!(stuck_cmd(&env, &["edit", &id, "--base", &current(&env, &id).1], &b)["status"], "written");
    let used = |args: &[&str]| {
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.env("RKB_SESSION", "s1");
        let o = rkb_with(c, &[args, &["--format", "json"]].concat(), "");
        (serde_json::from_slice::<serde_json::Value>(&o.stdout).unwrap(), o.status.code())
    };
    let (v, code) = used(&["used", &id, "--irrelevant", "--reason", "a"]);
    assert_eq!((v["status"].as_str(), code), (Some("recorded"), Some(0)), "{v}");
    // Two bad reports on a lesson this session already wrote twice: the demote archive is the third
    // write, so the stuck guard asks. The question must reach the caller with exit 3.
    let (v, code) = used(&["used", &id, "--irrelevant", "--reason", "b"]);
    assert_eq!((v["status"].as_str(), code), (Some("needs_user"), Some(3)), "the demote archive asks the user: {v}");
    assert!(v["question"].as_str().unwrap().contains("may be stuck"), "{v}");
    assert_eq!(v["recorded"]["result"], "irrelevant", "the report is kept: {v}");
    assert!(current(&env, &id).0.contains("status: active"), "nothing was archived yet");
}

#[test]
fn stuck_ignores_bulk_adds() {
    let env = kb_with_topics();
    for i in 0..12 {
        let v = stuck_cmd(&env, &["add", "--topic", "cmake"], &lesson_with(&format!("Bulk lesson number {i}")));
        assert_eq!(v["status"], "written", "{v}");
    }
}

#[test]
fn trusted_harness_confirms_without_terminal() {
    let env = kb_with_topics();
    env.trust_claude();
    let (v, _) = env.json(&["add", "--topic", "cmake"], &public_pitfall());
    let req = v["request"].as_str().unwrap();
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("CLAUDECODE", "1");
    let o = rkb_with(c, &["confirm", req, "--choice", LOOSEN, "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["status"], "written", "{v}");
    assert!(std::fs::read_to_string(env.kb().join(v["path"].as_str().unwrap())).unwrap().contains("sensitivity: public"));
}

#[test]
fn project_scope_and_alias_paths() {
    let env = kb_with_topics();
    let v = add_ok(&env, "c++", &lesson_with("Avoid std::regex in hot loops").replace("tags:", "tags:\n  - regex"));
    assert!(v["path"].as_str().unwrap().starts_with("general/cpp/avoid-std-regex-in-hot-loops"));
    let proj = PITFALL.replace("verified_how: ran", "verified_how: ran\nwhen:\n  project: dftracer");
    let v = add_ok(&env, "cmake", &proj);
    assert!(v["path"].as_str().unwrap().starts_with("projects/dftracer/cmake/"), "{v}");
    assert_eq!(v["notes"], serde_json::json!(["created the folder projects/dftracer/cmake"]));
    assert!(!env.kb().join("projects/dftracer/README.md").exists(), "no note without a matching repository");
}

#[test]
fn duplicates_get_a_note() {
    let env = kb_with_topics();
    let first = add_ok(&env, "cpp", &lesson_with("Avoid std::regex in hot loops"));
    let id = first["id"].as_str().unwrap();
    let v = add_ok(&env, "cpp", &lesson_with("Avoid std regex in hot loops too"));
    let notes = v["notes"].as_array().unwrap();
    assert!(notes.len() == 1 && notes[0].as_str().unwrap().contains(id) && notes[0].as_str().unwrap().contains("rkb edit"), "{v}");
    let v = add_ok(&env, "cpp", &lesson_with("Link libstdc++ statically on old clusters"));
    assert_eq!(v["notes"], serde_json::json!([]));
    let v = add_ok(&env, "cpp", &lesson_with("Avoid std::regex in hot loops"));
    assert_eq!(v["path"], "general/cpp/avoid-std-regex-in-hot-loops-2.md");
    let human = stdout(&env.rkb_in(&["add", "--topic", "cpp", "--format", "human"], &lesson_with("Avoid std::regex in hot loops")));
    assert!(human.contains("note: looks like"), "{human}");
}

#[test]
fn looser_label_needs_user() {
    let env = kb_with_topics();
    let public = PITFALL.replace("tags:", "labels:\n  sensitivity: public\ntags:");
    let (v, code) = env.json(&["add", "--topic", "cmake"], &public);
    assert_eq!(code, Some(3));
    assert_eq!(v["options"][0], "loosen sensitivity to public");
    let strict = PITFALL.replace("tags:", "labels:\n  sensitivity: confidential\ntags:");
    add_ok(&env, "cmake", &strict);
}

fn show(env: &Env, id: &str) -> serde_json::Value {
    env.json(&["show", id], "").0
}

fn current(env: &Env, id: &str) -> (String, String) {
    let s = show(env, id);
    let path = s["path"].as_str().unwrap().to_string();
    (std::fs::read_to_string(env.kb().join(&path)).unwrap(), s["hash"].as_str().unwrap().to_string())
}

#[test]
fn edit_rules() {
    let env = kb_with_topics();
    let id = add_ok(&env, "cmake", PITFALL)["id"].as_str().unwrap().to_string();
    let (text, hash) = current(&env, &id);

    let (v, _) = env.json(&["edit", &id, "--base", &hash], &text);
    assert_eq!(v["status"], "unchanged");
    let (v, code) = env.json(&["edit", &id], &text);
    assert_eq!((v["error"]["code"].as_str(), code), (Some("usage"), Some(2)));
    let (v, _) = env.json(&["edit", &id, "--base", &format!("{hash}6")], &text);
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(
        v["error"]["code"] != "conflict" && msg.contains("13 characters") && msg.contains("do not retype"),
        "a retyped base is a typo, not a change: {v}"
    );

    for (from, to) in [
        (id.as_str(), "0000000000"),
        ("schema: 1", "schema: 2"),
        ("verified_how: ran", "verified_how: checked"),
        ("status: active", "status: archived"),
    ] {
        let (v, _) = env.json(&["edit", &id, "--base", &hash], &text.replacen(from, to, 1));
        assert!(matches!(v["error"]["code"].as_str(), Some("refused")), "{from}: {v}");
    }

    let edited = text.replace("Set HDF5_ROOT.", "Set HDF5_ROOT to the install prefix.");
    let (v, _) = env.json(&["edit", &id, "--base", &hash], &edited);
    assert_eq!(v["status"], "written", "{v}");
    assert!(head_subject(&env).starts_with("edit(general/cmake): "));

    let (v, _) = env.json(&["edit", &id, "--base", &hash], &edited.replace("prefix", "root"));
    assert_eq!(v["error"]["code"], "conflict");
    assert!(v["error"]["message"].as_str().unwrap().contains(&current(&env, &id).1));
}

#[test]
fn flag_revive_used_and_log() {
    let env = kb_with_topics();
    let id = add_ok(&env, "cmake", PITFALL)["id"].as_str().unwrap().to_string();
    let (v, _) = env.json(&["flag", &id, "--reason", "fix failed with HDF5 1.14.3"], "");
    assert_eq!(v["status"], "written");
    let (text, hash) = current(&env, &id);
    assert!(text.contains("status: stale\nstale_reason: fix failed with HDF5 1.14.3\n"));
    let (v, _) = env.json(&["flag", &id, "--reason", "  "], "");
    assert_eq!(v["error"]["code"], "refused");

    let revived = text.replace("Configure passed with the flag.", "Configure passed with the flag on HDF5 1.14.3 too.");
    let (v, _) = env.json(&["edit", &id, "--base", &hash], &revived);
    assert_eq!(v["status"], "written", "{v}");
    let (text, _) = current(&env, &id);
    assert!(text.contains("status: active") && !text.contains("stale_reason"));

    let (v, _) = env.json(&["used", &id, "--worked"], "");
    assert_eq!(v["status"], "recorded");
    assert!(show(&env, &id)["last_worked"].is_string());
    assert!(show(&env, &id)["last_failed"].is_null());
    // Two worked reports keep the harmful count at or below the helpful one, so the failure below
    // flags the lesson without also demoting it.
    assert_eq!(env.json(&["used", &id, "--worked"], "").0["status"], "recorded");
    let (v, _) = env.json(&["used", &id, "--irrelevant", "--reason", "about pi events, not sed"], "");
    assert_eq!((v["status"].as_str(), v.get("flag")), (Some("recorded"), None), "{v}");
    assert!(current(&env, &id).0.contains("status: active"), "irrelevant never flags");
    assert_eq!(show(&env, &id)["usage"]["irrelevant"], 1);
    let irrelevant: Vec<_> = usage_records(&env).into_iter().filter(|r| r["event"] == "irrelevant").collect();
    assert_eq!((irrelevant.len(), irrelevant[0]["reason"].as_str()), (1, Some("about pi events, not sed")));
    assert_eq!(env.rkb(&["used", &id, "--worked", "--irrelevant"]).status.code(), Some(2), "one result only");
    let commits = stdout(&env.git(&["rev-list", "--count", "HEAD"]));
    let (v, _) = env.json(&["used", &id, "--failed", "--reason", "still fails"], "");
    assert_eq!(v["flag"]["status"], "written");
    assert_eq!(
        stdout(&env.git(&["rev-list", "--count", "HEAD"])).trim().parse::<u32>().unwrap(),
        commits.trim().parse::<u32>().unwrap() + 1
    );
    assert!(show(&env, &id)["last_failed"].is_string());

    let (v, _) = env.json(&["log", &id], "");
    let subjects: Vec<&str> = v["commits"].as_array().unwrap().iter().map(|c| c["subject"].as_str().unwrap()).collect();
    assert_eq!(subjects.iter().map(|s| s.split('(').next().unwrap()).collect::<Vec<_>>(), ["flag", "edit", "flag", "add"]);
}

#[test]
fn template_prints_skeleton() {
    let env = Env::new();
    let o = env.rkb(&["add", "--type", "recipe", "--template"]);
    let out = stdout(&o);
    assert!(out.contains("type: recipe") && out.contains("## When to use") && out.contains("## Steps"), "{out}");
    assert!(out.starts_with("---\ntype: recipe\n"), "{out}");
    assert!(!env.kb().exists());
}

#[test]
fn parallel_writers_and_appends() {
    let env = kb_with_topics();
    // A loaded machine has held the lock past the 10 s default while the other writer waits.
    let toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    env.write("kb.toml", &format!("{toml}\n[lock]\nwait_s = 60\n"));
    env.git(&["commit", "-qam", "lock wait"]);
    let children: Vec<_> = ["Parallel lesson one about builds", "Another lesson on linking order"]
        .iter()
        .map(|t| {
            let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
            c.args(["add", "--topic", "cmake", "--format", "json"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped());
            let mut child = c.spawn().unwrap();
            use std::io::Write;
            child.stdin.take().unwrap().write_all(lesson_with(t).as_bytes()).unwrap();
            child
        })
        .collect();
    for c in children {
        let o = c.wait_with_output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout));
    }
    assert_eq!(stdout(&env.git(&["rev-list", "--count", "HEAD"])).trim(), "5", "two adds on top of the setup commits");

    let id = show_first_id(&env);
    let used: Vec<_> = (0..20)
        .map(|_| env.cmd(env!("CARGO_BIN_EXE_rkb")).args(["used", &id, "--worked"]).stdin(std::process::Stdio::null()).spawn().unwrap())
        .collect();
    for mut c in used {
        assert!(c.wait().unwrap().success());
    }
    let files: Vec<_> = std::fs::read_dir(env.kb().join(".rkb/usage"))
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .collect();
    assert_eq!(files.len(), 1, "one file for this machine, even when 20 processes start at once");
    let log = std::fs::read_to_string(files[0].path()).unwrap();
    assert_eq!(log.lines().count(), 20);
    for l in log.lines() {
        let v: serde_json::Value = serde_json::from_str(l).unwrap();
        assert_eq!(v["event"], "worked");
    }
}

#[test]
fn review_reads_use_records() {
    let env = search_kb();
    // Fixture dates age; move every lesson to today so only `unused` depends on the date.
    let today = jiff::Zoned::now().date().to_string();
    for f in stdout(&env.git(&["ls-files", "*.md"])).lines() {
        let e = env.kb().join(f);
        let text = std::fs::read_to_string(&e).unwrap();
        std::fs::write(&e, text.replace("verified: 2026-09-25", &format!("verified: {today}"))).unwrap();
    }
    let old = env.kb().join("general/git/squash-fixups-with-autosquash.md");
    let text = std::fs::read_to_string(&old).unwrap();
    std::fs::write(&old, text.replace(&format!("verified: {today}"), "verified: 2020-01-01")).unwrap();
    let dir = env.kb().join(".rkb/usage");
    std::fs::create_dir_all(&dir).unwrap();
    let line = |id: &str, event: &str| format!("{{\"time\":\"{today}T00:00:00Z\",\"id\":\"{id}\",\"event\":\"{event}\"}}\n");
    let mut text = line("1a00000012", "failed") + &line("1a00000012", "failed");
    for _ in 0..5 {
        text += &line("1a00000003", "injected");
    }
    text += &line("1a00000005", "injected");
    for _ in 0..4 {
        text += &line("1a00000006", "irrelevant");
    }
    text += &line("1a00000006", "worked");
    for _ in 0..3 {
        text += &line("1a00000007", "irrelevant");
    }
    for _ in 0..5 {
        text += &line("1a00000007", "worked");
    }
    std::fs::write(dir.join("other-0001.jsonl"), text).unwrap();
    let (v, code) = env.json(&["review"], "");
    assert_eq!(code, Some(0), "{v}");
    let reasons = |id: &str| -> Vec<String> {
        v["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["id"] == id)
            .flat_map(|c| c["reasons"].as_array().unwrap().iter().map(|r| r["kind"].as_str().unwrap().to_string()))
            .collect()
    };
    assert_eq!(reasons("1a00000012"), ["failed_repeatedly"], "{v}");
    assert_eq!(reasons("1a00000003"), ["never_helped"], "{v}");
    assert_eq!(reasons("1a00000006"), ["often_irrelevant"], "{v}");
    let detail = v["candidates"].as_array().unwrap().iter().find(|c| c["id"] == "1a00000006").unwrap()["reasons"][0]["detail"].clone();
    assert_eq!(detail, "irrelevant 4, helped 1");
    assert!(reasons("1a00000007").is_empty(), "it mostly helps: {v}");
    assert!(reasons("1a00000005").is_empty(), "one injection is not enough, and a recent record counts as use: {v}");

    std::fs::write(dir.join("other-0001.jsonl"), "").unwrap();
    let (v, _) = env.json(&["review"], "");
    let unused: Vec<_> =
        v["candidates"].as_array().unwrap().iter().filter(|c| c["reasons"][0]["kind"] == "unused").map(|c| c["id"].clone()).collect();
    assert_eq!(unused, [serde_json::json!("1a00000005")], "{v}");
}

/// Every use record in the knowledge base, over every machine's file.
fn usage_records(env: &Env) -> Vec<serde_json::Value> {
    let mut out = vec![];
    for e in std::fs::read_dir(env.kb().join(".rkb/usage")).into_iter().flatten().flatten() {
        let text = std::fs::read_to_string(e.path()).unwrap_or_default();
        out.extend(text.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()));
    }
    out
}

fn show_first_id(env: &Env) -> String {
    let dir = env.kb().join("general/cmake");
    let file = std::fs::read_dir(dir).unwrap().flatten().find(|e| e.file_name() != "README.md").unwrap();
    let text = std::fs::read_to_string(file.path()).unwrap();
    text.lines().find_map(|l| l.strip_prefix("id: ")).unwrap().trim_matches('"').to_string()
}

#[test]
fn held_lock_gives_locked_error() {
    let env = kb_with_topics();
    let toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap() + "\n[lock]\nwait_s = 1\n";
    env.write("kb.toml", &toml);
    let host = stdout(&Command::new("hostname").output().unwrap()).trim().to_string();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    env.write(".git/rkb.lock", &format!("{host} {} {now}", std::process::id()));
    let (v, _) = env.json(&["add", "--topic", "cmake"], PITFALL);
    assert_eq!(v["error"]["code"], "locked", "{v}");
    env.write(".git/rkb.lock", &format!("{host} 999999 {now}"));
    add_ok(&env, "cmake", PITFALL);
    let (v, _) = env.json(&["add", "--topic", "general/cmake/"], &PITFALL.replace("# CMake cannot", "# Again, CMake cannot"));
    assert!(v["path"].as_str().unwrap().starts_with("general/cmake/"), "the whole folder works as a topic: {v}");
}

#[test]
fn expired_request() {
    let env = kb_with_topics();
    let (v, _) = env.json(&["add", "--topic", "cmake"], &public_pitfall());
    let req = v["request"].as_str().unwrap();
    let file = env.dir.path().join(format!("state/rkb/requests/{req}.json"));
    let mut r: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    r["created"] = serde_json::json!(0);
    std::fs::write(&file, r.to_string()).unwrap();
    let (v, _) = env.json(&["confirm", req, "--choice", LOOSEN], "");
    assert_eq!(v["error"]["code"], "expired");
}

#[test]
fn piped_stdin_never_starts_an_editor() {
    let env = kb_with_topics();
    let marker = env.dir.path().join("editor-ran");
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("VISUAL", format!("touch {}", marker.display()));
    let o = rkb_with(c, &["add", "--topic", "cmake", "--type", "pitfall"], PITFALL);
    assert!(o.status.success(), "{}", stdout(&o));
    assert!(!marker.exists());
}

#[test]
fn long_diff_is_cut_and_needs_user_reads_well_for_people() {
    let env = kb_with_topics();
    let evidence: String = (1..=100).map(|i| format!("Run {i} passed.\n")).collect();
    let v = add_ok(&env, "cmake", &PITFALL.replace("Configure passed with the flag.\n", &evidence));
    let diff = v["diff"].as_array().unwrap();
    let last = diff.last().unwrap().as_str().unwrap();
    assert_eq!(diff.len(), 81);
    assert!(last.contains("lines; run `git -C") && last.contains(v["commit"].as_str().unwrap()), "{last}");

    let o = env.rkb_in(
        &["add", "--topic", "cmake", "--format", "human"],
        &public_pitfall().replace("CMake cannot find HDF5", "CMake still cannot find HDF5"),
    );
    assert_eq!(o.status.code(), Some(3));
    let out = stdout(&o);
    assert!(
        out.contains("Needs your decision:")
            && out.contains(&format!("Options: \"{LOOSEN}\""))
            && out.contains("Answer with: rkb confirm r-")
            && !out.contains("  1. "),
        "{out}"
    );
}

#[test]
fn full_write_flow() {
    let env = Env::new();
    env.init();
    env.trust_claude();
    let trusted = |args: &[&str]| {
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.env("CLAUDECODE", "1");
        let mut all = args.to_vec();
        all.extend(["--format", "json"]);
        serde_json::from_slice::<serde_json::Value>(&rkb_with(c, &all, "").stdout).unwrap()
    };

    let v = add_ok(&env, "cmake", PITFALL);
    assert_eq!(v["notes"], serde_json::json!(["created the folder general/cmake"]));
    let id = v["id"].as_str().unwrap().to_string();

    let (v, _) = env.json(
        &["add", "--topic", "cmake"],
        &public_pitfall().replace("CMake cannot find HDF5", "Something else about cmake builds and HDF5"),
    );
    let v = trusted(&["confirm", v["request"].as_str().unwrap(), "--choice", "cancel"]);
    assert_eq!(v["status"], "cancelled");

    let (text, hash) = current(&env, &id);
    let (v, _) = env.json(&["edit", &id, "--base", &hash], &text.replace("Set HDF5_ROOT.", "Set HDF5_ROOT to the prefix."));
    assert_eq!(v["status"], "written");
    assert_eq!(env.json(&["flag", &id, "--reason", "breaks on 1.14.3"], "").0["status"], "written");
    assert_eq!(env.json(&["used", &id, "--worked"], "").0["status"], "recorded");
    assert_eq!(env.json(&["log", &id], "").0["commits"].as_array().unwrap().len(), 3);
    assert!(show(&env, &id)["last_worked"].is_string());
    let lint = env.rkb(&["lint"]);
    assert!(lint.status.success(), "{}", stdout(&lint));
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "?? .rkb/\n", "the use record waits for the next rkb commit");
    assert_eq!(
        env.json(&["doctor"], "").0["checks"].as_array().unwrap().iter().find(|c| c["check"] == "uncommitted changes").unwrap()["status"],
        "ok"
    );
    let (text, hash) = current(&env, &id);
    let (v, _) = env.json(&["edit", &id, "--base", &hash], &text.replace("to the prefix.", "to the install prefix."));
    assert_eq!(v["status"], "written");
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "", "the next write carries it");
    let files = stdout(&env.git(&["show", "--name-only", "--format=", "HEAD"]));
    assert!(files.contains(".rkb/usage/") && files.contains(".rkb/.gitignore"), "{files}");
}

fn list_cmd(env: &Env, args: &[&str], columns: &str, lang: &str) -> String {
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("COLUMNS", columns).env("LANG", lang).env_remove("LC_ALL").env_remove("LC_CTYPE");
    let o = rkb_with(c, args, "");
    assert!(o.status.success(), "{}", stdout(&o));
    String::from_utf8(o.stdout).unwrap()
}

fn fixture_kb() -> Env {
    let env = Env::new();
    env.init();
    env.copy_fixture();
    env.git(&["add", "-A"]);
    assert!(env.git(&["commit", "-q", "-m", "add: fixture"]).status.success());
    env
}

#[test]
fn list_topic_snapshot() {
    let env = fixture_kb();
    let out = list_cmd(&env, &["list", "projects/dftracer/cmake", "--format", "human"], "80", "en_US.UTF-8");
    assert_eq!(
        out,
        "projects/dftracer/cmake · 1 lesson\n\ncmake\n  ! CMake cannot find HDF5 unless HDF5_ROOT…   system tuolumne  hdf5 1.12:1.14.2\n\n! pitfall\n"
    );
    assert!(!out.contains('\x1b'));
}

#[test]
fn list_overview_snapshot_and_wrap() {
    let env = fixture_kb();
    let out = list_cmd(&env, &["list", "--format", "human"], "80", "en_US.UTF-8");
    assert_eq!(
        out,
        "~/kb · 5 lessons\n\ngeneral                                                2\n  cpp 1   git 1\n\nprojects\n  dftracer                                             2\n    cmake 1   lustre 1\n\nsystems\n  tuolumne                                             1\n    lustre 1\n"
    );
    for t in ["cmake", "latex", "lustre", "peer-review", "writing"] {
        env.write(&format!("general/{t}/a.md"), &FACT.replace("0a1b2c3d4e", &format!("{:0>10}", t.len())).replace("tags:\n  - demo\n", ""));
    }
    let narrow = list_cmd(&env, &["list", "general", "--format", "human"], "40", "en_US.UTF-8");
    assert!(narrow.lines().all(|l| l.chars().count() <= 40), "{narrow}");
    assert!(narrow.lines().filter(|l| l.starts_with("  ")).count() >= 2, "{narrow}");
}

#[test]
fn list_topic_markers_sections_and_widths() {
    let env = fixture_kb();
    let base = |id: &str, title: &str, extra: &str| {
        FACT.replace("0a1b2c3d4e", id).replace("# A fact", &format!("# {title}")).replace("status: active\n", extra)
    };
    env.write(
        "general/io/a.md",
        &base(
            "1000000001",
            "A very long title about striping large checkpoint files on parallel file systems",
            "status: active\nwhen:\n  fs: lustre\n",
        ),
    );
    env.write("general/io/b.md", &base("1000000002", "Stale one", "status: stale\nstale_reason: broke\n"));
    env.write(
        "general/io/c.md",
        &base("1000000003", "Checked one", "status: active\n").replace("verified_how: ran", "verified_how: checked"),
    );
    env.write("general/io/d.md", &(base("1000000004", "Old one", "status: archived\n") + "\n## Why archived\nGone.\n"));
    env.write("general/io/README.md", "# Input and output\n");

    let out = list_cmd(&env, &["list", "general/io", "--format", "human"], "50", "en_US.UTF-8");
    assert!(out.starts_with("general/io · Input and output · 4 lessons\n"), "{out}");
    assert!(out.lines().all(|l| l.chars().count() <= 50), "{out}");
    assert!(out.contains("…"));
    assert!(out.lines().any(|l| l.contains("Stale one") && l.trim_end().ends_with("stale")));
    assert!(out.lines().any(|l| l.contains("Checked one") && l.trim_end().ends_with("✓")));
    let archived = out.find("archived · superseded").unwrap();
    assert!(out[archived..].contains("Old one") && out.find("Old one").unwrap() > archived);
    assert!(out.trim_end().ends_with("· fact  ✓ checked"), "{out}");

    let ascii = list_cmd(&env, &["list", "general/io", "--format", "human"], "80", "C");
    assert!(ascii.contains("general/io - Input and output - 4 lessons") && ascii.contains("  - Stale one"), "{ascii}");
    assert!(!ascii.contains('·') && !ascii.contains('✓'));

    let toon = list_cmd(&env, &["list", "general/io", "--toon"], "80", "en_US.UTF-8");
    assert!(toon.contains("lessons[4]{id,type,status,title,tags,when,verified}:"), "{toon}");
    assert!(toon.contains("Run `rkb show <id>`"));
    let rows: Vec<&str> = toon.lines().filter(|l| l.starts_with("  \"1000")).collect();
    assert_eq!(rows.len(), 4);
    assert!(rows[3].starts_with("  \"1000000004\",fact,archived"), "{toon}");
}

#[test]
fn list_is_read_only_and_reports_unknown_folders() {
    let env = fixture_kb();
    for args in [vec!["list"], vec!["list", "general"], vec!["list", "projects/dftracer"], vec!["list", "general/cpp"]] {
        assert!(env.rkb(&args).status.success());
    }
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "");
    let (v, code) = env.json(&["list", "general/nope"], "");
    assert_eq!((v["error"]["code"].as_str(), code), (Some("not_found"), Some(1)));
    assert!(v["error"]["fix"].as_str().unwrap().contains("rkb list"));
    let (v, _) = env.json(&["list"], "");
    assert_eq!(v["lessons"], 5);
    assert_eq!(v["topics"].as_array().unwrap().len(), 5);
}

fn project_repo(env: &Env, remote: &str) -> PathBuf {
    let dir = env.dir.path().join("dftracer");
    std::fs::create_dir_all(&dir).unwrap();
    for args in [vec!["init", "-q"], vec!["commit", "-q", "--allow-empty", "-m", "root"], vec!["remote", "add", "origin", remote]] {
        assert!(env.cmd("git").arg("-C").arg(&dir).args(&args).status().unwrap().success(), "{args:?}");
    }
    dir
}

fn in_dir(env: &Env, dir: &Path, args: &[&str], extra: &[(&str, &str)]) -> (serde_json::Value, String) {
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.current_dir(dir);
    for (k, v) in extra {
        c.env(k, v);
    }
    let mut all = args.to_vec();
    all.extend(["--format", "json"]);
    let o = rkb_with(c, &all, "");
    let human = {
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.current_dir(dir);
        for (k, v) in extra {
            c.env(k, v);
        }
        let mut all = args.to_vec();
        all.extend(["--format", "human"]);
        String::from_utf8_lossy(&rkb_with(c, &all, "").stdout).into_owned()
    };
    (serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o))), human)
}

#[test]
fn status_and_context_with_and_without_a_match() {
    let env = fixture_kb();
    let outside = env.dir.path().join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    let (v, human) = in_dir(&env, &outside, &[], &[]);
    assert!(v["project"].is_null() && v["system"].is_null(), "{v}");
    assert_eq!(v["lessons"]["general"], 2);
    assert!(human.contains("none matched here"), "{human}");

    let repo = project_repo(&env, "git@github.com:llnl/dftracer.git");
    let (v, human) = in_dir(&env, &repo, &[], &[("RKB_SYSTEM", "tuolumne")]);
    assert_eq!(v["project"]["name"], "dftracer");
    assert_eq!(v["project"]["rule"], "remote github.com/llnl/dftracer");
    assert_eq!(v["system"]["name"], "tuolumne");
    assert_eq!(v["lessons"]["project"], 2);
    assert!(v["help"][0].as_str().unwrap().contains("rkb list projects/dftracer"));
    assert!(human.contains("dftracer") && human.contains("Next: rkb list projects/dftracer"), "{human}");

    let checkouts = std::fs::read_to_string(env.dir.path().join("state/rkb/checkouts.toml")).unwrap();
    assert!(checkouts.contains("dftracer"), "{checkouts}");

    for format in ["human", "toon", "json"] {
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.current_dir(&repo);
        let out = stdout(&rkb_with(c, &["context", "--format", format], ""));
        assert!(out.trim_end().lines().count() <= 5, "{format}: {out}");
        assert!(out.contains("rkb list projects/dftracer"), "{format}: {out}");
    }
}

#[test]
fn doctor_healthy_then_problems() {
    let env = Env::new();
    env.init();
    let (v, _) = env.json(&["doctor"], "");
    assert_eq!(v["failed"], 0, "{v}");

    std::fs::remove_file(env.kb().join(".git/hooks/pre-commit")).unwrap();
    env.write("general/demo/a.md", FACT);
    env.git(&["config", "--unset", "receive.denyCurrentBranch"]);
    let (v, _) = env.json(&["add", "--topic", "demo"], &public_pitfall());
    assert_eq!(v["status"], "needs_user", "loosening a label asks: {v}");
    env.write(".git/rkb.lock", "otherhost 1 0");
    let o = env.rkb(&["doctor", "--format", "json"]);
    assert_eq!(o.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let status =
        |name: &str| v["checks"].as_array().unwrap().iter().find(|c| c["check"] == name).unwrap_or_else(|| panic!("{name}")).clone();
    assert_eq!(status("pre-commit hook")["status"], "fail");
    for name in ["uncommitted changes", "pending requests", "write lock", "push to this clone"] {
        assert_eq!(status(name)["status"], "warn", "{name}");
        assert!(!status(name)["fix"].as_str().unwrap().is_empty(), "{name}");
    }

    let fix = status("pre-commit hook")["fix"].as_str().unwrap().to_string();
    assert!(Command::new("sh").arg("-c").arg(&fix).status().unwrap().success());
    let (v, _) = env.json(&["doctor"], "");
    let hook = v["checks"].as_array().unwrap().iter().find(|c| c["check"] == "pre-commit hook").unwrap().clone();
    assert_eq!(hook["status"], "ok", "{hook}");
}

#[test]
fn doctor_warns_on_a_mode_that_never_prompts() {
    let env = Env::new();
    env.init();
    let home = env.dir.path();
    std::fs::create_dir_all(home.join(".claude/plugins")).unwrap();
    std::fs::write(home.join(".claude/plugins/installed_plugins.json"), r#"{"version":2,"plugins":{"rkb@rkb":[{"version":"x"}]}}"#)
        .unwrap();
    let finding = |env: &Env| {
        let (v, code) = env.json(&["doctor"], "");
        (v["checks"].as_array().unwrap().iter().find(|c| c["check"] == "claude permissions").cloned(), code)
    };
    std::fs::write(home.join(".claude/settings.json"), r#"{"permissions":{"allow":[]}}"#).unwrap();
    assert_eq!(finding(&env).0, None);
    std::fs::write(home.join(".claude/settings.json"), r#"{"permissions":{"defaultMode":"bypassPermissions"}}"#).unwrap();
    let (c, code) = finding(&env);
    let c = c.expect("permission check");
    assert_eq!((c["status"].as_str(), code), (Some("warn"), Some(0)), "{c}");
    assert!(c["detail"].as_str().unwrap().contains("bypassPermissions") && c["fix"].as_str().unwrap().contains("rkb confirm"), "{c}");
}

#[test]
fn doctor_reports_missing_root_commit() {
    let env = fixture_kb();
    let repo = project_repo(&env, "https://github.com/llnl/dftracer");
    let (v, _) = in_dir(&env, &repo, &["doctor"], &[]);
    let c = v["checks"].as_array().unwrap().iter().find(|c| c["check"] == "project root commit").cloned().expect("root commit check");
    assert_eq!(c["status"], "warn");
    assert!(c["fix"].as_str().unwrap().starts_with("add `root_commit: "), "{c}");
}

#[test]
fn break_lock_confirmed_and_changed() {
    let env = Env::new();
    env.init();
    env.trust_claude();
    let (v, _) = env.json(&["doctor", "--break-lock"], "");
    assert_eq!(v["status"], "done");

    let lock = env.kb().join(".git/rkb.lock");
    env.write(".git/rkb.lock", "otherhost 1 5");
    let (v, code) = env.json(&["doctor", "--break-lock"], "");
    assert_eq!(code, Some(3));
    assert!(v["question"].as_str().unwrap().contains("otherhost 1 5"));
    let req = v["request"].as_str().unwrap().to_string();
    let confirm = |req: &str| {
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.env("CLAUDECODE", "1");
        serde_json::from_slice::<serde_json::Value>(
            &rkb_with(c, &["confirm", req, "--choice", "break lock", "--format", "json"], "").stdout,
        )
        .unwrap()
    };
    assert!(confirm(&req)["message"].as_str().unwrap().starts_with("Removed"));
    assert!(!lock.exists());

    env.write(".git/rkb.lock", "otherhost 1 5");
    let (v, _) = env.json(&["doctor", "--break-lock"], "");
    env.write(".git/rkb.lock", "otherhost 2 9");
    let out = confirm(v["request"].as_str().unwrap());
    assert!(out["message"].as_str().unwrap().contains("changed or is gone"), "{out}");
    assert!(lock.exists());
}

#[test]
fn add_creates_a_project_with_its_remote() {
    let env = Env::new();
    env.init();
    env.trust_claude();
    let repo = project_repo(&env, "git@github.com:llnl/dftracer.git");
    let lesson = PITFALL.replace("verified_how: ran", "verified_how: ran\nwhen:\n  project: dftracer");
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.current_dir(&repo);
    let v: serde_json::Value =
        serde_json::from_slice(&rkb_with(c, &["add", "--topic", "cmake", "--format", "json"], &lesson).stdout).unwrap();
    assert_eq!(v["status"], "written", "{v}");
    assert!(
        v["notes"].as_array().unwrap().iter().any(|n| n == "wrote projects/dftracer/README.md with the remote github.com/llnl/dftracer"),
        "{v}"
    );
    let note = std::fs::read_to_string(env.kb().join("projects/dftracer/README.md")).unwrap();
    assert!(note.starts_with("---\nremotes:\n  - github.com/llnl/dftracer\nroot_commit: "), "{note}");
    let files = stdout(&env.git(&["show", "--name-only", "--format=", "HEAD"]));
    assert_eq!(
        files.trim().lines().collect::<Vec<_>>(),
        ["projects/dftracer/README.md", "projects/dftracer/cmake/cmake-cannot-find-hdf5-unless-hdf5-root-is-set.md"]
    );
    assert!(env.rkb(&["lint"]).status.success());

    let other = Env::new();
    other.init();
    let repo = project_repo(&other, "git@github.com:someone/other-tool.git");
    let mut c = other.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.current_dir(&repo);
    let v: serde_json::Value =
        serde_json::from_slice(&rkb_with(c, &["add", "--topic", "cmake", "--format", "json"], &lesson).stdout).unwrap();
    assert_eq!(v["status"], "written", "{v}");
    assert!(!other.kb().join("projects/dftracer/README.md").exists());
}

#[test]
fn projects_without_a_remote_are_recognized() {
    let env = Env::new();
    env.init();
    env.trust_claude();
    let add_from = |dir: &Path, project: &str| -> serde_json::Value {
        let lesson = PITFALL.replace("verified_how: ran", &format!("verified_how: ran\nwhen:\n  project: {project}"));
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.current_dir(dir);
        serde_json::from_slice(&rkb_with(c, &["add", "--topic", "cmake", "--format", "json"], &lesson).stdout).unwrap()
    };

    // A git repository with no remote: its root commit goes into the project note.
    let tool = env.dir.path().join("tool");
    std::fs::create_dir_all(tool.join("src")).unwrap();
    for args in [vec!["init", "-q"], vec!["commit", "-q", "--allow-empty", "-m", "root"]] {
        assert!(env.cmd("git").arg("-C").arg(&tool).args(&args).status().unwrap().success());
    }
    let v = add_from(&tool, "tool");
    assert_eq!(v["status"], "written", "{v}");
    let note = std::fs::read_to_string(env.kb().join("projects/tool/README.md")).unwrap();
    assert!(note.starts_with("---\nroot_commit: ") && !note.contains("remotes"), "{note}");
    assert!(env.rkb(&["lint"]).status.success(), "the note is valid");
    let (v, _) = in_dir(&env, &tool.join("src"), &[], &[]);
    assert_eq!(
        (v["project"]["name"].as_str(), v["project"]["rule"].as_str().map(|r| r.starts_with("root commit"))),
        (Some("tool"), Some(true)),
        "{v}"
    );

    // No git at all (here an hg checkout): the path is remembered on this machine.
    let notes = env.dir.path().join("notes-proj");
    std::fs::create_dir_all(notes.join(".hg")).unwrap();
    std::fs::create_dir_all(notes.join("docs")).unwrap();
    let v = add_from(&notes.join("docs"), "notes-proj");
    assert_eq!(v["status"], "written", "{v}");
    assert!(v["notes"].to_string().contains("now matches project notes-proj on this machine"), "{v}");
    let (v, _) = in_dir(&env, &notes.join("docs"), &[], &[]);
    assert_eq!(v["project"]["name"], "notes-proj", "{v}");
    assert!(v["project"]["rule"].as_str().unwrap().contains("checkout"), "{v}");

    // A folder with another name is not taken for the project.
    let other = env.dir.path().join("elsewhere");
    std::fs::create_dir_all(&other).unwrap();
    let v = add_from(&other, "tool");
    assert!(!v["notes"].to_string().contains("now matches"), "{v}");
}

#[test]
fn show_applies_with_reasons() {
    let env = fixture_kb();
    let (v, _) = env.json(&["show", "7f3a9c2b41", "--with", "hdf5=1.14.3", "--system", "tuolumne"], "");
    assert_eq!(v["applies"]["result"], "no", "{v}");
    let hdf5 = v["applies"]["keys"].as_array().unwrap().iter().find(|k| k["key"] == "hdf5").unwrap().clone();
    assert_eq!((hdf5["want"].as_str(), hdf5["have"].as_str(), hdf5["result"].as_str()), (Some("1.12:1.14.2"), Some("1.14.3"), Some("no")));

    let human = stdout(&env.rkb(&["show", "7f3a9c2b41", "--with", "hdf5=1.14.3", "--system", "tuolumne", "--format", "human"]));
    assert!(human.contains("applies here: no (hdf5 wants 1.12:1.14.2, has 1.14.3)"), "{human}");
    let toon = stdout(&env.rkb(&["show", "7f3a9c2b41", "--with", "hdf5=1.14.3", "--system", "tuolumne", "--toon"]));
    assert!(toon.contains("keys[2]{key,want,have,result,reason}:"), "{toon}");

    let (v, _) = env.json(&["show", "7f3a9c2b41", "--with", "hdf5=1.14.1", "--system", "tuolumne"], "");
    assert_eq!(v["applies"]["result"], "yes");
    let (v, _) = env.json(&["show", "7f3a9c2b41"], "");
    assert_eq!(v["applies"]["result"], "unknown");
    let (v, _) = env.json(&["show", "5d2e8a1c90"], "");
    assert_eq!(v["applies"]["result"], "yes");
    assert_eq!(v["applies"]["keys"], serde_json::json!([]));

    let o = env.rkb(&["show", "7f3a9c2b41", "--with", "hdf5"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stdout(&o).contains("has no `=`"));
}

fn search_kb() -> Env {
    named_kb("search")
}

fn named_kb(name: &str) -> Env {
    let env = Env::new();
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures").join(name).join("kb");
    copy_dir(&src, &env.kb());
    for args in [
        vec!["init", "-q"],
        vec!["add", "-A"],
        vec!["-c", "user.name=T", "-c", "user.email=t@example.org", "commit", "-q", "-m", "fixture"],
    ] {
        assert!(env.git(&args).status.success());
    }
    env
}

#[test]
fn search_fixture_meets_recall() {
    let env = search_kb();
    let q = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/search/queries.toml");
    let o = env.rkb(&["eval", "--queries", q.to_str().unwrap(), "--min-recall", "0.9", "--format", "json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert!(o.status.success(), "{v}");
    assert!(v["queries"].as_u64().unwrap() >= 15);
    assert!(v["recall_at_5"].as_f64().unwrap() >= 0.9, "{v}");

    let bad = env.dir.path().join("bad.toml");
    std::fs::write(&bad, "[[query]]\ntext = \"x\"\nexpect = [\"0000000000\"]\n").unwrap();
    let (v, code) = env.json(&["eval", "--queries", bad.to_str().unwrap()], "");
    assert_eq!(code, Some(1));
    assert!(v["error"]["message"].as_str().unwrap().contains("0000000000"));
    let low = env.dir.path().join("low.toml");
    std::fs::write(&low, "[[query]]\ntext = \"nothing matches this\"\nexpect = [\"1a00000001\"]\n").unwrap();
    let (v, _) = env.json(&["eval", "--queries", low.to_str().unwrap()], "");
    assert_eq!(v["missed"], serde_json::json!(["1a00000001"]), "{v}");
    assert_eq!(env.rkb(&["eval", "--queries", low.to_str().unwrap(), "--min-recall", "0.5"]).status.code(), Some(1));

    let (v, code) = env.json(&["eval", "--self"], "");
    assert_eq!(code, Some(0), "{v}");
    let lessons = stdout(&env.git(&["grep", "-l", "-e", "^## Symptom", "-e", "^## Statement", "--", "*.md"])).lines().count();
    assert_eq!(v["queries"].as_u64().unwrap() as usize, lessons, "{v}");
    assert!(v["recall_at_5"].as_f64().unwrap() >= 0.9, "{v}");
}

#[test]
fn eval_fixture_meets_recall() {
    let env = named_kb("eval");
    let q = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/eval/queries.toml");
    let q = q.to_str().unwrap();
    let o = env.rkb(&["eval", "--queries", q, "--min-recall", "0.85", "--format", "json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert!(o.status.success(), "{v}");
    assert_eq!((v["queries"].as_u64(), v["answerable"].as_u64()), (Some(124), Some(102)), "{v}");
    let rows = v["rows"].as_array().unwrap();
    assert_eq!(rows.iter().filter(|r| r["answerable"] == false).count(), 22, "{v}");
    assert!(v["recall"].is_null(), "recall does not apply with BM25: {v}");
    let o = env.rkb(&["eval", "--queries", q]);
    assert!(stdout(&o).contains("recall does not apply"), "{}", stdout(&o));
}

#[test]
fn search_outputs_and_hidden_results() {
    let env = search_kb();
    let (v, _) = env.json(&["search", "undefined reference to vtable"], "");
    assert_eq!(v["results"][0]["id"], "1a00000012");
    assert_eq!(v["ranked_by"], "bm25");
    assert!(v["results"][0]["summary"].as_str().unwrap().starts_with("The link fails with"));

    let (v, _) = env.json(&["search", "H5_USE_18_API", "--with", "hdf5=1.8.21"], "");
    assert_eq!(v["results"], serde_json::json!([]), "{v}");
    assert_eq!((v["hidden"].as_u64(), v["hidden_by"]["hdf5"].as_u64()), (Some(1), Some(1)));
    let (v, _) = env.json(&["search", "H5_USE_18_API", "--with", "hdf5=1.8.21", "--all"], "");
    assert_eq!((v["results"][0]["id"].as_str(), v["results"][0]["applies"].as_str()), (Some("1a00000014"), Some("no")));

    let (v, _) = env.json(&["search", "zzzqqq nothing"], "");
    assert_eq!(v["results"], serde_json::json!([]));
    assert_eq!(v["help"].as_array().unwrap().len(), 3);
    let human = stdout(&env.rkb(&["search", "zzzqqq nothing", "--format", "human"]));
    assert!(human.contains("rkb find") && human.contains("--all") && human.contains("rkb list"), "{human}");

    let (v, _) = env.json(&["search", "--literal", "--start-group"], "");
    assert!(v["results"][0]["line"].as_str().unwrap().contains("--start-group"), "{v}");
    let (v, code) = env.json(&["search", "--regex", "("], "");
    assert_eq!((v["error"]["code"].as_str(), code), (Some("usage"), Some(2)));

    let (v, _) = env.json(&["search", "stripe large files"], "");
    assert!(
        v["results"].as_array().unwrap().iter().all(|r| r["id"] != "1a00000015"),
        "system lessons stay out without a system match: {v}"
    );
    let (v, _) = env.json(&["search", "stripe large files", "--system", "hpc1"], "");
    assert_eq!(v["results"][0]["id"], "1a00000015");

    let (v, _) = env.json(&["find", "undefind vtable"], "");
    assert_eq!(v["results"][0]["id"], "1a00000012");
    let toon = stdout(&env.rkb(&["search", "git rebase", "--toon"]));
    assert!(toon.contains("results[3]{id,type,title,path,status,tier,applies,worked,failed,injected,irrelevant,summary}:"), "{toon}");
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "");
}

fn set_chain(env: &Env, rest: &str) {
    let p = env.kb().join("kb.toml");
    let text = std::fs::read_to_string(&p).unwrap();
    let (head, tail) = text.split_once("[rerank]\n").unwrap();
    let tail = &tail[tail.find("\n\n").unwrap()..];
    std::fs::write(p, format!("{head}[rerank]\n{rest}{tail}")).unwrap();
}

fn ids(v: &serde_json::Value) -> Vec<String> {
    v["results"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect()
}

#[test]
fn search_falls_back_to_bm25_without_a_model() {
    let env = search_kb();
    let (plain, _) = env.json(&["search", "git rebase"], "");
    set_chain(&env, "chain = [\"missing\", \"bm25\"]");
    let (v, _) = env.json(&["search", "git rebase"], "");
    let by = v["ranked_by"].as_str().unwrap();
    assert!(by.starts_with("bm25 (missing: unknown backend `missing`; this build knows jev, bm25-bert and bm25)"), "{v}");
    assert_eq!(ids(&v), ids(&plain));
    assert!(v["results"][0].get("relevance").is_none(), "{v}");
    let human = stdout(&env.rkb(&["search", "git rebase", "--format", "human"]));
    assert!(human.contains("reranker: bm25 (missing: unknown backend"), "{human}");
    let (v, _) = env.json(&["search", "git rebase", "--no-model"], "");
    assert_eq!(v["ranked_by"], "bm25");

    set_chain(&env, "chain = [\"missing\"]\nstrict = true");
    let (v, code) = env.json(&["search", "git rebase"], "");
    assert_eq!(code, Some(1), "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("missing"), "{v}");
    let o =
        env.cmd(env!("CARGO_BIN_EXE_rkb")).args(["search", "git rebase", "--format", "json"]).env("RKB_NO_MODEL", "1").output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!((v["ranked_by"].as_str(), o.status.code()), (Some("bm25"), Some(0)), "{v}");

    set_chain(&env, "chain = [\"bm25-bert\", \"bm25\"]");
    let o = env
        .cmd(env!("CARGO_BIN_EXE_rkb"))
        .args(["search", "git rebase", "--format", "json"])
        .env("XDG_DATA_HOME", env.dir.path().join("empty-data"))
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let by = v["ranked_by"].as_str().unwrap();
    assert!(by.starts_with("bm25 (bm25-bert: not configured") && by.contains("rkb models fetch"), "{v}");
    assert_eq!(ids(&v), ids(&plain));
    let o = env
        .cmd(env!("CARGO_BIN_EXE_rkb"))
        .args(["doctor", "--format", "json"])
        .env("XDG_DATA_HOME", env.dir.path().join("empty-data"))
        .output()
        .unwrap();
    let d: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let check = d["checks"].as_array().unwrap().iter().find(|c| c["check"] == "bm25-bert").unwrap_or_else(|| panic!("{d}"));
    assert_eq!((check["status"].as_str(), check["fix"].as_str()), (Some("warn"), Some("rkb models fetch")), "{d}");
    set_chain(&env, "chain = [\"missing\", \"bm25\"]");

    let q = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/search/queries.toml");
    let q = q.to_str().unwrap();
    let (v, code) = env.json(&["eval", "--queries", q, "--rerank", "bm25"], "");
    assert_eq!((v["ranked_by"].as_str(), code), (Some("bm25"), Some(0)), "{v}");
    let (v, code) = env.json(&["eval", "--queries", q, "--rerank", "missing"], "");
    assert_eq!(code, Some(1), "a skipped backend fails the eval: {v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("missing"), "{v}");
}

fn dir_tree(dir: &Path) -> Vec<String> {
    fn walk(p: &Path, prefix: &str, out: &mut Vec<String>) {
        for e in std::fs::read_dir(p).unwrap().flatten() {
            let name = format!("{prefix}/{}", e.file_name().to_string_lossy());
            if e.file_type().unwrap().is_dir() {
                walk(&e.path(), &name, out);
            } else {
                out.push(name);
            }
        }
    }
    let mut out = vec![];
    walk(dir, "", &mut out);
    out.sort();
    out
}

#[test]
fn no_home_stops_before_writing_anything() {
    let env = search_kb();
    let clean = env.dir.path().join("empty-cwd");
    std::fs::create_dir_all(&clean).unwrap();
    let bare = |args: &[&str], vars: &[(&str, &str)]| {
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.current_dir(&clean);
        for k in ["RKB_HOME", "RKB_TEST_HOME", "HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_CONFIG_HOME"] {
            c.env_remove(k);
        }
        for (k, v) in vars {
            c.env(k, v);
        }
        c.args(args).stdin(std::process::Stdio::null()).output().unwrap()
    };
    let before = dir_tree(&clean);
    for args in [vec!["--format", "json", "list"], vec!["--format", "json", "search", "git rebase"], vec!["--format", "json", "doctor"]] {
        let o = bare(&args, &[]);
        assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        let msg = v["error"]["message"].as_str().unwrap();
        assert!(msg.contains("RKB_HOME") && msg.contains("HOME"), "names both: {v}");
        assert!(v["error"]["fix"].as_str().unwrap().contains("RKB_HOME") && v["error"]["fix"].as_str().unwrap().contains("HOME"), "{v}");
    }
    for (args, input) in [
        (vec!["mcp"], r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#),
        (vec!["hook", "session-start"], r#"{"session_id":"s1","cwd":"."}"#),
        (vec!["tool", "rkb_note"], r#"{"text":"from a homeless agent"}"#),
    ] {
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.current_dir(&clean);
        for k in ["RKB_HOME", "RKB_TEST_HOME", "HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_CONFIG_HOME"] {
            c.env_remove(k);
        }
        let mut child = c.args(&args).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
        std::io::Write::write_all(&mut child.stdin.take().unwrap(), input.as_bytes()).unwrap();
        let o = child.wait_with_output().unwrap();
        assert_eq!(o.status.code(), Some(1), "{args:?} stops: {}", stdout(&o));
    }
    assert_eq!(dir_tree(&clean), before, "the throwaway folder is unchanged");

    let o = bare(&["--format", "json", "list"], &[("RKB_HOME", env.kb().to_str().unwrap())]);
    assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert!(v.get("error").is_none(), "RKB_HOME alone runs: {v}");
    // With RKB_HOME alone, state goes inside the knowledge base's .git, never into the working directory.
    let o = bare(&["--format", "json", "note", "kept inside git"], &[("RKB_HOME", env.kb().to_str().unwrap())]);
    assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));
    assert_eq!(dir_tree(&clean), before, "the working directory is unchanged");
    assert!(env.kb().join(".git/rkb-home/.local/state/rkb").is_dir(), "state lives in .git/rkb-home");
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "", "the knowledge base stays clean");
}

#[test]
fn doctor_reports_server_state() {
    let env = search_kb();
    let doctor = |lock: &str| {
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        std::fs::write(env.kb().join(".git/rkb.lock"), lock).unwrap();
        c.args(["doctor", "--format", "json"]).env("RKB_OFFLINE", "1");
        let o = c.output().unwrap();
        serde_json::from_slice::<serde_json::Value>(&o.stdout).unwrap()
    };
    let find = |v: &serde_json::Value, name: &str| {
        v["checks"].as_array().unwrap().iter().find(|c| c["check"] == name).cloned().unwrap_or_else(|| panic!("{name} missing: {v}"))
    };

    let v = doctor("other 4242 0");
    assert!(find(&v, "offline")["detail"].as_str().unwrap().contains("on"), "{v}");
    let files = find(&v, "model files");
    assert!(files["detail"].as_str().unwrap().starts_with("0 present, 3 missing"), "{v}");
    assert!(files["fix"].as_str().unwrap().contains("rkb models path"), "{v}");
    let lock = find(&v, "write lock")["detail"].as_str().unwrap().to_string();
    assert!(lock.contains("pid 4242") && lock.contains("other"), "names the pid and the host: {lock}");

    let v = doctor("4242 0");
    let lock = find(&v, "write lock")["detail"].as_str().unwrap().to_string();
    assert!(lock.contains("pid 4242") && lock.contains("old lock"), "an old lock file still names the holder: {lock}");

    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.args(["doctor", "--format", "human"]).env("RKB_OFFLINE", "1");
    let human = stdout(&c.output().unwrap());
    assert!(human.contains("offline") && human.contains("pid 4242"), "{human}");
}

const MODEL_SHAS: [&str; 3] = [
    "69f22ccd1ee0971b6173678c4fcadd29a49a3f5b6b8a6927002418abe4bd5b9c",
    "6c8aaa9a542084f2457eab775d4eeb51f92a70c0fd9de28d5edb0ddec3c08d30",
    "dea4708ed1a6df7306b483037750464a96e77ccb0c7e88f4f61a0116944a71dd",
];

/// A local HTTP server that only counts connections, so a test proves an offline run connects nowhere.
fn counting_server() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::Write;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c = count.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    });
    (url, count)
}

#[test]
fn offline_never_downloads_the_model() {
    let env = search_kb();
    set_chain(&env, "chain = [\"bm25-bert\", \"bm25\"]");
    let (url, hits) = counting_server();
    let model_dir = env.dir.path().join("data/rkb/models/modernbert-embed-base");
    let plain = env.json(&["search", "git rebase"], "").0;
    let run = |vars: &[(&str, &str)], args: &[&str]| {
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.args(args).env("RKB_MODELS_URL", &url);
        for (k, v) in vars {
            c.env(k, v);
        }
        c.stdin(std::process::Stdio::null()).output().unwrap()
    };

    let offline_search = |var: &str| {
        let o = run(&[(var, "1")], &["search", "git rebase", "--format", "json"]);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(o.status.code(), Some(0), "{v}");
        let by = v["ranked_by"].as_str().unwrap();
        assert!(by.starts_with("bm25 (bm25-bert: offline mode is on"), "{v}");
        assert!(by.contains(&model_dir.display().to_string()), "names the folder: {v}");
        for sha in MODEL_SHAS {
            assert!(by.contains(sha), "names each sha: {v}");
        }
        assert!(by.contains("copy them from a machine that has them") && by.contains("rkb models path"), "{v}");
        assert_eq!(ids(&v), ids(&plain), "BM25 results stay: {v}");
    };
    offline_search("RKB_OFFLINE");
    offline_search("HF_HUB_OFFLINE");

    let o = run(&[("RKB_OFFLINE", "1")], &["search", "git rebase", "--format", "json", "--no-model"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!((v["ranked_by"].as_str(), o.status.code()), (Some("bm25"), Some(0)), "{v}");

    for args in [["models", "warm"], ["models", "fetch"]] {
        let o = run(&[("RKB_OFFLINE", "1")], &["--format", "json", args[0], args[1]]);
        assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        let msg = v["error"]["message"].as_str().unwrap();
        assert!(msg.starts_with("offline mode is on"), "{v}");
        assert!(msg.contains(&model_dir.display().to_string()) && msg.contains("copy them from a machine that has them"), "{v}");
    }

    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0, "offline made a network request");
    assert!(!model_dir.exists(), "offline created the model folder");
}

#[test]
fn models_path_prints_the_folder_and_the_files() {
    let env = Env::new();
    let dir = env.dir.path().join("data/rkb/models/modernbert-embed-base");
    let o = env.rkb(&["models", "path", "--format", "human"]);
    assert!(o.status.success(), "{}", stdout(&o));
    assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), dir.display().to_string());

    let (v, code) = env.json(&["models", "path"], "");
    assert_eq!(code, Some(0));
    assert_eq!(v["path"].as_str().unwrap(), dir.display().to_string());
    let files = v["files"].as_array().unwrap();
    for (i, name) in ["model.safetensors", "tokenizer.json", "config.json"].into_iter().enumerate() {
        assert_eq!((files[i]["name"].as_str(), files[i]["sha256"].as_str()), (Some(name), Some(MODEL_SHAS[i])), "{v}");
        assert_eq!(files[i]["present"], serde_json::json!(false), "{v}");
    }
    assert_eq!(env.rkb(&["models", "path", "--toon"]).status.code(), Some(0));
}

fn graph_kb() -> Env {
    let env = Env::new();
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    copy_dir(&fixtures.join("search/kb"), &env.kb());
    copy_dir(&fixtures.join("graph/kb"), &env.kb());
    for args in [
        vec!["init", "-q"],
        vec!["add", "-A"],
        vec!["-c", "user.name=T", "-c", "user.email=t@example.org", "commit", "-q", "-m", "fixture"],
    ] {
        assert!(env.git(&args).status.success());
    }
    env
}

#[test]
fn dupes_lists_only_the_duplicate_pair() {
    let env = graph_kb();
    let (v, code) = env.json(&["dupes"], "");
    assert_eq!(code, Some(0));
    let pairs = v["pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), 1, "{v}");
    let ids = [pairs[0]["a"]["id"].as_str().unwrap(), pairs[0]["b"]["id"].as_str().unwrap()];
    assert!(ids.contains(&"1a00000001") && ids.contains(&"1b00000001"), "{v}");
    assert!(v["help"].to_string().contains("rkb supersede"), "{v}");

    let (v, _) = env.json(&["dupes", "--min", "0.9"], "");
    assert_eq!(v["pairs"], serde_json::json!([]));
    let human = stdout(&env.rkb(&["dupes", "--min", "0.9", "--format", "human"]));
    assert!(human.contains("No likely duplicates at 0.90"), "{human}");
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "");
}

/// The graph fixture with Jev in the chain, whose reply rates every pair `verdict`.
fn dupes_jev(verdict: &'static str) -> (Env, String, JevLog) {
    let env = graph_kb();
    set_chain(&env, "chain = [\"jev\", \"bm25\"]");
    jev_config(&env, "[jev]\napi_key = \"k\"\n", 0o600);
    let (url, log) = fake_jev(move |body| {
        let answers: serde_json::Map<String, serde_json::Value> = body["questions"]
            .as_object()
            .unwrap()
            .keys()
            .map(|k| {
                let other = if verdict == "different" { "same" } else { "different" };
                let a = serde_json::json!({ "type": "choice", "choice": verdict, "probabilities": { verdict: 0.8, other: 0.1 } });
                (k.clone(), a)
            })
            .collect();
        (200, serde_json::json!({ "answers": answers }).to_string())
    });
    (env, url, log)
}

fn dupes_run(env: &Env, url: &str, args: &[&str]) -> serde_json::Value {
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_JEV_URL", url);
    let mut all = args.to_vec();
    all.extend(["--format", "json"]);
    let o = rkb_with(c, &all, "");
    serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)))
}

#[test]
fn dupes_check_asks_once_and_again_after_an_edit() {
    let (env, url, log) = dupes_jev("same");
    let v = dupes_run(&env, &url, &["dupes", "--check"]);
    assert_eq!(v["pairs"][0]["jev"]["verdict"], "same", "{v}");
    assert_eq!(v["pairs"][0]["jev"]["p"], 0.8, "{v}");
    let body = log.lock().unwrap()[0].1.clone();
    assert_eq!(body["questions"].as_object().unwrap().len(), 1, "{body}");
    assert!(body["state"].as_str().unwrap().contains("A: "), "{body}");
    dupes_run(&env, &url, &["dupes", "--check"]);
    assert_eq!(log.lock().unwrap().len(), 1, "asked once");
    let p = env.kb().join("general/build/new-library-not-found-until-cmake-cache-is-cleared.md");
    std::fs::write(&p, std::fs::read_to_string(&p).unwrap() + "\nAlso delete CMakeCache.txt by hand.\n").unwrap();
    dupes_run(&env, &url, &["dupes", "--check"]);
    assert_eq!(log.lock().unwrap().len(), 2, "asked again after an edit");
}

#[test]
fn dupes_check_without_jev_changes_nothing() {
    let env = graph_kb();
    let v = dupes_run(&env, "http://127.0.0.1:1/", &["dupes", "--check"]);
    assert!(v["check"].as_str().unwrap().contains("not in the rerank chain"), "{v}");
    assert!(!env.dir.path().join("state/rkb/dupes.json").exists());
}

#[test]
fn dupes_check_keeps_internal_lessons_from_jev() {
    let (env, url, log) = dupes_jev("same");
    let toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    env.write(
        "kb.toml",
        &toml.replace(
            "[sinks.jev]\nallow = { sensitivity = [\"public\", \"internal\"] }",
            "[sinks.jev]\nallow = { sensitivity = [\"public\"] }",
        ),
    );
    env.write("general/README.md", "---\nlabels:\n  sensitivity: public\n---\n\n# General\n");
    env.write("general/build/README.md", "---\nlabels:\n  sensitivity: internal\n---\n\n# Build\n");
    let v = dupes_run(&env, &url, &["dupes", "--check"]);
    assert_eq!(v["pairs"][0]["jev"]["unchecked"], "jev: label internal", "{v}");
    assert!(log.lock().unwrap().is_empty(), "nothing was sent");
}

#[test]
fn dupes_hide_pairs_rated_different_unless_all() {
    let (env, url, _) = dupes_jev("different");
    let v = dupes_run(&env, &url, &["dupes", "--check"]);
    assert_eq!((v["pairs"].as_array().unwrap().len(), &v["hidden"]), (0, &serde_json::json!(1)), "{v}");
    let human = stdout(&env.rkb(&["dupes", "--format", "human"]));
    assert!(human.contains("1 pairs hidden") && human.contains("--all"), "{human}");
    let v = dupes_run(&env, &url, &["dupes", "--all"]);
    assert_eq!(v["pairs"][0]["jev"], serde_json::json!({ "verdict": "different", "p": 0.8 }), "{v}");
    let human = stdout(&env.rkb(&["dupes", "--all", "--format", "human"]));
    assert!(human.contains("different 0.80"), "{human}");
}

#[test]
fn add_names_a_similar_lesson_in_another_folder() {
    let env = search_kb();
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/graph/kb/general/build/new-library-not-found-until-cmake-cache-is-cleared.md");
    let text: String = std::fs::read_to_string(src)
        .unwrap()
        .lines()
        .filter(|l| !["schema:", "id:", "status:"].iter().any(|k| l.starts_with(k)))
        .map(|l| format!("{l}\n"))
        .collect();
    let (v, code) = env.json(&["add", "--topic", "build"], &text);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["path"], "general/build/a-new-library-is-not-found-until-the-cmake-cache-is-cleared.md", "{v}");
    assert!(v["notes"].to_string().contains("1a00000001"), "{v}");
}

fn near_duplicates(v: &serde_json::Value) -> Vec<String> {
    v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["rule"] == "content/near-duplicate")
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn lint_warns_about_near_duplicates() {
    let env = graph_kb();
    let (v, code) = env.json(&["lint"], "");
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(near_duplicates(&v), ["general/build/new-library-not-found-until-cmake-cache-is-cleared.md"], "{v}");

    let env = search_kb();
    let (v, _) = env.json(&["lint"], "");
    assert!(near_duplicates(&v).is_empty(), "{v}");
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/graph/kb/general/build");
    copy_dir(&src, &env.kb().join("general/build"));
    assert!(env.git(&["add", "-A"]).status.success());
    let (v, code) = env.json(&["lint", "--staged"], "");
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(near_duplicates(&v).len(), 1, "{v}");
    let hook = env.kb().join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\nexec rkb lint --staged\n").unwrap();
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let o = env.git(&["-c", "user.name=T", "-c", "user.email=t@example.org", "commit", "-q", "-m", "dup"]);
    assert!(o.status.success(), "the hook allows a near duplicate: {}", stdout(&o));
}

fn related_ids(env: &Env, id: &str) -> Vec<(String, Vec<String>)> {
    let (v, code) = env.json(&["related", id], "");
    assert_eq!(code, Some(0), "{v}");
    v["related"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let kinds = r["edges"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap().to_string()).collect();
            (r["id"].as_str().unwrap().to_string(), kinds)
        })
        .collect()
}

#[test]
fn related_by_link_and_supersede() {
    let env = graph_kb();
    let hub = related_ids(&env, "1b00000002");
    assert_eq!(hub.len(), 2, "{hub:?}");
    assert!(hub.iter().all(|(_, k)| k[0] == "link"), "{hub:?}");
    assert!(related_ids(&env, "1a00000013").iter().any(|(id, k)| id == "1b00000002" && k.contains(&"link".to_string())));
    assert!(related_ids(&env, "1a00000007").iter().any(|(id, k)| id == "1b00000003" && k == &["supersedes"]));
    let (v, code) = env.json(&["related", "0000000000"], "");
    assert_eq!((v["error"]["code"].as_str(), code), (Some("not_found"), Some(1)), "{v}");

    let (v, _) = env.json(&["show", "1b00000002"], "");
    let shown: Vec<&str> = v["related"].as_array().unwrap().iter().map(|r| r["edges"][0]["kind"].as_str().unwrap()).collect();
    assert_eq!(shown, ["link", "link"], "{v}");
    let human = stdout(&env.rkb(&["show", "1b00000002", "--format", "human"]));
    assert!(human.contains("related") && human.contains("1a00000013"), "{human}");
}

#[test]
fn graph_views() {
    let env = graph_kb();
    let (v, _) = env.json(&["graph"], "");
    assert_eq!(
        (v["edges"]["link"].as_u64(), v["edges"]["supersedes"].as_u64(), v["likely_duplicates"].as_u64()),
        (Some(2), Some(1), Some(1)),
        "{v}"
    );
    let (v, _) = env.json(&["graph", "--orphans"], "");
    let orphans: Vec<&str> = v["orphans"].as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap()).collect();
    assert!(orphans.contains(&"1b00000004") && !orphans.contains(&"1b00000002"), "{orphans:?}");
    let (v, _) = env.json(&["graph", "--clusters"], "");
    assert!(v["clusters"].to_string().contains("1b00000001"), "{v}");

    let edges = |text: &str, marks: &[&str]| text.lines().filter(|l| marks.iter().any(|m| l.contains(m))).count();
    let mermaid = stdout(&env.rkb(&["graph", "--mermaid"]));
    assert!(mermaid.starts_with("graph"), "{mermaid}");
    let (counts, _) = env.json(&["graph"], "");
    let total = ["link", "supersedes", "similar"].iter().map(|k| counts["edges"][k].as_u64().unwrap() as usize).sum::<usize>();
    assert_eq!(edges(&mermaid, &[" --> ", " -.->", " ---|"]), total, "{mermaid}");
    let dot = stdout(&env.rkb(&["graph", "--dot"]));
    assert!(dot.starts_with("digraph rkb {") && dot.trim_end().ends_with('}'), "{dot}");
    assert_eq!(edges(&dot, &[" -> "]), total, "{dot}");
    assert_eq!(env.rkb(&["graph", "--dot", "--mermaid"]).status.code(), Some(2));
}

#[test]
fn note_and_inbox() {
    let env = search_kb();
    let (v, code) = env.json(&["note", "tuolumne needs module load cmake"], "");
    assert_eq!(code, Some(0), "{v}");
    let first = v["id"].as_str().unwrap().to_string();
    let (v, _) = env.json(&["note"], "the linker wants -lz after -lhdf5\n");
    let second = v["id"].as_str().unwrap().to_string();
    let (v, code) = env.json(&["note"], "  ");
    assert_eq!((v["error"]["code"].as_str(), code), (Some("usage"), Some(2)), "{v}");

    let (v, _) = env.json(&["inbox"], "");
    let ids: Vec<&str> = v["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), 2, "{v}");
    assert!(ids.contains(&first.as_str()) && ids.contains(&second.as_str()));
    let (v, _) = env.json(&["inbox", "show", &second], "");
    assert_eq!(v["body"], "the linker wants -lz after -lhdf5");

    let (v, _) = env.json(&["search", "tuolumne cmake"], "");
    assert!(!v.to_string().contains("module load"), "inbox items are not searched: {v}");
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "", "the knowledge base is unchanged");

    let (v, code) = env.json(&["inbox", "done", &first, "0000000000"], "");
    assert_eq!(code, Some(1), "an unknown id is reported: {v}");
    assert_eq!(v["items"][0]["removed"], true);
    let (v, _) = env.json(&["inbox"], "");
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    assert_eq!(v["items"][0]["id"], second.as_str());

    let old = env.dir.path().join("state/rkb/inbox/0123456789.md");
    std::fs::write(&old, "---\nkind: note\ntime: 1000\n---\n\nold\n").unwrap();
    let (v, _) = env.json(&["inbox"], "");
    assert_eq!((v["expired"].as_u64(), v["items"].as_array().unwrap().len()), (Some(0), 2), "an old item is kept under the cap: {v}");
    env.write_abs(&env.dir.path().join("config/rkb/config.toml"), "[inbox]\nmax_items = 1\n");
    let (v, _) = env.json(&["inbox"], "");
    assert_eq!((v["expired"].as_u64(), v["items"].as_array().unwrap().len()), (Some(1), 1), "{v}");
    assert!(!old.exists(), "the oldest item is dropped over the cap");
}

#[test]
fn argument_errors_are_records_on_stdout() {
    let env = search_kb();
    let o = env.rkb(&["list", "--stat", "x", "--toon"]);
    assert_eq!(o.status.code(), Some(2));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("code: usage") && out.contains("--stat") && out.contains("valid for `rkb list`"), "{out}");
    let (v, code) = env.json(&["search", "x", "--limt", "3"], "");
    assert_eq!((v["error"]["code"].as_str(), code), (Some("usage"), Some(2)), "{v}");
    assert!(v["error"]["fix"].as_str().unwrap().contains("--limit"), "{v}");
    let (v, code) = env.json(&["lsit"], "");
    assert_eq!(code, Some(2));
    assert!(v["error"]["fix"].as_str().unwrap().contains("list"), "{v}");

    let help = env.rkb(&["show", "--help"]);
    assert_eq!(help.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&help.stdout).contains("rkb show 7f3a9c2b41"));
    for flag in ["-v", "-V", "--version"] {
        let o = env.rkb(&[flag]);
        assert_eq!((o.status.code(), String::from_utf8_lossy(&o.stdout).trim()), (Some(0), concat!("rkb ", env!("CARGO_PKG_VERSION"))));
    }
}

#[test]
fn home_view_identifies_the_binary() {
    let env = search_kb();
    let (v, _) = env.json(&[], "");
    let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    assert_eq!(keys[..3], ["bin", "description", "kb"], "{v}");
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_rkb"));
    assert_eq!(
        v["bin"].as_str().unwrap(),
        bin.canonicalize().unwrap().display().to_string().replace(&env.dir.path().display().to_string(), "~"),
        "{v}"
    );
    assert_eq!(v["kb"], "~/kb", "the knowledge base under HOME is shown with ~: {v}");
    assert!(v["description"].as_str().unwrap().contains("lessons learned"));
}

fn tool(env: &Env, name: &str, args: &serde_json::Value) -> (String, Option<i32>) {
    let o = env.rkb_in(&["tool", name], &args.to_string());
    (String::from_utf8_lossy(&o.stdout).into_owned(), o.status.code())
}

#[test]
fn agent_tools_match_the_commands() {
    let env = search_kb();
    let (v, _) = env.json(&["tools"], "");
    let names: Vec<&str> = v["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["rkb_search", "rkb_show", "rkb_add", "rkb_note", "rkb_used", "rkb_flag", "rkb_edit"]);
    assert!(
        v["tools"][0]["description"].as_str().unwrap().contains("before a web search")
            || v["tools"][0]["description"].as_str().unwrap().contains("BEFORE a web search")
    );

    let (out, code) = tool(&env, "rkb_search", &serde_json::json!({ "query": "undefined reference to vtable" }));
    assert_eq!(code, Some(0));
    assert_eq!(out, stdout(&env.rkb(&["search", "undefined reference to vtable", "--toon"])));
    let (out, code) = tool(&env, "rkb_show", &serde_json::json!({}));
    assert_eq!(code, Some(2));
    assert!(out.contains("code: usage") && out.contains("`id` is required"), "{out}");
    let (out, code) = tool(&env, "rkb_show", &serde_json::json!({ "id": "1a00000012" }));
    assert_eq!((code, out), (Some(0), stdout(&env.rkb(&["show", "1a00000012", "--toon"]))));
    let (out, _) = tool(&env, "rkb_note", &serde_json::json!({ "text": "tuolumne wants module load cmake" }));
    assert!(out.starts_with("id: "), "{out}");

    let (out, code) = tool(&env, "rkb_used", &serde_json::json!({ "id": "1a00000012", "result": "worked" }));
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("status: recorded"), "{out}");
    assert_eq!(usage_records(&env).iter().filter(|r| r["id"] == "1a00000012" && r["event"] == "worked").count(), 1);
    let (out, code) = tool(&env, "rkb_used", &serde_json::json!({ "id": "1a00000012", "result": "failed" }));
    assert_eq!(code, Some(2), "a failure needs a reason: {out}");
    let (out, code) = tool(&env, "rkb_used", &serde_json::json!({ "id": "1a00000012", "result": "irrelevant" }));
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(usage_records(&env).iter().filter(|r| r["id"] == "1a00000012" && r["event"] == "irrelevant").count(), 1);
    assert!(!std::fs::read_to_string(env.kb().join(show(&env, "1a00000012")["path"].as_str().unwrap())).unwrap().contains("status: stale"));
    let (out, code) = tool(&env, "rkb_used", &serde_json::json!({ "id": "1a00000012", "result": "maybe" }));
    assert_eq!(code, Some(2), "{out}");

    let (v, _) = env.json(&["show", "1a00000012"], "");
    let (hash, path) = (v["hash"].as_str().unwrap().to_string(), v["path"].as_str().unwrap().to_string());
    let text = std::fs::read_to_string(env.kb().join(&path)).unwrap();
    let edited = format!("{}\nAlso seen with clang.\n", text.trim_end());
    let (out, code) = tool(&env, "rkb_edit", &serde_json::json!({ "id": "1a00000012", "base": hash, "text": edited }));
    assert_eq!(code, Some(0), "{out}");
    assert!(
        out.contains("status: written") && std::fs::read_to_string(env.kb().join(&path)).unwrap().contains("Also seen with clang."),
        "{out}"
    );
    let (out, code) = tool(&env, "rkb_edit", &serde_json::json!({ "id": "1a00000012", "base": hash, "text": text }));
    assert_ne!(code, Some(0), "a stale base is refused: {out}");
    let (out, code) = tool(&env, "rkb_flag", &serde_json::json!({ "id": "1a00000012", "reason": "breaks with lld 18" }));
    assert_eq!(code, Some(0), "{out}");
    assert!(std::fs::read_to_string(env.kb().join(&path)).unwrap().contains("stale_reason: breaks with lld 18"));
}

#[test]
fn rkb_add_builds_the_lesson() {
    let env = search_kb();
    let pitfall = serde_json::json!({
        "type": "pitfall", "title": "Ninja ignores CFLAGS changes until a reconfigure", "topic": "cmake",
        "symptom": "A changed CFLAGS has no effect.", "cause": "CMake caches the flags at configure time.",
        "fix": "Run `cmake --fresh -B build`.", "evidence": "The next build used the new flags.", "tags": ["ninja"],
    });
    let (out, code) = tool(&env, "rkb_add", &pitfall);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("status: written") && out.contains("general/cmake/"), "{out}");
    let id = out.lines().find_map(|l| l.strip_prefix("id: ")).unwrap().trim_matches('"').to_string();
    let (v, _) = env.json(&["show", &id], "");
    let body = v["body"].as_str().unwrap_or_else(|| panic!("show {id}: {v}\nadd said: {out}"));
    let heads: Vec<&str> = body.lines().filter_map(|l| l.strip_prefix("## ")).collect();
    assert_eq!(heads, ["Symptom", "Cause", "Fix", "Evidence"]);
    assert_eq!(v["frontmatter"]["tags"][0], "ninja");

    let recipe = serde_json::json!({ "type": "recipe", "title": "t", "topic": "git", "when_to_use": "w", "evidence": "e" });
    let before = stdout(&env.git(&["log", "--oneline"]));
    let (out, code) = tool(&env, "rkb_add", &recipe);
    assert_eq!(code, Some(2));
    assert!(out.contains("`steps`"), "{out}");
    assert_eq!(stdout(&env.git(&["log", "--oneline"])), before, "nothing was written");

    let mut near = pitfall.clone();
    near["topic"] = "cmak".into();
    near["title"] = "Another lesson for a close topic".into();
    let (out, code) = tool(&env, "rkb_add", &near);
    assert_eq!(code, Some(0), "no question for a close topic: {out}");
    assert!(out.contains("status: written") && out.contains("close to general/cmake"), "{out}");
}

#[test]
fn mcp_server_offers_the_tools() {
    let env = search_kb();
    let msgs = [
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } } }),
        serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        serde_json::json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "rkb_search", "arguments": { "query": "undefined reference to vtable" } } }),
        serde_json::json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "rkb_show", "arguments": {} } }),
        serde_json::json!({ "jsonrpc": "2.0", "id": 5, "method": "resources/list" }),
        serde_json::json!({ "jsonrpc": "2.0", "id": 6, "method": "ping" }),
    ];
    let input: String = msgs.iter().map(|m| format!("{m}\n")).collect();
    let o = env.rkb_in(&["mcp"], &input);
    let replies: Vec<serde_json::Value> = String::from_utf8_lossy(&o.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies.len(), 6, "one reply per request, none for the notification: {replies:?}");
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    assert!(replies[0]["result"]["capabilities"]["tools"].is_object());
    assert_eq!(replies[1]["result"]["tools"].as_array().unwrap().len(), 7);
    let text = replies[2]["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(format!("{text}\n"), stdout(&env.rkb(&["search", "undefined reference to vtable", "--toon"])));
    assert_eq!(replies[2]["result"]["isError"], false);
    assert_eq!(replies[3]["result"]["isError"], true);
    assert_eq!((replies[4]["id"].as_i64(), replies[4]["error"]["code"].as_i64()), (Some(5), Some(-32601)));
    assert_eq!(replies[5]["result"], serde_json::json!({}));
}

/// The replies of an `rkb mcp` server started with `args`, one per request line.
fn mcp_replies(env: &Env, args: &[&str], requests: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let input: String = requests.iter().map(|r| format!("{r}\n")).collect();
    let o = env.rkb_in(&[&["mcp"], args].concat(), &input);
    String::from_utf8_lossy(&o.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn listed(reply: &serde_json::Value) -> Vec<String> {
    reply["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
}

#[test]
fn mcp_tool_sets() {
    let env = search_kb();
    let list = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} });
    assert_eq!(listed(&mcp_replies(&env, &["--tools", "read"], std::slice::from_ref(&list))[0]), ["rkb_search", "rkb_show"]);
    assert_eq!(listed(&mcp_replies(&env, &["--tools", "propose"], std::slice::from_ref(&list))[0]), ["rkb_search", "rkb_show", "rkb_note"]);
    assert_eq!(listed(&mcp_replies(&env, &[], std::slice::from_ref(&list))[0]).len(), 7, "the default serves every tool");

    // A valid rkb_add outside the set is refused and writes nothing.
    let add = serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "rkb_add", "arguments": {
        "type": "pitfall", "title": "Ninja ignores CFLAGS changes until a reconfigure", "topic": "cmake",
        "symptom": "A changed CFLAGS has no effect.", "cause": "CMake caches the flags at configure time.",
        "fix": "Run `cmake --fresh -B build`.", "evidence": "The next build used the new flags.",
    } } });
    let (commits, status) = (commit_count(&env), stdout(&env.git(&["status", "--porcelain"])));
    let replies = mcp_replies(&env, &["--tools", "propose"], std::slice::from_ref(&add));
    assert_eq!(replies[0]["result"]["isError"], true, "{}", replies[0]);
    let text = replies[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("rkb_add") && text.contains("rkb mcp --tools propose"), "the error names the set: {text}");
    assert_eq!((commit_count(&env), stdout(&env.git(&["status", "--porcelain"]))), (commits, status), "nothing written");
    assert!(inbox_items(&env).is_empty(), "no inbox item");
    // An unknown tool keeps its own error, and the full set still writes.
    let unknown =
        serde_json::json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "rkb_nope", "arguments": {} } });
    let replies = mcp_replies(&env, &["--tools", "propose"], &[unknown]);
    assert!(replies[0]["result"]["content"][0]["text"].as_str().unwrap().contains("unknown tool"), "{}", replies[0]);
    let replies = mcp_replies(&env, &[], &[add]);
    assert_eq!(replies[0]["result"]["isError"], false, "{}", replies[0]);
    assert_eq!(commit_count(&env), commits + 1);
}

#[test]
fn mcp_tool_list_fits() {
    let env = search_kb();
    let list = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} });
    let count = mcp_replies(&env, &[], &[list])[0]["result"].to_string().chars().count();
    println!("tools/list result: {count} characters");
    assert!(count <= 6000, "tools/list result is {count} characters");
}

#[test]
fn install_claude_needs_the_claude_command() {
    let env = Env::new();
    std::fs::create_dir_all(env.dir.path().join(".claude")).unwrap();
    let o = env.cmd(env!("CARGO_BIN_EXE_rkb")).args(["install", "claude", "--toon"]).env("RKB_CLAUDE", "/no/such/claude").output().unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(out.contains("claude") && out.contains("PATH"), "{out}");
    assert!(!env.dir.path().join("data/rkb/claude-plugin").exists() && !env.dir.path().join("state/rkb/requests").exists());

    let (v, code) = env.json(&["install", "claude"], "");
    assert_eq!(code, Some(3));
    let q = v["question"].as_str().unwrap();
    assert!(q.contains("claude plugin rkb@rkb") && q.contains("~/.claude/settings.json") && q.contains("trust.toml"), "{q}");
    assert_eq!(q.matches("settings.json").count(), 1, "each file once: {q}");
}

#[test]
fn install_from_an_agent_needs_the_user() {
    let env = Env::new();
    let home = env.dir.path();
    for h in [".claude", ".pi"] {
        std::fs::create_dir_all(home.join(h)).unwrap();
    }
    std::fs::write(home.join(".claude/settings.json"), "{\"theme\": \"dark\", \"permissions\": {\"ask\": [\"Bash(rm:*)\"]}}").unwrap();

    let (v, _) = env.json(&["install", "--list"], "");
    let rows = v["harnesses"].as_array().unwrap();
    assert_eq!(
        rows.iter().map(|r| (r["harness"].as_str().unwrap(), r["detected"].as_bool().unwrap())).collect::<Vec<_>>(),
        [("claude", true), ("pi", true), ("omp", false), ("hermes", false)]
    );

    let (v, code) = env.json(&["install"], "");
    assert_eq!((v["status"].as_str(), code), (Some("needs_user"), Some(3)));
    let q = v["question"].as_str().unwrap();
    assert!(q.contains("claude plugin rkb@rkb") && q.contains("~/.claude/settings.json") && q.contains("trust.toml"), "{q}");
    assert!(!home.join("data/rkb/claude-plugin").exists(), "nothing written before confirm");

    let (v, _) = env.json(&["confirm", v["request"].as_str().unwrap(), "--choice", "install"], "");
    assert_eq!(v["error"]["code"], "needs_terminal", "an agent cannot confirm its own install");

    let (v, _) = env.json(&["install"], "");
    let tty = env.dir.path().join("tty");
    std::fs::write(&tty, "yes\n").unwrap();
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_TTY", &tty);
    let o = rkb_with(c, &["confirm", v["request"].as_str().unwrap(), "--choice", "install", "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["status"], "done", "{v}");
    let settings: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(home.join(".claude/settings.json")).unwrap()).unwrap();
    assert_eq!(settings["permissions"]["ask"], serde_json::json!(["Bash(rm:*)", "Bash(rkb confirm:*)"]));
    assert_eq!(settings["theme"], "dark");
    assert!(std::fs::read_to_string(home.join("config/rkb/trust.toml")).unwrap().contains("gated = true"));
    assert_eq!(settings["permissions"]["allow"].as_array().unwrap().len(), 4, "the plugin tools are allowed");
    let message = v["message"].as_str().unwrap();
    assert!(message.contains("Restart Claude Code"), "{message}");
    let (v, _) = env.json(&["install", "--list"], "");
    assert_eq!((v["harnesses"][0]["current"].as_bool(), v["harnesses"][1]["current"].as_bool()), (Some(true), Some(true)), "{v}");

    let (v, code) = env.json(&["install", "vscode"], "");
    assert_eq!((v["error"]["code"].as_str(), code), (Some("usage"), Some(2)));
    assert!(v["error"]["fix"].as_str().unwrap().contains("claude, pi, omp, hermes"));
}

/// Runs `rkb install hermes` (or `--uninstall`) in a throwaway `HERMES_HOME` and answers the confirm
/// request the way `rkb install claude` is answered. The real `~/.hermes` is never touched.
fn hermes_install(env: &Env, hermes: &Path, uninstall: bool) -> serde_json::Value {
    let mut args = vec!["install", "hermes", "--format", "json"];
    if uninstall {
        args.push("--uninstall");
    }
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("HERMES_HOME", hermes);
    let o = rkb_with(c, &args, "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("not json: {}", stdout(&o)));
    assert_eq!((v["status"].as_str(), o.status.code()), (Some("needs_user"), Some(3)), "{v}");

    let tty = env.dir.path().join("tty");
    std::fs::write(&tty, "yes\n").unwrap();
    let choice = if uninstall { "uninstall" } else { "install" };
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("HERMES_HOME", hermes).env("RKB_TTY", &tty);
    let o = rkb_with(c, &["confirm", v["request"].as_str().unwrap(), "--choice", choice, "--format", "json"], "");
    serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("not json: {}", stdout(&o)))
}

fn hermes_config_yaml(hermes: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(hermes.join("config.yaml")).unwrap();
    serde_norway::from_str(&text).unwrap()
}

#[test]
fn install_hermes_into_an_empty_home() {
    let env = Env::new();
    let hermes = env.dir.path().join("hermes-home");
    let v = hermes_install(&env, &hermes, false);
    assert_eq!(v["status"], "done", "{v}");

    let doc = hermes_config_yaml(&hermes);
    let entry = &doc["mcp_servers"]["rkb"];
    let command = entry["command"].as_str().unwrap();
    assert!(Path::new(command).is_absolute() && command.ends_with("rkb"), "the running rkb: {command}");
    assert_eq!(entry["args"], serde_json::json!(["mcp", "--tools", "propose"]));
    assert_eq!(entry["env"]["RKB_AGENT"], "hermes");
    let config = std::fs::read_to_string(hermes.join("config.yaml")).unwrap();
    assert!(!config.to_lowercase().contains("token"), "no secret is written: {config}");

    let skill = std::fs::read_to_string(hermes.join("skills/rkb/SKILL.md")).unwrap();
    assert!(skill.contains("rkb owns checked fixes that apply across projects"), "{skill}");
    assert!(skill.len() < 2000, "the Hermes skill stays short: {} characters", skill.len());
    assert!(!env.dir.path().join(".hermes").exists(), "HERMES_HOME wins over ~/.hermes");
}

#[test]
fn install_hermes_uses_home_by_default() {
    let env = Env::new();
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_AUTO_CONFIRM", "install");
    let o = rkb_with(c, &["install", "hermes", "--format", "json"], "");
    assert!(o.status.success(), "{}", stdout(&o));
    assert!(env.dir.path().join(".hermes/config.yaml").exists(), "Hermes' own default is ~/.hermes");
    assert!(env.dir.path().join(".hermes/skills/rkb/SKILL.md").exists());
}

#[test]
fn install_hermes_passes_the_rkb_variables() {
    // Hermes gives a stdio server only PATH, HOME, XDG_* and a few more, so the entry carries them.
    let env = Env::new();
    let kb = env.dir.path().join("server-kb");
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_AUTO_CONFIRM", "install").env("RKB_HOME", &kb).env("RKB_OFFLINE", "1").env("RKB_NTFY_TOKEN", "sekrit");
    let o = rkb_with(c, &["install", "hermes", "--format", "json"], "");
    assert!(o.status.success(), "{}", stdout(&o));
    let hermes = env.dir.path().join(".hermes");
    let entry = hermes_config_yaml(&hermes)["mcp_servers"]["rkb"]["env"].clone();
    assert_eq!(entry["RKB_HOME"].as_str(), Some(kb.to_str().unwrap()), "{entry}");
    assert_eq!(entry["RKB_OFFLINE"], "1", "{entry}");
    assert_eq!(entry["RKB_NTFY_TOKEN"], "${RKB_NTFY_TOKEN}", "only a reference: {entry}");
    let config = std::fs::read_to_string(hermes.join("config.yaml")).unwrap();
    assert!(!config.contains("sekrit"), "the token value is never written: {config}");
}

#[test]
fn install_hermes_keeps_comments_and_other_servers() {
    let env = Env::new();
    let hermes = env.dir.path().join("hermes-home");
    std::fs::create_dir_all(&hermes).unwrap();
    let original = "# my Hermes config\nmodel: claude\n\nmcp_servers:\n  fence:            # keep the fence\n    command: \"fence-cli\"\n    args: [\"serve\"]\n\nlogging:\n  level: info\n";
    std::fs::write(hermes.join("config.yaml"), original).unwrap();

    let v = hermes_install(&env, &hermes, false);
    assert_eq!(v["status"], "done", "{v}");
    let after = std::fs::read_to_string(hermes.join("config.yaml")).unwrap();
    assert!(after.contains("# my Hermes config") && after.contains("# keep the fence"), "comments stay: {after}");
    for line in original.lines().filter(|l| !l.is_empty()) {
        assert!(after.contains(line), "lost {line:?}: {after}");
    }
    assert_eq!(hermes_config_yaml(&hermes)["mcp_servers"]["fence"]["command"], "fence-cli");
    assert_eq!(std::fs::read_to_string(hermes.join("config.yaml.rkb-backup")).unwrap(), original);

    let v = hermes_install(&env, &hermes, true);
    assert_eq!(v["status"], "done", "{v}");
    assert_eq!(std::fs::read_to_string(hermes.join("config.yaml")).unwrap(), original, "the rest is byte for byte");
}

#[test]
fn install_hermes_twice_is_a_no_op() {
    let env = Env::new();
    let hermes = env.dir.path().join("hermes-home");
    hermes_install(&env, &hermes, false);
    let first = std::fs::read_to_string(hermes.join("config.yaml")).unwrap();

    let v = hermes_install(&env, &hermes, false);
    assert_eq!(v["status"], "done", "{v}");
    assert_eq!(std::fs::read_to_string(hermes.join("config.yaml")).unwrap(), first);
    let message = v["message"].as_str().unwrap();
    assert!(message.contains("unchanged") && !message.contains("written"), "{message}");
}

#[test]
fn install_hermes_refuses_a_different_entry() {
    let env = Env::new();
    let hermes = env.dir.path().join("hermes-home");
    std::fs::create_dir_all(&hermes).unwrap();
    let original = "mcp_servers:\n  rkb:\n    command: \"/usr/local/bin/other\"\n    args: [\"mcp\"]\n";
    std::fs::write(hermes.join("config.yaml"), original).unwrap();

    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("HERMES_HOME", &hermes);
    let o = rkb_with(c, &["install", "hermes", "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("not json: {}", stdout(&o)));
    assert_eq!((v["error"]["code"].as_str(), o.status.code()), (Some("refused"), Some(1)), "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("by hand"), "{v}");
    assert_eq!(std::fs::read_to_string(hermes.join("config.yaml")).unwrap(), original, "the user's entry is untouched");
}

#[test]
fn install_hermes_uninstall_keeps_an_edited_skill() {
    let env = Env::new();
    let hermes = env.dir.path().join("hermes-home");
    hermes_install(&env, &hermes, false);
    let skill = hermes.join("skills/rkb/SKILL.md");
    let edited = format!("{}mine\n", std::fs::read_to_string(&skill).unwrap());
    std::fs::write(&skill, &edited).unwrap();

    let v = hermes_install(&env, &hermes, true);
    assert_eq!(v["status"], "done", "{v}");
    assert_eq!(std::fs::read_to_string(&skill).unwrap(), edited, "an edited skill stays");
    let message = v["message"].as_str().unwrap();
    assert!(message.contains("kept") && message.contains("edited"), "{message}");
    let config = std::fs::read_to_string(hermes.join("config.yaml")).unwrap_or_default();
    assert!(!config.contains("rkb"), "the server entry goes: {config}");
}

#[test]
fn init_clone_sets_up_a_second_machine() {
    let env = fixture_kb();
    let origin = env.kb().display().to_string();
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    let second = env.dir.path().join("second");
    c.env("RKB_HOME", &second);
    let o = rkb_with(c, &["init", "--clone", &origin, "--format", "json"], "");
    assert!(o.status.success(), "{}", stdout(&o));
    let hook = std::fs::read_to_string(second.join(".git/hooks/pre-commit")).unwrap();
    assert!(hook.contains("rkb lint --staged"));
    let deny = env.cmd("git").arg("-C").arg(&second).args(["config", "receive.denyCurrentBranch"]).output().unwrap();
    assert_eq!(stdout(&deny).trim(), "updateInstead");
    let log = |dir: &Path| stdout(&env.cmd("git").arg("-C").arg(dir).args(["rev-parse", "HEAD"]).output().unwrap());
    assert_eq!(log(&second), log(&env.kb()));
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_HOME", &second);
    assert!(rkb_with(c, &["lint"], "").status.success());

    let plain = env.dir.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    for args in [vec!["init", "-q"], vec!["commit", "-q", "--allow-empty", "-m", "x"]] {
        assert!(env.cmd("git").arg("-C").arg(&plain).args(&args).status().unwrap().success());
    }
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_HOME", env.dir.path().join("third"));
    let o = rkb_with(c, &["init", "--clone", plain.to_str().unwrap(), "--format", "json"], "");
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).contains("no kb.toml"), "{}", stdout(&o));
}

#[test]
fn rkb_test_home_wins_over_rkb_home() {
    let env = fixture_kb();
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_HOME", env.dir.path().join("elsewhere")).env("RKB_TEST_HOME", env.kb());
    let o = rkb_with(c, &["lint"], "");
    assert!(o.status.success(), "{}", stdout(&o));
}

fn hook(env: &Env, event: &str, payload: &serde_json::Value) -> String {
    let o = env.rkb_in(&["hook", event], &payload.to_string());
    assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn bash(session: &str, cwd: &Path, command: &str) -> serde_json::Value {
    serde_json::json!({ "session_id": session, "cwd": cwd, "tool_name": "Bash", "tool_input": { "command": command } })
}

fn signal(env: &Env, session: &str, record: &str) {
    let p = env.dir.path().join(format!("state/rkb/sessions/{session}.jsonl"));
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let old = std::fs::read_to_string(&p).unwrap_or_default();
    std::fs::write(&p, format!("{old}{record}\n")).unwrap();
}

fn inbox_items(env: &Env) -> Vec<serde_json::Value> {
    env.json(&["inbox"], "").0["items"].as_array().unwrap().clone()
}

#[test]
fn session_end_saves_an_extract_only_after_a_signal() {
    let env = search_kb();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/transcripts/claude.jsonl");
    let payload = |id: &str| serde_json::json!({ "session_id": id, "transcript_path": fixture, "cwd": "/work/demo", "reason": "exit" });

    assert_eq!(hook(&env, "session-end", &payload("quiet")), "");
    assert!(inbox_items(&env).is_empty(), "a quiet session saves nothing");

    signal(&env, "busy", r#"{"kind":"failed","program":"cmake"}"#);
    signal(&env, "busy", r#"{"kind":"fixed","program":"cmake"}"#);
    assert_eq!(hook(&env, "pre-compact", &payload("busy")), "");
    let items = inbox_items(&env);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["kind"], "transcript");
    let (v, _) = env.json(&["inbox", "show", items[0]["id"].as_str().unwrap()], "");
    assert_eq!((v["meta"]["session"].as_str(), v["meta"]["signals"][0].as_str()), (Some("busy"), Some("fixed cmake")), "{v}");
    assert!(v["body"].as_str().unwrap().contains("[user] The configure step fails"), "{v}");

    assert_eq!(hook(&env, "session-end", &payload("busy")), "");
    assert_eq!(inbox_items(&env).len(), 1, "nothing new after the compaction extract");

    let o = env.rkb_in(&["hook", "session-end", "--harness", "pi"], &serde_json::json!({ "session_id": "busy" }).to_string());
    assert_eq!((o.status.code(), o.stdout.is_empty()), (Some(0), true), "a payload without a transcript is a logged hook error");
}

#[test]
fn session_start_names_no_inbox_count() {
    let env = search_kb();
    let start =
        |env: &Env| hook(env, "session-start", &serde_json::json!({ "session_id": "s", "cwd": env.dir.path(), "source": "startup" }));
    for i in 0..5 {
        env.json(&["note", &format!("finding {i}")], "");
    }
    let text = start(&env);
    assert!(text.contains("rkb: ") && text.contains("rkb search \"<words>\" --toon"), "the context lines: {text}");
    assert!(!text.contains("inbox"), "a full inbox adds no work nudge: {text}");
    let o = env.rkb_in(
        &["hook", "session-start", "--harness", "pi"],
        &serde_json::json!({ "session_id": "p", "cwd": env.dir.path() }).to_string(),
    );
    let pi = String::from_utf8_lossy(&o.stdout);
    assert!(!pi.contains("inbox"), "pi too: {pi}");
    let (v, _) = env.json(&["status"], "");
    assert_eq!(v["inbox"]["total"], 5, "the count stays in rkb status: {v}");
}

#[test]
fn recall_needs_a_model() {
    let env = search_kb();
    let payload = serde_json::json!({ "session_id": "r", "cwd": env.dir.path(), "prompt": "the linker says undefined reference to vtable for Widget" });
    assert_eq!(hook(&env, "prompt", &payload), "", "BM25 alone never recalls");
}

#[test]
fn eval_replays_logged_failures() {
    eval_replay(false);
}

#[test]
fn eval_replay_finds_sessions_through_written_log() {
    eval_replay(true);
}

fn eval_replay(by_log: bool) {
    let env = search_kb();
    let o = env.rkb(&["eval", "--replay"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(stdout(&o).contains("nothing to replay"), "{}", stdout(&o));

    let lesson = env.kb().join("general/cpp/undefined-vtable-means-missing-virtual.md");
    let text = std::fs::read_to_string(&lesson).unwrap();
    if by_log {
        let id = text.lines().find_map(|l| l.strip_prefix("id: ")).unwrap().trim().to_string();
        assert!(!text.contains("session"), "{text}");
        std::fs::create_dir_all(env.dir.path().join("state/rkb")).unwrap();
        let rec = serde_json::json!({ "time": 1, "id": id, "session": "s1" });
        std::fs::write(env.dir.path().join("state/rkb/written.jsonl"), format!("junk\n{rec}\n")).unwrap();
    } else {
        std::fs::write(&lesson, text.replacen("---\n\n", "meta:\n  source:\n    session: s1\n---\n\n", 1)).unwrap();
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let line = |session: &str, query: &str| {
        serde_json::json!({ "time": now - 3600, "session": session, "project": null, "system": null, "query": query, "injected": null })
            .to_string()
            + "\n"
    };
    let mut log = line("s1", "g++ collect2: error: ld returned 1 exit status")
        + &line("s1", "g++ main.cpp:(.text+0x1f): undefined reference to `vtable for Widget'");
    for s in ["a", "b", "c", "d"] {
        log += &line(s, "sed sed: 1: \"s/a/b/\": unterminated substitute pattern zebra quokka");
    }
    std::fs::create_dir_all(env.dir.path().join("state/rkb")).unwrap();
    std::fs::write(env.dir.path().join("state/rkb/replays.jsonl"), log).unwrap();

    let (v, code) = env.json(&["eval", "--replay", "--min-recall", "1"], "");
    assert_eq!(code, Some(0), "{v}");
    assert_eq!((v["failures"].as_u64(), v["sessions_with_lesson"].as_u64(), v["found"].as_u64()), (Some(6), Some(1), Some(1)), "{v}");
    assert_eq!(v["recall_at_5"], 1.0, "{v}");
    assert_eq!(v["uncovered"][0]["program"], "sed", "{v}");
    assert_eq!(v["uncovered"][0]["count"], 4, "{v}");
    assert!(v["uncovered"][0]["example"].as_str().unwrap().contains("unterminated"), "{v}");
    assert!(v["would_inject"].as_u64().unwrap() >= 1, "the vtable query finds its lesson: {v}");
    assert_eq!(stdout(&env.git(&["status", "--porcelain", "--", ".rkb"])), "", "changes nothing");
    assert_eq!(env.rkb(&["eval", "--replay", "--self"]).status.code(), Some(2));
    assert_eq!(env.rkb(&["eval", "--days", "3"]).status.code(), Some(2), "--days needs --replay");
}

#[test]
fn capture_of_a_large_transcript_is_fast() {
    let env = search_kb();
    let path = env.dir.path().join("big.jsonl");
    let mut text = String::new();
    let mut n = 0;
    while text.len() < 20 << 20 {
        n += 1;
        text.push_str(&format!(
            "{}\n{}\n",
            serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":format!("t{n}"),"name":"Read","input":{"file_path":"/work/demo/a.c"}}]}}),
            serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":format!("t{n}"),"content":"x".repeat(2000)}]}})
        ));
    }
    text.push_str(&format!("{}\n", serde_json::json!({"type":"user","message":{"role":"user","content":"remember the flag"}})));
    std::fs::write(&path, text).unwrap();
    signal(&env, "big", r#"{"kind":"remember"}"#);
    let start = std::time::Instant::now();
    hook(&env, "session-end", &serde_json::json!({ "session_id": "big", "transcript_path": path }));
    let took = start.elapsed();
    assert_eq!(inbox_items(&env).len(), 1);
    if !cfg!(debug_assertions) {
        assert!(took < std::time::Duration::from_secs(1), "{took:?}");
    }
    eprintln!("20 MB transcript captured in {took:?}");
}

#[test]
fn hook_never_fails() {
    let env = Env::new();
    let o = env.rkb_in(&["hook", "tool-failed"], "not json");
    assert_eq!((o.status.code(), stdout(&o)), (Some(0), String::new()));
    let log = std::fs::read_to_string(env.dir.path().join("state/rkb/hook-errors.log")).unwrap();
    assert!(log.contains("tool-failed"), "{log}");
    assert_eq!(hook(&env, "session-start", &serde_json::json!({ "session_id": "s", "cwd": env.dir.path() })), "");
    assert_eq!(hook(&env, "no-such-event", &serde_json::json!({})), "");
    assert!(env.dir.path().join("state/rkb/heartbeat/claude-code").exists());
}

#[test]
fn hook_session_start_uses_the_payload_cwd() {
    let env = fixture_kb();
    let repo = project_repo(&env, "git@github.com:llnl/dftracer.git");
    let out = hook(&env, "session-start", &serde_json::json!({ "session_id": "s", "source": "startup", "cwd": repo }));
    assert!(out.trim_end().lines().count() <= 5, "{out}");
    assert!(out.contains("project dftracer") && out.contains("rkb list projects/dftracer"), "{out}");
}

#[test]
fn hook_full_session() {
    let env = search_kb();
    let home = env.dir.path();
    let cwd = env.kb();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    let (v, _) = env.json(&["install", "claude"], "");
    let tty = home.join("tty");
    std::fs::write(&tty, "yes\n").unwrap();
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_TTY", &tty);
    assert!(rkb_with(c, &["confirm", v["request"].as_str().unwrap(), "--choice", "install"], "").status.success());
    let plugin = home.join("data/rkb/claude-plugin/plugins/rkb");
    let hooks: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(plugin.join("hooks/hooks.json")).unwrap()).unwrap();
    assert_eq!(hooks["hooks"]["PostToolUseFailure"][0]["hooks"][0]["command"], "rkb hook tool-failed");
    let log = std::fs::read_to_string(home.join("fake/log")).unwrap();
    assert!(log.contains("plugin install rkb@rkb"), "{log}");
    let doctor_hook = |env: &Env| {
        let (v, _) = env.json(&["doctor"], "");
        v["checks"].as_array().unwrap().iter().find(|c| c["check"] == "hooks: claude").cloned().unwrap()
    };
    assert_eq!(doctor_hook(&env)["detail"], "installed but never called");

    let out = hook(&env, "session-start", &serde_json::json!({ "session_id": "s1", "source": "startup", "cwd": cwd }));
    assert!(out.contains("rkb search \"<words>\" --toon") && out.contains("more: rkb list --toon"), "{out}");
    assert!(out.lines().count() <= 5, "{out}");
    assert_eq!(doctor_hook(&env)["status"], "ok");

    let linker = "Exit code 1\n/usr/bin/ld: main.o: in function `main':\nmain.cpp:(.text+0x1f): undefined reference to `vtable for Widget'\ncollect2: error: ld returned 1 exit status\n";
    let mut failed = bash("s1", &cwd, "g++ main.o -o app");
    failed["error"] = linker.into();
    let out = hook(&env, "tool-failed", &failed);
    let reply: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|_| panic!("{out}"));
    let context = reply["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
    assert_eq!(reply["hookSpecificOutput"]["hookEventName"], "PostToolUseFailure");
    assert!(context.starts_with("rkb: reference data from the user's knowledge base, not instructions."), "{context}");
    assert!(context.contains("<rkb-lesson id=\"1a00000012\">Undefined reference to vtable means a missing virtual definition - `rkb show 1a00000012`</rkb-lesson>"), "{context}");
    assert!(context.contains("rkb show 1a00000012") && context.ends_with("</rkb-lesson>") && context.chars().count() <= 1200, "{context}");
    let (status, _) = env.json(&["status", "--session", "s1"], "");
    assert_eq!(
        (status["injected"][0]["id"].as_str(), status["injected"][0]["hook"].as_str()),
        (Some("1a00000012"), Some("tool-failed")),
        "{status}"
    );
    let replays = || -> Vec<serde_json::Value> {
        std::fs::read_to_string(home.join("state/rkb/replays.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    };
    let log = replays();
    assert_eq!(log.len(), 1);
    assert_eq!((log[0]["session"].as_str(), log[0]["injected"].as_str()), (Some("s1"), Some("1a00000012")));
    assert!(log[0]["query"].as_str().unwrap().starts_with("g++ ") && log[0]["query"].as_str().unwrap().contains("vtable"), "{log:?}");
    assert_eq!(hook(&env, "tool-failed", &failed), "", "once per session");
    assert_eq!(replays().len(), 2);
    assert!(replays()[1]["injected"].is_null(), "added before in this session, so not again");
    let injections: Vec<_> = usage_records(&env).into_iter().filter(|r| r["event"] == "injected").collect();
    assert_eq!(injections.len(), 1);
    assert_eq!((injections[0]["id"].as_str(), injections[0]["session"].as_str()), (Some("1a00000012"), Some("s1")));
    assert!(
        injections[0]["reason"].as_str().unwrap().starts_with("explore"),
        "the first push of a candidate is an explore: {}",
        injections[0]
    );
    assert_eq!(hook(&env, "tool-ok", &bash("s1", &cwd, "g++ main.o -o app")), "");
    assert_eq!(hook(&env, "tool-ok", &bash("s1", &cwd, "g++ main.o -o app")), "", "a second success infers nothing new");
    let inferred: Vec<_> = usage_records(&env).into_iter().filter(|r| r["event"] == "inferred").collect();
    assert_eq!(inferred.len(), 1, "g++ worked after the lesson was injected");
    assert_eq!((inferred[0]["id"].as_str(), inferred[0]["session"].as_str()), (Some("1a00000012"), Some("s1")));
    let (v, code) = env.json(&["review", "--signals"], "");
    assert_eq!(code, Some(0), "{v}");
    assert_eq!((v["signals"]["injected"].as_u64(), v["signals"]["injected_then_helped"].as_u64()), (Some(1), Some(1)), "{v}");
    assert_eq!(env.json(&["used", "1a00000012", "--irrelevant", "--session", "s1"], "").0["status"], "recorded");
    let (v, _) = env.json(&["review", "--signals"], "");
    let path = &v["signals"]["paths"]["explore"];
    assert_eq!((path["injected"].as_u64(), path["helped"].as_u64(), path["irrelevant"].as_u64()), (Some(1), Some(1), Some(1)), "{v}");
    assert_eq!(v["signals"]["paths"]["tool-failed"]["injected"], 0, "a candidate push is not a proven push: {v}");
    assert_eq!(v["signals"]["paths"]["recall"]["injected"], 0, "{v}");
    assert!(stdout(&env.rkb(&["review", "--signals"])).contains("explore: 1 injected, 1 then helped (100%), 1 marked irrelevant"));

    let mut weak = bash("s1", &cwd, "make");
    weak["error"] = "Exit code 2\nerror: the build did not work today\n".into();
    assert_eq!(hook(&env, "tool-failed", &weak), "");
    let mut interrupted = bash("s1", &cwd, "sleep 100");
    interrupted["is_interrupt"] = true.into();
    assert_eq!(hook(&env, "tool-failed", &interrupted), "");

    let mut cmake = bash("s1", &cwd, "cmake -B build");
    cmake["error"] = "Exit code 1\nsomething odd\n".into();
    hook(&env, "tool-failed", &cmake);
    assert_eq!(hook(&env, "tool-ok", &bash("s1", &cwd, "cmake -B build -DX=1")), "");
    assert_eq!(hook(&env, "tool-ok", &bash("s1", &cwd, "ls")), "");
    assert_eq!(
        hook(&env, "prompt", &serde_json::json!({ "session_id": "s1", "prompt": "actually, use the release build and remember this" })),
        ""
    );
    let session = std::fs::read_to_string(home.join("state/rkb/sessions/s1.jsonl")).unwrap();
    let kinds: Vec<String> =
        session.lines().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["kind"].as_str().unwrap().to_string()).collect();
    assert_eq!(
        kinds,
        ["failed", "injected", "failed", "fixed", "inferred", "failed", "nohit", "failed", "nohit", "fixed", "correction", "remember"],
        "{session}"
    );
    assert!(
        !session.contains("sleep") && !session.contains("-B") && !session.contains("release") && !session.contains("actually"),
        "{session}"
    );

    let stop = serde_json::json!({ "session_id": "s1", "stop_hook_active": false });
    let out = hook(&env, "stop", &stop);
    let reply: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|_| panic!("{out}"));
    assert_eq!(reply["decision"], "block", "a correction, a remember and fixes score high, so the agent records it now: {out}");
    let reason = reply["reason"].as_str().unwrap();
    assert!(
        reason.contains("worth a lesson") && reason.contains("cmake") && reason.contains("rkb_search") && reason.contains("rkb_add"),
        "{reason}"
    );
    assert_eq!(hook(&env, "stop", &stop), "", "no new signal");
    hook(&env, "prompt", &serde_json::json!({ "session_id": "s1", "prompt": "no, use ninja" }));
    assert_eq!(hook(&env, "stop", &serde_json::json!({ "session_id": "s1", "stop_hook_active": true })), "");
    let out = hook(&env, "stop", &stop);
    let reply: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|_| panic!("{out}"));
    assert!(reply.get("decision").is_none(), "asked once already, so only a note: {out}");
    assert!(reply["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains("1 correction(s)"), "{out}");
    hook(&env, "prompt", &serde_json::json!({ "session_id": "s1", "prompt": "remember that ninja is faster" }));
    let toml = std::fs::read_to_string(cwd.join("kb.toml")).unwrap();
    std::fs::write(cwd.join("kb.toml"), format!("{toml}\n[hooks]\nstop_nudge = false\nreplay_log = false\n")).unwrap();
    assert_eq!(hook(&env, "stop", &stop), "", "turned off");
    let before = replays().len();
    hook(&env, "tool-failed", &weak);
    assert_eq!(replays().len(), before, "replay_log = false");

    assert_eq!(hook(&env, "pre-tool", &bash("s1", &cwd, "ls -la")), "");
    let (v, _) = env.json(&["install", "claude", "--uninstall"], "");
    let req = v["request"].as_str().unwrap();
    let question = v["question"].as_str().unwrap();
    for command in [format!("sh -c 'rkb confirm {req} --choice \"uninstall\"'"), format!("/opt/bin/rkb confirm {req} --choice uninstall")] {
        let out = hook(&env, "pre-tool", &bash("s1", &cwd, &command));
        let reply: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|_| panic!("{out}"));
        assert_eq!(reply["hookSpecificOutput"]["permissionDecision"], "ask");
        let reason = reply["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains(question) && reason.ends_with("choice: uninstall"), "{reason}");
    }
    let out = hook(&env, "pre-tool", &bash("s1", &cwd, "rkb confirm"));
    assert!(out.contains("\"ask\""), "{out}");
    let in_mode = |mode: &str, command: &str| {
        let mut p = bash("s1", &cwd, command);
        p["permission_mode"] = mode.into();
        hook(&env, "pre-tool", &p)
    };
    let command = format!("rkb confirm {req} --choice uninstall");
    for mode in ["bypassPermissions", "dontAsk"] {
        let out = in_mode(mode, &command);
        let reply: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|_| panic!("{out}"));
        assert_eq!(reply["hookSpecificOutput"]["permissionDecision"], "deny", "{mode}");
        let reason = reply["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(
            reason.contains(question)
                && reason.contains(&format!("run: rkb confirm {req} --choice \"uninstall\""))
                && reason.contains(mode),
            "{reason}"
        );
    }
    assert!(in_mode("auto", &command).contains("\"ask\""));
    assert!(in_mode("acceptEdits", &command).contains("\"ask\""));
    assert_eq!(in_mode("bypassPermissions", "ls -la"), "");

    let (v, _) = env.json(&["review", "--signals"], "");
    assert_eq!(v["signals"]["repeated"], 0, "{v}");
    failed["session_id"] = "s2".into();
    assert!(hook(&env, "tool-failed", &failed).contains("1a00000012"), "a new session gets the lesson again");
    let (v, _) = env.json(&["review", "--signals"], "");
    assert_eq!((v["signals"]["repeated"].as_u64(), v["signals"]["repeated_with_lesson"].as_u64()), (Some(1), Some(1)), "{v}");
    assert_eq!(std::fs::read_to_string(home.join("state/rkb/hook-errors.log")).unwrap_or_default(), "");
}

#[test]
fn hook_heartbeat_per_harness() {
    let env = Env::new();
    let beat = |h: &str| env.dir.path().join("state/rkb/heartbeat").join(h);
    let o = env.rkb_in(&["hook", "tool-ok", "--harness", "pi"], "{}");
    assert_eq!((o.status.code(), stdout(&o)), (Some(0), String::new()));
    assert!(beat("pi").exists() && !beat("claude-code").exists());
    let o = env.rkb_in(&["hook", "tool-ok", "--harness", "bogus"], "{}");
    assert_eq!((o.status.code(), stdout(&o)), (Some(0), String::new()));
    assert!(!beat("bogus").exists());
    let log = std::fs::read_to_string(env.dir.path().join("state/rkb/hook-errors.log")).unwrap();
    assert!(log.contains("unknown harness"), "{log}");
}

#[test]
fn extension_drives_rkb_hook() {
    let ts = Command::new("node")
        .args(["-p", "process.features.typescript"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    if !matches!(ts.as_deref(), Some("strip" | "transform")) {
        eprintln!("skipped: no node with TypeScript support on PATH");
        return;
    }
    let env = search_kb();
    let toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    std::fs::write(env.kb().join("kb.toml"), format!("{toml}\n[hooks]\nstop_nudge = true\n")).unwrap();
    std::fs::create_dir_all(env.dir.path().join(".claude")).unwrap();
    let (req, _) = env.json(&["install", "claude"], "");
    let package = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../extensions/pi");
    if !package.join("node_modules/typebox").exists() {
        eprintln!("skipped: run `npm install` in extensions/pi to test the extension");
        return;
    }
    let script = package.join("test/run.mjs");
    // Inside the package, so `import "typebox"` resolves as pi and omp resolve it.
    let generated = package.join(".test");
    std::fs::create_dir_all(&generated).unwrap();
    for (harness, h) in [("pi", rkb_core::install::Harness::Pi), ("omp", rkb_core::install::Harness::Omp)] {
        let file = generated.join(format!("rkb-{harness}-{}.ts", std::process::id()));
        // This test process is not the rkb binary, so point the extension at the `rkb` on PATH.
        let here = serde_json::to_string(&rkb_core::install::hook_binary()).unwrap();
        std::fs::write(&file, h.extension().replace(&format!("const RKB: string = {here};"), "const RKB: string = \"rkb\";")).unwrap();
        let o = env
            .cmd("node")
            .arg(&script)
            .arg(&file)
            .arg(harness)
            .env("RKB_KB", env.kb())
            .env("RKB_TIMEOUT_MS", "30000")
            .env("RKB_REQUEST", req["request"].as_str().unwrap())
            .env("RKB_QUESTION", req["question"].as_str().unwrap())
            .env("RKB_TRANSCRIPT", PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/transcripts/pi.jsonl"))
            .output()
            .unwrap();
        let _ = std::fs::remove_file(&file);
        assert!(o.status.success(), "{harness}: {}", stdout(&o));
        assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), format!("ok {harness}"));
    }
}

#[test]
fn install_pi_from_an_agent_and_list() {
    let env = Env::new();
    let home = env.dir.path();
    std::fs::create_dir_all(home.join(".pi")).unwrap();
    let (v, code) = env.json(&["install", "pi"], "");
    assert_eq!((v["status"].as_str(), code), (Some("needs_user"), Some(3)));
    let q = v["question"].as_str().unwrap();
    for f in ["~/.pi/agent/skills/rkb/SKILL.md", "~/.pi/agent/extensions/rkb.ts", "trust.toml"] {
        assert!(q.contains(f), "{f} missing: {q}");
    }
    assert!(!home.join(".pi/agent/extensions").exists(), "nothing written before confirm");

    let tty = home.join("tty");
    std::fs::write(&tty, "yes\n").unwrap();
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_TTY", &tty);
    let o = rkb_with(c, &["confirm", v["request"].as_str().unwrap(), "--choice", "install", "--format", "json"], "");
    assert!(o.status.success(), "{}", stdout(&o));
    assert!(std::fs::read_to_string(home.join("config/rkb/trust.toml")).unwrap().contains("[harness.pi]"));

    let (v, _) = env.json(&["install", "--list"], "");
    let pi = v["harnesses"].as_array().unwrap().iter().find(|r| r["harness"] == "pi").unwrap().clone();
    assert_eq!(pi["extension"], serde_json::json!({ "installed": true, "current": true }));
    let claude = v["harnesses"].as_array().unwrap().iter().find(|r| r["harness"] == "claude").unwrap().clone();
    assert!(claude.get("extension").is_none());
    let human = stdout(&env.rkb(&["install", "--list", "--format", "human"]));
    assert!(human.contains("~/.pi/agent/extensions/rkb.ts") && human.contains("current"), "{human}");
}

fn json_with_auto(env: &Env, auto: Option<&str>, args: &[&str]) -> (serde_json::Value, Option<i32>) {
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    if let Some(a) = auto {
        c.env("RKB_AUTO_CONFIRM", a);
    }
    let mut all = args.to_vec();
    all.extend(["--format", "json"]);
    let o = rkb_with(c, &all, "");
    (serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("not json: {}", stdout(&o))), o.status.code())
}

#[test]
fn auto_confirm_answers_only_listed_options() {
    let env = Env::new();
    let ext = env.dir.path().join(".pi/agent/extensions/rkb.ts");
    std::fs::create_dir_all(env.dir.path().join(".pi")).unwrap();

    for auto in [None, Some("continue")] {
        let (v, code) = json_with_auto(&env, auto, &["install", "pi"]);
        assert_eq!((v["status"].as_str(), code), (Some("needs_user"), Some(3)), "{auto:?}: {v}");
        assert!(!ext.exists());
    }

    let (v, code) = json_with_auto(&env, Some("continue,install"), &["install", "pi"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["auto_confirmed"], "install (RKB_AUTO_CONFIRM)");
    assert!(ext.exists());
}

#[test]
fn install_list_shows_the_tier_docs() {
    let env = Env::new();
    let skill = env.dir.path().join(".pi/agent/skills/rkb/SKILL.md");
    std::fs::create_dir_all(env.dir.path().join(".pi")).unwrap();
    let (v, code) = json_with_auto(&env, Some("install"), &["install", "pi"]);
    assert_eq!(code, Some(0), "{v}");
    let text = std::fs::read_to_string(&skill).unwrap();
    assert!(text.contains("rkb tier") && text.contains("`candidate`") && text.contains("`proven`"), "the skill documents the tiers");
    let (v, _) = env.json(&["install", "--list"], "");
    assert_eq!(v["harnesses"][1]["current"].as_bool(), Some(true), "the installed skill matches this rkb: {v}");
}

#[test]
fn confirm_without_a_terminal_needs_the_variable() {
    let env = Env::new();
    std::fs::create_dir_all(env.dir.path().join(".pi")).unwrap();
    let (v, _) = env.json(&["install", "pi"], "");
    let id = v["request"].as_str().unwrap();
    let (e, _) = json_with_auto(&env, None, &["confirm", id, "--choice", "install"]);
    assert_eq!(e["error"]["code"], "needs_terminal", "{e}");
    let (e, code) = json_with_auto(&env, Some("install"), &["confirm", id, "--choice", "install"]);
    assert_eq!(code, Some(0), "{e}");
    assert!(env.dir.path().join(".pi/agent/extensions/rkb.ts").exists());
}

/// Two clones of one knowledge base through a bare remote: the fixture at `env.kb()` and a clone at `second`.
fn sync_pair() -> (Env, PathBuf, PathBuf) {
    let env = fixture_kb();
    let bare = env.dir.path().join("remote.git");
    assert!(env.cmd("git").args(["init", "-q", "--bare"]).arg(&bare).status().unwrap().success());
    assert!(env.git(&["remote", "add", "origin", bare.to_str().unwrap()]).status.success());
    let (v, code) = sync_at(&env, &env.kb(), &[]);
    assert_eq!(code, Some(0), "{v}");
    assert!(!v["pushed"].as_array().unwrap().is_empty(), "empty remote gets the branch: {v}");
    let second = env.dir.path().join("second");
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_HOME", &second);
    assert!(rkb_with(c, &["init", "--clone", bare.to_str().unwrap()], "").status.success());
    (env, bare, second)
}

fn sync_at(env: &Env, kb: &Path, extra: &[&str]) -> (serde_json::Value, Option<i32>) {
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_HOME", kb);
    let mut args = vec!["sync", "--format", "json"];
    args.extend(extra);
    let o = rkb_with(c, &args, "");
    (serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o))), o.status.code())
}

fn commit_file(env: &Env, kb: &Path, rel: &str, text: &str, message: &str) {
    let p = kb.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
    let git = |args: &[&str]| env.cmd("git").arg("-C").arg(kb).args(args).output().unwrap();
    assert!(git(&["add", "-A"]).status.success());
    let o = git(&["commit", "-q", "-m", message]);
    assert!(o.status.success(), "{}", stdout(&o));
}

fn fact(id: &str, title: &str, statement: &str) -> String {
    FACT.replace("0a1b2c3d4e", id).replace("# A fact", &format!("# {title}")).replace("Something is true.", statement)
}

#[test]
fn usage_from_two_machines_syncs_without_conflict() {
    let (env, _bare, second) = sync_pair();
    let id = {
        let mut todo = vec![env.kb().join("general")];
        let mut found = None;
        while let Some(d) = todo.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    todo.push(p);
                } else if found.is_none() && p.extension().is_some_and(|x| x == "md") && e.file_name() != "README.md" {
                    found = std::fs::read_to_string(&p)
                        .unwrap()
                        .lines()
                        .find_map(|l| l.strip_prefix("id: ").map(|v| v.trim_matches('"').to_string()));
                }
            }
        }
        found.unwrap()
    };
    let used = |kb: &Path, config: &str| {
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.env("RKB_HOME", kb).env("XDG_CONFIG_HOME", env.dir.path().join(config)).env("CLAUDE_CODE_SESSION_ID", config);
        let o = rkb_with(c, &["used", &id, "--worked", "--format", "json"], "");
        assert!(o.status.success(), "{}", stdout(&o));
    };
    used(&env.kb(), "machine-a");
    used(&second, "machine-b");
    for kb in [env.kb(), second.clone(), env.kb()] {
        let (v, code) = sync_at(&env, &kb, &[]);
        assert_eq!(code, Some(0), "no conflict: {v}");
    }
    for kb in [env.kb(), second] {
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.env("RKB_HOME", &kb);
        let o = rkb_with(c, &["show", &id, "--format", "json"], "");
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)));
        assert_eq!(v["usage"]["worked"], 2, "both machines' records count: {v}");
        assert!(std::fs::read_dir(kb.join(".rkb/usage")).unwrap().count() >= 2, "one file per machine");
    }
}

#[test]
fn sync_pulls_pushes_and_refuses() {
    let (env, _bare, second) = sync_pair();
    let (v, code) = sync_at(&env, &env.kb(), &[]);
    assert_eq!((code, v["pulled"].clone(), v["pushed"].clone()), (Some(0), serde_json::json!([]), serde_json::json!([])));
    let human = {
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.env("RKB_HOME", env.kb());
        stdout(&rkb_with(c, &["sync", "--format", "human"], ""))
    };
    assert!(human.contains("up to date"), "{human}");

    commit_file(
        &env,
        &second,
        "general/cpp/a-new-fact.md",
        &fact("0a1b2c3d4e", "A new fact", "One."),
        "add(general/cpp): A new fact [0a1b2c3d4e]",
    );
    let (v, code) = sync_at(&env, &second, &[]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["pushed"][0]["subject"], "add(general/cpp): A new fact [0a1b2c3d4e]");
    let (v, _) = sync_at(&env, &env.kb(), &[]);
    assert_eq!(v["pulled"][0]["subject"], "add(general/cpp): A new fact [0a1b2c3d4e]");
    assert!(env.kb().join("general/cpp/a-new-fact.md").exists());

    std::fs::write(env.kb().join("general/cpp/README.md"), "# C++ changed\n").unwrap();
    let (v, code) = sync_at(&env, &env.kb(), &[]);
    assert_eq!((v["error"]["code"].as_str(), code), (Some("refused"), Some(1)));
    assert!(v["error"]["message"].as_str().unwrap().contains("general/cpp/README.md"), "{v}");
    assert!(env.git(&["checkout", "--", "general/cpp/README.md"]).status.success());

    let (v, code) = sync_at(&env, &env.kb(), &["--remote", "nowhere"]);
    assert_eq!((v["error"]["code"].as_str(), code), (Some("not_found"), Some(1)));
    assert_eq!(v["error"]["remotes"], serde_json::json!(["origin"]));
}

#[test]
fn sync_conflict_stops_in_the_rebase() {
    let (env, _bare, second) = sync_pair();
    let rel = "general/cpp/a-shared-fact.md";
    commit_file(&env, &env.kb(), rel, &fact("0a1b2c3d4e", "A shared fact", "One."), "add: shared");
    assert_eq!(sync_at(&env, &env.kb(), &[]).1, Some(0));
    assert_eq!(sync_at(&env, &second, &[]).1, Some(0));
    commit_file(&env, &env.kb(), rel, &fact("0a1b2c3d4e", "A shared fact", "Two."), "edit: first");
    commit_file(&env, &second, rel, &fact("0a1b2c3d4e", "A shared fact", "Three."), "edit: second");
    assert_eq!(sync_at(&env, &env.kb(), &[]).1, Some(0));
    let (v, code) = sync_at(&env, &second, &[]);
    assert_eq!((v["error"]["code"].as_str(), code), (Some("conflict"), Some(1)), "{v}");
    assert_eq!(v["error"]["files"], serde_json::json!([rel]));
    assert!(v["error"]["fix"].as_str().unwrap().contains("rebase --abort"));
    let status = stdout(&env.cmd("git").arg("-C").arg(&second).args(["status"]).output().unwrap());
    assert!(status.contains("rebase in progress"), "{status}");
    let (v, _) = sync_at(&env, &second, &[]);
    assert!(v["error"]["message"].as_str().unwrap().contains("rebase or merge is in progress"), "{v}");
}

#[test]
fn sync_keeps_a_bad_pull_and_does_not_push() {
    let (env, bare, second) = sync_pair();
    commit_file(&env, &env.kb(), "general/cpp/first.md", &fact("0a1b2c3d4e", "First", "One."), "add: first");
    assert_eq!(sync_at(&env, &env.kb(), &[]).1, Some(0));
    commit_file(&env, &second, "general/git/second.md", &fact("0a1b2c3d4e", "Second", "Two."), "add: second");
    let branch =
        stdout(&env.cmd("git").arg("-C").arg(&second).args(["symbolic-ref", "--short", "HEAD"]).output().unwrap()).trim().to_string();
    let remote_head = |dir: &Path| stdout(&env.cmd("git").arg("-C").arg(dir).args(["rev-parse", &branch]).output().unwrap());
    let before = remote_head(&bare);
    assert_eq!(before.trim().len(), 40, "{before}");
    let (v, code) = sync_at(&env, &second, &[]);
    assert_eq!((v["status"].as_str(), code), (Some("not_pushed"), Some(1)), "{v}");
    assert_eq!(v["pulled"][0]["subject"], "add: first");
    let dup = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["rule"].as_str().is_some_and(|r| r.contains("id")))
        .cloned()
        .unwrap_or_default();
    let both = format!("{} {}", dup["path"].as_str().unwrap_or(""), dup["message"].as_str().unwrap_or(""));
    assert!(both.contains("general/cpp/first.md") && both.contains("general/git/second.md"), "{v}");
    assert!(second.join("general/cpp/first.md").exists() && second.join("general/git/second.md").exists(), "both commits kept");
    assert_eq!(remote_head(&bare), before, "nothing pushed");
}

#[test]
fn sync_pushes_into_a_checked_out_clone() {
    let env = fixture_kb();
    let cluster = env.dir.path().join("cluster");
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_HOME", &cluster);
    assert!(rkb_with(c, &["init", "--clone", env.kb().to_str().unwrap()], "").status.success());
    assert!(env.git(&["remote", "add", "cluster", cluster.to_str().unwrap()]).status.success());
    commit_file(&env, &env.kb(), "general/cpp/from-laptop.md", &fact("0a1b2c3d4e", "From the laptop", "One."), "add: from laptop");
    let (v, code) = sync_at(&env, &env.kb(), &["--remote", "cluster"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["pushed"][0]["subject"], "add: from laptop");
    assert!(cluster.join("general/cpp/from-laptop.md").exists(), "updateInstead updates the working tree");
}

fn import_dir(env: &Env, files: &[(&str, &str)]) -> PathBuf {
    let dir = env.dir.path().join("to-import");
    for (rel, text) in files {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    dir
}

fn confirm_on_tty(env: &Env, request: &str, choice: &str) -> (serde_json::Value, Option<i32>) {
    let tty = env.dir.path().join("tty");
    std::fs::write(&tty, "yes\n").unwrap();
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_TTY", &tty);
    let o = rkb_with(c, &["confirm", request, "--choice", choice, "--format", "json"], "");
    (serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o))), o.status.code())
}

fn commit_count(env: &Env) -> usize {
    stdout(&env.git(&["rev-list", "--count", "HEAD"])).trim().parse().unwrap()
}

#[test]
fn import_invalid_batch_writes_nothing() {
    let env = kb_with_topics();
    let dir = import_dir(
        &env,
        &[
            ("notes.md", "# loose\n"),
            ("cmake/README.md", "# CMake\n"),
            ("cmake/no-title.md", "---\ntype: fact\n---\n\n## Statement\nx\n"),
            ("cmake/ok.md", &lesson_with("Fine lesson")),
        ],
    );
    let before = commit_count(&env);
    let (v, code) = env.json(&["import", dir.to_str().unwrap()], "");
    assert_eq!((v["status"].as_str(), code), (Some("invalid"), Some(1)), "{v}");
    let sources: Vec<&str> = v["invalid"].as_array().unwrap().iter().map(|i| i["source"].as_str().unwrap()).collect();
    assert_eq!(sources, ["cmake/README.md", "cmake/no-title.md", "notes.md"]);
    assert!(v["invalid"][2]["reasons"][0].as_str().unwrap().contains("<topic>/<name>.md"));
    assert!(v["invalid"][1]["reasons"][0].as_str().unwrap().contains("Title"), "{v}");
    assert_eq!(commit_count(&env), before);
    assert!(
        !env.dir.path().join("state/rkb/requests").exists()
            || std::fs::read_dir(env.dir.path().join("state/rkb/requests")).unwrap().next().is_none()
    );

    let empty = env.dir.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let (v, code) = env.json(&["import", empty.to_str().unwrap()], "");
    assert_eq!((v["error"]["code"].as_str(), code), (Some("not_found"), Some(1)));
}

#[test]
fn import_report_then_import_all() {
    let env = kb_with_topics();
    let dir = import_dir(
        &env,
        &[
            ("cmake/hdf5.md", &lesson_with("CMake cannot find HDF5 unless HDF5_ROOT is set")),
            ("cpp/a.md", &lesson_with("Vtable errors come from missing virtual definitions")),
            ("cpp/b.md", &lesson_with("Vtable errors come from missing virtual definitions")),
            ("ninja/x.md", &lesson_with("Ninja rebuilds everything after a clock change")),
            ("cmake/old.md", &lesson_with("Presets replace long cmake command lines")),
        ],
    );
    let existing = add_ok(&env, "cmake", &lesson_with("Presets replace long cmake command lines"));
    let before = commit_count(&env);
    let (v, code) = env.json(&["import", dir.to_str().unwrap()], "");
    assert_eq!((v["status"].as_str(), code), (Some("needs_user"), Some(3)), "{v}");
    let q = v["question"].as_str().unwrap();
    assert!(q.contains("Import 5 lessons") && q.contains("[new topic folder general/ninja]"), "{q}");
    let old_line = q.lines().find(|l| l.contains("(from cmake/old.md)")).unwrap();
    assert!(old_line.contains(&format!("[looks like {}]", existing["id"].as_str().unwrap())), "{q}");
    let human = stdout(&env.rkb(&["import", dir.to_str().unwrap(), "--format", "human"]));
    assert!(human.contains("Needs your decision") && human.contains("Options: \"import all\""), "{human}");
    let b_line = q.lines().find(|l| l.contains("(from cpp/b.md)")).unwrap();
    assert!(b_line.contains("[looks like"), "{q}");
    assert_eq!(v["options"], serde_json::json!(["import all", "import without duplicates", "cancel"]));
    assert_eq!(commit_count(&env), before, "nothing written before confirm");

    let (v, code) = confirm_on_tty(&env, v["request"].as_str().unwrap(), "import all");
    assert_eq!((v["status"].as_str(), code), (Some("done"), Some(0)), "{v}");
    assert_eq!(v["added"].as_array().unwrap().len(), 5);
    assert_eq!(commit_count(&env), before + 5, "one commit per lesson");
    assert!(env.kb().join("general/ninja").is_dir());
    assert!(dir.join("cpp/a.md").exists() && dir.join("ninja/x.md").exists(), "sources are left alone");
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "");
}

#[test]
fn import_without_duplicates_and_stop_on_change() {
    let env = kb_with_topics();
    let dir = import_dir(&env, &[("cpp/a.md", &lesson_with("Same title here")), ("cpp/b.md", &lesson_with("Same title here"))]);
    let (v, _) = env.json(&["import", dir.to_str().unwrap()], "");
    let (v, _) = confirm_on_tty(&env, v["request"].as_str().unwrap(), "import without duplicates");
    assert_eq!((v["added"].as_array().unwrap().len(), v["skipped"].clone()), (1, serde_json::json!(["cpp/b.md"])), "{v}");

    let dir2 = env.dir.path().join("second-batch");
    for (rel, title) in [
        ("cmake/a.md", "Toolchain files must be set before project"),
        ("cmake/b.md", "Generator expressions run at build time"),
        ("ninj/c.md", "Ninja rebuilds after a clock change"),
    ] {
        let p = dir2.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, lesson_with(title)).unwrap();
    }
    let (report, _) = env.json(&["import", dir2.to_str().unwrap()], "");
    assert_eq!(report["options"], serde_json::json!(["import all", "cancel"]));
    // A close topic name no longer asks, so a topic created after the report does not stop the import.
    add_ok(&env, "ninja", &lesson_with("Ninja needs a build.ninja file"));
    let (v, code) = confirm_on_tty(&env, report["request"].as_str().unwrap(), "import all");
    assert_eq!((v["status"].as_str(), code), (Some("done"), Some(0)), "{v}");
    assert_eq!(v["added"].as_array().unwrap().len(), 3);
}

#[test]
fn sync_refuses_an_unrelated_history() {
    let env = fixture_kb();
    let stranger = env.dir.path().join("stranger");
    std::fs::create_dir_all(&stranger).unwrap();
    for args in [vec!["init", "-q"], vec!["commit", "-q", "--allow-empty", "-m", "other kb"]] {
        assert!(env.cmd("git").arg("-C").arg(&stranger).args(&args).status().unwrap().success());
    }
    assert!(env.git(&["remote", "add", "stranger", stranger.to_str().unwrap()]).status.success());
    let head = stdout(&env.git(&["rev-parse", "HEAD"]));
    let (v, code) = sync_at(&env, &env.kb(), &["--remote", "stranger"]);
    assert_eq!((v["error"]["code"].as_str(), code), (Some("refused"), Some(1)), "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("shares no history"), "{v}");
    assert_eq!(stdout(&env.git(&["rev-parse", "HEAD"])), head);
}

#[test]
fn sync_through_a_bundle_file() {
    let env = fixture_kb();
    let file = env.dir.path().join("carry/kb.bundle");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let bundle = file.to_str().unwrap();

    let (v, code) = sync_at(&env, &env.kb(), &["--bundle", bundle]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["pulled"], serde_json::json!([]));
    assert_eq!(v["pushed"].as_array().unwrap().len(), commit_count(&env), "a new file gets the whole branch");
    assert!(file.exists());
    let (v, _) = sync_at(&env, &env.kb(), &["--bundle", bundle]);
    assert_eq!((v["pulled"].clone(), v["pushed"].clone()), (serde_json::json!([]), serde_json::json!([])));

    let cluster = env.dir.path().join("cluster");
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_HOME", &cluster);
    assert!(rkb_with(c, &["init", "--clone", bundle], "").status.success());
    commit_file(&env, &cluster, "general/cpp/from-cluster.md", &fact("0a1b2c3d4e", "From the cluster", "One."), "add: from cluster");
    let (v, code) = sync_at(&env, &cluster, &["--bundle", bundle]);
    assert_eq!((code, v["pushed"][0]["subject"].as_str()), (Some(0), Some("add: from cluster")), "{v}");

    commit_file(&env, &env.kb(), "general/git/from-laptop.md", &fact("0b1b2c3d4e", "From the laptop", "Two."), "add: from laptop");
    let (v, code) = sync_at(&env, &env.kb(), &["--bundle", bundle]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["pulled"][0]["subject"], "add: from cluster");
    assert_eq!(v["pushed"][0]["subject"], "add: from laptop");
    let (v, _) = sync_at(&env, &cluster, &["--bundle", bundle]);
    assert_eq!(v["pulled"][0]["subject"], "add: from laptop");
    for kb in [env.kb(), cluster.clone()] {
        assert!(kb.join("general/cpp/from-cluster.md").exists() && kb.join("general/git/from-laptop.md").exists(), "{}", kb.display());
    }
    assert!(!file.parent().unwrap().join(".kb.bundle.rkb-tmp").exists());

    let junk = env.dir.path().join("junk.bundle");
    std::fs::write(&junk, "not a bundle").unwrap();
    let (v, code) = sync_at(&env, &env.kb(), &["--bundle", junk.to_str().unwrap()]);
    assert_eq!((v["error"]["code"].as_str(), code), (Some("refused"), Some(1)), "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("junk.bundle"));
    assert_eq!(std::fs::read_to_string(&junk).unwrap(), "not a bundle");

    let o = env.rkb(&["sync", "--remote", "origin", "--bundle", bundle]);
    assert_eq!(o.status.code(), Some(2), "{}", stdout(&o));
}

#[test]
fn import_adds_a_lesson_that_became_a_duplicate() {
    let env = kb_with_topics();
    let dir = import_dir(&env, &[("cmake/a.md", &lesson_with("Presets replace long cmake command lines"))]);
    let (report, _) = env.json(&["import", dir.to_str().unwrap()], "");
    add_ok(&env, "cmake", &lesson_with("Presets replace long cmake command lines"));
    let (v, code) = confirm_on_tty(&env, report["request"].as_str().unwrap(), "import all");
    assert_eq!((v["status"].as_str(), code), (Some("done"), Some(0)), "{v}");
    assert_eq!(v["added"][0]["path"], "general/cmake/presets-replace-long-cmake-command-lines-2.md");
}

fn trusted_confirm(env: &Env, request: &str, choice: &str) -> serde_json::Value {
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("CLAUDECODE", "1");
    let o = rkb_with(c, &["confirm", request, "--choice", choice, "--format", "json"], "");
    serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)))
}

#[test]
fn tier_reports_counts_and_changes_nothing() {
    let env = fixture_kb();
    let before = (commit_count(&env), stdout(&env.git(&["status", "--porcelain"])));
    let (v, code) = env.json(&["tier"], "");
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(
        (v["counts"]["candidate"].as_u64(), v["counts"]["proven"].as_u64(), v["counts"]["archive"].as_u64()),
        (Some(5), Some(0), Some(0)),
        "{v}"
    );
    assert_eq!((commit_count(&env), stdout(&env.git(&["status", "--porcelain"]))), before, "tier changes nothing");

    let (v, _) = env.json(&["used", "7f3a9c2b41", "--worked"], "");
    assert_eq!(v["status"], "recorded", "{v}");
    let (v, _) = env.json(&["tier"], "");
    assert_eq!(v["counts"]["proven"], 1, "{v}");
    assert_eq!(v["proven"][0]["id"], "7f3a9c2b41", "{v}");
    assert_eq!(v["proven"][0]["helpful"], 1.0, "{v}");
}

#[test]
fn search_tier_filter_and_display() {
    let env = fixture_kb();
    let (v, _) = env.json(&["used", "2b81d05e77", "--worked"], "");
    assert_eq!(v["status"], "recorded", "{v}");

    let human = stdout(&env.rkb(&["search", "--no-model", "--all", "--format", "human", "stripe"]));
    assert!(human.contains("proven") && human.contains("candidate"), "the result line shows the tier: {human}");

    let (v, _) = env.json(&["search", "--no-model", "--all", "--tier", "proven", "stripe"], "");
    assert_eq!(
        v["results"].as_array().unwrap().iter().map(|r| r["id"].clone()).collect::<Vec<_>>(),
        [serde_json::json!("2b81d05e77")],
        "{v}"
    );
    assert!(v["results"].as_array().unwrap().iter().all(|r| r["tier"] == "proven"), "{v}");

    let (v, _) = env.json(&["archive", "c04e11a9f3", "--reason", "tier test"], "");
    assert_eq!(v["status"], "written", "{v}");
    let (v, _) = env.json(&["search", "--no-model", "--all", "--tier", "archive", "stripe"], "");
    assert!(v["results"].as_array().unwrap().iter().any(|r| r["id"] == "c04e11a9f3"), "--tier archive reads a tombstone: {v}");
    let (v, _) = env.json(&["search", "--no-model", "--all", "stripe"], "");
    assert!(!v["results"].as_array().unwrap().iter().any(|r| r["id"] == "c04e11a9f3"), "a tombstone stays hidden by default: {v}");
}

#[test]
fn used_demotes_a_lesson_with_bad_signals() {
    let env = fixture_kb();
    let id = "7f3a9c2b41";
    let path = env.kb().join("projects/dftracer/cmake/cmake-needs-hdf5-root.md");
    let (v, _) = env.json(&["used", id, "--worked"], "");
    assert_eq!(v["status"], "recorded", "{v}");
    let (v, _) = env.json(&["used", id, "--irrelevant", "--reason", "not this"], "");
    assert_eq!(v["status"], "recorded", "{v}");
    assert!(v.get("demote").is_none(), "one bad report cannot remove a lesson that worked: {v}");
    assert!(std::fs::read_to_string(&path).unwrap().contains("status: active\n"), "still active");
    let (v, _) = env.json(&["used", id, "--irrelevant", "--reason", "not this either"], "");
    assert_eq!(v["demote"]["status"], "written", "two bad reports and one worked is more harmful: {v}");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("status: archived\n"), "{text}");
    assert!(text.contains("## Why archived\ndemoted: more harmful than helpful"), "{text}");
}

#[test]
fn unarchive_records_the_restoration_and_keeps_the_tier() {
    let env = fixture_kb();
    let id = "c04e11a9f3";
    let (v, _) = env.json(&["used", id, "--worked"], "");
    assert_eq!(v["status"], "recorded", "{v}");
    let (v, _) = env.json(&["archive", id, "--reason", "retired for the test"], "");
    assert_eq!(v["status"], "written", "{v}");
    let (v, _) = env.json(&["tier"], "");
    assert_eq!(v["counts"]["archive"], 1, "{v}");
    let (v, _) = env.json(&["unarchive", id], "");
    assert_eq!(v["status"], "written", "{v}");
    let restored: Vec<_> = usage_records(&env).into_iter().filter(|r| r["id"] == id && r["event"] == "restored").collect();
    assert_eq!(restored.len(), 1, "the restoration is recorded: {restored:?}");
    let (v, _) = env.json(&["tier"], "");
    assert_eq!((v["counts"]["proven"].as_u64(), v["counts"]["archive"].as_u64()), (Some(1), Some(0)), "{v}");
    assert_eq!(v["proven"][0]["id"], id, "a restored lesson with a worked record is proven: {v}");
}

#[test]
fn a_restored_lesson_is_not_demoted_by_its_old_reports() {
    let env = fixture_kb();
    let id = "7f3a9c2b41";
    let path = env.kb().join("projects/dftracer/cmake/cmake-needs-hdf5-root.md");
    let (v, _) = env.json(&["used", id, "--worked"], "");
    assert_eq!(v["status"], "recorded", "{v}");
    let (v, _) = env.json(&["used", id, "--irrelevant", "--reason", "not this"], "");
    assert!(v.get("demote").is_none(), "one bad report is not enough: {v}");
    let (v, _) = env.json(&["used", id, "--irrelevant", "--reason", "not this either"], "");
    assert_eq!(v["demote"]["status"], "written", "{v}");
    assert!(std::fs::read_to_string(&path).unwrap().contains("status: archived\n"));
    let (v, _) = env.json(&["unarchive", id], "");
    assert_eq!(v["status"], "written", "{v}");
    // The counts restart at the restoration, so the old bad reports do not archive it again.
    let (v, _) = env.json(&["used", id, "--worked"], "");
    assert_eq!(v["status"], "recorded", "{v}");
    assert!(v.get("demote").is_none(), "the restored lesson is not archived again: {v}");
    assert!(std::fs::read_to_string(&path).unwrap().contains("status: active\n"), "the lesson stays active");
}

#[test]
fn supersede_archive_unarchive() {
    let env = fixture_kb();
    env.trust_claude();
    let before = commit_count(&env);

    let (v, code) = env.json(&["supersede", "5d2e8a1c90", "--by", "9a4c7e2b10", "--reason", "the regex advice is wrong now"], "");
    assert_eq!((v["status"].as_str(), code), (Some("written"), Some(0)), "no question: {v}");
    assert_eq!(commit_count(&env), before + 1);
    let text = std::fs::read_to_string(env.kb().join("general/cpp/avoid-std-regex-in-hot-loops.md")).unwrap();
    assert!(text.contains("status: superseded") && text.contains("superseded_by: 9a4c7e2b10"), "{text}");
    assert!(
        text.contains("## Why superseded\nthe regex advice is wrong now") && text.contains("(../git/prefer-subject-only-commits.md)"),
        "{text}"
    );
    assert!(head_subject(&env).starts_with("supersede(general/cpp): "));
    let (s, _) = env.json(&["search", "--literal", "std::regex"], "");
    assert!(s["results"].as_array().unwrap().iter().all(|r| r["id"] != "5d2e8a1c90"), "search hides it: {s}");

    let (v, code) = env.json(&["archive", "projects/dftracer", "--reason", "the project moved on"], "");
    assert_eq!((v["status"].as_str(), code), (Some("needs_user"), Some(3)), "{v}");
    let q = v["question"].as_str().unwrap();
    assert!(q.contains("7f3a9c2b41") && q.contains("2b81d05e77"), "{q}");
    let v = trusted_confirm(&env, v["request"].as_str().unwrap(), "archive projects/dftracer");
    assert_eq!(v["status"], "written", "{v}");
    assert_eq!(head_subject(&env), "archive(projects/dftracer): 2 lessons");
    let files = stdout(&env.git(&["show", "--name-only", "--format=", "HEAD"]));
    assert_eq!(files.trim().lines().count(), 2, "{files}");
    let cmake = env.kb().join("projects/dftracer/cmake/cmake-needs-hdf5-root.md");
    assert!(std::fs::read_to_string(&cmake).unwrap().contains("status: archived\n"));
    assert!(std::fs::read_to_string(&cmake).unwrap().contains("## Why archived\nthe project moved on"));
    let (v, _) = env.json(&["changes"], "");
    let rows: Vec<_> = v["changes"].as_array().unwrap().iter().map(|c| (c["kind"].clone(), c["id"].clone(), c["reason"].clone())).collect();
    assert_eq!(
        rows,
        [
            (serde_json::json!("archive"), serde_json::json!(""), serde_json::json!("the project moved on")),
            (serde_json::json!("supersede"), serde_json::json!("5d2e8a1c90"), serde_json::json!("the regex advice is wrong now")),
        ],
        "newest first, fixture commit is not an rkb commit: {v}"
    );
    let (v, code) = env.json(&["archive", "projects/dftracer", "--reason", "again"], "");
    assert_eq!((v["error"]["code"].as_str(), code), (Some("refused"), Some(1)), "{v}");

    let (v, code) = env.json(&["supersede", "c04e11a9f3", "--by", "7f3a9c2b41", "--reason", "x"], "");
    assert_eq!((v["error"]["code"].as_str(), code), (Some("refused"), Some(1)), "the replacement is archived: {v}");

    let (v, code) = env.json(&["unarchive", "7f3a9c2b41"], "");
    assert_eq!((v["status"].as_str(), code), (Some("written"), Some(0)), "no question: {v}");
    assert!(v["notes"][0].as_str().unwrap().contains("back in search"));
    let text = std::fs::read_to_string(&cmake).unwrap();
    assert!(text.contains("status: active\n") && !text.contains("Why archived"), "{text}");
    let (v, _) = env.json(&["unarchive", "5d2e8a1c90"], "");
    assert_eq!(v["error"]["code"], "refused", "a superseded lesson is not archived: {v}");

    let (v, code) = env.json(&["archive", "c04e11a9f3", "--reason", "tuolumne retired"], "");
    assert_eq!((v["status"].as_str(), code), (Some("written"), Some(0)), "one lesson, no question: {v}");
    assert!(head_subject(&env).starts_with("archive(systems/tuolumne/lustre): ") && head_subject(&env).ends_with("[c04e11a9f3]"));
    assert!(env.kb().join("systems/tuolumne/lustre/lustre-needs-striping-for-large-files.md").exists(), "files never move");
    let lint = env.rkb(&["lint"]);
    assert!(lint.status.success(), "{}", stdout(&lint));
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "");
}

#[test]
fn review_lists_candidates_with_reasons() {
    let env = fixture_kb();
    let (v, code) = env.json(&["review"], "");
    assert_eq!((v["candidates"].clone(), code), (serde_json::json!([]), Some(0)), "{v}");
    assert!(stdout(&env.rkb(&["review", "--format", "human"])).contains("Nothing to review"));

    let note = std::fs::read_to_string(env.kb().join("systems/tuolumne/README.md")).unwrap();
    env.write("systems/tuolumne/README.md", &note.replacen("---\n", "---\nretired: true\n", 1));
    let toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    env.write("kb.toml", &format!("{toml}\n[facts.hdf5]\noldest_supported = \"1.12\"\n"));
    let with_when = |id: &str, title: &str, when: &str| fact(id, title, "x").replacen("verified:", &format!("when:\n{when}\nverified:"), 1);
    env.write("general/cpp/old-hdf5.md", &with_when("0c1b2c3d4e", "Old HDF5 trick", "  hdf5: \"1.8:1.10.7\""));
    env.write("general/cpp/open-hdf5.md", &with_when("0d1b2c3d4e", "Open HDF5 range", "  hdf5: \"1.10:\""));
    env.write(
        "projects/dftracer/cmake/pinned.md",
        &with_when("0e1b2c3d4e", "Pinned to a commit", "  project: dftracer\n  since: 0123abcd"),
    );
    env.git(&["add", "-A"]);
    assert!(env.git(&["commit", "-q", "-m", "setup"]).status.success());

    let repo = project_repo(&env, "git@github.com:llnl/dftracer.git");
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.current_dir(&repo);
    assert!(rkb_with(c, &["context"], "").status.success(), "records the checkout");

    let old = (jiff::Zoned::now() - jiff::SignedDuration::from_hours(24 * 120)).strftime("%Y-%m-%dT12:00:00").to_string();
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("GIT_COMMITTER_DATE", &old).env("GIT_AUTHOR_DATE", &old);
    assert!(rkb_with(c, &["flag", "5d2e8a1c90", "--reason", "unsure"], "").status.success());
    let before = stdout(&env.git(&["rev-parse", "HEAD"]));

    let (v, code) = env.json(&["review"], "");
    assert_eq!(code, Some(0), "{v}");
    let by_id = |id: &str| v["candidates"].as_array().unwrap().iter().find(|c| c["id"] == id).cloned();
    let reasons = |id: &str| -> Vec<(String, String)> {
        by_id(id).unwrap_or_else(|| panic!("{id} missing: {v}"))["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["kind"].as_str().unwrap().to_string(), r["detail"].as_str().unwrap().to_string()))
            .collect()
    };
    assert_eq!(reasons("c04e11a9f3"), [("retired".to_string(), "systems/tuolumne".to_string())]);
    assert_eq!(reasons("0c1b2c3d4e"), [("unsupported".to_string(), "hdf5 1.8:1.10.7 < 1.12".to_string())]);
    assert!(by_id("0d1b2c3d4e").is_none() && by_id("7f3a9c2b41").is_none(), "open and current ranges are fine: {v}");
    let stale = reasons("5d2e8a1c90");
    assert!(stale[0].0 == "long_stale" && stale[0].1.starts_with("stale 12"), "{stale:?}");
    assert_eq!(reasons("0e1b2c3d4e"), [("missing_commit".to_string(), "since 0123abcd".to_string())]);
    assert!(by_id("c04e11a9f3").unwrap()["last_worked"].is_null());

    let (v, _) = env.json(&["review", "systems"], "");
    assert_eq!(v["candidates"].as_array().unwrap().len(), 1);
    let (v, code) = env.json(&["review", "nowhere"], "");
    assert_eq!((v["error"]["code"].as_str(), code), (Some("not_found"), Some(1)));
    assert_eq!(stdout(&env.git(&["rev-parse", "HEAD"])), before);
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "");
    assert!(
        !env.dir.path().join("state/rkb/requests").exists()
            || std::fs::read_dir(env.dir.path().join("state/rkb/requests")).unwrap().next().is_none()
    );
}

#[test]
fn review_health_lists_three_kinds() {
    let env = kb_with_topics();
    let (v, code) = env.json(&["review", "--health"], "");
    assert_eq!(code, Some(0));
    assert!(v["candidates"].as_array().unwrap().is_empty(), "no lessons yet: {v}");
    assert!(stdout(&env.rkb(&["review", "--health", "--format", "human"])).contains("Nothing to review"));

    let lesson = |tag: &str, query: &str, title: &str, body: &str| {
        format!("---\ntype: pitfall\nverified_how: ran\ntags:\n  - {tag}\nqueries:\n  - {query}\n---\n\n# {title}\n\n{body}")
    };
    let orphan = add_ok(
        &env,
        "cmake",
        &lesson(
            "harbor",
            "harbor ferry horn codes",
            "Harbor ferry horn codes",
            "## Symptom\nThe ferry horn pattern confuses deckhands.\n\n## Cause\nNo posted code sheet at the harbor gate.\n\n## Fix\nPin the horn code sheet beside the roster.\n\n## Evidence\nDeckhands waved the correct pennants after pinning.\n",
        ),
    );
    let unmatched = add_ok(
        &env,
        "cmake",
        &lesson(
            "quartz",
            "quartz scheduler flags",
            "Quartz scheduler flags",
            "## Symptom\nJobs stall past midnight on quartz.\n\n## Cause\nThe scheduler drops flags after migration.\n\n## Fix\nReapply flags with the quartz console.\n\n## Evidence\nQueues drained before sunrise after reapplying.\n",
        ),
    );
    let healthy = add_ok(
        &env,
        "cpp",
        &lesson(
            "ninja",
            "ninja build speed",
            "Ninja build speed",
            "## Symptom\nLinking drags each afternoon.\n\n## Cause\nThe spool directory fills with stale objects.\n\n## Fix\nEmpty the spool directory before lunch.\n\n## Evidence\nAfternoon links finish promptly now.\n",
        ),
    );
    let old = add_ok(
        &env,
        "cpp",
        &lesson(
            "beacon",
            "harbor ferry horn codes",
            "Beacon battery winter storage",
            "## Symptom\nBeacons dim during January patrols.\n\n## Cause\nCold cells lose charge overnight.\n\n## Fix\nSwap cells at dusk from the warm locker.\n\n## Evidence\nPatrols stayed lit through February.\n",
        ),
    );
    // A fact is never pushed on a failure, so an unmatched query does not list it.
    let fact = add_ok(
        &env,
        "cpp",
        "---\ntype: fact\nverified_how: ran\nqueries:\n  - lighthouse lens polish schedule\n---\n\n# Lens polish schedule\n\n## Statement\nThe lens is polished every spring.\n\n## Evidence\nThe keeper log says so.\n",
    );
    let fact_path = fact["path"].as_str().unwrap().to_string();
    // Links keep every lesson but the orphan connected; backdating stands in for age.
    let (healthy_path, old_path, unmatched_path) = (
        healthy["path"].as_str().unwrap().to_string(),
        old["path"].as_str().unwrap().to_string(),
        unmatched["path"].as_str().unwrap().to_string(),
    );
    let healthy_file = healthy_path.rsplit('/').next().unwrap();
    for (path, link) in [
        (&unmatched_path, format!("../cpp/{healthy_file}")),
        (&old_path, format!("./{healthy_file}")),
        (&fact_path, format!("./{healthy_file}")),
    ] {
        let text = std::fs::read_to_string(env.kb().join(path)).unwrap();
        env.write(path, &format!("{text}\nSee [build speed]({link}).\n"));
    }
    let old_text = std::fs::read_to_string(env.kb().join(&old_path)).unwrap();
    let backdated =
        old_text.lines().map(|l| if l.starts_with("verified:") { "verified: 2025-01-01" } else { l }).collect::<Vec<_>>().join("\n") + "\n";
    env.write(&old_path, &backdated);
    env.git(&["add", "-A"]);
    assert!(env.git(&["commit", "-q", "-m", "add: health fixture"]).status.success());
    // The replay log holds failures that answer every query but one.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    env.write_abs(
        &env.dir.path().join("state/rkb/replays.jsonl"),
        &format!(
            "{{\"time\":{now},\"session\":\"s1\",\"query\":\"what are the harbor ferry horn codes\"}}\n{{\"time\":{now},\"session\":\"s2\",\"query\":\"ninja build speed on this host\"}}\n"
        ),
    );

    let (v, code) = env.json(&["review", "--health"], "");
    assert_eq!(code, Some(0), "{v}");
    let by_id = |id: &str| v["candidates"].as_array().unwrap().iter().find(|c| c["id"] == id).cloned();
    assert!(by_id(fact["id"].as_str().unwrap()).is_none(), "a fact is not judged by failure queries: {v}");
    let kinds = |id: &str| -> Vec<String> {
        by_id(id).unwrap_or_else(|| panic!("{id} missing: {v}"))["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["kind"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(kinds(orphan["id"].as_str().unwrap()), ["orphan"]);
    assert_eq!(kinds(unmatched["id"].as_str().unwrap()), ["unmatched"]);
    assert_eq!(kinds(old["id"].as_str().unwrap()), ["old_verified"]);
    assert!(by_id(healthy["id"].as_str().unwrap()).is_none(), "a linked, matched and recent lesson is healthy: {v}");
    let detail = |id: &str| by_id(id).unwrap()["reasons"].as_array().unwrap()[0]["detail"].as_str().unwrap().to_string();
    assert!(detail(unmatched["id"].as_str().unwrap()).contains("90"), "{v}");
    assert!(detail(old["id"].as_str().unwrap()).contains("2025-01-01"), "{v}");
    let human = stdout(&env.rkb(&["review", "--health", "--format", "human"]));
    assert!(human.contains("orphan") && human.contains("unmatched") && human.contains("old_verified"), "{human}");

    let (v, _) = env.json(&["review", "--health", "general/cpp"], "");
    assert_eq!(v["candidates"].as_array().unwrap().len(), 1, "only the old lesson is under general/cpp: {v}");
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "", "the report changes nothing");
}

#[test]
fn show_why_prints_the_source() {
    let env = Env::new();
    env.init();
    env.write_abs(&env.dir.path().join("config/rkb/config.toml"), "[observer]\npi = \"session\"\n");
    approve_as_claude(&env);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    std::fs::create_dir_all(env.dir.path().join("state/rkb/inbox")).unwrap();
    std::fs::write(
        env.dir.path().join("state/rkb/inbox/3f2a1b4c5d.md"),
        format!(
            "---\nkind: note\ntime: {now}\nsession: s1\nsignatures:\n  - abc123def4\n---\n\ncmake could not find HDF5 on this machine\n"
        ),
    )
    .unwrap();
    std::fs::write(
        env.dir.path().join("state/rkb/signatures.jsonl"),
        format!(
            "{{\"time\":{now},\"sig\":\"abc123def4\",\"session\":\"s1\"}}\n{{\"time\":{now},\"sig\":\"abc123def4\",\"session\":\"s2\"}}\n"
        ),
    )
    .unwrap();
    let fake = env.dir.path().join("fake/pi");
    env.write_abs(
        &fake,
        r#"#!/bin/sh
cat > /dev/null
printf '%s' '{"action":"add","topic":"cmake","type":"pitfall","title":"Cmake needs the prefix","queries":["cmake could not find HDF5"],"sections":{"Symptom":"cmake could not find HDF5 on this machine","Cause":"The prefix was missing.","Fix":"Set the prefix.","Evidence":"Saw it."},"quotes":["cmake could not find HDF5"]}'"#,
    );
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut c = with_pi(&env);
    c.env("RKB_HARNESS", "pi");
    let o = rkb_with(c, &["promote", "--harness", "pi", "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)));
    assert_eq!(v["promoted"], 1, "{v}");
    let (found, _) = env.json(&["search", "cmake could not find HDF5"], "");
    let id = found["results"][0]["id"].as_str().unwrap().to_string();
    let path = found["results"][0]["path"].as_str().unwrap().to_string();
    // The file carries the chain, and show reads it back.
    let file = std::fs::read_to_string(env.kb().join(&path)).unwrap();
    assert!(file.contains("3f2a1b4c5d") && file.contains("quote:") && file.contains("cmake could not find HDF5"), "{file}");
    let today = jiff::Zoned::now().date().to_string();
    let (v, code) = env.json(&["show", &id, "--why"], "");
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["source"]["inbox"], "3f2a1b4c5d", "{v}");
    assert_eq!(v["source"]["quote"], "cmake could not find HDF5", "{v}");
    assert_eq!(v["source"]["session"], "s1", "{v}");
    assert_eq!(v["source"]["harness"], "pi", "{v}");
    assert_eq!(v["source"]["date"], today.as_str(), "{v}");
    let human = stdout(&env.rkb(&["show", &id, "--why", "--format", "human"]));
    assert!(human.contains("inbox: 3f2a1b4c5d") && human.contains("quote: cmake could not find HDF5"), "{human}");
    assert!(human.contains("session: s1") && human.contains("harness: pi") && human.contains(&format!("date: {today}")), "{human}");
    let plain = stdout(&env.rkb(&["show", &id, "--format", "human"]));
    assert!(plain.contains(file.trim()), "plain show still prints the whole file");
    let (v, _) = env.json(&["show", &id], "");
    assert!(v.get("source").is_none(), "no chain without --why: {v}");

    // A hand-written lesson has no item, session or quote.
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env_remove("RKB_HARNESS");
    let o = rkb_with(c, &["add", "--topic", "cmake", "--format", "json"], &lesson_with("A hand-written lesson"));
    assert!(o.status.success(), "{}", stdout(&o));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)));
    let hand = v["id"].as_str().unwrap().to_string();
    let human = stdout(&env.rkb(&["show", &hand, "--why", "--format", "human"]));
    assert!(human.contains("inbox: unknown") && human.contains("quote: unknown"), "{human}");
    assert!(human.contains("session: unknown") && human.contains("harness: unknown"), "{human}");
    assert!(human.contains(&format!("date: {today}")), "{human}");
}

#[test]
fn move_and_rename_rewrite_links() {
    let env = kb_with_topics();
    let a = add_ok(&env, "cpp", &lesson_with("Alpha lesson about linking"));
    let b = add_ok(&env, "git", &lesson_with("Beta lesson about history"));
    let (a_id, a_path, b_path) =
        (a["id"].as_str().unwrap().to_string(), a["path"].as_str().unwrap().to_string(), b["path"].as_str().unwrap().to_string());
    let a_text = std::fs::read_to_string(env.kb().join(&a_path)).unwrap();
    env.write(
        &a_path,
        &format!("{a_text}\nSee [beta](../git/beta-lesson-about-history.md).\n\n![plot](alpha-lesson-about-linking.assets/plot.svg)\n"),
    );
    env.write("general/cpp/alpha-lesson-about-linking.assets/plot.svg", "<svg xmlns=\"http://www.w3.org/2000/svg\"/>\n");
    let b_text = std::fs::read_to_string(env.kb().join(&b_path)).unwrap();
    env.write(&b_path, &format!("{b_text}\nSee [alpha](../cpp/alpha-lesson-about-linking.md#fix).\n"));
    env.git(&["add", "-A"]);
    let o = env.git(&["commit", "-q", "-m", "links"]);
    assert!(o.status.success(), "{}", stdout(&o));
    let before = commit_count(&env);

    let (v, code) = env.json(&["move", &a_id, "general/build"], "");
    assert_eq!((v["status"].as_str(), code), (Some("written"), Some(0)), "{v}");
    assert_eq!(v["path"], "general/build/alpha-lesson-about-linking.md");
    let notes: Vec<&str> = v["notes"].as_array().unwrap().iter().map(|n| n.as_str().unwrap()).collect();
    assert!(
        notes.contains(&"created the folder general/build") && notes.contains(&format!("updated links in {b_path}").as_str()),
        "{notes:?}"
    );
    assert_eq!(commit_count(&env), before + 1, "one commit");
    assert!(head_subject(&env).starts_with("move(general/build): Alpha lesson about linking"));
    assert!(!env.kb().join(&a_path).exists() && env.kb().join("general/build/alpha-lesson-about-linking.assets/plot.svg").exists());
    let b_now = std::fs::read_to_string(env.kb().join(&b_path)).unwrap();
    assert!(b_now.contains("](../build/alpha-lesson-about-linking.md#fix)"), "{b_now}");
    let a_now = std::fs::read_to_string(env.kb().join("general/build/alpha-lesson-about-linking.md")).unwrap();
    assert!(
        a_now.contains("](../git/beta-lesson-about-history.md)") && a_now.contains("](alpha-lesson-about-linking.assets/plot.svg)"),
        "{a_now}"
    );
    let lint = env.rkb(&["lint"]);
    assert!(lint.status.success(), "{}", stdout(&lint));
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "");

    let (v, code) = env.json(&["rename", &a_id, "linking-alpha"], "");
    assert_eq!((v["status"].as_str(), code), (Some("written"), Some(0)), "{v}");
    assert!(head_subject(&env).starts_with("rename(general/build): "));
    let a_now = std::fs::read_to_string(env.kb().join("general/build/linking-alpha.md")).unwrap();
    assert!(a_now.contains("](linking-alpha.assets/plot.svg)"), "{a_now}");
    assert!(std::fs::read_to_string(env.kb().join(&b_path)).unwrap().contains("](../build/linking-alpha.md#fix)"));
    assert!(env.rkb(&["lint"]).status.success());

    let (v, code) = env.json(&["rename", b["id"].as_str().unwrap(), "../escape"], "");
    assert_eq!((v["error"]["code"].as_str(), code), (Some("refused"), Some(1)), "{v}");
    assert_eq!(add_ok(&env, "git", &lesson_with("Taken"))["path"], "general/git/taken.md");
    let head = stdout(&env.git(&["rev-parse", "HEAD"]));
    let (v, code) = env.json(&["rename", b["id"].as_str().unwrap(), "taken"], "");
    assert_eq!((v["error"]["code"].as_str(), code), (Some("refused"), Some(1)), "{v}");
    assert_eq!(stdout(&env.git(&["rev-parse", "HEAD"])), head, "nothing changed");

    let proj = add_ok(&env, "cmake", &PITFALL.replace("verified_how: ran", "verified_how: ran\nwhen:\n  project: dftracer"));
    let proj_path = proj["path"].as_str().unwrap().to_string();
    let head = stdout(&env.git(&["rev-parse", "HEAD"]));
    let (v, code) = env.json(&["move", proj["id"].as_str().unwrap(), "general/cmake"], "");
    assert_eq!((v["error"]["code"].as_str(), code), (Some("invalid_lesson"), Some(1)), "wrong scope: {v}");
    assert!(v.to_string().contains("layout/scope"), "{v}");
    assert_eq!(stdout(&env.git(&["rev-parse", "HEAD"])), head, "nothing changed");
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "");
    assert!(env.kb().join(&proj_path).exists());

    let (v, code) = env.json(&["move", &a_id, "projects/dftracer/cmake"], "");
    assert_eq!((v["status"].as_str(), code), (Some("written"), Some(0)), "narrowing into a project is fine: {v}");
    assert!(env.rkb(&["lint"]).status.success());
}

fn with_check(title: &str, script: &str) -> String {
    format!("{}\n## Check\n```bash\n{script}\n```\n", lesson_with(title).trim_end())
}

fn rkb_exit(env: &Env, args: &[&str], exit_file: &Path) -> (serde_json::Value, Option<i32>) {
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_TEST_EXIT", exit_file);
    let mut all = args.to_vec();
    all.extend(["--format", "json"]);
    let o = rkb_with(c, &all, "");
    (serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o))), o.status.code())
}

fn front(env: &Env, path: &str) -> String {
    std::fs::read_to_string(env.kb().join(path)).unwrap()
}

#[test]
fn approve_and_verify_checks() {
    let env = kb_with_topics();
    let exit = env.dir.path().join("exit-code");
    let script = "exit \"$(cat \"$RKB_TEST_EXIT\")\"";
    let a = add_ok(&env, "cmake", &with_check("Checked lesson one", script));
    let (a_id, a_path) = (a["id"].as_str().unwrap().to_string(), a["path"].as_str().unwrap().to_string());

    let plain = add_ok(&env, "cpp", &lesson_with("No script here"));
    let (v, code) = env.json(&["approve", plain["id"].as_str().unwrap(), "check"], "");
    assert_eq!((v["error"]["code"].as_str(), code), (Some("refused"), Some(1)), "{v}");

    let (v, code) = env.json(&["approve", &a_id, "check"], "");
    assert_eq!((v["status"].as_str(), code), (Some("needs_user"), Some(3)), "{v}");
    let q = v["question"].as_str().unwrap();
    assert!(q.contains(script) && q.contains("sha256 ") && q.contains("host:"), "{q}");
    assert_eq!(v["options"][0], format!("approve {a_id} check"));
    let (c, _) = confirm_on_tty(&env, v["request"].as_str().unwrap(), &format!("approve {a_id} check"));
    assert_eq!(c["status"], "done", "{c}");
    assert!(std::fs::read_to_string(env.dir.path().join("config/rkb/approvals.toml")).unwrap().contains("sha256"));
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "");
    let (v, _) = env.json(&["approve", &a_id, "check"], "");
    assert!(v["message"].as_str().unwrap().contains("already approved"), "{v}");

    let before = commit_count(&env);
    std::fs::write(&exit, "2").unwrap();
    let (v, _) = rkb_exit(&env, &["verify", &a_id], &exit);
    assert!(v["message"].as_str().unwrap().contains("unknown"), "{v}");
    assert_eq!(commit_count(&env), before, "unknown changes nothing");

    std::fs::write(&exit, "1").unwrap();
    let (v, code) = rkb_exit(&env, &["verify", &a_id], &exit);
    assert_eq!((v["status"].as_str(), code), (Some("written"), Some(0)), "{v}");
    assert!(head_subject(&env).starts_with("verify(general/cmake): "));
    let text = front(&env, &a_path);
    assert!(text.contains("status: stale") && text.contains("stale_reason: check failed on host:") && text.contains("(exit 1)"), "{text}");
    let checks = std::fs::read_to_string(env.dir.path().join("state/rkb/checks.jsonl")).unwrap();
    assert!(checks.lines().last().unwrap().contains("\"result\":\"fail\""), "{checks}");

    std::fs::write(&exit, "0").unwrap();
    let (v, _) = rkb_exit(&env, &["verify", &a_id], &exit);
    assert_eq!(v["status"], "written", "{v}");
    let text = front(&env, &a_path);
    let today = jiff::Zoned::now().date().to_string();
    assert!(
        text.contains("status: active") && text.contains("verified_how: checked") && text.contains(&format!("verified: {today}")),
        "{text}"
    );
    assert!(!text.contains("stale_reason"));
    let lint = env.rkb(&["lint"]);
    assert!(lint.status.success(), "{}", stdout(&lint));

    let b = add_ok(&env, "cmake", &with_check("Second checked lesson", "test -n \"$RKB_TEST_EXIT\""));
    let b_id = b["id"].as_str().unwrap().to_string();
    let (v, code) = rkb_exit(&env, &["verify", &b_id], &exit);
    assert_eq!((v["status"].as_str(), code), (Some("needs_user"), Some(3)), "{v}");
    assert!(v["question"].as_str().unwrap().starts_with("Approve and run"));
    let tty = env.dir.path().join("tty");
    std::fs::write(&tty, "yes\n").unwrap();
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_TTY", &tty).env("RKB_TEST_EXIT", &exit);
    let o = rkb_with(c, &["confirm", v["request"].as_str().unwrap(), "--choice", &format!("approve {b_id} check"), "--format", "json"], "");
    assert!(stdout(&o).contains("pass"), "approved and ran: {}", stdout(&o));
    assert!(std::fs::read_to_string(env.dir.path().join("state/rkb/checks.jsonl")).unwrap().contains(&b_id));

    let c_lesson = add_ok(&env, "cpp", &with_check("Never approved lesson", "exit 0"));
    let (v, _) = env.json(&["flag", &a_id, "--reason", "manual doubt"], "");
    assert_eq!(v["status"], "written");
    std::fs::write(&exit, "0").unwrap();
    let (v, code) = rkb_exit(&env, &["verify", "--auto"], &exit);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["passed"], serde_json::json!([a_id.clone(), b_id.clone()]), "{v}");
    assert_eq!(v["skipped"], serde_json::json!([c_lesson["id"].as_str().unwrap()]));
    assert!(front(&env, &a_path).contains("status: stale"), "--auto never reactivates");
    std::fs::write(&exit, "1").unwrap();
    let (v, _) = rkb_exit(&env, &["verify", "--auto"], &exit);
    assert_eq!(v["failed"], serde_json::json!([a_id.clone()]), "{v}");
    assert!(front(&env, b["path"].as_str().unwrap()).contains("status: active"), "b's script passes whatever the file says");
    assert!(
        !env.dir.path().join("state/rkb/requests").exists()
            || std::fs::read_dir(env.dir.path().join("state/rkb/requests")).unwrap().next().is_none(),
        "--auto never asks"
    );
}

fn with_when(title: &str, when: &str) -> String {
    lesson_with(title).replacen("verified_how:", &format!("when:\n{when}\nverified_how:"), 1)
}

fn with_probe(title: &str, when: Option<&str>, script: &str) -> String {
    let base = match when {
        Some(w) => with_when(title, w),
        None => lesson_with(title),
    };
    format!("{}\n## Probe\n```bash\n{script}\n```\n", base.trim_end())
}

fn search_ids(v: &serde_json::Value) -> Vec<String> {
    v["results"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect()
}

#[test]
fn fact_commands_decide_applies_and_are_cached() {
    let env = kb_with_topics();
    let count = env.dir.path().join("fact-runs");
    let old = add_ok(&env, "cmake", &with_when("Old HDF5 API trick", "  hdf5: \":1.12\""));
    let old_id = old["id"].as_str().unwrap().to_string();
    let toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    env.write("kb.toml", &format!("{toml}\n[facts.hdf5]\ncmd = \"echo run >> \\\"$RKB_TEST_COUNT\\\"; echo 1.14.3\"\n"));
    let search = |extra: &[(&str, &str)]| {
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.env("RKB_TEST_COUNT", &count);
        for (k, v) in extra {
            c.env(k, v);
        }
        let o = rkb_with(c, &["search", "Old HDF5 API trick", "--format", "json"], "");
        serde_json::from_slice::<serde_json::Value>(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)))
    };
    let runs = || std::fs::read_to_string(&count).map(|t| t.lines().count()).unwrap_or(0);

    let v = search(&[]);
    assert_eq!(search_ids(&v), std::slice::from_ref(&old_id), "unapproved: the key stays unknown: {v}");
    assert_eq!(v["results"][0]["applies"], "unknown");
    assert_eq!(runs(), 0, "an unapproved command never runs");

    let (v, code) = env.json(&["approve", "facts.hdf5"], "");
    assert_eq!((v["status"].as_str(), code), (Some("needs_user"), Some(3)), "{v}");
    assert!(v["question"].as_str().unwrap().contains("echo 1.14.3"));
    let (c, _) = confirm_on_tty(&env, v["request"].as_str().unwrap(), "approve facts.hdf5");
    assert_eq!(c["status"], "done", "{c}");

    let v = search(&[]);
    assert!(search_ids(&v).is_empty(), "{v}");
    assert_eq!((v["hidden"].as_u64(), v["hidden_by"]["hdf5"].as_u64()), (Some(1), Some(1)), "{v}");
    assert_eq!(runs(), 1);
    search(&[]);
    assert_eq!(runs(), 1, "cache hit");
    search(&[("LOADEDMODULES", "hdf5/1.14")]);
    assert_eq!(runs(), 2, "other modules, other cache entry");
    let hook_before = runs();
    let payload = serde_json::json!({ "session_id": "f1", "cwd": env.kb(), "tool_name": "Bash", "tool_input": { "command": "h5cc x" }, "error": "Exit code 1\nerror: Old HDF5 API trick\n" });
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_TEST_COUNT", &count).env("LOADEDMODULES", "brand/new");
    rkb_with(c, &["hook", "tool-failed"], &payload.to_string());
    assert_eq!(runs(), hook_before, "hooks never run fact commands");
}

#[test]
fn probes_decide_applies_in_search() {
    let env = kb_with_topics();
    let marker = env.dir.path().join("probe-ran");
    let toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    env.write("kb.toml", &format!("{toml}\n[probe]\nbudget_s = 2\n"));
    let fail = add_ok(&env, "cmake", &with_probe("Probe demo alpha", None, "exit 1"));
    let pass = add_ok(&env, "cmake", &with_probe("Probe demo beta", Some("  gpu: a100"), "exit 0"));
    let slow = add_ok(&env, "cmake", &with_probe("Probe demo gamma", None, "sleep 30"));
    let marked = add_ok(&env, "cpp", &with_probe("Marker probe lesson", None, "touch \"$RKB_TEST_MARK\"\nexit 0"));
    for l in [&fail, &pass, &slow, &marked] {
        let id = l["id"].as_str().unwrap();
        let (v, _) = env.json(&["approve", id, "probe"], "");
        let (c, _) = confirm_on_tty(&env, v["request"].as_str().unwrap(), &format!("approve {id} probe"));
        assert_eq!(c["status"], "done", "{c}");
    }

    let payload = serde_json::json!({ "session_id": "p1", "cwd": env.kb(), "tool_name": "Bash", "tool_input": { "command": "make" }, "error": "Exit code 1\nerror: Marker probe lesson\n" });
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_TEST_MARK", &marker);
    rkb_with(c, &["hook", "tool-failed"], &payload.to_string());
    assert!(!marker.exists(), "hooks never run probes");

    let started = std::time::Instant::now();
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_TEST_MARK", &marker);
    let o = rkb_with(c, &["search", "probe demo", "--format", "json"], "");
    let elapsed = started.elapsed();
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)));
    assert!(elapsed < std::time::Duration::from_secs(6), "the budget caps probes: {elapsed:?}");
    let ids = search_ids(&v);
    assert!(!ids.contains(&fail["id"].as_str().unwrap().to_string()), "{v}");
    assert_eq!(v["hidden_by"]["probe"], 1, "{v}");
    let result = |id: &str| v["results"].as_array().unwrap().iter().find(|r| r["id"] == id).cloned().unwrap_or_else(|| panic!("{id}: {v}"));
    assert_eq!(result(pass["id"].as_str().unwrap())["applies"], "yes", "a passing probe beats unknown");
    assert_eq!(result(slow["id"].as_str().unwrap())["applies"], "yes", "no when and a cut probe: applies stays as it was");

    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_TEST_MARK", &marker);
    rkb_with(c, &["search", "Marker probe lesson", "--format", "json"], "");
    assert!(marker.exists(), "search runs approved probes");
}

/// The search fixture with two git lessons labeled public, committed, and a template in `site/`.
fn publish_kb() -> Env {
    let env = search_kb();
    for f in ["general/git/rebase-with-local-edits-using-autostash.md", "general/git/squash-fixups-with-autosquash.md"] {
        let p = env.kb().join(f);
        let text = std::fs::read_to_string(&p).unwrap().replacen("\n---\n", "\nlabels:\n  sensitivity: public\n---\n", 1);
        std::fs::write(p, text).unwrap();
    }
    let tpl = env.kb().join("site");
    std::fs::create_dir_all(tpl.join("templates")).unwrap();
    std::fs::create_dir_all(tpl.join("static")).unwrap();
    std::fs::write(tpl.join("config.lua"), CONFIG_LUA).unwrap();
    std::fs::write(tpl.join("templates/index.html"), "<html><body>{{ title }}</body></html>\n").unwrap();
    std::fs::write(tpl.join("static/site.css"), "body { color: red }\n").unwrap();
    env.git(&["add", "-A"]);
    assert!(env.git(&["-c", "user.name=T", "-c", "user.email=t@example.org", "commit", "-qm", "publish"]).status.success());
    env.trust_claude();
    env
}

/// The rs-web template the publish tests build from. rkb does not run rs-web, so it only has to sit there.
const CONFIG_LUA: &str = r#"local rs = require("rs-web")
return {
  site = { title = "t", description = "", base_url = "", author = "" },
  build = { output_dir = "dist" },
  pages = function() return { { path = "/", template = "index.html", title = "t" } } end,
}
"#;

const PASSWORD: &str = "correct-horse-battery-staple";

fn publish(env: &Env, args: &[&str], vars: &[(&str, &str)]) -> (serde_json::Value, Option<i32>) {
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("CLAUDECODE", "1").env_remove("SITE_PASSWORD");
    for (k, v) in vars {
        c.env(k, v);
    }
    let mut all = args.to_vec();
    all.extend(["--format", "json"]);
    let o = rkb_with(c, &all, "");
    (serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o))), o.status.code())
}

/// Publishes once, confirming the first publication.
fn published_ok(env: &Env, dir: &str) -> serde_json::Value {
    let (v, code) = publish(env, &["publish", dir], &[]);
    assert_eq!((v["status"].as_str(), code), (Some("needs_user"), Some(3)), "{v}");
    let (v, code) = publish(env, &["confirm", v["request"].as_str().unwrap(), "--choice", "publish"], &[]);
    assert_eq!((v["status"].as_str(), code), (Some("published"), Some(0)), "{v}");
    v
}

/// Adds `projects/rait` with a note that has no labels and one unlabeled lesson, committed.
fn with_rait(env: &Env) {
    env.write("projects/rait/README.md", "---\nremotes:\n  - github.com/example/rait\n---\n\n# rait\n");
    let src = std::fs::read_to_string(env.kb().join("general/git/shallow-clones-break-git-describe.md")).unwrap();
    env.write("projects/rait/git/shallow-clones.md", &src.replace("id: 1a00000004", "id: 1a000000aa"));
    env.git(&["add", "-A"]);
    assert!(env.git(&["-c", "user.name=T", "-c", "user.email=t@example.org", "commit", "-qm", "rait"]).status.success());
}

#[test]
fn publish_writes() {
    let env = publish_kb();
    let out = env.dir.path().join("out");
    let out_s = out.to_str().unwrap();

    let (v, code) = publish(&env, &["publish", out_s], &[]);
    assert_eq!((v["status"].as_str(), code), (Some("needs_user"), Some(3)), "{v}");
    let q = v["question"].as_str().unwrap();
    assert!(q.contains("Publish 2 lessons") && q.contains("1a00000003") && q.contains("1a00000005"), "{q}");
    assert!(!out.exists(), "nothing is written before the answer");

    let v = published_ok(&env, out_s);
    assert_eq!(v["lessons"], 2, "{v}");
    assert_eq!(v["out"].as_str().unwrap(), out_s, "{v}");
    // The knowledge base's template, as it is in the repository.
    assert_eq!(std::fs::read_to_string(out.join("config.lua")).unwrap(), CONFIG_LUA);
    assert_eq!(std::fs::read_to_string(out.join("templates/index.html")).unwrap(), "<html><body>{{ title }}</body></html>\n");
    assert_eq!(std::fs::read_to_string(out.join("static/site.css")).unwrap(), "body { color: red }\n");
    // The allowed lessons and their paths, and the index rs-web reads.
    assert!(out.join("lessons/general/git/squash-fixups-with-autosquash.md").is_file());
    assert!(!out.join("lessons/general/git/shallow-clones-break-git-describe.md").exists());
    assert!(!out.join("protected").exists());
    let index: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out.join("site.json")).unwrap()).unwrap();
    let got: Vec<&str> = index["lessons"].as_array().unwrap().iter().map(|l| l["id"].as_str().unwrap()).collect();
    assert_eq!(got, ["1a00000003", "1a00000005"], "{index}");
    let css = index["assets"]["site_css"].as_str().unwrap();
    let hash = css.strip_prefix("/static/v/").and_then(|r| r.strip_suffix("/site.css")).unwrap_or_default();
    assert_eq!(hash.len(), 12, "{css}");
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()), "{css}");

    // An earlier rs-web output in the folder is left alone, and the second publish changes nothing.
    std::fs::create_dir_all(out.join("dist")).unwrap();
    std::fs::write(out.join("dist/index.html"), "built").unwrap();
    let (v, code) = publish(&env, &["publish", out_s], &[]);
    assert_eq!((v["status"].as_str(), code), (Some("published"), Some(0)), "{v}");
    assert_eq!(std::fs::read_to_string(out.join("dist/index.html")).unwrap(), "built");

    let p = env.kb().join("general/git/squash-fixups-with-autosquash.md");
    std::fs::write(&p, std::fs::read_to_string(&p).unwrap().replace("sensitivity: public", "sensitivity: internal")).unwrap();
    let (v, code) = publish(&env, &["publish", out_s], &[]);
    assert_eq!(code, Some(0), "{v}");
    let notes = v["notes"].to_string();
    assert!(notes.contains("no longer published") && notes.contains("1a00000005"), "{v}");
    assert!(notes.contains("internal lessons were left out; set SITE_PASSWORD"), "{v}");
    assert!(!out.join("lessons/general/git/squash-fixups-with-autosquash.md").exists(), "the lesson is staged out again");
}

#[test]
fn publish_refuses_a_folder_it_did_not_write() {
    let env = publish_kb();
    let out = env.dir.path().join("mine");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("notes.txt"), "keep").unwrap();
    let (v, code) = publish(&env, &["publish", out.to_str().unwrap()], &[]);
    assert_eq!(code, Some(1), "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("no site.json"), "{v}");
    assert_eq!(std::fs::read_to_string(out.join("notes.txt")).unwrap(), "keep");
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 1, "nothing was written");
}

#[test]
fn publish_names_folders_that_are_internal_by_default() {
    let env = publish_kb();
    with_rait(&env);
    let out = env.dir.path().join("out");
    let v = published_ok(&env, out.to_str().unwrap());
    let notes = v["notes"].to_string();
    assert!(
        notes.contains("1 lesson(s) in projects/rait are internal by default")
            && notes.contains("run `rkb label projects/rait sensitivity=public`"),
        "{v}"
    );
}

#[test]
fn publish_needs_a_template() {
    let env = publish_kb();
    let out = env.dir.path().join("out");
    published_ok(&env, out.to_str().unwrap());
    let site = env.kb().join("site");
    std::fs::remove_file(site.join("config.lua")).unwrap();
    std::fs::remove_dir_all(site.join("templates")).unwrap();
    let other = env.dir.path().join("other");
    let (v, code) = publish(&env, &["publish", other.to_str().unwrap()], &[]);
    assert_eq!((v["error"]["code"].as_str(), code), (Some("not_found"), Some(1)), "{v}");
    assert!(v["error"]["fix"].as_str().unwrap().contains("site/config.lua"), "{v}");
    assert!(!other.exists(), "no template, nothing written: {v}");
}

#[test]
fn publish_refuses_links_to_private_lessons() {
    let env = publish_kb();
    let p = env.kb().join("general/git/rebase-with-local-edits-using-autostash.md");
    let text = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, text.replacen("\n## Steps", "\nSee [describe](shallow-clones-break-git-describe.md).\n\n## Steps", 1)).unwrap();
    let (v, code) = publish(&env, &["publish", "out"], &[]);
    assert_eq!(code, Some(1), "{v}");
    let m = v["error"]["message"].as_str().unwrap();
    assert!(m.contains("rebase-with-local-edits-using-autostash.md -> general/git/shallow-clones-break-git-describe.md"), "{m}");
}

/// A 1x1 PNG whose `eXIf` chunk holds a GPS position.
fn png_with_gps() -> Vec<u8> {
    let chunk = |kind: &[u8], data: &[u8]| {
        let mut c = (data.len() as u32).to_be_bytes().to_vec();
        c.extend_from_slice(kind);
        c.extend_from_slice(data);
        c.extend_from_slice(&[0; 4]);
        c
    };
    let mut p = b"\x89PNG\r\n\x1a\n".to_vec();
    p.extend(chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 0, 0, 0, 0]));
    p.extend(chunk(b"eXIf", b"MM\0*GPSLatitude 41.8781"));
    p.extend(chunk(b"IDAT", &[0x78, 0x9c, 0x63, 0x60, 0, 0, 0, 2, 0, 1]));
    p.extend(chunk(b"IEND", &[]));
    p
}

/// Links `layout.png` (with GPS EXIF) from the public autostash lesson and `diagram.svg` from the
/// internal shallow-clones lesson.
fn publish_kb_with_images() -> Env {
    let env = publish_kb();
    let dir = env.kb().join("general/git");
    let link = |file: &str, img: &str| {
        let p = dir.join(format!("{file}.md"));
        let text = std::fs::read_to_string(&p).unwrap().replacen("\n## ", &format!("\n![img]({file}.assets/{img})\n\n## "), 1);
        std::fs::write(p, text).unwrap();
    };
    std::fs::create_dir_all(dir.join("rebase-with-local-edits-using-autostash.assets")).unwrap();
    std::fs::write(dir.join("rebase-with-local-edits-using-autostash.assets/layout.png"), png_with_gps()).unwrap();
    link("rebase-with-local-edits-using-autostash", "layout.png");
    std::fs::create_dir_all(dir.join("shallow-clones-break-git-describe.assets")).unwrap();
    std::fs::write(
        dir.join("shallow-clones-break-git-describe.assets/diagram.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><text>describe</text></svg>",
    )
    .unwrap();
    link("shallow-clones-break-git-describe", "diagram.svg");
    env
}

#[test]
fn publish_stages_images_without_metadata() {
    let env = publish_kb_with_images();
    let out = env.dir.path().join("out");
    let out_s = out.to_str().unwrap();
    let pw = [("SITE_PASSWORD", PASSWORD)];
    let (v, _) = publish(&env, &["publish", out_s], &pw);
    let (v, code) = publish(&env, &["confirm", v["request"].as_str().unwrap(), "--choice", "publish"], &pw);
    assert_eq!(code, Some(0), "{v}");
    let png = std::fs::read(out.join("lessons/general/git/rebase-with-local-edits-using-autostash.assets/layout.png")).unwrap();
    assert!(png.starts_with(b"\x89PNG") && !String::from_utf8_lossy(&png).contains("GPS"), "the staged image holds no EXIF");
    let svg = out.join("protected/general/git/shallow-clones-break-git-describe.assets/diagram.svg");
    let b64 = std::fs::read_to_string(svg.with_extension("svg.b64")).unwrap();
    assert!(b64.starts_with("PHN2Zy"), "the protected image is staged as base64 to embed: {b64}");

    std::fs::write(env.kb().join("general/git/rebase-with-local-edits-using-autostash.assets/layout.png"), b"\x89PNG\r\n\x1a\nbroken")
        .unwrap();
    let (v, code) = publish(&env, &["publish", out_s], &pw);
    assert_eq!(code, Some(1), "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("layout.png: truncated PNG"), "{v}");
}

/// The output checks read the `dist/` folder in `dir` against the `site.json` publish left there.
#[test]
fn publish_check() {
    let env = publish_kb();
    let out = env.dir.path().join("out");
    let out_s = out.to_str().unwrap();
    published_ok(&env, out_s);

    let check = |env: &Env| publish(env, &["publish", "--check", out_s], &[]);
    let (v, code) = check(&env);
    assert_eq!((v["error"]["code"].as_str(), code), (Some("not_found"), Some(1)), "{v}");
    assert!(v["error"]["fix"].as_str().unwrap().contains("rs-web build"), "{v}");
    assert!(!out.join("dist").exists());

    let dist = out.join("dist");
    std::fs::create_dir_all(&dist).unwrap();
    std::fs::write(dist.join("index.html"), "<html><body>Rebase with local edits using autostash</body></html>\n").unwrap();
    let (v, code) = check(&env);
    assert_eq!((v["status"].as_str(), code), (Some("ok"), Some(0)), "{v}");
    assert_eq!(v["findings"], 0, "{v}");

    let home = format!("{}/secret", env.dir.path().display());
    std::fs::write(dist.join("index.html"), format!("<html><body>{home}</body></html>\n")).unwrap();
    let (v, code) = check(&env);
    assert_eq!(code, Some(1), "{v}");
    let m = v["error"]["message"].as_str().unwrap();
    assert!(m.contains("index.html: leak/path"), "{m}");

    std::fs::write(dist.join("index.html"), "x").unwrap();
    std::fs::create_dir_all(dist.join("lessons/1a00000004")).unwrap();
    std::fs::write(dist.join("lessons/1a00000004/index.html"), "x").unwrap();
    std::fs::create_dir_all(dist.join("protected/1a00000099")).unwrap();
    std::fs::write(dist.join("protected/1a00000099/index.html"), "x").unwrap();
    let (v, code) = check(&env);
    assert_eq!(code, Some(1), "{v}");
    let m = v["error"]["message"].as_str().unwrap();
    for want in [
        "a public lesson page for `1a00000004`, which is not published that way",
        "a protected lesson page for `1a00000099`, which is not published that way",
    ] {
        assert!(m.contains(want), "{m}");
    }
    std::fs::remove_dir_all(dist.join("lessons")).unwrap();
    std::fs::remove_dir_all(dist.join("protected")).unwrap();

    // The vendored Argon2 build ships as it is, so its minified tokens are not findings.
    std::fs::create_dir_all(dist.join("static")).unwrap();
    std::fs::write(dist.join("static/argon2.umd.min.js"), "var a=`function(){var r=1e3}`;").unwrap();
    let (v, code) = check(&env);
    assert_eq!(code, Some(0), "{v}");

    // Text inside ciphertext is not plain text; a protected lesson's own title is.
    let pw = [("SITE_PASSWORD", PASSWORD)];
    let (v, _) = publish(&env, &["publish", out_s], &pw);
    let (v, code) = publish(&env, &["confirm", v["request"].as_str().unwrap(), "--choice", "publish"], &pw);
    assert_eq!(code, Some(0), "{v}");
    assert!(out.join("protected/general/git/shallow-clones-break-git-describe.md").is_file());
    let prot = out.join("protected/general/git");
    std::fs::create_dir_all(prot.join("shallow-clones-break-git-describe.assets")).unwrap();
    std::fs::write(prot.join("shallow-clones-break-git-describe.assets/diagram.svg"), b"SVGDATA").unwrap();
    std::fs::create_dir_all(dist.join("protected/1a00000004")).unwrap();
    std::fs::write(dist.join("protected/1a00000004/index.html"), r#"<div data-encrypted="Shallow clones break git describe"></div>"#)
        .unwrap();
    let (v, code) = check(&env);
    assert_eq!(code, Some(0), "text inside ciphertext is not plain text: {v}");

    // The site index shows protected lessons' tags and queries, so they are public, not findings.
    let index: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out.join("site.json")).unwrap()).unwrap();
    let row = index["protected"].as_array().unwrap().iter().find(|l| l["id"] == "1a00000004").unwrap();
    let words: Vec<&str> = ["tags", "queries"].iter().flat_map(|k| row[*k].as_array().unwrap().iter().filter_map(|w| w.as_str())).collect();
    assert!(!words.is_empty(), "{row}");
    std::fs::write(dist.join("index.html"), format!("<html><body>{}</body></html>\n", words.join(" | "))).unwrap();
    let (v, code) = check(&env);
    assert_eq!(code, Some(0), "public metadata of a protected lesson is not a finding: {v}");
    std::fs::write(dist.join("index.html"), "x").unwrap();

    std::fs::write(dist.join("protected/1a00000004/index.html"), "Shallow clones break git describe").unwrap();
    let (v, code) = check(&env);
    assert_eq!(code, Some(1), "{v}");
    let m = v["error"]["message"].as_str().unwrap();
    assert!(m.contains("protected/1a00000004/index.html: shows protected lesson `1a00000004` in plain text"), "{m}");
    assert!(!m.contains("Shallow clones"), "the finding names the lesson by id only: {m}");

    std::fs::write(dist.join("protected/1a00000004/index.html"), "x").unwrap();
    std::fs::create_dir_all(dist.join("general/git/shallow-clones-break-git-describe")).unwrap();
    std::fs::write(dist.join("general/git/shallow-clones-break-git-describe/index.html"), "x").unwrap();
    std::fs::write(dist.join("plain.svg"), b"SVGDATA").unwrap();
    let (v, code) = check(&env);
    assert_eq!(code, Some(1), "{v}");
    let m = v["error"]["message"].as_str().unwrap();
    assert!(m.contains("a page at the path of protected lesson `1a00000004`"), "{m}");
    assert!(m.contains("plain.svg: an image of protected lesson `1a00000004` as a plain file"), "{m}");
}

/// The first-publication question, the `--no-ask` fallback for CI, and the doctor's pending approvals.
#[test]
fn publish_ask() {
    let env = publish_kb();
    let out = env.dir.path().join("out");
    let out_s = out.to_str().unwrap();

    let (v, code) = publish(&env, &["publish", "--no-ask", out_s], &[]);
    assert_eq!(code, Some(1), "{v}");
    let m = v["error"]["message"].as_str().unwrap();
    assert!(m.contains("2 lessons wait") && m.contains("has no site/published.json"), "{m}");
    assert!(v["error"]["fix"].as_str().unwrap().contains("rkb publish` on your machine"), "{v}");
    assert!(!out.exists(), "CI without a record publishes nothing");
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "", "the knowledge base is unchanged");

    let (v, code) = publish(&env, &["publish", out_s], &[]);
    assert_eq!((v["status"].as_str(), code), (Some("needs_user"), Some(3)), "{v}");
    let q = v["question"].as_str().unwrap();
    assert!(q.contains("Publish 2 lessons") && q.contains("In the clear:"), "{q}");
    assert!(q.contains("1a00000003 Rebase with local edits using autostash") && q.contains("1a00000005 Squash fixups"), "{q}");
    assert!(!out.exists(), "nothing is written before the answer");
    let (v, code) = publish(&env, &["confirm", v["request"].as_str().unwrap(), "--choice", "cancel"], &[]);
    assert_eq!((v["status"].as_str(), code), (Some("cancelled"), Some(0)), "{v}");
    assert!(!out.exists(), "cancel writes nothing");

    let before = commit_count(&env);
    published_ok(&env, out_s);
    assert_eq!(commit_count(&env), before + 1, "the record is one commit");
    assert_eq!(head_subject(&env), "site: publish 2 lessons");
    let text = std::fs::read_to_string(env.kb().join("site/published.json")).unwrap();
    assert_eq!(text, "{\n  \"clear\": [\n    \"1a00000003\",\n    \"1a00000005\"\n  ],\n  \"encrypted\": {}\n}\n");
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "");

    // A second publish on the same machine asks nothing and commits nothing.
    let (v, code) = publish(&env, &["publish", out_s], &[]);
    assert_eq!((v["status"].as_str(), code), (Some("published"), Some(0)), "{v}");
    assert_eq!(commit_count(&env), before + 1);

    // Another machine that pulled the record does not ask either.
    std::fs::remove_dir_all(env.dir.path().join("state/rkb")).unwrap();
    let (v, code) = publish(&env, &["publish", "--no-ask", out_s], &[]);
    assert_eq!((v["status"].as_str(), code), (Some("published"), Some(0)), "{v}");
    assert_eq!(v["lessons"], 2, "{v}");
    assert_eq!(commit_count(&env), before + 1, "--no-ask never writes the knowledge base");

    // A new public lesson that a recorded lesson links to: CI leaves both out.
    let new = env.kb().join("general/git/shallow-clones-break-git-describe.md");
    std::fs::write(&new, std::fs::read_to_string(&new).unwrap().replacen("\n---\n", "\nlabels:\n  sensitivity: public\n---\n", 1)).unwrap();
    let linker = env.kb().join("general/git/squash-fixups-with-autosquash.md");
    let text = std::fs::read_to_string(&linker).unwrap();
    std::fs::write(&linker, text.replacen("\n## ", "\nSee [describe](shallow-clones-break-git-describe.md).\n\n## ", 1)).unwrap();

    let (v, _) = env.json(&["doctor"], "");
    let c = v["checks"].as_array().unwrap().iter().find(|c| c["check"] == "site approvals").cloned().unwrap_or_else(|| panic!("{v}"));
    assert_eq!(c["status"], "warn", "{c}");
    assert!(c["detail"].as_str().unwrap().contains("1a00000004 Shallow clones break git describe"), "{c}");
    assert!(c["fix"].as_str().unwrap().contains("rkb publish <dir>"), "{c}");

    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("CLAUDECODE", "1").env_remove("SITE_PASSWORD").env("GITHUB_ACTIONS", "true");
    let o = rkb_with(c, &["publish", "--no-ask", out_s, "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)));
    assert_eq!((v["status"].as_str(), o.status.code()), (Some("published"), Some(0)), "{v}");
    assert_eq!(v["lessons"], 1, "the waiting lesson and its referrer are left out: {v}");
    let index: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out.join("site.json")).unwrap()).unwrap();
    assert_eq!(index["lessons"].as_array().unwrap().len(), 1, "{index}");
    assert_eq!(index["lessons"][0]["id"], "1a00000003", "{index}");
    let notes = v["notes"].to_string();
    assert!(
        notes.contains("1a00000004 Shallow clones break git describe") && notes.contains("1a00000005") && notes.contains("links to one"),
        "{v}"
    );
    assert!(String::from_utf8_lossy(&o.stderr).contains("::warning title=rkb publish::"), "{}", stdout(&o));

    // With a record but nothing left to publish, CI fails instead of writing an empty folder.
    std::fs::write(env.kb().join("site/published.json"), "{\"clear\": [], \"encrypted\": {}}\n").unwrap();
    std::fs::remove_file(env.dir.path().join("state/rkb/site-published.json")).ok();
    let (v, code) = publish(&env, &["publish", "--no-ask", out_s], &[]);
    assert_eq!(code, Some(1), "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("nothing would be published"), "{v}");
}

/// A lesson encrypted before and public now asks again, and the record follows the answer.
#[test]
fn publish_ask_again_when_the_password_changes() {
    let env = publish_kb();
    let out = env.dir.path().join("out");
    let out_s = out.to_str().unwrap();
    let pw = [("SITE_PASSWORD", PASSWORD)];

    let (v, _) = publish(&env, &["publish", out_s], &pw);
    let q = v["question"].as_str().unwrap();
    let (clear, enc) = q.split_once("Encrypted with SITE_PASSWORD:").unwrap_or_else(|| panic!("{q}"));
    assert!(clear.contains("1a00000003") && !clear.contains("1a00000004"), "{q}");
    assert!(enc.contains("1a00000004 Shallow clones break git describe"), "{q}");
    let (v, code) = publish(&env, &["confirm", v["request"].as_str().unwrap(), "--choice", "publish"], &pw);
    assert_eq!(code, Some(0), "{v}");
    let published = std::fs::read_to_string(env.kb().join("site/published.json")).unwrap();
    assert!(published.contains("\"encrypted\"") && published.contains("1a00000004"), "{published}");
    assert!(out.join("protected/general/git/shallow-clones-break-git-describe.md").is_file());

    let p = env.kb().join("general/git/shallow-clones-break-git-describe.md");
    std::fs::write(&p, std::fs::read_to_string(&p).unwrap().replacen("\n---\n", "\nlabels:\n  sensitivity: public\n---\n", 1)).unwrap();
    let (v, code) = publish(&env, &["publish", out_s], &pw);
    assert_eq!((v["status"].as_str(), code), (Some("needs_user"), Some(3)), "encrypted before, public now: {v}");
    let q = v["question"].as_str().unwrap();
    assert!(q.contains("In the clear:\n  1a00000004"), "{q}");
}

#[test]
fn lint_checks_the_site_image() {
    let env = kb_with_topics();
    let toml = env.kb().join("kb.toml");
    let base = std::fs::read_to_string(&toml).unwrap();
    std::fs::write(&toml, format!("{base}\n[site]\nimage = \"site/og.gif\"\n")).unwrap();
    let (v, code) = env.json(&["lint"], "");
    assert_eq!(code, Some(1), "{v}");
    assert!(v.to_string().contains("config/site-image") && v.to_string().contains("must be a png or jpg"), "{v}");
    std::fs::write(&toml, format!("{base}\n[site]\nanalytics = \"plausible\"\n")).unwrap();
    let (v, code) = env.json(&["lint"], "");
    assert_eq!(code, Some(1), "{v}");
    assert!(v.to_string().contains("config/site-analytics"), "{v}");
    std::fs::write(&toml, format!("{base}\n[site]\nimage = \"site/og.png\"\n")).unwrap();
    let (v, _) = env.json(&["lint"], "");
    assert!(!v.to_string().contains("config/site-image"), "{v}");
}

/// One request the fake Jev API got: its `Authorization` header and its JSON body.
type JevLog = std::sync::Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>>;

/// A fake Jev API on 127.0.0.1. `reply` gives the status and body for each request's JSON body.
fn fake_jev(reply: impl Fn(&serde_json::Value) -> (u16, String) + Send + 'static) -> (String, JevLog) {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
    let log: JevLog = Default::default();
    let seen = log.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut r = BufReader::new(stream.try_clone().unwrap());
            let (mut auth, mut len) = (String::new(), 0usize);
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                let (name, value) = line.split_once(':').unwrap_or((&line, ""));
                match name.to_ascii_lowercase().as_str() {
                    "authorization" => auth = value.trim().to_string(),
                    "content-length" => len = value.trim().parse().unwrap_or(0),
                    _ => {}
                }
            }
            let mut body = vec![0u8; len];
            r.read_exact(&mut body).unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            let (status, text) = reply(&body);
            seen.lock().unwrap().push((auth, body));
            let mut w = stream;
            let _ = write!(
                w,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            );
        }
    });
    (url, log)
}

/// A fake ntfy server on 127.0.0.1. `reply(method, path, body) -> (status, response)` answers each
/// request; `log` records `(method, authorization, body)` of every request. The topic is `/topic`.
#[derive(Default, Clone)]
struct NtfyLog(std::sync::Arc<std::sync::Mutex<Vec<(String, String, String)>>>);

fn fake_ntfy(reply: impl Fn(&str, &str, &str) -> (u16, String) + Send + Sync + 'static) -> (String, NtfyLog) {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/topic", listener.local_addr().unwrap());
    let log = NtfyLog::default();
    let seen = log.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut r = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            if r.read_line(&mut request_line).unwrap_or(0) == 0 {
                continue;
            }
            let mut parts = request_line.split_whitespace();
            let method = parts.next().unwrap_or("").to_string();
            let path = parts.next().unwrap_or("").to_string();
            let (mut auth, mut len) = (String::new(), 0usize);
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                let (name, value) = line.split_once(':').unwrap_or((&line, ""));
                match name.to_ascii_lowercase().as_str() {
                    "authorization" => auth = value.trim().to_string(),
                    "content-length" => len = value.trim().parse().unwrap_or(0),
                    _ => {}
                }
            }
            let mut body = vec![0u8; len];
            r.read_exact(&mut body).unwrap();
            let body = String::from_utf8_lossy(&body).into_owned();
            let (status, text) = reply(&method, &path, &body);
            seen.0.lock().unwrap().push((method, auth, body));
            let mut w = stream;
            let _ = write!(
                w,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            );
        }
    });
    (url, log)
}

fn set_proposals_ntfy(env: &Env, url: &str) {
    env.write_abs(&env.dir.path().join("config/rkb/config.toml"), &format!("[proposals]\nntfy = \"{url}\"\n"));
}

#[test]
fn note_posts_to_ntfy() {
    let env = search_kb();
    let (url, log) = fake_ntfy(|_m, _p, _b| (200, "{}".to_string()));
    set_proposals_ntfy(&env, &url);

    // An agent's note becomes one POST whose body has the text and the agent name.
    let o1 = env
        .cmd(env!("CARGO_BIN_EXE_rkb"))
        .env("RKB_AGENT", "worker-1")
        .args(["note", "hdf5 needs -lz after -lhdf5", "--format", "json"])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(o1.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&o1.stdout).unwrap();
    let reqs = log.0.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1, "exactly one post");
    let (method, auth, body) = &reqs[0];
    assert_eq!(method, "POST");
    assert_eq!(auth, "", "no token set; empty auth header");
    let posted: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(posted["v"], 1);
    assert_eq!(posted["agent"], "worker-1");
    assert_eq!(posted["text"], "hdf5 needs -lz after -lhdf5");
    assert_eq!(posted["id"], v["id"].as_str().unwrap());
    assert!(posted["time"].as_u64().is_some(), "a post time is set");
    assert!(posted.get("cut").is_none(), "a short note is not cut");
    assert_eq!(v["id"].as_str().unwrap().len(), 10, "the note got an inbox id");

    // A note that the leak scan flags is not posted; the note stays local and the result says why.
    let o = env.rkb(&["note", "mail alice@example.org about the fix", "--toon"]);
    assert_eq!(o.status.code(), Some(0), "a leak-flagged note still succeeds locally");
    let human = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    assert!(human.contains("leak"), "the result says why it was not posted: {human}");
    assert_eq!(log.0.lock().unwrap().len(), 1, "the leak-flagged note was not posted");
    let (vinbox, _) = env.json(&["inbox"], "");
    assert_eq!(vinbox["items"].as_array().unwrap().len(), 2, "both notes are in the local inbox");

    // The token from the environment goes as Authorization: Bearer, and only there.
    let o2 = env
        .cmd(env!("CARGO_BIN_EXE_rkb"))
        .env("RKB_AGENT", "worker-2")
        .env("RKB_NTFY_TOKEN", "sekrit")
        .args(["note", "the token must reach the server", "--format", "json"])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(o2.status.code(), Some(0));
    let v2: serde_json::Value = serde_json::from_slice(&o2.stdout).unwrap();
    let reqs = log.0.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[1].1, "Bearer sekrit");
    // The token is never written to a state or config file.
    for entry in walkdir_files(env.dir.path()) {
        let text = std::fs::read_to_string(&entry).unwrap_or_default();
        if text.contains("sekrit") {
            panic!("the token leaked into {}: {text}", entry.display());
        }
    }
    let _ = &v2;
    // A `${RKB_NTFY_TOKEN}` that a launcher left unfilled sends no header.
    let o3 = env
        .cmd(env!("CARGO_BIN_EXE_rkb"))
        .env("RKB_NTFY_TOKEN", "${RKB_NTFY_TOKEN}")
        .args(["note", "an unfilled token reference", "--format", "json"])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(o3.status.code(), Some(0));
    assert_eq!(log.0.lock().unwrap()[2].1, "", "an unfilled reference is not a token");
}

#[test]
fn note_stays_when_ntfy_is_down() {
    // Port 1 refuses connections fast, so the post fails without hanging the note.
    let env = search_kb();
    set_proposals_ntfy(&env, "http://127.0.0.1:1/topic");
    let o = env.rkb(&["note", "a note that must survive a dead ntfy", "--toon"]);
    assert_eq!(o.status.code(), Some(0), "a failed post is not a note error");
    let human = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    assert!(human.contains("the ntfy post failed"), "{human}");
    let (vinbox, _) = env.json(&["inbox"], "");
    assert_eq!(vinbox["items"].as_array().unwrap().len(), 1, "the note stays in the inbox");
}

fn walkdir_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn inbox_pull_adds_once() {
    use serde_json::json;
    let env = search_kb();
    // ntfy's poll reply: one JSON event per line, oldest first, with string message ids.
    let events = [
        json!({"id": "kOpen1", "event": "open", "topic": "topic", "time": 50}),
        json!({"id": "sPs71M8A2T", "event": "message", "topic": "topic", "time": 100,
               "message": json!({"v": 1, "agent": "worker-1", "id": "a", "time": 100, "text": "first note"}).to_string()}),
        json!({"id": "Q2b9xLm0Kd", "event": "message", "topic": "topic", "time": 200,
               "message": json!({"v": 1, "agent": "worker-2", "id": "b", "time": 200, "text": "second \u{1b}[31mnote"}).to_string()}),
        json!({"id": "Zz3", "event": "message", "topic": "topic", "time": 300, "message": "this is not the JSON body"}),
        json!({"id": "Ab4", "event": "message", "topic": "topic", "time": 400,
               "message": json!({"v": 1, "agent": "Bad Agent!", "id": "d", "time": 400, "text": "from a bad agent"}).to_string()}),
    ];
    let lines: Vec<String> = events.iter().map(|e| e.to_string()).collect();
    let ids: Vec<String> = events.iter().map(|e| e["id"].as_str().unwrap().to_string()).collect();
    let reset = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ignore_since = reset.clone();
    let (url, log) = fake_ntfy(move |_m, path, _b| {
        let Some(since) = path.strip_prefix("/topic/json?poll=1&since=") else { return (404, String::new()) };
        // A server that lost its place answers everything again.
        let from = match ids.iter().position(|i| i == since) {
            Some(i) if !ignore_since.load(std::sync::atomic::Ordering::SeqCst) => i + 1,
            _ => 0,
        };
        (200, lines[from..].iter().map(|l| format!("{l}\n")).collect())
    });

    let (v, code) = env.json(&["inbox", "pull", &url], "");
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["added"], 3, "two valid and one bad-agent message are added: {v}");
    assert_eq!(v["skipped"], 1, "a malformed message is skipped and counted: {v}");

    let (v, code) = env.json(&["inbox", "pull", &url], "");
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["added"], 0, "a second pull adds nothing: {v}");
    reset.store(true, std::sync::atomic::Ordering::SeqCst);
    let (v, _) = env.json(&["inbox", "pull", &url], "");
    assert_eq!(v["added"], 0, "a since reset adds nothing: {v}");
    assert_eq!(log.0.lock().unwrap().len(), 3, "each pull is one GET");

    // The inbox has the messages once: 2 valid + 1 bad-agent (as unknown), not duplicated.
    let (vinbox, _) = env.json(&["inbox"], "");
    let items = vinbox["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 3, "{vinbox}");
    let id = items.iter().find(|i| i["preview"] == "first note").map(|i| i["id"].as_str().unwrap().to_string()).unwrap();
    let (shown, _) = env.json(&["inbox", "show", &id], "");
    assert_eq!(shown["meta"]["source"], "ntfy:worker-1", "{shown}");
    assert!(items.iter().any(|i| i["preview"] == "second [31mnote"), "control characters are stripped: {vinbox}");
    let bad = items.iter().find(|i| i["preview"] == "from a bad agent").map(|i| i["id"].as_str().unwrap().to_string()).unwrap();
    let (shown, _) = env.json(&["inbox", "show", &bad], "");
    assert_eq!(shown["meta"]["source"], "ntfy:unknown", "a bad agent name becomes unknown: {shown}");
    // Nothing pulled is promoted, approved or run automatically: the kb is untouched.
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "", "pulling writes nothing to the knowledge base");

    // A bad [proposals] ntfy refuses the note before it is written.
    set_proposals_ntfy(&env, "ftp://example.invalid/topic");
    let o = env.rkb(&["note", "refused before the add", "--toon"]);
    assert_ne!(o.status.code(), Some(0));
    let (vinbox, _) = env.json(&["inbox"], "");
    assert_eq!(vinbox["items"].as_array().unwrap().len(), 3, "a refused note is not written: {vinbox}");
}

/// Answers every question, rating later candidates higher, so a Jev ranking reverses BM25's.
fn jev_answers(body: &serde_json::Value) -> (u16, String) {
    let keys: Vec<String> = body["questions"].as_object().unwrap().keys().cloned().collect();
    let n = |k: &str| k.strip_prefix("lesson_").and_then(|n| n.parse::<f64>().ok()).unwrap_or(0.0);
    let answers: serde_json::Map<String, serde_json::Value> =
        keys.iter().map(|k| (k.clone(), serde_json::json!({ "type": "noul", "noul": (0.1 + n(k) / 100.0).min(1.0) }))).collect();
    (200, serde_json::json!({ "model": "jev-test", "answers": answers }).to_string())
}

fn jev_search(env: &Env, url: &str, query: &str, vars: &[(&str, &str)]) -> (serde_json::Value, String) {
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_JEV_URL", url).env_remove("RKB_JEV_API_KEY");
    for (k, v) in vars {
        c.env(k, v);
    }
    let o = rkb_with(c, &["search", query, "--format", "json"], "");
    let out = format!("{}{}", stdout(&o), String::from_utf8_lossy(&o.stderr));
    (serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{out}")), out)
}

fn jev_config(env: &Env, text: &str, mode: u32) {
    let p = env.dir.path().join("config/rkb/config.toml");
    env.write_abs(&p, text);
    std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(mode)).unwrap();
}

#[test]
fn jev_ranks_with_the_key_from_config() {
    let env = search_kb();
    set_chain(&env, "chain = [\"jev\", \"bm25\"]");
    let (url, log) = fake_jev(jev_answers);
    let (v, out) = jev_search(&env, &url, "git rebase", &[]);
    assert!(v["ranked_by"].as_str().unwrap().starts_with("bm25 (jev: not configured"), "{v}");
    assert!(log.lock().unwrap().is_empty());

    jev_config(&env, "[jev]\napi_key = \"sekrit-key-1\"\n", 0o644);
    let (v, out2) = jev_search(&env, &url, "git rebase", &[]);
    let why = v["ranked_by"].as_str().unwrap();
    assert!(why.contains("can be read by others") && why.contains("chmod 600"), "{v}");
    assert!(log.lock().unwrap().is_empty() && !out.contains("sekrit") && !out2.contains("sekrit"));

    std::fs::set_permissions(env.dir.path().join("config/rkb/config.toml"), std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let (bm25, _) = env.json(&["search", "git rebase", "--no-model"], "");
    let (v, out) = jev_search(&env, &url, "git rebase", &[]);
    assert_eq!(v["ranked_by"], "jev", "{v}");
    assert!(!out.contains("sekrit"), "the key never shows: {out}");
    let first =
        |v: &serde_json::Value| v["results"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    let (b, j) = (first(&bm25), first(&v));
    assert_eq!(j.first(), b.last(), "Jev rated the last BM25 candidate highest: {b:?} {j:?}");
    let (auth, body) = log.lock().unwrap()[0].clone();
    assert_eq!(auth, "Bearer sekrit-key-1");
    assert_eq!(body["model"], "jev-latest");
    assert!(body["state"].as_str().unwrap().starts_with("problem: git rebase\n\nlessons:\n\n[0]\n# "), "{body}");
    assert_eq!(body["questions"]["lesson_0"]["type"], "noul");
    assert_eq!(log.lock().unwrap().len(), 1, "one request for all candidates");

    let (v, _) = jev_search(&env, &url, "git rebase", &[("RKB_JEV_API_KEY", "env-key")]);
    assert_eq!(v["ranked_by"], "jev");
    assert_eq!(log.lock().unwrap().last().unwrap().0, "Bearer env-key", "the environment wins");

    jev_config(&env, "[jev]\napi_key_cmd = \"printf cmd-key\"\n", 0o644);
    let (v, _) = jev_search(&env, &url, "git rebase", &[]);
    assert_eq!(v["ranked_by"], "jev", "{v}");
    assert_eq!(log.lock().unwrap().last().unwrap().0, "Bearer cmd-key");
    jev_config(&env, "[jev]\napi_key_cmd = \"exit 3\"\n", 0o600);
    let (v, _) = jev_search(&env, &url, "git rebase", &[]);
    assert!(v["ranked_by"].as_str().unwrap().contains("api_key_cmd exited with 3"), "{v}");
}

#[test]
fn jev_never_receives_the_example_queries() {
    let env = search_kb();
    set_chain(&env, "chain = [\"jev\", \"bm25\"]");
    jev_config(&env, "[jev]\napi_key = \"k\"\n", 0o600);
    let files = stdout(&env.git(&["ls-files", "*.md"]));
    for f in files.lines() {
        let p = env.kb().join(f);
        let text = std::fs::read_to_string(&p).unwrap();
        std::fs::write(&p, text.replacen("\nstatus: active\n", "\nstatus: active\nqueries:\n  - zzprobe zzunique\n", 1)).unwrap();
    }
    let (url, log) = fake_jev(jev_answers);
    let (v, _) = jev_search(&env, &url, "git rebase", &[]);
    assert_eq!(v["ranked_by"], "jev", "{v}");
    let log = log.lock().unwrap();
    assert!(!log.is_empty() && log.iter().all(|(_, b)| !b.to_string().contains("zzprobe")), "{log:?}");
}

#[test]
fn jev_falls_back_and_sends_nothing_it_may_not() {
    let env = search_kb();
    set_chain(&env, "chain = [\"jev\", \"bm25\"]");
    jev_config(&env, "[jev]\napi_key = \"k\"\n", 0o600);
    let (url, log) = fake_jev(|_| (429, "{}".into()));
    let (v, _) = jev_search(&env, &url, "git rebase", &[]);
    assert_eq!(v["ranked_by"], "bm25 (jev: HTTP 429)", "{v}");
    assert_eq!(log.lock().unwrap().len(), 1);

    let (v, _) = jev_search(&env, &url, "testuser git rebase", &[]);
    assert!(v["ranked_by"].as_str().unwrap().contains("jev: the leak scan matched"), "$USER is testuser: {v}");
    assert_eq!(log.lock().unwrap().len(), 1, "nothing sent");

    let toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    env.write(
        "kb.toml",
        &toml.replace(
            "[sinks.jev]\nallow = { sensitivity = [\"public\", \"internal\"] }",
            "[sinks.jev]\nallow = { sensitivity = [\"public\"] }",
        ),
    );
    let (v, _) = jev_search(&env, &url, "git rebase", &[]);
    assert_eq!(v["ranked_by"], "bm25 (jev: no candidate is allowed by sinks.jev or passes the leak scan)", "{v}");
    assert_eq!(log.lock().unwrap().len(), 1, "nothing sent");

    env.write("kb.toml", &toml);
    let (url401, _) = fake_jev(|_| (401, "{}".into()));
    set_chain(&env, "chain = [\"jev\"]\nstrict = true");
    let (v, _) = jev_search(&env, &url401, "git rebase", &[]);
    assert!(v["error"]["message"].as_str().unwrap().contains("jev") && v["error"]["message"].as_str().unwrap().contains("HTTP 401"), "{v}");
}

#[test]
fn jev_one_request_per_candidate() {
    let env = search_kb();
    set_chain(&env, "chain = [\"jev\", \"bm25\"]");
    let toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    env.write("kb.toml", &format!("{toml}\n[jev]\nbatch = false\nmodel = \"jev-test\"\n"));
    jev_config(&env, "[jev]\napi_key = \"k\"\n", 0o600);
    let (url, log) = fake_jev(|_| (200, serde_json::json!({ "answers": { "relevant": { "type": "noul", "noul": 0.5 } } }).to_string()));
    let (v, _) = jev_search(&env, &url, "git rebase", &[]);
    assert_eq!(v["ranked_by"], "jev", "{v}");
    let log = log.lock().unwrap();
    assert!(log.len() >= 2, "one request per candidate: {}", log.len());
    assert!(log.iter().all(|(_, b)| b["model"] == "jev-test" && b["questions"].as_object().unwrap().len() == 1));
    assert!(log[0].1["state"].as_str().unwrap().starts_with("problem: git rebase\nlesson:\n# "), "{}", log[0].1);
}

#[test]
fn jev_config_lint_default_and_doctor() {
    let env = Env::new();
    env.init();
    let toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    assert!(toml.contains("[sinks.jev]\nallow = { sensitivity = [\"public\"] }"), "{toml}");
    env.write("kb.toml", &format!("{toml}\n[jev]\napi_key = \"k\"\n"));
    let o = env.rkb(&["lint", "--format", "json"]);
    let out = stdout(&o);
    assert!(out.contains("config/jev") && out.contains("~/.config/rkb/config.toml"), "{out}");

    env.write("kb.toml", &toml.replace("chain = [\"bm25\"]", "chain = [\"jev\", \"bm25\"]"));
    jev_config(&env, "[jev]\napi_key = \"sekrit-key-2\"\n", 0o600);
    let o = env.rkb(&["doctor", "--format", "json"]);
    let out = stdout(&o);
    assert!(!out.contains("sekrit"), "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let jev: Vec<&serde_json::Value> = v["checks"].as_array().unwrap().iter().filter(|c| c["check"] == "jev").collect();
    assert_eq!((jev.len(), jev[0]["status"].as_str(), jev[0]["detail"].as_str()), (1, Some("ok"), Some("key from config.toml")), "{v}");

    env.write(
        "kb.toml",
        &toml.replace("chain = [\"bm25\"]", "chain = [\"jev\", \"bm25\"]").replace(
            "allow = { sensitivity = [\"public\"] }\n\n[rerank]",
            "allow = { sensitivity = [\"public\", \"internal\"] }\n\n[rerank]",
        ),
    );
    let (v, _) = env.json(&["doctor"], "");
    let warn = v["checks"].as_array().unwrap().iter().find(|c| c["check"] == "jev" && c["status"] == "warn").cloned().unwrap_or_default();
    assert!(warn["detail"].as_str().unwrap_or("").contains("internal"), "{v}");
}

#[test]
fn recall_uses_jev_thresholds_when_jev_ranked() {
    let env = search_kb();
    set_chain(&env, "chain = [\"jev\", \"bm25\"]\n\n[hooks]\nhook_timeout_ms = 5000");
    jev_config(&env, "[jev]\napi_key = \"k\"\n", 0o600);
    let (url, _log) = fake_jev(|body| {
        let answers: serde_json::Map<String, serde_json::Value> = body["questions"]
            .as_object()
            .unwrap()
            .keys()
            .map(|k| (k.clone(), serde_json::json!({ "type": "noul", "noul": if k == "lesson_0" { 0.80 } else { 0.20 } })))
            .collect();
        (200, serde_json::json!({ "answers": answers }).to_string())
    });
    let run = |session: &str| {
        let payload = serde_json::json!({ "session_id": session, "cwd": env.dir.path(), "prompt": "the linker says undefined reference to vtable for Widget" });
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.args(["hook", "prompt"]).env("RKB_JEV_URL", &url);
        stdout(&rkb_with(c, &[], &payload.to_string()))
    };
    assert!(run("j1").contains("additionalContext"), "0.80 with a 0.60 lead passes Jev's 0.75 and 0.05");
    let toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    env.write("kb.toml", &toml.replace("hook_timeout_ms = 5000", "hook_timeout_ms = 5000\nrecall_min_relevance = 0.85"));
    assert_eq!(run("j2"), "", "a kb.toml value wins over Jev's default");
    assert_eq!(std::fs::read_to_string(env.dir.path().join("state/rkb/hook-errors.log")).unwrap_or_default(), "", "no hook error");

    env.write("kb.toml", &toml);
    let injected = run("j3");
    let id = injected.split("id=").nth(1).and_then(|r| r.split(['"', ' ', '\\']).find(|w| w.len() == 10)).unwrap().to_string();
    assert_eq!(env.json(&["flag", &id, "--reason", "the fix failed on clang 18"], "").0["status"], "written");
    assert_eq!(run("j4"), "", "a stale lesson is never injected");
}

fn with_pi(env: &Env) -> Command {
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_PI", env.dir.path().join("fake/pi"));
    c
}

#[test]
fn hooks_do_nothing_inside_an_observer_run() {
    let env = search_kb();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/transcripts/claude.jsonl");
    signal(&env, "child", r#"{"kind":"remember"}"#);
    let payload = serde_json::json!({ "session_id": "child", "transcript_path": fixture, "cwd": env.dir.path(), "prompt": "remember that cmake needs ZLIB_ROOT here" });
    for event in ["session-start", "prompt", "pre-tool", "tool-ok", "tool-failed", "stop", "pre-compact", "session-end"] {
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.env("RKB_OBSERVER", "1");
        let o = rkb_with(c, &["hook", event], &payload.to_string());
        assert_eq!((o.status.code(), stdout(&o)), (Some(0), String::new()), "{event}");
    }
    assert!(inbox_items(&env).is_empty());
    assert!(!env.dir.path().join("state/rkb/heartbeat").exists());
}

#[test]
fn jev_rates_only_the_candidates_it_may_see() {
    let env = search_kb();
    set_chain(&env, "chain = [\"jev\", \"bm25\"]");
    jev_config(&env, "[jev]\napi_key = \"k\"\n", 0o600);
    let toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    let public_only = toml.replace(
        "[sinks.jev]\nallow = { sensitivity = [\"public\", \"internal\"] }",
        "[sinks.jev]\nallow = { sensitivity = [\"public\"] }",
    );
    env.write("kb.toml", &public_only);
    let path = "general/git/squash-fixups-with-autosquash.md";
    let text = std::fs::read_to_string(env.kb().join(path)).unwrap();
    env.write(path, &text.replacen("tags:", "labels:\n  sensitivity: public\ntags:", 1));
    let (bm25, _) = env.json(&["search", "git rebase", "--no-model"], "");
    let (url, log) = fake_jev(|_| (200, serde_json::json!({ "answers": { "lesson_0": { "type": "noul", "noul": 0.9 } } }).to_string()));
    let (v, _) = jev_search(&env, &url, "git rebase", &[]);
    assert_eq!(v["ranked_by"], "jev", "{v}");
    let body = log.lock().unwrap()[0].1.clone();
    let state = body["state"].as_str().unwrap();
    assert!(state.contains("Squash fixups") && !state.contains("autostash") && !state.contains("[1]"), "only the public lesson: {state}");
    let rows = |v: &serde_json::Value| v["results"].as_array().unwrap().clone();
    let (b, j) = (rows(&bm25), rows(&v));
    assert_eq!(b.len(), j.len());
    for (x, y) in b.iter().zip(&j) {
        assert_eq!(x["id"], y["id"], "one rated lesson keeps every place: {b:?} {j:?}");
        let public = y["path"].as_str().unwrap() == path;
        assert_eq!(y["relevance"].is_null(), !public, "{y}");
    }

    set_chain(&env, "chain = [\"missing\", \"jev\", \"bm25\"]");
    let (v, _) = jev_search(&env, &url, "git rebase", &[]);
    assert!(v["ranked_by"].as_str().unwrap().contains("jev: a candidate is not allowed by sinks.jev"), "behind another backend: {v}");
    assert_eq!(log.lock().unwrap().len(), 1, "nothing more sent");
}

#[test]
fn prompt_hook_reads_only_what_the_user_typed_and_logs_rankings() {
    let env = search_kb();
    set_chain(&env, "chain = [\"missing\", \"bm25\"]");
    let sessions = env.dir.path().join("state/rkb/sessions/h1.jsonl");
    let send = |prompt: &str| hook(&env, "prompt", &serde_json::json!({ "session_id": "h1", "cwd": env.dir.path(), "prompt": prompt }));
    send(
        "Another Claude session sent a message:\n<agent-message from=\"a1\">\n  The hooks record correction/remember signals for cmake builds.\n</agent-message>",
    );
    send("<task-notification>\n<summary>remember this</summary>\n</task-notification>");
    assert!(!sessions.exists(), "no signal from harness text");
    let rankings = env.dir.path().join("state/rkb/rankings.jsonl");
    assert!(!rankings.exists(), "no search for harness text");
    send("please remember that cmake keeps a failed find_package result");
    assert!(std::fs::read_to_string(&sessions).unwrap().contains("remember"));
    let log = std::fs::read_to_string(&rankings).unwrap();
    let v: serde_json::Value = serde_json::from_str(log.lines().last().unwrap()).unwrap();
    assert_eq!((v["hook"].as_str(), v["backend"].as_str()), (Some("recall"), Some("bm25")), "{v}");
    assert!(v["skipped"][0].as_str().unwrap().starts_with("missing: "), "{v}");
    let (d, _) = env.json(&["doctor"], "");
    assert!(d["checks"].as_array().unwrap().iter().any(|c| c["check"] == "hook rankings"), "{d}");
}

#[test]
fn stop_blocks_only_for_the_users_own_signals() {
    let env = search_kb();
    for r in [
        r#"{"kind":"failed","program":"cmake"}"#,
        r#"{"kind":"failed","program":"cmake"}"#,
        r#"{"kind":"failed","program":"cmake"}"#,
        r#"{"kind":"nohit","program":"cmake"}"#,
        r#"{"kind":"fixed","program":"cmake"}"#,
    ] {
        signal(&env, "t1", r);
    }
    let out = hook(&env, "stop", &serde_json::json!({ "session_id": "t1", "stop_hook_active": false }));
    assert_eq!(out, "", "task signals alone score 3 but reach the stop not at all");
    signal(&env, "t2", r#"{"kind":"remember"}"#);
    let out = hook(&env, "stop", &serde_json::json!({ "session_id": "t2", "stop_hook_active": false }));
    assert!(out.contains("\"decision\":\"block\""), "the user asked to remember: {out}");
}

#[test]
fn failure_recall_can_be_turned_off() {
    let env = search_kb();
    set_chain(&env, "chain = [\"missing\", \"bm25\"]\n\n[hooks]\nfailure_recall = false");
    let mut failed = bash("f1", &env.kb(), "g++ main.o -o app");
    failed["error"] =
        "Exit code 1\n/usr/bin/ld: main.o: undefined reference to `vtable for Widget'\ncollect2: error: ld returned 1 exit status\n".into();
    assert_eq!(hook(&env, "tool-failed", &failed), "");
    let state = env.dir.path().join("state/rkb");
    assert!(std::fs::read_to_string(state.join("sessions/f1.jsonl")).unwrap().contains("\"failed\""), "the signal is kept");
    assert!(!state.join("rankings.jsonl").exists() && !state.join("replays.jsonl").exists(), "no search ran");
}

#[test]
fn failure_hook_pushes_only_pitfalls() {
    let env = kb_with_topics();
    let recipe = "---\ntype: recipe\nverified_how: ran\ntags:\n  - docs\n---\n\n# Build the docs with sphinx\n\n## When to use\nWhen the sphinx build fails with `sphinx build failed: missing autodoc extension for api reference`.\n\n## Steps\n```sh\npip install sphinx-autodoc\nsphinx-build -b html docs docs/_build\n```\n\n## Evidence\nThe sphinx build passed after the autodoc extension was added.\n";
    let recipe_id = add_ok(&env, "python", recipe)["id"].as_str().unwrap().to_string();
    let pitfall = "---\ntype: pitfall\nverified_how: ran\ntags:\n  - cmake\n---\n\n# CMake cannot find HDF5 unless HDF5_ROOT is set\n\n## Symptom\nConfigure fails with `could not find HDF5, set HDF5_ROOT to the install prefix`.\n\n## Cause\nThe module has no CMake config file.\n\n## Fix\nSet HDF5_ROOT to the install prefix.\n\n## Evidence\nConfigure passed with the flag.\n";
    let pitfall_id = add_ok(&env, "cmake", pitfall)["id"].as_str().unwrap().to_string();
    let cwd = env.kb();

    let mut sphinx = bash("s-recipe", &cwd, "sphinx-build -b html docs docs/_build");
    sphinx["error"] = "Exit code 2\nerror: sphinx build failed: missing autodoc extension for api reference\n".into();
    let out = hook(&env, "tool-failed", &sphinx);
    assert!(!out.contains("<rkb-lesson") && !out.contains(&recipe_id), "a recipe top hit pushes nothing: {out}");
    let replays: Vec<serde_json::Value> = std::fs::read_to_string(env.dir.path().join("state/rkb/replays.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(replays.last().unwrap()["injected"].is_null(), "the replay log records none: {replays:?}");

    let mut cmake = bash("s-pitfall", &cwd, "cmake -B build");
    cmake["error"] = "Exit code 1\nCMake Error: could not find HDF5, set HDF5_ROOT to the install prefix\n".into();
    let out = hook(&env, "tool-failed", &cmake);
    assert!(out.contains(&pitfall_id), "a pitfall top hit still pushes: {out}");
}

#[test]
fn when_tool_matches_the_failed_program() {
    let env = kb_with_topics();
    let lesson = PITFALL
        .replace("verified_how: ran", "verified_how: ran\nwhen:\n  tool: cmake")
        .replace("Configure fails.", "Configure fails with `could not find HDF5, set HDF5_ROOT to the install prefix`.")
        .replace("Set HDF5_ROOT.", "Set HDF5_ROOT to the install prefix.");
    let id = add_ok(&env, "cmake", &lesson)["id"].as_str().unwrap().to_string();
    let cwd = env.kb();
    let error: serde_json::Value = "Exit code 1\nCMake Error: could not find HDF5, set HDF5_ROOT to the install prefix\n".into();

    let (v, _) = env.json(&["search", "could not find HDF5", "--with", "tool=cmake"], "");
    assert!(
        v["results"].as_array().unwrap().iter().any(|r| r["id"] == id && r["applies"] == "yes"),
        "the search uses the lesson file: {v}"
    );
    let (v, _) = env.json(&["search", "could not find HDF5", "--with", "tool=make", "--all"], "");
    assert!(v["results"].as_array().unwrap().iter().any(|r| r["id"] == id && r["applies"] == "no"), "another tool does not apply: {v}");

    let mut cmake = bash("s-cmake", &cwd, "cmake -B build");
    cmake["error"] = error.clone();
    let out = hook(&env, "tool-failed", &cmake);
    assert!(out.contains(&id), "the failed program sets the tool fact: {out}");

    let mut make = bash("s-make", &cwd, "make");
    make["error"] = error;
    let out = hook(&env, "tool-failed", &make);
    assert!(!out.contains("<rkb-lesson") && !out.contains(&id), "another tool pushes nothing: {out}");
    let replays: Vec<serde_json::Value> = std::fs::read_to_string(env.dir.path().join("state/rkb/replays.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(replays.last().unwrap()["injected"].is_null(), "the replay log records none: {replays:?}");
}

fn tree(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = vec![];
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(tree(&p));
        } else {
            out.push((p.clone(), std::fs::read(&p).unwrap()));
        }
    }
    out.sort();
    out
}

#[test]
fn status_lists_waiting_work_and_session_injections() {
    let env = search_kb();
    for text in ["first high note", "second high note"] {
        assert_eq!(env.rkb(&["note", "--priority", "3", text]).status.code(), Some(0));
    }
    signal(&env, "s1", r#"{"kind":"injected","id":"1a00000011","hook":"recall","time":1}"#);
    let before = (tree(&env.kb()), tree(&env.dir.path().join("state")));
    let (v, code) = env.json(&["status", "--session", "s1"], "");
    assert_eq!(code, Some(0), "{v}");
    assert_eq!((v["inbox"]["total"].as_u64(), v["inbox"]["priority"]["3"].as_u64()), (Some(2), Some(2)), "{v}");
    assert_eq!(v["injected"][0]["id"], "1a00000011", "{v}");
    assert_eq!(v["injected"][0]["hook"], "recall", "{v}");
    assert!(v["injected"][0]["title"].as_str().is_some_and(|t| !t.is_empty()), "{v}");
    assert!(v["requests"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!((tree(&env.kb()), tree(&env.dir.path().join("state"))), before, "status changes no file");
}

#[test]
fn command_hooks_stay_quiet_for_the_session_the_mod_handles() {
    let env = search_kb();
    let run = |args: &[&str], session: &str, rkb_mod: &str| {
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.env("RKB_MOD", rkb_mod);
        let payload = serde_json::json!({ "session_id": session, "source": "startup", "cwd": env.kb() });
        let o = rkb_with(c, args, &payload.to_string());
        assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));
        stdout(&o)
    };
    let beat = env.dir.path().join("state/rkb/heartbeat/claude-code");
    assert_eq!(run(&["hook", "session-start"], "s1", "s1"), "", "the mod handles s1");
    assert!(!beat.exists(), "no heartbeat for a call the mod handles");
    assert!(run(&["hook", "session-start", "--mod"], "s1", "s1").contains("rkb search"), "the mod's own call runs");
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_MOD", "s1");
    let gate = serde_json::json!({ "session_id": "s1", "cwd": env.kb(), "tool_name": "Bash", "tool_input": { "command": "rkb confirm r-4f2a9c --choice continue" } });
    assert!(stdout(&rkb_with(c, &["hook", "pre-tool"], &gate.to_string())).contains("\"ask\""), "the confirm gate always runs");
    assert!(run(&["hook", "stop", "--harness", "pi"], "p7", "s1").is_empty(), "stop with no signal prints nothing");
    assert!(env.dir.path().join("state/rkb/heartbeat/pi").exists(), "a pi session inside a Claude Code shell still runs");
    assert!(run(&["hook", "session-start"], "s2", "s1").contains("rkb search"), "another session runs");
}

#[test]
fn tool_results_as_json_and_both() {
    let env = search_kb();
    let run = |args: &[&str], input: &serde_json::Value| {
        let o = env.rkb_in(args, &input.to_string());
        (String::from_utf8_lossy(&o.stdout).into_owned(), o.status.code())
    };
    let query = serde_json::json!({ "query": "undefined reference to vtable" });
    let (json, code) = run(&["tool", "rkb_search", "--format", "json"], &query);
    let (search, _) = env.json(&["search", "undefined reference to vtable"], "");
    assert_eq!((serde_json::from_str::<serde_json::Value>(&json).unwrap(), code), (search, Some(0)));

    let (err, code) = run(&["tool", "rkb_show", "--format", "json"], &serde_json::json!({}));
    let err: serde_json::Value = serde_json::from_str(&err).unwrap();
    assert_eq!((err["error"]["code"].as_str(), code), (Some("usage"), Some(2)), "{err}");
    assert!(err["error"]["message"].as_str().unwrap().contains("id"), "{err}");

    let (both, _) = run(&["tool", "rkb_search", "--both"], &query);
    let both: serde_json::Value = serde_json::from_str(&both).unwrap();
    assert_eq!(both["text"].as_str().unwrap(), tool(&env, "rkb_search", &query).0.trim_end(), "the same TOON as the plain call");
    assert_eq!(both["json"], env.json(&["search", "undefined reference to vtable"], "").0, "the same JSON as the command");

    let before = stdout(&env.git(&["log", "--oneline"])).lines().count();
    let pitfall = serde_json::json!({
        "type": "pitfall", "title": "Ninja ignores CFLAGS changes until a reconfigure", "topic": "cmake",
        "symptom": "A changed CFLAGS has no effect.", "cause": "CMake caches the flags at configure time.",
        "fix": "Run `cmake --fresh -B build`.", "evidence": "The next build used the new flags.",
    });
    let (both, code) = run(&["tool", "rkb_add", "--both"], &pitfall);
    let both: serde_json::Value = serde_json::from_str(&both).unwrap();
    assert_eq!(code, Some(0), "{both}");
    assert_eq!(stdout(&env.git(&["log", "--oneline"])).lines().count(), before + 1, "one run, one lesson");
    assert!(both["text"].as_str().unwrap().contains("status: written"), "{both}");
    assert_eq!(both["json"]["status"], "written", "{both}");
}

/// The keys `schema` requires are in `value`, and each listed property has its JSON type.
fn matches_schema(schema: &serde_json::Value, value: &serde_json::Value) -> Result<(), String> {
    let type_ok = |t: &serde_json::Value, v: &serde_json::Value| {
        let one = |t: &str| match t {
            "string" => v.is_string(),
            "number" => v.is_number(),
            "integer" => v.is_i64() || v.is_u64(),
            "object" => v.is_object(),
            "array" => v.is_array(),
            "null" => v.is_null(),
            _ => true,
        };
        match t {
            serde_json::Value::String(s) => one(s),
            serde_json::Value::Array(ts) => ts.iter().any(|t| t.as_str().is_some_and(one)),
            _ => true,
        }
    };
    if !type_ok(&schema["type"], value) {
        return Err(format!("{value} is not {}", schema["type"]));
    }
    for k in schema["required"].as_array().into_iter().flatten().filter_map(|k| k.as_str()) {
        if value.get(k).is_none() {
            return Err(format!("missing {k} in {value}"));
        }
    }
    for (k, sub) in schema["properties"].as_object().into_iter().flatten() {
        if let Some(v) = value.get(k) {
            matches_schema(sub, v)?;
        }
    }
    if let (Some(items), Some(list)) = (schema.get("items"), value.as_array()) {
        list.iter().try_for_each(|v| matches_schema(items, v))?;
    }
    Ok(())
}

#[test]
fn tool_metadata_and_output_schemas() {
    let env = search_kb();
    let (v, _) = env.json(&["tools"], "");
    let def = |name: &str| v["tools"].as_array().unwrap().iter().find(|t| t["name"] == name).unwrap().clone();
    assert_eq!(def("rkb_search")["annotations"]["readOnlyHint"], true);
    assert_eq!(def("rkb_edit")["annotations"]["destructiveHint"], true);
    assert_eq!(def("rkb_add")["annotations"]["readOnlyHint"], false);
    assert_eq!(v["namespace"]["name"], "rkb");
    assert!(v["namespace"]["instructions"].as_str().unwrap().contains("rkb_used"), "{v}");

    let query = serde_json::json!({ "query": "undefined reference to vtable" });
    let o = env.rkb_in(&["tool", "rkb_search", "--format", "json"], &query.to_string());
    let search: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert!(!search["results"].as_array().unwrap().is_empty());
    matches_schema(&def("rkb_search")["outputSchema"], &search).unwrap();
    let id = search["results"][0]["id"].as_str().unwrap();
    let o = env.rkb_in(&["tool", "rkb_show", "--format", "json"], &serde_json::json!({ "id": id }).to_string());
    matches_schema(&def("rkb_show")["outputSchema"], &serde_json::from_slice(&o.stdout).unwrap()).unwrap();
    assert!(matches_schema(&def("rkb_show")["outputSchema"], &serde_json::json!({ "id": "x" })).is_err(), "the check can fail");

    let list = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let o = env.rkb_in(&["mcp"], &format!("{list}\n"));
    let reply: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let tools = reply["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 7);
    assert!(tools.iter().all(|t| t.get("outputSchema").is_none() && t.get("annotations").is_some()), "{reply}");
}

/// Writes an inbox item file as rkb writes one; `signals` and the body are the item's.
fn inbox_item(env: &Env, id: &str, kind: &str, signals: &[&str], body: &str) {
    let dir = env.dir.path().join("state/rkb/inbox");
    std::fs::create_dir_all(&dir).unwrap();
    let signals: String = signals.iter().map(|s| format!("- {s}\n")).collect();
    let signals = if signals.is_empty() { String::new() } else { format!("signals:\n{signals}") };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    std::fs::write(dir.join(format!("{id}.md")), format!("---\nkind: {kind}\ntime: {now}\n{signals}---\n\n{body}\n")).unwrap();
}

#[test]
fn inbox_triage_splits_and_applies_the_rules() {
    let env = search_kb();
    inbox_item(
        &env,
        "0000000001",
        "observed",
        &[],
        "- 2026-10-02 [med] (fact; project) rkb is at version 0.0.14\n- 2026-10-02 [high] (pitfall; general) `cargo install --path` on a workspace root fails with `found a virtual manifest`; pass the member crate.\n- 2026-10-02 [low] (decision; project) Keep the confirm gate in the pre-tool hook, because only its input has permission_mode.",
    );
    inbox_item(&env, "0000000002", "transcript", &["fixed cd", "fixed ls"], "[user] list the files");
    inbox_item(&env, "0000000003", "transcript", &["fixed ls", "remember"], "[user] remember that ninja is faster here");
    let (v, code) = env.json(&["inbox", "triage"], "");
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["total"], serde_json::json!({ "keep": 1, "known": 0, "unsure": 2, "drop": 2 }), "{v}");
    let show = |id: &str| env.json(&["inbox", "show", id], "").0["meta"]["candidates"].clone();
    let observed = show("0000000001");
    assert_eq!(observed.as_array().unwrap().len(), 3, "one candidate per note: {observed}");
    assert_eq!((observed[0]["verdict"].as_str(), observed[0]["reason"].as_str()), (Some("drop"), Some("rule")), "{observed}");
    assert_eq!(observed[1]["line"], 1);
    assert_eq!((show("0000000002")[0]["verdict"].as_str(), show("0000000002")[0]["reason"].as_str()), (Some("drop"), Some("rule")));
    assert_eq!((show("0000000003")[0]["verdict"].as_str(), show("0000000003")[0]["reason"].as_str()), (Some("keep"), Some("remember")));
    let human = stdout(&env.rkb(&["inbox", "show", "0000000001", "--format", "human"]));
    assert!(human.contains("verdicts:") && human.contains("0 drop (rule) - 2026-10-02 [med] (fact; project) rkb is at version"), "{human}");
    let (list, _) = env.json(&["inbox"], "");
    let row = list["items"].as_array().unwrap().iter().find(|r| r["id"] == "0000000001").unwrap().clone();
    assert_eq!(row["verdicts"], serde_json::json!({ "keep": 0, "known": 0, "unsure": 2, "drop": 1 }), "{row}");
    assert!(stdout(&env.rkb(&["inbox", "--format", "human"])).contains("2 unsure, 1 drop"));
    let (again, _) = env.json(&["inbox", "triage"], "");
    assert!(again["items"].as_array().unwrap().is_empty(), "triaged items are not triaged again: {again}");
}

#[test]
fn inbox_triage_marks_what_a_lesson_already_says() {
    let env = search_kb();
    set_chain(&env, "chain = [\"jev\", \"bm25\"]");
    jev_config(&env, "[jev]\napi_key = \"k\"\n", 0o600);
    let (url, _log) = fake_jev(|body| {
        let answers: serde_json::Map<String, serde_json::Value> = body["questions"]
            .as_object()
            .unwrap()
            .keys()
            .map(|k| (k.clone(), serde_json::json!({ "type": "noul", "noul": if k == "lesson_0" { 0.91 } else { 0.10 } })))
            .collect();
        (200, serde_json::json!({ "answers": answers }).to_string())
    });
    inbox_item(
        &env,
        "0000000004",
        "note",
        &[],
        "undefined reference to vtable for Widget: a virtual function was declared but never defined",
    );
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_JEV_URL", &url);
    let o = rkb_with(c, &["inbox", "triage", "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)));
    assert_eq!(v["total"]["known"], 1, "{v}");
    let (show, _) = env.json(&["inbox", "show", "0000000004"], "");
    let c = &show["meta"]["candidates"][0];
    assert_eq!((c["verdict"].as_str(), c["reason"].as_str()), (Some("known"), Some("known")), "{show}");
    let (top, _) =
        jev_search(&env, &url, "undefined reference to vtable for Widget: a virtual function was declared but never defined", &[]);
    assert_eq!(c["lesson"], top["results"][0]["id"], "the lesson Jev rated highest: {show}");
}

#[test]
fn inbox_triage_asks_jev_whether_a_note_is_worth_a_lesson() {
    let env = search_kb();
    set_chain(&env, "chain = [\"jev\", \"bm25\"]");
    let toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    if !toml.contains("[sinks.jev]") {
        env.write("kb.toml", &format!("{toml}\n[sinks.jev]\nallow = {{ sensitivity = [\"public\", \"internal\"] }}\n"));
    }
    jev_config(&env, "[jev]\napi_key = \"k\"\n", 0o600);
    let (url, log) = fake_jev(|body| {
        let answers: serde_json::Map<String, serde_json::Value> = body["questions"]
            .as_object()
            .unwrap()
            .keys()
            .map(|k| {
                let a = match k.as_str() {
                    "durable_0" => serde_json::json!({ "type": "noul", "noul": 0.95 }),
                    "general_0" => serde_json::json!({ "type": "noul", "noul": 0.9 }),
                    "durable_1" | "general_1" => serde_json::json!({ "type": "noul", "noul": 0.9 }),
                    "kind_0" => serde_json::json!({ "type": "choice", "choice": "pitfall", "probabilities": { "pitfall": 0.9, "not_a_lesson": 0.05 } }),
                    "kind_1" => serde_json::json!({ "type": "choice", "choice": "not_a_lesson", "probabilities": { "fact": 0.1, "not_a_lesson": 0.85 } }),
                    _ => serde_json::json!({ "type": "noul", "noul": 0.1 }),
                };
                (k.clone(), a)
            })
            .collect();
        (200, serde_json::json!({ "answers": answers }).to_string())
    });
    inbox_item(
        &env,
        "0000000005",
        "observed",
        &[],
        "- 2026-10-02 [high] (pitfall; general) `cargo install --path` on a workspace root fails with `found a virtual manifest`; pass the member crate.\n- 2026-10-02 [med] (fact; general) The meeting about the release moved to the afternoon slot this week.",
    );
    let triage = |env: &Env| {
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.env("RKB_JEV_URL", &url);
        let o = rkb_with(c, &["inbox", "triage", "--format", "json"], "");
        serde_json::from_slice::<serde_json::Value>(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)))
    };
    let v = triage(&env);
    assert_eq!(v["total"], serde_json::json!({ "keep": 1, "known": 0, "unsure": 0, "drop": 1 }), "{v}");
    let (show, _) = env.json(&["inbox", "show", "0000000005"], "");
    let c = show["meta"]["candidates"].clone();
    assert_eq!(
        (c[0]["verdict"].as_str(), c[0]["reason"].as_str(), c[0]["score"].as_f64(), c[0]["kind"].as_str()),
        (Some("keep"), Some("jev"), Some(0.855), Some("pitfall")),
        "{c}"
    );
    assert_eq!((c[1]["verdict"].as_str(), c[1]["kind"].as_str()), (Some("drop"), Some("not_a_lesson")), "not a lesson: {c}");
    let worth = log.lock().unwrap().iter().filter(|(_, b)| b["questions"].get("kind_0").is_some()).count();
    assert_eq!(worth, 1, "one worth request for the item");

    set_chain(&env, "chain = [\"bm25\"]");
    inbox_item(&env, "0000000006", "note", &[], "the linker wants -lz after -lhdf5 on this cluster");
    let v = triage(&env);
    assert_eq!(v["total"], serde_json::json!({ "keep": 0, "known": 0, "unsure": 1, "drop": 0 }), "no scorer: {v}");
}

#[test]
fn inbox_triage_keeps_internal_notes_from_jev() {
    let env = search_kb();
    let toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    env.write(
        "kb.toml",
        &toml
            .replace(
                "[sinks.jev]\nallow = { sensitivity = [\"public\", \"internal\"] }",
                "[sinks.jev]\nallow = { sensitivity = [\"public\"] }",
            )
            .replace("chain = [\"bm25\"]", "chain = [\"jev\", \"bm25\"]"),
    );
    env.write(
        "projects/dftracer/README.md",
        "---\nremotes:\n  - github.com/llnl/dftracer\nlabels:\n  sensitivity: internal\n---\n\n# dftracer\n",
    );
    // Public lessons, so a search for the note's text would call Jev without the item's own guard.
    env.write("general/README.md", "---\nlabels:\n  sensitivity: public\n---\n\n# General\n");
    assert!(
        env.git(&["add", "-A"]).status.success()
            && env.git(&["-c", "user.name=T", "-c", "user.email=t@example.org", "commit", "-q", "-m", "p"]).status.success()
    );
    let repo = project_repo(&env, "git@github.com:llnl/dftracer.git");
    jev_config(&env, "[jev]\napi_key = \"k\"\n", 0o600);
    let (url, log) = fake_jev(jev_answers);
    let secret = "CMake cannot find HDF5 on the dftracer staging cluster behind the second VPN gateway unless HDF5_ROOT is set.";
    inbox_item(&env, "0000000007", "note", &[], secret);
    let path = env.dir.path().join("state/rkb/inbox/0000000007.md");
    let text = std::fs::read_to_string(&path).unwrap().replacen("---\n\n", &format!("cwd: {}\n---\n\n", repo.display()), 1);
    std::fs::write(&path, text).unwrap();
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_JEV_URL", &url);
    let o = rkb_with(c, &["inbox", "triage", "--format", "json"], "");
    assert!(o.status.success(), "{}", stdout(&o));
    let (show, _) = env.json(&["inbox", "show", "0000000007"], "");
    let c = &show["meta"]["candidates"][0];
    assert_eq!((c["verdict"].as_str(), c["reason"].as_str()), (Some("unsure"), Some("jev: label internal")), "{show}");
    assert!(log.lock().unwrap().iter().all(|(_, body)| !body.to_string().contains("second VPN gateway")), "the note never reached Jev");
}

fn approve_as_claude(env: &Env) {
    let (v, code) = env.json(&["approve", "observer"], "");
    assert_eq!((v["status"].as_str(), code), (Some("needs_user"), Some(3)), "{v}");
    env.trust_claude();
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("CLAUDECODE", "1");
    let o = rkb_with(c, &["confirm", v["request"].as_str().unwrap(), "--choice", "approve observer", "--format", "json"], "");
    assert!(o.status.success(), "{}", stdout(&o));
}

#[test]
fn unsure_candidates_go_to_the_triage_models() {
    let env = search_kb();
    let config = env.dir.path().join("config/rkb/config.toml");
    env.write_abs(&config, "[observer]\nclaude-code = \"sonnet\"\npi = \"session\"\n\n[triage]\npi = \"small\"\n");
    inbox_item(&env, "0000000008", "note", &[], "the linker wants -lz after -lhdf5 on this cluster");
    inbox_item(&env, "0000000009", "note", &[], "the meeting moved to the afternoon");
    let (v, _) = env.json(&["inbox", "triage"], "");
    assert_eq!(v["total"]["unsure"], 2, "no scorer in the fixture chain: {v}");
    let (none, _) = env.json(&["job", "prepare", "triage"], "");
    assert!(none["job"].is_null() && none["skipped"].as_str().unwrap().contains("not approved"), "{none}");
    approve_as_claude(&env);
    let (job, _) = env.json(&["job", "prepare", "triage"], "");
    assert_eq!(
        (job["kind"].as_str(), job["models"][0]["entry"].as_str()),
        (Some("triage"), Some("haiku")),
        "claude-code defaults to haiku: {job}"
    );
    let prompt = job["prompt"].as_str().unwrap();
    assert!(prompt.contains("[0]\nthe linker wants -lz") && prompt.contains("[1]\nthe meeting moved"), "{prompt}");
    let id = job["job"].as_str().unwrap();
    let (done, code) =
        env.json(&["job", "finish", id, "--part", "0", "--model", "haiku"], "0 keep a link order that fails\n1 drop chatter\n");
    assert_eq!((done["counts"]["keep"].as_u64(), done["counts"]["drop"].as_u64(), code), (Some(1), Some(1), Some(0)), "{done}");
    let (show, _) = env.json(&["inbox", "show", "0000000008"], "");
    assert_eq!(show["meta"]["candidates"][0]["reason"], "haiku: a link order that fails", "{show}");

    // A changed [triage] chain needs a new approval.
    env.write_abs(&config, "[observer]\nclaude-code = \"sonnet\"\npi = \"session\"\n\n[triage]\nclaude-code = \"opus\"\npi = \"small\"\n");
    let (v, code) = env.json(&["approve", "observer"], "");
    assert_eq!((v["status"].as_str(), code), (Some("needs_user"), Some(3)), "{v}");
    assert!(v["question"].as_str().unwrap().contains("triage claude-code = opus"), "{v}");

    // `--ask` runs the harness's own CLI with the triage model.
    approve_as_claude(&env);
    let fake = env.dir.path().join("fake/pi");
    env.write_abs(&fake, "#!/bin/sh\necho \"$*\" > \"$FAKE_CLAUDE_HOME/args.txt\"\ncat > /dev/null\necho '0 keep a cluster setting'\n");
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    inbox_item(&env, "000000000a", "note", &[], "the scheduler needs --exclusive for the GPU jobs");
    let o = rkb_with(with_pi(&env), &["inbox", "triage", "--ask", "pi", "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)));
    assert_eq!(v["asked"]["counts"]["keep"], 1, "{v}");
    assert!(std::fs::read_to_string(env.dir.path().join("args.txt")).unwrap().contains("--model small"));
}

#[test]
fn inbox_done_records_outcomes_for_triage() {
    let env = search_kb();
    inbox_item(
        &env,
        "000000000b",
        "observed",
        &[],
        "- 2026-10-02 [high] (pitfall; general) `rsync -a src dst` copies the folder into dst/src; a trailing slash copies the contents.\n- 2026-10-02 [high] (fact; general) `ssh -o BatchMode=yes` fails at once instead of asking for a password.",
    );
    env.json(&["inbox", "triage"], "");
    let (v, code) = env.json(&["inbox", "done", "000000000b", "--outcome", "0=add:c04e11a9f3"], "");
    assert_eq!((v["items"][0]["removed"].as_bool(), code), (Some(false), Some(0)), "partial outcome: {v}");
    let (show, _) = env.json(&["inbox", "show", "000000000b"], "");
    assert_eq!(show["meta"]["candidates"][0]["outcome"], "add:c04e11a9f3");
    let log = std::fs::read_to_string(env.dir.path().join("state/rkb/triage.jsonl")).unwrap();
    let lines: Vec<serde_json::Value> = log.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert!(lines.iter().any(|l| l["item"] == "000000000b" && l["index"] == 0 && l["verdict"].is_string()), "{log}");
    assert!(
        lines.iter().any(|l| l["item"] == "000000000b" && l["index"] == 0 && l["outcome"] == "add" && l["lesson"] == "c04e11a9f3"),
        "outcome joined: {log}"
    );
    let (bad, code) = env.json(&["inbox", "done", "000000000b", "--outcome", "1=used"], "");
    assert_eq!((bad["error"]["code"].as_str(), code), (Some("usage"), Some(2)), "{bad}");
    let (v, _) = env.json(&["inbox", "done", "000000000b", "--outcome", "1=noop"], "");
    assert_eq!(v["items"][0]["removed"], true, "{v}");
}

#[test]
fn promote_holds_a_lesson_with_a_url() {
    let env = Env::new();
    env.init();
    env.write_abs(&env.dir.path().join("config/rkb/config.toml"), "[observer]\npi = \"session\"\n");
    approve_as_claude(&env);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let write_item = |id: &str, sig: &str, body: &str| {
        let dir = env.dir.path().join("state/rkb/inbox");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{id}.md")), format!("---\nkind: note\ntime: {now}\nsignatures:\n  - {sig}\n---\n\n{body}\n"))
            .unwrap();
        let p = env.dir.path().join("state/rkb/signatures.jsonl");
        let old = std::fs::read_to_string(&p).unwrap_or_default();
        std::fs::write(
            &p,
            format!(
                "{old}{{\"time\":{now},\"sig\":\"{sig}\",\"session\":\"s1\"}}\n{{\"time\":{now},\"sig\":\"{sig}\",\"session\":\"s2\"}}\n"
            ),
        )
        .unwrap();
    };
    let answer = |text: &str| {
        let fake = env.dir.path().join("fake/pi");
        env.write_abs(&fake, &format!("#!/bin/sh\ncat > /dev/null\nprintf '%s' '{text}'\n"));
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    };
    // A lesson with a URL waits for the user.
    write_item("0a1b2c3d4e", "aaabbbcccddd", "cmake could not find HDF5 on this machine");
    answer(
        r#"{"action":"add","topic":"cmake","type":"pitfall","title":"Cmake needs the prefix","queries":["cmake could not find HDF5","HDF5 prefix fix"],"sections":{"Symptom":"cmake could not find HDF5 on this machine","Cause":"Not known yet.","Fix":"See https://example.com/fix for the patch","Evidence":"Not known yet."},"quotes":["cmake could not find HDF5"]}"#,
    );
    let o = rkb_with(with_pi(&env), &["promote", "--harness", "pi", "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)));
    assert_eq!((v["promoted"].as_u64(), v["left"].as_u64()), (Some(0), Some(1)), "{v}");
    assert_eq!(v["results"][0]["needs_user"], true, "{v}");
    let (found, _) = env.json(&["search", "cmake could not find HDF5"], "");
    assert!(found["results"].as_array().unwrap().is_empty(), "nothing written yet: {found}");
    let (status, _) = env.json(&["status"], "");
    assert_eq!(status["requests"].as_array().unwrap().len(), 1, "{status}");
    // The user confirms it, and the lesson lands.
    let id = status["requests"][0]["id"].as_str().unwrap().to_string();
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_AUTO_CONFIRM", "write lesson");
    let o = rkb_with(c, &["confirm", &id, "--choice", "write lesson", "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)));
    assert_eq!(v["status"], "written", "{v}");
    let (found, _) = env.json(&["search", "cmake could not find HDF5"], "");
    assert_eq!(found["results"].as_array().unwrap().len(), 1, "{found}");
    // A lesson with neither writes at once.
    write_item("0a1b2c3d4f", "eeefff000111", "ninja is faster here");
    answer(
        r#"{"action":"add","topic":"cmake","type":"fact","title":"Ninja is faster here","queries":["ninja faster","ninja build speed"],"sections":{"Statement":"ninja is faster here","Evidence":"saw it"},"quotes":["ninja is faster here"]}"#,
    );
    let o = rkb_with(with_pi(&env), &["promote", "--harness", "pi", "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)));
    assert_eq!((v["promoted"].as_u64(), v["left"].as_u64()), (Some(1), Some(0)), "{v}");
    let (status, _) = env.json(&["status"], "");
    assert!(status["requests"].as_array().unwrap().is_empty(), "{status}");
    let (found, _) = env.json(&["search", "ninja is faster here"], "");
    assert_eq!(found["results"].as_array().unwrap().len(), 1, "{found}");
}

#[test]
fn promote_skips_a_lesson_that_changes_nothing() {
    let env = Env::new();
    env.init();
    env.write_abs(&env.dir.path().join("config/rkb/config.toml"), "[observer]\npi = \"session\"\n");
    approve_as_claude(&env);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let dir = env.dir.path().join("state/rkb/inbox");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("0a1b2c3d4e.md"),
        format!("---\nkind: note\ntime: {now}\nsignatures:\n  - aaabbbcccddd\n---\n\ncmake could not find HDF5 on this machine\n"),
    )
    .unwrap();
    std::fs::write(
        env.dir.path().join("state/rkb/signatures.jsonl"),
        format!(
            "{{\"time\":{now},\"sig\":\"aaabbbcccddd\",\"session\":\"s1\"}}\n{{\"time\":{now},\"sig\":\"aaabbbcccddd\",\"session\":\"s2\"}}\n"
        ),
    )
    .unwrap();
    let fake = env.dir.path().join("fake/pi");
    env.write_abs(
        &fake,
        r#"#!/bin/sh
cat > /dev/null
printf '%s' '{"action":"add","topic":"cmake","type":"fact","title":"Cmake needs the prefix","queries":["cmake could not find HDF5","HDF5 prefix fix"],"sections":{"Statement":"cmake could not find HDF5 on this machine","Evidence":"saw it"},"quotes":["cmake could not find HDF5"],"changes_behavior":false}'"#,
    );
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let o = rkb_with(with_pi(&env), &["promote", "--harness", "pi", "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)));
    assert_eq!((v["promoted"].as_u64(), v["ready"].as_u64()), (Some(0), Some(1)), "{v}");
    assert_eq!(v["results"][0]["action"], "noop", "{v}");
    assert!(
        v["results"][0]["reason"].as_str().unwrap().contains("change what an agent does"),
        "the reason names the behavior question: {v}"
    );
    let (found, _) = env.json(&["search", "cmake could not find HDF5"], "");
    assert!(found["results"].as_array().unwrap().is_empty(), "nothing written: {found}");
    assert!(inbox_items(&env).is_empty(), "the answered item is gone");
    let log = std::fs::read_to_string(env.dir.path().join("state/rkb/promotions.jsonl")).unwrap();
    let entry: serde_json::Value = serde_json::from_str(log.lines().last().unwrap()).unwrap();
    assert_eq!(entry["action"], "noop", "{log}");
    assert!(entry["reason"].as_str().unwrap().contains("change what an agent does"), "the log names the behavior question: {log}");
}

#[test]
fn promote_keeps_an_item_with_a_bad_quote() {
    let env = Env::new();
    env.init();
    env.write_abs(&env.dir.path().join("config/rkb/config.toml"), "[observer]\npi = \"session\"\n");
    approve_as_claude(&env);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let dir = env.dir.path().join("state/rkb/inbox");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("0a1b2c3d4e.md"),
        format!("---\nkind: note\ntime: {now}\nsignatures:\n  - aaabbbcccddd\n---\n\ncmake could not find HDF5 on this machine\n"),
    )
    .unwrap();
    std::fs::write(
        env.dir.path().join("state/rkb/signatures.jsonl"),
        format!(
            "{{\"time\":{now},\"sig\":\"aaabbbcccddd\",\"session\":\"s1\"}}\n{{\"time\":{now},\"sig\":\"aaabbbcccddd\",\"session\":\"s2\"}}\n"
        ),
    )
    .unwrap();
    let fake = env.dir.path().join("fake/pi");
    env.write_abs(
        &fake,
        r#"#!/bin/sh
cat > /dev/null
printf '%s' '{"action":"add","topic":"cmake","type":"fact","title":"Cmake needs the prefix","queries":["cmake could not find HDF5","HDF5 prefix fix"],"sections":{"Statement":"cmake could not find HDF5 on this machine","Evidence":"saw it"},"quotes":["a line the item never said"]}'"#,
    );
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let o = rkb_with(with_pi(&env), &["promote", "--harness", "pi", "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)));
    assert_eq!(v["results"][0]["action"], "noop", "{v}");
    assert_eq!(v["results"][0]["reason"], "the quote is not in the item", "{v}");
    let (found, _) = env.json(&["search", "cmake could not find HDF5"], "");
    assert!(found["results"].as_array().unwrap().is_empty(), "nothing written: {found}");
    let items = inbox_items(&env);
    assert!(items.iter().any(|i| i["id"] == "0a1b2c3d4e"), "the item stays in the inbox: {items:?}");
    let sigs = std::fs::read_to_string(env.dir.path().join("state/rkb/signatures.jsonl")).unwrap();
    assert!(!sigs.lines().any(|l| l.contains("\"done\":true")), "the signature is not marked done: {sigs}");
    // A second miss closes the signature, so a third pass makes no call; the item still stays.
    let o = rkb_with(with_pi(&env), &["promote", "--harness", "pi", "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)));
    assert_eq!(v["results"][0]["reason"], "the quote is not in the item", "{v}");
    let o = rkb_with(with_pi(&env), &["promote", "--harness", "pi", "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)));
    assert!(v["results"].as_array().unwrap().is_empty(), "no third call: {v}");
    assert!(inbox_items(&env).iter().any(|i| i["id"] == "0a1b2c3d4e"), "the item still stays");
}

#[test]
fn review_signals_counts_injected_new_lessons() {
    let env = kb_with_topics();
    let vtable = "---\ntype: pitfall\nverified_how: ran\ntags:\n  - cpp\n---\n\n# Undefined reference to vtable means a missing definition\n\n## Symptom\nThe link fails with undefined reference to vtable for Widget.\n\n## Cause\nA virtual function is declared but never defined.\n\n## Fix\nDefine every virtual function.\n\n## Evidence\nThe link passed after adding the definition.\n";
    let ninja = "---\ntype: fact\nverified_how: ran\ntags:\n  - cmake\n---\n\n# Ninja builds faster than make here\n\n## Statement\nNinja builds this tree faster than make.\n\n## Evidence\nTimed both on the same tree.\n";
    let zlib = "---\ntype: pitfall\nverified_how: ran\ntags:\n  - cmake\n---\n\n# ZLIB outside the prefix needs ZLIB_ROOT\n\n## Symptom\nThe configure step fails with Could NOT find ZLIB.\n\n## Cause\nZLIB lives under /opt/zlib, outside the search prefix.\n\n## Fix\nPoint the build at it with ZLIB_ROOT=/opt/zlib.\n\n## Evidence\nConfigure passed with the flag.\n";
    let ssh = "---\ntype: fact\nverified_how: ran\ntags:\n  - cmake\n---\n\n# BatchMode makes ssh fail at once\n\n## Statement\nssh -o BatchMode=yes fails at once instead of asking for a password.\n\n## Evidence\nSaw it in a script.\n";
    add_ok(&env, "cmake", PITFALL);
    add_ok(&env, "cpp", vtable);
    add_ok(&env, "cmake", ninja);
    add_ok(&env, "cmake", zlib);
    add_ok(&env, "cmake", ssh);
    let cwd = env.kb();
    let mut cmake = bash("s1", &cwd, "cmake -B build");
    cmake["error"] = "Exit code 1\nCMake Error: could not find HDF5, set HDF5_ROOT to configure the module\n".into();
    let out = hook(&env, "tool-failed", &cmake);
    assert!(out.contains("HDF5_ROOT"), "the HDF5 lesson is injected: {out}");
    let linker = "Exit code 1\n/usr/bin/ld: main.o: in function `main':\nmain.cpp:(.text+0x1f): undefined reference to `vtable for Widget'\ncollect2: error: ld returned 1 exit status\n";
    let mut gxx = bash("s2", &cwd, "g++ main.o -o app");
    gxx["error"] = linker.into();
    let out = hook(&env, "tool-failed", &gxx);
    assert!(out.contains("missing definition"), "the vtable lesson is injected: {out}");
    let (v, code) = env.json(&["review", "--signals"], "");
    assert_eq!(code, Some(0), "{v}");
    assert_eq!((v["signals"]["new_lessons"].as_u64(), v["signals"]["new_lessons_injected"].as_u64()), (Some(5), Some(2)), "{v}");
    assert!(stdout(&env.rkb(&["review", "--signals"])).contains("2 of 5"), "the report says 2 of 5");
}

#[test]
fn eval_drafts_a_reviewed_set() {
    let env = kb_with_topics();
    let lesson = add_ok(&env, "cmake", &lesson_with("Cmake prefix lesson"));
    let id = lesson["id"].as_str().unwrap().to_string();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let replay = |session: &str, query: &str, injected: &str| {
        format!(
            "{{\"time\":{now},\"session\":\"{session}\",\"project\":null,\"system\":null,\"query\":\"{query}\",\"injected\":\"{injected}\"}}\n"
        )
    };
    std::fs::create_dir_all(env.dir.path().join("state/rkb")).unwrap();
    std::fs::write(
        env.dir.path().join("state/rkb/replays.jsonl"),
        replay("s1", "cmake could not find HDF5", &id)
            + &replay("s2", "linker error vtable", "00aa00aa00")
            + &replay("s3", "a failure nobody reported", "00bb00bb00"),
    )
    .unwrap();
    let usage = env.kb().join(".rkb/usage/test.jsonl");
    std::fs::create_dir_all(usage.parent().unwrap()).unwrap();
    std::fs::write(
        &usage,
        format!(
            "{{\"time\":\"2030-01-01T00:00:00Z\",\"id\":\"{id}\",\"event\":\"worked\",\"session\":\"s1\"}}\n{{\"time\":\"2030-01-01T00:00:00Z\",\"id\":\"00aa00aa00\",\"event\":\"irrelevant\",\"session\":\"s2\"}}\n"
        ),
    )
    .unwrap();
    let draft = env.dir.path().join("reviewed.toml");
    let (v, code) = env.json(&["eval", "--draft-reviewed", draft.to_str().unwrap()], "");
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["events"], 2, "{v}");
    let text = std::fs::read_to_string(&draft).unwrap();
    assert!(text.contains(&format!("expect = [\"{id}\"]")) && text.contains("rationale = \"auto\""), "{text}");
    assert!(text.contains("expect = []"), "{text}");
    let (v, code) = env.json(&["eval", "--reviewed", draft.to_str().unwrap()], "");
    assert_eq!(code, Some(0), "the draft parses: {v}");
    assert_eq!(v["metrics"]["samples"], 2, "{v}");
    assert_eq!(v["metrics"]["unavailable"], 0, "a drafted failure can feed --pick: {v}");
    let before = std::fs::read(&draft).unwrap();
    let (v, code) = env.json(&["eval", "--draft-reviewed", draft.to_str().unwrap()], "");
    assert_eq!(code, Some(1), "{v}");
    assert_eq!(std::fs::read(&draft).unwrap(), before, "the file is unchanged");
}

#[test]
fn eval_pick_prints_hooks_lines() {
    let env = kb_with_topics();
    let cmake = add_ok(&env, "cmake", &lesson_with("Cmake prefix lesson"))["id"].as_str().unwrap().to_string();
    let vtable = add_ok(
        &env,
        "cpp",
        "---\ntype: pitfall\nverified_how: ran\n---\n\n# Vtable linker lesson\n\n## Symptom\nUndefined reference to vtable for Widget.\n\n## Cause\nA missing virtual definition.\n\n## Fix\nDefine the virtual.\n\n## Evidence\nIt linked.\n",
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    let ninja = add_ok(
        &env,
        "cmake",
        "---\ntype: fact\nverified_how: ran\n---\n\n# Ninja lesson\n\n## Statement\nNinja is faster here.\n\n## Evidence\nSaw it.\n",
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    set_chain(&env, "chain = [\"jev\", \"bm25\"]");
    jev_config(&env, "[jev]\napi_key = \"k\"\n", 0o600);
    let kb_toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    env.write(
        "kb.toml",
        &kb_toml.replace("allow = { sensitivity = [\"public\"] }", "allow = { sensitivity = [\"public\", \"internal\"] }"),
    );
    let (url, _log) = fake_jev(|body| {
        let state = body["state"].as_str().unwrap_or_default();
        let top = if state.contains("alpha") {
            0.9
        } else if state.contains("beta") {
            0.82
        } else if state.contains("gamma") {
            0.78
        } else {
            0.72
        };
        let answers: serde_json::Map<String, serde_json::Value> = body["questions"]
            .as_object()
            .unwrap()
            .keys()
            .map(|k| {
                let v = if k == "lesson_0" { top } else { 0.0 };
                (k.clone(), serde_json::json!({ "type": "noul", "noul": v }))
            })
            .collect();
        (200, serde_json::json!({ "answers": answers }).to_string())
    });
    let head = stdout(&env.git(&["rev-parse", "HEAD"])).trim().to_string();
    let event = |key: &str, error: &str, expect: &str| {
        format!(
            "\n[[event]]\nkey = '{key}'\npath = 'failure'\ntext = '{error}'\nerror = '{error}'\ncommand = 'cmake -B build'\nexpect = {expect}\nworked = 'hand review'\nsource = 'hand'\nrationale = 'hand'\n"
        )
    };
    let reviewed = format!(
        "version = 1\nkb_revision = '{head}'\nsource_fingerprint = 'test'\n{}\n",
        event("e1", "cmake error: no CMake config file, set HDF5_ROOT alpha", &format!("[\"{cmake}\"]"))
            + &event("e2", "cmake error: no CMake config file, set HDF5_ROOT beta", &format!("[\"{cmake}\"]"))
            + &event("e3", "cmake error: no CMake config file, set HDF5_ROOT gamma", &format!("[\"{ninja}\"]"))
            + &event("e4", "cmake error: no CMake config file, set HDF5_ROOT delta", &format!("[\"{vtable}\"]"))
    );
    let path = env.dir.path().join("reviewed.toml");
    std::fs::write(&path, &reviewed).unwrap();
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_JEV_URL", &url);
    let o = rkb_with(c, &["eval", "--reviewed", path.to_str().unwrap(), "--pick", "0.95", "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)));
    assert_eq!(v["picks"]["jev"]["failure"], serde_json::json!({ "min": 0.8, "margin": 0.0 }), "{v}");
    assert_eq!(v["picks"]["jev"]["prompt"], serde_json::Value::Null, "{v}");
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_JEV_URL", &url);
    let o = rkb_with(c, &["eval", "--reviewed", path.to_str().unwrap(), "--pick", "0.95"], "");
    let human = stdout(&o);
    assert!(human.contains("[hooks.thresholds.jev]") && human.contains("failure = { min = 0.8, margin = 0.0 }"), "{human}");
    assert!(!human.contains("prompt = "), "no prompt line: {human}");
    assert!(human.contains("prompt jev"), "it says so: {human}");
}

fn pitfall_with(title: &str, symptom: &str, cause: &str, fix: &str) -> String {
    format!(
        "---\ntype: pitfall\nverified_how: ran\ntags:\n  - demo\n---\n\n# {title}\n\n## Symptom\n{symptom}\n\n## Cause\n{cause}\n\n## Fix\n{fix}\n\n## Evidence\nSaw it.\n"
    )
}

#[test]
fn eval_first_stage_misses() {
    let env = kb_with_topics();
    let id = add_ok(
        &env,
        "cmake",
        &pitfall_with(
            "Copper solder joint seeps under mains pressure",
            "Soldered copper joint drips when the line is pressurized.",
            "Heat was uneven.",
            "Clean and reheat evenly, then test again.",
        ),
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    let head = stdout(&env.git(&["rev-parse", "HEAD"])).trim().to_string();
    let event = |key: &str, error: &str| {
        format!(
            "\n[[event]]\nkey = '{key}'\npath = 'failure'\ntext = '{error}'\nerror = '{error}'\ncommand = 'zzprog run'\nexpect = [\"{id}\"]\nworked = 'hand review'\nsource = 'hand'\nrationale = 'hand review'\n"
        )
    };
    let reviewed = format!(
        "version = 1\nkb_revision = '{head}'\nsource_fingerprint = 'test'\n{}\n",
        event("e1", "error: copper solder joint seeps under pressure")
            + &event("e2", "error: soldered copper joint drips")
            + &event("e3", "error: mains pressure seeps at the joint")
            + &event("e4", "error: zebra quantum banana xylophone")
    );
    let path = env.dir.path().join("reviewed.toml");
    std::fs::write(&path, &reviewed).unwrap();
    let (v, code) = env.json(&["eval", "--reviewed", path.to_str().unwrap(), "--first-stage"], "");
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["paths"]["failure"]["answerable"], 4, "{v}");
    assert_eq!(v["paths"]["failure"]["misses"], 1, "{v}");
    assert_eq!(v["paths"]["failure"]["rate"], serde_json::json!(0.25), "{v}");
    assert_eq!(v["paths"]["prompt"]["answerable"], 0, "an empty path counts: {v}");
    assert_eq!(v["paths"]["prompt"]["misses"], 0, "{v}");
    assert!(v["paths"]["prompt"]["rate"].is_null(), "no divide by zero: {v}");
    let human = stdout(&env.rkb(&["eval", "--reviewed", path.to_str().unwrap(), "--first-stage"]));
    assert!(human.contains("failure 1 of 4"), "{human}");
    assert!(human.contains("0.25"), "{human}");
    assert!(human.contains("prompt 0 of 0"), "{human}");
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "", "first-stage writes nothing");
    let o = env.rkb(&["eval", "--first-stage"]);
    assert_eq!(o.status.code(), Some(2), "--first-stage needs --reviewed: {}", stdout(&o));
}

#[test]
fn eval_first_stage_paraphrases() {
    let env = kb_with_topics();
    let lessons = [
        (
            "Copper solder joint seeps under mains pressure",
            "Soldered copper joint drips when the line is pressurized.",
            "Heat was uneven.",
            "Clean and reheat evenly, then test again.",
        ),
        (
            "Tomato seedlings wilt after midday transplant",
            "Fresh tomato transplants droop within hours of midday planting.",
            "Roots dried out.",
            "Water deeply and shade them for a day.",
        ),
        (
            "Bicycle chain skips under heavy pedal load",
            "The chain jumps teeth when I stand on the pedals uphill.",
            "The links are worn.",
            "Fit a new link set and check the gears.",
        ),
        (
            "Sourdough loaf collapses during overnight proof",
            "The shaped loaf flattens and spreads instead of rising overnight.",
            "The culture was weak.",
            "Feed the culture twice, then bake early.",
        ),
    ];
    let ids: Vec<String> = lessons
        .iter()
        .map(|(title, symptom, cause, fix)| {
            add_ok(&env, "cmake", &pitfall_with(title, symptom, cause, fix))["id"].as_str().unwrap().to_string()
        })
        .collect();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/eval/paraphrase.toml");
    let text = std::fs::read_to_string(&fixture).unwrap();
    let doc: toml::Value = toml::from_str(&text).unwrap();
    let entries = doc["paraphrase"].as_array().unwrap();
    assert_eq!(entries.len(), 20, "the fixture holds 20 paraphrases");
    let head = stdout(&env.git(&["rev-parse", "HEAD"])).trim().to_string();
    let mut reviewed = format!("version = 1\nkb_revision = '{head}'\nsource_fingerprint = 'test'\n");
    for (i, e) in entries.iter().enumerate() {
        let slot = e["slot"].as_integer().unwrap() as usize;
        let q = e["text"].as_str().unwrap().replace('\'', "’");
        reviewed.push_str(&format!(
            "\n[[event]]\nkey = 'p{}'\npath = 'prompt'\ntext = '{}'\nexpect = [\"{}\"]\nworked = 'hand review'\nsource = 'hand'\nrationale = 'hand review'\nparaphrase = true\n",
            i + 1,
            q,
            ids[slot]
        ));
    }
    let path = env.dir.path().join("reviewed.toml");
    std::fs::write(&path, &reviewed).unwrap();
    let (v, code) = env.json(&["eval", "--reviewed", path.to_str().unwrap(), "--first-stage"], "");
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["paraphrase"]["answerable"], 20, "{v}");
    assert_eq!(v["paths"]["prompt"]["answerable"], 20, "paraphrases count also in their path: {v}");
    assert_eq!(v["paraphrase"]["misses"], v["paths"]["prompt"]["misses"], "same events: {v}");
    assert!(v["paraphrase"]["rate"].is_number(), "its own rate: {v}");
    let human = stdout(&env.rkb(&["eval", "--reviewed", path.to_str().unwrap(), "--first-stage"]));
    assert!(human.contains("paraphrase") && human.contains("of 20"), "the 20-entry line exists: {human}");
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "", "first-stage writes nothing");
}

#[test]
fn hook_thresholds_from_kb_toml() {
    let env = kb_with_topics();
    let id = add_ok(&env, "cmake", &lesson_with("Cmake prefix lesson"))["id"].as_str().unwrap().to_string();
    set_chain(&env, "chain = [\"jev\", \"bm25\"]");
    jev_config(&env, "[jev]\napi_key = \"k\"\n", 0o600);
    let kb_toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    let base = kb_toml.replace("allow = { sensitivity = [\"public\"] }", "allow = { sensitivity = [\"public\", \"internal\"] }");
    env.write("kb.toml", &base);
    let (url, _log) = fake_jev(|body| {
        let answers: serde_json::Map<String, serde_json::Value> = body["questions"]
            .as_object()
            .unwrap()
            .keys()
            .map(|k| {
                let v = if k == "lesson_0" { 0.9 } else { 0.0 };
                (k.clone(), serde_json::json!({ "type": "noul", "noul": v }))
            })
            .collect();
        (200, serde_json::json!({ "answers": answers }).to_string())
    });
    let run_hook = |event: &str, payload: &serde_json::Value| {
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.env("RKB_JEV_URL", &url);
        let o = rkb_with(c, &["hook", event], &payload.to_string());
        assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));
        stdout(&o)
    };
    let failed = |session: &str| {
        serde_json::json!({
            "session_id": session, "cwd": env.kb(), "tool_name": "Bash",
            "tool_input": { "command": "cmake -B build" },
            "error": "cmake error: no CMake config file, set HDF5_ROOT",
        })
    };
    let prompt =
        |session: &str| serde_json::json!({ "session_id": session, "cwd": env.kb(), "prompt": "cmake cannot find the HDF5 prefix here" });
    assert!(run_hook("tool-failed", &failed("s1")).contains(&id), "no table, the hook uses its defaults");
    assert!(run_hook("prompt", &prompt("s2")).contains(&id), "no table on the prompt path either");
    env.write("kb.toml", &format!("{base}\n[hooks.thresholds.jev]\nfailure = {{ min = 0.99, margin = 0.1 }}\n"));
    assert_eq!(run_hook("tool-failed", &failed("s3")), "", "a 0.9 top is below the 0.99 failure threshold");
    assert!(run_hook("prompt", &prompt("s4")).contains(&id), "no prompt table, the prompt path keeps its defaults");
    env.write(
        "kb.toml",
        &format!("{base}\n[hooks.thresholds.jev]\nfailure = {{ min = 0.99, margin = 0.1 }}\nprompt = {{ min = 0.99, margin = 0.1 }}\n"),
    );
    assert_eq!(run_hook("prompt", &prompt("s5")), "", "the prompt table holds the prompt path back");
    let out = stdout(&env.rkb(&["lint", "--format", "json"]));
    assert!(!out.contains("thresholds"), "a valid table lints clean: {out}");
    env.write("kb.toml", &format!("{base}\n[hooks.thresholds.jev]\nbogus = {{ min = 0.9, margin = 0.1 }}\n"));
    let out = stdout(&env.rkb(&["lint", "--format", "json"]));
    assert!(out.contains("config/thresholds"), "an unknown path is reported: {out}");
}

#[test]
fn session_start_leaves_the_inbox_to_status() {
    let env = search_kb();
    for i in 0..5 {
        inbox_item(&env, &format!("00000000d{i}"), "note", &[], "a note to distill");
    }
    let payload = serde_json::json!({ "session_id": "s1", "source": "startup", "cwd": env.kb() });
    let out = hook(&env, "session-start", &payload);
    assert!(!out.contains("inbox") && out.contains("rkb search"), "{out}");
    let o = env.rkb_in(&["hook", "session-start", "--harness", "pi"], &payload.to_string());
    let pi = stdout(&o);
    assert!(!pi.contains("inbox"), "pi too: {pi}");
    let (v, _) = env.json(&["status"], "");
    assert_eq!(v["inbox"]["total"], 5, "status still shows the inbox: {v}");
}

#[test]
fn hook_skips_a_lesson_that_keeps_missing() {
    let env = search_kb();
    let id = "1a00000012";
    let cwd = env.kb();
    let linker = "Exit code 1\n/usr/bin/ld: main.o: in function `main':\nmain.cpp:(.text+0x1f): undefined reference to `vtable for Widget'\ncollect2: error: ld returned 1 exit status\n";
    let failed = |session: &str| {
        let mut p = bash(session, &cwd, "g++ main.o -o app");
        p["error"] = linker.into();
        p
    };
    for _ in 0..3 {
        let (v, _) = env.json(&["used", id, "--worked"], "");
        assert_eq!(v["status"], "recorded");
    }
    let out = hook(&env, "tool-failed", &failed("s-proven"));
    assert!(out.contains(id), "proven lesson still pushes: {out}");
    for i in 0..3 {
        let (v, _) = env.json(&["used", id, "--irrelevant", "--reason", &format!("miss {i}")], "");
        assert_eq!(v["status"], "recorded", "{v}");
        assert!(v.get("demote").is_none(), "stays active for the mute rule: {v}");
    }
    let out = hook(&env, "tool-failed", &failed("s-muted"));
    assert!(!out.contains(id) && !out.contains("<rkb-lesson"), "3 misses stop the push: {out}");
    let (v, _) = env.json(&["search", "undefined reference to vtable"], "");
    assert!(v["results"].as_array().unwrap().iter().any(|r| r["id"] == id), "search still lists it: {v}");
    let (v, _) = env.json(&["used", id, "--worked"], "");
    assert_eq!(v["status"], "recorded");
    let out = hook(&env, "tool-failed", &failed("s-worked-again"));
    assert!(out.contains(id), "a new worked report pushes again: {out}");
}

#[test]
fn worked_adds_the_failure_query() {
    let env = search_kb();
    let id = "1a00000012";
    let cwd = env.kb();
    let linker = "Exit code 1\n/usr/bin/ld: main.o: in function `main':\nmain.cpp:(.text+0x1f): undefined reference to `vtable for Widget'\ncollect2: error: ld returned 1 exit status\n";
    let failed = |session: &str| {
        let mut p = bash(session, &cwd, "g++ main.o -o app");
        p["error"] = linker.into();
        p
    };
    let queries_of = |env: &Env| -> Vec<String> {
        show(env, id)["frontmatter"]["queries"]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
            .unwrap_or_default()
    };
    let set_queries = |env: &Env, qs: &[&str]| {
        let (text, hash) = current(env, id);
        let block = if qs.is_empty() {
            String::new()
        } else {
            format!("queries:\n{}", qs.iter().map(|q| format!("  - {q}\n")).collect::<String>())
        };
        let edited = if let Some(start) = text.find("\nqueries:\n") {
            let after = &text[start + "\nqueries:\n".len()..];
            let mut end = start + "\nqueries:\n".len();
            for line in after.split_inclusive('\n') {
                if line.starts_with("  - ") {
                    end += line.len();
                } else {
                    break;
                }
            }
            format!("{}{}{}", &text[..start + 1], block, &text[end..])
        } else {
            text.replacen("tags:\n  - linker\n", &format!("tags:\n  - linker\n{block}"), 1)
        };
        let (v, _) = env.json(&["edit", id, "--base", &hash], &edited);
        assert_eq!(v["status"], "written", "{v}");
    };
    let replay_query = |env: &Env, session: &str| -> String {
        std::fs::read_to_string(env.dir.path().join("state/rkb/replays.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .rev()
            .find(|r| r["session"] == session && r["injected"] == id)
            .unwrap_or_else(|| panic!("no replay for {session}"))["query"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let commits = || stdout(&env.git(&["rev-list", "--count", "HEAD"])).trim().parse::<u32>().unwrap();
    set_queries(&env, &["undefined reference to vtable", "linker cannot find vtable"]);
    assert_eq!(queries_of(&env).len(), 2);
    let out = hook(&env, "tool-failed", &failed("s-worked"));
    assert!(out.contains(id), "hook pushes for the failure: {out}");
    let query = replay_query(&env, "s-worked");
    assert!(query.starts_with("g++ ") && query.contains("vtable"), "{query}");
    let before = commits();
    let (v, _) = env.json(&["used", id, "--worked", "--session", "s-worked"], "");
    assert_eq!(v["status"], "recorded", "{v}");
    let qs = queries_of(&env);
    assert_eq!(qs.len(), 3, "{qs:?}");
    assert!(qs.contains(&query), "the pushed failure query is added: {qs:?} vs {query}");
    assert_eq!(commits(), before + 1, "one commit says so");
    let before = commits();
    let (v, _) = env.json(&["used", id, "--worked", "--session", "s-worked"], "");
    assert_eq!(v["status"], "recorded");
    assert_eq!(queries_of(&env).len(), 3, "already there adds nothing");
    assert_eq!(commits(), before, "no new commit");
    set_queries(&env, &["q1", "q2", "q3", "q4", "q5", "q6", "q7", "q8"]);
    let out = hook(&env, "tool-failed", &failed("s-full"));
    assert!(out.contains(id), "full lesson still pushes: {out}");
    let before = commits();
    let (v, _) = env.json(&["used", id, "--worked", "--session", "s-full"], "");
    assert_eq!(v["status"], "recorded");
    assert_eq!(queries_of(&env).len(), 8, "full stays full");
    assert_eq!(commits(), before, "full adds no commit");
}

fn promote_item(env: &Env, id: &str, sig: &str, body: &str, answer: &str) -> serde_json::Value {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let dir = env.dir.path().join("state/rkb/inbox");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{id}.md")), format!("---\nkind: note\ntime: {now}\nsignatures:\n  - {sig}\n---\n\n{body}\n")).unwrap();
    std::fs::write(
        env.dir.path().join("state/rkb/signatures.jsonl"),
        format!("{{\"time\":{now},\"sig\":\"{sig}\",\"session\":\"s1\"}}\n{{\"time\":{now},\"sig\":\"{sig}\",\"session\":\"s2\"}}\n"),
    )
    .unwrap();
    let fake = env.dir.path().join("fake/pi");
    env.write_abs(&fake, &format!("#!/bin/sh\ncat > /dev/null\nprintf '%s' '{answer}'\n"));
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let o = rkb_with(with_pi(env), &["promote", "--harness", "pi", "--format", "json"], "");
    serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)))
}

fn promote_env() -> Env {
    let env = Env::new();
    env.init();
    env.write_abs(&env.dir.path().join("config/rkb/config.toml"), "[observer]\npi = \"session\"\n");
    approve_as_claude(&env);
    env
}

#[test]
fn promote_skips_a_one_off_preference() {
    let env = promote_env();
    let v = promote_item(
        &env,
        "0a1b2c3d4e",
        "aaabbbcccddd",
        "use ninja for this build only",
        r#"{"action":"add","topic":"cmake","type":"preference","title":"Use ninja for this build","queries":["ninja for this build"],"sections":{"Rule":"Use ninja for this build only.","Why":"The user asked for it this time.","How to apply":"Pass -G Ninja this once."},"quotes":["use ninja for this build only"],"standing":false}"#,
    );
    assert_eq!((v["promoted"].as_u64(), v["ready"].as_u64()), (Some(0), Some(1)), "{v}");
    assert_eq!(v["results"][0]["action"], "noop", "{v}");
    assert!(v["results"][0]["reason"].as_str().unwrap().contains("one-off"), "the reason names the one-off: {v}");
    let (found, _) = env.json(&["search", "ninja for this build"], "");
    assert!(found["results"].as_array().unwrap().is_empty(), "nothing written: {found}");
    assert!(inbox_items(&env).is_empty(), "the answered item is gone");
    let log = std::fs::read_to_string(env.dir.path().join("state/rkb/promotions.jsonl")).unwrap();
    let entry: serde_json::Value = serde_json::from_str(log.lines().last().unwrap()).unwrap();
    assert_eq!(entry["action"], "noop", "{log}");
    assert!(entry["reason"].as_str().unwrap().contains("one-off"), "the log names the one-off: {log}");
    // A pitfall the model marks one-off still promotes: the flag fits failures, not lesson types.
    let v = promote_item(
        &env,
        "0a1b2c3d4f",
        "eeefff000111",
        "cmake could not find HDF5 on this machine",
        r#"{"action":"add","topic":"cmake","type":"pitfall","title":"Cmake needs the prefix","queries":["cmake could not find HDF5"],"sections":{"Symptom":"cmake could not find HDF5 on this machine","Cause":"Not known yet.","Fix":"Set the prefix.","Evidence":"saw it"},"quotes":["cmake could not find HDF5"],"standing":false}"#,
    );
    assert_eq!(v["promoted"].as_u64(), Some(1), "a pitfall ignores the one-off flag: {v}");
    let (found, _) = env.json(&["search", "cmake could not find HDF5"], "");
    assert_eq!(found["results"].as_array().unwrap().len(), 1, "{found}");
}

#[test]
fn session_start_adds_the_digest_when_on() {
    let env = kb_with_topics();
    add_ok(
        &env,
        "cmake",
        &preference_with("Always pass the module prefix explicitly", "Pass the module prefix explicitly on every configure."),
    );
    // A pitfall must not leak into the digest.
    add_ok(&env, "cmake", PITFALL);
    let payload = serde_json::json!({ "session_id": "s", "cwd": env.dir.path(), "source": "startup" });
    let off = hook(&env, "session-start", &payload);
    assert!(!off.contains("module prefix"), "off by default: {off}");
    let before = off.chars().count();
    let toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    env.write("kb.toml", &format!("{toml}\n[hooks]\ndigest = true\n"));
    assert!(env.rkb(&["lint"]).status.success(), "{}", stdout(&env.rkb(&["lint"])));
    let on = hook(&env, "session-start", &payload);
    assert!(on.contains("module prefix"), "the digest follows the context lines: {on}");
    assert!(on.find("rkb search").unwrap() < on.find("module prefix").unwrap(), "after the context lines: {on}");
    assert!(!on.contains("Configure fails"), "the pitfall stays out: {on}");
    // The second command reads the digest the first wrote: session start uses it.
    let (v, _) = env.json(&["digest"], "");
    let rule = v["lines"][0]["line"].as_str().unwrap();
    assert!(on.contains(rule), "session start prints the real digest line: {on}");
    assert!(on.chars().count() <= before + 1200 + 200, "{} chars on, {} off", on.chars().count(), before);
}

#[test]
fn skill_draft_lists_and_writes() {
    let env = kb_with_topics();
    let recipe = |title: &str, when: &str, step: &str| {
        format!(
            "---\ntype: recipe\nverified_how: ran\ntags:\n  - demo\n---\n\n# {title}\n\n## When to use\n{when}\n\n## Steps\n{step}\n\n## Evidence\nIt worked twice.\n"
        )
    };
    let a = add_ok(
        &env,
        "cmake",
        &recipe("Draft a skill from a repeated recipe", "A recipe repeats in three sessions.", "1. Run the tool.\n2. Check the output."),
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    let b = add_ok(&env, "cmake", &recipe("A recipe with only two sessions", "It repeated only twice.", "1. Do the thing."))["id"]
        .as_str()
        .unwrap()
        .to_string();
    // A pitfall and a stale recipe never become skills, however often they worked.
    let pitfall = add_ok(&env, "cmake", PITFALL)["id"].as_str().unwrap().to_string();
    let stale = add_ok(&env, "cmake", &recipe("A recipe that stopped working", "It worked until now.", "1. Try it."))["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(env.json(&["flag", &stale, "--reason", "the tool changed"], "").0["status"], "written");

    // Real reports, each under its own session of the harness.
    let worked_in = |session: &str, id: &str| {
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        c.env("RKB_SESSION", session);
        let o = rkb_with(c, &["used", id, "--worked", "--format", "json"], "");
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", stdout(&o)));
        assert_eq!(v["status"].as_str(), Some("recorded"), "{v}");
    };
    for s in ["s1", "s2", "s3"] {
        worked_in(s, &a);
        worked_in(s, &pitfall);
        worked_in(s, &stale);
    }
    for s in ["s1", "s2"] {
        worked_in(s, &b);
    }

    // Scenario: one candidate. Only the recipe with three sessions is listed, with 3.
    let (v, code) = env.json(&["skill-draft"], "");
    assert_eq!(code, Some(0), "{v}");
    let rows = v["candidates"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "one candidate: {v}");
    assert_eq!((rows[0]["id"].as_str(), rows[0]["sessions"].as_u64()), (Some(a.as_str()), Some(3)), "{v}");
    assert!(rows[0]["title"].as_str().unwrap().contains("repeated recipe"), "{v}");
    let human = stdout(&env.rkb(&["skill-draft"]));
    assert!(human.contains(&format!("{a}  3  ")), "{human}");
    assert!(!human.contains(&b) && !human.contains(&pitfall) && !human.contains(&stale), "{human}");

    // Scenario: not a candidate. Each refusal exits 1 with a fix and writes nothing.
    let drafts = env.dir.path().join("state/rkb/skill-drafts");
    let (v, code) = env.json(&["skill-draft", &b], "");
    assert_eq!(code, Some(1), "{v}");
    assert!(v["error"]["code"].as_str().is_some_and(|c| !c.is_empty() && c != "usage"), "{v}");
    assert!(v["error"]["fix"].as_str().is_some_and(|f| !f.trim().is_empty()), "{v}");
    let (v, code) = env.json(&["skill-draft", &pitfall], "");
    assert_eq!((v["error"]["code"].as_str(), code), (Some("refused"), Some(1)), "not a recipe: {v}");
    let (v, code) = env.json(&["skill-draft", &stale], "");
    assert_eq!((v["error"]["code"].as_str(), code), (Some("refused"), Some(1)), "not active: {v}");
    let (v, code) = env.json(&["skill-draft", "0000000000"], "");
    assert_eq!((v["error"]["code"].as_str(), code), (Some("not_found"), Some(1)), "{v}");
    assert!(!drafts.exists(), "a refused draft writes nothing: {}", drafts.display());

    let commits = commit_count(&env);
    let status = stdout(&env.git(&["status", "--porcelain"]));

    // Scenario: a draft. The file holds the description and the steps; the knowledge base does not move.
    let (v, code) = env.json(&["skill-draft", &a], "");
    assert_eq!(code, Some(0), "{v}");
    let path = PathBuf::from(v["path"].as_str().unwrap());
    assert!(path.starts_with(&drafts) && path.file_name().unwrap() == "SKILL.md", "{}", path.display());
    let text = std::fs::read_to_string(&path).unwrap();
    let (fm, body, _) = rkb_core::lesson::split(&text).unwrap();
    let fm: serde_norway::Value = serde_norway::from_str(fm).unwrap();
    assert_eq!(fm["name"].as_str(), path.parent().unwrap().file_name().unwrap().to_str(), "{text}");
    let description = fm["description"].as_str().unwrap();
    assert!(description.starts_with("Load before you "), "{text}");
    assert_eq!(description, "Load before you a recipe repeats in three sessions.", "{text}");
    assert!(!description.contains('\n'), "the description is one line: {text}");
    assert!(body.trim_start().starts_with("1. Run the tool."), "{text}");
    assert!(body.trim_end().ends_with(&format!("Source: rkb show {a}")), "{text}");
    assert!(!body.contains("## When to use") && !body.contains("## Evidence"), "only the steps: {text}");

    // Scenario: a second run writes the same file again.
    let human = stdout(&env.rkb(&["skill-draft", &a]));
    assert!(human.contains(v["path"].as_str().unwrap()), "{human}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "the same file, written again");
    let folders: Vec<_> = std::fs::read_dir(&drafts).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(folders.len(), 1, "one draft folder, overwritten: {folders:?}");

    assert_eq!(commit_count(&env), commits, "no commit");
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), status, "no file changed");

    // A `worked` report with no session counts by its own date, so `b` reaches three sessions.
    let (v, code) = env.json(&["used", &b, "--worked"], "");
    assert_eq!((v["status"].as_str(), code), (Some("recorded"), Some(0)), "{v}");
    let (v, _) = env.json(&["skill-draft"], "");
    let mut ids: Vec<&str> = v["candidates"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    ids.sort_unstable();
    let mut want = vec![a.as_str(), b.as_str()];
    want.sort_unstable();
    assert_eq!(ids, want, "the dated report counted as a session: {v}");
}

#[test]
fn agent_sink_hides_labels() {
    let env = Env::new();
    env.init();
    let kb_toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    env.write("kb.toml", &format!("{kb_toml}\n[sinks.agent]\nallow = {{ sensitivity = [\"public\", \"internal\"] }}\n"));

    let lesson = |id: &str, title: &str, label: Option<&str>, tag: &str, statement: &str| {
        let labels = label.map_or(String::new(), |l| format!("labels:\n  sensitivity: {l}\n"));
        format!(
            "---\nschema: 1\nid: {id}\ntype: fact\nstatus: active\nverified: 2026-09-25\nverified_how: ran\ntags:\n  - {tag}\n{labels}---\n\n# {title}\n\n## Statement\n{statement}\n\n## Evidence\nSeen in a test.\n"
        )
    };
    let public = |n: u8| {
        let (id, extra) = match n {
            1 => ("b000000001".to_string(), " See also [the cipher](secret-own.md).".to_string()),
            n => (format!("b00000000{n}"), String::new()),
        };
        lesson(
            &id,
            &format!("Squirrel cache note {n}"),
            Some("public"),
            if n == 1 { "acorn" } else { "plain" },
            &format!("The squirrel cache tunnels flood every spring, note {n}.{extra}"),
        )
    };
    for n in 1..=5 {
        env.write(&format!("general/nuts/note-{n}.md"), &public(n));
    }
    env.write(
        "general/nuts/secret-own.md",
        &lesson(
            "b000000006",
            "Squirrel cache cipher",
            Some("confidential"),
            "acorn",
            "The squirrel cache cipher hides the winter tunnels.",
        ),
    );
    env.write("general/vault/README.md", "---\nlabels:\n  sensitivity: confidential\n---\n\n# Vault\n");
    env.write(
        "general/vault/buried.md",
        &lesson("b000000007", "Buried squirrel cache map", None, "buried", "The buried squirrel cache map marks the north tunnels."),
    );
    env.write(
        "general/nuts/ledger.md",
        &lesson("b000000008", "Squirrel cache ledger", Some("internal"), "ledger", "The squirrel cache ledger counts every tunnel."),
    );
    assert!(env.git(&["add", "-A"]).status.success());
    let o = env.git(&["commit", "-q", "-m", "sink fixture"]);
    assert!(o.status.success(), "{}{}", stdout(&o), String::from_utf8_lossy(&o.stderr));

    let tool_json = |name: &str, args: &serde_json::Value| {
        let o = env.rkb_in(&["tool", name, "--format", "json"], &args.to_string());
        (serde_json::from_slice::<serde_json::Value>(&o.stdout).unwrap(), o.status.code())
    };
    let ids =
        |v: &serde_json::Value| v["results"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();

    // The user CLI stays unfiltered: every matching lesson comes back.
    let (v, _) = env.json(&["search", "squirrel cache"], "");
    let mine = ids(&v);
    for id in ["b000000006", "b000000007", "b000000008"] {
        assert!(mine.contains(&id.to_string()), "the user still sees {id}: {mine:?}");
    }

    // The agent search hides the confidential lessons, with no id, title or count of them.
    let (v, code) = tool_json("rkb_search", &serde_json::json!({ "query": "squirrel cache" }));
    assert_eq!(code, Some(0), "{v}");
    let mine = ids(&v);
    assert_eq!(mine.len(), 6, "five public plus one internal: {mine:?}");
    let text = serde_json::to_string(&v).unwrap();
    for hidden in ["b000000006", "b000000007", "Squirrel cache cipher", "Buried squirrel cache map"] {
        assert!(!mine.contains(&hidden.to_string()) && !text.contains(hidden), "no trace of {hidden}: {text}");
    }
    assert!(v.get("hidden").is_none() && v.get("hidden_by").is_none(), "no count that could include hidden lessons: {v}");

    // The filter runs before the limit: a limit of 5 still returns 5 allowed lessons.
    let (v, code) = tool_json("rkb_search", &serde_json::json!({ "query": "squirrel cache", "limit": 5 }));
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(ids(&v).len(), 5, "{v}");

    // An agent show of a hidden lesson is refused with the exact error text.
    let (v, code) = tool_json("rkb_show", &serde_json::json!({ "id": "b000000006" }));
    assert_eq!(code, Some(1), "{v}");
    assert_eq!(v["error"]["message"].as_str().unwrap(), "lesson b000000006 is not available to agents on this machine", "{v}");
    assert!(!v["error"]["fix"].as_str().unwrap().is_empty(), "{v}");
    assert!(!serde_json::to_string(&v).unwrap().contains("winter tunnels"), "no lesson text leaks: {v}");

    // A missing lesson reads exactly the same through the agent.
    let (v, code) = tool_json("rkb_show", &serde_json::json!({ "id": "0000000000" }));
    assert_eq!(code, Some(1), "{v}");
    assert_eq!(v["error"]["message"].as_str().unwrap(), "lesson 0000000000 is not available to agents on this machine", "{v}");

    // An allowed show comes back without the hidden related lesson; the user show keeps it.
    let (v, code) = tool_json("rkb_show", &serde_json::json!({ "id": "b000000001" }));
    assert_eq!(code, Some(0), "{v}");
    let related: Vec<String> = v["related"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect();
    assert!(!related.contains(&"b000000006".to_string()), "hidden related dropped: {related:?}");
    assert!(!serde_json::to_string(&v).unwrap().contains("Squirrel cache cipher"), "no hidden title: {v}");
    let (v, _) = env.json(&["show", "b000000001"], "");
    assert!(v["related"].as_array().unwrap().iter().any(|r| r["id"] == "b000000006"), "the user still sees it: {v}");

    // The machine config wins whole over kb.toml: public-only there hides the internal lesson.
    let kb_toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    env.write(
        "kb.toml",
        &kb_toml.replace(
            "allow = { sensitivity = [\"public\", \"internal\"] }",
            "allow = { sensitivity = [\"public\", \"internal\", \"confidential\"] }",
        ),
    );
    env.write_abs(&env.dir.path().join("config/rkb/config.toml"), "[sinks.agent]\nallow = { sensitivity = [\"public\"] }\n");
    let (v, code) = tool_json("rkb_search", &serde_json::json!({ "query": "squirrel cache" }));
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(ids(&v).len(), 5, "only the public lessons: {v:?}");

    // A machine config that does not parse fails closed with an error and a fix, never lessons.
    env.write_abs(&env.dir.path().join("config/rkb/config.toml"), "[sinks.agent\n");
    let (v, code) = tool_json("rkb_search", &serde_json::json!({ "query": "squirrel cache" }));
    assert_eq!(code, Some(1), "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("config.toml"), "{v}");
    assert!(!v["error"]["fix"].as_str().unwrap().is_empty(), "{v}");
    assert!(v.get("results").is_none(), "no lessons on failure: {v}");

    // An agent sink with bad fields fails the same way.
    env.write_abs(&env.dir.path().join("config/rkb/config.toml"), "[sinks.agent]\nbogus = 1\n");
    let (v, code) = tool_json("rkb_search", &serde_json::json!({ "query": "squirrel cache" }));
    assert_eq!(code, Some(1), "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("[sinks.agent]"), "{v}");
    assert!(!v["error"]["fix"].as_str().unwrap().is_empty(), "{v}");
}

#[test]
fn agent_sink_blocks_injection() {
    let env = Env::new();
    env.init();
    let pitfall = |id: &str, title: &str, label: &str, symptom: &str, cause: &str, fix: &str, evidence: &str| {
        format!(
            "---\nschema: 1\nid: {id}\ntype: pitfall\nstatus: active\nverified: 2026-09-25\nverified_how: ran\ntags:\n  - link\nlabels:\n  sensitivity: {label}\n---\n\n# {title}\n\n## Symptom\n{symptom}\n\n## Cause\n{cause}\n\n## Fix\n{fix}\n\n## Evidence\n{evidence}\n"
        )
    };
    env.write(
        "general/link/strong.md",
        &pitfall(
            "c000000001",
            "Linker vtable failure",
            "confidential",
            "The link fails with `undefined reference to vtable for Widget` in main.cpp text.",
            "vtable widget undefined reference vtable widget undefined reference.",
            "Define the first virtual function: vtable widget undefined reference.",
            "Seen with g++ and ld; `tunnel cipher mismatch at dawn cache`; `no usable tunnel remains`.",
        ),
    );
    env.write(
        "general/link/plain.md",
        &pitfall(
            "c000000002",
            "Linker vtable miss",
            "public",
            "The link fails with `undefined reference to vtable for Widget` in main.cpp text.",
            "A missing virtual definition confuses the morning build. The morning notebook sits by the river.",
            "Define the missing virtual function for Widget. A pencil rests on the chair.",
            "Seen with g++ and ld. The garden letter waits by the window.",
        ),
    );
    assert!(env.git(&["add", "-A"]).status.success());
    assert!(env.git(&["commit", "-q", "-m", "sink fixture"]).status.success());
    let cwd = env.kb();

    let vtable = "Exit code 1\n/usr/bin/ld: main.o: in function `main':\nmain.cpp:(.text+0x1f): undefined reference to `vtable for Widget'\ncollect2: error: ld returned 1 exit status\n";
    let mut failed = bash("s-control", &cwd, "g++ main.o -o app");
    failed["error"] = vtable.into();
    let out = hook(&env, "tool-failed", &failed);
    assert!(out.contains("c000000001"), "without a sink the confidential lesson pushes: {out}");

    let kb_toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    env.write("kb.toml", &format!("{kb_toml}\n[sinks.agent]\nallow = {{ sensitivity = [\"public\", \"internal\"] }}\n"));
    let replays = || -> Vec<serde_json::Value> {
        std::fs::read_to_string(env.dir.path().join("state/rkb/replays.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    };

    // The hidden lesson never takes the slot: the allowed runner-up is injected instead.
    let mut failed = bash("s2", &cwd, "g++ main.o -o app");
    failed["error"] = vtable.into();
    let out = hook(&env, "tool-failed", &failed);
    assert!(out.contains("c000000002"), "the allowed lesson takes the freed slot: {out}");
    assert!(!out.contains("c000000001"), "no trace of the hidden lesson: {out}");
    assert_eq!(replays().last().unwrap()["injected"], "c000000002", "the replay log names the injected lesson");

    // A failure only the hidden lesson covers injects nothing and logs no id.
    let cipher = "squirrel-cipher: fatal: tunnel cipher mismatch at dawn cache\nsquirrel-cipher: failed: no usable tunnel remains\n";
    let mut failed = bash("s3", &cwd, "squirrel-cipher --rotate");
    failed["error"] = cipher.into();
    assert_eq!(hook(&env, "tool-failed", &failed), "", "nothing is injected");
    let log = replays();
    assert!(log.last().unwrap()["injected"].is_null(), "the replay log records no id: {:?}", log.last().unwrap());
    let session = std::fs::read_to_string(env.dir.path().join("state/rkb/sessions/s3.jsonl")).unwrap();
    assert!(!session.contains("injected"), "no injection recorded: {session}");
    // The preference digest at session start leaves out a hidden preference.
    let preference = |id: &str, label: &str, rule: &str| {
        format!(
            "---\nschema: 1\nid: {id}\ntype: preference\nstatus: active\nverified: 2026-09-25\nverified_how: told\nlabels:\n  sensitivity: {label}\ntags:\n  - style\n---\n\n# {rule}\n\n## Rule\n{rule}\n\n## Why\nThe user said so.\n\n## How to apply\nFollow it.\n"
        )
    };
    env.write("general/link/open-rule.md", &preference("c000000003", "public", "Use short commit subjects."));
    env.write("general/link/secret-rule.md", &preference("c000000004", "confidential", "Name the cipher project only in person."));
    let kb_toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    env.write("kb.toml", &format!("{kb_toml}\n[hooks]\ndigest = true\n"));
    assert!(env.git(&["add", "-A"]).status.success());
    let o = env.git(&["commit", "-q", "-m", "preferences"]);
    assert!(o.status.success(), "{}{}", stdout(&o), String::from_utf8_lossy(&o.stderr));
    let start = hook(&env, "session-start", &serde_json::json!({ "session_id": "s4", "cwd": cwd, "source": "startup" }));
    assert!(start.contains("Use short commit subjects."), "the allowed preference is in the digest: {start}");
    assert!(!start.contains("cipher project"), "the hidden preference stays out: {start}");
    assert!(stdout(&env.rkb(&["digest"])).contains("cipher project"), "the user digest keeps it");
}

#[test]
fn doctor_shows_agent_sink() {
    let env = Env::new();
    env.init();
    let line = || {
        let (v, _) = env.json(&["doctor"], "");
        v["checks"].as_array().unwrap().iter().find(|c| c["check"] == "agent sink").cloned().unwrap()
    };
    let l = line();
    assert_eq!(l["status"], "ok", "{l}");
    assert!(l["detail"].as_str().unwrap().contains("every lesson passes"), "{l}");

    let kb_toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    env.write("kb.toml", &format!("{kb_toml}\n[sinks.agent]\nallow = {{ sensitivity = [\"public\", \"internal\"] }}\n"));
    let l = line();
    assert_eq!(l["status"], "ok", "{l}");
    let detail = l["detail"].as_str().unwrap();
    assert!(detail.contains("kb.toml") && detail.contains("internal"), "{l}");

    env.write_abs(&env.dir.path().join("config/rkb/config.toml"), "[sinks.agent]\nallow = { sensitivity = [\"public\"] }\n");
    let l = line();
    assert_eq!(l["status"], "ok", "{l}");
    let detail = l["detail"].as_str().unwrap();
    assert!(detail.contains("config.toml") && detail.contains("public") && !detail.contains("internal"), "{l}");

    env.write_abs(&env.dir.path().join("config/rkb/config.toml"), "[sinks.agent\n");
    let l = line();
    assert_eq!(l["status"], "fail", "{l}");
    assert!(l["detail"].as_str().unwrap().contains("config.toml"), "{l}");
    assert!(!l["fix"].as_str().unwrap().is_empty(), "{l}");
}

/// The machine config file of a test home, so a test can add `[[extra]]` or a sink.
fn machine_config(env: &Env, text: &str) {
    env.write_abs(&env.dir.path().join("config/rkb/config.toml"), text);
}

fn rev_count(o: &Output) -> usize {
    String::from_utf8_lossy(&o.stdout).trim().parse().unwrap()
}

#[test]
fn extra_kb_search_and_refuse() {
    let env = Env::new();
    env.init();
    let shared = env.dir.path().join("shared");
    let init_shared = env.cmd(env!("CARGO_BIN_EXE_rkb")).env("RKB_HOME", &shared).arg("init").output().unwrap();
    assert!(init_shared.status.success(), "{}", stdout(&init_shared));
    let lesson = |id: &str, title: &str, body: &str| {
        format!(
            "---\nschema: 1\nid: {id}\ntype: fact\nstatus: active\nverified: 2026-09-25\nverified_how: ran\ntags:\n  - cachetunnel\n---\n\n# {title}\n\n## Statement\n{body}\n\n## Evidence\nSeen in a test.\n"
        )
    };
    let write_shared = |rel: &str, text: &str| {
        let p = shared.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
    };
    write_shared("general/team/note.md", &lesson("c000000001", "Shared cache tunnels", "The shared cache tunnels flood every spring."));
    write_shared("general/secret/README.md", "---\nlabels:\n  sensitivity: confidential\n---\n\n# Secret\n");
    write_shared(
        "general/secret/hidden.md",
        &lesson("c000000002", "Hidden shared cache cipher", "The hidden shared cache cipher marks the tunnels."),
    );
    let extra_git = |args: &[&str]| env.cmd("git").arg("-C").arg(&shared).args(args).output().unwrap();
    assert!(extra_git(&["add", "-A"]).status.success());
    assert!(extra_git(&["commit", "-q", "-m", "seed"]).status.success());
    env.write("general/team/main-note.md", &lesson("d000000001", "Main cache tunnels", "The main cache tunnels flood every spring."));
    assert!(env.git(&["add", "-A"]).status.success());
    assert!(env.git(&["commit", "-q", "-m", "main seed"]).status.success());

    machine_config(&env, "[[extra]]\nname = \"Shared KB\"\npath = \"/tmp/shared\"\n");
    let (v, _) = env.json(&["search", "cache tunnels"], "");
    assert_eq!(v["error"]["code"], "refused", "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("Shared KB"), "{v}");
    assert!(!v["error"]["fix"].as_str().unwrap().is_empty(), "{v}");

    machine_config(&env, "[[extra]]\nname = \"shared\"\npath = \"/nonexistent/shared-kb\"\n");
    let (v, code) = env.json(&["search", "main cache tunnels"], "");
    assert_eq!(code, Some(0), "{v}");
    let ids: Vec<&str> = v["results"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"d000000001"), "{v}");
    assert!(v["help"].as_array().unwrap().iter().any(|h| h.as_str().unwrap().contains("shared")), "the skipped extra is named: {v}");

    machine_config(&env, &format!("[[extra]]\nname = \"shared\"\npath = \"{}\"\n", shared.display()));
    let (v, code) = env.json(&["search", "shared cache tunnels"], "");
    assert_eq!(code, Some(0), "{v}");
    let ids: Vec<&str> = v["results"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"shared:c000000001"), "{v}");
    assert!(ids.contains(&"d000000001"), "main and extra rank in one corpus: {v}");
    assert_eq!(ids[0], "shared:c000000001", "the extra title ranks first: {v}");

    let (v, code) = env.json(&["show", "shared:c000000001"], "");
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["id"], "shared:c000000001", "{v}");
    assert!(v["body"].as_str().unwrap().contains("shared cache tunnels flood"), "{v}");
    let hash = v["hash"].as_str().unwrap().to_string();

    let extra_file = shared.join("general/team/note.md");
    let before = std::fs::read_to_string(&extra_file).unwrap();
    let (main_before, extra_before) = (commit_count(&env), rev_count(&extra_git(&["rev-list", "--count", "HEAD"])));
    let (v, code) = env.json(&["edit", "shared:c000000001", "--base", &hash], "changed");
    assert_eq!(code, Some(1), "{v}");
    assert_eq!(v["error"]["code"], "refused", "{v}");
    assert!(v["error"]["fix"].as_str().unwrap().contains("main"), "{v}");
    assert_eq!(std::fs::read_to_string(&extra_file).unwrap(), before, "the extra file is unchanged");
    assert_eq!(
        (commit_count(&env), rev_count(&extra_git(&["rev-list", "--count", "HEAD"]))),
        (main_before, extra_before),
        "no commit in either repo"
    );

    let (v, code) = env.json(&["used", "shared:c000000001", "--worked"], "");
    assert_eq!(code, Some(0), "{v}");
    let usage: String =
        std::fs::read_dir(env.kb().join(".rkb/usage")).unwrap().map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap()).collect();
    assert!(usage.contains("\"id\":\"shared:c000000001\""), "the main usage file records the prefixed id: {usage}");
    let (v, code) = env.json(&["used", "shared:c999999999", "--worked"], "");
    assert_eq!(code, Some(1), "a missing extra lesson records nothing: {v}");

    // A main lesson with the same bare id as an extra one keeps its own use counts in the ranking:
    // the two tie on text, the extra title sorts first, and only the main lesson worked.
    let pair = |title: &str| lesson("c000000003", title, "The pair tunnels flood in autumn.");
    write_shared("general/team/pair.md", &pair("Alpha pair tunnels"));
    assert!(extra_git(&["add", "-A"]).status.success());
    assert!(extra_git(&["commit", "-q", "-m", "pair"]).status.success());
    env.write("general/team/pair.md", &pair("Omega pair tunnels"));
    assert!(env.git(&["add", "-A"]).status.success());
    assert!(env.git(&["commit", "-q", "-m", "pair"]).status.success());
    for _ in 0..5 {
        assert_eq!(env.json(&["used", "c000000003", "--worked"], "").1, Some(0));
    }
    let (v, _) = env.json(&["search", "pair tunnels autumn"], "");
    let ids: Vec<&str> = v["results"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids[..2], ["c000000003", "shared:c000000003"], "the extra lesson does not borrow the main counts: {v}");

    // A folder with a kb.toml that is not a git clone is skipped, not a failed search.
    let plain = env.dir.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::copy(env.kb().join("kb.toml"), plain.join("kb.toml")).unwrap();
    machine_config(
        &env,
        &format!(
            "[[extra]]\nname = \"shared\"\npath = \"{}\"\n\n[[extra]]\nname = \"plain\"\npath = \"{}\"\n",
            shared.display(),
            plain.display()
        ),
    );
    let (v, code) = env.json(&["search", "shared cache tunnels"], "");
    assert_eq!(code, Some(0), "{v}");
    assert!(v["help"].as_array().unwrap().iter().any(|h| h.as_str().unwrap().contains("plain")), "the skipped extra is named: {v}");

    machine_config(
        &env,
        &format!(
            "[sinks.agent]\nallow = {{ sensitivity = [\"internal\"] }}\n\n[[extra]]\nname = \"shared\"\npath = \"{}\"\n",
            shared.display()
        ),
    );
    let (out, _) = tool(&env, "rkb_search", &serde_json::json!({ "query": "hidden shared cache cipher" }));
    assert!(!out.contains("c000000002"), "a confidential extra lesson is hidden: {out}");
    let (out, _) = tool(&env, "rkb_search", &serde_json::json!({ "query": "shared cache tunnels" }));
    assert!(out.contains("shared:c000000001"), "an internal extra lesson passes the sink: {out}");
}

#[test]
fn agent_identity_recorded() {
    let env = Env::new();
    env.init();
    let add = |agent: Option<&str>, title: &str| -> (String, Output) {
        let text = format!(
            "---\ntype: fact\nverified_how: ran\ntags:\n  - agenttag\n---\n\n# {title}\n\n## Statement\nThe agent recorded its name.\n\n## Evidence\nSeen in a test.\n"
        );
        let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
        if let Some(a) = agent {
            c.env("RKB_AGENT", a);
        }
        let o = rkb_with(c, &["add", "--topic", "agents", "--format", "json"], &text);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("not json: {}", stdout(&o)));
        (v["id"].as_str().unwrap_or_default().to_string(), o)
    };
    let message = || stdout(&env.git(&["log", "-1", "--format=%B"]));

    let (_, o) = add(None, "Agent note one");
    assert!(o.status.success(), "{}", stdout(&o));
    assert!(!message().contains("Rkb-Agent:"), "no trailer without RKB_AGENT: {}", message());

    let (id, o) = add(Some("raven"), "Agent note two");
    assert!(o.status.success(), "{}", stdout(&o));
    assert!(message().contains("Rkb-Agent: raven"), "{}", message());

    let o = env.cmd(env!("CARGO_BIN_EXE_rkb")).env("RKB_AGENT", "raven").args(["used", &id, "--worked"]).output().unwrap();
    assert!(o.status.success(), "{}", stdout(&o));
    let usage: String =
        std::fs::read_dir(env.kb().join(".rkb/usage")).unwrap().map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap()).collect();
    assert!(usage.contains("\"agent\":\"raven\"") && usage.contains(&id), "{usage}");

    let (_, o) = add(Some("rav\nen"), "Agent note three");
    assert!(o.status.success(), "{}", stdout(&o));
    assert!(!message().contains("Rkb-Agent:"), "a value with a newline writes no trailer: {}", message());
    assert!(String::from_utf8_lossy(&o.stderr).contains("RKB_AGENT"), "{}", String::from_utf8_lossy(&o.stderr));
}

#[test]
fn expired_lesson_is_stale() {
    let env = kb_with_topics();
    let with_expires = |text: &str, date: &str| text.replacen("verified_how:", &format!("expires: {date}\nverified_how:"), 1);

    // Ranking and the search mark: an expired lesson acts as stale and ranks below an equal live one.
    let pit = pitfall_with(
        "Ninja cannot find the CMAKE_PREFIX_PATH package",
        "Configure fails to find the package.",
        "The prefix is not set.",
        "Set CMAKE_PREFIX_PATH.",
    );
    let live = add_ok(&env, "cmake", &pit)["id"].as_str().unwrap().to_string();
    let gone = add_ok(&env, "cpp", &with_expires(&pit, "2020-01-01"))["id"].as_str().unwrap().to_string();
    let (v, code) = env.json(&["search", "package prefix configure fails"], "");
    assert_eq!(code, Some(0), "{v}");
    let ids: Vec<&str> = v["results"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [live.as_str(), gone.as_str()], "the expired one ranks below the equal live one: {v}");
    let gone_row = &v["results"].as_array().unwrap()[1];
    assert_eq!((gone_row["status"].as_str(), gone_row["expired"].as_str()), (Some("stale"), Some("2020-01-01")), "{v}");
    let human = stdout(&env.rkb(&["search", "package prefix configure fails"]));
    assert!(human.contains("expired 2020-01-01"), "the search line marks the reason: {human}");

    // The digest and the session-start push leave the expired preference out; a future date outlives.
    let pref_gone = add_ok(
        &env,
        "cmake",
        &with_expires(&preference_with("Point the toolchain at the sdk root", "Point the toolchain at the sdk root."), "2020-01-01"),
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    let pref_later = add_ok(
        &env,
        "cpp",
        &with_expires(
            &preference_with("Write the target name explicitly in makefiles", "Write the target name explicitly in makefiles."),
            "2099-01-01",
        ),
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    let pref_live =
        add_ok(&env, "cmake", &preference_with("Quote shell variables in every script", "Quote shell variables in every script."))["id"]
            .as_str()
            .unwrap()
            .to_string();
    let (v, _) = env.json(&["digest"], "");
    let ids: Vec<&str> = v["lines"].as_array().unwrap().iter().map(|l| l["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&pref_live.as_str()) && ids.contains(&pref_later.as_str()), "the live and the future preferences stand: {v}");
    assert!(!ids.contains(&pref_gone.as_str()), "the expired preference is not in the digest: {v}");
    let toml = std::fs::read_to_string(env.kb().join("kb.toml")).unwrap();
    env.write("kb.toml", &format!("{toml}\n[hooks]\ndigest = true\n"));
    let out = hook(&env, "session-start", &serde_json::json!({ "session_id": "s", "cwd": env.dir.path(), "source": "startup" }));
    assert!(
        out.contains("Quote shell variables in every script.") && out.contains("Write the target name explicitly in makefiles."),
        "the live preferences are pushed: {out}"
    );
    assert!(!out.contains("Point the toolchain at the sdk root."), "the expired one is not pushed: {out}");

    // Review lists both expired lessons with the date and the fix, and nothing else.
    let (v, _) = env.json(&["review"], "");
    let candidates = v["candidates"].as_array().unwrap().clone();
    let expired: Vec<String> = candidates
        .iter()
        .filter(|c| c["reasons"].as_array().unwrap().iter().any(|r| r["kind"] == "expired"))
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(expired, vec![pref_gone.clone(), gone.clone()], "review lists both expired lessons: {v}");
    let detail = candidates.iter().find(|c| c["id"] == gone.as_str()).unwrap()["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "expired")
        .unwrap()["detail"]
        .as_str()
        .unwrap();
    assert!(detail.starts_with("expired 2020-01-01") && detail.contains("expires"), "the reason names the date and the fix: {detail}");
    let human = stdout(&env.rkb(&["review"]));
    assert!(human.contains("edit `expires`, supersede, or archive"), "the review entry carries the fix: {human}");
}

#[test]
fn expired_lesson_is_not_pushed_on_a_failure() {
    let env = search_kb();
    let cwd = env.kb();
    let linker = "Exit code 1\n/usr/bin/ld: main.o: in function `main':\nmain.cpp:(.text+0x1f): undefined reference to `vtable for Widget'\ncollect2: error: ld returned 1 exit status\n";
    let failed = |session: &str| {
        let mut f = bash(session, &cwd, "g++ main.o -o app");
        f["error"] = linker.into();
        f
    };
    assert!(hook(&env, "tool-failed", &failed("s1")).contains("1a00000012"), "a live lesson is pushed");
    let path = "general/cpp/undefined-vtable-means-missing-virtual.md";
    let text = std::fs::read_to_string(env.kb().join(path)).unwrap();
    env.write(path, &text.replacen("verified_how:", "expires: 2020-01-01\nverified_how:", 1));
    let out = hook(&env, "tool-failed", &failed("s2"));
    assert!(!out.contains("1a00000012"), "an expired lesson is not pushed: {out}");
}
