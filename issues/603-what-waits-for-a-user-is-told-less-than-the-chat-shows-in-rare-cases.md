# 603 — What waits for a user is told less than the chat shows in rare cases

**Status:** open
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
