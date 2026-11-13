//! The channels of a session the terminal follows for the user, and the one they talk in (issue
//! 561).

use crate::followed::Followed;
use caelumora_domain::approval::ApprovalId;
use caelumora_domain::channel::ChannelId;
use caelumora_domain::question::{QuestionId, QuestionRequest};
use caelumora_kernel::actor::user::Lagged;
use caelumora_kernel::actor::Shown;
use caelumora_kernel::session::SessionLoop;
use caelumora_kernel::view::Views;
use futures::future::select_all;
use std::future;

/// The channels of a session the terminal follows for the user - every one they take part in -
/// and the one they talk in: of the others, it tells what waits for the user.
pub(crate) struct Channels {
    /// The channels followed, in the order of the session.
    followed: Vec<Followed>,
    /// The channel talked in; `None` once it ended, until the user switches to another.
    talked_in: Option<ChannelId>,
}

impl Channels {
    /// Follows every channel of `session` the user of `views` takes part in, and talks in the first
    /// of them; `None` if they take part in none.
    pub(crate) async fn new(session: &SessionLoop, views: Views) -> Option<Self> {
        let mut followed = Vec::new();
        for channel in session.channels() {
            followed.extend(Followed::new(channel, views.clone()).await);
        }
        let talked_in = Some(followed.first()?.channel_id());
        Some(Self { followed, talked_in })
    }

    /// The channel talked in; `None` once it ended, until the user switches to another.
    pub(crate) fn talked_in(&self) -> Option<&Followed> {
        self.followed(self.talked_in?)
    }

    /// The channel `channel_id`, if it is followed.
    pub(crate) fn followed(&self, channel_id: ChannelId) -> Option<&Followed> {
        self.followed.iter().find(|followed| followed.channel_id() == channel_id)
    }

    /// The channel in which approval `id` is pending.
    pub(crate) fn awaiting(&mut self, id: ApprovalId) -> Option<&mut Followed> {
        self.followed.iter_mut().find(|followed| followed.is_pending(id))
    }

    /// The open questions put to the user that `arg` names by id, in whichever channel, or without
    /// one the last put to them in the channel talked in.
    pub(crate) fn questions_put_to_user(&self, arg: &str) -> Option<&QuestionRequest> {
        match arg.trim() {
            "" => self.talked_in()?.questions_put_to_user(""),
            id => self.followed.iter().find_map(|followed| followed.questions_put_to_user(id)),
        }
    }

    /// Whether questions `id` are open in a channel followed.
    pub(crate) fn is_open(&self, id: QuestionId) -> bool {
        self.followed.iter().any(|followed| followed.is_open(id))
    }

    /// Talks in the channel followed of the whole name `name` from now on, and answers the lines
    /// that tell so - who answers there, and what waits for the user there -, or that the user
    /// takes part in no channel of that name, and where they could talk; without a name, only
    /// where.
    pub(crate) async fn switch(&mut self, name: &str) -> Vec<String> {
        let wanted = name.trim();
        if wanted.is_empty() {
            return vec![self.hint()];
        }
        let Some(followed) = self.followed.iter().find(|followed| followed.name() == wanted) else {
            return vec![format!("--- You take part in no channel '{wanted}' ---"), self.hint()];
        };
        self.talked_in = Some(followed.channel_id());
        let mut lines = vec![format!("--- Talking in {} now ---", followed.name())];
        lines.extend(followed.who_answers().await);
        lines.extend(followed.waiting());
        lines
    }

    /// What one of the channels followed shows next, with the channel; never if none is followed.
    /// Nothing is lost if it is dropped before it completes.
    pub(crate) async fn next(&mut self) -> (ChannelId, Result<Shown, Lagged>) {
        if self.followed.is_empty() {
            return future::pending().await;
        }
        let next = self.followed.iter_mut().map(|followed| {
            let channel_id = followed.channel_id();
            Box::pin(async move { (channel_id, followed.next().await) })
        });
        select_all(next).await.0
    }

    /// Takes in what the channel `channel_id` showed (`Followed::take_in`), and answers the lines
    /// the terminal prints of it: all of the channel talked in; of another, what waits for the
    /// user, marked with the channel's name. Where a run of the user's actor began or ended,
    /// whether they still take part is read anew (`Followed::ended`); a channel they no longer
    /// take part in is followed no more, and told so.
    pub(crate) async fn take_in(
        &mut self,
        channel_id: ChannelId,
        shown: Result<Shown, Lagged>,
    ) -> Vec<String> {
        let talked_in = self.talked_in == Some(channel_id);
        let Some(at) = self.followed.iter().position(|f| f.channel_id() == channel_id) else {
            return Vec::new();
        };
        let followed = &mut self.followed[at];
        let name = followed.name();
        let marked = |lines: Vec<String>| match talked_in {
            true => lines,
            false => lines.into_iter().map(|line| format!("[{name}] {line}")).collect(),
        };
        match shown {
            Ok(Shown::Envelope(envelope)) => {
                let told = talked_in || followed.waits_for_user(&envelope);
                let lines = followed.take_in(&envelope);
                if told {
                    marked(lines)
                } else {
                    Vec::new()
                }
            }
            Ok(Shown::Started | Shown::Ended) => {
                if !followed.ended().await {
                    return Vec::new();
                }
                self.followed.remove(at);
                let mut lines = vec![format!(
                    "--- You no longer take part in channel {name}, or it was deleted ---"
                )];
                if talked_in {
                    self.talked_in = None;
                    lines.push(self.hint());
                }
                lines
            }
            Err(Lagged(lost)) => marked(vec![format!("[{lost} events missed]")]),
        }
    }

    /// The line that tells how the user talks in a channel, and in which they could: the channels
    /// they take part in.
    pub(crate) fn hint(&self) -> String {
        if self.followed.is_empty() {
            return "--- You take part in no channel of this session: type 'exit' to end ---"
                .to_string();
        }
        let names: Vec<String> =
            self.followed.iter().map(|followed| followed.name().to_string()).collect();
        format!("--- Type '/switch <channel>' to talk in one of: {} ---", names.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::followed::testing::{approval, asked_of, from, message};
    use caelumora_domain::actor::{ActorEvent, ActorId};
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::message::Message;
    use caelumora_kernel::channel::ChannelLoop;
    use caelumora_kernel::session::testing::{running_session_of_a_user, spec, test_kernel};
    use caelumora_kernel::session::SessionManager;
    use caelumora_kernel::view::Viewer;
    use caelumora_security::SecurityContext;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::time;

    /// A running session's channels as its owner follows them, with the session, the owner's
    /// context and the second user of the session: its first channel, `side` of the owner and the
    /// other user, and `theirs` of the other user alone.
    async fn followed() -> (Channels, Arc<SessionLoop>, SecurityContext, ActorId) {
        let manager = Arc::new(SessionManager::new(test_kernel()));
        let (session_loop, owner, other) = running_session_of_a_user(&manager).await;
        let side = spec("side", vec![owner.caller_id, other]);
        session_loop.create_channel(&owner, side).await.expect("the owner creates it");
        let theirs = SecurityContext::new(other, session_loop.id(), Default::default());
        session_loop.create_channel(&theirs, spec("theirs", vec![])).await.expect("they create it");
        let views = manager.views(Viewer::new(owner.clone()));
        let channels = Channels::new(&session_loop, views).await.expect("the owner takes part");
        (channels, session_loop, owner, other)
    }

    fn named(session_loop: &SessionLoop, name: &str) -> Arc<ChannelLoop> {
        session_loop.channel_named(name).expect("the channel runs")
    }

    /// What `channel` shows as `sender` sends `event` in it.
    fn shown(channel: &ChannelLoop, sender: ActorId, event: ActorEvent) -> Result<Shown, Lagged> {
        Ok(Shown::Envelope(from(channel, sender, event)))
    }

    #[tokio::test]
    async fn every_channel_taken_part_in_is_followed_and_the_first_talked_in() {
        let (channels, session_loop, _, _) = followed().await;
        let first = session_loop.channels().into_iter().next().expect("the first channel");
        assert_eq!(channels.talked_in().map(|f| f.channel_id()), Some(first.id()));
        assert!(channels.followed(named(&session_loop, "side").id()).is_some());
        assert!(channels.followed(named(&session_loop, "theirs").id()).is_none(), "not theirs");
    }

    /// What a channel other than the first shows comes from the watches of all of them.
    #[tokio::test]
    async fn what_any_channel_followed_shows_comes_next() {
        let (mut channels, session_loop, _, other) = followed().await;
        let side = named(&session_loop, "side");
        let ActorId::User(user) = other else { unreachable!("a user") };
        let theirs = side.user_actor(user).expect("they take part");
        theirs.send_message(Some("hello".to_string()), None, Vec::new()).await.expect("said");
        let said = async {
            loop {
                if let (channel_id, Ok(Shown::Envelope(envelope))) = channels.next().await {
                    if let ActorEvent::CompleteMessageGenerated(message) = envelope.event {
                        return (channel_id, message.content);
                    }
                }
            }
        };
        let (channel_id, content) =
            time::timeout(Duration::from_secs(5), said).await.expect("shown in time");
        assert_eq!((channel_id, content.as_deref()), (side.id(), Some("hello")));
    }

    /// Of a channel not talked in, only what waits for the user is told, marked with its name;
    /// switched to, it shows again what still waits.
    #[tokio::test]
    async fn of_another_channel_only_what_waits_for_the_user_is_told() {
        let (mut channels, session_loop, owner, other) = followed().await;
        let me = owner.caller_id;
        let side = named(&session_loop, "side");
        let agent_id = AgentId::new();
        let agent = ActorId::Agent(agent_id);

        let said = |text| ActorEvent::CompleteMessageGenerated(message(other, text));
        assert!(channels.take_in(side.id(), shown(&side, other, said("hello"))).await.is_empty());
        let mentioning = Message { mentions: vec![me], ..message(other, "@owner look") };
        let event = ActorEvent::CompleteMessageGenerated(mentioning);
        assert_eq!(
            channels.take_in(side.id(), shown(&side, other, event)).await,
            ["[side] other> \x1b[1;36m@owner\x1b[0m look"]
        );
        let theirs = ActorEvent::QuestionRequested(asked_of(&side, agent_id, other));
        assert!(channels.take_in(side.id(), shown(&side, agent, theirs)).await.is_empty());
        let request = approval(&side, agent_id);
        let id = request.id;
        let event = ActorEvent::ApprovalRequested(request);
        let told = channels.take_in(side.id(), shown(&side, agent, event)).await;
        assert_eq!(
            told.first(),
            Some(&format!("[side] --- Approval requested (id: {}) ---", id.0))
        );
        let lost = channels.take_in(side.id(), Err(Lagged(3))).await;
        assert_eq!(lost, ["[side] [3 events missed]"]);

        let first = channels.talked_in().expect("talked in").channel_id();
        let main = session_loop.channel(first).expect("the first channel");
        let event = ActorEvent::CompleteMessageGenerated(message(other, "hello"));
        assert_eq!(channels.take_in(first, shown(&main, other, event)).await, ["other> hello"]);

        let switched = channels.switch("side").await;
        assert_eq!(switched.first().map(String::as_str), Some("--- Talking in side now ---"));
        let approval_line = format!("--- Approval requested (id: {}) ---", id.0);
        assert!(switched.contains(&approval_line), "what waits there: {switched:?}");
        assert_eq!(channels.talked_in().map(|f| f.channel_id()), Some(side.id()));
    }

    #[tokio::test]
    async fn only_a_channel_taken_part_in_is_switched_to() {
        let (mut channels, _, _, _) = followed().await;
        let talked_in = channels.talked_in().map(|f| f.channel_id());
        let refused = channels.switch("theirs").await;
        assert_eq!(
            refused.first().map(String::as_str),
            Some("--- You take part in no channel 'theirs' ---")
        );
        assert!(refused.last().is_some_and(|hint| hint.contains("side")), "{refused:?}");
        assert_eq!(channels.talked_in().map(|f| f.channel_id()), talked_in, "unchanged");
        assert_eq!(channels.switch(" ").await, [channels.hint()], "where the user could talk");
    }

    /// An approval and questions are found in whichever channel they wait; without an id, only
    /// questions of the channel talked in.
    #[tokio::test]
    async fn approvals_and_questions_are_found_in_whichever_channel() {
        let (mut channels, session_loop, owner, _) = followed().await;
        let side = named(&session_loop, "side");
        let agent_id = AgentId::new();
        let agent = ActorId::Agent(agent_id);
        let request = approval(&side, agent_id);
        let id = request.id;
        channels
            .take_in(side.id(), shown(&side, agent, ActorEvent::ApprovalRequested(request)))
            .await;
        let questions = asked_of(&side, agent_id, owner.caller_id);
        let question_id = questions.id;
        let event = ActorEvent::QuestionRequested(questions);
        channels.take_in(side.id(), shown(&side, agent, event)).await;

        assert_eq!(channels.awaiting(id).map(|f| f.channel_id()), Some(side.id()));
        let named = channels.questions_put_to_user(&question_id.0.to_string()).map(|q| q.id);
        assert_eq!(named, Some(question_id), "by id, in another channel");
        assert!(channels.questions_put_to_user("").is_none(), "none in the channel talked in");
        assert!(channels.is_open(question_id));
    }

    /// A channel the user no longer takes part in is followed no more, and told so; if it was the
    /// one talked in, the user is told where they could talk.
    #[tokio::test]
    async fn a_channel_that_ends_is_followed_no_more() {
        let (mut channels, session_loop, owner, _) = followed().await;
        let side = named(&session_loop, "side");
        assert!(
            channels.take_in(side.id(), Ok(Shown::Ended)).await.is_empty(),
            "still taking part"
        );
        assert!(session_loop.delete_channel(&owner, side.id()).await.unwrap());
        assert_eq!(
            channels.take_in(side.id(), Ok(Shown::Ended)).await,
            ["--- You no longer take part in channel side, or it was deleted ---"]
        );
        assert!(channels.followed(side.id()).is_none());

        let first = channels.talked_in().expect("talked in").channel_id();
        assert!(session_loop.delete_channel(&owner, first).await.unwrap());
        let told = channels.take_in(first, Ok(Shown::Ended)).await;
        assert_eq!(told.len(), 2, "{told:?}");
        assert!(channels.talked_in().is_none(), "talking in none");
        assert_eq!(told.last(), Some(&channels.hint()));
    }
}
