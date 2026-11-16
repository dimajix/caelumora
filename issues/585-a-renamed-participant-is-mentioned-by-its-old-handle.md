# 585 — A renamed participant is mentioned by its old handle

**Status:** done
**Type:** bug
**Priority:** low
**Area:** Sessions and channels
**Summary:** A channel resolves `@`-mentions by a directory of its participants' handles (`MentionDirectory`) that it builds only as someone joins or leaves, so after an agent or a user taking part is renamed, `@old` still addresses them, `@new` addresses nobody, and the terminal highlights accordingly - until the next join or leave.

## The problem

`ChannelLoop` keeps `mentions: ArcSwap<MentionDirectory>`, rebuilt by `rebuild_mentions` from the
names of the actors that run in it - as an actor is adopted (`channel/mod.rs`) and as one leaves
(`leave_actor`), nowhere else. The actors themselves tell their names as they are now: an agent's
actor reads its live controller (`caelumora-agents/src/actor/body.rs`, `name`), a user's its
`UserController` (`actor/user.rs`, `name`). A rename becomes effective in that controller - one
made here as it is stored (`LiveDocument::update`), one made elsewhere as the `ControllerManager`'s
`Follower` takes over its announcement (`LiveDocument::take_over`) -, but the channel learns of
none: no event tells it, and the directory keeps the old handle.

So after `AgentController::update_name`, or a user renamed by their provider (the `UserManager`
of `user_provider`, which stores the new name and announces it), a message with `@new` resolves to
nobody - no agent is asked -, one with `@old` still to the renamed participant, and the terminal,
which highlights by the same directory since the work on 579's findings
(`ChannelLoop::mentions`, `MentionDirectory::mentioned_spans`), highlights `@old`. The gap closes by
itself as soon as anyone joins or leaves the channel. A session runs for long, and agents are
renamed in the web UI while it runs; how often it bites depends on how often that happens.

## Considerations

The architecture is driven by events: a list is built as what it is made of changes, not again for
every message. A rename is a change the channel is not told of; that is the gap to close.

1. **The session follows the renames of its participants on the event bus.** The session already
   follows its agents' topics, for their deletion and restoring (`follow_agents`, issues 314, 221).
   It follows its users' topics too, and on an `AgentUpdateEvent::NameChanged` or a
   `UserUpdateEvent::NameChanged` of a participant tells each of its channels
   (`ChannelLoop::participant_renamed`), which rebuilds its directory. One thing to mind: the
   `Follower` takes the same announcement into the live controller in a task of its own, so a
   channel that read the actors' names might still read the old one - a rename by the user
   provider is announced without going through the live controller. The channel therefore takes
   the new name from the announcement for that participant as it rebuilds, keeping no names of its
   own: the next rebuild reads the actors again, by then renamed. After the session lost events, its
   channels rebuild their directories from the actors, as it looks at its agents again then. It
   fits the existing design: the session already follows its participants on the bus.
2. **The channel follows the announcements itself and keeps the names they tell.** Free of the
   race as well, but the channel would keep a second copy of its participants' names beside the
   actors, to be read again from them whenever events were lost.
3. **The live controller tells that it changed.** `LiveDocument` gets a local signal beside
   `current` - a `tokio::sync::watch` of its revision -, fired where every change becomes effective
   (`update`, `take_over`, `take_stored`); an actor offers it as a change of its name, and the
   channel forwards it to its `run` as an `ActorChange::Renamed`, which rebuilds the directory. No
   race and no second source, and the owner of the name tells - but a new pattern: a local
   notification mechanism of live documents, an extension of the `Actor` trait in `caelumora-api`,
   a forwarder per participant in the channel. Worth it once more than the mentions want to follow
   a live document; `ParticipantRenamed` announced in the channel would be such a use.
4. **No directory kept; built at each request.** `ChannelLoop::mentions` builds it from the actors
   that run each time it is asked - for every message that enters, and every message the terminal
   prints. Simplest, never stale, cheap for a channel of a handful of participants; but it builds a
   list per message, which the event-driven design is to avoid.
5. **A cache invalidated as the participants change, validated as it is asked.** Rebuilding lazily
   saves nothing over rebuilding at once, as joins and leaves are rare - and a rename invalidates
   nothing unless an event tells of it, which is the gap itself. A reference into it cannot be
   given out either, as another task may replace it meanwhile: at most a guard, which the `Arc` of
   an `ArcSwap` already is.

## Decision

Option 1 (2026-11-16, with the maintainer): the gap is small, and following the participants'
renames on the bus as the session already follows its agents fits the existing design best; option
3 would bring a new pattern for it. The options are kept here to come back to, should more want to
follow a live document.

## Done when

A participant renamed while it takes part is mentioned by the handle of its new name - and no
longer by its old one - in every channel of the session it takes part in, without anyone joining or
leaving, whether renamed here or elsewhere; the terminal highlights accordingly; a test holds it for
an agent and for a user.

## History

**2026-11-16** - Opened with the maintainer from a finding of the review of the work on 579's
findings (`ChannelLoop::mentions`): the terminal now highlights by the channel's directory, and
with it by the channel's stale handles after a rename. Options 1 to 5 weighed; option 1 chosen.

**2026-11-16** - Done by option 1. The session follows the topics of its users beside those of its
agents, a participant taken in later too (`participant_topics`, `participant_taken_in`), and tells
its channels of a participant renamed as announced (`ChannelLoop::participant_renamed`), which
build their handles anew with the announced name for it; after it lost events, they build them anew
from their actors (`rebuild_mentions`). The scripted agent of the kernel's tests read its name once,
as it was built; it reads it as it is now, as an agent's actor does - else a channel building its
handles anew after the rename of another participant took the agent's old name again. A channel
builds and stores its handles under a lock (`building_mentions`), so that a build from older
actors - a join and a rename at once - does not replace a newer one.
Two rare cases remain, both of a rename made elsewhere - by the user provider, later by another
instance -, which the `ControllerManager` takes into the live controller in a task of its own: a
participant joining or leaving the same channel before it did builds the handles anew from the
actors and brings the old handle back, until the next join or leave; and where the session and the
`ControllerManager` both lose events in one burst, the session's rebuild after the loss may come
before the `ControllerManager` read its controllers again - what the decision says of the rebuild
after a loss holds only once they have. A rename made here changes the live controller before it is
announced, and is free of both.
