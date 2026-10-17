# 233 — An agent's private notes can be read only in the context of a message

**Status:** done
**Area:** User interaction
**Summary:** An agent's private notes can be read only inside the context of one of its messages, as they stood then, not on their own, current and for every channel of the session.

## The problem

The web UI shows an agent's public notes in the members panel, but its private notes
([51](51-agent-private-notes.md)) nowhere of their own. Whoever is responsible for the agent finds
them only by opening the context of one of its messages and reading the prompt part "[Your private
notes ...]" there - as they stood when that message was written, mixed with the private notes
recalled from other channels, and not at all before the agent has answered once since noting them.
What the agent keeps to itself in a channel right now cannot be looked up.

## Why it matters

Private notes steer what an agent does without anyone else seeing them; that is their purpose. The
person responsible for the agent is the one who is meant to see them (decided in
[54](54-private-notes-visible-to-users.md)), and they should be able to check them as directly as
the agent's tools ([194](194-tool-sets.md)) or its history - not by searching through contexts.

## Considerations

- **Who may see them is decided: `ReadAgentContext`.** Issue 54 weighed hiding private notes from
  the owner (a separate `ActionType::ReadPrivateNotes`, redacting the prompt part) and decided
  against it: private means private from other agents and other users, not from the person
  responsible for the agent, and `note_private` tells the model so. The context panel showing them
  is that decision, not a gap. A panel of their own shows nothing the context does not already show
  the same reader, so it needs no new permission - and if 54's decision is ever revisited, both
  places have to follow it together.
- **The runtime has it already.** `MemberView::private_notes` (`view/channel.rs`) returns the
  private notes the member wrote in the channel with the digest of their chain, asks
  `ReadAgentContext`, and of a channel read entry by entry only those released to where the viewer
  reads from. Nothing calls it yet. What is missing is the REST route and the web UI.
- **Every channel of the session, each note with its channel** (the maintainer's wish). The private
  notes prompt part already gathers them so: `PrivateNotesComponent::get_prompt` walks
  `SessionView::all_channels`, takes the agent as a member of each (`ChannelView::member`), and
  calls `MemberView::private_notes`; channel id and name come with the `ChannelView` for free. A
  second copy of that loop in the REST server would be the repeated logic AGENTS.md warns of, so it
  moves to the session's view first - a method like `SessionView::private_notes_of(agent)` returning
  per channel its id, name, notes and digest, sorted by name - and both the component and the route
  call it (restructure, then extend). The component keeps skipping its own channel, whose notes it
  takes from the turn.
- **What the reader gets is per channel, as the agent's is.** For a user, `all_channels` yields what
  they may read, a channel as a whole or entry by entry, and `private_notes` asks
  `ReadAgentContext` and keeps of a channel read entry by entry only what was released. Private
  notes in a channel the user may not read at all are therefore not shown, even to the agent's
  owner. That is the flow rule, not a defect: the context panel shows such notes only where the
  agent itself recalled them into a channel the user reads. The panel says nothing about channels
  left out, so that they cannot be probed.
- **Only this session.** An agent of user or global scope takes part in several sessions and has
  private notes in each. Those of other sessions belong to an agent's own page
  ([227](227-agent-management.md)), which reaches every session; the method above is the part of it
  per session.
- **REST:** `GET /sessions/{id}/actors/{aid}/private_notes` - a route of the session, since the
  answer spans its channels - mapping the view's errors like `history` and `tools` do
  (`Subject::Context`). It returns per channel its id and name and its notes as the public ones are
  returned (number, text, classification), with the digest. A channel without notes is left out.
  Not in the channel snapshot: the snapshot is read by every member of the channel, and the notes
  would have to be left out for all but one reader on every read.
- **Web UI:** a menu entry "Show private notes" on the agent (`memberActions` in `ChatView.tsx`,
  issue [230](230-member-actions-menu.md)), offered where `can_read_context`, opening a panel in the
  detail slot on the right as the history and tools panels do (one at a time, width shared, issue
  [231](231-resizable-panels.md)). Reloaded when the global stream announces
  `AgentChannelStateChanged` for any channel of the session (the event carries the channel, not the
  agent; reloading on every one is cheap enough for a panel that is open). Grouped by channel, the
  current channel first and named as such, the others by name; the number of a note only for the
  current channel's, as the prompt part does, since a number names a note in one chain. The
  classification of each note is shown, as in the members panel.
- **A former member** (`MemberView::former`) still has its private notes in the channel's state;
  offering the entry for it as for its history costs nothing and keeps the two alike.
- **Agent memory** (`remember`, `AgentState::memory`) has no view of its own either. It is not
  per channel, so it fits an agent's own page (227) rather than this panel; not part of this issue.

## Done when

Whoever may read an agent's context can open its current private notes from the agent's menu, in
the panel on the right: those of every channel of the session they may read, each with the channel
it was written in, kept current as the agent notes and revokes. The loop over the channels exists
once, shared by the prompt part and the REST route; the route and the panel are described in
REST-SERVER.md and WEB-UI.md.

## History

- **2026-10-05** — Issue created at the maintainer's request: the private notes of an agent should
  be shown in the right panel, from a new entry of the agent's menu. Who may see them was checked
  against [54](54-private-notes-visible-to-users.md): `ReadAgentContext`, decided 2026-09-26, and
  `MemberView::private_notes` already asks it. The maintainer then widened it from the current
  channel to every channel of the session, each note with the channel it came from.
- **2026-10-05** — Done, in two steps. *Restructure:* `SessionView::private_notes_of(agent)` walks
  `all_channels`, reads `MemberView::private_notes` in each where the agent is a member, and returns
  per channel the `ChannelView`, the notes and their digest, sorted by the channel's name - a tuple,
  not a type, as its two callers are close by. `Forbidden` ends the walk; a failed channel is
  logged and left out. `PrivateNotesComponent` takes its notes from elsewhere from it and drops its
  own channel's, whose notes come from the turn. Its behaviour is unchanged: it logged and skipped
  every failure before, and a `Forbidden` comes for every channel or none, since all are asked of
  the same agent. *Extend:* `GET /sessions/{id}/actors/{aid}/private_notes` (`PrivateNotesResponse`:
  `channel_id`, `channel_name`, `notes`, `digest`), and in the web UI "Show private notes" beside
  "Show history" in the menu of an agent and of a former member where `can_read_context`, opening
  `PrivateNotesPanel` in the detail slot. Two corrections to the considerations above:
  `AgentChannelStateChanged` carries the session and the agent as well as the channel, so the panel
  reloads only for its agent in its session; and the members panel shows no classification of a
  note - the panel shows it, the TypeScript `Note` gained the `classification` the server always
  sent. Tests: a view test of the walk (sorted, a channel without notes left out, `Forbidden` for a
  member who is not the owner) and `an_agents_private_notes_are_its_owners_alone` in the security
  suite. REST-SERVER.md, WEB-UI.md and MEMORY.md updated. Not tried in a browser.
