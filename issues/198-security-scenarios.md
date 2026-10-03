# 198 — Nothing says, end to end, what users with different permissions may do

**Status:** done
**Area:** Identity and access management

## The problem

The ownership policy is tested action by action (`caelumora_model::security`), and some operations
check themselves in their own tests (`session.rs`, `channel.rs`). What is not written down anywhere
is how these checks combine when real users act through the whole runtime: a personality one user
owns and another uses, an agent whose personality is withdrawn, a stranger asking for a session that
exists. Each such case touches several entities with different owners, and the expected behaviour
is a decision, not a consequence of one rule.

## Why it matters

Without the scenarios written down, a change to one check cannot be judged against what it should
leave intact, and a gap between two checks goes unnoticed - the first run of the suite below found
one (P3).

## Considerations

- **One suite through the public API.** `caelumora-runtime/tests/security_suite.rs` runs the
  runtime as a frontend does: the `MemoryStore` checking with the `OwnershipSecurityManager`, the
  `SessionManager`, running channels and the views. What the system starts from is put into the
  store as the system, as a setup or an administrator would; everything a user does goes through
  the public API in that user's name. Each test names its scenario here.
- **A recording policy.** The suite's `SecurityManager` is the `OwnershipSecurityManager`, wrapped
  so that it records every question it is asked: who asks - a user, or an agent on its owner's
  behalf -, which action, of whose entity, and the answer. A test of an action asserts the exact
  questions in order: that is the record, in code, of which permissions the action needs. What the
  runtime asks in its own name (`SecurityContext::system()`, reading controllers, roles and
  personalities for itself) is left out: it may do everything, and is no permission of anyone's.
- **Three users.** Alice and bob are on the staff (cleared for `internal`); carol is a stranger to
  everything of theirs. A session "shared" is alice's, with bob in it.
- **A refusal answers "not found" for what the caller may not see,** so that ids cannot be probed
  (SECURITY.md, *How stores check*); "permission denied" only for what they may see but not change.
- **Not repeated here:** what classifications decide - who is cleared for a channel, an agent
  cleared only as far as its owner and its personality, reclassifying a channel and its messages,
  reading other channels - is tested in `session.rs`
  (`only_who_is_cleared_takes_part_in_or_reads_a_channel`,
  `an_agent_is_cleared_only_as_far_as_its_owner_and_its_personality_are`,
  `a_channel_and_its_messages_are_reclassified_by_who_may`,
  `a_channel_reads_another_by_their_classification_not_by_who_takes_part`), and what an agent may
  recall in `component_impl/agent_memory.rs`.
- **A session has one user through the public API.** A template binds only its "user", the
  session's owner; there is no operation that adds a second user to a session. The suite stores the
  shared session as the system therefore. Whether sessions of several users are to be made through
  the API is a question of its own, not of this issue.

## The permissions each action needs

Asked in the acting user's name, in this order; "of X's" is whose entity the question is about. A
refusal ends the action where it is answered. Agents are named by their names, `alice as helper`
being her agent `helper` acting on her behalf.

| Action | Who | Questions, and answers | Test |
|---|---|---|---|
| Make a session from a template | alice, system's template, bob's personality shared with her | `GetSessionTemplate`(system's), `GetUser`(system's), `ListAgents`, `GetPersonality`(bob's), `CreateAgent`(alice's), `Classify`, `JoinChannel`(alice), `alice as helper: GetPersonality`(bob's), `JoinChannel`(helper), `CreateSession`(alice's), `CreateChannel`(alice's), `alice as helper: GetPersonality`(bob's) - all yes | `making_a_session_needs_the_template_the_personalities_and_creating_what_is_made` |
| … again, the agent found | alice | no `GetPersonality` nor `CreateAgent` for making it; the agent's own `GetPersonality` to run it | same |
| … from a template one may not read | carol, bob's private template | `GetSessionTemplate`(bob's) no | same |
| … with a personality one may not read | alice, bob's private personality | `GetSessionTemplate`, `GetUser`, `ListAgents` (filtered by `GetAgent` of each), `GetPersonality`(bob's) no | same |
| Open (start) a session | alice; bob, in it; carol | `GetSession`(alice's): yes, yes, no | `opening_a_session_needs_reading_it` |
| Stop a session | carol; bob; alice | carol: `GetSession` no. bob: `GetSession` yes, `StopSession` no. alice: both yes | `stopping_a_session_needs_owning_it` |
| Delete a session | carol; bob; alice | as stopping, with `DeleteSession` - asked twice for alice, by the session manager before it stops anything and by the store | `deleting_a_session_needs_owning_it` |
| Create a channel | alice, with bob in it | `UpdateSession`(alice's), `Classify`, `JoinChannel`(alice), `JoinChannel`(bob), `CreateChannel`(alice's), `GetSession`(alice's), `UpdateSession`(alice's) - all yes | `creating_a_channel_needs_owning_its_session` |
| … by who may not | bob | `UpdateSession`(alice's) no, nothing else - **F2** | `who_may_not_create_a_channel_is_refused_before_the_others_are_looked_at` |
| Delete a channel | bob; alice | bob: `GetSession` yes, `UpdateSession` no. alice: `GetSession`, `UpdateSession`, `GetChannel`, `DeleteChannel`(alice's) - yes | `deleting_a_channel_needs_owning_its_session` |
| Make an agent | alice | only by making a session from a template: `ListAgents`, `GetPersonality`, `CreateAgent` above | `making_a_session_…` |
| Delete an agent | bob, owner of its personality; alice, its owner | bob: `GetAgent`(alice's) no - nothing to delete. alice: `GetAgent`, `DeleteAgent`(alice's) yes | `deleting_an_agent_needs_owning_it` |
| Add an agent to a channel | alice | `UpdateChannel`(alice's), `alice as helper: GetPersonality`(bob's), `JoinChannel`(helper), `GetChannel`(alice's), `UpdateChannel`(alice's) - all yes | `adding_an_agent_needs_owning_the_channel_and_the_agent_its_personality` |
| … whose personality was withdrawn | alice | `UpdateChannel`(alice's) yes, `alice as other: GetPersonality`(bob's) no | same |
| … by who may not | bob | `UpdateChannel`(alice's) no, nothing else - **F2** | `who_may_not_add_to_a_channel_is_refused_before_the_agent_is_looked_at` |
| Remove an agent from a channel | bob; alice | `GetChannel` yes, `UpdateChannel`(alice's): no for bob, yes for alice; nothing of the agent | `removing_an_agent_needs_owning_the_channel` |

**F2 - the actor's own permission comes last.** Creating a channel and adding a participant ask
first about the others - everyone's clearance (`JoinChannel`), an agent's personality in its name -
and only then whether the actor may change the session or channel at all; that is asked only when
the change is stored. Someone who may not change it is refused all the same, but learns on the way
what is not theirs to know: that another participant is not cleared (`NotCleared`), or that an
agent's personality was withdrawn ("permission denied" for the personality instead of "forbidden"
for themselves). Expected: the actor's permission first, as `delete_channel` and
`remove_participant` do; then the participants.

## Scenarios

### Personalities and agents

| No. | Given | Who does what | Expected | Test |
|---|---|---|---|---|
| P1 | Bob's personality, not shared | Alice makes a session from her own template naming it | Refused as not found; no session, no agent is stored | `a_session_cannot_be_made_with_a_personality_the_user_may_not_read` |
| P2 | Bob's personality shared with alice; the system's personality | Alice makes a session from a template naming it | An agent is made, owned by alice; bob, though the personality is his, neither reads nor changes it | `a_shared_personality_makes_an_agent_of_whoever_uses_it` |
| P3 | Alice's agent of bob's shared personality, in her session | Bob withdraws the share (or deletes the personality, P3b); alice opens the session again | The session opens; the agent is not run, as a deleted agent is not (issue 120); everyone else is | `an_agent_whose_personality_was_withdrawn_is_left_out_of_its_channel`, `…_is_gone_…` |
| P4 | As P3, the agent in the session but in no channel | Alice adds it to a channel | Refused; the channel is unchanged | `an_agent_whose_personality_was_withdrawn_cannot_join_a_channel` |
| P5 | Bob's personality shared with alice | Alice changes or deletes it; carol reads or lists it | Alice reads but neither changes nor deletes it; carol does not find it, nor is it listed; bob changes it | `a_personality_is_changed_by_its_owner_and_read_by_who_it_is_shared_with` |
| P6 | Alice's private personality; her agent of it in the shared session | Bob looks at the channel's members | He sees the agent, its name and its introduction - the public part of the personality - though he may not read the personality | `an_agents_introduction_reaches_whoever_sees_it_but_not_its_personality` |

Why P3 is expected as it is: an agent runs only with a personality it may read - its owner may no
longer use what was withdrawn, and the channel checks that in the agent's name (issue 191). But the
session is alice's, and the agent is one of its members: its owner's loss must not lock alice and
everyone else out of the session, and a third party - bob, who owns the personality - must not be
able to stop someone else's session by withdrawing a share. A personality that is gone is the same
case. The channel leaves such a member out with a warning, as it leaves out a user or agent that no
longer exists; adding it to a channel is refused (P4), since that is an act for which a reason can
be given.

### Templates

| No. | Given | Who does what | Expected | Test |
|---|---|---|---|---|
| T1 | A template of bob's, one shared with alice, one of the system | Alice makes a session from each | Bob's private one is refused as not found; from the others she makes a session that is hers, not the template owner's | `a_template_is_used_only_by_who_may_read_it` |

### Sessions and channels

| No. | Given | Who does what | Expected | Test |
|---|---|---|---|---|
| S1 | Alice's shared session | Carol opens, stops, deletes or lists it | Not found, every time; not listed | `a_stranger_finds_nothing_of_a_session` |
| S2 | Alice's shared session | Bob, in it, opens it, sees its members, writes, cancels; renames it, changes its privacy, adds or deletes channels, removes alice, reclassifies, stops or deletes it | He takes part - opens, sees, writes, cancels - but changes nothing: every change is refused, permission denied where he may see what he would change | `a_participant_takes_part_but_changes_nothing` |
| S3 | Alice's shared session | Alice renames it, adds a channel with bob, deletes it, deletes the session | All of it; the session is gone afterwards | `the_owner_changes_and_ends_the_session` |

### What an agent saw

| No. | Given | Who does what | Expected | Test |
|---|---|---|---|---|
| A1 | Alice's agent in the shared session | Alice and bob ask what it saw | Offered to and read by alice, its owner; neither offered to nor read by bob, though he reads the channel with it | `what_an_agent_saw_is_its_owners_alone` |

### Users

| No. | Given | Who does what | Expected | Test |
|---|---|---|---|---|
| U1 | Alice, bob and carol | Alice writes her profile; bob writes hers or carol's; alice grants herself a role; bob looks at alice in the shared session | Her own profile only; nobody writes another's, nobody changes a user record - roles are the system's; bob reads her introduction | `a_profile_is_written_by_its_user_alone_and_read_by_who_shares_a_session` |

### Not covered yet

- An external agent (issue 179): its personality's roles limit what it joins - tested in
  `session.rs`; what it is given only on mention - an example template's choice, not a rule.
- Tool calls and approvals: who approves under which strategy, and that an approval can only be
  given by a user of the channel.
- Secrets: a component reads only the secrets its policy allows (`ReadSecret`).
- Commands: who is shown and may run which command (`ShowCommand`, `ExecuteCommand`).
- Preconfigured components (`kind: ref`): who may use another's component.

## Done when

- Every scenario and every action above has a test in the suite, and all of them pass.
- P3 and F2 are decided and the runtime behaves as decided.
- New permission rules come with a scenario here.

## History

- **2026-10-02** — Opened at the maintainer's request, with the suite. Of thirteen tests, eleven
  pass as expected. P3 does not: a member whose personality was withdrawn or deleted makes its whole
  session fail to open, with "permission denied" or "not found" for everyone in it, the owner
  included. `ChannelLoop::new` lets the error of `Member::of` end the channel rather than leaving
  the member out as it leaves out a deleted one. The two tests are `#[ignore]` with this issue as
  the reason until it is decided.
- **2026-10-02** — At the maintainer's request, the actions themselves: making, opening, stopping
  and deleting a session, creating and deleting a channel, making and deleting an agent, adding and
  removing one, each by its owner, someone taking part and a stranger. The suite's policy records
  every question; each action's test asserts the questions asked in a user's name, in order - the
  table above. A second finding, F2: creating a channel and adding a participant ask about the
  participants before the actor's own permission. Its two tests are `#[ignore]` like P3's. 20 tests
  pass, 4 are ignored.
- **2026-10-02** — P3 done as expected: `ChannelLoop::new` leaves out a member that cannot be run -
  `Member::of` failing with not found or permission denied, its agent's personality gone or
  withdrawn - with a warning, as it leaves out one that no longer exists; a failing store still
  fails the channel. Its two tests run. CHANNELS.md says so under *Running channels*.
- **2026-10-02** — F2 done, and the issue with it: `create_channel` asks `UpdateSession` and
  `add_participant` asks `UpdateChannel` before anything of the others in the channel; the actor
  who may not is refused at once. Their two tests run - all 24 of the suite pass. SECURITY.md has
  the rule - the actor's own permission first - and a section on the suite, and AGENTS.md the rule
  that every new action, and every change to what one asks, comes with its test in the suite. The
  scenarios under *Not covered yet* are left for when their actions change.
- **2026-10-03** — A first test of an external agent, from issue
  [211](211-turn-and-agent-work-structure.md):
  `an_external_agents_task_is_followed_in_the_name_of_whoever_asked`. Bob asks alice's external
  agent; the remote agent is sent his request, and after the session was stopped and opened again by
  alice, the task is followed, in his name (`bob as hermes: ReadSecret`), not in that of the agent's
  owner. The suite's `World` takes component kinds of a test (`World::with_kinds`). The rest of *Not
  covered yet* stays open.
