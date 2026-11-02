//! Who may do what, and what may flow where (issues 11, 23, 304, 517): in whose name something is
//! done, what is asked, and who decides.
//!
//! - A `SecurityContext` (`context`) says who acts, who answers for it, with whose rights and for
//!   whom (issues 255, 341) - what every operation is asked in, and what the
//!   `ConfidentialityPolicy` clears.
//! - An `ActionType` (`action`) is what is asked: what is done with a stored entity of any kind, by
//!   a `Verb`, or one of the kernel's acts.
//! - An `Admission` (`admission`) is the policy's rule for a verb on entities, by their attributes
//!   alone: what it decides one entity by, and what a store narrows a list by (issue 332); the
//!   groups an access list names it learns from `Memberships` (`memberships`).
//! - The `AccessPolicy` (`access`) decides, one per process; the questions asked of it from
//!   outside are `AccessQuestions`, what an operation enforces is `Enforcement`. The policy of a
//!   process is the `OwnershipPolicy` (`ownership`), or the `AllowAllPolicy` for tests.
//! - The `ConfidentialityPolicy` (`confidentiality`) decides what may flow where and who is cleared
//!   for what, beside it; `ConfidentialityRules` are the rules Caelumora applies. Across sessions,
//!   the session read from has to permit it as well (`PrivacyPolicy`, `privacy`).
//! - `Policies` (`policies`) are the `AccessPolicy` and the `ConfidentialityPolicy` of a process as
//!   one pair, which whatever asks both holds (issue 449), as the configuration chooses them.
//! - The `SecretManager` (`secret`) reads the secrets of the process from its `SecretProvider`s,
//!   each read asked of the policy.
//!
//! What an entity carries of who may reach it (`SecurityAttributes`) and what a refusal says
//! (`Refused`) are the domain's (`caelumora_domain::security`).

mod access;
mod action;
mod admission;
pub mod confidentiality;
mod context;
mod memberships;
mod ownership;
mod policies;
mod privacy;
pub mod secret;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use access::{AccessPolicy, AccessQuestions, AllowAllPolicy, Enforcement};
pub use action::{ActionType, CustomVerb, Reclassified, Requirement, Verb, JOIN, WRITE};
pub use admission::Admission;
pub use context::SecurityContext;
pub use memberships::{Memberships, NoMemberships};
pub use ownership::OwnershipPolicy;
pub use policies::Policies;
