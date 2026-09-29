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
allow = { sensitivity = ["public", "internal"] }

[rerank]
chain = ["bm25"]

[leak]
usernames = []
paths = []
allow = []
"#;

pub const DEFAULT_SENSITIVITY: &str = "internal";

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
}
