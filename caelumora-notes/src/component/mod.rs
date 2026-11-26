//! The components of an agent's notes (issues 17, 51, 607): `notes`, by which it notes what it
//! publishes in a channel and is given what every agent there noted, and `private_notes`, by which
//! it notes what it keeps for itself - both reaching the notes through the `NotesReader` and the
//! `NotesWriter` the plugin binds, and changing them by the logic of a turn (`turn`).

pub mod notes;
pub mod private_notes;
mod turn;

use caelumora_api::component::LibraryComponents;

/// Registers the component kinds of the notes with `registry`.
pub(crate) fn register(registry: &mut LibraryComponents<'_>) {
    notes::register(registry);
    private_notes::register(registry);
}
