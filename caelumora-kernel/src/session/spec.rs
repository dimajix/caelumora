//! What a caller chooses of a new session and of a new channel: `SessionSpec`, taken by
//! `SessionManager::create_from_template`, and `ChannelSpec`, taken by
//! `SessionLoop::create_channel`.

use caelumora_domain::actor::ActorId;
use caelumora_domain::channel;
use caelumora_domain::component::ComponentConfig;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::project::ProjectId;
use caelumora_domain::template::InternalActorTemplate;
use caelumora_domain::workspace::WorkspaceChoice;

/// What a new session made from a template should be (issues 266, 424, 425, 467), as it was asked:
/// its name, language and time zone are made from it by `create_from_template`. What it leaves out
/// is the template's; without a project named, the session's results go to a project of its own.
#[derive(Debug, Clone, Default)]
pub struct SessionSpec {
    /// Its name; the template's if `None`.
    pub name: Option<String>,
    /// The language its channels are held in where they hold none of their own, a BCP 47 tag; the
    /// template's if `None`.
    pub language: Option<String>,
    /// The time zone it is held in, an IANA name such as `Europe/Berlin`; the template's if `None`,
    /// else UTC.
    pub time_zone: Option<String>,
    /// Its workspace in place of the template's; the template's if `None`.
    pub workspace: Option<WorkspaceChoice>,
    /// The shared project its results go to; a project of its own if `None`.
    pub project: Option<ProjectId>,
}

/// What a new channel should be (issue 09), as it was asked: its name and
/// language are made from it by `create_channel` (issue 248).
#[derive(Debug, Clone)]
pub struct ChannelSpec {
    pub name: String,
    /// The channel's components (issue 174): its moderator and the policy
    /// for the moderator's slot; the defaults for what is left out, as in a
    /// template.
    pub components: ComponentConfig,
    /// Who takes part, from the session's participants. Whoever creates the
    /// channel takes part anyway, if they are a participant of the session.
    pub participants: Vec<ActorId>,
    /// The classification of the channel's contents (issue 133). Set by
    /// whoever may create the channel - its owner.
    pub classification: Classification,
    /// The language its conversation is held in (issue 56); `None` for none
    /// in particular. Offering the one its creator prefers is the
    /// frontend's (issue 331).
    pub language: Option<String>,
    /// Whether its participants work on the session's workspace in it (issue
    /// 253).
    pub uses_workspace: bool,
    /// The actors that belong to the channel itself (issue 420): its
    /// approver, if one is named - none that is not.
    pub internal_actors: Vec<InternalActorTemplate>,
}

impl ChannelSpec {
    /// A channel of `name`, as a template makes one that says nothing else: the default
    /// components and classification, nobody but its creator in it, no language in particular,
    /// using the session's workspace, and no actors of its own.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            components: ComponentConfig::default(),
            participants: Vec::new(),
            classification: Classification::default(),
            language: None,
            uses_workspace: channel::uses_workspace_by_default(),
            internal_actors: Vec::new(),
        }
    }
}
