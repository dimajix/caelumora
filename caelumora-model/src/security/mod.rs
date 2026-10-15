//! Who may do what (issues 11, 23, 304): in whose name something is done,
//! what is stored about who may, what is asked, and who decides.
//!
//! - A `SecurityContext` (`context`) says who acts, who answers for it,
//!   with whose rights and for whom (issues 255, 341) - what every
//!   operation is asked in, and what the `ConfidentialityPolicy` clears.
//! - `SecurityAttributes` (`attributes`) are what an entity carries of who
//!   may reach it: its owner and its access list of `Subject`s and their
//!   `Grant`s.
//! - An `ActionType` (`action`) is what is asked: what is done with a
//!   stored entity of any kind, by a `Verb`, or one of the core's acts.
//! - The `AccessPolicy` (`access`) decides, one per process; the questions
//!   asked of it from outside are `AccessQuestions`, what an operation
//!   enforces is `Enforcement`. The policy of a process is the
//!   `OwnershipPolicy` (`ownership`), or the `AllowAllPolicy` for tests.
//!
//! What may flow where is not decided here, but by the
//! `ConfidentialityPolicy` (`crate::confidentiality`), asked beside it.

mod access;
mod action;
mod attributes;
mod context;
mod ownership;
#[cfg(test)]
mod testing;

pub use access::{AccessPolicy, AccessQuestions, AllowAllPolicy, Enforcement};
pub use action::{ActionType, CustomVerb, Reclassified, Refused, Requirement, Verb, JOIN, WRITE};
pub use attributes::{AclEntry, Grant, Memberships, NoMemberships, Secured, SecurityAttributes, Subject};
pub use context::SecurityContext;
pub use ownership::OwnershipPolicy;
