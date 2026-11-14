//! The channels of a session the terminal follows for the user, and the one they talk in (issue
//! 561).

use crate::followed::Followed;
use caelumora_domain::actor::ActorId;
use caelumora_domain::approval::ApprovalId;
use caelumora_domain::channel::{Channel, ChannelId, JOINED};
use caelumora_domain::event::{EntityChange, GlobalEvent};
use caelumora_domain::question::QuestionId;
use caelumora_domain::value::name::Name;
use caelumora_kernel::actor::user::Lagged;
use caelumora_kernel::actor::Shown;
use caelumora_kernel::event::{EventBus, EventSubscription, SubscriptionTopic};
use caelumora_kernel::session::SessionLoop;
use caelumora_kernel::view::Views;
use futures::future::select_all;
use std::future;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;
use tracing::warn;
use uuid::Uuid;

/// The channels of a session the terminal follows for the user - every one they take part in,
/// those they join meanwhile too - and the one they talk in: of the others, it tells what concerns
/// the user (`Followed::concerns_user`).
pub(crate) struct Channels {
    /// The session, where the channels the user joins meanwhile are found.
    session: Arc<SessionLoop>,
    /// The views as the user reads them, through which a channel is followed.
    views: Views,
    /// What the event bus tells of the session in the user's name: where they joined a channel,
    /// or one was restored.
    session_events: EventSubscription,
    /// The channels followed, in the order of the session, those joined meanwhile after them.
    followed: Vec<Followed>,
    /// The channel talked in; `None` once it ended, until the user switches to another.
    talked_in: Option<ChannelId>,
}

/// What happens next in the channels followed, or to them.
#[expect(
    clippy::large_enum_variant,
    reason = "a value returned once and taken in at once, nearly always what a channel showed, as \
              `Shown` is"
)]
pub(crate) enum Happened {
    /// What the channel showed.
    Shown(ChannelId, Result<Shown, Lagged>),
    /// The channels the user takes part in may have changed: the event bus told that they joined
    /// one, or that one was restored, or lost what it would have told. That they left one, the
    /// channel shows as the end of their actor's run.
    TakingPart,
}

impl Channels {
    /// Follows every channel of `session` the user of `views` takes part in, and talks in the first
    /// of them; learns of those they join later from `bus`. `None` if they take part in none.
    pub(crate) async fn new(
        session: Arc<SessionLoop>,
        views: Views,
        bus: &EventBus,
    ) -> Option<Self> {
        // Subscribed first, so that no join is lost between the channels read and the
        // subscription.
        let topics = vec![SubscriptionTopic::Session(session.id())];
        let session_events = bus.subscribe(views.context().clone(), topics);
        let mut channels =
            Self { session, views, session_events, followed: Vec::new(), talked_in: None };
        channels.follow_the_others().await;
        channels.talked_in = Some(channels.followed.first()?.channel_id());
        Some(channels)
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
    pub(crate) fn awaiting(&self, id: ApprovalId) -> Option<&Followed> {
        self.followed.iter().find(|followed| followed.is_pending(id))
    }

    /// The channel whose questions `arg` names: the one that knows questions of that id
    /// (`Followed::knows_questions`), or without one the channel talked in.
    pub(crate) fn asking(&self, arg: &str) -> Option<&Followed> {
        match arg.trim() {
            "" => self.talked_in(),
            id => {
                let id = QuestionId(Uuid::parse_str(id).ok()?);
                self.followed.iter().find(|followed| followed.knows_questions(id))
            }
        }
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
        followed.talking_in().await
    }

    /// The lines the terminal opens with (issue 566): those of the channel talked in, as a switch
    /// to it tells them (`Followed::talking_in`), what waits for the user in the others
    /// (`Followed::waiting`), marked with their names, as following a channel anew tells it
    /// (issue 565), and, if the user takes part in others, where they could switch to.
    pub(crate) async fn opening(&self) -> Vec<String> {
        let mut lines = match self.talked_in() {
            Some(followed) => followed.talking_in().await,
            None => Vec::new(),
        };
        for followed in &self.followed {
            if self.talked_in != Some(followed.channel_id()) {
                lines.extend(marked(&followed.name(), followed.waiting()));
            }
        }
        if self.followed.len() > 1 {
            lines.push(self.hint());
        }
        lines
    }

    /// What happens next in the channels followed, or to them: what one of them shows, or what
    /// the event bus tells of the user's taking part. Nothing is lost if it is dropped before it
    /// completes.
    pub(crate) async fn next(&mut self) -> Happened {
        let me = self.views.context().caller_id;
        tokio::select! {
            (channel_id, shown) = shown(&mut self.followed) => Happened::Shown(channel_id, shown),
            () = taking_part(&mut self.session_events, me) => Happened::TakingPart,
        }
    }

    /// Takes in what `happened`, and answers the lines the terminal prints of it: of what a
    /// channel showed (`take_in_shown`), or of following anew where the channels the user takes
    /// part in may have changed (`follow_anew`).
    pub(crate) async fn take_in(&mut self, happened: Happened) -> Vec<String> {
        match happened {
            Happened::Shown(channel_id, shown) => self.take_in_shown(channel_id, shown).await,
            Happened::TakingPart => self.follow_anew().await,
        }
    }

    /// Follows anew where the channels the user takes part in may have changed - also where the
    /// event bus lost what it would have told, as every subscriber of it makes up for what it lost
    /// (`docs/EVENTS.md`): a channel followed they no longer take part in is followed no more
    /// (`unfollow_if_ended`), and every one they take part in that is not followed yet is
    /// followed, without a switch to it (`follow_the_others`). Answers the lines that tell so, and
    /// of a channel followed anew what waits for the user there (`Followed::waiting`, issue 565),
    /// marked with its name.
    async fn follow_anew(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        let followed: Vec<ChannelId> = self.followed.iter().map(Followed::channel_id).collect();
        for channel_id in followed {
            lines.extend(self.unfollow_if_ended(channel_id).await);
        }
        for followed in self.follow_the_others().await {
            let name = followed.name();
            lines.push(format!(
                "--- You take part in channel {name} now: type '/switch {name}' to talk there ---"
            ));
            lines.extend(marked(&name, followed.waiting()));
        }
        lines
    }

    /// Follows every channel of the session the user takes part in that is not followed yet, and
    /// answers those it followed.
    async fn follow_the_others(&mut self) -> &[Followed] {
        let before = self.followed.len();
        for channel in self.session.channels() {
            if self.followed(channel.id()).is_none() {
                let followed = Followed::new(channel, self.views.clone()).await;
                self.followed.extend(followed);
            }
        }
        &self.followed[before..]
    }

    /// Takes in what the channel `channel_id` showed (`Followed::take_in`), and answers the lines
    /// the terminal prints of it: all of the channel talked in; of another, what concerns the
    /// user (`Followed::concerns_user`), marked with the channel's name. Where the watch lagged or
    /// a run of the user's actor began, what waits for them is read anew (`Followed::catch_up`),
    /// and told where it was not known; where a run began or ended, whether they still take part
    /// (`unfollow_if_ended`).
    async fn take_in_shown(
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
        let told = |lines: Vec<String>| match talked_in {
            true => lines,
            false => marked(&name, lines),
        };
        match shown {
            Ok(Shown::Envelope(envelope)) => {
                let printed = talked_in || followed.concerns_user(&envelope);
                let lines = followed.take_in(&envelope);
                if printed {
                    told(lines)
                } else {
                    Vec::new()
                }
            }
            // What it showed between two runs never reached the user's actor; asked first
            // whether they still take part, as the channel stream of the REST server does.
            Ok(Shown::Started) => {
                let ended = self.unfollow_if_ended(channel_id).await;
                match self.followed.get_mut(at) {
                    Some(followed) if ended.is_empty() => told(followed.catch_up().await),
                    _ => ended,
                }
            }
            Ok(Shown::Ended) => self.unfollow_if_ended(channel_id).await,
            Err(Lagged(lost)) => {
                let mut lines = vec![format!("[{lost} events missed]")];
                lines.extend(followed.catch_up().await);
                told(lines)
            }
        }
    }

    /// Follows the channel `channel_id` no more if the user no longer takes part in it, as the
    /// views read it now (`Followed::ended`), and answers the lines that tell so - with where the
    /// user could talk, if it was the channel talked in -; nothing if they still take part.
    async fn unfollow_if_ended(&mut self, channel_id: ChannelId) -> Vec<String> {
        let Some(at) = self.followed.iter().position(|f| f.channel_id() == channel_id) else {
            return Vec::new();
        };
        if !self.followed[at].ended().await {
            return Vec::new();
        }
        let name = self.followed.remove(at).name();
        let mut lines =
            vec![format!("--- You no longer take part in channel {name}, or it was deleted ---")];
        if self.talked_in == Some(channel_id) {
            self.talked_in = None;
            lines.push(self.hint());
        }
        lines
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

/// What the tests do with the channels past their own API: take in what the bus has yet to deliver
/// of the user's taking part.
#[cfg(test)]
impl Channels {
    /// Takes in what the event bus has yet to deliver of what a test did before
    /// (`EventSubscription::settled`), so that what it does next is told by its own events alone.
    async fn settled(&mut self) {
        self.session_events.settled().await;
        let me = self.views.context().caller_id;
        while let Some(event) = self.session_events.try_recv() {
            if changes_taking_part(&event, me) {
                self.take_in(Happened::TakingPart).await;
            }
        }
    }
}

/// `lines` of the channel `name` as they are told while the user talks in another: each marked
/// with its name.
fn marked(name: &Name, lines: Vec<String>) -> Vec<String> {
    lines.into_iter().map(|line| format!("[{name}] {line}")).collect()
}

/// What one of `followed` shows next, with its channel; never if there are none. Nothing is lost
/// if it is dropped before it completes.
async fn shown(followed: &mut [Followed]) -> (ChannelId, Result<Shown, Lagged>) {
    if followed.is_empty() {
        return future::pending().await;
    }
    let next = followed.iter_mut().map(|followed| {
        let channel_id = followed.channel_id();
        Box::pin(async move { (channel_id, followed.next().await) })
    });
    select_all(next).await.0
}

/// Completes where the channels the user `me` takes part in may have changed: `session_events`
/// tells so (`changes_taking_part`), or lost what it would have told; never once the event bus is
/// gone. Nothing is lost if it is dropped before it completes.
async fn taking_part(session_events: &mut EventSubscription, me: ActorId) {
    loop {
        match session_events.receiver().recv().await {
            Ok(event) if changes_taking_part(&event, me) => return,
            Ok(_) => {}
            Err(RecvError::Lagged(lost)) => {
                warn!("The terminal lost {lost} events of its session, and follows anew the channels its user takes part in");
                return;
            }
            Err(RecvError::Closed) => future::pending().await,
        }
    }
}

/// Whether `event` may add to the channels the user `me` takes part in: they joined one, or one
/// was restored. A leave needs no event: the channel shows the end of their actor's run, where
/// following reads anew whether it lists them (`Followed::ended`).
fn changes_taking_part(event: &GlobalEvent, me: ActorId) -> bool {
    let Some((_, change)) = event.about::<Channel>() else { return false };
    change.data::<ActorId>(JOINED) == Some(me) || matches!(change, EntityChange::Restored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::followed::testing::{
        an_agent_in, answered, approval, approval_waiting_in, from, message,
    };
    use caelumora_domain::actor::ActorEvent;
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::approval::ApprovalRuleId;
    use caelumora_domain::channel::{HistoryAccess, AGENT_STATE_CHANGED, LEFT};
    use caelumora_domain::message::Message;
    use caelumora_domain::question::testing::make_question_request;
    use caelumora_domain::session::SessionId;
    use caelumora_domain::user::UserId;
    use caelumora_kernel::channel::ChannelLoop;
    use caelumora_kernel::kernel::Kernel;
    use caelumora_kernel::session::testing::{
        owned_kernel, running_session_of_a_user, spec, test_kernel,
    };
    use caelumora_kernel::session::SessionManager;
    use caelumora_kernel::view::Viewer;
    use caelumora_security::confidentiality::ConfidentialityRules;
    use caelumora_security::SecurityContext;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::time;

    /// A running session's channels as its owner follows them, with the session, the owner's
    /// context and the second user of the session: its first channel, `side` of the owner and the
    /// other user, and `theirs` of the other user alone.
    async fn followed() -> (Channels, Arc<SessionLoop>, SecurityContext, ActorId) {
        let kernel = test_kernel();
        let manager = Arc::new(SessionManager::new(Arc::clone(&kernel)));
        let (session_loop, owner, other) = running_session_of_a_user(&manager).await;
        let side = spec("side", vec![owner.caller_id, other]);
        session_loop.create_channel(&owner, side).await.expect("the owner creates it");
        let theirs = SecurityContext::new(other, session_loop.id(), Default::default());
        session_loop.create_channel(&theirs, spec("theirs", vec![])).await.expect("they create it");
        let views = manager.views(Viewer::new(owner.clone()));
        let channels = Channels::new(Arc::clone(&session_loop), views, kernel.event_bus())
            .await
            .expect("the owner takes part");
        (channels, session_loop, owner, other)
    }

    /// A running session's channels as its owner follows them, who takes part in its first
    /// channel alone, with the session, the owner's context and the kernel.
    async fn alone() -> (Channels, Arc<SessionLoop>, SecurityContext, Arc<Kernel>) {
        let kernel = test_kernel();
        let manager = Arc::new(SessionManager::new(Arc::clone(&kernel)));
        let (session_loop, owner, _) = running_session_of_a_user(&manager).await;
        let views = manager.views(Viewer::new(owner.clone()));
        let channels = Channels::new(Arc::clone(&session_loop), views, kernel.event_bus())
            .await
            .expect("the owner takes part");
        (channels, session_loop, owner, kernel)
    }

    fn named(session_loop: &SessionLoop, name: &str) -> Arc<ChannelLoop> {
        session_loop.channel_named(name).expect("the channel runs")
    }

    /// What `channel` shows as `sender` sends `event` in it.
    fn envelope_shown(channel: &ChannelLoop, sender: ActorId, event: ActorEvent) -> Happened {
        Happened::Shown(channel.id(), Ok(Shown::Envelope(from(channel, sender, event))))
    }

    /// Takes in what happens next, until a channel showed something: the lines printed of it.
    async fn until_shown(channels: &mut Channels) -> Vec<String> {
        loop {
            let happened = channels.next().await;
            let was_shown = matches!(happened, Happened::Shown(..));
            let lines = channels.take_in(happened).await;
            if was_shown {
                return lines;
            }
        }
    }

    /// Takes in what happens next, until the user is told they take part in channel `name` now,
    /// within five seconds: the lines that tell so.
    async fn until_told(channels: &mut Channels, name: &str) -> Vec<String> {
        let told = format!(
            "--- You take part in channel {name} now: type '/switch {name}' to talk there ---"
        );
        let taken_in = async {
            loop {
                let happened = channels.next().await;
                let lines = channels.take_in(happened).await;
                if lines.contains(&told) {
                    return lines;
                }
            }
        };
        time::timeout(Duration::from_secs(5), taken_in).await.expect("told in time")
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
                if let Happened::Shown(channel_id, Ok(Shown::Envelope(envelope))) =
                    channels.next().await
                {
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

    /// Of a channel not talked in, only what concerns the user is told, marked with its name;
    /// switched to, it shows again what still waits.
    #[tokio::test]
    async fn of_another_channel_only_what_concerns_the_user_is_told() {
        let (mut channels, session_loop, owner, other) = followed().await;
        let me = owner.caller_id;
        let side = named(&session_loop, "side");
        let agent_id = AgentId::new();
        let agent = ActorId::Agent(agent_id);

        let said = |text| ActorEvent::CompleteMessageGenerated(message(other, text));
        assert!(channels.take_in(envelope_shown(&side, other, said("hello"))).await.is_empty());
        let mentioning = Message { mentions: vec![me], ..message(other, "@owner look") };
        let event = ActorEvent::CompleteMessageGenerated(mentioning);
        assert_eq!(
            channels.take_in(envelope_shown(&side, other, event)).await,
            ["[side] other> \x1b[1;36m@owner\x1b[0m look"]
        );
        let theirs = ActorEvent::QuestionRequested(make_question_request(
            side.session_id(),
            side.id(),
            agent_id,
            other,
        ));
        assert!(channels.take_in(envelope_shown(&side, agent, theirs)).await.is_empty());
        let request = approval(&side, agent_id);
        let id = request.id;
        let event = ActorEvent::ApprovalRequested(request);
        let told = channels.take_in(envelope_shown(&side, agent, event)).await;
        assert_eq!(
            told.first(),
            Some(&format!("[side] --- Approval requested (id: {}) ---", id.0))
        );

        let first = channels.talked_in().expect("talked in").channel_id();
        let main = session_loop.channel(first).expect("the first channel");
        let event = ActorEvent::CompleteMessageGenerated(message(other, "hello"));
        assert_eq!(channels.take_in(envelope_shown(&main, other, event)).await, ["other> hello"]);

        let switched = channels.switch("side").await;
        assert_eq!(switched.first().map(String::as_str), Some("--- Talking in side now ---"));
        let approval_line = format!("--- Approval requested (id: {}) ---", id.0);
        assert!(switched.contains(&approval_line), "what waits there: {switched:?}");
        assert_eq!(channels.talked_in().map(|f| f.channel_id()), Some(side.id()));
    }

    /// The end of an approval told of another channel is told as well, marked with its name:
    /// withdrawn, or answered by a rule of the user's (issue 566).
    #[tokio::test]
    async fn the_end_of_an_approval_in_another_channel_is_told() {
        let (mut channels, session_loop, owner, _) = followed().await;
        let me = owner.caller_id;
        let side = named(&session_loop, "side");
        let agent_id = AgentId::new();
        let agent = ActorId::Agent(agent_id);
        let request = approval(&side, agent_id);
        let id = request.id;
        channels
            .take_in(envelope_shown(&side, agent, ActorEvent::ApprovalRequested(request)))
            .await;
        let withdrawn =
            || envelope_shown(&side, agent, ActorEvent::ApprovalRequestWithdrawn { id });
        assert_eq!(
            channels.take_in(withdrawn()).await,
            [format!("[side] --- Approval {} withdrawn: the turn ended ---", id.0)]
        );
        assert!(channels.take_in(withdrawn()).await.is_empty(), "told once");

        let request = approval(&side, agent_id);
        let id = request.id;
        channels
            .take_in(envelope_shown(&side, agent, ActorEvent::ApprovalRequested(request)))
            .await;
        let by_rule = answered(me, id, Some(ApprovalRuleId::new()));
        assert_eq!(
            channels.take_in(envelope_shown(&side, me, by_rule)).await,
            [format!("[side] --- Approval {} answered by your rule ---", id.0)]
        );
        assert!(channels.awaiting(id).is_none(), "no longer pending");
    }

    /// At its start the terminal tells the channel it talks in, as a switch to it does, and where
    /// the user could switch to - only if they take part in another (issue 566).
    #[tokio::test]
    async fn the_opening_tells_the_channel_talked_in_and_the_others() {
        let (channels, _, _, _) = followed().await;
        let talked_in = channels.talked_in().expect("talked in");
        let opening = channels.opening().await;
        assert_eq!(opening[..opening.len() - 1], talked_in.talking_in().await[..]);
        assert_eq!(opening.last(), Some(&channels.hint()), "{opening:?}");
        assert!(channels.hint().contains("side"), "the other channel");

        let (alone, _, _, _) = alone().await;
        let talked_in = alone.talked_in().expect("talked in");
        let name = talked_in.name();
        let opening = alone.opening().await;
        assert_eq!(opening, talked_in.talking_in().await, "no other channel");
        assert_eq!(opening.first(), Some(&format!("--- Talking in {name} now ---")));
    }

    /// What waits for the user in a channel other than the one talked in is told at the start,
    /// marked with its name, before where they could switch to (issue 565).
    #[tokio::test]
    async fn the_opening_tells_what_waits_in_the_others() {
        let kernel = test_kernel();
        let manager = Arc::new(SessionManager::new(Arc::clone(&kernel)));
        let (session_loop, owner, _) = running_session_of_a_user(&manager).await;
        let side = spec("side", vec![owner.caller_id]);
        let side = session_loop.create_channel(&owner, side).await.expect("the owner creates it");
        let agent_id = an_agent_in(&kernel, &session_loop, &owner, side.id()).await;
        let id = approval_waiting_in(&kernel, &side, agent_id).await;
        let views = manager.views(Viewer::new(owner));
        let channels = Channels::new(Arc::clone(&session_loop), views, kernel.event_bus())
            .await
            .expect("the owner takes part");
        assert_ne!(channels.talked_in().map(Followed::channel_id), Some(side.id()), "another");

        let opening = channels.opening().await;
        let approval_line = format!("[side] --- Approval requested (id: {}) ---", id.0);
        assert!(opening.contains(&approval_line), "{opening:?}");
        assert_eq!(opening.last(), Some(&channels.hint()), "{opening:?}");
    }

    /// Where the watch of another channel lagged, or a run of the user's actor there began, what
    /// waits for them is read anew and told where it was not known, marked with its name; one that
    /// ended meanwhile is no longer pending (issue 565). Where a run began of a user who no longer
    /// takes part, only that is told.
    #[tokio::test]
    async fn what_was_missed_in_another_channel_is_caught_up() {
        let kernel = test_kernel();
        let manager = Arc::new(SessionManager::new(Arc::clone(&kernel)));
        let (session_loop, owner, _) = running_session_of_a_user(&manager).await;
        let side = spec("side", vec![owner.caller_id]);
        let side = session_loop.create_channel(&owner, side).await.expect("the owner creates it");
        let agent_id = an_agent_in(&kernel, &session_loop, &owner, side.id()).await;
        let views = manager.views(Viewer::new(owner.clone()));
        let mut channels = Channels::new(Arc::clone(&session_loop), views, kernel.event_bus())
            .await
            .expect("the owner takes part");
        let requested =
            |id: ApprovalId| format!("[side] --- Approval requested (id: {}) ---", id.0);

        let first = approval_waiting_in(&kernel, &side, agent_id).await;
        let lost = channels.take_in(Happened::Shown(side.id(), Err(Lagged(3)))).await;
        assert_eq!(lost.first().map(String::as_str), Some("[side] [3 events missed]"));
        assert!(lost.contains(&requested(first)), "{lost:?}");
        let started = Happened::Shown(side.id(), Ok(Shown::Started));
        assert!(channels.take_in(started).await.is_empty(), "known already");

        let second = approval_waiting_in(&kernel, &side, agent_id).await;
        let started = Happened::Shown(side.id(), Ok(Shown::Started));
        let told = channels.take_in(started).await;
        assert!(told.contains(&requested(second)) && !told.contains(&requested(first)), "{told:?}");
        assert!(channels.awaiting(first).is_none(), "ended meanwhile");
        assert_eq!(channels.awaiting(second).map(Followed::channel_id), Some(side.id()));

        approval_waiting_in(&kernel, &side, agent_id).await;
        assert!(session_loop.delete_channel(&owner, side.id()).await.unwrap());
        let started = Happened::Shown(side.id(), Ok(Shown::Started));
        assert_eq!(
            channels.take_in(started).await,
            ["--- You no longer take part in channel side, or it was deleted ---"]
        );
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
            .take_in(envelope_shown(&side, agent, ActorEvent::ApprovalRequested(request)))
            .await;
        let questions =
            make_question_request(side.session_id(), side.id(), agent_id, owner.caller_id);
        let question_id = questions.id;
        let asked = from(&side, agent, ActorEvent::QuestionRequested(questions));
        let actor = channels.followed(side.id()).and_then(Followed::actor).expect("it runs");
        // As the channel routes it: the user's actor keeps it as open, and shows it.
        actor.take_in_as_routed(asked).await;
        until_shown(&mut channels).await;

        assert_eq!(channels.awaiting(id).map(|f| f.channel_id()), Some(side.id()));
        let asking = channels.asking(&question_id.0.to_string()).map(|f| f.channel_id());
        assert_eq!(asking, Some(side.id()), "by id, in another channel");
        let talked_in = channels.talked_in().map(|f| f.channel_id());
        assert_eq!(channels.asking("").map(|f| f.channel_id()), talked_in, "without one");
        let named = channels.followed(side.id()).and_then(|f| f.questions_put_to_user(""));
        assert_eq!(named.map(|q| q.id), Some(question_id));
    }

    /// A channel the user no longer takes part in is followed no more, and told so; if it was the
    /// one talked in, the user is told where they could talk.
    #[tokio::test]
    async fn a_channel_that_ends_is_followed_no_more() {
        let (mut channels, session_loop, owner, _) = followed().await;
        let side = named(&session_loop, "side");
        assert!(
            channels.take_in(Happened::Shown(side.id(), Ok(Shown::Ended))).await.is_empty(),
            "still taking part"
        );
        assert!(session_loop.delete_channel(&owner, side.id()).await.unwrap());
        assert_eq!(
            channels.take_in(Happened::Shown(side.id(), Ok(Shown::Ended))).await,
            ["--- You no longer take part in channel side, or it was deleted ---"]
        );
        assert!(channels.followed(side.id()).is_none());

        let first = channels.talked_in().expect("talked in").channel_id();
        assert!(session_loop.delete_channel(&owner, first).await.unwrap());
        let told = channels.take_in(Happened::Shown(first, Ok(Shown::Ended))).await;
        assert_eq!(told.len(), 2, "{told:?}");
        assert!(channels.talked_in().is_none(), "talking in none");
        assert_eq!(told.last(), Some(&channels.hint()));
    }

    /// A channel another adds the user to meanwhile is followed, and told how to talk there,
    /// without a switch to it: the user learns of it from the event bus, in their own name, under
    /// the ownership policy.
    #[tokio::test]
    async fn a_channel_the_user_is_added_to_meanwhile_is_followed() {
        let (kernel, manager) = owned_kernel(ConfidentialityRules::default());
        let (session_loop, owner, other) = running_session_of_a_user(&manager).await;
        let side = spec("side", vec![owner.caller_id, other]);
        session_loop.create_channel(&owner, side).await.expect("the owner creates it");
        let late = spec("late", vec![owner.caller_id]);
        let late = session_loop.create_channel(&owner, late).await.expect("the owner creates it");
        let theirs = SecurityContext::new(other, session_loop.id(), Default::default());
        let views = manager.views(Viewer::new(theirs));
        let mut channels = Channels::new(Arc::clone(&session_loop), views, kernel.event_bus())
            .await
            .expect("they take part");
        assert!(channels.followed(late.id()).is_none(), "not theirs yet");
        channels.settled().await;
        assert!(channels.followed(late.id()).is_none(), "still not theirs");

        late.add_participant(&owner, other, HistoryAccess::All).await.expect("the owner adds them");
        let talked_in = channels.talked_in().map(|f| f.channel_id());
        until_told(&mut channels, "late").await;
        assert!(channels.followed(late.id()).is_some());
        assert_eq!(channels.talked_in().map(|f| f.channel_id()), talked_in, "no switch");
    }

    /// What waits for the user in a channel they join meanwhile is told with the join, marked with
    /// the channel's name, and found where it waits (issue 565).
    #[tokio::test]
    async fn what_waits_in_a_channel_joined_meanwhile_is_told() {
        let kernel = test_kernel();
        let manager = Arc::new(SessionManager::new(Arc::clone(&kernel)));
        let (session_loop, owner, other) = running_session_of_a_user(&manager).await;
        let late = spec("late", vec![owner.caller_id]);
        let late = session_loop.create_channel(&owner, late).await.expect("the owner creates it");
        let agent_id = an_agent_in(&kernel, &session_loop, &owner, late.id()).await;
        let id = approval_waiting_in(&kernel, &late, agent_id).await;
        let theirs = SecurityContext::new(other, session_loop.id(), Default::default());
        let views = manager.views(Viewer::new(theirs));
        let mut channels = Channels::new(Arc::clone(&session_loop), views, kernel.event_bus())
            .await
            .expect("they take part");
        channels.settled().await;
        assert!(channels.awaiting(id).is_none(), "not followed yet");

        late.add_participant(&owner, other, HistoryAccess::All).await.expect("the owner adds them");
        let told = until_told(&mut channels, "late").await;
        let approval_line = format!("[late] --- Approval requested (id: {}) ---", id.0);
        assert!(told.contains(&approval_line), "{told:?}");
        assert_eq!(channels.awaiting(id).map(Followed::channel_id), Some(late.id()));
    }

    /// What may add to the channels the user takes part in: their own join and a channel
    /// restored; not another's join, nor a leave, which the end of a run shows.
    #[test]
    fn the_users_own_joins_and_restored_channels_count() {
        let (me, another) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
        let at = (SessionId::new(), ChannelId::new());
        let event = |name, who: ActorId| GlobalEvent::custom::<Channel>(&at, name, &who);
        assert!(changes_taking_part(&event(JOINED, me), me));
        assert!(changes_taking_part(&GlobalEvent::restored::<Channel>(&at), me));
        assert!(!changes_taking_part(&event(JOINED, another), me), "another's join");
        assert!(!changes_taking_part(&event(LEFT, me), me), "their own leave");
    }

    /// A channel restored that the user takes part in is followed again, as one joined: the user
    /// learns of it from the event bus.
    #[tokio::test]
    async fn a_channel_restored_is_followed_again() {
        let (mut channels, session_loop, owner, _) = followed().await;
        let side = named(&session_loop, "side");
        assert!(session_loop.delete_channel(&owner, side.id()).await.unwrap());
        channels.take_in(Happened::Shown(side.id(), Ok(Shown::Ended))).await;
        assert!(channels.followed(side.id()).is_none(), "followed no more");
        channels.settled().await;

        session_loop.restore_channel(&owner, side.id()).await.expect("the owner restores it");
        until_told(&mut channels, "side").await;
        assert!(channels.followed(side.id()).is_some());
    }

    /// Following anew drops a channel the user no longer takes part in.
    #[tokio::test]
    async fn following_anew_drops_a_channel_the_user_left() {
        let (mut channels, session_loop, owner, _) = followed().await;
        let side = named(&session_loop, "side");
        assert!(channels.take_in(Happened::TakingPart).await.is_empty(), "still taking part");
        side.remove_participant(&owner, owner.caller_id).await.expect("they may be removed");
        assert_eq!(
            channels.take_in(Happened::TakingPart).await,
            ["--- You no longer take part in channel side, or it was deleted ---"]
        );
        assert!(channels.followed(side.id()).is_none());
    }

    /// A loss on the event bus is made up for by following anew: a channel made with the user in
    /// it while the bus lost the announcement is followed all the same.
    #[tokio::test]
    async fn a_loss_on_the_bus_follows_anew() {
        let (mut channels, session_loop, owner, kernel) = alone().await;
        channels.settled().await;

        let late = spec("late", vec![owner.caller_id]);
        let late = session_loop.create_channel(&owner, late).await.expect("the owner creates it");
        let main = (session_loop.id(), channels.talked_in().expect("one talked in").channel_id());
        let busy = GlobalEvent::custom::<Channel>(&main, AGENT_STATE_CHANGED, &AgentId::new());
        kernel.event_bus().publish_past_capacity(&busy);
        kernel.event_bus().subscribe_all().settled().await;
        assert!(channels.followed(late.id()).is_none(), "not followed yet");

        until_told(&mut channels, "late").await;
        assert!(channels.followed(late.id()).is_some());
    }

    /// Following anew follows every channel the user takes part in that is not followed yet, and
    /// none twice.
    #[tokio::test]
    async fn following_anew_follows_only_what_is_not_followed() {
        let (mut channels, session_loop, owner, _) = followed().await;
        assert!(channels.take_in(Happened::TakingPart).await.is_empty(), "every one followed");
        let late = spec("late", vec![owner.caller_id]);
        let late = session_loop.create_channel(&owner, late).await.expect("the owner creates it");
        assert_eq!(channels.take_in(Happened::TakingPart).await.len(), 1, "the one not followed");
        assert!(channels.followed(late.id()).is_some());
    }
}
