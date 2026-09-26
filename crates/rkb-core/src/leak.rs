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

        let path_roots: Vec<String> = DEFAULT_PATH_ROOTS
            .iter()
            .map(|r| r.to_string())
            .chain(home.filter(|h| h.len() > 1).map(|h| format!("{}/", h.trim_end_matches('/'))))
            .chain(cfg.paths.iter().cloned())
            .collect();
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
        }
    }

    fn after_root_is_variable(&self, path: &str) -> bool {
        self.path_roots.iter().any(|r| r.ends_with('/') && path.strip_prefix(r.as_str()).is_some_and(|rest| rest.starts_with('$')))
    }

    pub fn scan(&self, text: &str) -> Vec<LeakMatch> {
        let mut out = vec![];
        for (i, line) in text.lines().enumerate() {
            let mut push = |kind, text: &str| {
                if !self.allow.contains(text) {
                    out.push(LeakMatch { kind, line: i + 1, text: text.to_string() });
                }
            };
            for (kind, re) in &self.rules {
                for cap in re.captures_iter(line) {
                    let m = cap.get(1).unwrap_or_else(|| cap.get(0).unwrap()).as_str();
                    if *kind == LeakKind::Email && m.split('@').next() == Some("git") {
                        continue;
                    }
                    // `/scratch/$USER/x` names no one: the part after the root is a variable.
                    if *kind == LeakKind::Path && self.after_root_is_variable(m) {
                        continue;
                    }
                    push(*kind, m);
                }
            }
            // An ASCII Flux ID looks like a word or a hex hash unless it mixes digits and capitals.
            for m in self.flux_ascii.find_iter(line) {
                let tail = &m.as_str()[1..];
                if tail.chars().any(|c| c.is_ascii_digit()) && tail.chars().any(|c| c.is_ascii_uppercase()) {
                    push(LeakKind::JobId, m.as_str());
                }
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
}
