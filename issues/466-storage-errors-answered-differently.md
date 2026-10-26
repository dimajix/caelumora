# 466 — The same failure of a storage is answered differently over REST

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** User interaction
**Summary:** A file too large, content that changed while it was kept, or a storage that broke is answered with another status and code by the artifacts than by attachments and workspaces, since each kind maps the storage's errors to its own and the REST layer maps each kind's on its own.

## The problem

Three kinds keep files in a storage: the files of a workspace (`TerminalProvider`), the versions of
an artifact and the attachments of a message (both `ContentStorage`, issue
[282](282-message-attachments.md)). The storage fails the same way under each - the file is larger
than allowed, its bytes are not what was announced, the storage broke -, but the client is told
differently:

| Failure | Workspace (`TerminalError`) | Attachment (`AttachmentError`) | Artifact (`ArtifactError`) |
|---|---|---|---|
| too large | `413 file_too_large` | `413 file_too_large` | `400 too_large` |
| changed while kept | - | `400 changed` | `409 changed` |
| storage broke | `503 storage_unavailable` | `503 storage_unavailable` | `500 internal_error` |
| storage not configured | `400 unknown_storage` (named by the client) | `503 storage_unavailable` (named by what is kept) | `400 unknown_storage` |

The cause is the cut of the errors: `ArtifactError` and `AttachmentError` each copy the variants of
`ContentError` (`TooLarge`, `Changed`, `Unavailable`, ...) into their own, and
`caelumora-artifacts/src/routes.rs` and `caelumora-rest-server/src/error.rs` each map their copy to
HTTP. `WorkspaceError` instead carries the `TerminalError` of its storage
(`WorkspaceError::Terminal`), which is mapped once.

## Why it matters

- **A client handles one failure twice.** The web UI uploads a file to a workspace, to an artifact
  and to a message, and has to know three answers for "too large".
- **The answer of a broken storage hides it.** `500 internal_error` logs the cause but tells the
  client nothing about whether to try again, where `503 storage_unavailable` does.
- **One more copy with every kind.** A further kind of content repeats the copy of the variants and
  their mapping, and can differ again.

## Considerations

- **Carry the storage's error, as the workspace does.** `ArtifactError::Content(ContentError)` and
  `AttachmentError::Content(ContentError)`, transparent, and one `From<ContentError> for ApiError`
  beside the one for `TerminalError`. The variants the kinds copy today go away; what each kind
  adds - not configured, not found, classified otherwise - stays its own.
- **Or align only the mapping.** Keep the copies and change the artifacts' answers to those of the
  workspace: smaller, but the copy and the chance to differ again stay.
- **`Changed`: `400` or `409`.** That the bytes are not what was announced is the request's fault
  (`400`); `409` says that the state of the resource conflicts with the request, which it does not.
- **A storage not configured.** Where the client names the storage (a workspace made in it), it is
  the request's fault (`400`); where what is kept names it (an artifact's or attachment's content
  in a storage since removed from the configuration), it is the server's (`503`). The distinction
  is real and stays, but it belongs to where the name comes from, not to the kind.

## Done when

- The same failure of a storage is answered with the same status and code whatever kind it is kept
  for, and the mapping of each failure exists once.
- REST-SERVER.md says how storage failures are answered.

## History

- **2026-10-24** — Opened at the maintainer's request, from the review of step 3 of issue 282,
  where the answers of attachments were made to follow the workspace's and the artifacts' were
  found to differ.
- **2026-10-26** — Step 1, done as recommended to the maintainer: `AttachmentError` and
  `ArtifactError` carry the storage's error (`Content(ContentError)`, transparent) in place of
  their copies of `TooLarge`, `Changed` and `Unavailable`, answered once by
  `From<ContentError> for ApiError` and `From<ContentError> for ToolError`;
  `AttachmentError::is_storage_failure` is gone. A key a storage does not know is one its owner
  made, so `500 internal_error`, not `404`. `UnknownStorage` stays each kind's - issue 282 took it
  out of `ContentError` -, for artifacts and attachments alike `503 storage_unavailable`, its name
  for the log. What a storage reported is shown, as a workspace's is, since
  `ContentError::Unavailable` says what happened, never where (`ContentError::unavailable`, which
  the owner reading a stream now uses too); a key is not (`error::content_message`). The artifact
  tool keeps its hint for `Changed`. REST-SERVER.md says how storage failures are answered.
- **2026-10-26** — Step 2 decided with the maintainer, from the review of step 1: a workspace's
  storage no longer configured is the same failure as an artifact's or an attachment's, but
  `WorkspaceError::UnknownStorage` stands for both cases - the storage the client names when it
  makes a workspace (`WorkspaceManager`) and the one a stored workspace names
  (`WorkspaceController::provider`) -, so the second is answered `400 unknown_storage` with its
  name shown. The two cases become two variants, the second answered `503 storage_unavailable`,
  its name for the log, as an artifact's and an attachment's. And the attachment tool turns what
  a stream of content fails with into a `ToolError` by hand, where `ContentError::unavailable` and
  `From<ContentError> for ToolError` do it.
- **2026-10-26** — Step 2, done as decided with the maintainer: a storage that what is kept names,
  since removed from the configuration, is one type for every kind, `StorageGone` of
  `caelumora_runtime::backend`, found by `NamedBackends::kept_in`; `WorkspaceError`,
  `ArtifactError` and `AttachmentError` carry it (`StorageGone`, transparent) - the latter two in
  place of their `UnknownStorage`, which now only `WorkspaceError` has, for the storage the client
  names (`400 unknown_storage`). It is answered once: `503 storage_unavailable` with its name in
  the log (`From<StorageGone> for ApiError`), hidden by `user_message`, and told the model as a
  fixed text without the name (`From<StorageGone> for ToolError`), as the maintainer decided. What
  a workspace fails with reaches the model through one `From<WorkspaceError> for ToolError`, in
  place of three conversions by hand; the attachment tool reads a stream of content through
  `ContentError::unavailable`. The managers' lookups of a storage by name (`storage`, `provider`)
  are gone.
- **2026-10-26** — Step 3 decided with the maintainer, from the review of step 2: the attachments
  and the artifacts keep the storage new content goes to as its name alone and look it up again
  at every upload and every new artifact - with `NamedBackends::kept_in`, which is for names that
  what is kept gives, and cannot fail there, since `NamedBackends::chosen` checked the name when
  the manager was made. `chosen` answers the backend chosen with its name, and the managers keep
  both, so that new content needs no lookup.
- **2026-10-26** — Step 3, done as decided: `NamedBackends::chosen` answers the backend chosen with
  its name; the `AttachmentManager` and the `ArtifactManager` keep both (`keep_in`), so that an
  upload and a new artifact go to the storage without a lookup, and `NamedBackends::kept_in` is
  asked only for a name that what is kept gives.
