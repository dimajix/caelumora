# 269 — Commands an agent carries out itself

**Status:** done
**Area:** User interaction
**Summary:** A user command can act on the channel but not on an agent - its transcript has one writer, the agent's actor - so neither `/compact` nor loading a skill for an agent can be built.

## The problem

A user command (issue [28](28-user-commands.md)) runs in the session's `CommandExecutor`, called by
the issuing user's `UserActor`, and acts on the channel through the channel's own operations
(`ChannelLoop::wakeup`). It cannot act on an *agent*: an agent's state in a channel - its
transcript, its notes, what it has loaded - has exactly one writer, the agent's actor there, and a
command has no way to reach it. Two things that are wanted cannot be built this way:

- **`/compact`**, named in issue 28 from the start and the subject of
  [16](16-context-compression.md): compressing an agent's transcript is a change of the agent's
  state in the channel, made - with a model call - by the agent alone.
- **`/skill <name>`** ([22](22-skill-management.md)): a skill a user loads for an agent goes into
  that agent's transcript, in this channel, and nowhere else - checked for the agent (`UseSkill`)
  and recorded as what the agent saw.

What a command may do to an agent is therefore limited to what the channel offers on the agent's
behalf, and the channel offers nothing that changes what an agent knows. A command that wanted to
would have to write into the agent's state from outside its actor, against "one writer per
document" ([ARCHITECTURE.md](../docs/ARCHITECTURE.md#principles)).

## Why it matters

- Both 16 and 22 need it; 22 is otherwise reduced to tools the model calls, so that a person
  cannot hand an agent a skill without asking the model to fetch it.
- The channel already has the shape for it: a user's actor sends an event addressed to one agent,
  the moderator routes it there alone, and the agent answers with an event of its own -
  `RetryRequested` / `RetryAccepted` / `RetryRejected` ([234](234-modify-revoke-and-retry-messages.md),
  [235](235-retry-of-an-answer-under-way.md)) do exactly that. Commands for agents are the same
  flow, generalised.
- Doing it any other way - a command that writes into `AgentChannelState` through the state store,
  a user actor that calls into the agent actor - creates a second writer for the agent's state, and
  everything that rests on one writer (staging and committing, the digest chain of the transcript,
  announcing after storing) has to be defended twice.

## Considerations

- **Commands for agents are commands.** They are typed as `/name @handle args`, recognised by the
  frontend by the same lexical rule, listed by `/help` and offered by completion, and checked with
  an action of their own before anything is sent. A user must not have to know which part of the
  system carries a command out. So the entry point stays `UserActor::execute_command`, and the
  session's `CommandExecutor` stays the one place that finds a command and checks who may see and
  run it. What is new is that a command found there may belong to an agent, and is then not run
  but *sent*.
- **Agents understand commands through components**, not natively. Native understanding -
  a `match` on the name in `AgentActor` - would hard-wire `/skill` into every agent, including those
  without a `skills` component, and `/compact` into agents whose personality has no room for a
  compaction. A command is contributed by a component of the personality, as a tool is: `skills`
  provides `/skill` with its tools, a `compact_command` component provides `/compact`. Which
  commands an agent understands then follows from its personality, is known to the views, and
  `/help` can list them per agent. That is also why the existing `CommandProvider` is not simply
  reused: its `execute` is given the session - the issuer, the channel, the other commands -
  and a command of an agent needs the agent's `Turn`, as a tool does. The two facets differ in
  what they are given, as `ToolProvider` and `CommandProvider` do, and are named by whose commands
  they are: `ChannelCommandProvider` (today's `CommandProvider`, renamed) and
  `AgentCommandProvider`. The type that holds them, checks for duplicates and asks the policy is
  one, generic.
- **An agent carries a command out in a turn of its own.** The alternative - changing
  `AgentChannelState` directly under the actor's lock, between turns - is simpler for `/skill`
  but cannot hold `/compact`: a model call, a result to stage, a state to store and announce, a
  crash to resume from. A turn gives a command all of that - `Turn::stage_…`, `Turn::commit`,
  `TurnFinished`, resumption after a restart - and gives the component the same handle a tool
  gets. The cost: a command turn is announced like an answer (`ResponseStarted`, `StateChanged`,
  `TurnFinished` with no messages), and `TurnState` has to say that it is a command's, so that a
  resumed one knows what to do. The turn's contexts are the request's, continued in the agent's
  name (`Answering::to`): the agent acts with its owner's rights, the issuer is its initiator
  ([255](255-whose-name-agents-act-in.md)) - `UseSkill` is asked of the agent for its owner, as
  every tool call is.
- **External agents carry commands out too.** An external agent has no cognitive cycle, but a
  command turn is not one: it is the handle through which a component stages and the actor
  commits, and `Turn::of` needs only what the `ExternalAgentActor` has as well - the agent, its
  channel state, its `AgentStateHandle`, its views. So one facet serves both kinds of agent, as
  `PromptProvider` and `PerceiveInProcessor` do (issue 204): `skills` puts a skill into the
  transcript, which an external agent gives to its remote agent with the next call as it gives a
  notice (issue 236); a `/compact` of an external agent is its component's - `a2a_agent` may
  implement the facet and tell the remote side, if its protocol has a word for it - or it has none.
- **The agent decides, the user asks.** The user actor checks what it can check before anything is
  sent - the command exists, the issuer may see and run it, the agent named is in the channel -,
  so that the user learns of a refusal at once, and the agent checks again what it takes, in the
  context the request came with, as the approver does with answers (issue 14) and the agent with
  answers to its questions (issue 165). What the command *does* is checked by the command, in the
  agent's context: `/skill` asks `UseSkill`, `/compact` whatever 16 decides.
- **The agent is always named.** `/compact` in a channel of three agents means nothing, and a rule
  that picks the one agent that understands the command makes the same line do different things
  as agents join. So a command of an agent always names its agent - `/compact @bob`, `/skill @bob
  release-notes` - and `@all` names every agent of the channel that understands it: one request
  per agent, each answered on its own, under one span. `@all` does not exist as a mention today;
  here it is the command's word, resolved by the user's actor, not a participant.
- **Who may run a command of a shared agent** is the question of
  [14](14-approval-privilege.md), again. `ExecuteCommand(name)` carries no attributes today, so the
  ownership policy cannot tell the agent's owner from a bystander. For a command of an agent the
  action has to carry the agent's attributes and the channel's; what the policy grants is then per
  command, not per policy: loading a skill for an agent is like writing to it, and everyone who may
  write in the channel may; compacting a shared agent's transcript is like approving its calls,
  the owner's and their contributors'. The command says which kind it is, as a tool says whether it
  requires approval.
- **The answer comes asynchronously.** A command sent to an agent is answered when the agent gets
  to it - after the turn it is in, after the retries of the same batch. The REST call returns once
  the request is on its way, and the answer reaches the issuer through their actor and its stream,
  the way an approval's outcome does. For `/help` and `/wakeup` nothing changes: a command the
  session runs answers at once, as today. A private answer is routed to the issuer alone, a
  channel-wide one to every user; `CommandExecuted` gets an addressee for that, as `QuestionClosed`
  has `shown_to`.
- **Commands are not messages, so they do not wake.** A request to respond or to answer again wakes
  a sleeping agent (`ChannelLoop::route_event_impl`); a command does not: `/compact` on a sleeping
  agent compacts and leaves it asleep. Nor does the agent respond to a command - it carries it out
  and ends the turn. A request to respond in the same batch begins its own turn after. A quiet
  channel - cancelled, not woken - delivers and carries out commands all the same: it drops only
  requests to respond.
- **A command is part of the record.** What an agent carried out on whose word belongs to the
  channel's history as a turn's end does (issue 258): the command, its arguments, who issued it,
  which agent - so that the chat shows it in its place after a reload, and the agent's history
  (issue 207) shows what was done to the agent. The answer's text is shown, not stored: a private
  answer is the issuer's, and the history is everybody's. A command refused or invalid did
  nothing, and leaves no entry.
- **A command takes effect at its place in what the agent takes in.** `take_in` perceives messages
  into the transcript as it takes them (issue 222). A command between two messages of one batch -
  a `/skill`, then the message that uses it - has to take effect between them, not after the
  batch, else the transcript's order differs from the history's. `take_in` therefore takes in up to
  the first command and hands it back; the actor carries it out and takes in the rest.
- **What the answer may say.** A command's answer is text its users see, unchecked: nothing
  withholds it as a tool's result is withheld ([133](133-classification-model.md)). A command
  answers what it did, not what it read - "Loaded skill 'release-notes'", not the skill - and
  anything that could carry what the channel may not see stays with the agent.
- **A restart** while a command is on its way loses the request, as it loses a request to answer
  again: the user sees no answer and asks again. A command *turn* under way is resumed as every
  turn is, and runs its command again from the start - a command is idempotent or says so, as a
  tool that was interrupted is asked to check its effect.
- **Which agents have which commands** is configured per personality, as tools are, and nothing
  else: a `/compact` every agent should have is listed in every personality. That is a chore the
  tools have already - the same components in every personality -, and the answer to it is not a
  second mechanism for commands but something like an inheritance between personalities, which is
  not this issue's.

## Decision

Commands for agents are built as the rest of the system is built: as components, sent as events,
carried out in turns.

1.  A component facet `AgentCommandProvider` (in `crate::command`, beside today's
    `CommandProvider`, which becomes `ChannelCommandProvider`) contributes commands an agent
    carries out, given the agent's `Turn`. Agents run here and external agents alike.
2.  `CommandExecutor` becomes generic over the provider facet and serves both: the session's
    commands, run by the `UserActor`; an agent's commands, run by its actor. Finding, checking for
    duplicates and asking the policy are written once.
3.  The `UserActor` finds every command through the session's executor; one that belongs to an
    agent it sends as `ActorEvent::CommandRequested` to the agent named - one per agent for
    `@all` -, which the moderator routes to it alone and which the channel admits only for an
    agent taking part, quiet or not.
4.  The agent carries the command out in a turn of its own - never retryable - and answers with
    `CommandExecuted` - to the issuer or to every user - or `CommandFailed` to the issuer. The
    channel records what was carried out in its history.
5.  The action asked for a command of an agent carries the agent's and the channel's attributes;
    the command declares whether it is for the agent's owners alone.

The user-facing shape: `/compact @bob`, `/skill @bob release-notes`, `/compact @all`; a command of
an agent without a handle is refused with the handles to choose from.

## Plan

Written against the code as of `42a7483`. Two refactorings without a change in behaviour first,
then the mechanism, then its frontends. No built-in command of an agent comes with it - the first
are `/skill` (22) and `/compact` (16) - so it is pinned by tests with a test provider, as the
`CommandExecutor` is today.

### A. What the code shows

- **Commands today.** `caelumora_runtime::command`: `CommandProvider { commands(), execute(name,
  args, &CommandContext) }`; `CommandContext { issuer, channel: &ChannelLoop, executor }`;
  `CommandExecutor { security_manager, entries: Vec<CommandEntry> }` with `new` (refuses two of one
  name), `visible(issuer)` (`ShowCommand`), `execute` (`Empty`, `Unknown` also for a hidden one,
  `ShowCommand`, `ExecuteCommand`, then the provider). Built once per session by
  `RuntimeManager::new_command_executor` from `AppConfig::commands` and `Session::commands`, under
  the session owner's context, shared by the session's channels through the `ActorRuntime`
  (`command_executor()`).
- **Running one.** `UserActor::execute_command(name, args) -> CommandReply { command, output,
  visibility }`: runs it, announces a channel-wide answer as `CommandExecuted { command, output }`
  through its own transport (`self.send`), returns the reply. REST `POST
  …/channels/{cid}/commands` (`routes/channels.rs::execute_command`) returns
  `CommandReplyResponse { command, output: Option<String> }`, the output only when private; the
  TUI (`main.rs:346`) prints the reply. `CommandError` words the refusals for the user.
  `wakeup_command` resolves its `[agent]` against the members' handles and names them all when it
  finds none.
- **The model.** `caelumora_model::command`: `Command { name, description, usage }`,
  `CommandVisibility { Private, Channel }`, `CommandOutput { text, visibility }`, `parse_input`.
  `HistoryEntry` (`history`) records what happened in a channel: messages said, changed,
  fitted, and every turn's end (`TurnFinished`, `TurnGivenUp`, `PlanStepStarted`).
- **The precedent.** `RetryRequested { target, author }` is sent by `UserActor::send_retry` with a
  `WakeupRequested` ahead of it; admitted by `ChannelLoop` (`may_retry_message`, `author` an agent
  taking part); routed by `PolicyModerator::route` with `route_to_agent(author)`; accepted by
  `AgentActor::event_filter`; collected by `intake::take_in` into its second return value;
  judged in `AgentActor::handle_waiting` after the batch is taken in, answered with
  `RetryAccepted` / `RetryRejected` in `Answering::to(request, …)` contexts, which
  `route_to_users` carries to every user. `ChannelLoop::route_event_impl` wakes the addressee of a
  `ResponseRequested` or `RetryRequested` before delivering, and drops a `ResponseRequested` while
  the channel is quiet.
- **A turn.** `AgentActor::request_response` begins one: `state.begin_turn(event_context,
  security_context, classification, tool_sets, profile)`, `control.begin_turn`, `ResponseStarted`,
  then the `ContextProvider`. `TurnState { work: Work, tools, memory, notes, private_notes,
  tool_calls, plan, requested_state, model_calls }`. `AgentActor::end_turn` commits, stores,
  announces `AgentChannelStateChanged` and `TurnFinished { messages }`; `fail_turn` gives up with
  `TurnGivenUp`. The handle is built by `Turn::of(agent_id, name, &mut state, agent_state)`
  `.with_views(views)` - from what both actors of an agent hold.
- **The external agent** (`actor/external_agent.rs`) shares `take_in` and `catch_up` with the
  agent run here, builds a `Work` per exchange with its remote agent and an `AgentWork` to read
  through, and has never begun a turn on its state.
- **The agent's components** are injected as `ComponentImpl<dyn …>` fields of `AgentActor`
  (`context_provider`, `llm_client`, `perceive_in_processors`, `emit_out_processors`,
  `tool_executor: GuardedToolExecutor` - whose default executor asks `context.all::<dyn
  ToolProvider>()`); the external agent's as fields of `ExternalAgentActor`.
- **The views.** `MemberSnapshot` carries per member what the viewer may do with it (`can_approve`,
  `can_retry`, …) and what it is doing (`state`, `turn_under_way`); the web UI builds its buttons
  from it. Nothing says what commands a member understands.
- **A message in the transcript alone**: `ExternalAgentActor`'s notices (issue 236) are
  `Message`s pushed with `state.transcript.push(..)`, never into a history - the shape a command
  that gives an agent something to read needs (22).

### B. Step 1 - one executor for both (`caelumora-runtime`, no change in behaviour)

1.  `CommandProvider` is renamed `ChannelCommandProvider`; `CommandContext` stays its context.
2.  `CommandExecutor<P: ?Sized>` with `entries: Vec<CommandEntry<P>>`, `CommandEntry<P> {
    command, component, provider: Arc<P> }`. Shared for every `P`: `new(security_manager,
    providers: Vec<ComponentImpl<P>>)` with the duplicate check - which needs the commands of a
    provider: a small trait `ProvidesCommands { fn commands(&self) -> Vec<Command> }` that both
    facets extend -, `empty`, `visible(issuer)`, and a private `admit(name, issuer) ->
    CommandResult<&CommandEntry<P>>` holding today's `Empty` / `Unknown` / `ShowCommand` /
    `ExecuteCommand` sequence, with the actions it asks given by the instantiation.
3.  `impl CommandExecutor<dyn ChannelCommandProvider> { pub async fn execute(name, args,
    &CommandContext) }` - `admit`, then the provider: today's `execute`, unchanged for its callers.
    The tests of `command/mod.rs` move with it. `RuntimeManager::new_command_executor` and the
    `ActorRuntime` name the type `CommandExecutor<dyn ChannelCommandProvider>`.

### C. Step 2 - `take_in` stops at a command (`caelumora-runtime`, no change in behaviour)

4.  `intake::take_in` returns what it took the batch to be, a struct in `intake.rs` - `TakenIn {
    request: Option<Envelope>, retries: Vec<Envelope>, command: Option<Envelope> }` is three
    things, so a struct, not a tuple -, and stops at the first `CommandRequested`, leaving what
    follows in the receiver. Until step 4 nothing sends one, so `command` is always `None` and both
    callers (`AgentActor::handle_waiting`, `ExternalAgentActor`'s loop) go on as before.
5.  Both waiting loops become a loop over `take_in`: retries judged and a request kept as today, a
    command carried out at its place (step 6), then `take_in` again while the receiver has more.

### D. Step 3 - the model (`caelumora-model`)

6.  `ActorEvent::CommandRequested { command: String, args: String, agent: ActorId }` - sent by a
    user's actor, routed to `agent` alone; the envelope's sender is the issuer and its security
    context theirs. `ActorEvent::CommandExecuted` gets `to: Option<ActorId>`: the issuer for a
    private answer, `None` for every user - the one event for a user's command run by the session
    (always `None`, sent by the user's actor, as today) and for an agent's (sent by the agent).
    `ActorEvent::CommandFailed { command: String, reason: String, to: ActorId }` - what the agent
    answers when it does not carry the command out: unknown to it, refused, invalid arguments,
    failed; `reason` worded for the user (`CommandError`'s texts), never with the error itself.
    Both answers are shown to users; no agent takes note of them.
7.  `HistoryEntry::CommandExecuted { turn_id, actor, issuer: ActorId, command, args, at }`: the
    channel records it from the agent's `CommandExecuted` as it records `TurnFinished` from the
    event (`ChannelLoop::append_history`), before the turn's end; the agent's own history gets it
    through `AgentChannelState::finish_turn` from `TurnState::command`. Not the output, not a
    failure.
8.  `Command` gets `owners_only: bool` (default `false`): whether the command is for the agent's
    owner and contributors alone, like approving its calls, or for everyone who may write in the
    channel. For a command of the session it is meaningless and `false`.
9.  `ActionType`: `ShowCommand(&str)` and `ExecuteCommand(&str)` stay for the session's commands.
    For an agent's: `ShowAgentCommand { command: &Command, agent: &SecurityAttributes, channel:
    &SecurityAttributes }` and `ExecuteAgentCommand { … }` alike - the ownership policy grants
    `owners_only` to the agent's owner and contributors (`may_read(agent)`, as `ApproveToolCall`),
    the rest to whoever may write in the channel (contributors of the channel). `describe` names
    the command and the agent's id, nothing more. The security suite gets both: a bystander in the
    channel may `/skill` but not `/compact`; the owner may both; who is not in the channel neither.
10. `TurnState` says what kind of turn it is: `command: Option<CommandRequest { command, args,
    issuer: ActorId }>`, `None` for an answer. Set by `begin_turn` from the request; a resumed turn
    with `command` set is a command turn (step 16). A command turn is never retryable:
    `AgentChannelState::answers_again_after` and `retried` refuse it (`RetryRefused`, a reason of
    its own or `Ended`), and `ChannelLoop::given_up_last` passes over it. Stored data changes:
    agent channel states with turns under way are stored anew the next time they are stored, and
    stored histories know no `CommandExecuted` yet; nothing has to be deleted.

### E. Step 4 - the runtime (`caelumora-runtime`)

11. **The facet.** In `crate::command`: `AgentCommandProvider: ProvidesCommands { async fn
    execute(&self, name, args, issuer: &SecurityContext, turn: &mut Turn) -> CommandResult<
    CommandOutput> }`. Registered as a facet in the `ComponentRegistry`; `COMPONENTS.md`'s facet
    table gets its row. `impl CommandExecutor<dyn AgentCommandProvider> { pub async fn
    execute(name, args, issuer, turn) }` - `admit` with the agent-command actions (the agent's
    attributes from its controller, the channel's from the turn), then the provider. Both actors
    of an agent hold one, `commands: CommandExecutor<dyn AgentCommandProvider>`, built where the
    actor is built from `context.all::<dyn AgentCommandProvider>()` of the agent's container, with
    its security manager; two components of one personality providing one name keep the agent from
    starting, as for tools, and so does a name the session's commands have (the collision of issue
    37, between a session and a personality: the session's would hide the agent's).
12. **What an agent understands** is asked of its actor: `Actor::commands(&self) -> Vec<Command>`
    on the trait (`Vec::new()` by default; both agent actors answer their executor's), and
    `ChannelLoop::agent_commands() -> Vec<(ActorId, Vec<Command>)>` over its running agent actors.
    The views carry it: `MemberView::commands()` - those the viewer may see
    (`ShowAgentCommand`, so a hidden command is not listed) -, `MemberSnapshot::commands`, so that
    the web UI completes and `/help` lists them. A stopped channel runs no actors and lists none,
    which is right: nothing could run them.
13. **Sending.** `UserActor::execute_command` asks the session's executor first; `Unknown` there,
    it looks among the channel's agents (`agent_commands`): the command is one of theirs if any
    agent has it, else `Unknown`. The agent is the first word of `args`, `@handle` or `@all`,
    resolved against the members' handles as `/wakeup` does; without one, or with one that names
    no agent of the channel, `CommandError::InvalidArguments` says which handles understand the
    command ("name the agent: /compact @bob, /compact @alice, or @all"); one that names an agent
    without the command is `Unknown` for that agent. For every agent so named the issuer is
    checked here - `ShowAgentCommand` (refused as `Unknown`), `ExecuteAgentCommand`
    (`Forbidden`) - so that the refusal is theirs at once, and `@all` sends to those that pass,
    naming in the reply those it left out; then `CommandRequested { command, args (without the
    handle), agent }` goes out through the actor's transport, one per agent under one `span_id`,
    no `WakeupRequested` ahead of it (the agent runs it asleep). `execute_command` returns
    `CommandReply` with `output: None` and the `span_id`, by which the answers can be told.
14. **Admitting and routing.** `ChannelLoop::admits` (where `RetryRequested` is checked): `agent`
    is an agent actor of the channel - the policy was asked by the sender and is asked again by the
    agent, so the channel asks nothing. `PolicyModerator::route`: `CommandRequested { agent }` →
    `route_to_agent(*agent)`; `CommandExecuted { to: Some(user) }` and `CommandFailed { to }` → that
    user alone (`route_to_user`, new beside `route_to_users`); `CommandExecuted { to: None }` →
    every user, as today. `route_event_impl` keeps waking only for `ResponseRequested` and
    `RetryRequested`, and a quiet channel delivers a `CommandRequested` as it delivers everything
    but a `ResponseRequested`.
15. **Carrying out.** Both agent actors' `event_filter` accept `CommandRequested`. In the loop of
    step 5, for the command `take_in` stopped at, `command(state, &request, output)` - written
    once, in `actor/intake.rs` or a module beside it, given what it needs of either actor (the
    executor, the handle to a `Turn`, the contexts; the agent run here passes its `tool_executor`
    for nothing - a command turn has no tools):
    - `Answering::to(request, …)` for the contexts; `begin_turn` as `request_response` does, with
      no tool sets and `TurnState::command` set, `ResponseStarted` and the agent's state announced;
      no `ContextProvider` - the command assembles nothing for a model unless it calls one itself,
      through the `LLMClient` its component asked for (`/compact`);
    - `self.commands.execute(name, args, &request.security, &mut turn)` - the executor's `admit` in
      the issuer's context as it came, the provider in the turn;
    - `Ok(output)` → `end_turn` (commit, store, announce, `TurnFinished { messages: [] }`), then
      `CommandExecuted { command, output: output.text, to }` in the turn's contexts, `to` the
      issuer for `Private`, `None` for `Channel`; the channel records the history entry from it;
    - `Err(e)` → for a `Failed` the turn is given up (`give_up`, `GivenUpReason::Failed` with the
      user's wording); a refusal and invalid arguments are no failure of the agent - the turn is
      cancelled quietly, `TurnGivenUp` would alarm -; then `CommandFailed { command, reason, to:
      issuer }`. Nothing of a failed command is applied: nothing was committed.
    - A command that arrives while a turn is under way waits in the receiver, as a message does,
      and is taken in when the actor waits again.
16. **Resuming.** Where a turn under way is taken up after a restart (`prepare_resume`, the run
    loop's first step): a turn with `TurnState::command` set runs the command again from the
    start - `admit` with the issuer recorded in the turn's `Work` security context, the provider
    anew - and answers as in step 15; the issuer is told through `to` whether they are still there
    or not (an answer to someone who left is dropped by the moderator, as every route to an absent
    actor is). The external agent, which resumes nothing today, gives such a turn up as it starts.
17. **`/help`** (`help_command`) lists after the session's commands the agents' - "Commands of the
    agents here:" with each command once and the handles that understand it - read through
    `context.channel.agent_commands()` and filtered by `ShowAgentCommand` for the issuer; `/help
    skill` describes one wherever it is found.

### F. The frontends

18. **REST.** `POST …/commands` answers `CommandReplyResponse { command, output: Option<String>,
    span_id: Option<Uuid> }`: `output` for a command the session answered privately, `span_id` for
    one sent to agents, whose answers come on the channel's stream. `GET …/commands` stays the
    session's commands; the agents' are in the member snapshots (`commands`), so the client builds
    its completion from both. The history over REST carries the new entry. `REST-SERVER.md` says
    so.
19. **Web UI.** `CommandExecuted` is shown as today, marked private when `to` is set (the stream
    only carries what reaches this user, so `to` present means "to me"); `CommandFailed` is shown
    as a private notice in the error's style; a `CommandExecuted` entry of the history is shown in
    its place after a reload, as a notice without output. Completion offers the agents' commands
    with the handles that understand them and `@all`. The `types/index.ts` mirrors of the events
    and the history change.
20. **TUI.** Prints the reply as today when `output` is there; shows `CommandExecuted` and
    `CommandFailed` from the stream as it shows `CommandExecuted` now.

### G. Tests

21. `command/mod.rs`: the executor's tests for both provider kinds; a test `AgentCommandProvider`
    (as `Named`) for the rest.
22. `actor/agent` and `actor/external_agent`: a command carried out between two messages of one
    batch leaves the transcript in the order of the history; a command for a sleeping agent runs
    and leaves it asleep; a command during a turn waits for it; a command whose provider fails
    answers `CommandFailed` and commits nothing; a command turn resumed after a restart runs again;
    a command turn cannot be asked for again; an external agent carries a command out and gives
    what it put into the transcript with its next call.
23. `actor/user`: finds a session's command, an agent's, one without a handle, one for `@all`, an
    unknown one; strips the handle from the arguments; checks the issuer before sending.
24. `channel` / `moderator`: `CommandRequested` admitted only for an agent taking part, routed to
    it alone, also while the channel is quiet; answers routed to the issuer alone or to every user;
    the history entry recorded.
25. Security suite: `a_bystander_runs_an_agents_command_but_not_an_owners_only_one`,
    `an_agents_command_runs_for_its_owner_asked_by_the_issuer` (the turn's contexts), and that a
    command hidden from the issuer is unknown to them.

### H. Documents

26. `TOOLS-AND-COMMANDS.md`, *User commands*: "Commands are components" names both facets;
    a new section *Commands of an agent* - how one is addressed (`@handle`, `@all`), sent, carried
    out in a turn by either kind of agent, answered, recorded, who may run it, and that it neither
    wakes nor is responded to; *Seeing and running* gets the agent-command actions; *Private or
    channel-wide answers* the asynchronous answer and `to`. The commands table gets a column *Of*
    (the session / the agent).
27. `COMPONENTS.md`: the facet row and the rename; `CHANNELS.md`: the events in the list of what
    actors send, and the external agent's commands; `AGENT-TURN.md`: a command turn beside an
    answer, and what `TurnState::command` means; `MEMORY.md`, *Terms*: the history holds commands
    carried out; `SECURITY.md`: the two actions under *The ownership policy*; `REST-SERVER.md`,
    `WEB-UI.md` as in F; `ARCHITECTURE.md`, *Agents are made of components*: "User commands … are
    components as well" extended by the commands an agent carries out itself.

### I. Steps

1.  Step 1 (B) and step 2 (C): the two refactorings, reviewed on their own.
2.  Step 3 (D): the model, with the security suite's tests failing until step 4.
3.  Step 4 (E): facet, executor, sending, routing, carrying out, resuming, `/help`.
4.  The frontends (F), the documents (H), the rest of the tests (G).
5.  Then 22 builds `/skill` on it, and 16 `/compact`.

### J. What to watch

- **The duplicate check across the two executors.** A session command and an agent command of the
  same name are found by the session's executor first, so the agent's is unreachable - refused
  where the agent is built, naming both (step 11), as everywhere else.
- **`Answering::to` continues the request's contexts**: the issuer's `SecurityContext` becomes the
  turn's with the agent as caller and its owner as principal, the issuer as initiator. `admit`
  must be given the issuer's context as it came (`request.security`), not the turn's - the
  question is whether *they* may run it.
- **`TurnFinished` with no messages** makes the moderator ask nobody (`ask` over an empty list),
  and the web UI ends an "answer under way" that showed nothing - check that a command turn leaves
  no empty bubble (issue 225's handling covers a turn given up; a finished one without messages
  may not have been seen before).
- **A command turn has no tools and no context**, so `Turn::full_context` is never called; a
  command that calls the model (`/compact`) builds its own input. `TurnState::tools` is empty, and
  `tool_sets` need not be asked - `begin_turn` takes them as a parameter today; a command turn
  passes none.
- **The external agent's first turn.** Nothing in `ExternalAgentActor` has begun a turn on its
  state so far: `processing_state`, `turn_under_way` for the views, `StateChanged`, what its
  `catch_up` and the channel's snapshot assume of a state in a turn - look at each before the first
  command turn runs there, and give the turn up on resume rather than resuming it.
- **Secrets and paths**: `CommandFailed::reason` is the user's wording. A provider's error is
  logged under the envelope's span and reaches nobody else.

## Open questions

1.  **`owners_only` on `Command`, or a separate kind?** A flag reads as a property of the command,
    which it is; a `CommandReach { Members, Owners }` enum would be a type for two values. The flag
    is proposed; whether the policy's two rules are the right two is 14's question and may change
    with roles ([142](142-roles.md)).
2.  **A command and a request in one batch.** Step 5 carries the command out at its place and
    keeps the request for after the batch. The order of the events the user sees is then: the
    command's `ResponseStarted`/`TurnFinished`/`CommandExecuted`, then the answer's. Should the web
    UI fold a command turn into its notice rather than show it as a turn? Decide when it is seen.

Decided on 2026-10-09, with the maintainer, and folded into the text above: the agents' commands
are configured per personality, like tools, and an inheritance between personalities is the answer
to the chore, not a default set; a quiet channel delivers and carries out commands; a command turn
is never retryable; a command carried out is recorded in the history.

## Done when

- `AgentCommandProvider` exists, `CommandExecutor` serves both facets, and a test provider's
  command typed by a user is carried out by the agent - run here or elsewhere - in a turn of its
  own and answered on the issuer's stream.
- `CommandRequested`, `CommandExecuted { to }` and `CommandFailed` are routed as decided, the
  channel admits a request only for an agent taking part, and records what was carried out.
- `ShowAgentCommand` / `ExecuteAgentCommand` carry the agent's and the channel's attributes, the
  ownership policy decides them by `owners_only`, and the security suite pins both rules.
- `/help`, completion, REST, the web UI and the TUI know the agents' commands and show the
  asynchronous answers.
- The documents named under H describe it.

## History

- **2026-10-09** — Issue created, out of the design of 22: a skill a user loads has to reach the
  agent's transcript, which only the agent writes; `/compact` (16) has the same need. Decided with
  the maintainer: an event the user's actor sends and the moderator routes to one agent, which
  carries the command out and answers with an event; agents understand commands through components
  (a facet beside `CommandProvider`, which becomes `ChannelCommandProvider`), not natively, so
  that `/help`, completion and the permission checks stay in one place; a command runs in a turn
  of the agent's own, for external agents too; the agent is always named (`@handle`, `@all`); a
  quiet channel carries commands out; a command turn is never retryable; a command carried out is
  recorded in the history. Plan written against `42a7483`.
- **2026-10-09** — Step 1 (B) done: `CommandProvider` is `ChannelCommandProvider`, with
  `ProvidesCommands` as what every command facet says of its commands, and `CommandExecutor<P>` is
  generic with `visible_by` and `admit` shared and the session's `visible` / `execute` on the
  instantiation. Step 2 (C) is not a step of its own after all: without `CommandRequested` there is
  nothing for `take_in` to stop at, and a `TakenIn` of two fields is today's tuple under a name; it
  is built with step 4, where the third field and the loop over `take_in` arrive together.
- **2026-10-09** — Step 3 (D) done, with three things the plan did not say:
  - `CommandExecuted` carries `args` and `turn_id: Option<TurnId>` besides `to`, since the channel
    records the history entry from the event: `Some` for a command an agent carried out, recorded;
    `None` for one the session ran, not recorded. The issuer is the envelope's security context's
    initiator, not a field.
  - `CommandRequest` keeps the `issuer` with the command and its arguments, so that `finish_turn`
    records the entry from the turn alone; `AgentChannelState::begin_command_turn` begins such a
    turn without tools and `finish_turn` takes the command.
  - A command turn that does not finish leaves no `TurnGivenUp`: nothing was said and nothing
    committed, so there is nothing to record, and `CommandFailed` tells the issuer. That makes
    "never retryable" hold in the model without a rule - no `TurnGivenUp` of a command turn is
    ever the last given up -; only a command turn *under way* has to be refused, in the runtime
    (step 4). Whether a command turn announces `ResponseStarted` at all is decided there too.
  - The policy's rule for `ApproveToolCall` is now `answers_for`, shared with the owners-only
    commands. `HistoryItemResponse` and the web UI's types mirror the new entry and events
    already; nothing stored has to be deleted.
- **2026-10-09** — Step 4 (E) done, except the documents, which come with the frontends. Where it
  differs from the plan:
  - **A command turn is no answer.** It announces neither `ResponseStarted` nor `StateChanged`
    nor `TurnFinished`, asks nobody to respond and is not registered as the turn under way
    (`RunControl::begin_turn`), so it can be neither cancelled nor asked for again - "never
    retryable" holds without a rule. The agent stays in `Idle` or `Sleeping` while it carries one
    out. `/compact` (16), which calls a model, will want a state to show and a way to cancel it;
    that is 16's to add.
  - **Not resumed, dropped.** A command turn is begun and ended between two stores of the agent's
    state, so it is never stored under way; `prepare_resume` drops one it finds, unanswered, as a
    request lost with a restart is. The plan's "run it again" is not needed.
  - **`AgentCommandProvider::execute(name, args, turn)`**, without the issuer: who asked is the
    turn's initiator.
  - **`Actor::commands(issuer)` and `Actor::admit_command(name, issuer)`** on the actor trait, as
    `tool_sets` is, instead of `Actor::commands()` and a second check in the user's actor: the
    check is the agent's executor's, asked by the user's actor before it sends and by the agent
    before it carries out, in one place. `ChannelLoop::agent_commands(issuer)` lists the agents
    with what the issuer may see of them, for `request_command`, `/help` and the views.
  - **`AgentCommands`** (`actor/commands.rs`) holds an agent's executor and what carrying out
    needs - the controller, the principal, the model's profile, the runtime, the agent's state,
    the views - and carries a command out; both kinds of agent hold one. Both run the same short
    loop over `take_in` (`TakenIn::then` joins two batches): five lines twice, which a shared
    intake for both actors would fold.
  - **`CommandReply`** has `output: Option<String>` and `span_id`; for `@all`, `output` names the
    agents the issuer may not ask, the others are asked. `CommandReplyResponse` has `span_id`
    already; `MemberSnapshot::commands` is filled by the views but not yet on
    `ChannelMemberResponse` - that is step F.
  - **A provider stages**, as a tool does; what it writes into the state directly stays when it
    fails. `Turn` has no way yet to stage a message for the transcript alone, which `/skill`
    needs - that is 22's.
- **2026-10-09** — Frontends (F) and documents (H) done. REST: `ChannelMemberResponse::commands`
  (the model's `Command`, as `GET …/commands` gives the channel's), `CommandReplyResponse::span_id`.
  Web UI: the answer of a command an agent carried out is the agent's notice, kept once by its turn
  (`commandNotice`, the same live and from the history, where it has no answer); `CommandFailed` a
  notice for the issuer; completion offers the agents' commands with each handle and `@all`; the
  agent's history panel marks what it carried out. TUI: prints both answers. The documents named
  under H describe it. One thing that shows only now: the answer to a private command reaches the
  issuer alone, but the history records the command for everybody - the others learn of it on
  their next reload, not live. That follows from "the record is everybody's, a private answer the
  issuer's"; a live notice without the answer for the others would close the gap, if it matters.
- **2026-10-09** — Closed: every point of *Done when* holds. The first commands of an agent are
  `/skill` ([22](22-skill-management.md)) and `/compact` ([16](16-context-compression.md)).
