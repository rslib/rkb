use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use globset::Glob;
use serde::Serialize;
use serde_norway::{Mapping, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Num(u64),
    Word(String),
}

/// A version compared by the rules in `PLAN.md` "Version rules", which follow Spack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    parts: Vec<Part>,
}

const TOP_WORDS: [&str; 4] = ["develop", "main", "master", "head"];

impl Version {
    /// `None` when the text has characters other than letters, digits, `.`, `-` and `_`, or no parts.
    pub fn parse(text: &str) -> Option<Self> {
        let t = text.trim();
        let t = match t.strip_prefix(['v', 'V']) {
            Some(rest) if rest.starts_with(|c: char| c.is_ascii_digit()) => rest,
            _ => t,
        };
        let mut parts = vec![];
        let mut cur = String::new();
        let flush = |cur: &mut String, parts: &mut Vec<Part>| -> Option<()> {
            if !cur.is_empty() {
                let p = if cur.chars().all(|c| c.is_ascii_digit()) {
                    Part::Num(cur.parse().ok()?)
                } else {
                    Part::Word(cur.to_ascii_lowercase())
                };
                parts.push(p);
                cur.clear();
            }
            Some(())
        };
        for c in t.chars() {
            if matches!(c, '.' | '-' | '_') {
                flush(&mut cur, &mut parts)?;
            } else if c.is_ascii_alphanumeric() {
                if cur.chars().last().is_some_and(|l| l.is_ascii_digit() != c.is_ascii_digit()) {
                    flush(&mut cur, &mut parts)?;
                }
                cur.push(c);
            } else {
                return None;
            }
        }
        flush(&mut cur, &mut parts)?;
        (!parts.is_empty()).then_some(Version { parts })
    }

    fn top(&self) -> bool {
        matches!(&self.parts[0], Part::Word(w) if TOP_WORDS.contains(&w.as_str()))
    }

    /// Whether every part of `self` equals the first parts of `other`.
    pub fn is_prefix_of(&self, other: &Version) -> bool {
        self.parts.len() <= other.parts.len() && self.parts.iter().zip(&other.parts).all(|(a, b)| a == b)
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.top(), other.top()) {
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
            _ => {}
        }
        for (a, b) in self.parts.iter().zip(&other.parts) {
            let o = match (a, b) {
                (Part::Num(x), Part::Num(y)) => x.cmp(y),
                (Part::Word(x), Part::Word(y)) => x.cmp(y),
                (Part::Word(_), Part::Num(_)) => Ordering::Less,
                (Part::Num(_), Part::Word(_)) => Ordering::Greater,
            };
            if o != Ordering::Equal {
                return o;
            }
        }
        // One is a prefix of the other: a longer version continuing with a number is higher, with a word lower.
        let longer_next = |v: &Version, n: usize| match v.parts.get(n) {
            Some(Part::Num(_)) => Ordering::Greater,
            Some(Part::Word(_)) => Ordering::Less,
            None => Ordering::Equal,
        };
        match self.parts.len().cmp(&other.parts.len()) {
            Ordering::Equal => Ordering::Equal,
            Ordering::Greater => longer_next(self, other.parts.len()),
            Ordering::Less => longer_next(other, self.parts.len()).reverse(),
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A version range whose bounds match by prefix. Either side may be open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    lo: Option<Version>,
    hi: Option<Version>,
}

impl Range {
    /// `a:b`, `a:` or `:b`; a text without `:` is the prefix range `a:a`. The error names the part that does not parse.
    pub fn parse(text: &str) -> Result<Self, String> {
        let side = |s: &str| -> Result<Option<Version>, String> {
            let s = s.trim();
            if s.is_empty() { Ok(None) } else { Version::parse(s).map(Some).ok_or_else(|| s.to_string()) }
        };
        match text.split_once(':') {
            Some((lo, hi)) => Ok(Range { lo: side(lo)?, hi: side(hi)? }),
            None => {
                let v = side(text)?.ok_or_else(|| text.to_string())?;
                Ok(Range { lo: Some(v.clone()), hi: Some(v) })
            }
        }
    }

    pub fn contains(&self, v: &Version) -> bool {
        let above_lo = self.lo.as_ref().is_none_or(|lo| v >= lo || lo.is_prefix_of(v));
        let below_hi = self.hi.as_ref().is_none_or(|hi| v <= hi || hi.is_prefix_of(v));
        above_lo && below_hi
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Yes,
    No,
    Unknown,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Yes => "yes",
            Verdict::No => "no",
            Verdict::Unknown => "unknown",
        }
    }

    /// `no` beats `unknown` beats `yes`.
    fn all(vs: impl IntoIterator<Item = Verdict>) -> Verdict {
        vs.into_iter().fold(Verdict::Yes, |acc, v| match (acc, v) {
            (Verdict::No, _) | (_, Verdict::No) => Verdict::No,
            (Verdict::Unknown, _) | (_, Verdict::Unknown) => Verdict::Unknown,
            _ => Verdict::Yes,
        })
    }

    /// Any `yes` wins; otherwise any `unknown`; `no` only when every item says `no`.
    fn any(vs: impl IntoIterator<Item = Verdict>) -> Verdict {
        vs.into_iter().fold(Verdict::No, |acc, v| match (acc, v) {
            (Verdict::Yes, _) | (_, Verdict::Yes) => Verdict::Yes,
            (Verdict::Unknown, _) | (_, Verdict::Unknown) => Verdict::Unknown,
            _ => Verdict::No,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyResult {
    pub key: String,
    pub want: String,
    pub have: Option<String>,
    pub result: Verdict,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Applies {
    pub result: Verdict,
    pub keys: Vec<KeyResult>,
}

impl Applies {
    /// One short line: the keys that decided a `no` or an `unknown`.
    pub fn summary(&self) -> String {
        let deciding: Vec<String> = self
            .keys
            .iter()
            .filter(|k| k.result == self.result)
            .map(|k| if k.result == Verdict::Unknown { format!("{}: {}", k.key, k.reason) } else { format!("{} {}", k.key, k.reason) })
            .collect();
        match self.result {
            Verdict::Yes if self.keys.is_empty() => "yes (no conditions)".into(),
            Verdict::Yes => "yes".into(),
            r => format!("{} ({})", r.as_str(), deciding.join("; ")),
        }
    }
}

/// Answers `since` and `until`. `None` means the commit is not known here.
pub trait Ancestry {
    fn is_ancestor(&self, commit: &str) -> Option<bool>;
}

/// Asks git in a checkout, once per commit.
pub struct GitAncestry {
    repo: PathBuf,
    memo: RefCell<HashMap<String, Option<bool>>>,
}

impl GitAncestry {
    pub fn new(repo: &Path) -> Self {
        GitAncestry { repo: repo.to_path_buf(), memo: RefCell::new(HashMap::new()) }
    }

    fn git(&self, args: &[&str]) -> Option<i32> {
        Command::new("git").arg("-C").arg(&self.repo).args(args).stdout(Stdio::null()).stderr(Stdio::null()).status().ok()?.code()
    }
}

impl Ancestry for GitAncestry {
    fn is_ancestor(&self, commit: &str) -> Option<bool> {
        if let Some(v) = self.memo.borrow().get(commit) {
            return *v;
        }
        let exists = self.git(&["cat-file", "-e", &format!("{commit}^{{commit}}")]) == Some(0);
        let v = if exists {
            match self.git(&["merge-base", "--is-ancestor", commit, "HEAD"]) {
                Some(0) => Some(true),
                Some(1) => Some(false),
                _ => None,
            }
        } else {
            None
        };
        self.memo.borrow_mut().insert(commit.to_string(), v);
        v
    }
}

/// The facts of one place, merged in source order: `--with`, then built-ins, then system note facts.
#[derive(Default)]
pub struct Facts {
    pub values: BTreeMap<String, String>,
    pub ancestry: Option<Box<dyn Ancestry>>,
}

impl Facts {
    /// Collects the facts of `place`: `--with` pairs first, then built-ins, then the system note's `facts`.
    pub fn gather(root: &Path, place: &crate::matching::Place, with: &[(String, String)]) -> Facts {
        let mut values = BTreeMap::new();
        if let Some(s) = &place.system {
            let note = std::fs::read_to_string(root.join("systems").join(&s.name).join("README.md")).unwrap_or_default();
            if let Ok(n) = crate::config::parse_note::<crate::config::SystemNote>(&note) {
                values.extend(n.facts);
            }
            values.insert("system".into(), s.name.clone());
        }
        if let Some(p) = &place.project {
            values.insert("project".into(), p.name.clone());
        }
        values.insert("os".into(), std::env::consts::OS.to_string());
        let mut ancestry: Option<Box<dyn Ancestry>> = None;
        if let Some(repo) = &place.repo {
            if let Ok(out) = crate::git::run(&repo.top, &["rev-parse", "--abbrev-ref", "HEAD"]) {
                let branch = String::from_utf8_lossy(&out).trim().to_string();
                if !branch.is_empty() && branch != "HEAD" {
                    values.insert("branch".into(), branch);
                }
            }
            ancestry = Some(Box::new(GitAncestry::new(&repo.top)));
        }
        values.extend(with.iter().cloned());
        Facts { values, ancestry }
    }
}

fn verdict(key: &str, want: &str, have: Option<String>, result: Verdict, reason: String) -> KeyResult {
    KeyResult { key: key.to_string(), want: want.to_string(), have, result, reason }
}

fn one(key: &str, want: &str, facts: &Facts) -> KeyResult {
    if key == "since" || key == "until" {
        let Some(anc) = &facts.ancestry else {
            return verdict(key, want, None, Verdict::Unknown, "needs a git checkout of the project".into());
        };
        return match anc.is_ancestor(want) {
            None => verdict(key, want, None, Verdict::Unknown, format!("commit {want} is not in this checkout")),
            Some(is) => {
                let yes = if key == "since" { is } else { !is };
                let r = if yes { Verdict::Yes } else { Verdict::No };
                let how = if is { "is an ancestor of HEAD" } else { "is not an ancestor of HEAD" };
                verdict(key, want, None, r, format!("commit {want} {how}"))
            }
        };
    }
    let Some(have) = facts.values.get(key).cloned() else {
        let hint = match key {
            "system" | "project" => format!("no {key} matched here; pass --{key} <name>"),
            "path" => "needs the file you work on; pass --with path=<file>".to_string(),
            _ => format!("has no fact; pass --with {key}=<value>"),
        };
        return verdict(key, want, None, Verdict::Unknown, hint);
    };
    let wants_has = |r: Verdict| {
        let reason = if r == Verdict::Yes { format!("is {have}") } else { format!("wants {want}, has {have}") };
        verdict(key, want, Some(have.clone()), r, reason)
    };
    let unknown = |why: String| verdict(key, want, Some(have.clone()), Verdict::Unknown, why);
    let yes_no = |b: bool| if b { Verdict::Yes } else { Verdict::No };

    if key == "path" {
        return match Glob::new(want) {
            Ok(g) => wants_has(yes_no(g.compile_matcher().is_match(&have))),
            Err(_) => unknown(format!("glob {want} does not compile")),
        };
    }
    if let Some((name, spec)) = want.split_once('@') {
        let Some((have_name, have_version)) = have.split_once('@') else {
            return unknown(format!("wants {want}, but the fact {have} has no name@ part"));
        };
        if have_name != name {
            return wants_has(Verdict::No);
        }
        let (Ok(range), Some(v)) = (Range::parse(spec), Version::parse(have_version)) else {
            return unknown(format!("cannot compare {want} with {have}"));
        };
        return wants_has(yes_no(range.contains(&v)));
    }
    if want.contains(':') {
        let (Ok(range), Some(v)) = (Range::parse(want), Version::parse(&have)) else {
            return unknown(format!("cannot compare {want} with {have}"));
        };
        return wants_has(yes_no(range.contains(&v)));
    }
    wants_has(yes_no(want == have))
}

/// Problems that stop a `when` map from being evaluated as written, one message per problem.
pub fn check(when: &Mapping) -> Vec<String> {
    let mut out = vec![];
    let commit = |s: &str| (7..=40).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit());
    for (k, v) in when {
        let key = k.as_str().unwrap_or("?");
        let items: Vec<&Value> = match v {
            Value::Sequence(items) => items.iter().collect(),
            other => vec![other],
        };
        for item in items {
            let Some(s) = item.as_str() else {
                out.push(format!(
                    "quote the value of `{key}` ({}): YAML reads unquoted numbers as numbers, so 1.10 would become 1.1",
                    text(item)
                ));
                continue;
            };
            let problem = if key == "since" || key == "until" {
                (!commit(s)).then(|| format!("`{key}: {s}` must be a commit hash of 7 to 40 hex characters"))
            } else if key == "path" {
                Glob::new(s).err().map(|e| format!("`path: {s}` is not a valid glob: {e}"))
            } else if let Some((_, spec)) = s.split_once('@') {
                Range::parse(spec).err().map(|bad| format!("`{key}: {s}`: version `{bad}` does not parse"))
            } else if s.contains(':') {
                Range::parse(s).err().map(|bad| format!("`{key}: {s}`: version `{bad}` does not parse"))
            } else {
                None
            };
            out.extend(problem);
        }
    }
    out
}

fn text(v: &Value) -> String {
    serde_norway::to_string(v).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Evaluates a lesson's `when` map against the facts. Pure: all I/O happened when `facts` was built.
pub fn evaluate(when: &Mapping, facts: &Facts) -> Applies {
    let mut keys = vec![];
    for (k, v) in when {
        let Some(key) = k.as_str() else { continue };
        let r = match v {
            Value::String(s) => one(key, s, facts),
            Value::Sequence(items) => {
                let rs: Vec<KeyResult> = items
                    .iter()
                    .map(|i| match i {
                        Value::String(s) => one(key, s, facts),
                        other => verdict(key, &text(other), None, Verdict::Unknown, "quote this value".into()),
                    })
                    .collect();
                let result = Verdict::any(rs.iter().map(|r| r.result));
                let want = items.iter().map(|i| i.as_str().map_or_else(|| text(i), str::to_string)).collect::<Vec<_>>().join("|");
                let have = rs.iter().find_map(|r| r.have.clone());
                let reason = match (result, &have) {
                    (Verdict::Yes, Some(h)) => format!("is {h}"),
                    (_, Some(h)) => format!("wants one of {want}, has {h}"),
                    (_, None) => rs[0].reason.clone(),
                };
                verdict(key, &want, have, result, reason)
            }
            other => verdict(key, &text(other), None, Verdict::Unknown, "quote this value".into()),
        };
        keys.push(r);
    }
    Applies { result: Verdict::all(keys.iter().map(|k| k.result)), keys }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap_or_else(|| panic!("{s}"))
    }

    #[test]
    fn version_order() {
        assert!(v("1.10") > v("1.9"));
        assert!(v("1.14rc1") < v("1.14"));
        assert!(v("1.14") < v("1.14.0"));
        assert!(v("1.14rc1") < v("1.14.0"));
        assert_eq!(v("v1.14"), v("1.14"));
        assert!(v("develop") > v("99.0"));
        assert!(v("main") > v("2024.1"));
        assert!(v("1.14.3-2") > v("1.14.3"));
        assert_eq!(v("1.14.3-2").parts, vec![Part::Num(1), Part::Num(14), Part::Num(3), Part::Num(2)]);
        assert_eq!(v("1.14rc1").parts, vec![Part::Num(1), Part::Num(14), Part::Word("rc".into()), Part::Num(1)]);
        assert!(v("1.0alpha") < v("1.0beta"));
        for bad in ["", "unknown build", "1.14 beta", "1/2", "99999999999999999999999"] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn ranges() {
        let r = Range::parse("1.12:1.14.2").unwrap();
        for (x, inside) in [
            ("1.11.9", false),
            ("1.12", true),
            ("1.12rc1", true),
            ("1.12.0", true),
            ("1.13.7", true),
            ("1.14.2", true),
            ("1.14.2.1", true),
            ("1.14.3", false),
            ("2.0", false),
        ] {
            assert_eq!(r.contains(&v(x)), inside, "{x}");
        }
        assert!(Range::parse(":1.14").unwrap().contains(&v("1.14.5")));
        assert!(!Range::parse(":1.14.2").unwrap().contains(&v("1.14.3")));
        assert!(Range::parse("1.12:").unwrap().contains(&v("develop")));
        assert!(Range::parse("12").unwrap().contains(&v("12.3.0")));
        assert_eq!(Range::parse("1.12:1 14"), Err("1 14".into()));
    }

    struct Map(HashMap<&'static str, Option<bool>>);
    impl Ancestry for Map {
        fn is_ancestor(&self, c: &str) -> Option<bool> {
            self.0.get(c).copied().flatten()
        }
    }

    fn facts(pairs: &[(&str, &str)]) -> Facts {
        Facts {
            values: pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            ancestry: Some(Box::new(Map(HashMap::from([("aaaaaaa", Some(true)), ("bbbbbbb", Some(false))])))),
        }
    }

    fn when(y: &str) -> Mapping {
        serde_norway::from_str(y).unwrap()
    }

    fn eval(y: &str, f: &[(&str, &str)]) -> Applies {
        evaluate(&when(y), &facts(f))
    }

    #[test]
    fn one_key_fails_and_unknown() {
        let a = eval("system: tuolumne\nhdf5: \"1.12:1.14.2\"", &[("system", "tuolumne"), ("hdf5", "1.14.3")]);
        assert_eq!(a.result, Verdict::No);
        assert_eq!((a.keys[0].result, a.keys[1].result), (Verdict::Yes, Verdict::No));
        assert!(a.keys[1].reason.contains("1.14.3"));
        assert_eq!(a.summary(), "no (hdf5 wants 1.12:1.14.2, has 1.14.3)");
        assert_eq!(eval("gpu: mi300a", &[]).result, Verdict::Unknown);
        assert_eq!(evaluate(&Mapping::new(), &Facts::default()).result, Verdict::Yes);
    }

    #[test]
    fn value_forms() {
        let list = "gpu:\n  - a100\n  - mi300a";
        assert_eq!(eval(list, &[("gpu", "mi300a")]).result, Verdict::Yes);
        assert_eq!(eval(list, &[("gpu", "h100")]).result, Verdict::No);
        assert_eq!(eval("compiler: \"gcc@12:\"", &[("compiler", "gcc@12.3.0")]).result, Verdict::Yes);
        assert_eq!(eval("compiler: \"gcc@12:\"", &[("compiler", "clang@17.0.1")]).result, Verdict::No);
        assert_eq!(eval("compiler: \"gcc@12:\"", &[("compiler", "12.3.0")]).result, Verdict::Unknown);
        assert_eq!(eval("compiler: \"gcc@12\"", &[("compiler", "gcc@12.3.0")]).result, Verdict::Yes);
        assert_eq!(eval("os: linux", &[("os", "linux")]).result, Verdict::Yes);
        assert_eq!(eval("os: linux", &[("os", "macos")]).result, Verdict::No);
        assert_eq!(eval("hdf5: \"1.14\"", &[("hdf5", "1.14.3")]).result, Verdict::No);
        assert_eq!(eval("hdf5: \"1.12:\"", &[("hdf5", "unknown build")]).result, Verdict::Unknown);
        assert_eq!(eval("hdf5: 1.10", &[("hdf5", "1.10")]).result, Verdict::Unknown);
        assert_eq!(eval("path: \"src/io/**\"", &[]).result, Verdict::Unknown);
        assert_eq!(eval("path: \"src/io/**\"", &[("path", "src/io/writer.cpp")]).result, Verdict::Yes);
        assert_eq!(eval("path: \"src/io/**\"", &[("path", "docs/a.md")]).result, Verdict::No);
    }

    #[test]
    fn since_and_until() {
        assert_eq!(eval("since: aaaaaaa", &[]).result, Verdict::Yes);
        assert_eq!(eval("until: aaaaaaa", &[]).result, Verdict::No);
        assert_eq!(eval("since: bbbbbbb", &[]).result, Verdict::No);
        assert_eq!(eval("since: ccccccc", &[]).result, Verdict::Unknown);
        assert_eq!(evaluate(&when("since: aaaaaaa"), &Facts::default()).result, Verdict::Unknown);
    }

    #[test]
    fn check_reports_what_cannot_be_evaluated() {
        assert!(
            check(&when("hdf5: \"1.12:\"\ncompiler: \"gcc@12:\"\npath: \"src/io/**\"\ngpu:\n  - a100\n  - mi300a\nsince: 0123abc"))
                .is_empty()
        );
        let e = check(&when("hdf5: 1.10"));
        assert!(e[0].contains("quote the value of `hdf5`"), "{e:?}");
        assert!(check(&when("hdf5: \"1.12:1 14\""))[0].contains("`1 14`"));
        assert!(check(&when("compiler: \"gcc@x y\""))[0].contains("does not parse"));
        assert!(check(&when("path: \"src/[io\""))[0].contains("glob"));
        assert!(check(&when("until: main"))[0].contains("commit hash"));
        assert_eq!(check(&when("gpu:\n  - a100\n  - 7")).len(), 1);
    }

    #[test]
    fn gather_merges_sources_in_order() {
        use crate::matching::{Matched, Place, Repo, Rule};
        let kb = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(kb.path().join("systems/tuolumne")).unwrap();
        std::fs::write(kb.path().join("systems/tuolumne/README.md"), "---\nfacts:\n  gpu: mi300a\n---\n").unwrap();
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(repo.path())
                    .args(["-c", "user.name=T", "-c", "user.email=t@example.org"])
                    .args(args)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .unwrap()
                    .success()
            );
        };
        git(&["init", "-q", "-b", "feature-x"]);
        git(&["commit", "-q", "--allow-empty", "-m", "a"]);
        let place = Place {
            project: Some(Matched { name: "dftracer".into(), rule: Rule::Flag }),
            system: Some(Matched { name: "tuolumne".into(), rule: Rule::Flag }),
            repo: Some(Repo { top: repo.path().to_path_buf(), remotes: vec![], root_commits: vec![] }),
        };
        let f = Facts::gather(kb.path(), &place, &[]);
        assert_eq!(f.values["gpu"], "mi300a");
        assert_eq!(f.values["system"], "tuolumne");
        assert_eq!(f.values["project"], "dftracer");
        assert_eq!(f.values["branch"], "feature-x");
        assert!(f.values.contains_key("os") && !f.values.contains_key("path"));
        let f = Facts::gather(kb.path(), &place, &[("gpu".into(), "a100".into()), ("path".into(), "src/a.c".into())]);
        assert_eq!((f.values["gpu"].as_str(), f.values["path"].as_str()), ("a100", "src/a.c"));

        git(&["checkout", "-q", "--detach"]);
        assert!(!Facts::gather(kb.path(), &place, &[]).values.contains_key("branch"));
    }

    #[test]
    fn git_ancestry_in_a_real_repository() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let o = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(["-c", "user.name=T", "-c", "user.email=t@example.org"])
                .args(args)
                .output()
                .unwrap();
            assert!(o.status.success(), "{args:?}");
            String::from_utf8(o.stdout).unwrap().trim().to_string()
        };
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "a"]);
        let first = git(&["rev-parse", "HEAD"]);
        let anc = GitAncestry::new(dir.path());
        assert_eq!(anc.is_ancestor(&first), Some(true));
        assert_eq!(anc.is_ancestor("0123456789abcdef0123456789abcdef01234567"), None);
    }
}
