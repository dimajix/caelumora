# 245 — Sessions and channels say nothing about what they are for

**Status:** done
**Area:** Sessions and channels

## The problem

A session and a channel have a name and nothing else that says what they are for. Neither the users
nor the agents in them learn what the conversation is about, what holds there, or what is expected
of whoever takes part - unless someone says it in the chat, where it scrolls away and is forgotten
by an agent whose history is cut.

The members panel of the web UI has meanwhile grown into a panel about the channel: besides the
members it shows the moderator, the language, the classification and the rules for tool calls. Its
name says less than it holds.

## Why it matters

An agent that knows what its channel is for answers to the point without being told again in every
request. For users, a channel's purpose is what they decide by whether to look at it. A long
description of the kind an `AGENTS.md` is - conventions, context, what to do and what not - is how
a place is set up for agents elsewhere, and is what the maintainer asks for.

## What is wanted

The maintainer's wish:

- A session and a channel each have two descriptions: a **short one**, seen at a glance, and a
  **long one** like an `AGENTS.md`, which can be read and edited.
- Both reach **every agent** in the channel - run here and external - through a new prompt
  provider.
- Both can be set in the **session template**, for the session and for each of its channels.
- The members panel becomes a **channel panel**.

## Considerations

### Names

`SessionTemplate::description` and `Personality::description` are catalogue lines: what a template
or a personality is, for whoever chooses one. The descriptions here say what a session or channel is
for, to those in it - like `Personality::introduction` (issue
[176](176-participants-in-system-prompt.md)), which was given a name of its own for that reason. So
neither should be called `description`. Candidates: `summary` for the short one; `briefing`,
`guide` or `instructions` for the long one. "Instructions" says most plainly what it is to an agent,
but invites reading it as commands (see *Trust* below).

### The model

- `Session` and `Channel` each gain the two fields, `Option<String>`, `None` if not set.
- `ChannelTemplate` gains both; `SessionTemplate` gains both beside its catalogue `description`,
  copied into the session it makes. Copied, not read through the template: a session is no longer
  tied to its template once made, and its descriptions are edited on the session.
- **Lengths.** The short one is a line - a limit of a few hundred characters, on one line as
  `one_line` keeps an introduction. The long one goes into the prompt of every agent on every turn:
  a limit, like `MAX_INTRODUCTION_CHARS`, keeps one channel from filling every context. Maybe some
  ten thousand characters; the token cost is reported by issue [209](209-token-usage.md).

### Who reads and who writes

- **Reading** follows the place: a channel's descriptions whoever reads the channel, a session's
  whoever reads the session. They appear in snapshots of the session and the channel, the long ones
  perhaps only with a `Detail` of their own, so that a stream's first snapshot does not carry them.
- **Writing** is `UpdateChannel` and `UpdateSession`, as for the name: a `PUT …/summary` and `PUT
  …/briefing` per place, like `…/language` and `…/approval-rules` - through the session's and the
  channel's controllers, which tell the change as a `SessionUpdateEvent` or `ChannelUpdateEvent`
  (like `LanguageChanged`), so that open pages learn of it.

### Confidentiality

- **A channel's descriptions** are part of the channel and classified like it; the prompt part
  carries the channel's classification.
- **A session's descriptions** reach the agents of every channel in it, and the channels are
  classified differently - a public one may have an external agent in it. Either a session's
  descriptions count as public, which whoever writes them must be told; or they carry a
  classification of their own and reach only channels that may receive it (`may_publish`), like a
  note. The first is simpler; the second is what the confidentiality rules would otherwise demand.

### Trust

The long description goes into the system prompt of every agent in the channel - also of agents
owned by others than its author. It is written to steer the agents, and that is its purpose; but it
is the channel's owner who steers someone else's agent, with the weight of the system prompt (a
cousin of issue [150](150-impersonation-towards-the-agent.md)). What holds the agent within its
owner's bounds - permissions, approvals, the rules of a place (issue [13](13-approval-modes.md)) -
is unchanged by it, so the risk is to what the agent says, not to what it may do. The prompt part
should name it as the description of the place, written by whoever may change it, and below the
personality's own instructions.

### The prompt provider

- A component of its own, like `channel_language_prompt_provider` and
  `channel_members_prompt_provider`: the session's and the channel's descriptions in one prompt
  part, read afresh at the start of every turn through the agent's views, so that a change reaches
  the agent with its next turn. In the system prompt by default (`position`): it changes rarely,
  and a cached system prompt stays cached.
- For an external agent, which keeps its conversation itself (issue
  [204](204-external-agent-roster.md)), told once and then only when it changed (`changes_only`),
  as the roster and the language are.
- **Wired in by the personality**, like the other prompt providers, or given to every agent by the
  runtime? The wish says every agent; the pattern so far is that a personality opts in, and
  `examples/seed.yaml` wires the providers into each. Opting in keeps the personality in control of
  its prompt; giving it to all means a channel can count on its agents knowing it. Whichever is
  chosen holds for the roster and the language too - a question of its own, worth deciding once.

### The web UI

- The members panel becomes the **channel panel**: at the top the channel's short description, the
  long one behind a click, readable and - for whoever may change the channel - editable in a larger
  editor (Markdown); then what it shows today.
- The short descriptions also where sessions and channels are listed, as a second line or a
  tooltip.
- A session's descriptions where the session's settings are - privacy, rules - which today has no
  panel of its own.

## Plan

Decided with the maintainer on 2026-10-06.

### Decisions

- **Names:** `summary` - the short one, one line - and `briefing` - the long one, what an agent
  should know before it works here. Not `description`, which is the catalogue line of a template or
  a personality.
- **Confidentiality:** a session's descriptions count as public - the session's document is read by
  everybody in it, as its name is - and the editor says so: everybody in the session and every
  agent in each of its channels reads them, external ones too. A channel's are part of the channel
  and carry its classification as it is now.
- **Wiring:** the personality wires the provider in, as it does the roster and the language;
  `examples/seed.yaml` wires it into every personality. Whether the runtime should give prompt parts
  to every agent regardless is a question for all three providers, not for this issue.
- **Trust:** the prompt part names itself as the description of the session or channel, written by
  whoever may change it, and comes after the personality's own instructions - by the order of the
  components. What an agent may do is unchanged by it.

### The model

- `Session`, `Channel`, `SessionTemplate` and `ChannelTemplate` gain `summary` and `briefing`,
  `Option<String>`, serde default, left out when `None`. A template's are copied into the session
  and channels it makes; `SessionTemplate::description` stays the catalogue line.
- **Limits:** a summary at most `MAX_SUMMARY_CHARS` (300), on one line; a briefing at most
  `MAX_BRIEFING_CHARS` (16 000) - it is in the context of every agent on every turn.
- **Checked in one place.** `UserProfile::introduce` checks a text today - trimmed, `None` if
  blank, a length, no control character that is no whitespace. That check becomes a function of the
  model that `introduce`, the summary and the briefing use alike, the summary also refusing line
  breaks. A step of its own, without a change of behaviour.

### Changing them

- `SessionUpdateEvent::DescriptionChanged { summary, briefing }` and the same on
  `ChannelUpdateEvent`, as they now stand.
- `SessionController::update_description(ctx, summary, briefing)` and the same on the
  `ChannelController`, behind `SessionLoop` and `ChannelLoop` as `update_language` is; checked as
  `UpdateSession` and `UpdateChannel`, before anything else.
- `PUT /sessions/{id}/description` and `PUT /sessions/{id}/channels/{cid}/description` with
  `{summary, briefing}` - both in one request, as the dialog edits them; `null` or blank removes
  one. `400 invalid_description` for a text that is too long or holds what it may not, `403` for
  who may not change the place.

### Reading them

- `SessionSnapshot` and `ChannelSnapshot` carry `summary` always and `briefing` with a new
  `Detail::BRIEFING`: the first snapshot of a channel's stream and `GET` of one session or channel
  ask for it, lists of sessions and channels do not. The REST responses likewise.
- `SessionView` and `ChannelView` offer `summary()` and `briefing()`.

### The prompt provider

`channel_briefing_prompt_provider`, a component providing `PromptProvider` like
`channel_language_prompt_provider`: at the start of every turn it reads the session's and the
channel's descriptions through the agent's views and renders them as one part - the session's
first, each under a heading of its own that says whose description it is and who wrote it. Nothing
if neither has any. Classified as the channel, if the channel's descriptions are in it, else
public. `position` as the others, `start` by default; `changes_only` for an external agent, which
is told once and then only when something changed.

### The web UI

- The members panel becomes the **channel panel**: its tab is called "Channel". At the top the
  channel's summary, the briefing behind a click, rendered as Markdown; for whoever may change the
  channel an edit button opening a dialog with both (`DescriptionDialog`).
- The session's descriptions are edited from the session's menu in the sidebar with the same
  dialog, which says there that everybody in the session reads them. The summaries are the tooltips
  of sessions and channels in the sidebar.

### Tests

The check of a text; the template's descriptions copied; the controllers' changes stored and
announced, refused for a text that is too long; the provider's part for session and channel, its
classification, and `changes_only`; the API's shapes; in the security suite, that only who may
change a place changes its descriptions.

### Steps

1. **Restructure:** the check of a text out of `UserProfile::introduce` into a function of its own.
2. **Model, controllers, REST, templates**, with the snapshots and `Detail::BRIEFING`.
3. **The prompt provider**, wired into `examples/seed.yaml`.
4. **The web UI.**

## Done when

- Sessions and channels have a short and a long description, set in the session template, read and
  changed through the REST API and the web UI by who may read and change the place.
- A prompt provider gives an agent - run here or external - the descriptions of its session and its
  channel, kept up when they change, with the classification they carry.
- The members panel is a channel panel that shows the channel's descriptions.
- `CHANNELS.md`, `AGENT-TURN.md` (prompt parts), `COMPONENTS.md`, `REST-SERVER.md`, `WEB-UI.md`
  and `examples/seed.yaml` describe and use it.

## History

- **2026-10-06** — Opened on the maintainer's request. Open: the names of the two descriptions,
  how a session's descriptions are classified, and whether the prompt provider is wired in by the
  personality or given to every agent.
- **2026-10-06** — Planned with the maintainer (see *Plan*): `summary` and `briefing`; a session's
  public, a channel's classified as the channel; the provider wired in by the personality; the
  prompt part named as the place's description, after the personality's instructions.
- **2026-10-06** — Done, in the four steps planned. Deviating from the plan: the check of a text is
  `TextRule` in `caelumora-model::text` - a field's name, length and whether it is one line - whose
  rules the field's owner keeps as constants (`UserProfile::INTRODUCTION`, `Session::SUMMARY`,
  `Session::BRIEFING`; a channel is described as its session is), not a kind of text the module
  knows; `TextError` names the field. `Session::described` checks both for the controllers and the
  templates, a template's when a session is made from it (`TemplateError::Description`). That the
  fields could be types that check themselves is issue [248](248-validated-text-fields.md). On the
  way: `routes::change_error` answers `400` with the code it is given, for `invalid_rule` and
  `invalid_description` alike, where `approval_rules_error` answered only the first; and
  `routes::session_view` answers what `GET` of a session and every change of one answer, read in
  four handlers before. Tests: the rule; a template's descriptions copied and checked; the views
  give the briefing only with `Detail::BRIEFING`; the provider's part - the session's first,
  classified as the channel where it holds the channel's, read anew at every turn, `changes_only`;
  the API's shapes; in the security suite, that a participant changes neither description and the
  owner both. Seen in the browser: the session's dialog from the sidebar, its summary as the
  tooltip, the channel panel's "Channel" tab with the summary and "Briefing" after "Say what this
  channel is for". Not seen: the part in an agent's context - the personalities in the local store
  were seeded before they wired the provider in, and the seed does not change stored documents.
