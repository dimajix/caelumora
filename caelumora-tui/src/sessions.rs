//! The sessions of the directory the terminal program works in (issues 554, 570), as its user
//! reaches them: those of its project, listed by their last message, newest first, started,
//! continued, resumed, stopped as the terminal leaves them, and deleted.

use crate::here::HERE;
use crate::id_prefix::{beginning_with, NotNamed};
use crate::printed::Printed;
use crate::typeable::Typeable;
use caelumora_domain::actor::ActorId;
use caelumora_domain::channel::ThreadId;
use caelumora_domain::command::ParameterKind;
use caelumora_domain::history::HistoryEntry;
use caelumora_domain::message::Message;
use caelumora_domain::project::ProjectId;
use caelumora_domain::query::{Filter, Query};
use caelumora_domain::session::SessionId;
use caelumora_domain::store::StoreResultExt;
use caelumora_domain::template::SessionTemplate;
use caelumora_domain::user::UserId;
use caelumora_domain::value::language::Language;
use caelumora_domain::workspace::{WorkspaceChoice, WorkspaceId};
use caelumora_kernel::kernel::Kernel;
use caelumora_kernel::session::{SessionError, SessionLoop, SessionManager, SessionSpec};
use caelumora_kernel::view::{ChannelView, SessionView, Views};
use chrono::{DateTime, Local, Utc};
use std::cmp::Reverse;
use std::error::Error;
use std::sync::Arc;
use tracing::warn;

/// How many characters of the last message a listed session shows.
const SHOWN: usize = 60;

/// What the program is asked to do with the sessions of its directory.
pub(crate) enum Invocation {
    /// List them, and open none.
    List,
    /// Open one, to talk in it.
    Open(Opening),
}

/// Which session of the directory is to be opened.
pub(crate) enum Opening {
    /// A new one, from the template named, else from the program's.
    Start {
        /// The template named, if one is.
        template: Option<String>,
    },
    /// The one spoken in last, or a new one if there is none.
    Continue,
    /// The one whose id begins so.
    Resume(String),
}

/// The sessions of the directory the program was started in, as its user reaches them: all of
/// them in the directory's project, where the program works in one. What fails is a
/// `SessionsError`.
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
    /// The template a session is started from where none is named: the program's (`--template`).
    template: String,
}

impl Sessions {
    /// The sessions of `owner`, read through `views` - in their name -, run by `manager` in
    /// `kernel`, in `place`, the project and workspace of the directory the program was started
    /// in, if it works there, started from the template named `template` where none is named.
    pub(crate) fn new(
        kernel: Arc<Kernel>,
        manager: Arc<SessionManager>,
        owner: UserId,
        views: Views,
        place: Option<(ProjectId, WorkspaceId)>,
        template: String,
    ) -> Self {
        Self { kernel, manager, owner, views, place, template }
    }

    /// The sessions of the directory the viewer may read, newest first by the last message heard
    /// in a channel they read, a session without one by when it was made.
    ///
    /// # Errors
    /// If the program works in no directory (`SessionsError::NoDirectory`), or the sessions cannot
    /// be read.
    pub(crate) async fn list(&self) -> SessionsResult<Vec<Listed>> {
        let mut listed = Vec::new();
        for session in self.of_directory().await? {
            let mut last: Option<Message> = None;
            for channel in session.channels().await {
                if let Some(message) = last_heard(&channel, ActorId::User(self.owner)).await {
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

    /// The session `opening` asks for, running, with what tells how it was chosen where that is
    /// not what was asked.
    ///
    /// # Errors
    /// If the program works in no directory where a session of it is asked for, it
    /// cannot be found or is open in another terminal, or starting one fails (`start`).
    pub(crate) async fn open(
        &self,
        opening: &Opening,
    ) -> SessionsResult<(Arc<SessionLoop>, Option<Printed>)> {
        let id = match opening {
            Opening::Start { .. } => None,
            Opening::Continue => self.list().await?.first().map(|session| session.id),
            Opening::Resume(prefix) => Some(self.id_beginning_with(prefix).await?),
        };
        let Some(id) = id else {
            let template = match opening {
                Opening::Start { template: Some(template) } => template,
                _ => &self.template,
            };
            let told = matches!(opening, Opening::Continue).then(|| {
                Printed::Notice(
                    "No session was started in this directory yet: a new one starts.".to_string(),
                )
            });
            return Ok((self.start(template).await?, told));
        };
        match self.manager.open(self.views.context(), id).await {
            Ok(opened) => Ok((opened, None)),
            Err(e) if matches!(e.downcast_ref(), Some(SessionError::RunsElsewhere(_))) => {
                Err(SessionsError::OpenElsewhere(id))
            }
            Err(e) => Err(SessionsError::failed(format!("The session {id} cannot be resumed"), e)),
        }
    }

    /// Stops the session `id`, which the terminal leaves for another.
    ///
    /// # Errors
    /// As `SessionManager::stop`.
    pub(crate) async fn stop(&self, id: SessionId) -> SessionsResult<()> {
        let stopped = self.manager.stop(self.views.context(), id).await;
        stopped
            .map_err(|e| SessionsError::failed(format!("The session {id} cannot be stopped"), e))?;
        Ok(())
    }

    /// Deletes the session of the directory whose id begins with `prefix`, but `talked_in`, the
    /// one the user talks in: it can be restored.
    ///
    /// # Errors
    /// If no session or several begin so (`id_beginning_with`), it is the one talked in, or
    /// deleting it fails. One gone meanwhile counts as deleted: for whoever wants it gone it is
    /// gone.
    pub(crate) async fn delete(
        &self,
        prefix: &str,
        talked_in: SessionId,
    ) -> SessionsResult<SessionId> {
        let id = self.id_beginning_with(prefix).await?;
        if id == talked_in {
            return Err(SessionsError::TalkedIn(id));
        }
        let deleted = self.manager.delete(self.views.context(), id).await;
        deleted
            .map_err(|e| SessionsError::failed(format!("The session {id} cannot be deleted"), e))?;
        Ok(id)
    }

    /// The sessions of the directory, as the user may read them.
    ///
    /// # Errors
    /// If the program works in no directory (`SessionsError::NoDirectory`), or its sessions
    /// cannot be read.
    async fn of_directory(&self) -> SessionsResult<Vec<SessionView>> {
        let project = self.place.map(|(project, _)| project).ok_or(SessionsError::NoDirectory)?;
        let read = async { self.views.project(project).await?.sessions().await };
        let unreadable =
            |e| SessionsError::failed("The sessions of this directory cannot be read", e);
        read.await.map_err(unreadable)
    }

    /// Adds to `typeable` what the user can type of the directory (issue 579): its sessions, by
    /// their ids as listed (`shown_id`), and the templates they may start one from. What cannot be
    /// read - also where the program works in no directory - adds nothing.
    pub(crate) async fn add_typeable(&self, typeable: &mut Typeable) {
        let ids = self.ids().await.unwrap_or_default();
        typeable.add_named(ParameterKind::Session, ids.into_iter().map(shown_id));
        let templates = self.templates().await.unwrap_or_default();
        typeable
            .add_named(ParameterKind::Template, templates.iter().map(|t| t.name.name.to_string()));
    }

    /// The ids of the sessions of the directory, as the user may read them: what a session is named
    /// by.
    ///
    /// # Errors
    /// As `of_directory`.
    async fn ids(&self) -> SessionsResult<Vec<SessionId>> {
        Ok(self.of_directory().await?.iter().map(SessionView::id).collect())
    }

    /// The session templates the user may start a session from.
    ///
    /// # Errors
    /// If they cannot be read.
    async fn templates(&self) -> SessionsResult<Vec<SessionTemplate>> {
        let stores = self.kernel.entity_stores();
        let templates = stores.session_templates.list(self.views.context(), &Query::all()).await;
        let unreadable = |e| SessionsError::failed("The session templates cannot be read", e);
        Ok(templates.map_err(unreadable)?.items)
    }

    /// The session of the directory whose id begins with `prefix` (`beginning_with`), as
    /// `--resume`, `/session resume` and `/session delete` name a session.
    ///
    /// # Errors
    /// If the program works in no directory, its sessions cannot be read, or none of
    /// them or several begin so.
    async fn id_beginning_with(&self, prefix: &str) -> SessionsResult<SessionId> {
        beginning_with(self.ids().await?, SessionId::to_string, prefix).map_err(|not| match not {
            NotNamed::Blank => SessionsError::NoneNamed,
            NotNamed::None(prefix) => SessionsError::NoneBeginning(prefix),
            NotNamed::Several(prefix) => SessionsError::SeveralBeginning(prefix),
        })
    }

    /// A new session of the user, started from the template named `template` - among those they
    /// may use -, in the directory's project and workspace, if the program works there.
    ///
    /// # Errors
    /// If there is no such template, or making the session fails.
    async fn start(&self, template: &str) -> SessionsResult<Arc<SessionLoop>> {
        let stores = self.kernel.entity_stores();
        let security = self.views.context();
        let named = self.templates().await?.into_iter().find(|t| t.name.name == template);
        let template = named.ok_or_else(|| SessionsError::NoTemplate(template.to_string()))?;
        // Held in the language the user prefers where the template names none:
        // the frontend's to offer, not the server's (issues 331, 425).
        let profile = stores.user_profiles.get(security, self.owner).await.optional();
        let profile =
            profile.map_err(|e| SessionsError::failed("Your profile cannot be read", e))?;
        let preferred = profile.and_then(|profile| profile.preferred_language);
        let language = preferred.filter(|_| template.language.is_none());
        let spec = SessionSpec {
            language: language.as_ref().map(Language::to_string),
            workspace: self.place.map(|(_, id)| WorkspaceChoice::Existing { id, share: None }),
            project: self.place.map(|(project, _)| project),
            ..SessionSpec::default()
        };
        let started = self.manager.create_from_template(security, template.id, spec).await;
        let what = format!("No session can be started from the template '{}'", template.name.name);
        started.map_err(|e| SessionsError::failed(what, e))
    }
}

/// Why what was asked of the sessions of the directory (`Sessions`) failed: what the user asked
/// that cannot be - told as it is -, or what failed of the kernel - told with what of its cause may
/// be shown (`talk::error_printed`).
#[derive(Debug, thiserror::Error)]
pub(crate) enum SessionsError {
    #[error(
        "Sessions are listed, continued, resumed and deleted by the directory they were started \
         in, which the configuration does not work in (it names no storage `{}`)",
        HERE
    )]
    NoDirectory,
    #[error(
        "No session named: give the beginning of its id, as --sessions or /session list shows it"
    )]
    NoneNamed,
    #[error(
        "No session of this directory begins with '{0}'; --sessions or /session list lists them"
    )]
    NoneBeginning(String),
    #[error("Several sessions of this directory begin with '{0}'")]
    SeveralBeginning(String),
    #[error("Session template '{0}' not found: templates lie in `templates` of the configuration directory, or come from the `seed` of the configuration")]
    NoTemplate(String),
    #[error("The session {0} is open in another terminal")]
    OpenElsewhere(SessionId),
    #[error("You talk in the session {0}: switch to another before you delete it")]
    TalkedIn(SessionId),
    /// What failed of the kernel, `what` telling it.
    #[error("{what}")]
    Failed {
        what: String,
        #[source]
        cause: Box<dyn Error + Send + Sync>,
    },
}

/// What is asked of the sessions of the directory comes to, or why not.
pub(crate) type SessionsResult<T> = Result<T, SessionsError>;

impl SessionsError {
    /// That `what` failed, for `cause` - kept as the error it is, which the user may be told of
    /// where it is meant for them (`user_message`): anyhow's own boxing would hide it.
    fn failed(what: impl Into<String>, cause: impl Into<anyhow::Error>) -> Self {
        let cause = cause.into().reallocate_into_boxed_dyn_error_without_backtrace();
        Self::Failed { what: what.into(), cause }
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

    /// The line `--sessions` and `/session list` print of it: the beginning of its id, enough to
    /// resume it by, when it was last spoken in, its name and what was said last.
    pub(crate) fn line(&self) -> String {
        let when = self.last.with_timezone(&Local).format("%Y-%m-%d %H:%M");
        let said = self.said.as_deref().map(|said| format!("  {said}")).unwrap_or_default();
        format!("{}  {when}  {}{said}", shown_id(self.id), self.name)
    }
}

/// How the session `id` is shown, and named most briefly: the first eight characters of its id, as
/// a session is listed and completed (issue 579).
pub(crate) fn shown_id(id: SessionId) -> String {
    id.to_string().chars().take(8).collect()
}

/// What tells of the sessions `listed`, a line each (`Listed::line`), `talked_in` marked as the
/// one talked in; that none was started yet, where there are none.
pub(crate) fn listing(listed: &[Listed], talked_in: Option<SessionId>) -> Vec<String> {
    if listed.is_empty() {
        return vec!["No session was started in this directory yet.".to_string()];
    }
    let line = |session: &Listed| match Some(session.id) == talked_in {
        true => format!("{}, talked in", session.line()),
        false => session.line(),
    };
    listed.iter().map(line).collect()
}

/// `listed`, the session spoken in last first.
fn newest_first(mut listed: Vec<Listed>) -> Vec<Listed> {
    listed.sort_by_key(|session| Reverse(session.last));
    listed
}

/// The last message heard in `channel` (`Filter::heard`) that the view of its line shows `viewer`
/// (`Filter::shown_in_line`, issue 557), as they read it; `None` if nothing was said, or its
/// history cannot be read - which is logged, and hides no session.
async fn last_heard(channel: &ChannelView, viewer: ActorId) -> Option<Message> {
    let in_line = Filter::shown_in_line(ThreadId::line(channel.id()), viewer);
    let heard = Filter::all_of([Filter::heard(), in_line]);
    match channel.history_page_of(None, 1, Some(heard)).await {
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

#[cfg(test)]
pub(crate) mod testing {
    //! What the tests of the terminal reach the sessions of a directory by: a directory the program
    //! works in, as it does where its configuration names the storage `here`.

    use super::Sessions;
    use crate::here::{place_of, HERE};
    use crate::home;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::command::default_commands;
    use caelumora_domain::component::ComponentConfig;
    use caelumora_domain::config::{AppConfig, BackendConfig, WorkspacesConfig};
    use caelumora_domain::project::ProjectId;
    use caelumora_domain::template::testing::{make_channel_template, make_template};
    use caelumora_domain::template::SessionTemplate;
    use caelumora_domain::user::testing::make_user;
    use caelumora_domain::workspace::WorkspaceId;
    use caelumora_kernel::kernel::Kernel;
    use caelumora_kernel::project::ProjectManager;
    use caelumora_kernel::session::SessionManager;
    use caelumora_kernel::testing::TestKernel;
    use caelumora_kernel::view::Viewer;
    use caelumora_security::SecurityContext;
    use caelumora_store::testing::record_users;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// The sessions of a directory the program works in, with what they run in.
    pub(crate) struct InDirectory {
        /// The sessions, started from the template `default` - one channel `main` of the user's -
        /// unless another is named.
        pub(crate) sessions: Sessions,
        /// What runs them.
        pub(crate) manager: Arc<SessionManager>,
        /// What they run in.
        pub(crate) kernel: Arc<Kernel>,
        /// The user they are reached for.
        pub(crate) owner: SecurityContext,
        /// The directory, to be kept as long as the test runs.
        pub(crate) directory: TempDir,
    }

    /// The sessions of a new directory the program works in: its storage `here` is a `local` one in
    /// it, its project and workspace found or made as the program finds them (`place_of`).
    pub(crate) async fn in_directory() -> InDirectory {
        in_directory_where(false, default_commands()).await
    }

    /// The same, its storage running commands, and a session with the program's commands, as it
    /// has them by default (`home::commands`, issue 587).
    pub(crate) async fn in_directory_running_commands() -> InDirectory {
        in_directory_where(true, home::commands()).await
    }

    /// The sessions of a new directory the program works in, its storage running commands if it
    /// has a `shell`, a session with `commands`.
    pub(crate) async fn in_directory_where(shell: bool, commands: ComponentConfig) -> InDirectory {
        let directory = tempfile::tempdir().unwrap();
        let params = json!({
            "root": directory.path(),
            "shell": shell,
            "destination": { "level": "internal" },
        });
        let workspaces = WorkspacesConfig {
            storage: [(HERE.to_string(), BackendConfig::new("local", params))].into(),
        };
        let config = AppConfig { workspaces, commands, ..Default::default() };
        let kernel = TestKernel { config, ..Default::default() }.build();
        let user = make_user("owner");
        record_users(kernel.entity_stores(), &[&user]).await;
        let owner =
            SecurityContext::new(ActorId::User(user.id), Default::default(), Default::default());
        let manager = Arc::new(SessionManager::new(Arc::clone(&kernel)));
        let projects = ProjectManager::new(Arc::clone(&manager));
        let place = place_of(&kernel, &projects, &owner, user.id, directory.path()).await.unwrap();
        let sessions = sessions_of(&kernel, &manager, &owner, Some(place)).await;
        InDirectory { sessions, manager, kernel, owner, directory }
    }

    /// The sessions of the user `owner` acts for, run by `manager` in `kernel`, in `place`; the
    /// templates `default` - one channel `main` of the user's - and `empty` - no channel - stored,
    /// `default` the program's.
    pub(crate) async fn sessions_of(
        kernel: &Arc<Kernel>,
        manager: &Arc<SessionManager>,
        owner: &SecurityContext,
        place: Option<(ProjectId, WorkspaceId)>,
    ) -> Sessions {
        let system = SecurityContext::system();
        let main = vec![make_channel_template("main")];
        let mut templates = [
            SessionTemplate { channels: main, ..make_template("default") },
            make_template("empty"),
        ];
        for template in &mut templates {
            kernel.entity_stores().session_templates.create(&system, template).await.unwrap();
        }
        let ActorId::User(user) = owner.caller_id else { panic!("sessions are a user's") };
        let views = manager.views(Viewer::new(owner.clone()));
        let template = "default".to_string();
        Sessions::new(Arc::clone(kernel), Arc::clone(manager), user, views, place, template)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::message::testing::make_message;
    use caelumora_domain::message::MessageId;
    use caelumora_kernel::session::testing::{running_session_of_a_user, test_kernel};
    use chrono::TimeDelta;

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
        let sessions = testing::sessions_of(&kernel, &manager, &owner, None).await;

        let start =
            |template: Option<&str>| Opening::Start { template: template.map(str::to_string) };
        let (started, told) = sessions.open(&start(None)).await.expect("started");
        assert_eq!(started.name(), "default", "named after the program's template");
        assert!(told.is_none(), "as asked");
        let missing = sessions.open(&start(Some("missing"))).await.expect_err("no such");
        assert!(missing.to_string().contains("'missing' not found"), "{missing}");

        let by_directory = "the configuration does not work in";
        let continued = sessions.open(&Opening::Continue).await.expect_err("refused");
        assert!(continued.to_string().contains(by_directory), "{continued}");
        let resumed = Opening::Resume("0a1b".to_string());
        let resumed = sessions.open(&resumed).await.expect_err("refused");
        assert!(resumed.to_string().contains(by_directory), "{resumed}");
        let listed = sessions.list().await.expect_err("refused");
        assert!(listed.to_string().contains(by_directory), "{listed}");
    }

    /// In a directory the program works in, its sessions are listed newest first, continued and
    /// resumed by the beginning of their id, and deleted - but the one talked in.
    #[tokio::test]
    async fn the_sessions_of_a_directory_are_listed_resumed_and_deleted() {
        let directory = testing::in_directory().await;
        let sessions = &directory.sessions;
        let (none, told) = sessions.open(&Opening::Continue).await.expect("a new one");
        assert!(told.is_some(), "told that none was there to continue");
        let start = Opening::Start { template: None };
        let (other, _) = sessions.open(&start).await.expect("started");
        let listed: Vec<SessionId> =
            sessions.list().await.unwrap().iter().map(|session| session.id).collect();
        assert_eq!(listed.len(), 2, "both");
        assert!(listed.contains(&none.id()) && listed.contains(&other.id()));

        let prefix = other.id().to_string()[..8].to_string();
        let (resumed, _) = sessions.open(&Opening::Resume(prefix.clone())).await.expect("resumed");
        assert_eq!(resumed.id(), other.id());
        let refused = sessions.delete(&prefix, other.id()).await.expect_err("talked in");
        assert!(refused.to_string().contains("switch to another"), "{refused}");
        assert_eq!(sessions.delete(&prefix, none.id()).await.expect("deleted"), other.id());
        let listed: Vec<SessionId> =
            sessions.list().await.unwrap().iter().map(|session| session.id).collect();
        assert_eq!(listed, [none.id()], "the other deleted");
    }

    /// What failed of the kernel keeps its cause as the error it is, for what the user may be told
    /// of it to be found.
    #[test]
    fn a_failure_keeps_its_cause_as_it_is() {
        let id = SessionId::new();
        let failed =
            SessionsError::failed("deleting", anyhow::Error::from(SessionError::RunsElsewhere(id)));
        let cause = failed.source().expect("a cause");
        assert!(matches!(cause.downcast_ref(), Some(SessionError::RunsElsewhere(of)) if *of == id));
    }
}
