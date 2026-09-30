use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result, io};

/// The rkb skill, built into the binary so an installed copy always matches it.
pub const SKILL: &str = include_str!("../../../skills/rkb/SKILL.md");
/// The `/rkb-retro`, `/rkb-distill` and `/rkb-curate` command prompts: file name and text with frontmatter.
pub const COMMANDS: [(&str, &str); 3] = [
    ("rkb-retro.md", include_str!("../../../skills/rkb/commands/rkb-retro.md")),
    ("rkb-distill.md", include_str!("../../../skills/rkb/commands/rkb-distill.md")),
    ("rkb-curate.md", include_str!("../../../skills/rkb/commands/rkb-curate.md")),
];
/// The line in every command file rkb writes; uninstall removes only files that have it.
const COMMAND_MARKER: &str = "\ngenerated-by: rkb install";

/// A command prompt without its frontmatter, as a harness sends it to the model.
pub fn command_body(text: &str) -> &str {
    text.strip_prefix("---\n").and_then(|t| t.split_once("\n---\n")).map_or(text, |(_, body)| body.trim_start())
}

/// The Claude Code permission rule that asks the user before `rkb confirm`.
pub const ASK_RULE: &str = "Bash(rkb confirm:*)";
/// The plugin's MCP tools, allowed without a prompt: every rkb write that needs the user still asks.
pub const TOOL_RULES: [&str; 4] =
    ["mcp__plugin_rkb_rkb__rkb_search", "mcp__plugin_rkb_rkb__rkb_show", "mcp__plugin_rkb_rkb__rkb_add", "mcp__plugin_rkb_rkb__rkb_note"];
/// The Claude Code plugin id: plugin `rkb` from the local marketplace `rkb`.
pub const PLUGIN: &str = "rkb@rkb";

/// The local marketplace rkb writes for Claude Code: `$XDG_DATA_HOME/rkb/claude-plugin`.
pub fn plugin_dir(home: &Path) -> PathBuf {
    // Unit tests pass a temporary home; the real XDG_DATA_HOME must never leak into them.
    if cfg!(test) {
        return home.join(".local/share/rkb/claude-plugin");
    }
    std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map_or_else(|| home.join(".local/share"), PathBuf::from)
        .join("rkb/claude-plugin")
}

/// The `claude` program install runs: `RKB_CLAUDE` when set, so tests never touch a real Claude Code.
pub fn claude_program() -> String {
    // A unit test that forgets the fake must fail, never reach the user's real Claude Code.
    if cfg!(test) {
        return "/nonexistent/claude-in-unit-tests".into();
    }
    std::env::var("RKB_CLAUDE").unwrap_or_else(|_| "claude".into())
}

/// The marketplace files, relative to `plugin_dir`, and the plugin version, which carries a hash of
/// the content so a changed binary path or prompt makes `claude plugin update` pick it up.
/// The Claude Code model for the distill command: `claude-code` under `[distill]` in this machine's
/// `config.toml`, when it is a plain model name. Unit tests never read the user's file.
pub fn distill_model() -> Option<String> {
    if cfg!(test) {
        return None;
    }
    let (_, t) = crate::config::machine(&crate::paths::config_dir()).ok()?;
    let m = t.get("distill")?.get("claude-code")?.as_str()?.trim().to_string();
    (!m.is_empty() && m.chars().all(|c| c.is_ascii_alphanumeric() || "._:/-[]".contains(c))).then_some(m)
}

/// The distill command with this machine's batch size. Unit tests use the default.
fn distill_command() -> String {
    let batch = if cfg!(test) { crate::distill::DEFAULT_BATCH } else { crate::distill::batch(&crate::paths::config_dir()) };
    COMMANDS[1].1.replace("__BATCH__", &batch.to_string())
}

/// A command file with `model: <model>` added to its frontmatter, so Claude Code runs it on that model.
fn with_model(command: &str, model: Option<&str>) -> String {
    match (model, command.strip_prefix("---\n")) {
        (Some(m), Some(rest)) => format!("---\nmodel: {m}\n{rest}"),
        _ => command.to_string(),
    }
}

pub fn plugin_files(bin: &str) -> (Vec<(String, String)>, String) {
    use sha2::{Digest, Sha256};
    let mut hooks = serde_json::Map::new();
    for (event, matcher, name) in CLAUDE_HOOKS {
        let mut entry = serde_json::json!({ "hooks": [{ "type": "command", "command": format!("{bin} hook {name}"), "timeout": 5 }] });
        if let Some(m) = matcher {
            entry["matcher"] = m.into();
        }
        hooks.insert(event.to_string(), serde_json::json!([entry]));
    }
    let pretty = |v: serde_json::Value| serde_json::to_string_pretty(&v).expect("json serializes") + "\n";
    let mut files = vec![
        ("plugins/rkb/skills/rkb/SKILL.md".to_string(), SKILL.to_string()),
        ("plugins/rkb/commands/retro.md".to_string(), COMMANDS[0].1.to_string()),
        ("plugins/rkb/commands/distill.md".to_string(), with_model(&distill_command(), distill_model().as_deref())),
        ("plugins/rkb/commands/curate.md".to_string(), COMMANDS[2].1.to_string()),
        ("plugins/rkb/hooks/hooks.json".to_string(), pretty(serde_json::json!({ "hooks": hooks }))),
        ("plugins/rkb/.mcp.json".to_string(), pretty(serde_json::json!({ "mcpServers": { "rkb": { "command": bin, "args": ["mcp"] } } }))),
    ];
    let mut h = Sha256::new();
    for (path, text) in &files {
        h.update(path.as_bytes());
        h.update(text.as_bytes());
    }
    let hash: String = h.finalize().iter().take(6).map(|b| format!("{b:02x}")).collect();
    let version = format!("{}+{hash}", env!("CARGO_PKG_VERSION"));
    let about = "rkb: search, read and record lessons learned in your knowledge base";
    let author = serde_json::json!({ "name": "rkb" });
    files.push((
        ".claude-plugin/marketplace.json".into(),
        pretty(serde_json::json!({
            "name": "rkb",
            "owner": author,
            "metadata": { "description": "The rkb plugin, written by `rkb install claude`" },
            "plugins": [{ "name": "rkb", "source": "./plugins/rkb", "description": about }],
        })),
    ));
    files.push((
        "plugins/rkb/.claude-plugin/plugin.json".into(),
        pretty(serde_json::json!({ "name": "rkb", "version": version, "description": about, "author": author })),
    ));
    (files, version)
}

/// The version of the rkb plugin Claude Code has installed, from `~/.claude/plugins/installed_plugins.json`.
pub fn installed_plugin_version(home: &Path) -> Option<String> {
    let text = std::fs::read_to_string(home.join(".claude/plugins/installed_plugins.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v["plugins"][PLUGIN].as_array()?.first()?["version"].as_str().map(str::to_string)
}

/// The harness name `trust.toml` and `request::trusted_harness` use for Claude Code.
pub const CLAUDE_TRUST: &str = "claude-code";

/// The pi and omp extension, with `__HARNESS__` where the harness name goes.
const EXTENSION: &str = include_str!("../../../extensions/rkb/src/rkb.ts");
/// The first line of every extension file rkb writes; uninstall removes only files that start with it.
const EXTENSION_MARKER: &str = "// Generated by rkb install;";

/// The Claude Code hook events rkb adds: event, tool matcher, and the `rkb hook` event it runs.
pub const CLAUDE_HOOKS: [(&str, Option<&str>, &str); 8] = [
    ("SessionStart", None, "session-start"),
    ("PreToolUse", Some("Bash"), "pre-tool"),
    ("PostToolUse", Some("Bash"), "tool-ok"),
    ("PostToolUseFailure", Some("Bash"), "tool-failed"),
    ("UserPromptSubmit", None, "prompt"),
    ("Stop", None, "stop"),
    ("PreCompact", None, "pre-compact"),
    ("SessionEnd", None, "session-end"),
];

/// The command of one of rkb's hook entries: `rkb hook <event>`, or a path to a binary whose name
/// starts with `rkb`, then ` hook ` and one of rkb's events.
static HOOK_COMMAND: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    let events: Vec<&str> = CLAUDE_HOOKS.iter().map(|h| h.2).collect();
    regex::Regex::new(&format!(r"^(?:\S*/)?rkb[^/\s]* hook (?:{})$", events.join("|"))).unwrap()
});

/// What hooks and the extension run: `rkb` when that name on `PATH` is this binary, which survives a
/// reinstall to the same place; otherwise this binary's absolute path, so they never run another rkb.
pub fn hook_binary() -> String {
    hook_binary_with(std::env::var_os("PATH").as_deref(), std::env::current_exe().ok().as_deref())
}

fn hook_binary_with(path: Option<&std::ffi::OsStr>, exe: Option<&Path>) -> String {
    let Some(exe) = exe else { return "rkb".into() };
    let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
    let on_path = path.and_then(|p| std::env::split_paths(p).map(|d| d.join("rkb")).find(|f| f.is_file()));
    match on_path.and_then(|f| f.canonicalize().ok()) {
        Some(found) if found == exe => "rkb".into(),
        _ => exe.display().to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    Claude,
    Pi,
    Omp,
}

impl Harness {
    pub const ALL: [Harness; 3] = [Harness::Claude, Harness::Pi, Harness::Omp];

    pub fn name(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Pi => "pi",
            Harness::Omp => "omp",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|h| h.name() == name)
    }

    /// The harness's own folder; the harness counts as detected when it exists.
    pub fn home(self, home: &Path) -> PathBuf {
        match self {
            Harness::Claude => home.join(".claude"),
            Harness::Pi => home.join(".pi"),
            Harness::Omp => home.join(".omp"),
        }
    }

    pub fn skill_dir(self, home: &Path) -> PathBuf {
        match self {
            Harness::Claude => home.join(".claude/skills/rkb"),
            Harness::Pi => home.join(".pi/agent/skills/rkb"),
            Harness::Omp => home.join(".omp/agent/skills/rkb"),
        }
    }

    pub fn detected(self, home: &Path) -> bool {
        self.home(home).is_dir()
    }

    /// The name `trust.toml`, `RKB_HARNESS` and `rkb hook --harness` use.
    pub fn trust_name(self) -> &'static str {
        match self {
            Harness::Claude => CLAUDE_TRUST,
            Harness::Pi => "pi",
            Harness::Omp => "omp",
        }
    }

    /// The rkb extension file, for harnesses that load TypeScript extensions.
    pub fn extension_file(self, home: &Path) -> Option<PathBuf> {
        match self {
            Harness::Claude => None,
            Harness::Pi => Some(home.join(".pi/agent/extensions/rkb.ts")),
            Harness::Omp => Some(home.join(".omp/agent/extensions/rkb.ts")),
        }
    }

    /// The extension text for this harness.
    pub fn extension(self) -> String {
        let json = |t: &str| serde_json::to_string(command_body(t)).expect("a string serializes");
        EXTENSION
            .replace("__HARNESS__", self.trust_name())
            .replace("\"__RETRO__\"", &json(COMMANDS[0].1))
            .replace("\"__DISTILL__\"", &json(&distill_command()))
            .replace("\"__CURATE__\"", &json(COMMANDS[2].1))
            .replace("\"__RKB__\"", &serde_json::to_string(&hook_binary()).expect("a string serializes"))
            .replace(
                "\"__TOOLS__\"",
                &serde_json::to_string(&serde_json::to_string(&crate::tools::definitions()).expect("definitions serialize"))
                    .expect("a string serializes"),
            )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    WriteSkill { dir: PathBuf },
    RemoveSkill { dir: PathBuf },
    AddAskRule { settings: PathBuf },
    RemoveAskRule { settings: PathBuf },
    AddHooks { settings: PathBuf, bin: String },
    RemoveHooks { settings: PathBuf },
    WriteExtension { file: PathBuf, text: String },
    RemoveExtension { file: PathBuf },
    WriteCommand { file: PathBuf, text: &'static str },
    RemoveCommand { file: PathBuf },
    WritePlugin { dir: PathBuf, bin: String },
    ClaudePlugin { dir: PathBuf, program: String, bin: String },
    RemovePlugin { dir: PathBuf, program: String },
    Trust { file: PathBuf, name: &'static str },
    Untrust { file: PathBuf, name: &'static str },
}

impl Step {
    /// The file this step writes or removes.
    pub fn file(&self) -> PathBuf {
        match self {
            Step::WriteSkill { dir } | Step::RemoveSkill { dir } => dir.join("SKILL.md"),
            Step::AddAskRule { settings }
            | Step::RemoveAskRule { settings }
            | Step::AddHooks { settings, .. }
            | Step::RemoveHooks { settings } => settings.clone(),
            Step::WriteExtension { file, .. } | Step::RemoveExtension { file } => file.clone(),
            Step::WriteCommand { file, .. } | Step::RemoveCommand { file } => file.clone(),
            Step::WritePlugin { dir, .. } | Step::RemovePlugin { dir, .. } => dir.clone(),
            Step::ClaudePlugin { .. } => PathBuf::from(format!("claude plugin {PLUGIN}")),
            Step::Trust { file, .. } | Step::Untrust { file, .. } => file.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    Written,
    Unchanged,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: PathBuf,
    pub effect: Effect,
}

/// The steps for `harnesses`. Reads nothing and writes nothing.
pub fn plan(home: &Path, config_dir: &Path, harnesses: &[Harness], uninstall: bool) -> Vec<Step> {
    let mut steps = vec![];
    for h in harnesses {
        let dir = h.skill_dir(home);
        if *h == Harness::Claude {
            let settings = home.join(".claude/settings.json");
            let (plugin, program) = (plugin_dir(home), claude_program());
            if uninstall {
                steps.push(Step::RemovePlugin { dir: plugin, program });
                steps.push(Step::RemoveAskRule { settings: settings.clone() });
            } else {
                let bin = hook_binary();
                steps.push(Step::WritePlugin { dir: plugin.clone(), bin: bin.clone() });
                steps.push(Step::ClaudePlugin { dir: plugin, program, bin });
                steps.push(Step::AddAskRule { settings: settings.clone() });
            }
            // What earlier versions wrote outside the plugin.
            steps.push(Step::RemoveHooks { settings });
            for (name, _) in COMMANDS {
                steps.push(Step::RemoveCommand { file: home.join(".claude/commands").join(name) });
            }
            steps.push(Step::RemoveSkill { dir });
        } else {
            steps.push(if uninstall { Step::RemoveSkill { dir } } else { Step::WriteSkill { dir } });
        }
        if let Some(file) = h.extension_file(home) {
            steps.push(if uninstall { Step::RemoveExtension { file } } else { Step::WriteExtension { file, text: h.extension() } });
        }
        let (file, name) = (config_dir.join("trust.toml"), h.trust_name());
        steps.push(if uninstall { Step::Untrust { file, name } } else { Step::Trust { file, name } });
    }
    steps
}

fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let dir = path.parent().expect("absolute path");
    std::fs::create_dir_all(dir).map_err(io(dir))?;
    let tmp = dir.join(format!(".{}.rkb-tmp", path.file_name().unwrap().to_string_lossy()));
    std::fs::write(&tmp, text).map_err(io(&tmp))?;
    std::fs::rename(&tmp, path).map_err(io(path))
}

fn read_settings(path: &Path) -> Result<serde_json::Value> {
    match std::fs::read_to_string(path) {
        Err(_) => Ok(serde_json::json!({})),
        Ok(t) if t.trim().is_empty() => Ok(serde_json::json!({})),
        Ok(t) => serde_json::from_str(&t).map_err(|e| Error::Refused(format!("{} is not valid JSON: {e}", path.display()))),
    }
}

fn write_settings(path: &Path, v: &serde_json::Value) -> Result<()> {
    let text = serde_json::to_string_pretty(v).expect("json serializes") + "\n";
    write_atomic(path, &text)
}

/// Adds or removes the ask rule. Returns whether the file changed.
/// Adds or removes rkb's rules: `ASK_RULE` under `permissions.ask`, `TOOL_RULES` under `permissions.allow`.
fn ask_rule(path: &Path, add: bool) -> Result<bool> {
    let mut v = read_settings(path)?;
    if !v.is_object() {
        return Err(Error::Refused(format!("{} is not a JSON object", path.display())));
    }
    let mut changed = false;
    for (list, rules) in [("ask", &[ASK_RULE][..]), ("allow", &TOOL_RULES[..])] {
        for rule in rules {
            let has = v["permissions"][list].as_array().is_some_and(|a| a.iter().any(|r| r == rule));
            if add && !has {
                let perms = v.as_object_mut().unwrap().entry("permissions").or_insert_with(|| serde_json::json!({}));
                perms
                    .as_object_mut()
                    .ok_or_else(|| Error::Refused(format!("`permissions` in {} is not an object", path.display())))?
                    .entry(list)
                    .or_insert_with(|| serde_json::json!([]))
                    .as_array_mut()
                    .ok_or_else(|| Error::Refused(format!("`permissions.{list}` in {} is not a list", path.display())))?
                    .push((*rule).into());
                changed = true;
            } else if !add && has {
                v["permissions"][list].as_array_mut().unwrap().retain(|r| r != rule);
                changed = true;
            }
        }
    }
    if !add {
        // Leave no empty list or block behind that only rkb's rules needed.
        if let Some(perms) = v["permissions"].as_object_mut() {
            perms.retain(|_, l| l.as_array().is_none_or(|a| !a.is_empty()));
            if perms.is_empty() {
                v.as_object_mut().unwrap().remove("permissions");
            }
        }
    }
    if changed {
        write_settings(path, &v)?;
    }
    Ok(changed)
}

fn run_claude(program: &str, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| Error::Refused(format!("cannot run `{program}`: {e}")))?;
    if !out.status.success() {
        let text = String::from_utf8_lossy(&out.stderr).into_owned() + &String::from_utf8_lossy(&out.stdout);
        let tail: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).rev().take(3).collect();
        return Err(Error::Refused(format!(
            "`claude {}` failed: {}",
            args.join(" "),
            tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn program_found(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
}

fn marketplace_known(program: &str) -> Result<bool> {
    let v: serde_json::Value =
        serde_json::from_str(&run_claude(program, &["plugin", "marketplace", "list", "--json"])?).unwrap_or_default();
    Ok(v.as_array().is_some_and(|a| a.iter().any(|m| m["name"] == "rkb")))
}

fn plugin_version(program: &str) -> Result<Option<String>> {
    let v: serde_json::Value = serde_json::from_str(&run_claude(program, &["plugin", "list", "--json"])?).unwrap_or_default();
    Ok(v.as_array().and_then(|a| a.iter().find(|p| p["id"] == PLUGIN)).and_then(|p| p["version"].as_str()).map(str::to_string))
}

fn write_plugin(dir: &Path, bin: &str) -> Result<bool> {
    let mut changed = false;
    for (rel, text) in plugin_files(bin).0 {
        let file = dir.join(rel);
        if std::fs::read_to_string(&file).is_ok_and(|t| t == text) {
            continue;
        }
        write_atomic(&file, &text)?;
        changed = true;
    }
    Ok(changed)
}

/// Makes Claude Code run the plugin in `dir`: adds the marketplace, then installs or updates the plugin.
fn claude_plugin(dir: &Path, program: &str, bin: &str) -> Result<bool> {
    let dir_text = dir.display().to_string();
    let mut changed = false;
    if !marketplace_known(program)? {
        run_claude(program, &["plugin", "marketplace", "add", &dir_text])?;
        changed = true;
    }
    let want = plugin_files(bin).1;
    match plugin_version(program)? {
        None => {
            run_claude(program, &["plugin", "install", PLUGIN])?;
            changed = true;
        }
        Some(v) if v != want => {
            run_claude(program, &["plugin", "marketplace", "update", "rkb"])?;
            run_claude(program, &["plugin", "update", PLUGIN])?;
            changed = true;
        }
        Some(_) => {}
    }
    Ok(changed)
}

fn remove_plugin(dir: &Path, program: &str) -> Result<bool> {
    let mut changed = false;
    if program_found(program) {
        if plugin_version(program)?.is_some() {
            run_claude(program, &["plugin", "uninstall", PLUGIN])?;
            changed = true;
        }
        if marketplace_known(program)? {
            run_claude(program, &["plugin", "marketplace", "remove", "rkb"])?;
            changed = true;
        }
    }
    let ours = std::fs::read_to_string(dir.join(".claude-plugin/marketplace.json")).is_ok_and(|t| t.contains("\"./plugins/rkb\""));
    if ours {
        std::fs::remove_dir_all(dir).map_err(io(dir))?;
        changed = true;
    }
    Ok(changed)
}

fn is_rkb_command(c: &serde_json::Value) -> bool {
    c.as_str().is_some_and(|c| HOOK_COMMAND.is_match(c))
}

fn is_rkb_entry(entry: &serde_json::Value) -> bool {
    entry["hooks"].as_array().is_some_and(|hs| hs.iter().any(|h| is_rkb_command(&h["command"])))
}

/// With `Some(bin)`, adds rkb's entries under `hooks` running `bin`, and points existing ones at `bin`;
/// with `None`, removes them. Keeps the user's own. Returns whether the file changed.
fn hooks(path: &Path, bin: Option<&str>) -> Result<bool> {
    let mut v = read_settings(path)?;
    let not_object = |what: &str| Error::Refused(format!("`{what}` in {} is not an object", path.display()));
    let root = v.as_object_mut().ok_or_else(|| not_object("the file"))?;
    let mut changed = false;
    if let Some(bin) = bin {
        let block = root.entry("hooks").or_insert_with(|| serde_json::json!({})).as_object_mut().ok_or_else(|| not_object("hooks"))?;
        for (event, matcher, name) in CLAUDE_HOOKS {
            let command = format!("{bin} hook {name}");
            let list = block
                .entry(event)
                .or_insert_with(|| serde_json::json!([]))
                .as_array_mut()
                .ok_or_else(|| Error::Refused(format!("`hooks.{event}` in {} is not a list", path.display())))?;
            let suffix = format!(" hook {name}");
            let mut found = false;
            for h in list.iter_mut().filter_map(|e| e["hooks"].as_array_mut()).flatten() {
                if is_rkb_command(&h["command"]) && h["command"].as_str().is_some_and(|c| c.ends_with(&suffix)) {
                    found = true;
                    if h["command"] != command.as_str() {
                        h["command"] = command.clone().into();
                        changed = true;
                    }
                }
            }
            if found {
                continue;
            }
            let mut entry = serde_json::json!({ "hooks": [{ "type": "command", "command": command, "timeout": 5 }] });
            if let Some(m) = matcher {
                entry = serde_json::json!({ "matcher": m, "hooks": entry["hooks"] });
            }
            list.push(entry);
            changed = true;
        }
    } else if let Some(block) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for list in block.values_mut() {
            if let Some(list) = list.as_array_mut() {
                let before = list.len();
                list.retain(|e| !is_rkb_entry(e));
                changed |= list.len() != before;
            }
        }
        if changed {
            block.retain(|_, list| list.as_array().is_none_or(|l| !l.is_empty()));
            if block.is_empty() {
                root.remove("hooks");
            }
        }
    }
    if changed {
        write_settings(path, &v)?;
    }
    Ok(changed)
}

/// Whether `settings.json` has any of rkb's hook entries.
pub fn hooks_installed(home: &Path) -> bool {
    let Ok(v) = read_settings(&home.join(".claude/settings.json")) else { return false };
    v["hooks"].as_object().is_some_and(|b| b.values().filter_map(|l| l.as_array()).flatten().any(is_rkb_entry))
}

/// Sets or removes `[harness.<name>] gated = true`. Returns whether the file changed.
fn trust(path: &Path, name: &str, add: bool) -> Result<bool> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut table: toml::Table =
        toml::from_str(&text).map_err(|e| Error::Refused(format!("{} does not parse: {}", path.display(), e.message())))?;
    let gated = table.get("harness").and_then(|h| h.get(name)).and_then(|c| c.get("gated")).and_then(|g| g.as_bool());
    if add == (gated == Some(true)) {
        return Ok(false);
    }
    let harness = table.entry("harness").or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let Some(harness) = harness.as_table_mut() else {
        return Err(Error::Refused(format!("`harness` in {} is not a table", path.display())));
    };
    if add {
        let mut entry = toml::Table::new();
        entry.insert("gated".into(), true.into());
        harness.insert(name.into(), entry.into());
    } else {
        harness.remove(name);
    }
    write_atomic(path, &toml::to_string(&table).expect("table serializes"))?;
    Ok(true)
}

fn write_extension(file: &Path, text: &str) -> Result<bool> {
    // A link is replaced, never written through, as for the skill.
    if file.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
        std::fs::remove_file(file).map_err(io(file))?;
    }
    if std::fs::read_to_string(file).is_ok_and(|t| t == text) {
        return Ok(false);
    }
    write_atomic(file, text)?;
    Ok(true)
}

fn remove_extension(file: &Path) -> Result<bool> {
    let ours =
        file.symlink_metadata().is_ok_and(|m| m.is_file()) && std::fs::read_to_string(file).is_ok_and(|t| t.starts_with(EXTENSION_MARKER));
    if !ours {
        return Ok(false);
    }
    std::fs::remove_file(file).map_err(io(file))?;
    Ok(true)
}

fn remove_command(file: &Path) -> Result<bool> {
    // Earlier versions marked the files with an HTML comment instead of the frontmatter key.
    let ours = file.symlink_metadata().is_ok_and(|m| m.is_file())
        && std::fs::read_to_string(file).is_ok_and(|t| t.contains(COMMAND_MARKER) || t.contains("<!-- Generated by rkb install;"));
    if !ours {
        return Ok(false);
    }
    std::fs::remove_file(file).map_err(io(file))?;
    Ok(true)
}

fn write_skill(dir: &Path, skill: &str) -> Result<bool> {
    // A link is replaced, never written through, so a folder elsewhere stays untouched.
    if dir.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
        std::fs::remove_file(dir).map_err(io(dir))?;
    }
    let file = dir.join("SKILL.md");
    if std::fs::read_to_string(&file).is_ok_and(|t| t == skill) {
        return Ok(false);
    }
    write_atomic(&file, skill)?;
    Ok(true)
}

fn remove_skill(dir: &Path) -> Result<bool> {
    if dir.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
        return Ok(false);
    }
    let file = dir.join("SKILL.md");
    if !file.is_file() {
        return Ok(false);
    }
    std::fs::remove_file(&file).map_err(io(&file))?;
    let _ = std::fs::remove_dir(dir);
    Ok(true)
}

/// Runs the steps. Every settings file is parsed first, so an invalid one stops the install before any write.
/// The lines that report `changes` to a person: clean-up steps that found nothing are left out, and a
/// changed Claude Code plugin ends with the restart it needs.
pub fn report(steps: &[Step], changes: &[Change]) -> Vec<String> {
    let quiet: Vec<PathBuf> = steps.iter().filter(|s| !touches(s)).map(Step::file).collect();
    let plugin = PathBuf::from(format!("claude plugin {PLUGIN}"));
    let mut lines: Vec<String> = changes
        .iter()
        .filter(|c| !(c.effect == Effect::Unchanged && quiet.contains(&c.path)))
        .map(|c| format!("{:<9} {}", format!("{:?}", c.effect).to_lowercase(), c.path.display()))
        .collect();
    if changes.iter().any(|c| c.path == plugin && c.effect != Effect::Unchanged) {
        lines.push("Restart Claude Code to load the rkb plugin.".into());
    }
    lines
}

/// Whether the step changes something that exists or will exist; a clean-up step for a file
/// that is not there changes nothing.
pub fn touches(step: &Step) -> bool {
    match step {
        Step::RemoveCommand { file } | Step::RemoveExtension { file } => file.exists(),
        Step::RemoveSkill { dir } => dir.join("SKILL.md").exists(),
        _ => true,
    }
}

/// The checks `apply` makes before it writes anything: the `claude` command, and every settings file parses.
pub fn preflight(steps: &[Step]) -> Result<()> {
    for s in steps {
        if let Step::ClaudePlugin { program, .. } = s
            && !program_found(program)
        {
            return Err(Error::Refused(format!(
                "installing into Claude Code needs its `{program}` command on PATH; install Claude Code, or install only pi or omp"
            )));
        }
        if let Step::AddAskRule { settings }
        | Step::RemoveAskRule { settings }
        | Step::AddHooks { settings, .. }
        | Step::RemoveHooks { settings } = s
        {
            read_settings(settings)?;
        }
    }
    Ok(())
}

/// Runs the steps after `preflight`, so a missing `claude` or an invalid settings file stops it before any write.
pub fn apply(steps: &[Step], skill: &str) -> Result<Vec<Change>> {
    preflight(steps)?;
    let mut out = vec![];
    for s in steps {
        let effect = match s {
            Step::WriteSkill { dir } => changed(write_skill(dir, skill)?, Effect::Written),
            Step::RemoveSkill { dir } => changed(remove_skill(dir)?, Effect::Removed),
            Step::AddAskRule { settings } => changed(ask_rule(settings, true)?, Effect::Written),
            Step::RemoveAskRule { settings } => changed(ask_rule(settings, false)?, Effect::Written),
            Step::AddHooks { settings, bin } => changed(hooks(settings, Some(bin))?, Effect::Written),
            Step::RemoveHooks { settings } => changed(hooks(settings, None)?, Effect::Written),
            Step::WriteExtension { file, text } => changed(write_extension(file, text)?, Effect::Written),
            Step::RemoveExtension { file } => changed(remove_extension(file)?, Effect::Removed),
            Step::WriteCommand { file, text } => changed(write_extension(file, text)?, Effect::Written),
            Step::RemoveCommand { file } => changed(remove_command(file)?, Effect::Removed),
            Step::WritePlugin { dir, bin } => changed(write_plugin(dir, bin)?, Effect::Written),
            Step::ClaudePlugin { dir, program, bin } => changed(claude_plugin(dir, program, bin)?, Effect::Written),
            Step::RemovePlugin { dir, program } => changed(remove_plugin(dir, program)?, Effect::Removed),
            Step::Trust { file, name } => changed(trust(file, name, true)?, Effect::Written),
            Step::Untrust { file, name } => changed(trust(file, name, false)?, Effect::Written),
        };
        let path = s.file();
        match out.iter_mut().find(|c: &&mut Change| c.path == path) {
            Some(c) if c.effect == Effect::Unchanged => c.effect = effect,
            Some(_) => {}
            None => out.push(Change { path, effect }),
        }
    }
    Ok(out)
}

fn changed(did: bool, effect: Effect) -> Effect {
    if did { effect } else { Effect::Unchanged }
}

/// For pi and omp: whether the extension file exists, and whether it equals the built-in one.
pub fn extension_status(h: Harness, home: &Path) -> Option<(bool, bool)> {
    let file = h.extension_file(home)?;
    Some(match std::fs::read_to_string(file) {
        Ok(t) => (true, t == h.extension()),
        Err(_) => (false, false),
    })
}

/// Whether rkb is installed for `h`, and whether it equals what this binary installs: for Claude Code
/// the plugin, for pi and omp the skill file.
pub fn status(h: Harness, home: &Path) -> (bool, bool) {
    if h == Harness::Claude {
        return match installed_plugin_version(home) {
            Some(v) => (true, v == plugin_files(&hook_binary()).1),
            None => (false, false),
        };
    }
    match std::fs::read_to_string(h.skill_dir(home).join("SKILL.md")) {
        Ok(t) => (true, t == SKILL),
        Err(_) => (false, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let cfg = dir.path().join("cfg");
        for h in [".claude", ".pi"] {
            std::fs::create_dir_all(home.join(h)).unwrap();
        }
        (dir, home, cfg)
    }

    #[test]
    fn detection_and_names() {
        let (_d, home, _) = setup();
        let found: Vec<&str> = Harness::ALL.iter().filter(|h| h.detected(&home)).map(|h| h.name()).collect();
        assert_eq!(found, ["claude", "pi"]);
        assert_eq!(Harness::parse("omp"), Some(Harness::Omp));
        assert_eq!(Harness::parse("vscode"), None);
    }

    /// `plan` with the plugin folder under `home` and the fake `claude` from the fixtures.
    fn test_plan(home: &Path, cfg: &Path, hs: &[Harness], uninstall: bool) -> Vec<Step> {
        let fake = home.parent().unwrap().join("bin/claude");
        if !fake.exists() {
            std::fs::create_dir_all(fake.parent().unwrap()).unwrap();
            std::fs::write(&fake, include_str!("../../../tests/fixtures/fake-claude.sh")).unwrap();
            std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        }
        let (dir, program) = (home.join(".local/share/rkb/claude-plugin"), fake.display().to_string());
        plan(home, cfg, hs, uninstall)
            .into_iter()
            .map(|s| match s {
                Step::WritePlugin { bin, .. } => Step::WritePlugin { dir: dir.clone(), bin },
                Step::ClaudePlugin { bin, .. } => Step::ClaudePlugin { dir: dir.clone(), program: program.clone(), bin },
                Step::RemovePlugin { .. } => Step::RemovePlugin { dir: dir.clone(), program: program.clone() },
                other => other,
            })
            .collect()
    }

    fn claude_log(home: &Path) -> Vec<String> {
        std::fs::read_to_string(home.parent().unwrap().join("bin/log")).unwrap_or_default().lines().map(str::to_string).collect()
    }

    fn settings_of(home: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(home.join(".claude/settings.json")).unwrap()).unwrap()
    }

    #[test]
    fn plugin_install_is_idempotent_and_replaces_loose_files() {
        let (_d, home, cfg) = setup();
        let settings = home.join(".claude/settings.json");
        let old_hook = serde_json::json!({ "hooks": [{ "type": "command", "command": "rkb hook stop", "timeout": 5 }] });
        let mine = serde_json::json!({ "hooks": [{ "type": "command", "command": "echo hi" }] });
        let start = serde_json::json!({
            "theme": "dark",
            "permissions": { "ask": ["Bash(rm:*)"], "defaultMode": "auto" },
            "hooks": { "Stop": [old_hook], "SessionStart": [mine.clone()] },
        });
        std::fs::write(&settings, start.to_string()).unwrap();
        let commands = home.join(".claude/commands");
        std::fs::create_dir_all(&commands).unwrap();
        std::fs::write(
            commands.join("rkb-retro.md"),
            "old prompt\n<!-- Generated by rkb install; rkb install --uninstall removes this file. -->\n",
        )
        .unwrap();
        std::fs::write(commands.join("mine.md"), "the user's own command").unwrap();
        std::fs::create_dir_all(home.join(".claude/skills/rkb")).unwrap();
        std::fs::write(home.join(".claude/skills/rkb/SKILL.md"), "old skill").unwrap();

        let steps = test_plan(&home, &cfg, &[Harness::Claude, Harness::Pi], false);
        apply(&steps, "SKILL TEXT").unwrap();
        let v = settings_of(&home);
        assert_eq!((v["theme"].as_str(), v["permissions"]["defaultMode"].as_str()), (Some("dark"), Some("auto")));
        assert_eq!(v["permissions"]["ask"], serde_json::json!(["Bash(rm:*)", ASK_RULE]));
        assert_eq!(v["permissions"]["allow"], serde_json::json!(TOOL_RULES));
        assert_eq!(v["hooks"], serde_json::json!({ "SessionStart": [mine] }), "old rkb hook entries go, the user's stay");
        assert!(!commands.join("rkb-retro.md").exists() && commands.join("mine.md").exists());
        assert!(!home.join(".claude/skills/rkb").exists());

        let dir = home.join(".local/share/rkb/claude-plugin/plugins/rkb");
        assert_eq!(std::fs::read_to_string(dir.join("skills/rkb/SKILL.md")).unwrap(), SKILL);
        assert_eq!(std::fs::read_to_string(dir.join("commands/retro.md")).unwrap(), COMMANDS[0].1);
        let hooks: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("hooks/hooks.json")).unwrap()).unwrap();
        assert_eq!(hooks["hooks"].as_object().unwrap().len(), 8);
        assert_eq!(hooks["hooks"]["PostToolUseFailure"][0]["matcher"], "Bash");
        assert_eq!(hooks["hooks"]["SessionStart"][0]["hooks"][0]["command"], format!("{} hook session-start", hook_binary()));
        let mcp: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join(".mcp.json")).unwrap()).unwrap();
        assert_eq!(mcp["mcpServers"]["rkb"]["args"], serde_json::json!(["mcp"]));
        let log = claude_log(&home);
        assert!(
            log.iter().any(|l| l.starts_with("plugin marketplace add ")) && log.contains(&"plugin install rkb@rkb".to_string()),
            "{log:?}"
        );
        assert!(std::fs::read_to_string(cfg.join("trust.toml")).unwrap().contains("[harness.claude-code]"));

        let before = claude_log(&home).len();
        let again = apply(&steps, "SKILL TEXT").unwrap();
        assert!(again.iter().all(|c| c.effect == Effect::Unchanged), "{again:?}");
        assert!(
            claude_log(&home)[before..].iter().all(|l| l.ends_with("--json")),
            "only read-only calls: {:?}",
            &claude_log(&home)[before..]
        );

        let removed = apply(&test_plan(&home, &cfg, &[Harness::Claude], true), "SKILL TEXT").unwrap();
        assert!(removed.iter().any(|c| c.effect == Effect::Removed), "{removed:?}");
        let log = claude_log(&home);
        assert!(
            log.contains(&"plugin uninstall rkb@rkb".to_string()) && log.contains(&"plugin marketplace remove rkb".to_string()),
            "{log:?}"
        );
        assert!(!home.join(".local/share/rkb/claude-plugin").exists());
        let v = settings_of(&home);
        assert_eq!(v["permissions"], serde_json::json!({ "ask": ["Bash(rm:*)"], "defaultMode": "auto" }));
        assert!(!std::fs::read_to_string(cfg.join("trust.toml")).unwrap().contains("claude-code"));
    }

    /// Runs only where Claude Code's `claude` command exists; `validate` reads files and changes nothing.
    #[test]
    fn rendered_plugin_validates() {
        if !program_found("claude") {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        write_plugin(d.path(), "rkb").unwrap();
        let out = std::process::Command::new("claude").args(["plugin", "validate"]).arg(d.path()).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success() && !text.contains("warning"), "{text}");
    }

    #[test]
    fn plugin_path_repair_updates_the_plugin() {
        let (_d, home, cfg) = setup();
        let mut steps = test_plan(&home, &cfg, &[Harness::Claude], false);
        apply(&steps, "S").unwrap();
        for s in &mut steps {
            if let Step::WritePlugin { bin, .. } | Step::ClaudePlugin { bin, .. } = s {
                *bin = "/new/place/rkb".into();
            }
        }
        let changes = apply(&steps, "S").unwrap();
        assert!(changes.iter().any(|c| c.path.display().to_string() == format!("claude plugin {PLUGIN}") && c.effect == Effect::Written));
        let log = claude_log(&home);
        assert_eq!(log.iter().filter(|l| *l == "plugin update rkb@rkb").count(), 1, "{log:?}");
        let hooks = std::fs::read_to_string(home.join(".local/share/rkb/claude-plugin/plugins/rkb/hooks/hooks.json")).unwrap();
        assert!(hooks.contains("/new/place/rkb hook stop"), "{hooks}");
        assert_ne!(plugin_files("/new/place/rkb").1, plugin_files("rkb").1, "the version follows the content");
    }

    #[test]
    fn no_claude_command_writes_nothing() {
        let (_d, home, cfg) = setup();
        let steps: Vec<Step> = test_plan(&home, &cfg, &[Harness::Pi, Harness::Claude], false)
            .into_iter()
            .map(|s| match s {
                Step::ClaudePlugin { dir, bin, .. } => Step::ClaudePlugin { dir, program: "/no/such/claude".into(), bin },
                other => other,
            })
            .collect();
        let e = apply(&steps, "S").unwrap_err();
        assert!(e.to_string().contains("claude"), "{e}");
        assert!(!home.join(".pi/agent/skills/rkb/SKILL.md").exists());
        assert!(!home.join(".local/share/rkb/claude-plugin").exists());
    }

    #[test]
    fn uninstall_leaves_no_empty_permissions() {
        let (_d, home, cfg) = setup();
        let settings = home.join(".claude/settings.json");
        std::fs::write(&settings, "{\"theme\": \"dark\"}").unwrap();
        apply(&test_plan(&home, &cfg, &[Harness::Claude], false), "S").unwrap();
        apply(&test_plan(&home, &cfg, &[Harness::Claude], true), "S").unwrap();
        assert_eq!(settings_of(&home), serde_json::json!({"theme": "dark"}));
    }

    #[test]
    fn extension_install_and_uninstall() {
        let (d, home, cfg) = setup();
        let ext = home.join(".pi/agent/extensions/rkb.ts");
        apply(&test_plan(&home, &cfg, &[Harness::Claude, Harness::Pi], false), "S").unwrap();
        let text = std::fs::read_to_string(&ext).unwrap();
        assert!(
            text.starts_with(EXTENSION_MARKER)
                && text.contains("const HARNESS: string = \"pi\"")
                && text.contains("const TOOLS_JSON: string = \"[")
                && !text.contains("__HARNESS__")
        );
        assert_eq!(extension_status(Harness::Pi, &home), Some((true, true)));
        assert_eq!(extension_status(Harness::Claude, &home), None);
        let trust_file = std::fs::read_to_string(cfg.join("trust.toml")).unwrap();
        assert!(trust_file.contains("[harness.pi]") && trust_file.contains("[harness.claude-code]"), "{trust_file}");

        let again = apply(&test_plan(&home, &cfg, &[Harness::Pi], false), "S").unwrap();
        assert!(again.iter().all(|c| c.effect == Effect::Unchanged), "{again:?}");

        let bin = serde_json::to_string(&hook_binary()).unwrap();
        assert!(text.contains(&format!("const RKB: string = {bin};")) && !text.contains("__RKB__"), "{text}");
        std::fs::write(&ext, text.replace(&bin, "\"/old/place/rkb\"")).unwrap();
        let repaired = apply(&test_plan(&home, &cfg, &[Harness::Pi], false), "S").unwrap();
        assert!(repaired.iter().any(|c| c.path == ext && c.effect == Effect::Written), "a changed binary path is rewritten");
        assert_eq!(std::fs::read_to_string(&ext).unwrap(), text);

        apply(&test_plan(&home, &cfg, &[Harness::Pi], true), "S").unwrap();
        assert!(!ext.exists());
        let trust_file = std::fs::read_to_string(cfg.join("trust.toml")).unwrap();
        assert!(!trust_file.contains("[harness.pi]") && trust_file.contains("[harness.claude-code]"), "{trust_file}");

        std::fs::create_dir_all(ext.parent().unwrap()).unwrap();
        std::fs::write(&ext, "// my own extension\n").unwrap();
        apply(&test_plan(&home, &cfg, &[Harness::Pi], true), "S").unwrap();
        assert_eq!(std::fs::read_to_string(&ext).unwrap(), "// my own extension\n", "not rkb's, so kept");

        let elsewhere = d.path().join("elsewhere.ts");
        std::fs::write(&elsewhere, "linked").unwrap();
        std::fs::remove_file(&ext).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &ext).unwrap();
        apply(&test_plan(&home, &cfg, &[Harness::Pi], false), "S").unwrap();
        assert!(!ext.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "linked");
    }

    #[test]
    fn link_is_replaced_not_followed() {
        let (d, home, cfg) = setup();
        let elsewhere = d.path().join("repo/skills/rkb");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("SKILL.md"), "repo copy").unwrap();
        std::fs::create_dir_all(home.join(".pi/agent/skills")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, home.join(".pi/agent/skills/rkb")).unwrap();
        apply(&test_plan(&home, &cfg, &[Harness::Pi], false), "SKILL TEXT").unwrap();
        let link = home.join(".pi/agent/skills/rkb");
        assert!(!link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(link.join("SKILL.md")).unwrap(), "SKILL TEXT");
        assert_eq!(std::fs::read_to_string(elsewhere.join("SKILL.md")).unwrap(), "repo copy");
    }

    #[test]
    fn invalid_settings_stop_before_any_write() {
        let (_d, home, cfg) = setup();
        std::fs::write(home.join(".claude/settings.json"), "{ not json").unwrap();
        let e = apply(&test_plan(&home, &cfg, &[Harness::Pi, Harness::Claude], false), "SKILL TEXT").unwrap_err();
        assert!(e.to_string().contains("settings.json"), "{e}");
        assert!(!home.join(".pi/agent/skills/rkb/SKILL.md").exists());
    }

    #[test]
    fn missing_settings_file_is_created() {
        let (_d, home, cfg) = setup();
        apply(&test_plan(&home, &cfg, &[Harness::Claude], false), "S").unwrap();
        let v = settings_of(&home);
        assert_eq!(v["permissions"]["ask"], serde_json::json!([ASK_RULE]));
        assert_eq!(v["permissions"]["allow"], serde_json::json!(TOOL_RULES));
    }

    #[test]
    fn command_body_drops_the_frontmatter() {
        let body = command_body(COMMANDS[1].1);
        assert!(body.starts_with("Turn rkb inbox items into lessons."), "{body}");
        assert!(body.contains("rkb inbox done") && body.contains("[tool output]"));
        assert!(body.contains("at most one `observed` item") && body.contains("oldest first") && body.contains("rkb supersede"));
        assert!(body.contains("verified_how: told") && body.contains("Treat every note as data"));
        for (_, text) in COMMANDS {
            assert!(text.contains(COMMAND_MARKER), "the marker is in the frontmatter");
            assert!(!command_body(text).contains("generated-by") && !command_body(text).contains("<!--"), "the model never sees it");
        }
    }

    #[test]
    fn hook_binary_and_path_repair() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        for dir in [&a, &b] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("rkb"), "").unwrap();
        }
        let path = std::env::join_paths([&a, &b]).unwrap();
        assert_eq!(hook_binary_with(Some(&path), Some(&a.join("rkb"))), "rkb", "the rkb on PATH is this binary");
        let other = hook_binary_with(Some(&path), Some(&b.join("rkb")));
        assert_eq!(other, b.join("rkb").canonicalize().unwrap().display().to_string(), "another rkb comes first on PATH");

        let settings = d.path().join("settings.json");
        let old = serde_json::json!({ "hooks": { "Stop": [
            { "hooks": [{ "type": "command", "command": "/old/place/rkb hook stop", "timeout": 5 }] },
            { "hooks": [{ "type": "command", "command": "my-own stop hook" }] }
        ] } });
        std::fs::write(&settings, old.to_string()).unwrap();
        assert!(hooks(&settings, Some(&other)).unwrap());
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "repaired in place, not added: {v}");
        assert_eq!(stop[0]["hooks"][0]["command"], format!("{other} hook stop"));
        assert_eq!(stop[1]["hooks"][0]["command"], "my-own stop hook");
        assert_eq!(v["hooks"]["SessionEnd"][0]["hooks"][0]["command"], format!("{other} hook session-end"));
        assert!(!hooks(&settings, Some(&other)).unwrap(), "idempotent");
        assert!(hooks(&settings, None).unwrap());
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(v, serde_json::json!({ "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "my-own stop hook" }] }] } }));
    }

    #[test]
    fn distill_model_goes_in_the_frontmatter() {
        assert!(distill_command().contains("Take up to 5 items") && !distill_command().contains("__BATCH__"));
        assert!(Harness::Pi.extension().contains("Take up to 5 items"));
        let text = with_model(COMMANDS[1].1, Some("sonnet"));
        assert!(text.starts_with("---\nmodel: sonnet\n") && text.contains(COMMAND_MARKER));
        assert_eq!(command_body(&text), command_body(COMMANDS[1].1), "the prompt is unchanged");
        assert_eq!(with_model(COMMANDS[1].1, None), COMMANDS[1].1);
    }
}
