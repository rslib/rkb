use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("gitconfig"), "[user]\n\tname = Test\n\temail = test@example.org\n").unwrap();
        Env { dir }
    }

    fn kb(&self) -> PathBuf {
        self.dir.path().join("kb")
    }

    fn cmd(&self, program: &str) -> Command {
        let bin = PathBuf::from(env!("CARGO_BIN_EXE_rkb"));
        let path = format!("{}:{}", bin.parent().unwrap().display(), std::env::var("PATH").unwrap());
        let mut c = Command::new(program);
        c.env("RKB_HOME", self.kb())
            .env("HOME", self.dir.path())
            .env("USER", "testuser")
            .env("GIT_CONFIG_GLOBAL", self.dir.path().join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("PATH", path)
            .env("XDG_STATE_HOME", self.dir.path().join("state"))
            .env("XDG_CONFIG_HOME", self.dir.path().join("config"))
            .env("RKB_TTY", self.dir.path().join("no-tty"))
            .env_remove("CLAUDECODE")
            .env_remove("RKB_FORMAT")
            .env_remove("EMAIL")
            .env_remove("GIT_AUTHOR_NAME")
            .env_remove("GIT_AUTHOR_EMAIL")
            .env_remove("GIT_COMMITTER_NAME")
            .env_remove("GIT_COMMITTER_EMAIL");
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
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
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
    let o = env.rkb(&["init"]);
    assert!(!o.status.success());
    assert!(stdout(&o).contains("code: exists"), "{}", stdout(&o));
    assert!(!env.kb().join(".git").exists());
}

#[test]
fn init_needs_a_git_identity() {
    let env = Env::new();
    std::fs::write(env.dir.path().join("gitconfig"), "").unwrap();
    let o = env.rkb(&["init"]);
    assert!(!o.status.success());
    let out = stdout(&o);
    assert!(out.contains("code: git_identity") && out.contains("git config --global user.name"), "{out}");
    assert!(!env.kb().exists());
}

#[test]
fn missing_knowledge_base_points_to_init() {
    let env = Env::new();
    let o = env.rkb(&["lint"]);
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
    let toon = stdout(&env.rkb(&["lint"]));
    assert!(toon.contains("errors: 0") && toon.contains("findings"), "{toon}");
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
    let toon = env.rkb(&["show", "0000000000"]);
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
    let msg = json["findings"][0]["message"].as_str().unwrap();
    assert!(msg.contains("rkb lint --full") && msg.len() < 300, "{msg}");
    let full: serde_json::Value = serde_json::from_slice(&env.rkb(&["lint", "--full", "--format", "json"]).stdout).unwrap();
    assert!(full["findings"][0]["message"].as_str().unwrap().contains(&long));
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
    assert_eq!(before["warnings"], 2);

    let o = env.rkb(&["lint", "--fix", "--format", "json"]);
    assert!(o.status.success(), "{}", stdout(&o));
    let json: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(json["fixed"], serde_json::json!(["general/demo/README.md", "general/demo/a.md"]));
    assert_eq!(json["warnings"], 0);
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

#[test]
fn new_topic_needs_user_and_confirm_paths() {
    let env = kb_with_topics();
    let o = env.rkb_in(&["add", "--topic", "cmak"], PITFALL);
    assert_eq!(o.status.code(), Some(3));
    let out = stdout(&o);
    assert!(out.contains("status: needs_user") && out.contains("create general/cmak") && out.contains("use general/cmake"), "{out}");
    let (v, _) = env.json(&["add", "--topic", "cmak"], PITFALL);
    let req = v["request"].as_str().unwrap().to_string();
    assert_eq!(v["options"][0], "create general/cmak");
    assert!(v["next"].as_str().unwrap().starts_with(&format!("rkb confirm {req} --choice")));

    let (bad, _) = env.json(&["confirm", &req, "--choice", "delete everything"], "");
    assert_eq!(bad["error"]["code"], "bad_choice");
    assert!(bad["error"]["options"].as_array().unwrap().iter().any(|o| o == "use general/cmake"));

    let (nt, _) = env.json(&["confirm", &req, "--choice", "use general/cmake"], "");
    assert_eq!(nt["error"]["code"], "needs_terminal");
    assert!(nt["error"]["fix"].as_str().unwrap().contains(&format!("rkb confirm {req} --choice \"use general/cmake\"")));

    let tty = env.dir.path().join("tty");
    std::fs::write(&tty, "yes\n").unwrap();
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("RKB_TTY", &tty);
    let o = rkb_with(c, &["confirm", &req, "--choice", "use general/cmake", "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["status"], "written", "{v}");
    assert!(v["path"].as_str().unwrap().starts_with("general/cmake/"));
    assert!(std::fs::read_to_string(&tty).unwrap().contains("Type yes to confirm"));

    let (again, _) = env.json(&["confirm", &req, "--choice", "use general/cmake"], "");
    assert_eq!(again["error"]["code"], "expired");
}

#[test]
fn trusted_harness_confirms_without_terminal() {
    let env = kb_with_topics();
    env.trust_claude();
    let (v, _) = env.json(&["add", "--topic", "cmak"], PITFALL);
    let req = v["request"].as_str().unwrap();
    let mut c = env.cmd(env!("CARGO_BIN_EXE_rkb"));
    c.env("CLAUDECODE", "1");
    let o = rkb_with(c, &["confirm", req, "--choice", "create general/cmak", "--format", "json"], "");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["status"], "written", "{v}");
    assert!(env.kb().join("general/cmak").is_dir());
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
    assert_eq!(stdout(&env.git(&["rev-list", "--count", "HEAD"])).trim(), "4");

    let id = show_first_id(&env);
    let used: Vec<_> = (0..20)
        .map(|_| env.cmd(env!("CARGO_BIN_EXE_rkb")).args(["used", &id, "--worked"]).stdin(std::process::Stdio::null()).spawn().unwrap())
        .collect();
    for mut c in used {
        assert!(c.wait().unwrap().success());
    }
    let log = std::fs::read_to_string(env.dir.path().join("state/rkb/usage.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 20);
    for l in log.lines() {
        serde_json::from_str::<serde_json::Value>(l).unwrap();
    }
}

fn show_first_id(env: &Env) -> String {
    let dir = env.kb().join("general/cmake");
    let file = std::fs::read_dir(dir).unwrap().flatten().find(|e| e.file_name() != "README.md").unwrap();
    let text = std::fs::read_to_string(file.path()).unwrap();
    text.lines().find_map(|l| l.strip_prefix("id: ")).unwrap().to_string()
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
}

#[test]
fn expired_request() {
    let env = kb_with_topics();
    let (v, _) = env.json(&["add", "--topic", "cmak"], PITFALL);
    let req = v["request"].as_str().unwrap();
    let file = env.dir.path().join(format!("state/rkb/requests/{req}.json"));
    let mut r: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    r["created"] = serde_json::json!(0);
    std::fs::write(&file, r.to_string()).unwrap();
    let (v, _) = env.json(&["confirm", req, "--choice", "create general/cmak"], "");
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

    let o = env.rkb_in(&["add", "--topic", "cmak", "--format", "human"], PITFALL);
    assert_eq!(o.status.code(), Some(3));
    let out = stdout(&o);
    assert!(
        out.contains("Needs your decision:") && out.contains("  1. create general/cmak") && out.contains("Next: rkb confirm r-"),
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

    let (v, _) = env.json(&["add", "--topic", "cmak"], &lesson_with("Something else about cmake builds"));
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
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "");
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

    let toon = list_cmd(&env, &["list", "general/io"], "80", "en_US.UTF-8");
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
    let (v, _) = env.json(&["add", "--topic", "demp"], PITFALL);
    assert_eq!(v["status"], "needs_user", "`demp` is a likely typo of `demo`: {v}");
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
fn show_applies_with_reasons() {
    let env = fixture_kb();
    let (v, _) = env.json(&["show", "7f3a9c2b41", "--with", "hdf5=1.14.3", "--system", "tuolumne"], "");
    assert_eq!(v["applies"]["result"], "no", "{v}");
    let hdf5 = v["applies"]["keys"].as_array().unwrap().iter().find(|k| k["key"] == "hdf5").unwrap().clone();
    assert_eq!((hdf5["want"].as_str(), hdf5["have"].as_str(), hdf5["result"].as_str()), (Some("1.12:1.14.2"), Some("1.14.3"), Some("no")));

    let human = stdout(&env.rkb(&["show", "7f3a9c2b41", "--with", "hdf5=1.14.3", "--system", "tuolumne", "--format", "human"]));
    assert!(human.contains("applies here: no (hdf5 wants 1.12:1.14.2, has 1.14.3)"), "{human}");
    let toon = stdout(&env.rkb(&["show", "7f3a9c2b41", "--with", "hdf5=1.14.3", "--system", "tuolumne"]));
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
    let env = Env::new();
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/search/kb");
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
    assert_eq!(env.rkb(&["eval", "--queries", low.to_str().unwrap(), "--min-recall", "0.5"]).status.code(), Some(1));
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
    let toon = stdout(&env.rkb(&["search", "git rebase"]));
    assert!(toon.contains("results[3]{id,type,title,path,status,applies,summary}:"), "{toon}");
    assert_eq!(stdout(&env.git(&["status", "--porcelain"])), "");
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
        [("claude", true), ("pi", true), ("omp", false)]
    );

    let (v, code) = env.json(&["install"], "");
    assert_eq!((v["status"].as_str(), code), (Some("needs_user"), Some(3)));
    let q = v["question"].as_str().unwrap();
    assert!(q.contains("~/.claude/skills/rkb/SKILL.md") && q.contains("~/.claude/settings.json") && q.contains("trust.toml"), "{q}");
    assert!(!home.join(".claude/skills/rkb").exists(), "nothing written before confirm");

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
    let (v, _) = env.json(&["install", "--list"], "");
    assert_eq!(v["harnesses"][1]["current"], true);

    let (v, code) = env.json(&["install", "vscode"], "");
    assert_eq!((v["error"]["code"].as_str(), code), (Some("usage"), Some(2)));
    assert!(v["error"]["fix"].as_str().unwrap().contains("claude, pi, omp"));
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

fn hook(env: &Env, event: &str, payload: &serde_json::Value) -> String {
    let o = env.rkb_in(&["hook", event], &payload.to_string());
    assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn bash(session: &str, cwd: &Path, command: &str) -> serde_json::Value {
    serde_json::json!({ "session_id": session, "cwd": cwd, "tool_name": "Bash", "tool_input": { "command": command } })
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
    let settings: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(home.join(".claude/settings.json")).unwrap()).unwrap();
    assert_eq!(settings["hooks"]["PostToolUseFailure"][0]["hooks"][0]["command"], "rkb hook tool-failed");
    let doctor_hook = |env: &Env| {
        let (v, _) = env.json(&["doctor"], "");
        v["checks"].as_array().unwrap().iter().find(|c| c["check"] == "hooks: claude").cloned().unwrap()
    };
    assert_eq!(doctor_hook(&env)["detail"], "installed but never called");

    let out = hook(&env, "session-start", &serde_json::json!({ "session_id": "s1", "source": "startup", "cwd": cwd }));
    assert!(out.contains("see lessons: rkb list"), "{out}");
    assert_eq!(doctor_hook(&env)["status"], "ok");

    let linker = "Exit code 1\n/usr/bin/ld: main.o: in function `main':\nmain.cpp:(.text+0x1f): undefined reference to `vtable for Widget'\ncollect2: error: ld returned 1 exit status\n";
    let mut failed = bash("s1", &cwd, "g++ main.o -o app");
    failed["error"] = linker.into();
    let out = hook(&env, "tool-failed", &failed);
    let reply: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|_| panic!("{out}"));
    let context = reply["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
    assert_eq!(reply["hookSpecificOutput"]["hookEventName"], "PostToolUseFailure");
    assert!(context.contains("1a00000012") && context.contains("rkb show 1a00000012") && context.lines().count() <= 2, "{context}");
    assert_eq!(hook(&env, "tool-failed", &failed), "", "once per session");
    let injections = std::fs::read_to_string(home.join("state/rkb/injections.jsonl")).unwrap();
    assert_eq!(injections.lines().count(), 1);

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
    assert_eq!(kinds, ["failed", "injected", "failed", "failed", "failed", "fixed", "correction", "remember"], "{session}");
    assert!(
        !session.contains("sleep") && !session.contains("-B") && !session.contains("release") && !session.contains("actually"),
        "{session}"
    );

    let stop = serde_json::json!({ "session_id": "s1", "stop_hook_active": false });
    assert_eq!(hook(&env, "stop", &stop), "", "nudge is off by default");
    let toml = std::fs::read_to_string(cwd.join("kb.toml")).unwrap();
    std::fs::write(cwd.join("kb.toml"), format!("{toml}\n[hooks]\nstop_nudge = true\n")).unwrap();
    let out = hook(&env, "stop", &stop);
    let reply: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|_| panic!("{out}"));
    let note = reply["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
    assert!(note.contains("cmake") && note.contains("rkb add"), "{note}");
    assert_eq!(hook(&env, "stop", &stop), "", "no new signal");
    hook(&env, "prompt", &serde_json::json!({ "session_id": "s1", "prompt": "no, use ninja" }));
    assert_eq!(hook(&env, "stop", &serde_json::json!({ "session_id": "s1", "stop_hook_active": true })), "");

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
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/extension/run.mjs");
    for harness in ["pi", "omp"] {
        let file = env.dir.path().join(format!("rkb-{harness}.ts"));
        std::fs::write(&file, include_str!("../../../extensions/rkb.ts").replace("__HARNESS__", harness)).unwrap();
        let o = env
            .cmd("node")
            .arg(&script)
            .arg(&file)
            .arg(harness)
            .env("RKB_KB", env.kb())
            .env("RKB_REQUEST", req["request"].as_str().unwrap())
            .env("RKB_QUESTION", req["question"].as_str().unwrap())
            .output()
            .unwrap();
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
    assert!(human.contains("Needs your decision") && human.contains("1. import all"), "{human}");
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
    // `ninj` is new and far from every topic at report time; `ninja` created now makes it a likely typo.
    add_ok(&env, "ninja", &lesson_with("Ninja needs a build.ninja file"));
    let (v, code) = confirm_on_tty(&env, report["request"].as_str().unwrap(), "import all");
    assert_eq!((v["status"].as_str(), code), (Some("stopped"), Some(1)), "{v}");
    assert_eq!(v["added"].as_array().unwrap().len(), 2);
    assert_eq!(v["not_added"][0]["source"], "ninj/c.md");
    assert!(v["not_added"][0]["reason"].as_str().unwrap().contains("changed since the report"), "{v}");
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
fn supersede_archive_unarchive() {
    let env = fixture_kb();
    env.trust_claude();
    let before = commit_count(&env);

    let (v, code) = env.json(&["supersede", "5d2e8a1c90", "--by", "9a4c7e2b10", "--reason", "the regex advice is wrong now"], "");
    assert_eq!((v["status"].as_str(), code), (Some("needs_user"), Some(3)), "{v}");
    assert_eq!(v["options"], serde_json::json!(["supersede 5d2e8a1c90", "cancel"]));
    let q = v["question"].as_str().unwrap();
    assert!(q.contains("5d2e8a1c90") && q.contains("9a4c7e2b10") && q.contains("Search will hide"), "{q}");
    assert_eq!(commit_count(&env), before, "nothing before confirm");
    let v = trusted_confirm(&env, v["request"].as_str().unwrap(), "supersede 5d2e8a1c90");
    assert_eq!(v["status"], "written", "{v}");
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

    let (v, _) = env.json(&["archive", "c04e11a9f3", "--reason", "tuolumne retired"], "");
    trusted_confirm(&env, v["request"].as_str().unwrap(), "archive c04e11a9f3");
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
