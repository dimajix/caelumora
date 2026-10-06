# 13 — Approval workflow: "Always approve" and "Brave mode"

**Status:** in progress
**Area:** Tools & approvals

## The problem

Approval today is per tool call and per decision. `ApproverActor` collects responses from the human
participants in a channel and resolves them according to the `Approver`'s `ApprovalStrategy` (`Any`,
`Majority`, `All`, `DenyOnFirst`). What it cannot do is remember a decision.

There is no way to say "yes, and stop asking me about this tool", and no way to say "I trust this
session, run everything". Every call to a benign, repeatedly-used tool costs a round trip through a
human.

## Why it matters

Approval fatigue is a security problem, not a convenience problem. A prompt that appears ten times
for `calculator_tool` trains the user to approve without reading, which is exactly the state in
which the eleventh prompt — the one that matters — gets approved too.

## Considerations

- The two modes are different in kind and should not be built as one switch:
  - **Always approve** is a *scoped, remembered decision*: this tool (possibly with these
    arguments), for this agent, in this session or channel, until revoked. It needs a scope model
    and somewhere to persist it.
  - **Brave mode** is a *blanket suspension* of approval for a session. It is trivially
    implementable and correspondingly easy to leave on by accident. It needs to be visible while
    active and should probably expire.
- Scoping "always approve" by tool *name* alone is too coarse — a shell tool approved once is
  approved for every command. Scoping by name plus argument shape is more defensible; deciding how
  far to go is the substance of this issue.
- A remembered approval is a grant, and grants belong in the audit record
  ([05](05-postgres-event-recorder.md)) and under the same access rules as giving the approval in
  the first place ([14](14-approval-privilege.md)).
- Interacts with [15](15-disallowed-tool-errors-to-llm.md): a denial and a never-asked are different
  outcomes and the agent should be able to tell them apart.

## Plan

The concept, decided on 2026-10-06. Issue [150](150-impersonation-towards-the-agent.md) settled what
needs approval by the tool's kind; this issue settles who may say "no need to ask", and who may say
"ask anyway".

### Two kinds of rules

Whether a call is approved is answered by someone who may approve it
([14](14-approval-privilege.md)): the `ApproverActor` has no right of its own. From that follows the
split:

- **Rules of a place** - the system's configuration, a session, a channel - can only *restrict*:
  `Deny` (the call never runs) or `Ask` (a person decides this very call, whatever rules they set
  themselves). They cannot allow, because at a place there is nobody who would stand for an
  approval. Who may set them is the `SecurityManager`'s to say: for a session or a channel its
  owner, as for every other field of the document (`UpdateSession`, `UpdateChannel`).
- **Rules of a person** can `Allow` or `Deny`, each with a reach: everywhere, one session, one
  channel. They are an answer the person gave in advance - "for my part, `ls` is always fine" - and
  they count exactly as far as the person's answer would: for the calls of agents they may approve,
  in the channels they are a member of. Other members may have other rules. The brave mode is such a
  rule: one session, everything allowed, with an expiry; the person stands for it. A rule is not
  bound to an agent; `may_approve` bounds it, and an agent in the reach can be added later if it
  is ever needed.

Every rule is the same shape: a *matcher* (the tool's name and, optionally, a pattern per argument),
a *decision*, a *reach* and an expiry. The matcher is generic over the arguments as JSON - a regex
per argument - which is reliable for structured arguments (`channel`, `id`, a domain) and is not for
an argument that is a language of its own (a shell command, SQL): Claude Code's prefix matching of
shell commands was bypassed by chaining and substitution often enough that it checks for injection
on top. A tool with such an argument proposes a matcher of its own (below). In the model the matcher
is data - it is stored and shown - as an enum with one variant so far; a matcher of another kind is
another variant.

### Where they are evaluated

Rules of a place are evaluated by the `GuardedToolExecutor` in `admit`, not by the approver,
because they decide whether a call is *admitted* to run or to be asked about: the approver only
ever sees calls that already ask, so a `Deny` or `Ask` for a tool whose default is not to ask could
not reach it, and a `Deny` answers the call `Forbidden` - refused by policy, never put to anyone -
not `Denied`, which is what a person says ([15](15-disallowed-tool-errors-to-llm.md)). `admit` is
where the checks of that kind are today (the policy, the call itself, the tool's default), and the
rules go in the same order: policy, `Deny`, the call, then `Ask` and the default together. An `Ask`
marks the `ApprovalRequest` as *explicit*. The approver is not changed.

Rules of a person are evaluated by their `UserActor` when `ApprovalRequested` reaches it - today it
only hands the envelope to whoever watches. A rule that matches answers through `send_approval`, the
way a click does: the approver sees an `ApprovalGranted` or `ApprovalDenied` of a member who may
approve, and `may_approve` and the `ApprovalStrategy` apply unchanged. Two members with opposite
rules are no new case - the strategy decides, as it does between two people: under `Any` an `Allow`
wins whoever answered first, under `All` and `DenyOnFirst` a `Deny` does. An explicit request is
answered by no rule.

Automatic answers arrive at once: under `Any`, a person's `Allow` decides before anyone has read the
request. That is intended and is why an `Allow` rule weighs as much as the button. The flow does not
tell the two apart, but the record must: `ApprovalResponse` says when an answer was given by a rule,
and which, for the audit record ([05](05-postgres-event-recorder.md)) and the UI. The approver
passes it on and does not act on it - it could not police it anyway: a person can always script
their client. Explicitness binds the runtime's rules, not the person.

### Where a rule comes from

A rule of a person is derived from a call: the approval dialog offers "approve, and from now on
...", for a reach the person picks. The tool proposes the matcher, as `describe` already gives the
text an approver is shown: `describe` says what this call does, the matcher says what other calls
are like it (a shell tool proposes the command's prefix, a reading tool the path). Without a
proposal the matcher is the call itself - its name and its arguments, exactly - and the person
widens it. All rules of a person, whatever their reach, are kept in their `UserProfile`: what a
person says about themselves, read by nobody else - a `Session` document is read by every member,
so a person's rule cannot live there. Rules of a place are a field of the place's document, and of
the system's configuration.

### The MCP hints

An MCP tool's `requires_approval` is a setting of its server, today `true` unless configured
([150](150-impersonation-towards-the-agent.md)). The MCP annotations `readOnlyHint` and
`destructiveHint` come from the server and are untrusted; they may set the default per tool - a
tool said to read only does not ask - but the configuration's `requires_approval` stays the last
word, and an explicit `Ask` of a place overrides a hint as it overrides everything. A point of this
issue, not one of its own. A person's `Allow` rule for one tool of a server is, besides, the way to
relax the server-wide default of 150 for that tool alone.

### The design in detail

#### New types (`caelumora-model`, next to `approval.rs`)

```rust
define_id!(ApprovalRuleId);

/// A standing decision on tool calls: a place's restriction, or a person's answer given in advance.
pub struct ApprovalRule {
    pub id: ApprovalRuleId,
    pub matcher: ToolMatcher,
    pub decision: RuleDecision,
    /// Narrower than where the rule is kept: in a profile, a session or a channel; in a
    /// session's or channel's document nothing is narrower, so it stays `Everywhere`.
    pub reach: Reach,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub enum RuleDecision { Allow, Ask, Deny }

pub enum Reach { Everywhere, Session(SessionId), Channel(ChannelId) }

/// What calls a rule is about. One kind so far; a tool that knows its calls better adds another.
pub enum ToolMatcher {
    /// The tool by name (`*`: every tool) and, per argument named, a regex its value must match
    /// as a whole - a scalar as its text, an object or array as its compact JSON. An argument the
    /// call lacks does not match.
    Arguments { tool: String, arguments: BTreeMap<String, String> },
}
```

- `ToolMatcher::exact(call)` - the call itself, its arguments as escaped regexes; the default
  proposal. `ToolMatcher::everything()` - for the brave mode.
- `ToolMatcher::validate()` compiles the regexes and bounds their length; `ApprovalRule::new`
  refuses a matcher that does not validate, so that a stored rule always compiles. Matching uses
  the `regex` crate (linear time, so a pattern cannot be made to run long) and compiles per use;
  caching is a later concern.
- `ApprovalRule::restricting()` - `Ask` or `Deny` with `Reach::Everywhere`: what a place may hold;
  checked when the configuration is read and when a document is changed.
- `ApprovalRule::decide(rules, call, session_id, channel_id, now) -> Option<&ApprovalRule>` - the
  one function both places evaluate with: among the rules in reach and not expired whose matcher
  matches, the strongest decision wins, `Deny` before `Ask` before `Allow`; the rule is returned so
  that the caller knows which (for `by_rule`, and for the log).

#### Existing types extended

The model:

- `AppConfig::approvals: ApprovalsConfig { rules: Vec<ApprovalRule> }` - a new section, every rule
  `restricting()` or the configuration is refused (as `compartments` is checked).
- `Channel::approval_rules`, `Session::approval_rules`, `ChannelTemplate::approval_rules`,
  `SessionTemplate::approval_rules`: `Vec<ApprovalRule>`, serde default, left out when empty; copied
  from the template when a session is made, as `commands` are.
- `ChannelUpdateEvent::ApprovalRulesChanged(Vec<ApprovalRule>)`,
  `SessionUpdateEvent::ApprovalRulesChanged(Vec<ApprovalRule>)`.
- `UserProfile::approval_rules: Vec<ApprovalRule>`, with `add_rule` and `remove_rule`; saving
  prunes what expired. `UserUpdateEvent::ProfileChanged` - without the profile: the global event
  stream reaches every client, and a profile is its user's alone, so the follower re-reads the
  profile from the store rather than taking it from the event. A deliberate exception to "an event
  carries the resulting value", for that reason.
- `ApprovalRequest::explicit: bool` and `ApprovalRequest::suggested: Option<ToolMatcher>` (`None`:
  the call itself), both from `ToolCallState`, which gets the same fields; serde defaults, so that
  stored turns still read. `ApprovalResponse::by_rule: Option<ApprovalRuleId>`, serde default; REST
  never sets it.

The runtime:

- `Admission::Ask { description, explicit, suggested }`; `Admission::state` carries them into the
  `ToolCallState`.
- `GuardedToolExecutor` takes, when made in `AgentActor::new`, the system's rules (from the
  configuration, through the `ActorRuntime`) and the controllers of its channel and its session,
  and reads their rules at every `admit`. `create_actor` has the channel's controller; how the
  session's reaches it - most likely the `ChannelRuntime` of a channel takes its session's
  controller when the session starts it - is settled when it is built.
- `ToolExecutor::matcher(call) -> ToolResult<ToolMatcher>` and `ToolProvider::matcher(name,
  params)`, both with the default `ToolMatcher::exact`; `DefaultToolExecutor` delegates, as it does
  for `describe`. `admit` fills `suggested` with it. The first tool with a matcher of its own is
  step 3.
- `UserController` holds the `UserProfile` beside the `User`, with the entity store and the event
  publisher, like every other controller: `add_approval_rule(ctx, rule)`,
  `remove_approval_rule(ctx, id)`, `update_introduction(..)`, each stored first
  (`UpdateUserProfile`, the user's own), then taken over and announced. The REST route `/profile`
  goes through it. This removes the exception STORAGE.md records ("users are not changed through
  it, so it has no store and announces nothing") and gives a rule added in one channel's dialog to
  the user's actors in all their channels at once, since there is one controller per user and
  instance.
- `UserActor::handle_actor_envelope`, on an `ApprovalRequested` that is not explicit: asks
  `may_approve` for the agent's calls first - an operation checks its permission before it asks
  anything of others - then `decide` over the profile's rules with the request's `session_id` and
  `channel_id`; a hit answers through `send_approval` with `by_rule`, and tells the actor's own
  watchers so (a notice only they get: the UI shows "answered by your rule ..."). The request is
  still handed to the watchers as today. The method becomes async for it.
- `ChannelController::update_approval_rules(ctx, rules)` and the same on the `SessionController`,
  the pattern of `update_language`; both refuse a rule that is not `restricting()`.
- `ApproverActor`: unchanged. `AgentActor`: unchanged but for `Admission::state`.
- `caelumora-tui`: reads the model types, so it compiles once the fields have defaults; it prints
  that a request is explicit and that a decision came by a rule, and manages no rules.

#### The REST API (`caelumora-rest-server`)

- `GET /profile` additionally answers `approval_rules`. `PUT /profile` stays what it is
  (introduction, language) and does not touch the rules.
- `POST /profile/approval-rules` with `{ matcher, decision, reach, expires_at? }` answers `201` and
  the rule with its id; `400 invalid_rule` for a matcher that does not validate or an `Ask`
  (a person's rule allows or denies). `DELETE /profile/approval-rules/{id}` answers `204`, `404`
  if there is no such rule. A sub-resource, not a `PUT` of the whole profile: the dialog adds *one*
  rule, and two channels open at once must not overwrite each other.
- `PUT /sessions/{id}/approval-rules` and `PUT /sessions/{id}/channels/{cid}/approval-rules`, a
  list each, for the owner (`UpdateSession`, `UpdateChannel`; `403` otherwise); `400 invalid_rule`
  for one that is not restricting. The session's and the channel's responses carry
  `approval_rules`; templates and seed files may name them.
- `POST .../approvals` is unchanged; `by_rule` in a body is ignored.
- The event stream carries the new fields: `explicit` and `suggested` on `ApprovalRequested`,
  `by_rule` on the decisions, and the user's own notice of an answer by rule.
- `caelumora-rest-client` and `caelumora-client-api` refer to none of these types; nothing changes
  there. `caelumora-web-ui` mirrors the types.

#### The UI

`ChatView`, the approval card:

- Not explicit: a checkbox "and from now on" opens the derived rule - the tool (fixed), a pattern
  per argument prefilled from `suggested` and editable, the reach (this session / this channel /
  everywhere), an optional expiry. Approve or deny first - the person decides this call now - then
  `POST /profile/approval-rules`; if the rule fails, the answer stands and the failure is reported.
  Denying "from now on" makes a `Deny` rule.
- Explicit: a line saying that this call needs the person's own decision, and no "from now on".
- A request a rule answered shows "answered by your rule ..." where today "you answered" stands,
  from the actor's notice.

The brave mode: a preset in the same dialog - every tool, this session, for an hour - and, while a
rule of the person's with `everything()` and `Allow` is in reach, a badge in the channel's header
("approving everything until ..") with a button that removes the rule.

`ProfileDialog`: the person's rules as a list - tool and patterns, decision, reach by the session's
or channel's name (or "gone", for one deleted since), expiry - each with a button to remove it.

A session's and a channel's rules: a dialog of the owner's, like `LanguageDialog`, listing the
`Ask` and `Deny` rules and adding or removing one.

#### What to take care of

- **Precedence** is `Deny` over `Ask` over the tool's default over `Allow`, and a place's over a
  person's; nothing a person sets opens what a place closes. Explicitness is applied by the
  runtime's `UserActor` only; a scripted client cannot be stopped, and the design does not pretend
  to.
- **Under `Any`**, a lone approver's `Deny` rule denies at once; among several, somebody's `Allow`
  wins. Under `All`, one `Deny` rule blocks every call it matches. The dialog says what a `Deny`
  rule means under the channel's strategy.
- **Matching** is a full match of the whole value, never a search within it; patterns are bounded in
  length, and the `regex` crate has no backtracking. Tools are matched by the name the model calls
  them by. A matcher over an argument that is a language is only as good as its tool's proposal.
- **Expiry and loss of reach** are checked when a rule is used; expired rules are pruned when the
  profile is saved, and a rule for a session or channel that is gone is shown as such and removed
  by its owner - profiles are not touched when a session is deleted.
- **Privacy:** a profile is read by its user alone, so `ProfileChanged` carries no content, and the
  REST responses of a session or channel show the place's rules only, never anyone's.
- **A restart** loses the approver's memory and the turn asks again; a rule answers again, which is
  intended.
- **Stored turns** carry `ToolCallState` with the new fields: serde defaults, nothing else.
- **The record:** the envelope of an answer by rule carries `by_rule` for the recorder; adding and
  removing a rule are announced (`ProfileChanged`, `ApprovalRulesChanged`).
- **Issue 150's decisions stand:** `stop`, `bye` and `silence` need no approval and are not
  affected; the MCP default stays `true` per server, relaxed per tool by a person's `Allow`.

#### Security suite

- A system `Deny` answers `Forbidden` although a member holds an `Allow` rule for the call.
- A channel's `Ask` leaves a request unanswered although a member holds an `Allow` rule for it.
- A member's rule answers for that member only: a request in a channel they are not in stays open.
- A rule of a member who may not approve the agent's calls does not answer.
- An expired rule, and a rule for another session or channel, do not answer.
- A rule that is not `restricting()` is refused for a session, a channel and the configuration.
- The rules of a session or channel are changed by its owner only; a profile's by its user only.
- A `by_rule` sent over REST is dropped.

### Steps

0. **Restructure:** the `UserController` holds the profile, with its store and publisher, and
   `/profile` goes through it; STORAGE.md loses its exception. No change of behaviour, reviewed on
   its own.
1. **Rules of a place:** the types and `decide`; the `approvals` section; the fields of `Channel`,
   `Session` and the templates, their controller methods and events, and their REST endpoints; the
   evaluation in `admit` and `ApprovalRequest::explicit`; the owner's dialog and the explicit line
   in the web UI; CONFIGURATION.md, TOOLS-AND-COMMANDS.md, REST-SERVER.md, WEB-UI.md, SECURITY.md;
   the security suite. After this the net is spanned, without any rule of a person.
2. **Rules of a person:** `UserProfile::approval_rules` and the controller's methods; the REST
   sub-resource; the evaluation in the `UserActor` with `may_approve` and the notice to its
   watchers; `ApprovalResponse::by_rule`; `ApprovalRequest::suggested` with `ToolExecutor::matcher`
   and its default; the approval dialog's "from now on", the brave-mode preset and badge, the list
   in `ProfileDialog`; the TUI's two lines; the documents again; the security suite.
3. **A tool's own matcher**, when a tool with a language for an argument arrives.

## Done when

- A remembered approval can be granted with an explicit scope and revoked.
- Brave mode, if implemented, is visible for as long as it is active and does not outlive its
  session.

## History

- **2026-09-10** — Issue created from `TODO.md`.
- **2026-10-06** — Planned (see *Plan*): rules of a place may only restrict (`Deny`, `Ask`, in
  `admit`), rules of a person may allow and are their answer given in advance (in the `UserActor`,
  so that the approver, `may_approve` and the strategies stay as they are). A rule is derived from a
  call, with a matcher the tool proposes. The MCP hints `readOnlyHint` and `destructiveHint` set a
  tool's default at most; they are a point of this issue rather than one of their own.
- **2026-10-06** — Plan detailed (see *The design in detail*): the types, the fields and methods
  added to the existing ones, the REST endpoints, the UI, what to take care of and the tests. Rules
  of a place are evaluated in `admit`, not by the approver, which only sees calls that already ask
  and could not answer a call `Forbidden`. A person's rules of every reach live in their profile,
  which only they read; the `UserController` takes the profile over first, as a step of its own. A
  rule is not bound to an agent for now.
- **2026-10-06** — Step 0 done: the `UserController` holds the profile beside the user record, with
  the store and the event publisher, and `PUT /profile` goes through it (`update_profile`, the
  introduction checked by `UserProfile::introduce`). It announces the profile as it now stands,
  `GlobalEvent::UserProfileUpdated(UserId, UserProfile)` - a profile is read by its user alone, and
  the bus delivers by `GetUserProfile`, so the event can carry it after all; *The design in detail*
  thought it could not. `GET /profile` still reads the store, as every other reader does.
- **2026-10-06** — Step 1 done: rules of a place. `ApprovalRule`, `ToolMatcher`, `RuleDecision`,
  `Reach` and `ApprovalRule::decide` in `caelumora-model::approval`; the section `approvals` of the
  configuration; `approval_rules` of `Session`, `Channel` and both templates, with their update
  events and controller methods, `ChannelLoop`/`SessionLoop::update_approval_rules`, and `PUT
  …/approval-rules` for both; the `GuardedToolExecutor` applies them in `admit` and again in
  `check`, so that a `deny` set while a call waits refuses it when it would run;
  `ApprovalRequest::explicit`; `ApprovalRulesDialog` in the web UI, a line on an explicit card, and
  one in the TUI. The `GuardedToolExecutor` reads the rules live from the controllers of its
  session and channel: the session's controller is made before its channels and handed to their
  `ChannelRuntime`, as its commands are, and an `AgentActor` is given its channel's controller, as
  the approver is. Deviating from *The design in detail*: `ToolMatcher` is a struct, not an enum of
  one variant - a later variant can make it an untagged enum without changing what is stored;
  `ApprovalRule` has no `created_at`; and a rule whose pattern cannot be matched - only one written
  into a document by hand - restricts and never allows, rather than refusing the document.

