//! An agent's notes in a channel (issue 17) and its private notes there (issue 51), as a turn sees
//! and changes them (issue 607).
//!
//! The notes are the plugin's to keep and to decide about: a work reads its agent's chains in its
//! channel through the `NotesReader` of its situation, and a turn stages what it changes of them
//! (`Turn::stage`), which the plugin commits when the turn completes, checking every change.
//! [`TurnNotes`] is the logic on top: noting - by a number the plugin reserves -, updating,
//! revoking, reclassifying, each looking at the notes as they stand with the turn's own changes in
//! them ([`TurnNotes::effective`], issue 156). The `notes` and `private_notes` components use it.

use crate::extension::{NotesReader, NotesWriter};
use crate::note::{NoteChange, NoteId, NoteKind, Notes};
use caelumora_agents::tool::{ToolError, ToolResult};
use caelumora_agents::turn::Turn;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::ledger::{Change, Staged};
use std::sync::Arc;

/// An agent's chains of notes in a channel, as a turn sees them: every function takes the turn and
/// which chain.
pub(crate) struct TurnNotes;

impl TurnNotes {
    /// The notes as they stand with the changes of the current turn in them (issue 156): what the
    /// turn's operations on notes look at.
    ///
    /// # Errors
    /// `ExecutionFailed` where the notes cannot be reached, or what the turn staged be read.
    pub(crate) async fn effective(turn: &Turn<'_>, kind: NoteKind) -> ToolResult<Staged<Notes>> {
        let reader = Self::reader(turn).await?;
        let staged = Self::staged(turn, kind)?;
        Ok(reader.own(kind).with_staged(staged.iter().map(|change| (change, ()))))
    }

    /// Notes `text` in this turn, classified as the turn has seen so far (`Situation::label`, issue
    /// 168), and returns the note's number: reserved at once by the plugin, which keeps the agent's
    /// numbers in the channel (`NotesWriter::reserve_id`), since its turns in the channel's other
    /// threads note at the same time - as the memory's numbers are.
    ///
    /// # Errors
    /// `ExecutionFailed` where the notes cannot be reached, no number can be reserved, or the
    /// change cannot be kept with the turn.
    pub(crate) async fn note(
        turn: &mut Turn<'_>,
        kind: NoteKind,
        text: String,
    ) -> ToolResult<NoteId> {
        let reserved = Self::writer(turn).await?.reserve_id(kind).await;
        let id = reserved.map_err(|e| {
            ToolError::ExecutionFailed(format!("no number could be reserved for the note: {e}"))
        })?;
        let classification = turn.situation().label();
        Self::stage(turn, kind, Change::Add { id, text, classification })?;
        Ok(id)
    }

    /// Updates note `id` - one of the current notes, or one noted earlier in this turn - to say
    /// `text` when the turn completes (issue 302), classified as the turn has seen so far, as a new
    /// note is. `false` if there is no such note (or it is revoked).
    ///
    /// # Errors
    /// As `note`.
    pub(crate) async fn update(
        turn: &mut Turn<'_>,
        kind: NoteKind,
        id: NoteId,
        text: String,
    ) -> ToolResult<bool> {
        if Self::effective(turn, kind).await?.get(id).is_none() {
            return Ok(false);
        }
        let classification = turn.situation().label();
        Self::stage(turn, kind, Change::Update { id, text, classification })?;
        Ok(true)
    }

    /// Revokes note `id` - one of the current notes, or one noted earlier in this turn - when the
    /// turn completes. `false` if there is no such note (or it is revoked already).
    ///
    /// # Errors
    /// As `note`.
    pub(crate) async fn revoke(
        turn: &mut Turn<'_>,
        kind: NoteKind,
        id: NoteId,
    ) -> ToolResult<bool> {
        if Self::effective(turn, kind).await?.get(id).is_none() {
            return Ok(false);
        }
        Self::stage(turn, kind, Change::Remove { id })?;
        Ok(true)
    }

    /// Reclassifies note `id` as `classification` (issue 133) - one of the current notes, or one
    /// noted earlier in this turn, and not revoked - if the kernel lets the agent
    /// (`NotesWriter::may_reclassify`): never above the channel. Taken over when the turn
    /// completes.
    ///
    /// # Errors
    /// `InvalidParameter` if there is no such note, `PermissionDenied` if the policy refuses, or as
    /// `note`.
    pub(crate) async fn reclassify(
        turn: &mut Turn<'_>,
        kind: NoteKind,
        id: NoteId,
        classification: Classification,
    ) -> ToolResult<()> {
        if Self::effective(turn, kind).await?.get(id).is_none() {
            return Err(ToolError::InvalidParameter(format!(
                "you have no note #{id}; only your own current notes can be reclassified"
            )));
        }
        if !Self::writer(turn).await?.may_reclassify(&classification).await {
            return Err(ToolError::PermissionDenied(format!(
                "reclassifying note #{id} as {classification}"
            )));
        }
        Self::stage(turn, kind, Change::Reclassify { id, classification })
    }

    /// The reader of the notes bound to the turn's work.
    async fn reader(turn: &Turn<'_>) -> ToolResult<Arc<dyn NotesReader>> {
        turn.situation()
            .reader::<dyn NotesReader>()
            .await
            .ok_or_else(|| ToolError::ExecutionFailed("your notes cannot be reached here".into()))
    }

    /// The writer of the notes bound to the turn's work.
    async fn writer(turn: &Turn<'_>) -> ToolResult<Arc<dyn NotesWriter>> {
        turn.writer::<dyn NotesWriter>()
            .await
            .ok_or_else(|| ToolError::ExecutionFailed("your notes cannot be reached here".into()))
    }

    /// The changes of `kind` the turn staged so far. A change taken back within the turn is taken
    /// back by another change, not dropped (issue 156).
    fn staged(turn: &Turn<'_>, kind: NoteKind) -> ToolResult<Vec<Change<NoteId>>> {
        let staged = turn.staged::<NoteChange>().map_err(|e| {
            ToolError::ExecutionFailed(format!("what the turn noted cannot be read: {e}"))
        })?;
        Ok(staged.into_iter().filter(|c| c.kind == kind).map(|c| c.change).collect())
    }

    /// Stages `change` to the chain of `kind` on the turn.
    fn stage(turn: &mut Turn<'_>, kind: NoteKind, change: Change<NoteId>) -> ToolResult<()> {
        turn.stage(&NoteChange { kind, change })
            .map_err(|e| ToolError::ExecutionFailed(format!("the change cannot be kept: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::{AgentNotes, AgentNotesId};
    use crate::plugin::NotesPlugin;
    use caelumora_agents::library::AgentsLibrary;
    use caelumora_agents::testing::agent_runtime;
    use caelumora_domain::actor::{ActorId, EventContext};
    use caelumora_domain::agent::{AgentChannelState, AgentId};
    use caelumora_domain::channel::ThreadId;
    use caelumora_domain::confidentiality::Confidentiality;
    use caelumora_domain::work::Work;
    use caelumora_kernel::channel::ChannelLoop;
    use caelumora_kernel::testing::{participant_runtime_in, World};

    /// The world with the notes, alice's agent `helper` in the main channel of a session she
    /// shares with bob, and that channel.
    async fn with_a_helper() -> (World, Arc<ChannelLoop>, AgentId) {
        let world = World::with(&[&NotesPlugin], &[&AgentsLibrary], |_| {}).await;
        let global = world.personality("global", None, &[]).await;
        let helper = world.agent("helper", world.alice, &global).await;
        let session = world.shared_session(&[helper], &[]).await;
        let main = Arc::clone(&session.channels()[0]);
        (world, main, helper)
    }

    /// A turn of `helper` begun on `state` in `main`, asked for by alice.
    fn turn_in<'a>(
        world: &World,
        main: &Arc<ChannelLoop>,
        helper: AgentId,
        state: &'a mut AgentChannelState,
    ) -> Turn<'a> {
        let line = ThreadId::line(main.id());
        let asked = ActorId::User(world.alice);
        let context = EventContext::new(main.session_id(), main.id(), asked, String::new());
        let work = Work::new(context, None, main.classification(), Default::default());
        state.begin_turn(line, work);
        let runtime = agent_runtime(participant_runtime_in(main, ActorId::Agent(helper)));
        Turn::of(state, line, runtime).expect("a turn was begun")
    }

    /// The texts and numbers of the chain of `kind` of `helper` in `main`, as stored.
    async fn stored(
        world: &World,
        main: &ChannelLoop,
        helper: AgentId,
        kind: NoteKind,
    ) -> Vec<(u32, String)> {
        let notes = world.entity_store::<AgentNotes>();
        let Ok(notes) = notes.get(&World::system(), AgentNotesId::of(helper, main.id())).await
        else {
            return Vec::new();
        };
        notes.chain(kind).current().into_iter().map(|note| (note.id.0, note.text)).collect()
    }

    /// Notes are committed when the turn completes, not before; each chain counts on its own, so
    /// that gaps in the public numbers do not reveal private notes.
    #[tokio::test]
    async fn notes_are_committed_with_the_turn_and_each_chain_numbered_apart() {
        let (world, main, helper) = with_a_helper().await;
        let mut state = AgentChannelState::default();
        let mut turn = turn_in(&world, &main, helper, &mut state);

        let public = TurnNotes::note(&mut turn, NoteKind::Public, "first".into()).await.unwrap();
        let second = TurnNotes::note(&mut turn, NoteKind::Public, "second".into()).await.unwrap();
        let private = TurnNotes::note(&mut turn, NoteKind::Private, "mine".into()).await.unwrap();
        assert_eq!((public, second, private), (NoteId(1), NoteId(2), NoteId(1)));
        assert!(stored(&world, &main, helper, NoteKind::Public).await.is_empty(), "not before");
        turn.take_over().await;

        let public = stored(&world, &main, helper, NoteKind::Public).await;
        assert_eq!(public, [(1, "first".to_string()), (2, "second".to_string())]);
        let private = stored(&world, &main, helper, NoteKind::Private).await;
        assert_eq!(private, [(1, "mine".to_string())]);
    }

    /// An updated note keeps its number; a revoked one is gone and can be neither updated nor
    /// reclassified, and its number is not given again - also one revoked in the turn that wrote
    /// it. A number that names no note is refused.
    #[tokio::test]
    async fn a_note_updated_keeps_its_number_and_one_revoked_uses_it_up() {
        let (world, main, helper) = with_a_helper().await;
        let mut state = AgentChannelState::default();
        let mut turn = turn_in(&world, &main, helper, &mut state);
        TurnNotes::note(&mut turn, NoteKind::Public, "tea".into()).await.unwrap();
        turn.take_over().await;

        let mut turn = turn_in(&world, &main, helper, &mut state);
        assert!(TurnNotes::update(&mut turn, NoteKind::Public, NoteId(1), "coffee".into())
            .await
            .unwrap());
        let gone = TurnNotes::note(&mut turn, NoteKind::Public, "gone".into()).await.unwrap();
        assert!(TurnNotes::revoke(&mut turn, NoteKind::Public, gone).await.unwrap());
        assert!(!TurnNotes::update(&mut turn, NoteKind::Public, gone, "again".into())
            .await
            .unwrap());
        let refused =
            TurnNotes::reclassify(&mut turn, NoteKind::Public, gone, Classification::PUBLIC).await;
        assert!(matches!(refused, Err(ToolError::InvalidParameter(_))), "{refused:?}");
        assert!(!TurnNotes::revoke(&mut turn, NoteKind::Private, NoteId(1)).await.unwrap(), "none");
        turn.take_over().await;

        assert_eq!(
            stored(&world, &main, helper, NoteKind::Public).await,
            [(1, "coffee".to_string())]
        );
        let mut turn = turn_in(&world, &main, helper, &mut state);
        let next = TurnNotes::note(&mut turn, NoteKind::Public, "next".into()).await.unwrap();
        assert_eq!(next, NoteId(3), "2 was used up by the note revoked in its turn");
    }

    /// Two turns of the agent in the channel - one set aside while another is taken - never give a
    /// note the same number: the plugin reserves each as it is noted, and both commit.
    #[tokio::test]
    async fn two_turns_at_the_same_time_give_their_notes_numbers_of_their_own() {
        let (world, main, helper) = with_a_helper().await;
        let (mut first, mut second) = (AgentChannelState::default(), AgentChannelState::default());
        let mut set_aside = turn_in(&world, &main, helper, &mut first);
        let mut taken = turn_in(&world, &main, helper, &mut second);

        let one = TurnNotes::note(&mut set_aside, NoteKind::Public, "aside".into()).await.unwrap();
        let other = TurnNotes::note(&mut taken, NoteKind::Public, "taken".into()).await.unwrap();
        assert_ne!(one, other, "each its own number");
        taken.take_over().await;
        set_aside.take_over().await;

        let mut texts: Vec<String> = stored(&world, &main, helper, NoteKind::Public)
            .await
            .into_iter()
            .map(|(_, t)| t)
            .collect();
        texts.sort();
        assert_eq!(texts, ["aside", "taken"], "neither commit refused");
    }

    /// A note is never reclassified above the channel it was written in.
    #[tokio::test]
    async fn a_note_is_not_reclassified_above_its_channel() {
        let (world, main, helper) = with_a_helper().await;
        let mut state = AgentChannelState::default();
        let mut turn = turn_in(&world, &main, helper, &mut state);
        let id = TurnNotes::note(&mut turn, NoteKind::Public, "tea".into()).await.unwrap();

        let secret = Classification::general(Confidentiality::Confidential);
        let refused = TurnNotes::reclassify(&mut turn, NoteKind::Public, id, secret).await;
        assert!(matches!(refused, Err(ToolError::PermissionDenied(_))), "{refused:?}");
        TurnNotes::reclassify(&mut turn, NoteKind::Public, id, Classification::PUBLIC)
            .await
            .expect("released by its author, also in the turn that wrote it");
    }
}
