//! The channels of a session the terminal follows for the user, and the one they talk in (issue
//! 561).

use crate::followed::Followed;
use caelumora_domain::actor::ActorId;
use caelumora_domain::approval::ApprovalId;
use caelumora_domain::channel::{Channel, ChannelId, JOINED};
use caelumora_domain::event::GlobalEvent;
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
/// those they join meanwhile too - and the one they talk in: of the others, it tells what waits
/// for the user.
pub(crate) struct Channels {
    /// The session, where the channels the user joins meanwhile are found.
    session: Arc<SessionLoop>,
    /// The views as the user reads them, through which a channel is followed.
    views: Views,
    /// What the event bus tells of the session in the user's name: where they joined a channel
    /// (`channel::JOINED`).
    joins: EventSubscription,
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
    /// The user may have joined a channel: the event bus told so, or lost what it would have told.
    Joined,
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
        let joins = bus.subscribe(views.context().clone(), topics);
        let mut channels = Self { session, views, joins, followed: Vec::new(), talked_in: None };
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
    pub(crate) fn awaiting(&mut self, id: ApprovalId) -> Option<&mut Followed> {
        self.followed.iter_mut().find(|followed| followed.is_pending(id))
    }

    /// The channel whose questions `arg` names: the one that printed questions of that id, or
    /// without one the channel talked in.
    pub(crate) fn asking(&self, arg: &str) -> Option<&Followed> {
        match arg.trim() {
            "" => self.talked_in(),
            id => {
                let id = QuestionId(Uuid::parse_str(id).ok()?);
                self.followed.iter().find(|followed| followed.printed(id))
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
        let mut lines = vec![format!("--- Talking in {} now ---", followed.name())];
        lines.extend(followed.who_answers().await);
        lines.extend(followed.waiting());
        lines
    }

    /// What happens next in the channels followed, or to them: what one of them shows, or that
    /// the user may have joined a channel. Nothing is lost if it is dropped before it completes.
    pub(crate) async fn next(&mut self) -> Happened {
        let me = self.views.context().caller_id;
        tokio::select! {
            (channel_id, shown) = shown(&mut self.followed) => Happened::Shown(channel_id, shown),
            () = joined(&mut self.joins, me) => Happened::Joined,
        }
    }

    /// Takes in what `happened`, and answers the lines the terminal prints of it: of what a
    /// channel showed (`take_in_shown`), or the channels the user joined meanwhile, now followed
    /// too - also those whose join the event bus lost, as every subscriber of it makes up for
    /// what it lost (`docs/EVENTS.md`). Joining a channel does not switch to it.
    pub(crate) async fn take_in(&mut self, happened: Happened) -> Vec<String> {
        match happened {
            Happened::Shown(channel_id, shown) => self.take_in_shown(channel_id, shown).await,
            Happened::Joined => {
                let joined = self.follow_the_others().await;
                let told = |name: Name| {
                    format!("--- You take part in channel {name} now: type '/switch {name}' to talk there ---")
                };
                joined.into_iter().map(told).collect()
            }
        }
    }

    /// Follows every channel of the session the user takes part in that is not followed yet, and
    /// answers their names.
    async fn follow_the_others(&mut self) -> Vec<Name> {
        let mut followed = Vec::new();
        for channel in self.session.channels() {
            if self.followed(channel.id()).is_none() {
                followed.extend(Followed::new(channel, self.views.clone()).await);
            }
        }
        let names = followed.iter().map(Followed::name).collect();
        self.followed.extend(followed);
        names
    }

    /// Takes in what the channel `channel_id` showed (`Followed::take_in`), and answers the lines
    /// the terminal prints of it: all of the channel talked in; of another, what waits for the
    /// user, marked with the channel's name. Where a run of the user's actor began or ended,
    /// whether they still take part is read anew (`Followed::ended`); a channel they no longer
    /// take part in is followed no more, and told so.
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

/// Completes where the user `me` may have joined a channel: `joins` tells that they did, or lost
/// what it would have told; never once the event bus is gone. Nothing is lost if it is dropped
/// before it completes.
async fn joined(joins: &mut EventSubscription, me: ActorId) {
    loop {
        match joins.receiver().recv().await {
            Ok(event) if is_join_of(&event, me) => return,
            Ok(_) => {}
            Err(RecvError::Lagged(lost)) => {
                warn!("The terminal lost {lost} events of its session, and follows anew the channels its user takes part in");
                return;
            }
            Err(RecvError::Closed) => future::pending().await,
        }
    }
}

/// Whether `event` tells that `me` joined a channel.
fn is_join_of(event: &GlobalEvent, me: ActorId) -> bool {
    event.about::<Channel>().is_some_and(|(_, change)| change.data::<ActorId>(JOINED) == Some(me))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::followed::testing::{approval, from, message};
    use caelumora_domain::actor::ActorEvent;
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::channel::{HistoryAccess, LEFT};
    use caelumora_domain::message::Message;
    use caelumora_domain::question::testing::make_question_request;
    use caelumora_domain::session::SessionId;
    use caelumora_domain::user::UserId;
    use caelumora_kernel::channel::ChannelLoop;
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
        let lost = channels.take_in(Happened::Shown(side.id(), Err(Lagged(3)))).await;
        assert_eq!(lost, ["[side] [3 events missed]"]);

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
        // What the bus has yet to deliver of the session's making, taken in first.
        while let Ok(happened) = time::timeout(Duration::from_millis(200), channels.next()).await {
            channels.take_in(happened).await;
        }
        assert!(channels.followed(late.id()).is_none(), "still not theirs");

        late.add_participant(&owner, other, HistoryAccess::All).await.expect("the owner adds them");
        let told = "--- You take part in channel late now: type '/switch late' to talk there ---";
        let talked_in = channels.talked_in().map(|f| f.channel_id());
        let joined = async {
            loop {
                let happened = channels.next().await;
                if channels.take_in(happened).await.iter().any(|line| line == told) {
                    return;
                }
            }
        };
        time::timeout(Duration::from_secs(5), joined).await.expect("told in time");
        assert!(channels.followed(late.id()).is_some());
        assert_eq!(channels.talked_in().map(|f| f.channel_id()), talked_in, "no switch");
    }

    /// A join is one of the user's, into a channel; neither another's nor a leave.
    #[test]
    fn only_the_users_own_join_counts() {
        let (me, another) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
        let at = (SessionId::new(), ChannelId::new());
        assert!(is_join_of(&GlobalEvent::custom::<Channel>(&at, JOINED, &me), me));
        assert!(
            !is_join_of(&GlobalEvent::custom::<Channel>(&at, JOINED, &another), me),
            "another's"
        );
        assert!(!is_join_of(&GlobalEvent::custom::<Channel>(&at, LEFT, &me), me), "a leave");
    }

    /// Following anew - where the user may have joined a channel - follows every channel the user
    /// takes part in that is not followed yet, and none twice.
    #[tokio::test]
    async fn following_anew_follows_only_what_is_not_followed() {
        let (mut channels, session_loop, owner, _) = followed().await;
        assert!(channels.take_in(Happened::Joined).await.is_empty(), "every one followed");
        let late = spec("late", vec![owner.caller_id]);
        let late = session_loop.create_channel(&owner, late).await.expect("the owner creates it");
        assert_eq!(channels.take_in(Happened::Joined).await.len(), 1, "the one not followed");
        assert!(channels.followed(late.id()).is_some());
    }
}
