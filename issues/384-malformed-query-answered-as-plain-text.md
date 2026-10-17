# 384 — A malformed query string is answered in plain text

**Status:** done
**Type:** bug
**Priority:** medium
**Area:** User interaction
**Summary:** A malformed query string, such as `?limit=abc`, is answered with a plain-text `400` instead of problem details with a `code`, since axum's `Query` extractor is used unwrapped.

## The problem

Every error of the REST API is answered as problem details with a `code` (REST-SERVER.md,
*Errors*), and so are a malformed body and a malformed path: `crate::error::Json` and
`crate::error::Path` wrap axum's extractors and turn their rejections into an `ApiError`
(`invalid_request_body`, `invalid_path`). A query string is read with axum's own `Query`, whose
rejection is not turned into one: `GET …/history?limit=abc` is answered `400` with the text "Failed
to deserialize query string: …", without a `code` or a `request_id`. Twelve handlers read a query
this way - the lists with `?deleted=`, the streams with `?snapshot=`, the histories with `?before=`
and `?limit=` ([218](218-paged-histories.md)), the files of a workspace, the candidates of a
session, the sharing of an artifact - in the REST server and in the plugins `caelumora-artifacts`
and `caelumora-skills`.

## Why it matters

- A client that decides by `code`, as REST-SERVER.md asks, finds none; the web UI's error handling
  reads a problem document and gets plain text.
- The answer bypasses the request's id, so it cannot be found in the log by it.
- Since 218 a query carries numbers a client passes on (`?before=`), where a malformed one is more
  likely than a malformed flag.

## Considerations

- **The same as `Json` and `Path`:** a `crate::error::Query<T>`, derived with
  `#[from_request(via(axum::extract::Query), rejection(ApiError))]`, and `From<QueryRejection> for
  ApiError` answering `400 invalid_query`. Every handler imports it in place of axum's; the plugins
  already use the core's `Json` and `Path` and take `Query` from the same place.
- Nothing else changes: the query types and what they default to stay as they are.

## Done when

- Every handler reads its query through the wrapper, and a malformed query is answered as problem
  details with `code` `invalid_query`.
- A test shows it for one handler; REST-SERVER.md names the extractor beside `Json` and `Path`.

## History

- **2026-10-16** — Opened while trying out 218 against a running server: `?limit=abc` was answered
  in plain text.
- **2026-10-17** — Done: `crate::error::Query` and `From<QueryRejection> for ApiError`
  (`400 invalid_query`); the fourteen handlers of the REST server, `caelumora-artifacts` and
  `caelumora-skills` read their query through it. The stand-in of `caelumora-open-terminal` keeps
  axum's, since it imitates another server rather than answering as ours. A test of the channel's
  history shows `?limit=abc` answered as problem details.
