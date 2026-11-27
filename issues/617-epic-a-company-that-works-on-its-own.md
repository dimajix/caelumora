# 617 — Epic: a company that works on its own

**Status:** open
**Type:** epic
**Priority:** medium
**Area:** Sessions and channels
**Summary:** Epic for modelling a whole company in Caelumora that works on its own: nothing shows how agents, people, sessions and projects are to be put together so that a company does its work with little human help, and the gaps such a company runs into are not known; the goal is an example seed of a complete company and every gap closed it needs.

## The goal

An example seed of a complete company - personalities, session templates, component bundles,
skills, projects - for a field of business still to be chosen, which runs with little human help:
it takes in work, does it, delivers results, and asks a person where a person has to decide. And
every gap Caelumora has for that, closed on the way.

## The hypothesis

The maintainer's hypothesis, to be tested by this epic:

- **An org chart is not enough.** Laying an organisational structure over the agents, as books
  describe one - departments, hierarchy, reporting lines -, copies the form of a human company, not
  what makes it work. What has to be designed is which **communication creates value**: who has to
  hand what to whom, who has to decide, who has to know.
- **Agents are not people.** Caelumora can afford what a human company cannot: agents that live
  for one task and are gone (sub-agents, issue 558), and one agent working in several channels and
  sessions at once (ARCHITECTURE.md *The domain*: one actor in each channel, one state across
  them). Roles a company fills with one person each because people are scarce need not be one agent
  each - nor one agent at a time.
- **Organisation is still a question**: who owns results, who answers for what, whose credentials
  are used, who may decide what, what goes to a person.

## Questions to work through

### Which communication creates value

A first sorting, to be tested on the chosen field:

- **Handing over a task and taking back a result** - a brief and what came of it, the shape of a
  sub-agent ([558](558-no-agent-hands-a-task-to-a-sub-agent.md)): no meeting, no chat, a closed
  thread.
- **A decision by a person** - an approval, a question put to a user
  ([APPROVALS.md](../docs/APPROVALS.md)): the one place a person is needed, so it has to be rare
  and well prepared.
- **Shared state instead of talk** - artifacts in a project, notes on a channel's board, a
  workspace: who needs to know reads, instead of being told in a meeting.
- **Review** - a second agent with another personality looking at a result before it goes out.
- **The door to the outside** - customers and suppliers by mail and messenger
  ([613](613-an-agent-cannot-read-or-send-email.md), [614](614-no-messenger-reaches-a-channel.md)).
- **Time** - recurring duties, deadlines, follow-ups
  ([612](612-an-agent-cannot-keep-an-appointment.md)).

What is left out on purpose - status meetings, reporting up a hierarchy, coordination for its own
sake - is as much a result as what is kept.

### What the organisation is

- **Roles are personalities**, departments perhaps projects: a project holds what outlasts a piece
  of work (PROJECTS.md). Whether a long-lived agent per role, or agents made per task from a
  role's personality, is the first design question.
- **Who owns a result.** Today only a person owns anything; a team cannot
  ([340](340-teams-as-owners.md)). A company's results being one person's is the problem 340
  describes.
- **Whose credentials.** A company's accounts - its mailbox, its bot, its API keys - are no single
  person's ([343](343-users-bring-their-own-secrets.md), 340).
- **Who may do what.** A personality limits its agents (CONFIDENTIALITY.md *Personalities limit
  their agents*); sub-agents are bounded
  ([597](597-sub-agents-are-bounded-by-a-list-of-personalities-alone.md)). Whether that suffices
  for a company's division of authority is to be seen.
- **Memory and character.** What an agent of a role learns over months - its memory, a character of
  its own ([582](582-a-soul-the-agent-writes-itself.md)).

### What running on its own means

To be measured, not assumed: for instance, a week of the company's work with a person only
deciding what is put to them - how many decisions, how many turns and tokens, what went wrong. A
company that runs for weeks also runs into resource limits
([378](378-resource-management-epic.md)).

## Gaps known so far

Candidates for the issues this epic brackets; the analysis of the first step will add and remove:

- [612](612-an-agent-cannot-keep-an-appointment.md) - work at a time, and the foundation of work
  that begins without anyone speaking.
- [613](613-an-agent-cannot-read-or-send-email.md), [614](614-no-messenger-reaches-a-channel.md) -
  the door to the outside.
- [591](591-an-external-agent-cannot-be-a-sub-agent.md) - agents elsewhere as sub-agents.
- [616](616-a-model-misreads-what-starting-a-sub-agent-does.md) - local models that use sub-agents
  as meant.
- [340](340-teams-as-owners.md), [343](343-users-bring-their-own-secrets.md) - a company's
  ownership and credentials.
- [604](604-the-personalities-of-sub-agents-are-the-agent-side-s-word.md) - the personalities of
  sub-agents checked on the kernel's side.
- [582](582-a-soul-the-agent-writes-itself.md) - an agent that develops over time.

## Recommended order

1. **Choose the field.** Candidates, each with what it tests:
   - *A small software house* maintaining a library: issues in, changes out - tests workspaces,
     review and sub-agents; Caelumora itself as the customer is close at hand.
   - *An agency of research reports or technical writing*: requests by mail, reports out as
     artifacts - tests the door to the outside, projects and review.
   - *Customer service of a small online shop*: messenger and mail, an order system by API -
     tests the door to the outside and decisions by people, little creative work.

   Recommended: the agency - it touches the most gaps with the least domain one has to build
   oneself. The maintainer chooses.
2. **Map the value streams** of the chosen field before any personality is written: which work
   comes in, what is delivered, which communications are needed for it - a document of its own,
   recorded here.
3. **Write the seed** with what exists, in a directory of its own beside `examples/seed`, and run
   it; what does not work becomes an issue of this epic.
4. **Close the gaps** in the order the runs show them; 612's foundation probably first.
5. **Run it for a week** and record what came of it here.

## Done when

- The field is chosen and its value streams are mapped.
- The example seed of the company is in the repository, documented, and runs the agreed time with
  people only deciding what is put to them.
- Every gap it needed is closed or recorded as an issue with a decision.

## History

- **2026-11-27** — Opened on the maintainer's proposal of modelling a company that works on its
  own, with the maintainer's hypothesis and first approaches.
