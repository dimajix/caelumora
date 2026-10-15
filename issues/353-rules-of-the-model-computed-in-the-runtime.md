# 353 — Rules of the model are computed in the runtime: approval strategies, history limits, resuming a state

**Status:** done
**Area:** Code structure

## The problem

ARCHITECTURE.md gives the model the data and the rules, and the runtime what runs. Four pure
functions over model types live in the runtime:

- `ApproverActor::check_strategy` (`actor/approval.rs:260`): what an `ApprovalStrategy` decides
  from the answers of those who may approve. Fifty lines of rule, tested only through the actor.
- `history_start` and `visible_from` (`channel/history.rs:49`, `:69`): where a `HistoryAccess`
  fixes a participant's `HistoryStart`, and what a `HistoryStart` shows of a history. Tested on
  their own already, in the runtime.
- `prepare_resume` (`actor/agent/mod.rs:931`): what a stored `AgentChannelState` is made fit to
  run from - a command turn dropped, a state without a turn set idle, approved calls without a
  result answered as not executed. A free function since [159](159-split-agent-actor-file.md); it
  uses the runtime only for `crate::tool::not_executed_result()`, a text.
- `SessionController::apply` and `AgentController::apply` (`controller/session.rs:211`,
  `controller/agent.rs:104`), and the inline `match` in `ChannelController::update`
  (`controller/channel.rs:240`): how an update event changes the document. The entities already
  own parts of it (`Session::take_in`, `describe`); the rest is in the runtime.

## Why it matters

A rule in the runtime is tested with what runs around it - the approver needs a channel, a user
and a controller to show that "majority" means more than half - and a reader looking for what a
strategy means looks in the model first. `apply` in the model is also what lets the controllers
share one shape ([358](358-controllers-apply-their-events-each-on-its-own.md)).

## Considerations

- `ApprovalStrategy::decide(&self, approvers, responses) -> Option<ApprovalStatus>`, given the
  approvers as `&[ActorId]` and their answers as `&HashMap<ActorId, ApprovalStatus>`, in
  `caelumora-model/src/approval.rs`, with the tests of the four strategies there; the approver
  calls it.
- `HistoryAccess::start_in(&self, history: &[Message], now) -> Option<HistoryStart>` and
  `HistoryStart::visible(&self, items: Vec<HistoryEntry>) -> Vec<HistoryEntry>` in the model, next
  to the types; `channel/history.rs` keeps `VisibleHistory` and the `ChannelLoop` methods.
- `AgentChannelState::prepare_resume(&mut self, not_executed) -> usize`, given what answers a call
  as not executed - a `Fn(&ToolCall, Classification) -> ToolCallResult`, or the text alone: the
  rule in the model, the text in the runtime, as [211](211-turn-and-agent-work-structure.md)
  decided for every text told to a model.
- `Session::apply(&mut self, &SessionUpdateEvent) -> Result<(), ValueError>`, `Channel::apply`,
  `Agent::apply` in the model; the controllers call them. `UserController` applies two events of
  its own in `process_user_update` and would follow.

## Done when

- The four rules are methods of the model types, tested there.
- No behaviour changes; the runtime tests that covered the rules pass unchanged or move with them.

## History

- **2026-10-14** — From the review of agents, sessions and channels, epic
  [349](349-core-review-epic.md).
- **2026-10-14** — Done, as considered:
  - `ApprovalStrategy::decide(&self, approvers, responses)` in `caelumora-model/src/approval.rs`,
    tested there for the four strategies; the approver calls it. The `user_count > 0` of `All` and
    `DenyOnFirst` is gone: no approvers is denied before.
  - `HistoryAccess::start_in` and `HistoryStart::visible` in `caelumora-model/src/channel.rs`, with
    their tests; `channel/history.rs` keeps `VisibleHistory`, `visible_part` - the case without a
    start - and the `ChannelLoop` methods. Its own `said` is gone: `HistoryEntry::said` of the model
    is public now.
  - `AgentChannelState::prepare_resume(&mut self, not_executed: &str)`, given the text alone, which
    stays in the runtime (`not_executed_result`); the `AgentActor` calls it as it resumes (issue
    363). Its test moved to the model, with a case for a command turn stored under way.
  - `Session::apply`, `Channel::apply`, `Agent::apply` and, as the issue foresaw, `User::apply` -
    `&mut self`, `Result<(), ValueError>` -, tested for the session and the channel. The session's
    and the agent's controller keep their `apply`, now cloning and calling the model's, for 358 to
    shape; the channel's `update` calls `Channel::apply`; `UserController::process_user_update`
    calls `User::apply`, and logs a change that does not fit as one of the user, not of the name.

  The documentation of `AgentChannelState::processing_state` said an external agent always stores
  `Idle`; since 363 it stores where it stood and does not resume from it, which it says now.
  EVENTS.md says whose rule applying an update is.
