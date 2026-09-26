use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0} is not an rkb knowledge base")]
    NotAKb(PathBuf),
    #[error("{0} exists and is not empty")]
    Exists(PathBuf),
    #[error("git has no user.name or user.email configured")]
    GitIdentity,
    #[error("`git {args}` failed: {stderr}")]
    Git { args: String, stderr: String },
    #[error("no lesson has id {0}")]
    NotFound(String),
    #[error("no folder {0} in the knowledge base")]
    NoFolder(String),
    #[error("{0}")]
    Refused(String),
    #[error("invalid pattern: {0}")]
    BadPattern(String),
    #[error("the lesson has {} lint error(s)", .0.len())]
    Invalid(Vec<crate::lint::Finding>),
    #[error("the knowledge base is locked by {0}")]
    Locked(String),
    #[error("the lesson changed since you read it; its hash is now {0}")]
    Conflict(String),
    #[error("the rebase stopped on a conflict in {}", .files.join(", "))]
    RebaseConflict { root: PathBuf, files: Vec<String> },
    #[error("{reason}")]
    NotReady { reason: String, fix: String },
    #[error("no git remote named {name}")]
    NoRemote { name: String, remotes: Vec<String> },
    #[error("no lesson files (*.md) in {0}")]
    NothingToImport(PathBuf),
    #[error("request {0} has expired or does not exist")]
    Expired(String),
    #[error("`{choice}` is not an option of this request")]
    BadChoice { choice: String, options: Vec<String> },
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> Error {
    let path = path.into();
    move |source| Error::Io { path, source }
}
