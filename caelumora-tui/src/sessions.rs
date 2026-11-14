//! The sessions of the directory the terminal program works in (issue 554), as its user reaches
//! them: those of its project, listed by their last message, newest first, and the one to start,
//! continue or resume.

use crate::here::HERE;
use crate::printed::Printed;
use anyhow::{bail, Context, Result};
use caelumora_domain::history::HistoryEntry;
use caelumora_domain::message::Message;
use caelumora_domain::project::ProjectId;
use caelumora_domain::query::{Filter, Query};
use caelumora_domain::session::SessionId;
use caelumora_domain::store::StoreResultExt;
use caelumora_domain::user::UserId;
use caelumora_domain::value::language::Language;
use caelumora_domain::workspace::{WorkspaceChoice, WorkspaceId};
use caelumora_kernel::kernel::Kernel;
use caelumora_kernel::session::{SessionError, SessionLoop, SessionManager, SessionSpec};
use caelumora_kernel::view::{ChannelView, Views};
use chrono::{DateTime, Local, Utc};
use std::cmp::Reverse;
use std::sync::Arc;
use tracing::warn;

/// How many characters of the last message a listed session shows.
const SHOWN: usize = 60;

/// What the program is asked to do with the sessions of its directory.
pub(crate) enum Asked {
    /// List them, and open none.
    List,
    /// Open one, to talk in it.
    Open(Opening),
}

/// Which session of the directory is to be opened.
pub(crate) enum Opening {
    /// A new one.
    Start,
    /// The one spoken in last, or a new one if there is none.
    Continue,
    /// The one whose id begins so.
    Resume(String),
}

/// The sessions of the directory the program was started in, as its user reaches them: all of
/// them in the directory's project, where the program works in one.
pub(crate) struct Sessions {
    /// The kernel, whose stores of templates and of profiles a session is started by: held, not
    /// the stores, since the terminal links no crate of stores (`docs/CRATES.md`), and the views
    /// read neither.
    kernel: Arc<Kernel>,
    /// What opens and starts them.
    manager: Arc<SessionManager>,
    /// The user the program acts for.
    owner: UserId,
    /// The views as the user reads them, in whose name - theirs, with their roles - the sessions
    /// are reached.
    views: Views,
    /// Where the program was started, its project and workspace, if it works there.
    place: Option<(ProjectId, WorkspaceId)>,
}

impl Sessions {
    /// The sessions of `owner`, read through `views` - in their name -, run by `manager` in
    /// `kernel`, in `place`, the project and workspace of the directory the program was started
    /// in, if it works there.
    pub(crate) fn new(
        kernel: Arc<Kernel>,
        manager: Arc<SessionManager>,
        owner: UserId,
        views: Views,
        place: Option<(ProjectId, WorkspaceId)>,
    ) -> Self {
        Self { kernel, manager, owner, views, place }
    }

    /// The sessions of the directory the viewer may read, newest first by the last message heard
    /// in a channel they read, a session without one by when it was made.
    ///
    /// # Errors
    /// If the program works in no directory (`project`), or the sessions cannot be read.
    pub(crate) async fn list(&self) -> Result<Vec<Listed>> {
        let mut listed = Vec::new();
        for session in self.views.project(self.project()?).await?.sessions().await? {
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

    /// The session `opening` asks for, running - started from the template named `template`
    /// where a new one is -, with what tells how it was chosen where that is not what was asked.
    ///
    /// # Errors
    /// If the program works in no directory where a session of it is asked for (`project`), it
    /// cannot be found or is open in another terminal, or starting one fails (`start`).
    pub(crate) async fn open(
        &self,
        opening: &Opening,
        template: &str,
    ) -> Result<(Arc<SessionLoop>, Option<Printed>)> {
        let id = match opening {
            Opening::Start => None,
            Opening::Continue => self.list().await?.first().map(|session| session.id),
            Opening::Resume(prefix) => {
                let sessions = self.views.project(self.project()?).await?.sessions().await?;
                Some(beginning_with(sessions.iter().map(|session| session.id()), prefix)?)
            }
        };
        let Some(id) = id else {
            let told = matches!(opening, Opening::Continue).then(|| {
                Printed::Notice(
                    "No session was started in this directory yet: a new one starts.".to_string(),
                )
            });
            return Ok((self.start(template).await?, told));
        };
        match self.manager.open(self.views.context(), id).await {
            Err(e) if matches!(e.downcast_ref(), Some(SessionError::RunsElsewhere(_))) => {
                bail!("The session {id} is open in another terminal")
            }
            opened => {
                Ok((opened.with_context(|| format!("The session {id} cannot be resumed"))?, None))
            }
        }
    }

    /// A new session of the user, started from the template named `template` - among those they
    /// may use -, in the directory's project and workspace, if the program works there.
    ///
    /// # Errors
    /// If there is no such template, or making the session fails.
    async fn start(&self, template: &str) -> Result<Arc<SessionLoop>> {
        let stores = self.kernel.entity_stores();
        let security = self.views.context();
        let templates = stores.session_templates.list(security, &Query::all()).await?.items;
        let template = templates.into_iter().find(|t| t.name.name == template).ok_or_else(|| {
            anyhow::anyhow!("Session template '{template}' not found: templates lie in `templates` of the configuration directory, or come from the `seed` of the configuration")
        })?;
        // Held in the language the user prefers where the template names none:
        // the frontend's to offer, not the server's (issues 331, 425).
        let preferred = stores
            .user_profiles
            .get(security, self.owner)
            .await
            .optional()?
            .and_then(|profile| profile.preferred_language);
        let language = preferred.filter(|_| template.language.is_none());
        let spec = SessionSpec {
            language: language.as_ref().map(Language::to_string),
            workspace: self.place.map(|(_, id)| WorkspaceChoice::Existing { id, share: None }),
            project: self.place.map(|(project, _)| project),
            ..SessionSpec::default()
        };
        self.manager.create_from_template(security, template.id, spec).await
    }

    /// The project of the directory the program was started in.
    ///
    /// # Errors
    /// If the program works in no directory: the configuration names no storage `here`.
    fn project(&self) -> Result<ProjectId> {
        self.place.map(|(project, _)| project).ok_or_else(|| {
            anyhow::anyhow!(
                "Sessions are listed, continued and resumed by the directory they were started \
                 in, which the configuration does not work in (it names no storage `{HERE}`)"
            )
        })
    }
}

/// A session of the directory as it is listed: its id, its name, when it was last spoken in - or
/// made, where nothing was said yet - and the beginning of what was said last.
#[derive(Debug)]
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

/// `listed`, the session spoken in last first.
fn newest_first(mut listed: Vec<Listed>) -> Vec<Listed> {
    listed.sort_by_key(|session| Reverse(session.last));
    listed
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
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::message::testing::make_message;
    use caelumora_domain::message::MessageId;
    use caelumora_domain::template::testing::make_template;
    use caelumora_kernel::session::testing::{running_session_of_a_user, test_kernel};
    use caelumora_kernel::view::Viewer;
    use caelumora_security::SecurityContext;
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

    /// A session is started from the template named, among those the user may use; continued,
    /// resumed and listed only by the directory the program works in.
    #[tokio::test]
    async fn a_session_is_started_from_the_template_named_and_others_by_the_directory() {
        let kernel = test_kernel();
        let manager = Arc::new(SessionManager::new(Arc::clone(&kernel)));
        let (_, owner, _) = running_session_of_a_user(&manager).await;
        let mut template = make_template("default");
        let system = SecurityContext::system();
        kernel.entity_stores().session_templates.create(&system, &mut template).await.unwrap();
        let ActorId::User(user) = owner.caller_id else { unreachable!("a user") };
        let views = manager.views(Viewer::new(owner));
        let sessions = Sessions::new(kernel, Arc::clone(&manager), user, views, None);

        let (started, told) = sessions.open(&Opening::Start, "default").await.expect("started");
        assert_eq!(started.name(), "default", "named after the template");
        assert!(told.is_none(), "as asked");
        let missing = sessions.open(&Opening::Start, "missing").await.expect_err("no such");
        assert!(missing.to_string().contains("'missing' not found"), "{missing}");

        let by_directory = "the configuration does not work in";
        let continued = sessions.open(&Opening::Continue, "default").await.expect_err("refused");
        assert!(continued.to_string().contains(by_directory), "{continued}");
        let resumed = Opening::Resume("0a1b".to_string());
        let resumed = sessions.open(&resumed, "default").await.expect_err("refused");
        assert!(resumed.to_string().contains(by_directory), "{resumed}");
        let listed = sessions.list().await.expect_err("refused");
        assert!(listed.to_string().contains(by_directory), "{listed}");
    }
}
