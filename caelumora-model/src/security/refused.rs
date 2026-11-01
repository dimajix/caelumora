//! `Refused`: an action the policy refused, as the error it ends in carries it.

/// An action refused (issue 323): what it was, without the attributes it
/// was asked with. Made where the permission is checked
/// (`Enforcement::require`) and carried by the error the refusal ends in,
/// as its source, so that whoever answers the error - the REST server -
/// finds it there and says what was refused, wherever the error went.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{action} is not permitted")]
pub struct Refused {
    /// What it is about, as the REST API names it in its codes (`skill_forbidden`): the kind of an
    /// entity, or `context`, `agent`, `command`, … for the kernel's own actions.
    pub subject: &'static str,
    /// The action as `ActionType::describe` names it.
    pub action: String,
}
