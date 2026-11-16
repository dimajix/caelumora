//! How the kernel tells an agent's work where the stores keep its session and its channel on the
//! host (issue 577): an extension of the kernel's, the reader `StorePlacesReader`, found from where
//! the backend of the entities keeps its data (`Persistence::places`) as it is bound.

use crate::view::SessionView;
use caelumora_api::places::StorePlacesReader;
use caelumora_domain::scope::Scope;
use caelumora_store::places::StorePlaces;
use futures::future;
use std::path::{Path, PathBuf};
use tracing::warn;

/// Where the session and the channel of a piece of work are kept, found as it is bound: the reader.
pub(super) struct WorkPlaces {
    data_dir: PathBuf,
    session_dir: PathBuf,
    channel_dir: PathBuf,
}

impl WorkPlaces {
    /// Where `places` keeps the session the work reads as `session` and the channel it reads from;
    /// `None` where it reads from no channel, or may not read it - and where finding a directory
    /// failed, which is logged.
    pub(super) async fn of_work(places: &dyn StorePlaces, session: &SessionView) -> Option<Self> {
        let channel_id = session.views().viewer().place()?.channel;
        session.channel(channel_id).await.ok()?;
        let session_id = session.id();
        let in_session = Scope::SessionScope { session_id };
        let in_channel = Scope::ChannelScope { session_id, channel_id };
        let found =
            future::try_join(places.directory_of(&in_session), places.directory_of(&in_channel));
        match found.await {
            Ok((session_dir, channel_dir)) => {
                Some(Self { data_dir: places.data_dir().to_path_buf(), session_dir, channel_dir })
            }
            Err(e) => {
                warn!(channel = %channel_id, "Where a channel is kept could not be found: {e}");
                None
            }
        }
    }
}

impl StorePlacesReader for WorkPlaces {
    fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    fn session_dir(&self) -> &Path {
        &self.session_dir
    }

    fn channel_dir(&self) -> &Path {
        &self.channel_dir
    }
}
