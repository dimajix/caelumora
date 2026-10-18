# 423 — A participant added misses what is said between catching up and its join

**Status:** open
**Type:** bug
**Priority:** medium
**Area:** Sessions and channels
**Summary:** An actor added to a running channel catches up from the store before `run` takes it in, so what the channel records in between is in the newcomer's range but neither caught up nor delivered to it.

## The problem

`ChannelLoop::start_participant` (`caelumora-runtime/src/channel/participants.rs`) builds the
actor, lets it catch up on the history from the position `from` (`Actor::catch_up`, read from the
store as a `Reader::Newcomer`), adopts it and hands it to `run` as `ActorChange::Start`. Only when
`run` handles that change does the actor get its inbox, and only then are its `HistoryShared` and
`ParticipantJoined` recorded (issue 417).

`run` goes on entering events meanwhile. What it records between the catch-up's read and the
handling of `ActorChange::Start` is delivered to the actors running then - not to the newcomer,
which has no inbox yet - and was not there yet when it caught up. By the record, though, the
newcomer was a member: its range begins at the `from` of its `HistoryShared`, before those entries
(`Memberships`). So the history says it was given what it never took in: an agent answers without
a message said just before it joined, and a later page of its own history shows that message as if
it had heard it.

The window is short - the time it takes to build an actor and read the store - but it is not
bounded, and in a busy channel it is hit. It was there before issue 417; since 417 the record shows
it.

## Considerations

- **Catch up inside `run`.** `run` is the channel's one sequencer: if it read what the newcomer may
  see from the store when it handles `ActorChange::Start` - right before it records the join and
  opens the inbox - nothing could be entered in between. Costs a store read in `run`'s loop, which
  holds up every other event of the channel for as long, and `catch_up` becomes something `run`
  calls rather than `start_participant`.
- **Deliver the rest after the join.** `catch_up` answers up to where it read; `run`, as it takes
  the actor in, delivers the entries between that position and the join from the store, as it
  delivers live ones. `run` stays as it is but for that read, and the newcomer gets what it missed
  in the order it was recorded.
- **Actors catch up by position in general.** An actor that knows up to which position it took
  in the history reads the rest from the store whenever it is behind - after joining, after a
  restart ([385](385-inbox-lost-on-restart.md)), after its inbox overflowed
  ([223](223-full-inbox-drops-events.md)). The cleanest, and the largest: it makes delivery a
  notice and the history the truth, and is a decision of its own.

## History

- **2026-10-18** — Opened from issue 417, step 7, where it was noted in the history.
