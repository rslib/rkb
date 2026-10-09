//! The approved model chain and the one model call rkb makes: triage's unsure band and promotion.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::approval::{self, Approval};
use crate::config::ObserverConfig;
use crate::error::{Error, Result, io};
use crate::request::{self, Action, Choice, Decision, Request};
use crate::write::{Ctx, Outcome};
use crate::{lock, paths, script};

/// Set in the model's environment; every rkb hook does nothing while it is set.
pub const ENV: &str = "RKB_OBSERVER";
/// Days of outcome lines kept in `observer.jsonl`.
/// Output of one model run read at most.
const MAX_OUTPUT: u64 = 1 << 20;

/// Whether this process runs inside an observer command.
pub fn active() -> bool {
    std::env::var_os(ENV).is_some()
}

/// Approvals of the observer are for this machine, whatever system or project matched.
pub fn system() -> String {
    format!("host:{}", lock::hostname())
}

/// The `[observer]` settings of this machine's `config.toml`.
pub fn load(config_dir: &Path) -> std::result::Result<ObserverConfig, String> {
    let (_, table) = crate::config::machine(config_dir)?;
    ObserverConfig::from_table(&table)
}

pub fn approved(config_dir: &Path, cfg: &ObserverConfig) -> bool {
    !cfg.chains.is_empty() && approval::is_approved(config_dir, &script::hash(&cfg.summary()), &system())
}

/// The settings, when `harness` has a chain and the chains are approved here.
pub fn ready(config_dir: &Path, harness: &str) -> Option<ObserverConfig> {
    load(config_dir).ok().filter(|c| !c.chain(harness).is_empty() && approved(config_dir, c))
}

fn no_chain() -> Error {
    Error::NotReady {
        reason: "no observer model chain is set".into(),
        fix: format!(
            "add `claude-code`, `pi` or `omp` with a list of models under [observer] in {}",
            paths::config_dir().join("config.toml").display()
        ),
    }
}

/// A request to approve the observer chains, or `None` when they are already approved here.
pub fn approve_request(ctx: &Ctx) -> Result<Option<Request>> {
    let dir = paths::config_dir();
    let cfg = load(&dir).map_err(Error::Refused)?;
    if cfg.chains.is_empty() {
        return Err(no_chain());
    }
    let summary = cfg.summary();
    let sha = script::hash(&summary);
    let system = system();
    if approval::is_approved(&dir, &sha, &system) {
        return Ok(None);
    }
    let question = format!(
        "Approve the observer on {system}? After each session with {} or more prompts ends, rkb runs that harness's own agent in print mode, with no tools, in the background, and sends it the session's text: your prompts, the agent's text, tool calls and error lines. It tries the models in order until one answers:\n{summary}\n(`session` is the model the session used.) sha256 {sha}",
        cfg.min_prompts
    );
    let req = Request {
        id: request::new_id(),
        created: request::now(),
        action: Action::ApproveObserver { sha256: sha, system },
        approved: vec![],
        question,
        choices: vec![
            Choice { text: "approve observer".into(), decision: Some(Decision::Approve) },
            Choice { text: "cancel".into(), decision: None },
        ],
        session: None,
    };
    request::save(&ctx.state.join("requests"), &req)?;
    Ok(Some(req))
}

/// Records a confirmed approval, when the chains are still the ones the request showed.
pub fn approve(ctx: &Ctx, sha256: &str, system: &str) -> Result<Outcome> {
    let dir = paths::config_dir();
    let cfg = load(&dir).map_err(Error::Refused)?;
    if script::hash(&cfg.summary()) != sha256 {
        return Err(Error::Refused("the observer chains changed since the request; run `rkb approve observer` again".into()));
    }
    approval::add(
        &dir,
        Approval {
            sha256: sha256.into(),
            system: system.into(),
            lesson: "observer".into(),
            kind: "observer".into(),
            date: ctx.today.to_string(),
        },
    )?;
    Ok(Outcome::Info(format!("Approved the observer on {system}")))
}

/// The program and arguments that run `harness`'s agent in print mode with `model` (its default model
/// when `None`), reading the prompt on stdin, with no tools, extensions, hooks or saved session.
/// `RKB_CLAUDE`, `RKB_PI` and `RKB_OMP` override the program.
pub fn command(harness: &str, model: Option<&str>) -> (String, Vec<String>) {
    let (var, name, flags): (&str, &str, &[&str]) = match harness {
        "claude-code" => (
            "RKB_CLAUDE",
            "claude",
            &[
                "--tools",
                "",
                "--output-format",
                "json",
                "--settings",
                r#"{"disableAllHooks":true}"#,
                "--strict-mcp-config",
                "--no-session-persistence",
                "--disable-slash-commands",
            ],
        ),
        "omp" => ("RKB_OMP", "omp", &["--no-session", "--no-tools", "--no-extensions", "--no-lsp"]),
        _ => ("RKB_PI", "pi", &["--no-session", "--no-tools", "--no-extensions"]),
    };
    let mut args = vec!["-p".to_string()];
    if let Some(m) = model {
        args.extend(["--model".to_string(), m.to_string()]);
    }
    args.extend(flags.iter().map(|f| f.to_string()));
    (std::env::var(var).unwrap_or_else(|_| name.into()), args)
}

/// Notes kept from a command's output, and how many other non-empty lines were dropped.
#[derive(Debug, Clone, PartialEq)]
pub enum Ran {
    Ok { text: String, usage: Option<(u64, u64)> },
    Failed(String),
    Timeout,
}

impl Ran {
    /// The model's answer text, when the run succeeded.
    pub fn text(&self) -> Option<&str> {
        match self {
            Ran::Ok { text, .. } => Some(text),
            _ => None,
        }
    }
}

/// A harness that prints JSON gives the answer text and the call's real usage; any other output is the
/// answer text itself, with no usage. Claude Code prints `{"result": ..., "usage": {...}}`.
fn parse_output(text: String) -> Ran {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim())
        && let Some(result) = v["result"].as_str()
    {
        let usage = v["usage"].as_object().map(|u| {
            let n = |k: &str| u.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
            (n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens"), n("output_tokens"))
        });
        return Ran::Ok { text: result.to_string(), usage };
    }
    Ran::Ok { text, usage: None }
}

/// Runs `program` with `input` on stdin and `RKB_OBSERVER=1`, in `scratch` and its own process group;
/// kills the group after `timeout`.
pub fn run(program: &str, args: &[String], input: &str, timeout: Duration, scratch: &Path) -> Result<Ran> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::process::CommandExt;
    std::fs::create_dir_all(scratch).map_err(io(scratch))?;
    let stem = format!("observe-{}", std::process::id());
    let input_path = scratch.join(format!("{stem}.in"));
    let output_path = scratch.join(format!("{stem}.out"));
    let open = |p: &Path| std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(p).map_err(io(p));
    std::io::Write::write_all(&mut open(&input_path)?, input.as_bytes()).map_err(io(&input_path))?;
    let out_file = open(&output_path)?;
    let stdin = std::fs::File::open(&input_path).map_err(io(&input_path))?;
    let mut c = Command::new(program);
    c.args(args).env(ENV, "1").current_dir(scratch).stdin(stdin).stdout(out_file).stderr(Stdio::null()).process_group(0);
    let ran = match c.spawn() {
        Err(e) => Ran::Failed(format!("did not start: {e}")),
        Ok(mut child) => {
            let start = Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) if status.success() => {
                        let mut text = String::new();
                        let _ = std::fs::File::open(&output_path).map(|f| f.take(MAX_OUTPUT).read_to_string(&mut text));
                        break parse_output(text);
                    }
                    Ok(Some(status)) => break Ran::Failed(format!("exit {}", status.code().map_or("signal".into(), |c| c.to_string()))),
                    Ok(None) if start.elapsed() >= timeout => {
                        let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", child.id())]).stderr(Stdio::null()).status();
                        let _ = child.kill();
                        let _ = child.wait();
                        break Ran::Timeout;
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                    Err(e) => break Ran::Failed(e.to_string()),
                }
            }
        }
    };
    let _ = std::fs::remove_file(&input_path);
    let _ = std::fs::remove_file(&output_path);
    Ok(ran)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_json_answer_gives_the_text_and_the_real_usage() {
        let out = r#"{"result":"{\"action\":\"noop\"}","usage":{"input_tokens":10,"cache_creation_input_tokens":100,"cache_read_input_tokens":1000,"output_tokens":7}}"#;
        match parse_output(out.to_string()) {
            Ran::Ok { text, usage } => {
                assert_eq!(text, "{\"action\":\"noop\"}");
                assert_eq!(usage, Some((1110, 7)), "cache reads are input tokens too");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn plain_text_stays_the_text_with_no_usage() {
        match parse_output("just text".to_string()) {
            Ran::Ok { text, usage } => {
                assert_eq!(text, "just text");
                assert_eq!(usage, None);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_command_asks_claude_for_json() {
        let (_, args) = command("claude-code", Some("haiku"));
        assert!(args.windows(2).any(|w| w == ["--output-format", "json"]), "{args:?}");
    }
}
