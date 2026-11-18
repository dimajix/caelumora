//! How a path of the REST API names an actor (issue 590): by its kind and its id, so that the
//! server hands the actor on as it is, and whether it takes part is asked where it is asked
//! anyway, by the kernel or the views.

use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::AgentId;
use caelumora_domain::user::UserId;
use serde::de::{self, Deserializer};
use serde::Deserialize;
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

/// An actor as a path of the REST API names it: its kind and its id, `User:<uuid>` or
/// `Agent:<uuid>` - the kind as the JSON of an `ActorId` names it, as the web UI keys an actor
/// (`actorKey`). Only a user or an agent is read from a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActorPath(pub ActorId);

/// A segment of a path that names neither a user nor an agent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("'{0}' names no actor: a user is User:<uuid>, an agent Agent:<uuid>")]
pub struct NotAnActorPath(String);

impl FromStr for ActorPath {
    type Err = NotAnActorPath;

    fn from_str(path: &str) -> Result<Self, Self::Err> {
        let not_an_actor = || NotAnActorPath(path.to_string());
        let (kind, id) = path.split_once(':').ok_or_else(not_an_actor)?;
        let id = Uuid::parse_str(id).map_err(|_| not_an_actor())?;
        match kind {
            "User" => Ok(Self(ActorId::User(UserId(id)))),
            "Agent" => Ok(Self(ActorId::Agent(AgentId(id)))),
            _ => Err(not_an_actor()),
        }
    }
}

/// The form `FromStr` reads - what a message of the server names an actor by, as the request named
/// it -, and for an actor no path names the kind alone, or with its id, as the web UI's `actorKey`
/// writes it.
impl fmt::Display for ActorPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            ActorId::User(id) => write!(f, "User:{}", id.0),
            ActorId::Agent(id) => write!(f, "Agent:{}", id.0),
            ActorId::Approver(id) => write!(f, "Approver:{}", id.0),
            ActorId::Delegate(id) => write!(f, "Delegate:{}", id.0),
            ActorId::System => f.write_str("System"),
            ActorId::None => f.write_str("None"),
        }
    }
}

/// Read as `FromStr` reads it, so that a path's extractor reads it.
impl<'de> Deserialize<'de> for ActorPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?.parse().map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_user_or_an_agent_is_read_as_it_is_written() {
        for actor in [ActorId::User(UserId::new()), ActorId::Agent(AgentId::new())] {
            let written = ActorPath(actor).to_string();
            assert_eq!(written.parse::<ActorPath>(), Ok(ActorPath(actor)), "{written}");
        }
        let id = Uuid::new_v4();
        assert_eq!(ActorPath(ActorId::Agent(AgentId(id))).to_string(), format!("Agent:{id}"));
    }

    #[test]
    fn what_names_no_user_or_agent_is_refused() {
        let id = Uuid::new_v4();
        for path in
            [id.to_string(), format!("Approver:{id}"), "System".to_string(), "User:x".into()]
        {
            assert!(path.parse::<ActorPath>().is_err(), "{path}");
        }
    }
}
