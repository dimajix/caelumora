//! The component kinds of the tools that reach the web (issue 544), a module each: `web_fetch`,
//! whose tool fetches a page by its URL, and `web_search`, whose tool searches the web.

mod web_fetch;
mod web_search;

use caelumora_api::component::LibraryComponents;

/// Registers the component kinds.
pub(crate) fn register(registry: &mut LibraryComponents<'_>) {
    web_fetch::register(registry);
    web_search::register(registry);
}
