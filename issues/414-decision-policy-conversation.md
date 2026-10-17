# 414 — The decision policy judges a message without the conversation before it

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Sessions and channels
**Summary:** The `decision_policy` is given only the message just said - or the messages of the turn just ended - without anything said before it and without who said what, so a reply like "Yes, do that" cannot be told apart from any other.

## The problem

The `PolicyModerator` asks its `ResponderPolicy` with the messages of the event it routes alone
(`PolicyModerator::ask`): a user's message on its own, or the messages of one turn. It keeps nothing
of what was said before. The `decision_policy` of [403](403-decision-model-policy.md) joins their
contents into the `state` it sends to the decision model, without their senders. The model is asked
"Which participant of the conversation should respond to the last message?", but sees neither the
conversation nor a participant in it.

403 recorded this as not done: "anything said before the message as context for the model".

## Why it matters

What a message means often depends on what came before: an answer, a follow-up, an agreement. A
decision model judging "Yes, please go ahead" alone chooses by chance; with the planner's proposal
before it, it chooses the coder. Without the sender's name, it cannot tell either whether the coder
reports a finished change (the reviewer should answer) or the reviewer asks for one (the coder
should).

## Considerations

### Where the conversation comes from

*   **(a) The channel passes its recent history to `Moderator::route`.** The history has one owner,
    but every route would copy messages out of the channel's state under its lock, the `Moderator`
    trait changes, and only the policy knows how many it needs.
*   **(b) The moderator keeps a window of the last messages said** and passes it to the policy. It
    sees every message said (`CompleteMessageGenerated`) and every change of one
    (`MessageModified`, `MessageRevoked`, `MessageReclassified`), and applies the change by the
    rules the message keeps itself (`Message::modify`, `revoke`, `reclassify`) - no second copy of
    the history's logic. The window is lost when the moderator is rebuilt or the channel restarted;
    the first decisions afterwards have less context, at worst as today.
*   **(c) The policy reads the channel's history itself** through a view given when it is built: a
    channel has no way yet to give its components access to its own state, the reason 403 rejected
    the same option.

Decided with the maintainer: (b).

A reclassification of the whole channel (`History::fit_into`) is no event and does not reach the
moderator. It only ever lowers a message's classification, so a window that misses it holds
messages classified higher than they are: they are left out of what is sent, never sent where they
may not go.

### What the policy is given

*   The window holds every message heard (`Message::as_heard`), by the name its sender was given
    when it said it - users and the sender itself are no candidates, and have no description the
    policy could take a name from. Its size is a parameter of the moderator, `context_messages`,
    as `max_agent_hops` is; the other policies ignore what they are given.
*   `ResponderPolicy::choose` keeps the messages of the event apart from those before them, so that
    a policy can treat them differently.

### What the decision policy sends

*   One line per message in the `state`: the sender's name and what was said.
*   What was said before is sent as far as it fits: the oldest is left out first, so that the
    `state` stays within a budget of the model's 8,192 tokens.
*   A message of the event that may not flow to `destination` asks as on failure, as before; one
    said before is left out, so that a single old internal message does not keep the policy from
    choosing for good.

## Done when

*   The `policy_moderator` keeps the last `context_messages` heard messages, as they stand after
    every change, and gives them to its policy with the event's own.
*   The `decision_policy` sends them, each with its sender's name, oldest left out first, and those
    that may not flow to `destination` left out.
*   Tests cover the window (its size, a change, a revocation), what the policy is told, and the
    `state` sent.
*   [CHANNELS.md](../docs/CHANNELS.md#moderators) describes the window and what the decision policy
    sends.

## History

2026-10-17: Opened from a question of the maintainer, who expected the moderator to pass the last
messages to its policy. Decided with them: option (b), the window a parameter of the moderator,
the event's messages apart from those before, earlier messages that may not be sent left out.
