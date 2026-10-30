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
//! - An `ActorRuntime` (`actor`) is what an actor runs with: the services
//!   every actor needs and what concerns this actor alone (issues 69, 359).

mod actor;
mod builder;
mod channel;
mod manager;

pub use actor::ActorRuntime;
pub use builder::RuntimeBuilder;
pub use channel::ChannelRuntime;
pub use manager::RuntimeManager;

use caelumora_model::type_map::TypeMap;

/// The services of the runtime, by type (issues 296, 323): the policy, the
/// secrets, the managers of the core that components use and what the
/// plugins provide. A component is given those its kind uses
/// (`ComponentKind::uses`), the views and a plugin's routes reach them all.
pub type Services = TypeMap;
