//! Lesson model, validation and knowledge base operations for rkb.
//! This crate never writes to the terminal; the `rkb` binary presents its results.

pub mod approval;
pub mod body;
pub mod conditions;
pub mod config;
pub mod distill;
pub mod doctor;
pub mod error;
pub mod eval;
pub mod facts;
pub mod git;
pub mod graph;
pub mod hooks;
pub mod import;
pub mod init;
pub mod install;
pub mod kb;
pub mod leak;
pub mod lesson;
pub mod lint;
pub mod list;
pub mod lock;
pub mod matching;
pub mod paths;
pub mod request;
pub mod rerank;
pub mod review;
pub mod script;
pub mod search;
pub mod state;
pub mod sync;
pub mod text;
pub mod tools;
pub mod usage;
pub mod verify;
pub mod write;
pub mod yaml;

pub use error::{Error, Result};
