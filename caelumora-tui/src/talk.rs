//! The talk of the user in the terminal (issues 28, 570, 571, 579): the channels it follows, what a
//! line typed does in them - a command of the terminal carried out, what is asked of the user
//! answered, or what is left sent to the channel talked in -, what they show, what is asked, and
//! what the user can type there.
//!
//! A shell command run in the workspace (`/workspace run`, `!` for short, issue 587) runs until it
//! ends, which may take long: the talk keeps it under way and follows it beside the channels
//! (`next`), so that the user talks on meanwhile - in another channel too, where how it ended is
//! told as of the channel it runs in; one at a time. Ctrl-C ends it (`end_shell_command`), which
//! aborts it and so kills it - and nothing is told of it in the channel; a switch to another
//! session ends it alike. How it ended, which the channel answers the user alone, may be told
//! before the call it made, which the channel tells everyone.

use crate::activity::Activity;
use crate::asking::{Asked, Asking, Offered};
use crate::channels::{Channels, Happened};
use crate::commands::{commands, terminal_help, TerminalCommand};
use crate::followed::Followed;
use crate::id_prefix::beginning_with;
use crate::printed::{ErrorKind, Printed};
use crate::sessions::{listing, Opening, Sessions, SessionsError};
use crate::typeable::{read_input, Typeable};
use anyhow::Result;
use caelumora_api::command::{check_arguments, CommandError, CommandSet};
use caelumora_domain::actor::ActorEvent;
use caelumora_domain::approval::{ApprovalRequest, ApprovalRule, Reach, RuleDecision, ToolMatcher};
use caelumora_domain::channel::{ChannelId, ThreadId};
use caelumora_domain::command::UserInput;
use caelumora_domain::question::{Question, QuestionRequest, Response};
#[cfg(test)]
use caelumora_domain::session::SessionId;
use caelumora_domain::value::command_name::CommandName;
use caelumora_domain::value::name::Name;
use caelumora_kernel::actor::user::UserActor;
use caelumora_kernel::actor::Shown;
use caelumora_kernel::command::help::HELP_COMMAND_NAME;
use caelumora_kernel::command::{CommandReply, RUN_COMMAND_NAME};
use caelumora_kernel::controller::UserController;
use caelumora_kernel::error::user_message;
use caelumora_kernel::event::EventBus;
use caelumora_kernel::session::SessionLoop;
use caelumora_kernel::view::Views;
use std::future;
use std::sync::Arc;
use tokio_util::task::AbortOnDropHandle;
use tracing::warn;

/// The response `line` gives to `question`: `-` skips it; for one with
/// choices, numbers of choices - separated by commas, several only with
/// `multi_select` - or a choice's label, else another answer; for one
/// without, the line as it is.
fn response_of(question: &Question, line: &str) -> Response {
    if line == "-" {
        return Response::Declined;
    }
    if question.choices.is_empty() {
        return Response::Text { text: line.to_string() };
    }
    let numbers: Option<Vec<usize>> = line
        .split(',')
        .map(|n| {
            n.trim().parse::<usize>().ok().filter(|n| (1..=question.choices.len()).contains(n))
        })
        .collect();
    let chosen: Option<Vec<String>> = match numbers {
        Some(numbers) => {
            Some(numbers.into_iter().map(|n| question.choices[n - 1].label.clone()).collect())
        }
        None => question.choices.iter().find(|c| c.label == line).map(|c| vec![c.label.clone()]),
    };
    match chosen {
        Some(chosen) => Response::Chosen { chosen, other: None },
        None => Response::Chosen { chosen: Vec::new(), other: Some(line.to_string()) },
    }
}

/// What happened in the talk: in the channels followed, or to the shell command run in the
/// workspace.
pub(crate) enum Occurred {
    /// What happened in the channels followed - boxed, being much larger.
    InChannels(Box<Happened>),
    /// The shell command ended (`/workspace run`), as the channel it runs in answered it.
    Ran {
        /// The channel it runs in.
        channel_id: ChannelId,
        /// Its name.
        channel: Name,
        /// What the channel answered.
        reply: Result<CommandReply>,
    },
}

/// A shell command run in the workspace under way (`/workspace run`, issue 587): the channel it
/// runs in, and the task that answers once it ended - aborted where it is dropped, which kills it.
struct ShellCommand {
    /// The channel it runs in.
    channel_id: ChannelId,
    /// Its name.
    channel: Name,
    /// The task, which runs `/workspace run` by the user's actor.
    task: AbortOnDropHandle<Result<CommandReply>>,
}

/// What a line typed came to.
pub(crate) enum Handled {
    /// What the terminal prints of it.
    Printed(Vec<Printed>),
    /// The user ends the program (`/exit`).
    Exit,
}

/// The talk of the user in the terminal (issue 571): the channels of the session it talks in that
/// it follows, what is asked of the user, the sessions of the directory, what the user can type
/// (issue 579) - what a line typed and what a channel shows change -, and the shell command they
/// run (issue 587).
pub(crate) struct Talk {
    /// The channels followed.
    channels: Channels,
    /// What is asked of the user now (`ask_next`), shown or put aside: while questions are shown,
    /// every line but a command answers the next; while an approval is, the number of a choice
    /// answers it.
    asked: Option<Asked>,
    /// The sessions of the directory, which the commands on sessions reach.
    sessions: Sessions,
    /// The user's controller, which keeps their rules (`approve_from_now_on`), and their
    /// context.
    user: Arc<UserController>,
    /// The commands the terminal carries out itself (`commands`).
    commands: CommandSet<dyn TerminalCommand>,
    /// What the user can type, as last read (issue 579): what the screen completes from.
    typeable: Arc<Typeable>,
    /// The shell command run in the workspace under way, if one is (issue 587), followed by
    /// `next`.
    shell_command: Option<ShellCommand>,
    /// The threads `/thread list` showed last, in its order, with the channel they are of: what
    /// the number `/thread follow` is given names (issue 595), as a number names a choice shown.
    listed_threads: Option<(ChannelId, Vec<ThreadId>)>,
}

impl Talk {
    /// Begins the talk of the user `views` are read by in the channels of `session_loop` they take
    /// part in - learning from `bus` of those they join meanwhile -, among the `sessions` of the
    /// directory, keeping their rules by their controller `user`, and answers what the terminal
    /// opens with (`Channels::opening`).
    ///
    /// # Errors
    /// If the user takes part in no channel of the session, or the terminal's commands cannot be
    /// told apart (`commands`).
    pub(crate) async fn begin(
        session_loop: Arc<SessionLoop>,
        views: Views,
        bus: Arc<EventBus>,
        sessions: Sessions,
        user: Arc<UserController>,
    ) -> Result<(Talk, Vec<Printed>)> {
        let session_id = session_loop.id();
        let mut channels =
            Channels::new(session_loop, views.clone(), bus).await.ok_or_else(|| {
                anyhow::anyhow!("The user takes part in no channel of the session {session_id}")
            })?;
        let opening = channels.opening().await;
        let commands = commands()?;
        let typeable = Arc::new(Typeable::default());
        let mut talk = Talk {
            channels,
            asked: None,
            sessions,
            user,
            commands,
            typeable,
            shell_command: None,
            listed_threads: None,
        };
        talk.ask_next().await;
        talk.read_typeable().await;
        Ok((talk, opening))
    }

    /// What happens next: in the channels followed (`Channels::next`), or the shell command run in
    /// the workspace ending. Nothing is lost if it is dropped before it completes.
    pub(crate) async fn next(&mut self) -> Occurred {
        let shell_command = &mut self.shell_command;
        let ended = async {
            match shell_command.as_mut() {
                Some(command) => {
                    let reply = (&mut command.task).await;
                    let reply = reply.map_err(anyhow::Error::from).and_then(|reply| reply);
                    let (channel_id, channel) = (command.channel_id, command.channel.clone());
                    Occurred::Ran { channel_id, channel, reply }
                }
                None => future::pending().await,
            }
        };
        let ran = tokio::select! {
            happened = self.channels.next() => return Occurred::InChannels(Box::new(happened)),
            ran = ended => ran,
        };
        self.shell_command = None;
        ran
    }

    /// Takes in what `occurred`, and answers what the terminal prints of it: of the shell command
    /// run in the workspace, how it ended, as the channel answered the user - told as of its
    /// channel where the user talks in another (`take_in_channels` for the rest).
    pub(crate) async fn take_in(&mut self, occurred: Occurred) -> Vec<Printed> {
        let (channel_id, channel, reply) = match occurred {
            Occurred::InChannels(happened) => return self.take_in_channels(*happened).await,
            Occurred::Ran { channel_id, channel, reply } => (channel_id, channel, reply),
        };
        let printed = match reply {
            Ok(reply) => told(reply.output, None),
            Err(e) => vec![error_printed(&e)],
        };
        match self.channels.talked_in().map(Followed::channel_id) == Some(channel_id) {
            true => printed,
            false => Printed::elsewhere(&channel, printed),
        }
    }

    /// Ends the shell command run in the workspace, if one is under way, by aborting it, which
    /// kills it (issue 587), and answers what tells so; `None` where none is. Ended in the moment
    /// it ends by itself, it may have ended already - or have been told in the channel.
    pub(crate) fn end_shell_command(&mut self) -> Option<Printed> {
        let ended = self.shell_command.take()?;
        let told = Printed::Notice("The command is ended".to_string());
        Some(match self.channels.talked_in().map(Followed::channel_id) == Some(ended.channel_id) {
            true => told,
            false => Printed::Elsewhere { channel: ended.channel, printed: Box::new(told) },
        })
    }

    /// Takes in what `happened` in the channels followed, and answers what the terminal prints of
    /// it (`Channels::take_in`) - with that the questions asked closed, where their channel tells
    /// so or is followed no more; then asks what waits next (`ask_next`).
    async fn take_in_channels(&mut self, happened: Happened) -> Vec<Printed> {
        let changes_typeable = self.changes_typeable(&happened);
        let mut printed = self.channels.take_in(happened).await;
        if changes_typeable {
            self.read_typeable().await;
        }
        if let Some(Asking::Question { request, .. }) = self.asking() {
            if !self.is_open(request) {
                printed.push(closed_before_answered(request));
            }
        }
        self.ask_next().await;
        printed
    }

    /// What is asked of the user now, shown or put aside.
    pub(crate) fn asked(&self) -> Option<&Asked> {
        self.asked.as_ref()
    }

    /// Puts what is asked aside: shown as waiting only, and no line an answer to it, until it is
    /// taken up again (`take_up`, `/question answer`) or something else is asked.
    pub(crate) fn put_aside(&mut self) {
        self.asked = self.asked.take().map(Asked::put_aside);
    }

    /// Takes what was put aside up again.
    pub(crate) fn take_up(&mut self) {
        self.asked = self.asked.take().map(Asked::taken_up);
    }

    /// What is asked of the user, shown or put aside.
    fn asking(&self) -> Option<&Asking> {
        self.asked.as_ref().map(Asked::asking)
    }

    /// What is asked of the user and shown: what a line typed answers.
    fn shown(&self) -> Option<&Asking> {
        self.asked.as_ref().and_then(Asked::shown)
    }

    /// Takes the choices of what is asked, by their indices, `chosen`, as the answer to it - each
    /// by what it means (`Asking::choices`) -, and answers what the terminal prints of it: an
    /// approval approved, approved with the calls alike from now on (`approve_from_now_on`), or
    /// denied; the next question answered by the choices taken. Then asks what waits next.
    pub(crate) async fn choose(&mut self, chosen: &[usize]) -> Vec<Printed> {
        let Some(asking) = self.shown().cloned() else { return Vec::new() };
        let offered = asking.choices();
        let chosen: Vec<&Offered> = chosen.iter().filter_map(|index| offered.get(*index)).collect();
        let printed = match (&asking, chosen.as_slice()) {
            (Asking::Approval(request), [Offered::Approve]) => {
                self.approve(request, true).await.err().into_iter().collect()
            }
            (Asking::Approval(request), [Offered::Deny]) => {
                self.approve(request, false).await.err().into_iter().collect()
            }
            (Asking::Approval(request), [Offered::ApproveFromNowOn { matcher, reach }]) => {
                self.approve_from_now_on(request, matcher, *reach).await
            }
            (Asking::Question { .. }, chosen) => {
                let labels = chosen.iter().filter_map(|offered| match offered {
                    Offered::Choice(choice) => Some(choice.label.clone()),
                    _ => None,
                });
                let response = Response::Chosen { chosen: labels.collect(), other: None };
                self.answer_question(response).await
            }
            (Asking::Approval(_), _) => Vec::new(),
        };
        self.ask_next().await;
        printed
    }

    /// Asks what waits next for the user in the channel talked in, where what is asked is no longer
    /// to be decided - or nothing is (`next_to_ask`). What is asked and still to be decided stays
    /// asked, also when put aside.
    async fn ask_next(&mut self) {
        let still = match self.asking() {
            Some(Asking::Approval(request)) => self.is_pending(request),
            Some(Asking::Question { request, .. }) => self.is_open(request),
            None => false,
        };
        if !still {
            self.asked = self.next_to_ask().await.map(Asked::Shown);
        }
    }

    /// What waits for the user in the channel talked in, as `Followed` tells it: the oldest
    /// approval pending for them there of an agent whose calls they may approve
    /// (`Followed::may_approve`), else the oldest questions still theirs to answer there.
    async fn next_to_ask(&mut self) -> Option<Asking> {
        let talked_in = self.channels.talked_in_mut()?;
        for request in talked_in.pending_approvals().to_vec() {
            if talked_in.may_approve(request.agent_id).await {
                return Some(Asking::Approval(request));
            }
        }
        talked_in.questions_to_answer().into_iter().next().map(Asking::questions)
    }

    /// Whether the approval `request` is still pending for the user.
    fn is_pending(&self, request: &ApprovalRequest) -> bool {
        self.channels.awaiting(request.id).is_some()
    }

    /// Whether the questions `request` are still the user's to answer in their channel.
    fn is_open(&self, request: &QuestionRequest) -> bool {
        let channel = self.channels.followed(request.channel_id);
        channel.is_some_and(|f| f.is_to_answer(request.id))
    }

    /// What is at work in the channels followed (`Channels::activity`).
    pub(crate) fn activity(&self) -> Activity {
        self.channels.activity()
    }

    /// Does what the user typed, `input`, read as the terminal reads it (`read_input`), and answers
    /// what it came to: a command of the terminal carried out - one refused arguments it does not
    /// take, or none where it requires one; one on the sessions of the directory asked of them -;
    /// while questions are asked, any other message the answer to the next, while an approval is,
    /// the number of a choice its answer; else the line sent to the channel talked in
    /// (`send_line`). Then asks what waits next.
    ///
    /// # Errors
    /// As `send_line`.
    pub(crate) async fn take_line(&mut self, input: &str) -> Result<Handled> {
        if input.is_empty() {
            return Ok(Handled::Printed(Vec::new()));
        }
        let parsed = read_input(input);
        let terminal = match &parsed {
            UserInput::Command(line) => {
                self.commands.find(line).map(|(command, args)| (Arc::clone(command), args))
            }
            UserInput::Message(_) => None,
        };
        let chosen = self.chosen_by(input);
        let printed = match terminal {
            Some((command, args)) => {
                if let Err(e) = check_arguments(command.description(), args) {
                    return Ok(Handled::Printed(vec![Printed::Error {
                        kind: ErrorKind::Failed,
                        message: e.to_string(),
                    }]));
                }
                match command.execute(args, self).await {
                    Handled::Printed(printed) => printed,
                    Handled::Exit => return Ok(Handled::Exit),
                }
            }
            // While questions are asked, each message answers the next; a command of the channel
            // goes to it.
            None if matches!(self.shown(), Some(Asking::Question { .. }))
                && matches!(parsed, UserInput::Message(_)) =>
            {
                let UserInput::Message(text) = &parsed else { unreachable!("a message") };
                self.answer_in_words(text).await
            }
            None if chosen.is_some() => self.choose(chosen.as_slice()).await,
            None => self.send_line(&parsed).await?,
        };
        self.ask_next().await;
        // What it did may have changed what can be typed: an agent made, a channel switched to.
        self.read_typeable().await;
        Ok(Handled::Printed(printed))
    }

    /// What the user can type now, as last read (`read_typeable`): what the screen completes from.
    pub(crate) fn typeable(&self) -> Arc<Typeable> {
        Arc::clone(&self.typeable)
    }

    /// Whether `happened` may change what the user can type: the channels they take part in, or
    /// who takes part in the channel talked in - a participant joined or left, a run of its user's
    /// actor began or ended, or what it showed was lost. Not everything it shows: the commands they
    /// may see are asked of the policy one by one.
    fn changes_typeable(&self, happened: &Happened) -> bool {
        let (channel_id, shown) = match happened {
            Happened::TakingPart => return true,
            Happened::Shown(channel_id, shown) => (*channel_id, shown),
        };
        if self.channels.talked_in().map(Followed::channel_id) != Some(channel_id) {
            return false;
        }
        match shown {
            Ok(Shown::Envelope(envelope)) => matches!(
                envelope.event,
                ActorEvent::ParticipantJoined { .. } | ActorEvent::ParticipantLeft { .. }
            ),
            Ok(Shown::Started | Shown::Ended) | Err(_) => true,
        }
    }

    /// Reads anew what the user can type (issue 579): the terminal's commands, and what its owners
    /// add of the session and the channel talked in (`Channels::add_typeable`) and of the
    /// directory (`Sessions::add_typeable`).
    async fn read_typeable(&mut self) {
        let mut typeable = Typeable::default();
        typeable.add_commands(self.commands.descriptions());
        self.channels.add_typeable(&mut typeable).await;
        self.sessions.add_typeable(&mut typeable).await;
        self.typeable = Arc::new(typeable);
    }

    /// The choice of what is asked - an approval - that `input` names by its number; `None` where
    /// nothing is asked, or `input` names none of its choices.
    fn chosen_by(&self, input: &str) -> Option<usize> {
        let asking @ Asking::Approval(_) = self.shown()? else { return None };
        let number = input.trim().parse::<usize>().ok()?;
        (1..=asking.choices().len()).contains(&number).then(|| number - 1)
    }

    /// Talks in the channel named `name` from now on (`Channels::switch`), and answers what tells
    /// so; what was asked in the channel left is asked no more, where the switch is made.
    pub(crate) async fn switch_channel(&mut self, name: &str) -> Vec<Printed> {
        let left = self.channels.talked_in().map(Followed::channel_id);
        let printed = self.channels.switch(name).await;
        if self.channels.talked_in().map(Followed::channel_id) != left {
            self.asked = None;
        }
        printed
    }

    /// What lists the threads open in the channel talked in (issue 595), numbered as
    /// `/thread follow` names them, the one followed marked; as `talking` where no channel is
    /// talked in.
    pub(crate) async fn list_threads(&mut self) -> Vec<Printed> {
        let followed = match self.channels.talked_in() {
            Some(followed) => followed,
            None => return vec![self.channels.hint()],
        };
        let threads = followed.open_threads().await;
        let ids = threads.iter().map(|thread| thread.id).collect();
        self.listed_threads = Some((followed.channel_id(), ids));
        if threads.is_empty() {
            return vec![Printed::Notice(format!("No thread is open in {}", followed.name()))];
        }
        let lines = threads.iter().enumerate().map(|(at, thread)| {
            let mark = if thread.id == followed.thread() { " (followed)" } else { "" };
            format!("{}. {} - {}{mark}", at + 1, thread.told, thread.id.0)
        });
        vec![Printed::Plain(lines.collect::<Vec<_>>().join("\n"))]
    }

    /// Follows the thread `which` of the channel talked in (issue 595) - its number in
    /// `/thread list`, or the beginning of its id -, and answers what tells so, or why not.
    pub(crate) async fn follow_thread(&mut self, which: &str) -> Vec<Printed> {
        let listed = self.listed_threads.clone();
        let Some(followed) = self.channels.talked_in_mut() else {
            return vec![self.channels.hint()];
        };
        let threads = followed.open_threads().await;
        // A number names a thread of the list shown last in this channel, an id what is open.
        let numbered = which.trim().parse::<usize>().ok().and_then(|n| n.checked_sub(1));
        let listed = listed.filter(|(channel, _)| *channel == followed.channel_id());
        let chosen = numbered.and_then(|at| listed.and_then(|(_, ids)| ids.get(at).copied()));
        let found = match chosen {
            Some(id) => threads.into_iter().find(|thread| thread.id == id).ok_or_else(|| {
                Printed::Notice(format!("Thread {which} of the list is no longer open"))
            }),
            None => beginning_with(threads, |thread| thread.id.0.to_string(), which)
                .map_err(|e| Printed::Notice(e.told("open threads"))),
        };
        match found {
            Ok(thread) => followed.follow(thread.id, &thread.title).await,
            Err(printed) => vec![printed],
        }
    }

    /// Follows the line of the channel talked in again (issue 595), and answers what tells so.
    pub(crate) async fn follow_line(&mut self) -> Vec<Printed> {
        let Some(followed) = self.channels.talked_in_mut() else {
            return vec![self.channels.hint()];
        };
        let line = ThreadId::line(followed.channel_id());
        followed.follow(line, "the line").await
    }

    /// What tells of the sessions of the directory, the one spoken in last first, the one talked in
    /// marked as such.
    pub(crate) async fn listed(&self) -> Vec<Printed> {
        match self.sessions.list().await {
            Ok(listed) => {
                let talked_in = self.channels.session_id();
                vec![Printed::Plain(listing(&listed, Some(talked_in)).join("\n"))]
            }
            Err(e) => vec![error_printed(&e.into())],
        }
    }

    /// Deletes the session of the directory whose id begins with `id` (`Sessions::delete`) - not
    /// the one talked in -, and answers what tells so.
    pub(crate) async fn delete_session(&self, id: &str) -> Vec<Printed> {
        match self.sessions.delete(id, self.channels.session_id()).await {
            Ok(id) => vec![Printed::Notice(format!("The session {id} is deleted"))],
            Err(e) => vec![error_printed(&e.into())],
        }
    }

    /// Talks in the session of the directory that `opening` asks for from now on, its channels
    /// followed in place of those of the session left (`Channels::of_session`), what was asked
    /// there asked no more, a shell command run there ended (`end_shell_command`), and the session
    /// left stopped;
    /// answers what tells so, as the terminal opens with. Where the session cannot be opened, or
    /// the user takes part in none of its channels, or it is the one talked in already, the talk
    /// stays where it is, and the user is told why.
    pub(crate) async fn switch_session(&mut self, opening: &Opening) -> Vec<Printed> {
        let left = self.channels.session_id();
        let (session, told) = match self.sessions.open(opening).await {
            Ok(opened) => opened,
            Err(e) => return vec![error_printed(&e.into())],
        };
        let id = session.id();
        if id == left {
            return vec![Printed::Notice(format!("You talk in the session {id} already"))];
        }
        let Some(mut followed) = self.channels.of_session(session).await else {
            if let Err(e) = self.sessions.stop(id).await {
                let e = anyhow::Error::from(e);
                warn!(
                    "The session {id} the terminal did not switch to could not be stopped: {e:#}"
                );
            }
            let message = format!("You take part in no channel of the session {id}");
            return vec![Printed::Error { kind: ErrorKind::Failed, message }];
        };
        // Ended first, told of the channel it ran in as the one talked in.
        let mut printed: Vec<Printed> = self.end_shell_command().into_iter().collect();
        printed.extend(told);
        printed.push(Printed::Notice(format!("Talking in the session {id} now")));
        printed.extend(followed.opening().await);
        self.channels = followed;
        self.asked = None;
        if let Err(e) = self.sessions.stop(left).await {
            printed.push(error_printed(&e.into()));
        }
        printed
    }

    /// Answers the approval `named` - `approved` or not (`approval_named`) -, and answers what the
    /// terminal prints of it.
    pub(crate) async fn answer_approval(&mut self, named: &str, approved: bool) -> Vec<Printed> {
        let approved = match self.approval_named(named) {
            Ok(request) => self.approve(&request, approved).await,
            Err(printed) => Err(printed),
        };
        approved.err().into_iter().collect()
    }

    /// The approval `named` by the beginning of its id among those pending in the channels
    /// followed - without one, the approval asked -; what tells why not, where none or several are.
    fn approval_named(&self, named: &str) -> Result<ApprovalRequest, Printed> {
        let prefix = named.trim();
        if prefix.is_empty() {
            return match self.asking() {
                Some(Asking::Approval(request)) => Ok(request.clone()),
                _ => Err(Printed::Plain(
                    "No approval is asked of you: name one by the beginning of its id".to_string(),
                )),
            };
        }
        let pending = self.channels.pending_approvals();
        let named = beginning_with(pending, |request| request.id.0.to_string(), prefix);
        named.cloned().map_err(|not| Printed::Plain(not.told("pending approvals")))
    }

    /// Answers the approval `request` - `approved` or not - in the channel followed where it is
    /// pending (`Followed::approve`); once answered, it is asked no more.
    ///
    /// # Errors
    /// What the terminal prints where it is pending no more, the user's actor there is between two
    /// runs, or the answer cannot be sent.
    async fn approve(&mut self, request: &ApprovalRequest, approved: bool) -> Result<(), Printed> {
        let Some(followed) = self.channels.awaiting_mut(request.id) else {
            return Err(Printed::Plain(format!("No pending approval with id {}", request.id.0)));
        };
        let actor = acting(followed)?;
        followed.approve(&actor, request.id, approved).await.map_err(|e| error_printed(&e))
    }

    /// Approves the call of `request`, then adds the user's rule that approves the calls `matcher`
    /// is about from now on within `reach` (`UserController::add_approval_rule`), and answers what
    /// the terminal prints of it: the answer first, as the web UI gives it - where the rule cannot
    /// be added, the answer stands, and the user is told so, with what of the cause may be told
    /// (`error_printed`).
    async fn approve_from_now_on(
        &mut self,
        request: &ApprovalRequest,
        matcher: &ToolMatcher,
        reach: Reach,
    ) -> Vec<Printed> {
        if let Err(printed) = self.approve(request, true).await {
            return vec![printed];
        }
        let rule = ApprovalRule::new(matcher.clone(), RuleDecision::Allow, reach, None);
        let context = self.user.security_context(Default::default(), Default::default());
        let added = match rule {
            Ok(rule) => {
                self.user.add_approval_rule(&context, rule).await.map_err(anyhow::Error::from)
            }
            Err(e) => Err(anyhow::Error::from(e)),
        };
        let Err(e) = added else { return Vec::new() };
        let approved = "The call is approved, but the rule for the calls alike could not be added";
        vec![Printed::Notice(approved.to_string()), error_printed(&e)]
    }

    /// Asks, in the channel talked in, for the answer under way, the turn given up or the last
    /// answer of an agent again (`Followed::retry`), and answers what the terminal prints of it.
    pub(crate) async fn retry(&self) -> Vec<Printed> {
        let (followed, actor) = match self.talking() {
            Ok(talking) => talking,
            Err(printed) => return vec![printed],
        };
        match followed.retry(&actor).await {
            Ok(true) => Vec::new(),
            Ok(false) => vec![Printed::Plain("No answer of an agent to give again".to_string())],
            Err(e) => vec![error_printed(&e)],
        }
    }

    /// The questions `named` by the beginning of their id among those put to the user in the
    /// channels followed (`Channels::questions_to_answer`) - without one, those asked, also when
    /// put aside -, with the user's actor in their channel; what tells why not, where none or
    /// several are, or no run of it.
    fn questions_named(&self, named: &str) -> Result<(QuestionRequest, Arc<UserActor>), Printed> {
        let asked = match self.asking() {
            Some(Asking::Question { request, .. }) => Some(request.id.0.to_string()),
            _ => None,
        };
        let named = match (named.trim(), asked) {
            ("", Some(asked)) => asked,
            ("", None) => {
                let none = "No questions are asked of you: name them by the beginning of their id";
                return Err(Printed::Plain(none.to_string()));
            }
            (named, _) => named.to_string(),
        };
        let to_answer = self.channels.questions_to_answer();
        let (followed, request) = beginning_with(to_answer, |(_, q)| q.id.0.to_string(), &named)
            .map_err(|not| Printed::Plain(not.told("open questions put to you")))?;
        Ok((request, acting(followed)?))
    }

    /// Declines the questions `id` names (`questions_named`) - without one, those asked -, and
    /// answers what the terminal prints of it; once declined, they are asked no more.
    pub(crate) async fn decline_questions(&self, id: &str) -> Vec<Printed> {
        let (request, actor) = match self.questions_named(id) {
            Ok(named) => named,
            Err(printed) => return vec![printed],
        };
        match actor.answer_questions(request.id, None).await {
            Ok(_) => Vec::new(),
            Err(e) => vec![error_printed(&e)],
        }
    }

    /// Asks the questions `id` names (`questions_named`) from now on - without one, those
    /// asked -, taken up where they were put aside, and answers what tells why not, where there
    /// are none.
    pub(crate) fn begin_answering(&mut self, id: &str) -> Vec<Printed> {
        match self.questions_named(id) {
            Ok((request, _)) => {
                let asked = matches!(
                    self.asking(),
                    Some(Asking::Question { request: asked, .. }) if asked.id == request.id
                );
                match asked {
                    true => self.take_up(),
                    false => self.asked = Some(Asked::Shown(Asking::questions(request))),
                }
                Vec::new()
            }
            Err(printed) => vec![printed],
        }
    }

    /// Takes `input` as the answer in words to the question asked (`response_of`).
    async fn answer_in_words(&mut self, input: &str) -> Vec<Printed> {
        let Some((_, question)) = self.shown().and_then(Asking::question) else {
            return Vec::new();
        };
        let response = response_of(question, input);
        self.answer_question(response).await
    }

    /// Takes `response` as the answer to the question asked; the next of its questions is asked
    /// then, or - the last answered - the answers are sent in the channel that asked them, after
    /// which they are asked no more. Between two runs of the user's actor there, only that nothing
    /// can be done there now, the last question still asked.
    async fn answer_question(&mut self, response: Response) -> Vec<Printed> {
        let Some(Asking::Question { request, mut responses }) =
            self.asked.take().map(Asked::into_asking)
        else {
            return Vec::new();
        };
        responses.push(response);
        if responses.len() < request.questions.questions.len() {
            self.asked = Some(Asked::Shown(Asking::Question { request, responses }));
            return Vec::new();
        }
        // Closed with its channel, if it is followed no more.
        let Some(followed) = self.channels.followed(request.channel_id) else {
            return vec![closed_before_answered(&request)];
        };
        match acting(followed) {
            Ok(actor) => match actor.answer_questions(request.id, Some(responses)).await {
                Ok(_) => Vec::new(),
                Err(e) => vec![error_printed(&e)],
            },
            // The last answer is given again once the user's actor runs.
            Err(no_run) => {
                responses.pop();
                self.asked = Some(Asked::Shown(Asking::Question { request, responses }));
                vec![no_run]
            }
        }
    }

    /// The channel talked in, with the user's actor in it as it runs now; what tells why not, if it
    /// talks in none, or no run of it is under way (`acting`).
    fn talking(&self) -> Result<(&Followed, Arc<UserActor>), Printed> {
        let followed = self.channels.talked_in().ok_or_else(|| self.channels.hint())?;
        Ok((followed, acting(followed)?))
    }

    /// Sends what the user typed, `input`, to the channel talked in as a command or a message, and
    /// answers what the terminal prints of it - of `/help`, what the terminal tells of its own
    /// commands (`terminal_help`) after what the channel answers, or in its place where the channel
    /// knows nothing of what was asked or no channel is talked in; of `/workspace run`, that it
    /// runs (`run_shell_command`).
    ///
    /// # Errors
    /// If a message cannot be sent into the channel.
    async fn send_line(&mut self, input: &UserInput) -> Result<Vec<Printed>> {
        let help = match input {
            UserInput::Command(line) => CommandName::fixed(HELP_COMMAND_NAME)
                .strip_from(line)
                .and_then(|args| terminal_help(&self.commands, args)),
            UserInput::Message(_) => None,
        };
        // What is sent goes to the thread followed (issue 595).
        let (thread, actor) = match self.talking() {
            Ok((followed, actor)) => (Some(followed.thread()), actor),
            Err(printed) => return Ok(vec![help.map_or(printed, Printed::Plain)]),
        };
        let run = CommandName::fixed(RUN_COMMAND_NAME);
        match input {
            UserInput::Command(line) if run.strip_from(line).is_some() => {
                Ok(vec![self.run_shell_command(&run, line).await])
            }
            UserInput::Command(line) => Ok(match actor.execute_command(thread, line).await {
                // Also for an answer to the whole channel: its announcement is not routed back to
                // its sender.
                Ok(reply) => told(reply.output, help),
                Err(e) if help.is_some() && is_unknown(&e) => told(None, help),
                // The line is not logged: what follows the command may be what was said. The error
                // names the command.
                Err(e) => vec![error_printed(&e)],
            }),
            UserInput::Message(text) => {
                let span_id =
                    actor.send_message(thread, Some(text.clone()), None, Vec::new()).await?;
                tracing::debug!(span_id = %span_id, "Message sent");
                Ok(Vec::new())
            }
        }
    }

    /// Runs the command line `line`, `/workspace run` - named `run` -, by the user's actor in the
    /// channel talked in, under way until it ends (`next`), and answers what tells that it runs -
    /// or why not: one runs already, the channel offers the user no `/workspace run`, which a
    /// configuration of the program may leave out and which they may have typed as `!`, or it is
    /// given no shell command (`check_arguments`); as `talking` where there is no run of the actor.
    async fn run_shell_command(&mut self, run: &CommandName, line: &str) -> Printed {
        let failed = |message: String| Printed::Error { kind: ErrorKind::Failed, message };
        if self.shell_command.is_some() {
            return Printed::Notice("A command runs already: Ctrl-C ends it".to_string());
        }
        let (channel_id, channel, thread, actor) = match self.talking() {
            Ok((followed, actor)) => {
                (followed.channel_id(), followed.name(), followed.thread(), actor)
            }
            Err(printed) => return printed,
        };
        let offered = actor.visible_commands().await;
        let Some(description) = offered.iter().find(|command| command.is_named(run)) else {
            return failed(format!("The channel offers you no /{run}, which `!` stands for"));
        };
        let args = run.strip_from(line).unwrap_or_default();
        if let Err(e) = check_arguments(description, args) {
            return failed(e.to_string());
        }
        let line = line.to_string();
        let task = AbortOnDropHandle::new(tokio::spawn(async move {
            actor.execute_command(Some(thread), &line).await
        }));
        self.shell_command = Some(ShellCommand { channel_id, channel, task });
        Printed::Notice("The command runs: Ctrl-C ends it".to_string())
    }
}

/// What the tests do with the talk past its own API: tell the session it talks in and whether
/// questions are asked, and reach the sessions of the directory.
#[cfg(test)]
impl Talk {
    /// The session talked in.
    fn session_id(&self) -> SessionId {
        self.channels.session_id()
    }

    /// Whether questions are asked, and shown.
    fn asks_questions(&self) -> bool {
        matches!(self.shown(), Some(Asking::Question { .. }))
    }

    /// The sessions of the directory.
    fn sessions(&self) -> &Sessions {
        &self.sessions
    }
}

/// What tells that the questions of `request` closed before the user answered them.
fn closed_before_answered(request: &QuestionRequest) -> Printed {
    Printed::Notice(format!("Questions {} closed before you answered", request.id.0))
}

/// The user's actor in `followed` as it runs now (`Followed::actor`); between two runs of it, what
/// tells the user that nothing can be done there right now.
fn acting(followed: &Followed) -> Result<Arc<UserActor>, Printed> {
    followed.actor().ok_or_else(|| {
        Printed::Notice(format!(
            "No run of yours in channel {} right now: try again in a moment",
            followed.name()
        ))
    })
}

/// What the terminal prints of what a command answered, `output`, and of what `help` tells after
/// it: nothing where there is neither - a command sent to agents answers nothing.
fn told(output: Option<String>, help: Option<String>) -> Vec<Printed> {
    let told: Vec<String> = output.into_iter().chain(help).collect();
    match told.is_empty() {
        true => Vec::new(),
        false => vec![Printed::Plain(told.join("\n\n"))],
    }
}

/// Whether `error` says that the channel knows no command of the line.
fn is_unknown(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| matches!(cause.downcast_ref(), Some(CommandError::Unknown(_))))
}

/// `error` as the user may be told it: of the sessions of the directory (`SessionsError`), what it
/// says - where the kernel failed, with what of its cause may be told -; else what `user_message`
/// tells. What may not be told stays in the log, where the user is told to find it, as does what
/// failed of the kernel; what the user asked amiss is not logged.
fn error_printed(error: &anyhow::Error) -> Printed {
    let of_sessions = error.chain().find_map(|cause| cause.downcast_ref::<SessionsError>());
    let told = user_message(error);
    let message = match (of_sessions, told) {
        (Some(failed @ SessionsError::Failed { .. }), told) => {
            warn!("Failed: {error:#}");
            match told {
                Some(told) => format!("{failed}: {told}"),
                None => failed.to_string(),
            }
        }
        (Some(asked_amiss), _) => asked_amiss.to_string(),
        (None, Some(told)) => told,
        (None, None) => {
            warn!("Failed: {error:#}");
            "It failed; the details are in the log".to_string()
        }
    };
    Printed::Error { kind: ErrorKind::Failed, message }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::followed::testing::{an_agent_in, approval, from};
    use crate::home;
    use crate::screen::testing::lines;
    use crate::sessions::shown_id;
    use crate::sessions::testing::{
        in_directory, in_directory_running_commands, in_directory_where, InDirectory,
    };
    use caelumora_domain::actor::{ActorEvent, ActorId};
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::approval::MAX_PERSONAL_RULES;
    use caelumora_domain::history::HistoryEntry;
    use caelumora_domain::question::testing::make_question_request;
    use caelumora_domain::question::Choice;
    use caelumora_kernel::actor::Shown;
    use caelumora_kernel::channel::ChannelLoop;
    use caelumora_kernel::kernel::Kernel;
    use caelumora_kernel::session::testing::spec;
    use caelumora_kernel::session::{SessionError, SessionLoop};
    use caelumora_kernel::view::Viewer;
    use caelumora_security::SecurityContext;
    use std::fs::File;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::time;
    use uuid::Uuid;

    fn question(choices: &[&str], multi_select: bool) -> Question {
        Question {
            header: "Pick".to_string(),
            question: "Which?".to_string(),
            choices: choices
                .iter()
                .map(|l| Choice { label: l.to_string(), description: None, recommended: false })
                .collect(),
            multi_select,
        }
    }

    #[test]
    fn a_line_answers_a_question_as_it_asks() {
        let chosen = |labels: &[&str]| Response::Chosen {
            chosen: labels.iter().map(|l| l.to_string()).collect(),
            other: None,
        };
        let other =
            |text: &str| Response::Chosen { chosen: Vec::new(), other: Some(text.to_string()) };
        let choices = question(&["Postgres", "SQLite", "None"], true);
        assert_eq!(response_of(&choices, "2"), chosen(&["SQLite"]));
        assert_eq!(response_of(&choices, "1, 3"), chosen(&["Postgres", "None"]));
        assert_eq!(response_of(&choices, "SQLite"), chosen(&["SQLite"]), "by its label");
        assert_eq!(response_of(&choices, "4"), other("4"), "no such choice: another answer");
        assert_eq!(response_of(&choices, "MariaDB"), other("MariaDB"));
        assert_eq!(response_of(&choices, "-"), Response::Declined);
        assert_eq!(
            response_of(&question(&[], false), "Kaya"),
            Response::Text { text: "Kaya".to_string() }
        );
        assert_eq!(response_of(&question(&[], false), "-"), Response::Declined);
    }

    /// What a talk runs in, kept as long as the test runs.
    struct Running {
        /// The kernel the session runs in.
        kernel: Arc<Kernel>,
        /// The owner of the directory, who talks.
        owner: SecurityContext,
        /// The directory.
        _directory: TempDir,
    }

    /// The talk of the owner of a directory in a session of theirs started from its template, the
    /// first channel of it talked in, with the session and what the talk runs in.
    async fn talking() -> (Talk, Arc<SessionLoop>, Running) {
        talking_in(in_directory().await).await
    }

    /// The same, in the directory given.
    async fn talking_in(in_directory: InDirectory) -> (Talk, Arc<SessionLoop>, Running) {
        let InDirectory { sessions, manager, kernel, owner, directory } = in_directory;
        let start = Opening::Start { template: None };
        let (session_loop, _) = sessions.open(&start).await.expect("started");
        let views = manager.views(Viewer::new(owner.clone()));
        let bus = Arc::clone(kernel.event_bus());
        let ActorId::User(user) = owner.caller_id else { unreachable!("a user") };
        let controller = kernel.controller_manager().user_controller(user).await;
        let user = controller.expect("the owner's controller");
        let begun = Talk::begin(Arc::clone(&session_loop), views, bus, sessions, user).await;
        let (talk, _) = begun.expect("the owner takes part");
        (talk, session_loop, Running { kernel, owner, _directory: directory })
    }

    /// The controller of the owner of `running`, the one the talk keeps their rules by.
    async fn owner_controller(running: &Running) -> Arc<UserController> {
        let ActorId::User(owner) = running.owner.caller_id else { unreachable!("a user") };
        let controller = running.kernel.controller_manager().user_controller(owner).await;
        controller.expect("the owner's controller")
    }

    /// Questions an agent puts to the owner of `running` in the first channel of `session_loop` -
    /// one, of the `choices` labelled so -, routed to their actor there as the channel would, and
    /// asked by `talk` as it takes in what the channel shows of them.
    async fn put_to_the_owner(
        talk: &mut Talk,
        session_loop: &SessionLoop,
        running: &Running,
        choices: &[&str],
    ) -> QuestionRequest {
        let me = running.owner.caller_id;
        let channel = session_loop.channels().into_iter().next().expect("its first channel");
        let ActorId::User(user) = me else { unreachable!("a user") };
        let actor = channel.user_actor(user).expect("the owner's actor runs");
        let agent = AgentId::new();
        let mut request = make_question_request(session_loop.id(), channel.id(), agent, me);
        for question in &mut request.questions.questions {
            question.choices = choices
                .iter()
                .map(|label| Choice {
                    label: label.to_string(),
                    description: None,
                    recommended: false,
                })
                .collect();
        }
        let asked = ActorEvent::QuestionRequested(request.clone());
        actor.take_in_as_routed(from(&channel, ActorId::Agent(agent), asked)).await;
        let asked = Asking::questions(request.clone());
        let until = async {
            while talk.asking() != Some(&asked) {
                let occurred = talk.next().await;
                talk.take_in(occurred).await;
            }
        };
        time::timeout(Duration::from_secs(5), until).await.expect("asked");
        request
    }

    /// What the terminal prints of `input` typed in `talk`.
    async fn typed(talk: &mut Talk, input: &str) -> Vec<String> {
        match talk.take_line(input).await.expect("handled") {
            Handled::Printed(printed) => lines(&printed),
            Handled::Exit => panic!("{input} ends the program"),
        }
    }

    /// What a command answered is printed with what the terminal tells after it, nothing where
    /// there is neither.
    #[test]
    fn what_a_command_answered_is_printed_only_where_there_is_something() {
        assert!(told(None, None).is_empty(), "a command sent to agents");
        let both = told(Some("theirs".to_string()), Some("mine".to_string()));
        assert_eq!(lines(&both), ["theirs\n\nmine"]);
        assert_eq!(lines(&told(None, Some("mine".to_string()))), ["mine"]);
    }

    /// Issue 579: what can be typed is read as the talk begins - the terminal's commands and the
    /// channel's, its participants, the channels and sessions -, and read anew after a line.
    #[tokio::test]
    async fn what_can_be_typed_is_read_as_the_talk_begins_and_after_a_line() {
        let (mut talk, session_loop, _running) = talking().await;
        let candidates =
            |talk: &Talk, line: &str| talk.typeable().complete(line, line.len()).candidates;
        assert_eq!(candidates(&talk, "/session res"), ["session resume"], "the terminal's");
        assert!(
            candidates(&talk, "/participant l").contains(&"participant list".to_string()),
            "the channel's"
        );
        let channel = session_loop.channels().into_iter().next().expect("its first channel");
        assert_eq!(candidates(&talk, "/channel switch "), [channel.name().to_string()]);
        let resumed = candidates(&talk, "/session resume ");
        assert_eq!(
            resumed,
            [shown_id(session_loop.id())],
            "the session of the directory, as listed"
        );
        let handles = channel.participants().into_iter().filter_map(|p| p.handle);
        let mut expected: Vec<String> = handles.map(|handle| format!("@{handle}")).collect();
        expected.extend(["@all".to_string(), "@channel".to_string()]);
        expected.sort();
        assert_eq!(candidates(&talk, "hi @"), expected, "its participants, and the group handles");

        typed(&mut talk, "/channel create side").await;
        assert!(
            candidates(&talk, "/channel join s").contains(&"side".to_string()),
            "a channel made since: {:?}",
            candidates(&talk, "/channel join ")
        );
    }

    /// Issue 579: who joins the channel talked in is offered once the channel shows it, without a
    /// line typed - an agent by its handle, with what it is offered as.
    #[tokio::test]
    async fn who_joins_the_channel_talked_in_is_offered_as_the_channel_shows_it() {
        let (mut talk, session_loop, running) = talking().await;
        let channel = session_loop.channels().into_iter().next().expect("its first channel");
        let offered = |talk: &Talk| talk.typeable().complete("/agent usage ", 13).candidates;
        assert!(offered(&talk).is_empty(), "no agent yet");
        an_agent_in(&running.kernel, &session_loop, &running.owner, channel.id()).await;
        let until = async {
            while offered(&talk).is_empty() {
                let occurred = talk.next().await;
                talk.take_in(occurred).await;
            }
        };
        time::timeout(Duration::from_secs(5), until).await.expect("offered once it joined");
        assert_eq!(offered(&talk), ["@helper"]);
    }

    /// `/exit` and `/quit` end the program, not given arguments, which they take none of; the bare
    /// words are messages.
    #[tokio::test]
    async fn the_program_ends_by_a_command_not_by_a_word() {
        let (mut talk, _, _running) = talking().await;
        for exit in ["/exit", "/quit"] {
            let handled = talk.take_line(exit).await.expect("handled");
            assert!(matches!(handled, Handled::Exit), "{exit}");
        }
        let refused = typed(&mut talk, "/exit now").await;
        assert_eq!(refused, ["Error: /exit: it takes no arguments"]);
        assert!(typed(&mut talk, "exit").await.is_empty(), "said");
    }

    /// `/help` tells the terminal's commands after the channel's, and `!` (issue 587); of one
    /// named, or of a group of the terminal alone, what the terminal tells.
    #[tokio::test]
    async fn help_tells_the_terminal_s_commands() {
        let (mut talk, _, _running) = talking().await;
        let all = typed(&mut talk, "/help").await.join("\n");
        let theirs = all.find("/participant list").expect("the channel's");
        let mine = all.find("Commands of this terminal:").expect("the terminal's");
        assert!(theirs < mine, "after the channel's: {all}");
        assert!(all.contains("- `/channel switch [channel]` (also `/switch`) — Talks"), "{all}");
        assert!(all.contains("- `!<command>` — Short for `/workspace run <command>`"), "{all}");
        let one = typed(&mut talk, "/help switch").await.join("\n");
        assert!(one.starts_with("`/channel switch [channel]` (also `/switch`)"), "{one}");
        let group = typed(&mut talk, "/help approval").await.join("\n");
        assert!(group.contains("/approval approve") && group.contains("/approval deny"), "{group}");
        assert!(!group.contains("/exit"), "{group}");
        let unknown = typed(&mut talk, "/help nothing").await.join("\n");
        assert!(unknown.contains("Unknown command"), "{unknown}");
    }

    /// Issue 595: `/thread list` lists the threads open in the channel talked in, `/thread follow`
    /// follows one by its number - what is typed goes there -, `/thread line` the line again; a
    /// thread named badly is told so.
    #[tokio::test]
    async fn a_thread_is_listed_followed_and_left_for_the_line() {
        use caelumora_domain::channel::ThreadOrigin;
        use caelumora_kernel::channel::ThreadSpec;
        let (mut talk, session_loop, running) = talking().await;
        let channel = session_loop.channels().into_iter().next().expect("its first channel");
        let ActorId::User(user) = running.owner.caller_id else { unreachable!("a user") };
        let line = ThreadId::line(channel.id());
        let origin = ThreadOrigin { thread: line, message: channel.said_by(user, line).await };
        let opening = channel.open_thread_by_procedure(&running.owner, ThreadSpec::new(origin));
        let thread = opening.await.expect("opened");

        channel.said_by(user, thread).await;
        let listed = typed(&mut talk, "/thread list").await.join("\n");
        assert!(
            listed.starts_with("1. Thread (") && listed.contains(&thread.0.to_string()),
            "{listed}"
        );
        let followed = typed(&mut talk, "/thread follow 1").await;
        assert!(
            followed.last().is_some_and(|told| told.starts_with("Following Thread")),
            "{followed:?}"
        );
        assert!(followed.iter().any(|line| line.contains("said by")), "its history: {followed:?}");
        typed(&mut talk, "aside").await;
        assert_eq!(channel.thread_said_in("aside").await, thread, "said in the thread followed");
        let back = typed(&mut talk, "/thread line").await;
        assert_eq!(back.last().map(String::as_str), Some("Following the line of main now"));
        let none = typed(&mut talk, "/thread follow zz").await;
        assert_eq!(none, ["None of the open threads begins with 'zz'"]);
    }

    /// What the terminal prints of a line typed where it cannot do what the line asks: an
    /// approval named badly or not pending, nothing to give again, no questions to answer, no
    /// such channel.
    #[tokio::test]
    async fn a_line_that_cannot_be_done_is_told_why() {
        let (mut talk, _, _running) = talking().await;
        assert!(typed(&mut talk, "").await.is_empty());
        let not_asked = typed(&mut talk, "/approve").await;
        assert_eq!(not_asked, ["No approval is asked of you: name one by the beginning of its id"]);
        let invalid = typed(&mut talk, "/approve Nope").await;
        assert_eq!(invalid, ["None of the pending approvals begins with 'nope'"]);
        let id = Uuid::new_v4();
        let not_pending = typed(&mut talk, &format!("/deny {id}")).await;
        assert_eq!(not_pending, [format!("None of the pending approvals begins with '{id}'")]);
        let retried = typed(&mut talk, "/retry").await;
        assert_eq!(retried, ["No answer of an agent to give again"]);
        let none = typed(&mut talk, "/answer").await;
        assert_eq!(none, ["No questions are asked of you: name them by the beginning of their id"]);
        let none_named = typed(&mut talk, &format!("/decline {id}")).await;
        let none_put = format!("None of the open questions put to you begins with '{id}'");
        assert_eq!(none_named, [none_put]);
        let switched = typed(&mut talk, "/switch nowhere").await;
        let unknown = "You take part in no channel 'nowhere'";
        assert_eq!(switched.first().map(String::as_str), Some(unknown));
        let unnamed = typed(&mut talk, "/resume").await;
        assert_eq!(unnamed, ["Error: /session resume: it takes <id>"], "required (issue 583)");
    }

    /// Questions put to the user are asked of the talk's own accord, and answered line by line;
    /// put aside, a line is no answer, until they are taken up again. Answered, they are asked no
    /// more, though the channel has yet to tell that they closed.
    #[tokio::test]
    async fn questions_are_asked_and_answered_line_by_line() {
        let (mut talk, session_loop, running) = talking().await;
        put_to_the_owner(&mut talk, &session_loop, &running, &[]).await;
        assert!(talk.asks_questions(), "asked of its own accord");
        let retried = typed(&mut talk, "/retry").await;
        assert_eq!(retried, ["No answer of an agent to give again"], "a command, no answer");
        talk.put_aside();
        assert!(!talk.asks_questions() && talk.asking().is_some(), "waiting");
        talk.take_up();
        let help = typed(&mut talk, "/help exit").await;
        assert!(help.join("\n").contains("/exit"), "a command of the channel, no answer: {help:?}");
        assert!(talk.asks_questions(), "still asked");
        assert!(talk.asks_questions(), "taken up");
        talk.put_aside();
        typed(&mut talk, "/answer").await;
        assert!(talk.asks_questions(), "taken up by the command");

        let answered = typed(&mut talk, "Kaya").await;
        assert!(answered.is_empty(), "{answered:?}");
        assert!(talk.asking().is_none(), "answered, not asked again");
    }

    /// An answer in words is read as a message is: `!!` writes a literal `!` (issue 587), `//` a
    /// literal `/`.
    #[tokio::test]
    async fn an_answer_in_words_is_read_as_a_message() {
        for (line, answer) in [("!!yes", "!yes"), ("//etc", "/etc")] {
            let (mut talk, session_loop, running) = talking().await;
            put_to_the_owner(&mut talk, &session_loop, &running, &[]).await;
            let channel = session_loop.channels().into_iter().next().expect("its first channel");
            let mut observed = channel.observe();
            typed(&mut talk, line).await;
            let answered = async {
                loop {
                    let envelope = observed.recv().await.expect("observed");
                    if let ActorEvent::QuestionAnswered { responses, .. } = envelope.event {
                        break responses;
                    }
                }
            };
            let responses = time::timeout(Duration::from_secs(5), answered).await.expect(line);
            assert_eq!(responses, [Response::Text { text: answer.to_string() }], "{line}");
        }
    }

    /// A question with choices is answered by the choices taken, and asked no more.
    #[tokio::test]
    async fn a_question_is_answered_by_its_choices() {
        let (mut talk, session_loop, running) = talking().await;
        put_to_the_owner(&mut talk, &session_loop, &running, &["red", "blue"]).await;
        let offered = talk.asking().map(Asking::choices).unwrap_or_default();
        let labels: Vec<&str> = offered
            .iter()
            .filter_map(|offered| match offered {
                Offered::Choice(choice) => Some(choice.label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(labels, ["red", "blue"]);
        assert!(talk.choose(&[1]).await.is_empty());
        assert!(talk.asking().is_none(), "answered, not asked again");
    }

    /// What the channel shows as agent `agent` asks for approval in `channel`.
    fn approval_requested(channel: &ChannelLoop, agent: AgentId) -> Occurred {
        requested(channel, approval(channel, agent))
    }

    /// What the channel shows as `request` is asked for approval in `channel`.
    fn requested(channel: &ChannelLoop, request: ApprovalRequest) -> Occurred {
        let agent = ActorId::Agent(request.agent_id);
        let shown = Shown::Envelope(from(
            channel,
            agent,
            ActorEvent::ApprovalRequested { request, to: None },
        ));
        Occurred::InChannels(Box::new(Happened::Shown(channel.id(), Ok(shown))))
    }

    /// An approval pending in the channel talked in is asked of the talk's own accord, where the
    /// user may approve the calls of its agent; answered by the number of a choice - or by a
    /// command with the beginning of its id -, and asked no more once answered.
    #[tokio::test]
    async fn an_approval_is_asked_and_answered_by_its_number() {
        let (mut talk, session_loop, running) = talking().await;
        let channel = session_loop.channels().into_iter().next().expect("its first channel");
        let agent = an_agent_in(&running.kernel, &session_loop, &running.owner, channel.id()).await;
        talk.take_in(approval_requested(&channel, AgentId::new())).await;
        assert_eq!(talk.asking(), None, "not of an agent whose calls the user may approve");
        talk.take_in(approval_requested(&channel, agent)).await;
        let Some(Asking::Approval(first)) = talk.asking().cloned() else {
            panic!("asked: {:?}", talk.asking())
        };
        talk.take_in(approval_requested(&channel, agent)).await;
        assert_eq!(talk.asking(), Some(&Asking::Approval(first)), "the first, until answered");

        assert!(typed(&mut talk, "3").await.is_empty(), "no choice, but a message");
        assert!(typed(&mut talk, "1").await.is_empty());
        let Some(Asking::Approval(second)) = talk.asking().cloned() else {
            panic!("the next asked: {:?}", talk.asking())
        };
        let short = &second.id.0.to_string()[..8];
        assert!(typed(&mut talk, &format!("/deny {short}")).await.is_empty());
        assert!(talk.asking().is_none(), "answered, not asked again");
    }

    /// An approval whose tool suggests the calls alike is answered by "yes, and from now on" in the
    /// channel: the call approved, and the user's rule for the calls alike added there.
    #[tokio::test]
    async fn an_approval_from_now_on_adds_the_user_s_rule() {
        let (mut talk, session_loop, running) = talking().await;
        let channel = session_loop.channels().into_iter().next().expect("its first channel");
        let agent = an_agent_in(&running.kernel, &session_loop, &running.owner, channel.id()).await;
        let suggested =
            ToolMatcher { tool: "test_tool".to_string(), arguments: Default::default() };
        let request =
            ApprovalRequest { suggested: Some(suggested.clone()), ..approval(&channel, agent) };
        talk.take_in(requested(&channel, request)).await;
        let matcher = suggested.clone();
        let from_now_on =
            Offered::ApproveFromNowOn { matcher, reach: Reach::Channel(channel.id()) };
        assert_eq!(
            talk.asking().map(Asking::choices).as_ref().and_then(|c| c.get(1)),
            Some(&from_now_on)
        );

        let chosen = talk.choose(&[1]).await;
        assert!(chosen.is_empty(), "{:?}", lines(&chosen));
        assert!(talk.asking().is_none(), "approved");
        let rules = owner_controller(&running).await.profile().approval_rules().to_vec();
        assert_eq!(rules.len(), 1, "{rules:?}");
        assert_eq!(
            (&rules[0].matcher, rules[0].decision, rules[0].reach, rules[0].expires_at),
            (&suggested, RuleDecision::Allow, Reach::Channel(channel.id()), None)
        );
    }

    /// Where the user's rule cannot be added - they keep as many as they may -, the call stays
    /// approved, and the user is told why the rule is not.
    #[tokio::test]
    async fn an_approval_stands_where_its_rule_cannot_be_added() {
        let (mut talk, session_loop, running) = talking().await;
        let channel = session_loop.channels().into_iter().next().expect("its first channel");
        let agent = an_agent_in(&running.kernel, &session_loop, &running.owner, channel.id()).await;
        let matcher = ToolMatcher { tool: "test_tool".to_string(), arguments: Default::default() };
        let controller = owner_controller(&running).await;
        let context = controller.security_context(Default::default(), Default::default());
        for _ in 0..MAX_PERSONAL_RULES {
            let rule =
                ApprovalRule::new(matcher.clone(), RuleDecision::Allow, Reach::Everywhere, None);
            controller.add_approval_rule(&context, rule.expect("a rule")).await.expect("added");
        }
        let request = ApprovalRequest { suggested: Some(matcher), ..approval(&channel, agent) };
        talk.take_in(requested(&channel, request)).await;

        let chosen = lines(&talk.choose(&[1]).await);
        assert_eq!(chosen.len(), 2, "{chosen:?}");
        assert!(chosen[0].contains("is approved, but the rule"), "{chosen:?}");
        assert!(chosen[1].starts_with("Error: "), "{chosen:?}");
        assert!(talk.asking().is_none(), "approved, not asked again");
        assert_eq!(controller.profile().approval_rules().len(), MAX_PERSONAL_RULES);
    }

    /// What is asked in the channel talked in is asked no more once the talk switches to another
    /// channel, and asked again as it switches back.
    #[tokio::test]
    async fn what_is_asked_follows_the_channel_talked_in() {
        let (mut talk, session_loop, running) = talking().await;
        let main = session_loop.channels().into_iter().next().expect("its first channel");
        let side = spec("side", vec![running.owner.caller_id]);
        let side = session_loop.create_channel(&running.owner, side).await.expect("created");
        let followed = async {
            while talk.channels.followed(side.id()).is_none() {
                let occurred = talk.next().await;
                talk.take_in(occurred).await;
            }
        };
        time::timeout(Duration::from_secs(5), followed).await.expect("followed");
        let agent = an_agent_in(&running.kernel, &session_loop, &running.owner, main.id()).await;
        talk.take_in(approval_requested(&main, agent)).await;
        assert!(talk.asking().is_some(), "asked in main");

        typed(&mut talk, "/switch side").await;
        assert_eq!(talk.asking(), None, "nothing waits in side");
        typed(&mut talk, "/switch main").await;
        assert!(talk.asking().is_some(), "asked again in main");
    }

    /// The commands on the sessions of the directory: listed, the one talked in marked; deleted,
    /// but the one talked in; started and resumed, switching the talk to them.
    #[tokio::test]
    async fn the_sessions_of_the_directory_are_reached_by_commands() {
        let (mut talk, talked_in, running) = talking().await;
        let start = Opening::Start { template: None };
        let (other, _) = talk.sessions().open(&start).await.expect("started");
        // Also while questions are asked, which stay so.
        put_to_the_owner(&mut talk, &talked_in, &running, &[]).await;
        let listed = typed(&mut talk, "/session list").await.join("\n");
        assert!(talk.asks_questions(), "still asked");
        let marked: Vec<&str> =
            listed.lines().filter(|line| line.ends_with(", talked in")).collect();
        assert_eq!(marked.len(), 1, "{listed}");
        assert!(marked[0].starts_with(&talked_in.id().to_string()[..8]), "{listed}");

        let mine = talked_in.id().to_string()[..8].to_string();
        let refused = typed(&mut talk, &format!("/session delete {mine}")).await;
        assert!(refused.join("\n").contains("switch to another"), "{refused:?}");
        let theirs = other.id().to_string()[..8].to_string();
        let deleted = typed(&mut talk, &format!("/session delete {theirs}")).await;
        assert_eq!(deleted, [format!("The session {} is deleted", other.id())]);

        // Switched to another session, the talk follows its channels, the questions asked
        // dropped, and the one left is stopped; to the one talked in, to none that can be
        // opened, or to one the user takes part in no channel of, it stays where it is.
        let mine = talked_in.id().to_string();
        let same = typed(&mut talk, &format!("/resume {mine}")).await;
        assert!(same.join("\n").contains("already"), "{same:?}");
        let missing = typed(&mut talk, "/resume ffffffff").await;
        assert!(missing.join("\n").contains("No session"), "{missing:?}");
        let empty = typed(&mut talk, "/session new empty").await;
        assert!(empty.join("\n").contains("no channel"), "{empty:?}");
        assert_eq!(talk.session_id(), talked_in.id(), "stays where it is");
        assert!(talk.asks_questions(), "still asked");

        let told = typed(&mut talk, "/new").await.join("\n");
        assert_ne!(talk.session_id(), talked_in.id(), "the other followed");
        assert!(told.contains("Talking in the session") && told.contains("Talking in main now"));
        assert!(!told.contains("Error"), "{told}");
        assert!(talk.asking().is_none(), "the questions of the session left");
        // Stopped: opened again, it is started anew, not the one that ran.
        let resumed = Opening::Resume(mine);
        let (again, _) = talk.sessions().open(&resumed).await.expect("opened again");
        assert!(!Arc::ptr_eq(&again, &talked_in), "the one left was stopped");
    }

    /// What the user asked of the sessions amiss is told as it is; what failed of the kernel with
    /// what of its cause may be told, and nothing of one that may not.
    #[test]
    fn what_failed_of_the_sessions_is_told_with_what_of_its_cause_may_be() {
        let told = |error: SessionsError| lines(&[error_printed(&error.into())]);
        let amiss = SessionsError::NoneBeginning("ff".to_string());
        assert_eq!(
            told(amiss),
            [format!("Error: {}", SessionsError::NoneBeginning("ff".to_string()))]
        );
        let id = SessionId::new();
        // As `Sessions` keeps a cause (`SessionsError::failed`): an error of the kernel, become
        // anyhow's by `?`.
        let cause = anyhow::Error::from(SessionError::RunsElsewhere(id));
        let elsewhere = SessionsError::Failed {
            what: format!("The session {id} cannot be deleted"),
            cause: cause.reallocate_into_boxed_dyn_error_without_backtrace(),
        };
        let shown = format!(
            "Error: The session {id} cannot be deleted: {}",
            SessionError::RunsElsewhere(id)
        );
        assert_eq!(told(elsewhere), [shown], "the kernel's cause, meant for users");
        let hidden = SessionsError::Failed {
            what: "The sessions of this directory cannot be read".to_string(),
            cause: "a path, say".into(),
        };
        assert_eq!(told(hidden), ["Error: The sessions of this directory cannot be read"]);
    }

    /// Questions asked that close meanwhile are told as such as the talk takes in what the channel
    /// shows, and asked no more.
    #[tokio::test]
    async fn questions_that_close_while_answered_are_told() {
        let (mut talk, session_loop, running) = talking().await;
        let request = put_to_the_owner(&mut talk, &session_loop, &running, &[]).await;
        let channel = session_loop.channels().into_iter().next().expect("its first channel");
        let ActorId::User(user) = running.owner.caller_id else { unreachable!("a user") };
        let actor = channel.user_actor(user).expect("the owner's actor runs");
        let closed = ActorEvent::QuestionClosed { id: request.id, shown_to: None };
        actor.take_in_as_routed(from(&channel, ActorId::Agent(request.agent_id), closed)).await;
        let told = async {
            loop {
                let occurred = talk.next().await;
                let printed = lines(&talk.take_in(occurred).await).join("\n");
                if printed.contains("closed before you answered") {
                    break;
                }
            }
        };
        time::timeout(Duration::from_secs(5), told).await.expect("told");
        assert!(talk.asking().is_none(), "no longer asked");
    }

    /// What the terminal prints as `talk` goes on, until each of `until` is contained in a line of
    /// it.
    async fn printed_until(talk: &mut Talk, until: &[&str]) -> Vec<String> {
        let mut printed: Vec<String> = Vec::new();
        let told = async {
            while !until.iter().all(|text| printed.iter().any(|line| line.contains(text))) {
                let occurred = talk.next().await;
                printed.extend(lines(&talk.take_in(occurred).await));
            }
        };
        time::timeout(Duration::from_secs(10), told).await.expect("told");
        printed
    }

    /// Whether the history of `channel` holds a call the user made.
    async fn holds_a_call(channel: &ChannelLoop) -> bool {
        let entries = channel.history_entries().await;
        let message = |entry: &HistoryEntry| match entry {
            HistoryEntry::Said { message } => message.is_a_persons_call(),
            _ => false,
        };
        entries.iter().any(message)
    }

    /// Issue 587: a line beginning with `!` runs its command in the workspace, while the talk goes
    /// on; once it ended, the user sees the call they made, as the channel tells it - by its
    /// description -, and how it ended.
    #[tokio::test]
    async fn a_bang_runs_a_command_and_the_user_sees_their_call() {
        let (mut talk, session_loop, _running) =
            talking_in(in_directory_running_commands().await).await;
        let channel = session_loop.channels().into_iter().next().expect("its first channel");

        assert_eq!(typed(&mut talk, "!echo hello").await, ["The command runs: Ctrl-C ends it"]);
        let until = ["exited with 0", "calls run_command", "run_command exit code 0, 1 line"];
        let printed = printed_until(&mut talk, &until).await;

        let call = printed.iter().find(|line| line.contains("calls run_command"));
        let described = "Run in the workspace:⏎echo hello";
        assert!(call.is_some_and(|line| line.contains(described)), "{printed:?}");
        assert!(holds_a_call(&channel).await, "recorded");
    }

    /// Issue 587: one command runs at a time, and the user talks on meanwhile; Ctrl-C ends it,
    /// which kills it, and nothing is told of it in the channel.
    #[tokio::test]
    async fn a_command_runs_alone_until_it_is_ended() {
        let (mut talk, session_loop, _running) =
            talking_in(in_directory_running_commands().await).await;
        let channel = session_loop.channels().into_iter().next().expect("its first channel");
        let elsewhere = tempfile::tempdir().unwrap();
        let (started, ended) = (elsewhere.path().join("started"), elsewhere.path().join("ended"));

        let line = format!("!touch {} && sleep 1 && touch {}", started.display(), ended.display());
        assert_eq!(typed(&mut talk, &line).await, ["The command runs: Ctrl-C ends it"]);
        assert_eq!(typed(&mut talk, "!ls").await, ["A command runs already: Ctrl-C ends it"]);
        assert!(typed(&mut talk, "/channel list").await.concat().contains("main"), "talks on");
        let starting = async {
            while !started.exists() {
                time::sleep(Duration::from_millis(10)).await;
            }
        };
        time::timeout(Duration::from_secs(5), starting).await.expect("started");
        let told = talk.end_shell_command().map(|ended| lines(&[ended]));
        assert_eq!(told, Some(vec!["The command is ended".to_string()]));
        assert!(talk.end_shell_command().is_none(), "none under way any more");

        time::sleep(Duration::from_secs(2)).await;
        assert!(!ended.exists(), "the command was killed");
        assert!(!holds_a_call(&channel).await, "nothing told");
    }

    /// Issue 587: the user talks on in another channel while a shell command runs; how it ended,
    /// and the call they made, are told as of the channel it ran in.
    #[tokio::test]
    async fn a_command_ended_is_told_of_its_channel_where_another_is_talked_in() {
        let (mut talk, _session_loop, _running) =
            talking_in(in_directory_running_commands().await).await;
        // Runs until the test lets it end, once it talks in the other channel.
        let elsewhere = tempfile::tempdir().unwrap();
        let go = elsewhere.path().join("go");
        let line = format!("!until [ -e {} ]; do sleep 0.05; done", go.display());
        assert_eq!(typed(&mut talk, &line).await, ["The command runs: Ctrl-C ends it"]);
        typed(&mut talk, "/channel create side").await;
        // Followed once the talk learns that the user joined it.
        let mut printed = Vec::new();
        let switching = async {
            while !typed(&mut talk, "/switch side").await.concat().contains("Talking in side") {
                let occurred = talk.next().await;
                printed.extend(lines(&talk.take_in(occurred).await));
            }
        };
        time::timeout(Duration::from_secs(5), switching).await.expect("switched");
        assert!(!printed.concat().contains("exited"), "ended before: {printed:?}");
        File::create(&go).unwrap();

        let until = ["In main: The command exited with 0", "In main: owner calls run_command"];
        printed_until(&mut talk, &until).await;
    }

    /// Issue 587: a switch to another session ends the shell command run in the one left.
    #[tokio::test]
    async fn a_switch_to_another_session_ends_the_command() {
        let (mut talk, _session_loop, _running) =
            talking_in(in_directory_running_commands().await).await;
        assert_eq!(typed(&mut talk, "!sleep 30").await, ["The command runs: Ctrl-C ends it"]);
        let printed = lines(&talk.switch_session(&Opening::Start { template: None }).await);
        assert!(printed.contains(&"The command is ended".to_string()), "{printed:?}");
        assert!(talk.end_shell_command().is_none(), "none under way any more");
    }

    /// Issue 587: a shell command refused or failing is told as an error - given none, as it is
    /// typed; the storage running none, once the channel answers so.
    #[tokio::test]
    async fn a_command_that_cannot_run_is_told() {
        let in_directory = in_directory_where(false, home::commands()).await;
        let (mut talk, _session_loop, _running) = talking_in(in_directory).await;
        let told = typed(&mut talk, "!").await;
        assert_eq!(told, ["Error: /workspace run: it takes <command>"]);
        assert!(talk.end_shell_command().is_none(), "nothing under way");

        assert_eq!(typed(&mut talk, "!ls").await, ["The command runs: Ctrl-C ends it"]);
        let printed = printed_until(&mut talk, &["runs no commands"]).await;
        assert!(printed.iter().any(|line| line.starts_with("Error: ")), "{printed:?}");
    }

    /// Issue 587: where the channel offers no `/workspace run` - a configuration that leaves it
    /// out -, `!` tells so, and runs nothing.
    #[tokio::test]
    async fn a_bang_tells_where_no_command_can_be_run() {
        let (mut talk, _session_loop, _running) = talking().await;
        let told = typed(&mut talk, "!ls").await;
        assert_eq!(told, ["Error: The channel offers you no /workspace run, which `!` stands for"]);
        assert!(talk.end_shell_command().is_none(), "nothing under way");
    }
}
