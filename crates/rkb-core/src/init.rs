use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::config::DEFAULT_KB_TOML;
use crate::error::{Error, Result, io};
use crate::git;
use crate::kb::SCOPES;

pub const PRE_COMMIT_HOOK: &str = r#"#!/bin/sh
command -v rkb >/dev/null || { echo "rkb not on PATH; the pre-commit lint cannot run" >&2; exit 1; }
RKB_HOME="$(git rev-parse --show-toplevel)"
export RKB_HOME
exec rkb lint --staged
"#;

/// Creates a new knowledge base at `root`. Refuses a path that exists and is not empty.
fn refuse_non_empty(root: &Path) -> Result<()> {
    if root.exists() && std::fs::read_dir(root).map_err(io(root))?.next().is_some() {
        return Err(Error::Exists(root.to_path_buf()));
    }
    Ok(())
}

/// The per-clone setup that git does not copy: pushes from other machines, and the pre-commit hook.
fn setup_clone(root: &Path) -> Result<()> {
    git::run(root, &["config", "receive.denyCurrentBranch", "updateInstead"])?;
    let git_dir = String::from_utf8_lossy(&git::run(root, &["rev-parse", "--git-path", "hooks"])?).trim().to_string();
    let hooks = root.join(git_dir);
    std::fs::create_dir_all(&hooks).map_err(io(&hooks))?;
    let hook = hooks.join("pre-commit");
    std::fs::write(&hook, PRE_COMMIT_HOOK).map_err(io(&hook))?;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).map_err(io(&hook))
}

/// Clones an existing knowledge base into `root` and sets up this clone. Makes no commit.
pub fn clone(root: &Path, url: &str) -> Result<()> {
    refuse_non_empty(root)?;
    let parent = root.ancestors().skip(1).find(|p| p.is_dir()).unwrap_or(Path::new("/"));
    git::run(parent, &["clone", "-q", url, &root.display().to_string()])?;
    setup_clone(root)?;
    if !root.join("kb.toml").is_file() {
        return Err(Error::Refused(format!(
            "cloned {url} into {}, but it has no kb.toml, so it is not an rkb knowledge base; check the URL, then remove the folder",
            root.display()
        )));
    }
    Ok(())
}

pub fn init(root: &Path) -> Result<()> {
    refuse_non_empty(root)?;
    let probe_dir = root.ancestors().find(|p| p.is_dir()).unwrap_or(Path::new("/"));
    git::check_identity(probe_dir)?;

    std::fs::create_dir_all(root).map_err(io(root))?;
    git::run(root, &["init", "-q"])?;
    setup_clone(root)?;

    let write = |rel: &str, text: &str| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).map_err(io(&p))?;
        std::fs::write(&p, text).map_err(io(&p))
    };
    write("kb.toml", DEFAULT_KB_TOML)?;
    for scope in SCOPES {
        write(&format!("{scope}/.gitkeep"), "")?;
    }

    git::run(root, &["add", "-A"])?;
    git::commit(root, "init: create knowledge base")
}
