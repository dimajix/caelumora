//! The component kinds of the tools that reach the web (issue 544), a module each: `web_fetch`,
//! whose tool fetches a page by its URL.

mod web_fetch;

use caelumora_api::component::LibraryComponents;

/// Registers the component kinds.
pub(crate) fn register(registry: &mut LibraryComponents<'_>) {
    web_fetch::register(registry);
}
