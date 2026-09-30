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

/// `[observer]` in the per-machine `config.toml`. Without `cmd` the observer is off.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ObserverConfig {
    pub cmd: Option<String>,
    pub timeout_secs: u64,
    pub min_prompts: usize,
}

impl Default for ObserverConfig {
    fn default() -> Self {
        ObserverConfig { cmd: None, timeout_secs: 600, min_prompts: 3 }
    }
}

impl ObserverConfig {
    pub fn from_table(machine: &toml::Table) -> Result<ObserverConfig, String> {
        match machine.get("observer") {
            None => Ok(ObserverConfig::default()),
            Some(v) => v.clone().try_into().map_err(|e: toml::de::Error| format!("[observer]: {}", e.message())),
        }
    }

    /// The command, when one is set and not blank.
    pub fn command(&self) -> Option<&str> {
        self.cmd.as_deref().map(str::trim).filter(|c| !c.is_empty())
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
        let t: toml::Table = toml::from_str("[observer]\ncmd = \"claude -p\"\nmin_prompts = 5\n").unwrap();
        let o = ObserverConfig::from_table(&t).unwrap();
        assert_eq!(o.command(), Some("claude -p"));
        assert_eq!((o.timeout_secs, o.min_prompts), (600, 5));
        let off = ObserverConfig::from_table(&toml::Table::new()).unwrap();
        assert_eq!(off, ObserverConfig::default());
        assert_eq!(off.command(), None);
        let blank: toml::Table = toml::from_str("[observer]\ncmd = \"  \"\n").unwrap();
        assert_eq!(ObserverConfig::from_table(&blank).unwrap().command(), None);
        let bad: toml::Table = toml::from_str("[observer]\ncommand = \"x\"\n").unwrap();
        assert!(ObserverConfig::from_table(&bad).is_err());
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
