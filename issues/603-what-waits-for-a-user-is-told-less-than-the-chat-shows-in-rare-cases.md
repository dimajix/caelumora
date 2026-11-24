# 603 — What waits for a user is told less than the chat shows in rare cases

**Status:** in progress
**Type:** bug
**Priority:** low
**Area:** User interaction
**Summary:** What issue 594 left open: an approval asked before a user joined, or one asked again after they answered it, or pending in a channel that does not run, is marked in the session list otherwise than the chat shows it; a prompt and the word "mentioned" still say what `@all` and an address meant before; two cases are not tested, and the mark has not been seen in a browser.

## The problem

Issue [594](594-a-user-is-told-of-what-concerns-them-in-no-one-way.md) made what waits for a user
in a channel - the approvals its agents' calls wait for, the questions put to them - the user's
actor's to keep (`UserActor::waiting`), given by a channel's snapshot (`ChannelSnapshot::waiting`)
and announced as it changes (`channel::WAITING_CHANGED`); the web UI marks it in the session list
and notifies of it. What its steps found and left open:

- **An approval asked before the user joined gives no mark.** A user's actor starts empty and
  learns of an approval only as the approver asks it of the users. `ApproverActor` asks nothing
  again as someone joins (`handle_global_event` rechecks its pending approvals on `JOINED`, and
  sends nothing to the newcomer): the chat shows the approval, read from the agents' stored turns
  (`Detail::APPROVALS`), the session list no mark. It arises where a user joins a channel while an
  agent there waits for an approval - not often, but whenever someone is added to a channel at
  work.
- **An approval the user answered can wait for them again.** The actor forgets an approval as the
  user's answer is sent (`UserActor::forward`). An agent actor restarted inside a running channel
  asks its pending approvals again (`request_pending`); the approver, still holding the request,
  announces it again, and the user's actor keeps it as waiting, though their answer still counts.
  Rare: it needs an agent restarted while its channel runs.
- **A channel that does not run gives no mark.** Its chat shows the approvals and questions its
  agents' stored turns wait for; the snapshot gives `waiting` only from a running user actor. As the
  channel runs, the agents ask again and the mark appears. Documented on
  `ChannelSnapshot::waiting`; whether a stopped channel should be marked is not decided.
- **A prompt still says `@all` reaches everyone.** `caelumora-agents/src/prompts.yaml:91` tells an
  agent to write "@all for all of them"; since 594, `@all` in a thread addresses only who takes
  part in that thread - in a sub-agent's thread, not the users of the line.
- **"Mentioned" says what "addressed" means.** Since issue 602 a message *mentions* those named and
  *addresses* everyone a group handle reaches as well (`Message::mentions`,
  `Message::addressees`); 594 renamed the views' wording to "addresses". What is new to a user
  still says `mentioned` - `Unread::mentioned` (`caelumora-domain/src/snapshot.rs:211`),
  `UnreadResponse::mentioned`, the web UI's `Unread.mentioned` and its `@` mark, the doc of
  `ChannelView::unread_in` (`caelumora-kernel/src/view/channel.rs`) and `docs/WEB-UI.md` - though
  it counts `addressees`. The moderators' docs say "mentions" where they read addressees too
  (`policy_moderator.rs:60`, `mentions_only_policy.rs`).
- **Two cases are not tested.** A user not shown the approvals (`ChannelView::shows_approvals`
  false: one who reads the channel from a later point on) is given `waiting.approvals` 0; an
  approval a rule of the user's answers adds and closes in one round of the actor and announces
  nothing.
- **The mark has not been seen in a browser.** The session list's mark and the browser
  notification of what waits were built without a model server that could raise an approval; only
  their functions are tested (`unread.test.ts`, `globalEvents.test.ts`).

## Why it matters

The mark is to tell the user where something waits for them. Where it tells less than the chat, or
more than what still waits, they learn to distrust it - the reason 594 was taken up. The wording
and the prompt mislead whoever reads the code or the agent that writes `@all`.

## Considerations

- **Asking a newcomer.** The approver sends its pending requests to a user who joined - on
  `JOINED`, to them alone - as it announces them again after a restart. The newcomer's actor then
  keeps them, and the frontends show them as any other.
- **An answer given, asked again.** The user's actor remembers the approvals its user answered
  until they end, and leaves out one asked again among them - as the terminal does
  (`Followed::answered_approvals`).
- **A stopped channel.** Either the snapshot reads what waits from the stored turns where the
  channel does not run - two sources for one number -, or the behaviour stays and is documented in
  CHANNELS.md as well.
- **The word.** Renaming `Unread::mentioned` to `addressed` changes the REST API and the web UI
  together; the `@` mark may stay.

## Done when

- The two cases of an approval are decided and built or documented, each held by a test.
- Whether a stopped channel is marked is decided.
- The prompt says what `@all` reaches in a thread.
- What counts `addressees` is called so, or kept as "mentioned" with the reason recorded here.
- The two cases are tested, and the mark and the notification were seen in a browser.

## History

- **2026-11-24** — Opened as the maintainer asked, from the findings of issue 594 left open.
- **2026-11-24** — Decided by the maintainer, as the author recommended: a stopped channel stays
  without a mark - nobody can answer there, and as it runs the agents ask again -, documented in
  CHANNELS.md; `mentioned` is renamed `addressed` wherever it counts `addressees` (`Unread`, its
  REST field, the web UI's `Unread` and `addresses`), the moderators left out - "mention" there is
  writing a handle, and `mentions_only_policy` a kind of the configuration; the mark is checked in
  a browser against a model of Ollama. Built: the approver asks a user who joins every approval
  still pending (see below); the user's actor keeps an approval its user answered
  until it ends, so that one asked anew neither waits for them nor is answered by their rule
  again; the prompt says what `@all` reaches. Not tested but removed: a user with a running actor
  is shown the approvals pending - they take part, but for the moment between their being let go
  and their actor's end, and read from no place, so they read the whole channel -, so the check of
  `shows_approvals` in `ChannelView::waiting` made no difference worth keeping.
- **2026-11-24** — Checked in a browser against `gemma4:26b` of a local Ollama, the app run with
  stores in memory: an approval asked in one channel marked it in the session list with another
  channel on screen, and the mark went as it was approved; the notification of the browser - its
  constructor stood in for, the permission not being given - was made once, titled with the
  channel, its tag the channel's id; a user who joined while the approval waited was told it, and
  the user who knew it already counted it once and was not notified again.
- **2026-11-24** — The review found that asking a newcomer on the global stream's `joined` misses
  them where the approver answers before their actor runs: the stream announces the join before
  the actor is built, and a user's actor reads from the end of the history as it starts. Decided
  by the maintainer, since actors talk through the channel alone (issue 567, point 10): the
  channel delivers a join (`ParticipantJoined`) to the approvers as well, recorded right before
  what the newcomer reads begins; the approver answers a user's join with every approval it holds,
  asked of them alone - `ApprovalRequested` names its one recipient (`to`), as `CommandExecuted`
  does -, so that the others' history holds nothing twice. Rejected: the newcomer's actor asking the
  approver as its run begins - a call past the channel; the approver asking every user again - the
  same request in everyone's history once per join. Taken in as well, as the maintainer asked:
  `ChannelLoop::approval_thread`, by which the user's actor asked the approver directly where an
  approval was asked (issue 602), is gone - the actor keeps the thread with each approval it was
  given (`OpenApproval`). A test through a running channel holds it: a user added while an
  approval waits is told it; it found that the approver's event filter let no join in. Checked in
  a browser again, as before: the mark and the notification as they were; a user who joined while
  the approval waited was told it, the history the others read held their join alone, and the
  user who was there was not notified again; once approved in the chat, the approval waited for
  neither of them.
- **2026-11-24** — A second review found that the channel records a join also where a user who
  takes part is drawn into a thread, and for each further thread a newcomer takes part in, so that
  every pending approval was asked of them again each time; and that one who answered, left and
  came back was asked again. Decided by the maintainer: a join says whether it is one into the
  channel (`ParticipantJoined::into_channel`, `true` for the join told where the newcomer's actor
  starts alone; `false` for one recorded before), and the approver asks only then, and nothing a
  user answered. Rejected: the approver remembering whom it asked - one extra request per user and
  approval all the same, and more state. Also from the review: `to` carries no serde attributes, as
  `CommandExecuted::to`, and an approver's request names a user alone; the documents say how the
  join comes before what the approver answers; `send_approval` is held to answer where an approval
  was asked. Next, as a step of its own: the approver learns of participants from the channel
  alone, no longer from the global stream.
