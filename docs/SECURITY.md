# Security

Who may do what in Caelumora: the `AccessPolicy` and the policies it applies, ownership of stored
entities, where permission checks belong in the code, and where users, logins and secrets come from.
How far *information* may travel between channels is a separate question, answered by
classifications in [CONFIDENTIALITY.md](CONFIDENTIALITY.md). The settings mentioned here are listed
in [CONFIGURATION.md](CONFIGURATION.md).

## The security model

Security is a context that travels with every operation, and explicit checks against it.

*   **`SecurityContext`** carries four identities, and the session and channel an action happens
    in: the *caller*, who acts; the *owner*, who answers for the caller; the *principal*, in whose
    name - with whose rights - the caller acts; and the *initiator*, who caused it (see [Who acts,
    for whom](#who-acts-for-whom)). There is no delegation chain. It carries the *roles* of the
    owner and the roles the caller is limited to - an agent's personality's. What a decision needs
    to know of the subject is in the context, put there where the context is built - a user's by
    their `UserController`, an agent's by its `AgentController` from its owner's, each with the
    roles as they are now - and not looked up while the decision is made. A context continued for
    another caller (`SecurityContext::next`) carries no roles until whoever continues it gives
    them: the initiator lends none.
*   **`AccessPolicy`** authorizes actions: `check(context, action)`.
*   **`ActionType`** enumerates every securable action. An action on a stored entity carries the
    entity's `SecurityAttributes`, not the entity itself, and is asked alike for every kind
    (issue [323](../issues/323-kinds-enumerated-in-the-core.md)): `ActionType::Entity`, by the
    entity's kind and a `Verb` - `Get`, `Create`, `Update`, `Delete`, `ReadDeleted`, `Restore`,
    `Purge` - and `ActionType::List` by the kind alone. It is named by both in the log and in this
    documentation: `GetSession`, `DeleteWorkspace`; a listing as `List(session)`.
*   **A kind's own verb** - using a skill (`skill::USE`, the plugin's own), writing a workspace's
    files or adding an artifact's version (`WRITE`, the core's, which any kind may use, issue
    328) - is a `CustomVerb`, named likewise (`UseSkill`, `WriteWorkspace`). It says what it
    *requires*, in a right every policy knows: what an entry grants (`Read`, `Write`), or owning
    the entity. A policy decides it by that, so a kind can map
    a verb onto a right but never invent one; a policy that decides one verb of one kind otherwise
    tells it by its kind and name. The verbs every kind has, and the actions of the core that
    concern no single stored entity, a policy decides one by one, so that a policy that forgets one
    does not compile.

### Who acts, for whom

Every action knows four things about who is behind it, also for the record (issues 255, 341). For
a person they coincide: a user acts themselves, answers for themselves, in their own name. For an
agent they fall apart - it is itself, it belongs to someone, it acts in someone's name, and someone
asked it. A bank's agent, asked by a customer, shows all four:

| | the question | a user | a bank's agent, asked by a customer | an agent acting on its own |
|---|---|---|---|---|
| `caller_id` | who acts? | the user | the agent | the agent |
| `owner_id` | who answers for it? | the user | its owner at the bank | its owner |
| `principal_id` | in whose name, with whose rights? | the user | its principal (`Agent::principal_id`): its owner for now, the bank once a team can be one (issue 340) | its principal |
| `initiator_id` | who caused it? | the user | the customer | the agent itself |

Each is read for its own questions, and only for them:

*   **The caller** for what is bound to the one acting: publishing, changing and revoking one's
    own messages, reading what an agent takes part in itself.
*   **The owner** for what the person responsible for the caller decides or bounds: the clearance
    (the context's roles are the owner's), the questions a tool puts to the agent's owner, and whose
    agent a user may take into a session (issue 221).
*   **The principal** for rights: the policy (`OwnershipPolicy::check`), the store filters
    and views, the owner of what is created, the credentials a call reaches with.
*   **The initiator** for the record, and for whom a tool's questions for the initiator are put to
    (see [APPROVALS.md](APPROVALS.md)) - never for rights. It is the root of the chain: when a
    customer asks agent A and A asks agent B, what B does is still initiated by the customer - who
    asked B directly is the sender of the message B answers. An agent that acts when nobody asked
    it initiates what it does itself; its owner caused nothing.

An agent stores its principal (`Agent::principal_id`) beside its owner. It is its owner when the
agent is made, and for now it stays so: the store refuses an agent acting in anybody else's name,
so that whoever may change an agent cannot lend it another's rights. Owner and principal are apart
so that every check says which of them it means; whom an agent may act for other than its owner -
a team, say - is issue [340](../issues/340-teams-as-owners.md).

### Three decisions

Every decision is made from the acting entity's `SecurityContext`, the verb, the object's
`SecurityAttributes`, the acting entity's clearances - its roles, in the context - and the object's
classification, and from nothing else. Nothing is looked up while a decision is made but the groups
an access list names. Three decisions are made apart, each by a function of its own, and an act
passes only where every one it needs says yes:

1.  **Security** - who may do what with the object: the `AccessPolicy`, from the context, the
    verb and the attributes. Access lists name actors, roles and groups (see
    [Access lists](#access-lists)).
2.  **Flow** - whether information may go where the act takes it, and who is cleared for it: the
    `ConfidentialityPolicy`, from the clearances, the object's classification and the
    destination's (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md)).
3.  **Visibility** - what a client or an agent is shown: the views reach each thing by reading it
    (security) and, where it is classified, by the clearance for it (flow); what may not be reached
    is not found (see [Views](#views)). Reaching decides what is visible in a context, never
    whether something is permitted: an operation that reaches an object by its id has exactly the
    rights of one that navigated to it.

### One manager per process

One `AccessPolicy` is built per process and handed to everything that checks - the stores, the
`SecretManager`, the `RuntimeManager` - as a constructor argument rather than built where it is
needed, so that a deployment's policy cannot be silently replaced by a default in one corner of the
runtime. The configuration's `security` section chooses the policy:

*   `ownership` - the default, `OwnershipPolicy` (see below);
*   `allow_all` - `AllowAllPolicy`, which allows every action to everybody; for tests and
    development only. The confidentiality rules apply beside it all the same.

What may flow where, and who is cleared for what, is decided apart from it, by one
`ConfidentialityPolicy` per process, handed to the runtime beside the `AccessPolicy` (see
[CONFIDENTIALITY.md](CONFIDENTIALITY.md#who-applies-the-rules)). The two are orthogonal: an act that
moves information asks both, its own permission first.

## Ownership

Every stored entity - session, channel, agent, personality, session template, workspace, skill,
artifact, group, preconfigured component - carries `SecurityAttributes`: an owner and an access
list. This is the one place ownership and access are recorded; see [Access lists](#access-lists).

*   A session and its channels are owned by the user they were created for, and read by their
    members: an entry naming the group of their participants (see [Groups](#groups)), which follows
    whoever takes part at the time.
*   An agent is owned by whoever created it, and has an empty list: it reads its own definition as
    its owner, whose rights it acts with.
*   A workspace is owned by the user it was made for, always a user; only its owner changes, shares,
    deletes and attaches it to a session (`UpdateWorkspace`). Its files are read by whom its list
    grants reading (`GetWorkspace`) and changed by whom it grants writing (`WriteWorkspace`) - by
    the workspace, wherever they act, cleared for its classification. One made for a session lets
    the group of the session's participants read and write it; one of its owner's grants nobody
    anything until shared. An agent uses it through a channel besides, where the channel uses it and
    the classifications let it (see [CHANNELS.md](CHANNELS.md#its-workspace)). A command of its
    shell is seen only in the channel it was started in, and ended there by whom may change the
    files there - who could have started it.
*   A skill is owned by the user it was made for - by an agent's tool, its owner - or by the system
    for a skill of the seed; whom its list grants reading read and use it. Using it is `UseSkill`,
    granted as reading: what the owner may read, the agent may follow. A user is shown one only if
    cleared for its classification; see [SKILLS.md](SKILLS.md#who-may-do-what).
*   An artifact is owned by the user the agent that published it acts for, always a user; only its
    owner keeps, describes, shares and deletes it. Its own list grants reading it - its name, its
    description, which versions it has - and adding to it; each version has a list of its own,
    which grants reading that version. Reading a version needs both, as a file in HDFS or ADLS
    needs its directory (issue [316](../issues/316-store-checks-artifact-without-versions.md)),
    wherever one acts and cleared for its classification. Publishing a version gives the group of
    the participants of the channel it is published in reading of the version and of the artifact,
    so drafts published in a channel of few reach nobody who reads only the version published to
    many. Sharing the artifact hands whom its list newly names to every version there is, unless
    asked not to; a later version reaches the channel it is published in alone (a default for new
    versions is issue [339](../issues/339-default-access-of-new-versions.md)). Whom the artifact's
    list no longer names reads none of its versions. A version is added by whom the artifact's list
    grants `Write`; a version's list grants reading alone.
*   A team is owned by the user who made it; only its owner renames it, sets its members and deletes
    it, and its members read it by an entry of its own list that names it. The group of the
    participants of a session or channel is owned by the session's owner and changed only by the
    runtime, as someone joins or leaves.
*   Initial data belongs to the system.
*   A `User` record belongs to the system, too: only the `UserManager` writes it, so that nobody can
    grant themselves roles. What a user writes about themselves - how they introduce themselves, the
    language they prefer - lives in a `UserProfile` of its own, whose owner is derived from the user
    rather than stored, so that nobody can write a profile of someone else that is theirs.
*   Introductions - a personality's, a user's profile's - are public: they are given to every agent
    in every channel their agents or their user take part in. A personality's introduction is
    written by whoever may change the personality; a user's by the user. The rest of a profile is
    read by its user alone - their rules on tool calls among it, which the runtime evaluates in
    their name, as their own answer.
*   What a session or channel is for - its summary and briefing - is written by whoever may change
    the place and given to every agent there whose personality wires it in, also to agents owned by
    others. It steers what they say with the weight of the system prompt, never what they may do;
    the prompt part names it as the owner's. A session's descriptions are public to everybody in it,
    external agents included; a channel's are classified as the channel.
*   Of an agent, whoever reads a channel it takes part in also learns, besides its introduction, the
    name of its personality - not the personality itself, which stays its readers' -, whether it
    runs here or elsewhere, and who owns it, by name, also when they share no session with the
    owner: the agent acts with its owner's authority, and who talks to it should know whose. All of
    it is read in the runtime's name, like the names of members.

### The ownership policy

The `OwnershipPolicy` decides by those attributes and the context's principal - a user
themselves, an agent its owner (see [In whose name an agent acts](#in-whose-name-an-agent-acts)):

*   The owner may do everything. Whom an entry grants `Read` may read and use - write into a
    session, cancel or wake a channel, ask an agent in it to answer again (`RetryMessage`), answer
    the questions of an agent's call (`AnswerQuestion` - whether they were asked is the agent's to
    check, not the policy's). What belongs to the system, everybody may read and use.
*   An agent reads (`Get`) what its owner may read, and what it takes part in itself - as the
    caller, a member of the channel or session. Everything else it does, it does as its owner:
    changing, deleting, cancelling and the rest are the principal's.
*   Creating is only allowed with oneself as owner; changing and deleting only for the owner.
*   What was deleted is the owner's to read (`ReadDeleted`) and to restore (`Restore`). Removing it
    for good (`Purge`) is the system's alone - the retention policy's - until there is an
    administrator.
*   A message is its author's to modify and to take back: whoever said it - the context's caller, an
    agent for its own message even when it acts for its owner - not the channel's owner.
*   A command an agent carries out itself is seen and asked for (`ShowAgentCommand`,
    `ExecuteAgentCommand`) by whoever may write in the channel - its members -, unless the command
    is for the agent's owners alone: then by whoever answers for the agent, as `ApproveToolCall` is
    granted. Who may not run one does not see it.
*   A command of the session is decided alike, by the channel it is run in (`ShowCommand`,
    `ExecuteCommand`): by whoever reads the channel, or for a command for the owners alone by the
    channel's owner.
*   Calling a tool (`CallTool`) and reading a secret (`ReadSecret`) are always allowed. They stay
    actions so that another policy can decide them; what decides a call here is the tools offered
    and the rules of its place (see [APPROVALS.md](APPROVALS.md#rules-of-a-place)). A secret has no
    owner to decide by yet, see [Secrets](#secrets).
*   Deleting an agent is its owner's. The channels it takes part in are not changed: it rests in
    them, and that its actors stop there - and start again once it is restored - is done in the
    runtime's name, a consequence of the deletion, not a change of channels owned by others.
*   What lies in a session's or a channel's scope - personalities, session templates, agents,
    workspaces, skills, artifacts - is deleted and restored with it in the name of whoever deletes
    or restores it (issue 320); only finding it is done in the runtime's name. Whoever deletes a
    session deletes what of its scope they may; if something is left - an artifact another
    participant published there, a contributor's agent -, the session is kept, and they are told
    what is left, by kind and count (`SessionError::ScopeNotEmpty`). What they may is deleted all
    the same, so that what is left to do is plain: its owners keep it elsewhere or delete it.
    Whoever restores a session restores it and what of its scope they may; the rest stays deleted
    for its owner to restore. Purging is the system's anyway and done in the runtime's name.
*   The groups of the participants of a session and its channels are not among it: they go and come
    back with the session or channel in the runtime's name, as they are made and changed (issue 348,
    see [Groups](#groups)), whatever whoever deletes or restores it may do with groups.
*   The runtime acting on its own uses `SecurityContext::system()`, which may do everything. This is
    deliberate and never a default: an empty context has no rights.

What may flow where is not the ownership policy's to decide: copying, publishing and the clearance
at a channel's door are asked of the `ConfidentialityPolicy`. Of a reclassification the policy
decides who may - a channel its owner, a message its author or the channel's owner, a note or memory
entry its author -, the `ConfidentialityPolicy` whether the new classification may be given there.
Whether an agent may reach the channel it copies from at all is the policy's, asked before, as
`GetChannel`; see [CONFIDENTIALITY.md](CONFIDENTIALITY.md#who-applies-the-rules).

### How stores check

The stores ask the same way for every entity kind (`store::access`):

*   **Reads** are checked with the attributes as stored. An entity the caller may not read is
    reported as not found, so ids cannot be probed. What lies within an entity under a list of its
    own - an artifact's versions - is the views' to cut from what the store hands out
    (issue [316](../issues/316-store-checks-artifact-without-versions.md)).
*   **The clearance** is asked next of what is classified - a channel, a workspace, a skill, an
    artifact (`Stored::classification`, issue 322): who is not cleared for it does not find it
    either, for every operation below, the deleted included. What the views read in the runtime's
    name they reach themselves: a channel from the reader's place (`view::channel_reading`) - an
    agent may read one it is not cleared for as a whole entry by entry.
*   **Lists** are filtered by what the caller reaches: what they may read and are cleared for. What
    a list returns is the store's to decide; a view does not filter it again.
*   **Changes and deletions** are checked against the attributes *as stored*, never against those of
    the new value, which the caller controls.
*   **Creations** are checked against the new entity's attributes.
*   **What was deleted** is not there for any of these. Reading it as deleted asks `ReadDeleted` and
    answers "not found" alike; restoring asks that and `Restore`, purging that and `Purge`. What is
    not marked is neither restored nor purged.
*   **Names** are checked against everything of the kind, also what the caller may not read: a name
    is unique in its name space or not at all. So a refusal (`NameTaken`) tells that the name is
    taken there - not by what, nor whose. For a name space of the caller's own that is nothing new;
    in the global scope it tells a little more than they may read, which is accepted.

### How the SessionManager checks

What is not a store call asks first and acts afterwards. The `SessionManager` finds a session for a
caller in one place and checks a running session like a stored one - `GetSession`, answering "not
found" alike whether it runs or not. Stopping and deleting check before they stop anything;
restoring and purging reach only what was deleted and leave `Restore` and `Purge` to the store. A
rename is stored first - which is where it is checked - and changes the live entity only then.

### Access lists

An access list holds entries of a subject and a grant. It only adds to what the owner may do -
allow-only: the union of the entries, no denial, no precedence. The owner needs no entry and cannot
lose theirs.

*   **Grants.** `Read` - getting, listing, using - and `Write`, which covers reading and the content
    besides. `Join` lets one take oneself into a session or channel - whoever may change it takes
    in anyone they may name - and nothing else: who may join reads nothing of it before they did,
    and who reads it may not join by that; a personal `Join` entry is an invitation, used up as it
    is taken (see
    [CHANNELS.md](CHANNELS.md#taking-someone-into-the-session)). Changing the attributes, deleting
    and reclassifying stay the owner's.
*   **Subjects**, matched against the context's principal, and for what is read also against its
    caller:
    *   `Actor` - a user. Never an agent, the system or nobody: an agent acts with its owner's
        rights, and an entry for it would let it reach what the person responsible for it may not.
        The stores refuse such an entry.
    *   `Role` - as the context carries it. The roles that count for a user are their own; for an
        agent its owner's, narrowed by its personality's limiting roles wherever a role is matched -
        the one way an agent may do less than its owner.
    *   `Group` - a team, or the participants of a session or a channel. A member counts wherever
        they act, not only in that place.
    *   `Everyone` - every user, an agent by its owner: a session anybody may join, a workspace
        anybody may read.
*   **What the policy learns.** Who belongs to a group is a fact neither the attributes nor the
    context carry. The policy asks a `Memberships` source - the store of groups, read in the
    system's name (`StoreMemberships`) -, only where an entry names a group, and only about that
    one group - never what an actor is a member of, which may be thousands of places. Until the
    source is bound, nobody is a member of anything.
*   **Who may name a group.** Whoever writes an access list names a group in it only if they read
    the group (`GetGroup`): naming one grants its members something, and nobody grants what they
    cannot tell to whom. It is asked after the permission to change the list, and only of a group
    the list did not name before.
*   **Entries for groups that are gone.** An entry naming a group that was purged - a team for
    good, the participants of a session or channel with it - grants nobody anything, but stays in
    the list until it is stored again: whoever stores a workspace or an artifact drops such entries
    from its lists, its versions' included (`group::prune_gone_groups`, issue
    [347](../issues/347-entries-naming-purged-groups.md)), looked for in the system's name. A
    group in the trash keeps its entries, since it comes back with what it belongs to, and so does
    one that cannot be looked up. Until then its owner sees the entry without a name. Sessions and
    channels do not prune: their lists come from their templates and do not grow.
*   **Roles by id.** What is stored names a role by its id; what is written by hand - the user
    providers' configuration, a seed - names roles and is resolved when it is loaded, an unknown
    name being an error.

### Groups

A group (`caelumora_model::group::Group`) is a list of actors that access lists name as one subject,
so that what is shared with a team or with a session's participants follows who belongs to it,
without a copy of the members in every list. There are two kinds, told apart by where a group lies:

*   **A team** is made by a user and lies in the global scope or its owner's; it holds users only
    (an agent is reached through its owner). Its one writer is the `GroupManager`, which leaves
    every permission to the store.
*   **The participants of a session or a channel** lie in its scope and have its id, so that whoever
    names them needs nothing but the session's or channel's id. They hold users and agents, not
    approvers. Their one writer is what changes the participants, writing in the runtime's name
    after the change it follows, which is the act that was permitted. The group is derived anew from
    the participants each time, so that a write that failed is corrected by the next. The
    `GroupManager` refuses to change it.

The group of the participants of a session or channel goes and comes back with it (issue 348): the
session's or channel's own list names it, and its members reach it only while the group is there, so
a session or channel that is there has its group there, too. It is deleted and restored with its
mark, in the runtime's name like every other write of it - not in the name of whoever deletes or
restores the session, as what lies in its scope is (issue 320): it is derived from the participants,
nobody's to keep or leave behind. All of the groups go or none, and if they cannot go, the session
or channel is kept; if they cannot come back, it stays deleted. The record of the deletion
(`Removal::by`) names the system; the session's or channel's names whoever deleted it.

The REST API names the participants of a session or channel by the session's or channel's id
(`kind: session`, `kind: channel`), a team by its own (`kind: group`).

### Not decided yet

*   Roles such as an administrator - a right without an object, a capability of a role.
*   What happens to an owner's entities when the owner is removed. For now they stay.

## In whose name an agent acts

An agent is its owner's delegate. It has no roles and no rights of its own, and whoever asks it
lends it none: it acts with its owner's rights in all it does. It is the model of a person one turns
to because they are entitled to something - they check the request, and carry it out with their own
rights. In three parts:

1.  **The agent acts with its owner's rights.** Every context an agent acts in names it as the
    caller, its owner as owner and its principal - its owner - as principal: the one it speaks in
    when nobody asked (`Agent::security_context`) and the one of a turn, the request's continued as
    the agent. A piece of work has one context: the views a turn navigates and the components it
    calls ask in the turn's; a component's own is only for what it reads while it is built. So what
    the agent reads, the tools it calls, the credentials they reach with and the clearance it is
    judged by are its owner's, whoever asked - within its session too: it reaches another channel if
    its owner may read it or it takes part in it itself, and copies from it what the flow rule lets
    into its own; what it knows itself - its memory, its private notes - it reaches through itself,
    by the flow rule alone. An agent must not see more than the person responsible for it, and it
    passes on whatever it read to whoever it answers next - so what it may read is fixed by its
    owner, not by who happens to ask.
2.  **The owner checks the request**, where a call is admitted (`ToolExecutor::admit`): the policy,
    the owner's rules for the place - evaluated in their name, as their own answer - and the
    approval of the owner or whom the agent's list lets read it, for an agent of the system the
    channel's owner (see [APPROVALS.md](APPROVALS.md#who-may-approve)). A
    personality's limiting roles narrow the agent further. A tool that needs no approval is the
    owner's approval given in advance.
3.  **Whoever asks may ask** - today by taking part in the channel the owner put the agent into.
    What comes of it reaches them only as the flow rule lets it into the channel.

Whoever asked stays in the context as its initiator - the root of the chain, the agent itself when
nobody asked: for the record, and for which request a turn answers - never for rights. The questions
a tool puts to the agent's owner go to the owner, not to whoever asked. What the runtime asks in
someone's name on its own - whether a member of a channel could still start a command there - has
the system as its initiator.

There is no delegation chain. An agent that mentions another does not delegate to it: the other
answers for its own owner, bounded by the channel, with whoever began the conversation as its
initiator. A chain is reconsidered when there are subagents.

**What this accepts.** An owner who puts an agent into a channel without requiring approval lets
every member read through it whatever the agent may read - within the channel's classification,
since nothing flows above it. That is the consequence of the owner's check, not a gap in the rule;
the answer is approval, rules of the place and limiting roles, not the asker's rights. Who may ask
an agent at all could later be decided by its owner, as an action of its own. The security suite
pins these rules.

## Where permission checks go

Every question has one of five reasons to be asked; a question with none is removed. Which operation
asks what, in whose name and by which reason is mapped, check by check, in
[PERMISSION-CHECKS.md](PERMISSION-CHECKS.md).

1.  **The entity layer always asks.** The store asks `Read` on every read and the verb on every
    write, answering "not found" and "denied" (see [How stores check](#how-stores-check)). It is the
    last line, whether or not an operation asked before.
2.  **An operation asks before it stores only if it does something first** - changes another object
    (stops actors, deletes files, makes agents, sends something), or asks something about others
    that the actor could not learn otherwise (a participant's clearance, whether a user exists).
    Then it asks its verb in the order (see [Operations that check
    themselves](#operations-that-check-themselves)), and the store asks again when it stores. That
    pair is the rule, not a double; no token tells the store it was asked already. What it only
    reads before - the object it reached, the flow of what the actor asks for, a group the actor
    names - is no reason: the store's question is then the only one.
3.  **The channel decides what enters it**, once, as it takes it in (see [What enters a
    channel](#what-enters-a-channel)). Actors ask nothing for what they send.
4.  **Every act has one owner, and only the owner asks, once**: a tool call the `ToolExecutor` as it
    admits it, a command of an agent the agent as it carries it out, a command of the session the
    `CommandExecutor`, a secret the `SecretManager`, a skill the `SkillManager`. An answer to an
    approval request is something that enters a channel: the channel asks whether its sender may
    approve the calls of the agent it names (rule 3), the approver only counts it (issue 312). A
    component owns the acts on what it keeps - reclassifying a note, forgetting a memory entry -
    and asks for those itself, in the turn's context; reading beyond its channel it leaves to the
    views.
5.  **Offering is not permitting.** The views, a snapshot's flags, the filters of lists, the
    approver's choice of whom to wait for, the visible commands and the event bus ask to decide what
    to show or whom to wait for, never to permit; the act asks again when it is done, and that is no
    double. The form tells them apart: enforcing asks with `Enforcement::require` and
    `ConfidentialityQuestions::require_*`, offering with `AccessQuestions::may_*` and
    `ConfidentialityQuestions::may_*`.

One enforcing site turns a refusal into an outcome of its own rather than an error: the
`ToolExecutor`, which answers a call refused by the policy as forbidden. It asks as every act does,
so that the refusal is logged alike. `ChannelLoop::reclassify` asks `may_publish` of the old
classification into the new one to learn whether the channel was lowered: a question, not a
permission, and the same one an agent asks at its next step, so that both agree on what was lowered.

### Access questions

A pure access question - "may they?", asked from outside an entity to decide what to list or offer -
is a method of `caelumora_model::security::AccessQuestions`, not of the entity. They are asked by
the views, which decide what a client lists and offers; by the views an agent's turn navigates and
by the components, deciding what an agent may read of other channels or recall, in the context of
the turn; by an agent, deciding what it may put up for approval; and by the approver, deciding whom
it waits for - an answer it then checks as the operation it counts for. An act has no question of
its own: calling a tool, changing a message, reclassifying it are asked with `Enforcement::require`
where they are decided.

The flow is asked alike of the `ConfidentialityPolicy`, through `ConfidentialityQuestions`: the
`may_*` to decide what to offer, the `require_*` for an act, answering a refusal with the act's own
error and logging it alike.

`AccessQuestions` is an extension trait on every `AccessPolicy`. Each question gets a name and
one place, but only translates into an `ActionType`. The decision stays with `check`, so no policy
can answer a question differently from the action.

### Operations that check themselves

An operation that checks its permission as part of what it does - `ChannelLoop::cancel`,
`add_participant`, a store's `update_channel` - keeps that check itself. Only pure questions go into
`AccessQuestions`, and none onto the entities.

Such an operation checks with `Enforcement::require(context, action, refusal)`, another extension
trait on every `AccessPolicy`. It answers a refusal with the operation's own error and logs every
refusal alike, at `debug!`, naming the action and what identifies its object, never content such as
a tool call's arguments. `refusal` is handed what was refused, a `Refused` (`ActionType::refused`:
what the action is about - the kind of an entity, or `context`, `agent`, `command`, … for the core's
own actions - and the action as the log names it). The operation's error carries it as its source,
so that whoever answers the error finds it in the chain, whatever wraps it (issue 323); an
operation that answers a refusal as absent, so as not to tell that the object exists, drops it.

**The order.** An operation reaches its object first; what tells the actor only about their own
request may follow; their own permission comes before anything of others and before anything is
changed:

1.  **Reaching** is reading the object (`Get…`) and, where the object is classified, being cleared
    for it - what the views ask (`channel_reading` for a channel). An operation that names its
    object by id reaches it as one that navigated to it. Who may not reach it is answered "not
    found", so that nothing can be probed; a channel deleted is "gone" only to whom a live one
    would be there. What is read through the store is reached by it, the clearance included (see
    [How stores check](#how-stores-check)); what was deleted is reached as the store lists it.
2.  **The actor's own request** - its form, the structure of the reached object, the flow of what
    they ask for - may be asked next: it tells them nothing they could not know.
3.  **The actor's own permission** comes before anything of others and before anything is changed:
    who may not do it is refused before the rest is looked at, and learns nothing of the others on
    the way. Where nothing of the kind follows, it is the store's question alone (rule 2). Every
    refusal of a permission is "forbidden".
4.  **The others** - a participant's clearance, an agent's personality, whether a user exists -
    and what the operation changes before it stores.

The store asks again as it stores (rule 2): that pair is the rule, not a double.

**Actors are controlled only through such operations.** `Actor::cancel`, `retry_turn`,
`interrupt_tool_call`, `wakeup` and `stop` check nothing: they are the consequence of an act that
was checked - cancelling or waking a channel or an actor, interrupting a tool call, removing a
participant, deleting a channel, a request to answer again the channel admitted - and know no
caller. That only the channel calls them is a rule for the code linked into the program, not a
boundary: the `Actor` trait is public, so that a kind of agent can be built in a crate of its own
(issue 370), as `SecurityContext::system` and the stores are public. Code linked into the program
is trusted; what is kept from it is kept by the API, not by a guarantee. The `Moderator` trait
that is handed actors and `ChannelLoop::actor` stay crate-internal, so that no agent's actor is
handed out: a frontend reaches its user's actor through `ChannelLoop::user_actor`, and acts
through that actor's own methods, never the control methods.

### What enters a channel

The channel decides what enters it: every event an actor sends is admitted or refused by the
channel's loop, in order, as it takes it in (see [CHANNELS.md](CHANNELS.md)) - against the channel
as it is then, so that nothing changes between the check and the entry. The actor that sends asks
nothing of the policy. So a message, a change of one, a reclassification, a request to answer
again, an answer to questions and an answer to an approval request are each asked once, in the
sender's name - first whether it may act so, then the flow of what it carries -, and what is refused
is answered to its sender alone (`MessageRejected`), never written to the history.

## The security suite

`caelumora-runtime/tests/security_suite.rs` runs the whole runtime with the ownership policy, as a
frontend does, with users of different permissions. Its policy records every question it is asked;
each test of an action asserts the questions asked in a user's name, in order, for the action's
owner, someone taking part and a stranger. The suite is the record, in code, of which permissions
each action needs; issue [198](../issues/198-security-scenarios.md) says what is expected and why.

**Every new action that acts in someone's name is added to the suite**, with what it asks and what
it answers to each of them; so is every change to what an existing action asks. A test of the suite
that changes is a change of the permissions an action needs, and is reviewed as one. What the
runtime asks in someone's name on its own, while an action runs, is not the action's: the suite
leaves it out of the action's questions and records it in a test of its own.

The world the suite runs in - the runtime, the recording policy, the users alice, bob and carol, and
the helpers that put what the system starts from into the store - is `caelumora_runtime::testing`,
public behind the feature `test-support` (issue 323). It runs no agent; the actions of agents are
tested in the same world with the agents' plugin and the models of a test
(`agent_testing::world`, issue 370). A plugin's actions are tested in the plugin, in the same world,
taken as a dev-dependency with that feature, and follow the same rule.

### Filters

`AccessPolicy::check` is called directly only by `AccessQuestions`, by `Enforcement` and by
filters: a store filtering a list, the event bus choosing whom an event reaches, the commands a user
is shown.

## Views

What a frontend reads, it reads through the runtime's **views** (`caelumora_runtime::view`): the
sessions, their channels and their members as one viewer navigates them. A view is a node the viewer
stands on; its methods are the edges to the next ones, and each edge asks its own access question,
in the viewer's name:

*   `SessionManager::views(viewer)` is the root, from which the sessions the viewer may read are
    reached. An agent's root is its own session, through its turn; it reads from its channel by the
    confidentiality rules (see [MEMORY.md](MEMORY.md#reading-other-channels)).
*   A session reaches the channels the viewer may read - a user cleared for their classification, an
    agent also by its own taking part and as far as they may be copied into its channel - and, for
    an agent, its own private notes of the session's other channels through itself.
*   A channel reaches its members, also an agent that took part and left, so that what it saw stays
    readable.
*   A member answers what may be read of it: an agent's context to whom may read it
    (`ReadAgentContext`); its tools to whoever reads the agent and every member of the channel,
    since what an agent can do there is no secret among those it works with (`ReadAgentTools`); what
    it was built from to its owner alone, since that is configuration and its preconfigured
    components may have been shared with its owner alone (`ReadAgentComponents`).
*   Workspaces and artifacts are reached by their access lists, wherever the viewer acts, cleared
    for their classification; an artifact's view holds and hands out only the versions the viewer
    may read. Where a version was published and by whom is named only where the viewer reaches that
    channel, so that a version shared beyond it names nothing of it. The viewer's own list of
    artifacts holds those they own and those shared with them, a role of theirs or a team they
    belong to; what reaches them only as a participant of a channel is listed in that channel.
*   Agents are reached on their own by reading them (`GetAgent`), wherever they take part. An
    agent's view reaches the channels it takes part in only as the viewer reaches them through
    their sessions: reading an agent grants nothing of where it takes part. What it is made of is
    named in the runtime's name; the personalities listed to make an agent of are those the viewer
    may read.
*   `Views::channel_of` reaches what a memory entry's origin or a recorded piece of knowledge names,
    wherever it is, and answers `None` for what is gone or not the viewer's to read.

What does not exist and what the viewer may not read are one answer, so that nothing can be probed.
A node is reached through the running loop where there is one and from the stores otherwise, so the
same navigation serves running and stopped sessions alike, and reaching a session does not start it.
Names are read in the runtime's name.

A node answers with a **snapshot** (`caelumora_model::snapshot`) for a client that lists and offers,
with flags such as `can_change` and `can_read_context` decided for the viewer. What else it holds
the caller chooses as `Detail` flags, combined freely, so that what reads every agent's state is
paid only where it is shown. A detail that cannot be read is left out, and the rest is answered. The
REST server only maps snapshots to responses and asks the `AccessPolicy` nothing. A frontend
never asks the access questions itself.

A node is data the viewer was allowed to reach; it decides nothing, and it does nothing: a view is
read, and offers nothing that changes something or acts in someone's name - not a terminal, not a
command - which is its owner's, the manager's. An operation never relies on a node: it checks its
own permission when it acts. Where it acts in a channel, it asks first whether the caller may read
the channel, as the views reach one: `view::channel_reading`, the one place that decides how a
channel is read.

## Users

Where users come from and how their credentials are verified is pluggable, in the same way as
secrets. A `UserProvider` is one source of users; the `UserManager` holds an ordered chain of them,
configured in the `users` section, each a backend that a plugin may add (see
[ARCHITECTURE.md](ARCHITECTURE.md#backends)). The runtime makes the manager when it is built and
offers it (`RuntimeManager::user_manager`), since it records users on the runtime's event bus.

Looking a user up and verifying a credential are separate operations. Verification never hands out
the stored credential, so backends that verify by binding (LDAP) fit the same trait as backends that
compare a hash. How a provider assigns `UserId`s is its own business; the only contract is that the
same user keeps the same id, across restarts included.

**The `UserManager` is the one writer of user records.** A provider says who a user is by its
source - id, name, roles - and writes nothing. What it answers at a login, and when the REST server
asks about the user on renewing a token, the manager compares with the record stored, stores what
changed and announces it, in the system's name. Every `UserController` follows these
announcements, in every instance, so that it holds the roles as they are stored - a role given or
taken takes effect at the next login or renewal, within half a token's lifetime.

### How the chain fails

The user chain fails differently from the secret chain:

*   `Ok(None)` passes the question to the next provider.
*   A rejected credential or a failing provider ends it. A broken user file must deny, never fall
    through.
*   An unknown user is reported exactly like a wrong password.

There is no default provider. The only built-in one so far, `accept_any`, is a development stand-in:
it accepts any username and password and finds users in the `EntityStore`. It must be enabled
explicitly and warns at startup. It gives users the roles its configuration names for them, which
must be configured roles - refused when it is made otherwise; what
roles grant - clearances - is described in [CONFIDENTIALITY.md](CONFIDENTIALITY.md#clearances).

## Authentication

Users are authenticated by the frontend that serves them; the runtime only receives the
`SecurityContext` it builds. The REST server logs a user in through the `UserManager` and keeps the
login as a short-lived signed token in a cookie, renewed by use and bounded in its lifetime, since
it cannot be revoked (see [REST-SERVER.md](REST-SERVER.md#authentication)). The TUI logs nobody in:
it acts as one fixed local user.

## Secrets

Component configuration - such as an `mcp_tool`'s `auth.secret` - may need to reference credentials
without embedding them in persisted configuration. A `SecretProvider` is a pluggable source of
secrets; the `SecretManager` tries its providers in turn and returns the first match. Built in are a
JSON key/value file, re-read on every lookup so that secrets can rotate without a restart, and the
process environment; the `secrets` section sets the chain, each provider a backend that a plugin may
add (see [ARCHITECTURE.md](ARCHITECTURE.md#backends)).

Every lookup is checked with the `AccessPolicy` (`ReadSecret`) before any provider is consulted.
The ownership policy allows every one: a secret is a key and a value, with no owner and no access
list to decide by. So whoever writes a component that names a secret decides which credential it
uses and where it is sent. Until secrets have an owner
([310](../issues/310-secrets-have-no-owner.md)), only the seed and the configuration - whoever runs
the server - may write personalities and components that name secrets; nothing may let users write
them.
