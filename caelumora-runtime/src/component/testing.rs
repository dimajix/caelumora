//! What the tests of the module share: facets, kinds of components that
//! provide them, configurations and the context they are built with.
#![expect(clippy::unwrap_used, reason = "test code: what fails here fails the test that runs it")]

use super::{ComponentContext, ComponentError, ComponentFacets, ComponentKind, ComponentRegistry};
use crate::runtime::Services;
use caelumora_model::component::{ComponentConfig, ComponentDescriptor, ComponentFailPolicy};
use caelumora_model::security::{AccessPolicy, AllowAllPolicy, Policies, SecurityContext};
use caelumora_model::session::SessionId;
use caelumora_model::store::{EntityStores, Persistence};
use caelumora_model::type_map::TypeKey;
use serde_json::{json, Value};
use std::fmt::Debug;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub trait Marker: Debug + Send + Sync {}

/// What the tests' containers are asked for, as the owner of one asks.
pub const ASKED: &[TypeKey] =
    &[TypeKey::of::<dyn Marker>(), TypeKey::of::<dyn Named>(), TypeKey::of::<dyn Choice>()];

pub fn test_context() -> ComponentContext {
    context_with_session(SessionId::default())
}

pub fn context_with_session(session_id: SessionId) -> ComponentContext {
    let services = Services::empty().with(Arc::new(AllowAllPolicy::new()) as Arc<dyn AccessPolicy>);
    ComponentContext::new(SecurityContext { session_id, ..SecurityContext::default() }, services)
}

pub fn descriptor(id: &str, kind: &str, params: Value) -> ComponentDescriptor {
    ComponentDescriptor {
        id: id.to_string(),
        kind: kind.to_string(),
        params,
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailOpen,
    }
}

/// A facet that says which component it came from.
pub trait Named: Debug + Send + Sync {
    fn name(&self) -> String;
}

#[derive(Debug)]
pub struct Name(pub String);
impl Named for Name {
    fn name(&self) -> String {
        self.0.clone()
    }
}

/// A second facet, for components that need each other.
pub trait Other: Debug + Send + Sync {}
impl Other for Name {}

/// A facet of which there is one.
pub trait Choice: Debug + Send + Sync {}
impl Choice for Name {}

pub fn named(id: &str) -> ComponentFacets {
    let mut facets = ComponentFacets::empty();
    facets.insert::<dyn Named>(Arc::new(Name(id.to_string())));
    facets
}

pub fn config(nodes: &[(&str, &str)]) -> ComponentConfig {
    ComponentConfig {
        component_nodes: nodes.iter().map(|(id, kind)| descriptor(id, kind, json!(null))).collect(),
    }
}

/// The component errors from the outermost to the innermost.
pub fn causes(error: &ComponentError) -> Vec<&ComponentError> {
    let mut chain = vec![error];
    while let ComponentError::Build { cause, .. } = chain[chain.len() - 1] {
        chain.push(cause);
    }
    chain
}

pub const NAMED: &[TypeKey] = &[TypeKey::of::<dyn Named>()];

/// A registry with the single `Choice`, the kinds `declared` and `other`
/// that provide it, and how often `declared` was built.
pub fn choices() -> (ComponentRegistry, Arc<AtomicUsize>) {
    let mut registry = ComponentRegistry::new(
        EntityStores::open(&Persistence::in_memory(), &Policies::permitting_all())
            .unwrap()
            .component_bundles,
    );
    let built = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&built);
    let choice = |id: &str| {
        let mut facets = ComponentFacets::empty();
        facets.insert::<dyn Choice>(Arc::new(Name(id.to_string())));
        facets
    };
    registry.register(
        ComponentKind::new("declared").provides::<dyn Choice>(),
        move |descriptor, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(choice(&descriptor.id))
        },
    );
    registry
        .register(ComponentKind::new("other").provides::<dyn Choice>(), move |descriptor, _| {
            Ok(choice(&descriptor.id))
        });
    registry.register(ComponentKind::new("part").provides::<dyn Named>(), |descriptor, _| {
        Ok(named(&descriptor.id))
    });
    registry.single::<dyn Choice>(Some("declared"));
    (registry, built)
}
