# 110 — Documentation that contradicts the code

**Status:** done
**Area:** Maintenance

## The problem

- ARCHITECTURE.md:271-274 names `ToolExecutionRequested/Granted/Denied`;
  the code has `ApprovalRequested/Granted/Denied`. ARCHITECTURE.md names
  `ComponentNodeConfig`; the type is `ComponentDescriptor`.
- `session_reader.rs:10` and `private_notes.rs:10` refer to
  `ChannelLoop::may_read`, which does not exist (it is
  `ChannelSessionReader::may_read` / `AccessQuestions::may_channel_read`);
  `channel_history.rs:4` refers to `notes::SessionReader`.
- `agent_turn.rs:290-294`: the doc comment for `record_knowledge` sits above
  `saw_private_knowledge`.
- `actor_impl/agent.rs:219-222` ("checks a cancel() that raced in") - the
  function checks nothing; `707-713` describes `Stopping` as falling through
  to `Idle`, the code goes to `Reflecting`.
- `moderator.rs:85-89`: `RouteDecision::broadcast` doc says only messages set
  it; `TurnFailed` and `CommandExecuted` do too.
- `context.rs:50` "the turn `context`" (the parameter is `turn`); `llm.rs:61`
  "stream of string chunks" (they are `MessageEvent`s);
  `create_plan_tool.rs:42-44` documents the configuration of one binary.
- `EventContext::span_depth` vs. the logging field `depth` named in
  AGENTS.md.
- `issues/INDEX.md`: the text of issue 36 had landed in the line of issue 53
  (fixed with this issue's creation).

## Why it matters

A doc that names a type that does not exist costs the reader a search and
their trust in the next doc.

## Considerations

Fix in one pass; the ones next to code that a structural issue rewrites
(84 for the `Stopping` doc, 100 for the tool docs) can go with it.

## Done when

`grep` for each name above finds only the code.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, section 6).
- **2026-09-29** — Done. ARCHITECTURE.md names `ComponentDescriptor`, and
  its approval flow is rewritten from the code: the approver in the middle,
  `ApprovalRequested/Granted/Denied/TimedOut`, the strategies, and calls
  answered before approval (118). The doc comments point at
  `ChannelSessionReader::may_read`/`AccessQuestions::may_channel_read` and
  `session_reader::SessionReader`; `record_knowledge` has its own comment
  back; `request_response`, `RouteDecision::broadcast`,
  `PromptProvider::get_prompt`, `LLMClient::chat_completion` (now also
  saying that dropping the stream abandons the request) and the plan
  tool's prompt wiring (pointing at 98) describe what the code does. The
  log field is now `span_depth`, like the `EventContext` field, in the
  envelope span and the agent's turn span, and AGENTS.md says so. The
  `Stopping` doc went with 84. A grep for each stale name finds nothing
  outside the issues and the review.
