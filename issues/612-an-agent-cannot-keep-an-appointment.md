# 612 — An agent cannot keep an appointment

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Sessions and channels
**Summary:** An agent works only when someone speaks to it: it cannot note that something is to be done at a time or again and again - a reminder, a daily report, a meeting - and nothing wakes it then, so whatever is to happen later needs a person to ask for it at that time.

## The problem

Every turn of an agent begins because something reached it in a channel: a message, a request to
respond, a brief given by a delegate (issue
[557](557-a-conversation-cannot-go-on-two-ways-at-once.md)), a command. The only work that begins
with nobody speaking is an external agent's task that outlives its request
([199](199-external-agent-speak-wakes-all.md)) and work resumed after a restart
(`procedure::session::resume_interrupted`, issue 388). Nothing begins at a time.

So an agent cannot:

- remind someone - or itself - of something at a time ("ask Alice tomorrow at 9 whether the review
  is done");
- do something regularly (a report every Monday, a check of a mailbox every hour, a summary of the
  day's channels every evening);
- call a meeting: bring several participants together at a time, in a channel made for it.

The current time is known to an agent (component `current_time`), and a session has a time
zone (`Session::effective_time_zone`), but neither is anything an agent can act on later.

## Why it matters

- Autonomous work is largely work at a time: recurring duties, deadlines, follow-ups. Without it
  an agent does only what a person starts, at the moment they start it - the company of issue
  [617](617-epic-a-company-that-works-on-its-own.md) needs it first.
- The same foundation - work that begins without a participant speaking - is what an agent needs
  to learn of a new mail ([613](613-an-agent-cannot-read-or-send-email.md)) or a message from a
  messenger ([614](614-no-messenger-reaches-a-channel.md)). It is described here once (*What the
  three share*) and referred to from there.

## Considerations

### What the three share: work that begins without anyone speaking

A calendar entry that falls due, a mail that arrives, a message on a messenger: each is an event
outside every channel that has to become work of an agent in a channel. Four questions are the same
for all of them, and should get one answer, not three:

1. **Who brings it in.** The closest pattern is the delegate of a thread
   (`caelumora-kernel/src/actor/delegate.rs`, CHANNELS.md *Delegating into a thread*): an internal
   actor of the channel, no participant, which gives the agent a text to read (`GivenToRead`) and
   asks it alone to respond (`RequestReason::Given`); the agent takes it in as it takes a brief, and
   nobody else hears it. An event from outside would be given the same way, by an internal actor
   of its own or by the kernel through the channel - in either case not as a message of a
   participant, since nobody said it. The concept needs one name; *trigger* is a proposal.
2. **Where it lands.** In a channel that runs: the channel the entry was made in, a channel a
   mailbox or a messenger chat is bound to, or a session made for it from a template. A session
   that is not running has to be started for it, as `resume_interrupted` starts the sessions with
   interrupted work after a restart.
3. **In whose name.** An agent reads in its owner's name and acts in the name of whoever asked it
   ([255](255-whose-name-agents-act-in.md)). Work that nobody asks for at the moment it begins
   has two precedents: an external agent's late task speaks with the authority of the request that
   began it, and one without that request speaks "afresh, as the agent itself" (CHANNELS.md
   *External agents*). An entry an agent made in a turn would keep that turn's initiator, checked
   again when it falls due - whoever made it may have left the channel since; a mail or a messenger
   message has no initiator here and would be work of the agent's own, in its owner's name.
4. **What may flow.** What an entry says is classified as the work that wrote it, and may wake an
   agent only in a channel it flows into; what comes from outside - a mail, a messenger message -
   is untrusted, public at most, and marked as coming from outside, the counterpart of the door at
   which an external agent is cleared (CONFIDENTIALITY.md).

Recommended: decide these four once, as their own step - possibly an issue of its own - before the
calendar, mail or messenger are built on them.

### Whose calendar

The question "whose calendar is it - the agent's, the session's, the project's?" has no answer that
fits every scenario: a reminder belongs to the conversation it was made in, a daily duty to the
agent, a meeting to those it brings together. Options:

- **A. One calendar per agent**, as an agent's memory is one document per agent (plugin
  `caelumora-memory`): simple, but an entry made in a channel would outlive the channel, and a
  meeting would belong to one of its participants.
- **B. A calendar per session or project**: fits meetings and project deadlines, but an agent's
  daily duty belongs to no session.
- **C. No calendar as a container: an entry is an entity, and a calendar is a view.** Each entry
  lies in the scope of the place it falls due in - a channel's for a reminder there, a user's for
  an entry that starts a session of its own - and goes with it, as everything in a scope does
  (`ScopedEntities`). It is owned by the owner of the agent that made it, since everything has a
  person as owner (issue 491). "The calendar of an agent", "of a channel", "of a project" are
  queries over the entries, not three kinds of calendar.

Recommended: C. It needs no decision of ownership beyond what scopes already decide, and an entry
cannot outlive the place it wakes something in.

### What happens when an entry falls due

Four scenarios, from simple to rich:

1. **A reminder here.** The agent that made it is given its text to read in the channel and thread
   it was made in, and responds there. Default, and probably most entries.
2. **Work of its own, alone.** A session is made from a template the entry names, with the agent
   in it, and the agent is given the text there - a daily report written apart from every
   conversation, its result published to the session's project.
3. **A meeting.** The same, with the participants the template - or the entry - names: agents are
   given the text, users are told as they are told of a mention (issue
   [594](594-a-user-is-told-of-what-concerns-them-in-no-one-way.md)).
4. **A note to a person.** Nothing wakes an agent; the users of the channel are told. Possibly no
   case of its own: scenario 1, whose agent passes it on.

Scenarios 2 and 3 are one: "make a session of this template, give this text". So the entry needs
three things beside its time: *what* (the text to be given), *where* (here, or a session of a
template), *when* (once, or a rule of recurrence).

### Keeping the tool simple

One component, say `calendar`, with few tools:

- `schedule` - `text`, `at` or `every`, and optionally `template` (a new session instead of
  here); returns the entry's id and when it next falls due, in the session's time zone.
- `list_schedule` - the entries the agent may see where it is: those of the channel, its own.
- `cancel_schedule` - by id.

Recurrence: a small rule of the agent's language ("every weekday at 9:00", "every 2 hours") read
into a cron-like rule, or an RFC 5545 `RRULE`; a cron expression is what models write most
reliably. Time zone: the session's, as times are written to the model.

Users would see and cancel the entries of a channel in the frontends, and the owner those of their
agents.

### Further questions

- **Missed entries.** An entry that fell due while the server did not run: given once as the server
  starts, saying when it was due - not once per missed recurrence.
- **Bounds.** How many entries an agent may keep, how often a recurring one may fall due - limits
  of the plugin's section of the configuration, as `plugins.subagents` bounds sub-agents (issue
  [597](597-sub-agents-are-bounded-by-a-list-of-personalities-alone.md)).
- **Where it runs.** A scheduler that holds the next due times and acts in the kernel's name has to
  be trusted as the kernel: a plugin (`caelumora-calendar`, entity, manager, scheduler) with a
  library beside it for the component, as `caelumora-subagents` is built.
- **Approval.** Whether scheduling asks for approval: a reminder hardly; an entry that starts a
  session every hour perhaps - configurable per component (`requires_approval`), as for sub-agents.

## Done when

- The four shared questions are decided (here or in an issue of their own).
- An agent schedules a reminder in its channel and is woken there with its text when it falls due,
  also after a restart; it lists and cancels its entries.
- An entry can start a session of a template and give the text to the agents in it.
- Recurring entries and missed entries behave as decided; covered by tests.
- Documented in a document of its own or in CHANNELS.md and TOOLS.md.

## History

- **2026-11-27** — Opened on the maintainer's proposal of a calendar for agents, with first
  approaches; the shared foundation with 613 and 614 is described here.
