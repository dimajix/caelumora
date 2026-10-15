//! The live documents of a running session and their one writer each
//! (issue 86): a controller holds the current `Session`, `Channel`, `Agent`,
//! `User` or `Workspace`, stores a change first - which is where the permission is
//! checked - and only then takes it over and announces it on the event bus.
//! The documents themselves are the model's; keeping them live, writing
//! them and announcing is the runtime's.
//!
//! Every controller holds its document as a `LiveDocument` (issue 358),
//! which keeps it live, holds a lock while it is changed, stores it - with
//! when it was changed (`Updatable::touch`) -, announces it and takes over
//! changes made elsewhere (`process_global_event`), and notes each change in
//! the log. What a controller does besides, it does between the steps of a
//! `Writing`.
//! The user's writes the user's profile; the user record itself belongs to
//! the system and is not changed through it.
//!
//! A session and its channels run in one instance, and so do their
//! controllers. Agents and users do not belong to a session: one of them
//! takes part in several, perhaps in several instances. Nor do workspaces:
//! one may be attached to several sessions, and is used directly too
//! (issue 344). The `ControllerManager` keeps one controller per agent, per
//! user and per workspace of its instance, shared by all that use it, and
//! takes the changes announced about them into those controllers (issue
//! 121).

mod agent;
mod channel;
mod document;
mod manager;
mod session;
mod user;
mod workspace;

pub use agent::AgentController;
pub use channel::ChannelController;
pub use manager::ControllerManager;
pub use session::SessionController;
pub use user::UserController;
pub use workspace::WorkspaceController;

use caelumora_model::approval::ApprovalRule;
use caelumora_model::store::{StoreError, StoreResult};

/// Nothing if a place may keep every one of `rules` (issue 13): a
/// session's or a channel's rules only ask or deny, everywhere within it.
///
/// # Errors
/// `StoreError::Validation` naming the first rule that does not, and why.
fn check_restricting(rules: &[ApprovalRule]) -> StoreResult<()> {
    for rule in rules {
        rule.check_restricting().map_err(|e| StoreError::Validation(format!("the rule for `{}`: {e}", rule.matcher.tool)))?;
    }
    Ok(())
}
