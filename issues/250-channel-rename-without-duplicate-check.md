# 250 — A channel can be renamed to the name of another channel of its session

**Status:** open
**Area:** Sessions and channels

## The problem

A session's channels are found by name (`SessionLoop::channel_by_name`), and creating or restoring
one refuses a name the session already has (`SessionError::DuplicateChannelName`). Renaming does
not: `ChannelLoop::update_name` stores whatever name it is given, so two channels of one session can
end up with one name - and a lookup by name then finds whichever comes first.

No route of the REST API renames a channel yet, so nobody can do it today; but the method is
public, and the web UI or a tool that renames channels would inherit the gap.

## Considerations

- The check belongs where the others are: the session knows its channels, so renaming would be
  asked of the `SessionLoop` (or checked by it), not of the channel alone - as creating is.
- A rename races a create of the same name; the session's lock for creating channels would have to
  cover it.

## Done when

Renaming a channel to a name another channel of its session has is refused as creating one is, and
a test says so.

## History

- **2026-10-07** — Opened while issue [248](248-validated-text-fields.md) made `update_name` take a
  string: creating and restoring a channel check its name against the session's, renaming does not.
