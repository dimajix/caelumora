//! The tool `grep_files` of `workspace`: the lines of the files of the workspace that match a
//! pattern, bounded in the files read, the matches given and the length of a pattern.

use super::super::reading;
use super::super::tool;
use super::GREP_FILES;
use super::{of_file, path, text, READ_LIMIT};
use async_trait::async_trait;
use caelumora_agents::tool::{arguments, Tool, ToolError, ToolOutput, ToolResult};
use caelumora_agents::turn::Turn;
use caelumora_api::terminal::{TerminalReader, WorkspacePath};
use caelumora_domain::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::ToolDescription;
use caelumora_domain::tool::{ToolParameters, ToolPresentation};
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use serde_json::{json, Value};

/// The most files one search reads.
const MAX_FILES_SEARCHED: usize = 2000;
/// The most matches one search gives unless fewer are asked for.
const DEFAULT_MATCHES: usize = 100;
const MAX_MATCHES: usize = 1000;
/// The longest line of a match given, in characters.
const MATCH_CHARS: usize = 300;

/// The longest pattern, in characters, and how large it may compile.
const MAX_PATTERN_CHARS: usize = 500;
const MAX_COMPILED_PATTERN_BYTES: usize = 1 << 20;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Grep {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    max_matches: Option<usize>,
}

/// `line` as a match shows it: at most `MATCH_CHARS` characters.
fn shown(line: &str) -> String {
    let line = line.trim_end_matches(['\r', '\n']);
    match line.char_indices().nth(MATCH_CHARS) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_string(),
    }
}

/// Searches `terminal` from `start` for `pattern`; what was searched and
/// the matches.
async fn search(
    terminal: &dyn TerminalReader,
    start: WorkspacePath,
    pattern: &Regex,
    most: usize,
) -> ToolResult<Value> {
    let mut pending = vec![start];
    let mut matches = Vec::new();
    let (mut searched, mut skipped) = (0usize, 0usize);
    let mut complete = true;
    'walk: while let Some(at) = pending.pop() {
        let found = terminal.stat(&at).await?;
        if found.is_dir {
            let mut entries = terminal.list(&at).await?;
            entries.reverse();
            for listed in entries {
                let below = if at.is_root() {
                    path(&listed.name)?
                } else {
                    path(&format!("{at}/{}", listed.name))?
                };
                pending.push(below);
            }
            continue;
        }
        if searched == MAX_FILES_SEARCHED {
            complete = false;
            break;
        }
        searched += 1;
        if found.size > READ_LIMIT {
            skipped += 1;
            continue;
        }
        let Ok((text, _)) = text(terminal, &at).await else {
            skipped += 1;
            continue;
        };
        for (number, line) in text.lines().enumerate() {
            if pattern.is_match(line) {
                if matches.len() == most {
                    complete = false;
                    break 'walk;
                }
                matches.push(
                    json!({ "path": at.to_string(), "line": number + 1, "text": shown(line) }),
                );
            }
        }
    }
    Ok(
        json!({ "matches": matches, "files_searched": searched, "files_skipped": skipped, "complete": complete }),
    )
}

/// Files searched (issue 265): its `pattern` and `path`, and the
/// `matches` of its result - each `path`, `line` and `text` - with how
/// many files were searched and skipped, and whether it is `complete`.
pub(super) const FILE_SEARCH_PRESENTATION: ToolPresentation = ToolPresentation::new("file_search");

/// `grep_files`: the lines of the files of the workspace that match a pattern.
#[derive(Debug)]
pub(super) struct GrepFilesTool {
    description: ToolDescription,
}

impl GrepFilesTool {
    /// The tool, asking for approval if `requires_approval`, its texts from `prompts`, told how
    /// many matches a search gives.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    pub(super) fn new(prompts: &PromptTexts, requires_approval: bool) -> Result<Self, PromptError> {
        let parameters = ToolParameters::object(
            [
                ("pattern", ToolParameters::string()),
                ("path", ToolParameters::string()),
                ("max_matches", ToolParameters::number()),
            ],
            ["pattern"],
        );
        let values = context! { max_matches => MAX_MATCHES, default_matches => DEFAULT_MATCHES };
        let description = tool(
            prompts,
            GREP_FILES,
            parameters,
            requires_approval,
            FILE_SEARCH_PRESENTATION,
            values,
        )?;
        Ok(Self { description })
    }
}

#[async_trait]
impl Tool for GrepFilesTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let (workspace, terminal) = reading(turn).await?;
        let Grep { pattern, path: at, max_matches } = arguments(params)?;
        if pattern.chars().count() > MAX_PATTERN_CHARS {
            return Err(ToolError::InvalidParameter(format!(
                "the pattern is longer than {MAX_PATTERN_CHARS} characters"
            )));
        }
        let pattern = RegexBuilder::new(&pattern)
            .size_limit(MAX_COMPILED_PATTERN_BYTES)
            .build()
            .map_err(|e| {
                ToolError::InvalidParameter(format!("the pattern is no regular expression: {e}"))
            })?;
        let at = path(&at.unwrap_or_default())?;
        let found = search(
            terminal.as_ref(),
            at.clone(),
            &pattern,
            max_matches.unwrap_or(DEFAULT_MATCHES).clamp(1, MAX_MATCHES),
        )
        .await?;
        Ok(of_file(turn, workspace, &at, None, found))
    }

    fn describe(&self, params: &Value) -> ToolResult<String> {
        let Grep { pattern, path, .. } = arguments(params)?;
        Ok(format!("Search the workspace's '{}' for:\n{pattern}", path.unwrap_or_default()))
    }
}
