use std::path::PathBuf;

use rkb_core::kb::Snapshot;
use rkb_core::lint::{Finding, LintEnv, Severity, lint};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/kb")
}

fn env() -> LintEnv {
    LintEnv { user: Some("fixtureuser".into()), home: Some("/home/fixtureuser".into()) }
}

fn run_all(files: &[(&str, &str)]) -> Vec<Finding> {
    let mut snap = Snapshot::from_dir(&fixture()).unwrap();
    for (p, t) in files {
        snap.files.insert(p.to_string(), t.as_bytes().to_vec());
    }
    lint(&snap, &env(), None)
}

/// Findings without the `quality/` warnings, which the short test lessons all get.
fn run(files: &[(&str, &str)]) -> Vec<Finding> {
    run_all(files).into_iter().filter(|f| !f.rule.starts_with("quality/")).collect()
}

#[test]
fn quality_warnings() {
    let quality = |text: &str| -> Vec<&'static str> {
        run_all(&[(P, text)]).into_iter().filter(|f| f.path == P && f.rule.starts_with("quality/")).map(|f| f.rule).collect()
    };
    assert_eq!(quality(FACT), ["quality/evidence", "quality/title"]);
    let good = fact("# A fact\n", "# The demo fact holds on every run\n").replace("It was seen.", "`demo --check` printed 1.");
    assert_eq!(quality(&good), Vec::<&str>::new());
    assert!(run_all(&[(P, &good)]).iter().all(|f| f.severity != Severity::Error));
}

fn rules(files: &[(&str, &str)]) -> Vec<&'static str> {
    run(files).into_iter().filter(|f| f.severity == Severity::Error).map(|f| f.rule).collect()
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

fn fact(from: &str, to: &str) -> String {
    assert!(FACT.contains(from), "{from}");
    FACT.replacen(from, to, 1)
}

const P: &str = "general/demo/a-fact.md";

#[test]
fn fixture_is_clean() {
    assert_eq!(run(&[]), vec![]);
    assert_eq!(rules(&[(P, FACT)]), Vec::<&str>::new());
}

#[test]
fn all_findings_are_reported() {
    let r = rules(&[(P, &fact("type: fact", "type: bug")), ("general/demo/b.md", &fact("id: 0a1b2c3d4e", "id: XYZ"))]);
    assert_eq!(r, ["format/parse", "format/id"]);
}

#[test]
fn frontmatter_rules() {
    let f = run(&[(P, &fact("type: fact", "type: bug"))]);
    assert!(f[0].message.contains("pitfall"), "{}", f[0].message);
    assert_eq!(rules(&[(P, &fact("verified_how: ran", "verified_how: ran\nverifed: x"))]), ["format/parse"]);
    assert_eq!(rules(&[(P, &fact("schema: 1", "schema: 2"))]), ["format/schema"]);
    assert_eq!(rules(&[(P, &fact("0a1b2c3d4e", "0A1B2C3D4E"))]), ["format/id"]);
    assert_eq!(rules(&[(P, &fact("status: active", "status: superseded"))]), ["body/headings", "format/superseded_by"]);
    assert_eq!(rules(&[(P, &fact("status: active", "status: active\nsuperseded_by: 7f3a9c2b41"))]), ["format/superseded_by"]);
    assert_eq!(rules(&[(P, &fact("status: active", "status: stale"))]), ["format/stale_reason"]);
    assert_eq!(rules(&[(P, &fact("status: active", "status: stale\nstale_reason: check failed"))]), Vec::<&str>::new());
    assert_eq!(rules(&[(P, &fact("status: active", "status: active\nstale_reason: x"))]), ["format/stale_reason"]);
    assert_eq!(rules(&[(P, &fact("  - demo", "  - Demo\n  - two words"))]), ["format/tags", "format/tags"]);
    assert_eq!(rules(&[(P, &fact("verified: 2026-09-25", "verified: 2026-13-01"))]), ["format/parse"]);
}

#[test]
fn superseded_needs_its_heading() {
    let ok = fact("status: active", "status: superseded\nsuperseded_by: 7f3a9c2b41") + "\n## Why superseded\nWrong range.\n";
    assert_eq!(rules(&[(P, &ok)]), Vec::<&str>::new());
    let archived = fact("status: active", "status: archived");
    assert_eq!(rules(&[(P, &archived)]), ["body/headings"]);
}

#[test]
fn labels() {
    assert_eq!(rules(&[(P, &fact("tags:\n  - demo", "labels:\n  sensitivity: public"))]), Vec::<&str>::new());
    let f = run(&[(P, &fact("tags:\n  - demo", "labels:\n  sensitivity: secret"))]);
    assert_eq!(f[0].rule, "labels/unknown");
    assert!(f[0].message.contains("confidential"));
    assert_eq!(rules(&[(P, &fact("tags:\n  - demo", "labels:\n  team: a"))]), ["labels/unknown"]);
    assert_eq!(rules(&[("projects/dftracer/README.md", "---\nlabels:\n  sensitivity: secret\n---\n# dftracer\n")]), ["labels/unknown"]);
}

#[test]
fn title_rules() {
    assert_eq!(rules(&[(P, &fact("# A fact\n", ""))]), ["body/title"]);
    assert_eq!(rules(&[(P, &fact("# A fact\n", "Intro.\n\n# A fact\n"))]), ["body/title"]);
    assert_eq!(rules(&[(P, &fact("## Evidence", "# Second\n\n## Evidence"))]), ["body/title"]);
}

#[test]
fn heading_rules() {
    assert_eq!(rules(&[(P, &fact("## Evidence\nIt was seen.\n", ""))]), ["body/headings"]);
    assert_eq!(rules(&[(P, &fact("It was seen.\n", ""))]), ["body/empty-section"]);
    assert_eq!(rules(&[(P, &(FACT.to_string() + "\n## Evidence\nAgain.\n"))]), ["body/headings"]);
    let swapped = fact(
        "## Statement\nSomething is true.\n\n## Evidence\nIt was seen.\n",
        "## Evidence\nIt was seen.\n\n## Statement\nSomething is true.\n",
    );
    let f = run(&[(P, &swapped)]);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].rule, "body/heading-order");
    assert!(f[0].message.contains("Statement") && f[0].message.contains("Evidence"));
    assert_eq!(f[0].line, Some(14));
}

#[test]
fn code_block_rules() {
    assert_eq!(rules(&[(P, &fact("It was seen.\n", "It was seen.\n\n```\nx\n```\n"))]), ["body/fence-lang"]);
    assert_eq!(rules(&[(P, &fact("It was seen.\n", "It was seen.\n\n    indented\n"))]), ["body/indented-code"]);
    let check = |blocks: &str| FACT.to_string() + "\n## Check\n" + blocks;
    assert_eq!(rules(&[(P, &check("```bash\ntrue\n```\n"))]), Vec::<&str>::new());
    assert_eq!(rules(&[(P, &check("```sh\ntrue\n```\n```sh\ntrue\n```\n"))]), ["body/script"]);
    assert_eq!(rules(&[(P, &check("```python\nprint()\n```\n"))]), ["body/script"]);
    assert_eq!(rules(&[(P, &(FACT.to_string() + "\n## Probe\nnone\n"))]), ["body/script"]);
}

#[test]
fn link_rules() {
    let with = |link: &str| fact("It was seen.", &format!("It was seen. See {link}."));
    assert_eq!(rules(&[(P, &with("[x](../cpp/avoid-std-regex-in-hot-loops.md)"))]), Vec::<&str>::new());
    assert_eq!(rules(&[(P, &with("[x](../../projects/dftracer/cmake/cmake-needs-hdf5-root.md#fix)"))]), Vec::<&str>::new());
    assert_eq!(rules(&[(P, &with("[x](https://example.org/doc)"))]), Vec::<&str>::new());
    let f = run(&[(P, &with("[x](../../general/hdf5/static-libs-need-flag.md)"))]);
    assert_eq!(f[0].rule, "links/broken");
    assert!(f[0].message.contains("static-libs-need-flag.md"));
    assert_eq!(rules(&[(P, &with("[x](../../../outside.md)"))]), ["links/broken"]);
}

#[test]
fn duplicate_ids() {
    let f = run(&[(P, &fact("0a1b2c3d4e", "7f3a9c2b41"))]);
    assert_eq!(f.len(), 2);
    assert!(f.iter().all(|f| f.rule == "format/duplicate-id"));
    assert!(f[0].message.contains("projects/dftracer/cmake/cmake-needs-hdf5-root.md"));
}

#[test]
fn asset_rules() {
    let a = "projects/dftracer/lustre/io-bandwidth-drops-with-small-stripes.assets";
    assert_eq!(rules(&[(&format!("{a}/run.log"), "x")]), ["assets/type", "assets/unlinked"]);
    assert_eq!(rules(&[(&format!("{a}/plot.png"), "x")]), ["assets/unlinked"]);
    let big = "x".repeat(1024 * 1024 + 1);
    let linked = "projects/dftracer/lustre/io-bandwidth-drops-with-small-stripes.assets/bandwidth-by-stripe-count.svg";
    assert_eq!(rules(&[(linked, &big)]), ["assets/size"]);
}

#[test]
fn folder_rules() {
    assert_eq!(rules(&[("general/Bad_Name/a.md", &fact("0a1b2c3d4e", "0a1b2c3d4f"))]), ["layout/folder-name"]);
    let long = FACT.replace("It was seen.\n", &"It was seen.\n".repeat(150));
    let f = run(&[(P, &long)]);
    assert_eq!(f.iter().map(|f| (f.rule, f.severity)).collect::<Vec<_>>(), [("size/lesson-lines", Severity::Warning)]);
}

#[test]
fn config_rules() {
    assert_eq!(rules(&[("kb.toml", "[lables]\n")]), ["config/invalid"]);
    assert_eq!(rules(&[("kb.toml", "[site]\ncolour = \"x\"\n")]), ["config/invalid"]);
    assert_eq!(rules(&[("systems/tuolumne/README.md", "---\nenv: {A: b}\n---\n# Tuolumne\n")]), ["config/invalid"]);
    assert_eq!(rules(&[("projects/dftracer/README.md", "---\nremotes: x\n---\n")]), ["config/invalid"]);
    assert_eq!(rules(&[("general/cpp/README.md", "---\ncolor: red\n---\n")]), ["config/invalid"]);
    assert_eq!(rules(&[("general/cpp/README.md", "# Plain note with no frontmatter\n")]), Vec::<&str>::new());
    let mut snap = Snapshot::from_dir(&fixture()).unwrap();
    snap.files.remove("kb.toml");
    assert_eq!(lint(&snap, &env(), None)[0].rule, "config/missing");
}

#[test]
fn leak_scan() {
    let f = run(&[(P, &fact("It was seen.", "Built in /Users/ray/Code/dftracer/build."))]);
    assert_eq!(f[0].rule, "leak/path");
    assert_eq!(f[0].line, Some(18));
    assert_eq!(rules(&[(P, &fact("It was seen.", "Built in $HOME/Code/dftracer/build."))]), Vec::<&str>::new());
    assert_eq!(rules(&[(P, &fact("It was seen.", "bandwidth drops by 123456 bytes"))]), Vec::<&str>::new());
    assert_eq!(rules(&[(P, &fact("tags:\n  - demo", "when:\n  path: \"/home/ray/src\""))]), ["leak/path"]);
    assert_eq!(rules(&[(P, &fact("It was seen.", "fixtureuser saw it."))]), ["leak/username"]);
    let svg = "projects/dftracer/lustre/io-bandwidth-drops-with-small-stripes.assets/bandwidth-by-stripe-count.svg";
    assert_eq!(rules(&[(svg, "<svg><text>alice@example.org</text></svg>")]), ["leak/email"]);

    let allow = std::fs::read_to_string(fixture().join("kb.toml")).unwrap().replace("allow = []", "allow = [\"alice@example.org\"]");
    assert_eq!(rules(&[("kb.toml", &allow), (P, &fact("It was seen.", "Ask alice@example.org."))]), Vec::<&str>::new());
}

#[test]
fn layout_rules() {
    assert_eq!(rules(&[("general/a-fact.md", FACT)]), ["layout/depth"]);
    assert_eq!(rules(&[("general/cpp/regex/a-fact.md", FACT)]), ["layout/depth"]);
    assert_eq!(rules(&[("projects/dftracer/a-fact.md", FACT)]), ["layout/depth"]);
    assert_eq!(rules(&[("general/cpp/regex/README.md", "# Regex\n")]), ["layout/depth"]);
    assert_eq!(rules(&[("projects/dftracer/io/a-fact.md", FACT)]), Vec::<&str>::new());
}

#[test]
fn scope_follows_when() {
    let with = |when: &str| fact("tags:\n  - demo", &format!("when:\n{when}"));
    let f = run(&[(P, &with("  project: dftracer"))]);
    assert_eq!(f[0].rule, "layout/scope");
    assert!(f[0].message.contains("projects/dftracer/"));
    assert_eq!(rules(&[(P, &with("  system: tuolumne"))]), ["layout/scope"]);
    assert_eq!(rules(&[(P, &with("  project: [dftracer, other]"))]), Vec::<&str>::new());
    let proj = "projects/dftracer/io/a-fact.md";
    assert_eq!(rules(&[(proj, &with("  project: other"))]), ["layout/scope"]);
    assert_eq!(rules(&[(proj, &with("  project: dftracer\n  system: tuolumne"))]), Vec::<&str>::new());
    assert_eq!(rules(&[("systems/tuolumne/io/a-fact.md", &with("  system: quartz"))]), ["layout/scope"]);
}

#[test]
fn alias_rules() {
    let f = run(&[("general/cxx/a-fact.md", FACT)]);
    assert_eq!(f.len(), 1);
    assert_eq!((f[0].rule, f[0].path.as_str()), ("layout/alias", "general/cxx"));
    assert!(f[0].message.contains("`cpp`"));
    let r = rules(&[("general/perf/README.md", "---\naliases: [cxx]\n---\n# Perf\n"), ("general/perf/a-fact.md", FACT)]);
    assert_eq!(r, ["layout/alias", "layout/alias"]);
}

#[test]
fn flow_style_is_a_warning() {
    let f = run(&[(P, &fact("tags:\n  - demo", "tags: [demo, x]"))]);
    assert_eq!(f.iter().map(|f| (f.rule, f.severity, f.line)).collect::<Vec<_>>(), [("format/flow-style", Severity::Warning, Some(8))]);
    let note = run(&[("general/cpp/README.md", "---\naliases: [c++, cxx]\nlabels: {sensitivity: public}\n---\n# C\n")]);
    assert_eq!(note.iter().map(|f| (f.rule, f.line)).collect::<Vec<_>>(), [("format/flow-style", Some(2)), ("format/flow-style", Some(3))]);
    assert_eq!(run(&[(P, &fact("tags:\n  - demo", "tags: []"))]), vec![]);
}

#[test]
fn when_syntax() {
    let with = |w: &str| fact("tags:\n  - demo", &format!("tags:\n  - demo\nwhen:\n{w}"));
    let f = run(&[(P, &with("  hdf5: 1.10"))]);
    assert_eq!(f.iter().map(|f| f.rule).collect::<Vec<_>>(), ["when/invalid"]);
    assert!(f[0].message.contains("quote"));
    let f = run(&[(P, &with("  hdf5: \"1.12:1 14\""))]);
    assert!(f[0].rule == "when/invalid" && f[0].message.contains("1 14"));
    let ok = "  hdf5: \"1.12:\"\n  compiler: \"gcc@12:\"\n  path: \"src/io/**\"\n  gpu:\n    - a100\n    - mi300a";
    assert_eq!(rules(&[(P, &with(ok))]), Vec::<&str>::new());
}
