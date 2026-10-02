# 182 — Definitions are read from the store again for every use

**Status:** open
**Area:** Persistence

## The problem

Documents that change rarely - personalities, agents, users and their
profiles - are read from the `EntityStore` every time something needs
them, even within one turn. As of 2026-10-02, after the duplicate reads
were merged (step 1, below):

- **Every turn of every agent** with `channel_members_prompt_provider`
  builds the channel's members through the views (`ChannelView::members`):
  for each agent member its `Agent` document and its personality, for each
  user their record and profile - for the introductions of issue 176.
- **Every `MemberView`** - a REST request for a channel, its notes, a
  member's context - reads the agent and its personality again, as the
  system.
- **Every actor** reads its personality once when it is built
  (`Member::of`), which is right; it is named here because it is the one
  read that a cache would also serve.

Live controllers exist for agents and users of running sessions
(`ControllerManager`, issues 120, 121), but the views read the store
deliberately: they show running and stopped sessions alike (issue 163).
Personalities have no controller at all.

## Why it matters

- With the JSON store every read is a file read and a JSON parse; with
  the Postgres store of issue [03](03-postgres-entity-store.md) a round
  trip. Per turn, the number of reads grows with the number of members.
- The work is the same every time: the documents change rarely, and only
  through a few paths.

## Considerations

- **Merge before caching (done).** Step 1: `MemberView::of` reads an
  agent's personality once and keeps it for its kind (issue 179) and its
  introduction (issue 176); `Member::of` reads it once, in the agent's
  context, and hands it to the actor, which no longer reads it again.
- **a) A `PersonalityController`** in the `ControllerManager`, as for
  agents and users: the document live (`ArcSwap`), kept in step by an
  event. Personalities are changed today past any controller (seed,
  `EntityStore::update_personality`), so a path by which a change reaches
  the controller is missing - and with it the decision whether personality
  changes go through a controller at all.
- **b) A caching `EntityStore` decorator**: general, for every kind of
  document, with no change to the callers. It must keep the store's
  permission checks (it caches documents, and asks per read whether the
  caller may have them), and its entries go stale as soon as there is more
  than one process (issue [122](122-several-instances.md)) - unless the
  store announces changes, which a Postgres store could (`LISTEN/NOTIFY`).
- **c) Views from the controllers where the session runs**: the views
  already prefer the running loop for history; they could prefer the
  controllers for agents and users likewise, and read the store only for
  stopped sessions. Covers agents and users, not personalities.
- **Per turn rather than per process**: what a turn reads could be read
  once at its start and kept with the turn - which also makes the turn
  consistent in itself, but helps neither the REST server nor the views.

## Done when

- Reading the members of a channel - per turn and per REST request -
  does not read every agent's, personality's and user's document from the
  store each time, and a changed document is seen within a bounded delay.

## History

- **2026-10-02** — Opened at the maintainer's request, after the
  personality was read up to three times for one member. Merged as step 1
  in the same change (see *Considerations*); caching is what remains.
