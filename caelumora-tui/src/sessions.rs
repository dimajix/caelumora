//! The sessions of the directory the terminal program works in (issue 554): those of its project,
//! listed by their last message, newest first, and the one to continue or resume found among them.

use anyhow::{bail, Result};
use caelumora_domain::history::HistoryEntry;
use caelumora_domain::message::Message;
use caelumora_domain::project::ProjectId;
use caelumora_domain::query::Filter;
use caelumora_domain::session::SessionId;
use caelumora_kernel::view::{ChannelView, Views};
use chrono::{DateTime, Local, Utc};
use std::cmp::Reverse;
use tracing::warn;

/// How many characters of the last message a listed session shows.
const SHOWN: usize = 60;

/// What the program is asked to do with the sessions of its directory.
pub(crate) enum Asked {
    /// Start a session.
    Start,
    /// Continue the one spoken in last, or start one if there is none.
    Continue,
    /// Resume the one whose id begins so.
    Resume(String),
    /// List them, and start none.
    List,
}

/// A session of the directory as it is listed: its id, its name, when it was last spoken in - or
/// made, where nothing was said yet - and the beginning of what was said last.
pub(crate) struct Listed {
    id: SessionId,
    name: String,
    last: DateTime<Utc>,
    said: Option<String>,
}

impl Listed {
    /// Session `id`, called `name` and made at `made`, whose last message heard is `last`, if any.
    fn of(id: SessionId, name: String, made: DateTime<Utc>, last: Option<Message>) -> Self {
        Self {
            id,
            name,
            last: last.as_ref().map_or(made, |message| message.timestamp),
            said: last.and_then(|message| message.content).map(|content| beginning(&content)),
        }
    }

    /// The line `--sessions` prints of it: the beginning of its id, enough to resume it by, when
    /// it was last spoken in, its name and what was said last.
    pub(crate) fn line(&self) -> String {
        let id = self.id.to_string();
        let when = self.last.with_timezone(&Local).format("%Y-%m-%d %H:%M");
        let said = self.said.as_deref().map(|said| format!("  {said}")).unwrap_or_default();
        format!("{}  {when}  {}{said}", &id[..8], self.name)
    }
}

/// The sessions of `project` the viewer of `views` may read, newest first by the last message
/// heard in a channel the viewer reads, a session without one by when it was made.
///
/// # Errors
/// If the project or its sessions cannot be read.
pub(crate) async fn listed(views: &Views, project: ProjectId) -> Result<Vec<Listed>> {
    let mut listed = Vec::new();
    for session in views.project(project).await?.sessions().await? {
        let mut last: Option<Message> = None;
        for channel in session.channels().await {
            if let Some(message) = last_heard(&channel).await {
                if last.as_ref().is_none_or(|last| message.timestamp > last.timestamp) {
                    last = Some(message);
                }
            }
        }
        let name = session.name().to_string();
        listed.push(Listed::of(session.id(), name, session.created_at(), last));
    }
    Ok(newest_first(listed))
}

/// `listed`, the session spoken in last first.
fn newest_first(mut listed: Vec<Listed>) -> Vec<Listed> {
    listed.sort_by_key(|session| Reverse(session.last));
    listed
}

/// The session to open of `project`, as `asked`: for `Continue` the one spoken in last, for
/// `Resume` the one whose id begins so; `None` where one is to start.
///
/// # Errors
/// If the sessions cannot be read; for `Resume` if none of them begins so, or several do.
#[expect(clippy::print_stdout, reason = "the TUI writes to the terminal")]
pub(crate) async fn chosen(
    views: &Views,
    project: ProjectId,
    asked: &Asked,
) -> Result<Option<SessionId>> {
    match asked {
        Asked::Start | Asked::List => Ok(None),
        Asked::Continue => {
            let last = listed(views, project).await?.first().map(|session| session.id);
            if last.is_none() {
                println!("No session was started in this directory yet: a new one starts.");
            }
            Ok(last)
        }
        Asked::Resume(prefix) => {
            let sessions = views.project(project).await?.sessions().await?;
            Ok(Some(beginning_with(sessions.iter().map(|session| session.id()), prefix)?))
        }
    }
}

/// The last message heard in `channel` (`Filter::heard`), as its viewer reads it; `None` if
/// nothing was said, or its history cannot be read - which is logged, and hides no session.
async fn last_heard(channel: &ChannelView) -> Option<Message> {
    match channel.history_page_of(None, 1, Some(Filter::heard())).await {
        Ok(page) => page.items.into_iter().find_map(|(_, entry)| match entry {
            HistoryEntry::Said { message } => Some(message),
            _ => None,
        }),
        Err(e) => {
            warn!(channel = %channel.id(), "The history of a channel cannot be read: {e}");
            None
        }
    }
}

/// The beginning of `content`, on one line: its first line that is not blank, cut to `SHOWN`
/// characters.
fn beginning(content: &str) -> String {
    let line = content.lines().find(|line| !line.trim().is_empty()).unwrap_or_default().trim();
    match line.char_indices().nth(SHOWN) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_string(),
    }
}

/// The one of `ids` that begins with `prefix`, as `--resume` names a session.
///
/// # Errors
/// If `prefix` is blank, none of them begins so, or several do.
fn beginning_with(ids: impl Iterator<Item = SessionId>, prefix: &str) -> Result<SessionId> {
    let prefix = prefix.trim().to_lowercase();
    if prefix.is_empty() {
        bail!("No session named: --resume takes the beginning of its id, as --sessions shows it");
    }
    let mut found = ids.filter(|id| id.to_string().starts_with(&prefix));
    match (found.next(), found.next()) {
        (Some(id), None) => Ok(id),
        (Some(_), Some(_)) => bail!("Several sessions of this directory begin with '{prefix}'"),
        (None, _) => {
            bail!("No session of this directory begins with '{prefix}'; --sessions lists them")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::message::testing::make_message;
    use caelumora_domain::message::MessageId;
    use chrono::TimeDelta;
    use uuid::Uuid;

    fn id(text: &str) -> SessionId {
        SessionId(Uuid::parse_str(text).unwrap())
    }

    #[test]
    fn a_session_is_named_by_the_beginning_of_its_id_if_no_other_begins_so() {
        let ids = [
            id("0a1b2c3d-0000-4000-8000-000000000001"),
            id("0a1b9999-0000-4000-8000-000000000002"),
        ];
        let named = |prefix| beginning_with(ids.into_iter(), prefix);
        assert_eq!(named("0A1B2").unwrap(), ids[0], "as written, in any case");
        assert!(named("0a1b").is_err(), "two of them");
        assert!(named("ff").is_err(), "none");
        assert!(named(" ").unwrap_err().to_string().starts_with("No session named"), "nothing");
    }

    /// The session spoken in last comes first; one nobody spoke in counts by when it was made, and
    /// shows nothing said.
    #[test]
    fn sessions_are_listed_by_their_last_message_else_by_when_they_were_made() {
        let now = Utc::now();
        let hour = TimeDelta::hours(1);
        let said = |text: &str, at| Message {
            content: Some(text.to_string()),
            timestamp: at,
            ..make_message(MessageId::new())
        };
        let (old, quiet, recent) = (SessionId::new(), SessionId::new(), SessionId::new());
        let listed = newest_first(vec![
            Listed::of(old, "old".into(), now - hour * 5, Some(said("first", now - hour * 4))),
            Listed::of(quiet, "quiet".into(), now - hour * 2, None),
            Listed::of(recent, "recent".into(), now - hour * 6, Some(said("later", now - hour))),
        ]);
        let order: Vec<SessionId> = listed.iter().map(|session| session.id).collect();
        assert_eq!(order, [recent, quiet, old]);
        assert!(listed[0].line().ends_with("recent  later"), "{}", listed[0].line());
        assert!(listed[1].line().ends_with("quiet"), "{}", listed[1].line());
    }

    #[test]
    fn what_was_said_last_is_shown_by_its_first_line_and_its_beginning() {
        assert_eq!(beginning("\n  hello\nthere"), "hello");
        let long = "ä".repeat(70);
        assert_eq!(beginning(&long), format!("{}…", "ä".repeat(60)));
    }
}
