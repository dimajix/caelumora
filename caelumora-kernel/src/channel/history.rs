//! Where a participant takes part in a channel's history and what part of it they see, read from
//! the store (issues 46, 379, 590): a view begins at a position, and the history is read in pages
//! from its end only as far back as is needed; whose attendance is still open is what a channel
//! reconciles as it is built.

use super::{ActorChange, ChannelLoop};
use caelumora_domain::actor::ActorId;
use caelumora_domain::chain::Position;
use caelumora_domain::channel::{ChannelId, HistoryAccess, HistoryStart, ThreadId};
use caelumora_domain::history::{HistoryEntry, Reader};
use caelumora_domain::message::{Message, MessageId};
use caelumora_domain::query::Filter;
use caelumora_domain::store::StoreResult;
use caelumora_security::SecurityContext;
use caelumora_store::history::HistoryStore;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use tracing::{info, warn};

/// How many items a page holds that is read to find something in a history
/// rather than to show it.
const READ_PAGE: usize = 200;

/// Where the view of the history of `channel` begins for a participant
/// granted `access` now (`HistoryAccess::start_in`): its messages read from
/// the end, newest first, only as far back as the grant reaches - those that
/// went to everybody, which is what a grant shares.
///
/// # Errors
/// If the store cannot be read.
async fn start_of(
    histories: &dyn HistoryStore,
    channel: ChannelId,
    access: HistoryAccess,
    now: DateTime<Utc>,
) -> StoreResult<HistoryStart> {
    let mut newest: Vec<Message> = Vec::new();
    let mut before = None;
    while !access.read_enough(&newest, now) {
        let page = histories
            .page(
                channel,
                &Reader::Outsider,
                Position::START,
                before,
                READ_PAGE,
                Some(&Filter::said()),
            )
            .await?;
        newest.extend(page.entries().rev().filter_map(HistoryEntry::said).cloned());
        match page.earlier {
            Some(earlier) => before = Some(earlier),
            None => break,
        }
    }
    Ok(access.start_in(&newest, now))
}

impl ChannelLoop {
    /// The message `id` of the history as it stands, as it was recorded, or
    /// `None` if it was never said here - read from the store, where the
    /// history is as it is in memory: what the checks before a change ask
    /// (issue 379). A store that cannot be read is logged, and the message
    /// is not found.
    pub(crate) async fn message(&self, id: MessageId) -> Option<Message> {
        match self.history_store.said(self.id, &Reader::Channel, id).await {
            Ok(said) => said.and_then(|(_, item)| item.said().cloned()),
            Err(e) => {
                warn!("Cannot read message {} of channel {}: {}", id, self.id, e);
                None
            }
        }
    }

    /// The position from which on a participant granted `access` now sees the history (`start_of`,
    /// issue 417): fixed as it joins, so that what it is given does not move with what is said
    /// after. `None` - what is said from its start on, as a founder (`ActorChange::Start`) - where
    /// the view begins at the join (`HistoryStart::Join`), and where the message it begins at is
    /// gone meanwhile, so that nothing is seen rather than more than was granted.
    ///
    /// # Errors
    /// If the store cannot be read.
    pub(crate) async fn history_start(
        &self,
        access: HistoryAccess,
    ) -> StoreResult<Option<Position>> {
        Ok(match start_of(self.history_store.as_ref(), self.id, access, Utc::now()).await? {
            HistoryStart::Beginning => Some(Position::START),
            HistoryStart::From(message) => {
                self.history_store.said(self.id, &Reader::Channel, message).await?.map(|(at, _)| at)
            }
            HistoryStart::Join => None,
        })
    }

    /// Where `participant`, a participant before - an agent restored, which rested in the channel
    /// (issue 314) -, takes part again: the position from which on it is given the history, where
    /// its last range of attendance began, as when it joined (issue 417), and the thread of that
    /// range (issue 557); all of the history, in the line, if it never took part.
    ///
    /// # Errors
    /// If the store cannot be read.
    pub(crate) async fn rejoining(
        &self,
        participant: ActorId,
    ) -> StoreResult<(Position, ThreadId)> {
        Ok(self
            .history_store
            .attendance(self.id, participant)
            .await?
            .last()
            .map_or((Position::START, ThreadId::line(self.id)), |at| (at.range.start, at.thread)))
    }

    /// Who takes part in the channel's history now, and in which thread: whoever has a range of
    /// attendance still open (`HistoryStore::attending`) - what the channel builds its threads and
    /// starts its actors by, and reconciles, as it is built; nobody if it cannot be read, which is
    /// logged.
    pub(super) async fn attending(&self) -> HashMap<ActorId, ThreadId> {
        self.history_store.attending(self.id).await.unwrap_or_else(|e| {
            warn!("Cannot read who takes part in channel {}: {}", self.id, e);
            HashMap::new()
        })
    }

    /// Brings the channel's history in line with its document as it is built (reconciling, issue
    /// 590): whoever the document keeps as a former participant with a range of attendance still
    /// open in its history - let go while the channel did not run, in its stored document - is
    /// handed to `run` as leaving (`ActorChange::Leave`), in the kernel's name, in the thread of
    /// that range, ahead of every actor it starts: `run` announces it before anything else, as
    /// though it had happened while the channel ran. Who let them go and when, the document keeps
    /// (`FormerParticipant::left`). Where the history was recorded on after they were let go - an
    /// actor removed whose task had not ended as its session stopped - their attendance ends only
    /// here.
    pub(super) async fn reconcile(&self, attending: &HashMap<ActorId, ThreadId>) {
        for former in &self.channel().former {
            let participant = former.participant.actor_id();
            let Some(&thread) = attending.get(&participant) else { continue };
            let name = self.name_of(participant).await;
            info!("{} left channel {} while it did not run: recorded now", participant, self.id);
            let leave = ActorChange::Leave(participant, name, SecurityContext::system(), thread);
            let _ = self.request(leave);
        }
    }
}
