//! Open Terminal's requests and answers, as its version 0.14.0 has them
//! (read from its sources; 0.13.0 answers alike). The API is 0.x: what is
//! relied on is here and pinned by the stand-in's tests, and an answer that
//! does not fit is a failed request naming what did not fit, never a panic.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `POST /execute`.
#[derive(Debug, Serialize)]
pub(crate) struct ExecRequest<'a> {
    pub(crate) command: &'a str,
    /// Absolute, inside the instance.
    pub(crate) cwd: &'a str,
    /// Merged into the instance's own environment.
    pub(crate) env: BTreeMap<&'static str, String>,
}

/// What `POST /execute` and `GET /execute/{id}/status` answer. `log_path`,
/// a path inside the instance, and `command` are not read.
#[derive(Debug, Deserialize)]
pub(crate) struct Process {
    pub(crate) id: String,
    /// `running`, `done` or `killed`.
    pub(crate) status: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) output: Vec<OutputEntry>,
    /// Whether entries before the offset asked for, or between it and what
    /// is given, were dropped.
    pub(crate) truncated: bool,
    /// Counts entries, not bytes.
    pub(crate) next_offset: u64,
}

/// A piece of what a process printed: `output` from a pseudo-terminal,
/// `stdout` or `stderr` from pipes.
#[derive(Debug, Deserialize)]
pub(crate) struct OutputEntry {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) data: String,
}

/// What `GET /files/list` answers; `dir`, a path inside the instance, and
/// `writable` are not read.
#[derive(Debug, Deserialize)]
pub(crate) struct Listing {
    pub(crate) entries: Vec<ListedEntry>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ListedEntry {
    pub(crate) name: String,
    /// `directory` or `file`, links followed.
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) size: u64,
    /// Seconds since the epoch.
    pub(crate) modified: Option<f64>,
}

/// `POST /files/mkdir`.
#[derive(Debug, Serialize)]
pub(crate) struct PathRequest<'a> {
    pub(crate) path: &'a str,
}

/// `POST /files/move`.
#[derive(Debug, Serialize)]
pub(crate) struct MoveRequest<'a> {
    pub(crate) source: &'a str,
    pub(crate) destination: &'a str,
}
