//! The built-in components that reach an external agent, one file per
//! component kind (issue 179). The facet trait, `ExternalAgent`, lives in
//! `crate::external`.

use crate::component::ComponentRegistry;

pub mod a2a_agent;

/// Registers the external agent kinds with `registry`, and `ExternalAgent`
/// as a facet of which an external personality has exactly one. It has no
/// default: a remote agent is always named, and a personality that names
/// none fails to build where its agent asks for it.
pub fn register_all(registry: &mut ComponentRegistry) {
    registry.single::<dyn crate::external::ExternalAgent>(None);

    a2a_agent::register(registry);
}
