//! Where the states of channels, agents and users lie (issue 554): a backend over files says so
//! with a `StateLayout`, and `JsonStates` keeps them there.

use super::to_backend_error;
use async_trait::async_trait;
use caelumora_domain::channel::ChannelId;
use caelumora_domain::store::StoreResult;
use std::fmt::Debug;
use std::fs as std_fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use tokio::fs;

/// Where the states of channels, agents and users lie. It only says where: what is written makes
/// the directories it needs, and a directory not there holds nothing. The directories `agents_in`
/// and `users_in` answer are `JsonStates`'s alone, which removes them whole with their channel, so
/// that nothing else may lie in them.
///
/// # Errors
/// A method answering a `StoreResult` fails if the place cannot be told. What the state stores
/// were asked then fails with that error, also a reading that otherwise answers a default.
#[async_trait]
pub trait StateLayout: Debug + Send + Sync {
    /// The snapshot of the state of `channel`; its journal and its history lie beside it.
    async fn channel_state(&self, channel: ChannelId) -> StoreResult<PathBuf>;

    /// The directory of the states of the agents in `channel`, those of each beside each other.
    async fn agents_in(&self, channel: ChannelId) -> StoreResult<PathBuf>;

    /// The directory of the states of the users in `channel`.
    async fn users_in(&self, channel: ChannelId) -> StoreResult<PathBuf>;

    /// What `agents_in` answers, of every channel.
    async fn agents_of_every_channel(&self) -> StoreResult<Vec<PathBuf>>;

    /// The directory of the index of the work under way.
    fn under_way(&self) -> PathBuf;
}

/// The layout of the JSON store: every kind of state in a directory of its own, below `runtime`,
/// named by the channel it is of.
#[derive(Debug)]
pub(crate) struct RuntimeLayout {
    runtime: PathBuf,
}

impl RuntimeLayout {
    /// The layout of the states under `root`.
    ///
    /// # Errors
    /// If its directories cannot be made, so that a path that cannot be written fails the start.
    pub(crate) fn new(root: impl AsRef<Path>) -> StoreResult<Self> {
        let layout = Self { runtime: root.as_ref().join("runtime") };
        for dir in [layout.agents(), layout.channels(), layout.users(), layout.under_way()] {
            std_fs::create_dir_all(dir).map_err(to_backend_error)?;
        }
        Ok(layout)
    }

    fn agents(&self) -> PathBuf {
        self.runtime.join("agent_channel_state")
    }

    fn channels(&self) -> PathBuf {
        self.runtime.join("channel_state")
    }

    fn users(&self) -> PathBuf {
        self.runtime.join("user_channel_state")
    }
}

#[async_trait]
impl StateLayout for RuntimeLayout {
    async fn channel_state(&self, channel: ChannelId) -> StoreResult<PathBuf> {
        Ok(self.channels().join(format!("{channel}.json")))
    }

    async fn agents_in(&self, channel: ChannelId) -> StoreResult<PathBuf> {
        Ok(self.agents().join(channel.to_string()))
    }

    /// The states of the users in `channel`, one file each, so that they go with the channel's.
    async fn users_in(&self, channel: ChannelId) -> StoreResult<PathBuf> {
        Ok(self.users().join(channel.to_string()))
    }

    async fn agents_of_every_channel(&self) -> StoreResult<Vec<PathBuf>> {
        let mut channels = match fs::read_dir(self.agents()).await {
            Ok(channels) => channels,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(to_backend_error(err)),
        };
        let mut found = Vec::new();
        while let Some(channel) = channels.next_entry().await.map_err(to_backend_error)? {
            found.push(channel.path());
        }
        Ok(found)
    }

    /// In one directory, so that adding a file never races with removing a directory emptied.
    fn under_way(&self) -> PathBuf {
        self.runtime.join("under_way")
    }
}
