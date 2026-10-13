//! `rkb maintain`: one upkeep pass under the lock it shares with `rkb promote`.

use std::path::Path;
use std::time::Duration;

use rkb_core::maintain;
use rkb_core::{config, paths, promote, request, triage};
use serde_json::json;

use crate::inbox;
use crate::output::{CliError, ErrorCode, Output};
use crate::writes::Env;

/// Runs one pass: pull proposals, run the rule triage, expire transcripts, fade unused lessons and
/// promote within the daily model cap. Every step reuses the command that already does it.
pub fn run(env: &Env, harness: &str) -> Result<Output, CliError> {
    let config_dir = paths::config_dir();
    std::fs::create_dir_all(&env.state)
        .map_err(|e| CliError::new(ErrorCode::Io, format!("cannot create {}: {e}", env.state.display()), "check the state directory"))?;
    let _guard = match rkb_core::lock::acquire(&maintain::lock_path(&env.state), 3600, Duration::ZERO) {
        Ok(guard) => guard,
        Err(rkb_core::Error::Locked(_)) => return Ok(busy()),
        Err(e) => return Err(CliError::new(ErrorCode::Io, e.to_string(), "check the state directory")),
    };
    let (pulled, pull_error) = match pull(env, &config_dir) {
        Ok(n) => (n, None),
        Err(e) => (0, Some(e.message)),
    };
    let triaged = rule_triage(env, &config_dir)?;
    let expired = maintain::expire(&env.state, &config_dir);
    let faded =
        maintain::fade(&env.ctx(), &config_dir).map_err(|e| CliError::new(ErrorCode::Io, e.to_string(), "check the knowledge base"))?;
    let (promoted, skip) = promote_step(env, harness, &config_dir)?;
    maintain::record(
        &env.state,
        maintain::Last {
            time: request::now(),
            pulled,
            triaged,
            expired,
            faded,
            promoted,
            pull_error: pull_error.clone(),
            skip: skip.clone(),
        },
    );
    let mut human = format!("rkb maintain: {pulled} pulled, {triaged} triaged, {expired} expired, {faded} faded, {promoted} promoted");
    if let Some(reason) = &pull_error {
        human.push_str(&format!("\nthe pull failed: {reason}"));
    }
    if let Some(reason) = &skip {
        human.push_str(&format!("\nskipped the model step: {reason}"));
    }
    Ok(Output {
        data: json!({ "pulled": pulled, "triaged": triaged, "expired": expired, "faded": faded, "promoted": promoted, "pull_error": pull_error, "skip": skip }),
        human,
        exit: 0,
        raw: false,
    })
}

/// A pass that found the upkeep lock held: exit 0, no step run.
fn busy() -> Output {
    let human = format!("{}; no step ran", maintain::BUSY);
    Output {
        data: json!({ "running": true, "reason": maintain::BUSY, "pulled": 0, "triaged": 0, "expired": 0, "faded": 0, "promoted": 0 }),
        human,
        exit: 0,
        raw: false,
    }
}

/// Pulls the `[proposals] ntfy` topic when it is set, and counts the items added. A failed pull does
/// not stop the pass: a machine that just woke may have no network yet.
fn pull(env: &Env, config_dir: &Path) -> Result<usize, CliError> {
    let Some(url) = config::proposals_ntfy(config_dir)
        .map_err(|m| CliError::new(ErrorCode::Refused, m, "fix [proposals] ntfy in the machine config.toml"))?
    else {
        return Ok(0);
    };
    let out = inbox::pull(env, &url)?;
    Ok(out.data["added"].as_u64().unwrap_or(0) as usize)
}

/// The cheap rule triage: no model, so a candidate the rules cannot decide stays unsure.
fn rule_triage(env: &Env, config_dir: &Path) -> Result<usize, CliError> {
    let stages = triage::Stages { thresholds: triage::Thresholds::load(config_dir), novelty: None, scorer: None };
    let done = triage::triage(&env.state, &stages).map_err(|e| CliError::new(ErrorCode::Io, e.to_string(), "check the state directory"))?;
    Ok(done.len())
}

/// The model step, within `min(remaining, max_per_pass)` calls. A missing approved chain, a failed
/// harness program or a reached daily cap skips it with a named reason, and the pass still exits 0.
fn promote_step(env: &Env, harness: &str, config_dir: &Path) -> Result<(usize, Option<String>), CliError> {
    let max_day = maintain::max_calls_per_day(config_dir);
    let remaining = maintain::remaining(&env.state, max_day);
    if remaining == 0 {
        return Ok((0, Some("the daily cap is reached".to_string())));
    }
    let cap = remaining.min(promote::max_per_pass(config_dir) as u64) as usize;
    let out = crate::promote::run_locked(env, Some(cap), harness, true, || maintain::take_call(&env.state, max_day))?;
    let promoted = out.data["promoted"].as_u64().unwrap_or(0) as usize;
    let skip = out.data["skip"].as_str().or_else(|| out.data["reason"].as_str()).map(str::to_string);
    Ok((promoted, skip))
}
