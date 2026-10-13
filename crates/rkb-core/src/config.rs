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

#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
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

/// An extra read-only knowledge base from the machine `[[extra]]` tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extra {
    pub name: String,
    pub path: std::path::PathBuf,
}

/// The `[[extra]]` knowledge bases of the machine `config.toml`, in order. An entry that breaks a
/// rule is an error naming it, so a typo cannot silently drop a whole knowledge base.
pub fn extras(config_dir: &std::path::Path) -> Result<Vec<Extra>, String> {
    let (path, table) = machine(config_dir)?;
    let Some(value) = table.get("extra") else { return Ok(vec![]) };
    let list = value.as_array().ok_or_else(|| format!("{}: [[extra]] must be an array of tables", path.display()))?;
    let mut out: Vec<Extra> = vec![];
    for (i, entry) in list.iter().enumerate() {
        let at = format!("{}: [[extra]] entry {}", path.display(), i + 1);
        let entry = entry.as_table().ok_or_else(|| format!("{at}: it is not a table"))?;
        for key in entry.keys() {
            if key != "name" && key != "path" {
                return Err(format!("{at}: unknown key `{key}`"));
            }
        }
        let name = entry.get("name").and_then(|v| v.as_str()).ok_or_else(|| format!("{at}: it has no name"))?;
        let at = format!("{at} ({name})");
        if !valid_extra_name(name) {
            return Err(format!("{at}: the name must be 1 to 32 characters from `a-z`, `0-9` and `-`"));
        }
        if name == "main" {
            return Err(format!("{at}: `main` belongs to the main knowledge base"));
        }
        if out.iter().any(|e| e.name == name) {
            return Err(format!("{at}: another [[extra]] entry already uses this name"));
        }
        let path_value = entry.get("path").and_then(|v| v.as_str()).ok_or_else(|| format!("{at}: it has no path"))?;
        if !std::path::Path::new(path_value).is_absolute() {
            return Err(format!("{at}: the path `{path_value}` is not absolute"));
        }
        out.push(Extra { name: name.to_string(), path: std::path::PathBuf::from(path_value) });
    }
    Ok(out)
}

fn valid_extra_name(name: &str) -> bool {
    (1..=32).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Where the `[sinks.agent]` in force came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSinkSource {
    /// The machine `config.toml`; it wins whole over `kb.toml`.
    Machine,
    /// `kb.toml`.
    Kb,
}

/// The `[sinks.agent]` in force and where it came from: the machine `config.toml` wins whole over
/// `kb.toml`, and `None` when neither sets one. A machine config that does not parse, or an agent
/// sink with bad fields, is an error that names the file. An empty `allow` lets nothing through,
/// like every other sink.
pub fn agent_sink(root: &std::path::Path, config_dir: &std::path::Path) -> Result<(Option<Sink>, Option<AgentSinkSource>), String> {
    let (path, table) = machine(config_dir)?;
    if let Some(sink) = agent_sink_in(&table, &path)? {
        return Ok((Some(sink), Some(AgentSinkSource::Machine)));
    }
    let text = std::fs::read_to_string(root.join("kb.toml")).unwrap_or_default();
    if text.trim().is_empty() {
        return Ok((None, None));
    }
    // A broken `kb.toml` could hide a sink, so it fails closed like a broken machine config.
    let kb = parse::<KbConfig>(&text).map_err(|e| format!("kb.toml does not parse: {e}"))?;
    Ok(kb.sinks.get("agent").cloned().map_or((None, None), |s| (Some(s), Some(AgentSinkSource::Kb))))
}

/// The `[sinks.agent]` of one config table. Unknown keys and mistyped values are errors, never
/// silently dropped, so a stricter-looking typo cannot open the sink.
fn agent_sink_in(table: &toml::Table, path: &std::path::Path) -> Result<Option<Sink>, String> {
    let Some(sinks) = table.get("sinks") else { return Ok(None) };
    let sinks = sinks.as_table().ok_or_else(|| format!("{}: [sinks] must be a table", path.display()))?;
    let Some(agent) = sinks.get("agent") else { return Ok(None) };
    agent.clone().try_into::<Sink>().map(Some).map_err(|e: toml::de::Error| format!("{}: [sinks.agent] {}", path.display(), e.message()))
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

    fn agent_sink_dirs() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let (root, config) = (d.path().join("kb"), d.path().join("config"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        (d, root, config)
    }

    #[test]
    fn agent_sink_needs_no_config() {
        let (_d, root, config) = agent_sink_dirs();
        assert_eq!(agent_sink(&root, &config).unwrap(), (None, None));
    }

    #[test]
    fn agent_sink_machine_wins_whole() {
        let (_d, root, config) = agent_sink_dirs();
        std::fs::write(root.join("kb.toml"), "[sinks.agent]\nallow = { sensitivity = [\"public\", \"internal\", \"confidential\"] }\n")
            .unwrap();
        std::fs::write(config.join("config.toml"), "[sinks.agent]\nallow = { sensitivity = [\"public\"] }\n").unwrap();
        let (sink, source) = agent_sink(&root, &config).unwrap();
        assert_eq!(source, Some(AgentSinkSource::Machine));
        assert_eq!(sink.unwrap().allow["sensitivity"], ["public"]);
    }

    #[test]
    fn agent_sink_falls_back_to_kb() {
        let (_d, root, config) = agent_sink_dirs();
        std::fs::write(root.join("kb.toml"), "[sinks.agent]\nallow = { sensitivity = [\"public\", \"internal\"] }\n").unwrap();
        let (sink, source) = agent_sink(&root, &config).unwrap();
        assert_eq!(source, Some(AgentSinkSource::Kb));
        assert_eq!(sink.unwrap().allow["sensitivity"], ["public", "internal"]);
    }

    #[test]
    fn agent_sink_fails_closed() {
        let (_d, root, config) = agent_sink_dirs();
        std::fs::write(config.join("config.toml"), "[sinks.agent\n").unwrap();
        let e = agent_sink(&root, &config).unwrap_err();
        assert!(e.contains("config.toml") && e.contains("does not parse"), "{e}");
        std::fs::write(config.join("config.toml"), "[sinks.agent]\nbogus = 1\n").unwrap();
        let e = agent_sink(&root, &config).unwrap_err();
        assert!(e.contains("config.toml") && e.contains("[sinks.agent]"), "{e}");
        std::fs::write(config.join("config.toml"), "[sinks]\nagent = \"x\"\n").unwrap();
        assert!(agent_sink(&root, &config).is_err());
        std::fs::remove_file(config.join("config.toml")).unwrap();
        std::fs::write(root.join("kb.toml"), "[sinks.agent\n").unwrap();
        assert!(agent_sink(&root, &config).unwrap_err().contains("kb.toml"), "a broken kb.toml fails closed too");
    }
}
