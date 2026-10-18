use std::collections::HashSet;

use regex::Regex;

use crate::config::LeakConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeakKind {
    Username,
    Email,
    Path,
    JobId,
}

impl LeakKind {
    pub fn rule(self) -> &'static str {
        match self {
            LeakKind::Username => "leak/username",
            LeakKind::Email => "leak/email",
            LeakKind::Path => "leak/path",
            LeakKind::JobId => "leak/job-id",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct LeakMatch {
    pub kind: LeakKind,
    /// 1-based line in the scanned text.
    pub line: usize,
    pub text: String,
}

pub const DEFAULT_PATH_ROOTS: [&str; 8] = ["/home/", "/Users/", "/g/g", "/p/lustre", "/p/vast", "/usr/workspace/", "/scratch/", "/lustre/"];

const BASE58: &str = "1-9A-HJ-NP-Za-km-z";

pub struct LeakScanner {
    rules: Vec<(LeakKind, Regex)>,
    flux_ascii: Regex,
    allow: HashSet<String>,
    path_roots: Vec<String>,
    /// `$HOME` with one trailing `/`, the root that `redact` turns into `~/`.
    home: Option<String>,
}

impl LeakScanner {
    /// `user` and `home` are the current `$USER` and `$HOME`, which always count as leaks.
    pub fn new(cfg: &LeakConfig, user: Option<&str>, home: Option<&str>) -> Self {
        let mut rules = vec![];

        let names: Vec<String> = user
            .into_iter()
            .map(str::to_string)
            .chain(cfg.usernames.iter().cloned())
            .filter(|n| n.len() >= 2)
            .map(|n| regex::escape(&n))
            .collect();
        if !names.is_empty() {
            rules.push((LeakKind::Username, Regex::new(&format!(r"\b(?:{})\b", names.join("|"))).unwrap()));
        }

        rules.push((LeakKind::Email, Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}\b").unwrap()));

        let home = home.filter(|h| h.len() > 1).map(|h| format!("{}/", h.trim_end_matches('/')));
        let path_roots: Vec<String> =
            DEFAULT_PATH_ROOTS.iter().map(|r| r.to_string()).chain(home.clone()).chain(cfg.paths.iter().cloned()).collect();
        let roots: Vec<String> = path_roots.iter().map(|r| regex::escape(r)).collect();
        rules.push((LeakKind::Path, Regex::new(&format!(r#"(?:^|[^A-Za-z0-9_$./~-])((?:{})[^\s"'`)\]>,;]+)"#, roots.join("|"))).unwrap()));

        rules.push((
            LeakKind::JobId,
            Regex::new(&format!(r"(?i:\bjob[_ ]?id\s*[=:]\s*\d+|\bsubmitted batch job \d+|\bSLURM_JOB_ID=\d+)|ƒ[{BASE58}]{{4,}}")).unwrap(),
        ));

        LeakScanner {
            rules,
            path_roots,
            flux_ascii: Regex::new(&format!(r"\bf[{BASE58}]{{6,12}}\b")).unwrap(),
            allow: cfg.allow.iter().cloned().collect(),
            home,
        }
    }

    fn after_root_is_variable(&self, path: &str) -> bool {
        self.path_roots.iter().any(|r| r.ends_with('/') && path.strip_prefix(r.as_str()).is_some_and(|rest| rest.starts_with('$')))
    }

    pub fn scan(&self, text: &str) -> Vec<LeakMatch> {
        text.lines()
            .enumerate()
            .flat_map(|(i, line)| {
                self.matches(line).into_iter().map(move |(kind, r)| LeakMatch { kind, line: i + 1, text: line[r].to_string() })
            })
            .collect()
    }

    /// `text` with each match replaced: a path under `$HOME` by `~/` and the rest of the path, any
    /// other match by a placeholder for its kind. The result can still match when a match hides in
    /// another one more than 4 levels deep, so a caller that must send clean text scans it again.
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for _ in 0..4 {
            let mut changed = false;
            let lines: Vec<String> = out
                .split('\n')
                .map(|line| {
                    let mut found = self.matches(line);
                    found.sort_by_key(|(_, r)| (r.start, std::cmp::Reverse(r.end)));
                    let (mut kept, mut end) = (vec![], 0);
                    for (kind, r) in found {
                        if r.start >= end {
                            end = r.end;
                            kept.push((kind, r));
                        }
                    }
                    let mut line = line.to_string();
                    for (kind, r) in kept.into_iter().rev() {
                        let with = self.replacement(kind, &line[r.clone()]);
                        line.replace_range(r, &with);
                        changed = true;
                    }
                    line
                })
                .collect();
            out = lines.join("\n");
            if !changed {
                break;
            }
        }
        out
    }

    fn replacement(&self, kind: LeakKind, text: &str) -> String {
        match kind {
            LeakKind::Path => match self.home.as_deref().and_then(|h| text.strip_prefix(h)) {
                Some(rest) => format!("~/{rest}"),
                None => "<path>".to_string(),
            },
            LeakKind::Username => "<user>".to_string(),
            LeakKind::Email => "<email>".to_string(),
            LeakKind::JobId => "<job-id>".to_string(),
        }
    }

    /// The matches in one line that the allow list does not cover, with their byte ranges.
    fn matches(&self, line: &str) -> Vec<(LeakKind, std::ops::Range<usize>)> {
        let mut out = vec![];
        let mut push = |kind, m: regex::Match| {
            if !self.allow.contains(m.as_str()) {
                out.push((kind, m.range()));
            }
        };
        for (kind, re) in &self.rules {
            for cap in re.captures_iter(line) {
                let m = cap.get(1).unwrap_or_else(|| cap.get(0).unwrap());
                if *kind == LeakKind::Email && m.as_str().split('@').next() == Some("git") {
                    continue;
                }
                // `/scratch/$USER/x` names no one: the part after the root is a variable.
                if *kind == LeakKind::Path && self.after_root_is_variable(m.as_str()) {
                    continue;
                }
                push(*kind, m);
            }
        }
        // An ASCII Flux ID looks like a word or a hex hash unless it mixes digits and capitals.
        for m in self.flux_ascii.find_iter(line) {
            let tail = &m.as_str()[1..];
            if tail.chars().any(|c| c.is_ascii_digit()) && tail.chars().any(|c| c.is_ascii_uppercase()) {
                push(LeakKind::JobId, m);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scanner(allow: &[&str]) -> LeakScanner {
        let cfg = LeakConfig {
            usernames: vec!["alice".into()],
            paths: vec!["/p/gpfs1/".into()],
            allow: allow.iter().map(|s| s.to_string()).collect(),
        };
        LeakScanner::new(&cfg, Some("ray"), Some("/Users/ray"))
    }

    fn kinds(text: &str) -> Vec<(LeakKind, String)> {
        scanner(&[]).scan(text).into_iter().map(|m| (m.kind, m.text)).collect()
    }

    #[test]
    fn finds_each_kind() {
        use LeakKind::*;
        assert_eq!(kinds("ask ray about it"), [(Username, "ray".into())]);
        assert_eq!(kinds("alice ran it"), [(Username, "alice".into())]);
        assert_eq!(kinds("mail bob@example.org"), [(Email, "bob@example.org".into())]);
        assert_eq!(kinds("cd /Users/ann/Code/x"), [(Path, "/Users/ann/Code/x".into())]);
        assert_eq!(kinds("ls /g/g20/bob/run"), [(Path, "/g/g20/bob/run".into())]);
        assert_eq!(kinds("cd /scratch/ann1/pip-cache"), [(Path, "/scratch/ann1/pip-cache".into())]);
        assert_eq!(kinds("ls /p/lustre1/$USER/run"), [(Path, "/p/lustre1/$USER/run".into())], "a site path still leaks");
        assert_eq!(kinds("in `/p/gpfs1/x/y`"), [(Path, "/p/gpfs1/x/y".into())]);
        assert_eq!(kinds("jobid=123456"), [(JobId, "jobid=123456".into())]);
        assert_eq!(kinds("Submitted batch job 4242"), [(JobId, "Submitted batch job 4242".into())]);
        assert_eq!(kinds("SLURM_JOB_ID=99"), [(JobId, "SLURM_JOB_ID=99".into())]);
        assert_eq!(kinds("flux job ƒ2BZ6cM3B done"), [(JobId, "ƒ2BZ6cM3B".into())]);
        assert_eq!(kinds("flux job f2BZ6cM3B done"), [(JobId, "f2BZ6cM3B".into())]);
    }

    #[test]
    fn line_numbers() {
        let m = scanner(&[]).scan("ok\nok\nbob@example.org\n");
        assert_eq!(m[0].line, 3);
    }

    #[test]
    fn no_false_matches() {
        for text in [
            "bandwidth drops by 123456 bytes",
            "function fallback f16 f2fs fPIC",
            "id f3a9c2b41e and commit fab12cd9e",
            "$HOME/Code/dftracer/build",
            "${SCRATCH}/run and $PROJECT_ROOT/src",
            "/usr/local/home/share",
            "https://example.org/home/page",
            "git@github.com:llnl/dftracer.git",
            "gcc@12.3.0 and hdf5@1.14.3",
            "Ray tracing is case-sensitive",
            "the /home/ directory",
            "/scratchpad/tmp",
            "export PIP_CACHE_DIR=/scratch/$USER/pip-cache",
            "cd /home/${USER}/src",
        ] {
            assert_eq!(kinds(text), [], "{text}");
        }
    }

    #[test]
    fn allow_list_is_exact() {
        let s = scanner(&["bob@example.org"]);
        assert!(s.scan("bob@example.org").is_empty());
        assert_eq!(s.scan("bob@example.com").len(), 1);
    }

    #[test]
    fn redact_replaces_each_kind() {
        let s = scanner(&[]);
        assert_eq!(s.redact("cd /Users/ray/Code/x && ls"), "cd ~/Code/x && ls");
        assert_eq!(s.redact("cd /Users/ann/Code/x"), "cd <path>");
        assert_eq!(s.redact("mail bob@example.org now"), "mail <email> now");
        assert_eq!(s.redact("alice ran it"), "<user> ran it");
        assert_eq!(s.redact("Submitted batch job 4242"), "<job-id>");
        assert_eq!(s.redact("flux job f2BZ6cM3B done"), "flux job <job-id> done");
        assert_eq!(s.redact("a\n/Users/ray/x\nb"), "a\n~/x\nb", "keeps the lines");
    }

    #[test]
    fn redact_leaves_clean_text() {
        let s = scanner(&[]);
        for text in ["no leak here", "/Users/ray/alice/run", "/Users/alice/run and ray@example.org", "x\n\n"] {
            assert!(s.scan(&s.redact(text)).is_empty(), "{text} -> {}", s.redact(text));
        }
        assert_eq!(s.redact("x\n\n"), "x\n\n");
        assert_eq!(s.redact("/Users/ray/alice/run"), "~/<user>/run", "a name inside a home path");
    }

    #[test]
    fn redact_keeps_multibyte_text() {
        let s = scanner(&[]);
        assert_eq!(s.redact("é job ƒ2BZ6cM3B ü /Users/ray/é/x"), "é job <job-id> ü ~/é/x");
    }

    #[test]
    fn redact_keeps_allowed_text() {
        assert_eq!(scanner(&["bob@example.org"]).redact("mail bob@example.org"), "mail bob@example.org");
    }
}
