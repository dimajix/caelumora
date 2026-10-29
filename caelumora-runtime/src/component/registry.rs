//! The registry of component kinds (`ComponentRegistry`): what each kind
//! provides and needs, and how a configuration is checked against them
//! before a container builds it - its `ref`s resolved to the component
//! bundles they name (issue 455), its components on the defaults of the
//! singles (issue 445).

use super::container::Planned;
use super::{ComponentContext, ComponentError, ComponentResult};
use crate::{command, moderator};
use caelumora_model::component::{ComponentBundle, ComponentConfig, ComponentDescriptor};
use caelumora_model::scope::Scope;
use caelumora_model::security::SecurityContext;
use caelumora_model::store::EntityStore;
use caelumora_model::type_map::{TypeKey, TypeMap};
use caelumora_model::value::name::{Name, ScopedName};
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fmt::Debug;
use std::future;
use std::future::Future;
use std::sync::Arc;

/// The facets a component provides, by type.
pub type ComponentFacets = TypeMap;

/// A component kind as it is registered: its name, the facets it provides
/// and those of others it needs, whatever its parameters - which is how
/// components that provide a facet are found, and what a container accepts
/// is known, before anything is built. The container checks that the
/// factory kept to it.
#[derive(Debug, Clone)]
pub struct ComponentKind {
    name: String,
    provides: Vec<TypeKey>,
    uses: Vec<TypeKey>,
    needs: Vec<TypeKey>,
    operator_only: bool,
}

impl ComponentKind {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            provides: Vec::new(),
            uses: Vec::new(),
            needs: Vec::new(),
            operator_only: false,
        }
    }

    /// Declares that components of this kind are configured by an operator
    /// alone (issue 403): their params are trusted - where they send what
    /// they are given, what that is cleared for -, so they are written only
    /// where an operator writes, in the templates, personalities and
    /// component bundles of the seed. A request names one only
    /// through a `ref` (`require_acceptable`).
    pub fn operator_only(mut self) -> Self {
        self.operator_only = true;
        self
    }

    /// Declares that components of this kind provide `T`.
    pub fn provides<T: ?Sized + 'static>(mut self) -> Self {
        self.provides.push(TypeKey::of::<T>());
        self
    }

    /// Declares that components of this kind use the service `T` of the
    /// runtime: their factory is given it (`ComponentContext::service`), and
    /// none it does not declare (issue 296).
    pub fn uses<T: ?Sized + 'static>(mut self) -> Self {
        self.uses.push(TypeKey::of::<T>());
        self
    }

    /// Declares that components of this kind need the facet `T` of others:
    /// their factory may ask for it (`ComponentContext::all`, `one`), and
    /// for none it does not declare; and a container that accepts them
    /// accepts components that provide it (issue 337).
    pub fn needs<T: ?Sized + 'static>(mut self) -> Self {
        self.needs.push(TypeKey::of::<T>());
        self
    }
}

type ComponentFactory = Box<
    dyn Fn(
            ComponentDescriptor,
            ComponentContext,
        ) -> BoxFuture<'static, ComponentResult<ComponentFacets>>
        + Send
        + Sync,
>;

/// A registered kind.
pub(super) struct Registration {
    provides: Vec<TypeKey>,
    pub(super) uses: Vec<TypeKey>,
    pub(super) needs: Vec<TypeKey>,
    operator_only: bool,
    factory: ComponentFactory,
}

impl Registration {
    fn of(kind: ComponentKind, factory: ComponentFactory) -> (String, Self) {
        let ComponentKind { name, provides, uses, needs, operator_only } = kind;
        (name, Self { provides, uses, needs, operator_only, factory })
    }
}

/// A facet of which there is exactly one per container, with the kind that
/// is built if no component of the container provides it - if there is one
/// that makes sense.
#[derive(Debug, Clone)]
struct Single {
    facet: TypeKey,
    default_kind: Option<String>,
}

/// Reserved `kind` that stands for the components of a bundle (`ComponentBundle`, issue 455)
/// instead of invoking a factory. `params` must deserialize to `{ "name": "<its name>" }`, with
/// `"scope"` beside it where the bundle lies in another than the global one.
pub const BUNDLE_REF_KIND: &str = "ref";

/// The bundle a `ref` names: its name, in exactly its scope.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleRef {
    name: Name,
    #[serde(default)]
    scope: Scope,
}

/// Reserved `kind` that builds on a component default (issue 445): one of
/// `AppConfig::component_defaults`, or of the defaults a container was built
/// with. `params` must deserialize to `{ "id": "<the default's id>",
/// "params": { ... } }`; the component is the default's kind, with its params
/// and those given over them.
pub const COMPONENT_DEFAULT_KIND: &str = "default";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DefaultRef {
    id: String,
    #[serde(default)]
    params: Value,
}

pub struct ComponentRegistry {
    kinds: HashMap<String, Registration>,
    singles: HashMap<TypeId, Single>,
    /// The default kinds of singles the program named (`set_default_kind`),
    /// in place of those they were declared with - kept apart, so that a
    /// plugin that declares a single after the program named its default
    /// does not undo the program's choice.
    chosen: HashMap<TypeId, String>,
    /// The configuration's defaults for singles (`AppConfig::component_defaults`),
    /// by the single they provide.
    defaults: HashMap<TypeId, ComponentDescriptor>,
    bundles: Arc<dyn EntityStore<ComponentBundle>>,
}

impl Debug for ComponentRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComponentRegistry")
            .field("kind_count", &self.kinds.len())
            .field("singles", &self.singles.values().map(|s| s.facet.name()).collect::<Vec<_>>())
            .finish()
    }
}

impl ComponentRegistry {
    /// A registry without kinds, resolving the bundles a `ref` names in
    /// `bundles`.
    pub fn new(bundles: Arc<dyn EntityStore<ComponentBundle>>) -> Self {
        Self {
            kinds: HashMap::new(),
            singles: HashMap::new(),
            chosen: HashMap::new(),
            defaults: HashMap::new(),
            bundles,
        }
    }

    /// A registry with the runtime's own component kinds - the user
    /// commands (`command`) and a channel's moderation (`moderator`) -,
    /// resolving the bundles a `ref` names in `bundles`. The kinds of an
    /// agent's components are the agents' plugin's
    /// (`caelumora_agents::plugin::AgentsPlugin`, issue 370), those of other
    /// plugins theirs.
    /// A binary passes it to the `RuntimeBuilder` as it is; a test may
    /// register kinds of its own first, such as a stand-in for the model.
    pub fn built_in(bundles: Arc<dyn EntityStore<ComponentBundle>>) -> Self {
        let mut registry = Self::new(bundles);
        command::register_all(&mut registry);
        moderator::register_all(&mut registry);
        registry
    }

    /// Registers `kind` with a factory that builds without waiting.
    pub fn register<F>(&mut self, kind: ComponentKind, factory: F)
    where
        F: Fn(&ComponentDescriptor, &ComponentContext) -> ComponentResult<ComponentFacets>
            + Send
            + Sync
            + 'static,
    {
        let factory: ComponentFactory = Box::new(move |descriptor, context| {
            let built = factory(&descriptor, &context);
            Box::pin(future::ready(built))
        });
        let (name, registration) = Registration::of(kind, factory);
        self.kinds.insert(name, registration);
    }

    /// Registers `kind` with a factory that waits: for a remote system, or
    /// for other components (`ComponentContext::all`, `one`).
    pub fn register_async<F, Fut>(&mut self, kind: ComponentKind, factory: F)
    where
        F: Fn(ComponentDescriptor, ComponentContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ComponentResult<ComponentFacets>> + Send + 'static,
    {
        let factory: ComponentFactory =
            Box::new(move |descriptor, context| Box::pin(factory(descriptor, context)));
        let (name, registration) = Registration::of(kind, factory);
        self.kinds.insert(name, registration);
    }

    /// Makes `T` a facet of which there is exactly one per container, built
    /// from `default_kind` - unless the configuration names another default
    /// (`set_defaults`) - if no component of the container provides it. With
    /// no default, a container that is asked for `T` and provides none fails
    /// to build it (issue 179: a remote agent has no default). Declared
    /// again - by a plugin registered into a registry the program prepared,
    /// say - the declaration replaces the one before, not the default kind
    /// the program named (`set_default_kind`).
    pub fn single<T: ?Sized + 'static>(&mut self, default_kind: Option<&str>) {
        let facet = TypeKey::of::<T>();
        self.singles
            .insert(facet.id, Single { facet, default_kind: default_kind.map(str::to_string) });
    }

    /// Makes `kind` the default kind of the single `T`, in place of the one
    /// it was declared with - for the program, which knows which kinds its
    /// plugins brought (issue 329: the model). The configuration's defaults
    /// (`set_defaults`) still replace it.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` if `T` is no facet of which there is
    /// one, or `kind` is not registered or does not provide `T`.
    pub fn set_default_kind<T: ?Sized + 'static>(&mut self, kind: &str) -> ComponentResult<()> {
        let facet = TypeKey::of::<T>();
        if !self.provides(kind).is_some_and(|facets| facets.iter().any(|f| f.id == facet.id)) {
            return Err(ComponentError::InvalidConfig(format!(
                "the default kind '{kind}' of {} is not registered as providing it",
                facet.name()
            )));
        }
        if !self.singles.contains_key(&facet.id) {
            return Err(ComponentError::InvalidConfig(format!(
                "{} is not a facet of which there is one",
                facet.name()
            )));
        }
        self.chosen.insert(facet.id, kind.to_string());
        Ok(())
    }

    /// Takes the configuration's defaults (`AppConfig::component_defaults`):
    /// each of its components replaces the default kind of the singles it
    /// provides, for every container - a model, of its `LLMClient` and of
    /// its profile alike, built once for both.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` naming the component if one is a
    /// `ref` or builds on a default, has an id another of them has too, has
    /// an unknown kind, provides no single, or provides one that another of
    /// them provides too; nothing is taken then.
    pub fn set_defaults(&mut self, config: &ComponentConfig) -> ComponentResult<()> {
        self.defaults = self.defaults_of(config)?;
        Ok(())
    }

    /// The components of `config` as defaults of singles, by the single each
    /// provides - for the whole registry (`set_defaults`) or one container
    /// (`ComponentContainer::with_defaults`).
    ///
    /// # Errors
    /// As `set_defaults`.
    pub(super) fn defaults_of(
        &self,
        config: &ComponentConfig,
    ) -> ComponentResult<HashMap<TypeId, ComponentDescriptor>> {
        let mut defaults: HashMap<TypeId, ComponentDescriptor> = HashMap::new();
        // By its id a component builds on a default (`COMPONENT_DEFAULT_KIND`).
        let mut ids = HashSet::new();
        for node in &config.component_nodes {
            if node.kind == BUNDLE_REF_KIND {
                return Err(ComponentError::InvalidConfig(format!(
                    "component default '{}' cannot be a ref",
                    node.id
                )));
            }
            if node.kind == COMPONENT_DEFAULT_KIND {
                return Err(ComponentError::InvalidConfig(format!(
                    "component default '{}' cannot build on a default",
                    node.id
                )));
            }
            if !ids.insert(node.id.as_str()) {
                return Err(ComponentError::InvalidConfig(format!(
                    "two component defaults have the id '{}'",
                    node.id
                )));
            }
            let registration = self.kinds.get(&node.kind).ok_or_else(|| {
                ComponentError::InvalidConfig(format!(
                    "component default '{}' has the unknown kind '{}'",
                    node.id, node.kind
                ))
            })?;
            let singles: Vec<&Single> =
                registration.provides.iter().filter_map(|f| self.singles.get(&f.id)).collect();
            if singles.is_empty() {
                return Err(ComponentError::InvalidConfig(format!(
                    "component default '{}' ({}) provides no facet of which there is one per agent",
                    node.id, node.kind
                )));
            }
            for single in singles {
                if let Some(other) = defaults.insert(single.facet.id, node.clone()) {
                    return Err(ComponentError::InvalidConfig(format!(
                        "component defaults '{}' and '{}' both provide {}",
                        other.id,
                        node.id,
                        single.facet.name()
                    )));
                }
            }
        }
        Ok(defaults)
    }

    /// The kind of the component of `config` that provides `T`, a facet of
    /// which there is one - or else of the default built for it - as
    /// configured, without building anything (issue 174); of one that builds
    /// on a default, the default's. `None` if `T` is no such facet, or if
    /// `config` refers to a bundle (`ref`) and none of its own provides `T`:
    /// what that one provides only the bundle in the store tells.
    pub fn kind_of<T: ?Sized + 'static>(&self, config: &ComponentConfig) -> Option<String> {
        let facet = TypeKey::of::<T>();
        if !self.is_single(facet.id) {
            return None;
        }
        let provides = |kind: &str| {
            self.provides(kind).is_some_and(|facets| facets.iter().any(|f| f.id == facet.id))
        };
        let kind_of = |node: &ComponentDescriptor| match node.kind.as_str() {
            COMPONENT_DEFAULT_KIND => {
                self.on_default(node, &HashMap::new()).ok().map(|built| built.kind)
            }
            kind => Some(kind.to_string()),
        };
        if let Some(kind) =
            config.component_nodes.iter().filter_map(kind_of).find(|kind| provides(kind))
        {
            return Some(kind);
        }
        if config.component_nodes.iter().any(|node| node.kind == BUNDLE_REF_KIND) {
            return None;
        }
        self.default_of(facet.id).map(|descriptor| descriptor.kind)
    }

    /// Whether `facet` is a facet of which there is one (`single`).
    pub(super) fn is_single(&self, facet: TypeId) -> bool {
        self.singles.contains_key(&facet)
    }

    /// The facets `kind` provides; `None` for a kind that is not registered.
    pub(super) fn provides(&self, kind: &str) -> Option<&[TypeKey]> {
        self.kinds.get(kind).map(|registration| registration.provides.as_slice())
    }

    /// The facets of others `kind` needs; none for a kind that is not
    /// registered.
    fn needs(&self, kind: &str) -> &[TypeKey] {
        self.kinds.get(kind).map_or(&[], |registration| registration.needs.as_slice())
    }

    /// What is built for the single `facet` if no component of a container
    /// provides it: the configuration's default, or else the default kind
    /// the program named, or else the one it was declared with, if it has
    /// one.
    fn default_of(&self, facet: TypeId) -> Option<ComponentDescriptor> {
        if let Some(descriptor) = self.defaults.get(&facet) {
            return Some(descriptor.clone());
        }
        let declared = self.singles.get(&facet)?.default_kind.as_deref();
        let kind = self.chosen.get(&facet).map(String::as_str).or(declared)?;
        Some(ComponentDescriptor::new(kind, kind))
    }

    /// Whether a container built for an owner that asks for the facets
    /// `asked` would accept `config`, its `ref`s resolved in the name
    /// `context`: for whoever writes a configuration in a request, before
    /// the verb (issue 337), and builds it in someone else's name - the
    /// components of a channel, built in its owner's (issue 321) -, so that
    /// they name only what they may read themselves, and a kind an operator
    /// configures alone only through a `ref` (issue 403). Nothing is built.
    ///
    /// # Errors
    /// As `ComponentContainer::from_config` before it builds anything:
    /// `ComponentError::Bundle`, wrapped with the `ref`, for one `context`
    /// may not read or whose store fails; `ComponentError::DuplicateId`,
    /// `ComponentError::UnsupportedKind`, `ComponentError::InvalidConfig` for
    /// two components of a single or one nobody asks for; and
    /// `ComponentError::OperatorOnly`, wrapped with the component, for one
    /// of a kind only an operator configures, written in `config` itself.
    pub async fn require_acceptable(
        &self,
        config: &ComponentConfig,
        asked: &[TypeKey],
        context: &SecurityContext,
    ) -> ComponentResult<()> {
        let planned = self.plan(config, &HashMap::new(), context).await?;
        if let Some(written) = planned.iter().find(|p| {
            p.brought_by.is_none()
                && self.kinds.get(&p.descriptor.kind).is_some_and(|r| r.operator_only)
        }) {
            return Err(ComponentError::of_component(
                &written.descriptor,
                ComponentError::OperatorOnly(written.descriptor.kind.clone()),
            ));
        }
        self.require_asked(&planned, &HashMap::new(), asked)
    }

    /// The components of `config`, its `ref`s resolved in the name `context` and those that build
    /// on a default resolved among `own_defaults` and the registry's, with the facets their kinds
    /// provide - checked, but not built.
    ///
    /// # Errors
    /// `ComponentError::DuplicateId` if two entries of `config` share an id,
    /// or two of the components they stand for do - wrapped with the `ref`
    /// that brings the second -; `ComponentError::UnsupportedKind` for a kind
    /// that is not registered, `ComponentError::InvalidConfig` if two provide
    /// a facet of which there is one; whatever resolving an entry reports,
    /// wrapped with it.
    pub(super) async fn plan(
        &self,
        config: &ComponentConfig,
        own_defaults: &HashMap<TypeId, ComponentDescriptor>,
        context: &SecurityContext,
    ) -> ComponentResult<Vec<Planned>> {
        let mut entries = HashSet::new();
        // The entry of `config` each component's id came from.
        let mut ids: HashMap<String, &ComponentDescriptor> = HashMap::new();
        let mut planned = Vec::with_capacity(config.component_nodes.len());
        for node in &config.component_nodes {
            if !entries.insert(node.id.as_str()) {
                return Err(ComponentError::DuplicateId(node.id.clone()));
            }
            let brought_by = (node.kind == BUNDLE_REF_KIND).then(|| node.clone());
            for descriptor in self
                .resolve(node, own_defaults, context)
                .await
                .map_err(|cause| ComponentError::of_component(node, cause))?
            {
                if let Some(earlier) = ids.insert(descriptor.id.clone(), node) {
                    // Two plain entries of one id are caught above: at least
                    // one of the two is a ref, and it is named.
                    let entry = if node.kind == BUNDLE_REF_KIND { node } else { earlier };
                    return Err(ComponentError::of_component(
                        entry,
                        ComponentError::DuplicateId(descriptor.id),
                    ));
                }
                let provides = self
                    .provides(&descriptor.kind)
                    .ok_or_else(|| {
                        ComponentError::of_component(
                            node,
                            ComponentError::UnsupportedKind(descriptor.kind.clone()),
                        )
                    })?
                    .to_vec();
                planned.push(Planned { descriptor, provides, brought_by: brought_by.clone() });
            }
        }
        for single in self.singles.values() {
            let providers: Vec<&str> = planned
                .iter()
                .filter(|p| p.provides.iter().any(|f| f.id == single.facet.id))
                .map(|p| p.descriptor.id.as_str())
                .collect();
            if let [first, second, ..] = providers.as_slice() {
                return Err(ComponentError::InvalidConfig(format!(
                    "components '{first}' and '{second}' both provide {}, of which there is one",
                    single.facet.name()
                )));
            }
        }
        Ok(planned)
    }

    /// Whether every component of `planned` provides a facet of the closure
    /// of `asked` (`closure`): one that provides none would be built for
    /// nothing, reading its owner's secrets (issue 337).
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig`, wrapped with its entry, for the
    /// first that provides none.
    pub(super) fn require_asked(
        &self,
        planned: &[Planned],
        own_defaults: &HashMap<TypeId, ComponentDescriptor>,
        asked: &[TypeKey],
    ) -> ComponentResult<()> {
        let closure = self.closure(planned, own_defaults, asked);
        match planned
            .iter()
            .find(|p| !p.provides.iter().any(|f| closure.iter().any(|c| c.id == f.id)))
        {
            None => Ok(()),
            Some(stray) => {
                let provided =
                    stray.provides.iter().map(TypeKey::name).collect::<Vec<_>>().join(", ");
                Err(stray.failed(&format!(
                    "provides {provided}, which nothing asks for where it is configured"
                )))
            }
        }
    }

    /// The facets the components of `planned` may be asked for: `asked`,
    /// and whatever the components that provide one of them - planned, or
    /// the default of a single none provides - need in turn, until nothing
    /// is added (issue 337).
    fn closure(
        &self,
        planned: &[Planned],
        own_defaults: &HashMap<TypeId, ComponentDescriptor>,
        asked: &[TypeKey],
    ) -> Vec<TypeKey> {
        let mut closure = asked.to_vec();
        let mut next = 0;
        while let Some(facet) = closure.get(next).copied() {
            next += 1;
            let mut kinds: Vec<String> = planned
                .iter()
                .filter(|p| p.provides.iter().any(|f| f.id == facet.id))
                .map(|p| p.descriptor.kind.clone())
                .collect();
            if kinds.is_empty() {
                kinds.extend(
                    self.default_for(facet.id, own_defaults).map(|descriptor| descriptor.kind),
                );
            }
            for need in kinds.iter().flat_map(|kind| self.needs(kind)) {
                if !closure.iter().any(|f| f.id == need.id) {
                    closure.push(*need);
                }
            }
        }
        closure
    }

    /// What is built for the single `facet` if no component of a container
    /// with the defaults `own_defaults` provides it: its own default, or
    /// else the registry's (`default_of`).
    pub(super) fn default_for(
        &self,
        facet: TypeId,
        own_defaults: &HashMap<TypeId, ComponentDescriptor>,
    ) -> Option<ComponentDescriptor> {
        own_defaults.get(&facet).cloned().or_else(|| self.default_of(facet))
    }

    /// `node` itself; for `kind: "default"` the default it builds on
    /// (`on_default`); or - for `kind: "ref"` - the components of the bundle
    /// it names, in their order (issues 270, 455), each built on its default
    /// as `node` would be, found by its name fresh on every call, in the
    /// name `context` of whichever component graph is referencing it.
    /// This ensures things like secret resolution run under the actual
    /// caller's name rather than a synthetic one, at the cost of not sharing
    /// a single instance across references (e.g. an `mcp_tool` of a bundle
    /// re-runs discovery for every personality that references it).
    async fn resolve(
        &self,
        node: &ComponentDescriptor,
        own_defaults: &HashMap<TypeId, ComponentDescriptor>,
        context: &SecurityContext,
    ) -> ComponentResult<Vec<ComponentDescriptor>> {
        if node.kind == COMPONENT_DEFAULT_KIND {
            return Ok(vec![self.on_default(node, own_defaults)?]);
        }
        if node.kind != BUNDLE_REF_KIND {
            return Ok(vec![node.clone()]);
        }
        let target: BundleRef = serde_json::from_value(node.params.clone())
            .map_err(|e| ComponentError::InvalidConfig(e.to_string()))?;
        let name = ScopedName { name: target.name, scope: target.scope };
        let components = self
            .bundles
            .find_by_name(context, &name)
            .await
            .map_err(|cause| ComponentError::Bundle { name: name.to_string(), cause })?
            .components;
        if let Some(inner) =
            components.component_nodes.iter().find(|inner| inner.kind == BUNDLE_REF_KIND)
        {
            return Err(ComponentError::InvalidConfig(format!(
                "component bundle '{name}' cannot include a ref ('{}')",
                inner.id
            )));
        }
        components
            .component_nodes
            .into_iter()
            .map(|inner| match inner.kind.as_str() {
                COMPONENT_DEFAULT_KIND => self
                    .on_default(&inner, own_defaults)
                    .map_err(|cause| ComponentError::of_component(&inner, cause)),
                _ => Ok(inner),
            })
            .collect()
    }

    /// `node`, of `kind: "default"`, as the component default it names -
    /// the container's own (`own_defaults`), or else the configuration's -
    /// with the params it gives over the default's: each replaces the
    /// default's of its name whole, so that nothing of the default's stays
    /// beside what it changes - an `endpoint` given takes none of the
    /// default's `auth` along (issue 445). Its kind is the default's; its id,
    /// tags and fail policy are its own.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` if its params are not of that form,
    /// there is no default of that id, or the default's or its own params
    /// are no object.
    fn on_default(
        &self,
        node: &ComponentDescriptor,
        own_defaults: &HashMap<TypeId, ComponentDescriptor>,
    ) -> ComponentResult<ComponentDescriptor> {
        let target: DefaultRef = serde_json::from_value(node.params.clone())
            .map_err(|e| ComponentError::InvalidConfig(e.to_string()))?;
        let base = own_defaults
            .values()
            .chain(self.defaults.values())
            .find(|default| default.id == target.id)
            .ok_or_else(|| {
                ComponentError::InvalidConfig(format!(
                    "there is no component default '{}'",
                    target.id
                ))
            })?;
        let mut params = match &base.params {
            Value::Object(params) => params.clone(),
            Value::Null => Map::new(),
            _ => {
                return Err(ComponentError::InvalidConfig(format!(
                    "the params of component default '{}' are no object",
                    base.id
                )))
            }
        };
        match target.params {
            Value::Object(own) => params.extend(own),
            Value::Null => {}
            _ => {
                return Err(ComponentError::InvalidConfig(format!(
                    "the params given over component default '{}' are no object",
                    base.id
                )))
            }
        }
        Ok(ComponentDescriptor {
            kind: base.kind.clone(),
            params: Value::Object(params),
            ..node.clone()
        })
    }

    pub(super) fn dispatch(
        &self,
        node: ComponentDescriptor,
        context: ComponentContext,
    ) -> ComponentResult<BoxFuture<'static, ComponentResult<ComponentFacets>>> {
        let Some(registration) = self.kinds.get(&node.kind) else {
            return Err(ComponentError::UnsupportedKind(node.kind.clone()));
        };
        Ok((registration.factory)(node, context.for_kind(registration)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::testing::*;
    use crate::component::ComponentContainer;
    use async_trait::async_trait;
    use caelumora_model::component::{ComponentBundle, ComponentBundleId, ComponentFailPolicy};
    use caelumora_model::removal::{Removal, RemovalId};
    use caelumora_model::scope::ScopeField;
    use caelumora_model::session::SessionId;
    use caelumora_model::store::query::{Page, Query};
    use caelumora_model::store::{Deleted, Persistence, StoreError, StoreResult};
    use caelumora_model::user::UserId;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[derive(Debug)]
    struct MarkerImpl;
    impl Marker for MarkerImpl {}

    /// A store holding the global bundles `name`, each the list `nodes`.
    async fn store_with(
        bundles: &[(&str, &[(&str, &str)])],
    ) -> Arc<dyn EntityStore<ComponentBundle>> {
        let store = Persistence::in_memory().entity_stores().unwrap().component_bundles;
        for (name, nodes) in bundles {
            store.create(&SecurityContext::default(), &bundle(name, config(nodes))).await.unwrap();
        }
        store
    }

    /// The global bundle `name` of `components`.
    fn bundle(name: &str, components: ComponentConfig) -> ComponentBundle {
        ComponentBundle::new(
            ScopedName::new(name, Scope::GlobalScope).unwrap(),
            components,
            Default::default(),
        )
    }

    fn reference(id: &str, target: &str) -> ComponentDescriptor {
        descriptor(id, BUNDLE_REF_KIND, json!({ "name": target }))
    }

    #[tokio::test]
    async fn ref_kind_builds_fresh_using_the_callers_context_each_time() {
        let build_count = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&build_count);
        let seen_sessions: Arc<Mutex<Vec<SessionId>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen_sessions);

        let store = store_with(&[("shared-marker", &[("marker", "counter")])]).await;
        let mut registry = ComponentRegistry::new(store);
        registry.register(
            ComponentKind::new("counter").provides::<dyn Marker>(),
            move |_node, context| {
                counted.fetch_add(1, Ordering::SeqCst);
                recorded.lock().unwrap().push(context.security_context.session_id);
                let mut facets = ComponentFacets::empty();
                facets.insert::<dyn Marker>(Arc::new(MarkerImpl));
                Ok(facets)
            },
        );
        let registry = Arc::new(registry);
        let config = ComponentConfig { component_nodes: vec![reference("ref", "shared-marker")] };

        let session_a = SessionId::new();
        let session_b = SessionId::new();
        let container_a = ComponentContainer::from_config(
            &config,
            ASKED,
            &registry,
            &context_with_session(session_a),
        )
        .await
        .unwrap();
        let container_b = ComponentContainer::from_config(
            &config,
            ASKED,
            &registry,
            &context_with_session(session_b),
        )
        .await
        .unwrap();

        assert_eq!(
            build_count.load(Ordering::SeqCst),
            2,
            "each ref must build fresh, not reuse a cached instance"
        );
        assert_eq!(
            *seen_sessions.lock().unwrap(),
            vec![session_a, session_b],
            "the factory must see the caller's own context, not a synthetic one"
        );
        assert!(!Arc::ptr_eq(
            &container_a.get_all::<dyn Marker>(None).unwrap()[0].facet,
            &container_b.get_all::<dyn Marker>(None).unwrap()[0].facet
        ));
    }

    /// A registry of the kind `named`, whose components are named by their
    /// id, resolving references in `store`.
    fn named_registry(store: Arc<dyn EntityStore<ComponentBundle>>) -> Arc<ComponentRegistry> {
        let mut registry = ComponentRegistry::new(store);
        registry.register(ComponentKind::new("named").provides::<dyn Named>(), |node, _| {
            Ok(named(&node.id))
        });
        Arc::new(registry)
    }

    fn names(container: &ComponentContainer) -> Vec<String> {
        container.get_all::<dyn Named>(None).unwrap().iter().map(|c| c.name()).collect()
    }

    #[tokio::test]
    async fn a_ref_stands_for_its_components_in_their_order_where_it_is_written() {
        let store = store_with(&[("basics", &[("stop", "named"), ("notes", "named")])]).await;
        let registry = named_registry(store);
        let config = ComponentConfig {
            component_nodes: vec![
                descriptor("prompt", "named", json!(null)),
                reference("basics", "basics"),
                descriptor("memory", "named", json!(null)),
            ],
        };

        let container = ComponentContainer::from_config(&config, ASKED, &registry, &test_context())
            .await
            .unwrap();
        assert_eq!(names(&container), ["prompt", "stop", "notes", "memory"]);
        let built_from: Vec<String> =
            container.components().component_nodes.into_iter().map(|node| node.id).collect();
        assert_eq!(
            built_from,
            ["prompt", "stop", "notes", "memory"],
            "what it was built from, resolved"
        );
    }

    #[tokio::test]
    async fn a_component_brought_twice_through_a_ref_is_refused_with_the_ref_named() {
        let store = store_with(&[
            ("basics", &[("stop", "named"), ("notes", "named")]),
            ("more", &[("notes", "named")]),
        ])
        .await;
        let registry = named_registry(store);
        let notes = || descriptor("notes", "named", json!(null));

        for (nodes, named) in [
            (vec![reference("basics", "basics"), notes()], "basics"),
            (vec![notes(), reference("basics", "basics")], "basics"),
            (vec![reference("basics", "basics"), reference("more", "more")], "more"),
            (vec![reference("basics", "basics"), reference("again", "basics")], "again"),
        ] {
            let error = ComponentContainer::from_config(
                &ComponentConfig { component_nodes: nodes },
                ASKED,
                &registry,
                &test_context(),
            )
            .await
            .unwrap_err();
            assert!(
                matches!(&error, ComponentError::Build { component, cause, .. } if component == named && matches!(cause.as_ref(), ComponentError::DuplicateId(_))),
                "{error}"
            );
        }
    }

    #[tokio::test]
    async fn a_ref_to_nothing_stored_is_refused_as_unavailable() {
        let registry =
            named_registry(Persistence::in_memory().entity_stores().unwrap().component_bundles);
        let config =
            ComponentConfig { component_nodes: vec![reference("ref-a", "does-not-exist")] };
        let error = ComponentContainer::from_config(&config, ASKED, &registry, &test_context())
            .await
            .unwrap_err();
        assert!(
            matches!(
                causes(&error)[..],
                [ComponentError::Build { .. }, ComponentError::Bundle { .. }]
            ),
            "{error}"
        );
        assert!(error.is_unavailable(), "through the component it was building");
        assert!(
            error.to_string().contains("'does-not-exist'"),
            "names what it could not read: {error}"
        );
    }

    /// A store of bundles that fails whatever it is asked.
    #[derive(Debug)]
    struct Failing;

    impl Failing {
        fn failed<T>() -> StoreResult<T> {
            Err(StoreError::Backend("unreachable".to_string()))
        }
    }

    #[async_trait]
    impl EntityStore<ComponentBundle> for Failing {
        async fn create(&self, _: &SecurityContext, _: &ComponentBundle) -> StoreResult<()> {
            Self::failed()
        }
        async fn get(
            &self,
            _: &SecurityContext,
            _: ComponentBundleId,
        ) -> StoreResult<ComponentBundle> {
            Self::failed()
        }
        async fn update(&self, _: &SecurityContext, _: &ComponentBundle) -> StoreResult<()> {
            Self::failed()
        }
        async fn delete(
            &self,
            _: &SecurityContext,
            _: ComponentBundleId,
            _: &Removal,
        ) -> StoreResult<bool> {
            Self::failed()
        }
        async fn list(
            &self,
            _: &SecurityContext,
            _: &Query<ScopeField>,
        ) -> StoreResult<Page<ComponentBundle>> {
            Self::failed()
        }
        async fn restore(
            &self,
            _: &SecurityContext,
            _: ComponentBundleId,
        ) -> StoreResult<ComponentBundle> {
            Self::failed()
        }
        async fn purge(&self, _: &SecurityContext, _: ComponentBundleId) -> StoreResult<bool> {
            Self::failed()
        }
        async fn get_deleted(
            &self,
            _: &SecurityContext,
            _: ComponentBundleId,
        ) -> StoreResult<Deleted<ComponentBundle>> {
            Self::failed()
        }
        async fn list_deleted(
            &self,
            _: &SecurityContext,
            _: &Query<ScopeField>,
        ) -> StoreResult<Page<Deleted<ComponentBundle>>> {
            Self::failed()
        }
        async fn list_deleted_with(
            &self,
            _: &SecurityContext,
            _: RemovalId,
            _: &Query<ScopeField>,
        ) -> StoreResult<Page<ComponentBundle>> {
            Self::failed()
        }
        async fn find_by_name(
            &self,
            _: &SecurityContext,
            _: &ScopedName,
        ) -> StoreResult<ComponentBundle> {
            Self::failed()
        }
    }

    /// A store that fails while a `ref`'s bundle is looked for is told as
    /// the store's failure, not as a bundle missing - no reason to leave
    /// anyone out, as a bundle gone is (issue 457).
    #[tokio::test]
    async fn a_ref_whose_store_fails_is_told_as_the_stores_failure() {
        let registry = named_registry(Arc::new(Failing));
        let config = ComponentConfig { component_nodes: vec![reference("ref-a", "tools")] };
        let error = ComponentContainer::from_config(&config, ASKED, &registry, &test_context())
            .await
            .unwrap_err();
        assert!(
            matches!(
                causes(&error)[..],
                [
                    ComponentError::Build { .. },
                    ComponentError::Bundle { cause: StoreError::Backend(_), .. }
                ]
            ),
            "{error}"
        );
        assert!(!error.is_unavailable(), "the store failed");
    }

    /// A `ref` names a bundle by its name in exactly its scope, the global
    /// one unless it names another (issue 455), and by nothing else.
    #[tokio::test]
    async fn a_ref_names_a_bundle_in_exactly_its_scope() {
        let store = Persistence::in_memory().entity_stores().unwrap().component_bundles;
        let user_id = UserId::new();
        let own = ComponentBundle::new(
            ScopedName::new("mine", Scope::UserScope { user_id }).unwrap(),
            config(&[("stop", "named")]),
            Default::default(),
        );
        store.create(&SecurityContext::default(), &own).await.unwrap();
        let registry = named_registry(store);
        let built = |params: Value| {
            let config = ComponentConfig {
                component_nodes: vec![descriptor("ref", BUNDLE_REF_KIND, params)],
            };
            let registry = Arc::clone(&registry);
            async move {
                ComponentContainer::from_config(&config, ASKED, &registry, &test_context()).await
            }
        };

        let found =
            built(json!({ "name": "mine", "scope": { "UserScope": { "user_id": user_id } } }))
                .await
                .unwrap();
        assert_eq!(names(&found), ["stop"], "in the scope it names");
        let global = built(json!({ "name": "mine" })).await.unwrap_err();
        assert!(
            global.is_unavailable(),
            "not looked for elsewhere than the global scope: {global}"
        );
        let by_id = built(json!({ "id": own.id })).await.unwrap_err();
        assert!(
            matches!(
                causes(&by_id)[..],
                [ComponentError::Build { .. }, ComponentError::InvalidConfig(_)]
            ),
            "nor by its id: {by_id}"
        );
    }

    #[tokio::test]
    async fn a_bundle_cannot_include_a_ref() {
        let store = Persistence::in_memory().entity_stores().unwrap().component_bundles;
        let nested = ComponentConfig {
            component_nodes: vec![
                descriptor("plain", "named", json!(null)),
                reference("inner", "b"),
            ],
        };
        store.create(&SecurityContext::default(), &bundle("a", nested)).await.unwrap();
        let registry = named_registry(store);
        let config = ComponentConfig { component_nodes: vec![reference("ref-a", "a")] };
        let error = ComponentContainer::from_config(&config, ASKED, &registry, &test_context())
            .await
            .unwrap_err();
        assert!(
            matches!(
                causes(&error)[..],
                [ComponentError::Build { .. }, ComponentError::InvalidConfig(_)]
            ),
            "{error}"
        );
        assert!(error.to_string().contains("'inner'"), "names the ref within: {error}");
    }

    /// A registry of `part`, providing `Named` and counting how often it is
    /// built; `stray`, providing `Other`; `picky`, providing `Named` and
    /// needing `Choice`; and the single `Choice`, whose default kind
    /// `choosing` needs `Other`, and `other`, which needs nothing.
    fn needing() -> (Arc<ComponentRegistry>, Arc<AtomicUsize>) {
        let mut registry = ComponentRegistry::new(
            Persistence::in_memory().entity_stores().unwrap().component_bundles,
        );
        let built = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&built);
        let choice = |id: &str| {
            ComponentFacets::empty().with(Arc::new(Name(id.to_string())) as Arc<dyn Choice>)
        };
        registry.register(
            ComponentKind::new("part").provides::<dyn Named>(),
            move |descriptor, _| {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(named(&descriptor.id))
            },
        );
        registry.register(ComponentKind::new("stray").provides::<dyn Other>(), |descriptor, _| {
            Ok(ComponentFacets::empty()
                .with(Arc::new(Name(descriptor.id.clone())) as Arc<dyn Other>))
        });
        registry.register_async(
            ComponentKind::new("picky").provides::<dyn Named>().needs::<dyn Choice>(),
            |descriptor, context| async move {
                context.one::<dyn Choice>().await?;
                Ok(named(&descriptor.id))
            },
        );
        registry.register_async(
            ComponentKind::new("choosing").provides::<dyn Choice>().needs::<dyn Other>(),
            move |descriptor, context| async move {
                context.all::<dyn Other>().await?;
                Ok(choice(&descriptor.id))
            },
        );
        registry.register(
            ComponentKind::new("other").provides::<dyn Choice>(),
            move |descriptor, _| Ok(choice(&descriptor.id)),
        );
        registry.single::<dyn Choice>(Some("choosing"));
        (Arc::new(registry), built)
    }

    /// A component that provides nothing its owner asks for, nor anything
    /// a component it asks for needs, is refused before anything is built
    /// (issue 337) - named by the entry that brings it, a `ref` too.
    #[tokio::test]
    async fn a_component_nobody_asks_for_is_refused_before_anything_is_built() {
        let (registry, built) = needing();
        let error = ComponentContainer::from_config(
            &config(&[("a", "part"), ("tracker", "stray")]),
            NAMED,
            &registry,
            &test_context(),
        )
        .await
        .expect_err("nobody asks for Other");
        assert!(
            matches!(&error, ComponentError::Build { component, .. } if component == "tracker"),
            "{error}"
        );
        assert!(
            error
                .to_string()
                .contains("it provides Other, which nothing asks for where it is configured"),
            "{error}"
        );
        assert_eq!(built.load(Ordering::SeqCst), 0, "nothing is built");

        let store = store_with(&[("shared", &[("tracker", "stray")])]).await;
        let mut through_ref = ComponentRegistry::new(store);
        through_ref.register(ComponentKind::new("stray").provides::<dyn Other>(), |_, _| {
            unreachable!("refused before it is built")
        });
        let error = ComponentContainer::from_config(
            &ComponentConfig { component_nodes: vec![reference("tools", "shared")] },
            NAMED,
            &Arc::new(through_ref),
            &test_context(),
        )
        .await
        .expect_err("nobody asks for Other");
        assert!(
            matches!(&error, ComponentError::Build { component, .. } if component == "tools"),
            "the ref: {error}"
        );
        assert!(
            error.to_string().contains("its component 'tracker' (stray) provides Other"),
            "{error}"
        );
    }

    /// What a component needs is accepted in turn - from the configuration,
    /// or as the default of a single none provides, with what that needs.
    #[tokio::test]
    async fn what_an_accepted_component_needs_is_accepted_in_turn() {
        let (registry, _) = needing();
        let refused = ComponentContainer::from_config(
            &config(&[("mine", "other")]),
            NAMED,
            &registry,
            &test_context(),
        )
        .await;
        assert!(refused.is_err(), "nobody asks for Choice");

        let container = ComponentContainer::from_config(
            &config(&[("picky", "picky"), ("mine", "other")]),
            NAMED,
            &registry,
            &test_context(),
        )
        .await
        .expect("picky needs Choice");
        assert_eq!(container.get_all::<dyn Named>(None).unwrap().len(), 1);

        ComponentContainer::from_config(
            &config(&[("picky", "picky"), ("stray", "stray")]),
            NAMED,
            &registry,
            &test_context(),
        )
        .await
        .expect("the default of Choice needs Other");
        let error = ComponentContainer::from_config(
            &config(&[("picky", "picky"), ("mine", "other"), ("stray", "stray")]),
            NAMED,
            &registry,
            &test_context(),
        )
        .await
        .expect_err("the configured Choice needs no Other");
        assert!(
            matches!(&error, ComponentError::Build { component, .. } if component == "stray"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn the_configuration_replaces_a_default_for_every_container() {
        let (mut registry, built) = choices();
        registry
            .set_defaults(&config(&[("configured", "other")]))
            .expect("replaces the default of Choice");
        let registry = Arc::new(registry);

        let container =
            ComponentContainer::from_config(&config(&[]), ASKED, &registry, &test_context())
                .await
                .unwrap();
        assert_eq!(container.one::<dyn Choice>().await.unwrap().id, "configured");
        assert_eq!(built.load(Ordering::SeqCst), 0, "the built-in default is not built");
    }

    #[test]
    fn the_kind_of_a_single_is_told_from_the_configuration_without_building_it() {
        let (mut registry, built) = choices();
        assert_eq!(
            registry.kind_of::<dyn Choice>(&config(&[("a", "part"), ("mine", "other")])).as_deref(),
            Some("other")
        );
        assert_eq!(
            registry.kind_of::<dyn Choice>(&config(&[("a", "part")])).as_deref(),
            Some("declared"),
            "the default if none provides it"
        );
        assert_eq!(
            registry.kind_of::<dyn Choice>(&config(&[("shared", BUNDLE_REF_KIND)])),
            None,
            "one of a bundle might provide it"
        );
        assert_eq!(
            registry.kind_of::<dyn Named>(&config(&[("a", "part")])),
            None,
            "of Named there may be any number"
        );
        registry.set_defaults(&config(&[("configured", "other")])).unwrap();
        assert_eq!(
            registry.kind_of::<dyn Choice>(&config(&[])).as_deref(),
            Some("other"),
            "the configured default"
        );
        assert_eq!(built.load(Ordering::SeqCst), 0, "nothing is built");
    }

    #[test]
    fn a_default_the_configuration_cannot_use_is_refused() {
        let (mut registry, _) = choices();
        for (nodes, expected) in [
            (config(&[("x", BUNDLE_REF_KIND)]), "cannot be a ref"),
            (config(&[("x", "unknown")]), "unknown kind 'unknown'"),
            (config(&[("x", "part")]), "provides no facet of which there is one"),
            (config(&[("x", "other"), ("y", "declared")]), "'x' and 'y' both provide Choice"),
            (config(&[("x", COMPONENT_DEFAULT_KIND)]), "cannot build on a default"),
            (config(&[("x", "other"), ("x", "part")]), "two component defaults have the id 'x'"),
        ] {
            let message = registry.set_defaults(&nodes).expect_err(expected).to_string();
            assert!(message.contains(expected), "{message}");
        }
    }

    /// An entry that builds on the default `target` with `params` over its own.
    fn on_default(id: &str, target: &str, params: Value) -> ComponentDescriptor {
        descriptor(id, COMPONENT_DEFAULT_KIND, json!({ "id": target, "params": params }))
    }

    /// A component that builds on a default (issue 445) is the default's
    /// kind, with the default's params and its own over them - each of
    /// its own replacing the default's of its name whole - and its own id,
    /// tags and fail policy.
    #[tokio::test]
    async fn a_component_built_on_a_default_is_the_default_with_its_own_params_over_the_defaults() {
        let (mut registry, built) = choices();
        registry
            .set_defaults(&ComponentConfig {
                component_nodes: vec![descriptor(
                    "model",
                    "other",
                    json!({ "model": "big", "endpoint": { "url": "http://gpu", "auth": "token" } }),
                )],
            })
            .unwrap();
        let registry = Arc::new(registry);
        let mine = ComponentDescriptor {
            fail_policy: ComponentFailPolicy::FailClosed,
            tags: vec!["mine".to_string()],
            ..on_default(
                "mine",
                "model",
                json!({ "endpoint": { "url": "http://elsewhere" }, "prompt_texts": "gemma4" }),
            )
        };
        let container = ComponentContainer::from_config(
            &ComponentConfig { component_nodes: vec![mine] },
            ASKED,
            &registry,
            &test_context(),
        )
        .await
        .unwrap();

        assert_eq!(
            container.one::<dyn Choice>().await.unwrap().id,
            "mine",
            "in place of the default"
        );
        assert_eq!(built.load(Ordering::SeqCst), 0, "the declared default is not built");
        let [resolved] = container.components().component_nodes.try_into().unwrap();
        assert_eq!(
            (resolved.id.as_str(), resolved.kind.as_str()),
            ("mine", "other"),
            "built as the default's kind"
        );
        assert_eq!(
            resolved.params,
            json!({ "model": "big", "endpoint": { "url": "http://elsewhere" }, "prompt_texts": "gemma4" }),
            "no auth beside the endpoint given"
        );
        assert_eq!(
            (resolved.tags, resolved.fail_policy),
            (vec!["mine".to_string()], ComponentFailPolicy::FailClosed),
            "its own"
        );
        assert_eq!(
            registry.kind_of::<dyn Choice>(&container.components()).as_deref(),
            Some("other")
        );
        assert_eq!(
            registry
                .kind_of::<dyn Choice>(&ComponentConfig {
                    component_nodes: vec![on_default("mine", "model", json!(null))]
                })
                .as_deref(),
            Some("other"),
            "told without building it"
        );
    }

    /// The default is found among the container's own defaults first, then
    /// the configuration's; within a bundle as where it is written.
    #[tokio::test]
    async fn a_component_builds_on_the_containers_own_default_and_within_a_ref_as_well() {
        let store = Persistence::in_memory().entity_stores().unwrap().component_bundles;
        let shared = bundle(
            "shared",
            ComponentConfig { component_nodes: vec![on_default("listed", "model", json!(null))] },
        );
        store.create(&SecurityContext::default(), &shared).await.unwrap();
        let (choices, _) = choices();
        let mut registry = ComponentRegistry { bundles: store, ..choices };
        registry.set_defaults(&config(&[("model", "other")])).unwrap();
        let registry = Arc::new(registry);
        let kinds = |container: &ComponentContainer| {
            container
                .components()
                .component_nodes
                .into_iter()
                .map(|node| (node.id, node.kind))
                .collect::<Vec<_>>()
        };

        let own = ComponentContainer::with_defaults(
            &ComponentConfig { component_nodes: vec![on_default("mine", "model", json!(null))] },
            &config(&[("model", "declared")]),
            ASKED,
            &registry,
            &test_context(),
        )
        .await
        .unwrap();
        assert_eq!(
            kinds(&own),
            [("mine".to_string(), "declared".to_string())],
            "the container's own first"
        );
        let listed = ComponentContainer::from_config(
            &ComponentConfig { component_nodes: vec![reference("ref", "shared")] },
            ASKED,
            &registry,
            &test_context(),
        )
        .await
        .unwrap();
        assert_eq!(
            kinds(&listed),
            [("listed".to_string(), "other".to_string())],
            "the configuration's"
        );
    }

    #[tokio::test]
    async fn a_component_built_on_a_default_that_cannot_be_had_is_refused_naming_it() {
        let (mut registry, _) = choices();
        registry
            .set_defaults(&ComponentConfig {
                component_nodes: vec![descriptor("model", "other", json!(["listed"]))],
            })
            .unwrap();
        let registry = Arc::new(registry);
        for (params, expected) in [
            (json!({ "id": "missing" }), "there is no component default 'missing'"),
            (json!({ "id": "model", "parms": {} }), "unknown field `parms`"),
            (
                json!({ "id": "model", "params": { "model": "big" } }),
                "the params of component default 'model' are no object",
            ),
        ] {
            let config = ComponentConfig {
                component_nodes: vec![descriptor("mine", COMPONENT_DEFAULT_KIND, params)],
            };
            let error = ComponentContainer::from_config(&config, ASKED, &registry, &test_context())
                .await
                .expect_err(expected);
            assert!(
                matches!(&error, ComponentError::Build { component, .. } if component == "mine"),
                "{error}"
            );
            assert!(error.to_string().contains(expected), "{error}");
        }
        let (mut registry, _) = choices();
        registry.set_defaults(&config(&[("model", "other")])).unwrap();
        let error = ComponentContainer::from_config(
            &ComponentConfig { component_nodes: vec![on_default("mine", "model", json!("big"))] },
            ASKED,
            &Arc::new(registry),
            &test_context(),
        )
        .await
        .expect_err("params no object");
        assert!(
            error
                .to_string()
                .contains("the params given over component default 'model' are no object"),
            "{error}"
        );
    }

    /// What an operator configures alone are the params of its kind
    /// (issue 403): a request that changes those of such a default is
    /// refused as one that writes the component.
    #[tokio::test]
    async fn a_request_cannot_change_the_params_of_a_default_only_an_operator_configures() {
        let (mut registry, _) = choices();
        registry.register(
            ComponentKind::new("operated").provides::<dyn Choice>().operator_only(),
            |descriptor, _| {
                let mut facets = ComponentFacets::empty();
                facets.insert::<dyn Choice>(Arc::new(Name(descriptor.id.clone())));
                Ok(facets)
            },
        );
        registry.set_defaults(&config(&[("model", "operated")])).unwrap();
        let config = ComponentConfig {
            component_nodes: vec![on_default(
                "mine",
                "model",
                json!({ "url": "http://elsewhere" }),
            )],
        };
        let error = registry
            .require_acceptable(&config, ASKED, &SecurityContext::default())
            .await
            .expect_err("written");
        assert!(
            matches!(causes(&error).last(), Some(ComponentError::OperatorOnly(kind)) if kind == "operated"),
            "{error}"
        );
    }

    /// The program names the default kind of a single (issue 329), and the
    /// configuration's default still replaces it.
    #[tokio::test]
    async fn the_program_names_the_default_kind_of_a_single() {
        let (mut registry, built) = choices();
        registry.set_default_kind::<dyn Choice>("other").expect("provides Choice");
        for (kind, expected) in [
            ("missing", "the default kind 'missing' of Choice is not registered as providing it"),
            ("part", "the default kind 'part' of Choice is not registered as providing it"),
        ] {
            let message =
                registry.set_default_kind::<dyn Choice>(kind).expect_err(expected).to_string();
            assert!(message.contains(expected), "{message}");
        }
        let message =
            registry.set_default_kind::<dyn Named>("part").expect_err("not a single").to_string();
        assert!(message.contains("Named is not a facet of which there is one"), "{message}");
        assert_eq!(
            registry.kind_of::<dyn Choice>(&config(&[])).as_deref(),
            Some("other"),
            "the program's, kept when refused"
        );

        let shared = Arc::new(registry);
        let container =
            ComponentContainer::from_config(&config(&[]), ASKED, &shared, &test_context())
                .await
                .unwrap();
        assert_eq!(container.one::<dyn Choice>().await.unwrap().id, "other");
        assert_eq!(built.load(Ordering::SeqCst), 0, "the declared default is not built");

        let (mut registry, _) = choices();
        registry.set_default_kind::<dyn Choice>("other").unwrap();
        registry.set_defaults(&config(&[("configured", "declared")])).unwrap();
        assert_eq!(
            registry.kind_of::<dyn Choice>(&config(&[])).as_deref(),
            Some("declared"),
            "the configuration's comes first"
        );
    }

    /// A single declared again - by a plugin registered after the program
    /// named its default, as the agents' plugin is in a test's registry
    /// (issue 370) - keeps the program's choice.
    #[test]
    fn a_single_declared_again_keeps_the_default_the_program_named() {
        let (mut registry, _) = choices();
        registry.set_default_kind::<dyn Choice>("other").unwrap();
        registry.single::<dyn Choice>(None);
        assert_eq!(
            registry.kind_of::<dyn Choice>(&config(&[])).as_deref(),
            Some("other"),
            "the program's"
        );

        let (mut registry, _) = choices();
        registry.single::<dyn Choice>(None);
        assert_eq!(registry.kind_of::<dyn Choice>(&config(&[])), None, "the declaration replaced");
    }
}
