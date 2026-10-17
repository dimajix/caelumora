# 216 — The web UI does not show an agent's reasoning

**Status:** done
**Area:** User interaction
**Summary:** The web UI shows an agent's reasoning nowhere - neither in the chat, while it streams or afterwards, nor in the context panel - although the server hands it out.

## The problem

A model's reasoning ("thinking") is stored in `Message::reasoning`, and since
[61](61-reasoning-leaves-the-agent.md) it is published by its `reasoning_visibility`: reasoning
that is the channel's travels with the message - streamed partials, the complete message, the
history, REST and the event stream - while reasoning that is the owner's is dropped from what is
published and kept in the agent's context.

The web UI shows none of it. Its message types have no reasoning, the chat shows a message's
content and tool calls only, and the context panel ([41](41-show-message-context.md)) leaves out
the reasoning of the messages it lists. What the server already hands out can be read only in the
browser's network tab or in the stored state.

## Why it matters

- Reasoning explains an answer - why an agent chose a tool, what it weighed - and is often the
  quickest way to see where it went wrong.
- Reasoning models think for a long time before the first word of the answer. Without the streamed
  reasoning, the chat shows nothing during exactly the longest wait.

## Considerations

- **Folded into the message.** The reasoning belongs to its message and is shown there, folded,
  to be opened by the user - it is often longer than the answer.
- **While it streams.** Reasoning arrives in partials before the content, so a message under way may
  consist of reasoning alone. The bubble has to show that - folded, with a sign that the agent is
  thinking - and keep it as it was once the content begins. How several answers under way at once
  are kept apart is [203](203-concurrent-answers-in-view.md).
- **Only what is published.** The UI shows what the API hands out; reasoning that is the owner's
  does not reach the chat, and whether it streams live is decided by the agent as before (61).
- **The context panel** could show the reasoning of the messages it lists the same way - folded -
  since it is the owner's full view, where reasoning that is the owner's can be read.
- **Not part of this issue:** who may see reasoning, and whether that needs a permission of its own.
  Today the rules of 61 decide it - the personality's `reasoning_visibility` and what the turn saw.
  A permission per viewer would reopen 61's decision that what appears in a channel depends on the
  channel alone, and is left to an issue of its own, should it be needed.

## Done when

- The web UI shows a message's reasoning folded into the message, to be opened by the user, while
  the message streams as well as afterwards.
- The context panel shows the reasoning of the messages it lists.

## History

- **2026-10-03** — Issue created at the maintainer's request. Found while writing it up: reasoning
  that is the channel's is already in every published message and in the event stream; only the web
  UI does not show it. Narrowed at the maintainer's request to the missing display; who may see
  reasoning, and by which permission, is left out for now.
- **2026-10-03** — Implemented in the web UI. The `Message` type carries `reasoning` and
  `reasoning_visibility`; `MessageView` shows the reasoning folded above the content (`Reasoning`),
  so the chat and the context panel show it the same way, the latter marking reasoning that is the
  owner's. The answer under way in `ChatView` collects the reasoning of its partials as well as
  their content and shows it under "Thinking" until the content begins. Not verified in the browser
  yet: the store held no message with reasoning, and none was sent for the check.
- **2026-10-03** — Closed at the maintainer's request. Left as they are: reasoning opened while
  it streams is folded again once the message is complete, since the partials carry no id of the
  message they become; and reasoning that is the owner's is still shown nowhere for the message it
  belongs to - the context panel shows what the model was given, not the answer itself.
