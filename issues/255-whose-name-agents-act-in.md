# 255 — Agents read in their owner's name and act in the name of whoever asked them

**Status:** open
**Area:** Identity & access management

## The problem

An agent acts in two security contexts, and which one an action is asked in depends on where in the
code it happens, not on a rule anyone decided (see
[SECURITY.md](../docs/SECURITY.md#in-whose-name-an-agent-acts)):

*   **Its own context** - its owner as initiator, the agent as caller (`Agent::security_context`,
    issue [87](87-security-context-in-entities.md)). Its views are made in it, so everything it
    reads through them is asked in its owner's name: other channels' history and notes, its private
    notes, its memory's recall, and the session's workspace through its channel. So are its
    components built, and what it says when nobody asked it.
*   **The turn's context** - the request's initiator, whoever asked the agent, continued as the
    agent (`Answering::to`). Its answers are published in it, its tool calls admitted in it
    (`CallTool`) and their results admitted into the channel, the credentials of a call read in it
    (`ReadSecret` for an MCP server or an external agent), and the questions of a call put in it.

Both meet in a single call. When bob asks alice's agent to run a command (issue
[251](251-open-terminal.md)), the call is asked as "bob as helper" and the workspace it runs in is
reached as "alice as helper" - the security suite pins exactly that
(`an_agents_command_is_called_for_who_asked_and_reaches_the_workspace_through_its_channel`).
Each half has a reason: an agent must not know more than the person responsible for it (issue
[142](142-roles.md)) and passes on what it read (issue
[131](131-agent-memory-bypasses-policy.md)); a person who asks lends the agent their authority for
the turn. But nobody decided that the two halves belong together this way, and where a new
component lands - in a view, in the turn - silently decides whose rights it uses.

## Why it matters

- **A confused deputy in one direction.** What the agent reads, it reads with its owner's
  clearance and brings into a channel where someone else asked. The flow rules keep what is read
  within the channel's classification, so a reader of the channel learns nothing above it - but
  within that bound the asker reaches, through the agent, whatever the owner may read and they may
  not: another channel of the session they are not in, the owner's workspace.
- **A borrowed credential in the other.** A call reads its credential in the asker's name, so an
  MCP server's token must be readable by every person who may ask the agent - or the call fails for
  some of them and not for others, depending on who asked, not on what the agent is.
- **Inconsistency is a risk of its own.** A tool that reads through a view and acts through the turn
  is checked by two people's rights at once, and its author has to know which part is which.
  External agents, notes, memory and the workspace each made that choice on their own.
- **Delegation chains.** An agent asked by an agent asked by a person: the turn's context continues
  the first initiator, the views the owner of the last agent. Issue
  [12](12-user-and-agent-permissions.md) asks for a rule over the whole chain; there is none yet.

## Considerations

- **Keep the split, decide it.** Reading in the owner's name, acting in the asker's, written down as
  the rule - with every kind of action assigned to one side explicitly (in `ActionType`, say), so
  that it no longer follows from where the code sits. Cheapest; leaves the confused deputy within
  the channel's bound.
- **Everything in the owner's name.** The asker only triggers the agent; the agent is its owner's
  delegate in all it does. One subject per agent, credentials of its own (12's *grant*). Whoever may
  ask the agent uses its owner's rights - acceptable only where asking is itself restricted, which
  it is not today: every member of a channel may address every agent in it.
- **Everything in the asker's name.** The agent can do and read only what the asker could. No
  deputy at all, but an agent with a credential its users lack is impossible, and what an agent
  remembers or noted in one turn would be readable or not depending on who asks next.
- **The intersection.** Every action asked in both names (12's conservative rule): the agent reads
  and does only what its owner and the asker may. Safe and consistent; it costs a second question
  per check, forbids the legitimate case of an agent holding a capability its users lack, and needs
  a rule for the chain.
- **Who may ask.** Part of the answer may lie not in the checks but in who may ask an agent to act
  at all - an `AskAgent` action decided by the owner, say - which would make "the owner's name"
  safe where today it is not.

Related: [12](12-user-and-agent-permissions.md) (the general rule for combining rights, which this
is a concrete case of), [142](142-roles.md) (an agent's clearance is its owner's),
[181](181-where-roles-decide.md) (where roles enter a check),
[213](213-external-task-taken-up-keeps-old-work.md) (whose authority an external task keeps).

## Decision

An agent is its owner's delegate. It acts with its owner's rights in everything it does - what it
reads, what it calls, the credentials it reaches with, the clearance it is judged by - and whoever
asks it lends it nothing. This is the model of a person one turns to because they are entitled to
something: they check the request, and carry it out with their own rights. The three parts of it:

1.  **The agent acts with its owner's rights.** The policy judges every action of an agent by the
    person responsible for its caller - the *principal* - and the context carries that person.
    The asker never enters a rights check.
2.  **The owner checks the request.** That has a place already: `GuardedToolExecutor::admit`, where
    the policy, the owner's rules for the place (issue 13, evaluated in their name) and the
    approval by the owner or their contributors (issue 14) decide one after the other, and a
    personality's limiting roles narrow the agent. A tool that needs no approval is the owner's
    approval given in advance.
3.  **The asker may ask.** Today by being a member of the channel the owner put the agent into;
    an explicit `AskAgent` would refine this sentence, not change the rule. The answer reaches the
    asker only as the flow rule lets it into the channel.

Who asked stays in the context, as its initiator: for the questions a call puts (issue 165), for
the record, for a later `AskAgent` - never for rights. Issues 211 and 222 speak of the initiator
and stay true as written.

**No chain.** `delegation_ids` goes: nobody reads it, and what it records - the actors whose
message caused the next turn - is a conversation between members, not a commission. An agent that
mentions another does not delegate to it; the other answers for its own owner, bounded by the
channel. A chain is reconsidered when there are subagents, and 12's rule over "the full delegation
chain" is narrowed to two identities, principal and initiator, deliberately.

**What this accepts.** An owner who puts an agent into a channel without approval lets every
member read through it whatever the agent may read, within the channel's classification. That is
a consequence of 2, not a gap in 1: the answer to it is approval, rules and limiting roles, not
the asker's rights. SECURITY.md says so.

## Plan

Written against the code as of `63affb0`. Three steps: the shape of the context, without a change
in behaviour; the rule, in the policy; one context per piece of work. Each is reviewed on its own.

### A. What the code shows

- **The policy hardly reads the initiator for an agent.** In `OwnershipSecurityManager::check`
  (`caelumora-model/src/security.rs`) `CallTool`, `ReadSecret`, `ShowCommand` and
  `ExecuteCommand` are `true`; `ReadChannel` decides by the two `Place`s alone; `Publish`,
  `ModifyMessage` and `RevokeMessage` by the caller; clearances come as roles from the runtime
  (`Member::clearances`, always the owner's). The initiator decides `Get*`, `Update*`, `Delete*`,
  `ReadDeleted`, `Restore` - for an agent's views that is `GetSession`. So the split is mostly a
  record in the security suite and a contract for other policies, and the rule is cheap; the
  plumbing is what costs.
- **Who reads `initiator_id` outside the policy**, and for which of the two questions:
  - *rights*: the store filters (`store/access.rs`), the roles for a clearance (`view/mod.rs`
    `user_roles`, `workspace.rs` twice, `session.rs` `require_cleared` and `preferred_language`),
    the owner of what is created (`session.rs` `create_session`, `template.rs`, `workspace.rs`),
    whether a viewer takes part (`view/channel.rs` `shows_approvals` and once more);
  - *who asked*: the questions of a call (`tool/mod.rs` `questions`, whose comment believes the
    initiator is the owner), the participants of a new channel (`session.rs` `create_channel`,
    the creator first).
- **Nobody reads `delegation_ids`.** It is extended at three places: `Answering::to`
  (`actor/mod.rs`), `Agent::security_context` (`agent.rs`), and the approver, which appends itself
  to the agent's turn context when it forwards a request and announces a decision
  (`actor/approval.rs`, three times) - a copy of `EventContext::next` onto the security context.
  An envelope travels through the channel unchanged.
- **One context per work is nearly there.** `Work` (`caelumora-model/src/work.rs`) holds the
  turn's `security_context`, stored in `TurnState` and `OpenTask`; `AgentWork::security_context`
  and `Turn::security_context` read it. The agent's own context is used for: `ViewsFactory`, built
  once per actor (`actor/agent/mod.rs`, `actor/external_agent.rs`, `ActorRuntime::views`);
  `ComponentContext::security_context`, which `agent_memory`, `notes` and `private_notes` keep as
  a field and ask with (`recalled`, `forget`, `reclassify`), and which `mcp_tool` and
  `a2a_agent` read their credential with at build time while `mcp_tool::execute` and
  `A2aClient` read it in the work's; `Answering::own`; `take_in`'s perceive processors.
- **The suite's recorder** (`tests/security_suite.rs`, `Recorder::check`) prints
  `initiator as caller`.

### B. The model (`caelumora-model`)

1.  `SecurityContext { initiator_id, caller_id, principal_id, session_id, channel_id }`:
    `principal_id` is the person whose rights the caller exercises - a user themselves, an agent
    its owner, the system itself. `delegation_ids` is removed. `new(actor, session, channel)` sets
    all three ids to `actor`; `next(caller, principal)` replaces `next(caller)`. The doc comment
    says which field answers which question. `Agent::security_context` becomes
    `new(owner, session, channel).next(Agent(id), owner)`.
2.  The policy (step 2): `let me = context.principal_id`. Nothing else in `check` changes - the
    confidentiality actions and the caller-bound ones already do not look at the initiator. The
    doc comment of `OwnershipSecurityManager` ("who *you* are is the context's initiator") says
    principal instead, and why.
3.  `ActionType` is unchanged: the decision is not per action any more. The doc comment of
    `ReadSecret` says whose credential it is.

### C. The runtime (`caelumora-runtime`)

4.  **The approver** (step 1) no longer calls `next` on the security context it forwards and
    announces in; the event context keeps its `next`. The approver is not a delegate, it answers.
5.  **`Answering::to`** (step 1) takes the owner besides the agent's id - `to(request, id, name,
    owner)` or the agent's attributes - and continues the request's context with
    `next(Agent(id), owner)`. Both actors pass what they hold (`agent_controller`,
    `self.security_context.principal_id`). `Answering::own` is unchanged.
6.  **Readers of `initiator_id` for rights** (step 2) read `principal_id`: the store filters, the
    roles lookups, the owner of what is created, `shows_approvals`. For a user's context nothing
    changes; for an agent's turn context the roles become the owner's, as the agent's own context
    had them. The two readers for *who asked* stay with `initiator_id`; the comment in
    `tool/mod.rs` `questions` is corrected.
7.  **One context per work** (step 3): the views an agent's turn navigates are built in the work's
    context. `ViewsFactory` loses its `context` field; `AgentWork::session` passes
    `self.work.security_context`, `ViewsFactory::session(context, channel, classification)`.
    `agent_memory`, `notes` and `private_notes` ask `recalled`, `find`, `forget` and `reclassify`
    with the work's context, which they are handed anyway, instead of the field - the field stays
    for what is decided when the component is built. `mcp_tool` and `a2a_agent` are unchanged:
    they read their credential at build time in the agent's own context and at call time in the
    work's, and from step 2 on both have the same principal. `take_in` and `Answering::own` keep
    the actor's own context: nobody asked.
8.  **The log** (step 2): a refusal is logged by `ActionType::describe` as before; the recorder
    of the suite (E) is where the identities show.

### D. The frontends

- Nothing. The REST server and the TUI build a user's context with `new`, which sets the
  principal; the web UI sees no context.

### E. Tests

9.  **The recorder** prints `principal as caller` and, where the initiator differs, `, asked by
    initiator` - so that a test reads `alice as helper, asked by bob: CallTool yes`.
10. **Tests that change what an action asks** (step 2), reviewed as such:
    `an_agents_command_is_called_for_who_asked_and_reaches_the_workspace_through_its_channel`
    and `an_agents_file_tool_asks_what_a_command_asks` (`asked_as_a_call_on_the_workspace`: the
    call and the workspace both in alice's name, asked by bob);
    `an_external_agents_task_is_followed_in_the_name_of_whoever_asked`,
    `an_agent_asked_by_several_while_it_answers_answers_once_in_the_name_of_the_last` and the
    cancelled task's test (`ReadSecret` in alice's name, asked by bob, by the last who asked):
    renamed to say what they pin now - the asker is recorded, the owner is judged.
11. **New in the suite** (step 2), for the *done when*: a delegation through a second agent -
    carol asks alice's agent, which mentions bob's agent, whose tool call is judged as bob,
    asked by carol; reading another channel through a tool, judged as the owner; and the
    questions of a call put to the asker, not the owner.
12. **Unit tests** that build a context with `next(agent)` (`security.rs`, `view/mod.rs`,
    `workspace.rs`, `channel.rs`, the component tests, `external_agent.rs` in the model) pass
    the owner; `tool/mod.rs`'s tests that set only `caller_id` set `principal_id` where the
    policy is the ownership one.
13. Step 1 ends with the suite unchanged but for the recorder: the proof that the shape alone
    changes no behaviour.

### F. Documents

- SECURITY.md: *The security model* (what the three ids of a `SecurityContext` answer); *In whose
  name an agent acts* rewritten as decided - the three parts, what it accepts, and that
  `GuardedToolExecutor::admit` is where the owner checks; the ownership policy's bullet on who
  "you" are; *Not decided yet* loses nothing, `AskAgent` is named as a refinement.
- CHANNELS.md: *its workspace* ("an agent acting there for whoever asked it"), the transport's
  paragraph on who an actor acts for, and the external agent's open task ("with the authority of
  whoever asked") say owner and asker apart.
- CONFIDENTIALITY.md, *What leaves through tools*: the tool sets are judged in the owner's name.
- STORAGE.md: what `Work` keeps, if it names the fields.
- Issues: this one and [12](12-user-and-agent-permissions.md) (history, done), and
  [181](181-where-roles-decide.md) (whose roles count when an agent acts: its owner's, by this
  decision - a history entry).

### G. Steps

1.  **The shape** (B1, C4, C5, E9, E12, E13): `principal_id` in, `delegation_ids` out, the
    approver no longer a delegate. No behaviour changes; the policy still judges by the
    initiator. **Stored data:** `Work` is stored in the state store, in every
    `AgentChannelState` with a turn or an open task - the agent channel states are to be deleted.
2.  **The rule** (B2, B3, C6, C8, E10, E11, F): the policy and the rights readers judge by the
    principal. The suite's changed tests are the review of what changes.
3.  **One context** (C7): views and components ask in the work's context; the agent's own context
    is only where nobody asked. No behaviour changes after step 2, since both contexts then
    carry the owner as principal; the step removes the second way of reaching a context.
4.  **Close**: history here and in 12 and 181, status `done`.

### H. What to watch

- **`tool_sets` at the start of a turn** (`request_response`) is judged in the turn's context:
  from step 2 on as the owner, so a set is withheld or offered alike whoever asks.
- **`ExternalAgentActor::acting_for`** keeps a task's stored work context; a task stored before
  step 1 does not load (G1).
- **Where the principal is `None`.** `SecurityContext::default()` has none, as it has no
  initiator: no rights, as before. A channel's and a session's components are built in a context
  of their owner (`ChannelLoop::new`, `new_command_executor`) - `new`, so the principal is set.
- **The user actor's context** is what its answers carry and the approver checks
  (`ApproveToolCall`); `new` sets the principal, nothing else to do.
- **`transport/memory.rs`** compares the caller of an envelope with its origin; unchanged.
- **`RemoteState` is `Eq` over `Work`**, hence over the context: the new field takes part.
- **The recorder's names** come from the users and agents the test world knows; a principal is
  always one of them.

## Done when

- It is decided, per kind of action, in whose name an agent asks - and the decision is in
  SECURITY.md and in the code at one place, not spread over views and turns.
- The security suite pins the decision for reading another channel, the workspace, a tool call, a
  credential and a delegation through a second agent.

## History

- **2026-10-08** — Opened at the maintainer's request, after the security suite test of step 5 of
  [251](251-open-terminal.md) showed a command called in the asker's name and its workspace reached
  in the owner's. The current model was written down in SECURITY.md (*In whose name an agent acts*)
  as it is, not as decided.
- **2026-10-08** — Decided with the maintainer (*Decision*): the agent is its owner's delegate and
  acts with its owner's rights in all it does; the owner checks the request (`admit`), the asker
  may ask and is recorded as the initiator, never judged. Against the intersection: one turns to
  the person entitled, who checks and acts with their own rights - cutting the rights to what
  both may is the wrong model. Found on the way: `delegation_ids` is read by nobody and records a
  conversation, not a commission; the approver appends itself to it; `mcp_tool` and `a2a_agent`
  read their credential at build time in the owner's name and at call time in the asker's; the
  comment of `GuardedToolExecutor::questions` believes the initiator is the owner. A plan is
  written (*Plan*), in three steps: the context's shape, the rule, one context per work.
- **2026-10-08** — Step 1 done: `SecurityContext` carries `principal_id` instead of
  `delegation_ids`; `new` sets caller, principal and initiator to one actor, `next(caller,
  principal)` continues a context. `Agent::security_context` and `Answering::to` give an agent its
  owner as principal - `Answering::to` and `reject_retries` take it, both actors pass the
  principal of their own context. The suite's recorder prints `principal as caller, asked by
  initiator`; seven tests changed in the names they record, none in an answer. Deviating from the
  plan (C4): the approver still continues the context it forwards and announces in, as
  `next(approver, approver)` - the transport admits an envelope only from the actor it names as
  its caller (`OriginEventSender::speaks_as`). Without a chain it appends itself to nothing; it
  speaks for itself, with whoever asked as the initiator. SECURITY.md describes the three
  identities. **Stored data:** the agent channel states in the state store - a `Work` in a turn or
  an open task holds the context - are to be deleted.
- **2026-10-08** — Step 2 done: the ownership policy judges by the principal (`me =
  principal_id`), and so do the readers of the initiator that asked about rights - the store
  filters' test policy, the roles for a clearance (`view/mod.rs`, `workspace.rs`, `session.rs`),
  `may_see_channel`'s participant, the owner of what is made (`create_from_template`,
  `template.rs`, `WorkspaceManager::create`) and the refusal's log line. The existing suite saw
  no change in any answer - as A said, the policy hardly read the initiator for an agent -, so
  its tests changed only in names and messages: the terminal test is
  `an_agents_command_is_called_for_its_owner_and_reaches_the_workspace_through_its_channel`, the
  external agents' `an_external_agents_task_is_followed_for_its_owner_as_whoever_asked_began_it`,
  `an_agent_asked_by_several_while_it_answers_answers_once_as_asked_by_the_last` and
  `lowering_a_channel_gives_up_an_external_agents_task_as_whoever_asked_began_it`. New in the
  suite: `an_agent_asked_by_an_agent_acts_for_its_own_owner` (carol asks bob's agent, which asks
  alice's: judged as alice, asked by carol) and
  `questions_for_the_owner_are_put_to_the_agents_owner_whoever_asked`; in the model
  `an_agent_is_judged_by_its_owner_not_by_whoever_asked_it`. SECURITY.md (*In whose name an agent
  acts*, the ownership policy), CHANNELS.md and CONFIDENTIALITY.md say it as decided. Deviating
  from the plan, two readers were sorted wrongly in A:
  - **The questions of a call** (`GuardedToolExecutor::questions`) take the principal, not the
    initiator: what a tool puts to `Addressee::Owner` is meant for the agent's owner, and went to
    whoever asked - a tool asking its owner for a password asked bob when bob asked alice's agent.
    This changes behaviour, and the new test fails with the old. The decision's sentence that the
    initiator is kept "for the questions a call puts" is wrong in that; the initiator is kept for
    the record and for which request a turn answers.
  - **Whether the viewer takes part** (`ChannelView::shows_approvals`, and which sensitive
    questions a member's snapshot shows) asks the caller, not the principal: it is about who
    looks, not whose rights they have - an agent's views must never show it the questions put to
    its owner in person. For a user the three are one, so nothing changes.
  Reading another channel through a tool is pinned with step 3, where the views are asked in the
  turn's context; before it, they are asked in the agent's own, which names nobody but its owner.
- **2026-10-08** — Step 3 done: one context per piece of work. `ViewsFactory` keeps only where
  the agent's session is reached; `ViewsFactory::session` takes the context, and
  `AgentWork::session` passes the work's. `ActorRuntime::views` takes none. `AgentMemory`
  (`recalled`, `find`, `forget`, `reclassify`) and `AgentNotes::reclassify` ask in the context of
  the turn or work they are given; `agent_memory`, `notes` and `private_notes` no longer keep a
  `SecurityContext` - it was only ever asked in during a turn. `mcp_tool` and `a2a_agent` are
  unchanged: their credential is read in the build-time context once and in the work's at every
  call, both with the owner as principal. The suite's workspace tests now see every question of
  the call in one context (`alice as helper, asked by bob`), answered as before. New in the suite:
  `an_agent_reads_another_channel_with_its_owners_rights_whoever_asked`. COMPONENTS.md, MEMORY.md
  and SECURITY.md say what a component's own context is for. Found on the way: reading another
  channel of the same session asks only `ReadChannel` - the flow rule between the channels'
  classifications, by design (issue 133) - and no `GetChannel`, so under the ownership policy
  whose rights an agent reads with does not decide it; it does for another session (privacy, issue
  134) and for a policy that looks at the principal. bob's agent in a channel with alice reads her
  channel "side" of the session if its classification flows into theirs.
