//! Where the program keeps the session and the channel of an agent's work on its host, as the work
//! reads it (`StorePlacesReader`, issue 577). The kernel binds it to every piece of work where the
//! backend of the stores tells where it keeps what lies in a scope, so that an agent can be told
//! where to look.

use std::path::Path;

/// Where the program keeps the data of the work's session and channel in the file system of its
/// host: what the work reads of it is data, found when it was bound.
pub trait StorePlacesReader: Send + Sync {
    /// The directory the program keeps its data below.
    fn data_dir(&self) -> &Path;

    /// The directory of what lies in the work's session.
    fn session_dir(&self) -> &Path;

    /// The directory of what lies in the work's channel.
    fn channel_dir(&self) -> &Path;
}
