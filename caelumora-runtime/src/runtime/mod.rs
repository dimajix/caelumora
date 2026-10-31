//! The runtime: what everything in a process shares, and what of it a
//! channel and an actor are given.
//!
//! - The `RuntimeManager` (`manager`) holds the process-wide services and
//!   managers - the policies, the stores, the event bus, the controllers,
//!   the component kinds -, made once by the `RuntimeBuilder` (`builder`)
//!   from the configuration and what the plugins register (issue 323).
//! - A `ChannelRuntime` (`channel`) is what a channel is built with: the
//!   services it reads once, by name, the user commands and controller of
//!   its session, and a handle to the channel itself (issues 360, 361).
//! - A `ParticipantRuntime` (`actor`) is what a participant's actor runs with: the context it
//!   acts in and what concerns this actor alone (issues 69, 359, 516).
//! - An `ActorRuntime` (`agent`) is what the actor of an agent of an application runs with: the
//!   one interface its kind is given, which the core's `AgentRuntime` serves (issue 516).
//! - What the plugins bind to the views of an application's work is kept as
//!   its extensions (`extension`, issue 516).

mod actor;
mod agent;
mod builder;
mod channel;
mod extension;
mod manager;

pub use actor::ParticipantRuntime;
pub use agent::{ActorRuntime, AgentRuntime, CallPermit, OwnRecord};
pub use builder::RuntimeBuilder;
pub use channel::ChannelRuntime;
pub(crate) use extension::Face;
pub use manager::RuntimeManager;

use caelumora_model::type_map::TypeMap;

/// The services of the runtime, by type (issues 296, 323): the policy, the
/// secrets, the managers of the core that components use and what the
/// plugins provide. A component is given those its kind uses
/// (`ComponentKind::uses`), the views and a plugin's routes reach them all.
pub type Services = TypeMap;
