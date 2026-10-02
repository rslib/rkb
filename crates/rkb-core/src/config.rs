use std::collections::BTreeMap;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_norway::{Mapping, Value};

/// `kb.toml`. Sections that later milestones use stay untyped tables until then.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KbConfig {
    #[serde(default)]
    pub labels: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub sinks: BTreeMap<String, Sink>,
    #[serde(default)]
    pub leak: LeakConfig,
    #[serde(default)]
    pub site: SiteConfig,
    pub rerank: Option<toml::Table>,
    pub facts: Option<toml::Table>,
    pub probe: Option<toml::Table>,
    pub verify: Option<toml::Table>,
    pub review: Option<toml::Table>,
    pub hooks: Option<toml::Table>,
    pub lock: Option<toml::Table>,
    pub jev: Option<toml::Table>,
    pub dupes: Option<toml::Table>,
    pub writes: Option<toml::Table>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteConfig {
    pub title: Option<String>,
    pub description: Option<String>,
    pub base_url: Option<String>,
    pub author: Option<String>,
    /// A PNG or JPEG in the knowledge base for link previews, such as `site/og.png`.
    pub image: Option<String>,
    /// `cloudflare`: allow Cloudflare Web Analytics' beacon in the Content-Security-Policy.
    pub analytics: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sink {
    #[serde(default)]
    pub allow: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeakConfig {
    #[serde(default)]
    pub usernames: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub allow: Vec<String>,
}

/// Frontmatter of `projects/<project>/README.md`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectNote {
    #[serde(default)]
    pub remotes: Vec<String>,
    pub root_commit: Option<String>,
    #[serde(default)]
    pub retired: bool,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub meta: Mapping,
}

/// Frontmatter of `systems/<system>/README.md`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemNote {
    #[serde(default)]
    pub hostname: Vec<String>,
    #[serde(default)]
    pub match_env: BTreeMap<String, String>,
    #[serde(default)]
    pub facts: BTreeMap<String, String>,
    #[serde(default)]
    pub retired: bool,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub meta: Mapping,
}

/// Frontmatter of a topic or scope folder note, such as `general/cpp/README.md`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopicNote {
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub meta: Mapping,
}

/// The frontmatter text of a folder note, or `None` when the note has none.
pub fn note_frontmatter(text: &str) -> Option<&str> {
    if !text.starts_with("---\n") && !text.starts_with("---\r\n") {
        return None;
    }
    crate::lesson::split(text).ok().map(|(fm, _, _)| fm)
}

pub fn parse_note<T: DeserializeOwned + Default>(text: &str) -> Result<T, String> {
    match note_frontmatter(text) {
        None if text.starts_with("---") => Err("frontmatter has no closing `---` line".into()),
        None => Ok(T::default()),
        Some(fm) if fm.trim().is_empty() => Ok(T::default()),
        Some(fm) => serde_norway::from_str(fm).map_err(|e| e.to_string()),
    }
}

pub fn parse<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    toml::from_str(text).map_err(|e| e.message().to_string())
}

pub const DEFAULT_KB_TOML: &str = r#"[labels]
sensitivity = ["public", "internal", "confidential"]

[sinks.web]
allow = { sensitivity = ["public"] }

[sinks.web-protected]
allow = { sensitivity = ["public", "internal"] }

[sinks.jev]
allow = { sensitivity = ["public"] }

[rerank]
chain = ["bm25"]

[leak]
usernames = []
paths = []
allow = []
"#;

pub const DEFAULT_SENSITIVITY: &str = "internal";

/// Whether a sink's `allow` lets a lesson with these effective labels through: every key it names
/// must hold one of its values. An empty `allow` lets nothing through.
pub fn sink_allows(allow: &BTreeMap<String, Vec<String>>, labels: &BTreeMap<String, String>) -> bool {
    !allow.is_empty() && allow.iter().all(|(k, vals)| labels.get(k).is_some_and(|v| vals.contains(v)))
}

/// A lesson's labels: its own, then each folder note from nearest to farthest, then `sensitivity = "internal"`.
pub fn effective_labels(lesson: &Mapping, folders: &[&BTreeMap<String, String>]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::from([("sensitivity".to_string(), DEFAULT_SENSITIVITY.to_string())]);
    for folder in folders.iter().rev() {
        out.extend(folder.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    for (k, v) in lesson {
        if let (Value::String(k), Value::String(v)) = (k, v) {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

/// The per-machine `config.toml` in the config folder, never synced: its path and its tables, empty
/// when the file does not exist.
pub fn machine(config_dir: &std::path::Path) -> Result<(std::path::PathBuf, toml::Table), String> {
    let path = config_dir.join("config.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            toml::from_str(&text).map(|t| (path.clone(), t)).map_err(|e| format!("{} does not parse: {}", path.display(), e.message()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((path, toml::Table::new())),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// The harnesses an observer chain can be set for, as `[observer]` keys.
pub const OBSERVER_HARNESSES: [&str; 3] = ["claude-code", "pi", "omp"];

/// A model chain: one model, or a list tried in order.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
enum Chain {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObserverTable {
    #[serde(rename = "claude-code")]
    claude_code: Option<Chain>,
    pi: Option<Chain>,
    omp: Option<Chain>,
    timeout_secs: Option<u64>,
    min_prompts: Option<usize>,
}

/// `[observer]` in the per-machine `config.toml`: a model chain per harness. Without a chain the
/// observer is off for that harness.
#[derive(Debug, Clone, PartialEq)]
pub struct ObserverConfig {
    /// `(harness, models)` in `OBSERVER_HARNESSES` order, only harnesses with at least one model.
    pub chains: Vec<(String, Vec<String>)>,
    pub timeout_secs: u64,
    pub min_prompts: usize,
    /// The small models that decide unsure triage candidates, per harness, from `[triage]`; only for
    /// harnesses with an observer chain, since the same approval covers both. Claude Code gets `haiku`
    /// when `[triage]` names none.
    pub triage: Vec<(String, Vec<String>)>,
}

impl Default for ObserverConfig {
    fn default() -> Self {
        ObserverConfig { chains: vec![], timeout_secs: 600, min_prompts: 3, triage: vec![] }
    }
}

/// `[triage] claude-code`, `pi` and `omp`: a model or a list of models; other keys are thresholds.
fn triage_chains(machine: &toml::Table, observed: &[(String, Vec<String>)]) -> Vec<(String, Vec<String>)> {
    let table = machine.get("triage").and_then(|v| v.as_table());
    observed
        .iter()
        .filter_map(|(h, _)| {
            let models: Vec<String> = match table.and_then(|t| t.get(h.as_str())) {
                Some(toml::Value::String(m)) => vec![m.trim().to_string()],
                Some(toml::Value::Array(ms)) => ms.iter().filter_map(|m| m.as_str().map(|m| m.trim().to_string())).collect(),
                _ if h == "claude-code" => vec!["haiku".into()],
                _ => vec![],
            };
            let models: Vec<String> = models.into_iter().filter(|m| !m.is_empty()).collect();
            (!models.is_empty()).then(|| (h.clone(), models))
        })
        .collect()
}

impl ObserverConfig {
    pub fn from_table(machine: &toml::Table) -> Result<ObserverConfig, String> {
        let Some(v) = machine.get("observer") else { return Ok(ObserverConfig::default()) };
        let t: ObserverTable = v.clone().try_into().map_err(|e: toml::de::Error| format!("[observer]: {}", e.message()))?;
        let d = ObserverConfig::default();
        let mut chains = vec![];
        for (h, c) in OBSERVER_HARNESSES.iter().zip([t.claude_code, t.pi, t.omp]) {
            let models: Vec<String> = match c {
                None => vec![],
                Some(Chain::One(m)) => vec![m],
                Some(Chain::Many(ms)) => ms,
            };
            let models: Vec<String> = models.iter().map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect();
            if !models.is_empty() {
                chains.push((h.to_string(), models));
            }
        }
        let triage = triage_chains(machine, &chains);
        Ok(ObserverConfig {
            chains,
            timeout_secs: t.timeout_secs.unwrap_or(d.timeout_secs),
            min_prompts: t.min_prompts.unwrap_or(d.min_prompts),
            triage,
        })
    }

    /// The models for `harness`, empty when it has no observer.
    pub fn chain(&self, harness: &str) -> &[String] {
        self.chains.iter().find(|(h, _)| h == harness).map_or(&[], |(_, m)| m)
    }

    /// What the user approves: every chain, one line per harness, such as `pi = openai-codex/gpt-5.6-luna, session`.
    /// The triage models for `harness`, empty when it has none.
    pub fn triage_chain(&self, harness: &str) -> &[String] {
        self.triage.iter().find(|(h, _)| h == harness).map_or(&[], |(_, m)| m)
    }

    /// What the approval shows and hashes: every chain that receives session or inbox text.
    pub fn summary(&self) -> String {
        let observer = self.chains.iter().map(|(h, m)| format!("{h} = {}", m.join(", ")));
        let triage = self.triage.iter().map(|(h, m)| format!("triage {h} = {}", m.join(", ")));
        observer.chain(triage).collect::<Vec<_>>().join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_kb_toml_parses() {
        let c: KbConfig = parse(DEFAULT_KB_TOML).unwrap();
        assert_eq!(c.labels["sensitivity"].len(), 3);
        assert_eq!(c.sinks.len(), 3);
    }

    #[test]
    fn site_section() {
        let c: KbConfig = parse("[site]\ntitle = \"Field notes\"\nbase_url = \"https://example.org\"\n").unwrap();
        assert_eq!((c.site.title.as_deref(), c.site.base_url.as_deref()), (Some("Field notes"), Some("https://example.org")));
        assert!(parse::<KbConfig>("[site]\ncolour = \"x\"\n").unwrap_err().contains("colour"));
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(parse::<KbConfig>("[lables]\n").unwrap_err().contains("lables"));
        let e = parse_note::<SystemNote>("---\nenv: {A: b}\n---\n# T\n").unwrap_err();
        assert!(e.contains("env"), "{e}");
        assert!(parse_note::<ProjectNote>("---\nremote: []\n---\n").is_err());
    }

    #[test]
    fn folder_notes() {
        let p: ProjectNote =
            parse_note("---\nremotes: [github.com/llnl/dftracer]\nroot_commit: a1b2\nlabels: {sensitivity: public}\n---\n# dftracer\n")
                .unwrap();
        assert_eq!(p.labels["sensitivity"], "public");
        let s: SystemNote =
            parse_note("---\nhostname: [\"tuolumne*\"]\nmatch_env: {LCSCHEDCLUSTER: tuolumne}\nfacts: {gpu: mi300a}\n---\n").unwrap();
        assert_eq!(s.facts["gpu"], "mi300a");
        let t: TopicNote = parse_note("# C and C++\n\nNo frontmatter.\n").unwrap();
        assert!(t.aliases.is_empty());
        let t: TopicNote = parse_note("---\naliases: [c++, cxx]\n---\n# C and C++\n").unwrap();
        assert_eq!(t.aliases, ["c++", "cxx"]);
        assert!(parse_note::<TopicNote>("---\naliases: [x]\n").is_err());
    }

    #[test]
    fn effective_label_order() {
        let topic = BTreeMap::from([("sensitivity".to_string(), "confidential".to_string())]);
        let project = BTreeMap::from([("sensitivity".to_string(), "public".to_string())]);
        assert_eq!(effective_labels(&Mapping::new(), &[&project])["sensitivity"], "public");
        assert_eq!(effective_labels(&Mapping::new(), &[&topic, &project])["sensitivity"], "confidential");
        assert_eq!(effective_labels(&Mapping::new(), &[])["sensitivity"], "internal");
        let mut own = Mapping::new();
        own.insert("sensitivity".into(), "internal".into());
        assert_eq!(effective_labels(&own, &[&topic, &project])["sensitivity"], "internal");
    }

    #[test]
    fn observer_table() {
        let t: toml::Table = toml::from_str(
            "[observer]\nclaude-code = \"sonnet\"\npi = [\"openai-codex/gpt-5.6-luna\", \" session \"]\nomp = []\nmin_prompts = 5\n",
        )
        .unwrap();
        let o = ObserverConfig::from_table(&t).unwrap();
        assert_eq!(o.chain("claude-code"), ["sonnet"]);
        assert_eq!(o.chain("pi"), ["openai-codex/gpt-5.6-luna", "session"]);
        assert!(o.chain("omp").is_empty(), "an empty list is off");
        assert_eq!((o.timeout_secs, o.min_prompts), (600, 5));
        assert_eq!(o.summary(), "claude-code = sonnet\npi = openai-codex/gpt-5.6-luna, session\ntriage claude-code = haiku");
        assert!(o.triage_chain("pi").is_empty(), "only Claude Code has a default");
        let set: toml::Table = toml::from_str(
            "[observer]\nclaude-code = \"sonnet\"\npi = \"session\"\n[triage]\nclaude-code = [\"claude-haiku-4-5\"]\npi = \"x/small\"\nomp = \"y\"\nkeep = 0.7\n",
        )
        .unwrap();
        let set = ObserverConfig::from_table(&set).unwrap();
        assert_eq!(
            (set.triage_chain("claude-code"), set.triage_chain("pi")),
            (&["claude-haiku-4-5".to_string()][..], &["x/small".to_string()][..])
        );
        assert!(set.triage_chain("omp").is_empty(), "no observer chain for omp, so no triage chain");
        let off = ObserverConfig::from_table(&toml::Table::new()).unwrap();
        assert_eq!(off, ObserverConfig::default());
        assert!(off.chains.is_empty());
        let bad: toml::Table = toml::from_str("[observer]\ncmd = \"claude -p\"\n").unwrap();
        assert!(ObserverConfig::from_table(&bad).unwrap_err().contains("cmd"));
    }

    #[test]
    fn machine_config_missing_is_empty() {
        let d = tempfile::tempdir().unwrap();
        let (path, t) = machine(d.path()).unwrap();
        assert!(t.is_empty() && path.ends_with("config.toml"));
        std::fs::write(d.path().join("config.toml"), "[jev\n").unwrap();
        assert!(machine(d.path()).is_err());
    }
}
