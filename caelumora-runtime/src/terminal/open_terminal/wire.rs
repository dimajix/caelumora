//! Open Terminal's requests and answers, as its version 0.14.0 has them
//! (read from its sources; 0.13.0 answers alike). The API is 0.x: what is
//! relied on is here and pinned by the stand-in's tests, and an answer that
//! does not fit is a failed request naming what did not fit, never a panic.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `POST /execute`.
#[derive(Debug, Serialize)]
pub(super) struct ExecRequest<'a> {
    pub(super) command: &'a str,
    /// Absolute, inside the instance.
    pub(super) cwd: &'a str,
    /// Merged into the instance's own environment.
    pub(super) env: BTreeMap<&'static str, String>,
}

/// What `POST /execute` and `GET /execute/{id}/status` answer. `log_path`,
/// a path inside the instance, and `command` are not read.
#[derive(Debug, Deserialize)]
pub(super) struct Process {
    pub(super) id: String,
    /// `running`, `done` or `killed`.
    pub(super) status: String,
    pub(super) exit_code: Option<i32>,
    pub(super) output: Vec<OutputEntry>,
    /// Whether entries before the offset asked for, or between it and what
    /// is given, were dropped.
    pub(super) truncated: bool,
    /// Counts entries, not bytes.
    pub(super) next_offset: u64,
}

/// A piece of what a process printed: `output` from a pseudo-terminal,
/// `stdout` or `stderr` from pipes.
#[derive(Debug, Deserialize)]
pub(super) struct OutputEntry {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) data: String,
}

/// What `GET /files/list` answers; `dir`, a path inside the instance, and
/// `writable` are not read.
#[derive(Debug, Deserialize)]
pub(super) struct Listing {
    pub(super) entries: Vec<ListedEntry>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ListedEntry {
    pub(super) name: String,
    /// `directory` or `file`, links followed.
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) size: u64,
    /// Seconds since the epoch.
    pub(super) modified: Option<f64>,
}

/// `POST /files/mkdir`.
#[derive(Debug, Serialize)]
pub(super) struct PathRequest<'a> {
    pub(super) path: &'a str,
}

/// `POST /files/move`.
#[derive(Debug, Serialize)]
pub(super) struct MoveRequest<'a> {
    pub(super) source: &'a str,
    pub(super) destination: &'a str,
}
