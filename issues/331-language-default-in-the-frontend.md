# 331 — The core falls back to the creator's preferred language

**Status:** done
**Area:** User interaction
**Summary:** The core holds a new channel or session in its creator's preferred language where nothing else is said, a decision that belongs to the frontend.

## The problem

When a channel or a session is created, the core decides its language where the request names
none: `SessionLoop::create_channel` takes the language the creator prefers if `ChannelSpec` names
none, and `SessionManager::create_from_template` always reads the creator's profile and hands
their preferred language to `SessionTemplate::build`, which holds every channel of the template in
it that does not name its own (both through `preferred_language` in
`caelumora-runtime/src/session/mod.rs`, issue [56](56-response-language.md)).

## Why it matters

- Taking the creator's preference where nothing is said is a convenience for the person creating
  something, and belongs where they see what is chosen - in the frontend. The core should create
  what it is asked to create, and not decide on the caller's behalf.
- `None` means two things: "nothing said" and "no language". Someone who prefers a language cannot
  create a channel held in none; issue 56 records this as not done - the language has to be taken
  back after creating.
- The web UI reads the profile anyway to preset the language in the new-channel dialog, so the
  rule is in two places already.

## Considerations

- **Channels.** The web UI already sends the language it presets; the core only drops its
  fallback.
- **Sessions.** `CreateSessionRequest` names no language. Two ways were weighed:
  - An optional `language` in the request, handed to `SessionTemplate::build`, which takes exactly
    this parameter already. One call, atomic, no extra events.
  - Two calls: the frontend creates the session, then sets the preferred language on each of its
    channels that has none, through `PUT …/channels/{cid}/language`. The API does not grow; the
    cost is that it is not atomic (a session whose language could not be set stays without one,
    which the frontend reports) and that every such channel emits a `LanguageChanged` event. No
    turn starts by itself when a session is created, so no agent answers before the language is
    set.
  - **Decided:** two calls.
- **Several frontends.** The TUI and any other client then hold the rule themselves, or create
  sessions and channels without a language. Accepted: the core stays without the decision.
- **Templates.** A channel template that names a language (`ChannelTemplate::language`) keeps it;
  the frontend sets the preferred language only on channels that have none.

## Done when

- The core creates channels and sessions in the language the request or the template names, and in
  none otherwise; it no longer reads the creator's profile for it.
- The web UI presets the preferred language when creating a channel, and sets it on the channels
  without one after creating a session; a failure to set it is reported.
- Creating a channel held in no language by someone who prefers one is possible.
- `docs/CHANNELS.md` (or wherever the channel language is described) says that the default is the
  frontend's.

## History

- **2026-10-12** — Opened from the maintainer's question whether the fallback to the preferred
  language belongs in the UI rather than in the core. Decided: it does, and for sessions the web UI
  sets the language with a second call instead of a new parameter.
- **2026-10-12** — Done. The core no longer reads the creator's profile when it creates a channel or
  a session: `preferred_language` in `session/mod.rs` is gone, and `SessionTemplate::build` and
  `ChannelTemplate::instantiate` take no language - a channel is held in what the request or its
  template names, else in none. The web UI's new-session dialog reads the profile and, after
  creating the session, sets the preferred language on each channel held in none (`PUT
  …/channels/{cid}/language`); a failure is reported and the session stays. The new-channel dialog
  presets it as before. The TUI does the same for the session it creates and logs a failure. With
  this, a channel held in no language can be created by someone who prefers one.
