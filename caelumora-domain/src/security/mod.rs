//! What an entity carries of who may reach it, and what a refusal says (issues 11, 23, 304):
//! `SecurityAttributes` (`attributes`) - its owner and its access list of `Subject`s and their
//! `Grant`s - and `Refused` (`refused`), an action refused, as the error it ends in carries it. Who
//! acts, what is asked and who decides are `caelumora-security`'s (issue 517).

mod attributes;
mod refused;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use attributes::{AclEntry, Grant, Secured, SecuredMut, SecurityAttributes, Subject};
pub use refused::Refused;
